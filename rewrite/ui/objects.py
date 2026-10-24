"""What the rich-text editor embeds in the text — pictures, charts, tables, formulas, notes, rules and
breaks — and the small dialogs that edit them. Objects are dicts in the Rust core's form (see flat.rs)."""
import copy
import os
import tkinter as tk
from tkinter import filedialog, messagebox, ttk
from tkinter import font as tkfont

from .. import docmodel, native
from .dialogs import Dialog
from .widgets import FlatButton, Tooltip

PICTURE_TYPES = [("Pictures", "*.png *.jpg *.jpeg *.gif *.bmp *.tif *.tiff *.webp"), ("All files", "*.*")]


def align_for(obj):
    return {"note": "top", "math": "baseline", "image": "baseline"}.get(obj.get("type"), "center")


def view_for(ed, oid, obj):
    """A PhotoImage (pictures) or a widget that shows the object in the editor."""
    kind = obj.get("type")
    if kind == "image":
        return picture(ed, obj)
    cls = {"chart": ChartView, "table": TableView, "math": MathView, "note": NoteView, "rule": RuleView}.get(kind)
    w = cls(ed, oid, obj) if cls else LabelView(ed, oid, obj)
    return w


def picture(ed, obj):
    """The picture at its size on the page (no wider than the text column)."""
    k = ed.pt_px()
    w = max(8.0, float(obj.get("width") or 0) * k)
    h = max(8.0, float(obj.get("height") or 0) * k)
    col = ed.column_px()
    if w > col:
        h, w = h * col / w, col
    w, h = int(w), int(h)
    bg = tuple(int(ed.th.c["surface"][i:i + 2], 16) for i in (1, 3, 5))
    try:
        ppm = native.image_thumbnail(bytes(obj.get("data") or b""), w, h, bg)
        return tk.PhotoImage(master=ed.t, data=ppm, format="ppm")
    except Exception:
        img = tk.PhotoImage(master=ed.t, width=w, height=h)
        img.put("#DDDDDD", to=(0, 0, w, h))
        return img


class ObjectView:
    """Shared behaviour: double-click edits, right-click offers the object's menu, a click puts the
    cursor next to it."""

    def _wire(self, ed, oid, widget=None):
        self.ed, self.oid = ed, oid
        for w in [widget or self] + list(getattr(self, "_extra", [])):
            w.bind("<Double-Button-1>", lambda e: ed.app.rich_edit_object(self.oid))
            w.bind("<Button-3>", lambda e: ed.app.rich_object_menu(self.oid, e))
            w.bind("<Button-1>", self._place_cursor, add="+")

    def _place_cursor(self, e):
        try:
            idx = self.ed.t.index(self)
            self.ed.t.mark_set("insert", f"{idx} +1c")
            self.ed.t.tag_remove("sel", "1.0", "end")
            self.ed.t.focus_set()
        except tk.TclError:
            pass


class LabelView(tk.Label, ObjectView):
    """Objects shown as a short label: kept-as-is Word parts, page and column breaks."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, bd=0, padx=ed.th.px(4), pady=0, font=ed.th.f_small, cursor="hand2")
        self.update_object(obj)
        self._wire(ed, oid)
        self.tip = Tooltip(self, ed.th, self._tip(obj))

    def _text(self, obj):
        kind = obj.get("type")
        if kind == "pagebreak" or (kind == "break" and obj.get("kind") == "page"):
            return "─────────  page break  ─────────"
        if kind == "break":
            return "↵ column break"
        return f"[{obj.get('label') or 'object'}]"

    def _tip(self, obj):
        if obj.get("type") == "opaque":
            return "Kept exactly as it was in the original file (it cannot be edited here)."
        return ""

    def update_object(self, obj):
        self.configure(text=self._text(obj))
        self.recolor()

    def recolor(self):
        c = self.ed.th.c
        self.configure(bg=c["surface"], fg=c["muted"])

    def read(self):
        return self.ed.objects[self.oid]


class RuleView(tk.Frame, ObjectView):
    """A horizontal line across the column."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, bd=0, height=ed.th.px(12), width=ed.column_px() - ed.th.px(8), cursor="hand2")
        self.line = tk.Frame(self, height=max(1, ed.th.px(1)), bd=0)
        self.line.place(relx=0, rely=0.5, relwidth=1)
        self._extra = [self.line]
        self._wire(ed, oid)
        self.recolor()

    def update_object(self, obj):
        self.configure(width=self.ed.column_px() - self.ed.th.px(8))

    def recolor(self):
        c = self.ed.th.c
        self.configure(bg=c["surface"])
        self.line.configure(bg=c["faint"])

    def read(self):
        return {"type": "rule"}


