//! LaTeX <-> Document.
//!
//! Reading understands what papers and notes usually contain: sectioning, \maketitle, lists, quotes,
//! figures with \includegraphics and captions, tabular, maths (inline and displayed), verbatim and
//! listings, footnotes, citations and cross-references (numbered like LaTeX would), accents, text styles
//! and simple \newcommand macros. Writing produces a document that compiles with pdflatex (or xelatex
//! when the text needs Unicode fonts).

use crate::charts;
use crate::imaging;
use crate::md::Files;
use crate::model::*;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

fn re(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "ss" => "ß", "ae" => "æ", "AE" => "Æ", "oe" => "œ", "OE" => "Œ", "o" => "ø", "O" => "Ø", "aa" => "å",
        "AA" => "Å", "l" => "ł", "L" => "Ł", "i" => "ı", "j" => "ȷ", "dag" => "†", "ddag" => "‡", "S" => "§",
        "P" => "¶", "copyright" => "©", "pounds" => "£", "euro" => "€", "textregistered" => "®",
        "texttrademark" => "™", "ldots" | "dots" | "textellipsis" => "…", "textbullet" => "•",
        "textendash" => "–", "textemdash" => "—", "textquoteleft" => "‘", "textquoteright" => "’",
        "textquotedblleft" => "“", "textquotedblright" => "”", "LaTeX" => "LaTeX", "TeX" => "TeX",
        "LaTeXe" => "LaTeX2e", "textbackslash" => "\\", "textasciitilde" => "~", "textasciicircum" => "^",
        "textless" => "<", "textgreater" => ">", "textbar" => "|", "textdegree" => "°", "texttimes" => "×",
        "textpm" => "±", "textmu" => "µ", "checkmark" => "✓", "&" => "&", "%" => "%", "$" => "$", "#" => "#",
        "_" => "_", "{" => "{", "}" => "}", " " => " ", "," => " ", ";" => " ", ":" => " ", "!" => "", "@" => "",
        "/" => "", "-" => "", "quad" | "qquad" | "enspace" | "thinspace" | "space" | "enskip" => " ",
        "nobreakspace" => "\u{a0}", "slash" => "/", "guillemotleft" => "«", "guillemotright" => "»",
        "textperiodcentered" => "·", "textsection" => "§", "textparagraph" => "¶",
        _ => return None,
    })
}

fn accent(name: &str) -> Option<char> {
    Some(match name {
        "'" => '\u{0301}', "`" => '\u{0300}', "^" => '\u{0302}', "\"" => '\u{0308}', "~" => '\u{0303}', "=" => '\u{0304}',
        "." => '\u{0307}', "u" => '\u{0306}', "v" => '\u{030c}', "H" => '\u{030b}', "c" => '\u{0327}', "k" => '\u{0328}',
        "r" => '\u{030a}', "d" => '\u{0323}', "b" => '\u{0331}',
        _ => return None,
    })
}

const DROP: &[&str] = &["label", "index", "noindent", "indent", "centering", "raggedright", "raggedleft", "small",
    "footnotesize", "scriptsize", "tiny", "large", "Large", "LARGE", "huge", "Huge", "normalsize", "vspace", "vskip",
    "hspace", "hfill", "vfill", "bigskip", "medskip", "smallskip", "clearpage", "protect", "nopagebreak", "linebreak",
    "pagestyle", "thispagestyle", "setlength", "addtolength", "setcounter", "addtocounter", "newpage", "phantomsection",
    "addcontentsline", "nocite", "bibliographystyle", "bibliography", "printbibliography", "tableofcontents",
    "listoffigures", "listoftables", "appendix", "frontmatter", "mainmatter", "backmatter", "sloppy", "fussy",
    "onecolumn", "twocolumn", "normalfont", "selectfont", "fontsize", "usefont", "hline", "toprule", "midrule",
    "bottomrule", "cline", "cmidrule", "centerline", "par", "rule", "strut", "displaystyle", "allowbreak",
    "FloatBarrier", "balance", "looseness"];
const NO_ARGS: &[&str] = &["centering", "noindent", "small", "large", "par", "hline"];

fn style_cmd(name: &str) -> Option<&'static str> {
    Some(match name {
        "textbf" | "bfseries" | "bf" => "b", "textit" | "itshape" | "it" | "textsl" | "slshape" | "sl" => "i",
        "underline" | "uline" => "u", "sout" | "st" | "xout" => "s", "texttt" | "ttfamily" | "tt" => "code",
        "textsuperscript" => "sup", "textsubscript" => "sub",
        _ => return None,
    })
}

fn set_fmt(r: &mut Run, key: &str) {
    match key {
        "b" => r.b = true,
        "i" => r.i = true,
        "u" => r.u = true,
        "s" => r.s = true,
        "code" => r.code = true,
        "sup" => r.sup = true,
        "sub" => r.sub = true,
        _ => {}
    }
}

fn cite_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| re(r"^(cite|citep|citet|citealp|citealt|citeauthor|citeyear|parencite|textcite|autocite|footcite|citeyearpar|Cite|Citep|Citet|smartcite|supercite)\*?$"))
}

fn ref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| re(r"^(ref|eqref|autoref|Autoref|cref|Cref|pageref|nameref|vref|Vref|subref)\*?$"))
}

fn color_named(name: &str) -> Option<&'static str> {
    Some(match name {
        "red" => "#FF0000", "blue" => "#0000FF", "green" => "#008000", "black" => "#000000", "white" => "#FFFFFF",
        "gray" | "grey" => "#808080", "cyan" => "#00FFFF", "magenta" => "#FF00FF", "yellow" => "#FFD700",
        "orange" => "#FF8000", "purple" | "violet" => "#800080", "brown" => "#996633", "teal" => "#008080",
        "olive" => "#808000", "darkgray" => "#404040", "lightgray" => "#BFBFBF", "pink" => "#FFBFBF", "lime" => "#BFFF00",
        _ => return None,
    })
}

fn color(name: &str, model: Option<&str>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match model.map(|m| m.trim()) {
        Some(m) if m.eq_ignore_ascii_case("html") => return Some(format!("#{}", name.to_uppercase())),
        Some("rgb") => {
            let v: Vec<f64> = name.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() >= 3 {
                return Some(format!("#{:02X}{:02X}{:02X}", (v[0].clamp(0.0, 1.0) * 255.0) as u8, (v[1].clamp(0.0, 1.0) * 255.0) as u8, (v[2].clamp(0.0, 1.0) * 255.0) as u8));
            }
            return None;
        }
        _ => {}
    }
    color_named(&name.split('!').next().unwrap_or("").to_lowercase()).map(String::from)
}

/// Remove % comments (a comment also eats its line break).
pub fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let lines: Vec<&str> = src.split('\n').collect();
    for (k, line) in lines.iter().enumerate() {
        let b = line.as_bytes();
        let mut cut = None;
        let mut slashes = 0;
        for (i, c) in b.iter().enumerate() {
            if *c == b'\\' {
                slashes += 1;
                continue;
            }
            if *c == b'%' && slashes % 2 == 0 {
                cut = Some(i);
                break;
            }
            slashes = 0;
        }
        match cut {
            Some(i) => out.push_str(&line[..i]),
            None => {
                out.push_str(line);
                if k + 1 < lines.len() {
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// The text inside a {...} group starting at s[i] (after optional spaces): (text, end).
pub fn read_group(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut j = i;
    while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n') {
        j += 1;
    }
    if j >= b.len() || b[j] != b'{' {
        return None;
    }
    let mut depth = 0;
    let mut k = j;
    while k < b.len() {
        match b[k] {
            b'\\' => {
                k += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[j + 1..k].to_string(), k + 1));
                }
            }
            _ => {}
        }
        k += 1;
    }
    Some((s[j + 1..].to_string(), s.len()))
}

pub fn read_optional(s: &str, i: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut j = i;
    while j < b.len() && matches!(b[j], b' ' | b'\t') {
        j += 1;
    }
    if j >= b.len() || b[j] != b'[' {
        return None;
    }
    let mut depth = 0;
    let mut k = j;
    while k < b.len() {
        match b[k] {
            b'\\' => {
                k += 2;
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[j + 1..k].to_string(), k + 1));
                }
            }
            _ => {}
        }
        k += 1;
    }
    Some((s[j + 1..].to_string(), s.len()))
}

