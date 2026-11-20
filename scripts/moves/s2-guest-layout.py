#!/usr/bin/env python3
"""Window S2: `guest-layout` takes `engine`'s guest boundary and points
(docs/dev/crate-structure.md).

    scripts/moves/s2-guest-layout.py [--dirty] [--quiet]

`engine/src/{points, memory, handle, datum, view, tags, testing}.rs` move,
unchanged, to `guest-layout/src/`. `engine` re-exports each module and the
macros they export (`record!`, `points!`, `point_loc!`, `point_sig!`,
`point_ret!`), so `engine::points`, `engine::GuestMemory` and
`engine::record!` resolve as before and no other crate's Rust changes.
`guest-layout` names itself `guest_layout` (`extern crate self`), so the
moved files' `guest_layout::` paths, and `crate::` ones, resolve where
they land. The moved files need `paste`, which `guest-layout` gains.

`GuestMemory`'s `take_write_watch` is behind a `runtime-glue` feature,
which `guest-layout` gains with an optional `runtime` dependency;
`engine`'s `runtime-glue` turns it on. `engine/src/runtime_glue.rs`'s
`impl GuestMemory for Guest` and `for WritebackView` would break the
orphan rule in `engine` (both are foreign there now), so they move to
`guest-layout/src/memory.rs` under that feature.

`engine/src/entry.rs` stays in `engine`: `bots` and `agent` name
`engine::entry`, and `engine` cannot re-export it from `hooks`, which
depends on `engine`.

Every step is skipped when it is done already, so the script runs twice.
Cargo is not run.
"""
import os
import re
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import movelib  # noqa: E402
from movelib import read, write  # noqa: E402

ENGINE = "crates/rewrites/engine"
LAYOUT = "crates/hooking/guest-layout"
MODULES = ["datum", "handle", "memory", "points", "tags", "testing", "view"]
MACROS = ["point_loc", "point_ret", "point_sig", "record"]  # `points!` comes with the module's name

report = []


def edit(path, old, new):
    text = read(path)
    if text is None:
        report.append(f"MISSING {path}")
        return
    if old not in text:
        if new not in text:
            report.append(f"NOT FOUND in {path}: {old!r}")
        return
    write(path, text.replace(old, new, 1))