class MathView(tk.Label, ObjectView):
    """A formula, shown as text with maths symbols; double-click edits its TeX."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, bd=0, padx=ed.th.px(2), pady=0, cursor="hand2")
        self.update_object(obj)
        self._wire(ed, oid)
        self.tip = Tooltip(self, ed.th, "Formula — double-click to edit")

    def update_object(self, obj):
        self.obj = obj
        try:
            shown = native.math_unicode(obj.get("tex", "")) or obj.get("tex", "")
        except Exception:
            shown = obj.get("tex", "")
        th = self.ed.th
        size = int(th.size * (1.25 if obj.get("display") else 1.0))
        self.configure(text=shown or "(formula)", font=(th.families["serif"], size, "italic"))
        self.recolor()

    def recolor(self):
        c = self.ed.th.c
        self.configure(bg=c["surface"], fg=c["text"])

    def read(self):
        return self.ed.objects[self.oid]


class NoteView(tk.Label, ObjectView):
    """A footnote's number; double-click edits the note."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, text="1", bd=0, padx=1, pady=0, cursor="hand2",
                         font=(ed.th.ui_family, max(7, int(ed.th.size * 0.7)), "bold"))
        self.update_object(obj)
        self._wire(ed, oid)
        self.tip = Tooltip(self, ed.th, "")

    def update_object(self, obj):
        self.obj = obj
        text = docmodel.inline_text(docmodel._from(obj.get("runs") or [])) if obj.get("runs") else ""
        if hasattr(self, "tip"):
            self.tip.text = "Note: " + (text.strip()[:200] or "(empty)") + "\nDouble-click to edit"
        self.recolor()

    def recolor(self):
        c = self.ed.th.c
        self.configure(bg=c["surface"], fg=c["accent"])

    def read(self):
        return self.ed.objects[self.oid]


