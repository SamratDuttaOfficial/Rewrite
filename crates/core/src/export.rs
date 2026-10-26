//! Export and conversion: whatever is open (text, a formatted document or a PDF) written as PDF, Word,
//! OpenDocument, RTF, HTML, Markdown, LaTeX or plain text (was `rewrite/exporting.py`). The formats
//! are read and written by [`convert`]; this decides the route and writes the files.
//!
//! Reading PDF pages needs PDFium, which lives in a crate built on this one, so it comes in as a
//! [`PdfReader`].

use crate::latex::{self, BuildResult};
use crate::model::{Document, PageSetup};
use crate::textclean::{self, Helper};
use crate::{convert, files, layout, office, pdfextract};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The export targets: extension and label.
pub const TARGETS: [(&str, &str); 10] = [
    (".pdf", "PDF"), (".docx", "Word"), (".odt", "OpenDocument"), (".rtf", "Rich Text"), (".doc", "Word 97–2003"),
    (".fodt", "Flat OpenDocument"), (".html", "Web page"), (".md", "Markdown"), (".tex", "LaTeX"), (".txt", "Plain text"),
];

/// The error of an export the user cancelled.
pub const CANCELLED: &str = "cancelled";

fn text_kind(target: &str) -> Option<&'static str> {
    match target {
        ".md" => Some("markdown"),
        ".tex" => Some("latex"),
        ".txt" => Some("plain"),
        _ => None,
    }
}

/// A still picture of what is open, taken on the UI thread.
#[derive(Clone, Debug)]
pub struct Source {
    /// "plain", "markdown", "latex", "html", "code", "rich" or "pdf".
    pub kind: String,
    /// The file name shown (also when there is no path yet).
    pub name: String,
    pub path: Option<PathBuf>,
    pub text: String,
    pub model: Option<Document>,
    pub data: Option<Vec<u8>>,
    pub encoding: String,
}

impl Source {
    /// A text document of the given kind.
    pub fn text(kind: &str, name: &str, path: Option<PathBuf>, text: String, encoding: &str) -> Source {
        Source { kind: kind.into(), name: name.into(), path, text, model: None, data: None, encoding: encoding.into() }
    }

    /// The name without its extension, safe as a file name ("document" when nothing is left).
    pub fn stem(&self) -> String {
        let name = if self.name.is_empty() { "document" } else { &self.name };
        let stem = Path::new(name).file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
        static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w.-]+").unwrap());
        let safe = UNSAFE.replace_all(&stem, "-");
        let safe = safe.trim_matches('-');
        if safe.is_empty() { "document".into() } else { safe.into() }
    }

    /// The folder of the document, where its pictures and included files are found.
    pub fn base_dir(&self) -> Option<PathBuf> {
        self.path.as_ref().and_then(|p| std::path::absolute(p).ok()?.parent().map(Path::to_path_buf))
    }
}

/// Reads PDF files (implemented with PDFium elsewhere).
pub trait PdfReader {
    /// The PDF as a formatted document.
    fn to_document(
        &self,
        data: &[u8],
        opts: &pdfextract::Options,
        helper: Option<&dyn Helper>,
        progress: Option<&dyn Fn(usize, usize)>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Document, String>;
    /// The first page as PNG (for LaTeX figures that are PDF files).
    fn first_page_png(&self, data: &[u8]) -> Option<Vec<u8>>;
}

/// What the export dialog chose.
#[derive(Clone, Debug)]
pub struct Options {
    /// How a PDF is made: "rewrite", "latex" or "office" (see [`engines`]); None is "rewrite".
    pub engine: Option<String>,
    /// The page for Rewrite's own layout; None keeps the document's.
    pub page: Option<PageSetup>,
    pub numbers: Option<bool>,
    /// Generic font family for Rewrite's own layout ("serif", "sans" ...).
    pub generic: String,
    /// One sentence per line in Markdown, LaTeX and plain text (needs a helper).
    pub sentences: bool,
    pub fix_words: bool,
    pub drop_headers: bool,
    pub images: bool,
    pub tables: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            engine: None,
            page: None,
            numbers: None,
            generic: "serif".into(),
            sentences: false,
            fix_words: true,
            drop_headers: true,
            images: true,
            tables: true,
        }
    }
}

