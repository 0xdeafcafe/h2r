# Game type markers

Status: Partly built. The marker set and the area model exist (MK1), Studio draws the markers and answers `markers.list` and `markers.render` (MK4), Forge draws them as world lines and answers `render.markers` (MK6), the Forge document stores areas, which `forge.things` and `forge.propose` carry (MK2), a script's zone reads them (MK3), Studio edits them (MK5), Forge places and edits them in the monitor (MK7) and a script shows an area's wall in play (MK8) and the native renderer draws the skin and the X-ray lines of Forge's markers and the walls in play (MK9), and each marker has an edit and a play form, an icon on a plate and the wall's look, and a flag return, a bomb goal and a siege goal have an area ([gametype-marker-forms.md](gametype-marker-forms.md), MK10); Studio's saved map and the wall on a client that does not run the script are design.
Code: `crates/formats/game-markers/src/record.rs` (the saved areas), `crates/extensions/game/src/map_areas.rs` (the game reads them), `crates/formats/rules/src/show.rs` (`Cue::Walls`), `crates/formats/game-markers/src/lib.rs` (`Marker::mesh`), `crates/formats/game-markers/src/area.rs` (`Area`), `crates/tools/scenario-edit/src/marker_view.rs` (`placed`, `overlays`), `crates/apps/studio/src/api/markers.rs`, `crates/apps/studio/src/api/markers_check.rs` (the `markers` check), `crates/tools/scenario-edit/src/area_edit.rs` (handles, drag), `crates/tools/scenario-edit/src/session/area.rs` (`set_area`, `convert_hill`), `crates/apps/studio/src/viewport/area.rs`, `crates/apps/studio/src/ui/inspector/area.rs`, `crates/extensions/game/src/forge_glue/markers_world.rs` (Forge's source, `render.markers`, `batch`), `crates/rewrites/native-render/src/markers.rs` (`Batch`, `Pass`), `crates/rewrites/native-render/src/frame_graph/markers.rs` (the stage), `crates/extensions/game/src/forge_glue/editing/area.rs` (Forge's draft area), `crates/formats/forge-doc/src/{area,entry,links}.rs`, `crates/extensions/forge/src/session.rs`, `crates/extensions/agent/src/forge/local.rs`, `crates/extensions/game/src/rules/world/zones.rs` (`Zones::new`), `crates/formats/rules/src/fake.rs` (`FakeGame::set_area`, `move_to`, `shown`), `crates/formats/rules/src/vm.rs` (`RulesExt::shown`), `crates/extensions/game/src/rules/world/walls.rs`, `crates/extensions/game/src/forge_glue/markers_world.rs` (`Wall`, `draw_walls`), `crates/live/bots/src/objective/zones.rs` (`first_hill`), `crates/extensions/agent-proto/src/rules.rs` (`rules.zones`). Tests: `crates/formats/game-markers/src/tests.rs`, `marker_view.rs`, `crates/apps/studio/src/api/tests/markers_tests.rs`, `crates/tools/scenario-edit/src/area_tests.rs`, `area_edit.rs`, `crates/apps/studio/src/viewport/area.rs`, `crates/extensions/game/src/forge_glue/markers_world/tests.rs`, `crates/rewrites/native-render/src/markers.rs` (tests), `crates/rewrites/native-render/tests/markers_gpu.rs`, `crates/extensions/game/src/forge_glue/editing/tests/area.rs`, `crates/formats/forge-doc/src/tests/area.rs`, `crates/extensions/forge/src/base_tests.rs`, `crates/extensions/agent/src/forge/tests.rs`, `zones.rs`' `a_flags_area_is_its_zone_before_corners_and_the_default`, `crates/formats/rules/tests/area_zones.rs`, `crates/formats/rules/tests/modes.rs` (`siege_shows_each_site_in_its_holders_colour`), `crates/extensions/game/src/rules/runtime/tests/world.rs`, `crates/extensions/game/src/rules/world/walls.rs`, `crates/live/bots/src/objective/zones/tests.rs`, `crates/extensions/agent/src/rules/tests.rs`.
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
flags of one label and number as the convex hull of their places, else
the default area of the flag's marker: the capture plate for a flag
return, a bomb goal or a siege goal, a cylinder of one world unit for
any other ([rules-world-api.md](rules-world-api.md), "Zones"). `rules.zones` reports each zone and the step that made it.

