//! Changes to an existing PDF: text, boxes, lines and pictures put on a page; text taken out of it;
//! highlights, notes and drawings as annotations; pages turned, removed, moved, added, and other PDFs
//! put in.
//!
//! Positions are in points from the top-left corner of the page as it is stored (before /Rotate), the
//! corner of its crop box. Drawing goes through the PDF writer (pdf.rs): what is to be added is drawn on
//! a page of its own, and that page is put on the target page as a form XObject. The page's own content
//! is wrapped in q ... Q once, so whatever state it leaves cannot shift what comes after it.

use crate::fonts;
use crate::pdf::{self, num};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, StringFormat};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn serif() -> String {
    "serif".into()
}
fn black() -> String {
    "#000000".into()
}
fn yellow() -> String {
    "#FFE14D".into()
}
fn one() -> f64 {
    1.0
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Text whose first line's top is at (x, y); lines split at '\n'.
    Text {
        page: usize,
        x: f64,
        y: f64,
        text: String,
        size: f64,
        #[serde(default)]
        font: Option<String>,
        #[serde(default = "serif")]
        generic: String,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default = "black")]
        color: String,
        /// Paint this colour under the text first (to cover what was there).
        #[serde(default)]
        background: Option<String>,
    },
    Rect {
        page: usize,
        x0: f64,
        top: f64,
        x1: f64,
        bottom: f64,
        #[serde(default)]
        fill: Option<String>,
        #[serde(default)]
        stroke: Option<String>,
        #[serde(default = "one")]
        width: f64,
    },
    Image {
        page: usize,
        x0: f64,
        top: f64,
        x1: f64,
        bottom: f64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Lines drawn into the page itself.
    Lines {
        page: usize,
        paths: Vec<Vec<(f64, f64)>>,
        #[serde(default = "black")]
        color: String,
        #[serde(default = "one")]
        width: f64,
    },
    /// Take out the text that starts inside the box.
    EraseText { page: usize, x0: f64, top: f64, x1: f64, bottom: f64 },
    /// A highlight, underline or strike-out over some text (one box per line).
    Mark {
        page: usize,
        kind: String,
        quads: Vec<[f64; 4]>,
        #[serde(default = "yellow")]
        color: String,
        #[serde(default)]
        note: String,
    },
    /// A sticky note.
    Note {
        page: usize,
        x: f64,
        y: f64,
        text: String,
        #[serde(default = "yellow")]
        color: String,
    },
    /// A freehand drawing, as an annotation.
    Ink {
        page: usize,
        paths: Vec<Vec<(f64, f64)>>,
        #[serde(default = "black")]
        color: String,
        #[serde(default = "one")]
        width: f64,
    },
    /// Remove the annotations (marks, notes, drawings) that touch the box.
    DeleteAnnots { page: usize, x0: f64, top: f64, x1: f64, bottom: f64 },
    /// Turn a page by a multiple of 90 degrees (clockwise).
    Rotate { page: usize, angle: i64 },
    DeletePage { page: usize },
    MovePage { page: usize, to: usize },
    InsertBlank { at: usize, width: f64, height: f64 },
    /// Put all pages of another PDF in at a position (the end when not given).
    Append {
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        #[serde(default)]
        at: Option<usize>,
    },
}

/// A PDF with the changes made, in order.
pub fn apply(data: &[u8], ops: &[Op]) -> Res<Vec<u8>> {
    let mut doc = open(data)?;
    for op in ops {
        apply_one(&mut doc, op)?;
    }
    save(&mut doc)
}

pub fn open(data: &[u8]) -> Res<Pdf> {
    let mut doc = Pdf::load_mem(data).map_err(|e| format!("The PDF could not be read for editing: {e}"))?;
    if doc.is_encrypted() {
        doc.decrypt("").map_err(|_| "This PDF is protected with a password; it cannot be edited here.".to_string())?;
    }
    if doc.trailer.has(b"Encrypt") && !doc.is_encrypted() {
        doc.trailer.remove(b"Encrypt");
    }
    Ok(doc)
}

pub fn save(doc: &mut Pdf) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(err)?;
    Ok(out)
}

