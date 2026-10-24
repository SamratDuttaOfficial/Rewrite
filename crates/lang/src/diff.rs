//! Diffs: a port of Python's `difflib.SequenceMatcher` (Ratcliff-Obershelp), and the word and token diffs
//! the editor shows ("% reworded", changed words, the changed pieces of a rewritten sentence).
//!
//! The matcher has to be the same algorithm, not just any minimal diff: the editor's numbers and highlights
//! were tuned on what difflib returns.

use crate::py;
use crate::text::WORD_RE;
use std::collections::HashMap;
use std::hash::Hash;

/// A run of `size` equal elements at `a[a..]` and `b[b..]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub a: usize,
    pub b: usize,
    pub size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

/// `a[i1..i2]` becomes `b[j1..j2]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opcode {
    pub tag: Tag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

/// difflib's `SequenceMatcher(None, a, b, autojunk)`: no junk function.
pub struct SequenceMatcher<'s, T> {
    a: &'s [T],
    b: &'s [T],
    /// Positions of each element of `b`, ascending (popular elements removed when `autojunk`).
    b2j: HashMap<&'s T, Vec<usize>>,
}

impl<'s, T: Hash + Eq> SequenceMatcher<'s, T> {
    /// With `autojunk`, elements of a `b` of 200 or more that make up over 1% of it are ignored when looking
    /// for matches, as in difflib. Rewrite always passes `false`.
    pub fn new(a: &'s [T], b: &'s [T], autojunk: bool) -> Self {
        let mut b2j: HashMap<&T, Vec<usize>> = HashMap::new();
        for (i, x) in b.iter().enumerate() {
            b2j.entry(x).or_default().push(i);
        }
        if autojunk && b.len() >= 200 {
            let ntest = b.len() / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        SequenceMatcher { a, b, b2j }
    }

    /// The longest matching block in `a[alo..ahi]` and `b[blo..bhi]`; of several, the one starting earliest
    /// in `a`, then earliest in `b`.
    pub fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        // j2len[j - blo + 1]: length of the match ending at a[i - 1], b[j] (row i - 1). Touched entries are
        // reset before the next row, so this is difflib's dict of the previous row.
        let mut j2len = vec![0usize; bhi - blo + 1];
        let mut newj2len = vec![0usize; bhi - blo + 1];
        let (mut touched, mut newtouched) = (Vec::new(), Vec::new());
        for i in alo..ahi {
            if let Some(js) = self.b2j.get(&a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j2len[j - blo] + 1;
                    newj2len[j - blo + 1] = k;
                    newtouched.push(j - blo + 1);
                    if k > bestsize {
                        (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                    }
                }
            }
            for &t in &touched {
                j2len[t] = 0;
            }
            std::mem::swap(&mut j2len, &mut newj2len);
            std::mem::swap(&mut touched, &mut newtouched);
            newtouched.clear();
        }
        // Extend by equal elements on each side. With autojunk, popular elements could not start a match but
        // may extend one (difflib only skips its junk function's elements here, and there are none).
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        Match { a: besti, b: bestj, size: bestsize }
    }

    /// Non-overlapping matching blocks in order, adjacent ones joined, ending with `(len(a), len(b), 0)`.
    pub fn get_matching_blocks(&self) -> Vec<Match> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let m = self.find_longest_match(alo, ahi, blo, bhi);
            if m.size > 0 {
                blocks.push(m);
                if alo < m.a && blo < m.b {
                    queue.push((alo, m.a, blo, m.b));
                }
                if m.a + m.size < ahi && m.b + m.size < bhi {
                    queue.push((m.a + m.size, ahi, m.b + m.size, bhi));
                }
            }
        }
        blocks.sort();
        let mut out: Vec<Match> = Vec::new();
        let mut cur = Match { a: 0, b: 0, size: 0 };
        for m in blocks {
            if cur.a + cur.size == m.a && cur.b + cur.size == m.b {
                cur.size += m.size;
            } else {
                if cur.size > 0 {
                    out.push(cur);
                }
                cur = m;
            }
        }
        if cur.size > 0 {
            out.push(cur);
        }
        out.push(Match { a: la, b: lb, size: 0 });
        out
    }

    /// How to turn `a` into `b`.
    pub fn get_opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        for m in self.get_matching_blocks() {
            let tag = match (i < m.a, j < m.b) {
                (true, true) => Some(Tag::Replace),
                (true, false) => Some(Tag::Delete),
                (false, true) => Some(Tag::Insert),
                _ => None,
            };
            if let Some(tag) = tag {
                out.push(Opcode { tag, i1: i, i2: m.a, j1: j, j2: m.b });
            }
            (i, j) = (m.a + m.size, m.b + m.size);
            if m.size > 0 {
                out.push(Opcode { tag: Tag::Equal, i1: m.a, i2: i, j1: m.b, j2: j });
            }
        }
        out
    }

    /// `2 * matches / (len(a) + len(b))`, 1.0 for two empty sequences.
    pub fn ratio(&self) -> f64 {
        let matches: usize = self.get_matching_blocks().iter().map(|m| m.size).sum();
        let total = self.a.len() + self.b.len();
        if total == 0 { 1.0 } else { 2.0 * matches as f64 / total as f64 }
    }
}

