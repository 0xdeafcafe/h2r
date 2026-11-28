# Simulation rewrite, phase 5: the Havok world

Status: phase 5 landing, 2026-11-25. Built, with unit tests only: the
`sim-havok` kernels (motion, support, welding, the sweep, the solver, the
pool, structure polygons, the accumulators) at
`crates/rewrites/sim-havok`; the engine adapters - the oracle ports'
fast paths (`sim_physics/support.rs`, `weld.rs`), the world entries and
capture arms, always installed
(`crates/rewrites/engine/src/sim_physics/{world,broadphase,motion,parity}.rs`),
installed by `sim-bridge/src/sim_physics.rs`; the heap digest
(`live-telemetry`'s `havok_agents::digest`, `havok.digest`, the sim
report's `havok.world` component); the shadow's `compare_only` arm for
guest-authoritative candidates; the broadphase batch update's, the
island integrate's, the deactivation test's and the integrate phase's
answering arms (§9.5, [havok-integrate.md](havok-integrate.md)); the
structure polygon builder's answering arm (slice 4,
[havok-structure.md](havok-structure.md)); slice 5's answering arms
([havok-convex.md](havok-convex.md)): the convex agent's separated
pairs, the closest-feature query whole (its shapes' virtuals, triangle
case `303dd0` and penetration call `30a6b0` in Rust), the add-point
call on a point that moves its slot, and the contact commit `318c10`
to its listener walk (Halo's callback and the tail stay lifted);
the island
solve entry `31c100` (slice 2). The per-function `native_havok_solver`,
`native_havok_support_vertices`, `native_havok_weld` and `native_mopp_box`
switches are retired; their bodies are differential oracles. The ladder
beyond the differential tests (disc sweep, run parity, the behaviour
suites) and the timing gate are open: the collide phase `30fcb0`, the
integrate phase's declined paths (pending work, listeners, splits, island
deactivation), the manifold refresh `302eb0` (ported and compared, no
arm), the add-point call's new slot and its cut `302a10` (ported and
compared, no arm: they call the commit, whose listeners are Halo's), a
constraint's first point (`30d2b0`) and the other agent/narrowphase
leaves still run lifted, and
slice 3's runs, lanes and worker solves are not built (each island reads
the solver buffer bytes the island before it left, so a run's islands
are not independent without a read check).

This is the phase 5 spec of [sim-rewrite.md](sim-rewrite.md), written
after the user's decision 8.1: **physics stays bit-exact with retail**
(bar E, a structural port). It replaces that page's phase 4 sketch, which
assumed bar D.

Read with [havok-runtime.md](havok-runtime.md) (layouts),
[sim-budget.md](../perf/sim-budget.md) (the profile) and
[FLOAT.md](../lifter/FLOAT.md) (the float contract). The three existing
ports in `crates/rewrites/engine/src/sim_physics` (`havok.rs` with `solver_rows.rs`
and `solver_pairs.rs`, `weld.rs`, `support.rs`) are oracles. They are not
the architecture: they model registers and the guest stack, which this
design drops.

Static reading of the owner's `default.xbe` with `h2 disasm`. Nothing here
was checked in a running game. No Havok SDK source or headers were used;
names are descriptive. No game data is on this page.

Evidence labels: **Code** (read from the disassembly at the address given),
**S** (inferred; check before building on it), **Open** (not known).

## 1. Decisions

- **Truth stays in guest memory.** Every Havok object (world, islands,
  entities, motions, shapes, agents, contact constraints) keeps its retail
  layout at its retail address. Rust owns the *work* of the step and three
  kinds of state: static caches, derived indexes, transient working sets.
  This is rule (c) of sim-rewrite.md §2, unchanged.
- **Compute, then commit.** A Rust kernel reads guest state into a
  transient SoA working set, computes with no guest access, and writes
  results back at the point in the retail sequence where retail wrote
  them. Three things follow, and the whole design rests on them:
  1. scratch memory (the solver buffer, the LIFO stack) is built in Rust
     and stored once, at the solve's end, with the bytes retail leaves
     there, because the next island's fixed accumulators read them;
  2. a fast-path float guard that fails can restart the kernel on the
     exact path, because nothing was written yet;
  3. computation may run early, in lanes or on workers, when its captured
     inputs are verified bit-equal at its retail position (§7.3).
- **Float order is retail's, everywhere.** Speed comes from removing the
  recompiled code and its dispatch, from static decode of the structure
  geometry, and from running *independent* units side by side. No float
  expression is reordered, refactored or fused.
- **No switch.** The world step and its arms always install.
  No per-slice switches and no environment variables. Fallback to the
  lifted code is by *data* (an island or pair the port does not cover runs
  the lifted entry), not by a switch.
- **The allocator is ported exactly** and every allocation happens in
  retail order. Heap addresses are persistent state (§3.1).

## 2. What retail does in a step

All **Code** unless marked. `W` is the world (`[0x0051e9a4]`), `I` an
island.

```
0x001c3d30  Halo wrapper
  0x00146bf0                  memory-pressure check (may tear the world down)
  _controlfp, MXCSR |= 0x1f80 all exceptions masked; precision untouched
  0x00311100 (W, &step)       step = { dt, 1/dt }
    0x0030fcb0  collide
      copy step to W+0x150; derive W+0x170.. from W+0x17c, W+0x180
      W+0x12c = 1 (locked); 0x0030f6b0 if W+0x24 > 0 (pending operations)
      for i in 0 .. W.active_count:                       (forward)
        0x00317d30 (I, broadphase W+0xc4, input W+0x140)
          0x00278e60 (I)        Halo: record overflow-stack use
          0x003175e0            AABBs, broadphase update, agents added/removed
          0x00317950 or 0x00317c00 (by W+0x23e)   process every pair's agent
          0x00278e80 (I)        Halo: flag I+0xae if overflow use changed
        0x003188c0 if W+0xac    listeners, per island
      0x0030f6b0, 0x0030fc30 (W+0xf4), 0x0030cda0 (W+0x100), 0x003187a0
    0x0030fef0  integrate
      0x00175f40 (empty), 0x0030f6b0, 0x0030ce20 (W+0x10c)
      for i in W.active_count-1 .. 0:                     (reverse)
        if W+0x23d and 0x00316b30 (I) says so: 0x0030cc60 deactivate I
        else 0x00316a20 (I, W+0x140); I+0x30 = 0
        0x00318830 if W+0xa0    listeners, per island
      0x0030e720 if W+0x23c     split islands
      0x0030fc30, 0x0030cda0, then 0x0030d400 over the array at W+0x114
  0x00278f00, 0x001c4260      Halo: overflow handling, bodies to objects
  0x001c4a20 per entity       Halo: object update from its body
```

`0x003175e0` (island broadphase): two local pair arrays (capacity hint
`W+0x238`); LIFO blocks of `n × 0x20` AABBs and `n × 4` handles; per
entity in array order, `shape->vt[+0x18]` (transform, tolerance, out) with
the tolerance `[W+0xcc]+8` times a constant; `broadphase->vt[+0x14]`
(handles, AABBs, n, new pairs, deleted pairs); the island's queued pairs
(`I+0x74`, count `I+0x78`) appended; `0x003173f0` creates and destroys
agents; `I+0x30 = 1`.

`0x00316a20` (island integrate): every action in `I+0x4c` (count `I+0x50`)
gets `vt[+0x14]`; with no constraints (`I+0x5c == 0`) each entity in
*reverse* order gets `motion->vt[+0x08]` then `vt[+0x04]`; otherwise
`0x0031c100`.

