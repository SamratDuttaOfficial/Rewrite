//! Build tasks:
//!
//!     cargo xtask run [file]   build Rewrite and start it from source
//!     cargo xtask runtime      fetch ONNX Runtime and PDFium for this system into runtime/<platform>/
//!     cargo xtask package      build the installer for this system into dist/installers/
//!     cargo xtask selftest     run the release build's self test
//!     cargo xtask thesaurus --wordnet <dir> --wordfreq <file> [--out <file>]
//!     cargo xtask icons [--font <ttf>]   draw the app icon at every size into assets/icons/
//!                              build assets/thesaurus.json.gz from WordNet 3.0 and wordfreq
//!
//! Packaging needs: on Windows MSYS2's UCRT64 GTK and Inno Setup 6; on macOS Homebrew's GTK; on Linux
//! dpkg-deb for the .deb and linuxdeploy (with its GTK plugin) for the AppImage.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

mod icons;
mod thesaurus;

const ORT: &str = "1.30.0";
const PDFIUM: &str = "8076";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("runtime") => runtime().map(|_| ()),
        Some("package") => package(),
        Some("selftest") => build_release().and_then(|e| selftest(&e)),
        Some("thesaurus") => thesaurus::run(&args[1..]),
        Some("icons") => icons::run(&root(), &args[1..]),
        _ => {
            eprintln!("usage: cargo xtask run [file] | runtime | package | selftest | thesaurus --wordnet <dir> --wordfreq <file> [--out <file>]");
            exit(2);
        }
    };
    if let Err(e) = res {
        eprintln!("[xtask] {e}");
        exit(1);
    }
}

fn say(msg: &str) {
    println!("[xtask] {msg}");
}

fn root() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().expect("the repository");
    // Windows gives a \\?\ path, which other programs (Inno Setup) do not take
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => p,
    }
}

fn version() -> String {
    let toml = fs::read_to_string(root().join("crates/app/Cargo.toml")).unwrap_or_default();
    toml.lines()
        .find_map(|l| l.strip_prefix("version = \"").map(|v| v.trim_end_matches('"').to_string()))
        .unwrap_or_else(|| "0.0.0".into())
}

