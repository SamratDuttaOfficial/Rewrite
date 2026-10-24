"""Button icons, drawn at run time so they are sharp at any screen scale and take the theme's colours.

An icon is a few strokes on a 24 x 24 grid:
    L(x1, y1, x2, y2, ...)   a line through the points
    A(cx, cy, r, from, to)   part of a circle; degrees, 0 = right, 90 = down
"""
import math
import struct
import zlib

GRID = 24
STROKE = 1.9  # line width, in grid units


def L(*xy):
    return list(zip(xy[::2], xy[1::2])), True


def A(cx, cy, r, a0, a1):
    n = max(2, int(abs(a1 - a0) / 12))
    angles = [math.radians(a0 + (a1 - a0) * k / n) for k in range(n + 1)]
    return [(cx + r * math.cos(a), cy + r * math.sin(a)) for a in angles], False


def _flip(strokes):
    return [([(GRID - x, y) for x, y in pts], straight) for pts, straight in strokes]


def _vflip(strokes):
    return [([(x, GRID - y) for x, y in pts], straight) for pts, straight in strokes]


def _capsule(x1, y1, x2, y2, r):
    """A rounded bar from (x1, y1) to (x2, y2), r wide on each side."""
    a = math.degrees(math.atan2(y2 - y1, x2 - x1))
    nx, ny = -math.sin(math.radians(a)), math.cos(math.radians(a))
    return [A(x2, y2, r, a - 90, a + 90), A(x1, y1, r, a + 90, a + 270),
            L(x1 + r * nx, y1 + r * ny, x2 + r * nx, y2 + r * ny), L(x1 - r * nx, y1 - r * ny, x2 - r * nx, y2 - r * ny)]


def _move(strokes, dx, dy):
    return [([(x + dx, y + dy) for x, y in pts], straight) for pts, straight in strokes]


def _lines(*rows):
    """Text lines: (x1, x2, y) each."""
    return [L(x1, y, x2, y) for x1, x2, y in rows]


_EYE = [A(12, 15.64, 10.64, -160, -20), A(12, 8.36, 10.64, 20, 160), A(12, 12, 3, 0, 360)]
_PAGE = [L(6, 3, 14.5, 3, 19, 7.5, 19, 21, 6, 21, 6, 3), L(14.5, 3, 14.5, 7.5, 19, 7.5)]
_TWO = [L(15, 3.5, 16, 2.6, 18.6, 2.6, 19.6, 3.6, 19.6, 4.8, 15, 9, 20, 9)]
_UNDO = [L(9, 14, 4, 9, 9, 4), L(4, 9, 14, 9), A(14, 15, 6, -90, 0), L(20, 15, 20, 19)]
_REFRESH = [A(11.5, 12.5, 8, 20, 318), L(17.45, 7.15, 20.5, 9.9), L(20.5, 4.4, 20.5, 9.9, 15, 9.9)]
_UP = [L(12, 20, 12, 5), L(6, 11, 12, 5, 18, 11)]

