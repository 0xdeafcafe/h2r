# Halo 1 encounter script controls

`h1scripts::aicontrol` binds native Halo 2 evaluators by opcode,
signature and XBE address. `h1ai::control` plans orders and migration
destinations from the original Halo 1 references before the campaign
converter strips empty squads. The compiled map uses Halo 2's
interpreter, squads, orders and seat mapping table.

Addresses below are from the owner's retail Xbox executables. The
survey reads the maps to learn builtin names and the arguments used
by the campaign. The test commands at the end cover the wire layouts
and translation. Runtime behavior still needs the integration build.

## Built

| Halo 1 control | Implementation and evidence |
|---|---|
| `ai_magically_see_encounter` | Existing mapping to Halo 2's `ai_magically_see` (304). The evaluator at `0x002a5360` calls `0x00273ac0`, which iterates actors and updates their matching perception records. |
| `ai_magically_see_unit` | Maps to `ai_magically_see_object` (305). Halo 2's callee `0x00273d30` accepts a bare squad index. `boot::h1_ai_control` enumerates member squads for a converted encounter and calls that routine for each. |
| `ai_magically_see_players` | Halo 1 `0x00141f10` walks the player records and reads each unit at `+0x34`. Converted scripts use descriptor 300, a discarded `(ai) -> void` entry unused in the Halo 2 map survey. On a Halo 1 campaign map the hook evaluates the argument through `0x00209d50`, walks Halo 2's occupied player records and sends live player units to the same native awareness routine as 305. Halo 2 updates its existing perception records. |
| `ai_attack`, `ai_defend` | Halo 1 `0x0013eee0` resolves an encounter to all its platoons and a squad to its own platoon. The compiler selects each member's attacking or defending order with `ai_set_orders` (333). The orders use the firing position groups at Halo 1 squad `+0x54` and `+0x60`. Halo 2 `0x00275cb0` dispatches the order to the given squad. |
| `ai_retreat`, `ai_maneuver`, `ai_maneuver_enable` | The two explicit requests share Halo 1 callee `0x001404e0` and arm the same persistent latch as the tag trigger. The enable switch gates movement independently of the latch (`0x00140460`). The converter retains destinations at squad `+0x4e` before other empty squads and snapshots actors before moving them. |
| Automatic platoon triggers | Read the two source conditions at platoon `+0x30` and `+0x3c`. The continuous script uses native squad counts and vitality, the nine thresholds from Halo 1 `0x0013af30`, and the specified platoon or whole encounter. At `0x0013b080` the maneuver latches before the combat-state check. Native order dispatch applies the opposite of the initial combat order, and native actor migration and dialogue handle triggered maneuvers. |
| `ai_follow_target_players`, `ai_follow_target_ai`, `ai_follow_target_unit`, `ai_follow_target_disable` | The converted registry stores the target separately from combat orders, as the encounter setters at Halo 1 `0x000efd90`–`0x000eff19` do. It resolves a live target each tick, including targets outside the encounter, and refreshes Halo 2's native area set at `0x00204450`. The collector uses squad `+0x80` as the target object. The area iterator at `0x00205039` reads that field while retaining the current combat order. Retained empty squads also get a starting order wherever the source gives them firing-position areas. |
| `ai_follow_distance` | The VM evaluates the radius each time the script sets it. The value applies to the next area calculation and to automatic migration. Halo 1 uses 2 m if the assigned distance is nonpositive (`0x0013cf2a`–`0x0013cf45`). |
| `ai_automatic_migration_target` | Preserves Halo 1 squad flag `0x20` and the separately enabled destination bit (setter `0x00140014`). The follow tick scores the original firing coordinates and the union of the three masks for the current platoon posture, following `0x0013caa0` and `0x0013cd80`. It uses the farthest occupied destination for hysteresis (`0x0013cf45`) and moves participants through Halo 2 `0x001e3400`. Halo 1 scores across BSPs, so an empty native area set no longer prevents destination selection. An encounter with no matching authored firing positions has no automatic destination. Disabling a destination still lets its current actors move elsewhere. |
| `ai_migrate` | Halo 2's 308 accepts encounter sources and bare squad destinations. It rejects group destinations at `0x002a54cc`. The converter resolves literal Halo 1 encounter and platoon destinations per source squad, preferring compatible variants and character types as in the selector at `0x001402a0`. Literal platoon sources expand to their member squads. |
| `ai_migrate_and_speak` | Moves the selected actors and requests the native combat vocalization at `0x0020ba60` for each changed actor. Halo 1 chooses advance or retreat in `0x00141fc0` and speaks after moving at `0x00141d0c`. Halo 2's `adlg` events 111 and 114 resolve through the index at `+0x1c` to patterns 138 (`newordr_advance`) and 144 (`newordr_retreat`). The native system selects an available voice line and applies its dialogue gate. |
| `ai_migrate_by_unit` | Evaluates the HaloScript object list, collects the actors of its units and vehicles’ riders, and migrates each with Halo 2 `0x001e3400`. Group destinations prefer a squad of the same actor type. |
| `ai_free`, `ai_free_units` | Call Halo 2's actor migration routine `0x001e3400` with no destination. It unlinks the old squad at `0x00203ed0` and puts the actor on the free-actor list at `0x00203f60`. A unit-list argument is evaluated by the VM and traversed through the native list pool. |
| `ai_attach_free` | Compiles the actor-variant token to a rebuilt `char` at the original tag path. Source `actr` perception and `actv` vitality and weapon tuning populate the Halo 2 character records. The hook uses Halo 2 `0x001e0160` with no squad to attach that character to the supplied unit. The routine replaces any previous actor at `0x001e0d98` and puts the new actor on the free list. Halo 1's allocator and free-list insertion are `0x0014b810` and `0x0013bdd0`. |
| `ai_teleport_to_starting_location` and its unsupported-only form | **Built.** Halo 1 `0x00141510` skips seated actors in the conditional form and casts a two-metre downward ray from 0.4 m above the unit. Halo 2 `0x001697c0` tests front-facing BSP geometry, `0x000b7430` sets the selected transform, and `0x000b7360` wakes physics. Authored required starts are consumed first. The ordinary pool resets only on exhaustion, following Halo 1 `0x0013b270` and `0x0013b500`. The Halo 2 spawn selector at `0x002039d0` shares these guest-backed reservations and honors the native wave's exclusions. Explicit starts consume the same bits at `0x00203780`. New maps encode both teleport forms in slot 300 with markers 38 and 39. |
| `ai_grenades` | Binds native 280. Halo 2 `0x002a4da1` writes the boolean at AI globals `+0x340`, read by grenade logic at `0x001ae48b` and `0x001e2d69`. Halo 1 `0x000f086d` writes its AI grenade gate. |
| `ai_spawn_actor` | Calls the two-argument overload of `ai_place` (291, `0x002a5010`) with count 1. For a literal platoon, the first member is selected as in Halo 1 `0x001422c0`. Ordinary `ai_place` uses the one-argument overload (290). |
| `ai_go_to_vehicle`, `ai_go_to_vehicle_override` | Both use Halo 2's native pathing and seat assignment at `0x00274a50`. Normal entry excludes actors already queued to board, matching Halo 1 `0x00141819`. Override uses retail 343, which writes the new transport command into slot 1 via `0x001a80e0` and invalidates the commands it replaces. The scenario seat mapping preserves labels such as the transport seat `p_riderlm`. |
| `ai_vehicle_encounter`, `ai_vehicle_enterable_*`, `ai_going_to_vehicle` | Use salted vehicle handles and persistent HaloScript globals for the registry. Halo 1 `0x00140e70` assigns the vehicle and moves its occupants. Halo 1 `0x0013f2d0` defaults boarding distance to 8 m and the setters add team, actor-type or squad criteria. The continuous control uses Halo 2 `0x00274a50` to pick seats and request native boarding. It moves riders to the assigned squad with `0x001e3400`. The count reads queued type `0x4c` commands and their vehicle dwords as written at `0x00274d47` and copied to the actor queue at `0x001a81de`. The converted registry holds 96 simultaneous vehicle records on maps using the setters. |
| `ai_link_activation` | Links the encounter groups by direction. A continuous HaloScript call reads each native squad's active bit (`0x00203ca4`), follows links starting from active encounters and calls the native activator `0x00203bf0` for linked squads. The Halo 1 setter `0x0013a680` permits three links per source. |
| `ai_try_to_fight`, `ai_try_to_fight_player`, `ai_try_to_fight_nothing` | A salted actor handle owns the saved preference, following Halo 1 `0x0013f710`–`0x0013f7c9`. It adds the stock 10-point preferred-target bonus to eligible candidates in Halo 2's scorer `0x00265d30`. Clearing the preference returns the actor to ordinary target selection. |
| `ai_playfight` | The encounter switch applies Halo 1's 0.3 outgoing damage multiplier (`0x001475f5`, called for the damage owner at `0x0009575c`) in the corresponding Halo 2 actor damage modifier at `0x001c8440`. The burst setup at Halo 2 `0x001fe620` applies Halo 1's 0.6 duration multiplier and doubles angular spread plus one degree (`0x00162436`, `0x00162492`). The pause at `0x001fed70` uses Halo 1's 1.7 multiplier from `0x001616ea`. The game retains its native firing and hit processing. |
| `ai_timer_expire` | **Built.** Compiles literal platoons to their members and accepts computed AI selections. Halo 1 `0x0013bcfa` truncates delays to 30 Hz ticks. The encounter's index selects its phase every 15 ticks (`0x0013eae0`). The first update with an alerted actor above its baseline latches the countdown (`0x0013db0c`). Later updates subtract 15 ticks. Bit 3 holds the timer until explicit expiry or an actor in the squad reaches combat level 5 (`0x00167bff`). Bit 4 reveals players on expiry (`0x0013e200`). Rebuilt maps use the Halo 1 awareness table on the focused prop. The movement hold operates at `0x001e4403`. |
| `ai_status` | Preserves the existing combat-status ladder and comparison lowering in `h1scripts::compile`. The translation survey now counts it as an adapter. The mapping of Halo 1 status 6 to Halo 2 visible status is described in [scripts.md](scripts.md). |
| `ai_living_fraction` | Binds Halo 2's 327. `0x002a59a0` divides living actors by spawned actors, using the counted result of `0x00273f30`. |
| `ai_detach`, `ai_braindead_by_unit` | Binds native 288 (`0x002a4f20`), which detaches the unit's actor, and 312 (`0x002a55f0`), which calls `0x00274f30` for the object list and its attached units. |
| `ai_force_active`, `ai_force_active_by_unit`, `ai_allow_dormant` | Binds 317 and 318. Their callees at `0x00275a50` and `0x00275ad0` set the squad and actor activation overrides. Halo 1's `ai_allow_dormant` callee `0x0013ff90` writes the inverse to the squad override, so the adapter calls 317 with `not allow`. |
| `ai_allegiance_remove`, `ai_allegiance_broken`, `ai_set_team` | Binds Halo 2's 310 (`0x002a5540`, allegiance removal), 332 (`0x002a5b70`, allegiance broken predicate) and 322 (`0x002a5850`, writes the actor team). |
| `ai_dialogue_triggers` | Calls `ai_dialogue_enable` (281), which writes Halo 2's AI dialogue gate at `0x002a4df1`. Halo 1 writes its dialogue gate at `0x000f08ad`. |
| `ai_reconnect` | The retail Halo 1 evaluator at `0x000ed620` returns void immediately. The compiler preserves that retail behavior for this editor operation. |

