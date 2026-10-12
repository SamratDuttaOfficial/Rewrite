"""Build rewrite/data/thesaurus.json.gz from WordNet 3.0 (+ wordfreq frequencies).

This is a one-off developer tool; the app itself only needs the generated file.

    pip install nltk wordfreq
    python -c "import nltk; nltk.download('wordnet')"
    python tools/build_thesaurus.py
"""
import gzip
import json
import os
import re

from nltk.corpus import wordnet as wn
from wordfreq import zipf_frequency

OUT = os.path.join(os.path.dirname(__file__), "..", "rewrite", "data", "thesaurus.json.gz")
POS_MAP = {"n": "n", "v": "v", "a": "a", "s": "a", "r": "r"}
GOOD = re.compile(r"^[a-z][a-z' -]*[a-z]$")


def clean(lemma_name):
    w = lemma_name.replace("_", " ")
    if not GOOD.match(w) or len(w.split()) > 3:
        return None
    return w


def main():
    synsets = list(wn.all_synsets())
    sid = {}
    table = []
    for s in synsets:
        lemmas = []
        for l in s.lemmas():
            w = clean(l.name())
            if w and w not in lemmas:
                lemmas.append(w)
        if not lemmas:
            continue
        sid[s.name()] = len(table)
        gloss = s.definition().split(";")[0].strip()
        if len(gloss) > 90:
            gloss = gloss[:87].rsplit(" ", 1)[0] + "..."
        table.append([POS_MAP[s.pos()], lemmas, gloss, s])

    synsets_by_id = [row[3].name() for row in table]
    index, ants, words = {}, {}, set()
    for i, row in enumerate(table):
        s = row[3]
        rel = []
        if s.pos() in ("a", "s"):
            rel = [x.name() for x in s.similar_tos()]
        elif s.pos() in ("n", "v"):
            rel = [x.name() for x in s.hypernyms()]
        row[3] = [sid[r] for r in rel if r in sid][:6]
        for l in s.lemmas():
            w = clean(l.name())
            if not w:
                continue
            words.add(w)
            index.setdefault(w, [])
            if i not in index[w]:
                index[w].append(i)
            for a in l.antonyms():
                aw = clean(a.name())
                if aw:
                    ants.setdefault(w, [])
                    if aw not in ants[w]:
                        ants[w].append(aw)

    # all_synsets() is not in per-word sense order; re-sort each word's senses by WordNet's
    # own ranking (most frequent sense first).
    for w, ids in index.items():
        order = {s.name(): k for k, s in enumerate(wn.synsets(w.replace(" ", "_")))}
        ids.sort(key=lambda i: order.get(synsets_by_id[i], 999))

    freq = {}
    for w in words:
        z = zipf_frequency(w, "en")
        if z >= 1.0:
            freq[w] = int(round(z * 10))

    data = {
        "v": 1,
        "S": [[p, l, g, r] for p, l, g, r in table],
        "I": index,
        "A": ants,
        "F": freq,
        "license": "WordNet 3.0 (c) Princeton University, WordNet license; frequencies from wordfreq (CC BY-SA 4.0)",
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with gzip.open(OUT, "wt", encoding="utf-8", compresslevel=9) as f:
        json.dump(data, f, separators=(",", ":"))
    print("synsets", len(table), "words", len(index), "antonyms", len(ants), "freq", len(freq))
    print("size MB", round(os.path.getsize(OUT) / 1e6, 2))


if __name__ == "__main__":
    main()