fn platform() -> String {
    let os = match env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

fn sh(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if status.success() { Ok(()) } else { Err(format!("{:?} failed ({status})", cmd.get_program())) }
}

fn output(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

fn run(args: &[String]) -> Result<(), String> {
    runtime()?;
    sh(Command::new("cargo").args(["build", "-p", "rewrite-app"]).current_dir(root()))?;
    let exe = root().join("target/debug").join(exe_name("rewrite"));
    sh(Command::new(exe).args(args))
}

// -- runtime libraries ------------------------------------------------------------------------

/// The ONNX Runtime and PDFium libraries for this system, fetched once into runtime/<platform>/.
fn runtime() -> Result<Vec<PathBuf>, String> {
    let dir = root().join("runtime").join(platform());
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (ort_files, pdfium_file): (Vec<&str>, &str) = match env::consts::OS {
        "windows" => (vec!["onnxruntime.dll", "onnxruntime_providers_shared.dll"], "pdfium.dll"),
        "macos" => (vec!["libonnxruntime.dylib"], "libpdfium.dylib"),
        _ => (vec!["libonnxruntime.so"], "libpdfium.so"),
    };
    let tmp = root().join("target").join("runtime-download");
    if !ort_files.iter().all(|f| dir.join(f).is_file()) {
        let (name, ext) = match platform().as_str() {
            "windows-x64" => ("onnxruntime-win-x64", "zip"),
            "macos-arm64" => ("onnxruntime-osx-arm64", "tgz"),
            "macos-x64" => ("onnxruntime-osx-x86_64", "tgz"),
            "linux-arm64" => ("onnxruntime-linux-aarch64", "tgz"),
            _ => ("onnxruntime-linux-x64", "tgz"),
        };
        let url = format!("https://github.com/microsoft/onnxruntime/releases/download/v{ORT}/{name}-{ORT}.{ext}");
        let lib = unpack(&url, &tmp.join("ort"))?.join(format!("{name}-{ORT}")).join("lib");
        for f in &ort_files {
            // the .so and .dylib are versioned files behind symbolic links: copy the real one
            let src = fs::read_dir(&lib)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(f.trim_end_matches(".so").trim_end_matches(".dylib").trim_end_matches(".dll"))))
                .filter(|p| !p.is_symlink() && p.is_file())
                .max_by_key(|p| p.file_name().map(|n| n.len()))
                .ok_or(format!("{f} is not in the ONNX Runtime download"))?;
            fs::copy(&src, dir.join(f)).map_err(|e| e.to_string())?;
        }
        say(&format!("ONNX Runtime {ORT} is in {}", dir.display()));
    }
    if !dir.join(pdfium_file).is_file() {
        let name = match platform().as_str() {
            "windows-x64" => "pdfium-win-x64",
            "macos-arm64" => "pdfium-mac-arm64",
            "macos-x64" => "pdfium-mac-x64",
            "linux-arm64" => "pdfium-linux-arm64",
            _ => "pdfium-linux-x64",
        };
        let url = format!("https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F{PDFIUM}/{name}.tgz");
        let out = unpack(&url, &tmp.join("pdfium"))?;
        let sub = if cfg!(windows) { "bin" } else { "lib" };
        fs::copy(out.join(sub).join(pdfium_file), dir.join(pdfium_file)).map_err(|e| e.to_string())?;
        for lic in ["LICENSE"] {
            let _ = fs::copy(out.join(lic), dir.join("PDFIUM-LICENSE"));
        }
        say(&format!("PDFium {PDFIUM} is in {}", dir.display()));
    }
    Ok(ort_files.iter().chain([&pdfium_file]).map(|f| dir.join(f)).collect())
}

/// Download an archive and unpack it into a fresh folder.
fn unpack(url: &str, into: &Path) -> Result<PathBuf, String> {
    let _ = fs::remove_dir_all(into);
    fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let archive = into.join(if url.ends_with(".zip") { "download.zip" } else { "download.tgz" });
    say(&format!("downloading {url}"));
    sh(Command::new("curl").args(["-fsSL", "--retry", "3", "-o"]).arg(&archive).arg(url))?;
    sh(Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(into))?;
    Ok(into.to_path_buf())
}

// -- packaging --------------------------------------------------------------------------------

fn build_release() -> Result<PathBuf, String> {
    runtime()?;
    sh(Command::new("cargo").args(["build", "--release", "-p", "rewrite-app"]).current_dir(root()))?;
    Ok(root().join("target/release").join(exe_name("rewrite")))
}

