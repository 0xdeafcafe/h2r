# studio (`h2-editor`)

![h2-editor with Ascension open: Tools and the outliner on the left, the level in the 3D viewport, the selected weapon spawn's fields in the inspector](../../docs/images/editor/window.jpg)

The editor suite on Dear ImGui (docking) + wgpu + winit: the shell, the
scenario tool with a 3D viewport and gizmo, the game variant tool and the
playlist (hopper) tool. The package is `studio`; the binary is
`h2-editor`. Players start at
[docs/editor/editor-guide.md](../../docs/editor/editor-guide.md);
developers at [docs/editor/editor.md](../../docs/editor/editor.md).

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <dir>] [--data <dir>]
cargo run --release -p studio -- --disc <iso> --pick <map> [--select <block>:<index>] --screenshot out.png
cargo run --release -p studio -- --disc <iso> --tool variants --variant 3 --screenshot out.png
cargo run --release -p studio -- --disc <iso> --hopper default --playlist 2 --screenshot out.png
scripts/editor-screenshots.sh     # the user guide's screenshots, from your disc
```

Fonts in `assets/fonts`: Inter (SIL OFL 1.1, `Inter-LICENSE.txt`) and a
subset of Lucide icons (ISC, `Lucide-LICENSE.txt`).
