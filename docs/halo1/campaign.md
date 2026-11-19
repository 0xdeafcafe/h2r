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
  scratch also need it ([maps.md](../game/maps.md)).
- **`crates/h1models`** (Built) converts Halo 1 `mode` and `antr` tags to
  Halo 2 `mode` and `jmad` block trees for the builder
  ([models.md](models.md)).
- **`crates/h1bitmaps`** (Built) decodes Halo 1 `bitm` images to RGBA8
  with Halo 2's texture decoder and converts them to Halo 2 `bitm` block
  trees, pixels copied as they are ([bitmaps.md](bitmaps.md)).
- **The converter** (`crates/h1convert`) works one tag group at a time, in
  the order of §3: collision, structure BSP, placements, MOPP
  (`h2 h1 convert`), see [multiplayer.md](multiplayer.md).
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
| `shdr` family (`senv`, `soso`, `schi`, `scex`, `swat`, `sgla`, `smet`, `spla`) | `shad` written from its definition on a retail `stem` | each Halo 1 shader type (and flags: alpha-tested `senv`, additive `schi`/`sotr`) picks a stem in `shared.map` (sky layers: the sky stems where the base has them) and `h1convert::shad` fills its parameters from Halo 1's maps and tiling; no Halo 2 `shad` bytes are copied. An `senv` with detail maps draws on `tex_bump_detail_blend_detail` where the base has it (Coagulation, Beaver Creek, five campaign maps): the base alpha blends the primary (alpha 0) and secondary (alpha 1) details, then the micro detail, each a biased multiply, as Halo 1 does; elsewhere `tex_bump` with the primary detail. A normal (unblended) `senv` with a reflection cube map (`+0x324`) and a reflection brightness draws on `tex_bump_env`: Halo 1's cube map, its perpendicular and parallel colours (`+0x2a8`, `+0x2b4`) times the specular brightness (`+0x290`) as the facing and grazing tints, and the perpendicular and parallel brightnesses (`+0x2f4`, `+0x2f8`) as the facing and grazing brightness (`env_reflection_is_halo1s_rule`); a blended one keeps its blend without the reflection. Other specular colours are black (a Halo 1 base alpha is a blend or cut-out mask). Proof: `h1convert/tests/shad_rebuild.rs` (`detail_blend_is_halo1s_rule` runs the stem's colour pass on the CPU against Halo 1's rule). Water (`swat`) with a ripple map draws on `water_pool` where the base has it (Delta Approach, Beaver Creek, Coagulation): Halo 1's ripple map (`+0xc8`) as the bump, tiled by the ripple scale (`+0xc4`) and scrolling at the ripple velocity (`+0xc0`) along the ripple angle (`+0xbc`), and its reflection map (`+0x9c`), both offset by the ripple over the scene behind the water and weighed by retail's fresnel ramp (`a_swat_draws_on_water_pool_its_ripple_scrolling_along_the_ripple_angle`); elsewhere `one_alpha_env` with its base map. Halo 1's four extra ripple layers (`+0x124`), base map and tints have no place on `water_pool`. Not converted yet: model (`soso`) reflections (`soso +0x164`). The look will be close, not exact |
| `bitm` | `bitm` | the same Xbox formats and layouts: pixels copy unchanged (**Built**, [bitmaps.md](bitmaps.md)) |
| `mod2` + `coll` + `phys` | `mode` + `coll` + `phmo` under an `hlmt` | physics uses Halo 2's (decision 1) |
| `antr` | `jmad` | animation codecs differ: decode, then re-encode |
| `bipd`, `vehi`, `weap`, `eqip`, `scen`, `mach`, `ctrl` | the same groups | field-by-field. Halo 1 tuning values carry over where Halo 2 has the field |
| `actr` + `actv` | `char` | |
| encounters, squads, firing positions, command lists | squads, zones, firing positions, orders | |
| scripts | Halo 2 scripts | decompile Halo 1's syntax nodes, rename the functions that differ, compile to Halo 2 nodes with `crates/hs`. Builtins Halo 2 lacks go in the Rust HS VM ([hs_vm.md](../engine/hs_vm.md)). `crates/h1scripts`, [scripts.md](scripts.md) |
| `snd!` | `snd!` + `ugh!` | both Xbox ADPCM |
| `ustr`, `hmt ` | string IDs, `unic` | |
| recorded animations | recorded animations | versions differ |

## 3. Order of work

Each step is proven with tests; the user plays the result.

1. **Import and read.** Recognise the Halo 1 disc (title ID `4D530004`),
   pull out its maps, decompress them, list their tags.
   **Built:** `crates/h1map`, `h2 h1 info|tags|bitmap|convert`,
   `h2 import halo1`; format in
   [map-format.md](map-format.md). The disc's multiplayer maps
   are read and converted in tests ([multiplayer.md](multiplayer.md)).
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
   **Built for weapons, equipment, vehicles, scenery, placed bipeds,
   the cyborg as the player, and machines, controls and light fixtures**
   (§9; devices since converter version 38, from the `mach` and `ctrl`
   definitions, [tag-definitions.md](../formats/tag-definitions.md)).
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
   saved per campaign. **Built** ([campaign-picker.md](campaign-picker.md)),
   confirmed in the game.

### Open work (2026-11-19)

Everything the converter still lacks, each on its own branch
(`h1-push-<name>`). The branches are written without building; they are
merged, built and tested together, then played.

