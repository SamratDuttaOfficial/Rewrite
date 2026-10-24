//! Opening and saving documents. Mirrors `rewrite/formats.py` and the extension tables of `rewrite/native.py`.
//!
//! Text files (plain, Markdown, LaTeX, HTML, code) are edited as text, keeping their encoding and line
//! endings. Word, OpenDocument and RTF files are edited with their formatting ("rich") as a [`Document`].
//! PDF files open in the PDF view as their bytes.

use crate::model::Document;
use crate::{convert, office};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The kind of text a file holds, by extension; anything else is "plain".
pub const KIND_BY_EXT: &[(&str, &str)] = &[
    (".tex", "latex"), (".latex", "latex"), (".sty", "latex"), (".cls", "latex"), (".bib", "latex"), (".ltx", "latex"),
    (".md", "markdown"), (".markdown", "markdown"), (".mdx", "markdown"), (".rmd", "markdown"), (".qmd", "markdown"),
    (".html", "html"), (".htm", "html"), (".xhtml", "html"), (".xml", "html"), (".svg", "html"),
    (".py", "code"), (".js", "code"), (".ts", "code"), (".jsx", "code"), (".tsx", "code"), (".c", "code"), (".h", "code"),
    (".cpp", "code"), (".hpp", "code"), (".cc", "code"), (".java", "code"), (".go", "code"), (".rs", "code"),
    (".rb", "code"), (".sh", "code"), (".bash", "code"), (".ps1", "code"), (".cs", "code"), (".swift", "code"),
    (".kt", "code"), (".php", "code"), (".r", "code"), (".m", "code"), (".scala", "code"), (".lua", "code"),
    (".pl", "code"), (".css", "code"), (".scss", "code"), (".sql", "code"), (".yaml", "code"), (".yml", "code"),
    (".toml", "code"), (".ini", "code"), (".cfg", "code"),
];

/// Extension -> format name of the readers in [`convert::read`].
pub const READ: &[(&str, &str)] = &[
    (".md", "md"), (".markdown", "md"), (".mdx", "md"), (".rmd", "md"), (".qmd", "md"), (".tex", "tex"), (".latex", "tex"),
    (".ltx", "tex"), (".txt", "txt"), (".text", "txt"), (".docx", "docx"), (".docm", "docx"), (".dotx", "docx"),
    (".odt", "odt"), (".ott", "odt"), (".fodt", "fodt"), (".rtf", "rtf"), (".doc", "doc"), (".dot", "doc"),
];

/// Extension -> format name of the writers in [`convert::write`].
pub const WRITE: &[(&str, &str)] = &[
    (".md", "md"), (".tex", "tex"), (".txt", "txt"), (".docx", "docx"), (".odt", "odt"), (".fodt", "fodt"), (".rtf", "rtf"),
    (".html", "html"), (".htm", "html"),
];

/// Extensions edited with their formatting.
pub const RICH: &[&str] = &[".docx", ".docm", ".dotx", ".odt", ".ott", ".fodt", ".rtf", ".doc", ".dot"];

/// Extension -> format a formatted document is saved in ("doc" goes through LibreOffice).
pub const SAVE_RICH: &[(&str, &str)] = &[
    (".docx", "docx"), (".docm", "docx"), (".dotx", "docx"), (".odt", "odt"), (".ott", "odt"), (".fodt", "fodt"),
    (".rtf", "rtf"), (".doc", "doc"), (".dot", "doc"),
];

pub const KIND_LABEL: &[(&str, &str)] = &[
    ("plain", "Plain text"), ("markdown", "Markdown"), ("latex", "LaTeX"), ("html", "HTML/XML"),
    ("code", "Code (comments only)"), ("rich", "Formatted document"), ("pdf", "PDF"),
];

pub const FORMAT_LABEL: &[(&str, &str)] = &[
    (".docx", "Word"), (".docm", "Word"), (".dotx", "Word template"), (".doc", "Word 97–2003"), (".dot", "Word 97–2003"),
    (".odt", "OpenDocument"), (".ott", "OpenDocument template"), (".fodt", "Flat OpenDocument"),
    (".rtf", "Rich Text"), (".pdf", "PDF"),
];

/// A filter of a file dialog: its label and glob patterns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileType {
    pub label: &'static str,
    pub patterns: &'static [&'static str],
}

