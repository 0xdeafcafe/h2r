# Game type markers

Status: Partly built. The marker set and the area model exist (MK1), Studio draws the markers and answers `markers.list` and `markers.render` (MK4), Forge draws them as world lines and answers `render.markers` (MK6), the Forge document stores areas, which `forge.things` and `forge.propose` carry (MK2), a script's zone reads them (MK3), Studio edits them (MK5), Forge places and edits them in the monitor (MK7) and a script shows an area's wall in play (MK8); the skin, Studio's saved map and the wall on a client that does not run the script are design.
Code: `crates/formats/game-markers/src/lib.rs` (`Marker::mesh`), `crates/formats/game-markers/src/area.rs` (`Area`), `crates/tools/scenario-edit/src/marker_view.rs` (`placed`, `overlays`), `crates/apps/studio/src/api/markers.rs`, `crates/apps/studio/src/api/markers_check.rs` (the `markers` check), `crates/tools/scenario-edit/src/area_edit.rs` (handles, drag), `crates/tools/scenario-edit/src/session/area.rs` (`set_area`, `convert_hill`), `crates/apps/studio/src/viewport/area.rs`, `crates/apps/studio/src/ui/inspector/area.rs`, `crates/extensions/game/src/forge_glue/markers_world.rs` (Forge's source, `render.markers`), `crates/extensions/game/src/forge_glue/editing/area.rs` (Forge's draft area), `crates/formats/forge-doc/src/{area,entry,links}.rs`, `crates/extensions/forge/src/session.rs`, `crates/extensions/agent/src/forge/local.rs`, `crates/extensions/game/src/rules/world/zones.rs` (`Zones::new`), `crates/formats/rules/src/fake.rs` (`FakeGame::set_area`, `move_to`, `shown`), `crates/formats/rules/src/vm.rs` (`RulesExt::shown`), `crates/extensions/game/src/rules/world/walls.rs`, `crates/extensions/game/src/forge_glue/markers_world.rs` (`Wall`, `draw_walls`), `crates/live/bots/src/objective/zones.rs` (`first_hill`), `crates/extensions/agent-proto/src/rules.rs` (`rules.zones`). Tests: `crates/formats/game-markers/src/tests.rs`, `marker_view.rs`, `crates/apps/studio/src/api/tests/markers_tests.rs`, `crates/tools/scenario-edit/src/area_tests.rs`, `area_edit.rs`, `crates/apps/studio/src/viewport/area.rs`, `crates/extensions/game/src/forge_glue/markers_world/tests.rs`, `crates/extensions/game/src/forge_glue/editing/tests/area.rs`, `crates/formats/forge-doc/src/tests/area.rs`, `crates/extensions/forge/src/base_tests.rs`, `crates/extensions/agent/src/forge/tests.rs`, `zones.rs`' `a_flags_area_is_its_zone_before_corners_and_the_default`, `crates/formats/rules/tests/area_zones.rs`, `crates/formats/rules/tests/modes.rs` (`siege_shows_each_site_in_its_holders_colour`), `crates/extensions/game/src/rules/runtime/tests/world.rs`, `crates/extensions/game/src/rules/world/walls.rs`, `crates/live/bots/src/objective/zones/tests.rs`, `crates/extensions/agent/src/rules/tests.rs`.
Switch: none.

A game type's parts are rows of a scenario with no body: a player
spawn, a flag stand, a hill's corners, a teleporter's ends, an item
spawner, a trigger volume, a label a script reads. This page gives each
a marker, one shape drawn by Forge and by Studio alike, in the manner of
Halo 3's Forge and its in-game objective markers: a hologram in the
team's colour, with lines that read from across the map and a thin
translucent skin. It also gives a hill, a territory or a site an
**area**, a radius round its marker or a path of points dropped round
it, which the tools edit and a script's zone reads. The rule that must
hold: Forge, Studio and a render draw the same mesh for the same
placement, made by one function from numbers of ours.

Evidence labels are those of [docs-style.md](../meta/docs-style.md),
plus **C**: the retail games as publicly documented.

## What a map has and what draws it

