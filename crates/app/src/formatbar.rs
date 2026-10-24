//! The formatting toolbar shown for formatted documents: paragraph style, font, character formatting,
//! alignment, lists, and things to insert. Icon buttons; each one's tooltip says what it does.

use crate::app::{App, AppRc};
use crate::objects::Menu;
use crate::rich::{highlight, RichEditor};
use crate::theme::MOD;
use crate::widgets::{self, Toggle};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::pango;
use rewrite_core::model::look;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

const STYLES: [&str; 10] = ["normal", "title", "subtitle", "h1", "h2", "h3", "h4", "quote", "caption", "code"];
const SIZES: [f64; 16] = [8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 48.0, 72.0];
const FONTS: [&str; 18] = [
    "Calibri", "Cambria", "Arial", "Helvetica", "Times New Roman", "Georgia", "Garamond", "Segoe UI", "Verdana", "Tahoma",
    "Liberation Serif", "Liberation Sans", "DejaVu Serif", "DejaVu Sans", "Noto Serif", "Noto Sans", "Courier New", "Consolas",
];
const COLORS: [(&str, Option<&str>); 11] = [
    ("Automatic", None),
    ("Black", Some("#000000")),
    ("Dark grey", Some("#595959")),
    ("Red", Some("#C00000")),
    ("Orange", Some("#ED7D31")),
    ("Gold", Some("#BF8F00")),
    ("Green", Some("#00B050")),
    ("Teal", Some("#008080")),
    ("Blue", Some("#0070C0")),
    ("Dark blue", Some("#1F3864")),
    ("Purple", Some("#7030A0")),
];
const HIGHLIGHTS: [(&str, Option<&str>); 8] = [
    ("No highlight", None),
    ("Yellow", Some("yellow")),
    ("Green", Some("green")),
    ("Cyan", Some("cyan")),
    ("Pink", Some("magenta")),
    ("Red", Some("red")),
    ("Blue", Some("blue")),
    ("Grey", Some("lightGray")),
];
const TOOL: i32 = 15;

fn style_label(s: &str) -> String {
    match s {
        "code" => "Code".into(),
        "h4" => "Heading 4".into(),
        _ => rewrite_core::model::style_label(s).to_string(),
    }
}