/// At s[i] == '\\': (name, end).
pub fn read_command(s: &str, i: usize) -> (String, usize) {
    let b = s.as_bytes();
    let mut j = i + 1;
    if j >= b.len() {
        return (String::new(), b.len());
    }
    if b[j].is_ascii_alphabetic() || b[j] == b'@' {
        while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'@') {
            j += 1;
        }
        if j < b.len() && b[j] == b'*' {
            j += 1;
        }
        return (s[i + 1..j].to_string(), j);
    }
    let ch = s[j..].chars().next().unwrap();
    (ch.to_string(), j + ch.len_utf8())
}

/// The span (start, after) of the \end{env} that closes an environment whose body starts at s[i].
pub fn find_end(s: &str, i: usize, env: &str) -> (usize, usize) {
    let pat = re(&format!(r"\\(begin|end)\s*\{{{}\}}", regex::escape(env)));
    let mut depth = 1;
    for m in pat.captures_iter(&s[i..]) {
        let whole = m.get(0).unwrap();
        depth += if &m[1] == "begin" { 1 } else { -1 };
        if depth == 0 {
            return (i + whole.start(), i + whole.end());
        }
    }
    (s.len(), s.len())
}

/// Expand simple \newcommand / \renewcommand / \def macros (one level).
pub fn expand_macros(src: &str) -> String {
    let mut src = src.to_string();
    let mut macros: HashMap<String, (usize, String)> = HashMap::new();
    let pat = re(r"\\(?:re)?newcommand\*?\s*\{?\\([A-Za-z@]+)\}?\s*(?:\[(\d)\])?(?:\[[^\]]*\])?\s*");
    let mut pos = 0;
    while let Some(m) = pat.captures(&src[pos..]) {
        let whole = m.get(0).unwrap();
        let (start, end) = (pos + whole.start(), pos + whole.end());
        match read_group(&src, end) {
            Some((body, after)) => {
                macros.insert(m[1].to_string(), (m.get(2).map(|x| x.as_str().parse().unwrap_or(0)).unwrap_or(0), body));
                src.replace_range(start..after, "");
                pos = start;
            }
            None => pos = end,
        }
    }
    for m in re(r"\\def\\([A-Za-z@]+)\s*\{").captures_iter(&src.clone()) {
        let start = m.get(0).unwrap().end() - 1;
        if let Some((body, _)) = read_group(&src, start) {
            macros.entry(m[1].to_string()).or_insert((0, body));
        }
    }
    if macros.is_empty() {
        return src;
    }
    let mut names: Vec<&String> = macros.keys().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let rx = re(&format!(r"\\({})([^A-Za-z@]|$)", names.iter().map(|n| regex::escape(n)).collect::<Vec<_>>().join("|")));
    for _ in 0..3 {
        let mut out = String::new();
        let mut pos = 0;
        let mut changed = false;
        let mut search = 0;
        while let Some(m) = rx.captures(&src[search..]) {
            let g1 = m.get(1).unwrap();
            let start = search + g1.start() - 1;
            let mut k = search + g1.end();
            let (nargs, body) = macros[g1.as_str()].clone();
            let mut args = vec![];
            for _ in 0..nargs {
                match read_group(&src, k) {
                    Some((g, k2)) => {
                        args.push(g);
                        k = k2;
                    }
                    None => break,
                }
            }
            if args.len() != nargs {
                search = search + g1.end();
                continue;
            }
            let mut body = body;
            for (n, a) in args.iter().enumerate() {
                body = body.replace(&format!("#{}", n + 1), a);
            }
            out.push_str(&src[pos..start]);
            let needs_space = body.chars().last().is_some_and(|c| c.is_alphabetic()) && src[k..].chars().next().is_some_and(|c| c.is_alphabetic());
            out.push_str(&body);
            if needs_space {
                out.push(' ');
            }
            pos = k;
            search = k;
            changed = true;
        }
        out.push_str(&src[pos..]);
        src = out;
        if !changed {
            break;
        }
    }
    src
}

/// Split at sep where it is not inside braces.
fn split_top(s: &str, sep: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with('\\') && !rest.starts_with(sep) {
            let n = rest.chars().take(2).map(|c| c.len_utf8()).sum::<usize>();
            cur.push_str(&rest[..n]);
            i += n;
            continue;
        }
        if depth == 0 && rest.starts_with(sep) {
            out.push(std::mem::take(&mut cur));
            i += sep.len();
            continue;
        }
        let c = rest.chars().next().unwrap();
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
        }
        cur.push(c);
        i += c.len_utf8();
    }
    out.push(cur);
    out
}

/// Plain text of a LaTeX fragment (for titles and the like).
pub fn plain(tex: &str) -> String {
    let mut r = Reader::new(None, false, String::new(), String::new(), String::new());
    r.inline(tex, None).iter().filter_map(|x| if let Inline::Run(r) = x { Some(r.text.clone()) } else { None }).collect::<String>().trim().to_string()
}

/// Turns the bytes of a PDF figure into PNG bytes (the program draws it with PDFium).
pub type PdfFigure<'a> = &'a dyn Fn(&[u8]) -> Option<Vec<u8>>;

pub struct Reader<'a> {
    base_dir: Option<&'a Path>,
    pub pdf_figure: Option<PdfFigure<'a>>,
    chapters: bool,
    title: String,
    author: String,
    date: String,
    pub out: Vec<Block>,
    labels: HashMap<String, String>,
    figures: usize,
    tables: usize,
    sections: Vec<usize>,
    caption_label: String,
    captions: Vec<Vec<Inline>>,
}

impl<'a> Reader<'a> {
    pub fn new(base_dir: Option<&'a Path>, chapters: bool, title: String, author: String, date: String) -> Reader<'a> {
        Reader { base_dir, pdf_figure: None, chapters, title, author, date, out: vec![], labels: HashMap::new(), figures: 0, tables: 0,
                 sections: vec![], caption_label: String::new(), captions: vec![] }
    }

    fn levels(&self) -> Vec<&'static str> {
        if self.chapters { vec!["chapter", "section", "subsection", "subsubsection"] } else { vec!["section", "subsection", "subsubsection"] }
    }

    pub fn number_labels(&mut self, body: &str) {
        let levels = self.levels();
        let mut nums = vec![0usize; levels.len()];
        let (mut fig, mut tab, mut eq) = (0, 0, 0);
        let mut current = String::new();
        let tok = re(r"\\(chapter|section|subsection|subsubsection)(\*?)|\\begin\{(figure|table|equation|align|gather|multline|eqnarray)(\*?)\}|\\label\{([^}]*)\}");
        for m in tok.captures_iter(body) {
            if let Some(sec) = m.get(1) {
                if !m[2].is_empty() {
                    continue;
                }
                let Some(k) = levels.iter().position(|l| *l == sec.as_str()) else { continue };
                nums[k] += 1;
                for n in nums.iter_mut().skip(k + 1) {
                    *n = 0;
                }
                current = nums[..=k].iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");
            } else if let Some(env) = m.get(3) {
                if !m[4].is_empty() {
                    continue;
                }
                match env.as_str() {
                    "figure" => {
                        fig += 1;
                        current = fig.to_string();
                    }
                    "table" => {
                        tab += 1;
                        current = tab.to_string();
                    }
                    _ => {
                        eq += 1;
                        current = eq.to_string();
                    }
                }
            } else if let Some(l) = m.get(5) {
                self.labels.insert(l.as_str().trim().to_string(), current.clone());
            }
        }
    }

    fn emit(&mut self, b: Block) {
        self.out.push(b);
    }

