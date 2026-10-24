//! The preview panel: Markdown rendered as you type, the PDF of a LaTeX file (with its errors), or a
//! formatted document as it prints.

use crate::app::{kind_label, App, AppRc};
use crate::pdfview::PdfView;
use crate::theme::UI_SIZE;
use crate::widgets;
use crate::workers::{self, Cancel, Lane};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, pango};
use rewrite_core::latex::{self, BuildResult};
use rewrite_core::mdpreview::{self, Block, Run, Target};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

/// The view's own margin; Tk's margins were counted from it, GTK's tag margins replace it.
const PAD: i32 = 18;
/// Fonts to fall back on for characters the text font lacks (task boxes, symbols).
const FALLBACK: &str = "Segoe UI Symbol, Segoe UI Emoji, Noto Sans Symbols2, DejaVu Sans";
const HEADING_SCALE: [(usize, f64); 6] = [(1, 1.9), (2, 1.5), (3, 1.25), (4, 1.1), (5, 1.0), (6, 0.92)];
/// The tags every render uses, in the order of their priority (later ones win).
const STATIC_TAGS: &[&str] = &[
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "li0", "li1", "li2", "li3", "li4", "li5", "li6", "li7", "q1", "q2", "q3", "q4",
    "q5", "q6", "q7", "q8", "pre", "rule", "gap", "th", "b", "i", "bi", "code", "math", "sup", "strike", "link", "muted",
];

// -- the style sheet ---------------------------------------------------------------------------------------

thread_local! {
    static CSS: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
}

fn css_text(app: &App) -> String {
    let t = app.theme();
    format!(
        "textview.md-view, textview.md-view text {{ background-color: {surface}; color: {text}; font-family: \"{fam}\", {fallback}; font-size: {size}pt; }}\n\
         textview.md-view text selection {{ background-color: {select}; color: {text}; }}\n\
         textview.error-list, textview.error-list text {{ background-color: {panel}; color: {text}; font-size: {small}pt; }}\n\
         .preview-msg {{ background-color: {panel}; }}\n",
        surface = t.c("surface"),
        text = t.c("text"),
        select = t.c("select"),
        panel = t.c("panel"),
        fam = t.editor_font(),
        size = 6.0f64.max(t.size.round()),
        small = UI_SIZE - 1.0,
        fallback = FALLBACK.split(", ").map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", "),
    )
}

fn refresh_css(app: &App) {
    CSS.with(|c| {
        if let Some(p) = c.get() {
            p.load_from_string(&css_text(app));
        }
    });
}

fn ensure_css(app: &AppRc) {
    CSS.with(|c| {
        if c.get().is_some() {
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css_text(app));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
        }
        let _ = c.set(provider);
        let me = app.weak();
        app.theme().on_change(move || {
            if let Some(app) = me.upgrade() {
                refresh_css(&app);
            }
        });
    });
}

// -- Markdown --------------------------------------------------------------------------------------------

/// Markdown drawn in a read-only text view.
struct MarkdownView {
    app: Weak<App>,
    scroll: gtk::ScrolledWindow,
    text: gtk::TextView,
    buf: gtk::TextBuffer,
    tags: HashMap<&'static str, gtk::TextTag>,
    /// Tags made for one render (links and tables), removed before the next.
    dynamic: RefCell<Vec<gtk::TextTag>>,
    links: RefCell<HashMap<String, String>>,
    base_dir: RefCell<Option<PathBuf>>,
    shown: RefCell<Option<(String, Option<PathBuf>, String, String, u64)>>,
    keep_frac: Rc<Cell<Option<f64>>>,
}

