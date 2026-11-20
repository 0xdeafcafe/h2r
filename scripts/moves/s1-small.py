#!/usr/bin/env python3
"""Window S1: small renames and deletions (docs/dev/crate-structure.md).

    scripts/moves/s1-small.py [--dirty] [--quiet]

  - `sim` becomes `sim-bench` (`crates/live/sim-bench`): the headless
    match harness, which is not the simulation. `agent`'s `sim-report`
    alias follows.
  - `apple-host` becomes `h2host`, its library's name already.
  - `fastmem-proto` is deleted.
  - `oracle`'s WMA oracle (`src/wma.rs`, its disc test and its example)
    moves into `wma` behind an `oracle` feature, which clears the
    `oracle` -> `wma` exception.

`rename_crate` in movelib.py does the renames. Every step is skipped when
it is done already, so the script runs twice. Cargo is not run: the next
cargo command rewrites `Cargo.lock` for the new and removed packages.
"""
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import git, read, tracked, write  # noqa: E402

ORACLE = "crates/recompiler/oracle"
WMA = "crates/platform/wma"
RULES = "scripts/crate-domains.toml"

report = []


def edit(path, old, new, count=1):
    """One fixed replacement; already-applied (new present, old gone) is fine."""
    text = read(path)
    if text is None:
        report.append(f"MISSING {path}")
        return
    n = text.count(old)
    if n == 0:
        if new not in text:
            report.append(f"NOT FOUND in {path}: {old!r}")
        return
    if new and new in text and old in new:
        return  # an insertion that has run
    if count is not None and n != count:
        report.append(f"COUNT {path}: {old!r} found {n} times, expected {count}")
    write(path, text.replace(old, new))


def renames():
    code = movelib.rename_crate("sim", "sim-bench", "s1")
    sys.argv.append("--dirty")  # the next rename runs on this one's output
    # `package = "sim"` behind another key is not a dependency key.
    edit("crates/extensions/agent/Cargo.toml", 'package = "sim", path', 'package = "sim-bench", path')
    # The review records the name it proposed changing.
    edit("docs/reviews/2026-11-20-architecture.md",
         "`crates/live/sim-bench` is not the simulation. It is the headless match harness\n"
         "(`crates/live/sim-bench/src/lib.rs:1-9`).",
         "`crates/live/sim` is not the simulation. It is the headless match harness\n"
         "(`crates/live/sim/src/lib.rs:1-9`).")

    code = movelib.rename_crate("apple-host", "h2host", "s1") or code
    # `-p apple-host` where the command rewrite does not look (Rust and
    # manifest comments).
    for path in ("crates/apps/h2host/Cargo.toml", "crates/apps/h2host/examples/pack.rs"):
        edit(path, "-p apple-host", "-p h2host")
    edit("crates/apps/boot/src/setting_defs.rs", "read live by `apple-host`", "read live by `h2host`")
    # The apps list named both.
    rules = read(RULES)
    out = rules.replace('"app", "app-icon", "h2host", "boot"', '"app", "app-icon", "boot"')
    if out != rules:
        write(RULES, out)
    return code


def fastmem():
    if tracked("crates/recompiler/fastmem-proto"):
        git("rm", "-r", "-q", "crates/recompiler/fastmem-proto")
    edit(RULES, '"emit", "fastmem-proto", "ir"', '"emit", "ir"')


