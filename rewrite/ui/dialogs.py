"""Dialogs: model downloads, auto-rewrite options, help."""
import sys
import threading
import time
import tkinter as tk
from tkinter import ttk

from .. import __author__, __url__, __version__, models
from .widgets import FlatButton

MOD = "Cmd" if sys.platform == "darwin" else "Ctrl"
if sys.platform == "darwin":  # Cmd+H hides the app on a Mac, and Cmd+G is find next there
    KEY_REPLACE, KEYS_REPLACE, KEY_NEXT, KEY_PREV = "Cmd+R", "Cmd+R", "Cmd+G", "Cmd+Shift+G"
else:
    KEY_REPLACE, KEYS_REPLACE, KEY_NEXT, KEY_PREV = "Ctrl+H", "Ctrl+H or Ctrl+R", "F3", "Shift+F3"


class Dialog(tk.Toplevel):
    def __init__(self, app, title, width=560):
        super().__init__(app.root)
        self.app, self.t = app, app.theme
        self.withdraw()
        self.title(title)
        self.transient(app.root)
        self.configure(bg=self.t.c["surface"])
        self.resizable(False, False)
        self.body = tk.Frame(self, bg=self.t.c["surface"], bd=0)
        self.body.pack(fill="both", expand=True, padx=self.t.px(18), pady=(self.t.px(14), self.t.px(14)))
        self.width = self.t.px(width)
        self.bind("<Escape>", lambda e: self.close())
        self.protocol("WM_DELETE_WINDOW", self.close)
        try:
            self.iconphoto(False, app.icon)
        except Exception:
            pass

    def show(self):
        self.update_idletasks()
        r = self.app.root
        w, h = self.winfo_reqwidth(), self.winfo_reqheight()
        x = r.winfo_rootx() + (r.winfo_width() - w) // 2
        y = r.winfo_rooty() + max(0, (r.winfo_height() - h) // 3)
        self.geometry(f"+{max(0, x)}+{max(0, y)}")
        self.deiconify()
        self.lift()
        try:
            self.grab_set()
        except tk.TclError:
            pass
        self.focus_set()

    def close(self):
        try:
            self.grab_release()
        except tk.TclError:
            pass
        self.destroy()

    # helpers
    def label(self, parent, text, font=None, fg="text", wrap=True, **kw):
        c = self.t.c
        return tk.Label(parent, text=text, font=font or self.t.f_ui, fg=c[fg], bg=c["surface"], anchor="w",
                        justify="left", wraplength=self.width if wrap else 0, **kw)

    def buttons(self):
        f = tk.Frame(self.body, bg=self.t.c["surface"])
        f.pack(fill="x", pady=(self.t.px(14), 0))
        return f


class ModelsDialog(Dialog):
    def __init__(self, app, welcome=False):
        super().__init__(app, "Welcome to Rewrite" if welcome else "Smart models", 480)
        c, t = self.t.c, self.t
        self.cancel = threading.Event()
        self.running = False
        self.label(self.body, "Welcome to Rewrite" if welcome else "Smart models", t.f_title).pack(fill="x")
        self.label(self.body, (
            "Rewrite already works offline with its built-in thesaurus and phrase book. For suggestions that "
            "actually understand the sentence, it can use three small models (22M to 220M parameters) that "
            "run on your CPU."),
            fg="muted").pack(fill="x", pady=(t.px(4), t.px(8)))
        self.vars = {}
        runtime = models.runtime_available()
        for key, spec in models.MODELS.items():
            row = tk.Frame(self.body, bg=c["surface"])
            row.pack(fill="x", pady=t.px(3))
            have = models.present(key)
            v = tk.BooleanVar(value=not have)
            self.vars[key] = v
            cb = ttk.Checkbutton(row, variable=v, command=self._update_total)
            cb.pack(side="left", anchor="n", pady=(t.px(2), 0))
            if have or not runtime:
                cb.state(["disabled"])
            txt = tk.Frame(row, bg=c["surface"])
            txt.pack(side="left", fill="x", expand=True, padx=(t.px(6), 0))
            head = tk.Frame(txt, bg=c["surface"])
            head.pack(fill="x")
            self.label(head, spec["title"], t.f_ui_bold, wrap=False).pack(side="left")
            status = "installed" if have else f"{models.size_mb(key)} MB"
            self.label(head, "  ✓ " + status if have else "  " + status, t.f_small,
                       fg="good" if have else "faint", wrap=False).pack(side="left")
            self.label(txt, spec["detail"], t.f_small, fg="muted").pack(fill="x")
        if not runtime:
            self.label(self.body, (
                "The model runtime (onnxruntime) is not installed in this Python. Close Rewrite and start it "
                "with  python run.py  — it installs everything automatically."), fg="danger").pack(
                fill="x", pady=(t.px(8), 0))
        self.status = self.label(self.body, "", t.f_small, fg="muted")
        self.status.pack(fill="x", pady=(t.px(8), t.px(3)))
        self.bar = ttk.Progressbar(self.body, mode="determinate", maximum=1000)
        self.bar.pack(fill="x")
        self.bar.pack_forget()
        b = self.buttons()
        self.go = FlatButton(b, t, "Download", self.start, kind="primary", bg="surface")
        self.go.pack(side="right")
        self.later = FlatButton(b, t, "Not now" if welcome else "Close", self.close, bg="surface")
        self.later.pack(side="right", padx=(0, t.px(6)))
        self._update_total()
        if not runtime:
            self.go.set_enabled(False)
        self.show()

    def _update_total(self):
        total = sum(models.size_mb(k) for k, v in self.vars.items() if v.get() and not models.present(k))
        self.go.configure(text=f"Download  ({total} MB)" if total else "Download")
        self.go.set_enabled(bool(total) and models.runtime_available())

    def start(self):
        keys = [k for k, v in self.vars.items() if v.get() and not models.present(k)]
        if not keys or self.running:
            return
        self.running = True
        self.go.set_enabled(False)
        self.later.configure(text="Cancel")
        self.bar.pack(fill="x")
        threading.Thread(target=self._download, args=(keys,), daemon=True).start()

    def _download(self, keys):
        last = [0.0]
        for n, key in enumerate(keys, 1):
            title = models.MODELS[key]["title"]

            def progress(done, total, title=title, n=n):
                now = time.time()
                if now - last[0] < 0.1 and done < total:
                    return
                last[0] = now
                self.app.post(lambda: self._progress(f"Downloading {title} ({n}/{len(keys)})…  "
                                                     f"{done / 1e6:.0f} of {total / 1e6:.0f} MB", done / total))
            try:
                models.download(key, progress, self.cancel)
            except InterruptedError:
                self.app.post(self.destroy_safe)
                return
            except Exception as e:
                self.app.post(lambda e=e: self._failed(e))
                return
        self.app.post(lambda: self._progress("Loading models…", 1.0))
        self.app.post(lambda: self.app.reload_models(done=self._done))

    def _progress(self, text, frac):
        if self.winfo_exists():
            self.status.configure(text=text)
            self.bar["value"] = int(frac * 1000)

    def _failed(self, e):
        if not self.winfo_exists():
            return
        self.running = False
        self.status.configure(text=f"Download failed: {e}\nCheck your internet connection and try again "
                                   "(partial downloads resume).", fg=self.t.c["danger"])
        self.go.set_enabled(True)
        self.go.configure(text="Retry")
        self.later.configure(text="Close")

    def _done(self):
        if not self.winfo_exists():
            return
        self.running = False
        self.status.configure(text="All set — smart suggestions are on.", fg=self.t.c["good"])
        self.later.configure(text="Close")
        self.go.pack_forget()

    def destroy_safe(self):
        if self.winfo_exists():
            Dialog.close(self)

    def close(self):
        if self.running:
            self.cancel.set()
            return
        self.app.settings["first_run_done"] = True
        models.save_settings(self.app.settings)
        super().close()


class AutoRewriteDialog(Dialog):
    DESC = {
        "light": "Rewords wordy phrases and swaps a few words (about 15%). Safest; meaning stays put.",
        "medium": "Also replaces about 30% of the eligible words with synonyms that fit the context.",
        "strong": "Changes up to half the eligible words and rephrases whole sentences when the paraphrase "
                  "model is installed. Review the result carefully.",
    }

    def __init__(self, app, has_selection, on_run):
        super().__init__(app, "Auto-rewrite", 440)
        c, t = self.t.c, self.t
        self.on_run = on_run
        self.label(self.body, "Auto-rewrite", t.f_title).pack(fill="x")
        self.label(self.body, "Replace everything in one go, or just highlight the suggested changes and "
                              "accept or reject them one by one.", fg="muted").pack(
            fill="x", pady=(t.px(4), t.px(10)))

        self.label(self.body, "APPLY TO", t.f_tiny, fg="faint").pack(fill="x")
        self.scope = tk.StringVar(value="selection" if has_selection else "document")
        sc = tk.Frame(self.body, bg=c["surface"])
        sc.pack(fill="x", pady=(t.px(2), t.px(8)))
        r1 = ttk.Radiobutton(sc, text="Selected text", value="selection", variable=self.scope)
        r1.pack(side="left")
        if not has_selection:
            r1.state(["disabled"])
        ttk.Radiobutton(sc, text="Whole document", value="document", variable=self.scope).pack(
            side="left", padx=(t.px(12), 0))

        self.label(self.body, "STRENGTH", t.f_tiny, fg="faint").pack(fill="x")
        seg = tk.Frame(self.body, bg=c["border"])
        seg.pack(anchor="w", pady=(t.px(4), t.px(4)))
        self.strength = "medium"
        self.seg_btns = {}
        for name in ("light", "medium", "strong"):
            b = tk.Label(seg, text=name.capitalize(), font=t.f_ui, padx=t.px(12), pady=t.px(3), cursor="hand2")
            b.pack(side="left", padx=(0 if name == "light" else 1, 0), pady=1)
            b.bind("<Button-1>", lambda e, n=name: self.set_strength(n))
            self.seg_btns[name] = b
        self.desc = self.label(self.body, "", t.f_small, fg="muted")
        self.desc.pack(fill="x", pady=(0, t.px(8)))

        self.label(self.body, "WHAT TO CHANGE", t.f_tiny, fg="faint").pack(fill="x", pady=(0, t.px(2)))
        self.o_phr = tk.BooleanVar(value=True)
        self.o_words = tk.BooleanVar(value=True)
        self.o_sent = tk.BooleanVar(value=False)
        ttk.Checkbutton(self.body, text="Reword wordy / common phrases", variable=self.o_phr).pack(anchor="w")
        ttk.Checkbutton(self.body, text="Swap words for synonyms that fit the context",
                        variable=self.o_words).pack(anchor="w")
        self.cb_sent = ttk.Checkbutton(self.body, text="Rephrase whole sentences (paraphrase model)",
                                       variable=self.o_sent)
        self.cb_sent.pack(anchor="w")
        if not app.engine.t5:
            self.cb_sent.state(["disabled"])
        self.label(self.body, "Markup, maths, code, citations and links are never changed.", t.f_small,
                   fg="faint").pack(fill="x", pady=(t.px(8), 0))
        b = self.buttons()
        FlatButton(b, t, "Replace all", lambda: self.run("all"), kind="primary", bg="surface",
                   tip="Apply every change now (one undo step)").pack(side="right")
        FlatButton(b, t, "Highlight to review", lambda: self.run("review"), kind="soft", bg="surface",
                   tip="Mark the changes; click each one to accept or reject it").pack(
            side="right", padx=(0, t.px(6)))
        FlatButton(b, t, "Cancel", self.close, bg="surface").pack(side="left")
        self.set_strength("medium")
        self.bind("<Return>", lambda e: self.run("review"))
        self.show()

    def set_strength(self, name):
        self.strength = name
        c = self.t.c
        for n, b in self.seg_btns.items():
            on = n == name
            b.configure(bg=c["accent"] if on else c["surface"], fg=c["on_accent"] if on else c["text"])
        self.desc.configure(text=self.DESC[name])
        if self.app.engine.t5:
            self.o_sent.set(name == "strong")

    def run(self, apply="all"):
        opts = {"scope": self.scope.get(), "strength": self.strength, "phrases": self.o_phr.get(),
                "words": self.o_words.get(), "sentences": self.o_sent.get() and bool(self.app.engine.t5),
                "apply": apply}
        self.close()
        self.on_run(opts)


HELP = f"""How Rewrite works

Open a file (txt, md, tex, docx, odt, pdf, rtf, html, code …), drop one on the window, or just paste text. Then:

•  Click any word — the panel shows alternatives ranked by how well they fit that exact sentence. Click one (or press Alt+1 … Alt+9) to swap it in. Case and word form (plural, past tense …) are matched automatically.
•  Select a phrase — you get phrase-level rewrites: phrase-book equivalents, one-word swaps and model paraphrases.
•  {MOD}+E — rephrase the whole sentence at the cursor. Each option shows how much meaning is kept and how much was reworded. ↻ next to “Whole sentence” samples more variations.
•  Auto-rewrite ({MOD}+Shift+R) — a pass over the selection or the whole document at light / medium / strong strength. “Replace all” applies everything at once; “Highlight to review” marks the changes in blue so you can click each one and Accept (Alt+Y) or Reject (Alt+N); F8 jumps to the next one.
•  Show changes (± in the toolbar) — highlights everything that differs from the original text. Click a highlighted part to see the original and revert it.
•  Phrase hints (the light bulb) — underlines wordy or common phrases that have ready-made alternatives.
•  Spelling and grammar — a red underline marks a word that is not in the dictionary, a blue one a grammar slip (agreement, verb form, a/an, easily confused words). Click the word to see the fix, or right-click it. The two buttons next to “Spelling” add the word to your dictionary or ignore it, so it is no longer marked. Both checks can be switched off in the View menu.
•  Find and replace ({MOD}+F; {KEYS_REPLACE} opens it with replace) — a bar above the text finds as you type. Enter or {KEY_NEXT} goes to the next match, Shift+Enter or {KEY_PREV} to the previous one, Esc closes the bar. Aa matches upper and lower case, ab finds whole words only, .* takes a regular expression (\\1 in the replacement puts back the first group). “Replace all” is undone with one {MOD}+Z.
•  Preview (the eye, {MOD}+P) — opens a second panel on the right that shows the document as it will look. Markdown follows as you type; LaTeX is compiled to a PDF every time you save (it needs a LaTeX installation such as MiKTeX, MacTeX or TeX Live). ↻ or F5 refreshes or rebuilds at any time. While the preview is open, the suggestions move below the text.

The buttons are icons: rest the mouse on one to see what it does. The status bar at the bottom shows the file format (click it to change which parts are protected) and whether the smart models are loaded.

Markup is protected: LaTeX commands, maths, citations and references, Markdown code and links, HTML tags, and (in source files) everything except comments stay untouched and are shown dimmed.

Badges
  synonym — a dictionary synonym (WordNet) that also fits the context
  related — a closely related dictionary word (broader / similar term)
  in context — proposed by the language model from the context; check that it keeps your meaning
  phrase — from the built-in phrase book
  rewrite — generated by the paraphrase model

Keyboard
  {MOD}+O open   {MOD}+S save   {MOD}+Shift+S save as   {MOD}+N new
  {MOD}+Z undo   {MOD}+Y / {MOD}+Shift+Z redo
  {MOD}+F find   {KEYS_REPLACE} replace   {KEY_NEXT} / {KEY_PREV} next / previous match
  Ctrl+Space  suggestions at cursor   {MOD}+E rephrase sentence
  {MOD}+=  / {MOD}+-  text size   {MOD}+0 reset size
  {MOD}+P  preview panel   F5 refresh preview / rebuild PDF

Everything runs locally on your computer. Models: RoBERTa-base (context), all-MiniLM-L6-v2 (meaning check), and a T5-base paraphraser, run with ONNX Runtime. Thesaurus: Princeton WordNet 3.0.

A note on honesty: rewording someone else's text does not make it yours. Cite your sources — this tool is meant for improving and varying your own writing.

Rewrite {__version__} · created by {__author__} · {__url__}"""


class HelpDialog(Dialog):
    def __init__(self, app):
        super().__init__(app, "How to use Rewrite", 560)
        c, t = self.t.c, self.t
        box = tk.Frame(self.body, bg=c["surface"], bd=0)
        box.pack(fill="both", expand=True)
        # the text is longer than a small screen is tall, so it scrolls
        lines = max(14, min(30, (app.root.winfo_screenheight() - t.px(260)) // t.f_ui.metrics("linespace")))
        txt = tk.Text(box, wrap="word", bd=0, highlightthickness=0, bg=c["surface"], fg=c["text"],
                      font=t.f_ui, width=84, height=lines, padx=0, pady=0, spacing1=0, spacing3=t.px(2))
        sb = ttk.Scrollbar(box, orient="vertical", command=txt.yview)
        txt.configure(yscrollcommand=sb.set)
        sb.pack(side="right", fill="y", padx=(t.px(6), 0))
        txt.pack(side="left", fill="both", expand=True)
        txt.insert("1.0", HELP)
        txt.tag_add("h", "1.0", "1.end")
        txt.tag_configure("h", font=t.f_title, spacing3=t.px(6))
        for head in ("Badges", "Keyboard"):
            idx = txt.search(head, "1.0", "end")
            if idx:
                txt.tag_add("b", idx, f"{idx} lineend")
        txt.tag_configure("b", font=t.f_ui_bold, spacing1=t.px(5))
        txt.configure(state="disabled")
        b = self.buttons()
        FlatButton(b, t, "Close", self.close, kind="primary", bg="surface").pack(side="right")
        self.show()
