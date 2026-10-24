//! Names of the text tags that hold a formatted document in the editor buffer.
//!
//! Character formatting lives in tags named "f:..." (f:b, f:i, f:size=14, f:link=https://...), each
//! paragraph's formatting in one tag "P:style|align|list kind|level|indent|lang", and a line break inside
//! a paragraph is a newline tagged "br". Fonts are drawn by "F:family|size|bold|italic" tags worked out
//! from the other two.

use rewrite_core::flat::Line;
use rewrite_core::model::Run;

pub const OBJ: char = '\u{FFFC}';
pub const HEADINGS: [&str; 8] = ["title", "subtitle", "h1", "h2", "h3", "h4", "h5", "h6"];
pub const HIGHLIGHT: [(&str, &str); 15] = [
    ("yellow", "#FFF59D"),
    ("green", "#C8E6C9"),
    ("cyan", "#B2EBF2"),
    ("magenta", "#F8BBD0"),
    ("blue", "#BBDEFB"),
    ("red", "#FFCDD2"),
    ("darkBlue", "#9FA8DA"),
    ("darkCyan", "#80CBC4"),
    ("darkGreen", "#A5D6A7"),
    ("darkMagenta", "#CE93D8"),
    ("darkRed", "#EF9A9A"),
    ("darkYellow", "#FFE082"),
    ("darkGray", "#BDBDBD"),
    ("lightGray", "#EEEEEE"),
    ("black", "#9E9E9E"),
];

pub fn highlight(name: &str) -> &str {
    HIGHLIGHT.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or(if name.starts_with('#') { name } else { "#FFF59D" })
}

/// A paragraph's attributes, as held in its P: tag.
#[derive(Clone, Debug, PartialEq)]
pub struct PAttrs {
    pub style: String,
    pub align: Option<String>,
    pub kind: Option<String>,
    pub level: u32,
    pub indent: u32,
    pub lang: String,
}

impl Default for PAttrs {
    fn default() -> Self {
        PAttrs { style: "normal".into(), align: None, kind: None, level: 0, indent: 0, lang: String::new() }
    }
}

impl PAttrs {
    pub fn tag(&self) -> String {
        format!(
            "P:{}|{}|{}|{}|{}|{}",
            self.style,
            self.align.as_deref().unwrap_or(""),
            self.kind.as_deref().unwrap_or(""),
            self.level,
            self.indent,
            self.lang
        )
    }
    pub fn of_line(l: &Line) -> PAttrs {
        PAttrs {
            style: if l.style.is_empty() { "normal".into() } else { l.style.clone() },
            align: l.align.clone().filter(|a| !a.is_empty()),
            kind: l.list.as_ref().map(|(k, _)| k.clone()).filter(|k| !k.is_empty()),
            level: l.list.as_ref().map_or(0, |(_, n)| *n),
            indent: l.indent,
            lang: l.lang.clone(),
        }
    }
    pub fn line(&self) -> Line {
        Line {
            style: self.style.clone(),
            align: self.align.clone(),
            list: self.kind.clone().map(|k| (k, self.level)),
            indent: self.indent,
            lang: self.lang.clone(),
        }
    }
}

pub fn ptag_default() -> String {
    PAttrs::default().tag()
}

pub fn pattrs(tag: &str) -> PAttrs {
    let body = tag.strip_prefix("P:").unwrap_or(tag);
    let mut parts: Vec<&str> = body.splitn(6, '|').collect();
    parts.resize(6, "");
    let opt = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
    PAttrs {
        style: if parts[0].is_empty() { "normal".into() } else { parts[0].to_string() },
        align: opt(parts[1]),
        kind: opt(parts[2]),
        level: parts[3].parse().unwrap_or(0),
        indent: parts[4].parse().unwrap_or(0),
        lang: parts[5].to_string(),
    }
}

/// A number as Python's "%g" would write it for the sizes used here (14.0 -> "14", 10.5 -> "10.5").
pub fn num_g(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('e') || s.contains("inf") || s.contains("NaN") {
        return format!("{v:e}");
    }
    s
}

/// The f: tags for a Run's formatting.
pub fn ftags(r: &Run) -> Vec<String> {
    let mut out = vec![];
    for (k, on) in [("b", r.b), ("i", r.i), ("u", r.u), ("s", r.s), ("sup", r.sup), ("sub", r.sub), ("code", r.code)] {
        if on {
            out.push(format!("f:{k}"));
        }
    }
    if let Some(v) = r.size {
        out.push(format!("f:size={}", num_g(v)));
    }
    for (k, v) in [("font", &r.font), ("color", &r.color), ("hl", &r.hl), ("link", &r.link)] {
        if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
            out.push(format!("f:{k}={v}"));
        }
    }
    out
}

