"""Formatted documents (Word, OpenDocument, RTF) edited in the main text widget.

The widget holds each paragraph on a line of its own. Character formatting lives in tags named "f:..."
(f:b, f:i, f:size=14, f:link=https://...), each paragraph's formatting in one tag
"P:style|align|list kind|level|indent|lang", a line break inside a paragraph is a newline tagged "br",
and pictures, charts, tables, formulas, notes and list markers are embedded images and windows. Fonts
are drawn by "F:..." tags worked out from the other two. The Rust core turns a Document into this form
and back (native.rich_flatten / native.rich_build).

Tk's own undo forgets formatting and embedded objects, so in this mode every change to the widget goes
through a small Tcl wrapper (Redirect) and is recorded here (History). Code elsewhere in the app keeps
calling text.insert / text.delete / text.edit_undo as usual.
"""
import copy
import time
import tkinter as tk
import traceback
from tkinter import font as tkfont

from . import docmodel, native

OBJ = "￼"
BR = " "
BOOLS = ("b", "i", "u", "s", "sup", "sub", "code")
HEADINGS = ("title", "subtitle", "h1", "h2", "h3", "h4", "h5", "h6")
HIGHLIGHT = {"yellow": "#FFF59D", "green": "#C8E6C9", "cyan": "#B2EBF2", "magenta": "#F8BBD0", "blue": "#BBDEFB",
             "red": "#FFCDD2", "darkBlue": "#9FA8DA", "darkCyan": "#80CBC4", "darkGreen": "#A5D6A7",
             "darkMagenta": "#CE93D8", "darkRed": "#EF9A9A", "darkYellow": "#FFE082", "darkGray": "#BDBDBD",
             "lightGray": "#EEEEEE", "black": "#9E9E9E"}
KEEP = ("f:", "P:")  # the tags that are part of the document (and of undo)

_PROC = r"""
set op [lindex $args 0]
switch -exact -- $op {
    insert - delete - replace - edit {return [@PY {*}$args]}
    image - window {if {[lindex $args 1] eq "create"} {return [@PY {*}$args]}}
    tag {if {[lindex $args 1] in {add remove} && [string match {[fP]:*} [lindex $args 2]]} {return [@PY {*}$args]}}
}
return [@ORIG {*}$args]
"""


def ptag(style="normal", align=None, kind=None, level=0, indent=0, lang=""):
    return f"P:{style}|{align or ''}|{kind or ''}|{int(level)}|{int(indent)}|{lang or ''}"


def pattrs(tag):
    style, align, kind, level, indent, lang = (tag[2:].split("|", 5) + [""] * 6)[:6]
    return {"style": style or "normal", "align": align or None, "kind": kind or None, "level": int(level or 0),
            "indent": int(indent or 0), "lang": lang}


def ftags(fmt):
    """The f: tags for a Run dict (or any dict with the same keys)."""
    out = [f"f:{k}" for k in BOOLS if fmt.get(k)]
    for k in ("size", "font", "color", "hl", "link"):
        v = fmt.get(k)
        if v not in (None, ""):
            out.append(f"f:{k}={v:g}" if k == "size" else f"f:{k}={v}")
    return out


def fmt_of(tags):
    """The Run fields that some f: tags stand for."""
    d = {}
    for t in tags:
        if not t.startswith("f:"):
            continue
        k, eq, v = t[2:].partition("=")
        if not eq:
            d[k] = True
        elif k == "size":
            try:
                d[k] = float(v)
            except ValueError:
                pass
        else:
            d[k] = v
    return d


def keep(tag):
    return tag.startswith(KEEP) or tag == "br"


class Redirect:
    """Routes the widget's editing commands to a Python handler; everything else goes straight to Tk."""

    def __init__(self, widget, handler):
        self.w = widget
        self.name = widget._w
        self.orig_name = self.name + "_orig"
        self.py_name = self.name + "_py"
        tk_ = widget.tk
        tk_.call("rename", self.name, self.orig_name)
        tk_.createcommand(self.py_name, handler)
        tk_.call("proc", self.name, "args", _PROC.replace("@PY", self.py_name).replace("@ORIG", self.orig_name))

    def __call__(self, *args):
        return self.w.tk.call((self.orig_name,) + args)

    def close(self):
        tk_ = self.w.tk
        try:
            tk_.call("rename", self.name, "")
            tk_.call("rename", self.orig_name, self.name)
        except tk.TclError:
            pass
        try:
            tk_.deletecommand(self.py_name)
        except tk.TclError:
            pass


# -- undo ------------------------------------------------------------------------------------------

class Put:
    """Content put in at i1 (it ends at i2)."""

    def __init__(self, i1, i2, frag):
        self.i1, self.i2, self.frag = i1, i2, frag

    def undo(self, ed):
        ed.raw_delete(self.i1, self.i2)
        return self.i1

    def redo(self, ed):
        ed.put(self.i1, self.frag)
        return self.i2


class Cut:
    """Content taken out between i1 and i2."""

    def __init__(self, i1, i2, frag):
        self.i1, self.i2, self.frag = i1, i2, frag

    def undo(self, ed):
        ed.put(self.i1, self.frag)
        return self.i2

    def redo(self, ed):
        ed.raw_delete(self.i1, self.i2)
        return self.i1


class Tag:
    """A tag's ranges between i1 and i2 before and after a change."""

    def __init__(self, tag, i1, i2, before, after):
        self.tag, self.i1, self.i2, self.before, self.after = tag, i1, i2, before, after

    def _set(self, ed, ranges):
        ed.orig("tag", "remove", self.tag, self.i1, self.i2)
        if ranges:
            ed.orig("tag", "add", self.tag, *ranges)
        return self.i1

    def undo(self, ed):
        return self._set(ed, self.before)

    def redo(self, ed):
        return self._set(ed, self.after)


class Change:
    """An object (a picture, chart, table ...) edited in place."""

    def __init__(self, oid, before, after):
        self.oid, self.before, self.after = oid, before, after

    def undo(self, ed):
        ed.set_object(self.oid, copy.deepcopy(self.before), record=False)
        return ed.object_index(self.oid)

    def redo(self, ed):
        ed.set_object(self.oid, copy.deepcopy(self.after), record=False)
        return ed.object_index(self.oid)


