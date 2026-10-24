//! Typesetting a Document into PDF pages.
//!
//! Paragraphs are broken into lines (greedy, with optional justification), then everything is put on
//! pages as "flow items" of known height. Headings stay with the next line, and a paragraph never leaves
//! a single line alone at the top or bottom of a page. Tables repeat their header row on a new page.
//! A character the main font lacks is taken from a fallback font.

use crate::charts;
use crate::fonts::{self, Font};
use crate::imaging;
use crate::mathtex;
use crate::model::*;
use crate::pdf::Writer;
use std::collections::HashMap;
use std::sync::Arc;

const LINE: f64 = 1.28;
const TEXT: &str = "#1A1A1A";
const LINK: &str = "#1F5FBF";
const CODE_BG: &str = "#F4F4F4";
const BORDER: &str = "#BFBFBF";
const HEAD_BG: &str = "#F0F0F0";
const QUOTE_BAR: &str = "#C8C8C8";

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

struct Fonts {
    family: Option<String>,
    generic: String,
    cache: HashMap<(bool, bool, bool, Option<String>), (Arc<Font>, bool, bool)>,
    fallbacks: Option<Vec<Arc<Font>>>,
    any: Arc<Font>,
}

impl Fonts {
    fn new(family: Option<String>, generic: &str) -> Result<Fonts, String> {
        let any = fonts::resolve(family.as_deref(), false, false, generic).ok_or("No usable font is installed.")?.0;
        Ok(Fonts { family, generic: generic.into(), cache: HashMap::new(), fallbacks: None, any })
    }

    fn get(&mut self, bold: bool, italic: bool, mono: bool, family: Option<&str>) -> (Arc<Font>, bool, bool) {
        let key = (bold, italic, mono, family.map(String::from));
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let r = if mono {
            fonts::resolve(None, bold, italic, "mono")
        } else {
            let fam = family.map(String::from).or_else(|| self.family.clone());
            fonts::resolve(fam.as_deref(), bold, italic, &self.generic)
        }
        .unwrap_or_else(|| (self.any.clone(), bold, italic));
        self.cache.insert(key, r.clone());
        r
    }

    fn for_char(&mut self, ch: char, f: &Arc<Font>) -> Arc<Font> {
        if f.has(ch) || ch == '\u{200b}' || ch == '\u{ad}' {
            return f.clone();
        }
        if self.fallbacks.is_none() {
            self.fallbacks = Some(fonts::fallbacks());
        }
        for x in self.fallbacks.as_ref().unwrap() {
            if x.has(ch) {
                return x.clone();
            }
        }
        f.clone()
    }
}

#[derive(Clone)]
struct Style {
    font: Arc<Font>,
    size: f64,
    color: String,
    fb: bool,
    fi: bool,
    u: bool,
    s: bool,
    hl: Option<String>,
    link: Option<String>,
    rise: f64,
}

fn same(a: &Style, b: &Style) -> bool {
    Arc::ptr_eq(&a.font, &b.font) && a.size == b.size && a.color == b.color && a.fb == b.fb && a.fi == b.fi
        && a.u == b.u && a.s == b.s && a.hl == b.hl && a.link == b.link && a.rise == b.rise
}

#[derive(Clone, Copy, PartialEq)]
enum K {
    Word,
    Glued, // a word piece glued to the next one: no line break in between
    Space,
    Obj,
    Br,
}

#[derive(Clone)]
struct Bx {
    kind: K,
    width: f64,
    asc: f64,
    desc: f64,
    text: String,
    style: Option<Arc<Style>>,
    obj: Option<Arc<Inline>>,
}

#[derive(Clone)]
struct Line {
    boxes: Vec<Bx>,
    width: f64,
    hard: bool,
    asc: f64,
    desc: f64,
}

impl Line {
    fn new(boxes: Vec<Bx>, hard: bool) -> Line {
        let width = boxes.iter().map(|b| b.width).sum();
        let asc = boxes.iter().map(|b| b.asc).fold(0.0, f64::max);
        let desc = boxes.iter().map(|b| b.desc).fold(0.0, f64::max);
        Line { boxes, width, hard, asc, desc }
    }
}

fn trim(mut b: Vec<Bx>) -> Vec<Bx> {
    while b.last().is_some_and(|x| x.kind == K::Space) {
        b.pop();
    }
    b
}

