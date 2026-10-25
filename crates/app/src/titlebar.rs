//! The windows' own title bar: compact and in the app's colours, instead of GTK's default one.
//!
//! The main window's bar holds the app icon, the menus (on macOS they are in the system menu bar instead),
//! the title in small print and the window buttons; a dialog's bar holds its title and a close button. Each
//! system gets the buttons its users expect: Windows-style buttons at the right on Windows, the three
//! coloured circles at the left on macOS, and on Linux GTK's own controls, which follow the desktop's
//! button layout and theme.

use crate::widgets;
use gtk4 as gtk;
use gtk4::prelude::*;

/// Which window buttons a bar has.
#[derive(Clone, Copy, PartialEq)]
pub enum Buttons {
    /// Minimise, maximise and close (the main window).
    All,
    /// Close only (dialogs).
    Close,
}

/// Give `win` the title bar; `start` (the menus) goes after the icon. Returns the title label, for
/// windows whose title changes.
pub fn install(win: &impl IsA<gtk::Window>, title: &str, start: Option<&gtk::Widget>, icon: bool, buttons: Buttons) -> gtk::Label {
    let win: &gtk::Window = win.upcast_ref();
    win.set_title(Some(title));
    let bar = gtk::CenterBox::new();
    bar.add_css_class("rw-titlebar");
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    left.set_valign(gtk::Align::Center);
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    right.set_valign(gtk::Align::Fill);

    if cfg!(target_os = "macos") {
        left.append(&mac_buttons(win, buttons));
    } else if cfg!(target_os = "linux") {
        left.append(&gtk::WindowControls::new(gtk::PackType::Start));
        right.append(&gtk::WindowControls::new(gtk::PackType::End));
    }
    if icon && !cfg!(target_os = "macos") {
        let img = gtk::Image::from_icon_name("rewrite");
        img.set_pixel_size(20);
        img.set_margin_start(9);
        img.set_margin_end(4);
        left.append(&img);
    }
    if let Some(s) = start {
        left.append(s);
    }
    if !icon && start.is_none() {
        left.set_margin_start(8);
    }
    let label = gtk::Label::new(Some(title));
    label.add_css_class("rw-title");
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_max_width_chars(60);
    if cfg!(windows) {
        right.append(&windows_buttons(win, buttons));
    }
    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&label));
    bar.set_end_widget(Some(&right));
    // the bar moves the window, and a double-click on it maximises (WindowHandle does both)
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&bar));
    win.set_titlebar(Some(&handle));
    label
}

/// Minimise, maximise / restore and close, flat, at the right; close turns red under the pointer.
fn windows_buttons(win: &gtk::Window, buttons: Buttons) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let button = |icon: &str, tip: &str, extra: &str| {
        let b = gtk::Button::new();
        b.add_css_class("rw-winbtn");
        if !extra.is_empty() {
            b.add_css_class(extra);
        }
        b.set_child(Some(&widgets::icon(icon, 14)));
        b.set_tooltip_text(Some(tip));
        b.set_focus_on_click(false);
        b.set_can_focus(false);
        b
    };
    if buttons == Buttons::All {
        let min = button("win-min", "Minimise", "");
        let w = win.clone();
        min.connect_clicked(move |_| w.minimize());
        row.append(&min);
        let max = button(if win.is_maximized() { "win-restore" } else { "win-max" }, "Maximise", "");
        let w = win.clone();
        max.connect_clicked(move |_| toggle_maximized(&w));
        let m = max.clone();
        win.connect_maximized_notify(move |w| {
            widgets::set_icon(&m, if w.is_maximized() { "win-restore" } else { "win-max" });
            m.set_tooltip_text(Some(if w.is_maximized() { "Restore" } else { "Maximise" }));
        });
        row.append(&max);
    }
    let close = button("win-close", "Close", "close");
    let w = win.clone();
    close.connect_clicked(move |_| w.close());
    row.append(&close);
    row
}

/// The three coloured circles of macOS: close, minimise, zoom.
fn mac_buttons(win: &gtk::Window, buttons: Buttons) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_start(10);
    row.set_margin_end(10);
    row.add_css_class("rw-mac-buttons");
    let dot = |class: &str, tip: &str| {
        let b = gtk::Button::new();
        b.add_css_class("rw-mac");
        b.add_css_class(class);
        b.set_tooltip_text(Some(tip));
        b.set_focus_on_click(false);
        b.set_can_focus(false);
        b.set_valign(gtk::Align::Center);
        b
    };
    let close = dot("close", "Close");
    let w = win.clone();
    close.connect_clicked(move |_| w.close());
    row.append(&close);
    let min = dot("min", "Minimise");
    let max = dot("max", "Zoom");
    if buttons == Buttons::All {
        let w = win.clone();
        min.connect_clicked(move |_| w.minimize());
        let w = win.clone();
        max.connect_clicked(move |_| toggle_maximized(&w));
    } else {
        // dialogs keep the three circles, as macOS does, with the other two disabled
        min.set_sensitive(false);
        max.set_sensitive(false);
    }
    row.append(&min);
    row.append(&max);
    row
}

fn toggle_maximized(w: &gtk::Window) {
    if w.is_maximized() {
        w.unmaximize();
    } else {
        w.maximize();
    }
}

/// The style of the bars, with the theme's colours.
pub fn css(c: &dyn Fn(&str) -> &'static str, small: f64) -> String {
    format!(
        r#"
.rw-titlebar {{ background-color: {bg}; min-height: 30px; padding: 0; border-bottom: 1px solid {border}; }}
.rw-title {{ font-size: {small}pt; color: {muted}; }}
.rw-titlebar menubar {{ background: none; border: none; box-shadow: none; }}
.rw-titlebar menubar > item {{ padding: 4px 8px; min-height: 0; }}
button.rw-winbtn {{ background: none; border: none; box-shadow: none; border-radius: 0; min-width: 44px;
                    min-height: 30px; padding: 0; color: {text}; }}
button.rw-winbtn:hover {{ background-color: {hover}; }}
button.rw-winbtn:active {{ background-color: {border}; }}
button.rw-winbtn.close:hover {{ background-color: #E81123; color: #FFFFFF; }}
button.rw-winbtn.close:active {{ background-color: #F1707A; color: #FFFFFF; }}
button.rw-mac {{ min-width: 12px; min-height: 12px; padding: 0; border-radius: 6px; border: none; box-shadow: none;
                 background-image: none; }}
button.rw-mac.close {{ background-color: #FF5F57; }}
button.rw-mac.min {{ background-color: #FEBC2E; }}
button.rw-mac.max {{ background-color: #28C840; }}
button.rw-mac:disabled {{ background-color: {border}; }}
windowcontrols button {{ min-width: 24px; min-height: 24px; padding: 0; margin: 0 2px; }}
windowcontrols {{ margin: 0 4px; }}
"#,
        bg = c("bg"),
        border = c("border"),
        muted = c("muted"),
        text = c("text"),
        hover = c("hover"),
    )
}
