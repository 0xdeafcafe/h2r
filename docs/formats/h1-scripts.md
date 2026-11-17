# Halo 1 campaign scripts to Halo 2

`crates/h1scripts` reads Halo 1's compiled scripts from a scenario tag,
decompiles them to HSC text, maps Halo 1's builtins and types to Halo 2's, and
compiles the result to Halo 2 syntax nodes and scenario tables for the
converter. It is step 6 of [campaigns.md](../game/campaigns.md) (the scripts
row of §2). Halo 2's side is [haloscript.md](haloscript.md) (`crates/hs`);
builtins Halo 2 lacks are candidates for the Rust VM ([hs_vm.md](../engine/hs_vm.md)).

No script text, table or value from either disc is in the repository. The
crate reads the owner's `disc/halo1.iso` and `disc/halo2.iso` at run time, and
its unit tests use invented builtins, types and scripts.

Evidence key: **E** an instruction or table in Halo 1's XBE (addresses are
guest VAs in the retail Xbox `default.xbe`, title `4D530004`); **D** checked
on all ten campaign maps (a10 a30 a50 b30 b40 c10 c20 c40 d20 d40).

## Halo 1's script tables in the scenario

Offsets from the start of the `scnr` tag. Blocks are the usual Halo 1 tag
block (count, address, unused); data references are 20 bytes: size `+0`,
address `+0xc`. The structure BSP block at `0x5a4`
([h1-map-format.md](h1-map-format.md)) places these fields in the same layout
as the PC scenario.

| Field | Layout | Evidence |
|---|---|---|
| `+0x474` script syntax data | data reference: a data array | **D**: header name `script node`, element size 20, signature `d@t@` |
| `+0x488` script string data | data reference: NUL-terminated tokens | **D**: every head, global and symbol token resolves inside it |
| `+0x49c` scripts | block, `0x5c` each: name[32], u16 type `+0x20`, u16 return type `+0x22`, root datum `+0x24` | **D**: every root resolves with its salt; names match script-call tokens |
| `+0x4a8` globals | block, `0x5c` each: name[32], u16 type `+0x20`, root datum `+0x28` | **D**: global atom tokens match the names at their indices |
| `+0x4b4` references | block, `0x28` each: tag reference at `+0x18` | **D**: groups and IDs read through `h1map` |

The syntax data is a data array header of `0x38` bytes (max count u16 `+0x20`,
element size u16 `+0x22`, signature `+0x28`, count up to the highest live slot
u16 `+0x2e`) followed by **every** slot up to the max count (19,001 on the
retail maps). A slot is 20 bytes with **the same layout and flag bits as
Halo 2's node** (salt, opcode, type, flags, next, string offset, data):
`hs::Node` decodes it. Salt zero is a free slot. A call's child is a function
name atom (type 2), whose sibling chain is the argument list. Flag bit 1 makes
the opcode a script index; bit 2 makes an atom a global (bit 15 of the index
selects an engine global).

