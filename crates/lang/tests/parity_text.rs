//! text, diff and phrases against rewrite/textutil.py, app.py's diffs and rewrite/phrases.py (fixture groups
//! "textutil", "diff", "phrases").

use rewrite_lang::{diff, phrases, text};
use rewrite_testkit::{byte_to_char, char_to_byte, json, Fixture};
use serde_json::{Map, Value};

fn spans(s: &str, v: &[(usize, usize)]) -> Value {
    json(v.iter().map(|&(a, b)| (byte_to_char(s, a), byte_to_char(s, b))).collect::<Vec<_>>())
}

#[test]
fn words() {
    Fixture::load("textutil").check("words", |c| {
        let t = c.str(0);
        json(text::words(t).iter().map(|&(s, e, w)| (byte_to_char(t, s), byte_to_char(t, e), w)).collect::<Vec<_>>())
    });
}

#[test]
fn sentence_bounds() {
    Fixture::load("textutil").check("sentence_bounds", |c| spans(c.str(0), &text::sentence_bounds(c.str(0), c.arg(1))));
}

#[test]
fn sentence_at() {
    Fixture::load("textutil").check("sentence_at", |c| {
        let t = c.str(0);
        json(text::sentence_at(t, char_to_byte(t, c.arg(1))).map(|(s, e)| (byte_to_char(t, s), byte_to_char(t, e))))
    });
}

#[test]
fn match_case() {
    Fixture::load("textutil").check("match_case", |c| json(text::match_case(c.str(0), c.str(1))));
}

#[test]
fn word_diff_ratio() {
    Fixture::load("textutil").check("word_diff_ratio", |c| json(text::word_diff_ratio(c.str(0), c.str(1))));
}

#[test]
fn stopwords() {
    Fixture::load("textutil").check("stopwords", |_| {
        let mut w: Vec<&str> = text::STOPWORDS.iter().copied().collect();
        w.sort();
        json(w)
    });
}

#[test]
fn compute_changes() {
    Fixture::load("diff").check("compute_changes", |c| {
        let (old, new) = (c.str(0), c.str(1));
        let (changes, ratio) = diff::compute_changes(old, new);
        let changes: Vec<_> = changes
            .iter()
            .map(|x| {
                [byte_to_char(new, x.cur_start), byte_to_char(new, x.cur_end), byte_to_char(old, x.orig_start), byte_to_char(old, x.orig_end)]
            })
            .collect();
        json((changes, ratio))
    });
}

#[test]
fn diff_chunks() {
    Fixture::load("diff").check("diff_chunks", |c| {
        let old = c.str(0);
        let chunks: Vec<_> =
            diff::diff_chunks(old, c.str(1)).into_iter().map(|(s, e, r)| (byte_to_char(old, s), byte_to_char(old, e), r)).collect();
        json(chunks)
    });
}

#[test]
fn phrase_book() {
    Fixture::load("phrases").check("book", |_| {
        Value::Object(phrases::BOOK.iter().map(|(k, v)| (k.to_string(), json(v))).collect::<Map<_, _>>())
    });
}

#[test]
fn phrase_find() {
    Fixture::load("phrases").check("find", |c| {
        let t = c.str(0);
        json(phrases::find(t).iter().map(|&(s, e, m)| (byte_to_char(t, s), byte_to_char(t, e), m)).collect::<Vec<_>>())
    });
}

#[test]
fn phrase_lookup() {
    Fixture::load("phrases").check("lookup", |c| json(phrases::lookup(c.str(0))));
}