const fn ft(label: &'static str, patterns: &'static [&'static str]) -> FileType {
    FileType { label, patterns }
}

pub const OPEN_TYPES: &[FileType] = &[
    ft("All supported", &["*.txt", "*.md", "*.markdown", "*.tex", "*.latex", "*.bib", "*.rst", "*.html", "*.htm",
        "*.docx", "*.docm", "*.dotx", "*.doc", "*.dot", "*.odt", "*.ott", "*.fodt", "*.rtf", "*.pdf", "*.csv", "*.json",
        "*.xml", "*.py", "*.js", "*.ts", "*.c", "*.cpp", "*.java", "*.go", "*.rs", "*.yaml", "*.yml"]),
    ft("Word", &["*.docx", "*.docm", "*.dotx", "*.doc", "*.dot"]),
    ft("OpenDocument / LibreOffice", &["*.odt", "*.ott", "*.fodt"]),
    ft("Rich text", &["*.rtf"]),
    ft("PDF", &["*.pdf"]),
    ft("Text", &["*.txt"]),
    ft("Markdown", &["*.md", "*.markdown"]),
    ft("LaTeX", &["*.tex", "*.latex", "*.bib"]),
    ft("All files", &["*.*"]),
];

pub const SAVE_RICH_TYPES: &[FileType] = &[
    ft("Word document", &["*.docx"]),
    ft("OpenDocument text", &["*.odt"]),
    ft("Flat OpenDocument", &["*.fodt"]),
    ft("Rich Text", &["*.rtf"]),
    ft("Word 97–2003 (needs LibreOffice)", &["*.doc"]),
];

/// The value for `key` in one of the tables above.
pub fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The lower-cased extension with its dot (".tex"), or "" for none.
pub fn ext_of(path: impl AsRef<Path>) -> String {
    path.as_ref().extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

pub fn kind_for(path: impl AsRef<Path>) -> &'static str {
    lookup(KIND_BY_EXT, &ext_of(path)).unwrap_or("plain")
}

/// The kind of a text without a telling extension: LaTeX commands, Markdown headings or lists, else plain.
pub fn sniff_kind(text: &str) -> &'static str {
    static TEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(documentclass|begin\{document\}|section\{|usepackage)").unwrap());
    static MD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(#{1,6} |```|\* |- \[)").unwrap());
    if TEX.is_match(text) {
        "latex"
    } else if MD.is_match(text) {
        "markdown"
    } else {
        "plain"
    }
}

/// An open file. Text kinds keep their text in the editor; a "rich" one starts from `model`; a "pdf" one
/// holds the file's bytes in `data`.
#[derive(Clone, Debug)]
pub struct OpenFile {
    pub text: String,
    pub path: Option<PathBuf>,
    pub kind: String,
    pub encoding: String,
    pub newline: String,
    /// The format of a formatted document or PDF (".docx", ".pdf"); None for text.
    pub source_ext: Option<String>,
    pub model: Option<Document>,
    pub data: Option<Vec<u8>>,
}

impl Default for OpenFile {
    fn default() -> Self {
        OpenFile {
            text: String::new(),
            path: None,
            kind: "plain".into(),
            encoding: "utf-8".into(),
            newline: "\n".into(),
            source_ext: None,
            model: None,
            data: None,
        }
    }
}

impl OpenFile {
    pub fn name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_name()).map_or("Untitled".into(), |n| n.to_string_lossy().into_owned())
    }

    pub fn ext(&self) -> String {
        self.path.as_ref().map(ext_of).unwrap_or_default()
    }
}

// -- loading -------------------------------------------------------------------------

pub fn load(path: impl AsRef<Path>) -> Result<OpenFile, String> {
    let path = path.as_ref();
    let ext = ext_of(path);
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    if RICH.contains(&ext.as_str()) {
        let dir = std::path::absolute(path).ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let model = match convert::read(lookup(READ, &ext).unwrap_or("txt"), &raw, dir.as_deref(), None) {
            Ok(m) => m,
            Err(_) if matches!(ext.as_str(), ".doc" | ".dot") && office::available() => {
                // an old Word file the reader cannot follow: let LibreOffice turn it into .docx first
                convert::read("docx", &office::convert(&raw, &ext, "docx", 180, &|| false)?, None, None)?
            }
            Err(e) => return Err(format!("The file could not be read: {e}")),
        };
        return Ok(OpenFile { path: Some(path.into()), kind: "rich".into(), source_ext: Some(ext), model: Some(model), ..Default::default() });
    }
    if ext == ".pdf" {
        if !raw.starts_with(b"%PDF") && !raw[..raw.len().min(1024)].windows(4).any(|w| w == b"%PDF") {
            return Err("This is not a PDF file.".into());
        }
        return Ok(OpenFile { path: Some(path.into()), kind: "pdf".into(), source_ext: Some(ext), data: Some(raw), ..Default::default() });
    }
    let (text, encoding) = decode(&raw)?;
    let head: String = text.chars().take(20000).collect();
    let newline = if head.contains("\r\n") { "\r\n" } else { "\n" };
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut kind = kind_for(path);
    if kind == "plain" && (ext.is_empty() || ext == ".txt") {
        kind = sniff_kind(&text);
    }
    Ok(OpenFile {
        text,
        path: Some(path.into()),
        kind: kind.into(),
        encoding: encoding.into(),
        newline: newline.into(),
        ..Default::default()
    })
}

