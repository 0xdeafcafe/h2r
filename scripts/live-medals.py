#!/usr/bin/env python3
"""Redraw Live's own medals, and draw Incel, in the superbounce medals' style.

A companion to `scripts/superbounce-medals.py`. The seven bounce medals are
the approved set and are not redrawn: that script's drawing is copied below,
unchanged, for its palette and for the reference row of the contact sheet
(their pixels come out the same). Every other medal that is ours and not
Halo 2's is drawn with the same light, palette, quantisation and marks: a
24 by 24 disc shaded top to bottom and lit from the upper left, marks with a
two-colour face, a dark edge and a drop shadow, the tier told by the count of
marks and the disc's colour, a spiked rim for a top tier. Nothing is copied
from a disc font or anyone's artwork: every picture is discs, polygons and
strokes written out below. Halo 2's retail medals are left alone.

    python3 scripts/live-medals.py --out DIR
    python3 scripts/live-medals.py --out DIR --repo CHECKOUT [--h2 BIN --disc ISO]
    python3 scripts/live-medals.py --out DIR --font-art crates/formats/font/src/medals/art.rs
    python3 scripts/live-medals.py --out DIR --hires

The site's files are DIR/site/<id>.webp and @2x, copied over
web/public/art/medals/ (web/public/art/ART.md, "Batch 06" and "Batch 08").
`--font-art` writes the medals the game draws as a glyph (a code point in
`MEDALS`: Incel, U+E2A7) into the font's art file, after the marker line
`FONT_MARK`, and keeps the superbounce medals above it, which
scripts/superbounce-medals.py writes (and which keeps this part).

Writes, under DIR:
    game/<id>.png        the picture at glyph size, 24 by 24, ARGB 4444
    big/<id>.png         the same, 8 times, nearest neighbour
    site/<id>.webp, @2x  the Live art pack's files (64 and 128 pixels), made
                         as web/public/art/ART.md's Batch 07 makes them
    art.rs               every picture as data, the shape of
                         crates/formats/font/src/medals/art.rs
    all.png              the contact sheet: old beside new. `--repo` gives it
                         the art pack's current files; `--h2` and `--disc`
                         the HUD font's words and the game medal the site
                         shows in a pack medal's place
    hires/<id>-512.png,  with `--hires`: the same drawing sampled at 512 with
    hires/<id>-256.png   no quantisation, and halved; source art, checked in
                         as web/art-source/medals/ (docs/live/live-art.md,
                         "Stats banners"); hires.png is their sheet

Needs Pillow. `--only id,id` draws a few; `--jobs N` sets the worker count.
"""

import argparse
import math
import os
import subprocess
import sys
import tempfile
from multiprocessing import Pool

SIZE = 24
SS = 8  # supersampling per axis


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def clamp(v):
    return max(0.0, min(1.0, v))


# ---------------------------------------------------------------------------
# The bounce medals: scripts/superbounce-medals.py, verbatim.
# ---------------------------------------------------------------------------

BOUNCE = [
    (0xE2A0, "SUPERBOUNCE", "Superbounce!"),
    (0xE2A1, "DOUBLE_BOUNCE", "Double bounce!"),
    (0xE2A2, "TRIPLE_BOUNCE", "Triple bounce!"),
    (0xE2A3, "QUAD_BOUNCE", "Quad bounce!"),
    (0xE2A4, "CHAIN_BOUNCE", "Chain bounce!"),
    (0xE2A5, "BOUNCETACULAR", "Bouncetacular!"),
    (0xE2A6, "BOUNCED", "Bounced!"),
]

STYLE = {
    "SUPERBOUNCE": dict(
        top=(0.42, 0.54, 0.72), bottom=(0.72, 0.82, 0.94),
        face=((0.92, 1.0, 1.0), (0.40, 0.84, 0.98)), edge=(0.04, 0.18, 0.36),
        strip=(0.22, 0.30, 0.46), rows=[1]),
    "DOUBLE_BOUNCE": dict(
        top=(1.0, 0.86, 0.42), bottom=(1.0, 1.0, 0.70),
        face=((0.42, 0.58, 1.0), (0.14, 0.22, 0.78)), edge=(0.04, 0.06, 0.36),
        strip=(0.66, 0.46, 0.10), rows=[1, 1]),
    "TRIPLE_BOUNCE": dict(
        top=(0.55, 0.66, 0.72), bottom=(0.80, 0.88, 0.90),
        face=((0.40, 1.0, 0.90), (0.0, 0.62, 0.60)), edge=(0.0, 0.20, 0.24),
        strip=(0.26, 0.34, 0.40), rows=[1, 1, 1]),
    "QUAD_BOUNCE": dict(
        top=(0.80, 0.50, 0.24), bottom=(0.98, 0.76, 0.48),
        face=((0.60, 1.0, 0.45), (0.12, 0.62, 0.16)), edge=(0.03, 0.24, 0.05),
        strip=(0.42, 0.22, 0.08), rows=[2, 2]),
    "CHAIN_BOUNCE": dict(
        top=(0.80, 0.78, 0.92), bottom=(0.97, 0.97, 1.0),
        face=((0.90, 0.58, 1.0), (0.46, 0.14, 0.74)), edge=(0.18, 0.03, 0.32),
        strip=(0.46, 0.42, 0.62), rows=[1, 2, 2]),
    "BOUNCETACULAR": dict(
        top=(1.0, 0.92, 0.66), bottom=(1.0, 1.0, 0.90),
        face=((1.0, 0.62, 0.30), (0.80, 0.10, 0.06)), edge=(0.32, 0.02, 0.0),
        strip=(0.70, 0.52, 0.20), rows=[2, 2, 2],
        rim=((1.0, 0.88, 0.48), (0.72, 0.44, 0.12))),
    "BOUNCED": dict(
        top=(0.50, 0.12, 0.12), bottom=(0.86, 0.36, 0.28),
        face=((1.0, 0.94, 0.45), (0.98, 0.58, 0.08)), edge=(0.30, 0.10, 0.0),
        strip=(0.24, 0.04, 0.04), rows=[], fall=True),
}


