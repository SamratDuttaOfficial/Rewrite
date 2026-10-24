"""Export and conversion: whatever is open (text, a formatted document or a PDF) written as PDF, Word,
OpenDocument, RTF, HTML, Markdown, LaTeX or plain text. The formats themselves are read and written by
the Rust core; this decides the route and writes the files."""
import os
import re
import tempfile

from . import docmodel, latexbuild, native, pdfdoc

TARGETS = (  # (extension, label)
    (".pdf", "PDF"), (".docx", "Word"), (".odt", "OpenDocument"), (".rtf", "Rich Text"),
    (".doc", "Word 97–2003"), (".fodt", "Flat OpenDocument"), (".html", "Web page"),
    (".md", "Markdown"), (".tex", "LaTeX"), (".txt", "Plain text"),
)
TEXT_KIND = {".md": "markdown", ".tex": "latex", ".txt": "plain"}


class Source:
    """A still picture of what is open, taken on the UI thread."""

    def __init__(self, kind, name, path=None, text=None, model=None, data=None, encoding="utf-8"):
        self.kind, self.name, self.path = kind, name, path
        self.text, self.model, self.data, self.encoding = text, model, data, encoding

    @property
    def stem(self):
        stem = os.path.splitext(self.name or "document")[0]
        return re.sub(r"[^\w.-]+", "-", stem).strip("-") or "document"

    @property
    def base_dir(self):
        return os.path.dirname(os.path.abspath(self.path)) if self.path else None


def engines(source, target):
    """The ways a PDF can be made from this source: [(key, label)], best first."""
    if target != ".pdf" or source.kind == "pdf":
        return []
    out = []
    tex = latexbuild.available()
    if source.kind == "latex" and tex:
        out.append(("latex", "LaTeX"))
    out.append(("rewrite", "Rewrite's own layout"))
    if source.kind == "markdown" and tex:
        out.append(("latex", "LaTeX (via a .tex file)"))
    if source.kind == "rich" and native.available() and native.office_path():
        out.append(("office", "LibreOffice"))
    return out


def model_of(source, opts, helper=None, progress=None, cancel=None):
    """The document as a docmodel Document."""
    if source.kind == "rich":
        return source.model
    if source.kind == "pdf":
        options = {"drop_headers": opts.get("drop_headers", True), "fix_words": opts.get("fix_words", True),
                   "images": opts.get("images", True), "tables": opts.get("tables", True)}
        return pdfdoc.to_document(source.data, options, helper if options["fix_words"] else None, progress, cancel)
    data = (source.text or "").encode("utf-8")
    if source.kind == "markdown":
        return native.read(data, "md", source.base_dir)
    if source.kind == "latex":
        return native.read(data, "tex", source.base_dir, pdfdoc.first_page_png)
    return native.read(data, "txt")


def export(source, target, path, opts, helper=None, progress=None, cancel=None):
    """Write the source to path in the target format. Returns the paths written."""
    stem = os.path.splitext(os.path.basename(path))[0]
    out_dir = os.path.dirname(os.path.abspath(path))
    sentences = helper.sentences if (opts.get("sentences") and helper) else None
    written = []

    def put(rel, data):
        full = os.path.join(out_dir, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        tmp = full + ".rewrite-tmp"
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, full)
        written.append(full)

    # text written as the same kind of text: the file as it is (unless sentences go on lines of their own)
    if target in TEXT_KIND and TEXT_KIND[target] == source.kind and not sentences:
        put(os.path.basename(path), (source.text or "").encode(source.encoding or "utf-8", "replace"))
        return written
    if target == ".pdf":
        if source.kind == "pdf":
            put(os.path.basename(path), source.data)
            return written
        engine = opts.get("engine") or "rewrite"
        if engine == "latex" and source.kind == "latex":
            res = latexbuild.build(source.text or "", source.path, source.encoding or "utf-8", cancel)
            if not res.pdf:
                raise RuntimeError(_latex_failure(res))
            put(os.path.basename(path), res.pdf)
            return written
        model = model_of(source, opts, helper, progress, cancel)
        page = opts.get("page")
        if engine == "latex":
            tex, files = native.write(model, "tex", stem)
            work = tempfile.mkdtemp(prefix="rewrite-export-")
            for rel, data in files.items():
                full = os.path.join(work, rel)
                os.makedirs(os.path.dirname(full), exist_ok=True)
                with open(full, "wb") as f:
                    f.write(data)
            res = latexbuild.build(tex.decode("utf-8"), os.path.join(work, stem + ".tex"), "utf-8", cancel)
            if not res.pdf:
                raise RuntimeError(_latex_failure(res))
            put(os.path.basename(path), res.pdf)
        elif engine == "office":
            docx, _ = native.write(model, "docx")
            put(os.path.basename(path), native.office_convert(docx, ".docx", "pdf", 300, cancel.is_set if cancel else None))
        else:
            if page is not None:
                model.page = page
            pdf = native.render_pdf(model, opts.get("generic") or "serif", opts.get("numbers"), page)
            put(os.path.basename(path), pdf)
        return written
    model = model_of(source, opts, helper, progress, cancel)
    if target in (".doc", ".docx", ".odt", ".fodt", ".rtf"):
        from .formats import save_rich
        save_rich(model, path)
        written.append(path)
        return written
    fmt = {".html": "html", ".md": "md", ".tex": "tex", ".txt": "txt"}[target]
    data, files = native.write(model, fmt, stem, sentences)
    put(os.path.basename(path), data)
    for rel, blob in files.items():
        put(rel, blob)
    return written


def _latex_failure(res):
    lines = [res.message or "LaTeX did not make a PDF."]
    for line, msg in (res.errors or [])[:6]:
        lines.append((f"Line {line}: " if line else "") + msg)
    return "\n".join(lines)


def default_page(source):
    if source.kind == "rich" and source.model is not None:
        return source.model.page
    return docmodel.PageSetup()
