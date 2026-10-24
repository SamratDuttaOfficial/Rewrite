//! Installed fonts: finding them by family, measuring text, and the fallbacks for missing characters.
//! Only TrueType-outline fonts that allow embedding are used, so they can be subset into PDF files.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const SERIF: [&str; 10] = ["Georgia", "Cambria", "Times New Roman", "Charter", "Iowan Old Style", "Liberation Serif",
                               "DejaVu Serif", "Noto Serif", "Caladea", "Times"];
pub const SANS: [&str; 11] = ["Calibri", "Segoe UI", "Arial", "Helvetica Neue", "Helvetica", "Carlito", "Liberation Sans",
                              "Noto Sans", "DejaVu Sans", "Ubuntu", "Cantarell"];
pub const MONO: [&str; 10] = ["Consolas", "Cascadia Mono", "Courier New", "Menlo", "SF Mono", "Liberation Mono",
                              "DejaVu Sans Mono", "Noto Sans Mono", "Ubuntu Mono", "Courier"];
pub const FALLBACK: [&str; 33] = [
    "Segoe UI", "Arial", "DejaVu Sans", "Noto Sans", "Liberation Sans", "Segoe UI Symbol", "Cambria Math",
    "DejaVu Sans Mono", "Nirmala UI", "Leelawadee UI", "Ebrima", "Gadugi", "Microsoft Himalaya", "Mongolian Baiti",
    "Microsoft Yi Baiti", "Javanese Text", "Myanmar Text", "Sylfaen", "Arial Unicode MS", "Microsoft YaHei",
    "Microsoft JhengHei", "Yu Gothic", "Malgun Gothic", "MS Gothic", "SimSun", "Noto Sans CJK SC",
    "Noto Sans Devanagari", "Noto Sans Bengali", "Noto Sans Arabic", "Noto Sans Hebrew", "Noto Sans Thai",
    "Segoe UI Emoji", "Segoe UI Historic",
];

/// Names Word documents use, and which kind of font stands in when they are not installed.
pub fn alias(family: &str) -> Option<&'static str> {
    Some(match family.to_lowercase().as_str() {
        "calibri" | "calibri light" | "arial" | "helvetica" | "aptos" | "segoe ui" | "verdana" | "tahoma" | "carlito"
        | "liberation sans" => "sans",
        "times new roman" | "times" | "cambria" | "georgia" | "garamond" | "book antiqua" | "palatino linotype"
        | "liberation serif" | "caladea" => "serif",
        "courier new" | "courier" | "consolas" | "liberation mono" => "mono",
        _ => return None,
    })
}

pub struct Font {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub family: String,
    pub ps_name: String,
    pub upem: f64,
    pub ascent: f64,
    pub descent: f64, // negative, font units
    pub gap: f64,
    pub cap_height: f64,
    pub italic_angle: f64,
    pub bbox: [f64; 4],
    pub fixed: bool,
    pub weight: u16,
    pub italic: bool,
    cmap: HashMap<u32, u16>,
    advances: Vec<u16>,
}

impl Font {
    fn load(data: Arc<Vec<u8>>, index: u32) -> Option<Font> {
        let face = ttf_parser::Face::parse(&data, index).ok()?;
        if face.tables().glyf.is_none() {
            return None; // CFF outlines: not used
        }
        if matches!(face.permissions(), Some(ttf_parser::Permissions::Restricted)) {
            return None;
        }
        let mut cmap = HashMap::new();
        if let Some(t) = face.tables().cmap {
            for sub in t.subtables {
                if !sub.is_unicode() && !(sub.platform_id == ttf_parser::PlatformId::Windows && sub.encoding_id == 0) {
                    continue;
                }
                let symbol = sub.platform_id == ttf_parser::PlatformId::Windows && sub.encoding_id == 0;
                sub.codepoints(|cp| {
                    if let Some(g) = sub.glyph_index(cp) {
                        if g.0 != 0 {
                            cmap.entry(cp).or_insert(g.0);
                            if symbol && (0xF000..=0xF0FF).contains(&cp) {
                                cmap.entry(cp - 0xF000).or_insert(g.0);
                            }
                        }
                    }
                });
            }
        }
        let n = face.number_of_glyphs();
        let advances = (0..n).map(|g| face.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or(0)).collect();
        let name = |id: u16| -> Option<String> {
            face.names().into_iter().filter(|n| n.name_id == id && n.is_unicode()).find_map(|n| n.to_string())
        };
        let family = name(16).or_else(|| name(1)).unwrap_or_else(|| "Font".into());
        let ps: String = name(6).unwrap_or_else(|| family.clone()).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let bb = face.global_bounding_box();
        let (mut ascent, mut descent, mut gap) =
            (face.ascender() as f64, face.descender() as f64, face.line_gap() as f64);
        if let Some(os2) = face.tables().os2 {
            if os2.use_typographic_metrics() {
                ascent = os2.typographic_ascender() as f64;
                descent = os2.typographic_descender() as f64;
                gap = os2.typographic_line_gap() as f64;
            }
        }
        Some(Font {
            index,
            family,
            ps_name: if ps.is_empty() { "Font".into() } else { ps },
            upem: face.units_per_em() as f64,
            ascent,
            descent,
            gap: gap.max(0.0),
            cap_height: face.capital_height().map(|h| h as f64).unwrap_or(ascent * 0.7),
            italic_angle: face.italic_angle() as f64,
            bbox: [bb.x_min as f64, bb.y_min as f64, bb.x_max as f64, bb.y_max as f64],
            fixed: face.is_monospaced(),
            weight: face.weight().to_number(),
            italic: face.is_italic(),
            cmap,
            advances,
            data: data.clone(),
        })
    }

