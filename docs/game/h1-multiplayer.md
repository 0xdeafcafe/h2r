# Halo 1 multiplayer maps in the Halo 2 engine (design)

Status: in progress, started 2026-11-16; converted maps confirmed in the
game by the owner (2026-11-17). Each section says what is
**Built** (with its proof) and what is design only. This applies the rules
of [campaigns.md](campaigns.md) to multiplayer: Halo 2's engine runs
everything (physics, AI, rendering, game types); Halo 1's health bar and
health packs are the one thing kept from Halo 1; Halo 1's assets are the
default, with a setting to use Halo 2's.

Like every import, this reads the user's own disc (`disc/halo1.iso`). No
map bytes, tag dumps or extracted assets are in the repository. Tests use
invented data, or read real maps at run time in `#[ignore]` tests
(`H1_MAP_DIR`, a folder of maps as stored on the disc).

## Goal

- Halo 1's multiplayer maps appear in Halo 2's multiplayer map list and
  play with Halo 2's game types.
- **Setting, Halo 1 maps sandbox: Halo 1 (default) / Halo 2.**
  - **Halo 2:** Halo 2's weapons, vehicles, bipeds, health and shields.
    Halo 1's weapon and vehicle spawns become their nearest Halo 2
    equivalents (table below).
  - **Halo 1:** Halo 1's weapons, vehicles and biped, converted to Halo 2
    tags and running on Halo 2's physics, with a health bar and health
    packs from the health subsystem ([campaigns.md](campaigns.md) §6).
- The import writes both variants of every map. The setting picks which one
  is served.

## Pipeline

```
disc/halo1.iso ─▶ <mp map>.map (cache v5, zlib)
                    │  h1map: header, index, scenario MP parts, sbsp
                    ▼
            Halo 1 data (in memory)
                    │  h1convert: per part, Halo 1 → Halo 2
                    ▼
   retail Halo 2 MP map (the base) + converted sbsp/ltmp, sky, placements
                    │  map::build (TagSet from the base)
                    ▼
   <data>/maps/h1_<map>/{h2,h1}/h1_<map>.map + manifest.toml ─▶ content package
```

**The base.** Each converted map starts from a retail Halo 2 multiplayer
map, which supplies the globals, the Halo 2 sandbox (weapons, vehicles,
bipeds, item and vehicle collections), the UI and everything else a
multiplayer level needs. The converter replaces its structure BSP,
lightmap and sky, and rewrites its scenario's multiplayer placements.

**Pinned base, either load-test outcome.** `TagSet::from_map` pins every
base tag at its address and keeps the index tail. The converter never adds
a tag slot: it gives the base's existing `sbsp` and `ltmp` slots new
`Built` payloads in a new BSP section, and grows only blocks (spawn points,
flags, equipment) through fixups or the scenario editor's metadata append.
So it works whether or not `h2 map testbuild`'s `fresh` variant loads.
The Halo 1 sandbox adds tags: `TagSet::add_tag` grows a pinned set's
index and keeps its tail (below, "Halo 1 sandbox").

## Halo 1 data read (**Built**)

`crates/h1map`, verified on beavercreek and carousel
([h1-map-format.md](../formats/h1-map-format.md) has every layout and its
evidence): the scenario's skies, player starting locations, netgame flags,
netgame equipment, vehicle placements and palette; item collections; each
BSP region's header and `sbsp` root; the collision BSP; and the render
geometry (lightmaps, materials, compressed vertices, triangles).

## Collision (**Built**)

`h1convert::collision::convert` turns the Halo 1 structure collision BSP
into Halo 2's (`tags::collision::Bsp`). Both are the same tree; Halo 1
stores every index as an `i32` with bit 31 as the flag, Halo 2 packs 3D
children into 24 bits and the rest into `u16` with bit 15 as the flag. The
conversion copies planes and vertices bit for bit and re-encodes every
index, refusing any that doesn't fit (no Halo 1 multiplayer map seen comes
close: beavercreek has 5,033 nodes and 2,476 surfaces).

Halo 2 keeps a structure BSP's collision at `sbsp+0x14`: one `0x40`-byte
element, the eight blocks of an object collision BSP without its leading
word, with collision materials at `sbsp+0x0c`. **V** on all 12 multiplayer
maps of the Halo 2 disc by `h1convert/tests/h2_sbsp.rs`. Surfaces with no
material hold `0xffff` in both games.

**Proof** (`h1convert/tests/real_maps.rs`, on both maps): the conversion is
encoded with Halo 2's layout and decoded back with `crates/tags`'s decoder,
passes its validator, and agrees with the Halo 1 source on every surface's
polygon, on 50,000 random points (open or solid, and which leaf), on 20,000
random segments and 20,000 segments from open space (where each first
enters solid, compared exactly), and on a ray down from every spawn point.

## Halo 2 sandbox table (**Built** as `h1convert::sandbox`)

Halo 1 items are recognised by plain words in their tag path; the Halo 2
item is found in the base map by the words in its tag name.

