#!/usr/bin/env python3
"""Draw the superbounce medal glyphs (docs/game/hud-kit.md, "The medals").

Writes the art as data, `crates/formats/font/src/medals/art.rs`:
24 x 24 ARGB 4444 pictures that `font::medals` adds to the HUD message fonts.
The pictures are drawn here from discs, polygons and gradients in the manner
of Halo 2's multi-kill medals (a shaded disc lit from the upper left, the tier
told by the count of marks and the disc's colour, a frame for the top tier);
nothing is copied from a disc font or anyone's artwork. New art is a new
`art.rs`: edit the drawing below, or replace the file by hand.

    python3 scripts/superbounce-medals.py            # write art.rs
    python3 scripts/superbounce-medals.py --preview DIR [--h2 BIN] [--disc ISO]
    python3 scripts/superbounce-medals.py --rs FILE --art DIR [--only id,id]

`--preview` also writes a sheet (each glyph at its size and 8 times, and a
mock notification line beside a retail one, drawn by `h2 font`) to DIR. It
needs Pillow; writing art.rs needs nothing outside the standard library.

`--art` writes each medal's files to DIR as `scripts/live-medals.py` does
(Pillow again): game/<id>.png (24 by 24, ARGB 4444), big/<id>.png (8 times),
site/<id>.webp and @2x (64 and 128), hires/<id>-512.png and -256.png (the
drawing sampled again at 512, and halved: web/art-source/medals/), and
sheet.png, every bounce medal side by side. `--only` keeps a few ids.

The Monster Bounce has several drawings (`MONSTER_VARIANTS`); STYLE names
the one in use and `--monster NAME` draws another. `--variants DIR` writes
sheet-v2.png, all of them beside the other medals (`--before PNG` adds the
medal as it was).
"""

import argparse
import math
import os
import subprocess
import sys
import tempfile

SIZE = 24
SS = 8  # supersampling per axis

# Code points, names and lines, in the order of `font::medals::MEDALS`.
MEDALS = [
    (0xE2A0, "SUPERBOUNCE", "Superbounce!"),
    (0xE2A1, "DOUBLE_BOUNCE", "Double bounce!"),
    (0xE2A2, "TRIPLE_BOUNCE", "Triple bounce!"),
    (0xE2A3, "QUAD_BOUNCE", "Quad bounce!"),
    (0xE2A4, "CHAIN_BOUNCE", "Chain bounce!"),
    (0xE2A5, "BOUNCETACULAR", "Bouncetacular!"),
    (0xE2A6, "BOUNCED", "Bounced!"),
    (0xE2A8, "MONSTER_BOUNCE", "Monster bounce!"),
]

