# Campaigns: measurements

Evidence for [../campaign.md](../campaign.md).

All numbers come from the ignored tests named in the entries, run against
the owner's discs. Tag memory is the map's metadata size; the file bound
and the tag memory bound are the ones the page names.

## 2026-11-20: placements and script-sound counts

Commits: `c55daf023` (placements), `0cc737264` (script sounds). Method:
the `h1_placement_flags` survey and the serialized reference tables
`both_sandboxes_rebuild_every_retail_map` checks, read-only walks of the
retail discs.

- Of 11,624 retail Halo 1 placements, four scenery rows have palette
  index -1, one each on a10, a30, b40 and c40.
- The ten campaign script reference tables hold 570 distinct map/sound
  pairs, including every authored sound atom; the busiest level is a10
  with 175. The thirteen multiplayer scenarios have no authored script
  sounds.

## 2026-11-19: locale raw extents

Commit: `f474ccc5b`. Method: a saved a10 converted with the generic raw
sizing walk.

An empty locale (zero rows, zero text bytes, nonzero file offsets) sized
by the walk reads `0x464be00` bytes; sized from the descriptor it reads
none. The synthetic test is
`matg_locales_use_descriptor_lengths_across_a_large_raw_gap`.

## 2026-11-19: recordings and cinematic command lists

Commits: `8e112edc2` (recordings), `83774dff5` (command lists). Method:
the recordings disc test and the `cutscene_survey` walk.

- The ten levels hold 349 version-4 recording streams, using unit
  control versions 0, 3 and 4; reading the original encoding directly
  loses event framing on 189 of them; 73 timed animation-state changes
  keep their original tick.
- On the source disc c10 has ten by-unit command-list calls and c40
  twenty.

## 2026-11-18: both asset variants of every level

Commit: `4b7fd6f5d` (converter version 55). Method: `H1_ISO=disc/halo1.iso
H2_DISC=disc/halo2.iso cargo test --release -p h1campaign --test assets --
--ignored --nocapture` (`every_level_builds_in_both_assets`, 260 s for all
twenty builds; `H1_LEVELS=a30,b40` for some).

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
little smaller everywhere. Every retail tag the swap table names was in
05a (no donor was needed). Swapped on the ten levels: the assault rifle,
pistol, sniper rifle, rocket launcher, shotgun, needler, plasma pistol and
rifle, both grenades, the warthog, ghost, scorpion, banshee, wraith and
shade; ammo pickups dropped (26 palette entries).

The same run is the b40 figure the cutscene section cites: b40 with Halo 1
assets uses `0x966000` of `0xc24000` bytes of tag memory.

## 2026-11-18: a10's opening, call by call