Like Halo 2, numeric atoms keep string offsets of discarded source that can
point past the string data; literals are decoded from their bits by source
type (the atom's opcode), and only strings and symbols read their token.

### Head indices one behind the call

On a30 (172 calls) and b30 (99), some function-name atoms hold the index one
**below** their call's: the first found is a call to
`player_effect_set_max_translation` (371) whose head says 370
(`structure_lens_flares_place`). The call's index matches the head's name in
the XBE table every time, so the call is authoritative. Inference: these two
levels were compiled while the heads' indices came from a table one entry
shorter. The decompiler counts these (`Stats::stale_heads`) and rejects any
other difference.

## Halo 1's builtin, type and script type tables (XBE)

Unlike Halo 2's, Halo 1's function definitions carry their names, so every
builtin is named from the executable. `Catalog::from_xbe` checks the title ID
and the count instruction and reads:

| Table | Evidence |
|---|---|
| Function pointer table `0x0022dd80`, 418 entries | **E**: `0x000f1d40` dispatches `mov eax, [eax*4+0x22dd80]` and passes definition `+0x1a` as the parameter types; the loop set up at `0x000ec21f` walks the table and stops at `cmp si, 0x1a2` (`0x000ec271`) |
| Value type names `0x0021c4e0`, 49 pointers | **E**: read at `0x000e9fdc`, `0x000ea350`, `0x000ead4c`, `0x000eade0`; the 50th word is the integer 5, not a pointer. **D**: every node type is below 49 |
| Script type names `0x0021c260`, 5 pointers | startup, dormant, continuous, static, stub, just before the type names' strings |

A function definition: u16 return type `+0`, name pointer `+4`, parse `+8`,
evaluate `+0xc`, documentation `+0x10`, parameter help `+0x14`, u16 parameter
count `+0x18`, u16 parameter types from `+0x1a`. There is no flags word. The
parse function tells special forms apart: 391 definitions share one parser
(fixed signatures, arguments checked against the parameters); the other 27,
such as `begin` and `!=`, have their own.

## Step 1: decompile

`decompile::decompile(&Tables, &Catalog)` writes `(global ...)` and
`(script ...)` forms and checks, for every node it reaches: salted links,
builtin index in range, head token equal to the XBE name at the call's index,
script-call index naming the token's script (and no arguments), scenario
global index naming the token's global, and for fixed-signature calls the
argument count and each argument's type equal to the declared parameter type.
Each script's return type and each global's type must be a known type.

Proof, `cargo test -p h1scripts --test disc -- --ignored` (reads the discs):
every script on all ten maps decompiles; every live node is reached from a
script or global root; no unknown builtin; every fixed-signature call checks.

| Map | Scripts | Globals | Refs | Live = reached nodes | Builtin calls (checked / special) | Script calls | Global refs | Literals | Builtins used |
|---|---|---|---|---|---|---|---|---|---|
| a10 | 185 | 84 | 238 | 17,516 | 5,302 (3,593 / 1,709) | 150 | 289 | 6,323 | 195 |
| a30 | 139 | 108 | 60 | 12,195 | 3,938 (2,193 / 1,745) | 44 | 473 | 3,758 | 148 |
| a50 | 209 | 128 | 73 | 14,980 | 4,786 (2,112 / 2,674) | 81 | 964 | 4,282 | 162 |
| b30 | 85 | 74 | 31 | 4,557 | 1,451 (848 / 603) | 24 | 164 | 1,443 | 122 |
| b40 | 225 | 45 | 56 | 10,436 | 3,430 (1,832 / 1,598) | 53 | 369 | 3,101 | 145 |
| c10 | 198 | 70 | 98 | 9,311 | 2,948 (1,850 / 1,098) | 66 | 172 | 3,111 | 140 |
| c20 | 232 | 50 | 47 | 8,791 | 2,933 (1,542 / 1,391) | 141 | 389 | 2,254 | 115 |
| c40 | 179 | 41 | 95 | 7,841 | 2,448 (1,484 / 964) | 73 | 251 | 2,548 | 126 |
| d20 | 189 | 58 | 68 | 7,648 | 2,442 (1,294 / 1,148) | 144 | 350 | 2,126 | 140 |
| d40 | 297 | 65 | 144 | 13,320 | 4,059 (2,277 / 1,782) | 223 | 579 | 4,177 | 160 |

No node is shared (emitted = reached on every map).

## Step 2: translate

Halo 2's descriptors have no names; `crates/hs` names a builtin only when
some retail Halo 2 script calls it (390 of 911). `translate::Target` is built
from an `hs::Catalog` that has observed every Halo 2 map, plus the engine
globals Halo 2's scripts name. A Halo 1 builtin is therefore **missing** when
no retail Halo 2 script calls a builtin of that name; Halo 2 may still have it
under an index nobody calls. Value types map by name; Halo 2 lacks
`actor_variant` and `ai_default_state`.

`translate::Table::build` classifies each Halo 1 builtin:

- **Direct**: same name; same parameter and return types once mapped, or a
  special form on both sides.
- **Renamed** (`RENAMES`): `ai_magically_see_encounter` → `ai_magically_see`,
  `game_save_totally_unsafe` → `game_save_immediate`, `ai_exit_vehicle` →
  `ai_vehicle_exit`, `object_pvs_set_object` → `pvs_set_object`,
  `object_pvs_clear` → `pvs_clear`, `ai_living_fraction` → `ai_strength`
  (Halo 2's squad vitality, 0 when all are dead; a30's waits on a fraction
  under 0.3 or at 0), `sound_impulse_time` → `sound_impulse_language_time`
  (and `ai_attach` → `ai_attach_units`, which is also a cast, so it counts
  as an adapter). Both impulse times
  are the ticks a playing impulse sound has left, 0 once it ends: a10
  waits `(sleep (sound_impulse_time line))` after a line and
  `(sleep_until (= 0 (sound_impulse_time line)))` for one to finish.
  **E:** Halo 2's evaluator `0x002a9fe0` (builtin 602, `(sound) -> long`)
  finds the sound among the 16 script impulses (`0x0018a240`) and returns
  its permutation's length less the time played, times 30. Stubbed (to
  converter version 16) every such wait ended at once, so a level's
  dialogue played over itself. `sound_impulse_time_waits_on_the_playing_sound`
  checks the mapping on the discs.