impl MarkdownView {
    fn new(app: &AppRc) -> Rc<MarkdownView> {
        let buf = gtk::TextBuffer::new(None);
        let text = gtk::TextView::with_buffer(&buf);
        text.add_css_class("md-view");
        text.set_editable(false);
        text.set_cursor_visible(false);
        text.set_focusable(false);
        text.set_wrap_mode(gtk::WrapMode::Word);
        text.set_left_margin(PAD);
        text.set_right_margin(PAD);
        text.set_top_margin(12);
        text.set_bottom_margin(12);
        text.set_pixels_inside_wrap(2);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_child(Some(&text));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);
        let tags = STATIC_TAGS.iter().map(|&n| (n, buf.create_tag(Some(n), &[]).expect("a new tag"))).collect();
        let md = Rc::new(MarkdownView {
            app: app.weak(),
            scroll: scroll.clone(),
            text: text.clone(),
            buf,
            tags,
            dynamic: RefCell::new(vec![]),
            links: RefCell::new(HashMap::new()),
            base_dir: RefCell::new(None),
            shown: RefCell::new(None),
            keep_frac: Rc::new(Cell::new(None)),
        });
        // keep the place in the text across renders
        let keep = md.keep_frac.clone();
        scroll.vadjustment().connect_upper_notify(move |adj| {
            if let Some(f) = keep.get() {
                adj.set_value(f * adj.upper());
            }
        });
        // links open on a click; the pointer shows where they are
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let w = Rc::downgrade(&md);
        click.connect_released(move |_, _, x, y| {
            let Some(md) = w.upgrade() else { return };
            if let Some(url) = md.link_at(x, y) {
                if let Some(app) = md.app.upgrade() {
                    app.open_link(&url);
                }
            }
        });
        text.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let w = Rc::downgrade(&md);
        motion.connect_motion(move |_, x, y| {
            let Some(md) = w.upgrade() else { return };
            md.text.set_cursor_from_name(Some(if md.link_at(x, y).is_some() { "pointer" } else { "default" }));
        });
        text.add_controller(motion);
        text.set_cursor_from_name(Some("default"));
        md
    }

    fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let (bx, by) = self.text.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let it = self.text.iter_at_location(bx, by)?;
        let links = self.links.borrow();
        it.tags().iter().find_map(|t| t.name().and_then(|n| links.get(n.as_str()).cloned()))
    }

    fn tag(&self, name: &str) -> &gtk::TextTag {
        &self.tags[name]
    }

    /// Point size of a font of the preview.
    fn font_size(app: &App, mono: bool, scale: f64) -> f64 {
        6.0f64.max((app.theme().size * scale * if mono { 0.92 } else { 1.0 }).round())
    }

    fn set_font(app: &App, tag: &gtk::TextTag, mono: bool, scale: f64, bold: bool, italic: bool) {
        let fam = if mono { app.theme().families.mono.clone() } else { app.theme().editor_font().to_string() };
        tag.set_family(Some(&format!("{fam}, {FALLBACK}")));
        tag.set_size_points(Self::font_size(app, mono, scale));
        tag.set_weight(if bold { 700 } else { 400 });
        tag.set_style(if italic { pango::Style::Italic } else { pango::Style::Normal });
    }

    fn font_desc(app: &App, bold: bool) -> pango::FontDescription {
        let mut d = pango::FontDescription::new();
        d.set_family(app.theme().editor_font());
        d.set_size((Self::font_size(app, false, 1.0) * pango::SCALE as f64) as i32);
        d.set_weight(if bold { pango::Weight::Bold } else { pango::Weight::Normal });
        d
    }

    /// (Re)define the tags for the current theme and text size.
    fn style(&self, app: &App) {
        let t = app.theme();
        let rgba = |k: &str| t.rgba(k);
        let size = t.size;
        let p = self.tag("p");
        p.set_pixels_below_lines((size * 1.2).round() as i32); // grows with the text size
        for (lvl, sc) in HEADING_SCALE {
            let h = self.tag(&format!("h{lvl}"));
            Self::set_font(app, h, false, sc, true, false);
            h.set_pixels_above_lines(if lvl < 3 { 10 } else { 6 });
            h.set_pixels_below_lines(5);
            h.set_foreground_rgba(Some(&rgba(if lvl < 6 { "text" } else { "muted" })));
        }
        for d in 0..8 {
            let left = 8 + 22 * d;
            let li = self.tag(&format!("li{d}"));
            // the first line starts at `left` with the marker; the text (and wrapped lines) at left + 20
            li.set_left_margin(PAD + left);
            li.set_indent(-20);
            li.set_pixels_below_lines(2);
            let mut tabs = pango::TabArray::new(1, true);
            tabs.set_tab(0, pango::TabAlign::Left, 20);
            li.set_tabs(Some(&tabs));
            let q = self.tag(&format!("q{}", d + 1));
            q.set_left_margin(PAD + 18 * (d + 1));
            q.set_foreground_rgba(Some(&rgba("muted")));
            q.set_pixels_below_lines(5);
        }
        let pre = self.tag("pre");
        Self::set_font(app, pre, true, 1.0, false, false);
        pre.set_paragraph_background_rgba(Some(&rgba("hover")));
        pre.set_left_margin(PAD + 12);
        pre.set_right_margin(PAD + 12);
        pre.set_wrap_mode(gtk::WrapMode::None);
        pre.set_pixels_inside_wrap(0);
        let rule = self.tag("rule");
        rule.set_size_points(1.0); // a line this small is drawn as a thin rule
        rule.set_paragraph_background_rgba(Some(&rgba("border")));
        self.tag("gap").set_size_points(2.0f64.max((size / 3.0).floor()));
        Self::set_font(app, self.tag("th"), false, 1.0, true, false);
        Self::set_font(app, self.tag("b"), false, 1.0, true, false);
        Self::set_font(app, self.tag("i"), false, 1.0, false, true);
        Self::set_font(app, self.tag("bi"), false, 1.0, true, true);
        let code = self.tag("code");
        Self::set_font(app, code, true, 1.0, false, false);
        code.set_background_rgba(Some(&rgba("hover")));
        let math = self.tag("math");
        Self::set_font(app, math, true, 1.0, false, true);
        math.set_foreground_rgba(Some(&rgba("muted")));
        let sup = self.tag("sup");
        Self::set_font(app, sup, false, 0.75, false, false);
        sup.set_rise(4 * pango::SCALE);
        sup.set_foreground_rgba(Some(&rgba("accent")));
        let strike = self.tag("strike");
        strike.set_strikethrough(true);
        strike.set_foreground_rgba(Some(&rgba("muted")));
        let link = self.tag("link");
        link.set_foreground_rgba(Some(&rgba("accent")));
        link.set_underline(pango::Underline::Single);
        self.tag("muted").set_foreground_rgba(Some(&rgba("faint")));
    }

    fn render(&self, source: &str, base_dir: Option<PathBuf>) {
        let Some(app) = self.app.upgrade() else { return };
        let key = {
            let t = app.theme();
            (source.to_string(), base_dir.clone(), t.p.name.to_string(), t.editor_family.clone(), t.size.to_bits())
        };
        if self.shown.borrow().as_ref() == Some(&key) {
            return;
        }
        *self.shown.borrow_mut() = Some(key);
        let adj = self.scroll.vadjustment();
        let top = if adj.upper() > 0.0 { adj.value() / adj.upper() } else { 0.0 };
        self.buf.set_text("");
        let table = self.buf.tag_table();
        for t in self.dynamic.borrow_mut().drain(..) {
            table.remove(&t);
        }
        self.links.borrow_mut().clear();
        *self.base_dir.borrow_mut() = base_dir;
        self.style(&app);
        let blocks = mdpreview::parse(source);
        let mut prev: Option<&'static str> = None;
        for (n, b) in blocks.iter().enumerate() {
            let kind = block_kind(b);
            if matches!(prev, Some("item" | "quote")) && prev != Some(kind) {
                self.insert("\n", &["gap"]); // lists and quotes are set tight; open up after them
            }
            self.block(&app, n, b);
            prev = Some(kind);
        }
        if blocks.is_empty() {
            self.insert("Nothing to preview yet.", &["muted"]);
        }
        // back to the same place once the new text is measured
        self.keep_frac.set(Some(top));
        adj.set_value(top * adj.upper());
        let keep = self.keep_frac.clone();
        workers::later("md-preview-keep", 400, move || keep.set(None));
    }

    fn insert(&self, text: &str, tags: &[&str]) {
        let mut end = self.buf.end_iter();
        self.buf.insert_with_tags_by_name(&mut end, text, tags);
    }

    fn runs(&self, app: &App, runs: &[Run], base: &str, plain: bool) {
        for r in runs {
            if r.styles.contains("img") {
                if let Some(Target::Image { alt, src }) = &r.target {
                    self.image(alt, src, base);
                }
                continue;
            }
            let mut tags: Vec<String> = vec![base.to_string()];
            let has = |s: &str| r.styles.contains(s);
            if !plain {
                let font = if has("code") {
                    Some("code")
                } else if has("math") {
                    Some("math")
                } else if has("b") && has("i") {
                    Some("bi")
                } else if has("b") {
                    Some("b")
                } else if has("i") {
                    Some("i")
                } else {
                    None
                };
                if let Some(f) = font {
                    tags.push(f.into());
                }
            } else if has("code") {
                tags.push("code".into());
            }
            for s in ["strike", "sup"] {
                if has(s) {
                    tags.push(s.into());
                }
            }
            if let Some(url) = r.link() {
                let name = format!("a:{}", self.links.borrow().len() + 1);
                if let Some(t) = self.buf.create_tag(Some(&name), &[]) {
                    self.dynamic.borrow_mut().push(t);
                }
                self.links.borrow_mut().insert(name.clone(), url.to_string());
                tags.push("link".into());
                tags.push(name);
            }
            let _ = app;
            let refs: Vec<&str> = tags.iter().map(String::as_str).collect();
            self.insert(&r.text, &refs);
        }
    }

    fn block(&self, app: &App, n: usize, b: &Block) {
        match b {
            Block::Heading { level, runs } => {
                let tag = format!("h{}", level.clamp(&1, &6));
                self.runs(app, runs, &tag, true);
                self.insert("\n", &[&tag]);
            }
            Block::Para(runs) => {
                self.runs(app, runs, "p", false);
                self.insert("\n", &["p"]);
            }
            Block::Item { depth, marker, checked, runs } => {
                let tag = format!("li{}", depth.min(&7));
                let bullet = match checked {
                    None => marker.as_str(),
                    Some(true) => "☑",
                    Some(false) => "☐",
                };
                if checked.is_none() {
                    self.insert(&format!("{bullet}\t"), &[&tag, "muted"]);
                } else {
                    self.insert(&format!("{bullet}\t"), &[&tag]);
                }
                self.runs(app, runs, &tag, false);
                self.insert("\n", &[&tag]);
            }
            Block::Quote { depth, runs } => {
                let tag = format!("q{}", depth.clamp(&1, &8));
                self.runs(app, runs, &tag, false);
                self.insert("\n", &[&tag]);
            }
            Block::Code { source, .. } | Block::Math(source) => {
                self.insert(&format!("{source}\n"), &["pre"]);
                self.insert("\n", &["gap"]);
            }
            Block::Rule => {
                self.insert("\n", &["gap"]);
                self.insert("\u{200B}\n", &["rule"]);
                self.insert("\n", &["gap"]);
            }
            Block::Table { head, align, rows } => self.table(app, n, head, align, rows),
        }
    }

    fn table(&self, app: &App, n: usize, head: &[Vec<Run>], align: &[&str], rows: &[Vec<Vec<Run>>]) {
        let cols = rows.iter().map(Vec::len).chain([head.len()]).max().unwrap_or(0);
        let mut width = vec![0i32; cols];
        let measure = |bold: bool, s: &str| {
            let layout = self.text.create_pango_layout(Some(s));
            layout.set_font_description(Some(&Self::font_desc(app, bold)));
            layout.pixel_size().0
        };
        for (k, row) in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            for (j, cell) in row.iter().enumerate() {
                let s: String = cell.iter().map(|r| r.text.as_str()).collect();
                width[j] = width[j].max(measure(k == 0, &s));
            }
        }
        let mut tabs = pango::TabArray::new(cols as i32, true);
        let mut pos = 4;
        for (j, &w) in width.iter().enumerate() {
            let a = align.get(j).copied().unwrap_or("left");
            let (x, al) = match a {
                "right" => (pos + w, pango::TabAlign::Right),
                "center" => (pos + w / 2, pango::TabAlign::Center),
                _ => (pos, pango::TabAlign::Left),
            };
            tabs.set_tab(j as i32, al, x);
            pos += w + 26;
        }
        let name = format!("tbl{n}");
        if let Some(t) = self.buf.create_tag(Some(&name), &[]) {
            t.set_tabs(Some(&tabs));
            t.set_wrap_mode(gtk::WrapMode::None);
            t.set_pixels_below_lines(3);
            self.dynamic.borrow_mut().push(t);
        }
        for (k, row) in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            let start = self.buf.end_iter().offset();
            for cell in row {
                self.insert("\t", &[&name]);
                self.runs(app, cell, &name, k == 0);
            }
            self.insert("\n", &[&name]);
            if k == 0 {
                let (a, b) = (self.buf.iter_at_offset(start), self.buf.end_iter());
                self.buf.apply_tag(self.tag("th"), &a, &b);
                self.insert("\u{200B}\n", &["rule"]);
            }
        }
        self.insert("\n", &["gap"]);
    }

    fn image(&self, alt: &str, src: &str, base: &str) {
        let path = if Path::new(src).is_absolute() {
            PathBuf::from(src)
        } else {
            self.base_dir.borrow().clone().unwrap_or_default().join(src)
        };
        let lower = src.to_lowercase();
        let mut tex: Option<gdk::Texture> = None;
        if !src.contains("://") && path.is_file() && (lower.ends_with(".png") || lower.ends_with(".gif")) {
            if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file(&path) {
                let room = 120.max(self.text.width() - 2 * 30);
                let pb = if pb.width() > room {
                    let f = (pb.width() as f64 / room as f64).ceil() as i32;
                    pb.scale_simple((pb.width() / f).max(1), (pb.height() / f).max(1), gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or(pb)
                } else {
                    pb
                };
                tex = Some(gdk::Texture::for_pixbuf(&pb));
            }
        }
        match tex {
            Some(t) => {
                let mut end = self.buf.end_iter();
                self.buf.insert_paintable(&mut end, &t);
            }
            None => {
                let name = if alt.is_empty() { Path::new(src).file_name().map_or(src.to_string(), |n| n.to_string_lossy().into_owned()) } else { alt.to_string() };
                self.insert(&format!("[image: {name}]"), &[base, "muted"]);
            }
        }
    }
}

