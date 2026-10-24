//! Sentence embeddings (all-MiniLM-L6-v2) used to check that a rewrite keeps the meaning (was `rewrite/nlp/embed.py`).
//!
//! The tokenizer pads every text to 128 tokens and those padding tokens count in the averages (the attention mask
//! is built from the padded ids). The thresholds in the engine were tuned that way, so it is kept (plan decision 2).

use crate::onnx::{self, Session};
use std::path::Path;
use tokenizers::{Encoding, Tokenizer, TruncationParams};

const BATCH: usize = 32;

pub struct Embedder {
    tok: Tokenizer,
    sess: Session,
    pad_id: i64,
}

/// Hidden states of one batch: `width` tokens of `dim` floats per text, and the mask they were run with.
struct Hidden {
    data: Vec<f32>,
    mask: Vec<i64>,
    width: usize,
    dim: usize,
}

impl Hidden {
    fn token(&self, row: usize, j: usize) -> &[f32] {
        let at = (row * self.width + j) * self.dim;
        &self.data[at..at + self.dim]
    }

    /// Mean of the given tokens' vectors in one row.
    fn mean(&self, row: usize, tokens: impl Iterator<Item = usize>) -> Vec<f32> {
        let mut sum = vec![0f32; self.dim];
        let mut n = 0;
        for j in tokens {
            sum.iter_mut().zip(self.token(row, j)).for_each(|(s, v)| *s += v);
            n += 1;
        }
        sum.iter_mut().for_each(|s| *s /= n.max(1) as f32);
        sum
    }

    /// Mean over the tokens the attention mask covers (the tokenizer's own padding included).
    fn masked_mean(&self, row: usize) -> Vec<f32> {
        let mask = &self.mask[row * self.width..(row + 1) * self.width];
        self.mean(row, (0..self.width).filter(|&j| mask[j] == 1))
    }
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt() + 1e-9;
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() as f64
}

/// Tokens overlapping the byte span `s..t`, special tokens (and padding) excluded.
fn span_tokens(e: &Encoding, s: usize, t: usize) -> Vec<usize> {
    let special = e.get_special_tokens_mask();
    e.get_offsets().iter().enumerate().filter(|&(j, &(x, y))| x < t && y > s && special[j] == 0).map(|(j, _)| j).collect()
}

impl Embedder {
    /// Loads `tokenizer.json` and `model.onnx` from a model folder.
    pub fn load(folder: &Path) -> Result<Embedder, String> {
        let mut tok = crate::tokenizer(folder)?;
        tok.with_truncation(Some(TruncationParams { max_length: 256, ..Default::default() })).map_err(|e| e.to_string())?;
        let pad_id = tok.token_to_id("[PAD]").unwrap_or(0) as i64;
        let sess = Session::load(&folder.join("model.onnx"))?;
        Ok(Embedder { tok, sess, pad_id })
    }

    fn run(&self, texts: &[&str]) -> Result<(Vec<Encoding>, Hidden), String> {
        let encs = self.tok.encode_batch(texts.to_vec(), true).map_err(|e| e.to_string())?;
        let seqs: Vec<Vec<i64>> = encs.iter().map(|e| e.get_ids().iter().map(|&i| i as i64).collect()).collect();
        let (ids, mask, width) = onnx::pad(&seqs, self.pad_id);
        let shape = [texts.len(), width];
        let mut feed = vec![
            ("input_ids", onnx::tensor_i64(&shape, ids)?.into()),
            ("attention_mask", onnx::tensor_i64(&shape, mask.clone())?.into()),
        ];
        if self.sess.has_input("token_type_ids") {
            feed.push(("token_type_ids", onnx::tensor_i64(&shape, vec![0; texts.len() * width])?.into()));
        }
        let out = self.sess.run(feed)?;
        let (dims, data) = onnx::floats(&out[0])?;
        let dim = *dims.last().ok_or("empty model output")?;
        Ok((encs, Hidden { data: data.to_vec(), mask, width, dim }))
    }

