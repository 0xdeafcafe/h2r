# Simulation rewrite: phase 1, animation

Status: Partly built (1a to 1d; the table in [sim-rewrite.md](sim-rewrite.md) gives each slice).
Native only, no switch.

The animation phase of [sim-rewrite.md](sim-rewrite.md): sampling (1a), node matrices (1b), unit pose, markers and first-person bodies (1c), manager and graph resolve (1d).

## Phase 1a: animation sampling

Built; the contract is [anim-sampling.md](anim-sampling.md). The boundary is
the three evaluation wrappers `0x00279d80` (plain frame), `0x00279e40`
(positional blend) and `0x0027a060` (interpolated), with everything below
them: `2798a0`, the ~60 per-node evaluators `27a6e0..28c090`, every codec
function and the codec table dispatch at `1c73a8`. Their callers have
resolved the resource (the 12-byte view built by `1ddb40`), so the boundary
makes no resource-cache calls and calls no guest code. Slice 1d owns `1c6f60`
and the preload at `1dd9d0`. Cache request and get (`1236f0`, `1237e0`) and
`1dd630` (with its RNG draw at `1dd6e1`) stay guest.

| Crate or file | Holds |
|---|---|
| `crates/rewrites/sim-core` | `math` (`Quat`, `Vec3`, `NodeMatrix`: `#[repr(C)]`, the 0x34-byte `real_matrix4x3` of native-renderer.md, scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`), `lanes` (the exact SSE-order f32x4 operations, shared with `crates/rewrites/engine/src/sim_native/lanes.rs`), `det` (the float rules and the environment check), `Epoch` (the derived-state invalidation counter) |
| `crates/rewrites/sim-anim` | `ClipStore` (a dense table keyed by `(map epoch, sizes-pointer address)`, `Arc<Clip>` entries, cleared on map load), `Clip::decode` (built from the guest-resident blob the first time the clip is sampled, so it cannot disagree with the guest), `sample(&Clip, Frame, mask, out)` writing the retail 32-byte `LocalNode` (rotation `[f32; 4]` at `+0`, translation `[f32; 3]` at `+0x10`, scale at `+0x1c`; the static stream first, then the animated one) |
| `crates/rewrites/engine/src/sim_anim.rs` | the adapter: reads the wrapper arguments, gets or builds the clip, calls `sample`, writes the output records and sets the return registers. It reads the view before writing any output, so it faults as the guest would |
| `crates/rewrites/sim-bridge/src/sim_native.rs` | installs the three entries |
| `crates/hooking/guest-layout/src/points.rs` | the three wrapper points |

The `tags` sampler differs from the engine on purpose (it holds the first key
and rejects some data), so `sim-anim` does not reuse it for values. The
output records stay the input of `112070`, `16760c` and the object pose path,
which compose them into the node matrices at `[body+0x116]` that Havok's
fetch `1ef070`, the markers, the bounding-sphere update and the renderer
read.

Verification is the ladder of section 3. The phase's own tests are unit tests
on synthetic streams for codecs 1 to 8 (first, last, fractional and on-key
frames; masks of 0, 1 and all nodes across an 8-node flag byte; both streams;
codec 8 alignment), a differential test per wrapper with `impl.sim.native_simd`
lanes on and off, and the `#[ignore]` disc sweep at four frames per animation.

## Phases 1b to 1d
 One Rust call (`sim_anim::build_matrices`, through
`engine::sim_anim::skeleton`) builds every node matrix of an object at the
loop's entry in `bd090` ([skeleton-walk.md](skeleton-walk.md)). `1dd1c0` runs
the same leaves for the animation graph's callers; `sim_anim::graph_matrices`
computes it in one call ([skeleton-walk.md](skeleton-walk.md#the-graph-walk-1dd1c0)).
The body answers (`anim.graph_walk`); a plan the walk declines stays a guest
call, counted by reason. The ports `sim.port.transform_compose` and
`sim.port.pose_matrix` install only where no subsystem body owns their point.

Each 1c slice has the same shape: a gather, one Rust kernel call, a
scatter. The table names the port that stays the vehicle and the Rust entry
it calls.

| Slice | Guest function | Port (vehicle) | Rust entry | Page |
|---|---|---|---|---|
| Markers | `b8d30`; scan `1d8f00`, transform fill `1d8f50`, basis leaf `141e10` | `sim_native::marker` | `sim_anim::model_markers` through `engine::sim_anim::markers` | [markers.md](markers.md) |
| First-person palette | `16760c` | `sim_physics::first_person` | `sim_anim::palette` through `engine::sim_anim::palette` | [palette.md](palette.md) |
| Unit pose | `112070` | `sim_native::unit::run` | plans the base and aim channels, angle projections and overlay tail one guest call at a time | [unit-pose.md](unit-pose.md) |
| Unit aim | `c7840` (nine return sites) | checked segment plan | `sim_anim::unit_aim` | |
| First-person filters | `166d75` (40 return sites; five calls to `11eed0` can run within the segment) | checked segment plan | `sim_anim::first_person_filters` | [first-person-filters.md](first-person-filters.md) |

The [channel manager](animation-manager.md) receives the unit pose's calls
through the dispatcher. Its contract describes the channel writes, callback
frames and sampling view that the phase 1d arms compare.

Unit aim projects the directions into joint space, enforces yaw and pitch
limits and updates direction and angular velocity. First-person filters
advance four seat axes and visit both weapon slots, including their motion
tracks and idle timers.

Each body resumes after a guest callee using the live guest frame. A declined
segment starts the original at its entry or return selector. An image must
export the entry and every return selector for that body.

The differential in
`engine/tests/sim_physics_differential.rs` checks the transfer, CPU and memory
at the boundaries and requires every site. The phase 1c ladder and cumulative
timing gate are in the table above.

The unit animation state `111650` is a body of the object update (slice 6c,
[object-update.md](object-update.md)).

