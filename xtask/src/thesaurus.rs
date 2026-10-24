//! `cargo xtask thesaurus`: build the thesaurus data file (assets/thesaurus.json.gz) from WordNet 3.0
//! and wordfreq's English word frequencies.
//!
//!     cargo xtask thesaurus --wordnet <dict dir> --wordfreq <large_en.msgpack.gz> [--out <file>]
//!
//! - WordNet 3.0: the `dict` folder of https://wordnet.princeton.edu/download (WordNet-3.0.tar.gz), or
//!   nltk's `corpora/wordnet` download, which holds the same files (data.*, index.*, *.exc). WordNet licence
//!   (permissive, BSD-like; keep Princeton's copyright notice).
//! - wordfreq: `wordfreq/data/large_en.msgpack.gz` from the wordfreq package (PyPI or
//!   github.com/rspeer/wordfreq). Its data is CC BY-SA 4.0.
//!
//! The output matches what tools/build_thesaurus.py wrote with nltk's WordNet reader and wordfreq 3.1:
//! synsets in nltk's `all_synsets()` order, each word's senses in `wn.synsets()` order (morphy included),
//! and frequencies from `zipf_frequency(word, "en")`. Two orders that the Python script left to Python's
//! randomised string hashing are fixed here: related synsets follow the data file (nltk kept them in a
//! set, so the script's lists, and which six it kept, changed from run to run) and "F" follows "I".

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

const LICENSE: &str =
    "WordNet 3.0 (c) Princeton University, WordNet license; frequencies from wordfreq (CC BY-SA 4.0)";

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut wordnet, mut wordfreq, mut out) = (None, None, crate::root().join("assets/thesaurus.json.gz"));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().map(PathBuf::from).ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--wordnet" => wordnet = Some(value()?),
            "--wordfreq" => wordfreq = Some(value()?),
            "--out" => out = value()?,
            _ => return Err(format!("unknown argument {a}\n{USAGE}")),
        }
    }
    let (Some(wordnet), Some(wordfreq)) = (wordnet, wordfreq) else { return Err(USAGE.into()) };
    let wn = WordNet::load(&wordnet)?;
    let freqs = read_cbpack(&wordfreq)?;
    let json = build(&wn, &freqs);
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    let bytes = gz.finish().map_err(|e| e.to_string())?;
    fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    crate::say(&format!("{} ({:.2} MB)", out.display(), bytes.len() as f64 / 1e6));
    Ok(())
}

const USAGE: &str = "usage: cargo xtask thesaurus --wordnet <WordNet 3.0 dict dir> --wordfreq <large_en.msgpack.gz> [--out <file>]";

// -- the thesaurus ------------------------------------------------------------------------------

/// A thesaurus word: lower-case letters, inner spaces, hyphens and apostrophes, at most three words.
fn clean(lemma: &str) -> Option<String> {
    let w = lemma.replace('_', " ");
    let b = w.as_bytes();
    let good = b.len() >= 2
        && b[0].is_ascii_lowercase()
        && b[b.len() - 1].is_ascii_lowercase()
        && b.iter().all(|&c| c.is_ascii_lowercase() || b"' -".contains(&c));
    (good && w.split_whitespace().count() <= 3).then_some(w)
}

/// The first clause of the definition, cut at a word to at most 90 characters.
fn short_gloss(definition: &str) -> String {
    let g = definition.split(';').next().unwrap_or("").trim();
    if g.chars().count() <= 90 {
        return g.to_string();
    }
    let head: String = g.chars().take(87).collect();
    let head = head.rsplit_once(' ').map_or(head.as_str(), |(h, _)| h);
    format!("{head}...")
}

/// An insertion-ordered map from words, like the Python dicts the file was first written from.
#[derive(Default)]
struct WordMap<T> {
    keys: HashMap<String, usize>,
    entries: Vec<(String, T)>,
}

impl<T: Default> WordMap<T> {
    fn entry(&mut self, w: &str) -> &mut T {
        let n = self.entries.len();
        let i = *self.keys.entry(w.to_string()).or_insert(n);
        if i == n {
            self.entries.push((w.to_string(), T::default()));
        }
        &mut self.entries[i].1
    }
}

