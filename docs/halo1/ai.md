# Halo 1 AI data, and Halo 2's

`crates/tools/h1campaign` is step 5 of the campaigns work ([campaign.md](campaign.md)):
it reads Halo 1's AI data out of an Xbox cache file (through `h1map`),
decodes Halo 2's scenario AI blocks and `char` tags out of a v8 map, and
converts the first into the second as `map::build` blocks. Halo 2's AI then
runs Halo 1's encounters (decision 1: where the engines differ, Halo 2 wins).

The readers have been checked on every campaign map of both discs. The a30
ground placement and loadout checks below use the source maps and generated
scenario. Soldier movement and combat still need a live a30 observation.

Evidence labels, as in [map-format.md](map-format.md):

- **V (verified on disc):** checked by the `#[ignore]` tests in
  `crates/tools/h1campaign/tests/ai/disc.rs` on the owner's discs. Strides were read off
  the spacing of consecutive blocks (count × stride = gap to the next block)
  and then checked by decoding every element with range checks.
- **S (unverified):** a field name or enum order that our disc checks and
  engine reads do not establish. Offsets were read off the maps.
- **R (read on disc, meaning unproven):** the value is there and in range,
  but nothing has shown what the engine does with it.

No game data is in the crate, its tests or this page. The unit tests build
invented data; the disc tests read the discs at run time.

Tests: unit tests on invented data run with `cargo xtask test -p h1campaign`;
the disc tests are the ignored ones in `crates/tools/h1campaign/tests/`
(`ai/disc.rs`, `ai/convert.rs`, `scripts/`), run with `H1_ISO=disc/halo1.iso`
and `H2_DISC=disc/halo2.iso` set.

## Halo 1: the scenario's AI blocks

Offsets in the Halo 1 `scnr` struct; blocks are 12 bytes (count, address,
unused), tag references 16.

| Offset | Block | Stride | Evidence |
|---|---|---|---|
| `420` | actor palette: an `actv` reference each | `10` | V: every entry names an `actv` on all 10 levels |
| `42c` | encounters | `b0` | V |
| `438` | command lists | `60` | V |
| `444` | AI animation references | `3c` | V (stride) |
| `450`, `45c` | AI script, recording references | `28` | S |
| `468` | AI conversations | `74` | V |

Encounter (`b0`): name `char[32]` `00`, flags `u32` `20`, team `u16` `24`,
search behaviour `u16` `28`, manual BSP `i16` `2a`, compiled BSP `i16` `7e`, respawn delay `f32[2]`
`2c`; blocks: squads `80` (`e8`), platoons `8c` (`ac`), firing positions
`98` (`18`), player starting locations `a4` (`34`, the scenario's own
layout). V for the blocks; the scalars are R.

Squad (`e8`), V for everything the converter uses:

