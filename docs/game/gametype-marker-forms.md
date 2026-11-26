# Game type markers: icons, the edit form and the play form

Status: Partly built. The forms, the parts, the icons and stroke digits, the wall and the areas of the three kinds that gain one exist in `game_markers` (MK10); the box and the sphere as areas exist in `game_markers::area`, `record` and `forge_doc::Area` (MK11); the game's drawer draws the edit form with the monitor on, its plates turned to the eye and scaled, and Forge's **Edit area** edits the four shapes on every kind with an area (MK13). Studio's use of the forms and what shows in a match are design (MK12, MK14 to MK16). It extends the built marker set of [gametype-markers.md](gametype-markers.md) (MK1 to MK9).
Code: `crates/formats/game-markers/src/{form,icon,digits}.rs` (the forms and icons), `crates/formats/game-markers/src/area.rs` (the box and the sphere), `crates/extensions/game/src/forge_glue/markers_world.rs` (the game's drawer), `crates/extensions/game/src/forge_glue/editing/area.rs` (Forge's Edit area). Tests: named per slice under "Slices".
Switch: none.

The marker set of gametype-markers.md is one hologram a kind, drawn
for an author and hidden in a game but for a hill's wall. This page
gives every marker an **icon** on
a plate that reads from across the map, and two **forms**: the edit
form an author sees from the monitor, and the play form players see in
a match (a hill's boundary, a capture plate, a plant zone, a
teleporter's portal). The rule that must hold: **the play form is a
part of the edit form, at the same place and size**, so what an author
lines up in Forge is what a player walks into.

Evidence labels are those of [docs-style.md](../meta/docs-style.md),
plus **C**: the retail games as publicly documented
([notes/forge-prior-art.md](notes/forge-prior-art.md)).

## What it takes after

Halo 3's Forge shows spawn and goal objects as holograms in the team's
colour, only for the game type being edited, and its hill in play is a
ring of light a player walks through; Halo: Reach gives a labelled
object a boundary that is a cylinder or a box with typed sizes, and
tells an initial spawn from a respawn point (**C**). The manner is
theirs: team-coloured holograms, a pad on the floor, an upright post,
a sign with an icon, a boundary with height. Every shape, icon and
number is ours, written in `game_markers`; no mesh, texture or emblem
of any game is used.

## The parts of a marker

`Kind::parts(&self, form: Form) -> Parts` names what a marker is made
of; `Marker::mesh_in(form, &Palette)` and `Marker::icon(&Palette,
number)` give the meshes. `Kind::mesh_in` draws one list of pieces and
leaves the edit-only ones out of the play form, so a play piece is the
edit form's piece itself; `Kind::edit_only` is the rest, what Forge
dims. `Kind::mesh` is the edit form with the plate facing the marker's
front, for a drawer that does not turn plates. Sizes are world units
against the 0.7 figure.

| Part | Is | Form |
|---|---|---|
| **Pad** | a ring and a thin disc on the floor, radius `kind::PAD` (0.3), wider for a vehicle | edit; play where the table says |
| **Body** | the kind's own hologram: the figure, the pole and pennant, the ball, the column (the shapes of gametype-markers.md, "The markers") | edit |
| **Post** | one upright line from the pad to the plate, `form::POST` (1.0) tall, or `form::PLATE_GAP` (0.1) over a taller body, so the plate clears a player standing on the pad | edit; play for a territory and a site |
| **Plate** | a disc of radius `form::PLATE` (0.14) with a rim, holding the icon, turned about the upright to face the eye; its centre is `Kind::plate_at` | edit; play for a territory and a site |
| **Facing** | an arrow on the floor the way a player, a vehicle or a wave faces | edit |
| **Boundary** | the area's wall ("Boundaries") | both |
| **Label** | the text over the anchor ("Red spawn", "Hill 2"), within 20 world units | edit |

**The icon** (`game_markers::Icon`) is a drawing in a unit square made
of lines and filled triangles, as every marker shape is, so it needs no
texture: the native marker pass draws it with the skin
(`native_render::markers::Batch`), and the window's line pass draws its
lines when the native renderer is off. `Icon::mesh(colour)` is its
mesh in the plate's space: the plate in the marker's front plane,
facing +x, the icon's square `form::ICON_HALF` each side of the centre.
`icon::plate` is the plate's disc and rim with the icon. A number on a
plate (a hill's, a territory's, a site's) is seven-segment stroke
digits (`game_markers::digits`) at its bottom right, the icon shrunk to
its top left.

**Size across the map.** The drawer scales the plate and its icon by
`form::icon_scale(distance)`, `max(1, distance / form::ICON_NEAR)` (8
world units), with `Mesh::scaled`, so the icon keeps
its size on screen out to the marker's fade distance; the plate's foot
stays on the post (`markers_world::plate_place`). Nothing else of a
marker scales.

## The kinds

| Kind | Icon | Edit form | Play form |
|---|---|---|---|
| `Spawn(Respawn)` | a figure inside a turning arrow | pad, figure outline, facing arrow, post and plate | none |
| `Spawn(Start)` | a figure standing on a bar | the same, the pad a double ring | none |
| `FlagStand` | a pennant | pad, pole and pennant, post and plate | the pad's ring and a short stub of the pole |
| `FlagReturn(Area)` | a pennant over an arrow down | the capture plate, post and plate | **the capture plate**: the area's ring on the floor, an inner ring at half its radius, and a rim a tenth of a unit high |
| `BombSpawn` | a ball with a fuse | pad and diamond, post and plate | the pad's ring |
| `BombGoal(Area)` | a target | the plant zone, post and plate | **the plant zone**: two target rings and a cross inside the area's ring, with the same low rim |
| `BallSpawn` | a ball | pad and ball of three rings, post and plate | the pad's ring |
| `Hill(Area)` | a crown, with the hill's number | the boundary, a centre pip, post and plate | the boundary |
| `Territory(Area)` | a pennant, with the territory's number | the boundary, post and plate | the boundary, the post and the plate, in the holder's colour |
| `TeleporterSender` | an arrow into a ring | the portal, column and arrow up, post and plate | **the portal**: a ring on the floor and an upright hoop one unit tall, amber |
| `TeleporterReceiver` | an arrow out of a ring | the portal, column, arrow down, facing arrow, post and plate | the portal in cyan, with a short arrow on the floor the way a player leaves |
| `Item(Weapon)`, `Item(Vehicle)`, `Item(Equipment)` | a long gun, a wedge with a cab, a diamond | pad, the item's model or its outline, a facing arrow for a vehicle, post and plate | none: the item is there |
| `Volume(..)` | none | the solid | the boundary of a soft boundary a script shows; none for a kill trigger |
| `Drop` | an arrow down onto a bar | as it is, with post and plate | none |
| `Base` | a house | as it is, with post and plate | none |
| `Cover` | a shield | as it is, with post and plate | none |
| `Site(Area)` | a pennant in a ring, with the site's number | a territory's edit form in violet | a territory's play form |
| `Goal(Area)` | a target with a diamond | the plate of a plant zone in violet, post and plate | the plant zone's play form |
| `Line` | a banner | as it is, with post and plate | none |

- **Three kinds gain an area.** A flag return, a bomb goal and a siege
  goal take `Area` as a hill does; with none stored they draw
  `Area::plate()` (`form::PLATE_RADIUS`, `PLATE_BELOW`, `PLATE_ABOVE`),
  a radius of half a unit, a tenth under and one over. `Kind::area`,
  `area_mut` and `with_area` cover all six kinds with an area.
  The area op is the one a hill uses (gametype-markers.md, "In the
  document"), and a script's zone on that flag reads the same area, so
  for a scripted game the plate drawn is the plate scored: a flag with
  no area of its own and no corners takes its marker's default area
  (`game::rules::world::zones::default_area`).
- **A spawn's use** is `SpawnUse::{Respawn, Start}`. The marker draws
  `Respawn` for every starting location until slice MK16 gives a
  starting location a use.
- **Teams.** A spawn, a flag place, a bomb place and an item spawner
  take their team's colour; a neutral one takes the palette's neutral.
  The team is also said in the label ("Blue flag return"), never by
  colour alone.

## Boundaries

`game_markers::area::Shape` gains two typed shapes beside `Radius` and
`Path`:

```rust
pub enum Shape {
    Radius(f32),                      // an upright cylinder
    Box { half: [f32; 2] },           // a box on the marker's axes
    Sphere(f32),                      // a ball round the marker
    Path(Vec<[f32; 2]>),              // a dropped outline
}
```

A cylinder, a box and a path have the area's `below` and `above` as
their height; a sphere has only its radius, and `below` and `above` are
ignored and written as zero. `Area::validate` holds a box's half
lengths (`AreaError::Half`) and a sphere's radius to the radius's
limits (0.25 to 64). `Area::contains` tests the box in the marker's own
axes and the sphere by distance; `Area::contains_turned` turns the box
to the marker's facing. A box lies on its marker's facing everywhere:
Studio, the game's drawer (`markers_world::place_of`) and a zone's test
(`rules::world::zones`, at the flag's facing). The document's wire form, the record at the end of a
Studio map and `rules.zones` carry the two shapes as they carry a
radius.

**The wall** is one function for both forms, `shapes::wall`, so an
author sees the boundary a player will:

- a full line round the outline at the area's lowest: the edge on the
  floor;
- a translucent wall from there to the area's highest, its alpha
  falling evenly from `form::WALL_LOW` (0.30) to `form::WALL_HIGH`
  (0.04), so the height shows and the top does not hide what is behind
  it;
- a line round the top at a third of full alpha;
- a post at each corner of a box or a path, and at every fourth point
  of a circle.

`shapes::wall_of` draws the same over any outline between two heights;
the rim of a capture plate and a plant zone is it, from the floor to
`form::RIM` over the area's outline.

A sphere draws three rings (level, and two upright at right angles),
and a skin on the half above the marker. The edit form adds what only
an author needs: the centre pip, a path's dashed closing edge, the
handles of gametype-markers.md, "Editing". There is no floor and no
lid.

## When a form shows

| Where | Draws |
|---|---|
| Studio's viewport, `view.snapshot`, `markers.render` | the edit form of every marker of a visible layer; `markers.render {form: "play"}` draws the play form alone |
| Forge, monitor on | the edit form of every thing of the game type the tool shows, the play form's parts inside it at full strength and the edit-only parts at `form::EDIT_ALPHA` (0.6) of it, so the part players will see stands out |
| Forge, playing in the session (D-pad Up) | exactly what a match draws |
| A match | the play form of each thing that is shown in play (below) |
| Theater | what the match drew: the play forms come from the film's re-run and the map variant |

A thing is **shown in play** when one of these holds:

1. a script shows it: `show_area(zone, team)` for a boundary
   (gametype-markers.md, "When a marker shows") and
   `show_marker(thing, team)` for any play form, in the team's
   colour it names, with `hide_marker(thing)` (slice MK15);
2. the document says so: `EntryOp::Show { target, on }`, a fact of a
   netgame flag kept in the map variant as its area is. A flag the
   document added is shown unless the op turns it off; a flag of the
   base map is not shown unless the op turns it on. The game reads it
   from the variant's census (`LabelledThing::show`) on every machine,
   so no cue is sent.

A retail game type on a retail map therefore draws nothing new, and a
teleporter or a capture point an author adds in Forge has a look in
play, which it would otherwise lack: a netgame flag has no body.

## How it is drawn

Markers stay an overlay: lines and a skin from `game_markers`, drawn by
the window's world-line pass and the native marker pass
(gametype-markers.md, "The game"). They are not tags in the kit and not
objects (**Design**).

| | Overlay | Generated tags in the kit |
|---|---|---|
| Tag memory | none | a `mode`, a shader and an object tag a kind, on maps with 0.1 to 1.5 MiB of headroom |
| Object pool | none | one of 2,048 a marker, 256 markers a view |
| Team colour and holder's colour | a vertex colour, each frame | a tag write or an object function a marker |
| A session, a film, another machine | drawn from the document; nothing to replicate | objects to make alike on every machine |
| Studio and `markers.render` | the same meshes | a second drawer |
| Lit and shadowed with the world | no | yes |

The last row is the one thing tags would give, and a hologram is not
lit. So the existing rule holds for both forms: a marker has no datum,
no collision and no guest memory. The drawer changes in three ways
(`forge_glue::markers_world`):

- it asks for a form (`Form::Edit` with the monitor on, `Form::Play`
  otherwise) and, in a match, builds entries from the census's shown
  things and the script's shown markers as it does from the shown
  walls;
- it places each plate's mesh on an upright axis turned to the eye,
  with the distance scale;
- it gives each vertex of a wall its own alpha: `mesh::Tri` carries a
  colour a corner, as `markers::Vertex` does.

## Interfaces

| Method | API | Change |
|---|---|---|
| `markers.render` | Studio | takes `form` (`edit`, the default, or `play`) and `use` for a spawn; the contact sheet has a row a form |
| `markers.list` | Studio | each marker gains `icon`, `show` and, for a spawn, `use` |
| `markers.area.set` | Studio | takes `{box: {half}}` and `{sphere}` beside `radius` and `path` |
| `forge.things`, `forge.propose` | game | a flag reports `show`; the op `"edit":{"entry":{"show":{target, on}}}` is taken |
| `render.markers` | game | each row has `form`, `icon` and `plate_scale` (`agent_proto::MarkerRow`), and `{form}` filters the rows; a play form in a match is a row of its kind |
| `rules.zones` | game | `area` carries the two new shapes |

In Forge, **Edit area** cycles Radius, Box, Sphere and Path on Y; a box
grows on the left stick's two axes, each stepping the box's axis
nearer to it as the monitor looks (forward the half length and right
the half width while looking along the marker's facing, the other way
round across it), and a sphere on the forward one. A flag return, a bomb goal
and a siege goal take **Edit area** as a hill does, from `Area::plate()`
when none is stored ([gametype-markers.md](gametype-markers.md),
"Forge"). Properties
gains **Show in play** (On, Off) on a flag.

## Rules and limits

- One shape per kind and form, from `game_markers`; a drawer adds
  nothing to it.
- Every vertex of a kind's play form is a vertex of its edit form.
- A marker is never an object, in either form.
- A play form says where something is, never who holds it or how long
  is left: the game's own navpoints and HUD do that. The one state it
  carries is the colour a script names.
- At most 16 markers shown by a script, beside the 8 areas; one more is
  a script error, as a ninth area is.
- No label text in play.
- Nothing is copied from a game: the icons are strokes written in
  `icon.rs`.

## Slices

Sizes as [gametype-markers.md](gametype-markers.md), "Slices". Tests
are on invented data.

| # | Slice | Size | Owns | Needs | Accepted when |
|---|---|---|---|---|---|
| MK10 (built) | **Forms and icons**: `Form`, `Parts`, `Icon` and the stroke digits, `SpawnUse`, the three kinds' areas, `shapes::wall`, `Kind::samples` in both forms | M | `crates/formats/game-markers/src/{form,icon,digits}.rs` (new), `kind.rs`, `shapes.rs`, `mesh.rs` (a colour a corner), `lib.rs`, `tests.rs`; the call sites of the three kinds that gain an area | nothing | a table of each kind's lines and triangles in each form; every play vertex is an edit vertex; each icon lies in its unit square and no two icons have equal meshes; the wall's alpha falls from low to high; a kind with no play form gives an empty mesh |
| MK11 | **The box and the sphere**: `Shape::Box`, `Shape::Sphere` through `validate`, `contains`, `outline`, the document's wire form and the map record | S | `crates/formats/game-markers/src/{area,record}.rs`, `area_shape_tests.rs` (new), `crates/formats/forge-doc/src/area.rs` and its test | nothing | a turned box holds its corners and not a point past them; a sphere holds a point over the marker that a cylinder of the same radius and no height does not; each refusal by name; both shapes round-trip canonical JSON and the record |
| MK12 | **Studio**: `form` and `use` in `markers.render`, the fields of `markers.list`, plates turned to the camera, the Shape picker and handles for a box and a sphere | M | `crates/apps/studio/src/api/markers.rs` and its tests, `viewport/area.rs`, `ui/inspector/area.rs`, `crates/tools/scenario-edit/src/{marker_view,area_edit}.rs` | MK10, MK11 | the sheet holds every kind in both forms; a play render of a spawn is empty and of a flag return is its plate; a corner handle drag writes one area op with a box |
| MK13 (built) | **The game's drawer**: forms by monitor state, turned and scaled plates, the edit-only parts dimmed, `render.markers`' fields, **Edit area** on the four shapes | M | `crates/extensions/game/src/forge_glue/markers_world.rs` and its tests, `forge_glue/editing/area.rs` and its tests, `crates/extensions/agent-proto/src/render.rs` | MK10, MK11 | on an invented session each thing gives its edit form with the monitor on; a plate's normal points at the eye and its size doubles from 8 to 16 units away; scripted pad input makes a box of the stick's two lengths in one op |
| MK14 | **Shown in play, from the document**: `EntryOp::Show`, the default by who added the flag, `LabelledThing::show`, the record's flag, the Properties row, the match's entries | M | `crates/formats/forge-doc/src/entry.rs` (the op) and `validate.rs`, `crates/extensions/game/src/map_areas.rs`, the census field, `crates/extensions/forge/src/props.rs`, the play entries in `markers_world.rs` | MK13 | a flag the document added draws its play form in a match and a base flag does not; the op flips each; a match on an invented variant lists the capture plate in `render.markers` with the monitor off; a retail map with no document draws nothing |
| MK15 | **Shown in play, from a script**: `show_marker`, `hide_marker`, `RulesExt::shown_markers`, the markers cue beside the walls cue | S | `crates/formats/rules/src/{show,vm,fake}.rs`, `crates/extensions/game/src/rules/world/walls.rs`, the builtin's row in `docs/game/rules-world-api.md` | MK14 | on the fake game a script shows a flag return in blue and hides it; the seventeenth is a script error; a host and a client hold equal lists after a tick; a client that runs no script draws from the cue |
| MK16 | **A spawn's use**: a starting location's `Respawn` or `Start` as document data, the Properties row, the spawn choice that reads it | S | waits for the owner's answer ("Not verified") | MK10 | a start-only point is not chosen for a respawn on the fake game; the marker draws the use |

MK10 and MK11 start together. MK12 and MK13 follow both and touch no
file in common. MK14 follows MK13, which it shares a file with; MK15
follows MK14.

## Evidence

- **C** Halo 3's holograms by game type and its hill in play; Reach's
  typed cylinder and box boundaries and its initial spawn and respawn
  point: [notes/forge-prior-art.md](notes/forge-prior-art.md).
- **Code** The marker set, the area, the drawers and the walls a
  script shows: the paths in [gametype-markers.md](gametype-markers.md).
- **Design** The parts, the icons, the two forms, the wall's look, the
  two shapes, shown in play and its defaults, the overlay.

## Not verified

- The radius within which the retail engines score a flag capture and
  arm a bomb. Until it is read from the image, a capture plate and a
  plant zone on a retail game type draw `Area::plate()`, which may be
  larger or smaller than the engine's test; a scripted game type scores
  the drawn area.
- Whether the game should tell a round-start spawn from a respawn point
  at all. Halo 2's starting locations make no such difference that this
  repository has read; MK16 is built only if the owner wants it.
- That the icons read at 60 world units with the distance scale, and
  that the dimmed edit-only parts leave the play form clear: a
  `markers.render` sheet in both forms, then a Forge session.
- That a play form does not confuse a retail game type's own markers
  (the flag's navpoint, the hill's): a match of each on a Forge-made
  map.

## Related

- [gametype-markers.md](gametype-markers.md): the set, the areas, the
  drawers and slices MK1 to MK9.
- [forge-blockout.md](forge-blockout.md): the ghost of a pending piece
  uses the same pass.
- [forge-editing.md](forge-editing.md): the object menu and Properties.
- [rules-world-api.md](rules-world-api.md): zones and the show channel.
- [firefight.md](firefight.md) and
  [game-types-without-carriers.md](game-types-without-carriers.md):
  the labels of the Firefight and Siege points.
