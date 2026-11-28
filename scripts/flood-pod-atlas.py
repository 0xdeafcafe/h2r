#!/usr/bin/env python3
"""Paints the Flood pod's texture atlas (content/flood-pod/README.md).

    python3 -I scripts/flood-pod-atlas.py            # writes content/flood-pod/flood_pod.png
    python3 -I scripts/flood-pod-atlas.py --check    # exits 1 if the PNG's pixels differ

Standard library only, no input files, fixed seeds: the same pixels every
run. The atlas is 512x256 RGBA, opaque. Its regions, in pixels
(x0, y0, x1, y1), are the table `content/flood-pod/kit.yaml`
(`regions`) holds, which the model's parts name:

    flesh      0    0  256  128   the root bulb, u around it
    petal      256  0  512  128   a petal's plate, u from hinge to tip
    sac        0  128  128  256   the sac under the petals, the pustules
    bone       128 128  256  256   the tip, the petals' spines
    rib        256 128  384  256   the collar, the ribs down the root
    tendril    384 128  512  256   the tendrils, u from root to end

A lathe maps u (left to right) around its axis and v (top to bottom) along
its profile; a sweep maps u along its path and v around its section; a box
maps the whole region onto each face.

Both runs print the pixels' hash (FNV-1a, 64 bits, over the RGBA bytes row
by row), which `content-kit`'s tests hold (`ATLAS_PIXELS`). `--check`
compares pixels, not file bytes: zlib builds differ in what they write.
"""

import math
import os
import struct
import sys
import zlib

W, H = 512, 256
REGIONS = {
    "flesh": (0, 0, 256, 128),
    "petal": (256, 0, 512, 128),
    "sac": (0, 128, 128, 256),
    "bone": (128, 128, 256, 256),
    "rib": (256, 128, 384, 256),
    "tendril": (384, 128, 512, 256),
}

# How far round a petal's section its outer face goes (v, 0 to 1): the rest
# is the inner face. From the section's edge lengths in flood_pod.yaml.
PETAL_OUTER = 0.513


def h32(x, y, seed):
    """A hash of two integers and a seed, 0..1."""
    n = (x * 374761393 + y * 668265263 + seed * 2246822519) & 0xFFFFFFFF
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    n ^= n >> 16
    return n / 4294967295.0


def vnoise(x, y, scale, seed):
    """Value noise, smooth over `scale` pixels."""
    fx, fy = x / scale, y / scale
    ix, iy = math.floor(fx), math.floor(fy)
    tx, ty = fx - ix, fy - iy
    tx, ty = tx * tx * (3 - 2 * tx), ty * ty * (3 - 2 * ty)
    a, b = h32(ix, iy, seed), h32(ix + 1, iy, seed)
    c, d = h32(ix, iy + 1, seed), h32(ix + 1, iy + 1, seed)
    return (a + (b - a) * tx) * (1 - ty) + (c + (d - c) * tx) * ty


def fbm(x, y, scale, seed):
    return (
        vnoise(x, y, scale, seed) * 0.55
        + vnoise(x, y, scale / 2, seed + 1) * 0.3
        + vnoise(x, y, scale / 4, seed + 2) * 0.15
    )


def mix(a, b, t):
    t = max(0.0, min(1.0, t))
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def scale(c, k):
    return tuple(v * k for v in c)


class Rng:
    """A small fixed generator, so the walks do not depend on `random`."""

    def __init__(self, seed):
        self.s = seed & 0xFFFFFFFF or 1

    def next(self):
        self.s ^= (self.s << 13) & 0xFFFFFFFF
        self.s ^= self.s >> 17
        self.s ^= (self.s << 5) & 0xFFFFFFFF
        return self.s / 4294967295.0

    def range(self, a, b):
        return a + (b - a) * self.next()


