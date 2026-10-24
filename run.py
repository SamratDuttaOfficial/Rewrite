#!/usr/bin/env python3
"""Start Rewrite:  python run.py  [file-to-open]

The first run creates a private environment in .venv next to this file and
installs the few small packages Rewrite needs, then builds the document
engine (native/, written in Rust) when Rust is installed. Later runs start
immediately; the engine is rebuilt only when its source changes.
Options:  --reinstall   rebuild the environment and the engine
"""
import hashlib
import os
import platform
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
VENV = os.path.join(HERE, ".venv")
REQ = os.path.join(HERE, "requirements.txt")
STAMP = os.path.join(VENV, ".rewrite-installed")
NATIVE = os.path.join(HERE, "native")
NATIVE_STAMP = os.path.join(VENV, ".rewrite-native")
CORE = ["numpy", "tokenizers", "lemminflect", "certifi"]
OPTIONAL = {"onnxruntime": "smart models will be unavailable",
            "tkinterdnd2": "files cannot be dropped on the window",
            "pypdfium2": "PDF files and the LaTeX preview cannot be shown"}


def say(msg):
    print(f"[rewrite] {msg}", flush=True)


def tk_help():
    system = platform.system()
    if system == "Linux":
        return ("Tkinter is missing. Install it with your package manager, e.g.\n"
                "  Ubuntu/Debian:  sudo apt install python3-tk python3-venv\n"
                "  Fedora:         sudo dnf install python3-tkinter\n"
                "  Arch:           sudo pacman -S tk")
    if system == "Darwin":
        return ("Tkinter is missing. Use the Python installer from python.org, or with Homebrew:\n"
                "  brew install python-tk")
    return "Tkinter is missing. Reinstall Python from python.org and keep 'tcl/tk and IDLE' ticked."


def venv_python():
    if os.name == "nt":
        return os.path.join(VENV, "Scripts", "python.exe")
    return os.path.join(VENV, "bin", "python")


def req_hash():
    with open(REQ, "rb") as f:
        return hashlib.sha1(f.read() + sys.version.encode()).hexdigest()


def pip(py, *args):
    return subprocess.call([py, "-m", "pip", "install", "--disable-pip-version-check", "-q", *args]) == 0


def setup():
    """Create .venv and install requirements. Returns the python to use."""
    py = venv_python()
    if os.path.exists(py) and os.path.exists(STAMP):
        with open(STAMP) as f:
            if f.read().strip() == req_hash():
                return py
    if not os.path.exists(py):
        say("First run: creating a private Python environment (one-time, about a minute)...")
        try:
            subprocess.check_call([sys.executable, "-m", "venv", VENV])
        except Exception:
            say("Could not create a virtual environment.")
            if platform.system() == "Linux":
                say("On Debian/Ubuntu install it with:  sudo apt install python3-venv")
            say("Falling back to the current Python installation.")
            return fallback_install()
    say("Installing packages: " + ", ".join(CORE + list(OPTIONAL)) + " ...")
    subprocess.call([py, "-m", "pip", "install", "--disable-pip-version-check", "-q", "--upgrade", "pip"])
    if not pip(py, "-r", REQ):
        say("Bulk install failed; trying packages one by one...")
        for pkg in CORE + list(OPTIONAL):
            if not pip(py, pkg):
                say(f"  could not install {pkg}" + (f" - {OPTIONAL[pkg]}" if pkg in OPTIONAL else ""))
    with open(STAMP, "w") as f:
        f.write(req_hash())
    say("Setup complete.")
    return py


def fallback_install():
    for pkg in CORE + list(OPTIONAL):
        try:
            __import__(pkg)
        except ImportError:
            pip(sys.executable, "--user", pkg)
    return sys.executable


def cargo():
    found = shutil.which("cargo")
    if found:
        return found
    home = os.path.join(os.path.expanduser("~"), ".cargo", "bin", "cargo.exe" if os.name == "nt" else "cargo")
    return home if os.path.exists(home) else None


def native_hash():
    """A fingerprint of the engine's source, so it is rebuilt only when that changes."""
    h = hashlib.sha1(sys.version.encode())
    for top in ("Cargo.toml", "Cargo.lock", "pyproject.toml", "src"):
        path = os.path.join(NATIVE, top)
        files = [path] if os.path.isfile(path) else sorted(
            os.path.join(d, f) for d, _, fs in os.walk(path) for f in fs if f.endswith(".rs"))
        for f in files:
            h.update(os.path.relpath(f, NATIVE).encode())
            with open(f, "rb") as fh:
                h.update(fh.read())
    return h.hexdigest()


def build_native(py):
    """Build the document engine (Word, OpenDocument, RTF, PDF and the rest) into the environment."""
    if not os.path.isdir(NATIVE):
        return
    want = native_hash()
    if os.path.exists(NATIVE_STAMP):
        with open(NATIVE_STAMP) as f:
            if f.read().strip() == want:
                return
    tool = cargo()
    if not tool:
        say("Rust is not installed, so the document engine cannot be built: text files work, but Word, "
            "OpenDocument, RTF and PDF files need it. Install Rust from https://rustup.rs and start again.")
        return
    say("Building the document engine (the first time takes a few minutes)...")
    env = dict(os.environ, VIRTUAL_ENV=VENV, PATH=os.path.dirname(tool) + os.pathsep + os.environ.get("PATH", ""))
    if subprocess.call([py, "-c", "import maturin"], stderr=subprocess.DEVNULL) != 0 and not pip(py, "maturin>=1.5"):
        say("Could not install maturin, the engine's build tool.")
        return
    code = subprocess.call([py, "-m", "maturin", "develop", "--release", "--manifest-path",
                            os.path.join(NATIVE, "Cargo.toml")], cwd=NATIVE, env=env)
    if code != 0:
        say("The document engine did not build (see above). Text files still work.")
        return
    with open(NATIVE_STAMP, "w") as f:
        f.write(want)
    say("Document engine ready.")


def main():
    if sys.version_info < (3, 9):
        say(f"Python 3.9 or newer is needed (this is {platform.python_version()}).")
        sys.exit(1)
    try:
        import tkinter  # noqa: F401
    except ImportError:
        say(tk_help())
        sys.exit(1)
    args = [a for a in sys.argv[1:] if a != "--reinstall"]
    if "--reinstall" in sys.argv:
        for stamp in (STAMP, NATIVE_STAMP):
            if os.path.exists(stamp):
                os.remove(stamp)
    py = setup()
    build_native(py)
    say("Starting Rewrite...")
    env = dict(os.environ, PYTHONPATH=HERE + os.pathsep + os.environ.get("PYTHONPATH", ""))
    code = subprocess.call([py, "-m", "rewrite", *args], cwd=HERE, env=env)
    sys.exit(code)


if __name__ == "__main__":
    main()
