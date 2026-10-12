"""Sentence paraphraser: T5-base fine-tuned for paraphrasing, run with ONNX Runtime.

Decoding is a small hand-written diverse (grouped) greedy search with
repetition / n-gram blocking, so no PyTorch or transformers are needed.
"""
import os

import numpy as np

from .onnx_util import session


class Paraphraser:
    def __init__(self, folder):
        from tokenizers import Tokenizer

        self.tok = Tokenizer.from_file(os.path.join(folder, "tokenizer.json"))
        self.enc = session(os.path.join(folder, "encoder.onnx"))
        self.dec = session(os.path.join(folder, "decoder.onnx"))
        self.dec_inputs = [i.name for i in self.dec.get_inputs()]
        self.dec_outputs = [o.name for o in self.dec.get_outputs()]
        self.past_names = [n for n in self.dec_inputs if n.startswith("past_key_values.")]
        shape = self.dec.get_inputs()[self.dec_inputs.index(self.past_names[0])].shape if self.past_names else None
        self.heads, self.head_dim = (shape[1], shape[3]) if shape else (12, 64)
        self.eos, self.pad, self.start = 1, 0, 0

    def _encode(self, text):
        ids = self.tok.encode("paraphrase: " + text).ids
        if not ids or ids[-1] != self.eos:
            ids = ids + [self.eos]
        return ids[:160]

    def generate(self, text, n=5, diversity=3.0, repetition=2.5, no_repeat=3,
                 sample=False, temperature=0.9, seed=None):
        rng = np.random.default_rng(seed)
        src = self._encode(text)
        ids = np.array([src], dtype=np.int64)
        mask = np.ones_like(ids)
        h = self.enc.run(None, {"input_ids": ids, "attention_mask": mask})[0]
        B = n
        enc_h = np.repeat(h, B, axis=0)
        enc_m = np.repeat(mask, B, axis=0)
        max_len = int(len(src) * 1.6) + 10
        seqs = [[self.start] for _ in range(B)]
        done = np.zeros(B, dtype=bool)
        past = None
        for step in range(max_len):
            feed = {"encoder_attention_mask": enc_m, "encoder_hidden_states": enc_h}
            if past is None:
                feed["input_ids"] = np.array(seqs, dtype=np.int64)
                for name in self.past_names:
                    feed[name] = np.zeros((B, self.heads, 0, self.head_dim), dtype=np.float32)
                if "use_cache_branch" in self.dec_inputs:
                    feed["use_cache_branch"] = np.array([False])
            else:
                feed["input_ids"] = np.array([[s[-1]] for s in seqs], dtype=np.int64)
                feed.update(past)
                feed["use_cache_branch"] = np.array([True])
            outs = self.dec.run(None, feed)
            logits = outs[0][:, -1, :].astype(np.float32)
            new_past = {}
            for name, val in zip(self.dec_outputs[1:], outs[1:]):
                key = name.replace("present.", "past_key_values.")
                if ".encoder." in key and past is not None:
                    new_past[key] = past[key]
                else:
                    new_past[key] = val
            past = new_past

            chosen_now = []
            for b in range(B):
                if done[b]:
                    seqs[b].append(self.pad)
                    continue
                lg = logits[b]
                gen = seqs[b][1:]
                if gen:
                    u = np.array(sorted(set(gen)))
                    vals = lg[u]
                    lg[u] = np.where(vals > 0, vals / repetition, vals * repetition)
                if no_repeat and len(gen) >= no_repeat - 1:
                    prefix = tuple(gen[len(gen) - (no_repeat - 1):])
                    for k in range(len(gen) - no_repeat + 1):
                        if tuple(gen[k:k + no_repeat - 1]) == prefix:
                            lg[gen[k + no_repeat - 1]] = -1e9
                for t in chosen_now:  # diverse groups: penalise what earlier groups just chose
                    lg[t] -= diversity
                if step == 0 and b > 0 and not sample:
                    lg[seqs[0][1] if len(seqs[0]) > 1 else 0] -= diversity
                if sample:
                    p = lg / temperature
                    p = np.exp(p - p.max())
                    top = np.argsort(-p)[:40]
                    pp = p[top] / p[top].sum()
                    t = int(top[rng.choice(len(top), p=pp)])
                else:
                    t = int(np.argmax(lg))
                chosen_now.append(t)
                seqs[b].append(t)
                if t == self.eos:
                    done[b] = True
            if done.all():
                break
        out = []
        for s in seqs:
            toks = [t for t in s[1:] if t not in (self.pad,)]
            if self.eos in toks:
                toks = toks[:toks.index(self.eos)]
            out.append(self.tok.decode(toks, skip_special_tokens=True).strip())
        return out