def chevron_dist(x, y, cx, tip, half_w, height, thick):
    def seg(ax, ay, bx, by):
        px, py = x - ax, y - ay
        dx, dy = bx - ax, by - ay
        t = clamp((px * dx + py * dy) / (dx * dx + dy * dy))
        ex, ey = px - dx * t, py - dy * t
        return math.hypot(ex, ey)
    base = tip + height
    d = min(seg(cx - half_w, base, cx, tip), seg(cx + half_w, base, cx, tip))
    return d - thick / 2


def strip(disc_r):
    return (SIZE / 2 + disc_r * 0.46, max(1.5, disc_r * 0.15), disc_r * 0.6)


def chevrons(rows, disc_r):
    if not rows:
        return []
    c = SIZE / 2
    k = disc_r / 11.95
    top = c - disc_r * 0.74
    bottom = strip(disc_r)[0] - 1.3
    n, cols = len(rows), max(rows)
    if cols == 1:
        h, th, hw = {1: (5.6, 3.4, 6.4), 2: (4.0, 2.5, 6.0)}.get(n, (2.9, 1.8, 5.6))
    elif n == 2:
        h, th, hw = 3.2, 2.0, 3.3
    else:
        h, th, hw = 2.6, 1.7, 3.0
    h, th, hw = h * k, th * k, hw * k
    span = bottom - top - th / 2 - h - th * 0.8
    gap = span / (n - 1) if n > 1 else 0
    first = top + th / 2 + (span / 2 if n == 1 else 0)
    pitch = 2 * hw + th + 2.0 * k
    out = []
    for i, m in enumerate(rows):
        y = first + i * gap
        for j in range(m):
            out.append((c + (j - (m - 1) / 2) * pitch, y, hw, h, th))
    return out


def bounce_sample(name, x, y):
    p = STYLE[name]
    cx = cy = SIZE / 2
    dx, dy = x - cx, y - cy
    r = math.hypot(dx, dy)
    ang = math.atan2(dy, dx)
    frame = "rim" in p
    disc_r = 9.6 if frame else 11.95
    col = None
    if frame:
        k = 12
        a = (ang + math.pi / 2) % (2 * math.pi / k) / (2 * math.pi / k)
        spike = 12.0 - abs(a - 0.5) * 2 * 3.2
        if disc_r - 0.5 <= r <= spike:
            light = clamp(0.5 - (dx + dy) / (2 * SIZE) * 1.4)
            col = mix(p["rim"][1], p["rim"][0], light)
            if r < disc_r + 0.6:
                col = mix(col, (0.2, 0.12, 0.05), 0.55)
    if r > disc_r:
        return (*col, 1.0) if col else (0.0, 0.0, 0.0, 0.0)
    t = clamp((y - (cy - disc_r)) / (2 * disc_r))
    col = mix(p["top"], p["bottom"], t ** 1.3)
    lit = -(dx + dy) / (math.sqrt(2) * max(r, 1e-6))
    if disc_r - r < 1.3:
        if lit > 0:
            col = mix(col, (1.0, 1.0, 1.0), clamp(lit) * 0.85)
        else:
            col = mix(col, mix(p["top"], (0, 0, 0), 0.35), clamp(-lit) * 0.6)
    shade = mix(p["top"], (0, 0, 0), 0.4)
    if p.get("fall"):
        sy, sh, sw = cy + 2.4, 2.0, disc_r * 0.7
        if abs(dx) <= sw and sy <= y <= sy + sh and abs(dx) >= 4.2 + (y - sy) * 0.8:
            col = p["strip"]
            if y < sy + 0.6:
                col = mix(p["strip"], p["bottom"], 0.6)
        marks = [(cx, cy + 7.6, 5.8, -8.0, 3.0)]
    else:
        sy, sh, sw = strip(disc_r)
        if abs(dx) <= sw and sy <= y <= sy + sh:
            col = p["strip"]
            if y > sy + sh - 0.6:
                col = mix(p["strip"], p["bottom"], 0.7)
        marks = chevrons(p["rows"], disc_r)
    for (mx, tip, hw, h, th) in marks:
        if chevron_dist(x - 0.6, y - 0.8, mx, tip, hw, h, th) < 0:
            col = mix(col, shade, 0.25)
    for (mx, tip, hw, h, th) in marks:
        d = chevron_dist(x, y, mx, tip, hw, h, th)
        if d < 0:
            lo, hi = min(tip, tip + h) - th / 2, max(tip, tip + h) + th / 2
            face = mix(p["face"][0], p["face"][1], clamp((y - lo) / (hi - lo)))
            if d > -min(0.75, th * 0.3):
                face = mix(face, p["edge"], 0.8)
            col = face
    return (*col, 1.0)


