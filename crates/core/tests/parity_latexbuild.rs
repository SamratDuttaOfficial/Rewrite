//! latex.rs against rewrite/latexbuild.py (fixture group "latexbuild"), and real builds with the TeX
//! installed here (skipped without one).

use rewrite_core::latex;
use rewrite_testkit::{json, root, Fixture};
use serde_json::Value;

fn fixture() -> Fixture {
    Fixture::load("latexbuild")
}

#[test]
fn parse_log() {
    fixture().check("parse_log", |c| {
        let got = json(latex::parse_log(c.str(0), c.str(1)));
        if c.str(1) != "missing.tex" {
            return got;
        }
        // Plan decision 9: Python joined MiKTeX's "missing.tex:3: ==> Fatal error occurred" line to the
        // 79-character line above it (TeX's line wrapping) and reported the two as one junk error. The port
        // keeps the line apart and drops it like any "Fatal error" line after a real error, so the recorded
        // result is expected without its junk entry.
        let mut want = c.out.clone();
        let errors = want[0].as_array_mut().unwrap();
        errors.retain(|e| !e[1].as_str().unwrap().contains("Fatal error occurred"));
        assert_eq!(errors.len(), 1);
        assert_eq!(got, want);
        c.out.clone()
    });
}

#[test]
fn pick_engine() {
    // Python recorded this with every engine installed.
    fixture().check("pick_engine", |c| json(latex::pick_engine_with(c.str(0), &|_| true)));
    assert_eq!(latex::pick_engine_with("\\usepackage{fontspec}", &|e| e == "lualatex"), Some("lualatex"));
    assert_eq!(latex::pick_engine_with("x", &|_| false), None);
}

#[test]
fn build_dir_is_stable() {
    assert_eq!(latex::build_dir(None), latex::build_dir(None));
    assert_ne!(latex::build_dir(None), latex::build_dir(Some(std::path::Path::new("a.tex"))));
}

/// Copies a corpus log folder file into a fresh temp folder.
fn corpus_copy(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rewrite-latex-test-{}-{}", std::process::id(), name.replace('.', "-")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::copy(root().join("tests").join("corpus").join("logs").join(name), &path).unwrap();
    path
}

fn installed() -> bool {
    let ok = latex::pick_engine("").is_some();
    if !ok {
        eprintln!("no LaTeX installed: build tests skipped");
    }
    ok
}

#[test]
fn build_errors() {
    if !installed() {
        return;
    }
    let path = corpus_copy("errors.tex");
    let source = std::fs::read_to_string(&path).unwrap();
    let res = latex::build(&source, Some(&path), "utf-8", &|| false).expect("not cancelled");
    assert!(!res.engine.is_empty());
    assert!(res.errors.len() >= 5, "{:?}\n{}", res.errors, res.message);
    assert_eq!(res.errors[0], (Some(5), "Undefined control sequence \\foobar".to_string()), "{:?}", res.errors);
    assert!(res.errors.iter().any(|(l, m)| *l == Some(6) && m == "Missing $ inserted."), "{:?}", res.errors);
    assert!(res.log.contains("errors.tex"));
    // nothing is written next to the document
    let beside: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(beside.len(), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn build_warnings() {
    if !installed() {
        return;
    }
    let path = corpus_copy("warnings.tex");
    let source = std::fs::read_to_string(&path).unwrap();
    let res = latex::build(&source, Some(&path), "utf-8", &|| false).expect("not cancelled");
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert!(res.warnings > 0);
    let pdf = res.pdf.expect("a PDF");
    assert!(pdf.starts_with(b"%PDF"));
    assert_eq!(res.message, "");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn build_cancel() {
    if !installed() {
        return;
    }
    let src = "\\documentclass{article}\\begin{document}x\\end{document}";
    let path = std::env::temp_dir().join(format!("rewrite-cancel-{}.tex", std::process::id()));
    assert!(latex::build(src, Some(&path), "utf-8", &|| true).is_err());
}

#[test]
fn untitled_build_makes_a_pdf() {
    if !installed() {
        return;
    }
    let src = "\\documentclass{article}\\begin{document}Hello \\section{A}\\label{a} See \\ref{a}.\\end{document}";
    let res = latex::build(src, None, "cp1252", &|| false).unwrap();
    assert!(res.pdf.is_some(), "{} {:?}", res.message, res.errors);
    let _: Value = json(&res.errors);
}

#[test]
fn build_with_bibtex() {
    if !installed() || latex::find("bibtex").is_none() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("rewrite-bib-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("refs.bib"), "@book{knuth, author={Donald Knuth}, title={The TeXbook}, year={1984}}\n").unwrap();
    let path = dir.join("paper.tex");
    let src = r"\documentclass{article}\begin{document}See \cite{knuth}.\bibliographystyle{plain}\bibliography{refs}\end{document}";
    let res = latex::build(src, Some(&path), "utf-8", &|| false).unwrap();
    assert!(res.pdf.is_some(), "{} {:?}", res.message, res.errors);
    let out = latex::build_dir(Some(&path));
    assert!(std::fs::read_to_string(out.join("paper.bbl")).unwrap().contains("TeXbook"));
    assert!(out.join("paper.bibstamp").is_file());
    assert!(!res.log.contains("Citation `knuth' on page 1 undefined"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}
