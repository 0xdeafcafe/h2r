# crates/tools

The tools domain is offline work on game data: the Halo 1 readers and converters, the UI-free cores of the editor's tools, the profiler and the model viewer. Nothing here hooks a running guest. A crate here may depend on its own domain and on `formats`, `recompiler`, `runtime`, `platform`, `rewrites` and `live`; only `apps` may depend on it. Tests that read a disc are `#[ignore]`d and take the disc paths from `H2_DISC` and `H1_ISO`. The rules are checked by `python3 scripts/crate-graph.py layers --check` and set out in [crate-structure](../../docs/dev/crate-structure.md).

| Crate | What it is |
|---|---|
| [h1map](h1map/README.md) | Halo 1 Xbox cache file reader. |
| [h1-assets](h1-assets/README.md) | Halo 1 bitmaps, models, sounds, skies and text as Halo 2 tags. |
| [h1objects](h1objects/README.md) | Halo 1 sandbox objects as Halo 2 tags. |
| [h1convert](h1convert/README.md) | Halo 1 multiplayer maps converted to Halo 2 tags. |
| [h1campaign](h1campaign/README.md) | Halo 1 campaign levels as Halo 2 v8 maps. |
| [editor-core](editor-core/README.md) | The UI-free parts of `h2-editor` shared by its tools. |
| [scenario-edit](scenario-edit/README.md) | The scenario placement editor: typed blocks, undo, patched `.map` output. |
| [hopper-edit](hopper-edit/README.md) | The playlist editor's UI-free logic. |
| [hud-edit](hud-edit/README.md) | The HUD editor's UI-free logic. |
| [mesh-edit](mesh-edit/README.md) | The modeling kernel: an editable radial-edge mesh, its operations and stable export. |
| [perf](perf/README.md) | Sampling profiler, timeline capture and the benchmark harness. |
| [viewer](viewer/README.md) | Render-model viewer and the offline BSP tools. |