/// Lower-cased words of `text[start..end]` with their byte spans.
pub fn word_tokens(text: &str, start: usize, end: usize) -> Vec<(usize, usize, String)> {
    WORD_RE.find_iter(&text[start..end]).map(|m| (start + m.start(), start + m.end(), m.as_str().to_lowercase())).collect()
}

/// A changed run of words: `cur[cur_start..cur_end]` replaced `orig[orig_start..orig_end]` (byte offsets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub cur_start: usize,
    pub cur_end: usize,
    pub orig_start: usize,
    pub orig_end: usize,
}

/// Word-level diff of the original and current text (lines first, so big documents stay fast): the changed
/// words, and the share of the current words that changed.
pub fn compute_changes(orig: &str, cur: &str) -> (Vec<Change>, f64) {
    fn lines(t: &str) -> Vec<(usize, usize, &str)> {
        let mut pos = 0;
        t.split('\n')
            .map(|ln| {
                let span = (pos, pos + ln.len(), ln);
                pos += ln.len() + 1;
                span
            })
            .collect()
    }
    let (la, lb) = (lines(orig), lines(cur));
    let ta: Vec<&str> = la.iter().map(|x| x.2).collect();
    let tb: Vec<&str> = lb.iter().map(|x| x.2).collect();
    let mut changes = Vec::new();
    let mut changed_words = 0;
    let total = WORD_RE.find_iter(cur).count();
    for op in SequenceMatcher::new(&ta, &tb, false).get_opcodes() {
        if op.tag == Tag::Equal {
            continue;
        }
        let a0 = if op.i1 < la.len() { la[op.i1].0 } else { orig.len() };
        let a1 = if op.i2 > op.i1 { la[op.i2 - 1].1 } else { a0 };
        let b0 = if op.j1 < lb.len() { lb[op.j1].0 } else { cur.len() };
        let b1 = if op.j2 > op.j1 { lb[op.j2 - 1].1 } else { b0 };
        let (wa, wb) = (word_tokens(orig, a0, a1), word_tokens(cur, b0, b1));
        let sa: Vec<&str> = wa.iter().map(|w| w.2.as_str()).collect();
        let sb: Vec<&str> = wb.iter().map(|w| w.2.as_str()).collect();
        for w in SequenceMatcher::new(&sa, &sb, false).get_opcodes() {
            if w.tag == Tag::Equal || w.j2 == w.j1 {
                continue;
            }
            changed_words += w.j2 - w.j1;
            let orig_start = if w.i1 < wa.len() { wa[w.i1].0 } else { a1 };
            let orig_end = if w.i2 > w.i1 { wa[w.i2 - 1].1 } else { orig_start };
            changes.push(Change { cur_start: wb[w.j1].0, cur_end: wb[w.j2 - 1].1, orig_start, orig_end });
        }
    }
    let ratio = if total > 0 { changed_words as f64 / total as f64 } else { 0.0 };
    (changes, ratio)
}

/// Tokens of `TOKEN_RE` (`\w+(?:['’-]\w+)*|\s+|[^\w\s]`): words with inner apostrophes and hyphens, runs of
/// white space, single other characters. Byte spans.
pub fn tokens(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((s, c)) = it.next() {
        let mut e = s + c.len_utf8();
        if py::is_word(c) {
            loop {
                while let Some(&(i, d)) = it.peek() {
                    if !py::is_word(d) {
                        break;
                    }
                    e = i + d.len_utf8();
                    it.next();
                }
                // a joiner counts only when a word character follows it
                let mut ahead = text[e..].chars();
                match (ahead.next(), ahead.next()) {
                    (Some(j @ ('\'' | '’' | '-')), Some(d)) if py::is_word(d) => {
                        it.next();
                        it.next();
                        e += j.len_utf8() + d.len_utf8();
                    }
                    _ => break,
                }
            }
        } else if py::is_space(c) {
            while let Some(&(i, d)) = it.peek() {
                if !py::is_space(d) {
                    break;
                }
                e = i + d.len_utf8();
                it.next();
            }
        }
        out.push((s, e));
    }
    out
}

