"""Loading/saving documents and finding the parts that must not be rewritten.

Text files (plain, Markdown, LaTeX, HTML, code) are edited as text. Word, OpenDocument and RTF files are
edited with their formatting ("rich"), read and written by the Rust core (native.py). PDF files open in
the PDF view.

`protected_spans(text, kind)` returns sorted, merged (start, end) character
ranges such as LaTeX commands and maths, Markdown code, URLs, HTML tags, or
(for source code) everything except comments.
"""
import os
import re

from . import native

KIND_BY_EXT = {
    ".tex": "latex", ".latex": "latex", ".sty": "latex", ".cls": "latex", ".bib": "latex", ".ltx": "latex",
    ".md": "markdown", ".markdown": "markdown", ".mdx": "markdown", ".rmd": "markdown", ".qmd": "markdown",
    ".html": "html", ".htm": "html", ".xhtml": "html", ".xml": "html", ".svg": "html",
    ".py": "code", ".js": "code", ".ts": "code", ".jsx": "code", ".tsx": "code", ".c": "code", ".h": "code",
    ".cpp": "code", ".hpp": "code", ".cc": "code", ".java": "code", ".go": "code", ".rs": "code",
    ".rb": "code", ".sh": "code", ".bash": "code", ".ps1": "code", ".cs": "code", ".swift": "code",
    ".kt": "code", ".php": "code", ".r": "code", ".m": "code", ".scala": "code", ".lua": "code",
    ".pl": "code", ".css": "code", ".scss": "code", ".sql": "code", ".yaml": "code", ".yml": "code",
    ".toml": "code", ".ini": "code", ".cfg": "code",
}
RICH = native.RICH  # edited with formatting
SAVE_RICH = {".docx": "docx", ".docm": "docx", ".dotx": "docx", ".odt": "odt", ".ott": "odt", ".fodt": "fodt",
             ".rtf": "rtf", ".doc": "doc", ".dot": "doc"}
KIND_LABEL = {"plain": "Plain text", "markdown": "Markdown", "latex": "LaTeX", "html": "HTML/XML",
              "code": "Code (comments only)", "rich": "Formatted document", "pdf": "PDF"}
FORMAT_LABEL = {".docx": "Word", ".docm": "Word", ".dotx": "Word template", ".doc": "Word 97–2003", ".dot": "Word 97–2003",
                ".odt": "OpenDocument", ".ott": "OpenDocument template", ".fodt": "Flat OpenDocument",
                ".rtf": "Rich Text", ".pdf": "PDF"}

OPEN_TYPES = [
    ("All supported", "*.txt *.md *.markdown *.tex *.latex *.bib *.rst *.html *.htm *.docx *.docm *.dotx *.doc *.dot "
                      "*.odt *.ott *.fodt *.rtf *.pdf *.csv *.json *.xml *.py *.js *.ts *.c *.cpp *.java *.go *.rs "
                      "*.yaml *.yml"),
    ("Word", "*.docx *.docm *.dotx *.doc *.dot"), ("OpenDocument / LibreOffice", "*.odt *.ott *.fodt"),
    ("Rich text", "*.rtf"), ("PDF", "*.pdf"),
    ("Text", "*.txt"), ("Markdown", "*.md *.markdown"), ("LaTeX", "*.tex *.latex *.bib"),
    ("All files", "*.*"),
]
SAVE_RICH_TYPES = [("Word document", "*.docx"), ("OpenDocument text", "*.odt"), ("Flat OpenDocument", "*.fodt"),
                   ("Rich Text", "*.rtf"), ("Word 97–2003 (needs LibreOffice)", "*.doc")]


