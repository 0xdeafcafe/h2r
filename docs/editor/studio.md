# h2-studio (Dear ImGui editor suite)

`h2-studio` (`crates/studio`) is the successor to the egui `h2-editor`
([editor.md](editor.md)): the same tools on Dear ImGui's docking branch,
with a game-tool look (Unreal, RenderDoc, Tracy). It is a native Rust app:
`dear-imgui-rs` for the UI, `dear-imgui-wgpu` on the workspace's wgpu 30,
`dear-imgui-winit` for the window, `dear-imguizmo` for the 3D gizmo. No web
views, no scripting.

The move happens in three phases. Until the last, `h2-editor` keeps
building and working, and stays the tool for whatever the studio lacks.

| Phase | What | State |
| --- | --- | --- |
| 1 | Shell, scenario tool, 3D viewport, headless screenshots | this page |
| 2 | Game variants and playlists (hoppers) | `h2-editor` has them |
| 3 | Guide screenshots with real assets, the user guide, retiring egui (`h2-editor` moves to the studio) | to do |

Nothing in the studio contains game data: maps come from your own disc at
run time.

## Running it

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <maps dir>] [--data <dir>]
```

With no source it opens `H2_DISC`, else `disc/halo2.iso` when it exists
(`editor_core::default_disc`). `--data` is the runtime's data root, as for
`h2 play`; map packages go to `<data>/maps`. The dock layout persists in
`<data>/h2-studio.ini` (View > Reset layout restores the default).

## The window

- **Main menu**: File (open a disc or map, open a folder of maps, save the
  map as a new `.map`, save as or open a package, quit), Edit (undo, redo,
  duplicate, delete, allow growing full blocks), View (frame the map,
  focus the selection, gizmo mode and axes, reset layout).
- **Tools** (left, top): the tools, then the opened source and its maps.
  Variants and playlists are listed but disabled until phase 2.
- **Outliner** (left, bottom): the placement blocks as a tree, a filter,
  Duplicate and Delete. Right-click an entry for Duplicate, Delete and
  Add; right-click a block for Add. A button is disabled, with the reason
  as its tooltip, when the editor refuses (docs/editor/scenario-editor.md
  §6). Double-click an entry to focus the camera on it.
- **Viewport** (centre): the map's structure BSP and a marker per
  placement, drawn by `scenario_edit::render::Viewport` into an offscreen
  texture that ImGui shows as an image. Left click picks; right-drag
  orbits, with WASD and QE flying while it is held; middle-drag pans; the
  wheel zooms. The selection has an ImGuizmo gizmo: W moves, E rotates
  (facing-only placements turn about z); one drag is one undo step.
- **Inspector** (right): the selection's typed fields. The palette index
  is a combo of the palette's tag names, tag references a combo of the
  map's item tags; positions, rotations (in degrees) and scale are drags;
  flags and other integers are bounded drags. Read-only fields are
  disabled; each field's tooltip gives its offset and evidence.
- **Log** (bottom): every action's result, errors in red.
- **Status bar**: the last message, unsaved state, the map's triangle and
  placement counts, frame rate.
- **Unsaved changes**: opening another source or map, or quitting, with
  unsaved edits asks first: Save as..., Discard or Cancel.

Shortcuts: Cmd/Ctrl+O open, Cmd/Ctrl+S save map as, Cmd/Ctrl+Z undo,
Shift+Cmd/Ctrl+Z or Cmd/Ctrl+Y redo, Cmd/Ctrl+D duplicate, Delete delete,
F focus, Home frame all, W/E gizmo mode, Cmd/Ctrl+Q quit. Dropping a disc
image, map or folder on the window opens it.

## Code

- `crates/editor-core`: what both editors share and no UI touches: map
  sources (`MapSource`: a disc image, a loose `.map` with its siblings, or
  a folder of maps), the snapshot `UndoStack` (coalescing by `u64` keys),
  `Status`.
- `crates/scenario-edit`: `Session` holds the scenario editor's state and
  every command (open, select, add, remove, plane drags, gizmo gestures,
  undo, save, packages) with no UI. The egui `panel` (feature `egui`, on
  by default) derefs to it; the studio builds the crate without egui.
  `render` and `geometry` are UI-free too (the camera gives separate view
  and projection matrices for ImGuizmo; `geometry::decompose` turns a
  gizmo matrix back into position and yaw/pitch/roll).
- `crates/studio`:
  - `app.rs`: `App`, the state and `Action`s (open, select map, save,
    packages, undo/redo, select, delete, duplicate, add, quit), with the
    unsaved-changes guard (`dispatch` holds a discarding action;
    `resolve` answers). Tested without a window (`app/tests.rs`).
  - `ui.rs`: the ImGui shell; it only turns input into `Action`s and draws
    `App`.
  - `viewport.rs`: the viewport texture (`register_external_texture`),
    camera input, picking and the gizmo.
  - `theme.rs`: colours, spacing, fonts (Inter 15 pt with a Lucide icon
    subset merged in; Dear ImGui 1.92 rasterises at the framebuffer scale,
    so text is crisp on HiDPI).
  - `headless.rs`: renders real frames of the UI offscreen and writes PNGs.

### Headless screenshots

```
cargo run --release -p studio -- --disc <iso> --pick <map> --screenshot out.png [--size 1600x1000] [--scale 2]
```

renders six frames of the real UI offscreen (no window) and writes the
last. `headless::tests::renders_the_studio_offscreen` does the same over
the scenario crate's invented test map, and skips when the machine has no
wgpu adapter. Screenshots of real maps are for local use; the repository
takes no game data.

## What the viewport shows, and what it does not yet

The world is the structure BSP's triangles (clusters and instanced
geometry, `scenario_edit::geometry::world_triangles`), shaded by slope,
with the placements as coloured diamonds pointing along their facing.
Sections whose raw data lives in a shared cache that is not beside the map
are skipped and counted in the status bar.

Not yet: textures and lightmaps on the BSP (the vertices are read without
UVs, and the shader and bitmap decode from `crates/assets` are not wired
in), and object models for placements (`crates/viewer` decodes render
models; the scenario palettes would need resolving through each object's
`hlmt` to its `mode`).
