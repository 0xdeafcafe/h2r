#!/usr/bin/env python3
"""Window S4: `recomp` out of `boot` (docs/dev/crate-structure.md).

    scripts/moves/s4-recomp.py [--check] [--list] [--force] [--dirty] [--quiet]

Build and load leave `crates/apps/boot` for a new `crates/runtime/recomp`:

  - the modules `fallback`, `shim`, `sse`, `x87`, `tier`, `host_coverage`
    and `host_report`, unchanged;
  - `build.rs`, `c/` and `tests/fast_tier.rs`;
  - from `boot/src/lib.rs`, by markers rather than line numbers, so a
    branch's edits inside them move too: the pipeline (the doc comment of
    `pub const BRIDGE` up to the one of `pub struct RunOpts`: stamps, the
    analysis cache, `build*`, `Module`, `load_module`), the `log` module,
    `SHIM_EVENTS`, and the root tests from `fn synthetic_xbe`'s doc
    comment to the end of `mod tests`.

`boot` depends on `recomp`, forwards `desktop-runtime` to it, and its root
says `pub use recomp::*;`, so `crate::fallback`, `crate::log` and
`stamp_path` in `boot`, and `boot::tier` and `boot::build` in `cli`, the
hosts and the tests, still resolve. Its build script keeps only the
campaign cards' hash (`campaign_cards` stays in `boot`).

In what moves, `pub(crate)` becomes `pub` (the root that reads it is
another crate now), as do `stamp_path` and `Module::slim`, which the run
reads, and `host_audit::` is `host_report::`, the half of the host audit
that moves.

--check prints what stops the move and exits 1 if anything does: a moved
module that names a `boot` item with `crate::`, or the extracted root
text naming a `boot` module or root item that stays. The window refuses
to run until there are none (--force overrides).

Every step is skipped when it is done already, so the script runs twice.
Cargo is not run.
"""
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta, read, write  # noqa: E402
import split  # noqa: E402

MODULES = ["fallback", "host_coverage", "host_report", "shim", "sse", "tier", "x87"]
TESTS = ["fast_tier"]
DESCRIPTION = ("Build and load: the XBE lifted, emitted and compiled into one host module, its build stamp, "
               "the module loaded with the boot bridge, and the interpreter for what the module leaves out")
# boot's dependencies the moved code uses, copied with their specs.
DEPS = ["anyhow", "xbe", "xiso", "analyze", "lift", "lifted", "emit", "clif", "ir", "runtime", "x87",
        "iced-x86", "libloading", "rustc-hash", "serde", "serde_json"]
# Those whose comment in boot's manifest is about the moved code.
COMMENTED = {"lifted", "clif"}

report = []


def item_start(lines, i):
    """The first line of the item at line i: its doc comments and attributes."""
    while i > 0 and re.match(r"\s*(///|#\[)", lines[i - 1]):
        i -= 1
    return i


def block_end(lines, i):
    """The line after the `{ ... }` that opens on line i."""
    depth, seen = 0, False
    for j in range(i, len(lines)):
        for ch in lines[j]:
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
        if seen and depth == 0:
            return j + 1
    return None


def find(lines, pattern, start=0):
    rx = re.compile(pattern)
    for i in range(start, len(lines)):
        if rx.match(lines[i]):
            return i
    return None


def extract(lib_text):
    """(boot's lib.rs without the moved parts, {part: text}) or None when a
    marker is missing."""
    lines = lib_text.split("\n")
    parts, cut = {}, []
    bridge = find(lines, r"pub const BRIDGE: &str")
    run_opts = find(lines, r"pub struct RunOpts\b")
    if bridge is None or run_opts is None:
        return None
    a, b = item_start(lines, bridge), item_start(lines, run_opts)
    parts["pipeline"] = lines[a:b]
    cut.append((a, b))
    log = find(lines, r"pub mod log \{")
    if log is not None:
        a, b = item_start(lines, log), block_end(lines, log)
        parts["log"] = lines[a:b]
        cut.append((a, b))
    events = find(lines, r"pub(\(crate\))? static SHIM_EVENTS\b")
    if events is not None:
        a = item_start(lines, events)
        parts["events"] = lines[a:events + 1]
        cut.append((a, events + 1))
    tests = find(lines, r"mod tests \{")
    synthetic = find(lines, r"\s+fn synthetic_xbe\(", tests or 0) if tests is not None else None
    if synthetic is not None:
        a, b = item_start(lines, synthetic), block_end(lines, tests) - 1
        parts["tests"] = lines[a:b]
        cut.append((a, b))
    keep = []
    for i, line in enumerate(lines):
        if not any(a <= i < b for a, b in cut):
            keep.append(line)
    # One blank line where a part left.
    out = []
    for line in keep:
        if line == "" and out and out[-1] == "" and len(out) > 1 and out[-2] == "":
            continue
        out.append(line)
    return "\n".join(out), {k: "\n".join(v).strip("\n") for k, v in parts.items()}