fn block_kind(b: &Block) -> &'static str {
    match b {
        Block::Heading { .. } => "heading",
        Block::Para(_) => "para",
        Block::Code { .. } => "code",
        Block::Rule => "rule",
        Block::Item { .. } => "item",
        Block::Quote { .. } => "quote",
        Block::Table { .. } => "table",
        Block::Math(_) => "math",
    }
}

// -- the panel ---------------------------------------------------------------------------------------------

struct PvState {
    /// Stops the compiler (or drops the layout) when a newer build starts.
    cancel: Option<Cancel>,
    /// markdown | latex | rich, or None
    mode: Option<String>,
    /// The LaTeX source that the shown PDF was made from.
    built: Option<String>,
    building: bool,
    /// (status text, colour)
    state: (String, String),
    summary: String,
    /// The document whose pages are shown (its number in `Pv::doc_gen`).
    pdf_of: Option<u64>,
    /// error tag name -> line
    error_lines: HashMap<String, u32>,
}

struct Pv {
    app: Weak<App>,
    root: gtk::Box,
    status: gtk::Label,
    zoom_in: gtk::Button,
    zoom_out: gtk::Button,
    errors: gtk::TextView,
    errors_box: gtk::ScrolledWindow,
    line_tag: gtk::TextTag,
    stack: gtk::Stack,
    md: Rc<MarkdownView>,
    pdf: PdfView,
    message: gtk::Label,
    message_box: gtk::Box,
    lane: Lane,
    st: RefCell<PvState>,
    /// Counts the documents put in the editor: it goes up whenever the app loads a text or a model into it
    /// (a new document, an opened file), and not for edits, saves or a change of format.
    doc_gen: Rc<Cell<u64>>,
}

