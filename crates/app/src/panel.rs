//! Side panel: alternatives for the current word or phrase, proofreading remarks, the review of
//! suggested changes, the original text and sentence rewrites.

use crate::app::{App, AppRc};
use crate::engine::Suggestion;
use crate::theme::MOD;
use crate::widgets;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

const PAD: i32 = 10; // margin left and right of the content
const WIDE: i32 = 480; // from this width on (the panel below the editor) there is room for two columns

fn tips() -> Vec<(String, &'static str)> {
    vec![
        ("Click a word".into(), "to see alternatives that fit its context."),
        ("Select a phrase".into(), "(2+ words) for phrase-level rewrites."),
        (format!("{MOD}+E"), "rephrases the whole sentence at the cursor."),
        ("Alt+1 … Alt+9".into(), "applies a suggestion without the mouse."),
        ("Auto-rewrite".into(), "does a pass over the selection or the whole document."),
        ("Red / blue underlines".into(), "mark spelling and grammar slips; click one to fix it."),
        ("Preview".into(), "shows Markdown, the LaTeX PDF or a Word document as it prints."),
    ]
}

/// What the panel shows for the target.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TargetView {
    pub text: String,
    /// word | phrase | spell
    pub kind: String,
    pub loading: bool,
    pub protected: bool,
    pub error: Option<String>,
    /// (group kind, heading, suggestions, index of the first one)
    pub groups: Vec<(String, String, Vec<SuggestionView>, usize)>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SuggestionView {
    pub text: String,
    pub kind: String,
    pub fit: f64,
    pub note: String,
}

impl TargetView {
    pub fn protected(text: &str, kind: &str) -> TargetView {
        TargetView { text: text.into(), kind: kind.into(), protected: true, ..Default::default() }
    }
    pub fn loading(text: &str, kind: &str) -> TargetView {
        TargetView { text: text.into(), kind: kind.into(), loading: true, ..Default::default() }
    }
    pub fn groups(text: &str, kind: &str, groups: Vec<(String, String, Vec<Suggestion>, usize)>, error: Option<String>) -> TargetView {
        let groups = groups
            .into_iter()
            .map(|(k, h, items, off)| {
                (k, h, items.into_iter().map(|s| SuggestionView { text: s.text, kind: s.kind, fit: s.fit, note: s.note }).collect(), off)
            })
            .collect();
        TargetView { text: text.into(), kind: kind.into(), error, groups, ..Default::default() }
    }
}

#[derive(Default, Clone, PartialEq)]
struct PanelState {
    target: Option<TargetView>,
    /// idle | loading | results | empty | nomodel
    sentence: String,
    sentences: Vec<(String, String)>,
    orig: Option<String>,
    /// (pending, current (old, new, position))
    review: Option<(usize, Option<(String, String, usize)>)>,
    /// (text, message, fixes)
    issue: Option<(String, String, Vec<String>)>,
}

pub struct Panel {
    app: Weak<App>,
    scroll: gtk::ScrolledWindow,
    inner: gtk::Box,
    state: RefCell<PanelState>,
    wide: Cell<bool>,
    anim: RefCell<Option<glib::SourceId>>,
    sentence_head: RefCell<Option<gtk::Widget>>,
}

impl Panel {
    pub fn new(app: &AppRc) -> Panel {
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.add_css_class("panel");
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        inner.add_css_class("panel");
        scroll.set_child(Some(&inner));
        let p = Panel {
            app: app.weak(),
            scroll,
            inner,
            state: RefCell::new(PanelState { sentence: "idle".into(), ..Default::default() }),
            wide: Cell::new(false),
            anim: RefCell::new(None),
            sentence_head: RefCell::new(None),
        };
        let me = app.weak();
        p.scroll.hadjustment().connect_page_size_notify(move |adj| {
            let Some(app) = me.upgrade() else { return };
            if let Some(panel) = app.panel.get() {
                if (adj.page_size() as i32 >= WIDE) != panel.wide.get() {
                    panel.render();
                }
            }
        });
        p
    }

    pub fn widget(&self) -> gtk::Widget {
        self.scroll.clone().upcast()
    }

    fn app(&self) -> Option<AppRc> {
        self.app.upgrade()
    }

    /// A new document: start empty.
    pub fn reset(&self) {
        *self.state.borrow_mut() = PanelState { sentence: "idle".into(), ..Default::default() };
        self.render();
    }

    /// The panel moved (beside the editor or below it).
    pub fn relayout(&self) {
        let me = self.app.clone();
        glib::idle_add_local_once(move || {
            if let Some(app) = me.upgrade() {
                app.panel().render();
            }
        });
    }

