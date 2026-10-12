"""Right-hand side panel: alternatives for the current word/phrase and sentence rewrites."""
import tkinter as tk

from .widgets import FitBar, FlatButton, ScrollFrame

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
        self.inner = self.scroll.inner
        self.wrap_labels = []
        self.state = {"target": None, "sentence": "idle", "sentences": [], "orig": None, "review": None,
                      "issue": None}
        self.scroll.canvas.bind("<Configure>", self._rewrap, add="+")
        self._dots = 0
        self._anim = None
        self.render()

    # -- helpers ------------------------------------------------------------------
    @property
    def width(self):
        return max(self.t.px(220), self.scroll.canvas.winfo_width() - self.t.px(36))

    def _rewrap(self, _=None):
        w = self.width
        for lbl, extra in self.wrap_labels:
            try:
                lbl.configure(wraplength=max(50, w - extra))
            except tk.TclError:
                pass

    def _label(self, parent, text, font, fg="text", bg="panel", wrap=0, **kw):
        lbl = tk.Label(parent, text=text, font=font, anchor="w", justify="left", bd=0, **kw)
        self.t.paint(lbl, bg=bg, fg=fg)
        if wrap is not None:
            lbl.configure(wraplength=max(50, self.width - wrap))
            self.wrap_labels.append((lbl, wrap))
        return lbl

    def _caption(self, text, pady=(18, 6)):
        f = tk.Frame(self.inner, bd=0)
        self.t.paint(f, bg="panel")
        f.pack(fill="x", padx=self.t.px(18), pady=(self.t.px(pady[0]), self.t.px(pady[1])))
        self._label(f, text.upper(), self.t.f_tiny, fg="faint", wrap=None).pack(side="left")
        return f

    def _divider(self):
        d = tk.Frame(self.inner, height=1, bd=0)
        self.t.paint(d, bg="border")
        d.pack(fill="x", padx=self.t.px(18), pady=(self.t.px(14), 0))

    def _badge(self, parent, kind, bg):
        fg, soft = self.t.c["badge"].get(kind, (self.t.c["muted"], self.t.c["hover"]))
        return tk.Label(parent, text=kind, font=self.t.f_tiny, fg=fg, bg=soft, padx=self.t.px(6), pady=0, bd=0)

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
        for w in self.inner.winfo_children():
            w.destroy()
        self.wrap_labels = []
        if self._anim:
            self.after_cancel(self._anim)
            self._anim = None
        if self.state["review"]:
            self._render_review(self.state["review"])
        if self.state["issue"]:
            self._render_issue(self.state["issue"])
        t = self.state["target"]
        if t is None:
            if not self.state["review"] and not self.state["issue"]:
                self._render_tips()
        else:
            self._render_target(t)
        if self.state["orig"] is not None:
            self._render_original(self.state["orig"])
        self._render_sentence()
        tk.Frame(self.inner, height=self.t.px(24), bg=self.t.c["panel"]).pack(fill="x")
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

    def _render_review(self, rv):
        t, c, app = self.t, self.t.c, self.app
        n, cur = rv["pending"], rv.get("current")
        if cur:
            self._caption(f"Suggested change  ·  {cur['pos']} of {n}", pady=(18, 6))
            outer = tk.Frame(self.inner, bd=0, bg=c["accent"])
            outer.pack(fill="x", padx=t.px(18))
            card = tk.Frame(outer, bd=0, bg=c["surface"])
            card.pack(fill="x", padx=1, pady=1)
            old = cur["old"].strip() or "(nothing)"
            new = cur["new"].strip() or "(remove this)"
            for text, font, fg, pady in ((old, t.f_strike, c["muted"], (t.px(10), 0)),
                                         ("↓", t.f_small, c["faint"], (t.px(2), t.px(2))),
                                         (new, t.f_item_bold, c["text"], (0, t.px(10)))):
                lbl = tk.Label(card, text=text, font=font, fg=fg, bg=c["surface"], anchor="w", justify="left",
                               wraplength=max(60, self.width - t.px(30)))
                self.wrap_labels.append((lbl, t.px(30)))
                lbl.pack(fill="x", padx=t.px(12), pady=pady)
            row = tk.Frame(self.inner, bd=0, bg=c["panel"])
            row.pack(fill="x", padx=t.px(18), pady=(t.px(10), 0))
            FlatButton(row, t, "Accept", app.accept_prop, kind="primary", bg="panel",
                       tip="Alt+Y").pack(side="left")
            FlatButton(row, t, "Reject", app.reject_prop, bg="panel", tip="Alt+N").pack(
                side="left", padx=(t.px(6), 0))
            FlatButton(row, t, "Next ›", app.next_prop, kind="link", bg="panel", tip="F8").pack(
                side="right")
        else:
            self._caption("Review suggestions", pady=(18, 6))
            self._label(self.inner, f"{n} suggested change{'s' if n != 1 else ''} highlighted in blue. "
                        "Click one to accept or reject it.", self.t.f_ui, wrap=0).pack(
                fill="x", padx=t.px(18))
            row = tk.Frame(self.inner, bd=0, bg=c["panel"])
            row.pack(fill="x", padx=t.px(18), pady=(t.px(10), 0))
            FlatButton(row, t, "Go to first", lambda: app.next_prop(start="1.0"), kind="soft",
                       bg="panel").pack(side="left")
        links = tk.Frame(self.inner, bd=0, bg=c["panel"])
        links.pack(fill="x", padx=t.px(18), pady=(t.px(8), 0))
        FlatButton(links, t, f"Accept all {n}", app.accept_all_props, kind="link", bg="panel").pack(side="left")
        FlatButton(links, t, "Reject all", app.reject_all_props, kind="link", bg="panel").pack(
            side="left", padx=(t.px(14), 0))
        self._divider()

    def _render_issue(self, issue):
        t, c, app = self.t, self.t.c, self.app
        self._caption("Grammar", pady=(18, 6))
        outer = tk.Frame(self.inner, bd=0, bg=c["grammar"])
        outer.pack(fill="x", padx=t.px(18))
        card = tk.Frame(outer, bd=0, bg=c["surface"])
        card.pack(fill="x", padx=1, pady=1)
        fix = issue["fixes"][0] if issue["fixes"] else None
        rows = [(issue["text"].strip() or "(space)", t.f_strike if fix is not None else t.f_item_bold, c["muted"],
                 (t.px(10), 0))]
        if fix is not None:
            rows += [("↓", t.f_small, c["faint"], (t.px(2), t.px(2))),
                     (fix.strip() or "(remove)", t.f_item_bold, c["text"], (0, 0))]
        rows.append((issue["message"], t.f_small, c["muted"], (t.px(8), t.px(10))))
        for text, font, fg, pady in rows:
            lbl = tk.Label(card, text=text, font=font, fg=fg, bg=c["surface"], anchor="w", justify="left",
                           wraplength=max(60, self.width - t.px(30)))
            self.wrap_labels.append((lbl, t.px(30)))
            lbl.pack(fill="x", padx=t.px(12), pady=pady)
        row = tk.Frame(self.inner, bd=0, bg=c["panel"])
        row.pack(fill="x", padx=t.px(18), pady=(t.px(10), 0))
        for n, f in enumerate(issue["fixes"][:3]):
            FlatButton(row, t, "Fix" if len(issue["fixes"]) == 1 else f.strip(), lambda n=n: app.fix_issue(n),
                       kind="primary" if n == 0 else "soft", bg="panel").pack(side="left", padx=(0, t.px(6)))
        FlatButton(row, t, "Ignore", app.ignore_issue, bg="panel").pack(side="left")
        self._divider()

    def _render_tips(self):
        self._caption("Get started", pady=(20, 8))
        for head, rest in TIPS:
            f = tk.Frame(self.inner, bd=0)
            self.t.paint(f, bg="panel")
            f.pack(fill="x", padx=self.t.px(18), pady=self.t.px(3))
            lbl = self._label(f, "", self.t.f_ui, wrap=self.t.px(4))
            lbl.configure(text=f"{head} — {rest}")
            lbl.pack(fill="x")
        if not self.app.engine.smart:
            self._divider()
            self._caption("Lite mode")
            self._label(self.inner, "Suggestions come from the offline thesaurus only. Download the small "
                        "context models for suggestions that understand the sentence.",
                        self.t.f_small, fg="muted", wrap=self.t.px(0)).pack(fill="x", padx=self.t.px(18))
            FlatButton(self.inner, self.t, "Get smart models…", self.app.open_models, kind="soft",
                       bg="panel").pack(anchor="w", padx=self.t.px(18), pady=(self.t.px(10), 0))

    def _render_target(self, t):
        kind = t.get("kind", "word")
        self._caption({"phrase": "Alternatives for this phrase", "spell": "Spelling"}.get(kind, "Alternatives for"),
                      pady=(18, 2))
        text = t["text"] if len(t["text"]) < 160 else t["text"][:157] + "…"
        font = self.t.f_item_bold if kind == "phrase" else self.t.f_title
        self._label(self.inner, text, font, fg="spell" if kind == "spell" else "text",
                    wrap=self.t.px(0)).pack(fill="x", padx=self.t.px(18))
        status = t.get("status")
        if kind == "spell" and not t.get("protected") and status != "loading":
            self._render_spelling(t)
            return
        if t.get("protected"):
            self._label(self.inner, "This part is markup (LaTeX command, maths, code or a link), so it is "
                        "left untouched.", self.t.f_small, fg="muted",
                        wrap=0).pack(fill="x", padx=self.t.px(18), pady=(self.t.px(8), 0))
            return
        if status == "loading":
            lbl = self._label(self.inner, "Thinking", self.t.f_small, fg="muted", wrap=None)
            lbl.pack(anchor="w", padx=self.t.px(18), pady=(self.t.px(10), 0))
            self._animate(lbl, "Thinking")
            return
        if t.get("error"):
            self._label(self.inner, t["error"], self.t.f_small, fg="danger", wrap=0).pack(
                fill="x", padx=self.t.px(18), pady=(self.t.px(8), 0))
            return
        groups = t.get("groups") or [("word", t["text"], [], 0)]
        for g, (gkind, heading, items, offset) in enumerate(groups):
            if g > 0:
                self._caption("Or reword the whole phrase", pady=(16, 2))
                self._label(self.inner, heading, self.t.f_item_bold, wrap=self.t.px(0)).pack(
                    fill="x", padx=self.t.px(18))
            if not items:
                msg = ("No good alternatives found here. Try selecting a longer phrase, or rephrase the whole "
                       "sentence below.") if len(groups) == 1 else "No strong single-word alternatives here."
                self._label(self.inner, msg, self.t.f_small, fg="muted", wrap=0).pack(
                    fill="x", padx=self.t.px(18), pady=(self.t.px(8), 0))
                continue
            box = tk.Frame(self.inner, bd=0)
            self.t.paint(box, bg="panel")
            box.pack(fill="x", padx=self.t.px(10), pady=(self.t.px(10), 0))
            last_note = None
            for i, s in enumerate(items):
                note = s.note if s.note != last_note else ""  # say each word sense only once
                last_note = s.note or last_note
                self._row(box, offset + i, s, note)

    def _render_spelling(self, t):
        """A word that is not in the dictionary: corrections, or accept the word."""
        px = self.t.px
        items = (t.get("groups") or [("spell", t["text"], [], 0)])[0][2]
        msg = "Not in the dictionary. Did you mean:" if items else "Not in the dictionary, and nothing similar was found."
        self._label(self.inner, t.get("error") or msg, self.t.f_small, fg="danger" if t.get("error") else "muted",
                    wrap=0).pack(fill="x", padx=px(18), pady=(px(6), 0))
        if items:
            box = tk.Frame(self.inner, bd=0)
            self.t.paint(box, bg="panel")
            box.pack(fill="x", padx=px(10), pady=(px(6), 0))
            for i, s in enumerate(items):
                self._row(box, i, s, s.note if i < 3 else "")
        links = tk.Frame(self.inner, bd=0)
        self.t.paint(links, bg="panel")
        links.pack(fill="x", padx=px(18), pady=(px(10), 0))
        FlatButton(links, self.t, "Add to dictionary", self.app.add_word, kind="link", bg="panel",
                   tip="Never mark this word again").pack(side="left")
        FlatButton(links, self.t, "Ignore", lambda: self.app.add_word(session=True), kind="link", bg="panel",
                   tip="Leave it alone until Rewrite is closed").pack(side="left", padx=(px(14), 0))

    def _row(self, parent, i, s, note=None):
        note = s.note if note is None else note
        t = self.t
        c = t.c
        row = tk.Frame(parent, bd=0, cursor="hand2", bg=c["panel"])
        row.pack(fill="x", pady=0)
        top = tk.Frame(row, bd=0, bg=c["panel"])
        top.pack(fill="x", padx=t.px(8), pady=(t.px(6), 0))
        num = tk.Label(top, text=str(i + 1) if i < 9 else "", width=2, font=t.f_small, fg=c["faint"],
                       bg=c["panel"], anchor="w")
        num.pack(side="left")
        word = tk.Label(top, text=s.text, font=t.f_item, fg=c["text"], bg=c["panel"], anchor="w",
                        justify="left", wraplength=max(60, self.width - t.px(130)))
        self.wrap_labels.append((word, t.px(130)))
        word.pack(side="left", fill="x", expand=True)
        badge = self._badge(top, s.kind, c["panel"])
        badge.pack(side="right", padx=(t.px(6), 0))
        bar = FitBar(top, t, s.fit, c["panel"])
        bar.pack(side="right", padx=(t.px(6), 0))
        widgets = [row, top, num, word, bar]
        if note:
            lbl = tk.Label(row, text=note, font=t.f_small, fg=c["muted"], bg=c["panel"], anchor="w",
                           justify="left", wraplength=max(60, self.width - t.px(40)))
            self.wrap_labels.append((lbl, t.px(40)))
            lbl.pack(fill="x", padx=(t.px(8) + t.f_small.measure("00") + t.px(4), t.px(8)))
            widgets.append(lbl)
        tk.Frame(row, height=t.px(6), bg=c["panel"]).pack(fill="x")
        widgets.append(row.winfo_children()[-1])

        def hover(on):
            bg = c["hover"] if on else c["panel"]
            for w in widgets:
                w.configure(bg=bg)

        for w in widgets + [badge]:
            w.bind("<Enter>", lambda e: hover(True))
            w.bind("<Leave>", lambda e: hover(False))
            w.bind("<Button-1>", lambda e, n=i: self.app.apply_suggestion(n))

    def _render_original(self, orig):
        self._divider()
        self._caption("Original text here")
        f = tk.Frame(self.inner, bd=0)
        self.t.paint(f, bg="panel")
        f.pack(fill="x", padx=self.t.px(18))
        txt = orig if orig.strip() else "(nothing — this text was added)"
        if len(txt) > 300:
            txt = txt[:297] + "…"
        self._label(f, txt, self.t.f_ui, fg="muted", wrap=self.t.px(0)).pack(fill="x")
        FlatButton(f, self.t, "Revert to original", self.app.revert_here, kind="link", bg="panel").pack(
            anchor="w", pady=(self.t.px(6), 0))

    def _render_sentence(self):
        self._divider()
        self._sentence_head = self._caption("Whole sentence")
        status = self.state["sentence"]
        btns = tk.Frame(self.inner, bd=0)
        self.t.paint(btns, bg="panel")
        btns.pack(fill="x", padx=self.t.px(18))
        FlatButton(btns, self.t, "Rephrase sentence", self.app.rephrase_sentence, kind="soft", bg="panel",
                   tip="Rewrite the sentence at the cursor (Ctrl+R)").pack(side="left")
        if status == "results":
            FlatButton(btns, self.t, "More variations", lambda: self.app.rephrase_sentence(more=True),
                       kind="link", bg="panel").pack(side="left", padx=(self.t.px(10), 0))
        if status == "loading":
            lbl = self._label(self.inner, "Rewriting", self.t.f_small, fg="muted", wrap=None)
            lbl.pack(anchor="w", padx=self.t.px(18), pady=(self.t.px(10), 0))
            self._animate(lbl, "Rewriting")
            return
        if status == "empty":
            self._label(self.inner, "No rewrite kept the meaning well enough. Try “More variations”.",
                        self.t.f_small, fg="muted", wrap=0).pack(fill="x", padx=self.t.px(18),
                                                                  pady=(self.t.px(8), 0))
        if status == "nomodel":
            self._label(self.inner, "Sentence rewriting works best with the paraphrase model "
                        "(Models menu). Showing thesaurus-based rewrites instead.",
                        self.t.f_small, fg="muted", wrap=0).pack(fill="x", padx=self.t.px(18),
                                                                  pady=(self.t.px(8), 0))
        for i, s in enumerate(self.state["sentences"]):
            self._card(i, s)

    def _card(self, i, s):
        t, c = self.t, self.t.c
        outer = tk.Frame(self.inner, bd=0, bg=c["border"], cursor="hand2")
        outer.pack(fill="x", padx=t.px(18), pady=(t.px(10), 0))
        card = tk.Frame(outer, bd=0, bg=c["surface"])
        card.pack(fill="x", padx=1, pady=1)
        body = tk.Label(card, text=s.text.strip(), font=t.f_ui, fg=c["text"], bg=c["surface"], anchor="w",
                        justify="left", wraplength=max(60, self.width - t.px(30)))
        self.wrap_labels.append((body, t.px(30)))
        body.pack(fill="x", padx=t.px(12), pady=(t.px(10), t.px(4)))
        meta = tk.Label(card, text=s.note, font=t.f_small, fg=c["muted"], bg=c["surface"], anchor="w")
        meta.pack(fill="x", padx=t.px(12), pady=(0, t.px(10)))
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
