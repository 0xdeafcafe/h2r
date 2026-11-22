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
"""

import json
import re
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


def main() -> int:
    try:
        event = json.load(sys.stdin)
    except ValueError:
        return 0
    command = (event.get("tool_input") or {}).get("command") or ""
    reason = check(command)
    if reason is None:
        return 0
    print(f"Blocked by scripts/agent-cargo-guard.py: {reason}.", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
