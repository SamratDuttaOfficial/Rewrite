//! The formatted document every import and export goes through. Mirrors `rewrite/docmodel.py`:
//! on the Python side the same structure is a set of dataclasses, and the two meet as plain dicts
//! (blocks and inline items carry a "type" key with the lower-cased class name).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Run {
    pub text: String,
    pub b: bool,
    pub i: bool,
    pub u: bool,
    pub s: bool,
    pub sup: bool,
    pub sub: bool,
    pub code: bool,
    pub math: bool,
    pub size: Option<f64>,
    pub font: Option<String>,
    pub color: Option<String>,
    pub hl: Option<String>,
    pub link: Option<String>,
}

impl Run {
    pub fn new(text: impl Into<String>) -> Run {
        Run { text: text.into(), ..Default::default() }
    }
    pub fn styled(&self, text: impl Into<String>) -> Run {
        Run { text: text.into(), ..self.clone() }
    }
    pub fn same_format(&self, o: &Run) -> bool {
        self.b == o.b && self.i == o.i && self.u == o.u && self.s == o.s && self.sup == o.sup
            && self.sub == o.sub && self.code == o.code && self.math == o.math && self.size == o.size
            && self.font == o.font && self.color == o.color && self.hl == o.hl && self.link == o.link
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Image {
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    pub ext: String,
    pub width: f64,
    pub height: f64,
    pub alt: String,
    pub name: String,
}

impl Image {
    /// Width and height scaled down (never up) to fit `max_w` x `max_h` points.
    pub fn fit(&self, max_w: f64, max_h: Option<f64>) -> (f64, f64) {
        let w = if self.width == 0.0 { 1.0 } else { self.width };
        let h = if self.height == 0.0 { 1.0 } else { self.height };
        let k = 1f64.min(max_w / w).min(max_h.filter(|&m| m != 0.0).map_or(1.0, |m| m / h));
        (w * k, h * k)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Chart {
    pub kind: String,
    pub title: String,
    pub categories: Vec<String>,
    pub series: Vec<(String, Vec<Option<f64>>)>,
    pub x_title: String,
    pub y_title: String,
    pub legend: bool,
    pub width: f64,
    pub height: f64,
}

impl Default for Chart {
    fn default() -> Self {
        Chart {
            kind: "bar".into(),
            title: String::new(),
            categories: vec![],
            series: vec![],
            x_title: String::new(),
            y_title: String::new(),
            legend: true,
            width: 432.0,
            height: 252.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Break {
    pub kind: String,
}

impl Default for Break {
    fn default() -> Self {
        Break { kind: "line".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Note {
    pub runs: Vec<Inline>,
}

/// A piece of a Word file that is not edited here; written back exactly as it was.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Opaque {
    pub xml: String,
    pub label: String,
    pub text: String,
    pub rels: BTreeMap<String, (String, String, String)>,
    pub block: bool,
    pub fmt: String,
}

impl Default for Opaque {
    fn default() -> Self {
        Opaque {
            xml: String::new(),
            label: "object".into(),
            text: String::new(),
            rels: BTreeMap::new(),
            block: false,
            fmt: "docx".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Inline {
    Run(Run),
    Image(Image),
    Chart(Chart),
    Break(Break),
    Note(Note),
    Opaque(Opaque),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Paragraph {
    pub content: Vec<Inline>,
    pub style: String,
    pub align: Option<String>,
    pub list: Option<(String, u32)>,
    pub indent: u32,
}

impl Default for Paragraph {
    fn default() -> Self {
        Paragraph { content: vec![], style: "normal".into(), align: None, list: None, indent: 0 }
    }
}

impl Paragraph {
    pub fn new(content: Vec<Inline>) -> Paragraph {
        Paragraph { content, ..Default::default() }
    }
    pub fn styled(content: Vec<Inline>, style: &str) -> Paragraph {
        Paragraph { content, style: style.into(), ..Default::default() }
    }
    pub fn text(&self) -> String {
        inline_text(&self.content, true)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Table {
    pub rows: Vec<Vec<Vec<Paragraph>>>,
    pub header: bool,
    pub widths: Option<Vec<f64>>,
}

impl Default for Table {
    fn default() -> Self {
        Table { rows: vec![], header: true, widths: None }
    }
}

impl Table {
    pub fn ncols(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CodeBlock {
    pub text: String,
    pub lang: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct MathBlock {
    pub tex: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Rule {}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PageBreak {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    CodeBlock(CodeBlock),
    MathBlock(MathBlock),
    Rule(Rule),
    PageBreak(PageBreak),
}

/// The paragraph styles, in menu order.
pub const STYLES: [&str; 11] = ["normal", "title", "subtitle", "h1", "h2", "h3", "h4", "h5", "h6", "quote", "caption"];

/// Names shown for the paragraph styles (and for code and maths blocks).
pub const STYLE_LABEL: [(&str, &str); 13] = [
    ("normal", "Normal text"), ("title", "Title"), ("subtitle", "Subtitle"), ("h1", "Heading 1"), ("h2", "Heading 2"),
    ("h3", "Heading 3"), ("h4", "Heading 4"), ("h5", "Heading 5"), ("h6", "Heading 6"), ("quote", "Quote"),
    ("caption", "Caption"), ("code", "Code"), ("math", "Maths"),
];

/// The label of a style, or the style name itself when it has none.
pub fn style_label(style: &str) -> &str {
    STYLE_LABEL.iter().find(|(s, _)| *s == style).map_or(style, |(_, l)| l)
}

pub const A4: (f64, f64) = (595.28, 841.89);
pub const PAGE_SIZES: [(&str, f64, f64); 4] =
    [("A4", 595.28, 841.89), ("Letter", 612.0, 792.0), ("Legal", 612.0, 1008.0), ("A5", 419.53, 595.28)];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PageSetup {
    pub width: f64,
    pub height: f64,
    pub margins: (f64, f64, f64, f64), // top, right, bottom, left
    pub numbers: bool,
}

impl Default for PageSetup {
    fn default() -> Self {
        PageSetup { width: A4.0, height: A4.1, margins: (72.0, 72.0, 72.0, 72.0), numbers: true }
    }
}

impl PageSetup {
    pub fn text_width(&self) -> f64 {
        self.width - self.margins.1 - self.margins.3
    }
    pub fn text_height(&self) -> f64 {
        self.height - self.margins.0 - self.margins.2
    }
    /// "A4", "A4 landscape", or the size in inches ("8.3 x 11.1 in") when it is no named size.
    pub fn name(&self) -> String {
        for (n, w, h) in PAGE_SIZES {
            if (w - self.width).abs() < 2.0 && (h - self.height).abs() < 2.0 {
                return n.to_string();
            }
            if (h - self.width).abs() < 2.0 && (w - self.height).abs() < 2.0 {
                return format!("{n} landscape");
            }
        }
        format!("{:.1} x {:.1} in", self.width / 72.0, self.height / 72.0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub title: String,
    pub author: String,
    pub page: PageSetup,
    pub font: Option<String>,
    pub size: f64,
    pub source: BTreeMap<String, serde_bytes::ByteBuf>,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            blocks: vec![],
            title: String::new(),
            author: String::new(),
            page: PageSetup::default(),
            font: None,
            size: 11.0,
            source: BTreeMap::new(),
        }
    }
}

/// How each paragraph style looks: size relative to the body text, weight, slant, space before and after
/// (points), extra indent and colour. Shared by the PDF layout and the Word/ODF writers.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub scale: f64,
    pub bold: bool,
    pub italic: bool,
    pub before: f64,
    pub after: f64,
    pub indent: f64,
    pub color: Option<&'static str>,
}

pub fn look(style: &str) -> Look {
    let l = |scale, bold, italic, before, after, indent, color| Look { scale, bold, italic, before, after, indent, color };
    match style {
        "title" => l(2.4, false, false, 0.0, 10.0, 0.0, Some("#17365D")),
        "subtitle" => l(1.35, false, false, 0.0, 10.0, 0.0, Some("#4F6228")),
        "h1" => l(1.65, true, false, 16.0, 6.0, 0.0, Some("#1F3864")),
        "h2" => l(1.35, true, false, 12.0, 4.0, 0.0, Some("#2F5496")),
        "h3" => l(1.18, true, false, 10.0, 4.0, 0.0, Some("#1F3864")),
        "h4" => l(1.05, true, true, 8.0, 2.0, 0.0, Some("#2F5496")),
        "h5" => l(1.0, true, false, 8.0, 2.0, 0.0, Some("#2F5496")),
        "h6" => l(1.0, false, true, 8.0, 2.0, 0.0, Some("#1F3864")),
        "quote" => l(1.0, false, true, 4.0, 8.0, 36.0, Some("#404040")),
        "caption" => l(0.85, false, true, 2.0, 10.0, 0.0, Some("#44546A")),
        "code" => l(0.88, false, false, 4.0, 8.0, 0.0, None),
        "math" => l(1.0, false, true, 6.0, 8.0, 0.0, None),
        _ => l(1.0, false, false, 0.0, 6.0, 0.0, None),
    }
}

pub fn is_heading(style: &str) -> bool {
    matches!(style, "title" | "subtitle" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

pub fn inline_text(content: &[Inline], objects: bool) -> String {
    let mut out = String::new();
    for x in content {
        match x {
            Inline::Run(r) => out.push_str(&r.text),
            Inline::Break(_) => out.push('\n'),
            Inline::Opaque(o) if objects => out.push_str(&o.text),
            _ => {}
        }
    }
    out
}

/// Join neighbouring runs that look the same; drop empty ones.
pub fn merge_runs(content: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(content.len());
    for x in content {
        if let Inline::Run(r) = &x {
            if r.text.is_empty() {
                continue;
            }
            if let Some(Inline::Run(prev)) = out.last_mut() {
                if prev.same_format(r) {
                    prev.text.push_str(&r.text);
                    continue;
                }
            }
        }
        out.push(x);
    }
    out
}

/// Inline content without white space at its ends; a paragraph that holds only pictures or charts (and
/// spaces) keeps just those.
pub fn strip_content(content: Vec<Inline>) -> Vec<Inline> {
    let mut content = merge_runs(content);
    let objects: Vec<&Inline> = content.iter().filter(|x| !matches!(x, Inline::Run(_))).collect();
    let all_ws = content.iter().all(|x| match x {
        Inline::Run(r) => r.text.trim().is_empty(),
        _ => true,
    });
    if !objects.is_empty()
        && objects.iter().all(|x| matches!(x, Inline::Image(_) | Inline::Chart(_) | Inline::Break(_)))
        && all_ws
    {
        return content.into_iter().filter(|x| matches!(x, Inline::Image(_) | Inline::Chart(_))).collect();
    }
    while matches!(content.first(), Some(Inline::Run(r)) if r.text.trim().is_empty()) {
        content.remove(0);
    }
    while matches!(content.last(), Some(Inline::Run(r)) if r.text.trim().is_empty()) {
        content.pop();
    }
    if let Some(Inline::Run(r)) = content.first_mut() {
        r.text = r.text.trim_start().to_string();
    }
    if let Some(Inline::Run(r)) = content.last_mut() {
        r.text = r.text.trim_end().to_string();
    }
    content
}

/// A caption's content without its "Figure 3:" label (LaTeX and Word number captions themselves).
pub fn caption_text(content: &[Inline]) -> Vec<Inline> {
    let text = inline_text(content, true);
    let re = regex::Regex::new(r"(?i)^\s*(Figure|Fig\.|Table|Tab\.|Chart|Listing)\s*[\dIVX]+[.:]?\s*").unwrap();
    let Some(m) = re.find(&text) else { return content.to_vec() };
    let mut drop = m.end();
    let mut out = vec![];
    for x in content {
        if let Inline::Run(r) = x {
            if drop > 0 {
                if r.text.len() <= drop {
                    drop -= r.text.len();
                    continue;
                }
                out.push(Inline::Run(r.styled(&r.text[drop..])));
                drop = 0;
                continue;
            }
        }
        out.push(x.clone());
    }
    out
}

/// A plain text as paragraphs: blank lines separate paragraphs, single line breaks are kept.
pub fn plain_paragraphs(text: &str) -> Vec<Block> {
    let re = regex::Regex::new(r"\n[ \t]*\n").unwrap();
    let mut blocks = vec![];
    for para in re.split(text.trim_matches('\n')) {
        if para.trim().is_empty() {
            continue;
        }
        let mut content = vec![];
        for (k, line) in para.split('\n').enumerate() {
            if k > 0 {
                content.push(Inline::Break(Break::default()));
            }
            content.push(Inline::Run(Run::new(line)));
        }
        blocks.push(Block::Paragraph(Paragraph::new(merge_runs(content))));
    }
    blocks
}

pub fn number_label(n: usize, level: u32) -> String {
    match level % 3 {
        1 => {
            let mut s = String::new();
            let mut k = n;
            while k > 0 {
                let r = (k - 1) % 26;
                k = (k - 1) / 26;
                s.insert(0, (b'a' + r as u8) as char);
            }
            s + "."
        }
        2 => roman(n).to_lowercase() + ".",
        _ => format!("{n}."),
    }
}

pub fn roman(mut n: usize) -> String {
    let vals = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
                (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut out = String::new();
    for (v, s) in vals {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    if out.is_empty() { "0".into() } else { out }
}

pub const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// The marker of every list paragraph (by its index in `blocks`): "•", "1.", "a." ... Numbering restarts
/// after anything that is not a list paragraph.
pub fn list_labels(blocks: &[Block]) -> Vec<Option<String>> {
    let mut labels = vec![None; blocks.len()];
    let mut counters: Vec<usize> = vec![];
    for (k, b) in blocks.iter().enumerate() {
        let Block::Paragraph(p) = b else {
            counters.clear();
            continue;
        };
        let Some((kind, level)) = &p.list else {
            counters.clear();
            continue;
        };
        let level = *level as usize;
        counters.truncate(level + 1);
        while counters.len() <= level {
            counters.push(0);
        }
        if kind == "number" {
            counters[level] += 1;
            labels[k] = Some(number_label(counters[level], level as u32));
        } else {
            counters[level] = 0;
            labels[k] = Some(BULLETS[level % 3].to_string());
        }
    }
    labels
}

/// The text of a document, paragraphs separated by blank lines (tables as tab-separated rows).
pub fn document_text(doc: &Document) -> String {
    let mut out = vec![];
    for b in &doc.blocks {
        match b {
            Block::Paragraph(p) => out.push(p.text()),
            Block::Table(t) => out.push(
                t.rows
                    .iter()
                    .map(|r| r.iter().map(|c| c.iter().map(|p| p.text()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\t"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Block::CodeBlock(c) => out.push(c.text.clone()),
            Block::MathBlock(m) => out.push(m.tex.clone()),
            _ => {}
        }
    }
    out.into_iter().filter(|x| !x.trim().is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// Every paragraph, including those inside table cells.
pub fn all_paragraphs(doc: &Document) -> Vec<&Paragraph> {
    let mut out = vec![];
    for b in &doc.blocks {
        match b {
            Block::Paragraph(p) => out.push(p),
            Block::Table(t) => {
                for row in &t.rows {
                    for cell in row {
                        out.extend(cell.iter());
                    }
                }
            }
            _ => {}
        }
    }
    out
}
