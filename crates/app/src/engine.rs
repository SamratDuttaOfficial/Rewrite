//! The language side as the window sees it. Every offset here counts characters, as GTK text positions
//! do (an embedded object is one U+FFFC); the libraries underneath work in bytes, and the conversion
//! happens in this one place.

use std::sync::Arc;

/// A suggestion for a word, phrase or sentence.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub text: String,
    /// synonym | related | in context | phrase | rewrite | spelling
    pub kind: String,
    /// 0..1, for the bar beside it
    pub fit: f64,
    pub note: String,
}

/// A proofreading remark: spelling (`kind` "spell") or grammar ("gram").
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub start: usize,
    pub end: usize,
    pub kind: String,
    pub message: String,
    pub fixes: Vec<String>,
}

/// A grammar slip found by the context model in one sentence: (start, end, message, fixes), sentence offsets.
pub type Slip = (usize, usize, String, Vec<String>);

/// Options of one auto-rewrite pass.
#[derive(Clone, Debug)]
pub struct AutoOptions {
    pub strength: String,
    pub phrases: bool,
    pub words: bool,
    pub sentences: bool,
    pub lines: bool,
}

/// One edit from auto-rewrite: (start, end, new text, whole sentence).
pub type Edit = (usize, usize, String, bool);

/// Character offset <-> byte offset in one text.
pub struct Offsets {
    /// byte position of every character, and the length at the end
    bytes: Vec<u32>,
}

impl Offsets {
    pub fn new(text: &str) -> Offsets {
        let mut bytes: Vec<u32> = text.char_indices().map(|(b, _)| b as u32).collect();
        bytes.push(text.len() as u32);
        Offsets { bytes }
    }
    pub fn byte(&self, ch: usize) -> usize {
        self.bytes[ch.min(self.bytes.len() - 1)] as usize
    }
    pub fn char(&self, byte: usize) -> usize {
        match self.bytes.binary_search(&(byte as u32)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }
    pub fn len(&self) -> usize {
        self.bytes.len() - 1
    }
}

/// `text[s..e]` in characters.
pub fn slice(text: &str, off: &Offsets, s: usize, e: usize) -> String {
    let (a, b) = (off.byte(s), off.byte(e.max(s)));
    text[a..b].to_string()
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<rewrite_nlp::engine::Engine>,
}

impl Engine {
    pub fn new(user_words: &[String]) -> Engine {
        Engine { inner: Arc::new(rewrite_nlp::engine::Engine::new(user_words)) }
    }

    pub fn load_models(&self) {
        self.inner.load_models();
    }

    /// The context model is loaded.
    pub fn smart(&self) -> bool {
        self.inner.smart()
    }
    pub fn has_meaning_check(&self) -> bool {
        self.inner.has_emb()
    }
    pub fn has_paraphraser(&self) -> bool {
        self.inner.has_t5()
    }
    pub fn has_context_model(&self) -> bool {
        self.inner.smart()
    }

    pub fn suggest(&self, text: &str, s: usize, e: usize, n: usize, generate: bool) -> Vec<Suggestion> {
        let off = Offsets::new(text);
        self.inner.suggest(text, off.byte(s), off.byte(e), n, generate).into_iter().map(conv).collect()
    }

    pub fn spell_suggest(&self, text: &str, s: usize, e: usize, n: usize) -> Vec<Suggestion> {
        let off = Offsets::new(text);
        self.inner.spell_suggest(text, off.byte(s), off.byte(e), n).into_iter().map(conv).collect()
    }

    /// Rewrites of a sentence; `protected` are character spans inside it.
    pub fn paraphrase(&self, sentence: &str, protected: &[(usize, usize)], n: usize, variation: u32) -> Vec<Suggestion> {
        let off = Offsets::new(sentence);
        let spans: Vec<(usize, usize)> = protected.iter().map(|&(a, b)| (off.byte(a), off.byte(b))).collect();
        self.inner.paraphrase(sentence, &spans, n, variation).into_iter().map(conv).collect()
    }

    pub fn grammar(&self, sentence: &str, protected: &[(usize, usize)]) -> Vec<Slip> {
        let off = Offsets::new(sentence);
        let spans: Vec<(usize, usize)> = protected.iter().map(|&(a, b)| (off.byte(a), off.byte(b))).collect();
        self.inner
            .grammar(sentence, &spans)
            .into_iter()
            .map(|(a, b, msg, fixes)| (off.char(a), off.char(b), msg, fixes))
            .collect()
    }

    pub fn auto_rewrite(
        &self,
        text: &str,
        regions: &[(usize, usize)],
        protected: &[(usize, usize)],
        opts: &AutoOptions,
        progress: &dyn Fn(usize, usize),
        cancelled: &dyn Fn() -> bool,
    ) -> Vec<Edit> {
        let off = Offsets::new(text);
        let b = |v: &[(usize, usize)]| -> Vec<(usize, usize)> { v.iter().map(|&(x, y)| (off.byte(x), off.byte(y))).collect() };
        let o = rewrite_nlp::engine::AutoOptions {
            strength: opts.strength.clone(),
            phrases: opts.phrases,
            words: opts.words,
            sentences: opts.sentences,
            lines: opts.lines,
        };
        self.inner
            .auto_rewrite(text, &b(regions), &b(protected), &o, progress, cancelled)
            .into_iter()
            .map(|(s, e, new, whole)| (off.char(s), off.char(e), new, whole))
            .collect()
    }

