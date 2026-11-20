#!/usr/bin/env bash
# Builds the README's 1600x500 and 3200x1000 headers and its 1280x720
# looping gameplay WebP. Uses HUD-less clips from readme-video.sh when
# available, with readme-screenshots.sh JPEGs filling in missing levels.
#
#   scripts/readme-art.sh [--frames <dir>] [--src <dir>] [--dest <dir>] [--header <n>]
#
# Still sources default to docs/images/readme, frames to <src>/frames and
# output to <src>. The header takes the first three levels by default.
# Requires Python 3 with Pillow and NumPy, and img2webp (brew install webp).
# Commit only the output WebPs under docs/images/** in Git LFS.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$root/docs/images/readme" "$@" <<'PY'
"""Build README images from captured clips and fallback JPEGs."""

import argparse
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image, ImageOps


FPS = 30
SIZE = (1280, 720)
LANCZOS = Image.Resampling.LANCZOS
BUDGET = 8_000_000
LEVEL_ORDER = ("oldmombasa", "deltaapproach", "ascension")
FRAME_NAME = re.compile(r"frame-([0-9]+)\.png")


def find_clips(src, frames_dir):
    clips = {}
    for still in sorted(src.glob("*.jpg")):
        stem = still.stem
        name = stem[:-3] if stem.endswith("-4x") else stem
        clips[name] = [still]

    for folder in sorted(frames_dir.iterdir()) if frames_dir.is_dir() else []:
        if not folder.is_dir() or folder.name.startswith("."):
            continue
        frames = []
        for path in folder.iterdir():
            match = FRAME_NAME.fullmatch(path.name)
            if match and path.is_file():
                frames.append((int(match[1]), path))
        if not frames:
            continue
        frames.sort()
        first = frames[0][0]
        if len(frames) < 2 * FPS or any(
            number != first + i for i, (number, _) in enumerate(frames)
        ):
            raise SystemExit(f"{folder}: expected at least 60 consecutive PNG frames")
        clips[folder.name] = [path for _, path in frames]

    names = [name for name in LEVEL_ORDER if name in clips]
    names.extend(sorted(name for name in clips if name not in LEVEL_ORDER))
    return [(name, clips[name]) for name in names]


def load_rgb(path):
    with Image.open(path) as image:
        return image.convert("RGB")


def upper_band(image, still):
    # Stop above the crosshair even for JPEGs with the full HUD. The JPEGs
    # also need the shield and ammo indicators trimmed off the top.
    top = 0.19 if still else 0.08
    return image.crop((0, round(image.height * top), image.width, round(image.height * 0.42)))


def sharpness(path):
    band = upper_band(load_rgb(path), still=False).convert("L")
    band.thumbnail((320, 320), LANCZOS)
    gray = np.asarray(band, dtype=np.float32)
    laplacian = np.diff(gray, n=2, axis=0)[:, 1:-1] + np.diff(gray, n=2, axis=1)[1:-1, :]
    return laplacian.var()


