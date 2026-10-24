//! The engine against rewrite/engine.py and cleanup.py: Lite mode (fixture group "engine_lite", recorded with no
//! models) and Smart mode (the engine-level calls of "engine_smart", skipped when the models are not there).

use rewrite_core::textclean::Helper as _;
use rewrite_nlp::engine::{AutoOptions, Engine, Suggestion};
use rewrite_nlp::helper::Helper;
use rewrite_testkit::{align_ties, byte_to_char, char_to_byte, json, Case, Fixture};
use serde_json::{json as j, Map, Value};
use std::sync::{Arc, LazyLock};

/// The engine the fixtures were recorded with, without models.
static LITE: LazyLock<Engine> = LazyLock::new(|| Engine::new(&["rewriteword"]));

/// The same with the models loaded, if they are there.
static SMART: LazyLock<Option<Arc<Engine>>> = LazyLock::new(|| {
    let dir = rewrite_testkit::models_dir()?;
    if !rewrite_nlp::onnx::available() {
        return None;
    }
    let e = Engine::new(&["rewriteword"]);
    e.load_models_in(&dir);
    assert!(e.smart() && e.has_emb() && e.has_t5(), "{:?}", e.errors());
    Some(Arc::new(e))
});

fn smart() -> Option<&'static Engine> {
    let e = SMART.as_deref();
    if e.is_none() {
        eprintln!("skipped: the models are not downloaded");
    }
    e
}

fn suggestion(s: &Suggestion) -> Value {
    j!({"text": s.text, "kind": s.kind, "fit": s.fit, "note": s.note, "score": s.score})
}

fn suggestions(v: &[Suggestion]) -> Value {
    Value::Array(v.iter().map(suggestion).collect())
}

/// Arguments `i` and `i + 1` as a byte span of `text`.
fn span(c: &Case, text: &str, i: usize) -> (usize, usize) {
    (char_to_byte(text, c.arg(i)), char_to_byte(text, c.arg(i + 1)))
}

/// Argument `i`: character spans as byte spans of `text`.
fn spans(c: &Case, text: &str, i: usize) -> Vec<(usize, usize)> {
    c.arg::<Vec<(usize, usize)>>(i).into_iter().map(|(a, b)| (char_to_byte(text, a), char_to_byte(text, b))).collect()
}

/// Suggestions with equal scores were ordered by Python's set order in places; compare those as groups.
fn tied(c: &Case, got: Value, n: usize) -> Value {
    let full = c.out.as_array().is_some_and(|w| w.len() == n);
    align_ties(&c.out, got, |v| j!((v["score"].as_f64().unwrap_or(0.0) * 1e4).round()), full)
}

fn suggest_case(e: &Engine, c: &Case) -> Value {
    let text = c.str(0);
    let (a, b) = span(c, text, 1);
    let n: usize = c.arg(3);
    tied(c, suggestions(&e.suggest(text, a, b, n, c.arg(4))), n)
}

fn spell_case(e: &Engine, c: &Case) -> Value {
    let text = c.str(0);
    let (a, b) = span(c, text, 1);
    let n: usize = c.arg(3);
    tied(c, suggestions(&e.spell_suggest(text, a, b, n)), n)
}

fn paraphrase_case(e: &Engine, c: &Case) -> Value {
    let s = c.str(0);
    suggestions(&e.paraphrase(s, &spans(c, s, 1), c.arg(2), c.arg(3)))
}

fn grammar_case(e: &Engine, c: &Case) -> Value {
    let s = c.str(0);
    let out = e.grammar(s, &spans(c, s, 1));
    json(out.into_iter().map(|(a, b, m, f)| (byte_to_char(s, a), byte_to_char(s, b), m, f)).collect::<Vec<_>>())
}

fn alternatives_case(e: &Engine, c: &Case) -> Value {
    Value::Object(e.alternatives(c.str(0), c.str(1)).into_iter().map(|(k, v)| (k, json(v))).collect::<Map<_, _>>())
}

fn auto_case(e: &Engine, c: &Case) -> Value {
    let text = c.str(0);
    let o: Value = c.arg(3);
    let opts = AutoOptions {
        strength: o["strength"].as_str().unwrap().to_string(),
        phrases: o["phrases"].as_bool().unwrap(),
        words: o["words"].as_bool().unwrap(),
        sentences: o["sentences"].as_bool().unwrap(),
        lines: o["lines"].as_bool().unwrap(),
    };
    let edits = e.auto_rewrite(text, &spans(c, text, 1), &spans(c, text, 2), &opts, &|_, _| {}, &|| false);
    json(edits.into_iter().map(|(s, en, new, whole)| (byte_to_char(text, s), byte_to_char(text, en), new, whole)).collect::<Vec<_>>())
}

