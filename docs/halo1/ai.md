# Halo 1 AI data, and Halo 2's

`crates/h1ai` is step 5 of the campaigns work ([campaign.md](campaign.md)):
it reads Halo 1's AI data out of an Xbox cache file (through `h1map`),
decodes Halo 2's scenario AI blocks and `char` tags out of a v8 map, and
converts the first into the second as `map::build` blocks. Halo 2's AI then
runs Halo 1's encounters (decision 1: where the engines differ, Halo 2 wins).

The readers have been checked on every campaign map of both discs. The a30
ground placement and loadout checks below use the source maps and generated
scenario. Soldier movement and combat still need a live a30 observation.

Evidence labels, as in [map-format.md](map-format.md):

- **V (verified on disc):** checked by the `#[ignore]` tests in
  `crates/h1ai/tests/disc.rs` on the owner's discs. Strides were read off
  the spacing of consecutive blocks (count × stride = gap to the next block)
  and then checked by decoding every element with range checks.
- **S (unverified):** a field name or enum order that our disc checks and
  engine reads do not establish. Offsets were read off the maps.
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
search behavior `u16` `28`, manual BSP `i16` `2a`, compiled BSP `i16` `7e`, respawn delay `f32[2]`
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
| firing positions | the zone's firing positions, grouped by area; one area per group letter used, named `a`..`z`, with the tools' mean, deviation, first and count filled in. Ground points name a generated sector in the encounter's assigned BSP. Airborne points retain their authored altitude and cluster |
| squad | squad `encounter/squad` (Halo 1 scripts' own spelling, cut to 31 characters), in the encounter's group and zone; counts, delay and major upgrade carried over |
| squad's attacking groups | an order of the squad's name over the areas of those letters (every area when none match), set as the squad's initial order |
| squad starting location | starting location: position, facing as yaw; a location's own actor becomes its own character |
| squad with no starting locations | **Built.** A named migration destination has its own squad datum, zero initial occupants and an order. A direct script placement with an actor can use the squad's move positions or its assigned firing groups. |
| actor variant | One palette entry per distinct character. Crew and Sentinel variants are rebuilt from the source tags. Other types use the retail character table. |
| command list | **Built.** Its own point set and Halo 2 command script. The authored list index selects both. A starting location with a command list carries the generated script name and index. |
| conversations | **Built** as Halo 2 scripts with converted `snd!` references (below) |
| move positions, respawn | not converted (noted) |

Values Halo 1 has no counterpart for come from the census (`Options`):
firing position flags `0x42`, zone flags 1, area type 0, order style `-1`.
Grenades by species: covenant plasma for elites and grunts, fragmentation
for marines and crew, none otherwise.

### Conversations

**Built.** `h1ai::conversations` reads retail participants and all six sound
references on every line. `h1scripts` builds the worker and state into the
scenario's regular Halo 2 script tables. `h1campaign` installs the referenced
Halo 1 sounds with the other script sounds and resolves their datums.

| Element | Offsets used | Evidence from Halo 1 Xbox |
|---|---|---|
| Participant | Flags `02`, selection `04`, actor type `06`, use and assign object name `08`/`0a`, six voice selectors `18`, encounter token `24` and compiled AI selector `44` | `0x00143c00` selects a role, `0x001440f3` compares voice selectors, and `0x0014476c` installs the chosen unit under the assigned object name |
| Line | Flags `00`, speaker `02`, addressee kind/index `04`/`06`, post-line pause in seconds `0c`, six `snd!` references starting `1c` | `0x001425a0` takes the cast's voice slot, resolves the sound and converts the pause to ticks. `0x00144abd` waits for advance on flag `0x08` |
| Conversation | Delay flag `0x40`, trigger distance `24`, run distance `28` | `0x00144466` tests the distance, and `0x00144b7a` retries at 30-tick intervals |

**Built.** Cast selection ranks distinct living units in the authored
encounter or the converted rosters for their actor type. The original
selection switch (`0x00143f6d`) checks allegiance, shared vehicles, unmounted
actors or preference for a unique voice. The bridge reads Halo 2's live
campaign alliance matrix (`0x001df560`) and vehicle driver (`0x002a4270`).
Candidates closer to players and to an already cast peer get the original
distance and view-direction weights (`0x00143a96`, `0x00144026`).
For the first cast member, the bridge screens prospective players with
Halo 1's directional BSP visibility rows (`0x00143a37`). The converter
copies those rows from the Halo 1 BSP's `+0x140/+0x14c` data reference into
the rebuilt v8 BSP at `+0x54/+0x58`. A clear head-to-head collision ray or
separation under three world units earns the original `0.5` bonus (Halo 1
`0x00143a6e`, Halo 2 `0x000b8d30` and `0x001697c0`). The Halo 2
object location update stores the cluster in the object header (`0x000b866b`).
Voice selection prefers the exact persona, then the wildcard, then an
eligible generic alternative (`0x001440e2`).
Assigned object names still resolve to their captured actors.

**Built.** Generic voices are seeded at unit construction. The converter
pairs converted units and AI starting locations with the generic selectors
authored on the source biped or vehicle (`+0x2b4`). The native unit hook
rotates through those selectors in author order, including repeated zeroes,
with a process-wide counter shared across unit definitions (Halo 1
`0x000466e0`, Halo 2 `0x000c4410`).

**Built.** The map carries the source marine squads' unique-leader rules.
The placement-wave hook applies the forced, random and encounter-size
rules before the first spawn attempt (Halo 1 `0x0013e563`, Halo 2
`0x00201520` and `0x00203780`). The voice and leader flag follow the salted
unit datum through squad migration and cast changes. The script map-init
hook moves early births into scenario globals, which carry the profiles
in checkpoints. Rebuild converted maps to install the spawn metadata and
checkpoint globals.

**Built.** Flag `0x20` runs the cast toward the focus player using the
source distance at `+0x28` (Halo 1 `0x0014476c`). Line flags `0x10` and
`0x20` wait for the speaker or the cast to finish moving
(`0x001448db`). Halo 2's native `cs_approach` (429,
`0x002a81b0`) does the movement and pathfinding. The scene chooses
a live focus player and applies the view cone when flag `0x80`
requires attention (`0x001445de`).

