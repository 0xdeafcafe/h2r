# Halo 1 multiplayer maps in the Halo 2 engine (design)

Status: **Partly built.** Converted maps are confirmed in the game by the
owner. Each section says what is **Built** (with its proof) and what is
design only. This applies the rules
of [campaign.md](campaign.md) to multiplayer: Halo 2's engine runs
everything (physics, AI, rendering, game types); Halo 1's health bar and
shield model are kept in the Halo 1 setting. Both settings retain Halo 1's
health packs, overshields and active camouflage. Halo 1's assets are the
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
   tag store levels h1_<map>_h1 and h1_<map>_h2 ─▶ guest content offer
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

`crates/tools/h1map`, verified on beavercreek and carousel
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
close: beavercreek has 5,033 nodes and 2,476 surfaces). It also refuses by name a surface
ring over 64 edges or a 3D tree deeper than 256 nodes, the Rust collision walk's
bounds (`collision::check_bounds`, `Error::CollisionBound`; scenery pieces and
the merged level are checked too). Nothing is split or trimmed. The per-map
maxima over the disc are not measured yet.

Halo 2 keeps a structure BSP's collision at `sbsp+0x14`: one `0x40`-byte
element, the eight blocks of an object collision BSP without its leading
word, with collision materials at `sbsp+0x0c`. **V** on all 12 multiplayer
maps of the Halo 2 disc by `h1convert/tests/h2_sbsp.rs`. Surfaces with no
material hold `0xffff` in both games.

**Proof** (`h1convert/tests/real_maps.rs`, on both maps): the conversion is
encoded with Halo 2's layout and decoded back with `crates/formats/tags`'s decoder,
passes its validator, and agrees with the Halo 1 source on every surface's
polygon, on 50,000 random points (open or solid, and which leaf), on 20,000
random segments and 20,000 segments from open space (where each first
enters solid, compared exactly), and on a ray down from every spawn point.

