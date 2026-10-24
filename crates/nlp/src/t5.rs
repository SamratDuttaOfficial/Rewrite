//! Sentence paraphraser: T5-base fine-tuned for paraphrasing (was `rewrite/nlp/t5.py`).
//!
//! The encoder runs once; the merged decoder then runs one token at a time with its key/value cache (on the first
//! step the cache inputs are empty tensors and `use_cache_branch` is false). Decoding is a small diverse (grouped)
//! greedy search with a repetition penalty and n-gram blocking, or top-40 sampling. Sampling uses Rust's generator,
//! so a seed gives different samples than numpy did (plan decision 4).

use crate::onnx::{self, Session};
use ort::session::SessionInputValue;
use ort::value::DynValue;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use tokenizers::Tokenizer;

const EOS: u32 = 1;
const PAD: u32 = 0;
const START: u32 = 0;

/// Decoding settings; the defaults are the Python ones.
#[derive(Clone, Debug)]
pub struct GenerateOptions {
    /// Penalty on a token that an earlier group chose at the same step.
    pub diversity: f32,
    /// Divides positive (multiplies negative) logits of tokens already generated.
    pub repetition: f32,
    /// Blocks repeating any n-gram of this size (0: off).
    pub no_repeat: usize,
    /// Top-40 sampling instead of greedy choice.
    pub sample: bool,
    pub temperature: f32,
    /// Seed for sampling; None draws one.
    pub seed: Option<u64>,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        GenerateOptions { diversity: 3.0, repetition: 2.5, no_repeat: 3, sample: false, temperature: 0.9, seed: None }
    }
}

pub struct Paraphraser {
    tok: Tokenizer,
    enc: Session,
    dec: Session,
    past_names: Vec<String>,
    heads: usize,
    head_dim: usize,
}

impl Paraphraser {
    /// Loads `tokenizer.json`, `encoder.onnx` and `decoder.onnx` from a model folder.
    pub fn load(folder: &Path) -> Result<Paraphraser, String> {
        let tok = crate::tokenizer(folder)?;
        let enc = Session::load(&folder.join("encoder.onnx"))?;
        let dec = Session::load(&folder.join("decoder.onnx"))?;
        let past_names: Vec<String> = dec.inputs().iter().filter(|n| n.starts_with("past_key_values.")).cloned().collect();
        let dim = |i: usize, default: usize| {
            let shape = past_names.first().and_then(|n| dec.input_shape(n));
            shape.and_then(|s| s.get(i)).filter(|&&d| d > 0).map_or(default, |&d| d as usize)
        };
        let (heads, head_dim) = (dim(1, 12), dim(3, 64));
        Ok(Paraphraser { tok, enc, dec, past_names, heads, head_dim })
    }

    /// "paraphrase: <text>" as token ids ending in EOS.
    fn source(&self, text: &str) -> Result<Vec<u32>, String> {
        let mut ids = self.tok.encode(format!("paraphrase: {text}"), true).map_err(|e| e.to_string())?.get_ids().to_vec();
        if ids.last() != Some(&EOS) {
            ids.push(EOS);
        }
        ids.truncate(160);
        Ok(ids)
    }

    /// `n` paraphrases of `text`, one per group.
    pub fn generate(&self, text: &str, n: usize, opts: &GenerateOptions) -> Result<Vec<String>, String> {
        let mut rng = match opts.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => rand::make_rng::<StdRng>(),
        };
        let src = self.source(text)?;
        let len = src.len();
        let h = self.enc.run(vec![
            ("input_ids", onnx::tensor_i64(&[1, len], src.iter().map(|&t| t as i64).collect())?.into()),
            ("attention_mask", onnx::tensor_i64(&[1, len], vec![1; len])?.into()),
        ])?;
        let (dims, h) = onnx::floats(&h[0])?;
        let b = n;
        let enc_h = onnx::tensor_f32(&[b, len, dims[2]], h.repeat(b))?;
        let enc_m = onnx::tensor_i64(&[b, len], vec![1; b * len])?;
        let max_len = (len as f64 * 1.6) as usize + 10;
        let mut seqs: Vec<Vec<u32>> = vec![vec![START]; b];
        let mut done = vec![false; b];
        let mut past: Option<HashMap<String, DynValue>> = None;
        let use_cache = self.dec.has_input("use_cache_branch");