pub struct Preview {
    p: Rc<Pv>,
}

impl Preview {
    pub fn new(app: &AppRc) -> Preview {
        ensure_css(app);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("panel");
        root.set_size_request(240, -1);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.set_margin_start(10);
        head.set_margin_end(4);
        head.set_margin_top(2);
        head.set_margin_bottom(2);
        head.append(&widgets::label("PREVIEW", &["tiny"]));
        let status = widgets::label("", &["small", "muted"]);
        status.set_margin_start(8);
        status.set_hexpand(true);
        status.set_ellipsize(pango::EllipsizeMode::End);
        head.append(&status);
        let weak_pv: Rc<RefCell<Weak<Pv>>> = Rc::new(RefCell::new(Weak::new()));
        let with = |f: fn(&Rc<Pv>)| {
            let w = weak_pv.clone();
            move || {
                if let Some(p) = w.borrow().upgrade() {
                    f(&p);
                }
            }
        };
        let zoom_out = widgets::sized_icon_button("minus", 14, "Zoom out", with(|p| p.pdf.set_zoom(p.pdf.zoom() / 1.2)));
        let zoom_in = widgets::sized_icon_button("plus", 14, "Zoom in (Ctrl + mouse wheel)", with(|p| p.pdf.set_zoom(p.pdf.zoom() * 1.2)));
        head.append(&zoom_out);
        head.append(&zoom_in);
        head.append(&widgets::sized_icon_button("refresh", 14, "Refresh the preview / rebuild the PDF (F5)", with(|p| p.refresh(true))));
        let me = app.weak();
        head.append(&widgets::sized_icon_button("close", 14, "Close the preview", move || {
            if let Some(app) = me.upgrade() {
                app.set_preview(false, true);
            }
        }));
        root.append(&head);
        root.append(&widgets::hairline());
        // LaTeX errors, listed above the page; click one to jump to its line
        let errors = gtk::TextView::new();
        errors.add_css_class("error-list");
        errors.set_editable(false);
        errors.set_cursor_visible(false);
        errors.set_focusable(false);
        errors.set_wrap_mode(gtk::WrapMode::WordChar);
        errors.set_left_margin(10);
        errors.set_right_margin(10);
        errors.set_top_margin(5);
        errors.set_bottom_margin(5);
        errors.set_cursor_from_name(Some("default"));
        let line_tag = errors.buffer().create_tag(Some("line"), &[]).expect("a new tag");
        let errors_box = gtk::ScrolledWindow::new();
        errors_box.set_hscrollbar_policy(gtk::PolicyType::Never);
        errors_box.set_propagate_natural_height(true);
        errors_box.set_child(Some(&errors));
        errors_box.set_visible(false);
        root.append(&errors_box);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        let md = MarkdownView::new(app);
        let pdf = PdfView::new(app);
        let message = widgets::wrap_label("", &["muted"]);
        message.set_yalign(0.0);
        message.set_valign(gtk::Align::Start);
        message.set_margin_start(14);
        message.set_margin_end(14);
        message.set_margin_top(12);
        message.set_selectable(true);
        message.set_focusable(false);
        let message_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        message_box.add_css_class("preview-msg");
        message_box.append(&message);
        stack.add_child(&md.scroll);
        stack.add_child(&pdf.widget());
        stack.add_child(&message_box);
        root.append(&stack);
        let p = Rc::new(Pv {
            app: app.weak(),
            root,
            status,
            zoom_in,
            zoom_out,
            errors,
            errors_box,
            line_tag,
            stack,
            md,
            pdf,
            message,
            message_box,
            lane: Lane::new("latex"),
            st: RefCell::new(PvState {
                cancel: None,
                mode: None,
                built: None,
                building: false,
                state: (String::new(), "muted".into()),
                summary: String::new(),
                pdf_of: None,
                error_lines: HashMap::new(),
            }),
            doc_gen: Rc::new(Cell::new(0)),
        });
        *weak_pv.borrow_mut() = Rc::downgrade(&p);
        let w = Rc::downgrade(&p);
        p.pdf.on_report(move |msg| {
            if let Some(p) = w.upgrade() {
                p.note(msg);
            }
        });
        // a click on an error puts the cursor on its line
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_PRIMARY);
        let w = Rc::downgrade(&p);
        click.connect_released(move |_, _, x, y| {
            let Some(p) = w.upgrade() else { return };
            if let Some(line) = p.error_at(x, y) {
                if let Some(app) = p.app.upgrade() {
                    app.goto_line(line as usize);
                }
            }
        });
        p.errors.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let w = Rc::downgrade(&p);
        motion.connect_motion(move |_, x, y| {
            let Some(p) = w.upgrade() else { return };
            p.errors.set_cursor_from_name(Some(if p.error_at(x, y).is_some() { "pointer" } else { "default" }));
        });
        p.errors.add_controller(motion);
        p.recolor();
        p.note("");
        Preview { p }
    }
    pub fn widget(&self) -> gtk::Widget {
        self.p.root.clone().upcast()
    }
    /// Show the document again; `manual` = asked for (F5 or the refresh button).
    pub fn refresh(&self, manual: bool) {
        self.p.refresh(manual);
    }
    /// The editor's text changed.
    pub fn text_changed(&self) {
        self.p.text_changed();
    }
    /// Another document is in the editor: pages shown so far belong to the one before.
    pub fn new_document(&self) {
        self.p.doc_gen.set(self.p.doc_gen.get() + 1);
    }
    /// Stop a running build.
    pub fn stop(&self) {
        self.p.stop_build();
        self.p.st.borrow_mut().mode = None;
    }
    pub fn retheme(&self) {
        if let Some(app) = self.p.app.upgrade() {
            refresh_css(&app);
        }
        self.p.recolor();
        let md = self.p.st.borrow().mode.as_deref() == Some("markdown");
        if md {
            self.p.refresh(false);
        }
    }
}

