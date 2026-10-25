//! The Export / convert dialog: the open document written as another format.

use crate::app::{App, AppRc};
use crate::dialogs::Dialog;
use crate::widgets;
use crate::workers::{self, Cancel, Lane};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use rewrite_core::export::{self, Options, PdfReader, Source, Tools, CANCELLED, TARGETS};
use rewrite_core::model::{Document, PageSetup, PAGE_SIZES};
use rewrite_core::office;
use rewrite_core::pdfextract;
use rewrite_core::textclean::{Helper, NoHelper};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};

/// PDF files read with PDFium.
struct Pdfium;

impl PdfReader for Pdfium {
    fn to_document(
        &self,
        data: &[u8],
        opts: &pdfextract::Options,
        helper: Option<&dyn Helper>,
        progress: Option<&dyn Fn(usize, usize)>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Document, String> {
        // PDFium looks at a flag before each page; it is raised from the progress report
        let flag = AtomicBool::new(cancel());
        let report = |k: usize, n: usize| {
            if cancel() {
                flag.store(true, Ordering::SeqCst);
            }
            if let Some(p) = progress {
                p(k, n);
            }
        };
        let none = NoHelper;
        rewrite_pdfium::to_document(data, opts, helper.unwrap_or(&none), Some(&report), Some(&flag)).map_err(|e| match e {
            rewrite_pdfium::Error::Cancelled => CANCELLED.to_string(),
            e => e.to_string(),
        })
    }

    fn first_page_png(&self, data: &[u8]) -> Option<Vec<u8>> {
        rewrite_pdfium::first_page_png(data, 2.0)
    }
}

thread_local! {
    /// Exports run here, one at a time.
    static LANE: Lane = Lane::new("export");
    /// The export dialog that is open, for the progress reports of its export.
    static OPEN: RefCell<Weak<ExportDialog>> = RefCell::new(Weak::new());
}

struct ExportDialog {
    app: Weak<App>,
    d: Dialog,
    source: Source,
    opts: gtk::Box,
    status: gtk::Label,
    bar: gtk::ProgressBar,
    go: gtk::Button,
    target: RefCell<String>,
    paper: RefCell<String>,
    numbers: Cell<bool>,
    generic: RefCell<String>,
    engine: RefCell<String>,
    sentences: Cell<bool>,
    fix_words: Cell<bool>,
    drop_headers: Cell<bool>,
    images: Cell<bool>,
    tables: Cell<bool>,
    cancel: RefCell<Option<Cancel>>,
    pulse: RefCell<Option<glib::SourceId>>,
    closed: Cell<bool>,
}

fn with_open(f: impl FnOnce(&Rc<ExportDialog>)) {
    if let Some(d) = OPEN.with(|o| o.borrow().upgrade()) {
        if !d.closed.get() {
            f(&d);
        }
    }
}

pub fn show(app: &AppRc, source: Source) {
    let d = Dialog::new(app, "Export / convert", 470);
    let body = d.body.clone();
    let s = app.settings().clone();
    d.label(&body, "Export / convert", &["title"]);
    let what = match source.kind.as_str() {
        "rich" => "formatted document",
        "pdf" => "PDF",
        "markdown" => "Markdown",
        "latex" => "LaTeX",
        "html" => "HTML",
        "code" => "code",
        "plain" => "text",
        k => k,
    };
    let l = d.label(&body, &format!("{}  ·  {what}", source.name), &["muted"]);
    l.set_margin_top(2);
    l.set_margin_bottom(10);

    d.caption(&body, "SAVE AS");
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(1);
    grid.set_margin_top(2);
    grid.set_margin_bottom(8);
    body.append(&grid);
    let is_pdf = source.kind == "pdf";
    let targets: Vec<(&str, &str)> = TARGETS.iter().copied().filter(|(ext, _)| !(*ext == ".pdf" && is_pdf)).collect();
    let mut default = s.export_target.clone().unwrap_or_else(|| if is_pdf { ".docx".into() } else { ".pdf".into() });
    // .doc needs LibreOffice: without it that choice is greyed out, so it is not the one picked
    let office = office::available();
    if (is_pdf && default == ".pdf") || (default == ".doc" && !office) || !targets.iter().any(|(e, _)| *e == default) {
        default = if is_pdf { ".docx".into() } else { ".pdf".into() };
    }

    let opts = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&opts);
    let page = export::default_page(&source);
    let paper = PAGE_SIZES
        .iter()
        .find(|(_, w, h)| {
            let mut a = [w.round() as i64, h.round() as i64];
            let mut b = [page.width.round() as i64, page.height.round() as i64];
            a.sort();
            b.sort();
            a == b
        })
        .map(|(n, _, _)| n.to_string())
        .unwrap_or_else(|| s.export_paper.clone().unwrap_or_else(|| "A4".into()));
    let bar = gtk::ProgressBar::new();
    bar.set_margin_top(4);
    bar.set_visible(false);
    body.append(&bar);
    let status = d.label(&body, "", &["small", "muted"]);
    status.set_margin_top(8);
    let (left, right) = d.buttons();
    let go = widgets::flat_button("Export…", "primary", None, || with_open(|d| d.run()));
    right.append(&go);
    left.append(&widgets::flat_button("Cancel", "ghost", None, || with_open(|d| d.stop())));