enum Draw {
    Space,
    Page,
    Text { line: Line, x: f64, width: f64, lh: f64, align: Option<String>, last: bool, marker: Option<(String, Arc<Style>)>, quote: bool, left: f64 },
    Code { line: Line, x: f64, width: f64, lh: f64, top: f64, bottom: f64 },
    Rule { x: f64, width: f64 },
    Row { cells: Vec<Vec<(Line, f64, Option<String>)>>, x: f64, cw: Vec<f64>, height: f64, header: bool },
}

struct Item {
    height: f64,
    draw: Draw,
    keep: bool,
    table: Option<usize>,
    header: Option<usize>,
    anchor: Option<(String, String)>,
}

impl Item {
    fn new(height: f64, draw: Draw) -> Item {
        Item { height, draw, keep: false, table: None, header: None, anchor: None }
    }
    fn space(h: f64) -> Item {
        Item::new(h, Draw::Space)
    }
}

pub struct Options {
    pub generic: String,
    pub numbers: Option<bool>,
    pub page: Option<PageSetup>,
}

struct Typesetter<'a> {
    doc: &'a Document,
    page: PageSetup,
    numbers: bool,
    f: Fonts,
    size: f64,
    w: Writer,
    notes: Vec<Vec<Inline>>,
    labels: Vec<Option<String>>,
    tables: usize,
}

fn expand_math(content: &[Inline]) -> Vec<Inline> {
    let mut out = vec![];
    for x in content {
        match x {
            Inline::Run(r) if r.math => {
                for (text, level, italic, bold) in mathtex::runs(&r.text) {
                    out.push(Inline::Run(Run { text, i: italic, b: bold || r.b, sup: level > 0, sub: level < 0, size: r.size,
                                               color: r.color.clone(), link: r.link.clone(), ..Default::default() }));
                }
            }
            other => out.push(other.clone()),
        }
    }
    out
}

impl<'a> Typesetter<'a> {
    fn style(&mut self, run: &Run, look: &Look, bold_override: bool, base: f64, code: bool) -> Style {
        let mono = code || run.code || run.math;
        let bold = run.b || bold_override || (look.bold && !mono);
        let italic = run.i || run.math || look.italic;
        let mut size = run.size.unwrap_or(base);
        let fam = if mono { None } else { run.font.as_deref() };
        let (font, fb, fi) = self.f.get(bold, italic, mono, fam);
        let color = run.color.clone().unwrap_or_else(|| {
            if run.link.is_some() { LINK.into() } else { look.color.unwrap_or(TEXT).into() }
        });
        let mut rise = 0.0;
        if run.sup || run.sub {
            rise = size * if run.sup { 0.33 } else { -0.12 };
            size *= 0.68;
        }
        if mono && !code {
            size *= 0.92;
        }
        Style { font, size, color, fb, fi, u: run.u || run.link.is_some(), s: run.s, hl: run.hl.clone(), link: run.link.clone(), rise }
    }

    fn space(&self, st: &Arc<Style>, n: usize) -> Bx {
        let (asc, desc, _) = st.font.line_metrics(st.size);
        Bx { kind: K::Space, width: st.font.width(" ", st.size) * n as f64, asc, desc, text: " ".repeat(n), style: Some(st.clone()), obj: None }
    }

    fn word_box(&self, text: String, font: &Arc<Font>, st: &Arc<Style>) -> Bx {
        let st = if Arc::ptr_eq(font, &st.font) {
            st.clone()
        } else {
            Arc::new(Style { font: font.clone(), ..(**st).clone() })
        };
        let (asc, desc, _) = font.line_metrics(st.size);
        Bx { kind: K::Word, width: font.width(&text, st.size), asc: asc + st.rise.max(0.0), desc: desc + (-st.rise).max(0.0), text, style: Some(st), obj: None }
    }

