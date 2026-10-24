//! Compiling a LaTeX document to PDF for the preview and for export (was `rewrite/latexbuild.py`).
//!
//! The text in the editor is compiled, not the file on disk, so the preview also works for unsaved changes
//! and untitled documents. Auxiliary files go to a private folder in the system's temporary directory;
//! nothing is written next to the user's .tex file. The run happens in the document's own folder, so
//! `\input`, `\includegraphics` and .bib files are found as usual. The private folder is searched first, so
//! the build's own .aux, .bbl and .toc files win over stale copies another editor left next to the document.

use crate::office::{kill_tree, spawn_quiet};
use regex::Regex;
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub const ENGINES: [&str; 3] = ["pdflatex", "xelatex", "lualatex"];
/// The longest one program run may take.
pub const TIMEOUT: Duration = Duration::from_secs(180);

static RERUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Rerun to get|Please rerun LaTeX|rerun LaTeX|Rerun LaTeX|run LaTeX again").unwrap());
/// What BibTeX and Biber say when there is no .bib file.
static NO_DATABASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"I couldn't open database file|ERROR - Cannot find\b").unwrap());

/// The user cancelled the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// One error of a build: the line in the document (None when it is elsewhere or unknown) and the message.
pub type LogError = (Option<u32>, String);

#[derive(Debug, Clone, Default)]
pub struct BuildResult {
    /// The PDF, or None when nothing could be produced.
    pub pdf: Option<Vec<u8>>,
    pub errors: Vec<LogError>,
    pub log: String,
    pub engine: String,
    pub seconds: f64,
    pub warnings: usize,
    /// Why there is no PDF (no compiler, timeout ...).
    pub message: String,
}

/// Where TeX usually lives; programs started from a desktop icon often get a short PATH.
fn extra_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    if cfg!(windows) {
        let mut dirs: Vec<PathBuf> = [
            var("LOCALAPPDATA").map(|d| d.join("Programs")),
            var("ProgramFiles"),
        ]
        .into_iter()
        .flatten()
        .map(|d| d.join("MiKTeX").join("miktex").join("bin").join("x64"))
        .collect();
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let root = PathBuf::from(format!("{drive}\\texlive"));
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut years: Vec<_> = rd.flatten().map(|e| e.file_name()).collect();
            years.sort();
            dirs.extend(years.into_iter().rev().map(|y| root.join(y).join("bin").join("windows")));
        }
        return dirs;
    }
    let home = crate::paths::home();
    let mut dirs: Vec<PathBuf> =
        ["/Library/TeX/texbin", "/opt/homebrew/bin", "/usr/local/bin", "/usr/texbin", "/opt/local/bin"].map(PathBuf::from).into();
    dirs.extend([home.join("bin"), home.join(".local").join("bin"), home.join(".cargo").join("bin")]);
    dirs
}

/// A program on the PATH, the way a shell finds it (on Windows with the PATHEXT endings).
fn which(name: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pathext.split(';').filter(|e| !e.is_empty()).map(str::to_lowercase).collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| exts.iter().map(|x| d.join(format!("{name}{x}"))).find(|p| p.is_file()))
}

/// Full path of a TeX program (looked up once), or None.
pub fn find(name: &str) -> Option<PathBuf> {
    static TOOLS: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> = LazyLock::new(Default::default);
    let mut tools = TOOLS.lock().unwrap_or_else(|e| e.into_inner());
    tools
        .entry(name.to_string())
        .or_insert_with(|| {
            which(name).or_else(|| {
                let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
                extra_dirs().into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
            })
        })
        .clone()
}

/// True if some LaTeX compiler is installed.
pub fn available() -> bool {
    ENGINES.iter().chain(&["tectonic"]).any(|e| find(e).is_some())
}

/// The compiler the document asks for (a "% !TEX program = xelatex" line, or packages that need XeLaTeX),
/// else the first one installed.
pub fn pick_engine(source: &str) -> Option<&'static str> {
    pick_engine_with(source, &|e| find(e).is_some())
}

