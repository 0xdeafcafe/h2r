# Simulation rewrite: plan

Status: plan, 2026-11-20. Nothing here is built. It replaces the verdict in
[feature-engine.md](feature-engine.md) ("Subsystem map and ranking", rows
15-18) that objects, AI and physics "stay recompiled". The user asked on
2026-11-16 and 2026-11-17 for whole-subsystem rewrites, not per-function
patches. The per-function ports in `crates/engine/src/sim_native` and
`crates/engine/src/sim_physics` are bit-exact, register-level ports of single
functions. From here on they serve as **reference and test oracles**. They are
not the architecture.

Numbers come from [sim-budget.md](../perf/sim-budget.md) (Waterworks,
four local bots, 14.12 ms/tick, guest code 70.76% of the main thread) unless
another source is named. Addresses are guest virtual addresses already named
in the repo docs.

## Decision summary

- **State.** Guest-layout memory stays the persistent truth: the
  `0x3fe000`-byte checkpoint arena and the tag data. Rust owns the *work*:
  how a pass runs, its data layout, and its batching. Rust also owns the state
  that is not persistent truth:
  - **static** data built from tags and the map (decoded clips, collision
    BVHs);
  - **derived** caches that can be thrown away;
  - **transient** per-tick working sets, which a pass gathers from guest
    memory and scatters back to it;
  - **extension** state for new features, in a serialised side arena.

  This is option (c), hybrid, with one fixed rule rather than a separate
  choice per subsystem. Checkpoints, revert, saves, the arena hash, lockstep,
  films and snapshots keep working unchanged.
- **Exactness.** Product contract: the same build and switches give identical
  results on every platform. Matching retail bit for bit is not a product
  requirement, because no feature talks to a retail peer. It is the
  **verification bar** for every phase up to and including objects. With it,
  `h2 sim diff` against the lifted build proves a rewrite. Physics (Havok) is
  the first subsystem allowed to be deterministic but not exact, and only once
  the user signs it off.
- **Order.** Animation and pose first, then collision and spatial queries,
  then the object update, then Havok. A campaign census runs alongside
  phase 1, before the order after phase 2 is fixed.
- **Switches.** Each subsystem gets one switch, `impl.sim.native.<subsystem>`.
  `-impl.sim.native` turns them all off, because `Switches::implementation`
  falls back up the dotted key. Each new switch retires the per-function
  `impl.sim.native_*` switches it covers. Once its exit criteria hold, the
  switch is removed.

## 1. Goals, non-goals, done

**Goals.** Replace the costly sim families with Rust subsystems that own
their data layout and batch their work. Each must also benefit at least one
of the areas below.

| Area | "Done" means (measured) |
|---|---|
| Frame rate | Waterworks 4-bot sim **≤ 7.5 ms/tick** (from 14.12; the sim-budget target). Campaign: guest CPU **≤ 12 ms** of the frame where it is ~23 ms today at 4x (needs the phase 0 census to set per-level numbers). Every phase lands with its own ms/tick delta from three unprofiled medians. |
| Host cost | Matches per core **≥ 1.8×** today on the light profile (follows ms/tick). Incremental RSS per match process **≤ +32 MiB** for all Rust sim state. Static caches are shared read-only across processes on one machine (mmapped, content-addressed, in the user cache dir, never committed). No worker threads on hosts by default; parallel matches use the cores. |
| Theater and film | Films stay exact (same build). Rust state adds nothing to a film beyond the arena plus the extension arena. A keyframe is the arena plus the extension blob, ≤ 2 ms to take. Seek in a 10-minute film within 1 s, from keyframes every 10 s instead of restarting from boot. Poses come from animation state plus the shared clip store, so the web viewer and theater need no stored poses. |
| Stats | New consumers read typed state and events from a `sim-state` API, with no raw offsets. Kill and damage events come from the subsystem that applies them, not from memory scraping. |
| Engine expansion | New gameplay is written in Rust against the subsystem APIs, with its state in the extension arena and saved with checkpoints. First consumer: Halo 1 health ([halo1/health.md](../halo1/health.md), campaign.md §6). Converted Halo 1 content runs through the same Rust paths. |

**Non-goals.**

- Matching the Xbox's code shape, call graph or register state inside a
  subsystem.
