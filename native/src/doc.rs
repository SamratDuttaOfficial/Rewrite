//! Word 97-2003 documents (.doc, the binary format) -> Document, read from the file's own structures:
//! the compound file, the piece table (text), character and paragraph formatting runs, the style sheet
//! (headings), lists, tables, fields (hyperlinks), footnotes and inline pictures.
//! Older Word 6/95 files and encrypted files are refused; the caller can fall back to LibreOffice.

use crate::convert::cp1252;
use crate::imaging;
use crate::model::*;
use std::collections::HashMap;
use std::io::{Cursor, Read};

fn u16le(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).unwrap_or(0)
}

fn u32le(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).unwrap_or(0)
}

fn i16le(b: &[u8], i: usize) -> i16 {
    u16le(b, i) as i16
}

struct Piece {
    cp_start: u32,
    cp_end: u32,
    fc: u32, // byte offset in WordDocument
    compressed: bool,
}

#[derive(Clone, Default, Debug)]
struct Chp {
    b: bool,
    i: bool,
    u: bool,
    s: bool,
    sup: bool,
    sub: bool,
    hidden: bool,
    deleted: bool,
    special: bool,
    size: Option<f64>,
    color: Option<String>,
    hl: Option<String>,
    pic: Option<u32>,
}

#[derive(Clone, Default, Debug)]
struct Pap {
    istd: u16,
    align: Option<String>,
    in_table: bool,
    row_end: bool,
    ilfo: u16,
    ilvl: u8,
    left: i32,
    outline: Option<u8>,
}

const ICO: [&str; 17] = ["", "#000000", "#0000FF", "#00FFFF", "#00FF00", "#FF00FF", "#FF0000", "#FFFF00", "#FFFFFF",
                         "#000080", "#008080", "#008000", "#800080", "#800000", "#808000", "#808080", "#C0C0C0"];

/// Size of a sprm's operand from its opcode (and the data, for variable sizes).
fn sprm_len(op: u16, data: &[u8], at: usize) -> usize {
    match op >> 13 {
        0 | 1 => 1,
        2 | 4 | 5 => 2,
        3 => 4,
        7 => 3,
        _ => {
            if op == 0xD608 || op == 0xD606 {
                u16le(data, at) as usize + 1 // a two-byte count of the rest, plus one
            } else if op == 0xC615 && data.get(at) == Some(&255) {
                let del = *data.get(at + 1).unwrap_or(&0) as usize;
                let add = *data.get(at + 2 + 4 * del).unwrap_or(&0) as usize;
                1 + 1 + 4 * del + 1 + 3 * add
            } else {
                *data.get(at).unwrap_or(&0) as usize + 1
            }
        }
    }
}

fn each_sprm(grpprl: &[u8], mut f: impl FnMut(u16, &[u8])) {
    let mut i = 0;
    while i + 2 <= grpprl.len() {
        let op = u16le(grpprl, i);
        let len = sprm_len(op, grpprl, i + 2);
        let end = (i + 2 + len).min(grpprl.len());
        f(op, &grpprl[i + 2..end]);
        i = end;
    }
}

fn toggle(cur: bool, v: u8, style: bool) -> bool {
    match v {
        0 => false,
        1 => true,
        0x80 => style,
        0x81 => !style,
        _ => cur,
    }
}

fn apply_chp(chp: &mut Chp, grpprl: &[u8], style: &Chp) {
    each_sprm(grpprl, |op, a| {
        let v = *a.first().unwrap_or(&0);
        match op {
            0x0835 => chp.b = toggle(chp.b, v, style.b),
            0x0836 => chp.i = toggle(chp.i, v, style.i),
            0x0837 => chp.s = toggle(chp.s, v, style.s),
            0x083C => chp.hidden = toggle(chp.hidden, v, style.hidden),
            0x0800 => chp.deleted = v != 0, // sprmCFRMarkDel
            0x0855 => chp.special = v != 0,
            0x2A3E => chp.u = v != 0,
            0x2A48 => {
                chp.sup = v == 1;
                chp.sub = v == 2;
            }
            0x4A43 => chp.size = Some(u16le(a, 0) as f64 / 2.0),
            0x2A42 => chp.color = ICO.get(v as usize).filter(|c| !c.is_empty() && **c != "#000000").map(|c| c.to_string()),
            0x6870 => {
                let cv = u32le(a, 0);
                if cv & 0xFF00_0000 == 0 && cv != 0 {
                    chp.color = Some(format!("#{:02X}{:02X}{:02X}", cv & 0xFF, (cv >> 8) & 0xFF, (cv >> 16) & 0xFF));
                }
            }
            0x2A0C => chp.hl = ICO.get(v as usize).filter(|c| !c.is_empty()).map(|c| c.to_string()),
            0x6A03 => chp.pic = Some(u32le(a, 0)),
            _ => {}
        }
    });
}

