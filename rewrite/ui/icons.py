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


_EYE = [A(12, 15.64, 10.64, -160, -20), A(12, 8.36, 10.64, 20, 160), A(12, 12, 3, 0, 360)]
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