/// The minimal changed pieces between two versions of a sentence: `(old_start, old_end, replacement)` with
/// byte offsets into `old`. Pure insertions are attached to a neighbouring word so they can be highlighted.
pub fn diff_chunks(old: &str, new: &str) -> Vec<(usize, usize, String)> {
    let (a, b) = (tokens(old), tokens(new));
    let ta: Vec<&str> = a.iter().map(|&(s, e)| &old[s..e]).collect();
    let tb: Vec<&str> = b.iter().map(|&(s, e)| &new[s..e]).collect();
    let mut pieces: Vec<[usize; 4]> = Vec::new();
    for op in SequenceMatcher::new(&ta, &tb, false).get_opcodes() {
        if op.tag == Tag::Equal {
            continue;
        }
        let os = if op.i1 < a.len() { a[op.i1].0 } else { old.len() };
        let oe = if op.i2 > op.i1 { a[op.i2 - 1].1 } else { os };
        let ns = if op.j1 < b.len() { b[op.j1].0 } else { new.len() };
        let ne = if op.j2 > op.j1 { b[op.j2 - 1].1 } else { ns };
        match pieces.last_mut() {
            // only a space in between: one change, not two
            Some(p) if old[p[1]..os] == new[p[3]..ns] && py::strip(&old[p[1]..os]).is_empty() => {
                p[1] = oe;
                p[3] = ne;
            }
            _ => pieces.push([os, oe, ns, ne]),
        }
    }
    let nonspace = |c: char| !py::is_space(c);
    let mut out = Vec::new();
    for [mut os, mut oe, ns, ne] in pieces {
        let mut rep = new[ns..ne].to_string();
        if os == oe {
            if let Some(k) = old[oe..].find(nonspace) {
                let end = old[oe + k..].find(py::is_space).map_or(old.len(), |n| oe + k + n);
                rep.push_str(&old[os..end]);
                oe = end;
            } else {
                let Some(last) = old[..os].rfind(nonspace) else { continue };
                let start = old[..last].rfind(py::is_space).map_or(0, |i| i + py::char_at(old, i).unwrap().len_utf8());
                rep = format!("{}{rep}", &old[start..os]);
                os = start;
            }
        }
        while os < oe {
            match (py::char_at(old, os), rep.chars().next()) {
                (Some(c), Some(r)) if c == r && py::is_space(c) => {
                    os += c.len_utf8();
                    rep.remove(0);
                }
                _ => break,
            }
        }
        while os < oe {
            match (py::char_before(old, oe), rep.chars().next_back()) {
                (Some(c), Some(r)) if c == r && py::is_space(c) => {
                    oe -= c.len_utf8();
                    rep.pop();
                }
                _ => break,
            }
        }
        if oe > os && old[os..oe] != rep {
            out.push((os, oe, rep));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difflib_examples() {
        // from the difflib documentation
        let a: Vec<char> = "qabxcd".chars().collect();
        let b: Vec<char> = "abycdf".chars().collect();
        let ops: Vec<_> = SequenceMatcher::new(&a, &b, false)
            .get_opcodes()
            .iter()
            .map(|o| (o.tag, o.i1, o.i2, o.j1, o.j2))
            .collect();
        assert_eq!(
            ops,
            [
                (Tag::Delete, 0, 1, 0, 0),
                (Tag::Equal, 1, 3, 0, 2),
                (Tag::Replace, 3, 4, 2, 3),
                (Tag::Equal, 4, 6, 3, 5),
                (Tag::Insert, 6, 6, 5, 6)
            ]
        );
        let a: Vec<char> = " abcd".chars().collect();
        let b: Vec<char> = "abcd abcd".chars().collect();
        assert_eq!(SequenceMatcher::new(&a, &b, false).find_longest_match(0, 5, 0, 9), Match { a: 0, b: 4, size: 5 });
    }

    #[test]
    fn token_shapes() {
        let t = "It's well-known -- x' y";
        let got: Vec<&str> = tokens(t).iter().map(|&(s, e)| &t[s..e]).collect();
        assert_eq!(got, ["It's", " ", "well-known", " ", "-", "-", " ", "x", "'", " ", "y"]);
    }
}
