# Skeleton walk (`bd090`'s node loop)

Status: Partly built. The walk inside `bd090` is Rust. The graph walk
`1dd1c0` is Rust: the guest answers and the Rust result is compared with
it until 4096 calls have matched with no mismatch, then the Rust body
answers and one call in 64 is still compared.
Code: `crates/rewrites/sim-anim/src/skeleton.rs` (`build_matrices`, `pose`,
`compose`), `crates/rewrites/sim-anim/src/graph_walk.rs`
(`graph_matrices`, `graph_order`),
`crates/rewrites/engine/src/sim_anim/skeleton.rs` (`walk`),
`crates/rewrites/engine/src/sim_anim/graph_walk.rs` (`plan`, `run`,
`enter`, `compare`, `reference`). Tests:
`crates/rewrites/sim-anim/src/skeleton_tests.rs`,
`crates/rewrites/sim-anim/src/graph_walk_tests.rs`,
`crates/rewrites/engine/src/sim_anim/skeleton/tests.rs`,
`crates/rewrites/engine/src/sim_anim/graph_walk/tests.rs`,
`crates/rewrites/engine/tests/sim_physics_differential.rs`
(`object_node_matrices`),
`crates/rewrites/engine/tests/sim_anim_graph_differential.rs`.
`bd090`'s walk always runs in Rust, with no switch; a walk it declines
is a counted `Decline` and runs the port's per-node loop.
The graph walk is native only too.

Phase 1b of the [simulation rewrite](sim-rewrite.md). `bd090` turns an
object's node records (the 32-byte records [animation
sampling](anim-sampling.md) writes) into its node matrices: per node, the
pose matrix `1421f0` and then the composition `142a60` with the parent's
matrix. The walk replaces that loop with one gather, one kernel call over
Rust arrays and one scatter. Every byte and register the loop leaves is
the same, so the guest cannot tell which ran.

## How it works

`bd090` runs natively as the register-level port
`engine::sim_physics::node_matrices` (as the body `sim_objects::pose`
in the walk table; or as the port
`sim.port.pose_matrix`). At the
loop's entry (`bd50c`, node count above 1, `esi` 1) it calls
`engine::sim_anim::skeleton::walk`, and runs its own per-node loop `nodes`
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
  `OBJECT_TRANSFORM_COMPOSE`) and their ports stay: other
  callers, among them `1dd1c0`, still reach them.
- The graph walk is the point `ANIM_GRAPH_MATRICES` (`1dd1c0`). `sim-bridge` hooks it with a `before` that plans
  the call and answers with the body; it has no `after`.