| Halo 1 | Halo 2 | Why |
|---|---|---|
| Assault rifle | SMG; the battle rifle in precision variants | The user's rule, below |
| Pistol | Magnum | Same slot; the user's choice |
| Plasma pistol | Plasma pistol | |
| Plasma rifle | Plasma rifle | |
| Needler | Needler | |
| Sniper rifle | Sniper rifle | |
| Rocket launcher | Rocket launcher | |
| Fuel rod gun (plasma cannon) | Rocket launcher | Halo 2's multiplayer has no fuel rod |
| Shotgun | Shotgun | |
| Flamethrower | Shotgun | Close-range role; Halo 2 has none |
| Frag grenade, plasma grenade | Same | |
| Overshield, active camouflage | Same (`powerup_super_shield`, `powerup_invisibility`) | Dropped with a report when the base has no collection for it (Beaver Creek and Coagulation have no camouflage collection) |
| Warthog, Ghost, Scorpion, Banshee | Same | A collection is built when the base lacks one ([Vehicles](#vehicles)) |
| Rocket warthog | Gauss warthog | The user's decision |
| Health pack | Dropped | Halo 2 has no health |
| Ammo pickups | Dropped | Halo 2 has no ammo pickups |
| Double speed, full-spectrum vision | Dropped | Unused on Xbox maps |

**Precision variants (the user's rule, data in `sandbox::PRECISION`).** A
Halo 1 assault rifle spawn is the SMG by default and the battle rifle when
the game variant is a precision one: its primary starting weapon is the
battle rifle, carbine or sniper rifle (`PRECISION_STARTING_WEAPONS`).
`h2_equivalent_precision` gives the precision item. Choosing it at game
start needs a runtime hook, not built yet: at round start, read the
variant's primary starting weapon, and if it is a precision weapon swap
the item collection of the spawns that came from assault rifles (the
converter would record which netgame equipment entries those are, e.g. in
the package manifest). Until then both variants place the SMG.

## Placements plan (**Built** as `h1convert::placements`)

`plan_map` turns a Halo 1 map's multiplayer parts into Halo 2 sandbox
records, and lists everything it leaves out:

| Halo 1 | Halo 2 |
|---|---|
| Player starting location | Starting location: position, facing, team. Halo 1's per-spawn game types are dropped (Halo 2 has no field for them; decided) |
| CTF flag (per team) | CTF flag spawn and CTF flag return at the same place (Halo 1 uses one point for both) |
| Oddball | Oddball spawn |
| Teleporter from / to | Teleporter source / destination, channel in the team field |
| Hill flag | Hill marker (hill number in the team field) |
| Race track, race and CTF vehicle points, Vegas bank | Dropped: Halo 2 has no race |
| Netgame equipment | Item spawn of the item collection's first item's Halo 2 role (table above), with its respawn time |
| Vehicle placement | Vehicle spawn by its palette entry's role |

`resolve` picks the base map's item collection (`itmc`) or vehicle
collection (`vehc`) for a role by the words in its name. **V:** on
beavercreek and carousel every spawn is placed and every item spawn is
either placed or one of the known gaps (health packs, ammo,
`the_halo_2_sandbox_plan_of_each_map`).

## Placements onto the base (**Built** as `h1convert::apply`)

`apply` writes a plan into a base map's scenario through `scenario-edit`
(without its egui panel): it removes the base level's object placements
(scenery, vehicles, equipment, weapons, machines, sound scenery, crates),
replaces every starting location, netgame flag and netgame equipment entry
with the plan's (each a copy of the base's first entry, so unset fields
keep retail values), and reports item roles the base has no collection
for. `scenario_edit::save::write_patched` then writes the map; blocks that
grow go to the end of the metadata. **Proof:** a synthetic test on the
editor's fixture reads every written field back, and
`the_plan_applies_to_a_retail_base` writes beavercreek's plan into the
Halo 2 disc's `beavercreek` (70 spawns, 88 flags, 28 items, 56 base
placements removed; the 3 camouflage spawns are reported, the base has
none) and reads the scenario back.

Halo 2's netgame flag kind values (`H2Flag::raw`) follow the community
enum, and the disc agrees (`h1convert/tests/h2_scenario.rs`: 0 and 1 once
per team on all 12 maps, 4 always neutral, 7 and 8 in equal numbers, 10
to 16 runs of boundary points); not yet confirmed from engine code.

**Trigger volumes (Built, `convert::park_trigger_volumes`).** The base's
trigger volumes (`scnr+0x108`, stride `0x44`, E `0x0011c383`..
`0x0011c392`; the engine builds each volume's frame from the directions at
`+0x0c` and `+0x18` and the origin at `+0x24`, E `0x0011c40c`..
`0x0011c417`) are its kill zones and script triggers, shaped for its own
geometry. They aren't removed, since scripts, the kill list and BSP
switches (`scnr+0x130`, `0xe` entries, E `0x0014b6d6`) hold their
indices; each origin moves to `(0, 0, -1e6)`, where nothing ever is.

**Lights (emptied, `convert::BASE_BLOCKS`).** `scnr+0xe8` is the light
fixture placements, `0x6c` each: `+0x00` an index into the light
palette at `+0xf0` (`0x28` each, its tag reference at `+0`), `+0x02` a
name (-1), `+0x08` the position. Coagulation has 21, Beaver Creek 10, all
placed for the base's geometry; the block is emptied with the
decorators, background sounds and sound environments.

**Decals.** The base has no placed decals: no scenario block on
Coagulation or Beaver Creek names a `deca`. Their 51 `deca` tags come
from effects (bullet holes, scorch marks), which Halo 1's objects use too,
so nothing is emptied for decals.

## The converted structure BSP (**Built** as `h1convert::structure`)

`Structure::write` writes the `sbsp` and `ltmp` below as `map::build`
block trees into the base's own `sbsp`/`ltmp` slots (same datums, so
nothing that names them changes), each cluster's render geometry as a raw
`blkh` block, and adds a built BSP section; `Prepared::build` then points
the base scenario's BSP descriptor (file offset, size, address) at it.
The base's old BSP region stays in the file, unused.

- **Render geometry.** Each Halo 1 lightmap material is a mesh: its
  vertices (position, packed normal/binormal/tangent, texture and lightmap
  coordinates, which both games store alike) and its triangles from the
  render surfaces. Triangles are turned to Halo 2's winding, decided once
  per level by majority against the packed normals: Halo 1's beavercreek
  needed flipping. Meshes are packed into sections by lightmap page (a
  cluster has one page, its render info's; unlit meshes last), each of
  at most 65535 vertices and indices (the section's `u16` counts), so a
  level becomes one cluster or more per page (Beaver Creek 14, Blood
  Gulch 16); parts never span sections. Streams written:
  `01` position, `18` texture coordinate, `1b` frame, `1f` lightmap
  coordinate, `30` incident light direction (Halo 1's lightmap vertex
  direction, the same 11:11:10 packing; the normal where a vertex has
  none). One subpart per part.
- **Every render field retail fills** (docs/formats/structure-bsp.md,
  "Geometry header", "Section struct"). Parts: kind 2 (opaque) and flags
  `0xb`, as on all 2,450 retail multiplayer parts; the bounding sphere
  (`+0x10` centre of the box, `+0x2c` farthest vertex from it, at least
  1 mm) and the box (`+0x30`). Cluster section info: shadow-casting,
  opaque and rigid counts equal to the totals, compression flags 4 (the
  `i16` lightmap coordinates), the subpart count, lighting flags 3
  (lightmap coordinates and incident radiance; 0 for an unlit cluster).
  Cluster tail: `+0x6d` `0xff`, the structure checksum at `+0x94`, and
  the predicted resources (`+0x84`): the cluster's own geometry (type 3),
  its lightmap page (type 0, image = page) and every bitmap its
  materials' shaders name (type 0, image 0), each only when the bitmap
  has that image (the engine indexes the images unchecked); the engine
  loads these for the camera's cluster and those it sees (E
  `0x0016e5e0`). Lightmap bucket entries (types 6 to 8) are never
  written: the converted lightmap has no buckets and the engine indexes
  them unchecked. `+0x6c` stays 0: the engine reads it as the cluster's
  first sky (E `0x00130f60`) and falls back to `+0x6e` only for -1, with
  a check that can pick the BSP's own sky instead. Each section has a
  visibility MOPP with one leaf per subpart (`mopp::build` over each
  part's box) and a reorder table of `2n` entries, the leaf order and its
  inverse, which is how the engine's volume pass (E `0x00044e90`) reads
  a wholly seen subtree's keys (docs/formats/structure-bsp.md). Without
  them the pass took the object branch and faulted every converted level
  at converter version 7 (pc `0x4539c`). Version 9 keyed a leaf per
  triangle and wrote a table of `n`: the pass read past it and set bits
  past the view's mask area, into the area's word count, and clearing
  that many words ran over the view and what follows it. The campaign
  levels faulted: a30 and d20 in the clear (`0x134925`), a10, b30 and d40
  on a render pass object it had zeroed (`0x147b75`).
  `tests/render_fields.rs` surveys which 16-bit words of every drawn
  retail multiplayer cluster, part and section are nonzero, and which
  words of every retail bitmap image are none, and fails if a converted
  map differs, except for the cluster's portals (Halo 1's are not
  converted), or if a converted section's code and table are not laid
  out as the pass reads them. `h1campaign/tests/disc.rs`
  (`every_campaign_level_s_clusters_are_drawn_as_the_engine_reads_them`)
  checks the same layout, the mask count and every predicted resource on
  all ten converted campaign levels.
- **Bitmap images say "not cached".** Every converted image (Halo 1's
  textures, its lightmap) wrote 0 at `+0x20..+0x33`; every retail image
  has `0xffffffff` there: LOD pointers 1 and 2, and the texture cache's
  handle per LOD (`+0x28`, `+0x2c`, `+0x30`). The cache reads a handle
  other than -1 as "resident in that slot" and never loads the pixels
  (E `0x0012c9fb`), so every converted texture drew whatever cache slot
  0 held: the level's one flat colour. `h1bitmaps` now writes -1 there
  (`IMAGE_NONE_WORDS`).
- **Clusters see each other.** The visibility rows have every cluster's
  bit set, and every collision leaf is in cluster 0, so everything is
  drawn from everywhere. Halo 1's own portals and clusters are the
  improvement for large levels.
- **Materials** (`h1convert::shaders`). One structure material per Halo 1
  shader, each copying the head (shader reference) of the base structure
  material it is matched to by name: the words of the Halo 1 shader's name
  against each base shader's (`grass`, `cliff`, `metal panel`, ...), Halo 1
  water (`swat`) to a base water shader, anything unmatched to the base's
  first plain opaque shader. Water, glass, sky, decal, transparent,
  self-illuminated, plasma and `unwrapped` shaders never stand in for an
  ordinary surface. Until this, every surface used the base's first
  material's shader, which on Beaver Creek is an `unwrapped` cliff (its
  textures laid out for one mesh's own UVs) and on Coagulation a water
  pool: tiled over Halo 1's UVs both look like scattered multicoloured
  blocks, which is what the editor saw. The bitmaps themselves were never
  wrong: `disc_maps.rs` decodes every image of every converted map
  through `assets::decode_image` and compares it with the base's, and all
  are identical (1618 images on Beaver Creek, 1686 on Coagulation). The
  CLI prints each choice (`shader <Halo 1> -> <Halo 2>`); the chosen
  shader is then the template for Halo 1's own textures (next). One
  collision material per
  Halo 1 collision material, copies of the base's first (no pointers in
  it), `+0x0a` = -1.
