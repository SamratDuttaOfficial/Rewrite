"""Colours, fonts and ttk styling (light and dark)."""
import sys
import tkinter as tk
from tkinter import font as tkfont
from tkinter import ttk

from . import icons

LIGHT = {
    "bg": "#F4F3EF", "surface": "#FFFFFF", "panel": "#FAF9F6", "border": "#E3E0D8", "hover": "#F0EEE8",
    "text": "#1E2227", "muted": "#646A73", "faint": "#9A9FA7",
    "accent": "#3B5BDB", "accent_hover": "#2F4AC0", "accent_soft": "#E8EDFF", "on_accent": "#FFFFFF",
    "changed": "#DDF3E4", "hint": "#FFEBD6", "hint_line": "#E8590C", "protected": "#8C9199",
    "target": "#FFF1B8", "flash": "#C9D6FF", "select": "#CADBFF", "proposal": "#D4E1FF", "found": "#FFDF8A",
    "good": "#2B8A3E", "warn": "#E8590C", "danger": "#C92A2A", "spell": "#E03131", "grammar": "#1971C2",
    "badge": {
        "synonym": ("#2B8A3E", "#E3F5E8"), "related": ("#0B7285", "#E0F4F7"),
        "in context": ("#6741D9", "#EFEAFF"), "phrase": ("#C2410C", "#FFEEDF"),
        "rewrite": ("#1C6FC4", "#E3F0FD"), "spelling": ("#C92A2A", "#FFE8E8"),
    },
}

DARK = {
    "bg": "#15171B", "surface": "#1C1F24", "panel": "#191C20", "border": "#2C3037", "hover": "#252930",
    "text": "#E4E6E9", "muted": "#9CA2AB", "faint": "#6A707A",
    "accent": "#7D95FF", "accent_hover": "#98ABFF", "accent_soft": "#262E4A", "on_accent": "#0F1220",
    "changed": "#1E3B2A", "hint": "#3B2B1C", "hint_line": "#FF9A57", "protected": "#6E747E",
    "target": "#4A4219", "flash": "#34437A", "select": "#2E4170", "proposal": "#2B3B6B", "found": "#6E5318",
    "good": "#69DB7C", "warn": "#FFA94D", "danger": "#FF6B6B", "spell": "#FF6B6B", "grammar": "#4DABF7",
    "badge": {
        "synonym": ("#8CE99A", "#1D3526"), "related": ("#66D9E8", "#163339"),
        "in context": ("#B197FC", "#2A2446"), "phrase": ("#FFA94D", "#3A2A1A"),
        "rewrite": ("#74C0FC", "#182F44"), "spelling": ("#FFA8A8", "#3D1F1F"),
    },
}


def _alive(w):
    try:
        return bool(w.winfo_exists())
    except tk.TclError:
        return False


def _first_available(candidates, fallback):
    try:
        fams = set(tkfont.families())
    except tk.TclError:
        return fallback
    for c in candidates:
        if c in fams:
            return c
    return fallback


