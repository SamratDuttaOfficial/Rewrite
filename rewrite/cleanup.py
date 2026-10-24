"""The language side of conversions: answers the Rust core's doubtful cases (is "co-operate" one word or
two? does a sentence end after "al."? is "thequick" two words?) with the spelling dictionary and, when
it is loaded, the context model; and splits paragraphs into sentences for "one sentence per line"."""
from . import native


class Helper:
    """Passed to native.pdf_document and native.sentence_starts (they call known, zipf and choose)."""

    def __init__(self, engine, use_model=True):
        self.e = engine
        self.model = engine.mlm if use_model else None
        self._known, self._zipf, self._choose = {}, {}, {}

    def known(self, word):
        w = word.lower()
        if w not in self._known:
            self._known[w] = bool(self.e.speller.known(w))
        return self._known[w]

    def zipf(self, word):
        w = word.lower()
        if w not in self._zipf:
            self._zipf[w] = float(self.e.speller.zipf(w)) if self.known(w) else 0.0
        return self._zipf[w]

    def choose(self, before, options, after):
        """The index of the option the context model finds clearly more likely here, or -1."""
        if not self.model or not options:
            return -1
        key = (before[-120:], tuple(options), after[:120])
        if key in self._choose:
            return self._choose[key]
        before, after = before[-300:], after[:300]
        sep = "" if not before or before.endswith((" ", "\n")) else " "
        text = before + sep + options[0] + (" " if after and not after.startswith((" ", ".", ",")) else "") + after
        start = len(before) + len(sep)
        try:
            scores = self.model.fill(text, [(start, start + len(options[0]), list(options))])[0][0]
        except Exception:
            scores = {}
        best, k = -1, -1
        ranked = sorted(((scores.get(o, -99.0), i) for i, o in enumerate(options)), reverse=True)
        if ranked and ranked[0][0] > -99.0:
            if len(ranked) == 1 or ranked[0][0] - ranked[1][0] >= 1.0:  # only a clear preference counts
                best, k = ranked[0][0], ranked[0][1]
        del best
        self._choose[key] = k
        return k

    def sentences(self, text):
        """Character offsets where sentences start (for writers' "one sentence per line")."""
        return native.sentence_starts(text, self)
