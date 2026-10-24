//! Cleaning up text taken from PDFs and splitting it into sentences.
//!
//! The hard calls (is "co-operate" one word or two? does "al." end a sentence?) are asked of a `Helper`:
//! the Python side answers with its dictionary and, when it is loaded, the context model. Without a
//! helper only safe rules are used.

use std::collections::HashSet;
use std::sync::OnceLock;

pub trait Helper {
    /// A word the dictionary knows.
    fn known(&self, _word: &str) -> bool {
        false
    }
    /// How common a word is (0..7).
    fn zipf(&self, _word: &str) -> f64 {
        0.0
    }
    /// Which option fits best between `before` and `after` (the context model); None when unsure.
    fn choose(&self, _before: &str, _options: &[String], _after: &str) -> Option<usize> {
        None
    }
}

pub struct NoHelper;

impl Helper for NoHelper {}

pub fn ligatures(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ﬀ' => out.push_str("ff"),
            'ﬁ' => out.push_str("fi"),
            'ﬂ' => out.push_str("fl"),
            'ﬃ' => out.push_str("ffi"),
            'ﬄ' => out.push_str("ffl"),
            'ﬅ' | 'ﬆ' => out.push_str("st"),
            '\u{ad}' | '\u{200b}' | '\u{feff}' => {}
            '\u{a0}' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// "T I T L E" (letters spaced out for show) -> "TITLE".
pub fn collapse_spaced(s: &str) -> String {
    let parts: Vec<&str> = s.split(' ').collect();
    if parts.len() >= 4 && parts.iter().filter(|p| !p.is_empty()).all(|p| p.chars().count() == 1 && p.chars().all(|c| c.is_alphanumeric())) {
        return parts.concat();
    }
    s.to_string()
}

/// A word broken over two lines ("infor-" + "mation"): one word, or a hyphenated compound?
pub fn dehyphen(left: &str, right: &str, before: &str, after: &str, h: &dyn Helper) -> String {
    let lw: String = left.chars().rev().take_while(|c| c.is_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
    let rw: String = right.chars().take_while(|c| c.is_alphabetic()).collect();
    if lw.is_empty() || rw.is_empty() || !rw.chars().next().unwrap().is_lowercase() {
        return format!("{left}-{right}");
    }
    let joined = format!("{lw}{rw}").to_lowercase();
    let (lk, rk, jk) = (h.known(&lw.to_lowercase()), h.known(&rw.to_lowercase()), h.known(&joined));
    let one = format!("{left}{right}");
    let two = format!("{left}-{right}");
    if jk && !(lk && rk && lw.len() > 2 && rw.len() > 2) {
        return one;
    }
    if lk && rk && !jk {
        return two;
    }
    if jk && lk && rk {
        // both readings are words ("co-operate", "re-cover"): the context model decides
        if let Some(k) = h.choose(before, &[format!("{lw}{rw}"), format!("{lw}-{rw}")], after) {
            return if k == 0 { one } else { two };
        }
        return if h.zipf(&joined) >= 2.5 { one } else { two };
    }
    // neither is a known word: a word broken by the line end is far more common than a compound
    one
}

/// Split a run-together word ("thequickbrown") into common words, when that is clearly better.
pub fn split_run_together(word: &str, h: &dyn Helper) -> Option<String> {
    let lower = word.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    if chars.len() < 9 || !chars.iter().all(|c| c.is_alphabetic()) || h.known(&lower) {
        return None;
    }
    let n = chars.len();
    // best[i] = (score, previous cut) for the first i characters
    let mut best: Vec<Option<(f64, usize)>> = vec![None; n + 1];
    best[0] = Some((0.0, 0));
    for i in 1..=n {
        for j in i.saturating_sub(18)..i {
            let Some((s, _)) = best[j] else { continue };
            let piece: String = chars[j..i].iter().collect();
            let len = piece.chars().count();
            if !(len >= 2 || piece == "a" || piece == "i") || !h.known(&piece) {
                continue;
            }
            let z = h.zipf(&piece);
            if z < 3.0 {
                continue;
            }
            let score = s + z - 4.0; // fewer, more common words win
            if best[i].is_none_or(|(b, _)| score > b) {
                best[i] = Some((score, j));
            }
        }
    }
    best[n]?;
    let mut cuts = vec![];
    let mut i = n;
    while i > 0 {
        let (_, j) = best[i].unwrap();
        cuts.push(j);
        i = j;
    }
    cuts.reverse();
    if cuts.len() < 2 || cuts.len() > 5 {
        return None;
    }
    let orig: Vec<char> = word.chars().collect();
    let mut out = String::new();
    for (k, &c) in cuts.iter().enumerate() {
        let end = cuts.get(k + 1).copied().unwrap_or(n);
        if k > 0 {
            out.push(' ');
        }
        out.extend(&orig[c..end]);
    }
    Some(out)
}

fn abbreviations() -> &'static HashSet<&'static str> {
    static A: OnceLock<HashSet<&str>> = OnceLock::new();
    A.get_or_init(|| {
        ["e.g", "i.e", "etc", "vs", "al", "fig", "figs", "eq", "eqs", "sec", "ch", "no", "vol", "pp", "dr", "mr", "mrs", "ms",
         "prof", "st", "jr", "sr", "inc", "ltd", "co", "cf", "approx", "resp", "jan", "feb", "mar", "apr", "jun", "jul", "aug",
         "sep", "sept", "oct", "nov", "dec", "u.s", "ref", "refs", "tab", "thm", "def", "lem", "eds", "ed", "repr", "viz"]
            .into_iter().collect()
    })
}

/// Byte offsets where sentences start in a paragraph (after the first one).
pub fn sentence_starts(text: &str, h: &dyn Helper) -> Vec<usize> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !(c == b'.' || c == b'!' || c == b'?') {
            i += 1;
            continue;
        }
        // the end of the punctuation (with closing quotes and brackets)
        let mut j = i + 1;
        while j < b.len() && matches!(b[j], b'.' | b'!' | b'?' | b'"' | b'\'' | b')' | b']') {
            j += 1;
        }
        let rest = &text[j..];
        let rest_trim = rest.trim_start_matches([' ', '\t']);
        if rest_trim.len() == rest.len() || rest_trim.is_empty() {
            i = j;
            continue; // no space after it: "3.5", "e.g."
        }
        let next_at = text.len() - rest_trim.len();
        let next = rest_trim.chars().next().unwrap();
        // the closing quotes of „ “ ” ‘ ’ are multi-byte: look at characters, not bytes
        let starts_like_sentence = next.is_uppercase() || next.is_ascii_digit() || "\"“‘'([".contains(next);
        if !starts_like_sentence {
            i = j;
            continue;
        }
        if c == b'.' {
            let before = &text[..i];
            let tok = before.rsplit(|ch: char| ch.is_whitespace() || ch == '(' || ch == '[').next().unwrap_or("").trim_start_matches(['"', '\'', '“']);
            let low = tok.to_lowercase();
            let initial = tok.chars().count() == 1 && tok.chars().all(|ch| ch.is_uppercase());
            let number = !tok.is_empty() && tok.chars().all(|ch| ch.is_ascii_digit());
            if abbreviations().contains(low.as_str()) || initial || (number && next.is_ascii_digit()) {
                // "et al. The" ends a sentence, "Fig. 3" and "J. Smith" do not: ask the context model
                let word: String = rest_trim.chars().take_while(|ch| ch.is_alphabetic()).collect();
                let mut boundary = false;
                if !word.is_empty() && next.is_uppercase() && (low == "al" || low == "etc") {
                    let lowered = word.to_lowercase();
                    if h.known(&lowered) {
                        let after: String = rest_trim[word.len()..].chars().take(120).collect();
                        let start = before.len().saturating_sub(240);
                        let start = (start..=before.len()).find(|k| text.is_char_boundary(*k)).unwrap_or(0);
                        let ctx = format!("{}.", &before[start..]);
                        boundary = h.choose(&ctx, &[word.clone(), lowered], &after) == Some(0);
                    }
                }
                if !boundary {
                    i = j;
                    continue;
                }
            }
        }
        out.push(next_at);
        i = j;
    }
    out
}
