# Forge blockout: the natural set, dev textures and monitor

The natural set, the dev textures, the monitor, interfaces, rules, slices and evidence. Part of [forge-blockout.md](forge-blockout.md).

## The natural set

Rocks are generated, seeded and convex. They ship as code, they cost a
document one small integer, and they take the look of the map they are
placed on by naming its own rock or ground shader. Scenery of another
map (a rock from a campaign level) is a `transplant` recipe and belongs
to slice F10 of [forge-design.md](forge-design.md); it appears in the
Scenery group when F10 lands and is not part of this set.

| Shape | Is | Starts at |
|---|---|---|
| `Boulder` | a rounded lump, widest near its base | 96 x 96 x 64 |
| `Slab` | a flat rock with a walkable top | 256 x 192 x 32 |
| `Cliff` | a tall face, craggy in front and flat behind, to stand against a wall or in a row | 256 x 64 x 256 |
| `TerrainWedge` | a ramp of ground with an uneven surface and a walkable slope | 256 x 256 x 96 |
| `Spire` | a tall narrow rock | 64 x 64 x 256 |

`forge_shapes::natural::generate` makes each the same way:

1. A point set of at most 34 points on a form that suits the shape,
   each shape drawing its own seeded numbers from
   `forge_shapes::noise::Stream`:
   - `Boulder`: the 26 directions of a cube's corners, edges and faces,
     each turned a little, at a radius that wanders with low-frequency
     noise and a jitter of its own, drawn in toward the top, the nine
     lowest directions set on the ground plane as a flat footprint.
   - `Slab`: three irregular rings of ten (a base drawn in, a belly and a
     top edge with a small noise in its height) and a crown at the full
     height, so the top is within a few degrees of level.
   - `Cliff`: a front lattice of five columns by four rows pushed out to
     craggy depths by noise, the end columns in the planes x = 0 and
     x = 1 so cliffs stand side by side, and a back in the plane y = 1.
   - `TerrainWedge`: six columns by five rows of a surface rising along
     x with noise on its height, the edge rows in the planes y = 0 and
     y = 1, the toe at ground level and the back in the plane x = 1.
   - `Spire`: four rings of eight narrowing upward, each with ragged
     radii, a ledge of its own and a lean, and an apex at the full
     height.
2. Noise is `forge_shapes::noise`: an integer hash of the lattice cell
   and the seed, blended with a cubic in `f32` (`value`, and `fbm` for
   octaves). The seed of a rock is a hash of its shape and its param.
   Round rings take their points from `forge_shapes::circle`, never
   `sin` or `cos`, so a seed gives the same bits on every host.
3. The points are stretched so each axis spans the unit box, which puts
   the lowest point on z = 0 and the bounds at the size, and are rounded
   to a grid of 4,096 across it. The convex hull of the grid points
   (`forge_shapes::hull`, exact `i128` sign tests: every plane through
   three points with the rest on or behind it, its corners the strict
   two-dimensional hull of the points on it) is scaled to the size. A
   hull of 34 points has at most 64 faces.

The hull is the shell and the one convex piece, so a rock is one
polyhedron for Havok and a faceted, low-polygon rock to the eye, with
normals shared between faces that meet at under 40 degrees. Texture
coordinates are a box projection: each face takes the pair of axes its
normal is most square to. A hull has at most 64 faces. The Natural
group of the object menu lists eight seeds a shape; **Reroll** in
Properties takes the next seed. A cliff with an overhang or a rock
arch is several pieces placed together and saved as a prefab.

## The dev textures

The dev look is ours, drawn by code (`crates/formats/dev-texture`): a
measured grid in the colour of the face's role, and on each rectangular
face a plate that says what the piece is and how big that face is.

```
BLOCK
256 × 128 U
7.80 × 3.90 M
```

