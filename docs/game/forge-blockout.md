# Forge block-out: pieces, the natural set and the dev textures

Status: Partly built. Slices BO1 and BO3 are built: `forge-shapes` has the definition, the canonical name, the structural generators (block, wall, floor plate, ramp, wedge, stairs, cylinder, arch), `STARTER`, and the natural set (`noise`, `hull` and the five rock generators). Slice BO2 is built: `crates/formats/dev-texture` has the rasters, the font and the label layout. Slice BO5 is built: `forge-doc` has `Forge::Piece`, `PIECE_TAG`, the validator's rules and `ForgeDoc::pieces` (document format 6). Slice BO4 is built: `crates/tools/content-kit/src/blockout` installs a definition as tags with the dev set and its `Cost`, and has an invented map for tests. Slice BO11 is built: Studio's `blockout.render` (a definition or the contact sheet) and `blockout.describe` in `crates/apps/studio/src/api/blockout.rs`. Slice BO6 is built: `forge_build::blockout` materialises a document's pieces (the definitions, the crate palette rows, the starter set and the cut order in the kit) and `content_kit::blockout::palette` is the palette survey. Slice BO7 is built: the session places, changes and drops pieces and knows each one's state (`Session::{place_piece, set_piece, pieces}`, `forge::piece`, `World::piece_loaded`), and `forge.pieces` lists them. Slice BO8 is built: the monitor's Structure and Natural groups, Resize and its draft, Size, Material, Reroll, the Block-out grid, the ghost and the Block-out overlay. The rest of the page is a design. It is slice F8 of [forge-design.md](forge-design.md), split into slices BO1 to BO12 below.
Code: `crates/formats/forge-shapes` (the definitions and generators), `crates/formats/forge-doc/src/piece.rs` (the document), `crates/formats/dev-texture` (the dev textures), `crates/tools/content-kit/src/blockout` (a definition as tags), `crates/tools/forge-build/src/blockout.rs` (materialising), `crates/extensions/forge/src/{piece.rs,session/pieces.rs}` (the session), `crates/extensions/agent/src/forge/pieces.rs` (`forge.pieces`), `crates/extensions/forge/src/{piece.rs,tool.rs}` (the grid, `Draft`, the resize steps), `crates/extensions/game/src/forge_screens/blockout.rs` and `forge_glue/{editing/resize.rs,pieces_world.rs}` (the monitor), `crates/apps/studio/src/api/blockout.rs` (Studio's render and describe). Tests: named per slice under "Slices".
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

`content_kit::blockout::install(&mut TagSet, &PieceDef, &Facts) -> Result<Installed>`
adds a definition's tags to a map's tag set, as `pogo_kit::kit` and
`content_kit::stage::kit` add theirs. `Facts` is what is read from the base map
when it is built: the shader templates (`h1convert::shad::Library`,
of which the pieces draw with `shad::TEX_BUMP`), the global material
(`h1objects::rebuild::PIECE_MATERIAL`'s where the map has it, else the
first). Every tag
is at the definition's path, and a definition the set already holds is
refused; so is one that does not generate, whose collision fails its
check, or whose `Map` material names a shader the set lacks, with
nothing added. A set made with `TagSet::from_map` takes
`content_kit::blockout::make_index_room` first.

| Tag | Made from | With |
|---|---|---|
| `bloc` | its definition in `tags::defs`, as `h1objects::rebuild::scenery` writes a `scen` (`content-kit/src/blockout/object.rs`) | the `hlmt`, the bounding sphere of the bounds |
| `hlmt` | `h1objects::rebuild::model` | the `mode` (none for a barrier), `coll` and `phmo`; one region and permutation, `default`; one node, `piece`, at the origin; the collision material `blockout` mapped to the global one |
| `mode` | the mesh of `content_kit::blockout::mesh`, through `h1_assets::models::mode::convert_model` as `pogo_kit::kit::install_model` does | one part per shader in use, triangle lists; absent when every face is `Nodraw` or `Clip` |
| `coll` | `collision_build::build` over the shell, checked by `collision_build::verify::check`, the BSP's eight blocks (`Bsp::encode_blocks`) written as a tag tree so the set interns its names (`content_kit::blockout::collision`) | one BSP, one material, one node, no BSP physics slots |
| `phmo` | `h1objects::rebuild::physics_model` over the hulls (`content_kit::blockout::physics`) | one rigid body on no node, `content_kit::blockout::piece_body`: flags 0, motion 5, the size class `+0x1e` of the retail band its bounding sphere falls in; a mass of 1,000 kg whatever the size |

**The render mesh** (`content_kit::blockout::mesh::build`). A face draws with
its role's material: `Dev` the role's tile, `Map` the named shader,
`Nodraw` and `Clip` not at all. A `Dev` face that is a rectangle, on a
shape that is not a rock, carries its plate: the face is cut into four
frame quads around the plate, drawn with the tile, and the plate's 48
cell quads, drawn with the font atlas, `v` down. On a wall the plate's
up is the edge nearest world up and its x runs to the right seen from
outside; on a floor or a ceiling its x is the edge nearest world x. A
rock's vertex takes the mean normal of the faces at it within 40
degrees of its own; any other vertex its face's.

**The dev twin.** A definition whose roles are not all `Dev` also gets
the `dev` render model, every role `Dev`, at the path of
`PieceDef::dev(shape, size, param)`, so definitions that differ only in
materials share one; an all-`Dev` definition's render model is that
twin. A definition with no drawn face has neither.

A piece with no object is drawn by `forge_glue::pieces_world`: a pending piece's shell as lines and a skin in the pending tint, the ghost of a placed or resized piece, and with the Block-out overlay on the barriers and `Nodraw` faces, each a `markers_world::Group`. `ForgeWorld::piece_loaded` is true for a definition whose `bloc` the session's palette holds (`forge::piece::loaded_names`).

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

`Installed` lists the tags and `Cost`: what the install added, the dev
set included when this install added it. `Cost::tag_memory` is each
added tag's size as `map::build` lays it out (`content_kit::blockout::tag_size`:
the blocks in emission order, each at its alignment),
`Cost::raw` the raw bytes, `Cost::tags` the index entries. Tag memory is the scarce one: multiplayer maps
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

