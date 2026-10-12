"""Compile a LaTeX document to PDF for the preview panel.

The text in the editor is compiled, not the file on disk, so the preview also
works for unsaved changes and untitled documents. Auxiliary files go to a
private folder in the system's temporary directory; nothing is written next
to the user's .tex file. The run happens in the document's own folder, so
\\input, \\includegraphics and .bib files are found as usual.
"""
import hashlib
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ENGINES = ("pdflatex", "xelatex", "lualatex")
TIMEOUT = 180  # seconds per program run
_RERUN = re.compile(r"Rerun to get|Please rerun LaTeX|rerun LaTeX|Rerun LaTeX|run LaTeX again", re.I)
_tools = {}


class Cancelled(Exception):
    pass


class Result:
    def __init__(self, pdf=None, errors=(), log="", engine="", seconds=0.0, warnings=0, message=""):
        self.pdf = pdf  # the PDF as bytes, or None when nothing could be produced
        self.errors = list(errors)  # [(line number in the document or None, message)]
        self.log, self.engine, self.seconds, self.warnings = log, engine, seconds, warnings
        self.message = message  # why there is no PDF (no compiler, timeout, ...)


def _extra_dirs():
    """Where TeX usually lives; apps started from a desktop icon often get a short PATH."""
    home = os.path.expanduser("~")
    if sys.platform == "win32":
        dirs = [os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs", "MiKTeX", "miktex", "bin", "x64"),
                os.path.join(os.environ.get("ProgramFiles", ""), "MiKTeX", "miktex", "bin", "x64")]
        for root in (os.environ.get("SystemDrive", "C:") + "\\texlive",):
            if os.path.isdir(root):
                dirs += [os.path.join(root, y, "bin", "windows") for y in sorted(os.listdir(root), reverse=True)]
        return dirs
    return ["/Library/TeX/texbin", "/opt/homebrew/bin", "/usr/local/bin", "/usr/texbin", "/opt/local/bin",
            os.path.join(home, "bin"), os.path.join(home, ".local", "bin"), os.path.join(home, ".cargo", "bin")]


def find(name):
    """Full path of a TeX program, or None."""
    if name not in _tools:
        path = shutil.which(name)
        if not path:
            exts = (".exe",) if sys.platform == "win32" else ("",)
            path = next((os.path.join(d, name + x) for d in _extra_dirs() for x in exts
                         if d and os.path.isfile(os.path.join(d, name + x))), None)
        _tools[name] = path
    return _tools[name]


def available():
    """True if some LaTeX compiler is installed."""
    return any(find(e) for e in ENGINES + ("tectonic",))


def pick_engine(source):
    """The compiler the document asks for: a "% !TEX program = xelatex" line, or packages that need one."""
    m = re.search(r"^%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*(\w+)", source[:3000], re.I | re.M)
    want = m.group(1).lower() if m else None
    if want not in ENGINES:
        head = source[:source.find("\\begin{document}")] if "\\begin{document}" in source else source[:8000]
        head = re.sub(r"(?<!\\)%.*", "", head)
        want = "xelatex" if re.search(r"\\usepackage(\[[^\]]*\])?\{[^}]*\b(fontspec|polyglossia|xeCJK|"
                                      r"unicode-math)\b", head) else "pdflatex"
    order = [want] + [e for e in ENGINES if e != want]
    return next((e for e in order if find(e)), None)


def _popen(cmd, cwd, env, stdin=None):
    kw = {}
    if sys.platform == "win32":
        kw["creationflags"] = subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kw["start_new_session"] = True
    return subprocess.Popen(cmd, cwd=cwd, env=env, stdin=stdin or subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, **kw)