- **Halo 1's own textures** (`h1convert::textures`). For each Halo 1
  environment (`senv`) or water (`swat`) shader, its bitmaps by role:
  `senv` base map `+0x88`, primary detail `+0xb8` (else micro detail
  `+0xfc`), bump `+0x128`; `swat` base `+0x4c` (offsets from a sweep of
  every shader on the 13 maps). The chosen base shader's roles come from
  its compiled passes (`tags::material`): base is stage 0 of the first
  pass with a bitmap, detail its stage 1, bump any parameter bitmap in
  `P8Bump` (format 17 in both games; a Halo 1 bump is swapped only when
  it is `P8Bump` too). The Halo 1 bitmaps are converted by
  `h1bitmaps::convert_bitmap` and added as new `bitm` tags under their
  Halo 1 names; a new `shad`, named after the Halo 1 shader, is the
  chosen shader's whole tree (the schema's depth-first copy,
  `Relocator::to_built`) in which every bitmap parameter (postprocess
  `+0x20`, its parameters `+0x04`, stride `0xc`, datum at `+0`) holding
  a swapped role's datum names the Halo 1 bitmap. The copy points at
  nothing of the chosen shader's, so it holds in the fresh build too
  (`tests/fresh.rs`: every reference of the fresh Blood Gulch
  resolves); the blocks on the path to the parameters drop their
  sharing key, the rest keep it. A material head is appended per
  copy and the structure material points at it. `textures::plan` runs in
  `prepare`, `Retexture::install` in `assemble` after the tags whose
  datums are predicted (objects, collections). Shaders of other types
  (`soso`, `schi`, `sgla`, ...) keep the base's, and the CLI says so.
  Left out: Halo 1's detail and bump tiling scales (the template's stay),
  the detail blend function, and the shader's `+0x2c` block (it still
  names the chosen shader's bitmaps; the draw path reads the
  parameters). On blend-detail templates (`tex_bump_detail_blend_detail`,
  Coagulation's ground) stage 1 of the first pass is the second blended
  base (`coag_grass`), so Halo 1's detail map lands there: a game run
  shows whether that reads well.
- **The sky is Halo 1's** (`h1convert::sky`). Cluster `+0x6e` is the
  cluster's sky index into the scenario's skies (`scnr+0x8`, 8-byte
  references; E `0x0014b38d`..`0x0014b3c4`, where -1 also falls back to
  the first sky when the scenario has one). Every cluster gets 0. Each
  sky the Halo 1 scenario names is converted by `h1sky` (model, lights,
  fog colour; docs/formats/h1-sky.md) and added as a new tag, and every
  element of the base's skies block names one (element `k` Halo 1's sky
  `k`, or Halo 1's last), so none names the base's. Rat Race and Chiron
  TL34 are indoors and name no sky: the base's stays, unseen. `disc_maps.rs`
  checks every converted map's skies. The base's decorator block
  (`convert::BASE_BLOCKS`: `+0x378`, 2 `DECR` on Coagulation, D) is
  emptied: the converted clusters name none. The background sound and
  sound environment palettes (`scnr+0x250`, `+0x258`) name Halo 1's
  (below, "Ambient sound"). The base per map (`convert::MAPS`):
  Coagulation for the maps with vehicles (below), Beaver Creek otherwise.
  Waterworks (snow, and a wraith) would suit Sidewinder and Ice Fields,
  but Sidewinder's placements grow Waterworks' metadata past the engine's
  tag memory, so they take Coagulation too.
- **Lightmap** (Halo 1's own). One group, the structure checksum (FNV-1a
  of the positions), the bitmap reference `+0x18` naming Halo 1's
  lightmap bitmap converted (`h1bitmaps::convert_bitmap`: one R5G6B5
  image per page, Halo 1's bytes unchanged), and per cluster a render
  info `{page, -1}` (`{-1, -1}` for unlit clusters). The bitmap takes the
  slot of the base's own lightmap bitmap, which nothing else names
  ([Freed slots](#vehicles)), through `h1models::Converted::install_at`.
  The lightmap coordinates (stream `1f`, in every cluster section as on
  retail maps) are page-relative, in [0, 1]. Retail lightmaps also carry
  per-vertex lighting buckets (`+0x40`, streams `30` and `2f`); clusters
  read none (`+0x60` empty, as on Coagulation).

  **Colour pages with no palette (E).** Retail pages are P8 with a
  palette per cluster; Halo 1's are R5G6B5. Nothing in the engine needs
  P8: the structure renderer stores the cluster's page whatever its
  palette, and binds a palette at stage 3 only when one is named (E
  `0x0004d3ff`..`0x0004d420`: `cmp edx, -1` skips the bind, the page goes
  to `[0x485a60]` either way), and the texture sampler returns a colour
  for both. The CPU lightmap query that lights objects (E `0x000d2f90`)
  samples the page through the general pixel reader (`0x001362b0`) and
  looks the value up in a palette only when the image's format is P8
  (`cmp word [esi+0Ch], 12h` at `0x000d309a`); otherwise the sample is the
  colour. So the pages stay as Halo 1 stored them: no quantizing.

**Proof (tests).** `structure_tests.rs` writes an invented two-mesh level
and a 30,000-triangle one into an invented map and reads them back with
Halo 2's own decoders: `tags::structure` (clusters, parts, materials,
every vertex, triangles, the lightmap group), `tags::collision` (the
collision BSP equals the converted one), and the root's leaves,
visibility, bounds, Havok box, MOPP block and the one-element blocks. The
ignored `beavercreek_converts_onto_its_halo_2_base_and_reads_back` does
the whole conversion on the real maps: Halo 1's beavercreek onto the
disc's Beaver Creek gives one cluster, 4242 vertices, 3317 triangles, 18
materials, 13 collision materials, 70 spawns, 88 flags, 28 items; the map
reads back with a valid checksum, its scenario's descriptor leads to the
new structure, every cluster loads, and the trigger volumes are parked.
Those two tests use a stand-in MOPP block (a header and `00`, which emits
nothing); nothing written for play does.

Not covered by the engine reading and left empty: root blocks at `+0x4c`,
`+0x94`, `+0x148` and `+0x1d0`, which every retail multiplayer BSP fills
(see the disc survey under "Still to do"). No direct read of them from
the active BSP pointer was found; if the game faults on load, these are
the first suspects.

## What the engine reads

Read from the retail XBE's code (static reading only; **E** cites the
instruction, **Guess** is inference). The scan followed every load of the
active BSP pointer `[0x4e0348]` and active lightmap `[0x4e0344]`, and the
cluster pointers derived from them. Plan: **one cluster** holding all of
Halo 1's render geometry, no portals, no instanced geometry.

### Blocker: the structure's Havok MOPP (`sbsp+0x1e0`)

- At every BSP activation the new-BSP callback `0x001c2910` calls
  `0x001c3b70` unconditionally (E `0x001c2928`). That builds the
  structure's collision shape over the collision BSP (`0x001eed40`), then
  `0x001c3b20` reads `[sbsp+0x1e4]` (E `0x001c3b3e`) and passes it to the
  `hkMoppBvTreeShape` constructor `0x002df760`, which increments a
  reference count inside the code block (`0x003009b0`, `inc [code+0x24]`).
  The count at `+0x1e0` is never read. **An empty block faults at BSP
  activation.**
- Nothing builds MOPP code at run time: the XBE names `hkMoppBvTreeShape`
  but no MOPP compiler (absence of evidence, not proof).
- Code layout (E, `0x003008d0`, `0x00301720`): `+0x00` f32×3 offset,
  `+0x0c` f32 scale, `+0x20` u32 size including the 0x30-byte header (V),
  `+0x24` runtime reference count, bytecode from `+0x30`. Opcodes above `0x6b` assert; `0x00`
  returns at once; terminals `0x30..0x4f` emit key `op - 0x30` plus a
  running offset; a key of type 1 (`key >> 29`) is structure collision
  surface `key & 0xffff` (E `0x001ef3e0`).
- A one-byte program (`00`) would load, but no Havok body (vehicles,
  dropped weapons, grenades, crates) would ever touch the level: they
  fall through the floor. Bipeds use Halo's own collision BSP (Guess). That
  is a hack, so it isn't done. **A converted map needs MOPP code that
  emits every structure surface a query box overlaps**: an offline MOPP
  writer, built from the interpreter's opcodes (`0x00300740`,
  `0x00301610`, `0x003027b0`), whose output is checked by running the
  engine's own interpreter semantics in a test against the collision BSP.
  **Done:** `mopp::build_structure(&bsp)` returns the block's bytes
  ([mopp.md](../formats/mopp.md), proven on all 78 retail BSPs).
- `+0x204/+0x208` (a second MOPP, breakable surfaces, Guess) is optional:
  E `jle` at `0x001c293a`. `+0x1ec..+0x200` is the world box (min, max)
  for the Havok broadphase, always read (E `0x001c2da8..0x001c2e0b`).

### Everything else: minimal content for one cluster

| Field | Minimal content | Evidence |
|---|---|---|
| `+0x0c` collision materials (0x14) | One per Halo 1 material: `+0x08` i16 a valid globals material index, `+0x0a` -1 | Indexed by surface material unchecked (E `0x1840d1`, `0x1844b4`); `+0x10` datum dereferenced (E `0x355e0`) |
| `+0x14` collision BSP | The converted BSP (one element) | Element 0 used without a count (E `0x11c331`) |
| `+0x1c`, `+0x20`, `+0x1c0` f32 | 0.0 (vehicle height clamps and a radius, off at 0) | E `0xf020c`, `0xf0254`, `0xf0288` |
| `+0x2c` leaves (8) | One per collision leaf: cluster 0, no surface references | Indexed by leaf unchecked (E `0x11be9d` and 25 more) |
| `+0x34` world bounds | Real bounds | E `0xa5ed3` |
| `+0x54` visibility | One u32, bit 0 set | Bit matrix indexed unchecked (E `0x11c015..0x11c044`) |
| `+0x5c` portals | Empty | Count checked (E `0x210e27`) |
| `+0x64` (0x18) | Empty, with cluster `+0x70` = `+0x71` = 0xff | Seven sites check only 0xff (E `0xf8526`, ...) |
| `+0x84`, `+0x8c` weather | Empty; cluster `+0x76` = -1 | E `0x36cb8`, `0x385a3` |
| `+0x94` (0x24) | One zeroed element | Every retail map has exactly this; no reader found, written to match |
| `+0x148` ambience, `+0x150` reverb sound clusters (0x14) | One each: `+0x00` -1 (no palette entry), `+0x04` no portals, `+0x0c` every cluster (i16); every cluster's `+0x78` and `+0x7a` = 0 | `[sbsp+0x14c] + 0x14 * cluster[+0x78]` read unchecked every tick (E `0x18d7b4..0x18d7c0`; reverb `0x18dd10..0x18dd20`): empty faults at address 4 |
| `+0x9c` clusters (0xb0) | One: geometry header and raw block, bounds, `+0x6e` = `+0x6f` = -1, `+0x70` = `+0x71` = 0xff, `+0x72` = `+0x74` = `+0x76` = -1, `+0x7c` = -1 and `+0x7e` = 0, its blocks empty | E, see the reading notes |
| `+0xa4` materials (0x20) | One per part material: shader reference at `+0x08`/`+0x0c`, `+0x1c` = 0xff | Indexed unchecked (E `0x45dee..0x45e08`) |
| `+0xac`, `+0xb4`, `+0xc4` (pathfinding), `+0xd4`, `+0xdc`, `+0xf4`, `+0x138`, `+0x140`, `+0x154`, `+0x20c`, `+0x214`, `+0x22c` | Empty | Each count-checked (E `0x3eb4e`, `0x181ddd`, every `+0xc8` site, `0x11bf56`, `0x18ddf4`, `0x23aa7d`, `0x183f6a`, `0x1e1dd`, `0x184214`, `0x3bec6`, `0xbfd63`) |
| `+0x224` audibility (0x34) | One element: no doors, `+0x04`/`+0x08` distance range, `+0x1c` a deafening bit per cluster pair `i < j` (all clear), `+0x24` a distance byte per pair (all 0: near) | Count-checked only at E `0x221ac7`; the pair readers (E `0x249d60`, called every tick from `0x18cb77`, and `0x249c20`) fall back without it to the sound PAS data at `+0xe4`, empty in every retail map, and read its null pointer (E `0x249f03`) as soon as there are two clusters. Every retail MP map has the element, 0 doors, `n(n-1)/2` distance bytes |
| `+0x21c` | One zeroed element | Element 0 dereferenced unchecked (E `0x210f0f`) |
| `+0x234` | One zeroed element (size Guess `0x30`) | Read every frame, `[elem+0x20]` unchecked (E `0x3f927`) |
| `+0x1e0` | MOPP code (above) | |

**Lightmap (`ltmp`):** it must exist. The loader accepts a null one, but
the structure renderer reads `[ltmp+0x84]`, then `[group+0x2c][cluster*4]`,
with no check (E `0x4d3e7..0x4d406`). Minimal: one group whose render
info has one entry `{-1, -1}` for the cluster, a null bitmap reference
(the renderer falls back to a default texture, E `0x18d1f`), the group's
checksum `+0x04` equal to `sbsp+0x08`, `ltmp+0x34` = 0.0, every other
group block empty. Lighting from Halo 1's lightmap pages comes later. The
load itself validates only the region's `sbsp` FourCC (E `0x122b6d`); a
checksum mismatch only leaves the lightmap unused.

## Converting a map (**Built**: `h1convert::convert`, `h2 h1 convert`)

The library entry point, for the CLI and the editor:

```rust
let p = h1convert::convert::prepare(&h1, source_sha256, &mut base, Sandbox::Halo2)?;
// p: placements, structure counts, what wasn't placed (a dry run's report)
let done = h1convert::convert::convert(&h1, source_sha256, &mut base, Sandbox::Halo2)?;
// done.name ("h1_beavercreek"), done.bytes (the map), done.manifest
```

`convert(h1, source_sha256, base, sandbox)` takes the Halo 1 map's file
hash as well as the map (the `[origin]` below needs it, and `H1Map` holds
the inflated bytes, not the file). It is `prepare`, then the MOPP code
from `convert::structure_mopp`, then `Prepared::build`.
`structure_mopp` calls `mopp::build_structure(&bsp)`
([mopp.md](../formats/mopp.md)). `prepare` and `convert` with
`Sandbox::Halo1` return `Error::Halo1Sandbox`: the Halo 1 sandbox is
`convert::halo1::{prepare_halo1, convert_halo1}`, which take the objects'
converter (`h1objects::sandbox::Objects::new(&h1, &Base::read(&mut base)?)`;
below, "Halo 1 sandbox"). `h2 h1 convert` and the editor call it.

```sh
cargo h2 h1 convert disc/halo1.iso beavercreek --out ~/h2-h1maps [--sandbox h2] [--base beavercreek] [--disc disc/halo2.iso] [--force]
cargo h2 h1 convert path/to/beavercreek.map --dry-run
```

Multiplayer maps are written **fresh** by default (`--pinned` for the
old way, `--fresh` still accepted), and so does `h2 import halo1`: the same map with **nothing pinned**:
every tag is rebuilt as a tree, and the base's raw resources, sound
chunks and low-detail bitmaps are copied, so nothing depends on the base's
file layout. The scenario is built from its blocks. See
[tag-schema.md](../formats/tag-schema.md), "Fresh maps". The output path
and manifest are the same as without it:

```sh
cargo h2 h1 convert disc/halo1.iso bloodgulch --out ~/h2-h1maps [--pinned]
```

**Ambient sound (Built, `h1convert::ambience`, `h1sounds::looping`).**
Halo 1 keeps a level's background sounds and sound environments in the
structure (`sbsp +0x1fc`, `0x74` each, and `+0x208`, `0x50` each; a name
and the `lsnd` or `snde` at `+0x20`); Halo 2 in the scenario
(`scnr +0x250`, `0x64` each, and `+0x258`, `0x48` each). Each Halo 1
looping sound converts with every `snd!` it plays
(`h1sounds::looping::read_looping`, then `install_looping`, which adds
the sounds to the `ugh!` in one pass and reuses any tag already of that
path); each sound environment converts with its four intensities from
linear to decibels (`read_environment`). The base scenario is pinned, so
the entries are written into the base's own palette storage, as many as
it holds (Coagulation 3 and 4, Beaver Creek 2 and 3; every Halo 1
multiplayer level has 1 and 1), and the count shrinks to that. Halo 1 has
no cutoff distance or interpolation speed; the values written are ones
retail uses. Blood Gulch gets `sound\sfx\ambience\multiplayer\bloodgultch`
(its one track plays `countryday2`) and `mountains`. Clusters still name
neither palette (-1), so nothing plays until the structure writes the
indices (Halo 1's cluster `+0x04` and `+0x06`, `i16`: 0 on every
cluster of Blood Gulch and a10, whose palettes have one entry each; to
confirm). A gestalt that gains sounds is built, not pinned, and a fresh
build now copies its base chunks from the pinned file too
(`fresh::sound_gestalt`). `h1sounds/tests/looping_disc.rs`: all 734
looping sounds (829 tracks, 52 detail sounds, 1261 sounds) and 63 sound
environments on the disc convert.

**The base's unreachable tags are dropped (`fresh::prune`).** After the
fresh tree is built, the base's palette entries at `scnr+0xe0` (scenery),
`+0xf0` (lights) and `+0x330` (sound scenery) are nulled, and the
scenario's tag list at `+0x3d8` (4-byte datums) keeps only base tags
still reachable. Then every base tag nothing reaches from `scnr`, `matg`
or the kept roots loses its slot (the slot stays, empty, so indices
don't move). Roots kept whatever references them (`fresh::KEEP_GROUPS`):
`matg`, `scnr`, `ugh!`, `unic`, `mulg`, `stem`, `spas`, `vrtx`, `pmov`,
`sncl`, `snmx`, and every tag named under `shaders\` or `rasterizer\`
(`KEEP_PREFIXES`: a `stem` names `spas` passes by a word the tree doesn't
hold as a reference). No group the engine walks on its own is confirmed
in the XBE yet; the list is the conservative guess a game run tests.
Campaign levels use the same machinery with their own scenario
(`fresh::prune_with`, `fresh::Keep`; docs/game/campaigns.md §9).
Blood Gulch on Coagulation drops 227 tags (76 `bitm`, 44 `shad`, 14
`snd!`, 12 `mode`, 11 `hlmt`, 10 `lsnd`, 10 `scen`, ...), 13 palette
entries and 19 tag list entries; all 21580 references in the kept tags
still resolve (`tests/fresh.rs`). The scenery palette at `+0x58` is left
alone: the schema gives it 4-byte elements where the object types table
gives it `0x28`; settle that first. The dropped tags' raw data stays in
the file, unused.

**Then built from scratch (`h1convert::scratch`, converter version 28).**
The pruned set is a step, not the output: `fresh::from_scratch` reads
every tag by `(group, path)` through `tags::repo` (`scratch::LevelSource`:
the conversion's tags from the pinned map it wrote, the rest from the
retail base as it is on the disc), from the roots `matg`, the keep list
and every tag the built scenario and structure name, and lays them out on
a dense index of their own (`to_tag_set`); the scenario and structure are
the fresh build's, their datums renamed (`scratch::Transfer`). Equal
blocks are shared across tags, as retail does, except bitmaps and the
sound gestalt, which the readers that take one tag's bytes
(`tags::DecodedTag`, `SoundGestalt::decode`) need whole
(`scratch::SELF_CONTAINED`). If it fails, the pruned set is written
(`FreshReport::scratch_failed`). Blood Gulch on Coagulation: 4,385 tags
(4,333 retail, 49 converted, 3 scenario and structure), 4,385 index
slots, 0 dangling, 25.2 MB (pinned 48.6 MB); every tag but the sounds
identical to the pinned map's by name, address- and index-free, the
scenario's 20 blocks and root, every bitmap and the 9 languages' strings
the same; 21,570 references resolve (`tests/fresh.rs`). The sound gestalt
is cut to the 1,270 kept sounds (`h1convert::compact`; campaigns.md §9):
5,293 chunks to 5,054, 223 raw resources (8.1 MB) dropped, each sound
naming the same entries, import names and chunk bytes as in the pinned
map.

**The map's card is Halo 1's** (`h1convert::ui`). Halo 1's `ui.map` holds
the map select's pictures (`ui\shell\bitmaps\mp_map_grafix`, 14 images,
256x128), names (`ui\shell\main_menu\mp_map_list`) and descriptions
(`...\mp_map_select\map_data`), all in one list order (`ui::MP_ORDER`;
image 9 is Blood Gulch); each picture fills the top-left 139x113 of its
image. The converter crops it, writes it as `preview.png` in the package
and sets the manifest's `name`, `description` and `image`, which the map
list already shows for added maps (`boot::new_maps`). From a disc it reads
the disc's `ui.map`; from a `.map` file, a `ui.map` beside it, else the
card stays the base's. The campaign's (`ui\shell\bitmaps\sp_levels`,
`ui::campaign_card`) read the same way; the campaign picker's pictures are
bitmaps in `mainmenu.map` (content::campaign_list), not served yet.

The Halo 1 map is a `.map` file or a disc and a map name; the base comes
from the Halo 2 disc (`--disc`, else `$H2_DISC`, else `disc/halo2.iso`).
It prints the structure (clusters, vertices, triangles, materials), the
placements and what wasn't placed, then writes the package
`DIR/h1_<map>/`: `manifest.toml` and the variant's map at
`<sandbox>/h1_<map>.map` (`h2/h1_beavercreek.map`). A second run with the
other sandbox adds its variant to the same manifest. `--dry-run` does
everything but write and prints what it would write.

**Package.** One package per converted map at `<data>/maps/h1_<map>/`,
one `[[variant]]` per sandbox, each a whole map with its origin (the form
the editor reads, `editor_core::packages::halo1_origins`):

```toml
[package]
format = 1
name = "Battle Creek"

[base]
map = "h1_beavercreek"
derived = true

[list]
base = "beavercreek"

[[variant]]
file = "h2/h1_beavercreek.map"
sha256 = "<the map's SHA-256>"
origin = { game = "halo1", map = "beavercreek", sandbox = "h2", source_sha256 = "<the Halo 1 map file's SHA-256>" }

[[variant]]
file = "h1/h1_beavercreek.map"
sha256 = "..."
origin = { game = "halo1", map = "beavercreek", sandbox = "h1", source_sha256 = "..." }
```

`content::Manifest` holds them (`variants`, `content::Variant`,
`content::Origin`; `sha256` is ours beyond the editor's form, which
ignores it). A variant package is derived and has no `[[patch]]`;
sandboxes are `h1` or `h2`, one variant each. `Manifest::select_variant`
gives the plain derived manifest of one variant (its file as the one
layer), so each variant has its own `MapId` from its bytes, and `origin`
is in no `MapId`. The resolver (`content::Library`) loads the default
variant: `h1` if the package has it, else the first. Retail-style and
other manifests parse and resolve unchanged (the existing content
tests).

**The setting (Built):** `multiplayer.halo1_sandbox` in
`boot::setting_defs` (`HALO1_SANDBOX`: `h1` default or `h2`, `Restart`,
`H2_HALO1_SANDBOX`). `boot::new_maps::install` reads it and opens the
library with `content::Library::open_with_sandbox`, so the next boot
installs that variant of each converted map
(`new_maps::tests::a_converted_map_installs_the_sandbox_the_setting_names`).
Its row is "Halo 1 Maps" (Halo 1 Sandbox / Halo 2 Sandbox), the last on
our Video screen; a change says "(restart)" until the next launch
(`video_screen::tests::the_halo1_maps_row_picks_the_sandbox_for_the_next_launch`).
`h2 settings set multiplayer.halo1_sandbox h2` and `settings.toml` set it
too.

**One list ID per map (Built).** `content::Package::list_key` is
`Manifest::variants_key` for a package with variants (from its origin's
game and map), else its `MapId`; `boot::new_maps` derives list IDs from
it, so a converted map keeps its list ID and content folder whichever
sandbox is installed (`new_maps::tests::a_converted_map_installs_the_sandbox_the_setting_names`).

## Vehicles

Halo 2 spawns multiplayer vehicles from netgame equipment, not from the
scenario's vehicle block (which is empty on every retail map but
Headlong's seven placements with no palette). An entry names a vehicle
collection (`vehc`, `multiplayer\single_vehicles\<role>`: one entry, the
`vehi` and its model variant) and carries a type, the vehicle's class
(V, every retail and Map Pack map): 1 warthog and gauss warthog, 2 ghost,
3 wraith and scorpion, 4 banshee, 6 and 7 turrets. The spawner hands the
type to the game variant's vehicle options (`+0x209 + type - 1`, E
`0x0015a4d0`), which keep the vehicle, swap it for another of its class
or remove it; types 3 and 5 go to the heavy chooser (E `0x0015a250`:
wraith or scorpion, variant `multiplayer`). Retail maps also hold typed
entries **with no item**: a variant's vehicle option fills them. That is
how Coagulation gets a scorpion: it has the scorpion `vehi` and two empty
type-3 entries, but no scorpion collection.

**Every retail multiplayer map holds every vehicle tag** (banshee, ghost,
scorpion, spectre, warthog, wraith, both turrets). Only the collections
differ:

| Map | Vehicle collections |
|---|---|
| Coagulation | warthog, ghost, banshee |
| Waterworks | banshee, ghost, warthog, wraith, h_turret_ap |
| Headlong | warthog, warthog_gauss, ghost, h_turret_ap, banshee |
| Zanzibar | warthog_gauss, warthog, ghost, h_turret_ap |
| Burial Mounds | h_turret_ap, warthog, ghost |
| Ascension | banshee |
| Foundation | h_turret_ap |
| Beaver Creek, Lockout, Colossus, Ivory Tower (cyclotron), Midship | none |
| Map Pack: Containment | h_turret_ap, warthog_gauss, scorpion, banshee |
| Map Pack: Relic (dune) | warthog, ghost, h_turret_ap |
| Map Pack: Terminal (triplicate) | h_turret_ap, ghost, wraith, warthog_gauss |
| Map Pack: Sanctuary (deltatap) | h_turret_ap |
| Map Pack: Turf | warthog |
| Map Pack: Backwash, Elongation, Gemini, Warlock | none |

The Map Pack maps read with the same tools (`xiso` entries under
`/demos/dlc/0000000N/`); `h2 tags <map> vehi --disc` looks in `/maps`,
which the Map Pack disc doesn't have. Containment (snow, and the only map
with a scorpion collection) would suit Sidewinder or Ice Fields, but a
base must be on the player's disc, so the bases stay retail.

So a converted vehicle's collection is the base's when it has one, and
**otherwise a built one** for the base's own `vehi`, in the slot of a tag
the conversion frees (below): weight 1, the `vehi`, the variant
(`gauss` for the gauss warthog, `multiplayer` for the scorpion and wraith,
as retail collections name them; none otherwise), and the `+0x8` word of
one of the base's collections. Every netgame equipment entry now gets
its class's type (weapons 0, grenades 8, powerups 9, vehicles above);
before, every entry kept the base's first entry's type. The rocket
warthog becomes the gauss warthog (the user's decision). Blood Gulch and
Sidewinder place all 8 of their vehicles (2 warthogs, 4 ghosts, 2
scorpions; the Xbox maps have no rocket warthog).

**Added tags.** A built vehicle collection is added after the base's
tags (`TagSet::add_tag`, which keeps the pinned index's tail); its datum
is known when the placements are written (slot `n`, salt `0xe174 + n`,
`convert::halo1::added_datum`) and checked when the map is built. No base
tag gives up its slot (`tests/disc_maps.rs` checks every built collection
and the index size). The converted lightmap still takes the base's own
lightmap bitmap's slot: it replaces that bitmap, whose pixels would
otherwise stay in the map unused. (`h1convert::slots::spare`, the freed
slots the collections used before, is no longer called.)

## What isn't placed, and why

Every placement the Halo 2 sandbox leaves out is one of these (the Halo 1
sandbox leaves out race flags only), and `h2 h1 convert` lists each with
its position (`not placed: ...`):

| Left out | Why |
|---|---|
| Race flags (`RaceTrack`, `RaceVehicle`) | Halo 2 has no race game type |
| Health packs | The base has no health pickup; the Halo 1 sandbox places Halo 1's |
| Active camouflage | The base has no collection for it (`invisibility`; Beaver Creek and Coagulation have none). A built `itmc` would do, as for vehicles |
| Overshield | The base has no collection for it (`super_shield`; Coagulation has none) |

All 13 multiplayer maps on the Halo 1 disc convert
(`tests/disc_maps.rs`, `H1_ISO` and `H2_DISC`; 5 minutes in a debug
build), each map reads back with a valid checksum, and its bitmaps are
the base's, pixel for pixel, but for the lightmap. The test also checks,
on every map: each drawn part's material names the shader `shaders::pick`
chose (or, with Halo 1's textures, its copy: same template, the same
bitmaps but each swapped role's, which is the Halo 1 bitmap), none of those
shaders' bitmaps is an `unwrapped` one; every added Halo 1 bitmap decodes
(Halo 2's decoder, the converted map's own raw data) to the pixels
`h1bitmaps` decodes from Halo 1, every image (Halo 1 bitmaps / shader
copies / shaders left on the base's: beavercreek 25/14/4, bloodgulch
18/10/7, boardingaction 17/10/4, carousel 17/8/10, chillout 24/15/7,
damnation 21/12/6, hangemhigh 21/9/1, longest 21/9/5, prisoner 23/10/8,
putput 16/11/2, ratrace 29/13/6, sidewinder 24/12/5, wizard 14/7/3; the
whole test now takes 14 minutes in a debug build); the texture
and lightmap coordinates read back as written; the lightmap names the
converted bitmap, every lit cluster's page is one of its images, and each
page decodes (Halo 2's decoder, the converted map's own raw data) to the
Halo 1 page's pixels. Lightmap pages (clusters): beavercreek 13 (14),
bloodgulch 15 (16), boardingaction 25 (26), carousel 28 (29), chillout 9
(10), damnation 10 (11), hangemhigh 2 (3), longest 1 (2), prisoner 2 (3),
putput 9 (10), ratrace 11 (12), sidewinder 23 (24), wizard 11 (12); the
extra cluster holds the unlit surfaces. Per map (placed / Halo 1; items include
vehicles, which Halo 2 places as items):

| Map | Base | Triangles | Spawns | Flags | Items | Vehicles | Left out |
|---|---|---|---|---|---|---|---|
| beavercreek | beavercreek | 3317 | 70/70 | 88/97 | 28/35 | 0/0 | 11 race, 4 health, 3 camo |
| bloodgulch | coagulation | 5503 | 71/71 | 172/191 | 39/43 | 8/8 | 21 race, 2 health, 1 camo, 1 overshield |
| boardingaction | beavercreek | 7142 | 73/73 | 66/77 | 43/52 | 0/0 | 13 race, 7 health, 2 camo |
| carousel | beavercreek | 2940 | 63/63 | 71/79 | 33/37 | 0/0 | 10 race, 1 health, 3 camo |
| chillout | beavercreek | 3108 | 58/58 | 39/49 | 27/33 | 0/0 | 12 race, 4 health, 2 camo |
| damnation | beavercreek | 8618 | 67/67 | 77/79 | 41/46 | 0/0 | 4 race, 4 health, 1 camo |
| hangemhigh | beavercreek | 3218 | 67/67 | 83/92 | 41/45 | 0/0 | 11 race, 2 health, 2 camo |
| longest | beavercreek | 2319 | 71/71 | 48/51 | 35/40 | 0/0 | 5 race, 4 health, 1 camo |
| prisoner | beavercreek | 5234 | 58/58 | 40/47 | 45/51 | 0/0 | 9 race, 5 health, 1 camo |
| putput | beavercreek | 10494 | 59/59 | 127/134 | 26/40 | 0/0 | 9 race, 13 health, 1 camo |
| ratrace | beavercreek | 2692 | 48/48 | 41/43 | 27/30 | 0/0 | 4 race, 2 health, 1 camo |
| sidewinder | coagulation | 10036 | 76/76 | 147/166 | 50/55 | 8/8 | 21 race, 2 health, 1 camo, 2 overshield |
| wizard | beavercreek | 2972 | 48/48 | 57/59 | 36/44 | 0/0 | 4 race, 4 health, 4 camo |

Flag counts don't add up one for one: a Halo 1 flag can become several
Halo 2 flags (`placements::h2_flags`). Danger Canyon, Death Island,
Gephyrophobia, Ice Fields, Infinity and Timberland aren't on the Xbox
disc.

## Still to do, in order

1. ~~**MOPP writer**~~: done, `mopp::build_structure`
   ([mopp.md](../formats/mopp.md)), called from `convert::structure_mopp`.
2. ~~**Decals and lights** of the base~~: lights emptied, no placed
   decals (above); the base's unreachable tags are dropped in fresh
   builds (`fresh::prune`, above); Halo 1's background sounds and sound
   environments fill the scenario's palettes ("Ambient sound"). Left:
   each cluster's palette indices (the structure, with the render
   lane's next round), `h1campaign` calling `h1sounds::looping` for its
   scripts' `looping_sound` references, and the scenery palette at
   `+0x58`.
3. ~~**Lightmaps**~~ and ~~**Halo 1's own textures**~~ on structure
   `senv`/`swat` shaders: done (above), and object and scenery models
   (item 6). Next: Halo 1's tiling scales.
   What the disc shows (`structure_bsp_root_blocks_on_the_disc`,
   all 12 multiplayer maps; element sizes are upper bounds from block
   spacing, not yet engine-backed): every map uses the root blocks at
   `+0x0c` (collision materials, 20), `+0x14` (collision BSP), `+0x2c`
   (8 bytes, **one per collision leaf** on every map: V), `+0x4c` (8),
   `+0x54` (1), `+0x5c`, `+0x94` (36), `+0x9c` (clusters, `0xb0`),
   `+0xa4` (materials, 32), `+0xac` (2), `+0xd4` (100), `+0xdc` (72),
   `+0x138`/`+0x140` (instanced geometry, `0xc8`/`0x58`), `+0x148`,
   `+0x150`, `+0x1d0` (one element), `+0x1e0` (bytes, up to 160 KB: a
   likely Havok MOPP code block), `+0x21c`, `+0x224`, `+0x234` (one
   element each). The rest are used by some maps only. The converter
   writes `+0x4c`, `+0x94`, `+0x148` and `+0x1d0` empty without an engine
   reading that allows it (above).
4. **Precision variants** at round start (the AR rule above; not built).
   What a hook would read: the game variant at `[game globals]+0x8+0x134`
   (`live_telemetry` layout), its starting weapon byte at `+0xd6`
   (`variants::record`, setting 89; battle rifle 3, sniper rifle 10,
   carbine 13 as `community_variants::Weapon` numbers them). Still open:
   how the hook knows which spawns came from assault rifles without tag
   names in memory, and whether the spawner has rolled them by the game
   start.
5. **The setting and the map list** (below): built, with its row.
6. **Halo 1 sandbox**: objects, the cyborg as the player and the health
   model per variant are built (below), and Halo 1's skies. Next a HUD
   with the health meter. Halo 1's textures on the converted object
   models are built (below). Neither the structure nor converted models
   list predicted resources yet; whether the copies' bitmaps need to be
   listed is for the render lane's next round and a game run.

## Importing (**Built**: `cli::h1_import`)

Every Halo 1 multiplayer map on the disc, in both sandboxes, is converted
by `h2 import halo1`, and by `h2 play` itself when the disc is there:

1. Put your Halo: Combat Evolved disc image next to the Halo 2 one as
   `halo1.iso` (`disc/halo1.iso` beside `disc/halo2.iso`).
2. `cargo h2 play disc/halo2.iso`. The first start converts the disc's
   maps (and the campaign), a line each: `halo1: [n/N] multiplayer
   bloodgulch (Halo 1 sandbox): done (<seconds> s)`.
3. The maps are `h1_<map>` in the multiplayer map list, under Halo 1's
   names (Blood Gulch, Hang 'Em High, ...). The
   `multiplayer.halo1_sandbox` setting picks which variant plays (Halo 1's
   objects by default); it applies at the next start.

Each map is `<data>/maps/h1_<map>/` with `h2/h1_<map>.map`,
`h1/h1_<map>.map` and one `manifest.toml` listing both, on the map's
default base (`MAPS`), pinned. When it converts again, the stamp, the
converter version to bump (`CONVERTER_VERSION`, `crates/cli/src/h1_import.rs`),
`--no-halo1` / `H2_HALO1=0` and the tests are in
[campaigns.md](campaigns.md) §10. `h2 h1 convert` still converts one map
by hand, into any `--out`.

## The setting and the map list (design)

- **Row:** `multiplayer.halo1_sandbox`, a choice `halo1` (default) /
  `halo2`, one `settings::Def` in `boot::setting_defs` with a row on the
  multiplayer settings screen, `Restart` (it applies when maps are
  installed at boot), per [settings-menu.md](settings-menu.md).
- **Packaging:** each converted map is a new map in the sense of
  [maps.md](maps.md) §4: Halo 1's maps aren't on the Halo 2 disc, so
  `boot::new_maps` installs each as a content package with its
  `patch.lvl` record, and 1.0 lists it. One package per map holding both
  variants, each a `[[variant]]` with its `origin` (above).
- **Where the choice happens:** in `boot::new_maps`, which already writes
  each new map's `.map` into its content directory at boot. It writes the
  variant the setting names, through the resolver's variant selection
  (above). That is the one place the game's map file is chosen for a new
  map.
- **Names (decided):** converted maps are `h1_<map>` (`h1_beavercreek`:
  Halo 2 has its own `beavercreek`), with Halo 1's display name in the
  list.
- **Identity (built):** one list ID per Halo 1 map, whichever
  variant (`Package::list_key`, above); the `MapId` (content hash)
  differs per variant; online, the host's variant is used.

## Halo 1 sandbox (**Built**: `convert::halo1`, `h1objects::sandbox`)

`h2 h1 convert --sandbox h1` builds the same structure, spawns and flags
as the Halo 2 variant, and places every Halo 1 item and vehicle spawn as
the Halo 1 object itself: `h1objects` converts each `weap`, `eqip` and
`vehi` (and what they reference: `proj`, models, collision, animations)
onto a Halo 2 template ([h1-objects.md](../formats/h1-objects.md)), and
the conversion adds the tags to the pinned base after its own slots.
Nothing is left out but race flags. Slot `n` of a set made from a map gets
salt `0xe174 + n` (`TagSet::from_map`), so the spawns name each new tag's
datum before it exists (`convert::halo1::added_datum`), and the build
checks the datums came out so.

Each netgame equipment spawn names its object, not a collection. The
spawner rolls an `itmc` or `vehc` and places any other object tag as it
is (E `0x0015aad0`); a respawn time of 0 on a non-collection takes the
engine's default (E `0x0015ac83`). Type 0 (weapons) keeps a weapon the
globals' weapon list doesn't know (E `0x0015a640`: `0x0015eb80` returns
-1 and the item is kept); vehicle types 1 to 7 and the powerup types keep
the item while the variant leaves the map's default (E `0x0015a4d0`,
`0x0015a5b0`). Types: weapons 0; warthog 1, ghost 2, scorpion 3; grenades
8; camouflage, overshield and the health pack 9.

`h1objects/tests/sandbox_disc.rs` converts Blood Gulch onto Coagulation
and reads every netgame equipment spawn of the built map back: 43 of 43
(35 items, 8 vehicles), each naming a tag this conversion added, of its
group, with a render model (`tags::object::Context`). 68 tags are added:
8 weapons with their projectiles, camouflage, overshield, the health pack
(on the overshield template), both grenades, warthog, ghost and scorpion,
and the cyborg (`characters\cyborg_mp\cyborg_mp`, on `masterchief_mp`):
73 in all.

**The player is the cyborg (Built, `h1convert::globals`).** The engine
picks a player's unit in `0x0014f040` (E): the player's representation
byte (player `+0x88`), clamped to the globals' player representation
block (`matg +0x138`, elements of `0xbc`), and in multiplayer (game mode
2, `[0x4e6948]+8`) 0 becomes element 2 and 1 element 3; the unit is the
element's tag reference at `+0xb0` (datum `+0xb4`), with a word at `+0xb8`.
On the disc every multiplayer map's elements are none, none,
`masterchief_mp`, `elite_mp`; campaign maps fill element 0
(`masterchief`) or 1 (`dervish`) (`tests/h2_globals.rs`, both checks).
`globals::set_player_unit(set, biped, Players::Multiplayer)` points
elements 2 and 3 at the converted cyborg and clears the word;
`Players::Campaign` does elements 0 and 1, for `h1campaign`. The
sandbox's disc test checks both elements name the added cyborg, which
decodes with a render model. The first-person arms stay Halo 2's (element
`+0x0`).

**The health model per variant (Built, `boot::health`).** At each game
start the loaded map's name (`0x547898`) picks the model
(`health::model_for_map`): `h1_<map>` is Halo 1's when the converted
maps were installed in the Halo 1 sandbox (the boot reads
`multiplayer.halo1_sandbox` once, as `new_maps` does), Halo 2's in the
Halo 2 sandbox; Halo 1's campaign level names (`a10` to `d40`) are Halo
1's. `H2_HEALTH_MODEL` still overrides. The HUD meter is the window's
until a converted HUD has a body widget.

**Object and scenery textures (Built, `textures::ModelTemplate`).**
Each Halo 1 model shader gets a Halo 2 `shad` copy: the base's first
`objects\` shader on `shaders\shader_templates\opaque\tex_bump`
(by name, so the pick is stable), its base map swapped for the Halo 1
shader's base map, its detail for the Halo 1 detail map (else the base's
`default_detail`) and its bump for `default_vector` (Halo 1 model
shaders have no bump). The maps come from `textures::h1_maps_of`, which
reads `soso` (base `+0xa4`, detail `+0xdc`), `senv`, `swat`, `smet`,
`schi`, `sotr` and `sgla`. `h1objects` converts each map once
(`h1bitmaps`) and names the copies in the model's materials;
campaign scenery goes through `h1objects` too, so it gets them as well.
On Blood Gulch's sandbox 17 models are converted, 81 materials draw
Halo 1 textures and 5 name no Halo 1 shader (`tests/sandbox_disc.rs`).

### Design notes


`weap`, `proj`, `bipd`, `vehi`, `eqip` and their `mod2`/`coll`/`phys`,
`antr` and effects, converted per [campaigns.md](campaigns.md) §2, with
Halo 2's physics. The health subsystem of campaigns.md §6 is shared with
the campaign: no body regeneration, health packs refill on pickup, a
health meter under the shield meter, on for Halo 1 maps in this variant.
Hooks (docs/game/health.md): call `boot::health::set_mode(HealthModel::Halo1
| Halo2)` at every game start, and `boot::health::set_hud_has_meter(true)`
once a converted HUD (`nhdt`) has a body widget (input 17). A health pack
is an `eqip` with `+0x12c` = 5 (i16) and `+0x130` > 0; retail Halo 2
already handles that powerup type, so no hook is needed.

## What only a game run can prove

- that the engine loads a converted `sbsp`/`ltmp` and the player stands on
  the converted collision;
- that spawn points, flags and weapons land where the Halo 1 map had them;
- that the map is listed and starts from the multiplayer lobby.
