# studio (`h2-studio`)

The editor suite on Dear ImGui (docking) + wgpu + winit, successor to the
egui `h2-editor`: the shell, the scenario tool with a 3D viewport and
gizmo, the game variant tool and the playlist (hopper) tool. See [docs/editor/studio.md](../../docs/editor/studio.md).

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <dir>] [--data <dir>]
cargo run --release -p studio -- --disc <iso> --pick <map> --screenshot out.png
cargo run --release -p studio -- --disc <iso> --tool variants --variant 3 --screenshot out.png
cargo run --release -p studio -- --disc <iso> --hopper default --playlist 2 --screenshot out.png
```

Fonts in `assets/fonts`: Inter (SIL OFL 1.1, `Inter-LICENSE.txt`) and a
subset of Lucide icons (ISC, `Lucide-LICENSE.txt`).
