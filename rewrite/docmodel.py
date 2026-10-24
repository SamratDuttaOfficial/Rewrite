"""A formatted document, the hub that every import and export goes through.

The readers and writers (Word, OpenDocument, RTF, Markdown, LaTeX, PDF ...) live in the Rust core
(crates/core/src); this module is the Python side of the same structure, used by the editor. The two meet
as plain dicts: `to_native` and `from_native` convert. A document is a list of blocks:

    Paragraph   inline content with a style (normal, title, h1 ... h6, quote, caption), alignment and
                list membership
    Table       rows of cells, each cell a list of paragraphs
    CodeBlock   preformatted text            MathBlock   TeX maths shown on its own line
    Rule        a horizontal line            PageBreak

Inline content is a list of `Run` (text with character formatting), `Image`, `Chart`, `Break`,
`Note` (a footnote) and `Opaque` (a piece of a Word file that is kept as it was).
"""
import copy
import re
from dataclasses import dataclass, field, fields

STYLES = ("normal", "title", "subtitle", "h1", "h2", "h3", "h4", "h5", "h6", "quote", "caption")
STYLE_LABEL = {"normal": "Normal text", "title": "Title", "subtitle": "Subtitle", "h1": "Heading 1",
               "h2": "Heading 2", "h3": "Heading 3", "h4": "Heading 4", "h5": "Heading 5", "h6": "Heading 6",
               "quote": "Quote", "caption": "Caption", "code": "Code", "math": "Maths"}

# How each paragraph style looks: size relative to the body text, weight, slant, space before/after
# (in points) and extra indent. Shared by the editor, the PDF layout and the Word/ODF writers.
LOOK = {
    "normal": dict(scale=1.0, bold=False, italic=False, before=0, after=6),
    "title": dict(scale=2.4, bold=False, italic=False, before=0, after=10, color="#17365D"),
    "subtitle": dict(scale=1.35, bold=False, italic=False, before=0, after=10, color="#4F6228"),
    "h1": dict(scale=1.65, bold=True, italic=False, before=16, after=6, color="#1F3864"),
    "h2": dict(scale=1.35, bold=True, italic=False, before=12, after=4, color="#2F5496"),
    "h3": dict(scale=1.18, bold=True, italic=False, before=10, after=4, color="#1F3864"),
    "h4": dict(scale=1.05, bold=True, italic=True, before=8, after=2, color="#2F5496"),
    "h5": dict(scale=1.0, bold=True, italic=False, before=8, after=2, color="#2F5496"),
    "h6": dict(scale=1.0, bold=False, italic=True, before=8, after=2, color="#1F3864"),
    "quote": dict(scale=1.0, bold=False, italic=True, before=4, after=8, indent=36, color="#404040"),
    "caption": dict(scale=0.85, bold=False, italic=True, before=2, after=10, color="#44546A"),
    "code": dict(scale=0.88, bold=False, italic=False, before=4, after=8),
    "math": dict(scale=1.0, bold=False, italic=True, before=6, after=8),
}

PAGE_SIZES = {"A4": (595.28, 841.89), "Letter": (612.0, 792.0), "Legal": (612.0, 1008.0), "A5": (419.53, 595.28)}


@dataclass
class Run:
    text: str = ""
    b: bool = False
    i: bool = False
    u: bool = False
    s: bool = False  # struck through
    sup: bool = False
    sub: bool = False
    code: bool = False
    math: bool = False  # TeX maths source, shown inline
    size: float = None  # points; None = the paragraph's size
    font: str = None  # family name; None = the document's font
    color: str = None  # "#RRGGBB"
    hl: str = None  # highlight colour "#RRGGBB"
    link: str = None

    def same_format(self, other):
        return all(getattr(self, f.name) == getattr(other, f.name) for f in fields(self) if f.name != "text")

    def styled(self, text):
        r = copy.copy(self)
        r.text = text
        return r


@dataclass
class Image:
    data: bytes = b""
    ext: str = "png"  # png, jpeg, gif, bmp, emf, wmf, tiff, svg ...
    width: float = 0.0  # points as placed in the document
    height: float = 0.0
    alt: str = ""
    name: str = ""  # suggested file name when written out

    def fit(self, max_w, max_h=None):
        """Width and height scaled down (never up) to fit max_w x max_h points."""
        w, h = self.width or 1.0, self.height or 1.0
        k = min(1.0, max_w / w, (max_h / h) if max_h else 1.0)
        return w * k, h * k


@dataclass
class Chart:
    kind: str = "bar"  # bar (vertical), hbar, line, area, pie, scatter
    title: str = ""
    categories: list = field(default_factory=list)  # x labels (scatter: x values as text)
    series: list = field(default_factory=list)  # [(name, [numbers])]
    x_title: str = ""
    y_title: str = ""
    legend: bool = True
    width: float = 432.0  # points (6 x 3.5 inches)
    height: float = 252.0


@dataclass
class Break:
    kind: str = "line"  # line | page | column


