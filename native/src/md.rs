//! Markdown <-> Document.

use crate::charts;
use crate::imaging;
use crate::model::*;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;

pub type Files = BTreeMap<String, Vec<u8>>;

// -- reading ------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Fmt {
    b: u32,
    i: u32,
    s: u32,
    sup: u32,
    sub: u32,
    u: u32,
    hl: u32,
}

enum Target {
    Para(Paragraph),
    Cell,
    Note(String),
}

struct Reader<'a> {
    base_dir: Option<&'a Path>,
    blocks: Vec<Block>,
    fmt: Fmt,
    links: Vec<String>,
    cur: Vec<Inline>,
    target: Vec<Target>,
    lists: Vec<Option<u64>>,
    quote: u32,
    item_first: bool,
    table: Option<(Table, Vec<Alignment>, Vec<Paragraph>)>, // the table, its column alignment, the row being read
    code: Option<(String, String)>,
    image: Option<(String, String)>, // url, alt text so far
    notes: BTreeMap<String, Vec<Inline>>,
    title: String,
    align: Option<String>,
    pending_display: bool, // the paragraph holds displayed maths ($$ ... $$)
    note_marks: Vec<usize>, // where the blocks of an open footnote definition start
}

fn load_image(src: &str, base: Option<&Path>, alt: &str) -> Option<Image> {
    if src.contains("://") || src.starts_with("data:") {
        return None;
    }
    let src = src.replace("%20", " ");
    let p = Path::new(&src);
    let path = if p.is_absolute() { p.to_path_buf() } else { base.map(|b| b.join(p)).unwrap_or_else(|| p.to_path_buf()) };
    let data = std::fs::read(&path).ok()?;
    let ext = match imaging::sniff(&data) {
        "" => imaging::ext_for(&path.to_string_lossy()),
        e => e.to_string(),
    };
    let (w, h) = imaging::natural_size(&data, Some(451.0));
    Some(Image { data, ext, width: w, height: h, alt: alt.into(), name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() })
}

impl<'a> Reader<'a> {
    fn run(&self, text: &str) -> Run {
        Run {
            text: text.into(),
            b: self.fmt.b > 0,
            i: self.fmt.i > 0,
            s: self.fmt.s > 0,
            sup: self.fmt.sup > 0,
            sub: self.fmt.sub > 0,
            u: self.fmt.u > 0,
            hl: if self.fmt.hl > 0 { Some("#FFFF00".into()) } else { None },
            link: self.links.last().cloned(),
            ..Default::default()
        }
    }

    fn push_text(&mut self, text: &str) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
            return;
        }
        if let Some((code, _)) = &mut self.code {
            code.push_str(text);
            return;
        }
        let r = self.run(text);
        self.cur.push(Inline::Run(r));
    }

    fn start_para(&mut self, style: &str) {
        let mut p = Paragraph::styled(vec![], style);
        if self.quote > 0 && style == "normal" {
            p.style = "quote".into();
            p.indent = self.quote - 1;
        }
        if let Some(kind) = self.lists.last() {
            if self.item_first {
                let level = (self.lists.len() - 1).min(8) as u32;
                p.list = Some((if kind.is_some() { "number" } else { "bullet" }.into(), level));
                self.item_first = false;
            } else {
                p.indent = self.lists.len() as u32;
            }
        }
        p.align = self.align.clone();
        self.target.push(Target::Para(p));
    }

    fn end_para(&mut self) {
        let content = strip_content(std::mem::take(&mut self.cur));
        match self.target.pop() {
            Some(Target::Para(mut p)) => {
                let only: Vec<&Inline> = content.iter().filter(|x| !matches!(x, Inline::Run(r) if r.text.trim().is_empty())).collect();
                if only.len() == 1 {
                    if let Inline::Run(r) = only[0] {
                        if r.math && self.pending_display {
                            self.blocks.push(Block::MathBlock(MathBlock { tex: r.text.trim().into() }));
                            self.pending_display = false;
                            return;
                        }
                    }
                }
                self.pending_display = false;
                p.content = content;
                if p.content.is_empty() && p.list.is_none() {
                    return;
                }
                self.blocks.push(Block::Paragraph(p));
            }
            Some(other) => self.target.push(other),
            None => {}
        }
    }
}

