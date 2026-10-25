//! Rewrite: an offline editor that suggests better words, phrases and sentences.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod app;
mod dialogs;
mod engine;
mod export_dialog;
mod findbar;
mod formatbar;
mod icons;
mod objects;
mod panel;
mod pdfview;
mod preview;
mod recovery;
mod rich;
mod selftest;
mod settings;
mod theme;
mod titlebar;
mod widgets;
mod workers;

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// In the macOS app bundle, GTK's data sits in Contents/Resources/share: tell GLib and GTK before they start.
fn bundle_env() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return };
    let share = dir.join("..").join("Resources").join("share");
    if share.is_dir() {
        let mut data = share.to_string_lossy().into_owned();
        if let Ok(old) = std::env::var("XDG_DATA_DIRS") {
            data = format!("{data}:{old}");
        }
        // SAFETY: called first in main, before any other thread exists
        unsafe {
            std::env::set_var("XDG_DATA_DIRS", data);
            std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0").join("schemas"));
        }
    }
}

fn main() -> glib::ExitCode {
    bundle_env();
    recovery::install();
    let args: Vec<String> = std::env::args().collect();
    glib::set_application_name("Rewrite");
    if cfg!(target_os = "linux") {
        // the X11 window class, so desktops match the window with com.samratdutta.rewrite.desktop
        glib::set_prgname(Some("com.samratdutta.rewrite"));
    }
    if args.get(1).map(String::as_str) == Some("--selftest") {
        return glib::ExitCode::from(selftest::run(args.get(2).map(PathBuf::from)) as u8);
    }
    // macOS Finder may pass a -psn_ process id argument
    let file = args.iter().skip(1).find(|a| !a.starts_with("-psn_") && !a.starts_with("--")).map(PathBuf::from);
    let gapp = gtk::Application::builder()
        .application_id("com.samratdutta.rewrite")
        .flags(gio::ApplicationFlags::NON_UNIQUE | gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    let window: Rc<RefCell<Option<app::AppRc>>> = Rc::new(RefCell::new(None));
    let w = window.clone();
    gapp.connect_activate(move |gapp| start(gapp, &w, file.clone()));
    let w = window.clone();
    gapp.connect_open(move |gapp, files, _| {
        let path = files.first().and_then(|f| f.path());
        match w.borrow().as_ref() {
            Some(app) => {
                if let Some(p) = path.clone() {
                    app.open_file(Some(p));
                }
            }
            None => {}
        }
        if w.borrow().is_none() {
            start(gapp, &w, path);
        }
    });
    // GTK would treat the file argument as its own; the window opens it instead
    gapp.run_with_args(&args[..1])
}

fn start(gapp: &gtk::Application, slot: &Rc<RefCell<Option<app::AppRc>>>, file: Option<PathBuf>) {
    if let Some(app) = slot.borrow().as_ref() {
        app.win.present();
        return;
    }
    let app = app::App::new(gapp);
    app.win.present();
    let show_preview = app.settings().preview;
    if show_preview {
        app.set_preview(true, false);
    }
    match file {
        Some(p) => {
            let a = app.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || a.open_file(Some(p)));
        }
        None => app.set_text("", None),
    }
    app.reload_models(None);
    recovery::check(&app);
    let first_run = !app.settings().first_run_done && !rewrite_nlp::download::MODELS.iter().all(|m| rewrite_nlp::download::present(m.key));
    if first_run {
        let a = app.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || dialogs::models_dialog(&a, true));
    }
    *slot.borrow_mut() = Some(app);
}
