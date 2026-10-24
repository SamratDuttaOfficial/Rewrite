//! Rewrite's native core: document formats, PDF writing and layout, charts, maths and pictures, opening
//! and saving files, protected spans, LaTeX builds, the Markdown preview and export.
//! The Tk app reaches it through the `rewrite_native` Python module (crates/py).

pub mod charts;
pub mod convert;
pub mod doc;
pub mod files;
pub mod flat;
pub mod docx;
pub mod export;
pub mod fonts;
pub mod html;
pub mod imaging;
pub mod latex;
pub mod layout;
pub mod mathtex;
pub mod md;
pub mod mdpreview;
pub mod model;
pub mod office;
pub mod odt;
pub mod opc;
pub mod paths;
pub mod pdf;
pub mod pdfedit;
pub mod pdfextract;
pub mod protect;
pub mod rtf;
pub mod tex;
pub mod textclean;
pub mod xml;

