# Waterworks simulation tick budget

The 2026-11-20 five-minute, four-local-bot comparison at the same seed measured **14.96 → 14.12 ms/tick on Waterworks** and **9.07 → 8.49 ms/tick on Lockout** after commit `36b417561`. Lockout kills matched. One Waterworks bot scored four kills in one run and one in the other. Same-build Waterworks repeatability is open. An earlier 62% bot-glue sample came from a different roster with two phantom seats. These are historical measurements. Use the phantom-only fixtures below for current comparisons.

## Where the main thread spent its time

The in-process Waterworks profile sampled the main thread at 1,000 Hz over 258.85 seconds, with 84,358 samples, 84,343 on CPU and a mean sampling pause of 485.7 µs. Percentages below use all 84,358 samples. Use the separate unprofiled match times for throughput.

| Charged category | Samples | Share |
|---|---:|---:|
| Guest | 59,690 | 70.76% |
| Runtime | 20,050 | 23.77% |
| HLE | 2,816 | 3.34% |
| Kernel | 1,633 | 1.94% |
| Other | 123 | 0.15% |
| D3D | 46 | 0.05% |

Even eliminating all non-guest time would give about **1.41×** on this sample. Halving the profiled tick would still require removing at least **20.76 percentage points of guest time**. The original Waterworks target is 7.48 ms/tick. The 30 functions in the first table of guest addresses together account for 10,446 self samples, or 12.38% of the thread. Optimizing even every one of these leaves in isolation has a limited ceiling. A rewrite of a frequently executed family, such as collision traversals or pose evaluation, can also remove work in its callees.

## Guest functions and their callers

Coverage is the union of the top 25 guest-own and top 25 guest-with-services lists in `profile.txt` and `profile.json`. Of 66,470 samples containing a guest frame in `profile.folded`, 42,629 have exactly one. Three measurements answer different questions:

- **Self** charges the innermost classified frame, using the profiler's symbol classification.
- **Inclusive** counts a sample once if this address occurs anywhere on the visible stack, including below it in guest children or host services. Recursive appearances count once. Trampolines hide ancestors, so these are lower bounds. Rows overlap.
- **With services** follows the profiler's `guest_total`, charging the innermost visible guest even when a host service is running. For example, object update `bd090` appears in 1,524 samples, but is the innermost guest in only 490.

Caller chains are static paths through the retail image identified in the engine map. Direct calls and tail jumps were decoded from the local XBE's .text section at VA `0x12000`, using Ghidra function boundaries and sampled function starts. Indirect edges (⇢) use descriptor or codec slots. The paths identify possible callers, without assigning a shared leaf's entire sample count to any one caller. All bare addresses and offsets in the table are hexadecimal. “Yes” means at least one gameplay path needs the operation or its side effects. “Unknown” means its output or side effects have no established safe substitute. [Engine map](../engine/engine-map.md) · [Named entry points](../../crates/hooking/guest-layout/src/points.rs) · [Havok bodies and contact agents](../engine/havok-runtime.md) · [MOPP queries](../formats/mopp.md) · [Object functions](../halo1/objects.md) · [Profiler accounting](../../crates/tools/perf/src/profile.rs)

**Chain roots.** Tick = GAME_TICK `137fe0`, pre = OBJECTS_PRE_PHYSICS `b6f10`, post = OBJECTS_POST_PHYSICS `b7060`, world physics = `1c3d30`, weapon fire = `104150`, sound service = `125b40`. A chain starting at a middleware method or a type table stops at the dispatch boundary where caller identity is known.

