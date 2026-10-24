"""Rewrite - main window."""
import bisect
import contextlib
import difflib
import os
import queue
import re
import sys
import threading
import time
import traceback
import webbrowser
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

from . import __author__, __url__, __version__, docmodel, formats, models, native, phrases, proof
from .engine import Engine
from .lane import Lane
from .rich import RichEditor
from .textutil import WORD_RE, sentence_bounds
from .ui import objects
from .ui.dialogs import KEY_NEXT, KEY_PREV, KEY_REPLACE, MOD, AutoRewriteDialog, HelpDialog, ModelsDialog
from .ui.findbar import FindBar
from .ui.formatbar import FormatBar
from .ui.panel import SuggestionPanel
from .ui.pdfmode import PdfBar, PdfEditor
from .ui.preview import PreviewPanel
from .ui.theme import Theme
from .ui.widgets import FlatButton, IconButton, Toggle, Tooltip

ASTRAL = re.compile("[\U00010000-\U0010FFFF]")
MODKEY = "Command" if sys.platform == "darwin" else "Control"
ASSETS = os.path.join(os.path.dirname(__file__), "assets")
GRAMMAR_BATCH = 300  # sentences the context model reads per pass, nearest to the view first


PDF_HINT = "To rewrite a PDF's text, use Convert to document; the pencil tool changes a line in place."


class Snapshot:
    """Editor text at one moment, with Python offset <-> Tk index conversion."""

    astral_width = 1  # set at startup: how many Tk index units a non-BMP character takes

    def __init__(self, text):
        self.text = text
        self.line_starts = [0] + [m.end() for m in re.finditer("\n", text)]
        self.astral = [m.start() for m in ASTRAL.finditer(text)] if Snapshot.astral_width > 1 else []

    def index(self, off):
        off = max(0, min(off, len(self.text)))
        line = bisect.bisect_right(self.line_starts, off) - 1
        col = off - self.line_starts[line]
        if self.astral:
            col += bisect.bisect_left(self.astral, off) - bisect.bisect_left(self.astral, self.line_starts[line])
        return f"{line + 1}.{col}"

    def offset(self, index):
        line, col = (int(x) for x in str(index).split("."))
        line = max(1, min(line, len(self.line_starts)))
        base = self.line_starts[line - 1]
        if self.astral:
            k = 0
            for p in self.astral[bisect.bisect_left(self.astral, base):]:
                if p - base + k >= col:
                    break
                k += 1
            col -= k
        end = self.line_starts[line] - 1 if line < len(self.line_starts) else len(self.text)
        return min(base + col, end)


def word_tokens(text, start=0, end=None):
    end = len(text) if end is None else end
    return [(m.start(), m.end(), m.group().lower()) for m in WORD_RE.finditer(text, start, end)]


def compute_changes(orig, cur):
    """Word-level diff (line diff first, so it stays fast on big documents).

    Returns (changes [(cur_s, cur_e, orig_s, orig_e)], share of current words that changed)."""
    def lines(t):
        out, pos = [], 0
        for ln in t.split("\n"):
            out.append((pos, pos + len(ln), ln))
            pos += len(ln) + 1
        return out

    la, lb = lines(orig), lines(cur)
    sm = difflib.SequenceMatcher(None, [x[2] for x in la], [x[2] for x in lb], autojunk=False)
    changes, changed_words = [], 0
    total = len(WORD_RE.findall(cur))
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal":
            continue
        a0 = la[i1][0] if i1 < len(la) else len(orig)
        a1 = la[i2 - 1][1] if i2 > i1 else a0
        b0 = lb[j1][0] if j1 < len(lb) else len(cur)
        b1 = lb[j2 - 1][1] if j2 > j1 else b0
        wa, wb = word_tokens(orig, a0, a1), word_tokens(cur, b0, b1)
        wsm = difflib.SequenceMatcher(None, [w[2] for w in wa], [w[2] for w in wb], autojunk=False)
        for wop, x1, x2, y1, y2 in wsm.get_opcodes():
            if wop == "equal" or y2 == y1:
                continue
            changed_words += y2 - y1
            os_ = wa[x1][0] if x2 > x1 else (wa[x1][0] if x1 < len(wa) else a1)
            oe = wa[x2 - 1][1] if x2 > x1 else os_
            changes.append((wb[y1][0], wb[y2 - 1][1], os_, oe))
    return changes, (changed_words / total if total else 0.0)


TOKEN_RE = re.compile(r"\w+(?:['’-]\w+)*|\s+|[^\w\s]")


def diff_chunks(old, new):
    """The minimal changed pieces between two versions of a sentence: [(old_start, old_end, replacement)]."""
    a, b = list(TOKEN_RE.finditer(old)), list(TOKEN_RE.finditer(new))
    sm = difflib.SequenceMatcher(None, [m.group() for m in a], [m.group() for m in b], autojunk=False)
    pieces = []
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal":
            continue
        os_ = a[i1].start() if i1 < len(a) else len(old)
        oe = a[i2 - 1].end() if i2 > i1 else os_
        ns = b[j1].start() if j1 < len(b) else len(new)
        ne = b[j2 - 1].end() if j2 > j1 else ns
        if pieces and old[pieces[-1][1]:os_] == new[pieces[-1][3]:ns] and not old[pieces[-1][1]:os_].strip():
            pieces[-1][1], pieces[-1][3] = oe, ne  # only a space in between: one change, not two
        else:
            pieces.append([os_, oe, ns, ne])
    out = []
    for os_, oe, ns, ne in pieces:
        rep = new[ns:ne]
        if os_ == oe:  # pure insertion: attach it to a neighbouring word so it can be highlighted
            m = re.compile(r"\S+").search(old, oe)
            if m:
                oe, rep = m.end(), rep + old[os_:m.end()]
            else:
                prev = list(re.finditer(r"\S+", old[:os_]))
                if not prev:
                    continue
                os_, rep = prev[-1].start(), old[prev[-1].start():os_] + rep
        while os_ < oe and rep and old[os_] == rep[0] and old[os_].isspace():
            os_, rep = os_ + 1, rep[1:]
        while os_ < oe and rep and old[oe - 1] == rep[-1] and old[oe - 1].isspace():
            oe, rep = oe - 1, rep[:-1]
        if oe > os_ and old[os_:oe] != rep:
            out.append((os_, oe, rep))
    return out


