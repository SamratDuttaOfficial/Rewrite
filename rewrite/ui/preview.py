"""Preview panel: rendered Markdown, or the compiled PDF of a LaTeX document."""
import math
import os
import threading
import tkinter as tk
import webbrowser
from tkinter import font as tkfont
from tkinter import ttk

from .. import latexbuild, mdrender, native, pdfcore
from ..lane import Lane
from .widgets import IconButton

HEADING_SCALE = {1: 1.9, 2: 1.5, 3: 1.25, 4: 1.1, 5: 1.0, 6: 0.92}


class MarkdownView(tk.Frame):
    """Markdown drawn in a read-only text widget."""

    def __init__(self, parent, app):
        super().__init__(parent, bd=0, highlightthickness=0)
        self.app, self.t = app, app.theme
        t = self.t
        self.text = tk.Text(self, wrap="word", bd=0, highlightthickness=0, relief="flat", width=1, height=1,
                            padx=t.px(18), pady=t.px(12), cursor="arrow", insertwidth=0, takefocus=0)
        self.vsb = ttk.Scrollbar(self, orient="vertical", command=self.text.yview)
        self.text.configure(yscrollcommand=self.vsb.set, state="disabled")
        self.vsb.pack(side="right", fill="y")
        self.text.pack(side="left", fill="both", expand=True)
        self.fonts, self.images, self.links, self._font_key = {}, [], 0, None
        self._hair = tkfont.Font(self.text, size=1)  # a line this small is drawn as a thin rule
        self._gap = tkfont.Font(self.text, size=4)

    def _font(self, kind="body", scale=1.0, bold=False, italic=False):
        key = (kind, scale, bold, italic)
        if key not in self.fonts:
            fam = self.t.families["mono"] if kind == "mono" else self.t.families[self.t.editor_family]
            size = max(6, int(round(self.t.size * scale * (0.92 if kind == "mono" else 1.0))))
            self.fonts[key] = tkfont.Font(self.text, family=fam, size=size, weight="bold" if bold else "normal",
                                          slant="italic" if italic else "roman")
        return self.fonts[key]

    def _style(self):
        """(Re)define the tags for the current theme and text size."""
        t, c, x = self.t, self.t.c, self.text
        if self._font_key != (t.editor_family, t.size):
            self.fonts.clear()
            self._font_key = (t.editor_family, t.size)
        x.configure(bg=c["surface"], fg=c["text"], font=self._font(), selectbackground=c["select"],
                    selectforeground=c["text"], inactiveselectbackground=c["select"],
                    spacing1=0, spacing2=t.px(2), spacing3=0)
        x.tag_configure("p", spacing3=t.px(round(t.size * 1.2)))  # grows with the text size
        for lvl, sc in HEADING_SCALE.items():
            x.tag_configure(f"h{lvl}", font=self._font(scale=sc, bold=True), spacing1=t.px(10 if lvl < 3 else 6),
                            spacing3=t.px(5), foreground=c["text"] if lvl < 6 else c["muted"])
        for d in range(8):
            left = t.px(8 + 22 * d)
            x.tag_configure(f"li{d}", lmargin1=left, lmargin2=left + t.px(20), spacing3=t.px(2),
                            tabs=(left + t.px(20),))
            x.tag_configure(f"q{d + 1}", lmargin1=t.px(18 * (d + 1)), lmargin2=t.px(18 * (d + 1)),
                            foreground=c["muted"], spacing3=t.px(5))
        x.tag_configure("pre", font=self._font("mono"), background=c["hover"], lmargin1=t.px(12),
                        lmargin2=t.px(12), rmargin=t.px(12), wrap="none", spacing2=0)
        x.tag_configure("rule", font=self._hair, background=c["border"])
        self._gap.configure(size=max(2, t.size // 3))
        x.tag_configure("gap", font=self._gap)
        x.tag_configure("th", font=self._font(bold=True))
        x.tag_configure("b", font=self._font(bold=True))
        x.tag_configure("i", font=self._font(italic=True))
        x.tag_configure("bi", font=self._font(bold=True, italic=True))
        x.tag_configure("code", font=self._font("mono"), background=c["hover"])
        x.tag_configure("math", font=self._font("mono", italic=True), foreground=c["muted"])
        x.tag_configure("sup", font=self._font(scale=0.75), offset=t.px(4), foreground=c["accent"])
        x.tag_configure("strike", overstrike=True, foreground=c["muted"])
        x.tag_configure("link", foreground=c["accent"], underline=True)
        x.tag_configure("muted", foreground=c["faint"])
        try:
            x.tag_configure("pre", lmargincolor=c["hover"], rmargincolor=c["hover"])
        except tk.TclError:  # older Tk
            pass

    def render(self, source, base_dir=None):
        x = self.text
        key = (source, base_dir, self.t.name, self.t.editor_family, self.t.size)
        if key == getattr(self, "_shown", None):
            return
        self._shown = key
        top = x.yview()[0]
        x.configure(state="normal")
        x.delete("1.0", "end")
        for tag in x.tag_names():
            if tag.startswith(("a:", "tbl")):
                x.tag_delete(tag)
        self.images, self.links, self.base_dir = [], 0, base_dir
        self._style()
        blocks = mdrender.parse(source)
        prev = None
        for n, b in enumerate(blocks):
            if prev in ("item", "quote") and b[0] != prev:  # lists and quotes are set tight; open up after them
                x.insert("end", "\n", ("gap",))
            self._block(n, b)
            prev = b[0]
        if not blocks:
            x.insert("end", "Nothing to preview yet.", ("muted",))
        x.configure(state="disabled")
        x.yview_moveto(top)

    def _runs(self, runs, base, plain=False):
        x = self.text
        for text, styles, link in runs:
            if "img" in styles:
                self._image(*link, base)
                continue
            tags = [base]
            if not plain:
                font = "code" if "code" in styles else "math" if "math" in styles else \
                    "bi" if {"b", "i"} <= styles else "b" if "b" in styles else "i" if "i" in styles else None
                if font:
                    tags.append(font)
            elif "code" in styles:
                tags.append("code")
            tags += [s for s in ("strike", "sup") if s in styles]
            if link:
                self.links += 1
                name = f"a:{self.links}"
                x.tag_bind(name, "<Button-1>", lambda e, url=link: self._open(url))
                x.tag_bind(name, "<Enter>", lambda e: x.configure(cursor="hand2"))
                x.tag_bind(name, "<Leave>", lambda e: x.configure(cursor="arrow"))
                tags += ["link", name]
            x.insert("end", text, tuple(tags))

    def _block(self, n, b):
        x, kind = self.text, b[0]
        if kind == "heading":
            self._runs(b[2], f"h{b[1]}", plain=True)
            x.insert("end", "\n", (f"h{b[1]}",))
        elif kind == "para":
            self._runs(b[1], "p")
            x.insert("end", "\n", ("p",))
        elif kind == "item":
            _, depth, marker, checked, runs = b
            tag = f"li{min(depth, 7)}"
            bullet = marker if checked is None else ("☑" if checked else "☐")
            x.insert("end", bullet + "\t", (tag, "muted") if checked is None else (tag,))
            self._runs(runs, tag)
            x.insert("end", "\n", (tag,))
        elif kind == "quote":
            tag = f"q{min(b[1], 8)}"
            self._runs(b[2], tag)
            x.insert("end", "\n", (tag,))
        elif kind in ("code", "math"):
            x.insert("end", b[-1] + "\n", ("pre",))
            x.insert("end", "\n", ("gap",))
        elif kind == "rule":
            x.insert("end", "\n", ("gap",))
            x.insert("end", "\n", ("rule",))
            x.insert("end", "\n", ("gap",))
        elif kind == "table":
            self._table(n, b[1], b[2], b[3])

    def _table(self, n, head, align, rows):
        x, t = self.text, self.t
        body, bold = self._font(), self._font(bold=True)
        cols = max([len(head)] + [len(r) for r in rows])
        width = [0] * cols
        for font, row in [(bold, head)] + [(body, r) for r in rows]:
            for j, cell in enumerate(row):
                width[j] = max(width[j], font.measure("".join(r[0] for r in cell)))
        tabs, pos = [], t.px(4)
        for j in range(cols):
            a = align[j] if j < len(align) else "left"
            tabs += [pos + (width[j] if a == "right" else width[j] // 2 if a == "center" else 0), a]
            pos += width[j] + t.px(26)
        tag = f"tbl{n}"
        x.tag_configure(tag, tabs=tuple(tabs), wrap="none", spacing3=t.px(3))
        for k, row in enumerate([head] + rows):
            for cell in row:
                x.insert("end", "\t", (tag,))
                self._runs(cell, tag, plain=k == 0)
            x.insert("end", "\n", (tag,))
            if k == 0:
                x.tag_add("th", "end-2l linestart", "end-1c")
                x.insert("end", "\n", ("rule",))
        x.insert("end", "\n", ("gap",))

    def _image(self, alt, src, base):
        path = src if os.path.isabs(src) else os.path.join(self.base_dir or "", src)
        img = None
        if "://" not in src and os.path.isfile(path) and path.lower().endswith((".png", ".gif")):
            try:
                img = tk.PhotoImage(file=path)
                room = max(120, self.text.winfo_width() - 2 * self.t.px(30))
                if img.width() > room:
                    img = img.subsample(math.ceil(img.width() / room))
            except tk.TclError:
                img = None
        if img is None:
            self.text.insert("end", f"[image: {alt or os.path.basename(src)}]", (base, "muted"))
        else:
            self.images.append(img)  # keep a reference, or Tk drops the picture
            self.text.image_create("end", image=img)

    def _open(self, url):
        if url.startswith(("http://", "https://", "mailto:")):
            webbrowser.open(url)
        else:
            self.app.flash_msg(f"Link: {url}")


class PdfView(tk.Frame):
    """A PDF shown as a column of pages. Pages are drawn as they scroll into view."""

    def __init__(self, parent, app):
        super().__init__(parent, bd=0, highlightthickness=0)
        self.app, self.t = app, app.theme
        self.canvas = tk.Canvas(self, bd=0, highlightthickness=0, takefocus=0)
        self.vsb = ttk.Scrollbar(self, orient="vertical", command=self._yview)
        self.hsb = ttk.Scrollbar(self, orient="horizontal", command=self.canvas.xview)
        self.canvas.configure(yscrollcommand=self.vsb.set, xscrollcommand=self._xset)
        self.vsb.pack(side="right", fill="y")
        self.canvas.pack(side="top", fill="both", expand=True)
        self.lane = Lane(app, "pdf")  # every PDFium call happens on this one thread
        self.data, self.version, self.sizes, self.zoom = None, 0, [], 1.0
        self.boxes = []  # page rectangles on the canvas: (x, y, width, height)
        self.images = {}  # page -> (version, width, PhotoImage, canvas item)
        self._doc, self._doc_version = None, -1  # used by the worker thread only
        self._hbar = False
        self.canvas.bind("<Configure>", lambda e: self.app.later("pdf-layout", 120, self._layout))
        self.canvas.bind("<MouseWheel>", self._wheel)
        self.canvas.bind("<Button-4>", self._wheel)
        self.canvas.bind("<Button-5>", self._wheel)
        self.recolor()

    def recolor(self):
        self.canvas.configure(bg=self.t.c["bg"])

    def _xset(self, a, b):
        self.hsb.set(a, b)
        need = float(a) > 0.0 or float(b) < 1.0
        if need != self._hbar:
            self._hbar = need
            if need:
                self.hsb.pack(side="bottom", fill="x", before=self.canvas)
            else:
                self.hsb.pack_forget()

    def _yview(self, *args):
        self.canvas.yview(*args)
        self._request()

    def _wheel(self, e):
        if getattr(e, "num", None) in (4, 5):
            step = -1 if e.num == 4 else 1
        else:
            step = -1 if e.delta > 0 else 1
        if e.state & 0x4:  # Ctrl + wheel zooms
            self.set_zoom(self.zoom * (1.1 if step < 0 else 1 / 1.1))
        else:
            self.canvas.yview_scroll(step * 3, "units")
            self._request()
        return "break"

    def set_zoom(self, z):
        z = max(0.4, min(4.0, z))
        if abs(z - self.zoom) > 1e-3:
            self.zoom = z
            self._layout()

    def show(self, data):
        """Display a new PDF (bytes). The old pages stay up until the new ones are drawn."""
        self.data = data
        self.version += 1
        self._submit([])

    def clear(self):
        self.data, self.sizes, self.boxes = None, [], []
        self.version += 1
        self.images.clear()
        self.canvas.delete("all")
        self.canvas.configure(scrollregion=(0, 0, 1, 1))

    # -- worker --------------------------------------------------------------------
    def _submit(self, pages):
        version, data, scales = self.version, self.data, {p: self._scale(p) for p in pages}
        if data is None:
            return

        def job():
            import pypdfium2 as pdfium
            with pdfcore.LOCK:  # PDFium serves one thread at a time
                if self._doc_version != version:
                    if self._doc is not None:
                        self._doc.close()
                    self._doc = pdfium.PdfDocument(data)
                    self._doc_version = version
                doc = self._doc
                sizes = [doc.get_page_size(i) for i in range(len(doc))]
            out = []
            for p, scale in scales.items():
                with pdfcore.LOCK:
                    if p >= len(doc):
                        continue
                    page = doc[p]
                    bm = page.render(scale=scale, rev_byteorder=True)
                    arr = bm.to_numpy()
                    out.append((p, bm.width, b"P6 %d %d 255\n" % (bm.width, bm.height) + arr[:, :, :3].tobytes()))
                    bm.close()
                    page.close()
            return version, sizes, out

        self.lane.submit(job, self._done)

    def report(self, msg):
        self.app.preview.note(msg)

    def after_layout(self):
        """The pages moved (zoom, resize, new document): subclasses put their marks back."""

    def _done(self, res, err):
        if err or not res or res[0] != self.version:
            if err:
                self.report(f"The PDF could not be shown:\n{err}")
            return
        _, sizes, pages = res
        if sizes != self.sizes:
            self.sizes = sizes
            self._layout(request=False)
        for p, width, ppm in pages:
            if p >= len(self.boxes) or width != self.boxes[p][2]:
                continue  # the window was resized meanwhile
            img = tk.PhotoImage(data=ppm, format="ppm")
            old = self.images.get(p)
            item = self.canvas.create_image(self.boxes[p][0], self.boxes[p][1], image=img, anchor="nw")
            if old:
                self.canvas.delete(old[3])
            self.images[p] = (self.version, width, img, item)
        self.canvas.tag_raise("ov")  # marks drawn over the pages stay on top
        self._request()

    # -- layout ---------------------------------------------------------------------
    def _scale(self, p):
        return self.boxes[p][2] / self.sizes[p][0]

    def _layout(self, request=True):
        c, t = self.canvas, self.t
        if not self.sizes:
            return
        gap = t.px(8)
        room = max(t.px(120), c.winfo_width() - 2 * gap)
        scale = min(room * self.zoom, 3600) / max(w for w, _ in self.sizes)
        total_w = max(c.winfo_width(), int(max(w for w, _ in self.sizes) * scale) + 2 * gap)
        top = c.yview()[0]
        boxes, y = [], gap
        for w, h in self.sizes:
            pw, ph = max(1, int(round(w * scale))), max(1, int(round(h * scale)))
            boxes.append(((total_w - pw) // 2, y, pw, ph))
            y += ph + gap
        changed = boxes != self.boxes
        self.boxes = boxes
        if changed:
            c.delete("paper")
            for p in [p for p in self.images if p >= len(boxes)]:
                c.delete(self.images.pop(p)[3])
            for p, (x, y0, pw, ph) in enumerate(boxes):
                c.create_rectangle(x, y0, x + pw, y0 + ph, fill="#FFFFFF", outline=t.c["border"], tags="paper")
                if p in self.images:  # keep the old picture in place until the sharp one arrives
                    c.coords(self.images[p][3], x, y0)
            c.tag_lower("paper")
            c.configure(scrollregion=(0, 0, total_w, y))
            c.yview_moveto(top)
            self.after_layout()
        if request:
            self._request()

    def _request(self):
        """Ask the worker for the visible pages that are missing or out of date."""
        if not self.boxes or self.data is None:
            return
        c = self.canvas
        y0 = c.canvasy(0)
        y1 = y0 + c.winfo_height()
        seen = [p for p, (_, y, _, h) in enumerate(self.boxes) if y + h >= y0 - h and y <= y1 + h]
        if not seen:
            return
        for p in [p for p in self.images if p < seen[0] - 4 or p > seen[-1] + 4
                  or (p not in seen and self.images[p][1] != self.boxes[p][2])]:
            c.delete(self.images.pop(p)[3])  # far away (saves memory) or drawn for another zoom
        need = [p for p in seen
                if p not in self.images or self.images[p][0] != self.version or self.images[p][1] != self.boxes[p][2]]
        if need:
            self._submit(need)


class PreviewPanel(tk.Frame):
    def __init__(self, parent, app):
        super().__init__(parent, bd=0, highlightthickness=0)
        self.app, self.t = app, app.theme
        t = self.t
        t.paint(self, bg="panel")
        head = tk.Frame(self, bd=0)
        t.paint(head, bg="panel")
        head.pack(fill="x", padx=(t.px(10), t.px(4)), pady=t.px(2))
        cap = tk.Label(head, text="PREVIEW", font=t.f_tiny, bd=0)
        t.paint(cap, bg="panel", fg="faint")
        cap.pack(side="left")
        self.status = tk.Label(head, text="", font=t.f_small, bd=0, anchor="w")
        self.status.pack(side="left", padx=(t.px(8), 0), fill="x", expand=True)
        tool = dict(bg="panel", pad=(4, 4), size=14)
        IconButton(head, t, "close", lambda: app.set_preview(False), "Close the preview", **tool).pack(side="right")
        IconButton(head, t, "refresh", lambda: self.refresh(manual=True),
                   "Refresh the preview / rebuild the PDF (F5)", **tool).pack(side="right")
        self.zoom_in = IconButton(head, t, "plus", lambda: self.pdf.set_zoom(self.pdf.zoom * 1.2),
                                  "Zoom in (Ctrl + mouse wheel)", **tool)
        self.zoom_out = IconButton(head, t, "minus", lambda: self.pdf.set_zoom(self.pdf.zoom / 1.2),
                                   "Zoom out", **tool)
        line = tk.Frame(self, height=1, bd=0)
        t.paint(line, bg="border")
        line.pack(fill="x")
        # LaTeX errors, listed above the page; click one to jump to its line
        self.errors = tk.Text(self, height=1, bd=0, highlightthickness=0, wrap="word", font=t.f_small,
                              padx=t.px(10), pady=t.px(5), cursor="arrow", takefocus=0)
        self.body = tk.Frame(self, bd=0)
        self.body.pack(fill="both", expand=True)
        self.body.grid_rowconfigure(0, weight=1)
        self.body.grid_columnconfigure(0, weight=1)
        self.md = MarkdownView(self.body, app)
        self.pdf = PdfView(self.body, app)
        self.message = tk.Label(self.body, text="", font=t.f_ui, justify="left", anchor="nw", bd=0,
                                padx=t.px(14), pady=t.px(12))
        t.paint(self.message, bg="panel", fg="muted")
        for w in (self.md, self.pdf, self.message):
            w.grid(row=0, column=0, sticky="nsew")
        self.message.bind("<Configure>", lambda e: self.message.configure(wraplength=max(80, e.width - t.px(28))))
        self.lane = Lane(app, "latex")
        self.cancel = None  # stops the compiler when a newer build starts
        self.mode = None
        self.built = None  # the LaTeX source that the shown PDF was made from
        self.building = False
        self._state = ("", "muted")
        self.recolor()
        self.note("")

    # -- small helpers ---------------------------------------------------------------
    def recolor(self):
        c = self.t.c
        self.status.configure(bg=c["panel"], fg=c[self._state[1]])
        self.errors.configure(bg=c["panel"], fg=c["text"])
        self.errors.tag_configure("line", foreground=c["danger"], font=self.t.f_ui_bold)
        self.errors.tag_configure("jump", foreground=c["accent"])
        self.pdf.recolor()

    def retheme(self):
        self.recolor()
        if self.mode == "markdown":
            self.refresh()

    def set_status(self, text, color="muted"):
        self._state = (text, color)
        self.status.configure(text=text, fg=self.t.c[color])

    def _show(self, view, pdf_tools=False):
        view.tkraise()
        for b in (self.zoom_in, self.zoom_out):
            b.pack_forget()
        if pdf_tools:
            self.zoom_in.pack(side="right")
            self.zoom_out.pack(side="right")

    def note(self, text):
        """Show a message instead of a page."""
        self.message.configure(text=text)
        self._show(self.message)

    def _set_errors(self, errors):
        x = self.errors
        x.configure(state="normal")
        x.delete("1.0", "end")
        for tag in x.tag_names():
            if tag.startswith("e:"):
                x.tag_delete(tag)
        for n, (line, msg) in enumerate(errors[:12]):
            tag = f"e:{n}"
            x.insert("end", ("\n" if n else "") + (f"Line {line}  " if line else "•  "), ("line", tag))
            x.insert("end", msg, (tag,))
            if line:
                x.tag_bind(tag, "<Button-1>", lambda e, ln=line: self.app.goto_line(ln))
                x.tag_bind(tag, "<Enter>", lambda e: x.configure(cursor="hand2"))
                x.tag_bind(tag, "<Leave>", lambda e: x.configure(cursor="arrow"))
        x.configure(state="disabled", height=min(6, max(1, len(errors[:12]))))
        if errors:
            x.pack(fill="x", before=self.body)
        else:
            x.pack_forget()

    # -- what to show ------------------------------------------------------------------
    def refresh(self, manual=False):
        """Bring the preview up to date with the editor (called on open, save, format change and ↻)."""
        app = self.app
        kind = app.doc.kind
        if kind != "latex":
            self._stop_build()
            self._set_errors([])
            self.built = None
        if kind == "markdown":
            self.mode = "markdown"
            self.md.render(app.text.get("1.0", "end-1c"), os.path.dirname(app.doc.path) if app.doc.path else None)
            self._show(self.md)
            self.set_status("Markdown · updates as you type")
        elif kind == "latex":
            self.mode = "latex"
            self.build()
        elif kind == "rich":
            self.mode = "rich"
            self.build_rich()
        else:
            self.mode = None
            self.pdf.clear()
            self.set_status("")
            self.note("The preview shows Markdown, LaTeX and formatted (Word, OpenDocument, RTF) documents.\n\n"
                      f"This one is treated as “{app.doc_kind_label()}”. If it is Markdown or LaTeX, click the "
                      "file name in the status bar and pick the format.")

    def text_changed(self):
        """The editor text changed: Markdown follows at once, LaTeX waits for a save or ↻."""
        if self.mode != self.app.doc.kind:
            return  # another document is being opened; refresh() follows
        if self.mode == "markdown":
            self.refresh()
        elif self.mode == "rich":
            self.set_status("Edited · the pages follow in a moment", "muted")
            self.app.later("rich-preview", 1500, lambda: self.mode == "rich" and self.build_rich())
        elif self.mode == "latex" and not self.building and self.built is not None:
            if self.app.text.get("1.0", "end-1c") != self.built:
                self.set_status("Edited · save or press ↻ to rebuild", "warn")
            elif self._state[1] == "warn":
                self.set_status(self._summary, "muted")

    def _stop_build(self):
        if self.cancel is not None:
            self.cancel.set()
        self.cancel, self.building = None, False

    def stop(self):
        """The panel was closed: stop a running compiler."""
        self._stop_build()
        self.mode = None

    def build(self):
        app = self.app
        self._stop_build()
        try:
            import pypdfium2  # noqa: F401
        except Exception:
            self.set_status("")
            self.note("Showing the PDF needs the pypdfium2 package.\n\nStart Rewrite with  python run.py  "
                      "and it is installed automatically.")
            return
        source = app.text.get("1.0", "end-1c")
        if not source.strip():
            self.pdf.clear()
            self.set_status("")
            self.note("Nothing to build yet.")
            return
        cancel = self.cancel = threading.Event()
        self.building = True
        self.set_status("Building…")
        if self._pdf_of is not app.doc:  # do not leave another document's pages up
            self._pdf_of = app.doc
            self.pdf.clear()
        if self.pdf.data is None:
            self.note("Building the PDF…")
        path, enc = app.doc.path, app.doc.encoding

        def job():
            try:
                return latexbuild.build(source, path, enc, cancel)
            except latexbuild.Cancelled:
                return None

        def done(res, err):
            if cancel is not self.cancel or (res is None and not err):
                return  # a newer build has started
            self.building = False
            if err:
                self.set_status("Build failed", "danger")
                self.note(f"The build could not run:\n{err}")
                return
            self.built = source
            self._set_errors(res.errors)
            if res.pdf:
                self.pdf.show(res.pdf)
                self._show(self.pdf, pdf_tools=True)
            else:
                self.pdf.clear()
                tail = "\n".join(res.log.strip().split("\n")[-12:]) if res.log and not res.errors else ""
                self.note(res.message + ("\n\n" + tail if tail else ""))
            n = len(res.errors)
            self._summary = f"{res.engine} · {res.seconds:.1f} s" if res.engine else ""
            if n:
                self.set_status(f"{n} error{'s' if n != 1 else ''} · {self._summary}", "danger")
            else:
                self.set_status(self._summary)
            self.text_changed()  # typed during the build?

        self.lane.submit(job, done)

    def build_rich(self):
        """Typeset a formatted document as it will print (Rewrite's own layout, as in a PDF export)."""
        app = self.app
        if not app.rich.active:
            return
        self._stop_build()
        try:
            model = app.rich.document()
        except Exception as e:
            self.set_status("Preview failed", "danger")
            self.note(f"The document could not be laid out:\n{e}")
            return
        if self._pdf_of is not app.doc:
            self._pdf_of = app.doc
            self.pdf.clear()
        if self.pdf.data is None:
            self.note("Laying out the pages…")
        self.set_status("Laying out…")
        token = self.cancel = threading.Event()
        generic = "sans" if (model.font or "").lower() in ("calibri", "arial", "helvetica", "segoe ui", "verdana") \
            else "serif"

        def job():
            return native.render_pdf(model, generic)

        def done(pdf, err):
            if token is not self.cancel or self.mode != "rich":
                return
            self.cancel = None
            if err:
                self.set_status("Preview failed", "danger")
                self.note(f"The document could not be laid out:\n{err}")
                return
            self.pdf.show(pdf)
            self._show(self.pdf, pdf_tools=True)
            self.set_status("As it prints · Rewrite's layout")

        self.lane.submit(job, done)

    _summary = ""
    _pdf_of = None
