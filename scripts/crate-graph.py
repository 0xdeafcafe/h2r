#!/usr/bin/env python3
"""Measure the workspace's crate graph (docs/dev/crate-structure.md).

    scripts/crate-graph.py sizes
        One row per crate: lines of Rust, files, internal dependencies out
        and in, the rebuild cone, integration test binaries.
    scripts/crate-graph.py closure <package> [--features a,b] [--default]
        The workspace crates a build of <package> compiles with those
        features: `closure cli --features host` is the host-only build.
    scripts/crate-graph.py modules <crate>/src [--closed a,b,c]
        The module graph inside one crate: for each top-level module, the
        other top-level modules it names through `crate::`. With --closed,
        list every reference that leaves the given set and exit 1 if there
        is one: the readiness check before that set moves to its own crate.
    scripts/crate-graph.py layers [--check]
        Each crate's `package.metadata.h2.layer`, and every dependency that
        points from a lower layer to a higher one. --check exits 1 on any.

The rebuild cone of a crate is the crate plus everything that depends on
it, in lines: what an edit to it recompiles. Reads `cargo metadata
--offline --no-deps` and the source files; nothing is built.
"""
import collections
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Lowest first; the same list as scripts/crate-table.py. `tools` (offline
# converters, editor cores, the profiler) and `live` (the service) are side
# stacks: they may use anything up to `render`, and only `binaries` and
# their own stack may use them.
LAYERS = ["formats", "recompiler", "runtime", "hooks", "subsystems", "render", "product", "binaries", "tools", "live"]
SIDE = {"tools", "live"}


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    return {p["name"]: p for p in json.loads(out)["packages"]}


def count(path):
    with open(path, errors="replace") as f:
        return sum(1 for _ in f)


def is_test(rel):
    return (
        rel.startswith("tests" + os.sep)
        or (os.sep + "tests" + os.sep) in rel
        or rel.endswith("tests.rs")
    )


def crate_lines(pkg):
    """(all lines, test lines, files, src lines) of a package's Rust."""
    root = os.path.dirname(pkg["manifest_path"])
    total = tests = files = src = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d != "target" and not os.path.exists(os.path.join(dirpath, d, "Cargo.toml"))]
        for name in filenames:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, root)
            n = count(path)
            total += n
            files += 1
            if is_test(rel):
                tests += n
            if rel.startswith("src" + os.sep):
                src += n
    return total, tests, files, src


def internal_deps(pkgs):
    deps = {n: set() for n in pkgs}
    for n, p in pkgs.items():
        for d in p["dependencies"]:
            if d["name"] in pkgs and d.get("path") and d["kind"] is None:
                deps[n].add(d["name"])
    return deps


def reach(start, edges):
    seen, stack = {start}, [start]
    while stack:
        for y in edges[stack.pop()]:
            if y not in seen:
                seen.add(y)
                stack.append(y)
    return seen


def sizes():
    pkgs = metadata()
    deps = internal_deps(pkgs)
    rdeps = collections.defaultdict(set)
    for n, ds in deps.items():
        for d in ds:
            rdeps[d].add(n)
    lines = {n: crate_lines(p) for n, p in pkgs.items()}
    all_src = sum(l[3] for l in lines.values())
    print(f"{len(pkgs)} crates, {sum(l[0] for l in lines.values()):,} lines of Rust "
          f"({all_src:,} under src/), {sum(sum(1 for t in p['targets'] if t['kind'][0] == 'test') for p in pkgs.values())} integration test binaries\n")
    print("| crate | lines | of which tests | files | deps out | deps in | rebuild cone | test binaries |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|")
    for n in sorted(pkgs, key=lambda n: -lines[n][0]):
        total, tests, files, _ = lines[n]
        cone = sum(lines[x][3] for x in reach(n, rdeps))
        bins = sum(1 for t in pkgs[n]["targets"] if t["kind"][0] == "test")
        print(f"| `{n}` | {total:,} | {tests:,} | {files} | {len(deps[n])} | {len(rdeps[n])} | {100 * cone / all_src:.0f}% | {bins} |")
    return 0


def closure(args):
    if not args:
        sys.exit(__doc__)
    root, feats, default = args[0], set(), "--default" in args
    if "--features" in args:
        feats = set(f for f in args[args.index("--features") + 1].split(",") if f)
    pkgs = metadata()
    enabled = collections.defaultdict(set)
    active, external = set(), set()
    work = [(root, feats | ({"default"} if default else set()))]
    while work:
        name, wanted = work.pop()
        p = pkgs[name]
        first, fresh, stack = name not in active, False, list(wanted)
        active.add(name)
        while stack:
            f = stack.pop()
            if f in enabled[name]:
                continue
            if f not in p["features"] and not f.startswith("dep:"):
                if not any(d["name"] == f and d["optional"] for d in p["dependencies"]):
                    continue
                enabled[name].add("dep:" + f)
            enabled[name].add(f)
            fresh = True
            stack.extend(p["features"].get(f, []))
        if not (fresh or first):
            continue
        on = enabled[name]
        for d in p["dependencies"]:
            dn = d["name"]
            if d["kind"] in ("dev", "build"):
                continue
            used = (not d["optional"] or "dep:" + dn in on or dn in on
                    or any(e.startswith(dn + "/") for e in on))
            if not used:
                continue
            if dn not in pkgs or not d.get("path"):
                external.add(dn)
                continue
            df = set(d["features"]) | ({"default"} if d["uses_default_features"] else set())
            df |= {e.split("/", 1)[1] for e in on if e.startswith((dn + "/", dn + "?/"))}
            work.append((dn, df))
    src = {n: crate_lines(pkgs[n])[3] for n in active}
    everything = sum(crate_lines(p)[3] for p in pkgs.values())
    print(f"{root} [{','.join(sorted(feats)) or '-'}{' +default' if default else ''}]: "
          f"{len(active)} of {len(pkgs)} workspace crates, {sum(src.values()):,} of {everything:,} src lines "
          f"({100 * sum(src.values()) / everything:.0f}%), {len(external)} direct third-party crates")
    for n in sorted(active, key=lambda n: -src[n]):
        on = ",".join(sorted(f for f in enabled[n] if not f.startswith("dep:")))
        print(f"  {src[n]:8,}  {n}" + (f"  [{on}]" if on else ""))
    return 0


