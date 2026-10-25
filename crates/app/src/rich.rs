//! The formatted-document editor: a Document shown in the main text view with paragraph styles,
//! character formats, lists, embedded pictures, charts, formulas, notes and tables, and its own undo.
//!
//! The buffer holds each paragraph on a line of its own (see rich/tags.rs for the tag names). Pictures,
//! formulas and list markers are paintables (they stand on the text's baseline); charts, tables, notes,
//! rules, breaks and kept-as-is parts are widgets at child anchors. Every embedded thing is one U+FFFC
//! in buf.slice(), so offsets in plain() match buffer offsets. The Rust core turns a Document into this form and back (rewrite_core::flat).
//!
//! GTK's own undo forgets formatting and embedded objects, so in this mode every change to the buffer is
//! seen by signal handlers and recorded in History. Code elsewhere keeps calling buf.insert / buf.delete
//! as usual: inserted text takes the look of the text it replaces or follows, and paragraphs are kept
//! whole (one paragraph tag, a list marker at the start of list items).

mod history;
mod tags;

pub use history::{Frag, Item, Op};
pub use tags::*;

use crate::app::{App, AppRc, Snap};
use crate::objects::{Glyphs, View};
use crate::workers;
use gtk4 as gtk;
use gtk4::glib::translate::{from_glib_none, ToGlibPtr};
use gtk4::prelude::*;
use gtk4::{gdk, glib, pango};
use history::History;
use rewrite_core::flat::{self, Flat, Line, Obj, Span};
use rewrite_core::model::{look, Document, PageSetup, Run};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Weak;

/// An embedded object and what shows it in the editor (None while it is not in the buffer).
pub struct Entry {
    pub obj: Obj,
    pub view: Option<View>,
}

struct PendingIns {
    n: usize,
    tags: Vec<String>,
    explicit: bool,
    newline: bool,
}

struct PendingDel {
    cut: (usize, Vec<String>),
    newline: bool,
}

struct PendingTag {
    tag: String,
    before: Vec<(usize, usize)>,
}

pub struct RichEditor {
    app: Weak<App>,
    buf: gtk::TextBuffer,
    text: gtk::TextView,
    active: Cell<bool>,
    /// > 0: changes are not recorded or adjusted (loading, undo itself, raw edits)
    quiet: Cell<u32>,
    /// > 0: inside the handling of a change
    depth: Cell<u32>,
    in_action: Cell<bool>,
    /// normalising paragraphs (its own edits are not normalised again)
    fixing: Cell<bool>,
    history: RefCell<History>,
    objects: RefCell<HashMap<String, Entry>>,
    /// list markers, by the address of their paintable
    markers: RefCell<HashMap<usize, Glyphs>>,
    /// the document without its blocks (title, page, source package ...)
    meta: RefCell<Document>,
    /// F: tags made so far
    fonts: RefCell<HashSet<String>>,
    /// tags whose look is set up
    configured: RefCell<HashSet<String>>,
    /// formatting picked with nothing selected, for the next typed text (key -> value, None = off)
    pending: RefCell<BTreeMap<String, Option<String>>>,
    /// (offset, f: tags) of the latest deletion, so a replacement keeps its look
    last_cut: RefCell<Option<(usize, Vec<String>)>>,
    /// (plain text, fragment) of the latest copy, to paste with formatting
    clip: RefCell<Option<(String, Frag)>>,
    next: Cell<u64>,
    /// the paragraph tag of an empty last paragraph (it has no character to carry it)
    tail: RefCell<Option<String>>,
    /// tags for the next insert, instead of the ones worked out from the neighbours
    explicit: RefCell<Option<Vec<String>>>,
    pend_ins: RefCell<Vec<Option<PendingIns>>>,
    pend_del: RefCell<Vec<Option<PendingDel>>>,
    pend_tag: RefCell<Vec<PendingTag>>,
    css: gtk::CssProvider,
    column: Cell<i32>,
    /// the text view's own left and right margins (paragraph tags include them)
    margins: Cell<(i32, i32)>,
    /// a document tag changed during the current change
    restyled: Cell<bool>,
    /// the clipboard is being set from here
    setting_clip: Cell<bool>,
}

type IterFn = Box<dyn Fn(&mut gtk::TextIter)>;

unsafe extern "C" fn insert_after_cb(
    _buf: *mut gtk::ffi::GtkTextBuffer,
    loc: *mut gtk::ffi::GtkTextIter,
    _text: *const std::ffi::c_char,
    _len: std::ffi::c_int,
    data: glib::ffi::gpointer,
) {
    unsafe {
        let f = &*(data as *const IterFn);
        let mut it: gtk::TextIter = from_glib_none(loc as *const gtk::ffi::GtkTextIter);
        f(&mut it);
        let stash: glib::translate::Stash<*const gtk::ffi::GtkTextIter, gtk::TextIter> = it.to_glib_none();
        std::ptr::copy_nonoverlapping(stash.0, loc, 1);
    }
}

unsafe extern "C" fn delete_after_cb(
    _buf: *mut gtk::ffi::GtkTextBuffer,
    start: *mut gtk::ffi::GtkTextIter,
    end: *mut gtk::ffi::GtkTextIter,
    data: glib::ffi::gpointer,
) {
    unsafe {
        let f = &*(data as *const IterFn);
        let mut it: gtk::TextIter = from_glib_none(start as *const gtk::ffi::GtkTextIter);
        f(&mut it);
        let stash: glib::translate::Stash<*const gtk::ffi::GtkTextIter, gtk::TextIter> = it.to_glib_none();
        std::ptr::copy_nonoverlapping(stash.0, start, 1);
        std::ptr::copy_nonoverlapping(stash.0, end, 1);
    }
}

/// Connect a handler that runs after GTK's own and may change the buffer: the location it is given is
/// written back, so the caller's iterator stays valid.
fn connect_after(buf: &gtk::TextBuffer, signal: &'static std::ffi::CStr, cb: *const (), f: IterFn) {
    unsafe {
        let data = Box::into_raw(Box::new(f)) as glib::ffi::gpointer;
        glib::gobject_ffi::g_signal_connect_data(
            buf.as_ptr() as *mut glib::gobject_ffi::GObject,
            signal.as_ptr(),
            Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(cb)),
            data,
            None,
            glib::gobject_ffi::G_CONNECT_AFTER,
        );
    }
}

fn with(a: &Weak<App>, f: impl FnOnce(&AppRc, &RichEditor)) {
    if let Some(app) = a.upgrade() {
        if let Some(r) = app.rich.get() {
            if r.active.get() {
                f(&app, r);
            }
        }
    }
}

thread_local! {
    static FAMILIES: RefCell<Option<HashSet<String>>> = const { RefCell::new(None) };
}

fn installed(w: &gtk::TextView, family: &str) -> bool {
    FAMILIES.with(|f| {
        f.borrow_mut()
            .get_or_insert_with(|| w.pango_context().list_families().iter().map(|f| f.name().to_string()).collect())
            .contains(family)
    })
}

/// The installed font families (for the font menu).
pub fn families(w: &gtk::TextView) -> HashSet<String> {
    installed(w, "");
    FAMILIES.with(|f| f.borrow().clone().unwrap_or_default())
}

/// A piece of flat text as (text, is a line break) pieces: U+2028 becomes a newline tagged "br".
fn split_breaks(chunk: &str) -> Vec<(String, bool)> {
    let mut out = vec![];
    let parts: Vec<&str> = chunk.split(flat::BREAK).collect();
    for (k, part) in parts.iter().enumerate() {
        if !part.is_empty() {
            out.push((part.to_string(), false));
        }
        if k + 1 < parts.len() {
            out.push(("\n".to_string(), true));
        }
    }
    out
}