| Address | Self % | Inclusive % | With services % | Static caller chain to this address | Subsystem and operation | Sim-needed |
|---|---:|---:|---:|---|---|---|
| `0x00142a60` | 0.98 | 1.02 | 1.02 | Tick → post → bc820 → bd090 → 142a60 | Transform composition for object poses. Marker path b8d30 → 1d8f50 also calls it. | Yes |
| `0x003009e0` | 0.95 | 1.41 | 1.41 | Weapon fire → 1a4c00 → 1697c0 → 244980 → 2df720 → 301800 → 3009e0 | Collision, float MOPP traversal, likely ray. Recursive. | Yes |
| `0x001de6d0` | 0.83 | 0.85 | 0.85 | Weapon fire → 1a4c00 → 1697c0 → 1de630 → 1de6d0 | Collision BSP segment walk. Sound service also reaches 1de630 through 221a70 → 221e30. | Yes |
| `0x001421f0` | 0.60 | 0.62 | 0.62 | Tick → post → bc820 → bd090 → 1421f0 | Shared pose math. Both bd090 and animation caller 1dd1c0 appear in folded stacks. | Yes |
| `0x0031c940` | 0.55 | 0.63 | 0.62 | Tick → world physics → 311100 → 30fef0 → 316a20 → 31c100 → 31c940 | Havok world step. The specific solver operation is unresolved. | Yes |
| `0x0014a280` | 0.54 | 0.55 | 0.55 | Sound service → 127400 → 11bed0 → 14a280 | BSP point-to-leaf. Shared with observer 16f280 and collision query 1697c0. | Yes |
| `0x000bd090` | 0.53 | 1.81 | 0.58 | Tick → post → bc820 → bd090 | Object transform/animation update. Also called from the object creation path b7b40 → bd020. | Yes |
| `0x000c0c80` | 0.49 | 1.02 | 0.51 | Tick → c1400 → c0c80 | Light record update. Reads object function outputs and writes the light datum. | Yes |
| `0x002ff8b0` | 0.48 | 0.49 | 0.49 | Structure collision → 1eea50 → 2df640 → 300150 → 2ff8b0 | Integer MOPP box traversal for contact candidates. Recursive. | Yes |
| `0x003048a0` | 0.48 | 0.64 | 0.56 | Convex collision agent → 2daf70 → 2ee610 → 3048a0 | Reconstructs support vertices from cached feature IDs. Also reachable through agent method table slot 412c68. | Yes |
| `0x001697c0` | 0.47 | 1.35 | 0.52 | Weapon fire → 1a4c00 → 1697c0 | Collision query shared with observer 16f280 → 170fd0 → 171830 and decal placement 17e670. | Yes |
| `0x002fa2f0` | 0.41 | 0.67 | 0.51 | Welding agent [412f7c + 10] ⇢ 2faa60 → 2fa2f0 | Tree-A argument swap into the BV-tree contact/weld update. Tree-B also dispatches here. | Yes |
| `0x0014a370` | 0.38 | 0.58 | 0.55 | Tick → pre → 109580 → bb050 → 14a5b0 → 14a370 | Spatial query used in the object visit walk. Also reachable from lights. | Yes |
| `0x002798a0` | 0.38 | 0.50 | 0.39 | Unit pose 112070 → 1c6f60 → 279d80 → 2798a0 | Animation frame and stream selection. Plain, interpolated and blend wrappers share it. | Yes |
| `0x00112070` | 0.34 | 0.69 | 0.43 | Object pose bd090 → bdb60 → 109050 ⇢ 112070 | Unit pose evaluation. Unit descriptor 4679b0 + 6c, indirect call at 1090a5, then animation manager calls from the leaf. | Yes |
| `0x001dd9d0` | 0.33 | 0.66 | 0.37 | Object pose bd090 → 1c6440 → 1daea0 → 1dd9d0 | Animation graph resolution/preload. Resource residency can affect which pose is available. | Yes |
| `0x00108d90` | 0.32 | 0.38 | 0.38 | Tick → pre → bc470 → be8e0 → bab40 → 108d90 | Lookup of an object function import through its type descriptors. Also used by lights. | Yes |
| `0x0016760c` | 0.32 | 0.62 | 0.33 | Tick → 165dc1 → 166d75 → 16760c | First-person animation evaluation, inferred. Uses per-player 20cc-byte records shared with weapon render 165f16, calls 1c6f60. | Yes |
| `0x000bab40` | 0.31 | 0.50 | 0.33 | Tick → pre → bc470 → be8e0 → bab40 | Object function evaluator. Also serves light updates and sound attachment volumes. | Yes |
| `0x000b8d30` | 0.30 | 0.63 | 0.31 | First-person camera 23c110 → 23bd70 → cafc0 → b8d30 | Object marker transform. Weapon fire also directly calls it. | Yes |
| `0x000bb050` | 0.30 | 0.56 | 0.30 | Tick → pre → 109580 → bb050 | Spatial object visit marks. Also called on the players-after path 14d8b0 → 1508e0. | Yes |
| `0x0028c530` | 0.29 | 0.29 | 0.29 | Animation evaluation → per-node evaluator ⇢ codec 8 [47fc70] ⇢ 28c530 | Interpolated blend-screen rotation. Node evaluators in 27a6e0–28c090 call the selected rotation slot. | Yes |
| `0x0011e5e0` | 0.29 | 0.29 | 0.29 | Weapon fire → 1a4c00 → 1697c0 → 11e5e0 | Collision geometry test using squared distance, radius and direction. | Yes |
| `0x000bcc20` | 0.28 | 0.34 | 0.30 | Object function bab40 → 108d90 ⇢ bcc20 | Base object import handler. Type ancestry dispatches descriptor slot +4c. | Yes |
| `0x00166d75` | 0.28 | 0.45 | 0.30 | Tick → 165dc1 → 166d75 | Per-local-player first-person update, inferred from shared weapon-view records. Can reach sound via 166992 → 126360. | Yes |
| `0x00171d90` | 0.25 | 0.48 | 0.48 | Observer 16f280 → 170fd0 → 171d90 | Observer camera result, including derived vertical FOV. Particle inputs and camera collision consume its result. | Yes |
| `0x000c7840` | 0.26 | 0.42 | 0.39 | Tick → pre → bc470 → 108c60 ⇢ c6b30 → c60c0 → c7840 | Unit update, vector math. Unit descriptor +40 selects c6b30. Exact meaning of the computed vector is unresolved. | Yes |
| `0x002195f0` | 0.08 | 0.38 | 0.38 | Sound service → 1293f0 → 12a810 → 2195f0 | Sound gain conversion, inferred from the clamp and x87 exponential. 1251e0 is the most visible immediate caller. | Unknown |
| `0x00030bf0` | 0.23 | 0.37 | 0.37 | Weapon fire → cafc0 → 30bf0 | Three-vector length. Used by damage, camera, unit and light code. | Yes |
| `0x00249170` | 0.14 | 0.47 | 0.37 | Tick → 176e40 → 1792b0 → 179020 → 178c80 → 175270 → 248970 → 247470 → 247950 → 249170 | Particle emitter initialization consumes the engine RNG behind 4e7408. | Yes |

