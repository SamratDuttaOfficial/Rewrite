//! The main window: toolbar, editor, suggestions, preview and status bar, the menus and keys, and
//! opening and saving files. What happens as the text changes (analysis, suggestions, review) is in
//! analysis.rs.

use crate::engine::{self, Engine, Offsets};
use crate::findbar::FindBar;
use crate::formatbar::FormatBar;
use crate::panel::Panel;
use crate::pdfview::{PdfBar, PdfEditor};
use crate::preview::Preview;
use crate::rich::RichEditor;
use crate::settings::{self, Settings};
use crate::theme::{self, Families, Theme, MOD};
use crate::widgets::{self, Toggle};
use crate::workers::{self, Cancel, Lane};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use rewrite_core::files;
use rewrite_core::model::Document;
use std::cell::{Cell, OnceCell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHOR: &str = "Samrat Dutta";
pub const URL: &str = "github.com/samratduttaofficial";
pub const PDF_HINT: &str = "To rewrite a PDF's text, use Convert to document; the pencil tool changes a line in place.";

/// The editor's text at one moment, with character <-> byte conversion.
pub struct Snap {
    pub text: String,
    pub off: Offsets,
    /// A number of its own, so background results can say which text they belong to.
    pub id: u64,
}

impl Snap {
    pub fn new(text: String) -> Rc<Snap> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let off = Offsets::new(&text);
        Rc::new(Snap { text, off, id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) })
    }
    /// The text between two character offsets.
    pub fn get(&self, s: usize, e: usize) -> &str {
        let (a, b) = (self.off.byte(s.min(self.len())), self.off.byte(e.min(self.len()).max(s.min(self.len()))));
        &self.text[a..b]
    }
    pub fn len(&self) -> usize {
        self.off.len()
    }
    pub fn char_at(&self, i: usize) -> Option<char> {
        self.text[self.off.byte(i)..].chars().next()
    }
}

/// The open document.
pub type Doc = files::OpenFile;

/// A word, phrase or misspelling the suggestions are for.
#[derive(Clone)]
pub struct Target {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// word | phrase | spell
    pub kind: String,
    /// None while loading; (suggestion, start, end, old text) each
    pub items: Option<Vec<(engine::Suggestion, usize, usize, String)>>,
    pub hint: Option<(usize, usize)>,
    pub hint_text: Option<String>,
    pub id: u64,
}

pub struct Proposal {
    pub old: String,
    pub new: String,
    pub start: gtk::TextMark,
    pub end: gtk::TextMark,
}

/// Everything about the document being edited that changes as the user works.
pub struct State {
    pub doc: Doc,
    pub mode: String,
    pub original: String,
    pub rich_original: Option<Document>,
    pub snap: Rc<Snap>,
    pub snap_dirty: bool,
    pub protected: Vec<(usize, usize)>,
    pub hint_spans: Vec<(usize, usize)>,
    pub changes: Vec<(usize, usize, usize, usize)>,
    pub change_ratio: f64,
    pub target: Option<Target>,
    pub next_target: u64,
    pub sent_target: Option<(usize, usize, String, u64)>,
    pub sent_items: Vec<engine::Suggestion>,
    pub variation: u32,
    pub busy: bool,
    pub dirty: bool,
    pub cache: HashMap<String, (Vec<engine::Suggestion>, Vec<engine::Suggestion>)>,
    pub cache_order: Vec<String>,
    pub popup_pending: bool,
    pub current_change: Option<(usize, usize, usize, usize)>,
    pub proposals: HashMap<u64, Proposal>,
    pub next_pid: u64,
    pub issues: Vec<engine::Issue>,
    pub issues_snap: Option<Rc<Snap>>,
    pub current_issue: Option<engine::Issue>,
    pub ignored: Vec<(String, String)>,
    pub proof_gen: u64,
    pub proof_base: Option<(Rc<Snap>, Vec<engine::Issue>)>,
    pub proof_more: bool,
    pub found: Vec<engine::Issue>,
    pub preview_on: bool,
    pub cancel: Cancel,
    pub models_state: String,
    pub suggest_name: Option<String>,
    /// Set while the app itself changes the text (loading a file), so it does not count as an edit.
    pub loading: bool,
}

pub struct Lanes {
    pub word: Lane,
    pub sent: Lane,
    pub slow: Lane,
    pub diff: Lane,
    pub proof: Lane,
}

pub struct Tags {
    pub prot: gtk::TextTag,
    pub hint: gtk::TextTag,
    pub chg: gtk::TextTag,
    pub prop: gtk::TextTag,
    pub target: gtk::TextTag,
    pub flash: gtk::TextTag,
    pub found: gtk::TextTag,
    pub gram: gtk::TextTag,
    pub spell: gtk::TextTag,
}

pub struct Widgets {
    /// The title in the window's own title bar.
    pub title: gtk::Label,
    pub bar_slot: gtk::Box,
    pub pane: gtk::Paned,
    pub left: gtk::Paned,
    pub editor_frame: gtk::Box,
    pub scroller: gtk::ScrolledWindow,
    pub panel_frame: gtk::Box,
    pub status_file: gtk::Button,
    pub status_stats: gtk::Label,
    pub mode_btn: gtk::Button,
    pub progress: gtk::ProgressBar,
    pub progress_lbl: gtk::Label,
    pub cancel_btn: gtk::Button,
    pub msg: gtk::Label,
    pub t_changes: Toggle,
    pub t_preview: Toggle,
    pub theme_btn: gtk::Button,
    pub recent_menu: gio::Menu,
}

pub struct App {
    pub gapp: gtk::Application,
    pub win: gtk::ApplicationWindow,
    pub theme: RefCell<Theme>,
    pub settings: RefCell<Settings>,
    pub engine: Engine,
    pub text: gtk::TextView,
    pub buf: gtk::TextBuffer,
    pub tags: Tags,
    pub st: RefCell<State>,
    pub lanes: Lanes,
    pub w: OnceCell<Widgets>,
    pub panel: OnceCell<Panel>,
    pub find: OnceCell<FindBar>,
    pub rich: OnceCell<RichEditor>,
    pub formatbar: OnceCell<FormatBar>,
    pub pdf: OnceCell<PdfEditor>,
    pub pdfbar: OnceCell<PdfBar>,
    pub preview: OnceCell<Preview>,
    pub me: RefCell<Weak<App>>,
    pub editor_css: gtk::CssProvider,
    /// The width of the reading column was last set for this view width.
    centred_for: Cell<i32>,
}

pub type AppRc = Rc<App>;

