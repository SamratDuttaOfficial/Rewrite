//! rewrite-pdfium against rewrite/pdfdoc.py and the PDF editor's PDFium calls (fixture group "pdfdoc", PDFs in
//! tests/corpus/pdf). Floats were rounded to 3 decimals; pictures are compared by size and SHA-1.

use rewrite_core::pdfextract::{Options, PPage};
use rewrite_core::textclean::Helper;
use rewrite_pdfium::{first_page_png, to_document, Document};
use rewrite_testkit::{json, root, Case, Fixture};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashSet;

const TOLERANCE: f64 = 1e-3;

fn fixture() -> Fixture {
    Fixture::load("pdfdoc")
}

fn pdf(name: &str) -> Vec<u8> {
    std::fs::read(root().join("tests").join("corpus").join("pdf").join(name)).unwrap_or_else(|_| name.as_bytes().to_vec())
}

fn open(case: &Case) -> Document {
    Document::open(pdf(case.str(0))).expect("the corpus PDF opens")
}

/// Bytes as the fixture records them: SHA-1, length and, for PNG, the pixel size.
fn hashed(data: &[u8]) -> Value {
    let mut out = json!({"sha1": Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect::<String>(), "len": data.len()});
    if data.starts_with(b"\x89PNG\r\n\x1a\n") && data.len() >= 24 {
        let be = |i: usize| u32::from_be_bytes(data[i..i + 4].try_into().unwrap());
        out["png"] = json!([be(16), be(20)]);
    }
    out
}

fn png(data: Option<Vec<u8>>) -> Value {
    data.map_or(Value::Null, |d| hashed(&d))
}

fn pages_json(pages: Vec<PPage>) -> Value {
    Value::Array(
        pages
            .into_iter()
            .map(|p| {
                let chars: Vec<Value> = p.chars.into_iter().map(|c| json!([c.0, c.1, c.2, c.3, c.4, c.5, c.6, c.7, c.8, c.9, c.10])).collect();
                let images: Vec<Value> = p.images.into_iter().map(|i| json!({"x0": i.x0, "top": i.top, "x1": i.x1, "bottom": i.bottom, "data": hashed(&i.data)})).collect();
                let shapes: Vec<Value> = p.shapes.into_iter().map(|s| json!([s.0, s.1, s.2, s.3, s.4])).collect();
                json!({"width": p.width, "height": p.height, "chars": chars, "images": images, "shapes": shapes})
            })
            .collect(),
    )
}

/// A serialised Document with its byte strings (pictures, kept sources) hashed like the fixture's.
fn hash_bytes(v: &mut Value) {
    let as_bytes = |v: &Value| -> Option<Vec<u8>> { v.as_array()?.iter().map(|n| n.as_u64().and_then(|n| u8::try_from(n).ok())).collect() };
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k == "data" {
                    if let Some(b) = as_bytes(x) {
                        *x = hashed(&b);
                        continue;
                    }
                }
                if k == "source" {
                    if let Value::Object(files) = x {
                        for f in files.values_mut() {
                            if let Some(b) = as_bytes(f) {
                                *f = hashed(&b);
                            }
                        }
                        continue;
                    }
                }
                hash_bytes(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(hash_bytes),
        _ => {}
    }
}

/// Python's to_native of a docmodel Document gives the paragraphs in table cells a "type" too.
fn type_cells(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("table") {
                for p in o.get_mut("rows").and_then(Value::as_array_mut).into_iter().flatten().filter_map(Value::as_array_mut).flatten().filter_map(Value::as_array_mut).flatten() {
                    if let Value::Object(p) = p {
                        p.entry("type").or_insert_with(|| json!("paragraph"));
                    }
                }
            }
            o.values_mut().for_each(type_cells);
        }
        Value::Array(a) => a.iter_mut().for_each(type_cells),
        _ => {}
    }
}

/// The recorder's stand-in for the language engine: a fixed dictionary.
struct Words(HashSet<String>);

impl Helper for Words {
    fn known(&self, word: &str) -> bool {
        self.0.contains(&word.to_lowercase())
    }
    fn zipf(&self, word: &str) -> f64 {
        if self.known(word) { 4.0 } else { 0.0 }
    }
}

#[test]
fn page_sizes() {
    fixture().check_within("page_sizes", TOLERANCE, |c| json(open(c).page_sizes().unwrap()));
}

#[test]
fn read_pages() {
    fixture().check_within("read_pages", TOLERANCE, |c| pages_json(open(c).read_pages(c.arg(1), None, None).unwrap()));
}

#[test]
fn renderer() {
    fixture().check_within("renderer", TOLERANCE, |c| png(open(c).render_area(c.arg(1), c.arg(2), c.arg(3), c.arg(4), c.arg(5))));
}

#[test]
fn chars() {
    fixture().check_within("chars", TOLERANCE, |c| {
        let chars = open(c).chars(c.arg(1), c.arg(2), c.arg(3)).unwrap();
        Value::Array(chars.into_iter().map(|c| json!([c.text, c.x0, c.top, c.x1, c.bottom, c.size, c.bold, c.italic, c.font])).collect())
    });
}

#[test]
fn text_bounded() {
    fixture().check("text_bounded", |c| json(open(c).text_bounded(c.arg(1), c.arg(2), c.arg(3), c.arg(4), c.arg(5)).unwrap()));
}

#[test]
fn first_page() {
    fixture().check("first_page_png", |c| png(first_page_png(&pdf(c.str(0)), c.arg(1))));
}

#[test]
fn convert() {
    fixture().check_within("to_document", TOLERANCE, |c| {
        let opts: Options = c.arg(1);
        let words: Option<Vec<String>> = c.arg(2);
        let helper = Words(words.unwrap_or_default().into_iter().collect());
        let doc = to_document(&pdf(c.str(0)), &opts, &helper, None, None).unwrap();
        let mut v = json(doc);
        hash_bytes(&mut v);
        type_cells(&mut v);
        v
    });
}

#[test]
fn convert_progress_and_cancel() {
    let data = pdf("document.pdf");
    let seen = std::cell::RefCell::new(vec![]);
    let progress = |k: usize, n: usize| seen.borrow_mut().push((k, n));
    to_document(&data, &Options::default(), &rewrite_core::textclean::NoHelper, Some(&progress), None).unwrap();
    assert_eq!(*seen.borrow(), [(0, 2), (1, 2)]);
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let got = to_document(&data, &Options::default(), &rewrite_core::textclean::NoHelper, None, Some(&cancel));
    assert_eq!(got.err(), Some(rewrite_pdfium::Error::Cancelled));
    assert!(Document::open(b"not a pdf".to_vec()).is_err());
}
