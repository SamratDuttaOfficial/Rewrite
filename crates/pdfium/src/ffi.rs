//! The PDFium functions Rewrite calls, loaded from the shipped library at run time: no link-time
//! dependency, so the build needs no PDFium SDK and works with the GNU toolchain.
//!
//! PDFium is not thread-safe. The table lives behind one global mutex ([`lock`]), and every call is
//! made while holding it.

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_ushort, c_void};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub type Doc = *mut c_void;
pub type Page = *mut c_void;
pub type TextPage = *mut c_void;
pub type Obj = *mut c_void;
pub type Bitmap = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

#[repr(C)]
#[derive(Default)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[repr(C)]
#[derive(Default)]
pub struct SizeF {
    pub width: f32,
    pub height: f32,
}

/// FPDF_LIBRARY_CONFIG, version 2 (the one pypdfium2 used).
#[repr(C)]
pub struct Config {
    version: c_int,
    user_font_paths: *const *const c_char,
    isolate: *mut c_void,
    v8_embedder_slot: c_uint,
}

pub const OBJ_TEXT: c_int = 1;
pub const OBJ_PATH: c_int = 2;
pub const OBJ_IMAGE: c_int = 3;
pub const OBJ_SHADING: c_int = 4;
pub const OBJ_FORM: c_int = 5;

pub const BITMAP_GRAY: c_int = 1;
pub const BITMAP_BGR: c_int = 2;
pub const BITMAP_BGRX: c_int = 3;
pub const BITMAP_BGRA: c_int = 4;
pub const BITMAP_BGRA_PREMUL: c_int = 5;

pub const RENDER_ANNOT: c_int = 0x01;
pub const RENDER_REVERSE_BYTE_ORDER: c_int = 0x10;

macro_rules! api {
    ($($name:ident($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        pub struct Api {
            $(pub $name: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
            _lib: libloading::Library,
        }

        impl Api {
            unsafe fn load(lib: libloading::Library) -> Result<Api, String> {
                Ok(Api {
                    $($name: unsafe { *lib.get::<unsafe extern "system" fn($($arg),*) $(-> $ret)?>(stringify!($name)).map_err(|e| format!("PDFium has no {}: {e}", stringify!($name)))? },)*
                    _lib: lib,
                })
            }
        }
    };
}

api! {
    FPDF_InitLibraryWithConfig(*const Config);
    FPDF_GetLastError() -> c_ulong;
    FPDF_LoadMemDocument64(*const c_void, usize, *const c_char) -> Doc;
    FPDF_CloseDocument(Doc);
    FPDF_GetPageCount(Doc) -> c_int;
    FPDF_GetPageSizeByIndexF(Doc, c_int, *mut SizeF) -> c_int;
    FPDF_LoadPage(Doc, c_int) -> Page;
    FPDF_ClosePage(Page);
    FPDF_GetPageWidthF(Page) -> f32;
    FPDF_GetPageHeightF(Page) -> f32;
    FPDF_RenderPageBitmap(Bitmap, Page, c_int, c_int, c_int, c_int, c_int, c_int);
    FPDFBitmap_CreateEx(c_int, c_int, c_int, *mut c_void, c_int) -> Bitmap;
    FPDFBitmap_FillRect(Bitmap, c_int, c_int, c_int, c_int, c_ulong) -> c_int;
    FPDFBitmap_Destroy(Bitmap);
    FPDFBitmap_GetFormat(Bitmap) -> c_int;
    FPDFBitmap_GetWidth(Bitmap) -> c_int;
    FPDFBitmap_GetHeight(Bitmap) -> c_int;
    FPDFBitmap_GetStride(Bitmap) -> c_int;
    FPDFBitmap_GetBuffer(Bitmap) -> *mut c_void;
    FPDFText_LoadPage(Page) -> TextPage;
    FPDFText_ClosePage(TextPage);
    FPDFText_CountChars(TextPage) -> c_int;
    FPDFText_GetUnicode(TextPage, c_int) -> c_uint;
    FPDFText_IsGenerated(TextPage, c_int) -> c_int;
    FPDFText_GetCharBox(TextPage, c_int, *mut f64, *mut f64, *mut f64, *mut f64) -> c_int;
    FPDFText_GetLooseCharBox(TextPage, c_int, *mut RectF) -> c_int;
    FPDFText_GetFontSize(TextPage, c_int) -> f64;
    FPDFText_GetFontInfo(TextPage, c_int, *mut c_void, c_ulong, *mut c_int) -> c_ulong;
    FPDFText_GetFontWeight(TextPage, c_int) -> c_int;
    FPDFText_GetTextObject(TextPage, c_int) -> Obj;
    FPDFText_GetCharOrigin(TextPage, c_int, *mut f64, *mut f64) -> c_int;
    FPDFText_GetBoundedText(TextPage, f64, f64, f64, f64, *mut c_ushort, c_int) -> c_int;
    FPDFPage_CountObjects(Page) -> c_int;
    FPDFPage_GetObject(Page, c_int) -> Obj;
    FPDFPageObj_GetType(Obj) -> c_int;
    FPDFPageObj_GetBounds(Obj, *mut f32, *mut f32, *mut f32, *mut f32) -> c_int;
    FPDFPageObj_GetMatrix(Obj, *mut Matrix) -> c_int;
    FPDFPageObj_SetMatrix(Obj, *const Matrix) -> c_int;
    FPDFPageObj_GetFillColor(Obj, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_int;
    FPDFPath_GetDrawMode(Obj, *mut c_int, *mut c_int) -> c_int;
    FPDFFormObj_CountObjects(Obj) -> c_int;
    FPDFFormObj_GetObject(Obj, c_ulong) -> Obj;
    FPDFImageObj_GetImagePixelSize(Obj, *mut c_uint, *mut c_uint) -> c_int;
    FPDFImageObj_GetRenderedBitmap(Doc, Page, Obj) -> Bitmap;
}

// Function pointers and the library handle may move between threads; the mutex makes the calls take turns.
unsafe impl Send for Api {}

/// The platform's file name of the PDFium library.
pub const LIBRARY: &str = if cfg!(windows) {
    "pdfium.dll"
} else if cfg!(target_os = "macos") {
    "libpdfium.dylib"
} else {
    "libpdfium.so"
};

static PDFIUM: OnceLock<Result<Mutex<Api>, String>> = OnceLock::new();

/// The PDFium function table, loaded and initialised on first use, held for the caller alone.
pub fn lock() -> Result<MutexGuard<'static, Api>, String> {
    let api = PDFIUM.get_or_init(|| {
        // The shipped copy first (next to the program or in runtime/<platform>/), then the system's.
        let path = rewrite_core::paths::runtime_library(LIBRARY).map(|p| p.into_os_string()).unwrap_or_else(|| LIBRARY.into());
        let lib = unsafe { libloading::Library::new(&path) }.map_err(|e| format!("PDFium could not be loaded: {e}"))?;
        let api = unsafe { Api::load(lib)? };
        let config = Config { version: 2, user_font_paths: std::ptr::null(), isolate: std::ptr::null_mut(), v8_embedder_slot: 0 };
        unsafe { (api.FPDF_InitLibraryWithConfig)(&config) };
        Ok(Mutex::new(api))
    });
    match api {
        Ok(m) => Ok(m.lock().unwrap_or_else(|e| e.into_inner())),
        Err(e) => Err(e.clone()),
    }
}