fn apply_pap(pap: &mut Pap, grpprl: &[u8]) {
    each_sprm(grpprl, |op, a| {
        let v = *a.first().unwrap_or(&0);
        match op {
            0x2403 | 0x2461 => pap.align = match v { 1 => Some("center".into()), 2 => Some("right".into()), 3 | 4 | 5 => Some("justify".into()), _ => None },
            0x2416 => pap.in_table = v != 0,
            0x2417 => pap.row_end = v != 0,
            0x6649 => pap.in_table = u32le(a, 0) > 0, // table depth
            0x664A => pap.row_end = v != 0,           // inner table row end
            0x260A => pap.ilvl = v,
            0x460B => pap.ilfo = u16le(a, 0),
            0x840F | 0x845E => pap.left = i16le(a, 0) as i32,
            0x2640 => pap.outline = Some(v),
            _ => {}
        }
    });
}

struct Style {
    name: String,
    sti: u16,
    base: u16,
    chp: Vec<u8>,
    pap: Vec<u8>,
}

struct Doc {
    word: Vec<u8>,
    table: Vec<u8>,
    data: Vec<u8>,
    pieces: Vec<Piece>,
    chpx: Vec<(u32, u32, Vec<u8>)>, // fc start, fc end, grpprl
    papx: Vec<(u32, u32, u16, Vec<u8>)>, // fc start, fc end, istd, grpprl
    styles: Vec<Option<Style>>,
    lists: HashMap<u16, Vec<u8>>, // ilfo -> number format of each level
}

impl Doc {
    fn fc_lcb(&self, k: usize) -> (u32, u32) {
        let w = &self.word;
        let csw = u16le(w, 32) as usize;
        let lw_at = 34 + csw * 2;
        let cslw = u16le(w, lw_at) as usize;
        let fc_at = lw_at + 2 + cslw * 4 + 2;
        let n = u16le(w, lw_at + 2 + cslw * 4) as usize;
        if k >= n {
            return (0, 0);
        }
        (u32le(w, fc_at + 8 * k), u32le(w, fc_at + 8 * k + 4))
    }

    fn ccp(&self, k: usize) -> u32 {
        let w = &self.word;
        let csw = u16le(w, 32) as usize;
        u32le(w, 34 + csw * 2 + 2 + 4 * k)
    }

    fn read_pieces(&mut self) -> Result<(), String> {
        let (fc, lcb) = self.fc_lcb(33);
        let clx = self.table.get(fc as usize..(fc + lcb) as usize).ok_or("broken piece table")?.to_vec();
        let mut i = 0;
        while i < clx.len() {
            match clx[i] {
                1 => i += 3 + u16le(&clx, i + 1) as usize,
                2 => {
                    let lcb = u32le(&clx, i + 1) as usize;
                    let plc = &clx[i + 5..(i + 5 + lcb).min(clx.len())];
                    let n = (plc.len() - 4) / 12;
                    for k in 0..n {
                        let cp_start = u32le(plc, 4 * k);
                        let cp_end = u32le(plc, 4 * (k + 1));
                        let pcd = 4 * (n + 1) + 8 * k;
                        let fcv = u32le(plc, pcd + 2);
                        let compressed = fcv & 0x4000_0000 != 0;
                        let fc = if compressed { (fcv & 0x3FFF_FFFF) / 2 } else { fcv };
                        self.pieces.push(Piece { cp_start, cp_end, fc, compressed });
                    }
                    return Ok(());
                }
                _ => break,
            }
        }
        Err("no piece table".into())
    }