| Offset | Field |
|---|---|
| `00` | name, `char[32]` |
| `20`, `22` | actor palette index, platoon (`i16`) |
| `24`, `26` | initial, return state (`u16`) |
| `28` | flags (`u32`, R) |
| `2c` | unique leader type (`u16`, R) |
| `4e` | maneuver-to squad (`i16`, read by retail Halo 1 at `0x0013e191`) |
| `50` | delay, seconds (`f32`) |
| `54`..`6c` | firing position groups (`u32` masks, bit 0 = A): attacking, attacking search, attacking guard, defending, defending search, defending guard, pursuing (S order) |
| `7c`, `7e` | actors on normal, on legendary (`i16`) |
| `80` | major upgrade (`u16`) |
| `84`..`88` | respawn min, max, total (`i16`, R) |
| `8c` | respawn delay (`f32[2]`, R) |
| `c4` | move positions (`50`): position, facing `0c`, weight `10`, time `f32[2]` `14`, animation `i16` `1c` |
| `d0` | starting locations (`1c`): position, facing `0c`, sequence ID `i8` `12`, flags `u8` `13`, return and initial state `u16` `14`/`16`, actor `i16` `18` (`-1`: the squad's), command list `i16` `1a` |

Firing position (`18`): position, group `u16` `0c` (0 = A .. 25 = Z),
cluster `i16` `0e`, surface `u32` `14`. Command list (`60`): name, flags
`u32` `20`, manual BSP `i16` `2c`, resolved BSP `i16` `2e`, atoms `30` (`20`), points `3c` (`14`:
position, surface `u32` `0c`). Atom (`20`): type `u16` `00`, modifier
`i16` `02`, two `f32` parameters `04`, two point indices `0c`/`0e`,
animation `10`, script `12`, recording `14`, "command" `16` (for a loop
atom, an index into the list's own atoms), object name `18`. Atom types
0..27 in this order (S): pause, go to, go to and face, move in
direction, look, animation mode, crouch, shoot, grenade, vehicle, running
jump, targeted jump, script, animate, recording, action, vocalize,
targeting, initiative, wait, loop, die, move immediate, look random, look
player, look object, set radius, teleport. Conversation (`74`): name, flags
`u16` `20`, trigger distance `24`, run-to-player distance `28`,
participants `50` (`54`), lines `5c` (`7c`). The layout that feeds the
generated dialogue is below.

**Stale indices (V).** The H1 dispatcher at `0x16b250` reads operands by
atom kind. For example, go-to reads the first point, go-to-and-face reads
both, and a facing-relative timed move reads neither. Retail records retain
unrelated operands. Some go-to records also contain unusable targets
(a10's `moving_1_3` names point 5 in a set of five). The interpreter skips
that atom and can still run the next one.

### Spawn and control indices

**Built.** The converter preserves the effective actor, starting weapon and
command list of each authored location. The retail Xbox executables show
which absent indices actually suppress an action.

| Source field | Retail behaviour and v8 representation |
|---|---|
| Actor variant | H1 loads the squad default and replaces it only when the start's index differs from -1 (`0x13e390..0x13e3ae`). A negative or out-of-range effective index or a null palette datum skips creation (`0x13e3b0..0x13e3dd`). H2 makes the same checks at `0x2037bf..0x2037fa`. When an explicit empty override shares a squad with valid inherited starts, the converter puts the default on those siblings and leaves both character fields empty for the failed spawn. |
| Weapon | H1 tests the effective variant's weapon datum at `0x14d6ee`. A null datum skips weapon creation at `0x14d70b`. A mixed squad stores its weapons on individual starts so an explicitly unarmed variant cannot inherit another variant's weapon. An armed source requires a resolved destination weapon palette slot. |
| Starting command list | H1 forwards start `+0x1a` at `0x14e51b..0x14e540`, stores it at actor `+0x90` at `0x14cd76`, and bypasses the active-list path on -1 at `0x167c6a`. Scripted v8 maps bind every real list to its generated script. Unrepresentable indices and missing source lists are conversion errors. |
| Conversation cast scope | H1 reads participant `+0x44` at `0x143d69` and passes every non-null selector to the actor iterator at `0x13eda0`. The display text at `+0x24` does not participate. Casting uses the converted numeric squad or group even when the text is empty or duplicated. Of 327 retail roles, 35 specify an encounter and 292 leave this scope open. |
| Generic voice bank | H1 visits unit `+0x2b4`, takes up to 16 selectors below 100 and leaves the initial voice unchanged if none qualify (`0x46716..0x4675a`). In that case the converted unit has no generic bank. |
| Squad and platoon | H1 decodes the encounter from the low word and the local squad or platoon from the next byte (`0x13eeee`, `0x13ef34..0x13ef63`). Campaign roster selection records those source ordinals through pruning and generated starts. Ordinary AI script literals and counted spawn controls bind those ordinals. Duplicate display names remain independently addressable. Counted spawn selects the first squad of a platoon and returns without spawning for a bare encounter or an empty platoon (`0x1422ec..0x14236c`). A required squad missing from the converted roster is an error. A squad with no matching platoon is outside that platoon's selection (`0x13e50f..0x13e522`). |
| Maneuver and trigger targets | A negative or out-of-range maneuver target bypasses migration (`0x13e191..0x13e1be`). A trigger subject outside its platoon table uses the entire encounter's counts and vitality (`0x13af44..0x13af86`). Native order index overflow is a conversion error. |

A read-only census of the 23 playable Xbox maps found 10,129 authored
starts. Of these, 8,206 inherit the squad actor, 112 have no effective
actor, and 1,614 specify a starting command list. The ignored
`retail_spawn_bindings_keep_every_actor_weapon_and_command` check compares
those bindings with the decoded v8 scenario for every location. The source
script tables also contain 4,695 literal AI operands: 2,041 encounters,
235 platoons and 2,419 squads. Each names an existing source row.

The ignored disc tests that check this section:

- `retail_spawn_bindings_keep_every_actor_weapon_and_command`
  (`tests/ai/disc.rs`);
- `retail_overlay_defaults_round_trip_on_all_23_maps`
  (`h1-assets`, `models/tests.rs`);
- `halo2_sandbox_places_a_vehicle_without_a_counterpart` (`h1objects`,
  `tests/sandbox_disc.rs`);
- `encounter_controls_emit_live_evaluators_and_native_orders`
  (`tests/scripts/disc.rs`);
- `every_campaign_conversation_compiles_with_its_source_sounds`
  (`tests/scripts/conversations_disc.rs`);
- `both_sandboxes_rebuild_every_retail_map` (`tests/disc/no_fallback.rs`).

### `actr` and `actv`

`actr` is `0x4f8` bytes; the converter reads flags `u32` `00`, more flags
`u32` `04` and the type `u16` at `14` (V: matches every tag's path on all
10 levels). Types (S): 0 elite, 1 jackal, 2 grunt, 3 hunter, 4 engineer, 5
assassin, 6 player, 7 marine, 8 crew, 9 combat form, 10 infection form, 11
carrier form, 12 monitor, 13 sentinel, 14 none, 15 mounted weapon.

`actv` is `0x238` bytes plus its change-colour block (`0x20` each, at
`22c`). Read: flags `00`, actor `actr` `04`, unit `14` (V: a `bipd` or
`vehi`), major variant `actv` `24`, weapon `64` (V: a `weap` when set).

The rest of `actv` is tuning a faithful conversion would carry into a
`char` instead of picking a retail one. Fields with a Halo 2 home (names S;
offsets not read):

| Halo 1 `actv` | Halo 2 `char` block |
|---|---|
| unit | the character's unit |
| major variant | major character |
| movement switching, crouch chance and time, run time | movement properties |
| weapon, maximum firing distance, rate of fire, projectile error, burst geometry, desired combat range, target tracking and leading, damage modifier | weapons properties (and firing patterns) |
| grenade type, stimulus, count, throw velocity | grenades properties |
| body and shield vitality, shield recharge | vitality properties |
| drop weapon and grenade chances, equipment | the unit's death drops |
| change colours | the unit's own variant colours |

`actr` fields with a Halo 2 home: perception (vision and hearing ranges,
angles: perception properties), the danger and cover rules (evasion, cover,
retreat properties), swarm and leader behaviour (swarm, engage properties).
Halo 1's actor-type-wide flags (`can see in darkness`, `sneak`, ...) have no
single counterpart.

### Census (V)

Every campaign level (`h1_campaign_ai_reads_on_every_level`): palette
entries, distinct actors, encounters, squads, squad starting locations,
move positions, platoons, firing positions, command lists, atoms, points,
conversations.

| Level | pal | actr | enc | squads | starts | move | plat | fp | cl | atoms | pts | conv |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| a10 | 18 | 7 | 39 | 144 | 296 | 10 | 41 | 2381 | 124 | 558 | 270 | 20 |
| a30 | 10 | 7 | 22 | 179 | 390 | 3 | 72 | 4645 | 3 | 8 | 6 | 59 |
| a50 | 34 | 13 | 33 | 530 | 1180 | 337 | 77 | 4976 | 54 | 203 | 104 | 44 |
| b30 | 10 | 8 | 32 | 115 | 280 | 143 | 39 | 4470 | 7 | 16 | 8 | 19 |
| b40 | 19 | 11 | 75 | 985 | 1103 | 591 | 84 | 20789 | 32 | 127 | 93 | 25 |
| c10 | 34 | 11 | 39 | 255 | 2938 | 512 | 22 | 2293 | 76 | 358 | 215 | 5 |
| c20 | 46 | 7 | 81 | 169 | 1096 | 0 | 25 | 1560 | 98 | 542 | 328 | 0 |
| c40 | 39 | 15 | 67 | 247 | 836 | 38 | 56 | 2746 | 56 | 174 | 97 | 5 |
| d20 | 64 | 12 | 62 | 172 | 868 | 13 | 36 | 1554 | 33 | 102 | 43 | 0 |
| d40 | 35 | 10 | 56 | 192 | 1142 | 2 | 31 | 1758 | 69 | 237 | 145 | 0 |

Sanity checks on every level: every index above in range (bar the stale
ones listed), every position finite and inside a structure BSP's world
bounds, except firing positions of flying actors (a30's banshees: 34 up to
90 m out over the sea) and a few starts and points just over an edge (a50:
15 firing positions, 6 starts and 1 point within 13 m; b40: 1 point;
c10: 8 starts within 16 m); facings within
±7 rad; counts non-negative. Actor types across all palettes: combat form
129, elite 55, grunt 42, marine 27, jackal 13, crew 8, hunter 8, infection
form 8, sentinel 8, carrier form 6, monitor 4, engineer 1.

## Halo 2: the scenario's AI blocks and `char`

Offsets in the v8 `scnr` struct; blocks are 8 bytes (count, address), tag
references 8 (group, datum). Strides V (spacing, then every element
decoded on all 15 campaign levels with `check_indices` clean).

| Offset | Block | Stride |
|---|---|---|
| `158` | squad groups | `24` |
| `160` | squads | `74` |
| `168` | zones | `38` |
| `170` | mission scenes (not read) | |
| `178` | character palette: a `char` reference each | `8` |
| `1a0` | Legacy conversation field, unused by the converted scripts | |
| `1d8` | scripting data, one element: point sets at `00` | `80` |
| `240` | orders | `7c` |

These fit the field order of the scenario definition, anchored by the
structure BSP block at `210` ([pathfinding.md](../formats/pathfinding.md)) and the
placement blocks in [scenario-editor.md](../editor/scenario-editor.md).

Squad group (`24`): name `char[32]`, parent `i16` `20`, initial order
`i16` `22`.

Squad (`74`): name `char[32]` `00`, flags `u32` `20`, team `u16` `24`,
squad group `i16` `26`, delay `f32` `28`, normal and legendary counts `i16`
`2c`/`2e`, major upgrade `u16` `30`, vehicle `i16` `34`, character `i16`
`36`, initial zone `i16` `38`, initial weapon and secondary weapon `i16`
`3c`/`3e`, grenade type `u16` `40`, initial order `i16` `42`, vehicle
variant string ID `44`, starting locations `48`, placement script
`char[32]` `50`, `i16` `70` (`-1` on retail; R).

Starting location (`64`): name string ID `00`, position `04`, reference
frame `i16` `10`, facing yaw and pitch `f32` `14`, flags `u32` `1c`,
character `20`, initial weapon `22`, secondary `24`, vehicle `28` (`i16`),
seat type `u16` `2a`, grenade type `u16` `2c`, swarm count `i16` `2e`,
actor variant string ID `30`, vehicle variant `34`, initial movement
distance `f32` `38`, emitter vehicle `i16` `3c`, movement mode `u16` `3e`,
placement script name `char[32]` `40` and compiled script index `i16` `60` (absent = `-1`). On 03a, 57 locations name a kind-5 script whose index matches this field.

Zone (`38`): name, flags `u32` `20` (1 on 118 of 119 retail zones), manual
BSP `i16` `24`, firing positions `28` (`20`), areas `30` (`88`).

Firing position (`20`): position, reference frame `i16` `0c`, flags `u16`
`0e` (R: 21 distinct values; most common `0x42`, `0x41`, `0`), area `i16`
`10`, cluster `i16` `12`, pathfinding sector `u32` `14` (all ones when unassigned), normal yaw and pitch `f32`
`18` (pitch π/2 is straight up).

Area (`88`): name, flags `u32` `20`, then fields the tools compute: mean of
the area's firing positions `24`, its reference frame `i16` `30`, their
deviation `f32` `34`, first firing position `i16` `38` and count `i16` `3a`
(V: every area's run tiles the zone's firing positions, which are grouped by
area; an empty area is `-1+0`), cluster bit mask `3c`..`7c` (exclusive end),
manual reference frame `i16` `7c`, flight hints block `80`.

Order (`7c`): name, style `i16` `20` (an index into the style palette at
`150`, `-1` none), flags `u32` `24`, force combat status `u16` `28`, entry
script `char[32]` `2c`, entry script index `i16` `4c`, follow squad
`i16` `4e`, follow radius `f32` `50`,
primary area set `54` and secondary `5c` (`8` each: area type `u16` `00`,
zone `i16` `04`, area `i16` `06`; V by range check), secondary set trigger
`64`, special movement `6c`, order endings `74` (`2c`; counted only).

Point set (`30`, in scripting data): name, points `20` (`3c`: name, position
`20`, reference frame `i16` `2c`, surface `u32` `30`, facing `f32[2]` `34`),
BSP `i16` `28`, manual reference frame `i16` `2a`, flags `u32` `2c`.

`char` (root `ec`): flags `u32` `00`, references (8 bytes each) parent
`04`, unit `0c`, creature `14`, style `1c`, major character `24`; blocks
from `2c`: variants, then general properties at `34` (`c`: flags `u32`,
type `u16` at `04`). V: the type is Halo 1's actor type numbering,
extended (16 brute, 17 prophet, 18 bugger, 19 juggernaut); a tag without
general properties inherits its parent's. Some retail tags' blocks point
outside their own payload (`ai\generic`, 01b's marine, Miranda, Cortana);
the decoder reads those as "not set here".

### Census (V)

`h2_retail_census`: squad groups, squads, starting locations, zones,
firing positions, areas, orders, point sets, palette characters.

| Level | grp | squads | starts | zones | fp | areas | orders | point sets | char |
|---|---|---|---|---|---|---|---|---|---|
| 00a_introduction | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 01a_tutorial | 0 | 6 | 27 | 2 | 120 | 13 | 4 | 2 | 5 |
| 01b_spacestation | 60 | 117 | 297 | 11 | 3677 | 445 | 103 | 13 | 20 |
| 03a_oldmombasa | 48 | 132 | 376 | 14 | 3267 | 389 | 184 | 40 | 15 |
| 03b_newmombasa | 41 | 93 | 234 | 11 | 2414 | 232 | 132 | 31 | 15 |
| 04a_gasgiant | 36 | 146 | 352 | 57 | 4206 | 528 | 127 | 13 | 6 |
| 04b_floodlab | 38 | 149 | 730 | 23 | 4895 | 358 | 100 | 10 | 13 |
| 05a_deltaapproach | 46 | 197 | 620 | 15 | 4531 | 545 | 195 | 13 | 13 |
| 05b_deltatowers | 36 | 157 | 554 | 24 | 4479 | 592 | 202 | 8 | 13 |
| 06a_sentinelwalls | 27 | 113 | 557 | 27 | 6104 | 350 | 103 | 14 | 20 |
| 06b_floodzone | 81 | 170 | 560 | 23 | 4894 | 451 | 176 | 30 | 12 |
| 07a_highcharity | 52 | 252 | 752 | 20 | 6892 | 546 | 211 | 22 | 28 |
| 07b_forerunnership | 33 | 78 | 291 | 25 | 2108 | 276 | 78 | 13 | 18 |
| 08a_deltacliffs | 38 | 85 | 226 | 11 | 1741 | 194 | 126 | 23 | 12 |
| 08b_deltacontrol | 57 | 112 | 273 | 18 | 2325 | 264 | 163 | 32 | 20 |

Area set types: 0 on 7865 entries, 1 on 2457, 2 on 1280. Order styles: `-1`
175, 0 828, and 1..8 the rest. 07a's palette has one null entry.

## The conversion

Moved to [Halo 1 AI: the conversion](ai-conversion.md#the-conversion).

## Involuntary speech and body damage

**Built.** Halo 1's active Flood revival passes involuntary speech type 5 at
`0x4508c` after the revival timer expires with positive body vitality.
That type selects `udlg+0xb80` at `0x46c84`. Halo 2's reanimation
path checks for a depleted parent at `0x28fedc` and calls `0xcaf00`
through `0x1e0dc0` to clear depletion and restore the unit. Converted
units with that authored slot queue it on this transition.

**Built.** The Xbox retail dialogue scheduler never selects slots `0xa90` (partial
sighting), `0xaa0` (nothing there), or `0xb60` (uncover exclamation).
They appear in neither the primary or reply tables nor the hard-coded
involuntary selectors, and no entry in the fallback table selects them.
The converted bank preserves the original sounds and indices for explicit
source-slot lookup.

**Built.** The airborne scream at `udlg+0xb0` is separate from damage
on landing. The Halo 1 biped update at `0x49280` checks more than one
second airborne, then checks for ground within six units and estimates
vertical speed on impact. The vehicle rider branch at `0x4b580`
requires more than four seconds in an eligible seat, more than one
second airborne, and no floor below or in the projected path. Both
source branches gate probes at half-second intervals and suppress them
during custom animation. The biped branch also respects the converted
command list's falling-damage exemption. The boot hook uses Halo 2's
biped and vehicle airborne counters, the animation manager predicate at
`0x11b930`, and the BSP ray at `0x1697c0`. It queues the original
scream in the speaker's speech cache.

The Xbox retail XBE has a scheduler row for stimulus 46 at
`0x1ef9f8` that also selects `0xb0`. That row is dormant in this
executable. The scheduler at `0x145f30` has 43 direct call sites. Only
the command-list update and the combat-aftermath update pass a
preselected event ID. The
command-list update at `0x16c66c` forwards the event at actor `+0xfc`;
the action switch at `0x16ba93` only writes events 38, 39, 41, 42, 44
or the no-event sentinel to that field. The aftermath switch feeding
`0x16d20e` only supplies events 48 through 56. The retail
`ai_debug_vocalize` evaluator at `0xec990` returns void without
dispatching an event. Explicit source-slot requests still resolve
`0xb0` through the unit's converted voice bank.

**Built.** Halo 1's collision damage accumulates applied body damage in
a byte for each collision region at `0x9512f`. It destroys that region
after crossing the source `coll +0x28` threshold. The live unit
notification at `0x3dd10` selects `0xd0` or `0xe0` by collision
region flag `0x200`. The converted `hlmt` carries these source
thresholds and flags. The Halo 2 body-damage callback reads the hit
region that `0xd7e1d` writes and the applied normalized damage that
`0xd8e5e` writes. Its region history is per salted object handle.

## Spawning

Halo 1 creates an encounter at load unless its flag bit 0 ("not
initially created") is set; on the ten levels almost all are set (a30:
21 of 22, b40: 74 of 75), so Halo 1's scripts place them. Halo 2 has no
such default: no squad on any retail campaign map sets a placement flag
(survey `squad_flag_bits`: only bits 8, 9 and 13, on a few squads), and
retail scripts place every squad with `ai_place`. So `h1campaign` adds a
helper, `h1_initial_encounters`, called first by the earliest authored
startup script (or registered as startup when there is none). It places the group of every
encounter with bit 0 clear (a30: `mid_cliff_1`), and Halo 1's own
`ai_place` calls keep one argument, as most retail calls do
(scripts.md). Squad flags stay 0.

Zone firing positions and areas are written as shared arrays keyed by
their bytes: Halo 1 encounters repeat firing positions (b40's 20,789 hold
8,335 distinct; survey `h1_ai_sizes`), and the builder places identical
arrays once. On b40 the scenario's tree is `0x14838f` bytes and lays
out in `0xfe618`.

## What only a game run can prove

- That Halo 2 places each wave, moves actors between its firing positions,
  takes cover, targets the player and uses the selected weapons and grenades.
  The agent method `world.ai` reports squad, cached sector, behaviour, focus
  object and combat status. Match its unit ID to positions across ticks in
  `world.object`. [Agent API](../runtime/agent-api-world.md#worldai)
- That the chosen characters, pulled into a Halo 1 level from other maps,
  load with their units, weapons and animations.
- How squads behave under orders with no endings, and how closely the generated command scripts reproduce H1 movement and timing.
- Halo 1 behaviour carried by the dropped data: move positions,
  respawning and maneuvers into the empty squads.
- Conversation audibility, speaker positioning and transitions in play.

## Open questions

- The firing position flag bits (`0x1`, `0x2`, `0x4`, `0x8`, `0x40`, ...)
  and area set types 0..2: values from the census, meanings unknown.
- What contributes additional clusters to retail area masks. In 05a the masks
  include all 666 distinct area/position-cluster pairs and 1,528 additional
  bits. The conversion writes only the clusters evidenced by its positions.
- The two trailing `i16` fields that are `-1` on retail (squad `70`
  and starting location `60`). The order fields were identified by
  disassembly in [ai-control.md](ai-control.md).
- Halo 1 squads' other six group masks (defending, guarding, pursuing):
  Halo 2 expresses them as order endings and secondary area sets. The
  scripted defend order uses the defending mask. The search, guard and
  pursue masks remain unused.
- Vehicle actor variants (`warthog gunner`, `banshee` pilots) become plain
  characters; Halo 2 puts them in vehicles through the starting location's
  vehicle and seat fields, which need the vehicle palette.
