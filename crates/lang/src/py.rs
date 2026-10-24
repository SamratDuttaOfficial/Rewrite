//! Python's string semantics where the ported code depends on them: `str.isspace`, the `\w` class, `isupper`,
//! `capitalize` and character slicing. Offsets elsewhere are byte offsets; these helpers keep the character
//! counts Python used.

use regex::Regex;
use std::sync::LazyLock;

/// `str.isspace` for one character (also Python's `\s`): Unicode white space plus the separators U+001C..U+001F.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// A character of Python's `\w`: a letter, a number or `_`.
pub fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    static W: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}]$").unwrap());
    W.is_match(c.encode_utf8(&mut [0; 4]))
}

/// `str.isalpha` for one character (a letter).
pub fn is_alpha(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    static L: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").unwrap());
    L.is_match(c.encode_utf8(&mut [0; 4]))
}

/// `str.isdecimal` for one character (Python's `\d`).
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    static D: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").unwrap());
    D.is_match(c.encode_utf8(&mut [0; 4]))
}

/// `str.isdigit` for one character: decimal digits plus the superscript and subscript digits.
pub fn is_digit(c: char) -> bool {
    is_decimal(c) || matches!(c, '²' | '³' | '¹' | '⁰' | '⁴'..='⁹' | '₀'..='₉')
}

/// `str.isalnum` for one character.
pub fn is_alnum(c: char) -> bool {
    c != '_' && is_word(c)
}

/// `str.isupper`: at least one upper-case letter and no lower-case one.
pub fn is_upper(s: &str) -> bool {
    let mut cased = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        cased |= c.is_uppercase();
    }
    cased
}

/// `str.islower`: at least one lower-case letter and no upper-case one.
pub fn is_lower(s: &str) -> bool {
    let mut cased = false;
    for c in s.chars() {
        if c.is_uppercase() {
            return false;
        }
        cased |= c.is_lowercase();
    }
    cased
}

/// The first character is upper case (`s[:1].isupper()`).
pub fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

/// The first character is lower case (`s[:1].islower()`).
pub fn starts_lower(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_lowercase)
}

/// `str.capitalize`: the first character upper case, the rest lower case.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars.as_str().to_lowercase().chars()).collect(),
        None => String::new(),
    }
}

/// `s[:1].upper() + s[1:]`.
pub fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `str.strip()` with Python's notion of white space.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.split()`: runs of white space separate, empty pieces are dropped.
pub fn split_ws(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|p| !p.is_empty())
}

/// Byte offset `n` characters before byte offset `b` (or 0): Python's `text[b - n:b]` start.
pub fn back(s: &str, b: usize, n: usize) -> usize {
    if n == 0 {
        return b;
    }
    s[..b].char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i)
}

/// Byte offset `n` characters after byte offset `b` (or the end): Python's `text[b:b + n]` end.
pub fn ahead(s: &str, b: usize, n: usize) -> usize {
    s[b..].char_indices().nth(n).map_or(s.len(), |(i, _)| b + i)
}

/// `s[:-n]` in characters (empty when `n` is at least the length; all of `s` when `n` is 0).
pub fn drop_last(s: &str, n: usize) -> &str {
    if n == 0 {
        return s;
    }
    &s[..s.char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i)]
}

/// `s[-n:]` in characters.
pub fn last_chars(s: &str, n: usize) -> &str {
    &s[drop_last(s, n).len()..]
}

/// The character before byte offset `b`, if any.
pub fn char_before(s: &str, b: usize) -> Option<char> {
    s[..b].chars().next_back()
}

/// The character at byte offset `b`, if any.
pub fn char_at(s: &str, b: usize) -> Option<char> {
    s[b..].chars().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_semantics() {
        assert!(is_space('\u{1f}') && is_space('\u{a0}') && !is_space('x'));
        assert!(is_word('é') && is_word('²') && is_word('_') && !is_word('-') && !is_word('\u{301}'));
        assert!(is_upper("WORD1") && !is_upper("Word") && !is_upper("123"));
        assert_eq!(capitalize("mcDONALD"), "Mcdonald");
        assert_eq!(drop_last("héllo", 2), "hél");
        assert_eq!(drop_last("ab", 5), "");
        assert_eq!(last_chars("héllo", 4), "éllo");
        let s = "aé€b";
        assert_eq!(back(s, s.len(), 2), 3);
        assert_eq!(back(s, s.len(), 9), 0);
        assert_eq!(ahead(s, 1, 2), 6);
        assert_eq!(ahead(s, 1, 9), s.len());
    }
}
