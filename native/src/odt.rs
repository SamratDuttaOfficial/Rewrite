//! OpenDocument text (.odt, and the single-file .fodt) <-> Document.
//!
//! LibreOffice's own format. Reading follows the named and automatic styles (bold, italic, sizes, colours,
//! headings, quotations, preformatted text, captions), lists, tables, pictures, links, footnotes, formulas
//! and page size. Charts are written as pictures (SVG) that carry their data, so they open again as
//! editable charts; equations are written as LibreOffice formula objects (MathML with the TeX kept).

use crate::model::*;
use crate::opc::{self, Package};
use crate::xml::{esc, esc_attr, find, is, kid};
use crate::{charts, imaging, mathtex};
use base64::Engine;
use regex::Regex;
use roxmltree::Node;
use std::collections::{BTreeMap, HashMap};

const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const XLINK: &str = "http://www.w3.org/1999/xlink";
const SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
const MATH: &str = "http://www.w3.org/1998/Math/MathML";
const CHART_MARK: &str = "rewrite-chart:";

const DECL: [(&str, &str); 12] = [
    ("office", OFFICE), ("style", STYLE), ("text", TEXT), ("table", TABLE), ("draw", DRAW), ("fo", FO), ("xlink", XLINK),
    ("dc", "http://purl.org/dc/elements/1.1/"), ("meta", "urn:oasis:names:tc:opendocument:xmlns:meta:1.0"), ("svg", SVG),
    ("math", MATH), ("loext", "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0"),
];

pub fn length_pt(v: &str) -> Option<f64> {
    let m = Regex::new(r"^\s*(-?[\d.]+)\s*(pt|cm|mm|in|px|pc)?\s*$").unwrap().captures(v)?;
    let n: f64 = m[1].parse().ok()?;
    Some(n * match m.get(2).map(|x| x.as_str()) { Some("cm") => 28.3465, Some("mm") => 2.83465, Some("in") => 72.0, Some("px") => 0.75, Some("pc") => 12.0, _ => 1.0 })
}

#[derive(Default, Clone, Debug)]
struct TextProps {
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    s: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
    size: Option<f64>,
    color: Option<String>,
    hl: Option<String>,
    font: Option<String>,
}

impl TextProps {
    fn read(tp: Node) -> TextProps {
        let a = |ns: &str, n: &str| tp.attribute((ns, n));
        let mut p = TextProps::default();
        match a(FO, "font-weight") {
            Some("bold" | "600" | "700" | "800" | "900") => p.b = Some(true),
            Some("normal") => p.b = Some(false),
            _ => {}
        }
        match a(FO, "font-style") {
            Some("italic" | "oblique") => p.i = Some(true),
            Some("normal") => p.i = Some(false),
            _ => {}
        }
        if a(STYLE, "text-underline-style").is_some_and(|v| v != "none") {
            p.u = Some(true);
        }
        if a(STYLE, "text-line-through-style").is_some_and(|v| v != "none") {
            p.s = Some(true);
        }
        if let Some(pos) = a(STYLE, "text-position") {
            let first = pos.split_whitespace().next().unwrap_or("");
            let num = first.trim_end_matches('%').parse::<f64>().ok();
            if first == "super" || num.is_some_and(|n| n > 0.0) {
                p.sup = Some(true);
            } else if first == "sub" || num.is_some_and(|n| n < 0.0) {
                p.sub = Some(true);
            }
        }
        if let Some(s) = a(FO, "font-size") {
            if !s.ends_with('%') {
                p.size = length_pt(s);
            }
        }
        let hex = |v: &str| v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit());
        if let Some(c) = a(FO, "color").filter(|c| hex(c) && !c.eq_ignore_ascii_case("#000000")) {
            p.color = Some(c.to_uppercase());
        }
        if let Some(c) = a(FO, "background-color").filter(|c| hex(c) && !c.eq_ignore_ascii_case("#FFFFFF")) {
            p.hl = Some(c.to_uppercase());
        }
        if let Some(f) = a(STYLE, "font-name").or(a(FO, "font-family")) {
            p.font = Some(f.trim_matches('\'').to_string());
        }
        p
    }

    fn under(&mut self, o: &TextProps) {
        macro_rules! fill { ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = o.$f.clone(); } )* }; }
        fill!(b, i, u, s, sup, sub, size, color, hl, font);
    }
}

#[derive(Default, Clone, Debug)]
struct StyleInfo {
    parent: Option<String>,
    display: String,
    text: TextProps,
    align: Option<String>,
    margin: Option<f64>,
    brk: Option<String>,
    border: Option<String>,
    outline: Option<u32>,
}

struct Reader<'f> {
    files: &'f BTreeMap<String, Vec<u8>>,
    styles: HashMap<(String, String), StyleInfo>,
    lists: HashMap<String, HashMap<u32, String>>,
    doc: Document,
}

impl<'f> Reader<'f> {
    fn read_styles(&mut self, container: Node) {
        for st in container.children().filter(|n| n.is_element()) {
            if is(&st, STYLE, "style") {
                let family = st.attribute((STYLE, "family")).unwrap_or("").to_string();
                let name = st.attribute((STYLE, "name")).unwrap_or("").to_string();
                let mut info = StyleInfo {
                    parent: st.attribute((STYLE, "parent-style-name")).map(String::from),
                    display: st.attribute((STYLE, "display-name")).unwrap_or(&name).to_string(),

                    outline: st.attribute((STYLE, "default-outline-level")).and_then(|v| v.parse().ok()),
                    ..Default::default()
                };
                if let Some(tp) = kid(st, STYLE, "text-properties") {
                    info.text = TextProps::read(tp);
                }
                if let Some(pp) = kid(st, STYLE, "paragraph-properties") {
                    info.align = pp.attribute((FO, "text-align")).map(String::from);
                    info.margin = pp.attribute((FO, "margin-left")).and_then(length_pt);
                    info.brk = pp.attribute((FO, "break-before")).map(String::from);
                    info.border = pp.attribute((FO, "border-bottom")).or(pp.attribute((FO, "border"))).map(String::from);
                }
                self.styles.insert((family, name), info);
            } else if is(&st, TEXT, "list-style") {
                let mut levels = HashMap::new();
                for lv in st.children().filter(|n| n.is_element()) {
                    let k: u32 = lv.attribute((TEXT, "level")).and_then(|v| v.parse().ok()).unwrap_or(1);
                    let kind = match lv.tag_name().name() { "list-level-style-bullet" => "bullet", "list-level-style-number" => "number", _ => "none" };
                    levels.insert(k.saturating_sub(1), kind.to_string());
                }
                self.lists.insert(st.attribute((STYLE, "name")).unwrap_or("").to_string(), levels);
            }
        }
    }