fn page_id(doc: &Pdf, page: usize) -> Res<ObjectId> {
    doc.get_pages().get(&(page as u32 + 1)).copied().ok_or_else(|| format!("There is no page {}.", page + 1))
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn resolve<'a>(doc: &'a Pdf, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

/// A page attribute, from the page or the page-tree node it inherits from.
fn inherited(doc: &Pdf, id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(c) = cur {
        let d = doc.get_dictionary(c).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(resolve(doc, v).clone());
        }
        cur = d.get(b"Parent").and_then(Object::as_reference).ok();
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    None
}

/// The page's visible box: left, bottom, right, top (PDF units).
fn page_box(doc: &Pdf, id: ObjectId) -> [f64; 4] {
    let get = |key: &[u8]| -> Option<[f64; 4]> {
        let o = inherited(doc, id, key)?;
        let a = o.as_array().ok()?;
        let v: Vec<f64> = a.iter().filter_map(|x| number(resolve(doc, x))).collect();
        (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    };
    get(b"CropBox").or_else(|| get(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0])
}

fn rotation(doc: &Pdf, id: ObjectId) -> i64 {
    inherited(doc, id, b"Rotate").and_then(|o| number(&o)).map(|r| r as i64).unwrap_or(0).rem_euclid(360)
}

/// Copy inherited resources and boxes into the page, so that changing them changes only this page and
/// the page can move in the tree.
fn materialize(doc: &mut Pdf, id: ObjectId) {
    for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
        let has = doc.get_dictionary(id).map(|d| d.has(key)).unwrap_or(true);
        if !has {
            if let Some(v) = inherited(doc, id, key) {
                if let Ok(d) = doc.get_dictionary_mut(id) {
                    d.set(key.to_vec(), v);
                }
            }
        }
    }
    let has_res = doc.get_dictionary(id).map(|d| d.has(b"Resources")).unwrap_or(true);
    if !has_res {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.set("Resources", Dictionary::new());
        }
    }
}