impl App {
    pub fn new(gapp: &gtk::Application) -> AppRc {
        let settings = settings::load();
        let win = gtk::ApplicationWindow::builder().application(gapp).title("Rewrite").build();
        let families = Families::detect(&win);
        let theme = Theme::new(&settings.theme, &settings.font_family, settings.font_size, families);
        let buf = gtk::TextBuffer::new(None);
        buf.set_enable_undo(true);
        buf.set_max_undo_levels(0);
        let text = gtk::TextView::with_buffer(&buf);
        let tags = make_tags(&buf);
        let engine = Engine::new(&settings.user_words);
        let st = State {
            doc: Doc::default(),
            mode: "plain".into(),
            original: String::new(),
            rich_original: None,
            snap: Snap::new(String::new()),
            snap_dirty: false,
            protected: vec![],
            hint_spans: vec![],
            changes: vec![],
            change_ratio: 0.0,
            target: None,
            next_target: 1,
            sent_target: None,
            sent_items: vec![],
            variation: 0,
            busy: false,
            dirty: false,
            cache: HashMap::new(),
            cache_order: vec![],
            popup_pending: false,
            current_change: None,
            proposals: HashMap::new(),
            next_pid: 0,
            issues: vec![],
            issues_snap: None,
            current_issue: None,
            ignored: vec![],
            proof_gen: 0,
            proof_base: None,
            proof_more: false,
            found: vec![],
            preview_on: false,
            cancel: Cancel::new(),
            models_state: "loading".into(),
            suggest_name: None,
            loading: false,
        };
        let app = Rc::new(App {
            gapp: gapp.clone(),
            win,
            theme: RefCell::new(theme),
            settings: RefCell::new(settings),
            engine,
            text,
            buf,
            tags,
            st: RefCell::new(st),
            lanes: Lanes {
                word: Lane::new("words"),
                sent: Lane::new("sentences"),
                slow: Lane::new("slow"),
                diff: Lane::new("diff"),
                proof: Lane::new("proof"),
            },
            w: OnceCell::new(),
            panel: OnceCell::new(),
            find: OnceCell::new(),
            rich: OnceCell::new(),
            formatbar: OnceCell::new(),
            pdf: OnceCell::new(),
            pdfbar: OnceCell::new(),
            preview: OnceCell::new(),
            me: RefCell::new(Weak::new()),
            editor_css: gtk::CssProvider::new(),
            centred_for: Cell::new(-1),
        });
        *app.me.borrow_mut() = Rc::downgrade(&app);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &app.editor_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        }
        app.build();
        app
    }

    pub fn weak(&self) -> Weak<App> {
        self.me.borrow().clone()
    }

    pub fn w(&self) -> &Widgets {
        self.w.get().expect("built")
    }
    pub fn panel(&self) -> &Panel {
        self.panel.get().expect("built")
    }
    pub fn find(&self) -> &FindBar {
        self.find.get().expect("built")
    }
    pub fn rich(&self) -> &RichEditor {
        self.rich.get().expect("built")
    }
    pub fn pdf(&self) -> &PdfEditor {
        self.pdf.get().expect("built")
    }
    pub fn preview(&self) -> &Preview {
        self.preview.get().expect("built")
    }
    pub fn theme(&self) -> Ref<'_, Theme> {
        self.theme.borrow()
    }
    pub fn settings(&self) -> Ref<'_, Settings> {
        self.settings.borrow()
    }
    pub fn settings_mut(&self) -> RefMut<'_, Settings> {
        self.settings.borrow_mut()
    }
    pub fn save_settings(&self) {
        settings::save(&self.settings.borrow());
    }
    pub fn busy(&self) -> bool {
        self.st.borrow().busy
    }
    pub fn mode(&self) -> String {
        self.st.borrow().mode.clone()
    }
    pub fn rich_active(&self) -> bool {
        self.rich.get().is_some_and(|r| r.active())
    }

    // -- building the window ---------------------------------------------------------------

    fn build(self: &Rc<Self>) {
        let win = &self.win;
        let (sw, sh) = screen_size();
        let saved = parse_size(&self.settings().geometry);
        // a window that filled the screen (or was maximised) opens maximised: at the screen's full size it
        // would run past its edges with the window's border and shadow
        let full = self.settings().maximized || saved.is_some_and(|(w, h)| w >= sw - 40 && h >= sh * 85 / 100);
        let (w, h) = match saved {
            Some((w, h)) if !full => (w.min(sw), h.min(sh)),
            _ => (1360.min(sw * 82 / 100), 860.min(sh * 82 / 100)),
        };
        if full {
            win.maximize();
        }
        win.set_default_size(w, h);
        win.set_size_request(760, 480);
        if let (Some(display), Some(icons)) = (gdk::Display::default(), rewrite_core::paths::asset("icons")) {
            gtk::IconTheme::for_display(&display).add_search_path(icons);
        }
        win.set_icon_name(Some("rewrite"));
        gtk::Window::set_default_icon_name("rewrite");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("bg");

        let recent_menu = gio::Menu::new();
        let model = self.build_menu(&recent_menu);
        let menubar: Option<gtk::Widget> = if cfg!(target_os = "macos") {
            // on macOS the menus are in the system menu bar at the top of the screen
            self.gapp.set_menubar(Some(&model));
            self.win.set_show_menubar(true);
            None
        } else {
            Some(gtk::PopoverMenuBar::from_model(Some(&model)).upcast())
        };
        let title = crate::titlebar::install(&self.win, "Rewrite", menubar.as_ref(), true, crate::titlebar::Buttons::All);

        // toolbar: icon buttons, each one explained by its tooltip
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bar.set_margin_start(6);
        bar.set_margin_end(6);
        bar.set_margin_top(3);
        bar.set_margin_bottom(3);
        let a = self.weak();
        let act = move |f: fn(&AppRc)| {
            let a = a.clone();
            move || {
                if let Some(app) = a.upgrade() {
                    f(&app)
                }
            }
        };
        bar.append(&widgets::icon_button("open", &format!("Open a file ({MOD}+O)"), act(|a| a.open_file(None))));
        bar.append(&widgets::icon_button("save", &format!("Save ({MOD}+S)"), act(|a| a.save(false))));
        bar.append(&widgets::separator(5));
        bar.append(&widgets::icon_button("undo", &format!("Undo ({MOD}+Z)"), act(|a| a.undo())));
        bar.append(&widgets::icon_button("redo", &format!("Redo ({MOD}+Y)"), act(|a| a.redo())));
        bar.append(&widgets::icon_button(
            "search",
            &format!("Find and replace ({MOD}+F, {})", theme::KEY_REPLACE),
            act(|a| a.find().open(false)),
        ));
        bar.append(&widgets::icon_button(
            "export",
            &format!("Export or convert: PDF, Word, OpenDocument, Markdown, LaTeX, text … ({MOD}+Shift+E)"),
            act(|a| a.export_dialog()),
        ));
        bar.append(&widgets::separator(5));
        bar.append(&widgets::icon_button("pencil", &format!("Rephrase the sentence at the cursor ({MOD}+E)"), act(|a| a.rephrase_sentence(false))));
        let auto = widgets::icon_text_button(
            "wand",
            "Auto-rewrite…",
            "primary",
            Some(&format!("Rewrite the selection or the whole document ({MOD}+Shift+R)")),
            act(|a| a.auto_rewrite()),
        );
        auto.set_margin_start(4);
        bar.append(&auto);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        bar.append(&spacer);
        let (show_changes, hints) = (self.settings().show_changes, self.settings().phrase_hints);
        let a2 = self.weak();
        let t_preview = Toggle::new(
            Some("eye"),
            "",
            false,
            &format!("Preview: the document as it will look: Markdown, the LaTeX PDF, or a formatted document as it prints ({MOD}+P)"),
            move |on| {
                if let Some(app) = a2.upgrade() {
                    app.set_preview(on, true)
                }
            },
        );
        bar.append(&t_preview.button);
        let a2 = self.weak();
        let t_hints = Toggle::new(Some("bulb"), "", hints, "Phrase hints: underline wordy phrases that have ready alternatives", move |on| {
            if let Some(app) = a2.upgrade() {
                app.set_hints(on)
            }
        });
        t_hints.button.set_margin_start(2);
        bar.append(&t_hints.button);
        let a2 = self.weak();
        let t_changes = Toggle::new(Some("diff"), "", show_changes, "Show changes: highlight everything that differs from the original", move |on| {
            if let Some(app) = a2.upgrade() {
                app.set_show_changes(on)
            }
        });
        t_changes.button.set_margin_start(2);
        bar.append(&t_changes.button);
        bar.append(&widgets::separator(5));
        let theme_btn = widgets::icon_button(if self.theme().dark() { "sun" } else { "moon" }, "Light / dark theme", act(|a| a.toggle_theme()));
        bar.append(&theme_btn);
        root.append(&bar);
        let _ = &t_hints;

        // the format bar (formatted documents) or the PDF bar goes here
        let bar_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&bar_slot);

        // the editor
        let text = &self.text;
        text.add_css_class("editor");
        text.set_wrap_mode(gtk::WrapMode::WordChar);
        text.set_top_margin(14);
        text.set_bottom_margin(14);
        text.set_left_margin(24);
        text.set_right_margin(24);
        text.set_pixels_above_lines(1);
        text.set_pixels_inside_wrap(3);
        text.set_pixels_below_lines(6);
        text.set_hexpand(true);
        text.set_vexpand(true);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_child(Some(text));
        scroller.add_css_class("surface");
        let editor_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor_frame.add_css_class("surface");
        let panel_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel_frame.add_css_class("panel");
        panel_frame.set_size_request(320, 220);

        let left = gtk::Paned::new(gtk::Orientation::Vertical);
        left.set_start_child(Some(&editor_frame));
        left.set_resize_start_child(true);
        left.set_shrink_start_child(false);
        left.set_shrink_end_child(false);
        let pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        pane.set_start_child(Some(&left));
        pane.set_end_child(Some(&panel_frame));
        pane.set_resize_start_child(true);
        pane.set_resize_end_child(false);
        pane.set_shrink_start_child(false);
        pane.set_shrink_end_child(false);
        pane.set_vexpand(true);
        root.append(&pane);

        // status bar: file and format, word count | messages, model status
        let sb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sb.add_css_class("bg");
        let status_file = widgets::flat_button("", "ghost", Some("File format (click to change)"), act(|a| a.pick_kind()));
        status_file.add_css_class("compact");
        status_file.add_css_class("small");
        status_file.set_margin_start(2);
        sb.append(&status_file);
        let status_stats = widgets::label("", &["small", "muted"]);
        status_stats.set_margin_start(6);
        sb.append(&status_stats);
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.set_hexpand(true);
        sb.append(&fill);
        let msg = widgets::label("", &["small", "muted"]);
        msg.set_ellipsize(gtk::pango::EllipsizeMode::End);
        msg.set_max_width_chars(110);
        sb.append(&msg);
        let progress_lbl = widgets::label("", &["small", "muted"]);
        progress_lbl.set_margin_end(8);
        progress_lbl.set_visible(false);
        sb.append(&progress_lbl);
        let progress = gtk::ProgressBar::new();
        progress.set_size_request(120, -1);
        progress.set_valign(gtk::Align::Center);
        progress.set_visible(false);
        sb.append(&progress);
        let cancel_btn = widgets::flat_button("Cancel", "link", None, act(|a| a.cancel_auto()));
        cancel_btn.add_css_class("small");
        cancel_btn.set_margin_start(8);
        cancel_btn.set_visible(false);
        sb.append(&cancel_btn);
        let mode_btn = widgets::flat_button("Loading models…", "ghost", Some("Models"), act(|a| a.open_models()));
        mode_btn.add_css_class("compact");
        mode_btn.add_css_class("small");
        mode_btn.set_margin_start(8);
        mode_btn.set_margin_end(2);
        sb.append(&mode_btn);
        root.append(&sb);
        win.set_child(Some(&root));

        let _ = self.w.set(Widgets {
            title,
                        bar_slot,
            pane,
            left,
            editor_frame: editor_frame.clone(),
            scroller: scroller.clone(),
            panel_frame: panel_frame.clone(),
            status_file,
            status_stats,
            mode_btn,
            progress,
            progress_lbl,
            cancel_btn,
            msg,
            t_changes,
            t_preview,
            theme_btn,
            recent_menu,
        });

        // the parts that need the app
        let _ = self.find.set(FindBar::new(self));
        editor_frame.append(&self.find().widget());
        editor_frame.append(&scroller);
        let _ = self.panel.set(Panel::new(self));
        panel_frame.append(&self.panel().widget());
        let _ = self.rich.set(RichEditor::new(self));
        let _ = self.formatbar.set(FormatBar::new(self));
        let _ = self.pdf.set(PdfEditor::new(self));
        let _ = self.pdfbar.set(PdfBar::new(self));
        let _ = self.preview.set(Preview::new(self));

        self.load_editor_css();
        self.color_editor();
        self.install_keys();
        self.connect_editor();
        self.accept_drops();
        self.update_recent_menu();
        let a = self.weak();
        self.theme().on_change(move || {
            if let Some(app) = a.upgrade() {
                app.color_editor();
            }
        });
        // keep the lines at a comfortable reading width on wide windows
        let a = self.weak();
        scroller.hadjustment().connect_page_size_notify(move |_| {
            if let Some(app) = a.upgrade() {
                app.center_column();
            }
        });
        let a = self.weak();
        scroller.vadjustment().connect_value_changed(move |_| {
            if let Some(app) = a.upgrade() {
                app.on_scroll();
            }
        });
        let a = self.weak();
        win.connect_close_request(move |_| {
            if let Some(app) = a.upgrade() {
                app.quit();
            }
            glib::Propagation::Stop
        });
        let a = self.weak();
        glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
            if let Some(app) = a.upgrade() {
                app.place_sash();
            }
        });
        text.grab_focus();
    }

    fn load_editor_css(&self) {
        let t = self.theme();
        self.editor_css.load_from_string(&format!(
            "textview.editor, textview.editor text {{ font-family: \"{}\"; font-size: {}pt; }}",
            t.editor_font(),
            t.size
        ));
        drop(t);
        self.centred_for.set(-1);
        self.center_column();
    }

    pub fn center_column(&self) {
        let width = self.w().scroller.hadjustment().page_size() as i32;
        if width <= 1 {
            return;
        }
        let layout = self.text.create_pango_layout(Some("n"));
        let col = layout.pixel_size().0 * 84;
        let pad = 18.max((width - col) / 2);
        if self.centred_for.get() != width || self.text.left_margin() != pad {
            self.centred_for.set(width);
            self.text.set_left_margin(pad);
            self.text.set_right_margin(pad);
            if let Some(f) = self.find.get() {
                f.align(pad);
            }
        }
    }

    pub fn place_sash(&self) {
        let w = self.w();
        let width = w.pane.width();
        if width < 500 {
            let a = self.weak();
            glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
                if let Some(app) = a.upgrade() {
                    app.place_sash();
                }
            });
            return;
        }
        if self.st.borrow().preview_on {
            let split = self.settings().preview_split;
            w.pane.set_position(360.max((width as f64 * split) as i32));
            let h = w.left.height();
            if h > 0 {
                w.left.set_position(140.max(h - 150.max((h as f64 * 0.3) as i32)));
            }
        } else {
            w.pane.set_position(360.max(width - 320));
        }
    }

    pub fn color_editor(&self) {
        let t = self.theme();
        let s = self.settings();
        let tg = &self.tags;
        tg.prot.set_foreground_rgba(Some(&t.rgba("protected")));
        if s.phrase_hints {
            tg.hint.set_background_rgba(Some(&t.rgba("hint")));
            tg.hint.set_underline(gtk::pango::Underline::Single);
        } else {
            tg.hint.set_background_rgba(None);
            tg.hint.set_underline(gtk::pango::Underline::None);
        }
        tg.chg.set_background_rgba(if s.show_changes { Some(t.rgba("changed")) } else { None }.as_ref());
        tg.prop.set_background_rgba(Some(&t.rgba("proposal")));
        tg.prop.set_underline(gtk::pango::Underline::Single);
        tg.target.set_background_rgba(Some(&t.rgba("target")));
        tg.flash.set_background_rgba(Some(&t.rgba("flash")));
        tg.found.set_background_rgba(Some(&t.rgba("found")));
        tg.gram.set_underline(gtk::pango::Underline::Single);
        tg.gram.set_underline_rgba(Some(&t.rgba("grammar")));
        tg.spell.set_underline(gtk::pango::Underline::Single);
        tg.spell.set_underline_rgba(Some(&t.rgba("spell")));
        // the stacking order: later ones are drawn over earlier ones
        let n = self.buf.tag_table().size();
        for (k, tag) in [&tg.hint, &tg.chg, &tg.gram, &tg.spell, &tg.prop, &tg.found, &tg.target, &tg.flash].iter().enumerate() {
            tag.set_priority(n - 8 + k as i32);
        }
        tg.prot.set_priority(0.max(n - 9));
    }

    // -- menus and keys ------------------------------------------------------------------

    fn build_menu(self: &Rc<Self>, recent: &gio::Menu) -> gio::Menu {
        let m = gio::Menu::new();
        let item = |menu: &gio::Menu, label: &str, action: &str, accel: &str| {
            let it = gio::MenuItem::new(Some(label), Some(action));
            if !accel.is_empty() {
                it.set_attribute_value("accel", Some(&accel.to_variant()));
            }
            menu.append_item(&it);
        };
        let section = |parent: &gio::Menu| {
            let s = gio::Menu::new();
            parent.append_section(None, &s);
            s
        };
        let acc = |k: &str| format!("<Primary>{k}");

        let f = gio::Menu::new();
        let s1 = section(&f);
        item(&s1, "New text", "win.new", &acc("n"));
        item(&s1, "New formatted document", "win.new-rich", "");
        item(&s1, "Open…", "win.open", &acc("o"));
        s1.append_submenu(Some("Open recent"), recent);
        let s2 = section(&f);
        item(&s2, "Save", "win.save", &acc("s"));
        item(&s2, "Save as…", "win.save-as", &acc("<Shift>s"));
        item(&s2, "Export / convert…", "win.export", &acc("<Shift>e"));
        let s3 = section(&f);
        item(&s3, "Page set-up…", "win.page-setup", "");
        let s4 = section(&f);
        item(&s4, "Quit", "win.quit", "");
        m.append_submenu(Some("File"), &f);

        let e = gio::Menu::new();
        let s1 = section(&e);
        item(&s1, "Undo", "win.undo", &acc("z"));
        item(&s1, "Redo", "win.redo", &acc("y"));
        let s2 = section(&e);
        item(&s2, "Cut", "win.cut", "");
        item(&s2, "Copy", "win.copy", "");
        item(&s2, "Paste", "win.paste", "");
        item(&s2, "Select all", "win.select-all", &acc("a"));
        let s3 = section(&e);
        item(&s3, "Find…", "win.find", &acc("f"));
        item(&s3, "Replace…", "win.replace", if cfg!(target_os = "macos") { "<Primary>r" } else { "<Primary>h" });
        item(&s3, "Find next", "win.find-next", if cfg!(target_os = "macos") { "<Primary>g" } else { "F3" });
        item(&s3, "Find previous", "win.find-prev", if cfg!(target_os = "macos") { "<Primary><Shift>g" } else { "<Shift>F3" });
        let s4 = section(&e);
        item(&s4, "Use current text as the original", "win.mark-original", "");
        item(&s4, "Revert everything to the original", "win.revert-all", "");
        m.append_submenu(Some("Edit"), &e);

        let ins = gio::Menu::new();
        for (label, kind) in [
            ("Picture…", "image"),
            ("Table…", "table"),
            ("Chart…", "chart"),
            ("Formula…", "math"),
            ("Footnote…", "note"),
            ("Link…", "link"),
            ("Horizontal line", "rule"),
            ("Page break", "pagebreak"),
        ] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some("win.insert"), Some(&kind.to_variant()));
            if kind == "link" {
                it.set_attribute_value("accel", Some(&acc("k").to_variant()));
            }
            ins.append_item(&it);
        }
        m.append_submenu(Some("Insert"), &ins);

        let fm = gio::Menu::new();
        let s1 = section(&fm);
        for (label, name, key) in [
            ("Bold", "b", acc("b")),
            ("Italic", "i", acc("i")),
            ("Underline", "u", acc("u")),
            ("Strikethrough", "s", String::new()),
            ("Superscript", "sup", String::new()),
            ("Subscript", "sub", String::new()),
        ] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some("win.format"), Some(&name.to_variant()));
            if !key.is_empty() {
                it.set_attribute_value("accel", Some(&key.to_variant()));
            }
            s1.append_item(&it);
        }
        let it = gio::MenuItem::new(Some("Clear formatting"), None);
        it.set_action_and_target_value(Some("win.format"), Some(&"clear".to_variant()));
        s1.append_item(&it);
        let s2 = section(&fm);
        let styles = gio::Menu::new();
        for s in ["normal", "title", "subtitle", "h1", "h2", "h3", "quote", "caption", "code"] {
            let label = if s == "code" { "Code".to_string() } else { style_label(s) };
            let it = gio::MenuItem::new(Some(&label), None);
            it.set_action_and_target_value(Some("win.para-style"), Some(&s.to_variant()));
            styles.append_item(&it);
        }
        s2.append_submenu(Some("Paragraph style"), &styles);
        for (label, a) in [("Align left", "left"), ("Centre", "center"), ("Align right", "right"), ("Justify", "justify")] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some("win.align"), Some(&a.to_variant()));
            s2.append_item(&it);
        }
        let s3 = section(&fm);
        for (label, a) in [("Bulleted list", "bullet"), ("Numbered list", "number"), ("Increase indent", "indent"), ("Decrease indent", "outdent")] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some("win.list"), Some(&a.to_variant()));
            s3.append_item(&it);
        }
        m.append_submenu(Some("Format"), &fm);

        let w = gio::Menu::new();
        item(&w, "Suggestions at cursor", "win.popup", "<Control>space");
        item(&w, "Rephrase sentence", "win.rephrase", &acc("e"));
        item(&w, "Auto-rewrite…", "win.auto-rewrite", &acc("<Shift>r"));
        m.append_submenu(Some("Rewrite"), &w);

        let v = gio::Menu::new();
        let s1 = section(&v);
        item(&s1, "Zoom in", "win.zoom-in", &acc("equal"));
        item(&s1, "Zoom out", "win.zoom-out", &acc("minus"));
        item(&s1, "Actual size", "win.zoom-reset", &acc("0"));
        let s2 = section(&v);
        for (fam, label) in [("serif", "Serif font"), ("sans", "Sans-serif font"), ("mono", "Monospace font")] {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some("win.font-family"), Some(&fam.to_variant()));
            s2.append_item(&it);
        }
        let s3 = section(&v);
        item(&s3, "Check spelling (red underline)", "win.spell-check", "");
        item(&s3, "Check grammar (blue underline)", "win.grammar-check", "");
        let s4 = section(&v);
        item(&s4, "Preview panel", "win.preview", &acc("p"));
        item(&s4, "Refresh preview / rebuild PDF", "win.refresh-preview", "F5");
        let s5 = section(&v);
        item(&s5, "Light / dark theme", "win.theme", "");
        m.append_submenu(Some("View"), &v);

        let mm = gio::Menu::new();
        item(&mm, "Manage smart models…", "win.models", "");
        m.append_submenu(Some("Models"), &mm);
        let hm = gio::Menu::new();
        item(&hm, "How to use Rewrite", "win.help", "F1");
        item(&hm, "About", "win.about", "");
        m.append_submenu(Some("Help"), &hm);

        self.install_actions();
        m
    }

    fn install_actions(self: &Rc<Self>) {
        let win = &self.win;
        let add = |name: &str, f: fn(&AppRc)| {
            let a = gio::SimpleAction::new(name, None);
            let me = self.weak();
            a.connect_activate(move |_, _| {
                if let Some(app) = me.upgrade() {
                    f(&app);
                }
            });
            win.add_action(&a);
        };
        add("new", |a| a.new_file(false));
        add("new-rich", |a| a.new_file(true));
        add("open", |a| a.open_file(None));
        add("save", |a| a.save(false));
        add("save-as", |a| a.save(true));
        add("export", |a| a.export_dialog());
        add("page-setup", |a| a.page_setup());
        add("quit", |a| a.quit());
        add("undo", |a| a.undo());
        add("redo", |a| a.redo());
        add("cut", |a| a.clipboard("cut"));
        add("copy", |a| a.clipboard("copy"));
        add("paste", |a| a.clipboard("paste"));
        add("select-all", |a| a.select_all());
        add("find", |a| a.find().open(false));
        add("replace", |a| a.find().open(true));
        add("find-next", |a| a.find().next(false));
        add("find-prev", |a| a.find().next(true));
        add("mark-original", |a| a.mark_original());
        add("revert-all", |a| a.revert_all());
        add("popup", |a| a.popup_at_cursor());
        add("rephrase", |a| a.rephrase_sentence(false));
        add("auto-rewrite", |a| a.auto_rewrite());
        add("zoom-in", |a| a.zoom(1));
        add("zoom-out", |a| a.zoom(-1));
        add("zoom-reset", |a| a.zoom(0));
        add("refresh-preview", |a| a.refresh_preview());
        add("theme", |a| a.toggle_theme());
        add("models", |a| a.open_models());
        add("help", |a| crate::dialogs::help_dialog(a));
        add("about", |a| a.about());
        let with_arg = |name: &str, f: fn(&AppRc, &str)| {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let me = self.weak();
            a.connect_activate(move |_, v| {
                if let (Some(app), Some(s)) = (me.upgrade(), v.and_then(|v| v.str())) {
                    f(&app, s);
                }
            });
            win.add_action(&a);
        };
        with_arg("insert", |a, k| a.rich_insert(k));
        with_arg("format", |a, k| {
            if k == "clear" {
                a.fmt(|r| r.clear_format());
            } else {
                let k = k.to_string();
                a.fmt(move |r| r.toggle(&k));
            }
        });
        with_arg("para-style", |a, s| {
            let s = s.to_string();
            a.fmt(move |r| r.set_style(&s));
        });
        with_arg("align", |a, s| {
            let s = if s == "left" { None } else { Some(s.to_string()) };
            a.fmt(move |r| r.set_align(s.clone()));
        });
        with_arg("list", |a, s| match s {
            "indent" => a.fmt(|r| r.change_level(1)),
            "outdent" => a.fmt(|r| r.change_level(-1)),
            k => {
                let k = k.to_string();
                a.fmt(move |r| r.toggle_list(&k));
            }
        });
        with_arg("open-recent", |a, p| a.open_file(Some(PathBuf::from(p))));
        // stateful: the font family radio and the check items
        let fam = self.theme().editor_family.clone();
        let font = gio::SimpleAction::new_stateful("font-family", Some(glib::VariantTy::STRING), &fam.to_variant());
        let me = self.weak();
        font.connect_activate(move |act, v| {
            if let (Some(app), Some(v)) = (me.upgrade(), v) {
                act.set_state(v);
                app.set_font_family(v.str().unwrap_or("serif"));
            }
        });
        win.add_action(&font);
        for (name, on) in [("spell-check", self.settings().spell_check), ("grammar-check", self.settings().grammar_check)] {
            let act = gio::SimpleAction::new_stateful(name, None, &on.to_variant());
            let me = self.weak();
            let key = name.to_string();
            act.connect_activate(move |act, _| {
                let v = !act.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
                act.set_state(&v.to_variant());
                if let Some(app) = me.upgrade() {
                    if key == "spell-check" {
                        app.settings_mut().spell_check = v;
                    } else {
                        app.settings_mut().grammar_check = v;
                    }
                    app.set_proofing();
                }
            });
            win.add_action(&act);
        }
        let prev = gio::SimpleAction::new_stateful("preview", None, &false.to_variant());
        let me = self.weak();
        prev.connect_activate(move |act, _| {
            let v = !act.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            if let Some(app) = me.upgrade() {
                app.set_preview(v, true);
            }
        });
        win.add_action(&prev);
        self.set_rich_menus(false);
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.win.lookup_action(name).and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    }

    fn set_rich_menus(&self, on: bool) {
        for name in ["insert", "format", "para-style", "align", "list"] {
            if let Some(a) = self.action(name) {
                a.set_enabled(on);
            }
        }
    }

    /// Keyboard shortcuts. Editing keys (Ctrl+Z, Ctrl+A ...) are left to a text field that has the focus.
    fn install_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.weak();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(app) = me.upgrade() else { return glib::Propagation::Proceed };
            if app.on_key(key, state) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        self.win.add_controller(keys);
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> bool {
        let primary = if cfg!(target_os = "macos") { gdk::ModifierType::META_MASK } else { gdk::ModifierType::CONTROL_MASK };
        let prim = state.contains(primary);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let lower = key.to_lower();
        let focus = gtk::prelude::GtkWindowExt::focus(&self.win);
        let in_text = focus.as_ref().is_some_and(|f| f == self.text.upcast_ref::<gtk::Widget>() || f.is_ancestor(&self.text));
        // the editor itself, not a table cell inside it
        let in_main = focus.as_ref().is_some_and(|f| f == self.text.upcast_ref::<gtk::Widget>());
        let in_entry = focus.as_ref().is_some_and(|f| f.is::<gtk::Text>() || f.ancestor(gtk::Entry::static_type()).is_some());
        let pdf = self.mode() == "pdf";
        if prim && !alt {
            match lower {
                gdk::Key::o => self.open_file(None),
                gdk::Key::s if shift => self.save(true),
                gdk::Key::s => self.save(false),
                gdk::Key::n => self.new_file(false),
                gdk::Key::e if shift => self.export_dialog(),
                gdk::Key::e => self.rephrase_sentence(false),
                gdk::Key::f => self.find().open(false),
                gdk::Key::r if shift => self.auto_rewrite(),
                gdk::Key::r => self.find().open(true),
                gdk::Key::h if !cfg!(target_os = "macos") => self.find().open(true),
                gdk::Key::g if cfg!(target_os = "macos") => self.find().next(shift),
                gdk::Key::p => {
                    let on = !self.st.borrow().preview_on;
                    self.set_preview(on, true)
                }
                gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add => self.zoom(1),
                gdk::Key::minus | gdk::Key::KP_Subtract => self.zoom(-1),
                gdk::Key::_0 | gdk::Key::KP_0 => self.zoom(0),
                gdk::Key::y if !in_entry => self.redo(),
                gdk::Key::z if shift && !in_entry => self.redo(),
                gdk::Key::z if !in_entry => self.undo(),
                gdk::Key::a if in_main || (pdf && !in_entry) => self.select_all(),
                gdk::Key::k if self.rich_active() && in_main => self.rich_link(),
                gdk::Key::b | gdk::Key::i | gdk::Key::u if self.rich_active() && in_main => {
                    let n = match lower {
                        gdk::Key::b => "b",
                        gdk::Key::i => "i",
                        _ => "u",
                    };
                    self.fmt(move |r| r.toggle(n));
                }
                _ => return false,
            }
            return true;
        }
        if ctrl && lower == gdk::Key::space {
            self.popup_at_cursor();
            return true;
        }
        if alt && in_text && !prim {
            match lower {
                gdk::Key::y => self.accept_prop(),
                gdk::Key::n => self.reject_prop(),
                k => {
                    let n = match k {
                        gdk::Key::_1 => 1,
                        gdk::Key::_2 => 2,
                        gdk::Key::_3 => 3,
                        gdk::Key::_4 => 4,
                        gdk::Key::_5 => 5,
                        gdk::Key::_6 => 6,
                        gdk::Key::_7 => 7,
                        gdk::Key::_8 => 8,
                        gdk::Key::_9 => 9,
                        _ => return false,
                    };
                    self.apply_suggestion(n - 1);
                }
            }
            return true;
        }
        match key {
            gdk::Key::F1 => crate::dialogs::help_dialog(self),
            gdk::Key::F3 if !cfg!(target_os = "macos") => self.find().next(shift),
            gdk::Key::F5 => self.refresh_preview(),
            gdk::Key::F8 => self.next_prop(None, false),
            gdk::Key::Escape if in_text && self.find().visible() => self.find().close(),
            _ => return false,
        }
        true
    }

    fn clipboard(&self, what: &str) {
        // through the text view, so a formatted document copies and pastes its formatting too
        match what {
            "cut" => self.text.emit_cut_clipboard(),
            "copy" => self.text.emit_copy_clipboard(),
            _ => self.text.emit_paste_clipboard(),
        }
    }

    // -- drag and drop --------------------------------------------------------------------

    fn accept_drops(self: &Rc<Self>) {
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let me = self.weak();
        drop.connect_enter(move |_, _, _| {
            if let Some(app) = me.upgrade() {
                app.flash_msg("Drop the file to open it", 60000);
            }
            gdk::DragAction::COPY
        });
        let me = self.weak();
        drop.connect_leave(move |_| {
            if let Some(app) = me.upgrade() {
                app.flash_msg("", 1);
            }
        });
        let me = self.weak();
        drop.connect_drop(move |_, value, _, _| {
            let Some(app) = me.upgrade() else { return false };
            app.flash_msg("", 1);
            let files: Vec<PathBuf> = value
                .get::<gdk::FileList>()
                .map(|l| l.files().iter().filter_map(|f| f.path()).filter(|p| p.is_file()).collect())
                .unwrap_or_default();
            if files.is_empty() {
                app.flash_msg("Drop a file, not a folder.", 5000);
                return true;
            }
            // open after the drop has returned, so the program the file came from is not kept waiting
            let a = app.weak();
            glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                let Some(app) = a.upgrade() else { return };
                app.win.present();
                if app.busy() {
                    app.flash_msg("Wait for the rewrite to finish, then drop the file again.", 5000);
                    return;
                }
                let first = files[0].clone();
                let more = files.len() > 1;
                let a2 = app.weak();
                app.open_path_then(first.clone(), move |opened| {
                    if let (true, true, Some(app)) = (opened, more, a2.upgrade()) {
                        app.flash_msg(
                            &format!("Opened {} — Rewrite works on one file at a time.", first.file_name().unwrap_or_default().to_string_lossy()),
                            5000,
                        );
                    }
                });
            });
            true
        });
        self.win.add_controller(drop);
    }

    // -- documents --------------------------------------------------------------------------

    /// plain: text files; rich: formatted documents (the format bar shows); pdf: the PDF view.
    pub fn set_mode(&self, mode: &str) {
        let old = self.mode();
        if old == mode {
            return;
        }
        let w = self.w();
        if old == "rich" {
            self.rich().detach();
        }
        widgets::clear(&w.bar_slot);
        if old == "pdf" {
            w.left.set_start_child(Some(&w.editor_frame));
            if !self.st.borrow().preview_on {
                w.pane.set_end_child(Some(&w.panel_frame));
            }
            let a = self.weak();
            glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                if let Some(app) = a.upgrade() {
                    app.place_sash();
                }
            });
        }
        self.st.borrow_mut().mode = mode.to_string();
        if mode == "rich" {
            w.bar_slot.append(&self.formatbar.get().unwrap().widget());
            self.rich().attach();
        }
        if mode == "pdf" {
            if self.st.borrow().preview_on {
                self.set_preview(false, false);
            }
            w.bar_slot.append(&self.pdfbar.get().unwrap().widget());
            w.left.set_start_child(Some(&self.pdf().widget()));
            w.pane.set_end_child(None::<&gtk::Widget>);
        }
        self.set_rich_menus(mode == "rich");
    }

    /// Put plain text in the editor.
    pub fn set_text(&self, text: &str, original: Option<String>) {
        self.set_mode("plain");
        self.text.set_editable(true);
        self.clear_proposals();
        self.st.borrow_mut().loading = true;
        self.buf.begin_irreversible_action();
        self.buf.set_text(text);
        self.buf.end_irreversible_action();
        self.buf.place_cursor(&self.buf.start_iter());
        self.st.borrow_mut().loading = false;
        self.fresh(text.to_string(), original);
    }

    /// Show a formatted document for editing.
    pub fn set_rich(&self, model: Document) {
        self.set_mode("rich");
        self.text.set_editable(true);
        self.clear_proposals();
        self.st.borrow_mut().loading = true;
        self.rich().load(&model);
        self.st.borrow_mut().loading = false;
        self.st.borrow_mut().rich_original = Some(model);
        let plain = self.rich().plain();
        self.fresh(plain, None);
        self.formatbar.get().unwrap().refresh();
    }

    /// A document was just put in the editor: forget everything about the previous one.
    fn fresh(&self, text: String, original: Option<String>) {
        {
            let mut st = self.st.borrow_mut();
            st.original = original.unwrap_or_else(|| text.clone());
            st.snap = Snap::new(text);
            st.snap_dirty = false;
            st.dirty = false;
            st.target = None;
            st.sent_items.clear();
            st.sent_target = None;
            st.cache.clear();
            st.cache_order.clear();
            st.issues.clear();
            st.current_issue = None;
        }
        self.panel().reset();
        self.preview().new_document();
        self.update_title();
        self.analyze();
        if self.st.borrow().preview_on {
            self.preview().refresh(false);
        }
    }

    pub fn update_title(&self) {
        let st = self.st.borrow();
        let name = st.doc.name();
        let dot = if st.dirty { "• " } else { "" };
        let title = format!("{dot}{name} — Rewrite");
        self.win.set_title(Some(&title));
        self.w().title.set_label(&title);
        let mut kind = kind_label(&st.doc.kind);
        if matches!(st.doc.kind.as_str(), "rich" | "pdf") {
            let ext = st.doc.source_ext.clone().unwrap_or_else(|| st.doc.ext());
            if let Some(l) = format_label(&ext) {
                kind = l;
            }
        }
        self.w().status_file.set_label(&format!("{name}  ·  {kind}"));
    }

    /// Ask to save unsaved changes, then run `then(true)` if the document may be replaced.
    pub fn confirm_discard(self: &Rc<Self>, then: impl FnOnce(bool) + 'static) {
        if !self.st.borrow().dirty {
            then(true);
            return;
        }
        let name = self.st.borrow().doc.name();
        let me = self.clone();
        crate::dialogs::message(&self.win, "Unsaved changes", &format!("Save changes to {name}?"), &["Cancel", "Don't save", "Save"], 2, 0, move |choice| {
            match choice {
                2 => me.save_then(false, then),
                1 => then(true),
                _ => then(false),
            }
        });
    }

    pub fn new_file(self: &Rc<Self>, rich: bool) {
        if self.busy() {
            return;
        }
        let me = self.clone();
        self.confirm_discard(move |ok| {
            if !ok {
                return;
            }
            if rich {
                me.st.borrow_mut().doc = Doc { kind: "rich".into(), model: Some(Document::default()), ..Doc::default() };
                me.set_rich(Document::default());
            } else {
                me.st.borrow_mut().doc = Doc::default();
                me.set_text("", None);
            }
        });
    }

    pub fn open_file(self: &Rc<Self>, path: Option<PathBuf>) {
        self.open_path_then_opt(path, |_| {});
    }

    pub fn open_path_then(self: &Rc<Self>, path: PathBuf, then: impl FnOnce(bool) + 'static) {
        self.open_path_then_opt(Some(path), then);
    }

    fn open_path_then_opt(self: &Rc<Self>, path: Option<PathBuf>, then: impl FnOnce(bool) + 'static) {
        if self.busy() {
            then(false);
            return;
        }
        let me = self.clone();
        self.confirm_discard(move |ok| {
            if !ok {
                then(false);
                return;
            }
            match path {
                Some(p) => then(me.load_path(&p)),
                None => {
                    let dialog = gtk::FileDialog::builder().title("Open a document").modal(true).build();
                    dialog.set_filters(Some(&file_filters(files::OPEN_TYPES)));
                    if let Some(d) = me.settings().last_dir.clone().or_else(|| Some(rewrite_core::paths::home().to_string_lossy().into_owned())) {
                        dialog.set_initial_folder(Some(&gio::File::for_path(d)));
                    }
                    let me2 = me.clone();
                    glib::spawn_future_local(async move {
                        if let Ok(f) = dialog.open_future(Some(&me2.win)).await {
                            if let Some(p) = f.path() {
                                then(me2.load_path(&p));
                                return;
                            }
                        }
                        then(false);
                    });
                }
            }
        });
    }

    /// Read a file and show it. Returns whether it opened.
    fn load_path(&self, path: &Path) -> bool {
        self.win.set_cursor_from_name(Some("wait"));
        flush_events();
        let loaded = files::load(path);
        self.win.set_cursor(None::<&gdk::Cursor>);
        let loaded = match loaded {
            Ok(l) => l,
            Err(e) => {
                self.error("Could not open file", &format!("{}\n\n{e}", path.file_name().unwrap_or_default().to_string_lossy()));
                return false;
            }
        };
        {
            let mut s = self.settings_mut();
            s.last_dir = path.parent().map(|d| d.to_string_lossy().into_owned());
            let p = path.to_string_lossy().into_owned();
            s.recent.retain(|r| r != &p);
            s.recent.insert(0, p);
            s.recent.truncate(8);
        }
        self.save_settings();
        self.update_recent_menu();
        match loaded.kind.as_str() {
            "rich" => {
                let model = loaded.model.clone().unwrap_or_default();
                self.st.borrow_mut().doc = loaded;
                self.set_rich(model);
            }
            "pdf" => {
                let data = loaded.data.clone().unwrap_or_default();
                self.st.borrow_mut().doc = loaded;
                self.open_pdf(data);
            }
            _ => {
                let text = std::mem::take(&mut self.st.borrow_mut().doc.text);
                let text = if text.is_empty() { loaded.text.clone() } else { text };
                self.st.borrow_mut().doc = Doc { text: String::new(), ..loaded };
                self.set_text(&text, None);
            }
        }
        true
    }

    pub fn update_recent_menu(&self) {
        let menu = &self.w().recent_menu;
        menu.remove_all();
        let rec: Vec<String> = self.settings().recent.iter().filter(|p| Path::new(p).exists()).cloned().collect();
        if rec.is_empty() {
            menu.append(Some("(none)"), Some("win.none"));
        }
        for p in rec {
            let it = gio::MenuItem::new(Some(&p.replace('_', "__")), None);
            it.set_action_and_target_value(Some("win.open-recent"), Some(&p.to_variant()));
            menu.append_item(&it);
        }
    }

    pub fn save(self: &Rc<Self>, as_: bool) {
        self.save_then(as_, |_| {});
    }

    /// Save, then call `then(saved)`.
    pub fn save_then(self: &Rc<Self>, as_: bool, then: impl FnOnce(bool) + 'static) {
        if self.busy() {
            then(false);
            return;
        }
        if self.mode() == "pdf" {
            self.pdf().save(as_, Box::new(then));
            return;
        }
        let rich = self.mode() == "rich";
        let (mut path, kind, cur) = {
            let st = self.st.borrow();
            (st.doc.path.clone(), st.doc.kind.clone(), st.doc.path.clone())
        };
        if rich {
            if let Some(p) = &path {
                if !files::SAVE_RICH.iter().any(|(e, _)| ext_of(p) == *e) {
                    path = None; // a template or other read-only kind: save under a new name
                }
            }
        }
        if !as_ {
            if let Some(p) = path {
                then(self.write_to(&p));
                return;
            }
        }
        let base_path = path.or(cur);
        let (base, mut ext) = match &base_path {
            Some(p) => (p.file_stem().unwrap_or_default().to_string_lossy().into_owned(), ext_of(p)),
            None => ("untitled".to_string(), String::new()),
        };
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        if rich {
            if !matches!(ext.as_str(), ".docx" | ".odt" | ".fodt" | ".rtf" | ".doc") {
                ext = ".docx".into();
            }
            let mut types: Vec<&files::FileType> = files::SAVE_RICH_TYPES.iter().collect();
            let want = format!("*{ext}");
            types.sort_by_key(|t| !t.patterns.contains(&want.as_str()));
            for t in types {
                filters.append(&filter(t.label, t.patterns));
            }
        } else {
            if ext.is_empty() {
                ext = match kind.as_str() {
                    "latex" => ".tex",
                    "markdown" => ".md",
                    "html" => ".html",
                    _ => ".txt",
                }
                .into();
            }
            let same = format!("*{ext}");
            for (label, pats) in [("Same as original", vec![same.as_str()]), ("Text", vec!["*.txt"]), ("Markdown", vec!["*.md"]), ("LaTeX", vec!["*.tex"]), ("All files", vec!["*"])] {
                filters.append(&filter(label, &pats));
            }
        }
        let dialog = gtk::FileDialog::builder().title("Save as").modal(true).initial_name(format!("{base}{ext}")).build();
        dialog.set_filters(Some(&filters));
        let dir = base_path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.settings().last_dir.clone().map(PathBuf::from));
        if let Some(d) = dir {
            dialog.set_initial_folder(Some(&gio::File::for_path(d)));
        }
        let me = self.clone();
        glib::spawn_future_local(async move {
            let Ok(f) = dialog.save_future(Some(&me.win)).await else { return then(false) };
            let Some(mut p) = f.path() else { return then(false) };
            if p.extension().is_none() {
                p.set_extension(ext.trim_start_matches('.'));
            }
            then(me.write_to(&p));
        });
    }

    fn write_to(&self, path: &Path) -> bool {
        let rich = self.mode() == "rich";
        let mut path = path.to_path_buf();
        let res = if rich {
            if !files::SAVE_RICH.iter().any(|(e, _)| ext_of(&path) == *e) {
                let mut s = path.into_os_string();
                s.push(".docx");
                path = PathBuf::from(s);
            }
            self.win.set_cursor_from_name(Some("wait"));
            let model = self.rich().document();
            let r = files::save_rich(&model, &path);
            self.win.set_cursor(None::<&gdk::Cursor>);
            if r.is_ok() {
                let mut st = self.st.borrow_mut();
                st.doc.path = Some(path.clone());
                st.doc.source_ext = Some(ext_of(&path));
                st.doc.model = Some(model);
            }
            r
        } else {
            let text = self.plain_text();
            let mut st = self.st.borrow_mut();
            files::save(&mut st.doc, &text, Some(&path))
        };
        if let Err(e) = res {
            self.error("Could not save", &e);
            return false;
        }
        self.st.borrow_mut().dirty = false;
        self.update_title();
        self.flash_msg(&format!("Saved {}", path.file_name().unwrap_or_default().to_string_lossy()), 5000);
        if self.st.borrow().preview_on {
            self.preview().refresh(false); // LaTeX is rebuilt on every save
        }
        true
    }

    pub fn pick_kind(self: &Rc<Self>) {
        if self.mode() != "plain" {
            self.flash_msg("Formatted documents and PDFs keep their format; use Export to convert them.", 5000);
            return;
        }
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Top);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let head = widgets::label("Which parts are protected from changes?", &["muted", "small"]);
        head.set_margin_start(8);
        head.set_margin_end(8);
        head.set_margin_top(4);
        head.set_margin_bottom(4);
        col.append(&head);
        let cur = self.st.borrow().doc.kind.clone();
        let mut group: Option<gtk::CheckButton> = None;
        for (k, label) in files::KIND_LABEL {
            let b = gtk::CheckButton::with_label(label);
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            b.set_active(cur == *k);
            let me = self.weak();
            let k = k.to_string();
            let p = pop.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    if let Some(app) = me.upgrade() {
                        app.set_kind(&k);
                    }
                    p.popdown();
                }
            });
            col.append(&b);
        }
        pop.set_child(Some(&col));
        pop.set_parent(&self.w().status_file);
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    pub fn set_kind(&self, kind: &str) {
        self.st.borrow_mut().doc.kind = kind.to_string();
        self.update_title();
        self.analyze();
        if self.st.borrow().preview_on {
            self.preview().refresh(false);
        }
    }

    /// The plain editor's text (no embedded objects).
    pub fn plain_text(&self) -> String {
        self.buf.text(&self.buf.start_iter(), &self.buf.end_iter(), true).to_string()
    }

    /// The editor's text; in a formatted document every embedded object counts as one U+FFFC, so that
    /// offsets in it match the buffer's positions.
    pub fn editor_text(&self) -> String {
        if self.rich_active() {
            self.rich().plain()
        } else {
            self.buf.slice(&self.buf.start_iter(), &self.buf.end_iter(), true).to_string()
        }
    }

    pub fn refresh_snapshot(&self) -> Rc<Snap> {
        let dirty = self.st.borrow().snap_dirty;
        if dirty {
            let snap = Snap::new(self.editor_text());
            let mut st = self.st.borrow_mut();
            st.snap = snap;
            st.snap_dirty = false;
        }
        self.st.borrow().snap.clone()
    }

    pub fn iter_at(&self, off: usize) -> gtk::TextIter {
        self.buf.iter_at_offset(off as i32)
    }

    pub fn cursor_offset(&self) -> usize {
        self.buf.iter_at_mark(&self.buf.get_insert()).offset() as usize
    }

    /// The selection as character offsets.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.buf.selection_bounds().map(|(a, b)| (a.offset() as usize, b.offset() as usize))
    }

    pub fn error(&self, title: &str, detail: &str) {
        crate::dialogs::message(&self.win, title, detail, &["OK"], 0, 0, |_| {});
    }

    pub fn info(&self, title: &str, detail: &str) {
        self.error(title, detail);
    }

    /// Ask a yes/no question; `then(true)` on yes.
    pub fn ask(&self, title: &str, detail: &str, then: impl FnOnce(bool) + 'static) {
        crate::dialogs::message(&self.win, title, detail, &["No", "Yes"], 1, 0, move |c| then(c == 1));
    }

    pub fn flash_msg(&self, text: &str, ms: u32) {
        let msg = self.w().msg.clone();
        msg.set_label(text);
        msg.set_tooltip_text(if text.is_empty() { None } else { Some(text) });
        workers::later("msg", ms, move || msg.set_label(""));
    }

    pub fn bell(&self) {
        self.win.error_bell();
    }

    // -- preview, view and models --------------------------------------------------------------

    /// Show or hide the preview. While it is open, the suggestions sit below the editor.
    pub fn set_preview(&self, on: bool, save: bool) {
        let w = self.w();
        if on && self.mode() == "pdf" {
            w.t_preview.set(false);
            if let Some(a) = self.action("preview") {
                a.set_state(&false.to_variant());
            }
            self.flash_msg("A PDF is shown as it is; the preview is for text and formatted documents.", 5000);
            return;
        }
        let was = self.st.borrow().preview_on;
        if on != was {
            if on {
                w.pane.set_end_child(None::<&gtk::Widget>);
                w.left.set_end_child(Some(&w.panel_frame));
                w.pane.set_end_child(Some(&self.preview().widget()));
                w.pane.set_resize_end_child(true);
            } else {
                let width = w.pane.width().max(1);
                self.settings_mut().preview_split = ((w.pane.position() as f64 / width as f64) * 1000.0).round() / 1000.0;
                w.pane.set_end_child(None::<&gtk::Widget>);
                w.left.set_end_child(None::<&gtk::Widget>);
                w.pane.set_end_child(Some(&w.panel_frame));
                w.pane.set_resize_end_child(false);
            }
            self.st.borrow_mut().preview_on = on;
            self.panel().relayout();
            let a = self.weak();
            glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                if let Some(app) = a.upgrade() {
                    app.place_sash();
                }
            });
        }
        w.t_preview.set(on);
        if let Some(a) = self.action("preview") {
            a.set_state(&on.to_variant());
        }
        if save {
            self.settings_mut().preview = on;
            self.save_settings();
        }
        if on {
            self.preview().refresh(false);
        } else {
            self.preview().stop();
        }
        self.text.grab_focus();
    }

    pub fn refresh_preview(&self) {
        if self.st.borrow().preview_on {
            self.preview().refresh(true);
        } else {
            self.set_preview(true, true);
        }
    }

    /// Put the cursor on a line of the editor (used by the LaTeX error list).
    pub fn goto_line(&self, line: usize) {
        let last = self.buf.line_count().max(1) as usize;
        let line = line.clamp(1, last) - 1;
        let Some(it) = self.buf.iter_at_line(line as i32) else { return };
        self.buf.place_cursor(&it);
        let mut end = it;
        end.forward_to_line_end();
        self.text.scroll_to_iter(&mut it.clone(), 0.1, false, 0.0, 0.3);
        self.buf.remove_tag(&self.tags.flash, &self.buf.start_iter(), &self.buf.end_iter());
        self.buf.apply_tag(&self.tags.flash, &it, &end);
        let buf = self.buf.clone();
        let tag = self.tags.flash.clone();
        workers::later("flash", 900, move || buf.remove_tag(&tag, &buf.start_iter(), &buf.end_iter()));
        self.text.grab_focus();
        self.update_target(false);
    }

    pub fn reload_models(&self, done: Option<Box<dyn FnOnce()>>) {
        self.st.borrow_mut().models_state = "loading".into();
        self.update_mode();
        let eng = self.engine.clone();
        let me = self.weak();
        self.lanes.slow.submit(
            move || eng.load_models(),
            move |_| {
                let Some(app) = me.upgrade() else { return };
                {
                    let mut st = app.st.borrow_mut();
                    st.models_state = "ready".into();
                    st.cache.clear();
                    st.cache_order.clear();
                }
                crate::analysis::clear_grammar_cache();
                app.update_mode();
                app.analyze();
                if app.st.borrow().target.is_some() {
                    app.update_target(true);
                }
                app.panel().render();
                if let Some(d) = done {
                    d();
                }
            },
        );
    }

    pub fn update_mode(&self) {
        let b = &self.w().mode_btn;
        for c in ["good", "warn", "muted"] {
            b.remove_css_class(c);
        }
        let e = &self.engine;
        if self.st.borrow().models_state == "loading" {
            b.set_label("◌ Loading models…");
            b.add_css_class("muted");
        } else if e.smart() {
            let mut parts = vec!["context"];
            if e.has_meaning_check() {
                parts.push("meaning check");
            }
            if e.has_paraphraser() {
                parts.push("paraphraser");
            }
            b.set_label(if e.has_meaning_check() && e.has_paraphraser() { "● Smart mode" } else { "● Smart mode (partial)" });
            b.add_css_class("good");
            b.set_tooltip_text(Some(&format!("Models loaded: {}\nClick to manage models", parts.join(", "))));
        } else {
            b.set_label("○ Lite mode");
            b.add_css_class("warn");
            b.set_tooltip_text(Some("Only the offline thesaurus is active. Click to download the small models."));
        }
    }

    pub fn open_models(self: &Rc<Self>) {
        crate::dialogs::models_dialog(self, false);
    }

    pub fn set_hints(&self, on: bool) {
        self.settings_mut().phrase_hints = on;
        self.save_settings();
        self.color_editor();
        let me = self.weak();
        workers::later("target", 10, move || {
            if let Some(app) = me.upgrade() {
                app.update_target(true);
            }
        });
    }

    pub fn set_show_changes(&self, on: bool) {
        self.settings_mut().show_changes = on;
        self.save_settings();
        self.color_editor();
    }

    pub fn set_proofing(&self) {
        self.save_settings();
        self.analyze();
        let me = self.weak();
        workers::later("target", 30, move || {
            if let Some(app) = me.upgrade() {
                app.update_target(true);
            }
        });
    }

    pub fn toggle_theme(&self) {
        let name = if self.theme().dark() { "light" } else { "dark" };
        self.theme.borrow_mut().switch(name);
        widgets::set_icon(&self.w().theme_btn, if name == "light" { "moon" } else { "sun" });
        self.color_editor();
        self.update_mode();
        self.theme().changed();
        self.panel().render();
        self.preview().retheme();
        self.rich().retheme();
        self.settings_mut().theme = name.into();
        self.save_settings();
    }

    pub fn zoom(&self, d: i32) {
        let size = if d == 0 { settings::DEFAULT_FONT_SIZE } else { (self.theme().size + d as f64).clamp(8.0, 40.0) };
        self.theme.borrow_mut().size = size;
        self.load_editor_css();
        self.preview().retheme();
        self.rich().retheme();
        self.settings_mut().font_size = size;
        self.save_settings();
    }

    pub fn set_font_family(&self, fam: &str) {
        self.theme.borrow_mut().editor_family = fam.to_string();
        self.load_editor_css();
        self.preview().retheme();
        self.rich().retheme();
        self.settings_mut().font_family = fam.into();
        self.save_settings();
    }

    pub fn undo(&self) {
        if self.mode() == "pdf" {
            self.pdf().undo();
        } else if self.rich_active() {
            self.rich().undo();
        } else if self.buf.can_undo() {
            self.buf.undo();
        }
    }

    pub fn redo(&self) {
        if self.mode() == "pdf" {
            self.pdf().redo();
        } else if self.rich_active() {
            self.rich().redo();
        } else if self.buf.can_redo() {
            self.buf.redo();
        }
    }

    pub fn select_all(&self) {
        if self.mode() == "pdf" {
            return;
        }
        self.buf.select_range(&self.buf.start_iter(), &self.buf.end_iter());
        let me = self.weak();
        workers::later("target", 200, move || {
            if let Some(app) = me.upgrade() {
                app.update_target(false);
            }
        });
    }

    pub fn about(&self) {
        let dir = rewrite_core::paths::data_dir();
        self.info(
            "About Rewrite",
            &format!(
                "Rewrite {VERSION}\n\nA small offline rewriting editor.\nCreated by {AUTHOR}\n{URL}\n\nModels and data are stored in:\n{}",
                dir.display()
            ),
        );
    }

    pub fn quit(self: &Rc<Self>) {
        if self.busy() {
            self.st.borrow().cancel.set();
        }
        let me = self.clone();
        self.confirm_discard(move |ok| {
            if !ok {
                return;
            }
            me.st.borrow_mut().proof_gen += 1;
            me.preview().stop();
            {
                let w = me.w();
                if me.st.borrow().preview_on {
                    let width = w.pane.width().max(1);
                    me.settings_mut().preview_split = ((w.pane.position() as f64 / width as f64) * 1000.0).round() / 1000.0;
                }
                let (ww, hh) = (me.win.width(), me.win.height());
                let maximized = me.win.is_maximized();
                me.settings_mut().maximized = maximized;
                if ww > 0 && hh > 0 && !maximized {
                    me.settings_mut().geometry = format!("{ww}x{hh}");
                }
            }
            me.save_settings();
            me.win.destroy();
            me.gapp.quit();
        });
    }

    // -- formatted documents --------------------------------------------------------------------

    pub fn fmt(&self, f: impl FnOnce(&RichEditor)) {
        if self.rich_active() && !self.busy() {
            f(self.rich());
            self.text.grab_focus();
            self.formatbar.get().unwrap().refresh();
        }
    }

    pub fn rich_insert(self: &Rc<Self>, kind: &str) {
        if !self.rich_active() || self.busy() {
            self.flash_msg(
                "Pictures, tables and the like go into formatted documents (File › New formatted document, or open a Word or OpenDocument file).",
                5000,
            );
            return;
        }
        crate::objects::insert(self, kind);
        self.text.grab_focus();
    }

    pub fn rich_link(self: &Rc<Self>) {
        if !self.rich_active() || self.busy() {
            return;
        }
        crate::objects::link_dialog(self);
    }

    pub fn open_link(&self, url: &str) {
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
            let launcher = gtk::UriLauncher::new(url);
            launcher.launch(Some(&self.win), gio::Cancellable::NONE, |_| {});
        } else {
            self.flash_msg(&format!("Link: {url}"), 5000);
        }
    }

    pub fn page_setup(self: &Rc<Self>) {
        if self.mode() != "rich" {
            self.flash_msg("Page set-up is for formatted documents; for an export, pick the paper in the Export dialog.", 5000);
            return;
        }
        crate::objects::page_setup_dialog(self);
    }

    pub fn export_dialog(self: &Rc<Self>) {
        if self.busy() {
            return;
        }
        let st = self.st.borrow();
        let name = if st.doc.path.is_some() { st.doc.name() } else { st.suggest_name.clone().unwrap_or_else(|| "document".into()) };
        let path = st.doc.path.clone();
        let kind = st.doc.kind.clone();
        let mode = st.mode.clone();
        let encoding = if st.doc.source_ext.is_none() { st.doc.encoding.clone() } else { "utf-8".into() };
        drop(st);
        let src = match mode.as_str() {
            "pdf" => rewrite_core::export::Source { data: Some(self.pdf().data()), ..rewrite_core::export::Source::text("pdf", &name, path, String::new(), "utf-8") },
            "rich" => rewrite_core::export::Source { model: Some(self.rich().document()), ..rewrite_core::export::Source::text("rich", &name, path, String::new(), "utf-8") },
            _ => rewrite_core::export::Source::text(&kind, &name, path, self.plain_text(), &encoding),
        };
        crate::export_dialog::show(self, src);
    }

    // -- PDF ---------------------------------------------------------------------------------------

    pub fn open_pdf(&self, data: Vec<u8>) {
        self.clear_proposals();
        self.set_mode("pdf");
        self.st.borrow_mut().loading = true;
        self.buf.begin_irreversible_action();
        self.buf.set_text("");
        self.buf.end_irreversible_action();
        self.st.borrow_mut().loading = false;
        let path = self.st.borrow().doc.path.clone();
        self.pdf().load(data, path);
        self.fresh(String::new(), None);
        self.pdf_changed(false);
    }

    /// The PDF was changed (or undone): show it in the title and the status bar.
    pub fn pdf_changed(&self, dirty: bool) {
        let pdf = self.pdf();
        {
            let mut st = self.st.borrow_mut();
            st.dirty = dirty && pdf.can_undo();
            st.doc.data = Some(pdf.data());
        }
        self.update_title();
        let n = pdf.page_count();
        self.w().status_stats.set_label(&format!("{n} page{}", if n != 1 { "s" } else { "" }));
    }

    /// Make an editable document of the PDF.
    pub fn convert_pdf(self: &Rc<Self>) {
        if self.mode() != "pdf" || self.busy() {
            return;
        }
        let me = self.clone();
        self.confirm_discard(move |ok| {
            if ok {
                me.run_convert();
            }
        });
    }

    fn run_convert(self: &Rc<Self>) {
        let data = self.pdf().data();
        let name = {
            let n = self.st.borrow().doc.name();
            Path::new(&n).file_stem().map_or(n.clone(), |s| s.to_string_lossy().into_owned())
        };
        let flag = Cancel::new();
        {
            let mut st = self.st.borrow_mut();
            st.busy = true;
            st.cancel = flag.clone();
        }
        self.show_progress("Reading the PDF…");
        let helper = self.engine.helper();
        let flag2 = flag.clone();
        let me = self.weak();
        self.lanes.slow.submit(
            move || {
                let progress = |k: usize, n: usize| {
                    workers::post(move || crate::analysis::with_app(|app| app.set_progress(k, n, &format!("Reading page {} of {n}", k + 1))));
                };
                rewrite_pdfium::to_document(&data, &rewrite_core::pdfextract::Options::default(), &helper, Some(&progress), Some(flag2.flag()))
                    .map_err(|e| e.to_string())
            },
            move |res| {
                let Some(app) = me.upgrade() else { return };
                app.st.borrow_mut().busy = false;
                app.hide_progress();
                match res {
                    Ok(Ok(model)) => {
                        app.st.borrow_mut().doc = Doc { kind: "rich".into(), model: Some(model.clone()), ..Doc::default() };
                        app.set_rich(model);
                        app.st.borrow_mut().dirty = true;
                        app.st.borrow_mut().suggest_name = Some(name);
                        app.update_title();
                        app.flash_msg("Converted. Save it as Word or OpenDocument, or use Export for Markdown, LaTeX or text.", 12000);
                    }
                    Ok(Err(e)) if flag.is_set() => {
                        let _ = e;
                    }
                    Ok(Err(e)) | Err(e) => app.error("Convert PDF", &format!("The PDF could not be converted.\n\n{e}")),
                }
            },
        );
    }

    pub fn show_progress(&self, label: &str) {
        let w = self.w();
        w.msg.set_visible(false);
        w.progress.set_fraction(0.0);
        w.progress_lbl.set_label(label);
        w.progress_lbl.set_visible(true);
        w.progress.set_visible(true);
        w.cancel_btn.set_visible(true);
    }

    pub fn set_progress(&self, k: usize, n: usize, label: &str) {
        if !self.busy() {
            return;
        }
        let w = self.w();
        w.progress.set_fraction(k as f64 / n.max(1) as f64);
        w.progress_lbl.set_label(label);
    }

    pub fn hide_progress(&self) {
        let w = self.w();
        w.progress.set_visible(false);
        w.progress_lbl.set_visible(false);
        w.cancel_btn.set_visible(false);
        w.msg.set_visible(true);
    }

    pub fn cancel_auto(&self) {
        if self.busy() {
            self.st.borrow().cancel.set();
            self.w().progress_lbl.set_label("Stopping…");
        }
    }
}

