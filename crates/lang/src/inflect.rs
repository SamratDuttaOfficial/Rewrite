//! Lemmas and inflections of English words: a port of lemminflect 0.2.3 (MIT, see
//! `assets/lemminflect/LICENSE`), as Rewrite uses it.
//!
//! Known words come from lemminflect's lookup tables and override files; other words go through its two small
//! networks (exported once by `tools/export_lemminflect_models.py`): one picks a suffix rule for the lemma, the
//! other the inflection style (regular, doubled final consonant, Greco-Latin). Parts of speech are universal
//! tags ("NOUN", "VERB", "ADJ", "ADV", "AUX", "PROPN"); forms are keyed by Penn tags ("NNS", "VBD", ...).

use crate::py;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;

const DICT_UPOS_TYPES: [&str; 6] = ["NOUN", "PROPN", "VERB", "ADJ", "ADV", "AUX"];

/// Forms by tag in lemminflect's order (a Python dict of tuples).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forms(pub Vec<(&'static str, Vec<String>)>);

impl Forms {
    pub fn get(&self, tag: &str) -> Option<&[String]> {
        self.0.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &[String])> {
        self.0.iter().map(|(t, v)| (*t, v.as_slice()))
    }

    /// Every spelling, in order.
    pub fn values(&self) -> impl Iterator<Item = &String> {
        self.0.iter().flat_map(|(_, v)| v)
    }
}

/// Raw table entry: spellings by tag, in file order.
type Entry = Vec<(&'static str, Vec<&'static str>)>;

/// `dict.update` on an entry: replaces in place or appends.
fn update(entry: &mut Entry, key: &'static str, value: Vec<&'static str>) {
    match entry.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => entry.push((key, value)),
    }
}

/// A tag or part-of-speech name as a `&'static str`.
fn intern(s: &str) -> &'static str {
    const NAMES: [&str; 20] = [
        "NOUN", "PROPN", "VERB", "ADJ", "ADV", "AUX", "NN", "NNS", "NNP", "NNPS", "VB", "VBD", "VBG", "VBN", "VBP",
        "VBZ", "JJ", "JJR", "JJS", "RB",
    ];
    const MORE: [&str; 3] = ["RBR", "RBS", "MD"];
    NAMES.iter().chain(&MORE).find(|n| **n == s).copied().unwrap_or_else(|| Box::leak(s.to_string().into_boxed_str()))
}

fn gunzip(bytes: &[u8]) -> &'static str {
    let mut text = String::new();
    GzDecoder::new(bytes).read_to_string(&mut text).expect("embedded lemminflect table");
    Box::leak(text.into_boxed_str())
}

/// `OverridesCodec.load`: `word,tag,form` lines.
fn overrides(text: &'static str) -> HashMap<&'static str, Entry> {
    let mut d: HashMap<&str, Entry> = HashMap::new();
    for line in text.lines().map(py::strip) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split(',');
        let (Some(word), Some(pos), Some(out)) = (parts.next(), parts.next(), parts.next()) else { continue };
        update(d.entry(word).or_default(), intern(pos), vec![out]);
    }
    d
}

struct Lemmas {
    dict: HashMap<&'static str, Entry>,
    keys: Vec<&'static str>,
    overrides: HashMap<&'static str, Entry>,
}

