//! export.rs: the routes from each kind of source to each target (exporting.py has no fixtures).

use rewrite_core::export::{self, Options, PdfReader, Source, Tools};
use rewrite_core::model::{self, Block, Document, Paragraph, Run};
use rewrite_core::{convert, files, latex, pdfextract, textclean};
use rewrite_testkit::TempDir;
use std::cell::Cell;
use std::path::Path;

const MD: &str = "# Results\n\nThe method works. It is fast.\n\n- one\n- two\n";

fn md_source() -> Source {
    Source::text("markdown", "notes.md", None, MD.into(), "utf-8")
}

fn text_of(path: &Path, fmt: &str) -> String {
    model::document_text(&convert::read(fmt, &std::fs::read(path).unwrap(), None, None).unwrap())
}

#[test]
fn markdown_to_formatted_and_text() {
    let dir = TempDir::new("export-md");
    let tools = Tools::default();
    for (ext, fmt) in [(".docx", "docx"), (".odt", "odt"), (".rtf", "rtf"), (".fodt", "fodt")] {
        let path = dir.join(format!("out{ext}"));
        let written = export::export(&md_source(), ext, &path, &Options::default(), &tools).unwrap();
        assert_eq!(written, [path.clone()]);
        let text = text_of(&path, fmt);
        assert!(text.contains("Results") && text.contains("It is fast."), "{ext}: {text}");
    }
    let html = dir.join("out.html");
    export::export(&md_source(), ".html", &html, &Options::default(), &tools).unwrap();
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("<h1") && page.contains("<li"), "{page}");
    let txt = dir.join("out.txt");
    export::export(&md_source(), ".txt", &txt, &Options::default(), &tools).unwrap();
    assert!(std::fs::read_to_string(&txt).unwrap().starts_with("Results\n-------"));
    assert!(!dir.join("out.txt.rewrite-tmp").exists());
}

#[test]
fn same_kind_text_is_copied_in_its_encoding() {
    let dir = TempDir::new("export-copy");
    let text = "# Café “menu”\n\nCrème brûlée €5\n";
    let src = Source::text("markdown", "menu.md", None, text.into(), "cp1252");
    let path = dir.join("copy.md");
    export::export(&src, ".md", &path, &Options::default(), &Tools::default()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), files::encode(text, "cp1252", false).unwrap());
    // characters the encoding cannot hold become "?"
    let src = Source::text("plain", "a.txt", None, "a → b".into(), "latin-1");
    export::export(&src, ".txt", &dir.join("a.txt"), &Options::default(), &Tools::default()).unwrap();
    assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"a ? b");
}

#[test]
fn sentences_on_lines_of_their_own() {
    let dir = TempDir::new("export-sentences");
    let path = dir.join("split.md");
    let opts = Options { sentences: true, ..Default::default() };
    // without a helper the text is copied as it is, as in Python
    export::export(&md_source(), ".md", &path, &opts, &Tools::default()).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), MD);
    let tools = Tools { helper: Some(&textclean::NoHelper), ..Default::default() };
    export::export(&md_source(), ".md", &path, &opts, &tools).unwrap();
    let out = std::fs::read_to_string(&path).unwrap();
    assert!(out.contains("The method works.\nIt is fast."), "{out}");
}

#[test]
fn rewrite_engine_pdf() {
    let dir = TempDir::new("export-pdf");
    let path = dir.join("out.pdf");
    let letter = model::PageSetup { width: 612.0, height: 792.0, ..Default::default() };
    let opts = Options { page: Some(letter), numbers: Some(false), ..Default::default() };
    export::export(&md_source(), ".pdf", &path, &opts, &Tools::default()).unwrap();
    let pdf = std::fs::read(&path).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    assert!(String::from_utf8_lossy(&pdf).contains("612"));
}

#[test]
fn latex_engine_pdf() {
    if latex::pick_engine("").is_none() {
        eprintln!("no LaTeX installed: skipped");
        return;
    }
    let dir = TempDir::new("export-latexpdf");
    let opts = Options { engine: Some("latex".into()), ..Default::default() };
    let path = dir.join("via-tex.pdf");
    assert_eq!(export::export(&md_source(), ".pdf", &path, &opts, &Tools::default()).unwrap(), [path.clone()]);
    assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF"));

    let tex = Source::text("latex", "paper.tex", Some(dir.join("paper.tex")), r"\documentclass{article}\begin{document}Hi\end{document}".into(), "utf-8");
    let _build = TempDir::adopt(latex::build_dir(Some(&dir.join("paper.tex"))));
    let path = dir.join("paper.pdf");
    export::export(&tex, ".pdf", &path, &opts, &Tools::default()).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF"));

    let broken = Source::text("latex", "bad.tex", None, r"\documentclass{article}\begin{document}\foo\end{document}".into(), "utf-8");
    let err = export::export(&broken, ".pdf", &dir.join("bad.pdf"), &opts, &Tools::default()).unwrap_err();
    assert!(err.starts_with("The document did not compile.\nLine "), "{err}");
    assert!(err.contains("Undefined control sequence \\foo"), "{err}");

    let cancelled = Tools { cancel: &|| true, ..Default::default() };
    assert_eq!(export::export(&tex, ".pdf", &path, &opts, &cancelled).unwrap_err(), export::CANCELLED);
}

