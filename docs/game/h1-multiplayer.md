# Halo 1 multiplayer maps in the Halo 2 engine (design)

Status: in progress, started 2026-11-16. Each section says what is
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
   <data>/maps/<h1 map>/{h2,h1}/<map>.map + manifest ─▶ content package
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
If `fresh` loads (the index tail can be dropped), the next step is to let
the converter add tags (Halo 1 sandbox tags) to the pinned base; if it
doesn't, Halo 1 sandbox tags must replace unused base slots instead. That
choice only matters for phase 4.

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
| Overshield, active camouflage | Same (`powerup_super_shield`, `powerup_invisibility`) | Dropped with a report when the base has no such collection (H2's beavercreek has no camouflage) |
| Warthog, Ghost, Scorpion, Banshee | Same | |
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

Not cleared yet: decals, lights and decorators. Their scenario blocks
aren't located in engine code yet (the trigger volumes above were found
by following the scenario pointer `[0x4e0350]`; the same scan lists
every scenario block the engine reads, `h1mp-scripts/scnr.py` in the
reading notes). They don't affect loading as far as known, but would show
up in the wrong place: decals stuck to air, lights shining from nowhere.
Decorators may matter more, if their placements carry cluster indices
past the converted structure's clusters; check first.

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
  needed flipping. Meshes are packed into sections of at most 65535
  vertices and indices (the section's `u16` counts), so a large level
  becomes several clusters; parts never span sections. Streams written:
  `01` position, `18` texture coordinate, `1b` frame, `1f` lightmap
  coordinate. Part kind 2 (a drawn kind; which drawn kind means what is
  unknown), triangle-list flag, one subpart per part.
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
  CLI prints each choice (`shader <Halo 1> -> <Halo 2>`). Halo 1's own
  textures replace these once they convert. One collision material per
  Halo 1 collision material, copies of the base's first (no pointers in
  it), `+0x0a` = -1.
- **The sky is the base's.** Cluster `+0x6e` is the cluster's sky index
  into the scenario's skies (`scnr+0x8`, 8-byte references; E
  `0x0014b38d`..`0x0014b3c4`, where -1 also falls back to the first sky
  when the scenario has one). Every cluster gets 0, so the base's first
  sky. Halo 1's skies are models and bitmaps of their own and convert
  with the Halo 1 sandbox work. The base per map (`convert::MAPS`):
  Coagulation for the maps with vehicles (below), Beaver Creek otherwise.
  Waterworks (snow, and a wraith) would suit Sidewinder and Ice Fields,
  but Sidewinder's placements grow Waterworks' metadata past the engine's
  tag memory, so they take Coagulation too.
- **Lightmap.** One group, the structure checksum (FNV-1a of the
  positions), no bitmap, a render info entry `{-1, -1}` per cluster.
  Levels render unlit (the default texture path) until Halo 1's lightmap
  pages are converted. The pages now convert (`h1bitmaps::lightmap_pages`
  and `convert_bitmap`, byte-identical on a30); what this writer must add
  to use them is in [h1-bitmaps.md](../formats/h1-bitmaps.md#lightmaps-for-the-structure-writer).

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
| `+0x9c` clusters (0xb0) | One: geometry header and raw block, bounds, `+0x6e` = `+0x6f` = -1, `+0x70` = `+0x71` = 0xff, `+0x72` = `+0x74` = `+0x76` = -1, `+0x7c` = -1 and `+0x7e` = 0, its blocks empty | E, see the reading notes |
| `+0xa4` materials (0x20) | One per part material: shader reference at `+0x08`/`+0x0c`, `+0x1c` = 0xff | Indexed unchecked (E `0x45dee..0x45e08`) |
| `+0xac`, `+0xb4`, `+0xc4` (pathfinding), `+0xd4`, `+0xdc`, `+0xf4`, `+0x138`, `+0x140`, `+0x154`, `+0x20c`, `+0x214`, `+0x224`, `+0x22c` | Empty | Each count-checked (E `0x3eb4e`, `0x181ddd`, every `+0xc8` site, `0x11bf56`, `0x18ddf4`, `0x23aa7d`, `0x183f6a`, `0x1e1dd`, `0x184214`, `0x3bec6`, `0x221ac7`, `0xbfd63`) |
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
([mopp.md](../formats/mopp.md)). `Sandbox::Halo1` returns
`Error::Halo1Sandbox` until phase 4.

```sh
cargo h2 h1 convert disc/halo1.iso beavercreek --out ~/h2-h1maps [--sandbox h2] [--base beavercreek] [--disc disc/halo2.iso] [--force]
cargo h2 h1 convert path/to/beavercreek.map --dry-run
```

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
file = "h1/h1_beavercreek.map"   # once the Halo 1 sandbox exists
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

**Not done yet:** the sandbox setting and `boot::new_maps` choosing by
it. The plan: a `multiplayer.halo1_sandbox` `settings::Def` (choice
`h1`/`h2`, default `h1`, `Restart`) in `boot::setting_defs`; the library
gets the choice (`Manifest::select_variant(Some(..))` in place of `None`
in `content::resolve::load`, through a `Library` option that
`new_maps::install` sets from the setting).

## Vehicles

Halo 2 spawns multiplayer vehicles from netgame equipment naming a
vehicle collection (`vehc`, `multiplayer\single_vehicles\<name>`), not
from the scenario's vehicle block, so converted vehicles go there and the
Vehicles block stays empty, as on retail maps. Only some bases have the
collections: Coagulation (warthog, ghost, banshee), Headlong, Zanzibar,
Burial Mounds, Waterworks (also a wraith); Beaver Creek and Lockout have
none. Halo 1's maps with vehicles therefore use Coagulation as their base.
Warthog, rocket warthog (no closer Halo 2 vehicle: the gauss warthog's
turret is a different weapon), ghost and banshee map to the same names;
the name match prefers an exact collection name, so `warthog` never
lands on `warthog_gauss`. **No Halo 2 multiplayer map has a scorpion
collection**, so Halo 1's scorpions are not placed.

## What isn't placed, and why

Every placement a conversion leaves out is one of these, and `h2 h1
convert` lists each with its position (`not placed: ...`):

| Left out | Why |
|---|---|
| Race flags (`RaceTrack`, `RaceVehicle`) | Halo 2 has no race game type |
| Health packs | Halo 2's sandbox has no health; they return with the Halo 1 sandbox (health, phase 4) |
| Active camouflage | The base has no `invisibility` collection (Beaver Creek and Coagulation have none) |
| Overshield | The base has no `super_shield` collection (Coagulation has none) |
| Scorpion | No Halo 2 multiplayer base has one (above) |

All 13 multiplayer maps on the Halo 1 disc convert
(`tests/disc_maps.rs`, `H1_ISO` and `H2_DISC`; 5 minutes in a debug
build), each map reads back with a valid checksum, and its bitmaps are
the base's, pixel for pixel. Per map (placed / Halo 1; items include
vehicles, which Halo 2 places as items):

