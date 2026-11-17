# Campaigns: Halo 1 in the Halo 2 engine (design)

Status: design, with parts **Built**. a30 converts to a Halo 2 map with
its structure, sky, player starts, objects, AI, scripts and script sounds,
proven offline only (§9); no converted campaign level has been played yet.
Started 2026-11-16.

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
| 5 | When conversion happens | **Offline, at import**: `h2 import halo1` is to write Halo 2 `.map` files into the data folder (today it copies the Halo 1 source maps only). The engine loads them like any other level |

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
- **`crates/h1bitmaps`** (Built) decodes Halo 1 `bitm` images to RGBA8
  with Halo 2's texture decoder and converts them to Halo 2 `bitm` block
  trees, pixels copied as they are ([h1-bitmaps.md](../formats/h1-bitmaps.md)).
- **The converter** (`crates/h1convert`) works one tag group at a time, in
  the order of §3: collision, structure BSP, placements, MOPP
  (`h2 h1 convert`), see [h1-multiplayer.md](h1-multiplayer.md).
  **`crates/h1campaign`** puts it and the other Halo 1 crates together for
  a campaign level (§9).
- **The import command**, `h2 import halo1 [disc/halo1.iso]`, is built as
  step 1 only: it copies the disc's campaign maps unchanged to
  `<data>/campaigns/halo1/source/` with a SHA-256 manifest. The goal is
  for it to run the whole pipeline for every level and write the maps,
  plus the campaign's level list (§4).

## 2. What each Halo 1 tag becomes

| Halo 1 | Halo 2 | Notes |
|---|---|---|
| `scnr` scenario | `scnr` | placements, spawn points, trigger volumes, cutscene flags and cameras |
| `sbsp` structure BSP | `sbsp` + `ltmp` | collision BSP is close to Halo 2's shape. Render geometry becomes cluster sections. Halo 1 lightmap pages become Halo 2 lightmap bitmaps |
| pathfinding | `sbsp` pathfinding data | Halo 1 maps don't contain the sectors, links and refs that Halo 2's AI needs, so we generate them from the collision BSP |
| `shdr` family (`senv`, `soso`, `schi`, `scex`, `swat`, `sgla`, `smet`, `spla`) | `shad` on existing `stem` templates | each Halo 1 shader type maps to the closest Halo 2 template in the shared maps, with its parameters. The look will be close, not exact |
| `bitm` | `bitm` | the same Xbox formats and layouts: pixels copy unchanged (**Built**, [h1-bitmaps.md](../formats/h1-bitmaps.md)) |
| `mod2` + `coll` + `phys` | `mode` + `coll` + `phmo` under an `hlmt` | physics uses Halo 2's (decision 1) |
| `antr` | `jmad` | animation codecs differ: decode, then re-encode |
| `bipd`, `vehi`, `weap`, `eqip`, `scen`, `mach`, `ctrl` | the same groups | field-by-field. Halo 1 tuning values carry over where Halo 2 has the field |
| `actr` + `actv` | `char` | |
| encounters, squads, firing positions, command lists | squads, zones, firing positions, orders | |
| scripts | Halo 2 scripts | decompile Halo 1's syntax nodes, rename the functions that differ, compile to Halo 2 nodes with `crates/hs`. Builtins Halo 2 lacks go in the Rust HS VM ([hs_vm.md](../engine/hs_vm.md)). `crates/h1scripts`, [h1-scripts.md](../formats/h1-scripts.md) |
| `snd!` | `snd!` + `ugh!` | both Xbox ADPCM |
| `ustr`, `hmt ` | string IDs, `unic` | |
| recorded animations | recorded animations | versions differ |

## 3. Order of work

Each step is proven with tests; the user plays the result.

1. **Import and read.** Recognise the Halo 1 disc (title ID `4D530004`),
   pull out its maps, decompress them, list their tags.
   **Built:** `crates/h1map`, `h2 h1 info|tags|bitmap|convert`,
   `h2 import halo1`; format in
   [h1-map-format.md](../formats/h1-map-format.md). The disc's multiplayer maps
   are read and converted in tests ([h1-multiplayer.md](h1-multiplayer.md)).
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
   **Built, offline proof only** (§9): `h2 h1 convert <iso> a30`. Needs a
   game run.
4. **Objects.** Scenery, machines and doors, weapons, vehicles, equipment.
   **Built for weapons, equipment, vehicles with a template, and the
   player biped** (§9); scenery, machines, placed bipeds not yet.
5. **AI.** Characters, encounters to squads, generated pathfinding.
   **Built** (§9): `h1ai` on retail Halo 2 characters, pathfinding from
   `crates/pathfind` in every converted BSP.
6. **Scripts.** **Built, stubbed** (§9): `h1scripts` with
   `Missing::Stub`, hooks resolved to the placed names, trigger volumes,
   squads and the sounds the scripts name.
7. **Cutscenes, sound and dialogue, text and HUD, checkpoints.**
8. **Menus.** The campaign picker and its level list (§4), and progress
   saved per campaign. **Built** ([multiple-campaigns.md](multiple-campaigns.md)),
   confirmed in the game.