class ChartView(tk.Canvas, ObjectView):
    """A chart drawn from the Rust core's layout."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, bd=0, highlightthickness=0, cursor="hand2")
        self.update_object(obj)
        self._wire(ed, oid)

    def update_object(self, obj):
        self.obj = obj
        ed = self.ed
        k = ed.pt_px()
        w_pt, h_pt = float(obj.get("width") or 432), float(obj.get("height") or 252)
        col = ed.column_px()
        if w_pt * k > col:
            k = col / w_pt
        self.configure(width=int(w_pt * k), height=int(h_pt * k), bg="#FFFFFF")
        self.delete("all")
        fam = ed.th.ui_family
        fonts = {}

        def font(size, bold):
            key = (max(6, int(round(size * k))), bold)  # pixels: the chart's points scaled like the drawing
            if key not in fonts:
                fonts[key] = tkfont.Font(self, family=fam, size=-key[0], weight="bold" if bold else "normal")
            return fonts[key]

        def measure(text, size, bold):
            return font(size, bold).measure(text) / k

        try:
            shapes = native.chart_shapes(obj, w_pt, h_pt, measure)
        except Exception as e:
            self.create_text(8, 8, text=f"Chart could not be drawn: {e}", anchor="nw")
            return
        anchors = {"n": "n", "s": "s", "e": "e", "w": "w", "c": "center"}
        for s in shapes:
            kind = s[0]
            if kind == "rect":
                _, x, y, w, h, fill, stroke = s
                self.create_rectangle(x * k, y * k, (x + w) * k, (y + h) * k, fill=fill or "", outline=stroke or "")
            elif kind == "line":
                _, pts, color, width = s
                if len(pts) >= 2:
                    self.create_line(*[v * k for p in pts for v in p], fill=color, width=max(1, width * k))
            elif kind == "poly":
                _, pts, fill, stroke = s
                if len(pts) >= 3:
                    self.create_polygon(*[v * k for p in pts for v in p], fill=fill or "", outline=stroke or "")
            elif kind == "dot":
                _, x, y, r, fill = s
                self.create_oval((x - r) * k, (y - r) * k, (x + r) * k, (y + r) * k, fill=fill, outline="")
            elif kind == "text":
                _, x, y, text, size, color, anchor, bold, angle = s
                try:
                    self.create_text(x * k, y * k, text=text, fill=color, anchor=anchors.get(anchor, "center"),
                                     font=font(size, bold), angle=angle)
                except tk.TclError:
                    self.create_text(x * k, y * k, text=text, fill=color, anchor=anchors.get(anchor, "center"),
                                     font=font(size, bold))
        self._fonts = fonts

    def recolor(self):
        pass

    def read(self):
        return self.ed.objects[self.oid]


def cell_text(cell):
    """The text of a table cell (a list of paragraph dicts), paragraphs on lines of their own."""
    out = []
    for p in cell or []:
        parts = []
        for x in p.get("content") or []:
            if x.get("type") == "run":
                parts.append(x.get("text", ""))
            elif x.get("type") == "break":
                parts.append(" ")
        out.append("".join(parts))
    return "\n".join(out)


class TableView(tk.Frame, ObjectView):
    """A table edited in place: one small text box per cell."""

    def __init__(self, ed, oid, obj):
        self.ed = ed
        super().__init__(ed.t, bd=0, highlightthickness=0)
        self.cells = []
        self._wire(ed, oid)
        self.update_object(obj)

    def _wire(self, ed, oid, widget=None):
        self.ed, self.oid = ed, oid

    def update_object(self, obj):
        self.obj = copy.deepcopy(obj)
        for w in self.winfo_children():
            w.destroy()
        th, c = self.ed.th, self.ed.th.c
        self.configure(bg=c["border"])
        rows = self.obj.get("rows") or [[[]]]
        ncols = max(1, max(len(r) for r in rows))
        col_px = self.ed.column_px() - th.px(6)
        widths = self.obj.get("widths")
        if widths and len(widths) == ncols and sum(widths) > 0:
            fr = [w / sum(widths) for w in widths]
        else:
            fr = [1.0 / ncols] * ncols
        from ..rich import ptag
        font = self.ed.fonts.get(self.ed.font_tag(ptag(), [])) or th.f_editor
        bold = self.ed.fonts.get(self.ed.font_tag(ptag(), ["f:b"])) or th.f_editor
        char = max(1, font.measure("0"))
        self.cells = []
        for r, row in enumerate(rows):
            line = []
            for k in range(ncols):
                cell = row[k] if k < len(row) else []
                head = r == 0 and self.obj.get("header")
                box = tk.Text(self, wrap="word", bd=0, highlightthickness=0, undo=True, padx=th.px(5),
                              pady=th.px(3), width=max(2, int(col_px * fr[k] / char) - 1), height=1,
                              font=bold if head else font, bg=c["hover"] if head else c["surface"], fg=c["text"],
                              insertbackground=c["text"], selectbackground=c["select"])
                text = cell_text(cell)
                box.insert("1.0", text)
                box.edit_reset()
                box.edit_modified(False)
                box.grid(row=r, column=k, sticky="nsew", padx=(0 if k else 1, 1), pady=(0 if r else 1, 1))
                box.bind("<KeyRelease>", lambda e, b=box: self._fit(b))
                box.bind("<Button-3>", lambda e, rr=r, kk=k: self.ed.app.rich_table_menu(self.oid, rr, kk, e))
                box.bind("<<Modified>>", self._modified)
                box.bind("<Tab>", lambda e, rr=r, kk=k: self._next_cell(rr, kk, 1))
                box.bind("<Shift-Tab>", lambda e, rr=r, kk=k: self._next_cell(rr, kk, -1))
                line.append((box, text))
            self.cells.append(line)
        for k in range(ncols):
            self.grid_columnconfigure(k, weight=1)
        self.after_idle(self._fit_all)

    def _fit(self, box):
        """As many lines as the cell's text needs at the cell's width."""
        width = max(1, int(box.cget("width")))
        n = sum(max(1, -(-len(line) // width)) for line in box.get("1.0", "end-1c").split("\n"))
        if int(box.cget("height")) != n:
            box.configure(height=n)

    def _fit_all(self):
        for line in self.cells:
            for box, _ in line:
                if box.winfo_exists():
                    self._fit(box)

    def _modified(self, e):
        box = e.widget
        if box.edit_modified():
            box.edit_modified(False)
            self.ed.t.edit_modified(True)

    def _next_cell(self, r, k, d):
        flat = [(i, j) for i in range(len(self.cells)) for j in range(len(self.cells[i]))]
        pos = flat.index((r, k)) + d
        if 0 <= pos < len(flat):
            i, j = flat[pos]
            box = self.cells[i][j][0]
            box.focus_set()
            box.tag_add("sel", "1.0", "end-1c")
        return "break"

    def read(self):
        """The table with the cells' edits."""
        obj = copy.deepcopy(self.obj)
        rows = obj.get("rows") or []
        for r, line in enumerate(self.cells):
            for k, (box, before) in enumerate(line):
                try:
                    now = box.get("1.0", "end-1c")
                except tk.TclError:
                    continue
                if now == before:
                    continue
                while len(rows) <= r:
                    rows.append([])
                while len(rows[r]) <= k:
                    rows[r].append([])
                old = rows[r][k]
                first = next((x for p in old for x in p.get("content") or [] if x.get("type") == "run"), None)
                fmt = {key: v for key, v in (first or {}).items() if key not in ("text",)} or {"type": "run"}
                rows[r][k] = [{"content": [dict(fmt, type="run", text=line)] if line else [], "style": "normal",
                               "align": (old[0].get("align") if old else None), "list": None, "indent": 0}
                              for line in now.split("\n")]
        obj["rows"] = rows
        return obj

    def recolor(self):
        self.update_object(self.read())


