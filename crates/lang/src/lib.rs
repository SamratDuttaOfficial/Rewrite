//! Text utilities, diff, phrase book, thesaurus, inflection, spelling and grammar rules for Rewrite.
//!
//! Ported from the Python modules `textutil`, `phrases`, `thesaurus` and `proof`, from difflib and from
//! lemminflect, with parity tests against what those returned (`tests/fixtures`). Offsets are byte offsets.
//! The data (phrase book, word lists, thesaurus, lemminflect tables and networks) is built into the library.

pub mod diff;
pub mod grammar;
pub mod inflect;
pub mod phrases;
pub mod py;
pub mod speller;
pub mod text;
pub mod thesaurus;

pub use grammar::{dedupe, grammar_rules, Issue};
pub use speller::Speller;
pub use thesaurus::{Candidate, Thesaurus};

#[cfg(test)]
mod tests {
    #[test]
    fn shared_across_threads() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<super::Speller>();
        send_sync::<super::Thesaurus>();
        send_sync::<super::Candidate>();
        send_sync::<super::Issue>();
        send_sync::<super::phrases::PhraseBook>();
        send_sync::<super::inflect::Forms>();
    }
}