fn apply_one(doc: &mut Pdf, op: &Op) -> Res<()> {
    match op {
        Op::Text { page, x, y, text, size, font, generic, bold, italic, color, background } => {
            let (font, fake_b, fake_i) = fonts::resolve(font.as_deref(), *bold, *italic, generic)
                .ok_or("No font is available to write with.")?;
            let fallbacks = fonts::fallbacks();
            let size = size.max(1.0);
            let lines: Vec<&str> = text.split('\n').collect();
            let lh = size * 1.2;
            let widths: Vec<f64> = lines.iter().map(|l| width_of(&font, &fallbacks, l, size)).collect();
            let (x, y) = (*x, *y);
            draw(doc, *page, |w, p| {
                if let Some(bg) = background {
                    let wmax = widths.iter().cloned().fold(0.0, f64::max);
                    w.rect(p, x - 1.0, y - 1.0, wmax + 2.0, lh * lines.len() as f64 + 2.0, Some(bg), None, 0.0);
                }
                for (k, line) in lines.iter().enumerate() {
                    let base = y + size * 0.95 + lh * k as f64;
                    let mut cx = x;
                    for (f, run) in runs(&font, &fallbacks, line) {
                        w.text(p, cx, base, &f, size, &run, color, 0.0, fake_b && Arc::ptr_eq(&f, &font),
                               fake_i && Arc::ptr_eq(&f, &font), 0.0);
                        cx += f.width(&run, size);
                    }
                }
            })
        }
        Op::Rect { page, x0, top, x1, bottom, fill, stroke, width } => {
            let (x, y, w_, h) = (x0.min(*x1), top.min(*bottom), (x1 - x0).abs(), (bottom - top).abs());
            draw(doc, *page, |w, p| w.rect(p, x, y, w_, h, fill.as_deref(), stroke.as_deref(), *width))
        }
        Op::Image { page, x0, top, x1, bottom, data } => {
            let (x, y, w_, h) = (x0.min(*x1), top.min(*bottom), (x1 - x0).abs(), (bottom - top).abs());
            let data = data.clone();
            let mut ok = true;
            draw(doc, *page, |w, p| match w.image_bytes(&data).cloned() {
                Some(img) => w.image(p, &img, x, y, w_, h),
                None => ok = false,
            })?;
            if ok {
                Ok(())
            } else {
                Err("The picture could not be read.".into())
            }
        }
        Op::Lines { page, paths, color, width } => draw(doc, *page, |w, p| {
            for path in paths {
                w.polyline(p, path, color, *width);
            }
        }),
        Op::EraseText { page, x0, top, x1, bottom } => {
            let id = page_id(doc, *page)?;
            let b = page_box(doc, id);
            let pad = ((bottom - top).abs() * 0.25).max(1.0);
            let rect = [b[0] + x0.min(*x1) - 1.0, b[3] - top.max(*bottom) - pad, b[0] + x0.max(*x1) + 1.0, b[3] - top.min(*bottom) + pad];
            erase_text(doc, id, rect).map(|_| ())
        }
        Op::Mark { page, kind, quads, color, note } => mark(doc, *page, kind, quads, color, note),
        Op::Note { page, x, y, text, color } => {
            let id = page_id(doc, *page)?;
            let b = page_box(doc, id);
            let (px, py) = (b[0] + x, b[3] - y);
            let rect = [px, py - 20.0, px + 20.0, py];
            let mut d = annot_base("Text", rect, color);
            d.set("Contents", text_string(text));
            d.set("Name", "Comment");
            d.set("Open", false);
            let ap = appearance(doc, rect, &format!("{} 0.5 G 0.6 w 1 1 18 18 re B 0 G 4 13 m 16 13 l 4 9 m 13 9 l S", fill(color)), None);
            d.set("AP", Dictionary::from_iter(vec![("N", Object::Reference(ap))]));
            add_annot(doc, id, d)
        }
        Op::Ink { page, paths, color, width } => {
            let id = page_id(doc, *page)?;
            let b = page_box(doc, id);
            let pts: Vec<Vec<(f64, f64)>> = paths.iter().map(|p| p.iter().map(|(x, y)| (b[0] + x, b[3] - y)).collect()).collect();
            let all: Vec<&(f64, f64)> = pts.iter().flatten().collect();
            if all.is_empty() {
                return Ok(());
            }
            let pad = width + 2.0;
            let rect = [all.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - pad, all.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - pad,
                        all.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + pad, all.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + pad];
            let mut d = annot_base("Ink", rect, color);
            d.set("InkList", pts.iter().map(|p| Object::Array(p.iter().flat_map(|(x, y)| [real(*x), real(*y)]).collect())).collect::<Vec<_>>());
            d.set("BS", Dictionary::from_iter(vec![("W", real(*width))]));
            let mut s = format!("{} {} w 1 J 1 j ", stroke(color), num(*width));
            for p in &pts {
                for (k, (x, y)) in p.iter().enumerate() {
                    s.push_str(&format!("{} {} {} ", num(x - rect[0]), num(y - rect[1]), if k == 0 { "m" } else { "l" }));
                }
                s.push_str("S ");
            }
            let ap = appearance(doc, rect, &s, None);
            d.set("AP", Dictionary::from_iter(vec![("N", Object::Reference(ap))]));
            add_annot(doc, id, d)
        }
        Op::DeleteAnnots { page, x0, top, x1, bottom } => {
            let id = page_id(doc, *page)?;
            let b = page_box(doc, id);
            let area = [b[0] + x0.min(*x1), b[3] - top.max(*bottom), b[0] + x0.max(*x1), b[3] - top.min(*bottom)];
            delete_annots(doc, id, area)
        }
        Op::Rotate { page, angle } => {
            let id = page_id(doc, *page)?;
            let r = (rotation(doc, id) + angle).rem_euclid(360) / 90 * 90;
            doc.get_dictionary_mut(id).map_err(err)?.set("Rotate", r);
            Ok(())
        }
        Op::DeletePage { page } => {
            if doc.get_pages().len() <= 1 {
                return Err("A PDF needs at least one page.".into());
            }
            page_id(doc, *page)?;
            doc.delete_pages(&[*page as u32 + 1]);
            Ok(())
        }
        Op::MovePage { page, to } => {
            let (root, mut pages) = flatten_tree(doc)?;
            if *page >= pages.len() {
                return Err(format!("There is no page {}.", page + 1));
            }
            let p = pages.remove(*page);
            pages.insert((*to).min(pages.len()), p);
            set_kids(doc, root, &pages)
        }
        Op::InsertBlank { at, width, height } => {
            let (root, mut pages) = flatten_tree(doc)?;
            let content = doc.add_object(Stream::new(Dictionary::new(), vec![]));
            let mut d = Dictionary::new();
            d.set("Type", "Page");
            d.set("Parent", root);
            d.set("MediaBox", vec![0.into(), 0.into(), real(*width), real(*height)]);
            d.set("Resources", Dictionary::new());
            d.set("Contents", content);
            let id = doc.add_object(d);
            pages.insert((*at).min(pages.len()), id);
            set_kids(doc, root, &pages)
        }
        Op::Append { data, at } => append(doc, data, *at),
    }
}

