#!/usr/bin/env python3
"""Record what the Python modules return for the parity corpus, so the Rust ports can be tested against it.

    .venv/Scripts/python.exe tools/parity_dump.py [group ...]     (all groups when none are named)

Each group is written to tests/fixtures/<group>.json as
    {"group": ..., "source": <commit>, "cases": {<function>: [{"args": [...], "out": ...}, ...]}}
Offsets are Python string offsets (characters). A call that raises records {"error": "<Type>: <message>"}.
The engine_smart group needs the three models; it is skipped when they are missing.
This script goes away with the last Python module; the fixtures stay as regression tests.
"""
import base64
import dataclasses
import json
import math
import os
import random
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
CORPUS = os.path.join(ROOT, "tests", "corpus")
OUT = os.path.join(ROOT, "tests", "fixtures")


def corpus(name):
    with open(os.path.join(CORPUS, name), encoding="utf-8") as f:
        return f.read()


def example(name):
    with open(os.path.join(ROOT, "examples", name), encoding="utf-8") as f:
        return f.read()


def plain(x):
    """A JSON-ready copy: tuples and sets become lists, objects become dicts, bytes become base64."""
    if isinstance(x, float):
        return None if math.isnan(x) else round(x, 6)
    if isinstance(x, (str, int, bool)) or x is None:
        return x
    if hasattr(x, "tolist"):  # numpy scalars and arrays
        return plain(x.tolist())
    if isinstance(x, (bytes, bytearray)):
        return {"b64": base64.b64encode(bytes(x)).decode()}
    if isinstance(x, dict):
        return {str(k): plain(v) for k, v in x.items()}
    if isinstance(x, (set, frozenset)):
        return sorted(plain(v) for v in x)
    if hasattr(x, "_asdict"):
        return plain(list(x))
    if dataclasses.is_dataclass(x):
        return {f.name: plain(getattr(x, f.name)) for f in dataclasses.fields(x)}
    if hasattr(x, "__slots__"):
        return {k: plain(getattr(x, k)) for k in x.__slots__}
    if isinstance(x, (list, tuple)) or hasattr(x, "__iter__"):
        return [plain(v) for v in x]
    raise TypeError(f"cannot record {type(x).__name__}")


class Group:
    def __init__(self, name):
        self.name, self.cases = name, {}

    def add(self, fn, args, call):
        try:
            out = plain(call())
        except Exception as e:
            out = {"error": f"{type(e).__name__}: {e}"}
        self.cases.setdefault(fn, []).append({"args": plain(list(args)), "out": out})

    def write(self, source):
        os.makedirs(OUT, exist_ok=True)
        path = os.path.join(OUT, self.name + ".json")
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"group": self.name, "source": source, "cases": self.cases}, f, ensure_ascii=False,
                      separators=(",", ":"), sort_keys=True)
            f.write("\n")
        n = sum(len(v) for v in self.cases.values())
        print(f"  {self.name}: {n} cases, {os.path.getsize(path) // 1024} KB", flush=True)


# -- shared inputs -----------------------------------------------------------------------

PROSE = corpus("prose.txt")
TEXTS = {"prose": PROSE, "edge.md": corpus("edge.md"), "edge.tex": corpus("edge.tex"),
         "edge.html": corpus("edge.html"), "sample.md": example("sample.md"), "sample.tex": example("sample.tex"),
         "code.py": corpus("code.py"), "code.c": corpus("code.c"), "code.sql": corpus("code.sql")}
PARAGRAPHS = [p for p in PROSE.split("\n\n") if p.strip()]
SENTENCES = None  # filled from textutil once it is imported


def sentences():
    global SENTENCES
    if SENTENCES is None:
        from rewrite import textutil
        SENTENCES = [PROSE[s:e] for s, e in textutil.sentence_bounds(PROSE)]
    return SENTENCES


def word_list(n_freq=1200, every=40):
    """Frequent words, a spread of the lemma dictionary, and some made-up and newer words."""
    from rewrite import thesaurus
    T = thesaurus.get()
    freq = sorted((w for w in T.F if w.isalpha()), key=lambda w: (-T.F[w], w))[:n_freq]
    from lemminflect import Lemmatizer
    lemma_keys = sorted(k for k in Lemmatizer()._getLemmaDict() if k.isalpha())[::every]
    extra = ["blorfing", "googled", "tweeted", "zoomers", "datasets", "unfriended", "glomped", "criteria",
             "phenomena", "analyses", "indices", "radii", "geese", "mice", "better", "best", "worse", "went",
             "children", "data", "running", "ran", "swum", "lying", "lay", "flies", "studied", "quizzes",
             "happier", "gladdest", "boxes", "buses", "heroes", "knives", "wolves", "theses", "crises", "be",
             "is", "was", "were", "am", "could", "might", "would", "should", "must", "ought", "dare", "can't"]
    seen, out = set(), []
    for w in freq + lemma_keys + extra:
        if w not in seen:
            seen.add(w)
            out.append(w)
    return out


