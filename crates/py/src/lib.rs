//! The Python face of the native core. Documents and charts cross as dicts (see model.rs); heavy work
//! runs with the GIL released so the Tk window stays responsive.

use rewrite_core::charts::{self, Shape};
use rewrite_core::model::{Chart, Document, PageSetup};
use rewrite_core::{convert, fonts, imaging, layout, mathtex};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

fn doc_from(obj: &Bound<'_, PyAny>) -> PyResult<Document> {
    pythonize::depythonize(obj).map_err(|e| PyValueError::new_err(format!("bad document: {e}")))
}

fn chart_from(obj: &Bound<'_, PyAny>) -> PyResult<Chart> {
    pythonize::depythonize(obj).map_err(|e| PyValueError::new_err(format!("bad chart: {e}")))
}

/// PDF bytes for a document dict.
#[pyfunction]
#[pyo3(signature = (doc, generic="serif", numbers=None, page=None))]
fn render_pdf<'py>(py: Python<'py>, doc: &Bound<'py, PyAny>, generic: &str, numbers: Option<bool>,
                   page: Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyBytes>> {
    let doc = doc_from(doc)?;
    let page: Option<PageSetup> = match page {
        Some(p) if !p.is_none() => Some(pythonize::depythonize(p).map_err(|e| PyValueError::new_err(e.to_string()))?),
        _ => None,
    };
    let opts = layout::Options { generic: generic.to_string(), numbers, page };
    let out = py.detach(|| layout::render(&doc, &opts)).map_err(PyRuntimeError::new_err)?;
    Ok(PyBytes::new(py, &out))
}

/// A PDF with changes made: ops is a list of dicts with an "op" key (see pdfedit.rs).
#[pyfunction]
fn pdf_edit<'py>(py: Python<'py>, data: &[u8], ops: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let ops: Vec<rewrite_core::pdfedit::Op> = pythonize::depythonize(ops).map_err(|e| PyValueError::new_err(format!("bad edit: {e}")))?;
    let out = py.detach(|| rewrite_core::pdfedit::apply(data, &ops)).map_err(PyRuntimeError::new_err)?;
    Ok(PyBytes::new(py, &out))
}

/// (left, bottom, right, top, rotation) of every page of a PDF: its visible box and its turn.
#[pyfunction]
fn pdf_pages(data: &[u8]) -> PyResult<Vec<(f64, f64, f64, f64, i64)>> {
    rewrite_core::pdfedit::pages(data).map_err(PyRuntimeError::new_err)
}

/// A document dict in the form the rich-text editor holds it (see flat.rs): {"text", "lines", "spans",
/// "objects", "labels"}.
#[pyfunction]
fn rich_flatten<'py>(py: Python<'py>, doc: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let doc = doc_from(doc)?;
    pythonize::pythonize(py, &rewrite_core::flat::flatten(&doc)).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// The document dict for an editor's content; `meta` (a document dict) gives the title, page set-up and
/// the rest that the editor does not hold.
#[pyfunction]
fn rich_build<'py>(py: Python<'py>, flat: &Bound<'py, PyAny>, meta: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let f: rewrite_core::flat::Flat = pythonize::depythonize(flat).map_err(|e| PyValueError::new_err(format!("bad editor content: {e}")))?;
    let meta = doc_from(meta)?;
    pythonize::pythonize(py, &rewrite_core::flat::build(&f, meta)).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// List markers for lines given as [(kind, level) or None].
#[pyfunction]
fn rich_labels(lists: Vec<Option<(String, u32)>>) -> Vec<Option<String>> {
    rewrite_core::flat::labels(&lists)
}

/// The shapes of a chart in a w x h point box, measuring text with measure(text, size, bold) -> width.
/// Each shape is a tuple: ("rect", x, y, w, h, fill, stroke), ("line", points, color, width),
/// ("poly", points, fill, stroke), ("dot", x, y, r, fill), ("text", x, y, text, size, color, anchor, bold, angle).
#[pyfunction]
fn chart_shapes<'py>(py: Python<'py>, chart: &Bound<'py, PyAny>, w: f64, h: f64, measure: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyTuple>>> {
    let chart = chart_from(chart)?;
    let m = |t: &str, size: f64, bold: bool| -> f64 {
        measure.call1((t, size, bold)).and_then(|v| v.extract::<f64>()).unwrap_or_else(|_| charts::approx_measure(t, size, bold))
    };
    let mut out = vec![];
    for s in charts::layout(&chart, w, h, &m) {
        let t = match s {
            Shape::Rect { x, y, w, h, fill, stroke } => ("rect", x, y, w, h, fill, stroke).into_pyobject(py)?,
            Shape::Line { pts, color, width } => ("line", pts, color, width).into_pyobject(py)?,
            Shape::Poly { pts, fill, stroke } => ("poly", pts, fill, stroke).into_pyobject(py)?,
            Shape::Dot { x, y, r, fill } => ("dot", x, y, r, fill).into_pyobject(py)?,
            Shape::Text { x, y, text, size, color, anchor, bold, angle } => ("text", x, y, text, size, color, anchor, bold, angle).into_pyobject(py)?,
        };
        out.push(t);
    }
    Ok(out)
}