    /// Text and the file offset (fc) of every character from cp a to cp b.
    fn text(&self, a: u32, b: u32) -> Vec<(char, u32)> {
        let mut out = vec![];
        for p in &self.pieces {
            if p.cp_end <= a || p.cp_start >= b {
                continue;
            }
            let s = a.max(p.cp_start);
            let e = b.min(p.cp_end);
            if p.compressed {
                for cp in s..e {
                    let fc = p.fc + (cp - p.cp_start);
                    let byte = *self.word.get(fc as usize).unwrap_or(&b' ');
                    out.push((cp1252(byte), fc));
                }
            } else {
                let mut cp = s;
                while cp < e {
                    let fc = p.fc + 2 * (cp - p.cp_start);
                    let u = u16le(&self.word, fc as usize);
                    let ch = if (0xD800..0xDC00).contains(&u) && cp + 1 < e {
                        let lo = u16le(&self.word, fc as usize + 2);
                        cp += 1;
                        char::decode_utf16([u, lo]).next().and_then(|r| r.ok()).unwrap_or('?')
                    } else {
                        char::from_u32(u as u32).unwrap_or('?')
                    };
                    out.push((ch, fc));
                    cp += 1;
                }
            }
        }
        out
    }

    fn read_fkps(&mut self) {
        // character runs
        let (fc, lcb) = self.fc_lcb(12);
        let plc = self.table.get(fc as usize..(fc + lcb) as usize).unwrap_or(&[]).to_vec();
        let n = if plc.len() >= 4 { (plc.len() - 4) / 8 } else { 0 };
        for k in 0..n {
            let pn = u32le(&plc, 4 * (n + 1) + 4 * k) & 0x3F_FFFF;
            let page = self.word.get(pn as usize * 512..pn as usize * 512 + 512).unwrap_or(&[]).to_vec();
            if page.len() < 512 {
                continue;
            }
            let crun = page[511] as usize;
            for r in 0..crun {
                let (fa, fb) = (u32le(&page, 4 * r), u32le(&page, 4 * (r + 1)));
                let off = page[4 * (crun + 1) + r] as usize * 2;
                let grpprl = if off == 0 { vec![] } else {
                    let cb = page[off] as usize;
                    page.get(off + 1..off + 1 + cb).unwrap_or(&[]).to_vec()
                };
                self.chpx.push((fa, fb, grpprl));
            }
        }
        // paragraph runs
        let (fc, lcb) = self.fc_lcb(13);
        let plc = self.table.get(fc as usize..(fc + lcb) as usize).unwrap_or(&[]).to_vec();
        let n = if plc.len() >= 4 { (plc.len() - 4) / 8 } else { 0 };
        for k in 0..n {
            let pn = u32le(&plc, 4 * (n + 1) + 4 * k) & 0x3F_FFFF;
            let page = self.word.get(pn as usize * 512..pn as usize * 512 + 512).unwrap_or(&[]).to_vec();
            if page.len() < 512 {
                continue;
            }
            let crun = page[511] as usize;
            for r in 0..crun {
                let (fa, fb) = (u32le(&page, 4 * r), u32le(&page, 4 * (r + 1)));
                let off = page[4 * (crun + 1) + 13 * r] as usize * 2;
                if off == 0 || off >= 511 {
                    self.papx.push((fa, fb, 0, vec![]));
                    continue;
                }
                let cb = page[off] as usize;
                let (start, len) = if cb == 0 { (off + 2, page[off + 1] as usize * 2) } else { (off + 1, 2 * cb - 1) };
                let body = page.get(start..(start + len).min(511)).unwrap_or(&[]);
                let istd = u16le(body, 0);
                self.papx.push((fa, fb, istd, body.get(2..).unwrap_or(&[]).to_vec()));
            }
        }
        self.chpx.sort_by_key(|x| x.0);
        self.papx.sort_by_key(|x| x.0);
    }

    fn read_styles(&mut self) {
        let (fc, lcb) = self.fc_lcb(1);
        let st = self.table.get(fc as usize..(fc + lcb) as usize).unwrap_or(&[]).to_vec();
        if st.len() < 4 {
            return;
        }
        let cb_stshi = u16le(&st, 0) as usize;
        let cstd = u16le(&st, 2) as usize;
        let cb_base = u16le(&st, 4) as usize;
        let mut i = 2 + cb_stshi;
        for _ in 0..cstd {
            if i + 2 > st.len() {
                break;
            }
            let cb = u16le(&st, i) as usize;
            let std = st.get(i + 2..(i + 2 + cb).min(st.len())).unwrap_or(&[]);
            i += 2 + cb;
            if cb == 0 || std.len() < cb_base {
                self.styles.push(None);
                continue;
            }
            let sti = u16le(std, 0) & 0x0FFF;
            let stk = u16le(std, 2) & 0x000F;
            let base = u16le(std, 2) >> 4;
            let cupx = u16le(std, 4) & 0x000F;
            // name: a count of UTF-16 units, the units and a terminating zero
            let n_at = cb_base;
            let cch = u16le(std, n_at) as usize;
            let name: String = (0..cch).map(|k| char::from_u32(u16le(std, n_at + 2 + 2 * k) as u32).unwrap_or('?')).collect();
            let mut j = n_at + 2 + 2 * cch + 2;
            let (mut chp, mut pap) = (vec![], vec![]);
            for u in 0..cupx {
                if j % 2 == 1 {
                    j += 1;
                }
                let cbx = u16le(std, j) as usize;
                let upx = std.get(j + 2..(j + 2 + cbx).min(std.len())).unwrap_or(&[]).to_vec();
                j += 2 + cbx;
                match (stk, u) {
                    (1, 0) => pap = upx.get(2..).unwrap_or(&[]).to_vec(), // paragraph style: istd, then the sprms
                    (1, 1) | (2, 0) => chp = upx,
                    _ => {}
                }
            }
            self.styles.push(Some(Style { name, sti, base, chp, pap }));
        }
    }

