//! Small widgets that look the same everywhere: flat buttons, icon buttons, toggles, the fit bar.
//! Icons are symbolic paintables, so they take the colour of the text around them from the style sheet:
//! hover, disabled and theme colours need no code.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct IconPaintable {
        pub name: RefCell<String>,
        pub size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconPaintable {
        const NAME: &'static str = "RewriteIcon";
        type Type = super::IconPaintable;
        type Interfaces = (gdk::Paintable, gtk::SymbolicPaintable);
    }

    impl ObjectImpl for IconPaintable {}

    impl PaintableImpl for IconPaintable {
        fn intrinsic_width(&self) -> i32 {
            self.size.get()
        }
        fn intrinsic_height(&self) -> i32 {
            self.size.get()
        }
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            self.snapshot_symbolic(snapshot, width, height, &[gdk::RGBA::BLACK]);
        }
    }

    impl SymbolicPaintableImpl for IconPaintable {
        fn snapshot_symbolic(&self, snapshot: &gdk::Snapshot, width: f64, height: f64, colors: &[gdk::RGBA]) {
            let c = colors.first().copied().unwrap_or(gdk::RGBA::BLACK);
            let hex = format!(
                "#{:02X}{:02X}{:02X}",
                (c.red() * 255.0).round() as u8,
                (c.green() * 255.0).round() as u8,
                (c.blue() * 255.0).round() as u8
            );
            // drawn at twice the size: sharp on scaled screens, and smooth when shown at 1x
            let px = ((width.max(height) * 2.0).ceil() as usize).max(8);
            let tex = crate::icons::texture(&self.name.borrow(), &hex, px);
            let snap = snapshot.downcast_ref::<gtk::Snapshot>().expect("a GTK snapshot");
            if c.alpha() < 1.0 {
                snap.push_opacity(c.alpha() as f64);
            }
            snap.append_texture(&tex, &graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
            if c.alpha() < 1.0 {
                snap.pop();
            }
        }
    }
}

glib::wrapper! {
    pub struct IconPaintable(ObjectSubclass<imp::IconPaintable>) @implements gdk::Paintable, gtk::SymbolicPaintable;
}

impl IconPaintable {
    pub fn new(name: &str, size: i32) -> IconPaintable {
        let p: IconPaintable = glib::Object::new();
        p.imp().name.replace(name.to_string());
        p.imp().size.set(size);
        p
    }

    pub fn set_name(&self, name: &str) {
        self.imp().name.replace(name.to_string());
        self.invalidate_contents();
    }
}

/// An icon image that takes the text colour of its widget.
pub fn icon(name: &str, size: i32) -> gtk::Image {
    let img = gtk::Image::from_paintable(Some(&IconPaintable::new(name, size)));
    img.set_pixel_size(size);
    img
}

/// Change the icon shown by a button made with [`icon_button`] or [`toggle`].
pub fn set_icon(button: &impl IsA<gtk::Widget>, name: &str) {
    if let Some(img) = find_image(button.upcast_ref()) {
        if let Some(p) = img.paintable().and_then(|p| p.downcast::<IconPaintable>().ok()) {
            p.set_name(name);
            img.queue_draw();
        }
    }
}