class Theme:
    def __init__(self, root, name="light", editor_family="serif", size=11):
        self.root = root
        self.name = name
        self.c = LIGHT if name == "light" else DARK
        self.scale = max(1.0, root.winfo_fpixels("1i") / 96.0)
        if sys.platform == "win32":
            ui = _first_available(["Segoe UI"], "TkDefaultFont")
            serif = _first_available(["Georgia", "Cambria"], "Times New Roman")
            sans = ui
            mono = _first_available(["Cascadia Mono", "Consolas"], "Courier New")
        elif sys.platform == "darwin":
            ui = _first_available(["SF Pro Text", "Helvetica Neue"], "TkDefaultFont")
            serif = _first_available(["Georgia", "Charter", "Iowan Old Style"], "Times")
            sans = ui
            mono = _first_available(["SF Mono", "Menlo"], "Courier")
        else:
            ui = _first_available(["Inter", "Cantarell", "Ubuntu", "Noto Sans", "DejaVu Sans"], "TkDefaultFont")
            serif = _first_available(["Noto Serif", "DejaVu Serif", "Liberation Serif"], "Times")
            sans = _first_available(["Noto Sans", "Ubuntu", "DejaVu Sans"], ui)
            mono = _first_available(["JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono"], "Courier")
        self.families = {"serif": serif, "sans": sans, "mono": mono}
        self.ui_family = ui
        self.editor_family = editor_family if editor_family in self.families else "serif"
        self.size = size
        base = 9 if sys.platform != "darwin" else 12
        self.f_ui = tkfont.Font(root, family=ui, size=base)
        self.f_ui_bold = tkfont.Font(root, family=ui, size=base, weight="bold")
        self.f_small = tkfont.Font(root, family=ui, size=base - 1)
        self.f_strike = tkfont.Font(root, family=ui, size=base, overstrike=1)
        self.f_tiny = tkfont.Font(root, family=ui, size=base - 1, weight="bold")
        self.f_title = tkfont.Font(root, family=ui, size=base + 4, weight="bold")
        self.f_head = tkfont.Font(root, family=ui, size=base + 2, weight="bold")
        self.f_item = tkfont.Font(root, family=ui, size=base + 1)
        self.f_item_bold = tkfont.Font(root, family=ui, size=base + 1, weight="bold")
        self.f_editor = tkfont.Font(root, family=self.families[self.editor_family], size=size)

        self._reg = []  # (widget, {option: colour key}) for live theme switching
        self._icons = {}  # (name, colour, pixels) -> PhotoImage
        self.blank = tk.PhotoImage(master=root, width=1, height=1)  # see-through; lets a text label be sized in pixels

    def px(self, n):
        return int(round(n * self.scale))

    def icon(self, name, color, size=16):
        """The picture for an icon button, in a colour given as #RRGGBB."""
        key = (name, color, self.px(size))
        if key not in self._icons:
            rgb = tuple(int(color[k:k + 2], 16) for k in (1, 3, 5))
            self._icons[key] = tk.PhotoImage(master=self.root, data=icons.png(name, key[2], rgb), format="png")
        return self._icons[key]

    def paint(self, widget, **roles):
        """Colour a widget from the palette and remember it for theme switches."""
        widget.configure(**{opt: self.c[key] for opt, key in roles.items()})
        self._reg.append((widget, roles))
        if len(self._reg) > 3000:
            self._reg = [(w, r) for w, r in self._reg if _alive(w)]
        return widget

    def repaint(self):
        keep = []
        for w, roles in self._reg:
            try:
                if roles:
                    w.configure(**{opt: self.c[key] for opt, key in roles.items()})
                if hasattr(w, "recolor"):
                    w.recolor()
                keep.append((w, roles))
            except tk.TclError:
                pass
        self._reg = keep

    def set_editor_font(self, family=None, size=None):
        if family:
            self.editor_family = family
        if size:
            self.size = max(8, min(40, size))
        self.f_editor.configure(family=self.families[self.editor_family], size=self.size)

    def switch(self, name):
        self.name = name
        self.c = LIGHT if name == "light" else DARK

    def style_ttk(self):
        c = self.c
        st = ttk.Style(self.root)
        try:
            st.theme_use("clam")
        except tk.TclError:
            pass
        st.configure(".", background=c["bg"], foreground=c["text"], font=self.f_ui,
                     bordercolor=c["border"], lightcolor=c["bg"], darkcolor=c["bg"],
                     troughcolor=c["bg"], focuscolor=c["accent"])
        for name, bg in (("Vertical.TScrollbar", c["surface"]), ("Panel.Vertical.TScrollbar", c["panel"])):
            st.configure(name, background=c["border"], troughcolor=bg, bordercolor=bg,
                         lightcolor=c["border"], darkcolor=c["border"], arrowcolor=c["muted"],
                         gripcount=0, arrowsize=self.px(9), width=self.px(9))
            st.map(name, background=[("active", c["faint"]), ("!active", c["border"])])
        st.layout("Vertical.TScrollbar", [("Vertical.Scrollbar.trough", {
            "children": [("Vertical.Scrollbar.thumb", {"expand": "1", "sticky": "nswe"})], "sticky": "ns"})])
        st.layout("Panel.Vertical.TScrollbar", [("Vertical.Scrollbar.trough", {
            "children": [("Vertical.Scrollbar.thumb", {"expand": "1", "sticky": "nswe"})], "sticky": "ns"})])
        st.configure("TProgressbar", background=c["accent"], troughcolor=c["border"],
                     bordercolor=c["border"], lightcolor=c["accent"], darkcolor=c["accent"], thickness=self.px(4))
        st.configure("TPanedwindow", background=c["border"])
        st.configure("Sash", sashthickness=self.px(4), gripcount=0, background=c["border"])  # wide enough to grab
        st.configure("TCheckbutton", background=c["surface"], foreground=c["text"], font=self.f_ui,
                     indicatorbackground=c["surface"], indicatorforeground=c["accent"])
        st.map("TCheckbutton", background=[("active", c["surface"])],
               indicatorbackground=[("selected", c["surface"]), ("disabled", c["hover"])],
               foreground=[("disabled", c["faint"])])
        st.configure("TRadiobutton", background=c["surface"], foreground=c["text"], font=self.f_ui,
                     indicatorbackground=c["surface"], indicatorforeground=c["accent"])
        st.map("TRadiobutton", background=[("active", c["surface"])],
               foreground=[("disabled", c["faint"])])
        self.root.configure(bg=c["bg"])
        self.root.option_add("*Menu.background", c["surface"])
        self.root.option_add("*Menu.foreground", c["text"])
        self.root.option_add("*Menu.activeBackground", c["accent_soft"])
        self.root.option_add("*Menu.activeForeground", c["text"])
        self.root.option_add("*Menu.relief", "flat")