    /// A word as boxes, one per font it needs (fallback fonts for missing characters).
    fn word(&mut self, word: &str, st: &Arc<Style>) -> Vec<Bx> {
        let mut out = vec![];
        let mut cur = String::new();
        let mut cur_font: Option<Arc<Font>> = None;
        for ch in word.chars() {
            let f = self.f.for_char(ch, &st.font);
            if let Some(cf) = &cur_font {
                if !Arc::ptr_eq(cf, &f) && !cur.is_empty() {
                    out.push(self.word_box(std::mem::take(&mut cur), cf, st));
                }
            }
            cur_font = Some(f);
            cur.push(ch);
        }
        if let Some(cf) = &cur_font {
            if !cur.is_empty() {
                out.push(self.word_box(cur, cf, st));
            }
        }
        let n = out.len();
        for b in out.iter_mut().take(n.saturating_sub(1)) {
            b.kind = K::Glued;
        }
        out
    }

    fn glue(out: &mut [Bx], tok: &str) {
        if let Some(last) = out.last_mut() {
            if last.kind == K::Word && last.text.chars().last().is_some_and(|c| !is_cjk(c)) && tok.chars().next().is_some_and(|c| !is_cjk(c)) {
                last.kind = K::Glued;
            }
        }
    }

    fn boxes(&mut self, content: &[Inline], look: &Look, bold: bool, base: f64, code: bool) -> Vec<Bx> {
        let mut out: Vec<Bx> = vec![];
        for x in expand_math(content) {
            match &x {
                Inline::Run(r) => {
                    let st = Arc::new(self.style(r, look, bold, base, code));
                    for tok in tokens(&r.text) {
                        if tok == "\n" {
                            out.push(Bx { kind: K::Br, width: 0.0, asc: 0.0, desc: 0.0, text: String::new(), style: None, obj: None });
                        } else if tok.starts_with([' ', '\t']) {
                            let n = if code { tok.replace('\t', "    ").chars().count() } else { 1 };
                            out.push(self.space(&st, n));
                        } else {
                            Self::glue(&mut out, &tok);
                            out.extend(self.word(&tok, &st));
                        }
                    }
                }
                Inline::Break(_) => out.push(Bx { kind: K::Br, width: 0.0, asc: 0.0, desc: 0.0, text: String::new(), style: None, obj: None }),
                Inline::Image(img) => out.push(Bx { kind: K::Obj, width: if img.width > 0.0 { img.width } else { 100.0 }, asc: if img.height > 0.0 { img.height } else { 60.0 }, desc: 0.0, text: String::new(), style: None, obj: Some(Arc::new(x.clone())) }),
                Inline::Chart(c) => out.push(Bx { kind: K::Obj, width: c.width, asc: c.height, desc: 0.0, text: String::new(), style: None, obj: Some(Arc::new(x.clone())) }),
                Inline::Note(n) => {
                    self.notes.push(n.runs.clone());
                    let st = Arc::new(self.style(&Run { sup: true, ..Default::default() }, look, bold, base, false));
                    let label = self.notes.len().to_string();
                    out.extend(self.word(&label, &st));
                }
                Inline::Opaque(o) => {
                    let st = Arc::new(self.style(&Run { color: Some("#7F7F7F".into()), ..Default::default() }, look, bold, base, false));
                    let text = if o.text.trim().is_empty() { format!("[{}]", o.label) } else { o.text.trim().to_string() };
                    for tok in tokens(&text) {
                        if tok.starts_with([' ', '\t', '\n']) {
                            out.push(self.space(&st, 1));
                        } else {
                            out.extend(self.word(&tok, &st));
                        }
                    }
                }
            }
        }
        out
    }