def typos(words, n=400, seed=7):
    """Misspellings made from frequent words by one deletion, transposition, replacement or insertion."""
    rng = random.Random(seed)
    out = []
    pool = [w for w in words if len(w) >= 4][:600]
    for _ in range(n):
        w = rng.choice(pool)
        i = rng.randrange(len(w))
        op = rng.randrange(4)
        c = chr(97 + rng.randrange(26))
        if op == 0:
            t = w[:i] + w[i + 1:]
        elif op == 1 and i < len(w) - 1:
            t = w[:i] + w[i + 1] + w[i] + w[i + 2:]
        elif op == 2:
            t = w[:i] + c + w[i + 1:]
        else:
            t = w[:i] + c + w[i:]
        if t != w:
            out.append(t)
    return sorted(set(out))


# -- groups ------------------------------------------------------------------------------

def g_textutil():
    from rewrite import textutil as tu
    g = Group("textutil")
    for name, text in TEXTS.items():
        g.add("words", [text], lambda t=text: tu.words(t))
        for lines in (False, True):
            g.add("sentence_bounds", [text, lines], lambda t=text, ln=lines: tu.sentence_bounds(t, lines=ln))
    for pos in range(0, len(PROSE), 37):
        g.add("sentence_at", [PROSE, pos], lambda p=pos: tu.sentence_at(PROSE, p))
    for tmpl in ("word", "Word", "WORD", "wORD", "W", "w", "", "McDonald"):
        for word in ("example", "Example", "EXAMPLE", "e.g.", "a lot", "iPhone"):
            g.add("match_case", [tmpl, word], lambda a=tmpl, b=word: tu.match_case(a, b))
    sents = sentences()
    for a, b in zip(sents, sents[1:] + sents[:1]):
        g.add("word_diff_ratio", [a, b], lambda x=a, y=b: tu.word_diff_ratio(x, y))
        g.add("word_diff_ratio", [a, a.replace("the", "a")], lambda x=a: tu.word_diff_ratio(x, x.replace("the", "a")))
    g.add("stopwords", [], lambda: sorted(tu.STOPWORDS))
    return g


