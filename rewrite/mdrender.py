"""A small Markdown parser for the preview panel.

`parse(text)` returns a list of blocks; text inside a block is a list of runs
`(text, styles, link)` with styles from {"b", "i", "code", "strike", "math"}.
It covers what people actually write (headings, emphasis, lists, quotes, code,
tables, links, images) and shows anything else as plain text.
"""
import html
import re

_FENCE = re.compile(r"^ {0,3}(```+|~~~+)\s*([\w+#.-]*)[^\n]*$")
_HEADING = re.compile(r"^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
_RULE = re.compile(r"^ {0,3}([-*_])(?:\s*\1){2,}\s*$")
_ITEM = re.compile(r"^(\s*)([-*+]|\d{1,9}[.)])\s+(?:\[([ xX])\]\s+)?(.*)$")
_QUOTE = re.compile(r"^ {0,3}>\s?(.*)$")
_TABLE_SEP = re.compile(r"^ {0,3}\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$")
_LINK_DEF = re.compile(r"^ {0,3}\[([^\]\n]+)\]:\s*<?(\S+?)>?(?:\s+[\"'(].*)?\s*$")
_FRONT = re.compile(r"\A---\n[\s\S]*?\n(?:---|\.\.\.)\s*(?:\n|\Z)")

_INLINE = re.compile(
    r"\\([\\`*_{}\[\]()#+\-.!|~>$])"  # 1 escaped character
    r"|(`+)(.+?)\2(?!`)"  # 2, 3 code span
    r"|!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+[\"'][^\"']*[\"'])?\s*\)"  # 4, 5 image
    r"|\[((?:[^\[\]]|\[[^\]]*\])*)\]\(\s*<?([^)\s>]*)>?(?:\s+[\"'][^\"']*[\"'])?\s*\)"  # 6, 7 link
    r"|\[((?:[^\[\]])+)\]\[([^\]]*)\]"  # 8, 9 reference link
    r"|<((?:https?://|mailto:)[^>\s]+)>"  # 10 autolink
    r"|(?<![\w/(\"'=])((?:https?://|www\.)[^\s<>]*[^\s<>.,;:!?)\]'\"])"  # 11 bare URL
    r"|(\*\*\*|___)(?=\S)(.+?)(?<=\S)\12"  # 12, 13 bold italic
    r"|(\*\*|__)(?=\S)(.+?)(?<=\S)\14"  # 14, 15 bold
    r"|(?<![\w*])(\*|_)(?=[^\s*_])(.+?)(?<=[^\s*_])\16(?![\w*])"  # 16, 17 italic
    r"|~~(?=\S)(.+?)(?<=\S)~~"  # 18 strike
    r"|(?<![\\$\w])\$(?=\S)([^$\n]+?)(?<=\S)\$(?!\w)"  # 19 inline maths
    r"|\[\^([^\]\s]+)\]"  # 20 footnote reference
    r"|(<br\s*/?>)"  # 21 line break
    r"|(</?[A-Za-z][^>\n]*>|<!--.*?-->)"  # 22 other HTML: dropped
    r"|(&(?:#\d+|#x[0-9a-fA-F]+|\w+);)",  # 23 entity
    re.S)


def inline(text, styles=frozenset(), link=None, refs=None):
    """Split inline Markdown into runs: [(text, styles, link)]. An image is ("", {"img"}, (alt, src))."""
    out, pos = [], 0

    def emit(s, st=styles, ln=link):
        if s:
            out.append((s, st, ln))

    for m in _INLINE.finditer(text):
        emit(text[pos:m.start()])
        pos = m.end()
        g = m.group
        if g(1):
            emit(g(1))
        elif g(2):
            emit(g(3).strip() if g(3).strip() else g(3), styles | {"code"})
        elif g(5):
            out.append(("", frozenset({"img"}), (g(4), g(5))))
        elif g(6) is not None and g(7) is not None:
            out.extend(inline(g(6), styles, g(7) or None, refs))
        elif g(8):
            key = (g(9) or g(8)).lower()
            if refs and key in refs:
                out.extend(inline(g(8), styles, refs[key], refs))
            else:
                emit(m.group())
        elif g(10) or g(11):
            url = g(10) or g(11)
            emit(url, styles, url if "://" in url or url.startswith("mailto:") else "https://" + url)
        elif g(13):
            out.extend(inline(g(13), styles | {"b", "i"}, link, refs))
        elif g(15):
            out.extend(inline(g(15), styles | {"b"}, link, refs))
        elif g(17):
            out.extend(inline(g(17), styles | {"i"}, link, refs))
        elif g(18):
            out.extend(inline(g(18), styles | {"strike"}, link, refs))
        elif g(19):
            emit(g(19), styles | {"math"})
        elif g(20):
            emit("[" + g(20) + "]", styles | {"sup"})
        elif g(21):
            emit("\n")
        elif g(23):
            emit(html.unescape(g(23)))
    emit(text[pos:])
    return out


def _cells(line):
    line = line.strip()
    line = line[1:] if line.startswith("|") else line
    line = line[:-1] if line.endswith("|") and not line.endswith("\\|") else line
    return [c.strip().replace("\\|", "|") for c in re.split(r"(?<!\\)\|", line)]


def parse(text):
    """Blocks of a Markdown document, each a tuple starting with its type:

    ("heading", level, runs)  ("para", runs)  ("code", language, source)  ("rule",)
    ("item", depth, marker, checked, runs)  ("quote", depth, runs)
    ("table", [header runs], [alignment], [[row runs]])  ("math", source)
    """
    text = text.replace("\r\n", "\n").expandtabs(4)
    m = _FRONT.match(text)
    if m:
        text = text[m.end():]
    lines = text.split("\n")
    refs = {}
    keep = []
    for ln in lines:  # link definitions can come after their use
        d = _LINK_DEF.match(ln)
        if d and not ln.lstrip().startswith("[^"):
            refs[d.group(1).lower()] = d.group(2)
        else:
            keep.append(ln)
    lines = keep
    blocks, para, i, n = [], [], 0, len(lines)
    indents = []  # indentation of the open list levels

    def flush():
        if para:
            blocks.append(("para", inline(" ".join(x.strip() for x in para), refs=refs)))
            para.clear()

    while i < n:
        ln = lines[i]
        if not ln.strip():
            flush()
            i += 1
            continue
        f = _FENCE.match(ln)
        if f:
            flush()
            mark, body = f.group(1), []
            i += 1
            while i < n and not re.match(r"^ {0,3}" + re.escape(mark[0]) + "{" + str(len(mark)) + r",}\s*$", lines[i]):
                body.append(lines[i])
                i += 1
            i += 1
            blocks.append(("code", f.group(2), "\n".join(body)))
            continue
        if ln.strip().startswith("$$"):
            flush()
            body = [ln.strip()[2:]]
            if not (body[0].rstrip().endswith("$$") and body[0].strip() != ""):
                i += 1
                while i < n and "$$" not in lines[i]:
                    body.append(lines[i])
                    i += 1
                if i < n:
                    body.append(lines[i].split("$$")[0])
            else:
                body[0] = body[0].rstrip()[:-2]
            i += 1
            blocks.append(("math", "\n".join(x for x in body if x.strip()).strip()))
            continue
        h = _HEADING.match(ln)
        if h:
            flush()
            blocks.append(("heading", len(h.group(1)), inline(h.group(2), refs=refs)))
            i += 1
            continue
        if para and re.match(r"^ {0,3}(=+|-+)\s*$", ln):  # setext heading
            level = 1 if ln.strip().startswith("=") else 2
            blocks.append(("heading", level, inline(" ".join(x.strip() for x in para), refs=refs)))
            para.clear()
            i += 1
            continue
        if _RULE.match(ln):
            flush()
            blocks.append(("rule",))
            i += 1
            continue
        if "|" in ln and i + 1 < n and _TABLE_SEP.match(lines[i + 1]) and "-" in lines[i + 1]:
            flush()
            head = _cells(ln)
            align = ["center" if c.startswith(":") and c.endswith(":") else "right" if c.endswith(":") else "left"
                     for c in _cells(lines[i + 1])]
            rows = []
            i += 2
            while i < n and "|" in lines[i] and lines[i].strip():
                rows.append([inline(c, refs=refs) for c in _cells(lines[i])])
                i += 1
            blocks.append(("table", [inline(c, refs=refs) for c in head], align, rows))
            continue
        q = _QUOTE.match(ln)
        if q:
            flush()
            body, depth = [], 1
            while i < n and _QUOTE.match(lines[i]):
                inner = _QUOTE.match(lines[i]).group(1)
                d = 1
                while _QUOTE.match(inner):
                    inner, d = _QUOTE.match(inner).group(1), d + 1
                depth = max(depth, d)
                if inner.strip():
                    body.append(inner.strip())
                elif body:
                    blocks.append(("quote", depth, inline(" ".join(body), refs=refs)))
                    body = []
                i += 1
            if body:
                blocks.append(("quote", depth, inline(" ".join(body), refs=refs)))
            continue
        it = _ITEM.match(ln)
        if it and (not para or len(it.group(1)) < 4):
            flush()
            indent = len(it.group(1))
            while indents and indent < indents[-1]:
                indents.pop()
            if not indents or indent > indents[-1] + 1:
                indents.append(indent)
            body = [it.group(4)]
            i += 1
            while i < n and lines[i].strip() and not _ITEM.match(lines[i]) and not _FENCE.match(lines[i]) \
                    and not _HEADING.match(lines[i]):
                body.append(lines[i].strip())  # lazy continuation line
                i += 1
            checked = None if it.group(3) is None else it.group(3) != " "
            marker = it.group(2) if it.group(2)[0].isdigit() else "•"
            blocks.append(("item", len(indents) - 1, marker, checked, inline(" ".join(body), refs=refs)))
            if i < n and not lines[i].strip() and not (i + 1 < n and _ITEM.match(lines[i + 1])):
                indents.clear()
            continue
        if not para and ln.startswith("    ") and not indents:  # indented code
            body = []
            while i < n and (lines[i].startswith("    ") or not lines[i].strip()):
                body.append(lines[i][4:])
                i += 1
            blocks.append(("code", "", "\n".join(body).rstrip("\n")))
            continue
        indents.clear()
        para.append(ln)
        i += 1
    flush()
    return blocks