The scenario order fields `+0x4c` and `+0x4e` are distinct. Halo 2
`0x00202abe` uses `+0x4c` to index the static script block.
`0x00204f11` uses `+0x4e` to index the runtime squad table.
`h1ai::h2::Order` now exposes both fields, and the converter writes them
at those offsets.

## Builtin slot claims

The named `hs::h1_ai_slots::CLAIMS` table contains every slot this lane
hooks. All other builtin bindings use their retail evaluator directly.

| Slot | Retail descriptor | Use in converted maps |
|---|---|---|
| 241 | `(unit) -> short`, evaluator `0x002a4310` | A marked `ai_going_to_vehicle` call counts pending boarders. Unmarked calls still count the unit’s grenades. |
| 300 | `(ai) -> void`, discard evaluator `0x002ab6c0` | A function-head data word starting with `0x4831` selects the converted operation and argument types. An older converted map's unmarked call still gives player awareness. |
| 305 | `(ai, object) -> void`, evaluator `0x002a53b0` | Group-aware object awareness. |

The hook keeps pending arguments and results on the retail interpreter's
frame. The other opcodes sharing `0x002ab6c0` use their retail evaluator.
Static disc analysis and the synthetic guest tests need no running game
or Agent API method.

## Playfight and maneuver posture

**Built.** Halo 1 `0x00163c8c` skips the actor definition's time and
vitality tests when playfighting, then checks actor restrictions. Halo 2
presearch at `0x001b44d0` uses a character search block and a tracked
target. During playfight it skips that character prerequisite while
preserving the live target and behavior safety checks. Halo 2's usual
uncover rank at `0x001b84a0` has no matching character time or vitality
tests.