`0x0031c100` (island solve), cdecl, nine arguments (step `W+0x150`, solver
info `W+0x160`, pair scratch `W+0x190`, `W+0xe0`, island, entities, count,
constraints, count):

1. Take the solver buffer `[0x00484b38]` (`+0x08` base, `+0x0c` size,
   `+0x10` busy). If the island's sizes (`I+0x08..+0x18`) do not fit, take
   LIFO blocks from the memory system `[0x00480118]` instead.
2. Accumulators, `0x80` bytes each: a zeroed one of type 2 first, then one
   per entity in array order. `motion+0x08` receives the accumulator's
   offset from the buffer base (a persistent write). `motion->vt[+0x0c]`
   fills it. A type 3 byte ends the list.
3. Per constraint in array order: the two accumulator addresses go to
   `W+0x190`/`+0x194` (persistent), then `constraint->vt[+0x24]` appends
   its Jacobian rows and schemas (`0x0031f620` for a contact).
4. End schema `0x400`; `I+0x18` impulse words zeroed.
5. `0x0031c940` solves. When it returns 1, each entity in array order gets
   `motion->vt[+0x10]` (step, `[info+0x20]`, accumulator).
6. Release the buffer.

Motion vtables (**Code**, the constructors at `0x003193f7`, `0x0031a6e7`,
`0x0031a884`, `0x0031ada0`): `0x00415130` and `0x004151b0` (no dynamics;
type getters `0x0009ac70`, `0x0009b910`), `0x00415230` (`+0x0c`
`0x0031b300`, `+0x10` `0x0031a8c0`), `0x004152b0` (`+0x10` `0x0031b4c0`).
Shared slots: `+0x04` `0x0031b060`, `+0x08` `0x0031b1c0`.

The solver reads a schema stream. Its dispatch table at `0x0031e6a4` has
17 entries (opcodes 16 to 32), the write-back table at `0x0031e704` has 8.
A row is three vectors: linear `+0x00` (right-hand side in `w`), angular A
`+0x10`, angular B `+0x20`. An accumulator holds linear velocity `+0x10`,
angular velocity `+0x20`, inverse inertia with inverse mass in `w` `+0x30`,
and the per-iteration sums `+0x40`, `+0x50`. Projection is packed SSE; the
impulse arithmetic is x87 (the oracle ports, **Code**).

## 3. Findings that change the plan

### 3.1 The Havok heap is not in the checkpoint arena

sim-rewrite.md §4 says the world is "in the arena". It is not. **Code:**
`0x00146b30` reserves `0x106470` bytes with its own virtual allocation and
stores the aligned base at `[0x00479888]`; `0x00146a20` builds the memory
system there (vtable `0x004576a4`: allocate `0x0022c300`, free
`0x0022c320`, LIFO allocate `0x002d7020`, LIFO free `0x002d7090`), and the
world is allocated from it (`0x001c2ee7`). A second, borrowed block
(`[0x00510c44]`, `0x452e8` bytes, object at `[0x0047989c]`) is created by
`0x00146b80` and destroyed by `0x00146de0` around rendering.

Consequences:

- `lockstep::hash_sim` and the `arena.NN` blocks do not cover Havok state
  directly. They cover it only through what objects copy out of it. Phase
  4 needs its own digest (§9.2).
- Heap addresses still matter. The physics component array
  (`[0x0051e9b8]`) and the world hold entity, shape and agent pointers, and
  `0x00146fd0` turns heap usage into a pressure level that makes
  `0x00146bf0` rebuild the world (`0x001c27a0`, `0x001c2690`). A port that
  allocated differently would change pointers in persistent state and
  change when that rebuild happens. Hence the exact allocator.
- How a checkpoint revert brings Havok back in line with the reverted
  arena is **Open** (§12). The design does not depend on the answer: the
  Rust side holds no persistent state and drops everything derived on
  revert.

### 3.2 Halo code runs inside the step

`0x00278e60` and `0x00278e80` bracket each island's collide and compare
the overflow stacks' use (`0x00147090`) before and after; a change sets
`I+0xae` and `[0x0055e4fc]`, and `0x00278f00` acts on it after the step.
`0x00278eb0`/`0x00278ed0` do the same around the pending operations in
`0x0030f6b0`. So *whether the LIFO stack overflowed* is behaviour. The port
therefore replays every LIFO allocate and free on the allocator's
bookkeeping, with retail's sizes and order, without using the memory.

### 3.3 The monitor stream must be off

Many functions append a timer record with `rdtsc` when
`[0x00480154] < [0x00480158]`. `weld::supported` already requires
`head >= end`. The port keeps that as an entry condition for the whole
step; when it does not hold, the lifted step runs.

### 3.4 x87 here is double arithmetic

The game pins PC=53, round to nearest, exceptions masked (FLOAT.md), and
the wrapper changes only the exception masks. An x87 chain in the solver
is therefore a chain of binary64 operations in the guest's order, with a
binary32 rounding at each store to memory. That is cheap and identical on
arm64 and x86-64 (§6).

## 4. Measured cost

Waterworks, four local bots, 84,358 main-thread samples
(`prof-ww/profile.folded`). "With services" charges the innermost guest
frame, including host services it called. Function ownership uses the
Ghidra boundaries.

| Family | Address band | With services | Share | Self |
|---|---|---:|---:|---:|
| Solver, Jacobians | `0x31c100..0x320500` | 852 | 1.01% | 187 |
| World, islands, contact manager, motions | `0x30a000..0x31c100` | 1,277 | 1.51% | 506 |
| Broadphase | `0x370000..0x379c60` | 408 | 0.48% | 146 |
| Biped (type `0x17`) agent and shape | `0x2d9a00..0x2dc800` | 651 | 0.77% | 182 |
| Shapes | `0x2dc800..0x2e8000` | 368 | 0.44% | 120 |
| BV-tree and convex agents | `0x2e8000..0x2f9000` | 619 | 0.73% | 236 |
| Welding agents | `0x2f9000..0x2fc000` | 789 | 0.94% | 243 |
| Other narrowphase | `0x2fc000..0x2ff800` | 325 | 0.39% | 110 |
| Integer MOPP box | `0x2ff800..0x3009b0` | 535 | 0.63% | 254 |
| Closest features, manifolds | `0x301a00..0x30a000` | 879 | 1.04% | 229 |
| Halo structure shapes | `0x1ea000..0x1f0400` | 861 | 1.02% | 275 |
| Halo physics components | `0x1ce000..0x1d8000` | 1,019 | 1.21% | 358 |
| Halo wrapper, listeners, memory | `0x1c2000..0x1c5000`, `0x277000..0x279800`, `0x22c000..0x22cd00`, `0x146a00..0x147200` | 502 | 0.60% | 210 |
| **Total** | | **9,085** | **10.77%** | 3,056 |

Left out as phase 2's: the ray paths `0x3009e0`, `0x301800`, `0x2df720`,
`0x2df680`, `0x2df760`, `0x244980` (1,588 samples, 1.88%).

- Narrowphase against the level is the cost: 4,659 samples (5.52%) across
  the agents, the MOPP box walk, feature tracking and Halo's polygon
  builder (`0x1edd00`, `0x1edf50`, `0x1ef950`, `0x1eea50`, `0x1ef3e0`,
  `0x1ee350`). The solver is 1%.
- With the plan's runtime tax (runtime 23.77% against guest 70.76%, so
  ×1.336) the family is about 14.4% of the thread: **2.0 ms of the
  14.12 ms tick**. That is the ceiling.