# ---------------------------------------------------------------------------
# Shapes: signed distances (< 0 inside), in glyph pixels.
# ---------------------------------------------------------------------------

def circle(cx, cy, r):
    return lambda x, y: math.hypot(x - cx, y - cy) - r


def ring(cx, cy, r, th):
    return lambda x, y: abs(math.hypot(x - cx, y - cy) - r) - th / 2


def oval_ring(cx, cy, rx, ry, th):
    m = min(rx, ry)
    return lambda x, y: abs(math.hypot((x - cx) / rx, (y - cy) / ry) - 1) * m - th / 2


def seg(ax, ay, bx, by, th):
    dx, dy = bx - ax, by - ay
    ll = dx * dx + dy * dy

    def f(x, y):
        px, py = x - ax, y - ay
        t = clamp((px * dx + py * dy) / ll)
        return math.hypot(px - dx * t, py - dy * t) - th / 2
    return f


def box(cx, cy, hw, hh, rnd=0.0):
    def f(x, y):
        qx, qy = abs(x - cx) - hw + rnd, abs(y - cy) - hh + rnd
        return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - rnd
    return f


def poly(pts):
    n = len(pts)

    def f(x, y):
        d = 1e18
        s = 1.0
        j = n - 1
        for i in range(n):
            xi, yi = pts[i]
            xj, yj = pts[j]
            ex, ey = xj - xi, yj - yi
            wx, wy = x - xi, y - yi
            t = clamp((wx * ex + wy * ey) / (ex * ex + ey * ey))
            bx, by = wx - ex * t, wy - ey * t
            d = min(d, bx * bx + by * by)
            c1, c2, c3 = y >= yi, y < yj, ex * wy > ey * wx
            if (c1 and c2 and c3) or (not c1 and not c2 and not c3):
                s = -s
            j = i
        return s * math.sqrt(d)
    return f








def union(*fs):
    return lambda x, y: min(f(x, y) for f in fs)


def inter(*fs):
    return lambda x, y: max(f(x, y) for f in fs)




def above(yy):
    """Everything over the line y = yy."""
    return lambda x, y: y - yy


# ---------------------------------------------------------------------------
# Palette: the bounce medals' bodies (STYLE), and their kin.
# ---------------------------------------------------------------------------

def body_of(name):
    return STYLE[name]["top"], STYLE[name]["bottom"]


GOLD = body_of("DOUBLE_BOUNCE")
BRONZE = body_of("QUAD_BOUNCE")
LAVENDER = body_of("CHAIN_BOUNCE")
PALE_GOLD = body_of("BOUNCETACULAR")
MAROON = body_of("BOUNCED")
GOLD_RIM = STYLE["BOUNCETACULAR"]["rim"]
RED = ((0.72, 0.14, 0.12), (0.98, 0.52, 0.42))
CHARCOAL = ((0.24, 0.26, 0.34), (0.52, 0.56, 0.66))
CREAM = ((0.90, 0.84, 0.70), (1.0, 0.98, 0.90))
SKY = ((0.34, 0.56, 0.86), (0.74, 0.90, 1.0))
STEEL_RIM = ((0.86, 0.92, 0.96), (0.36, 0.42, 0.50))

# Marks: (face top, face bottom), edge.
M_BLUE = (STYLE["DOUBLE_BOUNCE"]["face"], STYLE["DOUBLE_BOUNCE"]["edge"])
M_GREEN = (STYLE["QUAD_BOUNCE"]["face"], STYLE["QUAD_BOUNCE"]["edge"])
M_PURPLE = (STYLE["CHAIN_BOUNCE"]["face"], STYLE["CHAIN_BOUNCE"]["edge"])
M_RED = (STYLE["BOUNCETACULAR"]["face"], STYLE["BOUNCETACULAR"]["edge"])
M_YELLOW = (STYLE["BOUNCED"]["face"], STYLE["BOUNCED"]["edge"])
M_ICE = (STYLE["SUPERBOUNCE"]["face"], STYLE["SUPERBOUNCE"]["edge"])
M_WHITE = (((1.0, 1.0, 1.0), (0.78, 0.84, 0.92)), (0.10, 0.14, 0.26))
M_DARK = (((0.30, 0.30, 0.36), (0.08, 0.08, 0.12)), (0.0, 0.0, 0.02))
M_SIGN = (((1.0, 0.36, 0.30), (0.78, 0.06, 0.06)), (0.30, 0.0, 0.0))
M_OLIVE = (((0.62, 0.76, 0.40), (0.26, 0.42, 0.16)), (0.06, 0.14, 0.02))
M_TEA = (((0.92, 0.72, 0.42), (0.58, 0.34, 0.12)), (0.24, 0.10, 0.0))
M_VISOR = (((1.0, 0.86, 0.36), (0.92, 0.50, 0.06)), (0.28, 0.10, 0.0))
M_HALO = (((1.0, 1.0, 0.72), (1.0, 0.80, 0.20)), (0.44, 0.24, 0.0))


def mark(shape, style, ew=0.6, shadow=True):
    """A mark: its shape, its (face top and bottom, edge), the edge's width
    and whether it casts a shadow on the disc."""
    return dict(shape=shape, face=style[0], edge=style[1], ew=ew, shadow=shadow)


# ---------------------------------------------------------------------------
# The medals. Each: a disc (its colours, a rim for a top tier) and its marks.
# ---------------------------------------------------------------------------