static LEMMAS: LazyLock<Lemmas> = LazyLock::new(|| {
    let mut dict: HashMap<&str, Entry> = HashMap::new();
    let mut keys = Vec::new();
    for line in gunzip(include_bytes!("../../../assets/lemminflect/lemma_lu.csv.gz")).lines() {
        let mut parts = py::strip(line).split(',');
        let (Some(word), Some(category), Some(forms)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let upos = if category == "modal" { "AUX" } else { intern(&category.to_uppercase()) };
        let entry = dict.entry(word).or_insert_with(|| {
            keys.push(word);
            Vec::new()
        });
        update(entry, upos, forms.split('/').collect());
    }
    let overrides = overrides(include_str!("../../../assets/lemminflect/lemma_overrides.csv"));
    Lemmas { dict, keys, overrides }
});

struct Inflections {
    dict: HashMap<&'static str, Entry>,
    overrides: HashMap<&'static str, Entry>,
}

static INFLECTIONS: LazyLock<Inflections> = LazyLock::new(|| {
    let mut dict: HashMap<&str, Entry> = HashMap::new();
    for line in gunzip(include_bytes!("../../../assets/lemminflect/infl_lu.csv.gz")).lines() {
        let parts: Vec<&str> = py::strip(line).split(',').collect();
        if parts.len() < 2 {
            continue;
        }
        let (word, category, forms) = (parts[0], parts[1], &parts[2..]);
        let (tags, base): (&[&str], &[&str]) = match category {
            "noun" => (&["NNS"], &["NN"]),
            "adj" => (&["JJR", "JJS"], &["JJ"]),
            "adv" => (&["RBR", "RBS"], &["RB"]),
            "verb" => (&["VBD", "VBN", "VBG", "VBZ"], &["VB", "VBP"]),
            _ => continue, // lemminflect reads no other categories from this table
        };
        let mut entry: Entry = Vec::new();
        for (tag, spellings) in tags.iter().zip(forms) {
            if !spellings.is_empty() {
                entry.push((tag, spellings.split('/').collect()));
            }
        }
        for tag in base {
            entry.push((tag, vec![word]));
        }
        let slot = dict.entry(word).or_default();
        for (tag, v) in entry {
            update(slot, tag, v);
        }
    }
    // Auxiliaries and modals are hard-coded, as they do not follow the rules.
    let fixed: [(&str, &[(&str, &[&str])]); 8] = [
        ("can", &[("VB", &["can"]), ("VBD", &["could"])]),
        ("may", &[("VB", &["may"]), ("VBD", &["might"])]),
        ("will", &[("VB", &["will"]), ("VBD", &["would"])]),
        ("shall", &[("VB", &["shall"]), ("VBD", &["should"])]),
        ("must", &[("VB", &["must"]), ("VBD", &["must"])]),
        ("ought", &[("VB", &["ought"]), ("VBD", &["ought"])]),
        ("dare", &[("VB", &["dare"])]),
        (
            "be",
            &[
                ("VB", &["be"]),
                ("VBD", &["was", "were"]),
                ("VBG", &["being"]),
                ("VBN", &["been"]),
                ("VBP", &["am", "are"]),
                ("VBZ", &["is"]),
            ],
        ),
    ];
    for (word, forms) in fixed {
        dict.insert(word, forms.iter().map(|(t, v)| (*t, v.to_vec())).collect());
    }
    let overrides = overrides(include_str!("../../../assets/lemminflect/infl_overrides.csv"));
    Inflections { dict, overrides }
});

/// One of lemminflect's networks: 9x28 one-hot letters -> dense relu -> dense softmax.
struct Net {
    hidden: usize,
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: Vec<f32>,
    classes: Vec<&'static str>,
}

impl Net {
    fn load(bytes: &'static [u8]) -> Net {
        assert_eq!(&bytes[..4], b"LIM1", "lemminflect network file");
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let (n_in, hidden, n_out) = (u32_at(4), u32_at(8), u32_at(12));
        let mut pos = 16;
        let mut floats = |n: usize| {
            let v: Vec<f32> =
                bytes[pos..pos + 4 * n].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            pos += 4 * n;
            v
        };
        let (w1, b1, w2, b2) = (floats(n_in * hidden), floats(hidden), floats(hidden * n_out), floats(n_out));
        let classes: Vec<&str> = std::str::from_utf8(&bytes[pos..]).expect("UTF-8 classes").split('\n').collect();
        assert_eq!(classes.len(), n_out);
        Net { hidden, w1, b1, w2, b2, classes }
    }

    /// The most likely class for a word of a lexicon category ("noun", "verb", "adj", "adv"); None for other
    /// categories, as lemminflect raises there.
    fn run(&self, word: &str, category: &str) -> Option<&'static str> {
        let cat = ["noun", "verb", "adj", "adv"].iter().position(|c| *c == category)?;
        // Row 0 holds the category, rows 1..9 the last eight letters from the end: a-z, or 1 for anything else.
        let mut active = vec![cat];
        for (i, c) in word.to_lowercase().chars().rev().take(8).enumerate() {
            let one_hot = if c.is_ascii_lowercase() { c as usize - 95 } else { 1 };
            active.push((i + 1) * 28 + one_hot);
        }
        active.sort_unstable();
        let h: Vec<f32> = (0..self.hidden)
            .map(|j| {
                let x = active.iter().fold(0.0f32, |s, &i| s + self.w1[i * self.hidden + j]) + self.b1[j];
                if x > 0.0 { x } else { 0.0 }
            })
            .collect();
        let n_out = self.b2.len();
        let y: Vec<f32> = (0..n_out)
            .map(|k| h.iter().enumerate().fold(0.0f32, |s, (j, x)| s + x * self.w2[j * n_out + k]) + self.b2[k])
            .collect();
        // softmax as numpy computes it, then the first maximum
        let max = y.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let e: Vec<f32> = y.iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = e.iter().sum();
        let mut best = 0;
        for (k, v) in e.iter().enumerate() {
            if v / sum > e[best] / sum {
                best = k;
            }
        }
        Some(self.classes[best])
    }
}

