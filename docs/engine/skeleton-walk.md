# Skeleton walk (`bd090`'s node loop)

Status: Partly built. The walk inside `bd090` is Rust; the same loop in
`1dd1c0` (the animation graph's own callers) and the run ladder are not.
Code: `crates/rewrites/sim-anim/src/skeleton.rs` (`build_matrices`, `pose`,
`compose`), `crates/rewrites/engine/src/sim_anim/skeleton.rs` (`run`,
`walk`). Tests: `crates/rewrites/sim-anim/src/skeleton_tests.rs`,
`crates/rewrites/engine/src/sim_anim/skeleton/tests.rs`,
`crates/rewrites/engine/tests/sim_physics_differential.rs`
(`object_node_matrices`).
Switch: `impl.sim.native.anim.skeleton` (on), listed in
[switches.md](../runtime/switches.md).

Phase 1b of the [simulation rewrite](sim-rewrite.md). `bd090` turns an
object's node records (the 32-byte records [animation
sampling](anim-sampling.md) writes) into its node matrices: per node, the
pose matrix `1421f0` and then the composition `142a60` with the parent's
matrix. The walk replaces that loop with one gather, one kernel call over
Rust arrays and one scatter. Every byte and register the loop leaves is
the same, so the guest cannot tell which ran.

## How it works

`bd090` runs natively as the register-level port
`engine::sim_physics::node_matrices` (under `impl.sim.native_pose_matrix`,
or as the `impl.sim.native.objects` body `sim_objects::pose`). At the
loop's entry (`bd50c`, node count above 1, `esi` 1) it calls
`engine::sim_anim::skeleton::run`, and runs its own per-node loop `nodes`
only when that returns `false`.

1. **Gather** (`plan`). From `bd090`'s frame: the skeleton definition
   `[esp+14]` (node count `+48`, node array `+4c`, parent `i16` at `+4` of
   each 0x60-byte node), the record block `[esp+18]` (0x20 bytes a node),
   the matrices `[esp+1c]` (0x34 bytes a node) and the keep mask at
   `esp+70` (one bit a node). From the image: 1.0 `[45dbc0]`, the sign mask
   at `453750`, and the pose cutoff: `[55ebb0]` when `[55ebb4]` bit 0 is
   set, else `[45dbdc]`, which the first pose copies there. XMM0 to XMM7
   from the CPU.
2. **Kernel** (`sim_anim::build_matrices`). Nodes 1 to `count - 1` in
   order; a kept node is skipped. Otherwise `pose` writes the node's slot
   and `compose(matrices[parent], posed)` overwrites it. The array is
   updated in place, so a parent later in the order gives its matrix from
   before the walk and a node that names itself composes with its own
   pose, as in the guest.
3. **Scatter** (`scatter`). The posed nodes' matrices; `[55ebb4] |= 1` and
   the cutoff when the first pose initialises them; the dead stack of the
   last posed node's two calls (`esp-10` its record, `esp-c` `bd55e`,
   `esp-8` `bd571`, `esp-4` its matrix); XMM0 to XMM7 as the last
   composition leaves them; and the loop's exit registers.

### The leaves' arithmetic

Both leaves are packed SSE in a fixed order; `pose` and `compose` repeat
it lane for lane, through `sim_core::det::Fp`.

| Leaf | Computes |
|---|---|
| `1421f0` pose | `n = (q0² + q2²) + (q1² + q3²)`; at or below the cutoff, the identity basis. Else `r = rcp(n)`, `f = 2((r + r) - (n·r)·r)`, `s = q·f`; `xx`, `yy`, `ww` are `q0·s`, `q1·s`, `q3·s`, `zz = s2·q2`; `cross = [xx1, xx2, xx1, xx2] + ([ww2, ww1, ww2, ww1] ^ signs)`; forward `[(1 - zz) - yy1, cross2, cross3]`, left `[cross0, (1 - zz) - xx0, yy2 + ww0]`, up `[cross1, yy2 - ww0, (1 - xx0) - yy1]`. Scale and position copied as bits |
| `142a60` compose `a ∘ b` | columns load as `[x, 0, y, z]`; position `((tx·f + ty·l) + tz·u)·a.scale + a.position`; each of `b`'s up, left, forward columns `c0·f + (c1·l + c2·u)`; scale `b.scale · a.scale` |