**Built.** The speaker, the addressee and other participants turn as their
line directs (`0x00144c30`). Halo 2's `cs_face_object` and
`cs_look_object` (400 and 394) run as temporary commands stacked over the
actors' existing commands (`0x00276480`). Line completion or a scene
stop removes only its own commands by script and actor (`0x00258540`).
Selection 1 supplies an unbound radio voice. Selections 6 and 7 bind
an actor and route the voice over radio (`0x001426a3`, `0x00144992`).

**Built.** Every spoken line owns its native sound instance. Spatial
voices use the unit's head marker and the biped lip-sync path
(`0x001891d0`, `0x00109220`). Radio uses the engine's untracked impulse
entry (`0x00189fe0`). Workers poll the salted sound datum until it ends
before applying the authored pause and advance flag. Stopping calls the
engine by instance (`0x00126360`), so overlapping lines sharing a
`snd!` tag keep separate timing and stop handles.

The converted-map hook claims **builtin slot 845** only. Its retail
descriptor is four longs returning void and its original evaluator
(`0x002ab130`) discards the call. The shared table is
`hs::h1_conversation::BUILTINS`. Approach, facing and queue queries use
stock builtin slots. The converted-map operation validates scenario
global types and uses salted object, actor and sound handles.

### Command scripts

**Built.** The ten Halo 1 campaign scenarios contain 552 command lists.
`h1scripts::compile::commands` appends a kind-5 definition for each, in
list order after the authored scripts. A list may have the same display
name as another list. The generated name `h1_cmd_<index>` keeps their
references distinct. Halo 1 checks the resolved BSP at list offset
`+0x2e` (`0x16a808`). Each point keeps its position and collision surface.
Halo 2 addresses a point by set index in the high word and point index
in the low word.

The generator emits blocking Halo 2 command-script verbs for movement,
look, animations, jumps and delays. Backward edges repeat their command
sequence. The stimulus can arrive before a wait, and interruption retires
the executing instance. The boot feature handles the H1 commands that
need a runtime calculation. H1 `0x16b74d` uses the direction from the
unit to a point, while `0x16b895` uses the unit's facing for a duration.
H2's begin/update pair is `0x258cf0` / `0x259430`. The two H1 vehicle
atoms search for a driver seat on a nearby live vehicle (`0x16bd0c`).
H2 tests seat admission at `0xc8200` and submits the entry behavior at
`0x1a80e0`. H1's falling-damage exemption sets and clears a unit flag at
`0x16a603` and `0x16b23c`. H2's landing routine tests its counterpart at
`0xe3727` before applying the distance damage effect.

Vocalize looks up the actor model's converted dialogue and plays its
source sound on the unit. H1 indexes the dialogue tag at `0x46a40`
and follows the executable's fallback table when the line is absent.
None of the retail campaign lists uses this atom.

#### Cinematic bindings (x50 and c40)

