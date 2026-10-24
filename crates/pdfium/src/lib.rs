//! PDF reading and rendering through PDFium for Rewrite: pages drawn for the views, characters with their
//! boxes and fonts for the PDF editor, and pages read for conversion (read.rs), which
//! `rewrite_core::pdfextract` turns into a Document.
//!
//! PDFium is loaded from the shipped library at run time (ffi.rs). It must never run on two threads at
//! once: every call below holds one global lock, so a [`Document`] can be shared between threads.

mod ffi;
mod read;

pub use read::{first_page_png, to_document};

use ffi::{Api, BITMAP_BGR, RENDER_ANNOT, RENDER_REVERSE_BYTE_ORDER};
use rewrite_core::pdfextract::PPage;
use std::ffi::c_int;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, MutexGuard};

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The PDFium library is missing or could not be loaded.
    Unavailable(String),
    /// PDFium could not open the file, a page or a text page, or draw a page.
    Pdfium(String),
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable(m) | Error::Pdfium(m) => f.write_str(m),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn lock() -> Result<MutexGuard<'static, Api>> {
    ffi::lock().map_err(Error::Unavailable)
}

/// Whether PDFium can be loaded.
pub fn available() -> bool {
    ffi::lock().is_ok()
}

/// An open PDF. The bytes are kept alive with it, as PDFium reads them lazily.
pub struct Document {
    raw: ffi::Doc,
    _data: Arc<[u8]>,
}

// The handle is only used while holding the global lock.
unsafe impl Send for Document {}
unsafe impl Sync for Document {}

/// Drawn page pixels, RGBA (opaque, on white), `width * 4` bytes per row.
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// A character as the PDF editor uses it: points from the given page corner, font size, bold and italic,
/// and the font's name (to write new text in a matching face).
#[derive(Clone, Debug, PartialEq)]
pub struct Char {
    pub text: String,
    pub x0: f64,
    pub top: f64,
    pub x1: f64,
    pub bottom: f64,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub font: String,
}

impl Document {
    pub fn open(data: impl Into<Arc<[u8]>>) -> Result<Document> {
        let data = data.into();
        let api = lock()?;
        let raw = unsafe { (api.FPDF_LoadMemDocument64)(data.as_ptr().cast(), data.len(), std::ptr::null()) };
        if raw.is_null() {
            let why = match unsafe { (api.FPDF_GetLastError)() } {
                2 => "File access error",
                3 => "Data format error",
                4 => "Incorrect password error",
                5 => "Unsupported security scheme error",
                6 => "Page not found or content error",
                _ => "Unknown error",
            };
            return Err(Error::Pdfium(format!("Failed to load document (PDFium: {why}).")));
        }
        Ok(Document { raw, _data: data })
    }

    pub fn page_count(&self) -> usize {
        lock().map(|api| unsafe { (api.FPDF_GetPageCount)(self.raw) }.max(0) as usize).unwrap_or(0)
    }

    /// Width and height of every page in points, as shown (after the page's own turn).
    pub fn page_sizes(&self) -> Result<Vec<(f64, f64)>> {
        let api = lock()?;
        let n = unsafe { (api.FPDF_GetPageCount)(self.raw) }.max(0);
        (0..n)
            .map(|k| {
                let mut s = ffi::SizeF::default();
                if unsafe { (api.FPDF_GetPageSizeByIndexF)(self.raw, k, &mut s) } == 0 {
                    return Err(Error::Pdfium(format!("Failed to get size of page {k}.")));
                }
                Ok((s.width as f64, s.height as f64))
            })
            .collect()
    }

    /// A page drawn at `scale` pixels per point (annotations included), `ceil(width * scale)` by
    /// `ceil(height * scale)` pixels.
    pub fn render_page(&self, page: usize, scale: f64) -> Result<Bitmap> {
        let api = lock()?;
        let p = PageRef::load(&api, self.raw, page)?;
        let (width, height, pixels) = render(&api, p.raw, scale, [0.0; 4], ffi::BITMAP_BGRA, RENDER_ANNOT | RENDER_REVERSE_BYTE_ORDER)?;
        Ok(Bitmap { width, height, pixels })
    }

    /// The characters of a page (those PDFium infers too), measured from `left` and down from `top`: the
    /// PDF editor passes the stored page box, so positions match what it sends to the core.
    pub fn chars(&self, page: usize, left: f64, top: f64) -> Result<Vec<Char>> {
        let api = lock()?;
        let p = PageRef::load(&api, self.raw, page)?;
        let tp = TextRef::load(&api, p.raw)?;
        Ok(read::named_chars(&api, tp.raw, left, top))
    }