    fn read_lists(&mut self) {
        let (fc, lcb) = self.fc_lcb(73);
        // the level definitions follow the list table, outside its stated length
        let lst = if lcb > 0 { self.table.get(fc as usize..).unwrap_or(&[]).to_vec() } else { vec![] };
        let (fc2, lcb2) = self.fc_lcb(74);
        let lfo = self.table.get(fc2 as usize..(fc2 + lcb2) as usize).unwrap_or(&[]).to_vec();
        if lst.len() < 2 || lfo.len() < 4 {
            return;
        }
        // lists: their ids and the number format of each level
        let clst = u16le(&lst, 0) as usize;
        let mut ids = vec![];
        for k in 0..clst {
            let at = 2 + 28 * k;
            let lsid = u32le(&lst, at);
            let simple = lst.get(at + 26).is_some_and(|f| f & 1 != 0);
            ids.push((lsid, if simple { 1 } else { 9 }));
        }
        let mut j = 2 + 28 * clst;
        let mut formats: HashMap<u32, Vec<u8>> = HashMap::new();
        for (lsid, levels) in ids {
            let mut fmts = vec![];
            for _ in 0..levels {
                if j + 28 > lst.len() {
                    break;
                }
                let nfc = lst[j + 4];
                let cb_chpx = lst[j + 24] as usize;
                let cb_papx = lst[j + 25] as usize;
                j += 28 + cb_papx + cb_chpx;
                let cch = u16le(&lst, j) as usize;
                j += 2 + 2 * cch;
                fmts.push(nfc);
            }
            formats.insert(lsid, fmts);
        }
        let n = u32le(&lfo, 0) as usize;
        for k in 0..n {
            let lsid = u32le(&lfo, 4 + 16 * k);
            if let Some(f) = formats.get(&lsid) {
                self.lists.insert((k + 1) as u16, f.clone());
            }
        }
    }

    fn style_chp(&self, istd: u16, depth: u32) -> Chp {
        let mut chp = Chp::default();
        if depth > 10 {
            return chp;
        }
        if let Some(Some(s)) = self.styles.get(istd as usize) {
            if s.base != 0x0FFF && s.base != istd {
                chp = self.style_chp(s.base, depth + 1);
            }
            let base = chp.clone();
            apply_chp(&mut chp, &s.chp, &base);
        }
        chp
    }

    fn style_pap(&self, istd: u16, depth: u32) -> Pap {
        let mut pap = Pap::default();
        if depth > 10 {
            return pap;
        }
        if let Some(Some(s)) = self.styles.get(istd as usize) {
            if s.base != 0x0FFF && s.base != istd {
                pap = self.style_pap(s.base, depth + 1);
            }
            apply_pap(&mut pap, &s.pap);
        }
        pap.istd = istd;
        pap
    }

    fn chp_at(&self, fc: u32, istd: u16) -> Chp {
        let style = self.style_chp(istd, 0);
        let mut chp = style.clone();
        let k = self.chpx.partition_point(|x| x.0 <= fc);
        if k > 0 {
            let (a, b, g) = &self.chpx[k - 1];
            if *a <= fc && fc < *b {
                apply_chp(&mut chp, g, &style);
            }
        }
        chp
    }

