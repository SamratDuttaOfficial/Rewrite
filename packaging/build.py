"""Build the Rewrite installer for the operating system this runs on.

    python packaging/build.py

Windows  dist/installers/Rewrite-<ver>-Windows-Setup.exe     (needs Inno Setup 6)
macOS    dist/installers/Rewrite-<ver>-macOS-<arch>.dmg
Linux    dist/installers/rewrite_<ver>_<arch>.deb              (needs dpkg-deb)
         dist/installers/Rewrite-<ver>-<arch>.AppImage        (needs appimagetool on PATH or in $APPIMAGETOOL)

The document engine (native/, Rust) is built as a wheel with maturin, so Rust must be installed
(rustup.rs). The app is frozen with PyInstaller in a private build environment (build/venv). The unpacked app is only
an intermediate step: it is made in build/app and removed once the installers exist, so dist/ holds
nothing but the installers.
Installers can only be built on their own OS. .github/workflows/installers.yml builds all three.
"""
import glob
import os
import platform
import shutil
import struct
import subprocess
import sys
import venv

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
BUILD = os.path.join(ROOT, "build")
APP = os.path.join(BUILD, "app")  # the frozen app; deleted again at the end of the build
OUT = os.path.join(ROOT, "dist", "installers")
ASSETS = os.path.join(ROOT, "rewrite", "assets")

sys.path.insert(0, ROOT)
from rewrite import __author__, __email__, __version__ as VERSION  # noqa: E402

SUMMARY = "Offline, context-aware rewriting editor"
DESKTOP = """[Desktop Entry]
Type=Application
Name=Rewrite
Comment={summary}
Exec={exec} %f
Icon=rewrite
Terminal=false
Categories=Office;TextTools;
MimeType=text/plain;text/markdown;text/x-tex;application/pdf;application/rtf;application/msword;application/vnd.openxmlformats-officedocument.wordprocessingml.document;application/vnd.oasis.opendocument.text;
StartupWMClass=Rewrite
"""


def run(*cmd, **kw):
    print("+", " ".join(map(str, cmd)), flush=True)
    subprocess.check_call([str(c) for c in cmd], **kw)


