# crates/runtime

The machine the lifted code runs on: guest memory and CPU state, the
kernel, the filesystem, snapshots, the host fault handler, timeline
events, and the build-and-load driver that turns an XBE into a running
module. A crate here may depend on `formats` and `recompiler` and on other
runtime crates (`scripts/crate-domains.toml`, checked by
`scripts/crate-graph.py layers --check`). `platform`, `rewrites` and the
apps sit above it and use it. See
[docs/dev/crate-structure.md](../../docs/dev/crate-structure.md) and
[docs/runtime/BOOT.md](../../docs/runtime/BOOT.md).

| Crate | What |
|---|---|
| [hostfault](hostfault/) | Process-wide host fault handler: regions and fixup tables |
| [kernel](kernel/) | The Xbox kernel's services over the runtime |
| [lifted](lifted/) | The lifted original on the IR interpreter, the oracle for ports |
| [recomp](recomp/) | Build and load: lift, emit, compile, load, and the interpreter fallback |
| [runtime](runtime/) | Runtime ABI: guest memory, CPU state, dispatch, loader and runner |
| [snapshot](snapshot/) | Versioned snapshot container, RAM page codec and store |
| [timeline](timeline/) | Near-zero-cost Chrome/Perfetto timeline events and frame counter |
| [vfs](vfs/) | Xbox object-manager storage: disc, host, package and override backends |
