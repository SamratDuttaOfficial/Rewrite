"""Reading PDF files for conversion: PDFium gives every character with its box and font, the pictures and
the drawings; the Rust core (pdfextract) turns those into paragraphs, headings, lists and so on, and asks
for parts of pages (charts, equations) to be drawn as pictures."""
import ctypes
import re

from . import native, pdfcore

_BOLD = re.compile(r"bold|black|heavy|semibold|demi|cmbx|cmb\d|sfbx|bx\d")
_ITALIC = re.compile(r"italic|oblique|cmti|cmmi|cmsl|sfti|sfsl|cmbxti")
_MONO = re.compile(r"mono|courier|consolas|typewriter|cmtt|sftt|inconsolata|menlo")


def _font_style(tp, i, buf, cache):
    """(bold, italic, fixed pitch) of character i, cached per text object."""
    import pypdfium2.raw as raw
    obj = raw.FPDFText_GetTextObject(tp, i)
    key = ctypes.cast(obj, ctypes.c_void_p).value if obj else None
    if key in cache:
        return cache[key]
    flags = ctypes.c_int(0)
    n = raw.FPDFText_GetFontInfo(tp, i, buf, len(buf), ctypes.byref(flags))
    name = bytes(buf[:max(0, n - 1)]).decode("latin-1", "replace").lower() if n else ""
    weight = raw.FPDFText_GetFontWeight(tp, i)
    bold = weight >= 600 or bool(_BOLD.search(name)) or bool(flags.value & 0x40000)
    italic = bool(_ITALIC.search(name)) or bool(flags.value & 0x40)
    mono = bool(flags.value & 1) or bool(_MONO.search(name))
    cache[key] = (bold, italic, mono)
    return cache[key]


def _chars(tp, h):
    """The characters of a text page: (text, x0, top, x1, bottom, size, bold, italic, generated, mono,
    baseline), in points from the top-left."""
    import pypdfium2.raw as raw
    chars = []
    buf = (ctypes.c_char * 256)()
    cache = {}
    left, right, bottom, top = (ctypes.c_double() for _ in range(4))
    ox, oy = ctypes.c_double(), ctypes.c_double()
    loose = raw.FS_RECTF()
    for i in range(raw.FPDFText_CountChars(tp)):
        u = raw.FPDFText_GetUnicode(tp, i)
        if u in (0xFFFE, 0xFFFF):
            continue
        ch = chr(u) if u < 0x110000 else "?"
        generated = raw.FPDFText_IsGenerated(tp, i) == 1
        if not raw.FPDFText_GetCharBox(tp, i, ctypes.byref(left), ctypes.byref(right), ctypes.byref(bottom),
                                       ctypes.byref(top)):
            left.value = right.value = bottom.value = top.value = 0.0
        size = raw.FPDFText_GetFontSize(tp, i)
        if size <= 1.5 and ch.strip() and raw.FPDFText_GetLooseCharBox(tp, i, ctypes.byref(loose)):
            size = max(size, (loose.top - loose.bottom) / 1.15)  # a size of 1 scaled up by the text matrix
        bold, italic, mono = _font_style(tp, i, buf, cache) if not generated else (False, False, False)
        x0, x1 = left.value, right.value
        t, b = h - top.value, h - bottom.value
        if ch.strip() and size > 0 and (b - t) < size * 0.3:  # tiny boxes: use the font size
            t, b = b - size * 0.75, b + size * 0.2
        base = h - oy.value if raw.FPDFText_GetCharOrigin(tp, i, ctypes.byref(ox), ctypes.byref(oy)) else b
        chars.append((ch, x0, t, x1, b, float(size), bold, italic, generated, mono, base))
    return chars


def _transform(m, l, b, r, t):
    """Bounds (left, bottom, right, top) mapped by a matrix (a, b, c, d, e, f)."""
    if m is None:
        return l, b, r, t
    a, bb, c, d, e, f = m
    pts = [(a * x + c * y + e, bb * x + d * y + f) for x, y in ((l, b), (l, t), (r, b), (r, t))]
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    return min(xs), min(ys), max(xs), max(ys)