    fn section_number(&mut self, base: &str) -> String {
        let levels = self.levels();
        let Some(k) = levels.iter().position(|l| *l == base) else { return String::new() };
        if self.sections.len() < levels.len() {
            self.sections.resize(levels.len(), 0);
        }
        self.sections[k] += 1;
        for n in self.sections.iter_mut().skip(k + 1) {
            *n = 0;
        }
        self.sections[..=k].iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".")
    }

    fn flush(&mut self, buf: &mut String, style: &str, align: &Option<String>, lst: &Option<(String, u32)>, indent: u32, first_item: &mut bool) {
        let src = buf.trim().to_string();
        buf.clear();
        if src.is_empty() {
            return;
        }
        let mut content = self.inline(&src, None);
        let captions = std::mem::take(&mut self.captions);
        while matches!(content.first(), Some(Inline::Break(_))) {
            content.remove(0);
        }
        let content = strip_content(content);
        let has = content.iter().any(|x| match x {
            Inline::Run(r) => !r.text.trim().is_empty(),
            Inline::Break(_) => false,
            _ => true,
        });
        if has {
            if *first_item {
                self.emit(Block::Paragraph(Paragraph { content, style: style.into(), align: align.clone(), list: lst.clone(), indent }));
                *first_item = false;
            } else {
                let extra = lst.as_ref().map(|l| l.1 + 1).unwrap_or(0);
                self.emit(Block::Paragraph(Paragraph { content, style: style.into(), align: align.clone(), list: None, indent: indent + extra }));
            }
        }
        for cap in captions {
            self.emit(Block::Paragraph(Paragraph { content: merge_runs(cap), style: "caption".into(), align: Some("center".into()), ..Default::default() }));
        }
    }

    pub fn blocks(&mut self, text: &str, style: &str, align: Option<String>, lst: Option<(String, u32)>, indent: u32) {
        let mut buf = String::new();
        let mut first_item = lst.is_some();
        let blank = re(r"^\n[ \t]*\n\s*");
        let b = text.as_bytes();
        let n = b.len();
        let mut i = 0;
        let mut heads: HashMap<&str, &str> = [("part", "title"), ("chapter", "h1"), ("section", "h1"), ("subsection", "h2"),
            ("subsubsection", "h3"), ("paragraph", "h4"), ("subparagraph", "h5")].into_iter().collect();
        if self.chapters {
            heads.extend([("section", "h2"), ("subsection", "h3"), ("subsubsection", "h4"), ("paragraph", "h5"), ("subparagraph", "h6")]);
        }
        while i < n {
            let c = b[i];
            if c == b'\n' {
                if let Some(m) = blank.find(&text[i..]) {
                    self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                    i += m.end();
                    continue;
                }
                buf.push(' ');
                i += 1;
                continue;
            }
            if c == b'$' && text[i..].starts_with("$$") {
                let end = text[i + 2..].find("$$").map(|e| i + 2 + e).unwrap_or(n);
                self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                self.emit(Block::MathBlock(MathBlock { tex: text[i + 2..end].trim().into() }));
                i = (end + 2).min(n);
                continue;
            }
            if c != b'\\' {
                let ch = text[i..].chars().next().unwrap();
                buf.push(ch);
                i += ch.len_utf8();
                continue;
            }
            let (name, j) = read_command(text, i);
            if name == "[" {
                let end = text[j..].find("\\]").map(|e| j + e).unwrap_or(n);
                self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                self.emit(Block::MathBlock(MathBlock { tex: text[j..end].trim().into() }));
                i = (end + 2).min(n);
                continue;
            }
            if name == "begin" {
                let Some((env, k)) = read_group(text, j) else {
                    buf.push_str(&text[i..j]);
                    i = j;
                    continue;
                };
                let env = env.trim().to_string();
                let (s, e) = find_end(text, k, &env);
                let inner = text[k..s].to_string();
                self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                if !self.block_env(&env, &inner, style, &align, &lst, indent) {
                    buf.push_str(&text[i..e]);
                }
                i = e;
                continue;
            }
            let base = name.trim_end_matches('*');
            if let Some(h) = heads.get(base).copied() {
                let k = read_optional(text, j).map(|x| x.1).unwrap_or(j);
                if let Some((title, k)) = read_group(text, k) {
                    self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                    let mut content = self.inline(&title, None);
                    if !name.ends_with('*') && !matches!(base, "paragraph" | "subparagraph" | "part") {
                        let num = self.section_number(base);
                        if !num.is_empty() {
                            content.insert(0, Inline::Run(Run::new(format!("{num}  "))));
                        }
                    }
                    self.emit(Block::Paragraph(Paragraph::styled(merge_runs(content), h)));
                    i = k;
                    continue;
                }
            }
            match base {
                "maketitle" => {
                    self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                    if !self.title.is_empty() {
                        let t = self.title.clone();
                        let c = self.inline(&t, None);
                        self.emit(Block::Paragraph(Paragraph::styled(c, "title")));
                    }
                    if !self.author.is_empty() {
                        let a = self.author.clone();
                        for part in re(r"\\and\b").split(&a) {
                            let c = self.inline(part, None);
                            self.emit(Block::Paragraph(Paragraph::styled(c, "subtitle")));
                        }
                    }
                    if !self.date.is_empty() {
                        let d = self.date.clone();
                        let c = self.inline(&d, None);
                        self.emit(Block::Paragraph(Paragraph { content: c, align: Some("center".into()), ..Default::default() }));
                    }
                    i = j;
                    continue;
                }
                "newpage" | "clearpage" | "pagebreak" | "cleardoublepage" => {
                    self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                    self.emit(Block::PageBreak(PageBreak {}));
                    i = j;
                    continue;
                }
                "hrule" | "hrulefill" if buf.trim().is_empty() => {
                    self.emit(Block::Rule(Rule {}));
                    i = j;
                    continue;
                }
                "par" => {
                    self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
                    i = j;
                    continue;
                }
                "bibliography" | "printbibliography" | "addbibresource" => {
                    let k = read_optional(text, j).map(|x| x.1).unwrap_or(j);
                    i = read_group(text, k).map(|x| x.1).unwrap_or(k);
                    continue;
                }
                "verb" | "lstinline" => {
                    let m = re(r"^\*?(?:\[[^\]]*\])?(.)").captures(&text[j..]);
                    let mut k = j;
                    if let Some(m) = m {
                        let delim = m.get(1).unwrap();
                        let after = j + delim.end();
                        if let Some(e) = text[after..].find(delim.as_str()) {
                            k = after + e + delim.as_str().len();
                        }
                    }
                    buf.push_str(&text[i..k]);
                    i = k;
                    continue;
                }
                _ => {}
            }
            // anything else belongs to the paragraph (its arguments follow as ordinary text)
            buf.push_str(&text[i..j]);
            i = j;
        }
        self.flush(&mut buf, style, &align, &lst, indent, &mut first_item);
    }

    fn block_env(&mut self, env: &str, inner: &str, style: &str, align: &Option<String>, lst: &Option<(String, u32)>, indent: u32) -> bool {
        let base = env.trim_end_matches('*');
        match base {
            "itemize" | "enumerate" | "description" | "compactitem" | "compactenum" | "inparaenum" | "asparaenum" | "list" | "tasks" => {
                let level = lst.as_ref().map(|l| l.1 + 1).unwrap_or(0);
                let kind = if base.contains("enum") { "number" } else { "bullet" };
                for (label, body) in items(inner, "item") {
                    let body = match (&label, base) {
                        (Some(l), "description") => format!("\\textbf{{{l}}} {body}"),
                        (Some(l), _) => format!("{l} {body}"),
                        _ => body,
                    };
                    self.blocks(&body, style, align.clone(), Some((kind.into(), level.min(8))), indent);
                }
                true
            }
            "quote" | "quotation" | "verse" => {
                self.blocks(inner, "quote", align.clone(), None, indent);
                true
            }
            "center" | "flushleft" | "flushright" => {
                let a = match base { "center" => Some("center".into()), "flushright" => Some("right".into()), _ => None };
                self.blocks(inner, style, a, lst.clone(), indent);
                true
            }
            "abstract" => {
                self.emit(Block::Paragraph(Paragraph { content: vec![Inline::Run(Run { text: "Abstract".into(), b: true, ..Default::default() })], align: Some("center".into()), ..Default::default() }));
                self.blocks(inner, style, Some("justify".into()), None, indent);
                true
            }
            "verbatim" | "Verbatim" | "lstlisting" | "minted" | "alltt" | "code" | "BVerbatim" | "spverbatim" => {
                let mut lang = String::new();
                let mut body = inner.to_string();
                if base == "minted" {
                    if let Some((g, k)) = read_group(inner, 0) {
                        lang = g;
                        body = inner[k..].to_string();
                    }
                } else if let Some((opt, k)) = read_optional(inner, 0) {
                    if let Some(m) = re(r"language\s*=\s*\{?([\w+#-]+)").captures(&opt) {
                        lang = m[1].to_string();
                    }
                    body = inner[k..].to_string();
                }
                self.emit(Block::CodeBlock(CodeBlock { text: body.trim_matches('\n').into(), lang: lang.to_lowercase() }));
                true
            }
            "equation" | "align" | "alignat" | "gather" | "multline" | "eqnarray" | "flalign" | "displaymath" | "math" | "dmath" => {
                let mut tex = re(r"\\label\{[^}]*\}").replace_all(inner.trim(), "").into_owned();
                if matches!(base, "align" | "alignat" | "eqnarray" | "flalign" | "gather" | "multline") {
                    tex = format!("\\begin{{aligned}}\n{}\n\\end{{aligned}}", tex.replace("&=&", "&="));
                }
                self.emit(Block::MathBlock(MathBlock { tex }));
                true
            }
            "figure" | "table" | "wrapfigure" | "subfigure" | "SCfigure" | "sidewaysfigure" | "sidewaystable" | "minipage"
            | "multicols" | "titlepage" | "landscape" | "adjustbox" | "small" | "footnotesize" | "scriptsize" | "spacing"
            | "singlespace" | "onehalfspace" | "doublespace" | "frame" | "block" | "column" | "columns" | "subtable"
            | "threeparttable" | "resizebox" => {
                let mut inner = inner.to_string();
                if matches!(base, "wrapfigure" | "minipage" | "subfigure" | "subtable" | "multicols" | "column" | "resizebox") {
                    let k = read_optional(&inner, 0).map(|x| x.1).unwrap_or(0);
                    inner = match read_group(&inner, k) {
                        Some((_, k2)) => inner[k2..].to_string(),
                        None => inner[k..].to_string(),
                    };
                }
                if matches!(base, "figure" | "wrapfigure" | "sidewaysfigure" | "SCfigure") {
                    self.figures += 1;
                    self.caption_label = format!("Figure {}: ", self.figures);
                } else if matches!(base, "table" | "sidewaystable") {
                    self.tables += 1;
                    self.caption_label = format!("Table {}: ", self.tables);
                }
                let k = read_optional(&inner, 0).map(|x| x.1).unwrap_or(0);
                let a = if matches!(base, "figure" | "table") { Some("center".to_string()) } else { align.clone() };
                self.blocks(&inner[k..], style, a, lst.clone(), indent);
                true
            }
            "tabular" | "tabularx" | "tabulary" | "longtable" | "tabu" | "supertabular" | "xtabular" => {
                let mut k = 0;
                if matches!(base, "tabularx" | "tabulary" | "tabu") {
                    k = read_group(inner, 0).map(|x| x.1).unwrap_or(0);
                }
                k = read_optional(inner, k).map(|x| x.1).unwrap_or(k);
                let (spec, k) = read_group(inner, k).unwrap_or((String::new(), k));
                let t = self.table(&inner[k..], &spec);
                self.emit(Block::Table(t));
                true
            }
            "thebibliography" => {
                let k = read_group(inner, 0).map(|x| x.1).unwrap_or(0);
                self.emit(Block::Paragraph(Paragraph::styled(vec![Inline::Run(Run::new("References"))], "h1")));
                for (n, (_label, body)) in items(&inner[k..], "bibitem").into_iter().enumerate() {
                    let body = match read_group(&body, 0) {
                        Some((_, k2)) => body[k2..].to_string(),
                        None => body,
                    };
                    self.blocks(&format!("[{}] {}", n + 1, body), "normal", None, None, indent);
                }
                true
            }
            "tikzpicture" | "pgfpicture" | "picture" | "pspicture" | "circuitikz" | "forest" | "axis" => {
                self.emit(Block::Paragraph(Paragraph {
                    content: vec![Inline::Run(Run { text: format!("[{base} drawing]"), i: true, color: Some("#7F7F7F".into()), ..Default::default() })],
                    align: Some("center".into()),
                    ..Default::default()
                }));
                true
            }
            "comment" => true,
            "document" => {
                self.blocks(inner, style, align.clone(), lst.clone(), indent);
                true
            }
            "theorem" | "lemma" | "proof" | "definition" | "corollary" | "proposition" | "example" | "remark" | "note"
            | "claim" | "conjecture" | "exercise" | "solution" | "assumption" | "observation" | "fact" | "hypothesis"
            | "notation" | "problem" | "question" | "answer" | "acknowledgments" | "acknowledgements" | "IEEEkeywords"
            | "keywords" | "acks" => {
                let (opt, k) = read_optional(inner, 0).map(|(o, k)| (Some(o), k)).unwrap_or((None, 0));
                let mut label = base[..1].to_uppercase() + &base[1..];
                if base == "IEEEkeywords" {
                    label = "Keywords".into();
                }
                let head = if base == "proof" {
                    format!("\\textit{{Proof{}.}} ", opt.as_ref().map(|o| format!(" of {o}")).unwrap_or_default())
                } else {
                    format!("\\textbf{{{label}}}{}. ", opt.as_ref().map(|o| format!(" ({o})")).unwrap_or_default())
                };
                self.blocks(&format!("{head}{}", &inner[k..]), style, align.clone(), lst.clone(), indent);
                true
            }
            _ => {
                self.blocks(inner, style, align.clone(), lst.clone(), indent);
                true
            }
        }
    }

    fn table(&mut self, body: &str, spec: &str) -> Table {
        let body = re(r"\\(hline|toprule|midrule|bottomrule|endhead|endfirsthead|endfoot|endlastfoot)\b").replace_all(body, "");
        let body = re(r"\\c(?:line|midrule)(?:\([^)]*\))?\{[^}]*\}").replace_all(&body, "").into_owned();
        let clean_spec = re(r"\{[^}]*\}|@|>|<|\|").replace_all(spec, "");
        let aligns: Vec<Option<String>> = clean_spec.chars().filter(|c| "lcrpmbXJLCR".contains(*c))
            .map(|c| match c { 'c' | 'C' => Some("center".into()), 'r' | 'R' => Some("right".into()), _ => None }).collect();
        let lead = re(r"^\s*\[[^\]]*\]");
        let mut rows = vec![];
        for row in split_top(&body, "\\\\") {
            let row = lead.replace(&row, "").into_owned();
            if row.trim().is_empty() {
                continue;
            }
            let mut cells: Vec<Vec<Paragraph>> = vec![];
            for cell in split_top(&row, "&") {
                let mut cell = cell.trim().to_string();
                let mut span = 1;
                if let Some(m) = re(r"^\\multicolumn\s*\{(\d+)\}").captures(&cell) {
                    span = m[1].parse().unwrap_or(1);
                    let end = m.get(0).unwrap().end();
                    let k = read_group(&cell, end).map(|x| x.1).unwrap_or(end);
                    cell = read_group(&cell, k).map(|x| x.0).unwrap_or_default();
                }
                if let Some(m) = re(r"^\\multirow\s*\{[^}]*\}\s*\{[^}]*\}").find(&cell) {
                    cell = read_group(&cell, m.end()).map(|x| x.0).unwrap_or_default();
                }
                let content = self.inline(&cell, None);
                let a = aligns.get(cells.len()).cloned().flatten();
                cells.push(vec![Paragraph { content: strip_content(content), align: a, ..Default::default() }]);
                for _ in 1..span {
                    cells.push(vec![Paragraph::default()]);
                }
            }
            rows.push(cells);
        }
        let header = rows.len() > 1;
        Table { rows, header, widths: None }
    }

    pub fn inline(&mut self, text: &str, fmt: Option<Run>) -> Vec<Inline> {
        let mut fmt = fmt.unwrap_or_default();
        let mut out: Vec<Inline> = vec![];
        let mut buf = String::new();
        let n = text.len();
        let mut i = 0;
        let ws = re(r"[ \t\n]+");
        let flush = |buf: &mut String, out: &mut Vec<Inline>, fmt: &Run| {
            if buf.is_empty() {
                return;
            }
            let s = ws.replace_all(buf, " ").into_owned();
            let s = s.replace("---", "—").replace("--", "–").replace("``", "“").replace("''", "”").replace('`', "‘").replace('\'', "’").replace('~', "\u{a0}");
            out.push(Inline::Run(fmt.styled(s)));
            buf.clear();
        };
        while i < n {
            let c = text[i..].chars().next().unwrap();
            if c == '{' {
                let (g, k) = read_group(text, i).unwrap_or((String::new(), n));
                flush(&mut buf, &mut out, &fmt);
                let inner = self.inline(&g, Some(fmt.styled("")));
                out.extend(inner);
                i = k;
                continue;
            }
            if c == '}' {
                i += 1;
                continue;
            }
            if c == '$' {
                if text[i..].starts_with("$$") {
                    if let Some(e) = text[i + 2..].find("$$") {
                        flush(&mut buf, &mut out, &fmt);
                        let mut r = fmt.styled(text[i + 2..i + 2 + e].trim());
                        r.math = true;
                        out.push(Inline::Run(r));
                        i = i + 2 + e + 2;
                        continue;
                    }
                }
                // $...$ with escaped characters inside
                let b = text.as_bytes();
                let mut k = i + 1;
                while k < n && b[k] != b'$' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
                if k < n && k > i + 1 {
                    flush(&mut buf, &mut out, &fmt);
                    let mut r = fmt.styled(text[i + 1..k].trim());
                    r.math = true;
                    out.push(Inline::Run(r));
                    i = k + 1;
                    continue;
                }
            }
            if c != '\\' {
                buf.push(c);
                i += c.len_utf8();
                continue;
            }
            let (name, j) = read_command(text, i);
            let base = name.trim_end_matches('*').to_string();
            if name == "(" {
                let end = text[j..].find("\\)").map(|e| j + e).unwrap_or(n);
                flush(&mut buf, &mut out, &fmt);
                let mut r = fmt.styled(text[j..end].trim());
                r.math = true;
                out.push(Inline::Run(r));
                i = (end + 2).min(n);
                continue;
            }
            if name == "\\" || matches!(base.as_str(), "newline" | "linebreak" | "break") {
                let k = read_optional(text, j).map(|x| x.1).unwrap_or(j);
                flush(&mut buf, &mut out, &fmt);
                out.push(Inline::Break(Break::default()));
                i = k;
                continue;
            }
            if let Some(acc) = accent(&base) {
                let letter_cmd = base.len() == 1 && base.chars().next().unwrap().is_ascii_alphabetic();
                let follows = text[j..].starts_with('{') || text[j..].starts_with(' ');
                if !letter_cmd || follows {
                    let (g, k) = match read_group(text, j) {
                        Some(x) => x,
                        None => {
                            let mut k = j;
                            while k < n && text.as_bytes()[k] == b' ' {
                                k += 1;
                            }
                            let ch = text[k..].chars().next().map(|c| c.to_string()).unwrap_or_default();
                            let l = ch.len();
                            (ch, k + l)
                        }
                    };
                    let letter = g.trim().trim_start_matches('\\').to_string();
                    let mut chars = letter.chars();
                    if let Some(first) = chars.next() {
                        let composed: String = format!("{first}{acc}").nfc().collect();
                        buf.push_str(&composed);
                        buf.push_str(chars.as_str());
                    }
                    i = k;
                    continue;
                }
            }
            if let Some(sym) = symbol(&base) {
                buf.push_str(sym);
                i = j;
                if base.chars().all(|c| c.is_ascii_alphabetic()) && text[j..].starts_with(' ') {
                    i += 1;
                }
                if text[j..].starts_with("{}") {
                    i = j + 2;
                }
                continue;
            }
            if base == "today" {
                buf.push_str(&today());
                i = j;
                continue;
            }
            if base == "verb" || base == "lstinline" {
                if let Some(m) = re(r"^\*?(?:\[[^\]]*\])?(.)").captures(&text[j..]) {
                    let d = m.get(1).unwrap();
                    let after = j + d.end();
                    if let Some(e) = text[after..].find(d.as_str()) {
                        flush(&mut buf, &mut out, &fmt);
                        let mut r = fmt.styled(&text[after..after + e]);
                        r.code = true;
                        out.push(Inline::Run(r));
                        i = after + e + d.as_str().len();
                        continue;
                    }
                }
            }
            if matches!(base.as_str(), "bfseries" | "bf" | "itshape" | "it" | "slshape" | "sl" | "ttfamily" | "tt" | "em") {
                flush(&mut buf, &mut out, &fmt);
                fmt = fmt.styled("");
                if base == "em" {
                    fmt.i = !fmt.i;
                } else {
                    set_fmt(&mut fmt, style_cmd(&base).unwrap_or(""));
                }
                i = j;
                continue;
            }
            if style_cmd(&base).is_some() || base == "emph" {
                if let Some((g, k)) = read_group(text, j) {
                    flush(&mut buf, &mut out, &fmt);
                    let mut f2 = fmt.styled("");
                    if base == "emph" {
                        f2.i = !f2.i;
                    } else {
                        set_fmt(&mut f2, style_cmd(&base).unwrap());
                    }
                    let inner = self.inline(&g, Some(f2));
                    out.extend(inner);
                    i = k;
                    continue;
                }
            }
            if matches!(base.as_str(), "textcolor" | "color" | "colorbox" | "hl" | "highlight") {
                let (opt, k) = read_optional(text, j).map(|(o, k)| (Some(o), k)).unwrap_or((None, j));
                let (col, k2) = if matches!(base.as_str(), "hl" | "highlight") {
                    ("yellow".to_string(), k)
                } else {
                    read_group(text, k).unwrap_or((String::new(), k))
                };
                if base == "color" {
                    flush(&mut buf, &mut out, &fmt);
                    fmt = fmt.styled("");
                    fmt.color = color(&col, opt.as_deref());
                    i = k2;
                    continue;
                }
                if let Some((g, k3)) = read_group(text, k2) {
                    flush(&mut buf, &mut out, &fmt);
                    let mut f2 = fmt.styled("");
                    if base == "textcolor" {
                        f2.color = color(&col, opt.as_deref());
                    } else {
                        f2.hl = Some(color(&col, opt.as_deref()).unwrap_or_else(|| "#FFFF00".into()));
                    }
                    let inner = self.inline(&g, Some(f2));
                    out.extend(inner);
                    i = k3;
                    continue;
                }
            }
            if matches!(base.as_str(), "href" | "url" | "nolinkurl") {
                if let Some((url, k)) = read_group(text, j) {
                    flush(&mut buf, &mut out, &fmt);
                    let url = url.replace('\\', "");
                    if base == "href" {
                        let (g, k2) = read_group(text, k).unwrap_or((url.clone(), k));
                        let mut f2 = fmt.styled("");
                        f2.link = Some(url);
                        let inner = self.inline(&g, Some(f2));
                        out.extend(inner);
                        i = k2;
                    } else {
                        let mut r = fmt.styled(&url);
                        r.link = Some(url);
                        out.push(Inline::Run(r));
                        i = k;
                    }
                    continue;
                }
            }
            if matches!(base.as_str(), "footnote" | "footnotetext" | "thanks" | "endnote") {
                let k = read_optional(text, j).map(|x| x.1).unwrap_or(j);
                if let Some((g, k2)) = read_group(text, k) {
                    flush(&mut buf, &mut out, &fmt);
                    let runs = self.inline(&g, None);
                    out.push(Inline::Note(Note { runs }));
                    i = k2;
                    continue;
                }
            }
            if cite_re().is_match(&base) {
                let mut opts = vec![];
                let mut k = j;
                for _ in 0..2 {
                    match read_optional(text, k) {
                        Some((o, k2)) => {
                            opts.push(o);
                            k = k2;
                        }
                        None => break,
                    }
                }
                if let Some((keys, k2)) = read_group(text, k) {
                    let note = opts.last().filter(|o| !o.is_empty()).map(|o| format!(", {o}")).unwrap_or_default();
                    buf.push_str(&format!("[{}{}]", keys.split(',').map(|x| x.trim()).collect::<Vec<_>>().join(", "), note));
                    i = k2;
                    continue;
                }
            }
            if ref_re().is_match(&base) {
                if let Some((g, k)) = read_group(text, j) {
                    let labels: Vec<String> = g.split(',').map(|x| self.labels.get(x.trim()).cloned().unwrap_or_else(|| "??".into())).collect();
                    let mut s = labels.join(", ");
                    if base == "eqref" {
                        s = format!("({s})");
                    } else if matches!(base.as_str(), "autoref" | "Autoref" | "cref" | "Cref") {
                        let kind = g.split(':').next().unwrap_or("").trim().to_lowercase();
                        let word = match kind.as_str() {
                            "sec" => "Section", "fig" => "Figure", "tab" => "Table", "eq" => "Equation", "ch" => "Chapter",
                            "app" => "Appendix", "alg" => "Algorithm", "thm" => "Theorem", "lem" => "Lemma", _ => "",
                        };
                        if g.contains(':') && !word.is_empty() {
                            s = format!("{word} {s}");
                        }
                    }
                    buf.push_str(&s);
                    i = k;
                    continue;
                }
            }
            if base == "includegraphics" {
                let (opt, k) = read_optional(text, j).unwrap_or((String::new(), j));
                if let Some((path, k2)) = read_group(text, k) {
                    flush(&mut buf, &mut out, &fmt);
                    match self.image(path.trim(), &opt) {
                        Some(img) => out.push(Inline::Image(img)),
                        None => out.push(Inline::Run(Run {
                            text: format!("[image: {}]", Path::new(path.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                            i: true,
                            color: Some("#7F7F7F".into()),
                            ..Default::default()
                        })),
                    }
                    i = k2;
                    continue;
                }
            }
            if base == "caption" {
                let k = read_optional(text, j).map(|x| x.1).unwrap_or(j);
                if let Some((g, k2)) = read_group(text, k) {
                    flush(&mut buf, &mut out, &fmt);
                    let label = std::mem::take(&mut self.caption_label);
                    let mut cap = vec![Inline::Run(Run { text: label, b: true, ..Default::default() })];
                    cap.extend(self.inline(&g, None));
                    self.captions.push(cap);
                    i = k2;
                    continue;
                }
            }
            if base == "frac" {
                if let (Some((a, k)), true) = (read_group(text, j), true) {
                    if let Some((bb, k2)) = read_group(text, k) {
                        buf.push_str(&format!("{}/{}", plain(&a), plain(&bb)));
                        i = k2;
                        continue;
                    }
                }
            }
            if DROP.contains(&base.as_str()) {
                let mut k = j;
                for _ in 0..3 {
                    if let Some((_, k2)) = read_optional(text, k) {
                        k = k2;
                        continue;
                    }
                    if NO_ARGS.contains(&base.as_str()) {
                        break;
                    }
                    match read_group(text, k) {
                        Some((_, k2)) => k = k2,
                        None => break,
                    }
                }
                i = k;
                continue;
            }
            // an unknown command: drop it, keep what is in its arguments
            let mut k = j;
            let mut parts = vec![];
            loop {
                if let Some((_, k2)) = read_optional(text, k) {
                    k = k2;
                    continue;
                }
                match read_group(text, k) {
                    Some((g, k2)) => {
                        parts.push(g);
                        k = k2;
                    }
                    None => break,
                }
            }
            let skip = ["newcommand", "renewcommand", "def", "usepackage", "documentclass", "setcounter", "newtheorem",
                        "geometry", "hypersetup", "definecolor", "graphicspath", "pgfplotsset", "usetikzlibrary", "lstset",
                        "author", "title", "date"];
            if let Some(last) = parts.last() {
                if !skip.contains(&base.as_str()) {
                    flush(&mut buf, &mut out, &fmt);
                    let inner = self.inline(last, Some(fmt.clone()));
                    out.extend(inner);
                }
            }
            i = k;
            if parts.is_empty() && i < n && text.as_bytes()[i] == b' ' && name.chars().all(|c| c.is_ascii_alphabetic()) {
                i += 1;
            }
        }
        flush(&mut buf, &mut out, &fmt);
        // no double spaces where two runs meet
        let mut tidy: Vec<Inline> = vec![];
        for x in out {
            if let (Inline::Run(r), Some(Inline::Run(prev))) = (&x, tidy.last()) {
                if prev.text.ends_with(' ') && r.text.starts_with(' ') {
                    tidy.push(Inline::Run(r.styled(r.text.trim_start_matches(' '))));
                    continue;
                }
            }
            tidy.push(x);
        }
        merge_runs(tidy)
    }

    fn image(&self, path: &str, opts: &str) -> Option<Image> {
        let base = self.base_dir?;
        let cands: Vec<String> = if Path::new(path).extension().is_some() {
            vec![path.to_string()]
        } else {
            [".png", ".jpg", ".jpeg", ".pdf"].iter().map(|e| format!("{path}{e}")).collect()
        };
        for cand in cands {
            let full = base.join(&cand);
            let Ok(mut data) = std::fs::read(&full) else { continue };
            if full.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                match self.pdf_figure.and_then(|f| f(&data)) {
                    Some(png) => data = png,
                    None => continue,
                }
            }
            let ext = imaging::sniff(&data);
            if !imaging::SHOWN.contains(&ext) {
                continue;
            }
            let (mut w, mut h) = imaging::natural_size(&data, Some(451.0));
            if let Some(m) = re(r"width\s*=\s*([\d.]*)\s*\\(?:text|line|column)width").captures(opts) {
                let k: f64 = m[1].parse().unwrap_or(1.0);
                h = h * 451.0 * k / w;
                w = 451.0 * k;
            } else {
                if let Some(m) = re(r"width\s*=\s*([\d.]+)\s*(cm|mm|in|pt)").captures(opts) {
                    let unit = match &m[2] { "cm" => 28.35, "mm" => 2.835, "in" => 72.0, _ => 1.0 };
                    let nw = m[1].parse::<f64>().unwrap_or(1.0) * unit;
                    h = h * nw / w;
                    w = nw;
                }
                if let Some(m) = re(r"scale\s*=\s*([\d.]+)").captures(opts) {
                    let s: f64 = m[1].parse().unwrap_or(1.0);
                    w *= s;
                    h *= s;
                }
            }
            return Some(Image { data, ext: ext.into(), width: w, height: h, alt: String::new(), name: full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() });
        }
        None
    }
}