/// The thesaurus as compact JSON: {"v", "S": [[pos, lemmas, gloss, related]], "I": word -> synsets,
/// "A": word -> antonyms, "F": word -> Zipf frequency x 10, "license"}.
fn build(wn: &WordNet, freqs: &HashMap<String, f64>) -> String {
    // synsets with at least one usable lemma, in nltk's all_synsets() order
    let mut row_of: HashMap<Id, usize> = HashMap::new();
    let mut rows: Vec<(&Synset, Vec<String>, String)> = vec![];
    for s in &wn.synsets {
        let mut lemmas: Vec<String> = vec![];
        for w in s.lemmas.iter().filter_map(|l| clean(l)) {
            if !lemmas.contains(&w) {
                lemmas.push(w);
            }
        }
        if !lemmas.is_empty() {
            row_of.insert(s.id, rows.len());
            rows.push((s, lemmas, short_gloss(&s.definition)));
        }
    }

    let mut index: WordMap<Vec<usize>> = WordMap::default();
    let mut ants: WordMap<Vec<String>> = WordMap::default();
    let mut related = vec![];
    for (i, (s, ..)) in rows.iter().enumerate() {
        let rel = match s.pos {
            'a' | 's' => &s.similar[..],
            'n' | 'v' => &s.hypernyms[..],
            _ => &[],
        };
        related.push(rel.iter().filter_map(|id| row_of.get(id).copied()).take(6).collect::<Vec<_>>());
        for name in &s.lemmas {
            let Some(w) = clean(name) else { continue };
            let ids = index.entry(&w);
            if !ids.contains(&i) {
                ids.push(i);
            }
            for &(target, k) in s.antonyms.get(name).into_iter().flatten() {
                if let Some(a) = wn.synset(target).and_then(|t| t.lemmas.get(k)).and_then(|n| clean(n)) {
                    let list = ants.entry(&w);
                    if !list.contains(&a) {
                        list.push(a);
                    }
                }
            }
        }
    }
    // each word's senses in WordNet's own ranking (most frequent first), as wn.synsets() lists them
    for (w, ids) in &mut index.entries {
        let order = wn.synsets_of(&w.replace(' ', "_"));
        ids.sort_by_key(|&i| order.get(&rows[i].0.id).copied().unwrap_or(999));
    }

    let mut j = String::with_capacity(16 << 20);
    j.push_str("{\"v\":1,\"S\":[");
    for (i, ((s, lemmas, gloss), rel)) in rows.iter().zip(&related).enumerate() {
        if i > 0 {
            j.push(',');
        }
        let pos = if s.pos == 's' { 'a' } else { s.pos };
        let _ = write!(j, "[\"{pos}\",");
        json_list(&mut j, lemmas, |j, w| json_str(j, w));
        j.push(',');
        json_str(&mut j, gloss);
        j.push(',');
        json_list(&mut j, rel, |j, r| { let _ = write!(j, "{r}"); });
        j.push(']');
    }
    j.push_str("],\"I\":");
    json_map(&mut j, &index.entries, |j, ids| json_list(j, ids, |j, r| { let _ = write!(j, "{r}"); }));
    j.push_str(",\"A\":");
    json_map(&mut j, &ants.entries, |j, list| json_list(j, list, |j, w| json_str(j, w)));
    j.push_str(",\"F\":");
    let freq: Vec<(String, i64)> = index
        .entries
        .iter()
        .filter_map(|(w, _)| zipf_frequency(freqs, w).filter(|&z| z >= 1.0).map(|z| (w.clone(), (z * 10.0).round_ties_even() as i64)))
        .collect();
    json_map(&mut j, &freq, |j, f| { let _ = write!(j, "{f}"); });
    j.push_str(",\"license\":");
    json_str(&mut j, LICENSE);
    j.push('}');
    j
}

fn json_list<T>(j: &mut String, items: &[T], mut item: impl FnMut(&mut String, &T)) {
    j.push('[');
    for (k, x) in items.iter().enumerate() {
        if k > 0 {
            j.push(',');
        }
        item(j, x);
    }
    j.push(']');
}

fn json_map<T>(j: &mut String, entries: &[(String, T)], mut value: impl FnMut(&mut String, &T)) {
    j.push('{');
    for (k, (key, v)) in entries.iter().enumerate() {
        if k > 0 {
            j.push(',');
        }
        json_str(j, key);
        j.push(':');
        value(j, v);
    }
    j.push('}');
}

/// A JSON string as Python's json.dumps writes it (ASCII only, everything else escaped).
fn json_str(j: &mut String, s: &str) {
    j.push('"');
    for c in s.chars() {
        match c {
            '"' => j.push_str("\\\""),
            '\\' => j.push_str("\\\\"),
            '\n' => j.push_str("\\n"),
            '\r' => j.push_str("\\r"),
            '\t' => j.push_str("\\t"),
            '\u{8}' => j.push_str("\\b"),
            '\u{c}' => j.push_str("\\f"),
            ' '..='~' => j.push(c),
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(j, "\\u{u:04x}");
                }
            }
        }
    }
    j.push('"');
}

