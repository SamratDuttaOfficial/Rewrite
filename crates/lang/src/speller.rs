//! Spelling (was `proof.Speller`): which words are known, which look misspelled, and likely corrections.
//!
//! No extra word list is needed: a word counts as known when it is in the WordNet thesaurus, in lemminflect's
//! lexicon (inflected forms), in the small lists in `data/`, or, when the context model is loaded, a whole
//! word of its vocabulary. User words persist; session words last until the app closes.

use crate::inflect;
use crate::py;
use crate::text::match_case;
use crate::thesaurus::Thesaurus;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{LazyLock, Mutex, OnceLock, RwLock};

fn word_set(text: &'static str) -> HashSet<&'static str> {
    text.split_whitespace().collect()
}

/// Function words: always known, never corrected into.
pub static FUNCTION_WORDS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| word_set(include_str!("../data/function_words.txt")));
pub static CONTRACTIONS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| word_set(include_str!("../data/contractions.txt")));
/// Everyday and technical words that are missing from WordNet 3.0 and the lexicon.
pub static EXTRA_WORDS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| word_set(include_str!("../data/extra_words.txt")));
/// Frequent misspellings and their corrections. These are never accepted, even when some word list has them.
pub static MISSPELLINGS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    include_str!("../data/misspellings.txt")
        .split_whitespace()
        .filter_map(|x| x.split_once('>'))
        .map(|(k, v)| (k, v.replace('_', " ")))
        .collect()
});

const PREFIXES: [&str; 25] = [
    "un", "re", "non", "pre", "over", "under", "mis", "multi", "co", "sub", "inter", "anti", "semi", "super", "hyper",
    "micro", "macro", "meta", "post", "self", "cross", "auto", "pseudo", "de", "dis",
];

/// Every spelling one edit away (deletion, transposition, replacement, insertion of a-z), without `w`.
pub fn edits1(w: &str) -> HashSet<String> {
    let chars: Vec<char> = w.chars().collect();
    let n = chars.len();
    let s = |a: &[char]| a.iter().collect::<String>();
    let mut out = HashSet::new();
    for i in 0..=n {
        let (a, b) = (&chars[..i], &chars[i..]);
        if !b.is_empty() {
            out.insert(s(a) + &s(&b[1..]));
            for c in 'a'..='z' {
                out.insert(format!("{}{c}{}", s(a), s(&b[1..])));
            }
        }
        if b.len() > 1 {
            out.insert(format!("{}{}{}{}", s(a), b[1], b[0], s(&b[2..])));
        }
        for c in 'a'..='z' {
            out.insert(format!("{}{c}{}", s(a), s(b)));
        }
    }
    out.remove(w);
    out
}

/// `formats.overlaps`: whether `[start, end)` touches any of the sorted, merged `spans`.
pub fn overlaps(spans: &[(usize, usize)], start: usize, end: usize) -> bool {
    let i = spans.partition_point(|&(s, _)| s <= start);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|j| spans.get(j)).any(|&(s, e)| s < end && start < e)
}

/// `’` as `'`, lower case: the key words are known and corrected by.
fn key(word: &str) -> String {
    word.replace('’', "'").to_lowercase()
}

#[derive(Default)]
struct Words {
    user: HashSet<String>,
    session: HashSet<String>,
    /// Whole-word tokens of the context model, once it is loaded.
    vocab: Option<HashSet<String>>,
}

/// Corrections of recent words, the oldest dropped first.
#[derive(Default)]
struct Fixes {
    map: HashMap<String, Vec<(String, f64)>>,
    order: VecDeque<String>,
}

pub struct Speller {
    pub t: &'static Thesaurus,
    words: RwLock<Words>,
    known: Mutex<HashMap<String, bool>>,
    fixes: Mutex<Fixes>,
    /// Every listed word, built on first use to speed up corrections.
    all_words: OnceLock<HashSet<&'static str>>,
}

impl Speller {
    pub fn new<S: AsRef<str>>(thes: &'static Thesaurus, user_words: impl IntoIterator<Item = S>) -> Speller {
        let user = user_words.into_iter().map(|w| w.as_ref().to_lowercase()).collect();
        Speller {
            t: thes,
            words: RwLock::new(Words { user, ..Words::default() }),
            known: Mutex::default(),
            fixes: Mutex::default(),
            all_words: OnceLock::new(),
        }
    }