    fn pap_at(&self, fc: u32) -> Pap {
        let k = self.papx.partition_point(|x| x.0 <= fc);
        if k > 0 {
            let (a, b, istd, g) = &self.papx[k - 1];
            if *a <= fc && fc < *b {
                let mut pap = self.style_pap(*istd, 0);
                apply_pap(&mut pap, g);
                return pap;
            }
        }
        Pap::default()
    }

    fn style_kind(&self, istd: u16) -> String {
        let mut k = istd;
        for _ in 0..10 {
            let Some(Some(s)) = self.styles.get(k as usize) else { break };
            if (1..=9).contains(&s.sti) {
                return format!("h{}", s.sti.min(6));
            }
            let n = s.name.to_lowercase();
            if n == "title" || s.sti == 62 {
                return "title".into();
            }
            if n == "subtitle" || s.sti == 74 {
                return "subtitle".into();
            }
            if let Some(h) = n.strip_prefix("heading").and_then(|x| x.trim().parse::<u32>().ok()) {
                return format!("h{}", h.clamp(1, 6));
            }
            if n == "caption" || s.sti == 34 {
                return "caption".into();
            }
            if matches!(n.as_str(), "html preformatted" | "source code" | "sourcecode" | "code" | "macro text" | "plain text" | "preformatted text") {
                return "code".into();
            }
            if n == "quote" || n == "block text" {
                return "quote".into();
            }
            if s.base == 0x0FFF || s.base == k {
                break;
            }
            k = s.base;
        }
        "normal".into()
    }

    /// A picture whose data begins at `fc` in the Data stream.
    fn picture(&self, fc: u32) -> Option<Image> {
        let d = &self.data;
        let at = fc as usize;
        let lcb = u32le(d, at) as usize;
        let cb_header = u16le(d, at + 4) as usize;
        if lcb < cb_header || at + lcb > d.len() || cb_header < 0x3A {
            return None;
        }
        let mm = i16le(d, at + 6);
        let dxa = u16le(d, at + 28) as f64;
        let dya = u16le(d, at + 30) as f64;
        let mx = u16le(d, at + 32) as f64;
        let my = u16le(d, at + 34) as f64;
        let mut start = at + cb_header;
        if mm == 0x66 {
            start += 1 + *d.get(start).unwrap_or(&0) as usize; // a picture name comes first
        }
        let region = &d[start..at + lcb];
        let (data, ext) = find_blip(region)?;
        let mut w = dxa * if mx > 0.0 { mx / 1000.0 } else { 1.0 } / 20.0;
        let mut h = dya * if my > 0.0 { my / 1000.0 } else { 1.0 } / 20.0;
        if w < 2.0 || h < 2.0 {
            let (nw, nh) = imaging::natural_size(&data, Some(451.0));
            w = nw;
            h = nh;
        }
        Some(Image { data, ext: ext.into(), width: w, height: h, alt: String::new(), name: format!("picture.{ext}") })
    }
}

/// The first picture (BLIP) in a run of OfficeArt records: (bytes, format).
fn find_blip(b: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    let mut i = 0;
    while i + 8 <= b.len() {
        let ver_inst = u16le(b, i);
        let typ = u16le(b, i + 2);
        let len = u32le(b, i + 4) as usize;
        let body_at = i + 8;
        let inst = ver_inst >> 4;
        if ver_inst & 0xF == 0xF {
            // a container: look inside
            if let Some(x) = find_blip(&b[body_at..(body_at + len).min(b.len())]) {
                return Some(x);
            }
        } else if typ == 0xF007 {
            // FBSE: the picture follows its 36-byte header (and an optional name)
            let name_len = *b.get(body_at + 33).unwrap_or(&0) as usize;
            let inner = body_at + 36 + name_len;
            if inner < b.len() {
                if let Some(x) = find_blip(&b[inner..(body_at + len).min(b.len())]) {
                    return Some(x);
                }
            }
        } else if (0xF018..=0xF117).contains(&typ) {
            let two = |single: u16| inst != single;
            let (skip, ext) = match typ {
                0xF01D | 0xF02A => (if two(0x46A) && two(0x6E2) { 33 } else { 17 }, "jpeg"),
                0xF01E => (if two(0x6E0) { 33 } else { 17 }, "png"),
                0xF01F => (if two(0x7A8) { 33 } else { 17 }, "bmp"),
                0xF029 => (if two(0x6E4) { 33 } else { 17 }, "tiff"),
                0xF01A => (if two(0x3D4) { 66 } else { 50 }, "emf"),
                0xF01B => (if two(0x216) { 66 } else { 50 }, "wmf"),
                _ => (17, ""),
            };
            let s = body_at + skip;
            let e = (body_at + len).min(b.len());
            if s < e && !ext.is_empty() {
                let mut data = b[s..e].to_vec();
                if ext == "emf" || ext == "wmf" {
                    // metafiles may be deflate-compressed
                    if b.get(body_at + skip - 2) == Some(&0) {
                        let mut z = flate2::read::ZlibDecoder::new(&data[..]);
                        let mut out = vec![];
                        if z.read_to_end(&mut out).is_ok() && !out.is_empty() {
                            data = out;
                        }
                    }
                }
                if ext == "bmp" {
                    // a DIB without its file header
                    let mut f = b"BM".to_vec();
                    f.extend_from_slice(&((data.len() + 14) as u32).to_le_bytes());
                    f.extend_from_slice(&[0, 0, 0, 0]);
                    let hdr = u32le(&data, 0);
                    let colors = if u16le(&data, 14) <= 8 { (if u32le(&data, 32) > 0 { u32le(&data, 32) } else { 1 << u16le(&data, 14) }) * 4 } else { 0 };
                    f.extend_from_slice(&(14 + hdr + colors).to_le_bytes());
                    f.extend_from_slice(&data);
                    data = f;
                }
                return Some((data, ext));
            }
        }
        i = body_at + len;
    }
    // last resort: a PNG or JPEG signature anywhere in the record
    if let Some(p) = b.windows(8).position(|w| w == b"\x89PNG\r\n\x1a\n") {
        return Some((b[p..].to_vec(), "png"));
    }
    if let Some(p) = b.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) {
        return Some((b[p..].to_vec(), "jpeg"));
    }
    None
}