# Each medal: the disc from its top to its bottom (the retail discs are
# lighter low down), the chevrons' face (top to bottom) and edge, the
# strip's colour, the chevrons as rows top first (a count of chevrons in
# each row: the tier, as a multi-kill disc counts its stars), and for the
# top tier a spiked rim. `BOUNCED` is a fall through a broken strip.
# `MONSTER_BOUNCE` is a monster: `monster` names its drawing, one of
# `MONSTER_VARIANTS` below.
STYLE = {
    # Slate blue, one pale chevron.
    "SUPERBOUNCE": dict(
        top=(0.42, 0.54, 0.72), bottom=(0.72, 0.82, 0.94),
        face=((0.92, 1.0, 1.0), (0.40, 0.84, 0.98)), edge=(0.04, 0.18, 0.36),
        strip=(0.22, 0.30, 0.46), rows=[1]),
    # Gold, blue chevrons (beside the Double Kill's gold and blue).
    "DOUBLE_BOUNCE": dict(
        top=(1.0, 0.86, 0.42), bottom=(1.0, 1.0, 0.70),
        face=((0.42, 0.58, 1.0), (0.14, 0.22, 0.78)), edge=(0.04, 0.06, 0.36),
        strip=(0.66, 0.46, 0.10), rows=[1, 1]),
    # Steel, teal chevrons.
    "TRIPLE_BOUNCE": dict(
        top=(0.55, 0.66, 0.72), bottom=(0.80, 0.88, 0.90),
        face=((0.40, 1.0, 0.90), (0.0, 0.62, 0.60)), edge=(0.0, 0.20, 0.24),
        strip=(0.26, 0.34, 0.40), rows=[1, 1, 1]),
    # Bronze, green chevrons.
    "QUAD_BOUNCE": dict(
        top=(0.80, 0.50, 0.24), bottom=(0.98, 0.76, 0.48),
        face=((0.60, 1.0, 0.45), (0.12, 0.62, 0.16)), edge=(0.03, 0.24, 0.05),
        strip=(0.42, 0.22, 0.08), rows=[2, 2]),
    # Lavender, purple chevrons.
    "CHAIN_BOUNCE": dict(
        top=(0.80, 0.78, 0.92), bottom=(0.97, 0.97, 1.0),
        face=((0.90, 0.58, 1.0), (0.46, 0.14, 0.74)), edge=(0.18, 0.03, 0.32),
        strip=(0.46, 0.42, 0.62), rows=[1, 2, 2]),
    # Pale gold in a spiked gold rim, red chevrons.
    "BOUNCETACULAR": dict(
        top=(1.0, 0.92, 0.66), bottom=(1.0, 1.0, 0.90),
        face=((1.0, 0.62, 0.30), (0.80, 0.10, 0.06)), edge=(0.32, 0.02, 0.0),
        strip=(0.70, 0.52, 0.20), rows=[2, 2, 2],
        rim=((1.0, 0.88, 0.48), (0.72, 0.44, 0.12))),
    # Maroon, a yellow chevron falling through a broken strip.
    "BOUNCED": dict(
        top=(0.50, 0.12, 0.12), bottom=(0.86, 0.36, 0.28),
        face=((1.0, 0.94, 0.45), (0.98, 0.58, 0.08)), edge=(0.30, 0.10, 0.0),
        strip=(0.24, 0.04, 0.04), rows=[], fall=True),
    # Hot orange, crimson horns and rim, pale marks (the Superbounce's
    # chevron and its colours); the dark of a mouth or a seam for a strip.
    "MONSTER_BOUNCE": dict(
        top=(1.0, 0.34, 0.10), bottom=(1.0, 0.80, 0.26),
        face=((0.92, 1.0, 1.0), (0.40, 0.84, 0.98)), edge=(0.04, 0.18, 0.36),
        strip=(0.40, 0.05, 0.02), rows=[], monster="horned",
        rim=((1.0, 0.46, 0.20), (0.52, 0.05, 0.04))),
}


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def clamp(v):
    return max(0.0, min(1.0, v))


def chevron_dist(x, y, cx, tip, half_w, height, thick):
    """Signed distance (<0 inside) to a chevron: two strokes from the
    corners at `tip + height` to the tip at (cx, tip). A negative height
    points it down."""
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
    """(top y, height, half width) of the strip across the lower disc."""
    return (SIZE / 2 + disc_r * 0.46, max(1.5, disc_r * 0.15), disc_r * 0.6)


def chevrons(rows, disc_r):
    """(centre x, tip y, half width, height, thickness) of each chevron,
    fitted between the disc's top and its strip: rows top first, each
    row's chevrons side by side."""
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
    # The strokes reach th/2 above the tip and about th below the base.
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


def mark_face(p, x, y, d, tip, h, th):
    """A chevron's colour at (x, y), `d` inside it: its face from top to
    bottom, and its dark edge."""
    lo, hi = min(tip, tip + h) - th / 2, max(tip, tip + h) + th / 2
    face = mix(p["face"][0], p["face"][1], clamp((y - lo) / (hi - lo)))
    if d > -min(0.75, th * 0.3):
        face = mix(face, p["edge"], 0.8)
    return face


