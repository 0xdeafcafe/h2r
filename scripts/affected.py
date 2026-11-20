#!/usr/bin/env python3
"""Which workspace crates a branch can have changed, and a content key for
each crate (docs/dev/build-speed.md).

    scripts/affected.py                     crates to test for master...HEAD
    scripts/affected.py --base origin/master --why
    scripts/affected.py --format args       -p a -p b ..., for cargo
    scripts/affected.py --keys              every crate's content key

A crate is affected when a changed file is inside it, inside a crate it
depends on (normal, build or dev), or is a file outside the crates that
its sources name (`include_str!("../../../docs/x.md")`, `#[path]` into
another crate's tests). A change under a crate's `tests/`, `benches/` or
`examples/` affects that crate alone, not the crates that depend on it.
`Cargo.lock`, the root `Cargo.toml` and the cargo config affect every
crate. Changed files that map to no crate are listed on stderr, never
dropped quietly.

Reads `cargo metadata --offline --no-deps` and git. Runs no build.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

# A change to any of these can change every crate's build.
GLOBAL_INPUTS = ("Cargo.lock", "Cargo.toml", ".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml")

# Paths a source file names outside its own crate.
_QUOTED = r'"((?:[^"\\]|\\.)*)"'
_OUTSIDE = [
    re.compile(r"\binclude(?:_str|_bytes)?!\(\s*" + _QUOTED),
    re.compile(r"#\[path\s*=\s*" + _QUOTED),
]
# `concat!(env!("CARGO_MANIFEST_DIR"), "/../../x")` and the like.
_MANIFEST_RELATIVE = re.compile(r'CARGO_MANIFEST_DIR[^;\n]*?"(/?\.\./[^"]*)"')


def git(root, *args):
    return subprocess.run(
        ("git", "-C", root) + args, check=True, stdout=subprocess.PIPE, universal_newlines=True
    ).stdout


def repo_root():
    out = subprocess.run(
        ("git", "rev-parse", "--show-toplevel"), check=True, stdout=subprocess.PIPE, universal_newlines=True
    )
    return out.stdout.strip()


class Workspace:
    """The workspace's crates, their directories and their dependencies on
    each other."""

    def __init__(self, root):
        self.root = os.path.realpath(root)
        meta = json.loads(
            subprocess.run(
                ("cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"),
                cwd=self.root,
                check=True,
                stdout=subprocess.PIPE,
                universal_newlines=True,
            ).stdout
        )
        self.dir = {}  # crate -> directory, relative to the root
        by_dir = {}
        for p in meta["packages"]:
            d = os.path.relpath(os.path.dirname(os.path.realpath(p["manifest_path"])), self.root)
            self.dir[p["name"]] = d
            by_dir[d] = p["name"]
        self.deps = {}  # crate -> crates it builds against (normal and build)
        self.dev_deps = {}  # crate -> crates only its tests build against
        for p in meta["packages"]:
            name = p["name"]
            self.deps[name], self.dev_deps[name] = set(), set()
            for dep in p["dependencies"]:
                if not dep.get("path"):
                    continue
                target = by_dir.get(os.path.relpath(os.path.realpath(dep["path"]), self.root))
                if target is None or target == name:
                    continue
                (self.dev_deps if dep.get("kind") == "dev" else self.deps)[name].add(target)
        # Longest directory first, so a crate nested in another's directory
        # (crates/runtime/test-probe) claims its own files.
        self._dirs = sorted(self.dir.items(), key=lambda kv: -len(kv[1]))
        self._outside = None

    def crate_of(self, path):
        """The crate whose directory holds `path` (relative to the root)."""
        for name, d in self._dirs:
            if path == d or path.startswith(d + "/"):
                return name
        return None

    def test_only(self, crate, path):
        """Whether `path`, inside `crate`, is built only for its tests."""
        rel = path[len(self.dir[crate]) + 1 :]
        return rel.split("/", 1)[0] in ("tests", "benches", "examples")

    def outside_inputs(self):
        """Files and directories outside a crate that its sources name:
        path (relative to the root) -> {crate: named only from its tests}."""
        if self._outside is not None:
            return self._outside
        found = {}
        pattern = r"include(_str|_bytes)?!\(|#\[path|CARGO_MANIFEST_DIR"
        listing = subprocess.run(
            ("git", "-C", self.root, "grep", "-n", "-I", "-E", "-e", pattern, "--", "*.rs"),
            stdout=subprocess.PIPE,
            universal_newlines=True,
        )
        if listing.returncode not in (0, 1):
            raise SystemExit("affected.py: git grep failed")
        for line in listing.stdout.splitlines():
            parts = line.split(":", 2)
            if len(parts) != 3:
                continue
            source, _, text = parts
            owner = self.crate_of(source)
            if owner is None:
                continue
            named = []
            for rx in _OUTSIDE:
                named += [(os.path.dirname(source), m) for m in rx.findall(text)]
            named += [(self.dir[owner], m.lstrip("/")) for m in _MANIFEST_RELATIVE.findall(text)]
            for base, rel in named:
                if ".." not in rel:
                    continue
                target = os.path.normpath(os.path.join(base, rel))
                # Not the crate's own files, and not a directory above it
                # (code that only finds the checkout's root).
                if target.startswith("..") or self.crate_of(target) == owner:
                    continue
                if target == "." or (self.dir[owner] + "/").startswith(target + "/"):
                    continue
                readers = found.setdefault(target, {})
                readers[owner] = readers.get(owner, True) and self.test_only(owner, source)
        self._outside = found
        return found

    def closure(self, crate):
        """`crate`, what its tests build against, and all they build
        against in turn."""
        seen = set()
        stack = [crate] + sorted(self.dev_deps[crate])
        while stack:
            c = stack.pop()
            if c in seen:
                continue
            seen.add(c)
            stack += sorted(self.deps[c])
        return seen

    def dependents(self, crates):
        """`crates`, every crate that builds against one of them, and the
        crates whose tests alone do."""
        rev = {c: set() for c in self.dir}
        for c in self.dir:
            for d in self.deps[c]:
                rev[d].add(c)
        seen = set()
        stack = list(crates)
        while stack:
            c = stack.pop()
            if c in seen:
                continue
            seen.add(c)
            stack += sorted(rev[c])
        return seen | {c for c in self.dir if self.dev_deps[c] & seen}

    def order(self, crates):
        """`crates`, dependencies before dependents."""
        depth = {}

        def of(c, trail=()):
            if c in depth:
                return depth[c]
            if c in trail:  # a dev-dependency cycle
                return 0
            depth[c] = 1 + max([of(d, trail + (c,)) for d in self.deps[c]] or [0])
            return depth[c]

        return sorted(crates, key=lambda c: (of(c), c))

    def changed_files(self, base, head="HEAD", worktree=True):
        """Files that differ between the merge base of `base` and `head`,
        and `head`; with `worktree`, uncommitted and untracked files too."""
        files = set(git(self.root, "diff", "--name-only", "--no-renames", base + "..." + head).splitlines())
        if worktree:
            for line in git(self.root, "status", "--porcelain", "--untracked-files=all").splitlines():
                path = line[3:]
                if " -> " in path:
                    files.update(path.split(" -> "))
                else:
                    files.add(path)
        return sorted(f.strip('"') for f in files if f)

    def affected(self, files):
        """(crates to test, why each changed crate is one, files that map
        to no crate). `why` maps a crate to (file, tests only) pairs."""
        why = {}
        unmapped = []
        outside = self.outside_inputs()
        for f in files:
            if f in GLOBAL_INPUTS:
                for c in self.dir:
                    why.setdefault(c, []).append((f, False))
                continue
            owner = self.crate_of(f)
            if owner is not None:
                why.setdefault(owner, []).append((f, self.test_only(owner, f)))
            readers = {}
            for target, crates in outside.items():
                if f == target or f.startswith(target + "/"):
                    for c, tests in crates.items():
                        readers[c] = readers.get(c, True) and tests
            for c, tests in readers.items():
                why.setdefault(c, []).append((f, tests))
            if owner is None and not readers:
                unmapped.append(f)
        built = {c for c, files in why.items() if not all(tests for _, tests in files)}
        return self.dependents(built) | set(why), why, unmapped

    def keys(self, extra=()):
        """A content key per crate at HEAD: its own tree, the trees of every
        crate its tests build against, the outside files those name, and the
        workspace-wide inputs. None for a crate with uncommitted changes
        anywhere in that set. Two checkouts with equal keys build and test
        the same thing, whatever else differs between them."""
        names = sorted(self.dir)
        ids = git(self.root, "rev-parse", *["HEAD:" + self.dir[n] for n in names]).split()
        tree = dict(zip(names, ids))
        # What a dependent sees of a crate: its tree without the test-only
        # directories.
        lib = {n: hashlib.sha256() for n in names}
        listing = git(self.root, "ls-tree", "HEAD", "--", *[self.dir[n] + "/" for n in names])
        for line in listing.splitlines():
            info, path = line.split("\t", 1)
            owner = self.crate_of(path)
            if owner and not self.test_only(owner, path):
                lib[owner].update((info.split()[2] + " " + path + "\n").encode())
        dirty = set()
        dirty_files = []
        for line in git(self.root, "status", "--porcelain", "--untracked-files=all").splitlines():
            path = line[3:].split(" -> ")[-1].strip('"')
            dirty_files.append(path)
        outside = self.outside_inputs()
        globals_dirty = False
        for path in dirty_files:
            owner = self.crate_of(path)
            if owner:
                dirty.add(owner)
            if path in GLOBAL_INPUTS:
                globals_dirty = True
            for target, crates in outside.items():
                if path == target or path.startswith(target + "/"):
                    dirty |= set(crates)
        tracked = set(git(self.root, "ls-tree", "--name-only", "HEAD", "--", *GLOBAL_INPUTS, ".cargo/").splitlines())
        base = hashlib.sha256()
        for g in GLOBAL_INPUTS:
            if g in tracked:
                base.update((g + "=" + git(self.root, "rev-parse", "HEAD:" + g).strip() + "\n").encode())
        for e in extra:
            base.update((e + "\n").encode())
        outside_id = {}
        for target in sorted(outside):
            got = subprocess.run(
                ("git", "-C", self.root, "rev-parse", "--verify", "--quiet", "HEAD:" + target),
                stdout=subprocess.PIPE,
                universal_newlines=True,
            )
            outside_id[target] = got.stdout.strip() or "absent"
        keys = {}
        for name in names:
            members = self.closure(name)
            if globals_dirty or members & dirty:
                keys[name] = None
                continue
            h = base.copy()
            for m in sorted(members):
                h.update((m + "=" + (tree[m] if m == name else lib[m].hexdigest()) + "\n").encode())
            for target in sorted(outside):
                readers = outside[target]
                if name in readers or any(m in members and not tests for m, tests in readers.items()):
                    h.update((target + "=" + outside_id[target] + "\n").encode())
            keys[name] = h.hexdigest()[:32]
        return keys


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="master", help="compare against the merge base with this ref (default master)")
    ap.add_argument("--head", default="HEAD")
    ap.add_argument("--committed", action="store_true", help="ignore uncommitted and untracked files")
    ap.add_argument("--format", choices=("lines", "args", "json"), default="lines")
    ap.add_argument("--why", action="store_true", help="say on stderr which file put each crate in the set")
    ap.add_argument("--keys", action="store_true", help="print every crate's content key instead")
    args = ap.parse_args()

    ws = Workspace(repo_root())
    if args.keys:
        for name, key in sorted(ws.keys().items()):
            print(name, key or "uncommitted-changes")
        return 0

    files = ws.changed_files(args.base, args.head, worktree=not args.committed)
    crates, why, unmapped = ws.affected(files)
    crates = ws.order(crates)
    if args.why:
        for c in ws.order(why):
            only = all(tests for _, tests in why[c])
            more = ", ..." if len(why[c]) > 1 else ""
            print("changed: %s (%s%s)%s" % (c, why[c][0][0], more, ", its tests only" if only else ""), file=sys.stderr)
        print("%d crates changed, %d to test" % (len(why), len(crates)), file=sys.stderr)
    if unmapped:
        print("affected.py: %d changed files belong to no crate and no crate names them:" % len(unmapped), file=sys.stderr)
        for f in unmapped[:20]:
            print("  " + f, file=sys.stderr)
        if len(unmapped) > 20:
            print("  ... and %d more" % (len(unmapped) - 20), file=sys.stderr)
    if args.format == "json":
        print(json.dumps({"changed": ws.order(why), "test": crates, "unmapped": unmapped}))
    elif args.format == "args":
        print(" ".join("-p " + c for c in crates))
    else:
        for c in crates:
            print(c)
    return 0


if __name__ == "__main__":
    sys.exit(main())
