//! If the program stops on an internal error, the text being edited is written to `recovered.txt` in the
//! data folder; the next start says so. (An error inside a GTK callback cannot be survived, unlike in the
//! earlier Tk version, so the work is saved instead.)

use crate::app::AppRc;
use gtk4::prelude::*;
use rewrite_core::paths;
use std::path::PathBuf;

fn file() -> PathBuf {
    paths::data_dir().join("recovered.txt")
}

/// Install the panic hook; it runs before the process ends.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        // worker threads catch their own panics; only the UI thread loses the window
        if std::thread::current().name() != Some("main") {
            return;
        }
        crate::analysis::with_app(|app| {
            if app.mode() == "pdf" {
                return;
            }
            let text = app.buf.text(&app.buf.start_iter(), &app.buf.end_iter(), true).to_string();
            if text.trim().is_empty() {
                return;
            }
            let name = app.st.try_borrow().map(|st| st.doc.name()).unwrap_or_else(|_| "the document".into());
            let body = format!("Rewrite stopped on an internal error while editing {name}.\nThe text as it was:\n\n{text}");
            let _ = std::fs::write(file(), body);
        });
    }));
}

/// After a start: tell the user about text saved from a previous crash.
pub fn check(app: &AppRc) {
    let f = file();
    if f.is_file() {
        let kept = paths::data_dir().join(format!(
            "recovered-{}.txt",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
        ));
        if std::fs::rename(&f, &kept).is_ok() {
            app.info(
                "Rewrite closed unexpectedly",
                &format!("The text you were editing was saved to\n{}\n\nOpen it with File › Open.", kept.display()),
            );
        }
    }
}