The instruction list identifies 11 more owners. Their hottest instruction can rank highly even when the function spends little total time on CPU. Together these add 1,489 self samples (1.77 percentage points).

| Address | Self % | Inclusive % | With services % | Static caller chain to this address | Subsystem and operation | Sim-needed |
|---|---:|---:|---:|---|---|---|
| `0x0028c470` | 0.23 | 0.24 | 0.24 | Animation node evaluator ⇢ codec table 47fb14, record 1 → 28c470 | Static animation rotation decode. | Yes |
| `0x000682e0` | 0.08 | 0.09 | 0.09 | Tick loop 137f70 → Tick → 682e0 | Builds the per-tick simulation record, including its 0x4048-byte initialization. | Yes |
| `0x0028c9e0` | 0.25 | 0.26 | 0.26 | Animation node evaluator ⇢ codec table 47fb14, record 4 or 6 → 28c9e0 | Interpolated byte-keyframe rotation decode. | Yes |
| `0x00128ca0` | 0.21 | 0.68 | 0.22 | Sound service → 1293f0 → 128ca0 | Per-record sound service. It reads the sound record and tag entry. | Unknown |
| `0x00221a70` | 0.07 | 0.28 | 0.07 | Main loop 12b450 → Sound service → 221a70 | Listener/environment processing, inferred. Calls 221e30 → 1de630 for BSP queries. | Unknown |
| `0x00173ba0` | 0.16 | 0.38 | 0.17 | Particle update 247140 → 2ba3a0 → 2b8da0 → 173ba0 | Evaluates active particle inputs. Reads view state through 248df0. | Unknown |
| `0x00129380` | 0.07 | 0.17 | 0.07 | Main loop 12b450 → Sound service → 129380 | Walks sound records and their flags. | Unknown |
| `0x001de630` | 0.10 | 0.68 | 0.10 | Weapon fire → 1a4c00 → 1697c0 → 1de630 | Collision BSP vector-test wrapper. Also under sound service, with 254 observed caller samples from 221e30. | Yes |
| `0x001236f0` | 0.25 | 0.26 | 0.26 | Animation resolve 1daea0 → 1dd9d0 → 1236f0 | Animation cache load/prediction request. | Yes |
| `0x00159ac0` | 0.08 | 0.15 | 0.09 | Tick → game-engine update 15c150 → 159ac0 | Game-mode update reading game and engine globals. Precise operation unresolved. | Unknown |
| `0x000b89b0` | 0.25 | 0.27 | 0.27 | Weapon fire → 1a4c00 → 1697c0 → b89b0 | Datum walk inside the object collision query. | Yes |

