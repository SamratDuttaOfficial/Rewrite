//! Reading PDF pages for conversion: every character with its box and font, the pictures and the
//! drawings, in the form `rewrite_core::pdfextract` takes; it turns them into paragraphs, headings, lists and
//! so on, and asks for parts of pages (charts, equations) to be drawn as pictures.
//!
//! Each number is computed as pdfdoc.py did with pypdfium2, so conversions match the Python version.

use crate::ffi::{self, Api, Obj};
use crate::{lock, render, Char, Document, Error, PageRef, Result, TextRef};
use image::{DynamicImage, RgbImage};
use regex::Regex;
use rewrite_core::model;
use rewrite_core::pdfextract::{self, Options, PChar, PImage, PPage, PShape};
use rewrite_core::textclean::Helper;
use std::collections::HashMap;
use std::ffi::{c_int, c_uint, c_ulong};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// A Document from PDF bytes (`options.images` also decides whether pictures are read).
pub fn to_document(data: &[u8], options: &Options, helper: &dyn Helper, progress: Option<&dyn Fn(usize, usize)>,
                   cancel: Option<&AtomicBool>) -> Result<model::Document> {
    let doc = Document::open(data)?;
    let pages = read_pages(&doc, options.images, progress, cancel)?;
    Ok(pdfextract::convert(&pages, options, helper, &|k, x0, top, x1, bottom| doc.render_area(k, x0, top, x1, bottom)))
}

/// The first page of a PDF as PNG (LaTeX figures that are PDF files); None when it cannot be drawn.
pub fn first_page_png(data: &[u8], scale: f64) -> Option<Vec<u8>> {
    let doc = Document::open(data).ok()?;
    let api = lock().ok()?;
    let page = PageRef::load(&api, doc.raw, 0).ok()?;
    let (w, h, bgr) = render(&api, page.raw, scale, [0.0; 4], ffi::BITMAP_BGR, ffi::RENDER_ANNOT).ok()?;
    Some(png_from_bgr(&bgr, w, h))
}

pub(crate) fn read_pages(doc: &Document, images: bool, progress: Option<&dyn Fn(usize, usize)>, cancel: Option<&AtomicBool>) -> Result<Vec<PPage>> {
    let n = doc.page_count();
    let mut pages = Vec::with_capacity(n);
    for k in 0..n {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Error::Cancelled);
        }
        if let Some(p) = progress {
            p(k, n);
        }
        let api = lock()?;
        let page = PageRef::load(&api, doc.raw, k)?;
        let (w, h) = page.size();
        let chars = chars(&api, TextRef::load(&api, page.raw)?.raw, h);
        let (found, shapes) = objects(&api, page.raw, w, h);
        let pics = if images { found.iter().filter_map(|f| picture(&api, doc.raw, page.raw, f)).collect() } else { vec![] };
        pages.push(PPage { width: w, height: h, chars, images: pics, shapes });
    }
    Ok(pages)
}

pub(crate) fn render_area(doc: &Document, k: usize, x0: f64, top: f64, x1: f64, bottom: f64) -> Option<Vec<u8>> {
    let api = lock().ok()?;
    let page = PageRef::load(&api, doc.raw, k).ok()?;
    let (w, h) = page.size();
    let (x0, top, x1, bottom) = (x0.max(0.0), top.max(0.0), x1.min(w), bottom.min(h));
    if x1 - x0 < 2.0 || bottom - top < 2.0 {
        return None;
    }
    let scale = (900.0 / (x1 - x0).max(bottom - top)).max(2.0).min(4.0);
    let (pw, ph, bgr) = render(&api, page.raw, scale, [x0, h - bottom, w - x1, top], ffi::BITMAP_BGR, ffi::RENDER_ANNOT).ok()?;
    Some(png_from_bgr(&bgr, pw, ph))
}

