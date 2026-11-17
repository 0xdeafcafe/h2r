# Halo 1 AI data, and Halo 2's

`crates/h1ai` is step 5 of the campaigns work ([campaigns.md](../game/campaigns.md)):
it reads Halo 1's AI data out of an Xbox cache file (through `h1map`),
decodes Halo 2's scenario AI blocks and `char` tags out of a v8 map, and
converts the first into the second as `map::build` blocks. Halo 2's AI then
runs Halo 1's encounters (decision 1: where the engines differ, Halo 2 wins).

Status (2026-11-17): reader and decoders checked on every campaign level of
both discs; the converter's output for a30 and a10 builds, reads back
unchanged, and sits on the pathfinding generated from the converted
collision. Not yet loaded by the game: what only a game run can prove is
listed at the end.

Evidence labels, as in [h1-map-format.md](h1-map-format.md):

- **V (verified on disc):** checked by the `#[ignore]` tests in
  `crates/h1ai/tests/disc.rs` on the owner's discs. Strides were read off
  the spacing of consecutive blocks (count × stride = gap to the next block)
  and then checked by decoding every element with range checks.
- **S (from source):** field names and enum orders as Guerilla and c20
  (<https://c20.reclaimers.net/h1/tags/scenario/>,
  <https://c20.reclaimers.net/h1/tags/actor/>) list them, and Assembly's
  Halo 2 plugins name them. Offsets were read off the maps, never copied.
- **R (read on disc, meaning unproven):** the value is there and in range,
  but nothing has shown what the engine does with it.

No game data is in the crate, its tests or this page. The unit tests build
invented data; the disc tests read the discs at run time.

```
cargo test -p h1ai                                         # invented data
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test -p h1ai --release --test disc -- --ignored --nocapture
```

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
search behavior `u16` `28`, manual BSP `i16` `2a`, respawn delay `f32[2]`
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
| `4c` | maneuver-to squad (`i16`) |
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
`u32` `20`, manual BSP `i16` `2c`, atoms `30` (`20`), points `3c` (`14`:
position, surface `u32` `0c`). Atom (`20`): type `u16` `00`, modifier
`i16` `02`, two `f32` parameters `04`, two point indices `0c`/`0e`,
animation `10`, script `12`, recording `14`, "command" `16` (for a loop
atom, an index into the list's own atoms), object name `18`. Atom types
0..27 in Guerilla's order (S): pause, go to, go to and face, move in
direction, look, animation mode, crouch, shoot, grenade, vehicle, running
jump, targeted jump, script, animate, recording, action, vocalize,
targeting, initiative, wait, loop, die, move immediate, look random, look
player, look object, set radius, teleport. Conversation (`74`): name, flags
`u16` `20`, trigger distance `24`, run-to-player distance `28`,
participants `50` (`54`, encounter name `char[32]` at `24`), lines `5c`
(`7c`, participant `i16` at `02`).

**Stale indices (V):** fields an atom type doesn't read keep old values
(a `look` atom's points, a `pause`'s animation), so only the fields an atom
reads are checked: the first point of go to / move immediate / teleport,
both of go to and face, a loop's atom, an animate's animation. Even so, a
few retail atoms name a point past the end (a10's `moving_1_3` goes to point
5 of 5; 10 on the whole disc) and three squads name a platoon their
encounter doesn't have (a10, c40). The test prints and counts them.

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
offsets not yet read):

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
| `1a0` | AI conversations (not read) | |
| `1d8` | scripting data, one element: point sets at `00` | `80` |
| `240` | orders | `7c` |

These fit the field order of the scenario definition, anchored by the
structure BSP block at `210` ([pathfinding.md](pathfinding.md)) and the
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
placement script `char[32]` `40`, `i16` `60` (`-1`; R).

Zone (`38`): name, flags `u32` `20` (1 on 118 of 119 retail zones), manual
BSP `i16` `24`, firing positions `28` (`20`), areas `30` (`88`).

Firing position (`20`): position, reference frame `i16` `0c`, flags `u16`
`0e` (R: 21 distinct values; most common `0x42`, `0x41`, `0`), area `i16`
`10`, cluster `i16` `12`, surface `u32` `14`, normal yaw and pitch `f32`
`18` (pitch π/2 is straight up).

Area (`88`): name, flags `u32` `20`, then fields the tools compute: mean of
the area's firing positions `24`, its reference frame `i16` `30`, their
deviation `f32` `34`, first firing position `i16` `38` and count `i16` `3a`
(V: every area's run tiles the zone's firing positions, which are grouped by
area; an empty area is `-1+0`), cluster occupancy `3c`..`7c` (not written),
manual reference frame `i16` `7c`, flight hints block `80`.

Order (`7c`): name, style `i16` `20` (an index into the style palette at
`150`, `-1` none), flags `u32` `24`, force combat status `u16` `28`, entry
script `char[32]` `2c`, follow squad `i16` `4c`, follow radius `f32` `50`,
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

`h1ai::convert::convert(&H1Ai, &Options) -> Plan` makes the Halo 2 data as
the decoder's own types; `Plan::blocks(resolve)` writes them as
`map::build` blocks, resolving each character path to a datum in the map
being built; `ScenarioBlocks::apply(&mut scnr_root)` puts them on the
scenario's root block (character palette `178`, squad groups `158`, squads
`160`, zones `168`, orders `240`, scripting data `1d8`). The test builds
them into a map with `map::build::TagSet` and reads them back through
`h2::ScenarioAi::read`: the result equals the plan.

