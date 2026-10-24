//! Charts: one layout drawn by several back ends (Tk canvas on the Python side, PDF page, SVG), plus Word's
//! native chart XML (with the data workbook, so the chart stays editable in Word) and LaTeX pgfplots code.
//!
//! A chart is laid out once as shapes in points, top-left origin.

use crate::fonts::Font;
use crate::model::Chart;
use crate::pdf::Writer;
use crate::xml::esc;
use std::io::{Cursor, Write};
use std::sync::Arc;

pub const PALETTE: [&str; 10] =
    ["#4472C4", "#ED7D31", "#A5A5A5", "#FFC000", "#5B9BD5", "#70AD47", "#264478", "#9E480E", "#636363", "#997300"];
pub const KINDS: [(&str, &str); 6] =
    [("bar", "Columns"), ("hbar", "Bars"), ("line", "Lines"), ("area", "Area"), ("pie", "Pie"), ("scatter", "Scatter")];
const INK: &str = "#404040";
const GRID: &str = "#D9D9D9";
const AXIS: &str = "#8C8C8C";

pub fn color(i: usize) -> &'static str {
    PALETTE[i % PALETTE.len()]
}

pub fn tint(hex: &str, k: f64) -> String {
    let (r, g, b) = crate::pdf::rgb(hex);
    let f = |c: f64| ((c * 255.0) + (255.0 - c * 255.0) * k) as u8;
    format!("#{:02X}{:02X}{:02X}", f(r), f(g), f(b))
}