### Headless guest dependencies

- **First person.** At `1380a0` the tick calls `165dc1`. Its four slot checks at `165de2` already bypass `166d75` and `16760c` for unmapped local users. A signed-in local player has a mapped slot. Sim hosts leave all four slots unmapped. In that case `166d75` advances the second engine RNG word (`[4e7408]+4`, writes at `1674f9` and `167539`). The pose path through `16760c → bd970 → 1cb410 → 1cb3b0 → 1dd630` can select either RNG word, including the hashed seed at `1dd6e1`. Evaluation also requests shared animation resources (`1678d7 → 1c6f60 → 1dd9d0 → 1236f0`). Effect update `176e40 → 1792b0 → 179fb0 → 17aec0 → 1664da` reads the resulting hand node matrices, and the observer reads the transform at slot `+2098` through `16ebf0`. Keep the weapon and animation update for every mapped seat.
- **Lights.** `c1400` owns the datum walk, expiry and spatial links. `c0c80` can evaluate an attached object's functions at `c0dc4` and change light-list membership through `c2d00` or `c3220`. Under `impl.sim.native.objects` the walk, the record update, its modulation step `c0b00` and the list operations are Rust bodies ([object-update.md](../engine/object-update.md), "Slice 5 bodies"), so a placed light's update runs with no runtime dispatch between it and the walk; the lists themselves (`1cac60`, `1cae40`, `16ba40`) stay guest code.
- **Observer.** `171d90` rewrites the per-seat basis at `+20..+37`, FOV at `+4c` and scale at `+50`. After its call at `171291`, `170fd0` uses the basis to place the camera and calls `171830` for collision. Particle input at `248a31` uses that same scale. The observer also reaches the shared resource cache through `3f450 → 3bcb0 → 12ce00`. Keep camera derivation, collision and resource requests. [Render and simulation dependencies](../graphics/notes/pipelined-render.md)
- **Sound.** `127805` passes a listener point to `11bed0`, which calls `14a280` and returns the leaf, cluster and BSP. The listener stores the result at `127b04..127b0d`. Code at `221dd6..221e14` uses the cluster for a transmission threshold. The verdict at `127d9e` affects nearest-listener selection. Substituting a constant can change which voices play and when their packets complete, even on the silent clock. Keep the listener update, BSP queries, sound service and voice scheduling. Weapon collision shares `14a280`. [Listener ownership](../game/bots.md#43-hiding-the-views-traced) · [Clock-only host](../live/dedicated-host-light.md#light-host)

- The seats run the whole prediction walk at `3f549 → 16e5e0` in Rust: the list is read, each entry dispatched by its type, and every cache request made as a guest call, with the same success accumulation (docs/engine/sim-rewrite.md, phase 7). The cluster's neighbour scan and its separate call at `3f619` stay guest code. An entry type an edited jump table names beyond the nine the image owns declines to the original, as does a count too long to trust. The switch is off by default. With it on, `impl.sim.observer-resources` stands down at this point, because the walk covers it; with it off, the trim above keeps the headless host from executing the mode-1 types 0, 7 and 8. [Cluster resource layout](../formats/structure-bsp.md)
- The sound arm computes the mix-in room and filter descriptors and submits both setters in guest order. Either setter can deliver a pending stream callback. After the first call the pass reloads the mix-in handle for the filter. Silent hosts use the same submissions on the packet clock. [Sound simulation](../engine/sim-rewrite.md#sketches) · [Callback delivery](../audio/dsound.md)

Sound simulation is opt-in on both light and playback hosts.

## Subsystem boundaries

[sim-attribution.md](sim-attribution.md) supersedes the table below. It gives every sample an owner at any call depth, resolves "Multiple" and "Unreached", and splits the runtime share (6.8 points of it are the bots' Rust code, charged to `handler::execute` by the profiler's category rule).

This static census includes **all 59,690 guest self samples**. Starting at the entries listed, follow up to three direct calls or tail jumps, stopping before entering another group's named root. Also follow the verified unit-update, unit-pose and object-import descriptor edges shown above. Charge each sampled function's own code to the single group that reached it, or to “Multiple” if more than one did. “Unreached” includes deeper chains and unresolved indirect calls. A unique static match is a rewrite lead. It does not establish which caller ran, especially for helpers used by cold or unrecognized paths. Percentages in this table alone use guest self samples as the denominator.

| Static group | Root addresses (hex) | Self samples | % of guest |
|---|---|---:|---:|
| Physics / collision | 1c3d30 1697c0 1de630 14a280 2fa2f0 2ee610 2e96a0 300150 301800 | 5,772 | 9.67 |
| Objects / units | b6f10 b7060 b7150 bc470 bd090 c6b30 b8d30 bb050 108d90 | 6,719 | 11.26 |
| Animation | 1c6f60 1dd9d0 2798a0 28c470 28c530 28c9e0 1236f0 | 3,118 | 5.22 |
| Players / game mode / weapons | 14d3a0 14d8b0 15c150 15ca10 104150 d9640 1870a0 | 3,350 | 5.61 |
| Effects / particles / lights | c1400 17a8a0 247140 249170 176e40 173ba0 | 2,432 | 4.07 |
| Sound | 125b40 127400 1293f0 2195f0 221a70 | 3,606 | 6.04 |
| Camera / first person | 1554b0 16f280 23c110 165dc1 | 2,844 | 4.76 |
| AI / scripts | 1c83c0 209310 | 35 | 0.06 |
| HUD / UI | 1479c3 147af1 | 364 | 0.61 |
| Network / sim replication | 8df50 8dfc0 682e0 | 544 | 0.91 |
| Render | 223240 2234e0 2b5d0 2b790 | 25 | 0.04 |
| Multiple | — | 8,511 | 14.26 |
| Unreached | — | 22,370 | 37.48 |

Among the first 30 rows, collision/physics and its spatial queries account for **4,544 self samples (5.39%)**. Object update, animation and their shared math account for **4,585 (5.44%)**. The three first-person/observer candidates account for 712 (0.84%), tick lights and particle initialization for 538 (0.64%), and the guest sound gain conversion for 67 (0.08%). These are disjoint counts of named leaves. The other guest self samples do not acquire an owner merely because a static path to them exists.

| Entry | Samples with entry anywhere on stack | Consequence for sim |
|---|---:|---|
| Pre / post / final object update | 140 / 118 / 82 | Keep damage, transforms, markers and per-type callbacks. |
| World physics / AI / scripts | 134 / 7 / 18 | Keep the world step and gameplay state machines. Seven visible AI frames give no bound on the cost of its detached callees. |
| Effect part create / particle update | 17 / 0 | Effect parts can spawn objects or apply damage. Particle initialization alone has 396 visible samples and consumes the shared RNG. |
| Director / observer / first-person camera | 83 / 490 / 97 | Unrendered views still publish state. Camera collision updates shared visit marks, and particle inputs read view data. |
| UI update / UI render layers / render frame | 52 / 0 / 0 | UI update also services the lobby. The existing world-frame feature-plan skip already removes the draw. |
| Network pre / post update | 27 / 24 | Session and packet work matters to a light host and phantom seats. |
| Sound service | 276 | Polling, media progress and completion affect the guest. Listener and collision children are also visible without this parent. |
| Rust sim metrics / telemetry snapshot | 168 / 118 | Inclusive host stacks. 38 contain telemetry motion reads across the profile. Physics inspection walks can be omitted in the sim observer. |

The guest entries above are lower bounds on an unwound main-thread stack, with overlapping samples. For an inclusive phase measurement use the existing host-audit CPU hooks in a separate run. The normal sim's hooks and the default renderer need no change for attribution. [Host audit](../runtime/sim.md#host-audit-corpus) · [Observed camera and particle dependencies](../graphics/notes/pipelined-render.md) · [Light-host pruning constraints](../live/dedicated-host-light.md#measure-before-pruning)

## Changes and verification

- **Silent audio.** All 801 `Stream::play_frames` samples (0.95% of the thread) occur in the clock-only discard path. This profile was already advancing a logical clock without producing PCM. The packet clock advances exact common sample-rate ratios in batches. Other rates retain the renderer's repeated floating-point additions to preserve completion boundaries. Silent spatial buffers compute Doppler alone. The guest sound service, callback grid, completion order and buffer pitch still run. The sampled stream function sets a **0.95-point ceiling** on its own savings. The profile gives no separate measurable pan/cone saving.
- **Sim observation.** The sim skips the optional Havok body and character-contact diagnostics in its private player snapshot. Those reads supply the inspector and never feed the pads, scoring or seat binding. The old snapshot path is 118 samples (0.14%) inclusive. Only part of that time can be saved. A fixture compares both snapshots and takes another full read from the same guest memory.
- **Native SIMD lanes.** The native ports behind `impl.sim.native_*` model each packed SSE `addps`, `subps` and `mulps` the guest executes. When MXCSR rounds to nearest with PE sticky and all eight operand lanes are zero or within `[2^-63, 2^63)`, one host vector instruction (NEON on AArch64, SSE2 on x86-64) performs the range check and the operation. This covers transform composition `142a60`, pose matrix `1421f0`, the blend-screen codec `28c530` and the float MOPP clip blends under `3009e0`. Other operands use the bit-exact SSE model, and x87 sequences stay scalar. `impl.sim.native_simd` (default on) selects the host lanes. Turning it off restores the scalar per-lane loop for A/B timing. Both paths produce identical bits. The differential harness runs every native case both ways against the lifted guest, and a unit test without the XBE compares the lanes with the scalar loop and the SSE model. Marker transforms and unit pose make no packed float operations. The integer MOPP box walk decodes one node at a time, and each two-child visit is a guest call. Neither function has SIMD work to batch. [Lanes](../../crates/rewrites/sim-core/src/lanes.rs)
- **Object node matrices.** `impl.sim.native_pose_matrix` also replaces the pose root `bd090` with a native port (`OBJECT_NODE_MATRICES`). The port picks the orientation block, runs the root transform, then walks the skeleton from node 1. For each node it runs the pose matrix `1421f0` and the in-place composition with the parent `142a60` through the exact `sim_native` bodies. It writes the same pushes, return addresses and leaf frames as the guest calls would. The walk therefore makes no guest calls, and the two leaves no longer pay a dispatch round trip per node. The other 17 callees, including animation (`bdb60`, `1c6440`), component nodes `1d3920` and the root transform `bdc40`, stay guest calls that resume at their return sites. Removed: `bd090` self 0.53% and its share of `1421f0` (0.60% self; first-person `1dd1c0` and `1dd290` also call it) and `142a60` (0.98% self across about 80 call sites, `bd090` the top chain). That is an upper bound of about 2.1% of the main thread, and roughly 1–1.5 points after the cost of the Rust bodies and the first-person and marker callers that keep the guest leaves. The differential `object_node_matrices` steps the guest through both leaves and compares every call boundary. [Port](../../crates/rewrites/engine/src/sim_physics/node_matrices.rs)
- **Animation codecs and the root transform.** Innermost-guest counts from the same `profile.folded` (84,358 samples), charged to Ghidra function owners, split the animation code into: channel codecs `28c400–28d000` 1,127 (1.34%), node evaluators `27a3c0–28c400` 991 (1.17%), stream selection `279000–27a3c0` 710, animation manager `1c6000–1c8000` 874, graph resolution `1da000–1de000` 1,314 and `1cb000–1ce000` 679. Besides the blend-screen codec `28c530` (264), the codec port covers the four other hot codecs: static rotation `28c470` (211), single-frame rotation `28c960` (322), two-frame rotation `28c790` (93) and keyed translation `28cb70` (221). That replaces 847 samples (1.00%) of guest codec work. Each call still enters from a guest evaluator. `impl.sim.native_transform_compose` covers the root transform `bdc40` (142, 0.17%), which the node-matrix port runs inline. The evaluators are about 40 specialized variants, selected by stream mode (`504450`), blend kind (`504454`), node mask (`50447c`) and flag (`5044a0`), each 0.5–1.5 KB. Each loops over three channels and calls the codec slot `504484/88/8c` per node. The hottest are `280ac0` (197), `281370` (143), `27ace0` (139), `27d420` (120) and `2818e0` (117). The additive-layer evaluator `280ac0` has a native port alongside the codecs. It samples the primary and secondary frames per channel, blends them, then multiplies rotations into the pose, adds translations and multiplies scales. A slot that holds one of the five native codecs runs inline with the guest's return address pushed, so those channels make no guest call per node. Other codecs, including every scale codec, are still guest calls that resume at their return sites. [Codecs](../../crates/rewrites/engine/src/sim_native/codec.rs) · [Root transform](../../crates/rewrites/engine/src/sim_native/root.rs) · [Evaluators](../../crates/rewrites/engine/src/sim_native/evaluator.rs)

  Retired: `impl.sim.native.anim` ([anim-sampling.md](../engine/anim-sampling.md)) replaces `2798a0` and everything under it, so the per-function switches for the five codecs, `280ac0` and the stream call are deleted. Their bodies stay as differential-test oracles.
- **Runtime and other host work.** Dispatch execute and run loop self shares are 6.25% and 4.54%, guest reads 1.36%, exit/charge 0.84%/0.69%, resume/resolve 0.78%/0.53%, context creation 0.73%, fast acquisition 0.52%, x87 small division/compare 0.56%/0.44%. The runtime lane owns these. XAPI input sync accounts for 0.82% self and services controller state. Kernel file dispatch accounts for 0.67% self, 572 visible samples including children. The profile carries neither filenames nor per-call phase information. Animation and sound have resource-request paths, but a static path cannot identify the sampled file operations.

For the coordinator's comparison, use the same image, map, seed, roster and profile. With `H2_FEATURES=impl.sim.arena-hash,-impl.audio.packet-clock,-impl.sim.metrics-projection`, establish same-build A/A repeatability first. Then enable one optimisation at a time, with arena hashing enabled in both runs. Compare every `state_hashes_v1` tick, gameplay digest and arena digest, including the number and range of ticks. A gap, missing arena, or early end leaves parity unverified. Use the phantom-only Waterworks and Lockout fixtures below. For throughput, disable arena verification and the profiler in both arms. The new default gameplay digest runs in each arm. The current evidence establishes neither a 2× speedup nor Waterworks parity. [Sim report and controls](../runtime/sim.md#post-tick-state-hashes)

### Matched light-profile A/B

Use one optimised coordinator-built CLI and its full clang image for every arm. Set `H2_DISC`, `H2_IMAGE` and `H2_DATA` to absolute paths. The input data directory needs bot navigation meshes for both maps. Run the following in Bash with jq installed. Both cases use seed 7, team Slayer, a 1,800-tick limit and four legendary phantoms. Each process starts with fresh writable data and the same navigation files.

```bash
set -euo pipefail
out=$(mktemp -d "${RUSH_KEEP:?}/guest-skips.XXXXXX")
unset H2_PROFILE H2_TIMELINE H2_HOST_AUDIT H2_LOCKSTEP H2_HOST_MODULE H2_SERVER_MAP_DIR
off='-impl.sim.native.sound'
on='impl.sim.native.sound'

run_arm() (
    arm=$1; pass=$2; arena=$3
    case $arm in
        off|repeat) flags="$off" ;;
        all)        flags="$on" ;;
        *)          flags="$off,impl.sim.$arm" ;;
    esac
    scratch=$(mktemp -d "${TMPDIR:?}/h2-guest-skip.XXXXXX")
    trap 'rm -rf -- "$scratch"' EXIT
    ln -s "${H2_DATA:?}/bots" "$scratch/bots"
    report="$out/$case_id.$pass.$arm.json"
    H2_OPT=2 H2_FEATURES="host-light,$flags,impl.sim.arena-hash=$arena" \
        h2 sim run "$out/$case_id.json" --disc "${H2_DISC:?}" --out "${H2_IMAGE:?}" \
        --tier clang --data "$scratch" --pace unpaced --report "$report" >"$report.log" 2>&1
    jq -e --arg arena "$arena" '
        .result.end == "tick_limit" and .nav == "loaded" and .ticks == 1800 and
        (.state_hashes_v1 | length == 1800 and
            ([.[].tick] == [range(.[0].tick; .[0].tick + 1800)]) and
            all(.[]; (.gameplay | test("^[0-9a-f]{16}$")) and
                ($arena == "off" or (.arena | test("^[0-9a-f]{16}$")))))' "$report" >/dev/null
)

for case_id in lockout waterworks; do
    jq -n --arg map "$case_id" '{
        map: $map, game_type: "slayer", seed: 7, team_play: true, time_limit_s: 0,
        score_limit: 250, max_ticks: 1800, max_wall_s: 900, render_every: 0, pace: "unpaced",
        bot: [range(4) as $i | {name: (["Red 1","Blue 1","Red 2","Blue 2"][$i]),
            skill: "legendary", team: ($i % 2)}]
    }' > "$out/$case_id.json"
    for arm in off repeat native.sound all; do
        run_arm "$arm" parity on
        diff -u <(jq -c '."match", .state_hashes_v1[]' "$out/$case_id.parity.off.json") \
                <(jq -c '."match", .state_hashes_v1[]' "$out/$case_id.parity.$arm.json")
        jq -e '.state_components_v1 | .truncated_after == null and
            (.ticks[0].changed | has("sound.voices"))' "$out/$case_id.parity.$arm.json" >/dev/null
        result=$(h2 sim diff "$out/$case_id.parity.off.json" "$out/$case_id.parity.$arm.json")
        [[ "$result" == "identical: 1800 ticks, every component agrees" ]] || { echo "$result"; exit 1; }
    done
    for pass in 1 2 3; do
        arms=(off native.sound all)
        if [[ $pass == 2 ]]; then arms=(all native.sound off); fi
        for arm in "${arms[@]}"; do run_arm "$arm" "$pass" off; done
    done
    for arm in off native.sound all; do
        for pass in 1 2 3; do
            diff -u <(jq -c '."match", .state_hashes_v1[]' "$out/$case_id.1.off.json") \
                    <(jq -c '."match", .state_hashes_v1[]' "$out/$case_id.$pass.$arm.json")
        done
        jq -s --arg arm "$arm" --arg id "$case_id" \
            '{case: $id, arm: $arm, median_ms_per_tick: (map(.wall.ms_per_tick) | sort | .[1])}' \
            "$out/$case_id".{1,2,3}."$arm.json"
    done
done
```

The `repeat` arm checks baseline repeatability before any flag comparison. A diff shows the first differing tick, including whether the gameplay or checkpoint-arena digest changed. If A/A differs, keep both reports and resolve repeatability before attributing a difference to a skip. For each case report each arm's median, its difference from `off` in ms/tick and the ratio `off / arm`. Only an all-on ratio of at least 2 meets the throughput target. For guest attribution, run the pair below in the same shell with a profiling-enabled CLI. Compare on-CPU guest sample shares and per-function counts across the same 1,800 ticks. Use the unprofiled medians for the speedup. The profile gives no isolated savings estimate for observer prediction or the sound pass.

```bash
case_id=waterworks
for arm in off all; do
    H2_PERF_FROM=level H2_PROFILE_HZ=1000 H2_PROFILE_CPU=1 \
        H2_PROFILE="$out/profile-$arm" run_arm "$arm" profile off
done
```