## 4. More than one campaign

**Built, confirmed in the game:** [multiple-campaigns.md](multiple-campaigns.md).
In short: the level lists in `mainmenu.map`'s globals (`+0x170` start
levels, `+0x178` the level select's campaigns and levels) are keyed by
campaign id and have free slots, and looking a level up, starting it and
finding the next one work for any campaign id as data. Three things
assume campaign 1 and are hooked: the level select's list is built for
campaign 1, every writer of the menu's selected campaign writes 1, and the
level select finds a level's record by map id alone (so map ids are
unique across campaigns). Profiles keep progress as bare map ids, so other
campaigns' progress is kept in `<data>/campaigns/progress.toml`. A
campaign is `<data>/campaigns/<key>/campaign.toml`; Halo 1's lists a10,
a30, a50, b30, b40, c10, c20, c40, d20 and d40. A picker screen comes
before the main menu's level select. The VFS serves a converted level's
map (`<data>/campaigns/halo1/<map>.map`) as `maps\<map>.map`.

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

**Built** (tests only): the `h1-health` feature and its switch,
`boot::health::set_mode`. Nothing calls the switch yet, so the model is
on only through the debug override. Where Halo 2 regenerates the body,
how its own health powerup (`eqip` type 5) refills it, the HUD's body
input, what the converter must write, and the debug switch
`H2_HEALTH_MODEL=halo1`: [health.md](health.md).

## 7. Multiplayer

Halo 1's multiplayer maps follow the same rules, with a sandbox setting:
[h1-multiplayer.md](h1-multiplayer.md).

## 9. a30 on a pinned base (**Built**, tests only)

`crates/h1campaign`; `h2 h1 convert disc/halo1.iso a30 --out DIR
[--disc disc/halo2.iso] [--base MAP] [--dry-run] [--force]` writes
`DIR/a30.map` and `DIR/campaign.toml` (Halo 1's manifest,
`content::campaign_list::HALO1`, unless one is there already). With
`DIR = <data>/campaigns/halo1` the campaign picker's Halo 1 entry lists
a30 and `vfs::CampaignMapsBackend` serves it as `maps\a30.map`; the map's
header name is `a30` and its scenario path `scenarios\solo\a30\a30`, the
path a30's start record names.

**Pinned, not fresh (the user's stopgap).** A fresh map needs every tag
moved or built, and `tags::schema::Relocator` can't move `scnr` or `sbsp`
yet; a pinned base is reachable now. The base is **`05a_deltaapproach`**
(Delta Halo): Covenant elites, grunts and jackals against marines,
outdoors on a ring, so a30's AI finds every retail character it maps to
but the crewman (the marine stands in); it holds every Halo 2 template
`h1objects` needs for a30's weapons, equipment and vehicles; and of the
Covenant-and-marines levels it has the most tag memory to spare
(`0xaed000` of `0xc24000`; 03a has `0xb4a000`, 08a lacks the SMG,
magnum, warthog). New tags are **added** to the pinned set (index
growth, tail kept: the entry table moves to the metadata end).

What each layer writes, from the base map up:

| Layer | Written | Left out |
|---|---|---|
| Structure | Each Halo 1 BSP into the base's BSP slot of the same index, with `h1convert::structure` (collision, render geometry, materials matched to the base's shaders, Halo 1's lightmap as a new `bitm`, MOPP) and pathfinding generated from the collision at `sbsp+0xc4` | Halo 1 BSPs past the base's count (none for a30: 2 of 3 slots) |
| Sky | Halo 1's skies (`h1sky`), every cluster on the first | |
| Scenario | The base `scnr` rebuilt as a built `0x3e0`-byte root whose untouched block fields keep their raw addresses; the old bytes stay in place as unowned metadata. BSP descriptors through `FixupKind::Bsp` | |
| Player starts | a30's starting locations, each on the base's first element | |
| Objects | Every base placement removed. Weapons, equipment and vehicles from a30's palettes through `h1objects` (palettes keep Halo 1's order, a null entry for one that doesn't convert), placements with unique IDs `0x4831xxxx`, origin BSP -1, source 1. The cyborg is converted. Object names are Halo 1's, each pointing at its converted placement or -1 | Scenery (702) and placed bipeds (21): no `h1objects` path. Pelican, dropship, lifepods: no template. The cyborg is not yet the player's biped (the globals name the base's) |
| Trigger volumes | The base's parked (`h1convert::convert::PARKED`, object name -1); a30's 74 appended after them | |
| AI | `h1ai`: squad groups, squads, zones, orders, character palette. Characters the base lacks fall back to the marine (marine kinds) or the folder's first | |
| Scripts | `h1scripts` with `Missing::Stub`. Hooks: object, unit and vehicle names, `short`, team and difficulty as `0xffff0000 \| index`; trigger volumes as the index; AI squads as the index and squad groups as `0x40000000 \| index`; string IDs interned; sounds as the converted tag's datum (encodings V: every such atom in 05a's and 03a's retail scripts). Script references resolved by tag name | Cutscene flags and camera points, navpoints, device groups, looping sounds, damage effects, animation graphs, titles, platoon-level AI tokens: `none` |
| Sounds | The `snd!` tags the scripts name (`h1sounds`, onto the base's gestalt), halving the count until the gestalt's 16-bit indices fit | Every other a30 sound; `lsnd` (looping) |
| Text | Nothing | `h1text::Locales::patch_matg` would replace the base's English table, and the scripts' HUD text builtins are stubbed: no reader for Halo 1's text yet |

**Proof.** `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test
--release -p h1campaign --test disc -- --ignored --nocapture`
(`H1_LAYERS=none` for the structure alone). The test converts a30, reopens
it with `map::Map`, and checks: a valid checksum, name and scenario path;
the scenario resolves; every tag reference in the scenario's tree (walked
with `tags::schema`'s `scnr` layout over the whole metadata) names a tag of
its group; each BSP's descriptor leads to its region, its collision
decodes and validates, its pathfinding decodes and validates against the
collision's surface count; the first player start is a30's and stands on a
generated sector; the sky is a `sky `; every weapon, equipment and vehicle
placement's palette entry is a tag of that group; the AI decodes with
`h1ai::h2::ScenarioAi::read` and its indices check, every character is a
`char`; the scripts decode with `hs::Scenario::read` with the compiled
counts; `campaign.toml` lists a30 under the map's own path.
The object sweep (also run alone, over the retail base and over any
built map named by `$H1_SWEEP_MAP`, by `object_sweep_is_clean_on_the_retail_base`)
follows every object palette entry and AI character through its tag
references: each names a tag of its group with data, and each `hlmt`
has at least its render model's nodes and its physics model's
rigid-body nodes (h1-objects.md, Nodes). It starts from the skies too,
and checks every tag it reaches and every tag those name for root
blocks that are never empty on 05a (groups with at least 20 tags) yet
empty here: the engine indexes some without a count check (`mode +0x34`,
h1-models.md). Clean on 05a as shipped. It also prints placement scales:
05a leaves `+0x20` at 0 on 528 of 634 placements (32 of 34 weapons, all
45 equipment, all 4 vehicles), so the converter's 0 matches retail.
`h1_trigger_volumes_read` checks a30's trigger volumes (`scnr+0x360`,
`0x60` each, name at `+4`: printable names, unit forward and up vectors).