**Materialising.** `forge_build::blockout::definitions` collects the
definitions of the document's live pieces in name order, once each.
`blockout::install` installs each on the base map
(`content_kit::blockout::install` over `TagSet::from_map`) and writes the
scenario's crate palette again as a new block: the base's rows, then one
row a definition. The scenario editor then opens the map that gives, and
the placement edits go through it as any other: a `PIECE_TAG` palette
write is the placeholder -1, and `Apply::pieces`
(`forge_build::placement`) gives every piece's placement its
definition's row. The kit recipe (`KIT_RECIPE`) also installs the dev
set first and the **starter set** (`forge_shapes::STARTER`: about two
dozen common sizes, every role `Dev`, so a fresh session places pieces
with no rebuild; `forge_build::kit::starter`) after the document's
definitions, each with a palette row after theirs, and, with slice
BO10, the slots. A published play document has no kit, so it holds only
the definitions its pieces use. `Built::blockout` reports each row, its
`Cost`, what was cut and the headroom. A kit build on a map with nothing
the pieces draw with (no `tex_bump` template, `Facts::read`) carries no
block-out set and says why (`Report::unavailable`); a document with a
piece on such a map is refused.

**The cut order.** A kit that does not fit the map's tag memory (what
the tags add, as `Cost::tag_memory`, and the palette block, against
`blockout::headroom`: `max_metadata_size` less the header's `+0x1c`)
drops, in this order and so alike on every machine: the slots from the
last, the starter set from the last, then the kit's own dev set; then
the first definition the document needs that does not fit, in name
order, is refused by name with its `Cost` and the headroom left for it
(`Error::PieceCost`). The dev set is cut only where no definition the
document needs draws a `Dev` face, since any drawn definition has the
dev render model; a definition whose install adds the dev set is
refused with the dev set in its cost, the sentence naming its `Dev`
material. A `Map` material whose shader the base lacks is refused by
name before anything is installed (`Error::Piece`).