## The markers

`game_markers::Kind` is the set. `Marker { kind, team }` is one marker
and `Marker::mesh(&Palette)` its `Mesh`: `Line`s at full alpha and
`Tri`s at `FILL_ALPHA` (a colour a corner, so a wall's alpha falls up
it), in the marker's own space, x the way it faces, y left, z up, world
units, the origin at the placement. Each marker has an edit form and a
play form, `Marker::mesh_in(Form, &Palette)`, and a plate with an icon,
`Marker::icon(&Palette, number)`; `Marker::mesh` is the edit form with
its plate facing the marker's front
([gametype-marker-forms.md](gametype-marker-forms.md)).
`Mesh::placed(&Place)` puts it in the world. A player stands 0.7 tall
(**S**: our number for the figure, set by eye against the game), and
every shape is sized against that. A part that lies on the floor is
`LIFT` above it.

| Kind | For | Shape | Parameters | Colour |
|---|---|---|---|---|
| `Spawn(SpawnUse)` | a player starting location | a standing figure's outline on a pad (a double ring for `Start`), and an arrow on the floor the way the player faces | the use | team |
| `FlagStand` | `ctf_flag_spawn` | a pad, a pole and a pennant | none | team |
| `FlagReturn(Area)` | `ctf_flag_return` | the capture plate: the area's ring, an inner ring at half its radius and a rim a tenth of a unit high | the area, `Area::plate()` unshaped | team |
| `BombSpawn` | `assault_bomb_spawn` | a pad with a diamond over it | none | team |
| `BombGoal(Area)` | `assault_bomb_return` | the plant zone: the area's ring and rim, two target rings and a cross | the area, `Area::plate()` unshaped | team |
| `BallSpawn` | `oddball_spawn` | a pad with a ball of three rings over it | none | team |
| `Hill(Area)` | a hill | the area's wall, and a pip at the centre marker | the area | team |
| `Territory(Area)` | `territory` | the area's wall, and the post and plate | the area | team |
| `TeleporterSender` | `teleporter_sender` | the portal (a ring on the floor and an upright hoop), a column and an arrow up | none | amber |
| `TeleporterReceiver` | `teleporter_receiver` | the portal, a column, an arrow down and an arrow out the way a player leaves | none | cyan |
| `Item(Weapon)`, `Item(Vehicle)`, `Item(Equipment)` | a netgame equipment row | a pad and the outline of what it makes: a long gun, a wedge with a cab (on a wider pad, with a facing arrow), a diamond | the class | team, else the class's own |
| `Volume(Volume, Solid)` | a trigger volume, a kill trigger, a soft boundary, a BSP switch | the solid, translucent: `Box { half }`, `Cylinder { radius, height }` or `Sphere { radius }` | the solid | by what it does: yellow, red, orange, blue |
| `Drop` | `ff_drop` | a pad, an arrow down and an arrow the way the wave faces | none | green |
| `Base` | `ff_base` | a ring and a house | none | team |
| `Cover` | `ff_cover` | a shield and an arrow the way it faces | none | yellow |
| `Site(Area)` | `siege_site` | a territory's shape | the area | violet |
| `Goal(Area)` | `siege_goal` | a plant zone | the area, `Area::plate()` unshaped | violet |
| `Line` | `siege_line` | a banner across the way a side faces, and an arrow | none | team, else violet |

- **Every kind but a volume** has a post from the floor to a plate over
  its body, and an icon on the plate (`Kind::icon`, `Kind::plate_at`).
- **The wall of an area** is `shapes::wall` over `Area::outline`: the
  outline at the area's lowest, a fainter one at its highest, a post at
  each corner (every fourth point of a circle) and a translucent wall
  between whose alpha falls from foot to top, with no floor and no lid,
  as Halo 3's hill is a ring of light a player walks through (**C**);
  gametype-marker-forms.md, "Boundaries".
