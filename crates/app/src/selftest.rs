//! `rewrite --selftest [log]`: exercise every library the installed program needs (GTK, ONNX Runtime and
//! the models when they are downloaded, PDFium, the document formats, fonts). Exit code 0 means the bundle
//! is complete; the log says what was checked or what failed.

use rewrite_core::model::{Block, Document, Inline, Paragraph, Run};
use rewrite_core::{convert, fonts, layout, pdfedit};
use std::path::PathBuf;

pub fn run(log: Option<PathBuf>) -> i32 {
    let res = std::panic::catch_unwind(check).unwrap_or_else(|p| Err(crate::workers::panic_text(p)));
    let (msg, code) = match res {
        Ok(msg) => (msg, 0),
        Err(e) => (format!("selftest failed: {e}"), 1),
    };
    match log {
        Some(p) => {
            let _ = std::fs::write(p, &msg);
        }
        None => println!("{msg}"),
    }
    code
}

fn check() -> Result<String, String> {
    gtk4::init().map_err(|e| format!("GTK does not start: {e}"))?;
    let eng = crate::engine::Engine::new(&[]);
    eng.load_models();
    let t = "The results show a significant improvement in accuracy.";
    let s = t.find("significant").unwrap();
    if eng.suggest(t, s, s + "significant".len(), 5, false).is_empty() {
        return Err("no word suggestions".into());
    }
    if eng.has_paraphraser() && eng.paraphrase(t, &[], 2, 0).is_empty() {
        return Err("no paraphrases".into());
    }
    // the document engine: every format there and back
    let doc = Document {
        blocks: vec![Block::Paragraph(Paragraph::styled(vec![Inline::Run(Run::new("Engine check"))], "h1"))],
        ..Default::default()
    };
    for fmt in ["docx", "odt", "rtf", "md", "tex"] {
        let (data, _) = convert::write(fmt, &doc, "document", None)?;
        let back = convert::read(fmt, &data, None, None)?;
        let ok = back.blocks.first().is_some_and(|b| matches!(b, Block::Paragraph(p) if p.text().contains("Engine check")));
        if !ok {
            return Err(format!("{fmt} round trip"));
        }
    }
    if fonts::families().is_empty() {
        return Err("no fonts found for PDF output".into());
    }
    let pdf = layout::render(&doc, &layout::Options { generic: "serif".into(), numbers: None, page: None })?;
    let turned = pdfedit::apply(&pdf, &[pdfedit::Op::Rotate { page: 0, angle: 90 }])?;
    if pdfedit::pages(&turned)?.len() != 1 {
        return Err("PDF editing".into());
    }
    // PDFium: the PDF view and conversion
    let d = rewrite_pdfium::Document::open(pdf).map_err(|e| format!("PDFium: {e:?}"))?;
    let bmp = d.render_page(0, 0.5).map_err(|e| format!("PDFium does not render: {e:?}"))?;
    if bmp.width == 0 {
        return Err("PDFium rendered nothing".into());
    }
    Ok(format!("selftest ok (smart={}, paraphraser={})", eng.smart(), eng.has_paraphraser()))
}