/// Draw on a page through the PDF writer, then put the drawing on the page.
fn draw(doc: &mut Pdf, page: usize, f: impl FnOnce(&mut pdf::Writer, usize)) -> Res<()> {
    let id = page_id(doc, page)?;
    let b = page_box(doc, id);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let mut wr = pdf::Writer::new("", "");
    let p = wr.new_page(w, h);
    f(&mut wr, p);
    let bytes = wr.finish();
    graft(doc, id, &bytes, b)
}

fn graft(doc: &mut Pdf, page: ObjectId, bytes: &[u8], b: [f64; 4]) -> Res<()> {
    let mut small = Pdf::load_mem(bytes).map_err(err)?;
    small.renumber_objects_with(doc.max_id + 1);
    let opage = *small.get_pages().get(&1).ok_or("The drawing has no page.")?;
    let content = small.get_page_content(opage);
    let resources = small.get_dictionary(opage).map_err(err)?.get(b"Resources").ok().cloned()
        .unwrap_or_else(|| Object::Dictionary(Dictionary::new()));
    let mut skip: HashSet<ObjectId> = small.get_page_contents(opage).into_iter().collect();
    skip.insert(opage);
    if let Ok(root) = small.trailer.get(b"Root").and_then(Object::as_reference) {
        skip.insert(root);
        if let Ok(pages) = small.get_dictionary(root).and_then(|d| d.get(b"Pages")).and_then(Object::as_reference) {
            skip.insert(pages);
        }
    }
    if let Ok(info) = small.trailer.get(b"Info").and_then(Object::as_reference) {
        skip.insert(info);
    }
    for (id, obj) in std::mem::take(&mut small.objects) {
        if !skip.contains(&id) {
            doc.objects.insert(id, obj);
        }
    }
    doc.max_id = doc.max_id.max(small.max_id);
    let mut fd = Dictionary::new();
    fd.set("Type", "XObject");
    fd.set("Subtype", "Form");
    fd.set("BBox", vec![0.into(), 0.into(), real(b[2] - b[0]), real(b[3] - b[1])]);
    fd.set("Resources", resources);
    let mut st = Stream::new(fd, content);
    let _ = st.compress();
    let form = doc.add_object(st);
    let name = format!("RwEdit{}", form.0);
    materialize(doc, page);
    doc.add_xobject(page, name.as_bytes(), form).map_err(err)?;
    wrap_once(doc, page)?;
    doc.add_page_contents(page, format!("q 1 0 0 1 {} {} cm /{} Do Q\n", num(b[0]), num(b[1]), name).into_bytes())
        .map_err(err)
}

const OPEN: &[u8] = b"q % rewrite\n";

