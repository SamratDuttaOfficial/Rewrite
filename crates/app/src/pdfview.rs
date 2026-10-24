//! The PDF view: pages drawn by PDFium, with tools to mark, annotate and edit them (PdfEditor), and the
//! bar of tools shown above it (PdfBar). The preview panel uses the same view to show PDFs.
//!
//! Pages are drawn as they scroll into view, on a worker thread of the view's own; every change is made by
//! the core (`pdfedit::apply`) and gives a new version of the file, so undo simply goes back to the version
//! before. Positions sent to the core are in points from the top-left of the page as stored, before the
//! page's own turn (/Rotate).

use crate::app::{App, AppRc};
use crate::theme::MOD;
use crate::widgets::{self, Toggle};
use crate::workers::{self, Lane};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, gio, glib, graphene, gsk, pango};
use regex::Regex;
use rewrite_core::pdfedit::{self, Op};
use rewrite_pdfium::{Cached, Char};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, LazyLock, Mutex};

pub const TOOLS: [(&str, &str, &str); 11] = [
    ("select", "pointer", "Select text: drag over it, then copy it (Ctrl+C) or right-click to mark it"),
    ("edit", "pencil", "Edit text: click a line to change its words"),
    ("add", "type", "Add text: click where it goes"),
    ("highlight", "highlight", "Highlight: drag over text"),
    ("underline", "underline", "Underline: drag over text"),
    ("strike", "strike", "Strike out: drag over text"),
    ("note", "note", "Sticky note: click where it goes"),
    ("draw", "draw", "Draw by hand"),
    ("whiteout", "eraser", "White-out: drag a box to cover what is under it"),
    ("box", "box", "Rectangle: drag a box"),
    ("image", "image", "Picture: click where it goes"),
];
const MAX_UNDO_BYTES: usize = 400_000_000;
const PICTURE_TYPES: &[(&str, &[&str])] = &[
    ("Pictures", &["*.png", "*.jpg", "*.jpeg", "*.gif", "*.bmp", "*.tif", "*.tiff", "*.webp"]),
    ("All files", &["*"]),
];
/// The widest a page is drawn, in device pixels (memory: a page this wide is about 60 MB).
const MAX_RENDER_WIDTH: f64 = 4800.0;

fn mark_color(kind: &str) -> Option<&'static str> {
    match kind {
        "highlight" => Some("#FFE14D"),
        "underline" | "strike" => Some("#E03131"),
        _ => None,
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> graphene::Rect {
    graphene::Rect::new(x as f32, y as f32, w as f32, h as f32)
}

fn hex(c: &str) -> gdk::RGBA {
    gdk::RGBA::parse(c).unwrap_or(gdk::RGBA::BLACK)
}

// -- the style sheet of the views ---------------------------------------------------------------------

thread_local! {
    static CSS: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
}

fn css_text(app: &App) -> String {
    let t = app.theme();
    format!(
        "scrolledwindow.pdf-bg, scrolledwindow.pdf-bg > viewport {{ background-color: {bg}; }}\n\
         entry.pdf-entry {{ background-color: #FFFFFF; color: #000000; caret-color: #000000; border: 1px solid #000000;\n\
                            border-radius: 0; min-height: 0; min-width: 0; padding: 0 2px; box-shadow: none; }}\n\
         entry.pdf-entry > text {{ background-color: #FFFFFF; color: #000000; }}\n\
         entry.pdf-entry > text > selection {{ background-color: #A5C8FF; color: #000000; }}\n",
        bg = t.c("bg")
    )
}

/// The module's style rules, made once and remade on every theme switch.
fn ensure_css(app: &AppRc) {
    CSS.with(|c| {
        if c.get().is_some() {
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css_text(app));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
        }
        let _ = c.set(provider.clone());
        let me = app.weak();
        app.theme().on_change(move || {
            if let Some(app) = me.upgrade() {
                provider.load_from_string(&css_text(&app));
            }
        });
    });
}