// -- WordNet ------------------------------------------------------------------------------------

/// The data files in the order nltk's all_synsets() reads them.
const FILES: [&str; 4] = ["adj", "adv", "noun", "verb"];
const ADJ: usize = 0;
const ADV: usize = 1;
const NOUN: usize = 2;
const VERB: usize = 3;

/// A synset's identity: its data file and byte offset.
type Id = (usize, u32);

struct Synset {
    id: Id,
    /// n, v, a, s (adjective satellite) or r
    pos: char,
    /// lemma names as written, without adjective markers such as "(a)"
    lemmas: Vec<String>,
    /// the gloss without its quoted examples
    definition: String,
    /// "&" pointers, in file order without repeats
    similar: Vec<Id>,
    /// "@" pointers (not instance hypernyms), in file order without repeats
    hypernyms: Vec<Id>,
    /// "!" lemma pointers by source lemma name (target synset, lemma number), as nltk keys them
    antonyms: HashMap<String, Vec<(Id, usize)>>,
}

struct WordNet {
    synsets: Vec<Synset>,
    by_id: HashMap<Id, usize>,
    /// per data file: lemma -> synset offsets, most frequent sense first
    index: [HashMap<String, Vec<u32>>; 4],
    /// per data file: inflected form -> base forms
    exceptions: [HashMap<String, Vec<String>>; 4],
}

fn file_of(pos: &str) -> Result<usize, String> {
    match pos {
        "a" | "s" => Ok(ADJ),
        "r" => Ok(ADV),
        "n" => Ok(NOUN),
        "v" => Ok(VERB),
        _ => Err(format!("unknown part of speech {pos:?}")),
    }
}

/// nltk morphy's detachment rules (suffix, replacement), tried in this order.
fn substitutions(file: usize) -> &'static [(&'static str, &'static str)] {
    match file {
        NOUN => &[("s", ""), ("ses", "s"), ("ves", "f"), ("xes", "x"), ("zes", "z"), ("ches", "ch"), ("shes", "sh"), ("men", "man"), ("ies", "y")],
        VERB => &[("s", ""), ("ies", "y"), ("es", "e"), ("es", ""), ("ed", "e"), ("ed", ""), ("ing", "e"), ("ing", "")],
        ADJ => &[("er", ""), ("est", ""), ("er", "e"), ("est", "e")],
        _ => &[],
    }
}

impl WordNet {
    fn load(dir: &Path) -> Result<Self, String> {
        let read = |name: String| {
            let path = dir.join(&name);
            fs::read_to_string(&path).map_err(|e| format!("{}: {e} (--wordnet must be WordNet 3.0's dict folder)", path.display()))
        };
        let mut wn = WordNet { synsets: vec![], by_id: HashMap::new(), index: Default::default(), exceptions: Default::default() };
        for (file, name) in FILES.iter().enumerate() {
            let data = read(format!("data.{name}"))?;
            for line in data.lines().filter(|l| !l.starts_with(char::is_whitespace) && !l.is_empty()) {
                let s = parse_synset(file, line).map_err(|e| format!("data.{name}: {e} in {line:?}"))?;
                wn.by_id.insert(s.id, wn.synsets.len());
                wn.synsets.push(s);
            }
            for line in read(format!("index.{name}"))?.lines().filter(|l| !l.starts_with(' ')) {
                let t: Vec<&str> = line.split_whitespace().collect();
                let bad = || format!("index.{name}: bad line {line:?}");
                let n: usize = t.get(2).and_then(|n| n.parse().ok()).ok_or_else(bad)?;
                let p: usize = t.get(3).and_then(|n| n.parse().ok()).ok_or_else(bad)?;
                let offsets = t.get(6 + p..6 + p + n).ok_or_else(bad)?;
                let offsets = offsets.iter().map(|o| o.parse().map_err(|_| bad())).collect::<Result<_, _>>()?;
                wn.index[file].insert(t[0].to_string(), offsets);
            }
            for line in read(format!("{name}.exc"))?.lines() {
                let mut t = line.split_whitespace();
                if let Some(form) = t.next() {
                    wn.exceptions[file].insert(form.to_string(), t.map(String::from).collect());
                }
            }
        }
        Ok(wn)
    }

    fn synset(&self, id: Id) -> Option<&Synset> {
        self.by_id.get(&id).map(|&i| &self.synsets[i])
    }

