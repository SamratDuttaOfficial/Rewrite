"""Sentence embeddings (all-MiniLM-L6-v2, ONNX) used to check that meaning is preserved."""
import os

import numpy as np

from .onnx_util import pad, session


class Embedder:
    def __init__(self, folder):
        from tokenizers import Tokenizer

        self.tok = Tokenizer.from_file(os.path.join(folder, "tokenizer.json"))
        self.tok.enable_truncation(256)
        self.sess = session(os.path.join(folder, "model.onnx"))
        self.inputs = {i.name for i in self.sess.get_inputs()}
        self.pad_id = self.tok.token_to_id("[PAD]") or 0

    def encode(self, texts, batch=32):
        out = []
        for k in range(0, len(texts), batch):
            chunk = texts[k:k + batch]
            ids, mask = pad([e.ids for e in self.tok.encode_batch(chunk)], self.pad_id)
            feed = {"input_ids": ids, "attention_mask": mask}
            if "token_type_ids" in self.inputs:
                feed["token_type_ids"] = np.zeros_like(ids)
            h = self.sess.run(None, feed)[0]
            m = mask[..., None].astype(np.float32)
            v = (h * m).sum(1) / np.maximum(m.sum(1), 1e-9)
            v /= np.linalg.norm(v, axis=1, keepdims=True) + 1e-9
            out.append(v)
        return np.concatenate(out) if out else np.zeros((0, 384), dtype=np.float32)

    def similarity(self, original, variants):
        if not variants:
            return []
        v = self.encode([original] + list(variants))
        return [float(x) for x in v[1:] @ v[0]]

    def multi_span_similarity(self, sentence, items, batch=32):
        """items: [(start, end, [replacements])] -> per item, span similarity of each replacement."""
        texts, spans, owners = [sentence], [None], [None]
        for k, (a, b, reps) in enumerate(items):
            for r in reps:
                texts.append(sentence[:a] + r + sentence[b:])
                spans.append((a, a + len(r)))
                owners.append(k)
        hs, encs = [], []
        for k in range(0, len(texts), batch):
            e = self.tok.encode_batch(texts[k:k + batch])
            ids, mask = pad([x.ids for x in e], self.pad_id)
            feed = {"input_ids": ids, "attention_mask": mask}
            if "token_type_ids" in self.inputs:
                feed["token_type_ids"] = np.zeros_like(ids)
            h = self.sess.run(None, feed)[0]
            hs.extend(h)
            encs.extend(e)

        def vec(i, s, t):
            e = encs[i]
            idx = [j for j, (x, y) in enumerate(e.offsets) if x < t and y > s and not e.special_tokens_mask[j]]
            v = hs[i][idx].mean(0) if idx else hs[i][: len(e.ids)].mean(0)
            return v / (np.linalg.norm(v) + 1e-9)

        out = [[] for _ in items]
        for i in range(1, len(texts)):
            k = owners[i]
            a, b, _ = items[k]
            out[k].append(float(vec(i, *spans[i]) @ vec(0, a, b)))
        return out

    def span_similarity(self, sentence, start, end, replacements, batch=32):
        """Contextual similarity of the replaced span vs the original span (and whole sentence).

        Returns (span_sims, sentence_sims), one value per replacement.
        """
        a = max(0, start - 300)
        sentence, start, end = sentence[a:end + 300], start - a, end - a
        texts = [sentence] + [sentence[:start] + r + sentence[end:] for r in replacements]
        spans = [(start, end)] + [(start, start + len(r)) for r in replacements]
        tok_vecs, sent_vecs = [], []
        for k in range(0, len(texts), batch):
            encs = self.tok.encode_batch(texts[k:k + batch])
            ids, mask = pad([e.ids for e in encs], self.pad_id)
            feed = {"input_ids": ids, "attention_mask": mask}
            if "token_type_ids" in self.inputs:
                feed["token_type_ids"] = np.zeros_like(ids)
            h = self.sess.run(None, feed)[0]
            for row, e in enumerate(encs):
                s, t = spans[k + row]
                idx = [i for i, (x, y) in enumerate(e.offsets)
                       if x < t and y > s and not e.special_tokens_mask[i]]
                m = mask[row].astype(bool)
                sv = h[row][m].mean(0)
                tv = h[row][idx].mean(0) if idx else sv
                tok_vecs.append(tv / (np.linalg.norm(tv) + 1e-9))
                sent_vecs.append(sv / (np.linalg.norm(sv) + 1e-9))
        tv, sv = np.array(tok_vecs), np.array(sent_vecs)
        return [float(x) for x in tv[1:] @ tv[0]], [float(x) for x in sv[1:] @ sv[0]]