**In the session.** A piece is an object of the session like any added
crate (`scene::project` gives it the item of its definition's own `bloc`,
`forge::piece::item`), so the ordinary object edits apply and the
world is asked for one call each: placing is `Session::place_piece`, two
ops in one undo step and one `World::place` of the `bloc`; changing is
`Session::set_piece`, one `Forge::Piece` and, when the origin moved, one
`Placement::Set` for each of the position and the turn; a drop is
`forge::piece::drop`, one `EntryOp::Set` for the position or the turn;
removing and duplicating are `Session::delete` and `Session::duplicate`,
the duplicate being a placement of the source's definition. A held piece
does not move in the world, because its body is fixed where its object was
made; the drop deletes the object and makes it again where it lands, and a
change of definition does the same, since another definition is another
`bloc`. A refused definition (`PieceDef::validate`) is refused before any
op is stamped. A shared session does not place pieces
(`PIECES_LOCAL_ONLY`): the host takes one op at a time, and the add of the
piece tag is refused until its definition's op follows.

**Pending pieces.** `Session::pieces` knows each piece's state:
`live`, `slot` or `pending`. The state is whether the world has an
object: the world makes one only for a definition it holds
(`World::piece_loaded(name)`, false by default), so a piece the map lacks
makes no engine call and is pending, until **Rebuild map** has the game
report the objects it made (`Session::adopt_copy`, after which nothing is
pending). `slot` is the state BO10 gives; nothing here does. A pending piece has no object. The marker
pass draws its shell as lines and a skin in the pending tint
(`forge_glue::pieces_world`, through `native_render::markers::Batch`),
and the **Rebuild map** row counts it (`Session::pending_rebuild`).

### Slots

A slot is a spare definition of one **topology class** installed at a
stand-in size: `Box` (block, wall, floor plate), `Prism3` (ramp, wedge)
and `Cylinder16`. Within a class every size has the same counts of
vertices, faces, planes and hull corners, so a slot becomes any
definition of its class by writing values and no block grows
(`content_kit::blockout::slot::plan(&PieceDef) -> Patch`).

| Tag | Written through | Path of [realtime-edits.md](../editor/realtime-edits.md) |
|---|---|---|
| `mode` | the section's vertex bytes, the compression box, the part bounds, the material references | "Geometry", moved vertices with the same counts |
| `coll` | the BSP's blocks staged in the edit arena and committed | "Collision", collision models |
| `phmo` | the hull's corners, planes, box and mass words | physics models, in-place writes |
| `bloc` | the bounding radius | a tag field write |

Then the slot's objects are made again (`world.object.respawn` with a
definition), since a live body keeps the shape it was built with. The
kit reserves `content_kit::blockout::slot::COUNTS` slots a class (16 boxes, 8
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

**The palette survey.** `content_kit::blockout::palette::survey(&mut Map, raw)`
lists the surfaces the level is made of, for the Material picker: for
each shader drawn by the scenario's structure BSPs
(`scenario_edit::geometry::world_parts`, transparent parts left out),
its key, the area of the render triangles drawn with it, the median
length in world units of one repeat of its texture across those
triangles (each triangle's mean of the lengths a unit step of `u` and
of `v` spans; `Surface::tile` is that in units rounded to a power of
two by ratio, so a piece tiles at the density the level does), and its
class from the global material name of the collision material
(`sbsp +0x0c`) that names the same shader (`palette::class`: rock and
ground, metal, other). `palette::rank` does the ranking over invented
triangles in the tests. The picker lists **Dev** first, then the map's
surfaces by area, named by the shader's base name. The natural set
starts as the largest surface of the rock and ground class
(`palette::natural`), or `Dev` on a map with none.

## Pages

| Page | What |
|---|---|
| [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md) | The natural set, the dev textures, the monitor, interfaces, rules, slices and evidence. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## The natural set

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#the-natural-set).

## The dev textures

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#the-dev-textures).

## In the monitor

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#in-the-monitor).

## Interfaces

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#interfaces).

## Rules and limits

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#rules-and-limits).

## Slices

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#slices).

## Decisions

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#decisions).

## Evidence

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#evidence).

## Not verified

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#not-verified).

## Related

See [Forge blockout: the natural set, dev textures and monitor](forge-blockout-set.md#related).
