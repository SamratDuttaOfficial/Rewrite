//! One entry point for every format: read bytes into a Document, write a Document as bytes (plus the
//! side files Markdown and LaTeX refer to, such as pictures).

use crate::md::{self, Files};
use crate::model::*;
use crate::{charts, doc, docx, html, mathtex, odt, rtf, tex};
use std::path::Path;

pub type Sentences<'a> = Option<&'a dyn Fn(&str) -> Vec<usize>>;

pub fn decode_text(data: &[u8]) -> String {
    if data.starts_with(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(&data[3..]).into_owned();
    }
    if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        return crate::xml::decode(data);
    }
    match std::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        Err(_) => data.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// A Windows-1252 byte as a character.
pub fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = ['€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
                              '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ'];
    if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }
}

pub fn read(fmt: &str, data: &[u8], base_dir: Option<&Path>, pdf_figure: Option<tex::PdfFigure>) -> Result<Document, String> {
    Ok(match fmt {
        "md" | "markdown" => md::parse(&decode_text(data), base_dir),
        "tex" | "latex" => tex::parse(&decode_text(data), base_dir, pdf_figure),
        "txt" | "text" | "plain" => Document { blocks: plain_paragraphs(&decode_text(data).replace("\r\n", "\n")), ..Default::default() },
        "docx" | "docm" | "dotx" => docx::read(data)?,
        "odt" | "ott" => odt::read(data, false)?,
        "fodt" => odt::read(data, true)?,
        "rtf" => rtf::read(data),
        "doc" | "dot" => doc::read(data)?,
        _ => return Err(format!("cannot read {fmt} files")),
    })
}

pub fn write(fmt: &str, doc: &Document, stem: &str, sentences: Sentences) -> Result<(Vec<u8>, Files), String> {
    Ok(match fmt {
        "md" | "markdown" => {
            let (t, f) = md::write(doc, stem, sentences);
            (t.into_bytes(), f)
        }
        "tex" | "latex" => {
            let (t, f) = tex::write(doc, stem, sentences);
            (t.into_bytes(), f)
        }
        "txt" | "text" | "plain" => (text(doc, sentences).into_bytes(), Files::new()),
        "docx" => (docx::write(doc), Files::new()),
        "odt" => (odt::write(doc, false), Files::new()),
        "fodt" => (odt::write(doc, true), Files::new()),
        "rtf" => rtf::write(doc),
        "html" | "htm" => (html::write(doc), Files::new()),
        _ => return Err(format!("cannot write {fmt} files")),
    })
}

fn inline_plain(content: &[Inline]) -> String {
    let mut out = String::new();
    for x in content {
        match x {
            Inline::Run(r) if r.math => out.push_str(&mathtex::to_unicode(&r.text)),
            Inline::Run(r) => out.push_str(&r.text),
            Inline::Break(_) => out.push('\n'),
            Inline::Image(i) => out.push_str(&format!("[{}]", if i.alt.is_empty() { "picture" } else { &i.alt })),
            Inline::Chart(c) => out.push_str(&charts::describe(c)),
            Inline::Note(n) => out.push_str(&format!(" ({})", inline_plain(&n.runs))),
            Inline::Opaque(o) => out.push_str(&o.text),
        }
    }
    out
}

/// The document as plain text: paragraphs separated by blank lines, lists marked, tables as columns.
pub fn text(doc: &Document, sentences: Sentences) -> String {
    let labels = list_labels(&doc.blocks);
    let mut out: Vec<String> = vec![];
    let mut prev_list = false;
    for (k, b) in doc.blocks.iter().enumerate() {
        match b {
            Block::Paragraph(p) => {
                let mut t = inline_plain(&p.content);
                if let Some(f) = sentences {
                    if p.list.is_none() && matches!(p.style.as_str(), "normal" | "quote") {
                        t = md::split_lines(&t, &f(&t));
                    }
                }
                if let Some((_, level)) = &p.list {
                    let line = format!("{}{} {}", "  ".repeat(*level as usize), labels[k].clone().unwrap_or_else(|| "-".into()), t);
                    if prev_list {
                        out.last_mut().unwrap().push_str(&format!("\n{line}"));
                    } else {
                        out.push(line);
                    }
                    prev_list = true;
                    continue;
                }
                prev_list = false;
                if t.trim().is_empty() {
                    continue;
                }
                if matches!(p.style.as_str(), "title" | "h1") {
                    let n = t.chars().count().min(80);
                    out.push(format!("{t}\n{}", if p.style == "title" { "=" } else { "-" }.repeat(n)));
                } else {
                    out.push(t);
                }
            }
            Block::Table(t) => {
                prev_list = false;
                let rows: Vec<Vec<String>> = t.rows.iter().map(|r| r.iter().map(|c| c.iter().map(|p| inline_plain(&p.content)).collect::<Vec<_>>().join(" ").replace('\n', " ")).collect()).collect();
                let n = t.ncols();
                let widths: Vec<usize> = (0..n).map(|c| rows.iter().map(|r| r.get(c).map(|s| s.chars().count()).unwrap_or(0)).max().unwrap_or(0).min(40)).collect();
                let lines: Vec<String> = rows.iter().map(|r| (0..n).map(|c| {
                    let s = r.get(c).cloned().unwrap_or_default();
                    format!("{s:<w$}", w = widths[c])
                }).collect::<Vec<_>>().join("  ").trim_end().to_string()).collect();
                out.push(lines.join("\n"));
            }
            Block::CodeBlock(c) => {
                prev_list = false;
                out.push(c.text.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n"));
            }
            Block::MathBlock(m) => {
                prev_list = false;
                out.push(format!("    {}", mathtex::to_unicode(&m.tex)));
            }
            Block::Rule(_) => {
                prev_list = false;
                out.push("-".repeat(40));
            }
            Block::PageBreak(_) => {
                prev_list = false;
                out.push("\u{c}".into());
            }
        }
    }
    out.join("\n\n") + "\n"
}