    pub fn set_target(&self, target: Option<TargetView>) {
        self.state.borrow_mut().target = target;
        self.render();
    }

    pub fn set_review(&self, review: Option<(usize, Option<(String, String, usize)>)>) {
        if self.state.borrow().review != review {
            self.state.borrow_mut().review = review;
            self.render();
        }
    }

    pub fn set_issue(&self, issue: Option<(String, String, Vec<String>)>) {
        if self.state.borrow().issue != issue {
            self.state.borrow_mut().issue = issue;
            self.render();
        }
    }

    pub fn set_original(&self, orig: Option<String>) {
        if self.state.borrow().orig != orig {
            self.state.borrow_mut().orig = orig;
            self.render();
        }
    }

    pub fn set_sentence(&self, status: &str, items: Vec<Suggestion>) {
        {
            let mut st = self.state.borrow_mut();
            st.sentence = status.into();
            st.sentences = items.into_iter().map(|s| (s.text, s.note)).collect();
        }
        self.render();
        if status != "idle" {
            self.show_sentence_section();
        }
    }

    // -- rendering --------------------------------------------------------------------------

    pub fn render(&self) {
        let Some(app) = self.app() else { return };
        widgets::clear(&self.inner);
        if let Some(id) = self.anim.borrow_mut().take() {
            id.remove();
        }
        // one column beside the editor; below it, the sentence rewrites get a column of their own
        let wide = self.scroll.hadjustment().page_size() as i32 >= WIDE;
        self.wide.set(wide);
        let mk = || {
            let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
            c.set_hexpand(true);
            c.set_valign(gtk::Align::Start);
            c
        };
        let first = mk();
        self.inner.append(&first);
        let second = if wide {
            let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
            line.add_css_class("vline");
            self.inner.append(&line);
            let s = mk();
            self.inner.append(&s);
            self.inner.set_homogeneous(false);
            s
        } else {
            first.clone()
        };
        let st = self.state.borrow().clone();
        if let Some(rv) = &st.review {
            self.render_review(&app, &first, rv);
        }
        if let Some(issue) = &st.issue {
            self.render_issue(&app, &first, issue);
        }
        match &st.target {
            None => {
                if st.review.is_none() && st.issue.is_none() {
                    self.render_tips(&app, &first);
                }
            }
            Some(t) => self.render_target(&app, &first, t),
        }
        if let Some(orig) = &st.orig {
            self.render_original(&app, &second, orig);
        }
        self.render_sentence(&app, &second, &st);
        for col in [&first, &second] {
            let pad = gtk::Box::new(gtk::Orientation::Vertical, 0);
            pad.set_size_request(-1, 10);
            col.append(&pad);
        }
        self.scroll.vadjustment().set_value(0.0);
    }

    fn show_sentence_section(&self) {
        let me = self.app.clone();
        glib::idle_add_local_once(move || {
            let Some(app) = me.upgrade() else { return };
            let p = app.panel();
            let Some(head) = p.sentence_head.borrow().clone() else { return };
            if let Some(pt) = head.compute_point(&p.inner, &gtk::graphene::Point::new(0.0, 0.0)) {
                let adj = p.scroll.vadjustment();
                if adj.upper() > adj.page_size() {
                    adj.set_value((pt.y() as f64 - 10.0).max(0.0));
                }
            }
        });
    }

    fn text(&self, col: &gtk::Box, text: &str, classes: &[&str], top: i32) -> gtk::Label {
        let l = widgets::wrap_label(text, classes);
        l.set_margin_start(PAD);
        l.set_margin_end(PAD);
        l.set_margin_top(top);
        col.append(&l);
        l
    }