impl Pv {
    fn recolor(&self) {
        let Some(app) = self.app.upgrade() else { return };
        let t = app.theme();
        self.line_tag.set_foreground_rgba(Some(&t.rgba("danger")));
        self.line_tag.set_weight(700);
        self.line_tag.set_size_points(UI_SIZE);
        drop(t);
        let (text, color) = self.st.borrow().state.clone();
        self.set_status(&text, &color);
    }

    fn set_status(&self, text: &str, color: &str) {
        self.st.borrow_mut().state = (text.to_string(), color.to_string());
        self.status.set_label(text);
        for c in ["muted", "warn", "danger", "good"] {
            self.status.remove_css_class(c);
        }
        self.status.add_css_class(color);
    }

    fn show(&self, view: &impl IsA<gtk::Widget>, pdf_tools: bool) {
        self.stack.set_visible_child(view);
        self.zoom_in.set_visible(pdf_tools);
        self.zoom_out.set_visible(pdf_tools);
    }

    fn note(&self, text: &str) {
        self.message.set_label(text);
        self.show(&self.message_box, false);
    }

    fn error_at(&self, x: f64, y: f64) -> Option<u32> {
        let (bx, by) = self.errors.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let it = self.errors.iter_at_location(bx, by)?;
        let st = self.st.borrow();
        it.tags().iter().find_map(|t| t.name().and_then(|n| st.error_lines.get(n.as_str()).copied()))
    }