fn stream(comp: &mut cfb::CompoundFile<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut s = comp.open_stream(name).ok()?;
    let mut buf = vec![];
    s.read_to_end(&mut buf).ok()?;
    Some(buf)
}

pub fn read(data: &[u8]) -> Result<Document, String> {
    let mut comp = cfb::CompoundFile::open(Cursor::new(data)).map_err(|_| "not a Word 97-2003 document".to_string())?;
    let word = stream(&mut comp, "WordDocument").ok_or("not a Word document (no WordDocument stream)")?;
    if u16le(&word, 0) != 0xA5EC {
        return Err("not a Word document".into());
    }
    let nfib = u16le(&word, 2);
    let flags = u16le(&word, 10);
    if nfib < 0x00C1 {
        return Err("this is an old Word 6/95 document".into());
    }
    if flags & 0x0100 != 0 {
        return Err("the document is encrypted".into());
    }
    let table_name = if flags & 0x0200 != 0 { "1Table" } else { "0Table" };
    let table = stream(&mut comp, table_name).ok_or("the table stream is missing")?;
    let data_stream = stream(&mut comp, "Data").unwrap_or_default();
    let mut d = Doc { word, table, data: data_stream, pieces: vec![], chpx: vec![], papx: vec![], styles: vec![], lists: HashMap::new() };
    d.read_pieces()?;
    d.read_fkps();
    d.read_styles();
    d.read_lists();
    let ccp_text = d.ccp(3);
    let ccp_ftn = d.ccp(4);
    // footnotes: where their marks are in the main text, and their own text after it
    let mut notes: HashMap<u32, Vec<Inline>> = HashMap::new();
    let (fc_ref, lcb_ref) = d.fc_lcb(2);
    let (fc_txt, lcb_txt) = d.fc_lcb(3);
    if lcb_ref >= 4 && lcb_txt >= 8 && ccp_ftn > 0 {
        let refs = d.table[fc_ref as usize..(fc_ref + lcb_ref) as usize].to_vec();
        let txts = d.table[fc_txt as usize..(fc_txt + lcb_txt) as usize].to_vec();
        let n = (refs.len() - 4) / 6;
        for k in 0..n {
            let cp = u32le(&refs, 4 * k);
            let (a, b) = (u32le(&txts, 4 * k), u32le(&txts, 4 * (k + 1)));
            let text: String = d.text(ccp_text + a, ccp_text + b).into_iter().map(|(c, _)| c)
                .filter(|c| !matches!(*c, '\u{2}' | '\r' | '\u{5}')).collect();
            notes.insert(cp, vec![Inline::Run(Run::new(text.trim().to_string()))]);
        }
    }
    let chars = d.text(0, ccp_text);
    let mut doc = Document { size: 11.0, ..Default::default() };
    // the body text size: the most common run size
    let mut sizes: HashMap<i64, usize> = HashMap::new();
    let mut content: Vec<Inline> = vec![];
    let mut fields: Vec<(String, bool, Option<String>)> = vec![]; // instruction, in result, link
    let mut cell: Vec<Paragraph> = vec![];
    let mut row: Vec<Vec<Paragraph>> = vec![];
    let mut table: Option<Table> = None;
    let flush_table = |table: &mut Option<Table>, blocks: &mut Vec<Block>| {
        if let Some(t) = table.take() {
            if !t.rows.is_empty() {
                blocks.push(Block::Table(t));
            }
        }
    };
    let mut cp = 0u32;
    for (ch, fc) in chars {
        let this_cp = cp;
        cp += 1;
        match ch {
            '\u{13}' => {
                fields.push((String::new(), false, None));
                continue;
            }
            '\u{14}' => {
                if let Some(f) = fields.last_mut() {
                    f.1 = true;
                    if let Some(m) = regex::Regex::new(r#"HYPERLINK\s+(?:\\l\s+)?"([^"]+)""#).unwrap().captures(&f.0) {
                        f.2 = Some(m[1].to_string());
                    }
                }
                continue;
            }
            '\u{15}' => {
                fields.pop();
                continue;
            }
            _ => {}
        }
        if let Some(f) = fields.last_mut() {
            if !f.1 {
                f.0.push(ch);
                continue;
            }
        }
        if fields.iter().any(|f| !f.1) {
            continue;
        }
        if ch == '\r' || ch == '\u{7}' {
            let pap = d.pap_at(fc);
            let style = d.style_kind(pap.istd);
            let list = if pap.ilfo > 0 {
                let fmt = d.lists.get(&pap.ilfo).and_then(|l| l.get(pap.ilvl as usize)).copied().unwrap_or(23);
                if fmt == 255 { None } else { Some((if fmt == 23 { "bullet" } else { "number" }.to_string(), (pap.ilvl as u32).min(8))) }
            } else {
                None
            };
            let mut style = style;
            if let (Some(o), "normal") = (pap.outline, style.as_str()) {
                if o < 9 {
                    style = format!("h{}", (o + 1).min(6));
                }
            }
            let p = Paragraph {
                content: strip_content(std::mem::take(&mut content)),
                style,
                align: pap.align.clone(),
                list: list.clone(),
                indent: if list.is_none() && pap.left >= 600 { (pap.left / 720).min(6) as u32 } else { 0 },
            };
            if pap.in_table {
                if ch == '\u{7}' && pap.row_end {
                    let r = std::mem::take(&mut row);
                    table.get_or_insert_with(|| Table { rows: vec![], header: false, widths: None }).rows.push(r);
                    cell.clear();
                } else if ch == '\u{7}' {
                    cell.push(p);
                    row.push(std::mem::take(&mut cell));
                } else {
                    cell.push(p);
                }
            } else {
                flush_table(&mut table, &mut doc.blocks);
                doc.blocks.push(Block::Paragraph(p));
            }
            continue;
        }
        let pap_istd = d.pap_at(fc).istd;
        let chp = d.chp_at(fc, pap_istd);
        if chp.hidden || chp.deleted {
            continue;
        }
        let link = fields.iter().rev().find_map(|f| f.2.clone());
        if chp.special {
            match ch {
                '\u{1}' => {
                    if let Some(img) = chp.pic.and_then(|fc| d.picture(fc)) {
                        content.push(Inline::Image(img));
                    }
                }
                '\u{2}' => {
                    if let Some(n) = notes.get(&this_cp) {
                        content.push(Inline::Note(Note { runs: n.clone() }));
                    }
                }
                _ => {}
            }
            continue;
        }
        let text = match ch {
            '\u{b}' => {
                content.push(Inline::Break(Break::default()));
                continue;
            }
            '\u{c}' => {
                flush_table(&mut table, &mut doc.blocks);
                if !content.is_empty() {
                    doc.blocks.push(Block::Paragraph(Paragraph::new(strip_content(std::mem::take(&mut content)))));
                }
                doc.blocks.push(Block::PageBreak(PageBreak {}));
                continue;
            }
            '\u{1e}' => "\u{2011}".to_string(),
            '\u{1f}' | '\u{8}' | '\u{1}' | '\u{5}' => continue,
            c => c.to_string(),
        };
        if let Some(s) = chp.size {
            *sizes.entry((s * 2.0) as i64).or_insert(0) += 1;
        }
        content.push(Inline::Run(Run {
            text,
            b: chp.b,
            i: chp.i,
            u: chp.u && link.is_none(),
            s: chp.s,
            sup: chp.sup,
            sub: chp.sub,
            size: chp.size,
            color: if link.is_some() { None } else { chp.color.clone() },
            hl: chp.hl.clone(),
            link,
            ..Default::default()
        }));
    }
    if !content.is_empty() {
        doc.blocks.push(Block::Paragraph(Paragraph::new(strip_content(content))));
    }
    flush_table(&mut table, &mut doc.blocks);
    if let Some((s, _)) = sizes.iter().max_by_key(|x| x.1) {
        doc.size = *s as f64 / 2.0;
    }
    let body = doc.size;
    // runs at the body size do not need their own size
    let clear = |content: &mut Vec<Inline>| {
        for x in content.iter_mut() {
            if let Inline::Run(r) = x {
                if r.size == Some(body) {
                    r.size = None;
                }
            }
        }
        *content = merge_runs(std::mem::take(content));
    };
    for b in doc.blocks.iter_mut() {
        match b {
            Block::Paragraph(p) => {
                clear(&mut p.content);
                if p.style != "normal" {
                    let lk = look(&p.style);
                    for x in p.content.iter_mut() {
                        if let Inline::Run(r) = x {
                            r.size = None; // headings take their style's look
                            r.b = false;
                            if lk.italic {
                                r.i = false;
                            }
                            r.color = None;
                        }
                    }
                }
            }
            Block::Table(t) => {
                for row in t.rows.iter_mut() {
                    for c in row.iter_mut() {
                        for p in c.iter_mut() {
                            clear(&mut p.content);
                        }
                    }
                }
                let n = t.ncols();
                for row in t.rows.iter_mut() {
                    while row.len() < n {
                        row.push(vec![Paragraph::default()]);
                    }
                }
                t.header = t.rows.first().is_some_and(|first| first.iter().all(|cell| {
                    cell.iter().any(|p| !p.text().trim().is_empty()) && cell.iter().all(|p| p.content.iter().all(|x| matches!(x, Inline::Run(r) if r.b || r.text.trim().is_empty())))
                }));
            }
            _ => {}
        }
    }
    let mut grouped: Vec<Block> = vec![];
    for b in std::mem::take(&mut doc.blocks) {
        if let Block::Paragraph(p) = &b {
            if p.style == "code" {
                if let Some(Block::CodeBlock(c)) = grouped.last_mut() {
                    c.text.push('\n');
                    c.text.push_str(&p.text());
                } else {
                    grouped.push(Block::CodeBlock(CodeBlock { text: p.text(), lang: String::new() }));
                }
                continue;
            }
        }
        grouped.push(b);
    }
    doc.blocks = grouped;
    if let Some(Block::Paragraph(p)) = doc.blocks.iter().find(|b| matches!(b, Block::Paragraph(p) if p.style == "title" || p.style == "h1")) {
        doc.title = p.text().trim().chars().take(200).collect();
    }
    // the SummaryInformation title and author, when present
    if let Some(si) = stream(&mut comp, "\u{5}SummaryInformation") {
        if let Some(t) = summary_string(&si, 2) {
            if !t.trim().is_empty() {
                doc.title = t.trim().to_string();
            }
        }
        if let Some(a) = summary_string(&si, 4) {
            doc.author = a.trim().to_string();
        }
    }
    Ok(doc)
}

/// A string property (2 = title, 4 = author) of an OLE SummaryInformation stream.
fn summary_string(si: &[u8], pid: u32) -> Option<String> {
    let set_at = u32le(si, 44) as usize;
    let count = u32le(si, set_at + 4) as usize;
    for k in 0..count.min(100) {
        let id = u32le(si, set_at + 8 + 8 * k);
        let off = u32le(si, set_at + 12 + 8 * k) as usize;
        if id != pid {
            continue;
        }
        let at = set_at + off;
        let typ = u32le(si, at);
        let len = u32le(si, at + 4) as usize;
        let bytes = si.get(at + 8..at + 8 + len)?;
        return Some(match typ {
            0x1E => bytes.iter().take_while(|b| **b != 0).map(|b| cp1252(*b)).collect(),
            0x1F => String::from_utf16_lossy(&bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).take_while(|u| *u != 0).collect::<Vec<_>>()),
            _ => return None,
        });
    }
    None
}

