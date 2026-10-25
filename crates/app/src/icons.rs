//! Button icons, drawn at run time so they are sharp at any screen scale and take the theme's colours.
//!
//! An icon is a few strokes on a 24 x 24 grid: `l(&[x1, y1, x2, y2, ...])` is a line through the points,
//! `a(cx, cy, r, from, to)` part of a circle (degrees, 0 = right, 90 = down).

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::collections::HashMap;


const GRID: f64 = 24.0;
const STROKE: f64 = 1.9; // line width, in grid units

#[derive(Clone)]
struct Stroke {
    pts: Vec<(f64, f64)>,
    straight: bool,
}

fn l(xy: &[f64]) -> Stroke {
    Stroke { pts: xy.chunks(2).map(|p| (p[0], p[1])).collect(), straight: true }
}

fn a(cx: f64, cy: f64, r: f64, a0: f64, a1: f64) -> Stroke {
    let n = (((a1 - a0).abs() / 12.0) as usize).max(2);
    let pts = (0..=n)
        .map(|k| {
            let ang = (a0 + (a1 - a0) * k as f64 / n as f64).to_radians();
            (cx + r * ang.cos(), cy + r * ang.sin())
        })
        .collect();
    Stroke { pts, straight: false }
}

fn flip(s: Vec<Stroke>) -> Vec<Stroke> {
    s.into_iter().map(|st| Stroke { pts: st.pts.iter().map(|&(x, y)| (GRID - x, y)).collect(), ..st }).collect()
}

fn vflip(s: Vec<Stroke>) -> Vec<Stroke> {
    s.into_iter().map(|st| Stroke { pts: st.pts.iter().map(|&(x, y)| (x, GRID - y)).collect(), ..st }).collect()
}

fn shift(s: Vec<Stroke>, dx: f64, dy: f64) -> Vec<Stroke> {
    s.into_iter().map(|st| Stroke { pts: st.pts.iter().map(|&(x, y)| (x + dx, y + dy)).collect(), ..st }).collect()
}

/// A rounded bar from (x1, y1) to (x2, y2), r wide on each side.
fn capsule(x1: f64, y1: f64, x2: f64, y2: f64, r: f64) -> Vec<Stroke> {
    let ang = (y2 - y1).atan2(x2 - x1).to_degrees();
    let (nx, ny) = (-ang.to_radians().sin(), ang.to_radians().cos());
    vec![
        a(x2, y2, r, ang - 90.0, ang + 90.0),
        a(x1, y1, r, ang + 90.0, ang + 270.0),
        l(&[x1 + r * nx, y1 + r * ny, x2 + r * nx, y2 + r * ny]),
        l(&[x1 - r * nx, y1 - r * ny, x2 - r * nx, y2 - r * ny]),
    ]
}

/// Text lines: (x1, x2, y) each.
fn lines(rows: &[(f64, f64, f64)]) -> Vec<Stroke> {
    rows.iter().map(|&(x1, x2, y)| l(&[x1, y, x2, y])).collect()
}

fn cat(parts: Vec<Vec<Stroke>>) -> Vec<Stroke> {
    parts.into_iter().flatten().collect()
}

fn eye() -> Vec<Stroke> {
    vec![a(12.0, 15.64, 10.64, -160.0, -20.0), a(12.0, 8.36, 10.64, 20.0, 160.0), a(12.0, 12.0, 3.0, 0.0, 360.0)]
}
fn page() -> Vec<Stroke> {
    vec![l(&[6., 3., 14.5, 3., 19., 7.5, 19., 21., 6., 21., 6., 3.]), l(&[14.5, 3., 14.5, 7.5, 19., 7.5])]
}
fn two() -> Vec<Stroke> {
    vec![l(&[15., 3.5, 16., 2.6, 18.6, 2.6, 19.6, 3.6, 19.6, 4.8, 15., 9., 20., 9.])]
}
fn undo() -> Vec<Stroke> {
    vec![l(&[9., 14., 4., 9., 9., 4.]), l(&[4., 9., 14., 9.]), a(14., 15., 6., -90., 0.), l(&[20., 15., 20., 19.])]
}
fn refresh() -> Vec<Stroke> {
    vec![a(11.5, 12.5, 8., 20., 318.), l(&[17.45, 7.15, 20.5, 9.9]), l(&[20.5, 4.4, 20.5, 9.9, 15., 9.9])]
}
fn up() -> Vec<Stroke> {
    vec![l(&[12., 20., 12., 5.]), l(&[6., 11., 12., 5., 18., 11.])]
}

