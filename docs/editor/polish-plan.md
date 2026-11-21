# h2-editor polish: what is wrong, the design, and the slices

Status: P1 to P7, P12 and P18 are implemented. The remaining slices, and the game link past its bar (G1 to G9), describe the design.
Code: `crates/apps/studio/src` (the window: `ui.rs`, `ui/`, `play.rs`, `viewport.rs`, `theme.rs`), `crates/tools/scenario-edit/src` (the scene and its renderer). Tests: named per slice.
Switch: none.

This page lists what makes `h2-editor` hard to use, with the place in the
code for each, and the design that fixes it: one layout, a game link that
makes the editor and the running game one tool (selection, edits and
the camera shared both ways, Play as one key), the scenario's tags as a
tree, overlays for the volumes and collision that the game does not
draw, edits to those, and a viewport drawn by the native renderer. It ends with slices sized for one implementer each. It builds
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
| R3 | Scenario and Tags are two tools over one map. They have separate selections, a second map picker, and separate undo: the inspector's tag section carries its own **Undo tag edit** and **Redo tag edit** buttons because Cmd+Z goes to the active tool's stack. | `ui/tags.rs:62` (`map_picker`), `ui/inspector.rs:245` to `ui/inspector.rs:251`, `app.rs:585` (`can_undo`) |

### Tags and the scenario

| # | Problem | Where |
|---|---|---|

### What cannot be seen or edited in the viewport

| # | Problem | Where |
|---|---|---|
| R22 | Scene handles eleven scenario placement blocks. Trigger volumes, kill triggers, BSP-switch volumes, decals and portals have no Scene overlay. | `crates/tools/scenario-edit/src/layout.rs:12` (`BlockKind`) |
| R23 | Collision is not drawn. The parsers exist (`Bsp::decode_structure`, `Collision::decode`, `PhysicsModel::decode` in `crates/formats/tags/src/collision.rs`) and no editor code calls them. Picking uses render triangles, so a wall with collision and no render geometry cannot be seen, clicked or measured. | `crates/tools/scenario-edit/src/geometry.rs:880` (`surface`) |
| R24 | Instanced geometry has no collision reader anywhere in the tree; `crates/formats/tags/src/structure.rs` reads instance definitions for drawing only. | `crates/formats/tags/src/structure.rs:185` |

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
  every tool. Left: the tools as tabs.
  Centre: the open document's name and its unsaved mark. Right: the
  game link (below).
- **Left dock**: **Scene** (the outliner: placements, then Volumes and
  Collision groups), **Tags** (the scenario tag tree) and **Maps** as
  three tabs of one full-height column. Maps has the whole column when
  picked. The Map tool is click-to-edit.md's: `ToolKind::Tags` is a tab,
  not a top-level tool. The Tags tree, fields and Data and bytes tab
  live alongside the viewport and share the Map document.
- **Windows keep one meaning**. Each tool has its own window keys
  (`variants-list`, `variants-settings`, ...), shown only in that tool
  and placed by the default layout in the same regions. The Scene,
  Viewport and Inspector windows are the Map tool's alone.
  `Keys::layout` gives each tool its node; a saved `h2-editor.ini` with
  the old three IDs still loads, since those IDs stay the Map tool's.
- **The viewport toolbar** is icon buttons with tooltips that name the
  action and its key, in groups: gizmo (move, rotate, scale, axes),
  snapping (a toggle and a step combo each, no right-click), camera
  (frame all, frame selection, fly speed), **Show** (the layers popover)
  (P6) and **Lighting** (P14). Mouse help uses the status bar's hint slot,
  which changes with what the pointer is over.
- **Empty states** are a centred card with the one action that
  moves on: **Open a disc or map...** with no source, the map grid with
  a source and no map, "Click something in the level" with a map and no
  selection.
- **Save** uses `App::save_target`. Cmd+S writes the open package or the
  map's last standalone output. An editable map from a source opens
  **Save as package...** with the name filled in. Shift+Cmd+S opens
  **Save as...** for a standalone map file.
- **Go to anything** (Cmd+P, `ui/palette.rs`) searches maps, tags
  (`tag_tree::search`), placements (`commands::placement_matches`) and
  menu actions in that order. Up and Down move the selection, Enter
  runs it and Esc closes the popup.
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

The editor and the running game are one tool. What is selected in one
is marked in the other, an edit is in the game as it is made, either
camera can lead, Play is one key, and a problem shows on the thing it
is about. Everything crosses the two agent APIs: the editor calls the
game's named methods ([agent-api.md](../runtime/agent-api.md)) and
reads its stream ([live-mirror.md](live-mirror.md)); the game never
calls the editor and holds no editor state past a lease.

#### The link's state

`Play::link_view(&App) -> LinkView` reduces the pairing to one value,
with no UI types, that the bar, the Game panel and `pair.status` all
read:

| `LinkState` | Pill | Primary action |
|---|---|---|
| `NoGame` | grey, "No game" | **Play** (F5) when a map is open |
| `Found { map, matches }` | amber, "Halo 2 is running zanzibar" | **Pair**; shown only when auto-pair left it alone |
| `Connecting` | amber, "Connecting..." | Cancel |
| `Reconnecting { since }` | amber, "Reconnecting..." | Cancel |
| `Viewing { map, phase, tick }` | blue, "Viewing zanzibar · playing" | **Edit** switch, off |
| `Editing { .. }` | green, "Editing zanzibar live" | **Edit** switch, on |
| `Blocked { why }` | amber, "Edits stay here: co-op session" | the reason as the tooltip |
| `OtherMap { game, open }` | amber, "Game runs lockout" | **Open lockout** |
| `Lost { why }` | red, "Game lost" | **Pair again** |

