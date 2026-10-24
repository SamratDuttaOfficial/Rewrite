//! Things inserted into formatted documents and their dialogs: pictures, tables, charts, formulas,
//! notes, links, lines and page breaks, and page set-up. Objects are rewrite_core::flat::Obj values; the
//! editor shows each one with a View: pictures and formulas as paintables in the text, the others as
//! widgets at child anchors (tables are edited in place, one small text box per cell).

use crate::app::{App, AppRc};
use crate::rich::RichEditor;
use crate::widgets;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{cairo, gdk, gio, glib, graphene, gsk, pango};
use rewrite_core::charts::{self, Shape};
use rewrite_core::flat::{Math, Obj};
use rewrite_core::model::{inline_text, Chart, Image, Inline, Note, PageSetup, Paragraph, Run, Table, PAGE_SIZES};
use rewrite_core::{imaging, mathtex};
use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

pub const PICTURE_TYPES: &[(&str, &[&str])] =
    &[("Pictures", &["*.png", "*.jpg", "*.jpeg", "*.gif", "*.bmp", "*.tif", "*.tiff", "*.webp"]), ("All files", &["*"])];

// -- pictures --------------------------------------------------------------------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Picture {
        pub tex: RefCell<Option<gdk::Texture>>,
        pub hash: Cell<u64>,
        pub w: Cell<i32>,
        pub h: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Picture {
        const NAME: &'static str = "RewritePicture";
        type Type = super::Picture;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for Picture {}

    impl PaintableImpl for Picture {
        fn intrinsic_width(&self) -> i32 {
            self.w.get()
        }
        fn intrinsic_height(&self) -> i32 {
            self.h.get()
        }
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let snap = snapshot.downcast_ref::<gtk::Snapshot>().expect("a GTK snapshot");
            let rect = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            match self.tex.borrow().as_ref() {
                Some(t) => snap.append_scaled_texture(t, gsk::ScalingFilter::Trilinear, &rect),
                None => snap.append_color(&crate::theme::rgba("#DDDDDD"), &rect),
            }
        }
    }
}

glib::wrapper! {
    /// A picture in the text, drawn at its size on the page.
    pub struct Picture(ObjectSubclass<imp::Picture>) @implements gdk::Paintable;
}

mod gimp {
    use super::*;