| Halo 1 | Halo 2 |
|---|---|
| encounter | a squad group and a zone of the same name; team carried over (S: both games number teams default, player, human, covenant, flood, sentinel) |
| firing positions | the zone's firing positions, grouped by area; one area per group letter used, named `a`..`z`, with the tools' mean, deviation, first and count filled in. Surface carried over (the converted collision BSP keeps Halo 1's surface indices); cluster `-1` |
| squad | squad `encounter/squad` (Halo 1 scripts' own spelling, cut to 31 characters), in the encounter's group and zone; counts, delay and major upgrade carried over |
| squad's attacking groups | an order of the squad's name over the areas of those letters (every area when none match), set as the squad's initial order |
| squad starting location | starting location: position, facing as yaw; a location's own actor becomes its own character |
| squad with no starting locations | kept, empty: no starting locations and counts 0, with a note. It can never spawn, but scripts name it (on a30 these are mostly the attack/defend/retreat squads Halo 1 maneuvers actors into). Retail Halo 2 has such squads too (06b: 28 of 170, `squad_flag_bits`). Until 2026-11-17 they were left out and their script names stayed `none`. The converter keeps only those the scripts name, and only while the level stays under the engine's squad cap (335, `h1campaign::scenario::MAX_SQUADS`, campaigns.md §9): a50 (309 placed + 75 named empty) and b40 (283 + 67) leave them out, and their names resolve to a sibling squad: the encounter's first converted squad (`<encounter>/...`), or its squad group if it has none. Not the group first: Halo 2's `ai_migrate` (function 308, evaluator `0x002a54a0`) returns without moving anyone when the destination has either top bit set (`test ecx,0C0000000h` at `0x002a54cc`), so a squad group destination is a no-op, and `ai_attach_units` (287, through `0x002730c0` to `0x00273040`) reads the value's low 16 bits as a squad index (bounded by `scnr+0x160` at `0x0027305e`), so a group would name an unrelated squad |
| actor variant | a retail character ([`character_for`]): one palette entry per distinct character |
| command list | a point set of its name (points `p0`..), an order stub of its name, and a `CommandScript` (atoms by name, with their points) for the scripts lane |
| conversations, platoons, move positions, respawn | not converted (noted) |

Values Halo 1 has no counterpart for come from the census (`Options`):
firing position flags `0x42`, zone flags 1, area type 0, order style `-1`.
Grenades by species: covenant plasma for elites and grunts, fragmentation
for marines and crew, none otherwise.

### Characters (the stopgap)

The nearest retail Halo 2 `char` by type, refined by words in the variant's
path. Every target is on the Halo 2 disc with the same type (V: the census
test checks the table against the disc).

| Halo 1 type (variant words) | Halo 2 `char` | On |
|---|---|---|
| elite (stealth major / stealth / zealot / specops / major / other) | `elite_stealth_major` / `elite_stealth` / `elite_zealot` / `elite_specops` / `elite_major` / `elite` | 01b, 03a, 03b, ... |
| jackal (major / other) | `jackal_major` / `jackal` | 03a, 03b, 05a, ... |
| grunt (specops / major / other) | `grunt_specops` / `grunt_major` / `grunt` | 04a, 04b, 07a / 01b, 03a, ... |
| hunter | `hunter` | 03a, 05b, 07a, 08a, 08b |
| marine, crew (johnson / sergeant) | `marine_johnson` / `marine_sgt` | 01a, 01b, 03a, ... |
| marine | `marine` | 01a, 01b, 03a, ... |
| crew | `marine\ai\crewman` | 01a |
| combat form (human / other) | `flood_combat_human` / `floodcombat_elite` | 06a, 06b, 07b / 04b, ... |
| infection form | `flood_infection` | 04b, 06a, 06b, 07a, 07b |
| carrier form | `flood_carrier` | 04b, 06a, 06b, 07b |
| monitor | `monitor\ai\monitor` | 04b, 08b |
| sentinel | `sentinel_aggressor_halo1` (Halo 2's own Halo 1 sentinel) | 04a, 04b |
| mounted weapon | `ai\generic_turret` | most levels |
| engineer, assassin, player, none | `ai\generic` | every level with AI |

A converted level needs each chosen tag in its map; no single retail map
holds them all. `h1campaign::donor` copies the ones the base (05a) lacks
from a donor, whole tree and sounds (`h1sounds::transplant`): hunter
from 03a, flood combat elite, infection, carrier and the monitor from
04b, flood combat human from 06a, and the special-ops elite and grunt
from 04b. Two stay out: the crewman (no donor; the marine stands in) and
the sentinel: every sentinel character but the enforcer shares
`sentinel_aggressor`'s animation graph, which `tags::schema` refuses (an
unknown pointer in the `+0xb4` block's elements, `donor_survey`).

### a30 and a10 (V)

`a30_and_a10_convert_read_back_and_sit_on_generated_pathfinding` converts
both levels, builds and reads back the result, then generates pathfinding
(`crates/pathfind`) from each BSP's collision converted by
`h1convert::collision::convert`, and checks positions against the
generated sectors: inside a sector's polygon with its floor at most 2 m
below and 0.25 m above.

| | a30 | a10 |
|---|---|---|
| squad groups, zones | 22 | 39 |
| squads (no starts, left out until 2026-11-17) | 89 (90) | 114 (30) |
| starting locations (off sectors) | 390 (24) | 296 (1) |
| firing positions (flying, off sectors) | 4645 (584, 52) | 2381 (0, 7) |
| orders, point sets | 92, 3 | 238, 124 |
| characters | 5 | 5 |

Every squad has a starting location and every index is in range. a30's
three banshee encounters fight in the air, over the sea: an encounter whose
firing positions are mostly off any floor counts as flying and is left out
of the floor check. The rest must be on a sector for at least 98% of firing
positions and 90% of starting locations; the misses (a30's river and cave
floor, a10's flanks) stand on scenery, which isn't in the structure BSP, on
slopes past 45°, or in water.

## Spawning

Halo 1 creates an encounter at load unless its flag bit 0 ("not
initially created") is set; on the ten levels almost all are set (a30:
21 of 22, b40: 74 of 75), so Halo 1's scripts place them. Halo 2 has no
such default: no squad on any retail campaign map sets a placement flag
(survey `squad_flag_bits`: only bits 8, 9 and 13, on a few squads), and
retail scripts place every squad with `ai_place`. So `h1campaign` adds a
startup script, `h1_initial_encounters`, placing the squad group of every
encounter with bit 0 clear (a30: `mid_cliff_1`), and Halo 1's own
`ai_place` calls keep one argument, as most retail calls do
(h1-scripts.md). Squad flags stay 0.

Zone firing positions and areas are written as shared arrays keyed by
their bytes: Halo 1 encounters repeat firing positions (b40's 20,789 hold
8,335 distinct; survey `h1_ai_sizes`), and the builder places identical
arrays once. On b40 the scenario's tree is `0x14838f` bytes and lays
out in `0xfe618`.

## What only a game run can prove

- That Halo 2 spawns the squads at all: the converted scenario still lacks
  the cluster occupancy bits of each area and the firing positions' cluster
  index (written `-1`), which the tools compute from the structure's
  clusters; the engine may need them to activate or search areas.
- That actors move between firing positions on the generated pathfinding.
- That the chosen characters, pulled into a Halo 1 level from other maps,
  load with their units, weapons and animations.
- That orders with no endings, and order stubs with no areas, are harmless.
- Halo 1 behaviour carried by the dropped data: platoons, move positions,
  respawning, maneuvers into the empty squads, conversations.

## Open questions

- The firing position flag bits (`0x1`, `0x2`, `0x4`, `0x8`, `0x40`, ...)
  and area set types 0..2: values from the census, meanings unknown.
- Whether `area +0x3c` really is cluster occupancy, and whether the engine
  reads the area's mean and deviation or recomputes them.
- The three trailing `i16` fields that are `-1` on retail (squad `70`,
  starting location `60`, order `4e`).
- Halo 1 squads' other six group masks (defending, guarding, pursuing):
  Halo 2 expresses them as order endings and secondary area sets; only the
  attacking mask is used.
- Vehicle actor variants (`warthog gunner`, `banshee` pilots) become plain
  characters; Halo 2 puts them in vehicles through the starting location's
  vehicle and seat fields, which need the vehicle palette.
- How the scripts lane names the converted squads and command scripts; the
  plan uses Halo 1's `encounter/squad` names and the command lists' names.
