"""Read the workspace's manifests without cargo (docs/dev/crate-structure.md).

`packages(root)` returns what `cargo metadata --no-deps` would for the
fields the crate scripts use: name, manifest path, description, features,
dependencies (workspace inheritance resolved), targets by cargo's
auto-discovery rules, and `package.metadata`. It parses the `Cargo.toml`
files with `tomllib`, so it runs in the middle of a move, on a tree cargo
would refuse, and it builds nothing.

`domain_of(pkg, rules(root), root)` is the crate's domain: the directory between
`crates/` and the crate (`crates/<domain>/<crate>`). For a crate that
still sits flat in `crates/`, it is where `scripts/crate-domains.toml`
places it.
"""
import glob
import os
import tomllib

def load(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def member_dirs(root, workspace=None):
    """The member directories of the workspace, as paths relative to root."""
    ws = workspace if workspace is not None else load(os.path.join(root, "Cargo.toml"))["workspace"]
    out = []
    for pattern in ws.get("members", []):
        for hit in sorted(glob.glob(os.path.join(root, pattern))):
            if os.path.isfile(os.path.join(hit, "Cargo.toml")):
                rel = os.path.relpath(hit, root)
                if rel not in out:
                    out.append(rel)
    excluded = {os.path.normpath(e) for e in ws.get("exclude", [])}
    return [m for m in out if m not in excluded]


def _dependencies(manifest, crate_dir, root, ws_deps):
    out = []

    def add(table, kind, target=None):
        for key, spec in (table or {}).items():
            if isinstance(spec, str):
                spec = {"version": spec}
            optional = bool(spec.get("optional"))
            features = list(spec.get("features", []))
            defaults = spec.get("default-features", True)
            base = crate_dir
            if spec.get("workspace"):
                inherited = ws_deps.get(key, {})
                if isinstance(inherited, str):
                    inherited = {"version": inherited}
                features = list(inherited.get("features", [])) + features
                defaults = inherited.get("default-features", True) and defaults
                spec = {**inherited, **{k: v for k, v in spec.items() if k != "workspace"}}
                base = root
            path = spec.get("path")
            out.append({
                "name": spec.get("package", key),
                "rename": key if "package" in spec else None,
                "kind": kind,
                "optional": optional,
                "features": features,
                "uses_default_features": defaults,
                "target": target,
                "path": os.path.normpath(os.path.join(base, path)) if path else None,
            })

    for section, kind in (("dependencies", None), ("dev-dependencies", "dev"), ("build-dependencies", "build")):
        add(manifest.get(section), kind)
        for target, tables in (manifest.get("target") or {}).items():
            add(tables.get(section), kind, target)
    return out


def _targets(manifest, crate_dir, name):
    """Cargo's targets: declared ones, then auto-discovered ones."""
    pkg = manifest.get("package", {})
    out = []
    lib = manifest.get("lib", {})
    lib_path = os.path.join(crate_dir, lib.get("path", "src/lib.rs"))
    if os.path.isfile(lib_path):
        kinds = lib.get("crate-type") or (["proc-macro"] if lib.get("proc-macro") else ["lib"])
        out.append({"name": lib.get("name", name.replace("-", "_")), "kind": list(kinds)})

    def discovered(sub, kind, enabled, declared):
        names = {t.get("name") for t in declared}
        paths = {os.path.normpath(t["path"]) for t in declared if "path" in t}
        for t in declared:
            out.append({"name": t.get("name"), "kind": [kind]})
        if not enabled:
            return
        d = os.path.join(crate_dir, sub)
        if not os.path.isdir(d):
            return
        for entry in sorted(os.listdir(d)):
            full = os.path.join(d, entry)
            if entry.endswith(".rs") and os.path.isfile(full):
                target, rel = entry[:-3], os.path.join(sub, entry)
            elif os.path.isfile(os.path.join(full, "main.rs")):
                target, rel = entry, os.path.join(sub, entry, "main.rs")
            else:
                continue
            if target not in names and os.path.normpath(rel) not in paths:
                out.append({"name": target, "kind": [kind]})

    bins = manifest.get("bin", [])
    if os.path.isfile(os.path.join(crate_dir, "src/main.rs")) and pkg.get("autobins", True):
        if not any(os.path.normpath(b.get("path", "")) == "src/main.rs" or b.get("name") == name for b in bins):
            out.append({"name": name, "kind": ["bin"]})
    discovered("src/bin", "bin", pkg.get("autobins", True), bins)
    discovered("tests", "test", pkg.get("autotests", True), manifest.get("test", []))
    discovered("examples", "example", pkg.get("autoexamples", True), manifest.get("example", []))
    discovered("benches", "bench", pkg.get("autobenches", True), manifest.get("bench", []))
    return out


def packages(root):
    """{package name: package} for every workspace member."""
    top = load(os.path.join(root, "Cargo.toml"))
    ws = top.get("workspace", {})
    ws_deps = ws.get("dependencies", {})
    out, seen, queue = {}, set(), list(member_dirs(root, ws))
    while queue:
        rel = queue.pop(0)
        if rel in seen:
            continue
        seen.add(rel)
        crate_dir = os.path.join(root, rel)
        manifest = load(os.path.join(crate_dir, "Cargo.toml"))
        pkg = manifest.get("package")
        if not pkg:
            continue
        name = pkg["name"]
        description = pkg.get("description")
        out[name] = {
            "name": name,
            "dir": rel,
            "manifest_path": os.path.join(crate_dir, "Cargo.toml"),
            "description": description if isinstance(description, str) else None,
            "features": manifest.get("features", {}),
            "dependencies": _dependencies(manifest, crate_dir, root, ws_deps),
            "targets": _targets(manifest, crate_dir, name),
            "metadata": pkg.get("metadata"),
        }
        # A path dependency inside the workspace root is a member too
        # (`crates/runtime/runtime/test-probe` is matched by no glob).
        for dep in out[name]["dependencies"]:
            if dep["path"] and os.path.isfile(os.path.join(dep["path"], "Cargo.toml")):
                dep_rel = os.path.relpath(dep["path"], root)
                if not dep_rel.startswith(".."):
                    queue.append(dep_rel)
    return out


RULES = os.path.join("scripts", "crate-domains.toml")


def rules(root):
    """scripts/crate-domains.toml: {"domains": {name: table}, "exceptions": [...]}.
    Domain order is the file's, lowest first."""
    table = load(os.path.join(root, RULES))
    exceptions = table.pop("exception", [])
    return {"domains": table, "exceptions": exceptions}


def planned_domains(rule_set):
    """{crate name: domain} as crate-domains.toml places them."""
    return {crate: name for name, d in rule_set["domains"].items() for crate in d.get("crates", [])}


def domain_of(pkg, rule_set, root):
    """(domain, placed). placed is False while the crate sits outside a
    domain directory; the domain is then the planned one, or None. A
    domain directory holds crates and is not one (`crates/runtime/runtime` is a
    crate until the move, and a domain after it)."""
    parts = pkg["dir"].split(os.sep)
    if (len(parts) >= 3 and parts[0] == "crates" and parts[1] in rule_set["domains"]
            and not os.path.isfile(os.path.join(root, "crates", parts[1], "Cargo.toml"))):
        return parts[1], True
    return planned_domains(rule_set).get(pkg["name"]), False