def disc_body(cols):
    return dict(shape=circle(SIZE / 2, SIZE / 2, 11.95), cols=cols, band=1.3)


def rim_body(cols, rim, k=12):
    """A disc in a spiked rim, as Bouncetacular's."""
    return dict(shape=circle(SIZE / 2, SIZE / 2, 9.6), cols=cols, band=1.3, rim=rim, k=k)


def gem(cx, cy, hw, hh):
    return poly([(cx, cy - hh), (cx + hw, cy), (cx, cy + hh), (cx - hw, cy)])


def gems(points, hw, hh, style):
    """The chain medals' marks: one gem for each knockout of the chain."""
    return [mark(gem(x, y, hw, hh), style, ew=0.55) for (x, y) in points]


def flame_pts(cx, cy, s):
    pts = [(0.4, -8.4), (3.0, -4.4), (5.0, -0.4), (4.6, 4.4), (2.4, 7.4), (0, 8.0),
           (-2.6, 7.4), (-4.8, 4.4), (-4.8, 0.6), (-3.2, -2.6), (-1.8, -0.4), (-1.2, -4.6)]
    return [(cx + x * s, cy + y * s) for x, y in pts]


def flame(cx, cy, s, outer, inner):
    """A flame and its heart."""
    heart = [(0.2, 0.2), (2.0, 3.2), (1.4, 6.0), (0, 6.6), (-1.6, 6.0), (-2.2, 3.6)]
    return [
        mark(poly(flame_pts(cx, cy, s)), outer, ew=0.55),
        mark(poly([(cx + x * s, cy + y * s) for x, y in heart]), inner, ew=0.0, shadow=False),
    ]


def star(cx, cy, ro, ri):
    pts = []
    for i in range(10):
        a = -math.pi / 2 + i * math.pi / 5
        r = ro if i % 2 == 0 else ri
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return poly(pts)


def curtains():
    """A stage: its curtains drawn back under their rail, and the star of
    the show."""
    left = poly([(3.4, 6.0), (9.0, 6.0), (7.0, 12.0), (7.8, 19.6), (3.4, 17.6)])
    right = poly([(20.6, 6.0), (15.0, 6.0), (17.0, 12.0), (16.2, 19.6), (20.6, 17.6)])
    return [
        mark(star(12, 13.6, 4.6, 2.3), M_YELLOW, ew=0.45),
        mark(left, M_SIGN, ew=0.55),
        mark(right, M_SIGN, ew=0.55),
        mark(box(12, 5.4, 8.8, 1.3, 0.5), M_YELLOW, ew=0.45),
    ]


def last_word():
    """A speech bubble: a line of words and its full stop."""
    bubble = union(box(12, 10.4, 7.4, 5.4, 2.6), poly([(8.4, 14.6), (13.0, 14.6), (7.4, 20.0)]))
    return [
        mark(bubble, M_WHITE, ew=0.6),
        mark(seg(7.8, 10.6, 12.2, 10.6, 1.5), M_DARK, ew=0.0, shadow=False),
        mark(circle(15.8, 10.7, 1.6), M_SIGN, ew=0.0, shadow=False),
    ]


def payback():
    """An arrow that goes out and comes back."""
    th = 2.1
    s = union(seg(9.0, 7.6, 14.6, 7.6, th),
              inter(ring(14.6, 11.6, 4.0, th), lambda x, y: 14.6 - x),
              seg(14.6, 15.6, 10.6, 15.6, th),
              poly([(10.8, 11.8), (10.8, 19.4), (5.0, 15.6)]))
    return [mark(s, M_SIGN, ew=0.5)]


def orb():
    """Untouchable: a shield's bubble, nothing through it."""
    c = 12
    return [
        mark(circle(c, c, 6.6), M_ICE, ew=0.7),
        mark(ring(c, c, 4.0, 1.1), M_WHITE, ew=0.0, shadow=False),
        mark(circle(c - 2.0, c - 2.2, 1.2), M_WHITE, ew=0.0, shadow=False),
    ]


def teabag(cx, cy, s=1.0):
    """A teabag: the bag, its fold, and its string out to the tag."""
    bag = poly([(cx + x * s, cy + y * s) for x, y in
                [(-2.6, -4.4), (2.6, -4.4), (4.8, -2.0), (4.8, 5.8), (-4.8, 5.8), (-4.8, -2.0)]])
    tx, ty = cx + 6.0 * s, cy - 7.6 * s
    return [
        mark(seg(cx, cy - 4.4 * s, tx, ty, 0.9), M_DARK, ew=0.0, shadow=False),
        mark(box(tx, ty, 2.1, 1.7, 0.3), M_WHITE, ew=0.4),
        mark(bag, M_TEA, ew=0.6),
        mark(seg(cx - 2.6 * s, cy - 1.6 * s, cx + 2.6 * s, cy - 1.6 * s, 0.8), ((M_TEA[1], M_TEA[1]), M_TEA[1]), ew=0.0,
             shadow=False),
    ]


def no_sign():
    c = 12
    k = 9.6 / math.sqrt(2)
    return [mark(union(ring(c, c, 10.0, 1.8), seg(c - k, c - k, c + k, c + k, 1.7)), M_SIGN, ew=0.5)]


def incel():
    """No teabags: a teabag (bag, string and tag) under a no sign."""
    return dict(body=disc_body(CREAM), marks=teabag(10.8, 13.2, 1.1) + no_sign())