def modules(args):
    if not args:
        sys.exit(__doc__)
    src = args[0] if os.path.isabs(args[0]) else os.path.join(ROOT, args[0])
    closed = set(args[args.index("--closed") + 1].split(",")) if "--closed" in args else None
    files = collections.defaultdict(list)
    for entry in sorted(os.listdir(src)):
        path = os.path.join(src, entry)
        if entry.endswith(".rs"):
            files[entry[:-3]].append(path)
        elif os.path.isdir(path):
            for dirpath, _, names in os.walk(path):
                files[entry] += [os.path.join(dirpath, n) for n in names if n.endswith(".rs")]
    names = set(files)
    deps, lines, sites = {}, {}, collections.defaultdict(list)
    for mod, paths in files.items():
        found, n = set(), 0
        for path in paths:
            for i, line in enumerate(open(path, errors="replace"), 1):
                n += 1
                hit = set(re.findall(r"\bcrate::(\w+)", line))
                for group in re.findall(r"\bcrate::\{([^}]*)\}", line):
                    hit |= set(re.findall(r"(?:^|,)\s*(\w+)", group))
                for other in hit & names - {mod}:
                    found.add(other)
                    sites[(mod, other)].append(f"{os.path.relpath(path, ROOT)}:{i}")
        deps[mod], lines[mod] = found, n
    if closed is not None:
        unknown = closed - names
        if unknown:
            sys.exit(f"not top-level modules of {args[0]}: {sorted(unknown)}")
        leaving = sorted((m, o) for m in closed for o in deps[m] if o not in closed)
        total = sum(len(sites[e]) for e in leaving)
        print(f"{len(closed)} modules, {sum(lines[m] for m in closed):,} lines; "
              f"{len(leaving)} module edges ({total} references) leave the set")
        for m, o in leaving:
            print(f"  {m} -> {o}: {' '.join(sites[(m, o)][:6])}" + (" ..." if len(sites[(m, o)]) > 6 else ""))
        return 1 if leaving else 0
    used_by = collections.Counter(o for ds in deps.values() for o in ds)
    for mod in sorted(names, key=lambda m: -lines[m]):
        print(f"{mod:32} {lines[mod]:7,} lines  used by {used_by[mod]:3}  uses: {' '.join(sorted(deps[mod])) or '-'}")
    return 0


def allowed(user, dep, rank):
    """Whether a crate in layer `user` may depend on one in layer `dep`."""
    if dep in SIDE:
        return user == "binaries" or user == dep
    if user in SIDE:
        return rank[dep] <= rank["render"]
    return rank[dep] <= rank[user]


def layers(args):
    pkgs = metadata()
    deps = internal_deps(pkgs)
    layer = {n: (((p.get("metadata") or {}).get("h2")) or {}).get("layer") for n, p in pkgs.items()}
    rank = {l: i for i, l in enumerate(LAYERS)}
    missing = sorted(n for n, l in layer.items() if l not in rank)
    if len(missing) == len(pkgs):
        print("no crate declares `package.metadata.h2.layer` yet (docs/dev/crate-structure.md, step A1)")
        return 0
    bad = []
    for n, ds in sorted(deps.items()):
        for d in sorted(ds):
            a, b = layer.get(n), layer.get(d)
            if a in rank and b in rank and a != b and not allowed(a, b, rank):
                bad.append(f"{n} ({a}) depends on {d} ({b})")
    for l in LAYERS:
        members = sorted(n for n in pkgs if layer[n] == l)
        if members:
            print(f"{l:11} {' '.join(members)}")
    for n in missing:
        print(f"crate-graph: {n}: no layer", file=sys.stderr)
    for b in bad:
        print(f"crate-graph: upward dependency: {b}", file=sys.stderr)
    return 1 if "--check" in args and (bad or missing) else 0


def main(argv):
    if len(argv) < 2 or argv[1] not in ("sizes", "closure", "modules", "layers"):
        sys.exit(__doc__)
    cmd, rest = argv[1], argv[2:]
    return {"sizes": lambda: sizes(), "closure": lambda: closure(rest),
            "modules": lambda: modules(rest), "layers": lambda: layers(rest)}[cmd]()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
