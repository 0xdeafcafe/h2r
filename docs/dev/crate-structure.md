# Crate structure: domains, and the windows that move to them

Status: Built. The workspace uses domain directories and checks their dependencies.
Code: [crate-domains.toml](../../scripts/crate-domains.toml), [crate-graph.py](../../scripts/crate-graph.py), [workspace manifest](../../Cargo.toml).
Switch: none.

A Cargo package lives in `crates/<domain>/<crate>`. The directory
assigns its domain. One workspace glob per domain makes the manifest
discoverable. The generated [crate table](../ARCHITECTURE.md#crates)
lists the packages that exist. The domain rules can also reserve names
for designs, so the rules file alone is not a package inventory.

## The domains

| Domain | Owns |
|---|---|
| `formats` | Disc, tag and file formats, including settings, variants and Forge documents |
| `recompiler` | XBE analysis, x86 lifting, IR, the C and Cranelift backends and CPU oracles |
| `runtime` | CPU state, guest memory, scheduling, filesystems, snapshots and image build/load |
| `platform` | Host implementations of Xbox platform services, including graphics, audio and input |
| `hooking` | Hook plans, guest layout and entry points, and implementation switches |
| `rewrites` | Native simulation and rendering subsystems and their guest bridges |
| `live` | Live service and client, co-op transport, bots, film containers, telemetry and cloud saves |
| `extensions` | In-game features including menus, bot glue, Theater, and the agent API |
| `tools` | Offline editors and converters, Forge materialisation, profiling and workspace checks |
| `apps` | The game composition root, player shell, command line, editor and embedded hosts |

### What each domain may use

A package may depend on its own domain and the domains in the `uses`
list in `scripts/crate-domains.toml`. The check applies to normal and
build dependencies. Tests can use dev-dependencies from other domains.

The `guest_free` lists impose a tighter boundary: a guest-free crate
may use `formats`, `platform` and other guest-free crates in any domain.
For example, `frame-scene` and `native-render` consume typed frame data,
and `background` and `timeline` are guest-free runtime utilities. The
runtime and hook dependencies sit in `render-bridge`; `scene-extract`,
which reads guest offsets, is not guest-free. The types `d3d` and
`frame-scene` share sit below both in `crates/platform/frame-types`.
The rules file lists each dependency exception and the condition for
removing it.

### The check

```sh
python3 scripts/crate-graph.py layers
python3 scripts/crate-graph.py layers --check
```

The check reads manifests through `scripts/cratemeta.py`. It reports an
unclaimed package, a package in the wrong directory, a refused
dependency or an unused exception.

### W3. The Halo 1 merge

`crates/tools/h1map` reads Halo 1 maps. `crates/tools/h1-assets`,
`crates/tools/h1objects`, `crates/tools/h1convert` and
`crates/tools/h1campaign` handle conversion. `crates/formats/h1-contract`
names the markers and script slots the converters and the game's
`crates/extensions/h1-runtime` hooks both read.

### S2. `guest-layout` takes `engine`'s guest boundary and points

`crates/hooking/guest-layout` owns the guest's typed layouts and hook
points, the async entries, the override adapters and their runtime
registration, the locale table and the point shapes. `hooks` depends on
it, not on `engine`. `crates/rewrites/engine` re-exports the paths its
callers use.

### S3. `hooks` gives away what is not the hook engine

`crates/hooking/hooks` builds and installs hook plans and reads
implementation switches. The render and simulation bridges are
`crates/rewrites/render-bridge` and `crates/rewrites/sim-bridge`.

### S4. `recomp` out of `boot`

`crates/runtime/recomp` builds, selects and loads game modules.
`crates/apps/boot` supplies the services those modules run against, and
registers the retail image's hooked render sites with the builder
(`boot::register_image_entries`), so `recomp` names no hook point.

### S5. `boot` keeps the root

The host composition and registration are in `crates/apps/boot/src/lib.rs`
and `crates/apps/boot/src/registration.rs`. In-game extensions and their
UI helpers live in `crates/extensions/game`. Command implementations
live with the `h2` binary in `crates/apps/cli`.

## Local registration

`crates/apps/boot/src/registration.rs` assembles the feature registry.
Features declare their hooks through `hooks::Feature` and
`hooks::Builder` in `crates/hooking/hooks/src/lib.rs`.
`Registry::build` checks the plan, and the chain orders operations at
their named hook points. See [the feature engine](../engine/feature-engine.md).

## Gates

`crates/tools/xtask` owns the local check and test workflow.
[Build speed](build-speed.md) describes its inputs, test tiers and
shared ledger. CI jobs are defined in
[.github/workflows/ci.yml](../../.github/workflows/ci.yml): the `repo checks`
job runs `crate-graph.py layers --check`, `crate-table.py --check` and
`docs-check.py` with python3 alone, and the macOS and Linux jobs run
the pinned compiler from `rust-toolchain.toml`
(`scripts/ci/install-rust.sh`), clippy with `-D warnings` on macOS and
`cargo check --tests` on Linux.

## The crate table

`scripts/crate-table.py` renders the table in
[ARCHITECTURE.md](../ARCHITECTURE.md#crates) from the package manifests.
It reads the description and the `doc` and `status` fields under
`package.metadata.h2`. Update the package metadata when adding a crate
or changing its purpose, then regenerate the table. The script's
`--check` mode checks the generated table.

## Related

[Architecture](../ARCHITECTURE.md), [developer checks](build-speed.md),
[feature engine](../engine/feature-engine.md).
