# h2-editor polish: what is wrong, the design, and the slices

Status: Design. Nothing here is built; each slice below ships part of it.
Code: `crates/apps/studio/src` (the window: `ui.rs`, `ui/`, `play.rs`, `viewport.rs`, `theme.rs`), `crates/tools/scenario-edit/src` (the scene and its renderer). Tests: named per slice.
Switch: none.

This page lists what makes `h2-editor` hard to use, with the place in the
code for each, and the design that fixes it: one layout, a game link that
is always on screen, the scenario's tags as a tree, tag edits that reach
the running game, overlays for the volumes and collision that the game
does not draw, edits to those, and a viewport drawn by the native
renderer. It ends with slices sized for one implementer each. It builds
on two designs and does not restate them: the live stream and apply
classes in [live-mirror.md](live-mirror.md), and the merged Map tool in
[click-to-edit.md](click-to-edit.md). The rule that must hold: the editor
stays one native window (Dear ImGui through `dear-imgui-rs`, wgpu), and
every action it gains is also a method of its
[agent API](agent-api.md).

A finding below is deleted from this page by the slice that fixes it.

## What is wrong

Read from the code and from the user guide's screenshots
(`docs/images/editor/window.jpg`, `first-run.jpg`). No editor was running
during the read, so nothing here comes from the socket API.

### The shell

| # | Problem | Where |
|---|---|---|
| R1 | Tools and maps share one window. The **Tools** list (six rows) and the whole **Maps** browser (disc, map pack, packages, Halo 1, shared) sit in one panel that the default layout gives 38 % of the left column, so the map list scrolls in a slot about seven rows tall. | `ui.rs:672` (`tools_window`), headings at `ui.rs:682` and `ui.rs:698`, split at `ui.rs:67` |
| R2 | Picking a tool swaps what three windows mean. The window with the ID `viewport` is titled Viewport, Settings, Entries, Preview, HUD or Fields; `outliner` and `inspector` change the same way. A user who docks "the inspector" somewhere finds a hex dump there in the Tags tool. | `ui.rs:576` to `ui.rs:664` (`variant_windows` through `tag_windows`) |
| R3 | Scenario and Tags are two tools over one map. They have separate selections, a second map picker, and separate undo: the inspector's tag section carries its own **Undo tag edit** and **Redo tag edit** buttons because Cmd+Z goes to the active tool's stack. | `ui/tags.rs:62` (`map_picker`), `ui/inspector.rs:245` to `ui/inspector.rs:251`, `app.rs:585` (`can_undo`) |
| R4 | The viewport toolbar is a row of small text buttons with the keys in their labels (`Move (W)`), followed by a line of dim help text that is cut off in a narrow window. Snap steps are behind a right-click that only a tooltip mentions. | `ui.rs:1213` (`viewport_toolbar`), `ui.rs:1278`, `ui.rs:1297`, `ui.rs:1314` |
| R5 | Nothing chooses what the viewport shows. The View menu holds gizmo modes and snapping; the only show or hide switches are **Halo 1 overlay** and, in the Game panel, **Show live objects**. Markers, the sky, models and fog cannot be turned off. | `ui.rs:390` to `ui.rs:459`, `ui/play.rs:269` |
| R6 | Save is always "Save as". Cmd+S opens a file dialog every time for a map, and a package opened from the Maps list is saved through a different menu item with a form. | `ui.rs:189`, `ui.rs:316` to `ui.rs:335` |
| R7 | Empty states do not say what to do next. With no source: "Pick a map to see it here." and "No map open." With no selection: two lines about click modifiers. No button opens a disc from the centre of the window. | `ui.rs:909`, `ui.rs:780`, `ui/inspector.rs:100` |
| R8 | The status bar is three unseparated runs of text (the last message, the game chip, the counts) in one colour family; the message repeats the last log line. Its background is a literal colour outside the theme. | `ui.rs:956` (`status_bar`), `ui.rs:968` |
| R9 | No search across the app. Maps, tags and placements each have their own filter box, and commands are only in menus. | `ui.rs:784`, `ui/tags.rs:132` |
| R10 | The look is flat: one font size for everything, 24 `small_button`s for primary actions, section headings that are bold text, marker colours set as literals in the scene crate. | `theme.rs:71` (`apply`), `crates/tools/scenario-edit/src/geometry.rs:698` |

### The game link

| # | Problem | Where |
|---|---|---|
| R11 | The Game panel is the second tab behind the Log, so it is hidden at start. | `ui.rs:78` |
| R12 | The panel, the F5 shortcut and the status chip exist only in the Scenario tool. The link itself runs in every tool (`self.play.update`), so edits can be going to a game while nothing on screen says so. | `ui.rs:143`, `ui.rs:154`, `ui.rs:155`, `ui.rs:984` to `ui.rs:988` |
| R13 | The chip is a line of coloured text in the status bar ("Game: zanzibar playing tick 1234 (the open map) · 60 Hz · 12 ms · 0 dropped, 0 skipped"). It is not a control, and colour alone carries its state. | `ui/play.rs:354` (`status_chip`), `ui/play.rs:54` (`chip`), `play.rs:224` |
| R14 | The panel is one column of about thirty controls in the order they were built: Play, Stop, the spawn, a path field for the `h2` binary, the log path, a notice, Auto-pair, a socket field, two different **Pair** buttons, **Find**, a list of games, a list of raw socket paths, Unpair, then the paired controls. | `ui/play.rs:102` to `ui/play.rs:313` |
| R15 | Whether edits are going to the game is a checkbox and a sentence inside that panel. The viewport and the inspector look the same paired and unpaired. | `ui/play.rs:248` to `ui/play.rs:259` |
| R16 | Only placement fields are sent. A tag edit made in the Tags tool or the inspector stays in the document; the game has `edit.set` for it and the editor never calls it for a tag other than the scenario. | `play.rs:731` (`push_edits`), `play/sync.rs` |
| R17 | The game's `edit.set` and `edit.revert` do not check the session. `sim.state.set` refuses in co-op and as a client (`crates/extensions/agent/src/sim/mod.rs:408`); the edit methods have no such check, so the editor's own check (`play.rs:726`) is the only one. | `crates/extensions/agent/src/hot.rs:121` |