struct Parts {
    style_btn: gtk::Button,
    font_btn: gtk::Button,
    size_btn: gtk::Button,
    color_btn: gtk::Button,
    hl_btn: gtk::Button,
    toggles: HashMap<&'static str, Toggle>,
    aligns: Vec<(Option<&'static str>, Toggle)>,
    lists: Vec<(&'static str, Toggle)>,
}

pub struct FormatBar {
    app: Weak<App>,
    root: gtk::Box,
    parts: Rc<Parts>,
}

/// Run a formatting command on the editor, then give it the focus back and show the new state.
fn run(a: &Weak<App>, f: impl FnOnce(&RichEditor)) {
    if let Some(app) = a.upgrade() {
        app.fmt(f);
    }
}

fn text_button(text: &str, tip: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = widgets::flat_button(text, "ghost", Some(tip), f);
    b.add_css_class("compact");
    b.add_css_class("small");
    b
}

impl FormatBar {
    pub fn new(app: &AppRc) -> FormatBar {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("bg");
        root.set_margin_start(6);
        root.set_margin_end(6);
        root.set_margin_bottom(3);
        let w = app.weak();

        let a = w.clone();
        let style_btn = text_button("Normal text ▾", "Paragraph style: headings, title, quote, code …", move || {
            if let Some(app) = a.upgrade() {
                style_menu(&app);
            }
        });
        let a = w.clone();
        let font_btn = text_button("Font ▾", "Font", move || {
            if let Some(app) = a.upgrade() {
                font_menu(&app);
            }
        });
        let a = w.clone();
        let size_btn = text_button("11 ▾", "Text size", move || {
            if let Some(app) = a.upgrade() {
                size_menu(&app);
            }
        });
        root.append(&style_btn);
        root.append(&font_btn);
        root.append(&size_btn);
        root.append(&widgets::separator(4));

        let mut toggles = HashMap::new();
        for (name, icon, tip) in [
            ("b", "bold", format!("Bold ({MOD}+B)")),
            ("i", "italic", format!("Italic ({MOD}+I)")),
            ("u", "underline", format!("Underline ({MOD}+U)")),
            ("s", "strike", "Strikethrough".to_string()),
            ("sup", "sup", "Superscript".to_string()),
            ("sub", "sub", "Subscript".to_string()),
        ] {
            let a = w.clone();
            let t = Toggle::new(Some(icon), "", false, &tip, move |_| run(&a, |r| r.toggle(name)));
            root.append(&t.button);
            toggles.insert(name, t);
        }
        let a = w.clone();
        let color = widgets::sized_icon_button("color", TOOL, "Text colour", move || {
            if let Some(app) = a.upgrade() {
                color_menu(&app);
            }
        });
        root.append(&color);
        let a = w.clone();
        let hl = widgets::sized_icon_button("highlight", TOOL, "Highlight", move || {
            if let Some(app) = a.upgrade() {
                highlight_menu(&app);
            }
        });
        root.append(&hl);
        let a = w.clone();
        root.append(&widgets::sized_icon_button("link", TOOL, &format!("Link ({MOD}+K)"), move || {
            if let Some(app) = a.upgrade() {
                app.rich_link();
            }
        }));
        let a = w.clone();
        root.append(&widgets::sized_icon_button("clear-format", TOOL, "Clear formatting of the selection", move || {
            run(&a, |r| r.clear_format())
        }));
        root.append(&widgets::separator(4));

        let mut aligns = vec![];
        for (align, icon, tip) in [
            (None, "align-left", "Align left"),
            (Some("center"), "align-center", "Centre"),
            (Some("right"), "align-right", "Align right"),
            (Some("justify"), "align-justify", "Justify"),
        ] {
            let a = w.clone();
            let t = Toggle::new(Some(icon), "", false, tip, move |_| run(&a, |r| r.set_align(align.map(String::from))));
            root.append(&t.button);
            aligns.push((align, t));
        }
        root.append(&widgets::separator(4));

        let mut lists = vec![];
        for (kind, icon, tip) in [("bullet", "list-bullet", "Bulleted list"), ("number", "list-number", "Numbered list")] {
            let a = w.clone();
            let t = Toggle::new(Some(icon), "", false, tip, move |_| run(&a, |r| r.toggle_list(kind)));
            root.append(&t.button);
            lists.push((kind, t));
        }
        let a = w.clone();
        root.append(&widgets::sized_icon_button("outdent", TOOL, "Decrease indent (Shift+Tab in a list)", move || {
            run(&a, |r| r.change_level(-1))
        }));
        let a = w.clone();
        root.append(&widgets::sized_icon_button("indent", TOOL, "Increase indent (Tab in a list)", move || run(&a, |r| r.change_level(1))));
        root.append(&widgets::separator(4));

        for (kind, icon, tip) in [
            ("image", "image", "Insert a picture"),
            ("table", "table", "Insert a table"),
            ("chart", "chart", "Insert a chart"),
            ("math", "sigma", "Insert a formula"),
            ("note", "footnote", "Insert a footnote"),
            ("rule", "rule", "Insert a horizontal line"),
            ("pagebreak", "page-break", "Insert a page break"),
        ] {
            let a = w.clone();
            root.append(&widgets::sized_icon_button(icon, TOOL, tip, move || {
                if let Some(app) = a.upgrade() {
                    app.rich_insert(kind);
                }
            }));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        root.append(&spacer);
        let a = w.clone();
        root.append(&widgets::sized_icon_button("page-setup", TOOL, "Page set-up: paper size, margins, page numbers", move || {
            if let Some(app) = a.upgrade() {
                app.page_setup();
            }
        }));

        FormatBar { app: w, root, parts: Rc::new(Parts { style_btn, font_btn, size_btn, color_btn: color, hl_btn: hl, toggles, aligns, lists }) }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Show the formatting at the cursor.
    pub fn refresh(&self) {
        let Some(app) = self.app.upgrade() else { return };
        if !app.rich_active() {
            return;
        }
        let rich = app.rich();
        let (fmt, para) = rich.state();
        let p = &self.parts;
        for (name, t) in &p.toggles {
            t.set(rewrite_core_fmt(&fmt, name));
        }
        for (align, t) in &p.aligns {
            t.set(para.align.as_deref() == *align || (align.is_some_and(|a| a == "justify") && para.align.as_deref() == Some("both")));
        }
        for (kind, t) in &p.lists {
            t.set(para.kind.as_deref() == Some(*kind));
        }
        p.style_btn.set_label(&format!("{} ▾", style_label(&para.style)));
        let size = fmt.size.unwrap_or_else(|| (rich.base_size() * look(&para.style).scale * 2.0).round() / 2.0);
        p.size_btn.set_label(&format!("{} ▾", crate::rich::num_g(size)));
        let font = fmt.font.clone().filter(|f| !f.is_empty()).or_else(|| rich.base_font()).unwrap_or_else(|| "Font".into());
        p.font_btn.set_label(&format!("{} ▾", font.chars().take(16).collect::<String>()));
    }
}

fn rewrite_core_fmt(fmt: &rewrite_core::model::Run, name: &str) -> bool {
    crate::rich::fmt_get(fmt, name).is_some()
}

fn bar(app: &AppRc) -> Option<Rc<Parts>> {
    app.formatbar.get().map(|f| f.parts.clone())
}

fn style_menu(app: &AppRc) {
    if !app.rich_active() {
        return;
    }
    let cur = app.rich().state().1.style;
    let m = Menu::new();
    for s in STYLES {
        let a = app.weak();
        m.add_choice(&style_label(s), cur == s, move || run(&a, |r| r.set_style(s)));
    }
    if let Some(p) = bar(app) {
        m.popup_under(&p.style_btn);
    }
}

fn font_menu(app: &AppRc) {
    let fams = crate::rich::families(&app.text);
    let m = Menu::new();
    let a = app.weak();
    m.add("Document font", move || run(&a, |r| r.set_value("font", None)));
    m.sep();
    for f in FONTS {
        if !fams.contains(f) {
            continue;
        }
        let l = widgets::label(f, &[]);
        let mut fd = pango::FontDescription::new();
        fd.set_family(f);
        fd.set_size(10 * pango::SCALE);
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrFontDesc::new(&fd));
        l.set_attributes(Some(&attrs));
        let a = app.weak();
        m.add_widget(&l, move || run(&a, |r| r.set_value("font", Some(f.to_string()))));
    }
    if let Some(p) = bar(app) {
        m.popup_under(&p.font_btn);
    }
}

fn size_menu(app: &AppRc) {
    let m = Menu::new();
    let a = app.weak();
    m.add("Style size", move || run(&a, |r| r.set_value("size", None)));
    m.sep();
    for s in SIZES {
        let a = app.weak();
        m.add(&crate::rich::num_g(s), move || run(&a, |r| r.set_value("size", Some(crate::rich::num_g(s)))));
    }
    if let Some(p) = bar(app) {
        m.popup_under(&p.size_btn);
    }
}

fn colored_label(text: &str, fg: Option<&str>, bg: Option<&str>) -> gtk::Label {
    let l = widgets::label(text, &[]);
    let attrs = pango::AttrList::new();
    let parse = |c: &str| {
        let (r, g, b) = crate::theme::rgb(c);
        (r as u16 * 257, g as u16 * 257, b as u16 * 257)
    };
    if let Some(c) = fg {
        let (r, g, b) = parse(c);
        attrs.insert(pango::AttrColor::new_foreground(r, g, b));
    }
    if let Some(c) = bg {
        let (r, g, b) = parse(c);
        attrs.insert(pango::AttrColor::new_background(r, g, b));
        attrs.insert(pango::AttrColor::new_foreground(0x1E1E, 0x2222, 0x2727));
    }
    l.set_attributes(Some(&attrs));
    l
}

fn color_menu(app: &AppRc) {
    let m = Menu::new();
    for (label, col) in COLORS {
        let a = app.weak();
        let l = colored_label(label, col, None);
        m.add_widget(&l, move || run(&a, |r| r.set_value("color", col.map(String::from))));
    }
    if let Some(p) = bar(app) {
        m.popup_under(&p.color_btn);
    }
}

fn highlight_menu(app: &AppRc) {
    let m = Menu::new();
    for (label, name) in HIGHLIGHTS {
        let a = app.weak();
        let bg = name.map(|n| highlight(n).to_string());
        let l = colored_label(label, None, bg.as_deref());
        m.add_widget(&l, move || run(&a, |r| r.set_value("hl", name.map(String::from))));
    }
    if let Some(p) = bar(app) {
        m.popup_under(&p.hl_btn);
    }
}