- `engine::sim_anim::graph_walk::stats()` counts the body's calls and
  nodes. The agent method `anim.graph_walk` returns
  `{addr, body:{native,nodes}}`
  ([agent-api.md](../runtime/agent-api-checks.md#anim-the-animation-graph-walk)).

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

## The graph walk `1dd1c0`

`1dd1c0(graph, matrices, records, root)` (stdcall, `ret 0x10`) is the
same pose-and-compose loop for the animation graph's own callers (the
channel manager's `1cbd50` among them). It differs from `bd090`'s walk in
three ways:

- **Order.** A first-in, first-out queue of node indices in its frame
  starts with node 0. Each visited node queues its first child (`+4` of
  its 0x20-byte graph node) and then its next sibling (`+6`), when not
  `-1`. A node linked twice is visited twice; a node linked from nothing
  is not visited, and its matrix is not written.
- **Scratch.** The pose goes to a scratch matrix in the frame, which is
  then composed with the parent's matrix into the node's slot. The parent
  is `matrices + 0x34 * parent` (`+8`), read as it stands, or `root` for
  node 0, whose parent field is not read.
- **No mask.** Every visited node is posed.

The node count is `[graph + 0xc]`, the graph nodes `[graph + 0x10]`, the
records `records + 0x20 * node`. A count at or below 0 returns at once.

### What the call leaves

Below the entry `esp` (the return address's slot):

| Offset | Bytes |
|---|---|
| `-0x3fc` up | the queue, one `u32` per queued node |
| `-0x430` | the scratch matrix: the last node's pose |
| `-0x434` | the queue length |
| `-0x438`, `-0x43c` | the caller's `ebx` and `ebp`; the only bytes written when the count is at or below 0 |
| `-0x440`, `-0x444` | the caller's `esi` and `edi` |
| `-0x448` | the last node's output matrix, the composition's argument |
| `-0x44c`, `-0x450` | the return addresses `1dd241` and `1dd230` |
| `-0x454` | the pose's saved `ebp`: `matrices` |

The queue has room for 255 entries; a 256th would overwrite the return
address.

| Register | Value |
|---|---|
| `eax` | the queue length; `graph` when the count is at or below 0 |
| `ecx` | the last node's first child, sign-extended, when it has one; else its parent's matrix (`root` for node 0); the count when at or below 0 |
| `edx` | the last node's next sibling, sign-extended, when it has one; else the scratch matrix; unchanged when the count is at or below 0 |
| `ebx`, `ebp`, `esi`, `edi` | restored |
| XMM0 to XMM7 | as the last composition leaves them; unchanged when the count is at or below 0 |
| EFLAGS | `add esp, 0x434` |

Both leaves initialise the cutoff on the first pose, as in `bd090`'s walk.

### Plan, arm and body

`plan` reads the arguments, the graph, the records, the matrices, the
root, the cutoff and the constants, builds the queue (`graph_order`),
runs `graph_matrices` and returns every byte and register above without
writing. The body (`run`, called by `enter`) writes the plan and returns
without the guest. A declined plan returns its `Decline`, and the hook
panics with `anim graph walk: <reason>`; no guest runs the call.

`reference` is `1dd1c0` instruction by instruction with the exact
`sim_native` leaves: the oracle of the engine tests and the third arm of
the lifted differential.

### The graph walk declines when

- MXCSR is not masked, round-to-nearest and PE-sticky, or an operand is
  off the fast path (`float`);
- a read faults (`fault`);
- the count is above 1024 (`count`);
- a queued link is neither `-1` nor below the count, or the queue would
  pass 255 entries (`queue`);
- a range it reads or writes is not inside one mapping, or a written
  range (the matrices, the frame, the cutoff) overlaps another range
  (`alias`);
- a visited node other than 0 names a parent outside the nodes
  (`parent`).

The plan reads every node's graph entry, record and matrix up to the
count, where the guest reads only the visited ones; an unvisited range
that is not mapped declines a call the guest would complete.

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
- `graph_walk_tests.rs`: the queue order (children before siblings, node
  0's sibling, a node queued twice), out-of-range links, a cycle, exactly
  255 entries and one more; the walk against the leaves called in queue
  order; the root as node 0's parent; parents read as they stand;
  unvisited nodes kept; a walk in index order giving other matrices.
- `graph_walk/tests.rs` (engine, no image): `run` against `reference` on
  synthetic guest memory, counts 0 to 255, random trees with unlinked
  nodes, forward and self parents, both cutoff states, both lane paths;
  RAM byte for byte and every CPU field must match. Every decline leaves
  RAM and CPU untouched. `compare` names a deliberate mismatch (a register,
  an XMM lane, a matrix byte, a queue entry, the scratch, a return address).
- `sim_anim_graph_differential.rs` (`H2_XBE`): the lifted `1dd1c0` with
  both leaves on the interpreter against `run` (both lane paths),
  `reference` and the plan, over 24 synthetic graphs; all memory
  and registers. A byte and a register changed in the guest's result are
  named by `compare`.

## Not verified

- Run parity: `h2 sim diff` against the lifted build at the defaults
  (the walk table runs `bd090`), identical `state_hashes_v1`; the
  cross-architecture arm.
- The decline rate on real skeletons (`stats()`), and the time saved per
  tick on Waterworks.
- The graph walk in a run: `anim.graph_walk`, over the sim-budget matches and a campaign
  level, without a decline panic. Its callers and call rate per tick are not measured.

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1b.
- [object-update.md](object-update.md), "Node matrices `bd090`".
- [anim-sampling.md](anim-sampling.md): the records the walk reads.