ICONS = {
    "open": [L(3, 6, 9, 6, 11, 9, 21, 9, 21, 19, 3, 19, 3, 6)],
    "save": [L(4, 4, 16, 4, 20, 8, 20, 20, 4, 20, 4, 4), L(8, 4, 8, 9, 15, 9), L(8, 20, 8, 14, 16, 14, 16, 20)],
    "undo": _UNDO,
    "redo": _flip(_UNDO),
    "pencil": [L(4, 20, 4, 16, 15, 5, 19, 9, 8, 20, 4, 20), L(13, 7, 17, 11)],
    "wand": [L(3, 21, 13, 11), L(17, 3, 17, 11), L(13, 7, 21, 7), L(20, 15, 20, 19), L(18, 17, 22, 17),
             L(7.5, 3.5, 7.5, 6.5), L(6, 5, 9, 5)],
    "eye": _EYE,
    "eye-off": _EYE + [L(4, 3.5, 20, 20.5)],
    "bulb": [A(12, 9, 6, 140, 400), L(16.6, 12.86, 15, 15.5, 9, 15.5, 7.4, 12.86), L(9.5, 18.5, 14.5, 18.5),
             L(10.5, 21.5, 13.5, 21.5)],
    "diff": [L(12, 4, 12, 13), L(7.5, 8.5, 16.5, 8.5), L(7.5, 19, 16.5, 19)],
    "moon": [A(12, 12, 9, 5, 265), A(16.84, 7.16, 7, 53.5, 216.5)],
    "sun": [A(12, 12, 4, 0, 360)] + [L(12 + 7.2 * math.cos(a), 12 + 7.2 * math.sin(a),
                                        12 + 9.8 * math.cos(a), 12 + 9.8 * math.sin(a))
                                      for a in (math.radians(45 * k) for k in range(8))],
    "close": [L(6, 6, 18, 18), L(18, 6, 6, 18)],
    "plus": [L(12, 5, 12, 19), L(5, 12, 19, 12)],
    "minus": [L(5, 12, 19, 12)],
    "refresh": _REFRESH,
    "revert": _flip(_REFRESH),
    "check": [L(5, 12.5, 10, 17.5, 19, 7)],
    "check-all": [L(17, 6, 7, 17, 2, 12), L(22, 10, 14.5, 17.5, 13, 16)],
    "next": [L(9, 5, 16, 12, 9, 19)],
    "up": _UP,
    "down": _vflip(_UP),
    "chev-right": [L(10, 7, 15, 12, 10, 17)],
    "chev-down": [L(7, 10, 12, 15, 17, 10)],
    "search": [A(10.5, 10.5, 6.5, 0, 360), L(15.5, 15.5, 20.5, 20.5)],
    "book-plus": [L(5, 4, 19, 4, 19, 20, 5, 20, 5, 4), L(8.5, 4, 8.5, 20), L(13.75, 9, 13.75, 15),
                  L(10.75, 12, 16.75, 12)],
    "trash": [L(4, 7, 20, 7), L(6.5, 7, 7.5, 20, 16.5, 20, 17.5, 7), L(9.5, 7, 9.5, 4, 14.5, 4, 14.5, 7)],
    # text formatting
    "bold": [L(7, 4, 7, 20), L(7, 4, 12.5, 4), A(12.5, 8, 4, -90, 90), L(12.5, 12, 7, 12), A(13.5, 16, 4, -90, 90),
             L(13.5, 20, 7, 20), L(9, 4, 9, 20)],
    "italic": [L(10, 4, 18, 4), L(6, 20, 14, 20), L(14, 4, 10, 20)],
    "underline": [L(7, 3.5, 7, 11), A(12, 11, 5, 180, 0), L(17, 11, 17, 3.5), L(5, 21, 19, 21)],
    "strike": [A(12, 7.5, 3.8, -20, -270), A(12, 15.1, 3.8, -90, 160), L(4, 11.3, 20, 11.3)],
    "sup": [L(3.5, 9, 11.5, 20), L(11.5, 9, 3.5, 20)] + _TWO,
    "sub": [L(3.5, 4, 11.5, 15), L(11.5, 4, 3.5, 15)] + _move(_TWO, 0, 12.4),
    "color": [L(6, 17, 12, 3.5, 18, 17), L(8.3, 12, 15.7, 12), L(3.5, 21, 20.5, 21)],
    "highlight": [L(14, 3.5, 20, 9.5, 11, 18.5, 5, 12.5, 14, 3.5), L(5, 12.5, 3.5, 17.5, 6, 20, 11, 18.5),
                  L(3, 21.5, 21, 21.5)],
    "link": _capsule(5.5, 18.5, 10.2, 13.8, 3) + _capsule(13.8, 10.2, 18.5, 5.5, 3) + [L(9.5, 14.5, 14.5, 9.5)],
    "clear-format": [L(4, 4, 15, 4), L(9.5, 4, 9.5, 16), L(14, 14, 20.5, 20.5), L(20.5, 14, 14, 20.5)],
    "align-left": _lines((4, 20, 5), (4, 14, 9.5), (4, 20, 14), (4, 14, 18.5)),
    "align-center": _lines((4, 20, 5), (7, 17, 9.5), (4, 20, 14), (7, 17, 18.5)),
    "align-right": _lines((4, 20, 5), (10, 20, 9.5), (4, 20, 14), (10, 20, 18.5)),
    "align-justify": _lines((4, 20, 5), (4, 20, 9.5), (4, 20, 14), (4, 20, 18.5)),
    "list-bullet": [A(5, 6, 1.2, 0, 360), A(5, 12, 1.2, 0, 360), A(5, 18, 1.2, 0, 360)]
                   + _lines((9, 20, 6), (9, 20, 12), (9, 20, 18)),
    "list-number": [L(3.5, 4.2, 5.2, 3, 5.2, 9), L(3.4, 15.2, 4.2, 14.4, 5.8, 14.4, 6.6, 15.2, 6.6, 16.2, 3.4, 20,
                                                   7, 20)] + _lines((10, 20, 6), (10, 20, 17)),
    "indent": _lines((11, 20, 5), (11, 20, 10), (11, 20, 15), (4, 20, 20)) + [L(4, 8, 7.5, 11.5, 4, 15)],
    "outdent": _lines((11, 20, 5), (11, 20, 10), (11, 20, 15), (4, 20, 20)) + [L(7.5, 8, 4, 11.5, 7.5, 15)],
    "style": [L(4, 6, 4, 4, 14, 4, 14, 6), L(9, 4, 9, 16), L(15, 13, 15, 11, 21, 11, 21, 13), L(18, 11, 18, 20)],
    # inserting things
    "image": [L(3, 5, 21, 5, 21, 19, 3, 19, 3, 5), L(3, 16, 9, 10.5, 13, 14.5, 16, 11.5, 21, 16.5),
              A(15.5, 8.8, 1.5, 0, 360)],
    "table": [L(3, 5, 21, 5, 21, 19, 3, 19, 3, 5), L(3, 9.7, 21, 9.7), L(3, 14.3, 21, 14.3), L(9, 5, 9, 19),
              L(15, 5, 15, 19)],
    "chart": [L(3, 20.5, 21, 20.5), L(4.5, 20.5, 4.5, 13, 7.5, 13, 7.5, 20.5), L(10.5, 20.5, 10.5, 7, 13.5, 7, 13.5, 20.5),
              L(16.5, 20.5, 16.5, 10.5, 19.5, 10.5, 19.5, 20.5)],
    "sigma": [L(18, 5.5, 18, 4, 6, 4, 12.5, 12, 6, 20, 18, 20, 18, 18.5)],
    "footnote": _lines((3, 13, 7), (3, 21, 12), (3, 21, 16.5), (3, 14, 21)) + [L(16, 4.2, 18, 2.8, 18, 9)],
    "rule": [L(3, 12, 21, 12), L(3, 8.5, 3, 15.5), L(21, 8.5, 21, 15.5)],
    "page-break": [L(5, 3, 5, 8, 19, 8, 19, 3), L(5, 21, 5, 16, 19, 16, 19, 21), L(3, 12, 6.5, 12), L(10.25, 12, 13.75, 12),
                   L(17.5, 12, 21, 12)],
    "page-setup": _PAGE + _lines((9, 16, 11.5), (9, 16, 15), (9, 13.5, 18.5)),
    "export": [L(12, 15, 12, 3), L(7.5, 7.5, 12, 3, 16.5, 7.5), L(5, 12, 5, 20.5, 19, 20.5, 19, 12)],
    "file": _PAGE,
    "convert": [L(4, 8, 18.5, 8), L(14.5, 4, 18.5, 8, 14.5, 12), L(20, 16, 5.5, 16), L(9.5, 12, 5.5, 16, 9.5, 20)],
    # PDF tools
    "pointer": [L(6, 3, 6, 18.5, 10, 14.8, 12.8, 21, 15.3, 19.9, 12.5, 13.8, 17.8, 13.8, 6, 3)],
    "type": [L(5, 6, 5, 4, 19, 4, 19, 6), L(12, 4, 12, 20), L(9, 20, 15, 20)],
    "note": [L(4, 5, 20, 5, 20, 16, 12, 16, 8, 20, 8, 16, 4, 16, 4, 5), L(8, 9, 16, 9), L(8, 12.5, 13.5, 12.5)],
    "draw": [L(3, 17, 5.5, 12, 8.5, 9.5, 10.5, 11.5, 10, 16, 12.5, 18, 15.5, 13.5, 17, 9, 19.5, 7, 21, 9)],
    "eraser": [L(9.5, 20, 4, 14.5, 14.5, 4, 21, 10.5, 11.5, 20, 9.5, 20), L(9, 9.5, 15.5, 16), L(9.5, 20, 21, 20)],
    "box": [L(4, 5, 20, 5, 20, 19, 4, 19, 4, 5)],
    "rotate-left": _flip(_REFRESH),
    "rotate-right": _REFRESH,
    "page-add": _PAGE + [L(12.5, 10.5, 12.5, 17.5), L(9, 14, 16, 14)],
    "pages": [L(8, 3, 20, 3, 20, 17), L(4, 7, 16, 7, 16, 21, 4, 21, 4, 7)],
    "merge": [L(8, 3, 20, 3, 20, 17), L(4, 7, 16, 7, 16, 21, 4, 21, 4, 7), L(10, 11, 10, 17), L(7, 14, 13, 14)],
}