class Document:
    """An open file. Text kinds keep their text in the editor; a "rich" document starts from `model` (a
    docmodel.Document); a "pdf" one holds the file's bytes in `data`."""

    def __init__(self, text="", path=None, kind="plain", encoding="utf-8", newline="\n", source_ext=None,
                 model=None, data=None):
        self.text, self.path, self.kind = text, path, kind
        self.encoding, self.newline = encoding, newline
        self.source_ext = source_ext  # e.g. ".docx": the format of a formatted document or PDF
        self.model = model
        self.data = data

    @property
    def name(self):
        return os.path.basename(self.path) if self.path else "Untitled"

    @property
    def ext(self):
        return os.path.splitext(self.path or "")[1].lower()


def kind_for(path):
    ext = os.path.splitext(path or "")[1].lower()
    return KIND_BY_EXT.get(ext, "plain")


def sniff_kind(text):
    if re.search(r"\\(documentclass|begin\{document\}|section\{|usepackage)", text):
        return "latex"
    if re.search(r"^(#{1,6} |```|\* |- \[)", text, re.M):
        return "markdown"
    return "plain"


# -- loading -------------------------------------------------------------------------

def load(path):
    ext = os.path.splitext(path)[1].lower()
    with open(path, "rb") as f:
        raw = f.read()
    if ext in RICH:
        native.require()
        try:
            model = native.read(raw, native.READ[ext], os.path.dirname(os.path.abspath(path)))
        except Exception as e:
            if ext in (".doc", ".dot") and native.office_path():
                # an old Word file the reader cannot follow: let LibreOffice turn it into .docx first
                model = native.read(native.office_convert(raw, ext, "docx", 180, None), "docx")
            else:
                raise ValueError(f"The file could not be read: {e}")
        return Document("", path, "rich", source_ext=ext, model=model)
    if ext == ".pdf":
        if not raw.startswith(b"%PDF") and b"%PDF" not in raw[:1024]:
            raise ValueError("This is not a PDF file.")
        return Document("", path, "pdf", source_ext=ext, data=raw)
    text, enc = _decode(raw)
    newline = "\r\n" if "\r\n" in text[:20000] else "\n"
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    kind = kind_for(path)
    if kind == "plain" and ext in ("", ".txt"):
        kind = sniff_kind(text)
    return Document(text, path, kind, enc, newline)


def _decode(raw):
    if raw.startswith(b"\xef\xbb\xbf"):
        return raw[3:].decode("utf-8", "replace"), "utf-8-sig"
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        return raw.decode("utf-16"), "utf-16"
    if b"\x00" in raw[:4000]:
        raise ValueError("This looks like a binary file, not text.")
    for enc in ("utf-8", "cp1252"):
        try:
            return raw.decode(enc), enc
        except UnicodeDecodeError:
            pass
    return raw.decode("latin-1"), "latin-1"


# -- saving --------------------------------------------------------------------------

def save(doc, text, path=None):
    """Save a text document (in its own encoding and line endings), or its text as Word/OpenDocument/RTF."""
    path = path or doc.path
    ext = os.path.splitext(path)[1].lower()
    if ext in SAVE_RICH:
        model = native.read(text.encode("utf-8"), {"markdown": "md", "latex": "tex"}.get(doc.kind, "txt"))
        save_rich(model, path)
        return
    enc = doc.encoding if doc.source_ext is None else "utf-8"
    data = text.replace("\n", doc.newline if doc.source_ext is None else os.linesep)
    with open(path, "w", encoding=enc, newline="") as f:
        f.write(data)
    doc.path = path
    doc.source_ext = None
    doc.kind = kind_for(path) if kind_for(path) != "plain" else doc.kind


def save_rich(model, path):
    """Write a docmodel Document in the format the file name asks for. Old .doc files are made by
    LibreOffice from a .docx (Rewrite writes no .doc itself)."""
    ext = os.path.splitext(path)[1].lower()
    fmt = SAVE_RICH.get(ext)
    if fmt is None:
        raise ValueError(f"Formatted documents are saved as .docx, .odt, .fodt, .rtf or .doc, not {ext or 'no extension'}.")
    if fmt == "doc":
        if not native.office_path():
            raise ValueError("Saving as .doc needs LibreOffice. Save as .docx instead (Word opens it), or install "
                             "LibreOffice from libreoffice.org.")
        data, _ = native.write(model, "docx")
        data = native.office_convert(data, ".docx", "doc", 180, None)
    else:
        data, _ = native.write(model, fmt)
    tmp = path + ".rewrite-tmp"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, path)


