"""Offline thesaurus built from WordNet, with inflection-aware synonyms (via lemminflect)."""
import gzip
import json
import os
import re

try:
    import lemminflect as LI
except Exception:  # lemminflect is optional; we fall back to crude rules
    LI = None

DATA = os.path.join(os.path.dirname(__file__), "data", "thesaurus.json.gz")
WN2UPOS = {"n": "NOUN", "v": "VERB", "a": "ADJ", "r": "ADV"}
UPOS2WN = {v: k for k, v in WN2UPOS.items()}
TAG_UPOS = {
    "NN": "NOUN", "NNS": "NOUN", "VB": "VERB", "VBD": "VERB", "VBG": "VERB", "VBN": "VERB",
    "VBP": "VERB", "VBZ": "VERB", "JJ": "ADJ", "JJR": "ADJ", "JJS": "ADJ", "RB": "ADV",
    "RBR": "ADV", "RBS": "ADV",
}


class Candidate:
    __slots__ = ("text", "relation", "gloss", "sense_rank", "pos", "score", "lm", "sim", "source")

    def __init__(self, text, relation, gloss="", sense_rank=0, pos=""):
        self.text, self.relation, self.gloss, self.sense_rank, self.pos = text, relation, gloss, sense_rank, pos
        self.score = self.lm = self.sim = None
        self.source = "thesaurus"

    def __repr__(self):
        return f"<{self.text} {self.relation} {self.score}>"


