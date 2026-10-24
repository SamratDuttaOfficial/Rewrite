//! A small PDF writer: pages with text, lines, shapes, pictures, links and bookmarks.
//!
//! Text uses embedded TrueType subsets (Unicode, copy-and-paste safe through a ToUnicode map). Page
//! coordinates are points from the top-left corner, like the layout code thinks; they are flipped to
//! PDF's bottom-left origin here.

use crate::fonts::Font;
use crate::imaging;
use flate2::{Compression, write::ZlibEncoder};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::Arc;
use subsetter::GlyphRemapper;

pub fn num(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

/// A PDF text string: plain ASCII as a literal, anything else as UTF-16 with a byte order mark.
pub fn text_string(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let esc = s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
        format!("({esc})")
    } else {
        let mut out = String::from("<FEFF");
        for u in s.encode_utf16() {
            out.push_str(&format!("{u:04X}"));
        }
        out.push('>');
        out
    }
}

pub fn name(s: &str) -> String {
    let mut out = String::from("/");
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "-_.+".contains(c) {
            out.push(c);
        } else {
            for b in c.to_string().bytes() {
                out.push_str(&format!("#{b:02X}"));
            }
        }
    }
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

pub fn rgb(color: &str) -> (f64, f64, f64) {
    let c = color.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(c.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f64 / 255.0;
    (p(0), p(2), p(4))
}

fn color_op(color: &str, stroke: bool) -> String {
    let (r, g, b) = rgb(color);
    format!("{} {} {} {}", num(r), num(g), num(b), if stroke { "RG" } else { "rg" })
}

enum Obj {
    Dict(String),
    Stream(String, Vec<u8>), // dictionary entries (without Length) and data
}

struct FontRes {
    name: String,
    font: Arc<Font>,
    remap: GlyphRemapper,
    used: BTreeMap<u16, char>, // new glyph id -> character
}

#[derive(Clone)]
pub struct ImageRes {
    pub name: String,
    pub id: usize,
    pub px_w: u32,
    pub px_h: u32,
}

struct Annot {
    rect: [f64; 4],
    uri: Option<String>,
    dest: Option<(usize, f64)>,
}

pub struct Page {
    pub width: f64,
    pub height: f64,
    ops: Vec<u8>,
    fonts: Vec<usize>,
    xobjects: BTreeMap<String, usize>,
    gstates: BTreeMap<String, f64>,
    annots: Vec<Annot>,
    id: usize,
}

pub struct Writer {
    objects: Vec<Option<Obj>>,
    pub pages: Vec<Page>,
    fonts: Vec<FontRes>,
    font_index: HashMap<usize, usize>,
    images: HashMap<u64, Option<ImageRes>>,
    pub outline: Vec<(String, u32, usize, f64)>, // title, level, page, y from top
    pub title: String,
    pub author: String,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new("", "")
    }
}

impl Writer {
    pub fn new(title: &str, author: &str) -> Writer {
        Writer {
            objects: vec![None],
            pages: vec![],
            fonts: vec![],
            font_index: HashMap::new(),
            images: HashMap::new(),
            outline: vec![],
            title: title.into(),
            author: author.into(),
        }
    }

    fn reserve(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len() - 1
    }

    fn add(&mut self, o: Obj) -> usize {
        self.objects.push(Some(o));
        self.objects.len() - 1
    }

    fn set(&mut self, id: usize, o: Obj) {
        self.objects[id] = Some(o);
    }

    fn stream(&mut self, dict: &str, data: Vec<u8>, compress: bool) -> usize {
        if compress {
            let packed = deflate(&data);
            if packed.len() < data.len() {
                return self.add(Obj::Stream(format!("{dict}/Filter/FlateDecode"), packed));
            }
        }
        self.add(Obj::Stream(dict.to_string(), data))
    }

    pub fn new_page(&mut self, width: f64, height: f64) -> usize {
        let id = self.reserve();
        self.pages.push(Page {
            width,
            height,
            ops: vec![],
            fonts: vec![],
            xobjects: BTreeMap::new(),
            gstates: BTreeMap::new(),
            annots: vec![],
            id,
        });
        self.pages.len() - 1
    }