def sample(name, x, y):
    """Straight RGBA (0..1) of the picture at (x, y), in pixels."""
    p = STYLE[name]
    if "monster" in p:
        return monster_sample(p, x, y)
    cx = cy = SIZE / 2
    dx, dy = x - cx, y - cy
    r = math.hypot(dx, dy)
    ang = math.atan2(dy, dx)
    frame = "rim" in p
    disc_r = 9.6 if frame else 11.95
    col = None
    # The spiked rim: twelve points between the disc and the edge.
    if frame:
        k = 12
        a = (ang + math.pi / 2) % (2 * math.pi / k) / (2 * math.pi / k)
        spike = 12.0 - abs(a - 0.5) * 2 * 3.2
        if disc_r - 0.5 <= r <= spike:
            light = clamp(0.5 - (dx + dy) / (2 * SIZE) * 1.4)
            col = mix(p["rim"][1], p["rim"][0], light)
            # A dark seam where the rim meets the disc.
            if r < disc_r + 0.6:
                col = mix(col, (0.2, 0.12, 0.05), 0.55)
    if r > disc_r:
        return (*col, 1.0) if col else (0.0, 0.0, 0.0, 0.0)
    t = clamp((y - (cy - disc_r)) / (2 * disc_r))
    col = mix(p["top"], p["bottom"], t ** 1.3)
    # Light from the upper left on the rim, shade on the lower right.
    lit = -(dx + dy) / (math.sqrt(2) * max(r, 1e-6))
    if disc_r - r < 1.3:
        if lit > 0:
            col = mix(col, (1.0, 1.0, 1.0), clamp(lit) * 0.85)
        else:
            col = mix(col, mix(p["top"], (0, 0, 0), 0.35), clamp(-lit) * 0.6)
    shade = mix(p["top"], (0, 0, 0), 0.4)
    if p.get("fall"):
        # The strip across the middle, broken at the centre, and a chevron
        # pointing down below the break.
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
    # Drop shadow of the chevrons on the disc (down and to the right).
    for (mx, tip, hw, h, th) in marks:
        if chevron_dist(x - 0.6, y - 0.8, mx, tip, hw, h, th) < 0:
            col = mix(col, shade, 0.25)
    for (mx, tip, hw, h, th) in marks:
        d = chevron_dist(x, y, mx, tip, hw, h, th)
        if d < 0:
            col = mark_face(p, x, y, d, tip, h, th)
    return (*col, 1.0)


# ---------------------------------------------------------------------------
# The Monster Bounce: a disc as the others', and shapes over and behind it.
# ---------------------------------------------------------------------------

def s_circle(cx, cy, r):
    return lambda x, y: math.hypot(x - cx, y - cy) - r


def s_seg(ax, ay, bx, by, th):
    dx, dy = bx - ax, by - ay
    ll = dx * dx + dy * dy

    def f(x, y):
        px, py = x - ax, y - ay
        t = clamp((px * dx + py * dy) / ll)
        return math.hypot(px - dx * t, py - dy * t) - th / 2
    return f


def s_poly(pts):
    """Signed distance to a polygon, any winding."""
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


def s_union(*fs):
    return lambda x, y: min(f(x, y) for f in fs)


def s_inter(*fs):
    return lambda x, y: max(f(x, y) for f in fs)


def s_chevron(cx, tip, half_w, height, thick):
    return lambda x, y: chevron_dist(x, y, cx, tip, half_w, height, thick)


def s_mirror(f):
    """A shape and its mirror image across the medal's middle."""
    return s_union(f, lambda x, y: f(SIZE - x, y))


def s_bounds(shape):
    """(top, bottom) of a shape, to a quarter pixel: its face runs between."""
    ys = [iy / 4 for iy in range(-8, SIZE * 4 + 9)
          if any(shape(ix / 4, iy / 4) < 0 for ix in range(-8, SIZE * 4 + 9))]
    return (ys[0], ys[-1] + 0.25) if ys else (0.0, float(SIZE))


def m_mark(shape, ew=0.6, shadow=True):
    """A pale mark: its shape, its dark edge's width, whether it casts a
    shadow."""
    lo, hi = s_bounds(shape)
    return dict(shape=shape, ew=ew, shadow=shadow, lo=lo, hi=hi)


def horn(cy):
    """A horn up from the disc's upper left, curving in to its point."""
    return s_poly([(3.6, cy - 2.2), (0.7, cy - 6.4), (2.4, cy - 12.0), (4.0, cy - 8.2), (7.4, cy - 6.8)])


