"""Side panel: alternatives for the current word or phrase, proofreading remarks and sentence rewrites."""
import tkinter as tk

from .widgets import FitBar, FlatButton, IconButton, ScrollFrame, Tooltip

PAD = 10  # margin left and right of the content
WIDE = 480  # from this width on (the panel below the editor) there is room for two columns

TIPS = [
    ("Click a word", "to see alternatives that fit its context."),
    ("Select a phrase", "(2+ words) for phrase-level rewrites."),
    ("Ctrl+R", "rephrases the whole sentence at the cursor."),
    ("Alt+1 … Alt+9", "applies a suggestion without the mouse."),
    ("Auto-rewrite", "does a pass over the selection or the whole document."),
    ("Red / blue underlines", "mark spelling and grammar slips; click one to fix it."),
    ("Preview", "shows Markdown or the LaTeX PDF next to the text."),
]


class SuggestionPanel(tk.Frame):
    def __init__(self, parent, app):
        super().__init__(parent, bd=0, highlightthickness=0)
        self.app, self.t = app, app.theme
        self.t.paint(self, bg="panel")
        self.scroll = ScrollFrame(self, self.t, bg="panel")
        self.scroll.pack(fill="both", expand=True)
        self.inner = self.col = self.scroll.inner
        self.wide = False
        self.wrap_labels = []  # (label, pixels of its line taken by other things): wrapped to the panel width
        self.clip_labels = []  # (label, tooltip, text, font, pixels taken): kept to one line, cut with …
        self.state = {"target": None, "sentence": "idle", "sentences": [], "orig": None, "review": None,
                      "issue": None}
        self.scroll.canvas.bind("<Configure>", self._rewrap, add="+")
        self._dots = 0
        self._anim = None
        self.render()

    # -- helpers ------------------------------------------------------------------
    @property
    def width(self):
        """Room for text in a column."""
        w = self.scroll.canvas.winfo_width()
        return max(self.t.px(180), (w // 2 if self.wide else w) - self.t.px(2 * PAD))

    def _rewrap(self, _=None):
        if (self.winfo_width() >= self.t.px(WIDE)) != self.wide:
            self.render()  # room for two columns now, or no longer
            return
        w = self.width
        for lbl, extra in self.wrap_labels:
            try:
                lbl.configure(wraplength=max(50, w - extra))
            except tk.TclError:
                pass
        for entry in self.clip_labels:
            try:
                self._clip(*entry)
            except tk.TclError:
                pass

    def _clip(self, lbl, tip, text, font, extra):
        """Show as much of the text as fits on one line; the tooltip has all of it."""
        room, shown = self.width - extra, text
        if font.measure(text) > room:
            lo, hi = 0, len(text)
            while lo < hi:
                mid = (lo + hi + 1) // 2
                lo, hi = (mid, hi) if font.measure(text[:mid] + "…") <= room else (lo, mid - 1)
            shown = text[:lo].rstrip() + "…"
        lbl.configure(text=shown)
        tip.text = text if shown != text else ""

    def _label(self, parent, text, font, fg="text", bg="panel", wrap=0, **kw):
        lbl = tk.Label(parent, text=text, font=font, anchor="w", justify="left", bd=0, **kw)
        self.t.paint(lbl, bg=bg, fg=fg)
        if wrap is not None:
            lbl.configure(wraplength=max(50, self.width - wrap))
            self.wrap_labels.append((lbl, wrap))
        return lbl

    def _text(self, text, font=None, fg="muted", top=2):
        """A wrapped line of text across the column."""
        lbl = self._label(self.col, text, font or self.t.f_small, fg=fg)
        lbl.pack(fill="x", padx=self.t.px(PAD), pady=(self.t.px(top), 0))
        return lbl

    def _caption(self, text, top=8):
        """A small heading. Icon buttons added with _tool() line up at its right."""
        f = tk.Frame(self.col, bd=0)
        self.t.paint(f, bg="panel")
        f.pack(fill="x", padx=(self.t.px(PAD), self.t.px(PAD - 4)), pady=(self.t.px(top), self.t.px(2)))
        f.tools = tk.Frame(f, bd=0)
        self.t.paint(f.tools, bg="panel")
        f.tools.pack(side="right")
        self._label(f, text.upper(), self.t.f_tiny, fg="faint", wrap=None).pack(side="left")
        return f

    def _section(self, text):
        """Start a new part of the panel: a hairline (except at the top) and its heading."""
        if self.col.winfo_children():
            d = tk.Frame(self.col, height=1, bd=0)
            self.t.paint(d, bg="border")
            d.pack(fill="x", pady=(self.t.px(8), 0))
        return self._caption(text, top=6)

    def _tool(self, head, icon, command, tip, kind="ghost", gap=0):
        b = IconButton(head.tools, self.t, icon, command, tip, kind=kind, bg="panel", pad=(3, 3), size=14)
        b.pack(side="left", padx=(self.t.px(gap), 0))
        return b

    def _badge(self, parent, kind, bg):
        fg, soft = self.t.c["badge"].get(kind, (self.t.c["muted"], self.t.c["hover"]))
        return tk.Label(parent, text=kind, font=self.t.f_tiny, fg=fg, bg=soft, padx=self.t.px(4), pady=0, bd=0)

    # -- state updates --------------------------------------------------------------
    def set_target(self, target):
        """target: dict(text, kind, status, items, protected, error) or None."""
        self.state["target"] = target
        self.render()

    def set_review(self, review):
        """review: None or {pending, current: {old, new, pos} | None}."""
        if review != self.state["review"]:
            self.state["review"] = review
            self.render()

    def set_issue(self, issue):
        """issue: None or {text, message, fixes} - the grammar remark under the cursor."""
        if issue != self.state["issue"]:
            self.state["issue"] = issue
            self.render()

    def set_original(self, orig):
        if orig != self.state["orig"]:
            self.state["orig"] = orig
            self.render()

    def set_sentence(self, status, items=()):
        self.state["sentence"] = status
        self.state["sentences"] = list(items)
        self.render(focus_sentence=status != "idle")

    # -- rendering ------------------------------------------------------------------
    def render(self, focus_sentence=False):
        t = self.t
        for w in self.inner.winfo_children():
            w.destroy()
        self.wrap_labels, self.clip_labels = [], []
        if self._anim:
            self.after_cancel(self._anim)
            self._anim = None
        # one column beside the editor; below it, the sentence rewrites get a column of their own
        self.wide = self.winfo_width() >= t.px(WIDE)
        cols = []
        for k in (0, 2):
            on = k == 0 or self.wide
            self.inner.grid_columnconfigure(k, weight=int(on), uniform="col" if on else "")
            if on:
                cols.append(tk.Frame(self.inner, bd=0))
                t.paint(cols[-1], bg="panel")
                cols[-1].grid(row=0, column=k, sticky="new")
        if self.wide:
            line = tk.Frame(self.inner, width=1, bd=0)
            t.paint(line, bg="border")
            line.grid(row=0, column=1, sticky="ns")
        self.col = cols[0]
        if self.state["review"]:
            self._render_review(self.state["review"])
        if self.state["issue"]:
            self._render_issue(self.state["issue"])
        target = self.state["target"]
        if target is None:
            if not self.state["review"] and not self.state["issue"]:
                self._render_tips()
        else:
            self._render_target(target)
        self.col = cols[-1]
        if self.state["orig"] is not None:
            self._render_original(self.state["orig"])
        self._render_sentence()
        for col in cols:
            tk.Frame(col, height=t.px(10), bg=t.c["panel"]).pack(fill="x")
        self.scroll.top()
        if focus_sentence:
            self.after_idle(self._show_sentence_section)

    def _show_sentence_section(self):
        try:
            self.update_idletasks()
            total = max(1, self.inner.winfo_height())
            y = self._sentence_head.winfo_y()
            if total > self.scroll.canvas.winfo_height():
                self.scroll.canvas.yview_moveto(max(0.0, (y - self.t.px(10)) / total))
        except (tk.TclError, AttributeError):
            pass

    def _change(self, old, new, edge, note=None):
        """A card for a replacement: the old text struck out, the new text in bold, a remark below."""
        t, c = self.t, self.t.c
        outer = tk.Frame(self.col, bd=0, bg=c[edge])
        outer.pack(fill="x", padx=t.px(PAD), pady=(t.px(2), 0))
        card = tk.Frame(outer, bd=0, bg=c["surface"])
        card.pack(fill="x", padx=1, pady=1)
        if new is None:
            parts = [(old, t.f_item_bold, c["text"])]
        else:
            parts = [(old, t.f_strike, c["muted"]), ("→", t.f_small, c["faint"]), (new, t.f_item_bold, c["text"])]
        room = self.width - t.px(18)
        in_line = sum(font.measure(text) + t.px(6) for text, font, _ in parts) <= room  # short: old → new
        line = tk.Frame(card, bd=0, bg=c["surface"])
        line.pack(fill="x", padx=t.px(8), pady=(t.px(5), 0 if note else t.px(5)))
        for text, font, fg in parts:
            lbl = tk.Label(line, text="↓" if text == "→" and not in_line else text, font=font, fg=fg,
                           bg=c["surface"], anchor="w", justify="left", bd=0, padx=0, wraplength=max(60, room))
            self.wrap_labels.append((lbl, t.px(18)))
            if in_line:
                lbl.pack(side="left", padx=(0, t.px(6)))
            else:
                lbl.pack(fill="x")
        if note:
            lbl = tk.Label(card, text=note, font=t.f_small, fg=c["muted"], bg=c["surface"], anchor="w",
                           justify="left", bd=0, padx=0, wraplength=max(60, room))
            self.wrap_labels.append((lbl, t.px(18)))
            lbl.pack(fill="x", padx=t.px(8), pady=(t.px(2), t.px(5)))

    def _render_review(self, rv):
        app = self.app
        n, cur = rv["pending"], rv.get("current")
        head = self._section(f"Change {cur['pos']} of {n}" if cur else "Review")
        if cur:
            self._tool(head, "check", app.accept_prop, "Accept this change (Alt+Y)", kind="primary")
            self._tool(head, "close", app.reject_prop, "Reject this change (Alt+N)", gap=2)
            self._tool(head, "next", app.next_prop, "Next change (F8)")
        else:
            self._tool(head, "next", lambda: app.next_prop(start="1.0", inclusive=True), "Go to the first change (F8)")
        self._tool(head, "check-all", app.accept_all_props, f"Accept all {n} changes", gap=8)
        self._tool(head, "trash", app.reject_all_props, "Reject all changes")
        if cur:
            self._change(cur["old"].strip() or "(nothing)", cur["new"].strip() or "(remove this)", "accent")
        else:
            self._text(f"{n} suggested change{'s' if n != 1 else ''} highlighted in blue. "
                       "Click one to accept or reject it.", self.t.f_ui, fg="text")

    def _render_issue(self, issue):
        t, app, fixes = self.t, self.app, issue["fixes"]
        head = self._section("Grammar")
        if len(fixes) == 1:
            self._tool(head, "check", app.fix_issue, "Fix it", kind="primary")
        self._tool(head, "eye-off", app.ignore_issue, "Ignore this remark", gap=2)
        self._change(issue["text"].strip() or "(space)", (fixes[0].strip() or "(remove)") if fixes else None,
                     "grammar", issue["message"])
        if len(fixes) > 1:  # several ways to fix it: one button each
            row = tk.Frame(self.col, bd=0, bg=t.c["panel"])
            row.pack(fill="x", padx=t.px(PAD), pady=(t.px(5), 0))
            for n, f in enumerate(fixes[:3]):
                FlatButton(row, t, f.strip() or "(remove)", lambda n=n: app.fix_issue(n),
                           kind="primary" if n == 0 else "soft", bg="panel", pad=(8, 2)).pack(
                    side="left", padx=(0, t.px(5)))

    def _render_tips(self):
        self._section("Get started")
        for head, rest in TIPS:
            self._text(f"{head} — {rest}", self.t.f_ui, fg="text", top=1)
        if not self.app.engine.smart and self.app.models_state != "loading":
            self._section("Lite mode")
            self._text("Suggestions come from the offline thesaurus only. Download the small context models for "
                       "suggestions that understand the sentence.")
            FlatButton(self.col, self.t, "Get smart models…", self.app.open_models, kind="soft", bg="panel").pack(
                anchor="w", padx=self.t.px(PAD), pady=(self.t.px(6), 0))

    def _render_target(self, t):
        kind, app = t.get("kind", "word"), self.app
        status = t.get("status")
        spelling = kind == "spell" and not t.get("protected") and status != "loading"
        head = self._section({"phrase": "Alternatives for this phrase", "spell": "Spelling"}.get(
            kind, "Alternatives for"))
        if spelling:
            self._tool(head, "book-plus", app.add_word, "Add to dictionary: never mark this word again")
            self._tool(head, "eye-off", lambda: app.add_word(session=True),
                       "Ignore: leave it alone until Rewrite is closed")
        text = t["text"] if len(t["text"]) < 160 else t["text"][:157] + "…"
        self._text(text, self.t.f_item_bold if kind == "phrase" else self.t.f_head,
                   fg="spell" if kind == "spell" else "text", top=0)
        if spelling:
            self._render_spelling(t)
        elif t.get("protected"):
            self._text("This part is markup (LaTeX command, maths, code or a link), so it is left untouched.")
        elif status == "loading":
            self._animate(self._text("Thinking", top=4), "Thinking")
        elif t.get("error"):
            self._text(t["error"], fg="danger")
        else:
            groups = t.get("groups") or [("word", t["text"], [], 0)]
            for g, (_, heading, items, offset) in enumerate(groups):
                if g > 0:
                    self._caption("Or reword the whole phrase", top=10)
                    self._text(heading, self.t.f_item_bold, fg="text", top=0)
                if items:
                    self._rows(items, offset)
                else:
                    self._text("No good alternatives found here. Try selecting a longer phrase, or rephrase the "
                               "whole sentence." if len(groups) == 1 else "No strong single-word alternatives here.")

    def _render_spelling(self, t):
        """A word that is not in the dictionary: corrections (accepting the word is in the heading)."""
        items = (t.get("groups") or [("spell", t["text"], [], 0)])[0][2]
        msg = "Not in the dictionary. Did you mean:" if items else "Not in the dictionary, and nothing similar was found."
        self._text(t.get("error") or msg, fg="danger" if t.get("error") else "muted")
        if items:
            self._rows(items, 0, notes=3)

    def _rows(self, items, offset, notes=None):
        """The list of alternatives. Each sense of the word is described once (`notes`: only on that many rows)."""
        box = tk.Frame(self.col, bd=0)
        self.t.paint(box, bg="panel")
        box.pack(fill="x", pady=(self.t.px(3), 0))
        last_note = None
        for i, s in enumerate(items):
            note = s.note if (s.note != last_note if notes is None else i < notes) else ""
            last_note = s.note or last_note
            self._row(box, offset + i, s, note)

    def _row(self, parent, i, s, note):
        t, c = self.t, self.t.c
        row = tk.Frame(parent, bd=0, cursor="hand2", bg=c["panel"])
        row.pack(fill="x")
        top = tk.Frame(row, bd=0, bg=c["panel"])
        top.pack(fill="x", padx=t.px(PAD), pady=(t.px(3), 0 if note else t.px(3)))
        num = tk.Label(top, text=str(i + 1) if i < 9 else "", width=2, font=t.f_small, fg=c["faint"],
                       bg=c["panel"], anchor="w", bd=0, padx=0)
        num.pack(side="left")
        word = tk.Label(top, text=s.text, font=t.f_item, fg=c["text"], bg=c["panel"], anchor="w",
                        justify="left", bd=0, padx=0, wraplength=max(60, self.width - t.px(112)))
        self.wrap_labels.append((word, t.px(112)))
        word.pack(side="left", fill="x", expand=True)
        badge = self._badge(top, s.kind, c["panel"])
        badge.pack(side="right", padx=(t.px(5), 0))
        bar = FitBar(top, t, s.fit, c["panel"])
        bar.pack(side="right", padx=(t.px(5), 0))
        widgets = [row, top, num, word, bar]
        lbl = None
        if note:
            indent = t.f_small.measure("0") * 2
            lbl = tk.Label(row, font=t.f_small, fg=c["muted"], bg=c["panel"], anchor="w", bd=0, padx=0)
            lbl.pack(fill="x", padx=(t.px(PAD) + indent, t.px(PAD)), pady=(0, t.px(3)))
            widgets.append(lbl)

        def hover(on):
            bg = c["hover"] if on else c["panel"]
            for w in widgets:
                w.configure(bg=bg)

        for w in widgets + [badge]:
            w.bind("<Enter>", lambda e: hover(True))
            w.bind("<Leave>", lambda e: hover(False))
            w.bind("<Button-1>", lambda e, n=i: self.app.apply_suggestion(n))
        if lbl:  # after the bindings above, which the tooltip adds to
            self.clip_labels.append((lbl, Tooltip(lbl, t, ""), note, t.f_small, indent))
            self._clip(*self.clip_labels[-1])

    def _render_original(self, orig):
        head = self._section("Original text here")
        self._tool(head, "revert", self.app.revert_here, "Revert to the original")
        txt = orig if orig.strip() else "(nothing — this text was added)"
        self._text(txt if len(txt) <= 300 else txt[:297] + "…", self.t.f_ui, top=0)

    def _render_sentence(self):
        status, app = self.state["sentence"], self.app
        head = self._sentence_head = self._section("Whole sentence")
        if status in ("results", "empty"):
            self._tool(head, "refresh", lambda: app.rephrase_sentence(more=True), "More variations")
        self._tool(head, "pencil", app.rephrase_sentence, "Rephrase the sentence at the cursor (Ctrl+R)")
        if status == "loading":
            self._animate(self._text("Rewriting", top=0), "Rewriting")
            return
        if status == "empty":
            self._text("No rewrite kept the meaning well enough. Press ↻ for other variations.", top=0)
        if status == "nomodel":
            self._text("Sentence rewriting works best with the paraphrase model (Models menu). Showing "
                       "thesaurus-based rewrites instead.", top=0)
        for i, s in enumerate(self.state["sentences"]):
            self._card(i, s)

    def _card(self, i, s):
        t, c = self.t, self.t.c
        outer = tk.Frame(self.col, bd=0, bg=c["border"], cursor="hand2")
        outer.pack(fill="x", padx=t.px(PAD), pady=(t.px(2) if i == 0 else t.px(5), 0))
        card = tk.Frame(outer, bd=0, bg=c["surface"])
        card.pack(fill="x", padx=1, pady=1)
        body = tk.Label(card, text=s.text.strip(), font=t.f_ui, fg=c["text"], bg=c["surface"], anchor="w",
                        justify="left", bd=0, padx=0, wraplength=max(60, self.width - t.px(18)))
        self.wrap_labels.append((body, t.px(18)))
        body.pack(fill="x", padx=t.px(8), pady=(t.px(5), t.px(1)))
        meta = tk.Label(card, text=s.note, font=t.f_small, fg=c["muted"], bg=c["surface"], anchor="w", bd=0, padx=0)
        meta.pack(fill="x", padx=t.px(8), pady=(0, t.px(5)))
        widgets = [card, body, meta]

        def hover(on):
            outer.configure(bg=c["accent"] if on else c["border"])

        for w in widgets + [outer]:
            w.bind("<Enter>", lambda e: hover(True))
            w.bind("<Leave>", lambda e: hover(False))
            w.bind("<Button-1>", lambda e, n=i: self.app.apply_sentence(n))

    def _animate(self, lbl, base):
        def tick():
            if not lbl.winfo_exists():
                return
            self._dots = (self._dots + 1) % 4
            lbl.configure(text=base + "." * self._dots)
            self._anim = self.after(350, tick)
        tick()
