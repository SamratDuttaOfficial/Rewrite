"""Find and replace: a strip above the editor."""
import bisect
import re
import sys
import tkinter as tk
from tkinter import font as tkfont

from .. import models
from .dialogs import KEY_NEXT, KEY_PREV, KEY_REPLACE, MOD
from .widgets import FlatButton, IconButton, Toggle, Tooltip

MODKEY = "Command" if sys.platform == "darwin" else "Control"
LIMIT = 100000  # matches counted; past this the count reads "100,000+"
MARGIN = 20000  # characters before and after the view whose matches are highlighted
PIECES = 50  # "Replace all" changes the text in at most this many pieces (see _apply)


class FindBar(tk.Frame):
    """Finds with Python's re in the editor text. The current match is the editor's selection, so it stays
    selected when the bar closes; the other matches are highlighted around the view."""

    def __init__(self, parent, app):
        super().__init__(parent, bd=0)
        t = self.t = app.theme
        self.app, self.text = app, app.text
        self.visible = self.replacing = False
        self.snap, self.rx, self.error, self.picked = None, None, None, None
        self.matches, self.starts, self.more = [], [], False
        self._lead = self._painted = None
        px, s = t.px, app.settings
        t.paint(self, bg="surface")
        line = tk.Frame(self, height=1, bd=0)
        t.paint(line, bg="border")
        line.pack(side="bottom", fill="x")
        g = self.body = tk.Frame(self, bd=0)
        t.paint(g, bg="surface")
        g.pack(side="left", padx=px(6), pady=px(4))
        tool = dict(bg="surface", pad=(4, 4), size=14)

        self.chev = IconButton(g, t, "chev-right", self.toggle_replace, f"Replace ({KEY_REPLACE})", **tool)
        self.chev.grid(row=0, column=0)
        self.find_e = self._entry(g, "Find")
        self.find_e.grid(row=0, column=1, sticky="ew", padx=(px(2), px(4)))
        self.find_e.var.trace_add("write", lambda *_: self.visible and self.app.later(
            "find", 120, lambda: self.search(move=True)))
        g.grid_columnconfigure(1, weight=1)  # the field gives way first when the editor is narrow
        self.count = tk.Label(g, font=t.f_small, bd=0)
        t.paint(self.count, bg="surface", fg="muted")
        self.count.grid(row=0, column=2, sticky="ew")
        self.count_tip = Tooltip(self.count, t, "")
        g.grid_columnconfigure(2, minsize=t.f_small.measure("999 of 999") + px(12))
        self._under = tkfont.Font(self, family=t.ui_family, size=t.f_ui.cget("size"), underline=1)
        self.case = Toggle(g, t, None, bool(s.get("find_case")), self._option, "Match case", text="Aa",
                           font=t.f_ui, **tool)
        self.word = Toggle(g, t, None, bool(s.get("find_word")), self._option, "Whole words only", text="ab",
                           font=self._under, **tool)
        self.regex = Toggle(g, t, None, bool(s.get("find_regex")), self._option,
                            "Regular expression: in the replacement, \\1 puts back the first group",
                            text=".*", font=t.f_ui_bold, **tool)
        for k, w in enumerate((self.case, self.word, self.regex)):
            w.grid(row=0, column=3 + k)
        sep = tk.Frame(g, width=1, height=px(16), bd=0)
        t.paint(sep, bg="border")
        sep.grid(row=0, column=6, padx=px(4))
        IconButton(g, t, "up", lambda: self.next(back=True), f"Previous match (Shift+Enter, {KEY_PREV})",
                   **tool).grid(row=0, column=7)
        IconButton(g, t, "down", self.next, f"Next match (Enter, {KEY_NEXT})", **tool).grid(row=0, column=8)
        IconButton(g, t, "close", self.close, "Close (Esc)", **tool).grid(row=0, column=9)

        self.repl_e = self._entry(g, "Replace")
        self.repl_e.grid(row=1, column=1, sticky="ew", padx=(px(2), px(4)), pady=(px(4), 0))
        self.repl_btns = tk.Frame(g, bd=0)
        t.paint(self.repl_btns, bg="surface")
        self.repl_btns.grid(row=1, column=2, columnspan=8, sticky="w", pady=(px(4), 0))
        alt = "Option" if sys.platform == "darwin" else "Alt"
        FlatButton(self.repl_btns, t, "Replace", self.replace, kind="soft", bg="surface", font=t.f_small,
                   pad=(8, 3), tip="Replace this match, then go to the next (Enter)").pack(side="left")
        FlatButton(self.repl_btns, t, "Replace all", self.replace_all, kind="soft", bg="surface", font=t.f_small,
                   pad=(8, 3), tip=f"Replace every match ({MOD}+{alt}+Enter)").pack(side="left", padx=(px(4), 0))
        self.repl_e.grid_remove()
        self.repl_btns.grid_remove()

        bind = self._bind
        for e in (self.find_e, self.repl_e):
            bind(e, "<Escape>", self.close)
            bind(e, f"<{MODKEY}-a>", lambda e=e: (e.select_range(0, "end"), e.icursor("end")))
            bind(e, f"<{MODKEY}-f>", self.open)
            bind(e, f"<{MODKEY}-r>", lambda: self.open(replace=True))
            if sys.platform != "darwin":
                bind(e, f"<{MODKEY}-h>", lambda: self.open(replace=True))
            for mod in ("Alt", "Option") if sys.platform == "darwin" else ("Alt",):
                bind(e, f"<{MODKEY}-{mod}-Return>", self.replace_all)
            bind(e, "<Tab>", self._swap)
            bind(e, "<Shift-Tab>", self._swap)
            if sys.platform != "darwin":
                bind(e, "<ISO_Left_Tab>", self._swap)  # Shift+Tab on Linux
        for seq in ("<Return>", "<KP_Enter>"):
            bind(self.find_e, seq, self.next)
            bind(self.repl_e, seq, self.replace)
        bind(self.find_e, "<Shift-Return>", lambda: self.next(back=True))
        t._reg.append((self, {}))  # after the parts: recolor() puts back the red of "No results"

    def _bind(self, widget, seq, fn):
        try:
            widget.bind(seq, lambda e: (fn(), "break")[1])
        except tk.TclError:  # a key this platform does not have
            pass

    def _entry(self, parent, hint):
        t, px = self.t, self.t.px
        var = tk.StringVar()
        # exportselection off: selecting in the field must not clear the editor's selection, the current match
        e = tk.Entry(parent, textvariable=var, width=26, font=t.f_ui, bd=px(3), relief="flat", highlightthickness=1,
                     exportselection=False)
        t.paint(e, bg="surface", fg="text", insertbackground="text", selectbackground="select",
                selectforeground="text", highlightbackground="border", highlightcolor="accent")
        ph = tk.Label(e, text=hint, font=t.f_ui, bd=0, padx=0, pady=0, cursor="xterm")
        t.paint(ph, bg="surface", fg="faint")
        ph.bind("<Button-1>", lambda _: e.focus_set())

        def placeholder(*_):
            if var.get():
                ph.place_forget()
            else:
                ph.place(x=px(2) + 1, rely=0.5, anchor="w")  # just right of the blinking cursor

        var.trace_add("write", placeholder)
        placeholder()
        e.var = var
        return e

    # -- showing ------------------------------------------------------------------------
    def open(self, replace=False):
        """Show the bar and put the cursor in it; a short selection in the text becomes the search."""
        seed = self._seed()
        self.show()
        if replace:
            self.set_replace(True)
        if seed is not None and seed != self.find_e.var.get():
            self.find_e.var.set(seed)
        e = self.repl_e if replace and self.find_e.var.get() else self.find_e
        e.focus_set()
        e.select_range(0, "end")
        e.icursor("end")

    def show(self):
        if not self.visible:
            self.visible = True
            self.pack(side="top", fill="x", before=self.app.vsb)
        self.search()

    def close(self):
        if self.visible:
            self.visible = False
            self.pack_forget()
            self.text.tag_remove("found", "1.0", "end")
            self._painted = None
        self.text.focus_set()
        self.text.see("insert")

    def set_replace(self, on):
        self.replacing = on
        for w in (self.repl_e, self.repl_btns):
            w.grid() if on else w.grid_remove()
        self.chev.set_icon("chev-down" if on else "chev-right")
        self.chev.tip.text = "Hide replace" if on else f"Replace ({KEY_REPLACE})"

    def toggle_replace(self):
        self.set_replace(not self.replacing)
        (self.repl_e if self.replacing else self.find_e).focus_set()

    def _swap(self):
        """Tab moves between the two fields."""
        if self.replacing:
            (self.repl_e if self.focus_get() is self.find_e else self.find_e).focus_set()

    def align(self, x):
        """Line the find field up with the text column, which starts x pixels into the editor."""
        px = self.t.px
        lead = max(px(6), x - self.chev.winfo_reqwidth() - px(2) - 1 - px(3))
        if lead != self._lead:
            self._lead = lead
            self.body.pack_configure(padx=(lead, px(6)))

    def _seed(self):
        """The text selected in the editor, to look for; None keeps the last search."""
        r = self.text.tag_ranges("sel")
        if not r:
            return None
        s = self.text.get(r[0], r[1])
        if not s.strip() or "\n" in s or len(s) > 200:
            return None
        if self.visible:
            if self._stale():
                self.search()
            if self.current() is not None:  # the selection is a match already: keep the search as it is
                return None
        return re.escape(s) if self.regex.value else s

    def _option(self, _value):
        s = self.app.settings
        s["find_case"], s["find_word"], s["find_regex"] = self.case.value, self.word.value, self.regex.value
        models.save_settings(s)
        self.search(move=True)

    # -- searching ----------------------------------------------------------------------
    def _stale(self):
        return self.snap is not self.app.snap or self.app.snap_dirty

    def search(self, move=False):
        """Find every match in the text; with move, select the first one from the selection or cursor on."""
        q = self.find_e.var.get()
        self.rx, self.error = None, None
        if q:
            pat = q if self.regex.value else re.escape(q)
            if self.word.value:
                pat = rf"(?<!\w)(?:{pat})(?!\w)"
            try:
                self.rx = re.compile(pat, re.MULTILINE | (0 if self.case.value else re.IGNORECASE))
            except re.error as err:
                self.error = ("Invalid pattern", str(err))
        self.snap = self.app.refresh_snapshot()
        found, self.more = [], False
        if self.rx:
            for m in self.rx.finditer(self.snap.text):
                if m.end() > m.start():  # an empty match (such as ^) can be neither shown nor replaced
                    found.append(m.span())
                    if len(found) >= LIMIT:
                        self.more = True
                        break
        self.matches, self.starts = found, [a for a, _ in found]
        if move:
            if found:
                self._select(self._index_from(self._sel()[0]))
            else:
                r = self.text.tag_ranges("sel")
                if r and self.picked == (str(r[0]), str(r[1])):  # the match picked before no longer matches
                    self.text.tag_remove("sel", "1.0", "end")
        self.paint()
        self.show_count()

    def paint(self):
        """Highlight the matches around the view: tagging many thousands at once would be slow."""
        t = self.text
        if self._stale():
            return  # the highlights move with the text until the next search
        i = j = 0
        if self.visible and self.matches:
            try:
                a = self.snap.offset(t.index("@0,0")) - MARGIN
                b = self.snap.offset(t.index(f"@{t.winfo_width()},{t.winfo_height()}")) + MARGIN
            except (ValueError, tk.TclError):
                return
            i, j = bisect.bisect_left(self.starts, a), bisect.bisect_right(self.starts, b)
        p = self._painted
        if p and p[0] is self.matches and p[1:] == (i, j):
            return  # scrolled within the part that is highlighted already
        self._painted = (self.matches, i, j)
        t.tag_remove("found", "1.0", "end")
        idx = []
        for s, e in self.matches[i:j]:
            idx += (self.snap.index(s), self.snap.index(e))
        for k in range(0, len(idx), 2000):
            t.tag_add("found", *idx[k:k + 2000])

    def _sel(self):
        """The editor's selection, or the cursor, as offsets into the searched text."""
        r = self.text.tag_ranges("sel")
        try:
            a = self.snap.offset(self.text.index(r[0] if r else "insert"))
            return a, (self.snap.offset(self.text.index(r[1])) if r else a)
        except (ValueError, tk.TclError):
            return 0, 0

    def current(self):
        """The number of the match that is selected in the editor, or None."""
        if not self.matches:
            return None
        a, b = self._sel()
        i = bisect.bisect_left(self.starts, a)
        return i if i < len(self.matches) and self.matches[i] == (a, b) else None

    def _index_from(self, pos):
        """The first match that starts at pos or later; after the last one, the first in the text."""
        i = bisect.bisect_left(self.starts, pos)
        return i if i < len(self.starts) else 0

    def _select(self, i):
        s, e = self.matches[i]
        i1, i2 = self.snap.index(s), self.snap.index(e)
        t = self.text
        t.tag_remove("sel", "1.0", "end")
        t.tag_add("sel", i1, i2)
        t.mark_set("insert", i2)
        t.see(i2)
        t.see(i1)
        self.picked = (i1, i2)
        self.show_count()

    def next(self, back=False):
        if not self.find_e.var.get():
            self.open()
            return
        if not self.visible:
            self.show()
        elif self._stale():
            self.search()
        if not self.matches:
            self.app.root.bell()
            return
        a, b = self._sel()
        if back:  # the last match that starts before the selection; before the first, the last in the text
            i = bisect.bisect_left(self.starts, a) - 1
            self._select(i if i >= 0 else len(self.matches) - 1)
        else:
            self._select(self._index_from(b))

    def show_count(self):
        c = self.t.c
        q = self.find_e.var.get()
        bad = False
        if self.error:
            text, bad = self.error[0], True
        elif not q:
            text = ""
        elif not self.matches:
            text, bad = "No results", True
        else:
            n = f"{len(self.matches):,}{'+' if self.more else ''}"
            i = self.current()
            text = f"{i + 1:,} of {n}" if i is not None else f"{n} found"
        self.count.configure(text=text, fg=c["danger"] if bad else c["muted"])
        self.count_tip.text = self.error[1] if self.error else ""
        ring = c["danger"] if bad and q else None
        self.find_e.configure(highlightbackground=ring or c["border"], highlightcolor=ring or c["accent"])

    def recolor(self):
        self.show_count()

    # -- replacing ----------------------------------------------------------------------
    def _replacement(self, m):
        """What replaces match m: the text as typed, or for a regular expression, with \\1 … filled in."""
        rep = self.repl_e.var.get()
        return m.expand(rep) if self.regex.value else rep

    def _fail(self, err):
        self.error = ("Invalid replacement", str(err))
        self.show_count()
        self.app.root.bell()

    def _can_edit(self):
        if self.app.busy:
            self.app.flash_msg("Wait for the rewrite to finish.")
            return False
        return True

    def replace(self):
        """Replace the selected match and select the next one; with no match selected, go to the next."""
        if not self._can_edit():
            return
        if self._stale():
            self.search()
        i = self.current()
        if i is None:
            self.next()
            return
        s, e = self.matches[i]
        m = self.rx.match(self.snap.text, s)
        if not m or m.end() != e:
            self.next()
            return
        try:
            new = self._replacement(m)
        except (re.error, IndexError) as err:
            self._fail(err)
            return
        if self.app.replace(s, e, self.snap.text[s:e], new) and self.matches:  # replace() searches again
            self._select(self._index_from(s + len(new)))

    def replace_all(self):
        """Replace every match, as one step for undo."""
        if not self._can_edit():
            return
        self.search()
        if not self.matches:
            self.app.root.bell()
            return
        snap = self.snap
        try:
            edits = [(m.start(), m.end(), self._replacement(m))
                     for m in self.rx.finditer(snap.text) if m.end() > m.start()]
        except (re.error, IndexError) as err:
            self._fail(err)
            return
        self._apply(snap, edits)
        self.app.analyze()
        n = len(edits)
        self.app.flash_msg(f"Replaced {n:,} match{'es' if n != 1 else ''} — {MOD}+Z undoes "
                           f"{'them all' if n != 1 else 'it'}.")

    def _apply(self, snap, edits):
        """Make the edits [(start, end, new)], sorted offsets into snap, as one step for undo.

        Tk takes a few milliseconds per piece to undo in a long text, so 20,000 replacements made one by one
        would take over half a minute to undo. They are joined into at most PIECES pieces instead, and the marks
        inside a piece (the cursor, review suggestions) and the view are put back where they belong."""
        app, t, text = self.app, self.text, snap.text
        size = -(-len(edits) // PIECES)
        pieces = []
        for k in range(0, len(edits), size):
            part, out = edits[k:k + size], []
            pos = part[0][0]
            for s, e, new in part:
                out += (text[pos:s], new)
                pos = e
            pieces.append((part[0][0], part[-1][1], "".join(out)))
        ends, shift = [e for _, e, _ in edits], [0]
        for s, e, new in edits:
            shift.append(shift[-1] + len(new) - (e - s))

        def moved(off):
            i = bisect.bisect_right(ends, off)  # edits that end at or before off
            if i < len(edits) and edits[i][0] < off:  # inside a replaced match: its start
                return edits[i][0] + shift[i]
            return off + shift[i]

        marks = ["insert"] + [f"p{pid}{x}" for pid in app.proposals for x in "se"]
        where = {m: moved(snap.offset(t.index(m))) for m in marks}
        top = moved(snap.offset(t.index("@0,0")))
        with app.one_step():
            for s, e, new in reversed(pieces):  # back to front keeps the earlier positions valid
                i1 = snap.index(s)
                t.delete(i1, snap.index(e))
                t.insert(i1, new, ())
        app.snap_dirty = True
        snap = app.refresh_snapshot()
        for m, off in where.items():
            t.mark_set(m, snap.index(off))
        for pid in app.proposals:
            if t.compare(f"p{pid}s", "<", f"p{pid}e"):
                t.tag_add("prop", f"p{pid}s", f"p{pid}e")
        t.yview(snap.index(top))
