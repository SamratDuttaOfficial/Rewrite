//! Word documents (.docx, Office Open XML) <-> Document.
//!
//! Reading keeps what can be edited here: paragraphs with their styles (title, headings, quote, caption,
//! code), lists, alignment, character formatting, hyperlinks, tables (merged cells spread out), pictures,
//! Word charts (as editable charts), footnotes, equations (as TeX) and page setup. What cannot be edited
//! here (text boxes, shapes, SmartArt, tables of contents ...) becomes an Opaque piece that is written back
//! exactly as it was. Writing a document that came from a .docx reuses that file's styles, headers,
//! footers, theme and settings.

use crate::model::*;
use crate::opc::{self, Package, Rel};
use crate::xml::{attr, element_source, esc, esc_attr, find, is, kid, kids};
use crate::{charts, imaging, mathtex};
use regex::Regex;
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const V: &str = "urn:schemas-microsoft-com:vml";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const EMU: f64 = 12700.0;

pub const NAMESPACES: [(&str, &str); 19] = [
    ("w", W), ("r", R), ("wp", WP), ("a", A), ("pic", "http://schemas.openxmlformats.org/drawingml/2006/picture"),
    ("c", C), ("m", M), ("mc", MC), ("v", V), ("o", "urn:schemas-microsoft-com:office:office"),
    ("w10", "urn:schemas-microsoft-com:office:word"), ("w14", "http://schemas.microsoft.com/office/word/2010/wordml"),
    ("w15", "http://schemas.microsoft.com/office/word/2012/wordml"),
    ("wp14", "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing"),
    ("wps", "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"),
    ("wpg", "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"),
    ("wpc", "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas"),
    ("dgm", "http://schemas.openxmlformats.org/drawingml/2006/diagram"),
    ("asvg", "http://schemas.microsoft.com/office/drawing/2016/SVG/main"),
];

fn highlight(name: &str) -> Option<&'static str> {
    Some(match name {
        "yellow" => "#FFFF00", "green" => "#00FF00", "cyan" => "#00FFFF", "magenta" => "#FF00FF", "blue" => "#0000FF",
        "red" => "#FF0000", "darkBlue" => "#000080", "darkCyan" => "#008080", "darkGreen" => "#008000",
        "darkMagenta" => "#800080", "darkRed" => "#800000", "darkYellow" => "#808000", "darkGray" => "#808080",
        "lightGray" => "#C0C0C0", "black" => "#000000", "white" => "#FFFFFF",
        _ => return None,
    })
}

const CODE_STYLES: [&str; 10] = ["html preformatted", "source code", "sourcecode", "code", "macro text", "verbatim",
                                 "preformatted text", "plain text", "code block", "html code"];

fn on(el: Option<Node>) -> Option<bool> {
    let el = el?;
    Some(match el.attribute((W, "val")) {
        None => true,
        Some(v) => !matches!(v.to_lowercase().as_str(), "0" | "false" | "off" | "none"),
    })
}

fn wval<'a>(el: Node<'a, '_>, name: &str) -> Option<&'a str> {
    kid(el, W, name).and_then(|n| n.attribute((W, "val")))
}

#[derive(Default, Clone, Debug)]
struct Props {
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    s: Option<bool>,
    sup: Option<bool>,
    sub: Option<bool>,
    hidden: Option<bool>,
    caps: Option<bool>,
    size: Option<f64>,
    color: Option<String>,
    font: Option<String>,
    hl: Option<String>,
    rstyle: Option<String>,
}

impl Props {
    fn read(rpr: Option<Node>) -> Props {
        let mut p = Props::default();
        let Some(rpr) = rpr else { return p };
        p.b = on(kid(rpr, W, "b"));
        p.i = on(kid(rpr, W, "i"));
        p.s = on(kid(rpr, W, "strike")).or(on(kid(rpr, W, "dstrike")));
        p.hidden = on(kid(rpr, W, "vanish"));
        p.caps = on(kid(rpr, W, "caps"));
        if let Some(u) = kid(rpr, W, "u") {
            p.u = Some(u.attribute((W, "val")).unwrap_or("single") != "none");
        }
        if let Some(v) = wval(rpr, "vertAlign") {
            p.sup = Some(v == "superscript");
            p.sub = Some(v == "subscript");
        }
        if let Some(v) = wval(rpr, "sz").and_then(|v| v.parse::<f64>().ok()) {
            p.size = Some(v / 2.0);
        }
        if let Some(v) = wval(rpr, "color") {
            if v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                p.color = Some(format!("#{}", v.to_uppercase()));
            }
        }
        if let Some(f) = kid(rpr, W, "rFonts") {
            p.font = f.attribute((W, "ascii")).or(f.attribute((W, "hAnsi"))).map(String::from);
        }
        if let Some(h) = wval(rpr, "highlight").and_then(highlight) {
            p.hl = Some(h.into());
        }
        if let Some(shd) = kid(rpr, W, "shd") {
            if let Some(f) = shd.attribute((W, "fill")) {
                if f.len() == 6 && f.chars().all(|c| c.is_ascii_hexdigit()) && !f.eq_ignore_ascii_case("FFFFFF") && p.hl.is_none() {
                    p.hl = Some(format!("#{}", f.to_uppercase()));
                }
            }
        }
        p.rstyle = wval(rpr, "rStyle").map(String::from);
        p
    }

    /// Fill what is unset here from `o`.
    fn under(&mut self, o: &Props) {
        macro_rules! fill {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = o.$f.clone(); } )* };
        }
        fill!(b, i, u, s, sup, sub, hidden, caps, size, color, font, hl);
    }
}

#[derive(Clone, Debug)]
struct StyleInfo {
    name: String,
    based: Option<String>,
    outline: Option<u32>,
    props: Props,
    kind: String,
}

struct Field {
    instr: String,
    result: bool,
    link: Option<String>,
}

pub struct Reader<'p> {
    pkg: &'p Package,
    main: String,
    rels: HashMap<String, Rel>,
    styles: HashMap<String, StyleInfo>,
    default_para: Option<String>,
    numbering: HashMap<String, HashMap<u32, String>>,
    footnotes: HashMap<String, Vec<Inline>>,
    doc: Document,
}

fn theme_fonts(pkg: &Package, rels: &HashMap<String, Rel>) -> HashMap<&'static str, String> {
    let mut out = HashMap::new();
    let Some(rel) = rels.values().find(|r| r.typ.ends_with("/theme")) else { return out };
    let Some(text) = pkg.text(&rel.target) else { return out };
    let Ok(doc) = roxmltree::Document::parse(&text) else { return out };
    for (kind, tag) in [("major", "majorFont"), ("minor", "minorFont")] {
        if let Some(f) = find(doc.root_element(), A, tag).and_then(|n| kid(n, A, "latin")).and_then(|n| n.attribute("typeface")) {
            out.insert(kind, f.to_string());
        }
    }
    out
}