class Canvas:
    def __init__(self):
        self.px = [(0.0, 0.0, 0.0)] * (W * H)

    def paint(self, region, fn):
        """`fn(x, y, w, h)` (region-local pixels) gives each pixel's colour."""
        x0, y0, x1, y1 = REGIONS[region]
        w, h = x1 - x0, y1 - y0
        for y in range(h):
            row = (y0 + y) * W + x0
            for x in range(w):
                self.px[row + x] = fn(x, y, w, h)

    def blend(self, region, x, y, colour, alpha):
        x0, y0, x1, y1 = REGIONS[region]
        if 0 <= x < x1 - x0 and 0 <= y < y1 - y0 and alpha > 0:
            i = (y0 + y) * W + x0 + x
            self.px[i] = mix(self.px[i], colour, alpha)

    def disc(self, region, cx, cy, r, colour, alpha):
        for y in range(int(cy - r - 1), int(cy + r + 2)):
            for x in range(int(cx - r - 1), int(cx + r + 2)):
                d = math.hypot(x - cx, y - cy)
                self.blend(region, x, y, colour, alpha * max(0.0, min(1.0, r - d + 0.5)))

    def vein(self, region, rng, start, heading, steps, colour, alpha, width, band=None):
        """A wandering line that forks: a vein. `band` keeps it between two
        rows (y0, y1)."""
        x, y = start
        for i in range(steps):
            heading += rng.range(-0.45, 0.45)
            x += math.cos(heading) * 1.6
            y += math.sin(heading) * 1.6
            if band and not band[0] <= y < band[1]:
                heading = -heading
                y = max(band[0], min(band[1] - 1, y))
            fade = 1.0 - i / steps
            r = max(0.6, width * (0.4 + 0.6 * fade))
            self.disc(region, x, y, r, colour, alpha * (0.5 + 0.5 * fade))
            if width > 1.2 and rng.next() < 0.035:
                self.vein(
                    region,
                    rng,
                    (x, y),
                    heading + rng.range(0.5, 1.1) * (1 if rng.next() < 0.5 else -1),
                    int((steps - i) * 0.6),
                    colour,
                    alpha * 0.8,
                    width * 0.6,
                    band,
                )

    def rgba(self):
        raw = bytearray()
        for r, g, b in self.px:
            raw += bytes(
                (
                    max(0, min(255, int(r + 0.5))),
                    max(0, min(255, int(g + 0.5))),
                    max(0, min(255, int(b + 0.5))),
                    255,
                )
            )
        return bytes(raw)


def png(rgba):
    raw = bytearray()
    for y in range(H):
        raw.append(0)
        raw += rgba[y * W * 4 : (y + 1) * W * 4]

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def unpng(data):
    """The RGBA bytes of a PNG this script wrote (8-bit RGBA, filter 0)."""
    at, idat = 8, b""
    while at < len(data):
        (n,) = struct.unpack(">I", data[at : at + 4])
        if data[at + 4 : at + 8] == b"IDAT":
            idat += data[at + 8 : at + 8 + n]
        at += 12 + n
    raw = zlib.decompress(idat)
    row = W * 4 + 1
    if len(raw) != row * H or any(raw[y * row] for y in range(H)):
        return None
    return b"".join(raw[y * row + 1 : (y + 1) * row] for y in range(H))


def fnv64(data):
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


# The Flood's colours, ours: sallow flesh, bruised folds, raw meat, old
# bone, and the sick yellow-green of what grows inside.
FLESH = (156, 132, 92)
FLESH_PALE = (196, 176, 128)
BRUISE = (86, 52, 58)
MEAT = (150, 58, 52)
MEAT_DARK = (84, 28, 30)
HIDE = (74, 66, 44)
HIDE_DARK = (38, 34, 26)
BONE = (214, 200, 160)
BONE_DARK = (120, 102, 70)
GLOW = (214, 222, 96)
GLOW_DEEP = (96, 124, 40)
GRISTLE = (110, 60, 50)
TENDRIL = (104, 70, 78)
TENDRIL_DARK = (40, 26, 32)