// -- Lite mode ------------------------------------------------------------------------------------------------

fn lite() -> Fixture {
    Fixture::load("engine_lite")
}

#[test]
fn lite_suggest() {
    lite().check_within("suggest", 1e-3, |c| suggest_case(&LITE, c));
}

#[test]
fn lite_spell_suggest() {
    lite().check_within("spell_suggest", 1e-3, |c| spell_case(&LITE, c));
}

#[test]
fn lite_paraphrase() {
    lite().check_within("paraphrase", 1e-3, |c| paraphrase_case(&LITE, c));
}

#[test]
fn lite_grammar() {
    lite().check("grammar", |c| grammar_case(&LITE, c));
}

#[test]
fn lite_alternatives() {
    lite().check("_alternatives", |c| alternatives_case(&LITE, c));
}

#[test]
fn lite_auto_rewrite() {
    lite().check("auto_rewrite", |c| auto_case(&LITE, c));
}

// -- Smart mode -----------------------------------------------------------------------------------------------

fn smart_fixture() -> Fixture {
    Fixture::load("engine_smart")
}

/// Phrase suggestions that include T5's sampled rewrites cannot match Python (plan decision 4), and the other
/// candidates' scores move with them: the quantised models scale activations per batch, so a batch holding
/// different texts gives slightly different numbers. Those cases are checked for shape only, and for the same
/// best phrase-book or swap suggestion.
fn sampled(c: &Case, got: &Value) -> bool {
    let rewrites = |v: &Value| v.as_array().is_some_and(|a| a.iter().any(|x| x["kind"] == "rewrite"));
    let target = &c.str(0)[span(c, c.str(0), 1).0..span(c, c.str(0), 1).1];
    c.arg::<bool>(4) && target.trim().contains(' ') && (rewrites(&c.out) || rewrites(got))
}

#[test]
fn smart_suggest() {
    let Some(e) = smart() else { return };
    let mut loose = 0;
    smart_fixture().check_within("suggest", 1e-3, |c| {
        let got = suggest_case(e, c);
        if !sampled(c, &got) {
            return got;
        }
        loose += 1;
        let items = got.as_array().unwrap();
        assert!(!items.is_empty() && items.len() <= c.arg::<usize>(3));
        assert!(items.windows(2).all(|w| w[0]["score"].as_f64() >= w[1]["score"].as_f64()));
        assert!(items.iter().all(|x| (0.0..=1.0).contains(&x["fit"].as_f64().unwrap())));
        let top = |v: &Value| v.as_array().unwrap().iter().find(|x| x["kind"] != "rewrite").map(|x| x["text"].clone());
        assert_eq!(top(&c.out), top(&got), "best phrase-book suggestion of case {:?}", c.args[1..].to_vec());
        c.out.clone()
    });
    assert!(loose <= 8, "{loose} cases compared loosely");
}

#[test]
fn smart_spell_suggest() {
    let Some(e) = smart() else { return };
    smart_fixture().check_within("spell_suggest", 1e-3, |c| spell_case(e, c));
}

#[test]
fn smart_paraphrase() {
    let Some(e) = smart() else { return };
    smart_fixture().check_within("paraphrase", 1e-3, |c| paraphrase_case(e, c));
}

#[test]
fn smart_grammar() {
    let Some(e) = smart() else { return };
    smart_fixture().check("grammar", |c| grammar_case(e, c));
}

#[test]
fn smart_alternatives() {
    let Some(e) = smart() else { return };
    smart_fixture().check("_alternatives", |c| alternatives_case(e, c));
}

#[test]
fn smart_auto_rewrite() {
    let Some(e) = smart() else { return };
    smart_fixture().check("auto_rewrite", |c| auto_case(e, c));
}

#[test]
fn smart_helper_choose() {
    let Some(_) = smart() else { return };
    let h = Helper::new(SMART.clone().unwrap());
    smart_fixture().check("helper.choose", |c| {
        json(h.choose(c.str(0), &c.arg::<Vec<String>>(1), c.str(2)).map_or(-1, |k| k as i64))
    });
}
