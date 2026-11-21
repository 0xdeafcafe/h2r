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
    (0xE2A3, "CHAIN_BOUNCE", "Chain bounce!"),
]

# Palettes, 0..1 RGB: the disc from its top to its bottom (the retail discs
# are lighter low down), and the chevrons' face, top to bottom, and edge.
PALETTES = {
    # Bronze, a green chevron.
    "SUPERBOUNCE": dict(
        top=(0.80, 0.50, 0.24), bottom=(0.98, 0.76, 0.48),
        face=((0.60, 1.0, 0.45), (0.12, 0.62, 0.16)), edge=(0.03, 0.24, 0.05),
        strip=(0.42, 0.22, 0.08)),
    # Gold, blue chevrons (beside the Double Kill's gold and blue).
    "DOUBLE_BOUNCE": dict(
        top=(1.0, 0.86, 0.42), bottom=(1.0, 1.0, 0.70),
        face=((0.42, 0.58, 1.0), (0.14, 0.22, 0.78)), edge=(0.04, 0.06, 0.36),
        strip=(0.66, 0.46, 0.10)),
    # Steel, teal chevrons.
    "TRIPLE_BOUNCE": dict(
        top=(0.55, 0.66, 0.72), bottom=(0.80, 0.88, 0.90),
        face=((0.40, 1.0, 0.90), (0.0, 0.62, 0.60)), edge=(0.0, 0.20, 0.24),
        strip=(0.26, 0.34, 0.40)),
    # Silver-lavender with a gold spiked rim, purple chevrons.
    "CHAIN_BOUNCE": dict(
        top=(0.80, 0.78, 0.92), bottom=(0.97, 0.97, 1.0),
        face=((0.90, 0.58, 1.0), (0.46, 0.14, 0.74)), edge=(0.18, 0.03, 0.32),
        strip=(0.46, 0.42, 0.62), rim=((1.0, 0.88, 0.48), (0.72, 0.44, 0.12))),
}

CHEVRONS = {"SUPERBOUNCE": 1, "DOUBLE_BOUNCE": 2, "TRIPLE_BOUNCE": 3, "CHAIN_BOUNCE": 3}


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def clamp(v):
    return max(0.0, min(1.0, v))


def chevron_dist(x, y, cx, tip, half_w, height, thick):
    """Signed distance (<0 inside) to a ^ chevron: two strokes from the
    bottom corners up to the tip at (cx, tip)."""
    def seg(ax, ay, bx, by):
        px, py = x - ax, y - ay
        dx, dy = bx - ax, by - ay
        t = clamp((px * dx + py * dy) / (dx * dx + dy * dy))
        ex, ey = px - dx * t, py - dy * t
        return math.hypot(ex, ey)
    base = tip + height
    d = min(seg(cx - half_w, base, cx, tip), seg(cx + half_w, base, cx, tip))
    return d - thick / 2


def chevrons(n, disc_r):
    """(tip y, half width, height, thickness) of each chevron, top first,
    in pixels from the top of the picture, fitted between the disc's top
    and its strip."""
    c = SIZE / 2
    top = c - disc_r * 0.74
    bottom = strip(disc_r)[0] - 1.3
    k = disc_r / 11.95
    if n == 1:
        h, th, hw = 5.6 * k, 3.4 * k, 6.4 * k
    elif n == 2:
        h, th, hw = 4.0 * k, 2.5 * k, 6.0 * k
    else:
        h, th, hw = 2.9 * k, 1.8 * k, 5.6 * k
    # The strokes reach th/2 above the tip and about th below the base.
    span = bottom - top - th / 2 - h - th * 0.8
    gap = span / (n - 1) if n > 1 else 0
    first = top + th / 2 + (span - gap * (n - 1)) / 2 if n == 1 else top + th / 2
    return [(first + i * gap, hw, h, th) for i in range(n)]


def strip(disc_r):
    """(top y, height, half width) of the strip across the lower disc."""
    return (SIZE / 2 + disc_r * 0.46, max(1.5, disc_r * 0.15), disc_r * 0.6)


def sample(name, x, y):
    """Straight RGBA (0..1) of the picture at (x, y), in pixels."""
    p = PALETTES[name]
    cx = cy = SIZE / 2
    dx, dy = x - cx, y - cy
    r = math.hypot(dx, dy)
    ang = math.atan2(dy, dx)
    frame = name == "CHAIN_BOUNCE"
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
    if r <= disc_r:
        t = clamp((y - (cy - disc_r)) / (2 * disc_r))
        col = mix(p["top"], p["bottom"], t ** 1.3)
        edge = disc_r - r
        # Light from the upper left on the rim, shade on the lower right.
        lit = -(dx + dy) / (math.sqrt(2) * max(r, 1e-6))
        if edge < 1.3:
            if lit > 0:
                col = mix(col, (1.0, 1.0, 1.0), clamp(lit) * 0.85)
            else:
                col = mix(col, mix(p["top"], (0, 0, 0), 0.35), clamp(-lit) * 0.6)
        # The strip: a sunk bar across the lower disc, lit on its lower lip.
        strip_y, strip_h, strip_w = strip(disc_r)
        if abs(dx) <= strip_w and strip_y <= y <= strip_y + strip_h:
            col = p["strip"]
            if y > strip_y + strip_h - 0.6:
                col = mix(p["strip"], p["bottom"], 0.7)
        marks = chevrons(CHEVRONS[name], disc_r)
        top = 0.0
        # Drop shadow of the chevrons on the disc (down and to the right).
        for (tip, hw, h, th) in marks:
            if chevron_dist(x - 0.6, y - 0.8, cx, top + tip, hw, h, th) < 0:
                col = mix(col, mix(p["top"], (0, 0, 0), 0.4), 0.25)
        for (tip, hw, h, th) in marks:
            d = chevron_dist(x, y, cx, top + tip, hw, h, th)
            if d < 0:
                tt = clamp((y - (top + tip - th / 2)) / (h + th))
                face = mix(p["face"][0], p["face"][1], tt)
                if d > -min(0.75, th * 0.3):
                    face = mix(face, p["edge"], 0.8)
                col = face
    if col is None:
        return (0.0, 0.0, 0.0, 0.0)
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
                retail = {
                    "SUPERBOUNCE": " Killing Spree!",
                    "DOUBLE_BOUNCE": " Double Kill!",
                    "TRIPLE_BOUNCE": " Triple Kill!",
                    "CHAIN_BOUNCE": " Killing Frenzy!",
                }[name]
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
