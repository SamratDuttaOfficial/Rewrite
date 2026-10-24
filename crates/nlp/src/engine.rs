//! The rewriting engine: word, phrase and sentence suggestions, whole-text auto-rewrite and the model-based
//! proofreading checks (was `rewrite/engine.py`).
//!
//! Smart mode (models downloaded):
//! * words: WordNet synonyms and RoBERTa proposals, scored by how well they fit the sentence (masked-LM
//!   pseudo-likelihood with the original visible), filtered by part of speech, inflection and antonyms, and
//!   checked for meaning with MiniLM contextual embeddings.
//! * phrases: phrase book, one-word swaps and T5 paraphrases of the phrase, ranked in context.
//! * sentences: T5 paraphrases, ranked by meaning similarity and amount reworded.
//!
//! Lite mode (no models): WordNet and the phrase book, ranked with a Lesk-style sense overlap.
//!
//! Proofreading reuses the same parts: spelling corrections are ranked by the context model, and grammar slips
//! (agreement, verb forms, confused words) are found by asking that model whether a sibling form of a word fits
//! the sentence far better than the word that was written.
//!
//! Offsets are byte offsets. Models are loaded on a worker thread while other threads keep calling: each call
//! takes the models loaded at its start.

use crate::download;
use crate::mlm::Scores;
use crate::{Embedder, GenerateOptions, MaskedLm, Paraphraser};
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use rewrite_core::protect::{clip, merge, overlaps};
use rewrite_lang::inflect;
use rewrite_lang::phrases;
use rewrite_lang::py;
use rewrite_lang::speller::{Speller, FUNCTION_WORDS};
use rewrite_lang::text::{match_case, sentence_bounds, word_diff_ratio, STOPWORDS, WORD_RE};
use rewrite_lang::thesaurus::{self, wn_to_upos, Candidate, Thesaurus};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// A suggestion for a word, phrase or sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub text: String,
    /// synonym | related | in context | phrase | rewrite | spelling
    pub kind: &'static str,
    /// 0..1, for display.
    pub fit: f64,
    pub note: String,
    /// The raw engine score (auto-rewrite thresholds use it; for sentences it is the meaning similarity).
    pub score: f64,
}

impl Suggestion {
    fn new(text: String, kind: &'static str, fit: f64, note: impl Into<String>, score: f64) -> Suggestion {
        Suggestion { text, kind, fit, note: note.into(), score }
    }
}

/// Options of one auto-rewrite pass.
#[derive(Clone, Debug)]
pub struct AutoOptions {
    /// light | medium | strong
    pub strength: String,
    pub phrases: bool,
    pub words: bool,
    pub sentences: bool,
    /// Every line is a paragraph of its own (formatted documents).
    pub lines: bool,
}

/// One auto-rewrite edit: (start, end, new text, whole sentence rephrased rather than words swapped).
pub type Edit = (usize, usize, String, bool);

/// A grammar slip: (start, end, message, fixes), offsets into the sentence.
pub type Slip = (usize, usize, String, Vec<String>);

/// Sibling forms a slip could have swapped in for a word, with the kinds of slip, in the order found.
pub type Alternatives = Vec<(String, Vec<&'static str>)>;

const UPOS4: [&str; 4] = ["NOUN", "VERB", "ADJ", "ADV"];

static NUMBER_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "zero one two three four five six seven eight nine ten eleven twelve twenty thirty forty fifty sixty seventy
eighty ninety hundred hundreds thousand thousands million millions billion billions trillion first second third
fourth fifth half double twice once dozen dozens percent"
        .split_whitespace()
        .collect()
});

/// Words that get mistaken for each other. The context model decides which one fits.
const CONFUSED: &[&[&str]] = &[
    &["their", "there", "they're"], &["its", "it's"], &["your", "you're"], &["then", "than"], &["to", "too"],
    &["affect", "effect"], &["affects", "effects"], &["lose", "loose"], &["accept", "except"], &["advice", "advise"],
    &["whether", "weather"], &["quite", "quiet"], &["were", "where"], &["whose", "who's"], &["passed", "past"],
    &["principle", "principal"], &["led", "lead"], &["choose", "chose"], &["breath", "breathe"],
    &["complement", "compliment"],
];
/// Function words that must agree with their subject or noun.
const AGREEING: &[&[&str]] = &[
    &["is", "are"], &["was", "were"], &["has", "have"], &["does", "do"], &["doesn't", "don't"], &["isn't", "aren't"],
    &["wasn't", "weren't"], &["hasn't", "haven't"], &["this", "these"], &["much", "many"],
];
const BASE_AFTER: &[&str] = &[
    "to", "not", "can", "could", "will", "would", "shall", "should", "may", "might", "must", "did", "does", "do",
    "didn't", "doesn't", "don't", "cannot", "can't", "couldn't", "won't", "wouldn't", "shouldn't",
];
const AGREE: &str = "Agreement: “{alt}” fits this sentence better than “{word}”.";
const NUMBER: &str = "Singular or plural? “{alt}” fits this sentence better than “{word}”.";

/// Kind of slip -> how much better (in log-probability) the other form must fit, and the message.
fn slip_rule(kind: &str) -> (f64, &'static str) {
    match kind {
        "confused" => (3.0, "Easily confused words: “{alt}” fits this sentence better than “{word}”."),
        "pair" | "agree" => (2.5, AGREE),
        "form" => (2.5, "Verb form: “{alt}” fits this sentence better than “{word}”."),
        "singular" => (3.5, NUMBER),
        _ => (3.0, NUMBER),
    }
}

/// The part of speech a kind of slip needs the position to have.
fn needs_pos(kind: &str) -> Option<&'static str> {
    match kind {
        "agree" | "form" => Some("VERB"),
        "singular" | "plural" => Some("NOUN"),
        _ => None,
    }
}

/// The written word must be unlikely, the other form plausible.
const MAX_OWN_FIT: f64 = -3.0;
const MIN_ALT_FIT: f64 = -4.5;
const PLURAL_CUES: &[&str] = &[
    "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "many", "several", "few", "various", "these",
    "those", "both", "multiple", "numerous", "dozen", "hundred", "thousand", "million",
];
const SINGULAR_CUES: &[&str] = &["a", "an", "each", "every", "another"];

static REF_CMD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\\(cite\w*|[Cc]ref|ref|eqref|autoref|pageref|nameref|url|href|verb|texttt|lstinline|mintinline|path|SI|si|num|qty|ang)\b").unwrap()
});
static LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z']+").unwrap());
static DIGITS_BEFORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+\s+(?:[\w-]+\s+)?$").unwrap());
static ENDS_STOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?;:]$").unwrap());
static TRAILING_STOPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?;:,]+$").unwrap());

/// Share of eligible words to change, minimum raw score, minimum meaning similarity.
fn strength(name: &str) -> (f64, f64, f64) {
    match name {
        "light" => (0.15, -3.5, 0.80),
        "strong" => (0.50, -6.5, 0.72),
        _ => (0.30, -5.0, 0.76),
    }
}

/// The suggestion kind for a relation.
fn kind_of(relation: Option<&str>) -> &'static str {
    match relation {
        Some("syn") => "synonym",
        Some("rel") => "related",
        Some(_) => "phrase",
        None => "in context",
    }
}

fn bonus(relation: Option<&str>) -> f64 {
    match relation {
        Some("syn") => 2.0,
        Some("rel") => 1.0,
        Some(_) => 1.5,
        None => -0.5,
    }
}