### Tags and the scenario

| # | Problem | Where |
|---|---|---|
| R18 | The scenario tag cannot be read. The registry has no `scnr` definition (`crates/formats/tags/src/defs` has no `scnr.txt`), so the tag shows no fields, and it is read-only because the Scenario tool owns its bytes. | `app/tag_doc.rs:270` to `app/tag_doc.rs:273` |
| R19 | No view starts from the scenario. The tag tree arranges every tag of the map by group or by path; nothing shows what the scenario uses (its BSPs, lightmaps, skies, object palettes, globals). The chain walker follows only fields a layout names, so it finds no links out of the scenario. | `app/tag_tree.rs:11`, `app/tag_api/chain.rs:157` (`links`) |
| R20 | Structure BSP and lightmap tags cannot be opened: their data is in a BSP region the tag document does not read. | `app/tag_doc.rs:267`, `app/tag_doc.rs:39` |
| R21 | A group with more than 600 tags lists the first 600 and "N more: type to narrow". | `ui/tags.rs:26`, `ui/tags.rs:181` |

### What cannot be seen or edited in the viewport

| # | Problem | Where |
|---|---|---|
| R22 | The scene reads eleven scenario blocks, all placements. Trigger volumes, kill triggers, BSP-switch volumes, decals and portals are not read by the editor. | `crates/formats/map/src/scenario_layout.rs` (`BlockKind`) |
| R23 | Collision is not drawn. The parsers exist (`Bsp::decode_structure`, `Collision::decode`, `PhysicsModel::decode` in `crates/formats/tags/src/collision.rs`) and no editor code calls them. Picking uses render triangles, so a wall with collision and no render geometry cannot be seen, clicked or measured. | `crates/tools/scenario-edit/src/geometry.rs:880` (`surface`) |
| R24 | Instanced geometry has no collision reader anywhere in the tree; `crates/formats/tags/src/structure.rs` reads instance definitions for drawing only. | `crates/formats/tags/src/structure.rs:185` |
| R25 | Spawn points and netgame flags are glyphs with one colour per block. Facing, team and flag type are in the inspector only. | `crates/tools/scenario-edit/src/geometry.rs:694` to `geometry.rs:745` |

### The viewport's picture

| # | Problem | Where |
|---|---|---|
| R26 | The viewport has its own renderer and shader. It draws a base map, detail maps and the lightmap, and lights objects from the floor under them. The game's material passes (bump, environment, specular), the unlit-BSP implementation, per-vertex light streams, light probes, fog planes, animated shaders and posed models are missing ([editor.md](editor.md), "What the viewport shows, and what it does not yet"). | `crates/tools/scenario-edit/src/render.rs:666` (`Viewport`), shaders at `render.rs:709` and `render.rs:812` |
| R27 | It already shares the native renderer's loaders (`native_render::load::bucket_streams` and the lightmap group choice) and then draws with its own pipeline, so the two can drift. | `crates/tools/scenario-edit/src/geometry.rs:339`, `geometry.rs:402` |

## How it works

### The layout

One window, five fixed regions. A tool changes the centre and the
side panels' contents, never the regions.

```
+----------------------------------------------------------------------------+
| File  Edit  View  Game  Help                                                |
+----------------------------------------------------------------------------+
| [Map] [Variants] [Playlists] [UI] [HUD]    zanzibar *     (link) [>] [Edit] |
+--------------+---------------------------------------------+---------------+
| Scene | Tags | [move][rot][scale] [grid][angle]  Show v  Lighting v        |
| Maps         |                                             |  Inspector    |
|              |                 viewport                    |               |
|  tree        |                                             |  fields       |
|              |                                             |               |
+--------------+---------------------------------------------+---------------+
| Log | Game | Pending | Data and bytes                                       |
+----------------------------------------------------------------------------+
| last message                          hint for what is under the mouse  fps |
+----------------------------------------------------------------------------+
```

- **The bar** under the menu (`ui/bar.rs`) is fixed, 36 pt, and drawn in
  every tool. Left: the tools as tabs (the Tools list of R1 goes away).
  Centre: the open document's name and its unsaved mark. Right: the
  game link (below).
- **Left dock**: **Scene** (the outliner: placements, then Volumes and
  Collision groups), **Tags** (the scenario tag tree) and **Maps** as
  three tabs of one full-height column. Maps has the whole column when
  picked. The Map tool is click-to-edit.md's: `ToolKind::Tags` is a tab,
  not a tool.
- **Windows keep one meaning** (R2). Each tool has its own window keys
  (`variants-list`, `variants-settings`, ...), shown only in that tool
  and placed by the default layout in the same regions. The Scene,
  Viewport and Inspector windows are the Map tool's alone.
  `Keys::layout` gives each tool its node; a saved `h2-editor.ini` with
  the old three IDs still loads, since those IDs stay the Map tool's.
- **The viewport toolbar** is icon buttons with tooltips that name the
  action and its key, in groups: gizmo (move, rotate, scale, axes),
  snapping (a toggle and a step combo each, no right-click), camera
  (frame all, frame selection, fly speed), **Show** (the layers popover)
  and **Lighting**. The mouse help moves to the status bar's hint slot,
  which changes with what the pointer is over.
- **Empty states** (R7) are a centred card with the one action that
  moves on: **Open a disc or map...** with no source, the map grid with
  a source and no map, "Click something in the level" with a map and no
  selection.
- **Save** (R6): Cmd+S saves to where the document came from when that
  is a package or a file the editor wrote; a disc map or a read-only
  source goes to **Save as package...** with the name filled in.
  Shift+Cmd+S is Save as.
- **Go to anything** (Cmd+P, `ui/palette.rs`): one box over maps, tags
  (`tag_tree::search`), placements (`outliner::matches`) and commands
  (the `Action`s the menus hold). Enter runs or selects.