    // -- word lists ---------------------------------------------------------------------------------------

    /// The context model's whole words (None or empty: no model).
    pub fn set_vocab<S: AsRef<str>>(&self, words: Option<impl IntoIterator<Item = S>>) {
        let vocab: Option<HashSet<String>> = words.map(|w| w.into_iter().map(|s| s.as_ref().to_string()).collect());
        self.words.write().unwrap().vocab = vocab.filter(|v| !v.is_empty());
        self.known.lock().unwrap().clear();
    }

    /// Adds a user word, or with `session` a word ignored until the app closes.
    pub fn add(&self, word: &str, session: bool) {
        let w = key(word);
        let mut words = self.words.write().unwrap();
        if session { words.session.insert(w) } else { words.user.insert(w) };
        drop(words);
        self.known.lock().unwrap().clear();
    }

    /// A word from the thesaurus or the lexicon (no abbreviations, no guessing).
    pub fn in_dictionary(&self, lw: &str) -> bool {
        FUNCTION_WORDS.contains(lw) || self.t.has(lw) || inflect::has_lemmas(lw)
    }

    fn listed(&self, lw: &str) -> bool {
        !MISSPELLINGS.contains_key(lw) && (EXTRA_WORDS.contains(lw) || CONTRACTIONS.contains(lw) || self.in_dictionary(lw))
    }