/// Python's `str.isspace` for one character (it counts the ASCII separators 0x1C-0x1F as space too).
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn font_name(api: &Api, tp: ffi::TextPage, i: c_int, buf: &mut [u8; 256], flags: &mut c_int) -> String {
    let n = unsafe { (api.FPDFText_GetFontInfo)(tp, i, buf.as_mut_ptr().cast(), buf.len() as c_ulong, flags) } as usize;
    // A longer name is not copied; like the Python code, take what the buffer holds.
    buf[..n.saturating_sub(1).min(buf.len())].iter().map(|&b| b as char).collect()
}

/// (bold, italic, fixed pitch) of character i, cached per text object.
fn font_style(api: &Api, tp: ffi::TextPage, i: c_int, buf: &mut [u8; 256], cache: &mut HashMap<usize, (bool, bool, bool)>) -> (bool, bool, bool) {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [bold_re, italic_re, mono_re] = RE.get_or_init(|| {
        [
            Regex::new(r"bold|black|heavy|semibold|demi|cmbx|cmb\d|sfbx|bx\d").unwrap(),
            Regex::new(r"italic|oblique|cmti|cmmi|cmsl|sfti|sfsl|cmbxti").unwrap(),
            Regex::new(r"mono|courier|consolas|typewriter|cmtt|sftt|inconsolata|menlo").unwrap(),
        ]
    });
    let key = unsafe { (api.FPDFText_GetTextObject)(tp, i) } as usize;
    *cache.entry(key).or_insert_with(|| {
        let mut flags = 0;
        let name = font_name(api, tp, i, buf, &mut flags).to_lowercase();
        let weight = unsafe { (api.FPDFText_GetFontWeight)(tp, i) };
        let bold = weight >= 600 || bold_re.is_match(&name) || flags & 0x40000 != 0;
        let italic = italic_re.is_match(&name) || flags & 0x40 != 0;
        let mono = flags & 1 != 0 || mono_re.is_match(&name);
        (bold, italic, mono)
    })
}

/// The characters of a text page, in points from the top-left of a page `h` high, with the corrections
/// for fonts of size 1 scaled up by the text matrix and for boxes too flat to select.
fn chars(api: &Api, tp: ffi::TextPage, h: f64) -> Vec<PChar> {
    let mut out = vec![];
    let mut buf = [0u8; 256];
    let mut cache = HashMap::new();
    let n = unsafe { (api.FPDFText_CountChars)(tp) };
    for i in 0..n {
        let u = unsafe { (api.FPDFText_GetUnicode)(tp, i) };
        if u == 0xFFFE || u == 0xFFFF {
            continue;
        }
        // Python took lone surrogates as they are; a Rust string cannot hold them, so they become "?" too.
        let c = char::from_u32(u).unwrap_or('?');
        let visible = !is_space(c);
        let generated = unsafe { (api.FPDFText_IsGenerated)(tp, i) } == 1;
        let (mut left, mut right, mut bottom, mut top) = (0.0, 0.0, 0.0, 0.0);
        if unsafe { (api.FPDFText_GetCharBox)(tp, i, &mut left, &mut right, &mut bottom, &mut top) } == 0 {
            (left, right, bottom, top) = (0.0, 0.0, 0.0, 0.0);
        }
        let mut size = unsafe { (api.FPDFText_GetFontSize)(tp, i) };
        let mut loose = ffi::RectF::default();
        if size <= 1.5 && visible && unsafe { (api.FPDFText_GetLooseCharBox)(tp, i, &mut loose) } != 0 {
            size = size.max((loose.top as f64 - loose.bottom as f64) / 1.15);
        }
        let (bold, italic, mono) = if generated { (false, false, false) } else { font_style(api, tp, i, &mut buf, &mut cache) };
        let (mut t, mut b) = (h - top, h - bottom);
        if visible && size > 0.0 && (b - t) < size * 0.3 {
            (t, b) = (b - size * 0.75, b + size * 0.2);
        }
        let (mut ox, mut oy) = (0.0, 0.0);
        let base = if unsafe { (api.FPDFText_GetCharOrigin)(tp, i, &mut ox, &mut oy) } != 0 { h - oy } else { b };
        out.push(PChar(c.to_string(), left, t, right, b, size, bold, italic, generated, mono, base));
    }
    out
}