NAVY = ((0.16, 0.24, 0.52), (0.42, 0.56, 0.86))
TEAL_BODY = ((0.30, 0.62, 0.66), (0.66, 0.90, 0.90))


def build(mid):
    c = 12
    if mid == "opening-act":
        return dict(body=disc_body(NAVY), marks=curtains())
    if mid == "last-word":
        return dict(body=disc_body(CHARCOAL), marks=last_word())
    if mid == "payback":
        return dict(body=disc_body(BRONZE), marks=payback())
    # The chain: a gem for each knockout, as the bounce chain counts chevrons.
    if mid == "twofer":
        return dict(body=disc_body(TEAL_BODY), marks=gems([(8.2, c), (15.8, c)], 3.2, 5.4, M_WHITE))
    if mid == "hat-trick":
        return dict(body=disc_body(LAVENDER), marks=gems([(c, 7.6), (7.6, 15.4), (16.4, 15.4)], 2.9, 4.2, M_PURPLE))
    if mid == "full-house":
        return dict(body=disc_body(GOLD),
                    marks=gems([(8.0, 7.8), (16.0, 7.8), (8.0, 16.2), (16.0, 16.2)], 2.8, 3.7, M_BLUE))
    if mid == "encore":
        pts = [(8.0, 8.2), (16.0, 8.2), (c, c), (8.0, 15.8), (16.0, 15.8)]
        return dict(body=rim_body(SKY, STEEL_RIM), marks=gems(pts, 2.3, 3.1, M_RED))
    # The sprees: a flame, three, then the bubble nothing got through.
    if mid == "hot-streak":
        return dict(body=disc_body(RED), marks=flame(c, 12.2, 1.0, M_YELLOW, M_WHITE))
    if mid == "wildfire":
        marks = flame(6.6, 13.6, 0.62, M_RED, M_YELLOW) + flame(17.4, 13.6, 0.62, M_RED, M_YELLOW) \
            + flame(c, 11.6, 0.86, M_RED, M_YELLOW)
        return dict(body=disc_body(GOLD), marks=marks)
    if mid == "untouchable":
        return dict(body=rim_body(PALE_GOLD, GOLD_RIM), marks=orb())
    if mid == "incel":
        return incel()
    raise KeyError(mid)


# id (the site's medal id, the file's name), name, the rule, a HUD line,
# the game medal the site shows in its place when the server has game art
# (web/src/lib/gameart.ts MEDAL_ALIASES) as (name, code point), the glyph's
# code point if the game is to draw it.
MEDALS = [
    ("opening-act", "Opening Act", "first knockout of the game", "Opening act!", None, None),
    ("last-word", "Last Word", "final knockout of the game", "Last word!", None, None),
    ("payback", "Payback", "knocked out whoever just got you, within 10 s", "Payback!", None, None),
    ("twofer", "Twofer", "2 knockouts, each within 4 s of the last", "Twofer!", ("double_kill", 0xE038), None),
    ("hat-trick", "Hat Trick", "3 knockouts in a chain", "Hat trick!", ("triple_kill", 0xE039), None),
    ("full-house", "Full House", "4 knockouts in a chain", "Full house!", ("killtacular", 0xE040), None),
    ("encore", "Encore", "5 or more knockouts in a chain", "Encore!", ("kill_frenzy", 0xE041), None),
    ("hot-streak", "Hot Streak", "5 knockouts without being knocked out", "Hot streak!", ("killing_spree", 0xE033), None),
    ("wildfire", "Wildfire", "10 knockouts without being knocked out", "Wildfire!", ("running_riot", 0xE034), None),
    ("untouchable", "Untouchable", "15 knockouts without being knocked out", "Untouchable!", ("rampage", 0xE035), None),
    ("incel", "Incel", "a whole game, no teabag given or taken", "Incel!", None, 0xE2A7),
]
BOUNCE_IDS = {n.lower().replace("_", "-"): n for (_, n, _) in BOUNCE}


# ---------------------------------------------------------------------------
# Drawing.
# ---------------------------------------------------------------------------

def bounds(shape):
    """Top and bottom of a shape, by a scan."""
    lo, hi = None, None
    for iy in range(SIZE * 4 + 1):
        y = iy / 4
        if any(shape(ix / 4, y) < 0 for ix in range(SIZE * 4 + 1)):
            lo = y if lo is None else lo
            hi = y
    if lo is None:
        return 0.0, float(SIZE)
    return lo - 0.125, hi + 0.125


def prepare(m):
    b = m["body"]
    b["lo"], b["hi"] = bounds(b["shape"])
    for k in m["marks"]:
        k["lo"], k["hi"] = bounds(k["shape"])
    return m


def rim_colour(b, x, y):
    """The spiked rim round a rimmed disc at (x, y), or None: Bouncetacular's."""
    dx, dy = x - SIZE / 2, y - SIZE / 2
    r = math.hypot(dx, dy)
    k = b["k"]
    a = (math.atan2(dy, dx) + math.pi / 2) % (2 * math.pi / k) / (2 * math.pi / k)
    spike = 12.0 - abs(a - 0.5) * 2 * 3.2
    if not (9.6 - 0.5 <= r <= spike):
        return None
    light = clamp(0.5 - (dx + dy) / (2 * SIZE) * 1.4)
    col = mix(b["rim"][1], b["rim"][0], light)
    if r < 9.6 + 0.6:
        col = mix(col, mix(b["rim"][1], (0, 0, 0), 0.7), 0.55)
    return col