# -- protected regions ---------------------------------------------------------------

_URL = re.compile(r"\b(?:https?://|www\.)[^\s<>()\[\]{}\"']+|\b[\w.+-]+@[\w-]+\.[\w.-]+\b")

_TEX_VERBATIM_ENVS = (
    "verbatim|Verbatim|lstlisting|minted|comment|tikzpicture|pgfpicture|equation\\*?|align\\*?|alignat\\*?|"
    "gather\\*?|multline\\*?|eqnarray\\*?|flalign\\*?|math|displaymath|split|array|[pbvBV]?matrix|cases|"
    "algorithmic|filecontents\\*?"
)
_TEX_ARG_CMDS = (
    "cite[a-zA-Z]*|[Cc]ref|ref|eqref|autoref|pageref|nameref|label|url|href|includegraphics|input|include|"
    "includeonly|usepackage|RequirePackage|documentclass|bibliography|bibliographystyle|addbibresource|begin|end|"
    "newcommand|renewcommand|providecommand|DeclareMathOperator|newenvironment|renewenvironment|def|let|"
    "setlength|addtolength|setcounter|addtocounter|hspace|vspace|color|textcolor|colorbox|definecolor|"
    "pagestyle|thispagestyle|geometry|hypersetup|graphicspath|lstset|lstinputlisting|newtheorem|numberwithin|"
    "captionsetup|bibitem|nocite|usetikzlibrary|pgfplotsset|sisetup|SI|si|num|ang|qty|unit|fontsize|"
    "rule|raisebox|resizebox|scalebox|multicolumn|multirow|cline|cmidrule|setcopyright|acmConference|"
    "doi|isbn|issn|orcid|email|affiliation|address|keywords|ccsdesc|verb|lstinline|mintinline|texttt|path"
)
_BR = r"\{(?:[^{}]|\{(?:[^{}]|\{[^{}]*\})*\})*\}"  # braces, 3 levels deep


def _latex_spans(text):
    spans = []
    begin_doc = text.find("\\begin{document}")
    if begin_doc > 0:
        spans.append((0, begin_doc + len("\\begin{document}")))
    for m in re.finditer(r"\\begin\{(" + _TEX_VERBATIM_ENVS + r")\}[\s\S]*?\\end\{\1\}", text):
        spans.append(m.span())
    for pat in (r"(?<!\\)%[^\n]*", r"\$\$[\s\S]*?\$\$", r"(?<!\\)\$(?:\\.|[^$\\])+?\$",
                r"\\\[[\s\S]*?\\\]", r"\\\([\s\S]*?\\\)", r"\\verb(.)[^\n]*?\1"):
        spans += [m.span() for m in re.finditer(pat, text)]
    for m in re.finditer(r"\\(" + _TEX_ARG_CMDS + r")\*?(?:\s*\[[^\]]*\])*(?:\s*" + _BR + r"(?:\s*\[[^\]]*\])*)*", text):
        spans.append(m.span())
    for m in re.finditer(r"\\[A-Za-z@]+\*?(?:\[[^\]\n]*\])*|\\[^A-Za-z\s]", text):
        spans.append(m.span())
    spans += [m.span() for m in re.finditer(r"[{}&~^_]", text)]
    return spans