- Retail-console interoperability. None exists today
  ([live.md](../live/live.md)), and co-op requires the same image stamp
  ([live-coop.md](../live/live-coop.md)).
- Rewriting the network replication protocol (0.91% of guest time; it is
  the multiplayer contract).
- Running several *guest* threads in parallel
  ([architecture-review.md](../perf/architecture-review.md), "More game
  threads in parallel?": no).
- Rewriting the renderer. The native renderer is a separate lane, and it
  consumes this work's outputs.

## 2. Who owns state

Today every reader goes through guest memory: HaloScript, AI, replication,
checkpoints (`0x123e20` snapshot, `0x123f60` revert, `0x124380` full restore,
writer `0x2151f0`), the renderer, theater (`lockstep::hash_sim(SimRegion::HALO2)`
every 30 ticks), telemetry, and the agent API.

### Options

**(a) Rust structs are the truth; guest layout is a synchronised mirror.**

- Pros: the cleanest Rust, and the most freedom of layout.
- Cons: every remaining guest reader needs the mirror current at the moment
  it reads. About 995 readers of published structures are known in loading
  alone, and object fields are read inline all over the image. A checkpoint
  revert is a guest `memcpy` of the whole arena, so Rust state goes stale
  without warning. Saves are arena bytes. Two copies of the truth means a
  whole class of desync bugs, plus a write-back on every tick that costs
  about what we save.
- Verdict: only reachable per structure once every reader of that structure
  is Rust (stage 3 in feature-engine.md "State").

**(b) Rust operates on guest memory through typed zero-copy views.**

- Pros: no synchronisation. Checkpoints and hashes are untouched.
- Cons: the work keeps the Xbox's array-of-structs, 32-bit-handle,
  pointer-chasing shape. Batching and SIMD across objects get hard. Static
  data stays in its compressed retail encodings.
- Verdict: the right *access* mechanism, but not enough on its own.

**(c) Hybrid.**

### Recommendation: (c), one rule

> At every point where guest code can run, and at every tick boundary, guest
> memory holds the complete persistent sim state in its retail layout.
> Rust state is allowed in four kinds only.

| Kind | Example | Lifetime | Checkpoint, revert, snapshot |
|---|---|---|---|
| **Static** | Decoded animation clips, collision BVHs built from BSP and MOPP, marker tables, object-function tables | per map load; content-addressed; shareable across processes | nothing to save; rebuilt from the map |
| **Derived** | Per-channel key cursors, last-pose caches, spatial-index acceleration | any time; must never change a result | dropped on an **epoch bump**: map load, revert `0x123f60`, full restore `0x124380`, snapshot load, or tick regression |
| **Transient** | The SoA working set of one pass (gather, compute, scatter) | inside one Rust call | none |
| **Extension** | State for new features (Halo 1 health) that has no retail home | the match or campaign | serialised into a versioned `SimExt` blob, saved and restored next to the arena by hooks on `0x123e20`, `0x123f60` and `0x124380`, carried in snapshots and films, and folded into the sim hash when present |

Why this holds up:

- **Checkpoints and saves.** These are arena bytes, so they need nothing new.
  Saves made by older builds load, because the layout is the retail one.
  Revert is the dangerous case: a raw `memcpy` that Rust never sees. The
  epoch bump on the revert and restore entry points makes it safe. So does
  the rule that derived state never changes results. A test enforces that
  rule: cold-cache results must equal warm-cache results.
- **Lockstep, co-op and films.** The inputs to guest memory are deterministic
  (section 3), and Rust adds no hidden persistent state. So the same build
  produces the same arena on every peer. The arena hash
  (`lockstep::hash_sim`, `impl.sim.arena-hash`, `HASH` chunks in films)
  keeps its meaning without change.
- **Snapshots.** Rust subsystems hold no continuations across ticks and are
  quiescent at tick boundaries by construction
  ([snapshots.md](../runtime/snapshots.md), "The rule"). A snapshot saves the
  extension blob and drops derived state.
- **Static data is where most of the win is.** Retail code decodes
  compressed tag encodings (animation codecs, the MOPP bytecode) on every
  query. A Rust subsystem decodes each one once into a hot layout.
- **The path to (a) stays open.** When every reader of a structure is Rust
  (for example, animation channel state after phase 3), that subsystem can
  keep a Rust-native copy and write the arena form only at tick end. The
  arena stays the checkpoint format.

The scheduler clock is counted in guest memory accesses
([architecture-review.md](../perf/architecture-review.md), change 2). Rust
code makes fewer of them, so slice ends move. That is the same for every run
of a build, so it does not affect determinism. It does mean whole-RAM traces
compare only within one switch set, which `exact` fingerprints already
handle.

## 3. Determinism and exactness

### Where each bar applies

| Consumer | Needs | Why |
|---|---|---|
| Co-op (synchronous path and below-game lockstep) | same build on all peers | the image stamp must match (live-coop.md) |
| Dedicated host and its clients | same build | replication is host-authoritative; clients run our build |
| Films | same build, and same results on another machine and architecture | Linux x86-64 hosts record; Mac arm64 players watch |
| Checkpoints and campaign saves | load across our builds | the arena layout is unchanged, so they do |
| Sim training, `h2 sim diff`, A/B | same build plus switches; exact against lifted while a subsystem is being proven | parity is the proof |
| Retail Xbox | nothing | no interop |

**Contract D (always):** the same build and switches give bit-identical
results on arm64 and x86-64, on macOS, Linux and Windows, and whatever the
worker-thread count.

**Bar E (verification, phases 1-3):** a rewritten subsystem produces the same
bits as the lifted code for every guest-visible write. The win comes from
structure, not from reordering floats. With E, `h2 sim diff` and the arena
digests prove a phase against `-impl.sim.native`, with no tolerance and no
judgement calls. Getting the animation codecs and pose math exact is cheap:
the guest does them in SSE with a fixed operation order, and the existing
ports already match it with NEON and SSE2 lanes. Where retail uses x87, the
Rust kernel uses the `x87` crate's exact operations, or proves the double
result equals the ext80 one, as `sse-native` does.

**Relaxing to D only (phase 4 onwards, per subsystem, with user sign-off):**
the kernel may reorder floats, use SIMD across bodies and solve islands in
parallel. Verification then moves to behaviour and statistics (below), and
films and goldens get re-recorded per build.

### Float rules for every Rust sim kernel (enforced in `sim-core`)

- IEEE `f32` and `f64` add, sub, mul, div and sqrt only. Those are correctly
  rounded and identical on NEON and SSE2.
- No `mul_add` or FMA, and no compiler fast-math. Rust never contracts on its
  own, and `sim-core` lints against `mul_add`.
- No hardware `rcp` or `rsqrt` estimates, and no platform `libm` (`sin`,
  `atan2`, `exp`, `powf`). Transcendentals come from `sim-core::det`
  (exact-x87 routines from the `x87` crate where bar E applies, fixed
  polynomials where D applies).
- A known host float environment: round to nearest, no flush to zero. The
  pass checks it once at entry; it does not reset it per operation.
- Reductions and accumulation run in a fixed order: object index order, never
  worker order. No `HashMap` iteration feeds results.
- Every RNG draw (`[0x4e7408]` words) stays on the game thread, in retail
  call order.

### Verification ladder (every phase climbs all of it)

1. **Unit tests** on synthetic data in the subsystem crate: every codec,
   shape and edge case. No game data.
2. **Differential tests** (`crates/engine/tests/differential.rs`,
   `differential` feature, owner's XBE, skipped without it). Each taken-over
   entry runs against the lifted code on the IR interpreter from the same
   guest state; compare all memory, return value and registers outside the
   clobber list. The old per-function ports join as extra oracles.
3. **Disc sweep** (`#[ignore]`, owner's disc). Run the subsystem over every
   relevant tag on every map (for animation: all 77,856 animations at
   frame 0, a middle frame, the last frame and a fractional frame) against
   the lifted path. Results stay local.
4. **Run parity.** The sim-budget A/B with `-impl.sim.native.<x>` against
   on: Lockout local, Waterworks local and Waterworks mixed, seed 7, 1,800
   ticks, `impl.sim.arena-hash` on. Every `state_hashes_v1` row and arena
   block must match, and `h2 sim diff` must print nothing. Add a headless
   campaign level through the agent API, with arena hashes.
5. **Cache-cold arm.** The same runs with derived caches dropped every tick
   must give identical hashes.
6. **Cross-architecture.** The same match on arm64 and x86-64 gives identical
   hash series (the coordinator's Linux host toolchain).
7. **Behaviour suites**, the D-only bar for phase 4 and later:
   - the headless Halo 1 behaviour suite the coordinator runs before play;
   - the superbounce regression ([high-framerate.md](../game/high-framerate.md) §4);
   - bot match outcome distributions across N seeds (kills, deaths, time
     alive, within a stated tolerance);
   - golden films re-recorded and replayed hash-clean.

## 4. Subsystem map

The shares are the static census in sim-budget.md, "Subsystem boundaries". The
first share column is a share of guest self samples; the second column turns
the named top rows into shares of the thread. 37.5% of guest samples are
"Unreached" and 14.3% "Multiple", so these are lower bounds. Phase 0 tightens
them. "Win" assumes a rewrite runs the family about 4× faster, and counts the
runtime overhead that goes with guest code (runtime is 23.8% of the thread
against 70.8% guest), all on 14.12 ms/tick.

| Subsystem | Guest self | Named top rows, % thread | Data owner today | Readers to keep fed | Risk | Win (Waterworks) |
|---|---:|---:|---|---|---|---|
| Objects and units update | 11.26% | ~2.5 (`bd090`, `bb050`, `14a370`, `108d90`, `bab40`, `bcc20`, `c7840`) | object datums in the arena; object type descriptors | everything: scripts, AI, replication, render, telemetry | high (per-type callbacks) | 1.2-1.6 ms |
| Physics, Havok and collision | 9.67% | 5.39 | Havok world `0x51e9a4` (in the arena), BSP and MOPP tags | objects, weapons, sound, camera, AI | collision medium; Havok very high | collision 0.8-1.1 ms; Havok 1.5-2.5 ms (most of it in Unreached) |
| Sound sim | 6.04% | ~1 | sound records, listener | voice scheduling, completion events | medium | 0.5 ms (light hosts already trim) |
| Players, game mode, weapons | 5.61% | ~1 | player and weapon datums | scoring, replication, HUD | high | later |
| Animation and pose | 5.22% plus the pose math under objects | ~3.6 (`142a60`, `1421f0`, `2798a0`, `112070`, `1dd9d0`, `28c530`, `28c470`, `28c9e0`, `16760c`, `b8d30`) | channel state in object data; clips in cached resources | node matrices `[body+0x116]` (Havok fetch `1ef070`, markers, render, bounding sphere `bdf90`) | **low** (pure sampling, SSE math) | 0.7-1.0 ms |
| Camera and first person | 4.76% | ~1 | per-seat observer records | particles, camera collision | medium | with phase 1c and 2 |
| Effects, particles, lights | 4.07% | ~0.6 | effect, particle and light datums | the shared RNG (`249170`), damage from effect parts | medium | 0.3-0.5 ms |
| AI and scripts | 0.06% here (bots, no actors) | — | actors, squads, HS threads | campaign: unknown | very high | **campaign census needed** |
| Network replication | 0.91% | — | the sim record `682e0` | peers | very high | none: stays |

### Sketches

**Animation and pose (phase 1).**

- *Data.* Clips are decoded once into SoA per channel. Static frames are
  dense `[i16; 4]` or `[f32; 3]` runs. Keyframed channels become
  `times: [u16]` plus `values` with a per-channel key range. Flag arrays are
  replaced by precomputed node-to-slot maps. The output is the retail 32-byte
  local record, so scatter is a plain copy.
- *Passes.* Sample a mask of nodes in one call, with no codec table dispatch
  per channel and none of the evaluation globals `0x504460..0x5044c0`. Then
  build node matrices for a whole object in one SIMD pass over the parent
  order. Later, batch across all objects in the post-physics walk.
- *Parallelism.* Per object, once the walk is batched (phase 3).
- *Benefits.* Theater and the web viewer sample the same store; Halo 1 models
  convert into the same codecs.

**Collision and spatial queries (phase 2).**

- *Data.* A Rust collision world per loaded BSP and per collision model:
  - surfaces, planes and leaves as flat arrays;
  - MOPP programs pre-decoded (the `mopp` crate already holds the interpreter
    as a query model and an offline compiler);
  - an object spatial index that replaces the `bb050` visit-mark walk on the
    Rust side, while still writing the guest's visit stamps.
- *Entries:*
  - collision query `1697c0`;
  - BSP segment `1de630` and `1de6d0`;
  - point to leaf `14a280`;
  - geometry test `11e5e0`;
  - float MOPP ray `3009e0` and integer MOPP box `2ff8b0`;
  - the datum walk `b89b0`;
  - spatial query `14a370`.
- *Exactness.* Bar E: hit choice and `t` must match bit for bit. That is
  feasible because the integer MOPP is exact by construction and the float
  paths are short SSE sequences.
- *Parallelism.* Little: queries arrive synchronously from guest callers. The
  win is cost per query, and later batched queries once phase 3 owns the
  callers.

**Objects and units update (phase 3).**

- *Orchestration.* Rust runs the pre-physics `b6f10`, post-physics `b7060`
  and final walks, `bc470` and `bc820`, with `bd090` per object, over an SoA
  gather of the transform, parent, flags and channel state. Per-type
  descriptor callbacks (unit `+0x40`, `c6b30`, and the others) stay guest at
  first and are called through the async boundary
  ([feature-engine.md](feature-engine.md), "Calling code that is still
  recompiled"). They come over type by type, biped and weapon first.
- *Also in this phase:* the object-function evaluator (`bab40`, `108d90`,
  `bcc20`), lights (`c1400`, `c0c80`, which subsume `impl.sim.static-light`'s
  bypass) and markers (`b8d30`).
- *Parallelism.* Pose and matrix passes run per object in hierarchy waves.

**Havok world (phase 4).**

- *Data.* A Rust rigid-body world in SoA, with its own broadphase, contact
  manifolds against the phase-2 collision world, and a solver: in place of
  `1c3d30` → `311100` → `30fef0` → `316a20` → `31c100`, the welding and
  BV-tree agents (`2fa2f0`, `2faa60`) and the convex agents (`2ee610`,
  `3048a0`).
- *Guest boundary.* The guest-visible body and contact layout
  ([havok-runtime.md](havok-runtime.md)) stays the arena truth, written at
  step end. Islands are solved in parallel with fixed ordering.
- *Exactness.* Bar D, which needs sign-off. Must pass the superbounce
  regression and the vehicle, grenade and ragdoll behaviour cases.
- *Risk.* The biggest win and the biggest risk. It opens up physics features
  and, eventually, a host with no Havok code at all.

**AI, actors, pathfinding and HaloScript (phase 5+, campaign).**

- *HaloScript.* The `crates/hs` VM exists and is not wired in
  ([hs_vm.md](hs_vm.md)). It goes in behind `impl.sim.native.hs`, with
  arena-hash parity on campaign levels.
- *Pathfinding.* `crates/pathfind` decodes and generates the data.
- *AI.* `AI_TARGET_SCORE` `265d30` is already a named hot point.
- *Order.* Set by the phase 0 campaign census, not guessed here.

**Effects and sound (phase 5).** Particle and effect state can become fully
Rust-owned (stage 3), because only the renderer reads it. The RNG draws in
`249170` must keep retail order. The sound service `125b40` and listener
`127400` move over once phase 2 provides the BSP queries they use.

**Network replication.** Stays guest. Rust subsystems feed it the same arena
state.

## 5. Roadmap

Each phase ships on its own and defaults on once its exit criteria hold.
Then its switch is deleted, along with the per-function `impl.sim.native_*`
ports it replaces; their bodies stay only as differential oracles if a test
still uses them.

| Phase | Scope | Switch | Exit criteria |
|---|---|---|---|
| **0** (now, alongside 1) | Campaign census: the sim-budget method on two campaign levels headless with AI (`03a` and Old Mombasa), plus a deeper attribution of Unreached. `sim-core` crate. Switch-tree change. | none | Census table committed to sim-budget.md. Phase order after 2 decided. |
| **1a** | Animation sampling (detailed below) | `impl.sim.native.anim` | Ladder steps 1-6 clean. Waterworks ≥ 0.3 ms/tick faster. Evaluator family self samples down ≥ 80%. Retires `native_rotation_codec` and `native_animation_stream`. |
| **1b** | Node-matrix build (`1421f0`, `142a60` and the loop that drives them in `bd090`'s pose step) | same switch | Ladder clean. Cumulative ≥ 0.6 ms. Retires `native_transform_compose` and `native_pose_matrix`. |
| **1c** | Unit pose and overlays (`112070`, aim grids), first-person pose (`16760c`), marker transforms (`b8d30`) | same switch | Ladder clean. Cumulative ≥ 0.8 ms. Retires `native_unit_pose`, `native_marker_transforms`, `native_first_person_pose`, `native_first_person_filters` and `native_unit_aim`. |
| **2** | Collision world and queries | `impl.sim.native.collision` | Ladder clean. ≥ 0.8 ms more. Retires `native_point_to_leaf`, `native_geometry_test`, `native_bsp_segment`, `native_collision_query`, `native_mopp_box`, `native_mopp_ray`, `native_spatial_visit` and `native_spatial_portal`. |
| **3** | Object update walk, function evaluator, lights, per-type callbacks type by type | `impl.sim.native.objects` | Ladder clean. ≥ 1.2 ms more. Retires `native_object_import`, `native_object_value`, `native_base_object_import`, `native_object_pre_update`, `native_object_post_update`, `native_objects_pre_physics`, `native_objects_post_physics`, `native_object_change_colors`, `native_unit_update`, `native_light_record`, `native_observer_camera` and `static-light`. |
| **4** | Havok world | `impl.sim.native.physics` | User sign-off on bar D. Behaviour suites pass. Cross-arch hashes identical. ≥ 1.5 ms more. **Waterworks ≤ 7.5 ms/tick.** Retires `native_havok_solver`, `native_havok_support_vertices` and `native_havok_weld`. |
| **5+** | Campaign order from phase 0: HaloScript VM, AI, pathfinding, effects, sound | one per subsystem | Campaign guest CPU ≤ 12 ms at the user's 4x case. |

### Phase 1a, ready to hand to a lane

**Boundary.** Override the three evaluation wrappers: `0x00279d80` (plain
frame), `0x00279e40` (positional blend) and `0x0027a060` (interpolated).
Everything below them goes:

- `2798a0`;
- the ~60 per-node evaluators `27a6e0..28c090`;
- every codec function (`28c470`, `28c530`, `28c9e0`, `28cdb0`, `28d560` and
  the others listed in engine-map.md, "Evaluation and the codec table");
- the codec table dispatch at `1c73a8`.

The callers have already resolved the resource: each wrapper receives the
12-byte view built by `1ddb40` (data pointer, sizes pointer `&element[+0x3c]`,
node count, frame-info type, frame count). So the boundary makes **no
resource-cache calls** and nothing inside it calls guest code. The bodies are
plain `fn`s. `1c6f60`, `1dd9d0` (resolve and preload), `1236f0` and `1237e0`
(cache request and get) and `1dd630` (with its RNG draw at `1dd6e1`) all stay
guest.

**Step 0 (research, about half a day, static reading with `h2 disasm`):**

- the argument ABI of the three wrappers;
- every memory write under them;
- a cross-reference census of `0x504460..0x5044c0`.

If any address outside the family reads those globals, the Rust body writes
their final values. Otherwise it leaves them. Record all of this in a new
contract page, `docs/engine/anim-sampling.md` (the engine README format),
including the exact retail operation order for normalisation and blending.

**Crates and modules (new):**

- `crates/sim-core`: no `runtime` dependency.
  - `math`: `Quat`, `Vec3` and `NodeMatrix`. `NodeMatrix` is `#[repr(C)]`
    with the 0x34-byte `real_matrix4x3` layout from native-renderer.md:
    scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`.
  - `lanes`: the exact SSE-order f32x4 operations, moved from
    `crates/engine/src/sim_native/lanes.rs` so both use one copy.
  - `det`: the float rules and the environment check.
  - `Epoch`: the derived-state invalidation counter.
- `crates/sim-anim`: depends on `sim-core`, and on `tags` for the codec enum
  and flag-array layout only. The `tags` sampler deliberately differs from the
  engine (it holds the first key and rejects some data), so it is not reused
  for values.
  - `ClipStore`: a dense table keyed by `(map epoch, sizes-pointer address)`.
    The sizes pointer is tag data, fixed for the map's lifetime, and names
    exactly one animation. Entries are `Arc<Clip>`. The table is cleared on
    map load.
  - `Clip::decode(view_bytes, sizes, nodes, frames)` builds from the
    guest-resident blob the first time the clip is sampled. It decodes the
    bytes the guest would read, with no separate map read, so it cannot
    disagree with the guest.
  - `Clip` fields:
    - `nodes` and `frames`;
    - up to two streams, each holding its codec, per-node slot maps for
      rotation, translation and scale (`u16::MAX` = none), and channel
      storage by codec: dense frame runs for 1, 2, 3 and 8; `times` plus
      `values` plus `key_range` per channel for 4 to 7, with u8 times
      widened to u16.
  - `sample(&Clip, Frame { index, next, blend, kind }, mask: &[u32], out: &mut [LocalNode])`.
    `LocalNode` is the retail 32-byte record: rotation `[f32; 4]` at `+0`,
    translation `[f32; 3]` at `+0x10`, scale `f32` at `+0x1c`. The static
    stream is applied first, then the animated stream. Arithmetic follows the
    retail operation order through `sim-core::lanes`.
- `crates/engine/src/sim_anim.rs`: the adapter.
  - Reads the wrapper arguments.
  - Gets or builds the clip.
  - Calls `sample`.
  - Writes the output records, and the globals only if step 0 says so.
  - Sets the return registers.
  - Faults the way the guest would on an unmapped view: it reads the view
    before writing any output.
- `crates/feature/src/sim_native.rs`: installs the three entries under
  `impl.sim.native.anim` (hierarchical, default off until exit) and removes
  the `native_rotation_codec` and `native_animation_stream` entries.
- `crates/engine/src/points.rs`: the three wrapper points, with ABIs from
  step 0.
- `crates/feature/src/switches.rs`: no new mechanism, because hierarchical
  lookup already exists.

**Guest readers still fed (unchanged):** the output records, which `112070`,
`16760c` and the object pose path then compose into node matrices at
`[body+0x116]`. Havok's rigid-body fetch `1ef070`, the markers, the
bounding-sphere update and the renderer all read those matrices.

**Verification:**

- **Unit tests in `sim-anim`.** Use synthetic streams for codecs 1-8:
  - frames 0, last, fractional, and exactly on a key;
  - masks of 0, 1 and all nodes, crossing an 8-node flag byte;
  - both streams present;
  - codec 8 alignment.
- **Differential test per wrapper.** Use synthetic views written into guest
  memory, with random frames and masks. The lifted wrappers run on the IR
  interpreter. Memory and registers must be bit-identical, with
  `impl.sim.native_simd` lanes on and off.
- **Disc sweep (`#[ignore]`).** Every animation on every map, at four
  frames, against the lifted path.
- **Run parity and the cache-cold arm.** The sim-budget A/B with
  `-impl.sim.native.anim` against `impl.sim.native.anim`, all three cases:
  identical hash series, and `h2 sim diff` prints nothing. Then the same with
  the `ClipStore` cleared every tick.
- **Cross-arch and campaign.** The same Waterworks case on x86-64 (cross-arch
  hashes), and one campaign level headless with arena hashes.
- **Timing.** Three unprofiled medians per arm, plus one profiled pair to
  confirm the evaluator family's self samples dropped.

**Lane ownership:**

- new: `crates/sim-core`, `crates/sim-anim`, `crates/engine/src/sim_anim.rs`,
  `docs/engine/anim-sampling.md`;
- edited: `crates/feature/src/sim_native.rs`, `crates/engine/src/points.rs`,
  the workspace `Cargo.toml`, `crates/engine/Cargo.toml`.

It touches nothing owned by Live, bots, theater or film.

**Phase 1b and 1c follow-on:** 1b's step 0 is to find the smallest function
that contains the whole local-record-to-node-matrix loop under `bd090` (or
`bdb60`), and override that, so one Rust call builds every matrix of an
object.

## 6. Threading inside a tick

- One host thread still runs every guest thread. Rust parallelism happens
  only **inside one Rust call**. The game thread gathers inputs, workers
  compute into Rust buffers, and the game thread scatters into guest memory
  in fixed order and joins before returning to guest code. Workers never
  touch guest memory or call guest code.
- `sim-core::Jobs` is a persistent pool with **fixed chunking by item
  index**. Results do not depend on the worker count; a test runs one, two
  and eight workers and compares bits.
- The worker count comes from settings. It defaults to 1 on hosts and sims,
  where parallel matches use the cores, and to `cores / 2` (capped at 4) for
  `h2 play`.
- Parallel work needs batches, and batches come with phase 3 (the object
  walk). Candidates:
  - pose sampling and matrices per object, in hierarchy waves (roots, then
    children, which compose with parent node matrices, `ba160`);
  - phase 4 islands;
  - effect and particle aging.
- Synchronous guest-driven queries (collision in phase 2) stay serial and
  get cheaper per call instead.
- RNG draws, event emission and anything replication reads happen on the
  game thread, in retail order.

## 7. Interfaces for theater, film, stats and Live

The other session (`bed6c8dc`: Live, bots, theater, film) owns the internals.
This is only the surface this work offers. It goes in a new `sim-state` crate
that depends on `sim-core` and the guest views. It is read-only, and is
called at tick end on the game thread.

```rust
pub trait SimRead {
    fn tick(&self) -> u32;
    fn players(&self) -> &[PlayerView];        // typed, from the arena
    fn objects(&self) -> ObjectIter<'_>;       // id, type, tag, transform, parent
    fn anim(&self, object: ObjectId) -> Option<AnimState>; // graph, animation, frame per channel
    fn pose(&self, object: ObjectId, out: &mut [NodeMatrix]) -> bool; // sampled via ClipStore
    fn events(&self) -> &[SimEvent];           // this tick: Kill, Damage, Spawn, Pickup, Score
}
pub struct Keyframe { pub tick: u32, pub arena: Bytes, pub ext: Bytes } // 4 MiB + ext
pub fn keyframe(&self) -> Keyframe;           // no Rust-only state beyond ext
pub fn sim_hash(&self) -> u64;                // hash_sim(HALO2) ⊕ ext, when ext non-empty
```

- **Theater and film.** `HASH` chunks keep `hash_sim`. `KEYF` (reserved
  today) can be `Keyframe`. Arena deltas against the previous keyframe use
  the existing 4 KiB write tracker, which keeps keyframes small. Seek: load
  the keyframe, then replay `NETR` and `RAND` forward. The machine-snapshot
  quiescence rules still apply to the guest side. Rust subsystems add no
  blockers.
- **Stats.** `SimEvent` comes from the Rust subsystem that applies the event,
  once that subsystem is Rust. Until then it comes from the existing hooks
  (`0x15ce70` kill and `0xd9640` damage report, live-gamestate.md).
  live-telemetry's raw readers can move onto `SimRead` at their own pace.
- **Live and hosts.**
  - Static caches (clip store, collision world) are content-addressed
    (`map digest + cache format version`). They are written once to
    `~/Library/Caches/h2recomp` (or the platform equivalent) and mmapped by
    every match process, so a per-match host pays decode once per machine.
  - `Jobs` stays at 1 on hosts.
  - `sim-state` gives a dedicated host the scoreboard and events without
    memory scraping.

## 8. Open questions for the user

1. **Bar D for physics.** Once phases 1-3 are exact, may Havok's replacement
   (phase 4) be deterministic but not retail-bit-exact? It would be checked
   by behaviour suites, including superbounces, and every film and golden
   would be re-recorded per build. If not, phase 4 is a structural port held
   to bar E, with a much smaller win.
2. **Cross-architecture films.** Should a film recorded on a Linux x86-64
   host replay on a Mac arm64 build? This plan's float rules assume yes. Today
   films require the same image.
3. **North star after phase 2.** Campaign frame rate (AI, scripts, actors) or
   host density (collision, objects, Havok)? The phase 0 census gives the
   numbers. The question is which one wins a tie.
4. **Per-function switches.** Delete the 32 `impl.sim.native_*` switches (16 in `sim_native`, 16 in `sim_physics`) as
   each subsystem switch replaces them, and keep their bodies only as test
   oracles? This plan assumes yes.