**Built.** Platoon flag `0x1` requests uncover once per actor migration.
Halo 1 queues it at `0x0014ff6d` and consumes it at `0x001673d0`.
Halo 2 behavior 35 uses native installer `0x001a80e0` and the uncover
record at `0x0047e908`. Halo 1 passes target `NONE` to its point
selector at `0x0016e070`. The marked Halo 2 uncover record therefore
uses the non-targeted firing-position planner at `0x001b1f70`. That
planner searches the active squad areas at `0x002605d0` and installs
the path at `0x002626b0`. The action follows that path until completion
and uses the stock uncover continuation at `0x001b8c60`.
This keeps the native Halo 2 point picker and pathfinder. Halo 1's exact
firing-position ranking and retry sequence have yet to be compared in play.

**Built.** Ordinary uncover takes its time and recent body damage requirements
from the source `actr` at `+0x2d8` and `+0x324`. The condition at
`0x00163c50` ignores a nonpositive requirement. A positive time requirement
blocks at combat level 7 and otherwise requires a full interval since the
last recorded recognition, when one exists. Damage must meet or exceed the
authored fraction. The retail a50 actor tags include the 0.5 s and 0.1
stealth elite pair and the 0 s and 0.1 sword elite pair. Character palette
entries with different requirements get distinct guest tag IDs even if
they share the same retail character payload. The aliases preserve direct
unit reads at `0x00200d11` as well as inherited properties. Scenario script
references carry the character IDs through fresh-map relocation.

