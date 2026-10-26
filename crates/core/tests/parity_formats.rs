//! files.rs and protect.rs against rewrite/formats.py (fixture group "formats").

use rewrite_core::{files, protect};
use rewrite_testkit::{byte_to_char, json, Fixture, TempDir};
use serde_json::Value;
use std::path::Path;

fn fixture() -> Fixture {
    Fixture::load("formats")
}

#[test]
fn kind_for() {
    fixture().check("kind_for", |c| json(files::kind_for(c.str(0))));
}

#[test]
fn sniff_kind() {
    fixture().check("sniff_kind", |c| json(files::sniff_kind(c.str(0))));
}

#[test]
fn decode() {
    fixture().check("decode", |c| match files::decode(&c.arg::<Vec<u8>>(0)) {
        Ok((text, enc)) => json((text, enc)),
        Err(e) => serde_json::json!({ "error": format!("ValueError: {e}") }),
    });
}

#[test]
fn protected_spans() {
    fixture().check("protected_spans", |c| {
        let text = c.str(0);
        let path: Option<String> = c.arg(2);
        let spans = protect::protected_spans(text, c.str(1), path.as_deref().map(Path::new));
        json(spans.iter().map(|&(s, e)| (byte_to_char(text, s), byte_to_char(text, e))).collect::<Vec<_>>())
    });
}

#[test]
fn merge() {
    fixture().check("merge", |c| json(protect::merge(c.arg(0))));
}

#[test]
fn overlaps() {
    fixture().check("overlaps", |c| json(protect::overlaps(&c.arg::<Vec<(usize, usize)>>(0), c.arg(1), c.arg(2))));
}

#[test]
fn clip() {
    fixture().check("clip", |c| json(protect::clip(&c.arg::<Vec<(usize, usize)>>(0), c.arg(1), c.arg(2))));
}

#[test]
fn encodings_round_trip() {
    for (text, enc) in [("café “quotes”", "cp1252"), ("\u{8d}été", "latin-1"), ("naïve", "utf-8-sig"), ("ünï", "utf-16"), ("x", "utf-8")] {
        let raw = files::encode(text, enc, false).unwrap();
        assert_eq!(files::decode(&raw).unwrap(), (text.to_string(), enc), "{enc}");
    }
    assert!(files::encode("€", "latin-1", false).is_err());
    assert_eq!(files::encode("a€", "latin-1", true).unwrap(), b"a?");
}

#[test]
fn load_and_save_keep_encoding_and_newlines() {
    let dir = TempDir::new("files");
    let path = dir.join("notes.txt");
    std::fs::write(&path, files::encode("# Title\r\n\r\ncafé “x”\r\n", "cp1252", false).unwrap()).unwrap();
    let mut doc = files::load(&path).unwrap();
    assert_eq!((doc.kind.as_str(), doc.encoding.as_str(), doc.newline.as_str()), ("markdown", "cp1252", "\r\n"));
    assert_eq!(doc.text, "# Title\n\ncafé “x”\n");
    files::save(&mut doc, "# Title\n\nchanged “y”\n", None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), files::encode("# Title\r\n\r\nchanged “y”\r\n", "cp1252", false).unwrap());
    assert!(!dir.join("notes.txt.rewrite-tmp").exists());

    // a text saved under a Word name is read as Markdown and written as .docx, which loads as formatted
    let docx = dir.join("notes.docx");
    files::save(&mut doc, "# Title\n\nSome **bold** text.\n", Some(&docx)).unwrap();
    let rich = files::load(&docx).unwrap();
    assert_eq!(rich.kind, "rich");
    assert_eq!(rich.source_ext.as_deref(), Some(".docx"));
    let text = rewrite_core::model::document_text(rich.model.as_ref().unwrap());
    assert!(text.contains("Some bold text."), "{text}");

    std::fs::write(dir.join("bad.pdf"), b"hello").unwrap();
    assert_eq!(files::load(dir.join("bad.pdf")).unwrap_err(), "This is not a PDF file.");
    std::fs::write(dir.join("ok.pdf"), b"junk%PDF-1.4").unwrap();
    assert_eq!(files::load(dir.join("ok.pdf")).unwrap().kind, "pdf");
    assert!(files::save_rich(rich.model.as_ref().unwrap(), dir.join("x.pdf")).unwrap_err().contains("not .pdf"));
}

#[test]
fn tables() {
    assert_eq!(files::lookup(files::READ, ".md"), Some("md"));
    assert_eq!(files::lookup(files::KIND_LABEL, "code"), Some("Code (comments only)"));
    assert_eq!(files::OPEN_TYPES.last().unwrap().patterns, ["*.*"]);
    let _: Value = json(files::SAVE_RICH_TYPES.iter().map(|t| t.label).collect::<Vec<_>>());
}