def _compose(outer, inner):
    """The matrix applying inner, then outer."""
    if outer is None:
        return inner
    if inner is None:
        return outer
    a1, b1, c1, d1, e1, f1 = inner
    a2, b2, c2, d2, e2, f2 = outer
    return (a2 * a1 + c2 * b1, b2 * a1 + d2 * b1, a2 * c1 + c2 * d1, b2 * c1 + d2 * d1,
            a2 * e1 + c2 * f1 + e2, b2 * e1 + d2 * f1 + f2)


def _objects(page, w, h):
    """The page's pictures [(x0, top, x1, bottom, raw object, depth)] and drawings [(x0, top, x1, bottom,
    kind)]: kind 0 a path, 1 a group (form) holding drawings or pictures, 2 a white or invisible fill.
    Groups covering most of the page are looked into; smaller ones count as one drawing."""
    import pypdfium2.raw as raw
    pics, shapes = [], []
    area = w * h
    l, b, r, t = (ctypes.c_float() for _ in range(4))
    budget = [40000]

    def bounds(o, m):
        if not raw.FPDFPageObj_GetBounds(o, ctypes.byref(l), ctypes.byref(b), ctypes.byref(r), ctypes.byref(t)):
            return None
        x0, y0, x1, y1 = _transform(m, l.value, b.value, r.value, t.value)
        return x0, h - y1, x1, h - y0

    def holds(form, depth):
        """Whether a group draws anything but text."""
        for i in range(raw.FPDFFormObj_CountObjects(form)):
            o = raw.FPDFFormObj_GetObject(form, i)
            ty = raw.FPDFPageObj_GetType(o)
            if ty in (raw.FPDF_PAGEOBJ_PATH, raw.FPDF_PAGEOBJ_SHADING, raw.FPDF_PAGEOBJ_IMAGE):
                return True
            if ty == raw.FPDF_PAGEOBJ_FORM and depth < 4 and holds(o, depth + 1):
                return True
        return False

    def invisible(o):
        fill, stroke = ctypes.c_int(), ctypes.c_int()
        if not raw.FPDFPath_GetDrawMode(o, ctypes.byref(fill), ctypes.byref(stroke)):
            return False
        if not fill.value and not stroke.value:
            return True
        if stroke.value:
            return False
        cr, cg, cb, ca = (ctypes.c_uint() for _ in range(4))
        if raw.FPDFPageObj_GetFillColor(o, ctypes.byref(cr), ctypes.byref(cg), ctypes.byref(cb), ctypes.byref(ca)):
            return ca.value == 0 or min(cr.value, cg.value, cb.value) >= 250
        return False

    def walk(objs, m, depth):
        for o in objs:
            budget[0] -= 1
            if budget[0] < 0:
                return
            ty = raw.FPDFPageObj_GetType(o)
            if ty == raw.FPDF_PAGEOBJ_TEXT:
                continue
            box = bounds(o, m)
            if box is None:
                continue
            x0, top, x1, bottom = box
            if ty == raw.FPDF_PAGEOBJ_IMAGE:
                if x1 - x0 >= 24 and bottom - top >= 24:
                    pics.append((x0, top, x1, bottom, o, depth))
            elif ty == raw.FPDF_PAGEOBJ_FORM:
                if (x1 - x0) * (bottom - top) > 0.5 * area and depth < 6:
                    fm = raw.FS_MATRIX()
                    inner = (fm.a, fm.b, fm.c, fm.d, fm.e, fm.f) if raw.FPDFPageObj_GetMatrix(o, ctypes.byref(fm)) else None
                    walk([raw.FPDFFormObj_GetObject(o, i) for i in range(raw.FPDFFormObj_CountObjects(o))],
                         _compose(m, inner), depth + 1)
                elif holds(o, 0):
                    shapes.append((x0, top, x1, bottom, 1))
            elif ty == raw.FPDF_PAGEOBJ_PATH:
                shapes.append((x0, top, x1, bottom, 2 if invisible(o) else 0))
            elif ty == raw.FPDF_PAGEOBJ_SHADING:
                shapes.append((x0, top, x1, bottom, 0))

    walk([raw.FPDFPage_GetObject(page.raw, i) for i in range(raw.FPDFPage_CountObjects(page.raw))], None, 0)
    return pics, shapes