**Built.** Use the command list index from the containing H1 scenario.
`h1ai::h1::command_script_name(i)` returns `h1_cmd_<i>` with the
zero-based index in decimal. For example, source list 7 in a level with
40 authored scripts becomes `h1_cmd_7`, H2 script index 47. Resolve the
index independently for each level. Duplicate display names are legal.

| Caller | Binding |
|---|---|
| Converted H1 script | `ai_command_list(ai, list)` runs the generated script on the group. `ai_command_list_by_unit(unit, list)` resolves the unit with `object_get_ai`. Both emit `cs_run_command_script`. |
| Generated H2 cinematic code | Pass the actor and a type-21 `ai_command_script` literal to `cs_run_command_script`. Its data is the generated definition's index. Its token is the generated name. The none value is `0xffffffff`. |
| Scenario starting location | Set `h1ai::convert::Options::first_command_script` to the number of authored scripts. Conversion binds `placement_script` to the generated name and `placement_script_index` to the first script plus the source list index. A location without a list keeps index `-1`. |
| Advance and poll | The H1 advance calls deliver one stimulus per addressed actor. Status of an object list is the highest member status: 0 inactive, 1 recently finished, 2 waiting for advance, 3 running. |

#### Command execution

**Built.** H1 selects gaze strengths 5, 2, 8 and 7 for modifiers 1 through
4 (`0x16b68f`). The default changes from 7 to 2 at combat status 4
(`0x15c244`). The head-only, aiming and forced-facing controls on H2
are set on the executing command. Strength 7 requests a body turn when
the target leaves the source actor's horizontal gaze cone. Perceived
units stay live. Unperceived unit targets contribute their head position
once (`0x16b6c9`). The gaze expires in whole H1 ticks, truncated at 30 Hz
before conversion to the simulation clock. Random looks with no authored
duration read the actor's idle interval at `actr+0xec/+0xf0`.

Go-to-and-face also waits for alignment after arrival with an authored
radius. H1 `0x16ae14` requires a dot product of at least 0.984 with the
direction to the face point. Ground actors use XY and flying actors use
XYZ. The source flight flag is bit 21 of `actr`. A driver can also inherit
flight from the vehicle (`0x14c4f5`).

**Built.** Shoot requests fire on entry (`0x16bc4c`). On update, a point
target must be strictly less than half a world unit from the authored
point. Each miss restarts the timer at the variant's `actv+0x84` interval
with a two-second floor. An already accepted point can finish on the
first update (`0x16af4a`). H2 copies its accepted target at `0x1fcb4b`.
A positive authored duration overrides the burst length for this actor
(`0x1623ba`). The normal firing controller handles subsequent shots.
Placement string IDs identify source tuning after an actor migrates.

**Built.** All six animation modifiers start one custom animation. Modes
0–2 blend for six source ticks and modes 3–5 start directly (`0x3f319`).
Modes 1 and 4 take XYZ velocity from the playing channel and use normal
collision. Modes 2 and 5 also advance through obstructions. H1 sets the
two biped flags at `0x16bf18`, rotates the authored XY by the biped
heading at `0x4977f`, and clears the flags at `0x16abee`. The H2 hook
reads the converted root-motion stream at the live frame and playback
rate (`0x1c6d00`). Completion follows the unit leaving its custom
animation state, including interruption (`0x16b0b6` in H1, `0x11b930`
in H2).

### Character selection

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
| crew | Halo 1 actor variant, unit, colours and weapon | source level |
| combat form (human / other) | `flood_combat_human` / `floodcombat_elite` | 06a, 06b, 07b / 04b, ... |
| infection form | `flood_infection` | 04b, 06a, 06b, 07a, 07b |
| carrier form | `flood_carrier` | 04b, 06a, 06b, 07b |
| monitor | `monitor\ai\monitor` | 04b, 08b |
| sentinel | Halo 1 actor variant, unit, weapon and animation graph | source level |
| mounted weapon | `ai\generic_turret` | most levels |
| engineer, assassin, player, none | `ai\generic` | every level with AI |

A converted level needs each chosen tag in its map; no single retail map
holds them all. `h1campaign::donor` copies the ones the base (05a) lacks
from a donor, whole tree and sounds (`h1sounds::transplant`): hunter
from 03a, flood combat elite, infection, carrier and the monitor from
04b, flood combat human from 06a, and the special-ops elite and grunt
from 04b.