    /// Greedy line breaking.
    fn break_lines(&self, boxes: &[Bx], width: f64) -> Vec<Line> {
        let mut lines = vec![];
        let mut cur: Vec<Bx> = vec![];
        let mut cur_w = 0.0;
        let avail = width;
        let mut i = 0;
        while i < boxes.len() {
            let b = &boxes[i];
            match b.kind {
                K::Br => {
                    lines.push(Line::new(trim(std::mem::take(&mut cur)), true));
                    cur_w = 0.0;
                    i += 1;
                    continue;
                }
                K::Space => {
                    if !cur.is_empty() {
                        cur.push(b.clone());
                        cur_w += b.width;
                    }
                    i += 1;
                    continue;
                }
                _ => {}
            }
            let mut j = i;
            let mut w = 0.0;
            loop {
                w += boxes[j].width;
                if boxes[j].kind != K::Glued || j + 1 >= boxes.len() {
                    break;
                }
                j += 1;
            }
            if !cur.is_empty() && cur_w + w > avail + 0.01 {
                lines.push(Line::new(trim(std::mem::take(&mut cur)), false));
                cur_w = 0.0;
            }
            if cur.is_empty() && w > avail && j == i && b.kind != K::Obj && b.text.chars().count() > 1 {
                // longer than a line: break it between characters
                let st = b.style.clone().unwrap();
                let mut piece = String::new();
                for ch in b.text.chars() {
                    let mut t = piece.clone();
                    t.push(ch);
                    if !piece.is_empty() && st.font.width(&t, st.size) > avail {
                        let nb = Bx { kind: K::Word, width: st.font.width(&piece, st.size), asc: b.asc, desc: b.desc, text: std::mem::take(&mut piece), style: Some(st.clone()), obj: None };
                        lines.push(Line::new(vec![nb], false));
                    }
                    piece.push(ch);
                }
                let nb = Bx { kind: K::Word, width: st.font.width(&piece, st.size), asc: b.asc, desc: b.desc, text: piece, style: Some(st), obj: None };
                cur_w = nb.width;
                cur = vec![nb];
                i = j + 1;
                continue;
            }
            cur.extend_from_slice(&boxes[i..=j]);
            cur_w += w;
            i = j + 1;
        }
        if !cur.is_empty() || lines.is_empty() {
            lines.push(Line::new(trim(cur), true));
        }
        lines
    }

    fn paragraph_items(&mut self, p: &Paragraph, idx: Option<usize>, x: f64, width: f64, first: bool) -> Vec<Item> {
        let lk = look(&p.style);
        let size = self.size * lk.scale;
        let mut items = vec![];
        let mut indent = p.indent as f64 * 18.0;
        if p.style == "quote" {
            indent += if lk.indent > 0.0 { lk.indent } else { 24.0 };
        }
        let mut marker = None;
        if let Some((_, level)) = &p.list {
            indent += 18.0 * (*level as f64 + 1.0);
            let label = idx.and_then(|k| self.labels.get(k).cloned().flatten()).unwrap_or_else(|| "•".into());
            let st = Arc::new(self.style(&Run::default(), &lk, false, size, false));
            marker = Some((label, st));
        }
        let mut boxes = self.boxes(&p.content, &lk, false, size, false);
        let avail = width - indent;
        let th = self.page.text_height();
        for b in boxes.iter_mut().filter(|b| b.kind == K::Obj) {
            let k = (avail / b.width.max(1.0)).min(th * 0.92 / b.asc.max(1.0)).min(1.0);
            b.width *= k;
            b.asc *= k;
        }
        let lines = self.break_lines(&boxes, avail);
        if !first && lk.before > 0.0 {
            items.push(Item::space(lk.before));
        }
        let heading = is_heading(&p.style);
        let align = p.align.clone().or_else(|| if p.style == "title" || p.style == "subtitle" { Some("center".into()) } else { None });
        let n = lines.len();
        for (k, line) in lines.into_iter().enumerate() {
            let only_obj = line.boxes.len() == 1 && line.boxes[0].kind == K::Obj;
            let lh = if only_obj { line.asc + line.desc + 4.0 } else { (size * LINE).max((line.asc + line.desc) * 1.12) };
            let keep = heading || (n >= 3 && (k == 0 || k == n - 2)) || (n == 2 && k == 0);
            let mut item = Item::new(lh, Draw::Text {
                line, x: x + indent, width: avail, lh, align: align.clone(), last: k == n - 1,
                marker: if k == 0 { marker.clone() } else { None }, quote: p.style == "quote", left: x + indent,
            });
            item.keep = keep;
            if heading && k == 0 {
                item.anchor = Some((p.style.clone(), p.text()));
            }
            items.push(item);
        }
        if lk.after > 0.0 {
            items.push(Item::space(lk.after * if p.list.is_some() { 0.5 } else { 1.0 }));
        }
        items
    }