# -- dialogs ---------------------------------------------------------------------------------------

class _Form(Dialog):
    def field(self, parent, label, value="", width=28):
        t, c = self.t, self.t.c
        row = tk.Frame(parent, bg=c["surface"])
        row.pack(fill="x", pady=(t.px(3), 0))
        tk.Label(row, text=label, font=t.f_ui, bg=c["surface"], fg=c["muted"], width=14, anchor="w").pack(side="left")
        var = tk.StringVar(value=value)
        e = tk.Entry(row, textvariable=var, width=width, font=t.f_ui, relief="flat", bg=c["bg"], fg=c["text"],
                     insertbackground=c["text"], highlightthickness=1, highlightbackground=c["border"],
                     highlightcolor=c["accent"])
        e.pack(side="left", fill="x", expand=True)
        return var, e

    def ok_cancel(self, ok_text="OK", on_ok=None):
        b = self.buttons()
        FlatButton(b, self.t, ok_text, on_ok or self.ok, kind="primary", bg="surface").pack(side="right")
        FlatButton(b, self.t, "Cancel", self.close, bg="surface").pack(side="right", padx=(0, self.t.px(6)))
        self.bind("<Return>", lambda e: (on_ok or self.ok)())


def _num(s, default):
    try:
        return float(str(s).replace(",", ".").strip())
    except ValueError:
        return default