fn wrap_once(doc: &mut Pdf, page: ObjectId) -> Res<()> {
    let contents = doc.get_page_contents(page);
    if let Some(first) = contents.first() {
        if let Ok(Object::Stream(s)) = doc.get_object(*first) {
            if s.decompressed_content().map(|c| c == OPEN).unwrap_or(s.content == OPEN) {
                return Ok(());
            }
        }
    }
    let open = doc.add_object(Stream::new(Dictionary::new(), OPEN.to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
    let mut arr: Vec<Object> = vec![open.into()];
    arr.extend(contents.iter().map(|id| Object::Reference(*id)));
    arr.push(close.into());
    doc.get_dictionary_mut(page).map_err(err)?.set("Contents", arr);
    Ok(())
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

fn width_of(font: &Arc<fonts::Font>, fallbacks: &[Arc<fonts::Font>], text: &str, size: f64) -> f64 {
    runs(font, fallbacks, text).iter().map(|(f, r)| f.width(r, size)).sum()
}

/// The text in pieces, each with a font that has its characters.
fn runs(font: &Arc<fonts::Font>, fallbacks: &[Arc<fonts::Font>], text: &str) -> Vec<(Arc<fonts::Font>, String)> {
    let mut out: Vec<(Arc<fonts::Font>, String)> = vec![];
    for ch in text.chars() {
        let f = if font.has(ch) || ch == ' ' {
            font.clone()
        } else {
            fallbacks.iter().find(|f| f.has(ch)).cloned().unwrap_or_else(|| font.clone())
        };
        match out.last_mut() {
            Some((lf, s)) if Arc::ptr_eq(lf, &f) => s.push(ch),
            _ => out.push((f, ch.to_string())),
        }
    }
    out
}

fn rgb(color: &str) -> (f64, f64, f64) {
    let c = color.trim_start_matches('#');
    let v = |i: usize| u8::from_str_radix(c.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f64 / 255.0;
    if c.len() >= 6 {
        (v(0), v(2), v(4))
    } else {
        (0.0, 0.0, 0.0)
    }
}

fn fill(color: &str) -> String {
    let (r, g, b) = rgb(color);
    format!("{} {} {} rg", num(r), num(g), num(b))
}

fn stroke(color: &str) -> String {
    let (r, g, b) = rgb(color);
    format!("{} {} {} RG", num(r), num(g), num(b))
}

fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut v = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(v, StringFormat::Hexadecimal)
    }
}

fn annot_base(subtype: &str, rect: [f64; 4], color: &str) -> Dictionary {
    let (r, g, b) = rgb(color);
    let mut d = Dictionary::new();
    d.set("Type", "Annot");
    d.set("Subtype", subtype);
    d.set("Rect", rect.iter().map(|v| real(*v)).collect::<Vec<_>>());
    d.set("C", vec![real(r), real(g), real(b)]);
    d.set("F", 4);
    d.set("T", text_string("Rewrite"));
    d
}

/// An appearance stream (form) for an annotation, drawn in the annotation's own box.
fn appearance(doc: &mut Pdf, rect: [f64; 4], ops: &str, multiply: Option<f64>) -> ObjectId {
    let mut fd = Dictionary::new();
    fd.set("Type", "XObject");
    fd.set("Subtype", "Form");
    fd.set("BBox", vec![0.into(), 0.into(), real(rect[2] - rect[0]), real(rect[3] - rect[1])]);
    let mut content = String::new();
    if let Some(alpha) = multiply {
        let mut gs = Dictionary::new();
        gs.set("Type", "ExtGState");
        gs.set("BM", "Multiply");
        gs.set("ca", real(alpha));
        gs.set("CA", real(alpha));
        let mut ext = Dictionary::new();
        ext.set("GS0", gs);
        let mut res = Dictionary::new();
        res.set("ExtGState", ext);
        fd.set("Resources", res);
        content.push_str("/GS0 gs ");
    }
    content.push_str(ops);
    doc.add_object(Stream::new(fd, content.into_bytes()))
}

fn add_annot(doc: &mut Pdf, page: ObjectId, mut d: Dictionary) -> Res<()> {
    d.set("P", page);
    let id = doc.add_object(d);
    let annots = doc.get_dictionary(page).map_err(err)?.get(b"Annots").ok().cloned();
    match annots {
        Some(Object::Reference(aid)) => {
            if let Ok(Object::Array(a)) = doc.get_object_mut(aid) {
                a.push(id.into());
                return Ok(());
            }
            doc.get_dictionary_mut(page).map_err(err)?.set("Annots", vec![Object::Reference(id)]);
        }
        Some(Object::Array(mut a)) => {
            a.push(id.into());
            doc.get_dictionary_mut(page).map_err(err)?.set("Annots", a);
        }
        _ => {
            doc.get_dictionary_mut(page).map_err(err)?.set("Annots", vec![Object::Reference(id)]);
        }
    }
    Ok(())
}

fn mark(doc: &mut Pdf, page: usize, kind: &str, quads: &[[f64; 4]], color: &str, note: &str) -> Res<()> {
    let id = page_id(doc, page)?;
    let b = page_box(doc, id);
    if quads.is_empty() {
        return Ok(());
    }
    // each box (x0, top, x1, bottom) in PDF units: left, bottom, right, top
    let boxes: Vec<[f64; 4]> = quads.iter().map(|q| [b[0] + q[0].min(q[2]), b[3] - q[1].max(q[3]), b[0] + q[0].max(q[2]), b[3] - q[1].min(q[3])]).collect();
    let rect = [boxes.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min) - 1.0, boxes.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min) - 1.0,
                boxes.iter().map(|q| q[2]).fold(f64::NEG_INFINITY, f64::max) + 1.0, boxes.iter().map(|q| q[3]).fold(f64::NEG_INFINITY, f64::max) + 1.0];
    let subtype = match kind {
        "underline" => "Underline",
        "strikeout" | "strike" => "StrikeOut",
        _ => "Highlight",
    };
    let mut d = annot_base(subtype, rect, color);
    let mut qp = vec![];
    for q in &boxes {
        for v in [q[0], q[3], q[2], q[3], q[0], q[1], q[2], q[1]] {
            qp.push(real(v));
        }
    }
    d.set("QuadPoints", qp);
    if !note.is_empty() {
        d.set("Contents", text_string(note));
    }
    let mut ops = String::new();
    let rel = |q: &[f64; 4]| (q[0] - rect[0], q[1] - rect[1], q[2] - rect[0], q[3] - rect[1]);
    let ap = match subtype {
        "Highlight" => {
            ops.push_str(&fill(color));
            for q in &boxes {
                let (x0, y0, x1, y1) = rel(q);
                ops.push_str(&format!(" {} {} {} {} re", num(x0), num(y0), num(x1 - x0), num(y1 - y0)));
            }
            ops.push_str(" f");
            appearance(doc, rect, &ops, Some(1.0))
        }
        _ => {
            ops.push_str(&stroke(color));
            for q in &boxes {
                let (x0, y0, x1, y1) = rel(q);
                let h = y1 - y0;
                let y = if subtype == "Underline" { y0 + h * 0.08 } else { y0 + h * 0.45 };
                ops.push_str(&format!(" {} w {} {} m {} {} l S", num((h * 0.07).max(0.6)), num(x0), num(y), num(x1), num(y)));
            }
            appearance(doc, rect, &ops, None)
        }
    };
    d.set("AP", Dictionary::from_iter(vec![("N", Object::Reference(ap))]));
    add_annot(doc, id, d)
}

