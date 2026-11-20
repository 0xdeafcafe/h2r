#!/usr/bin/env python3
"""Split window S5: `boot` keeps the composition root, `game` takes the rest.

    scripts/moves/s5-boot-root.py [--check] [--list] [--force] [--dirty] [--quiet]

Every top-level module of `crates/apps/boot/src` except `lib.rs` and
`registration` moves, unchanged, to a new `crates/extensions/game/src`.
`boot` keeps `lib.rs`, `registration`, its build script, `c/`, tests and
examples, depends on `game`, and re-exports it:

    pub use game::*;

so `crate::skulls` in the root, and `boot::tier` in `cli`, `studio` and
the hosts, still resolve. No other crate changes. After this window each
group of `game` (render glue, the Halo 1 runtime, the Live glue, the UI
toolkit) leaves with `scripts/moves/split.py`, without touching the root.

--check prints what stops the move, and exits 1 if anything does: a
module that names `crate::registration`, or an item the root defines
(`crate::RunOpts`, a macro). Those move into a module first; the window
refuses to run until there are none (--force overrides).

What runs: the `git mv`s; `game`'s manifest (a copy of `boot`'s
dependencies and features, to be trimmed) and a lib.rs of `pub mod`
lines carried from the root with their `#[cfg]`s; `boot`'s features
forward to `game`'s; `pub(crate)` becomes `pub` in the moved files, since
the root is another crate now. Cargo is not run.
"""
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta, read, write, tracked  # noqa: E402
import split  # noqa: E402

KEEP = {"lib", "registration"}
DESCRIPTION = "Our additions to the game: menus and settings screens, skulls, variants, the campaign picker, gameplay hooks and the glue to the platform crates"


def main():
    pkgs = cratemeta.packages(ROOT)
    if "game" in pkgs:
        print("s5: `game` exists: already split")
        return 0
    if "boot" not in pkgs:
        sys.exit("s5: no package `boot`")
    boot = pkgs["boot"]["dir"]
    domain = cratemeta.planned_domains(cratemeta.rules(ROOT))["game"]
    game = f"crates/{domain}/game"
    modules = split.top_modules(f"{boot}/src")
    leaving = sorted(set(modules) - KEEP)
    moving_files = [f for m in leaving for f in modules[m]]

    named = split.references(moving_files, None)
    blockers = {n: s for n, s in named.items() if n not in leaving}
    lines = sum(len((read(f) or "").split("\n")) for f in moving_files)
    print(f"s5: {len(leaving)} modules, {len(moving_files)} files, {lines:,} lines: {boot}/src -> {game}/src")
    if blockers:
        print(f"s5: {sum(map(len, blockers.values()))} references stop the move (a module names `registration` or a root item):")
        for n in sorted(blockers, key=lambda n: -len(blockers[n])):
            print(f"  {n}: {len(blockers[n])}  {' '.join(blockers[n][:3])}" + (" ..." if len(blockers[n]) > 3 else ""))
    if "--check" in sys.argv:
        return 1 if blockers else 0

    moves = []
    for m in leaving:
        if f"{boot}/src/{m}.rs" in modules[m]:
            moves.append((f"{boot}/src/{m}.rs", f"{game}/src/{m}.rs"))
        if any(f.startswith(f"{boot}/src/{m}/") for f in modules[m]):
            moves.append((f"{boot}/src/{m}", f"{game}/src/{m}"))
    if "--list" in sys.argv:
        for old, new in moves:
            print(f"git mv {old} {new}")
        return 0
    if blockers and "--force" not in sys.argv:
        print("s5: not run. Move those items into modules first, or pass --force.", file=sys.stderr)
        return 2

    w = movelib.Window("s5", moves)
    w.require_clean()
    w.mv()

    # game's manifest: boot's dependencies and features, no targets.
    text = read(f"{boot}/Cargo.toml")
    copy, dropped = movelib.copy_manifest(boot, game, "boot", "game", DESCRIPTION)
    write(f"{game}/Cargo.toml", copy)
    if dropped:
        w.report.append(f"{game}/Cargo.toml: not copied, because `{domain}` may not use them: {', '.join(dropped)}")

    # boot depends on game and forwards every feature but `default`.
    out, in_features = [], False
    for line in text.split("\n"):
        if line.startswith("["):
            in_features = line.strip() == "[features]"
        m = re.match(r"^([a-z0-9_-]+)\s*=\s*\[(.*)\]\s*$", line) if in_features else None
        if m and m.group(1) != "default" and re.search(rf"(?m)^{re.escape(m.group(1))}\s*=", copy):
            items = [i.strip() for i in m.group(2).split(",") if i.strip()]
            line = f'{m.group(1)} = [' + ", ".join(items + [f'"game/{m.group(1)}"']) + "]"
        elif in_features and re.match(r"^[a-z0-9_-]+\s*=\s*\[\s*$", line):
            w.report.append(f"UNSUPPORTED {boot}/Cargo.toml: a feature list over several lines is not forwarded: {line.strip()}")
        out.append(line)
    forwarded = "\n".join(out)
    forwarded = re.sub(r"(?m)^\[dependencies\]\n",
                       f'[dependencies]\ngame = {{ path = "{movelib.rel(game, boot)}", default-features = false }}\n', forwarded, count=1)
    write(f"{boot}/Cargo.toml", forwarded)

    # The mod lines go to game's root, with their attributes.
    kept, carried, pending, placed = [], [], [], False
    for line in read(f"{boot}/src/lib.rs").split("\n"):
        m = split.MOD_LINE.match(line)
        if line.lstrip().startswith(("#[", "///")) and not m:
            pending.append(line)
            continue
        if m and m.group(3) in leaving:
            carried += pending + [f"pub mod {m.group(3)};"]
            if not placed:
                kept.append("pub use game::*;")
                placed = True
        else:
            kept += pending + [line]
        pending = []
    write(f"{boot}/src/lib.rs", "\n".join(kept + pending))
    write(f"{game}/src/lib.rs", f"//! {DESCRIPTION}.\n//!\n//! `boot` is the composition root above this crate and re-exports it.\n\n" + "\n".join(carried) + "\n")

    opened = 0
    for path in [w.map.new(f) for f in moving_files]:
        text = read(path)
        out, n = re.subn(r"\bpub\(crate\)", "pub", text)
        if n:
            write(path, out)
            opened += n
    w.report.append(f"Rust: {opened} `pub(crate)` in the moved files are `pub` now")
    w.manifests()
    w.relative_paths()
    w.mentions()
    w.stage(f"{game}/Cargo.toml", f"{game}/src/lib.rs")
    return w.finish()


if __name__ == "__main__":
    sys.exit(main())