- **Named by position**: Halo 2 builtins no retail script calls, named
  from the XBE (below, "Builtins named by position").
- **`ai_status`** has no Halo 2 counterpart of the same scale. Halo 1's
  XBE help text gives its scale (0 inactive, 1 noncombat, 2 guarding, 3
  search/suspicious, 4 definite enemy, 5 visible enemy, 6 engaging);
  Halo 2's `ai_combat_status` is compared, in its own scripts, with
  engine globals in ascending order (`ai_combat_status_idle`, `_alert`,
  `_active`, `_uninspected`, `_certain`, `_visible`, ...).
  `compile::STATUS_LADDER` pairs them: Halo 1 status `k` is reached when
  Halo 2's reaches the global for `k` (6 visible, 5 certain, 4
  uninspected, 3 active, 2 alert, 1 idle; a visible enemy counts as
  engaging, which is what Halo 1's waits on 6 ask). Every use in the ten
  levels (35; a50, b30, b40) compares it with a number, and is compiled
  as one comparison: `(> (ai_status x) 3)` as `(>= (ai_combat_status x)
  ai_combat_status_uninspected)`, `<` and `<=` with `<`, `=` as an `and`
  of both, a threshold off the scale as `true` or `false`. Any other use
  is compiled as the full ladder, `(if (>= (ai_combat_status x)
  ai_combat_status_visible) 6 (if .. 5 .. 0))`, about 54 nodes a use
  (as a ladder everywhere b40's scripts grew by 1,188 nodes and it no
  longer fit its tag memory).
- **Adapter**: `Coerce` when the arity matches but types differ (argument
  nodes take Halo 2's parameter types and Halo 2's interpreter casts; a
  marker-name string becomes a `string_id` literal; a string the
  interpreter cannot cast becomes a hook of the expected type, below); or
  one of `ADAPTERS`:
  `ai_place` keeps Halo 1's one argument (`OmitOptional`: Halo 2's count
  is optional; retail Halo 2 scripts leave it out in 1,309 of 2,100 calls
  and never pass 0, survey `ai_place_counts`), `unit_exit_vehicle` gains a `short` mode
  (1, what every retail Halo 2 call passes), `device_group_set` gains `none`
  as its device, and `!=` becomes `(not (= a b))` because Halo 2's scripts
  never call `!=`.
- **Missing**: no name match, a different arity, a special form on one side
  only, or a parameter type Halo 2 lacks.

### Builtins named by position

