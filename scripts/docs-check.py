#!/usr/bin/env python3
"""Check the docs against the repository. Reads only; needs git and python3.

    scripts/docs-check.py                 # every check, every doc
    scripts/docs-check.py docs/perf       # only docs under these paths
    scripts/docs-check.py --strict ...    # names, paths and style fail too

Checks:

  links   relative markdown links and #anchors resolve (always fails the run)
  index   every docs/**/*.md is linked from docs/README.md (always fails)
  names   H2_* variables and impl.* switches a doc names exist in the code
  paths   backticked crates/, platform/, scripts/, deploy/ and web/ paths exist
  style   headings that record history instead of the design
  code    docs/*.md paths cited in code comments exist (always fails), and
          the section they cite (`§ 5.2` or "Title") is still a heading

names, paths and style skip dated records (docs/**/notes/, docs/reviews/)
and docs/meta/, which describe the tree as it was or quote what is wrong. They are findings to clear or to justify,
because a design page for unbuilt work names things that do not exist yet;
--strict turns them into failures. docs/meta/docs-style.md has the rules.
"""

import os
import re
import subprocess
import sys

ROOT = subprocess.check_output(
    ["git", "rev-parse", "--show-toplevel"], text=True
).strip()

LINK = re.compile(r'!?\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)')
ENV = re.compile(r"H2_[A-Z0-9_]+")
SWITCH = re.compile(r"impl\.[a-z0-9_-]+(?:\.[a-z0-9_-]+)*")
CODE_PATH = re.compile(
    r"`((?:crates|platform|scripts|deploy|web)/[A-Za-z0-9_./\-]+)(?:[:#][^`]*)?`"
)
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
HISTORY_HEADING = re.compile(
    r"20\d\d-\d\d-\d\d|\bas built\b|\bwhat (?:was|is) built\b|\bchangelog\b"
    r"|^(?:\d+\.? )?(?:development |change )?history$|\bsuperseded\b|^update\b"
    r"|\(re-?measured\)|^re-?measured\b|\(landed\)|\bbuilt: slice \d",
    re.I,
)
# Names that are placeholders in prose, not variables.
ENV_PLACEHOLDERS = {"H2_A", "H2_B", "H2_X", "H2_AFTER", "H2_BEFORE"}


def git_lines(*args):
    out = subprocess.run(
        ["git", "-C", ROOT, *args], text=True, capture_output=True
    ).stdout
    return [line for line in out.split("\n") if line]


def is_record(path):
    """Dated records and the pages about the docs: not statements of the design."""
    return (
        "/notes/" in path
        or path.startswith("docs/reviews/")
        or path.startswith("docs/meta/")
    )


def slug(heading):
    heading = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading.strip().lower())
    heading = heading.replace("`", "")
    out = []
    for ch in heading:
        if ch.isalnum() or ch in "-_":
            out.append(ch)
        elif ch == " ":
            out.append("-")
    return "".join(out)


def prose_lines(path):
    """(line number, text) outside fenced code blocks."""
    fenced = False
    with open(os.path.join(ROOT, path), encoding="utf-8") as f:
        for number, line in enumerate(f, 1):
            if line.lstrip().startswith("```"):
                fenced = not fenced
                continue
            if not fenced:
                yield number, line.rstrip("\n")


_anchors = {}


def anchors(path):
    if path not in _anchors:
        found, counts = set(), {}
        for _, line in prose_lines(path):
            m = HEADING.match(line)
            if m:
                s = slug(m.group(2))
                n = counts.get(s, 0)
                counts[s] = n + 1
                found.add(s if n == 0 else f"{s}-{n}")
            found.update(re.findall(r'<a\s+(?:name|id)="([^"]+)"', line))
        _anchors[path] = found
    return _anchors[path]


def check_links(files):
    bad = []
    for path in files:
        for number, line in prose_lines(path):
            for m in LINK.finditer(re.sub(r"`[^`]*`", "", line)):
                target = m.group(1)
                if re.match(r"^[a-z][a-z0-9+.-]*:", target):
                    continue
                file_part, _, fragment = target.partition("#")
                dest = path
                if file_part:
                    dest = os.path.normpath(
                        os.path.join(os.path.dirname(path), file_part)
                    )
                if not os.path.exists(os.path.join(ROOT, dest)):
                    bad.append(f"{path}:{number}: {target}: no such file")
                elif fragment and dest.endswith(".md"):
                    if fragment.lower() not in anchors(dest):
                        bad.append(f"{path}:{number}: {target}: no such heading")
    return bad


def check_index(all_docs):
    index = "docs/README.md"
    with open(os.path.join(ROOT, index), encoding="utf-8") as f:
        text = f.read()
    linked = {
        os.path.normpath(os.path.join("docs", m.group(1).partition("#")[0]))
        for m in LINK.finditer(text)
        if not re.match(r"^[a-z][a-z0-9+.-]*:", m.group(1))
    }
    return [
        f"{path}: not linked from {index}"
        for path in all_docs
        if path != index and path not in linked
    ]


def code_names():
    spec = ["--", ".", ":!docs", ":!*.md"]
    envs = set(git_lines("grep", "-ohE", "H2_[A-Z0-9_]+", *spec))
    # POSIX ERE for git grep: no (?:...) groups.
    switches = set(
        git_lines("grep", "-ohE", r"impl\.[a-z0-9_-]+(\.[a-z0-9_-]+)*", "--", "crates")
    )
    # Registered as `.implementation("sim.arena-hash", ..)`: the `impl.` is implied.
    # Ports name theirs as a plain string (`port(.., "sim.native_pose_matrix", ..)`).
    for quoted in git_lines(
        "grep", "-ohE", r'"(sim|audio|render|net|input)\.[a-z0-9_.-]+"', "--", "crates"
    ):
        switches.add("impl." + quoted.strip('"'))
    return envs, {s.rstrip(".") for s in switches}