    /// Unit-length sentence vectors (mean of the hidden states under the attention mask).
    pub fn encode<S: AsRef<str>>(&self, texts: &[S]) -> Result<Vec<Vec<f32>>, String> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH) {
            let chunk: Vec<&str> = chunk.iter().map(AsRef::as_ref).collect();
            let (_, h) = self.run(&chunk)?;
            for row in 0..chunk.len() {
                out.push(normalized(h.masked_mean(row)));
            }
        }
        Ok(out)
    }

    /// Cosine similarity of each variant to the original.
    pub fn similarity<S: AsRef<str>>(&self, original: &str, variants: &[S]) -> Result<Vec<f64>, String> {
        if variants.is_empty() {
            return Ok(Vec::new());
        }
        let mut texts = vec![original];
        texts.extend(variants.iter().map(AsRef::as_ref));
        let v = self.encode(&texts)?;
        Ok(v[1..].iter().map(|x| dot(x, &v[0])).collect())
    }

    /// For each `(start, end, replacements)`: how close the replaced span's contextual vector stays to the
    /// original span's, per replacement.
    pub fn multi_span_similarity(&self, sentence: &str, items: &[(usize, usize, Vec<String>)]) -> Result<Vec<Vec<f64>>, String> {
        let mut texts = vec![sentence.to_string()];
        let mut spans = vec![(0, 0)];
        let mut owners = vec![usize::MAX];
        for (k, (a, b, reps)) in items.iter().enumerate() {
            for r in reps {
                texts.push(format!("{}{r}{}", &sentence[..*a], &sentence[*b..]));
                spans.push((*a, a + r.len()));
                owners.push(k);
            }
        }
        // Per text: its batch, its row in the batch and its encoding.
        let mut batches = Vec::new();
        let mut rows = Vec::new();
        for (bi, chunk) in texts.chunks(BATCH).enumerate() {
            let chunk: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let (encs, h) = self.run(&chunk)?;
            rows.extend(encs.into_iter().enumerate().map(|(row, e)| (bi, row, e)));
            batches.push(h);
        }
        let vec = |i: usize, s: usize, t: usize| {
            let (bi, row, e) = &rows[i];
            let idx = span_tokens(e, s, t);
            let h = &batches[*bi];
            normalized(if idx.is_empty() { h.mean(*row, 0..e.get_ids().len()) } else { h.mean(*row, idx.into_iter()) })
        };
        let mut out = vec![Vec::new(); items.len()];
        let mut originals = vec![None; items.len()];
        for i in 1..texts.len() {
            let k = owners[i];
            let (a, b, _) = &items[k];
            let orig = originals[k].get_or_insert_with(|| vec(0, *a, *b));
            let (s, t) = spans[i];
            out[k].push(dot(&vec(i, s, t), orig));
        }
        Ok(out)
    }

    /// Contextual similarity of the replaced span to the original span, and of the whole replaced text to the
    /// original: `(span_sims, sentence_sims)`, one value per replacement. Only 300 characters either side of the
    /// span are used.
    pub fn span_similarity<S: AsRef<str>>(
        &self,
        sentence: &str,
        start: usize,
        end: usize,
        replacements: &[S],
    ) -> Result<(Vec<f64>, Vec<f64>), String> {
        let a = crate::chars_back(sentence, start, 300);
        let sentence = &sentence[a..crate::chars_ahead(sentence, end, 300)];
        let (start, end) = (start - a, end - a);
        let mut texts = vec![sentence.to_string()];
        let mut spans = vec![(start, end)];
        for r in replacements {
            let r = r.as_ref();
            texts.push(format!("{}{r}{}", &sentence[..start], &sentence[end..]));
            spans.push((start, start + r.len()));
        }
        let (mut tok_vecs, mut sent_vecs) = (Vec::new(), Vec::new());
        for (k, chunk) in texts.chunks(BATCH).enumerate() {
            let chunk: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let (encs, h) = self.run(&chunk)?;
            for (row, e) in encs.iter().enumerate() {
                let (s, t) = spans[k * BATCH + row];
                let idx = span_tokens(e, s, t);
                let sv = h.masked_mean(row);
                let tv = if idx.is_empty() { sv.clone() } else { h.mean(row, idx.into_iter()) };
                tok_vecs.push(normalized(tv));
                sent_vecs.push(normalized(sv));
            }
        }
        let sims = |v: &[Vec<f32>]| v[1..].iter().map(|x| dot(x, &v[0])).collect();
        Ok((sims(&tok_vecs), sims(&sent_vecs)))
    }
}