- The pill is in the bar in every tool. Clicking it opens a popover
  with games by map name and phase, Unpair and Auto-pair. Clicking the
  pill when one game is offered pairs directly.
- **Play**, **Stop**, **Pause**, **Step** and **Resume** are icon
  buttons beside the pill, shown when they apply. A **Game** menu holds
  the same commands with their keys, so F5 works in every tool.
- The **Edit** switch changes between View and Edit. While `Editing`,
  the viewport has a 2 pt green frame and the inspector shows a "Live"
  pill. While `Blocked`, both are amber.
- `LinkView::issues` counts what needs the user: edits waiting, edits
  that need a reload, writes the game refused, fields the game changed.
  The pill appends the largest count ("Editing zanzibar live · 3 need a
  reload") and a click on it opens the Pending tab.
- The Game panel's **Session** section shows the map, phase, tick,
  stream rate, delay and session kind. **Player** holds go to player,
  teleport and the spawn choice. **Advanced** starts collapsed and
  holds the socket field, **Find**, the `h2` binary path and the
  launch log.

#### Finding and keeping a game

The user never types a port or picks a socket. `play::pair::Finder`
lists the games in the discovery directory
([agent-api.md](../runtime/agent-api.md), "Finding a game") once a
second, and `play::auto` (`auto::choose`) picks from that list:

1. The game the editor started with Play is paired as soon as its
   socket answers, in Edit.
2. One local game running: paired, in View.
3. Several games running: the one whose map is the open map, when
   exactly one is; otherwise none, and the pill says how many run and
   its popover lists them.
4. A game the user unpaired from stays unpaired until its process
   exits.

When the link drops, `Play` asks the finder whether the game is still
there. A game whose process is gone gives `NoGame` and one log line. A
game that is still listed gives `Reconnecting`: the editor reopens the
socket after 0.25, 0.5, 1 and 2 seconds, then every 2 seconds, for 30
seconds (`auto::Backoff`). On return it asks `edit.map`; the same epoch
and checksum keep the mode, the baseline and the pending list, and
anything else starts in View with the pending list rebuilt. After 30
seconds the state is `Lost` with the socket's last error. A game that
restarts on a new socket is a new game and pairs by rules 1 to 3.

A `tcp://` endpoint (the Xbox app) and a socket outside the discovery
directory are typed once under **Advanced**.

#### Play is one action

F5 (**Play** in the bar and the Game menu, `pair.playtest`) takes the
open map, with its unsaved edits, into a game, whatever is running.
`play::playtest::decide` is a pure function of the link's state, the
pending list and the game's methods, and returns one step:

| The link | The pending edits | F5 does |
|---|---|---|
| no game | any | writes the play copy and starts `h2 play` on it, paired in Edit (`Launch`) |
| a local game found, not paired | any | pairs, then decides again |
| paired, the open map | none, or pokes only | sends the pokes, resumes a paused game, raises the game window (`Resume`) |
| paired, the open map | some need a reload | reloads the map from a new play copy, then puts the player back (`Reload`) |
| paired, another map, the editor's own game | any | the same reload, onto the open map |
| paired, another map, a game the editor did not start | any | asks: **Load zanzibar in the game** (`map.reload {file}`) or **Open lockout here** |

- A reload is `map.reload {file}` where the game lists it and a
  relaunch of the editor's own game where it does not. A game the
  editor did not start and that lacks `map.reload` leaves the edits
  pending and says to save the map and load it in the game.
- Sending edits to a game the editor did not start asks once per
  pairing ("Send your edits to this game?"), since F5 never turns Edit
  on by itself there.
- **The game's place is kept.** Before a reload or relaunch the editor
  takes the local player's position and facing from the stream
  (`playtest::Place`). When the map is back in gameplay and
  `SPAWN_AFTER_TICKS` have run, it sends them with `sim.state.set`.
  The spawn choice gains **Where I was**, the default once a game has
  run this map; Level's spawn, Editor camera and Selected placement
  remain.
- **The editor's state is kept.** A playtest changes no selection,
  camera, undo history or open tag. The game keeps running when the
  editor takes focus. **Pause the game while I edit** (Game menu, off
  by default) pauses it when the editor's window gains focus and
  resumes it on F5.
- After the step the editor calls `editor.focus`, so the game window
  is in front.

#### The game shows the link

A paired editor sends `editor.link {editor, mode, text?, ttl_ms}` once
a second. `h2 play` draws a small plate in a corner of the game window
while the lease holds: "Editor · viewing", "Editor · editing live",
"Editor · edits blocked", with `text` under it for the one line the
editor wants seen there ("3 edits need a reload", "Aim select"). The
plate goes when the editor unpairs (`{clear: true}`), exits or stops
answering (the lease runs out, 3 seconds by default). A game with no
window (`h2 serve`) answers `{shown: false, why}` and the editor stops
asking. The indicator is presentation: it is sent in View mode and in
every session kind.

#### Selection both ways

**From the game to the editor.**

- **Click.** A live object in the viewport is picked like a placement.
  An object with a placement link selects that placement; one with
  none (a projectile, a dropped weapon) selects its definition tag, as
  click-to-edit.md's click does. With the game paused and the viewport
  on the player's eyes or the game camera, this is clicking what the
  game showed.
- **Aim select**, the edit cursor (Game menu, the crosshair button in
  the bar, `mirror.aim_select`). While it is on,
  `play::select::aim_pick` casts the local player's aim ray each
  sample: from the game view's camera when the stream has the `camera`
  layer, otherwise from the unit's position raised by `EYE_HEIGHT`
  along the `players` layer's aim vector. The ray is tested against
  every live object's bounding sphere (the model bounds the viewport
  draws it with) and against the level (`geometry::surface`); the
  nearest object in front of the level is the **aim candidate**. A
  candidate held for `AIM_DWELL` (0.6 s) becomes the editor's
  selection. The candidate and the selection are both marked in the
  game, so the player sees the cursor land and then take.