    fn chain(&self, family: &str, name: Option<&str>) -> Vec<(String, StyleInfo)> {
        let mut out = vec![];
        let mut name = name.map(String::from);
        for _ in 0..12 {
            let Some(n) = name else { break };
            let Some(st) = self.styles.get(&(family.to_string(), n.clone())) else { break };
            out.push((n, st.clone()));
            name = st.parent.clone();
        }
        out
    }

    fn para_kind(&self, name: Option<&str>) -> String {
        for (n, st) in self.chain("paragraph", name) {
            let disp = if st.display.is_empty() { n.clone() } else { st.display.clone() }.to_lowercase().replace("_20_", " ");
            match disp.as_str() {
                "title" => return "title".into(),
                "subtitle" => return "subtitle".into(),
                "quotations" | "quote" | "quotation" => return "quote".into(),
                "preformatted text" | "source text" | "source code" | "code" => return "code".into(),
                "caption" | "illustration" | "figure" | "table" | "drawing" => return "caption".into(),
                _ => {}
            }
            if let Some(k) = disp.strip_prefix("heading ").and_then(|x| x.parse::<u32>().ok()) {
                return format!("h{}", k.clamp(1, 6));
            }
            if let Some(o) = st.outline {
                return format!("h{}", o.clamp(1, 6));
            }
        }
        "normal".into()
    }

    fn text_props(&self, family: &str, name: Option<&str>) -> TextProps {
        let mut out = TextProps::default();
        for (_, st) in self.chain(family, name) {
            out.under(&st.text);
        }
        out
    }

    fn blocks(&self, parent: Node, lst: Option<(String, u32, Option<String>)>) -> Vec<Block> {
        let mut out = vec![];
        for el in parent.children().filter(|n| n.is_element()) {
            let ns = el.tag_name().namespace();
            let name = el.tag_name().name();
            if ns == Some(TEXT) {
                match name {
                    "p" | "h" => out.extend(self.paragraph(el, lst.clone())),
                    "list" => out.extend(self.list(el, lst.as_ref().and_then(|l| l.2.clone()), lst.as_ref().map(|l| l.1 + 1).unwrap_or(0))),
                    "section" | "index-body" | "soft-page-break" => out.extend(self.blocks(el, lst.clone())),
                    "table-of-content" | "alphabetical-index" | "bibliography" | "illustration-index" | "table-index" | "user-index" => {
                        if let Some(b) = kid(el, TEXT, "index-body") {
                            out.extend(self.blocks(b, lst.clone()));
                        }
                    }
                    _ => {}
                }
            } else if is(&el, TABLE, "table") {
                out.push(Block::Table(self.table(el)));
            }
        }
        out
    }

    fn list(&self, el: Node, inherited: Option<String>, level: u32) -> Vec<Block> {
        let style = el.attribute((TEXT, "style-name")).map(String::from).or(inherited);
        let mut out = vec![];
        for item in el.children().filter(|n| n.is_element()) {
            let header = is(&item, TEXT, "list-header");
            if !is(&item, TEXT, "list-item") && !header {
                continue;
            }
            let mut first = true;
            for child in item.children().filter(|n| n.is_element()) {
                if is(&child, TEXT, "list") {
                    out.extend(self.list(child, style.clone(), level + 1));
                    continue;
                }
                let kind = style.as_ref().and_then(|s| self.lists.get(s)).and_then(|l| l.get(&level)).cloned().unwrap_or_else(|| "bullet".into());
                let this = if first && !header { Some((if kind == "none" { "bullet".into() } else { kind }, level, style.clone())) } else { None };
                for mut b in self.blocks_one(child, this) {
                    if let Block::Paragraph(p) = &mut b {
                        if first {
                            first = false;
                        } else {
                            p.indent = level + 1;
                        }
                    }
                    out.push(b);
                }
            }
        }
        out
    }

    fn blocks_one(&self, el: Node, lst: Option<(String, u32, Option<String>)>) -> Vec<Block> {
        if is(&el, TEXT, "p") || is(&el, TEXT, "h") {
            self.paragraph(el, lst)
        } else if is(&el, TABLE, "table") {
            vec![Block::Table(self.table(el))]
        } else {
            self.blocks(el, lst)
        }
    }

    fn paragraph(&self, p: Node, lst: Option<(String, u32, Option<String>)>) -> Vec<Block> {
        let sname = p.attribute((TEXT, "style-name"));
        let mut kind = self.para_kind(sname);
        if is(&p, TEXT, "h") && kind != "title" && kind != "subtitle" {
            let lvl: u32 = p.attribute((TEXT, "outline-level")).and_then(|v| v.parse().ok()).unwrap_or(1);
            kind = format!("h{}", lvl.clamp(1, 6));
        }
        let mut align = None;
        let mut margin = None;
        let mut brk = None;
        let mut border = None;
        for (_, st) in self.chain("paragraph", sname) {
            align = align.or(st.align.clone());
            margin = margin.or(st.margin);
            brk = brk.or(st.brk.clone());
            border = border.or(st.border.clone());
        }
        let align = align.and_then(|a| match a.as_str() { "center" => Some("center"), "end" | "right" => Some("right"), "justify" => Some("justify"), _ => None }).map(String::from);
        let mut base = if kind == "normal" { self.text_props("paragraph", sname) } else { TextProps::default() };
        base.size = None;
        base.font = None;
        base.sup = None;
        base.sub = None;
        base.hl = None;
        let content = merge_runs(self.inline(p, &base, None));
        let mut out = vec![];
        if brk.as_deref() == Some("page") {
            out.push(Block::PageBreak(PageBreak {}));
        }
        let only: Vec<&Inline> = content.iter().filter(|x| !matches!(x, Inline::Run(r) if r.text.trim().is_empty())).collect();
        if only.is_empty() && border.as_deref().is_some_and(|b| b != "none") && kind == "normal" {
            out.push(Block::Rule(Rule {}));
            return out;
        }
        if only.len() == 1 && lst.is_none() {
            if let Inline::Run(r) = only[0] {
                if r.math {
                    out.push(Block::MathBlock(MathBlock { tex: r.text.clone() }));
                    return out;
                }
            }
        }
        let indent = if lst.is_none() && kind == "normal" { margin.filter(|m| *m > 20.0).map(|m| ((m / 36.0) as u32).min(6)).unwrap_or(0) } else { 0 };
        let content = if kind == "code" { content } else { strip_content(content) };
        out.push(Block::Paragraph(Paragraph { content, style: kind, align, list: lst.map(|l| (l.0, l.1.min(8))), indent }));
        out
    }

