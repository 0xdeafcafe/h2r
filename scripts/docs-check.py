#!/usr/bin/env python3
"""Check the docs against the repository. Reads only; needs git and python3.

    scripts/docs-check.py                 # every check, every doc
    scripts/docs-check.py docs/perf       # only docs under these paths
    scripts/docs-check.py --strict ...    # names, paths and style fail too

Checks:

  links   relative markdown links and #anchors resolve (always fails the run)
  index   every docs/**/*.md is linked from docs/README.md (always fails)
  names   H2_* variables and impl.* switches a doc names exist in the code
  switches  every H2_* variable the code reads has a row in
          docs/runtime/switches.md (a `H2_FOO_*` row covers the family)
  paths   backticked crates/, platform/, scripts/, deploy/ and web/ paths exist
  style   headings that record history instead of the design
  voice   an em dash, a banned word, an American spelling, connective filler
          or a manufactured contrast (docs/meta/docs-style.md rule 14)
  length  a paragraph over MAX_SENTENCES sentences (rule 12)
  code    docs/*.md paths cited in code comments exist (always fails), and
          the section they cite (`§ 5.2` or "Title") is still a heading

names and paths also skip pages whose status line says Design or plan: they
name things that do not exist yet.

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


DESIGN_STATUS = re.compile(r"^Status:\s*(?:Design|plan)\b", re.I | re.M)
_design = {}


def is_design(path):
    """A page whose status line says Design (or plan) names things that do
    not exist yet; names and paths on it are not checked."""
    if path not in _design:
        with open(os.path.join(ROOT, path), encoding="utf-8") as f:
            _design[path] = bool(DESIGN_STATUS.search(f.read(2000)))
    return _design[path]


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


# C ABI constants and macros in generated code and its tests: H2_ names that
# are not environment variables.
ENV_ABI = {
    "H2_ABI_VERSION", "H2_CALL", "H2_JUMP", "H2_RETURN", "H2_FAULT", "H2_STOP",
    "H2_ENTRY", "H2_RESUME", "H2_RETURN_TARGET", "H2_HARNESS_EXIT", "H2_PART",
    "H2_CLOCK", "H2_LOOP_POLL", "H2_PREEMPT", "H2_ACC_SETTLE",
    "H2_ACC_SETTLE_POLLED", "H2_LD", "H2_LDR", "H2_ST", "H2_STR", "H2_X",
    "H2_ACCOUNTING", "H2_ACC_REFRESH", "H2_ATOMIC", "H2_DCALL", "H2_DJUMP",
    "H2_DIRECT_VIEW", "H2_EPILOGUE", "H2_EPILOGUE_BODY", "H2_EXIT_CALL",
    "H2_EXPORT", "H2_FIX_ABS", "H2_FLAT_NOTE", "H2_FLAT_OWNER", "H2_GATE_DROP",
    "H2_H", "H2_HALT", "H2_ICALL", "H2_IJUMP", "H2_LOCALS_HOMED", "H2_PART_GOTO",
    "H2_PART_LENT", "H2_PART_RELEND", "H2_PREEMPT_LOCALS", "H2_RAISE", "H2_SETF",
    "H2_SPAN", "H2_SPAN_LD", "H2_SPAN_ST", "H2_SSE_RUN", "H2_UNDEFF",
    "H2_WRITE_XMM", "H2_XFER",
    # A -D macro that only keys the compiler cache (emit/src/compile/pgo.rs).
    "H2_PGO_PROFILE",
    # A placeholder in a usage message (`H2_NAME=VALUE`).
    "H2_NAME",
}
SWITCHES_DOC = "docs/runtime/switches.md"


def read_env_names():
    """H2_* names the code reads or sets: quoted literals, shell `$H2_X`
    expansions and `H2_X=` assignments, outside C sources and markdown."""
    pattern = r"([\"'$]|[$][{])H2_[A-Z0-9_]+|H2_[A-Z0-9_]+="
    lines = git_lines(
        "grep", "-ohE", pattern, "--", "crates", "web", "platform", "scripts",
        "deploy", "justfile", ".github",
        ":!*.md", ":!*.c", ":!*.h", ":!scripts/docs-check.py",
    )
    names = {ENV.search(line).group(0) for line in lines}
    return {n for n in names if n not in ENV_ABI and not n.endswith("_")}


def check_switches():
    """Every H2_* variable the code reads has a row in switches.md."""
    with open(os.path.join(ROOT, SWITCHES_DOC), encoding="utf-8") as f:
        text = f.read()
    listed = set(ENV.findall(text))
    # `H2_FOO_*`, `H2_FOO*` and `H2_FOO_` cover every name with that prefix.
    prefixes = set(re.findall(r"(H2_[A-Z0-9_]*)\*", text)) | {
        n for n in listed if n.endswith("_")
    }
    return [
        f"{SWITCHES_DOC}: {name}: read in the code, not listed"
        for name in sorted(read_env_names())
        if name not in listed and not any(name.startswith(p) for p in prefixes)
    ]


def code_names():
    spec = ["--", ".", ":!docs", ":!*.md"]
    envs = set(git_lines("grep", "-ohE", "H2_[A-Z0-9_]+", *spec))
    # POSIX ERE for git grep: no (?:...) groups.
    switches = set(
        git_lines("grep", "-ohE", r"impl\.[a-z0-9_-]+(\.[a-z0-9_-]+)*", "--", "crates")
    )
    # Registered as `.implementation("sim.arena-hash", ..)`: the `impl.` is implied.
    # Some switches are named as a plain string (`"sim.native_simd"`).
    for quoted in git_lines(
        "grep", "-ohE", r'"(sim|audio|render|net|input)\.[a-z0-9_.-]+"', "--", "crates"
    ):
        switches.add("impl." + quoted.strip('"'))
    return envs, {s.rstrip(".") for s in switches}


def check_names(files):
    envs, switches = code_names()
    bad = []
    for path in files:
        if is_record(path) or is_design(path):
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


NOT_YET = re.compile(r"\s*\((?:new|planned|proposed|build output)\)")


def check_paths(files):
    bad = []
    for path in files:
        if is_record(path) or is_design(path):
            continue
        for number, line in prose_lines(path):
            for m in CODE_PATH.finditer(line):
                cited = m.group(1).rstrip(".,;")
                if NOT_YET.match(line, m.end()):
                    continue  # marked as a file the page proposes or a build output
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
CODE_REF_EXAMPLES = {"docs/x.md", "docs/a.md"}


# A bold lead-in (`**Sky.**`, `- **Sky.**`) names its paragraph or list item:
# code may cite it as a section.
RUN_IN = re.compile(r"^\s*(?:[-*]\s+|\d+\.\s+)?\*\*([^*]{3,80}?)[.:]?\*\*")


def headings(path):
    found = []
    for _, line in prose_lines(path):
        m = HEADING.match(line) or RUN_IN.match(line)
        if m:
            found.append(m.group(m.lastindex).replace("`", "").lower())
    return found


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


# Voice rules: docs/meta/docs-style.md rule 14.
BANNED = re.compile(
    r"\b(?:delve[sd]?|leverag(?:e|es|ed|ing)|utili[sz](?:e|es|ed|ing)|robust(?:ly|ness)?"
    r"|seamless(?:ly)?|unlock(?:s|ed|ing)?|elevat(?:e|es|ed|ing)|empower(?:s|ed|ing)?"
    r"|tapestry|vibrant|groundbreaking|revolutionary|next-level|game-chang(?:er|ing)"
    r"|superpowers?|paradigm|synerg(?:y|ize|istic)|key takeaway|in essence|fundamentally"
    r"|essentially|streamline[sd]?|landscape|cutting-edge|state-of-the-art|best-in-class"
    r"|let's dive|at the end of the day|comprehensive solution|journey"
    r"|furthermore|moreover|additionally|it's worth noting|it is worth noting)\b"
    r"|\bindeed,",
    re.I,
)
CONTRAST = re.compile(
    r"\b(?:isn't|is not|not) (?:just|only|about)\b[^.]{0,60}?,? (?:it's|it is|but)\b"
    r"|\bisn't the interesting (?:bit|part)",
    re.I,
)
AMERICAN = re.compile(
    r"\b(?:behavior\w*|colors?|colored|colorful|centers?|centered|analyz(?:e|es|ed|ing)"
    r"|optimiz\w*|\w+ization\w*|realiz\w*|judgment|favorites?|neighbors?|gray"
    r"|licenses? (?=to\b)|catalog(?:ed|ing)?)\b",
    re.I,
)
# Words that match the American pattern but are right as written.
AMERICAN_OK = re.compile(r"^(?:realization)$", re.I)
MAX_SENTENCES = 4


def prose_only(line):
    """A line without inline code, link targets and URLs."""
    line = re.sub(r"`[^`]*`", "", line)
    line = re.sub(r"\[[^\]]*\.md\]", "[]", line)  # a file name used as link text
    line = re.sub(r"\]\([^)]*\)", "]", line)
    return re.sub(r"https?://\S+", "", line)


def check_voice(files):
    bad = []
    for path in files:
        if not path.startswith("docs/") or path.startswith("docs/meta/"):
            continue
        for number, line in prose_lines(path):
            text = prose_only(line)
            if "\u2014" in text:
                bad.append(f"{path}:{number}: em dash")
            for m in BANNED.finditer(text):
                bad.append(f"{path}:{number}: banned word: {m.group(0)}")
            for m in AMERICAN.finditer(text):
                if not AMERICAN_OK.match(m.group(0)):
                    bad.append(f"{path}:{number}: American spelling: {m.group(0)}")
            if CONTRAST.search(text):
                bad.append(f"{path}:{number}: manufactured contrast")
    return bad


SENTENCE_END = re.compile(r"(?<![A-Z])(?<!\be\.g)(?<!\bi\.e)(?<!\bvs)(?<!\betc)[.!?](?=\s+[A-Z])")
NOT_PROSE = re.compile(r"^\s*(?:[-*+>|#]|\d+[.)]\s|<)")


def check_length(files):
    bad = []
    for path in files:
        if not path.startswith("docs/") or path.startswith("docs/meta/"):
            continue
        para, start = [], 0

        def flush():
            if para:
                if para[0].startswith(("Status:", "Code:", "Tests:", "Switch:")):
                    para.clear()
                    return
                text = prose_only(" ".join(para))
                count = len(SENTENCE_END.findall(text)) + 1
                if count > MAX_SENTENCES:
                    bad.append(f"{path}:{start}: paragraph of {count} sentences")
            para.clear()

        in_item = False
        for number, line in prose_lines(path):
            if not line.strip():
                flush()
                in_item = False
                continue
            if NOT_PROSE.match(line):
                flush()
                in_item = True  # a list item or table: indented lines continue it
                continue
            if in_item and line[0] == " ":
                continue
            in_item = False
            if line.startswith("    "):
                flush()
                continue
            if not para:
                start = number
            para.append(line.strip())
        flush()
    return bad


MAX_LINES = 600


def check_pagelen(files):
    bad = []
    for path in files:
        if not path.startswith("docs/") or path.startswith("docs/meta/"):
            continue
        with open(os.path.join(ROOT, path), encoding="utf-8") as f:
            count = sum(1 for _ in f)
        if count > MAX_LINES:
            bad.append(f"{path}:1: page of {count} lines (over {MAX_LINES}: split by topic)")
    return bad


def changed_pages():
    """Pages changed in this branch against origin/master, or uncommitted."""
    base = git_lines("merge-base", "HEAD", "origin/master")
    names = set(git_lines("diff", "--name-only", base[0], "--")) if base else set()
    names.update(git_lines("diff", "--name-only", "HEAD", "--"))
    names.update(git_lines("ls-files", "--others", "--exclude-standard"))
    return names


def main(argv):
    strict = "--strict" in argv
    wanted = [a.rstrip("/") for a in argv if not a.startswith("--")]
    all_docs = git_lines("ls-files", "docs/*.md", "docs/**/*.md")
    tracked = all_docs + git_lines(
        "ls-files", "README.md", "AGENTS.md", "CONTRIBUTING.md", "crates/*/*/README.md"
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
        ("switches", check_switches(), False),
        ("paths", check_paths(files), False),
        ("style", check_style(files), False),
    ]
    touched = changed_pages()
    soft = [("voice", check_voice(files)), ("length", check_length(files)), ("pagelen", check_pagelen(files))]
    failed = False
    for name, found, hard in checks:
        for line in found:
            print(f"{name}: {line}")
        print(f"{name}: {len(found)} finding(s)", file=sys.stderr)
        failed = failed or (bool(found) and (hard or strict))
    for name, found in soft:
        # A page changed in this branch must be clean; the rest are warnings.
        mine = [f for f in found if f.split(":", 1)[0] in touched]
        for line in found:
            print(f"{name}: {line}" if line in mine else f"{name}: warning: {line}")
        print(f"{name}: {len(found)} finding(s), {len(mine)} on changed pages", file=sys.stderr)
        failed = failed or bool(mine)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
