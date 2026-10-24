//! Dialogs: smart models (and the first-run welcome), auto-rewrite and the help.

use crate::app::{App, AppRc, AUTHOR, URL, VERSION};
use crate::engine::AutoOptions;
use crate::theme::{KEYS_REPLACE, KEY_NEXT, KEY_PREV, MOD, UI_SIZE};
use crate::widgets;
use crate::workers;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use rewrite_nlp::download;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// A modal window over the main one: a padded body, Esc closes it, Enter runs its main action.
pub(crate) struct Dialog {
    pub win: gtk::Window,
    pub body: gtk::Box,
}

impl Dialog {
    /// `width` is the width the text wraps at.
    pub fn new(app: &App, title: &str, width: i32) -> Dialog {
        let win = gtk::Window::builder().title(title).modal(true).transient_for(&app.win).resizable(false).destroy_with_parent(true).build();
        win.add_css_class("dialog");
        win.set_icon_name(Some("rewrite"));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("dialog");
        body.set_margin_start(18);
        body.set_margin_end(18);
        body.set_margin_top(14);
        body.set_margin_bottom(14);
        body.set_size_request(width, -1);
        win.set_child(Some(&body));
        Dialog { win, body }
    }

    /// Esc runs `escape` (and the window's close button does), Enter runs `enter`.
    pub fn keys(&self, escape: impl Fn() + 'static, enter: impl Fn() + 'static) {
        let escape = Rc::new(escape);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let esc = escape.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::META_MASK);
            match key {
                gdk::Key::Escape => esc(),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter if plain => enter(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.win.add_controller(keys);
        self.win.connect_close_request(move |_| {
            escape();
            glib::Propagation::Stop
        });
    }

    /// A label as wide as the dialog that wraps.
    pub fn label(&self, parent: &gtk::Box, text: &str, classes: &[&str]) -> gtk::Label {
        let l = widgets::wrap_label(text, classes);
        l.set_max_width_chars(20); // the dialog's width decides where the lines break
        l.set_hexpand(true);
        parent.append(&l);
        l
    }

    /// A small heading in capitals ("APPLY TO").
    pub fn caption(&self, parent: &gtk::Box, text: &str) -> gtk::Label {
        let l = widgets::label(text, &["caption"]);
        parent.append(&l);
        l
    }

    /// The row of buttons at the bottom: (left part, right part).
    pub fn buttons(&self) -> (gtk::Box, gtk::Box) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_top(14);
        let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let right = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        left.set_hexpand(true);
        row.append(&left);
        row.append(&right);
        self.body.append(&row);
        (left, right)
    }

    pub fn show(&self) {
        self.win.present();
    }

    pub fn close(&self) {
        self.win.destroy();
    }
}

thread_local! {
    static CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

fn css_text(app: &App) -> String {
    let t = app.theme();
    format!(
        ".segment button {{ background-image: none; box-shadow: none; text-shadow: none; min-height: 0; }}\n\
         .segment button:hover {{ background-color: {hover}; }}\n\
         .segment button.on, .segment button.on:hover {{ background-color: {accent}; color: {on}; }}\n",
        hover = t.c("hover"),
        accent = t.c("accent"),
        on = t.c("on_accent"),
    )
}

/// The dialogs' own style rules (the segmented strength buttons), made again after a theme switch.
fn ensure_css(app: &App) {
    if CSS.with(|c| c.borrow().is_some()) {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css_text(app));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    }
    let me = app.weak();
    let p = provider.clone();
    app.theme().on_change(move || {
        if let Some(app) = me.upgrade() {
            p.load_from_string(&css_text(&app));
        }
    });
    CSS.with(|c| *c.borrow_mut() = Some(provider));
}

fn margins(w: &impl IsA<gtk::Widget>, top: i32, bottom: i32) {
    w.set_margin_top(top);
    w.set_margin_bottom(bottom);
}

// -- smart models -------------------------------------------------------------------------------

