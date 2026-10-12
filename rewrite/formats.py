"""Loading/saving documents and finding the parts that must not be rewritten.

`protected_spans(text, kind)` returns sorted, merged (start, end) character
ranges such as LaTeX commands and maths, Markdown code, URLs, HTML tags, or
(for source code) everything except comments.
"""
import io
import os
import re
import zipfile
from xml.etree import ElementTree as ET

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
BINARY_IMPORT = {".docx", ".odt", ".pdf", ".rtf"}
KIND_LABEL = {"plain": "Plain text", "markdown": "Markdown", "latex": "LaTeX", "html": "HTML/XML", "code": "Code (comments only)"}

OPEN_TYPES = [
    ("All supported", "*.txt *.md *.markdown *.tex *.latex *.bib *.rst *.html *.htm *.docx *.odt *.pdf *.rtf *.csv *.json *.xml *.py *.js *.ts *.c *.cpp *.java *.go *.rs *.yaml *.yml"),
    ("Text", "*.txt"), ("Markdown", "*.md *.markdown"), ("LaTeX", "*.tex *.latex *.bib"),
    ("Word / OpenDocument", "*.docx *.odt"), ("PDF", "*.pdf"), ("Rich text", "*.rtf"),
    ("All files", "*.*"),
]


class Document:
    def __init__(self, text="", path=None, kind="plain", encoding="utf-8", newline="\n", source_ext=None):
        self.text, self.path, self.kind = text, path, kind
        self.encoding, self.newline = encoding, newline
        self.source_ext = source_ext  # e.g. ".docx" when imported from a binary format

    @property
    def name(self):
        return os.path.basename(self.path) if self.path else "Untitled"

    @property
    def savable_in_place(self):
        return bool(self.path) and self.source_ext not in BINARY_IMPORT - {".docx"}


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
    if ext == ".docx":
        return Document(_docx_text(raw), path, "plain", source_ext=ext)
    if ext == ".odt":
        return Document(_odt_text(raw), path, "plain", source_ext=ext)
    if ext == ".pdf":
        return Document(_pdf_text(raw), path, "plain", source_ext=ext)
    if ext == ".rtf":
        return Document(_rtf_text(raw.decode("latin-1")), path, "plain", source_ext=ext)
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


W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _docx_text(raw):
    with zipfile.ZipFile(io.BytesIO(raw)) as z:
        root = ET.fromstring(z.read("word/document.xml"))
    paras = []
    for p in root.iter(W + "p"):
        buf = []
        for el in p.iter():
            if el.tag == W + "t" and el.text:
                buf.append(el.text)
            elif el.tag == W + "tab":
                buf.append("\t")
            elif el.tag in (W + "br", W + "cr"):
                buf.append("\n")
        paras.append("".join(buf))
    return "\n\n".join(p for p in paras if p.strip())


def _odt_text(raw):
    ns = "{urn:oasis:names:tc:opendocument:xmlns:text:1.0}"
    with zipfile.ZipFile(io.BytesIO(raw)) as z:
        root = ET.fromstring(z.read("content.xml"))
    paras = []
    for el in root.iter():
        if el.tag in (ns + "p", ns + "h"):
            paras.append("".join(el.itertext()))
    return "\n\n".join(p for p in paras if p.strip())


def _pdf_text(raw):
    try:
        from pypdf import PdfReader
    except Exception:
        raise ValueError("Reading PDF needs the 'pypdf' package (run.py installs it).")
    reader = PdfReader(io.BytesIO(raw))
    pages = []
    for page in reader.pages:
        t = page.extract_text() or ""
        # join hyphenated line breaks and single line breaks inside paragraphs
        t = re.sub(r"(\w)-\n(\w)", r"\1\2", t)
        t = re.sub(r"(?<![.!?:\n])\n(?!\n)", " ", t)
        pages.append(t.strip())
    return "\n\n".join(p for p in pages if p)


def _rtf_text(s):
    s = re.sub(r"\{\\\*[^{}]*(\{[^{}]*\}[^{}]*)*\}", "", s)
    s = re.sub(r"\{\\(fonttbl|colortbl|stylesheet|info)[\s\S]*?\}\s*\}", "", s)
    s = re.sub(r"\\'([0-9a-fA-F]{2})", lambda m: bytes([int(m.group(1), 16)]).decode("cp1252", "replace"), s)
    s = re.sub(r"\\u(-?\d+)\??", lambda m: chr(int(m.group(1)) % 65536), s)
    s = re.sub(r"\\(par|line)\b ?", "\n", s)
    s = re.sub(r"\\tab\b ?", "\t", s)
    s = re.sub(r"\\[a-zA-Z]+-?\d* ?", "", s)
    s = re.sub(r"\\([{}\\])", r"\1", s)
    s = s.replace("{", "").replace("}", "")
    return re.sub(r"\n{3,}", "\n\n", s).strip()


# -- saving --------------------------------------------------------------------------

def save(doc, text, path=None):
    path = path or doc.path
    ext = os.path.splitext(path)[1].lower()
    if ext == ".docx":
        _write_docx(text, path)
    else:
        enc = doc.encoding if doc.source_ext is None else "utf-8"
        data = text.replace("\n", doc.newline if doc.source_ext is None else os.linesep)
        with open(path, "w", encoding=enc, newline="") as f:
            f.write(data)
    doc.path = path
    doc.source_ext = None if ext != ".docx" else ".docx"
    if ext != ".docx":
        doc.kind = kind_for(path) if kind_for(path) != "plain" else doc.kind


def _esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _write_docx(text, path):
    body = []
    for para in re.split(r"\n\s*\n", text.strip("\n")):
        lines = para.split("\n")
        runs = '<w:r><w:br/></w:r>'.join(
            f'<w:r><w:t xml:space="preserve">{_esc(l)}</w:t></w:r>' for l in lines)
        body.append(f"<w:p>{runs}</w:p>")
    document = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        f'<w:body>{"".join(body)}<w:sectPr/></w:body></w:document>')
    content_types = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
        '</Types>')
    rels = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" '
        'Target="word/document.xml"/></Relationships>')
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", content_types)
        z.writestr("_rels/.rels", rels)
        z.writestr("word/document.xml", document)


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