class ImageDialog(_Form):
    """Size, description and replacement of a picture."""

    def __init__(self, app, obj, on_done):
        super().__init__(app, "Picture", 420)
        self.obj, self.on_done = copy.deepcopy(obj), on_done
        t = self.t
        self.label(self.body, "Picture", t.f_title).pack(fill="x")
        nat = native.image_natural_size(bytes(obj.get("data") or b""), None)
        self.ratio = (obj.get("height") or nat[1]) / max(1e-6, obj.get("width") or nat[0])
        self.w, we = self.field(self.body, "Width (pt)", f"{obj.get('width', nat[0]):.0f}", 10)
        self.h, he = self.field(self.body, "Height (pt)", f"{obj.get('height', nat[1]):.0f}", 10)
        self.keep = tk.BooleanVar(value=True)
        ttk.Checkbutton(self.body, text="Keep the proportions", variable=self.keep).pack(anchor="w", pady=(t.px(4), 0))
        we.bind("<KeyRelease>", lambda e: self._sync("w"))
        he.bind("<KeyRelease>", lambda e: self._sync("h"))
        self.alt, _ = self.field(self.body, "Description", obj.get("alt", ""), 30)
        row = tk.Frame(self.body, bg=t.c["surface"])
        row.pack(fill="x", pady=(t.px(8), 0))
        FlatButton(row, t, "Replace picture…", self.replace, kind="soft", bg="surface").pack(side="left")
        FlatButton(row, t, "Fit the page width", self.fit, kind="soft", bg="surface").pack(side="left", padx=(t.px(6), 0))
        self.ok_cancel()
        we.focus_set()
        self.show()

    def _sync(self, which):
        if not self.keep.get():
            return
        if which == "w":
            self.h.set(f"{_num(self.w.get(), 0) * self.ratio:.0f}")
        else:
            self.w.set(f"{_num(self.h.get(), 0) / max(1e-6, self.ratio):.0f}")

    def fit(self):
        page = self.app.rich.meta.get("page") or {}
        m = page.get("margins") or (72, 72, 72, 72)
        width = float(page.get("width", 595.28)) - float(m[1]) - float(m[3])
        self.w.set(f"{width:.0f}")
        self._sync("w")

    def replace(self):
        path = filedialog.askopenfilename(parent=self, title="Choose a picture", filetypes=PICTURE_TYPES)
        if not path:
            return
        obj = load_picture(self.app, path)
        if obj:
            w = _num(self.w.get(), obj["width"])
            self.obj.update(data=obj["data"], ext=obj["ext"], name=obj["name"])
            self.ratio = obj["height"] / max(1e-6, obj["width"])
            self.w.set(f"{w:.0f}")
            self._sync("w")

    def ok(self):
        self.obj["width"] = max(4.0, _num(self.w.get(), self.obj.get("width", 100)))
        self.obj["height"] = max(4.0, _num(self.h.get(), self.obj.get("height", 100)))
        self.obj["alt"] = self.alt.get()
        self.close()
        self.on_done(self.obj)


def load_picture(app, path, max_width=None):
    """A picture object from a file, sized as it would be printed (at most the page's text width)."""
    try:
        with open(path, "rb") as f:
            data = f.read()
        kind, _, _ = native.image_info(data)
    except Exception as e:
        messagebox.showerror("Picture", f"{os.path.basename(path)} could not be read.\n\n{e}", parent=app.root)
        return None
    if not kind:
        messagebox.showerror("Picture", f"{os.path.basename(path)} is not a picture Rewrite can show.", parent=app.root)
        return None
    if max_width is None:
        page = app.rich.meta.get("page") or {}
        m = page.get("margins") or (72, 72, 72, 72)
        max_width = float(page.get("width", 595.28)) - float(m[1]) - float(m[3])
    w, h = native.image_natural_size(data, max_width)
    return {"type": "image", "data": data, "ext": kind, "width": w, "height": h, "alt": "",
            "name": os.path.basename(path)}