impl<'p> Reader<'p> {
    pub fn new(pkg: &'p Package) -> Reader<'p> {
        let main = pkg.rels("").values().find(|r| r.typ.ends_with("/officeDocument")).map(|r| r.target.clone()).unwrap_or_else(|| "word/document.xml".into());
        let rels = pkg.rels(&main);
        let mut r = Reader { pkg, main, rels, styles: HashMap::new(), default_para: None, numbering: HashMap::new(), footnotes: HashMap::new(), doc: Document::default() };
        r.read_styles();
        r.read_numbering();
        r.read_notes();
        r
    }

    fn part(&self, kind: &str) -> Option<String> {
        self.rels.values().find(|r| r.typ.ends_with(&format!("/{kind}"))).map(|r| r.target.clone())
    }

    fn read_styles(&mut self) {
        let themes = theme_fonts(self.pkg, &self.rels);
        let Some(text) = self.part("styles").and_then(|n| self.pkg.text(&n)) else {
            self.doc.font = themes.get("minor").cloned();
            return;
        };
        let Ok(doc) = roxmltree::Document::parse(&text) else { return };
        let root = doc.root_element();
        for st in kids(root, W, "style") {
            let sid = st.attribute((W, "styleId")).unwrap_or("").to_string();
            let info = StyleInfo {
                name: wval(st, "name").unwrap_or(&sid).to_lowercase(),
                based: wval(st, "basedOn").map(String::from),
                outline: kid(st, W, "pPr").and_then(|p| wval(p, "outlineLvl")).and_then(|v| v.parse().ok()),
                props: Props::read(kid(st, W, "rPr")),
                kind: st.attribute((W, "type")).unwrap_or("").to_string(),
            };
            if matches!(st.attribute((W, "default")), Some("1") | Some("true")) && info.kind == "paragraph" {
                self.default_para = Some(sid.clone());
            }
            self.styles.insert(sid, info);
        }
        let mut size = None;
        let mut font = None;
        let defaults = find(root, W, "rPrDefault").and_then(|n| kid(n, W, "rPr"));
        let normal = self.default_para.as_ref().and_then(|d| kids(root, W, "style").find(|s| s.attribute((W, "styleId")) == Some(d))).and_then(|s| kid(s, W, "rPr"));
        for el in [defaults, normal].into_iter().flatten() {
            if let Some(v) = wval(el, "sz").and_then(|v| v.parse::<f64>().ok()) {
                size = Some(v / 2.0);
            }
            if let Some(f) = kid(el, W, "rFonts") {
                if let Some(a) = f.attribute((W, "ascii")).or(f.attribute((W, "hAnsi"))) {
                    font = Some(a.to_string());
                } else if let Some(t) = f.attribute((W, "asciiTheme")).or(f.attribute((W, "hAnsiTheme"))) {
                    if font.is_none() {
                        font = themes.get(if t.to_lowercase().contains("minor") { "minor" } else { "major" }).cloned();
                    }
                }
            }
        }
        self.doc.size = size.unwrap_or(11.0);
        self.doc.font = font.or_else(|| themes.get("minor").cloned());
    }

    fn read_numbering(&mut self) {
        let Some(text) = self.part("numbering").and_then(|n| self.pkg.text(&n)) else { return };
        let Ok(doc) = roxmltree::Document::parse(&text) else { return };
        let mut abstract_ = HashMap::new();
        for an in kids(doc.root_element(), W, "abstractNum") {
            let mut levels = HashMap::new();
            for lvl in kids(an, W, "lvl") {
                let k: u32 = lvl.attribute((W, "ilvl")).and_then(|v| v.parse().ok()).unwrap_or(0);
                levels.insert(k, wval(lvl, "numFmt").unwrap_or("decimal").to_string());
            }
            abstract_.insert(an.attribute((W, "abstractNumId")).unwrap_or("").to_string(), levels);
        }
        for num in kids(doc.root_element(), W, "num") {
            if let Some(a) = wval(num, "abstractNumId") {
                self.numbering.insert(num.attribute((W, "numId")).unwrap_or("").to_string(), abstract_.get(a).cloned().unwrap_or_default());
            }
        }
    }

    fn read_notes(&mut self) {
        let Some(name) = self.part("footnotes") else { return };
        let Some(text) = self.pkg.text(&name) else { return };
        let Ok(doc) = roxmltree::Document::parse(&text) else { return };
        for fn_ in kids(doc.root_element(), W, "footnote") {
            if matches!(fn_.attribute((W, "type")), Some("separator" | "continuationSeparator" | "continuationNotice")) {
                continue;
            }
            let mut runs: Vec<Inline> = vec![];
            for p in kids(fn_, W, "p") {
                if !runs.is_empty() {
                    runs.push(Inline::Run(Run::new(" ")));
                }
                let mut fields = vec![];
                let content = self.inline(p, &Props::default(), &mut fields, &None, &text);
                runs.extend(content.into_iter().filter(|x| !matches!(x, Inline::Run(r) if r.sup && r.text.trim().chars().all(|c| c.is_ascii_digit()) && !r.text.trim().is_empty())));
            }
            self.footnotes.insert(fn_.attribute((W, "id")).unwrap_or("").to_string(), strip_content(runs));
        }
    }

    fn style_kind(&self, sid: Option<&str>) -> String {
        let mut sid = sid.map(String::from);
        for _ in 0..12 {
            let Some(s) = &sid else { break };
            let Some(st) = self.styles.get(s) else { break };
            let name = st.name.as_str();
            if name == "title" {
                return "title".into();
            }
            if name == "subtitle" {
                return "subtitle".into();
            }
            if let Some(n) = name.strip_prefix("heading").map(|x| x.trim()).and_then(|x| x.parse::<u32>().ok()) {
                return format!("h{}", n.clamp(1, 6));
            }
            if matches!(name, "quote" | "intense quote" | "block text") {
                return "quote".into();
            }
            if name == "caption" {
                return "caption".into();
            }
            if CODE_STYLES.contains(&name) {
                return "code".into();
            }
            if let Some(o) = st.outline {
                if o < 9 {
                    return format!("h{}", (o + 1).min(6));
                }
            }
            sid = st.based.clone();
        }
        "normal".into()
    }

    fn style_props(&self, sid: Option<&str>) -> Props {
        let mut out = Props::default();
        let mut sid = sid.map(String::from);
        for _ in 0..12 {
            let Some(s) = &sid else { break };
            let Some(st) = self.styles.get(s) else { break };
            out.under(&st.props);
            sid = st.based.clone();
        }
        out
    }

    pub fn read(mut self) -> Result<Document, String> {
        let text = self.pkg.text(&self.main).ok_or("the document part is missing")?;
        let xml = roxmltree::Document::parse(&text).map_err(|e| format!("broken document: {e}"))?;
        let body = kid(xml.root_element(), W, "body").ok_or("the document has no body")?;
        let blocks = self.blocks(body, &text);
        self.doc.blocks = group_code(blocks);
        if let Some(sect) = kid(body, W, "sectPr") {
            self.doc.page = page_setup(sect);
        }
        if let Some(core) = self.pkg.text("docProps/core.xml") {
            if let Ok(d) = roxmltree::Document::parse(&core) {
                let get = |name: &str| d.descendants().find(|n| n.is_element() && n.tag_name().name() == name).and_then(|n| n.text()).unwrap_or("").trim().to_string();
                self.doc.title = get("title");
                self.doc.author = get("creator");
            }
        }
        if self.doc.title.is_empty() {
            if let Some(Block::Paragraph(p)) = self.doc.blocks.iter().find(|b| matches!(b, Block::Paragraph(p) if p.style == "title" || p.style == "h1")) {
                self.doc.title = p.text().trim().chars().take(200).collect();
            }
        }
        self.doc.source.insert("docx".into(), serde_bytes::ByteBuf::from(self.pkg.data.clone()));
        Ok(self.doc)
    }

    fn blocks(&self, parent: Node, src: &str) -> Vec<Block> {
        let mut out = vec![];
        for el in parent.children().filter(|n| n.is_element()) {
            let (ns, name) = (el.tag_name().namespace(), el.tag_name().name());
            if ns == Some(W) {
                match name {
                    "p" => out.extend(self.paragraph(el, src)),
                    "tbl" => out.push(Block::Table(self.table(el, src))),
                    "sdt" => {
                        let gallery = find(el, W, "docPartGallery").and_then(|g| g.attribute((W, "val"))).unwrap_or("");
                        if gallery.to_lowercase().contains("contents") {
                            out.push(Block::Paragraph(Paragraph::new(vec![self.opaque(el, src, "table of contents", None, true)])));
                        } else if let Some(c) = kid(el, W, "sdtContent") {
                            out.extend(self.blocks(c, src));
                        }
                    }
                    "customXml" | "ins" | "moveTo" | "smartTag" => out.extend(self.blocks(el, src)),
                    "altChunk" => out.push(Block::Paragraph(Paragraph::new(vec![self.opaque(el, src, "embedded content", None, true)]))),
                    _ => {}
                }
            } else if is(&el, MC, "AlternateContent") {
                if let Some(fb) = kid(el, MC, "Fallback") {
                    out.extend(self.blocks(fb, src));
                }
            }
        }
        out
    }

    fn paragraph(&self, p: Node, src: &str) -> Vec<Block> {
        let ppr = kid(p, W, "pPr");
        let mut sid: Option<String> = None;
        let (mut align, mut lst, mut indent) = (None, None, 0u32);
        let (mut page_before, mut rule, mut section_break) = (false, false, false);
        if let Some(ppr) = ppr {
            sid = wval(ppr, "pStyle").map(String::from);
            align = wval(ppr, "jc").and_then(|v| match v { "center" => Some("center"), "right" | "end" => Some("right"), "both" | "distribute" => Some("justify"), _ => None }).map(String::from);
            if let Some(num) = kid(ppr, W, "numPr") {
                let nid = wval(num, "numId").unwrap_or("");
                let level: u32 = wval(num, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                if let Some(fmt) = self.numbering.get(nid).and_then(|l| l.get(&level)) {
                    if nid != "0" && fmt != "none" {
                        lst = Some((if fmt == "bullet" { "bullet" } else { "number" }.to_string(), level.min(8)));
                    }
                }
            }
            if let Some(ind) = kid(ppr, W, "ind") {
                if lst.is_none() {
                    let left: i64 = ind.attribute((W, "left")).or(ind.attribute((W, "start"))).and_then(|v| v.parse().ok()).unwrap_or(0);
                    if left >= 600 {
                        indent = (left / 720).min(6) as u32;
                    }
                }
            }
            page_before = on(kid(ppr, W, "pageBreakBefore")).unwrap_or(false);
            rule = kid(ppr, W, "pBdr").and_then(|b| kid(b, W, "bottom")).is_some_and(|b| !matches!(b.attribute((W, "val")), None | Some("none") | Some("nil")));
            section_break = kid(ppr, W, "sectPr").is_some();
        }
        if sid.is_none() {
            sid = self.default_para.clone();
        }
        let style = self.style_kind(sid.as_deref());
        let sname = sid.as_ref().and_then(|s| self.styles.get(s)).map(|s| s.name.clone()).unwrap_or_default();
        if lst.is_none() && sname.starts_with("list bullet") {
            lst = Some(("bullet".into(), 0));
        }
        if lst.is_none() && sname.starts_with("list number") {
            lst = Some(("number".into(), 0));
        }
        let mut base = Props::default();
        if style == "normal" {
            let sp = self.style_props(sid.as_deref());
            base.b = sp.b;
            base.i = sp.i;
            base.u = sp.u;
            base.color = sp.color;
            if sp.font != self.doc.font {
                base.font = sp.font;
            }
            if sp.size != Some(self.doc.size) {
                base.size = sp.size;
            }
        }
        let mut fields = vec![];
        let content = self.inline(p, &base, &mut fields, &None, src);
        let mut out = vec![];
        if page_before {
            out.push(Block::PageBreak(PageBreak {}));
        }
        // a page break inside the paragraph splits it (marked by a Break of kind "page")
        let mut parts: Vec<Vec<Inline>> = vec![vec![]];
        for x in content {
            if matches!(&x, Inline::Break(b) if b.kind == "page") {
                parts.push(vec![]);
            } else {
                parts.last_mut().unwrap().push(x);
            }
        }
        let display = find(p, M, "oMathPara").is_some();
        for (k, part) in parts.into_iter().enumerate() {
            if k > 0 {
                out.push(Block::PageBreak(PageBreak {}));
            }
            let part = merge_runs(part);
            let only: Vec<&Inline> = part.iter().filter(|x| !matches!(x, Inline::Run(r) if r.text.trim().is_empty())).collect();
            if only.len() == 1 && display {
                if let Inline::Run(r) = only[0] {
                    if r.math {
                        out.push(Block::MathBlock(MathBlock { tex: r.text.clone() }));
                        continue;
                    }
                }
            }
            if rule && part.iter().all(|x| matches!(x, Inline::Run(r) if r.text.trim().is_empty())) {
                out.push(Block::Rule(Rule {}));
                continue;
            }
            if k > 0 && part.is_empty() {
                continue;
            }
            out.push(Block::Paragraph(Paragraph { content: part, style: style.clone(), align: align.clone(), list: lst.clone(), indent }));
        }
        if section_break {
            out.push(Block::PageBreak(PageBreak {}));
        }
        out
    }

    fn inline(&self, parent: Node, base: &Props, fields: &mut Vec<Field>, link: &Option<String>, src: &str) -> Vec<Inline> {
        let mut out = vec![];
        for el in parent.children().filter(|n| n.is_element()) {
            let (ns, name) = (el.tag_name().namespace(), el.tag_name().name());
            if ns == Some(W) {
                match name {
                    "r" => out.extend(self.run(el, base, fields, link, src)),
                    "hyperlink" => {
                        let url = el.attribute((R, "id")).and_then(|r| self.rels.get(r)).map(|r| r.target.clone())
                            .or_else(|| el.attribute((W, "anchor")).map(|a| format!("#{a}")));
                        out.extend(self.inline(el, base, fields, &url, src));
                    }
                    "ins" | "moveTo" | "smartTag" | "customXml" | "dir" | "bdo" => out.extend(self.inline(el, base, fields, link, src)),
                    "sdt" => {
                        if let Some(c) = kid(el, W, "sdtContent") {
                            out.extend(self.inline(c, base, fields, link, src));
                        }
                    }
                    "fldSimple" => {
                        let instr = el.attribute((W, "instr")).unwrap_or("");
                        let url = Regex::new(r#"HYPERLINK\s+"([^"]+)""#).unwrap().captures(instr).map(|m| m[1].to_string());
                        let l = if url.is_some() { url } else { link.clone() };
                        out.extend(self.inline(el, base, fields, &l, src));
                    }
                    _ => {}
                }
            } else if ns == Some(M) && (name == "oMath" || name == "oMathPara") {
                out.push(self.math(el, src));
            } else if is(&el, MC, "AlternateContent") {
                if let Some(fb) = kid(el, MC, "Fallback") {
                    out.extend(self.inline(fb, base, fields, link, src));
                }
            }
        }
        out
    }

    fn math(&self, el: Node, src: &str) -> Inline {
        let tex = mathtex::omml_to_tex(el).trim().to_string();
        if tex.is_empty() {
            let text: String = el.descendants().filter(|n| is(n, M, "t")).filter_map(|n| n.text()).collect();
            return self.opaque(el, src, "equation", Some(text), false);
        }
        Inline::Run(Run { text: tex, math: true, ..Default::default() })
    }

    fn run(&self, r: Node, base: &Props, fields: &mut Vec<Field>, link: &Option<String>, src: &str) -> Vec<Inline> {
        let rp = Props::read(kid(r, W, "rPr"));
        let mut props = rp.clone();
        if let Some(rs) = &rp.rstyle {
            let sp = self.style_props(Some(rs));
            let hyper = self.styles.get(rs).is_some_and(|s| s.name == "hyperlink");
            let mut sp2 = sp.clone();
            if hyper {
                sp2.u = None;
                sp2.color = None;
            }
            props.under(&sp2);
        }
        props.under(base);
        if props.hidden == Some(true) {
            return vec![];
        }
        let mut out = vec![];
        let allowed = |fields: &Vec<Field>| fields.iter().all(|f| f.result);
        let make = |text: &str, fields: &Vec<Field>| -> Inline {
            let mut l = link.clone();
            for f in fields {
                if f.link.is_some() {
                    l = f.link.clone();
                }
            }
            let text = if props.caps == Some(true) { text.to_uppercase() } else { text.to_string() };
            Inline::Run(Run {
                text,
                b: props.b.unwrap_or(false),
                i: props.i.unwrap_or(false),
                u: props.u.unwrap_or(false) && l.is_none(),
                s: props.s.unwrap_or(false),
                sup: props.sup.unwrap_or(false),
                sub: props.sub.unwrap_or(false),
                size: props.size.filter(|s| *s != self.doc.size),
                font: props.font.clone().filter(|f| Some(f) != self.doc.font.as_ref()),
                color: if l.is_some() { None } else { props.color.clone() },
                hl: props.hl.clone(),
                link: l,
                ..Default::default()
            })
        };
        for el in r.children().filter(|n| n.is_element()) {
            let (ns, name) = (el.tag_name().namespace(), el.tag_name().name());
            if ns == Some(MC) && name == "AlternateContent" {
                if allowed(fields) {
                    let label = if find(el, W, "txbxContent").is_some() { "text box" } else { "shape" };
                    out.push(self.opaque(r, src, label, None, false));
                }
                return out;
            }
            if ns != Some(W) {
                continue;
            }
            match name {
                "t" => {
                    if allowed(fields) {
                        let t = el.text().unwrap_or("");
                        if !t.is_empty() {
                            out.push(make(t, fields));
                        }
                    }
                }
                "tab" => {
                    if allowed(fields) {
                        out.push(make("\t", fields));
                    }
                }
                "br" | "cr" => {
                    if allowed(fields) {
                        let kind = if el.attribute((W, "type")) == Some("page") { "page" } else { "line" };
                        out.push(Inline::Break(Break { kind: kind.into() }));
                    }
                }
                "noBreakHyphen" => {
                    if allowed(fields) {
                        out.push(make("\u{2011}", fields));
                    }
                }
                "sym" => {
                    if let Some(code) = el.attribute((W, "char")).and_then(|c| u32::from_str_radix(c, 16).ok()) {
                        let c = if code >= 0xF000 { code - 0xF000 } else { code };
                        if let (Some(ch), true) = (char::from_u32(c), allowed(fields)) {
                            out.push(make(&ch.to_string(), fields));
                        }
                    }
                }
                "fldChar" => match el.attribute((W, "fldCharType")) {
                    Some("begin") => fields.push(Field { instr: String::new(), result: false, link: None }),
                    Some("separate") => {
                        if let Some(f) = fields.last_mut() {
                            f.result = true;
                            if let Some(m) = Regex::new(r#"HYPERLINK\s+"([^"]+)""#).unwrap().captures(&f.instr) {
                                f.link = Some(m[1].to_string());
                            }
                        }
                    }
                    Some("end") => {
                        fields.pop();
                    }
                    _ => {}
                },
                "instrText" => {
                    if let Some(f) = fields.last_mut() {
                        f.instr.push_str(el.text().unwrap_or(""));
                    }
                }
                "drawing" => {
                    if allowed(fields) {
                        out.push(self.drawing(el, r, src));
                    }
                }
                "pict" => {
                    if allowed(fields) {
                        out.push(self.pict(el, r, src));
                    }
                }
                "object" => {
                    if allowed(fields) {
                        out.push(self.opaque(r, src, "embedded object", None, false));
                    }
                    return out;
                }
                "footnoteReference" => {
                    if let Some(n) = el.attribute((W, "id")).and_then(|id| self.footnotes.get(id)) {
                        out.push(Inline::Note(Note { runs: n.clone() }));
                    }
                }
                "endnoteReference" => {
                    out.push(self.opaque(r, src, "endnote", None, false));
                    return out;
                }
                _ => {}
            }
        }
        out
    }

    fn drawing(&self, d: Node, run: Node, src: &str) -> Inline {
        let Some(bx) = kid(d, WP, "inline").or_else(|| kid(d, WP, "anchor")) else { return self.opaque(run, src, "drawing", None, false) };
        let ext = kid(bx, WP, "extent");
        let w = ext.and_then(|e| e.attribute("cx")).and_then(|v| v.parse::<f64>().ok()).map(|v| v / EMU).unwrap_or(200.0);
        let h = ext.and_then(|e| e.attribute("cy")).and_then(|v| v.parse::<f64>().ok()).map(|v| v / EMU).unwrap_or(150.0);
        let alt = kid(bx, WP, "docPr").and_then(|p| p.attribute("descr").or(p.attribute("title"))).unwrap_or("").to_string();
        let gd = kid(bx, A, "graphic").and_then(|g| kid(g, A, "graphicData"));
        let uri = gd.and_then(|g| g.attribute("uri")).unwrap_or("");
        if uri.ends_with("/picture") {
            let rid = gd.and_then(|g| find(g, A, "blip")).and_then(|b| b.attribute((R, "embed")));
            if let Some(rel) = rid.and_then(|r| self.rels.get(r)) {
                if let Some(data) = self.pkg.get(&rel.target) {
                    let e = match imaging::sniff(data) { "" => imaging::ext_for(&rel.target), x => x.to_string() };
                    let name = rel.target.rsplit('/').next().unwrap_or("").to_string();
                    return Inline::Image(Image { data: data.clone(), ext: e, width: w, height: h, alt, name });
                }
            }
        } else if uri.ends_with("/chart") {
            let rid = gd.and_then(|g| kid(g, C, "chart")).and_then(|c| c.attribute((R, "id")));
            if let Some(rel) = rid.and_then(|r| self.rels.get(r)) {
                if let Some(mut chart) = self.pkg.text(&rel.target).and_then(|x| charts::from_chart_xml(&x)) {
                    chart.width = w;
                    chart.height = h;
                    return Inline::Chart(chart);
                }
            }
            return self.opaque(run, src, "chart", None, false);
        }
        let label = if uri.contains("diagram") { "diagram" } else if gd.is_some_and(|g| find(g, W, "txbxContent").is_some()) { "text box" } else { "shape" };
        self.opaque(run, src, label, None, false)
    }

    fn pict(&self, el: Node, run: Node, src: &str) -> Inline {
        let textbox = find(el, V, "textbox").is_some();
        if let (Some(img), false) = (find(el, V, "imagedata"), textbox) {
            if let Some(rel) = img.attribute((R, "id")).and_then(|r| self.rels.get(r)) {
                if let Some(data) = self.pkg.get(&rel.target) {
                    let style = el.descendants().filter(|n| is(n, V, "shape")).filter_map(|n| n.attribute("style")).last().unwrap_or("");
                    let w = css_pt(style, "width").unwrap_or(200.0);
                    let h = css_pt(style, "height").unwrap_or(150.0);
                    let e = match imaging::sniff(data) { "" => imaging::ext_for(&rel.target), x => x.to_string() };
                    return Inline::Image(Image { data: data.clone(), ext: e, width: w, height: h, alt: String::new(), name: rel.target.rsplit('/').next().unwrap_or("").into() });
                }
            }
        }
        self.opaque(run, src, if textbox { "text box" } else { "shape" }, None, false)
    }

    fn opaque(&self, el: Node, src: &str, label: &str, text: Option<String>, block: bool) -> Inline {
        let mut rels = BTreeMap::new();
        for n in el.descendants().filter(|n| n.is_element()) {
            for a in n.attributes() {
                if a.namespace() == Some(R) {
                    if let Some(rel) = self.rels.get(a.value()) {
                        rels.insert(a.value().to_string(), (rel.typ.clone(), rel.target.clone(), if rel.external { "External".to_string() } else { String::new() }));
                    }
                }
            }
        }
        let text = text.unwrap_or_else(|| el.descendants().filter(|n| is(n, W, "t")).filter_map(|n| n.text()).collect::<Vec<_>>().join(" ").trim().to_string());
        Inline::Opaque(Opaque { xml: element_source(el, src), label: label.into(), text, rels, block, fmt: "docx".into() })
    }

    fn table(&self, tbl: Node, src: &str) -> Table {
        let mut t = Table { rows: vec![], header: false, widths: None };
        let grid: Vec<f64> = kid(tbl, W, "tblGrid").map(|g| kids(g, W, "gridCol").map(|c| c.attribute((W, "w")).and_then(|v| v.parse().ok()).unwrap_or(0.0)).collect()).unwrap_or_default();
        if !grid.is_empty() && grid.iter().sum::<f64>() > 0.0 {
            t.widths = Some(grid);
        }
        let look = kid(tbl, W, "tblPr").and_then(|p| kid(p, W, "tblLook"));
        let first_row = look.and_then(|l| {
            if let Some(fr) = l.attribute((W, "firstRow")) {
                Some(fr == "1" || fr == "true")
            } else {
                l.attribute((W, "val")).and_then(|v| u32::from_str_radix(v, 16).ok()).map(|v| v & 0x20 != 0)
            }
        });
        let mut header_flag = false;
        for (ri, tr) in kids(tbl, W, "tr").enumerate() {
            if ri == 0 && kid(tr, W, "trPr").and_then(|p| kid(p, W, "tblHeader")).is_some() {
                header_flag = true;
            }
            let mut row = vec![];
            for tc in kids(tr, W, "tc") {
                let tcpr = kid(tc, W, "tcPr");
                let span: usize = tcpr.and_then(|p| wval(p, "gridSpan")).and_then(|v| v.parse().ok()).unwrap_or(1);
                let cont = tcpr.and_then(|p| kid(p, W, "vMerge")).is_some_and(|v| v.attribute((W, "val")).unwrap_or("continue") == "continue");
                let mut paras = vec![];
                if !cont {
                    for el in tc.children().filter(|n| n.is_element()) {
                        if is(&el, W, "p") {
                            for b in self.paragraph(el, src) {
                                if let Block::Paragraph(mut p) = b {
                                    if p.style == "code" {
                                        p.style = "normal".into();
                                    }
                                    paras.push(p);
                                }
                            }
                        } else if is(&el, W, "tbl") {
                            let inner = self.table(el, src);
                            for irow in inner.rows {
                                let line = irow.iter().map(|c| c.iter().map(|p| p.text()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\t");
                                paras.push(Paragraph::new(vec![Inline::Run(Run::new(line))]));
                            }
                        } else if is(&el, W, "sdt") {
                            if let Some(c) = kid(el, W, "sdtContent") {
                                for b in self.blocks(c, src) {
                                    if let Block::Paragraph(p) = b {
                                        paras.push(p);
                                    }
                                }
                            }
                        }
                    }
                }
                if paras.is_empty() {
                    paras.push(Paragraph::default());
                }
                row.push(paras);
                for _ in 1..span {
                    row.push(vec![Paragraph::default()]);
                }
            }
            t.rows.push(row);
        }
        t.header = header_flag || first_row.unwrap_or(false);
        if !header_flag && first_row.is_none() {
            if let Some(first) = t.rows.first() {
                t.header = first.iter().all(|cell| {
                    cell.iter().any(|p| !p.text().trim().is_empty())
                        && cell.iter().all(|p| p.content.iter().all(|x| matches!(x, Inline::Run(r) if r.b || r.text.trim().is_empty())))
                });
            }
        }
        t
    }
}

fn css_pt(style: &str, key: &str) -> Option<f64> {
    let m = Regex::new(&format!(r"{key}\s*:\s*([\d.]+)\s*(pt|in|px|cm|mm)?")).unwrap().captures(style)?;
    let v: f64 = m[1].parse().ok()?;
    Some(v * match m.get(2).map(|x| x.as_str()) { Some("in") => 72.0, Some("px") => 0.75, Some("cm") => 28.35, Some("mm") => 2.835, _ => 1.0 })
}

fn page_setup(sect: Node) -> PageSetup {
    let mut page = PageSetup { numbers: false, ..Default::default() };
    if let Some(sz) = kid(sect, W, "pgSz") {
        if let (Some(w), Some(h)) = (attr(sz, W, "w").and_then(|v| v.parse::<f64>().ok()), attr(sz, W, "h").and_then(|v| v.parse::<f64>().ok())) {
            page.width = w / 20.0;
            page.height = h / 20.0;
        }
    }
    if let Some(m) = kid(sect, W, "pgMar") {
        let g = |k: &str| attr(m, W, k).and_then(|v| v.parse::<f64>().ok()).map(|v| v.abs() / 20.0).unwrap_or(72.0);
        page.margins = (g("top"), g("right"), g("bottom"), g("left"));
    }
    page
}

/// Consecutive code-style paragraphs become one code block.
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

pub fn read(data: &[u8]) -> Result<Document, String> {
    let pkg = Package::open(data)?;
    Reader::new(&pkg).read()
}

// -- writing ------------------------------------------------------------------------

const OUR_STYLES: [(&str, &str, &str); 16] = [
    ("title", "Title", "Title"), ("subtitle", "Subtitle", "Subtitle"), ("h1", "Heading1", "heading 1"),
    ("h2", "Heading2", "heading 2"), ("h3", "Heading3", "heading 3"), ("h4", "Heading4", "heading 4"),
    ("h5", "Heading5", "heading 5"), ("h6", "Heading6", "heading 6"), ("quote", "Quote", "Quote"),
    ("caption", "Caption", "caption"), ("code", "SourceCode", "Source Code"), ("list", "ListParagraph", "List Paragraph"),
    ("hyperlink", "Hyperlink", "Hyperlink"), ("fntext", "FootnoteText", "footnote text"),
    ("fnref", "FootnoteReference", "footnote reference"), ("table", "TableGrid", "Table Grid"),
];

fn style_xml(key: &str, sid: &str, base: f64) -> String {
    let name = OUR_STYLES.iter().find(|s| s.0 == key).map(|s| s.2).unwrap_or(key);
    match key {
        "title" | "subtitle" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "quote" | "caption" => {
            let lk = look(key);
            let size = (base * lk.scale * 2.0).round();
            let mut rpr = String::new();
            if let Some(c) = lk.color {
                rpr.push_str(&format!(r#"<w:color w:val="{}"/>"#, &c[1..]));
            }
            if lk.italic {
                rpr.push_str("<w:i/><w:iCs/>");
            }
            if lk.bold {
                rpr.push_str("<w:b/><w:bCs/>");
            }
            rpr.push_str(&format!(r#"<w:sz w:val="{size}"/><w:szCs w:val="{size}"/>"#));
            let (before, after) = ((lk.before * 20.0) as i64, (lk.after * 20.0) as i64);
            let mut ppr = if key.starts_with('h') || key == "title" || key == "subtitle" {
                format!(r#"<w:keepNext/><w:keepLines/><w:spacing w:before="{before}" w:after="{after}"/>"#)
            } else {
                format!(r#"<w:spacing w:before="{before}" w:after="{after}"/>"#)
            };
            if let Some(n) = key.strip_prefix('h').and_then(|n| n.parse::<u32>().ok()) {
                ppr.push_str(&format!(r#"<w:outlineLvl w:val="{}"/>"#, n - 1));
            }
            if key == "title" || key == "subtitle" {
                ppr.push_str(r#"<w:jc w:val="center"/>"#);
            }
            if key == "quote" {
                ppr.push_str(r#"<w:ind w:left="720" w:right="720"/>"#);
            }
            format!(r#"<w:style w:type="paragraph" w:styleId="{sid}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr>{ppr}</w:pPr><w:rPr>{rpr}</w:rPr></w:style>"#)
        }
        "code" => format!(r#"<w:style w:type="paragraph" w:customStyle="1" w:styleId="{sid}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F4F4F4"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:contextualSpacing/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="{}"/></w:rPr></w:style>"#, (base * 0.88 * 2.0).round()),
        "list" => format!(r#"<w:style w:type="paragraph" w:styleId="{sid}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>"#),
        "hyperlink" => format!(r#"<w:style w:type="character" w:styleId="{sid}"><w:name w:val="{name}"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="1F5FBF"/><w:u w:val="single"/></w:rPr></w:style>"#),
        "fntext" => format!(r#"<w:style w:type="paragraph" w:styleId="{sid}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0"/></w:pPr><w:rPr><w:sz w:val="{}"/></w:rPr></w:style>"#, (base * 0.82 * 2.0).round()),
        "fnref" => format!(r#"<w:style w:type="character" w:styleId="{sid}"><w:name w:val="{name}"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#),
        "table" => {
            let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"].iter().map(|k| format!(r#"<w:{k} w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>"#)).collect();
            format!(r#"<w:style w:type="table" w:styleId="{sid}"><w:name w:val="{name}"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders>{borders}</w:tblBorders><w:tblCellMar><w:top w:w="40" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="40" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>"#)
        }
        _ => String::new(),
    }
}

fn styles_part(base: f64, font: &str) -> String {
    let size = (base * 2.0).round();
    let f = esc_attr(font);
    let ours: String = OUR_STYLES.iter().map(|(k, sid, _)| style_xml(k, sid, base)).collect();
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/><w:sz w:val="{size}"/><w:szCs w:val="{size}"/><w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>{ours}</w:styles>"#)
}

fn lvl(ilvl: u32, kind: &str) -> String {
    let left = 720 * (ilvl + 1);
    if kind == "bullet" {
        let sym = BULLETS[(ilvl % 3) as usize];
        format!(r#"<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{sym}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#)
    } else {
        let fmt = ["decimal", "lowerLetter", "lowerRoman"][(ilvl % 3) as usize];
        format!(r#"<w:lvl w:ilvl="{ilvl}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#, ilvl + 1)
    }
}

struct Writer<'d> {
    doc: &'d Document,
    src: Option<Package>,
    main_dir: String,
    parts: BTreeMap<String, Vec<u8>>,
    overrides: BTreeMap<String, String>,
    rels: Vec<(String, String, String, bool)>,
    used_rids: BTreeSet<String>,
    next_rid: usize,
    media_n: usize,
    chart_n: usize,
    ids: usize,
    notes: Vec<Vec<Inline>>,
    styles: HashMap<&'static str, String>,
    missing: Vec<&'static str>,
    num_ids: Vec<(usize, &'static str)>,
    num_base: usize,
}

impl<'d> Writer<'d> {
    fn rid(&mut self) -> String {
        loop {
            let r = format!("rId{}", self.next_rid);
            self.next_rid += 1;
            if !self.used_rids.contains(&r) {
                self.used_rids.insert(r.clone());
                return r;
            }
        }
    }

    fn add_rel(&mut self, typ: &str, target: &str, external: bool) -> String {
        let r = self.rid();
        self.rels.push((r.clone(), typ.into(), target.into(), external));
        r
    }

    fn style(&self, key: &str) -> String {
        self.styles.get(key).cloned().unwrap_or_default()
    }

    fn rpr(&self, r: &Run) -> String {
        let mut p = String::new();
        if r.link.is_some() {
            p.push_str(&format!(r#"<w:rStyle w:val="{}"/>"#, self.style("hyperlink")));
        }
        if let Some(f) = &r.font {
            let f = esc_attr(f);
            p.push_str(&format!(r#"<w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:cs="{f}"/>"#));
        } else if r.code {
            p.push_str(r#"<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/>"#);
        }
        if r.b {
            p.push_str("<w:b/><w:bCs/>");
        }
        if r.i {
            p.push_str("<w:i/><w:iCs/>");
        }
        if r.s {
            p.push_str("<w:strike/>");
        }
        if let Some(c) = &r.color {
            p.push_str(&format!(r#"<w:color w:val="{}"/>"#, c.trim_start_matches('#').to_uppercase()));
        }
        if let Some(s) = r.size {
            p.push_str(&format!(r#"<w:sz w:val="{0}"/><w:szCs w:val="{0}"/>"#, (s * 2.0).round()));
        } else if r.code {
            p.push_str(&format!(r#"<w:sz w:val="{}"/>"#, (self.doc.size * 0.9 * 2.0).round()));
        }
        if let Some(h) = &r.hl {
            let named = ["yellow", "green", "cyan", "magenta", "blue", "red", "darkBlue", "darkCyan", "darkGreen", "darkMagenta", "darkRed", "darkYellow", "darkGray", "lightGray", "black", "white"]
                .iter().find(|n| highlight(n).is_some_and(|v| v.eq_ignore_ascii_case(h)));
            match named {
                Some(n) => p.push_str(&format!(r#"<w:highlight w:val="{n}"/>"#)),
                None => p.push_str(&format!(r#"<w:shd w:val="clear" w:color="auto" w:fill="{}"/>"#, h.trim_start_matches('#').to_uppercase())),
            }
        }
        if r.u && r.link.is_none() {
            p.push_str(r#"<w:u w:val="single"/>"#);
        }
        if r.sup {
            p.push_str(r#"<w:vertAlign w:val="superscript"/>"#);
        } else if r.sub {
            p.push_str(r#"<w:vertAlign w:val="subscript"/>"#);
        }
        if p.is_empty() { p } else { format!("<w:rPr>{p}</w:rPr>") }
    }

    fn run_xml(&self, r: &Run) -> String {
        if r.math {
            return mathtex::to_omml(&r.text, false);
        }
        let rpr = self.rpr(r);
        let mut out = String::new();
        for (k, piece) in r.text.split('\t').enumerate() {
            if k > 0 {
                out.push_str("<w:tab/>");
            }
            for (j, line) in piece.split('\n').enumerate() {
                if j > 0 {
                    out.push_str("<w:br/>");
                }
                if !line.is_empty() {
                    out.push_str(&format!(r#"<w:t xml:space="preserve">{}</w:t>"#, esc(line)));
                }
            }
        }
        format!("<w:r>{rpr}{out}</w:r>")
    }

    fn inline(&mut self, content: &[Inline]) -> String {
        let content = merge_runs(content.to_vec());
        let mut out = String::new();
        let mut i = 0;
        while i < content.len() {
            match &content[i] {
                Inline::Run(r) if r.link.is_some() => {
                    let link = r.link.clone().unwrap();
                    let mut inner = self.run_xml(r);
                    while i + 1 < content.len() {
                        if let Inline::Run(n) = &content[i + 1] {
                            if n.link.as_ref() == Some(&link) {
                                inner.push_str(&self.run_xml(n));
                                i += 1;
                                continue;
                            }
                        }
                        break;
                    }
                    if let Some(anchor) = link.strip_prefix('#') {
                        out.push_str(&format!(r#"<w:hyperlink w:anchor="{}">{inner}</w:hyperlink>"#, esc_attr(anchor)));
                    } else {
                        let rid = self.add_rel(&format!("{REL}/hyperlink"), &link, true);
                        out.push_str(&format!(r#"<w:hyperlink r:id="{rid}" w:history="1">{inner}</w:hyperlink>"#));
                    }
                }
                Inline::Run(r) => out.push_str(&self.run_xml(r)),
                Inline::Break(b) => out.push_str(if b.kind == "page" { r#"<w:r><w:br w:type="page"/></w:r>"# } else { "<w:r><w:br/></w:r>" }),
                Inline::Image(img) => {
                    let x = self.image_xml(img);
                    out.push_str(&x);
                }
                Inline::Chart(c) => {
                    let x = self.chart_xml(c);
                    out.push_str(&x);
                }
                Inline::Note(n) => {
                    self.notes.push(n.runs.clone());
                    out.push_str(&format!(r#"<w:r><w:rPr><w:rStyle w:val="{}"/></w:rPr><w:footnoteReference w:id="{}"/></w:r>"#, self.style("fnref"), self.notes.len()));
                }
                Inline::Opaque(o) => {
                    let x = self.opaque_xml(o);
                    out.push_str(&x);
                }
            }
            i += 1;
        }
        out
    }

    fn media(&mut self, data: &[u8], ext: &str) -> String {
        loop {
            self.media_n += 1;
            let name = format!("word/media/image{}.{}", self.media_n, ext);
            if !self.src.as_ref().is_some_and(|s| s.parts.contains_key(&name)) && !self.parts.contains_key(&name) {
                self.parts.insert(name.clone(), data.to_vec());
                return name;
            }
        }
    }

    fn image_xml(&mut self, img: &Image) -> String {
        let ext = match img.ext.as_str() { "" => imaging::sniff(&img.data).to_string(), "jpg" => "jpeg".into(), e => e.to_string() };
        let ext = if ext.is_empty() { "png".to_string() } else { ext };
        let name = self.media(&img.data, &ext);
        let target = opc::relative(&name, &self.main_dir.clone());
        let rid = self.add_rel(&format!("{REL}/image"), &target, false);
        self.ids += 1;
        let id = self.ids;
        let cx = ((if img.width > 0.0 { img.width } else { 200.0 }) * EMU) as i64;
        let cy = ((if img.height > 0.0 { img.height } else { 150.0 }) * EMU) as i64;
        format!(r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="Picture {id}" descr="{}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="Picture {id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#,
                esc_attr(&img.alt))
    }

    fn chart_xml(&mut self, chart: &Chart) -> String {
        let n = loop {
            self.chart_n += 1;
            let name = format!("word/charts/chart{}.xml", self.chart_n);
            if !self.src.as_ref().is_some_and(|s| s.parts.contains_key(&name)) {
                break self.chart_n;
            }
        };
        let name = format!("word/charts/chart{n}.xml");
        self.parts.insert(name.clone(), charts::chart_xml(chart, Some("rId1")).into_bytes());
        self.parts.insert(format!("word/embeddings/Microsoft_Excel_Worksheet{n}.xlsx"), charts::workbook(chart));
        self.parts.insert(format!("word/charts/_rels/chart{n}.xml.rels"), format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/package" Target="../embeddings/Microsoft_Excel_Worksheet{n}.xlsx"/></Relationships>"#).into_bytes());
        self.overrides.insert(format!("/{name}"), "application/vnd.openxmlformats-officedocument.drawingml.chart+xml".into());
        let target = opc::relative(&name, &self.main_dir.clone());
        let rid = self.add_rel(&format!("{REL}/chart"), &target, false);
        self.ids += 1;
        let id = self.ids;
        let (cx, cy) = ((chart.width * EMU) as i64, (chart.height * EMU) as i64);
        format!(r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="Chart {id}" descr="{}"/><wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="{rid}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#,
                esc_attr(if chart.title.is_empty() { "Chart" } else { &chart.title }))
    }

    fn opaque_xml(&mut self, o: &Opaque) -> String {
        if o.fmt != "docx" || o.xml.is_empty() {
            return if o.text.is_empty() { String::new() } else { self.run_xml(&Run::new(o.text.clone())) };
        }
        let mut xml = o.xml.clone();
        for (old, (typ, target, mode)) in &o.rels {
            let external = mode == "External";
            let t = if external {
                target.clone()
            } else {
                self.copy_part(target, 0);
                opc::relative(target, &self.main_dir.clone())
            };
            let new = self.add_rel(typ, &t, external);
            let re = Regex::new(&format!(r#"(r:\w+=")({})""#, regex::escape(old))).unwrap();
            xml = re.replace_all(&xml, format!("${{1}}{new}\"").as_str()).into_owned();
        }
        xml
    }

    /// Copy a part of the original file, with the parts it refers to.
    fn copy_part(&mut self, name: &str, depth: usize) {
        if depth > 4 || self.parts.contains_key(name) {
            return;
        }
        let Some(src) = &self.src else { return };
        let Some(data) = src.get(name).cloned() else { return };
        let rels = src.rels(name);
        let (dir, base) = name.rsplit_once('/').unwrap_or(("", name));
        let rels_name = if dir.is_empty() { format!("_rels/{base}.rels") } else { format!("{dir}/_rels/{base}.rels") };
        let rel_data = src.get(&rels_name).cloned();
        self.parts.insert(name.to_string(), data);
        if let Some(rd) = rel_data {
            self.parts.insert(rels_name, rd);
            for r in rels.values() {
                if !r.external {
                    self.copy_part(&r.target, depth + 1);
                }
            }
        }
    }

    fn para_xml(&mut self, p: &Paragraph, num: Option<usize>) -> String {
        let mut ppr = String::new();
        let sid = if OUR_STYLES.iter().any(|s| s.0 == p.style) && p.style != "normal" {
            Some(self.style(&p.style))
        } else if p.list.is_some() {
            Some(self.style("list"))
        } else {
            None
        };
        if let Some(s) = sid {
            ppr.push_str(&format!(r#"<w:pStyle w:val="{s}"/>"#));
        }
        if let (Some((_, level)), Some(n)) = (&p.list, num) {
            ppr.push_str(&format!(r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{n}"/></w:numPr>"#));
        }
        if p.indent > 0 && p.list.is_none() {
            ppr.push_str(&format!(r#"<w:ind w:left="{}"/>"#, 720 * p.indent));
        }
        if let Some(a) = &p.align {
            ppr.push_str(&format!(r#"<w:jc w:val="{}"/>"#, if a == "justify" { "both" } else { a }));
        }
        let content = self.inline(&p.content);
        if ppr.is_empty() { format!("<w:p>{content}</w:p>") } else { format!("<w:p><w:pPr>{ppr}</w:pPr>{content}</w:p>") }
    }

    fn table_xml(&mut self, t: &Table) -> String {
        let n = t.ncols();
        if n == 0 {
            return String::new();
        }
        let total = (self.doc.page.text_width() * 20.0) as i64;
        let cols: Vec<i64> = match &t.widths {
            Some(ws) if ws.len() == n && ws.iter().sum::<f64>() > 0.0 => {
                let s: f64 = ws.iter().sum();
                ws.iter().map(|w| (total as f64 * w / s) as i64).collect()
            }
            _ => vec![total / n as i64; n],
        };
        let grid: String = cols.iter().map(|c| format!(r#"<w:gridCol w:w="{c}"/>"#)).collect();
        let mut rows = String::new();
        for (r, row) in t.rows.iter().enumerate() {
            let header = t.header && r == 0;
            let mut cells = String::new();
            for c in 0..n {
                let mut paras = row.get(c).cloned().unwrap_or_else(|| vec![Paragraph::default()]);
                if header {
                    for p in paras.iter_mut() {
                        for x in p.content.iter_mut() {
                            if let Inline::Run(rr) = x {
                                rr.b = true;
                            }
                        }
                    }
                }
                let mut inner: String = paras.iter().map(|p| self.para_xml(p, None)).collect();
                if inner.is_empty() {
                    inner = "<w:p/>".into();
                }
                let shade = if header { r#"<w:shd w:val="clear" w:color="auto" w:fill="F0F0F0"/>"# } else { "" };
                cells.push_str(&format!(r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/>{shade}</w:tcPr>{inner}</w:tc>"#, cols[c]));
            }
            let trpr = if header { "<w:trPr><w:tblHeader/></w:trPr>" } else { "" };
            rows.push_str(&format!("<w:tr>{trpr}{cells}</w:tr>"));
        }
        let look = if t.header {
            r#"<w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#
        } else {
            r#"<w:tblLook w:val="0400" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#
        };
        format!(r#"<w:tbl><w:tblPr><w:tblStyle w:val="{}"/><w:tblW w:w="{}" w:type="dxa"/>{look}</w:tblPr><w:tblGrid>{grid}</w:tblGrid>{rows}</w:tbl>"#,
                self.style("table"), cols.iter().sum::<i64>())
    }

    fn new_num(&mut self, kind: &'static str) -> usize {
        let id = self.num_ids.iter().map(|x| x.0).max().unwrap_or(self.num_base).max(self.num_base) + 1;
        self.num_ids.push((id, kind));
        id
    }

    fn body(&mut self) -> String {
        let mut out: Vec<String> = vec![];
        let (mut bullet, mut number): (Option<usize>, Option<usize>) = (None, None);
        let mut prev_list = false;
        let blocks = &self.doc.blocks;
        for (k, b) in blocks.iter().enumerate() {
            match b {
                Block::Paragraph(p) => {
                    if let [Inline::Opaque(o)] = p.content.as_slice() {
                        if o.block {
                            let x = self.opaque_xml(o);
                            out.push(x);
                            prev_list = false;
                            continue;
                        }
                    }
                    let mut num = None;
                    if let Some((kind, _)) = &p.list {
                        if kind == "bullet" {
                            if bullet.is_none() {
                                bullet = Some(self.new_num("bullet"));
                            }
                            num = bullet;
                        } else {
                            if number.is_none() || !prev_list {
                                number = Some(self.new_num("number"));
                            }
                            num = number;
                        }
                    }
                    let x = self.para_xml(p, num);
                    out.push(x);
                    prev_list = p.list.is_some();
                    continue;
                }
                Block::Table(t) => {
                    let x = self.table_xml(t);
                    out.push(x);
                    if matches!(blocks.get(k + 1), Some(Block::Table(_))) {
                        out.push("<w:p/>".into()); // Word needs a paragraph between two tables
                    }
                }
                Block::CodeBlock(c) => {
                    let sid = self.style("code");
                    for line in c.text.split('\n') {
                        let r = if line.is_empty() { String::new() } else { format!(r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#, esc(line)) };
                        out.push(format!(r#"<w:p><w:pPr><w:pStyle w:val="{sid}"/></w:pPr>{r}</w:p>"#));
                    }
                }
                Block::MathBlock(m) => out.push(format!("<w:p>{}</w:p>", mathtex::to_omml(&m.tex, true))),
                Block::Rule(_) => out.push(r#"<w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="auto"/></w:pBdr></w:pPr></w:p>"#.into()),
                Block::PageBreak(_) => out.push(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#.into()),
            }
            prev_list = false;
        }
        out.concat()
    }

    fn map_styles(&mut self, text: &str) {
        let Ok(doc) = roxmltree::Document::parse(text) else { return };
        let mut by_name = HashMap::new();
        let mut ids = BTreeSet::new();
        for st in kids(doc.root_element(), W, "style") {
            let sid = st.attribute((W, "styleId")).unwrap_or("").to_string();
            ids.insert(sid.clone());
            if let Some(n) = wval(st, "name") {
                by_name.insert(n.to_lowercase(), sid);
            }
        }
        for (key, sid, name) in OUR_STYLES {
            let mut hit = by_name.get(&name.to_lowercase()).cloned();
            if key == "code" && hit.is_none() {
                hit = CODE_STYLES.iter().find_map(|n| by_name.get(*n).cloned());
            }
            match hit {
                Some(h) => {
                    self.styles.insert(key, h);
                }
                None => {
                    let s = if ids.contains(sid) { format!("Rw{sid}") } else { sid.to_string() };
                    self.styles.insert(key, s);
                    self.missing.push(key);
                }
            }
        }
    }

    fn sect_pr(&mut self, main: &str) -> String {
        let page = &self.doc.page;
        let (w, h) = ((page.width * 20.0).round() as i64, (page.height * 20.0).round() as i64);
        let m = page.margins;
        let size = format!(r#"<w:pgSz w:w="{w}" w:h="{h}"{}/>"#, if w > h { r#" w:orient="landscape""# } else { "" });
        let mar = format!(r#"<w:pgMar w:top="{}" w:right="{}" w:bottom="{}" w:left="{}" w:header="708" w:footer="708" w:gutter="0"/>"#,
                          (m.0 * 20.0).round(), (m.1 * 20.0).round(), (m.2 * 20.0).round(), (m.3 * 20.0).round());
        if let Some(text) = self.src.as_ref().and_then(|s| s.text(main)) {
            if let Ok(d) = roxmltree::Document::parse(&text) {
                if let Some(sect) = kid(d.root_element(), W, "body").and_then(|b| kid(b, W, "sectPr")) {
                    let xml = element_source(sect, &text);
                    let xml = Regex::new(r"<w:pgSz\b[^>]*/>").unwrap().replace_all(&xml, size.as_str()).into_owned();
                    let mut xml = Regex::new(r"<w:pgMar\b[^>]*/>").unwrap().replace_all(&xml, mar.as_str()).into_owned();
                    if !xml.contains("<w:pgSz") {
                        xml = xml.replace("</w:sectPr>", &format!("{size}{mar}</w:sectPr>"));
                    }
                    return xml;
                }
            }
        }
        let mut footer = String::new();
        if page.numbers {
            self.parts.insert("word/footer1.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:ftr xmlns:w="{W}"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#).into_bytes());
            self.overrides.insert("/word/footer1.xml".into(), "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml".into());
            let rid = self.add_rel(&format!("{REL}/footer"), "footer1.xml", false);
            footer = format!(r#"<w:footerReference w:type="default" r:id="{rid}"/>"#);
        }
        format!(r#"<w:sectPr>{footer}{size}{mar}<w:cols w:space="708"/></w:sectPr>"#)
    }

    fn footnotes_part(&mut self) -> Option<String> {
        if self.notes.is_empty() {
            return None;
        }
        let notes = std::mem::take(&mut self.notes);
        let (fntext, fnref) = (self.style("fntext"), self.style("fnref"));
        let mut body = String::new();
        for (k, n) in notes.iter().enumerate() {
            let inner = self.inline(n);
            body.push_str(&format!(r#"<w:footnote w:id="{}"><w:p><w:pPr><w:pStyle w:val="{fntext}"/></w:pPr><w:r><w:rPr><w:rStyle w:val="{fnref}"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> </w:t></w:r>{inner}</w:p></w:footnote>"#, k + 1));
        }
        let decl: String = NAMESPACES.iter().map(|(p, u)| format!(r#" xmlns:{p}="{u}""#)).collect();
        Some(format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes{decl}><w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>{body}</w:footnotes>"#))
    }

    fn build(mut self) -> Vec<u8> {
        let main = self.src.as_ref().and_then(|s| s.rels("").values().find(|r| r.typ.ends_with("/officeDocument")).map(|r| r.target.clone())).unwrap_or_else(|| "word/document.xml".into());
        self.main_dir = main.rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default();
        let body_types = ["/image", "/chart", "/hyperlink", "/oleObject", "/package", "/diagramData", "/diagramLayout", "/diagramQuickStyle",
                          "/diagramColors", "/diagramDrawing", "/control", "/aFChunk", "/video", "/audio", "/media", "/footnotes"];
        let mut kept: Vec<(String, String, String, bool)> = vec![];
        let mut keep_parts: BTreeSet<String> = BTreeSet::new();
        let mut num_part: Option<String> = None;
        let mut root_ns: Option<String> = None;
        if let Some(src) = &self.src {
            for (rid, rel) in src.rels(&main) {
                if body_types.iter().any(|t| rel.typ.ends_with(t)) {
                    continue;
                }
                let target = if rel.external { rel.target.clone() } else { opc::relative(&rel.target, &self.main_dir) };
                kept.push((rid.clone(), rel.typ.clone(), target, rel.external));
                if !rel.external {
                    keep_parts.insert(rel.target.clone());
                }
                if rel.typ.ends_with("/numbering") {
                    num_part = Some(rel.target.clone());
                }
            }
            // the original root element's namespaces (pieces kept as they were may use any of them)
            if let Some(t) = src.text(&main) {
                if let Some(start) = t.find("<w:document") {
                    let end = t[start..].find('>').map(|e| start + e).unwrap_or(start);
                    root_ns = Some(t[start + "<w:document".len()..end].trim_end_matches('/').to_string());
                }
            }
        }
        for (rid, ..) in &kept {
            self.used_rids.insert(rid.clone());
        }
        let styles_name = kept.iter().find(|r| r.1.ends_with("/styles")).map(|r| opc::join(&self.main_dir, &r.2));
        if let Some(name) = &styles_name {
            if let Some(t) = self.src.as_ref().and_then(|s| s.text(name)) {
                self.map_styles(&t);
            }
        }
        let num_text = num_part.as_ref().and_then(|n| self.src.as_ref().and_then(|s| s.text(n)));
        if let Some(t) = &num_text {
            let ids: Vec<usize> = Regex::new(r#"<w:num\s+w:numId="(\d+)""#).unwrap().captures_iter(t).filter_map(|m| m[1].parse().ok()).collect();
            self.num_base = ids.into_iter().max().unwrap_or(0);
        }
        let body = self.body();
        let footnotes = self.footnotes_part();
        // numbering: our two lists added to the original's definitions
        if !self.num_ids.is_empty() {
            let abase = num_text.as_ref().map(|t| {
                Regex::new(r#"w:abstractNumId="(\d+)""#).unwrap().captures_iter(t).filter_map(|m| m[1].parse::<usize>().ok()).max().map(|x| x + 1).unwrap_or(0)
            }).unwrap_or(0);
            let abstracts = format!(r#"<w:abstractNum w:abstractNumId="{abase}"><w:multiLevelType w:val="hybridMultilevel"/>{}</w:abstractNum><w:abstractNum w:abstractNumId="{}"><w:multiLevelType w:val="hybridMultilevel"/>{}</w:abstractNum>"#,
                                    (0..9).map(|i| lvl(i, "bullet")).collect::<String>(), abase + 1, (0..9).map(|i| lvl(i, "number")).collect::<String>());
            let nums: String = self.num_ids.iter().map(|(id, kind)| {
                let overrides = if *kind == "number" { (0..9).map(|i| format!(r#"<w:lvlOverride w:ilvl="{i}"><w:startOverride w:val="1"/></w:lvlOverride>"#)).collect::<String>() } else { String::new() };
                format!(r#"<w:num w:numId="{id}"><w:abstractNumId w:val="{}"/>{overrides}</w:num>"#, abase + if *kind == "bullet" { 0 } else { 1 })
            }).collect();
            match (&num_part, &num_text) {
                (Some(part), Some(t)) => {
                    let first_num = t.find("<w:num ").or_else(|| t.rfind("</w:numbering>")).unwrap_or(t.len());
                    let mut x = format!("{}{}{}", &t[..first_num], abstracts, &t[first_num..]);
                    x = x.replacen("</w:numbering>", &format!("{nums}</w:numbering>"), 1);
                    self.parts.insert(part.clone(), x.into_bytes());
                }
                _ => {
                    self.parts.insert("word/numbering.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="{W}">{abstracts}{nums}</w:numbering>"#).into_bytes());
                    self.overrides.insert("/word/numbering.xml".into(), "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml".into());
                    let t = opc::relative("word/numbering.xml", &self.main_dir.clone());
                    self.add_rel(&format!("{REL}/numbering"), &t, false);
                }
            }
        }
        if let Some(f) = footnotes {
            self.parts.insert("word/footnotes.xml".into(), f.into_bytes());
            self.overrides.insert("/word/footnotes.xml".into(), "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml".into());
            let t = opc::relative("word/footnotes.xml", &self.main_dir.clone());
            self.add_rel(&format!("{REL}/footnotes"), &t, false);
        }
        let sect = self.sect_pr(&main);
        let mut decl: String = NAMESPACES.iter().map(|(p, u)| format!(r#" xmlns:{p}="{u}""#)).collect();
        let mut ignorable: BTreeSet<String> = ["w14", "w15", "wp14"].iter().map(|s| s.to_string()).collect();
        if let Some(rn) = &root_ns {
            for m in Regex::new(r#"xmlns:(\w+)="([^"]*)""#).unwrap().captures_iter(rn) {
                if !NAMESPACES.iter().any(|(p, _)| *p == &m[1]) {
                    decl.push_str(&format!(r#" xmlns:{}="{}""#, &m[1], &m[2]));
                }
            }
            if let Some(m) = Regex::new(r#"mc:Ignorable="([^"]*)""#).unwrap().captures(rn) {
                for p in m[1].split_whitespace() {
                    if decl.contains(&format!("xmlns:{p}=")) {
                        ignorable.insert(p.to_string());
                    }
                }
            }
        }
        let document = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document{decl} mc:Ignorable="{}"><w:body>{body}{sect}</w:body></w:document>"#,
                               ignorable.into_iter().collect::<Vec<_>>().join(" "));
        self.parts.insert(main.clone(), document.into_bytes());
        let mut rels = kept;
        let new_rels = std::mem::take(&mut self.rels);
        rels.extend(new_rels);
        if self.src.is_none() {
            for (typ, target) in [("styles", "styles.xml"), ("settings", "settings.xml")] {
                let r = self.rid();
                rels.push((r, format!("{REL}/{typ}"), target.into(), false));
            }
        }
        let xml_rels: String = rels.iter().map(|(id, typ, target, ext)| format!(r#"<Relationship Id="{id}" Type="{typ}" Target="{}"{}/>"#, esc_attr(target), if *ext { r#" TargetMode="External""# } else { "" })).collect();
        let (d, base) = main.rsplit_once('/').unwrap_or(("", &main));
        let rels_name = if d.is_empty() { format!("_rels/{base}.rels") } else { format!("{d}/_rels/{base}.rels") };
        self.parts.insert(rels_name.clone(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{xml_rels}</Relationships>"#).into_bytes());
        if self.src.is_none() {
            self.parts.insert("word/styles.xml".into(), styles_part(self.doc.size, self.doc.font.as_deref().unwrap_or("Calibri")).into_bytes());
            self.parts.insert("word/settings.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="{W}"><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#).into_bytes());
            self.parts.insert("_rels/.rels".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#).into_bytes());
            self.parts.insert("docProps/core.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>{}</dc:creator></cp:coreProperties>"#, esc(&self.doc.title), esc(&self.doc.author)).into_bytes());
            self.parts.insert("docProps/app.xml".into(), r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Rewrite</Application></Properties>"#.as_bytes().to_vec());
        } else {
            // copy what the kept relationships need, and everything outside the document's folder
            let src = self.src.take().unwrap();
            let mut closure: BTreeSet<String> = BTreeSet::new();
            let mut todo: Vec<String> = keep_parts.iter().cloned().collect();
            while let Some(part) = todo.pop() {
                if !closure.insert(part.clone()) {
                    continue;
                }
                let (pd, pb) = part.rsplit_once('/').unwrap_or(("", &part));
                closure.insert(if pd.is_empty() { format!("_rels/{pb}.rels") } else { format!("{pd}/_rels/{pb}.rels") });
                for r in src.rels(&part).values() {
                    if !r.external {
                        todo.push(r.target.clone());
                    }
                }
            }
            for (name, data) in &src.parts {
                if *name == main || *name == rels_name || self.parts.contains_key(name) {
                    continue;
                }
                let outside = !name.starts_with(&format!("{d}/"));
                if outside || closure.contains(name) {
                    self.parts.insert(name.clone(), data.clone());
                }
            }
            if !self.missing.is_empty() {
                if let Some(name) = &styles_name {
                    if let Some(t) = self.parts.get(name).map(|x| crate::xml::decode(x)) {
                        let extra: String = self.missing.clone().iter().map(|k| style_xml(k, &self.style(k), self.doc.size)).collect();
                        self.parts.insert(name.clone(), t.replacen("</w:styles>", &format!("{extra}</w:styles>"), 1).into_bytes());
                    }
                }
            }
            self.src = Some(src);
        }
        let ct = self.content_types();
        let mut files: Vec<(String, Vec<u8>)> = vec![("[Content_Types].xml".into(), ct.into_bytes())];
        let mut names: Vec<&String> = self.parts.keys().collect();
        names.sort_by_key(|n| (!n.starts_with("_rels"), (*n).clone()));
        for n in names {
            files.push((n.clone(), self.parts[n].clone()));
        }
        opc::zip(&files, &[])
    }

    fn content_types(&self) -> String {
        let mut defaults: BTreeMap<String, String> = [
            ("rels", "application/vnd.openxmlformats-package.relationships+xml"), ("xml", "application/xml"),
            ("png", "image/png"), ("jpeg", "image/jpeg"), ("jpg", "image/jpeg"), ("gif", "image/gif"), ("bmp", "image/bmp"),
            ("tiff", "image/tiff"), ("tif", "image/tiff"), ("emf", "image/x-emf"), ("wmf", "image/x-wmf"), ("svg", "image/svg+xml"),
            ("webp", "image/webp"), ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            ("bin", "application/vnd.openxmlformats-officedocument.oleObject"),
        ].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let mut overrides: BTreeMap<String, String> = BTreeMap::new();
        if let Some(src) = &self.src {
            if let Some(t) = src.text("[Content_Types].xml") {
                if let Ok(d) = roxmltree::Document::parse(&t) {
                    for el in d.root_element().children().filter(|n| n.is_element()) {
                        match el.tag_name().name() {
                            "Default" => {
                                defaults.entry(el.attribute("Extension").unwrap_or("").to_lowercase()).or_insert_with(|| el.attribute("ContentType").unwrap_or("").to_string());
                            }
                            "Override" => {
                                let p = el.attribute("PartName").unwrap_or("");
                                if self.parts.contains_key(p.trim_start_matches('/')) {
                                    overrides.insert(p.to_string(), el.attribute("ContentType").unwrap_or("").to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else {
            for (p, t) in [("/word/document.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"),
                           ("/word/styles.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"),
                           ("/word/settings.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"),
                           ("/docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml"),
                           ("/docProps/app.xml", "application/vnd.openxmlformats-officedocument.extended-properties+xml")] {
                overrides.insert(p.into(), t.into());
            }
        }
        for (k, v) in &self.overrides {
            overrides.insert(k.clone(), v.clone());
        }
        for name in self.parts.keys() {
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if !defaults.contains_key(&ext) && !overrides.contains_key(&format!("/{name}")) {
                defaults.insert(ext.clone(), imaging::mime(&imaging::ext_for(&ext)).into());
            }
        }
        let d: String = defaults.iter().map(|(e, t)| format!(r#"<Default Extension="{e}" ContentType="{t}"/>"#)).collect();
        let o: String = overrides.iter().map(|(p, t)| format!(r#"<Override PartName="{p}" ContentType="{t}"/>"#)).collect();
        format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">{d}{o}</Types>"#)
    }
}

/// .docx bytes for a Document.
pub fn write(doc: &Document) -> Vec<u8> {
    let src = doc.source.get("docx").and_then(|b| Package::open(b).ok());
    let styles = OUR_STYLES.iter().map(|(k, sid, _)| (*k, sid.to_string())).collect();
    let w = Writer {
        doc,
        src,
        main_dir: "word".into(),
        parts: BTreeMap::new(),
        overrides: BTreeMap::new(),
        rels: vec![],
        used_rids: BTreeSet::new(),
        next_rid: 1,
        media_n: 0,
        chart_n: 0,
        ids: 0,
        notes: vec![],
        styles,
        missing: vec![],
        num_ids: vec![],
        num_base: 0,
    };
    w.build()
}