/// Collapses white-space runs to single spaces, remapping `start` and `end`.
fn normalize(s: &str, start: usize, end: usize) -> (String, usize, usize) {
    let mut res = String::with_capacity(s.len());
    let (mut ns, mut ne, mut prev) = (None, None, false);
    for (i, ch) in s.char_indices() {
        if i == start {
            ns = Some(res.len());
        }
        if i == end {
            ne = Some(res.len());
        }
        if py::is_space(ch) {
            if !prev {
                res.push(' ');
            }
            prev = true;
        } else {
            res.push(ch);
            prev = false;
        }
    }
    let n = res.len();
    (res, ns.unwrap_or(n), ne.unwrap_or(n))
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x.clamp(-30.0, 30.0)).exp())
}

/// Turns raw scores into 0..1 fits: mostly absolute, partly relative to the best one.
fn calibrate(items: &mut [Suggestion], center: f64, spread: f64) {
    let Some(best) = items.iter().map(|x| x.score).reduce(f64::max) else { return };
    for x in items {
        x.fit = 0.7 * sigmoid((x.score - center) / spread) + 0.3 * sigmoid((x.score - best) / spread + 2.0);
    }
}

fn by_score_desc(items: &mut [Suggestion]) {
    items.sort_by(|a, b| b.score.total_cmp(&a.score));
}

/// Tiny rule-based part-of-speech guess for Lite mode (no models).
fn guess_pos(s: &str, a: usize, b: usize) -> Option<&'static [&'static str]> {
    let word = s[a..b].to_lowercase();
    let prev = LETTERS.find_iter(&s[..a]).last().map(|m| m.as_str().to_lowercase()).unwrap_or_default();
    let prev = prev.as_str();
    if word.ends_with("ly") && word.chars().count() > 4 {
        return Some(&["ADV"]);
    }
    if ["to", "will", "would", "can", "could", "should", "must", "may", "might", "not", "did", "does", "do"].contains(&prev) {
        return Some(&["VERB"]);
    }
    if ["the", "a", "an", "this", "that", "these", "those", "his", "her", "their", "its", "my", "your", "our", "of", "each",
        "every", "some", "any", "no"]
        .contains(&prev)
    {
        return Some(&["NOUN", "ADJ"]);
    }
    if ["is", "are", "was", "were", "be", "been", "being", "am"].contains(&prev) && (word.ends_with("ing") || word.ends_with("ed")) {
        return Some(&["VERB", "ADJ"]);
    }
    if ["very", "more", "most", "less", "so", "too", "quite", "really", "rather"].contains(&prev) {
        return Some(&["ADJ", "ADV"]);
    }
    None
}

/// The parts of speech lemminflect knows a word as (of the four WordNet has).
fn upos(word: &str) -> BTreeSet<&'static str> {
    inflect::get_all_lemmas(&word.to_lowercase(), None).0.into_iter().map(|(u, _)| u).filter(|u| UPOS4.contains(u)).collect()
}

/// Votes for parts of speech from the model's guesses `(word, logprob)`.
fn pos_votes(guesses: &[(String, f64)]) -> (BTreeMap<&'static str, f64>, f64) {
    let mut votes: BTreeMap<&'static str, f64> = BTreeMap::new();
    for (w, lp) in guesses {
        let ups = upos(w);
        for &u in &ups {
            *votes.entry(u).or_default() += lp.exp() / ups.len() as f64;
        }
    }
    let total = votes.values().sum();
    (votes, total)
}

/// Share of each part of speech among the model's guesses for a position.
fn pos_share(guesses: &[(String, f64)]) -> BTreeMap<&'static str, f64> {
    let (votes, total) = pos_votes(guesses);
    if total == 0.0 {
        return BTreeMap::new();
    }
    votes.into_iter().map(|(u, v)| (u, v / total)).collect()
}

