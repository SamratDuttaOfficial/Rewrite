"""Masked language model (RoBERTa-base, ONNX) for context-aware word/phrase substitution.

The model sees the original sentence *and* a copy with the target masked
("<s> original </s></s> left <mask> right </s>"), so its predictions fit the
context while staying close to the original meaning (the LSBert trick).
Candidates are scored with a word-level pseudo log-likelihood (PLL-word-l2r).
"""
import os
import re

import numpy as np

from .onnx_util import log_softmax, pad, session

MAX_SIDE = 48  # tokens of context kept on each side of the target


class MaskedLM:
    def __init__(self, folder):
        from tokenizers import Tokenizer

        self.tok = Tokenizer.from_file(os.path.join(folder, "tokenizer.json"))
        self.sess = session(os.path.join(folder, "model.onnx"))
        self.inputs = {i.name for i in self.sess.get_inputs()}
        vocab = self.tok.get_vocab()
        self.roberta = "<mask>" in vocab
        if self.roberta:
            self.mask, self.cls, self.sep, self.pad = (vocab[t] for t in ("<mask>", "<s>", "</s>", "<pad>"))
        else:
            self.mask, self.cls, self.sep, self.pad = (vocab[t] for t in ("[MASK]", "[CLS]", "[SEP]", "[PAD]"))
        size = max(vocab.values()) + 1
        self.words_space = np.zeros(size, dtype=bool)  # whole-word tokens preceded by a space
        self.words_nospace = np.zeros(size, dtype=bool)  # word tokens with no space before (e.g. after "(")
        self.words_start = np.zeros(size, dtype=bool)  # capitalised words at the start of the text
        self.id2word = [""] * size
        alpha = re.compile(r"^[A-Za-z]{2,}$|^[aAI]$")
        for t, i in vocab.items():
            if self.roberta:
                if t.startswith("Ġ") and alpha.match(t[1:]):
                    self.words_space[i] = True
                    self.id2word[i] = t[1:]
                elif alpha.match(t):
                    self.words_nospace[i] = True
                    self.words_start[i] = t[0].isupper()
                    self.id2word[i] = t
            elif alpha.match(t):
                self.words_space[i] = self.words_nospace[i] = self.words_start[i] = True
                self.id2word[i] = t

    def enc(self, text):
        return self.tok.encode(text, add_special_tokens=False).ids if text else []

    def _parts(self, sentence, start, end, side=MAX_SIDE):
        """Token ids left/right of the target, and whether the target follows a space."""
        pre = re.sub(r"\s+", " ", sentence[max(0, start - 400):start])
        post = re.sub(r"\s+", " ", sentence[end:end + 400])
        space = pre.endswith(" ") and bool(pre.strip())
        left = self.enc(pre.rstrip(" ") if space else pre)[-side:] if pre.strip() else []
        right = self.enc(post)[:side]
        return left, right, space

    def _allowed(self, space, left):
        if space:
            return self.words_space
        return self.words_nospace if left else self.words_start

    def _proposals(self, lp, allowed, exclude, topk):
        masked_lp = np.where(allowed, lp, -1e9)
        k = min(len(lp) - 1, topk * 3)
        idx = np.argpartition(-masked_lp, k)[:k]
        out, seen = [], set()
        for i in idx[np.argsort(-masked_lp[idx])]:
            w = self.id2word[i]
            if not w or w.lower() in exclude or w.lower() in seen:
                continue
            seen.add(w.lower())
            out.append((w, float(lp[i])))
            if len(out) >= topk:
                break
        return out

    def score_many(self, sentence, targets, topk=12):
        """Fast single-pass scoring for several targets in one sentence.

        targets: [(start, end, [candidates])]. Only single-token candidates are scored.
        Returns [(scores {cand: logprob}, proposals [(word, logprob)])] per target.
        """
        seqs, pos, meta = [], [], []
        for start, end, cands in targets:
            left, right, space = self._parts(sentence, start, end)
            sp = " " if space else ""
            target = sentence[start:end]
            seg_a = left + self.enc(sp + target)[:8] + right
            seq, off = self._pair(seg_a, left + [self.mask] + right)
            seqs.append(seq)
            pos.append([off + len(left)])
            single = {}
            for c in cands:
                ids = self.enc(sp + c)
                if len(ids) == 1:
                    single[c] = ids[0]
            meta.append((single, self._allowed(space, left), target.lower()))
        res = self._run(seqs, pos, batch=16)
        out = []
        for (single, allowed, tl), r in zip(meta, res):
            lp = r[0]
            out.append(({c: float(lp[i]) for c, i in single.items()},
                        self._proposals(lp, allowed, {tl}, topk) if topk else []))
        return out

    def fill(self, sentence, targets, topk=0, side=MAX_SIDE):
        """Plain masked scoring with the original word hidden: how likely is each candidate there?

        Unlike `substitutes`, the model does not see the original, so a wrong word cannot defend
        itself. targets: [(start, end, [candidates])] -> [({candidate: logprob}, guesses)], where
        guesses are the model's own top `topk` words [(word, logprob)]. Candidates of up to 3 pieces.
        """
        seqs, pos, plan = [], [], []
        for start, end, cands in targets:
            left, right, space = self._parts(sentence, start, end, side)
            sp = " " if space else ""
            pieces = {}
            for c in cands:
                ids = self.enc(sp + c)
                if 0 < len(ids) <= 3:
                    pieces[c] = ids
            first = {}
            for n in sorted({len(ids) for ids in pieces.values()} | ({1} if topk else set())):
                first[n] = len(seqs)
                seqs.append([self.cls] + left + [self.mask] * n + right + [self.sep])
                pos.append([1 + len(left)])
            rest = []
            for c, ids in pieces.items():
                for j in range(1, len(ids)):
                    rest.append((c, ids[j], len(seqs)))
                    seqs.append([self.cls] + left + ids[:j] + [self.mask] * (len(ids) - j) + right + [self.sep])
                    pos.append([1 + len(left) + j])
            plan.append((pieces, first, rest, self._allowed(space, left)))
        res = self._run(seqs, pos, batch=16) if seqs else []
        out = []
        for pieces, first, rest, allowed in plan:
            scores = {c: float(res[first[len(ids)]][0][ids[0]]) for c, ids in pieces.items()}
            for c, tid, k in rest:
                scores[c] += float(res[k][0][tid])
            out.append((scores, self._proposals(res[first[1]][0], allowed, set(), topk) if topk else []))
        return out

    def known_words(self):
        """Whole words the model has a single token for (used as an extra spelling word list)."""
        # only tokens that start a word; pieces such as "ieve" or "ately" are not words
        return {self.id2word[i] for i in np.flatnonzero(self.words_space) if len(self.id2word[i]) > 2}

    def _pair(self, a, b):
        if self.roberta:
            return [self.cls] + a + [self.sep, self.sep] + b + [self.sep], len(a) + 3
        return [self.cls] + a + [self.sep] + b + [self.sep], len(a) + 2

    def _run(self, seqs, positions, batch=12):
        """Run sequences and return log-probs over the vocab at the requested positions."""
        out = [None] * len(seqs)
        order = sorted(range(len(seqs)), key=lambda i: len(seqs[i]))
        for k in range(0, len(order), batch):
            idx = order[k:k + batch]
            ids, mask = pad([seqs[i] for i in idx], self.pad)
            feed = {"input_ids": ids, "attention_mask": mask}
            if "token_type_ids" in self.inputs:
                feed["token_type_ids"] = np.zeros_like(ids)
            logits = self.sess.run(None, feed)[0]
            for row, i in enumerate(idx):
                out[i] = log_softmax(logits[row, positions[i]].astype(np.float32))
        return out

    def substitutes(self, sentence, start, end, candidates=(), topk=30, exclude=(), max_full=14):
        """Score `candidates` as replacements for sentence[start:end] and propose new ones.

        Returns (scores {cand: logprob}, original_logprob, proposals [(word, logprob)]).
        """
        target = sentence[start:end]
        left, right, space = self._parts(sentence, start, end)
        sp = " " if space else ""
        orig_ids = self.enc(sp + target)[:8]
        seg_a = left + orig_ids + right
        exclude = {e.lower() for e in exclude} | {target.lower()}

        cands = {}
        for c in candidates:
            if c.lower() in exclude:
                continue
            ids = self.enc(sp + c)
            if 0 < len(ids) <= 8:
                cands[c] = ids

        # Stage 1: one pass per distinct piece count (the first piece of every candidate).
        lengths = sorted({len(i) for i in cands.values()} | {1, len(orig_ids)})
        seqs, pos = [], []
        for n in lengths:
            seq, off = self._pair(seg_a, left + [self.mask] * n + right)
            seqs.append(seq)
            pos.append(list(range(off + len(left), off + len(left) + n)))
        res1 = dict(zip(lengths, self._run(seqs, pos)))
        first = {c: float(res1[len(ids)][0][ids[0]]) for c, ids in cands.items()}

        # Stage 2: finish multi-piece candidates, best first-piece scores only.
        multi = sorted((c for c, ids in cands.items() if len(ids) > 1), key=lambda c: -first[c])[:max_full]
        todo = [(c, cands[c]) for c in multi]
        if len(orig_ids) > 1:
            todo.append((None, orig_ids))
        seqs, pos, owners = [], [], []
        for c, ids in todo:
            for j in range(1, len(ids)):
                seq, off = self._pair(seg_a, left + ids[:j] + [self.mask] * (len(ids) - j) + right)
                seqs.append(seq)
                pos.append([off + len(left) + j])
                owners.append((c, ids[j]))
        res2 = self._run(seqs, pos) if seqs else []

        scores = {c: first[c] for c, ids in cands.items() if len(ids) == 1}
        for c in multi:
            scores[c] = first[c]
        orig = float(res1[len(orig_ids)][0][orig_ids[0]])
        for (c, tid), r in zip(owners, res2):
            if c is None:
                orig += float(r[0][tid])
            else:
                scores[c] += float(r[0][tid])

        proposals = self._proposals(res1[1][0], self._allowed(space, left), exclude, topk) if topk else []
        return scores, orig, proposals