/// The PDF editor's characters: measured from `left` and down from `top`, each with its font's name.
pub(crate) fn named_chars(api: &Api, tp: ffi::TextPage, left: f64, top: f64) -> Vec<Char> {
    let mut out: Vec<Char> = chars(api, tp, top)
        .into_iter()
        .map(|PChar(text, x0, t, x1, b, size, bold, italic, ..)| Char { text, x0: x0 - left, top: t, x1: x1 - left, bottom: b, size, bold, italic, font: String::new() })
        .collect();
    let mut names: HashMap<usize, String> = HashMap::new();
    let (mut buf, mut flags) = ([0u8; 256], 0);
    let mut k = 0;
    for i in 0..unsafe { (api.FPDFText_CountChars)(tp) } {
        let u = unsafe { (api.FPDFText_GetUnicode)(tp, i) };
        if u == 0xFFFE || u == 0xFFFF {
            continue;
        }
        if let Some(c) = out.get_mut(k) {
            let key = unsafe { (api.FPDFText_GetTextObject)(tp, i) } as usize;
            c.font = names.entry(key).or_insert_with(|| font_name(api, tp, i, &mut buf, &mut flags)).clone();
        }
        k += 1;
    }
    out
}

type M = [f64; 6];

/// Bounds (left, bottom, right, top) mapped by a matrix (a, b, c, d, e, f).
fn transform(m: Option<M>, l: f64, b: f64, r: f64, t: f64) -> (f64, f64, f64, f64) {
    let Some([ma, mb, mc, md, me, mf]) = m else { return (l, b, r, t) };
    let pts = [(l, b), (l, t), (r, b), (r, t)].map(|(x, y)| (ma * x + mc * y + me, mb * x + md * y + mf));
    let fold = |f: fn(f64, f64) -> f64, y: bool| pts.iter().map(|p| if y { p.1 } else { p.0 }).reduce(f).unwrap();
    (fold(f64::min, false), fold(f64::min, true), fold(f64::max, false), fold(f64::max, true))
}

/// The matrix applying inner, then outer.
fn compose(outer: Option<M>, inner: Option<M>) -> Option<M> {
    let (Some([a2, b2, c2, d2, e2, f2]), Some([a1, b1, c1, d1, e1, f1])) = (outer, inner) else { return outer.or(inner) };
    Some([a2 * a1 + c2 * b1, b2 * a1 + d2 * b1, a2 * c1 + c2 * d1, b2 * c1 + d2 * d1, a2 * e1 + c2 * f1 + e2, b2 * e1 + d2 * f1 + f2])
}

/// A picture found on a page: its box (points from the top-left) and object.
struct Found {
    x0: f64,
    top: f64,
    x1: f64,
    bottom: f64,
    obj: Obj,
}

struct Walk<'a> {
    api: &'a Api,
    h: f64,
    area: f64,
    budget: i64,
    pics: Vec<Found>,
    shapes: Vec<PShape>,
}