    fn draw_line(&mut self, page: usize, y: f64, line: &Line, x: f64, width: f64, lh: f64, align: Option<&str>, last: bool,
                 marker: Option<&(String, Arc<Style>)>, quote: bool, left: f64) {
        let extra = width - line.width;
        let mut spacing = 0.0;
        let dx = match align {
            Some("center") => extra / 2.0,
            Some("right") => extra,
            Some("justify") if !last && !line.hard => {
                let spaces = line.boxes.iter().filter(|b| b.kind == K::Space).count();
                if spaces > 0 && extra > 0.0 {
                    spacing = extra / spaces as f64;
                }
                0.0
            }
            _ => 0.0,
        };
        let only_obj = line.boxes.len() == 1 && line.boxes[0].kind == K::Obj;
        let base = if only_obj { y + 2.0 + line.asc } else { y + (lh - line.asc - line.desc) / 2.0 + line.asc };
        if quote {
            self.w.rect(page, left - 14.0, y, 2.5, lh, Some(QUOTE_BAR), None, 0.5);
        }
        if let Some((label, st)) = marker {
            let mw = st.font.width(label, st.size);
            self.w.text(page, left - 7.0 - mw, base, &st.font, st.size, label, "#595959", 0.0, st.fb, st.fi, 0.0);
        }
        let mut cx = x + dx;
        let b = &line.boxes;
        let mut k = 0;
        while k < b.len() {
            let bx = &b[k];
            match bx.kind {
                K::Obj => {
                    self.draw_object(page, bx, cx, base - bx.asc);
                    cx += bx.width;
                    k += 1;
                    continue;
                }
                K::Space => {
                    let st = bx.style.as_ref().unwrap();
                    if let Some(hl) = &st.hl {
                        self.w.rect(page, cx, base - bx.asc, bx.width + spacing, bx.asc + bx.desc, Some(hl), None, 0.5);
                    }
                    if st.u || st.s {
                        self.decorate(page, st, cx, base, bx.width + spacing);
                    }
                    cx += bx.width + spacing;
                    k += 1;
                    continue;
                }
                _ => {}
            }
            let st = bx.style.clone().unwrap();
            let mut text = bx.text.clone();
            let mut w = bx.width;
            k += 1;
            while k < b.len() && matches!(b[k].kind, K::Word | K::Glued | K::Space) && b[k].style.as_ref().is_some_and(|s| same(s, &st))
                && !(b[k].kind == K::Space && spacing != 0.0) {
                text.push_str(&b[k].text);
                w += b[k].width;
                k += 1;
            }
            if let Some(hl) = &st.hl {
                self.w.rect(page, cx, base - bx.asc, w, bx.asc + bx.desc, Some(hl), None, 0.5);
            }
            self.w.text(page, cx, base - st.rise, &st.font, st.size, &text, &st.color, 0.0, st.fb, st.fi, 0.0);
            if st.u || st.s {
                self.decorate(page, &st, cx, base - st.rise, w);
            }
            if let Some(link) = &st.link {
                if link.starts_with("http://") || link.starts_with("https://") || link.starts_with("mailto:") || link.starts_with("ftp://") || link.starts_with("www.") {
                    let uri = if link.contains("://") || link.starts_with("mailto:") { link.clone() } else { format!("https://{link}") };
                    self.w.link(page, cx, base - bx.asc, w, bx.asc + bx.desc, Some(&uri), None);
                }
            }
            cx += w;
        }
    }

    fn decorate(&mut self, page: usize, st: &Style, x: f64, base: f64, w: f64) {
        let t = (st.size * 0.055).max(0.4);
        if st.u {
            self.w.line(page, x, base + st.size * 0.13, x + w, base + st.size * 0.13, &st.color, t, None);
        }
        if st.s {
            self.w.line(page, x, base - st.size * 0.28, x + w, base - st.size * 0.28, &st.color, t, None);
        }
    }

    fn draw_object(&mut self, page: usize, b: &Bx, x: f64, y: f64) {
        match b.obj.as_deref() {
            Some(Inline::Image(img)) => {
                let shown = imaging::SHOWN.contains(&imaging::sniff(&img.data));
                let res = if shown { self.w.image_bytes(&img.data).map(|r| (r.name.clone(), r.id, r.px_w, r.px_h)) } else { None };
                if let Some((name, id, pw, ph)) = res {
                    let r = crate::pdf::ImageRes { name, id, px_w: pw, px_h: ph };
                    self.w.image(page, &r, x, y, b.width, b.asc);
                } else {
                    self.w.rect(page, x, y, b.width, b.asc, Some("#EEEEEE"), Some("#BBBBBB"), 0.5);
                    let (f, _, _) = self.f.get(false, false, false, None);
                    let ext = if img.ext.is_empty() { "picture".to_string() } else { img.ext.to_uppercase() };
                    self.w.text(page, x + 6.0, y + b.asc / 2.0, &f, 9.0, &format!("[{ext} picture]"), "#888888", 0.0, false, false, 0.0);
                }
            }
            Some(Inline::Chart(c)) => {
                let regular = self.f.get(false, false, false, None).0;
                let bold = self.f.get(true, false, false, None).0;
                let font_for = move |b: bool| Some(if b { bold.clone() } else { regular.clone() });
                charts::draw_pdf(&mut self.w, page, c, x, y, b.width, b.asc, &font_for);
            }
            _ => {}
        }
    }