- Estimate: 1.2 to 1.5 ms removed with everything through slice 8 on one
  thread. The plan's exit figure (≥ 1.5 ms) is at the top of that range;
  §11 gives each slice its own figure.
- Caveats. Trampolines hide ancestors, so the two Halo bands include calls
  from outside the step (object update, Halo's own collision queries);
  they are an upper bound. The profile has bots and no vehicles, ragdolls
  or AI. The co-op campaign census (decision 8.3) will move the split.

## 5. Ownership and invalidation

| Kind | What | Lifetime | Dropped when |
|---|---|---|---|
| Persistent | Every Havok object, in the guest heap, retail layout | retail's | never by Rust |
| Static | `StructureGeo`: per-surface polygons, pre-decoded MOPP | map load | map epoch |
| Derived | `BodyIndex` (entity address to dense row), decoded shape records, per-island stream signatures | any time | world epoch, or failed validation |
| Transient | the working set of one kernel call | one Rust call | on return |

**World epoch** bumps on: the world constructor `0x003101c0` and its
teardown (`0x001c27a0`); revert `0x00123f60`; full restore `0x00124380`;
snapshot load; map load; a tick number that went backwards. Everything
derived is dropped.

**Validate, do not trust.** A derived record carries the guest bytes it
was decoded from (a capsule is `0x30` bytes, a transform shape `0x50`).
Every use compares those bytes first and re-decodes on a mismatch. A
derived record can therefore never change a result, and guest code that
rewrites a shape in place (crouch rebuilds go through `0x001c3770`) needs
no hook. The cold-cache arm (§9.4) proves it.

**Not mirrored:** agent caches, contact points and properties, island
arrays. They are small, they are the persistent truth, and they are read
from guest memory by each kernel call.

## 6. Float rules

Kernels are generic over one trait with two implementations.

```rust
pub trait Fp {
    type E;                                   // guard miss, Fast only
    fn add(a: V4, b: V4) -> Result<V4, Self::E>;   // addps; also sub, mul, div, sqrt, min, max
    fn cmp(a: V4, b: V4, pred: Pred) -> Mask4;     // cmpps, and movmskps on the result
    fn adds(a: f32, b: f32) -> Result<f32, Self::E>; // scalar SSE: addss, mulss, ...
    fn xld(v: f32) -> X;                           // fld m32
    fn xadd(a: X, b: X) -> Result<X, Self::E>;     // also xsub, xmul, xdiv, xsqrt
    fn xst(a: X) -> Result<f32, Self::E>;          // fstp m32
    fn xcmp(a: X, b: X) -> X87Cond;                // C3, C2, C0, unordered included
}
pub type V4 = [f32; 4];
pub struct X(f64);
```

- `Fast` uses host binary32 and binary64 operations: NEON or SSE2 lanes
  for the packed ones (`sim_core::lanes`, extended from add, sub, mul to
  div, sqrt, min, max and compares), plain `f64` for `X`. Every operation
  checks retail's fast-path condition (operands zero or within a normal
  range, as `lanes.rs` proves for binary32 and
  [x87-native.md](../lifter/x87-native.md) proves for PC=53) and returns
  `Err` outside it.
- `Exact` calls the `simd` and `x87` crates: every NaN payload, denormal,
  sticky flag and precision mode. It cannot fail.
- A kernel runs on `Fast`. On `Err` its transient results are thrown away
  and it reruns on `Exact`. Nothing was committed, so the rerun is clean.
  The unit of restart is the kernel call: one island's solve, one pair's
  narrowphase.
- Entry check, once per step: MXCSR is `0x1f80` plus sticky bits; x87 is
  PC=53, nearest, masked, empty stack. Otherwise the whole step runs on
  `Exact`.
- `rcpps`, `rsqrtps` and their scalar forms have no host equivalent that
  matches. If the census (slice 0) finds any under `0x00311100`, both
  implementations call the `simd` crate's model for them.
- No `mul_add`, no platform `libm`, no fast-math (sim-core's lints).

### 6.1 Where bit-exactness forbids reordering

| # | Order that is fixed | Why |
|---|---|---|
| F1 | Operations and operands inside one expression, horizontal sums included (`project` adds `y + x`, then `z + (y + x)`) | rounding; NaN operand choice |
| F2 | Rows within a constraint, constraints within an island (the array at `I+0x58`, as it stands after every swap-remove), iterations | Gauss-Seidel: each row reads what the last wrote |
| F3 | Body A written before body B is read in an impulse application | A and B can be the same accumulator |
| F4 | Pairs within an island's pair list; children within a BV-tree agent (the MOPP walk's emission order); the welding selection order (strictly greater, ties keep their place) | contact creation order becomes constraint order, which is F2 |
| F5 | The broadphase's new-pair and deleted-pair output order | agent creation order becomes pair order, which is F4. The broadphase is ported as the same incremental algorithm; no other structure is substituted |
| F6 | Islands: forward in collide, reverse in integrate, over arrays that deactivation and merging mutate | commit and callback order |
| F7 | Heap and LIFO allocation order and sizes | addresses in persistent state; pressure levels; overflow flags |
| F8 | Every call into code that is still guest (actions, listeners, Halo's contact callback, Halo's hooks), its order and its arguments | they read and write shared state |
| F9 | Persistent writes, as a sequence, between any two guest calls | a guest callee sees memory |

### 6.2 Where it does not

| # | Free | Used for |
|---|---|---|
| A1 | Where scratch lives and whether it is written | Rust-owned accumulators, rows, schemas, AABB arrays |
| A2 | Layout of a working set | SoA bodies, rows by type, lane blocks |
| A3 | Order among computations that share no written data: AABB per body, accumulator per body, Jacobian per constraint (inputs are pre-solve body state), integration per body, narrowphase per pair, solve per island | lanes and workers |
| A4 | Constraints of one island that share no dynamic body with each other, between the constraints they do depend on | wavefront lanes (§7.5) |
| A5 | Decoding immutable inputs once | `StructureGeo`, the MOPP |
| A6 | Integer work whose outputs keep their order | MOPP traversal, sorting networks that reproduce retail's permutation |
| A7 | Writes with no guest call between them | bulk scatter |
| A8 | The dead stack below ESP, registers, the guest stack frame | not modelled (engine README, "Fault behaviour") |

A3 holds only when no guest code runs between the units, or under the
validation of §7.3.

## 7. Design

### 7.1 Crates and modules

```
crates/rewrites/sim-havok/      no runtime dependency; deps: sim-core, simd, x87
  src/mem.rs                    the pools, the dispatcher and the LIFO entries over guest words
  src/fp.rs                     Exact (simd, x87) and Fast (host f32) arithmetic
  src/layout.rs                 world, island, entity, motion and pool offsets
  src/view.rs                   GuestRead: u32, u16, bytes over guest memory
  src/array.rs                  the Havok array header and its growth
  src/motion.rs                 unconstrained motion: forces, pose integration
  src/jacobian.rs               motion accumulators on both sides of the solve
  src/solver.rs                 the sequential-impulse schema interpreter
  src/support.rs                the cached support simplex transform
  src/structure.rs              a structure child's polygon and support feature
  src/weld.rs                   the weld candidate selection from captured distances
  src/broadphase.rs             the incremental three-axis endpoint sweep
  (planned) shape, gjk, manifold, contact, island, step, fixture
crates/rewrites/engine/src/sim_physics/   the adapters and the oracle ports
  world.rs                      the step, island collide and island integrate entries
  alloc.rs                      the four memory-system entries
  broadphase.rs                 the batch update's capture arm
  motion.rs                     the two motion methods' capture arms
  parity.rs                     the arms' counters and the broadphase answering arm
  (planned)                     boundary.rs, callbacks.rs, spec.rs
crates/rewrites/engine/tests/sim_havok_differential.rs (planned)
docs/engine/havok-step.md       the contract page (engine README format)
```

