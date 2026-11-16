# h2-editor

`h2-editor` (`crates/editor`) is a native editor suite for the files
h2recomp's game reads: a Rust app on egui and wgpu (eframe), in the
spirit of the original tools' Sapien. It hosts tools:

- **Playlists (hoppers)**: edits `multiplayer_hopper.dat`, the 16
  matchmaking playlists the game downloads from Live (below).
- **Scenario**: the scenario editor (`crates/scenario-edit`,
  [scenario-editor.md](scenario-editor.md)), wired in by
  `crates/editor/src/scenario.rs`. Picking a map in the sidebar opens it,
  unless the open map has unsaved edits. Save always asks for a new path:
  the source map is never overwritten.
- **Game variants** (`crates/variants`): a separate crate, wired in the
  same way once it lands.

Nothing in the editor contains game data. Map names and everything else
it shows come from your own disc at run time.

## Running it

```
cargo run --release -p editor -- [--disc <iso> | --map <file.map>] [--hopper <multiplayer_hopper.dat>]
```

The map source is, in order: `--disc` or `--map`, then `H2_DISC`, then
`disc/halo2.iso` in the repository. With none, the editor starts without
one; **Open disc or map...** in the top bar or sidebar opens one later.
A disc image lists every `/maps/*.map` in the sidebar; a loose `.map`
lists itself, and its directory's `mainmenu.map`, `shared.map` and
`single_player_shared.map` serve as the shared caches. The source is
read-only.

## The window

- **Sidebar**: the tools (a `*` marks unsaved changes), then the maps of
  the open source; clicking one selects it for the tools that use it.
- **Top bar**: Open, Save, Save As (for the active tool's file), Undo,
  Redo, Open disc or map, and the active file's path (`*` when unsaved).
- **Status line**: the last result or error.
- **Prompts**: paths are typed into a small dialog. Opening a file over
  unsaved changes, or closing the window with any, asks Save / Discard /
  Cancel. Save with no file yet asks where, then carries on.
- **Shortcuts**: Cmd/Ctrl+O open, +S save, +Shift+S save as, +Z undo,
  +Shift+Z or +Y redo.

## Tool architecture

A tool implements `editor::Tool` (`crates/editor/src/tool.rs`):

```rust
pub trait Tool {
    fn title(&self) -> &str;
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>);
    fn is_dirty(&self) -> bool { false }
    fn path(&self) -> Option<&Path> { None }
    fn extensions(&self) -> &[&str] { &[] }
    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError>;   // default: Unsupported
    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError>;                // default: Unsupported
    fn save_as(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError>; // default: Unsupported
    fn can_undo(&self) -> bool { false }
    fn can_redo(&self) -> bool { false }
    fn undo(&mut self) -> bool { false }
    fn redo(&mut self) -> bool { false }
}

pub struct ToolContext<'a> {
    pub source: Option<&'a MapSource>,                 // the disc or loose map
    pub selected_map: Option<&'a str>,                 // picked in the sidebar
    pub render_state: Option<&'a egui_wgpu::RenderState>, // for 3D viewports; None headless
    pub status: &'a mut Option<Status>,                // or cx.info(..) / cx.error(..)
}
```

`save` returns `ToolError::NoPath` when the tool has no file yet; the
shell then asks for a path and calls `save_as`. `ToolError::Invalid`
carries a validation message for the status line. The shell owns the
prompts; a tool never has to ask about unsaved changes itself, only report
`is_dirty` honestly.

`MapSource` (`crates/editor/src/source.rs`) gives `map_names()`,
`open_map(name)` (a `map::Map` over the disc region or file) and
`resources(name)` (a `tags::geometry::RawSource` over the map and the
shared caches), so a tool can decode tags and geometry the way
`crates/viewer` does. For a 3D viewport, a tool registers an
`egui_wgpu::CallbackTrait` paint callback, or renders to a texture and
registers it with `render_state.renderer`, using `render_state.device` and
`queue`.

`editor::UndoStack<T>` is a snapshot undo stack: clone the state before
drawing, draw, then `track(before, &state, focused_widget)`. It records
one step per change and folds a run of edits from one focused widget
(typing, dragging) into one. Keep the tracked state small or share large
immutable parts through `Arc`.

### How a tool crate plugs in

Tool crates do not depend on `editor`. Each exposes a plain struct with
`pub fn ui(&mut self, ui: &mut egui::Ui, ...)` (plus whatever it needs:
open/save, undo, a `RenderState` for drawing), and the integrator writes
a few-line adapter in `crates/editor` that implements `Tool` by calling
it, then adds it in `main.rs` with `shell.add_tool(Box::new(adapter))`.
The trait has two required methods and stable defaults, so adapters stay
small. Both crates use the workspace's `egui`, `egui-wgpu` and `eframe`
dependencies (root `Cargo.toml`), so the versions always match.

## The hopper tool

The native form of the web admin's playlist builder (docs/live-admin.md
§3, `web/src/admin/HopperBuilder.tsx`), working on a file instead of the
server's copy:

- **Open** reads either form of `multiplayer_hopper.dat`: the raw
  0x162cc0-byte image the console caches, or the download envelope
  (header, size, zlib) the server publishes. **Save** writes the same form
  back. An envelope larger than the game's 128 KiB download buffer is
  refused (the game would keep its built-in playlists). With nothing open,
  **New blank file** starts from an all-zero image.
- Pick one of the 16 playlists; edit its title (15 bytes) and description
  (127 bytes), and per entry its map (a picker of the disc's multiplayer
  maps, by name; numbers without a disc) and weight (1..1000, with each
  entry's resulting chance). Each entry shows its game variant (name,
  game type, teams). Reorder, copy, add (copies the last) and remove
  entries; a playlist keeps 1..100.
- **Advanced: raw fields**: the raw name, type, status, the three range
  fields, and every record word as numbers.
- **Start from preset**: the web builder's historical lineups, resolved
  against the disc's maps and the entries already in the file (each new
  entry copies a whole published entry of the same game type, preferring
  the named variant, then the same team play). What does not resolve is
  listed as unavailable.
- **Undo/redo** covers every edit, including past a save.

The editing logic (`crates/editor/src/hopper/model.rs`) mirrors the
server's `apply_patch`: every entry remembers the slot its bytes come
from, and writing back copies that whole 0x37c-byte slot (game variant
and unknown blobs) before setting the edited fields, so reorders and
copies lose nothing. It is stricter about untouched bytes than the
server: an unchanged playlist is not rewritten at all, and in a changed
one the title and description (written to all nine language slots) are
rewritten only if they changed. An unedited file saves to the identical
image. Only changed playlists are validated, with the server's rules.

Map names come from the disc's main menu map (the globals' multiplayer
level list, as `h2 live assets extract` reads it for `catalog.json`).
The presets' playlist texts are the preset's own; the disc's
`matchmaking_hopper_descriptions` strings are not read yet.

To put a saved file on a Live server, run `h2 live hopper import <file>`
on the server's machine (docs/live-admin.md); the editor does not publish
by itself.