/// Today's date as LaTeX writes it, e.g. "October 24, 2026".
fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September",
                                "October", "November", "December"];
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

/// (optional label, body) for each \item of a list body (nested lists stay in their item).
fn items(inner: &str, word: &str) -> Vec<(Option<String>, String)> {
    let pat = re(&format!(r"\\(begin|end)\s*\{{[^}}]*\}}|\\{word}([^A-Za-z]|$)"));
    let mut out = vec![];
    let mut depth = 0;
    let mut cur: Option<(Option<String>, usize)> = None;
    for m in pat.captures_iter(inner) {
        let whole = m.get(0).unwrap();
        match m.get(1).map(|x| x.as_str()) {
            Some("begin") => depth += 1,
            Some("end") => depth -= 1,
            _ if depth == 0 => {
                if let Some((label, start)) = cur.take() {
                    out.push((label, inner[start..whole.start()].to_string()));
                }
                let end = whole.start() + 1 + word.len();
                let (label, k) = match read_optional(inner, end) {
                    Some((l, k)) => (Some(l), k),
                    None => (None, end),
                };
                cur = Some((label, k));
            }
            _ => {}
        }
    }
    if let Some((label, start)) = cur {
        out.push((label, inner[start.min(inner.len())..].to_string()));
    }
    out
}