@dataclass
class Note:
    """A footnote: its text, shown where the reference mark is."""
    runs: list = field(default_factory=list)


@dataclass
class Opaque:
    """A piece of a Word file this program does not edit (an equation, a text box, a field ...).
    It is shown as a small label and written back unchanged."""
    xml: str = ""
    label: str = "object"
    text: str = ""  # its visible text, for plain-text exports
    rels: dict = field(default_factory=dict)  # relationship id -> (type, target, target mode) it needs
    block: bool = False  # a whole block of the body (a table of contents ...), not a piece of a paragraph
    fmt: str = "docx"  # the format the xml belongs to


@dataclass
class Paragraph:
    content: list = field(default_factory=list)
    style: str = "normal"
    align: str = None  # None (left), center, right, justify
    list: tuple = None  # ("bullet" | "number", level)
    indent: int = 0  # extra indent steps (half an inch each)

    @property
    def text(self):
        return inline_text(self.content)


@dataclass
class Table:
    rows: list = field(default_factory=list)  # [[cell]], cell = [Paragraph]
    header: bool = True  # the first row is a header
    widths: list = None  # relative column widths

    @property
    def ncols(self):
        return max((len(r) for r in self.rows), default=0)


@dataclass
class CodeBlock:
    text: str = ""
    lang: str = ""


@dataclass
class MathBlock:
    tex: str = ""


@dataclass
class Rule:
    pass


@dataclass
class PageBreak:
    pass


@dataclass
class PageSetup:
    width: float = PAGE_SIZES["A4"][0]
    height: float = PAGE_SIZES["A4"][1]
    margins: tuple = (72.0, 72.0, 72.0, 72.0)  # top, right, bottom, left
    numbers: bool = True  # page numbers in the footer

    @property
    def text_width(self):
        return self.width - self.margins[1] - self.margins[3]

    @property
    def text_height(self):
        return self.height - self.margins[0] - self.margins[2]

    def name(self):
        for n, (w, h) in PAGE_SIZES.items():
            if abs(w - self.width) < 2 and abs(h - self.height) < 2:
                return n
            if abs(h - self.width) < 2 and abs(w - self.height) < 2:
                return n + " landscape"
        return f"{self.width / 72:.1f} x {self.height / 72:.1f} in"


@dataclass
class Document:
    blocks: list = field(default_factory=list)
    title: str = ""
    author: str = ""
    page: PageSetup = field(default_factory=PageSetup)
    font: str = None  # body font family; None = the program's default
    size: float = 11.0  # body text size in points
    source: dict = field(default_factory=dict)  # format-specific data kept for saving back (e.g. "docx")

    def paragraphs(self):
        """Every paragraph, including those inside table cells."""
        for b in self.blocks:
            if isinstance(b, Paragraph):
                yield b
            elif isinstance(b, Table):
                for row in b.rows:
                    for cell in row:
                        yield from cell


def inline_text(content, objects=True):
    """The visible text of inline content."""
    out = []
    for x in content:
        if isinstance(x, Run):
            out.append(x.text)
        elif isinstance(x, Break):
            out.append("\n")
        elif isinstance(x, Note):
            out.append("")
        elif isinstance(x, Opaque) and objects:
            out.append(x.text)
    return "".join(out)


def merge_runs(content):
    """Join neighbouring runs that look the same; drop empty ones."""
    out = []
    for x in content:
        if isinstance(x, Run):
            if not x.text:
                continue
            if out and isinstance(out[-1], Run) and out[-1].same_format(x):
                out[-1] = out[-1].styled(out[-1].text + x.text)
                continue
        out.append(x)
    return out


def strip_content(content):
    """Inline content without the white space at its start and end; a paragraph that holds only pictures
    or charts (and spaces) keeps just those."""
    content = merge_runs(content)
    objects = [x for x in content if not isinstance(x, Run)]
    if objects and all(isinstance(x, (Image, Chart, Break)) for x in objects) and \
            all(not x.text.strip() for x in content if isinstance(x, Run)):
        return [x for x in objects if not isinstance(x, Break)]
    while content and isinstance(content[0], Run) and not content[0].text.strip():
        content.pop(0)
    while content and isinstance(content[-1], Run) and not content[-1].text.strip():
        content.pop()
    if content and isinstance(content[0], Run):
        content[0] = content[0].styled(content[0].text.lstrip())
    if content and isinstance(content[-1], Run):
        content[-1] = content[-1].styled(content[-1].text.rstrip())
    return content


CAPTION_LABEL = re.compile(r"^\s*(Figure|Fig\.|Table|Tab\.|Chart|Listing)\s*[\dIVX]+[.:]?\s*", re.I)


def caption_text(content):
    """A caption's content without its "Figure 3:" label (LaTeX and Word number captions themselves)."""
    text = inline_text(content)
    m = CAPTION_LABEL.match(text)
    if not m:
        return content
    drop, out = m.end(), []
    for x in content:
        if isinstance(x, Run) and drop > 0:
            if len(x.text) <= drop:
                drop -= len(x.text)
                continue
            x = x.styled(x.text[drop:])
            drop = 0
        out.append(x)
    return out


