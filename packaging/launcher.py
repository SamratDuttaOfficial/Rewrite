"""Entry point for the installed (frozen) app."""
import sys
import traceback


def selftest(log=None):
    """Exercise every native library in the bundle. Exit code 0 means the bundle is complete."""
    try:
        import numpy as np
        import onnxruntime as ort
        ort.OrtValue.ortvalue_from_numpy(np.zeros(3, dtype=np.float32)).numpy()  # numpy C-API bridge
        from rewrite.engine import Engine
        e = Engine()
        e.load_models()
        t = "The results show a significant improvement in accuracy."
        s = t.index("significant")
        assert e.suggest(t, s, s + len("significant"), 5, False), "no word suggestions"
        if e.t5:
            assert e.paraphrase(t, n=2), "no paraphrases"
        import os
        import pypdfium2
        pdf = pypdfium2.PdfDocument.new()
        pdf.new_page(200, 200)
        assert pdf[0].render(scale=0.5).width == 100, "PDFium does not render"  # PDF preview
        import tkinterdnd2  # drag and drop; its Tcl library is loaded when the window opens
        assert os.listdir(os.path.join(os.path.dirname(tkinterdnd2.__file__), "tkdnd")), "tkdnd is missing"
        msg, code = f"selftest ok (smart={e.smart}, paraphraser={bool(e.t5)})", 0
    except Exception:
        msg, code = traceback.format_exc(), 1
    if log:
        with open(log, "w", encoding="utf-8") as f:
            f.write(msg)
    return code


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[:1] == ["--selftest"]:
        sys.exit(selftest(args[1] if len(args) > 1 else None))
    from rewrite.app import main
    # macOS Finder may pass a -psn_ process id argument.
    main([a for a in args if not a.startswith("-psn_")])