    let dlg = Rc::new(ExportDialog {
        app: app.weak(),
        d,
        source,
        opts,
        status,
        bar,
        go,
        target: RefCell::new(default.clone()),
        paper: RefCell::new(paper),
        numbers: Cell::new(page.numbers),
        generic: RefCell::new(s.export_font.clone().unwrap_or_else(|| "serif".into())),
        engine: RefCell::new(String::new()),
        sentences: Cell::new(s.export_sentences.unwrap_or(false)),
        fix_words: Cell::new(true),
        drop_headers: Cell::new(true),
        images: Cell::new(true),
        tables: Cell::new(true),
        cancel: RefCell::new(None),
        pulse: RefCell::new(None),
        closed: Cell::new(false),
    });
    OPEN.with(|o| *o.borrow_mut() = Rc::downgrade(&dlg));
    let mut group: Option<gtk::CheckButton> = None;
    for (k, (ext, label)) in targets.iter().enumerate() {
        let rb = gtk::CheckButton::with_label(&format!("{label}  ({ext})"));
        match &group {
            Some(g) => rb.set_group(Some(g)),
            None => group = Some(rb.clone()),
        }
        rb.set_active(*ext == default);
        if *ext == ".doc" && !office {
            rb.set_sensitive(false);
        }
        let ext = ext.to_string();
        rb.connect_toggled(move |b| {
            if b.is_active() {
                let ext = ext.clone();
                with_open(move |d| {
                    *d.target.borrow_mut() = ext;
                    d.update();
                });
            }
        });
        grid.attach(&rb, (k % 2) as i32, (k / 2) as i32, 1, 1);
    }
    dlg.d.keys(|| with_open(|d| d.close()), || with_open(|d| d.run()));
    let keep = dlg.clone();
    dlg.d.win.connect_destroy(move |_| {
        keep.closed.set(true);
        keep.stop_pulse();
    });
    dlg.update();
    dlg.d.show();
}

impl ExportDialog {
    // -- options that depend on the choice ----------------------------------------------------