        for step in 0..max_len {
            let outs = {
                let mut feed: Vec<(&str, SessionInputValue<'_>)> =
                    vec![("encoder_attention_mask", (&enc_m).into()), ("encoder_hidden_states", (&enc_h).into())];
                match &past {
                    None => {
                        let ids = seqs.iter().map(|s| s[0] as i64).collect();
                        feed.push(("input_ids", onnx::tensor_i64(&[b, 1], ids)?.into()));
                        for name in &self.past_names {
                            feed.push((name, onnx::tensor_f32(&[b, self.heads, 0, self.head_dim], Vec::new())?.into()));
                        }
                    }
                    Some(past) => {
                        let ids = seqs.iter().map(|s| *s.last().expect("never empty") as i64).collect();
                        feed.push(("input_ids", onnx::tensor_i64(&[b, 1], ids)?.into()));
                        feed.extend(past.iter().map(|(k, v)| (k.as_str(), v.into())));
                    }
                }
                if use_cache {
                    feed.push(("use_cache_branch", onnx::tensor_bool(&[1], vec![past.is_some()])?.into()));
                }
                self.dec.run(feed)?
            };
            let mut outs = outs.into_iter();
            let logits = outs.next().ok_or("the decoder returned nothing")?;
            let (dims, data) = onnx::floats(&logits)?;
            let (steps, vocab) = (dims[1], dims[2]);
            let mut new_past = HashMap::new();
            for (name, val) in self.dec.outputs()[1..].iter().zip(outs) {
                let key = name.replace("present.", "past_key_values.");
                // In the cache branch the encoder's keys and values are not recomputed: keep the first ones.
                let val = match past.as_mut() {
                    Some(old) if key.contains(".encoder.") => old.remove(&key).unwrap_or(val),
                    _ => val,
                };
                new_past.insert(key, val);
            }

            let mut chosen_now: Vec<u32> = Vec::new();
            for bi in 0..b {
                if done[bi] {
                    seqs[bi].push(PAD);
                    continue;
                }
                let at = (bi * steps + steps - 1) * vocab;
                let mut lg = data[at..at + vocab].to_vec();
                let generated = &seqs[bi][1..];
                for &t in &generated.iter().copied().collect::<BTreeSet<u32>>() {
                    let v = &mut lg[t as usize];
                    *v = if *v > 0.0 { *v / opts.repetition } else { *v * opts.repetition };
                }
                let nr = opts.no_repeat;
                if nr > 0 && generated.len() + 1 >= nr {
                    let prefix = &generated[generated.len() + 1 - nr..];
                    for k in 0..(generated.len() + 1).saturating_sub(nr) {
                        if &generated[k..k + nr - 1] == prefix {
                            lg[generated[k + nr - 1] as usize] = -1e9;
                        }
                    }
                }
                for &t in &chosen_now {
                    lg[t as usize] -= opts.diversity;
                }
                if step == 0 && bi > 0 && !opts.sample {
                    // The first group's choice is penalised a second time on the first step (kept from Python).
                    lg[seqs[0].get(1).copied().unwrap_or(0) as usize] -= opts.diversity;
                }
                let t = if opts.sample { sample_top40(&lg, opts.temperature, &mut rng) } else { argmax(&lg) };
                chosen_now.push(t);
                seqs[bi].push(t);
                if t == EOS {
                    done[bi] = true;
                }
            }
            past = Some(new_past);
            if done.iter().all(|&d| d) {
                break;
            }
        }
        seqs.iter()
            .map(|s| {
                let toks: Vec<u32> = s[1..].iter().copied().filter(|&t| t != PAD).take_while(|&t| t != EOS).collect();
                let text = self.tok.decode(&toks, true).map_err(|e| e.to_string())?;
                Ok(text.trim_matches(crate::py_space).to_string())
            })
            .collect()
    }
}

/// Index of the first largest value.
fn argmax(x: &[f32]) -> u32 {
    let mut best = 0;
    for (i, &v) in x.iter().enumerate() {
        if v > x[best] {
            best = i;
        }
    }
    best as u32
}

/// Samples among the 40 likeliest tokens after temperature scaling.
fn sample_top40(lg: &[f32], temperature: f32, rng: &mut StdRng) -> u32 {
    let scaled: Vec<f32> = lg.iter().map(|&v| v / temperature).collect();
    let max = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let p: Vec<f32> = scaled.iter().map(|&v| (v - max).exp()).collect();
    let mut top: Vec<usize> = (0..p.len()).collect();
    let desc = |a: &usize, b: &usize| p[*b].total_cmp(&p[*a]);
    if top.len() > 40 {
        top.select_nth_unstable_by(40, desc);
        top.truncate(40);
    }
    top.sort_by(desc);
    let total: f64 = top.iter().map(|&i| p[i] as f64).sum();
    let mut u = rng.random::<f64>() * total;
    for &i in &top {
        u -= p[i] as f64;
        if u < 0.0 {
            return i as u32;
        }
    }
    top[top.len() - 1] as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_takes_the_first_maximum() {
        assert_eq!(argmax(&[0.0, 2.0, 1.0, 2.0]), 1);
    }

    #[test]
    fn sampling_stays_in_the_top_40() {
        let lg: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..200 {
            assert!(sample_top40(&lg, 0.9, &mut rng) >= 60);
        }
    }
}