def widen(text):
    text = re.sub(r"\bpub\(crate\)", "pub", text)
    text = re.sub(r"(?m)^fn stamp_path\(", "pub fn stamp_path(", text)
    text = re.sub(r"(?m)^(    )slim: bool,", r"\1pub slim: bool,", text)
    text = re.sub(r"(?<![\w:])((?:crate::)?)host_audit::", r"\1host_report::", text)
    return text


def defined(text):
    return set(re.findall(r"(?m)^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:unsafe\s+)?(?:fn|struct|enum|const|static|type|mod|trait)\s+(\w+)", text))


def blockers(boot, files, parts, rest):
    """What stops the move: names the moved code takes from what stays."""
    out = []
    moved_names = set(MODULES) | {"log"} | defined("\n".join(parts.values()))
    renamed = {"host_audit"}  # widen() names it `host_report`
    for name, hits in split.references(files, None).items():
        if name not in moved_names | renamed:
            out += [f"{h}: crate::{name}" for h in hits]
    staying_mods = {m.group(3) for m in map(split.MOD_LINE.match, rest.split("\n")) if m} - set(MODULES)
    staying_items = defined(re.sub(r"(?ms)^#\[cfg\(test\)\]\nmod tests \{.*", "", rest)) - staying_mods - moved_names
    text = widen("\n".join(v for k, v in parts.items() if k != "tests"))
    for i, line in enumerate(text.split("\n"), 1):
        code = line.split("//")[0]
        for m in staying_mods:
            if re.search(rf"(?<![\w:]){m}::", code):
                out.append(f"{boot}/src/lib.rs (moved text, line {i}): {m}::")
        for n in staying_items:
            if re.search(rf"(?<![\w:.]){n}\b", code) and n not in ("tests",):
                out.append(f"{boot}/src/lib.rs (moved text, line {i}): {n}")
    return out


def recomp_lib(parts):
    mods = "\n".join(f"pub mod {m};" for m in MODULES)
    uses = "\n".join([
        "use std::path::{Path, PathBuf};",
        "use std::sync::Mutex;",
        '#[cfg(feature = "desktop-runtime")]',
        "use std::time::Instant;",
        "",
        "use anyhow::{Context, Result, bail};",
    ])
    head = ("//! Build and load: the XBE lifted, emitted and compiled into one host\n"
            "//! module, its build stamp, the module loaded with the boot bridge, and\n"
            "//! the interpreter for what the module leaves out (`fallback`, `x87`,\n"
            "//! `sse`).\n//!\n"
            "//! `boot` is the composition root above this crate and re-exports it,\n"
            "//! so `boot::build`, `boot::tier` and `boot::load_module` resolve as\n"
            "//! before (docs/runtime/BOOT.md, docs/dev/crate-structure.md).\n\n")
    body = [mods, uses, parts["pipeline"]]
    if "log" in parts:
        body.append(parts["log"])
    if "events" in parts:
        body.append(parts["events"])
    if "tests" in parts:
        body.append("#[cfg(test)]\nmod tests {\n    use super::*;\n\n" + parts["tests"] + "\n}")
    return head + "\n\n".join(body) + "\n"


