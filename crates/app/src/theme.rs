//! Colours, fonts and the style sheet, light and dark. The palettes are the ones the app has always had;
//! everything on screen takes its colours from them through CSS classes, so a theme switch is one reload.

use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

pub struct Palette {
    pub name: &'static str,
    pub c: HashMap<&'static str, &'static str>,
    pub badge: HashMap<&'static str, (&'static str, &'static str)>,
}

const LIGHT: &[(&str, &str)] = &[
    ("bg", "#F4F3EF"), ("surface", "#FFFFFF"), ("panel", "#FAF9F6"), ("border", "#E3E0D8"), ("hover", "#F0EEE8"),
    ("text", "#1E2227"), ("muted", "#646A73"), ("faint", "#9A9FA7"),
    ("accent", "#3B5BDB"), ("accent_hover", "#2F4AC0"), ("accent_soft", "#E8EDFF"), ("on_accent", "#FFFFFF"),
    ("changed", "#DDF3E4"), ("hint", "#FFEBD6"), ("hint_line", "#E8590C"), ("protected", "#8C9199"),
    ("target", "#FFF1B8"), ("flash", "#C9D6FF"), ("select", "#CADBFF"), ("proposal", "#D4E1FF"), ("found", "#FFDF8A"),
    ("good", "#2B8A3E"), ("warn", "#E8590C"), ("danger", "#C92A2A"), ("spell", "#E03131"), ("grammar", "#1971C2"),
];
const LIGHT_BADGE: &[(&str, (&str, &str))] = &[
    ("synonym", ("#2B8A3E", "#E3F5E8")), ("related", ("#0B7285", "#E0F4F7")),
    ("in context", ("#6741D9", "#EFEAFF")), ("phrase", ("#C2410C", "#FFEEDF")),
    ("rewrite", ("#1C6FC4", "#E3F0FD")), ("spelling", ("#C92A2A", "#FFE8E8")),
];
const DARK: &[(&str, &str)] = &[
    ("bg", "#15171B"), ("surface", "#1C1F24"), ("panel", "#191C20"), ("border", "#2C3037"), ("hover", "#252930"),
    ("text", "#E4E6E9"), ("muted", "#9CA2AB"), ("faint", "#6A707A"),
    ("accent", "#7D95FF"), ("accent_hover", "#98ABFF"), ("accent_soft", "#262E4A"), ("on_accent", "#0F1220"),
    ("changed", "#1E3B2A"), ("hint", "#3B2B1C"), ("hint_line", "#FF9A57"), ("protected", "#6E747E"),
    ("target", "#4A4219"), ("flash", "#34437A"), ("select", "#2E4170"), ("proposal", "#2B3B6B"), ("found", "#6E5318"),
    ("good", "#69DB7C"), ("warn", "#FFA94D"), ("danger", "#FF6B6B"), ("spell", "#FF6B6B"), ("grammar", "#4DABF7"),
];
const DARK_BADGE: &[(&str, (&str, &str))] = &[
    ("synonym", ("#8CE99A", "#1D3526")), ("related", ("#66D9E8", "#163339")),
    ("in context", ("#B197FC", "#2A2446")), ("phrase", ("#FFA94D", "#3A2A1A")),
    ("rewrite", ("#74C0FC", "#182F44")), ("spelling", ("#FFA8A8", "#3D1F1F")),
];

impl Palette {
    pub fn named(name: &str) -> Palette {
        let (c, b, name) = if name == "dark" { (DARK, DARK_BADGE, "dark") } else { (LIGHT, LIGHT_BADGE, "light") };
        Palette { name, c: c.iter().copied().collect(), badge: b.iter().copied().collect() }
    }
}

/// Font families: the first installed one of each list, as the app chose them before.
pub struct Families {
    pub ui: String,
    pub serif: String,
    pub sans: String,
    pub mono: String,
}

fn first_available(installed: &[String], candidates: &[&str], fallback: &str) -> String {
    candidates.iter().find(|c| installed.iter().any(|f| f == *c)).map_or(fallback, |c| c).to_string()
}

impl Families {
    pub fn detect(widget: &impl IsA<gtk::Widget>) -> Families {
        let installed: Vec<String> =
            widget.pango_context().list_families().iter().map(|f| f.name().to_string()).collect();
        let pick = |c: &[&str], f: &str| first_available(&installed, c, f);
        if cfg!(windows) {
            let ui = pick(&["Segoe UI"], "Sans");
            Families { serif: pick(&["Georgia", "Cambria"], "Times New Roman"), sans: ui.clone(), mono: pick(&["Cascadia Mono", "Consolas"], "Courier New"), ui }
        } else if cfg!(target_os = "macos") {
            let ui = pick(&["SF Pro Text", "Helvetica Neue"], "Helvetica");
            Families { serif: pick(&["Georgia", "Charter", "Iowan Old Style"], "Times"), sans: ui.clone(), mono: pick(&["SF Mono", "Menlo"], "Courier"), ui }
        } else {
            let ui = pick(&["Inter", "Cantarell", "Ubuntu", "Noto Sans", "DejaVu Sans"], "Sans");
            Families {
                serif: pick(&["Noto Serif", "DejaVu Serif", "Liberation Serif"], "Serif"),
                sans: pick(&["Noto Sans", "Ubuntu", "DejaVu Sans"], &ui),
                mono: pick(&["JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono"], "Monospace"),
                ui,
            }
        }
    }