| Part | Stored as | Forge draws | Studio draws |
|---|---|---|---|
| Player spawn | a player starting location: position, facing, team ([forge.md](forge.md) 1.2) | its marker, with the monitor on | its `Spawn` marker in the team's colour |
| Flag stand and return, bomb spawn and goal, ball spawn, territory | a netgame flag by type and team; its built-in label (`forge_doc::builtin_label`) | its marker | the marker of its flag type (`marker_view::flag_kind`) |
| Hill | netgame flags of types 11 to 18, one hill a type, each flag a corner (`hill_corner`, numbered by hill) | one `Hill` marker, at the corners' centre with `Area::of_corners` for three or more, else at each flag with the default area | three or more corners of one hill and number are one `Hill` with their hull's area; fewer are a `Hill` with the default area each |
| Teleporter | netgame flags of the two teleporter types and a channel | its marker; the Links tool lists them | the sender's or receiver's marker (a line between linked ends is not drawn) |
| Item spawner | a netgame equipment row | its marker, the outline of its class (the item tag's group) | the item's model, else its `Item` marker |
| Trigger volume, kill trigger, BSP switch | the scenario's trigger volumes and the blocks that name them (`scenario_edit::volumes`) | nothing | a `Volume` marker box by kind (`LayerId::Volume`) |
| Script label | a label op on a netgame flag (`forge_doc::GroundLabel`; [rules-engine.md](rules-engine.md#forge-labels)) | the label's marker (`ff_drop`, `siege_site`, ...), in orange when the last ground build could not honour it | not drawn |

A script's zone is made of the flags and their areas
(`game::rules::world::zones`): a flag's own area, else three or more
flags of one label and number as the convex hull of their places, else a
cylinder of one world unit ([rules-world-api.md](rules-world-api.md),
"Zones"). `rules.zones` reports each zone and the step that made it.

## The markers

`game_markers::Kind` is the set. `Marker { kind, team }` is one marker
and `Marker::mesh(&Palette)` its `Mesh`: `Line`s at full alpha and
`Tri`s at `FILL_ALPHA`, in the marker's own space, x the way it faces,
y left, z up, world units, the origin at the placement.
`Mesh::placed(&Place)` puts it in the world. A player stands 0.7 tall
(**S**: our number for the figure, set by eye against the game), and
every shape is sized against that. A part that lies on the floor is
`LIFT` above it.

| Kind | For | Shape | Parameters | Colour |
|---|---|---|---|---|
| `Spawn` | a player starting location | a standing figure's outline on a pad, and an arrow on the floor the way the player faces | none | team |
| `FlagStand` | `ctf_flag_spawn` | a pad, a pole and a pennant | none | team |
| `FlagReturn` | `ctf_flag_return` | a pad with an inner ring and an arrow down onto it | none | team |
| `BombSpawn` | `assault_bomb_spawn` | a pad with a diamond over it | none | team |
| `BombGoal` | `assault_bomb_return` | a target on the floor: two rings and a cross | none | team |
| `BallSpawn` | `oddball_spawn` | a pad with a ball of three rings over it | none | team |
| `Hill(Area)` | a hill | the area's wall, and a pip at the centre marker | the area | team |
| `Territory(Area)` | `territory` | the area's wall, and a post with a diamond on it | the area | team |
| `TeleporterSender` | `teleporter_sender` | a column with an arrow up | none | amber |
| `TeleporterReceiver` | `teleporter_receiver` | a column with an arrow down and an arrow out the way a player leaves | none | cyan |
| `Item(Weapon)`, `Item(Vehicle)`, `Item(Equipment)` | a netgame equipment row | a pad and the outline of what it makes: a long gun, a wedge with a cab (on a wider pad, with a facing arrow), a diamond | the class | team, else the class's own |
| `Volume(Volume, Solid)` | a trigger volume, a kill trigger, a soft boundary, a BSP switch | the solid, translucent: `Box { half }`, `Cylinder { radius, height }` or `Sphere { radius }` | the solid | by what it does: yellow, red, orange, blue |
| `Drop` | `ff_drop` | a pad, an arrow down and an arrow the way the wave faces | none | green |
| `Base` | `ff_base` | a ring and a house | none | team |
| `Cover` | `ff_cover` | a shield and an arrow the way it faces | none | yellow |
| `Site(Area)` | `siege_site` | a territory's shape | the area | violet |
| `Goal` | `siege_goal` | a target with a diamond over it | none | violet |
| `Line` | `siege_line` | a banner across the way a side faces, and an arrow | none | team, else violet |

- **The wall of an area** is `shapes::prism` over `Area::outline`: the
  outline at the area's lowest and highest, a post at each point and a
  translucent wall between, with no floor and no lid, as Halo 3's hill
  is a ring of light a player walks through (**C**).
- **An item spawner** in a drawer that has the item's model draws that
  model over the pad as the hologram and leaves the outline out. The
  outline is for a drawer with no model: a render with no map, a
  spawner whose tag did not load.
- **`Kind::of_label`** names the marker of a label's text, the flag
  types' built-in labels and the ground labels. A `hill_corner` is not
  one marker: a hill's only flag is a `Hill` with its area, and three or
  more corners together are one `Hill` with the area `Area::of_corners`
  gives, drawn at the centre it gives.
- **Counts.** Each shape's lines and triangles are fixed and are in the
  comments of `Kind::mesh`; an area's wall is three lines and two
  triangles a point of its outline, 32 points for a radius. The largest
  marker is a territory on a radius: 109 lines and 72 triangles.
- **Original shapes.** Every number is written in
  `crates/formats/game-markers/src/kind.rs` and
  `crates/formats/game-markers/src/shapes.rs`. Nothing is read from a
  disc and no shape is copied from a game's assets: the manner is Halo
  3's, the geometry is ours.

### Colours

`Palette` holds the eight teams' colours and a neutral one, and
`Kind::tint` says which a kind takes: `Tint::Team` (the team's, neutral
without one), `Tint::TeamOr` (the team's, else the kind's own) or
`Tint::Fixed`. The game fills `Palette::teams` from the loaded map as
the HUD kit's world marks do ([hud-kit.md](hud-kit.md), "World marks",
`hud_kit_world::colour`). A tool with no map draws
`Palette::default()`, stand-ins of ours in Halo 2's team order: red,
blue, yellow, green, purple, orange, brown, pink.

A drawer adds three states on top, by changing the colour it hands
`Kind::mesh`: selected is white, a label the last ground build could
not honour is the problem orange of `forge_glue::editing`, and a marker
of another game type than the one shown is `Mesh::faded` to a quarter.

## Areas

`game_markers::Area` is what a hill, a territory or a site covers:

```rust
pub enum Shape {
    Radius(f32),            // a circle round the centre marker
    Path(Vec<[f32; 2]>),    // offsets from the centre marker, in the order dropped
}
pub struct Area { pub shape: Shape, pub below: f32, pub above: f32 }
```

- **The centre marker** is the netgame flag the area belongs to. A
  path's points are offsets from it across the floor, so moving the
  marker moves its area and a path costs no flags.
- **A radius** is Halo 3's hill: a circle round the marker (**C**).
  `Area::default()` is the cylinder a lone flag's zone is: radius 1,
  half a unit under the marker and one and a half over
  (`area::DEFAULT_RADIUS`, `DEFAULT_BELOW`, `DEFAULT_ABOVE`, equal to
  `zones::RADIUS`, `BELOW` and `ABOVE`).
- **A path** is a custom area: the points joined in the order they were
  dropped and closed back to the first. It may be concave and may not
  cross itself.
- **`Area::validate`** refuses what a tool must not store, each with a
  sentence (`AreaError`): a number that is not finite; a radius outside
  0.25 to 64; a height under 0 or over 32, or under 0.25 in all; a path
  of fewer than 3 or more than 32 points (`MAX_POINTS`); a point farther than
  64 from the marker; an edge under 0.1; points in a line; edges that
  cross or touch.
- **`Area::contains(centre, p)`** is the zone test, edges included: the
  heights, then the circle or the polygon (an edge test, then the
  crossings of a ray).
- **`Area::of_corners`** is the area of a retail hill: the convex hull
  of its corners round their mean, from half a unit under the lowest to
  one and a half over the highest, which is the polygon
  `zones::Shape::Polygon` is today. It is read from the map and never
  stored, so `MAX_POINTS` does not bound it.

### In the document

An area is document data with no scenario bytes, as a label and an item
spawner's team are ([forge-map-variants.md](forge-map-variants.md),
"Properties without a retail field"). `EntryOp::Area { target, area }`
sets or clears the area of a netgame flag. Its `area` is `null` to
clear the area, or one of

```json
{"radius": {"radius": "40000000", "below": "3f000000", "above": "3fc00000"}}
{"path": {"points": [["c0000000", "c0000000"], ["40000000", "c0000000"], ["40000000", "40000000"]], "below": "3f000000", "above": "3fc00000"}}
```

The wire format uses `forge_doc::F32` for each distance and offset. The
validator decodes a `game_markers::Area` and reports the sentence from
`Area::validate` on refusal. The target must be a netgame flag, and a
clear requires an active area. An entry op gives the document format 3.
The area stays in the map variant (`Tier::Variant`) and lowers to no
scenario write.

`ForgeDoc::areas` resolves active areas to final flag indices.
`LabelledThing::area` carries a flag's area into the census. An area
follows its flag through appends and swap-removes. Clear the area
before removing its flag. `Session::set_area` writes one edit, which
can be undone through the session's edit log.

A Studio map stores the same op as an edit line on the scenario, beside
its labels ([game-types-without-carriers.md](game-types-without-carriers.md),
"The build step").

**A hill is one flag or its corners.** A hill made in Forge or Studio is
one netgame flag of the hill's type (so its label is `hill_corner` and
its number the hill's) with an area. A retail hill keeps its corner
flags and has no area op. A map does not mix the two for one hill: an
area on a flag whose label and number three or more flags share is
refused by the validator (`a hill with corners takes no area`). The
validator reads explicit labels and the base map's built-in flag types
when the session or the materialiser supplies them.

### In a script

A script reads nothing new: `koth.rules` declares
`label hill_corner: zones numbered` and asks `inside(p, zone)`. The
host's zone builder (`game::rules::world::zones`) makes each zone's
shape in this order:

1. the flag's area, when the census gives it one (`Thing::area`):
   `zones::Shape::Area { at, area }`, tested with `Area::contains`;
2. three or more flags of one label and number: the hull, as
   `Area::of_corners` gives it;
3. any other flag: `Area::default()` round it.

So one test serves all three, a retail map plays as it does, and a hill
shaped in Forge is the hill the script scores. `Shape::middle` is
`Area::middle`, which `spawn_near` and the bots' `bot_hold` measure
from. `rules::fake::FakeGame` takes an area on a zone object
(`set_area`) and moves a player through it (`move_to` answers the
`enter` and `leave` the engine reports), so a test plays a path hill
headless. The bots' own zone read (`bots::objective::zones::first_hill`)
takes a hill's flag area from the host's census
(`bots::objective::set_flag_areas`) and keeps its corners' own test for
a retail hill.

The host has the document's areas where it has the document's labels:
the census of a materialised map variant
([rules-engine.md](rules-engine.md#forge-labels)). A map variant's
areas reach every machine in the variant file, so the test agrees
across a lockstep session.

## Drawing

Both drawers build `Marker`s from their own data, take each one's mesh
and place it. Neither holds a shape of its own.

### Studio

`scenario_edit::marker_view` turns the session's
`geometry::Marker`s, their `Gameplay` (flag type, team, identifier),
the document's labels and areas, and the trigger volumes into placed
`game_markers::Mesh`es (`marker_view::placed`), and
`render::Viewport::set_layer_overlay` takes them as the triangle list and
line list of the layer each belongs to (`SpawnPoints`, `Flags`,
`NetgameItems`, `Volume(kind)`; `marker_view::overlays`). They replace
the glyphs of those blocks and the volume boxes. A selected marker (any
corner of a hill, the selected volume) is white. The
Show popover and `view.layers` hide and show them as they do the layers
now, and X-ray draws them through walls. `view.snapshot` draws what the
window draws.

A placement `marker_view` has no marker for keeps its glyph: a flag of a
type the game does not read, and an item spawner whose model draws (the
model is the hologram). An item spawner's class is its item's tag group:
`weap` and `itmc` a weapon, `vehi` and `vehc` a vehicle, else equipment.
The overlay's blend sets the opacity of the skin (`OVERLAY_ALPHA`) and
of the lines, so the skin is not `FILL_ALPHA` there and `Mesh::faded`
does not reach it. The labels of `gameplay_labels` are unchanged.

A trigger volume's box is given by a corner and three full lengths
(`scenario_edit::volumes`); `marker_view` hands `Solid::Box` the half
lengths and a `Place` at the box's middle on the volume's axes.

### The game

The markers are a world-line source
(`crates/extensions/game/src/world_lines.rs`, `WorldLineSource`), as the
HUD kit's world marks and the superbounce guide are:
`forge_glue::markers_world` turns the session's things
(`Session::things`), its ground labels and the problems of the last
ground build into `Entry`s (`Editor::marker_entries`, rebuilt every
third tick), and its `Source` builds the mesh of each in the capture and
gives the window's world-line pass their lines, under the HUD, each
marker's label text ("Red spawn", "Hill 2") over its anchor. The ground
labels, which were rings painted on the HUD canvas, are markers; another
seat's monitor keeps its ring (`forge_glue::marker`). A hill's marker
takes `Area::of_corners` or the default area until the document holds an
area.

- **Lines first.** The world-line pass draws lines, with no depth test.
  That is the hologram's frame and is enough to read every marker. The
  translucent skin (`Mesh::tris`) is drawn by a marker pass of the
  native renderer (`native_render::markers`, new): alpha blended, depth
  tested and not written, after the world's transparents and before the
  HUD, behind the renderer's own switch `impl.render.native`
  ([switches.md](../runtime/switches.md)). With the native renderer off
  the lines alone draw.
- **Depth.** A marker behind a wall draws its lines at a third of their
  alpha (the X-ray look) and no skin, so an author finds a spawn through
  a wall and still sees which side of it the spawn is on. The line pass
  has no depth, so until the marker pass exists every line draws full.
- **Distance.** A marker fades from 40 world units to nothing at 60, an
  area's wall from 80 to 120, as the window's own per-line fade
  (`LineStyle::fade`); its label shows within 20. The aimed-at and held
  markers are white, never fade and are drawn first. A marker past its
  fade is not drawn and not counted.
- **Other game types.** With the tool showing this game type
  (`forge.show`), a thing of another game type has no marker; with all
  shown it draws at a quarter alpha (`Mesh::faded`).
- **Budget.** At most 256 markers a view (`markers_world::BUDGET`), the
  nearest first; the rest are counted in `render.markers`.

### When a marker shows

| Where | Shows |
|---|---|
| Studio's viewport and `view.snapshot` | every marker of a visible layer |
| Forge, monitor on | every marker of the game type the tool shows (`forge.show`: `this_game_type` or `all_game_types`); the others faded when all are shown |
| Forge, playing in the session (D-pad Up) | none, but for the boundary below |
| A game | none, but for the boundary below |

**The boundary in play.** A script asks for an area's wall to draw in
play, as Halo 3 draws its hill (**C**): `show_area(zone, team)`
([rules-world-api.md](rules-world-api.md), "Zones") turns it on for a
zone in a team's colour, `none` for neutral, and `hide_area(zone)` off.
`koth.rules` shows the hill in play and hides the last one when the hill
moves; `territories.rules` shows each registered territory, neutral until
a player's team takes it; `siege.rules` shows its sites and the
defenders' stand in the defenders' colour and a site it is taken in the
attackers' (at most six sites are shown, so the sites and the stand fit
the eight). The VM keeps at most eight shown areas in `RulesExt::shown`
(a zone object and a team, in the order first shown; a ninth is a script
error, and the round's end clears them). After each tick the runtime
turns each into a `Wall` of the zone's shape (`world/walls.rs`) and
publishes them; the same world-line source draws each wall alone
(`Kind::wall`, `draw_walls`), in the team's colour with no pip and no
label, fading over the area's range, and `render.markers` lists it as a
`wall`. A game draws it with or without a Forge session. A machine that
runs the VM draws it, and so does a film's re-run; a client that does not
run the script gets no wall yet. It is on in the built-in scripts, and a
script that never calls it draws nothing.

## Editing

One document op carries every edit, so Forge, Studio and the agent API
make the same document.

### Forge

- **Placing.** The Objects menu's Goals group offers **Hill**,
  **Territory** and **Siege site**. A places a centre flag with a
  radius of 1 in one undo step. A custom hill uses one centre flag of
  an unused hill type and one area op
  ([forge-editing.md](forge-editing.md), "The object menu").
- **Radius.** Choose **Edit area** in an aimed or held goal's
  Properties (X). The left stick grows or shrinks the radius by the
  movement snap, or by 0.25 with snap off. The wall follows the draft.
  X opens the typed **Radius**, **Below** and **Above** rows. Close the
  rows to return to the draft. A writes one `EntryOp::Area`. B cancels.
- **Path.** Y switches to a path. Fly the monitor and press A to drop
  each point on the floor under the reticle. The closing edge is
  dashed. X removes the last point and holding A on a point drags it.
  Invalid additions and drags leave the last valid points in place
  and show the `AreaError` sentence. Start or a tap of A on the first
  point closes a valid path and writes one area op. B cancels. Y
  makes a circle with the same floor area, or restores the radius
  from before the switch if the path has fewer than three points.
- **Moving the marker** moves the area with it and writes no area op.
- **Undo** is the op's inverse, as for any entry op.

### Studio

- The Scene tab's **AI and objectives** group places the same markers
  with **Place new with the mouse**.
- A selected marker with an area has gizmo handles
  (`crates/apps/studio/src/viewport.rs`): a radius has one handle on
  its rim, dragged in or out; a path has a handle a point (drag to
  move, Delete to remove) and one on the middle of each edge (drag to
  add a point there). The top and the bottom of the wall each have a
  handle for the heights. A drag that `Area::validate` refuses snaps
  back, with the sentence in the status line.
- The inspector shows **Shape** (Radius or Path), **Radius**, **Below**,
  **Above** and the points as a table of typed numbers.
- A retail hill's corners show **Convert to area**: it writes
  `Area::of_corners`' path on a new centre flag and removes the corner
  flags, in one undo step. A custom hill is one centre marker and an
  area. The marker models replace Studio's old glyphs (MK4).
- Areas are held in the open session, keyed by placement, and are not
  yet written to the saved `.map`: `changes()` and `is_dirty` ignore
  them.

## Agent API

Both APIs gain what a later agent needs to see a marker and to check
it, so a lane's report attaches a render and a check, not a screenshot
of a window ([agent API rules](../../AGENTS.md)).

| Method | API | Does |
|---|---|---|
| `markers.list {kind?, layer?}` (built) | Studio | every marker of the open map: `{id, kind, layer, team, position, facing, label, number, area, area_from, members, bounds, lines, triangles}`, `id` the placement as `scenario.*` names it (`netgame_flags[3]`; `volume[2]` for a trigger volume), `area` as the document writes it (floats in hex) or `null`, `area_from` `corners` or `default` while the area is derived and not stored, `members` the placements it stands for, `bounds` the placed mesh's box; with `counts` by kind |
| `markers.render {kind?, team?, area?, view?, size?, out?}` (built) | Studio | draws markers with no map, on the grid at the scale of a 0.7 figure: one kind, or with none the contact sheet of `Kind::samples` in red, blue and neutral (a kind of one fixed colour once). Answers `{path, width, height, view, covered, kinds}`, each of `kinds` with its cell and its centre in pixels. `area` is `{radius, below?, above?}` or `{path, below?, above?}` in plain numbers and is refused with `Area::validate`'s sentence. Needs no disc |
| `markers.area.set {id, area}` (built) | Studio | sets a flag's area (`{radius, below?, above?}` or `{path, below?, above?}`) or clears it (`null`), as one undo step; answers `{id, area}` as stored, or `Area::validate`'s sentence as `REFUSED`, the document untouched |
| `markers.area.convert {id}` (built) | Studio | a retail hill's corners become one centre flag with an area, in one undo step; answers `{id, area, position}` of the kept flag |
| `check.run` | Studio | the `markers` check (built): `markers.area.invalid`, `markers.hill.corners_and_area`, `markers.area.no_floor`, `markers.area.overlap`; a spawn or a marker under the floor or off the map is not checked |
| `view.layers`, `view.snapshot` | Studio | as they are: the layers now draw the markers |
| `forge.things` | game | each netgame flag reports `area` in the document spelling, or `null` |
| `forge.propose` | game | accepts an op with `"edit":{"entry":{"area":{target, area}}}` and reports refusals by name |
| `render.markers` | game | what the last capture drew: `{shown, hidden_by_budget, markers: [{thing, kind, team, anchor, lines, faded}]}`, `anchor` in world units, `kind` filtering the list ([agent-api.md](../runtime/agent-api.md), "`render.markers`"); the walls shown in play are rows of kind `wall` |
| `rules.zones` (built) | game | each zone the host built: `{label, number, shape: "area" \| "corners" \| "default", area, at}`, so a test reads the hill a script scores |

`markers.render` is the content render of this page's shapes, as
`model.render` is a model's
([agent-api.md](../editor/agent-api.md), "`model.*`"): a change to a
shape attaches its sheet.

## Rules and limits

- One shape per kind, from `game_markers`. A drawer does not add lines
  to a marker; a new part of a game type is a new `Kind`.
- A marker is never an object: it has no datum, no collision and no
  guest memory, and a film replays without it.
- A marker draws nothing in a game but a wall a script shows.
- An area is on a netgame flag only. A trigger volume is not a zone: the
  host's census has no trigger volumes
  ([rules-world-api.md](rules-world-api.md), "Zones").
- The retail engines read the corner flags and no area. A hill shaped
  as one flag with an area is a hill for a script; `koth.rules` is King
  of the Hill ([game-types-without-carriers.md](game-types-without-carriers.md)).
- A path is at most 32 points and a view at most 256 markers.

## Slices

Sizes as [rich-editors.md](../editor/rich-editors.md): S under 800
lines with tests, M under 2,500.

| # | Slice | Size | Needs | Accepted when |
|---|---|---|---|---|
| MK1 (built) | **The marker set and the area**: `game_markers`, every `Kind`'s mesh, `Palette`, `Area` with `validate`, `contains`, `outline`, `of_corners` | S | nothing | `every_kind_has_its_lines_faces_and_bounds` gives each kind's lines, triangles and box; a marker is its team's colour; a bow tie, a spike, points in a line and a radius out of range are refused by name; an L holds its inside and not its notch; a hill's corners give the hull the host makes |
| MK2 (built) | **The area in the document**: `EntryOp::Area`, its wire form, the validator's refusals, `ForgeDoc::areas`, `LabelledThing::area`, `Session::set_area`, `forge.propose` and `forge.things` | S | MK1 | an area op round-trips canonical JSON; a crossing path is refused with `AreaError`'s sentence; an area follows its flag through a swap-remove; a document with an area is format 3 and a map variant; a hill with corners refuses an area |
| MK3 (built) | **The zone read**: `zones::Shape::Area`, the three-step order, `FakeGame`'s areas, `bots::objective::zones`, `rules.zones` | S | MK2 | on the fake game `koth.rules` scores a player inside a path hill and not one in its notch; a retail hill's zone holds the same points before and after; a lone flag is the default cylinder |
| MK4 (built) | **Studio draws them**: `scenario_edit::marker_view`, the layers' overlays in place of the glyphs, outlines and volume boxes, `markers.list`, `markers.render` | M | MK1 | on the invented cache each placement block gives its kind and team; a hill's corners give one `Hill`; `markers.render` writes the sheet with every kind of `Kind::samples`; `view.snapshot` of a spawn differs from the background where its mesh projects |
| MK5 (built) | **Studio edits areas**: the gizmo handles, the inspector rows, `markers.area.set`, **Convert to area**, the `markers` check | M | MK2, MK4 | a rim drag writes one area op; an edge handle adds a point; a refused drag leaves the document as it was; converting a hill's corners gives a zone that holds the same points; the check names an overlapping pair |
| MK6 (built) | **Forge draws them**: `forge_glue::markers_world` as a world-line source, the fades and the budget, the label text, `render.markers`; the HUD-canvas rings for ground labels removed | M | MK1 | on an invented session every thing gives its marker's lines with the monitor on and none with it off; a marker of another game type is faded; the 257th marker is counted and not drawn |
| MK7 (built) | **Forge edits areas**: **Edit area**, radius by stick and typed, the path's drop, drag, remove and close, the notices | M | MK2, MK6 | scripted pad input (`forge.input`) that drops four points and closes writes one op with those points; a crossing fifth point is refused with its sentence and the path is unchanged; B leaves the document as it was |
| MK8 (built) | **The boundary in play**: `show_area`, `hide_area`, the eight shown areas in `RulesExt`, the built-ins' calls, the wall from the same source | S | MK3, MK6 | on the fake game `koth.rules` shows the hill in play and one area at a time as the hill moves; the shown areas are equal on a host and a client after a tick; a script that never calls it shows none |
| MK9 | **The skin**: `native_render::markers`, the translucent faces with depth, the X-ray lines | M | MK6 | on the renderer's headless target a hill's wall changes pixels in front of a floor and none behind a nearer wall; with `impl.render.native=off` the lines draw alone |

MK1 is first. MK2 and MK4 start together after it, and MK6 beside
them; MK3, MK5 and MK7 follow their needs; MK8 and MK9 are last.

## Evidence

- **Code** The rows, labels and zones named in "What a map has and what
  draws it": the paths given there.
- **Test** Each kind's lines, triangles and bounds:
  `every_kind_has_its_lines_faces_and_bounds`. Colours:
  `a_marker_is_its_teams_colour_with_solid_lines_and_a_thin_skin`,
  `a_kinds_own_colour_holds_or_gives_way_to_a_team`. The area's
  refusals: `a_radius_area_is_refused_outside_its_limits`,
  `a_path_is_refused_by_its_first_fault`. The zone test:
  `a_concave_path_holds_what_is_in_it_and_not_its_notch`,
  `a_radius_area_holds_a_cylinder`. A retail hill's area:
  `a_hills_corners_make_the_area_the_host_makes_of_them`. All in
  `crates/formats/game-markers/src/tests.rs`.
- **C** Halo 3's Forge shows spawn and goal objects as holograms in the
  team's colour under a chosen game type, and its hill in play is a
  ring of light on the floor; Halo: Reach gives a labelled object a
  boundary that is a cylinder or a box with typed sizes. From the
  games as shipped and their public guides. No asset of either is used.
- **Design** Everything else: the shapes and their sizes, the area as
  document data on one flag, the path, the order a zone's shape is
  chosen in, the fades, the budget, the script calls.

## Not verified

- That the markers read at the sizes given, in Studio and from a
  monitor: a `markers.render` sheet and a Forge session show it.
- The 0.7 figure against a player standing beside it in a game.
- The fade distances and the 256 budget on a map with every flag and
  spawn shown, at 60 fps: `render.stats` beside `render.markers`.
- The retail engines' own hill test and its heights, which
  `zones::RADIUS`, `ABOVE` and `BELOW` stand in for
  ([rules-world-api.md](rules-world-api.md), "Not verified").
- Which netgame flag types the retail engines read as a hill of one
  flag, if any.

## Related

- [forge-editing.md](forge-editing.md): the Objects tool, the object
  menu and Properties.
- [forge-map-variants.md](forge-map-variants.md): the things, entry ops
  and properties without a retail field.
- [game-types-without-carriers.md](game-types-without-carriers.md):
  "Maps for scripted games", the labels.
- [rules-engine.md](rules-engine.md#forge-labels) and
  [rules-world-api.md](rules-world-api.md): labels and zones.
- [hud-kit.md](hud-kit.md): world marks and team colours.
- [agent-api.md](../editor/agent-api.md) and
  [agent-api.md](../runtime/agent-api.md): the methods.