impl Walk<'_> {
    fn bounds(&self, o: Obj, m: Option<M>) -> Option<(f64, f64, f64, f64)> {
        let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
        if unsafe { (self.api.FPDFPageObj_GetBounds)(o, &mut l, &mut b, &mut r, &mut t) } == 0 {
            return None;
        }
        let (x0, y0, x1, y1) = transform(m, l as f64, b as f64, r as f64, t as f64);
        Some((x0, self.h - y1, x1, self.h - y0))
    }

    fn children(&self, form: Obj) -> Vec<Obj> {
        let n = unsafe { (self.api.FPDFFormObj_CountObjects)(form) };
        (0..n.max(0)).map(|i| unsafe { (self.api.FPDFFormObj_GetObject)(form, i as c_ulong) }).collect()
    }

    /// Whether a group draws anything but text.
    fn holds(&self, form: Obj, depth: usize) -> bool {
        self.children(form).into_iter().any(|o| match unsafe { (self.api.FPDFPageObj_GetType)(o) } {
            ffi::OBJ_PATH | ffi::OBJ_SHADING | ffi::OBJ_IMAGE => true,
            ffi::OBJ_FORM => depth < 4 && self.holds(o, depth + 1),
            _ => false,
        })
    }

    /// A path that leaves no mark on white paper: neither filled nor stroked, or only filled with white or
    /// nothing.
    fn invisible(&self, o: Obj) -> bool {
        let (mut fill, mut stroke) = (0, 0);
        if unsafe { (self.api.FPDFPath_GetDrawMode)(o, &mut fill, &mut stroke) } == 0 {
            return false;
        }
        if fill == 0 && stroke == 0 {
            return true;
        }
        if stroke != 0 {
            return false;
        }
        let (mut r, mut g, mut b, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0);
        if unsafe { (self.api.FPDFPageObj_GetFillColor)(o, &mut r, &mut g, &mut b, &mut a) } != 0 {
            return a == 0 || r.min(g).min(b) >= 250;
        }
        false
    }

    fn walk(&mut self, objs: Vec<Obj>, m: Option<M>, depth: usize) {
        for o in objs {
            self.budget -= 1;
            if self.budget < 0 {
                return;
            }
            let ty = unsafe { (self.api.FPDFPageObj_GetType)(o) };
            if ty == ffi::OBJ_TEXT {
                continue;
            }
            let Some((x0, top, x1, bottom)) = self.bounds(o, m) else { continue };
            match ty {
                ffi::OBJ_IMAGE => {
                    if x1 - x0 >= 24.0 && bottom - top >= 24.0 {
                        self.pics.push(Found { x0, top, x1, bottom, obj: o });
                    }
                }
                ffi::OBJ_FORM => {
                    if (x1 - x0) * (bottom - top) > 0.5 * self.area && depth < 6 {
                        let mut fm = ffi::Matrix::default();
                        let ok = unsafe { (self.api.FPDFPageObj_GetMatrix)(o, &mut fm) } != 0;
                        let inner = ok.then(|| [fm.a, fm.b, fm.c, fm.d, fm.e, fm.f].map(f64::from));
                        let kids = self.children(o);
                        self.walk(kids, compose(m, inner), depth + 1);
                    } else if self.holds(o, 0) {
                        self.shapes.push(PShape(x0, top, x1, bottom, 1));
                    }
                }
                ffi::OBJ_PATH => {
                    let kind = if self.invisible(o) { 2 } else { 0 };
                    self.shapes.push(PShape(x0, top, x1, bottom, kind));
                }
                ffi::OBJ_SHADING => self.shapes.push(PShape(x0, top, x1, bottom, 0)),
                _ => {}
            }
        }
    }
}

/// The page's pictures and drawings (kind 0 a path, 1 a group holding drawings or pictures, 2 a white or
/// invisible fill). Groups covering most of the page are looked into, to 6 levels; smaller ones count as
/// one drawing. At most 40 000 objects are looked at.
fn objects(api: &Api, page: ffi::Page, w: f64, h: f64) -> (Vec<Found>, Vec<PShape>) {
    let mut walk = Walk { api, h, area: w * h, budget: 40_000, pics: vec![], shapes: vec![] };
    let n = unsafe { (api.FPDFPage_CountObjects)(page) };
    let objs = (0..n.max(0)).map(|i| unsafe { (api.FPDFPage_GetObject)(page, i) }).collect();
    walk.walk(objs, None, 0);
    (walk.pics, walk.shapes)
}

