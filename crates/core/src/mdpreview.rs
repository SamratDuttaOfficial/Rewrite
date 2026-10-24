//! A small Markdown parser for the preview panel. Mirrors `rewrite/mdrender.py`.
//!
//! [`parse`] returns blocks; the text inside a block is a list of [`Run`]s with styles from "b", "i", "code",
//! "strike", "math", "sup" (and "img" for pictures). It covers what people actually write (headings,
//! emphasis, lists, task lists, quotes, code, tables, links, pictures, footnote marks) and shows anything
//! else as plain text.

use fancy_regex::{Captures, Regex};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

pub type Styles = BTreeSet<&'static str>;

/// Where a run leads: a link, or for a picture its alternative text and source.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Link(String),
    Image { alt: String, src: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub styles: Styles,
    pub target: Option<Target>,
}

impl Run {
    pub fn link(&self) -> Option<&str> {
        match &self.target {
            Some(Target::Link(url)) => Some(url),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: usize, runs: Vec<Run> },
    Para(Vec<Run>),
    Code { lang: String, source: String },
    Rule,
    /// A list item: nesting depth, its marker ("1." or "•"), and whether its task box is ticked.
    Item { depth: usize, marker: String, checked: Option<bool>, runs: Vec<Run> },
    Quote { depth: usize, runs: Vec<Run> },
    /// Header cells, column alignments ("left", "center", "right") and rows of cells.
    Table { head: Vec<Vec<Run>>, align: Vec<&'static str>, rows: Vec<Vec<Vec<Run>>> },
    Math(String),
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("bad pattern {pattern}: {e}"))
}

/// Matches at the start of `s` only, like Python's `re.match`.
fn at_start<'t>(r: &Regex, s: &'t str) -> Option<Captures<'t, str>> {
    r.captures(s).ok().flatten().filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}(```+|~~~+)\s*([\w+#.-]*)[^\n]*$"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$"));
static SETEXT: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}(=+|-+)\s*$"));
static RULE: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}([-*_])(?:\s*\1){2,}\s*$"));
static ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^(\s*)([-*+]|\d{1,9}[.)])\s+(?:\[([ xX])\]\s+)?(.*)$"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}>\s?(.*)$"));
static TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| re(r"^ {0,3}\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$"));
static LINK_DEF: LazyLock<Regex> = LazyLock::new(|| re(r#"^ {0,3}\[([^\]\n]+)\]:\s*<?(\S+?)>?(?:\s+["'(].*)?\s*$"#));
static FRONT: LazyLock<Regex> = LazyLock::new(|| re(r"\A---\n[\s\S]*?\n(?:---|\.\.\.)\s*(?:\n|\z)"));

static INLINE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?s)",
        r"\\([\\`*_{}\[\]()#+\-.!|~>$])", // 1 escaped character
        r"|(`+)(.+?)\2(?!`)", // 2, 3 code span
        r#"|!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+["'][^"']*["'])?\s*\)"#, // 4, 5 image
        r#"|\[((?:[^\[\]]|\[[^\]]*\])*)\]\(\s*<?([^)\s>]*)>?(?:\s+["'][^"']*["'])?\s*\)"#, // 6, 7 link
        r"|\[((?:[^\[\]])+)\]\[([^\]]*)\]", // 8, 9 reference link
        r"|<((?:https?://|mailto:)[^>\s]+)>", // 10 autolink
        r#"|(?<![\w/("'=])((?:https?://|www\.)[^\s<>]*[^\s<>.,;:!?)\]'"])"#, // 11 bare URL
        r"|(\*\*\*|___)(?=\S)(.+?)(?<=\S)\12", // 12, 13 bold italic
        r"|(\*\*|__)(?=\S)(.+?)(?<=\S)\14", // 14, 15 bold
        r"|(?<![\w*])(\*|_)(?=[^\s*_])(.+?)(?<=[^\s*_])\16(?![\w*])", // 16, 17 italic
        r"|~~(?=\S)(.+?)(?<=\S)~~", // 18 strike
        r"|(?<![\\$\w])\$(?=\S)([^$\n]+?)(?<=\S)\$(?!\w)", // 19 inline maths
        r"|\[\^([^\]\s]+)\]", // 20 footnote reference
        r"|(<br\s*/?>)", // 21 line break
        r"|(</?[A-Za-z][^>\n]*>|<!--.*?-->)", // 22 other HTML: dropped
        r"|(&(?:#\d+|#x[0-9a-fA-F]+|\w+);)", // 23 entity
    ))
});