pub fn parse(text: &str, base_dir: Option<&Path>) -> Document {
    let mut opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES | Options::ENABLE_MATH | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    let mut r = Reader {
        base_dir,
        blocks: vec![],
        fmt: Fmt::default(),
        links: vec![],
        cur: vec![],
        target: vec![],
        lists: vec![],
        quote: 0,
        item_first: false,
        table: None,
        code: None,
        image: None,
        notes: BTreeMap::new(),
        title: String::new(),
        align: None,
        pending_display: false,
        note_marks: vec![],
    };
    let centered = Regex::new(r#"(?is)^\s*<p\s+align="?center"?\s*>(.*)</p>\s*$"#).unwrap();
    let pagebreak = Regex::new(r#"(?i)page-break-(after|before)\s*:\s*always|<!--\s*pagebreak\s*-->|\\newpage"#).unwrap();
    let mut in_meta = false;
    let mut in_item_tight = false;
    for ev in Parser::new_ext(text, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if in_item_tight {
                        // the item paragraph was opened at Item start
                        in_item_tight = false;
                        continue;
                    }
                    r.start_para("normal");
                }
                Tag::Heading { level, .. } => {
                    let l = match level { HeadingLevel::H1 => 1, HeadingLevel::H2 => 2, HeadingLevel::H3 => 3, HeadingLevel::H4 => 4, HeadingLevel::H5 => 5, HeadingLevel::H6 => 6 };
                    r.start_para(&format!("h{l}"));
                    if let Some(Target::Para(p)) = r.target.last_mut() {
                        p.list = None;
                        p.indent = 0;
                    }
                }
                Tag::BlockQuote(_) => r.quote += 1,
                Tag::CodeBlock(kind) => {
                    let lang = match kind { CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(), _ => String::new() };
                    r.code = Some((String::new(), lang));
                }
                Tag::List(start) => {
                    if !r.cur.is_empty() || matches!(r.target.last(), Some(Target::Para(_))) {
                        if matches!(r.target.last(), Some(Target::Para(_))) {
                            r.end_para();
                        }
                    }
                    r.lists.push(start);
                }
                Tag::Item => {
                    r.item_first = true;
                    r.start_para("normal");
                    in_item_tight = true;
                }
                Tag::FootnoteDefinition(label) => {
                    r.note_marks.push(r.blocks.len());
                    r.target.push(Target::Note(label.to_string()));
                }
                Tag::Table(al) => r.table = Some((Table { rows: vec![], header: true, widths: None }, al, vec![])),
                Tag::TableHead | Tag::TableRow => {}
                Tag::TableCell => r.target.push(Target::Cell),
                Tag::Emphasis => r.fmt.i += 1,
                Tag::Strong => r.fmt.b += 1,
                Tag::Strikethrough => r.fmt.s += 1,
                Tag::Superscript => r.fmt.sup += 1,
                Tag::Subscript => r.fmt.sub += 1,
                Tag::Link { dest_url, .. } => r.links.push(dest_url.to_string()),
                Tag::Image { dest_url, .. } => r.image = Some((dest_url.to_string(), String::new())),
                Tag::MetadataBlock(_) => in_meta = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) => {
                    in_item_tight = false;
                    if matches!(r.target.last(), Some(Target::Para(_))) {
                        r.end_para();
                    }
                }
                TagEnd::BlockQuote(_) => r.quote = r.quote.saturating_sub(1),
                TagEnd::CodeBlock => {
                    if let Some((mut code, lang)) = r.code.take() {
                        if code.ends_with('\n') {
                            code.pop();
                        }
                        r.blocks.push(Block::CodeBlock(CodeBlock { text: code, lang }));
                    }
                }
                TagEnd::List(_) => {
                    if matches!(r.target.last(), Some(Target::Para(_))) {
                        r.end_para();
                    }
                    r.lists.pop();
                }
                TagEnd::Item => {
                    in_item_tight = false;
                    if matches!(r.target.last(), Some(Target::Para(_))) {
                        r.end_para();
                    }
                }
                TagEnd::FootnoteDefinition => {
                    if matches!(r.target.last(), Some(Target::Para(_))) {
                        r.end_para();
                    }
                    if let Some(Target::Note(label)) = r.target.pop() {
                        // the paragraphs written since the definition started are the note's text
                        let mark = r.note_marks.pop().unwrap_or(r.blocks.len()).min(r.blocks.len());
                        let mut runs: Vec<Inline> = vec![];
                        for b in r.blocks.drain(mark..) {
                            if let Block::Paragraph(p) = b {
                                if !runs.is_empty() {
                                    runs.push(Inline::Run(Run::new(" ")));
                                }
                                runs.extend(p.content);
                            }
                        }
                        r.notes.insert(label, runs);
                    }
                }
                TagEnd::Table => {
                    if let Some((t, _, _)) = r.table.take() {
                        r.blocks.push(Block::Table(t));
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some((t, _, row)) = &mut r.table {
                        t.rows.push(std::mem::take(row).into_iter().map(|p| vec![p]).collect());
                    }
                }
                TagEnd::TableCell => {
                    let content = strip_content(std::mem::take(&mut r.cur));
                    r.target.pop();
                    if let Some((_, al, row)) = &mut r.table {
                        let a = match al.get(row.len()) { Some(Alignment::Center) => Some("center".into()), Some(Alignment::Right) => Some("right".into()), _ => None };
                        row.push(Paragraph { content, align: a, ..Default::default() });
                    }
                }
                TagEnd::Emphasis => r.fmt.i = r.fmt.i.saturating_sub(1),
                TagEnd::Strong => r.fmt.b = r.fmt.b.saturating_sub(1),
                TagEnd::Strikethrough => r.fmt.s = r.fmt.s.saturating_sub(1),
                TagEnd::Superscript => r.fmt.sup = r.fmt.sup.saturating_sub(1),
                TagEnd::Subscript => r.fmt.sub = r.fmt.sub.saturating_sub(1),
                TagEnd::Link => {
                    r.links.pop();
                }
                TagEnd::Image => {
                    if let Some((url, alt)) = r.image.take() {
                        match load_image(&url, r.base_dir, &alt) {
                            Some(img) => r.cur.push(Inline::Image(img)),
                            None => {
                                let label = if alt.is_empty() { Path::new(&url).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(url.clone()) } else { alt };
                                let mut run = r.run(&format!("[{label}]"));
                                if url.contains("://") {
                                    run.link = Some(url);
                                }
                                r.cur.push(Inline::Run(run));
                            }
                        }
                    }
                }
                TagEnd::MetadataBlock(_) => in_meta = false,
                _ => {}
            },
            Event::Text(t) => {
                if in_meta {
                    for line in t.lines() {
                        if let Some(v) = line.strip_prefix("title:") {
                            r.title = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                        }
                    }
                    continue;
                }
                if !matches!(r.target.last(), Some(_)) && r.code.is_none() && r.image.is_none() {
                    r.start_para("normal");
                }
                r.push_text(&t);
            }
            Event::Code(t) => {
                let mut run = r.run(&t);
                run.code = true;
                r.cur.push(Inline::Run(run));
            }
            Event::InlineMath(t) => {
                let mut run = r.run(&t);
                run.math = true;
                r.cur.push(Inline::Run(run));
            }
            Event::DisplayMath(t) => {
                let mut run = r.run(&t);
                run.math = true;
                r.pending_display = true;
                if r.target.is_empty() {
                    r.blocks.push(Block::MathBlock(MathBlock { tex: t.trim().into() }));
                } else {
                    r.cur.push(Inline::Run(run));
                }
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                let tag = h.trim().to_lowercase();
                if pagebreak.is_match(&tag) {
                    r.blocks.push(Block::PageBreak(PageBreak {}));
                } else if let Some(m) = centered.captures(&h) {
                    let inner = parse(&m[1], base_dir);
                    for b in inner.blocks {
                        if let Block::Paragraph(mut p) = b {
                            p.align = Some("center".into());
                            r.blocks.push(Block::Paragraph(p));
                        }
                    }
                } else {
                    match tag.as_str() {
                        "<br>" | "<br/>" | "<br />" => r.cur.push(Inline::Break(Break::default())),
                        "<sup>" => r.fmt.sup += 1,
                        "</sup>" => r.fmt.sup = r.fmt.sup.saturating_sub(1),
                        "<sub>" => r.fmt.sub += 1,
                        "</sub>" => r.fmt.sub = r.fmt.sub.saturating_sub(1),
                        "<u>" | "<ins>" => r.fmt.u += 1,
                        "</u>" | "</ins>" => r.fmt.u = r.fmt.u.saturating_sub(1),
                        "<mark>" => r.fmt.hl += 1,
                        "</mark>" => r.fmt.hl = r.fmt.hl.saturating_sub(1),
                        "<s>" | "<del>" | "<strike>" => r.fmt.s += 1,
                        "</s>" | "</del>" | "</strike>" => r.fmt.s = r.fmt.s.saturating_sub(1),
                        _ => {}
                    }
                }
            }
            Event::FootnoteReference(label) => {
                r.cur.push(Inline::Note(Note { runs: vec![Inline::Run(Run::new(format!("\u{0}{label}")))] }));
            }
            Event::SoftBreak => r.push_text(" "),
            Event::HardBreak => r.cur.push(Inline::Break(Break::default())),
            Event::Rule => r.blocks.push(Block::Rule(Rule {})),
            Event::TaskListMarker(done) => r.cur.push(Inline::Run(Run::new(if done { "☑ " } else { "☐ " }))),
        }
    }
    // footnote references get their text
    let notes = r.notes.clone();
    let fill = |content: &mut Vec<Inline>| {
        for x in content.iter_mut() {
            if let Inline::Note(n) = x {
                if let Some(Inline::Run(run)) = n.runs.first() {
                    if let Some(label) = run.text.strip_prefix('\u{0}') {
                        n.runs = notes.get(label).cloned().unwrap_or_default();
                    }
                }
            }
        }
    };
    for b in r.blocks.iter_mut() {
        match b {
            Block::Paragraph(p) => fill(&mut p.content),
            Block::Table(t) => t.rows.iter_mut().for_each(|row| row.iter_mut().for_each(|c| c.iter_mut().for_each(|p| fill(&mut p.content)))),
            _ => {}
        }
    }
    let mut doc = Document { blocks: r.blocks, title: r.title, ..Default::default() };
    if doc.title.is_empty() {
        if let Some(Block::Paragraph(p)) = doc.blocks.iter().find(|b| matches!(b, Block::Paragraph(p) if p.style == "h1")) {
            doc.title = p.text().trim().to_string();
        }
    }
    doc
}