class LinkDialog(_Form):
    def __init__(self, app, url, text, on_done):
        super().__init__(app, "Link", 440)
        self.on_done = on_done
        self.label(self.body, "Link", self.t.f_title).pack(fill="x")
        self.url, e = self.field(self.body, "Address", url or "https://", 36)
        self.text = None
        if text is not None:
            self.text, _ = self.field(self.body, "Text to show", text, 36)
        self.label(self.body, "Leave the address empty to remove the link. Ctrl+click a link to open it.",
                   self.t.f_small, fg="muted").pack(fill="x", pady=(self.t.px(6), 0))
        self.ok_cancel()
        e.focus_set()
        e.select_range(0, "end")
        self.show()

    def ok(self):
        url = self.url.get().strip()
        text = self.text.get() if self.text is not None else None
        self.close()
        self.on_done(None if url in ("", "https://", "http://") else url, text)


class MathDialog(_Form):
    """A formula in TeX, with a live look at how it reads."""

    def __init__(self, app, obj, on_done):
        super().__init__(app, "Formula", 480)
        self.obj, self.on_done = dict(obj), on_done
        t, c = self.t, self.t.c
        self.label(self.body, "Formula", t.f_title).pack(fill="x")
        self.label(self.body, "Type it in TeX, for example  \\frac{a}{b},  x^2,  \\sum_{i=1}^{n} x_i,  \\alpha.",
                   t.f_small, fg="muted").pack(fill="x", pady=(t.px(2), t.px(6)))
        self.src = tk.Text(self.body, height=4, width=50, font=(t.families["mono"], t.size - 1),
                           bg=c["bg"], fg=c["text"], insertbackground=c["text"], relief="flat",
                           highlightthickness=1, highlightbackground=c["border"], highlightcolor=c["accent"])
        self.src.pack(fill="x")
        self.src.insert("1.0", obj.get("tex", ""))
        self.shown = tk.Label(self.body, text="", font=(t.families["serif"], t.size + 3, "italic"), bg=c["surface"],
                              fg=c["text"], anchor="w", justify="left", wraplength=self.width)
        self.shown.pack(fill="x", pady=(t.px(8), 0))
        self.display = tk.BooleanVar(value=bool(obj.get("display")))
        ttk.Checkbutton(self.body, text="On a line of its own (display formula)", variable=self.display).pack(
            anchor="w", pady=(t.px(6), 0))
        self.src.bind("<KeyRelease>", lambda e: self._show())
        self._show()
        self.ok_cancel(on_ok=self.ok)
        self.unbind("<Return>")
        self.bind("<Control-Return>", lambda e: self.ok())
        self.src.focus_set()
        self.show()

    def _show(self):
        tex = self.src.get("1.0", "end-1c")
        try:
            self.shown.configure(text=native.math_unicode(tex))
        except Exception:
            self.shown.configure(text=tex)

    def ok(self):
        tex = self.src.get("1.0", "end-1c").strip()
        self.close()
        if tex:
            self.on_done({"type": "math", "tex": tex, "display": bool(self.display.get())})


class NoteDialog(_Form):
    def __init__(self, app, obj, on_done):
        super().__init__(app, "Note", 460)
        self.obj, self.on_done = dict(obj), on_done
        t, c = self.t, self.t.c
        self.label(self.body, "Footnote", t.f_title).pack(fill="x")
        self.box = tk.Text(self.body, height=5, width=52, wrap="word", font=t.f_editor, bg=c["bg"], fg=c["text"],
                           insertbackground=c["text"], relief="flat", highlightthickness=1,
                           highlightbackground=c["border"], highlightcolor=c["accent"])
        self.box.pack(fill="x", pady=(t.px(6), 0))
        runs = obj.get("runs") or []
        self.before = "".join(r.get("text", "") for r in runs if r.get("type") == "run")
        self.box.insert("1.0", self.before)
        self.ok_cancel()
        self.unbind("<Return>")
        self.box.focus_set()
        self.show()

    def ok(self):
        text = self.box.get("1.0", "end-1c").strip()
        self.close()
        if text != self.before.strip():
            self.on_done({"type": "note", "runs": [{"type": "run", "text": text}] if text else []})