struct ModelsDialog {
    id: u64,
    app: Weak<App>,
    d: Dialog,
    checks: Vec<(&'static str, gtk::CheckButton)>,
    status: gtk::Label,
    bar: gtk::ProgressBar,
    go: gtk::Button,
    later: gtk::Button,
    cancel: RefCell<Arc<AtomicBool>>,
    running: Cell<bool>,
    closed: Cell<bool>,
}

thread_local! {
    /// The models dialog that is open, for the messages of its download thread.
    static MODELS: RefCell<Weak<ModelsDialog>> = RefCell::new(Weak::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Run `f` with the models dialog number `id`, if it is still open (on the UI thread).
fn with_models(id: u64, f: impl FnOnce(&Rc<ModelsDialog>)) {
    if let Some(d) = MODELS.with(|m| m.borrow().upgrade()) {
        if d.id == id && !d.closed.get() {
            f(&d);
        }
    }
}

/// The models dialog; `welcome` on the first start.
pub fn models_dialog(app: &AppRc, welcome: bool) {
    let title = if welcome { "Welcome to Rewrite" } else { "Smart models" };
    let d = Dialog::new(app, title, 480);
    let body = d.body.clone();
    d.label(&body, title, &["title"]);
    let intro = d.label(
        &body,
        "Rewrite already works offline with its built-in thesaurus and phrase book. For suggestions that actually understand the \
         sentence, it can use three small models (22M to 220M parameters) that run on your CPU.",
        &["muted"],
    );
    margins(&intro, 4, 8);
    let runtime = download::runtime_available();
    let mut checks = vec![];
    for spec in download::MODELS.iter() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        margins(&row, 3, 3);
        let have = download::present(spec.key);
        let cb = gtk::CheckButton::new();
        cb.set_active(!have);
        cb.set_valign(gtk::Align::Start);
        cb.set_sensitive(!(have || !runtime));
        row.append(&cb);
        let txt = gtk::Box::new(gtk::Orientation::Vertical, 0);
        txt.set_hexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.append(&widgets::label(spec.title, &["bold"]));
        let status = if have { "  ✓ installed".to_string() } else { format!("  {} MB", download::size_mb(spec.key)) };
        let st = widgets::label(&status, &["small", if have { "good" } else { "faint" }]);
        st.set_valign(gtk::Align::Center);
        head.append(&st);
        txt.append(&head);
        d.label(&txt, spec.detail, &["small", "muted"]);
        row.append(&txt);
        body.append(&row);
        checks.push((spec.key, cb));
    }
    if !runtime {
        let l = d.label(
            &body,
            "The model runtime (ONNX Runtime) could not be loaded: its library is missing from the folder Rewrite is installed in. \
             Reinstall Rewrite to put it back.",
            &["danger"],
        );
        margins(&l, 8, 0);
    }
    let status = d.label(&body, "", &["small", "muted"]);
    margins(&status, 8, 3);
    let bar = gtk::ProgressBar::new();
    bar.set_visible(false);
    body.append(&bar);
    let (_, right) = d.buttons();
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let call = move |f: fn(&Rc<ModelsDialog>)| move || with_models(id, f);
    let later = widgets::flat_button(if welcome { "Not now" } else { "Close" }, "ghost", None, call(|d| d.close()));
    let go = widgets::flat_button("Download", "primary", None, call(|d| d.start()));
    right.append(&later);
    right.append(&go);
    let dlg = Rc::new(ModelsDialog {
        id,
        app: app.weak(),
        d,
        checks,
        status,
        bar,
        go,
        later,
        cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        running: Cell::new(false),
        closed: Cell::new(false),
    });
    for (_, cb) in &dlg.checks {
        cb.connect_toggled(move |_| with_models(id, |d| d.update_total()));
    }
    dlg.d.keys(call(|d| d.close()), call(|d| {
        if d.go.is_visible() && d.go.is_sensitive() {
            d.start();
        }
    }));
    MODELS.with(|m| *m.borrow_mut() = Rc::downgrade(&dlg));
    dlg.update_total();
    if !runtime {
        dlg.go.set_sensitive(false);
    }
    dlg.d.show();
    // keep the dialog alive while its window is
    let keep = dlg.clone();
    dlg.d.win.connect_destroy(move |_| keep.closed.set(true));
}

impl ModelsDialog {
    fn wanted(&self) -> Vec<&'static str> {
        self.checks.iter().filter(|(k, cb)| cb.is_active() && !download::present(k)).map(|(k, _)| *k).collect()
    }

    fn update_total(&self) {
        let total: u64 = self.wanted().iter().map(|k| download::size_mb(k)).sum();
        self.go.set_label(&if total > 0 { format!("Download  ({total} MB)") } else { "Download".into() });
        self.go.set_sensitive(total > 0 && download::runtime_available());
    }

    fn start(&self) {
        let keys = self.wanted();
        if keys.is_empty() || self.running.get() {
            return;
        }
        self.running.set(true);
        self.go.set_sensitive(false);
        self.later.set_label("Cancel");
        self.status.remove_css_class("danger");
        self.status.add_css_class("muted");
        self.bar.set_visible(true);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = cancel.clone();
        let id = self.id;
        std::thread::Builder::new()
            .name("download".into())
            .spawn(move || download_all(id, keys, cancel))
            .expect("a download thread");
    }

    fn progress(&self, text: &str, frac: f64) {
        self.status.set_label(text);
        self.bar.set_fraction(frac.clamp(0.0, 1.0));
    }

    fn failed(&self, e: &str) {
        self.running.set(false);
        self.status.set_label(&format!("Download failed: {e}\nCheck your internet connection and try again (partial downloads resume)."));
        self.status.remove_css_class("muted");
        self.status.add_css_class("danger");
        self.go.set_sensitive(true);
        self.go.set_label("Retry");
        self.later.set_label("Close");
    }

    fn done(&self) {
        self.running.set(false);
        self.status.set_label("All set — smart suggestions are on.");
        self.status.remove_css_class("muted");
        self.status.add_css_class("good");
        self.later.set_label("Close");
        self.go.set_visible(false);
    }

    /// Close without marking the welcome as seen (a download was cancelled).
    fn destroy_safe(&self) {
        self.closed.set(true);
        self.d.close();
    }

    fn close(&self) {
        if self.running.get() {
            self.cancel.borrow().store(true, Ordering::SeqCst);
            return;
        }
        if let Some(app) = self.app.upgrade() {
            app.settings_mut().first_run_done = true;
            app.save_settings();
        }
        self.closed.set(true);
        self.d.close();
    }
}

/// The download thread: each model in turn, then the models are loaded.
fn download_all(id: u64, keys: Vec<&'static str>, cancel: Arc<AtomicBool>) {
    let mut last: Option<Instant> = None;
    let n_keys = keys.len();
    for (n, key) in keys.iter().enumerate() {
        let title = download::spec(key).title;
        let n = n + 1;
        let mut progress = |done: u64, total: u64| {
            if last.is_some_and(|t| t.elapsed().as_secs_f64() < 0.1) && done < total {
                return;
            }
            last = Some(Instant::now());
            let text = format!(
                "Downloading {title} ({n}/{n_keys})…  {:.0} of {:.0} MB",
                done as f64 / 1e6,
                total as f64 / 1e6
            );
            let frac = done as f64 / total.max(1) as f64;
            workers::post(move || with_models(id, |d| d.progress(&text, frac)));
        };
        match download::download(key, &mut progress, &cancel) {
            Ok(()) => {}
            Err(download::DownloadError::Cancelled) => {
                workers::post(move || with_models(id, |d| d.destroy_safe()));
                return;
            }
            Err(e) => {
                let e = e.to_string();
                workers::post(move || with_models(id, |d| d.failed(&e)));
                return;
            }
        }
    }
    workers::post(move || {
        with_models(id, |d| {
            d.progress("Loading models…", 1.0);
            if let Some(app) = d.app.upgrade() {
                app.reload_models(Some(Box::new(move || with_models(id, |d| d.done()))));
            }
        })
    });
}

// -- auto-rewrite -------------------------------------------------------------------------------

fn strength_text(name: &str) -> &'static str {
    match name {
        "light" => "Rewords wordy phrases and swaps a few words (about 15%). Safest; meaning stays put.",
        "medium" => "Also replaces about 30% of the eligible words with synonyms that fit the context.",
        _ => "Changes up to half the eligible words and rephrases whole sentences when the paraphrase model is installed. Review the result carefully.",
    }
}