`rcp` is the Pentium III estimate (`simd` crate). The pose's norm add also
adds XMM1's upper two lanes, as they stand on entry, into dead lanes: the
walk checks them like any operand.

### Registers left

| Register | Value |
|---|---|
| `esi`, `ecx` | the node count |
| `ebx` | `0x60 · count` |
| `ebp` | records `+ 0x20 · count` |
| `edx` | matrices `+ 0x34 · count` |
| `edi` | node array `+ 0x60 · (count - 1)` |
| `eax` | the skeleton definition |
| XMM0 | the last result column (forward) |
| XMM1, XMM2 | the last column's `y + z` and `z` products |
| XMM3 | `[scale product, 0, 0, 0]` |
| XMM4 to XMM6 | the parent's forward, left and up columns |
| XMM7 | the parent's scale in every lane |

With every node kept, the XMM registers are unchanged. EFLAGS is not
reproduced: `bd090`'s epilogue (`add esp`) replaces the arithmetic flags
before anything reads them.

## Interfaces

- Entered only from `bd090`'s loop entry, by both the port and the objects
  body; nothing else calls it.
- Writes nothing outside the loop's own write set: the matrices of posed
  nodes, `[55ebb4]` and `[55ebb0]`, and the 16 bytes below `esp`.
- `engine::sim_anim::skeleton::stats()` counts walks, nodes posed and
  declines by reason.
- The pose and composition entries (`OBJECT_POSE_MATRIX`,
  `OBJECT_TRANSFORM_COMPOSE`) and their per-function switches stay: other
  callers, `1dd1c0` among them, still reach them.

## Rules and limits

The walk declines, writing nothing, and the per-node loop runs, when:

- MXCSR is not masked, round-to-nearest and PE-sticky (`det::fast_env`),
  or any operand of any operation is not zero or of magnitude in
  `[2^-63, 2^63)` (`Float`);
- the node count is below 2 or above 1024 (`Count`);
- a range it reads or writes is not inside one mapping, or a written range
  overlaps another range it reads or writes, compared as physical
  addresses so aliased mappings count (`Alias`);
- a posed node's parent index is negative or not below the count
  (`Parent`);
- a read faults (`Fault`).

Under those checks every operation is one correctly rounded binary32 add,
subtract or multiply with no flag but PE, so host results equal the
guest's on arm64 and x86-64 and MXCSR does not change. The walk reads node
0's parent and the keep words even where the loop would not; both lie in
ranges checked to be mapped. A write fault in the scatter (a read-only
page) leaves a different partial state from the loop's.

## Evidence

- `skeleton_tests.rs`: `pose` and `compose` against the formulas written
  out in plain `f32`, the identity branch, the identity parent, the walk
  against the leaves called node by node (counts across mask words),
  parents read as they stand, declines on bad parents, short inputs,
  denormal operands and a non-finite dead lane, host lanes against the
  scalar loop.
- `skeleton/tests.rs` (engine, no image): the walk against the port's
  per-node loop on synthetic guest memory, counts 2 to 90, random trees
  with forward and self parents, random keep masks, both cutoff states,
  both lane paths; RAM byte for byte and every CPU field but EFLAGS must
  match. Every decline leaves RAM and CPU untouched.
- `sim_physics_differential::object_node_matrices` (`H2_XBE`) runs the
  port, and with it the walk, against the lifted `bd090` from the same
  synthetic objects, comparing all memory and registers at every
  boundary. The per-node loop is the oracle the walk replaces: that test
  and `sim_native_differential` hold it equal to the guest.

## Not verified

- Run parity: the sim-budget A/B with `-impl.sim.native.anim.skeleton`
  against on, with `impl.sim.native.objects` (or
  `impl.sim.native_pose_matrix`) on, identical `state_hashes_v1` and an
  empty `h2 sim diff`; the cross-architecture arm.
- The decline rate on real skeletons (`stats()`), and the time saved per
  tick on Waterworks.

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1b.
- [object-update.md](object-update.md), "Node matrices `bd090`".
- [anim-sampling.md](anim-sampling.md): the records the walk reads.