    /// The text PDFium finds inside a rectangle, in PDF coordinates (points up from the bottom-left).
    pub fn text_bounded(&self, page: usize, left: f64, bottom: f64, right: f64, top: f64) -> Result<String> {
        let api = lock()?;
        let p = PageRef::load(&api, self.raw, page)?;
        let tp = TextRef::load(&api, p.raw)?;
        let n = unsafe { (api.FPDFText_GetBoundedText)(tp.raw, left, top, right, bottom, std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Ok(String::new());
        }
        let mut buf = vec![0u16; n as usize];
        let got = unsafe { (api.FPDFText_GetBoundedText)(tp.raw, left, top, right, bottom, buf.as_mut_ptr(), n) };
        buf.truncate(got.clamp(0, n) as usize);
        // Unpaired surrogates are dropped, as Python's decode(errors="ignore") did.
        Ok(char::decode_utf16(buf).filter_map(|c| c.ok()).collect())
    }

    /// Every page as `pdfextract` reads it; PDFium is held only a page at a time. `progress(k, n)` is told
    /// before page k; setting `cancel` stops with [`Error::Cancelled`].
    pub fn read_pages(&self, images: bool, progress: Option<&dyn Fn(usize, usize)>, cancel: Option<&AtomicBool>) -> Result<Vec<PPage>> {
        read::read_pages(self, images, progress, cancel)
    }

    /// A part of a page (points from the top-left) as PNG, sharp enough to read: the drawings and
    /// equations `pdfextract` cuts out as pictures. None when the part is too small or cannot be drawn.
    pub fn render_area(&self, page: usize, x0: f64, top: f64, x1: f64, bottom: f64) -> Option<Vec<u8>> {
        read::render_area(self, page, x0, top, x1, bottom)
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        if let Ok(api) = lock() {
            unsafe { (api.FPDF_CloseDocument)(self.raw) };
        }
    }
}

/// The open document of the bytes shown now, reopened only when their version changes, so the views can
/// render page by page without opening the file each time.
#[derive(Default)]
pub struct Cached {
    open: Option<(u64, Document)>,
}

impl Cached {
    /// The document of `version`, opened from `data()` if the version differs from the last call's.
    pub fn get(&mut self, version: u64, data: impl FnOnce() -> Arc<[u8]>) -> Result<&Document> {
        if !matches!(&self.open, Some((v, _)) if *v == version) {
            self.open = None;
            self.open = Some((version, Document::open(data())?));
        }
        Ok(&self.open.as_ref().expect("opened above").1)
    }

    pub fn clear(&mut self) {
        self.open = None;
    }
}

/// A loaded page, closed when dropped (while the lock is still held).
struct PageRef<'a> {
    api: &'a Api,
    raw: ffi::Page,
}

impl<'a> PageRef<'a> {
    fn load(api: &'a Api, doc: ffi::Doc, k: usize) -> Result<PageRef<'a>> {
        let raw = c_int::try_from(k).map_or(std::ptr::null_mut(), |k| unsafe { (api.FPDF_LoadPage)(doc, k) });
        if raw.is_null() {
            return Err(Error::Pdfium(format!("Failed to load page {k}.")));
        }
        Ok(PageRef { api, raw })
    }

    fn size(&self) -> (f64, f64) {
        unsafe { ((self.api.FPDF_GetPageWidthF)(self.raw) as f64, (self.api.FPDF_GetPageHeightF)(self.raw) as f64) }
    }
}

impl Drop for PageRef<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.FPDF_ClosePage)(self.raw) };
    }
}

struct TextRef<'a> {
    api: &'a Api,
    raw: ffi::TextPage,
}

impl<'a> TextRef<'a> {
    fn load(api: &'a Api, page: ffi::Page) -> Result<TextRef<'a>> {
        let raw = unsafe { (api.FPDFText_LoadPage)(page) };
        if raw.is_null() {
            return Err(Error::Pdfium("Failed to load text page.".into()));
        }
        Ok(TextRef { api, raw })
    }
}

impl Drop for TextRef<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.FPDFText_ClosePage)(self.raw) };
    }
}

/// A page drawn on white at `scale` with `crop` (left, bottom, right, top, in points) cut off, as
/// pypdfium2's `PdfPage.render` did: (width, height, packed pixels in `format`).
fn render(api: &Api, page: ffi::Page, scale: f64, crop: [f64; 4], format: c_int, flags: c_int) -> Result<(usize, usize, Vec<u8>)> {
    let (w, h) = unsafe { ((api.FPDF_GetPageWidthF)(page) as f64, (api.FPDF_GetPageHeightF)(page) as f64) };
    let (src_w, src_h) = ((w * scale).ceil() as i64, (h * scale).ceil() as i64);
    let c = crop.map(|c| (c * scale).ceil() as i64);
    let (width, height) = (src_w - c[0] - c[2], src_h - c[1] - c[3]);
    if width < 1 || height < 1 {
        return Err(Error::Pdfium("Crop exceeds page dimensions".into()));
    }
    let bpp = if format == BITMAP_BGR { 3 } else { 4 };
    if width * height * bpp > i32::MAX as i64 || src_w > i32::MAX as i64 || src_h > i32::MAX as i64 {
        return Err(Error::Pdfium("Page too large to draw at this scale".into()));
    }
    let (width, height, bpp) = (width as usize, height as usize, bpp as usize);
    let mut pixels = vec![0u8; width * height * bpp];
    unsafe {
        let bm = (api.FPDFBitmap_CreateEx)(width as c_int, height as c_int, format, pixels.as_mut_ptr().cast(), (width * bpp) as c_int);
        if bm.is_null() {
            return Err(Error::Pdfium("Failed to create bitmap.".into()));
        }
        (api.FPDFBitmap_FillRect)(bm, 0, 0, width as c_int, height as c_int, 0xFFFF_FFFF);
        (api.FPDF_RenderPageBitmap)(bm, page, -c[0] as c_int, -c[3] as c_int, src_w as c_int, src_h as c_int, 0, flags);
        (api.FPDFBitmap_Destroy)(bm);
    }
    Ok((width, height, pixels))
}
