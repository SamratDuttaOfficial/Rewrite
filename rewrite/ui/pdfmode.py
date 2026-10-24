"""PDF files: pages to read, mark up and change.

The pages are drawn by PDFium (see PdfView); every change is made by the Rust core (native.pdf_edit) and
gives a new version of the file, so undo simply goes back to the version before. Positions sent to the
core are in points from the top-left of the page as stored, before the page's own turn (/Rotate).
"""
import ctypes
import os
import re
import tkinter as tk
from tkinter import filedialog, messagebox

from .. import native, pdfcore, pdfdoc
from ..lane import Lane
from .objects import PICTURE_TYPES, NoteDialog
from .preview import PdfView
from .widgets import FlatButton, IconButton, Toggle

TOOLS = (
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
)
MARK_COLORS = {"highlight": "#FFE14D", "underline": "#E03131", "strike": "#E03131"}
MAX_UNDO_BYTES = 400_000_000


class PdfBar(tk.Frame):
    """Tools, page operations and zoom for the PDF view."""

    def __init__(self, parent, app):
        super().__init__(parent, bd=0)
        self.app, self.t = app, app.theme
        t = self.t
        t.paint(self, bg="bg")
        tool = dict(pad=(4, 4), size=15)
        self.tools = {}
        for name, icon, tip in TOOLS:
            b = Toggle(self, t, icon, name == "select", lambda v, n=name: self.pick(n), tip=tip, **tool)
            b.pack(side="left")
            self.tools[name] = b
        self._sep()
        for icon, tip, fn in (("rotate-left", "Turn the page left", lambda e: e.rotate(-90)),
                              ("rotate-right", "Turn the page right", lambda e: e.rotate(90)),
                              ("up", "Move the page up", lambda e: e.move_page(-1)),
                              ("down", "Move the page down", lambda e: e.move_page(1)),
                              ("page-add", "Insert a blank page after this one", lambda e: e.insert_blank()),
                              ("merge", "Insert the pages of another PDF after this page", lambda e: e.merge()),
                              ("trash", "Delete this page", lambda e: e.delete_page())):
            IconButton(self, t, icon, lambda f=fn: f(app.pdfview), tip, **tool).pack(side="left")
        self._sep()
        IconButton(self, t, "minus", lambda: app.pdfview.set_zoom(app.pdfview.zoom / 1.2), "Zoom out", **tool).pack(side="left")
        IconButton(self, t, "plus", lambda: app.pdfview.set_zoom(app.pdfview.zoom * 1.2), "Zoom in (Ctrl + mouse wheel)",
                   **tool).pack(side="left")
        FlatButton(self, t, "Convert to document", app.convert_pdf, kind="soft", icon="convert", font=t.f_small, pad=(8, 3),
                   tip="Make an editable document of this PDF (text, headings, lists, tables, pictures), with the "
                       "language model fixing broken words and sentences").pack(side="right")

    def _sep(self):
        f = tk.Frame(self, width=1, height=self.t.px(16), bd=0)
        self.t.paint(f, bg="border")
        f.pack(side="left", padx=self.t.px(4))

    def pick(self, name):
        for n, b in self.tools.items():
            b.set(n == name)
        self.app.pdfview.set_tool(name)


