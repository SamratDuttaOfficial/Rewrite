//! Rich Text Format (.rtf) <-> Document.

use crate::convert::cp1252;
use crate::md::Files;
use crate::model::*;
use crate::{charts, imaging, mathtex};
use std::collections::HashMap;

#[derive(Clone, Default)]
struct State {
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    hidden: bool,
    size: Option<f64>,
    color: usize,
    hl: usize,
    font: Option<usize>,
    uc: usize,
    skip: bool, // inside a destination that is not text
    dest: Dest,
}

#[derive(Clone, Default, PartialEq, Debug)]
enum Dest {
    #[default]
    Text,
    FontTable,
    ColorTable,
    StyleSheet,
    Info(String),
    Pict,
    FieldInst,
    FieldResult,
    Footnote,
    ListTable,
    ListOverride,
    Skip,
}

#[derive(Default)]
struct Para {
    style: Option<usize>,
    align: Option<String>,
    indent: i64,
    outline: Option<u32>,
    list: Option<(usize, u32)>,
    in_table: bool,
}

struct Reader {
    fonts: HashMap<usize, String>,
    colors: Vec<Option<String>>,
    styles: HashMap<usize, String>,
    doc: Document,
    para: Para,
    content: Vec<Inline>,
    row: Vec<Vec<Paragraph>>,
    cell: Vec<Paragraph>,
    table: Option<Table>,
    pict: (String, Vec<u8>, f64, f64, f64, f64), // kind, hex bytes, picw, pich, goal w, goal h
    hex_pending: Option<u8>,
    field_inst: String,
    field_link: Option<String>,
    note: Option<Vec<Inline>>,
    font_name: String,
    color: (u8, u8, u8, bool),
    style_name: String,
    style_num: usize,
    info: String,
    def_font: usize,
    list_levels: Vec<String>,
    list_ids: HashMap<i64, Vec<String>>,
    override_list: i64,
    ls_kinds: HashMap<usize, Vec<String>>,
}

impl Reader {
    fn color(&self, k: usize) -> Option<String> {
        self.colors.get(k).cloned().flatten()
    }

    fn push_text(&mut self, st: &State, text: &str) {
        match &st.dest {
            Dest::Text | Dest::FieldResult | Dest::Footnote => {}
            Dest::FontTable => {
                self.font_name.push_str(text);
                return;
            }
            Dest::StyleSheet => {
                self.style_name.push_str(text);
                return;
            }
            Dest::Info(_) => {
                self.info.push_str(text);
                return;
            }
            Dest::FieldInst => {
                self.field_inst.push_str(text);
                return;
            }
            Dest::ColorTable => {
                for _ in text.matches(';') {
                    let (r, g, b, seen) = self.color;
                    self.colors.push(if seen { Some(format!("#{r:02X}{g:02X}{b:02X}")) } else { None });
                    self.color = (0, 0, 0, false);
                }
                return;
            }
            Dest::Pict => {
                for c in text.chars() {
                    if let Some(d) = c.to_digit(16) {
                        match self.hex_pending.take() {
                            Some(h) => self.pict.1.push(h * 16 + d as u8),
                            None => self.hex_pending = Some(d as u8),
                        }
                    }
                }
                return;
            }
            _ => return,
        }
        if st.hidden || st.skip {
            return;
        }
        let run = Run {
            text: text.into(),
            b: st.b,
            i: st.i,
            u: st.u && self.field_link.is_none(),
            s: st.s,
            sup: st.sup,
            sub: st.sub,
            size: st.size.filter(|s| *s != self.doc.size),
            font: st.font.and_then(|f| self.fonts.get(&f).cloned()).filter(|f| Some(f) != self.doc.font.as_ref()),
            color: if st.color > 0 { self.color(st.color) } else { None },
            hl: if st.hl > 0 { self.color(st.hl) } else { None },
            link: if st.dest == Dest::FieldResult { self.field_link.clone() } else { None },
            ..Default::default()
        };
        if st.dest == Dest::Footnote {
            self.note.get_or_insert_with(Vec::new).push(Inline::Run(run));
        } else {
            self.content.push(Inline::Run(run));
        }
    }

