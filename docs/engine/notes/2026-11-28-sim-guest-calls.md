# The native sim's guest calls

2026-11-28, base `5c0c88684`. Every `GuestCall::new`, `call_guest` and
`calls_guest` in `crates/rewrites/sim-bridge`, `crates/rewrites/engine`
and `crates/rewrites/sim-*`, read with
`git grep -n 'call_guest\|GuestCall::new\|calls_guest'` (88 lines) and
`h2 disasm`. The plan is [sim-rewrite.md](../sim-rewrite.md).

A guest call goes to a guest address. It lands on lifted code unless a
Rust body owns that address (`sim_objects::BODIES`, `sim_physics::PORTS`,
`sim_physics::PHYSICS_PORTS`, `sim_seats`); the **Lands on** column says
which.

## Per tick, by how often a frame hits them

| Site | Target | What it does | Lands on |
|---|---|---|---|
| `eng/sim_tick.rs:417` | `3212d6` | CRT `_controlfp(0x9001f, 0xfffff)`, the x87 control word | lifted |
| `eng/sim_tick.rs:417` | `682e0` | the update record: zero `0x4048` bytes, gather it (`69260`), flag a mismatch at `[4cf77c]+0x2c` | lifted |
| `eng/sim_tick.rs:417` | `683a0` | `SIM_UPDATE_APPLY`: the record's actions (theater and co-op hook it) | lifted |
| `eng/sim_tick.rs:417` | `684e0` | the record's close; may clear the run flag | lifted |
| `eng/sim_tick.rs:417` | `14d3a0` | phase 1, `players_before` | lifted ([players note](2026-11-28-players-phases-map.md)) |
| `eng/sim_tick.rs:417` | `138db0`, `1fb510`, `20b660`, `195ed0`, `138c10` | phases 2, 4, 5, 7, 9 | lifted |
| `eng/sim_tick.rs:417` | `15c150` | phase 6, `ENGINE_UPDATE`, the game engine | lifted |
| `eng/sim_tick.rs:417` | `b6f10` | phase 10, objects before physics | Rust (`sim_objects::walks::pre`) |
| `eng/sim_tick.rs:417` | `1c3d30` | phase 11, `WORLD_PHYSICS` | lifted shell; the Havok step, collide, solve and allocator are Rust ports |
| `eng/sim_tick.rs:417` | `b7060` | phase 12, objects after physics: every object's update | Rust (`walks::post`; unit, weapon, vehicle, item and damage bodies under it) |
| `eng/sim_tick.rs:417` | `b7150` | phase 13, objects final | Rust (`walks::finalize`) |
| `eng/sim_tick.rs:417` | `2268e0` | phase 14 | lifted |
| `eng/sim_tick.rs:417` | `c1400` | phase 16, the light walk | Rust (`lights::walk`) |
| `eng/sim_tick.rs:417` | `14d8b0` | phase 18, `players_after` | lifted ([players note](2026-11-28-players-phases-map.md)) |
| `eng/sim_tick.rs:417` | `1df500`, `138e40`, `138eb0`, `226050`, `13c680`, `123430`, `225730`, `153950` | phases 19 to 25, 27 | lifted |
| `eng/sim_tick.rs:417` | `165dc1`, `1917b2` | phases 26 and 28, first person and HUD (presentation) | lifted; the first-person pose and filters are Rust ports |
| `eng/ai_update.rs:111` | `1c81c0`, `201c80`, `268510`, `1e2f50`, `20fb30`, `293830` | the six AI systems | lifted ([AI function list](2026-11-28-ai-function-list.md)) |
| `eng/post_game.rs:165` | `15c550` | the round-state step: returns unless `G+0xc04` differs from the state `G+0x6c` | **ported**: `engine::round_state::run`; a change still calls the leaves below |
| `eng/hs_update.rs:106` | `1deed0` | the object-list sweep: frees unreferenced lists and their node chains | **ported**: `engine::hs_object_lists::sweep` |
| `eng/hs_update.rs:103` | `209850` | `HS_THREAD_EXECUTE`, once per runnable script thread | lifted (the Rust VM in `crates/formats/hs` is differential-only) |
| `eng/tick_poses.rs:130` | `bd090` | `OBJECT_NODE_MATRICES`, once per object still dirty at the tick's end | Rust (`sim_objects::pose::node_matrices`) |
| `sb/sim_frame.rs:427` | kernel `NtYieldExecution` (238) | the skipped frame's wait gives up its slice | the HLE kernel, not game code |

## Rare: state changes and game end

| Site | Target | What it does | Lands on |
|---|---|---|---|
| `eng/ai_update.rs:111` | `1e3240` | the AI teardown, once after AI goes off | lifted |
| `eng/post_game.rs:165` | `15fe50` | round state change (`edx` 1 or 3) | lifted |
| `eng/post_game.rs:165` | `15ba00` | the last round's end | lifted |
| `eng/post_game.rs:165` | `a7810` | the game's end (`4`) | lifted |
| `eng/post_game.rs:165` | `15b330`, `15b3a0` | the score at the end, then its post | lifted |
| `eng/post_game.rs:165` | `15dd10` | the end's finish | lifted |
| `eng/post_game.rs:165` | `19eb90` | `GAME_EVENT_POST`, the `0x22`-byte end event | Rust (`sim_seats::event`) |
| `eng/post_game.rs:165` | `162030`, `15c6f0`, `196430`, `196390` | the post-game screen's flags and its quiet and loud steps | lifted |
| `eng/round_state.rs` | `a7810`, `162030`, `157ae0`, `19f240`, `b58c0`, the engine vtable `+0x90` | a round state change: the round's end, the screen flag, the round reset, the player iterator, the object flags, the engine's state virtual | lifted |

## Other lanes and not the sim

| Site | Lines | Owner |
|---|---|---|
| `eng/effects_phase.rs:72` (`1794e0`, `179730`, `1774f0`, `1792b0`) | 1 | lane n-sim-seats-fx |
| `sb/sim_effects_parts.rs` | 9 | lane n-sim-seats-fx |
| `sb/sim_sound.rs`, `sim_sound/entries.rs`, `start_layer.rs`, `voice_arm.rs` | 9 | lane n-sim-sound |
| `eng/sim_collision/mopp_ray/collector.rs`, its tests | 6 | lane n-sim-collision-b |
| `eng/render/light_gather/` (`frame.rs` 14, `object_shadows/caster.rs` 11, `passes.rs` 9, `first_person.rs` 6, `items.rs` 5, `object_shadows.rs` 4, `object_shadows/rows.rs` 3, `first_person_effects.rs` 2) | 54 | the renderer: draw-list gathering, not the tick |

## The frame wait's yield

`sb/sim_frame.rs:427` calls a kernel function token, not a game address.
The wait runs on a guest thread; the kernel switches threads only at a
transfer, so a yield from Rust has to leave through one. It goes when the
skipped frame stops waiting on the vblank counter, which is the frame
owner's design, not this table's.

## What a port needs

| Target | Size | Notes |
|---|---|---|
| `19f240` | the player iterator, `0x19f240..` | the players pool walk with the `+2 & 2` filter; `sim_seats::event` calls it too |
| `682e0`, `683a0`, `684e0` | the record build calls `69260`, the input and network gather | theater and co-op hook `683a0`; one port with them |
| `3212d6` | the CRT | the lifted CPU's x87 state; no `Cx` access to it |
| the lifted phases | whole subsystems | each needs its own map, as the AI and players notes give |
