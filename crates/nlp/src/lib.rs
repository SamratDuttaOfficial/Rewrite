//! Model downloads, ONNX Runtime models and the suggestion engine for Rewrite.
//!
//! Offsets taken and returned by the models are byte offsets into the text, as everywhere in the Rust code
//! (the Python version counted characters). All model types are `Send + Sync`: the app runs them on worker
//! threads, and each model runs one call at a time.

pub mod download;
pub mod embed;
pub mod engine;
pub mod helper;
pub mod mlm;
pub mod onnx;
pub mod t5;

pub use embed::Embedder;
pub use engine::{AutoOptions, Engine, Suggestion};
pub use mlm::MaskedLm;
pub use t5::{GenerateOptions, Paraphraser};

/// The tokenizer in a model folder.
fn tokenizer(folder: &std::path::Path) -> Result<tokenizers::Tokenizer, String> {
    let path = folder.join("tokenizer.json");
    tokenizers::Tokenizer::from_file(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Python's `str.isspace` (Unicode White_Space plus the separators U+001C..U+001F), used where the Python code
/// relied on `\s` or `strip()`.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// The byte offset `n` characters before byte offset `i` (or 0): Python's `s[max(0, i - n):i]` window.
fn chars_back(s: &str, i: usize, n: usize) -> usize {
    if n == 0 {
        return i;
    }
    s[..i].char_indices().rev().nth(n - 1).map_or(0, |(b, _)| b)
}

/// The byte offset `n` characters after byte offset `i` (or the end).
fn chars_ahead(s: &str, i: usize, n: usize) -> usize {
    s[i..].char_indices().nth(n).map_or(s.len(), |(b, _)| i + b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_windows() {
        let s = "aé€bc";
        assert_eq!(chars_back(s, 6, 2), 1);
        assert_eq!(chars_back(s, 6, 9), 0);
        assert_eq!(chars_back(s, 6, 0), 6);
        assert_eq!(chars_ahead(s, 1, 2), 6);
        assert_eq!(chars_ahead(s, 1, 99), s.len());
    }
}
