//! Masked language model (RoBERTa-base) for context-aware word and phrase substitution (was `rewrite/nlp/mlm.py`).
//!
//! For `substitutes` and `score_many` the model sees the original sentence *and* a copy with the target masked
//! (`<s> original </s></s> left <mask> right </s>`), so its predictions fit the context while staying close to the
//! original meaning (the LSBert trick). A candidate of several pieces is scored piece by piece, left to right, each
//! with the pieces before it filled in (a word-level pseudo log-likelihood).
//!
//! Scores are returned in candidate order, as the Python dictionaries iterated.

use crate::onnx::{self, Session};
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use tokenizers::Tokenizer;

/// Tokens of context kept on each side of the target.
pub const MAX_SIDE: usize = 48;

/// Candidate scores (log-probabilities) in candidate order.
pub type Scores = Vec<(String, f64)>;

/// What `substitutes` found: candidate scores, the original's own score, and the model's proposals.
#[derive(Clone, Debug, PartialEq)]
pub struct Substitutes {
    pub scores: Scores,
    pub original: f64,
    pub proposals: Vec<(String, f64)>,
}

pub struct MaskedLm {
    tok: Tokenizer,
    sess: Session,
    roberta: bool,
    mask: u32,
    cls: u32,
    sep: u32,
    pad: u32,
    /// Whole-word tokens preceded by a space.
    words_space: Vec<bool>,
    /// Word tokens with no space before (e.g. after "(").
    words_nospace: Vec<bool>,
    /// Capitalised words at the start of the text.
    words_start: Vec<bool>,
    /// The word a token stands for, or "" when it is not one.
    id2word: Vec<String>,
    spaces: Regex,
}

/// `^[A-Za-z]{2,}$|^[aAI]$`
fn alpha(t: &str) -> bool {
    (t.len() >= 2 && t.bytes().all(|b| b.is_ascii_alphabetic())) || matches!(t, "a" | "A" | "I")
}

/// Where to look for a target's tokens: the token ids left and right of it, and whether it follows a space.
struct Parts {
    left: Vec<u32>,
    right: Vec<u32>,
    space: bool,
}

impl Parts {
    fn sp(&self) -> &'static str {
        if self.space { " " } else { "" }
    }
}

impl MaskedLm {
    /// Loads `tokenizer.json` and `model.onnx` from a model folder.
    pub fn load(folder: &Path) -> Result<MaskedLm, String> {
        let tok = crate::tokenizer(folder)?;
        let sess = Session::load(&folder.join("model.onnx"))?;
        let vocab = tok.get_vocab(true);
        let roberta = vocab.contains_key("<mask>");
        let names = if roberta { ["<mask>", "<s>", "</s>", "<pad>"] } else { ["[MASK]", "[CLS]", "[SEP]", "[PAD]"] };
        let [mask, cls, sep, pad] = names.map(|t| vocab.get(t).copied().ok_or_else(|| format!("the tokenizer has no {t}")));
        let size = vocab.values().max().map_or(0, |&m| m as usize + 1);
        let mut m = MaskedLm {
            tok,
            sess,
            roberta,
            mask: mask?,
            cls: cls?,
            sep: sep?,
            pad: pad?,
            words_space: vec![false; size],
            words_nospace: vec![false; size],
            words_start: vec![false; size],
            id2word: vec![String::new(); size],
            spaces: Regex::new(r"[\s\x1c-\x1f]+").expect("valid regex"),
        };
        for (t, &i) in &vocab {
            let i = i as usize;
            if !roberta {
                if alpha(t) {
                    (m.words_space[i], m.words_nospace[i], m.words_start[i]) = (true, true, true);
                    m.id2word[i] = t.clone();
                }
            } else if let Some(w) = t.strip_prefix('Ġ').filter(|w| alpha(w)) {
                m.words_space[i] = true;
                m.id2word[i] = w.to_string();
            } else if alpha(t) {
                m.words_nospace[i] = true;
                m.words_start[i] = t.starts_with(|c: char| c.is_uppercase());
                m.id2word[i] = t.clone();
            }
        }
        Ok(m)
    }

    /// Token ids of a text without special tokens.
    pub fn enc(&self, text: &str) -> Vec<u32> {
        if text.is_empty() {
            return Vec::new();
        }
        self.tok.encode(text, false).map(|e| e.get_ids().to_vec()).unwrap_or_default()
    }