Edited: `crates/hooking/guest-layout/src/points.rs`, `crates/rewrites/sim-bridge/src/sim_native.rs`,
the workspace and engine `Cargo.toml`, `crates/extensions/agent/src/havok.rs`,
`docs/runtime/agent-api.md`. Nothing owned by Live, bots, theater or film.

`sim-core` (`lanes`, `det`, `Epoch`, `Jobs`) comes from phase 1a. If it
has not landed when slice 1 starts, slice 1 creates those four modules at
the paths sim-rewrite.md §5 gives, and nothing else of it.

### 7.2 Data types

Guest addresses are `u32`. Row counts fit `u16` (the component array has
`0x200` entries).

```rust
/// Gathered per island for integrate, per step for collide.
pub struct Bodies {
    pub entity: Vec<u32>,
    pub motion: Vec<u32>,
    pub kind: Vec<MotionKind>,          // by motion vtable; Unknown forces fallback
    pub raw: Vec<[u8; MOTION_BYTES]>,   // the motion, bulk-read; the scatter source
    pub accum_off: Vec<u32>,            // value for motion+0x08
}

/// Retail's 0x80-byte accumulator, same field order, 16-byte aligned.
#[repr(C, align(16))]
pub struct Accum {
    pub kind: u32, _pad: [u32; 3],      // +0x00: 2 body, 3 end
    pub lin: V4,                        // +0x10
    pub ang: V4,                        // +0x20
    pub inv: V4,                        // +0x30 inverse inertia xyz, inverse mass w
    pub sum_lin: V4,                    // +0x40
    pub sum_ang: V4,                    // +0x50
    pub rest: [V4; 2],                  // +0x60, +0x70
}

#[repr(C, align(16))]
pub struct Row { pub lin: V4, pub ang_a: V4, pub ang_b: V4 }   // 0x30

pub struct Group {                      // one header schema
    pub a: u16, pub b: u16,             // accumulator rows; 0 is the fixed one
    pub rows: u32,                      // first Row
    pub ops: Range<u32>,                // into Stream::ops
    pub out: u32, pub out_stride: u32,  // write-back base and stride (0x1c for contacts)
}
pub struct Stream {
    pub accum: Vec<Accum>,              // [0] fixed, then entities in array order
    pub rows: Vec<Row>,
    pub ops: Vec<Op>,                   // decoded schema: opcode 16..=32 and operands
    pub groups: Vec<Group>,
    pub impulses: Vec<f32>,             // I+0x18 words, zeroed
    pub signature: u64,                 // hash of ops and group shapes (§7.5)
}

/// N islands with one signature, field-wise. N is 4.
pub struct AccumLanes { pub lin: [[f32; 4]; 4], pub ang: [[f32; 4]; 4], /* ... */ }

pub struct StructureGeo {               // static, per structure shape container
    pub ring: Vec<Range<u32>>,          // per surface, into verts
    pub verts: Vec<V4>,                 // the ring vertices as the builder emits them
    pub plane: Vec<V4>,
    pub extra: Vec<PolyExtra>,          // whatever else 0x001edd00 writes; slice 4, step 0
    pub mopp: mopp::Program,            // phase 2's pre-decoded form
}

pub enum ShapeRec {                     // decoded once, validated by `src`
    Sphere { r: f32 },
    Capsule { a: V4, b: V4, r: f32 },
    Transform { child: u32, m: [V4; 4] },
    Biped { child: u32, threshold: f32 },      // type 0x17, the pill at +0x30
    Structure { geo: Arc<StructureGeo> },
    Other { vtable: u32 },                     // forces fallback for its pairs
}
pub struct Decoded { pub rec: ShapeRec, pub src: SmallVec<[u8; 0x50]> }

pub struct PairWork {                   // one pair's narrowphase, uncommitted
    pub pair: u32,                      // record address in I+0x68
    pub inputs: InputStamp,             // hash and length of every guest span read
    pub cache_writes: Vec<(u32, SmallVec<[u8; 64]>)>,   // agent bytes, in retail order
    pub events: Vec<ContactEvent>,      // Add{point, normal, dist, child key}, Remove{id}, in retail order
    pub children: Vec<ChildWork>,       // BV-tree: created or destroyed child agents, in order
    pub lifo: Vec<LifoOp>,              // retail's scratch requests
}
```

Sizes: `Accum` 128 bytes; `Row` 48; a `Stream` for a biped on the ground
is two accumulators and about twelve rows, under 1 KiB. All working sets
come from a per-step bump arena that is reset, not freed. Rust state stays
under 2 MiB per match outside `StructureGeo`, which is about the size of
the collision BSP's vertex rings and is shareable (sim-rewrite.md §7).

### 7.3 Speculate, validate, commit

This is the only mechanism that lets work leave its retail position.

1. **Speculate.** A unit (a pair's narrowphase, an island's solve) is
   computed from a snapshot of its inputs. `InputStamp` records every
   guest span the kernel read and a 64-bit hash of the bytes.
2. **Validate.** At the unit's retail position, on the game thread, the
   spans are re-read and compared. Typical inputs are two motions, two
   shapes and one agent: a few hundred bytes.
3. **Commit** the unit's writes, allocations and callbacks in retail
   order, or **recompute** it serially when validation failed.

Exactness does not depend on any claim about what a callback touches. A
callback that moves a body costs a recompute, never a wrong result. Units
with no guest call between them (A3) skip validation.

### 7.4 Collide

- **AABBs.** Per entity, `ShapeRec::aabb(transform, tolerance)` in the
  shape's own operation order. Independent per body (A3); the output
  array is Rust scratch (A1). The two LIFO requests are replayed (F7).
- **Broadphase.** `broadphase.rs` ports the structure behind `W+0xc4`
  (code at `0x372000..0x373c00`) as it is, operating on its guest memory
  through `view`, so endpoint arrays and pair output order are retail's
  (F5). It is 0.48% of the thread; the gain is dispatch, not algorithm.
- **Agent creation and destruction** (`0x003173f0`): serial, in output
  order, through `mem.rs`. Filters and factories that are Havok code are
  ported; a filter that is Halo code is an awaited guest call.
- **Narrowphase.** Per pair, by agent vtable
  ([havok-runtime.md](havok-runtime.md), "Collision agents"):
  - `bvtree.rs` (`0x00412a98`, `0x00412adc`): the cached query box test at
    `0x002e9730`, then the MOPP box query on `StructureGeo::mopp` with keys
    in retail's emission order, then child agents by key.
  - `weld.rs` (`0x00412f5c`, `0x00412f7c`; spheres `0x00412fb0`,
    `0x00412fd0`): candidate collection, the selection order, welding.
    Oracle: `sim_physics/weld.rs`.
  - `convex.rs` (`0x00412c48`) and `t17.rs` (`0x00412290`): the separating
    bound shortcut, `gjk.rs` from the cached feature ids, the manifold
    slots. Oracle: `sim_physics/support.rs`.
  - A structure child's polygon comes from `StructureGeo` instead of
    Halo's builder: no per-step construction, no guest call. The polygon
    shape object the guest would have built on its stack is scratch (A1);
    slice 4's step 0 proves nothing keeps a pointer to it.
  - Any other vtable: the pair runs the lifted `vt[+0x10]`, awaited.