Counts on the owner's discs (2026-11-17):

| | a30 |
|---|---|
| Tags | 8,365 (8,274 in the base; 91 added: 2 lightmap bitmaps, skies and their models, 47 object tags, 38 sounds, the gestalt) |
| Metadata | `0xbc3000` of `0xc24000` |
| BSP 0 | 33 clusters, 41,018 vertices, 29,572 triangles, 33 materials, 19,138 collision surfaces, 32 lightmap pages, 7,764 sectors, 15,806 links |
| BSP 1 | 28 clusters, 40,303 vertices, 31,748 triangles, 34 materials, 23,187 collision surfaces, 27 lightmap pages, 10,960 sectors, 20,458 links |
| Player starts | 2, the first on a generated sector |
| Weapons | 19 of 19 placed (4 palette entries) |
| Equipment | 61 of 61 (5; the health pack on the frag grenade template) |
| Vehicles | 8 of 31 (warthog, banshee; pelican, dropship, lifepods dropped) |
| AI | 22 squad groups, 89 squads, 22 zones, 92 orders, 5 characters |
| Scripts | 139 scripts, 108 globals, 10,832 nodes, 1,419 hooks (305 left `none`), 583 missing calls stubbed, 39 of 60 references resolved |
| Sounds | 38 (every sound the scripts name), none left out |

**Needs a game run:**

- that the level loads at all: the rebuilt scenario root over pinned
  children, the moved index entry table, the base's decorators, lights and
  decals (not cleared: their scenario blocks aren't located), BSP 0's
  clusters against the base's per-cluster data;
- that the player spawns at a30's start and walks on the converted
  collision; whether the base's starting profile (its weapons) applies;
- the placement tail (`+0x34` on): zeros except the head; whether Halo 2
  needs a nonzero BSP policy or per-type tail for objects to spawn;
- AI on generated pathfinding; whether `ai_place` with the adapter's count
  0 places anyone (h1-scripts.md);
- the script hook encodings beyond the retail survey (squad index vs other
  AI kinds), and stubbed calls changing a30's flow (dialogue waits on
  `sound_impulse_time`, which is stubbed to 0);
- the health pack on a grenade template; the cyborg as the player (needs
  the globals' player biped, not located).

## 8. Open questions

- Memory: whether converted levels fit in Halo 2's tag memory
  (`max_metadata_size` for campaign maps is `0xc24000`). a30 has a big
  outdoor BSP.
- Which `stem` templates in the shared maps cover Halo 1's shader types.
- Generating pathfinding that Halo 2's AI accepts.