    #[derive(Default)]
    pub struct Glyphs {
        pub layout: RefCell<Option<pango::Layout>>,
        pub color: RefCell<Option<gdk::RGBA>>,
        pub w: Cell<i32>,
        pub h: Cell<i32>,
        pub base: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Glyphs {
        const NAME: &'static str = "RewriteGlyphs";
        type Type = super::Glyphs;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for Glyphs {}

    impl PaintableImpl for Glyphs {
        fn intrinsic_width(&self) -> i32 {
            self.w.get()
        }
        fn intrinsic_height(&self) -> i32 {
            self.h.get()
        }
        fn snapshot(&self, snapshot: &gdk::Snapshot, _width: f64, height: f64) {
            let snap = snapshot.downcast_ref::<gtk::Snapshot>().expect("a GTK snapshot");
            let Some(l) = self.layout.borrow().clone() else { return };
            let c = self.color.borrow().unwrap_or(gdk::RGBA::BLACK);
            snap.save();
            // the paintable stands on the baseline: draw the text's baseline at its bottom
            snap.translate(&graphene::Point::new(0.0, (height - self.base.get() as f64) as f32));
            snap.append_layout(&l, &c);
            snap.restore();
        }
    }
}

glib::wrapper! {
    /// A short piece of text drawn in the line (list markers, inline formulas), on the text's baseline.
    pub struct Glyphs(ObjectSubclass<gimp::Glyphs>) @implements gdk::Paintable;
}

impl Default for Glyphs {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Glyphs {
    pub fn set(&self, w: &impl IsA<gtk::Widget>, text: &str, fd: &pango::FontDescription, color: gdk::RGBA, min_width: i32) {
        let l = w.create_pango_layout(Some(text));
        l.set_font_description(Some(fd));
        let (pw, _) = l.pixel_size();
        let base = (l.baseline() / pango::SCALE).max(1);
        let p = self.imp();
        *p.layout.borrow_mut() = Some(l);
        *p.color.borrow_mut() = Some(color);
        p.w.set(pw.max(min_width).max(1));
        p.h.set(base);
        p.base.set(base);
        self.invalidate_size();
        self.invalidate_contents();
    }
}

fn data_hash(data: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

fn decode(data: &[u8]) -> Option<gdk::Texture> {
    let bytes = match imaging::sniff(data) {
        "png" | "jpeg" | "tiff" => data.to_vec(),
        "gif" | "bmp" | "webp" => imaging::to_png(data).ok()?,
        _ => return None,
    };
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()
}

impl Picture {
    fn new() -> Picture {
        glib::Object::new()
    }

    /// Show these pixels at the picture's size on the page (no wider than the text column).
    fn set(&self, ed: &RichEditor, img: &Image) {
        let p = self.imp();
        let hash = data_hash(&img.data);
        if p.tex.borrow().is_none() || p.hash.get() != hash {
            *p.tex.borrow_mut() = decode(&img.data);
            p.hash.set(hash);
        }
        let k = ed.pt_px();
        let mut w = (img.width * k).max(8.0);
        let mut h = (img.height * k).max(8.0);
        let col = ed.column_px();
        if w > col {
            h = h * col / w;
            w = col;
        }
        p.w.set(w as i32);
        p.h.set(h as i32);
        self.invalidate_size();
        self.invalidate_contents();
    }
}

// -- views -----------------------------------------------------------------------------------------

enum Kind {
    Picture(Picture),
    Label(gtk::Label),
    Rule(gtk::DrawingArea),
    Math(Glyphs),
    Note(gtk::Label),
    Chart(gtk::DrawingArea, Rc<RefCell<(Chart, f64)>>),
    Table(Rc<TableView>),
}

/// What shows an object in the editor.
pub struct View {
    anchor: Option<gtk::TextChildAnchor>,
    paint: Option<gdk::Paintable>,
    widget: Option<gtk::Widget>,
    kind: Kind,
}

impl View {
    pub fn new(app: &AppRc, ed: &RichEditor, oid: &str, obj: &Obj) -> View {
        let kind = match obj {
            Obj::Image(_) => Kind::Picture(Picture::new()),
            Obj::Chart(_) => {
                let d = gtk::DrawingArea::new();
                let data = Rc::new(RefCell::new((Chart::default(), 1.0)));
                let data2 = data.clone();
                let a = app.weak();
                d.set_draw_func(move |_, cr, _, _| {
                    let fam = a.upgrade().map(|app| app.theme().families.ui.clone()).unwrap_or_else(|| "Sans".into());
                    let (chart, k) = &*data2.borrow();
                    draw_chart(cr, chart, *k, &fam);
                });
                Kind::Chart(d, data)
            }
            Obj::Table(_) => Kind::Table(TableView::new(app)),
            Obj::Math(_) => Kind::Math(Glyphs::default()),
            Obj::Note(_) => {
                let l = gtk::Label::new(Some("1"));
                l.add_css_class("rich-note");
                l.set_valign(gtk::Align::Start);
                Kind::Note(l)
            }
            Obj::Rule(_) => {
                let d = gtk::DrawingArea::new();
                let a = app.weak();
                d.set_draw_func(move |_, cr, w, h| {
                    let col = a.upgrade().map(|app| app.theme().c("faint")).unwrap_or("#9A9FA7");
                    let (r, g, b) = crate::theme::rgb(col);
                    cr.set_source_rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
                    cr.rectangle(0.0, (h / 2) as f64, w as f64, 1.0);
                    let _ = cr.fill();
                });
                Kind::Rule(d)
            }
            _ => {
                let l = gtk::Label::new(None);
                l.add_css_class("rich-muted");
                l.set_margin_start(4);
                l.set_margin_end(4);
                if matches!(obj, Obj::Opaque(_)) {
                    l.set_tooltip_text(Some("Kept exactly as it was in the original file (it cannot be edited here)."));
                }
                Kind::Label(l)
            }
        };
        let widget: Option<gtk::Widget> = match &kind {
            Kind::Picture(_) | Kind::Math(_) => None,
            Kind::Label(l) | Kind::Note(l) => Some(l.clone().upcast()),
            Kind::Rule(d) | Kind::Chart(d, _) => Some(d.clone().upcast()),
            Kind::Table(t) => Some(t.grid.clone().upcast()),
        };
        if let (Some(w), false) = (&widget, matches!(kind, Kind::Table(_))) {
            wire(app, w, oid);
        }
        let paint: Option<gdk::Paintable> = match &kind {
            Kind::Picture(p) => Some(p.clone().upcast()),
            Kind::Math(g) => Some(g.clone().upcast()),
            _ => None,
        };
        let v = View { anchor: widget.as_ref().map(|_| gtk::TextChildAnchor::new()), paint, widget, kind };
        v.update(app, ed, obj);
        v
    }

    /// The paintable that stands in the text (pictures and formulas).
    pub fn paintable(&self) -> Option<&gdk::Paintable> {
        self.paint.as_ref()
    }

    pub fn anchor(&self) -> Option<&gtk::TextChildAnchor> {
        self.anchor.as_ref()
    }

    pub fn widget(&self) -> Option<&gtk::Widget> {
        self.widget.as_ref()
    }

    /// Show an edited version of the object.
    pub fn update(&self, app: &AppRc, ed: &RichEditor, obj: &Obj) {
        let th = app.theme();
        match (&self.kind, obj) {
            (Kind::Picture(p), Obj::Image(img)) => p.set(ed, img),
            (Kind::Label(l), o) => l.set_label(&label_text(o)),
            (Kind::Rule(d), _) => {
                d.set_content_width((ed.column_px() - 8.0).max(20.0) as i32);
                d.set_content_height(12);
                d.queue_draw();
            }
            (Kind::Math(g), Obj::Math(m)) => {
                let shown = mathtex::to_unicode(&m.tex);
                let shown = if shown.is_empty() { m.tex.clone() } else { shown };
                let mut fd = pango::FontDescription::new();
                fd.set_family(&th.families.serif);
                fd.set_style(pango::Style::Italic);
                let size = (th.size * if m.display { 1.25 } else { 1.0 }).floor();
                fd.set_size((size * pango::SCALE as f64) as i32);
                let text = format!(" {} ", if shown.is_empty() { "(formula)" } else { &shown });
                g.set(&app.text, &text, &fd, th.rgba("text"), 0);
            }
            (Kind::Note(l), Obj::Note(n)) => {
                let text = inline_text(&n.runs, true);
                let t = text.trim();
                let t: String = t.chars().take(200).collect();
                l.set_tooltip_text(Some(&format!("Note: {}\nDouble-click to edit", if t.is_empty() { "(empty)" } else { &t })));
            }
            (Kind::Chart(d, data), Obj::Chart(c)) => {
                let mut k = ed.pt_px();
                let (w_pt, h_pt) = (if c.width > 0.0 { c.width } else { 432.0 }, if c.height > 0.0 { c.height } else { 252.0 });
                let col = ed.column_px();
                if w_pt * k > col {
                    k = col / w_pt;
                }
                *data.borrow_mut() = (Chart { width: w_pt, height: h_pt, ..c.clone() }, k);
                d.set_content_width((w_pt * k) as i32);
                d.set_content_height((h_pt * k) as i32);
                d.queue_draw();
            }
            (Kind::Table(t), Obj::Table(tb)) => {
                drop(th);
                t.update(ed, tb);
            }
            _ => {}
        }
    }

    /// The object with the edits made in its view (tables), or None when the view does not edit.
    pub fn read(&self, obj: &Obj) -> Option<Obj> {
        match (&self.kind, obj) {
            (Kind::Table(t), Obj::Table(_)) => Some(Obj::Table(t.read())),
            _ => None,
        }
    }

    pub fn set_number(&self, n: usize) {
        if let Kind::Note(l) = &self.kind {
            l.set_label(&n.to_string());
        }
    }

    pub fn contains(&self, w: &gtk::Widget) -> bool {
        self.widget.as_ref().is_some_and(|me| me == w || w.is_ancestor(me))
    }

    pub fn table_cell(&self, w: &gtk::Widget) -> Option<(usize, usize)> {
        match &self.kind {
            Kind::Table(t) => t.cell_of(w),
            _ => None,
        }
    }
}

fn label_text(obj: &Obj) -> String {
    match obj {
        Obj::Pagebreak(_) => "─────────  page break  ─────────".into(),
        Obj::Break(b) if b.kind == "page" => "─────────  page break  ─────────".into(),
        Obj::Break(_) => "↵ column break".into(),
        Obj::Opaque(o) => format!("[{}]", if o.label.is_empty() { "object" } else { &o.label }),
        _ => "[object]".into(),
    }
}

/// Shared behaviour of object widgets: double-click edits, a click puts the cursor next to it.
fn wire(app: &AppRc, w: &gtk::Widget, oid: &str) {
    w.set_cursor_from_name(Some("pointer"));
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let a = app.weak();
    let oid = oid.to_string();
    click.connect_pressed(move |g, n, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        let Some(app) = a.upgrade() else { return };
        if n == 2 {
            let oid = oid.clone();
            glib::idle_add_local_once(move || app.rich_edit_object(&oid));
            return;
        }
        if let Some(i) = app.rich().object_index(&oid) {
            app.buf.place_cursor(&app.buf.iter_at_offset(i as i32 + 1));
        }
        app.text.grab_focus();
    });
    w.add_controller(click);
}

fn set_source(cr: &cairo::Context, hex: &str) {
    let (r, g, b) = crate::theme::rgb(hex);
    cr.set_source_rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
}

/// A chart drawn from the Rust core's layout, `k` pixels per point.
fn draw_chart(cr: &cairo::Context, chart: &Chart, k: f64, family: &str) {
    let font = |size: f64, bold: bool| {
        cr.select_font_face(family, cairo::FontSlant::Normal, if bold { cairo::FontWeight::Bold } else { cairo::FontWeight::Normal });
        cr.set_font_size((size * k).round().max(6.0));
    };
    let measure = |t: &str, size: f64, bold: bool| -> f64 {
        font(size, bold);
        cr.text_extents(t).map(|e| e.x_advance() / k).unwrap_or_else(|_| charts::approx_measure(t, size, bold))
    };
    let shapes = charts::layout(chart, chart.width, chart.height, &measure);
    for s in shapes {
        match s {
            Shape::Rect { x, y, w, h, fill, stroke } => {
                cr.rectangle(x * k, y * k, w * k, h * k);
                if let Some(f) = &fill {
                    set_source(cr, f);
                    if stroke.is_some() {
                        let _ = cr.fill_preserve();
                    } else {
                        let _ = cr.fill();
                    }
                }
                if let Some(st) = &stroke {
                    set_source(cr, st);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                }
                cr.new_path();
            }
            Shape::Line { pts, color, width } => {
                if pts.len() >= 2 {
                    cr.move_to(pts[0].0 * k, pts[0].1 * k);
                    for p in &pts[1..] {
                        cr.line_to(p.0 * k, p.1 * k);
                    }
                    set_source(cr, &color);
                    cr.set_line_width((width * k).max(1.0));
                    let _ = cr.stroke();
                }
            }
            Shape::Poly { pts, fill, stroke } => {
                if pts.len() >= 3 {
                    cr.move_to(pts[0].0 * k, pts[0].1 * k);
                    for p in &pts[1..] {
                        cr.line_to(p.0 * k, p.1 * k);
                    }
                    cr.close_path();
                    if let Some(f) = &fill {
                        set_source(cr, f);
                        let _ = cr.fill_preserve();
                    }
                    if let Some(st) = &stroke {
                        set_source(cr, st);
                        cr.set_line_width(1.0);
                        let _ = cr.stroke_preserve();
                    }
                    cr.new_path();
                }
            }
            Shape::Dot { x, y, r, fill } => {
                cr.arc(x * k, y * k, r * k, 0.0, std::f64::consts::TAU);
                set_source(cr, &fill);
                let _ = cr.fill();
            }
            Shape::Text { x, y, text, size, color, anchor, bold, angle } => {
                font(size, bold);
                let Ok(ext) = cr.text_extents(&text) else { continue };
                let Ok(fe) = cr.font_extents() else { continue };
                let (tw, th) = (ext.x_advance(), fe.ascent() + fe.descent());
                let (dx, dy) = charts::anchor_offset(anchor, tw, th);
                set_source(cr, &color);
                let _ = cr.save();
                cr.translate(x * k, y * k);
                if angle != 0.0 {
                    cr.rotate(-angle.to_radians());
                }
                cr.move_to(dx, dy + fe.ascent());
                let _ = cr.show_text(&text);
                let _ = cr.restore();
            }
        }
    }
}

// -- tables ----------------------------------------------------------------------------------------

/// The text of a table cell, paragraphs on lines of their own.
fn cell_text(cell: &[Paragraph]) -> String {
    cell.iter()
        .map(|p| {
            p.content
                .iter()
                .map(|x| match x {
                    Inline::Run(r) => r.text.clone(),
                    Inline::Break(_) => " ".into(),
                    _ => String::new(),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn empty_cell() -> Vec<Paragraph> {
    vec![Paragraph::default()]
}

/// A table edited in place: one small text box per cell.
pub struct TableView {
    grid: gtk::Grid,
    obj: RefCell<Table>,
    cells: RefCell<Vec<Vec<(gtk::TextView, String)>>>,
    app: Weak<App>,
}

impl TableView {
    fn new(app: &AppRc) -> Rc<TableView> {
        let grid = gtk::Grid::new();
        grid.add_css_class("rich-table");
        grid.set_column_spacing(1);
        grid.set_row_spacing(1);
        grid.set_halign(gtk::Align::Start);
        Rc::new(TableView { grid, obj: RefCell::new(Table::default()), cells: RefCell::new(vec![]), app: app.weak() })
    }

    fn update(self: &Rc<Self>, ed: &RichEditor, t: &Table) {
        *self.obj.borrow_mut() = t.clone();
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        let mut rows = t.rows.clone();
        if rows.is_empty() {
            rows = vec![vec![vec![]]];
        }
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
        let col_px = ed.column_px() - 6.0 - ncols as f64;
        let fr: Vec<f64> = match &t.widths {
            Some(w) if w.len() == ncols && w.iter().sum::<f64>() > 0.0 => {
                let s: f64 = w.iter().sum();
                w.iter().map(|x| x / s).collect()
            }
            _ => vec![1.0 / ncols as f64; ncols],
        };
        let mut cells = vec![];
        for (r, row) in rows.iter().enumerate() {
            let mut line = vec![];
            for k in 0..ncols {
                let cell: &[Paragraph] = row.get(k).map(|c| c.as_slice()).unwrap_or(&[]);
                let head = r == 0 && t.header;
                let tv = gtk::TextView::new();
                tv.set_wrap_mode(gtk::WrapMode::WordChar);
                tv.add_css_class("rich-cell");
                if head {
                    tv.add_css_class("head");
                }
                tv.set_left_margin(5);
                tv.set_right_margin(5);
                tv.set_top_margin(3);
                tv.set_bottom_margin(3);
                tv.set_size_request(((col_px * fr[k]) as i32).max(24), -1);
                let text = cell_text(cell);
                let b = tv.buffer();
                b.begin_irreversible_action();
                b.set_text(&text);
                b.end_irreversible_action();
                let a = self.app.clone();
                b.connect_changed(move |_| {
                    if let Some(app) = a.upgrade() {
                        app.rich().notify_modified();
                    }
                });
                let keys = gtk::EventControllerKey::new();
                keys.set_propagation_phase(gtk::PropagationPhase::Capture);
                let me = Rc::downgrade(self);
                keys.connect_key_pressed(move |_, key, _, state| {
                    let d = match key {
                        gdk::Key::Tab if state.contains(gdk::ModifierType::SHIFT_MASK) => -1,
                        gdk::Key::Tab => 1,
                        gdk::Key::ISO_Left_Tab => -1,
                        _ => return glib::Propagation::Proceed,
                    };
                    if let Some(me) = me.upgrade() {
                        me.next_cell(r, k, d);
                    }
                    glib::Propagation::Stop
                });
                tv.add_controller(keys);
                self.grid.attach(&tv, k as i32, r as i32, 1, 1);
                line.push((tv, text));
            }
            cells.push(line);
        }
        *self.cells.borrow_mut() = cells;
    }

    fn next_cell(&self, r: usize, k: usize, d: i32) {
        let cells = self.cells.borrow();
        let flat: Vec<(usize, usize)> = (0..cells.len()).flat_map(|i| (0..cells[i].len()).map(move |j| (i, j))).collect();
        let Some(pos) = flat.iter().position(|&p| p == (r, k)) else { return };
        let to = pos as i32 + d;
        if to < 0 || to as usize >= flat.len() {
            return;
        }
        let (i, j) = flat[to as usize];
        let tv = &cells[i][j].0;
        tv.grab_focus();
        let b = tv.buffer();
        b.select_range(&b.start_iter(), &b.end_iter());
    }

    fn cell_of(&self, w: &gtk::Widget) -> Option<(usize, usize)> {
        let cells = self.cells.borrow();
        for (r, line) in cells.iter().enumerate() {
            for (k, (tv, _)) in line.iter().enumerate() {
                let tw: &gtk::Widget = tv.upcast_ref();
                if tw == w || w.is_ancestor(tw) {
                    return Some((r, k));
                }
            }
        }
        // the lines between the cells: the menu acts on the first cell
        None.or_else(|| (!cells.is_empty()).then_some((0, 0)))
    }

    /// The table with the cells' edits.
    fn read(&self) -> Table {
        let mut obj = self.obj.borrow().clone();
        let cells = self.cells.borrow();
        for (r, line) in cells.iter().enumerate() {
            for (k, (tv, before)) in line.iter().enumerate() {
                let b = tv.buffer();
                let now = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
                if now == *before {
                    continue;
                }
                while obj.rows.len() <= r {
                    obj.rows.push(vec![]);
                }
                while obj.rows[r].len() <= k {
                    obj.rows[r].push(vec![]);
                }
                let old = &obj.rows[r][k];
                let first = old.iter().flat_map(|p| p.content.iter()).find_map(|x| if let Inline::Run(r) = x { Some(r.clone()) } else { None });
                let fmt = first.map(|r| Run { text: String::new(), ..r }).unwrap_or_default();
                let align = old.first().and_then(|p| p.align.clone());
                obj.rows[r][k] = now
                    .split('\n')
                    .map(|l| Paragraph {
                        content: if l.is_empty() { vec![] } else { vec![Inline::Run(fmt.styled(l))] },
                        style: "normal".into(),
                        align: align.clone(),
                        list: None,
                        indent: 0,
                    })
                    .collect();
            }
        }
        obj
    }
}

// -- menus -----------------------------------------------------------------------------------------

/// A small popup menu of flat buttons.
pub struct Menu {
    pub pop: gtk::Popover,
    col: gtk::Box,
}

impl Menu {
    pub fn new() -> Menu {
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        pop.set_halign(gtk::Align::Start);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let sc = gtk::ScrolledWindow::new();
        sc.set_hscrollbar_policy(gtk::PolicyType::Never);
        sc.set_propagate_natural_height(true);
        sc.set_propagate_natural_width(true);
        sc.set_max_content_height(520);
        sc.set_child(Some(&col));
        pop.set_child(Some(&sc));
        Menu { pop, col }
    }

    pub fn add_widget(&self, child: &impl IsA<gtk::Widget>, f: impl Fn() + 'static) -> gtk::Button {
        let b = gtk::Button::new();
        b.add_css_class("flat-btn");
        b.add_css_class("compact");
        b.set_child(Some(child));
        let p = self.pop.clone();
        b.connect_clicked(move |_| {
            p.popdown();
            f();
        });
        self.col.append(&b);
        b
    }

    pub fn add(&self, label: &str, f: impl Fn() + 'static) -> gtk::Button {
        let l = widgets::label(label, &[]);
        self.add_widget(&l, f)
    }

    /// An entry shown as chosen (a radio item).
    pub fn add_choice(&self, label: &str, on: bool, f: impl Fn() + 'static) -> gtk::Button {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let mark = widgets::label(if on { "•" } else { " " }, &["accent"]);
        mark.set_width_chars(1);
        row.append(&mark);
        row.append(&widgets::label(label, &[]));
        self.add_widget(&row, f)
    }

    pub fn sep(&self) {
        let s = widgets::hairline();
        s.set_margin_top(3);
        s.set_margin_bottom(3);
        self.col.append(&s);
    }

    fn cleanup(&self) {
        self.pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
    }

    /// Show at a point of a widget.
    pub fn popup_at(&self, parent: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        self.pop.set_parent(parent);
        self.pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.cleanup();
        self.pop.popup();
    }

    /// Show under a widget (a toolbar button).
    pub fn popup_under(&self, parent: &impl IsA<gtk::Widget>) {
        self.pop.set_parent(parent);
        self.cleanup();
        self.pop.popup();
    }
}

// -- dialogs ---------------------------------------------------------------------------------------

/// A small modal form: a title, rows of labelled fields, OK and Cancel.
struct Form {
    win: gtk::Window,
    body: gtk::Box,
    ok: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
}

impl Form {
    fn new(app: &AppRc, title: &str, width: i32) -> Form {
        let win = gtk::Window::builder().title(title).modal(true).transient_for(&app.win).resizable(false).default_width(width).build();
        win.add_css_class("dialog");
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.set_margin_start(16);
        body.set_margin_end(16);
        body.set_margin_top(14);
        body.set_margin_bottom(14);
        win.set_child(Some(&body));
        let ok: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        // Escape closes; Enter anywhere in the form is OK, except in a text box (a new line there) and in an
        // open drop-down list (it picks the item)
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = win.clone();
        let ok2 = ok.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter) {
                let typing = gtk::prelude::RootExt::focus(&w).is_some_and(|f| {
                    f.is::<gtk::TextView>()
                        || f.ancestor(gtk::TextView::static_type()).is_some()
                        || f.ancestor(gtk::Popover::static_type()).is_some()
                });
                let f = ok2.borrow().clone();
                if let (Some(f), false) = (f, typing) {
                    f();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        win.add_controller(keys);
        Form { win, body, ok }
    }

    fn heading(&self, text: &str) {
        let l = widgets::label(text, &["title"]);
        l.set_margin_bottom(4);
        self.body.append(&l);
    }

    fn note(&self, text: &str, classes: &[&str]) -> gtk::Label {
        let l = widgets::wrap_label(text, classes);
        l.set_max_width_chars(60);
        self.body.append(&l);
        l
    }

    fn row(&self, label: &str, w: &impl IsA<gtk::Widget>) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_top(3);
        let l = widgets::label(label, &["muted"]);
        l.set_width_chars(16);
        row.append(&l);
        w.set_hexpand(true);
        row.append(w);
        self.body.append(&row);
    }

    fn field(&self, label: &str, value: &str, width: i32) -> gtk::Entry {
        let e = gtk::Entry::new();
        e.set_text(value);
        e.set_width_chars(width);
        self.row(label, &e);
        e
    }

    fn check(&self, label: &str, value: bool) -> gtk::CheckButton {
        let c = gtk::CheckButton::with_label(label);
        c.set_active(value);
        c.set_margin_top(4);
        self.body.append(&c);
        c
    }

    fn text_box(&self, value: &str, lines: i32, mono: Option<&str>, wrap: bool) -> gtk::TextView {
        let tv = gtk::TextView::new();
        tv.buffer().set_text(value);
        tv.set_wrap_mode(if wrap { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });
        tv.set_left_margin(6);
        tv.set_right_margin(6);
        tv.set_top_margin(4);
        tv.set_bottom_margin(4);
        tv.add_css_class("card");
        if let Some(fam) = mono {
            let css = gtk::CssProvider::new();
            css.load_from_string(&format!("textview, text {{ font-family: \"{fam}\"; }}"));
            #[allow(deprecated)]
            tv.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 3);
        }
        let sc = gtk::ScrolledWindow::new();
        sc.set_child(Some(&tv));
        sc.set_min_content_height(lines * 18);
        sc.set_margin_top(4);
        self.body.append(&sc);
        tv
    }

    /// The OK and Cancel buttons; `on_ok` also runs for Enter anywhere in the form but a text box.
    fn buttons(&self, ok_text: &str, on_ok: Rc<dyn Fn()>) {
        *self.ok.borrow_mut() = Some(on_ok.clone());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::End);
        row.set_margin_top(12);
        let w = self.win.clone();
        row.append(&widgets::flat_button("Cancel", "ghost", None, move || w.close()));
        row.append(&widgets::flat_button(ok_text, "primary", None, move || on_ok()));
        self.body.append(&row);
    }

    fn show(&self) {
        self.win.present();
    }
}

fn num(s: &str, default: f64) -> f64 {
    s.replace(',', ".").trim().parse::<f64>().unwrap_or(default)
}

fn text_of(tv: &gtk::TextView) -> String {
    let b = tv.buffer();
    b.text(&b.start_iter(), &b.end_iter(), false).to_string()
}

/// The text width of the document's page, in points.
fn page_text_width(app: &AppRc) -> f64 {
    app.rich().page().text_width()
}

/// A picture object from a file, sized as it would be printed (at most the page's text width).
pub fn load_picture(app: &AppRc, path: &Path, max_width: Option<f64>) -> Option<Image> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            app.error("Picture", &format!("{name} could not be read.\n\n{e}"));
            return None;
        }
    };
    let kind = imaging::sniff(&data);
    if kind.is_empty() {
        app.error("Picture", &format!("{name} is not a picture Rewrite can show."));
        return None;
    }
    let max_width = max_width.unwrap_or_else(|| page_text_width(app));
    let (w, h) = imaging::natural_size(&data, Some(max_width));
    Some(Image { data, ext: kind.to_string(), width: w, height: h, alt: String::new(), name })
}

fn choose_picture(app: &AppRc, title: &str, then: impl FnOnce(PathBuf) + 'static) {
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    for (label, pats) in PICTURE_TYPES {
        filters.append(&crate::app::filter(label, pats));
    }
    dialog.set_filters(Some(&filters));
    let dir = app.settings().last_dir.clone().unwrap_or_else(|| rewrite_core::paths::home().to_string_lossy().into_owned());
    dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    let win = app.win.clone();
    glib::spawn_future_local(async move {
        if let Ok(f) = dialog.open_future(Some(&win)).await {
            if let Some(p) = f.path() {
                then(p);
            }
        }
    });
}

/// Size, description and replacement of a picture.
fn image_dialog(app: &AppRc, img: &Image, on_done: impl Fn(Obj) + 'static) {
    let f = Form::new(app, "Picture", 420);
    f.heading("Picture");
    let obj = Rc::new(RefCell::new(img.clone()));
    let nat = imaging::natural_size(&img.data, None);
    let w0 = if img.width > 0.0 { img.width } else { nat.0 };
    let h0 = if img.height > 0.0 { img.height } else { nat.1 };
    let ratio = Rc::new(Cell::new(h0 / w0.max(1e-6)));
    let we = f.field("Width (pt)", &format!("{w0:.0}"), 10);
    let he = f.field("Height (pt)", &format!("{h0:.0}"), 10);
    let keep = f.check("Keep the proportions", true);
    let busy = Rc::new(Cell::new(false));
    let sync = {
        let (we, he, keep, ratio, busy) = (we.clone(), he.clone(), keep.clone(), ratio.clone(), busy.clone());
        Rc::new(move |which: char| {
            if !keep.is_active() || busy.get() {
                return;
            }
            busy.set(true);
            if which == 'w' {
                he.set_text(&format!("{:.0}", num(&we.text(), 0.0) * ratio.get()));
            } else {
                we.set_text(&format!("{:.0}", num(&he.text(), 0.0) / ratio.get().max(1e-6)));
            }
            busy.set(false);
        })
    };
    let s = sync.clone();
    we.connect_changed(move |_| s('w'));
    let s = sync.clone();
    he.connect_changed(move |_| s('h'));
    let alt = f.field("Description", &img.alt, 30);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_top(8);
    {
        let (app2, obj, ratio, we, s) = (app.clone(), obj.clone(), ratio.clone(), we.clone(), sync.clone());
        row.append(&widgets::flat_button("Replace picture…", "soft", None, move || {
            let (app3, obj, ratio, we, s) = (app2.clone(), obj.clone(), ratio.clone(), we.clone(), s.clone());
            choose_picture(&app2, "Choose a picture", move |p| {
                if let Some(new) = load_picture(&app3, &p, None) {
                    let w = num(&we.text(), new.width);
                    {
                        let mut o = obj.borrow_mut();
                        o.data = new.data;
                        o.ext = new.ext;
                        o.name = new.name;
                    }
                    ratio.set(new.height / new.width.max(1e-6));
                    we.set_text(&format!("{w:.0}"));
                    s('w');
                }
            });
        }));
    }
    {
        let (app2, we, s) = (app.clone(), we.clone(), sync.clone());
        row.append(&widgets::flat_button("Fit the page width", "soft", None, move || {
            we.set_text(&format!("{:.0}", page_text_width(&app2)));
            s('w');
        }));
    }
    f.body.append(&row);
    let win = f.win.clone();
    f.buttons(
        "OK",
        Rc::new(move || {
            let mut o = obj.borrow().clone();
            o.width = num(&we.text(), if o.width > 0.0 { o.width } else { 100.0 }).max(4.0);
            o.height = num(&he.text(), if o.height > 0.0 { o.height } else { 100.0 }).max(4.0);
            o.alt = alt.text().to_string();
            win.close();
            on_done(Obj::Image(o));
        }),
    );
    f.show();
}

/// The link dialog: `text` is None when there is a selection (the link goes on it).
fn link_form(app: &AppRc, url: Option<&str>, text: Option<&str>, on_done: impl Fn(Option<String>, Option<String>) + 'static) {
    let f = Form::new(app, "Link", 440);
    f.heading("Link");
    let ue = f.field("Address", url.unwrap_or("https://"), 36);
    let te = text.map(|t| f.field("Text to show", t, 36));
    f.note(&format!("Leave the address empty to remove the link. {}+click a link to open it.", crate::theme::MOD), &["small", "muted"])
        .set_margin_top(6);
    let win = f.win.clone();
    let ue2 = ue.clone();
    f.buttons(
        "OK",
        Rc::new(move || {
            let url = ue2.text().trim().to_string();
            let text = te.as_ref().map(|e| e.text().to_string());
            win.close();
            on_done(if matches!(url.as_str(), "" | "https://" | "http://") { None } else { Some(url) }, text);
        }),
    );
    f.show();
    ue.grab_focus();
    ue.select_region(0, -1);
}

/// A formula in TeX, with a live look at how it reads.
fn math_dialog(app: &AppRc, m: &Math, on_done: impl Fn(Obj) + 'static) {
    let f = Form::new(app, "Formula", 480);
    f.heading("Formula");
    f.note("Type it in TeX, for example  \\frac{a}{b},  x^2,  \\sum_{i=1}^{n} x_i,  \\alpha.", &["small", "muted"]);
    let (mono, serif, size) = {
        let th = app.theme();
        (th.families.mono.clone(), th.families.serif.clone(), th.size)
    };
    let src = f.text_box(&m.tex, 4, Some(&mono), true);
    let shown = widgets::wrap_label("", &[]);
    shown.set_margin_top(8);
    let mut fd = pango::FontDescription::new();
    fd.set_family(&serif);
    fd.set_style(pango::Style::Italic);
    fd.set_size(((size + 3.0) * pango::SCALE as f64) as i32);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrFontDesc::new(&fd));
    shown.set_attributes(Some(&attrs));
    f.body.append(&shown);
    let display = f.check("On a line of its own (display formula)", m.display);
    let show = {
        let (src, shown) = (src.clone(), shown.clone());
        move || {
            let tex = text_of(&src);
            let u = mathtex::to_unicode(&tex);
            shown.set_label(if u.is_empty() { &tex } else { &u });
        }
    };
    show();
    src.buffer().connect_changed(move |_| show());
    let win = f.win.clone();
    let src2 = src.clone();
    let ok: Rc<dyn Fn()> = Rc::new(move || {
        let tex = text_of(&src2).trim().to_string();
        win.close();
        if !tex.is_empty() {
            on_done(Obj::Math(Math { tex, display: display.is_active() }));
        }
    });
    f.buttons("OK", ok.clone());
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, state| {
        if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && state.contains(gdk::ModifierType::CONTROL_MASK) {
            ok();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    f.win.add_controller(keys);
    f.show();
    src.grab_focus();
}

fn note_dialog(app: &AppRc, n: &Note, on_done: impl Fn(Obj) + 'static) {
    let f = Form::new(app, "Note", 460);
    f.heading("Footnote");
    let before: String = n.runs.iter().filter_map(|x| if let Inline::Run(r) = x { Some(r.text.as_str()) } else { None }).collect();
    let tb = f.text_box(&before, 5, None, true);
    let win = f.win.clone();
    let tb2 = tb.clone();
    f.buttons(
        "OK",
        Rc::new(move || {
            let text = text_of(&tb2).trim().to_string();
            win.close();
            if text != before.trim() {
                let runs = if text.is_empty() { vec![] } else { vec![Inline::Run(Run::new(text))] };
                on_done(Obj::Note(Note { runs }));
            }
        }),
    );
    f.show();
    tb.grab_focus();
}

fn table_dialog(app: &AppRc, on_done: impl Fn(Obj) + 'static) {
    let f = Form::new(app, "Table", 360);
    f.heading("Insert a table");
    let re = f.field("Rows", "3", 6);
    let ce = f.field("Columns", "3", 6);
    let header = f.check("First row is a header", true);
    let win = f.win.clone();
    let re2 = re.clone();
    f.buttons(
        "Insert",
        Rc::new(move || {
            let r = (num(&re2.text(), 3.0) as i64).clamp(1, 200) as usize;
            let k = (num(&ce.text(), 3.0) as i64).clamp(1, 30) as usize;
            win.close();
            let rows = (0..r).map(|_| (0..k).map(|_| empty_cell()).collect()).collect();
            on_done(Obj::Table(Table { rows, header: header.is_active(), widths: None }));
        }),
    );
    f.show();
    re.grab_focus();
    re.select_region(0, -1);
}

/// A chart's kind, titles and numbers (typed or pasted as a table).
fn chart_dialog(app: &AppRc, c: &Chart, on_done: impl Fn(Obj) + 'static) {
    let f = Form::new(app, "Chart", 520);
    f.heading("Chart");
    let labels: Vec<&str> = charts::KINDS.iter().map(|k| k.1).collect();
    let kind = gtk::DropDown::from_strings(&labels);
    kind.set_selected(charts::KINDS.iter().position(|k| k.0 == c.kind).unwrap_or(0) as u32);
    kind.set_hexpand(false);
    kind.set_halign(gtk::Align::Start);
    f.row("Kind", &kind);
    let te = f.field("Title", &c.title, 28);
    let xe = f.field("X axis title", &c.x_title, 28);
    let ye = f.field("Y axis title", &c.y_title, 28);
    let legend = f.check("Show the legend", c.legend);
    f.note(
        "DATA — first row: series names; first column: categories. Paste from a spreadsheet, or separate values with tabs or commas.",
        &["tiny", "faint"],
    )
    .set_margin_top(8);
    let mono = app.theme().families.mono.clone();
    let data = f.text_box(&charts::format_table(c), 8, Some(&mono), false);
    let win = f.win.clone();
    let base = c.clone();
    let app2 = app.clone();
    let ok: Rc<dyn Fn()> = Rc::new(move || {
        let text = text_of(&data);
        if let Err(e) = check_table(&text) {
            app2.error("Chart", &format!("The numbers could not be read.\n\n{e}"));
            return;
        }
        let (cats, series) = charts::parse_table(&text);
        if series.is_empty() {
            app2.error("Chart", "Give at least one column of numbers.");
            return;
        }
        let k = charts::KINDS.get(kind.selected() as usize).map_or("bar", |k| k.0);
        let chart = Chart {
            kind: k.into(),
            title: te.text().to_string(),
            x_title: xe.text().to_string(),
            y_title: ye.text().to_string(),
            legend: legend.is_active(),
            categories: cats,
            series,
            ..base.clone()
        };
        win.close();
        on_done(Obj::Chart(chart));
    });
    f.buttons("OK", ok);
    // Enter does not end this dialog: the titles are typed one by one and the numbers below them
    *f.ok.borrow_mut() = None;
    f.show();
}

/// Check that every value cell of chart data, split the way `charts::parse_table` splits it, is empty or a
/// number; parse_table itself leaves out cells that are not. Err says which cell is wrong.
fn check_table(text: &str) -> Result<(), String> {
    let mut rows: Vec<(usize, Vec<String>)> = vec![];
    for (n, line) in text.trim_matches(|c| c == '\n' || c == '\r').lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let sep = if line.contains('\t') { '\t' } else if line.contains(';') && !line.contains(',') { ';' } else { ',' };
        rows.push((n + 1, line.split(sep).map(|c| c.trim().to_string()).collect()));
    }
    if rows.first().is_some_and(|(_, r)| r.len() > 1 && r[1..].iter().all(|c| charts::to_number(c).is_none())) {
        rows.remove(0); // the series names
    }
    let width = rows.iter().map(|(_, r)| r.len()).max().unwrap_or(1);
    let first = if width < 2 { 0 } else { 1 }; // a single column holds numbers only; otherwise categories come first
    for (line, cells) in &rows {
        for (k, cell) in cells.iter().enumerate().skip(first) {
            if !cell.is_empty() && charts::to_number(cell).is_none() {
                return Err(format!("Line {line}, column {}: \u{201c}{cell}\u{201d} is not a number.", k + 1));
            }
        }
    }
    Ok(())
}

fn page_setup_form(app: &AppRc, page: &PageSetup, on_done: impl Fn(PageSetup) + 'static) {
    let f = Form::new(app, "Page set-up", 400);
    f.heading("Page set-up");
    let mm = |pt: f64| format!("{:.0}", pt / 72.0 * 25.4);
    let (w, h) = (page.width, page.height);
    let mut names: Vec<&str> = PAGE_SIZES.iter().map(|p| p.0).collect();
    names.push("Custom");
    let cur = PAGE_SIZES
        .iter()
        .position(|(_, pw, ph)| {
            let mut a = [pw.round() as i64, ph.round() as i64];
            let mut b = [w.round() as i64, h.round() as i64];
            a.sort();
            b.sort();
            a == b
        })
        .unwrap_or(PAGE_SIZES.len());
    let size = gtk::DropDown::from_strings(&names);
    size.set_selected(cur as u32);
    size.set_hexpand(false);
    size.set_halign(gtk::Align::Start);
    f.row("Paper", &size);
    let land = f.check("Landscape", w > h);
    let pw = f.field("Width (mm)", &mm(w), 8);
    let ph = f.field("Height (mm)", &mm(h), 8);
    let m = page.margins;
    let mt = f.field("Top margin (mm)", &mm(m.0), 8);
    let mb = f.field("Bottom (mm)", &mm(m.2), 8);
    let ml = f.field("Left (mm)", &mm(m.3), 8);
    let mr = f.field("Right (mm)", &mm(m.1), 8);
    let numbers = f.check("Page numbers", page.numbers);
    let preset = {
        let (size, land, pw, ph) = (size.clone(), land.clone(), pw.clone(), ph.clone());
        Rc::new(move || {
            if let Some((_, mut w, mut h)) = PAGE_SIZES.get(size.selected() as usize).copied() {
                if land.is_active() {
                    std::mem::swap(&mut w, &mut h);
                }
                pw.set_text(&format!("{:.0}", w / 72.0 * 25.4));
                ph.set_text(&format!("{:.0}", h / 72.0 * 25.4));
            }
        })
    };
    let p = preset.clone();
    size.connect_selected_notify(move |_| p());
    let p = preset.clone();
    land.connect_toggled(move |_| p());
    let win = f.win.clone();
    f.buttons(
        "OK",
        Rc::new(move || {
            let k = 72.0 / 25.4;
            let (mut w, mut h) = (num(&pw.text(), 210.0) * k, num(&ph.text(), 297.0) * k);
            if let Some((_, sw, sh)) = PAGE_SIZES.get(size.selected() as usize).copied() {
                // keep the exact sizes, not the rounded millimetres
                (w, h) = (sw, sh);
                if land.is_active() {
                    std::mem::swap(&mut w, &mut h);
                }
            }
            let g = |e: &gtk::Entry| (num(&e.text(), 25.0) * k).max(0.0);
            let margins = (g(&mt), g(&mr), g(&mb), g(&ml));
            win.close();
            on_done(PageSetup { width: w, height: h, margins, numbers: numbers.is_active() });
        }),
    );
    f.show();
}

// -- what the app does with them -------------------------------------------------------------------

/// Insert a picture, table, chart, formula (math), footnote (note), link, rule or pagebreak.
pub fn insert(app: &AppRc, kind: &str) {
    let a = app.weak();
    let put = move |o: Obj| {
        if let Some(app) = a.upgrade() {
            if app.rich_active() {
                app.rich().insert_object(o);
                app.text.grab_focus();
            }
        }
    };
    match kind {
        "image" => {
            let app2 = app.clone();
            choose_picture(app, "Insert a picture", move |p| {
                if let Some(img) = load_picture(&app2, &p, None) {
                    put(Obj::Image(img));
                }
            });
        }
        "table" => table_dialog(app, put),
        "chart" => {
            let chart = Chart {
                kind: "bar".into(),
                title: String::new(),
                categories: vec!["A".into(), "B".into(), "C".into()],
                series: vec![("Series 1".into(), vec![Some(3.0), Some(5.0), Some(2.0)])],
                x_title: String::new(),
                y_title: String::new(),
                legend: true,
                width: 432.0,
                height: 252.0,
            };
            chart_dialog(app, &chart, put);
        }
        "math" => math_dialog(app, &Math { tex: String::new(), display: false }, put),
        "note" => note_dialog(app, &Note::default(), move |o| {
            if matches!(&o, Obj::Note(n) if !n.runs.is_empty()) {
                put(o)
            }
        }),
        "link" => app.rich_link(),
        "rule" => put(Obj::Rule(Default::default())),
        "pagebreak" => put(Obj::Pagebreak(Default::default())),
        _ => {}
    }
}

/// Add, change or remove the link on the selection (or insert linked text).
pub fn link_dialog(app: &AppRc) {
    let rich = app.rich();
    // the dialog's entry takes the selection away; remember it
    let sel = rich.selection();
    let fmt = rich.state().0;
    let a = app.weak();
    let done = move |url: Option<String>, text: Option<String>| {
        let Some(app) = a.upgrade() else { return };
        if !app.rich_active() {
            return;
        }
        let r = app.rich();
        if let Some((s, e)) = sel {
            app.buf.select_range(&app.iter_at(s), &app.iter_at(e));
            r.set_value("link", url);
        } else if let (Some(url), Some(text)) = (url, text.filter(|t| !t.is_empty())) {
            let start = app.cursor_offset();
            app.one_step(|| app.buf.insert_at_cursor(&text));
            let end = app.cursor_offset();
            app.buf.select_range(&app.iter_at(start), &app.iter_at(end));
            r.set_value("link", Some(url));
            app.buf.place_cursor(&app.iter_at(end));
        }
        app.text.grab_focus();
    };
    link_form(app, fmt.link.as_deref(), if sel.is_some() { None } else { Some("") }, done);
}

pub fn page_setup_dialog(app: &AppRc) {
    let page = app.rich().page();
    let a = app.weak();
    page_setup_form(app, &page, move |p| {
        if let Some(app) = a.upgrade() {
            if app.rich_active() {
                app.rich().set_page(p);
            }
        }
    });
}

impl App {
    /// Open the dialog that edits an object.
    pub fn rich_edit_object(self: &Rc<Self>, oid: &str) {
        if self.busy() || !self.rich_active() {
            return;
        }
        let Some(obj) = self.rich().object(oid) else { return };
        let a = self.weak();
        let id = oid.to_string();
        let set = move |new: Obj| {
            if let Some(app) = a.upgrade() {
                if app.rich_active() {
                    app.rich().set_object(&id, new, true);
                }
            }
        };
        match &obj {
            Obj::Image(i) => image_dialog(self, i, set),
            Obj::Chart(c) => chart_dialog(self, c, set),
            Obj::Math(m) => math_dialog(self, m, set),
            Obj::Note(n) => note_dialog(self, n, set),
            Obj::Opaque(_) => self.flash_msg("This part is kept exactly as it was in the original file; it cannot be edited here.", 5000),
            _ => {}
        }
    }

