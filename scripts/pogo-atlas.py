#!/usr/bin/env python3
"""Paints the Pogo Stick's three texture atlases (content/pogo/README.md).

    python3 -I scripts/pogo-atlas.py            # writes content/pogo/models/<style>.png
    python3 -I scripts/pogo-atlas.py --check    # exits 1 if a PNG differs

Standard library only, no input files, fixed seeds: the same bytes every
run. Each atlas is 512x256 RGBA, opaque. The regions are the same on all
three, in pixels (x0, y0, x1, y1), and `content/pogo/kit.yaml`
(`regions`) holds the same table, which the models' parts name:

    body       0    0  256  128   the main shell, u around the shaft
    body_dark  256  0  384  128   the lower sleeve and the dark frame parts
    piston     384  0  512  128   the piston rod or light column
    grip       0  128  128  192   the two hand grips
    peg        0  192  128  256   the foot pegs' tread
    foot       128 128  256  256   the foot
    accent     256 128  384  256   painted panels, numerals, light seams
    glow       384 128  512  256   the coil, the plasma, the light rings

A lathe maps u (left to right) around its axis and v (top to bottom) along
its profile; a sweep maps u along its path and v around its section; a box
maps the whole region onto each face.
"""

import math
import os
import struct
import sys
import zlib

W, H = 512, 256
REGIONS = {
    "body": (0, 0, 256, 128),
    "body_dark": (256, 0, 384, 128),
    "piston": (384, 0, 512, 128),
    "grip": (0, 128, 128, 192),
    "peg": (0, 192, 128, 256),
    "foot": (128, 128, 256, 256),
    "accent": (256, 128, 384, 256),
    "glow": (384, 128, 512, 256),
}


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

    def pick(self, n):
        return min(n - 1, int(self.next() * n))


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

    def line(self, region, a, b, colour, alpha, width=1):
        (ax, ay), (bx, by) = a, b
        n = int(max(abs(bx - ax), abs(by - ay))) + 1
        for i in range(n + 1):
            t = i / n
            x, y = ax + (bx - ax) * t, ay + (by - ay) * t
            for dx in range(width):
                for dy in range(width):
                    self.blend(region, int(x) + dx, int(y) + dy, colour, alpha)

    def disc(self, region, cx, cy, r, colour, alpha):
        for y in range(int(cy - r - 1), int(cy + r + 2)):
            for x in range(int(cx - r - 1), int(cx + r + 2)):
                d = math.hypot(x - cx, y - cy)
                self.blend(region, x, y, colour, alpha * max(0.0, min(1.0, r - d + 0.5)))

    def png(self):
        raw = bytearray()
        for y in range(H):
            raw.append(0)
            for x in range(W):
                r, g, b = self.px[y * W + x]
                raw += bytes(
                    (
                        max(0, min(255, int(r + 0.5))),
                        max(0, min(255, int(g + 0.5))),
                        max(0, min(255, int(b + 0.5))),
                        255,
                    )
                )

        def chunk(kind, data):
            body = kind + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

        return (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
            + chunk(b"IEND", b"")
        )


# Our own 5x7 stencil digits: each row five bits, the stencil bridges cut in.
DIGITS = {
    "0": ("01110", "10001", "10001", "00000", "10001", "10001", "01110"),
    "1": ("00100", "01100", "00100", "00000", "00100", "00100", "01110"),
    "2": ("01110", "10001", "00001", "00110", "01000", "10000", "11011"),
    "3": ("11110", "00001", "00001", "01100", "00001", "00001", "11110"),
    "4": ("10010", "10010", "10010", "11011", "00010", "00010", "00010"),
    "5": ("11011", "10000", "10000", "01110", "00001", "00001", "11110"),
    "6": ("01110", "10000", "10000", "10110", "10001", "10001", "01110"),
    "7": ("11011", "00001", "00010", "00000", "00100", "00100", "00100"),
    "8": ("01110", "10001", "10001", "00000", "10001", "10001", "01110"),
    "9": ("01110", "10001", "10001", "01101", "00001", "00001", "01110"),
}