#[pyfunction]
fn chart_svg(chart: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(charts::svg(&chart_from(chart)?))
}

/// Chart data typed as rows -> (categories, [(series name, [values])]).
#[pyfunction]
fn chart_parse_table(text: &str) -> (Vec<String>, Vec<(String, Vec<Option<f64>>)>) {
    charts::parse_table(text)
}

#[pyfunction]
fn chart_format_table(chart: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(charts::format_table(&chart_from(chart)?))
}

#[pyfunction]
fn chart_kinds() -> Vec<(&'static str, &'static str)> {
    charts::KINDS.to_vec()
}

#[pyfunction]
fn chart_palette() -> Vec<&'static str> {
    charts::PALETTE.to_vec()
}

#[pyfunction]
#[pyo3(signature = (tex, display=false))]
fn math_to_omml(tex: &str, display: bool) -> String {
    mathtex::to_omml(tex, display)
}

#[pyfunction]
fn math_from_omml(xml: &str) -> String {
    mathtex::omml_xml_to_tex(xml)
}

#[pyfunction]
#[pyo3(signature = (tex, display=true))]
fn math_to_mathml(tex: &str, display: bool) -> String {
    mathtex::to_mathml(tex, display)
}

#[pyfunction]
fn math_from_mathml(xml: &str) -> String {
    mathtex::mathml_xml_to_tex(xml)
}

/// The maths as [(text, level, italic, bold)], level +1 superscript, -1 subscript.
#[pyfunction]
fn math_runs(tex: &str) -> Vec<(String, i32, bool, bool)> {
    mathtex::runs(tex)
}

#[pyfunction]
fn math_unicode(tex: &str) -> String {
    mathtex::to_unicode(tex)
}

/// (format, width px, height px, dpi) of picture bytes; format is "" when unknown.
#[pyfunction]
fn image_info(data: &[u8]) -> (String, Option<(u32, u32)>, f64) {
    (imaging::sniff(data).to_string(), imaging::size(data), imaging::dpi(data))
}

/// The size in points a picture takes at its own resolution, no wider than max_width.
#[pyfunction]
#[pyo3(signature = (data, max_width=None))]
fn image_natural_size(data: &[u8], max_width: Option<f64>) -> (f64, f64) {
    imaging::natural_size(data, max_width)
}

/// A picture resized to w x h pixels as PPM bytes (for Tk), transparency on the background colour.
#[pyfunction]
fn image_thumbnail<'py>(py: Python<'py>, data: &[u8], w: u32, h: u32, bg: (u8, u8, u8)) -> PyResult<Bound<'py, PyBytes>> {
    let data = data.to_vec();
    let out = py.detach(|| imaging::thumbnail_ppm(&data, w, h, bg)).map_err(PyValueError::new_err)?;
    Ok(PyBytes::new(py, &out))
}

#[pyfunction]
fn image_to_png<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    let data = data.to_vec();
    let out = py.detach(|| imaging::to_png(&data)).map_err(PyValueError::new_err)?;
    Ok(PyBytes::new(py, &out))
}

/// PNG bytes for raw pixels (RGB or RGBA rows, top to bottom).
#[pyfunction]
fn image_encode_png<'py>(py: Python<'py>, pixels: &[u8], w: u32, h: u32, channels: u8) -> PyResult<Bound<'py, PyBytes>> {
    let img = match channels {
        3 => image::RgbImage::from_raw(w, h, pixels.to_vec()).map(image::DynamicImage::ImageRgb8),
        4 => image::RgbaImage::from_raw(w, h, pixels.to_vec()).map(image::DynamicImage::ImageRgba8),
        _ => None,
    }
    .ok_or_else(|| PyValueError::new_err("pixel data does not match the size"))?;
    Ok(PyBytes::new(py, &imaging::encode_png(&img)))
}

#[pyfunction]
fn font_families(py: Python<'_>) -> Vec<String> {
    py.detach(fonts::families)
}

/// The Python helper object (known, zipf, choose) seen as a textclean::Helper.
struct PyHelper<'py> {
    obj: Option<Bound<'py, PyAny>>,
}