fn strokes(name: &str) -> Vec<Stroke> {
    match name {
        "open" => vec![l(&[3., 6., 9., 6., 11., 9., 21., 9., 21., 19., 3., 19., 3., 6.])],
        "save" => vec![l(&[4., 4., 16., 4., 20., 8., 20., 20., 4., 20., 4., 4.]), l(&[8., 4., 8., 9., 15., 9.]), l(&[8., 20., 8., 14., 16., 14., 16., 20.])],
        "undo" => undo(),
        "redo" => flip(undo()),
        "pencil" => vec![l(&[4., 20., 4., 16., 15., 5., 19., 9., 8., 20., 4., 20.]), l(&[13., 7., 17., 11.])],
        "wand" => vec![l(&[3., 21., 13., 11.]), l(&[17., 3., 17., 11.]), l(&[13., 7., 21., 7.]), l(&[20., 15., 20., 19.]), l(&[18., 17., 22., 17.]),
                       l(&[7.5, 3.5, 7.5, 6.5]), l(&[6., 5., 9., 5.])],
        "eye" => eye(),
        "eye-off" => cat(vec![eye(), vec![l(&[4., 3.5, 20., 20.5])]]),
        "bulb" => vec![a(12., 9., 6., 140., 400.), l(&[16.6, 12.86, 15., 15.5, 9., 15.5, 7.4, 12.86]), l(&[9.5, 18.5, 14.5, 18.5]), l(&[10.5, 21.5, 13.5, 21.5])],
        "diff" => vec![l(&[12., 4., 12., 13.]), l(&[7.5, 8.5, 16.5, 8.5]), l(&[7.5, 19., 16.5, 19.])],
        "moon" => vec![a(12., 12., 9., 5., 265.), a(16.84, 7.16, 7., 53.5, 216.5)],
        "sun" => {
            let mut v = vec![a(12., 12., 4., 0., 360.)];
            for k in 0..8 {
                let ang = (45.0 * k as f64).to_radians();
                v.push(l(&[12. + 7.2 * ang.cos(), 12. + 7.2 * ang.sin(), 12. + 9.8 * ang.cos(), 12. + 9.8 * ang.sin()]));
            }
            v
        }
        "close" => vec![l(&[6., 6., 18., 18.]), l(&[18., 6., 6., 18.])],
        "plus" => vec![l(&[12., 5., 12., 19.]), l(&[5., 12., 19., 12.])],
        "minus" => vec![l(&[5., 12., 19., 12.])],
        "refresh" => refresh(),
        "revert" => flip(refresh()),
        "check" => vec![l(&[5., 12.5, 10., 17.5, 19., 7.])],
        "check-all" => vec![l(&[17., 6., 7., 17., 2., 12.]), l(&[22., 10., 14.5, 17.5, 13., 16.])],
        "next" => vec![l(&[9., 5., 16., 12., 9., 19.])],
        "up" => up(),
        "down" => vflip(up()),
        "chev-right" => vec![l(&[10., 7., 15., 12., 10., 17.])],
        "chev-down" => vec![l(&[7., 10., 12., 15., 17., 10.])],
        "search" => vec![a(10.5, 10.5, 6.5, 0., 360.), l(&[15.5, 15.5, 20.5, 20.5])],
        "book-plus" => vec![l(&[5., 4., 19., 4., 19., 20., 5., 20., 5., 4.]), l(&[8.5, 4., 8.5, 20.]), l(&[13.75, 9., 13.75, 15.]), l(&[10.75, 12., 16.75, 12.])],
        "trash" => vec![l(&[4., 7., 20., 7.]), l(&[6.5, 7., 7.5, 20., 16.5, 20., 17.5, 7.]), l(&[9.5, 7., 9.5, 4., 14.5, 4., 14.5, 7.])],
        // text formatting
        "bold" => vec![l(&[7., 4., 7., 20.]), l(&[7., 4., 12.5, 4.]), a(12.5, 8., 4., -90., 90.), l(&[12.5, 12., 7., 12.]), a(13.5, 16., 4., -90., 90.),
                       l(&[13.5, 20., 7., 20.]), l(&[9., 4., 9., 20.])],
        "italic" => vec![l(&[10., 4., 18., 4.]), l(&[6., 20., 14., 20.]), l(&[14., 4., 10., 20.])],
        "underline" => vec![l(&[7., 3.5, 7., 11.]), a(12., 11., 5., 180., 0.), l(&[17., 11., 17., 3.5]), l(&[5., 21., 19., 21.])],
        "strike" => vec![a(12., 7.5, 3.8, -20., -270.), a(12., 15.1, 3.8, -90., 160.), l(&[4., 11.3, 20., 11.3])],
        "sup" => cat(vec![vec![l(&[3.5, 9., 11.5, 20.]), l(&[11.5, 9., 3.5, 20.])], two()]),
        "sub" => cat(vec![vec![l(&[3.5, 4., 11.5, 15.]), l(&[11.5, 4., 3.5, 15.])], shift(two(), 0., 12.4)]),
        "color" => vec![l(&[6., 17., 12., 3.5, 18., 17.]), l(&[8.3, 12., 15.7, 12.]), l(&[3.5, 21., 20.5, 21.])],
        "highlight" => vec![l(&[14., 3.5, 20., 9.5, 11., 18.5, 5., 12.5, 14., 3.5]), l(&[5., 12.5, 3.5, 17.5, 6., 20., 11., 18.5]), l(&[3., 21.5, 21., 21.5])],
        "link" => cat(vec![capsule(5.5, 18.5, 10.2, 13.8, 3.), capsule(13.8, 10.2, 18.5, 5.5, 3.), vec![l(&[9.5, 14.5, 14.5, 9.5])]]),
        "clear-format" => vec![l(&[4., 4., 15., 4.]), l(&[9.5, 4., 9.5, 16.]), l(&[14., 14., 20.5, 20.5]), l(&[20.5, 14., 14., 20.5])],
        "align-left" => lines(&[(4., 20., 5.), (4., 14., 9.5), (4., 20., 14.), (4., 14., 18.5)]),
        "align-center" => lines(&[(4., 20., 5.), (7., 17., 9.5), (4., 20., 14.), (7., 17., 18.5)]),
        "align-right" => lines(&[(4., 20., 5.), (10., 20., 9.5), (4., 20., 14.), (10., 20., 18.5)]),
        "align-justify" => lines(&[(4., 20., 5.), (4., 20., 9.5), (4., 20., 14.), (4., 20., 18.5)]),
        "list-bullet" => cat(vec![vec![a(5., 6., 1.2, 0., 360.), a(5., 12., 1.2, 0., 360.), a(5., 18., 1.2, 0., 360.)],
                                  lines(&[(9., 20., 6.), (9., 20., 12.), (9., 20., 18.)])]),
        "list-number" => cat(vec![vec![l(&[3.5, 4.2, 5.2, 3., 5.2, 9.]), l(&[3.4, 15.2, 4.2, 14.4, 5.8, 14.4, 6.6, 15.2, 6.6, 16.2, 3.4, 20., 7., 20.])],
                                  lines(&[(10., 20., 6.), (10., 20., 17.)])]),
        "indent" => cat(vec![lines(&[(11., 20., 5.), (11., 20., 10.), (11., 20., 15.), (4., 20., 20.)]), vec![l(&[4., 8., 7.5, 11.5, 4., 15.])]]),
        "outdent" => cat(vec![lines(&[(11., 20., 5.), (11., 20., 10.), (11., 20., 15.), (4., 20., 20.)]), vec![l(&[7.5, 8., 4., 11.5, 7.5, 15.])]]),
        "style" => vec![l(&[4., 6., 4., 4., 14., 4., 14., 6.]), l(&[9., 4., 9., 16.]), l(&[15., 13., 15., 11., 21., 11., 21., 13.]), l(&[18., 11., 18., 20.])],
        // inserting things
        "image" => vec![l(&[3., 5., 21., 5., 21., 19., 3., 19., 3., 5.]), l(&[3., 16., 9., 10.5, 13., 14.5, 16., 11.5, 21., 16.5]), a(15.5, 8.8, 1.5, 0., 360.)],
        "table" => vec![l(&[3., 5., 21., 5., 21., 19., 3., 19., 3., 5.]), l(&[3., 9.7, 21., 9.7]), l(&[3., 14.3, 21., 14.3]), l(&[9., 5., 9., 19.]), l(&[15., 5., 15., 19.])],
        "chart" => vec![l(&[3., 20.5, 21., 20.5]), l(&[4.5, 20.5, 4.5, 13., 7.5, 13., 7.5, 20.5]), l(&[10.5, 20.5, 10.5, 7., 13.5, 7., 13.5, 20.5]),
                        l(&[16.5, 20.5, 16.5, 10.5, 19.5, 10.5, 19.5, 20.5])],
        "sigma" => vec![l(&[18., 5.5, 18., 4., 6., 4., 12.5, 12., 6., 20., 18., 20., 18., 18.5])],
        "footnote" => cat(vec![lines(&[(3., 13., 7.), (3., 21., 12.), (3., 21., 16.5), (3., 14., 21.)]), vec![l(&[16., 4.2, 18., 2.8, 18., 9.])]]),
        "rule" => vec![l(&[3., 12., 21., 12.]), l(&[3., 8.5, 3., 15.5]), l(&[21., 8.5, 21., 15.5])],
        "page-break" => vec![l(&[5., 3., 5., 8., 19., 8., 19., 3.]), l(&[5., 21., 5., 16., 19., 16., 19., 21.]), l(&[3., 12., 6.5, 12.]), l(&[10.25, 12., 13.75, 12.]),
                             l(&[17.5, 12., 21., 12.])],
        "page-setup" => cat(vec![page(), lines(&[(9., 16., 11.5), (9., 16., 15.), (9., 13.5, 18.5)])]),
        "export" => vec![l(&[12., 15., 12., 3.]), l(&[7.5, 7.5, 12., 3., 16.5, 7.5]), l(&[5., 12., 5., 20.5, 19., 20.5, 19., 12.])],
        "file" => page(),
        "convert" => vec![l(&[4., 8., 18.5, 8.]), l(&[14.5, 4., 18.5, 8., 14.5, 12.]), l(&[20., 16., 5.5, 16.]), l(&[9.5, 12., 5.5, 16., 9.5, 20.])],
        // PDF tools
        "pointer" => vec![l(&[6., 3., 6., 18.5, 10., 14.8, 12.8, 21., 15.3, 19.9, 12.5, 13.8, 17.8, 13.8, 6., 3.])],
        "type" => vec![l(&[5., 6., 5., 4., 19., 4., 19., 6.]), l(&[12., 4., 12., 20.]), l(&[9., 20., 15., 20.])],
        "note" => vec![l(&[4., 5., 20., 5., 20., 16., 12., 16., 8., 20., 8., 16., 4., 16., 4., 5.]), l(&[8., 9., 16., 9.]), l(&[8., 12.5, 13.5, 12.5])],
        "draw" => vec![l(&[3., 17., 5.5, 12., 8.5, 9.5, 10.5, 11.5, 10., 16., 12.5, 18., 15.5, 13.5, 17., 9., 19.5, 7., 21., 9.])],
        "eraser" => vec![l(&[9.5, 20., 4., 14.5, 14.5, 4., 21., 10.5, 11.5, 20., 9.5, 20.]), l(&[9., 9.5, 15.5, 16.]), l(&[9.5, 20., 21., 20.])],
        "box" => vec![l(&[4., 5., 20., 5., 20., 19., 4., 19., 4., 5.])],
        "rotate-left" => flip(refresh()),
        "rotate-right" => refresh(),
        "page-add" => cat(vec![page(), vec![l(&[12.5, 10.5, 12.5, 17.5]), l(&[9., 14., 16., 14.])]]),
        "pages" => vec![l(&[8., 3., 20., 3., 20., 17.]), l(&[4., 7., 16., 7., 16., 21., 4., 21., 4., 7.])],
        // window buttons
        "win-min" => vec![l(&[6., 12., 18., 12.])],
        "win-max" => vec![l(&[6.5, 6.5, 17.5, 6.5, 17.5, 17.5, 6.5, 17.5, 6.5, 6.5])],
        "win-restore" => vec![l(&[6.5, 9.5, 14.5, 9.5, 14.5, 17.5, 6.5, 17.5, 6.5, 9.5]), l(&[9.5, 9.5, 9.5, 6.5, 17.5, 6.5, 17.5, 14.5, 14.5, 14.5])],
        "win-close" => vec![l(&[6.5, 6.5, 17.5, 17.5]), l(&[17.5, 6.5, 6.5, 17.5])],
        "merge" => vec![l(&[8., 3., 20., 3., 20., 17.]), l(&[4., 7., 16., 7., 16., 21., 4., 21., 4., 7.]), l(&[10., 11., 10., 17.]), l(&[7., 14., 13., 14.])],
        _ => vec![],
    }
}

