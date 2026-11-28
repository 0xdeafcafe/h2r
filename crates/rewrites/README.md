# crates/rewrites

The rewrites domain holds Rust that replaces a guest subsystem: ported engine functions, the native simulation, FrameScene and the native renderer, and the bridge crates that install each one behind hooks. The pure crates (`sim-core`, `sim-anim`, `sim-collision`, `sim-objects`, `sim-sound`, `sim-state`, `sim-world`, `frame-scene`, `native-render`) take typed values and use no hook and no lifted code; `scene-extract` reads guest offsets to fill a FrameScene. The bridges (`sim-bridge`, `render-bridge`) are the crates here that use `hooking`, and `engine` holds the adapters over guest memory. A crate here may depend on its own domain and on `formats`, `recompiler`, `runtime`, `platform` and `hooking`; `live`, `extensions`, `tools` and `apps` may depend on it. The rules are checked by `python3 scripts/crate-graph.py layers --check` and set out in [crate-structure](../../docs/dev/crate-structure.md).

| Crate | What it is |
|---|---|
| [engine](engine/README.md) | Hand-written Rust replacements for engine functions, over guest memory. |
| [sim-core](sim-core/README.md) | Shared math, exact SSE-order lanes and float rules for the native simulation. |
| [sim-anim](sim-anim/README.md) | Native animation sampling. |
| [sim-collision](sim-collision/README.md) | The native collision world and its queries. |
| [sim-objects](sim-objects/README.md) | The object update: typed pool views and object kernels. |
| [sim-script](sim-script/README.md) | HaloScript threads on the world: the scripts section and the per-tick update. |
| [sim-sound](sim-sound/README.md) | Sound scheduling, listener, environment and mixer parameters. |
| [sim-world](sim-world/README.md) | The World: salted slot pools, canonical save and restore, hash and RNG. |
| [sim-state](sim-state/README.md) | Read-only typed views of the simulation for theater, stats, Live and the agent API. |
| [sim-bridge](sim-bridge/README.md) | The guest side of the native simulation: installs, tick driver and shadow checks. |
| [frame-scene](frame-scene/README.md) | FrameScene: the owned, typed snapshot of a rendered frame. |
| [scene-extract](scene-extract/README.md) | Guest memory to FrameScene; the only render code with guest offsets. |
| [native-render](native-render/README.md) | The native GPU renderer, drawn from a FrameScene. |
| [render-bridge](render-bridge/README.md) | The guest side of the native renderer: stage hooks and the frame step. |
