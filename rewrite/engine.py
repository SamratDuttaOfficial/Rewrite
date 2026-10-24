"""The rewriting engine: word, phrase and sentence suggestions plus whole-text auto-rewrite.

Smart mode (models downloaded):
  * words     - WordNet synonyms + RoBERTa proposals, scored by how well they fit the
                sentence (masked-LM pseudo-likelihood with the original visible), filtered
                by part of speech / inflection / antonyms, and checked for meaning with
                MiniLM contextual embeddings.
  * phrases   - phrase book + one-word swaps + T5 paraphrases of the phrase, ranked in context.
  * sentences - T5 paraphrases, ranked by meaning similarity and amount reworded.
Lite mode (no models): WordNet + phrase book ranked with a Lesk-style sense overlap.

Proofreading reuses the same parts: spelling corrections are ranked by the context model, and
grammar slips (agreement, verb forms, confused words) are found by asking that model whether a
sibling form of a word fits the sentence far better than the word that was written.
"""
import math
import re
import threading
from dataclasses import dataclass

from . import formats, models, phrases, proof, thesaurus
from .textutil import STOPWORDS, WORD_RE, match_case, sentence_bounds, word_diff_ratio

try:
    import lemminflect as LI
except Exception:
    LI = None

UPOS4 = {"NOUN", "VERB", "ADJ", "ADV"}
NUMBER_WORDS = set("""zero one two three four five six seven eight nine ten eleven twelve twenty thirty forty
fifty sixty seventy eighty ninety hundred hundreds thousand thousands million millions billion billions trillion
first second third fourth fifth half double twice once dozen dozens percent""".split())
KIND = {"syn": "synonym", "rel": "related", None: "in context", "phrase": "phrase"}

# Words that get mistaken for each other. The context model decides which one fits.
CONFUSED = [g.split() for g in """their there they're|its it's|your you're|then than|to too|affect effect|
affects effects|lose loose|accept except|advice advise|whether weather|quite quiet|were where|whose who's|
passed past|principle principal|led lead|choose chose|breath breathe|complement compliment""".split("|")]
# Function words that must agree with their subject or noun.
AGREEING = [g.split() for g in """is are|was were|has have|does do|doesn't don't|isn't aren't|wasn't weren't|
hasn't haven't|this these|much many""".split("|")]
BASE_AFTER = set("to not can could will would shall should may might must did does do didn't doesn't don't "
                 "cannot can't couldn't won't wouldn't shouldn't".split())
# kind of slip -> (how much better, in log-probability, the other form must fit; message)
_AGREE = "Agreement: “{alt}” fits this sentence better than “{word}”."
_NUMBER = "Singular or plural? “{alt}” fits this sentence better than “{word}”."
SLIPS = {
    "confused": (3.0, "Easily confused words: “{alt}” fits this sentence better than “{word}”."),
    "pair": (2.5, _AGREE), "agree": (2.5, _AGREE),
    "form": (2.5, "Verb form: “{alt}” fits this sentence better than “{word}”."),
    "singular": (3.5, _NUMBER), "plural": (3.0, _NUMBER),
}
NEEDS_POS = {"agree": "VERB", "form": "VERB", "singular": "NOUN", "plural": "NOUN"}
MAX_OWN_FIT, MIN_ALT_FIT = -3.0, -4.5  # the written word must be unlikely, the other form plausible
PLURAL_CUES = set("two three four five six seven eight nine ten many several few various these those both "
                  "multiple numerous dozen hundred thousand million".split())
SINGULAR_CUES = set("a an each every another".split())
_REF_CMD = re.compile(r"\\(cite\w*|[Cc]ref|ref|eqref|autoref|pageref|nameref|url|href|verb|texttt|lstinline|"
                      r"mintinline|path|SI|si|num|qty|ang)\b")
BONUS = {"syn": 2.0, "rel": 1.0, "phrase": 1.5, None: -0.5}


@dataclass
class Suggestion:
    text: str
    kind: str  # synonym | related | in context | phrase | rewrite
    fit: float  # 0..1 for display
    note: str = ""
    score: float = 0.0  # raw engine score (used for auto-rewrite thresholds)


def _normalize(s, start, end):
    """Collapse whitespace runs to single spaces, remapping start/end."""
    res, ns, ne, prev = [], None, None, False
    for i, ch in enumerate(s):
        if i == start:
            ns = len(res)
        if i == end:
            ne = len(res)
        if ch.isspace():
            if not prev:
                res.append(" ")
            prev = True
        else:
            res.append(ch)
            prev = False
    return "".join(res), (len(res) if ns is None else ns), (len(res) if ne is None else ne)