def monster_launch():
    """The Superbounce's chevron out past the top of a rimmed disc, off two
    seams that fan out in a V from one point."""
    return dict(cy=12.0, r=9.6, spikes=12,
                flats=[s_chevron(12, 20.3, 3.3, -6.6, 1.4)],
                marks=[m_mark(s_chevron(12, 2.7, 6.2, 8.6, 3.2), 0.75)])


def monster_maw():
    """A horned face roaring: small eyes, a round mouth with teeth along
    its floor, and the chevron bursting up out of it."""
    cy = 12.7
    eye = s_poly([(3.7, 6.9), (8.5, 8.3), (8.2, 10.0), (4.7, 9.9)])
    teeth = s_mirror(s_union(
        s_poly([(6.5, 17.6), (8.9, 20.2), (9.3, 16.4)]),
        s_poly([(9.6, 20.7), (11.9, 21.3), (11.1, 17.8)])))
    return dict(cy=cy, r=9.3, spikes=0, back=[s_mirror(horn(cy))],
                flats=[s_circle(12, 15.5, 5.9)],
                marks=[m_mark(s_mirror(eye), 0.4, False), m_mark(teeth, 0.35, False),
                       m_mark(s_chevron(12, 7.4, 4.2, 6.0, 2.7), 0.7)])


def monster_horned():
    """A horned face: the seams' V for a brow, two lit eyes under it, a
    mouth of fangs, and the chevron up between the horns, past the disc."""
    cy = 12.7
    eye = s_poly([(5.0, 9.7), (10.7, 12.3), (10.1, 14.0), (5.9, 13.1)])
    mouth = s_inter(s_circle(12, 12.6, 7.6), lambda x, y: 15.9 - y)
    fangs = s_union(
        s_poly([(6.3, 15.8), (9.7, 15.8), (8.0, 19.6)]),
        s_poly([(14.3, 15.8), (17.7, 15.8), (16.0, 19.6)]),
        s_poly([(10.2, 15.8), (11.9, 15.8), (11.05, 17.6)]),
        s_poly([(12.1, 15.8), (13.8, 15.8), (12.95, 17.6)]))
    return dict(cy=cy, r=9.3, spikes=0, back=[s_mirror(horn(cy))],
                flats=[mouth, s_chevron(12, 12.3, 6.8, -4.6, 1.5)],
                marks=[m_mark(s_mirror(eye), 0.4, False), m_mark(fangs, 0.35, False),
                       m_mark(s_chevron(12, 2.3, 4.0, 4.6, 2.7), 0.7)])


def monster_grin():
    """A horned face built from the bounce marks: slanted eyes, and a wide
    mouth whose teeth are a row of chevrons."""
    cy = 12.7
    eye = s_poly([(4.6, 7.4), (10.7, 10.4), (10.1, 12.3), (5.6, 11.4)])
    mouth = s_inter(s_circle(12, 12.0, 8.0), lambda x, y: 14.2 - y)
    teeth = s_union(*[s_chevron(cx, 15.4, 1.9, 2.6, 1.5) for cx in (7.2, 12.0, 16.8)])
    return dict(cy=cy, r=9.3, spikes=0, back=[s_mirror(horn(cy))],
                flats=[mouth],
                marks=[m_mark(s_mirror(eye), 0.45, False), m_mark(teeth, 0.4, False)])


# The Monster Bounce's drawings by name; STYLE's `monster` picks one.
MONSTER_VARIANTS = dict(horned=monster_horned, maw=monster_maw, grin=monster_grin, launch=monster_launch)
_MONSTER = {}


