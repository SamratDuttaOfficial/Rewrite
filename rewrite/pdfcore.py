"""Shared access to PDFium (through pypdfium2).

PDFium must never run on two threads at once, so every caller holds LOCK while it uses it. The
preview, the PDF editor, the converters and the image decoder all run on different worker threads.
"""
import threading

LOCK = threading.RLock()


def available():
    try:
        import pypdfium2  # noqa: F401
        return True
    except Exception:
        return False