**The tiles.** One 512 by 512 tile a family, covering 256 units (two
texels a unit): a checker of 32-unit squares in two tones of the
family's colour, a faint line every 16 units, a stronger one every 64,
and a heavy one on the tile's edge every 256. A line is one unit wide
(two texels, more for the heavier ones) and centred on its coordinate,
darker the heavier it is, so a tile's first and last rows and columns
are equal and it wraps without a seam (`dev_texture::tile`). With origin-aligned coordinates the lines are the block-out grid
itself: a 16-unit step lands on a line.

| Family | Colour | Used for |
|---|---|---|
| Floor | grey | role `Floor` |
| Wall | orange | role `Wall` |
| Ceiling | blue | role `Ceiling` |
| Trim | green | role `Trim` |
| Nodraw | yellow | faces left out of the render model, in the monitor's overlay and Studio's renders |
| Clip | magenta | barriers, in the same two places |

The colours are numbers in `dev_texture::family`; the squared RGB
distance between the mid colours of any two families is over
`MIN_FAMILY_CONTRAST` and between one family's two tones under
`MAX_TONE_SPREAD`, so a role reads at a glance and the grid stays
quiet.

**The label** is geometry, not pixels, so it is exact at any size and
needs one small texture for every piece. The font is a 5 by 7 face of
ours (capitals, digits, the point, the times sign, a blank; the shapes
are a table in `dev_texture::font`, drawn for this repository, and no
font file or licence is involved) in a 128 by 128 atlas of 16-texel
cells. Each cell is a dark plate with the glyph on it at twice its size,
so a row of cells reads as one plate. On each rectangular face
`dev_texture::label::layout(shape, width, height)` gives a plate of 3
rows of 16 cells, centred, upright (up on a wall, along x on a floor):
the shape's name, the face's size in units, and the same in metres to
two places (a unit is 3.048 centimetres, rounded half up). A cell is `h`
units square, `h` the largest of 16, 8, 4 and 2 with the plate inside
the face and a margin of `h / 2` all round, so the plate needs `17h` by
`4h`; a face too small for `h` = 2, or whose text is over 16 cells, has
no plate. Coordinates are the face's own (x along, y up, from its
lower-left corner); the caller maps them. `label::read_label` reads a
plate back from the atlas's texels, which the tests compare with the
text the layout gives. The render mesh cuts the plate out of the face as
a grid of cell quads, each mapped to its glyph's cell in the atlas, so
nothing is coplanar with the face behind it and the cell count of a
face does not change with its size. A face that is not a rectangle (a
ramp's sides, a cylinder's caps, every face of a rock) draws the tiles
and has no plate.

**How it is made.** A CPU raster when the kit is built, not a shader of
the renderer: `dev_texture::tile(family) -> Image` and
`dev_texture::font() -> Image` draw RGBA, and `content_kit::blockout::dev`
writes each as a `bitm` (`content_kit::blockout::texture`: DXT5 with its mips
and opaque alpha through `h1_assets::bitmaps::convert::convert_with`, as
`pogo_kit::texture` writes its atlas) with a
`shad` on a template the map has (`h1convert::shad::Library::build`
over `shad::TEX_BUMP`, our bitmap as the base map). Four tiles (Floor,
Wall, Ceiling, Trim) and the font are tags, at
`forge\blockout\dev\<family>` and `forge\blockout\dev\font`. A tag is
the one form the lifted D3D path, the native renderer, a film and
another player's machine all draw without code of their own, and the
game's texture cache and mips serve it as any texture. The rasters are
a pure function of `dev_texture::VERSION`; a process draws and encodes
them once (`content_kit::blockout::dev::encoded`), and `dev::encoded_in(data)`
keeps the encoded chains under `<data>/cache/forge-blockout/dev-<VERSION>`
for the next. The dev set is added the first time a definition draws a
`Dev` face, and found after that.