impl rewrite_core::textclean::Helper for PyHelper<'_> {
    fn known(&self, word: &str) -> bool {
        self.obj.as_ref().and_then(|o| o.call_method1("known", (word,)).ok()).and_then(|v| v.extract().ok()).unwrap_or(false)
    }
    fn zipf(&self, word: &str) -> f64 {
        self.obj.as_ref().and_then(|o| o.call_method1("zipf", (word,)).ok()).and_then(|v| v.extract().ok()).unwrap_or(0.0)
    }
    fn choose(&self, before: &str, options: &[String], after: &str) -> Option<usize> {
        let v = self.obj.as_ref()?.call_method1("choose", (before, options.to_vec(), after)).ok()?;
        let k: i64 = v.extract().ok()?;
        if k < 0 { None } else { Some(k as usize) }
    }
}

/// A document dict from PDF pages as read by PDFium: [{"width", "height", "chars": [(text, x0, top, x1,
/// bottom, size, bold, italic, generated, mono, baseline)], "images": [{"x0", "top", "x1", "bottom",
/// "data"}], "shapes": [(x0, top, x1, bottom, kind)]}].
/// helper (optional): known(word), zipf(word), choose(before, options, after) for the doubtful cases.
/// render (optional): render(page, x0, top, x1, bottom) -> PNG bytes of that part of a page, for
/// drawings and equations; without it they stay as text.
#[pyfunction]
#[pyo3(signature = (pages, options=None, helper=None, render=None))]
fn pdf_document<'py>(py: Python<'py>, pages: &Bound<'py, PyAny>, options: Option<&Bound<'py, PyAny>>, helper: Option<Bound<'py, PyAny>>,
                     render: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
    let pages: Vec<rewrite_core::pdfextract::PPage> = pythonize::depythonize(pages).map_err(|e| PyValueError::new_err(format!("bad pages: {e}")))?;
    let opts: rewrite_core::pdfextract::Options = match options {
        Some(o) if !o.is_none() => pythonize::depythonize(o).map_err(|e| PyValueError::new_err(e.to_string()))?,
        _ => Default::default(),
    };
    let h = PyHelper { obj: helper.filter(|h| !h.is_none()) };
    let render = render.filter(|r| !r.is_none());
    let draw = |k: usize, x0: f64, top: f64, x1: f64, bottom: f64| -> Option<Vec<u8>> {
        let v = render.as_ref()?.call1((k, x0, top, x1, bottom)).ok()?;
        if v.is_none() {
            return None;
        }
        v.extract::<Vec<u8>>().ok()
    };
    let doc = rewrite_core::pdfextract::convert(&pages, &opts, &h, &draw);
    pythonize::pythonize(py, &doc).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// Character offsets where sentences start in a paragraph (after the first one).
#[pyfunction]
#[pyo3(signature = (text, helper=None))]
fn sentence_starts(text: &str, helper: Option<Bound<'_, PyAny>>) -> Vec<usize> {
    let h = PyHelper { obj: helper.filter(|h| !h.is_none()) };
    let bytes = rewrite_core::textclean::sentence_starts(text, &h);
    let mut out = vec![];
    let mut chars = 0;
    let mut bi = 0;
    for (b, _) in text.char_indices() {
        while bi < bytes.len() && bytes[bi] == b {
            out.push(chars);
            bi += 1;
        }
        chars += 1;
    }
    out
}

/// Full path of LibreOffice, or None.
#[pyfunction]
fn office_path() -> Option<String> {
    rewrite_core::office::find().map(|p| p.to_string_lossy().into_owned())
}

/// Convert file bytes with LibreOffice: (data, ".doc", "docx"). cancelled() -> bool is asked now and then.
#[pyfunction]
#[pyo3(signature = (data, src_ext, target, timeout=180, cancelled=None))]
fn office_convert<'py>(py: Python<'py>, data: &[u8], src_ext: &str, target: &str, timeout: u64, cancelled: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyBytes>> {
    let data = data.to_vec();
    let cancel_obj: Option<Py<PyAny>> = cancelled.map(|c| c.unbind());
    let out = py.detach(|| {
        let check = || -> bool {
            match &cancel_obj {
                Some(c) => Python::attach(|py| c.call0(py).and_then(|v| v.extract::<bool>(py)).unwrap_or(false)),
                None => false,
            }
        };
        rewrite_core::office::convert(&data, src_ext, target, timeout, &check)
    }).map_err(PyRuntimeError::new_err)?;
    Ok(PyBytes::new(py, &out))
}

