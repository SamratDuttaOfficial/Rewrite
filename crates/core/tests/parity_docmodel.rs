//! model.rs against rewrite/docmodel.py (fixture group "docmodel").

use rewrite_core::model::{self, Block, Document, Inline};
use rewrite_testkit::{json, Fixture};
use serde_json::{Map, Value};

fn fixture() -> Fixture {
    Fixture::load("docmodel")
}

#[test]
fn number_label() {
    fixture().check("number_label", |c| json(model::number_label(c.arg(0), c.arg(1))));
}

#[test]
fn roman() {
    fixture().check("roman", |c| json(model::roman(c.arg(0))));
}

#[test]
fn bullets() {
    fixture().check("bullets", |_| json(model::BULLETS));
}

#[test]
fn page_sizes() {
    fixture().check("page_sizes", |_| {
        let sizes: Map<String, Value> = model::PAGE_SIZES.iter().map(|(n, w, h)| (n.to_string(), json([w, h]))).collect();
        Value::Object(sizes)
    });
}

#[test]
fn look() {
    let f = fixture();
    let want = f.cases("look")[0].out.as_object().expect("LOOK is a dict").clone();
    f.check("look", |_| {
        let looks: Map<String, Value> = want
            .iter()
            .map(|(style, keys)| {
                let l = model::look(style);
                let all = json(serde_json::json!({"scale": l.scale, "bold": l.bold, "italic": l.italic,
                    "before": l.before, "after": l.after, "indent": l.indent, "color": l.color}));
                // Python leaves out indent and color when they are 0 and None.
                let same_keys = keys.as_object().unwrap().keys().map(|k| (k.clone(), all[k].clone())).collect();
                (style.clone(), Value::Object(same_keys))
            })
            .collect();
        Value::Object(looks)
    });
}

#[test]
fn inline_text() {
    fixture().check("inline_text", |c| json(model::inline_text(&c.arg::<Vec<Inline>>(0), c.arg(1))));
}

#[test]
fn merge_runs() {
    fixture().check("merge_runs", |c| json(model::merge_runs(c.arg(0))));
}

#[test]
fn strip_content() {
    fixture().check("strip_content", |c| json(model::strip_content(c.arg(0))));
}

#[test]
fn caption_text() {
    fixture().check("caption_text", |c| json(model::caption_text(&c.arg::<Vec<Inline>>(0))));
}

#[test]
fn plain_paragraphs() {
    fixture().check("plain_paragraphs", |c| json(model::plain_paragraphs(c.str(0))));
}

#[test]
fn list_labels() {
    fixture().check("list_labels", |c| json(model::list_labels(&c.arg::<Vec<Block>>(0))));
}

#[test]
fn document_text() {
    fixture().check("document_text", |c| json(model::document_text(&c.arg::<Document>(0))));
}

#[test]
#[ignore = "phase 1: PageSetup::name lacks the landscape and inch forms"]
fn page_name() {
    fixture().check("page_name", |c| {
        json(model::PageSetup { width: c.arg(0), height: c.arg(1), ..Default::default() }.name())
    });
}

#[test]
#[ignore = "phase 1: Image::fit, STYLES and STYLE_LABEL are not ported yet"]
fn not_ported_yet() {
    let f = fixture();
    for function in ["image_fit", "styles", "style_label"] {
        f.check(function, |_| Value::Null);
    }
}