    fn font_res(&mut self, font: &Arc<Font>) -> usize {
        let key = Arc::as_ptr(font) as usize;
        if let Some(&k) = self.font_index.get(&key) {
            return k;
        }
        let k = self.fonts.len();
        let mut remap = GlyphRemapper::new();
        remap.remap(0);
        self.fonts.push(FontRes { name: format!("F{}", k + 1), font: font.clone(), remap, used: BTreeMap::new() });
        self.font_index.insert(key, k);
        k
    }

    fn encode(&mut self, fi: usize, text: &str) -> String {
        let res = &mut self.fonts[fi];
        let mut out = String::with_capacity(text.len() * 4);
        for ch in text.chars() {
            let g = res.font.gid(ch);
            let n = res.remap.remap(g);
            res.used.entry(n).or_insert(ch);
            out.push_str(&format!("{n:04X}"));
        }
        out
    }

    fn op(&mut self, p: usize, s: &str) {
        let ops = &mut self.pages[p].ops;
        ops.extend_from_slice(s.as_bytes());
        ops.push(b'\n');
    }

    fn y(&self, p: usize, y: f64) -> f64 {
        self.pages[p].height - y
    }

    pub fn save(&mut self, p: usize) {
        self.op(p, "q");
    }

    pub fn restore(&mut self, p: usize) {
        self.op(p, "Q");
    }

    pub fn clip(&mut self, p: usize, x: f64, y: f64, w: f64, h: f64) {
        let yy = self.y(p, y + h);
        self.op(p, &format!("{} {} {} {} re W n", num(x), num(yy), num(w), num(h)));
    }

    pub fn alpha(&mut self, p: usize, a: f64) {
        let key = format!("GS{}", (a * 100.0) as i32);
        self.pages[p].gstates.insert(key.clone(), a);
        self.op(p, &format!("/{key} gs"));
    }

    pub fn rect(&mut self, p: usize, x: f64, y: f64, w: f64, h: f64, fill: Option<&str>, stroke: Option<&str>, width: f64) {
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let mut parts = vec![];
        if let Some(f) = fill {
            parts.push(color_op(f, false));
        }
        if let Some(s) = stroke {
            parts.push(color_op(s, true));
            parts.push(format!("{} w", num(width)));
        }
        let op = match (fill.is_some(), stroke.is_some()) {
            (true, true) => "B",
            (true, false) => "f",
            _ => "S",
        };
        let yy = self.y(p, y + h);
        self.op(p, &format!("q {} {} {} {} {} re {} Q", parts.join(" "), num(x), num(yy), num(w), num(h), op));
    }