class History:
    def __init__(self, ed):
        self.ed = ed
        self.done, self.undone = [], []
        self.group = None
        self.kind = None  # what the open group holds: "type", "erase" or None
        self.at = 0.0

    def reset(self):
        self.done, self.undone, self.group, self.kind = [], [], None, None

    def separator(self):
        self.group, self.kind = None, None

    def start(self, kind=None):
        """A new user action begins. Typing and erasing one character at a time join the open group
        (as Tk's own undo does) until a word ends, the kind of edit changes or a pause."""
        now = time.time()
        auto = self.ed.auto_separators()
        same = kind and self.kind and (kind == self.kind or (self.kind == "type" and kind == "type:sp"))
        joins = self.group is not None and (not auto or (same and now - self.at < 2.0))
        if not joins:
            self.group = []
            self.done.append(self.group)
            if len(self.done) > 400:
                self.done.pop(0)
        self.kind, self.at = kind, now

    def record(self, op):
        if self.group is None:
            self.start()
        self.group.append(op)
        self.undone.clear()

    def undo(self):
        while self.done and not self.done[-1]:
            self.done.pop()
        if not self.done:
            return False
        group = self.done.pop()
        self.separator()
        self.ed.replay(group, undo=True)
        self.undone.append(group)
        return True

    def redo(self):
        if not self.undone:
            return False
        group = self.undone.pop()
        self.separator()
        self.ed.replay(group, undo=False)
        self.done.append(group)
        return True


# -- the editor ----------------------------------------------------------------------------------------

