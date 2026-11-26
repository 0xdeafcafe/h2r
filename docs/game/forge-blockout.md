# Forge block-out: pieces, the natural set and the dev textures

Status: Partly built. Slices BO1 and BO3 are built: `forge-shapes` has the definition, the canonical name, the structural generators (block, wall, floor plate, ramp, wedge, stairs, cylinder, arch), `STARTER`, and the natural set (`noise`, `hull` and the five rock generators). Slice BO2 is built: `crates/formats/dev-texture` has the rasters, the font and the label layout. Slice BO5 is built: `forge-doc` has `Forge::Piece`, `PIECE_TAG`, the validator's rules and `ForgeDoc::pieces` (document format 6). The rest of the page is a design. It is slice F8 of [forge-design.md](forge-design.md), split into slices BO1 to BO12 below.
Code: `crates/formats/forge-shapes` (the definitions and generators), `crates/formats/forge-doc/src/piece.rs` (the document), `crates/formats/dev-texture` (the dev textures), `crates/tools/blockout-kit` (a definition as tags), `crates/tools/forge-build/src/blockout.rs` (materialising), `crates/extensions/forge/src/piece.rs` (the session). Tests: named per slice under "Slices".
Switch: none. Two settings of the profile's store, `forge.blockout.grid` and `forge.blockout.dev_view` ("In the monitor").

A monitor builds a level out of **pieces**: blocks, ramps, stairs,
cylinders, arches and rocks, each with a size typed or dragged on a
grid, working collision and a material. A piece is an object of a tag
our code generates from a short **definition** in the Forge document;
no mesh, texture or tag of it comes from a disc or from this
repository's files. The rule that must hold: **the same definition
gives the same tag bytes on every machine**, so a document with a
thousand pieces is a few kilobytes and two machines that materialise it
collide with the same shapes.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## Units and the grid