class PdfEditor(PdfView):
    def __init__(self, parent, app):
        super().__init__(parent, app)
        self.work = Lane(app, "pdf-change")
        self.tool = "select"
        self.pages = []  # (left, bottom, right, top, turn) of every page
        self.undo_stack, self.redo_stack = [], []
        self.path = None
        self.busy = False
        self.sel = None  # (page, first char, last char)
        self.drag = None
        self.entry = None
        self.current = 0
        self._chars = {}  # page -> chars of the shown version
        self._edoc, self._edoc_version = None, -1  # a PDFium document for text lookups (Tk thread, under LOCK)
        c = self.canvas
        c.configure(cursor="arrow", takefocus=1)
        c.bind("<ButtonPress-1>", self._press)
        c.bind("<B1-Motion>", self._motion)
        c.bind("<ButtonRelease-1>", self._release)
        c.bind("<Button-3>", self._menu)
        c.bind("<Control-c>", lambda e: self.copy())
        c.bind("<Escape>", lambda e: self.clear_selection())

    # -- the document --------------------------------------------------------------------------
    def load(self, data, path=None):
        self.path = path
        self.undo_stack, self.redo_stack = [], []
        self.zoom = 1.0
        self._set(data)

    def _set(self, data):
        try:
            self.pages = native.pdf_pages(data)
        except Exception:
            self.pages = []
        self._chars.clear()
        self.clear_selection()
        self.show(data)

    def report(self, msg):
        self.app.flash_msg(msg.replace("\n", " "), 9000)

    def change(self, ops, what, then=None):
        """Make changes (in the background); the result becomes the shown version."""
        if self.busy or self.data is None:
            return
        self.busy = True
        self.app.flash_msg(f"{what}…", 60000)
        data = self.data

        def job():
            out = native.pdf_edit(data, ops)
            return then(data, out) if then else out

        def done(out, err):
            self.busy = False
            if err:
                messagebox.showerror("PDF", f"{what} did not work.\n\n{err}", parent=self.app.root)
                self.app.flash_msg("")
                return
            self.undo_stack.append(data)
            while sum(len(d) for d in self.undo_stack) > MAX_UNDO_BYTES and len(self.undo_stack) > 1:
                self.undo_stack.pop(0)
            self.redo_stack.clear()
            self._set(out)
            self.app.pdf_changed()
            self.app.flash_msg(f"{what} — {'Cmd' if self.app.root.tk.call('tk', 'windowingsystem') == 'aqua' else 'Ctrl'}+Z undoes it.")

        self.work.submit(job, done)

    def undo(self):
        if self.undo_stack and not self.busy:
            self.redo_stack.append(self.data)
            self._set(self.undo_stack.pop())
            self.app.pdf_changed()

    def redo(self):
        if self.redo_stack and not self.busy:
            self.undo_stack.append(self.data)
            self._set(self.redo_stack.pop())
            self.app.pdf_changed()

    def save(self, as_=False):
        app = self.app
        path = self.path
        if as_ or not path:
            base = os.path.splitext(os.path.basename(path))[0] if path else "document"
            path = filedialog.asksaveasfilename(parent=app.root, title="Save PDF as", initialfile=base + ".pdf",
                                                defaultextension=".pdf", filetypes=[("PDF", "*.pdf")],
                                                initialdir=os.path.dirname(path) if path else app.settings.get("last_dir"))
            if not path:
                return False
        try:
            tmp = path + ".rewrite-tmp"
            with open(tmp, "wb") as f:
                f.write(self.data)
            os.replace(tmp, path)
        except OSError as e:
            messagebox.showerror("Could not save", str(e), parent=app.root)
            return False
        self.path = path
        app.doc.path = path
        app.dirty = False
        app.update_title()
        app.flash_msg(f"Saved {os.path.basename(path)}")
        return True

    # -- where things are ---------------------------------------------------------------------------
    def _page_size(self, p):
        b = self.pages[p] if p < len(self.pages) else (0, 0, 612, 792, 0)
        return b[2] - b[0], b[3] - b[1], int(b[4]) % 360

    def to_page(self, cx, cy, clamp=None):
        """Canvas point -> (page, x, y) in the page's stored, unturned coordinates; None off the pages.
        With clamp=page, a point off that page is moved to its edge."""
        for p, (bx, by, bw, bh) in enumerate(self.boxes):
            if clamp is not None and p != clamp:
                continue
            if clamp is None and not (bx <= cx <= bx + bw and by <= cy <= by + bh):
                continue
            s = bw / max(1e-6, self.sizes[p][0])
            u, v = (min(max(cx, bx), bx + bw) - bx) / s, (min(max(cy, by), by + bh) - by) / s
            w, h, r = self._page_size(p)
            if r == 90:
                return p, v, h - u
            if r == 180:
                return p, w - u, h - v
            if r == 270:
                return p, w - v, u
            return p, u, v
        return None

    def to_canvas(self, p, x, y):
        bx, by, bw, bh = self.boxes[p]
        s = bw / max(1e-6, self.sizes[p][0])
        w, h, r = self._page_size(p)
        u, v = {90: (h - y, x), 180: (w - x, h - y), 270: (y, w - x)}.get(r, (x, y))
        return bx + u * s, by + v * s

    def scale(self, p):
        return self.boxes[p][2] / max(1e-6, self.sizes[p][0])

    def _event(self, e, clamp=None):
        return self.to_page(self.canvas.canvasx(e.x), self.canvas.canvasy(e.y), clamp)

    def view_page(self):
        """The page in the middle of the view (or the last one clicked)."""
        c = self.canvas
        mid = c.canvasy(c.winfo_height() / 2)
        for p, (_, y, _, h) in enumerate(self.boxes):
            if y <= mid <= y + h + self.t.px(8):
                return p
        return min(self.current, max(0, len(self.boxes) - 1))

    # -- text on the page -----------------------------------------------------------------------
    def chars(self, p):
        """The page's characters: (text, x0, top, x1, bottom, size, bold, italic, font name), in stored
        page coordinates."""
        if p in self._chars:
            return self._chars[p]
        import pypdfium2 as pdfium
        import pypdfium2.raw as raw
        out = []
        with pdfcore.LOCK:
            if self._edoc_version != self.version:
                if self._edoc is not None:
                    self._edoc.close()
                self._edoc = pdfium.PdfDocument(self.data)
                self._edoc_version = self.version
            page = self._edoc[p]
            try:
                tp = page.get_textpage()
                try:
                    left, _, _, top = self.pages[p][:4] if p < len(self.pages) else (0, 0, 0, page.get_size()[1])
                    buf = (ctypes.c_char * 256)()
                    flags = ctypes.c_int(0)
                    for c in pdfdoc._chars(tp, top):
                        out.append([c[0], c[1] - left, c[2], c[3] - left, c[4], c[5], c[6], c[7], ""])
                    # the font of each character (its name, for writing new text in a matching face)
                    names = {}
                    k = 0
                    for i in range(raw.FPDFText_CountChars(tp)):
                        u = raw.FPDFText_GetUnicode(tp, i)
                        if u in (0xFFFE, 0xFFFF):
                            continue
                        if k < len(out):
                            obj = raw.FPDFText_GetTextObject(tp, i)
                            key = ctypes.cast(obj, ctypes.c_void_p).value if obj else None
                            if key not in names:
                                n = raw.FPDFText_GetFontInfo(tp, i, buf, len(buf), ctypes.byref(flags))
                                names[key] = bytes(buf[:max(0, n - 1)]).decode("latin-1", "replace") if n else ""
                            out[k][8] = names[key]
                        k += 1
                finally:
                    tp.close()
            finally:
                page.close()
        self._chars[p] = out
        return out

    def nearest(self, p, x, y):
        """The index of the character nearest to a point (None when the page has no text)."""
        best, bd = None, 1e18
        for i, c in enumerate(self.chars(p)):
            if not c[0].strip() or c[3] <= c[1]:
                continue
            dx = 0 if c[1] <= x <= c[3] else min(abs(x - c[1]), abs(x - c[3]))
            dy = 0 if c[2] <= y <= c[4] else min(abs(y - c[2]), abs(y - c[4]))
            d = dx * dx + 4 * dy * dy
            if d < bd:
                best, bd = i, d
        return best

    def lines(self, p, a, b):
        """The characters a..b grouped in lines: [(x0, top, x1, bottom, text)]."""
        out = []
        cur = None
        for c in self.chars(p)[a:b + 1]:
            if c[3] <= c[1] and not c[0].strip():
                if cur is not None and c[0] in " \t":
                    cur[4] += " "
                continue
            same = cur is not None and abs((c[2] + c[4]) / 2 - (cur[1] + cur[3]) / 2) < max(2.0, 0.5 * (c[4] - c[2])) \
                and c[1] >= cur[0] - 2
            if not same:
                cur = [c[1], c[2], c[3], c[4], ""]
                out.append(cur)
            cur[0], cur[1], cur[2], cur[3] = min(cur[0], c[1]), min(cur[1], c[2]), max(cur[2], c[3]), max(cur[3], c[4])
            if c[0] not in "\r\n":
                cur[4] += c[0]
        return [(x0, t, x1, b_, s.strip()) for x0, t, x1, b_, s in out]

    def line_at(self, p, x, y):
        """The line of text under a point: (first char, last char) or None."""
        i = self.nearest(p, x, y)
        if i is None:
            return None
        cs = self.chars(p)
        c = cs[i]
        if not (c[1] - 3 <= x <= c[3] + 3 * c[5] and c[2] - 3 <= y <= c[4] + 3):
            return None
        mid = (c[2] + c[4]) / 2

        def on_line(d):
            return abs((d[2] + d[4]) / 2 - mid) < 0.45 * max(d[5], 1) if d[3] > d[1] else True
        a = i
        while a > 0 and on_line(cs[a - 1]) and cs[a - 1][0] not in "\r\n" and cs[a - 1][3] <= cs[a][1] + 2 * c[5]:
            a -= 1
        b = i
        while b + 1 < len(cs) and on_line(cs[b + 1]) and cs[b + 1][0] not in "\r\n" and cs[b + 1][1] >= cs[b][1] - 2:
            b += 1
        return a, b

    # -- the tools --------------------------------------------------------------------------------
    def set_tool(self, name):
        self.commit_entry()
        self.tool = name
        self.clear_selection()
        self.canvas.configure(cursor={"select": "xterm", "edit": "xterm", "add": "xterm", "highlight": "xterm",
                                      "underline": "xterm", "strike": "xterm", "draw": "pencil", "note": "hand2",
                                      "image": "hand2"}.get(name, "crosshair"))

    def _press(self, e):
        self.canvas.focus_set()
        self.commit_entry()
        hit = self._event(e)
        self.drag = None
        if not hit:
            self.clear_selection()
            return
        p, x, y = hit
        self.current = p
        if self.tool in ("select", "highlight", "underline", "strike"):
            i = self.nearest(p, x, y)
            self.clear_selection()
            if i is not None:
                self.drag = {"page": p, "from": i}
                self.sel = (p, i, i)
        elif self.tool == "draw":
            self.drag = {"page": p, "points": [(x, y)]}
        elif self.tool in ("whiteout", "box"):
            self.drag = {"page": p, "start": (x, y), "end": (x, y)}
        else:
            self.drag = {"page": p, "click": (x, y)}

    def _motion(self, e):
        d = self.drag
        if not d:
            return
        hit = self._event(e, clamp=d["page"])
        if not hit:
            return
        p, x, y = hit
        if "from" in d:
            i = self.nearest(p, x, y)
            if i is not None:
                self.sel = (p, min(d["from"], i), max(d["from"], i))
                self.paint_selection()
        elif "points" in d:
            d["points"].append((x, y))
            pts = [v for q in d["points"] for v in self.to_canvas(p, *q)]
            self.canvas.delete("drag")
            if len(pts) >= 4:
                self.canvas.create_line(*pts, fill="#1971C2", width=2, tags=("ov", "drag"))
        elif "start" in d:
            d["end"] = (x, y)
            self.canvas.delete("drag")
            (x0, y0), (x1, y1) = self.to_canvas(p, *d["start"]), self.to_canvas(p, x, y)
            self.canvas.create_rectangle(x0, y0, x1, y1, outline="#C92A2A" if self.tool == "box" else "#868E96",
                                         dash=(3, 2), fill="#FFFFFF" if self.tool == "whiteout" else "",
                                         tags=("ov", "drag"))

    def _release(self, e):
        d, self.drag = self.drag, None
        if not d:
            return
        p = d["page"]
        self.canvas.delete("drag")
        tool = self.tool
        if "from" in d:
            if tool in MARK_COLORS and self.sel:
                self.mark(tool)
            return
        if "points" in d:
            if len(d["points"]) > 1:
                self.change([{"op": "ink", "page": p, "paths": [d["points"]], "color": "#1971C2", "width": 2.0}], "Drawing")
            return
        if "start" in d:
            (x0, y0), (x1, y1) = d["start"], d["end"]
            if abs(x1 - x0) < 3 or abs(y1 - y0) < 3:
                return
            box = {"page": p, "x0": min(x0, x1), "top": min(y0, y1), "x1": max(x0, x1), "bottom": max(y0, y1)}
            if tool == "whiteout":
                self.change([dict(box, op="rect", fill="#FFFFFF")], "White-out")
            else:
                self.change([dict(box, op="rect", stroke="#C92A2A", width=1.5)], "Rectangle")
            return
        x, y = d["click"]
        if tool == "note":
            NoteDialog(self.app, {"type": "note", "runs": []},
                       lambda o: o.get("runs") and self.change(
                           [{"op": "note", "page": p, "x": x, "y": y, "text": o["runs"][0]["text"]}], "Note"))
        elif tool == "image":
            path = filedialog.askopenfilename(parent=self.app.root, title="Put a picture on the page",
                                              filetypes=PICTURE_TYPES)
            if path:
                with open(path, "rb") as f:
                    data = f.read()
                w, h = native.image_natural_size(data, 200.0)
                self.change([{"op": "image", "page": p, "x0": x, "top": y, "x1": x + w, "bottom": y + h, "data": data}],
                            "Picture")
        elif tool == "add":
            self.open_entry(p, x, y, "", 12.0, None)
        elif tool == "edit":
            ln = self.line_at(p, x, y)
            if ln is None:
                self.open_entry(p, x, y, "", 12.0, None)
                return
            a, b = ln
            cs = self.chars(p)
            pieces = [c for c in cs[a:b + 1] if c[3] > c[1]]
            x0, top = min(c[1] for c in pieces), min(c[2] for c in pieces)
            x1, bottom = max(c[3] for c in pieces), max(c[4] for c in pieces)
            text = "".join(c[0] for c in cs[a:b + 1] if c[0] not in "\r\n")
            sizes = sorted(c[5] for c in pieces)
            size = sizes[len(sizes) // 2] if sizes else 12.0
            first = pieces[0]
            self.open_entry(p, x0, top, text.strip(), size, {"box": (x0, top, x1, bottom), "font": first[8],
                                                           "bold": first[6], "italic": first[7]})

    # -- typing on the page -------------------------------------------------------------------------
    def open_entry(self, p, x, y, text, size, edit):
        """A box to type in, on the page; Enter puts the text there, Esc drops it."""
        s = self.scale(p)
        px = max(6, int(round(size * s)))
        family = _family(edit["font"]) if edit else None
        cx, cy = self.to_canvas(p, x, y)
        var = tk.StringVar(value=text)
        width = max(12, len(text) + 4)
        e = tk.Entry(self.canvas, textvariable=var, width=width, relief="solid", bd=1,
                     font=(family or self.t.families["sans"], -px), bg="#FFFFFF", fg="#000000", insertbackground="#000000")
        win = self.canvas.create_window(cx, cy, window=e, anchor="nw", tags=("ov", "entry"))
        e.focus_set()
        e.icursor("end")
        e.bind("<Return>", lambda ev: self.commit_entry())
        e.bind("<Escape>", lambda ev: self.drop_entry())
        self.entry = {"widget": e, "win": win, "var": var, "page": p, "x": x, "y": y, "size": size, "edit": edit,
                      "before": text}

    def drop_entry(self):
        if self.entry:
            self.canvas.delete(self.entry["win"])
            self.entry["widget"].destroy()
            self.entry = None
            self.canvas.focus_set()

    def commit_entry(self):
        en = self.entry
        if not en:
            return
        text = en["var"].get()
        self.drop_entry()
        if text.strip() == (en["before"] or "").strip():
            return
        p, edit = en["page"], en["edit"]
        if edit is None:
            if text.strip():
                self.change([{"op": "text", "page": p, "x": en["x"], "y": en["y"], "text": text, "size": en["size"],
                              "generic": "sans"}], "Text")
            return
        x0, top, x1, bottom = edit["box"]
        name = _family(edit["font"])
        new = {"op": "text", "page": p, "x": x0, "y": top, "text": text, "size": en["size"], "font": name,
               "generic": _generic(edit["font"]), "bold": bool(edit["bold"]), "italic": bool(edit["italic"])}
        old_text = en["before"]
        box = {"page": p, "x0": x0, "top": top, "x1": x1, "bottom": bottom}

        def check(before, out):
            """The old words must be gone from the page; if they were drawn in a way that cannot be taken
            out, cover them instead."""
            if _still_there(out, p, box, old_text, self.pages):
                return native.pdf_edit(before, [dict(box, op="rect", fill="#FFFFFF", x0=x0 - 1, x1=x1 + 1),
                                                dict(new)] if text.strip() else
                                       [dict(box, op="rect", fill="#FFFFFF", x0=x0 - 1, x1=x1 + 1)])
            return native.pdf_edit(out, [new]) if text.strip() else out

        self.change([dict(box, op="erase_text")], "Text changed", then=check)

    # -- selection and marks ---------------------------------------------------------------------
    def clear_selection(self):
        self.sel = None
        self.canvas.delete("sel")

    def paint_selection(self):
        self.canvas.delete("sel")
        if not self.sel:
            return
        p, a, b = self.sel
        for x0, top, x1, bottom, _ in self.lines(p, a, b):
            (cx0, cy0), (cx1, cy1) = self.to_canvas(p, x0, top), self.to_canvas(p, x1, bottom)
            self.canvas.create_rectangle(min(cx0, cx1), min(cy0, cy1), max(cx0, cx1), max(cy0, cy1), fill="#4C6EF5",
                                         outline="", stipple="gray25", tags=("ov", "sel"))

    def after_layout(self):
        self.paint_selection()
        if self.entry:
            en = self.entry
            self.canvas.coords(en["win"], *self.to_canvas(en["page"], en["x"], en["y"]))

    def selected_text(self):
        if not self.sel:
            return ""
        p, a, b = self.sel
        return "\n".join(t for *_, t in self.lines(p, a, b))

    def copy(self):
        text = self.selected_text()
        if text:
            self.app.root.clipboard_clear()
            self.app.root.clipboard_append(text)
            self.app.flash_msg("Copied.")

    def mark(self, kind):
        if not self.sel:
            return
        p, a, b = self.sel
        quads = [[x0, top, x1, bottom] for x0, top, x1, bottom, _ in self.lines(p, a, b)]
        self.clear_selection()
        if quads:
            self.change([{"op": "mark", "page": p, "kind": {"strike": "strikeout"}.get(kind, kind), "quads": quads,
                          "color": MARK_COLORS[kind]}], {"highlight": "Highlight", "underline": "Underline",
                                                         "strike": "Strike-out"}[kind])

    def _menu(self, e):
        hit = self._event(e)
        if not hit:
            return
        p, x, y = hit
        self.current = p
        m = tk.Menu(self.app.root, tearoff=0)
        if self.sel:
            m.add_command(label="Copy", accelerator="Ctrl+C", command=self.copy)
            m.add_command(label="Highlight", command=lambda: self.mark("highlight"))
            m.add_command(label="Underline", command=lambda: self.mark("underline"))
            m.add_command(label="Strike out", command=lambda: self.mark("strike"))
            m.add_separator()
        m.add_command(label="Remove marks and notes here",
                      command=lambda: self.change([{"op": "delete_annots", "page": p, "x0": x - 4, "top": y - 4,
                                                    "x1": x + 4, "bottom": y + 4}], "Marks removed"))
        m.add_separator()
        m.add_command(label="Turn page left", command=lambda: self.rotate(-90, p))
        m.add_command(label="Turn page right", command=lambda: self.rotate(90, p))
        m.add_command(label="Delete page", command=lambda: self.delete_page(p))
        try:
            m.tk_popup(e.x_root, e.y_root)
        finally:
            m.grab_release()

    # -- pages ------------------------------------------------------------------------------------
    def _page(self, p=None):
        return self.view_page() if p is None else p

    def rotate(self, angle, p=None):
        self.change([{"op": "rotate", "page": self._page(p), "angle": angle}], "Page turned")

    def delete_page(self, p=None):
        p = self._page(p)
        if len(self.pages) <= 1:
            self.app.flash_msg("A PDF needs at least one page.")
            return
        if messagebox.askyesno("Delete page", f"Delete page {p + 1}? (You can undo this.)", parent=self.app.root):
            self.change([{"op": "delete_page", "page": p}], "Page deleted")

    def move_page(self, d, p=None):
        p = self._page(p)
        to = p + d
        if 0 <= to < len(self.pages):
            self.change([{"op": "move_page", "page": p, "to": to}], f"Page moved to {to + 1}")
            self.current = to

    def insert_blank(self, p=None):
        p = self._page(p)
        w, h, r = self._page_size(p)
        if r in (90, 270):
            w, h = h, w
        self.change([{"op": "insert_blank", "at": p + 1, "width": w, "height": h}], "Blank page inserted")

    def merge(self, p=None):
        p = self._page(p)
        path = filedialog.askopenfilename(parent=self.app.root, title="Insert the pages of a PDF",
                                          filetypes=[("PDF", "*.pdf")])
        if not path:
            return
        with open(path, "rb") as f:
            data = f.read()
        self.change([{"op": "append", "data": data, "at": p + 1}], f"Pages of {os.path.basename(path)} inserted")


def _family(font):
    """A font family from a PDF font name: "ABCDEF+Calibri-Bold" -> "Calibri", "TimesNewRomanPSMT" -> "Times New Roman"."""
    name = re.sub(r"^[A-Z]{6}\+", "", font or "")
    name = re.split(r"[-,]", name)[0]
    name = re.sub(r"(PSMT|PS|MT)$", "", name)
    known = {"TimesNewRoman": "Times New Roman", "CourierNew": "Courier New", "ArialMT": "Arial",
             "SegoeUI": "Segoe UI", "LiberationSerif": "Liberation Serif", "LiberationSans": "Liberation Sans",
             "DejaVuSans": "DejaVu Sans", "DejaVuSerif": "DejaVu Serif"}
    if name in known:
        return known[name]
    if re.match(r"^(CM|LM|SF)[A-Z]", name):  # TeX fonts
        return None
    return name or None


def _generic(font):
    n = (font or "").lower()
    if any(k in n for k in ("mono", "courier", "consol", "cmtt", "code")):
        return "mono"
    if any(k in n for k in ("times", "serif", "roman", "georgia", "cambria", "garamond", "cmr", "lmroman", "book",
                            "minion", "palatino")) and "sans" not in n:
        return "serif"
    return "sans"


def _still_there(data, p, box, text, pages):
    """Whether most of the old words can still be found in the box (they were drawn in a way that the
    core could not take out)."""
    import pypdfium2 as pdfium
    words = [w for w in re.findall(r"\w+", text or "") if len(w) > 2]
    if not words:
        return False
    with pdfcore.LOCK:
        doc = pdfium.PdfDocument(data)
        try:
            page = doc[p]
            left, _, _, top = pages[p][:4]
            tp = page.get_textpage()
            found = tp.get_text_bounded(left + box["x0"] - 1, top - box["bottom"] - 1, left + box["x1"] + 1,
                                        top - box["top"] + 1)
            tp.close()
            page.close()
        finally:
            doc.close()
    hits = sum(1 for w in words if w in found)
    return hits >= max(1, len(words) // 2)