/// The Run fields that some f: tags stand for.
pub fn fmt_of<S: AsRef<str>>(tags: &[S]) -> Run {
    let mut r = Run::default();
    for t in tags {
        let Some(body) = t.as_ref().strip_prefix("f:") else { continue };
        let (k, v) = match body.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (body, None),
        };
        match (k, v) {
            ("b", None) => r.b = true,
            ("i", None) => r.i = true,
            ("u", None) => r.u = true,
            ("s", None) => r.s = true,
            ("sup", None) => r.sup = true,
            ("sub", None) => r.sub = true,
            ("code", None) => r.code = true,
            ("size", Some(v)) => {
                if let Ok(x) = v.parse::<f64>() {
                    r.size = Some(x)
                }
            }
            ("font", Some(v)) => r.font = Some(v.to_string()),
            ("color", Some(v)) => r.color = Some(v.to_string()),
            ("hl", Some(v)) => r.hl = Some(v.to_string()),
            ("link", Some(v)) => r.link = Some(v.to_string()),
            _ => {}
        }
    }
    r
}

/// The value of one formatting key in a Run: Some("") for a switched-on flag, Some(v) for a value.
pub fn fmt_get(r: &Run, key: &str) -> Option<String> {
    let flag = |b: bool| if b { Some(String::new()) } else { None };
    match key {
        "b" => flag(r.b),
        "i" => flag(r.i),
        "u" => flag(r.u),
        "s" => flag(r.s),
        "sup" => flag(r.sup),
        "sub" => flag(r.sub),
        "code" => flag(r.code),
        "size" => r.size.map(num_g),
        "font" => r.font.clone().filter(|v| !v.is_empty()),
        "color" => r.color.clone().filter(|v| !v.is_empty()),
        "hl" => r.hl.clone().filter(|v| !v.is_empty()),
        "link" => r.link.clone().filter(|v| !v.is_empty()),
        _ => None,
    }
}

/// Set (Some) or clear (None) one formatting key; flags take any Some as on.
pub fn fmt_set(r: &mut Run, key: &str, v: Option<String>) {
    let on = v.is_some();
    match key {
        "b" => r.b = on,
        "i" => r.i = on,
        "u" => r.u = on,
        "s" => r.s = on,
        "sup" => r.sup = on,
        "sub" => r.sub = on,
        "code" => r.code = on,
        "size" => r.size = v.and_then(|s| s.parse().ok()),
        "font" => r.font = v.filter(|s| !s.is_empty()),
        "color" => r.color = v.filter(|s| !s.is_empty()),
        "hl" => r.hl = v.filter(|s| !s.is_empty()),
        "link" => r.link = v.filter(|s| !s.is_empty()),
        _ => {}
    }
}

/// The tags that are part of the document (and of undo).
pub fn keep(tag: &str) -> bool {
    tag.starts_with("f:") || tag.starts_with("P:") || tag == "br"
}

/// A text colour made readable on a dark background.
pub fn lighten(color: &str) -> String {
    let h = color.trim_start_matches('#');
    if h.len() < 6 {
        return color.to_string();
    }
    let p = |k: usize| u8::from_str_radix(&h[k..k + 2], 16).ok();
    let (Some(r), Some(g), Some(b)) = (p(0), p(2), p(4)) else { return color.to_string() };
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    if lum >= 110.0 {
        return color.to_string();
    }
    let f = 0.55;
    let m = |v: u8| (v as f64 + (255.0 - v as f64) * f) as u8;
    format!("#{:02X}{:02X}{:02X}", m(r), m(g), m(b))
}

/// (family, size in points, bold, italic) of an F: tag name.
pub fn font_of(name: &str) -> Option<(String, f64, bool, bool)> {
    let body = name.strip_prefix("F:")?;
    let mut it = body.rsplitn(4, '|');
    let italic = it.next()? == "1";
    let bold = it.next()? == "1";
    let size = it.next()?.parse().ok()?;
    let family = it.next()?.to_string();
    Some((family, size, bold, italic))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        let a = PAttrs { style: "h1".into(), align: Some("center".into()), kind: Some("number".into()), level: 2, indent: 1, lang: "py".into() };
        assert_eq!(pattrs(&a.tag()), a);
        let r = Run { b: true, size: Some(10.5), link: Some("https://x.org/a=b".into()), ..Default::default() };
        let t = ftags(&r);
        assert_eq!(t, vec!["f:b", "f:size=10.5", "f:link=https://x.org/a=b"]);
        assert!(fmt_of(&t).same_format(&r));
        assert_eq!(num_g(14.0), "14");
        assert_eq!(font_of("F:Times New Roman|12|1|0"), Some(("Times New Roman".into(), 12.0, true, false)));
    }
}