    fn end_para(&mut self) {
        let content = strip_content(std::mem::take(&mut self.content));
        let name = self.para.style.and_then(|s| self.styles.get(&s)).cloned().unwrap_or_default().to_lowercase();
        let mut style = if let Some(n) = name.strip_prefix("heading").and_then(|x| x.trim().parse::<u32>().ok()) {
            format!("h{}", n.clamp(1, 6))
        } else {
            match name.as_str() {
                "title" => "title".into(),
                "subtitle" => "subtitle".into(),
                "quote" | "quotation" | "block text" => "quote".into(),
                "caption" => "caption".into(),
                _ => "normal".into(),
            }
        };
        if let (Some(o), "normal") = (self.para.outline, style.as_str()) {
            if o < 9 {
                style = format!("h{}", (o + 1).min(6));
            }
        }
        let mut content = content;
        if style != "normal" {
            // the style's own look is not repeated on every run
            let lk = look(&style);
            for x in content.iter_mut() {
                if let Inline::Run(r) = x {
                    if lk.bold {
                        r.b = false;
                    }
                    if lk.italic {
                        r.i = false;
                    }
                    r.size = None;
                    if r.color.as_deref() == lk.color {
                        r.color = None;
                    }
                }
            }
        }
        let list = self.para.list.map(|(ls, l)| {
            let kind = self.ls_kinds.get(&ls).and_then(|k| k.get(l as usize).or(k.first())).cloned().unwrap_or_else(|| "bullet".into());
            (kind, l.min(8))
        });
        let p = Paragraph {
            content,
            style,
            align: self.para.align.clone(),
            list,
            indent: if self.para.list.is_none() && self.para.indent >= 600 { (self.para.indent / 720).min(6) as u32 } else { 0 },
        };
        if self.para.in_table {
            self.cell.push(p);
        } else {
            self.flush_table();
            self.doc.blocks.push(Block::Paragraph(p));
        }
    }

    fn flush_table(&mut self) {
        if let Some(t) = self.table.take() {
            if !t.rows.is_empty() {
                self.doc.blocks.push(Block::Table(t));
            }
        }
    }

    fn end_pict(&mut self) {
        let (kind, data, pw, ph, gw, gh) = std::mem::take(&mut self.pict);
        if data.is_empty() {
            return;
        }
        let ext = match kind.as_str() { "pngblip" => "png", "jpegblip" => "jpeg", "emfblip" => "emf", "wmetafile" => "wmf", _ => imaging::sniff(&data) };
        let (mut w, mut h) = (gw / 20.0, gh / 20.0);
        if w <= 0.0 || h <= 0.0 {
            let (nw, nh) = imaging::natural_size(&data, Some(451.0));
            w = if pw > 0.0 { pw * 0.75 } else { nw };
            h = if ph > 0.0 { ph * 0.75 } else { nh };
        }
        self.content.push(Inline::Image(Image { data, ext: ext.to_string(), width: w, height: h, alt: String::new(), name: format!("picture.{ext}") }));
    }
}