def _png(arr, mode="BGR"):
    """PNG bytes from a PDFium bitmap array (BGR, BGRx or BGRA, or grey); in BGRA, see-through parts become
    white (in BGRx the fourth byte is only padding)."""
    import numpy as np
    if arr.ndim == 2 or arr.shape[2] == 1:
        g = arr.reshape(arr.shape[0], arr.shape[1])
        rgb = np.dstack([g, g, g])
    elif arr.shape[2] == 4 and mode == "BGRA":
        a = arr[:, :, 3:4].astype(np.float32) / 255.0
        rgb = (arr[:, :, [2, 1, 0]].astype(np.float32) * a + 255.0 * (1.0 - a)).round().astype(np.uint8)
    else:
        rgb = arr[:, :, [2, 1, 0]]
    rgb = np.ascontiguousarray(rgb)
    return native.image_encode_png(rgb.tobytes(), rgb.shape[1], rgb.shape[0], 3)


def read_pages(doc, images=True, progress=None, cancel=None):
    """Every page of an open PdfDocument as {"width", "height", "chars", "images", "shapes"} for
    native.pdf_document. PDFium is held only a page at a time."""
    import pypdfium2 as pdfium
    pages = []
    with pdfcore.LOCK:
        n = len(doc)
    for k in range(n):
        if cancel is not None and cancel.is_set():
            raise InterruptedError("cancelled")
        if progress:
            progress(k, n)
        with pdfcore.LOCK:
            page = doc[k]
            try:
                w, h = page.get_size()
                tp = page.get_textpage()
                try:
                    chars = _chars(tp, h)
                finally:
                    tp.close()
                found, shapes = _objects(page, w, h)
                pics = []
                for x0, top, x1, bottom, raw_obj, depth in (found if images else []):
                    try:
                        obj = pdfium.PdfObject(raw_obj, page=page, pdf=doc, level=depth)
                        bm = obj.get_bitmap(render=True)
                        pics.append({"x0": x0, "top": top, "x1": x1, "bottom": bottom,
                                     "data": _png(bm.to_numpy(), bm.mode)})
                    except Exception:
                        continue
            finally:
                page.close()
        pages.append({"width": float(w), "height": float(h), "chars": chars, "images": pics, "shapes": shapes})
    return pages


def _renderer(doc):
    """render(page, x0, top, x1, bottom) -> PNG of that part of a page, sharp enough to read."""
    def render(k, x0, top, x1, bottom):
        try:
            with pdfcore.LOCK:
                page = doc[k]
                try:
                    w, h = page.get_size()
                    x0, top, x1, bottom = max(0.0, x0), max(0.0, top), min(w, x1), min(h, bottom)
                    if x1 - x0 < 2 or bottom - top < 2:
                        return None
                    scale = min(4.0, max(2.0, 900.0 / max(x1 - x0, bottom - top)))
                    arr = page.render(scale=scale, crop=(x0, h - bottom, w - x1, top)).to_numpy()
                finally:
                    page.close()
            return _png(arr)
        except Exception:
            return None
    return render


def to_document(data, options=None, helper=None, progress=None, cancel=None):
    """A Document from PDF bytes. options: drop_headers, fix_words, images, tables (all on by default)."""
    import pypdfium2 as pdfium
    opts = dict(options or {})
    with pdfcore.LOCK:
        doc = pdfium.PdfDocument(data)
    try:
        pages = read_pages(doc, opts.get("images", True), progress, cancel)
        return native.pdf_document(pages, opts, helper, _renderer(doc))
    finally:
        with pdfcore.LOCK:
            doc.close()


def first_page_png(data, scale=2.0):
    """The first page of a PDF as PNG bytes (LaTeX figures that are PDF files)."""
    try:
        import pypdfium2 as pdfium
        with pdfcore.LOCK:
            doc = pdfium.PdfDocument(data)
            try:
                arr = doc[0].render(scale=scale).to_numpy()
            finally:
                doc.close()
        return _png(arr)
    except Exception:
        return None