Commits: `d17d30c03` (ticks), `3f13910f0` (the whirr, converter version 67).
Method: `cinematic_calls_keep_halo1_s_ticks` (h1scripts, disc test against
Halo 2's signatures and retail literals).

`x10` calls `autumn_glory_1`, `autumn_glory_2` (space, `a10_space`),
`bridge` (BSP 1), then `hangar_1`, `hangar_2` and `cryo` (`x10hangar`, the
cryo bay). `fade_in`/`fade_out` take `(real real real short)` and
`camera_set` `(cutscene_camera_point short)`; retail Halo 2's own literals
are 15, 30, 60 and 500 ticks. No fade or camera call is mistranslated.

The cinematic's "big whirring sound" was a10's continuous
`dialog_flavor_captain` restarting each tick (Halo 2's `sleep` ignores a
negative duration); `sleep_forever` fixes it
([scripts.md](../scripts.md), "Sleeping forever"). The reported flicker has
no cause in the script translation.

## 2026-11-18: levels built from scratch (converter version 28)

Commit: `c67e79ed1`. Method: `every_level_converts_fresh`
(`h1campaign/tests/fresh.rs`), the gestalt cut; "pinned" is the pinned
build with the lean ladder, "pruned" the earlier fresh build.

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

The tag memory bound is `0xc24000`: every level has 3.6 to 4.9 MB to
spare, and the lean ladder fires nowhere (b40 keeps its level data, seat
table and object textures). The file bound was `0x11800000`
(293,601,280 bytes). On every level the map is built from scratch with one
index slot per tag, and no tag from `scenarios\solo\05a_deltaapproach\` or
`scenarios\multi\`. The reference sweep counts only problems the pinned map
of the same level does not have (none on any level).

Other figures from that run: a30 from scratch takes `0x85c000` of tag
memory against `0x8e5000` pruned. Scoping the sharing of equal blocks to
each tag had cost about 1 MB a level. On a30 the gestalt cut keeps 3,270
sounds, takes 18,053 chunks to 14,344 and drops 3,830 raw resources
(108.9 MB, 05a's own dialogue and music). Retail shares arrays across tags
in 25 to 28 groups per map (05a: 377,044 bytes of `effe` arrays and
245,966 of `shad` lie in other tags). Every dangling-reference count
recorded on retail data was 0 (this table, Blood Gulch in
[multiplayer.md](../multiplayer.md), c20 in the converter version 100
integration run). In that run every one of the 20 campaign variants failed
from scratch with `globals\globals.matg (3072 nonzero bytes in 1 ranges no
array covers)`, the damage-table arrays the Halo 1 cyborg scales replace;
`every_level_builds_in_both_assets` then passed on all 20 and the
multiplayer from-scratch tests on all 26. d40 written pinned fit only
under the retail campaign slot (`0x11800000`); from scratch it is
258,213,376 bytes and its old pruned build was `0x19494400`.

## 2026-11-18: the b40 tag memory accounting

Commit: `dfbce523f`. Method: `base_memory_and_vehicles`
(`h1campaign/tests/survey.rs`), the per-group and appended-tag
accounting in `Report`.

b40 went from `0xc21000` (3 of 13 BSPs) to `0xc23000` (13 of 13, the
hunter included), then to `0xc24000`, the bound exactly, with Halo 1's
textures on its structure (10 to 25 shader copies a BSP). Pinned b40 at
`0xc21000` and a50, c10 (`0xbe7000`) appended their scenario. The old
scenario bytes left as unowned metadata were `0x10b8ec`; the arrays other
tags keep inside its old range were `0x32` bytes on 05a; the rebuilt sound
gestalt's hole was `0xb4b80` bytes. a10's jump of `0x7f000` was its
scenario no longer fitting the base scenario's old range (appended,
`0xb512c` bytes). The pinned d40 deficit was `0x1baa00` bytes of animation
data, and pinned b40 needed `0x92000` for its level data; pinned b40
(`0xc23000`) and pinned d40 (`0x117b9800` bytes, with no music, ambience
or cutscene graphs) placed no devices.

## 2026-11-17: pinned sizes with and without cutscene data

Commit: `94aa80832`. Method: `every_level_converts` on the pinned base
with the retail 280 MiB slot, before the lean ladder kept recordings on
every step; the Without column is `Options::cutscene_data` off. Graphs
put their animation data in the map file, recordings their streams in tag
memory, inside the scenario.

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

## 2026-11-17: waits on stubbed builtins

Commit: `9c6724f30`. Method: `waits_on_stubs` (h1scripts disc test) over
all ten levels, before the round that mapped most of these stubs.

| Stub | Uses | Behaviour then |
|---|---|---|
| `ai_status` | 35 (a50 16, b40 17, b30 2) | `ai_combat_status` on Halo 1's scale |
| `ai_conversation_status` | 34 | live status from the generated dialogue worker |
| `ai_command_list_status` | 20 | done value 2 |
| `game_all_quiet` | 17 | retail Halo 2 threat query (587) |
| `ai_living_fraction` | 14 (a30) | `ai_strength` |
| `device_get_power` | 8 (d40) | Halo 2's unnamed builtin 255 |
| `player0_joystick_set_is_normal` | 7 | active local look-stick layout (722) |
| `game_time` | 6 (a30) | current simulation tick (888) |
| `game_safe_to_speak` | 5 | retail Halo 2 dialogue safety query (586) |
| `ai_going_to_vehicle` | 5 | `0` (none is going) |
| `sound_impulse_time` | 4 (a10) | `sound_impulse_language_time` |
| `hud_get_timer_ticks` | 3 (d40) | the native countdown |
| `ai_conversation_line` | 1 (b30) | the current line, or 999 when inactive |

Waits still on an unhandled stub, by level:

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

Hooks left `none` after the level data, seat table and kept squads (the
`h1scripts` round before round 3): a10 262, a30 152, a50 76, b30 153, b40
274 (converted without them: at the bound), c10 65, c20 46, c40 87, d20
64, d40 194; 0 unrunnable on every level.

## 2026-11-17: the ten levels on their base (round 3)

Commit: `b6a74cb4e`. Method: `every_campaign_level_converts`
(`h1campaign/tests/disc.rs`), all on 05a. Placed / Halo 1 placements; BSPs
with Halo 1's BSP switch volumes in brackets; the sweep clean and every BSP
checked on every level. Copied from donors: the hunter on a50, b30, b40,
c40, d20, d40; flood combat forms, carrier, infection and the monitor on
c10, c20, c40, d20, d40 (by what each level's AI uses).

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

The bound is `0xc24000`; the base alone is `0xaed000`. The check covers 69
BSPs in all (a10's 9 and b40's 13 included).

## 2026-11-17: the import's wall time

Commit: `9a3d7864d`. Method: `h2 play` on a release build, first start,
the Halo 1 disc's hash included.

36 maps (ten levels, 13 multiplayer maps in two sandboxes) import in about
90 s.

## 2026-11-17: a10's hidden placements and cutscene graphs

Commits: `45f326821` (hidden placements), `94aa80832` (graphs, dialogue
calls). Method: the `h1_placement_flags` survey and the `cutscene_survey`
walk, read-only.

- A10 hides 67 of 107 bipeds, 51 of 56 vehicles and 92 of 274 scenery
  from the automatic placement pass ("not placed automatically").
- Graphs a cutscene names: 34 nodes for the captain, 41 for Cortana, 1 to
  26 for props; 9 graphs on a10, 2 to 9 elsewhere.
- 493 `sound_impulse_time` calls.