    pub fn line(&mut self, p: usize, x1: f64, y1: f64, x2: f64, y2: f64, color: &str, width: f64, dash: Option<&[f64]>) {
        let d = dash.map(|d| format!("[{}] 0 d ", d.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "))).unwrap_or_default();
        let (ya, yb) = (self.y(p, y1), self.y(p, y2));
        self.op(p, &format!("q {} {} w 0 J {}{} {} m {} {} l S Q", color_op(color, true), num(width), d, num(x1), num(ya), num(x2), num(yb)));
    }

    fn path(&self, p: usize, points: &[(f64, f64)]) -> String {
        points
            .iter()
            .enumerate()
            .map(|(k, (x, y))| format!("{} {} {}", num(*x), num(self.y(p, *y)), if k == 0 { "m" } else { "l" }))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn polyline(&mut self, p: usize, points: &[(f64, f64)], color: &str, width: f64) {
        if points.len() < 2 {
            return;
        }
        let path = self.path(p, points);
        self.op(p, &format!("q {} {} w 1 j 1 J {} S Q", color_op(color, true), num(width), path));
    }

    pub fn polygon(&mut self, p: usize, points: &[(f64, f64)], fill: Option<&str>, stroke: Option<&str>, width: f64) {
        if points.len() < 3 || (fill.is_none() && stroke.is_none()) {
            return;
        }
        let path = self.path(p, points);
        let mut parts = vec![];
        if let Some(f) = fill {
            parts.push(color_op(f, false));
        }
        if let Some(s) = stroke {
            parts.push(color_op(s, true));
            parts.push(format!("{} w 1 j", num(width)));
        }
        let op = match (fill.is_some(), stroke.is_some()) {
            (true, true) => "b",
            (true, false) => "f",
            _ => "s",
        };
        self.op(p, &format!("q {} {} h {} Q", parts.join(" "), path, op));
    }

    /// Draw a string with its baseline at (x, y). spacing = extra points after every space (justify);
    /// bold/italic fake the style when the font has no such face; angle in degrees, anticlockwise.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, p: usize, x: f64, y: f64, font: &Arc<Font>, size: f64, s: &str, color: &str,
                spacing: f64, bold: bool, italic: bool, angle: f64) {
        if s.is_empty() {
            return;
        }
        let fi = self.font_res(font);
        if !self.pages[p].fonts.contains(&fi) {
            self.pages[p].fonts.push(fi);
        }
        let show = if spacing != 0.0 && s.contains(' ') {
            let adj = -spacing * 1000.0 / size;
            let space = self.encode(fi, " ");
            let mut parts = vec![];
            for (k, piece) in s.split(' ').enumerate() {
                if k > 0 {
                    parts.push(format!("<{}> {}", space, num(adj)));
                }
                if !piece.is_empty() {
                    parts.push(format!("<{}>", self.encode(fi, piece)));
                }
            }
            format!("[{}] TJ", parts.join(" "))
        } else {
            format!("<{}> Tj", self.encode(fi, s))
        };
        let mode = if bold {
            format!("2 Tr {} w {} ", num(size * 0.035), color_op(color, true))
        } else {
            "0 Tr ".to_string()
        };
        let m = if angle != 0.0 {
            let (c, sn) = (angle.to_radians().cos(), angle.to_radians().sin());
            format!("{} {} {} {}", num(c), num(sn), num(-sn), num(c))
        } else if italic {
            "1 0 0.2 1".into()
        } else {
            "1 0 0 1".into()
        };
        let yy = self.y(p, y);
        let fname = self.fonts[fi].name.clone();
        self.op(p, &format!("BT {} {}/{} {} Tf {} {} {} Tm {} ET", color_op(color, false), mode, fname, num(size), m,
                            num(x), num(yy), show));
    }

    pub fn image(&mut self, p: usize, img: &ImageRes, x: f64, y: f64, w: f64, h: f64) {
        self.pages[p].xobjects.insert(img.name.clone(), img.id);
        let yy = self.y(p, y + h);
        self.op(p, &format!("q {} 0 0 {} {} {} cm /{} Do Q", num(w), num(h), num(x), num(yy), img.name));
    }

    /// A clickable area: to a web address, or to (page, y from the top) in this document.
    pub fn link(&mut self, p: usize, x: f64, y: f64, w: f64, h: f64, uri: Option<&str>, dest: Option<(usize, f64)>) {
        let (y0, y1) = (self.y(p, y + h), self.y(p, y));
        self.pages[p].annots.push(Annot { rect: [x, y0, x + w, y1], uri: uri.map(String::from), dest });
    }

    /// An image XObject for picture bytes (PNG, JPEG, GIF, BMP, TIFF, WebP). None when it cannot be read.
    pub fn image_bytes(&mut self, data: &[u8]) -> Option<&ImageRes> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut h);
        let key = h.finish();
        if !self.images.contains_key(&key) {
            let n = self.images.len() + 1;
            let made = self.make_image(data).map(|(id, w, hh)| ImageRes { name: format!("Im{n}"), id, px_w: w, px_h: hh });
            self.images.insert(key, made);
        }
        self.images.get(&key).and_then(|x| x.as_ref())
    }

    fn make_image(&mut self, data: &[u8]) -> Option<(usize, u32, u32)> {
        match imaging::sniff(data) {
            "jpeg" => {
                let (w, h) = imaging::size(data)?;
                let (comps, adobe) = imaging::jpeg_components(data);
                let cs = match comps {
                    1 => "/DeviceGray",
                    4 => "/DeviceCMYK",
                    _ => "/DeviceRGB",
                };
                let decode = if comps == 4 && adobe { "/Decode[1 0 1 0 1 0 1 0]" } else { "" };
                let d = format!("/Type/XObject/Subtype/Image/Width {w}/Height {h}/ColorSpace{cs}/BitsPerComponent 8/Filter/DCTDecode{decode}");
                return Some((self.stream(&d, data.to_vec(), false), w, h));
            }
            "png" => {
                let info = imaging::png_info(data)?;
                if !info.interlaced && matches!(info.ctype, 0 | 2 | 3) && !info.trns {
                    let colors = if info.ctype == 2 { 3 } else { 1 };
                    let cs = match info.ctype {
                        3 => {
                            let hex: String = info.palette.iter().map(|b| format!("{b:02X}")).collect();
                            format!("[/Indexed/DeviceRGB {} <{}>]", info.palette.len() / 3 - 1, hex)
                        }
                        0 => "/DeviceGray".into(),
                        _ => "/DeviceRGB".into(),
                    };
                    let d = format!(
                        "/Type/XObject/Subtype/Image/Width {}/Height {}/ColorSpace{}/BitsPerComponent {}/Filter/FlateDecode/DecodeParms<</Predictor 15/Colors {}/BitsPerComponent {}/Columns {}>>",
                        info.width, info.height, cs, info.depth, colors, info.depth, info.width);
                    return Some((self.stream(&d, info.idat, false), info.width, info.height));
                }
            }
            _ => {}
        }
        let img = imaging::decode(data).ok()?.to_rgba8();
        Some(self.image_rgba(&img))
    }

