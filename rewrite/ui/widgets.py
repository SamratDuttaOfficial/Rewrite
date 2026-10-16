"""Small custom widgets that look the same on Windows, macOS and Linux."""
import sys
import time
import tkinter as tk
from tkinter import ttk


class FlatButton(tk.Label):
    """A flat, label-based button. kind: ghost | primary | soft | link.
    With icon=, a picture comes before the text; see IconButton for buttons without text."""

    def __init__(self, parent, theme, text="", command=None, kind="ghost", tip=None, bg="bg", font=None,
                 icon=None, pad=None, size=16, box=False, **kw):
        self.t, self.kind, self.command, self.bg_key = theme, kind, command, bg
        self.fg_key = None
        self.enabled = True
        self.icon, self.icon_size, self._img = icon, size, None
        square = box or (icon and not text)  # as big as an icon button; box= does this for a short text
        padx, pady = pad or ((2, 1) if kind == "link" else (5, 5) if square else (9, 4))
        if icon and text:
            kw.update(compound="left", text=" " + text)
        elif square:  # Tk ignores padx/pady on a label that shows a picture: give it its size instead
            kw.update(width=theme.px(size) + 2 * theme.px(padx), height=theme.px(size) + 2 * theme.px(pady))
            if not icon:  # over a blank picture, so that the size above is in pixels
                kw.update(text=text, image=theme.blank, compound="center")
        else:
            kw.update(text=text)
        super().__init__(parent, cursor="hand2", font=font or theme.f_ui, padx=theme.px(padx), pady=theme.px(pady),
                         bd=0, **kw)
        self._hover = False
        self.recolor()
        theme._reg.append((self, {}))  # recolor() is called by the app after theme switches
        self.bind("<Enter>", lambda e: self._set_hover(True))
        self.bind("<Leave>", lambda e: self._set_hover(False))
        self.bind("<ButtonRelease-1>", self._click)
        self.tip = Tooltip(self, theme, tip) if tip else None

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
        if self.icon:
            self._img = self.t.icon(self.icon, fg, self.icon_size)
            self.configure(image=self._img)

    def set_icon(self, name):
        self.icon = name
        self.recolor()

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


class IconButton(FlatButton):
    """A button that is only a picture; its tooltip says what it does."""

    def __init__(self, parent, theme, icon, command=None, tip=None, **kw):
        super().__init__(parent, theme, "", command, tip=tip, icon=icon, **kw)


class Toggle(FlatButton):
    """A button that stays lit while its setting is on: an icon, or with text= a short label such as "Aa"."""

    def __init__(self, parent, theme, icon=None, value=False, command=None, tip=None, text="", **kw):
        self.value, self.on_change = value, command
        super().__init__(parent, theme, text, self._toggle, tip=tip, icon=icon, box=not icon, **kw)

    def _colors(self):
        c = self.t.c
        if self.value:
            return (c["flash"] if self._hover else c["accent_soft"]), c["accent"]
        return (c["hover"] if self._hover else c[self.bg_key]), c["muted"]

    def _toggle(self):
        self.set(not self.value)
        if self.on_change:
            self.on_change(self.value)

    def set(self, v):
        self.value = v
        self.recolor()


class Tooltip:
    _closed = 0.0  # when a tooltip was last closed: the next one, on a neighbouring button, opens at once

    def __init__(self, widget, theme, text):
        self.w, self.t, self.text, self.tip, self.job = widget, theme, text, None, None
        widget.bind("<Enter>", self._schedule, add="+")
        widget.bind("<Leave>", self._hide, add="+")
        widget.bind("<ButtonPress>", self._hide, add="+")

    def _schedule(self, _):
        self._hide()
        self.job = self.w.after(60 if time.time() - Tooltip._closed < 0.5 else 500, self._show)

    def _show(self):
        self.job = None
        if self.tip or not self.text or not self.w.winfo_exists():
            return
        c, px = self.t.c, self.t.px
        tip = self.tip = tk.Toplevel(self.w)
        tip.wm_overrideredirect(True)
        tip.withdraw()
        tk.Label(tip, text=self.text, bg=c["text"], fg=c["surface"], font=self.t.f_small, padx=px(6), pady=px(3),
                 justify="left", wraplength=px(340)).pack()
        tip.update_idletasks()
        w, h = tip.winfo_reqwidth(), tip.winfo_reqheight()
        x = max(0, min(self.w.winfo_rootx(), self.w.winfo_screenwidth() - w - px(4)))  # stay on the screen
        y = self.w.winfo_rooty() + self.w.winfo_height() + px(3)
        if y + h > self.w.winfo_screenheight():
            y = self.w.winfo_rooty() - h - px(3)
        tip.wm_geometry(f"+{x}+{y}")
        tip.deiconify()

    def _hide(self, _=None):
        if self.job:
            self.w.after_cancel(self.job)
            self.job = None
        if self.tip:
            self.tip.destroy()
            self.tip = None
            Tooltip._closed = time.time()


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
        w, h = theme.px(28), theme.px(4)
        super().__init__(parent, width=w, height=h, bd=0, highlightthickness=0, bg=bg)
        c = theme.c
        color = c["good"] if fit >= 0.66 else c["accent"] if fit >= 0.4 else c["faint"]
        self.create_rectangle(0, 0, w, h, fill=c["border"], width=0)
        self.create_rectangle(0, 0, max(2, int(w * max(0.0, min(1.0, fit)))), h, fill=color, width=0)