def stencil(c, region, text, x, y, cell, colour, seed):
    """`text` at (x, y), each stencil cell `cell` pixels, chipped by noise."""
    for k, ch in enumerate(text):
        rows = DIGITS[ch]
        for j, row in enumerate(rows):
            for i, bit in enumerate(row):
                if bit != "1":
                    continue
                for dy in range(cell):
                    for dx in range(cell):
                        px, py = x + (k * 6 + i) * cell + dx, y + j * cell + dy
                        chip = fbm(px, py, 5, seed)
                        c.blend(region, px, py, colour, 0.95 if chip > 0.34 else 0.25)


def scratches(c, region, seed, count, colour, alpha, length=(6, 26)):
    x0, y0, x1, y1 = REGIONS[region]
    r = Rng(seed)
    for _ in range(count):
        ax, ay = r.range(0, x1 - x0), r.range(0, y1 - y0)
        ang = r.range(-0.5, 0.5) + (math.pi / 2 if r.next() < 0.6 else 0)
        ln = r.range(*length)
        c.line(
            region,
            (ax, ay),
            (ax + math.cos(ang) * ln, ay + math.sin(ang) * ln),
            colour,
            alpha * r.range(0.4, 1.0),
        )


def worn_metal(base, bare, seed, grain=1.0):
    """Paint over metal: the paint chipped where the noise is high and at
    the region's edges, the bare metal showing through."""

    def fn(x, y, w, h):
        n = fbm(x, y, 22, seed)
        fine = h32(x, y, seed + 7)
        shade = 0.86 + 0.22 * fbm(x, y, 48, seed + 3) + (fine - 0.5) * 0.07 * grain
        col = scale(base, shade)
        edge = min(x, y, w - 1 - x, h - 1 - y)
        wear = (n - 0.62) * 6 + max(0.0, (5 - edge) / 5) * (0.5 + n)
        if wear > 0:
            col = mix(col, scale(bare, 0.85 + 0.3 * fine), min(1.0, wear))
        return col

    return fn


def seams(c, region, xs, ys, dark, light):
    """Panel seams: a dark line and a light lip beside it."""
    x0, y0, x1, y1 = REGIONS[region]
    w, h = x1 - x0, y1 - y0
    for x in xs:
        c.line(region, (x, 0), (x, h - 1), dark, 0.85)
        c.line(region, (x + 1, 0), (x + 1, h - 1), light, 0.35)
    for y in ys:
        c.line(region, (0, y), (w - 1, y), dark, 0.85)
        c.line(region, (0, y + 1), (w - 1, y + 1), light, 0.35)


def rivets(c, region, points, dark, light):
    for x, y in points:
        c.disc(region, x + 0.6, y + 0.8, 2.6, dark, 0.7)
        c.disc(region, x, y, 2.0, light, 0.9)
        c.disc(region, x - 0.6, y - 0.6, 0.8, (255, 255, 255), 0.5)