def plain_paragraphs(text):
    """A plain text as paragraphs: blank lines separate paragraphs, single line breaks are kept."""
    blocks = []
    for para in re.split(r"\n[ \t]*\n", text.strip("\n")):
        if not para.strip():
            continue
        content = []
        for k, line in enumerate(para.split("\n")):
            if k:
                content.append(Break())
            content.append(Run(line))
        blocks.append(Paragraph(merge_runs(content)))
    return blocks


def document_text(doc, objects=True):
    """The text of a document, paragraphs separated by blank lines (tables as tab-separated rows)."""
    out = []
    for b in doc.blocks:
        if isinstance(b, Paragraph):
            out.append(b.text if objects else inline_text(b.content, False))
        elif isinstance(b, Table):
            out.append("\n".join("\t".join(" ".join(p.text for p in cell) for cell in row) for row in b.rows))
        elif isinstance(b, CodeBlock):
            out.append(b.text)
        elif isinstance(b, MathBlock):
            out.append(b.tex)
    return "\n\n".join(x for x in out if x.strip())


def number_label(n, level):
    """The label of item n (from 1) of a numbered list at a nesting level: 1. a. i. 1. ..."""
    kind = level % 3
    if kind == 1:
        s, k = "", n
        while k:
            k, r = divmod(k - 1, 26)
            s = chr(97 + r) + s
        return s + "."
    if kind == 2:
        return roman(n).lower() + "."
    return f"{n}."


def roman(n):
    vals = ((1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
            (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"))
    out = []
    for v, s in vals:
        while n >= v:
            out.append(s)
            n -= v
    return "".join(out) or "0"


BULLETS = ("•", "◦", "▪")


def list_labels(blocks):
    """The marker of every list paragraph in a block list, by id(paragraph): "•", "1.", "a." ...
    Numbering restarts after anything that is not a list paragraph."""
    labels, counters = {}, []
    for b in blocks:
        if not (isinstance(b, Paragraph) and b.list):
            counters = []
            continue
        kind, level = b.list
        del counters[level + 1:]
        while len(counters) <= level:
            counters.append(0)
        if kind == "number":
            counters[level] += 1
            labels[id(b)] = number_label(counters[level], level)
        else:
            counters[level] = 0
            labels[id(b)] = BULLETS[level % 3]
    return labels


# -- the dict form shared with the Rust core ---------------------------------------------

_TYPES = {"run": Run, "image": Image, "chart": Chart, "break": Break, "note": Note, "opaque": Opaque,
          "paragraph": Paragraph, "table": Table, "codeblock": CodeBlock, "mathblock": MathBlock, "rule": Rule,
          "pagebreak": PageBreak}
_NAMES = {cls: name for name, cls in _TYPES.items()}


def to_native(x):
    """A Document (or any part of one) as plain dicts and lists; blocks and inline items get a "type"."""
    if isinstance(x, (bytes, bytearray, str, int, float, bool)) or x is None:
        return bytes(x) if isinstance(x, bytearray) else x
    if hasattr(x, "__dataclass_fields__"):
        d = {f.name: to_native(getattr(x, f.name)) for f in fields(x)}
        if type(x) in _NAMES:
            d["type"] = _NAMES[type(x)]
        return d
    if isinstance(x, (list, tuple)):
        return [to_native(v) for v in x]
    if isinstance(x, dict):
        return {k: to_native(v) for k, v in x.items()}
    return x


def _from(x):
    if isinstance(x, dict) and x.get("type") in _TYPES:
        cls = _TYPES[x["type"]]
        names = {f.name for f in fields(cls)}
        obj = cls(**{k: _from(v) for k, v in x.items() if k in names})
        if cls is Paragraph and obj.list is not None:
            obj.list = (obj.list[0], int(obj.list[1]))
        if cls is Chart:
            obj.series = [(n, list(v)) for n, v in obj.series]
        if cls is Opaque:
            obj.rels = {k: tuple(v) for k, v in obj.rels.items()}
        if cls is Table:  # cells hold plain paragraphs (no "type" key)
            obj.rows = [[[p if isinstance(p, Paragraph) else _from(dict(p, type="paragraph")) for p in cell]
                         for cell in row] for row in obj.rows]
        return obj
    if isinstance(x, list):
        return [_from(v) for v in x]
    return x


def from_native(d):
    """A Document from its dict form."""
    page = PageSetup(**{k: v for k, v in (d.get("page") or {}).items() if k in PageSetup.__dataclass_fields__})
    page.margins = tuple(page.margins)
    return Document(blocks=_from(d.get("blocks", [])), title=d.get("title", ""), author=d.get("author", ""),
                    page=page, font=d.get("font"), size=d.get("size") or 11.0, source=dict(d.get("source") or {}))