    /// Token ids left and right of `sentence[start..end]` (at most 400 characters of text, `side` tokens each).
    fn parts(&self, sentence: &str, start: usize, end: usize, side: usize) -> Parts {
        let pre = self.spaces.replace_all(&sentence[crate::chars_back(sentence, start, 400)..start], " ");
        let post = self.spaces.replace_all(&sentence[end..crate::chars_ahead(sentence, end, 400)], " ");
        let has_text = !pre.trim_matches(crate::py_space).is_empty();
        let space = pre.ends_with(' ') && has_text;
        let left = if has_text {
            let ids = self.enc(if space { pre.trim_end_matches(' ') } else { &pre });
            ids[ids.len().saturating_sub(side)..].to_vec()
        } else {
            Vec::new()
        };
        let mut right = self.enc(&post);
        right.truncate(side);
        Parts { left, right, space }
    }

    fn allowed(&self, parts: &Parts) -> &[bool] {
        if parts.space {
            &self.words_space
        } else if !parts.left.is_empty() {
            &self.words_nospace
        } else {
            &self.words_start
        }
    }

    /// The model's best whole words among `allowed`, skipping `exclude` (lower case) and repeats.
    fn proposals(&self, lp: &[f32], allowed: &[bool], exclude: &HashSet<String>, topk: usize) -> Vec<(String, f64)> {
        let masked = |i: usize| if allowed.get(i).copied().unwrap_or(false) { lp[i] } else { -1e9 };
        let k = (lp.len().saturating_sub(1)).min(topk * 3);
        let mut idx: Vec<usize> = (0..lp.len()).collect();
        let desc = |a: &usize, b: &usize| masked(*b).total_cmp(&masked(*a));
        if k < idx.len() {
            idx.select_nth_unstable_by(k, desc);
            idx.truncate(k);
        }
        idx.sort_by(desc);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for i in idx {
            let w = self.id2word.get(i).map_or("", String::as_str);
            let lower = w.to_lowercase();
            if w.is_empty() || exclude.contains(&lower) || !seen.insert(lower) {
                continue;
            }
            out.push((w.to_string(), lp[i] as f64));
            if out.len() >= topk {
                break;
            }
        }
        out
    }

    /// `[cls] a [sep] ([sep]) b [sep]` and the offset where `b` starts.
    fn pair(&self, a: &[u32], b: &[u32]) -> (Vec<u32>, usize) {
        let mut seq = vec![self.cls];
        seq.extend_from_slice(a);
        seq.push(self.sep);
        if self.roberta {
            seq.push(self.sep);
        }
        let off = seq.len();
        seq.extend_from_slice(b);
        seq.push(self.sep);
        (seq, off)
    }

    /// Runs sequences (shortest first, `batch` at a time) and returns log-probabilities over the vocabulary at
    /// one position of each.
    fn run(&self, seqs: &[Vec<u32>], positions: &[usize], batch: usize) -> Result<Vec<Vec<f32>>, String> {
        let mut out = vec![Vec::new(); seqs.len()];
        let mut order: Vec<usize> = (0..seqs.len()).collect();
        order.sort_by_key(|&i| seqs[i].len());
        for idx in order.chunks(batch) {
            let rows: Vec<Vec<i64>> = idx.iter().map(|&i| seqs[i].iter().map(|&t| t as i64).collect()).collect();
            let (ids, mask, width) = onnx::pad(&rows, self.pad as i64);
            let shape = [idx.len(), width];
            let mut feed = vec![
                ("input_ids", onnx::tensor_i64(&shape, ids)?.into()),
                ("attention_mask", onnx::tensor_i64(&shape, mask)?.into()),
            ];
            if self.sess.has_input("token_type_ids") {
                feed.push(("token_type_ids", onnx::tensor_i64(&shape, vec![0; idx.len() * width])?.into()));
            }
            let logits = self.sess.run(feed)?;
            let (dims, data) = onnx::floats(&logits[0])?;
            let vocab = dims[2];
            for (row, &i) in idx.iter().enumerate() {
                let at = (row * width + positions[i]) * vocab;
                out[i] = onnx::log_softmax(&data[at..at + vocab]);
            }
        }
        Ok(out)
    }

