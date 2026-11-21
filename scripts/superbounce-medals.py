#!/usr/bin/env python3
"""Draw the four superbounce medal glyphs (docs/game/hud-kit.md, "The medals").

Writes the art as data, `crates/formats/font/src/medals/art.rs`: four
24 x 24 ARGB 4444 pictures that `font::medals` adds to the HUD message fonts.
The pictures are drawn here from discs, polygons and gradients in the manner
of Halo 2's multi-kill medals (a shaded disc lit from the upper left, the tier
told by the count of marks and the disc's colour, a frame for the top tier);
nothing is copied from a disc font or anyone's artwork. New art is a new
`art.rs`: edit the drawing below, or replace the file by hand.

    python3 scripts/superbounce-medals.py            # write art.rs
    python3 scripts/superbounce-medals.py --preview DIR [--h2 BIN] [--disc ISO]

`--preview` also writes a sheet (each glyph at its size and 8 times, and a
mock notification line beside a retail one, drawn by `h2 font`) to DIR. It
needs Pillow; writing art.rs needs nothing outside the standard library.
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
]

# Each medal: the disc from its top to its bottom (the retail discs are
# lighter low down), the chevrons' face (top to bottom) and edge, the
# strip's colour, the chevrons as rows top first (a count of chevrons in
# each row: the tier, as a multi-kill disc counts its stars), and for the
# top tier a spiked rim. `BOUNCED` is a fall through a broken strip.
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


def sample(name, x, y):
    """Straight RGBA (0..1) of the picture at (x, y), in pixels."""
    p = STYLE[name]
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
            lo, hi = min(tip, tip + h) - th / 2, max(tip, tip + h) + th / 2
            face = mix(p["face"][0], p["face"][1], clamp((y - lo) / (hi - lo)))
            if d > -min(0.75, th * 0.3):
                face = mix(face, p["edge"], 0.8)
            col = face
    return (*col, 1.0)


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
//! `scripts/superbounce-medals.py`; new art is a new file of the same
//! shape. Do not edit the rows by hand unless the art is meant to change.

/// Width and height of each picture.
pub const SIZE: u16 = 24;

"""


def write_rs(path, pics):
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


def to_image(pic):
    from PIL import Image
    im = Image.new("RGBA", (SIZE, SIZE))
    for i, v in enumerate(pic):
        a, r, g, b = (v >> 12) & 15, (v >> 8) & 15, (v >> 4) & 15, v & 15
        im.putpixel((i % SIZE, i // SIZE), (r * 17, g * 17, b * 17, a * 17))
    return im


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
    a = ap.parse_args()
    pics = [[argb4444(p) for p in draw(name)] for (_, name, _) in MEDALS]
    os.makedirs(os.path.dirname(a.rs), exist_ok=True)
    write_rs(a.rs, pics)
    print(a.rs)
    if a.preview:
        print(preview(a.preview, pics, a.h2, a.disc))


if __name__ == "__main__":
    sys.exit(main())