    fn update(&self) {
        widgets::clear(&self.opts);
        let target = self.target.borrow().clone();
        let src = &self.source;
        let head = |text: &str| {
            let l = widgets::label(text, &["caption"]);
            l.set_margin_top(6);
            l.set_margin_bottom(2);
            self.opts.append(&l);
        };
        let check = |text: &str, cell: fn(&ExportDialog) -> &Cell<bool>, tip: Option<&str>| {
            let cb = gtk::CheckButton::with_label(text);
            cb.set_active(cell(self).get());
            cb.connect_toggled(move |b| {
                let on = b.is_active();
                with_open(|d| cell(d).set(on));
            });
            self.opts.append(&cb);
            if let Some(tip) = tip {
                let l = self.d.label(&self.opts, tip, &["small", "muted"]);
                l.set_margin_start(22);
            }
        };

        if target == ".pdf" {
            let engines = export::engines(src, &target);
            if !engines.is_empty() {
                head("TYPESET WITH");
                if !engines.iter().any(|(k, _)| *k == self.engine.borrow().as_str()) {
                    *self.engine.borrow_mut() = engines[0].0.to_string();
                }
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                // a hidden first member keeps a lone choice drawn as a radio button
                let lead = gtk::CheckButton::new();
                lead.set_visible(false);
                row.append(&lead);
                let mut group: Option<gtk::CheckButton> = Some(lead);
                for (key, label) in &engines {
                    let rb = gtk::CheckButton::with_label(label);
                    match &group {
                        Some(g) => rb.set_group(Some(g)),
                        None => group = Some(rb.clone()),
                    }
                    rb.set_active(*key == self.engine.borrow().as_str());
                    let key = key.to_string();
                    rb.connect_toggled(move |b| {
                        if b.is_active() {
                            let key = key.clone();
                            // rebuilt after the click has been handled
                            glib::idle_add_local_once(move || {
                                with_open(move |d| {
                                    *d.engine.borrow_mut() = key;
                                    d.update();
                                })
                            });
                        }
                    });
                    row.append(&rb);
                }
                self.opts.append(&row);
            }
            if self.engine.borrow().as_str() == "rewrite" {
                head("PAGE");
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                let names: Vec<&str> = PAGE_SIZES.iter().map(|(n, _, _)| *n).collect();
                let dd = gtk::DropDown::from_strings(&names);
                let cur = self.paper.borrow().clone();
                dd.set_selected(names.iter().position(|n| *n == cur).unwrap_or(0) as u32);
                dd.connect_selected_notify(move |dd| {
                    let k = dd.selected() as usize;
                    if let Some(&(n, _, _)) = PAGE_SIZES.get(k) {
                        with_open(|d| *d.paper.borrow_mut() = n.to_string());
                    }
                });
                row.append(&dd);
                let serif = gtk::CheckButton::with_label("Serif");
                let sans = gtk::CheckButton::with_label("Sans serif");
                sans.set_group(Some(&serif));
                if self.generic.borrow().as_str() == "sans" { sans.set_active(true) } else { serif.set_active(true) };
                serif.set_margin_start(12);
                sans.set_margin_start(6);
                for (b, v) in [(&serif, "serif"), (&sans, "sans")] {
                    b.connect_toggled(move |b| {
                        if b.is_active() {
                            with_open(|d| *d.generic.borrow_mut() = v.to_string());
                        }
                    });
                    row.append(b);
                }
                let nums = gtk::CheckButton::with_label("Page numbers");
                nums.set_active(self.numbers.get());
                nums.set_margin_start(12);
                nums.connect_toggled(|b| {
                    let on = b.is_active();
                    with_open(|d| d.numbers.set(on));
                });
                row.append(&nums);
                self.opts.append(&row);
            }
        }
        if matches!(target.as_str(), ".md" | ".tex" | ".txt") {
            head("TEXT");
            check(
                "One sentence per line",
                |d| &d.sentences,
                Some(
                    "Sentence ends are found by the language model, so “e.g.” or “Fig. 3” do not split a sentence. Markdown and \
                     LaTeX show the text the same; it is easier to compare and edit line by line.",
                ),
            );
        }
        if src.kind == "pdf" && target != ".pdf" {
            head("READING THE PDF");
            check(
                "Mend broken and run-together words",
                |d| &d.fix_words,
                Some(
                    "Words hyphenated at line ends are joined (or kept as compounds), and words stuck together are split, with \
                     the dictionary and the language model deciding the doubtful cases.",
                ),
            );
            check("Leave out running headers, footers and page numbers", |d| &d.drop_headers, None);
            check("Keep pictures, charts and formulas (as pictures)", |d| &d.images, None);
            check("Find tables", |d| &d.tables, None);
        }
    }

    // -- running -------------------------------------------------------------------------------