fn make_tags(buf: &gtk::TextBuffer) -> Tags {
    let t = |name: &str| buf.create_tag(Some(name), &[]).expect("a new tag");
    Tags {
        prot: t("prot"),
        hint: t("hint"),
        chg: t("chg"),
        gram: t("gram"),
        spell: t("spell"),
        prop: t("prop"),
        found: t("found"),
        target: t("target"),
        flash: t("flash"),
    }
}

/// The size of the screen the window opens on (its first monitor), in GTK's units.
fn screen_size() -> (i32, i32) {
    let geo = gdk::Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_then(|m| m.downcast::<gdk::Monitor>().ok())
        .map(|m| m.geometry());
    geo.map_or((1920, 1080), |g| (g.width(), g.height()))
}

fn parse_size(geo: &str) -> Option<(i32, i32)> {
    let size = geo.split(['+', '-']).next()?;
    let (w, h) = size.split_once('x')?;
    let (w, h) = (w.parse::<i32>().ok()?, h.parse::<i32>().ok()?);
    (w >= 400 && h >= 300).then_some((w, h))
}

/// Let GTK draw pending changes (such as a wait cursor) before a long call on the UI thread.
fn flush_events() {
    let ctx = glib::MainContext::default();
    for _ in 0..20 {
        if !ctx.iteration(false) {
            break;
        }
    }
}

