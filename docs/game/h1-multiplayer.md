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

Not cleared yet: the base's other level-bound scenario data (decals,
lights, cutscene flags, trigger volumes, decorators). They don't affect
loading as far as known, but would show up in the wrong place.

## The converted structure BSP: what the engine reads

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
  `+0x0c` f32 scale, `+0x20` u32 code size, `+0x24` runtime reference
  count, bytecode from `+0x30`. Opcodes above `0x6b` assert; `0x00`
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
  This is the next step.
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

## Still to do, in order

1. **MOPP writer** for the structure (the blocker above).
2. **Structure BSP and lightmap** per the table above, laid into the
   base's `sbsp`/`ltmp` slots as a new BSP section (map-builder.md), with
   the scenario's BSP descriptor pointed at it. Render geometry: Halo 1's
   materials as one raw cluster section (structure-bsp.md), shaders on
   the base's existing `shad` tags.
   What the disc shows ( (`structure_bsp_root_blocks_on_the_disc`,
   all 12 multiplayer maps; element sizes are upper bounds from block
   spacing, not yet engine-backed): every map uses the root blocks at
   `+0x0c` (collision materials, 20), `+0x14` (collision BSP), `+0x2c`
   (8 bytes, **one per collision leaf** on every map: V), `+0x4c` (8),
   `+0x54` (1), `+0x5c`, `+0x94` (36), `+0x9c` (clusters, `0xb0`),
   `+0xa4` (materials, 32), `+0xac` (2), `+0xd4` (100), `+0xdc` (72),
   `+0x138`/`+0x140` (instanced geometry, `0xc8`/`0x58`), `+0x148`,
   `+0x150`, `+0x1d0` (one element), `+0x1e0` (bytes, up to 160 KB: a
   likely Havok MOPP code block), `+0x21c`, `+0x224`, `+0x234` (one
   element each). The rest are used by some maps only. Each of these needs
   a meaning, or an engine reading that shows it can be empty, before a
   converted `sbsp` can be written.
3. **Sky** onto the base's sky.
4. **`h2 h1 convert <iso|map> --out DIR [--sandbox h2|h1] [--base MAP]`**
   writing `h1_<map>.map` (both variants) and a manifest; then
   load-testing with `H2_MAP_DIR` as `h2 map testbuild` does.
5. **The setting and the map list** (below).
6. **Halo 1 sandbox** (below).

## The setting and the map list (design)

- **Row:** `multiplayer.halo1_sandbox`, a choice `halo1` (default) /
  `halo2`, one `settings::Def` in `boot::setting_defs` with a row on the
  multiplayer settings screen, `Restart` (it applies when maps are
  installed at boot), per [settings-menu.md](settings-menu.md).
- **Packaging:** each converted map is a new map in the sense of
  [maps.md](maps.md) §4: Halo 1's maps aren't on the Halo 2 disc, so
  `boot::new_maps` installs each as a content package with its
  `patch.lvl` record, and 1.0 lists it. The import writes one package per
  map holding both variants (`h2/<map>.map`, `h1/<map>.map`).
- **Where the choice happens:** in `boot::new_maps`, which already writes
  each new map's `.map` into its content directory at boot. It writes the
  variant the setting names. That is the one place the game's map file is
  chosen for a new map; the VFS and the content resolver need no change.
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
