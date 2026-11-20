"""Shared machinery of the move scripts (docs/dev/crate-structure.md).

A window script builds a list of moves (old path -> new path, files or
whole directories, relative to the repository root) and hands it to
`Window`. The window then:

  1. runs the `git mv`s;
  2. rewrites every `path = "..."` in every `Cargo.toml`, and the
     `[workspace]` tables of the root one;
  3. rewrites relative paths (`include_str!("../../x/y")`, `#[path]`,
     `CARGO_MANIFEST_DIR` joins, markdown links) in the files that moved;
  4. rewrites `crates/<old>` to `crates/<new>` where a path is written
     from the repository root: docs, scripts, comments;
  5. checks the result: manifests parse, every path dependency resolves,
     nothing tracked is left behind.

Every rewrite is by rule and may run twice: a path that already resolves
is left alone. Nothing here runs cargo.
"""
import os
import re
import subprocess
import sys
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import cratemeta  # noqa: E402

# Files the mention rewrite never touches: they name old paths on purpose.
KEEP = ("scripts/moves/", "docs/dev/crate-structure.md", "Cargo.lock")
# A worktree sits three directories below the main checkout
# (.claude/worktrees/<name>); some tests climb out to reach its disc/.
WORKTREE_DEPTH = 3
PATH_CHARS = r"A-Za-z0-9_.\-/"
TOKEN = re.compile(rf"((?:\.\./)+)([{PATH_CHARS}]*)")
FILE_RELATIVE = re.compile(r"include(?:_str|_bytes)?!\s*\(|#\[path\b")


def git(*args, check=True):
    return subprocess.run(["git", "-C", ROOT, *args], check=check, capture_output=True, text=True).stdout


def tracked(*pathspec):
    return [line for line in git("ls-files", "--", *pathspec).split("\n") if line]


def read(path):
    """The text of a tracked file, or None for a binary one."""
    try:
        with open(os.path.join(ROOT, path), encoding="utf-8") as f:
            return f.read()
    except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
        return None


