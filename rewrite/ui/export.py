"""The Export / convert dialog: the open document written as another format."""
import os
import threading
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

from .. import docmodel, exporting, models, native
from ..lane import Lane
from .dialogs import Dialog
from .widgets import FlatButton


class ExportDialog(Dialog):
    def __init__(self, app, source):
        super().__init__(app, "Export / convert", 470)
        self.source = source
        t, c = self.t, self.t.c
        s = app.settings
        self.label(self.body, "Export / convert", t.f_title).pack(fill="x")
        what = {"rich": "formatted document", "pdf": "PDF", "markdown": "Markdown", "latex": "LaTeX",
                "html": "HTML", "code": "code", "plain": "text"}.get(source.kind, source.kind)
        self.label(self.body, f"{source.name}  ·  {what}", fg="muted").pack(fill="x", pady=(t.px(2), t.px(10)))

        self.label(self.body, "SAVE AS", t.f_tiny, fg="faint").pack(fill="x")
        grid = tk.Frame(self.body, bg=c["surface"])
        grid.pack(fill="x", pady=(t.px(2), t.px(8)))
        default = s.get("export_target", ".pdf" if source.kind != "pdf" else ".docx")
        self.target = tk.StringVar(value=default)
        targets = [x for x in exporting.TARGETS if not (x[0] == ".pdf" and source.kind == "pdf")]
        for k, (ext, label) in enumerate(targets):
            rb = ttk.Radiobutton(grid, text=f"{label}  ({ext})", value=ext, variable=self.target, command=self._update)
            rb.grid(row=k // 2, column=k % 2, sticky="w", padx=(0, t.px(16)), pady=1)
            if ext == ".doc" and not native.office_path():
                rb.state(["disabled"])
        if source.kind == "pdf" and default == ".pdf":
            self.target.set(".docx")

        self.opts = tk.Frame(self.body, bg=c["surface"])
        self.opts.pack(fill="x")
        page = exporting.default_page(source)
        self.paper = tk.StringVar(value=next((n for n, (w, h) in docmodel.PAGE_SIZES.items()
                                              if {round(w), round(h)} == {round(page.width), round(page.height)}),
                                             s.get("export_paper", "A4")))
        self.numbers = tk.BooleanVar(value=bool(page.numbers))
        self.generic = tk.StringVar(value=s.get("export_font", "serif"))
        self.engine = tk.StringVar(value="")
        self.sentences = tk.BooleanVar(value=bool(s.get("export_sentences", False)))
        self.fix_words = tk.BooleanVar(value=True)
        self.drop_headers = tk.BooleanVar(value=True)
        self.images = tk.BooleanVar(value=True)
        self.tables = tk.BooleanVar(value=True)

        self.status = self.label(self.body, "", t.f_small, fg="muted")
        self.status.pack(fill="x", pady=(t.px(8), 0))
        self.bar = ttk.Progressbar(self.body, mode="determinate", maximum=1000)
        b = self.buttons()
        self.go = FlatButton(b, t, "Export…", self.run, kind="primary", bg="surface")
        self.go.pack(side="right")
        self.cancel_btn = FlatButton(b, t, "Cancel", self.stop, bg="surface")
        self.cancel_btn.pack(side="left")
        self.cancel = None
        self.lane = Lane(app, "export")
        self._update()
        self.bind("<Return>", lambda e: self.run())
        self.show()

    # -- options that depend on the choice -----------------------------------------------------
    def _update(self):
        for w in self.opts.winfo_children():
            w.destroy()
        t, c = self.t, self.t.c
        target, src = self.target.get(), self.source

        def head(text):
            tk.Label(self.opts, text=text, font=t.f_tiny, bg=c["surface"], fg=c["faint"], anchor="w").pack(
                fill="x", pady=(t.px(6), t.px(2)))

        def check(text, var, tip=None):
            ttk.Checkbutton(self.opts, text=text, variable=var).pack(anchor="w")
            if tip:
                tk.Label(self.opts, text=tip, font=t.f_small, bg=c["surface"], fg=c["muted"], anchor="w", justify="left",
                         wraplength=self.width - t.px(24)).pack(fill="x", padx=(t.px(22), 0))

        if target == ".pdf":
            engines = exporting.engines(src, target)
            if engines:
                head("TYPESET WITH")
                keys = [k for k, _ in engines]
                if self.engine.get() not in keys:
                    self.engine.set(keys[0])
                row = tk.Frame(self.opts, bg=c["surface"])
                row.pack(fill="x")
                for key, label in engines:
                    ttk.Radiobutton(row, text=label, value=key, variable=self.engine, command=self._update).pack(
                        side="left", padx=(0, t.px(12)))
            if self.engine.get() == "rewrite":
                head("PAGE")
                row = tk.Frame(self.opts, bg=c["surface"])
                row.pack(fill="x")
                ttk.Combobox(row, textvariable=self.paper, values=list(docmodel.PAGE_SIZES), state="readonly",
                             width=8).pack(side="left")
                ttk.Radiobutton(row, text="Serif", value="serif", variable=self.generic).pack(side="left", padx=(t.px(12), 0))
                ttk.Radiobutton(row, text="Sans serif", value="sans", variable=self.generic).pack(side="left", padx=(t.px(6), 0))
                ttk.Checkbutton(row, text="Page numbers", variable=self.numbers).pack(side="left", padx=(t.px(12), 0))
        if target in (".md", ".tex", ".txt"):
            head("TEXT")
            check("One sentence per line", self.sentences,
                  "Sentence ends are found by the language model, so “e.g.” or “Fig. 3” do not split a sentence. "
                  "Markdown and LaTeX show the text the same; it is easier to compare and edit line by line.")
        if src.kind == "pdf" and target != ".pdf":
            head("READING THE PDF")
            check("Mend broken and run-together words", self.fix_words,
                  "Words hyphenated at line ends are joined (or kept as compounds), and words stuck together are "
                  "split, with the dictionary and the language model deciding the doubtful cases.")
            check("Leave out running headers, footers and page numbers", self.drop_headers)
            check("Keep pictures, charts and formulas (as pictures)", self.images)
            check("Find tables", self.tables)
        self.update_idletasks()

    # -- running -------------------------------------------------------------------------------------
    def run(self):
        if self.cancel is not None:
            return
        target = self.target.get()
        src = self.source
        label = dict(exporting.TARGETS).get(target, target)
        start_dir = os.path.dirname(src.path) if src.path else self.app.settings.get("last_dir")
        path = filedialog.asksaveasfilename(parent=self, title=f"Export as {label}", defaultextension=target,
                                            initialfile=src.stem + target, initialdir=start_dir,
                                            filetypes=[(label, "*" + target), ("All files", "*.*")])
        if not path:
            return
        if not path.lower().endswith(target):
            path += target
        if src.path and os.path.abspath(path) == os.path.abspath(src.path):
            messagebox.showerror("Export", "Choose another name: this is the file that is open.", parent=self)
            return
        page = None
        if target == ".pdf" and self.engine.get() == "rewrite":
            w, h = docmodel.PAGE_SIZES.get(self.paper.get(), docmodel.PAGE_SIZES["A4"])
            base = exporting.default_page(src)
            page = docmodel.PageSetup(width=w, height=h, margins=base.margins, numbers=bool(self.numbers.get()))
        opts = {"engine": self.engine.get() or None, "page": page, "numbers": bool(self.numbers.get()),
                "generic": self.generic.get(), "sentences": bool(self.sentences.get()),
                "fix_words": bool(self.fix_words.get()), "drop_headers": bool(self.drop_headers.get()),
                "images": bool(self.images.get()), "tables": bool(self.tables.get())}
        s = self.app.settings
        s["export_target"], s["export_paper"], s["export_font"] = target, self.paper.get(), self.generic.get()
        s["export_sentences"] = bool(self.sentences.get())
        models.save_settings(s)
        helper = None
        if opts["sentences"] or (src.kind == "pdf" and opts["fix_words"]):
            from ..cleanup import Helper
            helper = Helper(self.app.engine)
        cancel = self.cancel = threading.Event()
        self.go.set_enabled(False)
        self.bar.pack(fill="x", pady=(self.t.px(4), 0), before=self.status)
        self.bar.configure(mode="indeterminate")
        self.bar.start(15)
        self.status.configure(text="Working…")

        def progress(k, n):
            def show():
                if self.cancel is cancel and self.winfo_exists():
                    self.bar.stop()
                    self.bar.configure(mode="determinate", value=int(1000 * k / max(1, n)))
                    self.status.configure(text=f"Reading page {k + 1} of {n}…")
            self.app.post(show)

        def job():
            return exporting.export(src, target, path, opts, helper, progress, cancel)

        def done(written, err):
            if self.cancel is not cancel:
                return
            self.cancel = None
            if not self.winfo_exists():
                return
            self.bar.stop()
            self.bar.pack_forget()
            self.go.set_enabled(True)
            if err:
                if cancel.is_set():
                    self.status.configure(text="Stopped.")
                    return
                self.status.configure(text="")
                messagebox.showerror("Export", f"The export did not work.\n\n{err}", parent=self)
                return
            extra = len(written) - 1
            more = f" and {extra} picture{'s' if extra != 1 else ''}" if extra > 0 else ""
            self.app.flash_msg(f"Exported {os.path.basename(path)}{more}.", 9000)
            self.close()

        self.lane.submit(job, done)

    def stop(self):
        if self.cancel is not None:
            self.cancel.set()
            self.status.configure(text="Stopping…")
            return
        self.close()

    def close(self):
        if self.cancel is not None:
            self.cancel.set()
        super().close()
