#!/usr/bin/env python3
"""Split window S5: `boot` keeps the composition root, `game` takes the rest.

    scripts/moves/s5-boot-root.py [--check] [--list] [--force] [--dirty] [--quiet]

Every top-level module of `crates/apps/boot/src` except `lib.rs` and
`registration` moves, unchanged, to a new `crates/extensions/game/src`,
with the build script (the campaign cards' hash). `boot` keeps `lib.rs`,
`registration`, tests and examples, depends on `game`, and re-exports it:

    pub use game::*;

so `crate::skulls` in the root, and `boot::tier` in `cli`, `studio` and
the hosts, still resolve. No other crate changes. After this window each
group of `game` (render glue, the Halo 1 runtime, the Live glue, the UI
toolkit) leaves with `scripts/moves/split.py`, without touching the root.

Two parts of the root go with the modules, so what they name resolves in
`game` unchanged:

  - every top-level `pub use` line of the root (`pub use recomp::*;`,
    `pub use live_telemetry as telemetry;`, ...), with the `//` comment
    above it, moves to `game`'s lib.rs: `crate::telemetry` and
    `crate::fallback` in a moved module mean what they meant;
  - `RunOpts`, `RunReport`, `Host`, `VfsReady` and `BetweenSlices`, cut
    from the root by name (doc comments and attributes with them), become
    `game/src/run_opts.rs`, which `game`'s root re-exports.

--check prints what stops the move, and exits 1 if anything does: a
module that names `crate::registration`, or an item the root defines and
keeps (a function, a macro). Those move into a module first; the window
refuses to run until there are none (--force overrides).

What runs: the `git mv`s; `game`'s manifest (a copy of `boot`'s
dependencies and features, without those `extensions` may not use) and a
lib.rs of `pub mod` lines carried from the root with their `#[cfg]`s.
`perf` stays a dependency of `game`, behind its `perf` feature, as an
exception in scripts/crate-domains.toml that the window adds: `sampling`
reaches the profiler, and `agent_glue` answers `profile.sample.*` with it;
`boot`'s features forward to `game`'s; `pub(crate)` becomes `pub` in the
moved files, since the root is another crate now. Cargo is not run.
"""
import os
import re
import sys
import textwrap

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta, read, write, tracked  # noqa: E402
import split  # noqa: E402

KEEP = {"lib", "registration"}
# Dependencies `game` keeps though `extensions` may not use them, and the
# exception the window adds for each.
EXCEPTIONS = {
    "perf": "the sampler is a hook boot installs: sampling.rs reaches the profiler behind `perf`, and agent_glue answers profile.sample.* with it",
}
# Root items that become game/src/run_opts.rs.
RUN_OPTS = ["RunOpts", "RunReport", "Host", "VfsReady", "BetweenSlices"]
DESCRIPTION = "Our additions to the game: menus and settings screens, skulls, variants, the campaign picker, gameplay hooks and the glue to the platform crates"


def item_span(lines, i):
    """(first, end) of the item declared on line i: its doc comments and
    attributes, through its `{ ... }` or its `;`."""
    a = i
    while a > 0 and re.match(r"\s*(///|#\[)", lines[a - 1]):
        a -= 1
    if lines[i].rstrip().endswith(";"):
        return a, i + 1
    depth, seen = 0, False
    for j in range(i, len(lines)):
        for ch in lines[j]:
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
        if seen and depth == 0:
            return a, j + 1
    return a, len(lines)


def cut_root(text):
    """(root without them, [`pub use` lines with their comments],
    {item: text}) for the parts of the root that move with the modules."""
    lines = text.split("\n")
    spans, items = [], {}
    for name in RUN_OPTS:
        for i, line in enumerate(lines):
            if re.match(rf"pub (struct|type|enum) {name}\b", line):
                a, b = item_span(lines, i)
                spans.append((a, b))
                items[name] = "\n".join(lines[a:b])
                break
    uses = []
    for i, line in enumerate(lines):
        if re.match(r"pub use [^;]+;\s*$", line):
            a = i
            while a > 0 and lines[a - 1].startswith("//") and not lines[a - 1].startswith(("///", "//!")):
                a -= 1
            spans.append((a, i + 1))
            uses.append("\n".join(lines[a:i + 1]))
    keep = [l for i, l in enumerate(lines) if not any(a <= i < b for a, b in spans)]
    out = []
    for line in keep:
        if line == "" and len(out) > 1 and out[-1] == "" and out[-2] == "":
            continue
        out.append(line)
    return "\n".join(out), uses, items


