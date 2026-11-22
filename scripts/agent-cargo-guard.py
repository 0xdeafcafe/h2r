#!/usr/bin/env python3
"""PreToolUse hook for coding agents: cargo goes through the shared caches.

Every Bash command an agent runs in any h2recomp worktree passes through
here (.claude/settings.json). It refuses the cargo uses that skip the
machine-wide caches:

- `cargo test` / `cargo nextest`: tests run through `cargo xtask test`,
  which tests only the affected packages and records passes in the shared
  ledger, so a package whose inputs already passed anywhere is skipped.
- `cargo install`: a fresh throwaway target every time. Run `cargo h2 ...`
  from the worktree instead.
- Setting RUSTC_WRAPPER, CARGO_BUILD_RUSTC_WRAPPER, SCCACHE_DIR or
  CARGO_TARGET_DIR on the command: kache is the machine-wide rustc wrapper
  (~/.cargo/config.toml) and each worktree's target is its own.

Exit 2 blocks the command and shows the reason to the agent
(docs/dev/build-speed.md, "6. Agents and the caches").

Before an allowed cargo command, a worktree with no `target` yet gets one:
the machine-local `target-on-builds.sh` and `seed-target.py` in the
repository's hooks directory link it to the Builds volume and clone the
closest warm target in. Worktrees made without `git worktree add`'s
post-checkout hook (an agent harness's own) would otherwise build cold.
Missing scripts or any error leave the command to run as it is.
"""

import json
import os
import re
import shlex
import subprocess
import sys

RULES = [
    (
        re.compile(r"(?:^|[\s;&|(])cargo\s+(?:\+\S+\s+)?(?:test|nextest)\b"),
        "use `cargo xtask test -p <crate> ...` (affected packages only, shared "
        "test ledger) instead of `cargo test`; `cargo xtask test` with no -p "
        "tests what the branch changed",
    ),
    (
        re.compile(r"(?:^|[\s;&|(])cargo\s+(?:\+\S+\s+)?install\b"),
        "don't `cargo install`: it rebuilds everything in a throwaway target; "
        "run `cargo h2 <command>` from the worktree, which builds h2 "
        "incrementally in the worktree's own target",
    ),
    (
        re.compile(
            r"(?:^|[\s;&|(])(?:export\s+)?"
            r"(RUSTC_WRAPPER|CARGO_BUILD_RUSTC_WRAPPER|SCCACHE_DIR|CARGO_TARGET_DIR)="
        ),
        "don't set {0}: kache is the machine-wide rustc wrapper "
        "(~/.cargo/config.toml) and each worktree keeps its own target",
    ),
]


def check(command: str):
    """The reason `command` is refused, or None."""
    for pattern, reason in RULES:
        m = pattern.search(command)
        if m:
            return reason.format(*m.groups())
    return None


CARGO = re.compile(r"(?:^|[\s;&|(])cargo\s")
LEADING_CD = re.compile(r"^\s*cd\s+(\S+)\s*(?:&&|;)")


def worktree_dir(command: str, cwd: str):
    """The directory a cargo command runs in: a leading `cd X &&` or cwd."""
    m = LEADING_CD.match(command)
    if m:
        try:
            d = os.path.expanduser(shlex.split(m.group(1))[0])
        except ValueError:
            d = m.group(1)
        return os.path.join(cwd, d) if cwd else d
    return cwd


def prepare_target(directory: str) -> None:
    """Links and seeds the worktree's target when it has none yet."""
    def git(*args):
        return subprocess.run(["git", "-C", directory, *args], capture_output=True, text=True, timeout=10).stdout.strip()

    top = git("rev-parse", "--show-toplevel")
    if not top or os.path.lexists(os.path.join(top, "target")):
        return
    if not os.path.isfile(os.path.join(top, "Cargo.toml")):
        return
    hooks = os.path.join(git("rev-parse", "--path-format=absolute", "--git-common-dir"), "hooks")
    link = os.path.join(hooks, "target-on-builds.sh")
    seed = os.path.join(hooks, "seed-target.py")
    if os.access(link, os.X_OK):
        subprocess.run([link, top], cwd=top, timeout=60, stdout=subprocess.DEVNULL)
    if os.access(seed, os.X_OK):
        subprocess.run([seed, top], cwd=top, timeout=300, stdout=subprocess.DEVNULL)


def main() -> int:
    try:
        event = json.load(sys.stdin)
    except ValueError:
        return 0
    command = (event.get("tool_input") or {}).get("command") or ""
    reason = check(command)
    if reason is None:
        if CARGO.search(command):
            try:
                prepare_target(worktree_dir(command, event.get("cwd") or os.getcwd()))
            except Exception:
                pass
        return 0
    print(f"Blocked by scripts/agent-cargo-guard.py: {reason}.", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
