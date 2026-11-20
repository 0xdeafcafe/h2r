Evidence for [native-renderer.md](../native-renderer.md).

## The all-native push (2026-11-19/20)

### Method

Headless `03a_oldmombasa` (`H2_LEVEL=03a_oldmombasa`), `H2_BOOT_GFX`
frames every 500 presents to present 5000, `H2_FRAME_STATS=1` from
present 3000, `--slices 10000000`. Without the higher slice limit the
run stops at about present 2500. The machine's load varied from 40 to
180 and moved D3D fps from 35 down to 23, so each D3D run is made back
to back with its native run under the same load.

**All native** meant every per-stage switch of the time on (world,
world programs, world layers, objects, transparents, decals, particles,
lights, gels, shadows, fog, water, first person, residual, post and UI).
Those switches are gone: the default is all native, and
`impl.render.native=off` is the D3D side.

### Pixels

Every captured frame — 1, 500 … 4500, cutscene and gameplay — is
identical to D3D after round 3: the max channel difference is 6 at
frame 500 and 0 elsewhere. Before round 3, one explosion frame (2000)
differed by 44 px.

### Gameplay fps by round

Presents 3000–5000, D3D against all native, same load:

| Round | D3D fps | All-native fps |
|---|---|---|
| before | 35.0 | 10.7 |
| round 1 | — | 4.75, worse than before |
| round 2 | 23.8 | 14.4 |
| round 3 | 25.6 | 15.4 average |
| round 4 | — | TODO: pending measurement |

Round 3's average hides two regimes: native is slower in a
mid-gameplay stretch (12–14 fps for about 15 s) and faster at the end
(29.3 against 24.3).

### Root causes found and fixed

- native-world prepared and uploaded every part per view. Now only the
  view's parts, with cached bindings and pipelines and queued draws,
  giving 1 worker round trip per frame instead of 12.
- The 156 "lights" worker round trips per frame were native shadows
  counted as lights. Shadows now queue with an exact D3D fallback and
  batch adjacent ranges.
- Objects and lights made the game thread wait on the render thread
  before masking guest items (render-wait 0 → 10–20 ms). They now queue
  with an ordered D3D fallback.
- The post glow never committed: first render-state setters counted as
  dependencies, then the plan matched the wrong surface field (the
  guest binds the embedded surface header), then a resource check
  refused a clean RTT alias.
- The per-copy `push_error_scope` + `block_on` validation moved to
  resource creation.

### Why native can lose despite less work

In the round 3 late window, native cut game-thread cpu from 34.7 to
23.9 ms and encode from 10.9 to 3.9 ms. But any synchronous admission
makes the game thread wait on the render thread, and per-pass
load/store costs (73 shadow passes per frame in round 3) make the
render thread the bottleneck.

### Status

Every stage has defaulted on since this push (see the top of this
document); `impl.render.native=off` is the D3D side of any A/B.

### Native cost (2026-11-20)

On, against off, Old Mombasa gameplay at 4x on the M3: 32.35 against
34.34 fps. Submit was 1.6 ms higher, d3d 0.9 ms and decode 0.3 ms, and
p99 49.7 against 39.5 ms. Objects counted 25 cache misses a frame of 49
draws.

Causes found in the code (Code):

- Objects and first person recorded render bundles and ran them in a
  render pass of their own ("native draw run"). Every run ended the
  merged D3D pass, then loaded and stored colour, depth and stencil again
  at 4x. First person used a private command buffer, which also closed
  the open encoder.
- The object bundle cache keyed each bundle on its uniform bind group and
  offsets. Those come from the submission's upload arena, which rotates
  through the pool, so about half the lookups missed and a new bundle was
  encoded and validated on the game thread. Every objects recording was
  also validated by encoding it into a throwaway encoder.
- On the render thread, an objects reservation pins its attachments once
  per run. But the previous run's commit was queued as an ordinary
  command, which counted as intervening work, so every run paid a
  synchronous worker round trip.

Changes:

- `LitObjects::draws` builds ordered `gfx::RecordedDraw` packets. They use
  persistent pipelines and texture groups, plus the submission's resident
  uniform run for the changed constant rows. Objects (`ObjectsPass`) and
  first person (`FirstPersonPass`) return them from `record_draws`, so they
  join the D3D pass already open on the same attachments, as world layers
  and the UI do. There is no extra pass, command buffer or bundle.
  `cache_hits` now means that the pipeline and texture group were
  resident.
- `ThreadedBackend::commit_external` queues packet and bundle commits with
  `push_external`, so the commit does not invalidate a pinned run.
  Adjacent object runs share one round trip. Standalone command buffers
  still invalidate it.
- New counters: `native_<stage>_command_buffers` and the frame
  `command_buffers` above.

Expected: `native_objects_reference_passes` and `display_passes`, and
`native_first_person_command_buffers`, fall to about 0. `passes` and
`command_buffers` fall by the number of object runs. `native_objects_cache_misses`
goes to 0 after warm-up, `native_objects_worker_round_trips` falls to
about the number of non-adjacent object runs, and `native_objects_encode_ns`
and `native_objects_prepare_ns` fall.

Second round (same day, code only, not measured yet):

- World: `GpuWorld::selection_draws` returns packets in the open D3D pass,
  adjacent triangle lists joined (a packet may cover several guest
  ranges). Its view rows go as `ExternalDraws::writes`: the backend writes
  them at the front of the submission when it holds no other bytes for
  those ranges and nothing writes them in draw order, and otherwise copies
  them in order before the packets (`ordered_writes`). The world pass
  takes the swap's batch key, so the render thread pins its attachments
  instead of a round trip. The fixed stage still records standalone.
- Fog and water: packets with their uniform blocks as writes; water that
  samples the view copies it in the open encoder (`record_encoded`).
  Decals, particles and gels: packets, per-draw data in the submission
  arena, pipelines and groups cached.
- Post (screen effects and glow) draw into targets of their own:
  `ExternalPass::record_encoded` issues their passes into the frame's open
  encoder at commit, validated once on a scratch encoder at prepare. No
  command buffer of their own.
- Lights admitted at `RunPass`: packets with resident uniforms and cached
  pipelines and groups, in place of a render bundle per draw each frame in
  a run pass of their own. `GfxBackend::native_bundle_passes` stays 0 for
  lights; the native-lights summary no longer prints it.

Expected: `native_<stage>_command_buffers` about 0 for world, fog, water,
decals, particles, gels and post; `native_world_worker_round_trips` and
fog's about 0; `native_lights_reference_passes` and `display_passes` count
only the D3D passes the packets join; `command_buffers` and `passes` fall
by the stages' former runs; `ordered_writes` near the number of extra
views per submission.

Still open:

- Side passes at a render scale above 1 stay undeferred. Not exact yet:
  in single-copy mode the reference backend still records guest-size
  intermediates and reads that the view samples, and the reverse. Deferring
  needs either one ordered command stream shared by the two backends or a
  flush of the other backend's deferred side work wherever
  `ScaledBackend` switches backends.
- Walk fallbacks and refused lights (needs research).
- The object walk still returns about 14 objects a frame to D3D as
  `walk_unsupported`. Their parts use stencil marking, conditional
  stream ranges or the auxiliary state toggle (`object_walk::read_part`).
  This is fallback, not drawing twice: those objects are not drawn
  natively.