def provided(uses, pkgs):
    """The names the carried `pub use` lines give `game`'s root."""
    names = set()
    for use in uses:
        line = use.split("\n")[-1]
        m = re.match(r"pub use ([\w:]+) as (\w+);", line)
        if m:
            names.add(m.group(2))
            continue
        m = re.match(r"pub use ([\w:]+)::\{(.*)\};", line)
        if m:
            names |= {re.split(r"\s+as\s+", n.strip())[-1] for n in m.group(2).split(",") if n.strip()}
            continue
        m = re.match(r"pub use (\w+)::\*;", line)
        if m:
            pkg = next((p for n, p in pkgs.items() if n.replace("-", "_") == m.group(1)), None)
            lib = read(f"{os.path.relpath(pkg['dir'], ROOT)}/src/lib.rs") if pkg else None
            names |= set(re.findall(r"(?m)^pub (?:unsafe )?(?:mod|fn|struct|enum|const|static|type|trait) (\w+)", lib or ""))
            continue
        m = re.match(r"pub use ([\w:]+)::(\w+);", line)
        if m:
            names.add(m.group(2))
    return names


def run_opts_file(items, leaving):
    """The run types as a module of `game`. A module the root named bare
    (`theater::HostView`) is `crate::theater::HostView` here: most of them
    sit behind a feature, where an import would go unused."""
    body = "\n\n".join(items[n] for n in RUN_OPTS if n in items)
    out = []
    for line in body.split("\n"):
        cut = line.find("//")
        code, rest = (line, "") if cut < 0 else (line[:cut], line[cut:])
        for m in leaving:
            code = re.sub(rf"(?<![\w:]){m}::", f"crate::{m}::", code)
        out.append(code + rest)
    return ("//! What a run is given and what it ends with: the options, the host it\n"
            "//! runs in, and the report. `boot`'s `run` and `run_hosted` take them.\n\n"
            "use std::path::PathBuf;\nuse std::sync::Arc;\n\n" + "\n".join(out) + "\n")


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

    lib_path = f"{boot}/src/lib.rs"
    root, uses, root_items = cut_root(read(lib_path))
    declared = {m.group(3) for m in map(split.MOD_LINE.match, root.split("\n")) if m}
    named = split.references(moving_files, None)
    resolved = set(leaving) | provided(uses, pkgs) | set(root_items) | {"run_opts"}
    blockers = {n: s for n, s in named.items() if n not in resolved}
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
    if movelib.tracked(f"{boot}/build.rs"):
        moves.append((f"{boot}/build.rs", f"{game}/build.rs"))
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
    copy, dropped = movelib.copy_manifest(boot, game, "boot", "game", DESCRIPTION, allow=EXCEPTIONS)
    rules_text = read(cratemeta.RULES)
    for dep, why in EXCEPTIONS.items():
        if re.search(rf'(?m)^from = "game"\nto = "{re.escape(dep)}"', rules_text) or not re.search(rf"(?m)^{re.escape(dep)}\s*=", copy):
            continue
        rules_text = rules_text.rstrip("\n") + f'\n\n[[exception]]\nfrom = "game"\nto = "{dep}"\nuntil = "{why}"\n'
        w.report.append(f"{cratemeta.RULES}: exception game -> {dep}")
    write(cratemeta.RULES, rules_text)
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
    for line in root.split("\n"):
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
    write(f"{boot}/src/lib.rs", re.sub(r"\n{3,}", "\n\n", "\n".join(kept + pending)))
    if root_items:
        write(f"{game}/src/run_opts.rs", run_opts_file(root_items, leaving))
        carried += ["pub mod run_opts;"]
    head = ("".join(f"//! {l}\n" for l in textwrap.wrap(DESCRIPTION + ".", 74))
            + "//!\n//! `boot` is the composition root above this crate and re-exports it.\n\n"
            + "\n".join(uses) + "\n")
    if root_items:
        head += "pub use run_opts::{" + ", ".join(sorted(root_items)) + "};\n"
    write(f"{game}/src/lib.rs", head + "\n" + "\n".join(carried) + "\n")

    # `pub(super)` at the top of a module the root declared reached the
    # root, which is another crate now, as `pub(crate)` did.
    opened = upper = 0
    for old in moving_files:
        path = w.map.new(old)
        text = read(path)
        out, n = re.subn(r"\bpub\(crate\)", "pub", text)
        rest = old[len(f"{boot}/src/"):]
        if rest[:-3] in declared or (rest.endswith("/mod.rs") and rest.split("/")[0] in declared and rest.count("/") == 1):
            out, k = re.subn(r"(?m)^pub\(super\)", "pub", out)
            upper += k
        if out != text:
            write(path, out)
            opened += n
    w.report.append(f"Rust: {opened} `pub(crate)` and {upper} top-level `pub(super)` in the moved files are `pub` now")
    w.manifests()
    w.relative_paths()
    w.mentions()
    w.stage(f"{game}/Cargo.toml", f"{game}/src/lib.rs", *([f"{game}/src/run_opts.rs"] if root_items else []))
    return w.finish()


if __name__ == "__main__":
    sys.exit(main())