pub fn parse(src: &str, base_dir: Option<&Path>, pdf_figure: Option<PdfFigure>) -> Document {
    let src = expand_macros(&strip_comments(src));
    let pre_end = src.find("\\begin{document}");
    let preamble = pre_end.map(|e| &src[..e]).unwrap_or("");
    let body_start = pre_end.map(|e| e + "\\begin{document}".len()).unwrap_or(0);
    let body_end = src.find("\\end{document}").unwrap_or(src.len());
    let mut body = src[body_start..body_end.max(body_start)].to_string();
    let all = format!("{preamble}{body}");
    let meta = |cmd: &str| -> String {
        let m = re(&format!(r"\\{cmd}\s*(?:\[[^\]]*\])?\s*\{{")).find(&all);
        m.and_then(|m| read_group(&all, m.end() - 1)).map(|x| x.0).unwrap_or_default()
    };
    let (title, author, date) = (meta("title"), meta("author"), meta("date"));
    for cmd in ["title", "author", "date"] {
        let pat = re(&format!(r"\\{cmd}\s*(?:\[[^\]]*\])?\s*\{{"));
        while let Some(m) = pat.find(&body.clone()) {
            let end = read_group(&body, m.end() - 1).map(|x| x.1).unwrap_or(m.end());
            body.replace_range(m.start()..end, "");
        }
    }
    let mut r = Reader::new(base_dir, body.contains("\\chapter"), title.clone(), author.clone(), date);
    r.pdf_figure = pdf_figure;
    r.number_labels(&body);
    r.blocks(&body, "normal", None, None, 0);
    let first_author = re(r"\\and\b").split(&author).next().unwrap_or("").to_string();
    let mut doc = Document { blocks: std::mem::take(&mut r.out), title: plain(&title), author: plain(&first_author), ..Default::default() };
    if !title.is_empty() && !body.contains("\\maketitle") && !doc.blocks.iter().any(|b| matches!(b, Block::Paragraph(p) if p.style == "title")) {
        let c = r.inline(&title, None);
        doc.blocks.insert(0, Block::Paragraph(Paragraph::styled(c, "title")));
    }
    doc
}

