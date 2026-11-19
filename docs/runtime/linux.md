## Headless host build

From the repository root, cross-build the Linux x86-64 `h2` binary with
cargo-zigbuild and Zig.

```sh
cargo zigbuild -p cli --no-default-features --features host --target x86_64-unknown-linux-gnu
```

The binary supports `h2 serve --system-link --host-light`, `h2 host run`,
`h2 sim` and `h2 agent`. Supply the matching Linux `libh2game.so` and the
owner's game data separately. See the
[dedicated-host build contract](../live/dedicated-host-build.md#10-linux-x86-64-build-owned-by-dh-linux-).
Keep `H2_OPT` consistent with the module build.

The `host` feature keeps the software D3D recorder, the DirectSound clock,
the Bink movie-start skip stub and the guest input model (including bots
and agent-injected input). The default `desktop` feature adds the windowed
app, wgpu presenter and native GPU passes, physical controllers, cpal
devices, movie playback, app icon and editor client commands.

### Game module ABI

Boot opens `libh2game.so` with `RTLD_NOW | RTLD_LOCAL`. The module defines
the generated code's `h2abi_v1_*`, `h2shim_v1_*` and `h2x87_v1_*` shims as
hidden ELF symbols. The Rust runtime supplies their C ABI callbacks by
address through the exported `h2boot_install_v3` and `h2x87bridge_install`
functions. This is the same callback handoff used on macOS. The bridge needs
no export from the `h2` executable. A future direct host import would need
the exact C ABI symbol in the executable's dynamic symbol table, via a
selective export or `-rdynamic` on the final link.