def _sigmoid(x):
    return 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, x))))


def _calibrate(items, center, spread):
    """Turn raw scores into 0..1 fits: mostly absolute, partly relative to the best one."""
    if not items:
        return
    best = max(x.score for x in items)
    for x in items:
        x.fit = 0.7 * _sigmoid((x.score - center) / spread) + 0.3 * _sigmoid((x.score - best) / spread + 2.0)


def _guess_pos(s, a, b):
    """Tiny rule-based part-of-speech guess for Lite mode (no models)."""
    word = s[a:b].lower()
    prev = re.findall(r"[A-Za-z']+", s[:a])[-1:] or [""]
    prev = prev[0].lower()
    if word.endswith("ly") and len(word) > 4:
        return {"ADV"}
    if prev in ("to", "will", "would", "can", "could", "should", "must", "may", "might", "not", "did", "does", "do"):
        return {"VERB"}
    if prev in ("the", "a", "an", "this", "that", "these", "those", "his", "her", "their", "its", "my", "your",
                "our", "of", "each", "every", "some", "any", "no"):
        return {"NOUN", "ADJ"}
    if prev in ("is", "are", "was", "were", "be", "been", "being", "am") and word.endswith(("ing", "ed")):
        return {"VERB", "ADJ"}
    if prev in ("very", "more", "most", "less", "so", "too", "quite", "really", "rather"):
        return {"ADJ", "ADV"}
    return None


def _pos_share(guesses):
    """Share of each part of speech among the model's guesses [(word, logprob)] for a position."""
    votes = {}
    for w, lp in guesses:
        ups = _upos(w)
        for u in ups:
            votes[u] = votes.get(u, 0.0) + math.exp(lp) / len(ups)
    total = sum(votes.values())
    return {u: v / total for u, v in votes.items()} if total else {}


def _upos(word):
    if not LI:
        return set()
    return set(LI.getAllLemmas(word.lower()).keys()) & UPOS4


def _pos_vote(proposals, target_upos):
    """Which parts of speech does the context want here? The model's own guesses vote."""
    votes = {}
    for w, lp in proposals[:15]:
        ups = _upos(w)
        for u in ups:
            votes[u] = votes.get(u, 0.0) + math.exp(lp) / len(ups)
    total = sum(votes.values())
    allowed = {u for u, v in votes.items() if total and v / total >= 0.2} & target_upos
    return allowed or target_upos