fn with(styles: &Styles, more: &[&'static str]) -> Styles {
    styles.iter().copied().chain(more.iter().copied()).collect()
}

/// Inline Markdown split into runs.
pub fn inline(text: &str) -> Vec<Run> {
    inline_in(text, &Styles::new(), None, &HashMap::new())
}

fn inline_in(text: &str, styles: &Styles, link: Option<&str>, refs: &HashMap<String, String>) -> Vec<Run> {
    let mut out = vec![];
    let emit = |out: &mut Vec<Run>, s: &str, st: &Styles, ln: Option<&str>| {
        if !s.is_empty() {
            out.push(Run { text: s.to_string(), styles: st.clone(), target: ln.map(|l| Target::Link(l.to_string())) });
        }
    };
    let mut pos = 0;
    for m in INLINE.captures_iter(text).filter_map(Result::ok) {
        let whole = m.get(0).unwrap();
        emit(&mut out, &text[pos..whole.start()], styles, link);
        pos = whole.end();
        let g = |i: usize| m.get(i).map(|x| x.as_str());
        let some = |i: usize| g(i).filter(|s| !s.is_empty());
        if let Some(c) = some(1) {
            emit(&mut out, c, styles, link);
        } else if some(2).is_some() {
            let code = g(3).unwrap();
            let trimmed = code.trim();
            emit(&mut out, if trimmed.is_empty() { code } else { trimmed }, &with(styles, &["code"]), link);
        } else if let Some(src) = some(5) {
            let target = Target::Image { alt: g(4).unwrap_or("").to_string(), src: src.to_string() };
            out.push(Run { text: String::new(), styles: Styles::from(["img"]), target: Some(target) });
        } else if let (Some(label), Some(url)) = (g(6), g(7)) {
            out.extend(inline_in(label, styles, Some(url).filter(|u| !u.is_empty()), refs));
        } else if let Some(label) = some(8) {
            let key = some(9).unwrap_or(label).to_lowercase();
            match refs.get(&key) {
                Some(url) => out.extend(inline_in(label, styles, Some(url), refs)),
                None => emit(&mut out, whole.as_str(), styles, link),
            }
        } else if let Some(url) = some(10).or(some(11)) {
            let target = if url.contains("://") || url.starts_with("mailto:") { url.to_string() } else { format!("https://{url}") };
            emit(&mut out, url, styles, Some(&target));
        } else if let Some(t) = some(13) {
            out.extend(inline_in(t, &with(styles, &["b", "i"]), link, refs));
        } else if let Some(t) = some(15) {
            out.extend(inline_in(t, &with(styles, &["b"]), link, refs));
        } else if let Some(t) = some(17) {
            out.extend(inline_in(t, &with(styles, &["i"]), link, refs));
        } else if let Some(t) = some(18) {
            out.extend(inline_in(t, &with(styles, &["strike"]), link, refs));
        } else if let Some(t) = some(19) {
            emit(&mut out, t, &with(styles, &["math"]), link);
        } else if let Some(t) = some(20) {
            emit(&mut out, &format!("[{t}]"), &with(styles, &["sup"]), link);
        } else if some(21).is_some() {
            emit(&mut out, "\n", styles, link);
        } else if let Some(t) = some(23) {
            emit(&mut out, &htmlize::unescape(t), styles, link);
        }
    }
    emit(&mut out, &text[pos..], styles, link);
    out
}

/// The cells of a table row: split at unescaped "|", outer pipes dropped.
fn cells(line: &str) -> Vec<String> {
    let mut line = line.trim();
    line = line.strip_prefix('|').unwrap_or(line);
    if line.ends_with('|') && !line.ends_with("\\|") {
        line = &line[..line.len() - 1];
    }
    let mut out = vec![];
    let mut start = 0;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'|' && (i == 0 || bytes[i - 1] != b'\\') {
            out.push(&line[start..i]);
            start = i + 1;
        }
    }
    out.push(&line[start..]);
    out.into_iter().map(|c| c.trim().replace("\\|", "|")).collect()
}

/// Tabs replaced by spaces up to the next multiple of 4, like Python's `str.expandtabs(4)`.
fn expand_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// Whether `line` closes a fence opened with `n` times `mark`.
fn closes_fence(line: &str, mark: char, n: usize) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let k = rest.chars().take_while(|&c| c == mark).count();
    k >= n && rest[k * mark.len_utf8()..].trim().is_empty()
}

fn is(r: &Regex, s: &str) -> bool {
    at_start(r, s).is_some()
}

