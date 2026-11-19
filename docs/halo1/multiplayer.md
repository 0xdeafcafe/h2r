# Halo 1 multiplayer maps in the Halo 2 engine (design)

Status: in progress, started 2026-11-16; converted maps confirmed in the
game by the owner (2026-11-17). Each section says what is
**Built** (with its proof) and what is design only. This applies the rules
of [campaign.md](campaign.md) to multiplayer: Halo 2's engine runs
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
    packs from the health subsystem ([campaign.md](campaign.md) §6).
- The import writes both variants of every map. The setting picks which one
  is served. It is chosen per game, on the pregame screen next to the map
  and game type options, and applies at the map's next load with no
  restart ("Serving the chosen sandbox" below).

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
([map-format.md](map-format.md) has every layout and its
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

**Placed scenery joins it** (converter version 92,
`h1convert::scenery_collision`). For each scenery placement there from the
start, its object's collision (`h1objects::coll::object_bsps`: each
collision node's BSP for permutation 0 of its region, the one a converted
placement draws, placed by the render model's default pose) is moved to
the placement (position, then yaw, pitch and roll as `Rz(yaw) ·
Ry(−pitch) · Rx(roll)`, the record's `+0x14` as it is drawn) and merged:

- **Moving a BSP.** Vertices and planes move rigidly. A 2D tree is written
  in its reference plane's projection (the dominant normal axis dropped,
  [collision.md](../formats/collision.md), "What the engine does with a
  BSP"), which a rotation changes; each 2D line is carried over exactly
  (new 2D point, lifted onto the moved plane, moved back, projected the old
  way: an affine map), so every 2D walk finds what it found before.
- **Merging.** The piece's planes, 2D nodes, surfaces, edges, vertices and
  references are appended (surfaces take collision material 0, every
  converted collision material being the base's one template; no
  breakable surface), and one copy of the piece's 3D tree. Then each of
  the level's open leaves the piece's solid reaches (inside its bounding
  box, its vertices' box 0.05 out) is replaced by six nodes testing that
  box, whose outside keeps the level's leaf and whose inside is the
  copy. Inside that leaf's cell the piece decides solid or open,
  everywhere else the level does: the union. The box keeps a piece whose
  tree calls unbounded space solid (an open mesh) inside it. Each open
  leaf of the copy carries, before its own references, those of every
  level leaf it stands in whose planes its cell there touches (cells are
  the box clipped by the level's path planes and the piece's, convex
  polyhedra in `f64`): a ray that leaves the piece's open space into the
  level's solid looks the surface up in the leaf it leaves, and the
  matcher tries each reference on the crossed plane, keeping a surface
  only when the point is on its polygon, so a reference a cell doesn't
  border costs a lookup and gives no false hit. A copied leaf is in the
  cluster of the first level leaf it stands in (`sbsp +0x2c`, the sound
  and visibility cluster). The level's own blocks keep their indices;
  nothing is renumbered. So a merge grows the level by one tree and six
  nodes per level leaf. Version 92 as first written copied the whole
  tree into every level leaf the box reached instead: Blood Gulch's
  12,071 nodes became 51,007 for 25 placements, past what the pinned
  build's BSP space holds.
- **Room.** What the merge adds is the pieces themselves: Boarding
  Action's 21 placements with collision take its collision BSP from
  311,492 bytes to 537,388, and its structure region past Beaver Creek's
  BSP space (its `sbsp` and `ltmp` are 655,344 bytes on the disc). A pinned map can't grow that space: every region loads
  at one base right under the metadata, and pinned metadata keeps its
  addresses. The fresh build doesn't need it to: it takes the converted
  BSP from the set, not from the pinned intermediate's bytes, and lays
  every tag out anew with a BSP space sized to its own region. So the
  intermediate it builds is built with `check_bsp_space` off
  (`Prepared::assemble`; [map-builder.md](../formats/map-builder.md),
  "Pinned maps"), and a `--pinned` Boarding Action is refused with
  "BSP space outgrew the pinned metadata base", as before.
- **Havok.** The structure's MOPP code is built over every structure
  surface after the merge (`structure_mopp`), so the rocks' surfaces are
  keyed with the level's.
- **Limits.** A merge that would pass what the packed fields hold (32,768
  planes and 2D nodes, 32,767 surfaces, 65,535 references, edges and
  vertices, 255 references a leaf) is refused whole, the level left as it
  was, and listed with the unresolved placements; that object stays
  walk-through.

Proof: `h1convert/src/scenery_collision_tests.rs` (invented: a moved box
hit on every face, 2D-split face included, after turns about all three
axes; a box merged into a floor-and-wall level is solid, its top hit from
above, the floor and the wall still hit from inside its gate box but off
it, rays away from it unchanged, the structure MOPP code returning it; two
overlapping boxes; a box in solid ground adding nothing; a merge that
doesn't fit changing nothing) and
`h1objects/tests/scenery_collision_disc.rs` (Blood Gulch on Coagulation:
a point inside `boulder_doublewide` that the level alone leaves open is
solid, a ray from above stops on the rock's top where the rock alone is
hit and not on the ground, the MOPP code returns that surface there, the
ground is found as before around the rock, and a 121 by 121 grid of
vertical rays away from every placed object meets the level exactly as
before; every multiplayer map merges every placement, validates (the level's own
surfaces may hold material `0xffff`, none, as retail structure BSPs do),
and keys every surface in its MOPP code; each prints its node growth and
collision BSP bytes before and after; Boarding Action, merged again
piece by piece, grows by each piece's own blocks once, six gate planes a
piece, six gate nodes a level leaf it is put in and the references its
copied leaves carry, and by nothing else).

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
`h2_equivalent_precision` gives the precision item. See "Precision
variants (**Built**)" below for how the choice is made at game start.

### Precision variants (**Built**: `apply::precision_swaps`, `boot::precision`)

- **The converter** places the default (the SMG) and records each swap
  in the package manifest, on the Halo 2 sandbox variant:
  `precision = [{ from = <SMG itmc datum>, to = <BR itmc datum> }]`. A
  row is recorded only when its default item comes from that row alone
  (`sandbox::default_sources`), so every netgame item naming `from` came
  from an assault rifle. The fresh build's prune keeps each `to`
  collection even when nothing places it. The swaps are not part of the
  `MapId`.
- **The hook** runs before the game activates (`0x18f000`, options in
  `eax`). The map's tags are loaded by then: `aim_assist` writes weapon
  tags at the same point. No item has spawned yet, because the netgame
  equipment spawner `0x15aad0` has one caller, the per-tick update
  `0x15ae70`. That is called only from `0x15c189` in `0x15c150`, which
  the game tick calls at `0x13803c`.
- **What the hook reads:**
  - the options' game type (`+0`, 2 multiplayer);
  - the starting weapon, byte `+0x134+0xd6` (battle rifle 3, sniper
    rifle 10, carbine 13);
  - the loaded map's name (`0x547898`);
  - the scenario (`0x4e0350`), whose netgame equipment is at `+0x120`
    (count, address; `0x90` an element). Each element's item reference
    is at `+0x58`: the group, then the datum at `+0x5c`.
- **What the hook writes:**
  - in a precision variant, every item naming `from` names `to`;
  - in any other variant, `to` goes back to `from`, because the scenario
    stays in memory between games on the same map;
  - the write happens in guest memory only, and only when the target is
    a loaded tag of the item's group (`tag_get_checked`).
- **The swap table** is read at boot from `<data>/maps/*/manifest.toml`
  (the `h2` variant of each package), keyed by `base.map`. The hook is
  idle in the Halo 1 sandbox, which places Halo 1's assault rifle.

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

The names of Halo 2's netgame flag kind values (`H2Flag::raw`) are
unverified; the disc's values fit them (`h1convert/tests/h2_scenario.rs`:
0 and 1 once per team on all 12 maps, 4 always neutral, 7 and 8 in equal
numbers, 10 to 16 runs of boundary points); not yet confirmed from engine
code.

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
  needed flipping. Each mesh is split by the Halo 1 cluster its surfaces
  are in, and each Halo 1 cluster becomes one section, its cluster's
  geometry (see **Clusters and portals**; at most 65535 vertices and
  indices, the section's `u16` counts, which no Halo 1 cluster comes near:
  the largest has 28,398 vertices). Streams written:
  `01` position, `18` texture coordinate, `1b` frame, `1f` lightmap
  coordinate, `30` incident light direction (Halo 1's lightmap vertex
  direction, the same 11:11:10 packing; the normal where a vertex has
  none). One subpart per part.
- **Every render field retail fills** (docs/formats/structure-bsp.md,
  "Geometry header", "Section struct"). Parts: flags `0xb`, as on all 2,450
  retail multiplayer parts, and the kind retail gives the template of
  the material's shader (`structure::part_kind`): 4 for every template
  under `shader_templates\transparent\` and for `water_pool`, 3 for
  `decal_simple` and `overlay`, 2 for every other (alpha-tested ones
  included: 155 of 157 retail `tex_bump_alpha_test_single_pass` parts
  are kind 2); the bounding sphere (`+0x10` centre of the box, `+0x2c`
  farthest vertex from it, at least 1 mm) and the box (`+0x30`). Cluster
  section info: shadow-casting triangles and parts are kind 2's, opaque
  parts kind 2 and 3's, as on every retail cluster with a kind 3 or 4
  part (961); rigid counts as the shadow-casting; compression flags 4 (the
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
  map differs, or if a converted section's code and table are not laid
  out as the pass reads them. `h1campaign/tests/disc.rs`
  (`every_campaign_level_s_clusters_are_drawn_as_the_engine_reads_them`)
  checks the same layout, the mask count and every predicted resource on
  all ten converted campaign levels, and their ambient sound: each
  structure's palettes name the scenario's, every cluster's indices
  and the scenario's cluster data agree, and every collision leaf plays
  its Halo 1 cluster's pair (all 700,932 leaves).
- **Bitmap images say "not cached".** Every converted image (Halo 1's
  textures, its lightmap) wrote 0 at `+0x20..+0x33`; every retail image
  has `0xffffffff` there: LOD pointers 1 and 2, and the texture cache's
  handle per LOD (`+0x28`, `+0x2c`, `+0x30`). The cache reads a handle
  other than -1 as "resident in that slot" and never loads the pixels
  (E `0x0012c9fb`), so every converted texture drew whatever cache slot
  0 held: the level's one flat colour. `h1bitmaps` now writes -1 there
  (`IMAGE_NONE_WORDS`).
- **Clusters and portals** (`h1convert::clusters`). Halo 2 draws a
  structure by flooding portals from the camera's cluster (E
  `0x00173130`, `0x00172840`): from a cluster it walks the portal list at
  `+0x8c` (u16), takes each portal (`sbsp +0x5c`, `0x24` each) whose
  plane faces the right way, which is open and whose screen rectangle
  overlaps the current one, and draws the cluster on its far side. Until
  converter version 40 there were no portals and every collision leaf
  was in cluster 0, so only cluster 0 was drawn: Blood Gulch's capture
  had one structure draw, cluster 0's cliffs, and no ground, and sky
  showed where the level should be. Now Halo 1's clusters, portals and
  leaves are kept. Halo 1's portal (`sbsp +0x154`, `0x40` each) has
  Halo 2's fields in Halo 2's order: `+0x00` the cluster behind the
  plane, `+0x02` the one in front, `+0x04` the collision plane, `+0x08`
  centre, `+0x14` radius, and its polygon (`+0x34` in Halo 1, `+0x1c` in
  Halo 2, 12 bytes a vertex). Both games put the cluster at `+0x00` behind
  the plane (V, by which side of the plane each cluster's centre is: 85 of
  Coagulation's 95 and 100 of Zanzibar's 125 portals, the rest ambiguous,
  none the other way round; on Halo 1's disc 31 portals come out the other
  way against thousands this way, Guess: clusters whose centre lies
  outside them). Flags (`+0x18`) are 0: bit 3 starts a portal closed (E
  `0x00210e43`). The converted collision BSP keeps Halo 1's planes in
  order, so the plane index carries over. A cluster's portals are its
  Halo 1 list (`+0x5c`, i16). Every surface goes to each Halo 1 cluster
  whose subclusters (`+0x34`, `0x24` each, a surface index block at
  `+0x18`) list it, and a handful (37 at most, b40) are in none, which
  Halo 1 never draws either. A surface that crosses a portal is listed by
  every cluster it is in (760 of Blood Gulch's 5503): Halo 1 draws it
  when any of them is seen, Halo 2 draws a cluster's section only when
  the flood reaches that cluster. Until converter version 65 such a
  surface went to its first cluster only, and looking from another of
  its clusters without seeing the first, it vanished: the culling the
  owner saw on Blood Gulch at version 60. `h1convert::flood` models the
  flood (one node per chain of portals, at most 256, E `0x001729d9`; the
  camera against the portal's collision plane within 1/64, E
  `0x00173890`; the portal's screen polygon clipped to the node's, E
  `0x00173380`) and `tests/portal_flood.rs`
  (`bloodgulch_surfaces_in_two_clusters`) proves it from every spawn
  turning through 360°: of the surfaces seen (centre in the frustum, the
  sight line clear of solid), drawn with the first cluster 189 went
  missing in 157 of 2556 views, drawn with each none. The same test
  finds the portals sound: all 70 name the cluster behind their plane
  first (the collision BSP's leaf 5 cm either side of each centre), the
  flood never comes near its 256-node limit (94 at most) and reaches
  every cluster a seen surface is in. Each collision leaf names its Halo 1 cluster (leaf
  `+0x08`). `tests/h1_clusters.rs`: every portal lies in its plane and is
  on both its clusters' lists, and every cluster with geometry is reached
  through portals from every other, on every level (Chiron TL-34 has no
  portals at all: its rooms are sealed, and Halo 1 draws them one at a
  time too). The visibility rows (`+0x54`) still say every cluster sees
  every other.
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
  Tiling: the copy's instance vectors that scale a swapped role's
  texture coordinates take Halo 1's tiling, the base map 1× (Halo 1's
  base map has no scale; Coagulation's ground tiled it 60×), the detail
  and bump maps the `senv` scale before each reference (primary detail
  `+0xb4`, micro `+0xf8`, bump `+0x124`; V: 1 wherever the map is null).
  A role's vector is the one the template binds to vertex constant
  `c[18 + stage]` with its x and y (`textures::stage_vector`); the disc
  test resolves every copy on the converted map and finds each scale in
  that constant. A role left as the base's (its bump map, when Halo 1's
  isn't `P8Bump`) keeps the base's tiling.
  Left out: the detail blend function, and the shader's `+0x2c` block (it still
  names the chosen shader's bitmaps; the draw path reads the
  parameters). On blend-detail templates (`tex_bump_detail_blend_detail`,
  Coagulation's ground) stage 1 of the first pass is the second blended
  base (`coag_grass`), so Halo 1's detail map lands there: a game run
  shows whether that reads well.
- **The sky is Halo 1's** (`h1convert::sky`). Cluster `+0x6e` is the
  cluster's sky index into the scenario's skies (`scnr+0x8`, 8-byte
  references; E `0x0014b38d`..`0x0014b3c4`, where -1 also falls back to
  the first sky when the scenario has one). The engine draws one sky,
  the camera's cluster's (E `0x0002b865` calls `0x0014b360`, which reads
  that cluster's `+0x6e`, and passes its index to the sky draw
  `0x0003ede0` through `0x004b9eec`); Halo 1 names a sky per cluster
  (`+0x00`, i16, -1 none). Each converted cluster keeps Halo 1's, and a
  cluster with none (an interior) takes the nearest cluster's through
  portals, so a window still shows the sky outside
  (`clusters::cluster_skies`). A cluster no sky reaches names none
  (`0xff`): `0x0014b360` turns -1 into sky 0 only when the scenario's
  `+0x12` has bit 1 and it has a sky (`0x0014b399`..`0x0014b3a6`), and
  converted scenarios leave the bit clear, so the engine draws no sky
  there, as Halo 1 doesn't. Until converter version 65 every cluster
  named sky 0; until 81 so did every cluster no sky reaches, and a10's
  cryo bay (`a10a`, 33 clusters, none with a sky or a portal to one)
  drew `sky\space\space` as a flat grey slab over two thirds of the view
  (proven live: the camera's cluster's `+0x6e` set to `0xff`, the bay
  drew whole and the sky's draw left the frame). Each
  sky the Halo 1 scenario names is converted by `h1sky` (model, lights,
  fog colour, and a model scale that keeps it behind the level;
  docs/halo1/sky.md) and added as a new tag, and every
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
  of the positions), the bitmap reference `+0x18` naming the converted
  lightmap bitmap, and per cluster a render info `{page, -1}` (`{-1, -1}`
  for unlit clusters). A Halo 2 cluster samples one page, its render
  info's; a Halo 1 cluster's surfaces sample several (22 of Blood Gulch's
  30 clusters use more than one), and a page serves several clusters. So
  each lit cluster gets a page of its own (`clusters::lightmap_pages`,
  `h1bitmaps::atlas`): its triangles fall into charts (triangles sharing
  vertices), each chart takes the texels bilinear filtering reads under
  its lightmap coordinates on its Halo 1 page, charts of one page that
  overlap or touch are merged so no texel is taken twice, and the pieces,
  each with a one-texel border repeating its edge, are packed by shelves
  into the smallest power-of-two page. The lightmap coordinates move with
  them. Surfaces Halo 1 doesn't lightmap (lights, glass, water) sample a
  white piece. The pages keep Halo 1's format (R5G6B5) and swizzling,
  at half Halo 1's light (`h1bitmaps::atlas::to_halo2_light`): Halo 2's
  lighting pass draws `2 × page × albedo`
  (`tests/lightmap_light.rs`, the pass run on the CPU for every retail
  `tex_bump` and `tex_bump_detail_blend_detail` on Coagulation), and a
  Halo 1 page is the albedo's multiplier itself. Blood Gulch and
  Coagulation, one valley with one ground albedo, show it: half of
  Blood Gulch's texels are at full light in their brightest channel,
  99 % of Coagulation's at most 0.85. Taken as they were, the ×2 drew
  Blood Gulch's ground saturated yellow and its cliffs bright orange.
  `tests/h1_clusters.rs` samples every lit vertex on both bitmaps
  (bilinear, clamped) on every level: all agree with half Halo 1's
  light within half a 5-bit step. The
  pages hold 1.6 to 3.9 times Halo 1's texels (power-of-two padding and
  the borders): Blood Gulch 220,160 for 126,016, c20's largest structure
  7.7 million for 2.9 million. The bitmap takes the
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

  **Every converted image is a cache resource (`0x200`); the dark
  lightmap.** a10 at version 14 drew without light. A capture of a10
  (frame 2806) showed why: the structure's lighting pass (pixel shader:
  `r0 = t3 · (1 + v1.a)`, then `× t0 × 2`, `t0` the screen copy of the
  albedo pass) had nothing bound at stage 3, and no R5G6B5 texture was
  created in the whole run. The binding itself was right (group, checksum
  and page all pass `0x00018bfe`); the texture cache returned null. It
  loads an image only when its flags have `0x200` (E `0x0012cd26`,
  `0x0012ce81`: `test byte [image+0Fh], 2`); otherwise it hands back the
  image's runtime texture (`+0x50`), never set for these. Every retail
  image has the bit (all 1,686 of Coagulation's and 2,231 of Delta
  Approach's: `0x203` DXT, `0x1209` swizzled, `0x290` linear) and no Halo
  1 image does. `h1bitmaps` now sets it on every converted image
  (`IMAGE_CACHED`), which covers the lightmap pages, Halo 1's textures on
  structure and objects, skies and the map card. Both disc tests check
  every image in every converted map (`check_images`).

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
| `+0x2c` leaves (8) | One per collision leaf: its Halo 1 cluster, no surface references | Indexed by leaf unchecked (E `0x11be9d` and 25 more) |
| `+0x34` world bounds | Real bounds | E `0xa5ed3` |
| `+0x54` visibility | One u32, bit 0 set | Bit matrix indexed unchecked (E `0x11c015..0x11c044`) |
| `+0x5c` portals (0x24) | Halo 1's (see **Clusters and portals**), at most 512 | Count checked (E `0x210e27`); the open-portal mask is 16 words (E `0x210df7`) |
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
(its one track plays `countryday2`) and `mountains`. Each cluster names
its sound (`clusters::sound_zones`). Halo 1's cluster keeps its
background sound and sound environment at `+0x04` and `+0x06` (`i16`
indices into the structure's palettes, -1 none; clusters at
`sbsp +0x134`, `0x68` each), and each structure leaf (`+0xe0`, `0x10`
each, one per collision leaf) its cluster at `+0x08`
(`h1convert/tests/h1_clusters.rs` checks both on every level). Halo 2
reads the listener's cluster (its collision leaf's) and that cluster's
`+0x72` and `+0x74`, indices into the structure's own copies of the
palettes (`sbsp +0xd4` and `+0xdc`, which equal the scenario's on all
160 retail structures; [structure-bsp.md](../formats/structure-bsp.md)).
The converter maps each Halo 1 pair through the written palettes
(`Ambience::index`; anything not written, or past the engine's 64, is
-1); each converted cluster is its Halo 1 cluster and plays its pair,
and each collision leaf names its Halo 1 cluster
(`clusters::sound_zones`). `check_sound_clusters` checks the ranges on
every converted map. A gestalt that gains sounds is built, not pinned, and a fresh
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
(`fresh::prune_with`, `fresh::Keep`; docs/halo1/campaign.md §9).
Blood Gulch on Coagulation drops 253 tags (85 `bitm`, 50 `shad`, 15
`snd!`, 14 `mode`, 13 `hlmt`, 12 `scen`, 11 `lsnd`, ...), 25 palette
entries and 21 tag list entries; all 21471 references in the kept tags
still resolve (`tests/fresh.rs`, converter version 17). The scenery palette at `+0x58` is
pruned like the others (converter version 17): its elements are `0x28`
bytes, a reference at `+0`, which the XBE settles
(`docs/formats/tag-schema.md`, "Engine strides"). The dropped tags' raw
data stays in the file, unused.

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
is cut to the 1,270 kept sounds (`h1convert::compact`; campaign.md §9):
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
`boot::setting_defs` (`HALO1_SANDBOX`: `h1` default or `h2`, `Live`,
`H2_HALO1_SANDBOX`). `boot::new_maps::install` reads it at boot and
writes that variant of each converted map into its content directory
(`new_maps::tests::a_converted_map_installs_the_sandbox_the_setting_names`);
a change after boot reaches `boot::h1_variants` through the setting's
change callback (`h1_options::install_switches`) and counts from the
map's next load (below). It is not on the Settings or Video screens: it
is chosen on the Halo 1 options screen ("Halo 1 options" below).
`h2 settings set multiplayer.halo1_sandbox h2` and `settings.toml` set it
too.

## Halo 1 options (**Built**, tests only: `boot::h1_options`, `boot::h1_options_row`)

The per-game Halo 1 or Halo 2 choices, as rows of one list
(`h1_options::OptionRow`: a two-choice setting, its label and what each
choice means). Adding an option is a setting in `setting_defs` (choices
1 Halo 1, 2 Halo 2), a row, and a switch in `h1_options::switch`.

| Row | Setting | Where | Switch |
|---|---|---|---|
| Sandbox | `multiplayer.halo1_sandbox` (`H2_HALO1_SANDBOX`) | multiplayer | `h1_variants::set_mp_sandbox` |
| Assets | `campaign.halo1_assets` (`H2_HALO1_ASSETS`) | campaign | `h1_variants::set_campaign_assets` |
| HUD | `halo1.hud` (`H2_HALO1_HUD`) | both | `hud::set_choice` |

- **Multiplayer:** the pregame Game Setup dialog (squad settings, screen
  23; list ctor `0x2bb4d2`, row text `0x2bb74f`, row chosen `0x2bb801`)
  gets a "Halo 1 Options" row under Quick Options, above the "Switch
  To:" rows (Change Map 0, Change Rules 1, Quick Options 4, Switch To 6
  to 8, from the row text table `0x2bb74f`), when the lobby's map
  (`0x51098c`, set by `0x19a6f2`) is a converted map: its content
  package's `h2recomp-map.toml` names an `h1_` map
  (`h1_options_row::converted_map_name`). Only custom lobbies have Change
  Map (action 0), so only they get the row. Choosing it opens the
  options screen (`h1-options`, a list class of ours) on the dialog's own
  window and slot, the way the skulls row opened its page. The screen is
  the lobby's Quick Options inside and out (`h1_options::DIALOG`,
  docs/game/ui-builder.md "The Dialog kind"): its panel, its header
  ("HALO 1 OPTIONS"), its `setting_lists` list with each row's name left
  and its choice alone in the value column ("Sandbox  Halo 1"), the
  row's one-line help (`OptionRow::help`) in Quick Options' help line,
  and the legend in its footer. Left, right or A changes a row; B backs
  out.
- **Campaign:** a Halo 1 level's Game Options page (the skulls page after
  the difficulty, docs/game/skulls.md) lists Assets and HUD under Start
  Level, above the skulls, and opens even with no skull collected.
- Each choice is saved to `settings.toml` at once and applies from the
  next game or level load; an environment variable locks it ("locked").
- **HUD:** `boot::hud` is the switch only: each game start picks
  `HudStyle::Halo1` on a Halo 1 map (an `h1_` map or a Halo 1 campaign
  level) with Halo 1's chosen, else Halo 2's, and logs
  `hud: <map>: Halo 1 HUD`. `boot::hud_swap` draws it: at game
  activation it points the converted owners' HUD references at Halo 1's
  or Halo 2's `nhdt` by the same rule (docs/halo1/hud.md).

**Serving the chosen sandbox (Built, tests only).** The choice is
`boot::h1_variants` (`set_mp_sandbox`, set from the setting at boot and
by the pregame screen's row). 1.0 opens a new map from its content
package, `T:\$c\<offer>\h1_<map>.map` (map-pack.md "Loading"), which
`boot::new_maps` still writes once with the setting's variant. The install
also registers, for each converted map, that path and both variant files
(`new_maps::variant_pair`, `content_path`) in a `vfs::ContentVariants`
table, and the data partition is mounted through `vfs::VariantFiles`
over its host backend, which serves the registered path from the variant
chosen when it is opened. Nothing is copied when the choice changes.

- A file open now keeps its variant until its last handle closes
  (`opened`, `closed`).
- `map_changed` is true for a closed map last opened in the other
  variant. `boot::map_slots` reads content package slots too
  (`t:\$c\<offer>\<map>.map`, `map_slots::slot_file`), so the game's
  cached copy of the old variant (slot 3) is dropped once that slot isn't
  the current map's, and the next load copies the chosen one.
- `health::set_halo1_sandbox` follows the switch, so the health model and
  the precision hook (`boot::precision`) are the chosen sandbox's at the
  next game start.
- `map_id` of the path is the served variant's `MapId`. The map probe the
  Live advertisement and telemetry use asks for `d:\maps\<map>.map`, which
  a new map has none of, so they send no `MapId` for a converted map,
  before this change and after it. A guest loads its own setting's
  variant: the host's choice is not carried to guests (not solved).
- The other variant's file is served as the package has it; its hash is
  checked at import, not again when the choice switches to it.

Tests: `vfs` `variants::tests::the_served_variant_follows_the_choice_at_the_next_open`,
`boot` `new_maps::tests::a_converted_map_is_served_in_the_sandbox_chosen_per_game`
(the install's table through a whole `Vfs`),
`map_slots::tests::a_content_package_slot_names_its_file_on_the_data_partition`,
`h1_variants::tests`.

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
pixels cut for its cluster. Lightmap pages (clusters): beavercreek 25
(29), bloodgulch 30 (30), boardingaction 41 (41), carousel 36 (36),
chillout 10 (10), damnation 12 (12), hangemhigh 1 (1), longest 1 (1),
prisoner 1 (1), putput 9 (9), ratrace 14 (16), sidewinder 32 (35),
wizard 18 (18); a cluster with nothing lit has no page. Per map (placed / Halo 1; items include
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

**Weather is not converted.** Halo 1's rain and snow (`rain` particle
systems in the BSP's weather palette) have a Halo 2 home: every `weat`
reference on the disc is at `+0x20` of a `0x88`-byte weather palette
element, in the structure at `sbsp +0x84` (50 references) or the
scenario at `+0x260` (12), and a cluster picks its entry at `+0x76`
(`weather_systems_are_named_by_the_weather_palettes`,
`h1convert/tests/h2_scenario.rs`; E for the structure fields, table
above). What stops it: Halo 2's `weat` is opaque
([lighting.md](../formats/lighting.md): its particle simulation and
render blocks are unreviewed), so a Halo 1 system can't be written into
one, and pointing the palette at a retail `weat` would be Halo 2's
weather, not Halo 1's. The structure side is also the render lane's
(`structure.rs`). The palette stays empty and each cluster `-1`.

## Still to do, in order

1. ~~**MOPP writer**~~: done, `mopp::build_structure`
   ([mopp.md](../formats/mopp.md)), called from `convert::structure_mopp`.
2. ~~**Decals and lights** of the base~~: lights emptied, no placed
   decals (above); the base's unreachable tags are dropped in fresh
   builds (`fresh::prune`, above); Halo 1's background sounds and sound
   environments fill the scenario's palettes ("Ambient sound"). Each
   cluster names its pair (above), on campaign levels too, and
   `h1campaign` converts its scripts' `looping_sound` references with
   `h1sounds::looping` (campaign.md, "Music, ambience and the map file"). The
   scenery palette at `+0x58` is settled (`0x28`) and pruned.
3. ~~**Lightmaps**~~ and ~~**Halo 1's own textures**~~ on structure
   `senv`/`swat` shaders: done (above), and object and scenery models
   (item 6), with Halo 1's tiling scales on the structure's.
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
4. **Precision variants**: built (above). A game run should show the
   battle rifle on Blood Gulch's assault rifle spawns in a BR-start
   variant and the SMG in Slayer.
5. **The setting and the map list** (below): built, with its row.
6. **Halo 1 sandbox**: objects, the cyborg as the player and the health
   model per variant are built (below), and Halo 1's skies. Next a HUD
   with the health meter. Halo 1's textures on the converted object
   models are built (below). The structure lists predicted resources
   (above). Converted object models don't need them: prediction only
   prefetches. A model's geometry block is loaded when drawn
   (`0x0012de70` → `0x00223c70`, from the draw paths) and a bitmap image
   when sampled with no cache handle (`0x0012c9fb` → `0x0012d5a0`); the
   lists (`obje +0xb4`, read at `0x000bbe00`..`0x000bbeb6` →
   `0x0016e5e0`, type 2 a render model at `0x0016e950`) only start those
   loads early. A converted object keeps its template's list, which
   prefetches the template's model; the cost is a possible first-frame
   hitch, not a missing model.

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
   objects by default); the pregame screen changes it per game, from the
   map's next load.

Each map is `<data>/maps/h1_<map>/` with `h2/h1_<map>.map`,
`h1/h1_<map>.map` and one `manifest.toml` listing both, on the map's
default base (`MAPS`), pinned. When it converts again, the stamp, the
converter version to bump (`CONVERTER_VERSION`, `crates/cli/src/h1_import.rs`),
`--no-halo1` / `H2_HALO1=0` and the tests are in
[campaign.md](campaign.md) §10. `h2 h1 convert` still converts one map
by hand, into any `--out`.

## On the stats site and in the viewers (**Built**: `cli::live_halo1`)

`h2 live assets extract` (and `h2 play --live-local`) draws every converted
map, multiplayer and campaign, like a Halo 2 map: geometry, sky, the five
views and a preview, keyed by the name the game reports (`h1_<map>`, or
the campaign level's), so live games and recordings on them plot on their
map ([live-art.md](../live/live-art.md#halo-1-maps)). The manifest marks
them `"game": "halo1"`; the site labels them Halo 1, names them from the
manifest, and does not call a converted map "modded" or its geometry
changed. The native viewer (`viewer`, `bsp`, `shadowlab`) opens them by
the same name from the data root (`$H2_DATA`, else the default), or any
`.map` by path, with the Halo 2 disc's shared maps.

## The setting and the map list (design)

- **Row:** `multiplayer.halo1_sandbox`, a choice `halo1` (default) /
  `halo2`, one `settings::Def` in `boot::setting_defs`, on the Halo 1
  options screen from the Game Setup dialog ("Halo 1 options" above).
- **Packaging:** each converted map is a new map in the sense of
  [maps.md](../game/maps.md) §4: Halo 1's maps aren't on the Halo 2 disc, so
  `boot::new_maps` installs each as a content package with its
  `patch.lvl` record, and 1.0 lists it. One package per map holding both
  variants, each a `[[variant]]` with its `origin` (above).
- **Where the choice happens:** in `boot::new_maps`, which already writes
  each new map's list record into its content directory at boot, for
  the variant the setting names, through the resolver's variant
  selection (above). The VFS serves the same variant: `boot` mounts the
  package library with the same setting (`map_override::mount_with_hashes`),
  and a rescan keeps it (`Library::reopen`). Opened without the
  setting, the VFS served the default variant (Halo 1's) whichever
  variant the menu listed.
- **Names (decided):** converted maps are `h1_<map>` (`h1_beavercreek`:
  Halo 2 has its own `beavercreek`), with Halo 1's display name in the
  list.
- **In the map select (built):** a new map whose name starts `h1_` is
  listed under **Halo: CE** ([map-select.md](../game/map-select.md)).
- **Identity (built):** one list ID per Halo 1 map, whichever
  variant (`Package::list_key`, above); the `MapId` (content hash)
  differs per variant; online, the host's variant is used.

## Halo 1 sandbox (**Built**: `convert::halo1`, `h1objects::sandbox`)

`h2 h1 convert --sandbox h1` builds the same structure, spawns and flags
as the Halo 2 variant, and places every Halo 1 item and vehicle spawn as
the Halo 1 object itself: `h1objects` converts each `weap`, `eqip` and
`vehi` (and what they reference: `proj`, models, collision, animations)
onto a Halo 2 template ([objects.md](objects.md)), and
the conversion adds the tags to the pinned base after its own slots.
Nothing is left out but race flags. Slot `n` of a set made from a map gets
salt `0xe174 + n` (`TagSet::from_map`), so the spawns name each new tag's
datum before it exists (`convert::halo1::added_datum`), and the build
checks the datums came out so.

**Each netgame equipment spawn names a collection built from Halo 1's**
(converter version 52). The spawner `0x0015aad0` reads the entry's item
(`+0x58` group, `+0x5c` datum) and rolls it with `0x0019f1a0` only when
the group is `itmc` or `vehc` (E `0x0015ab12`..`0x0015ab23`); any other
group leaves the item -1, and `0x0015ab8b` returns without placing.
Through version 49 each Halo 1 sandbox spawn named its `weap`, `eqip` or
`vehi` directly (the doc then read `0x0015aad0` as placing any other tag
as it is), so **no weapon, grenade, powerup or vehicle ever spawned** in
the Halo 1 sandbox: the user's "no weapons and no vehicles in the world"
on Blood Gulch, from the first Halo 1 sandbox build. Now
`convert::halo1` builds one `itmc` per Halo 1 item collection a spawn
uses (each converted item at Halo 1's weight; Halo 1's respawn time as
the root `i16` at `+8`, which the spawner reads when the spawn's own
`+0xe` is 0, E `0x0015ac83`..`0x0015acb0`) and one `vehc` per Halo 1
vehicle, added right after the objects so the spawns name their datums.
`0x0019f1a0` returns a one-entry collection's item whatever its weight
(E `0x0019f227`), and rolls the weights otherwise. Types: weapons 0;
warthog 1, ghost 2, scorpion 3; grenades 8; camouflage, overshield and
the health pack 9. Type 0 keeps a weapon the globals' weapon list
doesn't know (E `0x0015a640`: `0x0015eb80` returns -1 and the item is
kept); vehicle types 1 to 7 and the powerup types keep the item while
the variant leaves the map's default (E `0x0015a4d0`, `0x0015a5b0`).

`h1objects/tests/sandbox_disc.rs` converts Blood Gulch onto Coagulation
and reads every netgame equipment spawn of the built map back: 43 of 43
(35 items, 8 vehicles), each naming an `itmc` or `vehc` this conversion
added, whose entries name added objects of the collection's kind, each
with a render model (`tags::object::Context`). 68 tags are added:
8 weapons with their projectiles, camouflage, overshield, the health pack
(on the overshield template), both grenades, warthog, ghost and scorpion,
and the cyborg (`characters\cyborg_mp\cyborg_mp`, on `masterchief_mp`):
73 in all.

**A fresh map rehomes the templates' blocks.** A converted object is its
template's root bytes with Halo 1's values written over, so every block
field it doesn't rebuild still holds the template's address in the
pinned map. A fresh map lays the base's tags out anew, so those
addresses hold other bytes there. In a vehicle that meant `+0x2f8`
(elements of `0x2a0`), whose element `+0x38` count the load-time loop
`0x205550` (from `0x11c8f3`, resource preparation `0x11c840`) runs over,
writing `0x90`-byte entries from element `+0x60` with room for 4. The
junk counts (`0x290000b`, `0x3f800000`, `0xffffffff` on Blood Gulch)
wrote past the element: pc `0x205574` on Blood Gulch and Sidewinder. The
cyborg (10 fields), its model (4) and every
weapon, projectile and powerup were stale too. `build_fresh` now walks
each built tag with the schema and copies every block field that still
holds a pinned metadata address as a tree of its own
(`Unpinner::rehome`, `FreshReport::rehomed`; refusals in
`FreshReport::stale`). `sandbox_disc.rs` checks every multiplayer map on
the disc in both sandboxes: every vehicle's `+0x2f8` counts are at most
4 and nothing is stale. It also checks that every starting location is
in open space with the converted floor within 2 units below.

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
decodes with a render model.

**First-person arms (converter version 54).** The element's `+0x0` is the
first-person arms model (`+0x8` the first-person body). The Halo 1
sandbox and the Halo 1 campaign put Halo 1's arms there
(`globals::set_player_hands`; the arms Halo 1's globals name at
`matg +0x17c`, element `+0x0`, `characters\cyborg\fp\fp` on every map,
converted on their own by `h1objects`). The engine binds the arms' nodes
and the weapon's first-person model's nodes to the weapon's first-person
graph **by name** (E `0x001dd4c0`, called from `0x001680eb` for the
weapon model and `0x00168083` for the arms): Halo 2's arms name no node
of a converted Halo 1 graph (`l_upperarm` against `frame l upperarm`), so
until version 54 every converted first-person graph drove the arms with
no node bound. Halo 2's arms stay in the Halo 2 sandbox. The first-person
body stays Halo 2's (Halo 1 has none; its nodes bind to no node of a
Halo 1 graph either).

**The player's camera and hands (converter version 52).** A rebuilt
biped names its model's Halo 1 markers in the unit's marker fields
(`h1objects::rebuild::UNIT_MARKERS`): the camera at `unit +0xd4`
(`head`), and the weapon markers at `+0x158`/`+0x15c` (`right hand`,
`left hand`), which `0x000d0870` passes to the attach `0x0010cec0` when
a unit takes a weapon. Halo 1's units hold no marker names (its engine
uses fixed ones) and the rebuild left all three empty, so the cyborg's
camera sat at its origin (the feet in view, the view low: the user's
report on Blood Gulch) and its weapons had no marker to attach to (the
player held nothing; the converted Blood Gulch had no item to pick up
either, above). The disc test checks the cyborg's three fields name
markers of its render model.

**The health model per variant (Built, `boot::health`).** At each game
start the loaded map's name (`0x547898`) picks the model
(`health::model_for_map`): `h1_<map>` is Halo 1's when the Halo 1
sandbox is chosen for the game (`boot::h1_variants::set_mp_sandbox`
keeps `health::set_halo1_sandbox` in step), Halo 2's in the
Halo 2 sandbox; Halo 1's campaign level names (`a10` to `d40`) are Halo
1's. `H2_HEALTH_MODEL` still overrides. The HUD meter is the window's
until a converted HUD has a body widget.

**Halo 1's scenery, in both sandboxes (Built, converter version 53:
`convert::halo1::plan_scenery`, `fresh::put_scenery`).** The trees,
rocks and shrubs of a Halo 1 map were never placed: the conversion
empties the base's placements and placed none of Halo 1's. Now each
Halo 1 scenery palette entry (`scnr +0x21c`, `0x30`) is converted
through `h1objects` (the `scen` from its definition) and every placement
(`+0x210`, `0x48`) on an entry that converted is placed: Halo 2's
palette entry, no name, Halo 1's "not placed automatically" bit, Halo
1's position and rotation, and the unique ID, type and source bytes
`h1campaign` writes. The palette (`scnr +0x58`, `0x28`) and placements
(`+0x50`, `0x5c`) go into the **fresh** build's scenario, which is a
built tree there and takes any size; the pinned build (`--fresh false`)
can't grow the base's palette and places none. The Halo 2 sandbox now
converts through `h1objects` too (`halo1::prepare_halo2`): its scenery
tags are added after the vehicle collections, which are added first.
`every_map_places_halo1_s_scenery_in_both_sandboxes`
(`h1objects/tests/sandbox_disc.rs`) checks every palette entry names a
`scen` with a render model and every placement keeps Halo 1's place:
Blood Gulch 50 placements on 19 entries, Battle Creek 30 on 9, none left
out.

**Placed scenery is solid as structure (Built, converter version 92:
`h1convert::scenery_collision`, `convert::halo1::merge_scenery_collision`).**
The engine gives scenery no Havok body, so a placed rock or tree drawn as
scenery was walked through ([objects.md](objects.md), "Collision for
Havok"). Retail Halo 2 builds its rocks into the structure, and so does the
conversion now: in both sandboxes, every scenery placement there from the
start whose object has a collision model (decided by the `coll`, not the
name: Blood Gulch's boulders and trees among them) has that collision
merged into the level's collision BSP, and the object still draws as
scenery. "Collision" (above) has how.

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
On Blood Gulch's sandbox 17 models are converted and 81 materials draw
Halo 1 textures. Since converter version 73 a model shader that is not
`senv` or `soso` (glass, chicago, generic, meter, plasma, water) is
rebuilt first as the structure's shaders are (`shad::read_h1`, its own
blended, additive or self-illuminated stem and maps); the `tex_bump`
copy is what's left when that finds no stem or map. Before, a10's cryo
tube glass (`sotr` `levels\a10\shaders\cryo glass`, no map at the
offset `h1_maps_of` reads) named the stand-in below, a marine's head
shader, and drew as an opaque grey pane over the view from inside the
tube (`tests/model_shaders_disc.rs`). A transparent shader's map that is
a cube is its reflection, in `one_alpha_env`'s cube slot, and the least
tiled 2D map is its picture (`shad::transparent_maps`): the cryo glass
lists its reflection cube first. Since converter version 76 a `soso`
with a reflection (a cube map at `+0x164` and a brightness at `+0x144`
or `+0x154`, `shad::soso_reflection`) is rebuilt the same way on
`tex_bump_env` (the commonest retail object stem, 27 of Coagulation's
weapon, vehicle and character shaders) with its cube map and Halo 1's
tints facing the eye and grazing. Halo 1's weapons take their metal from
that reflection over dark base maps (the sniper rifle's averages 35 of
255); the tex_bump copy drew them near black, most visibly in first
person. **The reflection mask (converter version 80).** Halo 1 masks a
`soso`'s reflection with its multipurpose map (`+0xbc`): every weapon
`soso` on Blood Gulch has one, and on this (Xbox) disc the mask is its red
channel (V: the sniper rifle's, DXT1 512x512, is red only, mostly dark:
the metal reflects in places). `tex_bump_env` has no mask map of its own:
the reflection is masked by the colour pass's alpha, the base map's times
the detail's (`shad::TEX_BUMP_ENV`). Halo 1's weapon bases are DXT1 with
no alpha, so through version 79 the converted weapons reflected everywhere
at full strength. Now a `soso` drawn on `tex_bump_env` takes a copy of its
base whose alpha is the multipurpose map's red
(`h1bitmaps::mask::red_into_alpha`, `Converter::masked_bitmap`), named
`<base>_reflection_<multipurpose leaf>` and built once per pair: DXT5, as
retail's weapon bases are (V: the battle rifle's and sniper rifle's on
Coagulation, 0x203), the base's colours re-encoded from its decoded
blocks (exact where a DXT1 block's endpoints are among its texels) and
its whole mip chain kept, the mask sampled at each level's size (nearest).
A base or mask that does not decode keeps the plain base. Proof: h1bitmaps
`a_base_takes_its_masks_red_as_alpha` and the block round trips;
`tests/model_shaders_disc.rs`, `bloodgulch_sniper_metal_reflects_its_cube_map`
(the sniper's converted base against its multipurpose red). **Detail
tiling and cut-outs (converter version 84).** A `soso` tiles its detail
map by its own scale (`+0xd8`, v times `+0xec`; `textures::scales_from`):
16 on the Warthog's hull and chain gun and the Scorpion's hull, 8 on the
Ghost's, 10 on the cyborg's armour. Before, both the `tex_bump` copy and
the reflection rebuild tiled it once over the base, so the corroded metal
lay over each hull as a few large dark smears (proven live: the Ghost's
hull detail vector set to 8 and the Warthog's to 16 each turned the flat
panels into Halo 1's fine corroded metal). And Halo 1
alpha-tests a model shader's base unless its flags (`+0x28`) set bit 2,
"not alpha-tested", which no Blood Gulch `soso` does
(`shad::soso_alpha_tested`). Where the base stores an alpha (DXT3 and the
other alpha formats, `shad::h1_has_alpha`), the shader is rebuilt on
`tex_bump_alpha_test_single_pass`, its reflection dropped if it has one
(retail's `tex_bump_env_alpha_test` binds seven maps, which has no layout
here; retail's ammunition bags are alpha-tested without a reflection
too): the plants' and trees' leaves (DXT3, three quarters clear), the
ammunition bags and the weapon and ammunition decals, which drew as solid
cards on the opaque copy (a live frame shows the broadleaf's leaves as
dark green cards). The stem alone still left the cards whole: its depth
and shadow passes cut by the bump map's alpha, and from converter
version 92 the bump slot holds a flat bump cut out where the base is
(objects.md "Cut-out model shaders"). Proof:
`a_model_shader_tiles_its_detail_by_its_scale`,
`a_model_shader_cuts_out_where_its_base_is_clear`, and
`tests/model_shaders_disc.rs`,
`bloodgulch_leaves_cut_out_and_details_tile_by_their_scale`. Not converted
yet: a `soso`'s self-illumination (multipurpose green times its animated
colour, `+0x70` to `+0x8c`), its detail mask (`+0xd6`) and the reflection of a
cut-out one. A `soso`'s change colour (`+0x4c`, multipurpose
blue) draws with Halo 2's change-colour stem from converter version 84
(objects.md "Change colours"). A player biped also carries the player's
emblem (models.md "The emblem").

**Additive chicago chains and the view-angle fade
(converter version 90).** Blood Gulch's teleporter receivers (the three
`scenery\teleporter_base\teleporter_base` placements; the senders are
in the bases' structure) draw a light cone,
`scenery\teleporter_base\shaders\teleporter_cone` (`schi`, additive,
two-sided, framebuffer fade mode 2, "fade when parallel"): three maps,
each map's colour function (`+0x2c`, 2 multiply) multiplying it into the
next, the grey noise `base volumetric dust` twice, each scrolling at its
own rate (u 6 s, v −20 s; u −4 s, v 15 s), times `teleporter_dust`, a
32×32 green ramp black at the top (v ×1.05). Before, a transparent
shader drew its least tiled map alone: the first dust, so the cone was a
static grey speckled shell at full brightness, back faces culled, its
silhouette hard and none of Halo 1's green. Now an additive `schi` or
`scex` whose maps all multiply (every map but the last on function 2 or
3, `shad::multiply_chain`) is rebuilt on `two_add_clouds` (Coagulation,
Beaver Creek, 05a; `rgb = t1 × t2 × 2 × v0.rgb × c0.a × v0.a`, blend
one/one, two-sided): its first scrolling map on bitmap 0, which the
clouds tree scrolls (`shad::CLOUDS_TREE`, as retail's `cr_stream`), its
first still map on bitmap 1, and every other map folded into the tint
as its mean colour (a scrolling layer averages to it), halved for the
stem's doubling (`shad::Chain`). Without the stem, `one_add_illum` draws
the still map with the rest folded in. Every additive transparent shader
now also fades as Halo 1's fade mode says (`+0x2e`; `shad::Tint`): the
stems' vertex shader (`transparent_generic_reflection`) blends the
perpendicular colour and alpha (`v[10]`, the surface facing the eye)
into the parallel ones (`v[11]`, grazing), and the pass multiplies its
colour by both; retail's sun volumes and headlights use it the same
way. Fade when parallel is alpha 1 facing the eye and 0 grazing, so a
cone's silhouette dissolves; fade when perpendicular the reverse. D
(every model shader on the disc): 61 `schi` and 125 `sotr`; the chains
are seven, all fade when parallel: the teleporter cone, a50's
`volumetric light cone small` and `volumetric light cone ground`, b40's
`b40_foglamp`, the fighterbomber's `afterburners` and the pelican's
`hover thrusters` and `jet thrusters`; 8 `schi` and 16 `sotr` fade when
parallel, one `sotr` when perpendicular. Proof: `shad` unit tests
(`a_multiply_chain_draws_its_first_scrolling_map_and_first_still_one`,
`the_tint_and_scroll_land_in_the_stems_vectors_and_tree`,
`the_fade_mode_sets_the_facing_and_grazing_alphas`) and
`tests/shad_rebuild.rs`,
`additive_chicago_chains_draw_two_maps_scrolling_and_faded`. **Plasma
`sotr` (converter version 90).** The receiver also draws
`scenery\teleporter_shield\shaders\teleporter_shield` (`sotr`,
additive, fade when parallel): seven combiner stages over four maps,
`energy plasma` twice (scrolling u 10 s backwards and v 6 s; u 11 s and
v 7 s backwards), a still energy mask and the still shield picture,
whose alpha is a rounded square. It is one of Halo 1's plasma programs:
the difference of two scrolling noise samples, squared and sharpened
into thin moving bands, coloured and masked by the picture, the same
idea as Halo 2's `plasma_alpha` (retail's own teleporter and needler
shaders: `r0 = t1 − t2`, squared ×4 and `bx2` squared, coloured between
colour 1 and colour 0, times the mask's alpha and the fade). Before, it
drew its least tiled map alone on `one_add_illum`: `energy plasma`,
white, added over the base as a blown-out white square (live frame,
`before-receiver-close.png`). A `sotr` is now a plasma
(`shad::is_plasma`) when it is additive with seven stages over four 2D
maps, the first two the same bitmap and both scrolling, the last two
still; it is rebuilt on `plasma_alpha` (`shad::plasma_shader`): the first
noise on bitmap 0, the second on bitmap 1 scrolling as Halo 1's map 1
(the clouds tree, as retail's `needler_ammo_glow` animates it), the
picture on bitmap 2 as the mask, both band colours the picture's mean
colour, colour 2 black and colour 3's alpha 0 as on every retail
instance, and Halo 1's fade in vectors 7 (`v[10]`, facing) and 6
(`v[11]`, grazing): retail's teleporter plasma fades when parallel
too. D (every `sotr` on the disc): thirteen match, on four stage
programs: the teleporter shield, the jackal's and the field generator's
shields, the overshield and active camouflage powerups, the dropship's
tractor beam, the needler's core and ammo glow, the gun turret's and
c20 index platform's beam tendrils, a50's prison energy door and shuttle
bay doors, and b40's structure `b40_shaftenergy`. Of the 125 `sotr`
models draw, 12 are plasmas; none of the other 113 matches (D, by
framebuffer blend and stage count): additive with 0 stages 18, 1 stage
24, 2 stages 15, 3 stages 12, 4 stages 4 (chillout's hilltop, b40's holo
halo ring, a10's shield charge, Cortana); alpha-blended (blend 0) with
0 to 3 stages 22, 4, 5 and 1; blends 1, 6 and 7 with 0 to 2 stages 1,
6 and 1. They keep their single-map rebuild. Lost on a plasma: the energy mask
(map 2), Halo 1's stage constants (the teleporter's teal base glow,
added under the bands), and the first noise's own scroll (retail's
teleporter animates both pairs; that tree isn't verified here). Proof:
`a_plasma_is_seven_stages_over_one_noise_twice_then_two_still_maps`,
`a_plasma_scrolls_its_second_noise_and_fades_through_vectors_7_and_6`,
and `tests/shad_rebuild.rs`,
`plasma_sotrs_draw_on_plasma_alpha_and_no_other_sotr_does` (prints what
every `sotr` becomes). Which instance colour reaches which combiner
constant is read from the retail colour values, not the pass's
bindings (colours 2 and 3 are written the same, alpha 0, which makes
the order of those two moot: one is stage 5's, which reads only alpha,
the other stage 6's, which reads only colour).
Retail `plasma_alpha` scrolls either noise: `needler_ammo_glow` the
second (the clouds tree, bitmap target 1, `c[22]` and `c[23]`),
04a's `glass_gravunit_glow` the first (bitmap target 0, binding words
`0x01d2` and `0x01f3`: `c[20]` and `c[21]`).

**Teleporter receiver brightness against Halo 1 (converter version
90).** Face on, per channel (`teleporter_receiver_brightness_against_halo1`,
numbers from the disc's bitmaps). The cone (Halo 1 `dust × dust ×
ramp`, no doubling): Halo 1's mean (0.012, 0.030, 0.014) and the
conversion's are equal by construction (the dropped dust folded in as
its mean); its peak (0.148, 0.362, 0.175) against the conversion's
(0.062, 0.154, 0.073), the folded dust's peak averaged away. Before
version 90 the grey dust drew alone: peak 0.61 in every channel.

The shield's last stage (`teleporter_shield` stage 6, and every plasma
but the jackal's and the generator's shields) adds `picture.rgb × (band
+ c0)`. The band is 1 except along thin dark lines (the noises' squared
difference `s`, expanded and squared, then `x(2 − x)`); `c0` runs from
its lower to its upper bound by the object's A output (stage flag bit
2, `shad::STAGE_A_OUT_COLOR0`: set on the plasma stages alone, whose
bounds differ), and `teleporter_base` has no functions, so the glow is
the lower bound, (0, 0.329, 0.424). `plasma_alpha` adds `mask.a ×
(colour 0|1 × s + colour 2|3 × q)`, `q` the stem's own band (`s`
expanded and squared three times: 1 but for thin dark lines), so the
conversion (`shad::PlasmaLight`) binds the picture with its brightness
as alpha (`h1bitmaps::mask::brightest_into_alpha`: DXT5, alpha `gain ×
max(r, g, b)`, `<picture>_brightness_<gain × 1000>`), colours 2 and 3
the lit band `hue × (1 + glow) / gain` and 0 and 1 the glow `hue × glow
/ gain`, the gain `1 + glow` in the hue's brightest channel (1.329 for
the teleporter's green), so the stem's 8-bit colours keep Halo 1's
headroom and its clamp meets Halo 1's. Lit, Halo 1 peaks at (0.063,
1.0, 0.134) and averages (0.005, 0.269, 0.047) over the picture, the
conversion (0.020, 1.0, 0.173) and (0.005, 0.269, 0.046); on a dark
line Halo 1's glow peaks at (0, 0.262, 0.040) and averages (0, 0.067,
0.014), the conversion's (0, 0.247, 0.051) and (0, 0.066, 0.014)
(simulated before DXT5; the test decodes the mask the converter
writes). The red and blue peaks differ because the stem has one hue
where Halo 1 colours each texel its own. The first plasma rebuild (unreleased) drew the picture's
mean colour masked by its alpha (0.10 to 0.13, nearly flat): peak
(0.0005, 0.027, 0.004), 37 times too dim, shapeless and with no glow;
before that `energy plasma`, white, peak 1.0 in every channel. Lost: a
glow an object's A output animates (the jackal's shield) holds at its
lower bound; a plasma whose last stage is another sum (the jackal's and
the generator's shields add `v1 × (1 − picture.a)`) gets no glow.

A material with no Halo 1 base map to read names the
base's model shader itself; Blood Gulch has none. The sky model's
materials name copies of the base's sky shaders drawing Halo 1's sky
bitmaps at Halo 1's scale (`h1convert::sky_shaders`, sky.md "The
model's shaders"), or the base sky's own shader where one doesn't convert
(`tests/sandbox_disc.rs`).

A material never names no shader (NONE). The shader draw begin
`0x1bbf0` does not skip a NONE shader: at `0x1bcb9` it only clears its
cached words `0x485a64..0x485a70`. It still calls the draw
`0x16b90(0x485b48, NONE, ...)`, which reads tag slot `0xffff`'s entry (past
the table) as a `shad` and dereferences its compiled postprocess
(`0x16bc7`, `0x16bde`). Before converter version 21, converted skies named NONE on all 5
materials. That is the likely cause of the null read on Blood Gulch's
first frame (pc `0x16bde`, ebp `0x485b48`, the state cache).

### Starting weapons (**Built**, converter version 56)

A game variant's starting weapons are bytes (primary `+0xd6`, secondary
`+0xd7` of the variant; `variants` settings 89 and 94). `0x0019eda0`
turns one into a weapon: it jumps on the byte minus 1 through the table
at `0x0019eee8` to a read of one entry of the multiplayer globals' weapon
list (globals `+0x168` names the `mulg`; its `+8` block's first element,
`+0x68`, 8-byte tag references). Byte 1 gives none, 2 a random byte in
3..=18, and bytes 3..=20 each one entry (E; the table is
`h1convert::globals::STARTING_WEAPON_ENTRY`):

| Byte | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Entry | 3 | 0 | 1 | 6 | 7 | 5 | 2 | 4 | 12 | 9 | 10 | 13 | 11 | 15 | 14 | 8 | 16 | 17 |

The list is Halo 2's (Coagulation: magnum, SMG, shotgun, battle rifle,
sniper rifle, rocket launcher, plasma pistol, plasma rifle, brute plasma
rifle, needler, carbine, none, brute shot, beam rifle, energy sword,
rocket launcher, sentinel beam, none). The Halo 1 cyborg's graph has no
Halo 2 weapon type, so the default variants' SMG was refused
(docs/halo1/objects.md, "Weapon class and type") and players spawned
unarmed. In the Halo 1 sandbox every entry naming a weapon now names a
Halo 1 one (`sandbox::HALO1_WEAPON_LIST`, `convert::halo1`), the first of
its choices the map has:

| Halo 2 weapon | Halo 1 weapon | Why |
|---|---|---|
| magnum | pistol | the sandbox table reversed |
| SMG | assault rifle | reversed |
| battle rifle | assault rifle | the precision row reversed |
| shotgun, sniper rifle, rocket launcher, plasma pistol, plasma rifle, needler | the same | reversed |
| carbine | pistol | **D**: Halo 1's mid-range precision weapon |
| beam rifle | sniper rifle | **D**: long-range precision |
| brute shot | rocket launcher | **D**: explosive launcher |
| brute plasma rifle | plasma rifle | **D**: automatic Covenant plasma |
| energy sword | plasma rifle | **D**: Halo 1 has no melee weapon; nearest close-range Covenant one |
| sentinel beam | plasma rifle | **D**: continuous Covenant beam |

**Byte 0, the map's default** (64 of the 78 starting weapon bytes of
mainmenu's built-in variants, Slayer among them): `0x0019ef40` reads the
scenario's starting equipment (`scnr +0x128`, `0x9c` each; entries whose
game type `0x0015a090` accepts) and rolls its five `itmc` references
(`+0x3c`, 8 apart; `0x0019efef` -> `0x0019f1a0`) for the default (E). The
base's (Coagulation's one entry: the SMG collection) named Halo 2 weapons
too. In the Halo 1 sandbox each collection it names is rebuilt from the
same table, weight for weight, as a new `itmc` after the objects
(`convert::halo1::plan_starting_equipment`): players start with the
assault rifle on every map.

A weapon the map lacks falls to the next choice (the needler to the
plasma pistol, the plasma rifle, then the assault rifle, which every Halo 1
multiplayer map has). V: `tests/sandbox_disc.rs`
(`every_starting_weapon_is_held_by_the_halo1_player_graph`): on every
map, every byte the table maps, each built-in variant's primary and
secondary, and every weapon of the starting equipment's collections, is a
weapon the converted player biped's graph holds in mode `combat`.

### Design notes


`weap`, `proj`, `bipd`, `vehi`, `eqip` and their `mod2`/`coll`/`phys`,
`antr` and effects, converted per [campaign.md](campaign.md) §2, with
Halo 2's physics. The health subsystem of campaign.md §6 is shared with
the campaign: no body regeneration, health packs refill on pickup, a
health meter under the shield meter, on for Halo 1 maps in this variant.
Hooks (docs/halo1/health.md): the game start sets the model from the map's
name; the converted cyborg's HUD (`nhdt`) has a widget on the body
(input 17), built by `h1objects::health_hud`. A health pack
is an `eqip` with `+0x12c` = 5 (i16) and `+0x130` > 0; retail Halo 2
already handles that powerup type, so no hook is needed.

## What only a game run can prove

- that the engine loads a converted `sbsp`/`ltmp` and the player stands on
  the converted collision;
- that spawn points, flags and weapons land where the Halo 1 map had them;
- that the map is listed and starts from the multiplayer lobby.