fn delete_annots(doc: &mut Pdf, page: ObjectId, area: [f64; 4]) -> Res<()> {
    let annots = match doc.get_dictionary(page).map_err(err)?.get(b"Annots") {
        Ok(o) => resolve(doc, o).as_array().cloned().unwrap_or_default(),
        Err(_) => return Ok(()),
    };
    let mut keep = vec![];
    for a in annots {
        let d = match resolve(doc, &a).as_dict() {
            Ok(d) => d,
            Err(_) => {
                keep.push(a);
                continue;
            }
        };
        let sub = d.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
        let ours = matches!(sub, b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly" | b"Text" | b"Ink" | b"FreeText" | b"Square" | b"Circle" | b"Line");
        let r: Vec<f64> = d.get(b"Rect").and_then(Object::as_array).map(|a| a.iter().filter_map(|x| number(resolve(doc, x))).collect()).unwrap_or_default();
        let hit = r.len() == 4 && r[0].min(r[2]) <= area[2] && r[0].max(r[2]) >= area[0] && r[1].min(r[3]) <= area[3] && r[1].max(r[3]) >= area[1];
        if !(ours && hit) {
            keep.push(a);
        }
    }
    doc.get_dictionary_mut(page).map_err(err)?.set("Annots", keep);
    Ok(())
}

/// The page tree as one level under the root node: (root, pages in order).
fn flatten_tree(doc: &mut Pdf) -> Res<(ObjectId, Vec<ObjectId>)> {
    let root = doc.catalog().map_err(err)?.get(b"Pages").and_then(Object::as_reference).map_err(err)?;
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for &p in &pages {
        materialize(doc, p);
    }
    Ok((root, pages))
}

fn set_kids(doc: &mut Pdf, root: ObjectId, pages: &[ObjectId]) -> Res<()> {
    for &p in pages {
        if let Ok(d) = doc.get_dictionary_mut(p) {
            d.set("Parent", root);
        }
    }
    let d = doc.get_dictionary_mut(root).map_err(err)?;
    d.set("Kids", pages.iter().map(|p| Object::Reference(*p)).collect::<Vec<_>>());
    d.set("Count", pages.len() as i64);
    Ok(())
}

fn append(doc: &mut Pdf, data: &[u8], at: Option<usize>) -> Res<()> {
    let mut other = open(data)?;
    other.renumber_objects_with(doc.max_id + 1);
    let (_, other_pages) = flatten_tree(&mut other)?;
    let mut skip: HashSet<ObjectId> = HashSet::new();
    if let Ok(root) = other.trailer.get(b"Root").and_then(Object::as_reference) {
        skip.insert(root);
        if let Ok(pages) = other.get_dictionary(root).and_then(|d| d.get(b"Pages")).and_then(Object::as_reference) {
            skip.insert(pages);
        }
        // outlines and names point into the other document's tree; leave them behind
        if let Ok(d) = other.get_dictionary(root) {
            for key in [&b"Outlines"[..], b"Names", b"AcroForm", b"StructTreeRoot"] {
                if let Ok(r) = d.get(key).and_then(Object::as_reference) {
                    skip.insert(r);
                }
            }
        }
    }
    if let Ok(info) = other.trailer.get(b"Info").and_then(Object::as_reference) {
        skip.insert(info);
    }
    for (id, obj) in std::mem::take(&mut other.objects) {
        if !skip.contains(&id) {
            doc.objects.insert(id, obj);
        }
    }
    doc.max_id = doc.max_id.max(other.max_id);
    let (root, mut pages) = flatten_tree(doc)?;
    let at = at.unwrap_or(pages.len()).min(pages.len());
    for (k, p) in other_pages.into_iter().enumerate() {
        if let Ok(d) = doc.get_dictionary_mut(p) {
            d.remove(b"StructParents");
        }
        pages.insert(at + k, p);
    }
    set_kids(doc, root, &pages)
}

// -- taking text out ----------------------------------------------------------------------------------

type M = [f64; 6];

fn mul(a: &M, b: &M) -> M {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3],
     a[4] * b[0] + a[5] * b[2] + b[4], a[4] * b[1] + a[5] * b[3] + b[5]]
}