    fn set_errors(&self, errors: &[latex::LogError]) {
        let buf = self.errors.buffer();
        buf.set_text("");
        let table = buf.tag_table();
        let old: Vec<String> = self.st.borrow_mut().error_lines.drain().map(|(k, _)| k).collect();
        let mut stale = vec![];
        table.foreach(|t| {
            if t.name().is_some_and(|n| n.starts_with("e:")) {
                stale.push(t.clone());
            }
        });
        for t in stale {
            table.remove(&t);
        }
        let _ = old;
        let shown = &errors[..errors.len().min(12)];
        for (n, (line, msg)) in shown.iter().enumerate() {
            let tag = format!("e:{n}");
            let _ = buf.create_tag(Some(&tag), &[]);
            let mut end = buf.end_iter();
            let lead = format!("{}{}", if n > 0 { "\n" } else { "" }, line.map_or("•  ".to_string(), |l| format!("Line {l}  ")));
            buf.insert_with_tags_by_name(&mut end, &lead, &["line", &tag]);
            let mut end = buf.end_iter();
            buf.insert_with_tags_by_name(&mut end, msg, &[&tag]);
            if let Some(l) = line {
                self.st.borrow_mut().error_lines.insert(tag, *l);
            }
        }
        if shown.is_empty() {
            self.errors_box.set_visible(false);
            return;
        }
        let line_h = self.errors.create_pango_layout(Some("Ag")).pixel_size().1.max(10) + 1;
        let lines = shown.len().clamp(1, 6) as i32; // one line per error, at most six
        let h = lines * line_h + 10;
        // the minimum first when the box grows, the maximum first when it shrinks (min must stay <= max)
        if h > self.errors_box.max_content_height() {
            self.errors_box.set_max_content_height(h);
            self.errors_box.set_min_content_height(h);
        } else {
            self.errors_box.set_min_content_height(h);
            self.errors_box.set_max_content_height(h);
        }
        self.errors_box.set_visible(true);
    }