/// Text and the name of its encoding: a BOM (utf-8-sig, utf-16), else utf-8, cp1252 or latin-1, whichever
/// decodes first. Bytes with a NUL near the start are refused as binary.
pub fn decode(raw: &[u8]) -> Result<(String, &'static str), String> {
    if let Some(rest) = raw.strip_prefix(b"\xef\xbb\xbf") {
        return Ok((String::from_utf8_lossy(rest).into_owned(), "utf-8-sig"));
    }
    if raw.starts_with(b"\xff\xfe") || raw.starts_with(b"\xfe\xff") {
        let le = raw[0] == 0xFF;
        let body = &raw[2..];
        if body.len() % 2 == 1 {
            return Err("'utf-16' codec can't decode the last byte: truncated data".into());
        }
        let units = body.chunks(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
        return char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map(|t| (t, "utf-16"))
            .map_err(|_| "'utf-16' codec can't decode the text: illegal surrogate".into());
    }
    if raw[..raw.len().min(4000)].contains(&0) {
        return Err("This looks like a binary file, not text.".into());
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return Ok((s.to_string(), "utf-8"));
    }
    if let Some(s) = raw.iter().map(|&b| cp1252_char(b)).collect::<Option<String>>() {
        return Ok((s, "cp1252"));
    }
    Ok((raw.iter().map(|&b| b as char).collect(), "latin-1"))
}

/// A Windows-1252 byte as a character; None for the five bytes the code page leaves undefined.
fn cp1252_char(b: u8) -> Option<char> {
    let c = convert::cp1252(b);
    (!(0x80..0xA0).contains(&b) || c as u32 != b as u32).then_some(c)
}

/// `text` in the named encoding (as [`decode`] names them). Characters it cannot hold become "?" when
/// `replace` is set, else they are an error.
pub fn encode(text: &str, encoding: &str, replace: bool) -> Result<Vec<u8>, String> {
    match encoding {
        "utf-8" | "" => Ok(text.as_bytes().to_vec()),
        "utf-8-sig" => Ok([b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat()),
        "utf-16" => Ok([0xFFu8, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect()),
        "cp1252" | "latin-1" => {
            let mut out = Vec::with_capacity(text.len());
            for c in text.chars() {
                let b = if encoding == "latin-1" {
                    u8::try_from(c as u32).ok()
                } else if (c as u32) < 0x80 || (0xA0..0x100).contains(&(c as u32)) {
                    Some(c as u8)
                } else {
                    (0x80..0xA0u8).find(|&b| cp1252_char(b) == Some(c))
                };
                match b {
                    Some(b) => out.push(b),
                    None if replace => out.push(b'?'),
                    None => return Err(format!("The text has a character ({c}) that the file's encoding ({encoding}) cannot hold. Save it as UTF-8.")),
                }
            }
            Ok(out)
        }
        other => Err(format!("Unknown encoding {other}.")),
    }
}

// -- saving --------------------------------------------------------------------------

/// Writes `data` to `<path>.rewrite-tmp`, then moves it over `path`, so a failed write leaves the old file.
pub fn write_atomic(path: impl AsRef<Path>, data: &[u8]) -> Result<(), String> {
    let path = path.as_ref();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".rewrite-tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Saves the text of a document: in its own encoding and line endings, or (for a .docx/.odt/.rtf ... name)
/// as a formatted document read from the text. Without `path` it goes to the document's own path.
// Python wrote text files in place; here they are written atomically like every other save.
pub fn save(doc: &mut OpenFile, text: &str, path: Option<&Path>) -> Result<(), String> {
    let path = path.map(Path::to_path_buf).or_else(|| doc.path.clone()).ok_or("The document has no file name.")?;
    let ext = ext_of(&path);
    if lookup(SAVE_RICH, &ext).is_some() {
        let fmt = match doc.kind.as_str() {
            "markdown" => "md",
            "latex" => "tex",
            _ => "txt",
        };
        let model = convert::read(fmt, text.as_bytes(), None, None)?;
        return save_rich(&model, &path);
    }
    let own = doc.source_ext.is_none();
    let encoding = if own { doc.encoding.as_str() } else { "utf-8" };
    let newline = if own { doc.newline.as_str() } else if cfg!(windows) { "\r\n" } else { "\n" };
    write_atomic(&path, &encode(&text.replace('\n', newline), encoding, false)?)?;
    if !own {
        doc.encoding = "utf-8".into();
        doc.newline = newline.into();
    }
    doc.path = Some(path.clone());
    doc.source_ext = None;
    let kind = kind_for(&path);
    if kind != "plain" {
        doc.kind = kind.into();
    }
    Ok(())
}

/// Writes a formatted document in the format the file name asks for. Old .doc files are made by
/// LibreOffice from a .docx (Rewrite writes no .doc itself).
pub fn save_rich(model: &Document, path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    let ext = ext_of(path);
    let Some(fmt) = lookup(SAVE_RICH, &ext) else {
        let ext = if ext.is_empty() { "no extension" } else { &ext };
        return Err(format!("Formatted documents are saved as .docx, .odt, .fodt, .rtf or .doc, not {ext}."));
    };
    let data = if fmt == "doc" {
        if !office::available() {
            return Err("Saving as .doc needs LibreOffice. Save as .docx instead (Word opens it), or install \
                        LibreOffice from libreoffice.org."
                .into());
        }
        let (docx, _) = convert::write("docx", model, "document", None)?;
        office::convert(&docx, ".docx", "doc", 180, &|| false)?
    } else {
        convert::write(fmt, model, "document", None)?.0
    };
    write_atomic(path, &data)
}
