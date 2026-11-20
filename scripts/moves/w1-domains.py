#!/usr/bin/env python3
"""Window 1: every crate into its domain directory.

    scripts/moves/w1-domains.py [--list] [--dirty] [--quiet]

`crates/<crate>` becomes `crates/<domain>/<crate>`, by the `crates` lists
in scripts/crate-domains.toml. Nothing is renamed, split or merged, so
git sees every file as a rename. Then the paths that said where a crate
is are rewritten: `path = ` in every Cargo.toml, `[workspace] members`,
relative paths that leave a crate, and `crates/<crate>` in docs, scripts
and comments. --list prints the `git mv` lines and stops.

Run it on a clean tree at `move-w1-pre` (docs/dev/crate-structure.md,
"How a window runs"). Cargo is not run; `Cargo.lock` holds no paths and
does not change.
"""
import os
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import ROOT, cratemeta  # noqa: E402


def main():
    rule_set = cratemeta.rules(ROOT)
    domains = rule_set["domains"]
    planned = cratemeta.planned_domains(rule_set)
    moves, unknown = [], []
    for crate_dir in movelib.crate_dirs():
        parts = crate_dir.split("/")
        if len(parts) != 2:
            continue  # nested in another crate (crates/runtime/test-probe) or already in a domain
        name = parts[1]
        if name not in planned:
            unknown.append(name)
            continue
        moves.append((crate_dir, f"crates/{planned[name]}/{name}"))
    if unknown:
        sys.exit("w1: no domain for: " + ", ".join(unknown) + f"\nadd each to a `crates` list in {cratemeta.RULES}")
    if "--list" in sys.argv:
        for old, new in moves:
            print(f"git mv {old} {new}")
        return 0

    # A crate that has not landed yet moves nowhere, but a doc that names
    # it should name where it will be.
    here = {old.split("/")[1] for old, _ in moves}
    aliases = [(f"crates/{name}", f"crates/{domain}/{name}") for name, domain in planned.items() if name not in here]
    w = movelib.Window("w1", moves, aliases)
    w.require_clean()
    w.mv()
    w.workspace_members([f"crates/{d}/*" for d in domains])
    w.manifests()
    w.relative_paths()
    w.mentions()

    # Paths the rules cannot see: a shell variable in front, a glob, or a
    # real path inside a Rust string.
    for script in ("crates/live/live-worker/deploy.sh", "crates/live/live-worker/smoke/run.sh"):
        w.literal(script, '"$here/../.."', '"$here/../../.."', 1)
    w.literal("crates/live/live-worker/wrangler.toml", '"./../../deploy/host/Dockerfile"', '"./../../../deploy/host/Dockerfile"', 8)
    w.literal("scripts/docs-check.py", '"crates/*/README.md"', '"crates/*/*/README.md"', 1)
    w.literal("crates/apps/cli/src/main.rs", '"crates/runtime/include/h2abi.h"', '"crates/runtime/runtime/include/h2abi.h"', 1)
    w.literal("crates/apps/studio/src/play.rs", "--path crates/cli)", "--path crates/apps/cli)", 1)
    w.literal("crates/apps/cli/src/shader.rs", "through crates/shaders and", "through crates/formats/shaders and", 1)
    w.literal("crates/apps/boot/src/stopgaps.rs", "(crates/boot sha.rs)", "(crates/apps/boot sha.rs)", 1)

    w.stage()
    return w.finish(old_dirs=[old for old, new in moves if not new.startswith(old + "/")])


if __name__ == "__main__":
    sys.exit(main())