fn selftest(exe: &Path) -> Result<(), String> {
    let log = root().join("target").join("selftest.log");
    let _ = fs::remove_file(&log);
    let status = Command::new(exe).arg("--selftest").arg(&log).status().map_err(|e| e.to_string())?;
    let text = fs::read_to_string(&log).unwrap_or_default();
    say(text.trim());
    if status.success() { Ok(()) } else { Err("the self test failed".into()) }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn package() -> Result<(), String> {
    let exe = build_release()?;
    let build = root().join("build");
    let dist = root().join("dist").join("installers");
    fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    match env::consts::OS {
        "windows" => package_windows(&exe, &build, &dist),
        "macos" => package_macos(&exe, &build, &dist),
        _ => package_linux(&exe, &build, &dist),
    }
}

/// Windows: the program and GTK's DLLs in bin\, GTK's data in share\ and lib\ (GTK finds them next to its
/// DLLs), then an Inno Setup installer.
fn package_windows(exe: &Path, build: &Path, dist: &Path) -> Result<(), String> {
    let stage = build.join("app").join("Rewrite");
    let _ = fs::remove_dir_all(&stage);
    let bin = stage.join("bin");
    fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    fs::copy(exe, bin.join("Rewrite.exe")).map_err(|e| e.to_string())?;
    for lib in runtime()? {
        fs::copy(&lib, bin.join(lib.file_name().unwrap())).map_err(|e| e.to_string())?;
    }
    let msys = PathBuf::from(env::var("MSYS2_UCRT64").unwrap_or_else(|_| "C:/msys64/ucrt64".into()));
    if !msys.join("bin").is_dir() {
        return Err(format!("GTK is not at {} (set MSYS2_UCRT64)", msys.display()));
    }
    // every DLL the program needs, found by following the import tables
    let objdump = msys.join("bin").join("objdump.exe");
    let mut todo = vec![bin.join("Rewrite.exe")];
    let mut seen = BTreeSet::new();
    while let Some(f) = todo.pop() {
        let dump = output(Command::new(&objdump).arg("-p").arg(&f))?;
        for dll in dump.lines().filter_map(|l| l.trim().strip_prefix("DLL Name: ")) {
            let lower = dll.to_lowercase();
            let src = msys.join("bin").join(dll);
            if seen.insert(lower) && src.is_file() {
                let dst = bin.join(dll);
                fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                todo.push(dst);
            }
        }
    }
    // gdk-pixbuf's loaders (and the DLLs they need), GLib's schemas, the icon themes GTK draws with
    let loaders = msys.join("lib/gdk-pixbuf-2.0");
    if loaders.is_dir() {
        copy_dir(&loaders, &stage.join("lib/gdk-pixbuf-2.0"))?;
        for entry in walk(&stage.join("lib/gdk-pixbuf-2.0")) {
            if entry.extension().is_some_and(|e| e == "dll") {
                let dump = output(Command::new(&objdump).arg("-p").arg(&entry))?;
                for dll in dump.lines().filter_map(|l| l.trim().strip_prefix("DLL Name: ")) {
                    let src = msys.join("bin").join(dll);
                    if src.is_file() && !bin.join(dll).exists() {
                        fs::copy(&src, bin.join(dll)).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }
    copy_dir(&msys.join("share/glib-2.0/schemas"), &stage.join("share/glib-2.0/schemas"))?;
    for theme in ["hicolor", "Adwaita"] {
        let src = msys.join("share/icons").join(theme);
        if src.is_dir() {
            copy_dir(&src, &stage.join("share/icons").join(theme))?;
        }
    }
    copy_dir(&root().join("assets"), &stage.join("share/rewrite/assets"))?;
    selftest(&bin.join("Rewrite.exe"))?;
    let iscc = find_iscc().ok_or("Inno Setup 6 is needed: winget install JRSoftware.InnoSetup")?;
    let icon = build.join("rewrite.ico");
    write_ico(&icon)?;
    sh(Command::new(iscc)
        .arg(format!("/DVersion={}", version()))
        .arg(format!("/DSrc={}", stage.display()))
        .arg(format!("/DOutDir={}", dist.display()))
        .arg(format!("/DIcon={}", icon.display()))
        .arg(root().join("packaging/windows/rewrite.iss")))?;
    say(&format!("installer is in {}", dist.display()));
    Ok(())
}

fn symlink(target: &Path, link: &Path) {
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(target, link);
    #[cfg(not(unix))]
    let _ = (target, link);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn find_iscc() -> Option<PathBuf> {
    if let Ok(o) = Command::new("where").arg("ISCC").output() {
        if let Some(l) = String::from_utf8_lossy(&o.stdout).lines().next() {
            if !l.trim().is_empty() {
                return Some(PathBuf::from(l.trim()));
            }
        }
    }
    let mut places = vec![];
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(base) = env::var(var) {
            let base = PathBuf::from(base);
            places.push(base.join("Inno Setup 6/ISCC.exe"));
            places.push(base.join("Programs/Inno Setup 6/ISCC.exe"));
        }
    }
    places.into_iter().find(|p| p.is_file())
}

fn icon_pngs(sizes: &[u32]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    sizes
        .iter()
        .copied()
        .map(|s| {
            fs::read(root().join(format!("assets/icons/hicolor/{s}x{s}/apps/rewrite.png"))).map(|d| (s, d)).map_err(|e| e.to_string())
        })
        .collect()
}

/// A Windows icon holding the PNGs as they are.
fn write_ico(path: &Path) -> Result<(), String> {
    let pngs = icon_pngs(&[16, 20, 24, 32, 40, 48, 64, 256])?;
    let mut ico = vec![0u8, 0, 1, 0];
    ico.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for (size, data) in &pngs {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[s, s, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, d) in &pngs {
        ico.extend_from_slice(d);
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(path, ico).map_err(|e| e.to_string())
}

/// A macOS icon, from 16 to 1024 pixels (Retina screens use the large ones).
fn write_icns(path: &Path) -> Result<(), String> {
    let mut body = vec![];
    // the PNG types of an .icns: 16, 32, 64, 128, 256, 512 and 1024 pixels
    let tags = [b"icp4", b"icp5", b"icp6", b"ic07", b"ic08", b"ic09", b"ic10"];
    for ((_, data), tag) in icon_pngs(&[16, 32, 64, 128, 256, 512, 1024])?.iter().zip(tags) {
        body.extend_from_slice(tag);
        body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    fs::write(path, out).map_err(|e| e.to_string())
}

/// macOS: Rewrite.app with GTK's libraries in Frameworks (their install names pointed there) and its data
/// in Resources, in a .dmg.
fn package_macos(exe: &Path, build: &Path, dist: &Path) -> Result<(), String> {
    let app = build.join("app").join("Rewrite.app");
    let _ = fs::remove_dir_all(&app);
    let contents = app.join("Contents");
    let (macos, frameworks, resources) = (contents.join("MacOS"), contents.join("Frameworks"), contents.join("Resources"));
    for d in [&macos, &frameworks, &resources] {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let main = macos.join("Rewrite");
    fs::copy(exe, &main).map_err(|e| e.to_string())?;
    for lib in runtime()? {
        fs::copy(&lib, frameworks.join(lib.file_name().unwrap())).map_err(|e| e.to_string())?;
    }
    let brew = PathBuf::from(output(Command::new("brew").arg("--prefix")).unwrap_or_else(|_| "/opt/homebrew".into()).trim());
    // copy every non-system dylib the program needs and point the references at Frameworks
    // (file in the bundle, the folder it came from: where its @rpath and @loader_path references point)
    let mut todo = vec![(main.clone(), brew.join("lib"))];
    let mut seen = BTreeSet::new();
    while let Some((f, origin)) = todo.pop() {
        let deps = output(Command::new("otool").arg("-L").arg(&f))?;
        for line in deps.lines().skip(1) {
            let dep = line.trim().split(" (").next().unwrap_or("").to_string();
            if dep.starts_with("/usr/lib") || dep.starts_with("/System") || dep.is_empty() || dep.starts_with("@executable_path") {
                continue;
            }
            let name = Path::new(&dep).file_name().unwrap().to_string_lossy().into_owned();
            // Homebrew's libraries name each other as @rpath/... or @loader_path/...: find the real file
            let source = if dep.starts_with('@') {
                match [origin.join(&name), brew.join("lib").join(&name)].into_iter().find(|p| p.is_file()) {
                    Some(p) => p,
                    None if frameworks.join(&name).is_file() => frameworks.join(&name), // one of ours (runtime libraries)
                    None => return Err(format!("{dep} (needed by {}) is not in {} or Homebrew's lib", f.display(), origin.display())),
                }
            } else {
                PathBuf::from(&dep)
            };
            let _ = Command::new("install_name_tool").arg("-change").arg(&dep).arg(format!("@executable_path/../Frameworks/{name}")).arg(&f).status();
            if seen.insert(name.clone()) {
                let dst = frameworks.join(&name);
                if source != dst {
                    fs::copy(&source, &dst).map_err(|e| format!("{}: {e}", source.display()))?;
                }
                let _ = Command::new("chmod").arg("u+w").arg(&dst).status();
                let _ = Command::new("install_name_tool").arg("-id").arg(format!("@executable_path/../Frameworks/{name}")).arg(&dst).status();
                let real = source.canonicalize().unwrap_or(source.clone());
                todo.push((dst, real.parent().map(Path::to_path_buf).unwrap_or_else(|| brew.join("lib"))));
            }
        }
    }
    copy_dir(&brew.join("share/glib-2.0/schemas"), &resources.join("share/glib-2.0/schemas"))?;
    for theme in ["hicolor", "Adwaita"] {
        let src = brew.join("share/icons").join(theme);
        if src.is_dir() {
            copy_dir(&src, &resources.join("share/icons").join(theme))?;
        }
    }
    copy_dir(&root().join("assets"), &resources.join("assets"))?;
    write_icns(&resources.join("rewrite.icns"))?;
    let v = version();
    fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Rewrite</string>
<key>CFBundleDisplayName</key><string>Rewrite</string>
<key>CFBundleIdentifier</key><string>com.samratdutta.rewrite</string>
<key>CFBundleExecutable</key><string>Rewrite</string>
<key>CFBundleIconFile</key><string>rewrite.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{v}</string>
<key>CFBundleVersion</key><string>{v}</string>
<key>NSHumanReadableCopyright</key><string>© Samrat Dutta</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#
        ),
    )
    .map_err(|e| e.to_string())?;
    // ad-hoc signature: Apple silicon will not run unsigned code
    let _ = Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&app).status();
    selftest(&main)?;
    let dmg_dir = build.join("dmg");
    let _ = fs::remove_dir_all(&dmg_dir);
    fs::create_dir_all(&dmg_dir).map_err(|e| e.to_string())?;
    sh(Command::new("cp").arg("-R").arg(&app).arg(&dmg_dir))?;
    symlink(Path::new("/Applications"), &dmg_dir.join("Applications"));
    let arch = if env::consts::ARCH == "aarch64" { "arm64" } else { "x86_64" };
    let out = dist.join(format!("Rewrite-{v}-macOS-{arch}.dmg"));
    let _ = fs::remove_file(&out);
    sh(Command::new("hdiutil").args(["create", "-volname", "Rewrite", "-format", "UDZO", "-srcfolder"]).arg(&dmg_dir).arg(&out))?;
    say(&format!("installer is in {}", dist.display()));
    Ok(())
}

/// Linux: a .deb that uses the system's GTK, and an AppImage that carries its own (linuxdeploy's GTK plugin).
fn package_linux(exe: &Path, build: &Path, dist: &Path) -> Result<(), String> {
    let v = version();
    let arch = if env::consts::ARCH == "aarch64" { "arm64" } else { "amd64" };
    let libs = runtime()?;
    let desktop = |exec: &str| {
        format!(
            "[Desktop Entry]\nType=Application\nName=Rewrite\nComment=Offline editor that suggests better words, phrases and sentences\nExec={exec} %f\nIcon=rewrite\nTerminal=false\nCategories=Office;TextTools;\nMimeType=text/plain;text/markdown;text/x-tex;application/pdf;application/rtf;application/msword;application/vnd.openxmlformats-officedocument.wordprocessingml.document;application/vnd.oasis.opendocument.text;\nStartupWMClass=com.samratdutta.rewrite\n"
        )
    };
    // .deb
    if Command::new("dpkg-deb").arg("--version").output().is_ok() {
        let deb = build.join("deb");
        let _ = fs::remove_dir_all(&deb);
        let app = deb.join("opt/rewrite");
        fs::create_dir_all(&app).map_err(|e| e.to_string())?;
        fs::copy(exe, app.join("rewrite")).map_err(|e| e.to_string())?;
        for lib in &libs {
            fs::copy(lib, app.join(lib.file_name().unwrap())).map_err(|e| e.to_string())?;
        }
        copy_dir(&root().join("assets"), &app.join("assets"))?;
        fs::create_dir_all(deb.join("usr/share/applications")).map_err(|e| e.to_string())?;
        // named after the app id, which is how Wayland desktops find the window's entry and icon
        fs::write(deb.join("usr/share/applications/com.samratdutta.rewrite.desktop"), desktop("/opt/rewrite/rewrite")).map_err(|e| e.to_string())?;
        for n in icons::SIZES {
            let dir = deb.join(format!("usr/share/icons/hicolor/{n}x{n}/apps"));
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            fs::copy(root().join(format!("assets/icons/hicolor/{n}x{n}/apps/rewrite.png")), dir.join("rewrite.png")).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(deb.join("usr/bin")).map_err(|e| e.to_string())?;
        symlink(Path::new("/opt/rewrite/rewrite"), &deb.join("usr/bin/rewrite"));
        let size: u64 = walk(&deb).iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024;
        fs::create_dir_all(deb.join("DEBIAN")).map_err(|e| e.to_string())?;
        fs::write(
            deb.join("DEBIAN/control"),
            format!(
                "Package: rewrite\nVersion: {v}\nArchitecture: {arch}\nMaintainer: Samrat Dutta <https://github.com/samratduttaofficial>\nInstalled-Size: {size}\nSection: editors\nPriority: optional\nDepends: libc6, libgtk-4-1 (>= 4.12)\nHomepage: https://github.com/samratduttaofficial\nDescription: Offline editor that suggests better words, phrases and sentences\n Suggests context-aware alternatives for words, phrases and whole sentences,\n checks spelling and grammar, and opens Word, OpenDocument, RTF and PDF files.\n Everything runs on your own computer.\n"
            ),
        )
        .map_err(|e| e.to_string())?;
        selftest(&app.join("rewrite"))?;
        sh(Command::new("dpkg-deb").args(["--build", "--root-owner-group"]).arg(&deb).arg(dist.join(format!("rewrite_{v}_{arch}.deb"))))?;
    }
    // AppImage
    let linuxdeploy = env::var("LINUXDEPLOY").unwrap_or_else(|_| "linuxdeploy".into());
    if Command::new(&linuxdeploy).arg("--version").output().is_ok() {
        let appdir = build.join("AppDir");
        let _ = fs::remove_dir_all(&appdir);
        let bin = appdir.join("usr/bin");
        fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
        fs::copy(exe, bin.join("rewrite")).map_err(|e| e.to_string())?;
        for lib in &libs {
            fs::copy(lib, bin.join(lib.file_name().unwrap())).map_err(|e| e.to_string())?;
        }
        copy_dir(&root().join("assets"), &appdir.join("usr/share/rewrite/assets"))?;
        let desk = build.join("com.samratdutta.rewrite.desktop");
        fs::write(&desk, desktop("rewrite")).map_err(|e| e.to_string())?;
        let machine = if env::consts::ARCH == "aarch64" { "aarch64" } else { "x86_64" };
        sh(Command::new(&linuxdeploy)
            .arg("--appdir")
            .arg(&appdir)
            .arg("--plugin")
            .arg("gtk")
            .arg("--desktop-file")
            .arg(&desk)
            // linuxdeploy takes only the usual hicolor sizes (not 40, one of the Windows sizes)
            .args(icons::SIZES.iter().filter(|n| [16, 20, 24, 32, 48, 64, 128, 256, 512].contains(*n)).flat_map(|n| {
                ["--icon-file".into(), root().join(format!("assets/icons/hicolor/{n}x{n}/apps/rewrite.png")).into_os_string()]
            }))
            .arg("--icon-filename")
            .arg("rewrite")
            .arg("--output")
            .arg("appimage")
            .env("ARCH", machine)
            .env("LDAI_OUTPUT", dist.join(format!("Rewrite-{v}-{machine}.AppImage")))
            .env("DEPLOY_GTK_VERSION", "4"))?;
    }
    say(&format!("installers are in {}", dist.display()));
    Ok(())
}