    /// A small heading; returns the box its icon tools go in, at the right.
    fn caption(&self, col: &gtk::Box, text: &str, top: i32) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_margin_start(PAD);
        row.set_margin_end(PAD - 4);
        row.set_margin_top(top);
        row.set_margin_bottom(2);
        let l = widgets::label(&text.to_uppercase(), &["caption"]);
        l.set_hexpand(true);
        l.set_valign(gtk::Align::Center);
        row.append(&l);
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.append(&tools);
        col.append(&row);
        tools
    }

    /// Start a new part of the panel: a hairline (except at the top) and its heading.
    fn section(&self, col: &gtk::Box, text: &str) -> gtk::Box {
        if col.first_child().is_some() {
            let d = widgets::hairline();
            d.set_margin_top(8);
            col.append(&d);
        }
        self.caption(col, text, 6)
    }

    fn tool(&self, tools: &gtk::Box, icon: &str, tip: &str, kind: &str, gap: i32, f: impl Fn(&AppRc) + 'static) {
        let me = self.app.clone();
        let b = widgets::sized_icon_button(icon, 14, tip, move || {
            if let Some(app) = me.upgrade() {
                f(&app)
            }
        });
        b.add_css_class("compact");
        if kind != "ghost" {
            b.add_css_class(kind);
        }
        b.set_margin_start(gap);
        tools.append(&b);
    }

    /// A card for a replacement: the old text struck out, the new text in bold, a remark below.
    fn change(&self, col: &gtk::Box, old: &str, new: Option<&str>, edge: &str, note: Option<&str>) {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 2);
        card.add_css_class("card");
        card.add_css_class(&format!("{edge}-edge"));
        card.set_margin_start(PAD);
        card.set_margin_end(PAD);
        card.set_margin_top(2);
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_column_spacing(6);
        flow.set_max_children_per_line(3);
        flow.set_min_children_per_line(1);
        match new {
            None => flow.append(&widgets::wrap_label(old, &["item", "bold"])),
            Some(n) => {
                // short: old → new on one line; long: stacked
                let short = old.chars().count() + n.chars().count() < 40;
                flow.set_max_children_per_line(if short { 3 } else { 1 });
                flow.append(&widgets::wrap_label(old, &["strike", "muted"]));
                flow.append(&widgets::label(if short { "→" } else { "↓" }, &["small", "faint"]));
                flow.append(&widgets::wrap_label(n, &["item", "bold"]));
            }
        }
        card.append(&flow);
        if let Some(n) = note {
            card.append(&widgets::wrap_label(n, &["small", "muted"]));
        }
        col.append(&card);
    }

    fn render_review(&self, _app: &AppRc, col: &gtk::Box, rv: &(usize, Option<(String, String, usize)>)) {
        let (n, cur) = rv;
        let head = self.section(col, &match cur {
            Some((_, _, pos)) => format!("Change {pos} of {n}"),
            None => "Review".into(),
        });
        if cur.is_some() {
            self.tool(&head, "check", "Accept this change (Alt+Y)", "primary", 0, |a| a.accept_prop());
            self.tool(&head, "close", "Reject this change (Alt+N)", "ghost", 2, |a| a.reject_prop());
            self.tool(&head, "next", "Next change (F8)", "ghost", 0, |a| a.next_prop(None, false));
        } else {
            self.tool(&head, "next", "Go to the first change (F8)", "ghost", 0, |a| a.next_prop(Some(0), true));
        }
        self.tool(&head, "check-all", &format!("Accept all {n} changes"), "ghost", 8, |a| a.accept_all_props());
        self.tool(&head, "trash", "Reject all changes", "ghost", 0, |a| a.reject_all_props());
        match cur {
            Some((old, new, _)) => {
                let o = if old.trim().is_empty() { "(nothing)" } else { old.trim() };
                let nw = if new.trim().is_empty() { "(remove this)" } else { new.trim() };
                self.change(col, o, Some(nw), "accent", None);
            }
            None => {
                self.text(col, &format!("{n} suggested change{} highlighted in blue. Click one to accept or reject it.", if *n != 1 { "s" } else { "" }), &[], 2);
            }
        }
    }

    fn render_issue(&self, _app: &AppRc, col: &gtk::Box, issue: &(String, String, Vec<String>)) {
        let (text, message, fixes) = issue;
        let head = self.section(col, "Grammar");
        if fixes.len() == 1 {
            self.tool(&head, "check", "Fix it", "primary", 0, |a| a.fix_issue(0));
        }
        self.tool(&head, "eye-off", "Ignore this remark", "ghost", 2, |a| a.ignore_issue());
        let t = if text.trim().is_empty() { "(space)" } else { text.trim() };
        let first = fixes.first().map(|f| if f.trim().is_empty() { "(remove)".to_string() } else { f.trim().to_string() });
        self.change(col, t, first.as_deref(), "grammar", Some(message));
        if fixes.len() > 1 {
            // several ways to fix it: one button each
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            row.set_margin_start(PAD);
            row.set_margin_end(PAD);
            row.set_margin_top(5);
            for (n, f) in fixes.iter().take(3).enumerate() {
                let label = if f.trim().is_empty() { "(remove)" } else { f.trim() };
                let me = self.app.clone();
                let b = widgets::flat_button(label, if n == 0 { "primary" } else { "soft" }, None, move || {
                    if let Some(app) = me.upgrade() {
                        app.fix_issue(n)
                    }
                });
                b.add_css_class("compact");
                row.append(&b);
            }
            col.append(&row);
        }
    }

    fn render_tips(&self, app: &AppRc, col: &gtk::Box) {
        self.section(col, "Get started");
        for (head, rest) in tips() {
            self.text(col, &format!("{head} — {rest}"), &[], 1);
        }
        if !app.engine.smart() && app.st.borrow().models_state != "loading" {
            self.section(col, "Lite mode");
            self.text(
                col,
                "Suggestions come from the offline thesaurus only. Download the small context models for suggestions that understand the sentence.",
                &["small", "muted"],
                2,
            );
            let me = self.app.clone();
            let b = widgets::flat_button("Get smart models…", "soft", None, move || {
                if let Some(app) = me.upgrade() {
                    app.open_models()
                }
            });
            b.set_halign(gtk::Align::Start);
            b.set_margin_start(PAD);
            b.set_margin_top(6);
            col.append(&b);
        }
    }

    fn render_target(&self, app: &AppRc, col: &gtk::Box, t: &TargetView) {
        let spelling = t.kind == "spell" && !t.protected && !t.loading;
        let heading = match t.kind.as_str() {
            "phrase" => "Alternatives for this phrase",
            "spell" => "Spelling",
            _ => "Alternatives for",
        };
        let head = self.section(col, heading);
        if spelling {
            self.tool(&head, "book-plus", "Add to dictionary: never mark this word again", "ghost", 0, |a| a.add_word(false));
            self.tool(&head, "eye-off", "Ignore: leave it alone until Rewrite is closed", "ghost", 0, |a| a.add_word(true));
        }
        let text = if t.text.chars().count() < 160 { t.text.clone() } else { format!("{}…", t.text.chars().take(157).collect::<String>()) };
        let classes: &[&str] = match t.kind.as_str() {
            "phrase" => &["item", "bold"],
            "spell" => &["head", "danger"],
            _ => &["head"],
        };
        self.text(col, &text, classes, 0);
        if spelling {
            let items = t.groups.first().map(|g| g.2.clone()).unwrap_or_default();
            let msg = if items.is_empty() { "Not in the dictionary, and nothing similar was found." } else { "Not in the dictionary. Did you mean:" };
            match &t.error {
                Some(e) => self.text(col, e, &["small", "danger"], 2),
                None => self.text(col, msg, &["small", "muted"], 2),
            };
            if !items.is_empty() {
                self.rows(col, &items, 0, Some(3));
            }
        } else if t.protected {
            self.text(col, "This part is markup (LaTeX command, maths, code or a link), so it is left untouched.", &["small", "muted"], 2);
        } else if t.loading {
            let l = self.text(col, "Thinking", &["small", "muted"], 4);
            self.animate(&l, "Thinking");
        } else if let Some(e) = &t.error {
            self.text(col, e, &["small", "danger"], 2);
        } else {
            let groups = if t.groups.is_empty() { vec![("word".to_string(), t.text.clone(), vec![], 0)] } else { t.groups.clone() };
            let single = groups.len() == 1;
            for (g, (_, heading, items, offset)) in groups.iter().enumerate() {
                if g > 0 {
                    self.caption(col, "Or reword the whole phrase", 10);
                    self.text(col, heading, &["item", "bold"], 0);
                }
                if items.is_empty() {
                    let msg = if single {
                        "No good alternatives found here. Try selecting a longer phrase, or rephrase the whole sentence."
                    } else {
                        "No strong single-word alternatives here."
                    };
                    self.text(col, msg, &["small", "muted"], 2);
                } else {
                    self.rows(col, items, *offset, None);
                }
            }
        }
        let _ = app;
    }

    /// The list of alternatives. Each sense of the word is described once (`notes`: only on that many rows).
    fn rows(&self, col: &gtk::Box, items: &[SuggestionView], offset: usize, notes: Option<usize>) {
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bx.set_margin_top(3);
        let mut last_note: Option<String> = None;
        for (i, s) in items.iter().enumerate() {
            let note = match notes {
                None if Some(&s.note) != last_note.as_ref() => s.note.clone(),
                Some(k) if i < k => s.note.clone(),
                _ => String::new(),
            };
            if !s.note.is_empty() {
                last_note = Some(s.note.clone());
            }
            self.row(&bx, offset + i, s, &note);
        }
        col.append(&bx);
    }

    fn row(&self, parent: &gtk::Box, i: usize, s: &SuggestionView, note: &str) {
        let Some(app) = self.app() else { return };
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("row");
        row.set_cursor_from_name(Some("pointer"));
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        top.set_margin_start(PAD);
        top.set_margin_end(PAD);
        top.set_margin_top(3);
        top.set_margin_bottom(if note.is_empty() { 3 } else { 0 });
        let num = widgets::label(&if i < 9 { (i + 1).to_string() } else { String::new() }, &["small", "faint"]);
        num.set_width_chars(2);
        top.append(&num);
        let word = widgets::wrap_label(&s.text, &["item"]);
        word.set_hexpand(true);
        top.append(&word);
        top.append(&widgets::fit_bar(&app.theme(), s.fit));
        let badge = widgets::label(&s.kind, &["badge", &format!("badge-{}", s.kind.replace(' ', "-"))]);
        badge.set_valign(gtk::Align::Center);
        top.append(&badge);
        row.append(&top);
        if !note.is_empty() {
            let l = widgets::label(note, &["small", "muted"]);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_tooltip_text(Some(note));
            l.set_margin_start(PAD + 16);
            l.set_margin_end(PAD);
            l.set_margin_bottom(3);
            row.append(&l);
        }
        let click = gtk::GestureClick::new();
        let me = self.app.clone();
        click.connect_released(move |_, _, _, _| {
            if let Some(app) = me.upgrade() {
                app.apply_suggestion(i);
            }
        });
        row.add_controller(click);
        parent.append(&row);
    }

    fn render_original(&self, _app: &AppRc, col: &gtk::Box, orig: &str) {
        let head = self.section(col, "Original text here");
        self.tool(&head, "revert", "Revert to the original", "ghost", 0, |a| a.revert_here());
        let txt = if orig.trim().is_empty() { "(nothing — this text was added)".to_string() } else { orig.to_string() };
        let txt = if txt.chars().count() <= 300 { txt } else { format!("{}…", txt.chars().take(297).collect::<String>()) };
        self.text(col, &txt, &["muted"], 0);
    }

    fn render_sentence(&self, _app: &AppRc, col: &gtk::Box, st: &PanelState) {
        let status = st.sentence.as_str();
        let head = self.section(col, "Whole sentence");
        *self.sentence_head.borrow_mut() = head.parent();
        if matches!(status, "results" | "empty") {
            self.tool(&head, "refresh", "More variations", "ghost", 0, |a| a.rephrase_sentence(true));
        }
        self.tool(&head, "pencil", &format!("Rephrase the sentence at the cursor ({MOD}+E)"), "ghost", 0, |a| a.rephrase_sentence(false));
        if status == "loading" {
            let l = self.text(col, "Rewriting", &["small", "muted"], 0);
            self.animate(&l, "Rewriting");
            return;
        }
        if status == "empty" {
            self.text(col, "No rewrite kept the meaning well enough. Press ↻ for other variations.", &["small", "muted"], 0);
        }
        if status == "nomodel" {
            self.text(
                col,
                "Sentence rewriting works best with the paraphrase model (Models menu). Showing thesaurus-based rewrites instead.",
                &["small", "muted"],
                0,
            );
        }
        for (i, (text, note)) in st.sentences.iter().enumerate() {
            let card = gtk::Box::new(gtk::Orientation::Vertical, 1);
            card.add_css_class("card");
            card.add_css_class("clickable");
            card.set_cursor_from_name(Some("pointer"));
            card.set_margin_start(PAD);
            card.set_margin_end(PAD);
            card.set_margin_top(if i == 0 { 2 } else { 5 });
            card.append(&widgets::wrap_label(text.trim(), &[]));
            card.append(&widgets::label(note, &["small", "muted"]));
            let click = gtk::GestureClick::new();
            let me = self.app.clone();
            click.connect_released(move |_, _, _, _| {
                if let Some(app) = me.upgrade() {
                    app.apply_sentence(i);
                }
            });
            card.add_controller(click);
            col.append(&card);
        }
    }

    fn animate(&self, lbl: &gtk::Label, base: &'static str) {
        let lbl = lbl.clone();
        let dots = Rc::new(Cell::new(0));
        let id = glib::timeout_add_local(std::time::Duration::from_millis(350), move || {
            dots.set((dots.get() + 1) % 4);
            lbl.set_label(&format!("{base}{}", ".".repeat(dots.get())));
            glib::ControlFlow::Continue
        });
        *self.anim.borrow_mut() = Some(id);
    }
}
