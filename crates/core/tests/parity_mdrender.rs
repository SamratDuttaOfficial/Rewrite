//! mdpreview.rs against rewrite/mdrender.py (fixture group "mdrender").

use rewrite_core::mdpreview::{self, Block, Run, Target};
use rewrite_testkit::{json, Fixture};
use serde_json::{json as j, Value};

/// A run as Python records it: [text, sorted styles, link or [alt, src] or null].
fn run(r: &Run) -> Value {
    let target = match &r.target {
        None => Value::Null,
        Some(Target::Link(url)) => json(url),
        Some(Target::Image { alt, src }) => j!([alt, src]),
    };
    j!([r.text, r.styles.iter().collect::<Vec<_>>(), target])
}

fn runs(rs: &[Run]) -> Value {
    Value::Array(rs.iter().map(run).collect())
}

fn block(b: &Block) -> Value {
    match b {
        Block::Heading { level, runs: r } => j!(["heading", level, runs(r)]),
        Block::Para(r) => j!(["para", runs(r)]),
        Block::Code { lang, source } => j!(["code", lang, source]),
        Block::Rule => j!(["rule"]),
        Block::Item { depth, marker, checked, runs: r } => j!(["item", depth, marker, checked, runs(r)]),
        Block::Quote { depth, runs: r } => j!(["quote", depth, runs(r)]),
        Block::Table { head, align, rows } => j!([
            "table",
            head.iter().map(|c| runs(c)).collect::<Vec<_>>(),
            align,
            rows.iter().map(|row| row.iter().map(|c| runs(c)).collect::<Vec<_>>()).collect::<Vec<_>>()
        ]),
        Block::Math(tex) => j!(["math", tex]),
    }
}

#[test]
fn inline() {
    Fixture::load("mdrender").check("inline", |c| runs(&mdpreview::inline(c.str(0))));
}

#[test]
fn parse() {
    Fixture::load("mdrender").check("parse", |c| Value::Array(mdpreview::parse(c.str(0)).iter().map(block).collect()));
}
