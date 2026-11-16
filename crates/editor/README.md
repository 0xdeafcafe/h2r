# editor (`h2-editor`)

A native editor suite for h2recomp: an eframe app on the wgpu renderer.
The shell has a sidebar of tools and the maps of the opened disc, a top
bar (Open, Save, Save As, Undo, Redo, Open disc or map), a status line, and
prompts before unsaved changes are lost. The hopper (playlist) tool ships
in this crate; the scenario and game variant editors live in their own
crates and plug in through adapters. See [docs/editor.md](../../docs/editor.md).

```
cargo run --release -p editor -- [--disc <iso> | --map <file.map>] [--hopper <multiplayer_hopper.dat>]
```

## Plugging a tool in

A tool crate exposes a plain struct with a `ui` method and does not depend
on this crate:

```rust
pub struct VariantEditor { /* ... */ }
impl VariantEditor {
    pub fn ui(&mut self, ui: &mut egui::Ui /*, whatever it needs */) { /* ... */ }
}
```

The integrator adds a small adapter here that implements [`Tool`]
(`src/tool.rs`) and registers it in `src/main.rs` with
`shell.add_tool(Box::new(Adapter(...)))`:

```rust
struct Variants(variants::VariantEditor);

impl editor::Tool for Variants {
    fn title(&self) -> &str { "Game variants" }
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut editor::ToolContext<'_>) {
        self.0.ui(ui);
    }
    // Optional, all with "not supported" defaults: is_dirty, path,
    // extensions, open, save, save_as, can_undo, can_redo, undo, redo.
}
```

`ToolContext` carries the opened `MapSource` (disc image or loose map:
`map_names()`, `open_map(name)`, `resources(name)` for raw data), the map
picked in the sidebar, eframe's `egui_wgpu::RenderState` for 3D viewports
(`None` in headless tests), and the status line (`cx.info(..)`,
`cx.error(..)`).

`UndoStack<T>` (`src/undo.rs`) is a snapshot undo stack any tool can use:
clone the state before drawing, draw, then `undo.track(before, &state,
ui.ctx().memory(|m| m.focused()))`, which records one step per change and
folds a run of edits from one focused widget into one.

## Layout

- `src/tool.rs`: the `Tool` trait, `ToolContext`, `ToolError`.
- `src/shell.rs`: the window, and the prompt state machine (tested in
  `src/shell/tests.rs` without a window).
- `src/source.rs`: the disc image / loose map source (the viewer's disc
  reader, copied).
- `src/undo.rs`: the undo stack.
- `src/hopper/`: the hopper tool. `model.rs` (working copy, edits,
  validation, writing back) and `presets.rs` are UI-free and tested with
  synthetic images; `catalog.rs` reads map names from the disc; `mod.rs`
  is the panel and file handling.

Tests: `cargo test -p editor`. They are headless (egui frames run with no
window or GPU) and use synthetic bytes only; the one disc test is
`#[ignore]` and reads `H2_DISC`.