`translate::Target::from_catalog` also names 25 Halo 2 builtins no retail
script calls (`h1scripts::identified::IDENTIFIED`), so the Halo 1
builtins of those names translate like any other. Halo 2's table keeps
Halo 1's order with entries inserted between, so each is the only unnamed
Halo 2 builtin between the named ones around it whose signature (by type
name) is Halo 1's: the recordings 138 to 142 (between
`physics_disable_character_ground_adhesion_forces` 129 and
`texture_camera_set_object_marker` 144; E: the evaluators of 138, 139 and
140 call one function, `0x001fb360`, with play flags 0, 8 and `0x10`,
which reads the scenario's recorded animations), `object_set_facing` 106,
`scenery_animation_start` 184 and `_at_frame` 188, `unit_open` 194,
`unit_close` 195, `custom_animation_list` 205,
`unit_custom_animation_at_frame` 206, `unit_is_playing_custom_animation`
207, `unit_enter_vehicle` 216, `unit_suspended` 249, `camera_set_relative`
451, `camera_set_animation` 452, `camera_set_first_person` 456,
`camera_set_dead` 457 (an object in Halo 2), `object_pvs_set_camera` 471,
`cinematic_set_title_delayed` 561, `vehicle_hover` 621 and `show_hud` 625
(the first of three boolean-to-boolean builtins after
`hud_cinematic_fade`; the weakest). For waits outside the cutscenes:
`device_get_power` 255 (`(device) -> real`, between `device_set_power`
254 and `device_set_position` 256; E: its evaluator `0x002a4780` returns
the device's float at `+0x134`; d40's control rooms wait on it) and
`unit_has_weapon_readied` 243 (`(unit object_definition) -> boolean`,
after `unit_has_weapon` 242; a30's sniper tutorial). Other unnamed
descriptors with plausible signatures (241 `(unit) -> short`, 600
`(sound) -> long`, 586 and 587 `() -> boolean`) are not named: their
neighbours do not pin them down. Each is added only when that index is
unnamed and its signature matches. The neighbours and reasons are in the
module. Only the recordings have more than position as evidence.

Object, AI, device, trigger volume, cutscene, tag, enum and string ID values
keep their Halo 1 token and become hooks (step 3); AI encounters map to
squads in another lane.

Coverage, share of builtin calls (`translation_coverage`):

| Map | Calls | Direct | Renamed | Adapter | Missing | Missing builtins |
|---|---|---|---|---|---|---|
| a10 | 5,302 | 79.2% | 0.6% | 4.6% | 15.7% | 78 |
| a30 | 3,938 | 78.5% | 0.6% | 5.9% | 15.0% | 52 |
| a50 | 4,786 | 82.3% | 0.1% | 7.9% | 9.7% | 56 |
| b30 | 1,451 | 85.9% | 0.1% | 3.2% | 10.8% | 36 |
| b40 | 3,430 | 76.9% | 0.7% | 10.5% | 11.9% | 47 |
| c10 | 2,948 | 82.6% | 0.2% | 7.8% | 9.4% | 36 |
| c20 | 2,933 | 86.4% | 0.2% | 3.7% | 9.7% | 28 |
| c40 | 2,448 | 84.7% | 0.1% | 5.6% | 9.6% | 36 |
| d20 | 2,442 | 85.7% | 0.5% | 5.4% | 8.4% | 35 |
| d40 | 4,059 | 84.3% | 0.1% | 5.0% | 10.6% | 56 |
| **all** | **33,737** | **82.0%** | **0.4%** | **6.1%** | **11.5%** | **139** |

Missing builtins by calls across the campaign (top 40 of 139; 3,882 calls in
all; the test prints the full list with signatures):

sound_impulse_time 493 (now renamed, above), ai_magically_see_players 180, ai_conversation 169,
ai_command_list 150, ai_go_to_vehicle 143, recording_play 141,
ai_command_list_by_unit 136, show_hud_help_text 125, recording_time 96,
object_beautify 93, ai_follow_target_players 93, ai_spawn_actor 82,
vehicle_hover 77, ai_automatic_migration_target 72, hud_set_help_text 70,
recording_play_and_delete 65, recording_kill 63, ai_command_list_advance 61,
ai_attach_free 60, object_pvs_activate 57, ai_vehicle_encounter 57, show_hud 56,
unit_set_emotion 52, player_effect_set_max_translation 52, unit_suspended 51,
recording_play_and_hover 49, ai_link_activation 45, ai_retreat 44,
ai_status 44, hud_set_objective_text 44, unit_close 43, ai_playfight 43,
unit_set_seat 42, ai_try_to_fight 37, ai_conversation_status 34,
unit_enter_vehicle 32, enable_hud_help_flash 30, ai_conversation_stop 29,
ai_vehicle_enterable_distance 27, ai_dialogue_triggers 27.

They group into: recordings (`recording_*`), command lists (`ai_command_list*`,
which Halo 2 replaced with command scripts), conversations (`ai_conversation*`),
the Halo 1 HUD help and objective text, encounter-level AI control
(`ai_magically_see_players`, `ai_follow_target_*`, `ai_retreat`,
`ai_attack`, `ai_defend`, `ai_maneuver*`, `ai_vehicle_encounter`,
`ai_link_activation`), unit doors and seats (`unit_open`, `unit_close`,
`unit_set_seat`, `unit_enter_vehicle`, `unit_suspended`), and
`sound_impulse_time`, which a script waits on after almost every line of
dialogue (since mapped to `sound_impulse_language_time`).

## Step 3: compile

`compile::compile(tables, h1_catalog, table, target, policy, reference_names)`
builds Halo 2 tables: every node live and reached, salts `0x8000 | slot`, one
interned string blob, scripts and globals in Halo 1's order (script and global
indices carry over unchanged; Halo 1's script types 0–4 are Halo 2's).
Argument nodes take the Halo 2 parameter type as their node type, as the
retail compiler writes them; literals keep their source type.