/// A picture as PNG, drawn by PDFium with its transform and mask at about its own resolution (the
/// object's matrix is scaled for the drawing and put back after). None when it cannot be drawn.
fn picture(api: &Api, doc: ffi::Doc, page: ffi::Page, f: &Found) -> Option<PImage> {
    let o = f.obj;
    let (mut pw, mut ph): (c_uint, c_uint) = (0, 0);
    if unsafe { (api.FPDFImageObj_GetImagePixelSize)(o, &mut pw, &mut ph) } == 0 {
        return None;
    }
    let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
    if unsafe { (api.FPDFPageObj_GetBounds)(o, &mut l, &mut b, &mut r, &mut t) } == 0 {
        return None;
    }
    let (cw, ch) = ((r as f64 - l as f64).abs(), (t as f64 - b as f64).abs());
    if (pw < ph) != (cw < ch) {
        (pw, ph) = (ph, pw);
    }
    if cw == 0.0 || ch == 0.0 {
        return None;
    }
    let s = (pw as f64 / cw).max(ph as f64 / ch);
    let mut orig = ffi::Matrix::default();
    if unsafe { (api.FPDFPageObj_GetMatrix)(o, &mut orig) } == 0 {
        return None;
    }
    let sc = |v: f32| (v as f64 * s) as f32;
    let scaled = ffi::Matrix { a: sc(orig.a), b: sc(orig.b), c: sc(orig.c), d: sc(orig.d), e: sc(orig.e), f: sc(orig.f) };
    if unsafe { (api.FPDFPageObj_SetMatrix)(o, &scaled) } == 0 {
        return None;
    }
    let bm = unsafe { (api.FPDFImageObj_GetRenderedBitmap)(doc, page, o) };
    unsafe { (api.FPDFPageObj_SetMatrix)(o, &orig) };
    if bm.is_null() {
        return None;
    }
    let png = unsafe { bitmap_png(api, bm) };
    unsafe { (api.FPDFBitmap_Destroy)(bm) };
    Some(PImage { x0: f.x0, top: f.top, x1: f.x1, bottom: f.bottom, data: png? })
}

/// PNG bytes of a PDFium bitmap (grey, BGR, BGRx, BGRA or premultiplied BGRA). In BGRA, see-through parts
/// become white, computed in f32 as numpy did; in the others the fourth byte is dropped.
unsafe fn bitmap_png(api: &Api, bm: ffi::Bitmap) -> Option<Vec<u8>> {
    let (format, w, h, stride) = unsafe {
        ((api.FPDFBitmap_GetFormat)(bm), (api.FPDFBitmap_GetWidth)(bm), (api.FPDFBitmap_GetHeight)(bm), (api.FPDFBitmap_GetStride)(bm))
    };
    let buf = unsafe { (api.FPDFBitmap_GetBuffer)(bm) } as *const u8;
    let n = match format {
        ffi::BITMAP_GRAY => 1,
        ffi::BITMAP_BGR => 3,
        ffi::BITMAP_BGRX | ffi::BITMAP_BGRA | ffi::BITMAP_BGRA_PREMUL => 4,
        _ => return None,
    };
    if buf.is_null() || w < 0 || h < 0 || (stride as i64) < w as i64 * n as i64 {
        return None;
    }
    let (w, h, stride) = (w as usize, h as usize, stride as usize);
    let data = unsafe { std::slice::from_raw_parts(buf, stride * h) };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in data.chunks(stride).take(h) {
        for px in row[..w * n].chunks_exact(n) {
            match n {
                1 => rgb.extend([px[0]; 3]),
                _ if format == ffi::BITMAP_BGRA => {
                    let a = px[3] as f32 / 255.0;
                    rgb.extend([px[2], px[1], px[0]].map(|c| (c as f32 * a + 255.0 * (1.0 - a)).round_ties_even() as u8));
                }
                _ => rgb.extend([px[2], px[1], px[0]]),
            }
        }
    }
    Some(encode_rgb(rgb, w, h))
}

fn png_from_bgr(bgr: &[u8], w: usize, h: usize) -> Vec<u8> {
    encode_rgb(bgr.chunks_exact(3).flat_map(|p| [p[2], p[1], p[0]]).collect(), w, h)
}

fn encode_rgb(rgb: Vec<u8>, w: usize, h: usize) -> Vec<u8> {
    let img = RgbImage::from_raw(w as u32, h as u32, rgb).expect("pixel count matches the size");
    rewrite_core::imaging::encode_png(&DynamicImage::ImageRgb8(img))
}
