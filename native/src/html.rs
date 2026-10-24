//! Document -> a standalone web page (pictures embedded, charts as SVG, maths as MathML).

use crate::model::*;
use crate::xml::{esc, esc_attr};
use crate::{charts, imaging, mathtex};
use base64::Engine;

struct W {
    notes: Vec<Vec<Inline>>,
}

impl W {
    fn inline(&mut self, content: &[Inline]) -> String {
        let mut out = String::new();
        for x in merge_runs(content.to_vec()) {
            match &x {
                Inline::Run(r) => {
                    if r.math {
                        let m = mathtex::to_mathml(&r.text, false);
                        out.push_str(&m);
                        continue;
                    }
                    let mut s = esc(&r.text).replace('\n', "<br>");
                    if r.code {
                        s = format!("<code>{s}</code>");
                    }
                    if r.sup {
                        s = format!("<sup>{s}</sup>");
                    }
                    if r.sub {
                        s = format!("<sub>{s}</sub>");
                    }
                    if r.u && r.link.is_none() {
                        s = format!("<u>{s}</u>");
                    }
                    if r.s {
                        s = format!("<s>{s}</s>");
                    }
                    if r.i {
                        s = format!("<em>{s}</em>");
                    }
                    if r.b {
                        s = format!("<strong>{s}</strong>");
                    }
                    let mut css = vec![];
                    if let Some(c) = &r.color {
                        css.push(format!("color:{c}"));
                    }
                    if let Some(h) = &r.hl {
                        css.push(format!("background:{h}"));
                    }
                    if let Some(sz) = r.size {
                        css.push(format!("font-size:{sz}pt"));
                    }
                    if let Some(f) = &r.font {
                        css.push(format!("font-family:'{}'", f.replace('\'', "")));
                    }
                    if !css.is_empty() {
                        s = format!("<span style=\"{}\">{s}</span>", esc_attr(&css.join(";")));
                    }
                    if let Some(l) = &r.link {
                        s = format!("<a href=\"{}\">{s}</a>", esc_attr(l));
                    }
                    out.push_str(&s);
                }
                Inline::Break(_) => out.push_str("<br>"),
                Inline::Image(img) => {
                    let (data, ext) = if matches!(img.ext.as_str(), "png" | "jpeg" | "gif" | "svg" | "webp" | "bmp") {
                        (img.data.clone(), img.ext.clone())
                    } else {
                        match imaging::to_png(&img.data) {
                            Ok(p) => (p, "png".into()),
                            Err(_) => {
                                out.push_str(&format!("[{}]", esc(if img.alt.is_empty() { "picture" } else { &img.alt })));
                                continue;
                            }
                        }
                    };
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    out.push_str(&format!("<img src=\"data:{};base64,{b64}\" alt=\"{}\" style=\"width:{:.0}pt;max-width:100%\">", imaging::mime(&ext), esc_attr(&img.alt), img.width));
                }
                Inline::Chart(c) => out.push_str(&format!("<figure class=\"chart\">{}</figure>", charts::svg(c))),
                Inline::Note(n) => {
                    self.notes.push(n.runs.clone());
                    let k = self.notes.len();
                    out.push_str(&format!("<sup><a href=\"#fn{k}\" id=\"ref{k}\">{k}</a></sup>"));
                }
                Inline::Opaque(o) => out.push_str(&esc(&o.text)),
            }
        }
        out
    }
}