// -- a widget that draws whatever its owner snapshots into it --------------------------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub size: Cell<(i32, i32)>,
        #[allow(clippy::type_complexity)]
        pub draw: RefCell<Option<Box<dyn Fn(&gtk::Snapshot)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "RewritePdfCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {}

    impl WidgetImpl for Canvas {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (w, h) = self.size.get();
            let v = if orientation == gtk::Orientation::Horizontal { w } else { h };
            (v, v, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(f) = self.draw.borrow().as_ref() {
                f(snapshot);
            }
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    fn new() -> Canvas {
        glib::Object::new()
    }

    fn set_size(&self, w: i32, h: i32) {
        if self.imp().size.get() != (w, h) {
            self.imp().size.set((w, h));
            self.queue_resize();
        }
    }

    fn set_draw(&self, f: impl Fn(&gtk::Snapshot) + 'static) {
        self.imp().draw.replace(Some(Box::new(f)));
    }
}

// -- PdfView: a PDF shown as a column of pages ------------------------------------------------------------

/// A page rectangle on the canvas: x, y, width, height.
type PageBox = (i32, i32, i32, i32);

struct Pic {
    version: u64,
    width: i32,
    tex: gdk::Texture,
}

#[derive(Default)]
struct ViewState {
    data: Option<Arc<[u8]>>,
    version: u64,
    /// Page sizes in points, as shown (after their turn).
    sizes: Vec<(f64, f64)>,
    zoom: f64,
    boxes: Vec<PageBox>,
    images: HashMap<usize, Pic>,
    /// Lay the pages out again when the next sizes arrive (the zoom was reset).
    relayout: bool,
    /// Where to scroll (a fraction of the height) once the new height is in place.
    keep_frac: Option<f64>,
}

type Rendered = (u64, Vec<(f64, f64)>, Vec<(usize, i32, rewrite_pdfium::Bitmap)>);

struct ViewInner {
    app: Weak<App>,
    id: u64,
    scroll: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    canvas: Canvas,
    /// Every PDFium call of this view happens on this one thread.
    lane: Lane,
    cache: Arc<Mutex<Cached>>,
    st: RefCell<ViewState>,
    after_layout: RefCell<Option<Rc<dyn Fn()>>>,
    overlay: RefCell<Option<Rc<dyn Fn(&gtk::Snapshot)>>>,
    report: RefCell<Option<Rc<dyn Fn(&str)>>>,
}

/// A PDF shown as a column of pages. Pages are drawn as they scroll into view; the old picture of a
/// page stays up (scaled) until the sharp one arrives.
#[derive(Clone)]
pub struct PdfView {
    v: Rc<ViewInner>,
}

thread_local! {
    static NEXT_VIEW: Cell<u64> = const { Cell::new(1) };
}

impl PdfView {
    pub fn new(app: &AppRc) -> PdfView {
        ensure_css(app);
        let id = NEXT_VIEW.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        let scroll = gtk::ScrolledWindow::new();
        scroll.add_css_class("pdf-bg");
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);
        let fixed = gtk::Fixed::new();
        let canvas = Canvas::new();
        canvas.set_size(1, 1);
        fixed.put(&canvas, 0.0, 0.0);
        let vp = gtk::Viewport::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        vp.set_scroll_to_focus(false);
        vp.set_child(Some(&fixed));
        scroll.set_child(Some(&vp));
        let v = Rc::new(ViewInner {
            app: app.weak(),
            id,
            scroll: scroll.clone(),
            fixed,
            canvas: canvas.clone(),
            lane: Lane::new("pdf"),
            cache: Arc::new(Mutex::new(Cached::default())),
            st: RefCell::new(ViewState { zoom: 1.0, ..Default::default() }),
            after_layout: RefCell::new(None),
            overlay: RefCell::new(None),
            report: RefCell::new(None),
        });
        let view = PdfView { v };
        let w = Rc::downgrade(&view.v);
        canvas.set_draw(move |s| {
            if let Some(v) = w.upgrade() {
                PdfView { v }.draw(s);
            }
        });
        // a new size lays the pages out again (after a moment, as a drag of the sash sends many)
        for adj in [scroll.hadjustment(), scroll.vadjustment()] {
            let w = Rc::downgrade(&view.v);
            adj.connect_page_size_notify(move |_| {
                let Some(v) = w.upgrade() else { return };
                let w2 = Rc::downgrade(&v);
                workers::later(&format!("pdf-layout-{}", v.id), 120, move || {
                    if let Some(v) = w2.upgrade() {
                        PdfView { v }.layout(true);
                    }
                });
            });
        }
        let vadj = scroll.vadjustment();
        let w = Rc::downgrade(&view.v);
        vadj.connect_value_changed(move |_| {
            if let Some(v) = w.upgrade() {
                PdfView { v }.request();
            }
        });
        let w = Rc::downgrade(&view.v);
        vadj.connect_upper_notify(move |adj| {
            let Some(v) = w.upgrade() else { return };
            let frac = v.st.borrow_mut().keep_frac.take();
            if let Some(f) = frac {
                adj.set_value(f * adj.upper());
            }
        });
        // Ctrl + wheel zooms
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(&view.v);
        wheel.connect_scroll(move |c, _, dy| {
            if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            if let Some(v) = w.upgrade() {
                let view = PdfView { v };
                if dy < 0.0 {
                    view.set_zoom(view.zoom() * 1.1);
                } else if dy > 0.0 {
                    view.set_zoom(view.zoom() / 1.1);
                }
            }
            glib::Propagation::Stop
        });
        scroll.add_controller(wheel);
        let w = Rc::downgrade(&view.v);
        app.theme().on_change(move || {
            if let Some(v) = w.upgrade() {
                v.canvas.queue_draw();
            }
        });
        view
    }

    pub fn widget(&self) -> gtk::Widget {
        self.v.scroll.clone().upcast()
    }

    fn canvas(&self) -> &Canvas {
        &self.v.canvas
    }

    fn fixed(&self) -> &gtk::Fixed {
        &self.v.fixed
    }

    /// Run `f` after every layout (zoom, resize, new document), to put marks back in place.
    fn on_after_layout(&self, f: impl Fn() + 'static) {
        self.v.after_layout.replace(Some(Rc::new(f)));
    }

    /// Draw over the pages (selection, a box being dragged ...).
    fn on_overlay(&self, f: impl Fn(&gtk::Snapshot) + 'static) {
        self.v.overlay.replace(Some(Rc::new(f)));
    }

    /// Where messages about the view go (an error drawing the pages).
    pub fn on_report(&self, f: impl Fn(&str) + 'static) {
        self.v.report.replace(Some(Rc::new(f)));
    }

    fn report(&self, msg: &str) {
        let f = self.v.report.borrow().clone();
        match f {
            Some(f) => f(msg),
            None => {
                if let Some(app) = self.v.app.upgrade() {
                    app.flash_msg(&msg.replace('\n', " "), 9000);
                }
            }
        }
    }

    pub fn data(&self) -> Option<Arc<[u8]>> {
        self.v.st.borrow().data.clone()
    }

    pub fn has_data(&self) -> bool {
        self.v.st.borrow().data.is_some()
    }

    fn version(&self) -> u64 {
        self.v.st.borrow().version
    }

    pub fn zoom(&self) -> f64 {
        self.v.st.borrow().zoom
    }

    /// The zoom for the next document, without laying out the current one.
    fn reset_zoom(&self) {
        let mut st = self.v.st.borrow_mut();
        st.zoom = 1.0;
        st.relayout = true;
    }

    pub fn set_zoom(&self, z: f64) {
        let z = z.clamp(0.4, 4.0);
        let changed = {
            let mut st = self.v.st.borrow_mut();
            let changed = (z - st.zoom).abs() > 1e-3;
            if changed {
                st.zoom = z;
            }
            changed
        };
        if changed {
            self.layout(true);
        }
    }

    /// Display a new PDF. The old pages stay up until the new ones are drawn.
    pub fn show(&self, data: Arc<[u8]>) {
        {
            let mut st = self.v.st.borrow_mut();
            st.data = Some(data);
            st.version += 1;
        }
        self.submit(vec![]);
    }

    pub fn clear(&self) {
        {
            let mut st = self.v.st.borrow_mut();
            st.data = None;
            st.sizes.clear();
            st.boxes.clear();
            st.version += 1;
            st.images.clear();
            st.keep_frac = None;
        }
        self.v.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.v.canvas.set_size(1, 1);
        self.v.canvas.queue_draw();
    }

    pub fn boxes(&self) -> Vec<PageBox> {
        self.v.st.borrow().boxes.clone()
    }

    /// A page's size in points as shown.
    fn size(&self, p: usize) -> (f64, f64) {
        self.v.st.borrow().sizes.get(p).copied().unwrap_or((612.0, 792.0))
    }

    /// The vertical scroll position and the height of the view.
    fn visible(&self) -> (f64, f64) {
        let a = self.v.scroll.vadjustment();
        (a.value(), a.page_size())
    }

    // -- worker -----------------------------------------------------------------------------------

    fn submit(&self, pages: Vec<usize>) {
        let (version, data, jobs) = {
            let st = self.v.st.borrow();
            let factor = (self.v.canvas.scale_factor() as f64).max(1.0);
            let jobs: Vec<(usize, f64, i32)> = pages
                .iter()
                .filter(|&&p| p < st.boxes.len() && p < st.sizes.len())
                .map(|&p| {
                    let w = st.boxes[p].2;
                    let dev = factor.min(MAX_RENDER_WIDTH / w.max(1) as f64).max(1.0);
                    (p, w as f64 / st.sizes[p].0.max(1e-6) * dev, w)
                })
                .collect();
            (st.version, st.data.clone(), jobs)
        };
        let Some(data) = data else { return };
        let cache = self.v.cache.clone();
        let w = Rc::downgrade(&self.v);
        self.v.lane.submit(
            move || -> Result<Rendered, String> {
                let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
                let doc = c.get(version, || data.clone()).map_err(|e| e.to_string())?;
                let sizes = doc.page_sizes().map_err(|e| e.to_string())?;
                let mut out = vec![];
                for (p, scale, width) in jobs {
                    if p >= sizes.len() {
                        continue;
                    }
                    out.push((p, width, doc.render_page(p, scale).map_err(|e| e.to_string())?));
                }
                Ok((version, sizes, out))
            },
            move |res| {
                if let Some(v) = w.upgrade() {
                    PdfView { v }.done(res);
                }
            },
        );
    }

    fn done(&self, res: Result<Result<Rendered, String>, String>) {
        let (version, sizes, pages) = match res {
            Ok(Ok(r)) => r,
            Ok(Err(e)) | Err(e) => {
                self.report(&format!("The PDF could not be shown:\n{e}"));
                return;
            }
        };
        let relayout = {
            let mut st = self.v.st.borrow_mut();
            if version != st.version {
                return;
            }
            let relayout = sizes != st.sizes || st.relayout;
            if relayout {
                st.sizes = sizes;
                st.relayout = false;
            }
            relayout
        };
        if relayout {
            self.layout(false);
        }
        {
            let mut st = self.v.st.borrow_mut();
            for (p, width, bm) in pages {
                if p >= st.boxes.len() || width != st.boxes[p].2 {
                    continue; // the window was resized meanwhile
                }
                let bytes = glib::Bytes::from_owned(bm.pixels);
                let tex = gdk::MemoryTexture::new(bm.width as i32, bm.height as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, bm.width * 4);
                st.images.insert(p, Pic { version, width, tex: tex.upcast() });
            }
        }
        self.v.canvas.queue_draw();
        self.request();
    }

    // -- layout -----------------------------------------------------------------------------------

    fn layout(&self, request: bool) {
        let width = self.v.scroll.width().max(1);
        let changed = {
            let mut st = self.v.st.borrow_mut();
            if st.sizes.is_empty() {
                return;
            }
            let gap = 8;
            let room = 120.max(width - 2 * gap) as f64;
            let max_w = st.sizes.iter().map(|s| s.0).fold(1e-6, f64::max);
            let scale = (room * st.zoom).min(3600.0) / max_w;
            let total_w = width.max((max_w * scale) as i32 + 2 * gap);
            let mut boxes = Vec::with_capacity(st.sizes.len());
            let mut y = gap;
            for &(w, h) in &st.sizes {
                let (pw, ph) = (1.max((w * scale).round() as i32), 1.max((h * scale).round() as i32));
                boxes.push(((total_w - pw).div_euclid(2), y, pw, ph));
                y += ph + gap;
            }
            let changed = boxes != st.boxes;
            if changed {
                let n = boxes.len();
                st.boxes = boxes;
                st.images.retain(|&p, _| p < n);
                let adj = self.v.scroll.vadjustment();
                let (_, old_h) = self.v.canvas.imp().size.get();
                if old_h != y && adj.upper() > 0.0 {
                    st.keep_frac = Some(adj.value() / adj.upper());
                }
                drop(st);
                self.v.canvas.set_size(total_w, y);
            }
            changed
        };
        if changed {
            let hook = self.v.after_layout.borrow().clone();
            if let Some(f) = hook {
                f();
            }
            self.v.canvas.queue_draw();
        }
        if request {
            self.request();
        }
    }

    /// Ask the worker for the visible pages that are missing or out of date.
    fn request(&self) {
        let need = {
            let mut st = self.v.st.borrow_mut();
            if st.boxes.is_empty() || st.data.is_none() {
                return;
            }
            let (y0, h) = self.visible();
            let y1 = y0 + h;
            let seen: Vec<usize> = st
                .boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| (b.1 + b.3) as f64 >= y0 - b.3 as f64 && b.1 as f64 <= y1 + b.3 as f64)
                .map(|(p, _)| p)
                .collect();
            let (Some(&first), Some(&last)) = (seen.first(), seen.last()) else { return };
            let boxes = st.boxes.clone();
            // far away (saves memory) or drawn for another zoom
            st.images.retain(|&p, pic| {
                let far = (p as i64) < first as i64 - 4 || p > last + 4;
                !(far || (!seen.contains(&p) && pic.width != boxes[p].2))
            });
            let version = st.version;
            seen.into_iter()
                .filter(|p| st.images.get(p).is_none_or(|pic| pic.version != version || pic.width != boxes[*p].2))
                .collect::<Vec<_>>()
        };
        if !need.is_empty() {
            self.submit(need);
        }
    }

    fn draw(&self, s: &gtk::Snapshot) {
        let border = self.v.app.upgrade().map_or(hex("#E3E0D8"), |a| a.theme().rgba("border"));
        let white = gdk::RGBA::WHITE;
        {
            let st = self.v.st.borrow();
            let (top, h) = self.visible();
            for (p, &(x, y, w, ph)) in st.boxes.iter().enumerate() {
                if ((y + ph) as f64) < top - 20.0 || y as f64 > top + h + 20.0 {
                    continue;
                }
                let (x, y, w, ph) = (x as f64, y as f64, w as f64, ph as f64);
                s.append_color(&border, &rect(x - 1.0, y - 1.0, w + 2.0, ph + 2.0));
                s.append_color(&white, &rect(x, y, w, ph));
                if let Some(pic) = st.images.get(&p) {
                    s.append_scaled_texture(&pic.tex, gsk::ScalingFilter::Linear, &rect(x, y, w, ph));
                }
            }
        }
        let hook = self.v.overlay.borrow().clone();
        if let Some(f) = hook {
            f(s);
        }
    }
}