const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn operand(o: &Object) -> f64 {
    number(o).unwrap_or(0.0)
}

/// Remove the text-showing operators whose text starts inside rect (PDF units: left, bottom, right,
/// top). Positions after an operator are estimated from its length, which is enough to tell whether the
/// next piece of the same line still starts in the box. Returns how many were removed.
fn erase_text(doc: &mut Pdf, page: ObjectId, rect: [f64; 4]) -> Res<usize> {
    let content = doc.get_page_content(page);
    let c = Content::decode(&content).map_err(err)?;
    let two_byte: HashSet<Vec<u8>> = doc.get_page_fonts(page).unwrap_or_default().into_iter()
        .filter(|(_, d)| d.get(b"Subtype").and_then(Object::as_name).map(|n| n == b"Type0").unwrap_or(false))
        .map(|(k, _)| k).collect();
    let mut stack: Vec<M> = vec![];
    let mut ctm = ID;
    let (mut tm, mut tlm) = (ID, ID);
    let (mut size, mut leading, mut tc, mut tw, mut tz, mut rise) = (12.0, 0.0, 0.0, 0.0, 1.0, 0.0);
    let mut wide = false;
    let mut kept: Vec<Operation> = Vec::with_capacity(c.operations.len());
    let mut removed = 0;
    for op in c.operations {
        let o = &op.operands;
        let mut drop = false;
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(ID),
            "cm" if o.len() == 6 => {
                let m = [operand(&o[0]), operand(&o[1]), operand(&o[2]), operand(&o[3]), operand(&o[4]), operand(&o[5])];
                ctm = mul(&m, &ctm);
            }
            "BT" => {
                tm = ID;
                tlm = ID;
            }
            "Tf" if o.len() == 2 => {
                size = operand(&o[1]);
                wide = o[0].as_name().map(|n| two_byte.contains(n)).unwrap_or(false);
            }
            "TL" if !o.is_empty() => leading = operand(&o[0]),
            "Tc" if !o.is_empty() => tc = operand(&o[0]),
            "Tw" if !o.is_empty() => tw = operand(&o[0]),
            "Tz" if !o.is_empty() => tz = operand(&o[0]) / 100.0,
            "Ts" if !o.is_empty() => rise = operand(&o[0]),
            "Td" | "TD" if o.len() == 2 => {
                let (tx, ty) = (operand(&o[0]), operand(&o[1]));
                if op.operator == "TD" {
                    leading = -ty;
                }
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, tx, ty], &tlm);
                tm = tlm;
            }
            "Tm" if o.len() == 6 => {
                tlm = [operand(&o[0]), operand(&o[1]), operand(&o[2]), operand(&o[3]), operand(&o[4]), operand(&o[5])];
                tm = tlm;
            }
            "T*" => {
                tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -leading], &tlm);
                tm = tlm;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if op.operator == "'" || op.operator == "\"" {
                    if op.operator == "\"" && o.len() == 3 {
                        tw = operand(&o[0]);
                        tc = operand(&o[1]);
                    }
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -leading], &tlm);
                    tm = tlm;
                }
                let at = mul(&mul(&[1.0, 0.0, 0.0, 1.0, 0.0, rise], &tm), &ctm);
                let (px, py) = (at[4], at[5]);
                drop = px >= rect[0] && px <= rect[2] && py >= rect[1] && py <= rect[3];
                // move on by an estimate of the shown text's width
                let show = |bytes: &[u8]| {
                    let n = if wide { bytes.len() / 2 } else { bytes.len() } as f64;
                    let spaces = if wide { 0.0 } else { bytes.iter().filter(|b| **b == b' ').count() as f64 };
                    (n * 0.5 * size + n * tc + spaces * tw) * tz
                };
                let adv: f64 = match o.last() {
                    Some(Object::String(s, _)) => show(s),
                    Some(Object::Array(a)) => a.iter().map(|x| match x {
                        Object::String(s, _) => show(s),
                        other => -operand(other) / 1000.0 * size * tz,
                    }).sum(),
                    _ => 0.0,
                };
                tm = mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], &tm);
            }
            _ => {}
        }
        if drop {
            removed += 1;
        } else {
            kept.push(op);
        }
    }
    if removed > 0 {
        let bytes = Content { operations: kept }.encode().map_err(err)?;
        let contents = doc.get_page_contents(page);
        let s = doc.add_object(Stream::new(Dictionary::new(), bytes));
        if let Ok(Object::Stream(st)) = doc.get_object_mut(s) {
            let _ = st.compress();
        }
        let _ = contents;
        doc.get_dictionary_mut(page).map_err(err)?.set("Contents", s);
    }
    Ok(removed)
}

