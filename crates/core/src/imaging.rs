//! Pictures: format and size from the header, decoding, resizing and PNG/PPM encoding.

use image::{DynamicImage, ImageFormat, RgbaImage};
use std::io::Cursor;

pub const SHOWN: [&str; 6] = ["png", "jpeg", "gif", "bmp", "tiff", "webp"];

pub fn mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "xml" => "application/xml",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

/// The picture format of some bytes: png, jpeg, gif, bmp, tiff, emf, wmf, svg, webp or "".
pub fn sniff(data: &[u8]) -> &'static str {
    let h = &data[..data.len().min(64)];
    if h.starts_with(b"\x89PNG") {
        "png"
    } else if h.starts_with(b"\xff\xd8") {
        "jpeg"
    } else if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        "gif"
    } else if h.starts_with(b"BM") {
        "bmp"
    } else if h.starts_with(b"II*\x00") || h.starts_with(b"MM\x00*") {
        "tiff"
    } else if h.starts_with(b"\x01\x00\x00\x00") && data.len() > 44 && &data[40..44] == b" EMF" {
        "emf"
    } else if h.starts_with(b"\xd7\xcd\xc6\x9a") || h.starts_with(b"\x01\x00\x09\x00") || h.starts_with(b"\x02\x00\x09\x00") {
        "wmf"
    } else if h.starts_with(b"RIFF") && h.len() >= 12 && &h[8..12] == b"WEBP" {
        "webp"
    } else {
        let head = String::from_utf8_lossy(&data[..data.len().min(2000)]).to_lowercase();
        if head.contains("<svg") { "svg" } else { "" }
    }
}

/// "jpg" -> "jpeg", "image/png" -> "png" ...
pub fn ext_for(name: &str) -> String {
    let s = name.to_lowercase();
    let s = s.rsplit('.').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
    match s.as_str() {
        "jpg" | "jpe" => "jpeg".into(),
        "tif" => "tiff".into(),
        "x-emf" => "emf".into(),
        "x-wmf" => "wmf".into(),
        "svg+xml" => "svg".into(),
        _ => s,
    }
}

fn be16(d: &[u8], i: usize) -> Option<u16> {
    d.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], i: usize) -> Option<u32> {
    d.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// (width, height) in pixels.
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    match sniff(data) {
        "emf" => {
            let rd = |i: usize| data.get(i..i + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let (l, t, r, b) = (rd(24)?, rd(28)?, rd(32)?, rd(36)?);
            let w = ((r - l) as f64 / 2540.0 * 96.0).round().max(1.0) as u32;
            let h = ((b - t) as f64 / 2540.0 * 96.0).round().max(1.0) as u32;
            Some((w, h))
        }
        "" | "wmf" | "svg" => None,
        _ => image::ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?.into_dimensions().ok(),
    }
}

/// Resolution stored in the file, or 96.
pub fn dpi(data: &[u8]) -> f64 {
    match sniff(data) {
        "png" => {
            let mut i = 8;
            while i + 12 <= data.len() {
                let (Some(n), tag) = (be32(data, i), &data[i + 4..i + 8]) else { break };
                if tag == b"pHYs" && data.len() >= i + 17 {
                    let x = be32(data, i + 8).unwrap_or(0) as f64;
                    if data[i + 16] == 1 && x > 0.0 {
                        return (x * 0.0254).clamp(30.0, 1200.0);
                    }
                    break;
                }
                if tag == b"IDAT" {
                    break;
                }
                i += 12 + n as usize;
            }
            96.0
        }
        "jpeg" if data.len() > 18 && &data[6..11] == b"JFIF\x00" => {
            let unit = data[13];
            let x = be16(data, 14).unwrap_or(0) as f64;
            match unit {
                1 if x > 1.0 => x.clamp(30.0, 1200.0),
                2 if x > 1.0 => (x * 2.54).clamp(30.0, 1200.0),
                _ => 96.0,
            }
        }
        _ => 96.0,
    }
}

/// The size in points a picture takes at its own resolution, scaled down to max_width points.
pub fn natural_size(data: &[u8], max_width: Option<f64>) -> (f64, f64) {
    let (w, h) = size(data).unwrap_or((300, 200));
    let d = dpi(data);
    let (mut w, mut h) = (w as f64 * 72.0 / d, h as f64 * 72.0 / d);
    if let Some(m) = max_width {
        if w > m {
            h = h * m / w;
            w = m;
        }
    }
    (w, h)
}

pub fn decode(data: &[u8]) -> Result<DynamicImage, String> {
    let fmt = match sniff(data) {
        "png" => ImageFormat::Png,
        "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tiff" => ImageFormat::Tiff,
        "webp" => ImageFormat::WebP,
        other => return Err(format!("cannot decode {} picture", if other.is_empty() { "this" } else { other })),
    };
    image::load_from_memory_with_format(data, fmt).map_err(|e| e.to_string())
}

pub fn encode_png(img: &DynamicImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, ImageFormat::Png);
    out.into_inner()
}