def monster_sample(p, x, y):
    """Straight RGBA (0..1) of the Monster Bounce at (x, y): the disc and
    its light as `sample` draws them, horns and a spiked rim behind it,
    flat dark shapes on it (a mouth, seams), the marks' shadows, then the
    marks, which may stand past the disc."""
    v = _MONSTER.get(p["monster"])
    if v is None:
        v = _MONSTER[p["monster"]] = MONSTER_VARIANTS[p["monster"]]()
    cx, cy, disc_r = SIZE / 2, v["cy"], v["r"]
    dx, dy = x - cx, y - cy
    r = math.hypot(dx, dy)
    col = None
    if r > disc_r - 0.5:
        light = clamp(0.5 - (dx + dy) / (2 * SIZE) * 1.4)
        behind = mix(p["rim"][1], p["rim"][0], light)
        dark = mix(p["rim"][1], (0, 0, 0), 0.7)
        if v["spikes"]:
            k = v["spikes"]
            a = (math.atan2(dy, dx) + math.pi / 2) % (2 * math.pi / k) / (2 * math.pi / k)
            if r <= 12.0 - abs(a - 0.5) * 2 * 3.2:
                col = behind
        for shape in v.get("back", ()):
            d = shape(x, y)
            if d < 0:
                col = mix(behind, dark, 0.6) if d > -0.45 else behind
        # A dark seam where what is behind meets the disc.
        if col and r < disc_r + 0.6:
            col = mix(col, dark, 0.55)
    if r <= disc_r:
        t = clamp((y - (cy - disc_r)) / (2 * disc_r))
        col = mix(p["top"], p["bottom"], t ** 1.3)
        lit = -(dx + dy) / (math.sqrt(2) * max(r, 1e-6))
        if disc_r - r < 1.3:
            if lit > 0:
                col = mix(col, (1.0, 1.0, 1.0), clamp(lit) * 0.85)
            else:
                col = mix(col, mix(p["top"], (0, 0, 0), 0.35), clamp(-lit) * 0.6)
        for shape in v["flats"]:
            if shape(x, y) < 0:
                # Lit along its lower edge, as the strip is.
                col = mix(p["strip"], p["bottom"], 0.7) if shape(x, y + 0.6) >= 0 else p["strip"]
    if col:
        shade = mix(p["top"] if r <= disc_r else p["rim"][1], (0, 0, 0), 0.4)
        for k in v["marks"]:
            if k["shadow"] and k["shape"](x - 0.6, y - 0.8) < 0:
                col = mix(col, shade, 0.25)
    for k in v["marks"]:
        d = k["shape"](x, y)
        if d < 0:
            col = mix(p["face"][0], p["face"][1], clamp((y - k["lo"]) / (k["hi"] - k["lo"])))
            if d > -k["ew"]:
                col = mix(col, p["edge"], 0.8)
    return (*col, 1.0) if col else (0.0, 0.0, 0.0, 0.0)


def draw(name):
    """The picture as 24*24 straight RGBA floats, supersampled."""
    out = []
    for py in range(SIZE):
        for px in range(SIZE):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    r, g, b, a = sample(name, px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            a = acc[3] / (SS * SS)
            if a <= 0:
                out.append((0.0, 0.0, 0.0, 0.0))
            else:
                out.append((acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], a))
    return out


def argb4444(px):
    r, g, b, a = px
    q = lambda v: int(round(clamp(v) * 15))
    a4 = q(a)
    if a4 == 0:
        return 0
    return (a4 << 12) | (q(r) << 8) | (q(g) << 4) | q(b)


RS_HEAD = """\
//! The superbounce medal glyphs' pictures (docs/game/hud-kit.md, "The
//! medals"): 24 by 24, ARGB 4444, row-major, top row first. Written by
//! `scripts/superbounce-medals.py`, but for the part after the marker line
//! near the end (Incel), which `scripts/live-medals.py --font-art` writes;
//! each script keeps the other's part. New art is a new file of the same
//! shape. Do not edit the rows by hand unless the art is meant to change.

/// Width and height of each picture.
pub const SIZE: u16 = 24;

"""


# The line after which scripts/live-medals.py writes its glyph medals (Incel);
# what follows it is kept.
LIVE_MARK = "// ---- Live's medals with a glyph: written by scripts/live-medals.py --font-art ----\n"