    /// Fast single-pass scoring for several targets in one sentence. `targets`: `(start, end, candidates)`; only
    /// single-token candidates are scored. Returns, per target, the candidate scores and the model's `topk`
    /// proposals.
    pub fn score_many(
        &self,
        sentence: &str,
        targets: &[(usize, usize, Vec<String>)],
        topk: usize,
    ) -> Result<Vec<(Scores, Vec<(String, f64)>)>, String> {
        let (mut seqs, mut pos, mut meta) = (Vec::new(), Vec::new(), Vec::new());
        for (start, end, cands) in targets {
            let p = self.parts(sentence, *start, *end, MAX_SIDE);
            let target = &sentence[*start..*end];
            let mut orig = self.enc(&format!("{}{target}", p.sp()));
            orig.truncate(8);
            let seg_a = [&p.left[..], &orig, &p.right].concat();
            let (seq, off) = self.pair(&seg_a, &[&p.left[..], &[self.mask], &p.right].concat());
            seqs.push(seq);
            pos.push(off + p.left.len());
            let mut single: Vec<(String, u32)> = Vec::new();
            for c in cands {
                if let [id] = self.enc(&format!("{}{c}", p.sp()))[..] {
                    upsert(&mut single, c, id);
                }
            }
            meta.push((single, p, target.to_lowercase()));
        }
        let res = self.run(&seqs, &pos, 16)?;
        Ok(meta
            .into_iter()
            .zip(res)
            .map(|((single, p, tl), lp)| {
                let scores = single.into_iter().map(|(c, i)| (c, lp[i as usize] as f64)).collect();
                let props = if topk > 0 { self.proposals(&lp, self.allowed(&p), &HashSet::from([tl]), topk) } else { Vec::new() };
                (scores, props)
            })
            .collect())
    }

    /// Plain masked scoring with the original word hidden: how likely is each candidate there? Unlike
    /// [`substitutes`](Self::substitutes) the model does not see the original, so a wrong word cannot defend
    /// itself. Returns, per target, the scores of candidates of up to 3 pieces and the model's own top `topk` words.
    pub fn fill(
        &self,
        sentence: &str,
        targets: &[(usize, usize, Vec<String>)],
        topk: usize,
        side: usize,
    ) -> Result<Vec<(Scores, Vec<(String, f64)>)>, String> {
        struct Plan {
            pieces: Vec<(String, Vec<u32>)>,
            /// Piece count → index of the sequence with that many masks.
            first: Vec<(usize, usize)>,
            /// (candidate, piece id, sequence index) for the later pieces.
            rest: Vec<(usize, u32, usize)>,
            parts: Parts,
        }
        let (mut seqs, mut pos, mut plans) = (Vec::new(), Vec::new(), Vec::new());
        for (start, end, cands) in targets {
            let p = self.parts(sentence, *start, *end, side);
            let mut pieces: Vec<(String, Vec<u32>)> = Vec::new();
            for c in cands {
                let ids = self.enc(&format!("{}{c}", p.sp()));
                if (1..=3).contains(&ids.len()) {
                    upsert(&mut pieces, c, ids);
                }
            }
            let mut counts: BTreeSet<usize> = pieces.iter().map(|(_, ids)| ids.len()).collect();
            if topk > 0 {
                counts.insert(1);
            }
            let mut first = Vec::new();
            for n in counts {
                first.push((n, seqs.len()));
                seqs.push([&[self.cls][..], &p.left, &vec![self.mask; n], &p.right, &[self.sep]].concat());
                pos.push(1 + p.left.len());
            }
            let mut rest = Vec::new();
            for (ci, (_, ids)) in pieces.iter().enumerate() {
                for j in 1..ids.len() {
                    rest.push((ci, ids[j], seqs.len()));
                    seqs.push([&[self.cls][..], &p.left, &ids[..j], &vec![self.mask; ids.len() - j], &p.right, &[self.sep]].concat());
                    pos.push(1 + p.left.len() + j);
                }
            }
            plans.push(Plan { pieces, first, rest, parts: p });
        }
        let res = if seqs.is_empty() { Vec::new() } else { self.run(&seqs, &pos, 16)? };
        let seq_for = |first: &[(usize, usize)], n: usize| first.iter().find(|f| f.0 == n).map(|f| f.1).expect("planned");
        Ok(plans
            .into_iter()
            .map(|plan| {
                let mut scores: Scores = plan
                    .pieces
                    .iter()
                    .map(|(c, ids)| (c.clone(), res[seq_for(&plan.first, ids.len())][ids[0] as usize] as f64))
                    .collect();
                for &(ci, tid, k) in &plan.rest {
                    scores[ci].1 += res[k][tid as usize] as f64;
                }
                let props = if topk > 0 {
                    self.proposals(&res[seq_for(&plan.first, 1)], self.allowed(&plan.parts), &HashSet::new(), topk)
                } else {
                    Vec::new()
                };
                (scores, props)
            })
            .collect())
    }

    /// Whole words the model has a single token for (an extra word list for the speller). Only tokens that start
    /// a word count: pieces such as "ieve" or "ately" are not words.
    pub fn known_words(&self) -> BTreeSet<String> {
        self.words_space
            .iter()
            .enumerate()
            .filter(|&(i, &w)| w && self.id2word[i].len() > 2)
            .map(|(i, _)| self.id2word[i].clone())
            .collect()
    }

