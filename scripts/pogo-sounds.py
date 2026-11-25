#!/usr/bin/env python3
"""Write the Pogo Stick's sounds (content/pogo/README.md, "Sounds").

Usage: python3 -I scripts/pogo-sounds.py [OUT_DIR] [--check]

Three clips, synthesised here from nothing but arithmetic: no recording, no
sample and nothing from any game goes in. Each is 44.1 kHz mono 16-bit PCM.
The output is the same on every run (a fixed seed and no clock), so
`--check` can compare OUT_DIR's files with what the script would write and
exit 1 on a difference.

  compress.wav  the piston going in on a touchdown: a metal clank and the
                air being pushed out of the cylinder
  spring.wav    the piston springing out on a bounce: a low thump, a coil
                twang falling in pitch, and a short breath of air
  settle.wav    the piston easing back out after a landing with no bounce:
                a soft hiss and a tick as it seats

OUT_DIR defaults to content/pogo/sounds beside this script's repository.
"""
import math
import os
import struct
import sys

RATE = 44100
# Each clip's peak, as a share of full scale. `pogo_sound::PEAK` holds the same
# number, and the game plays each clip at `pogo_sound::level_mb` below it.
PEAK = 0.7


class Noise:
    """A small fixed generator, so the hiss is the same on every machine."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def next(self):
        # xorshift32
        x = self.state
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        self.state = x
        return x / 0x7FFFFFFF - 1.0


def seconds(n):
    return int(n * RATE)


def envelope(t, attack, decay):
    """0 to 1 over `attack`, then an exponential fall with time constant `decay`."""
    if t < 0:
        return 0.0
    rise = min(1.0, t / attack) if attack > 0 else 1.0
    return rise * math.exp(-max(0.0, t - attack) / decay)


def one_pole(samples, cutoff, high=False):
    """A one-pole low-pass (or its complement, a high-pass)."""
    a = math.exp(-2.0 * math.pi * cutoff / RATE)
    out, y = [], 0.0
    for x in samples:
        y = (1.0 - a) * x + a * y
        out.append(x - y if high else y)
    return out


def band(samples, low, high):
    return one_pole(one_pole(samples, low, high=True), high)


def hiss(noise, length, low, high, attack, decay, start=0.0):
    raw = [noise.next() for _ in range(length)]
    shaped = band(raw, low, high)
    return [s * envelope(i / RATE - start, attack, decay) for i, s in enumerate(shaped)]


def partials(length, tones, start=0.0):
    """Decaying sine partials: (frequency, level, decay) each, struck at `start`."""
    out = [0.0] * length
    for freq, level, decay in tones:
        for i in range(length):
            t = i / RATE - start
            if t >= 0:
                out[i] += level * math.sin(2.0 * math.pi * freq * t) * envelope(t, 0.0005, decay)
    return out


def sweep(length, f0, f1, glide, level, decay, wobble=0.0, wobble_hz=0.0):
    """A tone gliding from f0 to f1 over `glide` seconds, with a pitch wobble."""
    out, phase = [], 0.0
    for i in range(length):
        t = i / RATE
        k = min(1.0, t / glide)
        f = f0 + (f1 - f0) * (1.0 - (1.0 - k) ** 2)
        f *= 1.0 + wobble * math.sin(2.0 * math.pi * wobble_hz * t) * math.exp(-t / 0.12)
        phase += 2.0 * math.pi * f / RATE
        # A little second harmonic for the coil's ring.
        s = math.sin(phase) + 0.35 * math.sin(2.0 * phase + 0.6)
        out.append(level * s * envelope(t, 0.002, decay))
    return out


def mix(*tracks):
    length = max(len(t) for t in tracks)
    out = [0.0] * length
    for t in tracks:
        for i, s in enumerate(t):
            out[i] += s
    return out


def finish(samples):
    """Fades the last 10 ms to silence and scales the peak to PEAK."""
    fade = seconds(0.010)
    n = len(samples)
    for i in range(max(0, n - fade), n):
        samples[i] *= (n - i) / fade
    top = max(abs(s) for s in samples) or 1.0
    return [s * PEAK / top for s in samples]


def compress():
    noise = Noise(0x504F474F)
    n = seconds(0.28)
    clank = partials(n, [(412.0, 0.9, 0.030), (1130.0, 0.5, 0.022), (2710.0, 0.3, 0.012), (148.0, 0.8, 0.045)])
    air = hiss(noise, n, 900.0, 5200.0, 0.012, 0.070, start=0.008)
    click = hiss(noise, n, 2500.0, 9000.0, 0.0004, 0.004)
    return finish(mix(clank, [0.55 * s for s in air], [0.6 * s for s in click]))


def spring():
    noise = Noise(0x424F494E)
    n = seconds(0.42)
    thump = sweep(n, 190.0, 70.0, 0.06, 0.9, 0.050)
    twang = sweep(n, 520.0, 260.0, 0.22, 0.55, 0.110, wobble=0.06, wobble_hz=31.0)
    ring = partials(n, [(1870.0, 0.16, 0.060), (3140.0, 0.09, 0.035)])
    air = hiss(noise, n, 1200.0, 7000.0, 0.004, 0.045, start=0.004)
    return finish(mix(thump, twang, ring, [0.45 * s for s in air]))


def settle():
    noise = Noise(0x53455454)
    n = seconds(0.36)
    air = hiss(noise, n, 700.0, 3800.0, 0.060, 0.090)
    tick = partials(n, [(980.0, 0.35, 0.010), (2300.0, 0.2, 0.006)], start=0.24)
    return finish(mix([0.8 * s for s in air], tick))


CLIPS = {"compress": compress, "spring": spring, "settle": settle}


def wav(samples):
    data = b"".join(struct.pack("<h", int(round(max(-1.0, min(1.0, s)) * 32767))) for s in samples)
    fmt = struct.pack("<HHIIHH", 1, 1, RATE, RATE * 2, 2, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(body)) + body


def main(argv):
    check = "--check" in argv
    args = [a for a in argv if a != "--check"]
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = args[0] if args else os.path.join(here, "content", "pogo", "sounds")
    same = True
    for name, make in CLIPS.items():
        path = os.path.join(out, name + ".wav")
        bytes_ = wav(make())
        if check:
            try:
                with open(path, "rb") as f:
                    if f.read() != bytes_:
                        print(f"{path}: differs")
                        same = False
            except OSError as e:
                print(f"{path}: {e}")
                same = False
        else:
            os.makedirs(out, exist_ok=True)
            with open(path, "wb") as f:
                f.write(bytes_)
            print(f"{path}: {len(bytes_)} bytes")
    if check:
        print("same" if same else "different")
    return 0 if same else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
