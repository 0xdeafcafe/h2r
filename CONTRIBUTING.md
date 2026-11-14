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

## Checks

Run `scripts/check.sh` before you push. It runs `cargo fmt --check`, `cargo
clippy` with warnings treated as errors, and `cargo test --workspace`, and it
stops at the first failure.

- `scripts/check.sh --fix` applies `cargo fmt` and `cargo clippy --fix` first.
- `scripts/check.sh --crate <name>` limits the checks to one package.

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
