//! A document in the form a text editor holds it: each paragraph on a line of its own, character
//! formatting as spans over the text, paragraph formatting per line, and everything that is not text
//! (pictures, charts, notes, formulas, tables, rules ...) as an object character U+FFFC with the object
//! kept beside the text. `flatten` and `build` go between this and a Document.
//!
//! In the text, '\n' ends a paragraph, U+2028 is a line break inside one (and a new line in a code
//! block), and U+FFFC stands for the next object in `objects`. Offsets count characters (code points).

use crate::model::*;
use serde::{Deserialize, Serialize};

pub const OBJECT: char = '\u{FFFC}';
pub const BREAK: char = '\u{2028}';

/// Character formatting over [start, end): a Run whose text is unused.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub fmt: Run,
}

/// A paragraph's formatting.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Line {
    pub style: String,
    pub align: Option<String>,
    pub list: Option<(String, u32)>,
    pub indent: u32,
    /// The language of a code block.
    pub lang: String,
}

impl Default for Line {
    fn default() -> Self {
        Line { style: "normal".into(), align: None, list: None, indent: 0, lang: String::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Math {
    pub tex: String,
    pub display: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Obj {
    Image(Image),
    Chart(Chart),
    Note(Note),
    Opaque(Opaque),
    /// A page or column break inside a paragraph.
    Break(Break),
    Math(Math),
    Table(Table),
    Rule(Rule),
    Pagebreak(PageBreak),
}

impl Obj {
    /// Objects that stand as blocks of their own, between paragraphs.
    pub fn is_block(&self) -> bool {
        matches!(self, Obj::Table(_) | Obj::Rule(_) | Obj::Pagebreak(_)) || matches!(self, Obj::Math(m) if m.display)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Flat {
    pub text: String,
    pub lines: Vec<Line>,
    pub spans: Vec<Span>,
    pub objects: Vec<Obj>,
    /// The list marker of each line ("•", "1.", "a.") or None.
    pub labels: Vec<Option<String>>,
}

fn plain_format(r: &Run) -> Run {
    Run { text: String::new(), ..r.clone() }
}

fn has_format(r: &Run) -> bool {
    !Run::default().same_format(&plain_format(r))
}

struct Writer {
    f: Flat,
    n: usize,
}

impl Writer {
    fn line(&mut self, l: Line) {
        if !self.f.lines.is_empty() {
            self.f.text.push('\n');
            self.n += 1;
        }
        self.f.lines.push(l);
    }

    fn text(&mut self, s: &str, fmt: Option<&Run>) {
        let s: String = s.chars().map(|c| match c {
            '\n' | '\r' | '\u{2029}' => BREAK,
            OBJECT => ' ',
            c => c,
        }).collect();
        let len = s.chars().count();
        if len == 0 {
            return;
        }
        if let Some(r) = fmt.filter(|r| has_format(r)) {
            match self.f.spans.last_mut() {
                Some(last) if last.end == self.n && last.fmt.same_format(&plain_format(r)) => last.end += len,
                _ => self.f.spans.push(Span { start: self.n, end: self.n + len, fmt: plain_format(r) }),
            }
        }
        self.f.text.push_str(&s);
        self.n += len;
    }

    fn object(&mut self, o: Obj) {
        self.f.text.push(OBJECT);
        self.n += 1;
        self.f.objects.push(o);
    }
}

pub fn flatten(doc: &Document) -> Flat {
    let mut w = Writer { f: Flat::default(), n: 0 };
    for b in &doc.blocks {
        match b {
            Block::Paragraph(p) => {
                w.line(Line { style: p.style.clone(), align: p.align.clone(), list: p.list.clone(), indent: p.indent, lang: String::new() });
                for x in &p.content {
                    match x {
                        Inline::Run(r) if r.math => w.object(Obj::Math(Math { tex: r.text.clone(), display: false })),
                        Inline::Run(r) => w.text(&r.text, Some(r)),
                        Inline::Image(i) => w.object(Obj::Image(i.clone())),
                        Inline::Chart(c) => w.object(Obj::Chart(c.clone())),
                        Inline::Note(n) => w.object(Obj::Note(n.clone())),
                        Inline::Opaque(o) => w.object(Obj::Opaque(o.clone())),
                        Inline::Break(b) if b.kind == "line" => w.text("\n", None),
                        Inline::Break(b) => w.object(Obj::Break(b.clone())),
                    }
                }
            }
            Block::Table(t) => {
                w.line(Line::default());
                w.object(Obj::Table(t.clone()));
            }
            Block::CodeBlock(c) => {
                w.line(Line { style: "code".into(), lang: c.lang.clone(), ..Default::default() });
                w.text(&c.text, None);
            }
            Block::MathBlock(m) => {
                w.line(Line { style: "math".into(), ..Default::default() });
                w.object(Obj::Math(Math { tex: m.tex.clone(), display: true }));
            }
            Block::Rule(_) => {
                w.line(Line::default());
                w.object(Obj::Rule(Rule {}));
            }
            Block::PageBreak(_) => {
                w.line(Line::default());
                w.object(Obj::Pagebreak(PageBreak {}));
            }
        }
    }
    if w.f.lines.is_empty() {
        w.line(Line::default());
    }
    let lists: Vec<Option<(String, u32)>> = w.f.lines.iter().map(|l| l.list.clone()).collect();
    w.f.labels = labels(&lists);
    w.f
}

/// The marker of each list line; numbering restarts after any line that is not in a list.
pub fn labels(lists: &[Option<(String, u32)>]) -> Vec<Option<String>> {
    let blocks: Vec<Block> = lists.iter().map(|l| Block::Paragraph(Paragraph { list: l.clone(), ..Default::default() })).collect();
    list_labels(&blocks)
}

/// The document an editor's content stands for. `meta` gives everything but the blocks (title, page
/// set-up, the source package of a Word file ...).
pub fn build(f: &Flat, meta: Document) -> Document {
    let mut doc = Document { blocks: vec![], ..meta };
    let chars: Vec<char> = f.text.chars().collect();
    let mut spans = f.spans.clone();
    spans.sort_by_key(|s| s.start);
    let format_at = |i: usize| -> Run {
        let k = spans.partition_point(|s| s.start <= i);
        if k > 0 && spans[k - 1].end > i {
            plain_format(&spans[k - 1].fmt)
        } else {
            Run::default()
        }
    };
    let mut objects = f.objects.iter();
    let mut start = 0;
    let mut code_before = false;
    for (k, len) in f.text.split('\n').map(|l| l.chars().count()).enumerate() {
        let line = f.lines.get(k).cloned().unwrap_or_default();
        let (a, b) = (start, start + len);
        start = b + 1;
        if line.style == "code" {
            let mut text = String::new();
            for &c in &chars[a..b] {
                match c {
                    BREAK => text.push('\n'),
                    OBJECT => {
                        objects.next();
                    }
                    c => text.push(c),
                }
            }
            match doc.blocks.last_mut() {
                Some(Block::CodeBlock(prev)) if code_before && prev.lang == line.lang => {
                    prev.text.push('\n');
                    prev.text.push_str(&text);
                }
                _ => doc.blocks.push(Block::CodeBlock(CodeBlock { text, lang: line.lang.clone() })),
            }
            code_before = true;
            continue;
        }
        code_before = false;
        let para = |content: Vec<Inline>| {
            Block::Paragraph(Paragraph {
                content: merge_runs(content),
                style: if line.style == "math" { "normal".into() } else { line.style.clone() },
                align: line.align.clone(),
                list: line.list.clone(),
                indent: line.indent,
            })
        };
        let mut content: Vec<Inline> = vec![];
        let mut run = Run::default();
        let mut had_block = false;
        let flush = |run: &mut Run, content: &mut Vec<Inline>| {
            if !run.text.is_empty() {
                content.push(Inline::Run(std::mem::take(run)));
            }
        };
        for i in a..b {
            let c = chars[i];
            match c {
                OBJECT => {
                    flush(&mut run, &mut content);
                    let Some(o) = objects.next() else { continue };
                    if o.is_block() {
                        // a block in the middle of a paragraph splits it
                        if content.iter().any(|x| !matches!(x, Inline::Run(r) if r.text.trim().is_empty())) {
                            doc.blocks.push(para(std::mem::take(&mut content)));
                        }
                        content.clear();
                        doc.blocks.push(match o.clone() {
                            Obj::Table(t) => Block::Table(t),
                            Obj::Rule(r) => Block::Rule(r),
                            Obj::Pagebreak(p) => Block::PageBreak(p),
                            Obj::Math(m) => Block::MathBlock(MathBlock { tex: m.tex }),
                            _ => unreachable!(),
                        });
                        had_block = true;
                        continue;
                    }
                    content.push(match o.clone() {
                        Obj::Image(x) => Inline::Image(x),
                        Obj::Chart(x) => Inline::Chart(x),
                        Obj::Note(x) => Inline::Note(x),
                        Obj::Opaque(x) => Inline::Opaque(x),
                        Obj::Break(x) => Inline::Break(x),
                        Obj::Math(m) => Inline::Run(Run { text: m.tex, math: true, ..Default::default() }),
                        _ => unreachable!(),
                    });
                }
                BREAK => {
                    flush(&mut run, &mut content);
                    content.push(Inline::Break(Break::default()));
                }
                c => {
                    let fmt = format_at(i);
                    if !run.same_format(&fmt) {
                        flush(&mut run, &mut content);
                        run = fmt;
                    }
                    run.text.push(c);
                }
            }
        }
        flush(&mut run, &mut content);
        // an empty line is an empty paragraph, unless it only held a block
        let blank = content.iter().all(|x| matches!(x, Inline::Run(r) if r.text.trim().is_empty()));
        if !(had_block && blank) {
            doc.blocks.push(para(content));
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph(Paragraph::styled(vec![Inline::Run(Run::new("Title"))], "h1")),
                Block::Paragraph(Paragraph::new(vec![
                    Inline::Run(Run::new("Plain ")),
                    Inline::Run(Run { text: "bold".into(), b: true, ..Default::default() }),
                    Inline::Break(Break::default()),
                    Inline::Run(Run { text: "x^2".into(), math: true, ..Default::default() }),
                    Inline::Image(Image { ext: "png".into(), width: 10.0, height: 10.0, ..Default::default() }),
                ])),
                Block::Paragraph(Paragraph { content: vec![Inline::Run(Run::new("item"))], list: Some(("number".into(), 0)), ..Default::default() }),
                Block::Table(Table { rows: vec![vec![vec![Paragraph::new(vec![Inline::Run(Run::new("a"))])]]], ..Default::default() }),
                Block::CodeBlock(CodeBlock { text: "a\nb".into(), lang: "py".into() }),
                Block::MathBlock(MathBlock { tex: "E=mc^2".into() }),
                Block::Rule(Rule {}),
                Block::Paragraph(Paragraph::default()),
            ],
            ..Default::default()
        };
        let f = flatten(&doc);
        assert_eq!(f.lines.len(), 8);
        assert_eq!(f.labels[2].as_deref(), Some("1."));
        assert!(f.text.starts_with("Title\nPlain bold\u{2028}\u{FFFC}\u{FFFC}\nitem\n\u{FFFC}\na\u{2028}b\n"));
        let back = build(&f, Document::default());
        assert_eq!(back.blocks, doc.blocks);
    }
}