/// Which parts of speech the context wants here: the model's own guesses vote.
fn pos_vote(proposals: &[(String, f64)], target: BTreeSet<&'static str>) -> BTreeSet<&'static str> {
    let (votes, total) = pos_votes(&proposals[..proposals.len().min(15)]);
    let allowed: BTreeSet<&str> =
        votes.into_iter().filter(|&(u, v)| total != 0.0 && v / total >= 0.2 && target.contains(u)).map(|(u, _)| u).collect();
    if allowed.is_empty() { target } else { allowed }
}

fn score_of(scores: &Scores, text: &str) -> Option<f64> {
    scores.iter().find(|(k, _)| k == text).map(|&(_, v)| v)
}

/// Characters between two byte offsets.
fn char_gap(s: &str, i: usize, j: usize) -> usize {
    s[i.min(j)..i.max(j)].chars().count()
}

/// An insertion-ordered map where the first value set for a key stays (Python's `dict.setdefault`).
struct Ordered<V> {
    items: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> Ordered<V> {
    fn new() -> Self {
        Ordered { items: Vec::new(), index: HashMap::new() }
    }

    fn setdefault(&mut self, key: String, value: V) {
        if !self.index.contains_key(&key) {
            self.index.insert(key.clone(), self.items.len());
            self.items.push((key, value));
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.items[i].1)
    }

    fn keys(&self) -> Vec<String> {
        self.items.iter().map(|(k, _)| k.clone()).collect()
    }
}

/// A protected span replaced by a placeholder: tag, original text, whether white space was around it, and the
/// characters next to it.
struct Slot {
    tag: String,
    orig: String,
    before: bool,
    after: bool,
    prev_ch: String,
    next_ch: String,
}

/// One scored word filtered from the model's output: text, preliminary score, relation, gloss.
struct Row {
    text: String,
    prelim: f64,
    relation: Option<&'static str>,
    gloss: String,
}

/// A possible slip in a sentence: gain, word index, start, word, sibling, kind, fit of the word and of the sibling.
type SlipScore = (f64, usize, usize, String, String, &'static str, f64, f64);

#[derive(Clone, Default)]
struct Models {
    mlm: Option<Arc<MaskedLm>>,
    emb: Option<Arc<Embedder>>,
    t5: Option<Arc<Paraphraser>>,
}

type R<T> = Result<T, String>;

pub struct Engine {
    thes: &'static Thesaurus,
    speller: Speller,
    alts: Mutex<HashMap<(String, bool), Alternatives>>,
    models: RwLock<Models>,
    errors: Mutex<BTreeMap<String, String>>,
    loading: Mutex<()>,
}

impl Engine {
    pub fn new<S: AsRef<str>>(user_words: &[S]) -> Engine {
        let thes = thesaurus::get();
        Engine {
            thes,
            speller: Speller::new(thes, user_words.iter().map(AsRef::as_ref)),
            alts: Mutex::default(),
            models: RwLock::default(),
            errors: Mutex::default(),
            loading: Mutex::default(),
        }
    }

    /// Loads whatever models are present in the models folder. Safe to call again after downloads.
    pub fn load_models(&self) {
        self.load_models_in(&rewrite_core::paths::models_dir());
    }

    /// [`load_models`](Self::load_models) from another models folder.
    pub fn load_models_in(&self, dir: &Path) {
        let _one_at_a_time = self.loading.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = crate::onnx::environment() {
            self.errors.lock().unwrap().insert("runtime".into(), e);
            return;
        }
        self.errors.lock().unwrap().remove("runtime");
        let have = self.snapshot();
        let mut new_mlm = None;
        for key in ["context", "similarity", "paraphrase"] {
            let loaded = match key {
                "context" => have.mlm.is_some(),
                "similarity" => have.emb.is_some(),
                _ => have.t5.is_some(),
            };
            if loaded || !download::present_in(dir, key) {
                continue;
            }
            let folder = dir.join(key);
            let result = match key {
                "context" => MaskedLm::load(&folder).map(|m| {
                    let m = Arc::new(m);
                    new_mlm = Some(m.clone());
                    self.models.write().unwrap().mlm = Some(m);
                }),
                "similarity" => Embedder::load(&folder).map(|m| self.models.write().unwrap().emb = Some(Arc::new(m))),
                _ => Paraphraser::load(&folder).map(|m| self.models.write().unwrap().t5 = Some(Arc::new(m))),
            };
            let mut errors = self.errors.lock().unwrap();
            match result {
                Ok(()) => errors.remove(key),
                Err(e) => errors.insert(key.to_string(), e), // a damaged download, an unsupported CPU, ...
            };
        }
        if let Some(m) = new_mlm {
            self.speller.set_vocab(Some(m.known_words()));
        }
    }

    /// Why a model (or "runtime") could not be loaded.
    pub fn errors(&self) -> BTreeMap<String, String> {
        self.errors.lock().unwrap().clone()
    }

    /// The context model is loaded (Smart mode).
    pub fn smart(&self) -> bool {
        self.snapshot().mlm.is_some()
    }

    /// The meaning check is loaded.
    pub fn has_emb(&self) -> bool {
        self.snapshot().emb.is_some()
    }

    /// The sentence paraphraser is loaded.
    pub fn has_t5(&self) -> bool {
        self.snapshot().t5.is_some()
    }

    pub fn speller(&self) -> &Speller {
        &self.speller
    }

    pub fn thesaurus(&self) -> &'static Thesaurus {
        self.thes
    }

    /// The context model, if loaded.
    pub fn masked_lm(&self) -> Option<Arc<MaskedLm>> {
        self.snapshot().mlm
    }

    fn snapshot(&self) -> Models {
        self.models.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { e: self, t: self.thes, m: self.snapshot() }
    }

    /// Suggestions for `text[start..end]`; `text` is the surrounding context (one or two sentences).
    pub fn suggest(&self, text: &str, start: usize, end: usize, n: usize, generate: bool) -> Vec<Suggestion> {
        logged(self.ctx().suggest(text, start, end, n, generate))
    }

    /// Corrections for the misspelled word `text[start..end]`, best first.
    pub fn spell_suggest(&self, text: &str, start: usize, end: usize, n: usize) -> Vec<Suggestion> {
        logged(self.ctx().spell_suggest(text, start, end, n))
    }

    /// Rewrites of a whole sentence; `protected` are spans inside it that must come back unchanged.
    /// `variation` > 0 samples instead of the deterministic search (a different seed each time).
    pub fn paraphrase(&self, sentence: &str, protected: &[(usize, usize)], n: usize, variation: u32) -> Vec<Suggestion> {
        logged(self.ctx().paraphrase(sentence, protected, n, variation))
    }

    /// Slips in one sentence that the context model is confident about (none without it).
    pub fn grammar(&self, sentence: &str, spans: &[(usize, usize)]) -> Vec<Slip> {
        logged(self.ctx().grammar(sentence, spans))
    }

    /// Edits for the sentences inside `regions`, skipping `protected` spans (sorted and merged).
    pub fn auto_rewrite(
        &self,
        text: &str,
        regions: &[(usize, usize)],
        protected: &[(usize, usize)],
        opts: &AutoOptions,
        progress: &dyn Fn(usize, usize),
        cancelled: &dyn Fn() -> bool,
    ) -> Vec<Edit> {
        self.ctx().auto_rewrite(text, regions, protected, opts, progress, cancelled)
    }

    /// Sibling forms a slip could have swapped in for `word` (after the word `prev`), with the kinds of slip.
    pub fn alternatives(&self, word: &str, prev: &str) -> Alternatives {
        let lw = word.to_lowercase();
        let key = (lw.clone(), BASE_AFTER.contains(&prev));
        if let Some(hit) = self.alts.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let mut alts: Alternatives = Vec::new();
        for (kind, groups) in [("confused", CONFUSED), ("pair", AGREEING)] {
            for group in groups.iter().filter(|g| g.contains(&lw.as_str())) {
                for &x in group.iter().filter(|&&x| x != lw) {
                    match alts.iter_mut().find(|(k, _)| k == x) {
                        Some(slot) => slot.1 = vec![kind],
                        None => alts.push((x.to_string(), vec![kind])),
                    }
                }
            }
        }
        if !STOPWORDS.contains(lw.as_str()) && !FUNCTION_WORDS.contains(lw.as_str()) && lw.chars().count() > 2 && !lw.contains('\'') {
            let lemmas = self.thes.lemmas(&lw);
            let tags = self.thes.tags_with(&lw, &lemmas);
            let has = |t: &str| tags.contains(&t);
            let mut offer = |lemma: &str, tag: Option<&str>, kind: &'static str| {
                let x = match tag {
                    None => Some(lemma.to_string()),
                    Some(tag) => inflect::get_inflection(lemma, tag, false).into_iter().next(),
                };
                let Some(x) = x.filter(|x| !x.is_empty() && *x != lw && !x.contains(' ')) else { return };
                let slot = match alts.iter().position(|(k, _)| *k == x) {
                    Some(i) => &mut alts[i].1,
                    None => {
                        alts.push((x, Vec::new()));
                        &mut alts.last_mut().unwrap().1
                    }
                };
                if !slot.contains(&kind) {
                    slot.push(kind);
                }
            };
            let base_after = BASE_AFTER.contains(&prev);
            for v in lemmas.get("VERB").unwrap_or(&[]) {
                if has("VBZ") {
                    offer(v, None, "agree");
                }
                if has("VB") || has("VBP") {
                    offer(v, Some("VBZ"), "agree");
                }
                if has("VBD") && !has("VBN") {
                    offer(v, Some("VBN"), "form");
                }
                if has("VBN") && !has("VBD") {
                    offer(v, Some("VBD"), "form");
                }
                if base_after && !(has("VB") || has("VBP")) {
                    offer(v, None, "form");
                }
            }
            for noun in lemmas.get("NOUN").unwrap_or(&[]) {
                if has("NNS") {
                    offer(noun, None, "singular");
                }
                if has("NN") {
                    offer(noun, Some("NNS"), "plural");
                }
            }
        }
        let mut cache = self.alts.lock().unwrap();
        if cache.len() > 20000 {
            cache.clear();
        }
        cache.insert(key, alts.clone());
        alts
    }
}

/// The result, or nothing when a model call failed (the error is reported on stderr).
fn logged<T: Default>(r: R<T>) -> T {
    r.unwrap_or_else(|e| {
        eprintln!("rewrite engine: {e}");
        T::default()
    })
}

/// One call's view of the engine: the models loaded when it started.
struct Ctx<'e> {
    e: &'e Engine,
    t: &'static Thesaurus,
    m: Models,
}