/// The blocks of a Markdown document.
pub fn parse(text: &str) -> Vec<Block> {
    let mut text = expand_tabs(&text.replace("\r\n", "\n"));
    if let Some(m) = at_start(&FRONT, &text) {
        text = text[m.get(0).unwrap().end()..].to_string();
    }
    // link definitions can come after their use
    let mut refs = HashMap::new();
    let mut lines: Vec<&str> = vec![];
    for ln in text.split('\n') {
        match at_start(&LINK_DEF, ln) {
            Some(d) if !ln.trim_start().starts_with("[^") => {
                refs.insert(d[1].to_lowercase(), d[2].to_string());
            }
            _ => lines.push(ln),
        }
    }
    let inl = |s: &str| inline_in(s, &Styles::new(), None, &refs);
    let mut blocks = vec![];
    let mut para: Vec<&str> = vec![];
    let mut indents: Vec<usize> = vec![]; // indentation of the open list levels
    let n = lines.len();
    let mut i = 0;
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Para(inl(&para.iter().map(|x| x.trim()).collect::<Vec<_>>().join(" "))));
            para.clear();
        }
    };
    while i < n {
        let ln = lines[i];
        if ln.trim().is_empty() {
            flush(&mut para, &mut blocks);
            i += 1;
            continue;
        }
        if let Some(f) = at_start(&FENCE, ln) {
            flush(&mut para, &mut blocks);
            let mark = &f[1];
            let (ch, len) = (mark.chars().next().unwrap(), mark.chars().count());
            let mut body = vec![];
            i += 1;
            while i < n && !closes_fence(lines[i], ch, len) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code { lang: f[2].to_string(), source: body.join("\n") });
            continue;
        }
        if ln.trim().starts_with("$$") {
            flush(&mut para, &mut blocks);
            let first = &ln.trim()[2..];
            let mut body: Vec<&str> = vec![first];
            if !(first.trim_end().ends_with("$$") && !first.trim().is_empty()) {
                i += 1;
                while i < n && !lines[i].contains("$$") {
                    body.push(lines[i]);
                    i += 1;
                }
                if i < n {
                    body.push(lines[i].split("$$").next().unwrap());
                }
            } else {
                let t = first.trim_end();
                body[0] = &t[..t.len() - 2];
            }
            i += 1;
            let tex = body.iter().filter(|x| !x.trim().is_empty()).copied().collect::<Vec<_>>().join("\n");
            blocks.push(Block::Math(tex.trim().to_string()));
            continue;
        }
        if let Some(h) = at_start(&HEADING, ln) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Heading { level: h[1].len(), runs: inl(&h[2]) });
            i += 1;
            continue;
        }
        if !para.is_empty() && is(&SETEXT, ln) {
            let level = if ln.trim().starts_with('=') { 1 } else { 2 };
            blocks.push(Block::Heading { level, runs: inl(&para.iter().map(|x| x.trim()).collect::<Vec<_>>().join(" ")) });
            para.clear();
            i += 1;
            continue;
        }
        if is(&RULE, ln) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if ln.contains('|') && i + 1 < n && is(&TABLE_SEP, lines[i + 1]) && lines[i + 1].contains('-') {
            flush(&mut para, &mut blocks);
            let head = cells(ln);
            let align = cells(lines[i + 1])
                .iter()
                .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
                    (true, true) => "center",
                    (_, true) => "right",
                    _ => "left",
                })
                .collect();
            let mut rows = vec![];
            i += 2;
            while i < n && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(cells(lines[i]).iter().map(|c| inl(c)).collect());
                i += 1;
            }
            blocks.push(Block::Table { head: head.iter().map(|c| inl(c)).collect(), align, rows });
            continue;
        }
        if is(&QUOTE, ln) {
            flush(&mut para, &mut blocks);
            let mut body: Vec<String> = vec![];
            let mut depth = 1;
            while i < n {
                let Some(q) = at_start(&QUOTE, lines[i]) else { break };
                let mut inner = q.get(1).unwrap().as_str();
                let mut d = 1;
                while let Some(q) = at_start(&QUOTE, inner) {
                    inner = q.get(1).unwrap().as_str();
                    d += 1;
                }
                depth = depth.max(d);
                if !inner.trim().is_empty() {
                    body.push(inner.trim().to_string());
                } else if !body.is_empty() {
                    blocks.push(Block::Quote { depth, runs: inl(&body.join(" ")) });
                    body.clear();
                }
                i += 1;
            }
            if !body.is_empty() {
                blocks.push(Block::Quote { depth, runs: inl(&body.join(" ")) });
            }
            continue;
        }
        if let Some(it) = at_start(&ITEM, ln).filter(|it| para.is_empty() || it[1].chars().count() < 4) {
            flush(&mut para, &mut blocks);
            let indent = it[1].chars().count();
            while indents.last().is_some_and(|&last| indent < last) {
                indents.pop();
            }
            if indents.last().is_none_or(|&last| indent > last + 1) {
                indents.push(indent);
            }
            let mut body = vec![it.get(4).unwrap().as_str()];
            i += 1;
            while i < n && !lines[i].trim().is_empty() && !is(&ITEM, lines[i]) && !is(&FENCE, lines[i]) && !is(&HEADING, lines[i]) {
                body.push(lines[i].trim()); // lazy continuation line
                i += 1;
            }
            let checked = it.get(3).map(|c| c.as_str() != " ");
            let marker = match &it[2] {
                "-" | "*" | "+" => "•".to_string(),
                number => number.to_string(),
            };
            blocks.push(Block::Item { depth: indents.len() - 1, marker, checked, runs: inl(&body.join(" ")) });
            if i < n && lines[i].trim().is_empty() && !(i + 1 < n && is(&ITEM, lines[i + 1])) {
                indents.clear();
            }
            continue;
        }
        if para.is_empty() && ln.starts_with("    ") && indents.is_empty() {
            // indented code
            let mut body: Vec<String> = vec![];
            while i < n && (lines[i].starts_with("    ") || lines[i].trim().is_empty()) {
                body.push(lines[i].chars().skip(4).collect());
                i += 1;
            }
            blocks.push(Block::Code { lang: String::new(), source: body.join("\n").trim_end_matches('\n').to_string() });
            continue;
        }
        indents.clear();
        para.push(ln);
        i += 1;
    }
    flush(&mut para, &mut blocks);
    blocks
}