def _edits(text, seed):
    """A changed copy of a text: words swapped, dropped, added and replaced."""
    rng = random.Random(seed)
    words = text.split(" ")
    for _ in range(max(1, len(words) // 6)):
        i = rng.randrange(len(words))
        op = rng.randrange(4)
        if op == 0:
            words[i] = words[i].upper()
        elif op == 1 and len(words) > 2:
            del words[i]
        elif op == 2:
            words.insert(i, rng.choice(["very", "quite", "really", "new", "the"]))
        else:
            j = rng.randrange(len(words))
            words[i], words[j] = words[j], words[i]
    return " ".join(words)


def g_diff():
    from rewrite import app
    g = Group("diff")
    pairs = [(p, _edits(p, k)) for k, p in enumerate(PARAGRAPHS)]
    pairs += [(PROSE, "\n\n".join(_edits(p, 100 + k) for k, p in enumerate(PARAGRAPHS))), ("", "new text"),
              ("old text", ""), ("same", "same"), ("a b c", "a  b c"), ("It is good.", "It's good!")]
    for old, new in pairs:
        g.add("compute_changes", [old, new], lambda a=old, b=new: app.compute_changes(a, b))
        g.add("diff_chunks", [old, new], lambda a=old, b=new: app.diff_chunks(a, b))
    return g


def g_phrases():
    from rewrite import phrases
    g = Group("phrases")
    g.add("book", [], lambda: phrases.BOOK)
    for name, text in TEXTS.items():
        g.add("find", [text], lambda t=text: list(phrases.find(t)))
    for key in list(phrases.BOOK)[::3] + ["In  Order To", "not a phrase", " due to the fact that "]:
        g.add("lookup", [key], lambda k=key: phrases.lookup(k))
    return g


def g_formats():
    from rewrite import formats
    g = Group("formats")
    for path in ("a.tex", "b.MD", "c.html", "d.py", "e.rs", "f.txt", "g", "h.docx", "i.pdf", "j.markdown",
                 "k.ltx", "l.htm", "m.sql", "n.lua", "o.sh", "p.c", "q.json", "r.yaml", "s.rmd", "t.xml"):
        g.add("kind_for", [path], lambda p=path: formats.kind_for(p))
    for name, text in TEXTS.items():
        g.add("sniff_kind", [text], lambda t=text: formats.sniff_kind(t))
    kinds = {"prose": [("plain", None), ("markdown", None)], "edge.md": [("markdown", None)],
             "sample.md": [("markdown", None)], "edge.tex": [("latex", None)], "sample.tex": [("latex", None)],
             "edge.html": [("html", None)], "code.py": [("code", "code.py")], "code.c": [("code", "code.c")],
             "code.sql": [("code", "code.sql")]}
    for name, ks in kinds.items():
        for kind, path in ks:
            text = TEXTS[name]
            g.add("protected_spans", [text, kind, path], lambda t=text, k=kind, p=path: formats.protected_spans(t, k, p))
    for spans in ([], [(0, 5)], [(5, 9), (0, 3), (2, 6)], [(0, 1), (1, 2), (10, 20), (15, 30)]):
        g.add("merge", [spans], lambda s=spans: formats.merge(s))
        merged = formats.merge(spans)
        for a, b in ((0, 1), (3, 4), (6, 12), (25, 40), (2, 2)):
            g.add("overlaps", [merged, a, b], lambda s=merged, x=a, y=b: formats.overlaps(s, x, y))
            g.add("clip", [merged, a, b], lambda s=merged, x=a, y=b: formats.clip(s, x, y))
    samples = [b"plain ascii", "caf\u00e9 na\u00efve".encode("utf-8"), b"\xef\xbb\xbfwith bom",
               "utf-16 text".encode("utf-16"), "smart \u201cquotes\u201d".encode("cp1252"), b"\x81\x8d\x8f\x90\x9d",
               b"bin\x00ary", b"line\r\nbreaks\r\n", b"", "\u00e9t\u00e9".encode("latin-1")]
    for raw in samples:
        g.add("decode", [raw], lambda r=raw: formats._decode(r))
    return g


def g_docmodel():
    from rewrite import docmodel as dm
    g = Group("docmodel")
    for level in range(5):
        for n in (1, 2, 9, 26, 27, 52, 53, 702, 703, 3999):
            g.add("number_label", [n, level], lambda a=n, b=level: dm.number_label(a, b))
    for n in (0, 1, 4, 9, 14, 40, 90, 400, 1994, 2026, 3999, 4000):
        g.add("roman", [n], lambda a=n: dm.roman(a))
    for w, h in ((595.28, 841.89), (841.89, 595.28), (612, 792), (612, 1008), (419.53, 595.28), (600, 800),
                 (612.5, 792.5), (500, 500)):
        g.add("page_name", [w, h], lambda a=w, b=h: dm.PageSetup(width=a, height=b).name())
    for w, h, mw, mh in ((800, 600, 400, None), (100, 50, 400, None), (800, 600, 400, 100), (0, 0, 10, 10)):
        def fit(w=w, h=h, mw=mw, mh=mh):
            im = dm.Image(width=w, height=h)
            im.fit(mw, mh)
            return [im.width, im.height]
        g.add("image_fit", [w, h, mw, mh], fit)
    g.add("styles", [], lambda: list(dm.STYLES))
    g.add("style_label", [], lambda: dm.STYLE_LABEL)
    g.add("look", [], lambda: dm.LOOK)
    g.add("page_sizes", [], lambda: dm.PAGE_SIZES)
    g.add("bullets", [], lambda: list(dm.BULLETS))
    R = dm.Run
    contents = [
        [R(text="  Hello "), R(text="world", b=True), R(text="  ")],
        [R(text="Figure 3: "), R(text="A plot", i=True)],
        [R(text="a"), R(text="b"), R(text="", b=True), R(text="c", b=True), R(text="d", b=True)],
        [R(text=" "), dm.Image(data=b"x", width=10, height=10), R(text=" \n ")],
        [R(text="Table 2. "), R(text="Results")],
        [R(text="line"), dm.Break(), R(text="next"), dm.Note(runs=[R(text="note")]),
         dm.Opaque(text="[obj]", label="object")],
        [dm.Break(kind="page"), R(text="  ")],
    ]
    for c in contents:
        nat = dm.to_native(c)
        g.add("inline_text", [nat, True], lambda c=c: dm.inline_text(c, True))
        g.add("inline_text", [nat, False], lambda c=c: dm.inline_text(c, False))
        g.add("merge_runs", [nat], lambda c=c: dm.to_native(dm.merge_runs(c)))
        g.add("strip_content", [nat], lambda c=c: dm.to_native(dm.strip_content(c)))
        g.add("caption_text", [nat], lambda c=c: dm.to_native(dm.caption_text(c)))
    for text in ("one\n\ntwo\nlines\n\n\n three ", "", "single", PARAGRAPHS[0] + "\n\n" + PARAGRAPHS[1]):
        g.add("plain_paragraphs", [text], lambda t=text: dm.to_native(dm.plain_paragraphs(t)))
    P = dm.Paragraph
    blocks = [P(content=[R(text="intro")]), P(content=[R(text="a")], list=("number", 0)),
              P(content=[R(text="b")], list=("number", 1)), P(content=[R(text="c")], list=("number", 1)),
              P(content=[R(text="d")], list=("bullet", 2)), P(content=[R(text="e")], list=("number", 0)),
              dm.Rule(), P(content=[R(text="f")], list=("number", 0)), P(content=[R(text="g")], list=("bullet", 0)),
              P(content=[R(text="h")], list=("bullet", 1)), P(content=[R(text="i")], list=("number", 2))]

    def labels():
        got = dm.list_labels(blocks)
        return [got.get(id(b)) for b in blocks]
    g.add("list_labels", [dm.to_native(blocks)], labels)
    doc = dm.Document(blocks=blocks + [dm.Table(rows=[[[P(content=[R(text="x")])], [P(content=[R(text="y")])]]]),
                                       dm.CodeBlock(text="code", lang="py"), dm.MathBlock(tex="x^2")])
    g.add("document_text", [dm.to_native(doc)], lambda: dm.document_text(doc))
    return g


def g_mdrender():
    from rewrite import mdrender
    g = Group("mdrender")
    for name in ("edge.md", "sample.md", "prose"):
        g.add("parse", [TEXTS[name]], lambda t=TEXTS[name]: mdrender.parse(t))
    for snippet in ("**a *b* c**", "`x` and ``y`z``", "[a](b) [c][d] <http://e>", "x_y_z and _a_",
                    "$a$ and \\$b", "![alt](src.png)", "~~gone~~ ok", "[^1] note", "a<br/>b", "&lt;tag&gt;",
                    "***bi***", "www.example.com/path.", "trailing \\\\"):
        g.add("inline", [snippet], lambda s=snippet: mdrender.inline(s))
    return g


def g_latexbuild():
    from rewrite import latexbuild
    g = Group("latexbuild")
    logs = os.path.join(CORPUS, "logs")
    for name in sorted(os.listdir(logs)):
        if name.endswith(".log"):
            with open(os.path.join(logs, name), encoding="utf-8", errors="replace") as f:
                log = f.read()
            main = "errors.tex" if name == "classic.log" else name[:-4] + ".tex"
            g.add("parse_log", [log, main], lambda lg=log, m=main: latexbuild.parse_log(lg, m))
    real_find = latexbuild.find
    latexbuild.find = lambda name: name  # every engine installed: record the preference, not this machine
    try:
        sources = [TEXTS["edge.tex"], TEXTS["sample.tex"], "% !TEX TS-program = lualatex\n\\documentclass{x}",
                   "%!TEX program=pdflatex\n\\usepackage{fontspec}", "\\usepackage{polyglossia}\\begin{document}",
                   "% \\usepackage{fontspec}\n\\begin{document}", "\\usepackage{unicode-math}", "plain text",
                   "% !TEX program = context\n\\usepackage{xeCJK}"]
        for src in sources:
            g.add("pick_engine", [src], lambda s=src: latexbuild.pick_engine(s))
    finally:
        latexbuild.find = real_find
    return g


TAGS = ["NN", "NNS", "NNP", "NNPS", "VB", "VBD", "VBG", "VBN", "VBP", "VBZ", "JJ", "JJR", "JJS", "RB", "RBR",
        "RBS", "MD", "RP"]
UPOS = ["NOUN", "VERB", "ADJ", "ADV", "AUX", "PROPN"]


def g_lemminflect():
    import lemminflect as LI
    g = Group("lemminflect")
    words = word_list()
    for w in words:
        g.add("getAllLemmas", [w], lambda x=w: LI.getAllLemmas(x))
        g.add("getAllInflections", [w], lambda x=w: LI.getAllInflections(x))
    for w in words[::4]:
        for u in UPOS[:4]:
            g.add("getAllLemmas", [w, u], lambda x=w, y=u: LI.getAllLemmas(x, y))
            g.add("getAllLemmasOOV", [w, u], lambda x=w, y=u: LI.getAllLemmasOOV(x, y))
            g.add("getAllInflections", [w, u], lambda x=w, y=u: LI.getAllInflections(x, y))
            g.add("getAllInflectionsOOV", [w, u], lambda x=w, y=u: LI.getAllInflectionsOOV(x, y))
    for w in words[::6] + ["Run", "RUN", "Analysis"]:
        for t in TAGS:
            for oov in (True, False):
                g.add("getInflection", [w, t, oov], lambda x=w, y=t, z=oov: LI.getInflection(x, y, inflect_oov=z))
    return g


def _cands(cs):
    """Candidates as [text, relation, sense rank, pos, start of the gloss] (senses records the whole gloss)."""
    return [[c.text, c.relation, c.sense_rank, c.pos, c.gloss[:24]] for c in cs]


def g_thesaurus():
    from rewrite import thesaurus
    T = thesaurus.get()
    g = Group("thesaurus")
    words = word_list(n_freq=500, every=400)
    for w in words:
        g.add("lemmas", [w], lambda x=w: T.lemmas(x))
        g.add("tags", [w], lambda x=w: T.tags(x))
        g.add("zipf", [w], lambda x=w: T.zipf(x))
        g.add("antonyms", [w], lambda x=w: T.antonyms(x))
        g.add("senses", [w], lambda x=w: T.senses(x))
        g.add("candidates", [w, 6, True, 40], lambda x=w: _cands(T.candidates(x, max_per_sense=6, limit=40)))
    for w in words[::3]:
        g.add("candidates", [w, 8, True, 60], lambda x=w: _cands(T.candidates(x)))
        g.add("candidates", [w, 8, False, 60], lambda x=w: _cands(T.candidates(x, related=False)))
    for a, b in (("big", "large"), ("big", "small"), ("happy", "glad"), ("run", "sprint"), ("dog", "animal"),
                 ("good", "bad"), ("increase", "decrease"), ("car", "automobile"), ("fast", "quick"),
                 ("running", "runs"), ("study", "studies"), ("analysis", "analyses"), ("organise", "organize"),
                 ("light", "heavy"), ("light", "dark"), ("show", "demonstrate")):
        g.add("relation", [a, b], lambda x=a, y=b: T.relation(x, y))
        g.add("same_lemma", [a, b], lambda x=a, y=b: T.same_lemma(x, y))
        g.add("is_antonym", [a, b], lambda x=a, y=b: T.is_antonym(x, y))
    for lemma in ("run", "be", "go", "happy", "good", "child", "analysis", "carry out", "look up", "ice cream",
                  "well known", "study", "stop", "big"):
        for t in TAGS:
            g.add("inflect", [lemma, t], lambda x=lemma, y=t: T.inflect(x, y))
    ctx = ["the", "results", "show", "a", "improvement", "in", "accuracy", "river", "money", "bank"]
    for w in ("significant", "bank", "show", "improvement", "results", "light"):
        cands = T.candidates(w)
        g.add("lesk_rank", [w, ctx, [c.text for c in cands]],
              lambda x=w, cs=cands: [(c.text, c.score) for c in T.lesk_rank(x, ctx, cs)])
    return g


def _speller(vocab=None):
    from rewrite import proof, thesaurus
    sp = proof.Speller(thesaurus.get(), ["rewriteword", "Samrat"])
    if vocab:
        sp.set_vocab(vocab)
    return sp


def g_speller():
    from rewrite import formats, proof
    g = Group("speller")
    sp = _speller()
    words = word_list(n_freq=1500, every=60)
    bad = typos(words)
    misspelt = sorted(proof.MISSPELLINGS)[::2]
    g.add("lists", [], lambda: {"function_words": sorted(proof.FUNCTION_WORDS),
                                "contractions": sorted(proof.CONTRACTIONS), "extra_words": sorted(proof.EXTRA_WORDS),
                                "misspellings": proof.MISSPELLINGS})
    for w in words + bad + misspelt + ["rewriteword", "Samrat", "don't", "dont", "it’s", "self-driving",
                                       "re-run", "unhappy", "overfitting", "teacher's", "students'",
                                       "bookshelf", "bookable", "Xyzzy", "hyper-parameter"]:
        g.add("known", [w], lambda x=w: sp.known(x))
    for w in bad[::2] + misspelt + ["Teh", "RECIEVE", "alot", "thankyou", "infront"]:
        g.add("suggest", [w, 8], lambda x=w: sp.suggest(x, 8))
    for w in words[:300]:
        g.add("zipf", [w], lambda x=w: sp.zipf(x))
    kinds = {"prose": "plain", "edge.md": "markdown", "edge.tex": "latex", "edge.html": "html",
             "sample.md": "markdown", "sample.tex": "latex"}
    for name, kind in kinds.items():
        text = TEXTS[name]
        prot = formats.protected_spans(text, kind)
        g.add("check", [text, prot], lambda t=text, p=prot: sp.check(t, p))
    return g


def g_grammar():
    from rewrite import formats, proof
    g = Group("grammar")
    sp = _speller()
    texts = dict(TEXTS)
    extra = ["You should of known. It's own fault. Their is a cat. There own house. Your welcome.",
             "He was suppose to come. i think so. i'm sure. That is more better than this.",
             "Hello , world . A list,,of things;;here. a,b and x,y. one,two words.",
             "This is is a test. Had had happened. That that is fine. Very very good.",
             "A orange. An banana. A hour. An historic event. A FBI agent. An unicorn. A one-off. An X-ray.",
             "He are here. She have it. It do work. We is ready. They was late. You has it. I is fine. I are.",
             "She didn't went. He doesn't likes it. They have went. She has saw it. We had did it.",
             "Appendix A shows it. See (A) for details. The end. the next sentence starts small.",
             "it is bigger then that. Its a test. Its also fine. It's not."]
    for k, t in enumerate(extra):
        texts[f"extra{k}"] = t
    for name, text in texts.items():
        kind = {"edge.md": "markdown", "sample.md": "markdown", "edge.tex": "latex", "sample.tex": "latex",
                "edge.html": "html"}.get(name, "plain")
        path = name if name.startswith("code.") else None
        prot = formats.protected_spans(text, "code" if path else kind, path)
        g.add("grammar_rules", [text, prot], lambda t=text, p=prot: proof.grammar_rules(t, p, sp))
        g.add("grammar_rules_nospeller", [text, prot], lambda t=text, p=prot: proof.grammar_rules(t, p, None))
    return g


def _engine(smart):
    from rewrite import engine, models
    if not smart:
        real = models.present
        models.present = lambda key: False
    try:
        e = engine.Engine(["rewriteword"])
        e.load_models()
    finally:
        if not smart:
            models.present = real
    return e


def _targets(text, n=60):
    """(start, end) of single words and short phrases spread over a text."""
    from rewrite import textutil
    ws = [(s, e) for s, e, w in textutil.words(text) if len(w) >= 4]
    out = ws[::max(1, len(ws) // n)][:n]
    phrases = [(m.start(), m.end()) for m in __import__("re").finditer(
        r"in order to|a significant improvement|the vast majority of|Due to the fact that|At this point in time|"
        r"a large number of|make sure|tend to cluster around rare words", text)]
    return out + phrases


def _engine_cases(g, e, smart):
    from rewrite import formats, textutil
    for a, b in _targets(PROSE, 60 if smart else 120):
        g.add("suggest", [PROSE, a, b, 12, True], lambda x=a, y=b: e.suggest(PROSE, x, y, n=12))
        g.add("suggest", [PROSE, a, b, 5, False], lambda x=a, y=b: e.suggest(PROSE, x, y, n=5, generate=False))
    for word in ("recieve", "alot", "definately", "Teh", "accomodate", "seperate", "wierd", "goverment"):
        i = PROSE.find(word)
        text = PROSE if i >= 0 else f"I will {word} it tomorrow."
        i = text.find(word)
        g.add("spell_suggest", [text, i, i + len(word), 8], lambda t=text, x=i, w=word: e.spell_suggest(t, x, x + len(w), n=8))
    sents = [s for s in sentences() if len(s.split()) >= 4][:12 if smart else 30]
    for s in sents:
        g.add("paraphrase", [s, [], 5, 0], lambda x=s: e.paraphrase(x, (), n=5))
    tex = TEXTS["edge.tex"]
    tex_prot = formats.protected_spans(tex, "latex")
    for s, t in textutil.sentence_bounds(tex)[8:14]:
        spans = [(max(a, s) - s, min(b, t) - s) for a, b in tex_prot if a < t and b > s]
        g.add("paraphrase", [tex[s:t], spans, 5, 0], lambda x=tex[s:t], sp=spans: e.paraphrase(x, sp, n=5))
        g.add("grammar", [tex[s:t], spans], lambda x=tex[s:t], sp=spans: e.grammar(x, sp))
    for s in sentences():
        g.add("grammar", [s, []], lambda x=s: e.grammar(x, ()))
    for w, prev in (("is", "it"), ("are", "they"), ("went", "has"), ("go", "to"), ("studies", "the"),
                    ("children", "many"), ("their", ""), ("effect", "an"), ("ran", "have"), ("data", "the")):
        g.add("_alternatives", [w, prev], lambda x=w, y=prev: e._alternatives(x, y))
    region = PARAGRAPHS[:6] if smart else PARAGRAPHS
    text = "\n\n".join(region)
    prot = formats.protected_spans(text, "plain")
    for strength in ("light", "medium", "strong"):
        for sentences_on in ((False, True) if smart else (False,)):
            opts = {"scope": "all", "strength": strength, "phrases": True, "words": True,
                    "sentences": sentences_on, "apply": "all", "lines": False}
            g.add("auto_rewrite", [text, [[0, len(text)]], prot, opts],
                  lambda o=opts: e.auto_rewrite(text, [(0, len(text))], prot, o))


def g_engine_lite():
    g = Group("engine_lite")
    _engine_cases(g, _engine(False), False)
    return g


def g_engine_smart():
    from rewrite import cleanup, models
    if not all(models.present(k) for k in models.MODELS):
        print("  engine_smart: skipped, the models are not downloaded", flush=True)
        return None
    e = _engine(True)
    g = Group("engine_smart")
    _engine_cases(g, e, True)
    sents = sentences()[:10]
    g.add("emb.similarity", [sents[0], sents[1:]], lambda: e.emb.similarity(sents[0], sents[1:]))
    g.add("emb.encode", [sents[:4]], lambda: [list(v[:8]) for v in e.emb.encode(sents[:4])])
    for a, b in _targets(PROSE, 20):
        g.add("emb.span_similarity", [PROSE, a, b, ["better", "great"]],
              lambda x=a, y=b: e.emb.span_similarity(PROSE, x, y, ["better", "great"]))
        g.add("mlm.fill", [PROSE, [[a, b, [PROSE[a:b], "thing", "result"]]]],
              lambda x=a, y=b: e.mlm.fill(PROSE, [(x, y, [PROSE[x:y], "thing", "result"])], topk=5))
        g.add("mlm.substitutes", [PROSE, a, b, ["thing", "result", "outcome"]],
              lambda x=a, y=b: e.mlm.substitutes(PROSE, x, y, ["thing", "result", "outcome"], topk=10))
    for s in sents[:6]:
        g.add("t5.generate", [s, 3], lambda x=s: e.t5.generate(x, n=3))
    g.add("mlm.known_words", [], lambda: sorted(e.mlm.known_words())[::50])
    h = cleanup.Helper(e)
    for before, opts, after in (("The method is ", ["effective", "eff ective"], " in practice."),
                                ("We found a signifi", ["cant", "cant result"], " gain."),
                                ("It works well, e.g.", [" in", "in"], " most cases.")):
        g.add("helper.choose", [before, opts, after], lambda b=before, o=opts, a=after: h.choose(b, o, a))
    return g


class _WordHelper:
    """A stand-in for cleanup.Helper with a fixed dictionary (no language engine), so the PDF conversion's
    word fixing can be recorded and reproduced in Rust."""

    def __init__(self, words):
        self.words = set(words)

    def known(self, word):
        return word.lower() in self.words

    def zipf(self, word):
        return 4.0 if self.known(word) else 0.0

    def choose(self, before, options, after):
        return -1


def g_pdfdoc():
    import hashlib
    import re
    import struct
    import types
    import pypdfium2 as pdfium
    from rewrite import docmodel as dm, native, pdfdoc
    from rewrite.ui import pdfmode
    folder = os.path.join(CORPUS, "pdf")
    g = Group("pdfdoc")

    def r3(x):
        """Floats rounded to 3 decimals; PNG bytes as {"png": [width, height], "sha1": ...}; other bytes hashed."""
        if isinstance(x, float):
            return round(x, 3)
        if isinstance(x, (bytes, bytearray)):
            x = bytes(x)
            out = {"sha1": hashlib.sha1(x).hexdigest(), "len": len(x)}
            if x[:8] == b"\x89PNG\r\n\x1a\n":
                out["png"] = list(struct.unpack(">II", x[16:24]))
            return out
        if isinstance(x, dict):
            return {k: r3(v) for k, v in x.items()}
        if isinstance(x, (list, tuple)):
            return [r3(v) for v in x]
        return x

    names = sorted(n for n in os.listdir(folder) if n.endswith(".pdf"))
    for name in names:
        with open(os.path.join(folder, name), "rb") as f:
            data = f.read()
        doc = pdfium.PdfDocument(data)
        try:
            n = len(doc)
            g.add("page_sizes", [name], lambda: r3([doc.get_page_size(i) for i in range(n)]))
            g.add("read_pages", [name, True], lambda: r3(pdfdoc.read_pages(doc, True)))
            g.add("read_pages", [name, False], lambda: r3(pdfdoc.read_pages(doc, False)))
            render = pdfdoc._renderer(doc)
            for box in ((0, 0, 1000, 1000), (72.5, 100.25, 300.75, 260.5), (520, 800, 620, 850), (10, 10, 11, 50)):
                g.add("renderer", [name, 0, *box], lambda b=box: r3(render(0, *b)))
            pages = native.pdf_pages(data)
            for p in range(n):
                left, _, _, top = pages[p][:4]
                fake = types.SimpleNamespace(_chars={}, _edoc=None, _edoc_version=None, version=1, data=data,
                                             pages=pages)
                if p == 0:
                    g.add("chars", [name, p, left, top], lambda p=p, s=fake: r3(pdfmode.PdfEditor.chars(s, p)))
                page = doc[p]
                tp = page.get_textpage()
                pw, ph = page.get_size()
                for rect in ((0, 0, pw, ph), (60, ph - 200, pw - 60, ph - 60), (100, 100, 101, 101)):
                    g.add("text_bounded", [name, p, *rect], lambda t=tp, rc=rect: t.get_text_bounded(*rc))
                tp.close()
                page.close()
        finally:
            doc.close()
        g.add("first_page_png", [name, 2.0], lambda: r3(pdfdoc.first_page_png(data, 2.0)))
        g.add("first_page_png", [name, 0.75], lambda: r3(pdfdoc.first_page_png(data, 0.75)))
        text = "".join(c[0] for pg in pdfium.PdfDocument(data) for c in pdfdoc._chars(pg.get_textpage(), 0))
        words = sorted({w.lower() for w in re.findall(r"[A-Za-z]+", text)})
        for opts, helper in (({}, None), ({}, words), ({"images": False, "tables": False}, None),
                             ({"drop_headers": False, "fix_words": False}, None)):
            h = _WordHelper(helper) if helper is not None else None
            g.add("to_document", [name, opts, helper],
                  lambda o=opts, hh=h: r3(dm.to_native(pdfdoc.to_document(data, o, hh))))
    g.add("first_page_png", ["not a pdf", 2.0], lambda: pdfdoc.first_page_png(b"not a pdf", 2.0))
    return g


GROUPS = {"textutil": g_textutil, "diff": g_diff, "phrases": g_phrases, "formats": g_formats,
          "docmodel": g_docmodel, "mdrender": g_mdrender, "latexbuild": g_latexbuild,
          "lemminflect": g_lemminflect, "thesaurus": g_thesaurus, "speller": g_speller, "grammar": g_grammar,
          "engine_lite": g_engine_lite, "engine_smart": g_engine_smart, "pdfdoc": g_pdfdoc}


def main():
    if os.environ.get("PYTHONHASHSEED") != "0":
        # Several modules break ties in set order, which follows the string hash seed: pin it so that
        # recording twice gives the same fixtures.
        sys.exit(subprocess.call([sys.executable, *sys.argv], env=dict(os.environ, PYTHONHASHSEED="0")))
    names = sys.argv[1:] or list(GROUPS)
    unknown = [n for n in names if n not in GROUPS]
    if unknown:
        sys.exit(f"unknown group(s): {', '.join(unknown)}; choose from {', '.join(GROUPS)}")
    try:
        source = subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, text=True).strip()
    except Exception:
        source = "unknown"
    print(f"Recording fixtures from {source} into {OUT}", flush=True)
    for name in names:
        g = GROUPS[name]()
        if g:
            g.write(source)


if __name__ == "__main__":
    main()