    pub fn get(&self, kind: &str) -> &str {
        match kind {
            "sans" => &self.sans,
            "mono" => &self.mono,
            _ => &self.serif,
        }
    }
}

/// The base size of interface text, in points.
pub const UI_SIZE: f64 = if cfg!(target_os = "macos") { 12.0 } else { 9.0 };

pub struct Theme {
    pub p: Palette,
    pub families: Families,
    /// serif | sans | mono
    pub editor_family: String,
    /// Editor text size in points.
    pub size: f64,
    provider: gtk::CssProvider,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

impl Theme {
    pub fn new(name: &str, editor_family: &str, size: f64, families: Families) -> Theme {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let editor_family = if matches!(editor_family, "serif" | "sans" | "mono") { editor_family } else { "serif" };
        let t = Theme {
            p: Palette::named(name),
            families,
            editor_family: editor_family.to_string(),
            size: size.clamp(8.0, 40.0),
            provider,
            listeners: RefCell::new(Vec::new()),
        };
        t.load_css();
        t
    }

    /// A colour of the palette, e.g. c("accent").
    pub fn c(&self, key: &str) -> &'static str {
        self.p.c.get(key).copied().unwrap_or("#FF00FF")
    }

    pub fn rgba(&self, key: &str) -> gtk::gdk::RGBA {
        rgba(self.c(key))
    }

    pub fn dark(&self) -> bool {
        self.p.name == "dark"
    }

    pub fn editor_font(&self) -> &str {
        self.families.get(&self.editor_family)
    }

    /// Run `f` after every theme switch (widgets that draw their own colours repaint there).
    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    pub fn switch(&mut self, name: &str) {
        self.p = Palette::named(name);
        self.load_css();
    }

    pub fn changed(&self) {
        for f in self.listeners.borrow().iter() {
            f();
        }
    }

    fn load_css(&self) {
        self.provider.load_from_string(&self.css());
    }