class TableDialog(_Form):
    def __init__(self, app, on_done):
        super().__init__(app, "Table", 360)
        self.on_done = on_done
        t = self.t
        self.label(self.body, "Insert a table", t.f_title).pack(fill="x")
        self.rows, e = self.field(self.body, "Rows", "3", 6)
        self.cols, _ = self.field(self.body, "Columns", "3", 6)
        self.header = tk.BooleanVar(value=True)
        ttk.Checkbutton(self.body, text="First row is a header", variable=self.header).pack(anchor="w",
                                                                                          pady=(t.px(6), 0))
        self.ok_cancel("Insert")
        e.focus_set()
        e.select_range(0, "end")
        self.show()

    def ok(self):
        r = max(1, min(200, int(_num(self.rows.get(), 3))))
        k = max(1, min(30, int(_num(self.cols.get(), 3))))
        self.close()
        cell = {"content": [], "style": "normal", "align": None, "list": None, "indent": 0}
        self.on_done({"type": "table", "rows": [[[dict(cell)] for _ in range(k)] for _ in range(r)],
                      "header": bool(self.header.get()), "widths": None})


class ChartDialog(_Form):
    """A chart's kind, titles and numbers (typed or pasted as a table)."""

    def __init__(self, app, obj, on_done):
        super().__init__(app, "Chart", 520)
        self.obj, self.on_done = copy.deepcopy(obj), on_done
        t, c = self.t, self.t.c
        self.label(self.body, "Chart", t.f_title).pack(fill="x")
        row = tk.Frame(self.body, bg=c["surface"])
        row.pack(fill="x", pady=(t.px(4), 0))
        tk.Label(row, text="Kind", font=t.f_ui, bg=c["surface"], fg=c["muted"], width=14, anchor="w").pack(side="left")
        self.kinds = list(native.chart_kinds())
        self.kind = tk.StringVar(value=dict(self.kinds).get(obj.get("kind", "bar"), "Bar"))
        box = ttk.Combobox(row, textvariable=self.kind, values=[label for _, label in self.kinds], state="readonly",
                           width=20)
        box.pack(side="left")
        self.title, _ = self.field(self.body, "Title", obj.get("title", ""))
        self.xt, _ = self.field(self.body, "X axis title", obj.get("x_title", ""))
        self.yt, _ = self.field(self.body, "Y axis title", obj.get("y_title", ""))
        self.legend = tk.BooleanVar(value=bool(obj.get("legend", True)))
        ttk.Checkbutton(self.body, text="Show the legend", variable=self.legend).pack(anchor="w", pady=(t.px(4), 0))
        self.label(self.body, "DATA — first row: series names; first column: categories. Paste from a "
                              "spreadsheet, or separate values with tabs or commas.", t.f_tiny, fg="faint").pack(
            fill="x", pady=(t.px(8), t.px(2)))
        self.data = tk.Text(self.body, height=8, width=60, wrap="none", font=(t.families["mono"], t.size - 1),
                            bg=c["bg"], fg=c["text"], insertbackground=c["text"], relief="flat", highlightthickness=1,
                            highlightbackground=c["border"], highlightcolor=c["accent"])
        self.data.pack(fill="both", expand=True)
        try:
            self.data.insert("1.0", native.chart_format_table(self.obj))
        except Exception:
            pass
        self.ok_cancel()
        self.unbind("<Return>")
        self.show()

    def ok(self):
        try:
            cats, series = native.chart_parse_table(self.data.get("1.0", "end-1c"))
        except Exception as e:
            messagebox.showerror("Chart", f"The numbers could not be read.\n\n{e}", parent=self)
            return
        if not series:
            messagebox.showerror("Chart", "Give at least one column of numbers.", parent=self)
            return
        label = self.kind.get()
        kind = next((k for k, lab in self.kinds if lab == label), "bar")
        self.obj.update(type="chart", kind=kind, title=self.title.get(), x_title=self.xt.get(), y_title=self.yt.get(),
                        legend=bool(self.legend.get()), categories=list(cats), series=[list(s) for s in series])
        self.close()
        self.on_done(self.obj)