| Gap | Branch | Notes |
|---|---|---|
| a30 fails to convert at v95: a converted `ctrl` (b30's holo control) keeps a pointer-like word at `+0xb0` the fresh build can't move | `a30convert` | Found 2026-11-19; a30 can't run until it's fixed |
| Recorded animations and `recording_*`, `vehicle_hover`: the scripted Pelican, dropship, lifepod and banshee flights | `recordings` | Halo 1's recordings rebuilt in Halo 2's format |
| AI command lists (`ai_command_list*`) | `commandlists` | Halo 2 uses command scripts instead |
| AI conversations (`ai_conversation*`, `ai_dialogue_triggers`, `unit_set_emotion`) | `conversations` | |
| Encounter-level AI control (`ai_magically_see_players`, `ai_follow_*`, `ai_retreat`, `ai_go_to_vehicle`, ...) | `aicontrol` | Onto Halo 2 squads, zones and orders |
| Unit and object builtins (`unit_open`, `unit_close`, `unit_set_seat`, `unit_enter_vehicle`, `object_beautify`, ...) and the rest | `units` | |
| HUD help and objective text, Halo 1's string lists | `hudtext` | |
| Sentinels, the crewman, firing positions off the floor, squads with no starting locations | `airoster` | |
| Over-shield, active camouflage, b30's falling box as Halo 2's own powerup objects; health packs on the Halo 2 multiplayer setting; the lifepod as its own vehicle; a10's cryotube, chairs, fighter-bomber and ring as their own object types | `objects` | Today: missing, or built as turrets |
| Looping sounds past the file size limit; every other sound left out | `sounds` | The lifepod's loops come with its vehicle (`objects`) |
| The sniper's scope mask (`wphi +0xac`); the terrain shader's detail blend function and `+0x2c` block | `scopeshader` | |
| Halo 2 role values where Halo 1 has a value ([role-audit.md](role-audit.md)); the audit over the ten levels | `fullrebuild` | Every tag built from its definition with Halo 1's values |
| a30's AI behaviour in the game | `h1-a30-ai` | |
| a30's script flow: cutscenes, titles, checkpoints, missing globals, start-up encounters | `h1-a30-scripts` | Ties the builtin branches together |
| Multiplayer weapon HUD overlap; zoom only on scoped weapons | `h1-hud-zoom` | Its library tests pass; its disc tests run in the final build |

Not planned: race flags (Halo 2 has no race game type).

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
| Ammo pickups | Left out | Halo 2 has none, and the Halo 1 weapons they fill are gone |
| Warthog, rocket warthog | Warthog | The gauss warthog is a model variant a campaign placement does not name |
| Ghost, scorpion, banshee, wraith | The same | |
| Shade (`c gun turret`) | Covenant turret (`c_turret_ap`) | |
| Pelican, Covenant dropship, lifepod | Halo 1's, converted | Set pieces: the scripts fly them with Halo 1's recorded and custom animations and load the AI into their seats |
| The cyborg (the player) | Master Chief (`masterchief`) | Set as the campaign players' unit (`matg +0x138`) |
| Cutscene props (`..._cinematic`: a10's scorpion) | Halo 1's, converted | Cutscenes keep Halo 1's objects |
| Vehicles flown by a recording (`..._playback`: d20's banshee) | Halo 1's, converted | The recording replays Halo 1's controls, which on Halo 2's vehicle would fly another path |

A Halo 2 tag neither the base nor a donor has leaves the Halo 1 object
in place (converted); a `dropped` line says so.

**Kept Halo 1's in both variants:**

- **Placed bipeds.** They are the cutscenes' casts and the levels'
  bodies. Halo 1's cutscenes are animated on Halo 1's skeletons, so with
  Halo 2 assets they need retargeting; until that exists, cutscenes keep
  Halo 1's characters in both variants. They play on eight of the ten
  levels (§9, Cutscenes).
- **The AI's characters** are retail Halo 2 `char` tags in both
  (`h1ai::convert::character_for`): their bipeds and weapons are Halo 2's
  already, squads name them the same way (squad weapons -1, the
  character's own). The disc test checks the two variants pick the same
  characters.
- **Scenery, devices, sounds, text**: the level.

**The scripts' references.** Object names point at the placements as
before (the palette entry changed, not the placement). Each swap is kept
as an alias (`h2assets::Aliases`): the scripts' `object_definition`
atoms (`unit_has_weapon`, ...) and their reference block resolve a Halo
1 tag name to the Halo 2 tag placed for it. `object_definition` atoms
now resolve by tag name in the Halo 1 variant too (they were `none`).
Seat mappings already match Halo 2 vehicles' seats by role
(`h1scripts::seats`).

**Writing and serving it.** `h2 import halo1` writes it as the job
`<level>/h2` after every other job (Halo 1's assets are the default),
to `<data>/campaigns/halo1/h2/<level>.map`
(`h1_variants::campaign_alternate`), with no HUD pairs and no manifest of
its own. `boot::h1_variants::set_campaign_assets` flips the
`vfs::SandboxChoice` the campaign maps backend serves by
(`vfs::CampaignMapsBackend::with_assets`). With Halo 2 assets chosen and
no such file (an import older than converter version 55), the level is
served from its Halo 1 map and the first open says so once on stderr:
`campaigns: a30: no Halo 2 assets level (<path>); serving its Halo 1
map`. The choice applies at the level's next open: an open map keeps its
file, and `map_changed` lets `boot::map_slots` drop a cached copy of the
other file (slots 4 and 5). Each file has its own `MapId`. The Halo 1
map's HUD pairs (`<level>.hud.toml`) name its own datums, so `boot::hud_swap`
skips them while a level's Halo 2-assets map is served
(`hud_swap::campaign_halo2_levels`).

Tests: `h1campaign` `h2assets::tests` (the table, finding a role by
name, aliases); `vfs`
`campaign_maps::tests::the_assets_choice_picks_the_level_at_its_next_open`,
`without_a_halo2_assets_level_the_halo1_one_is_served`; `cli`
`h1_import::tests` (the job list and where `<level>/h2` writes); `boot`
`hud_swap::tests::campaign_levels_read_their_hud_files`. Against the
discs (ignored): `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo
test --release -p h1campaign --test assets -- --ignored --nocapture`
builds both variants of every level (`H1_LEVELS=a30,b40` for some): each
from scratch, every reference resolving, every script tag atom naming a
tag of its group; in the Halo 2 variant no Halo 1 item the table swaps
is placed or given, the player is the Chief, the AI's characters are the
Halo 1 variant's. Measured (2026-11-18,
converter version 55, 260 s for all twenty):

| Level | Tag memory, Halo 1 / Halo 2 assets | File, Halo 1 / Halo 2 | References resolving, Halo 2 |
|---|---|---|---|
| a10 | `0x8bc000` / `0x89f000` | 197,134,336 / 194,950,144 | 29,799/29,799 |
| a30 | `0x8c1000` / `0x88d000` | 117,349,376 / 113,353,216 | 28,652/28,652 |
| a50 | `0x8cd000` / `0x8c4000` | 165,808,128 / 163,699,200 | 30,143/30,143 |
| b30 | `0x86c000` / `0x82e000` | 130,685,952 / 126,113,792 | 29,138/29,138 |
| b40 | `0x966000` / `0x8f5000` | 210,296,832 / 202,618,880 | 30,708/30,708 |
| c10 | `0x8a4000` / `0x892000` | 187,266,048 / 184,510,464 | 29,330/29,330 |
| c20 | `0x787000` / `0x770000` | 157,552,128 / 155,148,800 | 21,997/21,997 |
| c40 | `0x89b000` / `0x834000` | 218,715,136 / 211,270,656 | 27,788/27,788 |
| d20 | `0x8a7000` / `0x885000` | 176,786,432 / 174,510,592 | 27,310/27,310 |
| d40 | `0x8cf000` / `0x89b000` | 258,213,376 / 254,423,552 | 28,061/28,061 |

Both variants of every level build from scratch with 0 dangling
references and every script tag atom resolving; the Halo 2 variant is a
little smaller everywhere. Every retail tag the table names was already in
05a (no donor was needed). Swapped on the ten levels: the assault rifle,
pistol, sniper rifle, rocket launcher, shotgun, needler, plasma pistol and
rifle, both grenades, the warthog, ghost, scorpion, banshee, wraith and
shade; ammo pickups dropped (26 palette entries).

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

**Built** (tests only): the `h1-health` feature and its switch,
`boot::health::set_mode`. Nothing calls the switch yet, so the model is
on only through the debug override. Where Halo 2 regenerates the body,
how its own health powerup (`eqip` type 5) refills it, the HUD's body
input, what the converter must write, and the debug switch
`H2_HEALTH_MODEL=halo1`: [health.md](health.md).

## 7. Multiplayer

Halo 1's multiplayer maps follow the same rules, with a sandbox setting:
[multiplayer.md](multiplayer.md).

## 9. The ten levels on a pinned base (**Built**, tests only)

`crates/h1campaign`; `h2 h1 convert disc/halo1.iso <level> --out DIR
[--disc disc/halo2.iso] [--base MAP] [--dry-run] [--force]` writes
`DIR/<level>.map` and `DIR/campaign.toml` (Halo 1's manifest,
`content::campaign_list::HALO1`, unless one is there already). With
`DIR = <data>/campaigns/halo1` the campaign picker's Halo 1 entry lists
the level and `vfs::CampaignMapsBackend` serves it as `maps\<level>.map`;
the map's header name is the level's and its scenario path
`scenarios\solo\<level>\<level>`, the path its start record names.

**Fresh by default; `--pinned` keeps the old way.** A level is first
built on the pinned base as before (the intermediate), then written
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
   | `+0x200`, `+0x358`, `+0x3b8` chapter title, subtitles, objectives | 05a's | null |
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
   cannot take whole fails the build. Equal blocks are shared across
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
3. **Fallback: pruned.** If the build from scratch fails, the level is
   built the slice-before way (a `dropped` line says why): every tag of
   the intermediate laid out anew (`h1convert::fresh::unpin_all` with
   `built_too`), the unreachable base tags dropped (`fresh::prune_with`,
   `KEEP`), on 05a's index. No level takes it today.

**Measured** (`tests/fresh.rs`, `every_level_converts_fresh`, 2026-11-17,
converter version 28, the gestalt cut; pinned is the old way, lean ladder
and all; pruned is version 23's fresh build):

| Level | Tag memory pinned | pruned | from scratch | File pinned | pruned | from scratch | Tags | References resolving |
|---|---|---|---|---|---|---|---|---|
| a10 | `0xb44000` | `0x89b000` | `0x810000` | 287,119,360 | 257,813,504 | 110,644,736 | 7,027 | 30,437/30,437 |
| a30 | `0xb3c000` | `0x8e5000` | `0x85c000` | 232,101,376 | 176,360,448 | 57,287,168 | 6,905 | 29,658/29,658 |
| a50 | `0xbeb000` | `0x8cf000` | `0x83e000` | 241,578,496 | 190,463,488 | 65,308,672 | 7,072 | 30,306/30,306 |
| b30 | `0xb29000` | `0x85e000` | `0x7db000` | 230,040,064 | 171,379,200 | 53,124,608 | 7,035 | 29,721/29,721 |
| b40 | `0xc22000` | `0x923000` | `0x891000` | 265,091,072 | 229,058,048 | 94,251,520 | 7,309 | 32,135/32,135 |
| c10 | `0xc01000` | `0x8a1000` | `0x7f1000` | 260,217,344 | 210,515,968 | 82,070,016 | 6,678 | 30,538/30,538 |
| c20 | `0xb3c000` | `0x83f000` | `0x745000` | 257,913,344 | 208,459,264 | 78,148,096 | 4,205 | 22,866/22,866 |
| c40 | `0xbe1000` | `0x8b7000` | `0x7d9000` | 272,847,360 | 231,427,072 | 93,013,504 | 6,086 | 29,241/29,241 |
| d20 | `0xbab000` | `0x8ed000` | `0x81a000` | 247,974,912 | 200,224,768 | 72,000,512 | 5,842 | 27,541/27,541 |
| d40 | `0xbec000` | `0x8ee000` | `0x815000` | 293,278,720 | 259,985,920 | 113,948,672 | 6,020 | 29,179/29,179 |

The bound is `0xc24000`: every level has 3.6 to 4.9 MB to spare, and the
lean ladder fires nowhere (b40 keeps its level data, seat table and object
textures; the ladder stays as a safety net). The file bound is
`0x11800000` (293,601,280 bytes). On every level the map is built from
scratch with one index slot per tag, and: no tag from
`scenarios\solo\05a_deltaapproach\` or `scenarios\multi\`, every script
atom naming an object definition, sound, effect, damage effect or
animation graph resolves to a tag of that group with data, and the
scripts' reference block reads back (`hs::Scenario::read`). The reference
sweep counts only problems the pinned map of the same level does not have
(none on any level). If the fresh map would pass the file bound the
pinned one is written instead (`Report::fresh` false, a `dropped` line).

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

From scratch takes less tag memory than pruned (a30 `0x85c000` against
`0x8e5000`): the dense index, the cut gestalt, and equal blocks placed
once. Scoping the sharing to each tag had cost about 1 MB a level, the
arrays retail itself shares across `effe`, `shad`, `styl` and `prt3`
tags.

**The sound gestalt cut to the kept sounds** (`h1convert::compact`, run by
the builder from scratch). 05a's `ugh!` holds every 05a sound's entries;
a new gestalt is built from nothing, each kept `snd!` in index order
taking its entries across (`h1sounds::merge::merge_sound`) with its
promotion carried whole (its rules index the sound's own pitch ranges:
`compact::PROMOTION_EVIDENCE`, V on 05a, Coagulation and 03a), and the raw
no chunk names any more is emptied. a30: 3,270 sounds kept, 18,053 chunks
to 14,344, 3,830 raw resources (108.9 MB, 05a's own dialogue and music)
dropped; on every level each kept sound names the same entries, import
names and chunk bytes as in the pinned map (`compact::map_entries`,
`every_level_converts_fresh`). A gestalt the cut cannot read stays whole
(`ScratchReport::gestalt_whole`).

**Base per level: `05a_deltaapproach` for all ten** (`h1campaign::base_for`).
Why (surveys `base_memory_and_vehicles`, `survey_bases`):

- It is the only campaign map with every vehicle template Halo 1's levels
  use: warthog, ghost, scorpion, banshee, pelican,
  phantom (for the dropship), wraith and `c_turret_ap`, and the pelican
  again for the lifepod (by role, objects.md). 03b has most of
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

With b40's repeated firing positions shared (ai.md, Spawning), all ten
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
| Structure | Every Halo 1 BSP, in Halo 1's order: into the base's BSP slot of the same index, past the base's three into new `sbsp`/`ltmp` tags under Halo 1's names (templates from the base's first), with `h1convert::structure` (collision, render geometry, materials matched to the base's shaders and drawing Halo 1's own textures on copies of them (`h1convert::textures`), Halo 1's lightmap as a new `bitm`, MOPP, audibility, sound clusters, each cluster's background sound and sound environment from Halo 1's ([multiplayer.md](multiplayer.md), "Ambient sound")) and pathfinding generated from the collision at `sbsp+0xc4` | |
| Sky | Halo 1's skies (`h1sky`), scaled to stay behind the level (sky.md, "The model scale"), every cluster on the first; the model's materials name copies of the base's sky shaders with Halo 1's bitmaps and map scale (sky.md, "The model's shaders"), else the base sky's shader, never NONE (`0x1bbf0` draws NONE, a null read in `0x16b90`; multiplayer.md) | |
| Scenario | The base `scnr` relocated as a block tree, replaced fields emptied. One BSP descriptor per Halo 1 BSP (`FixupKind::Bsp`, its `sbsp` and `ltmp`), so `switch_bsp`, zone and placement BSP indices keep Halo 1's meaning. The two root blocks every retail map holds one element per BSP of (survey `per_bsp_scenario_blocks`): cluster data `+0x290` (per cluster its background sound and sound environment, as the structure's cluster `+0x72`/`+0x74` names them, and the cluster's centroid, and the BSP's checksum `sbsp+0x08`) and `+0x388` (no points). BSP switch trigger volumes `+0x130`: Halo 1's (`scnr+0x39c`, 8 bytes, survey `h1_bsp_switch_volumes`) on its converted volumes | The base's own BSP switch volumes |
| Player | The level's starting locations, each on the base's first element. The cyborg is converted and set as the campaign players' unit (`matg +0x138` elements 0 and 1, `h1convert::globals::set_player_unit`); Halo 1 health follows from the map name (`boot::health`) | |
| Objects | Every base placement removed. Weapons, equipment, vehicles, scenery, placed bipeds and devices from the level's palettes through `h1objects` (palettes keep Halo 1's order, a null entry for one that doesn't convert; templates in objects.md), placements with unique IDs `0x4831xxxx`, origin BSP -1, source 1. Object names are Halo 1's, each pointing at its converted placement or -1. A palette entry whose template only a donor brought places the retail object itself. Halo 1's "not placed automatically" (bit 0 of the `u16` at `+4`) is written as Halo 2's "not at start" (bit 0 of the `u32` at `+4`), so cutscene casts and props wait for the scripts that create them (a10: 67 of 107 bipeds, 51 of 56 vehicles, 92 of 274 scenery) | Palette entries with no template: over shield and camouflage (no powerup on 05a), b30's falling box. Vehicles with no template (a10's cryotube, chairs, fighterbomber and Halo ring) are built from the `vehi` definition as turrets since converter version 33, with a physics model so `vehicle_new` creates them (a stand-in sphere in version 38, Halo 1's collision as Havok polyhedra since 43; tag-definitions.md). Machines, controls and light fixtures (as machines) are placed since version 38 with their power and position device groups and flags; since version 43 a device with solid collision has a physics model built from it, a body per moving node, so a closed door blocks the player; fixtures give no light |
| Trigger volumes | The base's parked (`h1convert::convert::PARKED`, object name -1); the level's appended after them | |
| Donors | `h1campaign::donor`: each character `h1ai`'s table names, copied whole from the first donor that has it (`03a_oldmombasa`, `04b_floodlab`, `06a_sentinelwalls`) by `h1sounds::transplant`: the tag tree, its raw, and its sounds' gestalt entries | The sentinel (its animation graph does not relocate) and the crewman (no donor) |
| AI | `h1ai`: squad groups, squads, zones, orders, character palette. Characters the base lacks and no donor brought fall back to the marine (marine kinds) or the folder's first | Sentinels: `none` |
| Scripts | `h1scripts` with `Missing::Stub` and each level's cutscene policy (below); `ai_place` with one argument. A startup script `h1_initial_encounters` places the encounters Halo 1 creates at load. Hooks: object, unit and vehicle names, `short`, team and difficulty as `0xffff0000 \| index`; trigger volumes as the index; AI squads as the index and squad groups as `0x40000000 \| index`; string IDs interned; sounds as the converted tag's datum (encodings V: every such atom in 05a's and 03a's retail scripts). Halo 1's cutscene flags, camera points, titles and device groups written to Halo 2's blocks in Halo 1's order (`h1scripts::level`), their atoms the index (a device group's runtime datum); the seat-mapping table `+0x228` built from the vehicles' seat labels (`h1scripts::seats`); navpoints `default`; looping sounds, effects, damage and animation graphs by tag name among the converted tags. Squads with no starting locations that the scripts name are kept empty (`h1ai`), so their names resolve. Script references resolved by tag name ([scripts.md](scripts.md), "Level data the scripts name") | Looping sounds left out for room (Sounds, below), effects, damage effects, animation graphs and object definitions with no converted tag of that name: `none` |
| Sounds | The `snd!` tags the scripts name (`h1sounds`, onto the base's gestalt), halving the count until the gestalt's 16-bit indices fit; each permutation's length at `+6`, which the scripts' waits read (`sound_impulse_time` is Halo 2's `sound_impulse_language_time`). `h1sounds::audit` reads every sound of a converted level back: a10's 3,988 play exactly what they were made from. The looping sounds (`lsnd`: music and ambience) the scripts start, stop or switch (`sound_looping_start`, `_stop`, `_set_alternate`), with their tracks' sounds (`h1sounds::looping`), in first-use order while they fit the map file (Music, below). Every BSP's background sounds and sound environments (`h1convert::ambience`, all BSPs' together) as the scenario's palettes `+0x250`/`+0x258` and each structure's copy (`sbsp+0xd4`/`+0xdc`) | Every other sound; looping sounds past the map file's size limit; lifepods drop the pelican template's engine loops |
| Text | Nothing: the base's English table stays as it is. `h1text::Locales::extending` builds on the base's table (its rows and blob first, new rows after) for when Halo 1's text is converted | Halo 1's string lists and HUD messages; the scripts' HUD text builtins are stubbed |

### Cutscenes (played on eight levels)

`h1campaign::Options::cutscenes` is `None` by default: each level takes
its own policy from `h1campaign::cutscenes::LEVELS` (`Some(..)` forces
one). `Cutscenes::Play` compiles the scripts as written;
`PlayShortPaths` plays the cinematics but still wakes a10's
`fast_setup` for `tutorial_setup` (`SHORT_PATHS`: the cryo tutorial
waits on `player_action_test_look_relative_left`/`_right`, which the
progression lane maps; until then it would hold input forever); `Skip`
compiles every call of the level's `cinematic_skip_start` guard as
`false`, the path Halo 1 takes after a skip (its fade in,
`cinematic_stop`, control back), and takes the short paths too.

| Level | Policy | Why |
|---|---|---|
| a10 | `PlayShortPaths` | every cutscene converts; the tutorial is the progression lane's |
| a30, a50, b30, c20, d20 | `Play` | every cutscene converts |
| c10, c40 | `Play` | every cutscene converts; the command lists that walk actors through x50 and c40's cinematics are stubbed (they stand) |
| b40 | `Skip` | at the tag memory bound: converted without level data, graphs and recordings (`Lean::LevelData`), so its cinematics would have no camera points, flags or animations |
| d40 | `Skip` | over the campaign cache's file size (`0x11800000`) with its cutscene graphs: they need `0x1baa00` bytes of animation data and `0x7c000` are left, so its characters would not animate |

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
  (`h1models::graph_from_map`, under its Halo 1 name, so the
  `animation_graph` hook resolves by name), and each animation name a
  script passes becomes the converted animation's name
  (`h1models::jmad::animation_names`; `h1scripts::compile::
  ANIMATION_NAME` hooks carry `<graph>|<animation>`). The cutscene
  characters already have their own Halo 1 model and graph (h1objects'
  placed bipeds, on the Chief's template), so a cinematic graph animates
  the skeleton it was made for. 9 graphs on a10, 2 to 9 elsewhere.
- **Recorded animations.** Halo 2 keeps them: E, `0x001fb360` (called by
  builtins 138, 139 and 140 with play flags 0, 8 and `0x10`) bounds the
  index by `scnr +0x110` and reads `0x34`-byte elements at `+0x114`:
  version `+0x20`, unit control data version `+0x22`, length `+0x24`, the
  event stream's pointer `+0x30`. That is Halo 1's element (`scnr
  +0x36c`, `0x40`) with Halo 2's 8-byte data reference. No retail Halo 2
  scenario has one. `read_recordings`/`recordings_block` copy the first
  `0x2c` bytes and the stream as they are; `cutscene_recording` hooks
  are the index in Halo 1's order. Every recording on the ten levels is
  version 4 (Halo 2's decoder table at `0x0046fd5c` has four versions);
  unit control versions 0, 3 and 4. `recording_play`, `_and_delete`,
  `_and_hover`, `recording_kill` and `recording_time` are 138 to 142.
  **Streams rewritten (converter version 28).** Copied as they are,
  Halo 1's streams faulted Halo 2's player (a30 and a50 headless: calls
  to pc 4 and pc 1 from `0x0029f13d`, under `0x001fb61e`), and version
  27 held the five builtins as stubs. Both games' version-4 players are
  the same loop: a header byte whose low two bits give the delay (none,
  one tick, a byte, a short) and whose upper six bits are the event type,
  dispatched through a handler table with no bound (Halo 2 `0x004710f0`,
  24 types; Halo 1 `0x00232538`, 23). The payloads differ in one place:
  control flags (type 4) are two bytes in Halo 1 and four in Halo 2, so
  Halo 2 lost its place in 189 of the 349 streams, most of them then
  reading a payload byte as a type past the table (the word after it is
  `0x00000001`). The
  initial unit control state differs too (Halo 2's fields are wider).
  `h1campaign::recordings` rewrites each stream: the state field by
  field, flag events widened, animation-state events (Halo 2's state is
  a mode string ID a byte cannot carry) folded into the initial mode,
  and only Halo 1's movement and stance flags (bits 0 to 9) kept: Halo 2
  moved the weapon actions to its high half, so recorded units move and
  look but do not fire. The disc test `every_converted_recording_walks_in_halo_2`
  walks every rewritten stream through Halo 2's table, and
  `decoder_tables_match_the_executables` checks the tables against both
  `default.xbe` files.
- **Camera.** `camera_set`, `camera_control`, `camera_time` map directly;
  `camera_set_relative` (451), `camera_set_first_person` (456) and
  `camera_set_dead` (457, an object in Halo 2) by position. No level
  calls `camera_set_animation` (452, mapped too). Camera points are the
  level data (`h1scripts::level`).
- **Dialogue.** `sound_impulse_start` maps directly. `sound_impulse_time`
  (493 calls; the scripts sleep on it after almost every line, and wait
  `(sleep_until (= 0 (sound_impulse_time line)))` for a line to finish)
  is Halo 2's `sound_impulse_language_time` (scripts.md, Renamed): the
  ticks the playing line has left, 0 once it ends. A constant length
  would never reach 0, so the cutscene round's length hook was dropped
  when the lanes were integrated. Looping sounds are the music lane's.
- **Flags, fades, titles.** `cinematic_start`/`stop`,
  `cinematic_show_letterbox`, `cinematic_set_title`, `fade_in`/`out`,
  `player_enable_input`, `player_camera_control`, `switch_bsp`,
  `object_teleport` map directly; `cinematic_set_title_delayed` (561),
  `object_set_facing` (106), `object_pvs_set_camera` (471), `show_hud`
  (625, the first of three boolean-to-boolean builtins there),
  `unit_open`/`unit_close` (194, 195), `unit_suspended` (249),
  `unit_enter_vehicle` (216) and `vehicle_hover` (621) by position.

Still stubbed inside cinematics (`cutscenes_convert_and_read_back` lists
them per level; none is waited on): `object_beautify`,
`object_pvs_activate`, `unit_set_emotion`, `unit_set_seat`, the
`cinematic_screen_effect_set_*` effects, `cinematic_set_near_clip_distance`,
`player_effect_set_max_translation` (camera shake), and on c10, c40, c20
and d40 the command lists (`ai_command_list_by_unit`, `ai_attach_free`).
`unit_enter_vehicle` passes Halo 1's seat text as the string ID, which no
Halo 2 seat label matches yet.

**a10's opening (`x10`), call by call** (2026-11-18, converter version
67). `x10` calls `autumn_glory_1`, `autumn_glory_2` (space, `a10_space`),
`bridge` (BSP 1), then `hangar_1`, `hangar_2` and `cryo` (`x10hangar`, the
cryo bay). Its fades, camera moves, letterbox, BSP switches and sounds
are the Halo 2 builtins of the same name with the same parameter types,
so every duration stays a count of ticks (`cinematic_calls_keep_halo1_s_ticks`,
h1scripts: `fade_in`/`fade_out` `(real real real short)`, `camera_set`
`(cutscene_camera_point short)`; retail Halo 2's own literals are 15, 30,
60, 500 ticks). No fade or camera call is mistranslated. Stubbed in `x10`:
`render_lights` (`hangar_1` turns dynamic lights off, `hangar_2` on),
`rasterizer_lights_reset_for_new_map` (space), `game_skip_ticks`,
`object_pvs_activate`, `cinematic_set_near_clip_distance` (0.0625, 0.01
on the bridge), and the three `cinematic_screen_effect_set_*` calls, while
`cinematic_screen_effect_start` (705) is kept: the bridge's last second
starts Halo 2's screen effect with nothing set. The cinematic's
"big whirring sound" was a10's continuous `dialog_flavor_captain`
restarting each tick (scripts.md, Sleeping forever), fixed in version 67.
The reported flicker has no cause in the script translation; what a run
should check is in the lane's report.

### Waits on stubs (playing a level through)

`h1scripts`' disc test `waits_on_stubs` counts, per level, the
`sleep_until` and `if` conditions that read a stubbed builtin, and which
of them nothing handles (a stub not in `DONE_VALUES`, not `ai_status`,
and not one whose zero is already the done value: `recording_time`,
`ai_going_to_vehicle`). What reads a stub most, over all ten levels (uses, before this round):

| Stub | Uses | Now |
|---|---|---|
| `ai_status` | 35 (a50 16, b40 17, b30 2) | `ai_combat_status` on Halo 1's scale (scripts.md, Step 2) |
| `ai_conversation_status` | 34 | done value 5 |
| `ai_command_list_status` | 20 | done value 2 |
| `game_all_quiet` | 17 | done value true |
| `ai_living_fraction` | 14 (a30) | `ai_strength` |
| `device_get_power` | 8 (d40) | Halo 2's unnamed builtin 255 (`IDENTIFIED`) |
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

**Tag memory and file size.** Graphs put their animation data in the
map file (raw; the campaign cache bounds the file at `0x11800000`),
recordings their streams in tag memory, inside the scenario. Per level, tag memory and file with them, and tag memory
without (`Options::cutscene_data` off):

| Level | Graph data | Recordings (bytes) | Tag memory | Without | File |
|---|---|---|---|---|---|
| a10 | `0x11ca00` | 99 (44,865) | `0xbc6000` | `0xb47000` | `0x11373200` |
| a30 | `0x2d000` | 63 (64,122) | `0xb4d000` | `0xb3c000` | `0xdd6ca00` |
| a50 | `0xa5a00` | 34 (39,276) | `0xbf7000` | `0xbed000` | `0xe69cc00` |
| b30 | `0x1d600` | 34 (44,392) | `0xb33000` | `0xb27000` | `0xdb5dc00` |
| b40 | none (`Lean::LevelData`; needs `0x92000`) | none | `0xc22000` | `0xc22000` | `0xfccfc00` |
| c10 | `0xc6800` | 51 (97,184) | `0xc1d000` | `0xc02000` | `0xf90de00` |
| c20 | `0x18400` | 1 (1,263) | `0xb37000` | `0xb36000` | `0xf5b1e00` |
| c40 | `0xdaa00` | 12 (5,459) | `0xbe3000` | `0xbdf000` | `0x104e6c00` |
| d20 | `0x3ca00` | 20 (21,412) | `0xbb1000` | `0xbab000` | `0xecc0400` |
| d40 | none (file size; needs `0x1baa00`) | 11 | `0xbed000` | `0xbea000` | `0x11787000` |

Since converter version 38 places devices, pinned b40 (tag memory,
`0xc23000`) and pinned d40 (the file, `0x117b9800` bytes, with no music,
ambience or cutscene graphs) place none (`Devices::None`); fresh b40 and
d40 keep everything.

a10's jump (`0x7f000`) is its scenario no longer fitting the base
scenario's old range and being appended (`0xb512c` bytes). The lean
ladder (`h1campaign::Lean`) drops the graphs and recordings
(`CutsceneData`) after the music and before the level data; a level over
the campaign cache's file size is converted again with less music, then
without the background sounds, then without the graphs (Engine caps, "Music, ambience and the map file"). To play b40's cutscenes, b40 needs
its level data plus the cinematics' graphs and 24 recordings back, which
is what dropping the base level's unreachable tags (`h1convert::fresh`'s
prune, for campaign) would free; d40 needs `0x1baa00` bytes of file, which
the same prune frees from the base's raw.

Placed objects Halo 1 leaves to the scripts are not created at load
(Objects, above): a10's x10 crew in the space BSP, Keyes and the bridge
crew, the hangar marines. The cinematics create them; skipped ones never
do.

Proof: `skipped_cutscenes_take_halo1_s_skip_path`,
`a_dialogue_length_is_a_hooked_number_the_converter_fills`,
`an_animation_name_is_hooked_with_its_graph` (h1scripts, synthetic);
`skipped_cutscenes_on_every_level` (h1scripts disc);
`cutscenes_convert_and_read_back` (h1campaign disc, `H1_LEVELS` for
some): per level, played, it converts, reopens the map and checks that
`scnr +0x110` holds Halo 1's recordings in order with their streams byte
for byte, and that every graph it converted is a `jmad` under its name;
`every_level_s_scripts_run_on_the_retail_interpreter` with the
identified builtins; `a30_converts_onto_its_base_and_reads_back` checks
every placement's flags against Halo 1's; `h1_placement_flags` and
`h1_initial_encounters` (survey) print the evidence.

**Needs a game run (cutscenes):** that Halo 2's decoders play Halo 1's
version-4 event streams and unit control data as recorded; that
`custom_animation` takes a converted graph on a converted biped (node
list checksums are Halo 1's on both); every identified builtin's
behaviour (they are found by position and signature, E only for the
recordings); the dialogue sleeps lining up with the converted sounds.

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
a30. The object sweep (objects.md; also over the retail base by
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
  index (`0x0027305e`) (ai.md). Every other level fits with them (the most: c40, 227).
  Survey: `ai_counts_against_the_caps`, `empty_squad_uses`.
- **Cutscene flags, camera points, titles and device groups.** The
  engine indexes these blocks without a check (b40 faulted at
  `0x002a003f` in `object_teleport`, reading flag -1): a call naming one
  that does not resolve is stubbed (scripts.md, Indexed values).
  `every_level_s_scripts_run_on_the_retail_interpreter` checks every
  reached atom of these types against its converted block on all ten
  levels. Names resolve without case (b30 and d40 named 3 and 9 flags
  and a device group in another case and lost them).
- **Units opened or closed.** `unit_open` and `unit_close` do not check
  their unit for `none` (`0x00113eae` reads `+0x33e` through object
  header 0xffff's null pointer), so a call naming an object name with no
  placement is stubbed (scripts.md, Units the engine does not check for
  none). a10 faulted there as its opening cinematic ended:
  `fast_setup`'s `(unit_open cryotube_1)`, the cryotube then having no
  template. Since converter version 33 the cryotube is built from its
  definition and placed (tag-definitions.md, Vehicles with no template),
  and a10 stubs no `unit_open`/`unit_close`. It still faulted (converter
  version 33, A pressed in the opening cinematic): the placement is never
  created. `vehicle_new` (`0x000eeb80`, the `vehi` type's new function)
  returns false when the definition's model (`hlmt`, object `+0x38`) has
  no physics model (`hlmt +0x20` `phmo`; `cmp [edx+24h],-1` at
  `0x000eebcb`), `object_new` (`0x000b7b40`) deletes the half-made object
  and returns `none`, and the name stays `none`. Every retail vehicle's
  model has one; the vehicles built from their definition (cryotube,
  chairs, fighterbomber, Halo ring) and fresh a10's
  `lifepod_atmosphere_entry` have none. Since converter version 35 every
  `unit_open`/`unit_close` runs inside `(if (> (list_count x) 0) ..)`,
  a no-op on `none` as in Halo 1 (scripts.md).
  `a10_opens_and_closes_only_placed_units` checks every reached call on
  a10 (fresh) is placed and guarded;
  `a10_cryotubes_are_placed_and_built_from_their_definition` checks the
  cryotubes' placements and tag; `a10_scripted_vehicles_can_be_created`
  checks every vehicle palette entry's model has one. Since converter
  version 38 a vehicle whose model has none (the rebuilt ones, and a
  templated one whose template's physics reaches past the converted
  nodes, as fresh a10's lifepod) gets one, so `vehicle_new` creates it:
  a stand-in sphere in version 38, Halo 1's collision as Havok
  polyhedra since 43 (tag-definitions.md, Physics models from Halo 1).
- **b40's level data.** b40 is at the tag memory bound. A level over
  it is converted again leaner (`h1campaign::convert`, `Lean`), dropping
  in turn: the empty squads kept for their names (`EmptySquads`), the
  seat table (`SeatTable`), Halo 1's textures on object models
  (`ObjectTextures`), the music (`Music`), the cutscenes' graphs and
  recordings (`CutsceneData`), the level data (`LevelData`). A level
  still over this bound or the file's (below) places fewer devices
  (`Options::devices`): no light fixtures (lamp models that give no
  light in Halo 2), then none. b40 still
  converts only without its level data (`0xc22000` of `0xc24000`): its
  74 calls naming a flag, camera point, title or device group are
  stubbed (the insertion's teleports and camera moves among them), and
  it has no music.
- **Music, ambience and the map file.** Halo 2's campaign map file is
  capped at `0x11800000` bytes (`map::writer::cache_size_limit`, the
  header validator's table at `0x00214ed0`). On the pinned base (its own
  raw kept whole) a level's BSPs, textures, sounds and the clusters'
  background sounds (`h1convert::ambience`, from `0x177810` bytes on c20
  to `0xc8a108` on c10) leave little for the scripts' looping sounds,
  which Halo 1 ships as Xbox ADPCM, kept as they are. A level over the cap
  (`Error::FileSize`) is converted again with that many fewer music bytes
  (and 256 KiB margin; the looping sounds go in in the order the scripts
  first name them while they fit), down to none; then with that many
  fewer background sound bytes (`scenario::bound_ambience`: an entry left
  out keeps its palette slot and tracks, with null sounds, so the
  clusters' indices stand), down to none; then without the cutscenes'
  animation graphs, which the cutscenes need. Each retry is in the
  report (`Report::retries`). Only d40 goes past the music: without music
  it is still `0x12254800`, so its 13 background sounds are silenced and
  its graphs dropped (`0x118f0c00` without them). The sound-zone split of
  the clusters (`h1convert::structure::sound_zones`) costs next to
  nothing: Halo 1's sections already outnumber the zones everywhere but
  c40's BSPs 3 and 4 (1 and 2 sections to 4: 322 and 710 vertices more).
  Building the levels fresh (no pinned base, its unreachable raw
  dropped) frees 30 to 60 MB of file a level and is the fix.

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
- scenery built from the `scen` definition (converter version 50, no
  template; tag-definitions.md): drawn at every distance, lit, and
  blocking through its `coll`'s BSP physics; placed bipeds;
- the pelican, phantom-for-dropship, wraith and turret templates under
  Halo 1 models;
- every BSP past the third (new `sbsp`/`ltmp` tags and descriptors, the
  rebuilt cluster data), reached by the scripts' `switch_bsp` and by
  Halo 1's BSP switch volumes, which Halo 2's engine is expected to
  switch on as it does for its own;
- the copied characters (hunter, flood, monitor) and their sounds, and
  lifepods (built from the `vehi` definition since converter version 49,
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
   map on stderr, `halo1: [3/36] campaign a50: done (5.4 s)`. On the
   owner's discs (2026-11-17) that is 36 maps (ten levels, 13 multiplayer
   maps in two sandboxes) in about 90 s of a release build, the disc's
   hash included. Later starts print `halo1: up to date (36 maps from
   halo1.iso)` and go straight on. Since converter version 55 each level's
   Halo 2-assets variant (§5) comes after those, ten more jobs (46),
   `halo1: [37/46] campaign a10 (Halo 2 assets): done`.
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
order, then the multiplayer maps the disc has, both sandboxes, then each
level's Halo 2-assets variant); where each writes (the folders
`boot::new_maps` and the campaigns read, the variant where
`h1_variants::campaign_alternate` looks); the stamp
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
picker's Halo 1 lists the ten levels, the VFS serves each level's map
(and its Halo 2-assets map when those are chosen), and
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