/// A short label for a number.
pub fn number(v: f64) -> String {
    if v.abs() >= 1e6 || (v != 0.0 && v.abs() < 1e-3) {
        return format!("{v:.3e}");
    }
    if v.fract() == 0.0 {
        let n = v as i64;
        let s = n.abs().to_string();
        let mut out = String::new();
        for (k, ch) in s.chars().enumerate() {
            if k > 0 && (s.len() - k) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        return if n < 0 { format!("-{out}") } else { out };
    }
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn to_number(s: &str) -> Option<f64> {
    s.replace(',', "").trim().parse::<f64>().ok()
}

pub fn nice_ticks(mut lo: f64, mut hi: f64) -> Vec<f64> {
    if lo == hi {
        if lo != 0.0 {
            lo -= 1.0;
            hi += 1.0;
        } else {
            hi = 1.0;
        }
    }
    let raw = (hi - lo) / 5.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let start = (lo / step + 1e-9).floor() * step;
    let end = (hi / step - 1e-9).ceil() * step;
    let mut ticks = vec![];
    let mut v = start;
    while v <= end + step * 1e-6 && ticks.len() < 50 {
        ticks.push((v * 1e10).round() / 1e10);
        v += step;
    }
    ticks
}

#[derive(Clone, Debug)]
pub enum Shape {
    Rect { x: f64, y: f64, w: f64, h: f64, fill: Option<String>, stroke: Option<String> },
    Line { pts: Vec<(f64, f64)>, color: String, width: f64 },
    Poly { pts: Vec<(f64, f64)>, fill: Option<String>, stroke: Option<String> },
    Dot { x: f64, y: f64, r: f64, fill: String },
    Text { x: f64, y: f64, text: String, size: f64, color: String, anchor: &'static str, bold: bool, angle: f64 },
}

pub type Measure<'a> = &'a dyn Fn(&str, f64, bool) -> f64;

pub fn approx_measure(text: &str, size: f64, bold: bool) -> f64 {
    text.chars().count() as f64 * size * if bold { 0.56 } else { 0.52 }
}

fn clip(text: &str, room: f64, size: f64, m: Measure) -> String {
    if m(text, size, false) <= room || text.chars().count() < 3 {
        return text.to_string();
    }
    let mut t: Vec<char> = text.chars().collect();
    while t.len() > 1 && m(&(t.iter().collect::<String>() + "…"), size, false) > room {
        t.pop();
    }
    t.into_iter().collect::<String>() + "…"
}

fn text(x: f64, y: f64, s: &str, size: f64, color: &str, anchor: &'static str, bold: bool, angle: f64) -> Shape {
    Shape::Text { x, y, text: s.to_string(), size, color: color.into(), anchor, bold, angle }
}

/// The shapes of a chart drawn in a w x h point box.
pub fn layout(chart: &Chart, w: f64, h: f64, m: Measure) -> Vec<Shape> {
    let mut out = vec![Shape::Rect { x: 0.0, y: 0.0, w, h, fill: Some("#FFFFFF".into()), stroke: None }];
    let (pad, fs) = (8.0, 8.5);
    let mut top = pad;
    if !chart.title.is_empty() {
        out.push(text(w / 2.0, top, &chart.title, 12.0, INK, "n", true, 0.0));
        top += 20.0;
    }
    let series = &chart.series;
    let mut cats: Vec<String> = chart.categories.clone();
    let n = series.iter().map(|s| s.1.len()).chain([cats.len()]).max().unwrap_or(0);
    cats.resize(n, String::new());
    let pie = chart.kind == "pie";
    let names: Vec<String> = if pie { cats.clone() } else { series.iter().map(|s| s.0.clone()).collect() };
    let mut right = w - pad;
    if chart.legend && !names.is_empty() && (pie || series.len() > 1 || series.first().is_some_and(|s| !s.0.is_empty())) {
        let lw = (names.iter().map(|x| m(x, fs, false)).fold(0.0, f64::max) + 18.0).min(w * 0.35);
        let mut ly = top + 6.0;
        for (i, nm) in names.iter().enumerate() {
            if ly > h - pad - 10.0 {
                break;
            }
            out.push(Shape::Rect { x: w - pad - lw, y: ly, w: 8.0, h: 8.0, fill: Some(color(i).into()), stroke: None });
            out.push(text(w - pad - lw + 12.0, ly + 4.0, &clip(nm, lw - 14.0, fs, m), fs, INK, "w", false, 0.0));
            ly += 14.0;
        }
        right = w - pad - lw - 10.0;
    }
    if series.is_empty() || n == 0 {
        out.push(text((pad + right) / 2.0, h / 2.0, "No data", fs + 2.0, AXIS, "c", false, 0.0));
        return out;
    }
    if pie {
        out.extend(pie_shapes(&series[0].1, pad, top, right, h - pad, fs));
        return out;
    }
    let nums: Vec<f64> = series.iter().flat_map(|s| s.1.iter().flatten().copied()).collect();
    let (mut lo, mut hi) = if nums.is_empty() {
        (0.0, 1.0)
    } else {
        (nums.iter().cloned().fold(f64::INFINITY, f64::min), nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
    };
    if matches!(chart.kind.as_str(), "bar" | "hbar" | "area") {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    let ticks = nice_ticks(lo, hi);
    let (vlo, vhi) = (ticks[0], *ticks.last().unwrap());
    let labels: Vec<String> = ticks.iter().map(|t| number(*t)).collect();
    let mut left = pad + if chart.y_title.is_empty() { 0.0 } else { 14.0 };
    let mut bottom = h - pad - if chart.x_title.is_empty() { 0.0 } else { 14.0 };
    if chart.kind == "hbar" {
        left += cats.iter().map(|c| m(c, fs, false)).fold(0.0, f64::max) + 6.0;
    } else {
        left += labels.iter().map(|s| m(s, fs, false)).fold(0.0, f64::max) + 6.0;
    }
    bottom -= fs + 6.0;
    if !chart.x_title.is_empty() {
        out.push(text((left + right) / 2.0, h - pad, &chart.x_title, fs, INK, "s", false, 0.0));
    }
    if !chart.y_title.is_empty() {
        out.push(text(pad, (top + bottom) / 2.0, &chart.y_title, fs, INK, "n", false, 90.0));
    }
    let pw = (right - left).max(10.0);
    let ph = (bottom - top).max(10.0);
    let zero = if vlo <= 0.0 && 0.0 <= vhi { 0.0 } else { vlo };
    if chart.kind == "hbar" {
        let vx = |v: f64| left + (v - vlo) / (vhi - vlo) * pw;
        for (t, s) in ticks.iter().zip(&labels) {
            let x = vx(*t);
            out.push(Shape::Line { pts: vec![(x, top), (x, bottom)], color: GRID.into(), width: 0.5 });
            out.push(text(x, bottom + 3.0, s, fs, INK, "n", false, 0.0));
        }
        let band = ph / n as f64;
        let bw = band * 0.75 / series.len() as f64;
        for (j, cat) in cats.iter().enumerate() {
            let y0 = top + j as f64 * band + band * 0.125;
            out.push(text(left - 4.0, top + (j as f64 + 0.5) * band, &clip(cat, left - pad - 6.0, fs, m), fs, INK, "e", false, 0.0));
            for (i, (_, vals)) in series.iter().enumerate() {
                let Some(Some(v)) = vals.get(j) else { continue };
                let (a, b) = if vx(zero) < vx(*v) { (vx(zero), vx(*v)) } else { (vx(*v), vx(zero)) };
                out.push(Shape::Rect { x: a, y: y0 + i as f64 * bw, w: (b - a).max(0.5), h: bw * 0.92, fill: Some(color(i).into()), stroke: None });
            }
        }
        out.push(Shape::Line { pts: vec![(vx(zero), top), (vx(zero), bottom)], color: AXIS.into(), width: 0.8 });
        return out;
    }
    let vy = |v: f64| bottom - (v - vlo) / (vhi - vlo) * ph;
    for (t, s) in ticks.iter().zip(&labels) {
        let y = vy(*t);
        out.push(Shape::Line { pts: vec![(left, y), (right, y)], color: GRID.into(), width: 0.5 });
        out.push(text(left - 4.0, y, s, fs, INK, "e", false, 0.0));
    }
    let base = vy(zero);
    let xs: Vec<Option<f64>> = cats.iter().map(|c| to_number(c)).collect();
    if chart.kind == "scatter" && !xs.is_empty() && xs.iter().all(|x| x.is_some()) {
        let xv: Vec<f64> = xs.iter().map(|x| x.unwrap()).collect();
        let xt = nice_ticks(xv.iter().cloned().fold(f64::INFINITY, f64::min), xv.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
        let (x0, x1) = (xt[0], *xt.last().unwrap());
        let vx = |x: f64| left + (x - x0) / (x1 - x0) * pw;
        for t in &xt {
            out.push(Shape::Line { pts: vec![(vx(*t), top), (vx(*t), bottom)], color: GRID.into(), width: 0.5 });
            out.push(text(vx(*t), bottom + 3.0, &number(*t), fs, INK, "n", false, 0.0));
        }
        for (i, (_, vals)) in series.iter().enumerate() {
            for (x, v) in xv.iter().zip(vals) {
                if let Some(v) = v {
                    out.push(Shape::Dot { x: vx(*x), y: vy(*v), r: 3.0, fill: color(i).into() });
                }
            }
        }
        out.push(Shape::Line { pts: vec![(left, bottom), (right, bottom)], color: AXIS.into(), width: 0.8 });
        return out;
    }
    let band = pw / n as f64;
    let widest = cats.iter().map(|c| m(c, fs, false)).fold(0.0, f64::max);
    let step = ((widest / (band - 4.0).max(1.0)).ceil() as usize).max(1);
    for (j, cat) in cats.iter().enumerate() {
        if j % step == 0 {
            out.push(text(left + (j as f64 + 0.5) * band, bottom + 3.0, &clip(cat, band * step as f64 - 4.0, fs, m), fs, INK, "n", false, 0.0));
        }
    }
    if chart.kind == "bar" {
        let bw = band * 0.75 / series.len() as f64;
        for j in 0..n {
            let x0 = left + j as f64 * band + band * 0.125;
            for (i, (_, vals)) in series.iter().enumerate() {
                let Some(Some(v)) = vals.get(j) else { continue };
                let (a, b) = if vy(*v) < base { (vy(*v), base) } else { (base, vy(*v)) };
                out.push(Shape::Rect { x: x0 + i as f64 * bw, y: a, w: bw * 0.92, h: (b - a).max(0.5), fill: Some(color(i).into()), stroke: None });
            }
        }
    } else {
        let layers: Vec<(usize, Vec<(f64, f64)>)> = series
            .iter()
            .enumerate()
            .map(|(i, (_, vals))| {
                (i, vals.iter().take(n).enumerate().filter_map(|(j, v)| v.map(|v| (left + (j as f64 + 0.5) * band, vy(v)))).collect())
            })
            .collect();
        if chart.kind == "area" {
            // the largest area at the back, so the smaller ones stay visible
            let mut order: Vec<&(usize, Vec<(f64, f64)>)> = layers.iter().collect();
            order.sort_by(|a, b| {
                let mean = |l: &Vec<(f64, f64)>| l.iter().map(|p| p.1).sum::<f64>() / l.len().max(1) as f64;
                mean(&a.1).partial_cmp(&mean(&b.1)).unwrap_or(std::cmp::Ordering::Equal)
            });
            for (i, pts) in order {
                if pts.len() >= 2 {
                    let mut poly = vec![(pts[0].0, base)];
                    poly.extend(pts.iter().copied());
                    poly.push((pts.last().unwrap().0, base));
                    out.push(Shape::Poly { pts: poly, fill: Some(tint(color(*i), 0.35)), stroke: None });
                }
            }
        }
        for (i, pts) in &layers {
            out.push(Shape::Line { pts: pts.clone(), color: color(*i).into(), width: 1.8 });
            if chart.kind == "line" {
                for (x, y) in pts {
                    out.push(Shape::Dot { x: *x, y: *y, r: 2.4, fill: color(*i).into() });
                }
            }
        }
    }
    out.push(Shape::Line { pts: vec![(left, base), (right, base)], color: AXIS.into(), width: 0.8 });
    out
}

fn pie_shapes(vals: &[Option<f64>], left: f64, top: f64, right: f64, bottom: f64, fs: f64) -> Vec<Shape> {
    let mut out = vec![];
    let vals: Vec<f64> = vals.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    let total: f64 = vals.iter().sum();
    let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
    let r = ((right - left).min(bottom - top) / 2.0 - 4.0).max(5.0);
    if total <= 0.0 {
        out.push(text(cx, cy, "No data", fs + 2.0, AXIS, "c", false, 0.0));
        return out;
    }
    let mut a = -90.0f64;
    for (i, v) in vals.iter().enumerate() {
        let sweep = 360.0 * v / total;
        if sweep <= 0.0 {
            continue;
        }
        let steps = ((sweep / 4.0) as usize).max(2);
        let mut pts = vec![(cx, cy)];
        for k in 0..=steps {
            let t = (a + sweep * k as f64 / steps as f64).to_radians();
            pts.push((cx + r * t.cos(), cy + r * t.sin()));
        }
        out.push(Shape::Poly { pts, fill: Some(color(i).into()), stroke: Some("#FFFFFF".into()) });
        if sweep >= 12.0 {
            let mid = (a + sweep / 2.0).to_radians();
            out.push(text(cx + r * 0.65 * mid.cos(), cy + r * 0.65 * mid.sin(), &format!("{:.0}%", 100.0 * v / total), fs, "#FFFFFF", "c", true, 0.0));
        }
        a += sweep;
    }
    out
}

/// Top-left of a tw x th text box whose anchor point is at (0, 0).
pub fn anchor_offset(anchor: &str, tw: f64, th: f64) -> (f64, f64) {
    let dx = match anchor {
        "w" | "nw" | "sw" => 0.0,
        "e" | "ne" | "se" => -tw,
        _ => -tw / 2.0,
    };
    let dy = match anchor {
        "n" | "nw" | "ne" => 0.0,
        "s" | "sw" | "se" => -th,
        _ => -th / 2.0,
    };
    (dx, dy)
}

/// Draw on a PDF page in the box at (x0, y0).
pub fn draw_pdf(w: &mut Writer, page: usize, chart: &Chart, x0: f64, y0: f64, cw: f64, ch: f64,
                font_for: &dyn Fn(bool) -> Option<Arc<Font>>) {
    let measure = |t: &str, size: f64, bold: bool| font_for(bold).map(|f| f.width(t, size)).unwrap_or_else(|| approx_measure(t, size, bold));
    for s in layout(chart, cw, ch, &measure) {
        match s {
            Shape::Rect { x, y, w: rw, h: rh, fill, stroke } => w.rect(page, x0 + x, y0 + y, rw, rh, fill.as_deref(), stroke.as_deref(), 0.5),
            Shape::Line { pts, color, width } => {
                let p: Vec<(f64, f64)> = pts.iter().map(|(x, y)| (x0 + x, y0 + y)).collect();
                w.polyline(page, &p, &color, width)
            }
            Shape::Poly { pts, fill, stroke } => {
                let p: Vec<(f64, f64)> = pts.iter().map(|(x, y)| (x0 + x, y0 + y)).collect();
                w.polygon(page, &p, fill.as_deref(), stroke.as_deref(), 0.6)
            }
            Shape::Dot { x, y, r, fill } => {
                let p: Vec<(f64, f64)> = (0..16)
                    .map(|t| {
                        let a = t as f64 / 8.0 * std::f64::consts::PI;
                        (x0 + x + r * a.cos(), y0 + y + r * a.sin())
                    })
                    .collect();
                w.polygon(page, &p, Some(&fill), Some("#FFFFFF"), 0.5)
            }
            Shape::Text { x, y, text, size, color, anchor, bold, angle } => {
                let Some(f) = font_for(bold) else { continue };
                let tw = f.width(&text, size);
                let (asc, desc, _) = f.line_metrics(size);
                if angle != 0.0 {
                    w.text(page, x0 + x + asc, y0 + y + tw / 2.0, &f, size, &text, &color, 0.0, false, false, 90.0);
                } else {
                    let (dx, dy) = anchor_offset(anchor, tw, asc + desc);
                    w.text(page, x0 + x + dx, y0 + y + dy + asc, &f, size, &text, &color, 0.0, false, false, 0.0);
                }
            }
        }
    }
}

/// The chart as an SVG document.
pub fn svg(chart: &Chart) -> String {
    let (w, h) = (chart.width, chart.height);
    let mut parts = vec![format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}pt" height="{h:.0}pt" viewBox="0 0 {w:.1} {h:.1}" font-family="Segoe UI, Arial, sans-serif">"#)];
    if !chart.title.is_empty() {
        parts.push(format!("<title>{}</title>", esc(&chart.title)));
    }
    let pts = |p: &[(f64, f64)]| p.iter().map(|(x, y)| format!("{x:.2},{y:.2}")).collect::<Vec<_>>().join(" ");
    for s in layout(chart, w, h, &approx_measure) {
        match s {
            Shape::Rect { x, y, w, h, fill, stroke } => parts.push(format!(
                r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="{}"{}/>"#,
                fill.unwrap_or("none".into()), stroke.map(|s| format!(r#" stroke="{s}""#)).unwrap_or_default())),
            Shape::Line { pts: p, color, width } => parts.push(format!(
                r#"<polyline points="{}" fill="none" stroke="{color}" stroke-width="{width}" stroke-linejoin="round" stroke-linecap="round"/>"#, pts(&p))),
            Shape::Poly { pts: p, fill, stroke } => parts.push(format!(
                r#"<polygon points="{}" fill="{}"{}/>"#, pts(&p), fill.unwrap_or("none".into()),
                stroke.map(|s| format!(r#" stroke="{s}" stroke-width="0.6""#)).unwrap_or_default())),
            Shape::Dot { x, y, r, fill } => parts.push(format!(
                r##"<circle cx="{x:.2}" cy="{y:.2}" r="{r}" fill="{fill}" stroke="#FFFFFF" stroke-width="0.5"/>"##)),
            Shape::Text { x, y, text, size, color, anchor, bold, angle } => {
                let (mut ta, mut base) = (
                    match anchor { "w" | "nw" | "sw" => "start", "e" | "ne" | "se" => "end", _ => "middle" },
                    match anchor { "n" | "nw" | "ne" => "hanging", "s" | "sw" | "se" => "text-after-edge", _ => "central" },
                );
                let rot = if angle != 0.0 {
                    ta = "middle";
                    base = "hanging";
                    format!(r#" transform="rotate({} {x:.2} {y:.2})""#, -angle)
                } else {
                    String::new()
                };
                parts.push(format!(
                    r#"<text x="{x:.2}" y="{y:.2}" font-size="{size}" fill="{color}" text-anchor="{ta}" dominant-baseline="{base}"{}{rot}>{}</text>"#,
                    if bold { r#" font-weight="bold""# } else { "" }, esc(&text)));
            }
        }
    }
    parts.push("</svg>".into());
    parts.join("\n")
}

/// The chart as a pgfplots figure (needs \usepackage{pgfplots}).
pub fn pgfplots(chart: &Chart, tex_escape: &dyn Fn(&str) -> String) -> String {
    let cats = &chart.categories;
    if chart.kind == "pie" {
        let vals: Vec<f64> = chart.series.first().map(|s| s.1.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect()).unwrap_or_default();
        let total: f64 = vals.iter().sum::<f64>().max(1e-9);
        let mut lines = vec!["\\begin{tikzpicture}".to_string()];
        let mut a = 90.0;
        for (i, v) in vals.iter().enumerate() {
            let sweep = 360.0 * v / total;
            lines.push(format!("  \\definecolor{{pie{i}}}{{HTML}}{{{}}}", &color(i)[1..]));
            lines.push(format!("  \\fill[pie{i}] (0,0) -- ({a:.2}:2cm) arc ({a:.2}:{:.2}:2cm) -- cycle;", a - sweep));
            let label = cats.get(i).map(|c| tex_escape(c)).unwrap_or_default();
            lines.push(format!("  \\node at ({:.2}:2.6cm) {{\\small {label}}};", a - sweep / 2.0));
            a -= sweep;
        }
        if !chart.title.is_empty() {
            lines.push(format!("  \\node[above] at (0,2.8cm) {{{}}};", tex_escape(&chart.title)));
        }
        lines.push("\\end{tikzpicture}".into());
        return lines.join("\n");
    }
    let mut opts = vec![];
    if !chart.title.is_empty() {
        opts.push(format!("title={{{}}}", tex_escape(&chart.title)));
    }
    if !chart.x_title.is_empty() {
        opts.push(format!("xlabel={{{}}}", tex_escape(&chart.x_title)));
    }
    if !chart.y_title.is_empty() {
        opts.push(format!("ylabel={{{}}}", tex_escape(&chart.y_title)));
    }
    let numeric = chart.kind == "scatter" && cats.iter().all(|c| to_number(c).is_some());
    if !numeric {
        let (key, ticks) = if chart.kind == "hbar" { ("yticklabels", "ytick") } else { ("xticklabels", "xtick") };
        opts.push(format!("{key}={{{}}}", cats.iter().map(|c| format!("{{{}}}", tex_escape(c))).collect::<Vec<_>>().join(",")));
        opts.push(format!("{ticks}={{0,...,{}}}", cats.len().saturating_sub(1)));
    }
    match chart.kind.as_str() {
        "bar" => opts.extend(["ybar".into(), "bar width=8pt".into(), "enlarge x limits=0.15".into()]),
        "hbar" => opts.extend(["xbar".into(), "bar width=6pt".into(), "enlarge y limits=0.15".into()]),
        _ => {}
    }
    if chart.series.len() > 1 && chart.legend {
        opts.push("legend style={font=\\small}".into());
    }
    opts.push("width=0.85\\linewidth".into());
    opts.push("height=6cm".into());
    let mut lines = vec!["\\begin{tikzpicture}".to_string(), format!("\\begin{{axis}}[{}]", opts.join(", "))];
    for (i, (name, vals)) in chart.series.iter().enumerate() {
        let style = match chart.kind.as_str() {
            "bar" | "hbar" => "fill",
            "area" => "fill, opacity=0.5",
            "scatter" => "only marks",
            _ => "mark=*",
        };
        lines.push(format!("\\definecolor{{series{i}}}{{HTML}}{{{}}}", &color(i)[1..]));
        let mut pts = vec![];
        for (j, v) in vals.iter().enumerate() {
            let Some(v) = v else { continue };
            let x = if numeric { to_number(&cats[j]).unwrap_or(j as f64) } else { j as f64 };
            pts.push(if chart.kind == "hbar" { format!("({v},{j})") } else { format!("({x},{v})") });
        }
        let closing = if chart.kind == "area" { " \\closedcycle" } else { "" };
        lines.push(format!("\\addplot[{style}, color=series{i}] coordinates {{{}}}{closing};", pts.join(" ")));
        if chart.series.len() > 1 && chart.legend {
            lines.push(format!("\\addlegendentry{{{}}}", tex_escape(name)));
        }
    }
    lines.push("\\end{axis}".into());
    lines.push("\\end{tikzpicture}".into());
    lines.join("\n")
}

// -- Word (DrawingML charts) ---------------------------------------------------------

const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn col(i: usize) -> String {
    let mut s = String::new();
    let mut i = i + 1;
    while i > 0 {
        let r = (i - 1) % 26;
        i = (i - 1) / 26;
        s.insert(0, (b'A' + r as u8) as char);
    }
    s
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

/// word/charts/chartN.xml for a chart (values cached, so Word and LibreOffice draw it at once).
pub fn chart_xml(chart: &Chart, workbook_rid: Option<&str>) -> String {
    let cats = &chart.categories;
    let n = cats.len();
    let last = n + 1;
    let tx_rich = |t: &str| format!(
        r#"<c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/>"#, esc(t));
    let cat_ref = |numeric: bool| {
        let pts: String = cats.iter().enumerate().map(|(j, c)| format!(r#"<c:pt idx="{j}"><c:v>{}</c:v></c:pt>"#, esc(c))).collect();
        if numeric {
            format!(r#"<c:numRef><c:f>Sheet1!$A$2:$A${last}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{n}"/>{pts}</c:numCache></c:numRef>"#)
        } else {
            format!(r#"<c:strRef><c:f>Sheet1!$A$2:$A${last}</c:f><c:strCache><c:ptCount val="{n}"/>{pts}</c:strCache></c:strRef>"#)
        }
    };
    let val_ref = |i: usize, vals: &[Option<f64>]| {
        let c = col(i + 1);
        let pts: String = vals.iter().take(n).enumerate()
            .filter_map(|(j, v)| v.map(|v| format!(r#"<c:pt idx="{j}"><c:v>{}</c:v></c:pt>"#, fmt_num(v))))
            .collect();
        format!(r#"<c:numRef><c:f>Sheet1!${c}$2:${c}${last}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{n}"/>{pts}</c:numCache></c:numRef>"#)
    };
    let mut sers = vec![];
    for (i, (name, vals)) in chart.series.iter().enumerate() {
        let c = col(i + 1);
        let fill = &color(i)[1..];
        let head = format!(
            r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:strRef><c:f>Sheet1!${c}$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>"#,
            esc(name));
        let (mut sp, mut marker) = match chart.kind.as_str() {
            "line" => (
                format!(r#"<c:spPr><a:ln w="28575" cap="rnd"><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill><a:round/></a:ln></c:spPr>"#),
                format!(r#"<c:marker><c:symbol val="circle"/><c:size val="5"/><c:spPr><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></c:spPr></c:marker>"#),
            ),
            "scatter" => (
                r#"<c:spPr><a:ln w="19050"><a:noFill/></a:ln></c:spPr>"#.to_string(),
                format!(r#"<c:marker><c:symbol val="circle"/><c:size val="5"/><c:spPr><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></c:spPr></c:marker>"#),
            ),
            _ => (format!(r#"<c:spPr><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill></c:spPr>"#), String::new()),
        };
        if chart.kind == "pie" {
            sp = String::new();
            marker = (0..n).map(|j| format!(
                r#"<c:dPt><c:idx val="{j}"/><c:bubble3D val="0"/><c:spPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill></c:spPr></c:dPt>"#,
                &color(j)[1..])).collect();
        }
        let body = if chart.kind == "scatter" {
            format!(r#"<c:xVal>{}</c:xVal><c:yVal>{}</c:yVal><c:smooth val="0"/>"#, cat_ref(true), val_ref(i, vals))
        } else {
            let mut b = format!("<c:cat>{}</c:cat><c:val>{}</c:val>", cat_ref(false), val_ref(i, vals));
            if chart.kind == "line" {
                b.push_str(r#"<c:smooth val="0"/>"#);
            }
            b
        };
        sers.push(format!("{head}{sp}{marker}{body}</c:ser>"));
        if chart.kind == "pie" {
            break;
        }
    }
    let sers = sers.join("");
    let axes = r#"<c:axId val="500000001"/><c:axId val="500000002"/>"#;
    let plot = match chart.kind.as_str() {
        "bar" | "hbar" => format!(
            r#"<c:barChart><c:barDir val="{}"/><c:grouping val="clustered"/><c:varyColors val="0"/>{sers}<c:gapWidth val="80"/>{axes}</c:barChart>"#,
            if chart.kind == "hbar" { "bar" } else { "col" }),
        "line" => format!(r#"<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>{sers}<c:marker val="1"/>{axes}</c:lineChart>"#),
        "area" => format!(r#"<c:areaChart><c:grouping val="standard"/><c:varyColors val="0"/>{sers}{axes}</c:areaChart>"#),
        "scatter" => format!(r#"<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/>{sers}{axes}</c:scatterChart>"#),
        _ => format!(r#"<c:pieChart><c:varyColors val="1"/>{sers}<c:firstSliceAng val="0"/></c:pieChart>"#),
    };
    let axis_title = |t: &str| if t.is_empty() { String::new() } else { format!("<c:title>{}</c:title>", tx_rich(t)) };
    let grid = r#"<c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>"#;
    let ax = if chart.kind == "pie" {
        String::new()
    } else {
        let cat_tag = if chart.kind == "scatter" { "c:valAx" } else { "c:catAx" };
        let (cat_pos, val_pos) = if chart.kind == "hbar" { ("l", "b") } else { ("b", "l") };
        let extra = if cat_tag == "c:catAx" {
            r#"<c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/>"#
        } else {
            ""
        };
        format!(
            r#"<{cat_tag}><c:axId val="500000001"/><c:scaling><c:orientation val="{}"/></c:scaling><c:delete val="0"/><c:axPos val="{cat_pos}"/>{}<c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="500000002"/><c:crosses val="autoZero"/>{extra}</{cat_tag}><c:valAx><c:axId val="500000002"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="{val_pos}"/>{grid}{}<c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="500000001"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx>"#,
            if chart.kind == "hbar" { "maxMin" } else { "minMax" }, axis_title(&chart.x_title), axis_title(&chart.y_title))
    };
    let title = if chart.title.is_empty() {
        r#"<c:autoTitleDeleted val="1"/>"#.to_string()
    } else {
        format!(r#"<c:title>{}</c:title><c:autoTitleDeleted val="0"/>"#, tx_rich(&chart.title))
    };
    let legend = if chart.legend && (chart.kind == "pie" || chart.series.len() > 1) {
        r#"<c:legend><c:legendPos val="r"/><c:overlay val="0"/></c:legend>"#
    } else {
        ""
    };
    let ext = workbook_rid.map(|r| format!(r#"<c:externalData r:id="{r}"><c:autoUpdate val="0"/></c:externalData>"#)).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="{C_NS}" xmlns:a="{A_NS}" xmlns:r="{R_NS}"><c:date1904 val="0"/><c:roundedCorners val="0"/><c:chart>{title}<c:plotArea><c:layout/>{plot}{ax}</c:plotArea>{legend}<c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:spPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:ln><a:noFill/></a:ln></c:spPr><c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="900"/></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr>{ext}</c:chartSpace>"#)
}

/// A small .xlsx with the chart's data (Word opens it for "Edit Data").
pub fn workbook(chart: &Chart) -> Vec<u8> {
    let mut strings: Vec<String> = vec![];
    let mut s = |t: &str| -> usize {
        if let Some(k) = strings.iter().position(|x| x == t) {
            k
        } else {
            strings.push(t.to_string());
            strings.len() - 1
        }
    };
    let mut rows = vec![];
    let mut head = format!(r#"<row r="1"><c r="A1" t="s"><v>{}</v></c>"#, s(" "));
    for (i, (name, _)) in chart.series.iter().enumerate() {
        head.push_str(&format!(r#"<c r="{}1" t="s"><v>{}</v></c>"#, col(i + 1), s(name)));
    }
    head.push_str("</row>");
    rows.push(head);
    for (j, cat) in chart.categories.iter().enumerate() {
        let r = j + 2;
        let first = match to_number(cat).filter(|_| chart.kind == "scatter") {
            Some(v) => format!(r#"<c r="A{r}"><v>{}</v></c>"#, fmt_num(v)),
            None => format!(r#"<c r="A{r}" t="s"><v>{}</v></c>"#, s(cat)),
        };
        let cells: String = chart.series.iter().enumerate()
            .filter_map(|(i, (_, vals))| vals.get(j).copied().flatten().map(|v| format!(r#"<c r="{}{r}"><v>{}</v></c>"#, col(i + 1), fmt_num(v))))
            .collect();
        rows.push(format!(r#"<row r="{r}">{first}{cells}</row>"#));
    }
    let ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let sheet = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="{ns}"><sheetData>{}</sheetData></worksheet>"#, rows.join(""));
    let sst = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="{ns}" count="{n}" uniqueCount="{n}">{}</sst>"#,
        strings.iter().map(|t| format!(r#"<si><t xml:space="preserve">{}</t></si>"#, esc(t))).collect::<String>(), n = strings.len());
    let files: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#.to_string()),
        ("_rels/.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#)),
        ("xl/workbook.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="{ns}" xmlns:r="{rel}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#)),
        ("xl/_rels/workbook.xml.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{rel}/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#)),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/sharedStrings.xml", sst),
    ];
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            let _ = z.start_file(name, opts);
            let _ = z.write_all(data.as_bytes());
        }
        let _ = z.finish();
    }
    buf.into_inner()
}

/// A Chart from Word's chart XML (the cached values), or None when the chart type is not supported.
pub fn from_chart_xml(xml: &str) -> Option<Chart> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let is = |n: &roxmltree::Node, local: &str| n.is_element() && n.tag_name().name() == local && n.tag_name().namespace() == Some(C_NS);
    let plot = doc.descendants().find(|n| is(n, "plotArea"))?;
    let mut node = None;
    let mut kind = String::new();
    for child in plot.children().filter(|n| n.is_element()) {
        let k = match child.tag_name().name() {
            "barChart" | "bar3DChart" => {
                let dir = child.children().find(|n| is(n, "barDir")).and_then(|n| n.attribute("val")).unwrap_or("col");
                if dir == "bar" { "hbar" } else { "bar" }
            }
            "lineChart" | "line3DChart" => "line",
            "areaChart" | "area3DChart" => "area",
            "pieChart" | "pie3DChart" | "doughnutChart" => "pie",
            "scatterChart" => "scatter",
            _ => continue,
        };
        kind = k.to_string();
        node = Some(child);
        break;
    }
    let node = node?;
    let text_of = |el: roxmltree::Node| -> String {
        el.descendants().filter(|n| n.is_element() && n.tag_name().name() == "t" && n.tag_name().namespace() == Some(A_NS))
            .filter_map(|n| n.text()).collect()
    };
    let cache_points = |el: Option<roxmltree::Node>| -> Vec<Option<String>> {
        let Some(el) = el else { return vec![] };
        for cache in el.descendants() {
            if is(&cache, "strCache") || is(&cache, "numCache") || is(&cache, "strLit") || is(&cache, "numLit") {
                let count = cache.children().find(|n| is(n, "ptCount")).and_then(|n| n.attribute("val")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                let mut pts: Vec<(usize, String)> = vec![];
                for pt in cache.children().filter(|n| is(n, "pt")) {
                    let idx = pt.attribute("idx").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let v = pt.children().find(|n| is(n, "v")).and_then(|n| n.text()).unwrap_or("").to_string();
                    pts.push((idx, v));
                }
                let n = pts.iter().map(|p| p.0 + 1).max().unwrap_or(0).max(count);
                let mut out = vec![None; n];
                for (i, v) in pts {
                    out[i] = Some(v);
                }
                return out;
            }
        }
        vec![]
    };
    let mut cats: Vec<String> = vec![];
    let mut series = vec![];
    for ser in node.children().filter(|n| is(n, "ser")) {
        let name = ser.children().find(|n| is(n, "tx")).map(|tx| {
            tx.descendants().find(|n| is(n, "v")).and_then(|v| v.text()).map(String::from).unwrap_or_else(|| text_of(tx))
        }).unwrap_or_default();
        let (cn, vn) = if kind == "scatter" { ("xVal", "yVal") } else { ("cat", "val") };
        let cat_el = ser.children().find(|n| is(n, cn));
        let val_el = ser.children().find(|n| is(n, vn));
        if cats.is_empty() {
            cats = cache_points(cat_el).into_iter().map(|c| c.unwrap_or_default()).collect();
        }
        let vals: Vec<Option<f64>> = cache_points(val_el).into_iter().map(|v| v.and_then(|v| to_number(&v))).collect();
        let nm = if name.is_empty() { format!("Series {}", series.len() + 1) } else { name };
        series.push((nm, vals));
    }
    if cats.is_empty() {
        if let Some((_, v)) = series.first() {
            cats = (1..=v.len()).map(|k| k.to_string()).collect();
        }
    }
    let mut chart = Chart { kind, categories: cats, series, ..Default::default() };
    let root_chart = doc.root_element().children().find(|n| is(n, "chart"));
    if let Some(rc) = root_chart {
        if let Some(t) = rc.children().find(|n| is(n, "title")) {
            chart.title = text_of(t);
        }
        chart.legend = rc.children().any(|n| is(&n, "legend"));
    }
    for ax in plot.children().filter(|n| n.is_element()) {
        let tag = ax.tag_name().name();
        if !matches!(tag, "catAx" | "valAx" | "dateAx") {
            continue;
        }
        let Some(t) = ax.children().find(|n| is(n, "title")) else { continue };
        let pos = ax.children().find(|n| is(n, "axPos")).and_then(|n| n.attribute("val")).unwrap_or("");
        let mut horizontal = pos == "b" || pos == "t";
        if chart.kind == "hbar" {
            horizontal = !horizontal;
        }
        if horizontal {
            chart.x_title = text_of(t);
        } else {
            chart.y_title = text_of(t);
        }
    }
    Some(chart)
}

/// Chart data typed as rows: "Category, Series 1, Series 2". The first row names the series when its
/// cells (after the first) are not numbers.
pub fn parse_table(text: &str) -> (Vec<String>, Vec<(String, Vec<Option<f64>>)>) {
    let mut rows: Vec<Vec<String>> = vec![];
    // only line ends are trimmed: a header row starts with an empty cell (a tab, when pasted from a spreadsheet)
    for line in text.trim_matches(|c| c == '\n' || c == '\r').lines() {
        if line.trim().is_empty() {
            continue;
        }
        let sep = if line.contains('\t') { '\t' } else if line.contains(';') && !line.contains(',') { ';' } else { ',' };
        rows.push(line.split(sep).map(|c| c.trim().to_string()).collect());
    }
    if rows.is_empty() {
        return (vec![], vec![]);
    }
    let mut header = None;
    if rows[0].len() > 1 && rows[0][1..].iter().all(|c| to_number(c).is_none()) {
        header = Some(rows.remove(0));
    }
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    if width < 2 {
        let vals = rows.iter().map(|r| to_number(&r[0])).collect();
        return ((1..=rows.len()).map(|k| k.to_string()).collect(), vec![("Series 1".into(), vals)]);
    }
    let mut names: Vec<String> = header.map(|h| h[1..].to_vec()).unwrap_or_default();
    for k in names.len() + 1..width {
        names.push(format!("Series {k}"));
    }
    let cats = rows.iter().map(|r| r[0].clone()).collect();
    let series = (1..width)
        .map(|k| (names[k - 1].clone(), rows.iter().map(|r| r.get(k).and_then(|c| to_number(c))).collect()))
        .collect();
    (cats, series)
}

pub fn format_table(chart: &Chart) -> String {
    let mut lines = vec![std::iter::once(String::new()).chain(chart.series.iter().map(|s| s.0.clone())).collect::<Vec<_>>().join(", ")];
    for (j, c) in chart.categories.iter().enumerate() {
        let mut cells = vec![c.clone()];
        for (_, vals) in &chart.series {
            cells.push(vals.get(j).copied().flatten().map(|v| number(v).replace(',', "")).unwrap_or_default());
        }
        lines.push(cells.join(", "));
    }
    lines.join("\n")
}

/// One line about a chart, for plain-text exports.
pub fn describe(chart: &Chart) -> String {
    let kind = KINDS.iter().find(|k| k.0 == chart.kind).map(|k| k.1.to_lowercase()).unwrap_or_else(|| chart.kind.clone());
    format!("[Chart: {} ({kind}, {} series)]", if chart.title.is_empty() { "untitled" } else { &chart.title }, chart.series.len())
}

pub fn slug(text: &str) -> String {
    let s: String = text.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "chart".into() } else { s }
}