- **One undo** for the Map tool is click-to-edit.md's journal (slice
  C4); the inspector's own undo buttons (R3) go with it.

**The look** (`theme.rs`). The theme gains named tokens and the UI stops
using literals: surface levels (`BG`, `PANEL`, `RAISED`), text levels,
the accent, and one colour per layer class (below) used by both the
scene crate and the legend. Type has three sizes (12 pt captions, 15 pt
body, 18 pt titles). Primary actions are full buttons; `small_button`
is for inline row actions only. A section is a header row with a rule,
drawn by one helper (`theme::section`). Status is a pill: a filled
rounded rect with a dot, an icon and text (`theme::pill`), so state
never rests on colour alone.

### The game link

`play::Play` already knows everything; the design is where it shows.
`Play::link_view(&App) -> LinkView` reduces it to one value, with no
UI types, that the bar, the Game panel and `pair.status` all read:

| `LinkState` | Pill | Primary action |
|---|---|---|
| `NoGame` | grey, "No game" | **Play** (F5) when a map is open |
| `Found { map, matches }` | amber, "Halo 2 is running zanzibar" | **Pair** |
| `Connecting` | amber, "Connecting..." | Cancel |
| `Viewing { map, phase, tick }` | blue, "Viewing zanzibar · playing" | **Edit** switch, off |
| `Editing { .. }` | green, "Editing zanzibar live" | **Edit** switch, on |
| `Blocked { why }` | amber, "Edits stay here: co-op session" | the reason as the tooltip |
| `OtherMap { game, open }` | amber, "Game runs lockout" | **Open lockout** |
| `Lost { why }` | red, "Game lost" | **Pair again** |

- The pill is in the bar in every tool (R12). Clicking it opens a
  popover: the games found, by map name and phase, one click to pair;
  Unpair; Auto-pair. The socket path field, **Find**, the `h2` binary
  path and the launch log are under **Advanced** in the Game panel
  (R14).
- **Play**, **Stop**, **Pause**, **Step** and **Resume** are icon
  buttons beside the pill, shown when they apply. A **Game** menu holds
  the same commands with their keys, so F5 works in every tool.
- **View and Edit** are live-mirror.md's switch. It replaces the "Send
  edits live" checkbox (R15). While `Editing`, the viewport has a 2 pt
  green frame and the inspector's title row shows a "Live" pill; while
  `Blocked`, both are amber. Unpaired, neither is drawn.
- The Game panel keeps the detail in three sections drawn by
  `theme::section`: **Session** (map, phase, tick, stream rate and
  delay, the session kind), **Player** (go to player, teleport, the
  spawn choice) and **Advanced**. The pending list is its own tab.
- The status bar loses the chip.

### The scenario tag tree

The **Tags** tab answers "what does this level use". Its first root is
the scenario; the old group and path trees stay as **All tags**.

```
Scenario  scenarios\multi\zanzibar\zanzibar
  Structure
    BSP 0   sbsp ...        lightmap  ltmp ...
  Skies
    sky  ...
  Objects
    Scenery (41)            <- the palette, an entry per row
      scen objects\...        3 placed
        Model  hlmt ...     <- tag_api::chain from here down
    Weapons (12) ...
  Globals   matg ...
  Other references (n)
All tags (by group | by path)
Not reachable from the scenario (n)
```

- `app/tag_tree.rs` gains `scenario_tree(doc, scenario) -> ScenarioTree`
  with no UI. Its named roots come from what the tree already reads:
  the BSP and lightmap pairs (`native_render::load::scenario_bsps`), the
  skies (`scene::scenario_skies`), and every object palette of
  `scenario_edit::layout::BLOCKS` with the count of placements that name
  each entry. Below a palette entry it is `tag_api::chain`.
- **Other references** come from a reference scan for tags with no
  layout: `tag_api::scan_refs(doc, tag)` walks the tag's bytes at
  4-byte steps and keeps each (group, datum) pair whose datum is a tag of
  the map and whose group is that tag's group or one of its parents.
  The same scan feeds `chain` for any group the registry lacks, which
  closes R19 for the scenario and for every other unlabelled group.
- **Not reachable** is the set difference of all tags and the tags the
  walk from the scenario and globals reaches; it is where UI, HUD and
  sound tags with no placement are found, with **All tags**.
- Rows show the edited mark, the shared-cache mark and, with a game
  paired, the live mark of the pending list. Selecting a row makes the
  tag the inspector's subject (`Selection::Tag`), as a jump does.
  Selecting a palette entry also outlines its placements in the
  viewport.
- The 600-row cap (R21) goes: every list is drawn through ImGui's list
  clipper, so only visible rows cost anything.
- The scenario's own fields (R18) get a `scnr.txt` definition in
  `crates/formats/tags/src/defs`, started with the fields this
  repository already has evidence for (the skies, the palettes and
  placement blocks of `layout.rs`, the trigger volumes, the BSP block)
  and padding elsewhere. Blocks that `layout::BLOCKS` owns stay
  read-only in the field panel with a link to the Scene tab.
- BSP and lightmap tags (R20) open read-only: `TagDoc::load` maps each
  BSP region the way `native_render::load::load_world` reads it, one
  region per BSP, and the row says which BSP it belongs to.

Agent API: `tag.tree {root?: "scenario", depth?}` returns the same
tree; `tag.refs {tag}` returns one tag's links with how each was found
(`field` or `scan`).

### Editing tags in the running game

The mechanism is live-mirror.md's "How an edit applies" and "Undo and
sync" (its slice M7). This section fixes the parts that page leaves
open and says what is safe.

**One sender for the whole document.** `play/sync.rs` diffs placement
blocks. `play/tag_sync.rs` does the same for `TagDoc`: `TagBaseline`
holds, per address, the bytes the game is taken to hold, and
`tag_sync::diff(&TagBaseline, &TagDoc)` turns `TagDoc::patches()` into
writes. A write names its place by address: a tag's metadata sits in
the game's memory at the address the map file gives it
([map-format.md](../formats/map-format.md)), which is the address
`TagDoc` already uses. `edit.batch` therefore takes
`{at, value, was}` beside the `{tag, path, value, was}` form, checks
that `at..at+len` lies inside one loaded tag, and journals it under that
tag. The scenario's placement writes move to the same batch, so one
undo step that touches a placement and a tag is one tick in the game.