def sample(m, x, y):
    """Straight RGBA (0..1) of a medal at (x, y), in glyph pixels: the disc
    and its light as `bounce_sample` draws them, then the marks' shadows,
    then the marks."""
    b = m["body"]
    top, bottom = b["cols"]
    d = b["shape"](x, y)
    if "rim" in b and d > -0.5:
        col = rim_colour(b, x, y)
        if col:
            return (*col, 1.0)
    if d > 0:
        return (0.0, 0.0, 0.0, 0.0)
    t = clamp((y - b["lo"]) / (b["hi"] - b["lo"]))
    col = mix(top, bottom, t ** 1.3)
    if -d < b["band"]:
        e = 0.05
        gx = b["shape"](x + e, y) - b["shape"](x - e, y)
        gy = b["shape"](x, y + e) - b["shape"](x, y - e)
        g = math.hypot(gx, gy) or 1e-6
        lit = -(gx + gy) / (math.sqrt(2) * g)
        if lit > 0:
            col = mix(col, (1.0, 1.0, 1.0), clamp(lit) * 0.85)
        else:
            col = mix(col, mix(top, (0, 0, 0), 0.35), clamp(-lit) * 0.6)
    shade = mix(top, (0, 0, 0), 0.4)
    for k in m["marks"]:
        if k["shadow"] and k["shape"](x - 0.6, y - 0.8) < 0:
            col = mix(col, shade, 0.25)
    for k in m["marks"]:
        dk = k["shape"](x, y)
        if dk < 0:
            face = mix(k["face"][0], k["face"][1], clamp((y - k["lo"]) / (k["hi"] - k["lo"])))
            if dk > -k["ew"]:
                face = mix(face, k["edge"], 0.8)
            col = face
    return (*col, 1.0)


def draw(mid):
    """The picture as 24*24 straight RGBA floats, supersampled."""
    if mid in BOUNCE_IDS:
        fn = lambda x, y, n=BOUNCE_IDS[mid]: bounce_sample(n, x, y)
    else:
        m = prepare(build(mid))
        fn = lambda x, y: sample(m, x, y)
    out = []
    for py in range(SIZE):
        for px in range(SIZE):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    r, g, b, a = fn(px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            a = acc[3] / (SS * SS)
            out.append((0.0, 0.0, 0.0, 0.0) if a <= 0 else (acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], a))
    return out


HIRES = 512
HIRES_SS = 3


def draw_hires(mid):
    """The same drawing sampled at 512 by 512, 3 by 3 samples a pixel,
    8 bits a channel: straight RGBA bytes. Not the 24-pixel picture
    enlarged: every disc, mark and rim is drawn again at this size.
    Source art (banners and the like), not a file any page shows."""
    if mid in BOUNCE_IDS:
        fn = lambda x, y, n=BOUNCE_IDS[mid]: bounce_sample(n, x, y)
    else:
        m = prepare(build(mid))
        fn = lambda x, y: sample(m, x, y)
    k = SIZE / HIRES
    out = bytearray()
    n2 = HIRES_SS * HIRES_SS
    for py in range(HIRES):
        for px in range(HIRES):
            ar = ag = ab = aa = 0.0
            for sy in range(HIRES_SS):
                yy = (py + (sy + 0.5) / HIRES_SS) * k
                for sx in range(HIRES_SS):
                    r, g, b, a = fn((px + (sx + 0.5) / HIRES_SS) * k, yy)
                    if a:
                        ar += r
                        ag += g
                        ab += b
                        aa += 1.0
            if aa:
                out += bytes((int(round(clamp(ar / aa) * 255)), int(round(clamp(ag / aa) * 255)),
                              int(round(clamp(ab / aa) * 255)), int(round(aa / n2 * 255))))
            else:
                out += bytes(4)
    return mid, bytes(out)


def hires_images(raw):
    """The 512 picture, and it halved (a box filter over premultiplied
    colour) to 256."""
    from PIL import Image
    big = Image.frombytes("RGBA", (HIRES, HIRES), raw)
    return big, big.convert("RGBa").resize((256, 256), Image.BOX).convert("RGBA")


def argb4444(px):
    r, g, b, a = px
    q = lambda v: int(round(clamp(v) * 15))
    a4 = q(a)
    if a4 == 0:
        return 0
    return (a4 << 12) | (q(r) << 8) | (q(g) << 4) | q(b)


def job(mid):
    return mid, [argb4444(p) for p in draw(mid)]


# ---------------------------------------------------------------------------
# Output.
# ---------------------------------------------------------------------------

def to_image(words):
    from PIL import Image
    im = Image.new("RGBA", (SIZE, SIZE))
    im.putdata([(((v >> 8) & 15) * 17, ((v >> 4) & 15) * 17, (v & 15) * 17, ((v >> 12) & 15) * 17) for v in words])
    return im


RS_HEAD = """\
//! Live's own medals and Incel, drawn in the superbounce medals' manner
//! (crates/formats/font/src/medals/art.rs): 24 by 24, ARGB 4444, row-major,
//! top row first. Written by `scripts/live-medals.py`; new art is a new file of the
//! same shape. Do not edit the rows by hand unless the art is meant to change.

/// Width and height of each picture.
pub const SIZE: u16 = 24;

"""


