"""The formatting toolbar of formatted documents: paragraph style, font, character formatting, alignment,
lists, and things to insert. Icon buttons; each one's tooltip says what it does."""
import sys
import tkinter as tk
from tkinter import font as tkfont

from .. import docmodel
from .widgets import FlatButton, IconButton, Toggle

MOD = "Cmd" if sys.platform == "darwin" else "Ctrl"
STYLES = ("normal", "title", "subtitle", "h1", "h2", "h3", "h4", "quote", "caption", "code")
STYLE_LABEL = dict(docmodel.STYLE_LABEL, code="Code", h4="Heading 4")
SIZES = (8, 9, 10, 10.5, 11, 12, 14, 16, 18, 20, 24, 28, 32, 36, 48, 72)
FONTS = ("Calibri", "Cambria", "Arial", "Helvetica", "Times New Roman", "Georgia", "Garamond", "Segoe UI",
         "Verdana", "Tahoma", "Liberation Serif", "Liberation Sans", "DejaVu Serif", "DejaVu Sans", "Noto Serif",
         "Noto Sans", "Courier New", "Consolas")
COLORS = (("Automatic", None), ("Black", "#000000"), ("Dark grey", "#595959"), ("Red", "#C00000"),
          ("Orange", "#ED7D31"), ("Gold", "#BF8F00"), ("Green", "#00B050"), ("Teal", "#008080"),
          ("Blue", "#0070C0"), ("Dark blue", "#1F3864"), ("Purple", "#7030A0"))
HIGHLIGHTS = (("No highlight", None), ("Yellow", "yellow"), ("Green", "green"), ("Cyan", "cyan"),
              ("Pink", "magenta"), ("Red", "red"), ("Blue", "blue"), ("Grey", "lightGray"))