/// A Document from RTF bytes.
pub fn read(data: &[u8]) -> Document {
    let mut r = Reader {
        fonts: HashMap::new(),
        colors: vec![None],
        styles: HashMap::new(),
        doc: Document { size: 12.0, ..Default::default() },
        para: Para::default(),
        content: vec![],
        row: vec![],
        cell: vec![],
        table: None,
        pict: Default::default(),
        hex_pending: None,
        field_inst: String::new(),
        field_link: None,
        note: None,
        font_name: String::new(),
        color: (0, 0, 0, false),
        style_name: String::new(),
        style_num: 0,
        info: String::new(),
        def_font: 0,
        list_levels: vec![],
        list_ids: HashMap::new(),
        override_list: 0,
        ls_kinds: HashMap::new(),
    };
    let mut stack: Vec<State> = vec![];
    let mut st = State { uc: 1, ..Default::default() };
    let mut i = 0;
    let n = data.len();
    let mut skip_chars = 0usize;
    let mut text = String::new();
    macro_rules! flush_text {
        () => {
            if !text.is_empty() {
                let t = std::mem::take(&mut text);
                r.push_text(&st, &t);
            }
        };
    }
    let mut ignorable_next = false;
    while i < n {
        let c = data[i];
        match c {
            b'{' => {
                flush_text!();
                stack.push(st.clone());
                i += 1;
            }
            b'}' => {
                flush_text!();
                // leaving a destination finishes what it was collecting
                match &st.dest {
                    Dest::FontTable if !r.font_name.is_empty() => {
                        let name = r.font_name.trim().trim_end_matches(';').trim().to_string();
                        if let Some(f) = st.font {
                            r.fonts.insert(f, name);
                        }
                        r.font_name.clear();
                    }
                    Dest::StyleSheet if !r.style_name.is_empty() => {
                        r.styles.insert(r.style_num, r.style_name.trim().trim_end_matches(';').trim().to_string());
                        r.style_name.clear();
                    }
                    Dest::Pict if stack.last().is_none_or(|p| p.dest != Dest::Pict) => r.end_pict(),
                    Dest::Info(field) if stack.last().is_none_or(|p| p.dest != st.dest) => {
                        let v = std::mem::take(&mut r.info).trim().to_string();
                        if field == "title" {
                            r.doc.title = v;
                        } else if field == "author" {
                            r.doc.author = v;
                        }
                    }
                    Dest::FieldInst if stack.last().is_none_or(|p| p.dest != Dest::FieldInst) => {
                        let inst = std::mem::take(&mut r.field_inst);
                        if let Some(m) = regex::Regex::new(r#"HYPERLINK\s+"([^"]+)""#).unwrap().captures(&inst) {
                            r.field_link = Some(m[1].to_string());
                        }
                    }
                    Dest::FieldResult if stack.last().is_none_or(|p| p.dest != Dest::FieldResult) => r.field_link = None,
                    Dest::Footnote if stack.last().is_none_or(|p| p.dest != Dest::Footnote) => {
                        if let Some(runs) = r.note.take() {
                            r.content.push(Inline::Note(Note { runs: strip_content(runs) }));
                        }
                    }
                    _ => {}
                }
                st = stack.pop().unwrap_or_default();
                i += 1;
            }
            b'\\' => {
                flush_text!();
                i += 1;
                if i >= n {
                    break;
                }
                let d = data[i];
                if d.is_ascii_alphabetic() {
                    let start = i;
                    while i < n && data[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = String::from_utf8_lossy(&data[start..i]).into_owned();
                    let pstart = i;
                    if i < n && (data[i] == b'-' || data[i].is_ascii_digit()) {
                        i += 1;
                        while i < n && data[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i64> = if i > pstart { String::from_utf8_lossy(&data[pstart..i]).parse().ok() } else { None };
                    if i < n && data[i] == b' ' {
                        i += 1;
                    }
                    if skip_chars > 0 && word != "bin" {
                        skip_chars -= 1;
                        continue;
                    }
                    let ign = ignorable_next;
                    ignorable_next = false;
                    control(&mut r, &mut st, &word, param, ign, &mut skip_chars, data, &mut i);
                } else {
                    i += 1;
                    match d {
                        b'\'' => {
                            if i + 2 <= n {
                                let h = u8::from_str_radix(&String::from_utf8_lossy(&data[i..i + 2]), 16).unwrap_or(b'?');
                                i += 2;
                                if skip_chars > 0 {
                                    skip_chars -= 1;
                                } else {
                                    text.push(cp1252(h));
                                }
                            }
                        }
                        b'*' => ignorable_next = true,
                        b'~' => text.push('\u{a0}'),
                        b'_' => text.push('\u{2011}'),
                        b'-' => {}
                        b'\n' | b'\r' => {
                            if st.dest == Dest::Text {
                                r.end_para();
                            }
                        }
                        b'\\' | b'{' | b'}' => text.push(d as char),
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if skip_chars > 0 {
                    skip_chars -= 1;
                    i += 1;
                    continue;
                }
                // plain text: UTF-8 is not expected, but take it if it is there
                let ch_len = if c < 0x80 { 1 } else { std::str::from_utf8(&data[i..(i + 4).min(n)]).map(|s| s.chars().next().map(|c| c.len_utf8()).unwrap_or(1)).unwrap_or(1) };
                if ch_len > 1 {
                    if let Ok(s) = std::str::from_utf8(&data[i..i + ch_len]) {
                        text.push_str(s);
                    }
                } else {
                    text.push(cp1252(c));
                }
                i += ch_len;
            }
        }
    }
    flush_text!();
    if !r.content.is_empty() {
        r.end_para();
    }
    r.flush_table();
    if r.doc.font.is_none() {
        r.doc.font = r.fonts.get(&r.def_font).cloned();
    }
    r.doc
}

#[allow(clippy::too_many_arguments)]
fn control(r: &mut Reader, st: &mut State, word: &str, param: Option<i64>, ignorable: bool, skip_chars: &mut usize, data: &[u8], i: &mut usize) {
    let on = param.map(|p| p != 0).unwrap_or(true);
    let p = param.unwrap_or(0);
    if st.dest == Dest::Skip {
        // nothing inside a skipped part counts (a page number field in a footer, a second copy of a picture ...)
        if word == "bin" {
            *i += p.max(0) as usize;
        }
        return;
    }
    if word == "pard" && !matches!(st.dest, Dest::Text | Dest::FieldResult) {
        return; // \pard inside \listtext and the like must not reset the paragraph
    }
    match word {
        // destinations
        "fonttbl" => st.dest = Dest::FontTable,
        "colortbl" => {
            st.dest = Dest::ColorTable;
            r.colors.clear();
            r.color = (0, 0, 0, false);
        }
        "stylesheet" => st.dest = Dest::StyleSheet,
        "info" => st.dest = Dest::Skip,
        "title" | "author" => {
            st.dest = Dest::Info(word.into());
            st.skip = false;
        }
        "pict" => {
            st.dest = Dest::Pict;
            r.pict = Default::default();
            r.hex_pending = None;
        }
        "pngblip" | "jpegblip" | "emfblip" | "wmetafile" | "dibitmap" | "wbitmap" => r.pict.0 = word.into(),
        "picw" => r.pict.2 = p as f64,
        "pich" => r.pict.3 = p as f64,
        "picwgoal" => r.pict.4 = p as f64,
        "pichgoal" => r.pict.5 = p as f64,
        "shppict" => {}
        "nonshppict" => st.dest = Dest::Skip,
        "fldinst" => st.dest = Dest::FieldInst,
        "fldrslt" => st.dest = Dest::FieldResult,
        "footnote" => {
            st.dest = Dest::Footnote;
            r.note = Some(vec![]);
        }
        "header" | "footer" | "headerl" | "headerr" | "headerf" | "footerl" | "footerr" | "footerf" | "pntext" | "pntxta"
        | "pntxtb" | "listtext" | "leveltext" | "levelnumbers" | "listname" | "rsidtbl" | "generator" | "xmlnstbl" | "themedata"
        | "colorschememapping" | "latentstyles" | "datastore" | "mmathPr" | "pgdsctbl" | "operator" | "company"
        | "revtbl" | "bkmkstart" | "bkmkend" | "object" | "objdata" | "doccomm" | "subject" | "keywords" | "comment" => {
            st.dest = Dest::Skip
        }
        "bin" => {
            *i += p.max(0) as usize; // binary data
            let _ = data;
        }
        // character formatting
        "plain" => {
            let keep = (st.dest.clone(), st.uc, st.skip);
            *st = State { dest: keep.0, uc: keep.1, skip: keep.2, ..Default::default() };
        }
        "b" => st.b = on,
        "i" => st.i = on,
        "ul" | "uld" | "uldb" | "ulw" | "ulwave" | "uldash" => st.u = on,
        "ulnone" => st.u = false,
        "strike" | "striked" => st.s = on,
        "super" => {
            st.sup = true;
            st.sub = false;
        }
        "sub" => {
            st.sub = true;
            st.sup = false;
        }
        "nosupersub" => {
            st.sup = false;
            st.sub = false;
        }
        "v" => st.hidden = on,
        "fs" => st.size = Some(p as f64 / 2.0),
        "cf" => st.color = p.max(0) as usize,
        "highlight" | "cb" | "chcbpat" => st.hl = p.max(0) as usize,
        "f" => {
            st.font = Some(p.max(0) as usize);
            if st.dest == Dest::FontTable {
                r.font_name.clear();
            }
        }
        "deff" => r.def_font = p.max(0) as usize,
        "uc" => st.uc = p.max(0) as usize,
        "u" => {
            let code = if p < 0 { (p + 65536) as u32 } else { p as u32 };
            if let Some(ch) = char::from_u32(code) {
                r.push_text(st, &ch.to_string());
            }
            *skip_chars = st.uc;
        }
        // colour table entries
        "red" => r.color = (p as u8, r.color.1, r.color.2, true),
        "green" => r.color = (r.color.0, p as u8, r.color.2, true),
        "blue" => r.color = (r.color.0, r.color.1, p as u8, true),
        // style sheet entries
        "s" => {
            if st.dest == Dest::StyleSheet {
                r.style_num = p.max(0) as usize;
                r.style_name.clear();
            } else {
                r.para.style = Some(p.max(0) as usize);
            }
        }
        // paragraphs
        "pard" => {
            let in_table = false;
            r.para = Para { in_table, ..Default::default() };
        }
        "par" => {
            if matches!(st.dest, Dest::Text | Dest::FieldResult) {
                r.end_para();
            } else if st.dest == Dest::Footnote {
                if let Some(n) = &mut r.note {
                    n.push(Inline::Run(Run::new(" ")));
                }
            }
        }
        "line" => r.content.push(Inline::Break(Break::default())),
        "tab" => r.push_text(st, "\t"),
        "page" => {
            r.end_para();
            r.doc.blocks.push(Block::PageBreak(PageBreak {}));
        }
        "qc" => r.para.align = Some("center".into()),
        "qr" => r.para.align = Some("right".into()),
        "qj" => r.para.align = Some("justify".into()),
        "ql" => r.para.align = None,
        "li" => r.para.indent = p,
        "outlinelevel" => r.para.outline = Some(p.max(0) as u32),
        "listtable" => st.dest = Dest::ListTable,
        "listoverridetable" => st.dest = Dest::ListOverride,
        "list" if st.dest == Dest::ListTable => r.list_levels.clear(),
        "listlevel" if st.dest == Dest::ListTable => r.list_levels.push("bullet".into()),
        "levelnfc" | "levelnfcn" if st.dest == Dest::ListTable => {
            if let Some(last) = r.list_levels.last_mut() {
                *last = if p == 23 || p == 255 { "bullet".into() } else { "number".into() };
            }
        }
        "listid" if st.dest == Dest::ListTable => {
            r.list_ids.insert(p, r.list_levels.clone());
        }
        "listid" if st.dest == Dest::ListOverride => r.override_list = p,
        "ls" if st.dest == Dest::ListOverride => {
            let kinds = r.list_ids.get(&r.override_list).cloned().unwrap_or_default();
            r.ls_kinds.insert(p.max(0) as usize, kinds);
        }
        "ls" => r.para.list = Some((p.max(0) as usize, r.para.list.map(|l| l.1).unwrap_or(0))),
        "ilvl" => r.para.list = Some((r.para.list.map(|l| l.0).unwrap_or(1), p.max(0) as u32)),
        "intbl" => r.para.in_table = true,
        "cell" => {
            r.end_para();
            let cell = std::mem::take(&mut r.cell);
            r.row.push(if cell.is_empty() { vec![Paragraph::default()] } else { cell });
        }
        "row" => {
            let row = std::mem::take(&mut r.row);
            let t = r.table.get_or_insert_with(|| Table { rows: vec![], header: false, widths: None });
            t.rows.push(row);
        }
        "trowd" => {}
        "emdash" => r.push_text(st, "—"),
        "endash" => r.push_text(st, "–"),
        "lquote" => r.push_text(st, "‘"),
        "rquote" => r.push_text(st, "’"),
        "ldblquote" => r.push_text(st, "“"),
        "rdblquote" => r.push_text(st, "”"),
        "bullet" => r.push_text(st, "•"),
        "emspace" | "enspace" | "qmspace" => r.push_text(st, " "),
        "paperw" => r.doc.page.width = p as f64 / 20.0,
        "paperh" => r.doc.page.height = p as f64 / 20.0,
        "margt" => r.doc.page.margins.0 = p as f64 / 20.0,
        "margr" => r.doc.page.margins.1 = p as f64 / 20.0,
        "margb" => r.doc.page.margins.2 = p as f64 / 20.0,
        "margl" => r.doc.page.margins.3 = p as f64 / 20.0,
        _ => {
            if ignorable {
                st.dest = Dest::Skip;
            }
        }
    }
}

// -- writing ------------------------------------------------------------------------

fn esc(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            '\n' => out.push_str("\\line "),
            '\u{a0}' => out.push_str("\\~"),
            c if (c as u32) < 128 => out.push(c),
            c => {
                for u in c.encode_utf16(&mut [0; 2]) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

struct W {
    fonts: Vec<String>,
    colors: Vec<String>,
    notes: usize,
}

impl W {
    fn font(&mut self, name: &str) -> usize {
        if let Some(k) = self.fonts.iter().position(|f| f == name) {
            return k;
        }
        self.fonts.push(name.into());
        self.fonts.len() - 1
    }

    fn color(&mut self, c: &str) -> usize {
        if let Some(k) = self.colors.iter().position(|x| x.eq_ignore_ascii_case(c)) {
            return k + 1;
        }
        self.colors.push(c.into());
        self.colors.len()
    }

    fn inline(&mut self, content: &[Inline], base_size: f64) -> String {
        let mut out = String::new();
        for x in merge_runs(content.to_vec()) {
            match &x {
                Inline::Run(r) => {
                    let mut f = String::new();
                    if r.b {
                        f.push_str("\\b");
                    }
                    if r.i || r.math {
                        f.push_str("\\i");
                    }
                    if r.u {
                        f.push_str("\\ul");
                    }
                    if r.s {
                        f.push_str("\\strike");
                    }
                    if r.sup {
                        f.push_str("\\super");
                    }
                    if r.sub {
                        f.push_str("\\sub");
                    }
                    if let Some(s) = r.size {
                        if s != base_size {
                            f.push_str(&format!("\\fs{}", (s * 2.0).round()));
                        }
                    }
                    if r.code {
                        let k = self.font("Consolas");
                        f.push_str(&format!("\\f{k}"));
                    } else if let Some(name) = &r.font {
                        let k = self.font(name);
                        f.push_str(&format!("\\f{k}"));
                    }
                    if let Some(c) = &r.color {
                        let k = self.color(c);
                        f.push_str(&format!("\\cf{k}"));
                    }
                    if let Some(h) = &r.hl {
                        let k = self.color(h);
                        f.push_str(&format!("\\highlight{k}"));
                    }
                    let text = if r.math { mathtex::to_unicode(&r.text) } else { r.text.clone() };
                    let body = if f.is_empty() { format!("{{{}}}", esc(&text)) } else { format!("{{{f} {}}}", esc(&text)) };
                    match &r.link {
                        Some(l) => out.push_str(&format!("{{\\field{{\\*\\fldinst HYPERLINK \"{}\"}}{{\\fldrslt {{\\ul\\cf{} {}}}}}}}", esc(l), self.color("#1F5FBF"), esc(&text))),
                        None => out.push_str(&body),
                    }
                }
                Inline::Break(_) => out.push_str("\\line "),
                Inline::Image(img) => {
                    let (data, kind) = match img.ext.as_str() {
                        "png" => (img.data.clone(), "pngblip"),
                        "jpeg" => (img.data.clone(), "jpegblip"),
                        "emf" => (img.data.clone(), "emfblip"),
                        _ => match imaging::to_png(&img.data) {
                            Ok(p) => (p, "pngblip"),
                            Err(_) => continue,
                        },
                    };
                    let (pw, ph) = imaging::size(&data).unwrap_or((100, 100));
                    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                    out.push_str(&format!("{{\\pict\\{kind}\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{} {hex}}}", (img.width * 20.0) as i64, (img.height * 20.0) as i64));
                }
                Inline::Chart(c) => out.push_str(&format!("{{\\i {}}}", esc(&charts::describe(c)))),
                Inline::Note(n) => {
                    self.notes += 1;
                    let inner = self.inline(&n.runs, base_size);
                    out.push_str(&format!("{{\\super\\chftn}}{{\\footnote\\pard\\plain{{\\super\\chftn}} {inner}}}"));
                }
                Inline::Opaque(o) => out.push_str(&esc(&o.text)),
            }
        }
        out
    }
}

/// RTF text for a Document.
pub fn write(doc: &Document) -> (Vec<u8>, Files) {
    let base_font = doc.font.clone().unwrap_or_else(|| "Calibri".into());
    let size = if doc.size > 0.0 { doc.size } else { 11.0 };
    let mut w = W { fonts: vec![base_font], colors: vec![], notes: 0 };
    let mut body = String::new();
    let labels = list_labels(&doc.blocks);
    // every numbered list gets its own list definition, so its numbering starts at 1
    let mut numbered = 0usize;
    let mut list_of: Vec<usize> = vec![0; doc.blocks.len()];
    let mut prev_list = false;
    for (k, b) in doc.blocks.iter().enumerate() {
        if let Block::Paragraph(Paragraph { list: Some((kind, _)), .. }) = b {
            if kind == "number" {
                if !prev_list || numbered == 0 {
                    numbered += 1;
                }
                list_of[k] = 1 + numbered;
            } else {
                list_of[k] = 1;
            }
            prev_list = true;
        } else {
            prev_list = false;
        }
    }
    let style_no = |s: &str| -> usize {
        match s {
            "h1" => 1, "h2" => 2, "h3" => 3, "h4" => 4, "h5" => 5, "h6" => 6, "title" => 7, "subtitle" => 8, "quote" => 9,
            "caption" => 10, _ => 0,
        }
    };
    for (k, b) in doc.blocks.iter().enumerate() {
        match b {
            Block::Paragraph(p) => {
                let lk = look(&p.style);
                let mut pf = format!("\\pard\\plain\\s{}", style_no(&p.style));
                match p.align.as_deref().or(if p.style == "title" || p.style == "subtitle" { Some("center") } else { None }) {
                    Some("center") => pf.push_str("\\qc"),
                    Some("right") => pf.push_str("\\qr"),
                    Some("justify") => pf.push_str("\\qj"),
                    _ => {}
                }
                if let Some(n) = p.style.strip_prefix('h').and_then(|n| n.parse::<u32>().ok()) {
                    pf.push_str(&format!("\\outlinelevel{}", n - 1));
                }
                pf.push_str(&format!("\\sb{}\\sa{}", (lk.before * 20.0) as i64, (lk.after * 20.0) as i64));
                let mut cf = format!("\\fs{}", (size * lk.scale * 2.0).round());
                if lk.bold {
                    cf.push_str("\\b");
                }
                if lk.italic {
                    cf.push_str("\\i");
                }
                if let Some(c) = lk.color {
                    if p.style != "normal" {
                        let k2 = w.color(c);
                        cf.push_str(&format!("\\cf{k2}"));
                    }
                }
                if let Some((_, level)) = &p.list {
                    let left = 720 * (level + 1);
                    // the label for readers without list support comes first, as Word writes it
                    pf.push_str(&format!("{{\\listtext\\pard\\plain {}\\tab}}\\ls{}\\ilvl{level}\\fi-360\\li{left}",
                                         esc(labels[k].as_deref().unwrap_or("•")), list_of[k]));
                } else if p.indent > 0 || p.style == "quote" {
                    pf.push_str(&format!("\\li{}", 720 * p.indent.max(1)));
                }
                let text = w.inline(&p.content, size * lk.scale);
                body.push_str(&format!("{pf}{{{cf} {text}}}\\par\n"));
            }
            Block::Table(t) => {
                let n = t.ncols().max(1);
                let tw = (doc.page.text_width() * 20.0) as i64;
                let cells: String = (1..=n).map(|c| format!("\\clbrdrt\\brdrs\\clbrdrl\\brdrs\\clbrdrb\\brdrs\\clbrdrr\\brdrs\\cellx{}", tw * c as i64 / n as i64)).collect();
                for (r, row) in t.rows.iter().enumerate() {
                    body.push_str(&format!("\\trowd\\trgaph108{cells}\n"));
                    for c in 0..n {
                        let paras = row.get(c).cloned().unwrap_or_default();
                        let text = paras.iter().map(|p| w.inline(&p.content, size)).collect::<Vec<_>>().join("\\line ");
                        let b = if t.header && r == 0 { "\\b" } else { "" };
                        body.push_str(&format!("\\pard\\intbl{{{b} {text}}}\\cell\n"));
                    }
                    body.push_str("\\row\n");
                }
                body.push_str("\\pard\n");
            }
            Block::CodeBlock(c) => {
                let k2 = w.font("Consolas");
                for line in c.text.split('\n') {
                    body.push_str(&format!("\\pard\\plain\\sa0{{\\f{k2}\\fs{} {}}}\\par\n", (size * 0.88 * 2.0).round(), esc(line)));
                }
            }
            Block::MathBlock(m) => body.push_str(&format!("\\pard\\plain\\qc{{\\i {}}}\\par\n", esc(&mathtex::to_unicode(&m.tex)))),
            Block::Rule(_) => body.push_str("\\pard\\plain\\brdrb\\brdrs\\brdrw10\\brsp20 \\par\n"),
            Block::PageBreak(_) => body.push_str("\\page\n"),
        }
    }
    let fonts: String = w.fonts.iter().enumerate().map(|(k, f)| format!("{{\\f{k}\\fnil {};}}", esc(f))).collect();
    let colors: String = w.colors.iter().map(|c| {
        let (r, g, b) = crate::pdf::rgb(c);
        format!("\\red{}\\green{}\\blue{};", (r * 255.0).round(), (g * 255.0).round(), (b * 255.0).round())
    }).collect();
    let pg = &doc.page;
    let info = if doc.title.is_empty() { String::new() } else { format!("{{\\info{{\\title {}}}{{\\author {}}}}}", esc(&doc.title), esc(&doc.author)) };
    // style sheet: headings and the other paragraph styles, so Word shows them as such
    let mut styles = String::from("{\\stylesheet{\\s0 Normal;}");
    for (n, (key, name)) in [("h1", "heading 1"), ("h2", "heading 2"), ("h3", "heading 3"), ("h4", "heading 4"), ("h5", "heading 5"),
                              ("h6", "heading 6"), ("title", "Title"), ("subtitle", "Subtitle"), ("quote", "Quote"), ("caption", "caption")]
        .iter().enumerate() {
        let lk = look(key);
        let outline = key.strip_prefix('h').and_then(|x| x.parse::<u32>().ok()).map(|x| format!("\\outlinelevel{}", x - 1)).unwrap_or_default();
        styles.push_str(&format!("{{\\s{}\\sbasedon0\\snext0{outline}\\fs{}{}{} {name};}}", n + 1, (size * lk.scale * 2.0).round(),
                                 if lk.bold { "\\b" } else { "" }, if lk.italic { "\\i" } else { "" }));
    }
    styles.push('}');
    // lists: one bulleted list and one numbered list for each run of numbered paragraphs
    let level = |k: usize, number: bool| -> String {
        let indent = format!("\\fi-360\\li{}", 720 * (k + 1));
        if number {
            let nfc = [0, 4, 2][k % 3];
            format!("{{\\listlevel\\levelnfc{nfc}\\levelnfcn{nfc}\\leveljc0\\levelstartat1\\levelfollow0{{\\leveltext\\'02\\'0{k}.;}}{{\\levelnumbers\\'01;}}{indent}}}")
        } else {
            let code = ['•', '◦', '▪'][k % 3] as u32;
            format!("{{\\listlevel\\levelnfc23\\levelnfcn23\\leveljc0\\levelstartat1\\levelfollow0{{\\leveltext\\'01\\u{code} ?;}}{{\\levelnumbers;}}{indent}}}")
        }
    };
    let mut lists = format!("{{\\*\\listtable{{\\list\\listtemplateid1\\listhybrid{}\\listid1}}", (0..9).map(|k| level(k, false)).collect::<String>());
    let mut overrides = String::from("{\\*\\listoverridetable{\\listoverride\\listid1\\listoverridecount0\\ls1}");
    for n in 0..numbered {
        let id = n + 2;
        lists.push_str(&format!("{{\\list\\listtemplateid{id}\\listhybrid{}\\listid{id}}}", (0..9).map(|k| level(k, true)).collect::<String>()));
        overrides.push_str(&format!("{{\\listoverride\\listid{id}\\listoverridecount0\\ls{id}}}"));
    }
    lists.push('}');
    overrides.push('}');
    let out = format!(
        "{{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1{{\\fonttbl{fonts}}}{{\\colortbl;{colors}}}{styles}{lists}{overrides}{info}\\paperw{}\\paperh{}\\margt{}\\margr{}\\margb{}\\margl{}\\fs{}\n{body}}}",
        (pg.width * 20.0) as i64, (pg.height * 20.0) as i64, (pg.margins.0 * 20.0) as i64, (pg.margins.1 * 20.0) as i64,
        (pg.margins.2 * 20.0) as i64, (pg.margins.3 * 20.0) as i64, (size * 2.0).round());
    (out.into_bytes(), Files::new())
}
