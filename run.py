#!/usr/bin/env python3
"""Start Rewrite:  python run.py  [file-to-open]

The first run creates a private environment in .venv next to this file and
installs the few small packages Rewrite needs. Later runs start immediately.
Options:  --reinstall   rebuild the environment
"""
import hashlib
import os
import platform
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
VENV = os.path.join(HERE, ".venv")
REQ = os.path.join(HERE, "requirements.txt")
STAMP = os.path.join(VENV, ".rewrite-installed")
CORE = ["numpy", "tokenizers", "lemminflect", "pypdf", "certifi"]
OPTIONAL = {"onnxruntime": "smart models will be unavailable",
            "tkinterdnd2": "files cannot be dropped on the window",
            "pypdfium2": "the LaTeX preview cannot show the PDF"}


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
        mod = {"pypdf": "pypdf"}.get(pkg, pkg)
        try:
            __import__(mod)
        except ImportError:
            pip(sys.executable, "--user", pkg)
    return sys.executable


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
    if "--reinstall" in sys.argv and os.path.exists(STAMP):
        os.remove(STAMP)
    py = setup()
    say("Starting Rewrite...")
    env = dict(os.environ, PYTHONPATH=HERE + os.pathsep + os.environ.get("PYTHONPATH", ""))
    code = subprocess.call([py, "-m", "rewrite", *args], cwd=HERE, env=env)
    sys.exit(code)


if __name__ == "__main__":
    main()