def write(path, text):
    full = os.path.join(ROOT, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w", encoding="utf-8") as f:
        f.write(text)


def exists(path):
    return os.path.exists(os.path.join(ROOT, path))


def norm(path):
    return os.path.normpath(path).replace(os.sep, "/")


def ups(path):
    """How many directories a normalised relative path climbs."""
    n = 0
    for part in path.split("/"):
        if part != "..":
            break
        n += 1
    return n


def rel(target, base):
    """`target` as seen from `base`; both root-relative, either may climb
    out of the repository."""
    deep = "/" + "/".join("u%d" % i for i in range(12))
    return norm(os.path.relpath(os.path.join(deep, target), os.path.join(deep, base or ".")))


class PathMap:
    """Old path -> new path by the longest moved prefix, and back."""

    def __init__(self, moves):
        self.moves = sorted(moves, key=lambda m: -len(m[0]))
        self.back = sorted(((n, o) for o, n in moves), key=lambda m: -len(m[0]))

    @staticmethod
    def _apply(table, path):
        for a, b in table:
            if path == a:
                return b
            if path.startswith(a + "/"):
                return b + path[len(a):]
        return path

    def new(self, path):
        return self._apply(self.moves, path)

    def old(self, path):
        return self._apply(self.back, path)


def crate_root(path):
    """The directory of the nearest Cargo.toml at or above a file."""
    d = os.path.dirname(path)
    while d and not exists(os.path.join(d, "Cargo.toml")):
        d = os.path.dirname(d)
    return d


class Window:
    def __init__(self, name, moves, aliases=()):
        """`moves`: (old, new) pairs that `git mv` runs. `aliases`: more
        (old, new) pairs that only translate a written path (a merged
        crate's directory, which has no single new place on disk)."""
        self.name = name
        self.moves = [(norm(a), norm(b)) for a, b in moves if norm(a) != norm(b)]
        self.map = PathMap(self.moves + [(norm(a), norm(b)) for a, b in aliases])
        self.report = []
        self.changed = set()
        self.top = {p.split("/")[0] for p in tracked()} | {"disc"}
        self.top.discard("crates")

    # 1. git mv -----------------------------------------------------------

    def require_clean(self):
        dirty = [l for l in git("status", "--porcelain").split("\n") if l and not l.startswith("??")]
        if dirty and "--dirty" not in sys.argv:
            sys.exit(f"{self.name}: the tree has uncommitted changes; commit them first (or pass --dirty):\n  "
                     + "\n  ".join(dirty[:10]))

    def mv(self):
        done = 0
        # A directory that moves into itself goes first (crates/runtime ->
        # crates/runtime/runtime), before anything lands beside it; then
        # deepest first, so a file leaves before its directory does.
        for old, new in sorted(self.moves, key=lambda m: (not m[1].startswith(m[0] + "/"), -m[0].count("/"))):
            if not tracked(old):
                continue
            if exists(new) and not os.path.isdir(os.path.join(ROOT, old)):
                sys.exit(f"{self.name}: {new} exists; cannot move {old} onto it")
            os.makedirs(os.path.dirname(os.path.join(ROOT, new)) or ROOT, exist_ok=True)
            if new.startswith(old + "/"):
                # A directory into itself (crates/runtime -> crates/runtime/runtime).
                parked = old + ".moving"
                git("mv", old, parked)
                os.makedirs(os.path.dirname(os.path.join(ROOT, new)), exist_ok=True)
                git("mv", parked, new)
            elif os.path.isdir(os.path.join(ROOT, old)) and os.path.isdir(os.path.join(ROOT, new)):
                # The destination is already there (a merge target): move the entries.
                for entry in sorted(os.listdir(os.path.join(ROOT, old))):
                    if tracked(f"{old}/{entry}"):
                        if exists(f"{new}/{entry}"):
                            sys.exit(f"{self.name}: {new}/{entry} exists; cannot move {old}/{entry} onto it")
                        git("mv", f"{old}/{entry}", f"{new}/{entry}")
            else:
                git("mv", old, new)
            done += 1
        self.report.append(f"git mv: {done} of {len(self.moves)} moves ran ({len(self.moves) - done} already done)")
        return done

    # 2. manifests ----------------------------------------------------------

    @staticmethod
    def _target_ok(path):
        """A manifest `path` value's target: a crate directory or a file."""
        if path.startswith(".."):
            return False
        full = os.path.join(ROOT, path)
        return os.path.isfile(full) or os.path.isfile(os.path.join(full, "Cargo.toml"))

    def manifest_path(self, manifest, value):
        """Where a `path = "<value>"` in `manifest` should point now."""
        here = os.path.dirname(manifest)
        if self._target_ok(norm(os.path.join(here, value))):
            return value
        then = norm(os.path.join(os.path.dirname(self.map.old(manifest)), value))
        target = self.map.new(then)
        if self._target_ok(target):
            return rel(target, here)
        return None

    def manifests(self):
        fixed = 0
        for manifest in tracked("Cargo.toml", "*/Cargo.toml"):
            text = read(manifest)

            def fix(m):
                nonlocal fixed
                new = self.manifest_path(manifest, m.group(2))
                if new is None:
                    self.report.append(f"UNRESOLVED {manifest}: path = \"{m.group(2)}\"")
                    return m.group(0)
                if new != m.group(2):
                    fixed += 1
                return m.group(1) + new + m.group(3)

            out = re.sub(r'(\bpath\s*=\s*")([^"]+)(")', fix, text)
            if out != text:
                write(manifest, out)
                self.changed.add(manifest)
        self.report.append(f"Cargo.toml: {fixed} path values rewritten")

    def workspace_members(self, members):
        text = read("Cargo.toml")
        block = "members = [\n" + "".join(f'    "{m}",\n' for m in members) + "]"
        out, n = re.subn(r"members\s*=\s*\[[^\]]*\]", block, text, count=1)
        if n != 1:
            sys.exit("Cargo.toml: no `members = [...]` to rewrite")
        if out != text:
            write("Cargo.toml", out)
            self.changed.add("Cargo.toml")

    # 3. relative paths in moved files ----------------------------------------

    def _valid(self, rel):
        """Whether a root-relative path is a place a file may point at."""
        if rel == ".":
            return True
        first = rel.split("/")[0]
        if first != "crates":
            return first in self.top
        return exists(rel) and bool(crate_root(rel + "/x"))

    def retarget(self, base_new, base_old, token):
        """A relative path written against base_old, rewritten for
        base_new; None to leave it alone."""
        slash = "/" if token.endswith("/") else ""
        as_is = norm(os.path.join(base_new, token))
        then = norm(os.path.join(base_old, token))
        if ups(as_is):
            # Out of the repository: the worktree's climb to the main disc/.
            if ups(as_is) != WORKTREE_DEPTH and ups(then) == WORKTREE_DEPTH:
                return rel(then, base_new) + slash
            return None
        if self._valid(as_is) or ups(then):
            return None
        target = self.map.new(then)
        if not self._valid(target):
            own = crate_root(base_new + "/x")
            if target != then and not (own and target.startswith(own + "/")):
                self.report.append(f"UNRESOLVED relative path into a moved crate: {token} (from {base_new})")
            return None
        return rel(target, base_new) + slash

    def relative_paths(self, files=None):
        fixed, touched = 0, []
        moved_roots = [new for _, new in self.moves]
        if files is None:
            files = [f for root in moved_roots for f in (tracked(root) if os.path.isdir(os.path.join(ROOT, root)) else [root])]
        for path in sorted(set(files)):
            if os.path.basename(path) == "Cargo.toml":
                continue
            text = read(path)
            if text is None or "../" not in text:
                continue
            old_path = self.map.old(path)
            dir_new, dir_old = os.path.dirname(path), os.path.dirname(old_path)
            root_new, root_old = crate_root(path), self.map.old(crate_root(path))
            is_rust = path.endswith(".rs")
            lines = text.split("\n")
            for i, line in enumerate(lines):
                if "../" not in line:
                    continue
                comment = is_rust and line.lstrip().startswith("//")
                if comment and "](" not in line:
                    continue
                by_file = not is_rust or comment or FILE_RELATIVE.search(line)
                base_new, base_old = (dir_new, dir_old) if by_file else (root_new, root_old)

                def fix(m):
                    nonlocal fixed
                    before = line[:m.start()]
                    if before.endswith("/"):
                        if re.search(r"[\w.\-}]/$", before) and not before.endswith("{}/"):
                            return m.group(0)
                    elif re.search(r"[\w.]$", before):
                        return m.group(0)
                    new = self.retarget(base_new, base_old, m.group(0))
                    if new is None or new == m.group(0):
                        return m.group(0)
                    fixed += 1
                    touched.append(f"{path}:{i + 1}: {m.group(0)} -> {new}")
                    return new

                lines[i] = TOKEN.sub(fix, line)
            out = "\n".join(lines)
            if out != text:
                write(path, out)
                self.changed.add(path)
        self.report.append(f"relative paths: {fixed} rewritten in moved files")
        self.relative_log = touched

    # 4. paths written from the repository root ---------------------------------

    def _translate(self, written):
        """A `crates/...` path as written, in the new layout. A path
        already under a new location is left alone."""
        trail = "/" if written.endswith("/") else ""
        bare = written.rstrip("/")
        if any(bare == n or bare.startswith(n + "/") for _, n in self.map.moves):
            return written
        return self.map.new(bare) + trail

    def _mention_lines(self, path, text):
        """(line index, start column of the part a rewrite may touch) for
        each line that names `crates/`. Prose and scripts: the whole line.
        Rust and Cargo.toml: comments only. A Rust string that names a
        crate path may be data (a generated banner, a test's stand-in
        tree); the real paths are in the window's `literal` list, and a
        manifest's paths are `manifests()`'s."""
        marker = {".rs": "//", ".toml": "#"}.get(os.path.splitext(path)[1])
        if os.path.basename(path) != "Cargo.toml" and not path.endswith(".rs"):
            marker = None
        for i, line in enumerate(text.split("\n")):
            if "crates/" not in line:
                continue
            if marker is None:
                yield i, 0
            elif line.lstrip().startswith(marker):
                yield i, 0
            else:
                cut = line.find(f" {marker} ")
                yield i, cut if cut >= 0 else len(line)

    MENTION = re.compile(rf"(?<![A-Za-z0-9_\-])crates/[{PATH_CHARS}]*[A-Za-z0-9_\-/]")

    def mentions(self):
        """`crates/<old>` -> `crates/<new>` where a path is written from the
        repository root: prose, scripts, comments. May run twice."""
        fixed = files = 0

        def fix(m):
            nonlocal fixed
            new = self._translate(m.group(0))
            fixed += new != m.group(0)
            return new

        for path in tracked():
            if path.startswith(KEEP) or path in KEEP:
                continue
            text = read(path)
            if text is None or "crates/" not in text:
                continue
            lines = text.split("\n")
            for i, start in self._mention_lines(path, text):
                lines[i] = lines[i][:start] + self.MENTION.sub(fix, lines[i][start:])
            out = "\n".join(lines)
            if out != text:
                write(path, out)
                self.changed.add(path)
                files += 1
        self.report.append(f"mentions: {fixed} `crates/<old>` paths rewritten in {files} files")

    def left_in_code(self):
        """Old `crates/...` paths still in Rust strings after the window."""
        left = []
        for path in tracked("*.rs"):
            text = read(path)
            if text is None or "crates/" not in text or path.startswith(KEEP):
                continue
            lines = text.split("\n")
            for i, start in self._mention_lines(path, text):
                left += [f"{path}:{i + 1}: {m.group(0)}" for m in self.MENTION.finditer(lines[i][:start])
                         if self._translate(m.group(0)) != m.group(0)]
        return left

    def literal(self, path, old, new, count=None):
        """One fixed replacement in one file; already-applied is fine."""
        text = read(path)
        if text is None:
            self.report.append(f"MISSING {path} (literal edit skipped)")
            return
        n = text.count(old)
        if n == 0:
            if new not in text:
                self.report.append(f"NOT FOUND in {path}: {old!r}")
            return
        if count is not None and n != count:
            self.report.append(f"COUNT {path}: {old!r} found {n} times, expected {count}")
        write(path, text.replace(old, new))
        self.changed.add(path)

    # 5. checks ------------------------------------------------------------------

    def stage(self, *new_files):
        git("add", "-u")
        if new_files:
            git("add", "--", *new_files)

    def verify(self, old_dirs=()):
        problems = []
        for manifest in tracked("Cargo.toml", "*/Cargo.toml"):
            try:
                with open(os.path.join(ROOT, manifest), "rb") as f:
                    tomllib.load(f)
            except tomllib.TOMLDecodeError as e:
                problems.append(f"{manifest}: {e}")
        try:
            pkgs = cratemeta.packages(ROOT)
        except Exception as e:  # a manifest that names a missing one
            problems.append(f"workspace does not load: {e}")
            pkgs = {}
        for name, p in pkgs.items():
            for d in p["dependencies"]:
                if d["path"] and not os.path.isfile(os.path.join(d["path"], "Cargo.toml")):
                    problems.append(f"{name}: dependency {d['name']} points at {os.path.relpath(d['path'], ROOT)}, which has no Cargo.toml")
        members = {p["dir"] for p in pkgs.values()}
        for manifest in tracked("crates/*Cargo.toml"):
            d = os.path.dirname(manifest)
            if d not in members:
                problems.append(f"{d}: a crate no `[workspace] members` entry reaches")
        for old in old_dirs:
            left = tracked(old)
            if left:
                problems.append(f"{old}: {len(left)} tracked files left behind ({left[0]}, ...)")
        self.problems = problems
        return problems

    def finish(self, old_dirs=()):
        # Directories the moves emptied (git removes only the ones it emptied itself).
        for d in old_dirs:
            for dirpath, _, _ in os.walk(os.path.join(ROOT, d), topdown=False):
                if not os.listdir(dirpath):
                    os.rmdir(dirpath)
        problems = self.verify(old_dirs)
        stray = [d for d in old_dirs if os.path.isdir(os.path.join(ROOT, d)) and not tracked(d)]
        print(f"{self.name}: {len(self.changed)} files edited")
        for line in self.report:
            print(f"  {line}")
        for line in getattr(self, "relative_log", [])[:0 if "--quiet" in sys.argv else 400]:
            print(f"    {line}")
        left = self.left_in_code()
        if left:
            print(f"  {len(left)} `crates/<old>` strings in Rust code left as they are (data, not paths):")
            for line in left:
                print(f"    {line}")
        for d in stray:
            print(f"  note: {d}/ still exists with untracked files only; delete it by hand")
        for p in problems:
            print(f"{self.name}: PROBLEM: {p}", file=sys.stderr)
        return 1 if problems or any(r.startswith(("UNRESOLVED", "NOT FOUND", "COUNT", "MISSING")) for r in self.report) else 0


def crate_dirs():
    """Every directory under crates/ that holds a tracked Cargo.toml."""
    return sorted(os.path.dirname(m) for m in tracked("crates/*Cargo.toml"))


# Rust path edits ---------------------------------------------------------------

def local_binding(old_ident):
    """Matches a file that gives `old_ident` another meaning: a module of
    that name, or an import that ends in it (`use a::old;`,
    `use a::old::{self, ..}`, `use a::{b, old}`, `as old`). In such a
    file a bare `old::` is not the crate."""
    i = re.escape(old_ident)
    return re.compile(
        rf"(?m)^\s*(?:pub(?:\([a-z ]+\))?\s+)?mod\s+{i}\s*[;{{]"
        rf"|\buse\s+[\w:]+::{i}\s*(?:;|::\s*\{{\s*self\b)"
        rf"|\buse\s+[\w:]+::\{{[^}}]*(?<![\w:]){i}\s*[,}}]"
        rf"|\bas\s+{i}\s*[;,}}]")


def rename_rust_crate(files, old_ident, new_path, own_crate_files=(), own_path=None, skipped=None):
    """`old_ident::` -> `new_path::` in Rust files. In `own_crate_files`
    (files that now live inside the crate the name points into) the
    replacement is `own_path::` instead. A file that binds `old_ident` to
    something else is left alone and appended to `skipped`. Returns the
    number of edits."""
    pattern = re.compile(r"(?<![\w:.$])" + re.escape(old_ident) + r"(?=::)")
    bare = re.compile(r"(?<![\w:.$])(use|extern crate)\s+" + re.escape(old_ident) + r"\s*(;| as )")
    local = local_binding(old_ident)
    own = set(own_crate_files)
    edits = 0
    for path in files:
        text = read(path)
        if text is None or old_ident not in text:
            continue
        if local.search(text):
            if skipped is not None and pattern.search(text):
                skipped.append(path)
            continue
        to = own_path if path in own and own_path else new_path
        out, n = pattern.subn(to, text)
        out, m = bare.subn(lambda b: f"{b.group(1)} {to}{b.group(2)}", out)
        if n or m:
            write(path, out)
            edits += n + m
    return edits


def rename_manifest_dep(text, old, new):
    """A dependency key and its feature references, renamed in one
    Cargo.toml's text. Returns (text, edits)."""
    edits = 0
    out = []
    for line in text.split("\n"):
        m = re.match(rf"^(\s*){re.escape(old)}(\s*=\s*[{{\"]|\.[a-z-]+\s*=)", line)
        if m:
            line = m.group(1) + new + line[m.start(2):]
            edits += 1
        elif re.match(rf"^\[profile\.[^\]]*\.package\.{re.escape(old)}\]", line):
            line = line.replace(f".package.{old}]", f".package.{new}]")
            edits += 1
        for a, b in ((f'"{old}/', f'"{new}/'), (f'"{old}?/', f'"{new}?/'), (f'"dep:{old}"', f'"dep:{new}"')):
            if a in line:
                edits += line.count(a)
                line = line.replace(a, b)
        out.append(line)
    return "\n".join(out), edits


def rename_crate(old, new, window_name):
    """Rename package `old` to `new`, directory and all, in place: the
    crate stays in its domain. Returns the process exit code."""
    pkgs = cratemeta.packages(ROOT)
    if old not in pkgs:
        if new in pkgs:
            print(f"{window_name}: `{old}` is already `{new}`")
            return 0
        sys.exit(f"{window_name}: no package `{old}`")
    if new in pkgs:
        sys.exit(f"{window_name}: a package `{new}` already exists")
    old_dir = pkgs[old]["dir"]
    new_dir = norm(os.path.join(os.path.dirname(old_dir), new))
    old_ident, new_ident = old.replace("-", "_"), new.replace("-", "_")
    w = Window(window_name, [(old_dir, new_dir)])
    w.require_clean()
    w.mv()

    # The package's own name, then every manifest that names it.
    own = f"{new_dir}/Cargo.toml"
    text = read(own)
    renamed, n = re.subn(rf'(?m)^name\s*=\s*"{re.escape(old)}"', f'name = "{new}"', text, count=1)
    if n != 1:
        sys.exit(f"{own}: no `name = \"{old}\"`")
    write(own, renamed)
    w.changed.add(own)
    keys = 0
    for manifest in tracked("Cargo.toml", "*/Cargo.toml"):
        text = read(manifest)
        out, n = rename_manifest_dep(text, old, new)
        if n:
            write(manifest, out)
            w.changed.add(manifest)
            keys += n
    w.report.append(f"Cargo.toml: {keys} dependency keys and feature references renamed")
    w.manifests()
    w.relative_paths()

    # Rust paths. A crate with its own top-level module of the new name
    # must say `::new::` to mean the crate.
    # Only a crate that depends on `old`, and `old` itself, can mean the
    # crate by that name; anywhere else `old::` is a module of its own.
    users = {new_dir} | {p["dir"] for p in pkgs.values() if any(d["name"] == old for d in p["dependencies"])}
    rust = [f for f in tracked("*.rs") if any(f.startswith(u + "/") for u in users)]
    shadowed = []
    for name, p in pkgs.items():
        root = p["dir"] if name != old else new_dir
        for entry in ("src/lib.rs", "src/main.rs"):
            text = read(f"{root}/{entry}")
            if text and re.search(rf"(?m)^\s*(pub(\([a-z ]+\))?\s+)?mod\s+{re.escape(new_ident)}\s*[;{{]", text):
                shadowed.append(root)
    inside = [f for f in rust if any(f.startswith(r + "/src/") for r in shadowed)]
    skipped = []
    edits = rename_rust_crate([f for f in rust if f not in set(inside)], old_ident, new_ident, skipped=skipped)
    edits += rename_rust_crate(inside, old_ident, "::" + new_ident, skipped=skipped)
    for f in rust:
        w.changed.add(f)  # counted loosely; `git status` has the truth
    w.changed = {f for f in w.changed if not f.endswith(".rs")}
    w.report.append(f"Rust: {edits} `{old_ident}::` paths renamed to `{new_ident}::`"
                    + (f" (`::{new_ident}::` in {', '.join(shadowed)}, which has its own `mod {new_ident}`)" if shadowed else ""))
    for path in skipped:
        w.report.append(f"left alone: {path} has its own `{old_ident}` (a module or an import); its `{old_ident}::` paths are not the crate's")
    w.mentions()

    # `-p old` in docs and scripts; the domain table.
    flags = 0
    flag = re.compile(rf"(-p |--package[ =]|package\(){re.escape(old)}(?![\w-])")
    for path in tracked("docs", "scripts", ".github", "*.md", "*.sh", "justfile", ".config"):
        if path.startswith(KEEP) or path in KEEP:
            continue
        text = read(path)
        if text is None or old not in text:
            continue
        out, n = flag.subn(lambda m: m.group(1) + new, text)
        if n:
            write(path, out)
            flags += n
    w.report.append(f"commands: {flags} `-p {old}` renamed")
    rules_text = read(cratemeta.RULES)
    lines = []
    for line in rules_text.split("\n"):
        if re.match(r'^(from|to)\s*=', line):
            line = line.replace(f'"{old}"', f'"{new}"')
        elif f'"{old}"' in line and f'"{new}"' in line:
            line = re.sub(rf'"{re.escape(old)}",\s*', "", line)
        elif f'"{old}"' in line and line.startswith(" "):
            line = line.replace(f'"{old}"', f'"{new}"')
        lines.append(line)
    write(cratemeta.RULES, "\n".join(lines))
    w.stage()
    w.report.append("Cargo.lock still names the old package: the next cargo command rewrites it; commit that with the window")
    return w.finish(old_dirs=[old_dir])


def copy_manifest(src_dir, dst_dir, src_pkg, dst_pkg, description):
    """A new package's manifest: the source's dependencies and features
    under the new name, without its targets, and without the dependencies
    the destination's domain may not have (scripts/crate-domains.toml).
    Returns (text, [dropped dependency names])."""
    rule_set = cratemeta.rules(ROOT)
    pkgs = cratemeta.packages(ROOT)
    domain_of = {n: cratemeta.domain_of(p, rule_set, ROOT)[0] for n, p in pkgs.items()}
    dst_domain = dst_dir.split("/")[1]
    allowed = set(rule_set["domains"][dst_domain].get("uses", [])) | {dst_domain}
    text = read(f"{src_dir}/Cargo.toml")
    text = re.sub(rf'(?m)^name\s*=\s*"{re.escape(src_pkg)}"', f'name = "{dst_pkg}"', text, count=1)
    text = re.sub(r"(?m)^description\s*=.*$", lambda _: 'description = "' + description.replace('"', '\\"') + '"', text, count=1)
    text = re.sub(r"(?ms)^\[\[(bin|test|example|bench)\]\].*?(?=^\[|\Z)", "", text)
    text = re.sub(r"(?ms)^\[lib\].*?(?=^\[|\Z)", "", text)
    text = re.sub(r"(?m)^(build|autotests|autobins|autoexamples|autobenches)\s*=.*\n", "", text)
    text = re.sub(r'(\bpath\s*=\s*")([^"]+)(")',
                  lambda m: m.group(1) + rel(norm(os.path.join(src_dir, m.group(2))), dst_dir) + m.group(3), text)
    out, dropped, header = [], [], ""
    for line in text.split("\n"):
        if line.startswith("["):
            header = line.strip()
        key = re.match(r"^([A-Za-z0-9_-]+)\s*(=|\.)", line)
        normal = header.endswith("dependencies]") and "dev-dependencies" not in header
        if key and normal and key.group(1) in domain_of and domain_of[key.group(1)] not in allowed:
            dropped.append(key.group(1))
            while out and out[-1].lstrip().startswith("#"):
                out.pop()  # the comment that explained it
            continue
        out.append(line)
    text = "\n".join(out)
    for name in dropped:
        text = re.sub(rf'\s*"(dep:{re.escape(name)}|{re.escape(name)}\??/[^"]*)",?', "", text)
    text = re.sub(r"\[\s*,\s*", "[", text)
    return text.rstrip("\n") + "\n", dropped