    fn css(&self) -> String {
        let c = |k: &str| self.c(k);
        let ui = &self.families.ui;
        let mut s = format!(
            r#"
window, .bg {{ background-color: {bg}; color: {text}; }}
* {{ font-family: "{ui}"; font-size: {size}pt; }}
.surface {{ background-color: {surface}; }}
.panel {{ background-color: {panel}; }}
.muted {{ color: {muted}; }}
.faint {{ color: {faint}; }}
.good {{ color: {good}; }}
.warn {{ color: {warn}; }}
.danger {{ color: {danger}; }}
.accent {{ color: {accent}; }}
.small {{ font-size: {small}pt; }}
.tiny {{ font-size: {small}pt; font-weight: bold; color: {faint}; }}
.title {{ font-size: {title}pt; font-weight: bold; }}
.head {{ font-size: {head}pt; font-weight: bold; }}
.item {{ font-size: {item}pt; }}
.bold {{ font-weight: bold; }}
.strike {{ text-decoration-line: line-through; }}
.sep {{ background-color: {border}; min-width: 1px; min-height: 1px; }}
.hairline {{ background-color: {border}; min-height: 1px; }}
.vline {{ background-color: {border}; min-width: 1px; }}

/* buttons: flat labels that change colour under the pointer */
button.flat-btn {{ background: none; border: none; box-shadow: none; outline: none; color: {text};
                   padding: 4px 9px; min-height: 0; min-width: 0; border-radius: 4px; }}
button.flat-btn:hover {{ background-color: {hover}; }}
button.flat-btn.square {{ padding: 5px; }}
button.flat-btn.compact {{ padding: 2px 6px; }}
button.flat-btn.primary {{ background-color: {accent}; color: {on_accent}; }}
button.flat-btn.primary:hover {{ background-color: {accent_hover}; }}
button.flat-btn.soft {{ background-color: {accent_soft}; color: {accent}; }}
button.flat-btn.soft:hover {{ background-color: {flash}; }}
button.flat-btn.link {{ color: {accent}; padding: 1px 2px; }}
button.flat-btn.link:hover {{ background: none; color: {accent_hover}; }}
button.flat-btn.toggle {{ color: {muted}; }}
button.flat-btn.toggle.on {{ background-color: {accent_soft}; color: {accent}; }}
button.flat-btn.toggle.on:hover {{ background-color: {flash}; }}
button.flat-btn:disabled {{ color: {faint}; background: none; }}
button.flat-btn.good {{ color: {good}; }}
button.flat-btn.warn {{ color: {warn}; }}
button.flat-btn.muted {{ color: {muted}; }}

/* the editor */
textview.editor, textview.editor text {{ background-color: {surface}; color: {text}; caret-color: {text}; }}
textview.editor text selection {{ background-color: {select}; color: {text}; }}
textview.reader, textview.reader text {{ background-color: {surface}; color: {text}; }}

/* dialogs, popovers, tooltips */
.dialog {{ background-color: {surface}; }}
.caption {{ font-size: {small}pt; font-weight: bold; color: {faint}; }}
.card {{ background-color: {surface}; border: 1px solid {border}; border-radius: 3px; padding: 6px 8px; }}
.card.accent-edge {{ border-color: {accent}; }}
.card.grammar-edge {{ border-color: {grammar}; }}
.card.clickable:hover {{ border-color: {accent}; }}
.row {{ border-radius: 0; }}
.row:hover, .row.hover {{ background-color: {hover}; }}
.badge {{ font-size: {small}pt; border-radius: 3px; padding: 0 4px; }}
tooltip {{ background-color: {text}; color: {surface}; border-radius: 3px; }}
tooltip * {{ color: {surface}; font-size: {small}pt; }}
popover contents, popover arrow {{ background-color: {surface}; color: {text}; }}
popover modelbutton:hover {{ background-color: {accent_soft}; }}
menubar {{ background-color: {bg}; padding: 0; }}
menubar > item {{ padding: 2px 8px; }}
menubar > item:hover {{ background-color: {hover}; }}
entry {{ background-color: {surface}; color: {text}; border: 1px solid {border}; border-radius: 3px;
         min-height: 22px; padding: 0 6px; }}
entry:focus-within {{ border-color: {accent}; }}
entry.error {{ border-color: {danger}; }}
paned > separator {{ background-color: {border}; min-width: 4px; min-height: 4px; }}
scrollbar {{ background: none; border: none; }}
scrollbar slider {{ background-color: {border}; min-width: 7px; min-height: 7px; border-radius: 4px; }}
scrollbar slider:hover {{ background-color: {faint}; }}
progressbar trough {{ background-color: {border}; min-height: 4px; }}
progressbar progress {{ background-color: {accent}; min-height: 4px; }}
checkbutton, radiobutton {{ color: {text}; }}
checkbutton:disabled, radiobutton:disabled {{ color: {faint}; }}
check:checked, radio:checked {{ background-color: {accent}; color: {on_accent}; border-color: {accent}; }}
dropdown button, combobox button {{ background-color: {surface}; color: {text}; border: 1px solid {border}; }}
.segment {{ background-color: {border}; padding: 1px; border-radius: 3px; }}
.segment button {{ background-image: none; box-shadow: none; background-color: {surface}; color: {text}; border: none; border-radius: 2px; padding: 3px 12px; }}
.segment button.on {{ background-color: {accent}; color: {on_accent}; }}
.error-list, .error-list text {{ background-color: {panel}; }}
"#,
            bg = c("bg"),
            text = c("text"),
            surface = c("surface"),
            panel = c("panel"),
            muted = c("muted"),
            faint = c("faint"),
            good = c("good"),
            warn = c("warn"),
            danger = c("danger"),
            accent = c("accent"),
            accent_hover = c("accent_hover"),
            accent_soft = c("accent_soft"),
            on_accent = c("on_accent"),
            hover = c("hover"),
            border = c("border"),
            flash = c("flash"),
            select = c("select"),
            grammar = c("grammar"),
            size = UI_SIZE,
            small = UI_SIZE - 1.0,
            title = UI_SIZE + 4.0,
            head = UI_SIZE + 2.0,
            item = UI_SIZE + 1.0,
        );
        s.push_str(&crate::titlebar::css(&|k| self.c(k), UI_SIZE - 1.0));
        for (kind, (fg, bg)) in &self.p.badge {
            s.push_str(&format!(".badge-{} {{ color: {fg}; background-color: {bg}; }}\n", kind.replace(' ', "-")));
        }
        s
    }
}

pub fn rgba(hex: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(hex).unwrap_or(gtk::gdk::RGBA::BLACK)
}

/// (r, g, b) bytes of a #RRGGBB colour.
pub fn rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let v = |k: usize| u8::from_str_radix(h.get(k..k + 2).unwrap_or("00"), 16).unwrap_or(0);
    (v(0), v(2), v(4))
}

/// Text for keyboard shortcuts: Cmd on macOS, Ctrl elsewhere.
pub const MOD: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
pub const KEY_REPLACE: &str = if cfg!(target_os = "macos") { "Cmd+R" } else { "Ctrl+H" };
pub const KEYS_REPLACE: &str = if cfg!(target_os = "macos") { "Cmd+R" } else { "Ctrl+H or Ctrl+R" };
pub const KEY_NEXT: &str = if cfg!(target_os = "macos") { "Cmd+G" } else { "F3" };
pub const KEY_PREV: &str = if cfg!(target_os = "macos") { "Cmd+Shift+G" } else { "Shift+F3" };