def recomp_manifest(boot, recomp):
    text = read(f"{boot}/Cargo.toml")
    deps, pending, section = {}, [], ""
    for line in text.split("\n"):
        if line.startswith("["):
            section = line.strip()
            pending = []
            continue
        if section != "[dependencies]":
            continue
        if line.lstrip().startswith("#"):
            pending.append(line)
            continue
        m = re.match(r"^([A-Za-z0-9_-]+)\s*(=|\.)", line)
        if m:
            line = re.sub(r'(\bpath\s*=\s*")([^"]+)(")',
                          lambda p: p.group(1) + movelib.rel(movelib.norm(os.path.join(boot, p.group(2))), recomp) + p.group(3), line)
            deps[m.group(1)] = (pending if m.group(1) in COMMENTED else []) + [line]
        pending = []
    missing = [d for d in DEPS if d not in deps]
    if missing:
        report.append(f"NOT FOUND in {boot}/Cargo.toml: dependencies {', '.join(missing)}")
    body = [l for d in DEPS for l in deps.get(d, [])]
    return "\n".join([
        "[package]",
        'name = "recomp"',
        f'description = "{DESCRIPTION}"',
        "edition.workspace = true",
        "version.workspace = true",
        "license.workspace = true",
        "",
        "[dependencies]",
        *body,
        "",
        "[features]",
        "# The Cranelift fast tier and the clang build (tier.rs, `build`).",
        'desktop-runtime = ["dep:clif"]',
        "",
        "[[test]]",
        'name = "fast_tier"',
        'required-features = ["desktop-runtime"]',
        "",
    ])


def boot_manifest(text, recomp, boot):
    if re.search(r"(?m)^recomp\s*=", text):
        return text
    dep = (f"# Build and load (docs/dev/crate-structure.md, S4).\n"
           f'recomp = {{ path = "{movelib.rel(recomp, boot)}" }}\n')
    text = re.sub(r"(?m)^\[dependencies\]\n", "[dependencies]\n" + dep, text, count=1)
    text, n = re.subn(r'(?m)^(desktop-runtime\s*=\s*\[)(.*)\]\s*$',
                      lambda m: m.group(1) + m.group(2) + ', "recomp/desktop-runtime"]', text, count=1)
    if n != 1:
        report.append(f"NOT FOUND in {boot}/Cargo.toml: a one-line `desktop-runtime = [...]`")
    text, n = re.subn(r'(?m)\n*^\[\[test\]\]\nname = "fast_tier"\nrequired-features = \[[^\]]*\]\n?', "\n", text)
    if n != 1:
        report.append(f"NOT FOUND in {boot}/Cargo.toml: the `fast_tier` [[test]]")
    return text.rstrip("\n") + "\n"


def split_build_script(text):
    """(recomp's build.rs, boot's build.rs): the campaign cards' hash stays."""
    m = re.search(r"(?ms)^    // The card cache holds.*?CAMPAIGN_CARDS_SOURCE_HASH=[^\n]*\n", text)
    if not m:
        report.append("NOT FOUND in build.rs: the campaign cards' hash")
        return text, None
    cards = m.group(0)
    recomp = text[:m.start()] + text[m.end():]
    helpers = re.search(r"(?ms)^use std::path::\{Path, PathBuf\};\n.*?^}\n\n/// Hash every file.*?^}\n", text)
    if not helpers:
        report.append("NOT FOUND in build.rs: `walk` and `hash`")
        return recomp, None
    boot = ("//! The campaign cards' source hash (`CAMPAIGN_CARDS_SOURCE_HASH`, FNV-1a\n"
            "//! over file names and contents), so a cached card is rebuilt whenever\n"
            "//! the rewrite or the encoder it calls changes (campaign_cards/cache.rs).\n"
            "//! The build's own hashes are `recomp`'s build script.\n\n"
            + helpers.group(0) + "\nfn main() {\n" + cards + "}\n")
    return recomp, boot