- **Contact commit** (`commit.rs`): the manager's add (`0x00318c10`):
  the first-point constraint add (`0x0030d2b0`, `0x00316cf0`), the point,
  the property insert and the ID map, friction and restitution, then the
  listener walks with Halo's contact callback (`0x00277b80`) awaited,
  then the impulse seed. `Remove` likewise. Serial, pair order, child
  order (F4, F7, F8).
- **Parallel form.** For `Jobs > 1`: speculate every pair of the island
  on workers, then validate and commit in pair order on the game thread.
  For `Jobs == 1` (hosts): compute and commit pair by pair, no stamps.
  Both give the same bits; a test runs 1, 2 and 8 workers.
- Halo's two hooks stay calls to guest code until slice 8 ports them
  (they are four instructions and a sum over two stacks).

### 7.5 Solve and integrate

Per island, in retail's reverse order:

1. Deactivation check (`0x00316b30`): two timers, two thresholds at
   `W+0x30`/`+0x34`, each entity's deactivator through `entity+0x54`. x87
   compares (`test ah,41h`: above, with unordered false).
2. Actions: each `vt[+0x14]` is an awaited guest call unless its vtable is
   in the ported set (slice 3's census).
3. Gather `Bodies` (bulk read of each motion).
4. `motion.rs` builds `Stream::accum`; `jacobian.rs` builds rows and ops
   per constraint in array order. Both are A3 inside one island.
5. `solver.rs` interprets `ops`: the 17 opcodes, with the set and restore
   parameter schemas, the per-iteration velocity correction, the
   write-back pass. The callback schema (`0x0031e0a6`) and any constraint
   class without a builder make the island **fall back**: the lifted
   `0x0031c100` runs for that island, awaited.
6. Scatter in retail order: `motion+0x08`, `W+0x190`/`+0x194`, the contact
   properties' impulse and velocity words, then each motion's integrated
   state. No guest call happens inside `0x0031c100` for a covered island,
   so this is one bulk scatter (A7).
7. Listeners at `W+0xa0`, awaited.

**Lanes across islands.** A *run* is a maximal sequence of consecutive
islands, in retail order, where no island has an action outside the
ported set, `W+0xa0` is empty, and none falls back. Inside a run there is
no guest call, so the islands are independent (A3): they share only fixed
bodies, which the solver reads and never changes in value (see the guard
below). Within a run, islands are grouped by `Stream::signature`; four
islands with one signature run as one lane block, each lane executing its
own island's rows in its own order. Packed operations become four lanes
per component, the binary64 chain two lanes at a time. Unequal row data
is fine; unequal control flow is not, so a lane whose branch (a clamp, a
friction limit, an unordered compare) differs from lane 0's leaves the
block and finishes scalar from its own state. Islands with unique
signatures run scalar. Scatter is in retail order whatever the lane.

**Fixed-accumulator guard.** Retail writes body B's velocity even when B
is the fixed accumulator; the value written is the old one unless an
impulse is NaN or infinite. The kernel computes that write and compares it
with the stored bits. If it would change, that island reruns on the
scalar path with a real shared accumulator. Other islands are unaffected,
because retail gives each island its own freshly zeroed one. Exact by
construction, and never taken in a sane world.

**Wavefront inside an island (A4).** Give each `Group` a level: one more
than the highest level among earlier groups that share a dynamic body with
it. Groups of one level touch disjoint dynamic bodies, and every pair of
groups that share a body keeps its relative order, so executing level by
level gives the bits of the sequential order. Groups of one level and one
signature run as lanes. This helps piles and ragdolls; a biped on the
ground has one dynamic body and stays serial. The fixed-accumulator guard
applies here too. It is the last slice, and only if the campaign census
shows islands large enough to pay for it.

**Workers.** For `Jobs > 1`, the islands of a run are solved on workers by
fixed chunks of island index, then scattered in order on the game thread.

### 7.6 Islands, deactivation, pending operations

`island.rs` ports membership (`0x00316c90`, `0x00316cf0`), activation
(`0x0030ccd0`), deactivation (`0x0030cc60`), merging (`0x0030fc30`),
splitting (`0x0030e720`) and the pending-operation queue (`0x0030f6b0`)
as they are, on guest arrays through `array.rs`, in retail order (F6, F7).
They are integer work plus allocation. Until slice 8 they remain lifted
and are awaited between Rust stages.

### 7.7 Bipeds, and the absence of character proxies

Retail has no character-proxy object. A biped is a rigid body whose shape
is the type `0x17` wrapper around a pill, and three things make it a
"character":

- the `0x00412290` agent, with its swept support offset and the side mask
  at agent `+0xf0`. It is `t17.rs`, a first-class kernel, because it is
  the superbounce path ([superbounce.md](../game/superbounce.md)) and
  0.77% of the thread;
- Halo's contact callback, which fills the component's contact list at
  `+0x88`. It stays guest until the biped port in phase 3 reaches it;
- the rebuild on crouch (`0x001c3770`), which is plain entity removal and
  creation and is seen through validation (§5).

Phantoms (the listener and phantom arrays on the world) are in slice 0's
census. Until ported, their callbacks are awaited guest calls at their
retail positions.

### 7.8 The boundary

`step.rs` is written as stages that return either `Done` or `Call(guest
target, arguments, resume stage)`. The adapter's `async fn step` awaits
each `Call` (feature-engine.md, "Calling code that is still recompiled").
No guest memory guard is held across an await; workers never touch guest
memory.

**Entries taken over**, each a `Point` with the ABI read here (**Code**):

| Slice | Entry | ABI | Body |
|---|---|---|---|
| 1 | `0x0022c300`, `0x0022c320`, `0x002d7020`, `0x002d7090` | memory-system virtuals, thiscall | plain `fn` |
| 2 | `0x0031c100` | cdecl, 9 stack arguments, caller pops `0x24` | plain `fn`, island fallback |
| 3 | `0x00316a20` | thiscall (island; input), `ret 4` | async (actions) |
| 3 | `0x00316b30` | thiscall (island; out byte, step), `ret 8` | plain `fn` |
| 3 | `0x0030fef0` | thiscall (world; step), `ret 4` | async |
| 4 | `0x001edd00`, the polygon builder under the collection's `vt[+0x34]` `0x001ee4d0` (vtable `0x00456194`) and its two other callers | thiscall-like (shape ecx, transform edi; five stack words), `ret 0x14` | plain `fn` |
| 5 | `0x002eeb40`, the convex process call under `vt[+0x10]` of `0x00412c48` and `0x00412290` | thiscall (agent; A, B, input, collector), `ret 0x10` | async (contact callback) |
| 6 | `vt[+0x10]` of `0x00412a98`, `0x00412adc`, `0x00412f5c`, `0x00412f7c`, `0x00412fb0`, `0x00412fd0` | slice 6, step 0 | async |
| 7 | `0x00317d30` | thiscall (island; broadphase, input), `ret 8` | async |
| 7 | `0x0030fcb0` | thiscall (world; step), `ret 4` | async |
| 8 | `0x00311100` | thiscall (world; step), `ret 4` | async |

Each later entry contains the earlier ones. When slice 8 lands, the
earlier points stay installed only for callers outside the step, if slice
0 finds any.

**Guest writes that must still happen** (the full per-entry list is the
contract page; the differential harness compares all of memory):

- Motions: velocities `+0x40`, `+0x50`, transform `+0x80..+0xbf`, the
  accumulator offset `+0x08`, and whatever else `vt[+0x10]` and `vt[+0x04]`
  write (slice 2, step 0).
- World: `+0x12c`, `+0x12d`, `+0x150..+0x1a7`, `+0x190`, `+0x194`,
  `+0x10c`, the array heads.
- Islands: `+0x2c`, `+0x30`, `+0x34`, `+0x38`, `+0x78`, `+0xae`, the
  entity, constraint, action and pair arrays.
- Agents: the whole inline cache (feature ids, counts, manifold slots,
  axis, bound, the BV-tree child list and query box, `+0xf0` for bipeds).
- Contact constraints: the id map, points, properties (the solver's
  impulse at `+0x00` and velocity term at `+0x04` included).
- The memory system's own words (`+0x08`, `+0x0c`, `+0x14`, `+0x20`, the
  pool free lists), the solver buffer's busy byte `[0x00484b38]+0x10`, and
  the overflow stacks' counters.