static LEMMA_NET: LazyLock<Net> = LazyLock::new(|| Net::load(include_bytes!("../../../assets/lemminflect/model_lemma.bin")));
static INFL_NET: LazyLock<Net> = LazyLock::new(|| Net::load(include_bytes!("../../../assets/lemminflect/model_infl.bin")));

#[derive(Clone, Copy)]
enum Caps {
    Upper,
    First,
    Lower,
}

fn caps_style(word: &str) -> Caps {
    if py::is_upper(word) {
        Caps::Upper
    } else if py::starts_upper(word) {
        Caps::First
    } else {
        Caps::Lower
    }
}

fn apply_caps(word: &str, style: Caps) -> String {
    match style {
        Caps::Upper => word.to_uppercase(),
        Caps::First => py::capitalize(word),
        Caps::Lower => word.to_lowercase(),
    }
}

fn to_forms(entry: Entry, style: Caps) -> Forms {
    Forms(entry.into_iter().map(|(t, v)| (t, v.into_iter().map(|w| apply_caps(w, style)).collect())).collect())
}

/// `uposToCategory`: the lexicon category of a universal tag.
fn category(upos: &str) -> String {
    if upos == "PROPN" { "noun".to_string() } else { upos.to_lowercase() }
}

/// Every lemma of a known word, by part of speech, with the word's capitalisation. With `upos`, only that part
/// of speech ("PROPN" looks up the capitalised word as a noun); an unknown `upos` gives nothing.
pub fn get_all_lemmas(word: &str, upos: Option<&str>) -> Forms {
    let mut upos = upos;
    if upos.is_some_and(|u| !DICT_UPOS_TYPES.contains(&u)) {
        return Forms::default();
    }
    let style = caps_style(word);
    let mut word = word.to_lowercase();
    if upos == Some("PROPN") {
        word = py::capitalize(&word);
        upos = Some("NOUN");
    }
    let data = &*LEMMAS;
    let mut lemmas = data.dict.get(word.as_str()).cloned().unwrap_or_default();
    for (k, v) in data.overrides.get(word.as_str()).cloned().unwrap_or_default() {
        update(&mut lemmas, k, v);
    }
    if let Some(u) = upos {
        lemmas.retain(|(k, _)| *k == u);
    }
    to_forms(lemmas, style)
}

/// The rule-based lemma of any word (the lemma network), as `{upos: [lemma]}`; empty for "AUX" and unknown tags.
pub fn get_all_lemmas_oov(word: &str, upos: &str) -> Forms {
    if !DICT_UPOS_TYPES.contains(&upos) {
        return Forms::default();
    }
    let style = caps_style(word);
    let Some(rule) = LEMMA_NET.run(word, &category(upos)) else { return Forms::default() };
    let mut parts = rule.split(',');
    let (remove, add, double) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next() == Some("True"));
    // The rule is applied even when the word does not end in `remove` (as lemminflect does).
    let mut lemma = py::drop_last(word, remove.chars().count()).to_string() + add;
    if double {
        lemma = py::drop_last(&lemma, 1).to_string();
    }
    Forms(vec![(intern(upos), vec![apply_caps(&lemma, style)])])
}

/// Every inflection of a known lemma, by Penn tag, with the lemma's capitalisation. With `upos`, only the tags
/// of that part of speech ("PROPN" looks up the capitalised lemma as a noun); an unknown `upos` gives nothing.
pub fn get_all_inflections(lemma: &str, upos: Option<&str>) -> Forms {
    let mut upos = upos;
    if upos.is_some_and(|u| !DICT_UPOS_TYPES.contains(&u)) {
        return Forms::default();
    }
    let style = caps_style(lemma);
    let mut lemma = lemma.to_lowercase();
    if upos == Some("PROPN") {
        lemma = py::capitalize(&lemma);
        upos = Some("NOUN");
    }
    let data = &*INFLECTIONS;
    let mut forms = data.dict.get(lemma.as_str()).cloned().unwrap_or_default();
    for (k, v) in data.overrides.get(lemma.as_str()).cloned().unwrap_or_default() {
        update(&mut forms, k, v);
    }
    if let Some(u) = upos {
        let tags = upos_to_tags(u);
        forms.retain(|(k, _)| tags.contains(k));
    }
    to_forms(forms, style)
}