    /// An image XObject for RGBA pixels.
    pub fn image_rgba(&mut self, img: &image::RgbaImage) -> (usize, u32, u32) {
        let (w, h) = (img.width(), img.height());
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        let mut alpha = Vec::with_capacity((w * h) as usize);
        let mut any = false;
        for px in img.pixels() {
            rgb.extend_from_slice(&px.0[..3]);
            alpha.push(px[3]);
            any |= px[3] < 255;
        }
        let mut d = format!("/Type/XObject/Subtype/Image/Width {w}/Height {h}/ColorSpace/DeviceRGB/BitsPerComponent 8");
        if any {
            let smask = self.stream(&format!("/Type/XObject/Subtype/Image/Width {w}/Height {h}/ColorSpace/DeviceGray/BitsPerComponent 8"), alpha, true);
            d.push_str(&format!("/SMask {smask} 0 R"));
        }
        (self.stream(&d, rgb, true), w, h)
    }

    pub fn register_image(&mut self, key: u64, id: usize, w: u32, h: u32) -> &ImageRes {
        let n = self.images.len() + 1;
        self.images.insert(key, Some(ImageRes { name: format!("Im{n}"), id, px_w: w, px_h: h }));
        self.images.get(&key).unwrap().as_ref().unwrap()
    }