/// The helpers an export may use.
#[derive(Clone, Copy)]
pub struct Tools<'a> {
    /// Dictionary and context model, for fixing words of PDFs and splitting sentences.
    pub helper: Option<&'a dyn Helper>,
    pub pdf: Option<&'a dyn PdfReader>,
    /// Pages read so far and the page count, while a PDF is read.
    pub progress: Option<&'a dyn Fn(usize, usize)>,
    pub cancel: &'a dyn Fn() -> bool,
}

impl Default for Tools<'_> {
    fn default() -> Self {
        Tools { helper: None, pdf: None, progress: None, cancel: &|| false }
    }
}

/// The ways a PDF can be made from this source: (key, label), best first. Empty for other targets.
pub fn engines(source: &Source, target: &str) -> Vec<(&'static str, &'static str)> {
    if target != ".pdf" || source.kind == "pdf" {
        return vec![];
    }
    let tex = latex::available();
    let mut out = vec![];
    if source.kind == "latex" && tex {
        out.push(("latex", "LaTeX"));
    }
    out.push(("rewrite", "Rewrite's own layout"));
    if source.kind == "markdown" && tex {
        out.push(("latex", "LaTeX (via a .tex file)"));
    }
    if source.kind == "rich" && office::available() {
        out.push(("office", "LibreOffice"));
    }
    out
}

/// The source as a formatted document.
pub fn model_of(source: &Source, opts: &Options, tools: &Tools) -> Result<Document, String> {
    match source.kind.as_str() {
        "rich" => source.model.clone().ok_or_else(|| "There is no document to export.".into()),
        "pdf" => {
            let reader = tools.pdf.ok_or("PDF files cannot be read here.")?;
            let data = source.data.as_deref().ok_or("There is no PDF to export.")?;
            let options = pdfextract::Options {
                drop_headers: opts.drop_headers,
                fix_words: opts.fix_words,
                images: opts.images,
                tables: opts.tables,
                ..Default::default()
            };
            let helper = if opts.fix_words { tools.helper } else { None };
            reader.to_document(data, &options, helper, tools.progress, tools.cancel)
        }
        "markdown" => convert::read("md", source.text.as_bytes(), source.base_dir().as_deref(), None),
        "latex" => {
            let figure = |pdf: &[u8]| tools.pdf.and_then(|r| r.first_page_png(pdf));
            convert::read("tex", source.text.as_bytes(), source.base_dir().as_deref(), Some(&figure))
        }
        _ => convert::read("txt", source.text.as_bytes(), None, None),
    }
}

