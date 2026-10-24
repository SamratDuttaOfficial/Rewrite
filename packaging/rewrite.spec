# PyInstaller spec for Rewrite. Normally run through packaging/build.py.
import os
import sys

from PyInstaller.utils.hooks import collect_data_files, collect_dynamic_libs, collect_submodules

ROOT = os.path.dirname(SPECPATH)
sys.path.insert(0, ROOT)
from rewrite import __author__, __version__  # noqa: E402

BUILD = os.path.join(ROOT, "build")
ICON = {"win32": os.path.join(BUILD, "rewrite.ico"),
        "darwin": os.path.join(BUILD, "rewrite.icns")}.get(sys.platform)

datas = [(os.path.join(ROOT, "rewrite", "data"), "rewrite/data"),
         (os.path.join(ROOT, "rewrite", "assets"), "rewrite/assets")]
datas += collect_data_files("lemminflect") + collect_data_files("certifi")
# Drag and drop (the tkdnd Tcl library inside tkinterdnd2) and the PDF preview (PDFium's version file).
datas += collect_data_files("tkinterdnd2") + collect_data_files("pypdfium2") + collect_data_files("pypdfium2_raw")

a = Analysis([os.path.join(SPECPATH, "launcher.py")], pathex=[ROOT], datas=datas,
             binaries=collect_dynamic_libs("onnxruntime") + collect_dynamic_libs("pypdfium2_raw"),
             # onnxruntime imports numpy.core.* at run time (hidden from analysis); NumPy 2 keeps these as shims.
             hiddenimports=collect_submodules("numpy.core") + ["tkinterdnd2", "pypdfium2", "rewrite_native"],
             excludes=["PIL", "matplotlib", "IPython", "pytest", "scipy", "pandas", "torch"])
pyz = PYZ(a.pure)
exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name="Rewrite", console=False, icon=ICON)
coll = COLLECT(exe, a.binaries, a.datas, name="Rewrite")

if sys.platform == "darwin":
    app = BUNDLE(coll, name="Rewrite.app", icon=ICON, bundle_identifier="com.samratdutta.rewrite",
                 version=__version__,
                 info_plist={"CFBundleShortVersionString": __version__,
                             "NSHumanReadableCopyright": f"© {__author__}",
                             "NSHighResolutionCapable": True})