class App:
    def __init__(self, root, path=None):
        self.root = root
        self.settings = models.load_settings()
        self.q = queue.Queue()
        self.theme = Theme(root, self.settings.get("theme", "light"),
                           self.settings.get("font_family") or "serif",
                           int(self.settings.get("font_size") or models.DEFAULTS["font_size"]))
        self.theme.style_ttk()
        self.icon = None
        try:
            self.icon = tk.PhotoImage(file=os.path.join(ASSETS, "icon64.png"))
            root.iconphoto(True, self.icon)
        except tk.TclError:
            pass
        self.engine = Engine(self.settings.get("user_words", []))
        self.doc = formats.Document()
        self.original = ""
        self.snap = Snapshot("")
        self.snap_dirty = False
        self.protected, self.hint_spans, self.changes = [], [], []
        self.change_ratio = 0.0
        self.target = None
        self.sent_target = None
        self.sent_items = []
        self.variation = 0
        self.busy = False
        self.dirty = False
        self.cache = {}
        self._jobs = {}
        self._popup_pending = False
        self.current_change = None
        self.proposals = {}  # review mode: id -> {"old", "new"}; position held by marks p<id>s / p<id>e
        self._next_pid = 0
        self.issues = []  # proofreading: proof.Issue list for the current snapshot, sorted by position
        self.ignored = set()  # grammar remarks dismissed in this session: (flagged text, message)
        self._proof_gen = 0
        self._proof_base, self._proof_more, self._found = None, False, []
        self._gram_cache = {}  # sentence -> context-model grammar slips, so only edited sentences are rechecked
        self.preview_on = False
        self.mode = "plain"  # plain (text) | rich (formatted document) | pdf
        self.cancel_flag = threading.Event()
        self.models_state = "loading"
        self.lane_word = Lane(self, "words")
        self.lane_sent = Lane(self, "sentences")
        self.lane_slow = Lane(self, "slow")
        self.lane_diff = Lane(self, "diff")
        self.lane_proof = Lane(self, "proof")
        self._detect_astral()
        self.build()
        self._accept_drops()
        if self.settings.get("preview"):
            self.set_preview(True, save=False)
        self.root.protocol("WM_DELETE_WINDOW", self.quit)
        self.root.after(30, self.poll)
        if path:
            self.root.after(50, lambda: self.open_file(path))
        else:
            self.set_text("")
        self.reload_models()
        if not self.settings.get("first_run_done") and not all(models.present(k) for k in models.MODELS):
            self.root.after(700, lambda: ModelsDialog(self, welcome=True))

    # -- plumbing -----------------------------------------------------------------
    def post(self, fn):
        """Run fn on the Tk thread (safe to call from worker threads)."""
        self.q.put(fn)

    def poll(self):
        try:
            while True:
                fn = self.q.get_nowait()
                try:
                    fn()
                except Exception:
                    traceback.print_exc()
        except queue.Empty:
            pass
        self.root.after(30, self.poll)

    def later(self, name, ms, fn):
        """Debounce: run fn after ms, cancelling a previous call with the same name."""
        if name in self._jobs:
            self.root.after_cancel(self._jobs[name])
        self._jobs[name] = self.root.after(ms, lambda: (self._jobs.pop(name, None), fn()))

    def _detect_astral(self):
        try:
            t = tk.Text(self.root)
            t.insert("1.0", "\U0001F600")
            Snapshot.astral_width = int(t.count("1.0", "end-1c", "chars")[0])
            t.destroy()
        except Exception:
            Snapshot.astral_width = 1

    def refresh_snapshot(self):
        if self.snap_dirty or self.snap.text is None:
            self.snap = Snapshot(self.editor_text())
            self.snap_dirty = False
        return self.snap

    def editor_text(self):
        """The editor's text; in a formatted document every embedded object counts as one U+FFFC, so
        that offsets in it match the widget's indices."""
        return self.rich.plain() if self.rich.active else self.text.get("1.0", "end-1c")

    # -- UI construction ------------------------------------------------------------
    def build(self):
        t, c = self.theme, self.theme.c
        r = self.root
        r.title("Rewrite")
        geo = self.settings.get("geometry")
        if geo and re.match(r"^\d+x\d+[+-]-?\d+[+-]-?\d+$", geo):
            r.geometry(geo)
        else:
            w = min(int(r.winfo_screenwidth() * 0.82), t.px(1360))
            h = min(int(r.winfo_screenheight() * 0.82), t.px(860))
            r.geometry(f"{w}x{h}+{(r.winfo_screenwidth() - w) // 2}+{max(0, (r.winfo_screenheight() - h) // 3)}")
        r.minsize(t.px(760), t.px(480))
        self.build_menu()

        # toolbar: icon buttons, each one explained by its tooltip
        bar = tk.Frame(r, bd=0)
        t.paint(bar, bg="bg")
        bar.pack(fill="x", padx=t.px(6), pady=t.px(3))
        self.toolbar = bar
        IconButton(bar, t, "open", self.open_file, f"Open a file ({MOD}+O)").pack(side="left")
        IconButton(bar, t, "save", self.save, f"Save ({MOD}+S)").pack(side="left")
        self._sep(bar)
        IconButton(bar, t, "undo", self.undo, f"Undo ({MOD}+Z)").pack(side="left")
        IconButton(bar, t, "redo", self.redo, f"Redo ({MOD}+Y)").pack(side="left")
        IconButton(bar, t, "search", lambda: self.find.open(),
                   f"Find and replace ({MOD}+F, {KEY_REPLACE})").pack(side="left")
        IconButton(bar, t, "export", self.export_dialog,
                   f"Export or convert: PDF, Word, OpenDocument, Markdown, LaTeX, text … ({MOD}+Shift+E)").pack(side="left")
        self._sep(bar)
        IconButton(bar, t, "pencil", self.rephrase_sentence,
                   f"Rephrase the sentence at the cursor ({MOD}+E)").pack(side="left")
        FlatButton(bar, t, "Auto-rewrite…", self.auto_rewrite, kind="primary", icon="wand", pad=(8, 5),
                   tip=f"Rewrite the selection or the whole document ({MOD}+Shift+R)").pack(
            side="left", padx=(t.px(4), 0))

        self.theme_btn = IconButton(bar, t, "moon" if t.name == "light" else "sun", self.toggle_theme,
                                    "Light / dark theme")
        self.theme_btn.pack(side="right")
        self._sep(bar, side="right")
        self.t_changes = Toggle(bar, t, "diff", self.settings.get("show_changes", True), self.set_show_changes,
                                tip="Show changes: highlight everything that differs from the original")
        self.t_changes.pack(side="right", padx=(t.px(2), 0))
        self.t_hints = Toggle(bar, t, "bulb", self.settings.get("phrase_hints", True), self.set_hints,
                              tip="Phrase hints: underline wordy phrases that have ready alternatives")
        self.t_hints.pack(side="right", padx=(t.px(2), 0))
        self.t_preview = Toggle(bar, t, "eye", False, self.set_preview,
                                tip=f"Preview: the document as it will look: Markdown, the LaTeX PDF, or a formatted document as it prints ({MOD}+P)")
        self.t_preview.pack(side="right")

        # body: editor | suggestions. With the preview open: (editor over suggestions) | preview.
        # The three parts are children of the window, so they can move between the two panes.
        self.pane = ttk.PanedWindow(r, orient="horizontal")
        self.pane.pack(fill="both", expand=True)
        self.left = ttk.PanedWindow(r, orient="vertical")
        ed = tk.Frame(r, bd=0)
        self.editor_frame = ed
        t.paint(ed, bg="surface")
        # width/height=1: let the paned window, not the text's own request, decide its size
        self.text = tk.Text(ed, wrap="word", undo=True, maxundo=-1, autoseparators=True, bd=0, width=1, height=1,
                            highlightthickness=0, font=t.f_editor, padx=t.px(24), pady=t.px(14),
                            spacing1=t.px(1), spacing2=t.px(3), spacing3=t.px(6), insertwidth=t.px(2),
                            exportselection=True, relief="flat", cursor="xterm")
        self.vsb = ttk.Scrollbar(ed, orient="vertical", command=self.text.yview)
        self.text.configure(yscrollcommand=self._on_scroll)
        self.vsb.pack(side="right", fill="y")
        self.text.pack(side="left", fill="both", expand=True)
        self.find = FindBar(ed, self)  # shown above the text by Ctrl+F
        self.color_editor()
        self.rich = RichEditor(self)
        self.formatbar = FormatBar(r, self)  # shown under the toolbar for formatted documents
        self.rich.on_change = self.formatbar.refresh
        self.pdfview = PdfEditor(r, self)  # takes the editor's place for PDF files
        self.pdfbar = PdfBar(r, self)
        self.panel_frame = tk.Frame(r, bd=0, width=t.px(320), height=t.px(220))
        self.panel_frame.pack_propagate(False)
        t.paint(self.panel_frame, bg="panel")
        self.panel = SuggestionPanel(self.panel_frame, self)
        self.panel.pack(fill="both", expand=True)
        self.preview = PreviewPanel(r, self)
        self.left.add(ed, weight=5)
        self.pane.add(self.left, weight=5)
        self.pane.add(self.panel_frame, weight=0)
        r.after(100, self._place_sash)

        # status bar: file and format, word count | messages, model status
        sb = tk.Frame(r, bd=0)
        t.paint(sb, bg="bg")
        sb.pack(fill="x", side="bottom", before=self.pane)
        self.statusbar = sb
        self.status_file = FlatButton(sb, t, "", self.pick_kind, tip="File format (click to change)",
                                      font=t.f_small, pad=(6, 2))
        self.status_file.pack(side="left", padx=(t.px(2), 0))
        self.status_stats = tk.Label(sb, text="", font=t.f_small, anchor="w", bd=0)
        t.paint(self.status_stats, bg="bg", fg="muted")
        self.status_stats.pack(side="left", padx=(t.px(6), 0))
        self.mode_btn = FlatButton(sb, t, "Loading models…", self.open_models, font=t.f_small, pad=(6, 2))
        self.mode_tip = Tooltip(self.mode_btn, t, "Models")
        self.mode_btn.pack(side="right", padx=(0, t.px(2)))
        self.status_right = tk.Frame(sb, bd=0)
        t.paint(self.status_right, bg="bg")
        self.status_right.pack(side="right", padx=(0, t.px(8)))
        self.progress = ttk.Progressbar(self.status_right, mode="determinate", length=t.px(120), maximum=1000)
        self.progress_lbl = tk.Label(self.status_right, text="", font=t.f_small, bd=0)
        t.paint(self.progress_lbl, bg="bg", fg="muted")
        self.cancel_btn = FlatButton(self.status_right, t, "Cancel", self.cancel_auto, kind="link", font=t.f_small)
        self.msg = tk.Label(self.status_right, text="", font=t.f_small, bd=0, anchor="w")  # long: cut at the end
        t.paint(self.msg, bg="bg", fg="muted")
        self.msg.pack(side="right")

        self.bind_keys()
        self.rich.install_keys()
        self.text.bind("<Configure>", self._center_column)
        r.bind_all("<MouseWheel>", self._wheel, add="+")
        r.bind_all("<Button-4>", self._wheel, add="+")
        r.bind_all("<Button-5>", self._wheel, add="+")
        self.text.focus_set()

    def _sep(self, bar, side="left"):
        f = tk.Frame(bar, width=1, height=self.theme.px(16), bd=0)
        self.theme.paint(f, bg="border")
        f.pack(side=side, padx=self.theme.px(5))

    def _place_sash(self):
        try:
            self.root.update_idletasks()
            w, px = self.pane.winfo_width(), self.theme.px
            if w < px(500):
                self.root.after(100, self._place_sash)
                return
            if self.preview_on:
                self.pane.sashpos(0, max(px(360), int(w * float(self.settings.get("preview_split", 0.5)))))
                self.root.update_idletasks()
                h = self.left.winfo_height()
                self.left.sashpos(0, max(px(140), h - max(px(150), int(h * 0.3))))
            else:
                self.pane.sashpos(0, max(px(360), w - px(320)))
        except tk.TclError:
            pass

    def set_preview(self, on, save=True):
        """Show or hide the preview. While it is open, the suggestions sit below the editor."""
        on = bool(on)
        if on and self.mode == "pdf":
            self.t_preview.set(False)
            self.preview_var.set(False)
            self.flash_msg("A PDF is shown as it is; the preview is for text and formatted documents.")
            return
        if on != self.preview_on:
            if on:
                self.pane.forget(self.panel_frame)
                self.left.add(self.panel_frame, weight=0)
                self.pane.add(self.preview, weight=4)
            else:
                try:  # remember how wide the user made it
                    self.settings["preview_split"] = round(self.pane.sashpos(0) / max(1, self.pane.winfo_width()), 3)
                except tk.TclError:
                    pass
                self.pane.forget(self.preview)
                self.left.forget(self.panel_frame)
                self.pane.add(self.panel_frame, weight=0)
            self.preview_on = on
            self.root.after(60, self._place_sash)
        self.t_preview.set(on)
        self.preview_var.set(on)
        if save:
            self.settings["preview"] = on
            models.save_settings(self.settings)
        if on:
            self.preview.refresh()
        else:
            self.preview.stop()
        self.text.focus_set()

    def refresh_preview(self):
        if self.preview_on:
            self.preview.refresh(manual=True)
        else:
            self.set_preview(True)

    def doc_kind_label(self):
        return formats.KIND_LABEL.get(self.doc.kind, self.doc.kind)

    def goto_line(self, line):
        """Put the cursor on a line of the editor (used by the LaTeX error list)."""
        last = int(self.text.index("end-1c").split(".")[0])
        line = max(1, min(int(line), last))
        self.text.tag_remove("sel", "1.0", "end")
        self.text.mark_set("insert", f"{line}.0")
        self.text.see(f"{line}.0")
        self.text.tag_remove("flash", "1.0", "end")
        self.text.tag_add("flash", f"{line}.0", f"{line}.0 lineend")
        self.root.after(900, lambda: self.text.tag_remove("flash", "1.0", "end"))
        self.text.focus_set()
        self.update_target()

    # -- drag and drop ------------------------------------------------------------------
    def _accept_drops(self):
        """Let files be dropped anywhere on the window (needs the optional tkinterdnd2 package)."""
        try:
            from tkinterdnd2 import DND_FILES, TkinterDnD
            TkinterDnD.require(self.root)
            for w in (self.toolbar, self.pane, self.left, self.editor_frame, self.text, self.panel_frame,
                      self.preview, self.statusbar):
                w.drop_target_register(DND_FILES)
                w.dnd_bind("<<DropEnter>>", self._drop_enter)
                w.dnd_bind("<<DropLeave>>", lambda e: self.flash_msg("", 1))
                w.dnd_bind("<<Drop>>", self._drop)
            self.can_drop = True
        except Exception:  # package missing or no tkdnd build for this platform: Open still works
            self.can_drop = False

    def _drop_enter(self, e):
        self.flash_msg("Drop the file to open it", 60000)
        return e.action

    def _drop(self, e):
        self.flash_msg("", 1)
        try:
            paths = [p for p in self.root.tk.splitlist(e.data) if os.path.isfile(p)]
        except tk.TclError:
            paths = []
        if not paths:
            self.flash_msg("Drop a file, not a folder.")
        else:
            # open after the drop has returned, so the program the file came from is not kept waiting
            self.root.after(60, lambda: self._open_dropped(paths))
        return e.action

    def _open_dropped(self, paths):
        self.root.lift()
        self.root.focus_force()
        if self.busy:
            self.flash_msg("Wait for the rewrite to finish, then drop the file again.")
        elif self.open_file(paths[0]) and len(paths) > 1:
            self.flash_msg(f"Opened {os.path.basename(paths[0])} — Rewrite works on one file at a time.")

    def _center_column(self, e=None):
        """Keep lines at a comfortable reading width on wide windows."""
        width = self.text.winfo_width()
        col = self.theme.f_editor.measure("n") * 84
        pad = max(self.theme.px(18), (width - col) // 2)
        if int(str(self.text.cget("padx"))) != pad:
            self.text.configure(padx=pad)
        self.find.align(pad)

    def color_editor(self):
        c, t = self.theme.c, self.text
        t.configure(bg=c["surface"], fg=c["text"], insertbackground=c["text"], selectbackground=c["select"],
                    selectforeground=c["text"], inactiveselectbackground=c["select"])
        t.tag_configure("prot", foreground=c["protected"])
        t.tag_configure("hint", background=c["hint"] if self.settings.get("phrase_hints", True) else "",
                        underline=bool(self.settings.get("phrase_hints", True)))
        t.tag_configure("chg", background=c["changed"] if self.settings.get("show_changes", True) else "")
        t.tag_configure("prop", background=c["proposal"], underline=True)
        t.tag_configure("target", background=c["target"])
        t.tag_configure("flash", background=c["flash"])
        t.tag_configure("found", background=c["found"])
        for tag, key in (("gram", "grammar"), ("spell", "spell")):
            try:
                t.tag_configure(tag, underline=True, underlinefg=c[key])
            except tk.TclError:  # Tk older than 8.6.6 cannot colour underlines
                t.tag_configure(tag, underline=True, foreground=c[key])
        for tag in ("hint", "chg", "gram", "spell", "prop", "found", "target", "flash"):
            t.tag_raise(tag)
        t.tag_raise("sel")

    def build_menu(self):
        m = tk.Menu(self.root)
        acc = MOD

        def menu(label):
            sub = tk.Menu(m, tearoff=0)
            m.add_cascade(label=label, menu=sub)
            return sub

        f = menu("File")
        f.add_command(label="New text", accelerator=f"{acc}+N", command=self.new_file)
        f.add_command(label="New formatted document", command=lambda: self.new_file(rich=True))
        f.add_command(label="Open…", accelerator=f"{acc}+O", command=self.open_file)
        self.recent_menu = tk.Menu(f, tearoff=0)
        f.add_cascade(label="Open recent", menu=self.recent_menu)
        f.add_separator()
        f.add_command(label="Save", accelerator=f"{acc}+S", command=self.save)
        f.add_command(label="Save as…", accelerator=f"{acc}+Shift+S", command=lambda: self.save(as_=True))
        f.add_command(label="Export / convert…", accelerator=f"{acc}+Shift+E", command=self.export_dialog)
        f.add_separator()
        f.add_command(label="Page set-up…", command=self.page_setup)
        f.add_separator()
        f.add_command(label="Quit", command=self.quit)
        self.file_menu = f
        e = menu("Edit")
        e.add_command(label="Undo", accelerator=f"{acc}+Z", command=self.undo)
        e.add_command(label="Redo", accelerator=f"{acc}+Y", command=self.redo)
        e.add_separator()
        e.add_command(label="Cut", command=lambda: self.text.event_generate("<<Cut>>"))
        e.add_command(label="Copy", command=lambda: self.text.event_generate("<<Copy>>"))
        e.add_command(label="Paste", command=lambda: self.text.event_generate("<<Paste>>"))
        e.add_command(label="Select all", accelerator=f"{acc}+A", command=self.select_all)
        e.add_separator()
        e.add_command(label="Find…", accelerator=f"{acc}+F", command=lambda: self.find.open())
        e.add_command(label="Replace…", accelerator=KEY_REPLACE, command=lambda: self.find.open(replace=True))
        e.add_command(label="Find next", accelerator=KEY_NEXT, command=lambda: self.find.next())
        e.add_command(label="Find previous", accelerator=KEY_PREV, command=lambda: self.find.next(back=True))
        e.add_separator()
        e.add_command(label="Use current text as the original", command=self.mark_original)
        e.add_command(label="Revert everything to the original", command=self.revert_all)
        ins = menu("Insert")
        for label, kind in (("Picture…", "image"), ("Table…", "table"), ("Chart…", "chart"), ("Formula…", "math"),
                            ("Footnote…", "note"), ("Link…", "link"), ("Horizontal line", "rule"),
                            ("Page break", "pagebreak")):
            ins.add_command(label=label, accelerator=f"{acc}+K" if kind == "link" else "",
                            command=lambda k=kind: self.rich_insert(k))
        self.insert_menu = ins
        fm = menu("Format")
        for label, name, key in (("Bold", "b", f"{acc}+B"), ("Italic", "i", f"{acc}+I"), ("Underline", "u", f"{acc}+U"),
                                 ("Strikethrough", "s", ""), ("Superscript", "sup", ""), ("Subscript", "sub", "")):
            fm.add_command(label=label, accelerator=key, command=lambda n=name: self._fmt(lambda r: r.toggle(n)))
        fm.add_command(label="Clear formatting", command=lambda: self._fmt(lambda r: r.clear_format()))
        fm.add_separator()
        styles = tk.Menu(fm, tearoff=0)
        for s in ("normal", "title", "subtitle", "h1", "h2", "h3", "quote", "caption", "code"):
            styles.add_command(label={"code": "Code"}.get(s, docmodel.STYLE_LABEL.get(s, s)),
                               command=lambda s=s: self._fmt(lambda r: r.set_para(style=s)))
        fm.add_cascade(label="Paragraph style", menu=styles)
        for label, a in (("Align left", None), ("Centre", "center"), ("Align right", "right"), ("Justify", "justify")):
            fm.add_command(label=label, command=lambda a=a: self._fmt(lambda r: r.set_para(align=a)))
        fm.add_separator()
        fm.add_command(label="Bulleted list", command=lambda: self._fmt(lambda r: r.toggle_list("bullet")))
        fm.add_command(label="Numbered list", command=lambda: self._fmt(lambda r: r.toggle_list("number")))
        fm.add_command(label="Increase indent", command=lambda: self._fmt(lambda r: r.change_level(1)))
        fm.add_command(label="Decrease indent", command=lambda: self._fmt(lambda r: r.change_level(-1)))
        self.format_menu = fm
        w = menu("Rewrite")
        w.add_command(label="Suggestions at cursor", accelerator="Ctrl+Space", command=self.popup_at_cursor)
        w.add_command(label="Rephrase sentence", accelerator=f"{acc}+E", command=self.rephrase_sentence)
        w.add_command(label="Auto-rewrite…", accelerator=f"{acc}+Shift+R", command=self.auto_rewrite)
        v = menu("View")
        v.add_command(label="Zoom in", accelerator=f"{acc}+=", command=lambda: self.zoom(1))
        v.add_command(label="Zoom out", accelerator=f"{acc}+-", command=lambda: self.zoom(-1))
        v.add_command(label="Actual size", accelerator=f"{acc}+0", command=lambda: self.zoom(0))
        v.add_separator()
        self.font_var = tk.StringVar(value=self.theme.editor_family)
        for fam, label in (("serif", "Serif font"), ("sans", "Sans-serif font"), ("mono", "Monospace font")):
            v.add_radiobutton(label=label, value=fam, variable=self.font_var, command=self.set_font_family)
        v.add_separator()
        self.spell_var = tk.BooleanVar(value=bool(self.settings.get("spell_check", True)))
        self.gram_var = tk.BooleanVar(value=bool(self.settings.get("grammar_check", True)))
        self.preview_var = tk.BooleanVar(value=False)
        v.add_checkbutton(label="Check spelling (red underline)", variable=self.spell_var, command=self.set_proofing)
        v.add_checkbutton(label="Check grammar (blue underline)", variable=self.gram_var, command=self.set_proofing)
        v.add_separator()
        v.add_checkbutton(label="Preview panel", accelerator=f"{acc}+P", variable=self.preview_var,
                          command=lambda: self.set_preview(self.preview_var.get()))
        v.add_command(label="Refresh preview / rebuild PDF", accelerator="F5", command=self.refresh_preview)
        v.add_separator()
        v.add_command(label="Light / dark theme", command=self.toggle_theme)
        mm = menu("Models")
        mm.add_command(label="Manage smart models…", command=self.open_models)
        h = menu("Help")
        h.add_command(label="How to use Rewrite", accelerator="F1", command=lambda: HelpDialog(self))
        h.add_command(label="About", command=self.about)
        self.root.configure(menu=m)
        self.update_recent_menu()

    def bind_keys(self):
        t, r = self.text, self.root
        K = MODKEY

        def both(seq, fn):
            for w in (t, r):
                w.bind(seq, lambda e: (fn(), "break")[1])

        both(f"<{K}-o>", self.open_file)
        both(f"<{K}-O>", self.open_file)
        both(f"<{K}-s>", self.save)
        both(f"<{K}-S>", lambda: self.save(as_=True))
        both(f"<{K}-Shift-s>", lambda: self.save(as_=True))
        both(f"<{K}-n>", self.new_file)
        both(f"<{K}-e>", self.rephrase_sentence)
        both(f"<{K}-E>", self.export_dialog)
        both(f"<{K}-Shift-e>", self.export_dialog)
        both(f"<{K}-f>", self.find.open)
        both(f"<{K}-r>", lambda: self.find.open(replace=True))
        if sys.platform == "darwin":  # Cmd+H hides the app; Cmd+G finds the next match
            both("<Command-g>", self.find.next)
            both("<Command-G>", lambda: self.find.next(back=True))
            both("<Command-Shift-g>", lambda: self.find.next(back=True))
        else:
            both(f"<{K}-h>", lambda: self.find.open(replace=True))
            both("<F3>", self.find.next)
            both("<Shift-F3>", lambda: self.find.next(back=True))
        both(f"<{K}-R>", self.auto_rewrite)
        both(f"<{K}-Shift-r>", self.auto_rewrite)
        both(f"<{K}-y>", self.redo)
        both(f"<{K}-Z>", self.redo)
        both(f"<{K}-Shift-z>", self.redo)
        both(f"<{K}-z>", self.undo)
        both(f"<{K}-a>", self.select_all)
        both(f"<{K}-equal>", lambda: self.zoom(1))
        both(f"<{K}-plus>", lambda: self.zoom(1))
        both(f"<{K}-minus>", lambda: self.zoom(-1))
        both(f"<{K}-Key-0>", lambda: self.zoom(0))
        both("<Control-space>", self.popup_at_cursor)
        both("<F1>", lambda: HelpDialog(self))
        both("<F5>", self.refresh_preview)
        both(f"<{K}-p>", lambda: self.set_preview(not self.preview_on))
        both("<F8>", self.next_prop)
        for mod in ("Alt", "Option") if sys.platform == "darwin" else ("Alt",):
            try:
                t.bind(f"<{mod}-y>", lambda e: (self.accept_prop(), "break")[1])
                t.bind(f"<{mod}-n>", lambda e: (self.reject_prop(), "break")[1])
            except tk.TclError:
                pass
        for n in range(1, 10):
            for mod in ("Alt", "Option") if sys.platform == "darwin" else ("Alt",):
                try:
                    t.bind(f"<{mod}-Key-{n}>", lambda e, n=n: (self.apply_suggestion(n - 1), "break")[1])
                except tk.TclError:
                    pass
        t.bind("<<Modified>>", self.on_modified)
        t.bind("<KeyRelease>", self._on_key)
        t.bind("<ButtonRelease-1>", lambda e: self.later("target", 60, self.update_target))
        t.bind("<<Selection>>", self._on_selection)
        t.bind("<Escape>", lambda e: (self.find.close(), "break")[1] if self.find.visible else None)
        t.bind("<<Paste>>", self._on_paste, add="+")
        if sys.platform == "darwin":
            t.bind("<Button-2>", self.context_menu)
            t.bind("<Control-Button-1>", self.context_menu)
        else:
            t.bind("<Button-3>", self.context_menu)

    def _on_selection(self, _=None):
        self.later("target", 280, self.update_target)
        if self.find.visible:
            self.find.show_count()

    def _on_key(self, e):
        if e.keysym in ("Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R", "Meta_L", "Meta_R"):
            return
        self.later("target", 350, self.update_target)

    def _wheel(self, e):
        try:
            w = self.root.winfo_containing(e.x_root, e.y_root)
        except (KeyError, tk.TclError):
            return
        if w is not None and self.panel.scroll.contains(w):
            self.panel.scroll.wheel(e)

    # -- documents -------------------------------------------------------------------
    def set_mode(self, mode):
        """plain: text files; rich: formatted documents (the format bar shows); pdf: the PDF view."""
        if mode == self.mode:
            return
        if self.mode == "rich":
            self.rich.detach()
            self.formatbar.pack_forget()
        if self.mode == "pdf":
            self.pdfbar.pack_forget()
            self.left.insert(0, self.editor_frame, weight=5)
            self.left.forget(self.pdfview)
            self.pane.add(self.panel_frame, weight=0)
            self.root.after(60, self._place_sash)
        self.mode = mode
        if mode == "rich":
            self.formatbar.pack(fill="x", padx=self.theme.px(6), pady=(0, self.theme.px(3)), after=self.toolbar)
        if mode == "pdf":
            if self.preview_on:
                self.set_preview(False, save=False)
            self.pdfbar.pack(fill="x", padx=self.theme.px(6), pady=(0, self.theme.px(3)), after=self.toolbar)
            self.left.insert(0, self.pdfview, weight=5)
            self.left.forget(self.editor_frame)
            self.pane.forget(self.panel_frame)
        for menu in (self.insert_menu, self.format_menu):
            last = menu.index("end")
            for k in range(0, (last or 0) + 1):
                try:
                    menu.entryconfigure(k, state="normal" if mode == "rich" else "disabled")
                except tk.TclError:
                    pass

    def set_text(self, text, original=None):
        self.set_mode("plain")
        self.text.configure(state="normal")
        self.clear_proposals()
        self.text.delete("1.0", "end")
        self.text.insert("1.0", text)
        self.text.edit_reset()
        self.text.edit_modified(False)
        self.text.mark_set("insert", "1.0")
        self._fresh(text, original)

    def set_rich(self, model):
        """Show a formatted document (a docmodel Document) for editing."""
        self.set_mode("rich")
        self.text.configure(state="normal")
        self.clear_proposals()
        self.rich.load(model)
        self.text.edit_modified(False)
        self.rich_original = model
        self._fresh(self.rich.plain(), None)
        self.formatbar.refresh()

    def _fresh(self, text, original):
        """A document was just put in the editor: forget everything about the previous one."""
        self.original = text if original is None else original
        self.snap = Snapshot(text)
        self.snap_dirty = False
        self.dirty = False
        self.target = None
        self.sent_items = []
        self.cache.clear()
        self.issues = []
        self.panel.state.update(target=None, sentence="idle", sentences=[], orig=None, issue=None)
        self.panel.render()
        self.update_title()
        self.analyze()
        if self.preview_on:
            self.preview.refresh()

    def update_title(self):
        name = self.doc.name
        dot = "• " if self.dirty else ""
        self.root.title(f"{dot}{name} — Rewrite")
        kind = formats.KIND_LABEL.get(self.doc.kind, self.doc.kind)
        if self.doc.kind in ("rich", "pdf") and self.doc.ext:
            kind = formats.FORMAT_LABEL.get(self.doc.ext, kind)
        self.status_file.configure(text=f"{name}  ·  {kind}")

    def confirm_discard(self):
        if not self.dirty:
            return True
        ans = messagebox.askyesnocancel("Unsaved changes", f"Save changes to {self.doc.name}?", parent=self.root)
        if ans is None:
            return False
        if ans:
            return self.save()
        return True

    def new_file(self, rich=False):
        if self.busy or not self.confirm_discard():
            return
        if rich:
            if not self._native_ready():
                return
            self.doc = formats.Document(kind="rich", model=docmodel.Document())
            self.set_rich(self.doc.model)
        else:
            self.doc = formats.Document()
            self.set_text("")

    def _native_ready(self):
        if native.available():
            return True
        messagebox.showerror("Document engine missing",
                             "Formatted documents and PDF need Rewrite's document engine, which is not built.\n\n"
                             "Start Rewrite with  python run.py  so it can build it (it needs Rust from rustup.rs).\n\n"
                             f"{native.ERROR or ''}", parent=self.root)
        return False

    def open_file(self, path=None):
        if self.busy or not self.confirm_discard():
            return False
        if not path:
            path = filedialog.askopenfilename(parent=self.root, title="Open a document",
                                              filetypes=formats.OPEN_TYPES,
                                              initialdir=self.settings.get("last_dir") or os.path.expanduser("~"))
        if not path:
            return False
        ext = os.path.splitext(path)[1].lower()
        if (ext in formats.RICH or ext == ".pdf") and not self._native_ready():
            return False
        try:
            self.root.configure(cursor="watch")
            self.root.update_idletasks()
            doc = formats.load(path)
        except Exception as e:
            messagebox.showerror("Could not open file", f"{os.path.basename(path)}\n\n{e}", parent=self.root)
            return False
        finally:
            self.root.configure(cursor="")
        self.doc = doc
        self.settings["last_dir"] = os.path.dirname(path)
        recent = [p for p in self.settings.get("recent", []) if p != path]
        self.settings["recent"] = [path] + recent[:7]
        models.save_settings(self.settings)
        self.update_recent_menu()
        if doc.kind == "rich":
            self.set_rich(doc.model)
        elif doc.kind == "pdf":
            self.open_pdf(doc)
        else:
            self.set_text(doc.text)
        return True

    def update_recent_menu(self):
        self.recent_menu.delete(0, "end")
        rec = [p for p in self.settings.get("recent", []) if os.path.exists(p)]
        if not rec:
            self.recent_menu.add_command(label="(none)", state="disabled")
        for p in rec:
            self.recent_menu.add_command(label=p, command=lambda p=p: self.open_file(p))

    def save(self, as_=False):
        if self.busy:
            return False
        if self.mode == "pdf":
            return self.pdfview.save(as_)
        path = self.doc.path
        rich = self.mode == "rich"
        if rich and path and os.path.splitext(path)[1].lower() not in formats.SAVE_RICH:
            path = None  # a template or other read-only kind: save under a new name
        if as_ or not path:
            base, ext = os.path.splitext(os.path.basename(path or self.doc.path or "")) if (path or self.doc.path) \
                else ("untitled", "")
            if rich:
                ext = ext if ext in (".docx", ".odt", ".fodt", ".rtf", ".doc") else ".docx"
                types = sorted(formats.SAVE_RICH_TYPES, key=lambda x: x[1] != "*" + ext)
            else:
                ext = ext or {"latex": ".tex", "markdown": ".md", "html": ".html"}.get(self.doc.kind, ".txt")
                types = [("Same as original", "*" + ext), ("Text", "*.txt"), ("Markdown", "*.md"), ("LaTeX", "*.tex"),
                         ("All files", "*.*")]
            path = filedialog.asksaveasfilename(
                parent=self.root, title="Save as", initialfile=base + ext, defaultextension=ext,
                initialdir=os.path.dirname(self.doc.path) if self.doc.path else self.settings.get("last_dir"),
                filetypes=types)
            if not path:
                return False
        try:
            if rich:
                if os.path.splitext(path)[1].lower() not in formats.SAVE_RICH:
                    path += ".docx"
                self.root.configure(cursor="watch")
                self.root.update_idletasks()
                model = self.rich.document()
                formats.save_rich(model, path)
                self.doc.path, self.doc.source_ext = path, os.path.splitext(path)[1].lower()
                self.doc.model = model
            else:
                formats.save(self.doc, self.text.get("1.0", "end-1c"), path)
        except Exception as e:
            messagebox.showerror("Could not save", str(e), parent=self.root)
            return False
        finally:
            self.root.configure(cursor="")
        self.dirty = False
        self.update_title()
        self.flash_msg(f"Saved {os.path.basename(path)}")
        if self.preview_on:
            self.preview.refresh()  # LaTeX is rebuilt on every save
        return True

    def pick_kind(self):
        if self.mode != "plain":
            self.flash_msg("Formatted documents and PDFs keep their format; use Export to convert them.")
            return
        m = tk.Menu(self.root, tearoff=0)
        self._kind_var = tk.StringVar(value=self.doc.kind)
        m.add_command(label="Which parts are protected from changes?", state="disabled")
        for k, label in formats.KIND_LABEL.items():
            m.add_radiobutton(label=label, value=k, variable=self._kind_var, command=lambda k=k: self.set_kind(k))
        m.update_idletasks()
        x = self.status_file.winfo_rootx()
        y = self.status_file.winfo_rooty() - m.winfo_reqheight()
        try:
            m.tk_popup(x, y)
        finally:
            m.grab_release()

    def set_kind(self, kind):
        self.doc.kind = kind
        self.update_title()
        self.analyze()
        if self.preview_on:
            self.preview.refresh()

    def _on_paste(self, e):
        if self.mode == "plain" and not self.text.get("1.0", "end-1c").strip():
            def after():
                txt = self.text.get("1.0", "end-1c")
                if not self.doc.path:
                    self.original = txt
                    if self.doc.kind == "plain":
                        self.doc.kind = formats.sniff_kind(txt)
                        self.update_title()
                    self.analyze()
                    if self.preview_on:
                        self.preview.refresh()
            self.root.after(10, after)

    # -- analysis (protected spans, hints, changes, stats) ------------------------------
    def on_modified(self, _=None):
        if not self.text.edit_modified():
            return
        self.text.edit_modified(False)
        self.snap_dirty = True
        if not self.dirty:
            self.dirty = True
            self.update_title()
        self.later("analyze", 400, self.analyze)

    def analyze(self):
        snap = self.refresh_snapshot()
        text = snap.text
        if self.proposals:
            before = len(self.proposals)
            self._prune_props()
            if len(self.proposals) != before:
                self.update_review()
        st = self.sent_target
        if st and text[st[0]:st[1]] != st[2]:  # the sentence was edited: its rewrites are stale
            self.sent_target, self.sent_items = None, []
            self.panel.set_sentence("idle")
        if self.rich.active:
            self.protected = formats.merge(self.rich.protected(snap) +
                                           formats.protected_spans(text, "plain"))
        else:
            self.protected = formats.protected_spans(text, self.doc.kind, self.doc.path)
        self._retag("prot", self.protected, snap)
        hints = []
        for s, e, _ in phrases.find(text):
            if not formats.overlaps(self.protected, s, e):
                hints.append((s, e))
        self.hint_spans = hints
        self._retag("hint", hints, snap)
        orig = self.original
        self.lane_diff.submit(lambda: compute_changes(orig, text), lambda r, err: self._changes_done(snap, r, err))
        self.proofread(snap)
        if self.preview_on:
            self.preview.text_changed()
        if self.find.visible:
            self.find.search()

    def _retag(self, tag, spans, snap):
        self.text.tag_remove(tag, "1.0", "end")
        if spans:
            idx = []
            for s, e in spans:
                idx.append(snap.index(s))
                idx.append(snap.index(e))
            for k in range(0, len(idx), 2000):
                self.text.tag_add(tag, *idx[k:k + 2000])

    def _changes_done(self, snap, res, err):
        if err or snap is not self.snap:
            return  # the text moved on; a newer diff is coming
        self.changes, self.change_ratio = res
        self._retag("chg", [(a, b) for a, b, _, _ in self.changes], snap)
        self.update_stats()
        self.update_original_section()

    # -- proofreading: spelling (red) and grammar (blue) underlines ---------------------------
    def set_proofing(self):
        self.settings["spell_check"] = bool(self.spell_var.get())
        self.settings["grammar_check"] = bool(self.gram_var.get())
        models.save_settings(self.settings)
        self.analyze()
        self.later("target", 30, lambda: self.update_target(force=True))

    def _on_scroll(self, first, last):
        self.vsb.set(first, last)
        if self.find.visible:
            self.later("find-view", 80, self.find.paint)
        if self._proof_more:  # a long text is checked around the view; follow it
            self.later("proof-view", 600, self._proof_view)

    def _proof_view(self):
        base = self._proof_base
        if base and base[0] is self.snap and not self.snap_dirty and not self.busy:
            self.proofread(self.snap, base[1])

    def proofread(self, snap, base=None):
        """Check the snapshot in the background; underlines appear as the results come in.
        `base` = the spelling and rule remarks of this snapshot, when they are already known."""
        self._proof_gen += 1
        gen = self._proof_gen
        spell = bool(self.settings.get("spell_check", True))
        gram = bool(self.settings.get("grammar_check", True))
        if not (spell or gram) or not snap.text.strip():
            self._proof_done(gen, snap, [])
            return
        text, prot, eng, cache = snap.text, self.protected, self.engine, self._gram_cache
        lines = self.rich.active
        try:
            focus = snap.offset(self.text.index("@0,0"))
        except (ValueError, tk.TclError):
            focus = 0
        interactive = (self.lane_word, self.lane_sent, self.lane_slow)

        def job():
            first = base
            if first is None:
                first = []
                if spell:
                    first += [proof.Issue(s, e, "spell", "", ()) for s, e in eng.speller.check(text, prot)]
                if gram:
                    first += proof.grammar_rules(text, prot, eng.speller)
                self.post(lambda: self._proof_done(gen, snap, first))
            if not (gram and eng.mlm):
                return
            # The context model reads one sentence at a time, starting with what is on screen. A long
            # text is not read to the end: the rest follows when the view moves there.
            slips, budget, more = [], GRAMMAR_BATCH, False
            last, fresh = time.time(), False
            for s, e in sorted(sentence_bounds(text, lines), key=lambda se: abs(se[0] - focus)):
                sent, spans = text[s:e], formats.clip(prot, s, e)
                key = (sent, tuple(spans))
                found = cache.get(key)
                if found is None:
                    if budget <= 0:
                        more = True
                        continue
                    budget -= 1
                    while any(lane.active for lane in interactive):  # suggestions the user asked for come first
                        time.sleep(0.05)
                    if gen != self._proof_gen:
                        return  # the text changed; a newer check has started
                    found = cache[key] = eng.grammar(sent, spans)
                    if len(cache) > 8000:
                        cache.pop(next(iter(cache)))
                for a, b, msg, fixes in found:
                    slips.append(proof.Issue(s + a, s + b, "gram", msg, fixes))
                    fresh = True
                if fresh and time.time() - last > 0.5:
                    last, fresh = time.time(), False
                    self.post(lambda part=list(slips): self._proof_done(gen, snap, first, part, True))
            self.post(lambda: self._proof_done(gen, snap, first, slips, more))

        self.lane_proof.submit(job)

    def _proof_done(self, gen, snap, base, slips=(), more=False):
        if gen != self._proof_gen or snap is not self.snap:
            return
        self._proof_base, self._proof_more = (snap, base), more
        self._found = list(base) + list(slips)
        self._show_issues()

    def _show_issues(self):
        snap = self._proof_base[0]
        text = snap.text
        self.issues = [it for it in proof.dedupe(self._found)
                       if (text[it.start:it.end], it.message) not in self.ignored]
        self._issues_snap = snap
        self._retag("spell", [(it.start, it.end) for it in self.issues if it.kind == "spell"], snap)
        self._retag("gram", [(it.start, it.end) for it in self.issues if it.kind == "gram"], snap)
        self.update_issue()
        t = self.target
        if t and t["kind"] in ("word", "spell") and                 (t["kind"] == "spell") != bool(self.issue_at(t["start"], t["end"], "spell")):
            self.later("target", 10, lambda: self.update_target(force=True))  # the word's verdict changed

    def issue_at(self, s, e=None, kind=None):
        """The proofreading remark that touches [s, e], if the remarks still match the text."""
        if getattr(self, "_issues_snap", None) is not self.snap or self.snap_dirty:
            return None
        e = s if e is None else e
        for it in self.issues:
            if it.start <= e and s <= it.end and (kind is None or it.kind == kind):
                return it
        return None

    def update_issue(self):
        """Show the grammar remark under the cursor (or selection) in the panel."""
        it = None
        if not self.busy:
            try:
                sel = self.text.tag_ranges("sel")
                a = self.snap.offset(self.text.index(sel[0] if sel else "insert"))
                b = self.snap.offset(self.text.index(sel[1])) if sel else a
                it = self.issue_at(a, b, "gram")
            except (ValueError, tk.TclError):
                it = None
        self.current_issue = it
        self.panel.set_issue({"text": self.snap.text[it.start:it.end], "message": it.message,
                              "fixes": list(it.fixes)} if it else None)

    def fix_issue(self, n=0):
        it = self.current_issue
        if self.busy or not it or n >= len(it.fixes) or self.issue_at(it.start, it.end, "gram") is not it:
            return
        if self.replace(it.start, it.end, self.snap.text[it.start:it.end], it.fixes[n]):
            self.later("target", 30, lambda: self.update_target(force=True))

    def ignore_issue(self):
        it = self.current_issue
        if it and self.issue_at(it.start, it.end, "gram") is it:
            self.ignored.add((self.snap.text[it.start:it.end], it.message))
            self._show_issues()

    def add_word(self, session=False):
        """Accept the misspelled word under the cursor: for good, or (Ignore) until the app closes."""
        t = self.target
        if not t or t["kind"] != "spell":
            return
        word = t["text"]
        self.engine.speller.add(word, session=session)
        if not session:
            words = self.settings.setdefault("user_words", [])
            if word.lower() not in words:
                words.append(word.lower())
            models.save_settings(self.settings)
            self.flash_msg(f"Added “{word}” to your dictionary.")
        self.analyze()
        self.later("target", 30, lambda: self.update_target(force=True))

    def update_stats(self):
        n = len(WORD_RE.findall(self.snap.text))
        parts = [f"{n:,} words"]
        if self.original and self.original != self.snap.text:
            parts.append(f"{self.change_ratio * 100:.0f}% reworded vs. original")
        self.status_stats.configure(text="   ·   ".join(parts))

    def change_at(self, s, e=None):
        e = s if e is None else e
        for ch in self.changes:
            if ch[0] <= e and s <= ch[1]:
                return ch
        return None

    def update_original_section(self):
        if not self.original or self.busy:
            self.panel.set_original(None)
            return
        snap = self.snap
        if self.target:
            ch = self.change_at(self.target["start"], self.target["end"])
        else:
            try:
                ch = self.change_at(snap.offset(self.text.index("insert")))
            except (ValueError, tk.TclError):
                ch = None
        self.current_change = ch
        self.panel.set_original(self.original[ch[2]:ch[3]] if ch else None)

    # -- targets & suggestions --------------------------------------------------------
    def _context(self, text, s, e):
        """The sentence(s) around [s, e) plus the previous sentence, as (ctx_start, ctx_end)."""
        ws = max(0, s - 3000)
        para = text.rfind("\n\n", ws, s)
        ws = para + 2 if para >= 0 else ws
        we = text.find("\n\n", e, e + 3000)
        we = we if we >= 0 else min(len(text), e + 3000)
        sents = [(ws + a, ws + b) for a, b in sentence_bounds(text[ws:we], self.rich.active)]
        cur = [k for k, (a, b) in enumerate(sents) if a <= e and s <= b]
        if not cur:
            return max(ws, s - 300), min(we, e + 300)
        first, last = cur[0], cur[-1]
        cs = sents[first - 1][0] if first > 0 and sents[first][0] - sents[first - 1][1] < 3 else sents[first][0]
        return min(cs, s), max(sents[last][1], e)

    def _sentence_at(self, text, off):
        ws = max(0, off - 4000)
        we = min(len(text), off + 4000)
        for a, b in sentence_bounds(text[ws:we], self.rich.active):
            if ws + a <= off <= ws + b:
                return ws + a, ws + b
        return None

    def update_target(self, force=False):
        if self.busy:
            return
        self.update_review()
        snap = self.refresh_snapshot()
        text = snap.text
        s = e = None
        kind = "word"
        sel = self.text.tag_ranges("sel")
        if sel:
            s, e = snap.offset(self.text.index(sel[0])), snap.offset(self.text.index(sel[1]))
            while s < e and not text[s].isalnum():
                s += 1
            while e > s and not (text[e - 1].isalnum() or text[e - 1] == "'"):
                e -= 1
            n = len(WORD_RE.findall(text[s:e]))
            if n == 0 or e - s > 700:
                s = e = None
            elif n >= 2:
                kind = "phrase"
            else:
                m = next((m for m in WORD_RE.finditer(text, max(0, s - 40), min(len(text), e + 40))
                          if m.start() <= s < m.end()), None)
                s, e = (m.start(), m.end()) if m else (None, None)
        hint = None
        if not sel:
            off = snap.offset(self.text.index("insert"))
            if self.settings.get("phrase_hints", True):
                hint = next(((a, b) for a, b in self.hint_spans
                             if a <= off <= b and " " in text[a:b] and phrases.lookup(text[a:b])), None)
            m = next((m for m in WORD_RE.finditer(text, max(0, off - 60), min(len(text), off + 60))
                      if m.start() <= off <= m.end()), None)
            if m:
                s, e = m.start(), m.end()
            elif hint:
                (s, e), kind, hint = hint, "phrase", None
            if hint and (s, e) == hint:
                hint = None
        self.update_issue()
        if s is None:
            self.text.tag_remove("target", "1.0", "end")
            if self.target is not None:
                self.target = None
                self.panel.set_target(None)
            self.update_original_section()
            return
        typo = self.issue_at(s, e, "spell") if kind == "word" else None
        if typo:
            s, e, kind, hint = typo.start, typo.end, "spell", None
        if self.target and not force and (self.target["start"], self.target["end"], self.target["text"]) == \
                (s, e, text[s:e]):
            return
        self.text.tag_remove("target", "1.0", "end")
        self.text.tag_add("target", snap.index(s), snap.index(e))
        target = {"start": s, "end": e, "text": text[s:e], "kind": kind, "items": None, "hint": hint,
                  "hint_text": text[hint[0]:hint[1]] if hint else None}
        self.target = target
        self.update_original_section()
        if formats.overlaps(self.protected, s, e):
            target["items"] = []
            self.panel.set_target({"text": target["text"], "kind": kind, "protected": True})
            return
        cs, ce = self._context(text, min(s, hint[0]) if hint else s, max(e, hint[1]) if hint else e)
        ctx, a, b = text[cs:ce], s - cs, e - cs
        ha, hb = (hint[0] - cs, hint[1] - cs) if hint else (0, 0)
        key = (ctx, a, b, ha, hb, kind == "spell", self.engine.smart, bool(self.engine.t5))
        if key in self.cache:
            self.on_suggestions(target, self.cache[key], None)
            return
        self.panel.set_target({"text": target["text"], "kind": kind, "status": "loading"})
        eng = self.engine

        def job():
            if kind == "spell":
                return eng.spell_suggest(ctx, a, b, n=8), []
            words = eng.suggest(ctx, a, b, n=12)
            # the cursor sits inside a highlighted phrase: offer whole-phrase rewrites too
            phrase = eng.suggest(ctx, ha, hb, n=5, generate=False) if hint else []
            return words, phrase

        def done(res, err, target=target, key=key):
            if not err:
                self.cache[key] = res
                if len(self.cache) > 300:
                    self.cache.pop(next(iter(self.cache)))
            self.on_suggestions(target, res, err)

        self.lane_word.submit(job, done)

    def on_suggestions(self, target, res, err):
        if target is not self.target:
            return
        words, phrase = res or ([], [])
        items = [(x, target["start"], target["end"], target["text"]) for x in words]
        groups = [(target["kind"], target["text"], list(words), 0)]
        if target["hint"] and phrase:
            hs, he = target["hint"]
            groups.append(("hint", target["hint_text"], list(phrase), len(items)))
            items += [(x, hs, he, target["hint_text"]) for x in phrase]
        target["items"] = items
        self.panel.set_target({"text": target["text"], "kind": target["kind"], "groups": groups,
                               "error": f"Something went wrong: {err}" if err else None})
        if self._popup_pending:
            self._popup_pending = False
            self.popup_at_cursor()

    def apply_suggestion(self, n):
        t = self.target
        if self.busy or not t or not t.get("items") or n >= len(t["items"]):
            return
        sug, s, e, old = t["items"][n]
        if not self.replace(s, e, old, sug.text):
            return
        if t["kind"] == "phrase" and (s, e) == (t["start"], t["end"]):
            snap = self.refresh_snapshot()
            self.text.tag_remove("sel", "1.0", "end")
            self.text.tag_add("sel", snap.index(s), snap.index(s + len(sug.text)))
        self.later("target", 30, self.update_target)

    @contextlib.contextmanager
    def one_step(self):
        """Make the edits inside one step for undo. With automatic separators, Tk would split a delete
        and the insert after it, and one Ctrl+Z would leave the text with the old words gone."""
        self.text.configure(autoseparators=False)
        self.text.edit_separator()
        try:
            yield
        finally:
            self.text.edit_separator()
            self.text.configure(autoseparators=True)

    def replace(self, s, e, expected, new, select=False):
        snap = self.refresh_snapshot()
        if snap.text[s:e] != expected:
            self.root.bell()
            self.flash_msg("The text changed — click the word again.")
            return False
        with self.one_step():
            self._put_text(snap, s, e, new)
        self.snap_dirty = True
        snap = self.refresh_snapshot()
        j1, j2 = snap.index(s), snap.index(s + len(new))
        self.text.mark_set("insert", j2)
        self.text.tag_remove("flash", "1.0", "end")
        self.text.tag_add("flash", j1, j2)
        self.root.after(700, lambda: self.text.tag_remove("flash", "1.0", "end"))
        self.text.see("insert")
        self.analyze()
        return True

    def _put_text(self, snap, s, e, new):
        """Replace snap.text[s:e] with new. In a formatted document only the words that differ are
        replaced, so everything else keeps its formatting and paragraphs keep their styles."""
        if self.rich.active:
            pieces = diff_chunks(snap.text[s:e], new)
            if pieces or snap.text[s:e] == new:
                for a, b, rep in reversed(pieces):
                    i1, i2 = snap.index(s + a), snap.index(s + b)
                    self.text.delete(i1, i2)
                    self.text.insert(i1, rep)
                return
        i1, i2 = snap.index(s), snap.index(e)
        self.text.delete(i1, i2)
        self.text.insert(i1, new)

    def revert_here(self):
        ch = getattr(self, "current_change", None)
        if not ch:
            return
        snap = self.refresh_snapshot()
        cs, ce, os_, oe = ch
        self.replace(cs, ce, snap.text[cs:ce], self.original[os_:oe])
        self.later("target", 30, lambda: self.update_target(force=True))

    def revert_all(self):
        if self.busy or not self.original:
            return
        if self.mode == "rich":
            if messagebox.askyesno("Revert everything", "Go back to the document as it was opened? "
                                   "(This cannot be undone.)", parent=self.root):
                self.rich.load(self.rich_original)
                self.snap_dirty = True
                self.analyze()
            return
        if messagebox.askyesno("Revert everything", "Replace the text with the original version? "
                               "(You can undo this.)", parent=self.root):
            with self.one_step():
                self.text.delete("1.0", "end")
                self.text.insert("1.0", self.original)

    def mark_original(self):
        self.original = self.refresh_snapshot().text
        self.analyze()
        self.flash_msg("Current text is now the original for change tracking.")

    # -- sentences ------------------------------------------------------------------------
    def rephrase_sentence(self, more=False):
        if self.busy:
            return
        if self.mode == "pdf":
            self.flash_msg(PDF_HINT, 8000)
            return
        snap = self.refresh_snapshot()
        text = snap.text
        if more and self.sent_target and text[self.sent_target[0]:self.sent_target[1]] == self.sent_target[2]:
            s, e = self.sent_target[0], self.sent_target[1]
            self.variation += 1
        else:
            sel = self.text.tag_ranges("sel")
            off = snap.offset(self.text.index(sel[0] if sel else "insert"))
            span = self._sentence_at(text, off)
            if not span:
                self.flash_msg("Put the cursor inside a sentence first.")
                return
            s, e = span
            self.variation = 0
        self.sent_target = (s, e, text[s:e])
        spans = formats.clip(self.protected, s, e)
        self.panel.set_sentence("loading")
        sentence, var, eng = text[s:e], self.variation, self.engine
        tgt = self.sent_target

        def job():
            return eng.paraphrase(sentence, spans, n=5, variation=var)

        def done(res, err):
            if tgt is not self.sent_target:
                return
            if err:
                self.sent_items = []
                self.panel.set_sentence("empty")
                self.flash_msg(f"Rephrasing failed: {err}")
                return
            self.sent_items = res or []
            status = "results" if self.sent_items else "empty"
            if not eng.t5 and self.sent_items:
                status = "results"
            self.panel.set_sentence(status, self.sent_items)
            if not eng.t5:
                self.flash_msg("Tip: the paraphrase model gives much better sentence rewrites (Models menu).")

        self.lane_sent.submit(job, done)

    def apply_sentence(self, n):
        if self.busy or not self.sent_target or n >= len(self.sent_items):
            return
        s, e, old = self.sent_target
        new = self.sent_items[n].text
        if self.replace(s, e, old, new):
            self.sent_target = None
            self.sent_items = []
            self.panel.set_sentence("idle")

    # -- auto rewrite -------------------------------------------------------------------
    def auto_rewrite(self):
        if self.busy:
            return
        if self.mode == "pdf":
            self.flash_msg(PDF_HINT, 8000)
            return
        AutoRewriteDialog(self, bool(self.text.tag_ranges("sel")), self._run_auto)

    def _run_auto(self, opts):
        snap = self.refresh_snapshot()
        text = snap.text
        if not text.strip():
            return
        regions = [(0, len(text))]
        sel = self.text.tag_ranges("sel")
        if opts["scope"] == "selection" and sel:
            regions = [(snap.offset(self.text.index(sel[0])), snap.offset(self.text.index(sel[1])))]
        self.clear_proposals()
        self.busy = True
        self.cancel_flag = threading.Event()
        self.text.configure(state="disabled")
        self.msg.pack_forget()
        self.progress["value"] = 0
        self.cancel_btn.pack(side="right", padx=(self.theme.px(8), 0))
        self.progress.pack(side="right")
        self.progress_lbl.configure(text="Rewriting…")
        self.progress_lbl.pack(side="right", padx=(0, self.theme.px(8)))
        prot, eng, flag = list(self.protected), self.engine, self.cancel_flag
        started = time.time()

        def progress(k, n):
            self.post(lambda: self._auto_progress(k, n, started))

        opts = dict(opts, lines=self.rich.active)

        def job():
            return eng.auto_rewrite(text, regions, prot, opts, progress=progress, cancelled=flag.is_set)

        self.lane_slow.submit(job, lambda r, err: self._auto_done(snap, r, err, opts))

    def _auto_progress(self, k, n, started):
        if not self.busy:
            return
        self.progress["value"] = int(1000 * k / max(1, n))
        eta = ""
        if k >= 2:
            rem = (time.time() - started) / k * (n - k)
            eta = f" · about {int(rem) + 1}s left" if rem < 90 else f" · about {int(rem / 60) + 1} min left"
        self.progress_lbl.configure(text=f"Rewriting sentence {min(k + 1, n)} of {n}{eta}")

    def cancel_auto(self):
        if self.busy:
            self.cancel_flag.set()
            self.progress_lbl.configure(text="Stopping…")

    def _auto_done(self, snap, edits, err, opts=None):
        self.busy = False
        self.text.configure(state="normal")
        for w in (self.progress, self.progress_lbl, self.cancel_btn):
            w.pack_forget()
        self.msg.pack(side="right")
        if err:
            messagebox.showerror("Auto-rewrite failed", str(err), parent=self.root)
            return
        edits = edits or []
        if self.editor_text() != snap.text:
            messagebox.showwarning("Auto-rewrite", "The text changed while rewriting; nothing was applied.",
                                   parent=self.root)
            return
        if not edits:
            self.flash_msg("Nothing to change at this strength.")
            return
        if (opts or {}).get("apply") == "review":
            n = self._make_proposals(snap, edits)
            self.flash_msg(f"{n} suggested change{'s' if n != 1 else ''} highlighted — "
                           "click one to accept or reject it.", 9000)
            self.next_prop(start="1.0", inclusive=True)
            return
        with self.one_step():
            for s, e, new, _whole in sorted(edits, reverse=True):
                self._put_text(snap, s, e, new)
        self.snap_dirty = True
        if not self.settings.get("show_changes", True):
            self.t_changes.set(True)
            self.set_show_changes(True)
        self.analyze()
        self.flash_msg(f"Rewrote {len(edits)} sentence{'s' if len(edits) != 1 else ''} "
                       f"— {MOD}+Z undoes the whole pass.")

    # -- review mode: highlighted suggestions accepted one by one ---------------------------
    def _make_proposals(self, snap, edits):
        self.clear_proposals()
        for s, e, new, whole in sorted(edits):
            old = snap.text[s:e]
            for a, b, rep in ([(0, len(old), new)] if whole else diff_chunks(old, new)):
                pid = self._next_pid
                self._next_pid += 1
                i1, i2 = snap.index(s + a), snap.index(s + b)
                # marks move with the text, so suggestions survive edits made elsewhere
                self.text.mark_set(f"p{pid}s", i1)
                self.text.mark_gravity(f"p{pid}s", "right")
                self.text.mark_set(f"p{pid}e", i2)
                self.text.mark_gravity(f"p{pid}e", "left")
                self.text.tag_add("prop", i1, i2)
                self.proposals[pid] = {"old": old[a:b], "new": rep}
        self.update_review()
        return len(self.proposals)

    def _prop_range(self, pid):
        return self.text.index(f"p{pid}s"), self.text.index(f"p{pid}e")

    def _prop_order(self):
        def key(pid):
            line, col = self.text.index(f"p{pid}s").split(".")
            return int(line), int(col)
        return sorted(self.proposals, key=key)

    def _drop_prop(self, pid):
        if pid not in self.proposals:
            return
        i1, i2 = self._prop_range(pid)
        if self.text.compare(i1, "<", i2):
            self.text.tag_remove("prop", i1, i2)
        self.text.mark_unset(f"p{pid}s", f"p{pid}e")
        del self.proposals[pid]

    def _prune_props(self):
        """Drop suggestions whose text the user has edited by hand."""
        for pid in list(self.proposals):
            i1, i2 = self._prop_range(pid)
            if self.text.get(i1, i2) != self.proposals[pid]["old"]:
                self._drop_prop(pid)

    def clear_proposals(self):
        for pid in list(self.proposals):
            self.text.mark_unset(f"p{pid}s", f"p{pid}e")
        self.proposals.clear()
        self.text.tag_remove("prop", "1.0", "end")
        self.update_review()

    def prop_at(self, index="insert"):
        for pid in self.proposals:
            i1, i2 = self._prop_range(pid)
            if self.text.compare(i1, "<=", index) and self.text.compare(index, "<=", i2):
                return pid
        return None

    def update_review(self):
        if not self.proposals:
            self.panel.set_review(None)
            return
        pid = self.prop_at("insert")
        cur = None
        if pid is not None:
            p = self.proposals[pid]
            cur = {"old": p["old"], "new": p["new"], "pos": self._prop_order().index(pid) + 1, "pid": pid}
        self.panel.set_review({"pending": len(self.proposals), "current": cur})

    def next_prop(self, start=None, inclusive=False):
        order = self._prop_order()
        if not order:
            self.update_review()
            self.later("target", 10, lambda: self.update_target(force=True))
            return
        start = self.text.index(start or "insert")
        op = ">=" if inclusive else ">"
        pid = next((p for p in order if self.text.compare(f"p{p}s", op, start)), order[0])  # wraps around
        self.text.tag_remove("sel", "1.0", "end")
        self.text.mark_set("insert", f"p{pid}s")
        self.text.see(f"p{pid}e")
        self.text.see(f"p{pid}s")
        self.text.focus_set()
        self.update_target()

    def accept_prop(self):
        if self.busy:
            return
        pid = self.prop_at("insert")
        if pid is None:
            if self.proposals:
                self.next_prop()
            return
        p = self.proposals[pid]
        i1, i2 = self._prop_range(pid)
        if self.text.get(i1, i2) != p["old"]:
            self._drop_prop(pid)
            self.flash_msg("That text was edited, so its suggestion was dropped.")
        else:
            with self.one_step():
                self.text.delete(i1, i2)
                self.text.insert(i1, p["new"], ())
            self._drop_prop(pid)
            end = self.text.index(f"{i1}+{len(p['new'])}c")
            self.text.tag_remove("flash", "1.0", "end")
            self.text.tag_add("flash", i1, end)
            self.root.after(700, lambda: self.text.tag_remove("flash", "1.0", "end"))
            self.snap_dirty = True
            self.analyze()
            i1 = end
        self.next_prop(start=i1, inclusive=True)

    def reject_prop(self):
        pid = self.prop_at("insert")
        if pid is None:
            if self.proposals:
                self.next_prop()
            return
        i1, _ = self._prop_range(pid)
        self._drop_prop(pid)
        self.next_prop(start=i1, inclusive=True)

    def accept_all_props(self):
        if self.busy or not self.proposals:
            return
        n = 0
        with self.one_step():
            for pid in reversed(self._prop_order()):  # back to front keeps earlier positions valid
                p = self.proposals[pid]
                i1, i2 = self._prop_range(pid)
                if self.text.get(i1, i2) == p["old"]:
                    self.text.delete(i1, i2)
                    self.text.insert(i1, p["new"], ())
                    n += 1
        self.clear_proposals()
        self.snap_dirty = True
        self.analyze()
        self.flash_msg(f"Accepted {n} change{'s' if n != 1 else ''} — {MOD}+Z undoes them all.")
        self.later("target", 10, lambda: self.update_target(force=True))

    def reject_all_props(self):
        n = len(self.proposals)
        self.clear_proposals()
        self.flash_msg(f"Dismissed {n} suggestion{'s' if n != 1 else ''}.")
        self.later("target", 10, lambda: self.update_target(force=True))

    # -- popups -----------------------------------------------------------------------------
    def context_menu(self, e):
        idx = self.text.index(f"@{e.x},{e.y}")
        sel = self.text.tag_ranges("sel")
        if not sel or not (self.text.compare(sel[0], "<=", idx) and self.text.compare(idx, "<=", sel[1])):
            self.text.tag_remove("sel", "1.0", "end")
            self.text.mark_set("insert", idx)
        self.update_target()
        self._show_menu(e.x_root, e.y_root)
        return "break"

    def popup_at_cursor(self):
        if self.mode == "pdf":
            return
        self.update_target()
        t = self.target
        if t and t.get("items") is None:
            self._popup_pending = True
            return
        bbox = self.text.bbox("insert")
        if not bbox:
            return
        x = self.text.winfo_rootx() + bbox[0]
        y = self.text.winfo_rooty() + bbox[1] + bbox[3] + self.theme.px(4)
        self._show_menu(x, y)

    def _show_menu(self, x, y):
        m = tk.Menu(self.root, tearoff=0)
        t = self.target
        pid = self.prop_at("insert")
        if pid is not None:
            new = self.proposals[pid]["new"].strip() or "(remove)"
            m.add_command(label=f"Accept: {new[:60]}", command=self.accept_prop)
            m.add_command(label="Reject suggestion", command=self.reject_prop)
            m.add_separator()
        it = getattr(self, "current_issue", None)
        if it:
            for n, fix in enumerate(it.fixes[:3]):
                m.add_command(label=f"Fix: {fix.strip() or '(remove)'}", accelerator="grammar",
                              command=lambda n=n: self.fix_issue(n))
            m.add_command(label="Ignore this remark", command=self.ignore_issue)
            m.add_separator()
        if t and t.get("kind") == "spell" and t.get("items") is not None:
            if not t["items"]:
                m.add_command(label="No spelling suggestions", state="disabled")
            for i, (s, _, _, _) in enumerate(t["items"][:8]):
                m.add_command(label=s.text, accelerator="spelling", command=lambda i=i: self.apply_suggestion(i))
            m.add_separator()
            m.add_command(label=f"Add “{t['text']}” to dictionary", command=self.add_word)
            m.add_command(label="Ignore this word", command=lambda: self.add_word(session=True))
            m.add_separator()
        elif t and t.get("items"):
            last_range = None
            for i, (s, a, b, _) in enumerate(t["items"][:14]):
                if last_range and last_range != (a, b):
                    m.add_separator()
                last_range = (a, b)
                m.add_command(label=s.text, accelerator=s.kind, command=lambda i=i: self.apply_suggestion(i))
            m.add_separator()
        elif t and t.get("items") is None:
            m.add_command(label="Finding alternatives…", state="disabled")
            m.add_separator()
        m.add_command(label="Cut", command=lambda: self.text.event_generate("<<Cut>>"))
        m.add_command(label="Copy", command=lambda: self.text.event_generate("<<Copy>>"))
        m.add_command(label="Paste", command=lambda: self.text.event_generate("<<Paste>>"))
        m.add_separator()
        m.add_command(label="Rephrase sentence", command=self.rephrase_sentence)
        if getattr(self, "current_change", None):
            m.add_command(label="Revert to original", command=self.revert_here)
        try:
            m.tk_popup(x, y)
        finally:
            m.grab_release()

    # -- formatted documents ------------------------------------------------------------------
    def _fmt(self, fn):
        if self.rich.active and not self.busy:
            fn(self.rich)
            self.text.focus_set()
            self.formatbar.refresh()

    def rich_insert(self, kind):
        if not self.rich.active or self.busy:
            self.flash_msg("Pictures, tables and the like go into formatted documents "
                           "(File › New formatted document, or open a Word or OpenDocument file).")
            return
        put = self.rich.insert_object
        if kind == "image":
            path = filedialog.askopenfilename(parent=self.root, title="Insert a picture",
                                              filetypes=objects.PICTURE_TYPES,
                                              initialdir=self.settings.get("last_dir") or os.path.expanduser("~"))
            obj = objects.load_picture(self, path) if path else None
            if obj:
                put(obj)
        elif kind == "table":
            objects.TableDialog(self, put)
        elif kind == "chart":
            chart = {"type": "chart", "kind": "bar", "title": "", "categories": ["A", "B", "C"],
                     "series": [["Series 1", [3.0, 5.0, 2.0]]], "x_title": "", "y_title": "", "legend": True,
                     "width": 432.0, "height": 252.0}
            objects.ChartDialog(self, chart, put)
        elif kind == "math":
            objects.MathDialog(self, {"type": "math", "tex": "", "display": False}, put)
        elif kind == "note":
            objects.NoteDialog(self, {"type": "note", "runs": []}, lambda o: put(o) if o.get("runs") else None)
        elif kind == "link":
            self.rich_link()
        elif kind in ("rule", "pagebreak"):
            put({"type": kind})
        self.text.focus_set()

    def rich_link(self):
        if not self.rich.active or self.busy:
            return
        sel = self.rich.selection()  # the dialog's entry takes the selection away; remember it
        fmt, _ = self.rich.state()

        def done(url, text):
            if sel:
                self.text.tag_add("sel", *sel)
                self.rich.set_value("link", url)
            elif url and text:
                with self.one_step():
                    start = self.text.index("insert")
                    self.text.insert("insert", text)
                    end = self.text.index("insert")
                self.text.tag_add("sel", start, end)
                self.rich.set_value("link", url)
                self.text.tag_remove("sel", "1.0", "end")
            self.text.focus_set()

        objects.LinkDialog(self, fmt.get("link"), None if sel else "", done)

    def open_link(self, url):
        if url.startswith(("http://", "https://", "mailto:")):
            webbrowser.open(url)
        else:
            self.flash_msg(f"Link: {url}")

    def rich_edit_object(self, oid):
        obj = self.rich.objects.get(oid)
        if not obj or self.busy:
            return
        kind = obj.get("type")

        def set_(new):
            self.rich.set_object(oid, new)
        if kind == "image":
            objects.ImageDialog(self, obj, set_)
        elif kind == "chart":
            objects.ChartDialog(self, obj, set_)
        elif kind == "math":
            objects.MathDialog(self, obj, set_)
        elif kind == "note":
            objects.NoteDialog(self, obj, set_)
        elif kind == "opaque":
            self.flash_msg("This part is kept exactly as it was in the original file; it cannot be edited here.")

    def rich_object_menu(self, oid, e):
        obj = self.rich.objects.get(oid)
        if not obj:
            return
        m = tk.Menu(self.root, tearoff=0)
        kind = obj.get("type")
        label = {"image": "Picture…", "chart": "Edit chart…", "math": "Edit formula…", "note": "Edit note…"}.get(kind)
        if label:
            m.add_command(label=label, command=lambda: self.rich_edit_object(oid))
        if kind == "image":
            m.add_command(label="Save picture as…", command=lambda: self._save_picture(obj))
        m.add_command(label="Delete", command=lambda: self._delete_object(oid))
        try:
            m.tk_popup(e.x_root, e.y_root)
        finally:
            m.grab_release()

    def _delete_object(self, oid):
        idx = self.rich.object_index(oid)
        if idx:
            with self.one_step():
                self.text.delete(idx)
            self.text.focus_set()

    def _save_picture(self, obj):
        ext = "." + (obj.get("ext") or "png").replace("jpeg", "jpg")
        path = filedialog.asksaveasfilename(parent=self.root, title="Save picture", defaultextension=ext,
                                            initialfile=os.path.splitext(obj.get("name") or "picture")[0] + ext)
        if path:
            with open(path, "wb") as f:
                f.write(bytes(obj.get("data") or b""))

    def rich_table_menu(self, oid, r, k, e):
        view = self.rich.views.get(oid)
        if view is None:
            return

        def change(fn):
            obj = view.read()
            rows = obj.setdefault("rows", [])
            n = max([len(x) for x in rows] + [1])
            for row in rows:
                while len(row) < n:
                    row.append([])
            fn(obj, rows, n)
            self.rich.set_object(oid, obj)

        empty = lambda: [{"content": [], "style": "normal", "align": None, "list": None, "indent": 0}]  # noqa: E731
        m = tk.Menu(self.root, tearoff=0)
        m.add_command(label="Insert row above", command=lambda: change(lambda o, rows, n: rows.insert(r, [empty() for _ in range(n)])))
        m.add_command(label="Insert row below", command=lambda: change(lambda o, rows, n: rows.insert(r + 1, [empty() for _ in range(n)])))
        m.add_command(label="Delete row", command=lambda: change(lambda o, rows, n: rows.pop(r) if len(rows) > 1 else None))
        m.add_separator()
        m.add_command(label="Insert column left", command=lambda: change(lambda o, rows, n: [row.insert(k, empty()) for row in rows]))
        m.add_command(label="Insert column right", command=lambda: change(lambda o, rows, n: [row.insert(k + 1, empty()) for row in rows]))
        m.add_command(label="Delete column", command=lambda: change(lambda o, rows, n: [row.pop(k) for row in rows] if n > 1 else None))
        m.add_separator()
        m.add_command(label="Header row on / off", command=lambda: change(lambda o, rows, n: o.update(header=not o.get("header"))))
        m.add_command(label="Delete table", command=lambda: self._delete_object(oid))
        try:
            m.tk_popup(e.x_root, e.y_root)
        finally:
            m.grab_release()
        return "break"

    def page_setup(self):
        if self.mode != "rich":
            self.flash_msg("Page set-up is for formatted documents; for an export, pick the paper in the Export dialog.")
            return
        objects.PageSetupDialog(self, self.rich.base.page, self.rich.set_page)

    def export_dialog(self):
        """Write what is open as another format (PDF, Word, OpenDocument, Markdown, LaTeX, text …)."""
        if self.busy:
            return
        if not self._native_ready():
            return
        from . import exporting
        from .ui.export import ExportDialog
        name = self.doc.name if self.doc.path else (getattr(self, "suggest_name", None) or "document")
        if self.mode == "pdf":
            src = exporting.Source("pdf", name, self.doc.path, data=self.pdfview.data)
        elif self.mode == "rich":
            try:
                model = self.rich.document()
            except Exception as e:
                messagebox.showerror("Export", f"The document could not be read from the editor.\n\n{e}",
                                     parent=self.root)
                return
            src = exporting.Source("rich", name, self.doc.path, model=model)
        else:
            src = exporting.Source(self.doc.kind, name, self.doc.path, text=self.text.get("1.0", "end-1c"),
                                   encoding=self.doc.encoding if self.doc.source_ext is None else "utf-8")
        ExportDialog(self, src)

    def open_pdf(self, doc):
        self.clear_proposals()
        self.set_mode("pdf")
        self.text.configure(state="normal")
        self.text.delete("1.0", "end")
        self.text.edit_reset()
        self.text.edit_modified(False)
        self.pdfview.load(doc.data, doc.path)
        self._fresh("", None)
        self.pdf_changed(dirty=False)

    def pdf_changed(self, dirty=True):
        """The PDF was changed (or undone): show it in the title and the status bar."""
        self.dirty = dirty and bool(self.pdfview.undo_stack)
        self.doc.data = self.pdfview.data
        self.update_title()
        n = len(self.pdfview.pages)
        self.status_stats.configure(text=f"{n} page{'s' if n != 1 else ''}")

    def convert_pdf(self):
        """Make an editable document of the PDF: paragraphs, headings, lists, tables and pictures, with
        broken words and sentences mended by the language model."""
        if self.mode != "pdf" or self.busy or not self.confirm_discard():
            return
        from . import cleanup, pdfdoc
        data = self.pdfview.data
        name = os.path.splitext(self.doc.name)[0]
        self.busy = True
        flag = self.cancel_flag = threading.Event()
        self.msg.pack_forget()
        self.progress["value"] = 0
        self.cancel_btn.pack(side="right", padx=(self.theme.px(8), 0))
        self.progress.pack(side="right")
        self.progress_lbl.configure(text="Reading the PDF…")
        self.progress_lbl.pack(side="right", padx=(0, self.theme.px(8)))
        helper = cleanup.Helper(self.engine)

        def progress(k, n):
            def show():
                self.progress["value"] = int(1000 * k / max(1, n))
                self.progress_lbl.configure(text=f"Reading page {k + 1} of {n}")
            self.post(show)

        def job():
            return pdfdoc.to_document(data, helper=helper, progress=progress, cancel=flag)

        def done(model, err):
            self.busy = False
            for w in (self.progress, self.progress_lbl, self.cancel_btn):
                w.pack_forget()
            self.msg.pack(side="right")
            if err:
                if not isinstance(err, InterruptedError):
                    messagebox.showerror("Convert PDF", f"The PDF could not be converted.\n\n{err}", parent=self.root)
                return
            self.doc = formats.Document(kind="rich", model=model)
            self.set_rich(model)
            self.dirty = True
            self.update_title()
            self.suggest_name = name
            self.flash_msg("Converted. Save it as Word or OpenDocument, or use Export for Markdown, LaTeX or text.", 12000)

        self.lane_slow.submit(job, done)

    # -- models ---------------------------------------------------------------------------
    def reload_models(self, done=None):
        self.models_state = "loading"
        self.update_mode()

        def job():
            self.engine.load_models()

        def finished(_, err):
            self.models_state = "ready"
            self.update_mode()
            self.cache.clear()
            self._gram_cache.clear()
            self.analyze()
            if self.target:
                self.update_target(force=True)
            if self.panel.state["target"] is None:
                self.panel.render()
            if done:
                done()

        self.lane_slow.submit(job, finished)

    def update_mode(self):
        e = self.engine
        if self.models_state == "loading":
            self.mode_btn.configure(text="◌ Loading models…")
            self.mode_btn.fg_key = "muted"
        elif e.smart:
            parts = ["context"] + (["meaning check"] if e.emb else []) + (["paraphraser"] if e.t5 else [])
            self.mode_btn.configure(text="● Smart mode" if e.emb and e.t5 else "● Smart mode (partial)")
            self.mode_btn.fg_key = "good"
            self.mode_tip.text = "Models loaded: " + ", ".join(parts) + "\nClick to manage models"
        else:
            self.mode_btn.configure(text="○ Lite mode")
            self.mode_btn.fg_key = "warn"
            self.mode_tip.text = "Only the offline thesaurus is active. Click to download the small models."
        self.mode_btn.recolor()

    def open_models(self):
        ModelsDialog(self)

    # -- view -------------------------------------------------------------------------------
    def set_hints(self, on):
        self.settings["phrase_hints"] = on
        models.save_settings(self.settings)
        self.color_editor()
        self.later("target", 10, lambda: self.update_target(force=True))

    def set_show_changes(self, on):
        self.settings["show_changes"] = on
        models.save_settings(self.settings)
        self.color_editor()

    def toggle_theme(self):
        name = "dark" if self.theme.name == "light" else "light"
        self.theme.switch(name)
        self.theme.style_ttk()
        self.theme.repaint()
        self.color_editor()
        self.theme_btn.set_icon("moon" if name == "light" else "sun")
        self.update_mode()
        self.panel.render()
        self.preview.retheme()
        self.rich.retheme()
        self.settings["theme"] = name
        models.save_settings(self.settings)

    def zoom(self, d):
        size = models.DEFAULTS["font_size"] if d == 0 else self.theme.size + d
        self.theme.set_editor_font(size=size)
        self._center_column()
        self.preview.retheme()
        self.rich.retheme()
        self.settings["font_size"] = self.theme.size
        models.save_settings(self.settings)

    def set_font_family(self):
        self.theme.set_editor_font(family=self.font_var.get())
        self._center_column()
        self.preview.retheme()
        self.rich.retheme()
        self.settings["font_family"] = self.font_var.get()
        models.save_settings(self.settings)

    def flash_msg(self, text, ms=5000):
        self.msg.configure(text=text)
        self.later("msg", ms, lambda: self.msg.configure(text=""))

    def undo(self):
        if self.mode == "pdf":
            self.pdfview.undo()
            return
        try:
            self.text.edit_undo()
        except tk.TclError:
            pass

    def redo(self):
        if self.mode == "pdf":
            self.pdfview.redo()
            return
        try:
            self.text.edit_redo()
        except tk.TclError:
            pass

    def select_all(self):
        self.text.tag_add("sel", "1.0", "end-1c")
        self.later("target", 200, self.update_target)

    def about(self):
        messagebox.showinfo("About Rewrite", f"Rewrite {__version__}\n\nA small offline rewriting editor.\n"
                            f"Created by {__author__}\n{__url__}\n\n"
                            "Models and data are stored in:\n" + models.DATA_DIR, parent=self.root)

    def quit(self):
        if self.busy:
            self.cancel_flag.set()
        if not self.confirm_discard():
            return
        self._proof_gen += 1
        self.preview.stop()
        try:
            if self.preview_on:
                self.settings["preview_split"] = round(self.pane.sashpos(0) / max(1, self.pane.winfo_width()), 3)
            self.settings["geometry"] = self.root.geometry()
            models.save_settings(self.settings)
        except Exception:
            pass
        self.root.destroy()


def _dpi_aware():
    if sys.platform == "win32":
        try:
            import ctypes
            ctypes.windll.shcore.SetProcessDpiAwareness(1)
        except Exception:
            try:
                ctypes.windll.user32.SetProcessDPIAware()
            except Exception:
                pass


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    _dpi_aware()
    root = tk.Tk(className="Rewrite")
    try:
        App(root, argv[0] if argv else None)
    except Exception:
        err = traceback.format_exc()
        print(err, file=sys.stderr)
        messagebox.showerror("Rewrite failed to start", err)
        raise
    root.mainloop()


if __name__ == "__main__":
    main()