/// A document dict from file bytes. fmt: md, tex, txt, docx, odt, fodt, rtf, doc, html.
/// base_dir: where pictures named by Markdown or LaTeX are found; pdf_figure(bytes) -> PNG bytes draws
/// a PDF figure of a LaTeX document.
#[pyfunction]
#[pyo3(signature = (fmt, data, base_dir=None, pdf_figure=None))]
fn read<'py>(py: Python<'py>, fmt: &str, data: &[u8], base_dir: Option<String>, pdf_figure: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
    let base = base_dir.as_deref().map(std::path::Path::new);
    let doc = match &pdf_figure {
        Some(cb) if !cb.is_none() => {
            let f = |bytes: &[u8]| -> Option<Vec<u8>> {
                cb.call1((PyBytes::new(py, bytes),)).ok().and_then(|v| if v.is_none() { None } else { v.extract::<Vec<u8>>().ok() })
            };
            convert::read(fmt, data, base, Some(&f))
        }
        _ => {
            let data = data.to_vec();
            let fmt = fmt.to_string();
            py.detach(|| convert::read(&fmt, &data, base, None))
        }
    }
    .map_err(PyValueError::new_err)?;
    pythonize::pythonize(py, &doc).map_err(|e| PyRuntimeError::new_err(e.to_string()))
}

/// (bytes, {relative path: bytes}) for a document dict written as fmt. sentences(text) -> [offsets of
/// sentence starts, in characters] puts each sentence on its own line (Markdown, LaTeX, text).
#[pyfunction]
#[pyo3(signature = (fmt, doc, stem="document", sentences=None))]
fn write<'py>(py: Python<'py>, fmt: &str, doc: &Bound<'py, PyAny>, stem: &str, sentences: Option<Bound<'py, PyAny>>) -> PyResult<(Bound<'py, PyBytes>, std::collections::BTreeMap<String, Bound<'py, PyBytes>>)> {
    let doc = doc_from(doc)?;
    let result = match &sentences {
        Some(cb) if !cb.is_none() => {
            let f = |text: &str| -> Vec<usize> {
                let chars: Vec<usize> = cb.call1((text,)).and_then(|v| v.extract()).unwrap_or_default();
                let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect();
                chars.into_iter().filter_map(|c| starts.get(c).copied()).collect()
            };
            convert::write(fmt, &doc, stem, Some(&f))
        }
        _ => {
            let fmt = fmt.to_string();
            let stem = stem.to_string();
            py.detach(|| convert::write(&fmt, &doc, &stem, None))
        }
    }
    .map_err(PyValueError::new_err)?;
    let files = result.1.into_iter().map(|(k, v)| (k, PyBytes::new(py, &v))).collect();
    Ok((PyBytes::new(py, &result.0), files))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(render_pdf, m)?)?;
    m.add_function(wrap_pyfunction!(rich_flatten, m)?)?;
    m.add_function(wrap_pyfunction!(pdf_edit, m)?)?;
    m.add_function(wrap_pyfunction!(pdf_pages, m)?)?;
    m.add_function(wrap_pyfunction!(rich_build, m)?)?;
    m.add_function(wrap_pyfunction!(rich_labels, m)?)?;
    m.add_function(wrap_pyfunction!(chart_shapes, m)?)?;
    m.add_function(wrap_pyfunction!(chart_svg, m)?)?;
    m.add_function(wrap_pyfunction!(chart_parse_table, m)?)?;
    m.add_function(wrap_pyfunction!(chart_format_table, m)?)?;
    m.add_function(wrap_pyfunction!(chart_kinds, m)?)?;
    m.add_function(wrap_pyfunction!(chart_palette, m)?)?;
    m.add_function(wrap_pyfunction!(math_to_omml, m)?)?;
    m.add_function(wrap_pyfunction!(math_from_omml, m)?)?;
    m.add_function(wrap_pyfunction!(math_to_mathml, m)?)?;
    m.add_function(wrap_pyfunction!(math_from_mathml, m)?)?;
    m.add_function(wrap_pyfunction!(math_runs, m)?)?;
    m.add_function(wrap_pyfunction!(math_unicode, m)?)?;
    m.add_function(wrap_pyfunction!(image_info, m)?)?;
    m.add_function(wrap_pyfunction!(image_natural_size, m)?)?;
    m.add_function(wrap_pyfunction!(image_thumbnail, m)?)?;
    m.add_function(wrap_pyfunction!(image_to_png, m)?)?;
    m.add_function(wrap_pyfunction!(image_encode_png, m)?)?;
    m.add_function(wrap_pyfunction!(font_families, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(pdf_document, m)?)?;
    m.add_function(wrap_pyfunction!(sentence_starts, m)?)?;
    m.add_function(wrap_pyfunction!(office_path, m)?)?;
    m.add_function(wrap_pyfunction!(office_convert, m)?)?;
    m.add_function(wrap_pyfunction!(write, m)?)?;
    Ok(())
}

#[pymodule]
fn rewrite_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    register(m)
}
