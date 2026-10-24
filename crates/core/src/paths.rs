//! Where Rewrite keeps its data and finds its runtime libraries.
//!
//! Running from source (the program sits in this repository's `target/`), settings and models live in the
//! repository root, as they did for the Python version. An installed program keeps them in `~/.rewrite`, so
//! an upgrade finds the settings and models of the version before. `REWRITE_DATA_DIR` overrides both.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The repository root when the running program was built from it.
pub fn source_root() -> Option<PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        exe.ancestors()
            .skip(1)
            .find(|d| d.join("Cargo.toml").is_file() && d.join("crates").is_dir() && d.join("assets").is_dir())
            .map(Path::to_path_buf)
    })
    .clone()
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The folder holding `settings.json` and `models/`.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("REWRITE_DATA_DIR") {
        return PathBuf::from(d);
    }
    source_root().filter(|r| writable(r)).unwrap_or_else(|| home().join(".rewrite"))
}

/// The downloaded models: `<data_dir>/models`, or `REWRITE_MODELS_DIR` (tests point it at existing models
/// while keeping their settings apart).
pub fn models_dir() -> PathBuf {
    match std::env::var_os("REWRITE_MODELS_DIR") {
        Some(d) => PathBuf::from(d),
        None => data_dir().join("models"),
    }
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

/// The platform folder name under `runtime/`: windows-x64, macos-arm64, linux-x64 ...
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// A shared library shipped with Rewrite (ONNX Runtime, PDFium): next to the program, in the app bundle's
/// Frameworks or lib folder, or in `runtime/<platform>/` when running from source. `file` is the platform's
/// file name, e.g. "pdfium.dll", "libpdfium.dylib", "libpdfium.so".
pub fn runtime_library(file: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut places = vec![exe_dir.clone(), exe_dir.join("lib"), exe_dir.join("..").join("Frameworks"), exe_dir.join("..").join("lib")];
    if let Some(root) = source_root() {
        places.push(root.join("runtime").join(platform()));
    }
    places.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

/// A data file shipped in `assets/`: next to the program (installed) or in the repository.
pub fn asset(rel: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut places = vec![exe_dir.join("assets"), exe_dir.join("..").join("Resources").join("assets"), exe_dir.join("..").join("share").join("rewrite").join("assets")];
    if let Some(root) = source_root() {
        places.push(root.join("assets"));
    }
    places.into_iter().map(|d| d.join(rel)).find(|p| p.exists())
}