// -- PdfEditor ----------------------------------------------------------------------------------------------

#[derive(Clone)]
struct EditInfo {
    bx: (f64, f64, f64, f64),
    font: String,
    bold: bool,
    italic: bool,
}

struct EntryState {
    widget: gtk::Entry,
    page: usize,
    x: f64,
    y: f64,
    size: f64,
    edit: Option<EditInfo>,
    before: String,
}

#[derive(Clone)]
enum Drag {
    Select { page: usize, from: usize },
    Ink { page: usize, points: Vec<(f64, f64)> },
    Area { page: usize, start: (f64, f64), end: (f64, f64), moved: bool },
    Click { page: usize, at: (f64, f64) },
}

type PageInfo = (f64, f64, f64, f64, i64);
type Then = Box<dyn FnOnce(Arc<[u8]>, Vec<u8>) -> Result<Vec<u8>, String> + Send>;

struct EdState {
    tool: String,
    /// (left, bottom, right, top, turn) of every page
    pages: Vec<PageInfo>,
    undo: Vec<Arc<[u8]>>,
    redo: Vec<Arc<[u8]>>,
    path: Option<PathBuf>,
    busy: bool,
    /// (page, first char, last char)
    sel: Option<(usize, usize, usize)>,
    /// The selection is drawn (after the pointer moved).
    painted: bool,
    drag: Option<Drag>,
    entry: Option<EntryState>,
    current: usize,
    chars: HashMap<usize, Rc<Vec<Char>>>,
}

struct Ed {
    app: Weak<App>,
    view: PdfView,
    work: Lane,
    /// A PDFium document of the shown version, for text lookups on the UI thread.
    edoc: RefCell<Cached>,
    st: RefCell<EdState>,
}

pub struct PdfEditor {
    e: Rc<Ed>,
}

impl PdfEditor {
    pub fn new(app: &AppRc) -> PdfEditor {
        let view = PdfView::new(app);
        let e = Rc::new(Ed {
            app: app.weak(),
            view,
            work: Lane::new("pdf-change"),
            edoc: RefCell::new(Cached::default()),
            st: RefCell::new(EdState {
                tool: "select".into(),
                pages: vec![],
                undo: vec![],
                redo: vec![],
                path: None,
                busy: false,
                sel: None,
                painted: false,
                drag: None,
                entry: None,
                current: 0,
                chars: HashMap::new(),
            }),
        });
        Ed::connect(&e);
        PdfEditor { e }
    }
    pub fn widget(&self) -> gtk::Widget {
        self.e.view.widget()
    }
    pub fn load(&self, data: Vec<u8>, path: Option<PathBuf>) {
        self.e.drop_entry();
        {
            let mut st = self.e.st.borrow_mut();
            st.path = path;
            st.undo.clear();
            st.redo.clear();
            st.drag = None;
        }
        self.e.view.reset_zoom();
        self.e.set(Arc::from(data));
    }
    /// Save (asking for a name when needed or `as_`), then `then(saved)`.
    pub fn save(&self, as_: bool, then: Box<dyn FnOnce(bool)>) {
        Ed::save(&self.e, as_, then);
    }
    pub fn undo(&self) {
        self.e.undo();
    }
    pub fn redo(&self) {
        self.e.redo();
    }
    pub fn data(&self) -> Vec<u8> {
        self.e.view.data().map(|d| d.to_vec()).unwrap_or_default()
    }
    pub fn can_undo(&self) -> bool {
        !self.e.st.borrow().undo.is_empty()
    }
    pub fn page_count(&self) -> usize {
        self.e.st.borrow().pages.len()
    }
    pub fn set_tool(&self, name: &str) {
        self.e.set_tool(name);
    }
    pub fn zoom(&self) -> f64 {
        self.e.view.zoom()
    }
    pub fn set_zoom(&self, z: f64) {
        self.e.view.set_zoom(z);
    }
    pub fn rotate(&self, angle: i64) {
        self.e.rotate(angle, None);
    }
    pub fn move_page(&self, d: i64) {
        self.e.move_page(d, None);
    }
    pub fn insert_blank(&self) {
        self.e.insert_blank(None);
    }
    pub fn merge(&self) {
        Ed::merge(&self.e, None);
    }
    pub fn delete_page(&self) {
        Ed::delete_page(&self.e, None);
    }
}

impl Ed {
    fn app(&self) -> Option<AppRc> {
        self.app.upgrade()
    }