/// Writes the source to `path` in the `target` format (an extension from [`TARGETS`]). Returns the files
/// written: the document first, then any pictures beside it. A cancelled export is `Err(CANCELLED)`.
pub fn export(source: &Source, target: &str, path: &Path, opts: &Options, tools: &Tools) -> Result<Vec<PathBuf>, String> {
    let stem = path.file_stem().map_or("document".into(), |s| s.to_string_lossy().into_owned());
    let out_dir = std::path::absolute(path).map_err(|e| e.to_string())?.parent().map(Path::to_path_buf).unwrap_or_default();
    let file_name = path.file_name().map(PathBuf::from).ok_or("The export needs a file name.")?;
    let split = |text: &str| textclean::sentence_starts(text, tools.helper.unwrap());
    let sentences: convert::Sentences = (opts.sentences && tools.helper.is_some()).then_some(&split as &dyn Fn(&str) -> Vec<usize>);
    let mut written = vec![];
    let mut put = |rel: &Path, data: &[u8]| -> Result<(), String> {
        let full = out_dir.join(rel);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        files::write_atomic(&full, data)?;
        written.push(full);
        Ok(())
    };

    // text written as the same kind of text: the file as it is (unless sentences go on lines of their own)
    if text_kind(target) == Some(source.kind.as_str()) && sentences.is_none() {
        let encoding = if source.encoding.is_empty() { "utf-8" } else { &source.encoding };
        put(&file_name, &files::encode(&source.text, encoding, true)?)?;
        return Ok(written);
    }
    if target == ".pdf" {
        if source.kind == "pdf" {
            put(&file_name, source.data.as_deref().unwrap_or_default())?;
            return Ok(written);
        }
        let engine = opts.engine.as_deref().unwrap_or("rewrite");
        if engine == "latex" && source.kind == "latex" {
            let encoding = if source.encoding.is_empty() { "utf-8" } else { &source.encoding };
            let res = latex::build(&source.text, source.path.as_deref(), encoding, tools.cancel).map_err(|_| CANCELLED)?;
            put(&file_name, &res.pdf.clone().ok_or_else(|| latex_failure(&res))?)?;
            return Ok(written);
        }
        let mut model = model_of(source, opts, tools)?;
        match engine {
            "latex" => {
                let (tex, side) = convert::write("tex", &model, &stem, None)?;
                let work = std::env::temp_dir().join(format!("rewrite-export-{}-{}", std::process::id(), unique()));
                let tex_path = work.join(format!("{stem}.tex"));
                let built = (|| {
                    for (rel, data) in &side {
                        let full = work.join(rel);
                        std::fs::create_dir_all(full.parent().unwrap_or(&work)).map_err(|e| e.to_string())?;
                        std::fs::write(&full, data).map_err(|e| e.to_string())?;
                    }
                    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                    let tex = String::from_utf8_lossy(&tex);
                    latex::build(&tex, Some(&tex_path), "utf-8", tools.cancel).map_err(|_| CANCELLED.to_string())
                })();
                // The build folder belongs to this one-off path, so nothing would ever reuse it.
                let _ = std::fs::remove_dir_all(latex::build_dir(Some(&tex_path)));
                let _ = std::fs::remove_dir_all(&work);
                let res = built?;
                put(&file_name, &res.pdf.clone().ok_or_else(|| latex_failure(&res))?)?;
            }
            "office" => {
                let (docx, _) = convert::write("docx", &model, "document", None)?;
                put(&file_name, &office::convert(&docx, ".docx", "pdf", 300, tools.cancel)?)?;
            }
            _ => {
                if let Some(page) = &opts.page {
                    model.page = page.clone();
                }
                let lo = layout::Options { generic: opts.generic.clone(), numbers: opts.numbers, page: opts.page.clone() };
                put(&file_name, &layout::render(&model, &lo)?)?;
            }
        }
        return Ok(written);
    }
    let model = model_of(source, opts, tools)?;
    if matches!(target, ".doc" | ".docx" | ".odt" | ".fodt" | ".rtf") {
        files::save_rich(&model, path)?;
        written.push(path.to_path_buf());
        return Ok(written);
    }
    let fmt = match target {
        ".html" => "html",
        ".md" => "md",
        ".tex" => "tex",
        ".txt" => "txt",
        other => return Err(format!("Rewrite cannot export {other} files.")),
    };
    let (data, side) = convert::write(fmt, &model, &stem, sentences)?;
    put(&file_name, &data)?;
    for (rel, blob) in &side {
        put(Path::new(rel), blob)?;
    }
    Ok(written)
}

fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// Why LaTeX made no PDF: the message and the first six errors.
pub fn latex_failure(res: &BuildResult) -> String {
    let mut lines = vec![if res.message.is_empty() { "LaTeX did not make a PDF.".to_string() } else { res.message.clone() }];
    for (line, msg) in res.errors.iter().take(6) {
        lines.push(match line {
            Some(l) if *l != 0 => format!("Line {l}: {msg}"),
            _ => msg.clone(),
        });
    }
    lines.join("\n")
}

/// The page a PDF export starts from: the formatted document's own, else A4.
pub fn default_page(source: &Source) -> PageSetup {
    match (&source.kind[..], &source.model) {
        ("rich", Some(m)) => m.page.clone(),
        _ => PageSetup::default(),
    }
}