- **Hooks**: an atom of a symbolic type (object and unit names, AI, trigger
  volumes, cutscene flags and points, device groups, tags, enums, string IDs,
  adapter defaults) holds `none` (`0xffffffff`), keeps its token, and is listed
  in `Compiled::hooks` with its slot, Halo 2 type and Halo 1 type.
  `Compiled::resolve_hooks` writes the converter's values in.
- **Casts**: Halo 2's interpreter casts every value to its node's type
  through a table with no null check (`hs::castable`, haloscript.md). A
  string literal stays a string only where that cast exists; elsewhere it
  becomes a hook of the expected type. A seat name where Halo 2 wants a
  `unit_seat_mapping` (`count << 16 | first` into the scenario's mapping
  table; retail `0x10004`) is written 0, an empty mapping, until the
  converter builds that table: b30's first fault was such a string, cast
  through a null table entry (call to address 0). The disc test
  `every_level_s_scripts_run_on_the_retail_interpreter` (h1campaign)
  checks every reached node of every level that converts against the
  cast rules and the builtin table; `hs`'s `casts_disc` checks the same
  rules clean on all 30 retail scenarios (295,322 reached nodes).
- **Missing builtins** follow `Missing`: `Stub` replaces the call with the
  expected type's zero (`false`, `0`, `none`, void) so retail Halo 2 runs it,
  except the status queries in `compile::DONE_VALUES`, which return the
  value every use in the ten levels reads as done (`ai_conversation_status`
  5, `ai_command_list_status` 2, `game_all_quiet` and `game_safe_to_speak`
  true, `hud_get_timer_ticks` 32767 so d40's escape timer never runs out,
  `ai_conversation_line` 99, past any line, for b30), so a `sleep_until`
  on them ends;
  `Extension` keeps the call with opcode `0x8000 + Halo 1 index` and the Halo 1
  name, for the Rust VM to bind (the retail interpreter cannot run it). Both
  list each call in `Compiled::missing`. Engine globals Halo 2's scripts
  never name become the expected type's zero and are listed in
  `missing_globals` (4 across the campaign).
- **Animation names**: a string literal right after an animation graph
  literal (`custom_animation`, `scenery_animation_start`, ...) is hooked
  with `h1_type` `compile::ANIMATION_NAME` and the token
  `<graph>|<animation>`, so the converter can write the string ID of the
  converted animation's name.
- **Indexed values**: a cutscene flag, camera point, title or device group
  atom is an index the engine reads without a check (b40 faulted at
  `0x002a003f` in `object_teleport` on flag -1). `compile_with`'s
  `resolves(type, token)` says which will get a value; a builtin call with a
  direct argument of one of these types (`compile::INDEXED`) that will not
  is compiled as its type's zero and listed in `Compiled::unindexed`. The
  converter resolves these names without case, as HaloScript does (b30's
  `shaftA_control_teleflag`, d40's `enc7_7_L1`).
- **Cutscenes**: `compile_with(.., Cutscenes::Skip, ..)` compiles every call
  of the level's `cinematic_skip_start` script as `false` (Halo 1's path
  after a skip) and wakes a10's `fast_setup` where it wakes
  `tutorial_setup`; `Cutscenes::PlayShortPaths` keeps the cinematics and
  takes the short path; `compile` keeps `Cutscenes::Play`. Why, and what each
  level does: [campaigns.md](../game/campaigns.md) §9, Cutscenes.