**What is safe, by what the edit is.** The editor classifies an edit
before sending it; the class is shown as a badge on the field.

| Edit | Class | Why |
|---|---|---|
| A scalar, flag, enum, colour, angle or bounds field of a tag in the map's own metadata | Poke (frame, use, creation or unknown, from `tags::defs`' `applies`) | same size, same address |
| A string ID field set to a string the loaded map has | Poke | the value exists in the game |
| A tag reference or datum set to a tag the loaded map has | Poke, shown as "unknown" until the field has an `applies` | the target is loaded; whether the engine cached the old one is per field |
| A placement field | Poke, next creation; plus **Respawn now** once `world.object.respawn` exists | read when the object is made |
| A trigger or kill volume's frame or size | Poke, next use | the volume test reads the scenario each time (**S**) |
| A block element added, duplicated or removed; a block grown (`TagDoc::grown`) | Reload | the game's block is laid out where the map put it |
| A new string ID (`TagDoc::new_strings`), a new tag, an imported bundle that adds tags, a unique copy | Reload | the game's tables do not hold them |
| A reference to a tag the loaded map lacks (another map's, a shared cache's not loaded) | Reload | no datum to point at |
| Raw resources: bitmap pixels, sound data, model and BSP geometry, lightmap pages | Reload | they are not in tag memory |
| Collision and MOPP edits | Reload | Havok built its shapes at load |
| Anything in a structure BSP region | Reload | the editor does not write there live |

**What blocks all of it.** Not paired; View mode; another map or
another checksum in the game; a session that refuses writes
(`SessionKind::writes`); the timeline scrubbed into the past. The
`Blocked` pill names which. The game enforces the session rule itself
(R17): `edit.set`, `edit.batch` and `edit.revert` call the same check
`sim.state.set` does.

**Undo.** The document is the truth. Undo and redo change the document;
the next diff against the baseline is what goes to the game, so undo
sends the old bytes with no second log. A `MISMATCH` (the game holds
neither the baseline's bytes nor the new ones) writes nothing, marks the
field in the pending list as "the game changed this" and offers **Take
the game's value** or **Overwrite**.

**Leaving.** Unpair asks once: **Keep the game as it is** or **Put it
back as it loaded** (`edit.revert`). On pairing with a game whose
`edit.changes` journal is not empty, the editor lists the entries and
offers to adopt them into the baseline or revert them, so a second
session never works over unknown bytes.

**Pending** (its own tab): one row per edit not in the game, grouped by
class, each with its reason. Buttons: **Send** (View to Edit),
**Respawn now**, **Reload map** (Play again for a game the editor
started; `map.reload` otherwise).

### Layers and overlays

Everything the viewport can show is a **layer** with a toggle, a count,
a colour and a legend row, in the toolbar's **Show** popover and in a
Layers section at the top of the Scene tab. `scenario_edit::layers`
owns the list; `view.layers {set?}` reads and sets it; presets
(Art, Gameplay, Collision, Everything) are one click.

| Group | Layer | Data | Read by | Drawn as |
|---|---|---|---|---|
| World | Structure, Sky, Objects, Fog | the map's art | `scene.rs` | the lit picture |
| Placements | one per `BlockKind` | the placement blocks | `layout.rs` | models and glyphs |
| Gameplay | Spawn points | starting locations | `layout.rs` | a pawn with a facing arrow, tinted by team |
| Gameplay | Netgame flags | netgame flags | `layout.rs` | a pennant by flag type and team, the type as a label; flags of one type and index joined as an outline (hills, territories) |
| Gameplay | Netgame equipment | netgame equipment | `layout.rs` | the item's model with its respawn time as a label |
| Volumes | Trigger volumes | the scenario's trigger volume block | `scenario_edit::volumes` | a translucent oriented box with wire edges and its name |
| Volumes | Kill volumes | the kill trigger block, which names trigger volumes | `scenario_edit::volumes` | the same box in the kill colour, hatched |
| Volumes | BSP switches | the BSP-switch block | `scenario_edit::volumes` | the box with "from to" as its label |
| Collision | Level collision | the structure's collision BSP | `scenario_edit::collision_view` | surfaces as translucent faces with wire edges |
| Collision | Collision only | level collision surfaces with no drawn surface over them | `collision_view::classify` | solid red hatch: the invisible walls |
| Collision | Instance collision | instanced geometry's collision | a new reader in `tags::structure` | as level collision |
| Collision | Object collision | each placed object's `coll` BSPs and `phmo` shapes | `tags::collision` | wire shapes at the placement's transform |
| Structure | Portals and clusters | modeling-targets.md, "Portals, clusters and visibility" | that design | outlines, read-only |
| Live | players, objects and the rest | the stream | live-mirror.md, "The editor's UI" | that design |

**Collision only** is the layer that answers "where are the invisible
barriers". A collision surface is in it when no render triangle of the
structure or its instances lies on its plane (within a tolerance) and
overlaps it. The test is geometric, so it does not depend on any
surface flag being named. `classify` also carries each surface's raw
flags, breakable byte and material, and the legend splits by them where
their meaning is established: bit 3 is breakable (`collision.rs`), and
the engine's surface matcher tests bit 1 and bit 3 against the query's
flags ([sim-rewrite-collision.md](../engine/sim-rewrite-collision.md)),
which is how a surface stops one kind of thing and not another. Which
queries set those bits (players, vehicles, projectiles, the camera) is
the research that turns "collision only" into "stops players only";
until it is done the legend shows the raw bits.

Halo 2's scenario has no block this repository knows as a soft ceiling
or soft kill barrier (**S**). Out-of-bounds play is stopped by kill
volumes and by collision-only surfaces, which are the two layers above.
If a disc survey finds another block, it is one more Volumes layer.

**The overlay pass.** `scenario_edit::render::Viewport` gains one pass
after the picture: filled translucent triangles, then lines, depth
tested against the picture's depth with an **X-ray** toggle that draws
hidden parts at a quarter strength. It replaces the single-purpose
`set_overlay` (the Halo 1 overlay becomes a layer). Labels are ImGui
text projected by the camera, culled by distance.

**Picking.** A visible overlay shape is pickable: a box by ray against
its oriented box, a collision surface by ray against its polygon. A
pick selects it (`Selection::Volume`, `Selection::Surface`), the Scene
tab highlights its row, and the inspector shows its fields. Hidden
layers are never picked.

### Editing volumes and barriers

- **Trigger, kill and BSP-switch volumes** are scenario blocks edited as
  placements are: new entries in `layout::BLOCKS` with their fields, the
  same `Set`, `Add` and `Remove` ops, the same undo. The gizmo moves and
  turns the box; scale handles on its faces change its size. Removing
  one is refused while a script, a kill trigger or a BSP switch names it
  by index (the refusal names the holder), as scenario-editor.md's
  section 6 does for placements. Adding one grows the block under the
  existing growth rule.
- **Make a kill volume** is one command: add a trigger volume at the
  camera focus and a kill trigger that names it.
- **Collision-only surfaces** cannot be edited in place: the collision
  BSP, its leaf block and the structure's MOPP all index them. The
  editor offers three edits, each a step of
  [modeling-targets.md](modeling-targets.md), "Structure collision":
  1. **Add a barrier.** Draw a box or a quad extruded to a slab in the
     viewport; it compiles to its own small BSP, merges into the level's
     and the MOPP is rebuilt (that page's step 2; modeling.md slices 8
     and 11). This is the first barrier edit built, since it appends and
     so keeps every existing index.
  2. **Remove or move a barrier.** Select surfaces of the Collision only
     layer (click, or **Select connected**) and delete or drag them; the
     level's collision is rebuilt from the remaining surfaces (step 3;
     slice 13). It is offered for converted Halo 1 maps first and for
     retail maps after the index-holder survey that page asks for.
  3. **Make drawn geometry solid, or not.** From render: build collision
     for selected render faces; the reverse deletes the collision under
     them. Both are step 3.