    /// A regular inflection the lexicon lacks ("dependencies"): its rule-based lemma must be a thesaurus word of
    /// that part of speech and must inflect back to exactly this spelling.
    fn inflected(&self, lw: &str) -> bool {
        for (upos, wn) in [("NOUN", "n"), ("VERB", "v"), ("ADJ", "a")] {
            for lem in inflect::get_all_lemmas_oov(lw, upos).get(upos).unwrap_or(&[]) {
                let senses = self.t.index.get(lem.as_str());
                if lem != lw && senses.is_some_and(|s| s.iter().any(|&sid| self.t.synsets[sid as usize].pos == wn)) {
                    let mut forms = inflect::get_all_inflections(lem, Some(upos));
                    if forms.is_empty() {
                        forms = inflect::get_all_inflections_oov(lem, upos);
                    }
                    if forms.values().any(|v| v == lw) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn word_set(&self) -> &HashSet<&'static str> {
        self.all_words.get_or_init(|| {
            let mut words: HashSet<&'static str> = FUNCTION_WORDS.iter().chain(EXTRA_WORDS.iter()).chain(CONTRACTIONS.iter()).copied().collect();
            words.extend(self.t.index.keys().filter(|w| !w.contains(' ')));
            words.extend(inflect::lemma_dict_keys().iter().filter(|w| w.chars().all(py::is_alpha) && !w.is_empty() && py::is_lower(w)));
            words.retain(|w| !MISSPELLINGS.contains_key(w));
            words
        })
    }

    /// Rough commonness of a known word (0..7).
    pub fn zipf(&self, lw: &str) -> f64 {
        if FUNCTION_WORDS.contains(lw) {
            return 6.0;
        }
        let mut z = self.t.zipf(lw);
        if z == 0.0 {
            z = inflect::get_all_lemmas(lw, None).values().map(|x| self.t.zipf(x)).fold(0.0, f64::max);
            z = if z != 0.0 { z - 0.3 } else { 0.0 };
        }
        if z != 0.0 {
            z
        } else if EXTRA_WORDS.contains(lw) || CONTRACTIONS.contains(lw) {
            3.0
        } else {
            1.5
        }
    }

    pub fn known(&self, word: &str) -> bool {
        let lw = key(word);
        if let Some(&hit) = self.known.lock().unwrap().get(&lw) {
            return hit;
        }
        let hit = self.check_word(&lw);
        self.known.lock().unwrap().insert(lw, hit);
        hit
    }

    fn check_word(&self, lw: &str) -> bool {
        {
            let words = self.words.read().unwrap();
            if lw.chars().count() <= 2 || words.user.contains(lw) || words.session.contains(lw) {
                return true;
            }
            if MISSPELLINGS.contains_key(lw) {
                return false;
            }
            let in_vocab =
                words.vocab.as_ref().is_some_and(|v| v.contains(lw) || v.contains(py::capitalize(lw).as_str()));
            if in_vocab || self.listed(lw) {
                return true;
            }
        }
        if lw.contains('\'') {
            // possessives
            return ["'s", "s'", "'"].iter().any(|sfx| lw.ends_with(sfx) && self.known(&lw[..lw.len() - sfx.len()]));
        }
        if lw.contains('-') {
            return lw.split('-').all(|p| self.known(p));
        }
        if self.inflected(lw) {
            return true;
        }
        // unhelpfully, reranked, nonlinear
        let n = lw.chars().count();
        if PREFIXES.iter().any(|p| lw.starts_with(p) && n - p.len() >= 4 && self.listed(&lw[p.len()..])) {
            return true;
        }
        self.compound(lw)
    }

    /// checkbox, filename, timestep: two common nouns written as one word.
    fn compound(&self, lw: &str) -> bool {
        let chars: Vec<(usize, char)> = lw.char_indices().collect();
        let noun = |x: &str| {
            self.t.index.get(x).is_some_and(|s| {
                self.t.zipf(x) >= 3.3 && s.iter().any(|&sid| self.t.synsets[sid as usize].pos == "n")
            })
        };
        for i in 3..chars.len().saturating_sub(2) {
            let (a, b) = lw.split_at(chars[i].0);
            if ["full", "ting", "able"].contains(&b) || FUNCTION_WORDS.contains(a) || FUNCTION_WORDS.contains(b) {
                continue;
            }
            if noun(a) && noun(b) {
                return true;
            }
        }
        false
    }

    // -- checking -----------------------------------------------------------------------------------------

    /// Byte spans of words that look misspelled, skipping `protected` spans (sorted and merged).
    pub fn check(&self, text: &str, protected: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut names = HashSet::new();
        let mut starts = Vec::new();
        for (s, e) in letter_tokens(text) {
            let w = text[s..e].replace('’', "'");
            let mut chars = w.chars();
            if w.chars().count() < 3 || !w.is_ascii() || chars.next().is_some_and(|_| chars.any(char::is_uppercase)) {
                continue; // other scripts, ACRONYMS, camelCase
            }
            let before = &text[py::back(text, s, 2)..s];
            let after = &text[e..py::ahead(text, e, 2)];
            let (b_last, b_first) = (before.chars().next_back(), before.chars().next());
            let (a_first, a_second) = (after.chars().next(), after.chars().nth(1));
            if b_last.is_some_and(|c| "_\\@#/&".contains(c) || py::is_digit(c))
                || a_first.is_some_and(|c| "_@/".contains(c) || py::is_digit(c))
            {
                continue; // identifiers, paths, x86
            }
            if (b_last == Some('.') && b_first.is_some_and(py::is_alnum))
                || (a_first == Some('.') && a_second.is_some_and(py::is_alnum))
            {
                continue; // file.txt, example.com
            }
            if overlaps(protected, s, e) {
                continue;
            }
            if py::starts_upper(&w) {
                if starts_sentence(text, s) {
                    starts.push((s, e, w));
                } else {
                    names.insert(w); // capitalised inside a sentence: a name
                }
            } else if !self.known(&w) {
                out.push((s, e));
            }
        }
        for (s, e, w) in starts {
            // A capitalised first word is only flagged when it clearly is a typo of a common word, so names at
            // the start of a sentence are left alone.
            if names.contains(&w) || self.known(&w) {
                continue;
            }
            let lw = w.to_lowercase();
            if MISSPELLINGS.contains_key(lw.as_str()) || edits1(&lw).iter().any(|c| self.listed(c) && self.zipf(c) >= 3.3) {
                out.push((s, e));
            }
        }
        out.sort();
        out
    }

    // -- corrections --------------------------------------------------------------------------------------

    /// Likely corrections, best first: `(text, prior score)`, in the word's capitalisation.
    pub fn suggest(&self, word: &str, n: usize) -> Vec<(String, f64)> {
        let lw = key(word);
        let cached = self.fixes.lock().unwrap().map.get(&lw).cloned();
        let fixes = cached.unwrap_or_else(|| {
            let fixes = self.suggest_word(&lw);
            let mut cache = self.fixes.lock().unwrap();
            if cache.map.insert(lw.clone(), fixes.clone()).is_none() {
                cache.order.push_back(lw);
            }
            if cache.map.len() > 2000 {
                if let Some(old) = cache.order.pop_front() {
                    cache.map.remove(&old);
                }
            }
            fixes
        });
        fixes.into_iter().take(n).map(|(t, sc)| (if py::starts_lower(&t) { match_case(word, &t) } else { t }, sc)).collect()
    }

    fn suggest_word(&self, lw: &str) -> Vec<(String, f64)> {
        let words = self.word_set();
        let first = lw.chars().next();
        let mut scores: HashMap<String, f64> = HashMap::new();
        let offer = |scores: &mut HashMap<String, f64>, cand: &str, dist: f64| {
            if cand.chars().count() > 1 && cand != lw {
                let sc = self.zipf(cand) - 2.2 * dist + if cand.chars().next() == first { 0.6 } else { 0.0 };
                let slot = scores.entry(cand.to_string()).or_insert(-99.0);
                *slot = slot.max(sc);
            }
        };
        let e1 = edits1(lw);
        for c in e1.iter().filter(|c| words.contains(c.as_str())) {
            offer(&mut scores, c, 1.0);
        }
        if scores.len() < 4 && lw.chars().count() <= 16 {
            let near: HashSet<String> = scores.keys().cloned().collect();
            for c in &e1 {
                for c2 in edits1(c) {
                    if words.contains(c2.as_str()) && !near.contains(&c2) {
                        offer(&mut scores, &c2, 2.0);
                    }
                }
            }
        }
        let common = |x: &str| FUNCTION_WORDS.contains(x) || (x.chars().count() > 2 && words.contains(x) && self.zipf(x) >= 4.3);
        for (i, _) in lw.char_indices().skip(1) {
            // a missing space: "alot", "infact"
            let (a, b) = lw.split_at(i);
            if common(a) && common(b) {
                scores.insert(format!("{a} {b}"), self.zipf(a).min(self.zipf(b)) - 2.6);
            }
        }
        let mut out: Vec<(String, f64)> = scores.into_iter().collect();
        // Best first; equal scores alphabetically (Python kept them in set order).
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(12);
        if let Some(best) = MISSPELLINGS.get(lw) {
            out.retain(|x| &x.0 != best);
            out.insert(0, (best.clone(), 9.0));
        }
        out
    }
}

/// Runs of letters joined by apostrophes or hyphens (`[^\W\d_]+(?:['’-][^\W\d_]+)*`), as byte spans.
fn letter_tokens(text: &str) -> Vec<(usize, usize)> {
    let letter = |c: char| c != '_' && py::is_word(c) && !py::is_decimal(c);
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(c) = py::char_at(text, pos) {
        if !letter(c) {
            pos += c.len_utf8();
            continue;
        }
        let start = pos;
        loop {
            while let Some(c) = py::char_at(text, pos).filter(|&c| letter(c)) {
                pos += c.len_utf8();
            }
            let mut ahead = text[pos..].chars();
            match (ahead.next(), ahead.next()) {
                (Some(j @ ('\'' | '’' | '-')), Some(d)) if letter(d) => pos += j.len_utf8(),
                _ => break,
            }
        }
        out.push((start, pos));
    }
    out
}

/// Whether the word at byte `s` opens a sentence (only quotes, brackets and markup before it).
fn starts_sentence(text: &str, s: usize) -> bool {
    match text[..s].chars().rev().find(|c| !" \t\"'“‘([*_#>-".contains(*c)) {
        None => true,
        Some(c) => ".!?:\n".contains(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits() {
        let e = edits1("ab");
        assert!(e.contains("b") && e.contains("ba") && e.contains("abc") && e.contains("xb") && !e.contains("ab"));
        // 2 deletions, 1 transposition, 50 replacements, 76 distinct insertions
        assert_eq!(e.len(), 129);
    }

    #[test]
    fn overlapping() {
        let spans = [(0, 3), (5, 9)];
        assert!(overlaps(&spans, 2, 4) && overlaps(&spans, 8, 20) && !overlaps(&spans, 3, 5) && !overlaps(&spans, 9, 12));
        assert!(!overlaps(&[], 0, 1));
    }
}