- **References** are Halo 1 group and tag name pairs; the converter maps them.

`Compiled::install(&mut scnr_block, resolve_reference)` adds `map::build`
fixups to a Halo 2 scenario block: syntax nodes block `+0x238`, string data
`+0x1b0`, scripts `+0x1b8`, globals `+0x1c0`, references `+0x1c8` (group plus a
`TagId` fixup). `Compiled::scenario_bytes(base)` lays the same tables out as a
minimal scenario payload for `hs::Scenario::decode`.

Proof (`every_campaign_script_compiles_and_round_trips`): under both
policies, all ten maps compile, `hs::Scenario::decode` reads the tables back
with byte-for-byte codec symmetry, and every node is reached. Under `Stub`
they also pass `Scenario::validate` against Halo 2's XBE catalog (every
fixed-signature call's arity and argument types equal Halo 2's descriptor) and
`hs` decompiles them with a matching node count.

| Map | Halo 2 nodes | Builtin calls | Hooks | Stubbed calls |
|---|---|---|---|---|
| a10 | 15,473 | 4,439 | 2,817 | 830 |
| a30 | 10,832 | 3,350 | 1,419 | 583 |
| a50 | 13,891 | 4,304 | 1,873 | 466 |
| b30 | 4,212 | 1,295 | 540 | 156 |
| b40 | 9,560 | 2,987 | 1,544 | 409 |
| c10 | 8,813 | 2,679 | 1,427 | 278 |
| c20 | 8,213 | 2,642 | 920 | 284 |
| c40 | 7,311 | 2,214 | 1,152 | 234 |
| d20 | 7,207 | 2,226 | 938 | 204 |
| d40 | 12,287 | 3,635 | 1,904 | 430 |

Stubbed calls count outermost missing calls; a missing call's arguments go
with it, which is why the Halo 2 node counts are lower than Halo 1's. All
maps fit Halo 2's `0x9000`-node pool.

## Step 4: hooks in a converted level

`crates/h1campaign` resolves hooks with the encodings retail Halo 2
scripts use (V: every atom of these types in `05a_deltaapproach` and
`03a_oldmombasa`, printed by `h1campaign`'s `ai_and_name_literal_encoding`
survey): object, unit and vehicle names, `short`, `team` and
`game_difficulty` are `0xffff0000 | index`; trigger volumes and cutscene
flags the bare index; `sound` the tag's datum; `ai` a squad's index, a
squad group's `0x40000000 | index`, a squad's starting location
`0xc0000000 | squad << 16 | location`. Adapter defaults arrive as
`<builtin>:<value>` tokens. Counts for a30 are in
[campaigns.md](../game/campaigns.md) §9.

## Level data the scripts name

Script atoms of these types index scenario blocks, so the converter writes
Halo 1's blocks into Halo 2's in Halo 1's order and each atom keeps its
index. `h1scripts::level` reads and writes them; `h1scripts::seats` builds
the seat-mapping table. Offsets come from the surveys in
`crates/h1scripts/tests/survey.rs` (they print offsets, strides and counts
only): for every script atom of a type, the block whose element at the
atom's index holds the atom's token as its name.

| Data | Halo 1 `scnr` (stride, name) | Halo 2 `scnr` (stride, name) | Atom value (Halo 2) |
|---|---|---|---|
| cutscene flags | `0x4e4` (`0x5c`, `+4`); position `+0x24`, facing `+0x30` | `0x1e0` (`0x38`, `+4`); position `+0x24`, facing `+0x30` | index |
| camera points | `0x4f0` (`0x68`, `+4`); position `+0x28`, orientation `+0x34` | `0x1e8` (`0x40`, `+4`); position `+0x28`, orientation `+0x34` | index |
| titles | `0x4fc` (`0x60`, `+4`) | `0x1f0` (`0x24`, string ID `+0`) | index |
| device groups | `0x288` (`0x34`, `+0`); initial value `+0x20`, flags `+0x24` | `0xa0` (`0x28`, `+0`); same fields | datum `(0xe564 + i) << 16 \| i` |
| recorded animations | `0x36c` (`0x40`, `+0`); stream data reference `+0x2c` | `0x110` (`0x34`, `+0`); stream data reference `+0x2c` | index |
| seat mappings | (seat text in the script) | `0x228` (8: unit `vehi` datum, seat mask) | `count << 16 \| first` |

