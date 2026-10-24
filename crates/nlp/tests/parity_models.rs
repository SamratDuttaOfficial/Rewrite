//! The model wrappers against rewrite/nlp/{embed,mlm,t5}.py (fixture group "engine_smart"). Skipped when the
//! models or the ONNX Runtime library are not there (as in CI).

use rewrite_nlp::{mlm, Embedder, GenerateOptions, MaskedLm, Paraphraser};
use rewrite_testkit::{char_to_byte, json, Case, Fixture};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

fn folder(key: &str) -> Option<PathBuf> {
    let Some(dir) = rewrite_testkit::models_dir() else {
        eprintln!("skipped: the models are not downloaded");
        return None;
    };
    if !rewrite_nlp::onnx::available() {
        eprintln!("skipped: the ONNX Runtime library is not there");
        return None;
    }
    Some(dir.join(key))
}

fn emb() -> Option<&'static Embedder> {
    static M: OnceLock<Option<Embedder>> = OnceLock::new();
    M.get_or_init(|| folder("similarity").map(|d| Embedder::load(&d).unwrap())).as_ref()
}

fn mlm() -> Option<&'static MaskedLm> {
    static M: OnceLock<Option<MaskedLm>> = OnceLock::new();
    M.get_or_init(|| folder("context").map(|d| MaskedLm::load(&d).unwrap())).as_ref()
}

fn t5() -> Option<&'static Paraphraser> {
    static M: OnceLock<Option<Paraphraser>> = OnceLock::new();
    M.get_or_init(|| folder("paraphrase").map(|d| Paraphraser::load(&d).unwrap())).as_ref()
}

fn fixture() -> Fixture {
    Fixture::load("engine_smart")
}

/// Arguments `i` and `i + 1` of a case as a byte span of `text`.
fn span(c: &Case, text: &str, i: usize) -> (usize, usize) {
    (char_to_byte(text, c.arg(i)), char_to_byte(text, c.arg(i + 1)))
}

fn scores(s: &mlm::Scores) -> Value {
    Value::Object(s.iter().map(|(k, v)| (k.clone(), json(v))).collect::<Map<_, _>>())
}

#[test]
fn emb_similarity() {
    let Some(m) = emb() else { return };
    fixture().check_within("emb.similarity", 1e-3, |c| json(m.similarity(c.str(0), &c.arg::<Vec<String>>(1)).unwrap()));
}

#[test]
fn emb_encode() {
    let Some(m) = emb() else { return };
    fixture().check_within("emb.encode", 1e-3, |c| {
        let v = m.encode(&c.arg::<Vec<String>>(0)).unwrap();
        json(v.iter().map(|x| x[..8].to_vec()).collect::<Vec<_>>())
    });
}

#[test]
fn emb_span_similarity() {
    let Some(m) = emb() else { return };
    fixture().check_within("emb.span_similarity", 1e-3, |c| {
        let text = c.str(0);
        let (a, b) = span(c, text, 1);
        json(m.span_similarity(text, a, b, &c.arg::<Vec<String>>(3)).unwrap())
    });
}

#[test]
fn mlm_fill() {
    let Some(m) = mlm() else { return };
    fixture().check_within("mlm.fill", 1e-3, |c| {
        let text = c.str(0);
        let targets: Vec<(usize, usize, Vec<String>)> = c
            .arg::<Vec<(usize, usize, Vec<String>)>>(1)
            .into_iter()
            .map(|(a, b, cands)| (char_to_byte(text, a), char_to_byte(text, b), cands))
            .collect();
        let out = m.fill(text, &targets, 5, mlm::MAX_SIDE).unwrap();
        Value::Array(out.iter().map(|(s, p)| Value::Array(vec![scores(s), json(p)])).collect())
    });
}

#[test]
fn mlm_substitutes() {
    let Some(m) = mlm() else { return };
    fixture().check_within("mlm.substitutes", 1e-3, |c| {
        let text = c.str(0);
        let (a, b) = span(c, text, 1);
        let s = m.substitutes(text, a, b, &c.arg::<Vec<String>>(3), 10, &[] as &[&str], 14).unwrap();
        Value::Array(vec![scores(&s.scores), json(s.original), json(s.proposals)])
    });
}

#[test]
fn mlm_known_words() {
    let Some(m) = mlm() else { return };
    fixture().check("mlm.known_words", |_| json(m.known_words().iter().step_by(50).collect::<Vec<_>>()));
}

#[test]
fn t5_generate() {
    let Some(m) = t5() else { return };
    fixture().check("t5.generate", |c| json(m.generate(c.str(0), c.arg(1), &GenerateOptions::default()).unwrap()));
}

#[test]
fn t5_sampling_varies_and_ends() {
    let Some(m) = t5() else { return };
    let opts = GenerateOptions { sample: true, seed: Some(7), temperature: 1.0, ..Default::default() };
    let text = "The results show a significant improvement in accuracy.";
    let a = m.generate(text, 3, &opts).unwrap();
    assert_eq!(a.len(), 3);
    assert!(a.iter().all(|s| !s.is_empty()), "{a:?}");
    assert_eq!(a, m.generate(text, 3, &opts).unwrap(), "a seed repeats");
}

#[test]
fn models_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Embedder>();
    check::<MaskedLm>();
    check::<Paraphraser>();
}