A block-out **unit** is one hundredth of a world unit, 3.048 cm
(**Design**; a world unit is 3.048 m, `forge_hud::METRES_PER_UNIT`). A
standing player is 70 units tall (`game_markers`' 0.7 figure), so sizes
read as a level designer expects: a doorway is 128 by 96, a step 16.

- A size is three whole numbers of units, 4 to 2,048 on each axis
  (`forge_shapes::units::{MIN, MAX}`), stored as integers. A length in
  world units is `units as f32 / 100.0`, one IEEE division, the same
  bits on arm64 and x86-64.
- The **block-out grid** is a power of two: 1, 2, 4, 8, 16, 32 or 64
  units, 16 by default. It rounds a held piece's position and a size
  being changed. It is its own setting because the Movement snap of
  [forge-editing.md](forge-editing.md), "Precision", is decimal world
  units, which no power of two divides.
- A piece's origin is the lowest corner of its bounds, so a piece whose
  size and position are multiples of the grid has every corner on the
  grid, at any quarter turn.

## A piece

`forge_shapes::PieceDef` is the definition:

```rust
pub struct PieceDef {
    pub shape: Shape,          // the table below
    pub size: [u16; 3],        // x, y, z in units
    pub param: u16,            // steps, sides or seed, by shape; else 0
    pub materials: Materials,  // one for all, or one per role
}
pub enum Material { Dev, Map(TagKey), Nodraw, Clip }
```

| Shape | Is | `param` | Convex pieces |
|---|---|---|---|
| `Block`, `Wall`, `FloorPlate` | a box; the three differ in the size the menu starts them at and the name the dev label prints | none | 1 |
| `Ramp` | a box cut by the plane from its lower front edge to its upper back edge | none | 1 |
| `Wedge` | a ramp turned on its side: a right triangle in plan, full height | none | 1 |
| `Stairs` | `param` equal steps (2 to 32) rising along x | steps | one a step |
| `Cylinder` | an upright prism of `param` sides (8, 12, 16 or 24) fitted to the bounds | sides | 1 |
| `Arch` | a body with a round head and a round-headed opening through y, half the width: two piers and a ring of `param` voussoirs (4 to 12), the outer arc half the width in radius; it needs a height of at least half the width plus one unit, and with an odd `param` the top sits a little under the bounds | voussoirs | 2 + `param` |
| `Boulder`, `Slab`, `Cliff`, `TerrainWedge`, `Spire` | the natural set (below) | seed | 1 |

`Shape::generate(&PieceDef) -> Result<Solid>` gives the geometry
everything else is made from:

```rust
pub struct Solid {
    pub shell: Vec<Face>,   // the closed outer surface
    pub hulls: Vec<Hull>,   // convex pieces whose union is the shell's inside
}
pub struct Face { pub ring: Vec<[f32; 3]>, pub uv: Vec<[f32; 2]>, pub role: Role }
pub enum Role { Floor, Wall, Ceiling, Trim }
```

- **The shell** is a closed manifold of convex planar faces wound
  counter-clockwise seen from outside, the form
  `collision_build::Polygon` takes. Two convex pieces that touch share
  no face in the shell, so the collision compiler never sees coplanar
  overlapping surfaces, which it refuses
  ([modeling.md](../editor/modeling.md), "The collision compiler").
- **The hulls** are what Havok collides with: one polyhedron each.
- **A role** is given by the generator in the piece's own space, so a
  piece keeps its roles when it is turned: a face whose normal is
  within 45 degrees of up is `Floor` (a ramp's slope and a stair's
  tread), of down `Ceiling`, any other `Wall`; a generator marks `Trim`
  itself (a stair's risers, an arch's soffit); a cylinder's caps are `Floor` and `Ceiling` and its sides `Wall`.
- **Texture coordinates** are aligned to the piece's origin: a face's
  u and v are the position along its two axes in units, divided by the
  material's tile size. Two pieces whose origins are a whole number of
  tiles apart continue one another's texture across the seam.
- **Arithmetic** is `+`, `-`, `*`, `/` and `sqrt` on `f32` and integer
  work, which IEEE 754 fixes bit for bit. A circle's points come from a
  table of rational points in `forge_shapes::circle` (integers over
  65,536, symmetric about both axes, exact at the quarter turns), never
  from `sin` or `cos` of the host's library.

### In the document

A piece is a placement in the scenario's crates block with one more
fact, its definition, as an area is a fact of a netgame flag
([gametype-markers.md](gametype-markers.md), "In the document").

- **Placing** is two ops in one undo step: a `Placement::Add` on
  `BlockKind::Crates` whose tag is the placeholder key
  `forge_doc::PIECE_TAG`, and `Forge::Piece { target, def }`, which
  gives that placement its definition.
- **Changing** a size, a material or a seed is one `Forge::Piece` on
  the same target, which replaces the definition. Its inverse is the
  log's undo of that op (`forge_doc::inverse`), so the definition it
  replaced is the one the earlier op in the document still gives.
- **Moving, turning, duplicating, grouping, saving in a prefab and
  removing** are the placement ops every object has. A removal clears
  the definition with it, and a duplicate (an add `like` a piece, with
  no palette write of its own) takes the source's definition.
- The validator refuses a `PIECE_TAG` placement with no definition, a
  definition on any other placement (including a placement whose
  palette index is written back to another tag while it has one), a
  target that is not a crate or is missing or removed, a size outside
  the range, a `param` outside its shape's range, and a `Map` material
  that is not a `shad` key or whose key is not a well-formed tag key.
  The checks of a definition are `PieceDef::validate`'s.
- **The number.** A document with a piece op is format 6
  (`forge_doc::FORMAT_PIECES`, `content::forge::DOCUMENT_FORMAT_PIECES`);
  number 5 is the Firefight wave sets. A document without a piece op
  keeps the number it has, and so its bytes and identity.
- **The tier.** A piece's definition installs tags, which a scenario
  patch has none of, so a document with a piece is a Forge map and not a
  map variant (`ForgeDoc::tier`).
- **Reading it.** `ForgeDoc::pieces(base_counts)` gives each live piece's
  final crate index, the op that gave its definition and the definition;
  `ForgeDoc::piece_targets` gives the definitions by placement identity
  for a live session; `replay` hands the first to `Apply::pieces`, whose
  default refuses a document that has a piece (`Unsupported("piece")`).

A definition's **name** is its canonical spelling,
`<shape>_<x>x<y>x<z>_<param>_<first 12 hex of the SHA-256 of its
canonical JSON>`, and its tags live at `forge\blockout\<name>`. Two
placements with equal definitions share one set of tags.

## A definition as tags

`blockout_kit::install(&mut TagSet, &PieceDef, &Facts) -> Result<Installed>`
adds a definition's tags to a map's tag set, as `pogo_kit::kit` and
`stage_kit::kit` add theirs. `Facts` is what is read from the base map
when it is built: the shader templates (`h1convert::shad::Library`),
the global material by name and the palette survey ("Materials").

| Tag | Made from | With |
|---|---|---|
| `bloc` | its definition in `tags::defs`, as `h1objects::rebuild::scenery` writes a `scen` | the `hlmt`, a bounding radius from the size |
| `hlmt` | `h1objects::rebuild::model` | the `mode`, `coll` and `phmo`; one region, one permutation |
| `mode` | the shell's faces as triangle lists, through `h1_assets::models::mode::convert_model` as `pogo_kit::kit::install_model` does | one part per material in use; absent when every face is `Nodraw` or `Clip` |
| `coll` | `collision_build::build` over the shell, `collision_build::coll::encode_collision` | one BSP, one material, no BSP physics slots |
| `phmo` | `h1objects::rebuild::physics_model` over the hulls | one rigid body, `blockout_kit::PIECE_BODY`: flags 0, motion 5 |

A piece is a crate and not scenery because the engine gives a rigid
body only to bipeds, vehicles, machines, crates and creatures: scenery
with a collision model stops bullets and lets a player walk through it
(**E**, [collision.md](../formats/collision.md), "BSP physics" and
"Which objects get bodies"). Motion 5 is Havok's fixed motion (**E**,
the same page, the rigid body's `+0x1a`), and a crate's body keeps the
motion its tag gives. The `coll` answers the engine's own ray and sweep
tests (bullets, projectiles, a biped's ground checks); the `phmo`
stands in front of Havok. A `coll` with a `phmo` beside it has no BSP
physics slots (**V**, the rule in the same section).

`Installed` lists the tags and `Cost`: the tag memory and the raw bytes
the definition takes. Tag memory is the scarce one: multiplayer maps
have 0.1 to 1.5 MiB of headroom and one has none
([forge-design.md](forge-design.md), "The kit").

## Reaching the game

The classes are those of [forge-design.md](forge-design.md), "What the
engine allows at runtime".

| Edit | Class | How |
|---|---|---|
| Place a piece whose definition the loaded map holds | Live | `object_new` on the definition's `bloc`; the crates block's spare rows and the palette row are the kit's |
| Move or turn a piece | Live | the object is deleted and made again where it is dropped, so its fixed body is built there (`forge::piece::drop`) |
| Remove a piece | Live | the object's deletion |
| Place or resize to a definition the map lacks, of a shape with a slot free | Kit | the slot's tags are rewritten in place and its objects made again ("Slots") |
| The same, with no slot | Rebuild | the piece is **pending**: drawn as a ghost with no collision until **Rebuild map** ([forge-design.md](forge-design.md), "Rebuild map") |
| Dev view | Live, this machine only | "The dev textures" |

**Materialising.** `forge_build::blockout` collects the definitions of
the document's live pieces in name order, installs each once, adds one
row a definition to the scenario's crate palette, and resolves every
`PIECE_TAG` placement to its definition's row. The kit recipe
(`forge_kit`) also installs the dev set, the **starter set**
(`forge_shapes::STARTER`: about two dozen common sizes, so a fresh
session places pieces with no rebuild) and the slots. A published play
document has no kit, so it holds only the definitions its pieces use.

**The cut order.** A kit that does not fit the map's tag memory drops,
in this order and so alike on every machine: the slots from the last,
the starter set from the last, then a definition the document needs is
refused by name with its `Cost` and the headroom. The dev set is
dropped last of all; without it a `Dev` material is refused by name.

**Pending pieces.** `Session::pieces` knows each piece's state:
`live`, `slot` or `pending`. A pending piece has no object. The marker
pass draws its shell as lines and a skin in the pending tint
(`forge_glue::pieces_world`, through `native_render::markers::Batch`),
and the **Rebuild map** row counts it (`Session::pending_rebuild`).

### Slots

A slot is a spare definition of one **topology class** installed at a
stand-in size: `Box` (block, wall, floor plate), `Prism3` (ramp, wedge)
and `Cylinder16`. Within a class every size has the same counts of
vertices, faces, planes and hull corners, so a slot becomes any
definition of its class by writing values and no block grows
(`blockout_kit::slot::plan(&PieceDef) -> Patch`).

| Tag | Written through | Path of [realtime-edits.md](../editor/realtime-edits.md) |
|---|---|---|
| `mode` | the section's vertex bytes, the compression box, the part bounds, the material references | "Geometry", moved vertices with the same counts |
| `coll` | the BSP's blocks staged in the edit arena and committed | "Collision", collision models |
| `phmo` | the hull's corners, planes, box and mass words | physics models, in-place writes |
| `bloc` | the bounding radius | a tag field write |

Then the slot's objects are made again (`world.object.respawn` with a
definition), since a live body keeps the shape it was built with. The
kit reserves `blockout_kit::slot::COUNTS` slots a class (16 boxes, 8
prisms, 4 cylinders). A slot is taken by the first definition a session
needs and freed by the next rebuild, which bakes that definition as a
tag of its own. Stairs, arches and the natural set have no slot: their
counts change with their `param`, so a new one is pending until the
rebuild. Every machine of a synchronous session takes slots in log
order, so they agree on which slot holds which definition.

## Materials

A face draws with the material of its role.

- **`Dev`** is the dev texture of the role's family ("The dev
  textures"). A new piece is `Dev` on every role.
- **`Map(shad)`** names a shader the base map holds, by tag key. The
  render model's material references that tag: the look is the map's
  own and no byte of it is copied. A document that names a shader the
  base lacks is refused by name.
- **`Nodraw`** leaves the role's faces out of the render model; they
  still collide.
- **`Clip`** on every role is a barrier: no render model at all
  ([forge-features.md](forge-features.md), "Invisible barriers").

**The palette survey.** `blockout_kit::palette::survey(&mut Map)` lists
the surfaces the level is made of, for the Material picker: for each
material of the first structure BSP, its shader's key, the area of the
render triangles drawn with it, the median length in world units of one
repeat of its texture across those triangles (so a piece tiles at the
density the level does, rounded to a power of two of units), and its
class from the collision material's global material name
(`palette::class`: rock and ground, metal, other). The picker lists
**Dev** first, then the map's surfaces by area, named by the shader's
base name. The natural set starts as the largest surface of the rock
and ground class, or `Dev` on a map with none.

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
`dev_texture::font() -> Image` draw RGBA, and `blockout_kit::dev`
writes each as a `bitm` (`h1_assets::bitmaps::convert::convert_with`,
DXT1 with its mips, as `pogo_kit::texture` writes its atlas) with a
`shad` on a template the map has (`h1convert::shad::Library::build`
over `shad::TEX_BUMP`, our bitmap as the base map). Four tiles (Floor,
Wall, Ceiling, Trim) and the font are tags, at
`forge\blockout\dev\<family>` and `forge\blockout\dev\font`. A tag is
the one form the lifted D3D path, the native renderer, a film and
another player's machine all draw without code of their own, and the
game's texture cache and mips serve it as any texture. The rasters are
a pure function of `dev_texture::VERSION`; a process draws and encodes
them once (`blockout_kit::dev::encoded`) and keeps the encoded bytes
under `<data>/cache/forge-blockout/dev-<VERSION>` for the next.

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

- **The object menu** gains two groups
  ([forge-editing.md](forge-editing.md), "The object menu"):
  **Structure** (Block, Wall, Floor plate, Ramp, Wedge, Stairs,
  Cylinder, Arch, Barrier) and **Natural** (the five shapes, eight
  seeds each). A row places the shape at its starting size, `Dev`, and
  holds it.
- **Holding a piece** rounds its origin to the block-out grid on the
  world's axes when the piece stands at a quarter turn, and otherwise
  rounds as any object does. The Rotation snap applies as it is.
- **Resize** (Properties) is a draft, as **Edit area** is: the reticle
  picks the face it aims at, the left stick's forward and back push or
  pull that face by a grid step, and the opposite face stays where it
  is. The draft draws as the pending ghost does, with the face's size
  in units on the HUD's readout. A writes one `Forge::Piece` (and a
  `Placement::Set` when the origin moved, in the same undo step); B
  cancels. X opens **Size**: Width, Depth and Height rows, each opening
  the keyboard for a number of units, left and right nudging by the
  grid; Steps, Sides or Voussoirs where the shape has one.
