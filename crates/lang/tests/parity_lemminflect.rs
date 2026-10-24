//! inflect against lemminflect 0.2.3 (fixture group "lemminflect").

use rewrite_lang::inflect::{self, Forms};
use rewrite_testkit::{json, Case, Fixture};
use serde_json::{Map, Value};

fn forms(f: Forms) -> Value {
    Value::Object(f.iter().map(|(t, v)| (t.to_string(), json(v))).collect::<Map<_, _>>())
}

fn upos(c: &Case) -> Option<String> {
    c.args.get(1).and_then(|v| v.as_str()).map(str::to_string)
}

#[test]
fn get_all_lemmas() {
    Fixture::load("lemminflect").check("getAllLemmas", |c| forms(inflect::get_all_lemmas(c.str(0), upos(c).as_deref())));
}

#[test]
fn get_all_lemmas_oov() {
    Fixture::load("lemminflect").check("getAllLemmasOOV", |c| forms(inflect::get_all_lemmas_oov(c.str(0), c.str(1))));
}

#[test]
fn get_all_inflections() {
    Fixture::load("lemminflect")
        .check("getAllInflections", |c| forms(inflect::get_all_inflections(c.str(0), upos(c).as_deref())));
}

#[test]
fn get_all_inflections_oov() {
    Fixture::load("lemminflect")
        .check("getAllInflectionsOOV", |c| forms(inflect::get_all_inflections_oov(c.str(0), c.str(1))));
}

#[test]
fn get_inflection() {
    Fixture::load("lemminflect").check("getInflection", |c| json(inflect::get_inflection(c.str(0), c.str(1), c.arg(2))));
}

#[test]
fn lemma_dict_keys() {
    let keys = inflect::lemma_dict_keys();
    // what list(Lemmatizer()._getLemmaDict()) gives
    assert_eq!(keys.len(), 69_980);
    assert_eq!((keys[0], keys[1], keys[2], keys[keys.len() - 1]), ("'d", "'ll", "'m", "zygotes"));
}