    fn code_items(&mut self, block: &CodeBlock, x: f64, width: f64) -> Vec<Item> {
        let lk = look("code");
        let size = self.size * lk.scale;
        let mut items = vec![Item::space(lk.before)];
        let pad = 6.0;
        let mut lines = vec![];
        for raw in block.text.replace('\t', "    ").split('\n') {
            let boxes = self.boxes(&[Inline::Run(Run { text: raw.into(), code: true, ..Default::default() })], &look("normal"), false, size, true);
            if boxes.is_empty() {
                lines.push(Line::new(vec![], true));
            } else {
                lines.extend(self.break_lines(&boxes, width - 2.0 * pad));
            }
        }
        let lh = size * 1.32;
        let n = lines.len();
        for (k, line) in lines.into_iter().enumerate() {
            let top = if k == 0 { pad } else { 0.0 };
            let bottom = if k == n - 1 { pad } else { 0.0 };
            let mut it = Item::new(lh + top + bottom, Draw::Code { line, x, width, lh, top, bottom });
            it.keep = n <= 6 && k + 1 < n;
            items.push(it);
        }
        items.push(Item::space(lk.after));
        items
    }

    fn table_items(&mut self, t: &Table, x: f64, width: f64) -> Vec<Item> {
        let ncols = t.ncols();
        if ncols == 0 {
            return vec![];
        }
        self.tables += 1;
        let tid = self.tables;
        let pad = 4.0;
        let body = self.size * 0.95;
        let mut maxw = vec![0.0f64; ncols];
        let mut minw = vec![0.0f64; ncols];
        let mut cells: Vec<Vec<Vec<(Paragraph, Look, Vec<Bx>)>>> = vec![];
        let empty: Vec<Paragraph> = vec![];
        for (r, row) in t.rows.iter().enumerate() {
            let mut crow = vec![];
            for c in 0..ncols {
                let paras = row.get(c).unwrap_or(&empty);
                let header = t.header && r == 0;
                let mut per = vec![];
                for p in paras {
                    let lk = look(&p.style);
                    let bx = self.boxes(&p.content, &lk, header, body * lk.scale, false);
                    let words = bx.iter().filter(|b| b.kind != K::Space).map(|b| b.width).fold(0.0, f64::max);
                    minw[c] = minw[c].max(words + 2.0 * pad);
                    maxw[c] = maxw[c].max(bx.iter().map(|b| b.width).sum::<f64>() + 2.0 * pad);
                    per.push((p.clone(), lk, bx));
                }
                crow.push(per);
            }
            cells.push(crow);
        }
        let cw: Vec<f64> = match &t.widths {
            Some(ws) if ws.len() == ncols && ws.iter().sum::<f64>() > 0.0 => {
                let total: f64 = ws.iter().sum();
                ws.iter().map(|w| width * w / total).collect()
            }
            _ if maxw.iter().sum::<f64>() <= width => {
                let spare = width - maxw.iter().sum::<f64>();
                let tot = maxw.iter().sum::<f64>().max(1e-6);
                maxw.iter().map(|m| m + spare * m / tot).collect()
            }
            _ => {
                let spare = width - minw.iter().sum::<f64>();
                if spare > 0.0 {
                    let want: Vec<f64> = maxw.iter().zip(&minw).map(|(m, n)| (m - n).max(0.0)).collect();
                    let tot = want.iter().sum::<f64>().max(1.0);
                    minw.iter().zip(&want).map(|(n, w)| n + spare * w / tot).collect()
                } else {
                    vec![width / ncols as f64; ncols]
                }
            }
        };
        let mut items = vec![Item::space(6.0)];
        let mut header_idx: Option<usize> = None; // index in `items`; flow() makes it global
        for (r, crow) in cells.into_iter().enumerate() {
            let mut laid = vec![];
            let mut row_h: f64 = 0.0;
            for (c, paras) in crow.into_iter().enumerate() {
                let mut cl = vec![];
                let mut h = pad;
                for (p, lk, bx) in paras {
                    for line in self.break_lines(&bx, (cw[c] - 2.0 * pad).max(10.0)) {
                        let lh = (body * lk.scale * LINE).max((line.asc + line.desc) * 1.1);
                        h += lh;
                        cl.push((line, lh, p.align.clone()));
                    }
                }
                laid.push(cl);
                row_h = row_h.max(h + pad);
            }
            row_h = row_h.max(body * LINE + 2.0 * pad);
            let is_header = t.header && r == 0;
            let mut item = Item::new(row_h, Draw::Row { cells: laid, x, cw: cw.clone(), height: row_h, header: is_header });
            item.keep = is_header;
            item.table = Some(tid);
            if is_header {
                header_idx = Some(items.len());
            } else {
                item.header = header_idx;
            }
            items.push(item);
        }
        items.push(Item::space(8.0));
        items
    }