/// How much of each pixel the icon covers: size * size values from 0 to 1, row by row.
pub fn mask(name: &str, size: usize) -> Vec<f64> {
    let k = size as f64 / GRID;
    let reach = (STROKE / 2.0 * k).max(0.6) + 0.5; // cover fades out half a pixel beyond the edge of the stroke
    let mut out = vec![0.0f64; size * size];
    for st in strokes(name) {
        let mut pts: Vec<(f64, f64)> = st.pts.iter().map(|&(x, y)| (x * k, y * k)).collect();
        if st.straight {
            // straight lines look cleaner when they start and end on the pixel grid (Python rounds half to even)
            pts = pts.iter().map(|&(x, y)| (round_half_even(x * 2.0) / 2.0, round_half_even(y * 2.0) / 2.0)).collect();
        }
        for w in pts.windows(2) {
            let ((x1, y1), (x2, y2)) = (w[0], w[1]);
            let (dx, dy) = (x2 - x1, y2 - y1);
            let len2 = dx * dx + dy * dy;
            let j0 = ((y1.min(y2) - reach) as i64).max(0) as usize;
            let j1 = (((y1.max(y2) + reach) as i64) + 1).min(size as i64).max(0) as usize;
            let i0 = ((x1.min(x2) - reach) as i64).max(0) as usize;
            let i1 = (((x1.max(x2) + reach) as i64) + 1).min(size as i64).max(0) as usize;
            for j in j0..j1 {
                let py = j as f64 + 0.5 - y1;
                for i in i0..i1 {
                    let px = i as f64 + 0.5 - x1;
                    let t = if len2 > 0.0 { ((px * dx + py * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let cover = reach - (px - t * dx).hypot(py - t * dy);
                    let cell = &mut out[j * size + i];
                    if cover > *cell {
                        *cell = cover.min(1.0);
                    }
                }
            }
        }
    }
    out
}

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - x.signum() } else { r }
}

thread_local! {
    static CACHE: RefCell<HashMap<(String, String, usize), gdk::Texture>> = RefCell::new(HashMap::new());
}

/// The icon as a texture of `px` pixels square, in a #RRGGBB colour.
pub fn texture(name: &str, color: &str, px: usize) -> gdk::Texture {
    let key = (name.to_string(), color.to_string(), px);
    if let Some(t) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return t;
    }
    let (r, g, b) = crate::theme::rgb(color);
    let cover = mask(name, px);
    let mut data = Vec::with_capacity(px * px * 4);
    for a in cover {
        data.extend_from_slice(&[r, g, b, (a * 255.0 + 0.5) as u8]);
    }
    let bytes = glib::Bytes::from_owned(data);
    let tex: gdk::Texture = gdk::MemoryTexture::new(px as i32, px as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, px * 4).upcast();
    CACHE.with(|c| c.borrow_mut().insert(key, tex.clone()));
    tex
}

#[cfg(test)]
pub fn exists(name: &str) -> bool {
    !strokes(name).is_empty()
}

#[cfg(test)]
pub const _ALL: &[&str] = &[
    "open", "save", "undo", "redo", "pencil", "wand", "eye", "eye-off", "bulb", "diff", "moon", "sun", "close", "plus",
    "minus", "refresh", "revert", "check", "check-all", "next", "up", "down", "chev-right", "chev-down", "search",
    "book-plus", "trash", "bold", "italic", "underline", "strike", "sup", "sub", "color", "highlight", "link",
    "clear-format", "align-left", "align-center", "align-right", "align-justify", "list-bullet", "list-number",
    "indent", "outdent", "style", "image", "table", "chart", "sigma", "footnote", "rule", "page-break", "page-setup",
    "export", "file", "convert", "pointer", "type", "note", "draw", "eraser", "box", "rotate-left", "rotate-right",
    "page-add", "pages", "merge", "win-min", "win-max", "win-restore", "win-close",
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_draws() {
        for name in super::_ALL {
            assert!(super::exists(name), "{name}");
            let m = super::mask(name, 24);
            assert!(m.iter().any(|&v| v > 0.5), "{name} is empty");
        }

    }
}
