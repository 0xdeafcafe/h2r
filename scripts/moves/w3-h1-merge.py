#!/usr/bin/env python3
"""Window 3: eleven Halo 1 converter crates become five.

    scripts/moves/w3-h1-merge.py [--list] [--dirty] [--quiet]

    h1bitmaps, h1models, h1sounds, h1sky, h1text  ->  h1-assets (new)
        as modules bitmaps, models, sounds, sky, text
    h1ai, h1scripts                               ->  h1campaign
        as modules ai, scripts
    h1map, h1convert, h1objects                       stay

For a crate S that becomes module M of crate T:

    S/src/lib.rs   -> T/src/M/mod.rs       S/tests/     -> T/tests/M/
    S/src/<rest>   -> T/src/M/<rest>       S/examples/  -> T/examples/M/
    S/Cargo.toml   -> merged into T's, then removed

and in Rust: `crate::` becomes `crate::M::` in the moved sources; `S::`
becomes `crate::M::` inside T and `T::M::` everywhere else. T's manifest
takes the union of the dependencies and one `[[test]]` per moved test
file (`M_<name>`), so every test binary keeps its own root and its
`mod common;`. Every other manifest's `S = { ... }` becomes `T = { ... }`
once. --list prints the `git mv` lines and stops.

Run it on a clean tree at `move-w3-pre`, after window 1. Cargo is not
run: the next cargo command rewrites `Cargo.lock`.
"""
import os
import re
import sys
import tomllib

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta, read, write, tracked, git  # noqa: E402

MERGES = [
    {
        "into": "h1-assets",
        "description": "Halo 1 assets as Halo 2 tags: bitmaps, models, sounds, skies and text, read from Xbox cache files (docs/halo1/README.md)",
        "docs": "Halo 1 assets as Halo 2 tags: bitmaps, models, sounds, skies and text.",
        "parts": [("h1bitmaps", "bitmaps"), ("h1models", "models"), ("h1sky", "sky"), ("h1sounds", "sounds"), ("h1text", "text")],
    },
    {
        "into": "h1campaign",
        "parts": [("h1ai", "ai"), ("h1scripts", "scripts")],
    },
]
SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
KEY = re.compile(r"^([A-Za-z0-9_-]+)\s*(=|\.)")


def sections(text):
    """A manifest as [(header line or None, [lines])], in order."""
    out = [(None, [])]
    for line in text.split("\n"):
        if line.startswith("["):
            out.append((line.strip(), []))
        else:
            out[-1][1].append(line)
    return out


def render(secs):
    return "\n".join(line for header, lines in secs for line in ([header] if header else []) + lines)


def section(secs, name, create=True):
    for header, lines in secs:
        if header == f"[{name}]":
            return lines
    if not create:
        return None
    secs.append((f"[{name}]", []))
    return secs[-1][1]


def key_of(line):
    m = KEY.match(line)
    return m.group(1) if m else None


def append(lines, line):
    """Add a line to a section, before its trailing blank lines."""
    i = len(lines)
    while i and not lines[i - 1].strip():
        i -= 1
    lines.insert(i, line)


def fold_feature_refs(text, part_of):
    """`"dep:S"` and `"S/x"` in feature lists name T; a list that named
    two parts of T names it once."""
    out, seen = [], set()
    for line in text.split("\n"):
        for s, (t, _) in part_of.items():
            for a, b in ((f'"dep:{s}"', f'"dep:{t}"'), (f'"{s}/', f'"{t}/'), (f'"{s}?/', f'"{t}?/')):
                line = line.replace(a, b)
        if "[" in line:
            seen = set()
            m = re.match(r"^(.*=\s*\[)(.*)(\].*)$", line)
            if m:
                items = [i.strip() for i in m.group(2).split(",") if i.strip()]
                line = m.group(1) + ", ".join(dict.fromkeys(items)) + m.group(3)
        elif re.match(r'^\s*"[^"]+",?\s*$', line):
            item = line.strip().rstrip(",")
            if item in seen:
                continue
            seen.add(item)
        out.append(line)
    return "\n".join(out)


