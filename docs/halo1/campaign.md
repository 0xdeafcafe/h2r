# Campaigns: Halo 1 in the Halo 2 engine (design)

Status: **Partly built.** All ten levels convert to Halo 2
maps with their structure (every BSP, and Halo 1's BSP switch volumes),
sky, player starts, the cyborg as the player, objects with scenery and
placed bipeds, AI (characters the base lacks copied from other retail
maps), scripts and script sounds, proven offline (§9). a30 has run headless
in the game (no input, so AI and play are unproven); no level is verified
by play.

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

- **`crates/tools/h1map`** reads Halo 1 Xbox cache files: version 5, a 0x800-byte
  header, then a zlib stream holding the rest of the file. It knows the tag
  index and the Halo 1 tag layouts the converter needs.
- **The map builder** (in `crates/formats/map`) writes a complete v8 cache file from
  a tag set: header, tag index, string IDs, metadata, raw resources and BSP
  slots. `MapEditor` patches an existing map; the builder writes one from
  nothing ([map-builder.md](../formats/map-builder.md),
  [maps.md](../game/maps.md)).
- **`crates/tools/h1-assets`** (Built) converts Halo 1 `mode` and `antr` tags to
  Halo 2 `mode` and `jmad` block trees for the builder
  ([models.md](models.md)).
- **`crates/tools/h1-assets`** (Built) decodes Halo 1 `bitm` images to RGBA8
  with Halo 2's texture decoder and converts them to Halo 2 `bitm` block
  trees, pixels copied as they are ([bitmaps.md](bitmaps.md)).
- **The converter** (`crates/tools/h1convert`) works one tag group at a time, in
  the order of §3: collision, structure BSP, placements, MOPP
  (`h2 h1 convert`), see [multiplayer.md](multiplayer.md).
  **`crates/tools/h1campaign`** puts it and the other Halo 1 crates together for
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
| `shdr` family (`senv`, `soso`, `schi`, `scex`, `swat`, `sgla`, `smet`, `spla`) | `shad` written from its definition on a retail `stem` | each Halo 1 shader type (and flags: alpha-tested `senv`, additive `schi`/`sotr`) picks a stem in `shared.map` (sky layers: the sky stems where the base has them) and `h1convert::shad` fills its parameters from Halo 1's maps and tiling; no Halo 2 `shad` bytes are copied. An `senv` with detail maps draws on `tex_bump_detail_blend_detail` where the base has it (Coagulation, Beaver Creek, five campaign maps): the base alpha blends the primary (alpha 0) and secondary (alpha 1) details, then the micro detail, each a biased multiply, as Halo 1 does; elsewhere `tex_bump` with the primary detail. A normal (unblended) `senv` with a reflection cube map (`+0x324`) and a reflection brightness draws on `tex_bump_env`: Halo 1's cube map, its perpendicular and parallel colours (`+0x2a8`, `+0x2b4`) times the specular brightness (`+0x290`) as the facing and grazing tints, and the perpendicular and parallel brightnesses (`+0x2f4`, `+0x2f8`) as the facing and grazing brightness (`env_reflection_is_halo1s_rule`); a blended one keeps its blend without the reflection. Other specular colours are black (a Halo 1 base alpha is a blend or cut-out mask). Proof: `h1convert/tests/shad_rebuild.rs` (`detail_blend_is_halo1s_rule` runs the stem's colour pass on the CPU against Halo 1's rule). Water (`swat`) with a ripple map draws on `water_pool` where the base has it (Delta Approach, Beaver Creek, Coagulation): Halo 1's ripple map (`+0xc8`) as the bump, tiled by the ripple scale (`+0xc4`) and scrolling at the ripple velocity (`+0xc0`) along the ripple angle (`+0xbc`), and its reflection map (`+0x9c`), both offset by the ripple over the scene behind the water and weighed by retail's fresnel ramp (`a_swat_draws_on_water_pool_its_ripple_scrolling_along_the_ripple_angle`); elsewhere `one_alpha_env` with its base map. Halo 1's four extra ripple layers (`+0x124`), base map and tints have no place on `water_pool`. Not converted yet: model (`soso`) reflections (`soso +0x164`). The look will be close, not exact |
| `bitm` | `bitm` | the same Xbox formats and layouts: pixels copy unchanged (**Built**, [bitmaps.md](bitmaps.md)) |
| `mod2` + `coll` + `phys` | `mode` + `coll` + `phmo` under an `hlmt` | physics uses Halo 2's (decision 1) |
| `antr` | `jmad` | animation codecs differ: decode, then re-encode |
| `bipd`, `vehi`, `weap`, `eqip`, `scen`, `mach`, `ctrl` | the same groups | field-by-field. Halo 1 tuning values carry over where Halo 2 has the field |
| `actr` + `actv` | `char` | |
| encounters, squads, firing positions, command lists | squads, zones, firing positions, orders | |
| scripts | Halo 2 scripts | decompile Halo 1's syntax nodes, rename the functions that differ, compile to Halo 2 nodes with `crates/formats/hs`. Builtins Halo 2 lacks go in the Rust HS VM ([hs_vm.md](../engine/hs_vm.md)). `crates/tools/h1campaign`, [scripts.md](scripts.md) |
| `snd!` | `snd!` + `ugh!` | both Xbox ADPCM |
| `ustr`, `hmt ` | string IDs, `unic` | |
| recorded animations | recorded animations | versions differ |

## 3. Order of work

Each step is proven with tests; the user plays the result.

1. **Import and read.** Recognise the Halo 1 disc (title ID `4D530004`),
   pull out its maps, decompress them, list their tags.
   **Built:** `crates/tools/h1map`, `h2 h1 info|tags|bitmap|convert`,
   `h2 import halo1`; format in
   [map-format.md](map-format.md). The disc's multiplayer maps
   are read and converted in tests ([multiplayer.md](multiplayer.md)).
2. **Map builder.** Write a whole v8 map. Proof: read a retail Halo 2 map
   into a tag set, build it again, and check the result reads the same, tag
   by tag. **Built:** `map::build` and `tags::rebuild`
   ([map-builder.md](../formats/map-builder.md)). All 30 retail maps
   rebuild byte-identical; 18 also pass with every bitmap moved. A map made from scratch has open questions: the
   index tail, zeroed header fields, salts.
3. **a30 walkable.** Geometry, collision, lightmaps, sky, shaders, one
   player spawn. You walk around a30 in Halo 2's engine with Halo 2's
   physics. Halo 2's assets stand in for anything not converted yet.
   **Built** (§9): `h2 h1 convert <iso> a30` (and the other nine levels,
   every BSP each); a30 ran headless in the game, and with round 2's
   three-BSP cap so did a10, b30, c10, c20, c40, d20 and d40.
4. **Objects.** Scenery, machines and doors, weapons, vehicles, equipment.
   **Built for weapons, equipment, vehicles, scenery, placed bipeds,
   the cyborg as the player, and machines, controls and light fixtures**
   (§9; devices from the `mach` and `ctrl`
   definitions, [tag-definitions.md](../formats/tag-definitions.md)).
5. **AI.** Characters, encounters to squads, generated pathfinding.
   **Built** (§9): `h1campaign::ai` on retail Halo 2 characters, pathfinding from
   `crates/formats/pathfind` in every converted BSP.
6. **Scripts.** **Built, stubbed** (§9): `h1campaign::scripts` with
   `Missing::Stub`, hooks resolved to the placed names, trigger volumes,
   squads and the sounds the scripts name.
7. **Cutscenes, sound and dialogue, text and HUD, checkpoints.**
   The converter rebuilds cutscene graphs and recordings. Per-level
   policy selects the cinematics and a10 tutorial path (§9, Cutscenes).
8. **Menus.** The campaign picker and its level list (§4), and progress
   saved per campaign. **Built** ([campaign-picker.md](campaign-picker.md)),
   confirmed in the game.

### a10 character animation integration checks

The disc checks assume the checkout has the two ISOs in `disc/` and read
the original maps and the converted tags offline. The library tests run
with `cargo xtask test -p h1-assets`. The ignored disc tests, with
`H1_ISO=disc/halo1.iso` set (and `H2_DISC=disc/halo2.iso` for the second):

- `campaign_biped_slots_and_talk_frames`
  (`crates/tools/h1-assets/src/models/tests/campaign_bipeds.rs`);
- `a10_dialogue_mouth_curves_reach_the_retail_resource_layout`
  (`crates/tools/h1-assets/tests/sounds/real_disc.rs`).

### Script sound capacity

**Built.** A scripted conversion installs every distinct source sound named
by a sound atom or a script reference, including generated conversation and
command-script references. The Halo 1 XBE passes the sound datum from the evaluated arguments at
`0xf2e4a..0xf2e53`; playback looks up that datum at `0x2b9e5..0x2b9f6`.
There is no authoring budget in that path. The converter grows the destination
sound list for the actual references. Its gestalt and map writers still check
their encoded index, tag memory and file size limits. A missing or ambiguous
sound is a conversion error naming the source map and sound.

The retail disc has 570 distinct map/sound pairs in the ten campaign script
reference tables, including every authored sound atom. The busiest level is a10
with 175. The thirteen multiplayer scenarios have no authored script sounds.
`both_sandboxes_rebuild_every_retail_map` checks the serialized reference tables
and the datums of the compiled sound atoms.

### Open work

Every group in §2 has a converter. What is unproven without a game run is
listed under "Needs a game run" in §9 and below, and a30's AI behaviour and
script flow (cutscenes, titles, checkpoints, start-up encounters) are
converted and read back offline, not played. Not planned: race flags
(Halo 2 has no race game type).

## 4. More than one campaign

**Built, confirmed in the game:** [campaign-picker.md](campaign-picker.md).
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

Chosen per game on the campaign's game options screen: **Halo 1 assets**
(default) or **Halo 2 assets**, kept as `campaign.halo1_assets`. The
import writes both variants of each level (**Built**, tests only). The
setting picks which file the VFS serves when the level loads.

**The Halo 2-assets variant (Built: `h1campaign::h2assets`,
`Options::assets`).** The level is converted as the Halo 1 one is
(structure, sky, scenery, devices, AI, scripts, sounds), except that the
weapon, equipment and vehicle palettes, the starting profiles' weapons and
the player name **retail Halo 2 tags**: the base's (05a), or copied whole
from a donor map off the Halo 2 disc (`h2assets::wanted`, then
`donor::bring`, as the characters are). A Halo 1 tag is recognised by
words in its path; the Halo 2 tag by its group and the last part of its
name (`h2assets::find`). No tag list is copied from either game.

| Halo 1 | Halo 2 | Why |
|---|---|---|
| Assault rifle | SMG | The user's multiplayer rule ([multiplayer.md](multiplayer.md)): both automatic; a campaign has no game variant to make it the battle rifle |
| Pistol | Magnum | Same slot, the user's multiplayer choice |
| Plasma pistol, plasma rifle, needler, sniper rifle, rocket launcher, shotgun | The same | |
| Fuel rod (`plasma_cannon`) | Fuel rod gun (`flak_cannon`) | Halo 2's campaign has one (multiplayer's table makes it the rocket launcher only because multiplayer lacks it) |
| Flamethrower | Shotgun | Close range; Halo 2 has none (multiplayer's choice) |
| Energy sword | Energy blade | |
| Frag and plasma grenades | Halo 2's | The globals keep the base's grenade types (Halo 1's are not written) |
| Health pack | Halo 1's, converted | Halo 1's health model stays whatever the assets (§6) |
| Overshield, active camouflage | Halo 1's, converted | The engine reads the rebuilt `eqip` type and duration |
| Ammo pickups | Left out | Halo 2 has none, and the Halo 1 weapons they fill are gone |
| Warthog, rocket warthog | Warthog | The gauss warthog is a model variant a campaign placement does not name |
| Ghost, scorpion, banshee, wraith | The same | |
| Shade (`c gun turret`) | Covenant turret (`c_turret_ap`) | |
| Pelican, Covenant dropship, lifepod | Halo 1's, converted | Set pieces: the scripts fly them with Halo 1's recorded and custom animations and load the AI into their seats |
| The cyborg (the player) | Master Chief (`masterchief`) | Set as the campaign players' unit (`matg +0x138`) |
| Cutscene props (`..._cinematic`: a10's scorpion) | Halo 1's, converted | Cutscenes keep Halo 1's objects |
| Vehicles flown by a recording (`..._playback`: d20's banshee) | Halo 1's, converted | The recording replays Halo 1's controls, which on Halo 2's vehicle would fly another path |

A Halo 2 tag neither the base nor a donor has is a conversion error
naming the Halo 1 tag and the map (`h2assets::stand_in`); the Halo 1
object is never placed in its stead (**Built**: the placements, the
starting profiles' weapons, the cyborg, the AI's weapons and the actor
variants' weapons). So is an ammo pickup or AI weapon the swap leaves
out, and an AI weapon with no retail counterpart (the AI weapon palette
has no converted Halo 1 weapon path), rather than an unarmed variant.

**Kept Halo 1's in both variants:**

- **Placed bipeds.** They are the cutscenes' casts and the levels'
  bodies. Halo 1's cutscenes are animated on Halo 1's skeletons, so with
  Halo 2 assets they need retargeting; until that exists, cutscenes keep
  Halo 1's characters in both variants. They play on eight of the ten
  levels (§9, Cutscenes).
- **The AI's characters.** Both asset variants use the same squad roster.
  Crew and Sentinels take Halo 1's actor variants, equipment and rebuilt
  bipeds; with Halo 2 assets their weapons are the retail counterparts,
  as every placed weapon is. Other types use the retail character table
  in `h1campaign::ai`.
- **Scenery, devices, sounds, text**: the level.

**The scripts' references.** Object names point at the placements as
before (the palette entry changed, not the placement). Each swap is kept
as an alias (`h2assets::Aliases`): the scripts' `object_definition`
atoms (`unit_has_weapon`, ...) and their reference block resolve a Halo
1 tag name to the Halo 2 tag placed for it. `object_definition` atoms
resolve by tag name in the Halo 1 variant too.
Seat mappings already match Halo 2 vehicles' seats by role
(`h1campaign::scripts::seats`).

**Writing and serving it.** `h2 import halo1` writes it as the job
`<level>/h2` after every other job (Halo 1's assets are the default),
to `<data>/campaigns/halo1/h2/<level>.map`
(`h1_variants::campaign_alternate`), with no HUD pairs and no manifest of
its own. `game::h1_variants::set_campaign_assets` flips the
`vfs::SandboxChoice` the campaign maps backend serves by
(`vfs::CampaignMapsBackend::with_assets`). With Halo 2 assets chosen and
no such file (the import did not write it), the level is
served from its Halo 1 map and the first open says so once on stderr:
`campaigns: a30: no Halo 2 assets level (<path>); serving its Halo 1
map`. The choice applies at the level's next open: an open map keeps its
file, and `map_changed` lets `game::map_slots` drop a cached copy of the
other file (slots 4 and 5). Each file has its own `MapId`. The Halo 1
map's HUD pairs (`<level>.hud.toml`) name its own datums, so `game::hud_swap`
skips them while a level's Halo 2-assets map is served
(`hud_swap::campaign_halo2_levels`).

Tests: `h1campaign` `h2assets::tests` (the table, finding a role by
name, aliases); `vfs`
`campaign_maps::tests::the_assets_choice_picks_the_level_at_its_next_open`,
`without_a_halo2_assets_level_the_halo1_one_is_served`; `cli`
`h1_import::tests` (the job list and where `<level>/h2` writes); `game`
`hud_swap::tests::campaign_levels_read_their_hud_files`. Against the
discs (ignored): `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo
test --release -p h1campaign --test assets -- --ignored --nocapture`
builds both variants of every level (`H1_LEVELS=a30,b40` for some): each
from scratch, every reference resolving, every script tag atom naming a
tag of its group; in the Halo 2 variant no Halo 1 item the table swaps
is placed or given, the player is the Chief, the AI's characters are the
Halo 1 variant's. Tag memory, file sizes and resolved reference counts of
all twenty variants are in [notes/campaign.md](notes/campaign.md). Both
variants of every level build from scratch with 0 dangling references and
every script tag atom resolving; the Halo 2 variant is a little smaller
everywhere. Every retail tag the table names is in 05a (no donor is
needed). Swapped on the ten levels: the assault rifle, pistol, sniper
rifle, rocket launcher, shotgun, needler, plasma pistol and rifle, both
grenades, the warthog, ghost, scorpion, banshee, wraith and shade; ammo
pickups dropped (26 palette entries).

**Only a game run shows:** Halo 2 weapons and vehicles in Halo 1's
encounters; the Chief in Halo 1's cutscenes that move the player (the
casts are Halo 1's, but a script may animate `(player0)`); Halo 1's
health model and meter on Halo 2's Chief; Halo 1's scripted drops with
Halo 1 pelicans and dropships carrying Halo 2 vehicles and AI.

## 6. Health (the exception)

Halo 1: a shield that recharges, over a health bar that doesn't; health
packs refill it. Halo 2: shields only, and the hidden body health
regenerates. For the Halo 1 campaign:

- the player's body health doesn't regenerate;
- health packs (Halo 1 `eqip` with a health powerup) refill it on pickup;
- the HUD draws a health meter under the shield meter.

**Built:** the `h1-health` feature and its switch,
`game::health::set_mode`. The game start picks the model from the loaded
map's name (`health::model_for_map`), so Halo 1's campaign levels get
Halo 1's. Where Halo 2 regenerates the body, how its own health powerup
(`eqip` type 5) refills it, the HUD's body input, what the converter must
write, and the debug switch `H2_HEALTH_MODEL=halo1`: [health.md](health.md).

## 7. Multiplayer

Halo 1's multiplayer maps follow the same rules, with a sandbox setting:
[multiplayer.md](multiplayer.md).

## 9. The ten levels on a pinned base (**Built**, tests only)

`crates/tools/h1campaign`; `h2 h1 convert disc/halo1.iso <level> --out DIR
[--disc disc/halo2.iso] [--base MAP] [--dry-run] [--force]` writes
`DIR/<level>.map` and `DIR/campaign.toml` (Halo 1's manifest,
`content::campaign_list::HALO1`, unless one is there already). With
`DIR = <data>/campaigns/halo1` the campaign picker's Halo 1 entry lists
the level and `vfs::CampaignMapsBackend` serves it as `maps\<level>.map`;
the map's header name is the level's and its scenario path
`scenarios\solo\<level>\<level>`, the path its start record names.

**Fresh by default; `--pinned` keeps the pinned base.** A level is first
built on the pinned base (the intermediate), then written
fresh (`h1campaign::convert`, `fresh`; `Options::pinned` and `h2 h1
convert --pinned` skip it):

1. **Its own scenario.** `scenario::fresh_root` starts from an empty
   `0x3e0` root; nothing of 05a's scenario is copied. Every root field
   05a fills that the conversion does not write, and what a fresh level
   writes instead (survey `survey_base_scenario_fields`,
   `survey_base_scenario_elements` in `tests/fresh.rs`, V on 05a, 03a and
   Coagulation):

   | Field | 05a | Fresh |
   |---|---|---|
   | `+0x00`, `+0x1f8`, `+0x208`, `+0x320` null references (`sbsp`, `unic`, `hmt `, `sfx+`) | group, -1 | the same (`FRESH_NULL_REFS`) |
   | `+0xe8`/`+0xf0` light volumes and palette (28, 2) | its own | empty |
   | `+0xf8` player starting profiles (2) | Halo 2's weapons | Halo 1's (`h1campaign::profiles`; the pinned build writes them too) |
   | `+0x100` starting locations | template element | Halo 1's, on a zeroed element (05a's bytes past the position and facing are zero) |
   | `+0x108` trigger volumes | 121, parked | Halo 1's only (no parked base volumes; hooks index from 0) |
   | `+0x138`/`+0x140` decals and palette | its own | empty |
   | `+0x150` AI styles (7) | its own | empty (every converted order has style -1) |
   | `+0x170` mission scenes (15) | its own | empty |
   | `+0x200` chapter titles | 05a's | converted Halo 1 help-text list |
   | `+0x358`, `+0x3b8` subtitles, objectives | 05a's | null |
   | `+0x210` BSP descriptors | template elements | zeroed elements, region and `sbsp`/`ltmp` written |
   | `+0x218` resources (1), `+0x230` kill triggers (4), `+0x248` AI triggers (50) | its own | empty |
   | `+0x250`..`+0x260` background sound, sound environment, weather palettes | its own | empty (the cluster data names none) |
   | `+0x298` object salts | 05a's counts | zero (Coagulation's are mostly zero) |
   | `+0x318` spawn data (1) | all zero | empty |
   | `+0x348`, `+0x350` fog and flocks; `+0x378`, `+0x380` decorators | its own | empty |
   | `+0x398` level data, `+0x3b0` mission dialogue | 05a's | empty |
   | `+0x3d8` simulation table (446) | its object, projectile and damage tags | every kept tag of those groups, in index order (`write_simulation_table`) |

   The base's BSP slots take Halo 1's BSP names.
2. **Built from scratch** (`h1campaign::scratch`, on `tags::repo`). The
   map is assembled from an empty set by tag path, not from 05a:
   `LevelSource` is one `TagSource` over two maps, the intermediate for
   every tag the conversion built or changed (built, aliased, or pinned
   with bytes unlike 05a's) and retail 05a for the rest. The roots are
   `matg`, the keep list below and every datum the written scenario and
   BSPs name; `tags::repo::closure` follows references from there and
   `to_tag_set` lays the tags out with a dense index (no slot of 05a's,
   no empty slots). The scenario, BSPs and lightmaps are the conversion's
   own blocks: their datum words are renamed to the new index, their raw
   blobs copied and their BSP sections made anew (`Transfer`), then the
   simulation table is written for the new index. A tag the extractor
   cannot take whole fails the build. So does a reference to a tag no
   source holds, from a retail or converted tag or from the scenario's
   and BSPs' datum words: the error names the map, the naming tag, the
   field (its offsets from the root, element indices where an array has
   more than one) and the missing tag. The retail levels have no such reference, so none needed a writer
or a target fixed. Tests: `a_reference_no_source_holds_fails_the_build_naming_the_field`,
`a_datum_the_new_set_lacks_is_an_error_naming_the_field`
(`h1convert::scratch`). Equal blocks are shared across
   tags, as retail shares arrays across tags in 25 to 28 groups per map
   (05a: 377,044 bytes of `effe` arrays and 245,966 of `shad` lie in
   other tags), except `bitm` (shares only within itself) and `ugh!` (not
   at all), which our readers take alone and retail never shares
   (`scratch::SELF_CONTAINED`, V on 05a, Coagulation, 03a and 07a). The core
   is `h1convert::scratch` (multiplayer uses it too) and takes any
   `TagSource`, so tags written from typed definitions plug in as a
   source. The sound gestalt is then cut to the kept sounds (below).
   Three 05a words outside every
   schema field read as pointers and are data (`fresh::DATA_WORDS`, given
   to the base's `MapSource::data_words`).
3. **No fallback.** A level that cannot be built from scratch fails with
   the reason (`h1campaign::Error::Scratch`, naming the level and the tags
   not taken exactly); nothing pruned or pinned is written in its place.
   Over the tag memory or the native campaign slot it fails the same way
   (`Error::TagMemory`, `Error::FileSize`). The grown globals keep the
damage-table arrays that the Halo 1 cyborg scales replace
(`h1objects::player_damage`) in their bytes with nothing pointing at them;
`map::build::GrownTag::replaced` names them and `MapSource::add_replaced`
counts them as described, so the extractor takes the tag whole.
`every_level_builds_in_both_assets` requires a build from scratch on all
20 campaign variants, and the multiplayer from-scratch tests on all 26. A
from-scratch d40 is 258,213,376 bytes ([notes](notes/campaign.md)), under
the native campaign slot's `0x20800000`
(`map::writer::NATIVE_CAMPAIGN_FILE_LIMIT`, the v8 header validator's
ceiling at `0x12289b`; `game::campaign_cache` grows the slots to it).
Test: `every_job_builds_from_scratch` (cli, `#[ignore]`, all 46 jobs).

**Locale raw extents.** The generic raw sizing walk can treat a locale's
file offsets as the start of a resource stretching to the next pointer it
recognizes, and the length then grows with intervening asset data. The
retail base's ninth locale has zero rows and zero text bytes (`matg +0x270`
and `+0x274`) with nonzero file offsets (`+0x278` and `+0x27c`); on a saved
a10 the walk gives `0x464be00` bytes for it, over the map reader's 64 MiB
limit on one raw read in `Unpinner::copy_raw`. Both fresh paths size all
nine locale indices as `count * 8` and their blobs as the descriptor's byte
count, matching the loader at `0x1a01e0`
([localization.md](../formats/localization.md)). The synthetic
`matg_locales_use_descriptor_lengths_across_a_large_raw_gap` test puts a
known boundary 80 MiB after an empty locale and checks both raw readers.

**Per-level sizes** are in [notes/campaign.md](notes/campaign.md)
(`tests/fresh.rs`, `every_level_converts_fresh`). The tag memory bound is
`0xc24000` and every level fits; the lean ladder fires
nowhere (b40 keeps its level data, seat table and object textures; the
ladder stays as a safety net). On every level the map is built from scratch
with one index slot per tag, and: no tag from
`scenarios\solo\05a_deltaapproach\` or `scenarios\multi\`, every script
atom naming an object definition, sound, effect, damage effect or
animation graph resolves to a tag of that group with data, and the
scripts' reference block reads back (`hs::Scenario::read`). The reference
sweep counts only problems the pinned map of the same level does not have
(none on any level). A fresh map over the file bound is an error
(`Error::FileSize`); the pinned map is never written in its place.

**Player starting profiles** (`h1campaign::profiles`). Halo 1's block
`scnr +0x348` (`0x68` each; S, checked on all
ten levels by `h1_starting_profiles_read`: every weapon is a `weap` or
none, a30's first is the assault rifle 60/120 and the pistol 12/64) is
written as Halo 2's `+0xf8` (`0x44` each, V on 05a and 03a): the name,
the health and shield modifiers, each weapon as its converted tag (the
same `h1objects` converter as the weapon palette, a donor's retail
weapon when only a donor has the template) with its rounds loaded and
total, and the fragmentation and plasma grenade counts.
`every_level_converts_fresh` checks every level's profiles: one per Halo
1 profile, each weapon Halo 1 names a `weap` of the map with data.

**Keep list** (`KEEP`, the multiplayer one, `fresh::MULTIPLAYER`):
`matg`, `scnr`, `ugh!`, `unic`, `mulg`, `stem`, `spas`, `vrtx`, `pmov`,
`sncl`, `snmx` and every tag under `shaders\` or `rasterizer\`. The HUD,
interface globals, AI globals and dialogue, the player's character and
the sound classes are all reached from `matg` (and the converted
scenario), so they need no entry: the fresh a30 keeps every `matg`
reference. No scenario palette is nulled (the conversion already wrote
them all).

From scratch takes less tag memory than the pruned build: the dense
index, the cut gestalt, and equal blocks placed once. Sharing equal blocks
across tags saves about 1 MB a level over sharing within a tag, the arrays
retail itself shares across `effe`, `shad`, `styl` and `prt3` tags.

**The sound gestalt cut to the kept sounds** (`h1convert::compact`, run by
the builder from scratch). 05a's `ugh!` holds every 05a sound's entries;
a new gestalt is built from nothing, each kept `snd!` in index order
taking its entries across (`h1_assets::sounds::merge::merge_sound`). The merge
carries native promotion rules and timers with their rebuilt pitch ranges.
Raw that no kept chunk names is emptied (a30's counts are in
[notes/campaign.md](notes/campaign.md)); on every level each kept sound names the same entries, import
names and chunk bytes as in the pinned map (`compact::map_entries`,
`every_level_converts_fresh`). A gestalt the cut cannot read stays whole
(`ScratchReport::gestalt_whole`).

**Base per level: `05a_deltaapproach` for all ten** (`h1campaign::base_for`).
Why (surveys `base_memory_and_vehicles`, `survey_bases`):

- It is the only campaign map with every vehicle template Halo 1's levels
  use: warthog, ghost, scorpion, banshee, pelican,
  phantom (for the dropship), wraith and `c_turret_ap`. 03b has most of
  them but not `c_turret_ap`, and `0xca000` less room; 05b, 04a and 07b,
  the ones with more room, lack the warthog, ghost and scorpion.
- It has Halo 2's marine, elite, grunt, jackal, drone and Chief bipeds and
  the retail AI characters used on a30. Halo 1 supplies the crew. The
  hunter, flood and monitor it lacks come from donors
  (below).
- It is big enough once the conversion stops wasting it: every level fits
  (§9, the notes); b40 exactly (`0xc24000`, nothing to spare).

**Tag memory.** The bound (`map::writer::max_metadata_size`) counts the
index, the BSP space and the ordinary metadata. Two things keep a50 and b40
inside it, as the per-group and appended-tag accounting in
`Report` (`groups`, `appended`, and `Error::TagMemory`, which names the
largest groups and scenario fields) shows:

1. Without relocation the rebuilt scenario would go after the base's
   metadata while the base's old scenario bytes (`0x10b8ec`) stayed as
   unowned metadata. The base scenario is relocated as a block tree
   (`tags::schema::Relocator::to_built_emptied`) with the fields the
   conversion replaces emptied first (`scenario::replaced_fields`:
   placements, palettes, names, starts, trigger volumes, BSP descriptors,
   AI, scripts); only the arrays other tags keep inside its old range stay
   (`Relocator::pointed_into`: `0x32` bytes on 05a). The old range is a
   hole the new scenario fits in.
2. Small built tags would split that hole and the rebuilt sound gestalt's
   (the last `0xb4b80` bytes), so the two big ones would be appended. Built
   tags are placed largest first (`TagSet::largest_built_first`).

With b40's repeated firing positions shared (ai.md, Spawning), all ten
fit. Carrying every BSP costs little tag memory: a BSP region lives in
the file, and only the largest region has to fit the base's BSP space
(`0x394000`); each further BSP adds its lightmap `bitm`, its scenario
elements and nothing else to the metadata. b40 with all 13 BSPs, the hunter
and Halo 1's textures on its structure (10 to 25 shader copies a BSP,
shared between BSPs by name) reaches the bound exactly; the copied
characters mostly fill holes in the base's metadata. b40, a50 and c10
append their scenario; the rest grow nothing past the base's metadata.
Per-level numbers: [notes/campaign.md](notes/campaign.md).

What each layer writes, from the base map up:

| Layer | Written | Left out |
|---|---|---|
| Structure | Every Halo 1 BSP, in Halo 1's order: into the base's BSP slot of the same index, past the base's three into new `sbsp`/`ltmp` tags under Halo 1's names (templates from the base's first), with `h1convert::structure` (collision, render geometry, materials matched to the base's shaders and drawing Halo 1's own textures on copies of them (`h1convert::textures`), Halo 1's lightmap as a new `bitm`, MOPP, audibility, sound clusters, each cluster's background sound and sound environment from Halo 1's ([multiplayer.md](multiplayer.md), "Ambient sound")) and pathfinding generated from the collision at `sbsp+0xc4` | |
| Sky | Halo 1's skies (`h1_assets::sky`), scaled to stay behind the level (sky.md, "The model scale"), every cluster on the first; the model's materials name copies of the base's sky shaders with Halo 1's bitmaps and map scale (sky.md, "The model's shaders"), else the base sky's shader, never NONE (`0x1bbf0` draws NONE, a null read in `0x16b90`; multiplayer.md) | |
| Scenario | The base `scnr` relocated as a block tree, replaced fields emptied. One BSP descriptor per Halo 1 BSP (`FixupKind::Bsp`, its `sbsp` and `ltmp`), so `switch_bsp`, zone and placement BSP indices keep Halo 1's meaning. The two root blocks every retail map holds one element per BSP of (survey `per_bsp_scenario_blocks`): cluster data `+0x290` (per cluster its background sound and sound environment, as the structure's cluster `+0x72`/`+0x74` names them, and the cluster's centroid, and the BSP's checksum `sbsp+0x08`) and `+0x388` (no points). BSP switch trigger volumes `+0x130`: Halo 1's (`scnr+0x39c`, 8 bytes, survey `h1_bsp_switch_volumes`) on its converted volumes | The base's own BSP switch volumes |
| Player | The level's starting locations, each on the base's first element. The cyborg is converted and set as the campaign players' unit (`matg +0x138` elements 0 and 1, `h1convert::globals::set_player_unit`); Halo 1 health follows from the map name (`game::health`) | |
| Objects | Every base placement removed. Weapons, equipment, vehicles, scenery, placed bipeds and devices from the level's palettes through `h1objects` (palettes keep Halo 1's order, a null entry for one that doesn't convert; templates in objects.md), placements with unique IDs `0x4831xxxx`, origin BSP -1, source 1. Object names are Halo 1's, each pointing at its converted placement or -1. A palette entry whose template only a donor brought places the retail object itself. Halo 1's "not placed automatically" (bit 0 of the `u16` at `+4`) is written as Halo 2's "not at start" (bit 0 of the `u32` at `+4`), so cutscene casts and props wait for the scripts that create them (a10: 67 of 107 bipeds, 51 of 56 vehicles, 92 of 274 scenery) | **Built.** Overshield and camouflage as native `eqip`, b30's falling box as a dynamic `bloc`, and a10's Halo ring as `scen`. The cryotube and chairs retain the stationary seats used by a10's scripts, with collision bodies on the model nodes. The fighter-bomber and lifepods use their respective source vehicle types and physics. The entry pod carries Halo 1’s reentry fire on its `burn` marker. Details and disc evidence are in [objects.md](objects.md#native-pickups-and-scripted-props). Machines, controls and light fixtures (as machines) are placed with their power and position device groups and flags; a device with solid collision has a physics model built from it, a body per moving node, so a closed door blocks the player; fixtures carry their Halo 1 lights on the original markers |
| Trigger volumes | The base's parked (`h1convert::convert::PARKED`, object name -1); the level's appended after them | |
| Donors | `h1campaign::donor`: each character `h1campaign::ai`'s table names, copied whole from the first donor that has it (`03a_oldmombasa`, `04b_floodlab`, `06a_sentinelwalls`) by `h1_assets::sounds::transplant`: the tag tree, its raw, and its sounds' gestalt entries | Crew and Sentinels use the source level's units and animation graphs |
| AI | `h1campaign::ai`: squad groups, squads, zones, orders, character palette. Characters the base lacks and no donor brought fall back to the marine (marine kinds) or the folder's first | **Built** crew and Sentinel variants, ground and airborne firing positions, scripted spawns and migration targets (ai.md) |
| Scripts | `h1campaign::scripts` with `Missing::Stub` and each level's cutscene policy (below); `ai_place` with one argument. A startup script `h1_initial_encounters` places the encounters Halo 1 creates at load. Hooks: object, unit and vehicle names, `short`, team and difficulty as `0xffff0000 \| index`; trigger volumes as the index; AI squads as the index and squad groups as `0x40000000 \| index`; string IDs interned; sounds as the converted tag's datum (encodings V: every such atom in 05a's and 03a's retail scripts). Halo 1's cutscene flags, camera points, titles and device groups written to Halo 2's blocks in Halo 1's order (`h1campaign::scripts::level`), their atoms the index (a device group's runtime datum); the seat-mapping table `+0x228` built from the vehicles' seat labels (`h1campaign::scripts::seats`); navpoints `default`; looping sounds, effects, damage and animation graphs by tag name among the converted tags. Script-named squads retain their own indices, including migration destinations. Script references resolved by tag name ([scripts.md](scripts.md), "Level data the scripts name") | Effects, damage effects, animation graphs and object definitions with no converted tag of that name: `none` |
| Sounds | **Built.** Every source looping sound, its tracks and details, and each distinct referenced sound rebuilt in the level's gestalt. All loops survive the fresh-map and pruning passes. Script sounds, object attachments, BSP background palettes and environments resolve by tag. The a30 entry pod plays its Halo 1 lifeboat track. Source inventory and cache evidence are in [sounds.md](sounds.md#campaign-loops-and-cache-slots-built). | End-to-end disc audit and playback pending |
| Text | The base's English table stays first; the level's string lists, HUD messages and object prompts are added after it (`h1_assets::text::scenario::LevelText::install` into `Locales::from_map(base)`, [text.md](text.md)) | |

**Built. Empty object placements.** The retail Halo 1 XBE returns the null
object handle when a placement's signed palette index is -1
(`0x90320..0x90332`) or the selected 0x30-byte palette entry's tag datum
at +0xc is null (`0x90376..0x90391`). Both branches bypass object creation
at `0x903d4`. The automatic scenario loop calls this routine at `0x91231`
and scripted object creation calls it at `0x9044d`. Campaign objects
and multiplayer scenery and vehicles honor both sentinels. Other indices
outside the palette produce a conversion error naming the map and placement.
The BSP placement pass performs the same two checks at `0x90cc1..0x90cc5`
and `0x90cec..0x90cff`, before creation at `0x90de7`. It records the
result, including the null handle, at `0x90f11`. Of 11,624 retail
placements, four scenery rows have index -1, one each on a10, a30, b40
and c40.

### Cutscenes on all ten levels

**Built.** The default policy is `Cutscenes::Play` on every level. The
compiler preserves Halo 1's skip guards, including a player's deliberate
skip, and a10 wakes its authored cryo tutorial. Its signed look latches,
controller layout and pitch settings come from the converted unit builtins.
An explicit `Options::cutscenes` can still select the shorter diagnostic paths.

The ordinary fresh writer retains the reachable base tags and resources.
Generated AI and cinematic globals share identical scalar initializer nodes
while retaining independent checkpointed cells. Each duplicate removed saves
one 20-byte syntax node. A full fresh scripted conversion reports a capacity
error before a fallback can discard content.
A full fresh scripted conversion fits b40 (`0x966000` of `0xc24000` bytes
with Halo 1 assets, [notes/campaign.md](notes/campaign.md)). Graphs already
converted for an object are shared with scripts. Recordings, camera points,
flags, titles and the remaining graphs have their original scenario
indices or named tag references. d40 relies on the native campaign cache
expansion, which raises slots 4 and 5 up to the header validator's 520 MiB
ceiling. The disc readback requires the full conversion on both levels.

**Built.** x50 and c40 actor movement runs through the rebuilt command
scripts. The disc readback walks each cinematic's original command-list
calls and compares their targets in order with the emitted
`cs_run_command_script` calls. On the source disc c10 has ten by-unit
commands, including Mendoza and Johnson's looks. c40 has twenty, including
the Monitor's flight and individual orders for four sentinels.

What a cutscene needed (survey `cutscene_survey`, h1campaign
`tests/cutscenes.rs`, over all ten levels):

- **Custom animations.** `custom_animation` (Halo 2 has it; the
  animation name casts from string to string ID). Halo 1's other
  animation builtins have Halo 2 builtins no retail script calls, found
  by their place in the table (scripts.md, "Builtins named by
  position"): `scenery_animation_start` (184), `_at_frame` (188),
  `custom_animation_list` (205), `unit_custom_animation_at_frame` (206),
  `unit_is_playing_custom_animation` (207). Every graph the scripts name
  is in the level and has its own node block (34 nodes for the captain,
  41 for Cortana, 1 to 26 for props). `h1campaign::cutscenes::
  install_graphs` converts each one that no object brought already
  (`h1_assets::models::graph_from_map`, under its Halo 1 name, so the
  `animation_graph` hook resolves by name), and each animation name a
  script passes becomes the converted animation's name
  (`h1_assets::models::jmad::animation_names`; `h1campaign::scripts::compile::
  ANIMATION_NAME` hooks carry `<graph>|<animation>`). The cutscene
  characters already have their own Halo 1 model and graph (h1objects'
  placed bipeds, on the Chief's template), so a cinematic graph animates
  the skeleton it was made for. 9 graphs on a10, 2 to 9 elsewhere.
- **Recorded animations. Built.** Halo 2's player at `0x1fb360` bounds the
  index by `scnr +0x110`. Each row is `0x34` bytes, with stream version
  at `+0x20`, unit control version at `+0x22`, duration at `+0x24` and
  the event-stream pointer at `+0x30`. The writer reads the Halo 1 rows
  at `scnr +0x36c` (stride `0x40`) and builds fresh rows from the
  `recorded_animation` definition. Script references use the original
  ordering. Each stream gets a relocatable Halo 2 data reference. The
  writer includes the block when building without scripts and throughout
  the lean ladder.
- **Control streams and playback. Built.** The ten Halo 1 campaign levels
  contain 349 version-4 streams, using unit control versions 0, 3 and 4.
  The Xbox decoder loops at H1 `0x119910` and H2 `0x29f0c0` agree on
  event types and delays. The initial layouts differ, and a flag event
  grows from two payload bytes to four. Reading the original encoding
  directly loses event framing on 189 streams. The encoder rebuilds the
  initial state, translates the flags and preserves each delay, control
  delta and end event. An unsupported stream fails conversion with its name.
  H1 control bits 2, 3, 14 and 15 have no observed campaign use and are
  rejected if encountered.

  All 73 timed animation-state changes retain their original tick. The
  Halo 2 type-2 handler (`0x29ec30`) sign-extends a single byte. A
  converted stream carries the low byte of the target mode, which the
  playback adapter expands to its full string ID. Only calls from the
  recording player (`0x1fb630`) carrying both single-weapon markers take
  this path. At that boundary the adapter also resolves inventory
  generation and the other hand's current slot. H1's `-1` slot means
  keep the current weapon (H1 `0x3b3d3`, H2 `0xc6e2b`).
  Crouch, jump, throttle, facing, aim, weapon triggers and reload go
  through the native unit control reader. The biped's held-facing and the forced-alert
  overrides apply to a private copy of the current tick. Grenade and
  seat-exit requests use the ordinary Halo 2 unit actions (`0x14fe58`,
  `0x14fb55`). The decoder keeps its own state across ticks.
- **Recording script calls. Built.** The existing signature-checked
  translations select the native play, play-and-delete, play-and-hover,
  kill and time builtins (138–142), and vehicle hover (621). The Xbox
  evaluators and their native call targets are listed in
  [scripts.md](scripts.md#builtins-named-by-position).
- **Camera.** `camera_set`, `camera_control`, `camera_time` map directly;
  `camera_set_relative` (451), `camera_set_first_person` (456) and
  `camera_set_dead` (457, an object in Halo 2) by position. No level
  calls `camera_set_animation` (452, mapped too). Camera points are the
  level data (`h1campaign::scripts::level`).
- **Dialogue.** `sound_impulse_start` maps directly. `sound_impulse_time`
  (493 calls; the scripts sleep on it after almost every line, and wait
  `(sleep_until (= 0 (sound_impulse_time line)))` for a line to finish)
  is Halo 2's `sound_impulse_language_time` (scripts.md, Renamed): the
  ticks the playing line has left, 0 once it ends. A constant length
  would never reach 0, so no length hook is used. Looping sounds are
  converted with the level's sounds (§9, Sounds row).
- **Flags, fades, titles.** `cinematic_start`/`stop`,
  `cinematic_show_letterbox`, `cinematic_set_title`, `fade_in`/`out`,
  `player_enable_input`, `player_camera_control`, `switch_bsp`,
  `object_teleport` map directly; `cinematic_set_title_delayed` (561),
  `object_set_facing` (106), `object_pvs_set_camera` (471), `show_hud`
  (625, the first of three boolean-to-boolean builtins there),
  `unit_open`/`unit_close` (194, 195), `unit_set_emotion` (218),
  `unit_suspended` (249),
  `unit_enter_vehicle` (216) by position.

The [unit builtin mappings](script-builtins.md) cover object beautification
and PVS activation, emotion and animation modes, the vehicle seat labels,
near clip distance, dynamic lights, BSP object spawn suppression and camera
shake. The same page describes the rebuilt screen effects, model reflection
tint, light-cache invalidation and co-op tick authority.

**a10's opening (`x10`), call by call.** `x10` calls `autumn_glory_1`, `autumn_glory_2` (space, `a10_space`),
`bridge` (BSP 1), then `hangar_1`, `hangar_2` and `cryo` (`x10hangar`, the
cryo bay). Its fades, camera moves, letterbox, BSP switches and sounds
are the Halo 2 builtins of the same name with the same parameter types,
so every duration stays a count of ticks (`cinematic_calls_keep_halo1_s_ticks`,
`h1campaign` scripts: `fade_in`/`fade_out` `(real real real short)`, `camera_set`
`(cutscene_camera_point short)`; retail Halo 2's own literals are 15, 30,
60, 500 ticks). No fade or camera call is mistranslated. **Built.** The hangar toggles dynamic lights, the space scene invalidates
cached lighting, and the bridge updates its PVS, clip distance and screen
effect. Extra cinematic ticks go through the native simulation batch.
The cinematic's continuous `dialog_flavor_captain` sleeps between lines
until woken, so it does not restart each tick (scripts.md, Sleeping
forever).

### Waits on stubs (playing a level through)

`h1campaign`'s disc test `waits_on_stubs` counts, per level, the
`sleep_until` and `if` conditions that read a stubbed builtin, and which
of them nothing handles (a stub other than `ai_status` and those whose
zero is already the done value: `recording_time`, `ai_going_to_vehicle`).
The counts per stub and the waits left per level are in
[notes/campaign.md](notes/campaign.md); the waits left are in tutorials
(a10's cryo tutorial, a30's night vision tutorial and joystick help) and
two branch conditions. What the stubs read in place of Halo 1's value:

| Stub | Behaviour |
|---|---|
| `ai_status` | `ai_combat_status` on Halo 1's scale (scripts.md, Step 2) |
| `ai_conversation_status` | live status from the generated dialogue worker |
| `ai_command_list_status` | done value 2 |
| `game_all_quiet` | retail Halo 2 threat query (587) |
| `ai_living_fraction` | `ai_strength` |
| `device_get_power` | Halo 2's unnamed builtin 255 (`IDENTIFIED`) |
| `player0_joystick_set_is_normal` | active local look-stick layout (722) |
| `game_time` | current simulation tick (888) |
| `game_safe_to_speak` | retail Halo 2 dialogue safety query (586) |
| `ai_going_to_vehicle` | `0` (none is going) |
| `sound_impulse_time` | `sound_impulse_language_time` (the ticks the line has left) |
| `hud_get_timer_ticks` | the native countdown returns the remaining ticks (see [text.md](text.md)) |
| `ai_conversation_line` | the current line, or 999 when inactive |

`ai_living_count`, `ai_strength`, `unit_get_health`, `volume_test_objects`
and `objects_can_see_object`/`_flag` translate directly; they are not
stubs.

The lean ladder (`h1campaign::Lean`) drops the graphs
(`CutsceneData`) after the music and before the level data; a level over
the campaign cache's file size is converted again with less music, then
without the background sounds, then without the graphs (Engine caps, "Music, ambience and the map file"). A fresh build drops the base level's unreachable
tags, which recovers the space b40's level data and cinematic graphs need.

Placed objects Halo 1 leaves to the scripts are not created at load
(Objects, above): a10's x10 crew in the space BSP (`keyes_x10_space`,
`space_crew_1` to `_3`), Keyes and the bridge crew, the hangar marines.
The cinematics create them; skipped ones never do.

Proof: `skipped_cutscenes_take_halo1_s_skip_path`,
`an_animation_name_is_hooked_with_its_graph` (`h1campaign` scripts unit tests);
`skipped_cutscenes_on_every_level` (`h1campaign` scripts disc);
`cutscenes_convert_and_read_back` (h1campaign disc, `H1_LEVELS` for
some): per level, played, it converts, reopens the map and checks that
`scnr +0x110` holds the named recording rows in order with their re-encoded streams, and that every graph it converted is a `jmad` under its name;
`every_level_s_scripts_run_on_the_retail_interpreter` with the
identified builtins; `a30_converts_onto_its_base_and_reads_back` checks
every placement's flags against Halo 1's; `h1_placement_flags` and
`h1_initial_encounters` (survey) print the evidence.

**Needs a game run (cutscenes):** that Halo 2's decoders play Halo 1's
version-4 event streams and unit control data as recorded; that
`custom_animation` takes a converted graph on a converted biped (node
list checksums are Halo 1's on both); every identified builtin's
behaviour (they are found by position and signature, E only for the
recordings and vehicle hover); the dialogue sleeps lining up with the converted sounds.

**Device change-colour relocation.** The holo control a30 borrows
from b30 points `+0xb0` at the change-colour array at `obje +0xac`, which
the relocation schema covers for `ctrl` and `mach` through the shared
object definition. Every campaign level's controls, machines and
light fixtures (built as machines) use the same device builder and layout.
`every_level_builds_in_both_assets` exercises all ten levels in both
asset settings. Both a30 maps are checked for the holo control's palette
reference and all four colour slots.

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
equal to Halo 1's BSP count). `a30_converts_onto_its_base_and_reads_back`
(`H1_LAYERS`) reopens a30 with `map::Map` and checks the checksum,
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
with `h1campaign::ai::h2::ScenarioAi::read` and its indices check; the scripts
decode with the compiled counts, and the startup script's `ai_place`
calls take one argument each, resolved to the squad groups it names;
every converted sound resolves through the gestalt; `campaign.toml` lists
a30. The object sweep (objects.md; also over the retail base by
`object_sweep_is_clean_on_the_retail_base`) follows every palette entry
and AI character: references resolve with data, each `hlmt` covers its
render model's and physics model's nodes, and no root block retail
always fills is empty. Surveys behind the choices are in
`tests/survey.rs`.

Per-level counts (tags added, tag memory, BSPs and switch volumes, placed
objects of each kind, squads, scripts and the hooks left `none`) are in
[notes/campaign.md](notes/campaign.md). The sweep is clean and every BSP
is checked on every level. Characters copied from donors: the hunter on
a50, b30, b40, c40, d20, d40; flood combat forms, carrier, infection and
the monitor on c10, c20, c40, d20, d40 (by what each level's AI uses). The
bound is `0xc24000`; the base alone is `0xaed000`.

### Engine caps and unchecked indices

- **Squads. Built.** The retail arrays hold 335 squads and 100 groups.
  On levels over the squad cap, the converter can omit unreferenced
  squads in encounters with deferred creation when no placement call
  can reach them. Script-named migration targets retain their own
  indices. The campaign allocator reserves 386 squad records and 100 groups.
  Before `0x00200b30` populates them, the runtime exposes the scenario's
  required squad count, with 335 as the retail minimum. The group array
  stays at its allocated address across map loads. See ai-control.md for
  the allocator and b40 readback checks.
- **Cutscene flags, camera points, titles and device groups.** The
  engine indexes these blocks without a check (b40 faulted at
  `0x002a003f` in `object_teleport`, reading flag -1): a call naming one
  that does not resolve is stubbed (scripts.md, Indexed values).
  `every_level_s_scripts_run_on_the_retail_interpreter` checks every
  reached atom of these types against its converted block on all ten
  levels. Names resolve without case (b30 and d40 named 3 and 9 flags
  and a device group in another case and lost them).
  The a30 disc test also reads the English string of each chapter from
  the final map. See [mission flow and title layout](scripts.md#level-data-the-scripts-name).
- **Units opened or closed.** `unit_open` and `unit_close` do not check
  their unit for `none` (`0x00113eae` reads `+0x33e` through object
  header 0xffff's null pointer), so a call naming an object name with no
  placement is stubbed (scripts.md, Units the engine does not check for
  none). a10's `fast_setup` calls `(unit_open cryotube_1)` as the opening
  cinematic ends, so the cryotube is built from its definition and placed
  (tag-definitions.md, Vehicles with no template). A placement is created
  only if the definition's model has a physics model: `vehicle_new`
  (`0x000eeb80`, the `vehi` type's new function) returns false when the
  definition's model (`hlmt`, object `+0x38`) has no physics model
  (`hlmt +0x20` `phmo`; `cmp [edx+24h],-1` at `0x000eebcb`), `object_new`
  (`0x000b7b40`) then deletes the half-made object and returns `none`, and
  the name stays `none`. Every retail vehicle's model has one. A vehicle
  built from its definition (cryotube, chairs, fighterbomber, Halo ring),
  or a templated one whose template's physics reaches past the converted
  nodes (fresh a10's `lifepod_atmosphere_entry`), gets one built from
  Halo 1's collision as Havok polyhedra (tag-definitions.md, Physics models
  from Halo 1), so `vehicle_new` creates it. Every `unit_open`/`unit_close`
  runs inside `(if (> (list_count x) 0) ..)`, a no-op on `none` as in
  Halo 1 (scripts.md).
  `a10_opens_and_closes_only_placed_units` checks every reached call on
  a10 (fresh) is placed and guarded;
  `a10_cryotubes_are_placed_and_built_from_their_definition` checks the
  cryotubes' placements and tag; `a10_scripted_vehicles_can_be_created`
  checks every vehicle palette entry's model has one.
- **Pinned b40.** The pinned conversion reaches the tag memory bound. A level over
  it is converted again leaner (`h1campaign::convert`, `Lean`), dropping
  the seat table (`SeatTable`), then cutscene graphs (`CutsceneData`), then
  level data (`LevelData`). Object materials and gameplay recordings stay installed. A level
  still over the tag memory bound places fewer devices
  (`Options::devices`): no light fixtures (lamp models that give no
  light in Halo 2), then none. Pinned b40 converts
  only without its level data: its 74 calls naming a flag, camera point,
  title or device group are stubbed (the insertion's teleports and camera
  moves among them), and it omits its music. The from-scratch build keeps
  all of it.
- **Music, ambience and the map file. Built.** All 548 source loop
  definitions across the ten levels have explicit roots. Local raw chunks
  hold their samples. Native campaign slots 4 and 5 grow for installed
  Halo 1 maps up to the header validator's 520 MiB maximum. A level over
  that limit reports its size. The disc audit compares the built tags and
  sample chains with the source. [Cache and disc evidence](sounds.md#campaign-loops-and-cache-slots-built).

**Needs a game run:**

- every level but a30 loading at all, and a30 still loading with the
  relocated scenario, scenery and bipeds;
- the player as the cyborg (Halo 1's model and animations on the Chief's
  unit tuning) with Halo 1 health;
- AI spawning: the startup script and Halo 1's own one-argument
  `ai_place` calls; actors on generated pathfinding;
- scenery built from the `scen` definition (no
  template; tag-definitions.md): drawn at every distance, lit, and
  blocking through its `coll`'s BSP physics; placed bipeds;
- the pelican, phantom-for-dropship, wraith and turret templates under
  Halo 1 models;
- every BSP past the third (new `sbsp`/`ltmp` tags and descriptors, the
  rebuilt cluster data), reached by the scripts' `switch_bsp` and by
  Halo 1's BSP switch volumes, which Halo 2's engine is expected to
  switch on as it does for its own;
- the copied characters (hunter, flood, monitor) and their sounds, and
  lifepods (built from the `vehi` definition,
  tag-definitions.md), flown by a30's recordings;
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
   map on stderr, `halo1: [3/46] campaign a50: done (5.4 s)`. The jobs are
   the ten levels, the 13 multiplayer maps in two sandboxes, and then each
   level's Halo 2-assets variant (§5), `halo1: [37/46] campaign a10 (Halo 2
   assets): done`. Timing: [notes/campaign.md](notes/campaign.md). Later
   starts print `halo1: up to date (46 maps from halo1.iso)` and go
   straight on.
3. Sign in, **Campaign**, then **Halo: Combat Evolved**, the picker's first row (Halo 2 is second):
   the ten levels, a10 to d40, as far as the profile has got. Halo 1's
   multiplayer maps are `h1_<map>` in the multiplayer map list (Custom
   Game, Xbox Live), named as in Halo 1 (docs/halo1/multiplayer.md
   "Importing"). `--level a30` starts a converted level directly.

`--no-halo1` or `H2_HALO1=0` (also `false`, `no`, `off`) skips the check.
A failure never stops the game: a map that fails is a line,
`halo1: [5/36] campaign b40: failed: <why>`, and the others are still
there; a problem before any map (an unreadable disc, a bare `default.xbe`
as the game disc) is `halo1: not imported: <why>`.

By hand, the same import: `cargo h2 import halo1 [disc/halo1.iso]
[--data DIR] [--disc disc/halo2.iso] [--force] [--dry-run]
[--copy-source]`. `--data` defaults to the data root `h2 play` uses
(`~/Library/Application Support/h2recomp/data`); `--disc` (the Halo 2 disc, for the bases) to
`$H2_DISC`, else `halo2.iso` beside the Halo 1 disc, else
`disc/halo2.iso`. It exits non-zero when a map failed, after trying every
map. `--copy-source` also copies the disc's campaign maps unchanged to
`<data>/campaigns/halo1/source/` with a SHA-256 manifest.

### What it writes

| Job | Output | Read by |
|---|---|---|
| each of a10 .. d40 | `<data>/campaigns/halo1/<level>.map`, `campaign.toml` (`content::campaign_list::HALO1`) | the campaign picker and lists (`game::campaigns`), `vfs::CampaignMapsBackend` as `maps\<level>.map`, `--level` (`level_boot::resolve_in`) |
| each Halo 1 multiplayer map on the disc (`h1convert::convert::MAPS`), Halo 2 then Halo 1 sandbox | `<data>/maps/h1_<map>/{h2,h1}/h1_<map>.map`, `manifest.toml` | `game::new_maps`, the `multiplayer.halo1_sandbox` setting picks the variant |
| then each level's Halo 2-assets variant, job `<level>/h2` (§5) | `<data>/campaigns/halo1/h2/<level>.map` | `vfs::CampaignMapsBackend::with_assets` when `campaign.halo1_assets` is Halo 2 |

Campaign levels are written fresh from their base (§9); multiplayer maps
fresh from their default base (`--pinned` for either keeps the base).

### The stamp, and when it converts again

`<data>/campaigns/halo1/import.stamp` holds the Halo 1 disc's SHA-256,
size and modification time, the converter version, and each map's
outcome, written after every map (an interrupted import resumes). A start
converts what is missing: a map not in the stamp, or stamped done but
whose file is gone. Everything is converted again when:

- **the converter version changes**: `CONVERTER_VERSION` in
  `crates/apps/cli/src/h1_import.rs`. **Bump it in the commit that changes what
  the Halo 1 converters write** (`h1convert`, `h1campaign`, `h1objects`, `h1-assets`, or the map
  builder under them). `--follow` builds each new commit's `h2 play`, so
  after such a merge the next F5 converts again with the new code;
- the disc is another one (its hash; it is hashed again only when its size
  or modification time changed);
- `--force`, or the stamp is deleted.

A map that failed is not tried again until one of those: a failure from
a converter bug stays a failure until the converter changes.

**Tests.** `cli` `h1_import::tests`: the job list (campaign in play
order, then the multiplayer maps the disc has, both sandboxes, then each
level's Halo 2-assets variant); where each writes (the folders
`game::new_maps` and the campaigns read, the variant where
`h1_variants::campaign_alternate` looks); the stamp
reads back; an unchanged disc is not hashed again and keeps its outcomes,
another disc or converter version starts again; what is pending; a map
that fails or panics is recorded, the stamp saved after each, the rest
converted; `H2_HALO1`/`--no-halo1`; finding `halo1.iso` (any case, or a
Halo 1 image by its XBE) and never the game disc or `mappack.iso`; no
Halo 1 disc, or a game "disc" that is no image, writes nothing.
`game` `campaigns::tests::a_halo1_import_is_picked_started_and_served`:
an import's folder (stamp and `source/` included) lists Halo 1 in the
picker with the ten levels in order and named; each level's
start record, looked up as `0x19c270` does after the lists are extended,
names a scenario path `0x214db0` turns into `d:\maps\<level>.map`, and
`CampaignMapsBackend` serves exactly the converted file there.
`level_boot::tests::an_installed_campaign_level_resolves_by_map_or_scenario`:
`--level a30` gets the campaign and map ids the lists give it.
Against the discs (ignored): `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso
cargo test --release -p cli --bin h2 h1_import::tests::real -- --ignored
--nocapture` imports everything into a scratch data root, imports again
(nothing to do), and checks `game::new_maps` lists every `h1_<map>`, the
picker's Halo 1 lists the ten levels, the VFS serves each level's map
(and its Halo 2-assets map when those are chosen), and
`--level` resolves each (`H1_JOBS=a30,bloodgulch` for some).

**`--level`.** `h2 play --level` and `H2_LEVEL` resolve the disc's levels
and the campaigns installed in the data root, with the ids
`game::campaigns` gives them (`level_boot::resolve_in`); `h2 play` imports
before it resolves the level.

## 8. Open questions

- Memory: all ten levels fit `0xc24000` on 05a with every BSP and Halo
  1's textures (§9); b40 fits exactly, with nothing to spare, so more b40 content needs a bigger hole (freeing
  the base's unreached tags) or less. Its zones (`scnr+0x168`,
  `0xce898` bytes, mostly firing positions) are the scenario's largest
  field.
- Which `stem` templates in the shared maps cover Halo 1's shader types.
- Generating pathfinding that Halo 2's AI accepts.
