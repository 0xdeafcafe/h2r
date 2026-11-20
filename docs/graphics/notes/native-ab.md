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


## Stage 1 and the BSP walk: A/B method

### Stage 1: the live A/B

The same 03a frame with and without the switch, E1 off, frame stats on.
What passes: the `native-world:` load line names `03a_oldmombasa`; the
views line shows every layer-3 view drawn natively; the frame shows the
world (no holes where the BSP was, objects and transparents in front of
and behind it as before); `cpu` and the `0x0002bed0` site (layer 3) drop
by most of the world's share of the 8.26 ms, and `encode` and `submit`
drop with the D3D draws that are gone, in proportion to the parts the
coverage line says are native.
A refused world reservation leaves the affected guest items in layer 3.
Use the stage's refusal reasons, per-copy range counts and invariant
failures to distinguish D3D fallback from native coverage.

The stage 1 look (base map times lightmap, on by default until
2026-11-19) was far from the D3D path on screen, lighting most of all:
no detail maps, no bump, none of the material's constants. The world is
now drawn only with each material's own translated pass, and a material
whose pass does not translate stays on the D3D path, so with any mix the
frame should match the D3D path's. Where it still differs, on native
surfaces only (the D3D ones are the game's own):

- **speckle or holes on world surfaces**, or objects showing through:
  depth. Layer 1 (D3D) wrote the world's depth and the native layer 3
  tests `LessEqual` against it, so the native W depth must be the D3D
  path's bit for bit; one step deeper fails the test;
- **a whole material too dark, too bright or flat**: its lightmap (the
  P8 page through its palette, bound at stage 3), its bump map (`P8Bump`
  through the device palette at stage 2; black walls with bright pillars
  was the bump sampled as its index) or a constant;
- **shimmer or blur at a distance**: sampling (the bump stage is 4x
  anisotropic; mip chains);
- **cut-out foliage or grates filled in or missing**: the pass's alpha
  test (03a's alpha-tested world passes pass below 255);
- **stencil-driven effects** after layer 3: the native pass writes the
  device's stencil as the layer starts and mode 2's mark on marked
  sections, and leaves views to D3D where an item switches the stencil
  and the layer does not start in mode 1 (no engine code after layer 3
  tests the stencil; a tag's or engine render-state list could, S);
- **decals, lights, fire and smoke gone, walls right**: the world drawn
  over the finished scene by a native pass in a layer 3 that is not the
  world's (the first-person render's; `skip::has_structure_items`).


The walk's A/B (2026-11, with a per-walk switch since removed) captured
the same Old Mombasa replay and present numbers with `H2_FRAME_STATS=1`
and `H2_FEATURES=render-timing`, changing only the walk. Use ordinary
render timing. Deep timing adds guest brackets that change the cost of the control path. Compare warmed per-frame `render`,
`scene_passes`, `d3d` and `cpu`, and count differing pixels on both target
copies at matching presents. Include views with placements, partial
visibility, fog, water and lights. Expected savings are in gathering,
material preparation, sort-slot construction and redundant D3D state
calls. The total number of late native draw admissions is not a count of
eliminated shader builds.



## Stage 2: the live A/B

Old Mombasa, E1 off, frame stats and `render-timing` on, the same spot
with the native renderer off and on . What passes:

- the load lines name the map's models, most as native; the refused ones
  give a reason (the run-time permutations: marines, some scenery);
- the views line shows every base-layer layer-3 view drawn natively, and
  objects and items per view in the tens;
- the frame shows every object in place and posed: bipeds animate (a
  T-pose or exploded limbs mean the `N · I` reading is wrong), held
  weapons sit in hands, vehicles and their turrets turn; flat-lit bases
  with the game's own self-illumination, specular and dynamic lights on
  top (those layers still draw through D3D);
- the first-person weapon and arms are unchanged (still D3D);
- the `0x0002bed0` site (layer 3) and `cpu` drop by the objects' share of
  layer 3's shader draws (below), `encode` and `submit` with them;
  `external_passes_dropped` stays 0 (a growth there is an object hole).

What this stage should remove: layer 3 is 8.26 ms on the census run,
about two thirds of it per-draw guest work in `0x00016b90` and the rest
D3D; objects are roughly a third to a half of layer 3's 1,882 D3D calls
on a populated 03a view (S: the split between world and object draws is
not measured; `render-census` can give it). So with the world already
native, expect about 2 to 4 ms off `0x0002bed0` and `cpu`, plus their
`encode`/`submit` share on the render thread. Layers 1, 5, 7, 8 and the
dynamic-light passes keep their object draws in this cut.



## Stage 2: object walk A/B method

Use the same 03a Old Mombasa firefight with at least six visible enemies,
the same camera, input and render scale, and a warmed map load for both
captures. Keep the other render switches fixed. Compare `H2_FEATURES=render-timing`
against `H2_FEATURES=render-timing,impl.render.native=off`.

Enable `H2_FRAME_STATS=1` for both. Collect the same window with
`profile.capture` through the [agent API](../../runtime/agent-api.md), or
stream it with `H2_FRAME_STATS_CSV`. Compare the `render` and `d3d`
phases, layer-3 and shader-begin time, object walk counters, and the same
view's images. Check biped animation, held weapons and vehicles through
fog, and the depth, stencil and draw order against the original path.
The `impl.render.native=off` capture is the all-D3D reference.
