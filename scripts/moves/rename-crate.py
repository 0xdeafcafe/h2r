#!/usr/bin/env python3
"""Rename one crate in place: directory, package, dependency keys, Rust paths.

    scripts/moves/rename-crate.py <old> <new> [--dirty] [--quiet]

The crate stays in its domain directory. `scripts/moves/w2-hooks.py` is
this for `feature` and `hooks`; `rename_crate` in movelib.py has the
rules. Run it on a clean tree, after window 1. Cargo is not run.
"""
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402

if __name__ == "__main__":
    names = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(names) != 2:
        sys.exit(__doc__)
    sys.exit(movelib.rename_crate(names[0], names[1], f"rename {names[0]}"))