_masks = {}


def mask(name, size):
    """How much of each pixel the icon covers: size * size values from 0 to 1, row by row."""
    key = (name, size)
    if key in _masks:
        return _masks[key]
    k = size / GRID
    reach = max(STROKE / 2 * k, 0.6) + 0.5  # cover fades out half a pixel beyond the edge of the stroke
    out = [0.0] * (size * size)
    for pts, straight in ICONS[name]:
        pts = [(x * k, y * k) for x, y in pts]
        if straight:  # straight lines look cleaner when they start and end on the pixel grid
            pts = [(round(x * 2) / 2, round(y * 2) / 2) for x, y in pts]
        for (x1, y1), (x2, y2) in zip(pts, pts[1:]):
            dx, dy = x2 - x1, y2 - y1
            len2 = dx * dx + dy * dy
            for j in range(max(0, int(min(y1, y2) - reach)), min(size, int(max(y1, y2) + reach) + 1)):
                py, row = j + 0.5 - y1, j * size
                for i in range(max(0, int(min(x1, x2) - reach)), min(size, int(max(x1, x2) + reach) + 1)):
                    px = i + 0.5 - x1
                    t = min(1.0, max(0.0, (px * dx + py * dy) / len2)) if len2 else 0.0
                    a = reach - math.hypot(px - t * dx, py - t * dy)
                    if a > out[row + i]:
                        out[row + i] = min(1.0, a)
    _masks[key] = out
    return out


def png(name, size, rgb):
    """The icon as PNG data: one colour, see-through around the strokes."""
    cover, color = mask(name, size), bytes(rgb)
    rows = [b"\x00" + b"".join(color + bytes((int(a * 255 + 0.5),)) for a in cover[y * size:(y + 1) * size])
            for y in range(size)]

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))