    /// nltk's _morphy(): the word and its base forms that are in this part of speech's index.
    fn morphy(&self, form: &str, file: usize) -> Vec<&str> {
        let forms: Vec<String> = match self.exceptions[file].get(form) {
            Some(bases) => bases.clone(),
            None => substitutions(file)
                .iter()
                .filter_map(|(old, new)| form.strip_suffix(old).map(|stem| format!("{stem}{new}")))
                .collect(),
        };
        let mut out: Vec<&str> = vec![];
        for f in std::iter::once(form).chain(forms.iter().map(String::as_str)) {
            if let Some((key, _)) = self.index[file].get_key_value(f) {
                if !out.contains(&key.as_str()) {
                    out.push(key);
                }
            }
        }
        out
    }

    /// The position of each synset in wn.synsets(word); for a synset listed twice the later position counts.
    fn synsets_of(&self, word: &str) -> HashMap<Id, usize> {
        let mut order = HashMap::new();
        let mut k = 0;
        for file in [NOUN, VERB, ADJ, ADV] {
            for form in self.morphy(word, file) {
                for &offset in &self.index[file][form] {
                    order.insert((file, offset), k);
                    k += 1;
                }
            }
        }
        order
    }
}

/// One line of a data file, read as nltk's _synset_from_pos_and_line() reads it.
fn parse_synset(file: usize, line: &str) -> Result<Synset, String> {
    let (columns, gloss) = line.trim().split_once('|').ok_or("no gloss")?;
    let mut t = columns.split_whitespace();
    let mut next = || t.next().ok_or("too few fields");
    let offset: u32 = next()?.parse().map_err(|_| "bad offset")?;
    next()?; // lexicographer file
    let pos = next()?.chars().next().ok_or("no part of speech")?;
    let n = usize::from_str_radix(next()?, 16).map_err(|_| "bad word count")?;
    let mut lemmas = vec![];
    for _ in 0..n {
        lemmas.push(strip_marker(next()?).to_string());
        next()?; // lex_id
    }
    let (mut similar, mut hypernyms, mut antonyms) = (vec![], vec![], HashMap::<String, Vec<_>>::new());
    let n: usize = next()?.parse().map_err(|_| "bad pointer count")?;
    for _ in 0..n {
        let (symbol, target, tpos, lemma_ids) = (next()?, next()?, next()?, next()?);
        let id = (file_of(tpos)?, target.parse().map_err(|_| "bad pointer offset")?);
        let set = match symbol {
            "&" => &mut similar,
            "@" => &mut hypernyms,
            _ => {
                if symbol == "!" && lemma_ids != "0000" && lemma_ids.len() == 4 {
                    let num = |s: &str| usize::from_str_radix(s, 16).ok().and_then(|n| n.checked_sub(1)).ok_or("bad lemma number");
                    let source = lemmas.get(num(&lemma_ids[..2])?).ok_or("bad lemma number")?;
                    antonyms.entry(source.clone()).or_default().push((id, num(&lemma_ids[2..])?));
                }
                continue;
            }
        };
        // only synset pointers ("0000"): similar_tos() and hypernyms() ignore lexical ones
        if lemma_ids == "0000" && !set.contains(&id) {
            set.push(id);
        }
    }
    Ok(Synset { id: (file, offset), pos, lemmas, definition: definition(gloss), similar, hypernyms, antonyms })
}

/// "word(a)" -> "word": the syntactic marker some adjectives carry.
fn strip_marker(name: &str) -> &str {
    match (name.strip_suffix(')'), name.rfind('(')) {
        (Some(body), Some(i)) if !body[i + 1..].contains(')') => &name[..i],
        _ => name,
    }
}