The native rank, presearch and entry callbacks at `0x001b84a0`,
`0x001b44d0` and `0x001b8540` apply these requirements. Forced maneuvers
and playfights retain the Halo 1 threshold exception. The body damage
packet stores its applied normalized damage at `0x000d8e5e`. The
compatibility accumulator caps at one and begins decaying by 1/60 at
age 60 source ticks, following Halo 1 `0x000951cb` and `0x000966e6`.
The native recognition timestamp at `0x00265006` feeds the time check
in place of the same sample recorded at Halo 1 `0x0015582e`. The
focus hook at `0x00266640` preserves the timestamp across lost sight
and clears it on a change of target, following `0x00152e76`.

**Built.** Rebuilt timers use the source focused-prop awareness decision
from `0x0015080c` and its combat table at `0x001f025c`. Halo 2
`0x00267798` supplies the selected interest. The bridge reads the raw
relation, recognition, visibility probe, recent threat, memory and target
vitality, with authored guard baselines and an actor-local alert latch.
This preserves different treatment for neutral props, heard reports,
remembered targets, and live sight. Older converted maps without the new
`h1_ai_perception_state` marker retain their previous timer logic.

## Roster dependency and remaining parity work

**Built.** The converter uses `h1campaign::squad_roster::prepare`
from `97830757` for script-created starts. The campaign storage hook
uses the retail game-state allocator at `0x00200930`. Its squad array
has room for 384 `0x98`-byte records and a bitmap. The group array
keeps its 100 retail records and stays at its startup address. Before
`0x00200b30` the hook sets the squad header capacity for the loaded
scenario, with 335 as the retail minimum. The added allocation is part
of guest game state. A further 6 KiB after the group array holds the
salted actor alert, recognition and injury state, with one record for each of the 256
native actor slots (`0x001dfae0`). The added storage changes the native checkpoint layout, so
affected images and checkpoints need regeneration.

All named b40 destinations now have independent native squad indices,
including the 16 vehicle assignments and two infantry destinations beyond
the old 344-slot partition. Source ordinals remain distinct when Halo 2
truncates names. Live squads, maneuver destinations and explicit
migration targets get native records in that priority. Remaining
unnamed automatic destinations use reusable squad orders and keep their
source order and masks. Slot 300 marker 40 passes the offset of this data
in the scenario script section.

- The raw perception bridge depends on Halo 2 selecting an interest and
  recognizing the target. Source-only alert reports and forced combat
  levels at `0x0014e8a0` and `0x001506e7` still need corresponding events
  in the bridge. Exact timer parity for those cases remains open. The hold
  gates native throttle. Halo 1 also checks the hold at individual
  behavior call sites.
- A substitute character that upgrades through a Halo 2-only major tag can
  leave the authored uncover table. Rebuilt major variants with source
  tags retain their own requirements.

Conversations and command lists have separate converter lanes.

## Tests

```sh
cargo test -p h1ai --test convert
cargo test -p hs --lib h1_ai_slots
cargo test -p h1scripts --lib ai_control
cargo test -p boot --lib h1_ai_control
cargo test -p boot --lib h1_squad_storage
cargo test -p engine --lib campaign_squad_pool
cargo test -p h1campaign --lib actor_control
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1scripts --test disc encounter_controls_emit_live_evaluators_and_native_orders -- --ignored
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc every_level_s_scripts_run_on_the_retail_interpreter -- --ignored
H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc migration_speech_events_use_retail_order_patterns -- --ignored
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc retail_ai_control_hooks_match_spawn_follow_and_uncover_paths -- --ignored
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc retail_uncover_and_combat_timer_contact_rules -- --ignored
```