/// Answers with a fixed document and remembers how it was asked.
struct FakeReader {
    calls: Cell<usize>,
    fix_words: Cell<Option<bool>>,
    had_helper: Cell<bool>,
    figures: Cell<usize>,
}

impl FakeReader {
    fn new() -> FakeReader {
        FakeReader { calls: Cell::new(0), fix_words: Cell::new(None), had_helper: Cell::new(false), figures: Cell::new(0) }
    }
}

impl PdfReader for FakeReader {
    fn to_document(
        &self,
        data: &[u8],
        opts: &pdfextract::Options,
        helper: Option<&dyn textclean::Helper>,
        progress: Option<&dyn Fn(usize, usize)>,
        _cancel: &dyn Fn() -> bool,
    ) -> Result<Document, String> {
        assert_eq!(data, b"%PDF-fake");
        self.calls.set(self.calls.get() + 1);
        self.fix_words.set(Some(opts.fix_words));
        self.had_helper.set(helper.is_some());
        if let Some(p) = progress {
            p(0, 1);
        }
        let para = Paragraph { content: vec![model::Inline::Run(Run::new("From the PDF."))], ..Default::default() };
        Ok(Document { blocks: vec![Block::Paragraph(para)], ..Default::default() })
    }

    fn first_page_png(&self, _data: &[u8]) -> Option<Vec<u8>> {
        self.figures.set(self.figures.get() + 1);
        let mut png = std::io::Cursor::new(vec![]);
        image::RgbImage::new(4, 3).write_to(&mut png, image::ImageFormat::Png).ok()?;
        Some(png.into_inner())
    }
}

#[test]
fn pdf_sources_go_through_the_reader() {
    let dir = TempDir::new("export-reader");
    let reader = FakeReader::new();
    let pages = Cell::new(0);
    let progress = |_: usize, n: usize| pages.set(n);
    let helper = textclean::NoHelper;
    let tools = Tools { pdf: Some(&reader), helper: Some(&helper), progress: Some(&progress), ..Default::default() };
    let src = Source { kind: "pdf".into(), name: "scan.pdf".into(), path: None, text: String::new(), model: None, data: Some(b"%PDF-fake".to_vec()), encoding: "utf-8".into() };

    let path = dir.join("scan.docx");
    export::export(&src, ".docx", &path, &Options::default(), &tools).unwrap();
    assert_eq!(text_of(&path, "docx").trim(), "From the PDF.");
    assert_eq!((reader.calls.get(), reader.fix_words.get(), reader.had_helper.get(), pages.get()), (1, Some(true), true, 1));

    let opts = Options { fix_words: false, ..Default::default() };
    export::export(&src, ".txt", &dir.join("scan.txt"), &opts, &tools).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("scan.txt")).unwrap(), "From the PDF.\n");
    assert_eq!((reader.fix_words.get(), reader.had_helper.get()), (Some(false), false));

    // PDF to PDF is a copy; no engines are offered
    export::export(&src, ".pdf", &dir.join("copy.pdf"), &Options::default(), &tools).unwrap();
    assert_eq!(std::fs::read(dir.join("copy.pdf")).unwrap(), b"%PDF-fake");
    assert!(export::engines(&src, ".pdf").is_empty());
    assert_eq!(reader.calls.get(), 2);

    // without a reader a PDF cannot be converted
    assert!(export::export(&src, ".txt", &dir.join("x.txt"), &Options::default(), &Tools::default()).is_err());
}

#[test]
fn latex_figures_that_are_pdfs_use_the_reader() {
    let dir = TempDir::new("export-figure");
    std::fs::write(dir.join("plot.pdf"), b"%PDF-fake").unwrap();
    let reader = FakeReader::new();
    let tools = Tools { pdf: Some(&reader), ..Default::default() };
    let text = r"\documentclass{article}\begin{document}Text.\includegraphics{plot.pdf}\end{document}";
    let src = Source::text("latex", "paper.tex", Some(dir.join("paper.tex")), text.into(), "utf-8");
    let out = dir.join("paper.docx");
    export::export(&src, ".docx", &out, &Options::default(), &tools).unwrap();
    assert_eq!(reader.figures.get(), 1);
}

#[test]
fn engines_pages_and_names() {
    let md = md_source();
    let keys: Vec<_> = export::engines(&md, ".pdf").into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys[0], "rewrite");
    assert_eq!(keys.contains(&"latex"), latex::available());
    assert!(export::engines(&md, ".docx").is_empty());
    let page = model::PageSetup { width: 612.0, height: 792.0, ..Default::default() };
    let rich = Source {
        kind: "rich".into(),
        name: "My report (final).docx".into(),
        path: None,
        text: String::new(),
        model: Some(Document { page: page.clone(), ..Default::default() }),
        data: None,
        encoding: "utf-8".into(),
    };
    assert_eq!(export::default_page(&rich), page);
    assert_eq!(export::default_page(&md), model::PageSetup::default());
    assert_eq!(rich.stem(), "My-report-final");
    assert_eq!(Source::text("plain", "", None, String::new(), "utf-8").stem(), "document");
    assert_eq!(Source::text("plain", "a--b c.txt", None, String::new(), "utf-8").stem(), "a--b-c");
    let res = latex::BuildResult { errors: vec![(Some(3), "Oops.".into()), (None, "x.sty:1: Bad.".into())], ..Default::default() };
    assert_eq!(export::latex_failure(&res), "LaTeX did not make a PDF.\nLine 3: Oops.\nx.sty:1: Bad.");
}