class Engine:
    STRENGTH = {  # share of eligible words to change, minimum raw score, minimum meaning similarity
        "light": (0.15, -3.5, 0.80),
        "medium": (0.30, -5.0, 0.76),
        "strong": (0.50, -6.5, 0.72),
    }

    def __init__(self, user_words=()):
        self.thes = thesaurus.get()
        self.speller = proof.Speller(self.thes, user_words)
        self._alts = {}
        self.mlm = self.emb = self.t5 = None
        self.errors = {}
        self._lock = threading.Lock()

    # -- models -------------------------------------------------------------------
    def load_models(self):
        """Load whatever models are present. Safe to call again after downloads."""
        with self._lock:
            if not models.runtime_available():
                self.errors["runtime"] = "onnxruntime/tokenizers are not installed"
                return
            from .nlp.embed import Embedder
            from .nlp.mlm import MaskedLM
            from .nlp.t5 import Paraphraser
            for key, attr, cls in (("context", "mlm", MaskedLM), ("similarity", "emb", Embedder),
                                   ("paraphrase", "t5", Paraphraser)):
                if getattr(self, attr) is None and models.present(key):
                    try:
                        setattr(self, attr, cls(models.folder(key)))
                        self.errors.pop(key, None)
                    except Exception as e:  # corrupt download, unsupported CPU, ...
                        self.errors[key] = str(e)
            if self.mlm and not self.speller.vocab:
                self.speller.set_vocab(self.mlm.known_words())

    @property
    def smart(self):
        return self.mlm is not None

    # -- dispatch -----------------------------------------------------------------
    def suggest(self, text, start, end, n=12, generate=True):
        """Suggestions for text[start:end]; `text` is the surrounding context (1-2 sentences)."""
        s, a, b = _normalize(text, start, end)
        if not s[a:b].strip():
            return []
        if " " in s[a:b].strip():
            return self.suggest_phrase(s, a, b, n, generate=generate)
        return self.suggest_word(s, a, b, n)

    # -- words --------------------------------------------------------------------
    def _word_rows(self, word, scores, proposals, cmap):
        """Filter raw model scores into [text, logprob, relation, gloss] rows."""
        T = self.thes
        allowed = _pos_vote(proposals, set(T.lemmas(word)) or UPOS4)
        target_tags = T.tags(word)
        rows, seen = [], set()
        for text, lp in list(scores.items()) + proposals:
            text = match_case(word, text)
            key = text.lower()
            if key in seen or key == word.lower():
                continue
            seen.add(key)
            if key in STOPWORDS or T.same_lemma(word, text) or T.is_antonym(word, text):
                continue
            if text in cmap:
                c = cmap[text]
                if c and thesaurus.WN2UPOS[c.pos] not in allowed:
                    continue
                relation, gloss = (c.relation, c.gloss) if c else ("phrase", "phrase book")
            else:
                ups = _upos(text)
                if ups and not (ups & allowed):
                    continue
                if target_tags:
                    ct = T.tags(text)
                    if ct and not (ct & target_tags):
                        continue
                relation, gloss = T.relation(word, text), ""
                if relation:
                    senses = T.senses(text)
                    gloss = senses[0][2] if senses else ""
            z = T.zipf(text.split(" ")[0])
            prelim = lp + BONUS[relation] - (2.5 if z < 2.5 else 1.0 if z < 3.2 else 0.0) - 0.7 * text.count(" ")
            rows.append([text, prelim, relation, gloss])
        return rows

    def suggest_word(self, s, a, b, n=12):
        word = s[a:b]
        T = self.thes
        cands = [c for c in T.candidates(word, max_per_sense=6, limit=40) if c.text.count(" ") <= 1]
        book = phrases.lookup(word)
        if not self.mlm:
            return self._lite_word(s, word, cands, book, n)

        cmap = {}
        for c in cands:
            cmap.setdefault(match_case(word, c.text), c)
        for x in book:
            cmap.setdefault(match_case(word, x), None)
        scores, _orig, proposals = self.mlm.substitutes(s, a, b, list(cmap), topk=25, exclude=[word], max_full=8)
        rows = sorted(self._word_rows(word, scores, proposals, cmap), key=lambda r: -r[1])[:max(16, n + 4)]
        if not rows:
            return []
        sims = [0.55] * len(rows)
        if self.emb:
            sims, _ = self.emb.span_similarity(s, a, b, [r[0] for r in rows])
        out = []
        for (text, prelim, relation, gloss), ts in zip(rows, sims):
            if self.emb and relation is None and ts < 0.5:
                continue  # a model guess that drifts in meaning (often an antonym: expensive -> free)
            out.append(Suggestion(text, KIND[relation], 0, gloss, prelim + 4.0 * (ts - 0.55)))
        _calibrate(out, -5.0, 1.5)
        out.sort(key=lambda x: -x.score)
        # keep dictionary-backed options; drop weak model-only guesses
        return [x for x in out if x.fit >= (0.15 if x.kind == "in context" else 0.08)][:n]

    def _lite_word(self, s, word, cands, book, n):
        a = s.find(word)
        pos = _guess_pos(s, a, a + len(word)) if a >= 0 else None
        if pos:
            keep = [c for c in cands if thesaurus.WN2UPOS[c.pos] in pos]
            cands = keep or cands
        cands = [c for c in cands if self.thes.zipf(c.text.split(" ")[0]) >= 2.8]
        ctx = [w for w in WORD_RE.findall(s) if w.lower() not in STOPWORDS]
        out, seen = [], set()
        for c in self.thes.lesk_rank(word, ctx, cands):
            if c.text not in seen:
                seen.add(c.text)
                score = c.score - (0.8 if "-" in c.text else 0.0)
                out.append(Suggestion(match_case(word, c.text), KIND[c.relation], 0, c.gloss, score))
        out.sort(key=lambda x: -x.score)
        _calibrate(out, 2.0, 1.2)
        out = [Suggestion(match_case(word, x), "phrase", 0.75, "phrase book", 3.0) for x in book] + out
        return out[:n]

    # -- phrases ------------------------------------------------------------------
    def _clean_generated(self, orig, gen):
        g = gen.strip().strip('"').strip()
        if not g or "\n" in g:
            return None
        if not re.search(r"[.!?;:]$", orig.strip()):
            g = re.sub(r"[.!?;:,]+$", "", g).strip()
        if orig[:1].islower() and g[:1].isupper() and not re.match(r"^(I\b|[A-Z]{2,})", g):
            g = g[0].lower() + g[1:]
        if g.lower() == orig.lower() or len(g.split()) > 2 * len(orig.split()) + 3:
            return None
        return g

    def _word_swaps(self, s, targets, per_word=2, min_score=-7.0, min_sim=0.0):
        """Batched, fast best-synonym search for several words of one sentence.

        targets: [(start, end)] -> {(start, end): [(text, score)]} (WordNet-backed only).
        """
        T = self.thes
        specs = []
        for a, b in targets:
            w = s[a:b]
            cm = {}
            for c in T.candidates(w, max_per_sense=6, limit=40):
                if " " not in c.text:
                    cm.setdefault(match_case(w, c.text), c)
            specs.append((a, b, w, cm))
        res = self.mlm.score_many(s, [(a, b, list(cm)) for a, b, _, cm in specs], topk=12)
        out = {}
        for (a, b, w, cm), (scores, props) in zip(specs, res):
            allowed = _pos_vote(props, set(T.lemmas(w)) or UPOS4)
            best = []
            for text, lp in scores.items():
                c = cm[text]
                if thesaurus.WN2UPOS[c.pos] not in allowed or T.zipf(text) < 3.0:
                    continue
                if T.same_lemma(w, text) or T.is_antonym(w, text):
                    continue
                score = lp + BONUS[c.relation]
                if score >= min_score:
                    best.append((text, score))
            best.sort(key=lambda x: -x[1])
            out[(a, b)] = best[:per_word + 1]
        if self.emb and min_sim:
            # meaning check: the swapped word must still "mean" the same thing in context
            keys = [k for k, v in out.items() if v]
            sims = self.emb.multi_span_similarity(s, [(a, b, [t for t, _ in out[(a, b)]]) for a, b in keys])
            for key, ss in zip(keys, sims):
                out[key] = [(t, sc) for (t, sc), x in zip(out[key], ss) if x >= min_sim]
        return {k: v[:per_word] for k, v in out.items()}

    def suggest_phrase(self, s, a, b, n=10, generate=True):
        phrase = s[a:b]
        alts = [(x, "phrase") for x in phrases.lookup(phrase)]
        if self.mlm:
            words = [(m.start(), m.end()) for m in WORD_RE.finditer(phrase)
                     if len(m.group()) >= 4 and m.group().lower() not in STOPWORDS][:3]
            swaps = self._word_swaps(s, [(a + x, a + y) for x, y in words])
            for (x, y), best in swaps.items():
                for text, _ in best:
                    alts.append((phrase[:x - a] + text + phrase[y - a:], "swap"))
        # T5 is unreliable on tiny fragments like "in order to"; the phrase book covers those.
        if generate and self.t5 and (len(phrase.split()) > 3 or not alts) and len(phrase.split()) <= 25:
            gens = self.t5.generate(phrase, n=5) + self.t5.generate(phrase, n=3, sample=True, seed=len(phrase))
            for g in gens:
                g = self._clean_generated(phrase, g)
                if g:
                    alts.append((g, "rewrite"))
        seen, uniq = set(), []
        for x, k in alts:
            x = match_case(phrase, x) if phrase[:1].isupper() else x
            if x.lower() not in seen and x.lower() != phrase.lower():
                seen.add(x.lower())
                uniq.append((x, k))
        if not uniq:
            return []
        if not self.mlm:
            return [Suggestion(x, "phrase", max(0.3, 0.85 - 0.05 * i), "phrase book")
                    for i, (x, k) in enumerate(uniq)][:n]
        texts = [x for x, _ in uniq]
        scores, orig, _ = self.mlm.substitutes(s, a, b, texts, topk=0, max_full=12)
        span_sim = sent_sim = [0.9] * len(texts)
        if self.emb:
            span_sim, sent_sim = self.emb.span_similarity(s, a, b, texts)
        out = []
        for (x, k), ss, cs in zip(uniq, span_sim, sent_sim):
            if x not in scores or (self.emb and k == "rewrite" and cs < 0.9):
                continue
            lm = (scores[x] - orig) / math.sqrt(max(1, len(self.mlm.enc(" " + x))))
            score = lm + 6.0 * (ss - 1.0) + 15.0 * (cs - 1.0) + {"phrase": 0.8, "swap": 0.5}.get(k, 0.0)
            kind = {"swap": "synonym"}.get(k, k)
            out.append(Suggestion(x, kind, 0, f"meaning {cs * 100:.0f}%" if self.emb else "", score))
        _calibrate(out, -4.0, 1.2)
        out.sort(key=lambda x: -x.score)
        return [x for x in out if x.fit >= 0.08][:n]

    def _choose_phrase(self, s, a, b, alts):
        """Pick the phrase-book alternative that best fits the sentence (cheap, for auto mode)."""
        if not self.mlm or len(alts) < 2:
            return alts[0]
        s, a, b = _normalize(s, a, b)
        sc, orig, _ = self.mlm.substitutes(s, a, b, alts, topk=0, max_full=len(alts))
        return max(alts, key=lambda x: (sc.get(x, -99.0) - orig) / math.sqrt(max(1, len(self.mlm.enc(" " + x)))))

    # -- sentences ----------------------------------------------------------------
    @staticmethod
    def _mask(sentence, spans):
        """Replace protected spans with placeholders ZQ1, ZQ2 ... (spaced so the model keeps them)."""
        parts, slots, pos = [], [], 0
        for i, (s, e) in enumerate(spans, 1):
            parts.append(sentence[pos:s])
            before = s > 0 and sentence[s - 1].isspace()
            after = e < len(sentence) and sentence[e].isspace()
            slots.append((f"ZQ{i}", sentence[s:e], before, after, sentence[s - 1:s], sentence[e:e + 1]))
            parts.append(("" if before or s == 0 else " ") + f"ZQ{i}" + ("" if after or e == len(sentence) else " "))
            pos = e
        parts.append(sentence[pos:])
        return "".join(parts), slots

    @staticmethod
    def _unmask(text, slots):
        for tag, orig, before, after, prev_ch, next_ch in slots:
            pat = re.compile(r"(\s?)" + r"\s?".join(tag.lower()) + r"(?!\d)(\s?)", re.I)
            m = pat.search(text)
            if not m or len(pat.findall(text)) != 1:
                return None  # the model dropped or duplicated protected content
            pre, post = m.group(1), m.group(2)
            # Drop only the spaces we added ourselves: when the neighbour is still the original one
            # (e.g. "Table~\ref{x})"), glue it back; otherwise keep the model's spacing.
            if pre and not before and text[m.start() - 1:m.start()] == prev_ch:
                pre = ""
            if post and not after and text[m.end():m.end() + 1] == next_ch:
                post = ""
            text = text[:m.start()] + pre + orig + post + text[m.end():]
        return text

    def paraphrase(self, sentence, protected=(), n=5, variation=0):
        """Rewrite a whole sentence. `protected` = spans relative to `sentence`."""
        lead = len(sentence) - len(sentence.lstrip())
        trail = len(sentence.rstrip())
        core = sentence[lead:trail]
        spans = formats.merge([(max(0, s - lead), min(len(core), e - lead))
                               for s, e in protected if e > lead and s < trail])
        if len(WORD_RE.findall(core)) < 3:
            return []
        if not self.t5:
            return self._lite_paraphrase(sentence, lead, trail, core, spans, n)
        masked, slots = self._mask(core, spans)
        if variation:
            gens = self.t5.generate(masked, n=n, sample=True, seed=variation, temperature=1.0)
        else:
            gens = self.t5.generate(masked, n=n)
        cands, seen = [], set()
        for g in gens:
            g = g.strip()
            if core[-1:] in ".!?" and g[-1:] not in ".!?":
                g += core[-1]
            restored = self._unmask(g, slots) if slots else g
            if not restored or restored.lower() in seen or restored == core:
                continue
            seen.add(restored.lower())
            cands.append((g, restored))
        if not cands:
            return []
        sims = [0.9] * len(cands)
        if self.emb:
            sims = self.emb.similarity(masked, [g for g, _ in cands])
        out = []
        for (g, r), sim in zip(cands, sims):
            if sim < 0.72:
                continue
            change = word_diff_ratio(core, r)
            fit = min(1.0, max(0.0, (sim - 0.72) / 0.25) * 0.75 + min(change, 0.8) * 0.3)
            out.append(Suggestion(sentence[:lead] + r + sentence[trail:], "rewrite", fit,
                                  f"meaning {sim * 100:.0f}% · {change * 100:.0f}% reworded", sim))
        out.sort(key=lambda x: -x.fit)
        return out

    def _lite_paraphrase(self, sentence, lead, trail, core, spans, n):
        variants = []
        text, text_spans = self._apply_phrases(core, spans)
        if text != core:
            variants.append(text)
        for ratio in (0.3, 0.6):
            v = self._swap_words(text, text_spans, ratio, "strong")
            if v != text and v not in variants:
                variants.append(v)
        return [Suggestion(sentence[:lead] + v + sentence[trail:], "rewrite", 0.5,
                           f"offline thesaurus · {word_diff_ratio(core, v) * 100:.0f}% reworded")
                for v in variants][:n]

    # -- whole-text auto rewrite -------------------------------------------------------
    def _apply_phrases(self, cur, spans):
        """Replace phrase-book expressions outside protected spans; returns (text, shifted spans)."""
        cur_spans = list(spans)
        for ps, pe, p in reversed(list(phrases.find(cur))):
            if formats.overlaps(cur_spans, ps, pe):
                continue
            alts = [x for x in phrases.lookup(p) if x not in ("e.g.",)]
            if not alts:
                continue
            alts = [match_case(p, x) if p[:1].isupper() else x for x in alts]
            choice = self._choose_phrase(cur, ps, pe, alts)
            cur = cur[:ps] + choice + cur[pe:]
            delta = len(choice) - (pe - ps)
            cur_spans = [(x + delta, y + delta) if x >= pe else (x, y) for x, y in cur_spans]
        return cur, cur_spans

    def _eligible_words(self, sent, spans):
        first = re.search(r"\S", sent)
        sent_start = first.start() if first else 0
        idioms = [(s, e) for s, e, _ in phrases.find(sent)]  # "for instance" must not become "for case"
        out = []
        for m in WORD_RE.finditer(sent):
            w, a, b = m.group(), m.start(), m.end()
            if len(w) < 4 or w.lower() in STOPWORDS or "'" in w or w.isupper() or w.lower() in NUMBER_WORDS:
                continue
            if w[:1].isupper() and a != sent_start:  # probably a proper noun
                continue
            if formats.overlaps(spans, a, b) or formats.overlaps(idioms, a, b):
                continue
            out.append((a, b))
        return out

    def _swap_words(self, sent, spans, ratio, strength="medium"):
        """Replace up to `ratio` of eligible words with their best context-fitting synonym."""
        eligible = self._eligible_words(sent, spans)
        if not eligible:
            return sent
        quota = max(1, round(len(eligible) * ratio))
        _, min_score, min_sim = self.STRENGTH[strength]
        if self.mlm:
            swaps = self._word_swaps(sent, eligible, per_word=1, min_score=min_score, min_sim=min_sim)
            ranked = sorted(((best[0][1], a, b, best[0][0]) for (a, b), best in swaps.items() if best),
                            reverse=True)
        else:
            # Lite mode has no context model, so only take very safe swaps: an adjective/adverb synonym
            # from one of the word's two most common senses, a common word, same shape (-ly stays -ly).
            ranked = []
            ctx = [w for w in WORD_RE.findall(sent) if w.lower() not in STOPWORDS]
            for a, b in eligible:
                w = sent[a:b]
                pos = _guess_pos(sent, a, b)
                cands = [c for c in self.thes.candidates(w, limit=30, related=False)
                         if " " not in c.text and "-" not in c.text and c.sense_rank <= 1
                         and c.pos in ("a", "r")  # nouns/verbs are too ambiguous without a model
                         and self.thes.zipf(c.text) >= 4.0 and (not pos or thesaurus.WN2UPOS[c.pos] in pos)
                         and w.lower().endswith("ly") == c.text.endswith("ly")]
                if cands:
                    best = self.thes.lesk_rank(w, ctx, cands)[0]
                    if best.score >= 3.0:
                        ranked.append((best.score, a, b, match_case(w, best.text)))
            ranked.sort(reverse=True)
        chosen, taken = [], []
        for score, a, b, text in ranked:
            if len(chosen) >= quota:
                break
            if any(abs(a - y) < 2 or abs(x - b) < 2 for x, y in taken):
                continue  # don't change two neighbouring words
            chosen.append((a, b, text))
            taken.append((a, b))
        for a, b, text in sorted(chosen, reverse=True):
            sent = sent[:a] + text + sent[b:]
        return sent

    def auto_rewrite(self, text, regions, protected, opts, progress=None, cancelled=None):
        """Return edits [(start, end, new_text, whole_sentence)] for the sentences inside `regions`."""
        strength = opts.get("strength", "medium")
        ratio = self.STRENGTH[strength][0]
        sentences = []
        for rs, re_ in regions:
            for s, e in sentence_bounds(text[rs:re_], opts.get("lines", False)):
                sentences.append((rs + s, rs + e))
        edits = []
        for k, (s, e) in enumerate(sentences):
            if cancelled and cancelled():
                break
            if progress:
                progress(k, len(sentences))
            sent = text[s:e]
            spans = formats.clip(protected, s, e)
            prose = sum(1 for m in WORD_RE.finditer(sent) if not formats.overlaps(spans, m.start(), m.end()))
            if prose < 3:
                continue
            new, whole = None, False
            if opts.get("sentences") and self.t5 and prose >= 6 and len(spans) <= 6:
                for sug in self.paraphrase(sent, spans, n=4):
                    if sug.score >= 0.9:  # meaning similarity
                        new, whole = sug.text, True
                        break
            if new is None:
                cur, cur_spans = sent, list(spans)
                if opts.get("phrases", True):
                    cur, cur_spans = self._apply_phrases(cur, cur_spans)
                if opts.get("words", True):
                    cur = self._swap_words(cur, cur_spans, ratio, strength)
                new = cur
            if new and new != sent:
                edits.append((s, e, new, whole))  # whole: a rephrased sentence, not word swaps
        if progress:
            progress(len(sentences), len(sentences))
        return edits

    # -- proofreading -----------------------------------------------------------------
    def spell_suggest(self, text, start, end, n=7):
        """Corrections for the misspelled word text[start:end], best first."""
        s, a, b = _normalize(text, start, end)
        cands = self.speller.suggest(s[a:b], 10)
        fits = {}
        if self.mlm and cands:
            fits = self.mlm.fill(s, [(a, b, [t for t, _ in cands if " " not in t])])[0][0]
        out = []
        for text, prior in cands:
            senses = self.thes.senses(text) if " " not in text else []
            lp = max(fits[text], -14.0) if text in fits else -7.0
            out.append(Suggestion(text, "spelling", 0, senses[0][2] if senses else "",
                                  prior + (0.45 * lp if self.mlm else 0.0)))
        _calibrate(out, 0.0 if self.mlm else 2.0, 2.0)
        out.sort(key=lambda x: -x.score)
        return out[:n]

    def _alternatives(self, word, prev):
        """Sibling forms a slip could have swapped in for the word: {form: [kinds of slip]}."""
        lw = word.lower()
        key = (lw, prev in BASE_AFTER)
        if key in self._alts:
            return self._alts[key]
        alts = {}
        for kind, groups in (("confused", CONFUSED), ("pair", AGREEING)):
            for group in groups:
                if lw in group:
                    alts.update((x, [kind]) for x in group if x != lw)
        if LI and lw not in STOPWORDS and lw not in proof.FUNCTION_WORDS and len(lw) > 2 and "'" not in lw:
            T = self.thes
            lemmas = T.lemmas(lw)
            tags = T.tags(lw, lemmas)

            def offer(lemma, tag, kind):
                x = lemma if tag is None else (LI.getInflection(lemma, tag, inflect_oov=False) or (None,))[0]
                if x and x != lw and " " not in x and kind not in alts.setdefault(x, []):
                    alts[x].append(kind)

            for v in lemmas.get("VERB", ()):
                if "VBZ" in tags:
                    offer(v, None, "agree")
                if tags & {"VB", "VBP"}:
                    offer(v, "VBZ", "agree")
                if "VBD" in tags and "VBN" not in tags:
                    offer(v, "VBN", "form")
                if "VBN" in tags and "VBD" not in tags:
                    offer(v, "VBD", "form")
                if prev in BASE_AFTER and not tags & {"VB", "VBP"}:
                    offer(v, None, "form")
            for noun in lemmas.get("NOUN", ()):
                if "NNS" in tags:
                    offer(noun, None, "singular")
                if "NN" in tags:
                    offer(noun, "NNS", "plural")
        if len(self._alts) > 20000:
            self._alts.clear()
        self._alts[key] = alts
        return alts

    @staticmethod
    def _prose(sent, spans):
        """The sentence as plain prose for the context model: commands and braces vanish, other
        protected pieces (maths, references, code, links) become "X".
        Returns (prose, [(prose_pos, sent_pos, length)])."""
        parts, segs, pos, n = [], [], 0, 0
        for s, e in list(spans) + [(len(sent), len(sent))]:
            if s > pos:
                segs.append((n, pos, s - pos))
                parts.append(sent[pos:s])
                n += s - pos
            p = sent[s:e]
            if p == "~":
                fill = " "
            elif not p or p in "{}[]&^_" or p[0] in "<%" or (p[0] == "\\" and not _REF_CMD.match(p)
                                                               and p[1:2] not in ("(", "[")):
                fill = ""
            else:
                fill = "X"
            parts.append(fill)
            n += len(fill)
            pos = max(pos, e)
        return "".join(parts), segs

    def _slip_scores(self, sent, spans=()):
        """Every word of a sentence that has sibling forms, with the best-fitting sibling:
        [(gain, word index, start, word, sibling, kind, fit of the word, fit of the sibling)]."""
        prose, segs = self._prose(sent, spans)
        words = list(WORD_RE.finditer(prose))
        if not 4 <= len(words) <= 80:
            return []
        first = words[0].start()
        targets, meta = [], []
        for k, m in enumerate(words):
            w, a, b = m.group(), m.start(), m.end()
            seg = next(((pp, sp) for pp, sp, ln in segs if pp <= a and b <= pp + ln), None)
            if seg is None or (w[:1].isupper() and (a != first or w.isupper())):
                continue  # inside a placeholder, a name, an acronym
            alts = self._alternatives(w, words[k - 1].group().lower() if k else "")
            if not alts:
                continue
            # Both numbers of a noun are usually fine; only check one that a nearby word contradicts
            # ("three experiment", "each players").
            digits = re.search(r"\b\d+\s+(?:[\w-]+\s+)?$", prose[:a])
            many = bool(digits and digits.group().split()[0] != "1") or \
                bool(PLURAL_CUES & {x.group().lower() for x in words[max(0, k - 3):k]})
            lone = bool(SINGULAR_CUES & {x.group().lower() for x in words[max(0, k - 2):k]})
            skip = {"plural"} - ({"plural"} if many else set()) | {"singular"} - ({"singular"} if lone else set())
            if a == first:  # "Keeps the meaning." - a note without a subject is not an agreement slip
                skip |= {"agree", "form", "singular", "plural"}
            cased = {match_case(w, x): [kd for kd in kinds if kd not in skip] for x, kinds in alts.items()}
            cased = {x: kinds for x, kinds in cased.items() if kinds}
            if cased:
                targets.append((a, b, [w] + list(cased)))
                meta.append((k, a + seg[1] - seg[0], w, cased))
        out = []
        for (k, start, w, cased), (fits, guesses) in zip(meta, self.mlm.fill(prose, targets[:40], topk=15, side=32)):
            if w not in fits:
                continue
            # The model's own guesses say whether a verb or a noun belongs here, so "the base classes"
            # is not read as a verb that should agree with something.
            share = _pos_share(guesses)
            best = None
            for x, kinds in cased.items():
                kinds = [kd for kd in kinds if share.get(NEEDS_POS.get(kd), 1.0) >= 0.5]
                if kinds and x in fits and (best is None or fits[x] > fits[best[0]]):
                    best = (x, kinds[0])
            if best:
                out.append((fits[best[0]] - fits[w], k, start, w, best[0], best[1], fits[w], fits[best[0]]))
        return out

    def grammar(self, sent, spans=()):
        """Slips in one sentence that the context model is confident about:
        [(start, end, message, [fix])] with offsets into the sentence. Needs the context model."""
        if not self.mlm:
            return []
        out, taken = [], []
        for gain, k, start, w, alt, kind, own, other in sorted(self._slip_scores(sent, spans), reverse=True):
            need, msg = SLIPS[kind]
            # a much better fit may itself be a rarer word; contractions always score lower (two pieces)
            floor = MIN_ALT_FIT - min(2.0, 0.5 * (gain - need)) - (2.5 if "'" in alt else 0.0)
            if gain < need or own > MAX_OWN_FIT or other < floor:
                continue
            # one agreement slip makes both the subject and the verb look wrong: keep the clearer one
            if kind != "confused":
                if any(abs(k - j) <= 3 for j in taken):
                    continue
                taken.append(k)
            out.append((start, start + len(w), msg.format(alt=alt, word=w), [alt]))
        return sorted(out)