- Halo's globals `0x0055e4fc..0x0055e50c`, by the hooks themselves.

- The solver buffer's and the LIFO block's contents as the solve leaves
  them: the accumulators, the rows with their corrected right-hand sides,
  the impulses and the schemas. A later island's fixed and leading
  accumulators keep their flag byte from these.

**Not written:** the monitor stream (off), the guest stack.

**Guest readers kept fed:** the Halo wrapper's post-step walk
(`0x001c4260`, `0x001c4a20`, the iterator `0x002797a0`); body fetches
(`0x001d08e0`, `0x001d09d0`, `0x000dc67d`, `0x001c4f51`); body rebuilds
(`0x001c3770`, `0x001cf120`, `0x001d01c0`); the memory-pressure path
(`0x00146bf0`, `0x00146fd0`, `0x001c4040`); Halo's listeners and contact
callback; the phase 2 queries that walk the same shapes; the agent API's
`havok.bodies`, `havok.contacts`, `havok.agents`; live-telemetry's readers.

## 8. Fallback by data

| Condition | Scope | What runs |
|---|---|---|
| Monitor stream on; float environment not the pinned one | step | lifted step, or `Exact` for the float case |
| Solver tables not initialised (`[0x00480c60] == 0`) | island | lifted `0x0031c100` |
| Motion vtable, constraint vtable or schema opcode not covered | island | lifted `0x0031c100` |
| Agent or shape vtable not covered | pair | lifted `vt[+0x10]` |
| A structure container `StructureGeo` could not decode | pair | lifted |
| A fast-path guard miss | kernel call | `Exact` rerun |

Every fallback increments a named counter (`sim-havok` exports them; the
sim report prints them). The exit criteria require the island and pair
fallback counters to be zero on the benchmark runs. A fallback is a named,
counted event, never a silent one.

## 9. Verification

Bar E, sim-rewrite.md's ladder, with these specifics.

### 9.1 Fixtures built by retail's own code

Havok layouts are too rich to write by hand. `fixture.rs` drives the
*lifted* constructors on the IR interpreter (world `0x003101c0`, entity
and motion construction, shape constructors, add-to-world) with synthetic
numbers: spheres, capsules, a biped wrapper, and a small synthetic
structure with its own MOPP from `crates/formats/mopp`'s compiler. The result is a
guest memory image holding a real world and no game data. Needs the
owner's XBE; skipped without it.

### 9.2 `havok.digest`

A canonical walk in `digest.rs`: world scalars, then islands in array
order (active, inactive, fixed), each entity (motion bytes, shape
identity, properties), each constraint (id map, points, properties), each
pair (agent bytes, child list), each action pointer, then the memory
system's bookkeeping words. Addresses are hashed as they are, so
allocation order is covered.

- The agent API answers it as `havok.digest` (read; `{}` → `{digest, bodies,
  constraints, pairs, heap_used}`), documented in agent-api.md, with a
  fixture test. Decision under AGENTS.md: expand, because every later
  physics lane and test needs this check.
- The sim report carries it as the `havok.world` row of
  `state_components_v1` when the report has components, so `h2 sim diff`
  names the first tick where physics differs. No new switch.
- A strict variant hashes the whole heap block `[0x00479888]` for
  `0x105470` bytes. It holds while freed blocks carry the same stale
  bytes, which is true through slice 7 for everything but scratch-like
  heap temporaries; the walk digest is the criterion, the raw one a
  diagnostic.

### 9.3 Differential tests

`crates/rewrites/engine/tests/sim_physics_differential.rs` (a module of `tests/it.rs`), `differential` feature.

- **Per entry.** From a fixture image, run the lifted entry on the
  interpreter and the Rust entry; compare all of memory outside the
  scratch regions of §7.8, the return value and the registers outside the
  clobber list. `Fast` and `Exact` both; lanes on and off; 1, 2 and 8
  workers.
