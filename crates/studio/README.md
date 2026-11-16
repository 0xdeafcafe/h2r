# studio (`h2-studio`)

The editor suite on Dear ImGui (docking) + wgpu + winit, successor to the
egui `h2-editor`. Phase 1: the shell and the scenario tool with a 3D
viewport and gizmo. See [docs/editor/studio.md](../../docs/editor/studio.md).

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <dir>] [--data <dir>]
cargo run --release -p studio -- --disc <iso> --pick <map> --screenshot out.png
```

Fonts in `assets/fonts`: Inter (SIL OFL 1.1, `Inter-LICENSE.txt`) and a
subset of Lucide icons (ISC, `Lucide-LICENSE.txt`).