/// Any decodable picture as PNG bytes (PNG is returned as it is).
pub fn to_png(data: &[u8]) -> Result<Vec<u8>, String> {
    if sniff(data) == "png" {
        return Ok(data.to_vec());
    }
    Ok(encode_png(&decode(data)?))
}

/// RGBA pixels on a background colour.
pub fn flatten(img: &RgbaImage, bg: (u8, u8, u8)) -> Vec<u8> {
    let mut out = Vec::with_capacity((img.width() * img.height() * 3) as usize);
    for p in img.pixels() {
        let a = p[3] as u32;
        for (c, b) in [p[0], p[1], p[2]].iter().zip([bg.0, bg.1, bg.2]) {
            out.push(((*c as u32 * a + b as u32 * (255 - a) + 127) / 255) as u8);
        }
    }
    out
}

/// A grey box with a cross, for pictures that cannot be shown.
pub fn placeholder(w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(2), h.max(2));
    let mut img = RgbaImage::from_pixel(w, h, image::Rgba([238, 238, 238, 255]));
    let fg = image::Rgba([170, 170, 170, 255]);
    for x in 0..w {
        img.put_pixel(x, 0, fg);
        img.put_pixel(x, h - 1, fg);
    }
    for y in 0..h {
        img.put_pixel(0, y, fg);
        img.put_pixel(w - 1, y, fg);
    }
    let n = w.max(h) * 2;
    for k in 0..n {
        let t = k as f64 / n as f64;
        let x = (t * (w - 1) as f64) as u32;
        let y = (t * (h - 1) as f64) as u32;
        img.put_pixel(x, y, fg);
        img.put_pixel(x, h - 1 - y, fg);
    }
    img
}

/// The header of a PNG: (width, height, bit depth, colour type, interlaced, palette, transparency, IDAT data).
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub ctype: u8,
    pub interlaced: bool,
    pub palette: Vec<u8>,
    pub trns: bool,
    pub idat: Vec<u8>,
}

pub fn png_info(data: &[u8]) -> Option<PngInfo> {
    let mut i = 8;
    let mut info = PngInfo { width: 0, height: 0, depth: 8, ctype: 2, interlaced: false, palette: vec![], trns: false, idat: vec![] };
    let mut seen = false;
    while i + 8 <= data.len() {
        let n = be32(data, i)? as usize;
        let tag = &data[i + 4..i + 8];
        let body = data.get(i + 8..i + 8 + n)?;
        match tag {
            b"IHDR" => {
                info.width = be32(body, 0)?;
                info.height = be32(body, 4)?;
                info.depth = body[8];
                info.ctype = body[9];
                info.interlaced = body[12] != 0;
                seen = true;
            }
            b"PLTE" => info.palette = body.to_vec(),
            b"tRNS" => info.trns = true,
            b"IDAT" => info.idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + n;
    }
    if seen { Some(info) } else { None }
}

/// Number of colour components of a JPEG and whether Adobe inverted its CMYK values.
pub fn jpeg_components(data: &[u8]) -> (u8, bool) {
    let adobe = data.windows(5).take(4000).any(|w| w == b"Adobe");
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = data[i + 1];
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            return (data[i + 9], adobe);
        }
        if m == 0xD8 || m == 0x01 || m == 0xFF || (0xD0..=0xD7).contains(&m) {
            i += 2;
        } else {
            i += 2 + be16(data, i + 2).unwrap_or(0) as usize;
        }
    }
    (3, adobe)
}