struct AutoDialog {
    app: Weak<App>,
    d: Dialog,
    selection: gtk::CheckButton,
    strength: RefCell<String>,
    seg: Vec<(&'static str, gtk::Button)>,
    desc: gtk::Label,
    phrases: gtk::CheckButton,
    words: gtk::CheckButton,
    sentences: gtk::CheckButton,
    t5: bool,
}

/// Ask how to auto-rewrite, then call `app.run_auto(scope, options, review)`.
pub fn auto_rewrite_dialog(app: &AppRc, has_selection: bool) {
    ensure_css(app);
    let d = Dialog::new(app, "Auto-rewrite", 440);
    let body = d.body.clone();
    d.label(&body, "Auto-rewrite", &["title"]);
    let l = d.label(
        &body,
        "Replace everything in one go, or just highlight the suggested changes and accept or reject them one by one.",
        &["muted"],
    );
    margins(&l, 4, 10);

    d.caption(&body, "APPLY TO");
    let sc = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    margins(&sc, 2, 8);
    let selection = gtk::CheckButton::with_label("Selected text");
    let document = gtk::CheckButton::with_label("Whole document");
    document.set_group(Some(&selection));
    selection.set_sensitive(has_selection);
    if has_selection { selection.set_active(true) } else { document.set_active(true) };
    sc.append(&selection);
    sc.append(&document);
    body.append(&sc);

    d.caption(&body, "STRENGTH");
    let segbox = gtk::Box::new(gtk::Orientation::Horizontal, 1);
    segbox.add_css_class("segment");
    segbox.set_halign(gtk::Align::Start);
    margins(&segbox, 4, 4);
    body.append(&segbox);
    let mut seg = vec![];
    for name in ["light", "medium", "strong"] {
        let mut label = name.to_string();
        label[..1].make_ascii_uppercase();
        let b = gtk::Button::with_label(&label);
        b.set_cursor_from_name(Some("pointer"));
        b.set_focus_on_click(false);
        segbox.append(&b);
        seg.push((name, b));
    }
    let desc = d.label(&body, "", &["small", "muted"]);
    margins(&desc, 0, 8);

    let wc = d.caption(&body, "WHAT TO CHANGE");
    margins(&wc, 0, 2);
    let phrases = gtk::CheckButton::with_label("Reword wordy / common phrases");
    phrases.set_active(true);
    let words = gtk::CheckButton::with_label("Swap words for synonyms that fit the context");
    words.set_active(true);
    let sentences = gtk::CheckButton::with_label("Rephrase whole sentences (paraphrase model)");
    let t5 = app.engine.has_paraphraser();
    sentences.set_sensitive(t5);
    for c in [&phrases, &words, &sentences] {
        body.append(c);
    }
    let foot = d.label(&body, "Markup, maths, code, citations and links are never changed.", &["small", "faint"]);
    margins(&foot, 8, 0);

    let (left, right) = d.buttons();
    let dlg: Rc<RefCell<Option<Rc<AutoDialog>>>> = Rc::new(RefCell::new(None));
    let call = |f: fn(&AutoDialog)| {
        let slot = dlg.clone();
        move || {
            let d = slot.borrow().clone();
            if let Some(d) = d {
                f(&d);
            }
        }
    };
    left.append(&widgets::flat_button("Cancel", "ghost", None, call(|d| d.close())));
    right.append(&widgets::flat_button(
        "Highlight to review",
        "soft",
        Some("Mark the changes; click each one to accept or reject it"),
        call(|d| d.run(true)),
    ));
    right.append(&widgets::flat_button("Replace all", "primary", Some("Apply every change now (one undo step)"), call(|d| d.run(false))));
    let a = Rc::new(AutoDialog {
        app: app.weak(),
        d,
        selection,
        strength: RefCell::new("medium".into()),
        seg,
        desc,
        phrases,
        words,
        sentences,
        t5,
    });
    *dlg.borrow_mut() = Some(a.clone());
    for (name, b) in &a.seg {
        let slot = dlg.clone();
        let name = *name;
        b.connect_clicked(move |_| {
            let d = slot.borrow().clone();
            if let Some(d) = d {
                d.set_strength(name);
            }
        });
    }
    a.d.keys(call(|d| d.close()), call(|d| d.run(true)));
    // the dialog lives as long as its window
    let slot = dlg.clone();
    a.d.win.connect_destroy(move |_| {
        slot.borrow_mut().take();
    });
    a.set_strength("medium");
    a.d.show();
}

impl AutoDialog {
    fn set_strength(&self, name: &str) {
        *self.strength.borrow_mut() = name.to_string();
        for (n, b) in &self.seg {
            if *n == name {
                b.add_css_class("on");
            } else {
                b.remove_css_class("on");
            }
        }
        self.desc.set_label(strength_text(name));
        if self.t5 {
            self.sentences.set_active(name == "strong");
        }
    }

