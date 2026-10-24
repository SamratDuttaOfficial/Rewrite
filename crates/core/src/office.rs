//! LibreOffice, when it is installed: used to write Word 97-2003 (.doc) files, which this program does not
//! write itself, and to read the files its own readers refuse (Word 6/95, encrypted ...).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Full path of LibreOffice's soffice program.
pub fn find() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let mut cands: Vec<PathBuf> = vec![];
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                for n in ["soffice", "soffice.exe", "libreoffice"] {
                    cands.push(d.join(n));
                }
            }
        }
        if cfg!(windows) {
            for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
                if let Some(base) = std::env::var_os(var) {
                    let mut b = PathBuf::from(base);
                    if var == "LOCALAPPDATA" {
                        b = b.join("Programs");
                    }
                    cands.push(b.join("LibreOffice").join("program").join("soffice.exe"));
                }
            }
        } else if cfg!(target_os = "macos") {
            cands.push("/Applications/LibreOffice.app/Contents/MacOS/soffice".into());
        } else {
            for p in ["/usr/bin/libreoffice", "/usr/bin/soffice", "/usr/lib/libreoffice/program/soffice", "/opt/libreoffice/program/soffice", "/snap/bin/libreoffice"] {
                cands.push(p.into());
            }
            if let Ok(rd) = std::fs::read_dir("/opt") {
                let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("libreoffice"))).collect();
                dirs.sort();
                dirs.reverse();
                cands.extend(dirs.into_iter().map(|d| d.join("program").join("soffice")));
            }
        }
        cands.into_iter().find(|p| p.is_file())
    }).clone()
}

fn filter(target: &str) -> &str {
    match target {
        "doc" => "doc:MS Word 97",
        "docx" => "docx:MS Word 2007 XML",
        "pdf" => "pdf:writer_pdf_Export",
        "txt" => "txt:Text (encoded):UTF8",
        t => t,
    }
}

/// Starts a helper program quietly: no console window on Windows, and in a process group of its own, so
/// [`kill_tree`] also reaches the programs it starts.
pub(crate) fn spawn_quiet(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// Ends a program started by [`spawn_quiet`] together with everything it started.
pub(crate) fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    let mut killer = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/F", "/T", "/PID", &pid]);
        c
    } else {
        let mut c = Command::new("kill");
        c.args(["-KILL", &format!("-{pid}")]);
        c
    };
    killer.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Ok(mut k) = spawn_quiet(&mut killer) {
        let _ = k.wait();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Convert file bytes of type `src_ext` (".doc") to `target` ("docx", "doc", "odt", "pdf" ...).
/// `cancelled` is asked now and then whether to give up.
pub fn convert(data: &[u8], src_ext: &str, target: &str, timeout: u64, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    let exe = find().ok_or("LibreOffice is not installed.")?;
    let base = std::env::temp_dir();
    let work = base.join(format!("rewrite-office-{}-{}", std::process::id(), Instant::now().elapsed().as_nanos() ^ (data.len() as u128)));
    let profile = base.join("rewrite-libreoffice-profile");
    std::fs::create_dir_all(work.join("out")).map_err(|e| e.to_string())?;
    let result = (|| {
        let src = work.join(format!("document{src_ext}"));
        std::fs::write(&src, data).map_err(|e| e.to_string())?;
        let url = format!("file:///{}", profile.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
        let mut cmd = Command::new(&exe);
        cmd.arg(format!("-env:UserInstallation={url}"))
            .args(["--headless", "--norestore", "--nolockcheck", "--nodefault", "--convert-to", filter(target), "--outdir"])
            .arg(work.join("out"))
            .arg(&src)
            .current_dir(&work)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_quiet(&mut cmd).map_err(|e| format!("LibreOffice could not start: {e}"))?;
        let start = Instant::now();
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            if cancelled() {
                kill_tree(&mut child);
                return Err("cancelled".to_string());
            }
            if start.elapsed() > Duration::from_secs(timeout) {
                kill_tree(&mut child);
                return Err(format!("LibreOffice did not finish within {timeout} seconds."));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let out = work.join("out").join(format!("document.{target}"));
        std::fs::read(&out).map_err(|_| "LibreOffice could not convert the file.".to_string())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

pub fn available() -> bool {
    find().is_some()
}
