//! The find and replace bar above the editor.
//!
//! Finds with a regular expression in the editor text (Python's syntax: lookaround and backreferences
//! work). The current match is the editor's selection, so it stays selected when the bar closes; the
//! other matches are highlighted around the view.

use crate::app::{App, AppRc, Snap};
use crate::engine;
use crate::theme::{KEY_NEXT, KEY_PREV, KEY_REPLACE, MOD};
use crate::widgets::{self, Toggle};
use crate::workers;
use fancy_regex::{Captures, Regex, RegexBuilder};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// Matches counted; past this the count reads "100,000+".
const LIMIT: usize = 100_000;
/// Characters before and after the view whose matches are highlighted.
const MARGIN: usize = 20_000;
/// Icon size of the bar's buttons.
const TOOL: i32 = 14;

#[derive(Default)]
struct FindState {
    visible: bool,
    replacing: bool,
    /// The text that was searched.
    snap: Option<Rc<Snap>>,
    rx: Option<Rc<Regex>>,
    /// (short text, detail) when the pattern or the replacement is wrong
    error: Option<(String, String)>,
    /// The match selected last, as character offsets.
    picked: Option<(usize, usize)>,
    /// Every match, as character offsets (buffer positions while the text has not changed).
    matches: Rc<Vec<(usize, usize)>>,
    more: bool,
    lead: i32,
    /// The matches that are highlighted: (the list, first, end).
    painted: Option<(Rc<Vec<(usize, usize)>>, usize, usize)>,
}

pub struct FindBar {
    app: Weak<App>,
    root: gtk::Box,
    body: gtk::Grid,
    chev: gtk::Button,
    find_e: gtk::Entry,
    repl_e: gtk::Entry,
    repl_btns: gtk::Box,
    count: gtk::Label,
    case: Toggle,
    word: Toggle,
    regex: Toggle,
    st: RefCell<FindState>,
}