def wma_oracle():
    moves = [
        (f"{ORACLE}/src/wma.rs", f"{WMA}/src/oracle.rs"),
        (f"{ORACLE}/tests/wma_disc.rs", f"{WMA}/tests/oracle_disc.rs"),
        (f"{ORACLE}/examples/wma_oracle.rs", f"{WMA}/examples/wma_oracle.rs"),
    ]
    w = movelib.Window("s1", moves)
    w.mv()
    w.mentions()
    report.extend(w.report)

    # oracle: no WMA, so no wma, asf or anyhow.
    edit(f"{ORACLE}/Cargo.toml", 'wma = ["unicorn"]\n', "")
    edit(f"{ORACLE}/Cargo.toml", '"dep:unicorn-engine", "dep:wma", "dep:asf", "dep:anyhow"]', '"dep:unicorn-engine"]')
    edit(f"{ORACLE}/Cargo.toml", 'wma = { path = "../../platform/wma", optional = true }\n', "")
    edit(f"{ORACLE}/Cargo.toml", 'asf = { path = "../../formats/asf", optional = true, features = ["disc"] }\n', "")
    edit(f"{ORACLE}/Cargo.toml", "anyhow = { workspace = true, optional = true }\n", "")
    edit(f"{ORACLE}/Cargo.toml", '\n[[example]]\nname = "wma_oracle"\nrequired-features = ["unicorn"]\n', "")
    edit(f"{ORACLE}/src/lib.rs", '\n#[cfg(feature = "unicorn")]\npub mod wma;\n', "\n")

    # wma: the oracle behind its own feature.
    edit(f"{WMA}/Cargo.toml", "synthetic = []\n",
         "synthetic = []\n"
         "# The game's own decoder under Unicorn, the reference this one is checked\n"
         "# against (src/oracle.rs, tests/oracle_disc.rs, examples/wma_oracle.rs).\n"
         "# A validation tool on the owner's disc, never the playback backend.\n"
         'oracle = ["dep:unicorn-engine", "dep:anyhow"]\n')
    edit(f"{WMA}/Cargo.toml", "snapshot = { path = \"../../runtime/snapshot\" }\n",
         "snapshot = { path = \"../../runtime/snapshot\" }\n"
         "anyhow = { workspace = true, optional = true }\n"
         'unicorn-engine = { version = "=2.1.5", optional = true, default-features = false, features = ["arch_x86"] }\n')
    text = read(f"{WMA}/Cargo.toml")
    if "[[example]]" not in text:
        write(f"{WMA}/Cargo.toml", text.rstrip("\n") + '\n\n[[example]]\nname = "wma_oracle"\nrequired-features = ["oracle"]\n')
    edit(f"{WMA}/src/lib.rs", "pub mod diagnostics;\n",
         'pub mod diagnostics;\n#[cfg(feature = "oracle")]\npub mod oracle;\n')
    edit(f"{WMA}/src/oracle.rs", "image: &::wma::VerifiedImage<'_>", "image: &crate::VerifiedImage<'_>")
    edit(f"{WMA}/examples/wma_oracle.rs", "oracle::wma::Decoder::new", "wma::oracle::Decoder::new")
    test = f"{WMA}/tests/oracle_disc.rs"
    edit(test, '#![cfg(feature = "unicorn")]', '#![cfg(feature = "oracle")]')
    edit(test, "`cargo test -p oracle --features unicorn --release --test wma_disc -- --ignored`",
         "`cargo test -p wma --features oracle --release --test oracle_disc -- --ignored`")
    edit(test, "oracle::wma::Decoder::new", "wma::oracle::Decoder::new")
    edit(f"{WMA}/examples/native_pcm.rs", "Native counterpart of oracle/examples/wma_oracle.rs",
         "Native counterpart of examples/wma_oracle.rs")

    # The exception the move clears.
    rules = read(RULES)
    out = re.sub(r'\n\[\[exception\]\]\nfrom = "oracle"\nto = "wma"\nuntil = "[^"]*"\n', "\n", rules)
    if out != rules:
        write(RULES, out)


def main():
    code = renames()
    fastmem()
    wma_oracle()
    git("add", "-u")
    git("add", "--", f"{WMA}/src/oracle.rs", f"{WMA}/tests/oracle_disc.rs", f"{WMA}/examples/wma_oracle.rs")
    for line in report:
        print(f"  s1: {line}")
    bad = [r for r in report if r.startswith(("NOT FOUND", "COUNT", "MISSING", "UNRESOLVED"))]
    return code or (1 if bad else 0)


if __name__ == "__main__":
    sys.exit(main())