/// [`pick_engine`] with `installed` telling which compilers there are.
pub fn pick_engine_with(source: &str, installed: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    static PROGRAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*(\w+)").unwrap());
    static COMMENT: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"(?<!\\)%.*").unwrap());
    static XE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\\usepackage(\[[^\]]*\])?\{[^}]*\b(fontspec|polyglossia|xeCJK|unicode-math)\b").unwrap()
    });
    let start: String = source.chars().take(3000).collect();
    let asked = PROGRAM.captures(&start).map(|c| c[1].to_lowercase());
    let want = match ENGINES.iter().find(|e| asked.as_deref() == Some(**e)) {
        Some(e) => *e,
        None => {
            let head = match source.find("\\begin{document}") {
                Some(i) => &source[..i],
                None => &source[..source.char_indices().nth(8000).map_or(source.len(), |(b, _)| b)],
            };
            let head = COMMENT.replace_all(head, "");
            if XE.is_match(&head) { "xelatex" } else { "pdflatex" }
        }
    };
    std::iter::once(want).chain(ENGINES.into_iter().filter(|e| *e != want)).find(|e| installed(e))
}

/// Runs one program to its end, giving it `stdin` if any. Cancelling or running out of time kills it and
/// everything it started.
fn run(cmd: &[PathBuf], cwd: &Path, env: &[(String, String)], cancel: &dyn Fn() -> bool, stdin: Option<&[u8]>) -> Result<(), RunError> {
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..])
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = spawn_quiet(&mut c).map_err(|e| RunError::Failed(e.to_string()))?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(data);
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) => return Err(RunError::Failed(e.to_string())),
        }
        if cancel() {
            kill_tree(&mut child);
            return Err(RunError::Cancelled);
        }
        if start.elapsed() > TIMEOUT {
            kill_tree(&mut child);
            let name = cmd[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(RunError::Failed(format!("{name} did not finish within {} seconds.", TIMEOUT.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

enum RunError {
    Cancelled,
    /// The program could not start or took too long: the message says which.
    Failed(String),
}

/// A text file as UTF-8 (Latin-1 when it is not), or "" when it cannot be read.
fn read(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(raw) => String::from_utf8(raw).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect()),
        Err(_) => String::new(),
    }
}

/// The file name of a path written with either kind of slash.
fn basename(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// The errors in a TeX log and the number of warnings. `main` is the compiled file's name: errors in other
/// files keep their file name in the message and have no line.
pub fn parse_log(log: &str, main: &str) -> (Vec<LogError>, usize) {
    static FLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?):(\d+): (.*)$").unwrap());
    static EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\w+$").unwrap());
    static NEW_MESSAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:! |\S*\.\w+:\d+: )").unwrap());
    static LINE_NO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^l\.(\d+) ").unwrap());
    static CS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[A-Za-z@]+").unwrap());
    static WARNING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(?:LaTeX|Package \S+) Warning:").unwrap());

    // TeX breaks long lines after 79 characters: put them back together. A line that starts a message of
    // its own is never a continuation: Python joined MiKTeX's "missing.tex:3: ==> Fatal error occurred" to
    // the 79-character memory statistics line above it, which made a junk error out of both.
    let mut lines: Vec<String> = vec![];
    let (mut wrapped, mut last_len) = (0usize, 0usize);
    for ln in log.split('\n') {
        if !lines.is_empty() && last_len > wrapped && (last_len - wrapped) % 79 == 0 && !NEW_MESSAGE.is_match(ln) {
            wrapped = last_len;
            last_len += ln.chars().count();
            lines.last_mut().unwrap().push_str(ln);
        } else {
            wrapped = 0;
            last_len = ln.chars().count();
            lines.push(ln.to_string());
        }
    }

    let mut errors: Vec<LogError> = vec![];
    for (i, ln) in lines.iter().enumerate() {
        let after = |n: usize| lines[i + 1..].iter().take(n);
        let (where_, mut msg) = if let Some(m) = FLE.captures(ln).filter(|m| m[1].is_empty() || EXT.is_match(&m[1])) {
            let Ok(line) = m[2].parse::<u32>() else { continue };
            let fname = &m[1];
            let mut msg = m[3].trim().to_string();
            let mine = [main, ""].contains(&basename(fname));
            if !mine {
                msg = format!("{}:{line}: {msg}", basename(fname));
            }
            if msg == "Undefined control sequence." {
                // name the command: it ends the "l.18 ..." line
                let ctx = after(5).find(|x| LINE_NO.is_match(x)).map_or("", |s| s.as_str());
                if let Some(cmd) = CS.find_iter(ctx).last() {
                    msg = format!("Undefined control sequence {}", cmd.as_str());
                }
            }
            (mine.then_some(line), msg)
        } else if let Some(rest) = ln.strip_prefix("! ") {
            let line = after(11).find_map(|x| LINE_NO.captures(x)).and_then(|c| c[1].parse().ok());
            (line, rest.trim().to_string())
        } else {
            continue;
        };
        if let Some(rest) = msg.strip_prefix("LaTeX Error: ") {
            msg = rest.to_string();
        }
        if !errors.is_empty() && (msg == "Emergency stop." || msg == "==> Fatal error occurred, no output PDF file produced!") {
            continue;
        }
        if !errors.iter().any(|(w, m)| *w == where_ && *m == msg) {
            errors.push((where_, msg));
        }
    }
    errors.truncate(30);
    (errors, WARNING.find_iter(log).count())
}

fn sha1_hex(data: &[u8]) -> String {
    Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The private build folder of a document (or of untitled ones).
pub fn build_dir(path: Option<&Path>) -> PathBuf {
    let name = path.map_or("untitled".to_string(), |p| absolute(p).to_string_lossy().into_owned());
    std::env::temp_dir().join("rewrite-preview").join(&sha1_hex(name.as_bytes())[..16])
}

fn path_list(first: &Path, var: &str) -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    format!("{}{sep}{}", first.display(), std::env::var(var).unwrap_or_default())
}

/// Compiles LaTeX `source`, the document at `path` if it has one, written in `encoding` (UTF-8 when the text
/// does not fit it). Runs BibTeX or Biber when the citations or a .bib file changed, and LaTeX again (up to
/// four runs) while the log asks for it. Errors are in the result; only cancelling is an `Err`.
pub fn build(source: &str, path: Option<&Path>, encoding: &str, cancel: &dyn Fn() -> bool) -> Result<BuildResult, Cancelled> {
    let t0 = Instant::now();
    let mut engine = pick_engine(source);
    if engine.is_none() && find("tectonic").is_none() {
        return Ok(BuildResult {
            message: "No LaTeX compiler was found.\n\nInstall MiKTeX (Windows), MacTeX (macOS) or TeX Live (Linux), then press ↻."
                .into(),
            ..Default::default()
        });
    }
    let out = build_dir(path);
    let cwd = path.and_then(|p| absolute(p).parent().map(Path::to_path_buf)).unwrap_or_else(|| out.clone());
    let stem = path.and_then(|p| p.file_stem()).map_or("untitled".into(), |s| s.to_string_lossy().into_owned());
    let job = if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) { stem } else { "preview".into() };
    let tex = out.join(format!("{job}.tex"));
    let data = crate::files::encode(source, encoding, false).unwrap_or_else(|_| source.as_bytes().to_vec());
    let prepared = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&out)?;
        static INCLUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\include\{\s*([^}]+?)/[^}/]+\}").unwrap());
        // \include writes sub/x.aux, so the folder must exist
        for sub in INCLUDE.captures_iter(source).map(|c| c[1].to_string()).collect::<BTreeSet<_>>() {
            if !Path::new(&sub).is_absolute() && !sub.contains("..") {
                std::fs::create_dir_all(out.join(&sub))?;
            }
        }
        std::fs::write(&tex, &data)
    })();
    if let Err(e) = prepared {
        return Ok(BuildResult { message: format!("Could not prepare the build folder:\n{e}"), ..Default::default() });
    }
    let (mut pdf, mut log) = (out.join(format!("{job}.pdf")), out.join(format!("{job}.log")));
    for stale in [&pdf, &log] {
        let _ = std::fs::remove_file(stale);
    }
    let mut env: Vec<(String, String)> =
        [("max_print_line", "1000"), ("error_line", "254"), ("half_error_line", "238")].map(|(k, v)| (k.into(), v.into())).into();

    let compiled = (|| -> Result<(), RunError> {
        let Some(name) = engine else {
            // Tectonic reads the document from standard input and fetches what it needs
            env.push(("TEXINPUTS".into(), path_list(&cwd, "TEXINPUTS")));
            let cmd = [find("tectonic").unwrap(), "--outdir".into(), out.clone(), "--keep-logs".into(), "--chatter".into(), "minimal".into(), "-".into()];
            run(&cmd, &cwd, &env, cancel, Some(&data))?;
            engine = Some("tectonic");
            (pdf, log) = (out.join("texput.pdf"), out.join("texput.log"));
            return Ok(());
        };
        // MiKTeX's pdflatex and xelatex look in the working folder before the output folder, so a stale
        // paper.bbl next to the document would hide the one BibTeX just made.
        env.push(("TEXINPUTS".into(), path_list(&out, "TEXINPUTS")));
        let mut out_arg = std::ffi::OsString::from("-output-directory=");
        out_arg.push(&out);
        let cmd = [
            find(name).unwrap_or_else(|| name.into()),
            "-interaction=nonstopmode".into(),
            "-file-line-error".into(),
            "-synctex=0".into(),
            out_arg.into(),
            format!("-jobname={job}").into(),
            tex.clone(),
        ];
        run(&cmd, &cwd, &env, cancel, None)?;
        let mut text = read(&log);
        let aux = read(&out.join(format!("{job}.aux")));
        let refs = aux
            .split('\n')
            .filter(|l| ["\\citation", "\\bibdata", "\\bibstyle"].iter().any(|p| l.starts_with(p)))
            .collect::<Vec<_>>()
            .join("\n");
        let bcf = out.join(format!("{job}.bcf"));
        let stamp = out.join(format!("{job}.bibstamp"));
        let sig = sha1_hex((refs.clone() + &read(&bcf)).as_bytes());
        let mut runs = 1;
        if (refs.contains("\\bibdata") || bcf.is_file()) && bib_changed(&sig, &stamp, &out, &job, &cwd, source) {
            let mut benv = env.clone();
            benv.push(("BIBINPUTS".into(), path_list(&cwd, "BIBINPUTS")));
            benv.push(("BSTINPUTS".into(), path_list(&cwd, "BSTINPUTS")));
            let tool: Option<Vec<PathBuf>> = match (find("biber"), find("bibtex")) {
                (Some(biber), _) if bcf.is_file() => Some(vec![biber, "--input-directory".into(), cwd.clone(), job.clone().into()]),
                (_, Some(bibtex)) if refs.contains("\\bibdata") => Some(vec![bibtex, job.clone().into()]),
                _ => None,
            };
            if let Some(tool) = tool {
                run(&tool, &out, &benv, cancel, None)?;
                shipped_bbl(path, &out, &job);
            }
            std::fs::write(&stamp, &sig).map_err(|e| RunError::Failed(e.to_string()))?;
            run(&cmd, &cwd, &env, cancel, None)?;
            runs += 1;
            text = read(&log);
        }
        while runs < 4 && RERUN.is_match(&text) {
            // cross-references, table of contents
            run(&cmd, &cwd, &env, cancel, None)?;
            runs += 1;
            text = read(&log);
        }
        Ok(())
    })();
    match compiled {
        Err(RunError::Cancelled) => return Err(Cancelled),
        Err(RunError::Failed(message)) => {
            return Ok(BuildResult {
                message,
                engine: engine.unwrap_or_default().into(),
                seconds: t0.elapsed().as_secs_f64(),
                log: read(&log),
                ..Default::default()
            });
        }
        Ok(()) => {}
    }
    let text = read(&log);
    let (errors, warnings) = parse_log(&text, &format!("{job}.tex"));
    let pdf = std::fs::read(&pdf).ok().filter(|d| !d.is_empty());
    let message = match (&pdf, errors.is_empty()) {
        (Some(_), _) => "",
        (None, false) => "The document did not compile.",
        (None, true) => "The compiler produced no PDF.",
    };
    Ok(BuildResult {
        pdf,
        errors,
        log: text,
        engine: engine.unwrap_or_default().into(),
        seconds: t0.elapsed().as_secs_f64(),
        warnings,
        message: message.into(),
    })
}