- `mirror.pick {from: "aim"}` returns the candidate (`object`,
  `placement`, `tag`, `point`, `distance`) without selecting, and
  `{ray}` does the same for any ray.

**From the editor to the game.** The editor sends its selection as
marks with `editor.marks`, on every change and once a second while it
holds (`play::presence`):

| Selected | Mark |
|---|---|
| a placement whose object is in the stream | `{object}`: the game draws that object's bounds and follows it |
| a placement with no live object, a trigger or kill volume | `{box}` at the entry's frame and size |
| a tag from the tag tree | `{object}` for every live object made from it, up to 64 |
| the aim candidate | `{object}` in the candidate style |

The game draws marks as wire boxes in a pass after its world, depth
tested, with hidden edges at a quarter strength when `xray` is set.
Colours are the theme's and travel as data. A game that cannot draw
them (no native world pass, no window) answers `{drawn: false, why}`
and the Game panel says so once. Marks are presentation: they are sent
in View mode and in every session kind.

#### Shared camera

Either side follows the other. `mirror.camera {mode}` and the viewport
toolbar's camera menu choose:

| Mode | Leads | Does |
|---|---|---|
| **Free** | neither | the editor's fly, orbit and pan camera |
| **Follow player** | the game | orbits the local player at the kept offset, from the `players` layer |
| **Player's eyes** | the game | at the unit's eye looking along its aim; exact once the `camera` layer exists (live-mirror M2), `EYE_HEIGHT` above the unit until then |
| **Game follows editor** | the editor | the game draws from the editor's camera: `editor.camera {pos, forward, up, fov_deg}` on every camera change, at most 30 a second. The player does not move. Leaving the mode, unpairing or a 1 second lease running out gives the game its camera back |

One-shot moves are beside them: **Go to player** (the editor camera
jumps to the player), **Player here** (T: `sim.state.set` to the point
under the cursor) and **Player here, looking my way** (Shift+T: the
same with the camera's yaw and pitch, so the game opens on what the
editor has framed). Moving the player is a write and follows the
session rules; **Game follows editor** moves no simulation state and is
allowed wherever the game draws for one local machine.

#### Edits land as you make them

In Edit, a change is in the game within one push interval (200 ms)
where it can be, and on the pending list with its reason where it
cannot. Tag fields go as live-mirror.md's "How an edit applies" says.
Objects follow the document too (`play/objects.rs`):

| In the editor | In the game |
|---|---|
| a placement moved or turned | its `scnr` bytes (`edit.batch`) and, for its live object, `world.object.set` with the new frame |
| a placement added | `sim.spawn` from the entry's tag and frame; the entry is "a preview until the map reloads" |
| a placement deleted | `world.object.delete` on its object; the entry is "still in the game's scenario until the map reloads" |
| a placement field read at creation changed | the poke, then **Respawn now** (`world.object.respawn`) for the objects that exist |

**Undo works across both, with one history.** The document is the
truth for objects as it is for tags. `objects::diff` compares the
document's placements with what was last sent for each live object and
each preview, and its output is the calls to make. Undo and redo
change the document, so the next diff moves the object back, deletes
the preview of an undone add, or respawns the object of an undone
delete. No second undo log exists, and a step that touched a tag and a
placement undoes in the game as it does here.

A drag sends `world.object.set` at most once per push interval and
once more when the gizmo is released. A game without `world.object.*`
gets the `scnr` bytes alone and the pending list says objects move at
their next creation.

#### Problems show where you are

- **On the field.** A write the game refused marks its field in the
  inspector with a red badge and the game's reason; a field the game
  changed (`MISMATCH`) shows amber with **Take the game's value** and
  **Overwrite** beside it. The field's row in the Scene tab or the tag
  tree carries the same mark.
- **In the viewport.** A game answer that failed (a teleport refused, a
  respawn refused, a mark the game cannot draw, the link dropping and
  returning) is a toast at the viewport's top right for 4 seconds, as
  well as a log line.
- **On the pill.** `LinkView::issues`, as above.
- **In the game.** The indicator plate's second line.
- `pair.status` carries `link.issues` as `[{kind, text, target}]`,
  where `target` is the field, placement or tag to jump to.

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

- [scenario_tree](../../crates/apps/studio/src/app/tag_tree.rs) builds
  the named sections from the source BSP and lightmap pairs, skies and
  globals, and counts current placements by palette slot. Object
  entries use the tag chain for their children.
