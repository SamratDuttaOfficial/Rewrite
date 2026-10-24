//! Rewrite's native core: document formats, PDF writing and layout, charts, maths and pictures.
//! The Tk app reaches it through the `rewrite_native` Python module (crates/py).

pub mod charts;
pub mod convert;
pub mod doc;
pub mod flat;
pub mod docx;
pub mod fonts;
pub mod html;
pub mod imaging;
pub mod layout;
pub mod mathtex;
pub mod md;
pub mod model;
pub mod office;
pub mod odt;
pub mod opc;
pub mod pdf;
pub mod pdfedit;
pub mod pdfextract;
pub mod rtf;
pub mod tex;
pub mod textclean;
pub mod xml;