    /// The menu of an object, at (x, y) in the editor.
    pub fn rich_object_menu(self: &Rc<Self>, oid: &str, x: f64, y: f64) {
        let Some(obj) = self.rich().object(oid) else { return };
        let m = Menu::new();
        let label = match &obj {
            Obj::Image(_) => Some("Picture…"),
            Obj::Chart(_) => Some("Edit chart…"),
            Obj::Math(_) => Some("Edit formula…"),
            Obj::Note(_) => Some("Edit note…"),
            _ => None,
        };
        if let Some(l) = label {
            let (a, id) = (self.weak(), oid.to_string());
            m.add(l, move || {
                if let Some(app) = a.upgrade() {
                    app.rich_edit_object(&id)
                }
            });
        }
        if let Obj::Image(img) = &obj {
            let (a, img) = (self.weak(), img.clone());
            m.add("Save picture as…", move || {
                if let Some(app) = a.upgrade() {
                    app.save_picture(&img)
                }
            });
        }
        let (a, id) = (self.weak(), oid.to_string());
        m.add("Delete", move || {
            if let Some(app) = a.upgrade() {
                app.delete_object(&id)
            }
        });
        m.popup_at(&self.text, x, y);
    }

    pub fn delete_object(&self, oid: &str) {
        if let Some(i) = self.rich().object_index(oid) {
            self.one_step(|| {
                let (mut a, mut b) = (self.iter_at(i), self.iter_at(i + 1));
                self.buf.delete(&mut a, &mut b);
            });
            self.text.grab_focus();
        }
    }