class Thesaurus:
    def __init__(self, path=DATA):
        with gzip.open(path, "rt", encoding="utf-8") as f:
            d = json.load(f)
        self.S, self.I, self.A, self.F = d["S"], d["I"], d["A"], d["F"]
        self._ant_cache = {}

    # -- morphology -------------------------------------------------------------
    def lemmas(self, word):
        """{upos: [lemma, ...]} for a surface word."""
        w = word.lower()
        out = {}
        if LI:
            for upos, lem in LI.getAllLemmas(w).items():
                if upos in UPOS2WN:
                    out[upos] = list(lem)
        if w in self.I:
            for sid in self.I[w]:
                out.setdefault(WN2UPOS[self.S[sid][0]], [])
                if w not in out[WN2UPOS[self.S[sid][0]]]:
                    out[WN2UPOS[self.S[sid][0]]].append(w)
        if not out and LI:
            for upos in ("NOUN", "VERB", "ADJ", "ADV"):
                lem = LI.getAllLemmasOOV(w, upos).get(upos)
                if lem and lem[0] in self.I:
                    out[upos] = [lem[0]]
        return out

    def tags(self, word, lemmas=None):
        """Penn tags the surface form can carry, e.g. running -> {VBG, NN}."""
        w = word.lower()
        tags = set()
        lemmas = lemmas if lemmas is not None else self.lemmas(w)
        if LI:
            for upos, lems in lemmas.items():
                for lem in lems:
                    for tag, forms in LI.getAllInflections(lem, upos).items():
                        if w in forms:
                            tags.add(tag)
                    if lem == w:
                        tags.add({"NOUN": "NN", "VERB": "VB", "ADJ": "JJ", "ADV": "RB"}[upos])
        return tags

    def inflect(self, lemma, tag):
        parts = lemma.split(" ")
        upos = TAG_UPOS.get(tag)
        if tag in ("NN", "JJ", "RB", "VB", "VBP") or not LI:
            return lemma
        if upos == "ADJ" and len(parts) > 1:
            return None
        head = len(parts) - 1 if upos == "NOUN" else 0
        forms = LI.getInflection(parts[head], tag=tag)
        if not forms:
            return None
        parts[head] = forms[0]
        return " ".join(parts)

    def same_lemma(self, a, b):
        la, lb = self.lemmas(a), self.lemmas(b)
        sa = {x for v in la.values() for x in v} | {a.lower()}
        sb = {x for v in lb.values() for x in v} | {b.lower()}
        if sa & sb:
            return True
        # crude derivational check: demonstrate / demonstration, clear / clearly
        a, b = a.lower(), b.lower()
        n = min(len(a), len(b))
        common = len(os.path.commonprefix([a, b]))
        return n >= 4 and common >= max(4, n - 2)

    # -- lookups ------------------------------------------------------------------
    def zipf(self, word):
        return self.F.get(word.lower(), 0) / 10.0

    def antonyms(self, word):
        """Direct antonyms of the word and of its synonyms, plus WordNet 'indirect' antonyms
        (adjectives similar to an antonym, e.g. good -> bad -> terrible)."""
        key = word.lower()
        if key in self._ant_cache:
            return self._ant_cache[key]
        direct = set()
        own = self._synsets(word)
        for sid in own:
            for lem in self.S[sid][1]:
                direct.update(self.A.get(lem, []))
        for lems in self.lemmas(word).values():
            for lem in lems:
                direct.update(self.A.get(lem, []))
        out = set(direct)
        pos = {self.S[sid][0] for sid in own}
        for a in direct:
            for sid in self.I.get(a, []):
                p, lemmas, _, rel = self.S[sid]
                if p not in pos or sid in own:
                    continue
                out.update(lemmas)
                if p == "a":
                    for r in rel:
                        out.update(self.S[r][1])
        # never call the word's own synonyms antonyms
        for sid in own:
            out.difference_update(self.S[sid][1])
        self._ant_cache[key] = out
        return out

    def is_antonym(self, word, cand):
        ants = self.antonyms(word)
        if not ants:
            return False
        if cand.lower() in ants:
            return True
        for lems in self.lemmas(cand).values():
            if ants & set(lems):
                return True
        return False

    def relation(self, word, cand):
        """'syn' if they share a WordNet synset, 'rel' if one level apart, else None."""
        sw = self._synsets(word)
        sc = self._synsets(cand)
        if sw & sc:
            return "syn"
        for sid in sw:
            if set(self.S[sid][3]) & sc:
                return "rel"
        for sid in sc:
            if set(self.S[sid][3]) & sw:
                return "rel"
        return None

    def _synsets(self, word):
        out = set()
        for upos, lems in self.lemmas(word).items():
            for lem in lems:
                for sid in self.I.get(lem, []):
                    if WN2UPOS[self.S[sid][0]] == upos:
                        out.add(sid)
        return out

    def senses(self, word):
        """[(sid, pos, gloss, [lemmas])] ordered by sense frequency."""
        out = []
        for upos, lems in self.lemmas(word).items():
            for lem in lems:
                for sid in self.I.get(lem, []):
                    if WN2UPOS[self.S[sid][0]] == upos:
                        out.append((sid, self.S[sid][0], self.S[sid][2], self.S[sid][1]))
        return out

    def candidates(self, word, max_per_sense=8, related=True, limit=60):
        """Synonyms of `word`, inflected to match it, one Candidate per surface form."""
        w = word.lower()
        lemmas = self.lemmas(w)
        tags = self.tags(w, lemmas)
        seen, out = {w}, []
        ants = self.antonyms(w)
        own = {w} | {x for v in lemmas.values() for x in v}
        for upos, lems in lemmas.items():
            wnpos = UPOS2WN[upos]
            my_tags = [t for t in tags if TAG_UPOS.get(t) == upos] or [
                {"NOUN": "NN", "VERB": "VB", "ADJ": "JJ", "ADV": "RB"}[upos]]
            for lem in lems:
                for rank, sid in enumerate(self.I.get(lem, [])):
                    pos, syns, gloss, rel = self.S[sid]
                    if pos != wnpos:
                        continue
                    groups = [("syn", syns)]
                    if related:
                        groups += [("rel", self.S[r][1]) for r in rel]
                    for relation, group in groups:
                        n = 0
                        for s in group:
                            # skip the word itself, antonyms, and "savings bank" for "bank"
                            if s == lem or s in ants or (" " in s and own & set(s.split(" "))):
                                continue
                            for tag in my_tags:
                                form = self.inflect(s, tag)
                                if not form or form in seen:
                                    continue
                                seen.add(form)
                                out.append(Candidate(form, relation, gloss, rank, wnpos))
                                n += 1
                            if n >= max_per_sense:
                                break
        return out[:limit]

    def lesk_rank(self, word, context_words, cands):
        """Lite-mode ranking: sense overlap with context + sense prior + word frequency."""
        ctx = {c.lower() for c in context_words if len(c) > 2}
        gloss_overlap = {}
        for c in cands:
            if c.gloss not in gloss_overlap:
                g = set(re.findall(r"[a-z]+", c.gloss.lower()))
                gloss_overlap[c.gloss] = len(g & ctx)
        for c in cands:
            z = self.zipf(c.text.split(" ")[0])
            c.score = (
                1.5 * gloss_overlap[c.gloss]
                - 0.35 * c.sense_rank
                + (0.8 if c.relation == "syn" else 0.0)
                + 0.6 * min(z, 5.5)
                - (1.5 if z < 2.5 else 0.0)
                - 0.4 * c.text.count(" ")
            )
        return sorted(cands, key=lambda c: -c.score)


_T = None


def get():
    global _T
    if _T is None:
        _T = Thesaurus()
    return _T