    // -- what to show ------------------------------------------------------------------------------

    /// Bring the preview up to date with the editor (called on open, save, format change and ↻).
    fn refresh(self: &Rc<Self>, manual: bool) {
        let _ = manual;
        let Some(app) = self.app.upgrade() else { return };
        let (kind, path) = {
            let st = app.st.borrow();
            (st.doc.kind.clone(), st.doc.path.clone())
        };
        if kind != "latex" {
            self.stop_build();
            self.set_errors(&[]);
            self.st.borrow_mut().built = None;
        }
        match kind.as_str() {
            "markdown" => {
                self.st.borrow_mut().mode = Some("markdown".into());
                let base = path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
                self.md.render(&app.plain_text(), base);
                self.show(&self.md.scroll, false);
                self.set_status("Markdown · updates as you type", "muted");
            }
            "latex" => {
                self.st.borrow_mut().mode = Some("latex".into());
                self.build();
            }
            "rich" => {
                self.st.borrow_mut().mode = Some("rich".into());
                self.build_rich();
            }
            _ => {
                self.st.borrow_mut().mode = None;
                self.pdf.clear();
                self.st.borrow_mut().pdf_of = None;
                self.set_status("", "muted");
                self.note(&format!(
                    "The preview shows Markdown, LaTeX and formatted (Word, OpenDocument, RTF) documents.\n\n\
                     This one is treated as “{}”. If it is Markdown or LaTeX, click the file name in the status bar and pick the format.",
                    kind_label(&kind)
                ));
            }
        }
    }

    /// The editor text changed: Markdown follows at once, LaTeX waits for a save or ↻.
    fn text_changed(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        let kind = app.st.borrow().doc.kind.clone();
        let mode = self.st.borrow().mode.clone();
        if mode.as_deref() != Some(kind.as_str()) {
            return; // another document is being opened; refresh() follows
        }
        match kind.as_str() {
            "markdown" => self.refresh(false),
            "rich" => {
                self.set_status("Edited · the pages follow in a moment", "muted");
                let w = Rc::downgrade(self);
                workers::later("rich-preview", 1500, move || {
                    if let Some(p) = w.upgrade() {
                        if p.st.borrow().mode.as_deref() == Some("rich") {
                            p.build_rich();
                        }
                    }
                });
            }
            "latex" => {
                let (building, built, color, summary) = {
                    let st = self.st.borrow();
                    (st.building, st.built.clone(), st.state.1.clone(), st.summary.clone())
                };
                if building {
                    return;
                }
                let Some(built) = built else { return };
                if app.plain_text() != built {
                    self.set_status("Edited · save or press ↻ to rebuild", "warn");
                } else if color == "warn" {
                    self.set_status(&summary, "muted");
                }
            }
            _ => {}
        }
    }

    fn stop_build(&self) {
        let mut st = self.st.borrow_mut();
        if let Some(c) = st.cancel.take() {
            c.set();
        }
        st.building = false;
    }