impl Ctx<'_> {
    fn suggest(&self, text: &str, start: usize, end: usize, n: usize, generate: bool) -> R<Vec<Suggestion>> {
        let (s, a, b) = normalize(text, start, end);
        let target = py::strip(&s[a..b]);
        if target.is_empty() {
            return Ok(Vec::new());
        }
        if target.contains(' ') {
            return self.suggest_phrase(&s, a, b, n, generate);
        }
        self.suggest_word(&s, a, b, n)
    }

    // -- words ----------------------------------------------------------------------------------------------

    /// The parts of speech of a word's lemmas, or all four.
    fn lemma_upos(&self, word: &str) -> BTreeSet<&'static str> {
        let set: BTreeSet<&'static str> = self.t.lemmas(word).0.into_iter().map(|(u, _)| u).collect();
        if set.is_empty() { UPOS4.into_iter().collect() } else { set }
    }

    /// Filters raw model scores into rows.
    fn word_rows(&self, word: &str, scores: &Scores, proposals: &[(String, f64)], cmap: &Ordered<Option<Candidate>>) -> Vec<Row> {
        let t = self.t;
        let allowed = pos_vote(proposals, self.lemma_upos(word));
        let target_tags = t.tags(word);
        let lword = word.to_lowercase();
        let (mut rows, mut seen) = (Vec::new(), HashSet::new());
        for (text, lp) in scores.iter().chain(proposals) {
            let text = match_case(word, text);
            let key = text.to_lowercase();
            if seen.contains(&key) || key == lword {
                continue;
            }
            seen.insert(key.clone());
            if STOPWORDS.contains(key.as_str()) || t.same_lemma(word, &text) || t.is_antonym(word, &text) {
                continue;
            }
            let (relation, gloss) = match cmap.get(&text) {
                Some(Some(c)) => {
                    if !allowed.contains(wn_to_upos(c.pos)) {
                        continue;
                    }
                    (Some(c.relation), c.gloss.to_string())
                }
                Some(None) => (Some("phrase"), "phrase book".to_string()),
                None => {
                    let ups = upos(&text);
                    if !ups.is_empty() && ups.is_disjoint(&allowed) {
                        continue;
                    }
                    if !target_tags.is_empty() {
                        let ct = t.tags(&text);
                        if !ct.is_empty() && !ct.iter().any(|x| target_tags.contains(x)) {
                            continue;
                        }
                    }
                    let relation = t.relation(word, &text);
                    let gloss = match relation {
                        Some(_) => t.senses(&text).first().map(|s| s.2.to_string()).unwrap_or_default(),
                        None => String::new(),
                    };
                    (relation, gloss)
                }
            };
            let z = t.zipf(text.split(' ').next().unwrap_or(""));
            let rare = if z < 2.5 { 2.5 } else if z < 3.2 { 1.0 } else { 0.0 };
            let prelim = lp + bonus(relation) - rare - 0.7 * text.matches(' ').count() as f64;
            rows.push(Row { text, prelim, relation, gloss });
        }
        rows
    }

    fn suggest_word(&self, s: &str, a: usize, b: usize, n: usize) -> R<Vec<Suggestion>> {
        let word = &s[a..b];
        let cands: Vec<Candidate> =
            self.t.candidates(word, 6, true, 40).into_iter().filter(|c| c.text.matches(' ').count() <= 1).collect();
        let book = phrases::lookup(word);
        let Some(mlm) = &self.m.mlm else { return Ok(self.lite_word(s, word, cands, book, n)) };

        let mut cmap = Ordered::new();
        for c in cands {
            cmap.setdefault(match_case(word, &c.text), Some(c));
        }
        for x in book {
            cmap.setdefault(match_case(word, &x), None);
        }
        let subs = mlm.substitutes(s, a, b, &cmap.keys(), 25, &[word], 8)?;
        let mut rows = self.word_rows(word, &subs.scores, &subs.proposals, &cmap);
        rows.sort_by(|x, y| y.prelim.total_cmp(&x.prelim));
        rows.truncate(16.max(n + 4));
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let sims = match &self.m.emb {
            Some(emb) => emb.span_similarity(s, a, b, &rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>())?.0,
            None => vec![0.55; rows.len()],
        };
        let mut out = Vec::new();
        for (r, ts) in rows.into_iter().zip(sims) {
            if self.m.emb.is_some() && r.relation.is_none() && ts < 0.5 {
                continue; // a model guess that drifts in meaning (often an antonym: expensive -> free)
            }
            out.push(Suggestion::new(r.text, kind_of(r.relation), 0.0, r.gloss, r.prelim + 4.0 * (ts - 0.55)));
        }
        calibrate(&mut out, -5.0, 1.5);
        by_score_desc(&mut out);
        // keep dictionary-backed options; drop weak model-only guesses
        Ok(out.into_iter().filter(|x| x.fit >= if x.kind == "in context" { 0.15 } else { 0.08 }).take(n).collect())
    }

    fn lite_word(&self, s: &str, word: &str, mut cands: Vec<Candidate>, book: Vec<String>, n: usize) -> Vec<Suggestion> {
        let t = self.t;
        if let Some(pos) = s.find(word).and_then(|a| guess_pos(s, a, a + word.len())) {
            let keep: Vec<Candidate> = cands.iter().filter(|c| pos.contains(&wn_to_upos(c.pos))).cloned().collect();
            if !keep.is_empty() {
                cands = keep;
            }
        }
        cands.retain(|c| t.zipf(c.text.split(' ').next().unwrap_or("")) >= 2.8);
        let ctx: Vec<&str> = WORD_RE.find_iter(s).map(|m| m.as_str()).filter(|w| !STOPWORDS.contains(w.to_lowercase().as_str())).collect();
        let (mut out, mut seen) = (Vec::new(), HashSet::new());
        for c in t.lesk_rank(word, &ctx, cands) {
            if seen.insert(c.text.clone()) {
                let score = c.score.unwrap_or(0.0) - if c.text.contains('-') { 0.8 } else { 0.0 };
                out.push(Suggestion::new(match_case(word, &c.text), kind_of(Some(c.relation)), 0.0, c.gloss, score));
            }
        }
        by_score_desc(&mut out);
        calibrate(&mut out, 2.0, 1.2);
        let mut all: Vec<Suggestion> =
            book.iter().map(|x| Suggestion::new(match_case(word, x), "phrase", 0.75, "phrase book", 3.0)).collect();
        all.extend(out);
        all.truncate(n);
        all
    }

    // -- phrases --------------------------------------------------------------------------------------------

    fn clean_generated(orig: &str, generated: &str) -> Option<String> {
        let mut g = py::strip(py::strip(generated).trim_matches('"')).to_string();
        if g.is_empty() || g.contains('\n') {
            return None;
        }
        if !ENDS_STOP.is_match(py::strip(orig)) {
            g = py::strip(&TRAILING_STOPS.replace(&g, "")).to_string();
        }
        let shouting = {
            let mut cs = g.chars();
            let (c0, c1) = (cs.next(), cs.next());
            (c0 == Some('I') && !c1.is_some_and(py::is_word)) || (c0.is_some_and(|c| c.is_ascii_uppercase()) && c1.is_some_and(|c| c.is_ascii_uppercase()))
        };
        if py::starts_lower(orig) && py::starts_upper(&g) && !shouting {
            let mut cs = g.chars();
            let first = cs.next().unwrap();
            g = first.to_lowercase().chain(cs).collect();
        }
        if g.to_lowercase() == orig.to_lowercase() || py::split_ws(&g).count() > 2 * py::split_ws(orig).count() + 3 {
            return None;
        }
        Some(g)
    }

    /// Batched, fast best-synonym search for several words of one sentence (WordNet-backed only): the best
    /// `per_word` swaps per target, in target order. Needs the context model.
    fn word_swaps(
        &self,
        mlm: &MaskedLm,
        s: &str,
        targets: &[(usize, usize)],
        per_word: usize,
        min_score: f64,
        min_sim: f64,
    ) -> R<Vec<((usize, usize), Vec<(String, f64)>)>> {
        let t = self.t;
        let mut specs = Vec::new();
        for &(a, b) in targets {
            let w = &s[a..b];
            let mut cm = Ordered::new();
            for c in t.candidates(w, 6, true, 40) {
                if !c.text.contains(' ') {
                    cm.setdefault(match_case(w, &c.text), c);
                }
            }
            specs.push((a, b, w, cm));
        }
        let res = mlm.score_many(s, &specs.iter().map(|(a, b, _, cm)| (*a, *b, cm.keys())).collect::<Vec<_>>(), 12)?;
        let mut out = Vec::new();
        for ((a, b, w, cm), (scores, props)) in specs.iter().zip(res) {
            let allowed = pos_vote(&props, self.lemma_upos(w));
            let mut best = Vec::new();
            for (text, lp) in &scores {
                let c = cm.get(text).expect("scored candidates come from the map");
                if !allowed.contains(wn_to_upos(c.pos)) || t.zipf(text) < 3.0 {
                    continue;
                }
                if t.same_lemma(w, text) || t.is_antonym(w, text) {
                    continue;
                }
                let score = lp + bonus(Some(c.relation));
                if score >= min_score {
                    best.push((text.clone(), score));
                }
            }
            best.sort_by(|x, y| y.1.total_cmp(&x.1));
            best.truncate(per_word + 1);
            out.push(((*a, *b), best));
        }
        if let Some(emb) = self.m.emb.as_ref().filter(|_| min_sim != 0.0) {
            // meaning check: the swapped word must still "mean" the same thing in context
            let keys: Vec<usize> = (0..out.len()).filter(|&i| !out[i].1.is_empty()).collect();
            let items: Vec<(usize, usize, Vec<String>)> =
                keys.iter().map(|&i| (out[i].0.0, out[i].0.1, out[i].1.iter().map(|(t, _)| t.clone()).collect())).collect();
            let sims = emb.multi_span_similarity(s, &items)?;
            for (&i, ss) in keys.iter().zip(sims) {
                let kept = out[i].1.drain(..).zip(ss).filter(|(_, x)| *x >= min_sim).map(|(ts, _)| ts).collect();
                out[i].1 = kept;
            }
        }
        for (_, v) in &mut out {
            v.truncate(per_word);
        }
        Ok(out)
    }

    fn suggest_phrase(&self, s: &str, a: usize, b: usize, n: usize, generate: bool) -> R<Vec<Suggestion>> {
        let phrase = &s[a..b];
        let mut alts: Vec<(String, &'static str)> = phrases::lookup(phrase).into_iter().map(|x| (x, "phrase")).collect();
        if let Some(mlm) = &self.m.mlm {
            let words: Vec<(usize, usize)> = WORD_RE
                .find_iter(phrase)
                .filter(|m| m.as_str().len() >= 4 && !STOPWORDS.contains(m.as_str().to_lowercase().as_str()))
                .take(3)
                .map(|m| (a + m.start(), a + m.end()))
                .collect();
            for ((x, y), best) in self.word_swaps(mlm, s, &words, 2, -7.0, 0.0)? {
                for (text, _) in best {
                    alts.push((format!("{}{text}{}", &phrase[..x - a], &phrase[y - a..]), "swap"));
                }
            }
        }
        // T5 is unreliable on tiny fragments like "in order to"; the phrase book covers those.
        let n_words = py::split_ws(phrase).count();
        if let Some(t5) = self.m.t5.as_ref().filter(|_| generate && (n_words > 3 || alts.is_empty()) && n_words <= 25) {
            let mut gens = t5.generate(phrase, 5, &GenerateOptions::default())?;
            let sampled = GenerateOptions { sample: true, seed: Some(phrase.chars().count() as u64), ..Default::default() };
            gens.extend(t5.generate(phrase, 3, &sampled)?);
            for g in gens {
                if let Some(g) = Self::clean_generated(phrase, &g) {
                    alts.push((g, "rewrite"));
                }
            }
        }
        let (mut seen, mut uniq) = (HashSet::new(), Vec::new());
        let lphrase = phrase.to_lowercase();
        for (x, k) in alts {
            let x = if py::starts_upper(phrase) { match_case(phrase, &x) } else { x };
            let lx = x.to_lowercase();
            if !seen.contains(&lx) && lx != lphrase {
                seen.insert(lx);
                uniq.push((x, k));
            }
        }
        if uniq.is_empty() {
            return Ok(Vec::new());
        }
        let Some(mlm) = &self.m.mlm else {
            return Ok(uniq
                .into_iter()
                .enumerate()
                .map(|(i, (x, _))| Suggestion::new(x, "phrase", (0.85 - 0.05 * i as f64).max(0.3), "phrase book", 0.0))
                .take(n)
                .collect());
        };
        let texts: Vec<&str> = uniq.iter().map(|(x, _)| x.as_str()).collect();
        let subs = mlm.substitutes(s, a, b, &texts, 0, &[] as &[&str], 12)?;
        let (span_sim, sent_sim) = match &self.m.emb {
            Some(emb) => emb.span_similarity(s, a, b, &texts)?,
            None => (vec![0.9; texts.len()], vec![0.9; texts.len()]),
        };
        let mut out = Vec::new();
        for (((x, k), ss), cs) in uniq.iter().zip(span_sim).zip(sent_sim) {
            let Some(sc) = score_of(&subs.scores, x) else { continue };
            if self.m.emb.is_some() && *k == "rewrite" && cs < 0.9 {
                continue;
            }
            let lm = (sc - subs.original) / (mlm.enc(&format!(" {x}")).len().max(1) as f64).sqrt();
            let extra = match *k {
                "phrase" => 0.8,
                "swap" => 0.5,
                _ => 0.0,
            };
            let score = lm + 6.0 * (ss - 1.0) + 15.0 * (cs - 1.0) + extra;
            let kind = if *k == "swap" { "synonym" } else { k };
            let note = if self.m.emb.is_some() { format!("meaning {:.0}%", cs * 100.0) } else { String::new() };
            out.push(Suggestion::new(x.clone(), kind, 0.0, note, score));
        }
        calibrate(&mut out, -4.0, 1.2);
        by_score_desc(&mut out);
        Ok(out.into_iter().filter(|x| x.fit >= 0.08).take(n).collect())
    }

    /// The phrase-book alternative that best fits the sentence (cheap, for auto mode).
    fn choose_phrase(&self, s: &str, a: usize, b: usize, alts: &[String]) -> R<String> {
        let Some(mlm) = self.m.mlm.as_ref().filter(|_| alts.len() >= 2) else { return Ok(alts[0].clone()) };
        let (s, a, b) = normalize(s, a, b);
        let subs = mlm.substitutes(&s, a, b, alts, 0, &[] as &[&str], alts.len())?;
        let value = |x: &String| {
            (score_of(&subs.scores, x).unwrap_or(-99.0) - subs.original) / (mlm.enc(&format!(" {x}")).len().max(1) as f64).sqrt()
        };
        let mut best = &alts[0];
        for x in &alts[1..] {
            if value(x) > value(best) {
                best = x;
            }
        }
        Ok(best.clone())
    }

    // -- sentences ------------------------------------------------------------------------------------------

    /// Replaces protected spans with placeholders ZQ1, ZQ2 ... (spaced so the model keeps them).
    fn mask(sentence: &str, spans: &[(usize, usize)]) -> (String, Vec<Slot>) {
        let (mut out, mut slots, mut pos) = (String::new(), Vec::new(), 0);
        for (i, &(s, e)) in spans.iter().enumerate() {
            out.push_str(&sentence[pos..s]);
            let prev = py::char_before(sentence, s);
            let next = py::char_at(sentence, e);
            let before = prev.is_some_and(py::is_space);
            let after = next.is_some_and(py::is_space);
            let tag = format!("ZQ{}", i + 1);
            if !(before || s == 0) {
                out.push(' ');
            }
            out.push_str(&tag);
            if !(after || e == sentence.len()) {
                out.push(' ');
            }
            slots.push(Slot {
                tag,
                orig: sentence[s..e].to_string(),
                before,
                after,
                prev_ch: prev.map(String::from).unwrap_or_default(),
                next_ch: next.map(String::from).unwrap_or_default(),
            });
            pos = e;
        }
        out.push_str(&sentence[pos..]);
        (out, slots)
    }

    /// Puts the protected spans back; None when the model dropped or duplicated one.
    fn unmask(text: &str, slots: &[Slot]) -> Option<String> {
        let mut text = text.to_string();
        for slot in slots {
            let body: Vec<String> = slot.tag.to_lowercase().chars().map(|c| regex::escape(&c.to_string())).collect();
            let pat = FancyRegex::new(&format!(r"(?i)(\s?){}(?!\d)(\s?)", body.join(r"\s?"))).ok()?;
            if pat.find_iter(&text).count() != 1 {
                return None;
            }
            let m = pat.captures(&text).ok()??;
            let whole = m.get(0)?;
            let (mut pre, mut post) = (m.get(1).map_or("", |g| g.as_str()), m.get(2).map_or("", |g| g.as_str()));
            // Drop only the spaces added here: when the neighbour is still the original one (e.g.
            // "Table~\ref{x})"), glue it back; otherwise keep the model's spacing.
            let before_match = py::char_before(&text, whole.start()).map(String::from).unwrap_or_default();
            if !pre.is_empty() && !slot.before && before_match == slot.prev_ch {
                pre = "";
            }
            let after_match = py::char_at(&text, whole.end()).map(String::from).unwrap_or_default();
            if !post.is_empty() && !slot.after && after_match == slot.next_ch {
                post = "";
            }
            text = format!("{}{pre}{}{post}{}", &text[..whole.start()], slot.orig, &text[whole.end()..]);
        }
        Some(text)
    }

    fn paraphrase(&self, sentence: &str, protected: &[(usize, usize)], n: usize, variation: u32) -> R<Vec<Suggestion>> {
        let lead = sentence.len() - sentence.trim_start_matches(py::is_space).len();
        let trail = sentence.trim_end_matches(py::is_space).len();
        let core = if lead < trail { &sentence[lead..trail] } else { "" };
        let spans = merge(
            protected
                .iter()
                .filter(|&&(s, e)| e > lead && s < trail)
                .map(|&(s, e)| (s.saturating_sub(lead), core.len().min(e - lead)))
                .collect(),
        );
        if WORD_RE.find_iter(core).count() < 3 {
            return Ok(Vec::new());
        }
        let Some(t5) = &self.m.t5 else { return self.lite_paraphrase(sentence, lead, trail, core, &spans, n) };
        let (masked, slots) = Self::mask(core, &spans);
        let opts = if variation > 0 {
            GenerateOptions { sample: true, seed: Some(variation as u64), temperature: 1.0, ..Default::default() }
        } else {
            GenerateOptions::default()
        };
        let gens = t5.generate(&masked, n, &opts)?;
        let (mut cands, mut seen) = (Vec::new(), HashSet::new());
        let stop = core.chars().last().filter(|c| ".!?".contains(*c));
        for g in gens {
            let mut g = py::strip(&g).to_string();
            if let Some(stop) = stop {
                if g.chars().last().is_some_and(|c| !".!?".contains(c)) {
                    g.push(stop);
                }
            }
            let restored = if slots.is_empty() { Some(g.clone()) } else { Self::unmask(&g, &slots) };
            let Some(restored) = restored.filter(|r| !r.is_empty()) else { continue };
            if seen.contains(&restored.to_lowercase()) || restored == core {
                continue;
            }
            seen.insert(restored.to_lowercase());
            cands.push((g, restored));
        }
        if cands.is_empty() {
            return Ok(Vec::new());
        }
        let sims = match &self.m.emb {
            Some(emb) => emb.similarity(&masked, &cands.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>())?,
            None => vec![0.9; cands.len()],
        };
        let mut out = Vec::new();
        for ((_, r), sim) in cands.into_iter().zip(sims) {
            if sim < 0.72 {
                continue;
            }
            let change = word_diff_ratio(core, &r);
            let fit = (((sim - 0.72) / 0.25).max(0.0) * 0.75 + change.min(0.8) * 0.3).min(1.0);
            let note = format!("meaning {:.0}% · {:.0}% reworded", sim * 100.0, change * 100.0);
            out.push(Suggestion::new(format!("{}{r}{}", &sentence[..lead], &sentence[trail..]), "rewrite", fit, note, sim));
        }
        out.sort_by(|a, b| b.fit.total_cmp(&a.fit));
        Ok(out)
    }

    fn lite_paraphrase(&self, sentence: &str, lead: usize, trail: usize, core: &str, spans: &[(usize, usize)], n: usize) -> R<Vec<Suggestion>> {
        let mut variants = Vec::new();
        let (text, text_spans) = self.apply_phrases(core, spans)?;
        if text != core {
            variants.push(text.clone());
        }
        for ratio in [0.3, 0.6] {
            let v = self.swap_words(&text, &text_spans, ratio, "strong")?;
            if v != text && !variants.contains(&v) {
                variants.push(v);
            }
        }
        Ok(variants
            .into_iter()
            .map(|v| {
                let note = format!("offline thesaurus · {:.0}% reworded", word_diff_ratio(core, &v) * 100.0);
                Suggestion::new(format!("{}{v}{}", &sentence[..lead], &sentence[trail..]), "rewrite", 0.5, note, 0.0)
            })
            .take(n)
            .collect())
    }

    // -- whole-text auto rewrite ------------------------------------------------------------------------------

    /// Replaces phrase-book expressions outside protected spans; returns the text and the shifted spans.
    fn apply_phrases(&self, cur: &str, spans: &[(usize, usize)]) -> R<(String, Vec<(usize, usize)>)> {
        let mut cur = cur.to_string();
        let mut cur_spans = spans.to_vec();
        let found: Vec<(usize, usize, String)> = phrases::find(&cur).into_iter().map(|(s, e, p)| (s, e, p.to_string())).collect();
        for (ps, pe, p) in found.into_iter().rev() {
            if overlaps(&cur_spans, ps, pe) {
                continue;
            }
            let alts: Vec<String> = phrases::lookup(&p)
                .into_iter()
                .filter(|x| x != "e.g.")
                .map(|x| if py::starts_upper(&p) { match_case(&p, &x) } else { x })
                .collect();
            if alts.is_empty() {
                continue;
            }
            let choice = self.choose_phrase(&cur, ps, pe, &alts)?;
            let old_len = pe - ps;
            cur.replace_range(ps..pe, &choice);
            for span in &mut cur_spans {
                if span.0 >= pe {
                    *span = (span.0 + choice.len() - old_len, span.1 + choice.len() - old_len);
                }
            }
        }
        Ok((cur, cur_spans))
    }

    fn eligible_words(sent: &str, spans: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let sent_start = sent.find(|c: char| !py::is_space(c)).unwrap_or(0);
        // "for instance" must not become "for case"
        let idioms: Vec<(usize, usize)> = phrases::find(sent).into_iter().map(|(s, e, _)| (s, e)).collect();
        let mut out = Vec::new();
        for m in WORD_RE.find_iter(sent) {
            let (w, a, b) = (m.as_str(), m.start(), m.end());
            let lw = w.to_lowercase();
            if w.len() < 4 || STOPWORDS.contains(lw.as_str()) || w.contains('\'') || py::is_upper(w) || NUMBER_WORDS.contains(lw.as_str()) {
                continue;
            }
            if py::starts_upper(w) && a != sent_start {
                continue; // probably a proper noun
            }
            if overlaps(spans, a, b) || overlaps(&idioms, a, b) {
                continue;
            }
            out.push((a, b));
        }
        out
    }

    /// Replaces up to `ratio` of the eligible words with their best context-fitting synonym.
    fn swap_words(&self, sent: &str, spans: &[(usize, usize)], ratio: f64, strength_name: &str) -> R<String> {
        let eligible = Self::eligible_words(sent, spans);
        if eligible.is_empty() {
            return Ok(sent.to_string());
        }
        let quota = ((eligible.len() as f64 * ratio).round_ties_even() as usize).max(1);
        let (_, min_score, min_sim) = strength(strength_name);
        let mut ranked: Vec<(f64, usize, usize, String)> = Vec::new();
        if let Some(mlm) = &self.m.mlm {
            for ((a, b), best) in self.word_swaps(mlm, sent, &eligible, 1, min_score, min_sim)? {
                if let Some((text, score)) = best.into_iter().next() {
                    ranked.push((score, a, b, text));
                }
            }
        } else {
            // Lite mode has no context model, so only take very safe swaps: an adjective/adverb synonym from one
            // of the word's two most common senses, a common word, same shape (-ly stays -ly).
            let t = self.t;
            let ctx: Vec<&str> = WORD_RE.find_iter(sent).map(|m| m.as_str()).filter(|w| !STOPWORDS.contains(w.to_lowercase().as_str())).collect();
            for &(a, b) in &eligible {
                let w = &sent[a..b];
                let pos = guess_pos(sent, a, b);
                let ly = w.to_lowercase().ends_with("ly");
                let cands: Vec<Candidate> = t
                    .candidates(w, 8, false, 30)
                    .into_iter()
                    .filter(|c| {
                        !c.text.contains(' ')
                            && !c.text.contains('-')
                            && c.sense_rank <= 1
                            && (c.pos == "a" || c.pos == "r") // nouns and verbs are too ambiguous without a model
                            && t.zipf(&c.text) >= 4.0
                            && pos.is_none_or(|p| p.contains(&wn_to_upos(c.pos)))
                            && ly == c.text.ends_with("ly")
                    })
                    .collect();
                if !cands.is_empty() {
                    let best = t.lesk_rank(w, &ctx, cands).swap_remove(0);
                    let score = best.score.unwrap_or(0.0);
                    if score >= 3.0 {
                        ranked.push((score, a, b, match_case(w, &best.text)));
                    }
                }
            }
        }
        ranked.sort_by(|x, y| y.0.total_cmp(&x.0).then(y.1.cmp(&x.1)).then(y.2.cmp(&x.2)).then(y.3.cmp(&x.3)));
        let (mut chosen, mut taken): (Vec<(usize, usize, String)>, Vec<(usize, usize)>) = (Vec::new(), Vec::new());
        for (_, a, b, text) in ranked {
            if chosen.len() >= quota {
                break;
            }
            if taken.iter().any(|&(x, y)| char_gap(sent, a, y) < 2 || char_gap(sent, x, b) < 2) {
                continue; // don't change two neighbouring words
            }
            chosen.push((a, b, text));
            taken.push((a, b));
        }
        chosen.sort_by(|x, y| y.cmp(x));
        let mut out = sent.to_string();
        for (a, b, text) in chosen {
            out.replace_range(a..b, &text);
        }
        Ok(out)
    }

    fn auto_rewrite(
        &self,
        text: &str,
        regions: &[(usize, usize)],
        protected: &[(usize, usize)],
        opts: &AutoOptions,
        progress: &dyn Fn(usize, usize),
        cancelled: &dyn Fn() -> bool,
    ) -> Vec<Edit> {
        let ratio = strength(&opts.strength).0;
        let mut sentences = Vec::new();
        for &(rs, re) in regions {
            sentences.extend(sentence_bounds(&text[rs..re], opts.lines).into_iter().map(|(s, e)| (rs + s, rs + e)));
        }
        let mut edits = Vec::new();
        for (k, &(s, e)) in sentences.iter().enumerate() {
            if cancelled() {
                break;
            }
            progress(k, sentences.len());
            let sent = &text[s..e];
            let spans = clip(protected, s, e);
            let prose = WORD_RE.find_iter(sent).filter(|m| !overlaps(&spans, m.start(), m.end())).count();
            if prose < 3 {
                continue;
            }
            let rewrite = || -> R<(String, bool)> {
                if opts.sentences && self.m.t5.is_some() && prose >= 6 && spans.len() <= 6 {
                    if let Some(sug) = self.paraphrase(sent, &spans, 4, 0)?.into_iter().find(|s| s.score >= 0.9) {
                        return Ok((sug.text, true)); // the meaning similarity is high enough
                    }
                }
                let (mut cur, mut cur_spans) = (sent.to_string(), spans.clone());
                if opts.phrases {
                    (cur, cur_spans) = self.apply_phrases(&cur, &cur_spans)?;
                }
                if opts.words {
                    cur = self.swap_words(&cur, &cur_spans, ratio, &opts.strength)?;
                }
                Ok((cur, false))
            };
            match rewrite() {
                Ok((new, whole)) if !new.is_empty() && new != sent => edits.push((s, e, new, whole)),
                Ok(_) => {}
                Err(err) => eprintln!("rewrite engine: {err}"),
            }
        }
        progress(sentences.len(), sentences.len());
        edits
    }

    // -- proofreading ---------------------------------------------------------------------------------------

    fn spell_suggest(&self, text: &str, start: usize, end: usize, n: usize) -> R<Vec<Suggestion>> {
        let (s, a, b) = normalize(text, start, end);
        let cands = self.e.speller.suggest(&s[a..b], 10);
        let fits = match &self.m.mlm {
            Some(mlm) if !cands.is_empty() => {
                let single: Vec<String> = cands.iter().filter(|(t, _)| !t.contains(' ')).map(|(t, _)| t.clone()).collect();
                mlm.fill(&s, &[(a, b, single)], 0, crate::mlm::MAX_SIDE)?.swap_remove(0).0
            }
            _ => Vec::new(),
        };
        let smart = self.m.mlm.is_some();
        let mut out: Vec<Suggestion> = cands
            .into_iter()
            .map(|(text, prior)| {
                let gloss = if text.contains(' ') { "" } else { self.t.senses(&text).first().map_or("", |s| s.2) };
                let lp = score_of(&fits, &text).map_or(-7.0, |f| f.max(-14.0));
                let score = prior + if smart { 0.45 * lp } else { 0.0 };
                Suggestion::new(text, "spelling", 0.0, gloss, score)
            })
            .collect();
        calibrate(&mut out, if smart { 0.0 } else { 2.0 }, 2.0);
        by_score_desc(&mut out);
        out.truncate(n);
        Ok(out)
    }

    /// The sentence as plain prose for the context model: commands and braces vanish, other protected pieces
    /// (maths, references, code, links) become "X". Returns the prose and its pieces of the sentence as
    /// (prose offset, sentence offset, length).
    fn prose(sent: &str, spans: &[(usize, usize)]) -> (String, Vec<(usize, usize, usize)>) {
        let (mut out, mut segs, mut pos) = (String::new(), Vec::new(), 0);
        for &(s, e) in spans.iter().chain([&(sent.len(), sent.len())]) {
            if s > pos {
                segs.push((out.len(), pos, s - pos));
                out.push_str(&sent[pos..s]);
            }
            let p = &sent[s..e.max(s)];
            let second = p.chars().nth(1);
            let fill = if p == "~" {
                " "
            } else if p.is_empty()
                || "{}[]&^_".contains(p)
                || p.starts_with(['<', '%'])
                || (p.starts_with('\\') && !REF_CMD.is_match(p) && !matches!(second, Some('(' | '[')))
            {
                ""
            } else {
                "X"
            };
            out.push_str(fill);
            pos = pos.max(e);
        }
        (out, segs)
    }

    /// Every word of a sentence that has sibling forms, with the best-fitting sibling.
    fn slip_scores(&self, mlm: &MaskedLm, sent: &str, spans: &[(usize, usize)]) -> R<Vec<SlipScore>> {
        let (prose, segs) = Self::prose(sent, spans);
        let words: Vec<regex::Match> = WORD_RE.find_iter(&prose).collect();
        if !(4..=80).contains(&words.len()) {
            return Ok(Vec::new());
        }
        let first = words[0].start();
        let (mut targets, mut meta) = (Vec::new(), Vec::new());
        for (k, m) in words.iter().enumerate() {
            let (w, a, b) = (m.as_str(), m.start(), m.end());
            let seg = segs.iter().find(|&&(pp, _, ln)| pp <= a && b <= pp + ln);
            let Some(&(pp, sp, _)) = seg else { continue }; // inside a placeholder
            if py::starts_upper(w) && (a != first || py::is_upper(w)) {
                continue; // a name, an acronym
            }
            let prev = if k > 0 { words[k - 1].as_str().to_lowercase() } else { String::new() };
            let alts = self.e.alternatives(w, &prev);
            if alts.is_empty() {
                continue;
            }
            // Both numbers of a noun are usually fine; only check one that a nearby word contradicts ("three
            // experiment", "each players").
            let near = |n: usize| -> HashSet<String> { words[k.saturating_sub(n)..k].iter().map(|x| x.as_str().to_lowercase()).collect() };
            let digits = DIGITS_BEFORE.find(&prose[..a]).is_some_and(|d| py::split_ws(d.as_str()).next() != Some("1"));
            let many = digits || near(3).iter().any(|x| PLURAL_CUES.contains(&x.as_str()));
            let lone = near(2).iter().any(|x| SINGULAR_CUES.contains(&x.as_str()));
            let mut skip: Vec<&str> = Vec::new();
            if !many {
                skip.push("plural");
            }
            if !lone {
                skip.push("singular");
            }
            if a == first {
                // "Keeps the meaning." - a note without a subject is not an agreement slip
                skip.extend(["agree", "form", "singular", "plural"]);
            }
            let mut cased: Alternatives = Vec::new();
            for (x, kinds) in &alts {
                let key = match_case(w, x);
                let kinds: Vec<&'static str> = kinds.iter().copied().filter(|kd| !skip.contains(kd)).collect();
                match cased.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = kinds,
                    None => cased.push((key, kinds)),
                }
            }
            cased.retain(|(_, kinds)| !kinds.is_empty());
            if !cased.is_empty() {
                let mut cands = vec![w.to_string()];
                cands.extend(cased.iter().map(|(x, _)| x.clone()));
                targets.push((a, b, cands));
                meta.push((k, a + sp - pp, w.to_string(), cased));
            }
        }
        targets.truncate(40);
        let mut out = Vec::new();
        for ((k, start, w, cased), (fits, guesses)) in meta.into_iter().zip(mlm.fill(&prose, &targets, 15, 32)?) {
            let Some(own) = score_of(&fits, &w) else { continue };
            // The model's own guesses say whether a verb or a noun belongs here, so "the base classes" is not
            // read as a verb that should agree with something.
            let share = pos_share(&guesses);
            let mut best: Option<(String, &'static str, f64)> = None;
            for (x, kinds) in &cased {
                let kind = kinds.iter().copied().find(|kd| needs_pos(kd).map_or(1.0, |p| share.get(p).copied().unwrap_or(1.0)) >= 0.5);
                let (Some(kind), Some(fx)) = (kind, score_of(&fits, x)) else { continue };
                if best.as_ref().is_none_or(|b| fx > b.2) {
                    best = Some((x.clone(), kind, fx));
                }
            }
            if let Some((alt, kind, other)) = best {
                out.push((other - own, k, start, w, alt, kind, own, other));
            }
        }
        Ok(out)
    }

    fn grammar(&self, sent: &str, spans: &[(usize, usize)]) -> R<Vec<Slip>> {
        let Some(mlm) = &self.m.mlm else { return Ok(Vec::new()) };
        let mut slips = self.slip_scores(mlm, sent, spans)?;
        slips.sort_by(|x, y| {
            y.0.total_cmp(&x.0)
                .then(y.1.cmp(&x.1))
                .then(y.2.cmp(&x.2))
                .then(y.3.cmp(&x.3))
                .then(y.4.cmp(&x.4))
                .then(y.5.cmp(x.5))
                .then(y.6.total_cmp(&x.6))
                .then(y.7.total_cmp(&x.7))
        });
        let (mut out, mut taken): (Vec<Slip>, Vec<usize>) = (Vec::new(), Vec::new());
        for (gain, k, start, w, alt, kind, own, other) in slips {
            let (need, msg) = slip_rule(kind);
            // a much better fit may itself be a rarer word; contractions always score lower (two pieces)
            let floor = MIN_ALT_FIT - (0.5 * (gain - need)).min(2.0) - if alt.contains('\'') { 2.5 } else { 0.0 };
            if gain < need || own > MAX_OWN_FIT || other < floor {
                continue;
            }
            // one agreement slip makes both the subject and the verb look wrong: keep the clearer one
            if kind != "confused" {
                if taken.iter().any(|&j| k.abs_diff(j) <= 3) {
                    continue;
                }
                taken.push(k);
            }
            let message = msg.replace("{alt}", &alt).replace("{word}", &w);
            out.push((start, start + w.len(), message, vec![alt]));
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_remaps() {
        assert_eq!(normalize("a  b\n\tc", 3, 4), ("a b c".to_string(), 2, 3));
        assert_eq!(normalize("ab", 2, 2), ("ab".to_string(), 2, 2));
    }

    #[test]
    fn mask_round_trip() {
        let s = r"See Table~\ref{x}) and $a+b$ here.";
        let a = s.find(r"\ref").unwrap();
        let b = s.find(')').unwrap();
        let c = s.find('$').unwrap();
        let spans = [(a, b), (c, c + 5)];
        let (masked, slots) = Ctx::mask(s, &spans);
        assert_eq!(masked, "See Table~ ZQ1 ) and ZQ2 here.");
        assert_eq!(Ctx::unmask(&masked, &slots).unwrap(), s);
        assert_eq!(Ctx::unmask("See zq 1 and ZQ2 then ZQ2.", &slots), None);
    }

    #[test]
    fn prose_placeholders() {
        let s = r"We use \cite{a} and \textbf{b} ~ here";
        let c = s.find(r"\cite").unwrap();
        let t = s.find(r"\textbf").unwrap();
        let tilde = s.find('~').unwrap();
        let (p, _) = Ctx::prose(s, &[(c, c + 8), (t, t + 8), (tilde, tilde + 1)]);
        assert_eq!(p, "We use X and b}   here");
    }

    #[test]
    fn engine_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Engine>();
    }
}