    fn rule_items(&self, x: f64, width: f64) -> Vec<Item> {
        vec![Item::new(12.0, Draw::Rule { x, width })]
    }

    fn flow(&mut self) -> Vec<Item> {
        let x = self.page.margins.3;
        let width = self.page.text_width();
        let mut items: Vec<Item> = vec![];
        self.labels = list_labels(&self.doc.blocks);
        let mut first = true;
        let doc = self.doc;
        for (k, b) in doc.blocks.iter().enumerate() {
            let start = items.len();
            let mut new: Vec<Item> = match b {
                Block::Paragraph(p) => self.paragraph_items(p, Some(k), x, width, first),
                Block::Table(t) => self.table_items(t, x, width),
                Block::CodeBlock(c) => self.code_items(c, x, width),
                Block::MathBlock(m) => {
                    let p = Paragraph { content: vec![Inline::Run(Run { text: m.tex.clone(), math: true, ..Default::default() })], align: Some("center".into()), ..Default::default() };
                    let mut v = vec![Item::space(6.0)];
                    v.extend(self.paragraph_items(&p, None, x, width, true));
                    v.push(Item::space(6.0));
                    v
                }
                Block::Rule(_) => self.rule_items(x, width),
                Block::PageBreak(_) => vec![Item::new(0.0, Draw::Page)],
            };
            for it in new.iter_mut() {
                if let Some(h) = it.header {
                    it.header = Some(h + start); // table rows point at their header row
                }
            }
            items.extend(new);
            first = false;
        }
        if !self.notes.is_empty() {
            items.extend(self.rule_items(x, width * 0.3));
            for (k, note) in self.notes.clone().into_iter().enumerate() {
                let mut content = vec![Inline::Run(Run::new(format!("{}. ", k + 1)))];
                content.extend(note);
                let p = Paragraph { content, style: "caption".into(), ..Default::default() };
                items.extend(self.paragraph_items(&p, None, x, width, true));
            }
        }
        items
    }

    fn draw_item(&mut self, page: usize, y: f64, it: &Item) {
        match &it.draw {
            Draw::Text { line, x, width, lh, align, last, marker, quote, left } => {
                self.draw_line(page, y, line, *x, *width, *lh, align.as_deref(), *last, marker.as_ref(), *quote, *left)
            }
            Draw::Code { line, x, width, lh, top, bottom } => {
                self.w.rect(page, *x, y, *width, lh + top + bottom, Some(CODE_BG), None, 0.5);
                let base = y + top + (lh - line.asc - line.desc) / 2.0 + line.asc;
                let mut cx = x + 6.0;
                for b in &line.boxes {
                    if let (Some(st), false) = (&b.style, b.text.is_empty()) {
                        self.w.text(page, cx, base, &st.font, st.size, &b.text, &st.color, 0.0, false, false, 0.0);
                    }
                    cx += b.width;
                }
            }
            Draw::Rule { x, width } => self.w.line(page, *x, y + 6.0, x + width, y + 6.0, BORDER, 0.8, None),
            Draw::Row { cells, x, cw, height, header } => {
                let mut cx = *x;
                for (c, cl) in cells.iter().enumerate() {
                    self.w.rect(page, cx, y, cw[c], *height, if *header { Some(HEAD_BG) } else { None }, Some(BORDER), 0.5);
                    let mut cy = y + 4.0;
                    for (line, lh, align) in cl {
                        self.draw_line(page, cy, line, cx + 4.0, cw[c] - 8.0, *lh, align.as_deref(), true, None, false, cx + 4.0);
                        cy += lh;
                    }
                    cx += cw[c];
                }
            }
            Draw::Space | Draw::Page => {}
        }
    }