def write(base, rel, text, mode=0o644):
    path = os.path.join(base, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    os.chmod(path, mode)
    return path


def png(size):
    with open(os.path.join(ASSETS, f"icon{size}.png"), "rb") as f:
        return f.read()


def make_icons():
    """Windows .ico and macOS .icns built from the PNG icons (both formats can embed PNGs)."""
    os.makedirs(BUILD, exist_ok=True)
    imgs = [(s, png(s)) for s in (32, 64, 256)]
    offset, entries, blobs = 6 + 16 * len(imgs), b"", b""
    for s, data in imgs:
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset + len(blobs))
        blobs += data
    with open(os.path.join(BUILD, "rewrite.ico"), "wb") as f:
        f.write(struct.pack("<HHH", 0, 1, len(imgs)) + entries + blobs)
    body = b"".join(t + struct.pack(">I", len(d) + 8) + d
                    for t, d in ((b"icp5", png(32)), (b"icp6", png(64)), (b"ic08", png(256))))
    with open(os.path.join(BUILD, "rewrite.icns"), "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(body) + 8) + body)


def build_python():
    env = os.path.join(BUILD, "venv")
    py = os.path.join(env, "Scripts", "python.exe") if os.name == "nt" else os.path.join(env, "bin", "python")
    if not os.path.exists(py):
        venv.create(env, with_pip=True)
        run(py, "-m", "pip", "install", "-q", "--upgrade", "pip")
    run(py, "-m", "pip", "install", "-q", "-r", os.path.join(ROOT, "requirements.txt"), "pyinstaller>=6.3",
        "maturin>=1.5")
    return py


def build_native(py):
    """The document engine as a wheel, installed into the build environment for PyInstaller to collect."""
    wheels = os.path.join(BUILD, "wheels")
    shutil.rmtree(wheels, ignore_errors=True)
    run(py, "-m", "maturin", "build", "--release", "--manifest-path", os.path.join(ROOT, "native", "Cargo.toml"),
        "--out", wheels)
    run(py, "-m", "pip", "install", "-q", "--force-reinstall", "--no-deps", *glob.glob(os.path.join(wheels, "*.whl")))


def freeze(py):
    run(py, "-m", "PyInstaller", "--noconfirm", "--clean", "--log-level", "WARN", "--distpath", APP,
        "--workpath", os.path.join(BUILD, "pyinstaller"), os.path.join(HERE, "rewrite.spec"))
    # Run the frozen app once so a missing module fails the build, not the user.
    exe = os.path.join(APP, "Rewrite", "Rewrite.exe" if os.name == "nt" else "Rewrite")
    log = os.path.join(BUILD, "selftest.log")
    code = subprocess.call([exe, "--selftest", log])
    with open(log, encoding="utf-8") as f:
        print(f.read())
    if code:
        sys.exit("Self-test of the frozen app failed.")


def iscc():
    cands = [shutil.which("iscc")]
    for base in (os.environ.get("ProgramFiles(x86)"), os.environ.get("ProgramFiles"),
                 os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")):
        if base:
            cands.append(os.path.join(base, "Inno Setup 6", "ISCC.exe"))
    return next((c for c in cands if c and os.path.exists(c)), None)


def windows():
    run(iscc(), "/Q", f"/DVersion={VERSION}", f"/DSrc={os.path.join(APP, 'Rewrite')}", f"/DOutDir={OUT}",
        f"/DIcon={os.path.join(BUILD, 'rewrite.ico')}", os.path.join(HERE, "windows", "rewrite.iss"))


def macos():
    stage = os.path.join(BUILD, "dmg")
    shutil.rmtree(stage, ignore_errors=True)
    shutil.copytree(os.path.join(APP, "Rewrite.app"), os.path.join(stage, "Rewrite.app"), symlinks=True)
    os.symlink("/Applications", os.path.join(stage, "Applications"))
    dmg = os.path.join(OUT, f"Rewrite-{VERSION}-macOS-{platform.machine()}.dmg")
    run("hdiutil", "create", "-volname", "Rewrite", "-srcfolder", stage, "-ov", "-format", "UDZO", dmg)


def linux():
    app, machine = os.path.join(APP, "Rewrite"), platform.machine()
    icon = os.path.join(ASSETS, "icon256.png")
    if shutil.which("dpkg-deb"):
        arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(machine, machine)
        root = os.path.join(BUILD, "deb")
        shutil.rmtree(root, ignore_errors=True)
        shutil.copytree(app, os.path.join(root, "opt", "rewrite"), symlinks=True)
        write(root, "usr/share/applications/rewrite.desktop",
              DESKTOP.format(summary=SUMMARY, exec="/opt/rewrite/Rewrite"))
        icons = os.path.join(root, "usr/share/icons/hicolor/256x256/apps")
        os.makedirs(icons)
        shutil.copy(icon, os.path.join(icons, "rewrite.png"))
        os.makedirs(os.path.join(root, "usr/bin"))
        os.symlink("/opt/rewrite/Rewrite", os.path.join(root, "usr/bin/rewrite"))
        size = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(root) for f in fs
                   if not os.path.islink(os.path.join(d, f))) // 1024
        write(root, "DEBIAN/control", f"""Package: rewrite
Version: {VERSION}
Architecture: {arch}
Maintainer: {__author__} <{__email__}>
Installed-Size: {size}
Section: editors
Priority: optional
Depends: libc6, libx11-6
Description: {SUMMARY}
 Suggests context-aware alternatives for words, phrases and whole sentences.
 Edits text, Markdown, LaTeX, Word, OpenDocument and RTF files, marks up and
 changes PDF files, and converts between them. Runs fully offline.
""")
        run("dpkg-deb", "--build", "--root-owner-group", root, os.path.join(OUT, f"rewrite_{VERSION}_{arch}.deb"))
    else:
        print("dpkg-deb not found: skipping the .deb")

    tool = os.environ.get("APPIMAGETOOL") or shutil.which("appimagetool")
    if not tool:
        print("appimagetool not found: skipping the AppImage")
        return
    appdir = os.path.join(BUILD, "Rewrite.AppDir")
    shutil.rmtree(appdir, ignore_errors=True)
    shutil.copytree(app, os.path.join(appdir, "usr", "lib", "rewrite"), symlinks=True)
    write(appdir, "rewrite.desktop", DESKTOP.format(summary=SUMMARY, exec="rewrite"))
    shutil.copy(icon, os.path.join(appdir, "rewrite.png"))
    write(appdir, "AppRun", '#!/bin/sh\nexec "$(dirname "$(readlink -f "$0")")/usr/lib/rewrite/Rewrite" "$@"\n', 0o755)
    run(tool, appdir, os.path.join(OUT, f"Rewrite-{VERSION}-{machine}.AppImage"),
        env={**os.environ, "ARCH": machine})


def main():
    if sys.platform == "win32" and not iscc():
        sys.exit("Inno Setup 6 is needed to build the Windows installer.\n"
                 "Install it with:  winget install JRSoftware.InnoSetup   (or from https://jrsoftware.org/isdl.php)")
    make_icons()
    py = build_python()
    build_native(py)
    freeze(py)
    os.makedirs(OUT, exist_ok=True)
    {"win32": windows, "darwin": macos}.get(sys.platform, linux)()
    shutil.rmtree(APP, ignore_errors=True)
    print("\nDone:", *sorted(glob.glob(os.path.join(OUT, "*"))), sep="\n  ")


if __name__ == "__main__":
    main()
