//! The parts of a text that must not be rewritten: LaTeX commands and maths, Markdown code and links, HTML
//! tags, URLs and e-mail addresses, or (for source code) everything except comments. Was part of
//! `rewrite/formats.py`; the patterns use lookbehind and backreferences, hence `fancy_regex`.
//!
//! Spans are byte ranges `(start, end)` into the text, sorted and merged.

use fancy_regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub type Span = (usize, usize);

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("bad pattern {pattern}: {e}"))
}

/// The spans of all matches (a match that runs out of backtracking budget is skipped, as there is no
/// sensible answer for it).
fn found(re: &Regex, text: &str) -> impl Iterator<Item = Span> {
    re.find_iter(text).filter_map(Result::ok).map(|m| (m.start(), m.end()))
}

fn all(patterns: &[Regex], text: &str) -> Vec<Span> {
    patterns.iter().flat_map(|p| found(p, text).collect::<Vec<_>>()).collect()
}

static URL: LazyLock<Regex> =
    LazyLock::new(|| re(r#"\b(?:https?://|www\.)[^\s<>()\[\]{}"']+|\b[\w.+-]+@[\w-]+\.[\w.-]+\b"#));

const TEX_VERBATIM_ENVS: &str = "verbatim|Verbatim|lstlisting|minted|comment|tikzpicture|pgfpicture|equation\\*?|align\\*?|alignat\\*?|\
gather\\*?|multline\\*?|eqnarray\\*?|flalign\\*?|math|displaymath|split|array|[pbvBV]?matrix|cases|\
algorithmic|filecontents\\*?";

const TEX_ARG_CMDS: &str = "cite[a-zA-Z]*|[Cc]ref|ref|eqref|autoref|pageref|nameref|label|url|href|includegraphics|input|include|\
includeonly|usepackage|RequirePackage|documentclass|bibliography|bibliographystyle|addbibresource|begin|end|\
newcommand|renewcommand|providecommand|DeclareMathOperator|newenvironment|renewenvironment|def|let|\
setlength|addtolength|setcounter|addtocounter|hspace|vspace|color|textcolor|colorbox|definecolor|\
pagestyle|thispagestyle|geometry|hypersetup|graphicspath|lstset|lstinputlisting|newtheorem|numberwithin|\
captionsetup|bibitem|nocite|usetikzlibrary|pgfplotsset|sisetup|SI|si|num|ang|qty|unit|fontsize|\
rule|raisebox|resizebox|scalebox|multicolumn|multirow|cline|cmidrule|setcopyright|acmConference|\
doi|isbn|issn|orcid|email|affiliation|address|keywords|ccsdesc|verb|lstinline|mintinline|texttt|path";

/// Braces, three levels deep.
const BR: &str = r"\{(?:[^{}]|\{(?:[^{}]|\{[^{}]*\})*\})*\}";

fn latex_spans(text: &str) -> Vec<Span> {
    static PATS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        vec![
            re(&format!(r"\\begin\{{({TEX_VERBATIM_ENVS})\}}[\s\S]*?\\end\{{\1\}}")),
            re(r"(?<!\\)%[^\n]*"),
            re(r"\$\$[\s\S]*?\$\$"),
            re(r"(?<!\\)\$(?:\\.|[^$\\])+?\$"),
            re(r"\\\[[\s\S]*?\\\]"),
            re(r"\\\([\s\S]*?\\\)"),
            re(r"\\verb(.)[^\n]*?\1"),
            re(&format!(r"\\({TEX_ARG_CMDS})\*?(?:\s*\[[^\]]*\])*(?:\s*{BR}(?:\s*\[[^\]]*\])*)*")),
            re(r"\\[A-Za-z@]+\*?(?:\[[^\]\n]*\])*|\\[^A-Za-z\s]"),
            re(r"[{}&~^_]"),
        ]
    });
    let mut spans = vec![];
    const BEGIN: &str = "\\begin{document}";
    if let Some(i) = text.find(BEGIN).filter(|&i| i > 0) {
        spans.push((0, i + BEGIN.len()));
    }
    spans.extend(all(&PATS, text));
    spans
}