def check_names(files):
    envs, switches = code_names()
    bad = []
    for path in files:
        if is_record(path):
            continue
        for number, line in prose_lines(path):
            for name in set(ENV.findall(line)):
                if name in envs or name in ENV_PLACEHOLDERS:
                    continue
                # `H2_BOOT_*` is written as a prefix: accept a real one.
                if name.endswith("_") and any(e.startswith(name) for e in envs):
                    continue
                bad.append(f"{path}:{number}: {name}: not read anywhere in the code")
            for name in set(SWITCH.findall(line)):
                name = name.rstrip(".")
                if name in switches or any(
                    s.startswith(name + ".")
                    or s.startswith(name + "_")
                    or (name.endswith("_") and s.startswith(name))
                    for s in switches
                ):
                    continue
                bad.append(f"{path}:{number}: {name}: no such switch in crates/")
    return bad


def ignored(path):
    return (
        subprocess.run(
            ["git", "-C", ROOT, "check-ignore", "-q", path], capture_output=True
        ).returncode
        == 0
    )


def check_paths(files):
    bad = []
    for path in files:
        if is_record(path):
            continue
        for number, line in prose_lines(path):
            for m in CODE_PATH.finditer(line):
                cited = m.group(1).rstrip(".,;")
                if any(ch in cited for ch in "*<>") or "..." in cited:
                    continue
                full = os.path.join(ROOT, cited)
                if os.path.exists(full) or os.path.exists(full + ".rs"):
                    continue
                if ignored(cited):
                    continue  # a build output or local file, by .gitignore
                bad.append(f"{path}:{number}: {cited}: no such path")
    return bad


CODE_REF = re.compile(
    r"(docs/[A-Za-z0-9_/.\-]+\.md)(?:[\s,;:(]{0,4}(?:§\s*([0-9]+(?:\.[0-9]+)*)|\"([^\"]{3,80})\"))?"
)
# Paths used as examples in doc comments, not citations.
CODE_REF_EXAMPLES = {"docs/x.md"}


def headings(path):
    return [
        HEADING.match(line).group(2).replace("`", "").lower()
        for _, line in prose_lines(path)
        if HEADING.match(line)
    ]


def check_code_refs():
    """(missing files, missing sections) cited from tracked non-markdown files."""
    missing, sections = [], []
    cache = {}
    hits = git_lines(
        "grep", "-nE", r"docs/[A-Za-z0-9_/.-]+\.md", "--",
        "crates", "platform", "scripts", "web", "deploy", "justfile", ":!*.md",
        ":!scripts/docs-check.py",
    )
    for hit in hits:
        src, number, text = hit.split(":", 2)
        for m in CODE_REF.finditer(text):
            doc, section, title = m.group(1), m.group(2), m.group(3)
            if doc in CODE_REF_EXAMPLES:
                continue
            if not os.path.exists(os.path.join(ROOT, doc)):
                missing.append(f"{src}:{number}: {doc}: no such file")
                continue
            if doc not in cache:
                cache[doc] = headings(doc)
            if section and not any(
                re.match(rf"(?:§\s*)?{re.escape(section)}(?:[.:\s]|$)", h)
                for h in cache[doc]
            ):
                sections.append(f"{src}:{number}: {doc} § {section}: no such heading")
            if title:
                want = title.replace("`", "").lower().rstrip(".")
                if not any(want in h for h in cache[doc]):
                    sections.append(f'{src}:{number}: {doc} "{title}": no such heading')
    return missing, sections


def check_style(files):
    bad = []
    for path in files:
        if is_record(path) or not path.startswith("docs/"):
            continue
        for number, line in prose_lines(path):
            m = HEADING.match(line)
            if m and HISTORY_HEADING.search(m.group(2)):
                bad.append(f"{path}:{number}: history heading: {m.group(2)}")
    return bad


def main(argv):
    strict = "--strict" in argv
    wanted = [a.rstrip("/") for a in argv if not a.startswith("--")]
    all_docs = git_lines("ls-files", "docs/*.md", "docs/**/*.md")
    tracked = all_docs + git_lines(
        "ls-files", "README.md", "AGENTS.md", "CONTRIBUTING.md", "crates/*/README.md"
    )
    files = [
        p
        for p in tracked
        if os.path.exists(os.path.join(ROOT, p))
        and (not wanted or any(p == w or p.startswith(w + "/") for w in wanted))
    ]
    all_docs = [p for p in all_docs if os.path.exists(os.path.join(ROOT, p))]

    missing, sections = check_code_refs()
    if wanted:
        missing = [m for m in missing if any(f" {w}" in m for w in wanted)]
        sections = [m for m in sections if any(f" {w}" in m for w in wanted)]
    checks = [
        ("links", check_links(files), True),
        ("index", check_index(all_docs), True),
        ("code", missing, True),
        ("code-sections", sections, False),
        ("names", check_names(files), False),
        ("paths", check_paths(files), False),
        ("style", check_style(files), False),
    ]
    failed = False
    for name, found, hard in checks:
        for line in found:
            print(f"{name}: {line}")
        print(f"{name}: {len(found)} finding(s)", file=sys.stderr)
        failed = failed or (bool(found) and (hard or strict))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
