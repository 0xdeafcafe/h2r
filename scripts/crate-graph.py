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
        Each domain's crates, and every dependency the domain rules in
        scripts/crate-domains.toml refuse. A crate's domain is its
        directory, crates/<domain>/<crate>; while a crate still sits flat
        in crates/ it is the planned one. --check exits 1 on a refused
        dependency, a crate outside its domain directory once any crate
        has moved, a crate no domain claims, or a stale exception.

The rebuild cone of a crate is the crate plus everything that depends on
it, in lines: what an edit to it recompiles. Reads the manifests with
scripts/cratemeta.py and the source files; cargo is not run and nothing
is built.
"""
import collections
import glob
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cratemeta  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def metadata():
    return cratemeta.packages(ROOT)


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


def layers(args):
    pkgs = metadata()
    deps = collections.defaultdict(set)
    for n, p in pkgs.items():
        for d in p["dependencies"]:
            if d["name"] in pkgs and d["path"] and d["kind"] in (None, "build"):
                deps[n].add(d["name"])
    rule_set = cratemeta.rules(ROOT)
    domains = rule_set["domains"]
    planned = cratemeta.planned_domains(rule_set)
    where = {n: cratemeta.domain_of(p, rule_set, ROOT) for n, p in pkgs.items()}
    domain = {n: w[0] for n, w in where.items()}
    moved = any(placed for _, placed in where.values())
    problems = []
    for n in sorted(pkgs):
        d, placed = where[n]
        if d is None:
            problems.append(f"{n}: no domain; add it to a `crates` list in {cratemeta.RULES}")
        elif moved and not placed:
            problems.append(f"{n}: at {pkgs[n]['dir']}, belongs in crates/{d}/")
        elif placed and planned.get(n, d) != d:
            problems.append(f"{n}: in crates/{d}/, but {cratemeta.RULES} places it in {planned[n]}")
    # A crate directory no workspace glob matches is invisible above.
    member_dirs = {p["dir"] for p in pkgs.values()}
    for manifest in sorted(glob.glob(os.path.join(ROOT, "crates", "*", "Cargo.toml"))):
        rel = os.path.relpath(os.path.dirname(manifest), ROOT)
        if moved and rel not in member_dirs:
            name = os.path.basename(rel)
            problems.append(f"{name}: at {rel}, outside the workspace; belongs in crates/{planned.get(name, '<domain>')}/")
    guest_free = {c: name for name, d in domains.items() for c in d.get("guest_free", [])}
    excused = {(e["from"], e["to"]): e for e in rule_set["exceptions"]}
    refused = {}
    for n in sorted(pkgs):
        for d in sorted(deps[n]):
            a, b = domain[n], domain[d]
            if a is None or b is None:
                continue
            if a != b and b not in domains[a].get("uses", []):
                refused[(n, d)] = f"{n} ({a}) depends on {d} ({b}); {a} may use: {', '.join(domains[a].get('uses', [])) or 'nothing'}"
            elif n in guest_free and d not in guest_free and b not in ("formats", "platform"):
                refused[(n, d)] = f"{n} is guest-free and depends on {d} ({b})"
    for name, table in domains.items():
        members = sorted(n for n in pkgs if domain[n] == name)
        if members:
            print(f"{name:11} {' '.join(members)}")
    if not moved:
        print("(no crate sits in a domain directory yet: these are the planned domains)")
    for key, text in sorted(refused.items()):
        if key in excused:
            print(f"excused: {text}; until {excused[key]['until']}")
        else:
            problems.append(f"refused dependency: {text}")
    for (a, b), e in sorted(excused.items()):
        if a in pkgs and b in pkgs and (a, b) not in refused:
            problems.append(f"stale exception: {a} -> {b} is no longer refused; delete it from {cratemeta.RULES}")
    for p in problems:
        print(f"crate-graph: {p}", file=sys.stderr)
    return 1 if "--check" in args and problems else 0


def main(argv):
    if len(argv) < 2 or argv[1] not in ("sizes", "closure", "modules", "layers"):
        sys.exit(__doc__)
    cmd, rest = argv[1], argv[2:]
    return {"sizes": lambda: sizes(), "closure": lambda: closure(rest),
            "modules": lambda: modules(rest), "layers": lambda: layers(rest)}[cmd]()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
