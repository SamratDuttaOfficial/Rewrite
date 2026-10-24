//! Curated phrase book: multi-word expressions and equivalent rewordings (was `phrases.py`).
//!
//! Entries are lower case. Alternatives are ranked by the context model when it is available; in Lite mode they
//! are shown in the order listed in `data/phrases.txt`.

use crate::py;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Phrases and their alternatives, in the order of the source file.
pub struct PhraseBook {
    entries: Vec<(String, Vec<String>)>,
    index: HashMap<String, usize>,
    /// Phrases by first letter, longest first (ties in book order): the order in which the alternatives of
    /// Python's pattern were tried.
    by_first: HashMap<char, Vec<usize>>,
}

impl PhraseBook {
    fn parse(raw: &str) -> PhraseBook {
        let mut book = PhraseBook { entries: Vec::new(), index: HashMap::new(), by_first: HashMap::new() };
        for line in py::strip(raw).lines() {
            let Some((k, v)) = line.split_once('|') else { continue };
            let k = py::strip(k).to_lowercase();
            let n = *book.index.entry(k.clone()).or_insert_with(|| {
                book.entries.push((k.clone(), Vec::new()));
                book.entries.len() - 1
            });
            for a in v.split(';').map(py::strip).filter(|a| !a.is_empty()) {
                let alts = &mut book.entries[n].1;
                if !alts.iter().any(|x| x == a) && a != k {
                    alts.push(a.to_string());
                }
            }
        }
        let mut by_length: Vec<usize> = (0..book.entries.len()).collect();
        by_length.sort_by_key(|&i| std::cmp::Reverse(book.entries[i].0.chars().count()));
        for i in by_length {
            if let Some(c) = book.entries[i].0.chars().next() {
                book.by_first.entry(c).or_default().push(i);
            }
        }
        book
    }

    pub fn get(&self, phrase: &str) -> Option<&[String]> {
        self.index.get(phrase).map(|&i| self.entries[i].1.as_slice())
    }

    pub fn contains_key(&self, phrase: &str) -> bool {
        self.index.contains_key(phrase)
    }

    /// `(phrase, alternatives)` in book order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub static BOOK: LazyLock<PhraseBook> = LazyLock::new(|| PhraseBook::parse(include_str!("../data/phrases.txt")));

/// Alternatives for a phrase (case and spacing ignored); empty when it is not in the book.
pub fn lookup(phrase: &str) -> Vec<String> {
    let key = py::split_ws(&py::strip(phrase).to_lowercase()).collect::<Vec<_>>().join(" ");
    BOOK.get(&key).map(<[String]>::to_vec).unwrap_or_default()
}

/// A letter, apostrophe or hyphen: a phrase must not touch one on either side.
fn joins(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '\'' || c == '-'
}

/// A text character folded the way Python's `re.I` compares it with a lower-case ASCII letter (simple case
/// folding: "ſ" is "s", the Kelvin sign is "k", dotless and dotted "i" are "i").
fn fold(c: char) -> char {
    match c {
        'ſ' => 's',
        '\u{212a}' => 'k',
        'ı' | 'İ' => 'i',
        c => c.to_ascii_lowercase(),
    }
}

/// The end of `phrase` matched at byte `at` of `text` (a space in the phrase matches any run of white space).
fn match_at(text: &str, at: usize, phrase: &str) -> Option<usize> {
    let mut it = text[at..].char_indices().peekable();
    for k in phrase.chars() {
        if k == ' ' {
            let mut n = 0;
            while it.next_if(|&(_, c)| py::is_space(c)).is_some() {
                n += 1;
            }
            if n == 0 {
                return None;
            }
        } else if fold(it.next()?.1) != k {
            return None;
        }
    }
    Some(it.peek().map_or(text.len(), |&(i, _)| at + i))
}

/// Phrase-book matches `(start, end, matched text)`, leftmost and longest first, not overlapping, never inside
/// a longer word. Byte offsets.
pub fn find(text: &str) -> Vec<(usize, usize, &str)> {
    let book = &*BOOK;
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(c) = py::char_at(text, pos) {
        if !py::char_before(text, pos).is_some_and(joins) {
            let hit = book.by_first.get(&fold(c)).and_then(|keys| {
                keys.iter().find_map(|&i| {
                    let end = match_at(text, pos, &book.entries[i].0)?;
                    (!py::char_at(text, end).is_some_and(joins)).then_some(end)
                })
            });
            if let Some(end) = hit {
                out.push((pos, end, &text[pos..end]));
                pos = end;
                continue;
            }
        }
        pos += c.len_utf8();
    }
    out
}