def cylinder_shine(base, bands, seed):
    """A polished rod: light and dark bands across u, brushed along v."""

    def fn(x, y, w, h):
        u = x / w
        k = 0.0
        for centre, width, gain in bands:
            d = min(abs(u - centre), 1 - abs(u - centre))
            k += gain * math.exp(-((d / width) ** 2))
        brushed = (h32(x, y // 3, seed) - 0.5) * 0.06
        return scale(base, 0.8 + k + brushed)

    return fn


def paint_unsc(c):
    olive, steel, gun = (84, 92, 58), (148, 150, 146), (56, 60, 64)
    c.paint("body", worn_metal(olive, steel, 11))
    seams(c, "body", [0, 64, 128, 192, 255], [14, 96, 120], (26, 30, 20), (132, 138, 100))
    rivets(
        c,
        "body",
        [(x + dx, y) for x in (0, 64, 128, 192) for dx in (7, 57) for y in (22, 88)],
        (30, 34, 24),
        (120, 126, 100),
    )
    stencil(c, "body", "07", 78, 40, 4, (206, 164, 42), 31)
    stencil(c, "body", "07", 206, 40, 4, (206, 164, 42), 32)
    scratches(c, "body", 12, 70, (170, 172, 160), 0.5)
    scratches(c, "body", 13, 30, (30, 34, 24), 0.4)

    c.paint("body_dark", worn_metal(gun, (160, 162, 164), 21))
    x0, y0, x1, y1 = REGIONS["body_dark"]
    for y in range(6, y1 - y0, 14):
        c.line("body_dark", (0, y), (x1 - x0 - 1, y), (22, 24, 28), 0.8, 2)
        c.line("body_dark", (0, y + 2), (x1 - x0 - 1, y + 2), (110, 116, 122), 0.45)
    scratches(c, "body_dark", 22, 40, (176, 180, 184), 0.45)

    c.paint("piston", cylinder_shine((150, 156, 164), [(0.28, 0.07, 0.55), (0.7, 0.12, -0.3)], 41))
    for y in (10, 60, 110):
        c.line("piston", (0, y), (127, y), (60, 64, 70), 0.5)
    scratches(c, "piston", 42, 18, (40, 44, 50), 0.35, (20, 60))

    def rubber(x, y, w, h):
        knurl = ((x + y) // 6 + (x - y) // 6) % 2
        n = fbm(x, y, 16, 51)
        return scale((34, 34, 36), 0.7 + 0.35 * knurl + 0.25 * n)

    c.paint("grip", rubber)
    for x in (4, 122):
        c.line("grip", (x, 0), (x, 63), (12, 12, 14), 0.9, 2)

    def tread(x, y, w, h):
        a = ((x + 2 * y) % 24) < 5 and ((x // 12 + y // 12) % 2 == 0)
        b = ((2 * y - x) % 24) < 5 and ((x // 12 + y // 12) % 2 == 1)
        n = fbm(x, y, 18, 61)
        base = scale((92, 96, 100), 0.8 + 0.3 * n)
        if a or b:
            base = mix(base, (176, 180, 184), 0.75)
        if n > 0.66:
            base = mix(base, (110, 82, 58), 0.5)
        return base

    c.paint("peg", tread)

    def foot(x, y, w, h):
        groove = (y % 16) < 4
        n = fbm(x, y, 20, 71)
        col = scale((40, 38, 36), 0.55 if groove else 0.85 + 0.3 * n)
        dust = max(0.0, (y - h * 0.62) / (h * 0.38)) * (0.35 + 0.5 * n)
        return mix(col, (128, 108, 78), dust)

    c.paint("foot", foot)

    c.paint("accent", worn_metal((196, 154, 34), steel, 81))
    x0, y0, x1, y1 = REGIONS["accent"]
    for y in range(96, 128):
        for x in range(128):
            if ((x + y) // 12) % 2 == 0 and fbm(x, y, 7, 82) > 0.3:
                c.blend("accent", x, y, (22, 22, 20), 0.9)
    c.line("accent", (0, 94), (127, 94), (22, 22, 20), 0.9, 2)
    stencil(c, "accent", "04", 34, 22, 6, (24, 24, 22), 83)
    scratches(c, "accent", 84, 40, (156, 156, 150), 0.55)

    def coil(x, y, w, h):
        v = y / h
        k = math.exp(-(((v - 0.3) / 0.12) ** 2)) * 0.9 - math.exp(-(((v - 0.8) / 0.15) ** 2)) * 0.3
        n = fbm(x, y, 14, 92)
        col = scale((48, 50, 54), 0.75 + k + 0.2 * n)
        return mix(col, (104, 62, 40), max(0.0, n - 0.68) * 3)

    c.paint("glow", coil)


def paint_covenant(c):
    def shell(seed, deep, mid, sheen):
        def fn(x, y, w, h):
            n = fbm(x + 40 * math.sin(y / 23.0), y, 46, seed)
            m = fbm(x, y + 30 * math.sin(x / 31.0), 20, seed + 5)
            col = mix(deep, mid, n * 1.25 - 0.1)
            # The iridescent sheen: long soft streaks that follow the noise.
            streak = math.sin((x + 55 * n) / 17.0 + m * 4.0)
            col = mix(col, sheen, max(0.0, streak - 0.55) * 1.6)
            vein = abs(math.sin((y + 26 * m) / 9.5 + x / 60.0))
            if vein < 0.07:
                col = scale(col, 0.55)
            elif vein < 0.14:
                col = mix(col, sheen, 0.25)
            return scale(col, 0.92 + (h32(x, y, seed + 9) - 0.5) * 0.06)

        return fn

    c.paint("body", shell(101, (58, 30, 112), (52, 78, 170), (190, 150, 226)))
    for x in (0, 128, 255):
        c.line("body", (x, 0), (x, 127), (22, 12, 52), 0.8, 2)
    for cx, cy in ((64, 30), (192, 30), (64, 98), (192, 98)):
        c.disc("body", cx, cy, 5.5, (20, 12, 46), 0.85)
        c.disc("body", cx, cy, 3.5, (120, 230, 255), 0.95)
        c.disc("body", cx - 1, cy - 1, 1.2, (240, 255, 255), 0.9)

    c.paint("body_dark", shell(111, (26, 20, 62), (44, 40, 118), (128, 104, 196)))
    c.line("body_dark", (0, 4), (127, 4), (150, 126, 214), 0.5, 2)
    c.line("body_dark", (0, 122), (127, 122), (12, 8, 34), 0.8, 2)

    def column(x, y, w, h):
        u = x / w
        core = math.exp(-(((u - 0.5) / 0.2) ** 2))
        flow = 0.75 + 0.25 * math.sin(y / 5.0 + 6 * fbm(x, y, 18, 121))
        return mix((18, 96, 170), (214, 255, 255), core * flow)

    c.paint("piston", column)

    def grip(x, y, w, h):
        ridge = (x % 10) < 3
        n = fbm(x, y, 20, 131)
        return scale((40, 40, 74), (0.6 if ridge else 0.95) + 0.25 * n)

    c.paint("grip", grip)

    c.paint("peg", shell(141, (50, 30, 100), (70, 60, 150), (176, 140, 220)))
    for y in (20, 44):
        c.line("peg", (6, y), (121, y), (16, 10, 40), 0.9, 3)
        c.line("peg", (6, y + 1), (121, y + 1), (110, 236, 255), 0.95)

    def cushion(x, y, w, h):
        v = y / h
        heat = math.exp(-(((v - 0.55) / 0.3) ** 2))
        n = fbm(x, y * 2, 16, 151)
        ripple = 0.8 + 0.2 * math.sin(y / 3.5 + n * 7)
        return mix((14, 60, 132), (190, 255, 255), heat * ripple * (0.7 + 0.5 * n))

    c.paint("foot", cushion)

    c.paint("accent", shell(161, (96, 70, 160), (150, 120, 214), (236, 214, 255)))
    for i in range(5):
        c.disc("accent", 20 + i * 22, 64, 4.5, (30, 16, 60), 0.85)
        c.disc("accent", 20 + i * 22, 64, 2.8, (255, 150, 220) if i % 2 else (120, 236, 255), 0.95)

    def plasma(x, y, w, h):
        n = fbm(x, y, 14, 171)
        return mix((60, 200, 240), (236, 255, 255), 0.35 + 0.8 * n)

    c.paint("glow", plasma)


def engrave(c, region, seed, walks, dark, light, glow=None):
    """Forerunner line work: walks of straight runs with 45 degree turns,
    cut dark with a light lip, some ending in a small square node."""
    x0, y0, x1, y1 = REGIONS[region]
    w, h = x1 - x0, y1 - y0
    r = Rng(seed)
    dirs = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]
    for _ in range(walks):
        x, y = r.range(4, w - 4), r.range(4, h - 4)
        d = r.pick(4) * 2
        for _ in range(3 + r.pick(4)):
            ln = r.range(8, 34) * (1.0 if d % 2 == 0 else 0.5)
            nx = max(3, min(w - 4, x + dirs[d][0] * ln))
            ny = max(3, min(h - 4, y + dirs[d][1] * ln))
            c.line(region, (x, y + 1), (nx, ny + 1), light, 0.5)
            c.line(region, (x, y), (nx, ny), dark, 0.9)
            if glow:
                c.line(region, (x, y), (nx, ny), glow, 0.35)
            x, y = nx, ny
            d = (d + (1 if r.next() < 0.5 else 7)) % 8
        for dy in range(-2, 3):
            for dx in range(-2, 3):
                c.blend(region, int(x) + dx, int(y) + dy, glow or dark, 0.9)


def paint_forerunner(c):
    def alloy(base, seed):
        def fn(x, y, w, h):
            brushed = (h32(x // 5, y, seed) - 0.5) * 0.1
            n = fbm(x, y, 60, seed + 1)
            band = 0.06 * math.sin(x / w * math.pi * 4)
            return scale(base, 0.84 + 0.22 * n + brushed + band)

        return fn

    blue, white = (70, 170, 255), (224, 244, 255)
    c.paint("body", alloy((168, 174, 182), 201))
    seams(c, "body", [0, 85, 170, 255], [10, 118], (70, 76, 86), (222, 228, 234))
    engrave(c, "body", 202, 26, (74, 80, 92), (226, 232, 238))
    for x in (42, 127, 212):
        for y in range(24, 104):
            fall = 1.0 - abs(y - 64) / 44.0
            c.blend("body", x, y, blue, 0.9 * fall)
            c.blend("body", x - 1, y, blue, 0.35 * fall)
            c.blend("body", x + 1, y, blue, 0.35 * fall)
        c.line("body", (x, 40), (x, 88), white, 0.7)

    c.paint("body_dark", alloy((104, 110, 120), 211))
    seams(c, "body_dark", [0, 127], [8, 64, 120], (40, 44, 52), (170, 176, 186))
    engrave(c, "body_dark", 212, 14, (44, 48, 58), (168, 174, 184), blue)

    def core(x, y, w, h):
        u = x / w
        k = math.exp(-(((u - 0.5) / 0.22) ** 2))
        pulse = 0.85 + 0.15 * math.sin(y / 6.0)
        return mix((30, 80, 170), white, k * pulse)

    c.paint("piston", core)

    c.paint("grip", alloy((58, 62, 70), 221))
    for y in (12, 32, 52):
        c.line("grip", (4, y), (123, y), (20, 22, 28), 0.9, 2)
        c.line("grip", (4, y + 2), (123, y + 2), blue, 0.6)

    c.paint("peg", alloy((150, 156, 166), 231))
    seams(c, "peg", [0, 127], [0, 63], (60, 66, 76), (220, 226, 232))
    for y in (6, 56):
        c.line("peg", (6, y), (121, y), blue, 0.85, 2)
    engrave(c, "peg", 232, 6, (70, 76, 88), (220, 226, 232))

    def pad(x, y, w, h):
        col = alloy((160, 166, 176), 241)(x, y, w, h)
        ring = abs(((y / h) * 4) % 1.0 - 0.5)
        if ring < 0.06:
            col = mix(col, blue, 0.9)
        elif ring < 0.12:
            col = scale(col, 0.6)
        return col

    c.paint("foot", pad)

    c.paint("accent", alloy((178, 184, 192), 251))
    for x in range(8, 128, 24):
        for y in range(128):
            fall = 0.55 + 0.45 * math.sin(y / 128.0 * math.pi)
            for dx, a in ((-2, 0.25), (-1, 0.6), (0, 1.0), (1, 0.6), (2, 0.25)):
                c.blend("accent", x + dx, y, blue if dx else white, a * fall)
    engrave(c, "accent", 252, 8, (80, 86, 98), (230, 236, 242))

    def rings(x, y, w, h):
        v = y / h
        k = math.exp(-(((v - 0.5) / 0.28) ** 2))
        return mix((40, 110, 210), white, k * (0.8 + 0.2 * fbm(x, y, 12, 261)))

    c.paint("glow", rings)


STYLES = {"unsc": paint_unsc, "covenant": paint_covenant, "forerunner": paint_forerunner}


def main():
    out = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "content", "pogo", "models")
    check = "--check" in sys.argv[1:]
    bad = 0
    for name, paint in STYLES.items():
        c = Canvas()
        paint(c)
        data = c.png()
        path = os.path.join(out, name + ".png")
        if check:
            with open(path, "rb") as f:
                same = f.read() == data
            print(f"{path}: {'same' if same else 'DIFFERENT'}")
            bad += not same
        else:
            os.makedirs(out, exist_ok=True)
            with open(path, "wb") as f:
                f.write(data)
            print(f"{path}: {len(data)} bytes")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
