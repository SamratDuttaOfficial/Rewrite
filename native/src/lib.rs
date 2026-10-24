//! Rewrite's native core: document formats, PDF writing and layout, charts, maths and pictures.
//! Python reaches it through the `rewrite_native` module; documents travel as plain dicts.

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

mod py;

use pyo3::prelude::*;

#[pymodule]
fn rewrite_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    py::register(m)
}