    /// Scores `candidates` as replacements for `sentence[start..end]` and proposes `topk` new ones. Multi-piece
    /// candidates are finished only for the `max_full` best first pieces.
    pub fn substitutes<S: AsRef<str>, E: AsRef<str>>(
        &self,
        sentence: &str,
        start: usize,
        end: usize,
        candidates: &[S],
        topk: usize,
        exclude: &[E],
        max_full: usize,
    ) -> Result<Substitutes, String> {
        let target = &sentence[start..end];
        let p = self.parts(sentence, start, end, MAX_SIDE);
        let mut orig_ids = self.enc(&format!("{}{target}", p.sp()));
        orig_ids.truncate(8);
        let seg_a = [&p.left[..], &orig_ids, &p.right].concat();
        let mut exclude: HashSet<String> = exclude.iter().map(|e| e.as_ref().to_lowercase()).collect();
        exclude.insert(target.to_lowercase());

        let mut cands: Vec<(String, Vec<u32>)> = Vec::new();
        for c in candidates {
            let c = c.as_ref();
            if exclude.contains(&c.to_lowercase()) {
                continue;
            }
            let ids = self.enc(&format!("{}{c}", p.sp()));
            if (1..=8).contains(&ids.len()) {
                upsert(&mut cands, c, ids);
            }
        }
        let masked = |n: usize| [&p.left[..], &vec![self.mask; n], &p.right].concat();

        // Stage 1: one pass per distinct piece count (the first piece of every candidate).
        let mut lengths: BTreeSet<usize> = cands.iter().map(|(_, ids)| ids.len()).collect();
        lengths.extend([1, orig_ids.len()]);
        let lengths: Vec<usize> = lengths.into_iter().collect();
        let (mut seqs, mut pos) = (Vec::new(), Vec::new());
        for &n in &lengths {
            let (seq, off) = self.pair(&seg_a, &masked(n));
            seqs.push(seq);
            pos.push(off + p.left.len());
        }
        let res1 = self.run(&seqs, &pos, 12)?;
        let stage1 = |n: usize| &res1[lengths.iter().position(|&l| l == n).expect("planned")];
        let first: Vec<f64> = cands.iter().map(|(_, ids)| stage1(ids.len())[ids[0] as usize] as f64).collect();

        // Stage 2: finish multi-piece candidates, best first pieces only.
        let mut multi: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].1.len() > 1).collect();
        multi.sort_by(|&a, &b| first[b].total_cmp(&first[a]));
        multi.truncate(max_full);
        let mut todo: Vec<(Option<usize>, &[u32])> = multi.iter().map(|&i| (Some(i), &cands[i].1[..])).collect();
        if orig_ids.len() > 1 {
            todo.push((None, &orig_ids));
        }
        let (mut seqs, mut pos, mut owners) = (Vec::new(), Vec::new(), Vec::new());
        for (c, ids) in todo {
            for j in 1..ids.len() {
                let b = [&p.left[..], &ids[..j], &vec![self.mask; ids.len() - j], &p.right].concat();
                let (seq, off) = self.pair(&seg_a, &b);
                seqs.push(seq);
                pos.push(off + p.left.len() + j);
                owners.push((c, ids[j]));
            }
        }
        let res2 = if seqs.is_empty() { Vec::new() } else { self.run(&seqs, &pos, 12)? };

        let mut order: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].1.len() == 1).collect();
        order.extend(&multi);
        let mut totals = first.clone();
        let mut original = orig_ids.first().map_or(0.0, |&t| stage1(orig_ids.len())[t as usize] as f64);
        for ((c, tid), lp) in owners.into_iter().zip(&res2) {
            match c {
                Some(i) => totals[i] += lp[tid as usize] as f64,
                None => original += lp[tid as usize] as f64,
            }
        }
        let scores = order.into_iter().map(|i| (cands[i].0.clone(), totals[i])).collect();
        let proposals = if topk > 0 { self.proposals(stage1(1), self.allowed(&p), &exclude, topk) } else { Vec::new() };
        Ok(Substitutes { scores, original, proposals })
    }
}

/// Sets `key` in an insertion-ordered list (a Python dict assignment: a repeated key keeps its first place).
fn upsert<V>(list: &mut Vec<(String, V)>, key: &str, value: V) {
    match list.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => list.push((key.to_string(), value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_tokens() {
        assert!(alpha("ab") && alpha("a") && alpha("I") && alpha("Zebra"));
        assert!(!alpha("b") && !alpha("ab1") && !alpha("") && !alpha("é"));
    }

    #[test]
    fn upsert_keeps_first_place() {
        let mut l = vec![];
        upsert(&mut l, "a", 1);
        upsert(&mut l, "b", 2);
        upsert(&mut l, "a", 3);
        assert_eq!(l, [("a".to_string(), 3), ("b".to_string(), 2)]);
    }
}