// -- writing ------------------------------------------------------------------------

fn escape(text: &str, table: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\`*_[]<>|".contains(c) {
            out.push('\\');
        }
        if table && c == '\n' {
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

fn run_md(r: &Run, table: bool) -> String {
    if r.text.is_empty() {
        return String::new();
    }
    if r.math {
        return format!("${}$", r.text);
    }
    let core = r.text.trim();
    if core.is_empty() {
        return r.text.clone();
    }
    let lead = &r.text[..r.text.len() - r.text.trim_start().len()];
    let trail = &r.text[r.text.trim_end().len()..];
    let mut s = if r.code {
        if core.contains('`') { format!("`` {core} ``") } else { format!("`{core}`") }
    } else {
        let mut s = escape(core, table);
        if r.sup {
            s = format!("<sup>{s}</sup>");
        }
        if r.sub {
            s = format!("<sub>{s}</sub>");
        }
        if r.u && r.link.is_none() {
            s = format!("<u>{s}</u>");
        }
        if r.hl.is_some() {
            s = format!("<mark>{s}</mark>");
        }
        if r.s {
            s = format!("~~{s}~~");
        }
        if r.b && r.i {
            s = format!("***{s}***");
        } else if r.b {
            s = format!("**{s}**");
        } else if r.i {
            s = format!("*{s}*");
        }
        s
    };
    if let Some(l) = &r.link {
        s = format!("[{s}]({})", l.replace(' ', "%20").replace(')', "%29"));
    }
    format!("{lead}{s}{trail}")
}