- Every barrier edit is class Reload in a paired game. The pending list
  says so before the first one is made.
- Object collision (`coll`, `phmo`) is edited in the tag, through the
  inspector, with the overlay as the live preview; reshaping it with
  handles is modeling.md's slices 8 and 9.

### The viewport on the native renderer

The viewport stops being a second renderer. It becomes a host of
`crates/rewrites/native-render`, the way Theater's playerless view is
(`crates/extensions/game/src/theater/host_view.rs`), and keeps only
what the game has no reason to draw: markers, the gizmo, the overlay
pass and labels. The **Lighting** menu picks the picture:

| Mode | Draws | For |
|---|---|---|
| **Game** | the native renderer's stages | what the level looks like |
| **Flat** | base maps at full brightness, no lightmap, no fog | reading geometry in dark places |
| **Lightmap only** | the lightmap on white | checking the lighting |
| **Collision** | Flat at a third strength with the Collision layers on | finding barriers |

It lands in three steps, each a better picture than the last.

1. **The native world and models, fixed path.** `scenario-edit` loads
   the BSP with `native_render::load::load_world`, uploads it with
   `GpuWorld::new` and placed objects with `GpuModels`, builds a
   `RenderCamera` from the editor's camera, and calls `prepare` and
   `encode` with the viewport's own colour and depth views
   (`native-render/src/gpu.rs`). The lightmap page decode and the
   first-bitmap decode move from the game crate
   (`native_world_glue::decode_lightmap`, `decode_first`) into
   `native_render::load`, so the editor does not depend on the game
   host. `scenario-edit`'s world, model and sky pipelines and their
   shaders are deleted; `view.stats` reports from the native world. The
   look is the renderer's `world.wgsl`: base map times lightmap, as the
   game's fixed path.
2. **The game's material passes.** The real look is the programs path:
   each material's translated layer-3 pass
   ([native-world.md](../graphics/native-world.md), "Match the look").
   It needs two things only the running game supplies: the view
   constants the D3D device holds (`program::ViewInputs`) and stage
   palettes read from guest memory. The editor supplies both without a
   game:
   - **Tag memory as guest memory.** A map's tags sit at fixed
     addresses, so `MapMemory` implements `GuestRead` over the open
     map's tag space (what `TagDoc::read` already serves). The
     renderer's modules that read only tag data run unchanged.
   - **A static scene.** `crates/rewrites/scene-static` fills a
     `FrameScene` through `SceneBuilder` from the map and the editor's
     camera: one view, every cluster, an instance per placement in its
     bind pose, the lights the tags place. This is the reader
     [framescene.md](../graphics/framescene.md) section 4.5 leaves for
     the editor.
   - **View constants** computed from the camera by the same function
     the game's glue uses once it is split from the device read.
   The first task of this step is a survey: each `GuestRead` use in
   `native-render` and `scene-extract`, marked tag data (served by
   `MapMemory`) or run state (synthesised by `scene-static`), written to
   `docs/graphics/notes/`.
3. **The game's own frame, when paired.** With a game that renders
   natively, **Game camera** (live-mirror.md M2) draws the game's sealed
   scene from the stream in the same viewport, so posed characters,
   effects and dynamic lights are the game's.

The overlay pass draws after the native stages into the same targets.
It uses the native pass's depth rule (`gpu::DepthRule`) so its depth
test agrees with the picture. Picking moves to the native world's
triangles with the same ray code.

## Interfaces

New methods of the editor's socket API, each with a provider in
`crates/apps/studio/src/api` and a line in [agent-api.md](agent-api.md)
when built. Game-side methods are live-mirror.md's table; this page adds
only the `at` form of `edit.batch` and the session check on `edit.set`
and `edit.revert`.

