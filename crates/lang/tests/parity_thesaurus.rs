//! thesaurus against rewrite/thesaurus.py (fixture group "thesaurus").

use rewrite_lang::thesaurus::{self, Candidate};
use rewrite_testkit::{align_ties, json, Fixture};
use serde_json::{Map, Value};
use std::time::Instant;

fn fixture() -> Fixture {
    Fixture::load("thesaurus")
}

#[test]
fn loads_quickly() {
    let t0 = Instant::now();
    let t = thesaurus::get();
    let took = t0.elapsed();
    assert!(t.synsets.len() > 90_000 && t.index.len() > 100_000);
    // Debug builds are much slower; release must stay well under half a second.
    let limit = if cfg!(debug_assertions) { 5.0 } else { 0.25 };
    assert!(took.as_secs_f64() < limit, "loading took {took:?}");
}

#[test]
fn lemmas() {
    let t = thesaurus::get();
    fixture().check("lemmas", |c| {
        Value::Object(t.lemmas(c.str(0)).iter().map(|(u, v)| (u.to_string(), json(v))).collect::<Map<_, _>>())
    });
}

#[test]
fn tags() {
    let t = thesaurus::get();
    fixture().check("tags", |c| {
        let mut tags = t.tags(c.str(0));
        tags.sort();
        json(tags)
    });
}

#[test]
fn zipf() {
    let t = thesaurus::get();
    fixture().check("zipf", |c| json(t.zipf(c.str(0))));
}

#[test]
fn antonyms() {
    let t = thesaurus::get();
    fixture().check("antonyms", |c| json(t.antonyms(c.str(0)).iter().collect::<Vec<_>>()));
}

#[test]
fn senses() {
    let t = thesaurus::get();
    fixture().check("senses", |c| json(t.senses(c.str(0))));
}

/// Candidates as recorded: text, relation, sense rank, pos, start of the gloss.
fn recorded(cs: &[Candidate]) -> Value {
    json(cs.iter().map(|c| (&c.text, c.relation, c.sense_rank, c.pos, c.gloss.chars().take(24).collect::<String>())).collect::<Vec<_>>())
}

#[test]
fn candidates() {
    let t = thesaurus::get();
    fixture().check("candidates", |c| {
        let limit: usize = c.arg(3);
        let got = recorded(&t.candidates(c.str(0), c.arg(1), c.arg(2), limit));
        // The forms of one synonym came in the order of a Python set of tags: compare each sense's run as a set.
        let sense = |v: &Value| json(&v.as_array().unwrap()[1..]);
        let full = c.out.as_array().is_some_and(|w| w.len() == limit);
        align_ties(&c.out, got, sense, full)
    });
}

#[test]
fn relation() {
    let t = thesaurus::get();
    fixture().check("relation", |c| json(t.relation(c.str(0), c.str(1))));
}

#[test]
fn same_lemma() {
    let t = thesaurus::get();
    fixture().check("same_lemma", |c| json(t.same_lemma(c.str(0), c.str(1))));
}

#[test]
fn is_antonym() {
    let t = thesaurus::get();
    fixture().check("is_antonym", |c| json(t.is_antonym(c.str(0), c.str(1))));
}

#[test]
fn inflect() {
    let t = thesaurus::get();
    fixture().check("inflect", |c| json(t.inflect(c.str(0), c.str(1))));
}

#[test]
fn lesk_rank() {
    let t = thesaurus::get();
    fixture().check("lesk_rank", |c| {
        let word = c.str(0);
        let ctx: Vec<String> = c.arg(1);
        let order: Vec<String> = c.arg(2);
        // Rank the candidates in the order Python had them (that order is only fixed up to tag order).
        let mut cands = t.candidates(word, 8, true, 60);
        cands.sort_by_key(|x| order.iter().position(|o| *o == x.text).unwrap_or(usize::MAX));
        let ranked = t.lesk_rank(word, &ctx, cands);
        json(ranked.iter().map(|x| (&x.text, x.score.unwrap())).collect::<Vec<_>>())
    });
}
