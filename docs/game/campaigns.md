# Campaigns: Halo 1 in the Halo 2 engine (design)

Status: design, with parts **Built**. All ten levels convert to Halo 2
maps with their structure (up to three BSPs), sky, player starts, the
cyborg as the player, objects with scenery and placed bipeds, AI, scripts
and script sounds, proven offline (§9). a30 has run headless in the game
(no input, so AI and play are unproven); no level has been played yet.
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
   **Built** (§9): `h2 h1 convert <iso> a30` (and the other nine levels,
   up to three BSPs each); a30 ran headless in the game.
4. **Objects.** Scenery, machines and doors, weapons, vehicles, equipment.
   **Built for weapons, equipment, vehicles with a template, scenery,
   placed bipeds, and the cyborg as the player** (§9); machines and doors
   not yet.
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

## 9. The ten levels on a pinned base (**Built**, tests only)

`crates/h1campaign`; `h2 h1 convert disc/halo1.iso <level> --out DIR
[--disc disc/halo2.iso] [--base MAP] [--dry-run] [--force]` writes
`DIR/<level>.map` and `DIR/campaign.toml` (Halo 1's manifest,
`content::campaign_list::HALO1`, unless one is there already). With
`DIR = <data>/campaigns/halo1` the campaign picker's Halo 1 entry lists
the level and `vfs::CampaignMapsBackend` serves it as `maps\<level>.map`;
the map's header name is the level's and its scenario path
`scenarios\solo\<level>\<level>`, the path its start record names.

**Pinned, not fresh (the user's stopgap).** A fresh map needs every tag
moved or built, and `tags::schema::Relocator` can't move `sbsp` yet; a
pinned base is reachable now. New tags are **added** to the pinned set
(index growth, tail kept: the entry table moves to the metadata end).

**Base per level: `05a_deltaapproach` for all ten** (`h1campaign::base_for`).
Why (surveys `base_memory_and_vehicles`, `survey_bases`):

- It is the only campaign map with every vehicle template Halo 1's levels
  use but the lifepod: warthog, ghost, scorpion, banshee, pelican,
  phantom (for the dropship), wraith and `c_turret_ap`. 03b has most of
  them but not `c_turret_ap`, and `0xca000` less room; 05b, 04a and 07b,
  the ones with more room, lack the warthog, ghost and scorpion.
- It has Halo 2's marine, elite, grunt, jackal, drone and Chief bipeds and
  the AI characters a30 maps to (every base lacks the crewman).
- It is big enough once the conversion stops wasting it: every level fits
  (table below), b40 with `0x3000` to spare.

**Tag memory.** The bound (`map::writer::max_metadata_size`) counts the
index, the BSP space and the ordinary metadata. Two things made a50 and b40
overflow, measured by the per-group and appended-tag accounting in
`Report` (`groups`, `appended`, and `Error::TagMemory`, which names the
largest groups and scenario fields):

1. The rebuilt scenario went after the base's metadata while the base's
   old scenario bytes (`0x10b8ec`) stayed as unowned metadata. Now the
   base scenario is relocated as a block tree (`tags::schema::Relocator::
   to_built_emptied`) with the fields the conversion replaces emptied
   first (`scenario::replaced_fields`: placements, palettes, names,
   starts, trigger volumes, BSP descriptors, AI, scripts); only the
   arrays other tags keep inside its old range stay
   (`Relocator::pointed_into`: `0x32` bytes on 05a). The old range is a
   hole the new scenario fits in.
2. Small built tags split that hole and the rebuilt sound gestalt's (the
   last `0xb4b80` bytes), so the two big ones were appended. Built tags
   are now placed largest first (`TagSet::largest_built_first`).

With b40's repeated firing positions shared (h1-ai.md, Spawning), all ten
fit. b40 (`0xc21000`) and a50, c10 (`0xbe7000`) still append their
scenario; the rest grow nothing past the base's metadata.

What each layer writes, from the base map up:

| Layer | Written | Left out |
|---|---|---|
| Structure | Each Halo 1 BSP into the base's BSP slot of the same index, with `h1convert::structure` (collision, render geometry, materials matched to the base's shaders (Halo 1's own textures on copies of them come from `h1convert::textures`: `plan` before building, `Retexture::install` after the predicted-datum tags, then the returned templates and material indices into `Structure`; not called from `h1campaign` yet), Halo 1's lightmap as a new `bitm`, MOPP, audibility, sound clusters) and pathfinding generated from the collision at `sbsp+0xc4` | BSPs past the base's 3 slots: a10 6, a50 1, b40 10, c10 3, c20 1, c40 10, d20 2, d40 7 (a30, b30: none) |
| Sky | Halo 1's skies (`h1sky`), every cluster on the first | |
| Scenario | The base `scnr` relocated as a block tree, replaced fields emptied; BSP descriptors through `FixupKind::Bsp` | |
| Player | The level's starting locations, each on the base's first element. The cyborg is converted and set as the campaign players' unit (`matg +0x138` elements 0 and 1, `h1convert::globals::set_player_unit`); Halo 1 health follows from the map name (`boot::health`) | |
| Objects | Every base placement removed. Weapons, equipment, vehicles, scenery and placed bipeds from the level's palettes through `h1objects` (palettes keep Halo 1's order, a null entry for one that doesn't convert; templates in h1-objects.md), placements with unique IDs `0x4831xxxx`, origin BSP -1, source 1. Object names are Halo 1's, each pointing at its converted placement or -1 | Palette entries with no template or an unsupported model: lifepods, the cyborg's cinematic variant and the pilot (compressed animations), over shield and camouflage (no powerup on 05a) |
| Trigger volumes | The base's parked (`h1convert::convert::PARKED`, object name -1); the level's appended after them | |
| AI | `h1ai`: squad groups, squads, zones, orders, character palette. Characters the base lacks fall back to the marine (marine kinds) or the folder's first | Hunters, flood, sentinels, monitor: no character on 05a (`none`) |
| Scripts | `h1scripts` with `Missing::Stub`; `ai_place` with one argument. A startup script `h1_initial_encounters` places the encounters Halo 1 creates at load. Hooks: object, unit and vehicle names, `short`, team and difficulty as `0xffff0000 \| index`; trigger volumes as the index; AI squads as the index and squad groups as `0x40000000 \| index`; string IDs interned; sounds as the converted tag's datum (encodings V: every such atom in 05a's and 03a's retail scripts). Script references resolved by tag name | Cutscene flags and camera points, navpoints, device groups, looping sounds, damage effects, animation graphs, titles; AI names of squads left out (no starting locations): `none` |
| Sounds | The `snd!` tags the scripts name (`h1sounds`, onto the base's gestalt), halving the count until the gestalt's 16-bit indices fit | Every other sound; `lsnd` (looping) |
| Text | Nothing: the base's English table stays as it is. `h1text::Locales::extending` builds on the base's table (its rows and blob first, new rows after) for when Halo 1's text is converted | Halo 1's string lists and HUD messages; the scripts' HUD text builtins are stubbed |

**Proof.** `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test
--release -p h1campaign --test disc -- --ignored --nocapture`.
`every_campaign_level_converts` (`H1_LEVELS=a30,b40` for some) converts
all ten levels on their base, checks each checksum and runs the object
sweep on each; `every_level_s_scripts_run_on_the_retail_interpreter`
finds no unrunnable cast or builtin on any of the ten. `a30_converts_onto_its_base_and_reads_back` (`H1_LAYERS`,
`H1_SOUNDS`) reopens a30 with `map::Map` and checks: a valid checksum,
name and scenario path; every tag reference in the scenario's tree (walked
with `tags::schema`'s `scnr` layout over the whole metadata) names a tag of
its group; each BSP's descriptor leads to its region, its collision
decodes and validates, audibility and sound clusters check, its
pathfinding decodes and validates; the first player start is a30's and
stands on a generated sector; the sky is a `sky `; globals players 0 and
1 are the converted cyborg; every weapon, equipment, vehicle, scenery and
biped placement's palette entry is a tag of that group; the AI decodes
with `h1ai::h2::ScenarioAi::read` and its indices check; the scripts
decode with the compiled counts, and the startup script's `ai_place`
calls take one argument each, resolved to the squad groups it names;
every converted sound resolves through the gestalt; `campaign.toml` lists
a30. The object sweep (h1-objects.md; also over the retail base by
`object_sweep_is_clean_on_the_retail_base`) follows every palette entry
and AI character: references resolve with data, each `hlmt` covers its
render model's and physics model's nodes, and no root block retail
always fills is empty. Surveys behind the choices are in
`tests/survey.rs`.

Per level on the owner's discs (2026-11-17), all on 05a; placed / Halo 1
placements; the sweep clean on every level:

| Level | Tags added | Tag memory | BSPs | Weapons | Equipment | Vehicles | Scenery | Bipeds | Squads | Scripts (hooks left `none`) | Placed at start |
|---|---|---|---|---|---|---|---|---|---|---|---|
| a10 | 359 | `0xb0f000` | 3 of 9 | 29/29 | 28/32 | 18/56 | 273/274 | 105/107 | 114 | 186 (767 of 2,776) | 2 |
| a30 | 166 | `0xb0e000` | 2 of 2 | 19/19 | 61/61 | 26/31 | 701/702 | 20/21 | 89 | 140 (423 of 1,494) | 1 |
| a50 | 195 | `0xbe7000` | 3 of 4 | 4/4 | 113/117 | 18/18 | 738/738 | 53/53 | 309 | 210 (342 of 1,669) | 1 |
| b30 | 191 | `0xb0f000` | 3 of 3 | 15/15 | 50/58 | 20/27 | 714/714 | 14/14 | 84 | 85 (240 of 514) | 0 |
| b40 | 257 | `0xc21000` | 3 of 13 | 45/45 | 199/203 | 74/74 | 854/855 | 50/50 | 283 | 226 (274 of 1,322) | 1 |
| c10 | 365 | `0xbe7000` | 3 of 6 | 46/46 | 42/47 | 7/7 | 1,523/1,523 | 95/95 | 206 | 199 (259 of 1,249) | 2 |
| c20 | 111 | `0xb0e000` | 3 of 4 | 2/2 | 24/25 | 0/0 | 171/171 | 13/13 | 149 | 233 (121 of 837) | 3 |
| c40 | 297 | `0xb48000` | 3 of 13 | 14/14 | 67/67 | 42/42 | 836/837 | 25/25 | 226 | 180 (270 of 1,088) | 1 |
| d20 | 216 | `0xb21000` | 3 of 5 | 6/6 | 17/20 | 11/11 | 864/864 | 74/74 | 162 | 190 (183 of 856) | 1 |
| d40 | 320 | `0xb43000` | 3 of 10 | 74/74 | 106/107 | 20/20 | 293/293 | 81/81 | 169 | 298 (641 of 1,790) | 1 |

The bound is `0xc24000`; the base alone is `0xaed000`.

**Needs a game run:**

- every level but a30 loading at all, and a30 still loading with the
  relocated scenario, scenery and bipeds;
- the player as the cyborg (Halo 1's model and animations on the Chief's
  unit tuning) with Halo 1 health;
- AI spawning: the startup script and Halo 1's own one-argument
  `ai_place` calls; actors on generated pathfinding;
- scenery and placed bipeds on their templates (bipeds with no physics
  model where the template's would not fit);
- the pelican, phantom-for-dropship, wraith and turret templates under
  Halo 1 models;
- levels whose scripts switch to a BSP past the third (a10, a50, b40,
  c10, c20, c40, d20, d40);
- the placement tail (`+0x34` on): zeros except the head, scenery
  included.

## 8. Open questions

- Memory: all ten levels fit `0xc24000` on 05a (§9); b40 has `0x3000`
  to spare, so more b40 content needs a bigger hole (freeing the base's
  unreached tags) or less.
- BSPs past the base's three slots: adding descriptors and `sbsp`/`ltmp`
  tags is a builder change, but the base's per-BSP scenario data (cluster
  data and the like) may need one element per BSP.
- Which `stem` templates in the shared maps cover Halo 1's shader types.
- Generating pathfinding that Halo 2's AI accepts.
