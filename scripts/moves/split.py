#!/usr/bin/env python3
"""A split window: top-level modules of one crate become another crate.

    scripts/moves/split.py <spec> [--check] [--list] [--force] [--dirty] [--quiet]

<spec> is a file in scripts/moves/splits/ (or its name without .toml):

    from = "hooks"              # the package the modules leave
    to = "sim-bridge"           # the package they join; made if missing
    description = "..."         # for a new package
    direction = "up"            # up: `to` depends on `from`
                                # down: `from` depends on `to` and re-exports it
    modules = ["sim_frame"]     # top-level modules of from/src: m.rs and m/
    tests = ["sim_physics"]     # from/tests/<name>.rs and from/tests/<name>/
    glob_in = ["boot"]          # packages whose root gains `pub use <to>::*;`

The domain of a new package is its entry in scripts/crate-domains.toml.

--check prints the references that stop the move and exits 1 if there are
any; the window refuses to run until there are none (--force overrides):

    up    no module that stays may name a module that leaves
    down  no module that leaves may name a module that stays, or an item
          of the crate root

What runs:

  1. `git mv` of each module file and directory, and of the tests.
  2. `to`: a new package gets `from`'s manifest (dependencies, features)
     under its own name, to be trimmed afterwards, and a lib.rs of
     `pub mod` lines; an existing one gets the `pub mod` lines.
  3. `from`: up, its `mod` lines for the modules go and the rest become
     `pub mod` (the new crate reads them); down, each `mod m;` becomes
     `pub use <to>::m;`, so `crate::m` and `<from>::m` still resolve.
  4. In the moved files (up), `crate::x` for anything that did not move
     with them (a module, a root item, a macro) becomes `<from>::x`; a
     `use crate::{...}` naming both kinds is split in two. Down, a module
     the root re-exports is named by its crate. Elsewhere, `<from>::m` for a moved module becomes
     `<to>::m`, and a crate that now names `<to>` gets the dependency.
  5. Paths, as in every window: Cargo.toml `path` values, relative
     paths, `crates/...` in docs and comments.

It does not make an item `pub` that the other crate now needs, move an
`impl` that the orphan rule refuses, or trim the copied manifest:
`cargo check` after the window lists those, and they are fixed in
following commits. Cargo is not run here.
"""
import os
import re
import sys
import tomllib

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta, read, write, tracked, git  # noqa: E402

MOD_LINE = re.compile(r"^(\s*)(pub(?:\([a-z: ]+\))?\s+)?mod\s+([a-z0-9_]+)\s*;(.*)$")


def top_modules(src):
    """{module: [files]} for the top-level modules under a src directory."""
    out = {}
    for path in tracked(src):
        rest = path[len(src) + 1:]
        if not rest.endswith(".rs"):
            continue
        head = rest.split("/")[0]
        out.setdefault(head[:-3] if "/" not in rest else head, []).append(path)
    return out


def references(files, names):
    """{name: [file:line]} for `crate::name` paths in the files."""
    found = {}
    for path in files:
        for i, line in enumerate((read(path) or "").split("\n"), 1):
            hit = set(re.findall(r"\bcrate::(\w+)", line))
            for group in re.findall(r"\bcrate::\{([^}]*)\}", line):
                hit |= set(re.findall(r"(?:^|,)\s*(\w+)", group))
            for name in hit:
                if names is None or name in names:
                    found.setdefault(name, []).append(f"{path}:{i}")
    return found


def group_names(body):
    """The first identifier of each top-level item of a `{...}` use group."""
    items, depth, cur = [], 0, ""
    for ch in body:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            items.append(cur)
            cur = ""
        else:
            cur += ch
    items.append(cur)
    return [re.match(r"\s*(\w+)", i).group(1) for i in items if i.strip()]


def scope_up(text, leaving, src_ident, mixed, path):
    """`crate::x` -> `<src>::x` in a moved file for every x that did not
    move. A `use crate::{...}` group naming both kinds is split in two;
    any other mixed group is left and listed. Returns (text, edits)."""
    edits = 0
    out, i = [], 0
    group = re.compile(r"(?<![\w$])crate::\{")
    for m in group.finditer(text):
        depth, j = 1, m.end()
        while j < len(text) and depth:
            depth += {"{": 1, "}": -1}.get(text[j], 0)
            j += 1
        body = text[m.end():j - 1]
        names = group_names(body)
        stay = [n for n in names if n not in leaving]
        if not stay:
            continue
        out.append(text[i:m.start()])
        if len(stay) == len(names):
            out.append(f"{src_ident}::{{{body}}}")
            edits += 1
        else:
            line_start = text.rfind("\n", 0, m.start()) + 1
            prefix = text[line_start:m.start()]
            semi = text[j:j + 1] == ";"
            if re.fullmatch(r"\s*(pub(\([\w: ]+\))?\s+)?use\s+", prefix) and semi and "{" not in body:
                items = [x.strip() for x in body.split(",") if x.strip()]
                moved_items = [x for x in items if re.match(r"\w+", x).group(0) in leaving]
                kept = [x for x in items if x not in moved_items]
                out.append("crate::{" + ", ".join(moved_items) + "};\n" + prefix
                           + f"{src_ident}::{{" + ", ".join(kept) + "}")
                edits += 1
            else:
                out.append(text[m.start():j])
                mixed.append(f"{path}:{text.count(chr(10), 0, m.start()) + 1}")
        i = j
    out.append(text[i:])
    text = "".join(out)
    single = re.compile(r"(?<![\w$])crate::(\w+)\b")

    def fix(m):
        nonlocal edits
        if m.group(1) in leaving:
            return m.group(0)
        edits += 1
        return f"{src_ident}::{m.group(1)}"

    return single.sub(fix, text), edits


