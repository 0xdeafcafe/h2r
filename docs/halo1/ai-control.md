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
| `ai_retreat`, `ai_maneuver`, `ai_maneuver_enable` | The two explicit requests share Halo 1 callee `0x001404e0`. The converter retains the requested maneuver destinations at squad `+0x4c` where the squad budget permits and snapshots the original actors before issuing native migrations, including reciprocal and cyclic routes. Each platoon with a scripted enable switch gets one boolean HaloScript global, initially enabled, to gate its explicit migrations. |
| `ai_follow_target_players`, `ai_follow_target_ai`, `ai_follow_target_disable` | Halo 1's setters (`0x000efd90` to `0x000eff19`) update the encounter selected by the low word. Halo 2 order flag `0x10` chooses the closest player (`0x00204489`), and `0x20` follows the order of squad `+0x4e` (`0x00204f09`). The four retail Halo 1 AI-target calls name a squad in the same encounter. The leader's own follow link clears before its peers follow it. Disabling follow selects each squad's ordinary order. Follow orders default to 4 m, as in several of Halo 2's campaign orders. |
| `ai_follow_distance` | A single finite literal before the encounter's only follow call in the same `begin` becomes that order's radius. This is the c20 campaign shape. Halo 1 `0x000efd71` stores that literal on the encounter. Halo 2 `0x00204668` reads the radius from order `+0x50`. |
| `ai_migrate` | Halo 2's 308 accepts encounter sources and bare squad destinations. It rejects group destinations at `0x002a54cc`. The converter resolves literal Halo 1 encounter and platoon destinations per source squad, preferring compatible variants and character types as in the selector at `0x001402a0`. Literal platoon sources expand to their member squads. |
| `ai_migrate_and_speak` | Moves the selected actors and requests the native combat vocalization at `0x0020ba60` for each changed actor. Halo 1 chooses advance or retreat in `0x00141fc0` and speaks after moving at `0x00141d0c`. Halo 2's `adlg` events 111 and 114 resolve through the index at `+0x1c` to patterns 138 (`newordr_advance`) and 144 (`newordr_retreat`). The native system selects an available voice line and applies its dialogue gate. |
| `ai_migrate_by_unit` | Evaluates the HaloScript object list, collects the actors of its units and vehicles’ riders, and migrates each with Halo 2 `0x001e3400`. Group destinations prefer a squad of the same actor type. |
| `ai_free`, `ai_free_units` | Call Halo 2's actor migration routine `0x001e3400` with no destination. It unlinks the old squad at `0x00203ed0` and puts the actor on the free-actor list at `0x00203f60`. A unit-list argument is evaluated by the VM and traversed through the native list pool. |
| `ai_attach_free` | Compiles the actor-variant token to a rebuilt `char` at the original tag path. Source `actr` perception and `actv` vitality and weapon tuning populate the Halo 2 character records. The hook uses Halo 2 `0x001e0160` with no squad to attach that character to the supplied unit. The routine replaces any previous actor at `0x001e0d98` and puts the new actor on the free list. Halo 1's allocator and free-list insertion are `0x0014b810` and `0x0013bdd0`. |
| `ai_teleport_to_starting_location` and its unsupported-only form | Select a starting pose in the actor's current squad and set the unit's transform with Halo 2 `0x000b7430`, then wake physics at `0x000b7360`. Halo 1 `0x00141510` checks support before the conditional move. The port uses Halo 2's biped support flag and seat attachment. When a start has no name, it selects the nearest available start in the squad. |
| `ai_grenades` | Binds native 280. Halo 2 `0x002a4da1` writes the boolean at AI globals `+0x340`, read by grenade logic at `0x001ae48b` and `0x001e2d69`. Halo 1 `0x000f086d` writes its AI grenade gate. |
| `ai_spawn_actor` | Calls the two-argument overload of `ai_place` (291, `0x002a5010`) with count 1. For a literal platoon, the first member is selected as in Halo 1 `0x001422c0`. Ordinary `ai_place` uses the one-argument overload (290). |
| `ai_go_to_vehicle` | Calls the seat-specific form of Halo 2's `ai_vehicle_enter` (343, `0x002a5ec0`), with native pathing and seat assignment through `0x00274a50`. A string literal resolves through the existing scenario seat mapping. Rebuilt transport labels such as `p_riderlm` keep their row and side. The two-argument overload (344) selects seats without the mapping. |
| `ai_vehicle_encounter`, `ai_vehicle_enterable_*`, `ai_going_to_vehicle` | Use salted vehicle handles and persistent HaloScript globals for the registry. Halo 1 `0x00140e70` assigns the vehicle and moves its occupants. Halo 1 `0x0013f2d0` defaults boarding distance to 8 m and the setters add team, actor-type or squad criteria. The continuous control uses Halo 2 `0x00274a50` to pick seats and request native boarding. It moves riders to the assigned squad with `0x001e3400`. The count reads queued type `0x4c` commands and their vehicle dwords as written at `0x00274d47` and copied to the actor queue at `0x001a81de`. The converted registry holds 96 simultaneous vehicle records on maps using the setters. |
| `ai_link_activation` | Links the encounter groups by direction. A continuous HaloScript call reads each native squad's active bit (`0x00203ca4`), follows links starting from active encounters and calls the native activator `0x00203bf0` for linked squads. The Halo 1 setter `0x0013a680` permits three links per source. |
| `ai_try_to_fight`, `ai_try_to_fight_player`, `ai_try_to_fight_nothing` | A salted actor handle owns the saved preference, following Halo 1 `0x0013f710`–`0x0013f7c9`. It adds the stock 10-point preferred-target bonus to eligible candidates in Halo 2's scorer `0x00265d30`. Clearing the preference returns the actor to ordinary target selection. |
| `ai_playfight` | The encounter switch applies Halo 1's 0.3 outgoing damage multiplier (`0x001475f5`, called for the damage owner at `0x0009575c`) in the corresponding Halo 2 actor damage modifier at `0x001c8440`. It preserves the native damage and hit-report pipeline. |
| `ai_timer_expire` | Clears the native squad activation countdown read at `0x0020207a`. For literal squads with Halo 1 flag `0x10`, the call also invokes player awareness, as at Halo 1 `0x0013e25a`. |
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

## Open

The source playfight firing and aiming adjustments at `0x001616ea`
and `0x00162492` still need Halo 2 equivalents. Halo 1's script-held
initial combat delay (squad flag `0x8`) is separate from Halo 2's timer.
Computed `ai_timer_expire` selections still need their source flag data.
The remaining migration work covers `ai_automatic_migration_target`,
and automatic platoon triggers.

Other unsupported forms include a dynamic follow radius, following an
arbitrary unit or another encounter, `ai_go_to_vehicle_override`,
and a maneuver with a stripped destination.
The follow, maneuver and order conversions use Halo 2's native behavior
for the resulting squads. In particular, selecting an attack, defend or
follow order replaces the previous order. Halo 1 could keep follow state
independently of its attack and defend state.

Conversations and command lists have separate converter lanes.

## Tests

```sh
cargo test -p h1ai --test convert
cargo test -p hs --lib h1_ai_slots
cargo test -p h1scripts --lib ai_control
cargo test -p boot --lib h1_ai_control
cargo test -p h1campaign --lib actor_control
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1scripts --test disc encounter_controls_emit_live_evaluators_and_native_orders -- --ignored
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc every_level_s_scripts_run_on_the_retail_interpreter -- --ignored
H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc migration_speech_events_use_retail_order_patterns -- --ignored
```