def paint(c):
    # The root: swollen rings going down it (the lathe's profile runs top
    # to bottom in v), mottled, bruised in the folds, veined.
    def root(x, y, w, h):
        v = y / h
        swell = 0.5 + 0.5 * math.cos(v * math.tau * 5.0 + 0.6 * fbm(x, y, 40, 11))
        base = mix(FLESH, FLESH_PALE, fbm(x, y, 28, 12))
        base = mix(base, BRUISE, (1.0 - swell) ** 2 * 0.75)
        base = mix(base, MEAT, max(0.0, fbm(x, y, 18, 15) - 0.55) * 1.6)
        # Darker toward the tip, where it has been through the air.
        base = mix(base, HIDE_DARK, max(0.0, v - 0.7) * 1.4)
        return scale(base, 0.86 + 0.28 * vnoise(x, y, 3, 16))

    c.paint("flesh", root)
    rng = Rng(101)
    for _ in range(9):
        c.vein("flesh", rng, (rng.range(0, 256), rng.range(0, 30)), math.pi / 2, 70, MEAT_DARK, 0.7, 2.2)
    for _ in range(14):
        c.disc("flesh", rng.range(6, 250), rng.range(8, 120), rng.range(2, 5), FLESH_PALE, 0.5)

    # A petal: its outer face a hide ridged from hinge to tip, its inner
    # face raw meat with a wet seam down the middle.
    outer = int(128 * PETAL_OUTER)

    def petal(x, y, w, h):
        u = x / w
        if y < outer:
            across = y / outer
            ridge = 0.5 + 0.5 * math.cos(across * math.tau * 6.0 + 0.8 * fbm(x, y, 50, 21))
            base = mix(HIDE_DARK, HIDE, ridge * (0.6 + 0.4 * fbm(x, y, 20, 22)))
            # Flesh shows through near the hinge and at the edges.
            edge = min(across, 1.0 - across)
            base = mix(base, FLESH, max(0.0, 0.12 - edge) * 5.0)
            base = mix(base, BRUISE, max(0.0, 0.18 - u) * 3.0)
            base = mix(base, BONE_DARK, max(0.0, u - 0.82) * 3.0)
        else:
            across = (y - outer) / (h - outer)
            base = mix(MEAT, MEAT_DARK, fbm(x, y, 16, 25))
            seam = math.exp(-(((across - 0.5) / 0.07) ** 2))
            base = mix(base, GLOW_DEEP, seam * 0.6)
            base = mix(base, FLESH_PALE, max(0.0, vnoise(x, y, 5, 26) - 0.78) * 2.5)
        return scale(base, 0.86 + 0.28 * vnoise(x, y, 3, 27))

    c.paint("petal", petal)
    rng = Rng(202)
    for _ in range(7):
        c.vein("petal", rng, (rng.range(0, 40), rng.range(4, outer - 4)), 0.0, 110, MEAT_DARK, 0.55, 1.8, (0, outer))
    for _ in range(6):
        c.vein("petal", rng, (rng.range(0, 60), rng.range(outer + 4, 124)), 0.0, 90, BRUISE, 0.6, 1.8, (outer, 128))

    # The sac: a membrane lit from inside, brightest in the middle rows,
    # with dark shapes behind it.
    def sac(x, y, w, h):
        v = y / h
        lit = 0.45 + 0.55 * math.sin(v * math.pi)
        base = mix(GLOW_DEEP, GLOW, lit * (0.55 + 0.45 * fbm(x, y, 22, 31)))
        shape = max(0.0, fbm(x, y, 30, 34) - 0.52) * 3.0
        base = mix(base, HIDE_DARK, shape * 0.7)
        return scale(base, 0.9 + 0.2 * vnoise(x, y, 3, 35))

    c.paint("sac", sac)
    rng = Rng(303)
    for _ in range(8):
        c.vein("sac", rng, (rng.range(0, 128), rng.range(0, 128)), rng.range(0, math.tau), 60, GRISTLE, 0.6, 1.6)

    # Bone: pale, grained along u, stained in its cracks.
    def bone(x, y, w, h):
        grain = fbm(x, y * 6, 36, 41)
        base = mix(BONE_DARK, BONE, 0.35 + 0.65 * grain)
        base = mix(base, GRISTLE, max(0.0, fbm(x, y, 14, 44) - 0.62) * 2.0)
        return scale(base, 0.88 + 0.24 * vnoise(x, y, 3, 45))

    c.paint("bone", bone)
    rng = Rng(404)
    for _ in range(6):
        c.vein("bone", rng, (rng.range(0, 30), rng.range(0, 128)), 0.0, 60, BONE_DARK, 0.8, 1.0)

    # The collar and ribs: gristle, banded across.
    def rib(x, y, w, h):
        band = 0.5 + 0.5 * math.cos(x / w * math.tau * 9.0 + fbm(x, y, 24, 51))
        base = mix(MEAT_DARK, GRISTLE, band * (0.5 + 0.5 * fbm(x, y, 18, 52)))
        base = mix(base, FLESH, max(0.0, vnoise(x, y, 6, 54) - 0.74) * 2.0)
        return scale(base, 0.86 + 0.28 * vnoise(x, y, 3, 55))

    c.paint("rib", rib)

    # A tendril: thick and bruised at the root (u 0), dark and dry at its
    # end, ringed all the way.
    def tendril(x, y, w, h):
        u = x / w
        ring = 0.5 + 0.5 * math.cos(u * math.tau * 14.0 + 0.5 * fbm(x, y, 20, 61))
        base = mix(TENDRIL, TENDRIL_DARK, u * 0.8 + 0.2 * (1.0 - ring))
        base = mix(base, FLESH, max(0.0, 0.2 - u) * 3.0 * fbm(x, y, 12, 63))
        return scale(base, 0.86 + 0.28 * vnoise(x, y, 3, 64))

    c.paint("tendril", tendril)


def main():
    out = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "content", "flood-pod")
    path = os.path.join(out, "flood_pod.png")
    c = Canvas()
    paint(c)
    rgba = c.rgba()
    print(f"pixels: {fnv64(rgba):#018x}")
    if "--check" in sys.argv[1:]:
        with open(path, "rb") as f:
            same = unpng(f.read()) == rgba
        print(f"{path}: {'same' if same else 'DIFFERENT'}")
        sys.exit(0 if same else 1)
    os.makedirs(out, exist_ok=True)
    data = png(rgba)
    with open(path, "wb") as f:
        f.write(data)
    print(f"{path}: {len(data)} bytes")


if __name__ == "__main__":
    main()