def _kill(proc):
    try:
        if sys.platform == "win32":
            subprocess.call(["taskkill", "/F", "/T", "/PID", str(proc.pid)], stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        else:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass


def _run(cmd, cwd, env, cancel, stdin_data=None):
    """Run one program to the end. Raises Cancelled or TimeoutError."""
    proc = _popen(cmd, cwd, env, subprocess.PIPE if stdin_data is not None else None)
    if stdin_data is not None:
        try:
            proc.stdin.write(stdin_data)
            proc.stdin.close()
        except OSError:
            pass
    start = time.time()
    while proc.poll() is None:
        if cancel is not None and cancel.is_set():
            _kill(proc)
            raise Cancelled()
        if time.time() - start > TIMEOUT:
            _kill(proc)
            raise TimeoutError(f"{os.path.basename(cmd[0])} did not finish within {TIMEOUT} seconds.")
        time.sleep(0.03)
    return proc.returncode


def _read(path):
    try:
        with open(path, "rb") as f:
            raw = f.read()
    except OSError:
        return ""
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("latin-1")


def parse_log(log, main):
    """(errors [(line or None, message)], number of warnings). `main` is the compiled file's name."""
    errors, seen = [], set()
    lines = []
    for ln in log.split("\n"):  # TeX breaks long lines after 79 characters: put them back together
        if lines and (len(lines[-1]) - wrapped) % 79 == 0 and len(lines[-1]) > wrapped:
            wrapped = len(lines[-1])
            lines[-1] += ln
        else:
            wrapped = 0
            lines.append(ln)
    for i, ln in enumerate(lines):
        m = re.match(r"^(.*?):(\d+): (.*)$", ln)  # -file-line-error style
        if m and (m.group(1) == "" or re.search(r"\.\w+$", m.group(1))):
            fname, line, msg = m.group(1), int(m.group(2)), m.group(3).strip()
            where = line if os.path.basename(fname.replace("\\", "/")) in (main, "") else None
            if where is None:
                msg = f"{os.path.basename(fname)}:{line}: {msg}"
            if msg == "Undefined control sequence.":  # name the command: it ends the "l.18 ..." line
                ctx = next((x for x in lines[i + 1:i + 6] if re.match(r"^l\.\d+ ", x)), "")
                cmd = re.findall(r"\\[A-Za-z@]+", ctx)
                msg = f"Undefined control sequence {cmd[-1]}" if cmd else msg
        elif ln.startswith("! "):
            msg, where = ln[2:].strip(), None
            for nxt in lines[i + 1:i + 12]:
                lm = re.match(r"^l\.(\d+) ", nxt)
                if lm:
                    where = int(lm.group(1))
                    break
        else:
            continue
        if msg.startswith("LaTeX Error: "):
            msg = msg[13:]
        if msg in ("Emergency stop.", "==> Fatal error occurred, no output PDF file produced!") and errors:
            continue
        if (where, msg) not in seen:
            seen.add((where, msg))
            errors.append((where, msg))
    warnings = len(re.findall(r"^(?:LaTeX|Package \S+) Warning:", log, re.M))
    return errors[:30], warnings


def build_dir(path):
    key = hashlib.sha1((os.path.abspath(path) if path else "untitled").encode("utf-8")).hexdigest()[:16]
    return os.path.join(tempfile.gettempdir(), "rewrite-preview", key)


def build(source, path=None, encoding="utf-8", cancel=None):
    """Compile LaTeX `source` (the document at `path`, if it has one). Returns a Result."""
    t0 = time.time()
    engine = pick_engine(source)
    if not engine and not find("tectonic"):
        return Result(message="No LaTeX compiler was found.\n\nInstall MiKTeX (Windows), MacTeX (macOS) or "
                              "TeX Live (Linux), then press ↻.")
    out = build_dir(path)
    cwd = os.path.dirname(os.path.abspath(path)) if path else out
    stem = os.path.splitext(os.path.basename(path))[0] if path else "untitled"
    job = stem if re.fullmatch(r"[A-Za-z0-9_.-]+", stem) else "preview"
    try:
        os.makedirs(out, exist_ok=True)
        for sub in set(re.findall(r"\\include\{\s*([^}]+?)/[^}/]+\}", source)):  # \include writes sub/x.aux
            if not os.path.isabs(sub) and ".." not in sub:
                os.makedirs(os.path.join(out, sub), exist_ok=True)
        tex = os.path.join(out, job + ".tex")
        try:
            data = source.encode(encoding or "utf-8")
        except (UnicodeEncodeError, LookupError):
            data = source.encode("utf-8")
        with open(tex, "wb") as f:
            f.write(data)
    except OSError as e:
        return Result(message=f"Could not prepare the build folder:\n{e}")
    pdf, log = os.path.join(out, job + ".pdf"), os.path.join(out, job + ".log")
    for stale in (pdf, log):
        try:
            os.remove(stale)
        except OSError:
            pass
    env = dict(os.environ, max_print_line="1000", error_line="254", half_error_line="238")
    sep = os.pathsep
    try:
        if not engine:  # Tectonic reads the document from standard input and fetches what it needs
            env["TEXINPUTS"] = cwd + sep + env.get("TEXINPUTS", "")
            _run([find("tectonic"), "--outdir", out, "--keep-logs", "--chatter", "minimal", "-"], cwd, env, cancel, data)
            engine = "tectonic"
            pdf, log = os.path.join(out, "texput.pdf"), os.path.join(out, "texput.log")
        else:
            cmd = [find(engine), "-interaction=nonstopmode", "-file-line-error", "-synctex=0",
                   "-output-directory=" + out, "-jobname=" + job, tex]
            _run(cmd, cwd, env, cancel)
            text = _read(log)
            aux = _read(os.path.join(out, job + ".aux"))
            refs = "\n".join(ln for ln in aux.split("\n") if ln.startswith(("\\citation", "\\bibdata", "\\bibstyle")))
            bcf = os.path.join(out, job + ".bcf")
            stamp = os.path.join(out, job + ".bibstamp")
            sig = hashlib.sha1((refs + _read(bcf)).encode("utf-8", "replace")).hexdigest()
            runs = 1
            if (("\\bibdata" in refs) or os.path.isfile(bcf)) and _bib_changed(sig, stamp, out, job, cwd, source):
                benv = dict(env, BIBINPUTS=cwd + sep + env.get("BIBINPUTS", ""),
                            BSTINPUTS=cwd + sep + env.get("BSTINPUTS", ""))
                if os.path.isfile(bcf) and find("biber"):
                    _run([find("biber"), "--input-directory", cwd, job], out, benv, cancel)
                elif "\\bibdata" in refs and find("bibtex"):
                    _run([find("bibtex"), job], out, benv, cancel)
                with open(stamp, "w") as f:
                    f.write(sig)
                _run(cmd, cwd, env, cancel)
                runs += 1
                text = _read(log)
            while runs < 4 and _RERUN.search(text):  # cross-references, table of contents
                _run(cmd, cwd, env, cancel)
                runs += 1
                text = _read(log)
    except Cancelled:
        raise
    except (TimeoutError, OSError) as e:
        return Result(message=str(e), engine=engine or "", seconds=time.time() - t0, log=_read(log))
    text = _read(log)
    errors, warnings = parse_log(text, job + ".tex")
    data = None
    try:
        with open(pdf, "rb") as f:
            data = f.read()
    except OSError:
        pass
    message = ""
    if not data:
        message = "The document did not compile." if errors else "The compiler produced no PDF."
    return Result(data or None, errors, text, engine, time.time() - t0, warnings, message)


def _bib_changed(sig, stamp, out, job, cwd, source):
    """Run BibTeX/Biber again only when the citations or a .bib file changed."""
    bbl = os.path.join(out, job + ".bbl")
    if not os.path.isfile(bbl) or _read(stamp) != sig:
        return True
    made = os.path.getmtime(bbl)
    names = re.findall(r"\\(?:bibliography|addbibresource)(?:\[[^\]]*\])?\{([^}]+)\}", source)
    for name in (n.strip() for group in names for n in group.split(",")):
        f = os.path.join(cwd, name if name.endswith(".bib") else name + ".bib")
        if os.path.isfile(f) and os.path.getmtime(f) > made:
            return True
    return False
