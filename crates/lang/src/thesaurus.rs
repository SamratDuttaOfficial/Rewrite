//! Offline thesaurus built from WordNet, with synonyms inflected to match the word (was `thesaurus.py`).
//!
//! The data is `assets/thesaurus.json.gz`, turned into a compact binary by `build.rs` and embedded, so loading
//! is a pass over the bytes with no JSON parsing; its strings are borrowed from the embedded data. Layout, all
//! numbers little-endian u32: `THS1`, string count, string end offsets, blob length, blob (UTF-8); synsets
//! (pos byte, lemma ids, gloss id, related synset ids); index (word id, synset ids); antonyms (word id, word
//! ids); frequencies (word id, zipf * 10). Lists are prefixed with their length.

use crate::inflect::{self, Forms};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

/// WordNet part of speech ("n", "v", "a", "r") -> universal tag.
pub fn wn_to_upos(pos: &str) -> &'static str {
    match pos {
        "n" => "NOUN",
        "v" => "VERB",
        "a" => "ADJ",
        _ => "ADV",
    }
}

/// Universal tag -> WordNet part of speech, for the four WordNet has.
pub fn upos_to_wn(upos: &str) -> Option<&'static str> {
    match upos {
        "NOUN" => Some("n"),
        "VERB" => Some("v"),
        "ADJ" => Some("a"),
        "ADV" => Some("r"),
        _ => None,
    }
}

/// Penn tag -> universal tag, for the tags the thesaurus inflects to (`TAG_UPOS`).
pub fn tag_upos(tag: &str) -> Option<&'static str> {
    match tag {
        "NN" | "NNS" => Some("NOUN"),
        "VB" | "VBD" | "VBG" | "VBN" | "VBP" | "VBZ" => Some("VERB"),
        "JJ" | "JJR" | "JJS" => Some("ADJ"),
        "RB" | "RBR" | "RBS" => Some("ADV"),
        _ => None,
    }
}

/// The base-form tag of a universal tag.
fn base_tag(upos: &str) -> &'static str {
    match upos {
        "NOUN" => "NN",
        "VERB" => "VB",
        "ADJ" => "JJ",
        _ => "RB",
    }
}

/// The fixed order in which [`Thesaurus::tags`] lists tags (Python returned a set).
pub const TAG_ORDER: [&str; 17] =
    ["NN", "NNS", "NNP", "NNPS", "VB", "VBD", "VBG", "VBN", "VBP", "VBZ", "MD", "JJ", "JJR", "JJS", "RB", "RBR", "RBS"];

/// A synonym or related word offered for a word.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    /// "syn" (same synset) or "rel" (one link away).
    pub relation: &'static str,
    pub gloss: &'static str,
    /// Rank of the sense among the word's senses (0 = most frequent).
    pub sense_rank: usize,
    /// WordNet part of speech.
    pub pos: &'static str,
    pub score: Option<f64>,
    pub lm: Option<f64>,
    pub sim: Option<f64>,
    pub source: &'static str,
}

impl Candidate {
    pub fn new(text: String, relation: &'static str, gloss: &'static str, sense_rank: usize, pos: &'static str) -> Self {
        Candidate { text, relation, gloss, sense_rank, pos, score: None, lm: None, sim: None, source: "thesaurus" }
    }
}

/// A WordNet synset: part of speech, lemmas, gloss, related synsets.
#[derive(Debug)]
pub struct Synset {
    pub pos: &'static str,
    pub lemmas: Vec<&'static str>,
    pub gloss: &'static str,
    pub rel: Vec<u32>,
}

pub struct Thesaurus {
    /// Synsets (`S`).
    pub synsets: Vec<Synset>,
    /// Word -> its synsets, most frequent sense first (`I`).
    pub index: HashMap<&'static str, Vec<u32>>,
    /// Word -> direct antonyms (`A`).
    pub antonym_map: HashMap<&'static str, Vec<&'static str>>,
    /// Word -> zipf frequency * 10 (`F`).
    pub freq: HashMap<&'static str, u32>,
    ant_cache: Mutex<HashMap<String, Arc<BTreeSet<&'static str>>>>,
}

static DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/thesaurus.bin"));

/// The process-wide thesaurus, loaded on first use.
pub fn get() -> &'static Thesaurus {
    static T: LazyLock<Thesaurus> = LazyLock::new(|| Thesaurus::load(DATA));
    &T
}

struct Reader {
    data: &'static [u8],
    pos: usize,
}

impl Reader {
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }

    fn len(&mut self) -> usize {
        self.u32() as usize
    }

    fn bytes(&mut self, n: usize) -> &'static [u8] {
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        b
    }
}