def main():
    pkgs = cratemeta.packages(ROOT)
    where = {name: p["dir"] for name, p in pkgs.items()}
    part_of = {}  # S -> (T, M)
    for merge in MERGES:
        for s, m in merge["parts"]:
            part_of[s] = (merge["into"], m)
    present = [s for s in part_of if s in where]
    if not present:
        print("w3: the Halo 1 crates are already merged")
        return 0
    tools = os.path.dirname(where[present[0]])
    if os.path.basename(tools) != "tools":
        sys.exit(f"w3: {where[present[0]]} is not in crates/tools/: run window 1 first")
    dir_of = lambda name: where.get(name, f"{tools}/{name}")

    moves, aliases, report = [], [], []
    for s in present:
        t, m = part_of[s]
        sd, td = dir_of(s), dir_of(t)
        entries = sorted({p[len(sd) + 1:].split("/")[0] for p in tracked(sd)})
        for entry in entries:
            if entry == "src":
                moves.append((f"{sd}/src/lib.rs", f"{td}/src/{m}/mod.rs"))
                moves.append((f"{sd}/src", f"{td}/src/{m}"))
            elif entry in ("tests", "examples", "benches"):
                moves.append((f"{sd}/{entry}", f"{td}/{entry}/{m}"))
            elif entry != "Cargo.toml":
                moves.append((f"{sd}/{entry}", f"{td}/{m}-{entry}"))
        aliases.append((sd, td))
    if "--list" in sys.argv:
        for old, new in moves:
            print(f"git mv {old} {new}")
        for s in present:
            print(f"git rm {dir_of(s)}/Cargo.toml")
        return 0

    w = movelib.Window("w3", moves, aliases)
    w.require_clean()

    # Read the manifests that go away, before anything moves.
    gone = {s: read(f"{dir_of(s)}/Cargo.toml") for s in present}
    moved_src = {s: [p for p in tracked(f"{dir_of(s)}/src") if p.endswith(".rs")] for s in present}
    w.mv()

    # T's manifest: the union of the parts' dependencies, their tests.
    for merge in MERGES:
        t, td = merge["into"], dir_of(merge["into"])
        parts = [(s, m) for s, m in merge["parts"] if s in gone]
        if not parts:
            continue
        manifest = f"{td}/Cargo.toml"
        if read(manifest) is None:
            write(manifest, "\n".join([
                "[package]", f'name = "{t}"', f'description = "{merge["description"]}"',
                "edition.workspace = true", "version.workspace = true", "license.workspace = true",
                "", "[dependencies]", "", "[dev-dependencies]", "",
            ]))
            write(f"{td}/src/lib.rs", "\n".join(
                [f"//! {merge['docs']}", "//!",
                 "//! Each module was a crate of its own and keeps its documentation.", "",
                 "#![forbid(unsafe_code)]", ""]
                + [f"pub mod {m};" for _, m in sorted(merge["parts"], key=lambda p: p[1])] + [""]))
            git("add", "--", manifest, f"{td}/src/lib.rs")
        else:
            lib = read(f"{td}/src/lib.rs")
            add = [f"pub mod {m};" for _, m in parts if not re.search(rf"(?m)^\s*(pub\s+)?mod\s+{m}\s*;", lib)]
            if add:
                write(f"{td}/src/lib.rs", lib.rstrip("\n") + "\n\n" + "\n".join(add) + "\n")
        secs = sections(read(manifest))
        own = {s for s, _ in merge["parts"]} | {t}
        for name in SECTIONS:
            lines = section(secs, name, create=False)
            if lines is not None:
                lines[:] = [l for l in lines if key_of(l) not in own]
        targets = []
        for s, m in parts:
            part = tomllib.loads(gone[s])
            for header, lines in sections(gone[s]):
                if header and header.strip("[]") not in SECTIONS + ("package", "features", "test", "example", "bench", "lints"):
                    report.append(f"UNSUPPORTED {s}/Cargo.toml: {header} is not carried into {t}; do it by hand")
                if header is None or header.strip("[]") not in SECTIONS:
                    continue
                dest = section(secs, header.strip("[]"))
                have = {key_of(l): l for l in dest if key_of(l)}
                for line in lines:
                    k = key_of(line)
                    if not k or k in own:
                        continue
                    if k in have:
                        if have[k].split("#")[0].strip() != line.split("#")[0].strip():
                            report.append(f"CONFLICT {t}: `{have[k].strip()}` kept, `{line.strip()}` from {s} dropped")
                        continue
                    append(dest, line)
                    have[k] = line
            if part.get("features"):
                report.append(f"UNSUPPORTED {s}/Cargo.toml: [features] {sorted(part['features'])} are not carried into {t}")
            for kind, sub in (("test", "tests"), ("example", "examples"), ("bench", "benches")):
                declared = part.get(kind, [])
                if part.get("package", {}).get(f"auto{sub}", True):
                    found = sorted(os.path.basename(p)[:-3] for p in tracked(f"{td}/{sub}/{m}")
                                   if p.endswith(".rs") and p.count("/") == f"{td}/{sub}/{m}".count("/") + 1)
                    by_name = {d.get("name"): d for d in declared}
                    declared = [by_name.get(n, {"name": n}) for n in found] + [d for d in declared if d.get("name") not in found]
                for d in declared:
                    block = [f"[[{kind}]]", f'name = "{m}_{d["name"]}"',
                             f'path = "{sub}/{m}/{d.get("path", sub + "/" + d["name"] + ".rs")[len(sub) + 1:]}"']
                    if d.get("required-features"):
                        block.append("required-features = [" + ", ".join(f'"{f}"' for f in d["required-features"]) + "]")
                    if d.get("harness") is False:
                        block.append("harness = false")
                    targets.append("\n".join(block))
        normal = {key_of(l) for l in section(secs, "dependencies") if key_of(l)}
        dev = section(secs, "dev-dependencies", create=False)
        if dev is not None:
            dev[:] = [l for l in dev if key_of(l) not in normal]
        text = render(secs).rstrip("\n") + "\n"
        if targets:
            text += "\n" + "\n\n".join(targets) + "\n"
        write(manifest, text)
        w.changed.add(manifest)
    for s in present:
        git("rm", "-q", "--", f"{dir_of(s)}/Cargo.toml")

    # Every other manifest: `S = ...` becomes `T = ...`, once per section.
    renamed = 0
    for manifest in tracked("Cargo.toml", "*/Cargo.toml"):
        text = read(manifest)
        if not any(s in text for s in present):
            continue
        secs = sections(text)
        for header, lines in secs:
            if not header or "dependencies" not in header:
                continue
            have = {key_of(l) for l in lines if key_of(l)}
            keep = []
            for line in lines:
                k = key_of(line)
                if k in part_of:
                    t = part_of[k][0]
                    renamed += 1
                    if t in have:
                        continue  # T is already a dependency here: fold S into it
                    have.add(t)
                    line = t + line[len(k):]
                keep.append(line)
            lines[:] = keep
        out = fold_feature_refs(render(secs), part_of)
        if out != text:
            write(manifest, out)
            w.changed.add(manifest)
    w.report.append(f"Cargo.toml: {renamed} dependency lines on merged crates renamed or folded")
    w.manifests()

    # Rust. First `crate::` inside each moved source tree, then the names.
    rust = tracked("*.rs")
    scoped = 0
    for s in present:
        t, m = part_of[s]
        for old in moved_src[s]:
            path = w.map.new(old)
            text = read(path)
            out, n = re.subn(r"\bcrate::", f"crate::{m}::", text)
            if n:
                write(path, out)
                scoped += n
    w.report.append(f"Rust: {scoped} `crate::` paths in the moved sources now name their module")
    edits = 0
    for s in present:
        t, m = part_of[s]
        t_ident, td = t.replace("-", "_"), dir_of(t)
        inside = [f for f in rust if f.startswith(f"{td}/src/")]
        edits += movelib.rename_rust_crate(rust, s, f"{t_ident}::{m}", inside, f"crate::{m}")
    w.report.append(f"Rust: {edits} paths through a merged crate's name rewritten")
    w.relative_paths()
    w.mentions()

    rules = read(cratemeta.RULES)
    for s in present:
        rules = re.sub(rf'"{s}",\s*', "", rules)
    write(cratemeta.RULES, rules)
    w.stage()
    w.report += report
    w.report.append("Cargo.lock still names the merged packages: the next cargo command rewrites it; commit that with the window")
    code = w.finish(old_dirs=[dir_of(s) for s in present])

    # Every `mod x;` of the merged crates must still find its file.
    missing = []
    for merge in MERGES:
        td = dir_of(merge["into"])
        for path in tracked(td):
            if not path.endswith(".rs"):
                continue
            text = read(path)
            here, stem = os.path.dirname(path), os.path.basename(path)[:-3]
            lines = text.split("\n")
            for i, line in enumerate(lines):
                m = re.match(r"\s*(?:pub(?:\([a-z: ]+\))?\s+)?mod\s+([a-z0-9_]+)\s*;", line)
                if not m:
                    continue
                attr = re.match(r'\s*#\[path\s*=\s*"([^"]+)"\]', lines[i - 1]) if i else None
                name = m.group(1)
                # Lenient: beside the file (a root or mod.rs) or under its stem.
                options = [f"{here}/{attr.group(1)}"] if attr else [
                    f"{here}/{name}.rs", f"{here}/{name}/mod.rs", f"{here}/{stem}/{name}.rs", f"{here}/{stem}/{name}/mod.rs"]
                if not any(os.path.isfile(os.path.join(ROOT, movelib.norm(o))) for o in options):
                    missing.append(f"{path}:{i + 1}: mod {name}; finds no file")
    for line in missing:
        print(f"w3: PROBLEM: {line}", file=sys.stderr)
    return code or (1 if missing else 0)


if __name__ == "__main__":
    sys.exit(main())