- **Per kernel, against the oracles.** `solver.rs` against `havok::solve`
  on the same stream: `tests/sim_physics_kernel.rs` (a module of
  `tests/it.rs`, `differential` feature, the owner's image) builds a
  seeded typed `Stream`, encodes it as the guest's schema, rows,
  accumulators and impulses, runs the oracle to its return, and compares
  every body vector, row word and impulse as bits under all four MXCSR
  rounding modes with the x87 control word `0x027f`
  (`rows_equal_register_port`, 10,000 cases;
  `correct_velocities_equal_register_port`, which zeroes inverse mass so
  the sums and the right-hand-side correction are what moves;
  `rows_equal_on_non_finite_impulses`, where the kernel declines the
  stream and the oracle still runs it). `sim-havok`'s
  `tests/no_mul_add.rs` and its `clippy.toml` keep fused multiply-add out
  of the kernel; `gjk.rs` against `support::vertices`; `weld.rs`
  against `weld::contacts`. The oracles already match the lifted code.
- **Seeded sweeps.** Random masses, velocities and contact sets, with
  seeded NaN, infinity, denormal and signed-zero inputs, so `Exact` and
  the guard-miss restart are exercised. Coincident bodies, zero-length
  capsules, a constraint with A equal to B.
- **Callback clobbers.** Awaited guest calls are served by seeded fakes
  that change bodies, remove contacts and queue world operations, as
  `sim_physics_differential.rs` does for child calls. This is the test of
  §7.3.
- **Multi-step.** 600 steps of a fixture world, comparing `havok.digest`
  every step. Divergence that needs history (cached features, welding,
  deactivation timers) shows here.

### 9.4 Runs

- **Call captures.** The n-th call of `0x00311100` in a real match,
  registers and RAM, kept in the user's data directory; the differential
  test runs from each. Captures for: a superbounce landing, a grenade
  bounce, a vehicle at speed, a ragdoll, a pile of crates.
- **Run parity.** The sim-budget A/B, the native world against the lifted
  build: Lockout local, Waterworks local, Waterworks mixed, seed 7, 1,800
  ticks, arena hashes on. Every `state_hashes_v1` row, every arena block
  and the `havok` component must match; `h2 sim diff` prints nothing. Then
  a co-op campaign level headless (decision 8.3), and the Halo 1 behaviour
  suite.
- **Cache-cold arm.** The same runs with `BodyIndex`, `Decoded` and
  `StructureGeo` dropped every tick.
- **Cross-architecture.** The same runs on arm64 and x86-64: identical
  hash series and `havok` digests.
- **Superbounce.** The regression in
  [high-framerate.md](../game/high-framerate.md) §4 must reproduce tick for
  tick. Under bar E it is a consequence, and it is the check the user will
  ask about.
- **Timing.** Three unprofiled medians per arm, and one profiled pair to
  confirm the family's samples dropped.

### 9.5 Capture arms and the answering arms

The kernels the world step absorbs run as capture arms in
`crates/rewrites/engine/src/sim_physics/parity.rs`, installed by
`crates/rewrites/sim-bridge/src/sim_physics.rs`. `parity::status()`
gives the compare-only arms' matched, mismatched and declined calls since
start; `parity::takeover()` gives, for the arms that answer, the calls
answered and declined since start.

| Arm | Entry | Mode |
|---|---|---|
| `physics.broadphase` | `2e0350`, `ret 0x14` | answers |
| `physics.motion.pose` | `31b060`, `ret 4` | compare-only |
| `physics.motion.forces` | `31b1c0`, `ret 8` | compare-only |
| `physics.mopp.box` | `300150` | compare-only |
| `physics.island.integrate` | `316a20`, `ret 4` | answers |
| `physics.island.deactivation` | `316b30`, `ret 8` | answers |
| `physics.integrate` | `30fef0`, `ret 4` | answers |
| `physics.structure.polygon` | `1edd00`, `ret 0x14` | answers |
| `physics.agent.convex` | `2eeb40`, `ret 0x10` | answers |
| `physics.agent.query` | `3048a0`, `ret 0x14` | answers |
| `physics.agent.point` | `302c70`, `ret` | answers |
| `physics.contact.commit` | `318c10` to `318320`, `ret 0x10` | answers to the walk |

- **Broadphase.** The broadphase arm is one `implement` at `2e0350`.
  `broadphase::capture` predicts the whole `CpuAbi` but `eip` and every
  byte span the call writes. From the first call, `Prediction::apply`
  writes the spans and the registers and the call returns to its caller.
  A call the capture refuses runs the lifted entry and is counted by
  reason. No call is compared with the guest.
- **Island integrate.** The same at `316a20`.
  `island_integrate::capture` runs the whole entry in Rust on a journal:
  the island solve with the nine arguments the entry pushes, or, with no
  constraint, each entity's forces and pose virtuals last to first
  ([havok-integrate.md](havok-integrate.md#the-island-integrate)). The prediction is
  every journalled word and ecx, ebx, ebp, esi, edi and esp; eax, edx and
  the arithmetic flags are dead in the one caller (`0x0030fef0`, read at
  `0x0031000b..0x0031000f`). The XMM registers, the x87 condition codes
  and the sticky exception flags are left as they were, as the island
  solve's entry leaves them. When the entry
  declines, the entry's frame replay (`sim_physics::ISLAND_INTEGRATE`)
  runs it, and the solve inside is the island solve's own entry. Its
  declines are counted by reason as well (`memory`, `action`, `motion`,
  `float`, `solve`, `monitor`, `unobserved`; `havok.islands`).
- **Deactivation test and integrate phase.** The same at `316b30` and `30fef0`, each one `implement` whose guest answer
  is the lifted original ([havok-integrate.md](havok-integrate.md)). The
  test's prediction is its journal (the timers, the counter, the
  deactivators' references, the out byte) with eax, ebx, ebp, esi, edi
  and esp. The phase's is every island's test and integrate on one
  journal, in the phase's order, with eax and ecx the monitor words, edx
  `0xffffffff`, ebx, ebp, esi, edi and esp. Their declines are counted by
  reason (`havok.islands`'s `deactivation` and `phase`). The phase
  composes the other two models, so an answered phase answers the whole
  island loop in one call.
- **Structure polygon.** The same at `1edd00`, the builder every
  child-shape getter of the structure's containers calls
  ([havok-structure.md](havok-structure.md)). The prediction is the
  shape's words with eax and ecx the shape, edx the tail, ebx, ebp, esi
  and edi and esp; declines `memory`, `ring`, `float`, `unobserved`
  (`havok.islands`'s `polygon`).
- **Convex agent.** The same at `2eeb40`, the convex agent's process
  call ([havok-convex.md](havok-convex.md)), on the separated-pair path
  only: the bound's store with eax the point-count address, ecx the
  monitor head, edx the manager word, ebx, ebp, esi, edi and esp;
  declines `memory`, `closest` (the query path), `points`, `float`,
  `monitor`, `unobserved` (`havok.islands`'s `convex`).
- **Closest-feature query.** The same at `3048a0`, inside the guest's
  process call: the whole query with the shapes' virtuals, the triangle
  case and its penetration call in Rust; every store above esp with eax,
  ecx and edx the query's; declines `memory`, `shape` (a virtual with no
  port), `float`, `monitor`, `unobserved` (`havok.islands`'s `query`).
- **Add-point.** The same at `302c70`, a point that moves the slot it
  matches: the slot's words with eax the slot, ecx and edx the point's
  distance and weight; declines `memory`, `commit` (the manager adds the
  contact), `float`, `unobserved` (`havok.islands`'s `point`).
- **Contact commit.** The same at `318c10`, up to
  the world listener walk `318320`, whose listeners include Halo's
  contact callback. An answer leaves the call at the walk's entry and
  the hook calls the walk in place (`ExecCtx::call_inline`); the lifted
  tail runs from its site `318d9b`. Declines `memory`, `full`, `first`, `grow`,
  `motion`, `island`, `float`, `unobserved` (`havok.islands`'s `commit`;
  [havok-convex.md](havok-convex.md#the-commit-arm)).
- **Declines.** A capture that refuses the state (float environment,
  array capacity, an unknown node) and an apply that refuses its spans (an
  access trap, a read-only page, a span across mappings, two spans on the
  same physical bytes) run the lifted entry and count as `declined`, by
  reason. The guest still answers those calls.
- **Compare-only arms.** The motion pose and forces and the MOPP box arms
  compare their prediction with the guest's return and never answer.
- **Motion stays compare-only.** A motion prediction holds `esp`, `ebx`,
  `ebp`, `esi`, `edi` and the body's `0x110` bytes. What retail leaves in
  `eax`, `ecx`, `edx`, the XMM registers, the x87 stack, status and tag
  words, the MXCSR flags and `eflags` is not modelled, and no
  differential test pins it, so an applied motion could not be shown to
  leave the guest's registers.
- **Live check.** On Lockout, 1,800 ticks, the native world against the
  lifted build: `havok.digest` equal at every tick, `physics.broadphase`,
  `physics.island.integrate`, `physics.island.deactivation` and
  `physics.integrate` answered above zero
  (`havok.islands` gives the last three's counts and declines). An answer drops no
  write only if its spans or words cover every byte retail writes outside
  the stack below `esp`; the digest
  check covers the rest.

## 10. Threads

As sim-rewrite.md §6. Workers take only speculated pairs (§7.4) and island
solves (§7.5), in fixed chunks by index. Default `Jobs` is 1 on hosts and
sims. Results do not depend on the count.

## 11. Slices

Each is one lane. Step 0 of a slice is static reading recorded in
`docs/engine/havok-step.md` before code. "Exit" lists what the coordinator
checks beyond a clean §9.3.

| # | Slice | Contents | Samples in scope | Exit |
|---|---|---|---:|---|
| 0 | Contract and digest | `havok-step.md`: the step tree of §2 with every persistent write; the censuses (motion, constraint, agent, shape, action, listener and phantom vtables; float instruction mix under `0x00311100` by `h2 fpstats --root`, `rcpps`/`rsqrtps` in particular; callers of each planned entry from outside the step); the revert question of §3.1. The digest (`live-telemetry`'s `havok_agents::digest`), `havok.digest`, the sim report component | 0 | A/A runs give equal `havok` components; the digest differs when one body is nudged |
| 1 | Foundations | `fp.rs` with both implementations and its unit tests against `simd` and `x87`; `layout.rs`, `view.rs`, `mem.rs`, `array.rs`, the four allocator entries; `fixture.rs` | 88 | allocator entries differential-clean; run parity |
| 2 | Island solve | `motion.rs` (accumulator build, integrate from accumulator), `jacobian.rs` for the contact constraint and each class the census found in multiplayer, `solver.rs`, entry `0x0031c100` | 852 + part of 1,277 | run parity; island fallback counter zero on the three benchmark runs; ≥ 0.15 ms |
| 3 | Integrate phase | `0x00316a20`, `0x00316b30`, `0x0030fef0`; the no-constraint path (`vt[+0x08]`, `vt[+0x04]`); action census and awaited calls; runs and lanes across islands; worker solves | rest of 1,277 | as 2; lanes on and off equal; ≥ 0.25 ms cumulative |
| 4 | Structure geometry | `structure.rs`: polygons per surface with the builder's exact output, MOPP through phase 2's decode; the container's child-shape virtuals | 861 + 535 | disc sweep (`#[ignore]`): every surface of every map equals the lifted builder; ≥ 0.45 ms cumulative |
| 5 | Convex agents | `shape.rs`, `gjk.rs`, `manifold.rs`, `agent/convex.rs`, `agent/t17.rs`, `contact.rs` | 651 + 879 + 368 + part of 619 | superbounce capture clean; ≥ 0.75 ms cumulative |
| 6 | Tree and welding agents | `agent/bvtree.rs`, `agent/weld.rs`, `agent/sphere.rs`; child agent lifetime; polygons no longer built in guest memory | 789 + 325 + rest of 619 | pair fallback counter zero on the benchmark runs; ≥ 1.0 ms cumulative |
| 7 | Collide phase | `broadphase.rs`, agent creation and destruction, `0x00317d30`, `0x0030fcb0`, speculative pairs on workers | 408 | 1, 2 and 8 workers equal; ≥ 1.1 ms cumulative on one thread |
| 8 | Whole step | `island.rs` (merge, split, pending operations), Halo's hooks ported, entry `0x00311100`; delete `impl.sim.native_havok_solver`, `_havok_support_vertices`, `_havok_weld` and keep their bodies as oracles | 502 (part) | the plan's exit: ladder clean, Waterworks ≤ 7.5 ms/tick with phases 1 to 3, phase 4 ≥ 1.2 ms measured |
| 9 | Wavefront lanes | §7.5, only if the co-op campaign census shows islands with ≥ 8 constraints over ≥ 3 dynamic bodies taking ≥ 0.2 ms | campaign | lanes on and off equal; measured gain |

Order rationale: slice 2 is first because it has two oracles and no guest
calls. Slices 4 to 6 are where the samples are. Slice 7 is late because
the broadphase is cheap and its order feeds everything. Slices 2 and 4 can
run in parallel lanes; 5 needs 4; 6 needs 5.

Coordinator commands per slice (lanes run nothing):

```
cargo test -p sim-havok
cargo test -p engine --features differential --test it sim_physics_differential::
cargo test -p engine --features differential --test it sim_havok_differential::
cargo test -p agent havok
```

## 12. Risks and what is not resolved

- **Revert and Havok.** Settled in [havok-step.md](havok-step.md), "Save,
  revert and rebuild": a save and a revert take every object's physics
  out of the world and step it once, and rebuild the bodies from the
  arena afterwards; the heap and the component array (its own virtual
  allocation) are never copied. The meaning of the flag bits is **S**.
- **Coverage of classes.** The static census is in
  [havok-step.md](havok-step.md), "Censuses": six motion classes (types 2
  to 7), the contact constraint and six joint classes built by Halo, 39
  agents, no actions. Which of them a match builds is a run question;
  until a class is ported its islands fall back, exactly.
- **Speculation hit rate (Open).** Whether Halo's contact callback changes
  body state during collide decides how often a speculated pair is
  recomputed. It cannot affect results.
- **`rsqrtss`.** The step has no `rcpps`, `rsqrtps` or `rcpss`, and 24
  `rsqrtss` sites, the contact Jacobian builder among them
  ([havok-step.md](havok-step.md), "Floats"). Each calls `simd::rsqrtss`,
  the recompiled build's definition; `Fast` has no host shortcut there.
- **The polygon builder's output (S).** A structure child's polygon
  depends on the surface and the builder's arguments (flag, owner, tail,
  transform) only ([havok-structure.md](havok-structure.md), **E**). That
  nothing retains a pointer to the temporary shape is still assumed.
- **Motion types 6 and 7, pair word `+0x0c`, property words `+0x04`,
  `+0x14`, `+0x18` (Open in havok-runtime.md).** A structural port copies
  the code that uses them, so naming them is not a blocker, but each must
  be read in its slice's step 0.
- **Fixed bodies in lanes.** Covered by the guard in §7.5, and by a seeded
  test with non-finite impulses.
- **The scheduler clock.** It counts guest memory accesses, and a Rust
  step makes fewer, so slice ends move (sim-rewrite.md §2). Whole-RAM
  traces compare only within one switch set. Run parity uses the arena
  hash and the `havok` component.
- **Fault behaviour.** A fault mid-step leaves fewer bytes written than
  retail would, because commits are batched. The step faults only on
  corrupt state; this is recorded as a deliberate difference, as the dead
  stack already is.
- **Cost attribution.** §4's Halo bands are upper bounds, and the estimate
  assumes a ported kernel runs about four times faster than its lifted
  form. Slice 2's measurement is the first real data point; if it comes in
  under 0.1 ms, re-plan before slice 5.
- **Size.** About 250 guest functions carry samples in this family. This
  is the largest phase. Island and pair fallback are what let it land in
  slices that are each exact.

## 13. Summary

1. Physics stays bit-exact: a structural port, verified against the lifted code and the three existing oracle ports.
2. Every Havok object keeps its retail layout and address in guest memory; Rust owns the step's work, static geometry and transient SoA working sets.
3. The Havok heap is its own `0x106470`-byte allocation, not part of the checkpoint arena, so the allocator is ported exactly and a new `havok.digest` covers what the arena hash cannot.
4. Kernels compute into Rust buffers and commit at retail's positions; scratch is never written, and a float guard miss reruns the kernel on the exact models.
5. x87 under the pinned PC=53 is binary64 in guest order with binary32 stores; packed SSE is host lanes; both are identical on arm64 and x86-64.
6. Fixed order: operations in an expression, rows and constraints in an island, pair, child and welding order, broadphase output, island order, allocations, guest callbacks.
7. Free: layout, scratch, static decode, and any units that share no written data, including pairs, islands in a callback-free run (four per lane block by signature) and wavefront levels in an island.
8. Work may leave its retail position only under speculate, validate inputs, commit in order; a callback that moves a body costs a recompute, never a wrong bit.
9. The family is 9,085 of 84,358 samples (10.77%, about 2.0 ms of the tick with runtime tax); narrowphase against the level is half of it and the solver 1%; expect 1.2 to 1.5 ms back.
10. Ten slices from contract and digest to the whole step; the contract page [havok-step.md](havok-step.md) holds the censuses, the float mix (`rsqrtss`, no packed estimates) and how a revert rebuilds the bodies.