/// The gloss with its quoted examples removed, trimmed of spaces and semicolons (nltk's definition()).
fn definition(gloss: &str) -> String {
    let mut out = String::with_capacity(gloss.len());
    let mut rest = gloss;
    while let Some(i) = rest.find('"') {
        match rest[i + 1..].find('"') {
            Some(k) => {
                out.push_str(&rest[..i]);
                rest = &rest[i + k + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out.trim().trim_matches([';', ' ']).to_string()
}

// -- wordfreq -----------------------------------------------------------------------------------

/// Word frequencies from a wordfreq "cBpack" file: gzipped msgpack of a header and then one list of
/// words per centibel step, list n holding the words of frequency 10^(-n/100).
fn read_cbpack(path: &Path) -> Result<HashMap<String, f64>, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let mut raw = vec![];
    GzDecoder::new(fs::File::open(path).map_err(|e| err(&e))?).read_to_end(&mut raw).map_err(|e| err(&e))?;
    let value = rmpv::decode::read_value(&mut raw.as_slice()).map_err(|e| err(&e))?;
    let lists = value.as_array().ok_or_else(|| err(&"not a wordfreq data file"))?;
    let header_ok = lists.first().and_then(|h| h.as_map()).is_some_and(|h| {
        let get = |k: &str| h.iter().find(|(key, _)| key.as_str() == Some(k)).map(|(_, v)| v);
        get("format").and_then(|v| v.as_str()) == Some("cB") && get("version").and_then(|v| v.as_u64()) == Some(1)
    });
    if !header_ok {
        return Err(err(&"not a wordfreq cBpack (version 1) file"));
    }
    let mut freqs = HashMap::new();
    for (n, bucket) in lists[1..].iter().enumerate() {
        let freq = 10f64.powf(-(n as f64) / 100.0);
        for w in bucket.as_array().ok_or_else(|| err(&"bad word list"))? {
            freqs.insert(w.as_str().ok_or_else(|| err(&"bad word"))?.to_string(), freq);
        }
    }
    Ok(freqs)
}

/// wordfreq's English tokens for a thesaurus word (see `clean`: lower-case letters, spaces, hyphens,
/// apostrophes). Spaces and hyphens break words, an apostrophe joins letters (Unicode word boundaries),
/// and a short elided start before a vowel is its own token ("d'etat" -> "d", "etat"; wordfreq's rule
/// for French articles).
fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = vec![];
    for chunk in text.split([' ', '-']) {
        // words: letters, and apostrophes with a letter on each side
        let b = chunk.as_bytes();
        let in_word = |i: usize| b[i] != b'\'' || (i > 0 && i + 1 < b.len() && b[i - 1] != b'\'' && b[i + 1] != b'\'');
        let mut start = None;
        for i in 0..=b.len() {
            match (i < b.len() && in_word(i), start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    split_elision(&chunk[s..i], &mut tokens);
                    start = None;
                }
                _ => {}
            }
        }
    }
    tokens
}

fn split_elision<'a>(mut word: &'a str, tokens: &mut Vec<&'a str>) {
    loop {
        let b = word.as_bytes();
        let apos = b.iter().take(3).position(|&c| c == b'\'');
        match apos {
            Some(k) if k >= 1 && b.get(k + 1).is_some_and(|c| b"aehiouy".contains(c)) => {
                tokens.push(&word[..k]);
                word = &word[k + 1..];
            }
            _ => {
                tokens.push(word.trim_matches('\''));
                return;
            }
        }
    }
}

/// wordfreq's zipf_frequency(word, "en"), or None when a token is not in the list. Token frequencies
/// combine as 1/f = 1/f1 + 1/f2 + ...; the frequency is rounded to three significant digits and the
/// Zipf value (log10 of the frequency per billion words) to two decimals, as Python's round() does.
fn zipf_frequency(freqs: &HashMap<String, f64>, word: &str) -> Option<f64> {
    let tokens = tokenize(word);
    if tokens.is_empty() {
        return None;
    }
    let mut one_over = 0.0;
    for t in tokens {
        one_over += 1.0 / freqs.get(t)?;
    }
    let freq = (1.0 / one_over).max(1e-9);
    let digits = (-(freq.ln() / 10f64.ln())).floor() as i32 + 3;
    let freq = py_round(freq, digits);
    Some(py_round(freq.ln() / 10f64.ln() + 9.0, 2))
}

/// Python's round(x, digits): the correctly rounded decimal, read back.
fn py_round(x: f64, digits: i32) -> f64 {
    format!("{x:.*}", digits.max(0) as usize).parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(tokenize("coup d'etat"), ["coup", "d", "etat"]);
        assert_eq!(tokenize("o'clock"), ["o'clock"]);
        assert_eq!(tokenize("buyers' market"), ["buyers", "market"]);
        assert_eq!(tokenize("wrong 'un"), ["wrong", "un"]);
        assert_eq!(tokenize("nor'-nor'-east"), ["nor", "nor", "east"]);
        assert_eq!(tokenize("t'ai chi"), ["t", "ai", "chi"]);
        assert_eq!(tokenize("shove-ha'penny"), ["shove", "ha'penny"]);
    }

    #[test]
    fn glosses() {
        assert_eq!(definition(r#"a dog; "the dog barked"; "x" "#), "a dog");
        assert_eq!(strip_marker("galore(ip)"), "galore");
        assert_eq!(strip_marker("word"), "word");
    }
}