def main():
    w = movelib.Window("s2", [(f"{ENGINE}/src/{m}.rs", f"{LAYOUT}/src/{m}.rs") for m in MODULES])
    w.require_clean()
    w.mv()

    # guest-layout: the modules, the root names engine code uses, and its
    # own name for the moved `guest_layout::` paths.
    lib = f"{LAYOUT}/src/lib.rs"
    text = read(lib)
    if "extern crate self as guest_layout;" not in text:
        text = text.replace("#[macro_use]\nmod macros;",
                            "// The moved engine modules say `guest_layout::` (S2, docs/dev/crate-structure.md).\n"
                            "extern crate self as guest_layout;\n\n#[macro_use]\nmod macros;", 1)
    if "pub mod points;" not in text:
        block = ("\n// The guest boundary and the named points, from `engine`, which\n"
                 "// re-exports them (docs/dev/crate-structure.md, S2).\n"
                 + "".join(f"pub mod {m};\n" for m in MODULES)
                 + "\npub use handle::DatumHandle;\npub use memory::{FlatMemory, GuestMemory, MemFault};\n")
        text, n = re.subn(r"(?m)^mod mem;\n", lambda m: m.group(0) + block, text, count=1)
        if n != 1:
            report.append(f"NOT FOUND in {lib}: `mod mem;`")
    write(lib, text)

    edit(f"{LAYOUT}/Cargo.toml", "[dependencies]\n",
         "[dependencies]\n# Field names for the accessors `record!` generates (src/view.rs).\npaste = \"1\"\n")
    text = read(f"{LAYOUT}/Cargo.toml")
    if text.count("paste = ") > 1:
        report.append(f"COUNT {LAYOUT}/Cargo.toml: paste twice")

    edit(f"{LAYOUT}/Cargo.toml",
         "# No dependencies, on purpose: bots, telemetry, the agent API, the engine\n"
         "# and the native renderer all read guest structures, and none of them\n"
         "# should pull another to do it.\n",
         "# Few dependencies, on purpose: bots, telemetry, the agent API, the engine\n"
         "# and the native renderer all read guest structures, and none of them\n"
         "# should pull another to do it. `runtime` only behind `runtime-glue`.\n")
    edit(f"{LAYOUT}/Cargo.toml", "[dependencies]\n",
         "[features]\n# GuestMemory::take_write_watch and the runtime's memories as GuestMemory\n"
         "# (src/memory.rs); engine's `runtime-glue` turns it on.\nruntime-glue = [\"dep:runtime\"]\n\n"
         "[dependencies]\n")
    edit(f"{LAYOUT}/Cargo.toml", 'paste = "1"\n', 'paste = "1"\nruntime = { workspace = true, optional = true }\n')
    edit(f"{ENGINE}/Cargo.toml", 'runtime-glue = ["dep:runtime", "dep:mopp"]', 'runtime-glue = ["dep:runtime", "dep:mopp", "guest-layout/runtime-glue"]')

    # The two impls the orphan rule refuses in engine now.
    glue = f"{ENGINE}/src/runtime_glue.rs"
    text = read(glue)
    start = text.find("/// The runtime's guest is a [`GuestMemory`]")
    end = text.find("\nfn regs_of(")
    memory = f"{LAYOUT}/src/memory.rs"
    if start >= 0 and end > start:
        impls = text[start:end].rstrip("\n").replace(
            "has none); [`invoke`] keeps it", "has none); `engine::runtime_glue::invoke` keeps it")
        write(glue, text[:start] + text[end + 1:])
        body = "\n".join(("    " + l) if l else l for l in impls.split("\n"))
        write(memory, read(memory).rstrip("\n") + (
            "\n\n/// The runtime's memories as [`GuestMemory`] (feature `runtime-glue`).\n"
            "/// The impls sit with the trait: the trait and the types are both\n"
            "/// foreign to `engine`, whose `runtime_glue` used them first.\n"
            "#[cfg(feature = \"runtime-glue\")]\nmod runtime_memory {\n"
            "    use super::{GuestMemory, MemFault};\n    use runtime::Guest;\n\n"
            + body + "\n}\n"))
    elif "mod runtime_memory" not in (read(memory) or ""):
        report.append(f"NOT FOUND in {glue}: the `impl GuestMemory for Guest` block")

    # engine: the modules become re-exports.
    lib = f"{ENGINE}/src/lib.rs"
    text = read(lib)
    for m in MODULES:
        text = re.sub(rf"(?m)^pub mod {m};\n", "", text)
    if "pub use guest_layout::{" not in text:
        reexport = ("// The guest boundary and the named points live in guest-layout\n"
                    "// (docs/dev/crate-structure.md, S2); these keep `engine::` paths.\n"
                    f"pub use guest_layout::{{{', '.join(MODULES)}}};\n"
                    f"pub use guest_layout::{{{', '.join(MACROS)}}};\n\n")
        text, n = re.subn(r"(?m)^pub use handle::DatumHandle;\n", lambda m: reexport + m.group(0), text, count=1)
        if n != 1:
            report.append(f"NOT FOUND in {lib}: `pub use handle::DatumHandle;`")
    write(lib, text)
    edit(f"{ENGINE}/Cargo.toml", "# Field names for the accessors `record!` generates (src/view.rs).\npaste = \"1\"\n", "")
    edit(f"{ENGINE}/Cargo.toml", "# Guest structure layouts and the globals table (src/points.rs re-exports\n# the globals; src/view.rs takes its offsets).\n",
         "# Guest structure layouts, the globals table, the guest boundary and the\n# named points; src/lib.rs re-exports the modules.\n")

    # The view module's example runs as guest-layout's doctest now.
    view = f"{LAYOUT}/src/view.rs"
    edit(view, "//! engine::record! {", "//! guest_layout::record! {")
    edit(view, "//!         position @ 0x4: engine::view::Vec3,", "//!         position @ 0x4: guest_layout::view::Vec3,")
    edit(view, "//! # use engine::{GuestMemory, testing::FakeGuest};", "//! # use guest_layout::{GuestMemory, testing::FakeGuest};")

    w.relative_paths()
    w.mentions()
    w.stage()
    w.report += report
    return w.finish(old_dirs=[])


if __name__ == "__main__":
    sys.exit(main())