def main():
    pkgs = cratemeta.packages(ROOT)
    if "recomp" in pkgs:
        print("s4: `recomp` exists: already split")
        return 0
    if "boot" not in pkgs:
        sys.exit("s4: no package `boot`")
    boot = pkgs["boot"]["dir"]
    domain = cratemeta.planned_domains(cratemeta.rules(ROOT))["recomp"]
    recomp = f"crates/{domain}/recomp"
    modules = split.top_modules(f"{boot}/src")
    leaving = [m for m in MODULES if m in modules]
    files = [f for m in leaving for f in modules[m]]
    lib = f"{boot}/src/lib.rs"
    got = extract(read(lib))
    if got is None:
        sys.exit(f"s4: {lib} has no `pub const BRIDGE` or no `pub struct RunOpts`: the markers moved")
    rest, parts = got
    stop = blockers(boot, files, parts, rest)
    lines = sum(len((read(f) or "").split("\n")) for f in files) + sum(len(p.split("\n")) for p in parts.values())
    print(f"s4: {len(leaving)} modules, {len(files)} files and {sum(len(p.split(chr(10))) for p in parts.values())} lines of lib.rs "
          f"({', '.join(parts)}), {lines:,} lines: {boot} -> {recomp}")
    if stop:
        print(f"s4: {len(stop)} references stop the move:")
        for s in stop:
            print(f"  {s}")
    if "--check" in sys.argv:
        return 1 if stop else 0

    moves = [(f"{boot}/src/{m}.rs", f"{recomp}/src/{m}.rs") for m in leaving if f"{boot}/src/{m}.rs" in files]
    moves += [(f"{boot}/src/{m}", f"{recomp}/src/{m}") for m in leaving
              if any(f.startswith(f"{boot}/src/{m}/") for f in modules[m])]
    moves += [(f"{boot}/build.rs", f"{recomp}/build.rs"), (f"{boot}/c", f"{recomp}/c")]
    moves += [(f"{boot}/tests/{t}.rs", f"{recomp}/tests/{t}.rs") for t in TESTS]
    if "--list" in sys.argv:
        for old, new in moves:
            print(f"git mv {old} {new}")
        return 0
    if stop and "--force" not in sys.argv:
        print("s4: not run. Cut those references first, or pass --force.", file=sys.stderr)
        return 2

    w = movelib.Window("s4", moves)
    w.require_clean()
    w.mv()

    # recomp: the manifest, the root, the moved files widened.
    write(f"{recomp}/Cargo.toml", recomp_manifest(boot, recomp))
    write(f"{recomp}/src/lib.rs", widen(recomp_lib(parts)))
    opened = 0
    for path in [w.map.new(f) for f in files]:
        text = read(path)
        out = widen(text)
        if out != text:
            write(path, out)
            opened += 1
    w.report.append(f"Rust: `pub(crate)` is `pub` and `host_audit::` is `host_report::` in {opened} moved files and recomp's lib.rs")
    test = f"{recomp}/tests/fast_tier.rs"
    text = read(test)
    if text is not None:
        write(test, re.sub(r"(?<![\w:])boot::", "recomp::", text))

    # The build script: the cards' hash stays with boot.
    recomp_build, boot_build = split_build_script(read(f"{recomp}/build.rs"))
    write(f"{recomp}/build.rs", recomp_build)
    if boot_build:
        write(f"{boot}/build.rs", boot_build)

    # boot: the root without the moved parts, `pub use recomp::*;` where
    # the first moved `mod` line was, the manifest.
    out, placed = [], False
    for line in rest.split("\n"):
        m = split.MOD_LINE.match(line)
        if m and m.group(3) in leaving:
            continue
        if not placed and (m or line.startswith("pub use ")):
            out += ["// Build and load: `crate::fallback`, `crate::tier`, `stamp_path` and the", "// rest are `recomp`'s.", "pub use recomp::*;"]
            placed = True
        out.append(line)
    text = re.sub(r"\n\n(\}\n?)$", r"\n\1", "\n".join(out))
    write(lib, text)
    write(f"{boot}/Cargo.toml", boot_manifest(read(f"{boot}/Cargo.toml"), recomp, boot))

    # What no rule sees: a test outside both crates that reads the bridge.
    w.literal("crates/recompiler/emit/tests/windows.rs", '"../../apps/boot/c/boot_bridge.c"',
              '"../../runtime/recomp/c/boot_bridge.c"', 1)
    w.report += report
    w.manifests()
    w.relative_paths()
    w.mentions()
    w.stage(f"{recomp}/Cargo.toml", f"{recomp}/src/lib.rs", f"{boot}/build.rs")
    w.report.append("Cargo.lock does not name `recomp` yet: the next cargo command adds it; commit that after the window")
    return w.finish()


if __name__ == "__main__":
    sys.exit(main())
