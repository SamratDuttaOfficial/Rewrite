"""Small custom widgets that look the same on Windows, macOS and Linux."""
import sys
import tkinter as tk
from tkinter import ttk


class FlatButton(tk.Label):
    """A flat, label-based button. kind: ghost | primary | soft | link."""

    def __init__(self, parent, theme, text, command=None, kind="ghost", tip=None, bg="bg", font=None, **kw):
        self.t, self.kind, self.command, self.bg_key = theme, kind, command, bg
        self.fg_key = None
        self.enabled = True
        super().__init__(parent, text=text, cursor="hand2", font=font or theme.f_ui,
                         padx=theme.px(12 if kind != "link" else 2), pady=theme.px(6 if kind != "link" else 1),
                         bd=0, **kw)
        self._hover = False
        self.recolor()
        theme._reg.append((self, {}))  # recolor() is called by the app after theme switches
        self.bind("<Enter>", lambda e: self._set_hover(True))
        self.bind("<Leave>", lambda e: self._set_hover(False))
        self.bind("<ButtonRelease-1>", self._click)
        if tip:
            Tooltip(self, theme, tip)

    def _colors(self):
        c = self.t.c
        if not self.enabled:
            return c[self.bg_key], c["faint"]
        if self.kind == "primary":
            return (c["accent_hover"] if self._hover else c["accent"]), c["on_accent"]
        if self.kind == "soft":
            return (c["flash"] if self._hover else c["accent_soft"]), c["accent"]
        if self.kind == "link":
            return c[self.bg_key], (c["accent_hover"] if self._hover else c["accent"])
        return (c["hover"] if self._hover else c[self.bg_key]), c[self.fg_key or "text"]

    def recolor(self):
        bg, fg = self._colors()
        self.configure(bg=bg, fg=fg, activebackground=bg)

    def _set_hover(self, v):
        self._hover = v and self.enabled
        self.recolor()

    def _click(self, e):
        if self.enabled and self.command and 0 <= e.x <= self.winfo_width() and 0 <= e.y <= self.winfo_height():
            self.command()

    def set_enabled(self, v):
        self.enabled = v
        self.configure(cursor="hand2" if v else "arrow")
        self.recolor()


class Toggle(FlatButton):
    """A pill that toggles a boolean."""

    def __init__(self, parent, theme, text, value=False, command=None, tip=None):
        self.value, self.label, self.on_change = value, text, command
        super().__init__(parent, theme, self._text(value), self._toggle, tip=tip)

    def _text(self, v):
        return ("✓ " if v else "  ") + self.label

    def _colors(self):
        c = self.t.c
        if self.value:
            return (c["flash"] if self._hover else c["accent_soft"]), c["accent"]
        return (c["hover"] if self._hover else c["bg"]), c["muted"]

    def _toggle(self):
        self.set(not self.value)
        if self.on_change:
            self.on_change(self.value)

    def set(self, v):
        self.value = v
        self.configure(text=self._text(v))
        self.recolor()


class Tooltip:
    def __init__(self, widget, theme, text):
        self.w, self.t, self.text, self.tip, self.job = widget, theme, text, None, None
        widget.bind("<Enter>", self._schedule, add="+")
        widget.bind("<Leave>", self._hide, add="+")
        widget.bind("<ButtonPress>", self._hide, add="+")

    def _schedule(self, _):
        self.job = self.w.after(600, self._show)

    def _show(self):
        if self.tip or not self.w.winfo_exists():
            return
        c = self.t.c
        x = self.w.winfo_rootx()
        y = self.w.winfo_rooty() + self.w.winfo_height() + self.t.px(4)
        self.tip = tk.Toplevel(self.w)
        self.tip.wm_overrideredirect(True)
        self.tip.wm_geometry(f"+{x}+{y}")
        tk.Label(self.tip, text=self.text, bg=c["text"], fg=c["surface"], font=self.t.f_small,
                 padx=self.t.px(8), pady=self.t.px(4), justify="left").pack()

    def _hide(self, _=None):
        if self.job:
            self.w.after_cancel(self.job)
            self.job = None
        if self.tip:
            self.tip.destroy()
            self.tip = None


class ScrollFrame(tk.Frame):
    """A vertically scrollable frame. Put children in `.inner`."""

    def __init__(self, parent, theme, bg="panel"):
        super().__init__(parent, bd=0, highlightthickness=0)
        self.t = theme
        theme.paint(self, bg=bg)
        self.canvas = tk.Canvas(self, bd=0, highlightthickness=0)
        theme.paint(self.canvas, bg=bg)
        self.sb = ttk.Scrollbar(self, orient="vertical", command=self.canvas.yview,
                                style="Panel.Vertical.TScrollbar")
        self.inner = tk.Frame(self.canvas, bd=0)
        theme.paint(self.inner, bg=bg)
        self.win = self.canvas.create_window(0, 0, window=self.inner, anchor="nw")
        self.canvas.configure(yscrollcommand=self._on_scroll)
        self.canvas.pack(side="left", fill="both", expand=True)
        self.inner.bind("<Configure>", self._update)
        self.canvas.bind("<Configure>", self._resize)
        self._sb_shown = False

    def _on_scroll(self, a, b):
        self.sb.set(a, b)
        need = not (float(a) <= 0.0 and float(b) >= 1.0)
        if need != self._sb_shown:
            self._sb_shown = need
            if need:
                self.sb.pack(side="right", fill="y")
            else:
                self.sb.pack_forget()

    def _update(self, _=None):
        self.canvas.configure(scrollregion=self.canvas.bbox("all"))

    def _resize(self, e):
        self.canvas.itemconfigure(self.win, width=e.width)

    def contains(self, widget):
        while widget is not None:
            if widget is self:
                return True
            widget = getattr(widget, "master", None)
        return False

    def wheel(self, e):
        """Scroll from a global mouse-wheel event (the app routes events here)."""
        if not self._sb_shown:
            return
        if getattr(e, "num", None) in (4, 5):
            step = -3 if e.num == 4 else 3
        elif sys.platform == "darwin":
            step = -e.delta
        else:
            step = -e.delta // 40
        self.canvas.yview_scroll(int(step), "units")

    def top(self):
        self.canvas.yview_moveto(0)


class FitBar(tk.Canvas):
    def __init__(self, parent, theme, fit, bg):
        w, h = theme.px(40), theme.px(5)
        super().__init__(parent, width=w, height=h, bd=0, highlightthickness=0, bg=bg)
        c = theme.c
        color = c["good"] if fit >= 0.66 else c["accent"] if fit >= 0.4 else c["faint"]
        self.create_rectangle(0, 0, w, h, fill=c["border"], width=0)
        self.create_rectangle(0, 0, max(2, int(w * max(0.0, min(1.0, fit)))), h, fill=color, width=0)