/// Rule-based inflections of any lemma (the style network picks regular, doubled or Greco-Latin forms).
pub fn get_all_inflections_oov(lemma: &str, upos: &str) -> Forms {
    if !DICT_UPOS_TYPES.contains(&upos) {
        return Forms::default();
    }
    let style = caps_style(lemma);
    let regular = INFL_NET.run(lemma, &category(upos)) == Some("reg");
    let own = || vec![lemma.to_string()];
    let entry: Vec<(&'static str, Vec<String>)> = match upos {
        "VERB" => {
            let [third, past, pres] = if regular { build_reg_verb(lemma) } else { build_doubled_verb(lemma) };
            vec![("VB", own()), ("VBZ", vec![third]), ("VBN", vec![past.clone()]), ("VBD", vec![past]), ("VBG", vec![pres])]
        }
        "ADJ" | "ADV" => {
            let [more, most] = if regular { build_reg_adj_adv(lemma) } else { build_doubled_adj_adv(lemma) };
            let tags = if upos == "ADJ" { ["JJ", "JJR", "JJS"] } else { ["RB", "RBR", "RBS"] };
            vec![(tags[0], own()), (tags[1], vec![more]), (tags[2], vec![most])]
        }
        "NOUN" | "PROPN" => {
            let plural = if regular { build_reg_noun(lemma) } else { build_grec_noun(lemma) };
            vec![("NN", own()), ("NNS", vec![plural])]
        }
        _ => Vec::new(),
    };
    Forms(entry.into_iter().map(|(t, v)| (t, v.iter().map(|w| apply_caps(w, style)).collect())).collect())
}

/// The spellings of `lemma` for one Penn tag: from the tables, then under related tags (VBD for VBN, JJ for RB,
/// ...), then by rule when `inflect_oov`. Empty when there is none.
pub fn get_inflection(lemma: &str, tag: &str, inflect_oov: bool) -> Vec<String> {
    let upos = tag_to_upos(tag);
    let forms = get_all_inflections(lemma, if upos == Some("PROPN") { upos } else { None });
    let mut form = extract_form(&forms, tag);
    if form.is_none() {
        form = penn_tag_alts(tag).iter().find_map(|alt| forms.get(alt));
    }
    if let Some(f) = form {
        return f.to_vec();
    }
    if inflect_oov {
        if let Some(upos) = upos {
            let forms = get_all_inflections_oov(lemma, upos);
            if let Some(f) = extract_form(&forms, tag) {
                return f.to_vec();
            }
        }
    }
    Vec::new()
}

/// Every word in the lemma table, in file order (lemminflect's lexicon, used by the speller).
pub fn lemma_dict_keys() -> &'static [&'static str] {
    &LEMMAS.keys
}

/// Whether [`get_all_lemmas`] (without a part of speech) finds anything for `word`, without building the result.
pub fn has_lemmas(word: &str) -> bool {
    let w = word.to_lowercase();
    LEMMAS.dict.contains_key(w.as_str()) || LEMMAS.overrides.contains_key(w.as_str())
}

fn extract_form<'f>(forms: &'f Forms, tag: &str) -> Option<&'f [String]> {
    forms.get(match tag {
        "NNP" => "NN",
        "NNPS" => "NNS",
        t => t,
    })
}

/// `tagToUPos`: the universal tag for a Penn tag (MD counts as a verb).
pub fn tag_to_upos(tag: &str) -> Option<&'static str> {
    let tag = tag.to_uppercase();
    match tag.as_str() {
        t if t.starts_with('J') => Some("ADJ"),
        t if t.starts_with('R') && t != "RP" => Some("ADV"),
        "NNP" | "NNPS" => Some("PROPN"),
        t if t.starts_with('N') => Some("NOUN"),
        t if t.starts_with('V') => Some("VERB"),
        "MD" => Some("VERB"),
        _ => None,
    }
}

fn upos_to_tags(upos: &str) -> &'static [&'static str] {
    match upos.to_uppercase().as_str() {
        "VERB" | "AUX" => &["VB", "VBD", "VBG", "VBN", "VBP", "VBZ", "MD"],
        "ADJ" => &["JJ", "JJR", "JJS"],
        "ADV" => &["RB", "RBR", "RBS"],
        "NOUN" => &["NN", "NNS", "NNP", "NNPS"],
        _ => &[],
    }
}