impl Thesaurus {
    fn load(data: &'static [u8]) -> Thesaurus {
        assert_eq!(&data[..4], b"THS1", "thesaurus data");
        let mut r = Reader { data, pos: 4 };
        let n = r.len();
        let ends: Vec<usize> = (0..n).map(|_| r.len()).collect();
        let blob_len = r.len();
        let blob = std::str::from_utf8(r.bytes(blob_len)).expect("UTF-8 thesaurus strings");
        let mut start = 0;
        let strings: Vec<&'static str> = ends
            .iter()
            .map(|&e| {
                let s = &blob[start..e];
                start = e;
                s
            })
            .collect();
        let str_list = |r: &mut Reader| -> Vec<&'static str> {
            let n = r.len();
            (0..n).map(|_| strings[r.len()]).collect()
        };
        let n = r.len();
        let mut synsets = Vec::with_capacity(n);
        for _ in 0..n {
            let pos = match r.bytes(1)[0] {
                b'n' => "n",
                b'v' => "v",
                b'a' => "a",
                _ => "r",
            };
            let lemmas = str_list(&mut r);
            let gloss = strings[r.len()];
            let k = r.len();
            let rel = (0..k).map(|_| r.u32()).collect();
            synsets.push(Synset { pos, lemmas, gloss, rel });
        }
        let n = r.len();
        let mut index = HashMap::with_capacity(n);
        for _ in 0..n {
            let w = strings[r.len()];
            let k = r.len();
            index.insert(w, (0..k).map(|_| r.u32()).collect());
        }
        let n = r.len();
        let mut antonym_map = HashMap::with_capacity(n);
        for _ in 0..n {
            let w = strings[r.len()];
            antonym_map.insert(w, str_list(&mut r));
        }
        let n = r.len();
        let mut freq = HashMap::with_capacity(n);
        for _ in 0..n {
            let w = strings[r.len()];
            freq.insert(w, r.u32());
        }
        Thesaurus { synsets, index, antonym_map, freq, ant_cache: Mutex::new(HashMap::new()) }
    }

    fn synset(&self, sid: u32) -> &Synset {
        &self.synsets[sid as usize]
    }

    fn sids(&self, word: &str) -> &[u32] {
        self.index.get(word).map_or(&[], Vec::as_slice)
    }

    /// Whether WordNet has `word` (exactly, lower case) as a lemma.
    pub fn has(&self, word: &str) -> bool {
        self.index.contains_key(word)
    }

    // -- morphology ---------------------------------------------------------------------------------------

    /// `{upos: [lemma, ...]}` for a surface word: lemminflect's lemmas, the word itself where WordNet has it,
    /// and for unknown words a rule-based lemma that WordNet knows.
    pub fn lemmas(&self, word: &str) -> Forms {
        let w = word.to_lowercase();
        let mut out: Vec<(&'static str, Vec<String>)> = inflect::get_all_lemmas(&w, None)
            .0
            .into_iter()
            .filter(|(upos, _)| upos_to_wn(upos).is_some())
            .collect();
        for &sid in self.sids(&w) {
            let upos = wn_to_upos(self.synset(sid).pos);
            let slot = match out.iter().position(|(u, _)| *u == upos) {
                Some(i) => &mut out[i].1,
                None => {
                    out.push((upos, Vec::new()));
                    &mut out.last_mut().unwrap().1
                }
            };
            if !slot.contains(&w) {
                slot.push(w.clone());
            }
        }
        if out.is_empty() {
            for upos in ["NOUN", "VERB", "ADJ", "ADV"] {
                let oov = inflect::get_all_lemmas_oov(&w, upos);
                if let Some(lem) = oov.get(upos).and_then(|l| l.first()) {
                    if self.has(lem) {
                        out.push((upos, vec![lem.clone()]));
                    }
                }
            }
        }
        Forms(out)
    }

    /// Penn tags the surface form can carry (running -> NN, VBG), in [`TAG_ORDER`].
    pub fn tags(&self, word: &str) -> Vec<&'static str> {
        self.tags_with(word, &self.lemmas(word))
    }

    /// [`Thesaurus::tags`] with the word's lemmas already looked up.
    pub fn tags_with(&self, word: &str, lemmas: &Forms) -> Vec<&'static str> {
        let w = word.to_lowercase();
        let mut found = HashSet::new();
        for (upos, lems) in lemmas.iter() {
            for lem in lems {
                for (tag, forms) in inflect::get_all_inflections(lem, Some(upos)).iter() {
                    if forms.contains(&w) {
                        found.insert(tag);
                    }
                }
                if *lem == w {
                    found.insert(base_tag(upos));
                }
            }
        }
        let mut out: Vec<&'static str> = TAG_ORDER.iter().copied().filter(|t| found.contains(t)).collect();
        out.extend(found.into_iter().filter(|t| !TAG_ORDER.contains(t)));
        out
    }

    /// `lemma` inflected for a Penn tag (the head word of a phrase: last for nouns, first otherwise); None when
    /// there is no such form, or for an adjective phrase.
    pub fn inflect(&self, lemma: &str, tag: &str) -> Option<String> {
        let mut parts: Vec<String> = lemma.split(' ').map(str::to_string).collect();
        let upos = tag_upos(tag);
        if matches!(tag, "NN" | "JJ" | "RB" | "VB" | "VBP") {
            return Some(lemma.to_string());
        }
        if upos == Some("ADJ") && parts.len() > 1 {
            return None;
        }
        let head = if upos == Some("NOUN") { parts.len() - 1 } else { 0 };
        let forms = inflect::get_inflection(&parts[head], tag, true);
        parts[head] = forms.into_iter().next()?;
        Some(parts.join(" "))
    }

    /// The two words share a lemma, or look like derivations of one stem (demonstrate / demonstration).
    pub fn same_lemma(&self, a: &str, b: &str) -> bool {
        let (al, bl) = (a.to_lowercase(), b.to_lowercase());
        let (la, lb) = (self.lemmas(a), self.lemmas(b));
        let sa: HashSet<&str> = la.values().map(String::as_str).chain([al.as_str()]).collect();
        if lb.values().map(String::as_str).chain([bl.as_str()]).any(|x| sa.contains(x)) {
            return true;
        }
        let n = al.chars().count().min(bl.chars().count());
        let common = al.chars().zip(bl.chars()).take_while(|(x, y)| x == y).count();
        n >= 4 && common >= 4.max(n.saturating_sub(2))
    }

    // -- lookups ------------------------------------------------------------------------------------------

    /// How common a word is, on the zipf scale (0 when unknown).
    pub fn zipf(&self, word: &str) -> f64 {
        self.freq.get(word.to_lowercase().as_str()).copied().unwrap_or(0) as f64 / 10.0
    }

    /// Direct antonyms of the word and of its synonyms, plus WordNet's indirect ones (adjectives similar to an
    /// antonym: good -> bad -> terrible), never the word's own synonyms. Cached per word.
    pub fn antonyms(&self, word: &str) -> Arc<BTreeSet<&'static str>> {
        let key = word.to_lowercase();
        if let Some(hit) = self.ant_cache.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let ants = |w: &str| self.antonym_map.get(w).map_or(&[][..], Vec::as_slice);
        let own = self.synsets_of(word);
        let mut direct: BTreeSet<&'static str> = BTreeSet::new();
        for &sid in &own {
            for lem in &self.synset(sid).lemmas {
                direct.extend(ants(lem).iter().copied());
            }
        }
        for lem in self.lemmas(word).values() {
            direct.extend(ants(lem).iter().copied());
        }
        let mut out = direct.clone();
        let pos: HashSet<&str> = own.iter().map(|&sid| self.synset(sid).pos).collect();
        for a in &direct {
            for &sid in self.sids(a) {
                let s = self.synset(sid);
                if !pos.contains(s.pos) || own.contains(&sid) {
                    continue;
                }
                out.extend(s.lemmas.iter().copied());
                if s.pos == "a" {
                    for &r in &s.rel {
                        out.extend(self.synset(r).lemmas.iter().copied());
                    }
                }
            }
        }
        for &sid in &own {
            for lem in &self.synset(sid).lemmas {
                out.remove(lem);
            }
        }
        let out = Arc::new(out);
        self.ant_cache.lock().unwrap().insert(key, out.clone());
        out
    }

    pub fn is_antonym(&self, word: &str, cand: &str) -> bool {
        let ants = self.antonyms(word);
        if ants.is_empty() {
            return false;
        }
        ants.contains(cand.to_lowercase().as_str()) || self.lemmas(cand).values().any(|l| ants.contains(l.as_str()))
    }

    /// "syn" when the words share a synset, "rel" when their synsets are one link apart.
    pub fn relation(&self, word: &str, cand: &str) -> Option<&'static str> {
        let (sw, sc) = (self.synsets_of(word), self.synsets_of(cand));
        if !sw.is_disjoint(&sc) {
            return Some("syn");
        }
        let linked = |from: &HashSet<u32>, to: &HashSet<u32>| from.iter().any(|&s| self.synset(s).rel.iter().any(|r| to.contains(r)));
        (linked(&sw, &sc) || linked(&sc, &sw)).then_some("rel")
    }

    /// The synsets of the word's lemmas, for their parts of speech.
    fn synsets_of(&self, word: &str) -> HashSet<u32> {
        let mut out = HashSet::new();
        for (upos, lems) in self.lemmas(word).iter() {
            for lem in lems {
                out.extend(self.sids(lem).iter().filter(|&&sid| wn_to_upos(self.synset(sid).pos) == upos));
            }
        }
        out
    }

    /// The word's senses `(synset id, pos, gloss, lemmas)`, most frequent first within each lemma.
    pub fn senses(&self, word: &str) -> Vec<(u32, &'static str, &'static str, &[&'static str])> {
        let mut out = Vec::new();
        for (upos, lems) in self.lemmas(word).iter() {
            for lem in lems {
                for &sid in self.sids(lem) {
                    let s = self.synset(sid);
                    if wn_to_upos(s.pos) == upos {
                        out.push((sid, s.pos, s.gloss, s.lemmas.as_slice()));
                    }
                }
            }
        }
        out
    }

    /// Synonyms of `word` (and with `related`, words one link away), inflected to match it, one candidate per
    /// surface form. Python's defaults: `max_per_sense` 8, `related` true, `limit` 60.
    pub fn candidates(&self, word: &str, max_per_sense: usize, related: bool, limit: usize) -> Vec<Candidate> {
        let w = word.to_lowercase();
        let lemmas = self.lemmas(&w);
        let tags = self.tags_with(&w, &lemmas);
        let mut seen: HashSet<String> = HashSet::from([w.clone()]);
        let mut out = Vec::new();
        let ants = self.antonyms(&w);
        let own: HashSet<&str> = lemmas.values().map(String::as_str).chain([w.as_str()]).collect();
        for (upos, lems) in lemmas.iter() {
            let Some(wnpos) = upos_to_wn(upos) else { continue };
            let mut my_tags: Vec<&str> = tags.iter().copied().filter(|t| tag_upos(t) == Some(upos)).collect();
            if my_tags.is_empty() {
                my_tags.push(base_tag(upos));
            }
            for lem in lems {
                for (rank, &sid) in self.sids(lem).iter().enumerate() {
                    let s = self.synset(sid);
                    if s.pos != wnpos {
                        continue;
                    }
                    let mut groups = vec![("syn", &s.lemmas)];
                    if related {
                        groups.extend(s.rel.iter().map(|&r| ("rel", &self.synset(r).lemmas)));
                    }
                    for (relation, group) in groups {
                        let mut n = 0;
                        for syn in group {
                            // skip the word itself, antonyms, and "savings bank" for "bank"
                            if syn == lem || ants.contains(syn) || (syn.contains(' ') && syn.split(' ').any(|p| own.contains(p))) {
                                continue;
                            }
                            for tag in &my_tags {
                                let Some(form) = self.inflect(syn, tag) else { continue };
                                if form.is_empty() || seen.contains(&form) {
                                    continue;
                                }
                                seen.insert(form.clone());
                                out.push(Candidate::new(form, relation, s.gloss, rank, wnpos));
                                n += 1;
                            }
                            if n >= max_per_sense {
                                break;
                            }
                        }
                    }
                }
            }
        }
        out.truncate(limit);
        out
    }

    /// Lite-mode ranking: overlap of the sense's gloss with the context, sense prior and word frequency. Sets
    /// each candidate's score and sorts best first (ties keep their order).
    pub fn lesk_rank<S: AsRef<str>>(&self, _word: &str, context_words: &[S], mut cands: Vec<Candidate>) -> Vec<Candidate> {
        let ctx: HashSet<String> =
            context_words.iter().map(AsRef::as_ref).filter(|c| c.chars().count() > 2).map(str::to_lowercase).collect();
        let mut overlap: HashMap<&str, usize> = HashMap::new();
        for c in &cands {
            overlap.entry(c.gloss).or_insert_with(|| {
                let gloss = c.gloss.to_lowercase();
                let g: HashSet<&str> = gloss.split(|ch: char| !ch.is_ascii_lowercase()).filter(|t| !t.is_empty()).collect();
                g.iter().filter(|t| ctx.contains(**t)).count()
            });
        }
        for c in &mut cands {
            let z = self.zipf(c.text.split(' ').next().unwrap_or(""));
            c.score = Some(
                1.5 * overlap[c.gloss] as f64 - 0.35 * c.sense_rank as f64
                    + if c.relation == "syn" { 0.8 } else { 0.0 }
                    + 0.6 * z.min(5.5)
                    - if z < 2.5 { 1.5 } else { 0.0 }
                    - 0.4 * c.text.matches(' ').count() as f64,
            );
        }
        cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        cands
    }
}