    fn connect(e: &Rc<Ed>) {
        let canvas = e.view.canvas().clone();
        canvas.set_focusable(true);
        canvas.set_cursor_from_name(Some("text"));
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let w = Rc::downgrade(e);
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(e) = w.upgrade() {
                Ed::press(&e, x, y);
            }
        });
        let w = Rc::downgrade(e);
        drag.connect_drag_update(move |g, dx, dy| {
            if let (Some(e), Some((sx, sy))) = (w.upgrade(), g.start_point()) {
                e.motion(sx + dx, sy + dy);
            }
        });
        let w = Rc::downgrade(e);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(e) = w.upgrade() {
                Ed::release(&e);
            }
        });
        canvas.add_controller(drag);
        let menu = gtk::GestureClick::new();
        menu.set_button(gdk::BUTTON_SECONDARY);
        let w = Rc::downgrade(e);
        menu.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(e) = w.upgrade() {
                Ed::menu(&e, x, y);
            }
        });
        canvas.add_controller(menu);
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(e);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(e) = w.upgrade() else { return glib::Propagation::Proceed };
            let primary = if cfg!(target_os = "macos") { gdk::ModifierType::META_MASK } else { gdk::ModifierType::CONTROL_MASK };
            if state.contains(primary) && key.to_lower() == gdk::Key::c {
                e.copy();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Escape {
                e.clear_selection();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        canvas.add_controller(keys);
        let w = Rc::downgrade(e);
        e.view.on_after_layout(move || {
            if let Some(e) = w.upgrade() {
                e.after_layout();
            }
        });
        let w = Rc::downgrade(e);
        e.view.on_overlay(move |s| {
            if let Some(e) = w.upgrade() {
                e.overlay(s);
            }
        });
        let w = Rc::downgrade(e);
        e.view.on_report(move |msg| {
            if let Some(e) = w.upgrade() {
                e.report(msg);
            }
        });
    }

    // -- the document ------------------------------------------------------------------------------

    fn set(&self, data: Arc<[u8]>) {
        let pages = pdfedit::pages(&data).unwrap_or_default();
        {
            let mut st = self.st.borrow_mut();
            st.pages = pages;
            st.chars.clear();
        }
        self.clear_selection();
        self.view.show(data);
    }

    fn report(&self, msg: &str) {
        if let Some(app) = self.app() {
            app.flash_msg(&msg.replace('\n', " "), 9000);
        }
    }

    /// Make changes (in the background); the result becomes the shown version.
    fn change(self: &Rc<Self>, ops: Vec<Op>, what: &str, then: Option<Then>) {
        let Some(data) = self.view.data() else { return };
        {
            let mut st = self.st.borrow_mut();
            if st.busy {
                return;
            }
            st.busy = true;
        }
        let Some(app) = self.app() else { return };
        app.flash_msg(&format!("{what}…"), 60000);
        let what = what.to_string();
        let before = data.clone();
        let w = Rc::downgrade(self);
        self.work.submit(
            move || -> Result<Vec<u8>, String> {
                let out = pdfedit::apply(&data, &ops)?;
                match then {
                    Some(f) => f(data, out),
                    None => Ok(out),
                }
            },
            move |res| {
                let Some(e) = w.upgrade() else { return };
                e.st.borrow_mut().busy = false;
                let Some(app) = e.app() else { return };
                let out = match res {
                    Ok(Ok(out)) => out,
                    Ok(Err(err)) | Err(err) => {
                        app.error("PDF", &format!("{what} did not work.\n\n{err}"));
                        app.flash_msg("", 1);
                        return;
                    }
                };
                {
                    let mut st = e.st.borrow_mut();
                    st.undo.push(before);
                    while st.undo.iter().map(|d| d.len()).sum::<usize>() > MAX_UNDO_BYTES && st.undo.len() > 1 {
                        st.undo.remove(0);
                    }
                    st.redo.clear();
                }
                e.set(Arc::from(out));
                app.pdf_changed(true);
                app.flash_msg(&format!("{what} — {MOD}+Z undoes it."), 5000);
            },
        );
    }

    fn undo(&self) {
        let prev = {
            let mut st = self.st.borrow_mut();
            if st.busy || st.undo.is_empty() {
                return;
            }
            let Some(cur) = self.view.data() else { return };
            st.redo.push(cur);
            st.undo.pop()
        };
        if let Some(d) = prev {
            self.set(d);
            if let Some(app) = self.app() {
                app.pdf_changed(true);
            }
        }
    }

    fn redo(&self) {
        let next = {
            let mut st = self.st.borrow_mut();
            if st.busy || st.redo.is_empty() {
                return;
            }
            let Some(cur) = self.view.data() else { return };
            st.undo.push(cur);
            st.redo.pop()
        };
        if let Some(d) = next {
            self.set(d);
            if let Some(app) = self.app() {
                app.pdf_changed(true);
            }
        }
    }

    fn save(e: &Rc<Ed>, as_: bool, then: Box<dyn FnOnce(bool)>) {
        let Some(app) = e.app() else { return then(false) };
        let path = e.st.borrow().path.clone();
        if !as_ {
            if let Some(p) = &path {
                then(e.write(p));
                return;
            }
        }
        let base = path.as_ref().and_then(|p| p.file_stem()).map_or("document".to_string(), |s| s.to_string_lossy().into_owned());
        let dialog = gtk::FileDialog::builder().title("Save PDF as").modal(true).initial_name(format!("{base}.pdf")).build();
        dialog.set_filters(Some(&file_filters(&[("PDF", &["*.pdf"])])));
        let dir = path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| app.settings().last_dir.clone().map(PathBuf::from));
        if let Some(d) = dir {
            if d.is_dir() {
                dialog.set_initial_folder(Some(&gio::File::for_path(d)));
            }
        }
        let e = e.clone();
        let win = app.win.clone();
        glib::spawn_future_local(async move {
            let Ok(f) = dialog.save_future(Some(&win)).await else { return then(false) };
            let Some(mut p) = f.path() else { return then(false) };
            if p.extension().is_none() {
                p.set_extension("pdf");
            }
            then(e.write(&p));
        });
    }

    fn write(&self, path: &Path) -> bool {
        let Some(app) = self.app() else { return false };
        let data = self.view.data().unwrap_or_else(|| Arc::from(Vec::new()));
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".rewrite-tmp");
        let tmp = PathBuf::from(tmp);
        let res = std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, path));
        if let Err(err) = res {
            let _ = std::fs::remove_file(&tmp);
            app.error("Could not save", &err.to_string());
            return false;
        }
        self.st.borrow_mut().path = Some(path.to_path_buf());
        {
            let mut st = app.st.borrow_mut();
            st.doc.path = Some(path.to_path_buf());
            st.dirty = false;
        }
        app.update_title();
        app.flash_msg(&format!("Saved {}", path.file_name().unwrap_or_default().to_string_lossy()), 5000);
        true
    }

    // -- where things are --------------------------------------------------------------------------

    fn page_size(&self, p: usize) -> (f64, f64, i64) {
        let b = self.st.borrow().pages.get(p).copied().unwrap_or((0.0, 0.0, 612.0, 792.0, 0));
        (b.2 - b.0, b.3 - b.1, b.4.rem_euclid(360))
    }

    /// Canvas point -> (page, x, y) in the page's stored, unturned coordinates; None off the pages.
    /// With `clamp` = a page, a point off that page is moved to its edge.
    fn to_page(&self, cx: f64, cy: f64, clamp: Option<usize>) -> Option<(usize, f64, f64)> {
        for (p, (bx, by, bw, bh)) in self.view.boxes().into_iter().enumerate() {
            let (bx, by, bw, bh) = (bx as f64, by as f64, bw as f64, bh as f64);
            if clamp.is_some_and(|c| c != p) {
                continue;
            }
            if clamp.is_none() && !(bx <= cx && cx <= bx + bw && by <= cy && cy <= by + bh) {
                continue;
            }
            let s = bw / self.view.size(p).0.max(1e-6);
            let (u, v) = ((cx.clamp(bx, bx + bw) - bx) / s, (cy.clamp(by, by + bh) - by) / s);
            let (w, h, r) = self.page_size(p);
            return Some(match r {
                90 => (p, v, h - u),
                180 => (p, w - u, h - v),
                270 => (p, w - v, u),
                _ => (p, u, v),
            });
        }
        None
    }

    fn to_canvas(&self, p: usize, x: f64, y: f64) -> (f64, f64) {
        let boxes = self.view.boxes();
        let Some(&(bx, by, bw, _)) = boxes.get(p) else { return (0.0, 0.0) };
        let s = bw as f64 / self.view.size(p).0.max(1e-6);
        let (w, h, r) = self.page_size(p);
        let (u, v) = match r {
            90 => (h - y, x),
            180 => (w - x, h - y),
            270 => (y, w - x),
            _ => (x, y),
        };
        (bx as f64 + u * s, by as f64 + v * s)
    }

    fn scale(&self, p: usize) -> f64 {
        self.view.boxes().get(p).map_or(1.0, |b| b.2 as f64 / self.view.size(p).0.max(1e-6))
    }

    /// The page in the middle of the view (or the last one clicked).
    fn view_page(&self) -> usize {
        let (top, h) = self.view.visible();
        let mid = top + h / 2.0;
        let boxes = self.view.boxes();
        for (p, &(_, y, _, bh)) in boxes.iter().enumerate() {
            if y as f64 <= mid && mid <= (y + bh + 8) as f64 {
                return p;
            }
        }
        self.st.borrow().current.min(boxes.len().saturating_sub(1))
    }

    // -- text on the page ----------------------------------------------------------------------------

    /// The page's characters, in stored page coordinates.
    fn chars(&self, p: usize) -> Rc<Vec<Char>> {
        if let Some(c) = self.st.borrow().chars.get(&p) {
            return c.clone();
        }
        let Some(data) = self.view.data() else { return Rc::new(vec![]) };
        let version = self.view.version();
        let (left, top) = self.st.borrow().pages.get(p).map(|b| (b.0, b.3)).unwrap_or((0.0, self.view.size(p).1));
        let out = {
            let mut c = self.edoc.borrow_mut();
            c.get(version, || data).and_then(|d| d.chars(p, left, top)).unwrap_or_default()
        };
        let out = Rc::new(out);
        self.st.borrow_mut().chars.insert(p, out.clone());
        out
    }

    /// The index of the character nearest to a point (None when the page has no text).
    fn nearest(&self, p: usize, x: f64, y: f64) -> Option<usize> {
        let mut best = None;
        let mut bd = 1e18;
        for (i, c) in self.chars(p).iter().enumerate() {
            if c.text.trim().is_empty() || c.x1 <= c.x0 {
                continue;
            }
            let dx = if c.x0 <= x && x <= c.x1 { 0.0 } else { (x - c.x0).abs().min((x - c.x1).abs()) };
            let dy = if c.top <= y && y <= c.bottom { 0.0 } else { (y - c.top).abs().min((y - c.bottom).abs()) };
            let d = dx * dx + 4.0 * dy * dy;
            if d < bd {
                best = Some(i);
                bd = d;
            }
        }
        best
    }

    /// The characters a..b grouped in lines: (x0, top, x1, bottom, text).
    fn lines(&self, p: usize, a: usize, b: usize) -> Vec<(f64, f64, f64, f64, String)> {
        let cs = self.chars(p);
        let mut out: Vec<(f64, f64, f64, f64, String)> = vec![];
        let mut cur: Option<usize> = None;
        let end = (b + 1).min(cs.len());
        for c in cs.get(a.min(end)..end).unwrap_or(&[]) {
            if c.x1 <= c.x0 && c.text.trim().is_empty() {
                if let Some(k) = cur {
                    if " \t".contains(c.text.as_str()) {
                        out[k].4.push(' ');
                    }
                }
                continue;
            }
            let same = cur.is_some_and(|k| {
                let l = &out[k];
                ((c.top + c.bottom) / 2.0 - (l.1 + l.3) / 2.0).abs() < 2.0f64.max(0.5 * (c.bottom - c.top)) && c.x0 >= l.0 - 2.0
            });
            if !same {
                out.push((c.x0, c.top, c.x1, c.bottom, String::new()));
                cur = Some(out.len() - 1);
            }
            let l = &mut out[cur.unwrap()];
            l.0 = l.0.min(c.x0);
            l.1 = l.1.min(c.top);
            l.2 = l.2.max(c.x1);
            l.3 = l.3.max(c.bottom);
            if !"\r\n".contains(c.text.as_str()) {
                l.4.push_str(&c.text);
            }
        }
        out.into_iter().map(|(x0, t, x1, b, s)| (x0, t, x1, b, s.trim().to_string())).collect()
    }

    /// The line of text under a point: (first char, last char) or None.
    fn line_at(&self, p: usize, x: f64, y: f64) -> Option<(usize, usize)> {
        let i = self.nearest(p, x, y)?;
        let cs = self.chars(p);
        let c = &cs[i];
        if !(c.x0 - 3.0 <= x && x <= c.x1 + 3.0 * c.size && c.top - 3.0 <= y && y <= c.bottom + 3.0) {
            return None;
        }
        let mid = (c.top + c.bottom) / 2.0;
        let on_line = |d: &Char| if d.x1 > d.x0 { ((d.top + d.bottom) / 2.0 - mid).abs() < 0.45 * d.size.max(1.0) } else { true };
        let nl = |d: &Char| "\r\n".contains(d.text.as_str());
        let mut a = i;
        while a > 0 && on_line(&cs[a - 1]) && !nl(&cs[a - 1]) && cs[a - 1].x1 <= cs[a].x0 + 2.0 * c.size {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < cs.len() && on_line(&cs[b + 1]) && !nl(&cs[b + 1]) && cs[b + 1].x0 >= cs[b].x0 - 2.0 {
            b += 1;
        }
        Some((a, b))
    }

    // -- the tools -----------------------------------------------------------------------------------

    fn set_tool(self: &Rc<Self>, name: &str) {
        self.commit_entry();
        self.st.borrow_mut().tool = name.to_string();
        self.clear_selection();
        let cursor = match name {
            "select" | "edit" | "add" | "highlight" | "underline" | "strike" => "text",
            "note" | "image" => "pointer",
            _ => "crosshair",
        };
        self.view.canvas().set_cursor_from_name(Some(cursor));
    }

    fn tool(&self) -> String {
        self.st.borrow().tool.clone()
    }

    fn press(e: &Rc<Ed>, cx: f64, cy: f64) {
        e.view.canvas().grab_focus();
        e.commit_entry();
        e.st.borrow_mut().drag = None;
        let Some((p, x, y)) = e.to_page(cx, cy, None) else {
            e.clear_selection();
            return;
        };
        e.st.borrow_mut().current = p;
        let tool = e.tool();
        let drag = match tool.as_str() {
            "select" | "highlight" | "underline" | "strike" => {
                let i = e.nearest(p, x, y);
                e.clear_selection();
                match i {
                    Some(i) => {
                        e.st.borrow_mut().sel = Some((p, i, i));
                        Some(Drag::Select { page: p, from: i })
                    }
                    None => None,
                }
            }
            "draw" => Some(Drag::Ink { page: p, points: vec![(x, y)] }),
            "whiteout" | "box" => Some(Drag::Area { page: p, start: (x, y), end: (x, y), moved: false }),
            _ => Some(Drag::Click { page: p, at: (x, y) }),
        };
        e.st.borrow_mut().drag = drag;
    }

    fn motion(&self, cx: f64, cy: f64) {
        let Some(d) = self.st.borrow().drag.clone() else { return };
        let page = match &d {
            Drag::Select { page, .. } | Drag::Ink { page, .. } | Drag::Area { page, .. } | Drag::Click { page, .. } => *page,
        };
        let Some((p, x, y)) = self.to_page(cx, cy, Some(page)) else { return };
        match d {
            Drag::Select { from, .. } => {
                if let Some(i) = self.nearest(p, x, y) {
                    let mut st = self.st.borrow_mut();
                    st.sel = Some((p, from.min(i), from.max(i)));
                    st.painted = true;
                }
            }
            Drag::Ink { .. } => {
                if let Some(Drag::Ink { points, .. }) = self.st.borrow_mut().drag.as_mut() {
                    points.push((x, y));
                }
            }
            Drag::Area { .. } => {
                if let Some(Drag::Area { end, moved, .. }) = self.st.borrow_mut().drag.as_mut() {
                    *end = (x, y);
                    *moved = true;
                }
            }
            Drag::Click { .. } => return,
        }
        self.view.canvas().queue_draw();
    }

    fn release(e: &Rc<Ed>) {
        let Some(d) = e.st.borrow_mut().drag.take() else { return };
        e.view.canvas().queue_draw();
        let tool = e.tool();
        match d {
            Drag::Select { .. } => {
                if mark_color(&tool).is_some() && e.st.borrow().sel.is_some() {
                    e.mark(&tool);
                }
            }
            Drag::Ink { page, points } => {
                if points.len() > 1 {
                    e.change(vec![Op::Ink { page, paths: vec![points], color: "#1971C2".into(), width: 2.0 }], "Drawing", None);
                }
            }
            Drag::Area { page, start: (x0, y0), end: (x1, y1), .. } => {
                if (x1 - x0).abs() < 3.0 || (y1 - y0).abs() < 3.0 {
                    return;
                }
                let (bx0, top, bx1, bottom) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
                if tool == "whiteout" {
                    e.change(
                        vec![Op::Rect { page, x0: bx0, top, x1: bx1, bottom, fill: Some("#FFFFFF".into()), stroke: None, width: 1.0 }],
                        "White-out",
                        None,
                    );
                } else {
                    e.change(
                        vec![Op::Rect { page, x0: bx0, top, x1: bx1, bottom, fill: None, stroke: Some("#C92A2A".into()), width: 1.5 }],
                        "Rectangle",
                        None,
                    );
                }
            }
            Drag::Click { page: p, at: (x, y) } => match tool.as_str() {
                "note" => {
                    let Some(app) = e.app() else { return };
                    let w = Rc::downgrade(e);
                    note_dialog(&app, move |text| {
                        if let Some(e) = w.upgrade() {
                            e.change(vec![Op::Note { page: p, x, y, text, color: "#FFE14D".into() }], "Note", None);
                        }
                    });
                }
                "image" => Ed::pick_picture(e, p, x, y),
                "add" => e.open_entry(p, x, y, "", 12.0, None),
                "edit" => {
                    let Some((a, b)) = e.line_at(p, x, y) else {
                        e.open_entry(p, x, y, "", 12.0, None);
                        return;
                    };
                    let cs = e.chars(p);
                    let pieces: Vec<&Char> = cs[a..=b].iter().filter(|c| c.x1 > c.x0).collect();
                    let Some(first) = pieces.first() else {
                        e.open_entry(p, x, y, "", 12.0, None);
                        return;
                    };
                    let x0 = pieces.iter().map(|c| c.x0).fold(f64::INFINITY, f64::min);
                    let top = pieces.iter().map(|c| c.top).fold(f64::INFINITY, f64::min);
                    let x1 = pieces.iter().map(|c| c.x1).fold(f64::NEG_INFINITY, f64::max);
                    let bottom = pieces.iter().map(|c| c.bottom).fold(f64::NEG_INFINITY, f64::max);
                    let text: String = cs[a..=b].iter().filter(|c| !"\r\n".contains(c.text.as_str())).map(|c| c.text.as_str()).collect();
                    let mut sizes: Vec<f64> = pieces.iter().map(|c| c.size).collect();
                    sizes.sort_by(|a, b| a.total_cmp(b));
                    let size = sizes.get(sizes.len() / 2).copied().unwrap_or(12.0);
                    let info = EditInfo { bx: (x0, top, x1, bottom), font: first.font.clone(), bold: first.bold, italic: first.italic };
                    e.open_entry(p, x0, top, text.trim(), size, Some(info));
                }
                _ => {}
            },
        }
    }

    fn pick_picture(e: &Rc<Ed>, p: usize, x: f64, y: f64) {
        let Some(app) = e.app() else { return };
        let dialog = gtk::FileDialog::builder().title("Put a picture on the page").modal(true).build();
        dialog.set_filters(Some(&file_filters(PICTURE_TYPES)));
        let w = Rc::downgrade(e);
        let win = app.win.clone();
        glib::spawn_future_local(async move {
            let Ok(f) = dialog.open_future(Some(&win)).await else { return };
            let (Some(path), Some(e)) = (f.path(), w.upgrade()) else { return };
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(err) => {
                    if let Some(app) = e.app() {
                        app.error("Could not open file", &err.to_string());
                    }
                    return;
                }
            };
            let (w, h) = rewrite_core::imaging::natural_size(&data, Some(200.0));
            e.change(vec![Op::Image { page: p, x0: x, top: y, x1: x + w, bottom: y + h, data }], "Picture", None);
        });
    }

    // -- typing on the page ----------------------------------------------------------------------------

    /// A box to type in, on the page; Enter puts the text there, Esc drops it.
    fn open_entry(self: &Rc<Self>, p: usize, x: f64, y: f64, text: &str, size: f64, edit: Option<EditInfo>) {
        let Some(app) = self.app() else { return };
        let s = self.scale(p);
        let px = 6.max((size * s).round() as i32);
        let family = edit.as_ref().and_then(|e| family(&e.font)).unwrap_or_else(|| app.theme().families.sans.clone());
        let (cx, cy) = self.to_canvas(p, x, y);
        let entry = gtk::Entry::new();
        entry.add_css_class("pdf-entry");
        entry.set_text(text);
        entry.set_width_chars(12.max(text.chars().count() as i32 + 4));
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrString::new_family(&family));
        attrs.insert(pango::AttrSize::new_size_absolute(px * pango::SCALE));
        entry.set_attributes(&attrs);
        self.view.fixed().put(&entry, cx, cy);
        let w = Rc::downgrade(self);
        entry.connect_activate(move |_| {
            if let Some(e) = w.upgrade() {
                e.commit_entry();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(e) = w.upgrade() {
                    e.drop_entry();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
        entry.grab_focus();
        entry.set_position(-1);
        self.st.borrow_mut().entry = Some(EntryState { widget: entry, page: p, x, y, size, edit, before: text.to_string() });
    }

    fn drop_entry(&self) {
        let en = self.st.borrow_mut().entry.take();
        if let Some(en) = en {
            // focus leaves the box before it goes, so its input method lets go of the window
            self.view.canvas().grab_focus();
            self.view.fixed().remove(&en.widget);
        }
    }

    fn commit_entry(self: &Rc<Self>) {
        let Some((text, p, ex, ey, size, edit, before)) = self.st.borrow().entry.as_ref().map(|en| {
            (en.widget.text().to_string(), en.page, en.x, en.y, en.size, en.edit.clone(), en.before.clone())
        }) else {
            return;
        };
        self.drop_entry();
        if text.trim() == before.trim() {
            return;
        }
        let Some(edit) = edit else {
            if !text.trim().is_empty() {
                let op = Op::Text {
                    page: p,
                    x: ex,
                    y: ey,
                    text,
                    size,
                    font: None,
                    generic: "sans".into(),
                    bold: false,
                    italic: false,
                    color: "#000000".into(),
                    background: None,
                };
                self.change(vec![op], "Text", None);
            }
            return;
        };
        let (x0, top, x1, bottom) = edit.bx;
        let new = Op::Text {
            page: p,
            x: x0,
            y: top,
            text: text.clone(),
            size,
            font: family(&edit.font),
            generic: generic(&edit.font).into(),
            bold: edit.bold,
            italic: edit.italic,
            color: "#000000".into(),
            background: None,
        };
        let pages = self.st.borrow().pages.clone();
        let has_text = !text.trim().is_empty();
        let cover = Op::Rect { page: p, x0: x0 - 1.0, top, x1: x1 + 1.0, bottom, fill: Some("#FFFFFF".into()), stroke: None, width: 1.0 };
        // The old words must be gone from the page; if they were drawn in a way that cannot be taken out,
        // cover them instead.
        let old_text = before;
        let check: Then = Box::new(move |before: Arc<[u8]>, out: Vec<u8>| {
            if still_there(&out, p, (x0, top, x1, bottom), &old_text, &pages)? {
                let ops = if has_text { vec![cover, new] } else { vec![cover] };
                return pdfedit::apply(&before, &ops);
            }
            if has_text { pdfedit::apply(&out, &[new]) } else { Ok(out) }
        });
        self.change(vec![Op::EraseText { page: p, x0, top, x1, bottom }], "Text changed", Some(check));
    }

    // -- selection and marks ---------------------------------------------------------------------------

    fn clear_selection(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.sel = None;
            st.painted = false;
        }
        self.view.canvas().queue_draw();
    }

    fn after_layout(&self) {
        let en = self.st.borrow().entry.as_ref().map(|en| (en.widget.clone(), en.page, en.x, en.y));
        {
            let mut st = self.st.borrow_mut();
            if st.sel.is_some() {
                st.painted = true;
            }
        }
        if let Some((w, p, x, y)) = en {
            let (cx, cy) = self.to_canvas(p, x, y);
            self.view.fixed().move_(&w, cx, cy);
        }
        self.view.canvas().queue_draw();
    }

    fn overlay(&self, s: &gtk::Snapshot) {
        let (sel, painted, drag, tool) = {
            let st = self.st.borrow();
            (st.sel, st.painted, st.drag.clone(), st.tool.clone())
        };
        if let (Some((p, a, b)), true) = (sel, painted) {
            let color = gdk::RGBA::new(0x4C as f32 / 255.0, 0x6E as f32 / 255.0, 0xF5 as f32 / 255.0, 0.3);
            for (x0, top, x1, bottom, _) in self.lines(p, a, b) {
                let (c0, c1) = (self.to_canvas(p, x0, top), self.to_canvas(p, x1, bottom));
                s.append_color(&color, &rect(c0.0.min(c1.0), c0.1.min(c1.1), (c1.0 - c0.0).abs(), (c1.1 - c0.1).abs()));
            }
        }
        match drag {
            Some(Drag::Ink { page, points }) if points.len() >= 2 => {
                let pts: Vec<(f64, f64)> = points.iter().map(|&(x, y)| self.to_canvas(page, x, y)).collect();
                let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
                for &(x, y) in &pts {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
                let cr = s.append_cairo(&rect(x0 - 3.0, y0 - 3.0, x1 - x0 + 6.0, y1 - y0 + 6.0));
                cr.set_source_rgb(0x19 as f64 / 255.0, 0x71 as f64 / 255.0, 0xC2 as f64 / 255.0);
                cr.set_line_width(2.0);
                cr.set_line_join(gtk::cairo::LineJoin::Round);
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.move_to(pts[0].0, pts[0].1);
                for &(x, y) in &pts[1..] {
                    cr.line_to(x, y);
                }
                let _ = cr.stroke();
            }
            Some(Drag::Area { page, start, end, moved: true }) => {
                let (a, b) = (self.to_canvas(page, start.0, start.1), self.to_canvas(page, end.0, end.1));
                let (x0, y0, w, h) = (a.0.min(b.0), a.1.min(b.1), (b.0 - a.0).abs(), (b.1 - a.1).abs());
                let cr = s.append_cairo(&rect(x0 - 2.0, y0 - 2.0, w + 4.0, h + 4.0));
                cr.rectangle(x0 + 0.5, y0 + 0.5, w, h);
                if tool == "whiteout" {
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgb(0x86 as f64 / 255.0, 0x8E as f64 / 255.0, 0x96 as f64 / 255.0);
                } else {
                    cr.set_source_rgb(0xC9 as f64 / 255.0, 0x2A as f64 / 255.0, 0x2A as f64 / 255.0);
                }
                cr.set_line_width(1.0);
                cr.set_dash(&[3.0, 2.0], 0.0);
                let _ = cr.stroke();
            }
            _ => {}
        }
    }

    fn selected_text(&self) -> String {
        let Some((p, a, b)) = self.st.borrow().sel else { return String::new() };
        self.lines(p, a, b).into_iter().map(|l| l.4).collect::<Vec<_>>().join("\n")
    }

    fn copy(&self) {
        let text = self.selected_text();
        if !text.is_empty() {
            self.view.canvas().clipboard().set_text(&text);
            if let Some(app) = self.app() {
                app.flash_msg("Copied.", 5000);
            }
        }
    }

    fn mark(self: &Rc<Self>, kind: &str) {
        let Some((p, a, b)) = self.st.borrow().sel else { return };
        let quads: Vec<[f64; 4]> = self.lines(p, a, b).into_iter().map(|(x0, t, x1, bt, _)| [x0, t, x1, bt]).collect();
        self.clear_selection();
        let (Some(color), false) = (mark_color(kind), quads.is_empty()) else { return };
        let label = match kind {
            "highlight" => "Highlight",
            "underline" => "Underline",
            _ => "Strike-out",
        };
        let kind = if kind == "strike" { "strikeout" } else { kind };
        self.change(vec![Op::Mark { page: p, kind: kind.into(), quads, color: color.into(), note: String::new() }], label, None);
    }

    fn menu(e: &Rc<Ed>, cx: f64, cy: f64) {
        let Some((p, x, y)) = e.to_page(cx, cy, None) else { return };
        e.st.borrow_mut().current = p;
        let has_sel = e.st.borrow().sel.is_some();
        let mut items: Vec<Option<(String, String, Box<dyn Fn(&Rc<Ed>)>)>> = vec![];
        let item = |label: &str, accel: &str, f: Box<dyn Fn(&Rc<Ed>)>| Some((label.to_string(), accel.to_string(), f));
        if has_sel {
            items.push(item("Copy", &format!("{MOD}+C"), Box::new(|e| e.copy())));
            items.push(item("Highlight", "", Box::new(|e| e.mark("highlight"))));
            items.push(item("Underline", "", Box::new(|e| e.mark("underline"))));
            items.push(item("Strike out", "", Box::new(|e| e.mark("strike"))));
            items.push(None);
        }
        items.push(item(
            "Remove marks and notes here",
            "",
            Box::new(move |e| {
                e.change(vec![Op::DeleteAnnots { page: p, x0: x - 4.0, top: y - 4.0, x1: x + 4.0, bottom: y + 4.0 }], "Marks removed", None)
            }),
        ));
        items.push(None);
        items.push(item("Turn page left", "", Box::new(move |e| e.rotate(-90, Some(p)))));
        items.push(item("Turn page right", "", Box::new(move |e| e.rotate(90, Some(p)))));
        items.push(item("Delete page", "", Box::new(move |e| Ed::delete_page(e, Some(p)))));

        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        pop.set_halign(gtk::Align::Start);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for it in items {
            let Some((label, accel, f)) = it else {
                let s = widgets::hairline();
                s.set_margin_top(3);
                s.set_margin_bottom(3);
                col.append(&s);
                continue;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let l = widgets::label(&label, &[]);
            l.set_hexpand(true);
            row.append(&l);
            if !accel.is_empty() {
                row.append(&widgets::label(&accel, &["muted", "small"]));
            }
            let b = gtk::Button::new();
            b.add_css_class("flat-btn");
            b.add_css_class("compact");
            b.set_child(Some(&row));
            let w = Rc::downgrade(e);
            let pp = pop.clone();
            b.connect_clicked(move |_| {
                pp.popdown();
                if let Some(e) = w.upgrade() {
                    f(&e);
                }
            });
            col.append(&b);
        }
        pop.set_child(Some(&col));
        pop.set_parent(e.view.canvas());
        pop.set_pointing_to(Some(&gdk::Rectangle::new(cx as i32, cy as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    // -- pages -------------------------------------------------------------------------------------------

    fn page(&self, p: Option<usize>) -> usize {
        p.unwrap_or_else(|| self.view_page())
    }

    fn rotate(self: &Rc<Self>, angle: i64, p: Option<usize>) {
        let page = self.page(p);
        self.change(vec![Op::Rotate { page, angle }], "Page turned", None);
    }

    fn delete_page(e: &Rc<Ed>, p: Option<usize>) {
        let p = e.page(p);
        let Some(app) = e.app() else { return };
        if e.st.borrow().pages.len() <= 1 {
            app.flash_msg("A PDF needs at least one page.", 5000);
            return;
        }
        let w = Rc::downgrade(e);
        app.ask("Delete page", &format!("Delete page {}? (You can undo this.)", p + 1), move |yes| {
            if let (true, Some(e)) = (yes, w.upgrade()) {
                e.change(vec![Op::DeletePage { page: p }], "Page deleted", None);
            }
        });
    }

    fn move_page(self: &Rc<Self>, d: i64, p: Option<usize>) {
        let p = self.page(p);
        let to = p as i64 + d;
        if to >= 0 && (to as usize) < self.st.borrow().pages.len() {
            let to = to as usize;
            self.change(vec![Op::MovePage { page: p, to }], &format!("Page moved to {}", to + 1), None);
            self.st.borrow_mut().current = to;
        }
    }

    fn insert_blank(self: &Rc<Self>, p: Option<usize>) {
        let p = self.page(p);
        let (mut w, mut h, r) = self.page_size(p);
        if r == 90 || r == 270 {
            std::mem::swap(&mut w, &mut h);
        }
        self.change(vec![Op::InsertBlank { at: p + 1, width: w, height: h }], "Blank page inserted", None);
    }

    fn merge(e: &Rc<Ed>, p: Option<usize>) {
        let p = e.page(p);
        let Some(app) = e.app() else { return };
        let dialog = gtk::FileDialog::builder().title("Insert the pages of a PDF").modal(true).build();
        dialog.set_filters(Some(&file_filters(&[("PDF", &["*.pdf"])])));
        let w = Rc::downgrade(e);
        let win = app.win.clone();
        glib::spawn_future_local(async move {
            let Ok(f) = dialog.open_future(Some(&win)).await else { return };
            let (Some(path), Some(e)) = (f.path(), w.upgrade()) else { return };
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(err) => {
                    if let Some(app) = e.app() {
                        app.error("Could not open file", &err.to_string());
                    }
                    return;
                }
            };
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            e.change(vec![Op::Append { data, at: Some(p + 1) }], &format!("Pages of {name} inserted"), None);
        });
    }
}

/// File dialog filters: (label, patterns), with upper-case extensions accepted too.
fn file_filters(types: &[(&str, &[&str])]) -> gio::ListStore {
    let store = gio::ListStore::new::<gtk::FileFilter>();
    for (label, pats) in types {
        let f = gtk::FileFilter::new();
        f.set_name(Some(label));
        for p in pats.iter() {
            f.add_pattern(p);
            if p.starts_with("*.") {
                f.add_pattern(&p.to_uppercase());
            }
        }
        store.append(&f);
    }
    store
}

/// The note text for a sticky note: a small dialog, as for a footnote.
fn note_dialog(app: &AppRc, on_done: impl FnOnce(String) + 'static) {
    let win = gtk::Window::builder().title("Note").modal(true).transient_for(&app.win).default_width(460).resizable(false).build();
    win.add_css_class("dialog");
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.add_css_class("dialog");
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(14);
    body.set_margin_bottom(12);
    body.append(&widgets::label("Footnote", &["title"]));
    let text = gtk::TextView::new();
    text.add_css_class("editor");
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    text.set_top_margin(4);
    text.set_bottom_margin(4);
    text.set_left_margin(6);
    text.set_right_margin(6);
    let frame = gtk::ScrolledWindow::new();
    frame.add_css_class("card");
    frame.set_hscrollbar_policy(gtk::PolicyType::Never);
    frame.set_min_content_height(96);
    frame.set_child(Some(&text));
    body.append(&frame);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(6);
    let done: Rc<RefCell<Option<Box<dyn FnOnce(String)>>>> = Rc::new(RefCell::new(Some(Box::new(on_done))));
    let w2 = win.clone();
    row.append(&widgets::flat_button("Cancel", "ghost", None, move || w2.close()));
    let w2 = win.clone();
    let t2 = text.clone();
    let d2 = done.clone();
    row.append(&widgets::flat_button("OK", "primary", None, move || {
        let b = t2.buffer();
        let s = b.text(&b.start_iter(), &b.end_iter(), false).trim().to_string();
        w2.close();
        if !s.is_empty() {
            if let Some(f) = d2.borrow_mut().take() {
                f(s);
            }
        }
    }));
    body.append(&row);
    let keys = gtk::EventControllerKey::new();
    let w2 = win.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w2.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(keys);
    win.set_child(Some(&body));
    win.present();
    text.grab_focus();
}

/// A font family from a PDF font name: "ABCDEF+Calibri-Bold" -> "Calibri", "TimesNewRomanPSMT" -> "Times New Roman".
fn family(font: &str) -> Option<String> {
    static SUBSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{6}\+").unwrap());
    static SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(PSMT|PS|MT)$").unwrap());
    static TEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(CM|LM|SF)[A-Z]").unwrap());
    let name = SUBSET.replace(font, "");
    let name = name.split(['-', ',']).next().unwrap_or("");
    let name = SUFFIX.replace(name, "").into_owned();
    let known = match name.as_str() {
        "TimesNewRoman" => Some("Times New Roman"),
        "CourierNew" => Some("Courier New"),
        "ArialMT" => Some("Arial"),
        "SegoeUI" => Some("Segoe UI"),
        "LiberationSerif" => Some("Liberation Serif"),
        "LiberationSans" => Some("Liberation Sans"),
        "DejaVuSans" => Some("DejaVu Sans"),
        "DejaVuSerif" => Some("DejaVu Serif"),
        _ => None,
    };
    if let Some(k) = known {
        return Some(k.to_string());
    }
    if TEX.is_match(&name) {
        return None; // TeX fonts
    }
    (!name.is_empty()).then_some(name)
}

fn generic(font: &str) -> &'static str {
    let n = font.to_lowercase();
    if ["mono", "courier", "consol", "cmtt", "code"].iter().any(|k| n.contains(k)) {
        return "mono";
    }
    if ["times", "serif", "roman", "georgia", "cambria", "garamond", "cmr", "lmroman", "book", "minion", "palatino"].iter().any(|k| n.contains(k))
        && !n.contains("sans")
    {
        return "serif";
    }
    "sans"
}

/// Whether most of the old words can still be found in the box (they were drawn in a way that the core
/// could not take out).
fn still_there(data: &[u8], p: usize, bx: (f64, f64, f64, f64), text: &str, pages: &[PageInfo]) -> Result<bool, String> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());
    let words: Vec<&str> = WORD.find_iter(text).map(|m| m.as_str()).filter(|w| w.chars().count() > 2).collect();
    if words.is_empty() {
        return Ok(false);
    }
    let doc = rewrite_pdfium::Document::open(data.to_vec()).map_err(|e| e.to_string())?;
    let (left, top) = pages.get(p).map(|b| (b.0, b.3)).ok_or_else(|| format!("no page {}", p + 1))?;
    let (x0, btop, x1, bottom) = bx;
    let found = doc.text_bounded(p, left + x0 - 1.0, top - bottom - 1.0, left + x1 + 1.0, top - btop + 1.0).map_err(|e| e.to_string())?;
    let hits = words.iter().filter(|w| found.contains(*w)).count();
    Ok(hits >= 1.max(words.len() / 2))
}

// -- PdfBar -----------------------------------------------------------------------------------------------

/// Tools, page operations and zoom for the PDF view.
pub struct PdfBar {
    root: gtk::Box,
}

impl PdfBar {
    pub fn new(app: &AppRc) -> PdfBar {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("bg");
        root.set_margin_start(6);
        root.set_margin_end(6);
        root.set_margin_bottom(3);
        let tools: Rc<RefCell<Vec<(String, Toggle)>>> = Rc::new(RefCell::new(vec![]));
        for (name, icon, tip) in TOOLS {
            let me = app.weak();
            let all = Rc::downgrade(&tools);
            let t = Toggle::new(Some(icon), "", name == "select", tip, move |_| {
                if let Some(all) = all.upgrade() {
                    for (n, b) in all.borrow().iter() {
                        b.set(n == name);
                    }
                }
                if let Some(app) = me.upgrade() {
                    app.pdf().set_tool(name);
                }
            });
            root.append(&t.button);
            tools.borrow_mut().push((name.to_string(), t));
        }
        root.append(&widgets::separator(4));
        let act = |f: fn(&PdfEditor)| {
            let me = app.weak();
            move || {
                if let Some(app) = me.upgrade() {
                    f(app.pdf());
                }
            }
        };
        let ops: [(&str, &str, fn(&PdfEditor)); 7] = [
            ("rotate-left", "Turn the page left", |e| e.rotate(-90)),
            ("rotate-right", "Turn the page right", |e| e.rotate(90)),
            ("up", "Move the page up", |e| e.move_page(-1)),
            ("down", "Move the page down", |e| e.move_page(1)),
            ("page-add", "Insert a blank page after this one", |e| e.insert_blank()),
            ("merge", "Insert the pages of another PDF after this page", |e| e.merge()),
            ("trash", "Delete this page", |e| e.delete_page()),
        ];
        for (icon, tip, f) in ops {
            root.append(&widgets::icon_button(icon, tip, act(f)));
        }
        root.append(&widgets::separator(4));
        root.append(&widgets::icon_button("minus", "Zoom out", act(|e| e.set_zoom(e.zoom() / 1.2))));
        root.append(&widgets::icon_button("plus", "Zoom in (Ctrl + mouse wheel)", act(|e| e.set_zoom(e.zoom() * 1.2))));
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        root.append(&spacer);
        let me = app.weak();
        let convert = widgets::icon_text_button(
            "convert",
            "Convert to document",
            "soft",
            Some(
                "Make an editable document of this PDF (text, headings, lists, tables, pictures), with the language model fixing \
                 broken words and sentences",
            ),
            move || {
                if let Some(app) = me.upgrade() {
                    app.convert_pdf();
                }
            },
        );
        convert.add_css_class("small");
        convert.set_valign(gtk::Align::Center);
        root.append(&convert);
        // the toggles live as long as the bar
        let keep = tools.clone();
        root.connect_destroy(move |_| {
            keep.borrow_mut().clear();
        });
        PdfBar { root }
    }
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }
}
