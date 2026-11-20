#!/usr/bin/env python3
"""Window 2: the `feature` crate becomes `hooks`.

    scripts/moves/w2-hooks.py [--dirty] [--quiet]

`crates/hooking/feature` moves to `crates/hooking/hooks`; the package,
every dependency key (`feature = { path = ... }`, `"feature/native-render"`),
every `feature::` path in Rust, `-p feature` in docs and the exception
list in scripts/crate-domains.toml follow. Cargo features, `#[cfg(feature
= "...")]` and the word in prose are not touched.

Run it on a clean tree at `move-w2-pre`, after window 1. Cargo is not
run: the next cargo command rewrites `Cargo.lock` for the new name.

`rename_crate` in movelib.py is the whole of it, and renames any other
crate the same way (`sim` to `sim-bench`, `apple-host` to `h2host`).
"""
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402


def main():
    code = movelib.rename_crate("feature", "hooks", "w2")
    # The profile attribution reads symbol names, which carry the crate's.
    w = movelib.Window("w2", [])
    w.literal("scripts/sim-attribution/attribute.py", '"feature::"', '"hooks::"')
    movelib.git("add", "-u")
    for line in w.report:
        print(f"  {line}")
    return code or (1 if w.report else 0)


if __name__ == "__main__":
    sys.exit(main())
