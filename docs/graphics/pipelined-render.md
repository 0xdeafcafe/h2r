# Pipelined rendering

Status: Built. The worker runs the frame end (device work and present) while the next tick runs; the whole-frame design that preceded it is evidence in the note.
Code: `crates/hooking/hooks/src/pipeline_render.rs` (`PipelineRender`), `pipeline_scene.rs` (scene segments, animation uses), `pipeline_snapshot.rs` (COW view and access trap); wired by `d3d_glue::pipeline_render` in `crates/apps/boot/src/d3d_glue.rs`.
Tests: `crates/hooking/hooks/tests/pipeline_render.rs`; `ordered_frame_owners_match_the_serial_tick_hash_while_drawing` in `crates/apps/boot/src/sim_glue/state_hash_pipeline_test.rs`.
Switch: `impl.render.pipelined_frame` (off), listed in [switches.md](../runtime/switches.md). It is independent of `impl.render.native`.

## Tick ordering and hash parity

For an eligible world frame, the game thread drives the view loop and hands
generated scene traversals to the render worker during preparation. Candidates
include the game and alternate views, scene passes and graph animation data
resolution inside HUD or UI calls. Existing native entry hooks run on their
original thread. At `render_loop::views::END_A`, the game thread runs the
frame wait once and launches the frame-end effects, device work and present
on the worker. Tick N+1 can run during that final submission.

| Tick-visible state | Publication before tick N+1 |
|---|---|
| Particles and effects | The original per-view update at `0x00174420` writes system visibility and location frame stamps on live RAM. Split-screen, culling and repeated visits retain their guest order. |
| RNG | Particle draws advance the effects word and object updates advance the game word in the live traversal. The tick starts with both seeds left by the last view. |
| Animation cache | A settled hit runs against frozen payloads. The game thread replays its use stamp at slot offset `+0x14`. Missing or pending payloads go through the original game-thread getter, including eviction, I/O and completion. |
| View and object state | View frame counters, visit marks and per-object lighting allocations occur during traversal. The live current-view context is cleared before the tick. |

The worker queues settled uses in visit order, including repeated uses of
the same slot. A miss ends the batch with the getter's registers and its
original stack argument. Before issuing that call the game thread publishes
the prefix and replays its stamp writes. The worker takes a fresh COW view
after the live getter returns. An entry with an outstanding I/O handle takes
this path even when its completion byte is set, because the getter still
has cleanup to do. A batch holds at most 256 settled uses.

A scene stops at a host override, particle update or cache-owner entry and
returns control to the game thread at that call. A changed observed page or
incomplete footprint discards the unpublished segment and finishes the frame
serially. The game thread waits for each scene segment during preparation.
Tick overlap begins when the prepared frame reaches the final submission.

The boot test `ordered_frame_owners_match_the_serial_tick_hash_while_drawing`
holds a worker draw open while a synthetic tick consumes particle state,
advances both RNGs and evicts both animation slots. It compares the slot and
stamp bytes, the ordered I/O digest, the drawn payload and the production
`state_hashes_v1` gameplay digest with the serial sequence. The feature rig
also covers consecutive prepared frames and a serial fallback.

## Snapshot coverage

The worker has a COW view of the 64 MiB guest address space, including the
dedicated guest stack on which the game thread ran the view loop. The
handoff checks the caller's return PC, stack depth and outstanding Rust
continuations. A different frame path or an open view debug bracket finishes
serially. This avoids moving thread-local cleanup or borrowing the simulation
stack while a tick is using it.

The worker's access trap records which pages it used. A change in virtual
mapping, backing offset or permissions selects the original serial path
(`snapshot_mapping`). An incomplete trace selects `snapshot_trace`.
The game thread rechecks coverage and outstanding I/O after frame preparation,
at the snapshot boundary. If eligibility changed, it finishes the pending
callee on live memory. Preparation, requests and the wait each run once.

`render.stats.pipeline` reports serial reasons, launches, completions and
`overlap_ms`, the cumulative worker time before the game thread joins.
`animation_uses` counts settled stamps published in batches,
`animation_owner_calls` counts suspended gets serviced on the game thread
and `scene_retries` counts discarded scene segments.
`sim_joins` is retained for telemetry compatibility and stays zero. A
game-thread device call or the next frame still joins when required.
Overlap can include time when the game thread is descheduled. The current
split has no measured speedup. For gameplay parity, compare the complete
`state_hashes_v1` tick series from matched headless runs with `-host-light`
and the switch explicitly off and on.

## Related

- [notes/pipelined-render.md](notes/pipelined-render.md): the whole-frame design, the `render-census` results, the prototype and its measurements.
- [render-thread.md](render-thread.md): the backend render thread, which stays as it is.
- [render-loop.md](render-loop.md) and [../engine/feature-engine.md](../engine/feature-engine.md): the front-end's structure and the native route.
- [../runtime/snapshots.md](../runtime/snapshots.md): quiescence.