    fn paginate(&mut self, items: &[Item]) {
        let top = self.page.margins.0;
        let bottom = self.page.height - self.page.margins.2;
        let mut page = self.w.new_page(self.page.width, self.page.height);
        let mut y = top;
        let n = items.len();
        let mut i = 0;
        while i < n {
            let it = &items[i];
            if matches!(it.draw, Draw::Page) {
                page = self.w.new_page(self.page.width, self.page.height);
                y = top;
                i += 1;
                continue;
            }
            if matches!(it.draw, Draw::Space) {
                if y > top {
                    y = (y + it.height).min(bottom);
                }
                i += 1;
                continue;
            }
            // the chain of items that must stay together (spaces in between do not break it)
            let mut j = i;
            let mut chain = it.height;
            while items[j].keep {
                let mut k = j + 1;
                while k < n && matches!(items[k].draw, Draw::Space) {
                    k += 1;
                }
                if k >= n || matches!(items[k].draw, Draw::Page) {
                    break;
                }
                j = k;
                chain += items[k].height;
            }
            if (y + chain > bottom && y > top && chain <= bottom - top) || (y + it.height > bottom && y > top) {
                page = self.w.new_page(self.page.width, self.page.height);
                y = top;
            }
            if y == top {
                if let (Some(h), Some(_)) = (it.header, it.table) {
                    if h < n {
                        self.draw_item(page, y, &items[h]); // a table continues: repeat its header row
                        y += items[h].height;
                    }
                }
            }
            self.draw_item(page, y, it);
            if let Some((style, text)) = &it.anchor {
                let level = match style.as_str() { "title" | "h1" => Some(0), "h2" => Some(1), "h3" => Some(2), "h4" => Some(3), "h5" => Some(4), "h6" => Some(5), _ => None };
                if let (Some(l), false) = (level, text.trim().is_empty()) {
                    let title: String = text.trim().chars().take(120).collect();
                    self.w.outline.push((title, l, self.w.pages.len() - 1, y));
                }
            }
            y += it.height;
            i += 1;
        }
    }
}

/// Split text into words, runs of spaces, line breaks and single CJK characters.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let mut space = false;
    for ch in text.chars() {
        if ch == '\n' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push("\n".into());
            continue;
        }
        let sp = ch == ' ' || ch == '\t';
        if is_cjk(ch) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(ch.to_string());
            continue;
        }
        if !cur.is_empty() && sp != space {
            out.push(std::mem::take(&mut cur));
        }
        space = sp;
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// PDF bytes for a Document.
pub fn render(doc: &Document, opts: &Options) -> Result<Vec<u8>, String> {
    let page = opts.page.clone().unwrap_or_else(|| doc.page.clone());
    let mut t = Typesetter {
        doc,
        numbers: opts.numbers.unwrap_or(doc.page.numbers),
        page,
        f: Fonts::new(doc.font.clone(), &opts.generic)?,
        size: if doc.size > 0.0 { doc.size } else { 11.0 },
        w: Writer::new(&doc.title, &doc.author),
        notes: vec![],
        labels: vec![],
        tables: 0,
    };
    let items = t.flow();
    t.paginate(&items);
    if t.numbers && t.w.pages.len() > 1 {
        let (f, _, _) = t.f.get(false, false, false, None);
        for k in 0..t.w.pages.len() {
            let label = (k + 1).to_string();
            let size = t.size * 0.85;
            let y = t.page.height - t.page.margins.2 / 2.0;
            let x = (t.page.width - f.width(&label, size)) / 2.0;
            t.w.text(k, x, y, &f, size, &label, "#7F7F7F", 0.0, false, false, 0.0);
        }
    }
    Ok(t.w.finish())
}