**Built — Sentinel graph.** The Halo 1 variant points to its own rebuilt
biped. The model converter reads the biped's `antr` and emits a `jmad`
with local animation resources and owner fixups. On c40, the fixed weapon
class's slot 0 selects each stance's idle. Slot 25 selects the nine-frame
aim with a three-by-three yaw/pitch screen. The builder emits the idle
and lookups for combat, patrol and crouch under class/type `any/any`, as
on 04b. Combat and patrol share the four directional aim overlay names.
c40's ordinary and
defensive variants both name the Sentinel beam and upgrade to the major.
The eleven flight parameters at Halo 1 `bipd+0x324` correspond to the
sequence at `+0x2cc` on 04b's Halo 1 aggressor. Both retail aggressors
have biped flags `0xe00` and character-physics flags `0x13`. The style
comes from 04b when donor maps are supplied.

**Built — crew.** Each distinct `actv` gets a definition-built `char`
with its unit, perception, vitality and weapon properties. a10's tech,
engineer, security, science and command variants have different RGB
bounds in the block at `actv+0x22c`. Their rebuilt bipeds share a model
and graph. Security carries the source pistol. The other five variants
(including the pilot) have null weapon references.

**Built — firing positions.** Halo 1 encounter flag `0x20` marks all
three a30 Banshee encounters (584 points). They keep their altitude and
use a null sector, as in 06a's flying Sentinel zones. Ground points use
their assigned BSP at encounter `+0x7e`. The surface reference and the
generated sector polygons give the floor point and normal, including on
slopes and polygon edges. Halo 2 uses a sector index at firing position
`+0x14`, confirmed against 05a's sector polygons. Ground clusters are
queried just above the floor and area centres and deviations recomputed.

The placement counts in this census cover squads with starting locations. a30's
three banshee encounters fight in the air, over the sea: an encounter whose
firing positions are mostly off any floor counts as flying and is left out
of the floor check. The rest must be on a sector for at least 98% of firing
positions and 90% of starting locations; the misses (a30's river and cave
floor, a10's flanks) stand on scenery, which isn't in the structure BSP, on
slopes past 45°, or in water.
**Built — script-created and migration squads.** Halo 1 squad literals
encode `0x80000000 | local_squad << 16 | encounter`. The converter uses
these indices to retain named targets, including on a50 and b40. The
native migration evaluator requires a squad datum (`0x002a54cc`).
Direct placements of empty squads with a defined actor use authored
positions from that squad's movement or firing groups. This site-selection
rule is an inference. Examples include a50's `marines_initial/marines_a`
and b40's `a2_top_cov/grunt_sniper`. b40's `a2_bottom_cov/grunts_tub`
has no actor or start to instantiate. It remains addressable for migration.

The campaign squad-pool hook runs before `0x00200b30` initializes the
arrays. Retail allocates consecutive `0xc760` and `0x163c` buffers
at `0x00200957` and `0x0020098a`. When necessary, it moves the boundary
within those buffers using the new scenario's counts. Smaller maps restore
the retail capacities. The converter uses the same capacity calculation.

The ignored disc tests cover the a30/a10 sector readback, every campaign's
scripted squad references, and the a10/c40 character-to-animation resource
chain. Gameplay and map-size verification are pending integration.

### a30 ground encounters

The manual BSP field is zero throughout a30. Its compiled BSP selects the
second structure for the later plateau encounters. The two structures have
separate sector tables. In retail 05a, 469 of 499 assigned firing positions
in the landing zone and 186 of 188 in the tower zone lie inside the named
sector in XY. Treating those indices as collision surfaces produced zero
polygon matches in both samples.

Applying the generator's walkability and area cuts to a30's source
collision assigns 1,635 of 1,656 ground positions in BSP 0 and 2,373 of
2,405 in BSP 1. The other 53 have no walkable source surface. The converter
keeps their coordinates and leaves the sector unassigned. In the first
wave 510 of 512 positions share one component through two-sided links.
All 54 in `mid_cliff_1` share a component in BSP 1. All 17 first-marine
starts and all five mid-cliff starts stand over the main firing component
of their respective encounters. None of the first wave's 73 starts stand
over its main firing component. That wave stages elsewhere and its scripts
place platoons and load their actors onto dropships. Checking a path from
the authored start to the landing zone would miss that transfer.

The ignored `a30_converts_onto_its_base_and_reads_back` test checks the
emitted map's sector references, cluster masks, connectivity and starting
weapons against the characters' inherited firing profiles.

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
  The agent method `world.ai` reports squad, cached sector, behavior, focus
  object and combat status. Match its unit ID to positions across ticks in
  `world.object`. [Agent API](../runtime/agent-api.md#worldai)
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