pub fn write(doc: &Document) -> Vec<u8> {
    let mut w = W { notes: vec![] };
    let mut body = String::new();
    let mut stack: Vec<String> = vec![]; // open list tags
    let size = if doc.size > 0.0 { doc.size } else { 11.0 };
    for b in &doc.blocks {
        let level_now = if let Block::Paragraph(p) = b { p.list.as_ref().map(|l| (l.0.clone(), l.1 as usize)) } else { None };
        match &level_now {
            Some((kind, level)) => {
                while stack.len() > level + 1 {
                    body.push_str(&format!("</li></{}>", stack.pop().unwrap()));
                }
                let tag = if kind == "number" { "ol" } else { "ul" };
                if stack.len() == level + 1 && stack.last().map(|s| s.as_str()) != Some(tag) {
                    body.push_str(&format!("</li></{}>", stack.pop().unwrap()));
                }
                if stack.len() == level + 1 {
                    body.push_str("</li>");
                }
                while stack.len() < level + 1 {
                    body.push_str(&format!("<{tag}>"));
                    stack.push(tag.into());
                    if stack.len() < level + 1 {
                        body.push_str("<li>");
                    }
                }
            }
            None => {
                while let Some(t) = stack.pop() {
                    body.push_str(&format!("</li></{t}>"));
                }
            }
        }
        match b {
            Block::Paragraph(p) => {
                let text = w.inline(&p.content);
                let style = p.align.as_ref().map(|a| format!(" style=\"text-align:{a}\"")).unwrap_or_default();
                if level_now.is_some() {
                    body.push_str(&format!("<li{style}>{text}"));
                    continue;
                }
                let tag = match p.style.as_str() {
                    "title" => "h1 class=\"title\"".to_string(),
                    "subtitle" => "p class=\"subtitle\"".into(),
                    "quote" => "blockquote".into(),
                    "caption" => "p class=\"caption\"".into(),
                    s if s.len() == 2 && s.starts_with('h') => s.to_string(),
                    _ => "p".into(),
                };
                let close = tag.split_whitespace().next().unwrap_or("p").to_string();
                let indent = if p.indent > 0 { format!(" style=\"margin-left:{}em\"", p.indent * 2) } else { String::new() };
                body.push_str(&format!("<{tag}{style}{indent}>{text}</{close}>\n"));
            }
            Block::Table(t) => {
                body.push_str("<table>\n");
                for (r, row) in t.rows.iter().enumerate() {
                    let cell = if t.header && r == 0 { "th" } else { "td" };
                    body.push_str("<tr>");
                    for c in 0..t.ncols() {
                        let paras = row.get(c).cloned().unwrap_or_default();
                        let text = paras.iter().map(|p| w.inline(&p.content)).collect::<Vec<_>>().join("<br>");
                        let a = paras.first().and_then(|p| p.align.clone()).map(|a| format!(" style=\"text-align:{a}\"")).unwrap_or_default();
                        body.push_str(&format!("<{cell}{a}>{text}</{cell}>"));
                    }
                    body.push_str("</tr>\n");
                }
                body.push_str("</table>\n");
            }
            Block::CodeBlock(c) => body.push_str(&format!("<pre><code>{}</code></pre>\n", esc(&c.text))),
            Block::MathBlock(m) => body.push_str(&format!("<div class=\"math\">{}</div>\n", mathtex::to_mathml(&m.tex, true))),
            Block::Rule(_) => body.push_str("<hr>\n"),
            Block::PageBreak(_) => body.push_str("<div style=\"page-break-after:always\"></div>\n"),
        }
    }
    while let Some(t) = stack.pop() {
        body.push_str(&format!("</li></{t}>"));
    }
    if !w.notes.is_empty() {
        let notes = std::mem::take(&mut w.notes);
        body.push_str("<hr class=\"notes\"><ol class=\"notes\">");
        for (k, n) in notes.iter().enumerate() {
            let t = w.inline(n);
            body.push_str(&format!("<li id=\"fn{0}\">{t} <a href=\"#ref{0}\">↩</a></li>", k + 1));
        }
        body.push_str("</ol>\n");
    }
    let font = doc.font.clone().map(|f| format!("'{}', ", f.replace('\'', ""))).unwrap_or_default();
    let title = if doc.title.is_empty() { "Document".to_string() } else { doc.title.clone() };
    let html = format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{}</title>
<style>
body {{ font-family: {font}Georgia, 'Times New Roman', serif; font-size: {size}pt; line-height: 1.45; max-width: 46em; margin: 2em auto; padding: 0 1em; color: #1A1A1A; }}
h1, h2, h3, h4, h5, h6 {{ line-height: 1.2; color: #1F3864; }}
h1.title {{ text-align: center; font-weight: normal; font-size: 2.2em; }}
p.subtitle {{ text-align: center; color: #4F6228; font-size: 1.2em; }}
p.caption {{ text-align: center; font-style: italic; color: #44546A; font-size: 0.9em; }}
blockquote {{ border-left: 3px solid #C8C8C8; margin-left: 0; padding-left: 1em; color: #404040; font-style: italic; }}
pre {{ background: #F4F4F4; padding: 0.6em 0.8em; overflow-x: auto; }}
code {{ font-family: Consolas, 'DejaVu Sans Mono', monospace; font-size: 0.92em; }}
table {{ border-collapse: collapse; margin: 1em 0; }}
th, td {{ border: 1px solid #BFBFBF; padding: 0.3em 0.6em; vertical-align: top; }}
th {{ background: #F0F0F0; }}
img {{ max-width: 100%; }}
.math {{ text-align: center; margin: 0.8em 0; }}
figure.chart {{ margin: 1em 0; }}
ol.notes {{ font-size: 0.9em; }}
</style>
</head>
<body>
{body}</body>
</html>
"#, esc(&title));
    html.into_bytes()
}