- **An item spawner** in a drawer that has the item's model draws that
  model over the pad as the hologram and leaves the outline out. The
  outline is for a drawer with no model: a render with no map, a
  spawner whose tag did not load.
- **`Kind::of_label`** names the marker of a label's text, the flag
  types' built-in labels and the ground labels. A `hill_corner` is not
  one marker: a hill's only flag is a `Hill` with its area, and three or
  more corners together are one `Hill` with the area `Area::of_corners`
  gives, drawn at the centre it gives.
- **Counts.** Each shape's lines and triangles in each form are fixed
  and are in the comments of `Kind::build`; an area's wall is two lines
  and two triangles a point of its outline and a line a post, 72 lines
  and 64 triangles for a radius of 32 points. The largest edit form is
  a plant zone on a radius: 139 lines and 64 triangles, and the plate
  adds 16 lines and 16 triangles and its icon's.
- **Original shapes.** Every number is written in
  `crates/formats/game-markers/src/kind.rs`,
  `crates/formats/game-markers/src/shapes.rs`, `icon.rs` and
  `digits.rs`. Nothing is read from a
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

`game_markers::Area` is what a hill, a territory, a site, a flag
return, a bomb goal or a siege goal covers:

```rust
pub enum Shape {
    Radius(f32),            // a circle round the centre marker
    Box { half: [f32; 2] }, // a box on the marker's own axes
    Sphere(f32),            // a ball round the marker
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
- **A box** is the marker's own axes, x the way it faces and y its
  left, by two half lengths, with the area's `below` and `above` as its
  height. **A sphere** is a ball round the marker: a radius and no
  height, so its `below` and `above` are ignored and written as zero
  (`Area::sphere`, `Area::boxed`).
- **A path** is a custom area: the points joined in the order they were
  dropped and closed back to the first. It may be concave and may not
  cross itself.
- **`Area::validate`** refuses what a tool must not store, each with a
  sentence (`AreaError`): a number that is not finite; a radius outside
  0.25 to 64 (a sphere's too); a box's half length outside 0.25 to 64
  (`AreaError::Half`); a height under 0 or over 32, or under 0.25 in all; a path
  of fewer than 3 or more than 32 points (`MAX_POINTS`); a point farther than
  64 from the marker; an edge under 0.1; points in a line; edges that
  cross or touch.
- **`Area::contains(centre, p)`** is the zone test, edges included: the
  heights, then the circle, the box or the polygon (an edge test, then
  the crossings of a ray); a sphere is tested by distance from the
  marker, in three axes, with no height test. `Area::contains_turned`
  takes the marker's facing in radians, which turns a box and no other
  shape; a box holds its edge to within `area::EDGE_SLACK` (0.0001) so a
  corner survives the turn. A zone (`rules::world::zones`) tests with
  `contains_turned` at its flag's facing, and the drawers place a box on
  that facing (`markers_world::place_of`), so a box scores where it is
  drawn. `Area::outline` of a box is its four
  corners and of a sphere its equator, in the marker's own axes.
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
{"box": {"half": ["40400000", "3f800000"], "below": "3f000000", "above": "3fc00000"}}
{"sphere": {"radius": "40000000"}}
```

The wire format uses `forge_doc::F32` for each distance and offset. The
validator decodes a `game_markers::Area` and reports the sentence from
`Area::validate` on refusal. The target must be a netgame flag, and a
clear requires an active area. An entry op gives the document format 3.
The area stays in the map variant (`Tier::Variant`) and lowers to no
scenario write.