**Dev view.** A definition with a material other than `Dev` has two
render models: `look`, with its materials, and `dev`, every role `Dev`
with the plates. The `dev` model depends only on the shape, size and
`param`, so definitions that differ in materials share it, and an
all-`Dev` definition has only it. The setting `forge.blockout.dev_view`
(Off by default; **Dev view** on the Overlays page) writes each such
`hlmt`'s render model reference to the `dev` model, or back, and makes
the definition's objects again (`forge_glue::dev_view`, through
`hot_tags::Editor::batch` and `world.object.respawn`). It is
presentation: it is not in the document or the log, a rebuild applies
it again, and each machine of a session has its own. With the monitor
on, the Block-out overlay also draws barriers and `Nodraw` faces as a
skin in their family's colour.

## In the monitor

- **The object menu** gains two groups (`forge_screens::blockout::with_groups`; the Barrier row is slice BO12's)
  ([forge-editing.md](forge-editing.md), "The object menu"):
  **Structure** (Block, Wall, Floor plate, Ramp, Wedge, Stairs,
  Cylinder, Arch, Barrier) and **Natural** (the five shapes, eight
  seeds each). A row places the shape at its starting size, `Dev`, and
  holds it.
- **Holding a piece** rounds its origin to the block-out grid on the
  world's axes when the piece stands at a quarter turn, and otherwise
  rounds as any object does. The Rotation snap applies as it is.
- **Resize** (Properties; `forge::tool::Draft`, `forge::piece::{pick_face, resize_step}`) is a draft, as **Edit area** is: the reticle
  picks the face it aims at, the left stick's forward and back push or
  pull that face by a grid step, and the opposite face stays where it
  is. The draft draws as the pending ghost does, with the face's size
  in units on the HUD's readout. A writes one `Forge::Piece` (and a
  `Placement::Set` when the origin moved, in the same undo step); B
  cancels. X opens **Size**: Width, Depth and Height rows, each opening
  the keyboard for a number of units, left and right nudging by the
  grid; Steps, Sides or Voussoirs where the shape has one.
- **Material** (Properties) opens a page of All faces and each role, each a picker: All faces, or Floor, Wall,
  Ceiling and Trim, then Dev, the map's surfaces (`Editor::set_surfaces`, from `content_kit::blockout::palette::survey`; none until a survey is read), Nodraw.
- **Reroll** (Properties, a natural piece) takes the next seed.
- **Tool settings** gains **Block-out grid**; the Overlays page gains
  **Block-out** and **Dev view**.
- A refused size or material shows the validator's sentence as the HUD
  notice and leaves the document as it was.

## Interfaces