// -- writing ------------------------------------------------------------------------

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\u{a0}' => out.push('~'),
            c => out.push(c),
        }
    }
    out
}

fn needs_unicode_engine(text: &str) -> bool {
    text.chars().any(|c| c as u32 > 0x24F && !"–—‘’“”…•€™©®°±×÷−≤≥≠∞→←·§¶†‡\u{a0}\u{200b}".contains(c))
}

struct W<'a> {
    image_dir: String,
    sentences: Option<&'a dyn Fn(&str) -> Vec<usize>>,
    files: Files,
    uses: std::collections::BTreeSet<&'static str>,
    n: usize,
}

impl<'a> W<'a> {
    fn run(&mut self, r: &Run) -> String {
        if r.math {
            return format!("${}$", r.text);
        }
        let mut s = escape(&r.text);
        if r.code {
            s = format!("\\texttt{{{s}}}");
        }
        if r.sup {
            s = format!("\\textsuperscript{{{s}}}");
        }
        if r.sub {
            s = format!("\\textsubscript{{{s}}}");
        }
        if r.s {
            s = format!("\\sout{{{s}}}");
            self.uses.insert("ulem");
        }
        if r.u && r.link.is_none() {
            s = format!("\\uline{{{s}}}");
            self.uses.insert("ulem");
        }
        if r.i {
            s = format!("\\emph{{{s}}}");
        }
        if r.b {
            s = format!("\\textbf{{{s}}}");
        }
        if let Some(c) = &r.color {
            if !matches!(c.to_uppercase().as_str(), "#000000" | "#1A1A1A") {
                s = format!("\\textcolor[HTML]{{{}}}{{{s}}}", c.trim_start_matches('#').to_uppercase());
                self.uses.insert("color");
            }
        }
        if let Some(h) = &r.hl {
            s = format!("\\colorbox[HTML]{{{}}}{{{s}}}", h.trim_start_matches('#').to_uppercase());
            self.uses.insert("color");
        }
        if let Some(l) = &r.link {
            let url = l.replace('%', "\\%").replace('#', "\\#");
            s = if r.text.trim() == l.trim() { format!("\\url{{{url}}}") } else { format!("\\href{{{url}}}{{{s}}}") };
        }
        s
    }

