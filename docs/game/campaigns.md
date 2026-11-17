# Campaigns: Halo 1 in the Halo 2 engine (design)

Status: design, with parts **Built**. All ten levels convert to Halo 2
maps with their structure (every BSP, and Halo 1's BSP switch volumes),
sky, player starts, the cyborg as the player, objects with scenery and
placed bipeds, AI (characters the base lacks copied from other retail
maps), scripts and script sounds, proven offline (§9). a30 has run headless in the game
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
| 5 | When conversion happens | **Offline, at import**: `h2 import halo1` writes Halo 2 `.map` files into the data folder, and `h2 play` runs it by itself when `halo1.iso` is next to the Halo 2 disc (§10). The engine loads them like any other level |

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
- **The import command**, `h2 import halo1 [disc/halo1.iso]`, converts
  every campaign level and every multiplayer map into the data root, with
  the campaign's level list (§4, §10). `h2 play`, `run`, `serve` and
  `play --follow` run it before booting when `halo1.iso` is beside the
  Halo 2 disc.

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
   every BSP each); a30 ran headless in the game, and with round 2's
   three-BSP cap so did a10, b30, c10, c20, c40, d20 and d40.
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
   Until cutscene animation and recordings are converted, the converter
   skips Halo 1's cinematics and a10's tutorial (§9, Cutscenes).
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
keep Halo 1's characters in both variants. For now they are skipped in
both (§9, Cutscenes).

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
  use: warthog, ghost, scorpion, banshee, pelican,
  phantom (for the dropship), wraith and `c_turret_ap`, and the pelican
  again for the lifepod (by role, h1-objects.md). 03b has most of
  them but not `c_turret_ap`, and `0xca000` less room; 05b, 04a and 07b,
  the ones with more room, lack the warthog, ghost and scorpion.
- It has Halo 2's marine, elite, grunt, jackal, drone and Chief bipeds and
  the AI characters a30 maps to (every base lacks the crewman). The
  hunter, flood and monitor it lacks come from donors
  (below).
- It is big enough once the conversion stops wasting it: every level fits
  (table below); b40 exactly (`0xc24000`, nothing to spare).

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
fit. Carrying every BSP costs little tag memory: a BSP region lives in
the file, and only the largest region has to fit the base's BSP space
(`0x394000`); each further BSP adds its lightmap `bitm`, its scenario
elements and nothing else to the metadata. b40 went from `0xc21000` (3
of 13 BSPs) to `0xc23000` (13 of 13, the hunter included; the copied
characters mostly fill holes in the base's metadata), and to `0xc24000`,
the bound exactly, with Halo 1's textures on its structure (10 to 25
shader copies a BSP, shared between BSPs by name). b40 (`0xc21000`) and a50, c10 (`0xbe7000`) still append their
scenario; the rest grow nothing past the base's metadata.

What each layer writes, from the base map up:

| Layer | Written | Left out |
|---|---|---|
| Structure | Every Halo 1 BSP, in Halo 1's order: into the base's BSP slot of the same index, past the base's three into new `sbsp`/`ltmp` tags under Halo 1's names (templates from the base's first), with `h1convert::structure` (collision, render geometry, materials matched to the base's shaders and drawing Halo 1's own textures on copies of them (`h1convert::textures`), Halo 1's lightmap as a new `bitm`, MOPP, audibility, sound clusters, each cluster's background sound and sound environment from Halo 1's ([h1-multiplayer.md](h1-multiplayer.md), "Ambient sound")) and pathfinding generated from the collision at `sbsp+0xc4` | |
| Sky | Halo 1's skies (`h1sky`), every cluster on the first | |
| Scenario | The base `scnr` relocated as a block tree, replaced fields emptied. One BSP descriptor per Halo 1 BSP (`FixupKind::Bsp`, its `sbsp` and `ltmp`), so `switch_bsp`, zone and placement BSP indices keep Halo 1's meaning. The two root blocks every retail map holds one element per BSP of (survey `per_bsp_scenario_blocks`): cluster data `+0x290` (per cluster its background sound and sound environment, as the structure's cluster `+0x72`/`+0x74` names them, and the cluster's centroid, and the BSP's checksum `sbsp+0x08`) and `+0x388` (no points). BSP switch trigger volumes `+0x130`: Halo 1's (`scnr+0x39c`, 8 bytes, survey `h1_bsp_switch_volumes`) on its converted volumes | The base's own BSP switch volumes |
| Player | The level's starting locations, each on the base's first element. The cyborg is converted and set as the campaign players' unit (`matg +0x138` elements 0 and 1, `h1convert::globals::set_player_unit`); Halo 1 health follows from the map name (`boot::health`) | |
| Objects | Every base placement removed. Weapons, equipment, vehicles, scenery and placed bipeds from the level's palettes through `h1objects` (palettes keep Halo 1's order, a null entry for one that doesn't convert; templates in h1-objects.md), placements with unique IDs `0x4831xxxx`, origin BSP -1, source 1. Object names are Halo 1's, each pointing at its converted placement or -1. A palette entry whose template only a donor brought places the retail object itself. Halo 1's "not placed automatically" (bit 0 of the `u16` at `+4`) is written as Halo 2's "not at start" (bit 0 of the `u32` at `+4`), so cutscene casts and props wait for the scripts that create them (a10: 67 of 107 bipeds, 51 of 56 vehicles, 92 of 274 scenery) | Palette entries with no template: over shield and camouflage (no powerup on 05a), a10's cryotube, chairs, fighterbomber and Halo ring, b30's falling box |
| Trigger volumes | The base's parked (`h1convert::convert::PARKED`, object name -1); the level's appended after them | |
| Donors | `h1campaign::donor`: each character `h1ai`'s table names, copied whole from the first donor that has it (`03a_oldmombasa`, `04b_floodlab`, `06a_sentinelwalls`) by `h1sounds::transplant`: the tag tree, its raw, and its sounds' gestalt entries | The sentinel (its animation graph does not relocate) and the crewman (no donor) |
| AI | `h1ai`: squad groups, squads, zones, orders, character palette. Characters the base lacks and no donor brought fall back to the marine (marine kinds) or the folder's first | Sentinels: `none` |
| Scripts | `h1scripts` with `Missing::Stub` and `Cutscenes::Skip` (below); `ai_place` with one argument. A startup script `h1_initial_encounters` places the encounters Halo 1 creates at load. Hooks: object, unit and vehicle names, `short`, team and difficulty as `0xffff0000 \| index`; trigger volumes as the index; AI squads as the index and squad groups as `0x40000000 \| index`; string IDs interned; sounds as the converted tag's datum (encodings V: every such atom in 05a's and 03a's retail scripts). Halo 1's cutscene flags, camera points, titles and device groups written to Halo 2's blocks in Halo 1's order (`h1scripts::level`), their atoms the index (a device group's runtime datum); the seat-mapping table `+0x228` built from the vehicles' seat labels (`h1scripts::seats`); navpoints `default`; looping sounds, effects, damage and animation graphs by tag name among the converted tags. Squads with no starting locations that the scripts name are kept empty (`h1ai`), so their names resolve. Script references resolved by tag name ([h1-scripts.md](../formats/h1-scripts.md), "Level data the scripts name") | Looping sounds left out for room (Sounds, below), effects, damage effects, animation graphs and object definitions with no converted tag of that name: `none` |
| Sounds | The `snd!` tags the scripts name (`h1sounds`, onto the base's gestalt), halving the count until the gestalt's 16-bit indices fit; each permutation's length at `+6`, which the scripts' waits read (`sound_impulse_time` is Halo 2's `sound_impulse_language_time`). `h1sounds::audit` reads every sound of a converted level back: a10's 3,988 play exactly what they were made from. The looping sounds (`lsnd`: music and ambience) the scripts start, stop or switch (`sound_looping_start`, `_stop`, `_set_alternate`), with their tracks' sounds (`h1sounds::looping`), in first-use order while they fit the map file (Music, below). Every BSP's background sounds and sound environments (`h1convert::ambience`, all BSPs' together) as the scenario's palettes `+0x250`/`+0x258` and each structure's copy (`sbsp+0xd4`/`+0xdc`) | Every other sound; looping sounds past the map file's size limit; lifepods drop the pelican template's engine loops |
| Text | Nothing: the base's English table stays as it is. `h1text::Locales::extending` builds on the base's table (its rows and blob first, new rows after) for when Halo 1's text is converted | Halo 1's string lists and HUD messages; the scripts' HUD text builtins are stubbed |

### Cutscenes (skipped by default)

Halo 1's cinematics need what is not converted yet: their characters'
custom animations, recordings, camera work over Halo 1's sets, and
dialogue timed by `sound_impulse_time`. Played as they are, a10 sat in
its x10 intro in the space BSP (about two minutes of fixed sleeps, a
black screen, a few placed humans floating there), and then, on normal
difficulty in single player, in the cryo tutorial: `tutorial_setup`
turns input off and `tutorial_looking` waits on
`player_action_test_look_relative_left` and `_right`, both stubbed
`false`, so control never came back.

`h1campaign::Options::cutscenes` (default `Cutscenes::Skip`;
`Cutscenes::Play` compiles the scripts as written):

- Every call of the level's `cinematic_skip_start` script, which guards
  every skippable cinematic as `(if (cinematic_skip_start) (x10))`, is
  compiled as `false`. That is the path Halo 1 takes when the player
  skips (the guard is false after the revert), so each level's own skip
  handling runs: the fade in, `cinematic_stop` and control back. Guards
  per level: a10 3, a30 4, a50 3, b30 2, b40 1, c10 2, c20 1, c40 2,
  d20 1, d40 2; the intros of a10, a30, c20, c40, d20 and d40 among them.
- `(wake tutorial_setup)` wakes `fast_setup` (`SHORT_PATHS`; only a10
  has both): the cryo exit Halo 1 runs in co-op and on other
  difficulties, which fades in, ends its cinematic and gives control.
- Not skippable in Halo 1 and played here: a50's, b30's, b40's and
  c10's insertions. Each is fixed sleeps (about 15 to 30 seconds) and
  gives control back at its end; none waits on a stub.

Stubbed status queries that scripts poll return a "done" value instead
of zero (`DONE_VALUES`, every use in the ten levels read):
`ai_conversation_status` 5, `ai_command_list_status` 2,
`game_all_quiet` and `game_safe_to_speak` true. The four waits on
`(= 1 (ai_command_list_status ...))` (a10, b40, c20, d40) still hold.

### Waits on stubs (playing a level through)

`h1scripts`' disc test `waits_on_stubs` counts, per level, the
`sleep_until` and `if` conditions that read a stubbed builtin, and which
of them nothing handles (a stub not in `DONE_VALUES`, not `ai_status`,
and not one whose zero is already the done value: `recording_time`,
`ai_going_to_vehicle`). What reads a stub most, over all ten levels (uses, before this round):

| Stub | Uses | Now |
|---|---|---|
| `ai_status` | 35 (a50 16, b40 17, b30 2) | `ai_combat_status` on Halo 1's scale (h1-scripts.md, Step 2) |
| `ai_conversation_status` | 34 | done value 5 |
| `ai_command_list_status` | 20 | done value 2 |
| `game_all_quiet` | 17 | done value true |
| `ai_living_fraction` | 14 (a30) | `ai_strength` |
| `device_get_power` | 8 (d40) | Halo 2's unnamed builtin 255 (`INFERRED`) |
| `player0_joystick_set_is_normal` | 7 | `false` (help text only) |
| `game_time` | 6 (a30) | `0`, or'd with a seat test |
| `game_safe_to_speak` | 5 | done value true |
| `ai_going_to_vehicle` | 5 | `0` (none is going) |
| `sound_impulse_time` | 4 (a10) | `sound_impulse_language_time` (the ticks the line has left) |
| `hud_get_timer_ticks` | 3 (d40) | done value 32767: the escape timer never runs out |
| `ai_conversation_line` | 1 (b30) | done value 99 |

`ai_living_count`, `ai_strength`, `unit_get_health`, `volume_test_objects`
and `objects_can_see_object`/`_flag` translate directly; they are not
stubs.

Waits still on an unhandled stub, by level (2026-11-17):

- a10: 11 `sleep_until` and 6 `if`, all in the cryo tutorial (the
  `player_action_test_*` look and move tests, joystick and pitch help
  text), which the cutscene skip never runs.
- a30: 7 and 2: `game_time` (6, each or'd with a test that ends it when
  the player boards), the night vision tutorial
  (`unit_solo_player_integrated_night_vision_is_active`), joystick help.
- a50: 1, `ai_allegiance_broken` (the traitor branch: `false`, the
  marines stay friendly).
- b40: 2 `if`s on joystick help text; c40: 1.
- b30, c10, c20, d20, d40: none.

Placed objects Halo 1 leaves to the scripts are no longer created at
load (Objects, above): a10's x10 crew in the space BSP (`keyes_x10_space`,
`space_crew_1` to `_3`, at about (500, -147, -28)), Keyes and the bridge
crew, the hangar marines. Skipped cinematics never create them.

Proof: `skipped_cutscenes_take_halo1_s_skip_path` (h1scripts, synthetic);
`skipped_cutscenes_on_every_level` (h1scripts disc: no guard call left
on any level, a10's wake names `fast_setup` by index);
`every_level_s_scripts_run_on_the_retail_interpreter` with the skip on;
`a30_converts_onto_its_base_and_reads_back` checks every placement's
flags against Halo 1's (a30: 31 vehicles, 2 scenery, 7 bipeds not at
start); `h1_placement_flags` and `h1_initial_encounters` (survey) print
the evidence.

**Proof.** `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test
--release -p h1campaign --test disc -- --ignored --nocapture`.
`every_campaign_level_converts` (`H1_LEVELS=a30,b40` for some) converts
all ten levels on their base, checks each checksum, runs the object
sweep on each, and checks every BSP of each (`check_bsps`, below: 69
BSPs, a10's 9 and b40's 13 included) and its BSP switch volumes (each on
a converted volume, between BSPs it has);
`every_level_s_scripts_run_on_the_retail_interpreter` finds no
unrunnable cast or builtin on any of the ten, and checks that Halo 1's
`switch_bsp` translates directly and that every `switch_bsp` in each
level's scripts names a BSP the converted scenario has (descriptor count
equal to Halo 1's BSP count). `a30_converts_onto_its_base_and_reads_back` (`H1_LAYERS`,
`H1_SOUNDS`) reopens a30 with `map::Map` and checks: a valid checksum,
name and scenario path; every tag reference in the scenario's tree (walked
with `tags::schema`'s `scnr` layout over the whole metadata) names a tag of
its group; one descriptor, cluster data element and `+0x388` element
per Halo 1 BSP; each BSP's descriptor names an `sbsp` and an `ltmp` and
leads to its region (no two regions overlap), its collision decodes and
validates, audibility and sound clusters check, its pathfinding decodes
and validates, and its cluster data has one entry per cluster in each
per-cluster block and its checksum; the first player start is a30's and
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

Per level on the owner's discs (2026-11-17, round 3), all on 05a; placed /
Halo 1 placements; BSPs with Halo 1's BSP switch volumes in brackets; the
sweep clean and every BSP checked on every level. Copied from donors: the hunter on a50, b30, b40, c40,
d20, d40; flood combat forms, carrier, infection and the monitor on c10,
c20, c40, d20, d40 (by what each level's AI uses):

| Level | Tags added | Tag memory | BSPs (switch volumes) | Weapons | Equipment | Vehicles | Scenery | Bipeds | Squads | Scripts (hooks left `none`; of them `ai`) | Placed at start |
|---|---|---|---|---|---|---|---|---|---|---|---|
| a10 | 490 | `0xb2d000` | 9 (12) | 29/29 | 28/32 | 23/56 | 273/274 | 107/107 | 114 | 186 (767 of 2,776; `ai` 47) | 2 |
| a30 | 238 | `0xb29000` | 2 (2) | 19/19 | 61/61 | 31/31 | 701/702 | 21/21 | 89 | 140 (423 of 1,494; `ai` 161) | 1 |
| a50 | 432 | `0xbe8000` | 4 (3) | 4/4 | 113/117 | 18/18 | 738/738 | 53/53 | 309 | 210 (342 of 1,669; `ai` 104) | 1 |
| b30 | 433 | `0xb16000` | 2 (2) | 15/15 | 50/58 | 20/27 | 714/714 | 14/14 | 84 | 85 (240 of 514; `ai` 76) | 0 |
| b40 | 542 | `0xc24000` | 13 (32) | 45/45 | 199/203 | 74/74 | 854/855 | 50/50 | 283 | 226 (274 of 1,322; `ai` 88) | 1 |
| c10 | 678 | `0xbec000` | 6 (14) | 46/46 | 42/47 | 7/7 | 1,523/1,523 | 95/95 | 206 | 199 (259 of 1,249; `ai` 42) | 2 |
| c20 | 430 | `0xb2d000` | 4 (6) | 2/2 | 24/25 | 0/0 | 171/171 | 13/13 | 149 | 233 (121 of 837; `ai` 0) | 3 |
| c40 | 830 | `0xbc8000` | 13 (31) | 14/14 | 67/67 | 42/42 | 836/837 | 25/25 | 226 | 180 (270 of 1,088; `ai` 5) | 1 |
| d20 | 703 | `0xb9e000` | 5 (4) | 6/6 | 17/20 | 11/11 | 864/864 | 74/74 | 162 | 190 (183 of 856; `ai` 5) | 1 |
| d40 | 918 | `0xbd1000` | 10 (13) | 74/74 | 106/107 | 20/20 | 293/293 | 81/81 | 169 | 298 (641 of 1,790; `ai` 34) | 1 |

The bound is `0xc24000`; the base alone is `0xaed000`.

### Engine caps and unchecked indices

- **Squads.** The AI's squad array at load holds 335 (`0x00200974`,
  `mov ebx,14Fh`, `0x98` per datum); `0x00200b30` takes one datum per
  scenario squad (`+0x160`, `0x74` each) and a squad past the cap faults
  at `0x00200c44` (a50 did, with 384). Squad groups have their own array
  of 100 (`0x002009ad`). `h1campaign::scenario::MAX_SQUADS` and
  `MAX_SQUAD_GROUPS`; the converter refuses a level over either. Squads
  with starting locations are always kept; the empty ones the scripts
  name (migration and attach targets, mostly) only while the total fits.
  a50 (309 placed + 75 named empty) and b40 (283 + 67) leave them out,
  and the scripts' names for them resolve to a sibling squad (the
  encounter's first converted one), or the encounter's squad group if it
  has none. A squad group is no stand-in for a squad: Halo 2's
  `ai_migrate` ignores a destination with either top bit set
  (`0x002a54cc`), and `ai_attach_units` reads the low 16 bits as a squad
  index (`0x0027305e`) (h1-ai.md). Every other level fits with them (the most: c40, 227).
  Survey: `ai_counts_against_the_caps`, `empty_squad_uses`.
- **Cutscene flags, camera points, titles and device groups.** The
  engine indexes these blocks without a check (b40 faulted at
  `0x002a003f` in `object_teleport`, reading flag -1): a call naming one
  that does not resolve is stubbed (h1-scripts.md, Indexed values).
  `every_level_s_scripts_run_on_the_retail_interpreter` checks every
  reached atom of these types against its converted block on all ten
  levels. Names resolve without case (b30 and d40 named 3 and 9 flags
  and a device group in another case and lost them).
- **b40's level data.** b40 is at the tag memory bound. A level over
  it is converted again leaner (`h1campaign::convert`), dropping in
  turn: the empty squads kept for their names, the seat table, Halo 1's
  textures on object models, the music, the level data. b40 still
  converts only without its level data (`0xc22000` of `0xc24000`): its
  74 calls naming a flag, camera point, title or device group are
  stubbed (the insertion's teleports and camera moves among them), and
  it has no music.
- **Music and the map file.** Halo 2's campaign map file is capped at
  `0x11800000` bytes (`map::writer::cache_size_limit`, the header
  validator's table at `0x00214ed0`). The base's file (its own raw
  data kept whole) with each level's BSPs and sounds leaves from under
  1 MB (d40) to about 50 MB (a50) for music, which Halo 1 ships as Xbox
  ADPCM, kept as it is. The looping
  sounds go in in the order the scripts first name them while their
  samples fit: a level over the cap (`Error::FileSize`) is converted
  again with that many fewer music bytes (and 256 KiB margin), once on
  every level so far. Per level, looping sounds in / named (2026-11-17,
  `every_campaign_level_converts`): a10 4/25, a30 16/16, a50 20/24, b30
  14/14, b40 0/16 (tag memory), c10 11/15, c20 7/8, c40 8/19, d20
  14/20, d40 2/29. A track left out leaves its `sound_looping_start`
  hook `none` (Halo 2 plays nothing). Fitting the rest needs room in
  the file: dropping the base's raw data no converted tag reaches (its
  own sounds, bitmaps and models) is the likely next step.

Hooks left `none` after the level data, seat table and kept squads
(2026-11-17, `h1scripts` round 2; the table's columns are from round 3,
before them): a10 262, a30 152, a50 76, b30 153, b40 274 (converted
without them: at the bound), c10 65, c20 46, c40 87, d20 64, d40 194; 0
unrunnable on every level.

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
- every BSP past the third (new `sbsp`/`ltmp` tags and descriptors, the
  rebuilt cluster data), reached by the scripts' `switch_bsp` and by
  Halo 1's BSP switch volumes, which Halo 2's engine is expected to
  switch on as it does for its own;
- the copied characters (hunter, flood, monitor) and their sounds, and
  lifepods on the pelican template (h1-objects.md), flown by a30's
  recordings;
- the placement tail (`+0x34` on): zeros except the head, scenery
  included.

## 10. Importing (**Built**: `cli::h1_import`)

### Playing Halo 1 (the user's steps)

1. Put your Halo: Combat Evolved disc image next to the Halo 2 one, named
   `halo1.iso`: `disc/halo1.iso` beside `disc/halo2.iso` (another name
   works if it is the only other Halo 1 image there; `mappack.iso` is never
   taken for it).
2. `cargo h2 play disc/halo2.iso` (or `play --follow`, `run`, `serve`).
   The first start converts the disc before the window opens, a line per
   map on stderr, `halo1: [3/36] campaign a50: done (5.4 s)`. On the
   owner's discs (2026-11-17) that is 36 maps (ten levels, 13 multiplayer
   maps in two sandboxes) in about 90 s of a release build, the disc's
   hash included. Later starts print `halo1: up to date (36 maps from
   halo1.iso)` and go straight on.
3. Sign in, **Campaign**, then **Halo: Combat Evolved**, the picker's first row (Halo 2 is second):
   the ten levels, a10 to d40, as far as the profile has got. Halo 1's
   multiplayer maps are `h1_<map>` in the multiplayer map list (Custom
   Game, Xbox Live), named as in Halo 1 (docs/game/h1-multiplayer.md
   "Importing"). `--level a30` starts a converted level directly.

`--no-halo1` or `H2_HALO1=0` (also `false`, `no`, `off`) skips the check.
A failure never stops the game: a map that fails is a line,
`halo1: [5/36] campaign b40: failed: <why>`, and the others are still
there; a problem before any map (an unreadable disc, a bare `default.xbe`
as the game disc) is `halo1: not imported: <why>`.

By hand, the same import: `cargo h2 import halo1 [disc/halo1.iso]
[--data DIR] [--disc disc/halo2.iso] [--force] [--dry-run]
[--copy-source]`. `--data` defaults to the data root `h2 play` uses
(`$TMPDIR/h2boot-data`); `--disc` (the Halo 2 disc, for the bases) to
`$H2_DISC`, else `halo2.iso` beside the Halo 1 disc, else
`disc/halo2.iso`. It exits non-zero when a map failed, after trying every
map. `--copy-source` also copies the disc's campaign maps unchanged to
`<data>/campaigns/halo1/source/` with a SHA-256 manifest (the old step 1).

### What it writes

| Job | Output | Read by |
|---|---|---|
| each of a10 .. d40 | `<data>/campaigns/halo1/<level>.map`, `campaign.toml` (`content::campaign_list::HALO1`) | the campaign picker and lists (`boot::campaigns`), `vfs::CampaignMapsBackend` as `maps\<level>.map`, `--level` (`level_boot::resolve_in`) |
| each Halo 1 multiplayer map on the disc (`h1convert::convert::MAPS`), Halo 2 then Halo 1 sandbox | `<data>/maps/h1_<map>/{h2,h1}/h1_<map>.map`, `manifest.toml` | `boot::new_maps`, the `multiplayer.halo1_sandbox` setting picks the variant |

Campaign levels go on their pinned base (§9); multiplayer maps on their
default base, pinned (`h2 h1 convert` without `--fresh`).

### The stamp, and when it converts again

`<data>/campaigns/halo1/import.stamp` holds the Halo 1 disc's SHA-256,
size and modification time, the converter version, and each map's
outcome, written after every map (an interrupted import resumes). A start
converts what is missing: a map not in the stamp, or stamped done but
whose file is gone. Everything is converted again when:

- **the converter version changes**: `CONVERTER_VERSION` in
  `crates/cli/src/h1_import.rs`. **Bump it in the commit that changes what
  the Halo 1 converters write** (h1convert, h1campaign, h1objects, h1ai,
  h1scripts, h1sounds, h1bitmaps, h1models, h1sky, h1text, or the map
  builder under them). `--follow` builds each new commit's `h2 play`, so
  after such a merge the next F5 converts again with the new code;
- the disc is another one (its hash; it is hashed again only when its size
  or modification time changed);
- `--force`, or the stamp is deleted.

A map that failed is not tried again until one of those: a failure from
a converter bug stays a failure until the converter changes.

**Tests.** `cli` `h1_import::tests` (12): the job list (campaign in play
order, then the multiplayer maps the disc has, both sandboxes); where each
writes (the folders `boot::new_maps` and the campaigns read); the stamp
reads back; an unchanged disc is not hashed again and keeps its outcomes,
another disc or converter version starts again; what is pending; a map
that fails or panics is recorded, the stamp saved after each, the rest
converted; `H2_HALO1`/`--no-halo1`; finding `halo1.iso` (any case, or a
Halo 1 image by its XBE) and never the game disc or `mappack.iso`; no
Halo 1 disc, or a game "disc" that is no image, writes nothing.
`boot` `campaigns::tests::a_halo1_import_is_picked_started_and_served`:
an import's folder (stamp and `source/` included) lists Halo 1 in the
picker after Halo 2 with the ten levels in order and named; each level's
start record, looked up as `0x19c270` does after the lists are extended,
names a scenario path `0x214db0` turns into `d:\maps\<level>.map`, and
`CampaignMapsBackend` serves exactly the converted file there.
`level_boot::tests::an_installed_campaign_level_resolves_by_map_or_scenario`:
`--level a30` gets the campaign and map ids the lists give it.
Against the discs (ignored): `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso
cargo test --release -p cli --bin h2 h1_import::tests::real -- --ignored
--nocapture` imports everything into a scratch data root, imports again
(nothing to do), and checks `boot::new_maps` lists every `h1_<map>`, the
picker's Halo 1 lists the ten levels, the VFS serves each level's map, and
`--level` resolves each (`H1_JOBS=a30,bloodgulch` for some).

**Found missing, and fixed:** `h2 play --level` and `H2_LEVEL` knew only
the disc's levels, so a converted level could only be reached through the
menus. `level_boot::resolve_in` also looks in the campaigns installed in
the data root, with the ids `boot::campaigns` gives them; `h2 play` imports
before it resolves the level.

## 8. Open questions

- Memory: all ten levels fit `0xc24000` on 05a with every BSP and Halo
  1's textures (§9); b40 fits exactly, with nothing to spare, so more b40 content needs a bigger hole (freeing
  the base's unreached tags) or less. Its zones (`scnr+0x168`,
  `0xce898` bytes, mostly firing positions) are the scenario's largest
  field.
- The sentinel: `tags::schema` refuses `sentinel_aggressor`'s animation
  graph, which every sentinel character but the enforcer uses.
- Which `stem` templates in the shared maps cover Halo 1's shader types.
- Generating pathfinding that Halo 2's AI accepts.