def write_rs(path, done):
    with open(path, "w") as f:
        f.write(RS_HEAD)
        first = True
        for (mid, name, rule, line, _, unit) in MEDALS:
            if mid not in done:
                continue
            if not first:
                f.write("\n")
            first = False
            at = f"U+{unit:04X}, " if unit else ""
            f.write(f"/// {at}`{line}` {name}: {rule}.\n")
            f.write("#[rustfmt::skip]\n")
            f.write(f"pub const {mid.upper().replace('-', '_')}: [u16; 576] = [\n")
            for y in range(SIZE):
                f.write("    " + ", ".join(f"0x{v:04x}" for v in done[mid][y * SIZE:(y + 1) * SIZE]) + ",\n")
            f.write("];\n")


# The line in crates/formats/font/src/medals/art.rs after which this script
# writes the glyph medals; scripts/superbounce-medals.py keeps what follows it.
FONT_MARK = "// ---- Live's medals with a glyph: written by scripts/live-medals.py --font-art ----\n"


def write_font_art(path, done):
    """The medals with a code point, after FONT_MARK in the font's art file;
    everything above the marker (the superbounce medals) is kept."""
    with open(path) as f:
        text = f.read()
    head = text.split(FONT_MARK, 1)[0].rstrip("\n") + "\n\n"
    with open(path, "w") as f:
        f.write(head)
        f.write(FONT_MARK)
        for (mid, name, rule, line, _, unit) in MEDALS:
            if not unit or mid not in done:
                continue
            f.write("\n")
            f.write(f"/// U+{unit:04X}, `{line}` {name}: {rule}.\n")
            f.write("#[rustfmt::skip]\n")
            f.write(f"pub const {mid.upper().replace('-', '_')}: [u16; 576] = [\n")
            for y in range(SIZE):
                f.write("    " + ", ".join(f"0x{v:04x}" for v in done[mid][y * SIZE:(y + 1) * SIZE]) + ",\n")
            f.write("];\n")


def site_images(im):
    """The art pack's two pictures, made as ART.md's Batch 07 makes them:
    the glyph 5 times (120) centred on 128, and that box-filtered to 60 on 64."""
    from PIL import Image
    up = im.resize((SIZE * 5, SIZE * 5), Image.NEAREST)
    two = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    two.alpha_composite(up, (4, 4))
    one = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    one.alpha_composite(up.resize((60, 60), Image.BOX), (2, 2))
    return one, two


def h2_text(h2, disc, text, cache):
    from PIL import Image
    if text not in cache:
        tmp = os.path.join(cache["dir"], f"{len(cache)}.png")
        r = subprocess.run([h2, "font", disc, "conduit-13", text, "--glyph-colors", "--color", "FFFFFF",
                            "--out", tmp, "--force"], capture_output=True)
        cache[text] = Image.open(tmp).convert("RGBA") if r.returncode == 0 else None
    return cache[text]


def sheet(out_dir, done, h2, disc, repo):
    """all.png: every medal, old beside new, at the glyph's size, 4 times,
    and at the site's sizes; its line in the HUD font; the approved bounce
    set underneath for comparison."""
    from PIL import Image, ImageDraw, ImageFont
    bg, ink, dim, rule_c = (28, 30, 38, 255), (235, 235, 240, 255), (150, 155, 170, 255), (48, 52, 64, 255)
    cache = {"dir": tempfile.mkdtemp()}
    fonts = bool(h2 and disc)
    try:
        font, head = ImageFont.load_default(size=11), ImageFont.load_default(size=14)
    except TypeError:
        font = head = ImageFont.load_default()
    Z = 4
    row_h = SIZE * Z + 14
    rows = [r for r in MEDALS if r[0] in done]
    X_OLD, X_NEW, X_SITE, X_LINE, X_ALIAS = 250, 440, 620, 860, 1100
    W = 1300
    H = 44 + len(rows) * row_h + 150
    out = Image.new("RGBA", (W, H), bg)
    dr = ImageDraw.Draw(out)
    for x, t in ((16, "medal"), (X_OLD, "OLD  24, 96"), (X_NEW, "NEW  24 (game), 96"),
                 (X_SITE, "NEW on site: 21, 26, 30, 64"), (X_LINE, "NEW as a HUD line, 1x and 2x"), (X_ALIAS, "site shows instead")):
        dr.text((x, 8), t, fill=dim, font=head)

    def put(im, x, y, z=1):
        if im is None:
            return
        if z != 1:
            im = im.resize((im.width * z, im.height * z), Image.NEAREST)
        out.alpha_composite(im, (x, y))

    for i, (mid, name, rule, line, alias, unit) in enumerate(rows):
        y = 40 + i * row_h
        new = to_image(done[mid])
        dr.text((16, y + 6), name, fill=ink, font=font)
        dr.text((16, y + 22), rule, fill=dim, font=font)
        where = f"site: medals/{mid}.webp"
        if unit:
            where += f"   game: U+{unit:04X}"
        dr.text((16, y + 38), where, fill=dim, font=font)
        old = os.path.join(repo, "web/public/art/medals", f"{mid}@2x.webp") if repo else None
        if old and os.path.exists(old):
            big = Image.open(old).convert("RGBA")
            put(big.resize((SIZE, SIZE), Image.LANCZOS), X_OLD, y + 4)
            put(big.resize((SIZE * Z, SIZE * Z), Image.LANCZOS), X_OLD + 40, y + 4)
        else:
            dr.text((X_OLD, y + 30), "(new medal)" if mid.startswith("incel") else "(pass --repo)", fill=dim, font=font)
        put(new, X_NEW, y + 4)
        put(new, X_NEW + 40, y + 4, Z)
        # On the site: the 64 and 128 files at the sizes the pages ask for
        # (Carnage.tsx: 21, 26, 30; MedalIcon's default 28), on its dark tile.
        one, two = site_images(new)
        sx = X_SITE
        for size in (21, 26, 30, 64):
            tile = Image.new("RGBA", (size + 8, size + 8), (17, 17, 17, 255))
            tile.alpha_composite((two if size > 32 else one).resize((size, size), Image.LANCZOS), (4, 4))
            put(tile, sx, y + 4)
            sx += size + 12 if size < 64 else 0
        if fonts:
            words = h2_text(h2, disc, " " + line, cache)
            if words is not None:
                ln = Image.new("RGBA", (SIZE + words.width, max(SIZE, words.height)), (0, 0, 0, 0))
                ln.alpha_composite(new, (0, 0))
                ln.alpha_composite(words, (SIZE, 0))
                put(ln, X_LINE, y + 4)
                put(ln, X_LINE, y + 4 + 34, 2)
            if alias:
                g = h2_text(h2, disc, chr(alias[1]), cache)
                put(g, X_ALIAS, y + 4)
                put(g, X_ALIAS + 40, y + 4, 3)
                dr.text((X_ALIAS, y + 82), f"{alias[0]} (retail, unchanged)", fill=dim, font=font)
        dr.line((16, y + row_h - 5, W - 16, y + row_h - 5), fill=rule_c)
    # The approved set, for comparison.
    y = 40 + len(rows) * row_h + 8
    dr.text((16, y), "The approved superbounce set (unchanged), 24 and 96:", fill=dim, font=head)
    x = 16
    for mid in BOUNCE_IDS:
        im = to_image(job(mid)[1])
        put(im, x, y + 26)
        put(im, x + 32, y + 26, Z)
        x += 32 + SIZE * Z + 28
    path = os.path.join(out_dir, "all.png")
    out.save(path)
    return path