fn find_image(w: &gtk::Widget) -> Option<gtk::Image> {
    if let Some(img) = w.downcast_ref::<gtk::Image>() {
        return Some(img.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(i) = find_image(&c) {
            return Some(i);
        }
        child = c.next_sibling();
    }
    None
}

/// kind: ghost (default), primary, soft, link.
pub fn flat_button(text: &str, kind: &str, tip: Option<&str>, on_click: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("flat-btn");
    if kind != "ghost" && !kind.is_empty() {
        b.add_css_class(kind);
    }
    if let Some(t) = tip {
        b.set_tooltip_text(Some(t));
    }
    b.set_cursor_from_name(Some("pointer"));
    b.set_focus_on_click(false);
    b.connect_clicked(move |_| on_click());
    b
}

/// A button with an icon before its text.
pub fn icon_text_button(icon_name: &str, text: &str, kind: &str, tip: Option<&str>, on_click: impl Fn() + 'static) -> gtk::Button {
    let b = flat_button("", kind, tip, on_click);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    row.append(&icon(icon_name, 16));
    row.append(&gtk::Label::new(Some(text)));
    b.set_child(Some(&row));
    b
}

/// A button that is only a picture; its tooltip says what it does.
pub fn icon_button(icon_name: &str, tip: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    sized_icon_button(icon_name, 16, tip, on_click)
}

pub fn sized_icon_button(icon_name: &str, size: i32, tip: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    let b = flat_button("", "ghost", Some(tip), on_click);
    b.add_css_class("square");
    b.set_child(Some(&icon(icon_name, size)));
    b
}

/// A button that stays lit while its setting is on. `set` changes it without calling `on_change`.
#[derive(Clone)]
pub struct Toggle {
    pub button: gtk::Button,
    value: Rc<Cell<bool>>,
}

impl Toggle {
    pub fn new(icon_name: Option<&str>, text: &str, value: bool, tip: &str, on_change: impl Fn(bool) + 'static) -> Toggle {
        let b = gtk::Button::new();
        b.add_css_class("flat-btn");
        b.add_css_class("toggle");
        b.add_css_class("square");
        b.set_focus_on_click(false);
        b.set_cursor_from_name(Some("pointer"));
        b.set_tooltip_text(Some(tip));
        match icon_name {
            Some(n) => b.set_child(Some(&icon(n, 16))),
            None => b.set_label(text),
        }
        let t = Toggle { button: b.clone(), value: Rc::new(Cell::new(false)) };
        t.set(value);
        let t2 = t.clone();
        b.connect_clicked(move |_| {
            let v = !t2.get();
            t2.set(v);
            on_change(v);
        });
        t
    }

    pub fn get(&self) -> bool {
        self.value.get()
    }

    pub fn set(&self, v: bool) {
        self.value.set(v);
        if v {
            self.button.add_css_class("on");
        } else {
            self.button.remove_css_class("on");
        }
    }
}

/// A thin vertical line between groups of toolbar buttons.
pub fn separator(margin: i32) -> gtk::Widget {
    let s = gtk::Box::new(gtk::Orientation::Vertical, 0);
    s.add_css_class("vline");
    s.set_size_request(1, 16);
    s.set_valign(gtk::Align::Center);
    s.set_margin_start(margin);
    s.set_margin_end(margin);
    s.upcast()
}

/// A horizontal hairline.
pub fn hairline() -> gtk::Widget {
    let s = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    s.add_css_class("hairline");
    s.set_size_request(-1, 1);
    s.upcast()
}

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A label that wraps to its width.
pub fn wrap_label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = label(text, classes);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

/// How well a suggestion fits, as a short bar: green from 0.66, accent from 0.4, faint below.
pub fn fit_bar(theme: &crate::theme::Theme, fit: f64) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(28);
    d.set_content_height(4);
    d.set_valign(gtk::Align::Center);
    let track = theme.rgba("border");
    let fill = theme.rgba(if fit >= 0.66 { "good" } else if fit >= 0.4 { "accent" } else { "faint" });
    d.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgba(track.red() as f64, track.green() as f64, track.blue() as f64, 1.0);
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
        cr.set_source_rgba(fill.red() as f64, fill.green() as f64, fill.blue() as f64, 1.0);
        cr.rectangle(0.0, 0.0, (w as f64 * fit.clamp(0.0, 1.0)).max(2.0), h as f64);
        let _ = cr.fill();
    });
    d
}

/// Remove every child of a box.
pub fn clear(container: &gtk::Box) {
    while let Some(c) = container.first_child() {
        container.remove(&c);
    }
}