fn markdown_spans(text: &str) -> Vec<Span> {
    static FRONT: LazyLock<Regex> = LazyLock::new(|| re(r"\A---\n[\s\S]*?\n(---|\.\.\.)\s*\n"));
    static PATS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r"^(```|~~~)[^\n]*\n[\s\S]*?^\1[^\n]*$",
            r"(`+)[^`\n][\s\S]*?\1",
            r"\]\([^)\n]*\)",
            r"<!--[\s\S]*?-->",
            r"</?[A-Za-z][^>\n]*>",
            r"^\s*\[[^\]\n]+\]:[^\n]*$",
            r"\$\$[\s\S]*?\$\$",
            r"(?<![\\$\w])\$(?=\S)[^$\n]+?(?<=\S)\$(?!\w)",
            r"^ {0,3}\|?[\s:|-]+\|[\s:|-]*$",
            r"\{[#.][^}\n]*\}",
            r"\[\^[^\]]+\]",
        ]
        .iter()
        .map(|p| re(&format!("(?m){p}")))
        .collect()
    });
    let mut spans: Vec<Span> = found(&FRONT, text).collect();
    spans.extend(all(&PATS, text));
    spans
}

fn html_spans(text: &str) -> Vec<Span> {
    static PATS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [r"<(script|style|pre|code)\b[\s\S]*?</\1\s*>", r"<!--[\s\S]*?-->", r"<[^>]+>", r"&#?\w+;"]
            .iter()
            .map(|p| re(&format!("(?i){p}")))
            .collect()
    });
    all(&PATS, text)
}

const HASH_LANGS: &[&str] = &[".py", ".sh", ".bash", ".rb", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".r", ".pl", ".ps1"];
const SLASH_LANGS: &[&str] = &[".js", ".ts", ".jsx", ".tsx", ".c", ".h", ".cpp", ".hpp", ".cc", ".java", ".go", ".rs", ".cs",
    ".swift", ".kt", ".php", ".scala", ".css", ".scss", ".m"];

/// Everything is protected except comments and docstrings.
fn code_spans(text: &str, path: Option<&Path>) -> Vec<Span> {
    static HASH: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [r#"(?m)"""[\s\S]*?""""#, r"(?m)'''[\s\S]*?'''", r"(?m)(?:^|(?<=\s))#(?![!{\[])[^\n]*"].map(re).into()
    });
    static SLASH: LazyLock<Vec<Regex>> =
        LazyLock::new(|| [r"(?m)/\*[\s\S]*?\*/", r#"(?m)(?<![:"'\w])//[^\n]*"#].map(re).into());
    static DASH: LazyLock<Vec<Regex>> =
        LazyLock::new(|| [r"(?m)(?:^|(?<=\s))--\s[^\n]*", r"(?m)/\*[\s\S]*?\*/"].map(re).into());
    let ext = path.map(crate::files::ext_of).unwrap_or_default();
    let allowed = if HASH_LANGS.contains(&ext.as_str()) {
        all(&HASH, text)
    } else if SLASH_LANGS.contains(&ext.as_str()) {
        all(&SLASH, text)
    } else if ext == ".sql" || ext == ".lua" {
        all(&DASH, text)
    } else {
        [all(&HASH, text), all(&SLASH, text), all(&DASH, text)].concat()
    };
    let mut spans = vec![];
    let mut pos = 0;
    for (s, e) in merge(allowed) {
        if s > pos {
            spans.push((pos, s));
        }
        pos = pos.max(e);
    }
    if pos < text.len() {
        spans.push((pos, text.len()));
    }
    spans
}

/// Sorted, with empty spans dropped and touching or overlapping ones joined.
pub fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_unstable();
    let mut out: Vec<Span> = vec![];
    for (s, e) in spans {
        if e <= s {
            continue;
        }
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// The protected spans of a text of the given kind ("latex", "markdown", "html", "code", anything else is
/// plain). `path` tells the comment syntax of code.
pub fn protected_spans(text: &str, kind: &str, path: Option<&Path>) -> Vec<Span> {
    let mut spans: Vec<Span> = found(&URL, text).collect();
    match kind {
        "latex" => spans.extend(latex_spans(text)),
        "markdown" => spans.extend(markdown_spans(text)),
        "html" => spans.extend(html_spans(text)),
        "code" => spans.extend(code_spans(text, path)),
        _ => {}
    }
    merge(spans)
}

/// True if `[start, end)` touches any span (spans sorted and merged).
pub fn overlaps(spans: &[Span], start: usize, end: usize) -> bool {
    let i = spans.partition_point(|&(s, _)| s <= start);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|j| spans.get(j)).any(|&(s, e)| s < end && start < e)
}

/// The spans inside `[start, end)`, cut to it and relative to `start`.
pub fn clip(spans: &[Span], start: usize, end: usize) -> Vec<Span> {
    let first = spans.partition_point(|&(s, _)| s <= start).saturating_sub(1);
    spans[first.min(spans.len())..]
        .iter()
        .take_while(|&&(s, _)| s < end)
        .filter(|&&(_, e)| e > start)
        .map(|&(s, e)| (s.max(start) - start, e.min(end) - start))
        .collect()
}