class RichEditor:
    def __init__(self, app):
        self.app, self.t, self.th = app, app.text, app.theme
        self.active = False
        self.redirect = None
        self.history = History(self)
        self.quiet = 0  # > 0: changes are not recorded (loading, undo itself)
        self.depth = 0  # > 0: inside the handling of a change
        self.objects = {}  # object id -> dict (the Rust Obj form)
        self.windows = {}  # widget path -> object id, for embedded windows
        self.views = {}  # object id -> widget or PhotoImage that shows it
        self.markers = set()  # widget paths of list markers
        self.meta = {}  # the document without its blocks (title, page, source package ...)
        self.fonts = {}  # F: tag -> tkinter Font
        self.pending = {}  # formatting picked with nothing selected, for the next typed text
        self.last_cut = None  # (index, f: tags) of the latest deletion, so a replacement keeps its look
        self.clip = None  # (plain text, fragment) of the latest copy, to paste with formatting
        self._next = 0
        self._configured = set()  # tags whose look is set up
        self._fixing = False  # normalising paragraphs (its own edits are not normalised again)
        self.base = docmodel.Document()
        self.on_change = None  # called after formatting changes (the toolbar shows the state)

    # -- switching the mode on and off ---------------------------------------------------------
    def attach(self):
        if self.active:
            return
        self.t.configure(undo=False)
        self.redirect = Redirect(self.t, self._handle)
        self.active = True

    def install_keys(self):
        """Keys and clicks of this mode, added after the app's own bindings. They stay bound and do nothing
        while the mode is off (unbinding one function would drop the app's bindings for the same keys)."""
        for seq, fn in (("<Return>", self._key_return), ("<Shift-Return>", self._key_soft_return),
                        ("<Tab>", lambda e: self._key_tab(1)), ("<Shift-Tab>", lambda e: self._key_tab(-1)),
                        ("<BackSpace>", self._key_backspace), ("<Home>", self._key_home),
                        ("<<Copy>>", self._copy), ("<<Cut>>", self._copy), ("<<Paste>>", self._paste)):
            self._bind(seq, fn)
        try:
            self._bind("<ISO_Left_Tab>", lambda e: self._key_tab(-1))
        except tk.TclError:
            pass
        mod = "Command" if self.t.tk.call("tk", "windowingsystem") == "aqua" else "Control"
        for key, name in (("b", "b"), ("i", "i"), ("u", "u")):
            self._bind(f"<{mod}-{key}>", lambda e, n=name: self.toggle(n))
            self._bind(f"<{mod}-{key.upper()}>", lambda e, n=name: self.toggle(n))
        self._bind(f"<{mod}-k>", lambda e: self.app.rich_link())
        self.t.bind("<ButtonRelease-1>", self._after_click, add="+")
        self.t.tag_bind("obj", "<Double-Button-1>", self._object_click)

    def _bind(self, seq, fn):
        def call(e, fn=fn):
            if not self.active:
                return None
            try:
                r = fn(e)
            except Exception:
                traceback.print_exc()
                return "break"
            return "break" if r is None else (None if r == "continue" else r)
        self.t.bind(seq, call, add="+")

    def detach(self):
        if not self.active:
            return
        self.quiet += 1
        try:
            self.clear()
        finally:
            self.quiet -= 1
        if self.redirect:
            self.redirect.close()
            self.redirect = None
        self.active = False
        self.t.configure(undo=True)
        self.t.edit_reset()

    def clear(self):
        self.orig("delete", "1.0", "end")
        for name in list(self.t.tag_names()):
            if name.startswith(("f:", "P:", "F:")) or name == "br":
                self.t.tag_delete(name)
        self.objects.clear()
        self.windows.clear()
        self.views.clear()
        self.markers.clear()
        self.fonts.clear()
        self._configured.clear()
        self.history.reset()
        self.pending = {}

    def orig(self, *args):
        return self.redirect(*args) if self.redirect else self.t.tk.call((self.t._w,) + args)

    def auto_separators(self):
        try:
            return bool(int(self.t.cget("autoseparators")))
        except (tk.TclError, ValueError):
            return True

    # -- loading and saving ------------------------------------------------------------------------
    def load(self, doc):
        """Show a docmodel Document (replacing everything)."""
        self.attach()
        meta = docmodel.to_native(doc)
        meta["blocks"] = []
        self.meta = meta
        self.base = docmodel.Document(title=doc.title, author=doc.author, page=doc.page, font=doc.font,
                                      size=doc.size or 11.0)
        flat = native.rich_flatten(docmodel.to_native(doc))
        self.quiet += 1
        try:
            self.clear()
            self._fill(flat)
        finally:
            self.quiet -= 1
        self.history.reset()
        self._break_spacing("1.0", "end")
        self.t.mark_set("insert", "1.0")
        self._skip_marker()
        self.renumber()

    def _fill(self, flat):
        text, lines, spans, objects = flat["text"], flat["lines"], flat["spans"], flat["objects"]
        labels = flat.get("labels") or []
        spans = sorted(spans, key=lambda s: s["start"])
        si = 0
        pos = 0
        oi = 0
        for k, line in enumerate(text.split("\n")):
            attrs = lines[k] if k < len(lines) else {}
            kind, level = (attrs.get("list") or (None, 0))
            pt = ptag(attrs.get("style") or "normal", attrs.get("align"), kind, level, attrs.get("indent", 0),
                      attrs.get("lang", ""))
            self._ensure_ptag(pt)
            start_index = self.t.index("end-1c")
            if kind:
                self._insert_marker("end-1c", labels[k] if k < len(labels) else None)
            # the line's text in pieces of one formatting each, objects in between
            seg_start = pos
            args = []

            def flush(upto, seg_start):
                nonlocal si
                i = seg_start
                while i < upto:
                    while si < len(spans) and spans[si]["end"] <= i:
                        si += 1
                    if si < len(spans) and spans[si]["start"] <= i:
                        j, fmt = min(upto, spans[si]["end"]), spans[si]["fmt"]
                    else:
                        j = min(upto, spans[si]["start"]) if si < len(spans) else upto
                        fmt = {}
                    chunk = text[i:j]
                    for t in ftags(fmt):
                        self._ensure_tag(t)
                    for piece, br in _split_breaks(chunk):
                        tags = ftags(fmt) + [pt, self.font_tag(pt, ftags(fmt))]
                        if br:
                            tags.append("br")
                        args.extend([piece, tuple(tags)])
                    i = j

            for i, ch in enumerate(line):
                if ch == OBJ:
                    flush(pos + i, seg_start)
                    if args:
                        self.orig("insert", "end-1c", *args)
                        args = []
                    obj = objects[oi] if oi < len(objects) else {"type": "opaque", "label": "object"}
                    oi += 1
                    self._create(self._new_id(), obj, "end-1c", pt)
                    seg_start = pos + i + 1
            flush(pos + len(line), seg_start)
            if args:
                self.orig("insert", "end-1c", *args)
            pos += len(line) + 1
            last = k == len(lines) - 1 or pos > len(text)
            if not last:
                self.orig("insert", "end-1c", "\n", (pt,))
            self.orig("tag", "add", pt, start_index, "end-1c" if last else "end-2c")
            if last:
                self.orig("tag", "add", pt, "end-1c", "end")

    def document(self):
        """The docmodel Document the widget shows."""
        flat = self.flat()
        return docmodel.from_native(native.rich_build(flat, self.meta))

    def flat(self, i1="1.0", i2="end-1c"):
        """The widget's content in the Rust core's editor form."""
        text, lines, spans, objects = [], [], [], []
        n = 0
        active = {t for t in self.t.tag_names(i1) if keep(t)}
        cur_p = next((t for t in active if t.startswith("P:")), None)
        span = None  # [start, end, tag set]

        def add_span(start, end, tags):
            nonlocal span
            fs = tuple(sorted(t for t in tags if t.startswith("f:")))
            if not fs:
                return
            if span and span[1] == start and span[2] == fs:
                span[1] = end
            else:
                span = [start, end, fs]
                spans.append(span)

        def end_line():
            attrs = pattrs(cur_p) if cur_p else pattrs("P:normal")
            lines.append({"style": attrs["style"], "align": attrs["align"],
                          "list": [attrs["kind"], attrs["level"]] if attrs["kind"] else None,
                          "indent": attrs["indent"], "lang": attrs["lang"]})

        for key, val, idx in self.t.dump(i1, i2, text=True, tag=True, image=True, window=True):
            if key == "tagon":
                if keep(val):
                    active.add(val)
                    if val.startswith("P:"):
                        cur_p = val
            elif key == "tagoff":
                active.discard(val)
            elif key == "text":
                br = "br" in active
                pieces = val.split("\n")
                for j, piece in enumerate(pieces):
                    if piece:
                        add_span(n, n + len(piece), active)
                        text.append(piece)
                        n += len(piece)
                    if j < len(pieces) - 1:
                        if br:
                            text.append(BR)
                        else:
                            end_line()
                            text.append("\n")
                        n += 1
            elif key in ("image", "window"):
                oid = val if key == "image" else self.windows.get(val)
                if key == "window" and val in self.markers:
                    continue
                if oid is None or oid not in self.objects:
                    continue
                if key == "window":
                    self._sync(oid)
                text.append(OBJ)
                objects.append(self.objects[oid])
                n += 1
        end_line()
        text = "".join(text)
        return {"text": text, "lines": lines, "objects": objects, "labels": [],
                "spans": [{"start": s, "end": e, "fmt": fmt_of(tags)} for s, e, tags in spans]}

    def plain(self):
        """The text with one U+FFFC for each embedded object (list markers too), so that offsets match
        the widget's indices."""
        out = []
        for key, val, _ in self.t.dump("1.0", "end-1c", text=True, image=True, window=True):
            out.append(val if key == "text" else OBJ)
        return "".join(out)

    def protected(self, snap):
        """Spans the rewriting must leave alone: objects, list markers and code."""
        text = snap.text
        spans = []
        i = text.find(OBJ)
        while i >= 0:
            spans.append((i, i + 1))
            i = text.find(OBJ, i + 1)
        for name in self.t.tag_names():
            if name.startswith("P:") and (pattrs(name)["style"] == "code" or name.startswith("P:math|")):
                r = self.t.tag_ranges(name)
                for a, b in zip(r[::2], r[1::2]):
                    spans.append((snap.offset(str(a)), snap.offset(str(b))))
            elif name == "f:code":
                r = self.t.tag_ranges(name)
                for a, b in zip(r[::2], r[1::2]):
                    spans.append((snap.offset(str(a)), snap.offset(str(b))))
        return spans

    # -- the Tcl hook --------------------------------------------------------------------------
    def _handle(self, op, *args):
        try:
            if op == "edit":
                return self._edit(*args)
            if self.quiet:
                return self.orig(op, *args)
            if op == "insert":
                return self._insert(args[0], args[1:])
            if op == "delete":
                return self._delete(*args)
            if op == "replace":
                i1, i2 = self.t.index(args[0]), self.t.index(args[1])
                self._delete(i1, i2)
                return self._insert(i1, args[2:])
            if op == "tag":
                return self._tag(*args)
            if op in ("image", "window"):
                return self._embed(op, *args)
            return self.orig(op, *args)
        except Exception:
            traceback.print_exc()
            try:
                return self.orig(op, *args)
            except tk.TclError:
                return ""

    def _edit(self, sub, *args):
        if sub == "undo":
            self.history.undo()
            return ""
        if sub == "redo":
            self.history.redo()
            return ""
        if sub == "separator":
            self.history.separator()
            return ""
        if sub == "reset":
            self.history.reset()
            return ""
        if sub == "canundo":
            return int(bool(self.history.done))
        if sub == "canredo":
            return int(bool(self.history.undone))
        return self.orig("edit", sub, *args)

    def _begin(self, kind=None):
        if self.depth == 0:
            self.history.start(kind)
        self.depth += 1

    def _end(self):
        self.depth -= 1

    def _insert(self, index, rest):
        i1 = self.t.index(index)
        pairs = list(rest)
        explicit = len(pairs) > 1 and any(str(x) for x in pairs[1::2])
        chars = "".join(str(x) for x in pairs[0::2])
        if not chars:
            return ""
        typing = index == "insert" and len(chars) == 1 and not explicit
        self._begin(("type:sp" if chars.isspace() else "type") if typing else None)
        try:
            if explicit:
                args = []
                for k in range(0, len(pairs), 2):
                    tags = pairs[k + 1] if k + 1 < len(pairs) else ()
                    tags = self.t.tk.splitlist(tags) if isinstance(tags, str) else tags
                    args += [pairs[k], tuple(tags)]
            else:
                tags = self._tags_for_insert(i1)
                args = [chars, tuple(tags)]
            self.t.mark_set("rx_end", i1)
            self.t.mark_gravity("rx_end", "right")
            self.orig("insert", i1, *args)
            i2 = self.t.index("rx_end")
            self.history.record(Put(i1, i2, self.fragment(i1, i2)))
            self.last_cut = None
            if not explicit:
                self.pending = {}
            if ("\n" in chars or explicit) and not self._fixing:
                self._normalize(i1, i2, newline="\n" in chars)
            else:
                self.restyle(i1, i2)
        finally:
            self._end()
        self._changed()
        return ""

    def _tags_for_insert(self, i1):
        """New text takes the look of the text it replaces, else of the text before it (or after it, at
        the start of a paragraph), with any formatting picked from the toolbar meanwhile."""
        if self.last_cut and self.t.compare(self.last_cut[0], "==", i1):
            f = list(self.last_cut[1])
        else:
            src = None
            if self.t.compare(i1, ">", f"{i1} linestart"):
                src = f"{i1} -1c"
            if src is None or self._is_object(src):
                src = i1
            f = [t for t in self.t.tag_names(src) if t.startswith("f:")]
            if self._is_object(src):
                f = []
        if self.pending:
            fmt = fmt_of(f)
            fmt.update(self.pending)
            f = ftags({k: v for k, v in fmt.items() if v})
        p = self.para_tag(i1)
        return f + [p, self.font_tag(p, f)]

    def _is_object(self, index):
        try:
            return self.t.get(index) == "" and self.t.compare(index, "<", "end-1c")
        except tk.TclError:
            return False

    def _delete(self, i1, i2=None):
        a = self.t.index(i1)
        b = self.t.index(i2) if i2 is not None else self.t.index(f"{a} +1c")
        if self.t.compare(b, ">", "end-1c"):
            b = self.t.index("end-1c")
        if not self.t.compare(a, "<", b):
            return ""
        single = self.t.compare(b, "==", f"{a} +1c")
        self._begin("erase" if single else None)
        try:
            frag = self.fragment(a, b)
            cut = (a, [t for t in self.t.tag_names(a) if t.startswith("f:")])
            self.history.record(Cut(a, b, frag))
            self.raw_delete(a, b)
            if not self._fixing:
                self._normalize(a, a, newline=any(x[0] == "t" and "\n" in x[1] for x in frag))
                self.last_cut = cut  # a replacement typed or put here next takes this look
        finally:
            self._end()
        self._changed()
        return ""

    def _tag(self, sub, tag, *idx):
        if not idx:
            return ""
        pairs = []
        k = 0
        while k < len(idx):
            a = self.t.index(idx[k])
            b = self.t.index(idx[k + 1]) if k + 1 < len(idx) else self.t.index(f"{a} +1c")
            pairs.append((a, b))
            k += 2
        self._begin()
        try:
            for a, b in pairs:
                before = self.ranges_in(tag, a, b)
                self.orig("tag", sub, tag, a, b)
                after = self.ranges_in(tag, a, b)
                if before != after:
                    self.history.record(Tag(tag, a, b, before, after))
        finally:
            self._end()
        return ""

    def _embed(self, op, sub, index, *opts):
        i1 = self.t.index(index)
        self._begin()
        try:
            res = self.orig(op, sub, i1, *opts)
            i2 = self.t.index(f"{i1} +1c")
            self.history.record(Put(i1, i2, self.fragment(i1, i2)))
        finally:
            self._end()
        return res

    def ranges_in(self, tag, a, b):
        out = []
        r = self.t.tag_ranges(tag)
        for s, e in zip(r[::2], r[1::2]):
            s, e = str(s), str(e)
            if self.t.compare(e, "<=", a) or self.t.compare(s, ">=", b):
                continue
            out += [s if self.t.compare(s, ">", a) else a, e if self.t.compare(e, "<", b) else b]
        return out

    def _changed(self):
        if self.on_change:
            self.app.later("rich-state", 60, self.on_change)

    # -- fragments: content taken out and put back --------------------------------------------------
    def fragment(self, i1, i2):
        out = []
        active = {t for t in self.t.tag_names(i1) if keep(t)}
        for key, val, idx in self.t.dump(i1, i2, text=True, tag=True, image=True, window=True):
            if key == "tagon":
                if keep(val):
                    active.add(val)
            elif key == "tagoff":
                active.discard(val)
            elif key == "text":
                out.append(("t", val, tuple(sorted(active))))
            elif key == "image":
                if val in self.objects:
                    out.append(("o", val, copy.deepcopy(self.objects[val]), tuple(sorted(active))))
            elif key == "window":
                if val in self.markers:
                    out.append(("m", tuple(sorted(active))))
                else:
                    oid = self.windows.get(val)
                    if oid in self.objects:
                        self._sync(oid)
                        out.append(("o", oid, copy.deepcopy(self.objects[oid]), tuple(sorted(active))))
        return out

    def put(self, i1, frag):
        """Put a fragment back at i1 (no recording)."""
        self.t.mark_set("rx_put", i1)
        self.t.mark_gravity("rx_put", "right")
        start = self.t.index(i1)
        for item in frag:
            if item[0] == "t":
                tags = [t for t in item[2] if keep(t)]
                p = next((t for t in tags if t.startswith("P:")), None)
                if p:
                    self._ensure_ptag(p)
                for t in tags:
                    self._ensure_tag(t)
                tags.append(self.font_tag(p or "P:normal", [t for t in tags if t.startswith("f:")]))
                self.orig("insert", "rx_put", item[1], tuple(tags))
            elif item[0] == "o":
                _, oid, obj, tags = item
                p = next((t for t in tags if t.startswith("P:")), "P:normal")
                self._create(oid, copy.deepcopy(obj), "rx_put", p, extra=[t for t in tags if keep(t)])
            elif item[0] == "m":
                self._insert_marker("rx_put", None, tags=[t for t in item[1] if keep(t)])
        end = self.t.index("rx_put")
        self.restyle(start, end)
        return end

    def raw_delete(self, i1, i2):
        for key, val, _ in self.t.dump(i1, i2, window=True):
            if key == "window":
                self.markers.discard(val)
                oid = self.windows.pop(val, None)
                if oid:
                    self._sync_from(oid, val)
                    self.views.pop(oid, None)
        self.orig("delete", i1, i2)

    def replay(self, group, undo):
        self.quiet += 1
        where = None
        try:
            for op in (reversed(group) if undo else group):
                try:
                    where = op.undo(self) if undo else op.redo(self)
                except tk.TclError:
                    traceback.print_exc()
        finally:
            self.quiet -= 1
        self.last_cut = None
        if where:
            try:
                self.t.mark_set("insert", where)
                self.t.see("insert")
            except tk.TclError:
                pass
        self.restyle("1.0", "end")
        self.renumber()
        self._changed()

    # -- paragraphs ---------------------------------------------------------------------------------
    def para_range(self, index):
        """(start, end) of the paragraph at index; end is just past its closing newline."""
        s = self.t.index(f"{index} linestart")
        while self.t.compare(s, ">", "1.0") and "br" in self.t.tag_names(f"{s} -1c"):
            s = self.t.index(f"{s} -1c linestart")
        e = self.t.index(f"{index} lineend")
        while "br" in self.t.tag_names(e) and self.t.compare(e, "<", "end-1c"):
            e = self.t.index(f"{e} +1c lineend")
        return s, self.t.index(f"{e} +1c")

    def para_tag(self, index):
        for t in self.t.tag_names(index):
            if t.startswith("P:"):
                return t
        s, _ = self.para_range(index)
        for t in self.t.tag_names(s):
            if t.startswith("P:"):
                return t
        return ptag()

    def paragraphs(self, i1, i2):
        """The starts of the paragraphs that [i1, i2] touches."""
        out = []
        s, e = self.para_range(i1)
        out.append(s)
        while self.t.compare(e, "<=", i2) and self.t.compare(e, "<=", "end-1c"):
            s, e = self.para_range(e)
            if s in out:
                break
            out.append(s)
        return out

    def _normalize(self, i1, i2, newline=False):
        """Make each paragraph between i1 and i2 one P: tag from start to end, with a list marker at its
        start when it is a list item and nowhere else. Changes made here are recorded with the edit."""
        self._fixing = True
        try:
            for s in self.paragraphs(self.t.index(f"{i1} linestart"), self.t.index(f"{i2} lineend")):
                s, e = self.para_range(s)
                names = [t for t in self.t.tag_names(s) if t.startswith("P:")]
                p = names[0] if names else None
                if p is None:
                    p = next((t for t in self.t.tag_names(f"{s} -1c") if t.startswith("P:")), None) \
                        if self.t.compare(s, ">", "1.0") else None
                    p = p or ptag()
                if newline and self.t.compare(s, ">", "1.0"):
                    prev = next((t for t in self.t.tag_names(f"{s} -1c") if t.startswith("P:")), None)
                    # a new, empty paragraph after a heading or caption is normal text
                    if prev == p and pattrs(p)["style"] in HEADINGS + ("caption",) and \
                            not self.t.get(s, f"{s} lineend").strip():
                        p = ptag("normal", pattrs(p)["align"])
                end = e if self.t.compare(e, "<", "end") else "end"
                for t in self.t.tag_names():
                    if t.startswith("P:") and t != p and self.ranges_in(t, s, end):
                        self.t.tag_remove(t, s, end)
                if self.ranges_in(p, s, end) != [s, self.t.index(end)]:
                    self._ensure_ptag(p)
                    self.t.tag_add(p, s, end)
                self._fix_markers(s, e, pattrs(p)["kind"] is not None)
                self.restyle(s, e)
        finally:
            self._fixing = False
        self.app.later("rich-renumber", 80, self.renumber)

    def _text_start(self, s):
        """Where a paragraph's text starts (after its list marker)."""
        for key, val, idx in self.t.dump(s, f"{s} +1c", window=True):
            if key == "window" and val in self.markers:
                return self.t.index(f"{s} +1c")
        return s

    def _fix_markers(self, s, e, wanted):
        found = [(val, idx) for key, val, idx in self.t.dump(s, e, window=True) if key == "window" and val in self.markers]
        for val, idx in reversed(found):
            if wanted and self.t.compare(idx, "==", s):
                continue
            self.t.delete(idx)
        if wanted and not any(self.t.compare(idx, "==", s) for _, idx in found):
            p = self.para_tag(s)
            mark = self._insert_marker(None, None)
            self.t.window_create(s, window=mark, align="baseline")
            self.t.tag_add(p, s)

    def _insert_marker(self, index, label, tags=None):
        """A list marker: a label at the start of a list item (one index wide, so renumbering does not
        move any text). With an index (a mark that moves past it), it is put there unrecorded."""
        lab = tk.Label(self.t, text=label or "•", bd=0, padx=0, pady=0, anchor="w", cursor="xterm",
                       bg=self.th.c["surface"], fg=self.th.c["text"], font=self.th.f_editor, width=3)
        lab.bind("<Button-1>", lambda e, w=lab: self._marker_click(w))
        self.markers.add(str(lab))
        if index is not None:
            self.orig("window", "create", index, "-window", lab, "-align", "baseline")
            for t in tags or ():
                if keep(t):
                    self.orig("tag", "add", t, f"{index} -1c")
        return lab

    def _marker_click(self, w):
        try:
            idx = self.t.index(w)
            self.t.mark_set("insert", f"{idx} +1c")
            self.t.focus_set()
        except tk.TclError:
            pass

    def renumber(self):
        """List numbers and note numbers, in document order."""
        if not self.active:
            return
        items = []  # (line, attrs, marker path)
        for key, val, idx in self.t.dump("1.0", "end", window=True):
            if key == "window" and val in self.markers:
                items.append((int(idx.split(".")[0]), self.para_tag(idx), val))
        lists, prev_line = [], None
        rows = []
        for line, p, path in items:
            a = pattrs(p)
            if prev_line is not None and not self._consecutive(prev_line, line):
                lists.append(None)
                rows.append(None)
            lists.append([a["kind"], a["level"]] if a["kind"] else None)
            rows.append((path, p))
            prev_line = line
        labels = native.rich_labels([tuple(x) if x else None for x in lists]) if lists else []
        for row, label in zip(rows, labels):
            if row is None:
                continue
            path, p = row
            try:
                w = self.t.nametowidget(path)
                font = self.fonts.get(self.font_tag(p, []))
                w.configure(text=label or "•", font=font or self.th.f_editor, bg=self.th.c["surface"],
                            fg=self.th.c["text"])
            except (tk.TclError, KeyError):
                pass
        n = 0
        for key, val, idx in self.t.dump("1.0", "end", window=True):
            oid = self.windows.get(val) if key == "window" else None
            if oid and self.objects.get(oid, {}).get("type") == "note":
                n += 1
                try:
                    self.t.nametowidget(val).configure(text=str(n))
                except (tk.TclError, KeyError):
                    pass

    def _consecutive(self, a, b):
        """Whether every line between two list lines is part of a list item."""
        for line in range(a + 1, b):
            if not pattrs(self.para_tag(f"{line}.0"))["kind"]:
                return False
        return True

    # -- looks -------------------------------------------------------------------------------------
    def _ensure_ptag(self, p):
        if p in self._configured:
            return
        self._configured.add(p)
        self._style_ptag(p)
        self.t.tag_lower(p)

    def _style_ptag(self, p):
        a = pattrs(p)
        look = docmodel.LOOK.get(a["style"], docmodel.LOOK["normal"])
        z = self.zoom()
        pt = self.th.root.winfo_fpixels("1p") * z
        indent = (look.get("indent", 0) + 36 * a["indent"]) * pt
        hang = 0
        if a["kind"]:
            indent += 22 * a["level"] * pt
            hang = self.th.f_editor.measure("000")
        opts = dict(spacing1=int(look["before"] * pt), spacing3=int(look["after"] * pt),
                    lmargin1=int(indent), lmargin2=int(indent + hang), rmargin=int(36 * pt) if a["style"] == "quote" else 0,
                    justify={"center": "center", "right": "right"}.get(a["align"] or "", "left"),
                    foreground=look.get("color") or "", background="")
        if a["style"] == "code":
            opts.update(background=self.th.c["hover"], lmargin1=int(indent + 6 * pt), lmargin2=int(indent + 6 * pt))
        if a["style"] in ("math",):
            opts["justify"] = "center"
        if a["kind"]:
            opts["spacing3"] = int(2 * pt)
        self.t.tag_configure(p, **opts)

    def _ensure_tag(self, t):
        if t in self._configured or not t.startswith("f:"):
            return
        self._configured.add(t)
        k, eq, v = t[2:].partition("=")
        c = self.th.c
        opts = {}
        if k == "u":
            opts["underline"] = True
        elif k == "s":
            opts["overstrike"] = True
        elif k == "sup":
            opts["offset"] = int(self.th.f_editor.metrics("ascent") * 0.35)
        elif k == "sub":
            opts["offset"] = -int(self.th.f_editor.metrics("descent") * 0.9)
        elif k == "code":
            opts["background"] = c["hover"]
        elif k == "color" and v:
            opts["foreground"] = v if self.th.name == "light" else _lighten(v)
        elif k == "hl" and v:
            opts["background"] = HIGHLIGHT.get(v, v if v.startswith("#") else "#FFF59D")
            opts["foreground"] = "#1E2227"
        elif k == "link":
            opts.update(foreground=c["accent"], underline=True)
        try:
            self.t.tag_configure(t, **opts)
        except tk.TclError:
            pass
        self.t.tag_lower(t, "prot") if "prot" in self.t.tag_names() else None
        if k == "link":
            self.t.tag_bind(t, "<Control-Button-1>", lambda e, url=v: self.app.open_link(url))

    def zoom(self):
        return self.th.size / 11.0

    def font_tag(self, p, ftag_list):
        """The F: tag that draws text of these f: tags in a paragraph of tag p."""
        a = pattrs(p or "P:normal")
        look = docmodel.LOOK.get(a["style"], docmodel.LOOK["normal"])
        fmt = fmt_of(ftag_list)
        base = float(self.base.size or 11.0)
        size = self.th.size * (fmt["size"] / base if fmt.get("size") else look["scale"])
        if fmt.get("sup") or fmt.get("sub"):
            size *= 0.7
        family = self.th.families[self.th.editor_family]
        want = fmt.get("font") or self.base.font
        if want and want in _families(self.t):
            family = want
        if fmt.get("code") or a["style"] == "code":
            family = self.th.families["mono"]
        bold = bool(fmt.get("b") or look["bold"])
        italic = bool(fmt.get("i") or look["italic"])
        size = max(6, int(round(size)))
        name = f"F:{family}|{size}|{int(bold)}|{int(italic)}"
        if name not in self.fonts:
            self.fonts[name] = tkfont.Font(self.t, family=family, size=size, weight="bold" if bold else "normal",
                                           slant="italic" if italic else "roman")
            self.t.tag_configure(name, font=self.fonts[name])
            self.t.tag_lower(name)
        return name

    def restyle(self, i1="1.0", i2="end"):
        """Redraw fonts between i1 and i2 from the f: and P: tags."""
        i1, i2 = self.t.index(i1), self.t.index(i2)
        if not self.t.compare(i1, "<", i2):
            return
        active = {t for t in self.t.tag_names(i1) if keep(t)}
        pieces = []
        for key, val, idx in self.t.dump(i1, i2, text=True, tag=True):
            if key == "tagon":
                if keep(val):
                    active.add(val)
            elif key == "tagoff":
                active.discard(val)
            elif key == "text":
                p = next((t for t in active if t.startswith("P:")), "P:normal")
                pieces.append((idx, val, self.font_tag(p, [t for t in active if t.startswith("f:")])))
        for name in list(self.fonts):
            self.orig("tag", "remove", name, i1, i2)
        groups = {}
        for idx, val, name in pieces:
            groups.setdefault(name, []).extend([idx, f"{idx} +{len(val)}c"])
        for name, idx in groups.items():
            for k in range(0, len(idx), 1000):
                self.orig("tag", "add", name, *idx[k:k + 1000])
        self._break_spacing(i1, i2)

    def _break_spacing(self, i1, i2):
        """Lines joined by a line break (in a code block, or Shift+Enter) belong to one paragraph: no
        paragraph spacing between them. Tk spaces every line of text, so the lines around a break get
        tags that take the spacing away."""
        if "br:end" not in self._configured:
            self._configured.add("br:end")
            self.t.tag_configure("br:end", spacing3=0)
            self.t.tag_configure("br:start", spacing1=0)
        first = int(self.t.index(f"{i1} -1l").split(".")[0])
        last = int(self.t.index(i2).split(".")[0])
        self.orig("tag", "remove", "br:end", f"{first}.0", f"{last}.end +1c")
        self.orig("tag", "remove", "br:start", f"{first}.0", f"{last}.end +1c")
        r = self.t.tag_ranges("br")
        for s, e in zip(r[::2], r[1::2]):
            line = int(str(s).split(".")[0])
            if line < first or line > last:
                continue
            self.orig("tag", "add", "br:end", f"{line}.0", f"{line}.end +1c")
            self.orig("tag", "add", "br:start", f"{line + 1}.0", f"{line + 1}.end +1c")

    def retheme(self):
        """The zoom or the theme changed: redraw fonts, paragraph spacing and objects."""
        if not self.active:
            return
        old = dict(self.fonts)
        self.fonts.clear()
        for name in old:
            self.t.tag_delete(name)
        for p in [t for t in self._configured if t.startswith("P:")]:
            if p in self.t.tag_names():
                self._style_ptag(p)
        for t in [t for t in self._configured if t.startswith("f:")]:
            self._configured.discard(t)
            self._ensure_tag(t)
        self.restyle("1.0", "end")
        for oid in list(self.objects):
            self._redraw(oid)
        self.renumber()

    # -- objects ------------------------------------------------------------------------------------
    def _new_id(self):
        self._next += 1
        return f"obj{self._next}"

    def _create(self, oid, obj, index, p, extra=()):
        """Embed an object at index (no recording)."""
        from .ui import objects as ov
        self.objects[oid] = obj
        view = ov.view_for(self, oid, obj)
        if isinstance(view, tk.PhotoImage):
            name = self.orig("image", "create", index, "-image", view, "-name", oid, "-align", "baseline")
            if str(name) != oid:  # the name was taken (should not happen): keep the one Tk gave
                self.objects[str(name)] = self.objects.pop(oid)
                oid = str(name)
        else:
            self.orig("window", "create", index, "-window", view, "-align", ov.align_for(obj))
            self.windows[str(view)] = oid
        self.views[oid] = view
        at = self.object_index(oid)
        if at:
            self.orig("tag", "add", "obj", at)
            for t in [p] + [t for t in extra if t != p]:
                if keep(t):
                    self.orig("tag", "add", t, at)
        return oid

    def object_index(self, oid):
        view = self.views.get(oid)
        if view is None:
            return None
        try:
            return self.t.index(oid if isinstance(view, tk.PhotoImage) else view)
        except tk.TclError:
            return None

    def object_at(self, index):
        index = self.t.index(index)
        for key, val, _ in self.t.dump(index, f"{index} +1c", image=True, window=True):
            oid = val if key == "image" else self.windows.get(val)
            if oid in self.objects:
                return oid
        return None

    def _sync(self, oid):
        """Read an object's widget back into its dict (tables are edited in place)."""
        view = self.views.get(oid)
        if view is not None and hasattr(view, "read"):
            try:
                self.objects[oid] = view.read()
            except tk.TclError:
                pass

    def _sync_from(self, oid, path):
        self._sync(oid)

    def set_object(self, oid, obj, record=True):
        """Replace an object with an edited version, as one step for undo."""
        if oid not in self.objects:
            return
        self._sync(oid)
        before = copy.deepcopy(self.objects[oid])
        self.objects[oid] = obj
        self._redraw(oid)
        if record and not self.quiet:
            self.history.separator()
            self.history.record(Change(oid, before, copy.deepcopy(obj)))
            self.history.separator()
            self.t.edit_modified(True)
        self.app.later("rich-renumber", 80, self.renumber)

    def _redraw(self, oid):
        from .ui import objects as ov
        view = self.views.get(oid)
        if view is None:
            return
        obj = self.objects[oid]
        if isinstance(view, tk.PhotoImage):
            img = ov.view_for(self, oid, obj)
            if isinstance(img, tk.PhotoImage):
                try:
                    self.orig("image", "configure", oid, "-image", img)
                    self.views[oid] = img
                except tk.TclError:
                    pass
        elif hasattr(view, "update_object"):
            view.update_object(obj)

    def insert_object(self, obj):
        """Put a new object at the cursor (replacing the selection) as one step for undo. Tables, rules,
        page breaks and display formulas get a paragraph of their own."""
        oid = self._new_id()
        block = obj.get("type") in ("table", "rule", "pagebreak") or (obj.get("type") == "math" and obj.get("display"))
        with self.app.one_step():
            sel = self.t.tag_ranges("sel")
            if sel:
                self.t.delete(sel[0], sel[1])
            if block:
                # split the paragraph so that the object stands on an empty one of its own
                s, _ = self.para_range("insert")
                if self.t.compare("insert", ">", self._text_start(s)):
                    self.t.insert("insert", "\n")
                if self.t.get("insert", "insert lineend").strip() or self._is_object("insert"):
                    self.t.insert("insert", "\n")
                    self.t.mark_set("insert", "insert -1c")
                s, e = self.para_range("insert")
                p = self.para_tag(s)
                newp = ptag("math" if obj.get("type") == "math" else "normal")
                end = e if self.t.compare(e, "<", "end") else "end"
                if newp != p:
                    self.t.tag_remove(p, s, end)
                    self._ensure_ptag(newp)
                    self.t.tag_add(newp, s, end)
                self._fixing = True
                try:
                    self._fix_markers(s, e, False)
                finally:
                    self._fixing = False
                self.t.mark_set("insert", s)
            at = self.t.index("insert")
            p = self.para_tag(at)
            self._begin()
            try:
                self._create(oid, obj, at, p)
                i2 = self.t.index(f"{at} +1c")
                self.history.record(Put(self.t.index(at), i2, self.fragment(at, i2)))
            finally:
                self._end()
            self.t.mark_set("insert", f"{at} +1c")
        self.t.edit_modified(True)
        self.app.later("rich-renumber", 80, self.renumber)
        self.t.see("insert")
        return oid

    def _object_click(self, e):
        idx = self.t.index(f"@{e.x},{e.y}")
        oid = self.object_at(idx)
        if oid:
            self.app.rich_edit_object(oid)
        return "break"

    # -- formatting commands --------------------------------------------------------------------
    def selection(self):
        sel = self.t.tag_ranges("sel")
        return (str(sel[0]), str(sel[1])) if sel else None

    def state(self):
        """What the toolbar shows: the formatting at the cursor (or the start of the selection) and the
        paragraph's attributes."""
        sel = self.selection()
        at = sel[0] if sel else self.t.index("insert")
        src = at
        if not sel and self.t.compare(at, ">", f"{at} linestart") and not self._is_object(f"{at} -1c"):
            src = f"{at} -1c"
        fmt = fmt_of(self.t.tag_names(src))
        if not sel and self.pending:
            fmt.update(self.pending)
            fmt = {k: v for k, v in fmt.items() if v}
        return fmt, pattrs(self.para_tag(at))

    def toggle(self, name):
        sel = self.selection()
        tag = f"f:{name}"
        if not sel:
            cur = self.state()[0].get(name, False)
            self.pending[name] = not cur
            if name in ("sup", "sub") and not cur:
                self.pending["sub" if name == "sup" else "sup"] = False
            self._changed()
            return
        on = self._all_tagged(tag, *sel)
        with self.app.one_step():
            if on:
                self.t.tag_remove(tag, *sel)
            else:
                self._ensure_tag(tag)
                if name in ("sup", "sub"):
                    self.t.tag_remove("f:sub" if name == "sup" else "f:sup", *sel)
                self.t.tag_add(tag, *sel)
        self.restyle(*sel)
        self._changed()

    def _all_tagged(self, tag, a, b):
        r = self.ranges_in(tag, a, b)
        if not r:
            return False
        covered = self.t.count(a, b, "indices")[0] if self.t.count(a, b, "indices") else 0
        got = sum((self.t.count(x, y, "indices") or (0,))[0] for x, y in zip(r[::2], r[1::2]))
        return got >= covered

    def set_value(self, kind, value):
        """size / font / color / hl / link for the selection (None removes it)."""
        sel = self.selection()
        if not sel:
            self.pending[kind] = value
            self._changed()
            return
        with self.app.one_step():
            for t in [t for t in self.t.tag_names() if t.startswith(f"f:{kind}=")]:
                if self.ranges_in(t, *sel):
                    self.t.tag_remove(t, *sel)
            if value not in (None, ""):
                tag = ftags({kind: value})[0]
                self._ensure_tag(tag)
                self.t.tag_add(tag, *sel)
        self.restyle(*sel)
        self._changed()

    def clear_format(self):
        sel = self.selection()
        if not sel:
            self.pending = {}
            self._changed()
            return
        with self.app.one_step():
            for t in [t for t in self.t.tag_names() if t.startswith("f:")]:
                if self.ranges_in(t, *sel):
                    self.t.tag_remove(t, *sel)
        self.restyle(*sel)
        self._changed()

    def set_para(self, **changes):
        """Change attributes of the paragraphs in the selection (or at the cursor)."""
        sel = self.selection() or (self.t.index("insert"), self.t.index("insert"))
        with self.app.one_step():
            for s in self.paragraphs(*sel):
                s, e = self.para_range(s)
                old = self.para_tag(s)
                a = pattrs(old)
                a.update(changes)
                if a["style"] in HEADINGS + ("code",) and "kind" not in changes:
                    a["kind"] = None
                if a["kind"] is None:
                    a["level"] = 0
                new = ptag(a["style"], a["align"], a["kind"], a["level"], a["indent"], a["lang"])
                end = e if self.t.compare(e, "<", "end") else "end"
                if new != old:
                    self.t.tag_remove(old, s, end)
                    self._ensure_ptag(new)
                    self.t.tag_add(new, s, end)
                self._fix_markers(s, e, a["kind"] is not None)
                self.restyle(s, e)
        self.renumber()
        self._changed()

    def toggle_list(self, kind):
        a = self.state()[1]
        self.set_para(kind=None if a["kind"] == kind else kind)

    def change_level(self, d):
        a = self.state()[1]
        if a["kind"]:
            self.set_para(level=max(0, min(8, a["level"] + d)))
        else:
            self.set_para(indent=max(0, min(10, a["indent"] + d)))

    # -- keys -----------------------------------------------------------------------------------
    def _key_return(self, e):
        s, _ = self.para_range("insert")
        a = pattrs(self.para_tag(s))
        if a["style"] == "code":
            return self._key_soft_return(e)
        if a["kind"] and not self.t.get(self._text_start(s), f"{s} lineend").strip() and \
                not self.t.tag_ranges("sel"):
            self.set_para(kind=None)  # Enter on an empty list item ends the list
            return "break"
        return "continue"

    def _key_soft_return(self, e):
        with self.app.one_step():
            sel = self.t.tag_ranges("sel")
            if sel:
                self.t.delete(sel[0], sel[1])
            at = self.t.index("insert")
            p = self.para_tag(at)
            self.t.insert(at, "\n", (p, "br"))
        self.t.see("insert")
        return "break"

    def _key_tab(self, d):
        a = self.state()[1]
        if a["kind"]:
            self.change_level(d)
            return "break"
        return "continue" if d > 0 else "break"

    def _key_backspace(self, e):
        if self.t.tag_ranges("sel"):
            return "continue"
        s, _ = self.para_range("insert")
        a = pattrs(self.para_tag(s))
        if a["kind"] and self.t.compare("insert", "==", self._text_start(s)):
            self.set_para(kind=None)
            return "break"
        if self.t.compare("insert", "==", s) and a["indent"]:
            self.set_para(indent=a["indent"] - 1)
            return "break"
        return "continue"

    def _key_home(self, e):
        self.app.root.after_idle(self._skip_marker)
        return "continue"

    def _after_click(self, e):
        self.pending = {}
        self.app.root.after_idle(self._skip_marker)

    def _skip_marker(self):
        if not self.active:
            return
        try:
            at = self.t.index("insert")
            if at.endswith(".0") and self._text_start(at) != at and not self.t.tag_ranges("sel"):
                self.t.mark_set("insert", f"{at} +1c")
        except tk.TclError:
            pass

    # -- clipboard with formatting -------------------------------------------------------------
    def _copy(self, e):
        sel = self.t.tag_ranges("sel")
        if sel:
            frag = [x for x in self.fragment(str(sel[0]), str(sel[1])) if x[0] != "m"]
            self.clip = (self.t.get(sel[0], sel[1]), frag)
        return "continue"

    def _paste(self, e):
        if not self.clip:
            return "continue"
        try:
            text = self.t.clipboard_get()
        except tk.TclError:
            return "continue"
        if text != self.clip[0]:
            return "continue"
        frag = []
        for item in self.clip[1]:
            if item[0] == "o":
                frag.append(("o", self._new_id(), copy.deepcopy(item[2]), item[3]))
            else:
                frag.append(item)
        with self.app.one_step():
            sel = self.t.tag_ranges("sel")
            if sel:
                self.t.delete(sel[0], sel[1])
            at = self.t.index("insert")
            self._begin()
            try:
                end = self.put(at, frag)
                self.history.record(Put(at, end, self.fragment(at, end)))
                self._normalize(at, end, newline=True)
            finally:
                self._end()
            self.t.mark_set("insert", end)
        self.t.edit_modified(True)
        self.t.see("insert")
        return "break"

    # -- pages ------------------------------------------------------------------------------------
    def column_px(self):
        """The width of the text column, in pixels."""
        w = self.t.winfo_width()
        pad = int(str(self.t.cget("padx")))
        if w < 50:
            return self.th.f_editor.measure("n") * 84
        return max(100, w - 2 * pad - self.th.px(4))

    def pt_px(self):
        """Pixels per point for objects: the page's text width fills the column."""
        width = float((self.meta.get("page") or {}).get("width", 595.28))
        margins = (self.meta.get("page") or {}).get("margins") or (72, 72, 72, 72)
        text_w = max(100.0, width - float(margins[1]) - float(margins[3]))
        return self.column_px() / text_w

    def set_page(self, page):
        self.meta["page"] = docmodel.to_native(page)
        self.base.page = page
        for oid in list(self.objects):
            self._redraw(oid)
        self.t.edit_modified(True)


def _split_breaks(chunk):
    """A piece of flat text as (text, is a line break) pieces: U+2028 becomes a newline tagged "br"."""
    out = []
    parts = chunk.split(BR)
    for k, part in enumerate(parts):
        if part:
            out.append((part, False))
        if k < len(parts) - 1:
            out.append(("\n", True))
    return out


_FAMILIES = None


def _families(w):
    global _FAMILIES
    if _FAMILIES is None:
        try:
            _FAMILIES = set(tkfont.families(w))
        except tk.TclError:
            _FAMILIES = set()
    return _FAMILIES


def _lighten(color):
    """A text colour made readable on a dark background."""
    try:
        r, g, b = (int(color[k:k + 2], 16) for k in (1, 3, 5))
    except (ValueError, IndexError):
        return color
    lum = 0.299 * r + 0.587 * g + 0.114 * b
    if lum >= 110:
        return color
    f = 0.55
    return "#%02X%02X%02X" % tuple(int(v + (255 - v) * f) for v in (r, g, b))