    pub fn has(&self, ch: char) -> bool {
        self.cmap.contains_key(&(ch as u32))
    }

    pub fn gid(&self, ch: char) -> u16 {
        *self.cmap.get(&(ch as u32)).unwrap_or(&0)
    }

    pub fn advance(&self, gid: u16) -> f64 {
        *self.advances.get(gid as usize).unwrap_or(&0) as f64
    }

    /// Width of a character in thousandths of the font size.
    pub fn char_width(&self, ch: char) -> f64 {
        self.advance(self.gid(ch)) * 1000.0 / self.upem
    }

    pub fn width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.char_width(c)).sum::<f64>() * size / 1000.0
    }

    /// (ascent, descent, line gap) in points; descent is positive.
    pub fn line_metrics(&self, size: f64) -> (f64, f64, f64) {
        let k = size / self.upem;
        (self.ascent * k, -self.descent * k, self.gap * k)
    }
}

struct Registry {
    db: fontdb::Database,
    loaded: HashMap<(String, u32), Option<Arc<Font>>>,
    fallbacks: Option<Vec<Arc<Font>>>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Mutex::new(Registry { db, loaded: HashMap::new(), fallbacks: None })
    })
}

fn load_face(reg: &mut Registry, id: fontdb::ID) -> Option<Arc<Font>> {
    let face = reg.db.face(id)?;
    let key = (format!("{:?}", face.source), face.index);
    if let Some(f) = reg.loaded.get(&key) {
        return f.clone();
    }
    let index = face.index;
    let data: Option<Arc<Vec<u8>>> = match &face.source {
        fontdb::Source::File(path) => std::fs::read(path).ok().map(Arc::new),
        fontdb::Source::Binary(b) => Some(Arc::new(b.as_ref().as_ref().to_vec())),
        fontdb::Source::SharedFile(path, _) => std::fs::read(path).ok().map(Arc::new),
    };
    let font = data.and_then(|d| Font::load(d, index)).map(Arc::new);
    reg.loaded.insert(key, font.clone());
    font
}

/// The closest installed face of a family: (font, is bold, is italic).
fn find_in(reg: &mut Registry, family: &str, bold: bool, italic: bool) -> Option<(Arc<Font>, bool, bool)> {
    let want = family.trim().to_lowercase();
    let mut faces: Vec<(i32, fontdb::ID, bool, bool)> = reg
        .db
        .faces()
        .filter(|f| f.families.iter().any(|(n, _)| n.to_lowercase() == want))
        .map(|f| {
            let is_italic = f.style != fontdb::Style::Normal;
            let w = f.weight.0 as i32;
            let target = if bold { 700 } else { 400 };
            let condensed = f.stretch != fontdb::Stretch::Normal;
            let score = (w - target).abs() + if is_italic == italic { 0 } else { 1000 } + if condensed { 50 } else { 0 };
            (score, f.id, w >= 600, is_italic)
        })
        .collect();
    faces.sort_by_key(|f| f.0);
    for (_, id, b, i) in faces {
        if let Some(font) = load_face(reg, id) {
            return Some((font, b, i));
        }
    }
    None
}

/// A font for a family (or its generic stand-in "serif", "sans", "mono") plus the styles that must be
/// faked: (font, fake bold, fake italic).
pub fn resolve(family: Option<&str>, bold: bool, italic: bool, generic: &str) -> Option<(Arc<Font>, bool, bool)> {
    let mut reg = registry().lock().unwrap();
    let mut names: Vec<String> = vec![];
    let mut generic = generic.to_string();
    if let Some(f) = family {
        names.push(f.to_string());
        if let Some(g) = alias(f) {
            generic = g.to_string();
        }
    }
    let list: &[&str] = match generic.as_str() {
        "mono" => &MONO,
        "sans" => &SANS,
        _ => &SERIF,
    };
    names.extend(list.iter().map(|s| s.to_string()));
    names.extend(SANS.iter().map(|s| s.to_string()));
    names.extend(SERIF.iter().map(|s| s.to_string()));
    for name in names {
        if let Some((f, b, i)) = find_in(&mut reg, &name, bold, italic) {
            return Some((f, bold && !b, italic && !i));
        }
    }
    let ids: Vec<fontdb::ID> = reg.db.faces().map(|f| f.id).collect();
    for id in ids {
        if let Some(f) = load_face(&mut reg, id) {
            return Some((f, bold, italic));
        }
    }
    None
}

/// Fonts tried, in order, for characters the chosen font does not have.
pub fn fallbacks() -> Vec<Arc<Font>> {
    let mut reg = registry().lock().unwrap();
    if let Some(f) = &reg.fallbacks {
        return f.clone();
    }
    let mut out = vec![];
    for name in FALLBACK {
        if let Some((f, _, _)) = find_in(&mut reg, name, false, false) {
            if !out.iter().any(|x: &Arc<Font>| Arc::ptr_eq(x, &f)) {
                out.push(f);
            }
        }
    }
    reg.fallbacks = Some(out.clone());
    out
}

/// Every installed family name, sorted.
pub fn families() -> Vec<String> {
    let reg = registry().lock().unwrap();
    let mut names: Vec<String> =
        reg.db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}
