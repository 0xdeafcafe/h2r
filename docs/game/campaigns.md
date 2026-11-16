# Campaigns: Halo 1 in the Halo 2 engine (design)

Status: design. Nothing here is built yet except where a section says
**Built**. Started 2026-11-16.

The goal: more than one campaign in the game, starting with Halo 1's
(Halo: Combat Evolved, Xbox). The Halo 1 levels run **in the Halo 2
engine**. We don't port Halo 1 engine code, and we don't recompile Halo 1's
XBE.

Like every import in this project, this uses the user's own disc
(`disc/halo1.iso`), and no game data goes in the repo. Tests use synthetic
bytes, or read the disc at run time in `#[ignore]` tests.

## Decisions

| # | Decision | Chosen |
|---|---|---|
| 1 | Where Halo 1 and Halo 2 differ, which engine wins | **Halo 2**, for physics, AI, damage, rendering, scripting, saves. Halo 1 data is converted to what Halo 2's code expects |
| 2 | Health | **Exception: keep Halo 1's** health bar and health packs. This needs a health system added to the Halo 2 engine (§6) |
| 3 | Characters, weapons, vehicles | **Halo 1's own, converted**, by default. A setting switches to **Halo 2's** assets instead (§5) |
| 4 | First walkable level | **a30** (Halo): open outdoor ground, the easiest first proof |
| 5 | When conversion happens | **Offline, at import**: `h2 import halo1` writes Halo 2 `.map` files into the data folder. The engine loads them like any other level |

## 1. Pipeline

```
disc/halo1.iso ──xiso──▶ maps/*.map (cache v5, zlib)
                           │  h1map: read tags (Halo 1 layouts)
                           ▼
                   Halo 1 tag set (in memory)
                           │  convert: per tag group, Halo 1 → Halo 2
                           ▼
                   Halo 2 tag set (+ references into shared.map /
                           │        single_player_shared.map)
                           │  map builder: write a whole v8 cache file
                           ▼
              <data>/campaigns/halo1/<level>.map  ──▶ Halo 2 engine
```

Pieces:

- **`crates/h1map`** reads Halo 1 Xbox cache files: version 5, a 0x800-byte
  header, then a zlib stream holding the rest of the file. It knows the tag
  index and the Halo 1 tag layouts the converter needs.
- **The map builder** (in `crates/map`) writes a complete v8 cache file from
  a tag set: header, tag index, string IDs, metadata, raw resources and BSP
  slots. Today `MapEditor` only patches an existing map. The builder is the
  biggest missing piece, and Halo 1 needs it first. New maps made from
  scratch also need it ([maps.md](maps.md)).
- **`crates/h1models`** (Built) converts Halo 1 `mode` and `antr` tags to
  Halo 2 `mode` and `jmad` block trees for the builder
  ([h1-models.md](../formats/h1-models.md)).
- **The converter** (`crates/h1convert`, planned) works one tag group at a
  time, in the order of §3.
- **The import command**, `h2 import halo1 [disc/halo1.iso]`, runs the
  pipeline for every level and writes the maps, plus the campaign's level
  list (§4).

## 2. What each Halo 1 tag becomes

| Halo 1 | Halo 2 | Notes |
|---|---|---|
| `scnr` scenario | `scnr` | placements, spawn points, trigger volumes, cutscene flags and cameras |
| `sbsp` structure BSP | `sbsp` + `ltmp` | collision BSP is close to Halo 2's shape. Render geometry becomes cluster sections. Halo 1 lightmap pages become Halo 2 lightmap bitmaps |
| pathfinding | `sbsp` pathfinding data | Halo 1 maps don't contain the sectors, links and refs that Halo 2's AI needs, so we generate them from the collision BSP |
| `shdr` family (`senv`, `soso`, `schi`, `scex`, `swat`, `sgla`, `smet`, `spla`) | `shad` on existing `stem` templates | each Halo 1 shader type maps to the closest Halo 2 template in the shared maps, with its parameters. The look will be close, not exact |
| `bitm` | `bitm` | mostly the same Xbox texture formats |
| `mod2` + `coll` + `phys` | `mode` + `coll` + `phmo` under an `hlmt` | physics uses Halo 2's (decision 1) |
| `antr` | `jmad` | animation codecs differ: decode, then re-encode |
| `bipd`, `vehi`, `weap`, `eqip`, `scen`, `mach`, `ctrl` | the same groups | field-by-field. Halo 1 tuning values carry over where Halo 2 has the field |
| `actr` + `actv` | `char` | |
| encounters, squads, firing positions, command lists | squads, zones, firing positions, orders | |
| scripts | Halo 2 scripts | decompile Halo 1's syntax nodes, rename the functions that differ, compile to Halo 2 nodes with `crates/hs`. Builtins Halo 2 lacks go in the Rust HS VM ([hs_vm.md](../engine/hs_vm.md)) |
| `snd!` | `snd!` + `ugh!` | both Xbox ADPCM |
| `ustr`, `hmt ` | string IDs, `unic` | |
| recorded animations | recorded animations | versions differ |

