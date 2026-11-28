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
              tag store level halo1_campaign_<level>_h1  ──▶ Halo 2 engine
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
| `shdr` family (`senv`, `soso`, `schi`, `scex`, `swat`, `sgla`, `smet`, `spla`) | `shad` written from its definition on a retail `stem` | each Halo 1 shader type (and flags: alpha-tested `senv`, additive `schi`/`sotr`) picks a stem in `shared.map` (sky layers: the sky stems where the base has them) and `h1convert::shad` fills its parameters from Halo 1's maps and tiling; no Halo 2 `shad` bytes are copied. An `senv` with detail maps draws on `tex_bump_detail_blend_detail` where the base has it (Coagulation, Beaver Creek, five campaign maps): the base alpha blends the primary (alpha 0) and secondary (alpha 1) details, then the micro detail, each a biased multiply, as Halo 1 does; elsewhere `tex_bump` with the primary detail. A normal (unblended) `senv` with a reflection cube map (`+0x324`) and a reflection brightness draws on `tex_bump_env`: Halo 1's cube map, its perpendicular and parallel colours (`+0x2a8`, `+0x2b4`) times the specular brightness (`+0x290`) as the facing and grazing tints, and the perpendicular and parallel brightnesses (`+0x2f4`, `+0x2f8`) as the facing and grazing brightness (`env_reflection_is_halo1s_rule`); a blended one keeps its blend without the reflection. Other specular colours are black (a Halo 1 base alpha is a blend or cut-out mask). Proof: `h1convert/tests/shad_rebuild.rs` (`detail_blend_is_halo1s_rule` runs the stem's colour pass on the CPU against Halo 1's rule). Water (`swat`) with a ripple map draws on `water_pool` where the base has it (Delta Approach, Beaver Creek, Coagulation): Halo 1's ripple map (`+0xc8`) as the bump, tiled by the ripple scale (`+0xc4`) and scrolling at the ripple velocity (`+0xc0`) along the ripple angle (`+0xbc`), and its reflection map (`+0x9c`), both offset by the ripple over the scene behind the water and weighed by retail's fresnel ramp (`a_swat_draws_on_water_pool_its_ripple_scrolling_along_the_ripple_angle`); elsewhere `one_alpha_env` with its base map. Halo 1's four extra ripple layers (`+0x124`), base map and tints have no place on `water_pool`. A `schi`, `scex` or `sotr` with no stage, or only null maps, draws nothing in Halo 1: the structure drops its parts (`textures::drop_no_draw_meshes`), its collision material keeps no shader, and the report's texture notes name it (`sky_shaders::draws_nothing`); a shader with maps that fail to read stays an error. A `soso` with no map, no reflection and no change colour (the spray and waterfall emitters' `invisible`) draws the white lit surface on `tex_bump` (`shad::textureless_layout`), as a map-less `senv` does; its group is not read off the disc yet (the role audit names the waterfall emitter's as `soso invisible`), and any other shader with no usable map is the named error `unsupported shader definition (group ..)`. Not converted yet: model (`soso`) reflections (`soso +0x164`). The look will be close, not exact |
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
   every BSP each); a30 has run headless in the game.
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
map from the tag store as `maps\<map>.map`.

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
| Hunter's fuel rod cannon | Halo 1's, converted | The hunter's own arm; Halo 2 has no hunter weapon to stand in (the row precedes the plain fuel rod's in `h2assets::SWAPS`) |
| Flamethrower | Shotgun | Close range; Halo 2 has none (multiplayer's choice) |
| Energy sword | Energy blade | |
| Frag and plasma grenades | Halo 2's | The globals keep the base's grenade types (Halo 1's are not written) |
| Health pack | Halo 1's, converted | Halo 1's health model stays whatever the assets (§6) |
| Overshield, active camouflage | Halo 1's, converted | The engine reads the rebuilt `eqip` type and duration |
| Ammo pickups | Left out | Halo 2 has none, and the Halo 1 weapons they fill are gone; a placement skips it with a `dropped` report line |
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
variants' weapons). `stand_in` answers `StandIn::Retail`, `StandIn::Own`
(Halo 1's, converted) or `StandIn::Dropped`. A placement the swap leaves
out (an ammo pickup) is skipped with a report line, not a map failure.
An AI weapon it leaves out is a named error, rather than an unarmed
variant; one it keeps is converted from Halo 1 with the roster
(`scenario::ai_weapons::retail_tags`, `roster::prepare`), as placements are.

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
to the store as `halo1_campaign_<level>_h2`. The manifest lists the
level once and its HUD pairs refer to the Halo 1-assets variant. `game::h1_variants::set_campaign_assets` flips the
`vfs::SandboxChoice` the campaign maps backend serves by
(`vfs::CampaignMapsBackend::with_assets`). With Halo 2 assets chosen and
no stored alternate, the level is served from its Halo 1 map and the first open says so once on stderr:
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
everywhere. Every retail tag the table names is in 05a except the fuel rod gun
(`flak_cannon`), which comes from a donor: `04b_floodlab`, else
`08a_deltacliffs` (`donor::MAPS`). Swapped on the ten levels: the assault rifle, pistol, sniper
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

Moved to [Halo 1 campaign: the ten levels](campaign-levels.md#9-the-ten-levels-on-a-pinned-base-built-tests-only).

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
   starts convert nothing and go straight on.
3. Sign in, **Campaign**, then **Halo: Combat Evolved**, the picker's first row (Halo 2 is second):
   the ten levels, a10 to d40, as far as the profile has got. Halo 1's
   multiplayer maps are `h1_<map>` in the multiplayer map list (Custom
   Game, Xbox Live), named as in Halo 1 (docs/halo1/multiplayer.md
   "Importing"). `--level a30` starts a converted level directly.

The start runs the `halo1` package's `convert` item
(`content/halo1/convert/halo1.convert.toml`, whose settings pick the
campaign, the Halo 2-assets variants, the multiplayer maps and the
sandboxes) through `h1_import::package::Halo1`; `h2 package prepare`
runs it alone ([studio-redesign.md](../editor/studio-redesign.md),
"Conversions"). The package's stamp,
`<data>/packages/stamps/halo1.toml`, sits beside the import stamp below,
and both are written after every map. `--no-halo1` or `H2_HALO1=0` (also
`false`, `no`, `off`) skips it. A failure never stops the game: a map
that fails is a line, `halo1: [5/36] campaign b40: failed: <why>`, and
the others are still there; a problem before any map (an unreadable
disc) is `halo1: not imported: <why>`. With no Halo 1 disc, or a bare
`default.xbe` as the game disc, and nothing converted before, the
`halo1` package is off, and `packages.list` and `h2 package prepare` say
why.

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
| each of a10 .. d40 | stored level `halo1_campaign_<level>_h1` and `campaign.toml` (`content::campaign_list::HALO1`) | the campaign picker and lists (`game::campaigns`), `vfs::CampaignMapsBackend` as `maps\<level>.map`, `--level` (`level_boot::resolve_in`) |
| each Halo 1 multiplayer map on the disc (`h1convert::convert::MAPS`), Halo 2 then Halo 1 sandbox | stored levels `h1_<map>_h2` and `h1_<map>_h1` and multiplayer list metadata | `game::new_maps`, the `multiplayer.halo1_sandbox` setting picks the variant |
| then each level's Halo 2-assets variant, job `<level>/h2` (§5) | stored level `halo1_campaign_<level>_h2` | `vfs::CampaignMapsBackend::with_assets` when `campaign.halo1_assets` is Halo 2 |

Campaign levels are written fresh from their base (§9); multiplayer maps
fresh from their default base (`--pinned` for either keeps the base).

### The stamp, and when it converts again

`<data>/campaigns/halo1/import.stamp` holds the Halo 1 disc's SHA-256,
size and modification time, the converter version, and each map's
outcome, written after every map (an interrupted import resumes). A start
converts what is missing: a map not in the stamp, or stamped done but
whose store marker is gone. Everything is converted again when:

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

A failed job leaves no playable map. If an older converter had written
that level, `run_jobs` (`h1_import::withdraw`) renames its store marker
(`<stem>.converted`) to the `<stem>.converting` ownership record, so the
level is no longer listed or booted and the next conversion still owns the
stem, and the line says so: `halo1: [5/36] campaign b40: the map an older
converter wrote is removed, so it is not offered`. A multiplayer map's other
sandbox stays; a map with neither sandbox left is not in the multiplayer
list. The stamp keeps the failure's reason.

**Tests.** `cli` `h1_import::tests`: the job list (campaign in play
order, then the multiplayer maps the disc has, both sandboxes, then each
level's Halo 2-assets variant); each job's store marker; the stamp
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
