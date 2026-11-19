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
| `ai_retreat`, `ai_maneuver`, `ai_maneuver_enable` | The two explicit requests share Halo 1 callee `0x001404e0`. The converter retains the requested maneuver destinations at squad `+0x4c` where the squad budget permits and emits native migrations. Each platoon with a scripted enable switch gets one boolean HaloScript global, initially enabled, to gate its explicit migrations. |
| `ai_follow_target_players`, `ai_follow_target_ai`, `ai_follow_target_disable` | Halo 1's setters (`0x000efd90` to `0x000eff19`) update the encounter selected by the low word. Halo 2 order flag `0x10` chooses the closest player (`0x00204489`), and `0x20` follows the order of squad `+0x4e` (`0x00204f09`). The four retail Halo 1 AI-target calls name a squad in the same encounter. The leader's own follow link clears before its peers follow it. Disabling follow selects each squad's ordinary order. Follow orders default to 4 m, as in several of Halo 2's campaign orders. |
| `ai_follow_distance` | A single finite literal before the encounter's only follow call in the same `begin` becomes that order's radius. This is the c20 campaign shape. Halo 1 `0x000efd71` stores that literal on the encounter. Halo 2 `0x00204668` reads the radius from order `+0x50`. |
| `ai_migrate` | Halo 2's 308 accepts encounter sources and bare squad destinations. It rejects group destinations at `0x002a54cc`. The converter resolves literal Halo 1 encounter and platoon destinations per source squad, preferring compatible variants and character types as in the selector at `0x001402a0`. Literal platoon sources expand to their member squads. |
| `ai_migrate_by_unit` | The retail Halo 1 uses name a single unit. Compiles to `ai_migrate (object_get_ai unit) destination`. Halo 2 `0x00272ff0` returns an actor reference with kind 2, which the migration routine iterates at `0x00274155`. |
| `ai_spawn_actor` | Calls the two-argument overload of `ai_place` (291, `0x002a5010`) with count 1. For a literal platoon, the first member is selected as in Halo 1 `0x001422c0`. Ordinary `ai_place` uses the one-argument overload (290). |
| `ai_go_to_vehicle` | Calls the seat-specific form of Halo 2's `ai_vehicle_enter` (343, `0x002a5ec0`), with native pathing and seat assignment through `0x00274a50`. A string literal resolves through the existing scenario seat mapping. Rebuilt transport labels such as `p_riderlm` keep their row and side. The two-argument overload (344) selects seats without the mapping. |
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

The awareness hook is limited to converted campaign maps. The other
opcodes sharing `0x002ab6c0` go through their original evaluator. The
hook keeps its pending arguments and return on the retail interpreter's
frame, with no host state across game ticks. Its unit tests use
synthetic guest bytes and a guest call oracle. Static disc analysis and
these tests do not require an Agent API method.

## Open

The remaining campaign calls need runtime behavior for free actors
(`ai_free`, `ai_free_units`, `ai_attach_free`), vehicle encounter
assignment and opportunistic boarding (`ai_vehicle_encounter`,
`ai_vehicle_enterable_distance`, `ai_vehicle_enterable_actor_type`,
`ai_vehicle_enterable_disable`, `ai_going_to_vehicle`), automatic
migration targeting (`ai_automatic_migration_target`), linked activation
(`ai_link_activation`), `ai_playfight`, actor-local target preferences
(`ai_try_to_fight`, `ai_try_to_fight_player`, `ai_try_to_fight_nothing`),
`ai_timer_expire`, `ai_migrate_and_speak`, both starting-location
teleport forms and `ai_grenades`. The explicit maneuver path also leaves
Halo 1's automatic platoon triggers open.

Other unsupported forms include a dynamic follow radius, following an
arbitrary unit or another encounter, `ai_go_to_vehicle_override`,
migrating a computed object list,
and a maneuver with a stripped destination or cyclic migration edges.
The follow, maneuver and order conversions use Halo 2's native behavior
for the resulting squads. In particular, selecting an attack, defend or
follow order replaces the previous order. Halo 1 could keep follow state
independently of its attack and defend state.

Conversations and command lists have separate converter lanes.

## Tests

```sh
cargo test -p h1ai --test convert
cargo test -p h1scripts --lib ai_control
cargo test -p boot --lib h1_ai_control
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1scripts --test disc encounter_controls_emit_live_evaluators_and_native_orders -- --ignored
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test -p h1campaign --test disc every_level_s_scripts_run_on_the_retail_interpreter -- --ignored
```