    pub fn save_picture(&self, img: &Image) {
        let ext = format!(".{}", if img.ext.is_empty() { "png" } else { &img.ext }.replace("jpeg", "jpg"));
        let stem = Path::new(if img.name.is_empty() { "picture" } else { &img.name })
            .file_stem()
            .map_or("picture".to_string(), |s| s.to_string_lossy().into_owned());
        let dialog = gtk::FileDialog::builder().title("Save picture").modal(true).initial_name(format!("{stem}{ext}")).build();
        let data = img.data.clone();
        let me = self.weak();
        let win = self.win.clone();
        glib::spawn_future_local(async move {
            let Ok(f) = dialog.save_future(Some(&win)).await else { return };
            let Some(mut p) = f.path() else { return };
            if p.extension().is_none() {
                p.set_extension(ext.trim_start_matches('.'));
            }
            if let Err(e) = std::fs::write(&p, &data) {
                if let Some(app) = me.upgrade() {
                    app.error("Save picture", &format!("{}\n\n{e}", p.display()));
                }
            }
        });
    }

    /// The row and column menu of a table cell.
    pub fn rich_table_menu(self: &Rc<Self>, oid: &str, r: usize, k: usize, x: f64, y: f64) {
        if self.rich().object(oid).is_none() {
            return;
        }
        let change = {
            let (a, id) = (self.weak(), oid.to_string());
            move |f: Box<dyn Fn(&mut Table, usize)>| {
                let Some(app) = a.upgrade() else { return };
                let rich = app.rich();
                let Some(Obj::Table(mut t)) = rich.object(&id) else { return };
                let n = t.rows.iter().map(|x| x.len()).max().unwrap_or(1).max(1);
                for row in t.rows.iter_mut() {
                    while row.len() < n {
                        row.push(empty_cell());
                    }
                }
                f(&mut t, n);
                rich.set_object(&id, Obj::Table(t), true);
            }
        };
        let change = Rc::new(change);
        let m = Menu::new();
        let item = |label: &str, f: Box<dyn Fn(&mut Table, usize)>| {
            let c = change.clone();
            let f = Rc::new(f);
            m.add(label, move || {
                let f = f.clone();
                c(Box::new(move |t, n| f(t, n)));
            });
        };
        item("Insert row above", Box::new(move |t, n| t.rows.insert(r.min(t.rows.len()), (0..n).map(|_| empty_cell()).collect())));
        item("Insert row below", Box::new(move |t, n| t.rows.insert((r + 1).min(t.rows.len()), (0..n).map(|_| empty_cell()).collect())));
        item(
            "Delete row",
            Box::new(move |t, _| {
                if t.rows.len() > 1 && r < t.rows.len() {
                    t.rows.remove(r);
                }
            }),
        );
        m.sep();
        item(
            "Insert column left",
            Box::new(move |t, _| {
                for row in t.rows.iter_mut() {
                    row.insert(k.min(row.len()), empty_cell());
                }
            }),
        );
        item(
            "Insert column right",
            Box::new(move |t, _| {
                for row in t.rows.iter_mut() {
                    row.insert((k + 1).min(row.len()), empty_cell());
                }
            }),
        );
        item(
            "Delete column",
            Box::new(move |t, n| {
                if n > 1 {
                    for row in t.rows.iter_mut() {
                        if k < row.len() {
                            row.remove(k);
                        }
                    }
                }
            }),
        );
        m.sep();
        item("Header row on / off", Box::new(|t, _| t.header = !t.header));
        let (a, id) = (self.weak(), oid.to_string());
        m.add("Delete table", move || {
            if let Some(app) = a.upgrade() {
                app.delete_object(&id)
            }
        });
        m.popup_at(&self.text, x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_table_check() {
        assert!(check_table(", Sales, Costs\nJan, 1, 2\nFeb, , 3").is_ok());
        assert!(check_table("1\n2\n3.5").is_ok());
        assert!(check_table("").is_ok());
        assert!(check_table("\tA\tB\nx\t1,000\t2").is_ok());
        let e = check_table(", Sales\nJan, 1\nFeb, lots").unwrap_err();
        assert!(e.contains("Line 3, column 2") && e.contains("lots"), "{e}");
        assert!(check_table("1\ntwo").is_err());
        // the header row is not checked; the categories are not numbers
        assert!(check_table("Month, Sales\nJan, 5").is_ok());
    }
}
