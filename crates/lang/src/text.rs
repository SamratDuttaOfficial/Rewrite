//! Small text helpers: words, sentences, case matching (was `textutil.py`). Offsets are byte offsets.

use crate::diff::SequenceMatcher;
use crate::py;
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

/// A word: ASCII letters with inner apostrophes and hyphen-joined parts ("don't", "well-known").
pub static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z][A-Za-z']*(?:-[A-Za-z]+)*").unwrap());

/// Abbreviations that end with a period but do not end a sentence.
const ABBREV: &[&str] = &[
    "e.g", "i.e", "etc", "vs", "al", "fig", "figs", "eq", "eqs", "sec", "ch", "no", "vol", "pp", "dr", "mr", "mrs",
    "ms", "prof", "st", "jr", "sr", "inc", "ltd", "co", "cf", "approx", "resp", "jan", "feb", "mar", "apr", "jun",
    "jul", "aug", "sep", "sept", "oct", "nov", "dec", "u.s", "ref",
];

/// Words left out when comparing or ranking content words.
pub static STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "a about above after again against all am an and any are as at be because been before being below
between both but by can could did do does doing down during each few for from further had has have
having he her here hers herself him himself his how i if in into is it its itself just me more most my
myself no nor not now of off on once only or other our ours ourselves out over own same she should so
some such than that the their theirs them themselves then there these they this those through to too
under until up very was we were what when where which while who whom why will with would you your yours
yourself yourselves also may might must shall s t don let us one ones get got"
        .split_whitespace()
        .collect()
});

static SENT_END: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r#"[.!?]+["')\]]*(?=\s)|\n\s*\n"#).unwrap());
static LINE_START: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(
        r"^\s*(#{1,6}\s|[-*+]\s|\d+[.)]\s|>|%|\||\\(?!(?:textbf|textit|emph|cite\w*|ref|eqref|footnote|url|href|textsc|underline|mbox|LaTeX|TeX)\b)[A-Za-z])",
    )
    .unwrap()
});
static STANDALONE: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"^\s*(#|%|\||\\(?!item\b|textbf|textit|emph)[A-Za-z])").unwrap());

/// Words with their byte spans.
pub fn words(text: &str) -> Vec<(usize, usize, &str)> {
    WORD_RE.find_iter(text).map(|m| (m.start(), m.end(), m.as_str())).collect()
}

fn lines_of(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for ln in text.split('\n') {
        if !py::strip(ln).is_empty() {
            out.push((pos, pos + ln.len()));
        }
        pos += ln.len() + 1;
    }
    out
}

fn matches(re: &FancyRegex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

/// Splits text at blank lines and around heading, command and title-like lines.
fn blocks(text: &str) -> Vec<(usize, usize)> {
    let mut cuts = BTreeSet::from([0, text.len()]);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut pos = 0;
    for (i, ln) in lines.iter().enumerate() {
        let (st, en) = (pos, pos + ln.len());
        pos = en + 1;
        let stripped = py::strip(ln);
        if stripped.is_empty() {
            cuts.extend([st, en]);
            continue;
        }
        if matches(&LINE_START, ln) {
            cuts.insert(st);
            if matches(&STANDALONE, ln) {
                cuts.insert(en);
            }
        } else if py::split_ws(stripped).count() <= 10
            && !stripped.ends_with(['.', '!', '?', ':', ';', ',', '"', '\'', ')', ']'])
        {
            let nxt = lines.get(i + 1).copied().unwrap_or("");
            if py::strip(nxt).is_empty() || py::starts_upper(py::strip(nxt)) || matches(&LINE_START, nxt) {
                cuts.extend([st, en]); // a title-like line
            }
        }
    }
    let cuts: Vec<usize> = cuts.into_iter().collect();
    cuts.windows(2).map(|w| (w[0], w[1])).filter(|&(a, b)| !py::strip(&text[a..b]).is_empty()).collect()
}

/// Byte spans of the sentences in `text`. With `lines` every line is a paragraph of its own (formatted
/// documents); otherwise blank lines and title-like lines separate paragraphs, and single line breaks may fall
/// inside a sentence.
pub fn sentence_bounds(text: &str, lines: bool) -> Vec<(usize, usize)> {
    let paragraphs = if lines { lines_of(text) } else { blocks(text) };
    let mut out = Vec::new();
    for (bs, be) in paragraphs {
        out.extend(sentences(&text[bs..be]).into_iter().map(|(s, e)| (bs + s, bs + e)));
    }
    out
}

fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for m in SENT_END.find_iter(text).flatten() {
        let end = m.end();
        if m.as_str() == "." {
            let tail = &text[py::back(text, m.start(), 12)..m.start()];
            let tok = tail.rsplit(py::is_space).next().unwrap_or("").to_lowercase();
            let tok = tok.trim_start_matches(['(', '[', '"', '\'']);
            let abbrev = ABBREV.contains(&tok);
            let initial = tok.len() == 1 && tok.as_bytes()[0].is_ascii_lowercase();
            if abbrev || initial || (!tok.is_empty() && tok.chars().all(py::is_decimal)) {
                // "Fig. 3", "e.g. this", single initials: not a boundary
                let nxt = &text[end..py::ahead(text, end, 3)];
                if !py::starts_upper(nxt.trim_start_matches(py::is_space)) || abbrev {
                    continue;
                }
            }
        }
        if !py::strip(&text[start..end]).is_empty() {
            spans.push((start, end));
        }
        start = end;
    }
    if !py::strip(&text[start..]).is_empty() {
        spans.push((start, text.len()));
    }
    spans
        .into_iter()
        .filter_map(|(s, e)| {
            let inner = &text[s..e];
            let lead = inner.len() - inner.trim_start_matches(py::is_space).len();
            let trimmed = inner.trim_matches(py::is_space);
            (!trimmed.is_empty()).then(|| (s + lead, s + lead + trimmed.len()))
        })
        .collect()
}

/// The sentence containing byte offset `pos` (its ends included).
pub fn sentence_at(text: &str, pos: usize) -> Option<(usize, usize)> {
    sentence_bounds(text, false).into_iter().find(|&(s, e)| s <= pos && pos <= e)
}

/// Gives `word` the capitalisation style of `template`.
pub fn match_case(template: &str, word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    if py::is_upper(template) && template.chars().count() > 1 {
        return word.to_uppercase();
    }
    if py::starts_upper(template) {
        return py::upper_first(word);
    }
    word.to_string()
}

/// Share of the words of `b` that are not aligned to identical words of `a` (case ignored).
pub fn word_diff_ratio(a: &str, b: &str) -> f64 {
    let wa: Vec<String> = words(a).into_iter().map(|w| w.2.to_lowercase()).collect();
    let wb: Vec<String> = words(b).into_iter().map(|w| w.2.to_lowercase()).collect();
    if wb.is_empty() {
        return 0.0;
    }
    let same: usize = SequenceMatcher::new(&wa, &wb, false).get_matching_blocks().iter().map(|m| m.size).sum();
    1.0 - same as f64 / wb.len().max(1) as f64
}