- **Material** (Properties) opens a picker: All faces, or Floor, Wall,
  Ceiling and Trim, then Dev, the map's surfaces, Nodraw.
- **Reroll** (Properties, a natural piece) takes the next seed.
- **Tool settings** gains **Block-out grid**; the Overlays page gains
  **Block-out** and **Dev view**.
- A refused size or material shows the validator's sentence as the HUD
  notice and leaves the document as it was.

## Interfaces

- **Document**: `Forge::Piece`, `forge_doc::PIECE_TAG`,
  `ForgeDoc::pieces` (each live piece's placement and definition).
- **Materialiser**: `forge_build::blockout::{definitions, install}`;
  `blockout_kit::{install, dev, slot, palette}`.
- **Session**: `Session::place_piece`, `set_piece`, `pieces`;
  `World::piece_loaded(name)`, `World::slot_take`, `slot_write`.
- **Nav.** A piece is a crate placement with a `coll`, which
  `forge_nav::read` already takes as a solid
  ([forge-nav.md](forge-nav.md)); a pending piece is given to
  **Generate nav** as its shell.
- **Game agent API**: `forge.pieces` lists each piece
  (`{target, def, name, state, slot?, bounds}`) and the slots in use;
  `forge.propose` takes the piece op; `forge.capacity` gains the
  block-out set's `Cost` and what the cut order dropped; `forge.status`
  carries `dev_view` and the grid.
- **Studio agent API**: `blockout.render {shape, size, param?,
  materials?, view?, out?}` draws one definition and, with no shape,
  the contact sheet of every shape in every dev family;
  `blockout.describe {def}` answers its name, counts, hulls, labels as
  text and `Cost`. Both need no disc while every material is `Dev`.
  They are the content render and check of this page's shapes, as
  `markers.render` is the markers'.

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
| **BO4** | **A definition as tags**: the render mesh with cut-out plates, `bloc`, `hlmt`, `mode`, `coll`, `phmo`, the dev set, `Cost`, an invented map for tests | L | `crates/tools/blockout-kit/` (new) but `palette.rs` and `slot.rs` | BO1, BO2 | each `coll` passes `verify::check` and 1,000 random segments agree with brute force on the shell; the `mode` decodes to the mesh within quantisation; the `phmo` decodes to the hulls' corners; the glyph cells of each plate read back as its text; a `Nodraw` role is in the `coll` and not the `mode`; a barrier has no `mode`; two installs are byte-equal; `Cost` equals the bytes added |
| **BO5** | **The document**: `Forge::Piece`, `PIECE_TAG`, the validator's rules, fold, inverse, `ForgeDoc::pieces` | S | `crates/formats/forge-doc/src/piece.rs` (new), `links.rs` (the variant), `validate.rs`, `document.rs` (`FORMAT_PIECES`, with its number in `content::forge`), `lib.rs`, `src/tests/piece.rs` | BO1 | golden canonical JSON and identity; each refusal has a refusing case; fold then inverses gives the start; the seeded fuzz covers the op; a document without a piece keeps its format |
| **BO6** | **Materialising**: definitions collected and installed once, the crate palette, the starter set and the cut order in the kit, the palette survey | M | `crates/tools/forge-build/src/blockout.rs` (new), `lib.rs` (the dispatch), `kit.rs` (the palette rows), `tests/blockout.rs`; `crates/tools/blockout-kit/src/palette.rs` | BO4, BO5 | two builds are byte-equal; three pieces of two definitions add two tag sets and resolve to their palette rows; a `Map` shader the base lacks is refused by name; with headroom set low the cut order is the page's and the refusal names the cost; the survey on an invented structure ranks by area and gives the repeat length |
| **BO7** | **The session**: placing, changing and dropping pieces, the three states, pending, `forge.pieces`, pieces as nav solids | M | `crates/extensions/forge/src/{piece,piece_tests}.rs` (new), `session/pieces.rs` (new), `things.rs` (the kind), `crates/extensions/agent/src/forge/pieces.rs` (new), `crates/extensions/agent-proto/src/forge.rs` | BO5; BO6 for names only | over a fake game: placing a loaded definition is two ops, one undo step and one `object_new` on its `bloc`; a definition not loaded makes no engine call and one pending piece; after `adopt_copy` nothing is pending; a drop deletes and makes the object; undo restores the definition |
| **BO8** | **The monitor**: the Structure and Natural groups, Resize and its draft, Size, Material, Reroll, the Block-out grid, the ghost and the Block-out overlay | M | `crates/extensions/game/src/forge_screens/blockout.rs` (new), the groups in `forge_screens`' sheet, `crates/extensions/forge/src/tool.rs` (the grid and the draft) with `tool_tests.rs`, `crates/extensions/game/src/forge_glue/{editing/resize,pieces_world}.rs` (new), one call in `forge_glue/markers_world.rs`, the two settings' definitions | BO7 | `Page` tables for each screen; a held piece's origin rounds to each grid; a push by one step is one `Forge::Piece`; a pull past the minimum is refused and the document unchanged; a pending piece and a barrier each give their batch |
| **BO9** | **Dev view**: the second render model's write and its undo, on every loaded definition, again after a rebuild | S | `crates/extensions/game/src/forge_glue/dev_view.rs` (new) and its tests | BO6, BO8 | on synthetic guest memory: On writes one reference a textured definition and asks for its respawn, Off restores the bytes; an all-`Dev` definition is untouched; the document and the log are unchanged |
| **BO10** | **Slots**: the stand-in tags, `slot::plan`, taking and freeing, the live writes | L | `crates/tools/blockout-kit/src/slot.rs` (new), `crates/extensions/agent/src/forge/slots.rs` (new), `crates/extensions/forge/src/session/slots.rs` (new), the reservation in `forge-build/src/kit.rs` | BO6, BO7; realtime-edits R4, R7, R12 | for a sweep of sizes a class's counts are equal; a plan applied to the stand-in's bytes on synthetic memory decodes to the tags BO4 makes for that definition; the seventeenth box is pending; a rebuild frees every slot; two sessions fed one log hold the same slots |
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