- **Other references** uses the layout walk and the aligned reference
  scan. The scan accepts a pair when its full datum resolves in this
  map and its group matches the target's group or an ancestor. It
  supplies links for a group with no registry layout and fills in
  scenario links outside the structural schema.
- **Not reachable** lists the tags the walk from the scenario and
  globals does not reach. The walk visits each tag once and is
  independent of the depth shown under palette entries.
- Tag rows show edited and shared-cache marks, plus a pending mark
  for edited tag metadata awaiting a matching paired game. A tag row
  selects the inspector subject. A palette row also selects the
  placements that use that slot for the viewport outlines.
- The scenario rows and the lists of tags use Dear ImGui's list
  clipper.
- The scenario fields in `crates/formats/tags/src/defs/scnr.txt` appear
  read-only in the field panel. Placement and palette blocks link to
  their Scene block, where the scenario session owns edits.
- `TagDoc::load` keeps each BSP region's bytes in its own address space.
  A structure BSP or lightmap row identifies its BSP and opens its
  fields for in-place editing with undo. Block operations use the
  region's original allocation.

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
| Anything in a structure BSP region | Reload | the map loads BSP regions by section |

**What blocks all of it.** Not paired; View mode; another map or
another checksum in the game; a session that refuses writes
(`SessionKind::writes`); the timeline scrubbed into the past. The
`Blocked` pill names which. The game enforces the session rule at the write boundary: `edit.set`, `edit.batch` and `edit.revert` call the same check
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

### Field names and descriptions

The editor takes its field names and descriptions from the checked-in
`crates/formats/tags/src/defs/*.txt`. A field's optional `doc="..."`
appears in its tooltip, the inspector detail row and the `description`
member of `tag.fields`. Fields without established names keep their
offset labels.

Assembly's retail Xbox plugins serve as a reference with permission
from one of Assembly's creators, the owner of this repository. Each
reference-sourced name is written in our naming style. Its evidence
includes `A Halo2Xbox <filename>`, and its description is written here.
The [field-name research note](notes/tag-field-names.md) records the
permission, coverage and layout disagreements.

Our layout governs every match. Existing named fields keep their names
and types. A reference field or set of fields can name an unknown or
padding span when it tiles the span exactly. A block must also agree on
element size and its nested offsets. Conflicting spans retain their
local layout. Shared object and unit layers are defined once, and
definitions used by converters preserve the paths and value types those
callers write.

