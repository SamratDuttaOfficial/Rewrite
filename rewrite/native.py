"""The Rust core (the rewrite_native extension, built from native/), seen from Python.

Documents go in and out as docmodel objects; everything else is passed through. Formats are read
and written in Rust; PDF pages are read here with PDFium (pdfdoc.py) and analysed in Rust.
"""
import os

from . import docmodel

try:
    import rewrite_native as _n
    ERROR = None
except ImportError as e:  # not built yet (run.py builds it) or built for another Python
    _n = None
    ERROR = str(e)

# file extension -> format name of the Rust readers and writers
READ = {".md": "md", ".markdown": "md", ".mdx": "md", ".rmd": "md", ".qmd": "md", ".tex": "tex", ".latex": "tex",
        ".ltx": "tex", ".txt": "txt", ".text": "txt", ".docx": "docx", ".docm": "docx", ".dotx": "docx",
        ".odt": "odt", ".ott": "odt", ".fodt": "fodt", ".rtf": "rtf", ".doc": "doc", ".dot": "doc"}
WRITE = {".md": "md", ".tex": "tex", ".txt": "txt", ".docx": "docx", ".odt": "odt", ".fodt": "fodt", ".rtf": "rtf",
         ".html": "html", ".htm": "html"}
RICH = {".docx", ".docm", ".dotx", ".odt", ".ott", ".fodt", ".rtf", ".doc", ".dot"}  # edited with formatting


def available():
    return _n is not None


def require():
    if _n is None:
        raise RuntimeError("Rewrite's document engine is not built. Start Rewrite with  python run.py  "
                           f"so it can build it (it needs Rust from rustup.rs).\n\n{ERROR}")
    return _n


def read(data, fmt, base_dir=None, pdf_figure=None):
    """A Document from file bytes in a format named by READ."""
    return docmodel.from_native(require().read(fmt, data, base_dir, pdf_figure))


def read_file(path, pdf_figure=None):
    ext = os.path.splitext(path)[1].lower()
    with open(path, "rb") as f:
        data = f.read()
    doc = read(data, READ.get(ext, "txt"), os.path.dirname(os.path.abspath(path)), pdf_figure)
    return doc


def write(doc, fmt, stem="document", sentences=None):
    """(bytes, {relative path: bytes}) for a Document written in a format named by WRITE."""
    return require().write(fmt, docmodel.to_native(doc), stem, sentences)


def render_pdf(doc, generic="serif", numbers=None, page=None):
    """PDF bytes for a Document, typeset here."""
    return require().render_pdf(docmodel.to_native(doc), generic, numbers,
                                docmodel.to_native(page) if page is not None else None)


def pdf_document(pages, options=None, helper=None, render=None):
    return docmodel.from_native(require().pdf_document(pages, options, helper, render))


def sentence_starts(text, helper=None):
    return require().sentence_starts(text, helper)


def chart_shapes(chart, w, h, measure):
    return require().chart_shapes(docmodel.to_native(chart), w, h, measure)


def chart_svg(chart):
    return require().chart_svg(docmodel.to_native(chart))


def chart_parse_table(text):
    return require().chart_parse_table(text)


def chart_format_table(chart):
    return require().chart_format_table(docmodel.to_native(chart))


def chart_kinds():
    return require().chart_kinds()


def __getattr__(name):
    """Everything else (math_*, image_*, office_*, font_families ...) straight from the extension."""
    return getattr(require(), name)