- **Document**: `Forge::Piece`, `forge_doc::PIECE_TAG`,
  `ForgeDoc::pieces` (each live piece's placement and definition).
- **Materialiser**: `forge_build::blockout::{definitions, install,
  headroom}`, `Built::blockout`;
  `content_kit::blockout::{install, dev, slot, palette}`.
- **Session**: `Session::place_piece`, `set_piece`, `pieces`, `piece`,
  `pending_pieces`, `forge::piece::{drop, bounds, item, bloc_key}`,
  `ThingKind::Piece`; `World::piece_loaded(name)`; BO10 adds
  `World::slot_take` and `slot_write`.
- **Nav.** A piece is a crate placement with a `coll`, which
  `forge_nav::read` already takes as a solid
  ([forge-nav.md](forge-nav.md)). **Generate nav** builds the document's
  play form (`NavBuilder::generate`), whose build installs the tags of
  every definition its pieces use, so a pending piece is a solid of the
  generated map as a live one is, and the builder is given nothing more.
- **Game agent API**: `forge.pieces` lists each piece
  (`{target, def, name, state, position, rotation, bounds}`, and `slot`
  once BO10 gives one), the slots in use and the pending count; its `kind`
  in `forge.things` is `piece`;
  `forge.propose` takes the piece op; `forge.capacity` gains the
  block-out set's `Cost` and what the cut order dropped; `forge.status`
  carries `dev_view` and the grid.
- **Studio agent API**: `blockout.render {name | shape, size, param?,
  view?, pixels?, plates?, fit?, out?}` draws one definition and, with
  none, the contact sheet of every structural piece of the starter set
  and three seeds of each natural shape, labelled by canonical name;
  `blockout.describe {name | shape, size, param?}` answers its name,
  counts, hulls, bounds and each plate's rows as text. Both need no disc
  while every material is `Dev` (`Cost` is the kit's, from a map).
  They are the content render and check of this page's shapes, as
  `markers.render` is the markers'
  ([agent-api.md](../editor/agent-api.md), "Block-out pieces").

## Rules and limits

- A definition is integers and tag keys. It holds no vertex, pixel or
  byte of a disc, and a `Map` material is a reference the user's own
  disc resolves.
- A size is 4 to 2,048 units an axis; stairs have 2 to 32 steps, a
  cylinder 8 to 24 sides, an arch 4 to 12 voussoirs, a rock at most 64
  faces. A shell's `coll` passes `collision_build::verify::check` or
  the definition is refused with the check's sentence.
- A piece is one object of the pool and one fixed body; the budget
  counts it as any object ([forge-editing.md](forge-editing.md)).
- A piece does not move in play. A moving platform is a machine, not
  block-out.
- Roles do not follow a turn: a block laid on its side keeps the role
  each face was generated with.
- Pieces are lit as objects; they cast and take no lightmap shadow
  ([forge-features.md](forge-features.md), "Geometry, textures and
  models").
- The dev textures, the font and every generated shape are drawn by
  this repository's code. No image of another game's dev textures is
  used.

## Slices

Sizes as [forge-design.md](forge-design.md), "Features". Every test is
on invented data. A slice updates this page's status line and, where it
adds a method, the agent API's page.

| # | Slice | Size | Owns | Needs | Proved by |
|---|---|---|---|---|---|
| **BO1** | **Definitions and primitives**: units, `PieceDef`, the canonical name, `Solid`, the generators for block, wall, floor plate, ramp, wedge, stairs, cylinder and arch, roles, origin-aligned coordinates, `STARTER` | M | `crates/formats/forge-shapes/` (new), the workspace member line | nothing | every shell is closed and wound outward (each edge once each way); 10,000 seeded points are inside the shell exactly when inside a hull; a block's, ramp's and wedge's volume equals its formula; two blocks a tile apart share coordinates along their seam; a golden hash of each shape's vertex bits; each refusal by name; the name round-trips |
| **BO2** | **Dev textures**: the six tiles, the font atlas, `label::layout` | M | `crates/formats/dev-texture/` (new), the workspace member line | nothing | `read_label` reads every row of a table of sizes back from the raster and from the layout's cells; a tile's opposite edges are equal; lines fall on 16, 64 and 256; the family contrast table; a golden SHA-256 of each image; a sweep of every size shows a plate inside its face or none |
| **BO3** | **The natural set**: `noise`, `hull`, the five generators | M | `crates/formats/forge-shapes/src/{natural,noise,hull}.rs` and their tests | BO1 | every vertex is on or behind every face's plane; the hull fills its bounds on each axis within a tenth; the lowest face lies on z = 0; a seed gives the same bits (golden) and eight seeds eight different hulls; at most 64 faces; a slab's top and a wedge's slope are `Floor` |
| **BO4** | **A definition as tags**: the render mesh with cut-out plates, `bloc`, `hlmt`, `mode`, `coll`, `phmo`, the dev set, `Cost`, an invented map for tests | L | `crates/tools/content-kit/src/blockout/` (new) but `palette.rs` and `slot.rs` | BO1, BO2 | each `coll` passes `verify::check` and 1,000 random segments agree with brute force on the shell; the `mode` decodes to the mesh within quantisation; the `phmo` decodes to the hulls' corners; the glyph cells of each plate read back as its text; a `Nodraw` role is in the `coll` and not the `mode`; a barrier has no `mode`; two installs are byte-equal; `Cost` equals the bytes added |
| **BO5** | **The document**: `Forge::Piece`, `PIECE_TAG`, the validator's rules, fold, inverse, `ForgeDoc::pieces` | S | `crates/formats/forge-doc/src/piece.rs` (new), `links.rs` (the variant), `validate.rs`, `document.rs` (`FORMAT_PIECES`, with its number in `content::forge`), `lib.rs`, `src/tests/piece.rs` | BO1 | golden canonical JSON and identity; each refusal has a refusing case; fold then inverses gives the start; the seeded fuzz covers the op; a document without a piece keeps its format |
| **BO6** | **Materialising**: definitions collected and installed once, the crate palette, the starter set and the cut order in the kit, the palette survey | M | `crates/tools/forge-build/src/blockout.rs` (new), `lib.rs` (the dispatch), `kit.rs` (the palette rows), `tests/blockout.rs`; `crates/tools/content-kit/src/blockout/palette.rs` | BO4, BO5 | two builds are byte-equal; three pieces of two definitions add two tag sets and resolve to their palette rows; a `Map` shader the base lacks is refused by name; with headroom set low the cut order is the page's and the refusal names the cost; the survey on an invented structure ranks by area and gives the repeat length |
| **BO7** | **The session**: placing, changing and dropping pieces, the three states, pending, `forge.pieces`, pieces as nav solids | M | `crates/extensions/forge/src/{piece,piece_tests}.rs` (new), `session/pieces.rs` (new), `things.rs` (the kind), `crates/extensions/agent/src/forge/pieces.rs` (new), `crates/extensions/agent-proto/src/forge.rs` | BO5; BO6 for names only | over a fake game: placing a loaded definition is two ops, one undo step and one `object_new` on its `bloc`; a definition not loaded makes no engine call and one pending piece; after `adopt_copy` nothing is pending; a drop deletes and makes the object; undo restores the definition |
| **BO8** | **The monitor**: the Structure and Natural groups, Resize and its draft, Size, Material, Reroll, the Block-out grid, the ghost and the Block-out overlay | M | `crates/extensions/game/src/forge_screens/blockout.rs` (new), the groups in `forge_screens`' sheet, `crates/extensions/forge/src/tool.rs` (the grid and the draft) with `tool_tests.rs`, `crates/extensions/game/src/forge_glue/{editing/resize,pieces_world}.rs` (new), one call in `forge_glue/markers_world.rs`, the two settings' definitions | BO7 | `Page` tables for each screen; a held piece's origin rounds to each grid; a push by one step is one `Forge::Piece`; a pull past the minimum is refused and the document unchanged; a pending piece and a barrier each give their batch |
| **BO9** | **Dev view**: the second render model's write and its undo, on every loaded definition, again after a rebuild | S | `crates/extensions/game/src/forge_glue/dev_view.rs` (new) and its tests | BO6, BO8 | on synthetic guest memory: On writes one reference a textured definition and asks for its respawn, Off restores the bytes; an all-`Dev` definition is untouched; the document and the log are unchanged |
| **BO10** | **Slots**: the stand-in tags, `slot::plan`, taking and freeing, the live writes | L | `crates/tools/content-kit/src/blockout/slot.rs` (new), `crates/extensions/agent/src/forge/slots.rs` (new), `crates/extensions/forge/src/session/slots.rs` (new), the reservation in `forge-build/src/kit.rs` | BO6, BO7; realtime-edits R4, R7, R12 | for a sweep of sizes a class's counts are equal; a plan applied to the stand-in's bytes on synthetic memory decodes to the tags BO4 makes for that definition; the seventeenth box is pending; a rebuild frees every slot; two sessions fed one log hold the same slots |
| **BO11** | **Studio**: `blockout.render`, `blockout.describe`, the contact sheet | S | `crates/apps/studio/src/api/blockout.rs` (new), its tests, the method table, `docs/editor/agent-api.md` | BO4 | `every_method_in_methods_has_a_test`; the sheet holds every shape in every family; a render of a block differs from the background where its plate projects; `describe` gives the labels BO2's layout gives |
| **BO12** | **Barriers**: the Barrier row, the Volumes tool's listing, what a barrier blocks, its enable input | S | `crates/extensions/forge/src/barrier.rs` (new) and its tests; the Volumes rows in `forge_screens/blockout.rs` | BO8; F7 for the input | a barrier is a piece with `Clip` on every role and no `mode`; its enable input removes and makes its object over a fake game |

BO1 and BO2 start together. BO3 and BO5 follow BO1; BO4 follows BO1
and BO2; BO11 follows BO4. BO6, BO7, BO8 and BO9 run in that order.
BO10 is last and is the only slice whose worth depends on a live check
that has not run.

## Decisions

- **Pieces are crates with a fixed body**, not scenery and not level
  collision. Merging into the level's collision
  (`collision_build::merge`) makes a solid a player stands on too, but
  a merged solid cannot be moved or removed without a rebuild of the
  level's collision, which a retail map with pathfinding refuses
  (realtime-edits.md, "Collision").
- **Exact sizes, not a ladder.** The kit holds a starter set and
  slots, not a ladder of fixed sizes to swap between; a size outside
  the starter set is a slot or a rebuild. Whether a placement's scale reaches collision is
  not known, and one scale cannot stretch one axis.
- **The dev label is geometry over a font atlas.** A texture baked per
  size would add a `bitm` a definition and need a rebuild for every
  size; a shader of the native renderer would not draw on the D3D path.
- **Natural pieces are generated and convex.** A transplant needs the
  other map on the user's disc and tag memory for its closure; it is
  F10's and offered beside this set.

## Evidence

- **E** Which objects get rigid bodies, the motion word and the fixed
  default: [collision.md](../formats/collision.md), "BSP physics" and
  the rigid body table.
- **V** The headroom of multiplayer maps:
  [scenario-editor.md](../editor/scenario-editor.md) section 6.
- **Code** The writers named in "A definition as tags"; the live paths
  named in "Slots".
- **Design** The unit, the grid, the shapes and their ranges, the
  roles, the families and their colours, the label, the cut order.

## Not verified

- That a crate whose `phmo` body has motion 5 stops a biped, a vehicle
  and a grenade, on which collision layer, and that it is still there
  after the object is made again. BO4's first game run places one
  block; if it fails, the piece becomes a machine, whose body the
  engine keyframes ([collision.md](../formats/collision.md)).
- That an `hlmt` naming no render model (a barrier) makes an object
  that collides and is not drawn.
- That a plate reads upright and unmirrored in the game: the cells map
  `v` down, as an image's rows are, and the frame quads meet the cells
  with T-junctions, which may show as specks on the D3D path.
- That a structure's shader draws correctly on an object, where no
  lightmap reaches it. If it does not, a `Map` material becomes a
  generated `shad` on `shad::TEX_BUMP` binding the surveyed shader's
  bitmaps (`native_render::load::shader_bitmaps`).
- That an object follows a write of its `hlmt`'s render model after it
  is made again, on both render paths.
- The tag memory a definition takes on each map, and how many of the
  starter set and the slots fit: `forge.capacity`, recorded in
  [notes/forge-probes.md](notes/forge-probes.md).
- The slot writes in a running game: realtime-edits.md lists the live
  checks of R4, R7 and R12 as not run.
- That the tiles and plates read from a monitor's usual distances and
  at the game's mip bias: `blockout.render`, then a session.
- What a barrier blocks: the collision groups of the `phmo` and `coll`
  material flags ([forge-features.md](forge-features.md)).

## Related

- [forge-design.md](forge-design.md): the document, the kit, the
  classes, Rebuild map, slice F8.
- [forge-editing.md](forge-editing.md): the object menu, Properties,
  Precision, Overlays.
- [forge-features.md](forge-features.md): barriers; faces and vertices
  in game (F13, F16).
- [gametype-markers.md](gametype-markers.md) and
  [gametype-marker-forms.md](gametype-marker-forms.md): the marker pass
  the ghost uses.
- [modeling.md](../editor/modeling.md): the collision compiler.
- [realtime-edits.md](../editor/realtime-edits.md): the live writes.
- [content-stage.md](../editor/content-stage.md): the stage's
  code-drawn grid, the precedent for `dev-texture`.