    /// Show the pages of another document only once they are drawn, never the previous document's.
    fn own_pages(&self) {
        let key = self.doc_gen.get();
        let same = self.st.borrow().pdf_of.as_ref() == Some(&key);
        if !same {
            self.st.borrow_mut().pdf_of = Some(key);
            self.pdf.clear();
        }
    }

    fn build(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        self.stop_build();
        if !rewrite_pdfium::available() {
            self.set_status("", "muted");
            self.note("Showing the PDF needs the PDFium library, which comes with Rewrite.\n\nReinstall Rewrite to get it back.");
            return;
        }
        let source = app.plain_text();
        if source.trim().is_empty() {
            self.pdf.clear();
            self.set_status("", "muted");
            self.note("Nothing to build yet.");
            return;
        }
        let cancel = Cancel::new();
        {
            let mut st = self.st.borrow_mut();
            st.cancel = Some(cancel.clone());
            st.building = true;
        }
        self.set_status("Building…", "muted");
        self.own_pages();
        if !self.pdf.has_data() {
            self.note("Building the PDF…");
        }
        let (path, enc) = {
            let st = app.st.borrow();
            (st.doc.path.clone(), st.doc.encoding.clone())
        };
        let src = source.clone();
        let flag = cancel.clone();
        let w = Rc::downgrade(self);
        self.lane.submit(
            move || latex::build(&src, path.as_deref(), &enc, &|| flag.is_set()).ok(),
            move |res| {
                if let Some(p) = w.upgrade() {
                    p.built(cancel, source, res);
                }
            },
        );
    }

    fn built(self: &Rc<Self>, cancel: Cancel, source: String, res: Result<Option<BuildResult>, String>) {
        let current = self.st.borrow().cancel.as_ref().is_some_and(|c| c.same(&cancel));
        if !current || matches!(res, Ok(None)) {
            return; // a newer build has started
        }
        self.st.borrow_mut().building = false;
        let res = match res {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                self.set_status("Build failed", "danger");
                self.note(&format!("The build could not run:\n{e}"));
                return;
            }
        };
        self.st.borrow_mut().built = Some(source);
        self.set_errors(&res.errors);
        match res.pdf {
            Some(pdf) => {
                self.pdf.show(Arc::from(pdf));
                self.show(&self.pdf.widget(), true);
            }
            None => {
                self.pdf.clear();
                let tail = if !res.log.is_empty() && res.errors.is_empty() {
                    let lines: Vec<&str> = res.log.trim().split('\n').collect();
                    lines[lines.len().saturating_sub(12)..].join("\n")
                } else {
                    String::new()
                };
                self.note(&format!("{}{}", res.message, if tail.is_empty() { String::new() } else { format!("\n\n{tail}") }));
            }
        }
        let n = res.errors.len();
        let summary = if res.engine.is_empty() { String::new() } else { format!("{} · {:.1} s", res.engine, res.seconds) };
        self.st.borrow_mut().summary = summary.clone();
        if n > 0 {
            self.set_status(&format!("{n} error{} · {summary}", if n != 1 { "s" } else { "" }), "danger");
        } else {
            self.set_status(&summary, "muted");
        }
        self.text_changed(); // typed during the build?
    }

    /// Typeset a formatted document as it will print (Rewrite's own layout, as in a PDF export).
    fn build_rich(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else { return };
        if !app.rich_active() {
            return;
        }
        self.stop_build();
        let model = app.rich().document();
        self.own_pages();
        if !self.pdf.has_data() {
            self.note("Laying out the pages…");
        }
        self.set_status("Laying out…", "muted");
        let token = Cancel::new();
        self.st.borrow_mut().cancel = Some(token.clone());
        let generic = match model.font.as_deref().map(str::to_lowercase).as_deref() {
            Some("calibri" | "arial" | "helvetica" | "segoe ui" | "verdana") => "sans",
            _ => "serif",
        }
        .to_string();
        let w = Rc::downgrade(self);
        self.lane.submit(
            move || rewrite_core::layout::render(&model, &rewrite_core::layout::Options { generic, numbers: None, page: None }),
            move |res| {
                let Some(p) = w.upgrade() else { return };
                {
                    let st = p.st.borrow();
                    if !st.cancel.as_ref().is_some_and(|c| c.same(&token)) || st.mode.as_deref() != Some("rich") {
                        return;
                    }
                }
                p.st.borrow_mut().cancel = None;
                match res {
                    Ok(Ok(pdf)) => {
                        p.pdf.show(Arc::from(pdf));
                        p.show(&p.pdf.widget(), true);
                        p.set_status("As it prints · Rewrite's layout", "muted");
                    }
                    Ok(Err(e)) | Err(e) => {
                        p.set_status("Preview failed", "danger");
                        p.note(&format!("The document could not be laid out:\n{e}"));
                    }
                }
            },
        );
    }
}