struct Ctx {
    image_dir: String,
    files: Files,
    notes: Vec<Vec<Inline>>,
    n: usize,
}

impl Ctx {
    fn name(&mut self, base: &str, ext: &str) -> String {
        self.n += 1;
        let stem = Path::new(base).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '-' }).collect();
        let clean = clean.trim_matches('-');
        let clean = if clean.is_empty() { "image" } else { clean };
        format!("{}/{:02}-{}.{}", self.image_dir, self.n, clean, ext)
    }

    fn image(&mut self, img: &Image) -> String {
        let mut data = img.data.clone();
        let mut ext = if img.ext.is_empty() { imaging::sniff(&img.data).to_string() } else { img.ext.clone() };
        if !matches!(ext.as_str(), "png" | "jpeg" | "gif" | "svg") {
            match imaging::to_png(&data) {
                Ok(p) => {
                    data = p;
                    ext = "png".into();
                }
                Err(_) => return format!("[{}]", escape(if img.alt.is_empty() { "picture" } else { &img.alt }, false)),
            }
        }
        let base = if !img.name.is_empty() { img.name.clone() } else if !img.alt.is_empty() { img.alt.clone() } else { "image".into() };
        let path = self.name(&base, if ext == "jpeg" { "jpg" } else { &ext });
        self.files.insert(path.clone(), data);
        format!("![{}]({})", escape(&img.alt, false), path.replace(' ', "%20"))
    }

    fn chart(&mut self, c: &Chart) -> String {
        let path = self.name(&charts::slug(if c.title.is_empty() { "chart" } else { &c.title }), "svg");
        self.files.insert(path.clone(), charts::svg(c).into_bytes());
        format!("![{}]({})", escape(if c.title.is_empty() { "Chart" } else { &c.title }, false), path)
    }

    fn inline(&mut self, content: &[Inline], table: bool) -> String {
        let items = merge_runs(content.to_vec());
        let plain = |x: &Inline| matches!(x, Inline::Run(r) if !r.code && !r.math && r.link.is_none() && !r.text.trim().is_empty());
        let marks = |x: &Inline| if let Inline::Run(r) = x { (r.b, r.i, r.s) } else { (false, false, false) };
        let mut out = String::new();
        let mut k = 0;
        while k < items.len() {
            // neighbouring runs with the same emphasis (but say a subscript between them) share one pair
            // of markers: *z<sub>i</sub>*, not *z**<sub>i</sub>*
            let m = marks(&items[k]);
            let mut j = k + 1;
            if plain(&items[k]) && m != (false, false, false) {
                while j < items.len() && plain(&items[j]) && marks(&items[j]) == m {
                    j += 1;
                }
            }
            if j - k > 1 {
                let inner: String = items[k..j].iter().map(|x| match x {
                    Inline::Run(r) => run_md(&Run { b: false, i: false, s: false, ..r.clone() }, table),
                    _ => String::new(),
                }).collect();
                let core = inner.trim();
                let lead = &inner[..inner.len() - inner.trim_start().len()];
                let trail = &inner[inner.trim_end().len()..];
                let mut s = core.to_string();
                if m.2 {
                    s = format!("~~{s}~~");
                }
                s = match (m.0, m.1) {
                    (true, true) => format!("***{s}***"),
                    (true, false) => format!("**{s}**"),
                    (false, true) => format!("*{s}*"),
                    _ => s,
                };
                out.push_str(&format!("{lead}{s}{trail}"));
                k = j;
                continue;
            }
            let x = &items[k];
            k += 1;
            match x {
                Inline::Run(r) => out.push_str(&run_md(r, table)),
                Inline::Break(_) => out.push_str(if table { "<br>" } else { "  \n" }),
                Inline::Image(i) => out.push_str(&self.image(i)),
                Inline::Chart(c) => out.push_str(&self.chart(c)),
                Inline::Note(n) => {
                    self.notes.push(n.runs.clone());
                    out.push_str(&format!("[^{}]", self.notes.len()));
                }
                Inline::Opaque(o) => out.push_str(&escape(&o.text, table)),
            }
        }
        out
    }
}