    fn inline(&mut self, content: &[Inline]) -> String {
        let mut out = String::new();
        for x in merge_runs(content.to_vec()) {
            match &x {
                Inline::Run(r) => {
                    if r.text.trim().is_empty() {
                        out.push_str(&escape(&r.text));
                    } else if r.math || r.code {
                        out.push_str(&self.run(r));
                    } else {
                        let lead = &r.text[..r.text.len() - r.text.trim_start().len()];
                        let trail = &r.text[r.text.trim_end().len()..];
                        out.push_str(&escape(lead));
                        out.push_str(&self.run(&r.styled(r.text.trim())));
                        out.push_str(&escape(trail));
                    }
                }
                Inline::Break(_) => out.push_str("\\\\\n"),
                Inline::Note(n) => {
                    let t = self.inline(&n.runs);
                    out.push_str(&format!("\\footnote{{{t}}}"));
                }
                Inline::Image(i) => out.push_str(&self.image(i)),
                Inline::Chart(c) => {
                    self.uses.insert("pgfplots");
                    out.push('\n');
                    out.push_str(&charts::pgfplots(c, &escape));
                    out.push('\n');
                }
                Inline::Opaque(o) => out.push_str(&escape(&o.text)),
            }
        }
        if out.ends_with("\\\\\n") {
            out.truncate(out.len() - 3);
        }
        out
    }

    fn image(&mut self, img: &Image) -> String {
        let mut data = img.data.clone();
        let mut ext = if img.ext.is_empty() { imaging::sniff(&img.data).to_string() } else { img.ext.clone() };
        if ext != "png" && ext != "jpeg" {
            match imaging::to_png(&data) {
                Ok(p) => {
                    data = p;
                    ext = "png".into();
                }
                Err(_) => return format!("[{}]", escape(if img.alt.is_empty() { "picture" } else { &img.alt })),
            }
        }
        self.n += 1;
        let stem = Path::new(if img.name.is_empty() { "image" } else { &img.name }).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
        let clean = clean.trim_matches('-');
        let path = format!("{}/{:02}-{}.{}", self.image_dir, self.n, if clean.is_empty() { "image" } else { clean }, if ext == "jpeg" { "jpg" } else { "png" });
        self.files.insert(path.clone(), data);
        self.uses.insert("graphicx");
        let w = if img.width > 0.0 { img.width } else { 300.0 };
        format!("\\includegraphics[width={:.2}\\linewidth]{{{path}}}", (w / 451.0).min(1.0))
    }

    fn split(&self, text: &str) -> String {
        match self.sentences {
            Some(f) => crate::md::split_lines(text, &f(text)),
            None => text.to_string(),
        }
    }
}

fn table_tex(t: &Table, w: &mut W, caption: &str) -> String {
    let n = t.ncols();
    if n == 0 {
        return String::new();
    }
    let mut aligns = vec![];
    for c in 0..n {
        let rows = if t.rows.len() > 1 { &t.rows[1..] } else { &t.rows[..] };
        let a = rows.iter().find_map(|r| r.get(c).and_then(|cell| cell.first()).and_then(|p| p.align.clone()));
        aligns.push(match a.as_deref() { Some("center") => "c", Some("right") => "r", _ => "l" });
    }
    let long_text = t.rows.iter().any(|r| r.iter().any(|c| c.iter().map(|p| p.text()).collect::<Vec<_>>().join(" ").chars().count() > 40));
    let spec = if long_text {
        format!("|{}|", vec![format!("p{{{:.2}\\linewidth}}", 0.9 / n as f64); n].join("|"))
    } else {
        format!("|{}|", aligns.join("|"))
    };
    let mut lines = vec!["\\begin{table}[htbp]".to_string(), "  \\centering".to_string()];
    if !caption.is_empty() {
        lines.push(format!("  \\caption{{{caption}}}"));
    }
    lines.push(format!("  \\begin{{tabular}}{{{spec}}}"));
    lines.push("    \\hline".into());
    for (r, row) in t.rows.iter().enumerate() {
        let mut cells = vec![];
        for c in 0..n {
            let paras = row.get(c).cloned().unwrap_or_default();
            let mut text = paras.iter().map(|p| w.inline(&p.content)).collect::<Vec<_>>().join(" ").replace("\\\\\n", " ");
            if t.header && r == 0 && !text.trim().is_empty() {
                text = format!("\\textbf{{{text}}}");
            }
            cells.push(text);
        }
        lines.push(format!("    {} \\\\ \\hline", cells.join(" & ")));
    }
    lines.push("  \\end{tabular}".into());
    lines.push("\\end{table}".into());
    lines.join("\n")
}

