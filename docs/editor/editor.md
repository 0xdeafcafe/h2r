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
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <maps dir>] [--halo1 <iso|maps dir>] [--hopper <file>|default] [--data <dir>]
```

The package is `studio`; its binary is `h2-editor`. With no source it
opens `H2_DISC`, else `disc/halo2.iso` when it exists
(`editor_core::default_disc`); with none, it starts empty and **File >
Open disc or map...** opens one later. A disc image lists every
`/maps/*.map`; a loose `.map` lists itself, and its directory's
`mainmenu.map`, `shared.map` and `single_player_shared.map` serve as the
shared caches. The source is read-only.

A Halo 1 source opens beside it: `--halo1`, else `$H1_ISO`, `$H1_MAP_DIR`
or the repository's `disc/halo1.iso` (`editor_core::halo1::default_source`).
**Open...** and a dropped path try Halo 1 first (`H1Source::open`: a disc
whose XBE is Halo: Combat Evolved's, a Halo 1 `.map`, or a folder holding
any), then Halo 2.

`--data` is the runtime's data root, as `h2 play --data`; the default is
the runtime's, `$TMPDIR/h2boot-data` (`content::store::default_data_root`).
Saved game variants (`U:`) and map packages (`maps/`) go there, so a run
with the same `--data` finds them. The dock layout persists in
`<default data root>/h2-editor.ini`, whatever `--data` says (View > Reset
layout restores the default).

## The window

![The editor window over Ascension with seven regions boxed and numbered in orange: 1 the menu bar, 2 Tools, 3 Outliner, 4 Viewport, 5 Inspector, 6 Log, 7 the status bar](../images/editor/layout-regions.jpg)

*The panels: (1) main menu bar, (2) Tools, (3) outliner, (4) the centre
window (the viewport here), (5) inspector, (6) log, (7) status bar. The
boxes and numbers are drawn by the headless renderer's `--callout`.*

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

### Packages and Halo 1 in the Maps list

Below the source's maps, **Maps** lists what is under `<data>/maps`
(`editor_core::packages::list`, re-read when the data root changes and
after a save), then Halo 1:

- **Packages**: edits of disc maps and new maps, by `[package] name`.
  An edit opens its disc map, then the package over it
  (`Session::open_package`, through `content::Library`); it needs a source
  with that map. A new map (`[base] derived = true`) opens from its
  resolved bytes (`Session::open_derived`), with the open disc's
  `shared.map` region as the place the shared caches come from. Saving
  either replaces that package.
- **Halo 1**: maps converted by the Halo 1 import
  ([h1-multiplayer.md](../game/h1-multiplayer.md)), one entry per sandbox
  variant, `Blood Gulch (H1 sandbox)` and `(H2 sandbox)`. Then, with a
  Halo 1 source open, its maps, read-only.

**What marks a converted map** is decided in one function,
`editor_core::packages::halo1_origins`: an `origin` table with
`game = "halo1"` (and `map`, `sandbox = "h1" | "h2"`, `source_sha256`),
either at the top of the manifest or in each entry of a top-level array
of tables (one per variant, each with its own `file`). That covers what
`content::Manifest` writes for converted maps: `[[variant]]` entries
with `file`, `sha256` and an `[variant.origin]` sub-table
(`packages::tests::the_converters_serialized_manifest_lists_both_variants`
feeds it `Manifest::to_toml`'s output). The manifest is read there as
plain TOML so that packages from builds with other fields still list; a
variant's own `file` is read directly, anything else through the
resolver, and a derived package the resolver refuses is read only when
its single layer is the whole map.

A converted variant opens as a new map and saves as a **new** package
(`<map>-edit`): the import's package holds both variants and their
origins, and the editor does not rewrite it. A derived package saved by
the editor carries the unedited map as its first layer and the edit as
the second (`package::save_package` with `derived`), so the resolver
rebuilds it without the import.

**Halo 1 maps, read-only.** `editor_core::halo1` reads a map through
`h1map`: every structure BSP's rendered triangles (each lightmap
material's render surfaces over its own vertices) and the multiplayer
placements (starting locations, netgame flags, netgame equipment,
vehicles), each with its fields as text. The studio shows them as a
`scenario_edit::Reference`: the session's world and markers with no
scenario, so the viewport, picking and framing are the same and nothing
can be edited.

The BSP is drawn textured and lightmapped through the same scene and
renderer as Halo 2's (`editor_core::halo1_art`, then
`scenario_edit::scene::reference_scene`):

- **Base maps.** Each lightmap material's shader names its base map at
  a place that depends on its group, past the 0x28-byte `shdr` header:
  `senv` +0x88 (flags +0x28, bit 0 alpha-tested: cut out), `soso`
  +0xa4, `swat` and `smet` +0x4c, `sgla` +0xac (else +0x158), `spla`
  +0xd4, and the first map of a maps block at +0x54 for `schi` (0xdc
  each, map at +0x6c) and `sotr`, Xbox's generic transparent shader
  (0x64 each, map at +0x1c); `scex` is read as `schi` but no
  multiplayer map has one. Every tag of those groups on five maps
  resolves a `bitm` there. The base map is the bitmap's first image,
  decoded by `h1bitmaps` at the first mip no larger than 512. Every
  group but `senv` and `soso` is drawn see-through.
- **Lightmaps.** `h1bitmaps::lightmap_pages` gives each lightmap's page
  (R5G6B5, no palette); a material with lightmap vertices draws with
  that page at its page-relative coordinates, as a Halo 2 lightmap is
  (base map × lightmap × 2 in gamma space). The rest (sky, unlit
  surfaces) keep the sun.
- **Winding.** Halo 1's surfaces turn the other way from the
  renderer's front faces; each triangle is turned to face its vertex
  normals.
- `editor_core::halo1_art::tests::disc_multiplayer_maps_are_textured_and_lightmapped`
  (ignored; needs the Halo 1 disc) checks all 13 multiplayer maps: at
  least 90 % of each map's shaders textured (all but four shaders over
  the 13: three glass shaders and a monitor glow name no base map) and
  some parts lit.

Not drawn yet: the sky and scenery models, detail and bump maps, and
`senv`'s blended types (the base map's alpha blends two detail maps,
which shows as dark patches on Sidewinder's snow, for example).

**Convert to Halo 2** (`app/convert.rs`). The inspector of a Halo 1 map
offers the sandbox (Halo 2, or Halo 1, whose `Error::Halo1Sandbox` is
shown as it comes) and names the base: `h1convert::convert::known`'s
default for the map (`convert::MAPS`; Beaver Creek, or Lockout for the
snowy ones), as `h2 h1 convert` without `--base`, read from the open
Halo 2 source; without it the button is disabled with the reason. The
button starts a thread that reads the base's bytes and runs
`h1convert::convert::convert(&h1, sha256 of the stored file, &mut base,
sandbox)`, then writes the result as `h2 h1 convert` does:
`<data>/maps/h1_<map>/<sandbox>/h1_<map>.map` and the `Converted`
manifest, keeping the other sandbox's `[[variant]]` from a manifest
already there (no manifest text is made here). Each frame `poll_convert`
picks up the result: the Maps list is re-read and the new variant opened
(`OpenPackageEntry`, guarded as usual), or the error is logged and shown
under the button in plain words (`explain`: `TooManySurfaces(n)` is
"This level has n surfaces; Halo 2 holds at most 65,536", any other error
its own text). The converter is a function pointer
(`App::set_converter`) so the tests run the whole flow with a stub;
`disc_maps_convert_open_and_find_their_source` (ignored) runs the real
one on the owner's discs, end to end.

**The Halo 1 overlay.** Opening a converted variant looks for its
`[origin] map` in the open Halo 1 source, then `$H1_MAP_DIR`, and checks
the stored file's SHA-256 against `source_sha256`. When it matches, View >
Halo 1 overlay (and the viewport toolbar's toggle) draws the Halo 1 BSP
and placement glyphs in orange at 35 % over the converted map, through
everything (`render::Viewport::set_overlay`), to check a conversion by
eye. The inspector says where the map came from, and why there is no
overlay when there is none.

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
  lightmaps) where no lightmap lights them.
- **Lightmaps.** Each cluster's and instance's lightmap page (its `ltmp`
  group's render info: an image of the lightmap bitmap and a palette) is
  decoded from `P8` through the group's palette, and its parts draw with
  their `1f` lightmap coordinates: base map × lightmap × 2 in gamma space,
  plus a faint fill light so no texel is pure black. Parts without a page,
  and object models, keep one sun and the sky, with dark base maps lifted.
- **Sky.** Behind a map with art, a gradient from the zenith to a pale
  horizon (not the scenario's sky model). Solid surfaces are
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
  a few, dropping the 4 % at each end of each axis (Lockout keeps a few
  far outside the level): distant scenery and sky geometry no longer
  shrink the level. `tests::disc_lockout_frames_its_level_not_its_outliers`
  (ignored) holds it.

Ascension used to draw only "its ground plane and rocks": its BSP has a
700-unit transparent cloud deck (part kind 4,
`transparent\sky_two_alpha_clouds`) at z = -14.5 under the structure,
drawn opaque and framed, which hid the lower half of the structure and
made the rest a speck. `tests::disc_ascension_frames_its_structure_not_its_cloud_deck`
(ignored; needs the disc) holds the fix.

Coagulation's bases and Midship's hull used to come out black. Their
lightmaps were never black (the pages, palettes and coordinates match
what the engine binds); the window was. The viewport renders into an
sRGB texture, and Dear ImGui's shader treats every texel it samples as
sRGB-encoded, raising it to the power 2.2 before its sRGB framebuffer
encodes it again. Given the texture's sRGB view it sampled linear
values and showed them as encoded ones, so a dim wall at linear 0.05
(sRGB 63) showed as 13. `Viewport::ui_view` hands the UI an `Rgba8Unorm`
view of the same pixels instead. `headless::tests::the_viewport_shows_what_it_rendered`
compares the window's pixels with the viewport's own, and
`headless::tests::disc_coagulation_bases_and_midship_hull_are_lit`
(ignored; needs the disc) checks that under 2 % of either map's
viewport is black (a quarter of Coagulation's was).

Not yet: the scenario's
sky model; the bucket streams (types `2f` and `30`: per-vertex colour and
light direction, not interpreted); detail and bump maps, animated or
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
  (coalescing by `u64` keys), `Status`, `variant_sources` (the
  variant schema, XBE save names and save storage for a source),
  `packages` (the packages under a data root, Halo 1 origins) and
  `halo1` (Halo 1 sources and what is shown of a map).
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
    tool's edits; `resolve` answers and re-dispatches it). `app/library.rs`
    holds the packages, the Halo 1 source and the overlay. Tested without
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
| `--halo1 <iso\|dir>` | open a Halo 1 source (else `$H1_ISO`, `$H1_MAP_DIR`, `disc/halo1.iso`) |
| `--halo1-map <map>` | show a map of it, read-only |
| `--package <dir>[#H1\|#H2]` | open a package from the Maps list (a converted map's variant by its sandbox) |
| `--select <block>:<index>` | select a placement (`vehicles:0`, `netgameequipment:3`) |
| `--focus` | focus the camera on the selection |
| `--camera <yaw>,<pitch>,<distance>` | orbit the camera (degrees, world units), after `--focus` |
| `--tool variants\|playlists` | show that tool |
| `--variant <n>` | open built-in variant `n` |
| `--copy-variant <name>` | copy it to an editable variant named `<name>` |
| `--hopper <file>\|default --playlist <1..16>` | open a playlist file (or the default lineup) at a playlist |
| `--preset <n>` | pick historical preset `n` for that playlist |
| `--dialog package\|unsaved` | open the Save as package window, or nudge the selection and ask to quit (the unsaved-changes window) |
| `--nudge <x>,<y>,<z>` | move the selection by that much, as an edit |
| `--page <title>` | in the variant tool, open only the settings pages whose title contains it |
| `--no-disc` | open no disc and no Halo 1 source (the first run) |

Then, to show a state the flags can't set up, scripted input runs over
the settled frames, and the written frame can be cropped and annotated:

| Flag | Effect |
| --- | --- |
| `--do '<step>; <step>...'` | `hover`, `click`, `rclick`, `dclick` or `move` a target; `down`, `up` (the left button, for drags); `type <text>`; `key <name>[*n]`; `wait <frames>`; `sleep <ms>` (wall time, for background work such as a conversion); `wheel <dy>` |
| `--crop <target>` (repeatable) | write only the union of these targets' rectangles |
| `--pad <points>` | grow the crop by this much (default 8) |
| `--crop-height <points>` | make the crop this tall from its top |
| `--callout <target>=<badge>` (repeatable) | draw an orange box round the target with the badge (a number) on it |

A target (`studio::script`) is a point `x,y`, a window `win:<name>`
(`win:###inspector`, `win:##Menu`, `win:##Tooltip`, `win:##Combo`,
`win:##Popup`), or a widget by label, `[<window>>]<label>[#n]` (`View`,
`Rotate (E)`, `##setting-2`, `###viewport>Edit#0`), each optionally
`@dx,dy` from its top left rather than its centre. Widgets are found by
label through Dear ImGui's test-engine hooks (`studio::probe`; the
`test-engine` feature of `dear-imgui-rs`): `ItemAdd` gives each item's
rectangle and window, `ItemInfo` its label. Combos never report a label,
so the labels a script names are also hashed on each item's window ID
stack and matched by ID. The hooks only record in headless renders. Set
`H2_EDITOR_PROBE=1` to print every window and widget the last frame drew,
to write a script against.

```
h2-editor --disc disc/halo2.iso --pick ascension --select netgameequipment:0 \
  --do 'click View; hover Rotate gizmo' --crop View --crop 'win:##Menu' --screenshot menu-view.png
```

![The View menu open with Rotate gizmo highlighted, cropped to the menu](../images/editor/menu-view.png)

*What that command writes: the View menu opened and an item hovered by
the script, cropped to the menu.*

`scripts/editor-screenshots.sh` renders the user guide's pictures this way
from your disc, into `docs/images/editor/` (editor-guide.md §10).
`headless::tests::renders_the_studio_offscreen` does the same over the
scenario crate's invented test map, and skips when the machine has no
wgpu adapter.