class PageSetupDialog(_Form):
    def __init__(self, app, page, on_done):
        super().__init__(app, "Page set-up", 400)
        self.on_done = on_done
        t, c = self.t, self.t.c
        self.label(self.body, "Page set-up", t.f_title).pack(fill="x")
        row = tk.Frame(self.body, bg=c["surface"])
        row.pack(fill="x", pady=(t.px(4), 0))
        tk.Label(row, text="Paper", font=t.f_ui, bg=c["surface"], fg=c["muted"], width=14, anchor="w").pack(side="left")
        w, h = page.width, page.height
        name = next((n for n, (pw, ph) in docmodel.PAGE_SIZES.items()
                     if {round(pw), round(ph)} == {round(w), round(h)}), "Custom")
        self.size = tk.StringVar(value=name)
        ttk.Combobox(row, textvariable=self.size, values=list(docmodel.PAGE_SIZES) + ["Custom"], state="readonly",
                     width=10).pack(side="left")
        self.land = tk.BooleanVar(value=w > h)
        ttk.Checkbutton(self.body, text="Landscape", variable=self.land).pack(anchor="w", pady=(t.px(4), 0))
        self.pw, _ = self.field(self.body, "Width (mm)", f"{w / 72 * 25.4:.0f}", 8)
        self.ph, _ = self.field(self.body, "Height (mm)", f"{h / 72 * 25.4:.0f}", 8)
        m = page.margins
        self.mt, _ = self.field(self.body, "Top margin (mm)", f"{m[0] / 72 * 25.4:.0f}", 8)
        self.mb, _ = self.field(self.body, "Bottom (mm)", f"{m[2] / 72 * 25.4:.0f}", 8)
        self.ml, _ = self.field(self.body, "Left (mm)", f"{m[3] / 72 * 25.4:.0f}", 8)
        self.mr, _ = self.field(self.body, "Right (mm)", f"{m[1] / 72 * 25.4:.0f}", 8)
        self.numbers = tk.BooleanVar(value=bool(page.numbers))
        ttk.Checkbutton(self.body, text="Page numbers", variable=self.numbers).pack(anchor="w", pady=(t.px(6), 0))
        self.size.trace_add("write", lambda *a: self._preset())
        self.land.trace_add("write", lambda *a: self._preset())
        self.ok_cancel()
        self.show()

    def _preset(self):
        name = self.size.get()
        if name in docmodel.PAGE_SIZES:
            w, h = docmodel.PAGE_SIZES[name]
            if self.land.get():
                w, h = h, w
            self.pw.set(f"{w / 72 * 25.4:.0f}")
            self.ph.set(f"{h / 72 * 25.4:.0f}")

    def ok(self):
        mm = 72 / 25.4
        w, h = _num(self.pw.get(), 210) * mm, _num(self.ph.get(), 297) * mm
        name = self.size.get()
        if name in docmodel.PAGE_SIZES:  # keep the exact sizes, not the rounded millimetres
            w, h = docmodel.PAGE_SIZES[name]
            if self.land.get():
                w, h = h, w
        margins = tuple(max(0.0, _num(v.get(), 25) * mm) for v in (self.mt, self.mr, self.mb, self.ml))
        self.close()
        self.on_done(docmodel.PageSetup(width=w, height=h, margins=margins, numbers=bool(self.numbers.get())))