pub fn ext_of(p: &Path) -> String {
    p.extension().map_or(String::new(), |e| format!(".{}", e.to_string_lossy().to_lowercase()))
}

pub fn kind_label(kind: &str) -> String {
    files::KIND_LABEL.iter().find(|(k, _)| *k == kind).map_or(kind.to_string(), |(_, l)| l.to_string())
}

pub fn format_label(ext: &str) -> Option<String> {
    files::FORMAT_LABEL.iter().find(|(e, _)| *e == ext).map(|(_, l)| l.to_string())
}

pub fn style_label(s: &str) -> String {
    rewrite_core::model::STYLE_LABEL.iter().find(|(k, _)| *k == s).map_or(s.to_string(), |(_, l)| l.to_string())
}

pub fn filter(label: &str, patterns: &[&str]) -> gtk::FileFilter {
    let f = gtk::FileFilter::new();
    f.set_name(Some(label));
    for p in patterns {
        f.add_pattern(p);
        // the dialogs match case-sensitively on some systems: accept upper-case extensions too
        if p.starts_with("*.") {
            f.add_pattern(&p.to_uppercase());
        }
    }
    f
}

pub fn file_filters(types: &[files::FileType]) -> gio::ListStore {
    let store = gio::ListStore::new::<gtk::FileFilter>();
    for t in types {
        store.append(&filter(t.label, t.patterns));
    }
    store
}