def _markdown_spans(text):
    spans = []
    m = re.match(r"---\n[\s\S]*?\n(---|\.\.\.)\s*\n", text)
    if m:
        spans.append(m.span())
    for pat in (r"^(```|~~~)[^\n]*\n[\s\S]*?^\1[^\n]*$", r"(`+)[^`\n][\s\S]*?\1", r"\]\([^)\n]*\)",
                r"<!--[\s\S]*?-->", r"</?[A-Za-z][^>\n]*>", r"^\s*\[[^\]\n]+\]:[^\n]*$",
                r"\$\$[\s\S]*?\$\$", r"(?<![\\$\w])\$(?=\S)[^$\n]+?(?<=\S)\$(?!\w)", r"^ {0,3}\|?[\s:|-]+\|[\s:|-]*$",
                r"\{[#.][^}\n]*\}", r"\[\^[^\]]+\]"):
        spans += [m.span() for m in re.finditer(pat, text, re.M)]
    return spans


def _html_spans(text):
    spans = []
    for pat in (r"<(script|style|pre|code)\b[\s\S]*?</\1\s*>", r"<!--[\s\S]*?-->", r"<[^>]+>", r"&#?\w+;"):
        spans += [m.span() for m in re.finditer(pat, text, re.I)]
    return spans


_HASH_LANGS = {".py", ".sh", ".bash", ".rb", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".r", ".pl", ".ps1"}
_SLASH_LANGS = {".js", ".ts", ".jsx", ".tsx", ".c", ".h", ".cpp", ".hpp", ".cc", ".java", ".go", ".rs", ".cs",
                ".swift", ".kt", ".php", ".scala", ".css", ".scss", ".m"}
_COMMENT_PATTERNS = {
    "hash": (r'"""[\s\S]*?"""', r"'''[\s\S]*?'''", r"(?:^|(?<=\s))#(?![!{\[])[^\n]*"),
    "slash": (r"/\*[\s\S]*?\*/", r"(?<![:\"'\w])//[^\n]*"),
    "dash": (r"(?:^|(?<=\s))--\s[^\n]*", r"/\*[\s\S]*?\*/"),
}


def _code_spans(text, path=None):
    """Everything is protected except comments and docstrings."""
    ext = os.path.splitext(path or "")[1].lower()
    if ext in _HASH_LANGS:
        pats = _COMMENT_PATTERNS["hash"]
    elif ext in _SLASH_LANGS:
        pats = _COMMENT_PATTERNS["slash"]
    elif ext in (".sql", ".lua"):
        pats = _COMMENT_PATTERNS["dash"]
    else:
        pats = sum(_COMMENT_PATTERNS.values(), ())
    allowed = []
    for pat in pats:
        allowed += [m.span() for m in re.finditer(pat, text, re.M)]
    allowed = merge(allowed)
    spans, pos = [], 0
    for s, e in allowed:
        if s > pos:
            spans.append((pos, s))
        pos = max(pos, e)
    if pos < len(text):
        spans.append((pos, len(text)))
    return spans


def merge(spans):
    out = []
    for s, e in sorted(spans):
        if e <= s:
            continue
        if out and s <= out[-1][1]:
            if e > out[-1][1]:
                out[-1] = (out[-1][0], e)
        else:
            out.append((s, e))
    return out


def protected_spans(text, kind, path=None):
    spans = [m.span() for m in _URL.finditer(text)]
    if kind == "latex":
        spans += _latex_spans(text)
    elif kind == "markdown":
        spans += _markdown_spans(text)
    elif kind == "html":
        spans += _html_spans(text)
    elif kind == "code":
        spans += _code_spans(text, path)
    return merge(spans)


def overlaps(spans, start, end):
    """True if [start, end) touches any protected span (spans sorted & merged)."""
    import bisect
    i = bisect.bisect_right(spans, (start, float("inf"))) - 1
    for j in (i, i + 1):
        if 0 <= j < len(spans):
            s, e = spans[j]
            if s < end and start < e:
                return True
    return False


def clip(spans, start, end):
    """Protected spans inside [start, end), relative to start."""
    import bisect
    out = []
    i = max(0, bisect.bisect_right(spans, (start, float("inf"))) - 1)
    while i < len(spans) and spans[i][0] < end:
        s, e = spans[i]
        if e > start:
            out.append((max(s, start) - start, min(e, end) - start))
        i += 1
    return out
