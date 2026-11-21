# Contributing

## Building

You need a stable Rust toolchain. Build the workspace with `cargo build
--workspace`. The `h2` command-line tool is available as `cargo h2 <command>`.

## The disc

Game data never goes in the repo. Put your own disc image in `disc/`, which is
git-ignored, or point `H2_DISC` at it. The `h2` commands pick it up from there.
Tests that need the disc are marked `#[ignore]`. Downloaded tool data lives in
`.tools/`, which is git-ignored too.

Do not commit game data or anything derived from it. That includes disc
images, extracted files, XBE or map files, Ghidra projects and exports, and
any output produced from them, even small snippets. If you are unsure whether
something is derived from the disc, leave it out.

Scripted documentation screenshots and gameplay footage are the exception.
Render them from your own disc with a script in `scripts/` and commit the
finished media under `docs/images/` in Git LFS. Run `git lfs install` once.
Keep raw frame sequences, one-off screenshots, debug frames and test captures
out of Git.

### README media

`scripts/readme-video.sh` captures a six-second clip per level from an
existing game image. Pass the image with `--out-image`, match its build's
`H2_OPT` (default 2), and pass your ISO with `--disc` if needed. The
script uses `h2 serve` and `jq` to wait for gameplay, hide the HUD, move
the camera and capture the display through the agent API.

`--fps 30|60|120` selects the capture rate (default 30). The camera moves
at the game's 30 Hz tick rate and the game renders the in-between
frames. `--size 3840x2160` takes a centered 16:9 crop of the 6× render.
`--scale` can increase the game's render scale for a smaller output.
Use `--dest` to keep the raw frames outside the checkout.

`scripts/readme-art.sh` takes the captured frame folders via `--frames`.
It writes `header.webp`, `header@2x.webp` and `gameplay.webp` under
`docs/images/readme/` by default. The embedded gameplay WebP uses 1280×720
at 30 fps. If a level has no clip, the script uses its JPEG from
`scripts/readme-screenshots.sh`. Install `jq` and `webp` with Homebrew and
use a Python 3 environment with Pillow and NumPy for encoding.

`--master <dir>` also writes a `clip-<level>.mp4` for each captured level
and a joined `gameplay.mp4`. FFmpeg with libx265 encodes the source
dimensions and frame rate, including 3840×2160 at 120 fps. Keep the
masters outside Git and commit only the final documentation media
under `docs/images/**` in Git LFS.

## Checks

Run `scripts/check.sh` before you push. It runs `cargo fmt --check`, `cargo
clippy` with warnings treated as errors, and `cargo test --workspace`, and it
stops at the first failure.

- `scripts/check.sh --fix` applies `cargo fmt` and `cargo clippy --fix` first.
- `scripts/check.sh --crate <name>` limits the checks to one package.
- `just check` runs the same script; `just test` runs the workspace tests.

Tests that need the disc are skipped by default. To run them, put the disc in
place and run:

```sh
cargo test -- --ignored
```

## Crates

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for what each crate is for
and how they fit together.

## Commits

Write the subject in the imperative mood, for example "Add tag loader for
bitmaps", and keep it short. If the change needs explaining, add a plain body
after a blank line: a few sentences of prose saying what changed and why.