def header_panel(clip, size):
    still = len(clip) == 1
    center = len(clip) // 2
    middle = clip[center - FPS // 2:center + FPS // 2]
    path = clip[0] if still else max(middle, key=sharpness)
    band = upper_band(load_rgb(path), still)
    panel = ImageOps.fit(band, size, method=LANCZOS)
    srgb = np.asarray(panel, dtype=np.float32) / 255
    return np.where(srgb <= 0.04045, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)


def make_headers(clips, scratch):
    width, height = 3200, 1000
    count = len(clips)
    # Evenly space panels with a 30% overlap, including rounding at 2x.
    panel_width = math.ceil(width / (count - 0.30 * (count - 1)))
    stride = (width - panel_width) / (count - 1) if count > 1 else 0
    panels = [header_panel(clip, (panel_width, height)) for clip in clips]
    luminance = [
        max(0.02, np.percentile(panel @ [0.2126, 0.7152, 0.0722], 60))
        for panel in panels
    ]
    target = np.median(luminance)
    canvas = np.empty((height, width, 3), dtype=np.float32)
    end = 0
    for i, (panel, level) in enumerate(zip(panels, luminance)):
        # Partly match midtone exposure, then apply the same mild contrast.
        exposure = np.clip(math.sqrt(target / level), 0.72, 1.38)
        panel *= exposure
        panel -= 0.18
        panel *= 1.06
        panel += 0.18
        np.clip(panel, 0, 1, out=panel)

        start = round(i * stride)
        overlap = end - start
        if overlap:
            phase = np.linspace(0, math.pi, overlap, dtype=np.float32)[None, :, None]
            mix = (1 - np.cos(phase)) / 2
            canvas[:, start:end] *= 1 - mix
            canvas[:, start:end] += panel[:, :overlap] * mix
        canvas[:, end:start + panel_width] = panel[:, overlap:]
        end = start + panel_width

    bottom = np.linspace(0, 1, height, dtype=np.float32)
    gradient = 1 - 0.23 * np.clip((bottom - 0.64) / 0.36, 0, 1) ** 2
    canvas *= gradient[:, None, None]
    srgb = np.where(canvas <= 0.0031308, 12.92 * canvas, 1.055 * canvas ** (1 / 2.4) - 0.055)
    header = Image.fromarray(np.rint(srgb * 255).astype(np.uint8))
    header.save(scratch / "header@2x.webp", quality=82, method=6)
    header.resize((1600, 500), LANCZOS).save(scratch / "header.webp", quality=82, method=6)


def video_frame(path):
    image = load_rgb(path)
    # The source is 4:3. Fit takes a centered 16:9 window before resizing.
    return ImageOps.fit(image, SIZE, method=LANCZOS)


def video_timeline(clips):
    for i, clip in enumerate(clips):
        next_clip = clips[(i + 1) % len(clips)]
        still = video_frame(clip[0]) if len(clip) == 1 else None
        if still is not None:
            # Four seconds of a still between the incoming and outgoing fades.
            yield still, 4 * FPS
            if len(clips) == 1:
                return
        else:
            for path in clip[FPS:-FPS]:
                yield video_frame(path), 1

        # The first second of each clip is used by the previous transition.
        # This includes the first clip, so the final dissolve leads into its
        # motion at the beginning of the loop.
        next_still = video_frame(next_clip[0]) if len(next_clip) == 1 else None
        for offset in range(FPS):
            a = still if still is not None else video_frame(clip[-FPS + offset])
            b = next_still if next_still is not None else video_frame(next_clip[offset])
            yield Image.blend(a, b, (offset + 1) / (FPS + 1)), 1


def make_gameplay(clips, scratch):
    args_path = scratch / "webp-args.txt"
    output = scratch / "gameplay.webp"
    frame_args = []
    ticks = 0
    for i, (frame, count) in enumerate(video_timeline(clips)):
        name = f"{i:06d}.png"
        frame.save(scratch / name, compress_level=1)
        # Round cumulative time to milliseconds so 33/34 ms frames stay at 30 Hz.
        duration = round((ticks + count) * 1000 / FPS) - round(ticks * 1000 / FPS)
        frame_args.extend(("-d", str(duration), name))
        ticks += count

    # Make every frame a keyframe. WebP delta coding can add an alpha
    # channel to otherwise opaque RGB sources.
    for quality in (80, 72, 64, 56):
        options = ["-loop", "0", "-kmax", "1", "-lossy", "-q", str(quality), "-m", "4"]
        args_path.write_text(" ".join(options + frame_args + ["-o", output.name]) + "\n")
        subprocess.run(["img2webp", args_path.name], cwd=scratch, check=True)
        size = output.stat().st_size
        print(f"gameplay.webp quality {quality}: {size:,} bytes", flush=True)
        if size < BUDGET:
            break
    else:
        print(f"gameplay.webp exceeds the {BUDGET:,}-byte target at quality {quality}", file=sys.stderr)

    with Image.open(output) as animation:
        if animation.mode != "RGB":
            raise SystemExit("gameplay.webp has an alpha channel")


def main():
    parser = argparse.ArgumentParser(prog="scripts/readme-art.sh", description=__doc__)
    parser.add_argument("--src", type=Path, default=Path(sys.argv[1]), help="source JPEG folder")
    parser.add_argument("--frames", type=Path, help="clip folders (default: <src>/frames)")
    parser.add_argument("--dest", type=Path, help="output folder (default: <src>)")
    parser.add_argument("--header", type=int, default=3, help="number of header panels (default: 3)")
    args = parser.parse_args(sys.argv[2:])
    if args.header < 1:
        parser.error("--header must be positive")
    if args.frames is not None and not args.frames.is_dir():
        parser.error(f"frame directory does not exist: {args.frames}")
    if shutil.which("img2webp") is None:
        parser.error("img2webp is required (brew install webp)")

    frames_dir = args.frames if args.frames is not None else args.src / "frames"
    dest = args.dest if args.dest is not None else args.src
    clips = find_clips(args.src, frames_dir)
    if not clips:
        parser.error(f"no frame folders in {frames_dir} or JPEGs in {args.src}")

    for name, clip in clips:
        source = f"{len(clip)} frames" if len(clip) > 1 else f"still {clip[0].name}"
        print(f"{name}: {source}", flush=True)

    with tempfile.TemporaryDirectory(prefix="readme-art.", dir=os.environ.get("TMPDIR")) as directory:
        scratch = Path(directory)
        make_headers([clip for _, clip in clips[:args.header]], scratch)
        make_gameplay([clip for _, clip in clips], scratch)
        dest.mkdir(parents=True, exist_ok=True)
        for name in ("header.webp", "header@2x.webp", "gameplay.webp"):
            shutil.move(str(scratch / name), str(dest / name))
            print(f"wrote {dest / name} ({(dest / name).stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
PY