    fn run(&self, text: &str, f: &TextProps, link: &Option<String>) -> Inline {
        let text = Regex::new(r"[\n\r]+").unwrap().replace_all(text, " ");
        let text = Regex::new(r"  +").unwrap().replace_all(&text, " ").into_owned();
        Inline::Run(Run {
            text,
            b: f.b.unwrap_or(false),
            i: f.i.unwrap_or(false),
            u: f.u.unwrap_or(false) && link.is_none(),
            s: f.s.unwrap_or(false),
            sup: f.sup.unwrap_or(false),
            sub: f.sub.unwrap_or(false),
            size: f.size.filter(|s| Some(*s) != Some(self.doc.size)),
            font: f.font.clone().filter(|x| Some(x) != self.doc.font.as_ref()),
            color: if link.is_some() { None } else { f.color.clone() },
            hl: f.hl.clone(),
            link: link.clone(),
            ..Default::default()
        })
    }

    fn inline(&self, parent: Node, f: &TextProps, link: Option<String>) -> Vec<Inline> {
        let mut out = vec![];
        for node in parent.children() {
            if node.is_text() {
                out.push(self.run(node.text().unwrap_or(""), f, &link));
                continue;
            }
            if !node.is_element() {
                continue;
            }
            let ns = node.tag_name().namespace();
            let name = node.tag_name().name();
            if ns == Some(TEXT) {
                match name {
                    "span" => {
                        let mut f2 = self.text_props("text", node.attribute((TEXT, "style-name")));
                        f2.under(f);
                        out.extend(self.inline(node, &f2, link.clone()));
                    }
                    "a" => out.extend(self.inline(node, f, node.attribute((XLINK, "href")).map(String::from))),
                    "s" => {
                        let n: usize = node.attribute((TEXT, "c")).and_then(|v| v.parse().ok()).unwrap_or(1);
                        out.push(self.run(&" ".repeat(n), f, &link));
                    }
                    "tab" => out.push(self.run("\t", f, &link)),
                    "line-break" => out.push(Inline::Break(Break::default())),
                    "note" => {
                        let mut runs: Vec<Inline> = vec![];
                        if let Some(body) = kid(node, TEXT, "note-body") {
                            for p in body.descendants().filter(|n| is(n, TEXT, "p")) {
                                if !runs.is_empty() {
                                    runs.push(Inline::Run(Run::new(" ")));
                                }
                                runs.extend(self.inline(p, &TextProps::default(), None).into_iter().filter(|x| matches!(x, Inline::Run(_))));
                            }
                        }
                        out.push(Inline::Note(Note { runs: strip_content(merge_runs(runs)) }));
                    }
                    "sequence" | "page-number" | "date" | "time" | "chapter" | "bookmark-ref" | "reference-ref" | "sequence-ref" | "title"
                    | "author-name" | "file-name" | "page-count" | "meta" | "ruby" | "ruby-base" | "hidden-text" => out.extend(self.inline(node, f, link.clone())),
                    _ => {}
                }
            } else if ns == Some(DRAW) {
                match name {
                    "frame" => out.push(self.frame(node)),
                    "a" => {
                        for fr in node.descendants().filter(|n| is(n, DRAW, "frame")) {
                            out.push(self.frame(fr));
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    fn frame(&self, fr: Node) -> Inline {
        let w = fr.attribute((SVG, "width")).and_then(length_pt).unwrap_or(200.0);
        let h = fr.attribute((SVG, "height")).and_then(length_pt).unwrap_or(150.0);
        let desc = kid(fr, SVG, "desc").and_then(|n| n.text()).unwrap_or("").to_string();
        let title = kid(fr, SVG, "title").and_then(|n| n.text()).unwrap_or("").to_string();
        if let Some(json) = desc.strip_prefix(CHART_MARK) {
            if let Ok(mut c) = serde_json::from_str::<Chart>(json) {
                c.width = w;
                c.height = h;
                return Inline::Chart(c);
            }
        }
        if let Some(obj) = kid(fr, DRAW, "object") {
            // a formula: its MathML, inside the frame or in its own folder of the package
            let tex = if let Some(m) = kid(obj, MATH, "math") {
                mathtex::mathml_to_tex(m)
            } else {
                let href = obj.attribute((XLINK, "href")).unwrap_or("").trim_start_matches("./").trim_end_matches('/');
                self.files.get(&format!("{href}/content.xml")).map(|x| crate::xml::decode(x)).filter(|x| x.contains("MathML")).map(|x| mathtex::mathml_xml_to_tex(&x)).unwrap_or_default()
            };
            if !tex.is_empty() {
                return Inline::Run(Run { text: tex, math: true, ..Default::default() });
            }
        }
        for img in fr.descendants().filter(|n| is(n, DRAW, "image")) {
            let href = img.attribute((XLINK, "href")).unwrap_or("");
            let mut data = None;
            if !href.is_empty() && !href.starts_with("http:") && !href.starts_with("https:") {
                data = self.files.get(href.trim_start_matches("./")).cloned();
            }
            if data.is_none() {
                if let Some(b) = kid(img, OFFICE, "binary-data").and_then(|n| n.text()) {
                    let clean: String = b.chars().filter(|c| !c.is_whitespace()).collect();
                    data = base64::engine::general_purpose::STANDARD.decode(clean).ok();
                }
            }
            if let Some(data) = data {
                let ext = match imaging::sniff(&data) { "" => imaging::ext_for(href), e => e.to_string() };
                let alt = if title.is_empty() { desc.clone() } else { title.clone() };
                let name = href.rsplit('/').next().unwrap_or("image").to_string();
                return Inline::Image(Image { data, ext, width: w, height: h, alt, name });
            }
        }
        if let Some(tb) = kid(fr, DRAW, "text-box") {
            let text: Vec<String> = tb.descendants().filter(|n| is(n, TEXT, "p")).map(|p| p.descendants().filter_map(|n| n.text()).collect()).collect();
            return Inline::Opaque(Opaque { label: "text box".into(), text: text.join(" "), fmt: "odt".into(), ..Default::default() });
        }
        if kid(fr, DRAW, "object").is_some() {
            return Inline::Opaque(Opaque { label: "object".into(), fmt: "odt".into(), ..Default::default() });
        }
        Inline::Run(Run::default())
    }

    fn table(&self, el: Node) -> Table {
        let mut rows: Vec<Node> = vec![];
        let mut header_rows = 0;
        fn collect<'a, 'i>(parent: Node<'a, 'i>, header: bool, rows: &mut Vec<Node<'a, 'i>>, hr: &mut usize) {
            for c in parent.children().filter(|n| n.is_element()) {
                match c.tag_name().name() {
                    "table-header-rows" => collect(c, true, rows, hr),
                    "table-rows" | "table-row-group" => collect(c, header, rows, hr),
                    "table-row" => {
                        let rep: usize = c.attribute((TABLE, "number-rows-repeated")).and_then(|v| v.parse().ok()).unwrap_or(1).min(50);
                        for _ in 0..rep {
                            rows.push(c);
                            if header {
                                *hr += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        collect(el, false, &mut rows, &mut header_rows);
        let mut t = Table { rows: vec![], header: false, widths: None };
        for tr in rows {
            let mut row = vec![];
            for tc in tr.children().filter(|n| n.is_element()) {
                let name = tc.tag_name().name();
                if name == "covered-table-cell" && tc.children().all(|c| !c.is_element()) {
                    continue;
                }
                if name != "table-cell" && name != "covered-table-cell" {
                    continue;
                }
                let rep: usize = tc.attribute((TABLE, "number-columns-repeated")).and_then(|v| v.parse().ok()).unwrap_or(1).min(50);
                for _ in 0..rep {
                    let mut paras: Vec<Paragraph> = self.blocks(tc, None).into_iter().filter_map(|b| if let Block::Paragraph(mut p) = b {
                        if p.style == "code" {
                            p.style = "normal".into();
                        }
                        Some(p)
                    } else { None }).collect();
                    if paras.is_empty() {
                        paras.push(Paragraph::default());
                    }
                    row.push(paras);
                    let span: usize = tc.attribute((TABLE, "number-columns-spanned")).and_then(|v| v.parse().ok()).unwrap_or(1);
                    for _ in 1..span {
                        row.push(vec![Paragraph::default()]);
                    }
                }
            }
            t.rows.push(row);
        }
        t.header = header_rows > 0 || t.rows.first().is_some_and(|first| first.iter().all(|cell| {
            cell.iter().any(|p| !p.text().trim().is_empty()) && cell.iter().all(|p| p.content.iter().all(|x| matches!(x, Inline::Run(r) if r.b || r.text.trim().is_empty())))
        }));
        t
    }
}

fn group_code(blocks: Vec<Block>) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    let mut open = false;
    for b in blocks {
        if let Block::Paragraph(p) = &b {
            if p.style == "code" {
                let text: String = p.content.iter().map(|x| match x { Inline::Run(r) => r.text.clone(), Inline::Break(_) => "\n".into(), _ => String::new() }).collect();
                if open {
                    if let Some(Block::CodeBlock(c)) = out.last_mut() {
                        c.text.push('\n');
                        c.text.push_str(&text);
                        continue;
                    }
                }
                out.push(Block::CodeBlock(CodeBlock { text, lang: String::new() }));
                open = true;
                continue;
            }
        }
        open = false;
        out.push(b);
    }
    out
}

/// A Document from .odt bytes (or .fodt bytes with flat = true).
pub fn read(data: &[u8], flat: bool) -> Result<Document, String> {
    let (files, pkg_data) = if flat || !data.starts_with(b"PK") {
        let mut m = BTreeMap::new();
        m.insert("content.xml".to_string(), data.to_vec());
        (m, None)
    } else {
        let p = Package::open(data)?;
        (p.parts, Some(data.to_vec()))
    };
    let mut r = Reader { files: &files, styles: HashMap::new(), lists: HashMap::new(), doc: Document::default() };
    let styles_text = files.get("styles.xml").map(|x| crate::xml::decode(x));
    let content_text = crate::xml::decode(files.get("content.xml").ok_or("content.xml is missing")?);
    let styles_doc = styles_text.as_ref().and_then(|t| roxmltree::Document::parse(t).ok());
    let content = roxmltree::Document::parse(&content_text).map_err(|e| format!("broken document: {e}"))?;
    for root in styles_doc.iter().map(|d| d.root_element()).chain(std::iter::once(content.root_element())) {
        for c in ["font-face-decls", "styles", "automatic-styles"] {
            if let Some(el) = kid(root, OFFICE, c) {
                r.read_styles(el);
            }
        }
    }
    // body text size and font
    let defaults_root = styles_doc.as_ref().map(|d| d.root_element()).unwrap_or(content.root_element());
    for d in defaults_root.descendants().filter(|n| is(n, STYLE, "default-style")) {
        if d.attribute((STYLE, "family")) == Some("paragraph") {
            if let Some(tp) = kid(d, STYLE, "text-properties") {
                r.doc.size = tp.attribute((FO, "font-size")).and_then(length_pt).unwrap_or(11.0);
                r.doc.font = tp.attribute((STYLE, "font-name")).map(String::from);
            }
        }
    }
    if let Some(std) = r.styles.get(&("paragraph".into(), "Standard".into())) {
        if let Some(s) = std.text.size {
            r.doc.size = s;
        }
        if let Some(f) = &std.text.font {
            r.doc.font = Some(f.clone());
        }
    }
    // page size
    let layouts: HashMap<String, Node> = defaults_root.descendants().filter(|n| is(n, STYLE, "page-layout")).map(|n| (n.attribute((STYLE, "name")).unwrap_or("").to_string(), n)).collect();
    let master = defaults_root.descendants().find(|n| is(n, STYLE, "master-page"));
    let pl = master.and_then(|m| m.attribute((STYLE, "page-layout-name"))).and_then(|n| layouts.get(n)).copied().or_else(|| layouts.values().next().copied());
    if let Some(props) = pl.and_then(|p| kid(p, STYLE, "page-layout-properties")) {
        let mut page = PageSetup { numbers: false, ..Default::default() };
        if let (Some(w), Some(h)) = (props.attribute((FO, "page-width")).and_then(length_pt), props.attribute((FO, "page-height")).and_then(length_pt)) {
            page.width = w;
            page.height = h;
        }
        let m = |k: &str| props.attribute((FO, k)).and_then(length_pt).unwrap_or(56.7);
        page.margins = (m("margin-top"), m("margin-right"), m("margin-bottom"), m("margin-left"));
        r.doc.page = page;
    }
    let body = kid(content.root_element(), OFFICE, "body").and_then(|b| kid(b, OFFICE, "text"));
    if let Some(b) = body {
        r.doc.blocks = group_code(r.blocks(b, None));
    }
    let meta_text = files.get("meta.xml").map(|x| crate::xml::decode(x));
    let meta_doc = meta_text.as_ref().and_then(|t| roxmltree::Document::parse(t).ok());
    let meta_root = meta_doc.as_ref().map(|d| d.root_element()).unwrap_or(content.root_element());
    r.doc.title = find(meta_root, "http://purl.org/dc/elements/1.1/", "title").and_then(|n| n.text()).unwrap_or("").trim().to_string();
    r.doc.author = find(meta_root, "http://purl.org/dc/elements/1.1/", "creator").and_then(|n| n.text()).unwrap_or("").trim().to_string();
    if r.doc.title.is_empty() {
        if let Some(Block::Paragraph(p)) = r.doc.blocks.iter().find(|b| matches!(b, Block::Paragraph(p) if p.style == "title" || p.style == "h1")) {
            r.doc.title = p.text().trim().chars().take(200).collect();
        }
    }
    let mut doc = r.doc;
    if let Some(d) = pkg_data {
        doc.source.insert("odt".into(), serde_bytes::ByteBuf::from(d));
    }
    Ok(doc)
}

// -- writing ------------------------------------------------------------------------

fn text_markup(s: &str) -> String {
    let mut out = String::new();
    for (k, piece) in s.split('\t').enumerate() {
        if k > 0 {
            out.push_str("<text:tab/>");
        }
        let e = esc(piece);
        // runs of spaces (and a leading one) as text:s
        let mut prev_space = true;
        let mut count = 0;
        for c in e.chars() {
            if c == ' ' {
                if prev_space {
                    count += 1;
                } else {
                    out.push(' ');
                    prev_space = true;
                }
                continue;
            }
            if count > 0 {
                out.push_str(&format!(r#"<text:s text:c="{count}"/>"#));
                count = 0;
            }
            prev_space = false;
            out.push(c);
        }
        if count > 0 {
            out.push_str(&format!(r#"<text:s text:c="{count}"/>"#));
        }
    }
    out
}

type TKey = (bool, bool, bool, bool, bool, bool, bool, Option<String>, Option<String>, Option<String>, Option<String>);

struct Writer<'d> {
    doc: &'d Document,
    flat: bool,
    text_styles: Vec<(TKey, String)>,
    para_styles: Vec<((String, Option<String>, u32, bool), String)>,
    list_styles: Vec<(Vec<String>, String)>,
    files: Vec<(String, Vec<u8>)>,
    objects: Vec<String>,
    notes: usize,
    frames: usize,
    formulas: usize,
    tables: usize,
}

impl<'d> Writer<'d> {
    fn tstyle(&mut self, r: &Run) -> Option<String> {
        let key: TKey = (r.b, r.i, r.u, r.s, r.sup, r.sub, r.code, r.size.map(|s| format!("{s}")), r.font.clone(), r.color.clone(), r.hl.clone());
        if !(key.0 || key.1 || key.2 || key.3 || key.4 || key.5 || key.6 || key.7.is_some() || key.8.is_some() || key.9.is_some() || key.10.is_some()) {
            return None;
        }
        if let Some((_, n)) = self.text_styles.iter().find(|(k, _)| *k == key) {
            return Some(n.clone());
        }
        let n = format!("T{}", self.text_styles.len() + 1);
        self.text_styles.push((key, n.clone()));
        Some(n)
    }

    fn pstyle(&mut self, base: &str, align: Option<&str>, indent: u32, page_break: bool) -> String {
        if align.is_none() && indent == 0 && !page_break {
            return base.to_string();
        }
        let key = (base.to_string(), align.map(String::from), indent, page_break);
        if let Some((_, n)) = self.para_styles.iter().find(|(k, _)| *k == key) {
            return n.clone();
        }
        let n = format!("P{}", self.para_styles.len() + 1);
        self.para_styles.push((key, n.clone()));
        n
    }

    fn formula(&mut self, tex: &str, display: bool) -> String {
        self.formulas += 1;
        let n = self.formulas;
        let mml = mathtex::to_mathml(tex, display);
        let size = if self.doc.size > 0.0 { self.doc.size } else { 11.0 };
        let flat = mathtex::to_unicode(tex);
        let tall = ["\\frac", "\\sum", "\\int", "\\prod", "\\begin", "\\sqrt"].iter().any(|s| tex.contains(s));
        let w = (flat.chars().count() as f64 * size * 0.52).max(size);
        let h = size * if tall { 2.6 } else { 1.5 };
        let inner = if self.flat {
            format!("<draw:object>{mml}</draw:object>")
        } else {
            self.files.push((format!("Object {n}/content.xml"), format!(r#"<?xml version="1.0" encoding="UTF-8"?>{mml}"#).into_bytes()));
            self.objects.push(format!("Object {n}/"));
            format!(r#"<draw:object xlink:href="./Object {n}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#)
        };
        format!(r#"<draw:frame draw:style-name="fr2" draw:name="Formula{n}" text:anchor-type="as-char" svg:width="{w:.2}pt" svg:height="{h:.2}pt" draw:z-index="{}">{inner}<svg:desc>{}</svg:desc></draw:frame>"#, 1000 + n, esc(tex))
    }

    fn frame(&mut self, data: &[u8], ext: &str, w: f64, h: f64, alt: &str, desc: &str) -> String {
        self.frames += 1;
        let n = self.frames;
        let ext = if ext == "jpeg" { "jpg" } else { ext };
        let name = format!("Pictures/image{n}.{ext}");
        let inner = if self.flat {
            format!("<draw:image><office:binary-data>{}</office:binary-data></draw:image>", base64::engine::general_purpose::STANDARD.encode(data))
        } else {
            self.files.push((name.clone(), data.to_vec()));
            format!(r#"<draw:image xlink:href="{name}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#)
        };
        let title = if alt.is_empty() { String::new() } else { format!("<svg:title>{}</svg:title>", esc(alt)) };
        let d = if desc.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(desc)) };
        format!(r#"<draw:frame draw:style-name="fr1" draw:name="Image{n}" text:anchor-type="as-char" svg:width="{w:.2}pt" svg:height="{h:.2}pt" draw:z-index="{n}">{inner}{title}{d}</draw:frame>"#)
    }

    fn inline(&mut self, content: &[Inline]) -> String {
        let mut out = String::new();
        for x in merge_runs(content.to_vec()) {
            match &x {
                Inline::Run(r) => {
                    let mut body = if r.math {
                        self.formula(&r.text, false)
                    } else {
                        let b = text_markup(&r.text);
                        match self.tstyle(r) {
                            Some(st) => format!(r#"<text:span text:style-name="{st}">{b}</text:span>"#),
                            None => b,
                        }
                    };
                    if let Some(l) = &r.link {
                        body = format!(r#"<text:a xlink:type="simple" xlink:href="{}">{body}</text:a>"#, esc_attr(l));
                    }
                    out.push_str(&body);
                }
                Inline::Break(_) => out.push_str("<text:line-break/>"),
                Inline::Image(img) => {
                    let mut data = img.data.clone();
                    let mut ext = if img.ext.is_empty() { imaging::sniff(&img.data).to_string() } else { img.ext.clone() };
                    if !matches!(ext.as_str(), "png" | "jpeg" | "gif" | "svg" | "bmp" | "tiff" | "emf" | "wmf") {
                        match imaging::to_png(&data) {
                            Ok(p) => {
                                data = p;
                                ext = "png".into();
                            }
                            Err(_) => {
                                out.push_str(&text_markup(&format!("[{}]", if img.alt.is_empty() { "picture" } else { &img.alt })));
                                continue;
                            }
                        }
                    }
                    let f = self.frame(&data, &ext, if img.width > 0.0 { img.width } else { 200.0 }, if img.height > 0.0 { img.height } else { 150.0 }, &img.alt, "");
                    out.push_str(&f);
                }
                Inline::Chart(c) => {
                    let desc = format!("{CHART_MARK}{}", serde_json::to_string(c).unwrap_or_default());
                    let f = self.frame(charts::svg(c).as_bytes(), "svg", c.width, c.height, &c.title, &desc);
                    out.push_str(&f);
                }
                Inline::Note(n) => {
                    self.notes += 1;
                    let k = self.notes;
                    let inner = self.inline(&n.runs);
                    out.push_str(&format!(r#"<text:note text:id="ftn{k}" text:note-class="footnote"><text:note-citation>{k}</text:note-citation><text:note-body><text:p text:style-name="Footnote">{inner}</text:p></text:note-body></text:note>"#));
                }
                Inline::Opaque(o) => out.push_str(&text_markup(&o.text)),
            }
        }
        out
    }

    fn list_xml(&mut self, items: &[&Paragraph]) -> String {
        let mut kinds = vec!["bullet".to_string(); 10];
        let mut seen = vec![false; 10];
        for p in items {
            let (kind, level) = p.list.clone().unwrap();
            let l = (level as usize).min(9);
            if !seen[l] {
                kinds[l] = kind;
                seen[l] = true;
            }
        }
        let name = match self.list_styles.iter().find(|(k, _)| *k == kinds) {
            Some((_, n)) => n.clone(),
            None => {
                let n = format!("L{}", self.list_styles.len() + 1);
                self.list_styles.push((kinds, n.clone()));
                n
            }
        };
        let mut out = String::new();
        let mut stack: Vec<bool> = vec![]; // one entry per open text:list: is a list item open in it?
        for p in items {
            let level = (p.list.as_ref().unwrap().1 as usize).min(9);
            if stack.is_empty() {
                out.push_str(&format!(r#"<text:list text:style-name="{name}">"#));
                stack.push(false);
            }
            while stack.len() - 1 > level {
                if stack.pop() == Some(true) {
                    out.push_str("</text:list-item>");
                }
                out.push_str("</text:list>");
            }
            while stack.len() - 1 < level {
                if !*stack.last().unwrap() {
                    out.push_str("<text:list-item>"); // an empty item holds the deeper list
                    *stack.last_mut().unwrap() = true;
                }
                out.push_str("<text:list>");
                stack.push(false);
            }
            if *stack.last().unwrap() {
                out.push_str("</text:list-item>");
            }
            out.push_str("<text:list-item>");
            *stack.last_mut().unwrap() = true;
            let inner = self.inline(&p.content);
            out.push_str(&format!(r#"<text:p text:style-name="List_20_Paragraph">{inner}</text:p>"#));
        }
        while let Some(open) = stack.pop() {
            if open {
                out.push_str("</text:list-item>");
            }
            out.push_str("</text:list>");
        }
        out
    }

    fn table_xml(&mut self, t: &Table) -> String {
        let n = t.ncols();
        if n == 0 {
            return String::new();
        }
        self.tables += 1;
        let cols: String = (0..n).map(|_| r#"<table:table-column table:style-name="TableCol"/>"#).collect();
        let mut rows = String::new();
        for (r, row) in t.rows.iter().enumerate() {
            let header = t.header && r == 0;
            let mut cells = String::new();
            for c in 0..n {
                let paras = row.get(c).cloned().unwrap_or_else(|| vec![Paragraph::default()]);
                let mut inner = String::new();
                for p in &paras {
                    let st = self.pstyle(if header { "Table_20_Heading" } else { "Table_20_Contents" }, p.align.as_deref(), 0, false);
                    let body = self.inline(&p.content);
                    inner.push_str(&format!(r#"<text:p text:style-name="{st}">{body}</text:p>"#));
                }
                if inner.is_empty() {
                    inner = "<text:p/>".into();
                }
                cells.push_str(&format!(r#"<table:table-cell table:style-name="{}" office:value-type="string">{inner}</table:table-cell>"#, if header { "TableHead" } else { "TableCell" }));
            }
            let x = format!("<table:table-row>{cells}</table:table-row>");
            if header {
                rows.push_str(&format!("<table:table-header-rows>{x}</table:table-header-rows>"));
            } else {
                rows.push_str(&x);
            }
        }
        format!(r#"<table:table table:name="Table{}" table:style-name="TableWide">{cols}{rows}</table:table>"#, self.tables)
    }

    fn body(&mut self) -> String {
        let mut out = String::new();
        let blocks = &self.doc.blocks;
        let mut k = 0;
        let mut page_break = false;
        while k < blocks.len() {
            let b = &blocks[k];
            k += 1;
            if let Block::PageBreak(_) = b {
                page_break = true;
                continue;
            }
            if let Block::Paragraph(p) = b {
                if p.list.is_some() {
                    let mut items = vec![p];
                    let mut top = if p.list.as_ref().unwrap().1 == 0 { Some(p.list.as_ref().unwrap().0.clone()) } else { None };
                    while let Some(Block::Paragraph(q)) = blocks.get(k) {
                        let Some((kind, level)) = &q.list else { break };
                        if *level == 0 && top.as_ref().is_some_and(|t| t != kind) {
                            break; // a list of the other kind follows
                        }
                        if *level == 0 {
                            top = Some(kind.clone());
                        }
                        items.push(q);
                        k += 1;
                    }
                    let x = self.list_xml(&items);
                    out.push_str(&x);
                    continue;
                }
                let base = match p.style.as_str() {
                    "title" => "Title".to_string(),
                    "subtitle" => "Subtitle".into(),
                    "quote" => "Quotations".into(),
                    "caption" => "Caption".into(),
                    s if s.len() == 2 && s.starts_with('h') => format!("Heading_20_{}", &s[1..]),
                    _ => "Text_20_body".into(),
                };
                let name = self.pstyle(&base, p.align.as_deref(), p.indent, page_break);
                page_break = false;
                let content = self.inline(&p.content);
                if p.style.len() == 2 && p.style.starts_with('h') {
                    out.push_str(&format!(r#"<text:h text:style-name="{name}" text:outline-level="{}">{content}</text:h>"#, &p.style[1..]));
                } else {
                    out.push_str(&format!(r#"<text:p text:style-name="{name}">{content}</text:p>"#));
                }
                continue;
            }
            match b {
                Block::Table(t) => {
                    let x = self.table_xml(t);
                    out.push_str(&x);
                }
                Block::CodeBlock(c) => {
                    for (j, line) in c.text.split('\n').enumerate() {
                        let name = self.pstyle("Preformatted_20_Text", None, 0, page_break && j == 0);
                        out.push_str(&format!(r#"<text:p text:style-name="{name}">{}</text:p>"#, text_markup(line)));
                    }
                    page_break = false;
                }
                Block::MathBlock(m) => {
                    let name = self.pstyle("Text_20_body", Some("center"), 0, page_break);
                    let f = self.formula(&m.tex, true);
                    out.push_str(&format!(r#"<text:p text:style-name="{name}">{f}</text:p>"#));
                    page_break = false;
                }
                Block::Rule(_) => {
                    let name = self.pstyle("Horizontal_20_Line", None, 0, page_break);
                    out.push_str(&format!(r#"<text:p text:style-name="{name}"/>"#));
                    page_break = false;
                }
                _ => {}
            }
        }
        out
    }

    fn auto_styles(&self) -> String {
        let mut out = String::new();
        for (key, name) in &self.text_styles {
            let (b, i, u, s, sup, sub, code, size, font, color, hl) = key;
            let mut p = vec![];
            if *b {
                p.push(r#"fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold""#.to_string());
            }
            if *i {
                p.push(r#"fo:font-style="italic" style:font-style-asian="italic" style:font-style-complex="italic""#.into());
            }
            if *u {
                p.push(r#"style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color""#.into());
            }
            if *s {
                p.push(r#"style:text-line-through-style="solid""#.into());
            }
            if *sup {
                p.push(r#"style:text-position="super 58%""#.into());
            } else if *sub {
                p.push(r#"style:text-position="sub 58%""#.into());
            }
            if *code {
                p.push(r#"style:font-name="Consolas" fo:font-family="Consolas""#.into());
            }
            if let Some(f) = font {
                p.push(format!(r#"style:font-name="{0}" fo:font-family="{0}""#, esc_attr(f)));
            }
            if let Some(sz) = size {
                p.push(format!(r#"fo:font-size="{sz}pt""#));
            }
            if let Some(c) = color {
                p.push(format!(r#"fo:color="{c}""#));
            }
            if let Some(h) = hl {
                p.push(format!(r#"fo:background-color="{h}""#));
            }
            out.push_str(&format!(r#"<style:style style:name="{name}" style:family="text"><style:text-properties {}/></style:style>"#, p.join(" ")));
        }
        for ((base, align, indent, pb), name) in &self.para_styles {
            let mut p = vec![];
            if let Some(a) = align {
                p.push(format!(r#"fo:text-align="{}""#, match a.as_str() { "right" => "end", "center" => "center", "justify" => "justify", _ => "start" }));
            }
            if *indent > 0 {
                p.push(format!(r#"fo:margin-left="{:.2}in""#, *indent as f64 * 0.5));
            }
            if *pb {
                p.push(r#"fo:break-before="page""#.into());
            }
            out.push_str(&format!(r#"<style:style style:name="{name}" style:family="paragraph" style:parent-style-name="{base}"><style:paragraph-properties {}/></style:style>"#, p.join(" ")));
        }
        out.push_str(r#"<style:style style:name="fr2" style:family="graphic" style:parent-style-name="Graphics"><style:graphic-properties style:vertical-pos="middle" style:vertical-rel="text" fo:background-color="transparent" style:wrap="none"/></style:style>"#);
        out.push_str(r#"<style:style style:name="fr1" style:family="graphic" style:parent-style-name="Graphics"><style:graphic-properties style:vertical-pos="top" style:vertical-rel="baseline" fo:background-color="transparent" style:wrap="none"/></style:style>"#);
        out.push_str(r#"<style:style style:name="TableWide" style:family="table"><style:table-properties style:width="100%" table:align="margins" style:rel-width="100%"/></style:style><style:style style:name="TableCol" style:family="table-column"/>"#);
        let border = r##"fo:border="0.5pt solid #BFBFBF" fo:padding="0.05in""##;
        out.push_str(&format!(r##"<style:style style:name="TableCell" style:family="table-cell"><style:table-cell-properties {border}/></style:style><style:style style:name="TableHead" style:family="table-cell"><style:table-cell-properties {border} fo:background-color="#F0F0F0"/></style:style>"##));
        for (kinds, name) in &self.list_styles {
            let mut levels = String::new();
            for (lv, kind) in kinds.iter().enumerate() {
                let indent = format!(r#"<style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="{0:.2}in" fo:text-indent="-0.25in" fo:margin-left="{0:.2}in"/></style:list-level-properties>"#, 0.5 * (lv as f64 + 1.0));
                if kind == "bullet" {
                    levels.push_str(&format!(r#"<text:list-level-style-bullet text:level="{}" text:bullet-char="{}">{indent}</text:list-level-style-bullet>"#, lv + 1, BULLETS[lv % 3]));
                } else {
                    levels.push_str(&format!(r#"<text:list-level-style-number text:level="{}" style:num-suffix="." style:num-format="{}">{indent}</text:list-level-style-number>"#, lv + 1, ["1", "a", "i"][lv % 3]));
                }
            }
            out.push_str(&format!(r#"<text:list-style style:name="{name}">{levels}</text:list-style>"#));
        }
        out
    }

    fn named_styles(&self) -> String {
        let size = if self.doc.size > 0.0 { self.doc.size } else { 11.0 };
        let font = esc_attr(self.doc.font.as_deref().unwrap_or("Liberation Serif"));
        let mut out = format!(r#"<style:default-style style:family="paragraph"><style:paragraph-properties fo:orphans="2" fo:widows="2"/><style:text-properties style:font-name="{font}" fo:font-family="{font}" fo:font-size="{size}pt" fo:language="en" fo:country="US"/></style:default-style><style:style style:name="Standard" style:family="paragraph" style:class="text"/><style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"><style:paragraph-properties fo:margin-top="0pt" fo:margin-bottom="6pt" fo:line-height="115%"/></style:style><style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="text"><style:paragraph-properties fo:keep-with-next="always"/></style:style>"#);
        for k in 1..=6 {
            let lk = look(&format!("h{k}"));
            out.push_str(&format!(r#"<style:style style:name="Heading_20_{k}" style:display-name="Heading {k}" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="{k}" style:class="text"><style:paragraph-properties fo:margin-top="{}pt" fo:margin-bottom="{}pt"/><style:text-properties fo:font-size="{:.1}pt" fo:color="{}"{}{}/></style:style>"#,
                lk.before, lk.after, size * lk.scale, lk.color.unwrap_or("#000000"), if lk.bold { r#" fo:font-weight="bold""# } else { "" }, if lk.italic { r#" fo:font-style="italic""# } else { "" }));
        }
        for (name, key, extra) in [("Title", "title", r#"fo:text-align="center""#), ("Subtitle", "subtitle", r#"fo:text-align="center""#),
                                   ("Quotations", "quote", r#"fo:margin-left="0.4in" fo:margin-right="0.4in""#), ("Caption", "caption", "")] {
            let lk = look(key);
            out.push_str(&format!(r#"<style:style style:name="{name}" style:display-name="{name}" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:margin-top="{}pt" fo:margin-bottom="{}pt" {extra}/><style:text-properties fo:font-size="{:.1}pt"{}{}/></style:style>"#,
                lk.before, lk.after, size * lk.scale, lk.color.map(|c| format!(r#" fo:color="{c}""#)).unwrap_or_default(), if lk.italic { r#" fo:font-style="italic""# } else { "" }));
        }
        out.push_str(&format!(r##"<style:style style:name="Preformatted_20_Text" style:display-name="Preformatted Text" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-top="0pt" fo:margin-bottom="0pt" fo:background-color="#F4F4F4"/><style:text-properties style:font-name="Consolas" fo:font-family="Consolas" fo:font-size="{:.1}pt"/></style:style><style:style style:name="List_20_Paragraph" style:display-name="List Paragraph" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:margin-bottom="2pt"/></style:style><style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"/><style:style style:name="Table_20_Heading" style:display-name="Table Heading" style:family="paragraph" style:parent-style-name="Table_20_Contents" style:class="extra"><style:text-properties fo:font-weight="bold"/></style:style><style:style style:name="Footnote" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:text-properties fo:font-size="{:.1}pt"/></style:style><style:style style:name="Horizontal_20_Line" style:display-name="Horizontal Line" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-top="0pt" fo:margin-bottom="8pt" fo:padding="0in" fo:border-left="none" fo:border-right="none" fo:border-top="none" fo:border-bottom="0.75pt solid #808080"/></style:style><style:style style:name="Graphics" style:family="graphic"/><style:style style:name="PageNum" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center"/></style:style>"##, size * 0.88, size * 0.82));
        out
    }

    fn build(mut self) -> Vec<u8> {
        let body = self.body();
        let decl: String = DECL.iter().map(|(p, u)| format!(r#" xmlns:{p}="{u}""#)).collect();
        let p = &self.doc.page;
        let m = p.margins;
        let layout = format!(r#"<style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="{:.2}pt" fo:page-height="{:.2}pt" style:print-orientation="{}" fo:margin-top="{:.2}pt" fo:margin-right="{:.2}pt" fo:margin-bottom="{:.2}pt" fo:margin-left="{:.2}pt"/>{}</style:page-layout>"#,
            p.width, p.height, if p.width > p.height { "landscape" } else { "portrait" }, m.0, m.1, m.2, m.3,
            if p.numbers { r#"<style:footer-style><style:header-footer-properties fo:min-height="0pt" fo:margin-top="10pt"/></style:footer-style>"# } else { "" });
        let footer = if p.numbers { r#"<style:footer><text:p text:style-name="PageNum"><text:page-number text:select-page="current"/></text:p></style:footer>"# } else { "" };
        let master = format!(r#"<style:master-page style:name="Standard" style:page-layout-name="pm1">{footer}</style:master-page>"#);
        let fonts = format!(r#"<office:font-face-decls><style:font-face style:name="Consolas" svg:font-family="Consolas" style:font-pitch="fixed"/>{}</office:font-face-decls>"#,
            self.doc.font.as_ref().map(|f| format!(r#"<style:font-face style:name="{0}" svg:font-family="{0}"/>"#, esc_attr(f))).unwrap_or_default());
        let auto = self.auto_styles();
        let named = self.named_styles();
        let meta = format!(r#"<office:meta><meta:generator>Rewrite</meta:generator><dc:title>{}</dc:title><dc:creator>{}</dc:creator></office:meta>"#, esc(&self.doc.title), esc(&self.doc.author));
        if self.flat {
            return format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document{decl} office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">{meta}{fonts}<office:styles>{named}</office:styles><office:automatic-styles>{auto}{layout}</office:automatic-styles><office:master-styles>{master}</office:master-styles><office:body><office:text>{body}</office:text></office:body></office:document>"#).into_bytes();
        }
        let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content{decl} office:version="1.3">{fonts}<office:automatic-styles>{auto}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#);
        let styles = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles{decl} office:version="1.3">{fonts}<office:styles>{named}</office:styles><office:automatic-styles>{layout}</office:automatic-styles><office:master-styles>{master}</office:master-styles></office:document-styles>"#);
        let meta_xml = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta{decl} office:version="1.3">{meta}</office:document-meta>"#);
        let mut entries = vec![r#"<manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/>"#.to_string()];
        for n in ["content.xml", "styles.xml", "meta.xml"] {
            entries.push(format!(r#"<manifest:file-entry manifest:full-path="{n}" manifest:media-type="text/xml"/>"#));
        }
        for f in &self.objects {
            entries.push(format!(r#"<manifest:file-entry manifest:full-path="{f}" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.formula"/>"#));
        }
        for (n, _) in &self.files {
            let kind = if n.ends_with(".xml") { "text/xml" } else { imaging::mime(&imaging::ext_for(n)) };
            entries.push(format!(r#"<manifest:file-entry manifest:full-path="{n}" manifest:media-type="{kind}"/>"#));
        }
        let manifest = format!(r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">{}</manifest:manifest>"#, entries.concat());
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("mimetype".into(), b"application/vnd.oasis.opendocument.text".to_vec()),
            ("content.xml".into(), content.into_bytes()),
            ("styles.xml".into(), styles.into_bytes()),
            ("meta.xml".into(), meta_xml.into_bytes()),
        ];
        files.extend(std::mem::take(&mut self.files));
        files.push(("META-INF/manifest.xml".into(), manifest.into_bytes()));
        opc::zip(&files, &["mimetype"])
    }
}

/// .odt bytes (or .fodt with flat = true) for a Document.
pub fn write(doc: &Document, flat: bool) -> Vec<u8> {
    Writer { doc, flat, text_styles: vec![], para_styles: vec![], list_styles: vec![], files: vec![], objects: vec![], notes: 0, frames: 0, formulas: 0, tables: 0 }.build()
}
