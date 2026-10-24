//! Rule-based grammar checks (was `proof.grammar_rules`): careful pattern rules that need no model. The
//! engine adds the context-model checks (agreement, confused words) on top.

use crate::inflect;
use crate::py;
use crate::speller::{overlaps, Speller, FUNCTION_WORDS};
use crate::text::{match_case, sentence_bounds};
use fancy_regex::{Captures, Regex};
use std::sync::LazyLock;

/// A remark on `text[start..end]` (byte offsets into the checked text). `kind` is "gram" here ("spell" for
/// spelling elsewhere); `fixes` may be empty (spelling fixes are looked up lazily).
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
    pub message: String,
    pub fixes: Vec<String>,
}

const MODALS: &str = r"(?:could|should|would|might|must)(?:n['’]t)?";
const NEGAUX: &str = r"(?:didn't|doesn't|don't|couldn't|wouldn't|shouldn't|can't|cannot|won't|did not|does not|do not|could not|would not|should not|will not)";
const DOUBLE_OK: [&str; 24] = [
    "that", "had", "is", "her", "very", "so", "no", "ha", "bye", "yeah", "really", "much", "many", "far", "more", "long",
    "blah", "ho", "knock", "now", "there", "well", "oh", "ah",
];
const COMPARATIVES: &str = "more|less|better|worse|greater|fewer|higher|lower|larger|smaller|faster|slower|rather|easier|harder|longer|shorter|bigger|cheaper|stronger|weaker";
/// A pronoun that opens a clause (so "does it have" and "let it do" are not touched).
const CLAUSE: &str = r#"(?:^|(?<=[.!?;:]\s)|(?<=,\s)|(?<=\band\s)|(?<=\bbut\s)|(?<=\bbecause\s)|(?<=\bso\s)|(?<=\bwhen\s))["“(]?"#;
const AGREE: [(&str, &[(&str, &str)]); 3] = [
    ("he|she|it", &[("are", "is"), ("have", "has"), ("do", "does"), ("don't", "doesn't"), ("aren't", "isn't"), ("haven't", "hasn't")]),
    (
        "we|they|you",
        &[
            ("is", "are"),
            ("was", "were"),
            ("has", "have"),
            ("does", "do"),
            ("doesn't", "don't"),
            ("isn't", "aren't"),
            ("wasn't", "weren't"),
            ("hasn't", "haven't"),
        ],
    ),
    ("I", &[("is", "am"), ("are", "am"), ("has", "have"), ("does", "do"), ("doesn't", "don't")]),
];
const PRONOUN_I: &str = "The pronoun “I” is always capitalised.";
const AAN_SKIP: [&str; 9] = ["historic", "historical", "historian", "hotel", "herb", "herbal", "homage", "hilarious", "habitual"];
/// Letter names that start with a vowel sound: an x-ray, an n-gram.
const AN_LETTERS: &str = "aefhilmnorsx";

enum Fix {
    Text(&'static str),
    Group(usize),
    /// The first character of a group (doubled punctuation).
    FirstOf(usize),
}

/// (pattern, group to underline, replacement, message); compiled case-insensitive and multi-line.
static RULES: LazyLock<Vec<(Regex, usize, Fix, &'static str)>> = LazyLock::new(|| {
    let rules: Vec<(String, usize, Fix, &str)> = vec![
        (format!(r"\b({MODALS})\s+(of)\b(?!\s+course)"), 2, Fix::Text("have"), "After “{1}”, use “have”, not “of”."),
        (format!(r"\b(?:{COMPARATIVES})\s+(then)\b"), 1, Fix::Text("than"), "Use “than” for comparisons; “then” is about time."),
        (
            r"\b(its)\s+(?:an?|the|not|been|also)\b(?!\s+(?:priori|posteriori))".into(),
            1,
            Fix::Text("it's"),
            "“it's” (it is / it has) is needed here; “its” shows possession.",
        ),
        (r"\b(it['’]s)\s+own\b".into(), 1, Fix::Text("its"), "Possessive “its” has no apostrophe."),
        (r"\b(their)\s+(?:is|are|was|were)\b".into(), 1, Fix::Text("there"), "Use “there” here; “their” shows possession."),
        (r"\b(there)\s+own\b".into(), 1, Fix::Text("their"), "Use the possessive “their” here."),
        (r"\b(your)\s+welcome\b".into(), 1, Fix::Text("you're"), "“you're” (you are) is needed here."),
        (
            r"\b(?:am|is|are|was|were|be|been)\s+(?:not\s+)?(suppose)\s+to\b".into(),
            1,
            Fix::Text("supposed"),
            "The phrase is “supposed to”.",
        ),
        (
            r"(?-i:(?<![\w.'’-])(i)(?=\s+(?:am|was|have|had|would|think|know|believe|hope|don['’]t)\b))".into(),
            1,
            Fix::Text("I"),
            PRONOUN_I,
        ),
        (r"(?-i:(?<![\w.'’-])(i)(?=['’](?:m|ve|ll|d)\b))".into(), 1, Fix::Text("I"), PRONOUN_I),
        (
            r"\b(?:more|most)\s+(better|worse|easier|harder|faster|simpler|best|worst|easiest)\b".into(),
            0,
            Fix::Group(1),
            "“{1}” is already a comparative; drop the word before it.",
        ),
        (r"(?<=\w)( +([,;!?]))(?=\s|$)".into(), 1, Fix::Group(2), "No space before this punctuation mark."),
        (r"(?<=[A-Za-z])( +(\.))(?=\s|$)".into(), 1, Fix::Group(2), "No space before the full stop."),
        (r"(?-i:(?<=[a-z])(,)(?=[A-Za-z]{2}))".into(), 1, Fix::Text(", "), "Add a space after the comma."),
        (r"(?<=\w)(,,|;;)".into(), 1, Fix::FirstOf(1), "Doubled punctuation."),
    ];
    rules.into_iter().map(|(rx, g, fix, msg)| (Regex::new(&format!("(?im){rx}")).unwrap(), g, fix, msg)).collect()
});

static DOUBLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?<![\w'’-])([A-Za-z]{2,})[ \t]+\1(?![\w'’-])").unwrap());
static ARTICLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(an?)[ \t]+([A-Za-z][A-Za-z-]*)").unwrap());
static NEG_VERB: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)\b({NEGAUX})\s+([a-z]+)\b")).unwrap());
static HAVE_VERB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:have|has|had|having)\s+([a-z]+)\b").unwrap());
/// "Appendix A is ...": a letter, not an article.
static LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:\b[A-Z][a-z]+|[(\[])\s*$").unwrap());
static SILENT_H: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:hour|honest|honou?r|heir)").unwrap());
static YOO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:uni(?![nm]|de)|unanim|use|usu|usa|usi|uti|uto|ute|ura|ure|uri|uro|eu|ew|ubi|uk|ufo|uv|ones?(?![a-z])|oneself|once)",
    )
    .unwrap()
});
static AGREE_RX: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    AGREE
        .iter()
        .map(|(subj, table)| {
            let verbs: Vec<String> = table.iter().map(|(v, _)| regex::escape(v)).collect();
            let flags = if *subj == "I" { "(?m)" } else { "(?mi)" };
            Regex::new(&format!(r"{flags}{CLAUSE}({subj})\s+({})\b(?!['’])", verbs.join("|"))).unwrap()
        })
        .collect()
});
/// The previous sentence ends in a lower-case word ("... the end. the next one ..."); `\n?` stands for
/// Python's `$`, which also matches before a final line break.
static PREV_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z]{4,})\.[ \t]+\n?$").unwrap());
static WORD_AT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[A-Za-z][A-Za-z']*(?:-[A-Za-z]+)*").unwrap());

fn is_match(re: &Regex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

fn article_for(word: &str) -> &'static str {
    let first = word.chars().next().map(|c| c.to_ascii_lowercase()).unwrap_or(' ');
    let mut it = word.chars();
    if it.next().is_some_and(|c| c.is_ascii_alphabetic()) && it.next() == Some('-') {
        return if AN_LETTERS.contains(first) { "an" } else { "a" };
    }
    if is_match(&SILENT_H, word) || ("aeiou".contains(first) && !is_match(&YOO, word)) {
        return "an";
    }
    "a"
}

fn group<'t>(m: &Captures<'t, str>, i: usize) -> (usize, usize, &'t str) {
    let g = m.get(i).expect("group takes part");
    (g.start(), g.end(), g.as_str())
}