/// Put each sentence on its own line (Markdown joins them back when it is shown).
pub fn split_lines(text: &str, cuts: &[usize]) -> String {
    let mut out: Vec<String> = vec![];
    let mut last = 0;
    for &c in cuts {
        if c <= last || c >= text.len() || !text.is_char_boundary(c) {
            continue;
        }
        out.push(text[last..c].trim_end_matches(' ').to_string());
        last = c;
    }
    out.push(text[last..].to_string());
    out.into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Markdown text and the files it refers to. `sentences` gives, for a paragraph's text, the byte offsets
/// where new sentences start (to put each on its own line).
pub fn write(doc: &Document, stem: &str, sentences: Option<&dyn Fn(&str) -> Vec<usize>>) -> (String, Files) {
    let mut ctx = Ctx { image_dir: format!("{stem}-images"), files: Files::new(), notes: vec![], n: 0 };
    let mut out: Vec<String> = vec![];
    let mut counters: BTreeMap<u32, usize> = BTreeMap::new();
    let mut prev_list = false;
    let has_title = doc.blocks.iter().any(|b| matches!(b, Block::Paragraph(p) if p.style == "title"));
    for b in &doc.blocks {
        if let Block::Paragraph(p) = b {
            let mut text = ctx.inline(&p.content, false);
            if let Some(f) = sentences {
                if matches!(p.style.as_str(), "normal" | "quote") && p.list.is_none() {
                    text = split_lines(&text, &f(&text));
                }
            }
            if let Some((kind, level)) = &p.list {
                let indent = "    ".repeat(*level as usize);
                let mark = if kind == "number" {
                    let c = counters.entry(*level).or_insert(0);
                    *c += 1;
                    let v = *c;
                    counters.retain(|k, _| *k <= *level);
                    format!("{v}.")
                } else {
                    "-".into()
                };
                let line = format!("{indent}{mark} {}", text.replace('\n', &format!("\n{indent}  ")));
                if prev_list {
                    out.last_mut().unwrap().push_str(&format!("\n{line}"));
                } else {
                    out.push(line);
                }
                prev_list = true;
                continue;
            }
            counters.clear();
            prev_list = false;
            let only_objects = p.content.iter().all(|x| matches!(x, Inline::Image(_) | Inline::Chart(_)) || matches!(x, Inline::Run(r) if r.text.trim().is_empty()));
            if text.trim().is_empty() && !p.content.iter().any(|x| matches!(x, Inline::Image(_) | Inline::Chart(_))) {
                continue;
            }
            match p.style.as_str() {
                "title" => out.push(format!("# {}", text.replace('\n', " "))),
                "subtitle" | "caption" => out.push(format!("*{}*", text.trim())),
                s if s.len() == 2 && s.starts_with('h') => {
                    let mut level: usize = s[1..].parse().unwrap_or(1);
                    if has_title {
                        level = (level + 1).min(6);
                    }
                    out.push(format!("{} {}", "#".repeat(level), text.replace('\n', " ")));
                }
                "quote" => out.push(text.split('\n').map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")),
                _ => {
                    if p.align.as_deref() == Some("center") && !text.contains('\n') && !only_objects {
                        out.push(format!("<p align=\"center\">{text}</p>"));
                    } else {
                        out.push(text);
                    }
                }
            }
            continue;
        }
        counters.clear();
        prev_list = false;
        match b {
            Block::Table(t) => out.push(table_md(t, &mut ctx)),
            Block::CodeBlock(c) => {
                let fence = if c.text.contains("```") { "````" } else { "```" };
                out.push(format!("{fence}{}\n{}\n{fence}", c.lang, c.text));
            }
            Block::MathBlock(m) => out.push(format!("$$\n{}\n$$", m.tex.trim())),
            Block::Rule(_) => out.push("---".into()),
            Block::PageBreak(_) => out.push("<div style=\"page-break-after: always\"></div>".into()),
            Block::Paragraph(_) => {}
        }
    }
    if !ctx.notes.is_empty() {
        let notes = std::mem::take(&mut ctx.notes);
        let mut lines = vec![];
        for (k, n) in notes.iter().enumerate() {
            lines.push(format!("[^{}]: {}", k + 1, ctx.inline(n, false).replace('\n', " ")));
        }
        out.push(lines.join("\n"));
    }
    (out.join("\n\n").trim_end().to_string() + "\n", ctx.files)
}

fn table_md(t: &Table, ctx: &mut Ctx) -> String {
    let n = t.ncols();
    if n == 0 {
        return String::new();
    }
    let mut rows: Vec<Vec<String>> = vec![];
    for row in &t.rows {
        let mut cells = vec![];
        for c in 0..n {
            let paras = row.get(c).cloned().unwrap_or_default();
            let txt: Vec<String> = paras.iter().map(|p| ctx.inline(&p.content, true)).collect();
            cells.push(txt.join("<br>").replace('|', "\\|").trim().to_string());
        }
        rows.push(cells);
    }
    let head = if t.header { rows[0].clone() } else { vec![String::new(); n] };
    let body = if t.header { &rows[1..] } else { &rows[..] };
    let align: Vec<&str> = (0..n)
        .map(|c| match t.rows.first().and_then(|r| r.get(c)).and_then(|cell| cell.first()).and_then(|p| p.align.as_deref()) {
            Some("center") => ":---:",
            Some("right") => "---:",
            _ => "---",
        })
        .collect();
    let mut lines = vec![format!("| {} |", head.join(" | ")), format!("| {} |", align.join(" | "))];
    for r in body {
        lines.push(format!("| {} |", r.join(" | ")));
    }
    lines.join("\n")
}