Evidence: Halo 1, a30 (every flag, camera point, title and device group
token at its index). Halo 2, 01b, 03a and 05a (the same, with the schema's
strides). Two of 03a's six camera points put the position at `+0x24`
instead; the two its scripts use (`e8_prediction`, `e1_prediction`) have it
at `+0x28` and the orientation after it, the layout written here. Titles:
the name string ID is the key into the level's text, which is not converted,
so a title shows nothing yet; bounds, colour and timing are this crate's own
defaults.

**Seats.** A unit's seats block is `vehi +0x1c8`, `0xb0` each, the label
string ID at `+4` (V: every `vehi` on 05a). A mapping element is a unit
definition and a mask of its seats; retail matches a seat when its label
starts with the script text (05a: `pelican_p` is the pelican's bits 1–10,
`pelican_p_l01` to `pelican_p_r05`). Halo 1 names roles on Halo 1's vehicles
(`driver`, `gunner`, `passenger`, `p-riderlf`), and the converted level
drives Halo 2's vehicles, so `seats::h2_label_matches` maps roles to Halo
2's label suffixes: driver `_d`, gunner `_g`, passenger `_p`, rider or
passenger with a side `_p_l…`/`_p_r…`, cargo `_lc`; boarding seats (`_b_`)
never; other text by retail's prefix rule. One run of elements per text,
over every `vehi` in the converted set.

**Navpoints** take Halo 2's `default` (0): Halo 1's navpoint types
(`default_red`, ...) are not mapped to Halo 2's. **Tags** (looping sounds,
effects, damage, animation graphs) resolve by the Halo 1 tag's name among
the converted tags of the Halo 2 group. `lsnd` converts with
`h1sounds::looping` (`read_looping`, `install_looping`; the multiplayer
converter uses it for background sounds), but `h1campaign` doesn't call it
yet, so looping sounds stay `none`.

**AI.** Most `ai` hooks left were squads `h1ai` dropped for having no
starting locations. Retail Halo 2 keeps such squads (06b: 28 of 170;
`h1campaign`'s `squad_flag_bits` survey), so `h1ai` now keeps them, with no
locations, no actors and no order, and `h1campaign` keeps only those the
scripts name. A level that then overflows tag memory is converted again
without them, and if still over, without the level data and seat table
above too, and if still over, without Halo 1's textures on the objects'
models (`h1objects`' shader copies and bitmaps; `h1campaign::convert`).
b40, which sat at the bound, takes the third step since the models gained
those textures (tag memory `0xc23000` of `0xc24000`), so its hooks are as
before and its objects draw with the base's model shader.
Most `ai` hooks still left name Halo 1 platoons (`…_toon`) or encounters
used as squads; Halo 2 has no platoons.

## Open questions

- The `unit_exit_vehicle` mode 1 default is not checked against Halo 2's
  evaluator (it is a hook, so the converter can change it). `ai_place`
  no longer passes a count (above).
- `compile::add_startup_placements` appends a startup script of one-argument
  `ai_place` calls, each squad an `ai` hook: h1campaign places Halo 1's
  initially created encounters with it (h1-ai.md).
- Halo 2's string literals are fixed up at load from `+0xc`; the compiled
  string atoms put the same offset in the data word. Untested in the engine.
- Salts: `0x8000 | slot`. The engine rebuilds occupancy from salts at load;
  whether it needs any particular salt sequence is not established.
- Names: Halo 2's 521 unnamed builtins may cover some "missing" Halo 1 names.
  Aligning the two tables by signature between named anchors could name
  some of them; not attempted.
- Halo 1 encounter semantics (`ai` values name encounters, squads or
  platoons) need the AI lane's mapping before AI hooks can be filled.

## Use

```sh
H1_ISO=disc/halo1.iso H2_ISO=disc/halo2.iso \
  cargo test -p h1scripts --test disc -- --ignored --nocapture
cargo test -p h1scripts
```