/// High-precision pattern rules on `text` (`protected`: sorted, merged byte spans to leave alone). With a
/// speller, also wrong verb forms after "didn't" / "have" and lower-case sentence starts.
pub fn grammar_rules(text: &str, protected: &[(usize, usize)], speller: Option<&Speller>) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut add = |s: usize, e: usize, message: String, fix: String| {
        if e > s && fix != text[s..e] && !overlaps(protected, s, e) {
            out.push(Issue { start: s, end: e, kind: "gram", message, fixes: vec![fix] });
        }
    };

    for (rx, grp, rep, msg) in RULES.iter() {
        for m in rx.captures_iter(text).flatten() {
            let (s, e, old) = group(&m, *grp);
            let mut new = match rep {
                Fix::Text(t) => t.to_string(),
                Fix::Group(i) => group(&m, *i).2.to_string(),
                Fix::FirstOf(i) => group(&m, *i).2.chars().next().map(String::from).unwrap_or_default(),
            };
            if py::starts_upper(old) && py::starts_lower(&new) {
                new = match_case(old, &new);
            }
            add(s, e, msg.replace("{1}", group(&m, 1).2), new);
        }
    }

    for m in DOUBLED.captures_iter(text).flatten() {
        let (_, _, word) = group(&m, 1);
        if !DOUBLE_OK.contains(&word.to_lowercase().as_str()) {
            let whole = m.get(0).unwrap();
            add(whole.start(), whole.end(), format!("“{word}” is repeated."), word.to_string());
        }
    }

    for m in ARTICLE.captures_iter(text).flatten() {
        let (a_s, a_e, art) = group(&m, 1);
        let (n_s, n_e, nxt) = group(&m, 2);
        let low = nxt.to_lowercase();
        let mut rest = nxt.chars().skip(1);
        if nxt.chars().count() < 2
            || rest.any(char::is_uppercase)
            || !low.contains(['a', 'e', 'i', 'o', 'u', 'y'])
            || AAN_SKIP.contains(&low.as_str())
            || FUNCTION_WORDS.contains(low.as_str())
            || (py::is_upper(art) && art.chars().count() == 2)
        {
            continue; // acronyms, "values a and b", html
        }
        let start = m.get(0).unwrap().start();
        if art == "A" && LABEL.find_from_pos(&text[..start], py::back(text, start, 20)).ok().flatten().is_some() {
            continue;
        }
        let want = article_for(nxt);
        if art.to_lowercase() != want && !overlaps(protected, n_s, n_e) {
            add(a_s, a_e, format!("Use “{want}” before “{nxt}”."), match_case(art, want));
        }
    }

    for ((_, table), rx) in AGREE.iter().zip(AGREE_RX.iter()) {
        for m in rx.captures_iter(text).flatten() {
            let (_, _, subj) = group(&m, 1);
            let (s, e, verb) = group(&m, 2);
            let lower = verb.to_lowercase();
            let Some(&(_, fix)) = table.iter().find(|(v, _)| *v == lower) else { continue };
            add(s, e, format!("“{subj}” takes “{fix}”, not “{verb}”."), match_case(verb, fix));
        }
    }

    if let Some(sp) = speller {
        let t = sp.t;
        for m in NEG_VERB.captures_iter(text).flatten() {
            // didn't went -> didn't go
            let (s, e, word) = group(&m, 2);
            let w = word.to_lowercase();
            let tags = t.tags(&w);
            let past_only = tags.iter().all(|t| matches!(*t, "VBD" | "VBN" | "VBZ" | "NNS"));
            if !tags.is_empty() && past_only && tags.iter().any(|t| matches!(*t, "VBD" | "VBN" | "VBZ")) {
                let lemmas = inflect::get_all_lemmas(&w, None);
                let lem = lemmas.get("VERB").and_then(|l| l.first()).cloned().unwrap_or(w);
                add(s, e, format!("Use the base form “{lem}” after “{}”.", group(&m, 1).2), lem);
            }
        }
        for m in HAVE_VERB.captures_iter(text).flatten() {
            // have went -> have gone
            let (s, e, word) = group(&m, 1);
            let w = word.to_lowercase();
            if !matches!(w.as_str(), "was" | "were" | "did") && t.tags(&w) == ["VBD"] {
                let lemmas = inflect::get_all_lemmas(&w, None);
                let Some(lem) = lemmas.get("VERB").and_then(|l| l.first()) else { continue };
                if let Some(part) = inflect::get_inflection(lem, "VBN", true).into_iter().next() {
                    add(s, e, format!("After “have”, use the past participle “{part}”."), part);
                }
            }
        }

        // "... the end. the next one ..."
        for (s, _) in sentence_bounds(text, false) {
            let Some(m) = WORD_AT.find(&text[s..]) else { continue };
            let word = m.as_str();
            if !py::is_lower(word) || word.chars().count() < 2 {
                continue;
            }
            let before = &text[py::back(text, s, 40)..s];
            let prev = PREV_WORD.captures(before).ok().flatten();
            if prev.is_some_and(|p| sp.in_dictionary(group(&p, 1).2)) && sp.in_dictionary(word) {
                add(s, s + word.len(), "A sentence starts with a capital letter.".into(), py::capitalize(word));
            }
        }
    }

    dedupe(out)
}

/// Sorts issues by position and drops any that overlap an earlier one.
pub fn dedupe(mut issues: Vec<Issue>) -> Vec<Issue> {
    issues.sort_by_key(|x| (x.start, x.end));
    let mut out: Vec<Issue> = Vec::new();
    for it in issues {
        if out.last().is_none_or(|last| it.start >= last.end) {
            out.push(it);
        }
    }
    out
}

/// Distinct issue messages, for tests.
#[cfg(test)]
fn messages(issues: &[Issue]) -> std::collections::HashSet<&str> {
    issues.iter().map(|i| i.message.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_without_speller() {
        let issues = grammar_rules("This the the fine. It it works. A apple, an banana.", &[], None);
        let m = messages(&issues);
        assert!(m.contains("“the” is repeated."), "{m:?}");
        assert!(m.contains("“It” is repeated."), "{m:?}");
        assert!(m.contains("Use “an” before “apple”."), "{m:?}");
        assert!(m.contains("Use “a” before “banana”."), "{m:?}");
    }
}
