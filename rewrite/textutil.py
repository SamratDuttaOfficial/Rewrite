"""Small text helpers: words, sentences, case matching."""
import re

WORD_RE = re.compile(r"[A-Za-z][A-Za-z']*(?:-[A-Za-z]+)*")

# Abbreviations that end with a period but do not end a sentence.
_ABBREV = {
    "e.g", "i.e", "etc", "vs", "al", "fig", "figs", "eq", "eqs", "sec", "ch", "no", "vol", "pp",
    "dr", "mr", "mrs", "ms", "prof", "st", "jr", "sr", "inc", "ltd", "co", "cf", "approx", "resp",
    "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "u.s", "ref",
}
_SENT_END = re.compile(r"[.!?]+[\"')\]]*(?=\s)|\n\s*\n")

STOPWORDS = set("""
a about above after again against all am an and any are as at be because been before being below
between both but by can could did do does doing down during each few for from further had has have
having he her here hers herself him himself his how i if in into is it its itself just me more most my
myself no nor not now of off on once only or other our ours ourselves out over own same she should so
some such than that the their theirs them themselves then there these they this those through to too
under until up very was we were what when where which while who whom why will with would you your yours
yourself yourselves also may might must shall s t don let us one ones get got
""".split())


def words(text):
    return [(m.start(), m.end(), m.group()) for m in WORD_RE.finditer(text)]


_LINE_START = re.compile(
    r"\s*(#{1,6}\s|[-*+]\s|\d+[.)]\s|>|%|\||"
    r"\\(?!(?:textbf|textit|emph|cite\w*|ref|eqref|footnote|url|href|textsc|underline|mbox|LaTeX|TeX)\b)[A-Za-z])")
_STANDALONE = re.compile(r"\s*(#|%|\||\\(?!item\b|textbf|textit|emph)[A-Za-z])")


def _blocks(text):
    """Split text at blank lines and around heading/command/title-like lines."""
    cuts = {0, len(text)}
    lines = text.split("\n")
    pos = 0
    for i, ln in enumerate(lines):
        st, en = pos, pos + len(ln)
        pos = en + 1
        stripped = ln.strip()
        if not stripped:
            cuts.update((st, en))
            continue
        if _LINE_START.match(ln):
            cuts.add(st)
            if _STANDALONE.match(ln):
                cuts.add(en)
        elif len(stripped.split()) <= 10 and not re.search(r"[.!?:;,\"')\]]$", stripped):
            nxt = lines[i + 1] if i + 1 < len(lines) else ""
            if not nxt.strip() or nxt.strip()[:1].isupper() or _LINE_START.match(nxt):
                cuts.update((st, en))  # a title-like line
    cuts = sorted(cuts)
    return [(a, b) for a, b in zip(cuts, cuts[1:]) if text[a:b].strip()]


def sentence_bounds(text):
    """Return a list of (start, end) spans of sentences in text."""
    out = []
    for bs, be in _blocks(text):
        out.extend((bs + s, bs + e) for s, e in _sentences(text[bs:be]))
    return out


def _sentences(text):
    spans, start = [], 0
    for m in _SENT_END.finditer(text):
        end = m.end()
        if m.group().startswith(".") and len(m.group()) == 1:
            tail = text[max(0, m.start() - 12):m.start()]
            tok = re.split(r"\s", tail)[-1].lower().lstrip("([\"'")
            if tok in _ABBREV or re.fullmatch(r"[a-z]", tok) or re.fullmatch(r"\d+", tok):
                # "Fig. 3", "e.g. this", single initials: not a boundary
                nxt = text[end:end + 3].lstrip()
                if not nxt[:1].isupper() or tok in _ABBREV:
                    continue
        if text[start:end].strip():
            spans.append((start, end))
        start = end
    if text[start:].strip():
        spans.append((start, len(text)))
    # trim whitespace from spans
    out = []
    for s, e in spans:
        while s < e and text[s].isspace():
            s += 1
        while e > s and text[e - 1].isspace():
            e -= 1
        if e > s:
            out.append((s, e))
    return out


def sentence_at(text, pos):
    for s, e in sentence_bounds(text):
        if s <= pos <= e:
            return s, e
    return None


def match_case(template, word):
    """Give `word` the capitalisation style of `template`."""
    if not word:
        return word
    if template.isupper() and len(template) > 1:
        return word.upper()
    if template[:1].isupper():
        return word[:1].upper() + word[1:]
    return word


def word_diff_ratio(a, b):
    """Fraction of words in b that are not aligned to identical words in a."""
    import difflib
    wa = [w.lower() for _, _, w in words(a)]
    wb = [w.lower() for _, _, w in words(b)]
    if not wb:
        return 0.0
    sm = difflib.SequenceMatcher(None, wa, wb, autojunk=False)
    same = sum(bl.size for bl in sm.get_matching_blocks())
    return 1.0 - same / max(1, len(wb))