def split_use_groups(text, src_ident, dst_ident, leaving):
    """`use <src>::{a, moved::X, b};` -> `use <src>::{a, b};` and
    `use <dst>::{moved::X};`; a group of moved modules only is renamed.
    Returns (text, edits, [line numbers left alone])."""
    edits, unresolved, out, i = 0, [], [], 0
    group = re.compile(r"(?<![\w:.$])" + re.escape(src_ident) + r"::\{")
    for m in group.finditer(text):
        if m.start() < i:
            continue
        depth, j = 1, m.end()
        while j < len(text) and depth:
            depth += {"{": 1, "}": -1}.get(text[j], 0)
            j += 1
        body = text[m.end():j - 1]
        items, d, cur = [], 0, ""
        for ch in body:
            d += {"{": 1, "}": -1}.get(ch, 0)
            if ch == "," and d == 0:
                items.append(cur.strip())
                cur = ""
            else:
                cur += ch
        items = [x for x in items + [cur.strip()] if x]
        moved = [x for x in items if re.match(r"\w+", x).group(0) in leaving]
        if not moved:
            continue
        kept = [x for x in items if x not in moved]
        out.append(text[i:m.start()])
        if not kept:
            out.append(f"{dst_ident}::{{{body}}}")
        else:
            line_start = text.rfind("\n", 0, m.start()) + 1
            prefix = text[line_start:m.start()]
            if re.fullmatch(r"\s*(pub(\([\w: ]+\))?\s+)?use\s+", prefix) and text[j:j + 1] == ";":
                out.append(f"{src_ident}::{{" + ", ".join(kept) + "};\n" + prefix
                           + f"{dst_ident}::{{" + ", ".join(moved) + "}")
            else:
                out.append(text[m.start():j])
                unresolved.append(text.count("\n", 0, m.start()) + 1)
                i = j
                continue
        edits += 1
        i = j
    out.append(text[i:])
    return "".join(out), edits, unresolved


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 1:
        sys.exit(__doc__)
    spec_path = args[0] if os.path.isfile(args[0]) else os.path.join(ROOT, "scripts/moves/splits", args[0] + ".toml")
    with open(spec_path, "rb") as f:
        spec = tomllib.load(f)
    name = os.path.basename(spec_path)[:-5]
    src_pkg, dst_pkg, up = spec["from"], spec["to"], spec.get("direction", "up") == "up"
    pkgs = cratemeta.packages(ROOT)
    if src_pkg not in pkgs:
        sys.exit(f"{name}: no package `{src_pkg}` (an earlier window makes it)")
    rule_set = cratemeta.rules(ROOT)
    src_dir = pkgs[src_pkg]["dir"]
    if dst_pkg in pkgs:
        dst_dir = pkgs[dst_pkg]["dir"]
    else:
        domain = cratemeta.planned_domains(rule_set).get(dst_pkg)
        if not domain:
            sys.exit(f"{name}: `{dst_pkg}` has no domain in {cratemeta.RULES}")
        dst_dir = f"crates/{domain}/{dst_pkg}"
    src_ident, dst_ident = src_pkg.replace("-", "_"), dst_pkg.replace("-", "_")

    modules = top_modules(f"{src_dir}/src")
    leaving = [m for m in spec["modules"] if m in modules]
    absent = [m for m in spec["modules"] if m not in modules]
    if not leaving:
        print(f"{name}: none of the modules is in {src_dir}/src: already moved")
        return 0
    staying = sorted(set(modules) - set(leaving) - {"lib", "main"})
    moving_files = [f for m in leaving for f in modules[m]]
    staying_files = [f for m in staying + ["lib", "main"] for f in modules.get(m, [])]

    # Modules an earlier split took out, which the root re-exports:
    # `pub use ui_kit::icons;` makes `crate::icons` mean `ui_kit::icons`.
    src_lib = f"{src_dir}/src/lib.rs" if read(f"{src_dir}/src/lib.rs") is not None else f"{src_dir}/src/main.rs"
    reexported = dict((m.group(2), m.group(1)) for m in re.finditer(r"(?m)^\s*pub use (\w+)::(\w+);", read(src_lib)))

    # What stops the move.
    if up:
        blockers = references(staying_files, set(leaving))
        rule = f"a module that stays in `{src_pkg}` names one that leaves"
    else:
        named = references(moving_files, None)
        blockers = {n: sites for n, sites in named.items() if n not in leaving and n not in reexported}
        rule = f"a module that leaves `{src_pkg}` names one that stays, or a root item"
    edges = sum(len(s) for s in blockers.values())
    lines = sum(len((read(f) or "").split("\n")) for f in moving_files)
    print(f"{name}: {len(leaving)} modules, {len(moving_files)} files, {lines:,} lines: {src_dir} -> {dst_dir}"
          + (f"; not found: {', '.join(absent)}" if absent else ""))
    if blockers:
        print(f"{name}: {edges} references stop the move ({rule}):")
        for n in sorted(blockers, key=lambda n: -len(blockers[n])):
            sites = blockers[n]
            print(f"  {n}: {len(sites)}  {' '.join(sites[:3])}" + (" ..." if len(sites) > 3 else ""))
    if "--check" in sys.argv:
        return 1 if blockers else 0

    moves = []
    for m in leaving:
        if f"{src_dir}/src/{m}.rs" in modules[m]:
            moves.append((f"{src_dir}/src/{m}.rs", f"{dst_dir}/src/{m}.rs"))
        if any(f.startswith(f"{src_dir}/src/{m}/") for f in modules[m]):
            moves.append((f"{src_dir}/src/{m}", f"{dst_dir}/src/{m}"))
    for t in spec.get("tests", []):
        for path in (f"{src_dir}/tests/{t}.rs", f"{src_dir}/tests/{t}"):
            if tracked(path):
                moves.append((path, f"{dst_dir}/tests/{os.path.basename(path)}"))
    if "--list" in sys.argv:
        for old, new in moves:
            print(f"git mv {old} {new}")
        return 0
    if blockers and "--force" not in sys.argv:
        print(f"{name}: not run. Cut those references first, or pass --force to move anyway.", file=sys.stderr)
        return 2

    w = movelib.Window(name, moves)
    w.require_clean()
    w.mv()

    # The destination's manifest and lib.rs.
    manifest = f"{dst_dir}/Cargo.toml"
    lib = f"{dst_dir}/src/lib.rs"
    new_files = []
    if read(manifest) is None:
        text, dropped = movelib.copy_manifest(src_dir, dst_dir, src_pkg, dst_pkg, spec["description"])
        if up:
            text = re.sub(r"(?m)^\[dependencies\]\n", f'[dependencies]\n{src_pkg} = {{ path = "{movelib.rel(src_dir, dst_dir)}" }}\n', text, count=1)
        write(manifest, text.rstrip("\n") + "\n")
        write(lib, f"//! {spec['description']}\n\n")
        new_files += [manifest, lib]
        w.report.append(f"{manifest}: new, a copy of {src_pkg}'s dependencies and features; trim it")
        if dropped:
            w.report.append(f"{manifest}: not copied, because its domain may not use them: {', '.join(dropped)}")
    elif up:
        text = read(manifest)
        if not re.search(rf"(?m)^{re.escape(src_pkg)}\s*[=.]", text):
            text = re.sub(r"(?m)^\[dependencies\]\n", f'[dependencies]\n{src_pkg} = {{ path = "{movelib.rel(src_dir, dst_dir)}" }}\n', text, count=1)
            write(manifest, text)
            w.report.append(f"{manifest}: depends on {src_pkg} now; its other new needs are `cargo check`'s to list")
    lib_text = read(lib)
    kept, carried, pending = [], [], []
    for line in read(src_lib).split("\n"):
        m = MOD_LINE.match(line)
        if line.lstrip().startswith(("#[", "///")) and not m:
            pending.append(line)
            continue
        if m and m.group(3) in leaving:
            carried += pending + [f"pub mod {m.group(3)};"]
            if not up:
                kept += pending + [f"{m.group(1)}pub use {dst_ident}::{m.group(3)};{m.group(4)}"]
        elif m and up and m.group(3) in staying:
            kept += pending + [f"{m.group(1)}pub mod {m.group(3)};{m.group(4)}"]
        else:
            kept += pending + [line]
        pending = []
    write(src_lib, "\n".join(kept + pending))
    write(lib, lib_text.rstrip("\n") + "\n" + "\n".join(carried) + "\n")
    w.changed |= {src_lib, lib}
    if not up:
        text = read(f"{src_dir}/Cargo.toml")
        if not re.search(rf"(?m)^{re.escape(dst_pkg)}\s*[=.]", text):
            text = re.sub(r"(?m)^\[dependencies\]\n", f'[dependencies]\n{dst_pkg} = {{ path = "{movelib.rel(dst_dir, src_dir)}" }}\n', text, count=1)
            write(f"{src_dir}/Cargo.toml", text)

    # Rust paths.
    moved = [w.map.new(f) for f in moving_files]
    scoped = 0
    if up:
        # Up, everything `crate::` named in a moved file that did not move
        # with it (a module, a root item, a macro) is the source crate's.
        mixed = []
        for path in moved:
            text = read(path)
            out, n = scope_up(text, set(leaving), src_ident, mixed, path)
            if n:
                write(path, out)
                scoped += n
        for site in mixed:
            w.report.append(f"left for a person: {site} names moved and staying items in one `crate::{{...}}`")
    elif reexported:
        names = sorted(reexported)
        pattern = re.compile(r"(?<![\w$])crate::(" + "|".join(map(re.escape, names)) + r")\b")
        for path in moved:
            text = read(path)
            out, n = pattern.subn(lambda m: f"{reexported[m.group(1)]}::{m.group(1)}", text)
            if n:
                write(path, out)
                scoped += n
    w.report.append(f"Rust: {scoped} `crate::<module that is elsewhere now>` paths in the moved files name its crate")
    users = 0
    pattern = re.compile(r"(?<![\w:.$])" + re.escape(src_ident) + r"::(" + "|".join(map(re.escape, leaving)) + r")\b")
    needs = {}  # crate directory -> True when its library names the new crate
    for path in tracked("*.rs"):
        if path.startswith(f"{src_dir}/src/") and not up:
            continue  # the re-export keeps these
        text = read(path)
        if text is None or src_ident not in text:
            continue
        text, n, unresolved = split_use_groups(text, src_ident, dst_ident, set(leaving))
        code = n
        for line in unresolved:
            w.report.append(f"left for a person: {path}:{line} names a moved module inside a `{src_ident}::{{...}}` that is not a `use`")
        lines = text.split("\n")
        for i, line in enumerate(lines):
            lines[i], k = pattern.subn(lambda m: f"{dst_ident}::{m.group(1)}", line)
            n += k
            code += k if not line.lstrip().startswith("//") else 0
        if n:
            write(path, "\n".join(lines))
            users += n
        if code:  # a comment that names the module needs no dependency
            crate = movelib.crate_root(path)
            needs[crate] = needs.get(crate, False) or path.startswith(f"{crate}/src/")
    w.report.append(f"Rust: {users} `{src_ident}::<moved module>` paths now say `{dst_ident}::`")
    for crate, in_library in sorted(needs.items()):
        text = read(f"{crate}/Cargo.toml")
        if crate in (dst_dir, "") or text is None or re.search(rf"(?m)^{re.escape(dst_pkg)}\s*[=.]", text):
            continue
        # Tests, benches and examples only: a dev-dependency, which may
        # point back up at a crate that depends on this one.
        table = "dependencies" if in_library else "dev-dependencies"
        if crate == src_dir and up and in_library:
            w.report.append(f"UNRESOLVED {crate}/src names `{dst_ident}::`, and {dst_pkg} depends on {src_pkg}: cut it by hand")
            continue
        line = f'{dst_pkg} = {{ path = "{movelib.rel(dst_dir, crate)}" }}\n'
        if f"[{table}]" in text:
            text = re.sub(rf"(?m)^\[{table}\]\n", f"[{table}]\n{line}", text, count=1)
        else:
            text = text.rstrip("\n") + f"\n\n[{table}]\n{line}"
        write(f"{crate}/Cargo.toml", text)
        w.report.append(f"{crate}/Cargo.toml: {dst_pkg} is a {'dependency' if in_library else 'dev-dependency'} now")
    # A crate above both that should still see the modules at its root.
    for above in spec.get("glob_in", []):
        if above not in pkgs:
            continue
        crate = pkgs[above]["dir"]
        root = read(f"{crate}/src/lib.rs")
        if root is not None and f"pub use {dst_ident}::*;" not in root:
            write(f"{crate}/src/lib.rs", root.rstrip("\n") + f"\npub use {dst_ident}::*;\n")
        text = read(f"{crate}/Cargo.toml")
        if not re.search(rf"(?m)^{re.escape(dst_pkg)}\s*[=.]", text):
            text = re.sub(r"(?m)^\[dependencies\]\n", f'[dependencies]\n{dst_pkg} = {{ path = "{movelib.rel(dst_dir, crate)}" }}\n', text, count=1)
            write(f"{crate}/Cargo.toml", text)
    w.manifests()
    w.relative_paths()
    w.mentions()
    w.stage(*new_files)
    return w.finish()


if __name__ == "__main__":
    sys.exit(main())