## 3. Order of work

Each step is proven with tests; the user plays the result.

1. **Import and read.** Recognise the Halo 1 disc (title ID `4D530004`),
   pull out its maps, decompress them, list their tags.
   **Built:** `crates/h1map`, `h2 h1 info|tags`, `h2 import halo1`;
   format in [h1-map-format.md](../formats/h1-map-format.md). Not yet
   checked against a real disc.
2. **Map builder.** Write a whole v8 map. Proof: read a retail Halo 2 map
   into a tag set, build it again, and check the result reads the same, tag
   by tag. **Built:** `map::build` and `tags::rebuild`
   ([map-builder.md](../formats/map-builder.md)). All 30 retail maps
   rebuild byte-identical; 18 also pass with every bitmap moved. Not yet
   loaded by the game. A map made from scratch has open questions: the
   index tail, zeroed header fields, salts.
3. **a30 walkable.** Geometry, collision, lightmaps, sky, shaders, one
   player spawn. You walk around a30 in Halo 2's engine with Halo 2's
   physics. Halo 2's assets stand in for anything not converted yet.
4. **Objects.** Scenery, machines and doors, weapons, vehicles, equipment.
5. **AI.** Characters, encounters to squads, generated pathfinding.
6. **Scripts.**
7. **Cutscenes, sound and dialogue, text and HUD, checkpoints.**
8. **Menus.** The campaign picker and its level list (§4), and progress
   saved per campaign.

## 4. More than one campaign

The engine already has a campaign ID: level boot asks for "campaign C,
map M" ([level-boot.md](../runtime/level-boot.md)), and the level list is
data in `mainmenu.map`'s globals. The plan is to make Halo 1 a second
campaign in that list. The menu gets a campaign picker in front of the
level list. Open: how 1.0's campaign UI reads the list, and whether a
second campaign ID needs code changes beyond the data. Read this before
step 8.

## 5. The asset setting

Settings, Campaign: **Halo 1 assets** (default) or **Halo 2 assets**. The
import writes both variants of each level. With Halo 2 assets, Halo 1's
characters, weapons and vehicles are swapped for their nearest Halo 2
equivalents (assault rifle to SMG or battle rifle, and so on; the table
gets fixed in step 4). The setting picks which file the VFS serves when
the level loads. Halo 1's cutscenes are animated on Halo 1's skeletons, so
with Halo 2 assets they need retargeting; until that exists, cutscenes
keep Halo 1's characters in both variants.

## 6. Health (the exception)

Halo 1: a shield that recharges, over a health bar that doesn't; health
packs refill it. Halo 2: shields only, and the hidden body health
regenerates. For the Halo 1 campaign:

- the player's body health doesn't regenerate;
- health packs (Halo 1 `eqip` with a health powerup) refill it on pickup;
- the HUD draws a health meter under the shield meter.

This is a feature-engine subsystem ([feature-engine.md](../engine/feature-engine.md)),
on only for Halo 1 levels. Open: where Halo 2 regenerates body vitality,
and how its equipment pickup can carry a health powerup.

## 7. Open questions

- Memory: whether converted levels fit in Halo 2's tag memory
  (`max_metadata_size` for campaign maps is `0xc24000`). a30 has a big
  outdoor BSP.
- Which `stem` templates in the shared maps cover Halo 1's shader types.
- Generating pathfinding that Halo 2's AI accepts.