| Method | Params | Result |
|---|---|---|
| `pair.status` | none | gains `link`: `{state, text, why, actions: [name]}`, the `LinkView` |
| `pair.edit` | `{enabled}` | the mode after it; `REFUSED` with the reason |
| `tag.tree` | `{root?, depth?}` | the scenario tree: sections, rows with `tag`, `group`, `name`, `placed`, `children` |
| `tag.refs` | `{tag}` | `[{to, via, found: field \| scan}]` |
| `view.layers` | `{set?: {name: bool}, preset?, xray?}` | every layer: `name`, `group`, `on`, `count` |
| `view.lighting` | `{mode?}` | the mode |
| `volume.list` | `{kind?}` | `[{kind, index, name, origin, forward, up, size, named_by}]` |
| `volume.set`, `volume.add`, `volume.remove` | as `scenario.set` and its kin | the volume; refusals name the holder |
| `collision.surfaces` | `{bsp?, class?, limit?}` | counts by class, and surfaces with `plane`, `flags`, `material`, `class`, `ring` |
| `collision.at` | `{ray \| point}` | the surface hit and its class |
| `ui.palette` | `{query}` | what Cmd+P lists for the query |

`view.snapshot` draws the layers that are on, so a check can look at the
overlays in a picture.

## Slices

Each ships something the user sees, in this order unless a row says
otherwise. Lanes write code and tests on invented data (the scenario
crate's fixture, `play::tests`' fake game); the coordinator builds and
runs them. Sizes are modeling.md's: S under 800 lines with tests, M
under 2,500, L more.