**Placed scenery joins it** (`h1convert::scenery_collision`). For each scenery placement there from the
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
  nodes per level leaf. Copying the whole tree into every level leaf the
  box reaches is not done: it grows Blood Gulch's collision BSP past what
  the pinned build's BSP space holds.
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
  "BSP space outgrew the pinned metadata base".
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
| Overshield, active camouflage | **Built** from Halo 1 | Native Halo 2 powerup types and rebuilt item collections |
| Warthog, Ghost, Scorpion, Banshee | Same | A collection is built when the base lacks one ([Vehicles](#vehicles)) |
| Rocket warthog | Gauss warthog | The user's decision |
| Health pack | **Built** from Halo 1 | The engine has a native health pickup (type 5) |
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
- **The swap table** is read at boot from `<data>/tags/halo1-mp/*/manifest.toml`
  (the `h2` variant of each package), keyed by `base.map`. The hook is
  idle in the Halo 1 sandbox, which places Halo 1's assault rifle.

## Bot shotgun range (Built)

The Halo 1 sandbox carries over the first barrel's projectile damage
range. Blood Gulch's shotgun has a 3-unit air damage endpoint, 15-unit aim
assist and 40-unit projectile travel. The bot uses the 3-unit endpoint to
limit its shots. The Halo 2 sandbox uses Coagulation's shotgun, with an
8-unit endpoint. Blood Gulch's assault rifle has a 15-unit air damage
endpoint. The ignored `weapon_range_disc` test reads both discs and both
installed sandboxes through the live bot's weapon profile reader.

The same round gate applies to both sandboxes and retail Halo 2 maps:
phantoms clear their queued input while the round is closed. The gate
and retail XBE addresses are in
[Bots](../game/bots.md#multiplayer-round-start-built).

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
placements removed; the 3 camouflage spawns were reported by the base-only conversion, the base has
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

**Halo 1 light fixtures (Built).** Both sandbox variants rebuild the
source fixtures as machines with their marker lights. Carousel,
Damnation and Rat Race have 46 placements between them. The fresh
scenario writes their machine blocks directly. The pinned builder appends
them to the metadata and updates the scenario descriptors. The source
placement's light values use the same rule as campaign fixtures.
See [objects.md](objects.md#light-fixture-placements-built).

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

Moved to [Halo 1 multiplayer: sandbox and converted structure](multiplayer-sandbox.md#the-converted-structure-bsp-built-as-h1convertstructure).

## What the engine reads

Read from the retail XBE's code (static reading only; **E** cites the
instruction, **Guess** is inference). The scan followed every load of the
active BSP pointer `[0x4e0348]` and active lightmap `[0x4e0344]`, and the
cluster pointers derived from them. The sections below were written for a
one-cluster conversion with no portals and no instanced geometry; the
converter keeps Halo 1's clusters and portals (above).

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
group block empty (the converter writes Halo 1's lightmap pages instead,
above). The load itself validates only the region's `sbsp` FourCC (E `0x122b6d`); a
checksum mismatch only leaves the lightmap unused.

## Converting a map (**Built**: `h1convert::convert`, `h2 h1 convert`)

Moved to [Halo 1 multiplayer: converting a map](multiplayer-convert.md#converting-a-map-built-h1convertconvert-h2-h1-convert).

## Halo 1 options (**Built**, tests only: `boot::h1_options`, `boot::h1_options_row`)

Moved to [Halo 1 multiplayer: converting a map](multiplayer-convert.md#halo-1-options-built-tests-only-booth1_options-booth1_options_row).

## Vehicles

Moved to [Halo 1 multiplayer: vehicles and what is not placed](multiplayer-vehicles.md#vehicles).

## What isn't placed, and why

Moved to [Halo 1 multiplayer: vehicles and what is not placed](multiplayer-vehicles.md#what-isnt-placed-and-why).

## Open items

- **Root blocks the engine read doesn't settle.** What the disc shows
  (`structure_bsp_root_blocks_on_the_disc`, all 12 multiplayer maps;
  element sizes are upper bounds from block spacing, not engine-backed):
  every map uses the root blocks at `+0x0c` (collision materials, 20),
  `+0x14` (collision BSP), `+0x2c` (8 bytes, **one per collision leaf** on
  every map: V), `+0x4c` (8), `+0x54` (1), `+0x5c`, `+0x94` (36), `+0x9c`
  (clusters, `0xb0`), `+0xa4` (materials, 32), `+0xac` (2), `+0xd4` (100),
  `+0xdc` (72), `+0x138`/`+0x140` (instanced geometry, `0xc8`/`0x58`),
  `+0x148`, `+0x150`, `+0x1d0` (one element), `+0x1e0` (bytes, up to
  160 KB: a Havok MOPP code block), `+0x21c`, `+0x224`, `+0x234` (one
  element each). The rest are used by some maps only. The converter
  writes `+0x4c`, `+0x94`, `+0x148` and `+0x1d0` empty without an engine
  reading that allows it (above).
- **Precision variants.** A game run should show the battle rifle on Blood
  Gulch's assault rifle spawns in a BR-start variant and the SMG in Slayer.
- **Predicted resources on converted object models.** The structure lists
  predicted resources (above); converted object models don't need them:
  prediction only prefetches. A model's geometry block is loaded when
  drawn (`0x0012de70` → `0x00223c70`, from the draw paths) and a bitmap
  image when sampled with no cache handle (`0x0012c9fb` → `0x0012d5a0`);
  the lists (`obje +0xb4`, read at `0x000bbe00`..`0x000bbeb6` →
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

The importer stores each variant as `h1_<map>_h1` or `h1_<map>_h2`.
The store writes a `<stem>.converted` marker for each completed variant.
The import stamp, converter version, and `--no-halo1` and `H2_HALO1=0`
controls are in [campaign.md](campaign.md) §10. `h2 h1 convert` exports
one map to the directory chosen with `--out`.

## On the stats site and in the viewers (**Built**: `cli::live_halo1`)

`h2 live assets extract` (and `h2 play --live-local`) draws every converted
map, multiplayer and campaign, like a Halo 2 map: geometry, sky, the five
views and a preview, keyed by the name the game reports (`h1_<map>`, or
the campaign level's), so live games and recordings on them plot on their
map ([live-art.md](../live/live-art-viewer-packs.md#halo-1-maps)). The manifest marks
them `"game": "halo1"`; the site labels them Halo 1, names them from the
manifest, and does not call a converted map "modded" or its geometry
changed.

## The setting and the map list (**Built**)

- **Row:** `multiplayer.halo1_sandbox`, a choice `halo1` (default) /
  `halo2`, one `settings::Def` in `boot::setting_defs`, on the Halo 1
  options screen from the Game Setup dialog ("Halo 1 options" above).
- **Packaging:** `boot::new_maps` checks the store marker against the
  variant hash and registers one content offer per map with a
  `patch.lvl` record for 1.0.
- **Where the choice happens:** the setting selects a sandbox at boot.
  The pregame screen can change it, and the VFS applies the choice on
  the map's next open.
- **Names (decided):** converted maps are `h1_<map>` (`h1_beavercreek`:
  Halo 2 has its own `beavercreek`), with Halo 1's display name in the
  list.
- **In the map select (built):** a new map whose name starts `h1_` is
  listed under **Halo: CE** ([map-select.md](../game/map-select.md)).
- **Identity (built):** one list ID per Halo 1 map, whichever
  variant (`Package::list_key`, above); the `MapId` (content hash)
  differs per variant; online, the host's variant is used.

## Halo 1 sandbox (**Built**: `convert::halo1`, `h1objects::sandbox`)

Moved to [Halo 1 multiplayer: sandbox and converted structure](multiplayer-sandbox.md#halo-1-sandbox-built-converthalo1-h1objectssandbox).

## What only a game run can prove

- that the engine loads a converted `sbsp`/`ltmp` and the player stands on
  the converted collision;
- that spawn points, flags and weapons land where the Halo 1 map had them;
- that the map is listed and starts from the multiplayer lobby.