impl RichEditor {
    pub fn new(app: &AppRc) -> RichEditor {
        let r = RichEditor {
            app: app.weak(),
            buf: app.buf.clone(),
            text: app.text.clone(),
            active: Cell::new(false),
            quiet: Cell::new(0),
            depth: Cell::new(0),
            in_action: Cell::new(false),
            fixing: Cell::new(false),
            history: RefCell::new(History::default()),
            objects: RefCell::new(HashMap::new()),
            markers: RefCell::new(HashMap::new()),
            meta: RefCell::new(Document::default()),
            fonts: RefCell::new(HashSet::new()),
            configured: RefCell::new(HashSet::new()),
            pending: RefCell::new(BTreeMap::new()),
            last_cut: RefCell::new(None),
            clip: RefCell::new(None),
            next: Cell::new(0),
            tail: RefCell::new(None),
            explicit: RefCell::new(None),
            pend_ins: RefCell::new(vec![]),
            pend_del: RefCell::new(vec![]),
            pend_tag: RefCell::new(vec![]),
            css: gtk::CssProvider::new(),
            column: Cell::new(0),
            margins: Cell::new((0, 0)),
            restyled: Cell::new(false),
            setting_clip: Cell::new(false),
        };
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &r.css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
        }
        r.connect(app);
        r
    }

    fn app(&self) -> AppRc {
        self.app.upgrade().expect("the app is alive")
    }

    pub fn active(&self) -> bool {
        self.active.get()
    }

    // -- switching the mode on and off ---------------------------------------------------------

    /// Take over the editor (formatted mode starts).
    pub fn attach(&self) {
        if self.active.get() {
            return;
        }
        self.buf.set_enable_undo(false);
        self.active.set(true);
        self.load_css();
    }

    /// Give the editor back as a plain text view, emptied.
    pub fn detach(&self) {
        if !self.active.get() {
            return;
        }
        self.quiet.set(self.quiet.get() + 1);
        self.clear();
        self.quiet.set(self.quiet.get() - 1);
        self.active.set(false);
        self.buf.set_enable_undo(true);
    }

    fn clear(&self) {
        let (mut a, mut b) = self.buf.bounds();
        self.buf.delete(&mut a, &mut b);
        let table = self.buf.tag_table();
        let mut names: Vec<String> = self.configured.borrow().iter().cloned().collect();
        names.extend(self.fonts.borrow().iter().cloned());
        names.extend(["br", "br:end", "br:start"].map(String::from));
        for n in names {
            if let Some(t) = table.lookup(&n) {
                table.remove(&t);
            }
        }
        self.objects.borrow_mut().clear();
        self.markers.borrow_mut().clear();
        self.fonts.borrow_mut().clear();
        self.configured.borrow_mut().clear();
        self.history.borrow_mut().reset();
        self.pending.borrow_mut().clear();
        *self.last_cut.borrow_mut() = None;
        *self.tail.borrow_mut() = None;
        self.pend_ins.borrow_mut().clear();
        self.pend_del.borrow_mut().clear();
        self.pend_tag.borrow_mut().clear();
    }

    // -- signals, keys and clicks ----------------------------------------------------------------

    fn connect(&self, app: &AppRc) {
        let buf = &self.buf;
        let a = app.weak();
        buf.connect_insert_text(move |_, it, text| with(&a, |_, r| r.on_insert_before(it.offset() as usize, text)));
        let a = app.weak();
        connect_after(
            buf,
            c"insert-text",
            insert_after_cb as *const (),
            Box::new(move |it| with(&a, |_, r| r.on_insert_after(it))),
        );
        let a = app.weak();
        buf.connect_delete_range(move |_, s, e| with(&a, |_, r| r.on_delete_before(s.offset() as usize, e.offset() as usize)));
        let a = app.weak();
        connect_after(
            buf,
            c"delete-range",
            delete_after_cb as *const (),
            Box::new(move |it| with(&a, |_, r| r.on_delete_after(it))),
        );
        for (sig, _) in [("apply-tag", true), ("remove-tag", false)] {
            let a = app.weak();
            buf.connect_local(sig, false, move |v| {
                if let (Ok(tag), Ok(s), Ok(e)) = (v[1].get::<gtk::TextTag>(), v[2].get::<gtk::TextIter>(), v[3].get::<gtk::TextIter>()) {
                    with(&a, |_, r| r.on_tag_before(&tag, s.offset() as usize, e.offset() as usize));
                }
                None
            });
            let a = app.weak();
            buf.connect_local(sig, true, move |v| {
                if let (Ok(tag), Ok(s), Ok(e)) = (v[1].get::<gtk::TextTag>(), v[2].get::<gtk::TextIter>(), v[3].get::<gtk::TextIter>()) {
                    with(&a, |_, r| r.on_tag_after(&tag, s.offset() as usize, e.offset() as usize));
                }
                None
            });
        }
        for sig in ["insert-child-anchor", "insert-paintable"] {
            let a = app.weak();
            buf.connect_local(sig, true, move |v| {
                if let Ok(it) = v[1].get::<gtk::TextIter>() {
                    with(&a, |_, r| r.on_embed_after(it.offset() as usize));
                }
                None
            });
        }
        let a = app.weak();
        buf.connect_begin_user_action(move |_| with(&a, |_, r| r.in_action.set(true)));
        let a = app.weak();
        buf.connect_end_user_action(move |_| {
            with(&a, |_, r| {
                r.in_action.set(false);
                if r.depth.get() == 0 {
                    r.history.borrow_mut().finish();
                }
            })
        });
        // the format bar follows the cursor
        let a = app.weak();
        buf.connect_mark_set(move |_, _, mark| {
            if mark.name().as_deref() == Some("insert") {
                with(&a, |_, r| r.changed());
            }
        });

        // keys of this mode
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let a = app.weak();
        keys.connect_key_pressed(move |_, key, _, state| {
            let mut stop = false;
            with(&a, |app, r| {
                if app.text.is_focus() && !app.busy() {
                    stop = r.on_key(key, state);
                }
            });
            if stop { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        self.text.add_controller(keys);

        // right-click (and Ctrl+click on macOS): the menus of objects and table cells
        // (on the scroller around the editor, so that it comes before the editor's own context menu)
        let menu = gtk::GestureClick::new();
        menu.set_button(gdk::BUTTON_SECONDARY);
        menu.set_propagation_phase(gtk::PropagationPhase::Capture);
        let a = app.weak();
        menu.connect_pressed(move |g, _, x, y| {
            with(&a, |_, r| {
                let Some(w) = g.widget() else { return };
                let Some(p) = w.compute_point(&r.text, &gtk4::graphene::Point::new(x as f32, y as f32)) else { return };
                if r.context_click(p.x() as f64, p.y() as f64) {
                    g.set_state(gtk::EventSequenceState::Claimed);
                }
            })
        });
        match app.w.get() {
            Some(w) => w.scroller.add_controller(menu),
            None => self.text.add_controller(menu),
        }
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let a = app.weak();
        click.connect_pressed(move |g, n, x, y| {
            with(&a, |app, r| {
                let state = g.current_event_state();
                let primary = if cfg!(target_os = "macos") { gdk::ModifierType::META_MASK } else { gdk::ModifierType::CONTROL_MASK };
                if state.contains(primary) {
                    if let Some(url) = r.link_at(x, y) {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        app.open_link(&url);
                        return;
                    }
                }
                if cfg!(target_os = "macos") && state.contains(gdk::ModifierType::CONTROL_MASK) && r.context_click(x, y) {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
                if n == 2 {
                    if let Some(oid) = r.picture_at(x, y) {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        let app = app.clone();
                        glib::idle_add_local_once(move || app.rich_edit_object(&oid));
                        return;
                    }
                }
                r.pending.borrow_mut().clear();
                let a = app.weak();
                glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || with(&a, |_, r| r.skip_marker()));
            })
        });
        self.text.add_controller(click);

        // a formula under the pointer: a hint and the hand
        let hover = gtk::EventControllerMotion::new();
        let over = std::rc::Rc::new(Cell::new(false));
        let set_over = {
            let (over, text) = (over.clone(), self.text.clone());
            move |on: bool| {
                if over.replace(on) != on {
                    text.set_tooltip_text(if on { Some("Formula — double-click to edit") } else { None });
                    text.set_cursor_from_name(Some(if on { "pointer" } else { "text" }));
                }
            }
        };
        let a = app.weak();
        let set = set_over.clone();
        hover.connect_motion(move |_, x, y| {
            let mut on = false;
            with(&a, |_, r| on = r.formula_at(x, y));
            set(on);
        });
        hover.connect_leave(move |_| set_over(false));
        self.text.add_controller(hover);

        // the clipboard keeps the formatting of what was copied here
        let a = app.weak();
        self.text.connect_copy_clipboard(move |tv| {
            with(&a, |_, r| {
                if r.copy(false) {
                    tv.stop_signal_emission_by_name("copy-clipboard");
                }
            })
        });
        let a = app.weak();
        self.text.connect_cut_clipboard(move |tv| {
            with(&a, |_, r| {
                if r.copy(true) {
                    tv.stop_signal_emission_by_name("cut-clipboard");
                }
            })
        });
        // a copy made by calling the buffer directly (the Edit menu, the context menu) would carry
        // GTK's own copy of the range, pictures and all: keep it as plain text plus our fragment
        let a = app.weak();
        self.text.clipboard().connect_changed(move |cb| with(&a, |_, r| r.on_clipboard(cb)));
        let a = app.weak();
        self.text.connect_paste_clipboard(move |tv| {
            let mut stop = false;
            with(&a, |app, r| {
                if r.clip.borrow().is_some() {
                    stop = true;
                    let cb = tv.clipboard();
                    let a = app.weak();
                    glib::spawn_future_local(async move {
                        let got = cb.read_text_future().await.ok().flatten().map(|s| s.to_string());
                        with(&a, |_, r| r.paste(got));
                    });
                }
            });
            if stop {
                tv.stop_signal_emission_by_name("paste-clipboard");
            }
        });

        // objects follow the width of the text column
        if let Some(w) = app.w.get() {
            let a = app.weak();
            w.scroller.hadjustment().connect_page_size_notify(move |_| {
                let a2 = a.clone();
                // after the app has centred the column
                glib::idle_add_local_once(move || with(&a2, |_, r| r.check_margins()));
                let a = a.clone();
                workers::later("rich-resize", 200, move || with(&a, |_, r| r.on_resize()));
            });
        }
        let a = app.weak();
        app.theme().on_change(move || {
            if let Some(app) = a.upgrade() {
                if let Some(r) = app.rich.get() {
                    r.load_css();
                }
            }
        });
    }

    /// The view's margins changed (the column is centred): paragraph tags include them.
    fn check_margins(&self) {
        let m = (self.text.left_margin(), self.text.right_margin());
        if m != self.margins.get() {
            self.margins.set(m);
            for p in self.p_tags() {
                self.style_ptag(&p);
            }
        }
    }

    fn on_resize(&self) {
        let col = self.column_px() as i32;
        if col != self.column.get() {
            self.column.set(col);
            self.redraw_all();
        }
    }

    // -- loading and saving ------------------------------------------------------------------------

    /// Show a Document (replacing everything).
    pub fn load(&self, doc: &Document) {
        self.attach();
        *self.meta.borrow_mut() = Document { blocks: vec![], ..doc.clone() };
        if self.meta.borrow().size <= 0.0 {
            self.meta.borrow_mut().size = 11.0;
        }
        let f = flat::flatten(doc);
        self.quiet.set(self.quiet.get() + 1);
        self.clear();
        self.fill(&f);
        self.quiet.set(self.quiet.get() - 1);
        self.history.borrow_mut().reset();
        self.break_spacing(0, self.len());
        self.buf.place_cursor(&self.buf.start_iter());
        self.skip_marker();
        self.column.set(self.column_px() as i32);
        self.load_css();
        self.renumber();
    }

    fn fill(&self, f: &Flat) {
        let mut spans: Vec<&Span> = f.spans.iter().collect();
        spans.sort_by_key(|s| s.start);
        let chars: Vec<char> = f.text.chars().collect();
        let mut objects = f.objects.iter();
        let mut si = 0usize;
        let mut pos = 0usize;
        let nlines = f.text.split('\n').count();
        for (k, line) in f.text.split('\n').enumerate() {
            let attrs = f.lines.get(k).map(PAttrs::of_line).unwrap_or_default();
            let pt = attrs.tag();
            self.ensure_ptag(&pt);
            let start = self.len();
            if attrs.kind.is_some() {
                let label = f.labels.get(k).cloned().flatten();
                self.insert_marker(self.len(), label.as_deref(), &[], false);
            }
            let len = line.chars().count();
            let mut seg = pos;
            let mut flush = |upto: usize, seg: usize| {
                let mut i = seg;
                while i < upto {
                    while si < spans.len() && spans[si].end <= i {
                        si += 1;
                    }
                    let (j, fmt) = if si < spans.len() && spans[si].start <= i {
                        (upto.min(spans[si].end), spans[si].fmt.clone())
                    } else {
                        (if si < spans.len() { upto.min(spans[si].start) } else { upto }, Run::default())
                    };
                    let chunk: String = chars[i..j].iter().collect();
                    let ft = ftags(&fmt);
                    let font = self.font_tag(&pt, &ft);
                    for (piece, br) in split_breaks(&chunk) {
                        let mut names = ft.clone();
                        names.push(pt.clone());
                        names.push(font.clone());
                        if br {
                            names.push("br".into());
                        }
                        self.insert_end(&piece, &names);
                    }
                    i = j.max(i + 1);
                }
            };
            for (i, ch) in line.chars().enumerate() {
                if ch == OBJ {
                    flush(pos + i, seg);
                    let obj = objects.next().cloned().unwrap_or(Obj::Opaque(Default::default()));
                    let oid = self.new_id();
                    self.create(&oid, obj, self.len(), &pt, &[]);
                    seg = pos + i + 1;
                }
            }
            flush(pos + len, seg);
            pos += len + 1;
            let last = k + 1 == nlines;
            if !last {
                self.insert_end("\n", &[pt.clone()]);
            }
            let end = self.len();
            if end > start {
                self.apply(&pt, start, end);
            }
            if last && end == start {
                *self.tail.borrow_mut() = Some(pt.clone());
            }
        }
    }

    fn insert_end(&self, s: &str, names: &[String]) {
        let table = self.buf.tag_table();
        for n in names {
            self.ensure(n);
        }
        let tags: Vec<gtk::TextTag> = names.iter().filter_map(|n| table.lookup(n)).collect();
        let refs: Vec<&gtk::TextTag> = tags.iter().collect();
        let mut end = self.buf.end_iter();
        self.buf.insert_with_tags(&mut end, s, &refs);
    }

    /// The Document the editor shows.
    pub fn document(&self) -> Document {
        flat::build(&self.flat(), self.meta.borrow().clone())
    }

    /// The editor's content in the Rust core's editor form.
    pub fn flat(&self) -> Flat {
        let mut f = Flat::default();
        let mut n = 0usize;
        let mut cur_p: Option<String> = None;
        let mut span: Option<Span> = None;
        let mut spans: Vec<Span> = vec![];
        let end_line = |cur_p: &Option<String>, lines: &mut Vec<Line>| {
            lines.push(pattrs(cur_p.as_deref().unwrap_or("P:normal")).line());
        };
        let len = self.len();
        for (s, e, tags) in self.segments(0, len) {
            if let Some(p) = tags.iter().find(|t| t.starts_with("P:")) {
                cur_p = Some(p.clone());
            }
            let fs: Vec<String> = tags.iter().filter(|t| t.starts_with("f:")).cloned().collect();
            let br = tags.iter().any(|t| t == "br");
            let text = self.slice(s, e);
            for (k, ch) in text.chars().enumerate() {
                let off = s + k;
                match ch {
                    OBJ => {
                        if self.is_marker(off) {
                            continue;
                        }
                        let Some(oid) = self.object_at(off) else { continue };
                        self.sync(&oid);
                        if let Some(o) = self.objects.borrow().get(&oid) {
                            f.text.push(OBJ);
                            f.objects.push(o.obj.clone());
                            n += 1;
                        }
                    }
                    '\n' => {
                        if br {
                            f.text.push(flat::BREAK);
                        } else {
                            end_line(&cur_p, &mut f.lines);
                            f.text.push('\n');
                        }
                        n += 1;
                    }
                    c => {
                        f.text.push(c);
                        if !fs.is_empty() {
                            let fmt = fmt_of(&fs);
                            match &mut span {
                                Some(sp) if sp.end == n && sp.fmt.same_format(&fmt) => sp.end = n + 1,
                                _ => {
                                    if let Some(sp) = span.take() {
                                        spans.push(sp);
                                    }
                                    span = Some(Span { start: n, end: n + 1, fmt });
                                }
                            }
                        }
                        n += 1;
                    }
                }
            }
        }
        if let Some(sp) = span.take() {
            spans.push(sp);
        }
        // the last line: an empty last paragraph keeps its own tag
        let last_empty = len == 0 || self.char_at(len - 1) == Some('\n');
        if last_empty {
            let t = self.tail.borrow().clone();
            end_line(&t.or_else(|| Some(ptag_default())), &mut f.lines);
        } else {
            end_line(&cur_p, &mut f.lines);
        }
        f.spans = spans;
        f
    }

    /// The text with one U+FFFC per embedded object (and list marker), matching buffer offsets.
    pub fn plain(&self) -> String {
        self.buf.slice(&self.buf.start_iter(), &self.buf.end_iter(), true).to_string()
    }

    /// Spans the rewriter must leave alone (objects, list markers, code and maths), in character offsets.
    pub fn protected(&self, snap: &Snap) -> Vec<(usize, usize)> {
        let mut spans = vec![];
        for (i, c) in snap.text.chars().enumerate() {
            if c == OBJ {
                spans.push((i, i + 1));
            }
        }
        let len = self.len();
        let names: Vec<String> = self.configured.borrow().iter().cloned().collect();
        for name in names {
            let wanted = if name.starts_with("P:") {
                let a = pattrs(&name);
                a.style == "code" || a.style == "math"
            } else {
                name == "f:code"
            };
            if wanted {
                spans.extend(self.ranges_in(&name, 0, len));
            }
        }
        spans
    }

    // -- the signal handlers ---------------------------------------------------------------------

    fn begin(&self, kind: Option<&'static str>) {
        if self.depth.get() == 0 {
            self.history.borrow_mut().begin(kind);
        }
        self.depth.set(self.depth.get() + 1);
    }

    fn end(&self) {
        self.depth.set(self.depth.get().saturating_sub(1));
        if self.depth.get() == 0 {
            if !self.in_action.get() {
                self.history.borrow_mut().finish();
            }
            if self.restyled.replace(false) {
                // formatting changed but no text: the app still has to know the document changed
                self.notify_modified();
            }
        }
    }

    fn record(&self, op: Op) {
        self.history.borrow_mut().record(op);
    }

    fn on_insert_before(&self, off: usize, text: &str) {
        if self.quiet.get() > 0 {
            return;
        }
        let n = text.chars().count();
        if n == 0 {
            self.pend_ins.borrow_mut().push(None);
            return;
        }
        let explicit = self.explicit.borrow_mut().take();
        let typing = explicit.is_none() && n == 1 && off == self.cursor();
        let kind = typing.then(|| if text.chars().all(char::is_whitespace) { "type:sp" } else { "type" });
        let tags = explicit.clone().unwrap_or_else(|| self.tags_for_insert(off));
        let newline = text.contains('\n');
        if newline && off >= self.len() {
            *self.tail.borrow_mut() = None;
        }
        self.pend_ins.borrow_mut().push(Some(PendingIns { n, tags, explicit: explicit.is_some(), newline }));
        self.begin(kind);
    }

    fn on_insert_after(&self, iter: &mut gtk::TextIter) {
        if self.quiet.get() > 0 {
            return;
        }
        let Some(Some(p)) = self.pend_ins.borrow_mut().pop() else { return };
        let i2 = iter.offset() as usize;
        let i1 = i2.saturating_sub(p.n);
        self.set_tags(i1, i2, &p.tags);
        let frag = self.fragment(i1, i2);
        self.record(Op::Put { i1, i2, frag });
        *self.last_cut.borrow_mut() = None;
        if !p.explicit {
            self.pending.borrow_mut().clear();
        }
        let mark = self.buf.create_mark(None, iter, true);
        if (p.newline || p.explicit) && !self.fixing.get() {
            self.normalize(i1, i2, p.newline);
        } else {
            self.restyle(i1, i2);
        }
        self.end();
        self.changed();
        *iter = self.buf.iter_at_mark(&mark);
        self.buf.delete_mark(&mark);
    }

    fn on_delete_before(&self, s: usize, e: usize) {
        let (a, b) = (s.min(e), s.max(e));
        if a >= b {
            if self.quiet.get() == 0 {
                self.pend_del.borrow_mut().push(None);
            }
            return;
        }
        if self.quiet.get() == 0 {
            self.begin(if b == a + 1 { Some("erase") } else { None });
            let frag = self.fragment(a, b);
            let cut = (a, self.ftags_at(a));
            let newline = frag.iter().any(|x| matches!(x, Item::Text(t, _) if t.contains('\n')));
            self.record(Op::Cut { i1: a, i2: b, frag });
            if b >= self.len() {
                let p = self.para_tag(a);
                *self.tail.borrow_mut() = Some(p);
            }
            self.pend_del.borrow_mut().push(Some(PendingDel { cut, newline }));
        }
        self.forget_views(a, b);
    }

    fn on_delete_after(&self, iter: &mut gtk::TextIter) {
        if self.quiet.get() > 0 {
            return;
        }
        let Some(Some(p)) = self.pend_del.borrow_mut().pop() else { return };
        let a = iter.offset() as usize;
        let mark = self.buf.create_mark(None, iter, true);
        if !self.fixing.get() {
            self.normalize(a, a, p.newline);
            *self.last_cut.borrow_mut() = Some(p.cut); // a replacement typed or put here next takes this look
        }
        self.end();
        self.changed();
        *iter = self.buf.iter_at_mark(&mark);
        self.buf.delete_mark(&mark);
    }

    fn on_tag_before(&self, tag: &gtk::TextTag, s: usize, e: usize) {
        if self.quiet.get() > 0 {
            return;
        }
        let Some(name) = tag.name().map(|n| n.to_string()).filter(|n| keep(n)) else { return };
        self.begin(None);
        let before = self.ranges_in(&name, s.min(e), s.max(e));
        self.pend_tag.borrow_mut().push(PendingTag { tag: name, before });
    }

    fn on_tag_after(&self, tag: &gtk::TextTag, s: usize, e: usize) {
        if self.quiet.get() > 0 {
            return;
        }
        let Some(name) = tag.name().map(|n| n.to_string()).filter(|n| keep(n)) else { return };
        let Some(p) = self.pend_tag.borrow_mut().pop() else { return };
        let (a, b) = (s.min(e), s.max(e));
        let after = self.ranges_in(&name, a, b);
        if after != p.before && p.tag == name {
            self.record(Op::Tag { tag: name, i1: a, i2: b, before: p.before, after });
            self.restyled.set(true);
        }
        self.end();
    }

    fn on_embed_after(&self, i2: usize) {
        if self.quiet.get() > 0 || i2 == 0 {
            return;
        }
        self.begin(None);
        let i1 = i2 - 1;
        let frag = self.fragment(i1, i2);
        self.record(Op::Put { i1, i2, frag });
        self.end();
        self.changed();
    }

    /// Objects and markers between a and b are about to leave the buffer.
    fn forget_views(&self, a: usize, b: usize) {
        let text = self.slice(a, b);
        if !text.contains(OBJ) {
            return;
        }
        for (k, c) in text.chars().enumerate() {
            if c != OBJ {
                continue;
            }
            let it = self.it(a + k);
            if let Some(p) = it.paintable() {
                self.markers.borrow_mut().remove(&(p.as_ptr() as usize));
            }
            if let Some(oid) = self.object_at(a + k) {
                self.sync(&oid);
                if let Some(e) = self.objects.borrow_mut().get_mut(&oid) {
                    e.view = None;
                }
            }
        }
    }

    fn changed(&self) {
        let a = self.app.clone();
        workers::later("rich-state", 60, move || {
            if let Some(app) = a.upgrade() {
                if let Some(f) = app.formatbar.get() {
                    f.refresh();
                }
            }
        });
    }

    fn later_renumber(&self) {
        let a = self.app.clone();
        workers::later("rich-renumber", 80, move || with(&a, |_, r| r.renumber()));
    }

    /// Tell the app the document changed although the text did not (an object edited in place).
    pub fn notify_modified(&self) {
        if !self.app().st.try_borrow().is_ok_and(|st| st.loading) {
            self.buf.emit_by_name::<()>("changed", &[]);
        }
    }

    // -- buffer helpers -------------------------------------------------------------------------

    pub fn len(&self) -> usize {
        self.buf.char_count().max(0) as usize
    }

    fn it(&self, off: usize) -> gtk::TextIter {
        self.buf.iter_at_offset(off.min(i32::MAX as usize) as i32)
    }

    fn cursor(&self) -> usize {
        self.buf.iter_at_mark(&self.buf.get_insert()).offset() as usize
    }

    fn slice(&self, a: usize, b: usize) -> String {
        if b <= a {
            return String::new();
        }
        self.buf.slice(&self.it(a), &self.it(b), true).to_string()
    }

    fn char_at(&self, off: usize) -> Option<char> {
        (off < self.len()).then(|| self.it(off).char())
    }

    fn tag_names(&self, off: usize) -> Vec<String> {
        if off >= self.len() {
            return vec![];
        }
        self.it(off).tags().iter().filter_map(|t| t.name().map(|n| n.to_string())).collect()
    }

    fn ptag_at(&self, off: usize) -> Option<String> {
        self.tag_names(off).into_iter().find(|t| t.starts_with("P:"))
    }

    fn ftags_at(&self, off: usize) -> Vec<String> {
        let mut v: Vec<String> = self.tag_names(off).into_iter().filter(|t| t.starts_with("f:")).collect();
        v.sort();
        v
    }

    fn has_br(&self, off: usize) -> bool {
        self.tag_names(off).iter().any(|t| t == "br")
    }

    fn is_object(&self, off: usize) -> bool {
        self.char_at(off) == Some(OBJ)
    }

    fn line_of(&self, off: usize) -> i32 {
        self.it(off).line()
    }

    fn line_start(&self, line: i32) -> usize {
        if line < 0 {
            return 0;
        }
        if line >= self.buf.line_count() {
            return self.len();
        }
        self.buf.iter_at_line(line).map_or(self.len(), |i| i.offset() as usize)
    }

    fn line_end(&self, line: i32) -> usize {
        let Some(mut it) = (line < self.buf.line_count()).then(|| self.buf.iter_at_line(line)).flatten() else { return self.len() };
        if !it.ends_line() {
            it.forward_to_line_end();
        }
        it.offset() as usize
    }

    fn linestart(&self, off: usize) -> usize {
        self.line_start(self.line_of(off))
    }

    fn lineend(&self, off: usize) -> usize {
        self.line_end(self.line_of(off))
    }

    fn tag(&self, name: &str) -> Option<gtk::TextTag> {
        self.buf.tag_table().lookup(name)
    }

    fn apply(&self, name: &str, a: usize, b: usize) {
        if b > a {
            self.ensure(name);
            if let Some(t) = self.tag(name) {
                self.buf.apply_tag(&t, &self.it(a), &self.it(b));
            }
        }
    }

    fn remove(&self, name: &str, a: usize, b: usize) {
        if b > a {
            if let Some(t) = self.tag(name) {
                self.buf.remove_tag(&t, &self.it(a), &self.it(b));
            }
        }
    }

    /// The ranges of a tag between a and b.
    pub fn ranges_in(&self, name: &str, a: usize, b: usize) -> Vec<(usize, usize)> {
        let Some(tag) = self.tag(name) else { return vec![] };
        let mut out = vec![];
        let mut it = self.it(a);
        let b = b.min(self.len());
        while (it.offset() as usize) < b {
            if it.has_tag(&tag) {
                let s = it.offset() as usize;
                it.forward_to_tag_toggle(Some(&tag));
                let e = (it.offset() as usize).min(b);
                if e > s {
                    out.push((s, e));
                }
                if e <= s {
                    break;
                }
            } else if !it.forward_to_tag_toggle(Some(&tag)) {
                break;
            }
        }
        out
    }

    /// [start, end, document tags] pieces of the buffer between a and b with the same tags.
    fn segments(&self, a: usize, b: usize) -> Vec<(usize, usize, Vec<String>)> {
        let mut out: Vec<(usize, usize, Vec<String>)> = vec![];
        let mut pos = a;
        while pos < b {
            let it = self.it(pos);
            let mut tags: Vec<String> = it.tags().iter().filter_map(|t| t.name().map(|n| n.to_string())).filter(|n| keep(n)).collect();
            tags.sort();
            let mut nx = it.clone();
            nx.forward_to_tag_toggle(None::<&gtk::TextTag>);
            let mut q = (nx.offset() as usize).min(b);
            if q <= pos {
                q = pos + 1;
            }
            match out.last_mut() {
                Some(last) if last.1 == pos && last.2 == tags => last.1 = q,
                _ => out.push((pos, q, tags)),
            }
            pos = q;
        }
        out
    }

    /// Every tag on some character between a and b.
    fn tags_in(&self, a: usize, b: usize) -> HashSet<String> {
        let mut out = HashSet::new();
        if b <= a {
            return out;
        }
        let mut it = self.it(a);
        loop {
            for t in it.tags() {
                if let Some(n) = t.name() {
                    out.insert(n.to_string());
                }
            }
            if !it.forward_to_tag_toggle(None::<&gtk::TextTag>) || it.offset() as usize >= b {
                break;
            }
        }
        out
    }

    /// Give [a, b) exactly these document tags (unrecorded).
    fn set_tags(&self, a: usize, b: usize, wanted: &[String]) {
        if b <= a {
            return;
        }
        self.quiet.set(self.quiet.get() + 1);
        for t in self.tags_in(a, b) {
            if keep(&t) && !wanted.contains(&t) {
                self.remove(&t, a, b);
            }
        }
        for t in wanted {
            if keep(t) {
                self.apply(t, a, b);
            }
        }
        self.quiet.set(self.quiet.get() - 1);
    }

    /// New text takes the look of the text it replaces, else of the text before it (or after it, at the
    /// start of a paragraph), with any formatting picked from the toolbar meanwhile.
    fn tags_for_insert(&self, i1: usize) -> Vec<String> {
        let cut = self.last_cut.borrow().clone();
        let mut f = match cut {
            Some((at, tags)) if at == i1 => tags,
            _ => {
                let mut src = (i1 > self.linestart(i1)).then(|| i1 - 1);
                if src.is_none_or(|s| self.is_object(s)) {
                    src = Some(i1);
                }
                let s = src.unwrap_or(i1);
                if self.is_object(s) { vec![] } else { self.ftags_at(s) }
            }
        };
        let pending = self.pending.borrow().clone();
        if !pending.is_empty() {
            let mut r = fmt_of(&f);
            for (k, v) in pending {
                fmt_set(&mut r, &k, v);
            }
            f = ftags(&r);
        }
        f.push(self.para_tag(i1));
        f
    }

    // -- fragments: content taken out and put back --------------------------------------------------

    pub fn fragment(&self, a: usize, b: usize) -> Frag {
        let mut out: Frag = vec![];
        let push_text = |out: &mut Frag, s: String, tags: &Vec<String>| {
            if s.is_empty() {
                return;
            }
            if let Some(Item::Text(prev, pt)) = out.last_mut() {
                if pt == tags {
                    prev.push_str(&s);
                    return;
                }
            }
            out.push(Item::Text(s, tags.clone()));
        };
        for (s, e, tags) in self.segments(a, b) {
            let text = self.slice(s, e);
            let mut piece = String::new();
            for (k, ch) in text.chars().enumerate() {
                if ch != OBJ {
                    piece.push(ch);
                    continue;
                }
                push_text(&mut out, std::mem::take(&mut piece), &tags);
                let off = s + k;
                if self.is_marker(off) {
                    out.push(Item::Marker(tags.clone()));
                } else if let Some(oid) = self.object_at(off) {
                    self.sync(&oid);
                    if let Some(o) = self.objects.borrow().get(&oid) {
                        out.push(Item::Obj(oid.clone(), o.obj.clone(), tags.clone()));
                    }
                }
            }
            push_text(&mut out, piece, &tags);
        }
        out
    }

    /// Put a fragment back at `at` (unrecorded). Returns where it ends.
    pub fn put(&self, at: usize, frag: &Frag) -> usize {
        self.quiet.set(self.quiet.get() + 1);
        let mark = self.buf.create_mark(None, &self.it(at), false);
        for item in frag {
            let here = self.buf.iter_at_mark(&mark).offset() as usize;
            match item {
                Item::Text(t, tags) => {
                    let mut it = self.it(here);
                    self.buf.insert(&mut it, t);
                    let n = t.chars().count();
                    let keep_tags: Vec<String> = tags.iter().filter(|t| keep(t)).cloned().collect();
                    self.set_tags(here, here + n, &keep_tags);
                }
                Item::Obj(oid, obj, tags) => {
                    let p = tags.iter().find(|t| t.starts_with("P:")).cloned().unwrap_or_else(ptag_default);
                    self.create(oid, obj.clone(), here, &p, tags);
                }
                Item::Marker(tags) => {
                    self.insert_marker(here, None, tags, false);
                }
            }
        }
        let end = self.buf.iter_at_mark(&mark).offset() as usize;
        self.buf.delete_mark(&mark);
        self.restyle(at, end);
        self.quiet.set(self.quiet.get() - 1);
        end
    }

    fn raw_delete(&self, a: usize, b: usize) {
        if b <= a {
            return;
        }
        self.quiet.set(self.quiet.get() + 1);
        let (mut s, mut e) = (self.it(a), self.it(b));
        self.buf.delete(&mut s, &mut e);
        self.quiet.set(self.quiet.get() - 1);
    }

    // -- undo ---------------------------------------------------------------------------------------

    /// The buffer of the table cell that has the focus, if one has.
    fn focused_cell(&self) -> Option<gtk::TextBuffer> {
        let app = self.app();
        let f = gtk::prelude::GtkWindowExt::focus(&app.win)?;
        let tv = f.downcast::<gtk::TextView>().ok()?;
        (tv != self.text && tv.is_ancestor(&self.text)).then(|| tv.buffer())
    }

    pub fn undo(&self) {
        if let Some(b) = self.focused_cell() {
            if b.can_undo() {
                b.undo();
            }
            return;
        }
        let g = self.history.borrow_mut().take_undo();
        if let Some(g) = g {
            self.replay(&g, true);
            self.history.borrow_mut().undone.push(g);
        }
    }

    pub fn redo(&self) {
        if let Some(b) = self.focused_cell() {
            if b.can_redo() {
                b.redo();
            }
            return;
        }
        let g = self.history.borrow_mut().take_redo();
        if let Some(g) = g {
            self.replay(&g, false);
            self.history.borrow_mut().done.push(g);
        }
    }

    fn replay(&self, group: &[Op], undo: bool) {
        self.quiet.set(self.quiet.get() + 1);
        let mut at = None;
        let ops: Vec<&Op> = if undo { group.iter().rev().collect() } else { group.iter().collect() };
        for op in ops {
            at = Some(match (op, undo) {
                (Op::Put { i1, i2, .. }, true) | (Op::Cut { i1, i2, .. }, false) => {
                    self.raw_delete(*i1, *i2);
                    *i1
                }
                (Op::Put { i1, frag, .. }, false) | (Op::Cut { i1, frag, .. }, true) => self.put(*i1, frag),
                (Op::Tag { tag, i1, i2, before, after }, _) => {
                    let ranges = if undo { before } else { after };
                    self.remove(tag, *i1, *i2);
                    for &(s, e) in ranges {
                        self.apply(tag, s, e);
                    }
                    *i1
                }
                (Op::Change { oid, before, after }, _) => {
                    self.set_object(oid, if undo { before.clone() } else { after.clone() }, false);
                    self.object_index(oid).unwrap_or_else(|| self.cursor())
                }
            });
        }
        self.quiet.set(self.quiet.get() - 1);
        *self.last_cut.borrow_mut() = None;
        if let Some(w) = at {
            self.buf.place_cursor(&self.it(w));
            self.text.scroll_mark_onscreen(&self.buf.get_insert());
        }
        self.restyle(0, self.len());
        self.renumber();
        self.changed();
        self.notify_modified();
    }

    // -- paragraphs ---------------------------------------------------------------------------------

    /// (start, end) of the paragraph at off; end is just past its closing newline.
    pub fn para_range(&self, off: usize) -> (usize, usize) {
        let n = self.len();
        let off = off.min(n);
        let mut sl = self.line_of(off);
        while sl > 0 {
            let st = self.line_start(sl);
            if st > 0 && self.has_br(st - 1) {
                sl -= 1;
            } else {
                break;
            }
        }
        let mut el = self.line_of(off);
        loop {
            let e = self.line_end(el);
            if e < n && self.has_br(e) {
                el += 1;
            } else {
                break;
            }
        }
        (self.line_start(sl), (self.line_end(el) + 1).min(n))
    }

    pub fn para_tag(&self, off: usize) -> String {
        if let Some(p) = self.ptag_at(off) {
            return p;
        }
        let (s, _) = self.para_range(off);
        if let Some(p) = self.ptag_at(s) {
            return p;
        }
        if s >= self.len() {
            if let Some(t) = self.tail.borrow().clone() {
                return t;
            }
        }
        ptag_default()
    }

    /// The first lines of the paragraphs that [i1, i2] touches.
    fn paragraphs(&self, i1: usize, i2: usize) -> Vec<i32> {
        let n = self.len();
        let (s, mut e) = self.para_range(i1);
        let mut out = vec![self.line_of(s)];
        while e <= i2 && e <= n {
            let (s2, e2) = self.para_range(e);
            let l = self.line_of(s2);
            if out.contains(&l) {
                break;
            }
            out.push(l);
            if e2 == e {
                break;
            }
            e = e2;
        }
        out
    }

    fn p_tags(&self) -> Vec<String> {
        self.configured.borrow().iter().filter(|t| t.starts_with("P:")).cloned().collect()
    }

    /// Make each paragraph between i1 and i2 one P: tag from start to end, with a list marker at its start
    /// when it is a list item and nowhere else. Changes made here are recorded with the edit.
    fn normalize(&self, i1: usize, i2: usize, newline: bool) {
        self.fixing.set(true);
        let lines = self.paragraphs(self.linestart(i1), self.lineend(i2));
        for l in lines {
            let (s, e) = self.para_range(self.line_start(l));
            let n = self.len();
            let mut p = self
                .ptag_at(s)
                .or_else(|| if s >= n { self.tail.borrow().clone() } else { None })
                .or_else(|| if s > 0 { self.ptag_at(s - 1) } else { None })
                .unwrap_or_else(ptag_default);
            if newline && s > 0 {
                let prev = self.ptag_at(s - 1);
                let a = pattrs(&p);
                // a new, empty paragraph after a heading or caption is normal text
                let text = self.buf.text(&self.it(s), &self.it(self.lineend(s)), false).to_string();
                if prev.as_deref() == Some(p.as_str())
                    && (HEADINGS.contains(&a.style.as_str()) || a.style == "caption")
                    && text.trim().is_empty()
                {
                    p = PAttrs { align: a.align.clone(), ..Default::default() }.tag();
                }
            }
            for t in self.p_tags() {
                if t != p && !self.ranges_in(&t, s, e).is_empty() {
                    self.remove(&t, s, e);
                }
            }
            if e > s && self.ranges_in(&p, s, e) != vec![(s, e)] {
                self.ensure_ptag(&p);
                self.apply(&p, s, e);
            }
            if s >= self.len() {
                *self.tail.borrow_mut() = Some(p.clone());
            }
            self.fix_markers(s, e, pattrs(&p).kind.is_some());
            let (s, e) = self.para_range(self.line_start(l));
            self.restyle(s, e);
        }
        self.fixing.set(false);
        self.later_renumber();
    }

    fn is_marker(&self, off: usize) -> bool {
        if !self.is_object(off) {
            return false;
        }
        let Some(p) = self.it(off).paintable() else { return false };
        self.markers.borrow().contains_key(&(p.as_ptr() as usize))
    }

    /// Where a paragraph's text starts (after its list marker).
    pub fn text_start(&self, s: usize) -> usize {
        if self.is_marker(s) { s + 1 } else { s }
    }

    fn markers_in(&self, s: usize, e: usize) -> Vec<usize> {
        let e = e.min(self.len());
        self.slice(s, e).chars().enumerate().filter(|&(k, c)| c == OBJ && self.is_marker(s + k)).map(|(k, _)| s + k).collect()
    }

    fn fix_markers(&self, s: usize, e: usize, wanted: bool) {
        let found = self.markers_in(s, e);
        for &o in found.iter().rev() {
            if wanted && o == s {
                continue;
            }
            let (mut a, mut b) = (self.it(o), self.it(o + 1));
            self.buf.delete(&mut a, &mut b);
        }
        if wanted && !found.contains(&s) {
            let p = self.para_tag(s);
            self.insert_marker(s, None, &[], true);
            self.ensure_ptag(&p);
            self.apply(&p, s, s + 1);
        }
    }

    /// A list marker: a label at the start of a list item (one character wide, so renumbering does not
    /// move any text). Recorded for undo when `record`.
    fn insert_marker(&self, at: usize, label: Option<&str>, tags: &[String], record: bool) {
        let pic = Glyphs::default();
        self.draw_marker(&pic, label.unwrap_or("•"), &self.para_tag(at));
        self.markers.borrow_mut().insert(pic.upcast_ref::<gdk::Paintable>().as_ptr() as usize, pic.clone());
        if !record {
            self.quiet.set(self.quiet.get() + 1);
        }
        let mut it = self.it(at);
        self.buf.insert_paintable(&mut it, &pic);
        if !record {
            self.quiet.set(self.quiet.get() - 1);
        }
        if !tags.is_empty() {
            let t: Vec<String> = tags.iter().filter(|t| keep(t)).cloned().collect();
            self.set_tags(at, at + 1, &t);
        }
    }

    /// A marker's label in the font of its paragraph.
    fn draw_marker(&self, pic: &Glyphs, label: &str, p: &str) {
        let font = self.font_tag(p, &[]);
        let mut fd = pango::FontDescription::new();
        if let Some((fam, size, bold, italic)) = font_of(&font) {
            fd.set_family(&fam);
            fd.set_size((size * pango::SCALE as f64) as i32);
            fd.set_weight(if bold { pango::Weight::Bold } else { pango::Weight::Normal });
            fd.set_style(if italic { pango::Style::Italic } else { pango::Style::Normal });
        }
        let color = self.app().theme().rgba("text");
        pic.set(&self.text, label, &fd, color, self.hang_px());
    }

    /// List numbers and note numbers, in document order.
    pub fn renumber(&self) {
        if !self.active.get() {
            return;
        }
        let all = self.slice(0, self.len());
        let mut items: Vec<(usize, i32, String, Glyphs)> = vec![];
        {
            let markers = self.markers.borrow();
            for (k, c) in all.chars().enumerate() {
                if c != OBJ {
                    continue;
                }
                let it = self.it(k);
                if let Some(p) = it.paintable() {
                    if let Some(m) = markers.get(&(p.as_ptr() as usize)) {
                        items.push((k, it.line(), String::new(), m.clone()));
                    }
                }
            }
        }
        for x in items.iter_mut() {
            x.2 = self.para_tag(x.0);
        }
        let mut lists: Vec<Option<(String, u32)>> = vec![];
        let mut rows: Vec<Option<(Glyphs, String)>> = vec![];
        let mut prev: Option<i32> = None;
        for (_, line, p, pic) in &items {
            let a = pattrs(p);
            if let Some(pl) = prev {
                if !self.consecutive(pl, *line) {
                    lists.push(None);
                    rows.push(None);
                }
            }
            lists.push(a.kind.clone().map(|k| (k, a.level)));
            rows.push(Some((pic.clone(), p.clone())));
            prev = Some(*line);
        }
        let labels = if lists.is_empty() { vec![] } else { flat::labels(&lists) };
        for (row, label) in rows.iter().zip(labels) {
            let Some((pic, p)) = row else { continue };
            self.draw_marker(pic, label.as_deref().unwrap_or("•"), p);
        }
        let mut notes: Vec<(usize, String)> = self
            .objects
            .borrow()
            .iter()
            .filter(|(_, e)| matches!(e.obj, Obj::Note(_)) && e.view.is_some())
            .map(|(k, _)| (0, k.clone()))
            .collect();
        for n in notes.iter_mut() {
            n.0 = self.object_index(&n.1).unwrap_or(usize::MAX);
        }
        notes.sort();
        for (k, (_, oid)) in notes.iter().enumerate() {
            if let Some(Some(v)) = self.objects.borrow().get(oid).map(|e| e.view.as_ref()) {
                v.set_number(k + 1);
            }
        }
    }

    /// Whether every line between two list lines is part of a list item.
    fn consecutive(&self, a: i32, b: i32) -> bool {
        (a + 1..b).all(|line| pattrs(&self.para_tag(self.line_start(line))).kind.is_some())
    }

    // -- looks -------------------------------------------------------------------------------------

    pub fn zoom(&self) -> f64 {
        self.app().theme().size / 11.0
    }

    /// Pixels per point of text at the current zoom.
    fn pt(&self) -> f64 {
        96.0 / 72.0 * self.zoom()
    }

    fn hang_px(&self) -> i32 {
        self.text.create_pango_layout(Some("000")).pixel_size().0.max(8)
    }

    /// Keep a new tag just under the app's decoration tags (protected text, hints, changes ...).
    fn place_mid(&self, tag: &gtk::TextTag) {
        let app = self.app();
        tag.set_priority(app.tags.prot.priority().max(0));
    }

    fn ensure(&self, name: &str) {
        if name.starts_with("P:") {
            self.ensure_ptag(name);
        } else if name.starts_with("f:") {
            self.ensure_tag(name);
        } else if self.tag(name).is_none() {
            if let Some(t) = self.buf.create_tag(Some(name), &[]) {
                if name.starts_with("F:") {
                    t.set_priority(0);
                } else {
                    self.place_mid(&t);
                }
            }
        }
    }

    fn ensure_ptag(&self, p: &str) {
        if self.configured.borrow().contains(p) && self.tag(p).is_some() {
            return;
        }
        self.configured.borrow_mut().insert(p.to_string());
        if self.tag(p).is_none() {
            if let Some(t) = self.buf.create_tag(Some(p), &[]) {
                t.set_priority(0);
            }
        }
        self.style_ptag(p);
    }

    fn style_ptag(&self, p: &str) {
        let Some(tag) = self.tag(p) else { return };
        let a = pattrs(p);
        let l = look(&a.style);
        let pt = self.pt();
        let app = self.app();
        let th = app.theme();
        let mut indent = (l.indent + 36.0 * a.indent as f64) * pt;
        let mut hang = 0.0;
        if a.kind.is_some() {
            indent += 22.0 * a.level as f64 * pt;
            hang = self.hang_px() as f64;
        }
        if a.style == "code" {
            indent += 6.0 * pt;
            tag.set_paragraph_background_rgba(Some(&th.rgba("hover")));
        } else {
            tag.set_paragraph_background_rgba(None);
        }
        tag.set_pixels_above_lines((l.before * pt) as i32);
        tag.set_pixels_below_lines(if a.kind.is_some() { (2.0 * pt) as i32 } else { (l.after * pt) as i32 });
        // a tag's margins replace the view's own, which centre the column
        let (vl, vr) = (self.text.left_margin(), self.text.right_margin());
        self.margins.set((vl, vr));
        tag.set_left_margin(vl + (indent + hang) as i32);
        tag.set_indent(-(hang as i32));
        tag.set_right_margin(vr + if a.style == "quote" { (36.0 * pt) as i32 } else { 0 });
        let just = if a.style == "math" {
            gtk::Justification::Center
        } else {
            match a.align.as_deref() {
                Some("center") => gtk::Justification::Center,
                Some("right") => gtk::Justification::Right,
                Some("justify") | Some("both") => gtk::Justification::Fill,
                _ => gtk::Justification::Left,
            }
        };
        tag.set_justification(just);
        match l.color {
            Some(c) => {
                let c = if th.dark() { lighten(c) } else { c.to_string() };
                tag.set_foreground_rgba(Some(&crate::theme::rgba(&c)));
            }
            None => tag.set_foreground_rgba(None),
        }
    }

    fn ensure_tag(&self, t: &str) {
        if self.configured.borrow().contains(t) && self.tag(t).is_some() {
            return;
        }
        self.configured.borrow_mut().insert(t.to_string());
        let tag = match self.tag(t) {
            Some(x) => x,
            None => match self.buf.create_tag(Some(t), &[]) {
                Some(x) => x,
                None => return,
            },
        };
        self.style_ftag(t, &tag);
        self.place_mid(&tag);
    }

    fn style_ftag(&self, t: &str, tag: &gtk::TextTag) {
        let body = &t[2..];
        let (k, v) = body.split_once('=').unwrap_or((body, ""));
        let app = self.app();
        let th = app.theme();
        let px = th.size * 96.0 / 72.0;
        match k {
            "u" => tag.set_underline(pango::Underline::Single),
            "s" => tag.set_strikethrough(true),
            "sup" => tag.set_rise((px * 0.92 * 0.35 * pango::SCALE as f64) as i32),
            "sub" => tag.set_rise(-(px * 0.24 * 0.9 * pango::SCALE as f64) as i32),
            "code" => tag.set_background_rgba(Some(&th.rgba("hover"))),
            "color" if !v.is_empty() => {
                let c = if th.dark() { lighten(v) } else { v.to_string() };
                tag.set_foreground_rgba(Some(&crate::theme::rgba(&c)));
            }
            "hl" if !v.is_empty() => {
                tag.set_background_rgba(Some(&crate::theme::rgba(highlight(v))));
                tag.set_foreground_rgba(Some(&crate::theme::rgba("#1E2227")));
            }
            "link" => {
                tag.set_foreground_rgba(Some(&th.rgba("accent")));
                tag.set_underline(pango::Underline::Single);
            }
            _ => {}
        }
    }

    /// The F: tag that draws text of these f: tags in a paragraph of tag p.
    pub fn font_tag(&self, p: &str, ftag_list: &[String]) -> String {
        let a = pattrs(p);
        let l = look(&a.style);
        let fmt = fmt_of(ftag_list);
        let app = self.app();
        let (base, doc_font) = {
            let m = self.meta.borrow();
            (if m.size > 0.0 { m.size } else { 11.0 }, m.font.clone())
        };
        let th = app.theme();
        let mut size = th.size * fmt.size.map_or(l.scale, |s| s / base);
        if fmt.sup || fmt.sub {
            size *= 0.7;
        }
        let mut family = th.editor_font().to_string();
        if let Some(want) = fmt.font.clone().or(doc_font).filter(|w| !w.is_empty()) {
            if installed(&self.text, &want) {
                family = want;
            }
        }
        if fmt.code || a.style == "code" {
            family = th.families.mono.clone();
        }
        drop(th);
        let bold = fmt.b || l.bold;
        let italic = fmt.i || l.italic;
        let size = size.round().max(6.0) as i32;
        let name = format!("F:{family}|{size}|{}|{}", bold as u8, italic as u8);
        if !self.fonts.borrow().contains(&name) || self.tag(&name).is_none() {
            self.fonts.borrow_mut().insert(name.clone());
            if self.tag(&name).is_none() {
                if let Some(t) = self.buf.create_tag(Some(&name), &[]) {
                    t.set_family(Some(&family));
                    t.set_size_points(size as f64);
                    t.set_weight(if bold { 700 } else { 400 });
                    t.set_style(if italic { pango::Style::Italic } else { pango::Style::Normal });
                    t.set_priority(0);
                }
            }
        }
        name
    }

    /// Redraw fonts between a and b from the f: and P: tags.
    pub fn restyle(&self, a: usize, b: usize) {
        let b = b.min(self.len());
        if a >= b {
            return;
        }
        let segs = self.segments(a, b);
        let mut groups: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (s, e, tags) in segs {
            let p = tags.iter().find(|t| t.starts_with("P:")).cloned().unwrap_or_else(|| "P:normal".into());
            let f: Vec<String> = tags.iter().filter(|t| t.starts_with("f:")).cloned().collect();
            groups.entry(self.font_tag(&p, &f)).or_default().push((s, e));
        }
        self.quiet.set(self.quiet.get() + 1);
        let fonts: Vec<String> = self.fonts.borrow().iter().cloned().collect();
        for name in fonts {
            self.remove(&name, a, b);
        }
        for (name, ranges) in groups {
            if let Some(t) = self.tag(&name) {
                for (s, e) in ranges {
                    self.buf.apply_tag(&t, &self.it(s), &self.it(e));
                }
            }
        }
        self.quiet.set(self.quiet.get() - 1);
        self.break_spacing(a, b);
    }

    /// Lines joined by a line break (in a code block, or Shift+Enter) belong to one paragraph: no
    /// paragraph spacing between them. GTK spaces every line, so the lines around a break get tags that
    /// take the spacing away.
    fn break_spacing(&self, a: usize, b: usize) {
        for (name, below) in [("br:end", true), ("br:start", false)] {
            if self.tag(name).is_none() {
                if let Some(t) = self.buf.create_tag(Some(name), &[]) {
                    if below {
                        t.set_pixels_below_lines(0);
                    } else {
                        t.set_pixels_above_lines(0);
                    }
                    self.place_mid(&t);
                }
            }
        }
        let first = (self.line_of(a) - 1).max(0);
        let last = self.line_of(b);
        let (s, e) = (self.line_start(first), self.line_start(last + 1));
        self.quiet.set(self.quiet.get() + 1);
        self.remove("br:end", s, e);
        self.remove("br:start", s, e);
        for (rs, re) in self.ranges_in("br", s, e) {
            for off in rs..re {
                let line = self.line_of(off);
                if line < first || line > last {
                    continue;
                }
                self.apply("br:end", self.line_start(line), self.line_start(line + 1));
                self.apply("br:start", self.line_start(line + 1), self.line_start(line + 2));
            }
        }
        self.quiet.set(self.quiet.get() - 1);
    }

    fn load_css(&self) {
        let Some(app) = self.app.upgrade() else { return };
        let th = app.theme();
        let font = self.font_tag(&ptag_default(), &[]);
        let (fam, size, _, _) = font_of(&font).unwrap_or((th.editor_font().to_string(), th.size, false, false));
        let c = |k: &str| th.c(k);
        self.css.load_from_string(&crate::theme::scale_css(&format!(
            r#"
.rich-table {{ background-color: {border}; padding: 1px; }}
textview.rich-cell, textview.rich-cell text {{ background-color: {surface}; color: {text}; caret-color: {text};
    font-family: "{fam}"; font-size: {size}pt; font-weight: normal; }}
textview.rich-cell.head, textview.rich-cell.head text {{ background-color: {hover}; font-weight: bold; }}
textview.rich-cell text selection {{ background-color: {select}; }}
label.rich-note {{ color: {accent}; font-family: "{ui}"; font-size: {note}pt; font-weight: bold; }}
label.rich-muted {{ color: {muted}; font-family: "{ui}"; font-size: {small}pt; }}
"#,
            border = c("border"),
            surface = c("surface"),
            text = c("text"),
            hover = c("hover"),
            select = c("select"),
            accent = c("accent"),
            muted = c("muted"),
            ui = th.families.ui,
            note = (th.size * 0.7).max(7.0).round(),
            small = crate::theme::UI_SIZE - 1.0,
        )));
    }

    /// The zoom or the theme changed: redraw fonts, paragraph spacing and objects.
    pub fn retheme(&self) {
        if !self.active.get() {
            return;
        }
        let table = self.buf.tag_table();
        let old: Vec<String> = self.fonts.borrow_mut().drain().collect();
        for name in old {
            if let Some(t) = table.lookup(&name) {
                table.remove(&t);
            }
        }
        let names: Vec<String> = self.configured.borrow().iter().cloned().collect();
        for n in names {
            if n.starts_with("P:") {
                self.style_ptag(&n);
            } else if let Some(t) = self.tag(&n) {
                // f: tags: start again from a clean tag
                t.set_underline(pango::Underline::None);
                t.set_strikethrough(false);
                t.set_rise(0);
                t.set_background_rgba(None);
                t.set_foreground_rgba(None);
                self.style_ftag(&n, &t);
            }
        }
        self.restyle(0, self.len());
        self.load_css();
        self.redraw_all();
        self.renumber();
    }

    // -- objects ------------------------------------------------------------------------------------

    pub fn new_id(&self) -> String {
        self.next.set(self.next.get() + 1);
        format!("obj{}", self.next.get())
    }

    /// Embed an object at `at` (unrecorded).
    pub fn create(&self, oid: &str, obj: Obj, at: usize, p: &str, extra: &[String]) -> String {
        self.quiet.set(self.quiet.get() + 1);
        let app = self.app();
        let view = View::new(&app, self, oid, &obj);
        let mut it = self.it(at);
        if let Some(pic) = view.paintable() {
            self.buf.insert_paintable(&mut it, pic);
        } else if let (Some(anchor), Some(w)) = (view.anchor(), view.widget()) {
            self.buf.insert_child_anchor(&mut it, anchor);
            self.text.add_child_at_anchor(w, anchor);
        }
        self.objects.borrow_mut().insert(oid.to_string(), Entry { obj, view: Some(view) });
        let mut tags = vec![p.to_string()];
        for t in extra {
            if keep(t) && !t.starts_with("P:") && !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        self.set_tags(at, at + 1, &tags);
        self.quiet.set(self.quiet.get() - 1);
        oid.to_string()
    }

    pub fn object(&self, oid: &str) -> Option<Obj> {
        self.sync(oid);
        self.objects.borrow().get(oid).map(|e| e.obj.clone())
    }

    /// Where an object is in the buffer.
    pub fn object_index(&self, oid: &str) -> Option<usize> {
        let objs = self.objects.borrow();
        let v = objs.get(oid)?.view.as_ref()?;
        if let Some(a) = v.anchor() {
            return (!a.is_deleted()).then(|| self.buf.iter_at_child_anchor(a).offset() as usize);
        }
        let pic = v.paintable()?.clone();
        drop(objs);
        let text = self.plain();
        for (k, c) in text.chars().enumerate() {
            if c == OBJ && self.it(k).paintable().as_ref() == Some(&pic) {
                return Some(k);
            }
        }
        None
    }

    pub fn object_at(&self, off: usize) -> Option<String> {
        if !self.is_object(off) {
            return None;
        }
        let it = self.it(off);
        let objs = self.objects.borrow();
        if let Some(p) = it.paintable() {
            return objs
                .iter()
                .find(|(_, e)| e.view.as_ref().and_then(|v| v.paintable()).is_some_and(|q| q == &p))
                .map(|(k, _)| k.clone());
        }
        if let Some(a) = it.child_anchor() {
            return objs.iter().find(|(_, e)| e.view.as_ref().and_then(|v| v.anchor()).is_some_and(|b| b == &a)).map(|(k, _)| k.clone());
        }
        None
    }

    /// Read an object's widget back into the object (tables are edited in place).
    fn sync(&self, oid: &str) {
        let new = {
            let objs = self.objects.borrow();
            objs.get(oid).and_then(|e| e.view.as_ref().and_then(|v| v.read(&e.obj)))
        };
        if let Some(n) = new {
            if let Some(e) = self.objects.borrow_mut().get_mut(oid) {
                e.obj = n;
            }
        }
    }

    /// Replace an object with an edited version, as one step for undo.
    pub fn set_object(&self, oid: &str, obj: Obj, record: bool) {
        if !self.objects.borrow().contains_key(oid) {
            return;
        }
        self.sync(oid);
        let before = self.objects.borrow().get(oid).map(|e| e.obj.clone()).unwrap();
        if let Some(e) = self.objects.borrow_mut().get_mut(oid) {
            e.obj = obj.clone();
        }
        self.redraw(oid);
        if record && self.quiet.get() == 0 {
            let mut h = self.history.borrow_mut();
            h.separator();
            h.begin(None);
            h.record(Op::Change { oid: oid.to_string(), before, after: obj });
            h.separator();
        }
        if self.quiet.get() == 0 {
            self.notify_modified();
        }
        self.later_renumber();
    }

    fn redraw(&self, oid: &str) {
        let Some(obj) = self.objects.borrow().get(oid).map(|e| e.obj.clone()) else { return };
        let app = self.app();
        if let Some(Some(v)) = self.objects.borrow().get(oid).map(|e| e.view.as_ref()) {
            v.update(&app, self, &obj);
        }
    }

    fn redraw_all(&self) {
        let ids: Vec<String> = self.objects.borrow().iter().filter(|(_, e)| e.view.is_some()).map(|(k, _)| k.clone()).collect();
        for oid in ids {
            self.sync(&oid);
            self.redraw(&oid);
        }
    }

    /// The object whose widget holds `w` (with the table cell, when it is one).
    pub fn object_of_widget(&self, w: &gtk::Widget) -> Option<(String, Option<(usize, usize)>)> {
        let objs = self.objects.borrow();
        for (k, e) in objs.iter() {
            if let Some(v) = &e.view {
                if v.contains(w) {
                    return Some((k.clone(), v.table_cell(w)));
                }
            }
        }
        None
    }

    /// Put a new object at the cursor (replacing the selection) as one step for undo. Tables, rules, page
    /// breaks and display formulas get a paragraph of their own.
    pub fn insert_object(&self, obj: Obj) -> String {
        let oid = self.new_id();
        let block = obj.is_block();
        let is_math = matches!(obj, Obj::Math(_));
        let buf = self.buf.clone();
        buf.begin_user_action();
        buf.delete_selection(false, true);
        if block {
            // split the paragraph so that the object stands on an empty one of its own
            let (s, _) = self.para_range(self.cursor());
            if self.cursor() > self.text_start(s) {
                buf.insert_at_cursor("\n");
            }
            let cur = self.cursor();
            let rest = buf.text(&self.it(cur), &self.it(self.lineend(cur)), false).to_string();
            if !rest.trim().is_empty() || self.is_object(cur) {
                buf.insert_at_cursor("\n");
                buf.place_cursor(&self.it(self.cursor().saturating_sub(1)));
            }
            let (s, e) = self.para_range(self.cursor());
            let p = self.para_tag(s);
            let newp = PAttrs { style: if is_math { "math".into() } else { "normal".into() }, ..Default::default() }.tag();
            if newp != p {
                self.remove(&p, s, e);
                self.ensure_ptag(&newp);
                self.apply(&newp, s, e);
                if s >= self.len() {
                    *self.tail.borrow_mut() = Some(newp.clone());
                }
            }
            self.fixing.set(true);
            self.fix_markers(s, e, false);
            self.fixing.set(false);
            buf.place_cursor(&self.it(s));
        }
        let at = self.cursor();
        let p = self.para_tag(at);
        self.begin(None);
        self.create(&oid, obj, at, &p, &[]);
        let frag = self.fragment(at, at + 1);
        self.record(Op::Put { i1: at, i2: at + 1, frag });
        self.end();
        buf.place_cursor(&self.it(at + 1));
        buf.end_user_action();
        self.notify_modified();
        self.later_renumber();
        self.text.scroll_mark_onscreen(&buf.get_insert());
        oid
    }

    // -- pointer -----------------------------------------------------------------------------------

    fn iter_at_point(&self, x: f64, y: f64) -> Option<(gtk::TextIter, i32, i32)> {
        let (bx, by) = self.text.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        self.text.iter_at_location(bx, by).map(|it| (it, bx, by))
    }

    fn inside(&self, it: &gtk::TextIter, bx: i32, by: i32) -> bool {
        let r = self.text.iter_location(it);
        bx >= r.x() && bx <= r.x() + r.width().max(1) && by >= r.y() && by <= r.y() + r.height()
    }

    /// The picture under the pointer.
    pub fn picture_at(&self, x: f64, y: f64) -> Option<String> {
        let (it, bx, by) = self.iter_at_point(x, y)?;
        let mut cands = vec![it.clone()];
        let mut prev = it.clone();
        if prev.backward_char() {
            cands.push(prev);
        }
        for c in cands {
            if c.paintable().is_some() && self.inside(&c, bx, by) {
                return self.object_at(c.offset() as usize);
            }
        }
        None
    }

    /// Whether the pointer is over a formula.
    fn formula_at(&self, x: f64, y: f64) -> bool {
        self.picture_at(x, y).is_some_and(|oid| matches!(self.objects.borrow().get(&oid), Some(Entry { obj: Obj::Math(_), .. })))
    }

    fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let (it, bx, by) = self.iter_at_point(x, y)?;
        if !self.inside(&it, bx, by) {
            return None;
        }
        self.tag_names(it.offset() as usize).into_iter().find_map(|t| t.strip_prefix("f:link=").map(String::from))
    }

    /// A right-click: the menu of the object (or table cell) under the pointer. Returns whether it was one.
    pub fn context_click(&self, x: f64, y: f64) -> bool {
        let app = self.app();
        if app.busy() {
            return false;
        }
        if let Some(w) = self.text.pick(x, y, gtk::PickFlags::DEFAULT) {
            if w != *self.text.upcast_ref::<gtk::Widget>() {
                if let Some((oid, cell)) = self.object_of_widget(&w) {
                    let app = app.clone();
                    glib::idle_add_local_once(move || match cell {
                        Some((r, k)) => app.rich_table_menu(&oid, r, k, x, y),
                        None => app.rich_object_menu(&oid, x, y),
                    });
                    return true;
                }
            }
        }
        if let Some(oid) = self.picture_at(x, y) {
            glib::idle_add_local_once(move || app.rich_object_menu(&oid, x, y));
            return true;
        }
        false
    }

    // -- formatting commands --------------------------------------------------------------------

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.buf.selection_bounds().map(|(a, b)| (a.offset() as usize, b.offset() as usize))
    }

    /// What the toolbar shows: the formatting at the cursor (or the start of the selection) and the
    /// paragraph's attributes.
    pub fn state(&self) -> (Run, PAttrs) {
        let sel = self.selection();
        let at = sel.map_or(self.cursor(), |s| s.0);
        let mut src = at;
        if sel.is_none() && at > self.linestart(at) && !self.is_object(at - 1) {
            src = at - 1;
        }
        let mut fmt = fmt_of(&self.ftags_at(src));
        if sel.is_none() {
            for (k, v) in self.pending.borrow().iter() {
                fmt_set(&mut fmt, k, v.clone());
            }
        }
        (fmt, pattrs(&self.para_tag(at)))
    }

    fn one_step(&self, f: impl FnOnce()) {
        self.buf.begin_user_action();
        f();
        self.buf.end_user_action();
    }

    /// b | i | u | s | sup | sub
    pub fn toggle(&self, name: &str) {
        let tag = format!("f:{name}");
        let Some((a, b)) = self.selection() else {
            let cur = fmt_get(&self.state().0, name).is_some();
            let mut p = self.pending.borrow_mut();
            p.insert(name.to_string(), (!cur).then(String::new));
            if (name == "sup" || name == "sub") && !cur {
                p.insert(if name == "sup" { "sub" } else { "sup" }.to_string(), None);
            }
            drop(p);
            self.changed();
            return;
        };
        let on = self.all_tagged(&tag, a, b);
        self.one_step(|| {
            if on {
                self.remove(&tag, a, b);
            } else {
                self.ensure_tag(&tag);
                if name == "sup" || name == "sub" {
                    self.remove(if name == "sup" { "f:sub" } else { "f:sup" }, a, b);
                }
                self.apply(&tag, a, b);
            }
        });
        self.restyle(a, b);
        self.changed();
    }

    fn all_tagged(&self, tag: &str, a: usize, b: usize) -> bool {
        let r = self.ranges_in(tag, a, b);
        !r.is_empty() && r.iter().map(|(s, e)| e - s).sum::<usize>() >= b - a
    }

    /// size / font / color / hl / link for the selection (None removes it).
    pub fn set_value(&self, kind: &str, value: Option<String>) {
        let value = value.filter(|v| !v.is_empty());
        let Some((a, b)) = self.selection() else {
            self.pending.borrow_mut().insert(kind.to_string(), value);
            self.changed();
            return;
        };
        let prefix = format!("f:{kind}=");
        self.one_step(|| {
            let names: Vec<String> = self.configured.borrow().iter().filter(|t| t.starts_with(&prefix)).cloned().collect();
            for t in names {
                if !self.ranges_in(&t, a, b).is_empty() {
                    self.remove(&t, a, b);
                }
            }
            if let Some(v) = &value {
                let mut r = Run::default();
                fmt_set(&mut r, kind, Some(v.clone()));
                if let Some(t) = ftags(&r).first() {
                    self.ensure_tag(t);
                    self.apply(t, a, b);
                }
            }
        });
        self.restyle(a, b);
        self.changed();
    }

    pub fn clear_format(&self) {
        let Some((a, b)) = self.selection() else {
            self.pending.borrow_mut().clear();
            self.changed();
            return;
        };
        self.one_step(|| {
            let names: Vec<String> = self.configured.borrow().iter().filter(|t| t.starts_with("f:")).cloned().collect();
            for t in names {
                if !self.ranges_in(&t, a, b).is_empty() {
                    self.remove(&t, a, b);
                }
            }
        });
        self.restyle(a, b);
        self.changed();
    }

    /// Change attributes of the paragraphs in the selection (or at the cursor).
    pub fn set_para(&self, change: impl Fn(&mut PAttrs), kind_changed: bool) {
        let (a, b) = self.selection().unwrap_or_else(|| (self.cursor(), self.cursor()));
        let lines = self.paragraphs(a, b);
        self.one_step(|| {
            for l in &lines {
                let (s, e) = self.para_range(self.line_start(*l));
                let old = self.para_tag(s);
                let mut at = pattrs(&old);
                change(&mut at);
                if (HEADINGS.contains(&at.style.as_str()) || at.style == "code") && !kind_changed {
                    at.kind = None;
                }
                if at.kind.is_none() {
                    at.level = 0;
                }
                let new = at.tag();
                if new != old {
                    self.remove(&old, s, e);
                    self.ensure_ptag(&new);
                    self.apply(&new, s, e);
                    if s >= self.len() {
                        *self.tail.borrow_mut() = Some(new.clone());
                    }
                }
                self.fix_markers(s, e, at.kind.is_some());
                let (s, e) = self.para_range(self.line_start(*l));
                self.restyle(s, e);
            }
        });
        self.renumber();
        self.changed();
    }

    pub fn set_style(&self, style: &str) {
        let s = style.to_string();
        self.set_para(move |a| a.style = s.clone(), false);
    }

    /// None = left
    pub fn set_align(&self, align: Option<String>) {
        self.set_para(move |a| a.align = align.clone(), false);
    }

    /// bullet | number
    pub fn toggle_list(&self, kind: &str) {
        let a = self.state().1;
        let new = if a.kind.as_deref() == Some(kind) { None } else { Some(kind.to_string()) };
        self.set_para(move |p| p.kind = new.clone(), true);
    }

    pub fn change_level(&self, d: i32) {
        let a = self.state().1;
        if a.kind.is_some() {
            let lv = (a.level as i32 + d).clamp(0, 8) as u32;
            self.set_para(move |p| p.level = lv, false);
        } else {
            let ind = (a.indent as i32 + d).clamp(0, 10) as u32;
            self.set_para(move |p| p.indent = ind, false);
        }
    }

    // -- keys -----------------------------------------------------------------------------------

    fn on_key(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::META_MASK | gdk::ModifierType::ALT_MASK);
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter if !ctrl => {
                if shift {
                    self.soft_return();
                    true
                } else {
                    self.key_return()
                }
            }
            gdk::Key::Tab | gdk::Key::KP_Tab if !ctrl => self.key_tab(if shift { -1 } else { 1 }),
            gdk::Key::ISO_Left_Tab if !ctrl => self.key_tab(-1),
            // with Shift or Ctrl too: at the start of a list item they end the list or take the indent back
            gdk::Key::BackSpace if !state.contains(gdk::ModifierType::ALT_MASK) => self.key_backspace(),
            gdk::Key::Home | gdk::Key::KP_Home => {
                let a = self.app.clone();
                glib::idle_add_local_once(move || with(&a, |_, r| r.skip_marker()));
                false
            }
            _ => false,
        }
    }

    fn key_return(&self) -> bool {
        let (s, _) = self.para_range(self.cursor());
        let a = pattrs(&self.para_tag(s));
        if a.style == "code" {
            self.soft_return();
            return true;
        }
        let ts = self.text_start(s);
        let blank = self.buf.text(&self.it(ts), &self.it(self.lineend(s)), false).trim().is_empty();
        if a.kind.is_some() && blank && self.selection().is_none() {
            self.set_para(|p| p.kind = None, true); // Enter on an empty list item ends the list
            return true;
        }
        false
    }

    fn soft_return(&self) {
        self.one_step(|| {
            self.buf.delete_selection(false, true);
            let at = self.cursor();
            let p = self.para_tag(at);
            *self.explicit.borrow_mut() = Some(vec![p, "br".into()]);
            let mut it = self.it(at);
            self.buf.insert(&mut it, "\n");
            *self.explicit.borrow_mut() = None;
        });
        self.text.scroll_mark_onscreen(&self.buf.get_insert());
    }

    fn key_tab(&self, d: i32) -> bool {
        if self.state().1.kind.is_some() {
            self.change_level(d);
            return true;
        }
        d < 0
    }

    fn key_backspace(&self) -> bool {
        if self.selection().is_some() {
            return false;
        }
        let cur = self.cursor();
        let (s, _) = self.para_range(cur);
        let a = pattrs(&self.para_tag(s));
        if a.kind.is_some() && cur == self.text_start(s) {
            self.set_para(|p| p.kind = None, true);
            return true;
        }
        if cur == s && a.indent > 0 {
            let ind = a.indent - 1;
            self.set_para(move |p| p.indent = ind, false);
            return true;
        }
        false
    }

    fn skip_marker(&self) {
        let at = self.cursor();
        if at == self.linestart(at) && self.text_start(at) != at && self.selection().is_none() {
            self.buf.place_cursor(&self.it(at + 1));
        }
    }

    // -- clipboard with formatting -------------------------------------------------------------

    /// Copy (or cut) the selection: plain text to the clipboard, the fragment kept here.
    fn copy(&self, cut: bool) -> bool {
        let Some((a, b)) = self.selection() else { return false };
        let frag: Frag = self.fragment(a, b).into_iter().filter(|x| !matches!(x, Item::Marker(_))).collect();
        let plain = self.buf.text(&self.it(a), &self.it(b), false).to_string();
        self.setting_clip.set(true);
        self.text.clipboard().set_text(&plain);
        self.setting_clip.set(false);
        *self.clip.borrow_mut() = Some((plain, frag));
        if cut && self.text.is_editable() {
            self.one_step(|| {
                self.buf.delete_selection(true, true);
            });
            self.text.scroll_mark_onscreen(&self.buf.get_insert());
        }
        true
    }

    fn on_clipboard(&self, cb: &gdk::Clipboard) {
        if self.setting_clip.get() || !cb.is_local() {
            return;
        }
        let Some(content) = cb.content() else { return };
        let ty = gtk::TextBuffer::static_type();
        if !content.formats().contains_type(ty) {
            return;
        }
        let Some(tb) = content.value(ty).ok().and_then(|v| v.get::<gtk::TextBuffer>().ok()) else { return };
        let copied = tb.text(&tb.start_iter(), &tb.end_iter(), false).to_string();
        let Some((a, b)) = self.selection() else { return };
        if copied != self.buf.text(&self.it(a), &self.it(b), false).as_str() {
            return; // a table cell's copy, or another text's
        }
        let frag: Frag = self.fragment(a, b).into_iter().filter(|x| !matches!(x, Item::Marker(_))).collect();
        self.setting_clip.set(true);
        cb.set_text(&copied);
        self.setting_clip.set(false);
        *self.clip.borrow_mut() = Some((copied, frag));
    }

    fn paste(&self, got: Option<String>) {
        let Some(text) = got else { return };
        let same = self.clip.borrow().as_ref().is_some_and(|(t, _)| *t == text);
        if !same {
            self.one_step(|| {
                self.buf.delete_selection(true, true);
                self.buf.insert_interactive_at_cursor(&text, true);
            });
            self.text.scroll_mark_onscreen(&self.buf.get_insert());
            return;
        }
        let frag: Frag = self.clip.borrow().as_ref().map_or(vec![], |(_, f)| {
            f.iter()
                .map(|x| match x {
                    Item::Obj(_, o, t) => Item::Obj(self.new_id(), o.clone(), t.clone()),
                    other => other.clone(),
                })
                .collect()
        });
        self.one_step(|| {
            self.buf.delete_selection(true, true);
            let at = self.cursor();
            self.begin(None);
            let end = self.put(at, &frag);
            let f2 = self.fragment(at, end);
            self.record(Op::Put { i1: at, i2: end, frag: f2 });
            self.normalize(at, end, true);
            self.end();
            self.buf.place_cursor(&self.it(end));
        });
        self.notify_modified();
        self.text.scroll_mark_onscreen(&self.buf.get_insert());
    }

    // -- pages ------------------------------------------------------------------------------------

    /// The width of the text column, in pixels.
    pub fn column_px(&self) -> f64 {
        let w = self.text.width();
        if w < 50 {
            return self.text.create_pango_layout(Some("n")).pixel_size().0 as f64 * 84.0;
        }
        (w - self.text.left_margin() - self.text.right_margin() - 4).max(100) as f64
    }

    /// Pixels per point for objects: the page's text width fills the column.
    pub fn pt_px(&self) -> f64 {
        let text_w = self.meta.borrow().page.text_width().max(100.0);
        self.column_px() / text_w
    }

    pub fn page(&self) -> PageSetup {
        self.meta.borrow().page.clone()
    }

    pub fn base_size(&self) -> f64 {
        let s = self.meta.borrow().size;
        if s > 0.0 { s } else { 11.0 }
    }

    pub fn base_font(&self) -> Option<String> {
        self.meta.borrow().font.clone()
    }

    pub fn set_page(&self, page: PageSetup) {
        self.meta.borrow_mut().page = page;
        self.redraw_all();
        self.notify_modified();
    }
}