| Map | Base | Triangles | Spawns | Flags | Items | Vehicles | Left out |
|---|---|---|---|---|---|---|---|
| beavercreek | beavercreek | 3317 | 70/70 | 88/97 | 28/35 | 0/0 | 11 race, 4 health, 3 camo |
| bloodgulch | coagulation | 5503 | 71/71 | 172/191 | 37/43 | 6/8 | 21 race, 2 health, 1 camo, 1 overshield, 2 scorpion |
| boardingaction | beavercreek | 7142 | 73/73 | 66/77 | 43/52 | 0/0 | 13 race, 7 health, 2 camo |
| carousel | beavercreek | 2940 | 63/63 | 71/79 | 33/37 | 0/0 | 10 race, 1 health, 3 camo |
| chillout | beavercreek | 3108 | 58/58 | 39/49 | 27/33 | 0/0 | 12 race, 4 health, 2 camo |
| damnation | beavercreek | 8618 | 67/67 | 77/79 | 41/46 | 0/0 | 4 race, 4 health, 1 camo |
| hangemhigh | beavercreek | 3218 | 67/67 | 83/92 | 41/45 | 0/0 | 11 race, 2 health, 2 camo |
| longest | beavercreek | 2319 | 71/71 | 48/51 | 35/40 | 0/0 | 5 race, 4 health, 1 camo |
| prisoner | beavercreek | 5234 | 58/58 | 40/47 | 45/51 | 0/0 | 9 race, 5 health, 1 camo |
| putput | beavercreek | 10494 | 59/59 | 127/134 | 26/40 | 0/0 | 9 race, 13 health, 1 camo |
| ratrace | beavercreek | 2692 | 48/48 | 41/43 | 27/30 | 0/0 | 4 race, 2 health, 1 camo |
| sidewinder | coagulation | 10036 | 76/76 | 147/166 | 48/55 | 6/8 | 21 race, 2 health, 1 camo, 2 overshield, 2 scorpion |
| wizard | beavercreek | 2972 | 48/48 | 57/59 | 36/44 | 0/0 | 4 race, 4 health, 4 camo |

Flag counts don't add up one for one: a Halo 1 flag can become several
Halo 2 flags (`placements::h2_flags`). Danger Canyon, Death Island,
Gephyrophobia, Ice Fields, Infinity and Timberland aren't on the Xbox
disc.

## Still to do, in order

1. ~~**MOPP writer**~~: done, `mopp::build_structure`
   ([mopp.md](../formats/mopp.md)), called from `convert::structure_mopp`.
2. **Decals, lights, decorators** of the base (above).
3. **Lightmaps** from Halo 1's lightmap pages, then **Halo 1's own
   textures** on per-shader Halo 2 shaders (base shaders are chosen by
   name until then).
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
4. **Precision variants** at round start (the AR rule above).
5. **The setting and the map list** (below).
6. **Halo 1 sandbox** (below), and Halo 1's skies with it.

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
- **Identity (decided default):** one list ID per Halo 1 map, whichever
  variant; the `MapId` (content hash) differs per variant; online, the
  host's variant is used.

## Halo 1 sandbox (design, phase 4)

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