/// Other tags to try when a tag gives nothing.
fn penn_tag_alts(tag: &str) -> Vec<String> {
    match tag {
        "RP" => vec!["JJ".into(), "RB".into()],
        "VBD" => vec!["VBN".into()],
        "VBN" => vec!["VBD".into()],
        t if t.starts_with("JJ") => vec![format!("RB{}", &t[2..])],
        t if t.starts_with("RB") => vec![format!("JJ{}", &t[2..])],
        _ => Vec::new(),
    }
}

fn last_char(s: &str) -> &str {
    py::last_chars(s, 1)
}

/// `[third person, past, present participle]` of a regular verb.
fn build_reg_verb(base: &str) -> [String; 3] {
    let base = base.to_lowercase();
    let ends = |s: &str| base.ends_with(s);
    let before_last = || py::last_chars(&base, 2).chars().next().filter(|_| base.chars().count() >= 2);
    if ["s", "z", "x", "ch", "sh"].iter().any(|s| ends(s)) {
        [format!("{base}es"), format!("{base}ed"), format!("{base}ing")]
    } else if ends("ie") {
        [format!("{base}s"), format!("{base}d"), format!("{}ying", py::drop_last(&base, 2))]
    } else if ends("ee") || ends("oe") || ends("ye") {
        [format!("{base}s"), format!("{base}d"), format!("{base}ing")]
    } else if ends("y") && before_last().is_some_and(|c| !"aeiou".contains(c)) {
        let b = py::drop_last(&base, 1);
        [format!("{b}ies"), format!("{b}ied"), format!("{b}ying")]
    } else if ends("e") && before_last().is_some_and(|c| !"iyeo".contains(c)) {
        let b = py::drop_last(&base, 1);
        [format!("{b}es"), format!("{b}ed"), format!("{b}ing")]
    } else {
        [format!("{base}s"), format!("{base}ed"), format!("{base}ing")]
    }
}

fn build_doubled_verb(base: &str) -> [String; 3] {
    let base = base.to_lowercase();
    let [third, _, _] = build_reg_verb(&base);
    let last = last_char(&base);
    [third, format!("{base}{last}ed"), format!("{base}{last}ing")]
}

/// `[comparative, superlative]` of a regular adjective or adverb.
fn build_reg_adj_adv(base: &str) -> [String; 2] {
    let base = base.to_lowercase();
    let before_last = py::last_chars(&base, 2).chars().next().filter(|_| base.chars().count() >= 2);
    let vowel = |c: char| "aeiou".contains(c);
    if base.ends_with('y') && before_last.is_some_and(|c| !vowel(c)) {
        let b = py::drop_last(&base, 1);
        [format!("{b}ier"), format!("{b}iest")]
    } else if base.ends_with('y') && before_last.is_some_and(vowel) {
        [format!("{base}er"), format!("{base}est")]
    } else if base.ends_with('e') && before_last.is_some_and(|c| !vowel(c)) {
        [format!("{base}r"), format!("{base}st")]
    } else {
        [format!("{base}er"), format!("{base}est")]
    }
}

fn build_doubled_adj_adv(base: &str) -> [String; 2] {
    let base = base.to_lowercase();
    let b = format!("{base}{}", last_char(&base));
    [format!("{b}er"), format!("{b}est")]
}

fn build_reg_noun(base: &str) -> String {
    let base = base.to_lowercase();
    let before_last = py::last_chars(&base, 2).chars().next().filter(|_| base.chars().count() >= 2);
    if base.ends_with('y') && before_last.is_some_and(|c| !"aeiou".contains(c)) {
        format!("{}ies", py::drop_last(&base, 1))
    } else if ["s", "z", "x", "ch", "sh"].iter().any(|s| base.ends_with(s)) {
        format!("{base}es")
    } else {
        format!("{base}s")
    }
}

fn build_grec_noun(base: &str) -> String {
    let base = base.to_lowercase();
    let rules: [(&str, usize, &str); 10] = [
        ("us", 2, "i"),
        ("ma", 2, "mata"),
        ("a", 1, "ae"),
        ("um", 2, "a"),
        ("on", 2, "a"),
        ("sis", 3, "ses"),
        ("is", 2, "ides"),
        ("men", 3, "mina"),
        ("ex", 2, "ices"),
        ("x", 1, "ces"),
    ];
    match rules.iter().find(|(end, _, _)| base.ends_with(end)) {
        Some((_, cut, add)) => format!("{}{add}", py::drop_last(&base, *cut)),
        None => build_reg_noun(&base),
    }
}
