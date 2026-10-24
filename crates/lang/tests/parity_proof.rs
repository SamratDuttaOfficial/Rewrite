//! speller and grammar against rewrite/proof.py (fixture groups "speller" and "grammar").

use rewrite_lang::speller::{self, Speller};
use rewrite_lang::{grammar_rules, thesaurus, Issue};
use rewrite_testkit::{align_ties, byte_to_char, char_to_byte, json, Case, Fixture};
use serde_json::{json as j, Map, Value};
use std::sync::LazyLock;

/// The speller the fixtures were recorded with.
static SPELLER: LazyLock<Speller> = LazyLock::new(|| Speller::new(thesaurus::get(), ["rewriteword", "Samrat"]));

/// Argument 1: protected spans in characters, as byte spans.
fn protected(c: &Case) -> Vec<(usize, usize)> {
    let t = c.str(0);
    c.arg::<Vec<(usize, usize)>>(1).into_iter().map(|(s, e)| (char_to_byte(t, s), char_to_byte(t, e))).collect()
}

fn sorted(set: &std::collections::HashSet<&'static str>) -> Vec<&'static str> {
    let mut v: Vec<_> = set.iter().copied().collect();
    v.sort();
    v
}

#[test]
fn lists() {
    Fixture::load("speller").check("lists", |_| {
        let missp: Map<String, Value> = speller::MISSPELLINGS.iter().map(|(k, v)| (k.to_string(), json(v))).collect();
        j!({"function_words": sorted(&speller::FUNCTION_WORDS), "contractions": sorted(&speller::CONTRACTIONS),
            "extra_words": sorted(&speller::EXTRA_WORDS), "misspellings": missp})
    });
}

#[test]
fn known() {
    Fixture::load("speller").check("known", |c| json(SPELLER.known(c.str(0))));
}

#[test]
fn suggest() {
    Fixture::load("speller").check("suggest", |c| {
        let n: usize = c.arg(1);
        let got = json(SPELLER.suggest(c.str(0), n));
        // Python ordered equal scores by set order; Rust alphabetically.
        let full = c.out.as_array().is_some_and(|w| w.len() == n);
        align_ties(&c.out, got, |v| j!((v[1].as_f64().unwrap() * 1e6).round()), full)
    });
}

#[test]
fn zipf() {
    Fixture::load("speller").check("zipf", |c| json(SPELLER.zipf(c.str(0))));
}

#[test]
fn check() {
    Fixture::load("speller").check("check", |c| {
        let t = c.str(0);
        let spans = SPELLER.check(t, &protected(c));
        json(spans.iter().map(|&(s, e)| (byte_to_char(t, s), byte_to_char(t, e))).collect::<Vec<_>>())
    });
}

#[test]
fn user_and_session_words() {
    let sp = Speller::new(thesaurus::get(), ["Zorbleck"]);
    assert!(sp.known("zorbleck") && !sp.known("quuxly"));
    sp.add("Quuxly", true);
    assert!(sp.known("quuxly"));
    sp.set_vocab(Some(["Blorptastic"]));
    assert!(sp.known("blorptastic"));
    sp.set_vocab(None::<Vec<String>>);
    assert!(!sp.known("blorptastic"));
}

fn issues(t: &str, list: Vec<Issue>) -> Value {
    json(
        list.into_iter()
            .map(|i| (byte_to_char(t, i.start), byte_to_char(t, i.end), i.kind, i.message, i.fixes))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn grammar_with_speller() {
    Fixture::load("grammar").check("grammar_rules", |c| issues(c.str(0), grammar_rules(c.str(0), &protected(c), Some(&SPELLER))));
}

#[test]
fn grammar_without_speller() {
    Fixture::load("grammar").check("grammar_rules_nospeller", |c| issues(c.str(0), grammar_rules(c.str(0), &protected(c), None)));
}