    fn run(&self, review: bool) {
        let scope = if self.selection.is_active() { "selection" } else { "document" };
        let opts = AutoOptions {
            strength: self.strength.borrow().clone(),
            phrases: self.phrases.is_active(),
            words: self.words.is_active(),
            sentences: self.sentences.is_active() && self.t5,
            lines: false, // run_auto sets it for formatted documents
        };
        self.close();
        if let Some(app) = self.app.upgrade() {
            app.run_auto(scope, opts, review);
        }
    }

    fn close(&self) {
        self.d.close();
    }
}

// -- help ---------------------------------------------------------------------------------------

fn help_text() -> String {
    format!(
        "How Rewrite works

Open a file (txt, md, tex, docx, doc, odt, rtf, pdf, html, code …), drop one on the window, or just paste text. Then:

•  Click any word — the panel shows alternatives ranked by how well they fit that exact sentence. Click one (or press Alt+1 … Alt+9) to swap it in. Case and word form (plural, past tense …) are matched automatically.
•  Select a phrase — you get phrase-level rewrites: phrase-book equivalents, one-word swaps and model paraphrases.
•  {MOD}+E — rephrase the whole sentence at the cursor. Each option shows how much meaning is kept and how much was reworded. ↻ next to “Whole sentence” samples more variations.
•  Auto-rewrite ({MOD}+Shift+R) — a pass over the selection or the whole document at light / medium / strong strength. “Replace all” applies everything at once; “Highlight to review” marks the changes in blue so you can click each one and Accept (Alt+Y) or Reject (Alt+N); F8 jumps to the next one.
•  Show changes (± in the toolbar) — highlights everything that differs from the original text. Click a highlighted part to see the original and revert it.
•  Phrase hints (the light bulb) — underlines wordy or common phrases that have ready-made alternatives.
•  Spelling and grammar — a red underline marks a word that is not in the dictionary, a blue one a grammar slip (agreement, verb form, a/an, easily confused words). Click the word to see the fix, or right-click it. The two buttons next to “Spelling” add the word to your dictionary or ignore it, so it is no longer marked. Both checks can be switched off in the View menu.
•  Find and replace ({MOD}+F; {KEYS_REPLACE} opens it with replace) — a bar above the text finds as you type. Enter or {KEY_NEXT} goes to the next match, Shift+Enter or {KEY_PREV} to the previous one, Esc closes the bar. Aa matches upper and lower case, ab finds whole words only, .* takes a regular expression (\\1 in the replacement puts back the first group). “Replace all” is undone with one {MOD}+Z.
•  Preview (the eye, {MOD}+P) — opens a second panel on the right that shows the document as it will look. Markdown follows as you type; LaTeX is compiled to a PDF every time you save (it needs a LaTeX installation such as MiKTeX, MacTeX or TeX Live). ↻ or F5 refreshes or rebuilds at any time. While the preview is open, the suggestions move below the text.

Word, OpenDocument and RTF files (docx, doc, odt, fodt, rtf) open with their formatting, and File › New formatted document starts one. A second toolbar sets the paragraph style, font and size, bold ({MOD}+B), italic ({MOD}+I), underline ({MOD}+U), strike-through, superscript, subscript, colour, highlight and links ({MOD}+K), alignment, bulleted and numbered lists (Tab and Shift+Tab change the level; Enter on an empty item ends the list), and inserts pictures, tables, charts, formulas, footnotes, lines and page breaks. Double-click a picture, chart, formula or note to change it; right-click it for more. Table cells are typed in directly; right-click a cell to add or remove rows and columns. Shift+Enter starts a new line in the same paragraph. Page set-up (the page button at the right) sets the paper, margins and page numbers. All the rewriting tools work here too, and keep the formatting of the words they change. Save writes the same format back; Save as can switch between Word, OpenDocument and RTF (.doc needs LibreOffice).

PDF files open in the PDF view. Its toolbar picks a tool: select text (then {MOD}+C copies it, or right-click to mark it), edit a line of text (click it, type, Enter), add text, highlight, underline, strike out, sticky notes, drawing, white-out boxes, rectangles and pictures. The page buttons turn, move, delete and insert pages, and put in the pages of another PDF. Every change can be undone with {MOD}+Z. “Convert to document” turns the PDF into an editable formatted document: headings, paragraphs, lists, tables, footnotes and pictures are recognised, charts and formulas are kept as pictures, and words broken at line ends are joined (or kept as compounds) with the dictionary and the language model deciding the doubtful cases.

Export / convert ({MOD}+Shift+E, the arrow-out-of-a-box button) writes what is open as PDF, Word, OpenDocument, RTF, Word 97–2003, HTML, Markdown, LaTeX or plain text. A PDF can be typeset by Rewrite itself, by LaTeX (for LaTeX and Markdown files, when LaTeX is installed) or by LibreOffice (for formatted documents). For Markdown, LaTeX and text, “One sentence per line” puts each sentence on its own line, with sentence ends found by the language model (so “e.g.” or “Fig. 3” do not split a sentence). Pictures are saved next to the exported file.

The buttons are icons: rest the mouse on one to see what it does. The status bar at the bottom shows the file format (click it to change which parts are protected) and whether the smart models are loaded.

Markup is protected: LaTeX commands, maths, citations and references, Markdown code and links, HTML tags, and (in source files) everything except comments stay untouched and are shown dimmed.

Badges
  synonym — a dictionary synonym (WordNet) that also fits the context
  related — a closely related dictionary word (broader / similar term)
  in context — proposed by the language model from the context; check that it keeps your meaning
  phrase — from the built-in phrase book
  rewrite — generated by the paraphrase model

Keyboard
  {MOD}+O open   {MOD}+S save   {MOD}+Shift+S save as   {MOD}+N new
  {MOD}+Z undo   {MOD}+Y / {MOD}+Shift+Z redo
  {MOD}+F find   {KEYS_REPLACE} replace   {KEY_NEXT} / {KEY_PREV} next / previous match
  Ctrl+Space  suggestions at cursor   {MOD}+E rephrase sentence
  {MOD}+=  / {MOD}+-  text size   {MOD}+0 reset size
  {MOD}+P  preview panel   F5 refresh preview / rebuild PDF   {MOD}+Shift+E export / convert
  {MOD}+B / I / U  bold / italic / underline   {MOD}+K link   Shift+Enter line break (formatted documents)

Everything runs locally on your computer. Models: RoBERTa-base (context), all-MiniLM-L6-v2 (meaning check), and a T5-base paraphraser, run with ONNX Runtime. Thesaurus: Princeton WordNet 3.0.

A note on honesty: rewording someone else's text does not make it yours. Cite your sources — this tool is meant for improving and varying your own writing.

Rewrite {VERSION} · created by {AUTHOR} · {URL}"
    )
}

pub fn help_dialog(app: &AppRc) {
    let d = Rc::new(Dialog::new(app, "How to use Rewrite", 560));
    let buf = gtk::TextBuffer::new(None);
    let text = gtk::TextView::with_buffer(&buf);
    text.add_css_class("reader");
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_wrap_mode(gtk::WrapMode::Word);
    text.set_pixels_below_lines(2);
    let help = help_text();
    buf.set_text(&help);
    let title = buf.create_tag(Some("h"), &[("size-points", &(UI_SIZE + 4.0)), ("weight", &700i32), ("pixels-below-lines", &6i32)]).unwrap();
    let mut end = buf.start_iter();
    end.forward_to_line_end();
    buf.apply_tag(&title, &buf.start_iter(), &end);
    let bold = buf.create_tag(Some("b"), &[("weight", &700i32), ("pixels-above-lines", &5i32)]).unwrap();
    for head in ["Badges", "Keyboard"] {
        if let Some(pos) = help.find(&format!("\n{head}\n")) {
            let s = help[..pos + 1].chars().count() as i32;
            let mut e = buf.iter_at_offset(s);
            e.forward_to_line_end();
            buf.apply_tag(&bold, &buf.iter_at_offset(s), &e);
        }
    }
    // the text is longer than a small screen is tall, so it scrolls
    let line = text.create_pango_layout(Some("Ag")).pixel_size().1.max(1);
    let screen = app.win.surface().and_then(|s| gdk::Display::default()?.monitor_at_surface(&s)).map_or(800, |m| m.geometry().height());
    let lines = 14.max(30.min((screen - 260) / line));
    let width = text.create_pango_layout(Some(&"0".repeat(84))).pixel_size().0;
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_child(Some(&text));
    // room for the scroll bar beside the text
    text.set_right_margin(12);
    sw.set_size_request(width + 12, lines * (line + 2));
    sw.set_vexpand(true);
    d.body.set_size_request(-1, -1);
    d.body.append(&sw);
    let (_, right) = d.buttons();
    let d2 = d.clone();
    right.append(&widgets::flat_button("Close", "primary", None, move || d2.close()));
    let (d2, d3) = (d.clone(), d.clone());
    d.keys(move || d2.close(), move || d3.close());
    d.show();
}