/// Every page's visible box (left, bottom, right, top, in PDF units) and turn (degrees clockwise).
pub fn pages(data: &[u8]) -> Res<Vec<(f64, f64, f64, f64, i64)>> {
    let doc = open(data)?;
    Ok(doc.get_pages().values().map(|&id| {
        let b = page_box(&doc, id);
        (b[0], b[1], b[2], b[3], rotation(&doc, id))
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = pdf::Writer::new("t", "");
        let f = fonts::resolve(None, false, false, "serif").unwrap().0;
        for k in 0..3 {
            let p = w.new_page(595.0, 842.0);
            w.text(p, 72.0, 100.0, &f, 12.0, &format!("Page {} text to change", k + 1), "#000000", 0.0, false, false, 0.0);
        }
        w.finish()
    }

    #[test]
    fn edits() {
        let data = sample();
        let out = apply(&data, &[
            Op::EraseText { page: 0, x0: 70.0, top: 88.0, x1: 300.0, bottom: 104.0 },
            Op::Text { page: 0, x: 72.0, y: 88.0, text: "New words".into(), size: 12.0, font: None, generic: "sans".into(), bold: true, italic: false, color: "#C00000".into(), background: None },
            Op::Rect { page: 1, x0: 70.0, top: 300.0, x1: 200.0, bottom: 340.0, fill: Some("#FFFFFF".into()), stroke: Some("#0000FF".into()), width: 1.0 },
            Op::Mark { page: 1, kind: "highlight".into(), quads: vec![[72.0, 90.0, 200.0, 104.0]], color: "#FFE14D".into(), note: String::new() },
            Op::Note { page: 2, x: 300.0, y: 300.0, text: "A note".into(), color: "#FFE14D".into() },
            Op::Ink { page: 2, paths: vec![vec![(100.0, 400.0), (150.0, 420.0), (200.0, 400.0)]], color: "#0000FF".into(), width: 2.0 },
            Op::Rotate { page: 2, angle: 90 },
            Op::MovePage { page: 2, to: 0 },
            Op::InsertBlank { at: 1, width: 300.0, height: 400.0 },
            Op::Append { data: sample(), at: None },
            Op::DeletePage { page: 4 },
        ]).unwrap();
        let p = pages(&out).unwrap();
        assert_eq!(p.len(), 6);
        assert_eq!(p[0].4, 90);
        assert_eq!((p[1].2 - p[1].0, p[1].3 - p[1].1), (300.0, 400.0));
    }
}