    /// Spelling slips as character spans.
    pub fn spell_check(&self, text: &str, protected: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let off = Offsets::new(text);
        let spans: Vec<(usize, usize)> = protected.iter().map(|&(a, b)| (off.byte(a), off.byte(b))).collect();
        self.inner.speller().check(text, &spans).into_iter().map(|(a, b)| (off.char(a), off.char(b))).collect()
    }

    /// The rule-based grammar remarks.
    pub fn grammar_rules(&self, text: &str, protected: &[(usize, usize)]) -> Vec<Issue> {
        let off = Offsets::new(text);
        let spans: Vec<(usize, usize)> = protected.iter().map(|&(a, b)| (off.byte(a), off.byte(b))).collect();
        rewrite_lang::grammar::grammar_rules(text, &spans, Some(self.inner.speller()))
            .into_iter()
            .map(|it| Issue {
                start: off.char(it.start),
                end: off.char(it.end),
                kind: it.kind.to_string(),
                message: it.message.clone(),
                fixes: it.fixes.clone(),
            })
            .collect()
    }

    pub fn add_word(&self, word: &str, session: bool) {
        self.inner.speller().add(word, session);
    }

    /// The engine as a helper for PDF conversion and sentence splitting in exports.
    pub fn helper(&self) -> rewrite_nlp::helper::Helper {
        rewrite_nlp::helper::Helper::new(self.inner.clone())
    }

}

fn conv(s: rewrite_nlp::engine::Suggestion) -> Suggestion {
    Suggestion { text: s.text, kind: s.kind.to_string(), fit: s.fit, note: s.note }
}

/// Sort and drop overlapping remarks.
pub fn dedupe(mut issues: Vec<Issue>) -> Vec<Issue> {
    issues.sort_by_key(|i| (i.start, i.end));
    let mut out: Vec<Issue> = Vec::new();
    for it in issues {
        if out.last().is_some_and(|l| it.start < l.end) {
            continue;
        }
        out.push(it);
    }
    out
}

// -- text utilities, in characters ------------------------------------------------------------

/// Sentence spans.
pub fn sentence_bounds(text: &str, lines: bool) -> Vec<(usize, usize)> {
    let off = Offsets::new(text);
    rewrite_lang::text::sentence_bounds(text, lines).into_iter().map(|(a, b)| (off.char(a), off.char(b))).collect()
}

/// Word spans (start, end).
pub fn words(text: &str) -> Vec<(usize, usize)> {
    let off = Offsets::new(text);
    rewrite_lang::text::words(text).into_iter().map(|(a, b, _)| (off.char(a), off.char(b))).collect()
}

pub fn word_count(text: &str) -> usize {
    rewrite_lang::text::words(text).len()
}

/// Word-level changes against the original: ([(cur_s, cur_e, orig_s, orig_e)], share of words changed).
pub fn compute_changes(orig: &str, cur: &str) -> (Vec<(usize, usize, usize, usize)>, f64) {
    let (oa, ob) = (Offsets::new(orig), Offsets::new(cur));
    let (ch, ratio) = rewrite_lang::diff::compute_changes(orig, cur);
    (ch.into_iter().map(|c| (ob.char(c.cur_start), ob.char(c.cur_end), oa.char(c.orig_start), oa.char(c.orig_end))).collect(), ratio)
}

/// The minimal changed pieces between two versions: [(old_start, old_end, replacement)].
pub fn diff_chunks(old: &str, new: &str) -> Vec<(usize, usize, String)> {
    let off = Offsets::new(old);
    rewrite_lang::diff::diff_chunks(old, new).into_iter().map(|(a, b, r)| (off.char(a), off.char(b), r)).collect()
}

/// Phrase-book matches.
pub fn phrase_spans(text: &str) -> Vec<(usize, usize)> {
    let off = Offsets::new(text);
    rewrite_lang::phrases::find(text).into_iter().map(|(a, b, _)| (off.char(a), off.char(b))).collect()
}

pub fn phrase_known(phrase: &str) -> bool {
    !rewrite_lang::phrases::lookup(phrase).is_empty()
}

/// Protected spans of a text of some kind (latex, markdown, html, code, plain).
pub fn protected_spans(text: &str, kind: &str, path: Option<&str>) -> Vec<(usize, usize)> {
    let off = Offsets::new(text);
    rewrite_core::protect::protected_spans(text, kind, path.map(std::path::Path::new)).into_iter().map(|(a, b)| (off.char(a), off.char(b))).collect()
}

pub fn merge(mut spans: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    spans.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Whether any span overlaps [s, e).
pub fn overlaps(spans: &[(usize, usize)], s: usize, e: usize) -> bool {
    let i = spans.partition_point(|&(_, b)| b <= s);
    spans.get(i).is_some_and(|&(a, _)| a < e.max(s + 1))
}

/// The spans inside [s, e), relative to s.
pub fn clip(spans: &[(usize, usize)], s: usize, e: usize) -> Vec<(usize, usize)> {
    let i = spans.partition_point(|&(_, b)| b <= s);
    spans[i..]
        .iter()
        .take_while(|&&(a, _)| a < e)
        .map(|&(a, b)| (a.max(s) - s, b.min(e) - s))
        .filter(|(a, b)| b > a)
        .collect()
}