def hires_sheet(out_dir, ids, hi):
    """hires.png: the hi-res set on its own, each at 256."""
    from PIL import Image, ImageDraw, ImageFont
    bg, ink = (28, 30, 38, 255), (235, 235, 240, 255)
    try:
        font = ImageFont.load_default(size=14)
    except TypeError:
        font = ImageFont.load_default()
    cols, cell = 6, 256 + 24
    rows = (len(ids) + cols - 1) // cols
    out = Image.new("RGBA", (cols * cell + 24, rows * (cell + 22) + 24), bg)
    dr = ImageDraw.Draw(out)
    names = {r[0]: r[1] for r in MEDALS}
    for i, mid in enumerate(ids):
        x, y = 24 + (i % cols) * cell, 16 + (i // cols) * (cell + 22)
        dr.text((x, y), names.get(mid, mid.replace("-", " ").capitalize()), fill=ink, font=font)
        out.alpha_composite(hires_images(hi[mid])[1], (x, y + 24))
    path = os.path.join(out_dir, "hires.png")
    out.save(path)
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--h2")
    ap.add_argument("--disc", default=os.environ.get("H2_DISC"))
    ap.add_argument("--repo", help="a checkout, for the art pack's current files in all.png")
    ap.add_argument("--hires", action="store_true",
                    help="also draw hires/ (512 and 256 px PNGs, source art) and hires.png; a few minutes")
    ap.add_argument("--font-art", help="the font's art.rs: write the glyph medals (Incel) into it")
    ap.add_argument("--only")
    ap.add_argument("--jobs", type=int, default=os.cpu_count())
    a = ap.parse_args()
    ids = [r[0] for r in MEDALS]
    if a.only:
        ids = [i for i in ids if i in a.only.split(",")]
    with Pool(a.jobs) as pool:
        done = dict(pool.map(job, ids))
    for sub in ("game", "big", "site"):
        os.makedirs(os.path.join(a.out, sub), exist_ok=True)
    from PIL import Image
    for mid in ids:
        im = to_image(done[mid])
        im.save(os.path.join(a.out, "game", f"{mid}.png"))
        im.resize((SIZE * 8, SIZE * 8), Image.NEAREST).save(os.path.join(a.out, "big", f"{mid}.png"))
        one, two = site_images(im)
        one.save(os.path.join(a.out, "site", f"{mid}.webp"), lossless=True)
        two.save(os.path.join(a.out, "site", f"{mid}@2x.webp"), lossless=True)
    write_rs(os.path.join(a.out, "art.rs"), done)
    if a.font_art:
        write_font_art(a.font_art, done)
    print(sheet(a.out, done, a.h2, a.disc, a.repo))
    if a.hires:
        hi_ids = ids + ([] if a.only else list(BOUNCE_IDS))
        with Pool(a.jobs) as pool:
            hi = dict(pool.map(draw_hires, hi_ids, chunksize=1))
        os.makedirs(os.path.join(a.out, "hires"), exist_ok=True)
        for mid in hi_ids:
            big, half = hires_images(hi[mid])
            big.save(os.path.join(a.out, "hires", f"{mid}-512.png"), optimize=True)
            half.save(os.path.join(a.out, "hires", f"{mid}-256.png"), optimize=True)
        print(hires_sheet(a.out, hi_ids, hi))


if __name__ == "__main__":
    sys.exit(main())