/// LaTeX source and the files it includes.
pub fn write(doc: &Document, stem: &str, sentences: Option<&dyn Fn(&str) -> Vec<usize>>) -> (String, Files) {
    let mut w = W { image_dir: format!("{stem}-images"), sentences, files: Files::new(), uses: Default::default(), n: 0 };
    let mut body: Vec<String> = vec![];
    let mut stack: Vec<(String, u32)> = vec![];
    let mut title = doc.title.clone();
    let mut authors: Vec<String> = vec![];
    let mut blocks: &[Block] = &doc.blocks;
    if let Some(Block::Paragraph(p)) = blocks.first() {
        if p.style == "title" {
            title = p.text().trim().to_string();
            blocks = &blocks[1..];
            while let Some(Block::Paragraph(p)) = blocks.first() {
                if p.style != "subtitle" {
                    break;
                }
                authors.push(p.text().trim().to_string());
                blocks = &blocks[1..];
            }
        }
    }
    let num_re = re(r"^\d+(\.\d+)*\.?\s+");
    let numbered = blocks.iter().any(|b| matches!(b, Block::Paragraph(p) if p.style.starts_with('h') && num_re.is_match(p.text().trim())));
    let close = |stack: &mut Vec<(String, u32)>, body: &mut Vec<String>, level: i64| {
        while stack.last().is_some_and(|s| s.1 as i64 > level) {
            let (kind, _) = stack.pop().unwrap();
            body.push(format!("{}{}", "  ".repeat(stack.len()), if kind == "number" { "\\end{enumerate}" } else { "\\end{itemize}" }));
        }
    };
    let mut k = 0;
    while k < blocks.len() {
        let b = &blocks[k];
        k += 1;
        if let Block::Paragraph(p) = b {
            if let Some((kind, level)) = &p.list {
                while let Some(top) = stack.last() {
                    if top.1 > *level || (top.1 == *level && &top.0 != kind) {
                        let lv = top.1 as i64 - 1;
                        close(&mut stack, &mut body, lv);
                    } else {
                        break;
                    }
                }
                while stack.last().map(|s| s.1 as i64).unwrap_or(-1) < *level as i64 {
                    let next = stack.last().map(|s| s.1 + 1).unwrap_or(0);
                    let k2 = if next == *level { kind.clone() } else { "bullet".to_string() };
                    body.push(format!("{}{}", "  ".repeat(stack.len()), if k2 == "number" { "\\begin{enumerate}" } else { "\\begin{itemize}" }));
                    stack.push((k2, next));
                }
                let t = w.inline(&p.content);
                let t = w.split(&t);
                body.push(format!("{}\\item {}", "  ".repeat(stack.len()), t));
                continue;
            }
        }
        close(&mut stack, &mut body, -1);
        match b {
            Block::Paragraph(p) => {
                let text = w.inline(&p.content);
                let only: Vec<&Inline> = p.content.iter().filter(|x| !matches!(x, Inline::Run(r) if r.text.trim().is_empty())).collect();
                if only.len() == 1 && matches!(only[0], Inline::Image(_) | Inline::Chart(_)) {
                    let mut cap = String::new();
                    if let Some(Block::Paragraph(c)) = blocks.get(k) {
                        if c.style == "caption" {
                            cap = w.inline(&caption_text(&c.content)).trim().to_string();
                            k += 1;
                        }
                    }
                    body.push(format!("\\begin{{figure}}[htbp]\n  \\centering\n  {}{}\n\\end{{figure}}", text.trim(),
                                      if cap.is_empty() { String::new() } else { format!("\n  \\caption{{{cap}}}") }));
                    continue;
                }
                if p.style == "caption" {
                    if let Some(Block::Table(t)) = blocks.get(k) {
                        let cap = w.inline(&caption_text(&p.content)).trim().to_string();
                        body.push(table_tex(t, &mut w, &cap));
                        k += 1;
                        continue;
                    }
                }
                if text.trim().is_empty() {
                    continue;
                }
                match p.style.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let cmd = match p.style.as_str() { "h1" => "section", "h2" => "subsection", "h3" => "subsubsection", "h4" => "paragraph", _ => "subparagraph" };
                        let mut t = text.trim().to_string();
                        let mut star = "*";
                        if numbered {
                            if let Some(m) = num_re.find(&t.clone()) {
                                t = t[m.end()..].to_string();
                                star = "";
                            }
                        }
                        body.push(format!("\\{cmd}{star}{{{t}}}"));
                    }
                    "title" => body.push(format!("{{\\centering\\LARGE {text}\\par}}")),
                    "subtitle" => body.push(format!("{{\\centering\\large {text}\\par}}")),
                    "quote" => body.push(format!("\\begin{{quote}}\n{}\n\\end{{quote}}", w.split(&text))),
                    "caption" => body.push(format!("{{\\centering\\small\\emph{{{text}}}\\par}}")),
                    _ => match p.align.as_deref() {
                        Some("center") => body.push(format!("\\begin{{center}}\n{}\n\\end{{center}}", w.split(&text))),
                        Some("right") => body.push(format!("\\begin{{flushright}}\n{}\n\\end{{flushright}}", w.split(&text))),
                        _ => body.push(w.split(&text)),
                    },
                }
            }
            Block::Table(t) => {
                let mut cap = String::new();
                if let Some(Block::Paragraph(c)) = blocks.get(k) {
                    if c.style == "caption" {
                        cap = w.inline(&caption_text(&c.content)).trim().to_string();
                        k += 1;
                    }
                }
                body.push(table_tex(t, &mut w, &cap));
            }
            Block::CodeBlock(c) => {
                if c.text.contains("\\end{verbatim}") {
                    w.uses.insert("listings");
                    body.push(format!("\\begin{{lstlisting}}\n{}\n\\end{{lstlisting}}", c.text));
                } else {
                    body.push(format!("\\begin{{verbatim}}\n{}\n\\end{{verbatim}}", c.text));
                }
            }
            Block::MathBlock(m) => body.push(format!("\\[\n{}\n\\]", m.tex.trim())),
            Block::Rule(_) => body.push("\\noindent\\rule{\\linewidth}{0.4pt}".into()),
            Block::PageBreak(_) => body.push("\\newpage".into()),
        }
    }
    close(&mut stack, &mut body, -1);
    let all = format!("{}{}{}", body.join("\n"), title, authors.join(" "));
    let xe = needs_unicode_engine(&all);
    let m = doc.page.margins;
    let paper = match doc.page.name().as_str() { "A4" => "a4paper", "Letter" => "letterpaper", "Legal" => "legalpaper", "A5" => "a5paper", _ => "" };
    let mut pre = vec![];
    if xe {
        pre.push("% !TEX program = xelatex".to_string());
    }
    pre.push(format!("\\documentclass[{}{}]{{article}}", if doc.size < 11.5 { "11pt" } else { "12pt" }, if paper.is_empty() { String::new() } else { format!(",{paper}") }));
    if xe {
        pre.push("\\usepackage{fontspec}".into());
    } else {
        pre.extend(["\\usepackage[utf8]{inputenc}".to_string(), "\\usepackage[T1]{fontenc}".into(), "\\usepackage{lmodern}".into(), "\\usepackage{textcomp}".into()]);
    }
    pre.push(format!("\\usepackage[top={:.2}in,bottom={:.2}in,left={:.2}in,right={:.2}in]{{geometry}}", m.0 / 72.0, m.2 / 72.0, m.3 / 72.0, m.1 / 72.0));
    pre.push("\\usepackage{amsmath,amssymb}".into());
    pre.push("\\usepackage{graphicx}".into());
    if w.uses.contains("ulem") {
        pre.push("\\usepackage[normalem]{ulem}".into());
    }
    if w.uses.contains("color") || w.uses.contains("pgfplots") {
        pre.push("\\usepackage[table]{xcolor}".into());
    }
    if w.uses.contains("listings") {
        pre.push("\\usepackage{listings}".into());
    }
    if w.uses.contains("pgfplots") {
        pre.push("\\usepackage{pgfplots}".into());
        pre.push("\\pgfplotsset{compat=1.16}".into());
    }
    pre.push("\\usepackage{hyperref}".into());
    pre.push("\\hypersetup{colorlinks=true,linkcolor=blue,urlcolor=blue}".into());
    if !title.is_empty() {
        pre.push(format!("\\title{{{}}}", escape(&title)));
        pre.push(format!("\\author{{{}}}", authors.iter().map(|a| escape(a)).collect::<Vec<_>>().join(" \\and ")));
        pre.push("\\date{}".into());
    }
    let out = format!("{}\n\n\\begin{{document}}\n{}{}\n\n\\end{{document}}\n", pre.join("\n"),
                      if title.is_empty() { "\n" } else { "\\maketitle\n\n" }, body.join("\n\n"));
    (out, w.files)
}
