# h2-editor

To *use* the editor (open a map, move a weapon, make a game variant, edit
a playlist), read the user guide, [editor-guide.md](editor-guide.md).
This page is for developers.

`h2-editor` (`crates/studio`) is a native editor suite for the files
h2recomp's game reads, in the spirit of the original tools' Sapien, with
a game-tool look (Unreal, RenderDoc, Tracy). It is a Rust app on Dear
ImGui's docking branch: `dear-imgui-rs` for the UI, `dear-imgui-wgpu` on
the workspace's wgpu 30, `dear-imgui-winit` for the window,
`dear-imguizmo` for the 3D gizmo, `rfd` for the system's file dialogs. No
web views, no scripting. It hosts three tools:

- **Scenario**: the scenario editor (`crates/scenario-edit`,
  [scenario-editor.md](scenario-editor.md)): placements on a map, in a 3D
  viewport, saved as a map package or a whole `.map`.
- **Game variants**: the game variant editor (`crates/variants`,
  [variants.md](variants.md)).
- **Playlists**: `multiplayer_hopper.dat`, the 16 matchmaking playlists
  the game downloads from Live (`crates/hopper-edit`, below).

Nothing in the editor contains game data. Map names and everything else
it shows come from your own disc at run time.

## Running it

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <maps dir>] [--hopper <file>|default] [--data <dir>]
```

The package is `studio`; its binary is `h2-editor`. With no source it
opens `H2_DISC`, else `disc/halo2.iso` when it exists
(`editor_core::default_disc`); with none, it starts empty and **File >
Open disc or map...** opens one later. A disc image lists every
`/maps/*.map`; a loose `.map` lists itself, and its directory's
`mainmenu.map`, `shared.map` and `single_player_shared.map` serve as the
shared caches. The source is read-only.

`--data` is the runtime's data root, as `h2 play --data`; the default is
the runtime's, `$TMPDIR/h2boot-data` (`content::store::default_data_root`).
Saved game variants (`U:`) and map packages (`maps/`) go there, so a run
with the same `--data` finds them. The dock layout persists in
`<default data root>/h2-editor.ini`, whatever `--data` says (View > Reset
layout restores the default).

## The window

- **Main menu**: File (open a disc or map, open a folder of maps, save the
  map as a new `.map`, save as or open a package, the variant and
  playlist tools' file commands, quit), Edit (undo, redo, duplicate,
  delete, allow growing full blocks), View (frame the map, focus the
  selection, gizmo mode and axes, reset layout).
- **Tools** (left, top): the tools (Scenario, Game variants, Playlists; a
  `*` marks one with unsaved edits), then **Maps**: the opened source and
  its maps. The outliner, centre and inspector windows show the active
  tool: below for the scenario tool, then [Game variants](#game-variants)
  and [Playlists](#playlists). Each tool's windows keep their dock slot
  (`WindowKey::label` retitles a window without changing its ID).
- **Outliner** (left, bottom): the placement blocks as a tree, a filter,
  Duplicate and Delete. Right-click an entry for Duplicate, Delete and
  Add; right-click a block for Add. A button is disabled, with the reason
  as its tooltip, when the editor refuses (scenario-editor.md §6).
  Double-click an entry to focus the camera on it.
- **Viewport** (centre): the map's art and a marker per placement, drawn
  by `scenario_edit::render::Viewport` into an offscreen texture that
  ImGui shows as an image. Left click picks; right-drag orbits, with WASD
  and QE flying while it is held; middle-drag pans; the wheel zooms. The
  selection has an ImGuizmo gizmo: W moves, E rotates (facing-only
  placements turn about z); one drag is one undo step.
- **Inspector** (right): the selection's typed fields. The palette index
  is a combo of the palette's tag names, tag references a combo of the
  map's item tags; positions, rotations (in degrees) and scale are drags;
  flags and other integers are bounded drags. Read-only fields are
  disabled; each field's tooltip gives its offset and evidence.
- **Log** (bottom): every action's result, errors in red.
- **Status bar**: the last message, unsaved state, a summary of what is
  open, frame rate.
- **Unsaved changes**: anything that would drop a tool's unsaved edits
  asks first, naming the tool: another source or map (scenario), another,
  new or imported variant, or an entry's variant opened from a playlist
  (variants), another or new playlist file (playlists). Save (Save as...
  for a map, or a playlist file with no path yet), Discard or Cancel.
  Quitting asks about each tool with unsaved edits in turn.

Shortcuts: Cmd/Ctrl+O open, Cmd/Ctrl+S save (the map as a new file, the
variant, the playlist file), Cmd/Ctrl+Z undo and Shift+Cmd/Ctrl+Z or
Cmd/Ctrl+Y redo (in the active tool), and in the scenario tool
Cmd/Ctrl+D duplicate, Delete delete, F focus, Home frame all, W/E gizmo
mode; Cmd/Ctrl+Q quit. Dropping a disc image, map or folder on the window
opens it.

## Scenario

Picking a map in **Maps** opens it, unless the open map has unsaved edits
(the guard asks). **Save map as...** always asks for a new path (a whole
`.map`, the export): the source map is never overwritten. **Save as
package...** and **Open package...** save and open map packages in
`<data>/maps` (scenario-editor.md §5, [maps.md](../game/maps.md) §8).

### What the viewport shows, and what it does not yet

`scenario_edit::scene` builds the map's art when it opens, from your own
disc at run time (about 30 to 150 ms for a multiplayer map in a release
build), and keeps the last four maps' scenes so reopening one is instant.

- **The structure BSP, textured.** Clusters and instanced geometry
  (`geometry::world_parts`) carry their vertex UVs and normals; each
  part's shader (`sbsp +0xa4`) gives its base map, the first bitmap of the
  compiled postprocess whose name reads as diffuse (the Live site's rule,
  docs/live/live-art.md). Bitmaps are decoded once by
  `assets::decode_image`, shrunk to 512 a side, given a mip chain and
  lifted when very dark (the game draws them under detail maps and
  lightmaps). One sun and the sky light the scene. Solid surfaces are
  one-sided, as in the game, so the camera sees into a level through the
  backs of the cliffs and walls around it.
- **Blends.** Templates in a `transparent` or `water` folder, plain
  `overlay` decals and part kind 4 are drawn see-through after everything
  else and are neither framed nor picked; `alpha_test` templates are cut
  out.
- **Object models.** Every palette entry (and netgame equipment's item
  collection, by its first entry) resolves object → `hlmt` → `mode`
  through `tags::object`; each region's first permutation at its best LOD
  is drawn at the placement's position, rotation and scale, instanced per
  model. Clicking a model picks it (ray against its triangles); the
  selection is tinted and keeps its marker.
- **Glyphs.** Placements without a model get a glyph in their block's
  colour: a standing pawn with an arrow for player spawns, a pole and
  pennant for netgame flags, a diamond for the rest. Past 40 units from
  the eye glyphs grow with distance, so they stay readable when the whole
  map is framed, and their pick spheres grow with them.
- **Framing.** Frame (Home) frames the placements, padded, when there are
  a few: distant scenery and sky geometry no longer shrink the level.

Ascension used to draw only "its ground plane and rocks": its BSP has a
700-unit transparent cloud deck (part kind 4,
`transparent\sky_two_alpha_clouds`) at z = -14.5 under the structure,
drawn opaque and framed, which hid the lower half of the structure and
made the rest a speck. `tests::disc_ascension_frames_its_structure_not_its_cloud_deck`
(ignored; needs the disc) holds the fix.

Not yet: lightmaps (the `ltmp` groups and their bitmaps are read by
`tags::structure` but not applied), detail and bump maps, animated or
self-illuminated shaders, model variants and permutations other than the
first, and node transforms of skinned models (models are drawn in their
bind pose).

## Game variants

The variant editor of [variants.md](variants.md), in the same windows:

- **Variants** (the outliner's place): New (pick a game type; a new
  variant has the type's defaults), the saved variants, the built-ins.
- **Settings** (centre): the open variant's name and its settings in the
  game's pages, each a combo of the values the game offers, a checkbox or
  a bounded number; a setting changed from the type's default is bold,
  and its help and default are on hover. Built-ins are read-only, as in
  the game: Copy makes a named, editable copy.
- **Inspector**: Save (to the game's save storage under the data root),
  Revert, Delete, Export... (a portable `.h2variant` file), Import... (a
  file, or pasted text), whether the game would accept it, and the
  changes from the type's defaults.

The schema comes from the disc's `mainmenu` map (the built-in model
without a disc); save names come from the XBE (`H2_XBE`, else
`default.xbe` beside the disc image) (`editor_core::variant_sources`).
Saves go to the runtime's save storage (`U:` under `<data>`), so there is
no Save As; use Export to share. Every edit undoes; typing a name is one
step.

## Playlists

The native form of the web admin's playlist builder (docs/live/live-admin.md
§3, `web/src/admin/HopperBuilder.tsx`), working on a file instead of the
server's copy:

- **Playlists** (the outliner's place): New (an all-zero image, every
  playlist empty), Default lineup (the playlists a Live server serves
  before any file is published), Open..., Save, Save as..., then the 16
  playlists (a mark on those that would not save). Open reads either form
  of `multiplayer_hopper.dat`: the raw 0x162cc0-byte image the console
  caches, or the download envelope (header, size, zlib) the server
  publishes. Save writes the same form back. An envelope larger than the
  game's 128 KiB download buffer is refused (the game would keep its
  built-in playlists).
- **Entries** (centre): the playlist's title (15 bytes) and description
  (127 bytes) and its entries: the map (a picker of the disc's
  multiplayer maps, by name; numbers without a disc), the game variant,
  Edit, the weight (1..1000) and the resulting chance, and move,
  duplicate, remove; Add an entry (copies the last). A playlist keeps
  1..100 entries. Validation errors are listed below.
- **Inspector**: start from a historical preset, the variant picker's
  sources (load a `.h2variant` file, re-read the saves), and **Advanced:
  raw fields** (the raw name, type, status, the three range fields, each
  entry's raw string, flags and second dword).

**Game variants.** Each entry shows its game variant (name, game type,
teams), decoded from the 0x130 bytes its slot will carry, with
"(picked)" once changed. Its combo offers the disc's built-in variants
(built the way the game builds them: the type's defaults, the name, the
description number, the variant's own settings, and the built-in flag
set, as the variant editor lists them), your saved variants (the
runtime's save storage, re-read with **Refresh saved**) and
`.h2variant` files loaded this session (typed path and **Load**, or
**Browse...**). Picking one copies its `GameVariant::to_bytes()` over
the slot's `+0x24c..+0x37c`; the map id, weight and every byte before
the variant stay as they were. A variant the game's clean-up would
change (`variants::record::check`) is disabled with the reasons on hover,
and is refused if picked another way. **Edit** opens a copy of the
entry's variant in the variant tool, as a new unsaved variant there (the
guard asks first when that tool has unsaved edits); to use the edited
variant, save it there and pick it from **Saved**.

**Start from a preset**: the web builder's historical lineups, resolved
against the disc's maps and the entries already in the file (each new
entry copies a whole published entry of the same game type, preferring
the named variant, then the same team play). What does not resolve is
listed as unavailable.

Each change is one undo step (typing in a field or dragging a weight is
one), and undo walks back past a save.

The editing logic (`crates/hopper-edit/src/model.rs`, driven through
`hopper_edit::HopperSession`) mirrors the server's `apply_patch`: every
entry remembers the slot its bytes come from, and writing back copies
that whole 0x37c-byte slot (game variant and unknown blobs) before
setting the edited fields, so reorders and copies lose nothing. It is
stricter about untouched bytes than the server: an unchanged playlist is
not rewritten at all, and in a changed one the title and description
(written to all nine language slots) are rewritten only if they changed.
An unedited file saves to the identical image. Only changed playlists are
validated, with the server's rules.

A picked game variant is kept as the whole 0x130-byte record beside the
entry's source slot; `model::slot_bytes` builds the slot (source bytes,
then the picked variant over the tail, then the editable fields) both
for the save and for what the table shows. The raw string, flags and
second dword of **Advanced: raw fields** lie inside the variant (at its
`+0x00`, `+0x48` and `+0x58`), so picking re-reads them from the new
record, and editing them afterwards edits the picked variant: the save
then checks the result with the clean-up rules again. Variants that came
with the file are written back as they are, never judged.

Map names come from the disc's main menu map (the globals' multiplayer
level list, as `h2 live assets extract` reads it for `catalog.json`).
The presets' playlist texts are the preset's own; the disc's
`matchmaking_hopper_descriptions` strings are not read yet.

To put a saved file on a Live server, run `h2 live hopper import <file>`
on the server's machine (docs/live/live-admin.md); the editor does not
publish by itself.

## Code

The tools' logic lives in UI-free crates with their own tests; the
studio crate only draws them and turns input into actions.

- `crates/editor-core`: map sources (`MapSource`: a disc image, a loose
  `.map` with its siblings, or a folder of maps; `map_names()`,
  `open_map(name)`, `resources(name)`), the snapshot `UndoStack`
  (coalescing by `u64` keys), `Status`, and `variant_sources` (the
  variant schema, XBE save names and save storage for a source).
- `crates/scenario-edit`: `Session` holds the scenario editor's state and
  every command (open, select, add, remove, plane drags, gizmo gestures,
  undo, save, packages). `render` and `geometry` draw it offscreen with
  wgpu (the camera gives separate view and projection matrices for
  ImGuizmo; `geometry::decompose` turns a gizmo matrix back into position
  and yaw/pitch/roll).
- `crates/variants`: the variant record, schema, saves and
  `VariantEditor` (state and commands).
- `crates/hopper-edit`: the playlist editor's logic (working copy,
  validation, presets, catalog, the variant picker's sources) and
  `HopperSession` (file handling, undo).
- `crates/studio`:
  - `app.rs`: `App`, the state (each tool's session) and `Action`s
    (open, select map, save, packages, undo/redo, select, delete,
    duplicate, add; variants; playlist files and entries; quit), with the
    unsaved-changes guard (`dispatch` holds an action that would drop a
    tool's edits; `resolve` answers and re-dispatches it). Tested without
    a window (`app/tests.rs`).
  - `ui.rs`: the ImGui shell and the scenario tool's windows; it only
    turns input into `Action`s and draws `App`. `ui/variants.rs` and
    `ui/playlists.rs` draw the other two tools.
  - `viewport.rs`: the viewport texture (`register_external_texture`),
    camera input, picking and the gizmo.
  - `theme.rs`: colours, spacing, fonts (Inter 15 pt with a Lucide icon
    subset merged in; Dear ImGui 1.92 rasterises at the framebuffer scale,
    so text is crisp on HiDPI).
  - `headless.rs`: renders real frames of the UI offscreen and writes PNGs.

## Headless screenshots

```
cargo run --release -p studio -- --disc <iso> --pick <map> --screenshot out.png [--size 1600x1000] [--scale 2]
```

renders six frames of the real UI offscreen (no window) and writes the
last. The flags that set the scene up:

| Flag | Effect |
| --- | --- |
| `--pick <map>` | open this map in the scenario tool |
| `--select <block>:<index>` | select a placement (`vehicles:0`, `netgameequipment:3`) |
| `--focus` | focus the camera on the selection |
| `--camera <yaw>,<pitch>,<distance>` | orbit the camera (degrees, world units), after `--focus` |
| `--tool variants\|playlists` | show that tool |
| `--variant <n>` | open built-in variant `n` |
| `--copy-variant <name>` | copy it to an editable variant named `<name>` |
| `--hopper <file>\|default --playlist <1..16>` | open a playlist file (or the default lineup) at a playlist |
| `--preset <n>` | pick historical preset `n` for that playlist |
| `--dialog package\|unsaved` | open the Save as package window, or nudge the selection and ask to quit (the unsaved-changes window) |

`scripts/editor-screenshots.sh` renders the user guide's pictures this way
from your disc, into `docs/images/editor/` (editor-guide.md §10).
`headless::tests::renders_the_studio_offscreen` does the same over the
scenario crate's invented test map, and skips when the machine has no
wgpu adapter.