| # | Slice | Size | Needs | Files | Tests and acceptance |
|---|---|---|---|---|---|
| P1 | **The game link in the bar** | S | nothing | below | below |
| P2 | **The bar and the layout**: tool tabs; Maps, Scene as tabs of one column; per-tool window keys; icon toolbar with tooltips and step combos; status-bar hint slot; empty-state cards; `theme` tokens, `section`, type sizes | M | P1 | `ui.rs`, `ui/bar.rs`, `ui/map_list.rs`, `theme.rs`, `headless.rs` | `the_default_layout_places_every_window` (each tool's windows exist and none shares an ID); a saved layout with the three old IDs loads; `--do` scripts that click `Scenario` in the Tools list click the Map tab (the script step maps the old label); no colour literal outside `theme.rs` in `ui/` (a source test); the guide's screenshots are re-shot by `scripts/editor-screenshots.sh` |
| P3 | **Save where it came from, and Go to anything** | S | P2 | `app.rs`, `app/commands.rs`, `ui/palette.rs`, `api/ui_screens.rs` (for `ui.palette`) | Cmd+S on an opened package rewrites it with no dialog; on a disc map opens the package form; `ui.palette "zan"` lists the map, its tags and matching placements in that order; Enter on a command dispatches its `Action` |
| P4 | **The scenario tag tree** | M | nothing | `app/tag_tree.rs`, `app/tag_api/chain.rs` (`scan_refs`), `ui/tags.rs`, `api/tags.rs`, `crates/apps/studio-proto/src/lib.rs` | fixture scenario with a palette, a sky and a BSP: `tag.tree` lists them under their sections with placement counts; `scan_refs` on an invented tag with no layout finds a planted reference and rejects a pair whose group does not match the datum's tag; a tag only a UI tag names is under Not reachable; a 5,000-tag invented group draws every row (list clipper) |
| P5 | **`scnr.txt` and BSP-region tags** | M | P4 | `crates/formats/tags/src/defs/scnr.txt`, `app/tag_doc.rs` | the registry's own tiling test passes for `scnr`; blocks owned by `layout::BLOCKS` are read-only in the panel; a fixture `sbsp` in its own region opens read-only; opt-in disc test: every disc scenario walks through the definition |
| P6 | **Layers and the overlay pass** | M | nothing | `crates/tools/scenario-edit/src/layers.rs`, `render.rs`, `scene.rs`, `viewport.rs`, `ui.rs`, `api/view.rs` | `view.layers` toggles change `view.snapshot`'s pixels on the fixture; a hidden layer is not picked; the Halo 1 overlay is a layer and its menu item still works; X-ray draws a box behind a wall at reduced strength (pixel test, skipped without an adapter) |
| P7 | **Spawn and netgame layers**: facing arrows, team tint, flag-type pennants and labels, joined outlines | S | P6 | `geometry.rs`, `layers.rs`, `theme.rs` | glyph geometry tests: an arrow points along the facing; two flags of one type and index make one outline; colours come from the theme's layer tokens |
| P8 | **Trigger, kill and BSP-switch volumes: see and pick** | M | P6 | `scenario-edit/src/volumes.rs`, `layout.rs`, `scenario.rs`, `api/scenario.rs` (`volume.list`) | research first: the volume's size field and the kill trigger element, from the retail readers with `h2 disasm`, recorded in scenario-editor.md with labels. Fixture with two volumes and a kill trigger: `volume.list` gives frames and sizes; a ray through a box picks it; the kill box is in the Kill layer |
| P9 | **Edit volumes** | M | P8 | `layout.rs` (`BLOCKS`), `edit.rs`, `session.rs`, `ui/inspector.rs`, `viewport.rs` | move, turn and resize through the gizmo are one undo step each; the saved map differs only in the volume's bytes and the checksum; removing a volume a kill trigger names is refused by name; Make a kill volume adds both entries and undoes as one step |
| P10 | **Level collision and the Collision only layer** | M | P6 | `scenario-edit/src/collision_view.rs`, `geometry.rs`, `api/view.rs` (`collision.*`) | invented room with one wall that has no render face: `classify` puts that wall alone in Collision only; a render face 1 mm off its plane still covers it; `collision.at` on the wall returns its class; triangulated rings equal `Bsp::surface_ring` |
| P11 | **Object and instance collision** | M | P10 | `crates/formats/tags/src/structure.rs` (instance collision reader), `collision_view.rs` | research first: the instance definition's collision block, labelled in structure-bsp.md. Fixture object with a `coll` box and a `phmo` sphere: both drawn at the placement's transform; an invented instance's collision reads back |
| P12 | **Tag edits reach the game** (live-mirror M7): `edit.map`; `edit.batch` with `was` and the `at` form; the session check on every `edit.*` write; `tag_sync`; View and Edit; class badges; the Pending tab; leave and rejoin | L | P1 | `crates/extensions/agent-proto/src/hot.rs`, `crates/extensions/agent/src/hot.rs`, `crates/extensions/hot-tags/src/lib.rs`, `play/tag_sync.rs`, `play.rs`, `ui/play.rs`, `fields.rs`, `api/pair.rs`, `api/mirror.rs` | live-mirror.md's M7 row, plus: an `at` write outside every tag is refused; an edit that grows a block is listed under Reload and nothing is sent; a `MISMATCH` offers both choices and each leaves document and baseline equal; pairing with a non-empty journal lists it; `edit.set` in a co-op session is refused by the game |
| P13 | **`applies` on fields** | S, repeatable | P12 | `crates/formats/tags/src/defs/*.txt`, `crates/formats/tags/src/defs.rs` | the parser reads `applies=frame\|use\|creation` with its evidence; hot-iteration.md's "What shows when" rows are the first entries; a field with none is `unknown` |
| P14 | **Viewport step 1: the native world** | L | P6 | `crates/rewrites/native-render/src/load.rs` (the moved decodes), `scenario-edit/src/render.rs`, `scene.rs`, `geometry.rs`, `viewport.rs`, `api/view.rs` | the fixture map draws through `GpuWorld` into the viewport target (GPU test, skipped without an adapter); `view.stats` keeps its fields; the Lighting modes change the snapshot; picking returns the same placement and surface as before on the fixture; `scenario-edit` holds no world or model shader; the Theater host view still builds on the moved functions |
| P15 | **Viewport step 2: the material passes** | L | P14 | `crates/rewrites/scene-static` (new), `native-render/src/program.rs`, `scenario-edit/src/render.rs` | the survey note first. `MapMemory` reads equal `TagDoc::read`; a static scene of the fixture has one view, its clusters and an instance per placement; an invented material with a translated pass draws its pass, one without falls back to step 1's look and is counted in `view.stats` |
| P16 | **Add a barrier** | L | P10; modeling.md slices 8 and 11 | `crates/formats/collision-build`, `scenario-edit/src/session.rs`, `viewport.rs` | modeling.md slice 11's acceptance, plus: the new solid is in Collision only after the edit; undo removes it; a paired game lists it under Reload |
| P17 | **Remove, move and From render** | L | P16; modeling.md slice 13 | as P16 | slice 13's acceptance; refused on a retail map until the survey note exists, with that reason |

P4, P6 and P12 need nothing from each other and can run side by side
after P1. Click-to-edit's C3 to C6 and live-mirror's M2 to M11 keep
their own rows on their pages; P2 is where C4's "the Tags windows docked
into Map" lands.

### P1, ready to hand off

**Goal.** In every tool, the user can see whether a game is linked and
whether edits are going to it, and can Play, Pair, Unpair, Pause and
switch edits on or off without opening a panel.

**Build.**

1. `play.rs`: add `LinkState` (the eight variants of the table in "The
   game link"), `LinkTone` (`Idle`, `Notice`, `View`, `Live`, `Warn`,
   `Error`) and `LinkView { state, tone, text, why: Option<String>,
   can_play, can_stop, can_pause, paused }`. Add
   `Play::link_view(&self, app: &App) -> LinkView`, built from `conn`,
   `status`, `notice(app)`, `map_match(app)`, `live_edits` and
   `edits_blocked(app)`. `Blocked` is "paired, edits wanted, and
   `edits_blocked` gives a reason other than the switch being off";
   `OtherMap` is `MapMatch::Different`. No ImGui types.
2. `theme.rs`: `BAR: f32 = 36.0`; `pill(ui, tone_colour, icon, text) ->
   bool` (a filled rounded rect behind the text, returns clicked); a
   colour per `LinkTone` beside `OK`, `WARN` and `ERROR`.
3. `ui/bar.rs` (new): `draw(ui, app, play: &mut PlayUi, fonts)`. A
   fixed window under the menu bar, built as `status_bar` builds its own
   (`ui.rs:956`), at the main viewport's `work_pos`, height `BAR`. This
   slice fills only its right side: the pill, then Play or Stop, Pause
   or Resume and Step when `LinkView` allows, then the **Edit** switch
   bound to `Play::live_edits`. The pill's popover lists
   `play.finder.games` by map and phase with a **Pair** row action,
   **Unpair** when paired, and the Auto-pair checkbox. A pill click with
   one found game pairs with it.
4. `ui.rs`: call `bar::draw` after `menu_bar` in `frame`; move the
   dock host down by `theme::BAR` (`dockspace`, `ui.rs:463`); draw the
   Game window in every tool (move `self.play.window` out of the
   `ToolKind::Scenario` arm at `ui.rs:155`) and move
   `self.play.shortcuts` beside the global shortcuts; add a **Game**
   menu (Play, Stop, Pair with the running game, Unpair, Pause, Step,
   Send edits live); remove the chip and `status_pair_button` from
   `status_bar` (`ui.rs:984` to `ui.rs:988`). The default layout puts
   Game before Log (`ui.rs:78`).
5. `ui/play.rs`: the panel opens with the same pill; the socket field,
   **Find**, the socket list, the `h2` path and the log path go under a
   collapsed **Advanced** header; one **Pair** button remains. While
   `Live`, `viewport_window` draws a 2 pt frame in the tone's colour
   around the image rect (`ViewportView::shown`).
6. `api/pair.rs`: `pair.status` gains `link` (`state`, `text`, `why`).
   Update its `MethodDef` result text and
   [agent-api.md](agent-api.md)'s row. `Bridge` publishes the
   `LinkView` beside what it already publishes.
7. Docs in the same branch: editor.md's "The window" and "Play and
   pair" (the bar, the Game menu, the pill's states; the status chip
   sentence goes), editor-guide.md where it tells the user to open the
   Game tab, and this page's R11 to R15 deleted.

**Tests** (no game data; the fake game of `play::tests`).