class FormatBar(tk.Frame):
    def __init__(self, parent, app):
        super().__init__(parent, bd=0)
        self.app, self.t = app, app.theme
        t = self.t
        t.paint(self, bg="bg")
        tool = dict(pad=(4, 4), size=15)
        self.style_btn = FlatButton(self, t, "Normal text ▾", self._style_menu, font=t.f_small, pad=(6, 3),
                                    tip="Paragraph style: headings, title, quote, code …")
        self.style_btn.pack(side="left")
        self.font_btn = FlatButton(self, t, "Font ▾", self._font_menu, font=t.f_small, pad=(6, 3), tip="Font")
        self.font_btn.pack(side="left")
        self.size_btn = FlatButton(self, t, "11 ▾", self._size_menu, font=t.f_small, pad=(6, 3), tip="Text size")
        self.size_btn.pack(side="left")
        self._sep()
        self.toggles = {}
        for name, icon, tip in (("b", "bold", f"Bold ({MOD}+B)"), ("i", "italic", f"Italic ({MOD}+I)"),
                                ("u", "underline", f"Underline ({MOD}+U)"), ("s", "strike", "Strikethrough"),
                                ("sup", "sup", "Superscript"), ("sub", "sub", "Subscript")):
            b = Toggle(self, t, icon, False, lambda v, n=name: self._rich(lambda r: r.toggle(n)), tip=tip, **tool)
            b.pack(side="left")
            self.toggles[name] = b
        IconButton(self, t, "color", self._color_menu, "Text colour", **tool).pack(side="left")
        IconButton(self, t, "highlight", self._highlight_menu, "Highlight", **tool).pack(side="left")
        IconButton(self, t, "link", app.rich_link, f"Link ({MOD}+K)", **tool).pack(side="left")
        IconButton(self, t, "clear-format", lambda: self._rich(lambda r: r.clear_format()),
                   "Clear formatting of the selection", **tool).pack(side="left")
        self._sep()
        self.aligns = {}
        for align, icon, tip in ((None, "align-left", "Align left"), ("center", "align-center", "Centre"),
                                 ("right", "align-right", "Align right"), ("justify", "align-justify", "Justify")):
            b = Toggle(self, t, icon, False, lambda v, a=align: self._rich(lambda r: r.set_para(align=a)), tip=tip,
                       **tool)
            b.pack(side="left")
            self.aligns[align] = b
        self._sep()
        self.lists = {}
        for kind, icon, tip in (("bullet", "list-bullet", "Bulleted list"), ("number", "list-number", "Numbered list")):
            b = Toggle(self, t, icon, False, lambda v, k=kind: self._rich(lambda r: r.toggle_list(k)), tip=tip, **tool)
            b.pack(side="left")
            self.lists[kind] = b
        IconButton(self, t, "outdent", lambda: self._rich(lambda r: r.change_level(-1)),
                   "Decrease indent (Shift+Tab in a list)", **tool).pack(side="left")
        IconButton(self, t, "indent", lambda: self._rich(lambda r: r.change_level(1)),
                   "Increase indent (Tab in a list)", **tool).pack(side="left")
        self._sep()
        for kind, icon, tip in (("image", "image", "Insert a picture"), ("table", "table", "Insert a table"),
                                ("chart", "chart", "Insert a chart"), ("math", "sigma", "Insert a formula"),
                                ("note", "footnote", "Insert a footnote"), ("rule", "rule", "Insert a horizontal line"),
                                ("pagebreak", "page-break", "Insert a page break")):
            IconButton(self, t, icon, lambda k=kind: app.rich_insert(k), tip, **tool).pack(side="left")
        IconButton(self, t, "page-setup", app.page_setup, "Page set-up: paper size, margins, page numbers",
                   **tool).pack(side="right")

    def _sep(self):
        f = tk.Frame(self, width=1, height=self.t.px(16), bd=0)
        self.t.paint(f, bg="border")
        f.pack(side="left", padx=self.t.px(4))

    def _rich(self, fn):
        if self.app.rich.active:
            fn(self.app.rich)
            self.app.text.focus_set()
            self.refresh()

    def _popup(self, widget, build):
        m = tk.Menu(self, tearoff=0)
        build(m)
        try:
            m.tk_popup(widget.winfo_rootx(), widget.winfo_rooty() + widget.winfo_height())
        finally:
            m.grab_release()

    def _style_menu(self):
        cur = self.app.rich.state()[1]["style"] if self.app.rich.active else "normal"

        def build(m):
            var = tk.StringVar(value=cur)
            m._var = var
            for s in STYLES:
                m.add_radiobutton(label=STYLE_LABEL.get(s, s), value=s, variable=var,
                                  command=lambda s=s: self._rich(lambda r: r.set_para(style=s)))
        self._popup(self.style_btn, build)

    def _font_menu(self):
        fams = set(tkfont.families(self))

        def build(m):
            m.add_command(label="Document font", command=lambda: self._rich(lambda r: r.set_value("font", None)))
            m.add_separator()
            for f in FONTS:
                if f in fams:
                    m.add_command(label=f, font=(f, 10), command=lambda f=f: self._rich(lambda r: r.set_value("font", f)))
        self._popup(self.font_btn, build)

    def _size_menu(self):
        def build(m):
            m.add_command(label="Style size", command=lambda: self._rich(lambda r: r.set_value("size", None)))
            m.add_separator()
            for s in SIZES:
                m.add_command(label=f"{s:g}", command=lambda s=s: self._rich(lambda r: r.set_value("size", float(s))))
        self._popup(self.size_btn, build)

    def _color_menu(self):
        def build(m):
            for label, col in COLORS:
                m.add_command(label=label, foreground=col or self.t.c["text"],
                              command=lambda c=col: self._rich(lambda r: r.set_value("color", c)))
        self._popup(self, build)

    def _highlight_menu(self):
        from ..rich import HIGHLIGHT

        def build(m):
            for label, name in HIGHLIGHTS:
                m.add_command(label=label, background=HIGHLIGHT.get(name, "") if name else "",
                              command=lambda n=name: self._rich(lambda r: r.set_value("hl", n)))
        self._popup(self, build)

    def refresh(self):
        """Show the formatting at the cursor."""
        rich = self.app.rich
        if not rich.active:
            return
        try:
            fmt, para = rich.state()
        except tk.TclError:
            return
        for name, b in self.toggles.items():
            b.set(bool(fmt.get(name)))
        for align, b in self.aligns.items():
            b.set(para["align"] == align)
        for kind, b in self.lists.items():
            b.set(para["kind"] == kind)
        self.style_btn.configure(text=STYLE_LABEL.get(para["style"], para["style"]) + " ▾")
        look = docmodel.LOOK.get(para["style"], docmodel.LOOK["normal"])
        size = fmt.get("size") or round(float(rich.base.size or 11) * look["scale"] * 2) / 2
        self.size_btn.configure(text=f"{size:g} ▾")
        self.font_btn.configure(text=(fmt.get("font") or rich.base.font or "Font")[:16] + " ▾")
