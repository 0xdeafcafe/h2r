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
| Assault rifle | SMG | Halo 2's automatic bullet hose; the battle rifle is the precision role the pistol filled |
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
| Overshield, active camouflage | Same | |
| Warthog, Ghost, Scorpion, Banshee | Same | |
| Health pack | Dropped | Halo 2 has no health |
| Ammo pickups | Dropped | Halo 2 has no ammo pickups |
| Double speed, full-spectrum vision | Dropped | Unused on Xbox maps |

## Placements plan (**Built** as `h1convert::placements`)

`plan_map` turns a Halo 1 map's multiplayer parts into Halo 2 sandbox
records, and lists everything it leaves out:

| Halo 1 | Halo 2 |
|---|---|
| Player starting location | Starting location: position, facing, team. Halo 1's per-spawn game types are not kept (Halo 2 has no field for them) |
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

## Still to do, in order

1. **Placements onto the base.** Write the plan into the base's scenario
   through `scenario-edit` (its `Scenario` reads, edits and grows these
   blocks). Halo 2's netgame flag kind values (`H2Flag::raw`) follow the
   community enum, and the disc agrees (`h1convert/tests/h2_scenario.rs`:
   0 and 1 once per team on all 12 maps, 4 always neutral, 7 and 8 in
   equal numbers, 10 to 16 runs of boundary points); not yet confirmed from
   engine code.
2. **Structure BSP and lightmap.** Write a whole Halo 2 `sbsp` and `ltmp`
   from Halo 1's geometry: collision (done), the leaves block parallel to
   the collision leaves (cluster per leaf), clusters with their render
   geometry as raw blocks ([structure-bsp.md](../formats/structure-bsp.md)),
   materials naming `shad` tags on existing `stem` templates, the lightmap
   group with Halo 1's lightmap pages as a bitmap. Open: which other `sbsp`
   blocks the engine needs to load a level (portals, the Havok MOPP for
   physics objects, sound environments, pathfinding); read the engine's
   BSP load path first.
3. **Sky** onto the base's sky.
4. **`h2 h1 convert <iso|map> --out DIR [--sandbox h2|h1] [--base MAP]`**
   writing both variants and a manifest; then load-testing with
   `H2_MAP_DIR` as `h2 map testbuild` does.
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
- **Identity:** the list record's map id should follow the Halo 1 map, not
  the variant's bytes, so switching the setting doesn't move the map in the
  list or break saved game variants. The `MapId` (content hash) still
  differs per variant, so online peers with different settings see
  different maps; [maps.md](maps.md) §5 refuses a mismatch. Open: whether
  that is the behaviour wanted, or the host's choice should win.

## Halo 1 sandbox (design, phase 4)

`weap`, `proj`, `bipd`, `vehi`, `eqip` and their `mod2`/`coll`/`phys`,
`antr` and effects, converted per [campaigns.md](campaigns.md) §2, with
Halo 2's physics. The health subsystem of campaigns.md §6 is shared with
the campaign: no body regeneration, health packs refill on pickup, a
health meter under the shield meter, on for Halo 1 maps in this variant.

## What only a game run can prove

- that the engine loads a converted `sbsp`/`ltmp` and the player stands on
  the converted collision;
- that spawn points, flags and weapons land where the Halo 1 map had them;
- that the map is listed and starts from the multiplayer lobby.