    fn run(&self) {
        if self.cancel.borrow().is_some() {
            return;
        }
        let Some(app) = self.app.upgrade() else { return };
        let target = self.target.borrow().clone();
        let label = TARGETS.iter().find(|(e, _)| *e == target).map_or(target.clone(), |(_, l)| l.to_string());
        let start_dir = self
            .source
            .path
            .as_ref()
            .and_then(|p| std::path::absolute(p).ok()?.parent().map(Path::to_path_buf))
            .or_else(|| app.settings().last_dir.clone().map(PathBuf::from));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&crate::app::filter(&label, &[&format!("*{target}")]));
        filters.append(&crate::app::filter("All files", &["*"]));
        let dialog = gtk::FileDialog::builder()
            .title(format!("Export as {label}"))
            .modal(true)
            .initial_name(format!("{}{target}", self.source.stem()))
            .filters(&filters)
            .build();
        if let Some(dir) = start_dir {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let win = self.d.win.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.save_future(Some(&win)).await else { return };
            let Some(path) = file.path() else { return };
            with_open(|d| d.start(path, target));
        });
    }

    fn start(&self, mut path: PathBuf, target: String) {
        if self.cancel.borrow().is_some() {
            return;
        }
        let Some(app) = self.app.upgrade() else { return };
        if !path.to_string_lossy().to_lowercase().ends_with(&target) {
            let mut s = path.into_os_string();
            s.push(&target);
            path = PathBuf::from(s);
        }
        let src = self.source.clone();
        if let Some(open) = &src.path {
            if std::path::absolute(&path).ok() == std::path::absolute(open).ok() {
                self.alert("Choose another name: this is the file that is open.");
                return;
            }
        }
        let engine = self.engine.borrow().clone();
        let page = if target == ".pdf" && engine == "rewrite" {
            let paper = self.paper.borrow().clone();
            let (_, w, h) = PAGE_SIZES.iter().copied().find(|(n, _, _)| *n == paper).unwrap_or(PAGE_SIZES[0]);
            let base = export::default_page(&src);
            Some(PageSetup { width: w, height: h, margins: base.margins, numbers: self.numbers.get() })
        } else {
            None
        };
        let opts = Options {
            engine: if engine.is_empty() { None } else { Some(engine) },
            page,
            numbers: Some(self.numbers.get()),
            generic: self.generic.borrow().clone(),
            sentences: self.sentences.get(),
            fix_words: self.fix_words.get(),
            drop_headers: self.drop_headers.get(),
            images: self.images.get(),
            tables: self.tables.get(),
        };
        {
            let mut s = app.settings_mut();
            s.export_target = Some(target.clone());
            s.export_paper = Some(self.paper.borrow().clone());
            s.export_font = Some(self.generic.borrow().clone());
            s.export_sentences = Some(opts.sentences);
        }
        app.save_settings();
        let helper = (opts.sentences || (src.kind == "pdf" && opts.fix_words)).then(|| app.engine.helper());
        let flag = Cancel::new();
        *self.cancel.borrow_mut() = Some(flag.clone());
        self.go.set_sensitive(false);
        self.bar.set_visible(true);
        self.bar.set_fraction(0.0);
        self.bar.set_pulse_step(0.04);
        let bar = self.bar.clone();
        *self.pulse.borrow_mut() = Some(glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
            bar.pulse();
            glib::ControlFlow::Continue
        }));
        self.status.set_label("Working…");

        let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let job_flag = flag.clone();
        let job = move || {
            let f = job_flag.clone();
            let progress = move |k: usize, n: usize| {
                let f = f.clone();
                workers::post(move || with_open(|d| d.progress(k, n, &f)));
            };
            let cancelled = || job_flag.is_set();
            let tools = Tools {
                helper: helper.as_ref().map(|h| h as &dyn Helper),
                pdf: Some(&Pdfium),
                progress: Some(&progress),
                cancel: &cancelled,
            };
            export::export(&src, &target, &path, &opts, &tools)
        };
        let me = OPEN.with(|o| o.borrow().clone());
        let done = move |res: Result<Result<Vec<PathBuf>, String>, String>| {
            let Some(d) = me.upgrade() else { return };
            if !d.cancel.borrow().as_ref().is_some_and(|c| c.same(&flag)) {
                return;
            }
            *d.cancel.borrow_mut() = None;
            if d.closed.get() {
                return;
            }
            d.stop_pulse();
            d.bar.set_visible(false);
            d.go.set_sensitive(true);
            match res {
                Ok(Ok(written)) => {
                    let extra = written.len().saturating_sub(1);
                    let more = if extra > 0 { format!(" and {extra} picture{}", if extra != 1 { "s" } else { "" }) } else { String::new() };
                    if let Some(app) = d.app.upgrade() {
                        app.flash_msg(&format!("Exported {name}{more}."), 9000);
                    }
                    d.close();
                }
                Ok(Err(err)) | Err(err) => {
                    if flag.is_set() {
                        d.status.set_label("Stopped.");
                        return;
                    }
                    d.status.set_label("");
                    d.alert(&format!("The export did not work.\n\n{err}"));
                }
            }
        };
        LANE.with(|l| l.submit(job, done));
    }

    fn progress(&self, k: usize, n: usize, flag: &Cancel) {
        if !self.cancel.borrow().as_ref().is_some_and(|c| c.same(flag)) {
            return;
        }
        self.stop_pulse();
        self.bar.set_fraction(k as f64 / n.max(1) as f64);
        self.status.set_label(&format!("Reading page {} of {n}…", k + 1));
    }

    fn stop_pulse(&self) {
        if let Some(id) = self.pulse.borrow_mut().take() {
            id.remove();
        }
    }

    fn alert(&self, detail: &str) {
        crate::dialogs::message(&self.d.win, "Export", detail, &["OK"], 0, 0, |_| {});
    }

    fn stop(&self) {
        if let Some(c) = self.cancel.borrow().as_ref() {
            c.set();
            self.status.set_label("Stopping…");
            return;
        }
        self.close();
    }

    fn close(&self) {
        if let Some(c) = self.cancel.borrow().as_ref() {
            c.set();
        }
        self.closed.set(true);
        self.stop_pulse();
        self.d.close();
    }
}
