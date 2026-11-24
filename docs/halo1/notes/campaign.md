# Halo 1 campaign conversion: measurements

Evidence for [../campaign.md](../campaign.md). The measurements below were
recorded in that page and moved here; the commit each was taken at was not
recorded. Reproduce by running the ignored disc test named in each entry
with `H1_ISO=disc/halo1.iso` and `H2_DISC=disc/halo2.iso` set
(`H1_LEVELS=a30,b40` selects levels).

## 2026-11-18: both asset variants of every level (converter version 55)

Test: `h1campaign` `tests/assets.rs`, 260 s for all twenty variants. Every
retail tag the swap table names was already in 05a (no donor needed).
Swapped on the ten levels: the assault rifle, pistol, sniper rifle, rocket
launcher, shotgun, needler, plasma pistol and rifle, both grenades, the
warthog, ghost, scorpion, banshee, wraith and shade; ammo pickups dropped
(26 palette entries). Both variants of every level build from scratch with
0 dangling references and every script tag atom resolving; the Halo 2
variant is a little smaller everywhere.

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

## 2026-11-17: pinned, pruned and from scratch (converter version 28)

Test: `h1campaign` `tests/fresh.rs`, `every_level_converts_fresh` (the
gestalt cut in). Pinned is the pinned base with the lean ladder; pruned is
version 23's fresh build. The tag memory bound was `0xc24000` and the file
bound `0x11800000` (293,601,280 bytes) for these runs: every level had 3.6
to 4.9 MB of tag memory to spare and the lean ladder fired nowhere. a30's
from-scratch tag memory (`0x85c000`) against pruned (`0x8e5000`) shows the
dense index, the cut gestalt and equal blocks placed once; scoping the
sharing to each tag cost about 1 MB a level.

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

a30's gestalt cut (same run): 3,270 sounds kept, 18,053 chunks to 14,344,
3,830 raw resources (108.9 MB, 05a's own dialogue and music) dropped.

## 2026-11-17: per-level conversion counts (round 3)

Test: `h1campaign` `tests/disc.rs`, `every_campaign_level_converts`, all on
05a, on the owner's discs. Placed / Halo 1 placements; BSPs with Halo 1's
BSP switch volumes in brackets; the object sweep clean and every BSP
checked on every level. The bound is `0xc24000`; the base alone is
`0xaed000`. Donors: the hunter on a50, b30, b40, c40, d20, d40; flood
combat forms, carrier, infection and the monitor on c10, c20, c40, d20,
d40 (by what each level's AI uses).

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

Hooks left `none` after the level data, seat table and kept squads
(`h1scripts` round 2; the table's columns are from round 3, before them):
a10 262, a30 152, a50 76, b30 153, b40 274 (converted without them: at the
bound), c10 65, c20 46, c40 87, d20 64, d40 194; 0 unrunnable on every
level.

## 2026-11-17: stubs that scripts wait on

Test: `h1campaign` `tests/scripts/disc.rs`, `waits_on_stubs`, over all ten
levels, before the stub behaviours in [../campaign.md](../campaign.md)
"Waits on stubs (playing a level through)" were mapped.

| Stub | Uses |
|---|---|
| `ai_status` | 35 (a50 16, b40 17, b30 2) |
| `ai_conversation_status` | 34 |
| `ai_command_list_status` | 20 |
| `game_all_quiet` | 17 |
| `ai_living_fraction` | 14 (a30) |
| `device_get_power` | 8 (d40) |
| `player0_joystick_set_is_normal` | 7 |
| `game_time` | 6 (a30) |
| `game_safe_to_speak` | 5 |
| `ai_going_to_vehicle` | 5 |
| `sound_impulse_time` | 4 (a10) |
| `hud_get_timer_ticks` | 3 (d40) |
| `ai_conversation_line` | 1 (b30) |

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

## 2026-11-19: pinned base sizes with cutscene data

Commit: `a86f3d06a` (the first commit with this table in the page).
Graphs put their animation data in the map file (raw, measured with the
retail 280 MiB slot), recordings their streams in tag memory, inside the
scenario. These measurements predate keeping recordings on every lean
step. The Without column uses `Options::cutscene_data` off. Pinned b40
(tag memory, `0xc23000`) and pinned d40 (the file, `0x117b9800` bytes,
with no music, ambience or cutscene graphs) placed no devices
(`Devices::None`). a10's jump (`0x7f000`) was its scenario no longer
fitting the base scenario's old range and being appended (`0xb512c`
bytes). d40 lacked `0x1baa00` bytes of animation data; b40's cutscene
graphs needed `0x92000` plus its level data.

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

## 2026-11-17: first import on the owner's discs

Command: `h2 play` with `halo1.iso` beside the Halo 2 disc, release build.
36 maps (ten levels, 13 multiplayer maps in two sandboxes) converted in
about 90 s, the disc's hash included. The ten Halo 2-assets variants were
added after (converter version 55).