`EntryOp::Show { target, on }` shows a netgame flag's play form in a
match or hides it
([gametype-marker-forms.md](gametype-marker-forms.md), "When a form
shows"); it is document data with no scenario bytes as an area is, and
`ForgeDoc::shown_flags` resolves it with the default of who added the
flag to final flag indices.

`ForgeDoc::areas` resolves active areas to final flag indices.
`LabelledThing::area` carries a flag's area into the census. An area
follows its flag through appends and swap-removes. Clear the area
before removing its flag. `Session::set_area` writes one edit, which
can be undone through the session's edit log.

A Studio map keeps its areas in the map file itself ("Areas saved with a
map"). A Studio map stores the same op as an edit line on the scenario, beside
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

### Areas saved with a map

A netgame flag has no spare bytes for an area, so a map Studio saves
carries them as a record at the end of its file
(`game_markers::record`, `crates/formats/game-markers/src/record.rs`):

```text
record  = "H2AR" version:u8 count:u16le row*count crc:u32le
row     = block:u8 index:u32le type:i16le identifier:i16le
          at:[u32le;3] shape:u8
          (below:u32le above:u32le
           (radius:u32le | points:u8 [u32le;2]*points | half:[u32le;2] | radius:u32le))?
trailer = length:u32le "H2AE"           ; the file's last 8 bytes
```

- **Shapes.** The low bits of `shape` are 0 for a radius, 1 a path, 2 a
  box and 3 a sphere; a sphere's `below` and `above` are written as
  zero. 4 is a row with no area: nothing follows it, and it only says
  the flag is shown. The bit `0x80` (`record::SHOWN`, `Row::show`) says
  the flag's play form shows in a match.
- **From a variant.** The materialiser (`forge_build::build`) writes the
  same record at the end of the map it builds: a row for each flag with
  an area, and for each flag the document shows whose marker has a play
  form (`crates/tools/forge-build/src/areas.rs`), so the game that loads
  the variant reads them as it reads a Studio map's, and a retail map and
  a document that shows and shapes nothing carry no record.
- **Same area as the document's.** The distances are the exact `f32`
  bits `forge_doc::Area` spells in hex, and every row passes
  `Area::validate` on read, so a record and a document name one area one
  way and `forge_doc::Area::try_from` takes either. A row is keyed by
  its block (netgame flags), index and identifier, and fingerprinted by
  the flag's type and position bits.
- **Finding the flag again.** `record::place` gives a row the flag at its
  own index when that flag still has its type and position, else the
  first free flag with its type, identifier and position. A flag a tool
  moved or retyped since the save has no area (the open map's status line
  counts them), and a flag takes one row.
- **Studio.** `MapSource::open` reads the map without the record, so the
  editor patches the map the engine reads, and `MapSource::areas` reads
  the record. `Scenario::load_areas` gives each flag its area as no edit.
  `write_patched` appends the record after the map when any flag has an
  area; a map with none is the source's bytes. `Changes::areas` holds the
  record when the areas differ from the ones the map was opened with, so
  `is_dirty` and undo see them.
- **The game.** At a map's activation (`EditorSources::activated`) the
  game reads the record through the VFS (`game::map_areas::activate`).
  The census (`rules::census::things`) attaches each row to its flag
  (`LabelledThing::area`), so the flag's zone takes it (step 1 of "In a
  script") and the bots read the hill from it, and a row that is shown
  shows the flag (`LabelledThing::show`). A flag with an area of its own
  (a Forge document's) keeps it.

### In a script

A script reads nothing new: `koth.rules` declares
`label hill_corner: zones numbered` and asks `inside(p, zone)`. The
host's zone builder (`game::rules::world::zones`) makes each zone's
shape in this order:

1. the flag's area, when the census gives it one (`Thing::area`):
   `zones::Shape::Area { at, area }`, tested with `Area::contains`;
2. three or more flags of one label and number: the hull, as
   `Area::of_corners` gives it;
3. any other flag: the default area of its label's marker round it
   (`zones::default_area`): `Area::plate()` for a flag return, a bomb
   goal and a siege goal, the capture plate their marker draws
   ([gametype-marker-forms.md](gametype-marker-forms.md), "The kinds"),
   else `Area::default()`.

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

## Pages

| Page | What |
|---|---|
| [Gametype markers: drawing and editing](gametype-markers-drawing.md) | Drawing, editing, the agent API, rules, slices and evidence. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Drawing

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#drawing).

## Editing

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#editing).

## Agent API

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#agent-api).

## Rules and limits

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#rules-and-limits).

## Slices

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#slices).

## Evidence

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#evidence).

## Not verified

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#not-verified).

## Related

See [Gametype markers: drawing and editing](gametype-markers-drawing.md#related).