The developer comparison lives in `crates/formats/tag-names`.
`every_plugin_merges_against_our_definitions` reads a local retail
directory on request and prints per-group coverage and conflicts.
Its input is listed in [switches.md](../runtime/switches.md#tools).
The editor needs only its built-in registry.

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
when built. The game's methods for the link follow in their own table.

| Method | Params | Result |
|---|---|---|
| `pair.status` | none | includes `link` with `{state, text, why}` from `LinkView` |
| `pair.edit` | `{enabled}` | queues the Edit switch change and returns `{requested}`. Read the mode in `pair.status`. |
| `pair.status` | none | `link` gains `issues: [{kind, text, target}]` and the `reconnecting` state |
| `pair.playtest` | `{confirm?: "send" \| "load"}` | what F5 does now: `{step: launch \| pair \| resume \| reload \| ask, ask?, place?}`. `confirm` answers an `ask` |
| `mirror.pick` | `{from: "aim"}` or `{ray: {origin, dir}}` | the live object hit: `{object, placement, tag, point, distance}`, or `null` |
| `mirror.aim_select` | `{enabled?}` | whether the aim cursor selects, and the current candidate |
| `mirror.camera` | `{mode?: free \| follow \| eyes \| lead, target?}` | the mode and what it follows; `lead` is **Game follows editor** |
| `mirror.player` | `{to: "cursor" \| "camera" \| [x, y, z], facing?: bool}` | `{requested}`: the player moved there, with the camera's yaw and pitch when `facing` |
| `tag.tree` | `{root?, depth?}` | the scenario tree: sections, rows with `tag`, `group`, `name`, `placed`, `children` |
| `tag.refs` | `{tag}` | `[{to, via, found: field \| scan}]` |
| `view.layers` | `{set?: {name: bool}, preset?, xray?}` | every layer: `name`, `group`, `on`, `count` |
| `view.lighting` | `{mode?}` | the mode |
| `volume.list` | `{kind?}` | `[{kind, index, name, origin, forward, up, size, named_by}]` |
| `volume.set`, `volume.add`, `volume.remove` | as `scenario.set` and its kin | the volume; refusals name the holder |
| `collision.surfaces` | `{bsp?, class?, limit?}` | counts by class, and surfaces with `plane`, `flags`, `material`, `class`, `ring` |
| `collision.at` | `{ray \| point}` | the surface hit and its class |
| `ui.palette` | `{query, run?}` | visible entries and per-kind totals, optionally runs the entry at index `run` |
| `tag.fields` | `{tag, path?}` | `{fields: [{offset, name, kind, size, value, description}]}`. A block element path selects its rows. A missing description is `null` |

`view.snapshot` draws the layers that are on, so a check can look at the
overlays in a picture.

The game's methods the link adds, named in `agent_proto::editor` and
`agent_proto::sim`, answered by `crates/extensions/agent`, and listed by
`session.methods` where the host registers them. The `world.object.*`
and `map.reload` rows are live-mirror.md's, with the shapes fixed here.

| Method | Access | Params | Result |
|---|---|---|---|
| `editor.link` | read | `{editor, mode: view \| edit \| blocked, text?, ttl_ms?}` or `{clear: true}`. `text` is at most 64 characters; `ttl_ms` is 500 to 10000, default 3000 | `{shown, why?, expires_ms}`. `shown: false` with `why` on a host with no window |
| `editor.focus` | act | none | `{focused, why?}`: the game window raised |
| `editor.marks` | read | `{marks: [mark], ttl_ms?}`, at most 256, replacing the last set. A mark is `{object: "<8 hex>"}`, `{box: {origin, forward, up, size}}` or `{point: [x, y, z]}`, with `color: [r, g, b]`, `style?: selected \| candidate`, `label?`, `xray?` | `{held, drawn, why?, refused: [{index, why}]}`. An object that is gone is refused by index; the rest hold |
| `editor.camera` | act | `{pos, forward, up?, fov_deg?, ttl_ms?}` (default lease 1000) or `{release: true}` | `{held, why?}`. Refused on a Live session and where the host draws no view of its own |
| `world.object.set` | act | `{id, pos?, forward?, up?}` | `{at, object}` after the next tick, through the engine's own move |
| `world.object.delete` | act | `{id}` | `{at, deleted}` |
| `world.object.respawn` | act | `{id}` or `{placement: {block, index}}` | `{at, old, object, linked}` |
| `map.reload` | act | `{file?}` | `{queued, map}` |

`editor.link`, `editor.marks` and `editor.camera` hold a lease and never
write simulation state, so they are allowed in View and in every
session the stream is. `world.object.*` and `map.reload` are host
writes: `SessionKind::writes` and the tick boundary, as `sim.state.set`.

## Slices

Each ships something the user sees, in this order unless a row says
otherwise. Lanes write code and tests on invented data (the scenario
crate's fixture, `play::tests`' fake game); the coordinator builds and
runs them. Sizes are modeling.md's: S under 800 lines with tests, M
under 2,500, L more.

| # | Slice | Size | Needs | Files | Tests and acceptance |
|---|---|---|---|---|---|
| P1 | **The game link in the bar** | S | nothing | `play.rs`, `ui/bar.rs`, `ui/play.rs`, `api/pair.rs` | below |
| P2 | **The bar and the layout**: tool tabs; Maps, Scene as tabs of one column; per-tool window keys; icon toolbar with tooltips and step combos; status-bar hint slot; empty-state cards; `theme` tokens, `section`, type sizes | M | P1 | `ui.rs`, `ui/bar.rs`, `ui/map_list.rs`, `theme.rs`, `headless.rs` | `the_default_layout_places_every_window` (each tool's windows exist and none shares an ID); a saved layout with the three old IDs loads; `--do` scripts that click `Scenario` in the Tools list click the Map tab (the script step maps the old label); no colour literal outside `theme.rs` in `ui/` (a source test); the guide's screenshots are re-shot by `scripts/editor-screenshots.sh` |
| P3 | **Save where it came from, and Go to anything** | S | P2 | `app.rs`, `app/commands.rs`, `ui/palette.rs`, `api/ui_screens.rs` (for `ui.palette`) | Cmd+S on an opened package rewrites it with no dialog; on a disc map opens the package form; `ui.palette` with the query `"blast"` lists the invented map, its tag and matching placements in that order; Enter on a command dispatches its `Action` |
| P4 | **The scenario tag tree** | M | nothing | `app/tag_tree.rs`, `app/tag_api/chain.rs` (`scan_refs`), `ui/tags.rs`, `api/tags.rs`, `crates/apps/studio-proto/src/lib.rs` | fixture scenario with a palette, a sky and a BSP: `tag.tree` lists them under their sections with placement counts; `scan_refs` on an invented tag with no layout finds a planted reference and rejects a pair whose group does not match the datum's tag; a tag only a UI tag names is under Not reachable; a 5,000-tag invented group draws every row (list clipper) |
| P5 | **`scnr.txt` and BSP-region tags** | M | P4 | `crates/formats/tags/src/defs/scnr.txt`, `app/tag_doc.rs` | the registry's own tiling test passes for `scnr`; blocks owned by `layout::BLOCKS` are read-only in the panel; fixture BSP and lightmap edits save at their own file offsets and undo; opt-in disc test: every disc scenario walks through the definition |
| P6 | **Layers and the overlay pass** | M | nothing | `crates/tools/scenario-edit/src/layers.rs`, `render.rs`, `scene.rs`, `viewport.rs`, `ui.rs`, `api/view.rs` | `view.layers` toggles change `view.snapshot`'s pixels on the fixture; a hidden layer is not picked; the Halo 1 overlay is a layer and its menu item still works; X-ray draws a box behind a wall at reduced strength (pixel test, skipped without an adapter) |
| P7 | **Spawn and netgame layers**: facing arrows, team tint, flag-type pennants and labels, joined outlines | S | P6 | `geometry.rs`, `layers.rs`, `theme.rs` | glyph geometry tests: an arrow points along the facing; two flags of one type and index make one outline; colours come from the theme's layer tokens |
| P8 | **Trigger, kill and BSP-switch volumes: see and pick** | M | P6 | `scenario-edit/src/volumes.rs`, `layout.rs`, `scenario.rs`, `api/scenario.rs` (`volume.list`) | research first: the volume's size field and the kill trigger element, from the retail readers with `h2 disasm`, recorded in scenario-editor.md with labels. Fixture with two volumes and a kill trigger: `volume.list` gives frames and sizes; a ray through a box picks it; the kill box is in the Kill layer |
| P9 | **Edit volumes** | M | P8 | `layout.rs` (`BLOCKS`), `edit.rs`, `session.rs`, `ui/inspector.rs`, `viewport.rs` | move, turn and resize through the gizmo are one undo step each; the saved map differs only in the volume's bytes and the checksum; removing a volume a kill trigger names is refused by name; Make a kill volume adds both entries and undoes as one step |
| P10 | **Level collision and the Collision only layer** | M | P6 | `scenario-edit/src/collision_view.rs`, `geometry.rs`, `api/view.rs` (`collision.*`) | invented room with one wall that has no render face: `classify` puts that wall alone in Collision only; a render face 1 mm off its plane still covers it; `collision.at` on the wall returns its class; triangulated rings equal `Bsp::surface_ring` |
| P11 | **Object and instance collision** | M | P10 | `crates/formats/tags/src/structure.rs` (instance collision reader), `collision_view.rs` | research first: the instance definition's collision block, labelled in structure-bsp.md. Fixture object with a `coll` box and a `phmo` sphere: both drawn at the placement's transform; an invented instance's collision reads back |
| P12 | **Tag edits reach the game** (live-mirror M7): `edit.map`; `edit.batch` with `was` and the `at` form; the session check on every `edit.*` write; `tag_sync`; View and Edit; class badges; the Pending tab; leave and rejoin | L | P1 | `crates/extensions/agent-proto/src/hot.rs`, `crates/extensions/agent/src/hot.rs`, `crates/extensions/hot-tags/src/lib.rs`, `play/tag_sync.rs`, `play.rs`, `ui/play.rs`, `fields.rs`, `api/pair.rs`, `api/mirror.rs` | live-mirror.md's M7 row, plus: an `at` write outside every tag is refused; an edit that grows a block is listed under Reload and nothing is sent; a `MISMATCH` offers both choices and each leaves document and baseline equal; pairing with a non-empty journal lists it; `edit.set` in a co-op session is refused by the game |
| P13 | **`applies` on fields** | S, repeatable | P12 | `crates/formats/tags/src/defs/*.txt`, `crates/formats/tags/src/defs.rs` | the parser reads `applies=frame\|use\|creation` with its evidence; hot-iteration.md's "What shows when" rows are the first entries; a field with none is `unknown` |
| P18 | **Field names and descriptions** from checked-in definitions, with a developer comparison against the retail reference | M | P12 | `crates/formats/tags/src/defs.rs`, `crates/formats/tag-names`, `fields.rs`, `api/tag_fields.rs` | Descriptions parse and round-trip, and appear in root and nested socket rows. Definitions tile each element. Invented plugins exercise compatible fields, overruns, stride conflicts and game refusal. The opt-in comparison prints group coverage and disagreements. |
| P14 | **Viewport step 1: the native world** | L | P6 | `crates/rewrites/native-render/src/load.rs` (the moved decodes), `scenario-edit/src/render.rs`, `scene.rs`, `geometry.rs`, `viewport.rs`, `api/view.rs` | the fixture map draws through `GpuWorld` into the viewport target (GPU test, skipped without an adapter); `view.stats` keeps its fields; the Lighting modes change the snapshot; picking returns the same placement and surface as before on the fixture; `scenario-edit` holds no world or model shader; the Theater host view still builds on the moved functions |
| P15 | **Viewport step 2: the material passes** | L | P14 | `crates/rewrites/scene-static` (new), `native-render/src/program.rs`, `scenario-edit/src/render.rs` | the survey note first. `MapMemory` reads equal `TagDoc::read`; a static scene of the fixture has one view, its clusters and an instance per placement; an invented material with a translated pass draws its pass, one without falls back to step 1's look and is counted in `view.stats` |
| P16 | **Add a barrier** | L | P10; modeling.md slices 8 and 11 | `crates/formats/collision-build`, `scenario-edit/src/session.rs`, `viewport.rs` | modeling.md slice 11's acceptance, plus: the new solid is in Collision only after the edit; undo removes it; a paired game lists it under Reload |
| P17 | **Remove, move and From render** | L | P16; modeling.md slice 13 | as P16 | slice 13's acceptance; refused on a retail map until the survey note exists, with that reason |

The game link's slices. G3 to G5 are the game's side
(`crates/extensions/agent`, `agent-proto`, the `h2 play` shell and the
native renderer); the rest are the editor's. Fake game means the agent
crate's server with an invented game behind it, as `play::tests` use.

| # | Slice | Size | Needs | Files | Tests and acceptance |
|---|---|---|---|---|---|
| G1 | **The link looks after itself**: `auto::choose`, `Reconnecting` and `auto::Backoff`, the mode kept across a reconnect, the process check on a drop | S | P12 | `play/auto.rs`, `play.rs`, `play/pair.rs`, `ui/play.rs`, `api/pair.rs` | `choose` over invented finder lists gives rules 1 to 4; fake game: a dropped socket that returns inside the window comes back in the same mode with the baseline kept, a changed epoch comes back in View, a game gone from the list gives `NoGame`, 30 seconds of silence gives `Lost`; the backoff's delays are the table's |
| G2 | **Play is one action**: `playtest::decide`, `Place`, **Where I was**, the two asks, **Pause the game while I edit**, `pair.playtest` | M | G1 | `play/playtest.rs`, `play.rs`, `play/launch.rs`, `ui/play.rs`, `ui/bar.rs`, `api/pair.rs` | `decide` returns each row of the table for an invented link; fake game: F5 with pokes pending sends one `edit.batch`, resumes and calls `editor.focus`; with a reload pending and `map.reload` listed it reloads and sends the recorded place once gameplay has run `SPAWN_AFTER_TICKS`; a foreign game is asked before any write; selection, camera and undo depth are equal before and after |
| G3 | **The game shows the link**: `editor.link`, `editor.focus`, the plate in `h2 play` | S | nothing | `crates/extensions/agent-proto/src/editor.rs`, `crates/extensions/agent/src/editor.rs`, `crates/apps/app/src/overlay.rs`, `crates/apps/app/src/shell.rs`, `crates/apps/cli/src/play_cmd.rs` | the lease holds for `ttl_ms` and no longer on an invented clock; `clear` drops it; a host with no window answers `shown: false`; the plate's two lines are the mode's text and `text`; `session.methods` lists both where registered |
| G4 | **The game moves and remakes objects**: `world.object.set`, `.delete`, `.respawn`, `map.reload` | M | nothing | `crates/extensions/agent-proto/src/sim/mod.rs`, `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/agent/src/sim/mod.rs` | on the fake guest of `sim/calls/tests.rs`: each call reaches its engine entry with the arguments the retail callers pass, in the tick's `before` hook; a gone id answers `GONE`; co-op and client sessions refuse before anything is queued; `respawn` deletes then makes from the scenario entry and reports `linked`; `map.reload` queues the running map through `Call::QueueGame` |
| G5 | **The game draws marks and takes the editor's camera**: `editor.marks`, `editor.camera`, the marks pass | M | G3 | `crates/extensions/agent/src/editor.rs`, `crates/rewrites/native-render/src/marks.rs`, `crates/extensions/game/src/editor_glue.rs` | invented marks: an object mark's box is the object's bounding sphere at its transform, a box mark's twelve edges are its frame's; the lease drops them; more than 256 are refused; a pose held by `editor.camera` is the view the next frame is drawn from on an invented scene, and release returns the game's; a host without the native world pass answers `drawn: false` |
| G6 | **Selection both ways**: live objects pickable, aim select, `mirror.pick`, `mirror.aim_select`, marks from the selection | M | G1 | `play/select.rs`, `play/presence.rs`, `viewport.rs`, `viewport/mirror.rs`, `api/mirror.rs` | `aim_pick` on an invented sample picks the nearest object in front of an invented wall and none behind it; a candidate held for `AIM_DWELL` becomes the selection and a shorter one does not; fake game: selecting a placement with a live object sends one `editor.marks` with its handle, and again within the lease; a click on a live object with a placement link selects the placement |
| G7 | **Shared camera**: Follow player, Player's eyes, Game follows editor, Player here looking my way, `mirror.camera`, `mirror.player` | S | G6 | `play/presence.rs`, `camera.rs`, `ui/toolbar.rs`, `api/mirror.rs` | invented sample: Follow keeps its offset as the player moves, Eyes looks along the aim; fake game: `lead` sends `editor.camera` on a camera change, at most 30 a second, and `release` on leaving; `mirror.player {facing: true}` sends the camera's yaw and pitch |
| G8 | **Edits land as you make them**: `objects::diff`, objects follow the gizmo, previews, deletes, Respawn now, undo through the document | M | G1, G4 | `play/objects.rs`, `play/edits.rs`, `play.rs`, `api/mirror.rs` | fake game: a gizmo drag sends the `scnr` writes and at most one `world.object.set` a push interval, plus one on release; undo sends the old frame; an added placement spawns a preview and its undo deletes it; a deleted placement's undo respawns it; a game without `world.object.set` receives none and the pending list says why |
| G9 | **Problems show where you are**: field and row marks, viewport toasts, `LinkView::issues`, the indicator's second line | S | G1 | `play.rs`, `ui/inspector.rs`, `ui/play.rs`, `viewport.rs`, `api/pair.rs` | a refused write and a `MISMATCH` each mark their field and appear in `pair.status.link.issues` with a `target` that `nav.go` accepts; a refused teleport raises one toast that expires; the pill's text carries the largest count |

G3 and G4 need nothing from the editor and run beside G1. Live-mirror's
M2 (the `camera` layer) makes Player's eyes and aim select exact and is
not needed by any G slice. P4, P6 and P12 need nothing from each other
and can run side by side after P1. Click-to-edit's C3 to C6 and live-mirror's M2 to M11 keep
their own rows on their pages; P2 is where C4's "the Tags windows docked
into Map" lands.

### P1. The game link in the bar

`play.rs` reduces the connection, game status, discovery and edit
permission to `LinkView`. The bar and Game panel draw that state.
`play::pair::Bridge` publishes it for `pair.status`. The Game menu and
the bar use the same `play::pair::Request` commands as the socket API.
The stable Game and Log window keys allow saved dock layouts to load.

The tests use invented data.

- `play::tests::the_link_view_names_every_state` goes through all eight
  states against the fake game and checks the state, tone and label.
- `play::tests::blocked_names_the_reason` checks the co-op explanation
  from `SessionKind::writes`.
- `api::tests::pair_status_carries_link_view` checks the link published
  for the off and connected states. The fake game pairing test also
  switches from View to Edit through `pair.edit`.
- `headless::tests::the_game_link_shows_in_every_tool` finds the pill and
  exercises F5 in all six tools, then toggles Edit through the bar.
  It skips on machines without a wgpu adapter.

## Rules and limits

- The editor stays native: Dear ImGui and wgpu, no web view, no second
  UI toolkit.
- A new control dispatches an `Action` or calls `Play`; no logic lives
  in `ui/`. Every new action has a socket method and a row in
  agent-api.md's command table.
- Nothing is written to a game without the Edit switch on, and the
  switch never turns itself on for a game the editor did not start
  (live-mirror.md). F5 asks before its first write to such a game.
- The link indicator, marks and the held camera are presentation. They
  change no simulation state, hold a lease the editor must renew, and
  are gone when it stops.
- The editor reaches the game only through methods `session.methods`
  lists. A method the game lacks turns its feature off with the reason
  shown; nothing is approximated by another call.
- The editor never reads the game's memory: live state comes from the
  stream, edits go through named methods.
- Overlays draw only what the map's own data says. A class whose meaning
  is not established shows its raw value, not a guess.
- Barrier edits change collision, never render geometry, and mark the
  map's pathfinding stale where the level has any.
- Colours and sizes come from `theme.rs`. The scene crate takes its
  layer colours as data from the caller.
- Only the retail `Halo2Xbox` folder serves as an external field-name
  reference. New labels and descriptions use our wording and cite the
  reference in their evidence. Plugin files and verbatim text stay out
  of the repository.
- No game data in the repository: fixtures are invented, disc tests are
  `#[ignore]`d, and the guide's screenshots are scripted and stored in
  Git LFS.

## Evidence

- **Code**: `TagDoc::patches` gives `(address, bytes)` pairs in the
  map's own addresses (`crates/apps/studio/src/app/tag_doc.rs`); the
  game's tag index gives each tag's address and size from the same
  table (`crates/extensions/hot-tags/src/index.rs`).
- **Code**: `crates/formats/tags/src/defs/scnr.txt` names scenario fields
  with evidence. The chain walker supplements layout references with a
  scan of the scenario payload (`crates/apps/studio/src/app/tag_api/chain.rs`).
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
- **Code**: `TagDoc::scan_refs` scans the readable range owned by the
  tag. A known layout can also follow block pointers into the document's
  other readable ranges.
- **S**: the trigger volume's size field and the kill trigger element's
  layout. Verified by P8's read of the retail volume test.
- **S**: the engine tests a trigger volume against the scenario's bytes
  each time, so a poke shows at once. Verified in a run by moving a
  volume with `edit.set` and reading the script result.
- **Code**: the game's host draws a banner, the watermark and the
  Theater plates over the frame in `h2 play`
  (`crates/apps/app/src/shell.rs`, `overlay.rs`); nothing draws in world
  space after the world pass, and `h2 serve` has no window.
- **Code**: `sim.state.set` and `sim.spawn` queue guest calls that run
  in a `before` hook of the next tick
  (`crates/extensions/agent/src/sim/calls.rs`); the engine's object
  move and delete entries are named there and called by the teleport
  and by `sim.give`'s refusal path.
- **Code**: the stream carries each player's unit and aim vector and
  each object's placement link (`agent_proto::live::Player`, `Object`);
  it has no `camera` layer (`Layers::negotiate`).
- **Code**: the playerless view writes a pose into the game's render
  camera (`crates/extensions/game/src/spectator_view.rs`), which is the
  route `editor.camera` takes.
- **Design**: reference names must agree with the local byte layout.
  The [field-name research note](notes/tag-field-names.md) records the
  retail source, permission and discrepancies for the mapped groups.
- **Test**: `tag_fields_describe_root_and_nested_registry_fields` covers
  a described field, an absent description and nested element selection.
- **Design**: the geometric test for Collision only; the layer list;
  the bar; addresses in `edit.batch`; the lease on everything the game
  shows for the editor; objects derived from the document by a diff;
  aim select by dwell.
- **S**: a pose written as the playerless view writes it holds for a
  frame of a game with players in it, and moves nothing the simulation
  reads. Verified by G5 against `sim.hashes` with and without the held
  camera on one input replay.
- **S**: the engine's object move re-clusters a placed object when
  called from the tick's `before` hook as the teleport calls it.
  Verified by G4's run.

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
- Which reference-sourced field meanings can also be established from an
  engine reader or a controlled change on the owner's disc.
- `EYE_HEIGHT` against the game's view for each biped, and so how far
  aim select is off before the `camera` layer exists.
- Whether `AIM_DWELL` of 0.6 seconds selects too eagerly in play.
- That a preview made by `sim.spawn` behaves as the placed object will
  after the reload.
- The marks pass's cost in a frame, and its depth against the game's
  own on a map with a sky pass.

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
