//! The language side of conversions (was `rewrite/cleanup.py`): answers the core's doubtful cases (is
//! "co-operate" one word or two? does a sentence end after "al."? is "thequick" two words?) with the spelling
//! dictionary and, when it is loaded, the context model; and splits paragraphs into sentences for "one sentence
//! per line".

use crate::{Engine, MaskedLm};
use rewrite_core::textclean;
use rewrite_lang::py;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Passed to the PDF conversion and to [`textclean::sentence_starts`]. Answers are cached.
pub struct Helper {
    engine: Arc<Engine>,
    /// The context model loaded when the helper was made.
    model: Option<Arc<MaskedLm>>,
    known: Mutex<HashMap<String, bool>>,
    zipf: Mutex<HashMap<String, f64>>,
    choose: Mutex<HashMap<(String, Vec<String>, String), Option<usize>>>,
}

impl Helper {
    pub fn new(engine: Arc<Engine>) -> Helper {
        let model = engine.masked_lm();
        Helper { engine, model, known: Mutex::default(), zipf: Mutex::default(), choose: Mutex::default() }
    }

    /// Byte offsets where sentences start (for writers' "one sentence per line").
    pub fn sentences(&self, text: &str) -> Vec<usize> {
        textclean::sentence_starts(text, self)
    }
}

impl textclean::Helper for Helper {
    fn known(&self, word: &str) -> bool {
        let w = word.to_lowercase();
        if let Some(&hit) = self.known.lock().unwrap().get(&w) {
            return hit;
        }
        let hit = self.engine.speller().known(&w);
        self.known.lock().unwrap().insert(w, hit);
        hit
    }

    fn zipf(&self, word: &str) -> f64 {
        let w = word.to_lowercase();
        if let Some(&hit) = self.zipf.lock().unwrap().get(&w) {
            return hit;
        }
        let z = if self.known(&w) { self.engine.speller().zipf(&w) } else { 0.0 };
        self.zipf.lock().unwrap().insert(w, z);
        z
    }

    /// The option the context model finds clearly more likely here (by at least 1.0 in log-probability).
    fn choose(&self, before: &str, options: &[String], after: &str) -> Option<usize> {
        let model = self.model.as_ref()?;
        if options.is_empty() {
            return None;
        }
        let key = (py::last_chars(before, 120).to_string(), options.to_vec(), after[..py::ahead(after, 0, 120)].to_string());
        if let Some(&hit) = self.choose.lock().unwrap().get(&key) {
            return hit;
        }
        let (before, after) = (py::last_chars(before, 300), &after[..py::ahead(after, 0, 300)]);
        let sep = if before.is_empty() || before.ends_with([' ', '\n']) { "" } else { " " };
        let gap = if !after.is_empty() && !after.starts_with([' ', '.', ',']) { " " } else { "" };
        let text = format!("{before}{sep}{}{gap}{after}", options[0]);
        let start = before.len() + sep.len();
        let scores = model
            .fill(&text, &[(start, start + options[0].len(), options.to_vec())], 0, crate::mlm::MAX_SIDE)
            .map(|mut r| r.swap_remove(0).0)
            .unwrap_or_default();
        let mut ranked: Vec<(f64, usize)> = options
            .iter()
            .enumerate()
            .map(|(i, o)| (scores.iter().find(|(k, _)| k == o).map_or(-99.0, |s| s.1), i))
            .collect();
        ranked.sort_by(|x, y| y.0.total_cmp(&x.0).then(y.1.cmp(&x.1)));
        // only a clear preference counts
        let pick = (ranked[0].0 > -99.0 && (ranked.len() == 1 || ranked[0].0 - ranked[1].0 >= 1.0)).then_some(ranked[0].1);
        self.choose.lock().unwrap().insert(key, pick);
        pick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Helper>();
    }

    #[test]
    fn without_a_model_it_only_knows_words() {
        use textclean::Helper as _;
        let h = Helper::new(Arc::new(Engine::new(&["rewriteword"])));
        assert!(h.known("Thesaurus"));
        assert!(h.known("rewriteword"));
        assert!(!h.known("qzxv"));
        assert_eq!(h.zipf("the"), 6.0);
        assert_eq!(h.zipf("qzxv"), 0.0);
        assert_eq!(h.choose("The method is ", &["effective".into(), "eff ective".into()], " in practice."), None);
    }
}