/// A callback that runs `f` on the find bar, if the app is still there.
fn on_bar(app: &Weak<App>, f: impl Fn(&FindBar) + 'static) -> impl Fn() + 'static {
    let a = app.clone();
    move || {
        if let Some(app) = a.upgrade() {
            if let Some(bar) = app.find.get() {
                f(bar);
            }
        }
    }
}

impl FindBar {
    pub fn new(app: &AppRc) -> FindBar {
        let weak = app.weak();
        let s = app.settings().clone();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("surface");
        root.set_visible(false);
        let g = gtk::Grid::new();
        g.set_margin_start(6);
        g.set_margin_end(6);
        g.set_margin_top(4);
        g.set_margin_bottom(4);
        g.set_halign(gtk::Align::Start);
        root.append(&g);
        root.append(&widgets::hairline());

        let tool = |name: &str, tip: &str, f: Box<dyn Fn()>| widgets::sized_icon_button(name, TOOL, tip, move || f());
        let chev = tool("chev-right", &format!("Replace ({KEY_REPLACE})"), Box::new(on_bar(&weak, |b| b.toggle_replace())));
        g.attach(&chev, 0, 0, 1, 1);
        let find_e = entry("Find");
        find_e.set_margin_start(2);
        find_e.set_margin_end(4);
        g.attach(&find_e, 1, 0, 1, 1);
        let count = widgets::label("", &["small", "muted"]);
        count.set_xalign(0.5);
        count.set_width_chars(11); // "999 of 999" and a little room
        g.attach(&count, 2, 0, 1, 1);
        let opt = on_bar(&weak, |b| b.option());
        let opt = Rc::new(opt);
        let o = opt.clone();
        let case = Toggle::new(None, "Aa", s.find_case, "Match case", move |_| o());
        let o = opt.clone();
        let word = Toggle::new(None, "ab", s.find_word, "Whole words only", move |_| o());
        let o = opt.clone();
        let regex =
            Toggle::new(None, ".*", s.find_regex, "Regular expression: in the replacement, \\1 puts back the first group", move |_| o());
        if let Some(l) = word.button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_markup("<u>ab</u>");
        }
        if let Some(l) = regex.button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_markup("<b>.*</b>");
        }
        for (k, t) in [&case, &word, &regex].iter().enumerate() {
            t.button.add_css_class("compact");
            g.attach(&t.button, 3 + k as i32, 0, 1, 1);
        }
        g.attach(&widgets::separator(4), 6, 0, 1, 1);
        g.attach(&tool("up", &format!("Previous match (Shift+Enter, {KEY_PREV})"), Box::new(on_bar(&weak, |b| b.next(true)))), 7, 0, 1, 1);
        g.attach(&tool("down", &format!("Next match (Enter, {KEY_NEXT})"), Box::new(on_bar(&weak, |b| b.next(false)))), 8, 0, 1, 1);
        g.attach(&tool("close", "Close (Esc)", Box::new(on_bar(&weak, |b| b.close()))), 9, 0, 1, 1);

        let repl_e = entry("Replace");
        repl_e.set_margin_start(2);
        repl_e.set_margin_end(4);
        repl_e.set_margin_top(4);
        g.attach(&repl_e, 1, 1, 1, 1);
        let repl_btns = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        repl_btns.set_margin_top(4);
        let alt = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
        let b1 = widgets::flat_button("Replace", "soft", Some("Replace this match, then go to the next (Enter)"), on_bar(&weak, |b| b.replace()));
        let b2 = widgets::flat_button("Replace all", "soft", Some(&format!("Replace every match ({MOD}+{alt}+Enter)")), on_bar(&weak, |b| b.replace_all()));
        for b in [&b1, &b2] {
            b.add_css_class("small");
            b.add_css_class("compact");
            repl_btns.append(b);
        }
        g.attach(&repl_btns, 2, 1, 8, 1);
        repl_e.set_visible(false);
        repl_btns.set_visible(false);

        let bar = FindBar {
            app: weak.clone(),
            root,
            body: g,
            chev,
            find_e: find_e.clone(),
            repl_e: repl_e.clone(),
            repl_btns,
            count,
            case,
            word,
            regex,
            st: RefCell::new(FindState::default()),
        };
        let a = weak.clone();
        find_e.connect_changed(move |_| {
            let Some(app) = a.upgrade() else { return };
            if app.find.get().is_some_and(|b| b.visible()) {
                let a2 = app.weak();
                workers::later("find", 120, move || {
                    if let Some(app) = a2.upgrade() {
                        app.find().search(true);
                    }
                });
            }
        });
        bar.keys(&find_e, true);
        bar.keys(&repl_e, false);
        // after a theme switch the colours of the count come from the style sheet again
        let redo = on_bar(&weak, |b| b.show_count());
        app.theme().on_change(redo);
        bar
    }

    fn keys(&self, e: &gtk::Entry, find: bool) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let a = self.app.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(app) = a.upgrade() else { return glib::Propagation::Proceed };
            let bar = app.find();
            let primary = if cfg!(target_os = "macos") { gdk::ModifierType::META_MASK } else { gdk::ModifierType::CONTROL_MASK };
            let prim = state.contains(primary);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let alt = state.contains(gdk::ModifierType::ALT_MASK);
            match key {
                gdk::Key::Escape => bar.close(),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter if prim && alt => bar.replace_all(),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter if find => bar.next(shift),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => bar.replace(),
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab | gdk::Key::KP_Tab => bar.swap(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        e.add_controller(keys);
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn app(&self) -> Option<AppRc> {
        self.app.upgrade()
    }

    // -- showing -------------------------------------------------------------------------

    /// Show the bar (with the replace row when `replace`), taking a short selection as the query.
    pub fn open(&self, replace: bool) {
        let seed = self.seed();
        self.show();
        if replace {
            self.set_replace(true);
        }
        if let Some(seed) = seed {
            if seed != self.find_e.text().as_str() {
                self.find_e.set_text(&seed);
            }
        }
        let e = if replace && !self.find_e.text().is_empty() { &self.repl_e } else { &self.find_e };
        e.grab_focus();
        e.select_region(0, -1);
    }

    fn show(&self) {
        let was = std::mem::replace(&mut self.st.borrow_mut().visible, true);
        if !was {
            self.root.set_visible(true);
        }
        self.search(false);
    }

    pub fn close(&self) {
        let Some(app) = self.app() else { return };
        let was = std::mem::replace(&mut self.st.borrow_mut().visible, false);
        if was {
            self.root.set_visible(false);
            app.buf.remove_tag(&app.tags.found, &app.buf.start_iter(), &app.buf.end_iter());
            self.st.borrow_mut().painted = None;
        }
        workers::cancel("find");
        app.text.grab_focus();
        app.text.scroll_mark_onscreen(&app.buf.get_insert());
    }

    pub fn visible(&self) -> bool {
        self.st.borrow().visible
    }

    fn set_replace(&self, on: bool) {
        self.st.borrow_mut().replacing = on;
        self.repl_e.set_visible(on);
        self.repl_btns.set_visible(on);
        widgets::set_icon(&self.chev, if on { "chev-down" } else { "chev-right" });
        self.chev.set_tooltip_text(Some(&if on { "Hide replace".to_string() } else { format!("Replace ({KEY_REPLACE})") }));
    }

    fn toggle_replace(&self) {
        let on = !self.st.borrow().replacing;
        self.set_replace(on);
        if on { self.repl_e.grab_focus() } else { self.find_e.grab_focus() };
    }

    /// Tab moves between the two fields.
    fn swap(&self) {
        if !self.st.borrow().replacing {
            return;
        }
        let in_find = self.find_e.has_focus() || self.find_e.focus_child().is_some();
        if in_find { self.repl_e.grab_focus() } else { self.find_e.grab_focus() };
    }

    /// Line the find field up with the editor's text column, which starts `pad` pixels into the editor.
    pub fn align(&self, pad: i32) {
        let chev = match self.chev.preferred_size().1.width() {
            w if w > 0 => w,
            _ => TOOL + 10,
        };
        // the entry's border (1) and its inner padding (6) come before its text
        let lead = 6.max(pad - chev - 2 - 1 - 6);
        if lead != self.st.borrow().lead {
            self.st.borrow_mut().lead = lead;
            self.body.set_margin_start(lead);
        }
    }

    /// The text selected in the editor, to look for; None keeps the last search.
    fn seed(&self) -> Option<String> {
        let app = self.app()?;
        let (a, b) = app.buf.selection_bounds()?;
        let s = app.buf.text(&a, &b, true).to_string();
        if s.trim().is_empty() || s.contains('\n') || s.chars().count() > 200 {
            return None;
        }
        if self.visible() {
            if self.stale() {
                self.search(false);
            }
            if self.current().is_some() {
                return None; // the selection is a match already: keep the search as it is
            }
        }
        Some(if self.regex.get() { py_escape(&s) } else { s })
    }

    fn option(&self) {
        let Some(app) = self.app() else { return };
        {
            let mut s = app.settings_mut();
            s.find_case = self.case.get();
            s.find_word = self.word.get();
            s.find_regex = self.regex.get();
        }
        app.save_settings();
        self.search(true);
    }

    // -- searching -----------------------------------------------------------------------

    fn stale(&self) -> bool {
        let Some(app) = self.app() else { return true };
        let st = self.st.borrow();
        let ast = app.st.borrow();
        match &st.snap {
            Some(s) => !Rc::ptr_eq(s, &ast.snap) || ast.snap_dirty,
            None => true,
        }
    }

    /// Search again (the text changed); `move_` selects the first match from the selection or cursor on.
    pub fn search(&self, move_: bool) {
        let Some(app) = self.app() else { return };
        let q = self.find_e.text().to_string();
        let mut rx = None;
        let mut error = None;
        if !q.is_empty() {
            let mut pat = if self.regex.get() { q.clone() } else { py_escape(&q) };
            if self.word.get() {
                pat = format!(r"(?<!\w)(?:{pat})(?!\w)");
            }
            let flags = if self.case.get() { "(?m)" } else { "(?mi)" };
            match RegexBuilder::new(&format!("{flags}{pat}")).build() {
                Ok(r) => rx = Some(Rc::new(r)),
                Err(e) => error = Some(("Invalid pattern".to_string(), e.to_string())),
            }
        }
        let snap = app.refresh_snapshot();
        let mut found = Vec::new();
        let mut more = false;
        if let Some(r) = &rx {
            for m in r.find_iter(snap.text.as_str()) {
                match m {
                    Ok(m) if m.end() > m.start() => {
                        // an empty match (such as ^) can be neither shown nor replaced
                        found.push((snap.off.char(m.start()), snap.off.char(m.end())));
                        if found.len() >= LIMIT {
                            more = true;
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // the pattern backtracks too much on this text
                        error = Some(("Pattern too complex".to_string(), e.to_string()));
                        found.clear();
                        break;
                    }
                }
            }
        }
        let empty = found.is_empty();
        {
            let mut st = self.st.borrow_mut();
            st.rx = if error.is_some() { None } else { rx };
            st.error = error;
            st.snap = Some(snap);
            st.matches = Rc::new(found);
            st.more = more;
        }
        if move_ {
            if !empty {
                let pos = self.sel().0;
                self.select(self.index_from(pos));
            } else if let Some((a, b)) = app.selection() {
                if self.st.borrow().picked == Some((a, b)) {
                    // the match picked before no longer matches
                    let ins = app.buf.iter_at_mark(&app.buf.get_insert());
                    app.buf.select_range(&ins, &ins);
                }
            }
        }
        self.paint();
        self.show_count();
    }

    /// Highlight the matches near the view: tagging many thousands at once would be slow.
    pub fn paint(&self) {
        let Some(app) = self.app() else { return };
        if self.stale() {
            return; // the highlights move with the text until the next search
        }
        let matches = self.st.borrow().matches.clone();
        let (mut i, mut j) = (0, 0);
        if self.visible() && !matches.is_empty() {
            let r = app.text.visible_rect();
            let a = app.text.line_at_y(r.y()).0.offset() as usize;
            let mut end = app.text.line_at_y(r.y() + r.height()).0;
            end.forward_to_line_end();
            let (a, b) = (a.saturating_sub(MARGIN), end.offset() as usize + MARGIN);
            i = matches.partition_point(|m| m.0 < a);
            j = matches.partition_point(|m| m.0 <= b);
        }
        {
            let mut st = self.st.borrow_mut();
            if let Some((p, pi, pj)) = &st.painted {
                if Rc::ptr_eq(p, &matches) && (*pi, *pj) == (i, j) {
                    return; // scrolled within the part that is highlighted already
                }
            }
            st.painted = Some((matches.clone(), i, j));
        }
        let buf = &app.buf;
        buf.remove_tag(&app.tags.found, &buf.start_iter(), &buf.end_iter());
        for &(s, e) in &matches[i..j] {
            buf.apply_tag(&app.tags.found, &app.iter_at(s), &app.iter_at(e));
        }
    }

    /// The editor's selection, or the cursor, as character offsets.
    fn sel(&self) -> (usize, usize) {
        let Some(app) = self.app() else { return (0, 0) };
        match app.selection() {
            Some(r) => r,
            None => {
                let c = app.cursor_offset();
                (c, c)
            }
        }
    }

    /// The number of the match that is selected in the editor.
    fn current(&self) -> Option<usize> {
        let matches = self.st.borrow().matches.clone();
        if matches.is_empty() {
            return None;
        }
        let (a, b) = self.sel();
        let i = matches.partition_point(|m| m.0 < a);
        (i < matches.len() && matches[i] == (a, b)).then_some(i)
    }

    /// The first match that starts at `pos` or later; after the last one, the first in the text.
    fn index_from(&self, pos: usize) -> usize {
        let st = self.st.borrow();
        let i = st.matches.partition_point(|m| m.0 < pos);
        if i < st.matches.len() { i } else { 0 }
    }

    fn select(&self, i: usize) {
        let Some(app) = self.app() else { return };
        let Some(&(s, e)) = self.st.borrow().matches.get(i) else { return };
        self.st.borrow_mut().picked = Some((s, e));
        let buf = &app.buf;
        buf.select_range(&app.iter_at(e), &app.iter_at(s));
        see(&app.text, &buf.selection_bound());
        self.show_count();
    }

    /// Go to the next (or previous) match, wrapping around.
    pub fn next(&self, back: bool) {
        if self.find_e.text().is_empty() {
            self.open(false);
            return;
        }
        if !self.visible() {
            self.show();
        } else if self.stale() {
            self.search(false);
        }
        let matches = self.st.borrow().matches.clone();
        if matches.is_empty() {
            if let Some(app) = self.app() {
                app.bell();
            }
            return;
        }
        let (a, b) = self.sel();
        if back {
            // the last match that starts before the selection; before the first, the last in the text
            let i = matches.partition_point(|m| m.0 < a);
            self.select(if i > 0 { i - 1 } else { matches.len() - 1 });
        } else {
            self.select(self.index_from(b));
        }
    }

    /// Update "i of N" after the selection moved.
    pub fn show_count(&self) {
        let q = self.find_e.text();
        let (error, n, more) = {
            let st = self.st.borrow();
            (st.error.clone(), st.matches.len(), st.more)
        };
        let mut bad = false;
        let text = if let Some((short, _)) = &error {
            bad = true;
            short.clone()
        } else if q.is_empty() {
            String::new()
        } else if n == 0 {
            bad = true;
            "No results".to_string()
        } else {
            let total = format!("{}{}", thousands(n), if more { "+" } else { "" });
            match self.current() {
                Some(i) => format!("{} of {total}", thousands(i + 1)),
                None => format!("{total} found"),
            }
        };
        self.count.set_label(&text);
        if bad {
            self.count.remove_css_class("muted");
            self.count.add_css_class("danger");
        } else {
            self.count.remove_css_class("danger");
            self.count.add_css_class("muted");
        }
        self.count.set_tooltip_text(error.as_ref().map(|e| e.1.as_str()).filter(|t| !t.is_empty()));
        if bad && !q.is_empty() {
            self.find_e.add_css_class("error");
        } else {
            self.find_e.remove_css_class("error");
        }
    }

    // -- replacing -----------------------------------------------------------------------

    /// What replaces a match: the text as typed, or for a regular expression, with \1 … filled in.
    fn replacement(&self, rx: &Regex, caps: &Captures<str>) -> Result<String, String> {
        let rep = self.repl_e.text().to_string();
        if self.regex.get() { expand(rx, caps, &rep) } else { Ok(rep) }
    }

    fn fail(&self, err: String) {
        self.st.borrow_mut().error = Some(("Invalid replacement".into(), err));
        self.show_count();
        if let Some(app) = self.app() {
            app.bell();
        }
    }

    fn can_edit(&self, app: &App) -> bool {
        if app.busy() {
            app.flash_msg("Wait for the rewrite to finish.", 5000);
            return false;
        }
        true
    }

    /// Replace the selected match and select the next one; with no match selected, go to the next.
    fn replace(&self) {
        let Some(app) = self.app() else { return };
        if !self.can_edit(&app) {
            return;
        }
        if self.stale() {
            self.search(false);
        }
        let Some(i) = self.current() else {
            self.next(false);
            return;
        };
        let (snap, rx, (s, e)) = {
            let st = self.st.borrow();
            (st.snap.clone(), st.rx.clone(), st.matches[i])
        };
        let (Some(snap), Some(rx)) = (snap, rx) else { return };
        let (bs, be) = (snap.off.byte(s), snap.off.byte(e));
        let caps = match rx.captures_from_pos(snap.text.as_str(), bs) {
            Ok(Some(c)) => c,
            _ => {
                self.next(false);
                return;
            }
        };
        let m = caps.get(0).expect("the whole match");
        if m.start() != bs || m.end() != be {
            self.next(false);
            return;
        }
        let new = match self.replacement(&rx, &caps) {
            Ok(n) => n,
            Err(err) => return self.fail(err),
        };
        // replace() analyses the text, which searches again
        if app.replace(s, e, snap.get(s, e), &new) && !self.st.borrow().matches.is_empty() {
            self.select(self.index_from(s + engine::char_len(&new)));
        }
    }

    /// Replace every match, as one step for undo.
    fn replace_all(&self) {
        let Some(app) = self.app() else { return };
        if !self.can_edit(&app) {
            return;
        }
        self.search(false);
        let (snap, rx, empty) = {
            let st = self.st.borrow();
            (st.snap.clone(), st.rx.clone(), st.matches.is_empty())
        };
        let (Some(snap), Some(rx), false) = (snap, rx, empty) else {
            app.bell();
            return;
        };
        let mut edits = Vec::new();
        for caps in rx.captures_iter(snap.text.as_str()) {
            let caps = match caps {
                Ok(c) => c,
                Err(e) => return self.fail(e.to_string()),
            };
            let m = caps.get(0).expect("the whole match");
            if m.end() > m.start() {
                match self.replacement(&rx, &caps) {
                    Ok(new) => edits.push((snap.off.char(m.start()), snap.off.char(m.end()), new)),
                    Err(err) => return self.fail(err),
                }
            }
        }
        self.apply(&app, &snap, &edits);
        app.analyze();
        let n = edits.len();
        app.flash_msg(
            &format!(
                "Replaced {} match{} — {MOD}+Z undoes {}.",
                thousands(n),
                if n != 1 { "es" } else { "" },
                if n != 1 { "them all" } else { "it" }
            ),
            5000,
        );
    }

    /// Make the edits [(start, end, new)], sorted character offsets into `snap`, as one step for undo.
    /// The cursor, the review suggestions and the view are put where they belong afterwards: inside a
    /// replaced match means at its start.
    fn apply(&self, app: &App, snap: &Snap, edits: &[(usize, usize, String)]) {
        let ends: Vec<usize> = edits.iter().map(|e| e.1).collect();
        let mut shift = vec![0isize];
        for (s, e, new) in edits {
            let last = *shift.last().unwrap();
            shift.push(last + engine::char_len(new) as isize - (e - s) as isize);
        }
        let moved = |off: usize| -> usize {
            let i = ends.partition_point(|&e| e <= off); // edits that end at or before off
            if i < edits.len() && edits[i].0 < off {
                return (edits[i].0 as isize + shift[i]) as usize; // inside a replaced match: its start
            }
            (off as isize + shift[i]).max(0) as usize
        };
        let buf = &app.buf;
        let mut marks: Vec<gtk::TextMark> = vec![buf.get_insert()];
        for p in app.st.borrow().proposals.values() {
            marks.push(p.start.clone());
            marks.push(p.end.clone());
        }
        let place: Vec<(gtk::TextMark, usize)> = marks.into_iter().map(|m| {
            let off = buf.iter_at_mark(&m).offset() as usize;
            (m, moved(off))
        }).collect();
        let r = app.text.visible_rect();
        let top = Some(moved(app.text.line_at_y(r.y()).0.offset() as usize));
        // In a plain text the edits are joined into at most PIECES pieces: each buffer edit is a step to redo
        // and undo, and 20,000 of them are slow both ways. In a formatted text each match is replaced on its
        // own, so the formatting of the text between matches stays.
        let joined;
        let edits = if app.rich_active() {
            edits
        } else {
            joined = join_edits(&snap.text, &snap.off, edits);
            &joined[..]
        };
        app.one_step(|| {
            for (s, e, new) in edits.iter().rev() {
                // back to front keeps the earlier positions valid
                let (mut i1, mut i2) = (app.iter_at(*s), app.iter_at(*e));
                buf.delete(&mut i1, &mut i2);
                let mut i1 = app.iter_at(*s);
                buf.insert(&mut i1, new);
            }
        });
        app.st.borrow_mut().snap_dirty = true;
        app.refresh_snapshot();
        for (m, off) in place {
            if m == buf.get_insert() {
                buf.place_cursor(&app.iter_at(off));
            } else {
                buf.move_mark(&m, &app.iter_at(off));
            }
        }
        let props: Vec<(gtk::TextMark, gtk::TextMark)> =
            app.st.borrow().proposals.values().map(|p| (p.start.clone(), p.end.clone())).collect();
        for (s, e) in props {
            let (a, b) = (buf.iter_at_mark(&s), buf.iter_at_mark(&e));
            if a < b {
                buf.apply_tag(&app.tags.prop, &a, &b);
            }
        }
        let at_start = r.y() <= 0; // the view was at the very top: it stays there
        if let (Some(top), false) = (top, at_start) {
            let mark = match buf.mark("find-top") {
                Some(m) => {
                    buf.move_mark(&m, &app.iter_at(top));
                    m
                }
                None => buf.create_mark(Some("find-top"), &app.iter_at(top), true),
            };
            app.text.scroll_to_mark(&mark, 0.0, true, 0.0, 0.0);
        }
    }
}

/// Most buffer edits one Replace all makes in a plain text.
const PIECES: usize = 50;

/// Join the edits [(start, end, new)], sorted character offsets into `text`, into at most PIECES edits that
/// each span a run of them, carrying the text between them over unchanged.
fn join_edits(text: &str, off: &engine::Offsets, edits: &[(usize, usize, String)]) -> Vec<(usize, usize, String)> {
    if edits.is_empty() {
        return vec![];
    }
    let size = edits.len().div_ceil(PIECES);
    edits
        .chunks(size)
        .map(|part| {
            let (first, last) = (part[0].0, part[part.len() - 1].1);
            let mut out = String::new();
            let mut pos = off.byte(first);
            for (s, e, new) in part {
                out.push_str(&text[pos..off.byte(*s)]);
                out.push_str(new);
                pos = off.byte(*e);
            }
            (first, last, out)
        })
        .collect()
}

/// Scroll so that the mark is in view: a little if it is near, to the middle if it is far.
fn see(text: &gtk::TextView, mark: &gtk::TextMark) {
    let it = text.buffer().iter_at_mark(mark);
    let loc = text.iter_location(&it);
    let r = text.visible_rect();
    let near = loc.y() >= r.y() - r.height() && loc.y() + loc.height() <= r.y() + 2 * r.height();
    if near {
        text.scroll_to_mark(mark, 0.0, false, 0.0, 0.0);
    } else {
        text.scroll_to_mark(mark, 0.0, true, 0.0, 0.5);
    }
}

fn entry(hint: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_placeholder_text(Some(hint));
    e.set_width_chars(26);
    e.set_hexpand(true);
    e
}

/// 12345 -> "12,345".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (k, ch) in s.chars().enumerate() {
        if k > 0 && (s.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Python's re.escape: a backslash before every character that has a meaning in a pattern.
fn py_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\u{b}\u{c}".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// A replacement template as Python's re expands it: \1 … \99 and \g<name> put back groups, \n, \t …
/// are characters, \0 and three-digit numbers are octal codes; an unknown letter escape or a group that
/// does not exist is an error.
fn expand(rx: &Regex, caps: &Captures<str>, template: &str) -> Result<String, String> {
    let groups = rx.captures_len();
    let group = |k: usize, out: &mut String| -> Result<(), String> {
        if k >= groups {
            return Err(format!("invalid group reference {k}"));
        }
        if let Some(m) = caps.get(k) {
            out.push_str(m.as_str());
        }
        Ok(())
    };
    let oct = |c: char| ('0'..='7').contains(&c);
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&c) = chars.get(i) else { return Err("bad escape (end of pattern)".into()) };
        i += 1;
        match c {
            'g' => {
                if chars.get(i) != Some(&'<') {
                    return Err("missing <".into());
                }
                let close = chars[i + 1..].iter().position(|&ch| ch == '>').ok_or("missing >, unterminated name")?;
                let name: String = chars[i + 1..i + 1 + close].iter().collect();
                i += close + 2;
                if name.is_empty() {
                    return Err("missing group name".into());
                }
                if name.chars().all(|ch| ch.is_ascii_digit()) {
                    group(name.parse::<usize>().map_err(|_| format!("invalid group reference {name}"))?, &mut out)?;
                } else if name.chars().next().is_some_and(|ch| ch.is_alphabetic() || ch == '_') && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                    let k = rx.capture_names().position(|n| n == Some(name.as_str())).ok_or(format!("unknown group name '{name}'"))?;
                    group(k, &mut out)?;
                } else {
                    return Err(format!("bad character in group name '{name}'"));
                }
            }
            '0' => {
                let mut v = 0u32;
                for _ in 0..2 {
                    match chars.get(i) {
                        Some(&d) if oct(d) => {
                            v = v * 8 + d.to_digit(8).unwrap();
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(char::from_u32(v & 0xff).unwrap_or('\0'));
            }
            '1'..='9' => {
                let mut digits = c.to_string();
                if let Some(&d) = chars.get(i).filter(|d| d.is_ascii_digit()) {
                    digits.push(d);
                    i += 1;
                    if oct(c) && oct(d) {
                        if let Some(&d3) = chars.get(i).filter(|d| oct(**d)) {
                            digits.push(d3);
                            i += 1;
                            let v = u32::from_str_radix(&digits, 8).unwrap();
                            if v > 0o377 {
                                return Err(format!("octal escape value \\{digits} outside of range 0-0o377"));
                            }
                            out.push(char::from_u32(v).unwrap());
                            continue;
                        }
                    }
                }
                group(digits.parse().unwrap(), &mut out)?;
            }
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{b}'),
            '\\' => out.push('\\'),
            c if c.is_ascii_alphabetic() => return Err(format!("bad escape \\{c}")),
            c => {
                out.push('\\');
                out.push(c);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(pat: &str, text: &str, tpl: &str) -> Result<String, String> {
        let rx = Regex::new(pat).unwrap();
        let caps = rx.captures(text).unwrap().unwrap();
        expand(&rx, &caps, tpl)
    }

    #[test]
    fn templates() {
        assert_eq!(rep(r"(\w+) (\w+)", "ab cd", r"\2 \1").unwrap(), "cd ab");
        assert_eq!(rep(r"(?P<x>\w+)", "ab", r"<\g<x>\g<0>>").unwrap(), "<abab>");
        assert_eq!(rep(r"(a)|(b)", "a", r"[\2]").unwrap(), "[]");
        assert_eq!(rep(r"a", "a", r"x\ny\.").unwrap(), "x\ny\\.");
        assert!(rep(r"a", "a", r"\1").is_err());
        assert!(rep(r"a", "a", r"\q").is_err());
        assert_eq!(rep(r"a", "a", r"\101").unwrap(), "A");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn joins_edits() {
        let text = "a cat, a cat — a cat";
        let off = engine::Offsets::new(text);
        let edits: Vec<(usize, usize, String)> =
            text.match_indices("cat").map(|(b, m)| (off.char(b), off.char(b + m.len()), "dog".to_string())).collect();
        let joined = join_edits(text, &off, &edits);
        assert_eq!(joined, edits); // up to PIECES edits stay as they are
        let pairs: Vec<(usize, usize, String)> = (0..PIECES * 2).map(|k| (k, k + 1, "x".into())).collect();
        let text2 = "y".repeat(PIECES * 2);
        let joined = join_edits(&text2, &engine::Offsets::new(&text2), &pairs);
        assert_eq!(joined.len(), PIECES);
        assert_eq!(joined[0], (0, 2, "xx".to_string()));
        let wide = "a cat, a cat — a cat ".repeat(60);
        let off = engine::Offsets::new(&wide);
        let cats: Vec<(usize, usize, String)> =
            wide.match_indices("cat").map(|(b, m)| (off.char(b), off.char(b + m.len()), "dog".to_string())).collect();
        let joined = join_edits(&wide, &off, &cats);
        assert_eq!(joined.len(), 45); // 180 edits in pieces of 4
        let mut out: Vec<char> = wide.chars().collect();
        for (s, e, new) in joined.iter().rev() {
            out.splice(*s..*e, new.chars());
        }
        assert_eq!(out.into_iter().collect::<String>(), wide.replace("cat", "dog"));
        let many: Vec<(usize, usize, String)> = (0..120).map(|k| (k * 2, k * 2 + 1, "x".into())).collect();
        let text = "ab".repeat(120);
        let joined = join_edits(&text, &engine::Offsets::new(&text), &many);
        assert!(joined.len() <= PIECES);
        let mut out = text.clone();
        for (s, e, new) in joined.iter().rev() {
            out.replace_range(*s..*e, new);
        }
        assert_eq!(out, "xb".repeat(120));
    }

    /// Timing of Replace all in a 1 MB plain text with 20,000 matches, one buffer edit per match against
    /// joined pieces, both doing and undoing. Run with `cargo test -p rewrite-app -- --ignored --nocapture
    /// replace_all_timing`.
    #[test]
    #[ignore]
    fn replace_all_timing() {
        use std::time::Instant;
        gtk::init().unwrap();
        let unit = "Plain words fill this line; one target is here. ";
        let mut text = String::new();
        for k in 0.. {
            if text.len() >= 1_000_000 {
                break;
            }
            text.push_str(unit);
            if k % 10 == 9 {
                text.push('\n');
            }
        }
        let off = engine::Offsets::new(&text);
        let edits: Vec<(usize, usize, String)> = text
            .match_indices("target")
            .take(20_000)
            .map(|(b, m)| (off.char(b), off.char(b + m.len()), "aim".to_string()))
            .collect();
        println!("{} bytes, {} matches", text.len(), edits.len());
        for joined in [false, true] {
            let buf = gtk::TextBuffer::new(None);
            buf.set_enable_undo(false);
            buf.set_text(&text);
            buf.set_enable_undo(true);
            let t0 = Instant::now();
            let pieces = if joined { join_edits(&text, &off, &edits) } else { edits.clone() };
            buf.begin_user_action();
            for (s, e, new) in pieces.iter().rev() {
                let (mut i1, mut i2) = (buf.iter_at_offset(*s as i32), buf.iter_at_offset(*e as i32));
                buf.delete(&mut i1, &mut i2);
                let mut i1 = buf.iter_at_offset(*s as i32);
                buf.insert(&mut i1, new);
            }
            buf.end_user_action();
            let done = t0.elapsed();
            let t1 = Instant::now();
            buf.undo();
            let undone = t1.elapsed();
            let (a, b) = buf.bounds();
            assert_eq!(buf.text(&a, &b, false).as_str(), text);
            println!("{}: {} buffer edits, replace {:?}, undo {:?}", if joined { "joined" } else { "per match" },
                pieces.len() * 2, done, undone);
        }
    }

    #[test]
    fn escapes_compile() {
        let s = "a b-c#d&e~f (g) [h] {i} ?*+|^$.\\";
        let rx = Regex::new(&format!("(?m){}", py_escape(s))).unwrap();
        assert_eq!(rx.find(s).unwrap().unwrap().as_str(), s);
        let rx = Regex::new(r"(?mi)(?<!\w)(?:cat)(?!\w)").unwrap();
        let found: Vec<_> = rx.find_iter("concat Cat cats cat").map(|m| m.unwrap().start()).collect();
        assert_eq!(found, vec![7, 16]);
    }
}