def write_rs(path, pics):
    tail = ""
    if os.path.exists(path):
        with open(path) as f:
            text = f.read()
        if LIVE_MARK in text:
            tail = "\n" + LIVE_MARK + text.split(LIVE_MARK, 1)[1]
    with open(path, "w") as f:
        f.write(RS_HEAD)
        for i, ((unit, name, line), pic) in enumerate(zip(MEDALS, pics)):
            if i:
                f.write("\n")
            f.write(f"/// U+{unit:04X}, `{line}`.\n")
            f.write("#[rustfmt::skip]\n")
            f.write(f"pub const {name}: [u16; 576] = [\n")
            for y in range(SIZE):
                row = pic[y * SIZE:(y + 1) * SIZE]
                f.write("    " + ", ".join(f"0x{v:04x}" for v in row) + ",\n")
            f.write("];\n")
        f.write(tail)


def to_image(pic):
    from PIL import Image
    im = Image.new("RGBA", (SIZE, SIZE))
    for i, v in enumerate(pic):
        a, r, g, b = (v >> 12) & 15, (v >> 8) & 15, (v >> 4) & 15, v & 15
        im.putpixel((i % SIZE, i // SIZE), (r * 17, g * 17, b * 17, a * 17))
    return im


def medal_id(name):
    return name.lower().replace("_", "-")


HIRES = 512
HIRES_SS = 3


def draw_hires(name, HIRES=HIRES):
    """The same drawing sampled at 512 by 512, 3 by 3 samples a pixel, 8
    bits a channel: straight RGBA bytes, as `scripts/live-medals.py --hires`
    draws it. Not the 24-pixel picture enlarged."""
    k = SIZE / HIRES
    n2 = HIRES_SS * HIRES_SS
    out = bytearray()
    for py in range(HIRES):
        for px in range(HIRES):
            ar = ag = ab = aa = 0.0
            for sy in range(HIRES_SS):
                yy = (py + (sy + 0.5) / HIRES_SS) * k
                for sx in range(HIRES_SS):
                    r, g, b, a = sample(name, (px + (sx + 0.5) / HIRES_SS) * k, yy)
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
    return bytes(out)


def variants(out_dir, pics, old):
    """sheet-v2.png: the bounce medals, the Monster Bounce before (the
    picture `old`, if given) and each of `MONSTER_VARIANTS`: at their size,
    8 times, and drawn again at 256."""
    from PIL import Image, ImageDraw
    p = STYLE["MONSTER_BOUNCE"]
    chosen = p["monster"]
    cells = [(line, to_image(pic), None) for (_, name, line), pic in zip(MEDALS, pics) if name != "MONSTER_BOUNCE"]
    if old:
        cells.append(("before", Image.open(old).convert("RGBA"), None))
    for v in MONSTER_VARIANTS:
        if v == "launch" and old:
            continue
        p["monster"] = v
        im = to_image([argb4444(q) for q in draw("MONSTER_BOUNCE")])
        hi = Image.frombytes("RGBA", (256, 256), draw_hires("MONSTER_BOUNCE", 256))
        cells.append((v + (" (default)" if v == chosen else ""), im, hi))
    p["monster"] = chosen
    bg, ink = (28, 30, 38, 255), (235, 235, 240, 255)
    z, pad = 8, 16
    cell = SIZE * z + pad
    big = [c for c in cells if c[2]]
    w = max(pad + len(cells) * cell, pad + len(big) * (256 + pad))
    h = pad * 6 + 28 + SIZE + SIZE * z + 256
    out = Image.new("RGBA", (w, h), bg)
    dr = ImageDraw.Draw(out)
    for i, (label, im, _) in enumerate(cells):
        x = pad + i * cell
        dr.text((x, pad), label, fill=ink)
        out.alpha_composite(im, (x, pad * 2 + 14))
        out.alpha_composite(im.resize((SIZE * z, SIZE * z), Image.NEAREST), (x, pad * 3 + 14 + SIZE))
    y = pad * 4 + 14 + SIZE + SIZE * z
    for i, (label, _, hi) in enumerate(big):
        x = pad + i * (256 + pad)
        dr.text((x, y), label, fill=ink)
        out.alpha_composite(hi, (x, y + 14 + pad))
    path = os.path.join(out_dir, "sheet-v2.png")
    out.save(path)
    return path


def site_images(im):
    """The art pack's two pictures, as web/public/art/ART.md's Batch 07
    makes them: the glyph 5 times (120) centred on 128, and that
    box-filtered to 60 on 64."""
    from PIL import Image
    up = im.resize((SIZE * 5, SIZE * 5), Image.NEAREST)
    two = Image.new("RGBA", (128, 128), (0, 0, 0, 0))
    two.alpha_composite(up, (4, 4))
    one = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    one.alpha_composite(up.resize((60, 60), Image.BOX), (2, 2))
    return one, two


def art(out_dir, pics, only):
    """The medals' files, laid out as `scripts/live-medals.py --out` lays
    out Live's, and sheet.png: every bounce medal at its size, 8 times and
    at the site's sizes, and those of `only` at 256 from the hi-res."""
    from PIL import Image, ImageDraw
    for sub in ("game", "big", "site", "hires"):
        os.makedirs(os.path.join(out_dir, sub), exist_ok=True)
    imgs = {name: to_image(pic) for (_, name, _), pic in zip(MEDALS, pics)}
    names = [n for (_, n, _) in MEDALS if not only or medal_id(n) in only]
    halves = {}
    for name in names:
        mid, im = medal_id(name), imgs[name]
        im.save(os.path.join(out_dir, "game", f"{mid}.png"))
        im.resize((SIZE * 8, SIZE * 8), Image.NEAREST).save(os.path.join(out_dir, "big", f"{mid}.png"))
        one, two = site_images(im)
        one.save(os.path.join(out_dir, "site", f"{mid}.webp"), lossless=True)
        two.save(os.path.join(out_dir, "site", f"{mid}@2x.webp"), lossless=True)
        big = Image.frombytes("RGBA", (HIRES, HIRES), draw_hires(name))
        half = big.convert("RGBa").resize((256, 256), Image.BOX).convert("RGBA")
        big.save(os.path.join(out_dir, "hires", f"{mid}-512.png"), optimize=True)
        half.save(os.path.join(out_dir, "hires", f"{mid}-256.png"), optimize=True)
        halves[name] = half
    bg, ink = (28, 30, 38, 255), (235, 235, 240, 255)
    z, pad = 8, 16
    cell = SIZE * z + pad
    w = pad + len(MEDALS) * cell
    h = pad * 6 + 14 + SIZE + SIZE * z + 64 + 128 + (256 + pad if halves else 0)
    out = Image.new("RGBA", (max(w, pad + len(halves) * (256 + pad)), h), bg)
    dr = ImageDraw.Draw(out)
    for i, (_, name, line) in enumerate(MEDALS):
        x, y, im = pad + i * cell, pad, imgs[name]
        dr.text((x, y), line, fill=ink)
        y += 14 + pad
        out.alpha_composite(im, (x, y))
        y += SIZE + pad
        out.alpha_composite(im.resize((SIZE * z, SIZE * z), Image.NEAREST), (x, y))
        y += SIZE * z + pad
        one, two = site_images(im)
        out.alpha_composite(one, (x, y))
        out.alpha_composite(two, (x, y + 64 + pad))
    for i, name in enumerate(halves):
        out.alpha_composite(halves[name], (pad + i * (256 + pad), h - 256 - pad))
    path = os.path.join(out_dir, "sheet.png")
    out.save(path)
    return path


def h2_line(h2, disc, font, text, out):
    cmd = [h2, "font", disc, font, text, "--glyph-colors", "--color", "FFFFFF",
           "--out", out, "--force"]
    subprocess.run(cmd, check=True, capture_output=True)


def preview(out_dir, pics, h2, disc):
    from PIL import Image, ImageDraw
    os.makedirs(out_dir, exist_ok=True)
    bg = (28, 30, 38, 255)
    imgs = [to_image(p) for p in pics]
    for (unit, name, _), im in zip(MEDALS, imgs):
        im.save(os.path.join(out_dir, f"{name.lower()}.png"))
    z = 8
    pad = 16
    rows = []
    # Native and 8x.
    w = pad + len(imgs) * (SIZE * z + pad)
    sheet = Image.new("RGBA", (w, SIZE * z + SIZE + pad * 3), bg)
    for i, im in enumerate(imgs):
        x = pad + i * (SIZE * z + pad)
        sheet.alpha_composite(im, (x, pad))
        sheet.alpha_composite(im.resize((SIZE * z, SIZE * z), Image.NEAREST), (x, pad * 2 + SIZE))
    rows.append(sheet)
    # Lines: ours beside retail, in both message fonts.
    if h2 and disc:
        tmp = tempfile.mkdtemp()
        for font in ("conduit-13", "conduit-12"):
            for (unit, name, line), im in zip(MEDALS, imgs):
                a = os.path.join(tmp, "ours.png")
                h2_line(h2, disc, font, " " + line, a)
                words = Image.open(a).convert("RGBA")
                ours = Image.new("RGBA", (SIZE + words.width, max(SIZE, words.height)), (0, 0, 0, 0))
                ours.alpha_composite(im, (0, 0))
                ours.alpha_composite(words, (SIZE, 0))
                # A retail medal line of the same tier, its glyph and words.
                glyph, words_r = {
                    "SUPERBOUNCE": (0xE033, "Killing Spree!"),
                    "DOUBLE_BOUNCE": (0xE038, "Double Kill!"),
                    "TRIPLE_BOUNCE": (0xE039, "Triple Kill!"),
                    "QUAD_BOUNCE": (0xE040, "Overkill!"),
                    "CHAIN_BOUNCE": (0xE041, "Killtacular!"),
                    "BOUNCETACULAR": (0xE042, "Killtrocity!"),
                    "BOUNCED": (0xE043, "Killimanjaro!"),
                    "MONSTER_BOUNCE": (0xE042, "Killtrocity!"),
                }[name]
                retail = chr(glyph) + " " + words_r
                b = os.path.join(tmp, "retail.png")
                h2_line(h2, disc, font, retail, b)
                ret = Image.open(b).convert("RGBA")
                lw = 260
                line_img = Image.new("RGBA", (pad + lw * 2, ours.height + 8), bg)
                line_img.alpha_composite(ours, (pad, 4))
                line_img.alpha_composite(ret, (pad + lw, 4))
                big = line_img.resize((line_img.width * 2, line_img.height * 2), Image.NEAREST)
                rows.append(big)
            label = Image.new("RGBA", (200, 14), bg)
            ImageDraw.Draw(label).text((pad, 0), f"{font}: ours (left), retail (right), 2x", fill=(200, 200, 200, 255))
            rows.append(label)
    W = max(r.width for r in rows)
    H = sum(r.height for r in rows) + pad
    out = Image.new("RGBA", (W, H), bg)
    y = 0
    for r in rows:
        out.alpha_composite(r, (0, y))
        y += r.height
    path = os.path.join(out_dir, "superbounce-medals-preview.png")
    out.save(path)
    return path


def main():
    ap = argparse.ArgumentParser()
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--rs", default=os.path.join(root, "crates/formats/font/src/medals/art.rs"))
    ap.add_argument("--preview")
    ap.add_argument("--h2")
    ap.add_argument("--disc", default=os.environ.get("H2_DISC"))
    ap.add_argument("--art", help="a directory for the game, big, site and hires files and sheet.png")
    ap.add_argument("--only", help="with --art: the ids to write, comma separated")
    ap.add_argument("--monster", choices=list(MONSTER_VARIANTS), help="the Monster Bounce's drawing, if not STYLE's")
    ap.add_argument("--variants", help="a directory for sheet-v2.png: every drawing of the Monster Bounce")
    ap.add_argument("--before", help="with --variants: a 24 by 24 picture of the medal as it was")
    a = ap.parse_args()
    if a.monster:
        STYLE["MONSTER_BOUNCE"]["monster"] = a.monster
    pics = [[argb4444(p) for p in draw(name)] for (_, name, _) in MEDALS]
    os.makedirs(os.path.dirname(a.rs), exist_ok=True)
    write_rs(a.rs, pics)
    print(a.rs)
    if a.preview:
        print(preview(a.preview, pics, a.h2, a.disc))
    if a.variants:
        os.makedirs(a.variants, exist_ok=True)
        print(variants(a.variants, pics, a.before))
    if a.art:
        print(art(a.art, pics, a.only.split(",") if a.only else None))


if __name__ == "__main__":
    sys.exit(main())