/// Uses the .bbl that came with the document when BibTeX/Biber found no .bib to make one from (arXiv sources
/// have a .bbl and no .bib); the empty .bbl in the build folder would hide it.
fn shipped_bbl(path: Option<&Path>, out: &Path, job: &str) {
    let Some(path) = path else { return };
    if !NO_DATABASE.is_match(&read(&out.join(format!("{job}.blg")))) {
        return;
    }
    let shipped = absolute(path).with_extension("bbl");
    if shipped.is_file() {
        let _ = std::fs::copy(&shipped, out.join(format!("{job}.bbl")));
    }
}

/// Whether BibTeX/Biber must run again: the citations or a .bib file changed since the last run.
fn bib_changed(sig: &str, stamp: &Path, out: &Path, job: &str, cwd: &Path, source: &str) -> bool {
    static BIB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\(?:bibliography|addbibresource)(?:\[[^\]]*\])?\{([^}]+)\}").unwrap());
    let bbl = out.join(format!("{job}.bbl"));
    let made = match std::fs::metadata(&bbl).and_then(|m| m.modified()) {
        Ok(t) if bbl.is_file() && read(stamp) == sig => t,
        _ => return true,
    };
    BIB.captures_iter(source).any(|c| {
        c[1].split(',').map(str::trim).any(|name| {
            let f = cwd.join(if name.ends_with(".bib") { name.to_string() } else { format!("{name}.bib") });
            f.is_file() && std::fs::metadata(&f).and_then(|m| m.modified()).is_ok_and(|t| t > made)
        })
    })
}