    // -- output -------------------------------------------------------------------------
    fn font_objects(&mut self, k: usize) -> usize {
        let (font, used, remap) = {
            let r = &self.fonts[k];
            (r.font.clone(), r.used.clone(), r.remap.clone())
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        used.keys().collect::<Vec<_>>().hash(&mut h);
        font.ps_name.hash(&mut h);
        let mut tagv = h.finish();
        let tag: String = (0..6).map(|_| {
            let c = (b'A' + (tagv % 26) as u8) as char;
            tagv /= 26;
            c
        }).collect();
        let base = format!("{}+{}", tag, font.ps_name);
        let k1000 = 1000.0 / font.upem;
        let sub = subsetter::subset(&font.data, font.index, &remap).unwrap_or_else(|_| font.data.to_vec());
        let file = self.stream(&format!("/Length1 {}", sub.len()), sub, true);
        let flags = 4 | if font.fixed { 1 } else { 0 } | if font.italic_angle != 0.0 { 64 } else { 0 };
        let bb = font.bbox;
        let desc = self.add(Obj::Dict(format!(
            "<</Type/FontDescriptor/FontName{}/Flags {}/FontBBox[{} {} {} {}]/ItalicAngle {}/Ascent {}/Descent {}/CapHeight {}/StemV 80/FontFile2 {} 0 R>>",
            name(&base), flags, (bb[0] * k1000).round(), (bb[1] * k1000).round(), (bb[2] * k1000).round(),
            (bb[3] * k1000).round(), num(font.italic_angle), (font.ascent * k1000).round(),
            (font.descent * k1000).round(), (font.cap_height * k1000).round(), file)));
        // widths of the new glyph ids, in order
        let olds: Vec<u16> = remap.remapped_gids().collect();
        let widths: Vec<String> = olds.iter().map(|g| format!("{}", (font.advance(*g) * k1000).round())).collect();
        let cid = self.add(Obj::Dict(format!(
            "<</Type/Font/Subtype/CIDFontType2/BaseFont{}/CIDSystemInfo<</Registry(Adobe)/Ordering(Identity)/Supplement 0>>/FontDescriptor {} 0 R/W[0[{}]]/DW 1000/CIDToGIDMap/Identity>>",
            name(&base), desc, widths.join(" "))));
        let mut lines = vec![];
        let pairs: Vec<(u16, char)> = used.into_iter().collect();
        for chunk in pairs.chunks(100) {
            lines.push(format!("{} beginbfchar", chunk.len()));
            for (g, ch) in chunk {
                let hex: String = ch.encode_utf16(&mut [0; 2]).iter().map(|u| format!("{u:04X}")).collect();
                lines.push(format!("<{g:04X}> <{hex}>"));
            }
            lines.push("endbfchar".into());
        }
        let cmap = format!(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n{}\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n",
            lines.join("\n"));
        let to_uni = self.stream("", cmap.into_bytes(), true);
        self.add(Obj::Dict(format!(
            "<</Type/Font/Subtype/Type0/BaseFont{}/Encoding/Identity-H/DescendantFonts[{} 0 R]/ToUnicode {} 0 R>>",
            name(&base), cid, to_uni)))
    }

    pub fn finish(mut self) -> Vec<u8> {
        let mut font_ids = vec![];
        for k in 0..self.fonts.len() {
            font_ids.push(self.font_objects(k));
        }
        let pages_id = self.reserve();
        let pages = std::mem::take(&mut self.pages);
        let page_refs: Vec<(usize, f64)> = pages.iter().map(|p| (p.id, p.height)).collect();
        for p in &pages {
            let content = self.stream("", p.ops.clone(), true);
            let mut res = String::from("/ProcSet[/PDF/Text/ImageB/ImageC]");
            if !p.fonts.is_empty() {
                res.push_str("/Font<<");
                for &fi in &p.fonts {
                    res.push_str(&format!("/{} {} 0 R", self.fonts[fi].name, font_ids[fi]));
                }
                res.push_str(">>");
            }
            if !p.xobjects.is_empty() {
                res.push_str("/XObject<<");
                for (n, id) in &p.xobjects {
                    res.push_str(&format!("/{n} {id} 0 R"));
                }
                res.push_str(">>");
            }
            if !p.gstates.is_empty() {
                res.push_str("/ExtGState<<");
                for (n, a) in &p.gstates {
                    res.push_str(&format!("/{n}<</Type/ExtGState/ca {}/CA {}>>", num(*a), num(*a)));
                }
                res.push_str(">>");
            }
            let mut annots = vec![];
            for a in &p.annots {
                let mut d = format!("<</Type/Annot/Subtype/Link/Border[0 0 0]/Rect[{} {} {} {}]",
                                    num(a.rect[0]), num(a.rect[1]), num(a.rect[2]), num(a.rect[3]));
                if let Some(u) = &a.uri {
                    d.push_str(&format!("/A<</S/URI/URI{}>>", text_string(u)));
                } else if let Some((pi, y)) = a.dest {
                    if let Some((pid, ph)) = page_refs.get(pi) {
                        d.push_str(&format!("/Dest[{} 0 R/XYZ 0 {} null]", pid, num(ph - y)));
                    }
                }
                d.push_str(">>");
                annots.push(self.add(Obj::Dict(d)));
            }
            let annots_s = if annots.is_empty() {
                String::new()
            } else {
                format!("/Annots[{}]", annots.iter().map(|a| format!("{a} 0 R")).collect::<Vec<_>>().join(" "))
            };
            self.set(p.id, Obj::Dict(format!(
                "<</Type/Page/Parent {} 0 R/MediaBox[0 0 {} {}]/Resources<<{}>>/Contents {} 0 R{}>>",
                pages_id, num(p.width), num(p.height), res, content, annots_s)));
        }
        let kids = page_refs.iter().map(|(id, _)| format!("{id} 0 R")).collect::<Vec<_>>().join(" ");
        self.set(pages_id, Obj::Dict(format!("<</Type/Pages/Kids[{}]/Count {}>>", kids, page_refs.len())));
        let mut catalog = format!("<</Type/Catalog/Pages {pages_id} 0 R");
        if !self.outline.is_empty() {
            let o = self.outlines(&page_refs);
            catalog.push_str(&format!("/Outlines {o} 0 R/PageMode/UseOutlines"));
        }
        if !self.title.is_empty() {
            catalog.push_str("/ViewerPreferences<</DisplayDocTitle true>>");
        }
        catalog.push_str(">>");
        let root = self.add(Obj::Dict(catalog));
        let stamp = {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            date_string(secs)
        };
        let mut info = format!("<</Producer(Rewrite)/Creator(Rewrite)/CreationDate({stamp})/ModDate({stamp})");
        if !self.title.is_empty() {
            info.push_str(&format!("/Title{}", text_string(&self.title)));
        }
        if !self.author.is_empty() {
            info.push_str(&format!("/Author{}", text_string(&self.author)));
        }
        info.push_str(">>");
        let info_id = self.add(Obj::Dict(info));
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = vec![0usize; self.objects.len()];
        for n in 1..self.objects.len() {
            offsets[n] = out.len();
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            match self.objects[n].take() {
                Some(Obj::Dict(d)) => out.extend_from_slice(d.as_bytes()),
                Some(Obj::Stream(d, data)) => {
                    out.extend_from_slice(format!("<<{}/Length {}>>\nstream\n", d, data.len()).as_bytes());
                    out.extend_from_slice(&data);
                    out.extend_from_slice(b"\nendstream");
                }
                None => out.extend_from_slice(b"null"),
            }
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", self.objects.len()).as_bytes());
        for n in 1..self.objects.len() {
            out.extend_from_slice(format!("{:010} 00000 n \n", offsets[n]).as_bytes());
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        out[..out.len().min(4000)].hash(&mut h);
        stamp.hash(&mut h);
        let id = format!("{:016X}{:016X}", h.finish(), h.finish().rotate_left(17));
        out.extend_from_slice(format!(
            "trailer\n<</Size {}/Root {} 0 R/Info {} 0 R/ID[<{id}><{id}>]>>\nstartxref\n{}\n%%EOF\n",
            self.objects.len(), root, info_id, xref).as_bytes());
        out
    }

    /// Bookmarks from (title, level, page, y) entries, nested by level.
    fn outlines(&mut self, pages: &[(usize, f64)]) -> usize {
        struct Node {
            id: usize,
            title: String,
            dest: String,
            parent: usize,
            kids: Vec<usize>,
            level: i64,
        }
        let root = self.reserve();
        let mut nodes: Vec<Node> = vec![Node { id: root, title: String::new(), dest: String::new(), parent: 0, kids: vec![], level: -1 }];
        let mut stack = vec![0usize];
        let entries = std::mem::take(&mut self.outline);
        for (title, level, pi, y) in entries {
            let Some((pid, ph)) = pages.get(pi) else { continue };
            let id = self.reserve();
            while nodes[*stack.last().unwrap()].level >= level as i64 {
                stack.pop();
            }
            let parent = *stack.last().unwrap();
            let k = nodes.len();
            nodes.push(Node { id, title, dest: format!("[{} 0 R/XYZ 0 {} null]", pid, num(ph - y + 4.0)), parent, kids: vec![], level: level as i64 });
            nodes[parent].kids.push(k);
            stack.push(k);
        }
        for k in 0..nodes.len() {
            let kids = nodes[k].kids.clone();
            let mut d = String::from("<<");
            if k == 0 {
                d.push_str("/Type/Outlines");
            } else {
                d.push_str(&format!("/Title{}/Parent {} 0 R/Dest{}", text_string(&nodes[k].title), nodes[nodes[k].parent].id, nodes[k].dest));
                let sib = &nodes[nodes[k].parent].kids;
                let pos = sib.iter().position(|x| *x == k).unwrap();
                if pos > 0 {
                    d.push_str(&format!("/Prev {} 0 R", nodes[sib[pos - 1]].id));
                }
                if pos + 1 < sib.len() {
                    d.push_str(&format!("/Next {} 0 R", nodes[sib[pos + 1]].id));
                }
            }
            if !kids.is_empty() {
                d.push_str(&format!("/First {} 0 R/Last {} 0 R/Count {}", nodes[kids[0]].id, nodes[*kids.last().unwrap()].id,
                                    if k == 0 { kids.len() as i64 } else { -(kids.len() as i64) }));
            } else if k == 0 {
                d.push_str("/Count 0");
            }
            d.push_str(">>");
            let id = nodes[k].id;
            self.set(id, Obj::Dict(d));
        }
        root
    }
}

fn date_string(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("D:{y:04}{mo:02}{d:02}{h:02}{m:02}{s:02}Z")
}