- `play::tests::the_link_view_names_every_state`: drive the fake game
  through no game, found, connecting, paired with the same map, paired
  with another map, edits on, a co-op session and a dropped socket;
  assert `state`, `tone` and that `text` is not empty for each.
- `play::tests::blocked_names_the_reason`: co-op gives `Blocked` whose
  `why` is `SessionKind::writes`' text.
- `api::tests`: `pair.status` carries `link.state` for off and
  connected (it already runs against a published bridge).
- `headless::tests::the_game_link_shows_in_every_tool`: for each
  `ToolKind`, render three frames and find the pill's item by its label
  prefix in `h.scene.items`, as
  `the_tags_tool_edits_a_field_through_its_widgets` finds its widgets;
  click **Edit** and assert `play.live_edits` flipped. Skipped without
  an adapter.
- Existing `play::tests` pass unchanged.

**Acceptance.** The pill and its buttons are on screen in all six
tools; F5 starts the game from any tool; with a game paired and edits
on, the viewport is framed and the pill reads "Editing <map> live";
unpaired there is no frame; the status bar holds no game text; a layout
saved before the change loads with the Game tab present.

**Not in P1.** The tool tabs and the document name in the bar (P2);
View and Edit as live-mirror defines them with a pending list (P12):
here the switch is the existing `live_edits` flag under its new name.

## Rules and limits

- The editor stays native: Dear ImGui and wgpu, no web view, no second
  UI toolkit.
- A new control dispatches an `Action` or calls `Play`; no logic lives
  in `ui/`. Every new action has a socket method and a row in
  agent-api.md's command table.
- Nothing is written to a game without the Edit switch on, and the
  switch never turns itself on for a game the editor did not start
  (live-mirror.md).
- The editor never reads the game's memory: live state comes from the
  stream, edits go through named methods.
- Overlays draw only what the map's own data says. A class whose meaning
  is not established shows its raw value, not a guess.
- Barrier edits change collision, never render geometry, and mark the
  map's pathfinding stale where the level has any.
- Colours and sizes come from `theme.rs`. The scene crate takes its
  layer colours as data from the caller.
- No game data in the repository: fixtures are invented, disc tests are
  `#[ignore]`d, and the guide's screenshots are scripted and stored in
  Git LFS.

## Evidence

- **Code**: the Game panel, F5 and the chip are in the Scenario arm
  only (`crates/apps/studio/src/ui.rs`, `Studio::frame` and
  `status_bar`), while `PlayUi::update` runs before the tool match.
- **Code**: `edit.set` is registered with no session check
  (`crates/extensions/agent/src/hot.rs`); `sim.state.set` has one
  (`crates/extensions/agent/src/sim/mod.rs`).
- **Code**: `TagDoc::patches` gives `(address, bytes)` pairs in the
  map's own addresses (`crates/apps/studio/src/app/tag_doc.rs`); the
  game's tag index gives each tag's address and size from the same
  table (`crates/extensions/hot-tags/src/index.rs`).
- **Code**: the registry has no `scnr` definition
  (`crates/formats/tags/src/defs`), and the chain walker reads only
  layout fields (`crates/apps/studio/src/app/tag_api/chain.rs`).
- **Code**: trigger volumes are a scenario block with a frame of two
  directions and an origin, read by the Halo 1 converter
  (`crates/tools/h1convert/src/convert.rs`, `TRIGGER_VOLUMES`); the
  BSP-switch block is read by `crates/apps/cli/src/live_geometry.rs`.
- **Code**: a drawn, lightmapped level from a map with no game exists
  twice: the editor's own pipeline and Theater's host view over
  `GpuWorld::new` (`crates/extensions/game/src/theater/host_view.rs`).
  `native-render`, `scenario-edit` and `studio` are all on wgpu 30.
- **Code**: `FrameScene` is filled only from guest memory
  (`crates/rewrites/render-bridge/src/scene_step.rs`,
  `crates/rewrites/scene-extract`); `crates/rewrites/frame-scene` itself
  depends on neither a guest nor a GPU.
- **S**: a tag's size in the index covers its blocks, so `scan_refs`
  over that range sees every reference. Verified by P4's fixture test
  and an opt-in disc test that compares the scan with the layout walk
  for every group the registry has.
- **S**: the trigger volume's size field and the kill trigger element's
  layout. Verified by P8's read of the retail volume test.
- **S**: the engine tests a trigger volume against the scenario's bytes
  each time, so a poke shows at once. Verified in a run by moving a
  volume with `edit.set` and reading the script result.
- **Design**: the geometric test for Collision only; the layer list;
  the bar; addresses in `edit.batch`.

## Not verified

- Whether the fixed path's look (step 1) is close enough to the game's
  on maps with many unlit or multi-pass materials; a side-by-side of
  `view.snapshot` and the game's frame on the user's disc shows it.
- Whether the overlay pass can share the native pass's depth buffer as
  it is, or must write its own depth for the level.
- Which collision queries set the flag bits the surface matcher tests,
  and so what each surface class stops.
- Whether any scenario or structure block beyond trigger volumes, kill
  triggers and collision bounds play in Halo 2.
- Whether a tag reference poked live is honoured for groups the engine
  resolves once at load.
- The bar's height and the pill's contrast on a non-HiDPI display.

## Related

- [editor.md](editor.md): the editor as built.
- [live-mirror.md](live-mirror.md): the stream, apply classes, session
  rules, slices M1 to M11.
- [click-to-edit.md](click-to-edit.md): the Map tool, navigation, the
  undo journal, slices C1 to C6.
- [modeling.md](modeling.md) and
  [modeling-targets.md](modeling-targets.md): collision compiling and
  the level collision steps the barrier edits use.
- [scenario-editor.md](scenario-editor.md): blocks, fields and the
  add and remove rules volumes follow.
- [agent-api.md](agent-api.md): the socket API the new methods join.
- [native-renderer.md](../graphics/native-renderer.md),
  [native-world.md](../graphics/native-world.md) and
  [framescene.md](../graphics/framescene.md): the renderer the viewport
  hosts.
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*` in the
  game and what shows when.
