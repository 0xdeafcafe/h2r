# Simulation rewrite, phase 2: the collision world

The collision world answers A, B and F, J’s sphere pretest and
state-exact exits in C, D, E, G and H. C and D's crossing exits answer
after a clean shadow run ([Crossing exits](collision-queries.md#crossing-exits)).
Other vector and spatial paths run their kernels alongside the lifted result. The instance and model rays
feed the whole-query planner, which also supplies `collision.trace`.
Entry I runs the general Havok ray and its callbacks in Rust, with no
guest call ([collision-queries-entries.md](collision-queries-entries.md),
"Entry I"). [Entry contracts](collision-queries.md) ·
[Umbrella plan](sim-rewrite.md)

Sources: the Waterworks profile in [sim-budget.md](../perf/sim-budget.md)
(84,358 main-thread samples; counts below were recomputed from its
`profile.folded`), the formats in [collision.md](../formats/collision.md),
[mopp.md](../formats/mopp.md) and
[structure-bsp.md](../formats/structure-bsp.md), the existing ports in
`crates/rewrites/engine/src/sim_native` and `sim_physics`, and static reading of the
owner's XBE. Addresses are guest virtual addresses. Evidence labels: **E**
read from the engine code for this page, **P** taken from an existing port
or doc, **Guess** not established.

## 1. What changes

The lifted paths and the older per-function ports keep the retail register
and stack shape:

- The BSP walk recurses through the dispatcher once per plane crossing and
  reads node, plane, leaf, reference, 2D node, surface, edge and vertex
  through seven pointers.
- The MOPP interpreters decode big-endian bytecode on every visit, copy a
  0x28 or 0x38-byte state on every two-child split, and make a guest call
  per split.
- One weapon trace (`0x001697c0`) makes a guest call per cluster, per
  instance, per object reference and per sphere test.

The rewrite decodes each BSP and each MOPP program once into flat arrays,
and runs each *query* as one Rust call that makes no guest calls. The
arithmetic keeps the retail operation order, so results do not change.

Three ideas carry the design:

1. **Closed kernels.** Each taken-over entry is a pure function of guest
   memory: gather, compute in Rust buffers, then scatter. Nothing is written
   to guest memory until the whole answer is known. A kernel that meets
   anything it cannot reproduce exactly returns `Bail`, and the adapter runs
   the original lifted body instead (section 5).
2. **Static lifting of the integer MOPP.** Every test of the box interpreter
   reduces at decode time to a compare of the query's full-precision corner
   against a precomputed threshold. Rescale, key-offset and jump opcodes
   disappear from the decoded tree (section 4.3).
3. **Ordered marks.** Batches carry each query’s epoch changes and mark
   stores into the next query. This preserves deduplication even when a
   generation wraps or a mapped alias reaches a mark (section 6).

## 2. Measured cost being removed

From `prof-ww/profile.folded`. "Self" is samples whose leaf frame is the
guest function. "With services" also counts host services running under it
(x87 helpers, memory access, dispatch exit), charged to the innermost visible
guest frame. Trampolines hide ancestors, so the per-family split is a lower
bound for the callers and exact for the leaves.

| Family | Guest functions | Self | With services | % thread |
|---|---|---:|---:|---:|
| BSP segment | `1de630` `1de6d0` `1dea00` `1debd0` `296520` | 747 | 824 | 0.98 |
| Point to leaf | `14a280` `11bed0` | 469 | 514 | 0.61 |
| MOPP ray and instances | `244980` `2df720` `301800` `3009e0` `2df760` `2df680` `1691a0` and its helpers | 1,091 | 1,644 | 1.95 |
| Cluster flood and object visit | `14a5b0` `14a370` `23a160` `bb050` `b8940` `b8a10` | 758 | 1,012 | 1.20 |
| Collision query | `1697c0` `11e5e0` `b89b0` `16b500` `168ae0` `169510` `1eff40` `1efca0` `1efbd0` | 656 | 1,025 | 1.22 |
| MOPP box (Havok structure) | `1eea50` `2df640` `300150` `2ff8b0` `1eece0` `1ef3e0` | 594 | 721 | 0.85 |
| Structure child shapes | `1ef950` `1edd00` `1ee500` | 241 | 318 | 0.38 |
| **Total** | | **4,556** | **6,058** | **7.18** |

7.18% of 14.12 ms/tick is **1.01 ms/tick**. The dispatcher's own time
between these functions (`run_from`, `execute`, resolve, charge) is not
attributed by the profiler and comes on top. The target is to remove at
least 80% of the attributed share, **≥ 0.8 ms/tick**, which is the phase 2
exit number in sim-rewrite.md. Each slice in section 9 names its own share.

Two facts from the stacks shape the order of work:

- `3009e0` is reached two ways. 437 samples have `244980` on the stack (the
  per-cluster instanced-geometry trace under `1697c0`). `2df720` appears in
  723, so at least 286 come from Havok's own ray casts through the same
  shape method. Both close in Rust: entry I runs each terminal's shape
  callbacks natively.
- `1de630` has two large callers: `1697c0` (265 samples) and the sound
  listener path `221e30` (254). `14a280` is called mostly through `11bed0`
  (277 of 463). Taking over the small entries first pays for sound, camera
  and weapons at once.

No campaign census exists yet. `1ddea0` (BSP volume query) and `16a8e0`
(sphere contact query) have zero samples on Waterworks and are left guest
until the phase 0 census says otherwise (slice S9).

## 3. Crates and modules

```
crates/rewrites/sim-core/            (shared with phase 1; S0 creates what is missing)
  src/det.rs                float environment, checked regions, x87 modes
  src/lanes.rs              f32x4 / i32x4 lanes (moved from sim_native/lanes.rs)
  src/epoch.rs              Epoch counter
crates/rewrites/sim-collision/       no `runtime` dependency; depends on sim-core, mopp
  src/lib.rs                CollisionWorld, Bail, public API
  src/mem.rs                trait GuestRead (bulk and scalar reads only)
  src/bsp/decode.rs         guest blocks -> Bsp
  src/bsp/point.rs          point to leaf
  src/bsp/segment.rs        vector test, leaf list, surface matcher
  src/bsp/ring.rs           projected rings, SIMD inside test
  src/mopp/mod.rs           code object header and source, constants
  src/mopp/decode.rs        bytecode -> Tree (unfolds state)
  src/mopp/boxq.rs          integer box query, single and packet
  src/mopp/ray.rs           float ray walker (pull style)
  src/structure.rs          clusters, leaf->cluster, instances, portals
  src/instance.rs           instanced-geometry trace (244980 and below)
  src/flood.rs              sphere cluster flood (14a5b0, 14a370)
  src/objects.rs            object reference gather, sphere pretests
  src/sphere.rs             segment–sphere pretest
  src/query.rs              the 1697c0 orchestration
  src/batch.rs              batched queries, ordered scatter
  src/store.rs              content-addressed mmapped cache (S10)
crates/rewrites/engine/src/sim_collision/
  mod.rs                    adapters: read ABI, call kernel, scatter, return
  layout.rs                 guest offsets as named constants (section 5)
  mopp_box.rs               entry F
  commit.rs                 ordered scatter helpers, mark replay
crates/rewrites/engine/tests/sim_collision_differential.rs
crates/rewrites/sim-bridge/src/sim_native.rs   always installs (`install_collision`)
docs/engine/collision-queries.md   contract page written in S0
```

`sim-collision` never writes guest memory. Its kernels take `&impl GuestRead`
for the dynamic state they need and return plain result structs. The engine
adapters own every guest write. This is what makes the kernels callable from
tests, from workers and from later Rust subsystems (phase 3 objects, phase 4
Havok) without the guest ABI.

`mopp` keeps its role: the bit model (`boxvm::query_box`, `vm`) is the oracle
for the decoded tree, and `code::Code::parse`, `terminal_key`,
`boxvm::diagonal_plane` are reused by the decoder.

## 4. Data

Decoded records use guest indices. The S10 on-disk format needs explicit
little-endian fields and a position-independent layout. The in-memory world
owns the arrays and address index. The runtime adapter manages subscriptions. Sizes below are per
element.

### 4.1 `CollisionWorld`

```rust
pub struct CollisionWorld {
    epoch: Epoch,                       // map generation it was built for
    bsps: Slots,                        // sorted guest addresses and decoded BSPs
    mopps: HashMap<u32, MoppSlot>,      // guest code-object pointer -> tree, source revision
    structure: Option<Structure>,       // for the active sbsp
    stats: Stats,                       // calls, bails by reason, decode time, bytes
}
```

A BSP is named by the guest pointer the engine itself passes (element `+4`
of a `coll` BSP, or the structure's). A MOPP tree is named by its code
object pointer (`sbsp+0x1e4`, a cluster's `+0xac`, a `coll` slot's `+0x68`,
a `phmo` code run). The BSP implementation looks up a guest address in a
sorted index. Its capacity and the owner of each decoded BSP count against
the map budget. Only bookkeeping visits that index in address order. The
query walks nodes in guest order. Decode is lazy: the first query
that names a pointer builds its entry from the guest-resident bytes, the
same bytes the guest would read. Nothing reads the map file separately.

### 4.2 `Bsp`

| Array | Element | Size | Notes |
|---|---|---:|---|
| `nodes` | plane bits, two sign-extended children, plane index, validation flags | 32 | Each node is aligned to 32 bytes and contains its split plane. Bit 23 marks a leaf. All ones means solid. |
| `planes`, `axis` | four binary32 words and a separate dominant-axis byte | 16 + 1 | The result can return the original guest address of the plane. |
| `leaves` | flags, reference count, first reference | 4 | The matcher derives leaf state from the flags. |
| `refs` | plane/flip word, 2D root, projection row | 6 | The row is computed from the dominant axis and flip at decode. |
| `nodes_2d` | three binary32 words, two child words | 16 | Child indices retain their guest encoding. |
| `surfaces` | plane, first edge, flags, breakable index, material, ring start and length, first packed group | 20 | A scalar matcher can follow the original first edge. |
| Optional ring edges | two binary32 xyz vertices | 24 | Pairs in traversal order. |
| Optional packed groups | four origins and four edge deltas, stored by axis | 96 | One or two per short ring. References share the groups across projection rows. |

Leaf indices, surface indices, plane indices and material indices are guest
visible and are never renumbered.

The scalar matcher uses the cached vertex pairs for longer rings and reads
the guest edge and vertex blocks when the ring cache is out of budget.

Structure-only tables live in `Structure`:

```rust
pub struct Structure {
    bsp: u32,                       // slot of the structure collision BSP
    bsp_index: u16,                 // [0x4686c4] at build
    leaf_cluster: Vec<i16>,         // sbsp leaves (+0x2c block, 8 bytes), i16 at +0
    clusters: Vec<Cluster>,         // sbsp+0x9c, stride 0xb0
    portals: Vec<Portal>,           // structure globals +0x60, stride 36
    instances: Vec<Instance>,       // placements sbsp+0x140 (0x58) joined to definitions sbsp+0x138 (0xc8)
    planes: Vec<[f32; 4]>,          // structure globals +0xc, for portals and cluster planes
}
pub struct Cluster { portals: Range<u32>, mopp: u32 /* slot or MAX */, plane_ref: i8, .. }
pub struct Instance {               // 80 bytes
    sphere: [f32; 4],               // placement +0x3c centre, +0x48 radius
    inverse: [f32; 13],             // result of 0x141590 on the placement, computed once
    def_bsp: u32,                   // slot of definition +0x70 collision BSP
    def_flags: u32,                 // what 0x1689b0 tests
    placement_addr: u32,            // for 0x168b40's result transform
}
```

`Instance::inverse` is the one place a retail *computation* is hoisted to
decode time. It is legal because `0x141590` is a pure function of the
placement, which is tag data, and it must be computed with the same
operation order. S3 proves it against the lifted `0x141590` for every
placement on the disc.

### 4.3 MOPP `Tree`

The decoder walks the bytecode from `+0x30` carrying the interpreter state
(shift, three frame offsets, key offset) and emits a node per *reached
(pc, state)* pair. Jumps (`05..07`), key-offset opcodes (`09..0b`),
rescales (`01..04`) and property 0 writes (`60`, `64`, `68`, which stay in
the interpreter's stack state) emit nothing for the box view: their
effect is folded into the thresholds and keys of the nodes below them. A
pc reached with two different states is emitted twice. Decode fails
closed. A reached point the interpreter faults or asserts on (an opcode
outside its table, property slots `61..63`, `65..67`, `69..6b`, an
operand past the program's size, a rescale below shift 0) becomes a
`Trap` node, and a query that reaches it bails to the lifted
interpreter. A program that unfolds past 16 nodes per bytecode byte is
marked `guest_only`, and every query on it bails. Thresholds are `i64`,
so no folded value can overflow.

The box view (`sim_collision::mopp::Node`) is an enum over thresholds on
the full-precision corners, children as node indices, the low child
always next:

```rust
pub enum Node {
    Split { axis: u8, below: i64, above: i64, low: u32, high: u32 },     // lo < below, hi >= above
    Diagonal { op: u8, shift: u8, below: i64, above: i64, low: u32, high: u32 },
    Bound { axis: u8, above: i64, below: i64, next: u32 },               // on while hi >= above, lo < below
    Leaf { key: u32 },
    Trap { pc: u32 },
}
```

The ray view (S3) adds the frame-space float operands and rescales:

```rust
#[repr(C)]
pub struct Node {                 // 32 bytes
    kind: u8,                     // Split, Split1, Diag, Bound, BoundWide, Leaf
    axis: u8,                     // 0..2, or the diagonal opcode
    shift: u8,                    // frame shift at this node (diagonals, ray)
    rescale: u8,                  // index+1 into `rescales` to apply before this node (ray only), 0 none
    lo_child: u32,                // node index, MAX = none
    hi_child: u32,
    box_a: i32,                   // full-precision threshold for the low test
    box_b: i32,                   // full-precision threshold for the high test
    ray_lo: f32,                  // frame-space operand, already converted
    ray_hi: f32,
    key: u32,                     // Leaf: terminal key with the key offset folded in
}
pub struct Tree {
    origin: [f32; 4], scale: f32, // code object +0x00, +0x0c
    nodes: Vec<Node>,             // DFS order: low child is always index + 1
    rescales: Vec<Rescale>,       // { sub: [f32; 4], mul: f32 /* 2^k */, offset_add: [f32; 4] }
    code_addr: u32, guest_only: bool,
}
```

**The box lift.** The box interpreter (`0x002ff8b0`, model
`mopp::boxvm`) compares frame values `L = (lo >> s) - off` and
`H = (hi >> s) + 1 - off` with operand bytes. For integers,
`(x >> s) < K` is `x < (K << s)`, so:

| Opcode | Retail test | Decoded test on full-precision `lo`, `hi` |
|---|---|---|
| `10..12`, `23..25` | low when `L < Lop`; high when `H > Hop` | low when `lo[a] < (Lop + off) << s`; high when `hi[a] >= (Hop + off) << s` |
| `20..22` | low when `L <= S`; high when `H > S` | low when `lo[a] < (S + off + 1) << s`; high when `hi[a] >= (S + off) << s` |
| `26..28` | ends when `H < l` or `L >= h` | continues when `hi[a] >= (l + off - 1) << s` and `lo[a] < (h + off) << s` |
| `29..2b` | full precision already | unchanged |
| `13..1c` | sums of per-axis `L` and `-H` against `m * op - k` | not liftable (a sum of floors): the node keeps `shift` and folds the offsets and `k` into `box_a`, `box_b`; the query shifts its corners by `shift` and sums |

`box_a` and `box_b` are those right-hand sides. A box query is then a DFS
over 32-byte nodes with two integer compares per split and no state copy.
Keys come out in the retail order because the DFS order is the same (low
child first) and each node's visit condition is the same predicate.

Two conditions make the table exact, and both are checked per query:

- the query has `lo <= hi` on every axis. The retail one-plane split takes
  the low child whenever `H <= S` without looking at `L`, which equals
  `L <= S` only for an ordered box. An inverted box bails;
- every corner lies in `[-2^28, 2^28)`. The interpreter forms its frame
  values and diagonal sums in 32 bits; inside that range none of them
  wraps, so the `i64` thresholds compare the same way. A far box bails.

**The ray view.** The float interpreter (`0x003009e0`) cannot be lifted the
same way: a rescale subtracts and scales the segment endpoints in `f32`, and
that rounding feeds every later test. So the ray walker
(`sim_collision::mopp::ray`) replays the retail float sequence per opcode,
walking the bytecode the `Tree` keeps: the rescale bytes as floats and
`2^k`, and the split thresholds `Lop * m - k` and `Hop * m - k`, exact in
`f32`. What it saves is the dispatch, the state copies and the guest call
per split. Walking the folded nodes instead (thresholds and rescales
converted once) is S8's layout work.

### 4.4 Rings and the inside test

The retail inside test (`0x001debd0`, E) walks the surface's edge ring. For
each edge it takes vertex A (the end vertex when `edge.right_surface` is the
surface, else the start) and vertex B (the other), projects both, and
rejects the point when

```
(Bv - Av) * (pu - Au) - (pv - Av) * (Bu - Au) > 0
```

The matcher in `sim_collision::bsp::ring` packs rings of up to eight edges.
Each group stores `A` and `B - A` for four edges by axis, repeating an
edge in any spare lane. Decode computes the differences once with
binary32 subtraction under the host float guard. Each reference selects
its own projection axes and the kernel computes the cross product with
two subtractions, two multiplications and one final subtraction per lane.
The existing native SIMD switch selects host SIMD or scalar lane arithmetic.

The packed test uses the same range guard as the scalar edge loop. If a
group fails that guard, the scalar loop replays the test in ring order.
This preserves an early rejection even if a later lane needs a decline.
The scalar loop also handles long rings, coordinates outside the packed
range and rings that exceed the cache budget.

### 4.5 Memory

Per map on the multiplayer set the structure BSP is the bulk. The budget is **≤ 8 MiB
per loaded map** for the whole collision world, inside the +32 MiB the plan
allows all Rust sim state. The world charges decoded array capacity,
including ring edges and packed groups, the packed leaf table,
reserved address-index capacity and the BSP owners.
On first overflow it releases expanded rings and uses scalar guest edge
reads for that generation. Essential tables that exceed the budget yield
a live cluster read or a `budget` bail. The disc sweep reports retained
bytes and fallbacks for each map. See [collision-queries.md](collision-queries.md).

## 5. The guest boundary

### 5.1 Rules for every adapter

1. **Gate.** Run the kernel only when the guest float environment is the
   supported one: MXCSR round-to-nearest, all exceptions masked, FTZ and DAZ
   off, the precision flag sticky or not (`det::guest_env`; the kernel
   raises PE as the guest's SSE code does); x87 control all-masked,
   round-to-nearest, precision control single or double. Otherwise the
   call panics by name (`collision: <entry>: env`).
2. **Compute, then commit.** The kernel returns `Ok(result)` or `Err(Bail)`
   before any guest write. On `Bail` the adapter returns "not handled" from
   the untouched entry state and the hook panics with the bail's name
   ([collision-queries.md](collision-queries.md), "Bails and stats").
   No entry runs the lifted call as a fallback.
3. **Checked region.** `sim_core::det::checked` clears the host's float
   status flags, runs the kernel, and bails if invalid, divide-by-zero,
   overflow or underflow was raised. With finite inputs (rule 4) that proves
   every intermediate was finite and no inexact subnormal was produced, which
   is exactly the set of values on which NEON, SSE2 and the guest's SSE and
   x87 agree bit for bit. The checked scalar operations use side-effecting host instructions. The
   result passes through `core::hint::black_box` before the flags are sampled.
   The region restores the incoming host status on return and unwind. A spurious flag only costs a bail.
4. **Finite inputs.** Static data is checked at decode (`Guard::finite`;
   a BSP or tree with a non-finite float is `guest_only`). Dynamic inputs
   (ray start and delta, limits, sphere centre and radius, object bounds)
   are checked per query with one lane compare.
5. **x87 sequences** are evaluated in the working format the guest's
   precision control selects, through `sim_core::det::Chain`:
   - single (`PC = 00`): `f32` operations;
   - double (`PC = 10`): `f64` operations on the widened operands, rounded to
     `f32` only where the retail code stores a single;
   - extended (`PC = 11`): bail.
   A value retail keeps unrounded on the x87 stack and compares (the MOPP
   ray's diagonal projections, the portal plane distance) is compared
   unrounded in that format. Entry histograms record which modes reach the
   running game. Synthetic tests compare both supported modes with `x87`.
6. **Registers.** The contract lists the registers, flags and x87 state the
   entry writes. Replay each value that can reach a caller or a status
   save. Omitting a value requires a liveness proof for every continuation,
   including branches, tail calls and indirect callbacks.
7. **Stack.** Replay observable writes. An exclusion in the differential
   oracle names a bounded frame or argument slot and requires a caller
   liveness proof before the adapter can be installed.
8. **Faults.** A kernel reads guest memory through the checked bus. A fault
   during gather is a `Bail`: the original then faults at the retail
   instruction.

A–D specialize these rules. The bounded BSP expressions use `Fp` under
nearest rounding, accepting FTZ/DAZ because all their operands and computed
nonzero results are normal. C/D's bounded split division also admits
extended guest precision. A/B replay all registers and the entire stack
frame. See the [adapter contracts](collision-queries.md).

The direct callers of C and D can forward volatile values to their own
callers. Both entries run lifted while their candidate kernels are compared
under the narrower masks in the differential harness (section 8.1).

### 5.2 Entries taken over

"Subsumes" lists functions that stop running when the switch is on. They
stay in the image and still run on a bail, and for callers outside the
family.

| # | Entry | ABI (E unless marked) | Subsumes | Guest writes the adapter must make |
|---|---|---|---|---|
| A | `0x0014a280` point to leaf | `node: eax, bsp: ecx, point: edx -> eax` | - | saved ESI and EDI at entry ESP−4 and ESP−8 |
| B | `0x0011bed0` location from point | `out: esi, point: edx`; reads `[0x4686c4]` (BSP index), `[0x4e033c]` (collision BSP), `[0x4e0348] + 0x30` (leaf table) | A | `out+0` leaf `u32`, `out+4` cluster `i16`, `out+6` BSP index `u16`; the no-structure and solid exits use the signed sentinel. Saved stack words are listed in the [entry contract](collision-queries.md#clobbers-and-dead-stack). |
| C | `0x001de630` BSP vector test | `out: eax, flags: ecx, bsp: edx, limit: xmm0`, stack: breakable count `u16`, breakable mask pointer, start pointer, delta pointer; `-> al`; `ret 0x10` | `1de6d0`, `1dea00`, `1debd0`, `296520` | the result record R at `out` (below) |
| D | `0x001de6d0` BSP vector test, context form | `ctx, node, t0, t1` on the stack; `-> al`; `ret 0x10` (P). Called directly by `0x001eff40` | its own recursion, `1dea00`, `1debd0`, `296520` | R through `ctx+0x18`; `ctx+0x1c` previous leaf, `ctx+0x20` previous kind and `ctx+0x24` last split plane |
| E | `0x00244980` instanced-geometry trace of one cluster | `cluster: eax, start: esi, delta: edi`, stack: flags, the BSP filter bits from `0x168ae0`, result pointer; `-> al`; `ret 0xc` | `2df760`, `2df720`, `301800`, `3009e0`, `2df680`, the shape callbacks at vtables `0x459e0c`/`0x459e48`, `244c10`, `1691a0`, `1689b0`, `141590`, `183fc0`, `168b40`, C | instance marks `[0x4e7c20 + 4 * i] = [0x4e7c1c]` for every instance retail would test, in retail order; the query result record (5.4) when an instance hit is kept |
| F | `0x00300150` MOPP box query | `ecx` box VM, stack `code, aabb, array` in increasing address order, `ret 0xc` | `2ff8b0` | VM fields below, ordered key appends at `array.data[size..]` and `array.size` |
| G | `0x0014a5b0` sphere cluster flood | stack: `cluster: i16, centre, radius, capacity: i16, out`; `-> ax` count; `ret 0x14` | its own recursion, `14a370`, `120850`, `23a160` | `out[i]` cluster `u16`s; cluster marks `[0x4e7418 + 4 * c] = [0x4e7414]` |
| H | `0x000bb050` object spatial visit | `lists, types, location, centre, radius, out, capacity`; `ret 0x1c` (P) | G, `b8940`, `b8a10` | `[0x4e7414]` increment and `[0x4e7411]` set and clear; `[0x4de2fc]` increment; `[0x4de2f8]` set and final value; object marks `[0x4de300 + 4 * slot]`; `out[i]` handles; `ax` count |
| I | `0x00301800` MOPP ray, general form | `ecx` ray VM, stack `collection, code, ray_input, parent_body, collector` in increasing address order, `ret 0x14` | `3009e0` | VM fields below and writes made by guest callbacks |
| J | `0x001697c0` collision query | `flags, start, delta, ignore_a, ignore_b, out`; `-> al`; `ret 0x18` (P) | C, E, `16b500`, `168ae0`, `11e5e0`, `b8940`, `b89b0`, `169510`, `1eff40` and helpers, B | 5.4 |

The segment–sphere kernel in `sim_collision::sphere` answers the
`0x0011e5e0` call in J through the adapter in
`engine::sim_collision::sphere`. The adapter keys on J's return PC,
`0x00169d3f`. E and the volume query continue through the lifted function.

#### The BSP result record R (entry C and D) (E)

| Offset | Field | Written when |
|---|---|---|
| `+0x00` | `f32` hit fraction; initialised to `max(limit, 0)` | always, then on hit |
| `+0x04` | guest pointer to the hit plane | hit |
| `+0x08`, `+0x0c` | lookup leaf index, accepted surface index | hit |
| `+0x10` | sign-extended surface plane word (bit 15 flipped) | hit |
| `+0x14`, `+0x15` | surface flags and breakable index, one byte each | hit |
| `+0x16` | `i16` material index | hit |
| `+0x18` | `u32` leaf count | always (0 at start) |
| `+0x1c..` | up to 256 leaf indices in visit order | per visited leaf |
| `+0x418` | last leaf when the list is full | overflow |

`0x001dea00` returns the surface. Its caller writes the accepted fields
at `0x001de94c..0x001de9a2`. The adapter names each in
`crates/rewrites/engine/src/sim_collision/layout.rs`.

Breakable surfaces: a surface whose `breakable` index is below the count
argument (0x100 for the structure, 8 for an instance) is skipped when its
bit is clear in the mask the caller passes (`0x1debec`, E). The mask is live
guest state. The kernel reads the mask words it needs during gather. It is
never cached.

### 5.3 Retail semantics the kernels must reproduce

**Point to leaf** (P, `sim_native/collision.rs`): per node
`dist = ((nz*pz + ny*py) + nx*px) - d`; front child when `dist >= 0`
(unordered goes back); stop at a leaf or at none.

**Segment walk** (P, `sim_native/bsp.rs`, collision.md "What the engine does
with a BSP"): per node `dist = ((pz*nz + py*ny) + nx*px) - d`,
`delta = (dz*nz + dy*ny) + dx*nx`, `s0 = delta*t0 + dist`,
`s1 = delta*t1 + dist`. Same side: descend. Else split at
`t = -(dist / delta)` (an x87 divide, negate and single store: one `Chain`),
near child first with `[t0, t]`, then the far child with `[t, t1]`, carrying
the crossed plane. Leaf states, the three hit patterns, the fall-through on a
missed lookup, and the leaf list are as the port has them. The Rust walk uses
an explicit stack of `(node, t0, t1, plane)` and stops at depth 256 with a
bail, where the port resumes the guest.

**Surface matcher** (E, `0x001dea00`): for each reference of the leaf whose
masked plane index equals the crossed plane: project the hit point with the
reference's `proj`, walk the 2D tree (`a*u + b*v - d >= 0` front), and
accept the first surface that passes the query-flag filters (surface flag
bit 1 against query bit 3, bit 3 against bit 4), the breakable test, and the
ring test of section 4.4.

**Instance trace** (E, `0x00244980`, `0x00244c10`, `0x001691a0`): walk the
cluster's MOPP tree with the ray from `start` to `start + delta` (each
component one `f32` add). For each terminal key, in walk order:

1. Index the mark with the signed low 16 bits of the key. If that mark
   equals the epoch `[0x4e7c1c]`, skip. Otherwise set it. Placement lookup
   and result +0x3c use the full key. The callback also requires a live
   code-object reference count to elide its destructor (see [entry E](collision-queries.md#entry-e---instance-trace-of-one-cluster)).
2. Filter by definition flags (`0x1689b0`), then the sphere pretest against
   the placement sphere.
3. Transform start and delta by the placement inverse (the scale multiply is
   skipped when the scale compares equal to the unit constant, `0x169221`).
4. Run the vector test on the definition BSP with the current best fraction
   as limit, breakable count 8 and the instance's mask from `0x183fc0`.
5. On a hit write result type 3, the material, the instance index at
   `+0x3c`, and transform the plane back (`0x168b40`).
6. Then the ray's early-out fraction becomes 0 under query flag
   `0x10000000`, else `min(fraction, result.t)`.

**MOPP ray walk** (P, `sim_native/ray.rs` and `mopp_query.rs`; E for the
return sites at `0x300e9d` and `0x300ff2`): endpoints are in frame units.
Rescale applies `(P - bytes) * 2^k` to both ends. A split projects both
ends, takes one child when both ends are on one side, and otherwise clips a
copy of the segment for the first child (the child the start lies in), then
on return clips the start for the second child. If the early-out fraction
has dropped below one, it also recomputes the end from the world-space ray:
`(((1 - f) * from + f * to) - origin) * frame_scale - frame_offset`, packed.
After a plain axis split it compares the new end on that axis with the far
bound and skips the second child when the ray now stops short. Clip weights
are x87: `w = left / (left - right)`, stored single. The walker is
pull-style:

```rust
pub struct RayWalk<B: AsRef<[u8]>> { /* the bytecode, an explicit stack of levels */ }
impl<B: AsRef<[u8]>> RayWalk<B> {
    /// Each interpreter call (pc, frame, segment) and each terminal key.
    pub fn next(&mut self, fraction: u32) -> Result<Option<Event>, Bail>;
}
```

The caller handles a terminal and passes the updated fraction to the next
call. Entry E drives it with the native instance test. Entry I runs the
walk and the filter, child and child-ray callbacks in Rust
(`CollectorCast::run`, `mopp_ray::callbacks`; collision-queries-entries.md,
"Entry I"), dispatched on the address each vtable slot holds.

**Ray VM at entry I** (E, `0x002df74d`, `0x00301800`). The tree shape's
slot `+0x24` (`0x002df720`) constructs a VM on its stack and passes the
shape's collection at `+0x0c` and code at `+0x10`, followed by its own
three arguments. After the call it restores its frame and saved registers.
The result travels through the collector. The setup and terminal together
establish these VM fields:

| Offset | Value |
|---|---|
| `+0x10`, `+0x14` | Code pointer and the single-precision store of the x87 reciprocal of its scale. |
| `+0x20`, `+0x30` | Two complete 16-byte vectors copied from ray input, the original endpoints before frame conversion. |
| `+0x40`, `+0x44` | Two words copied from ray input `+0x20` and `+0x24`. The second points to the optional filter. The child and filter receive the copy beginning at VM `+0x20`. |
| `+0x50` | One-byte hit flag cleared at setup. Only the sibling output-record path (`0x00301720`) sets it at a terminal. |
| `+0x54` | Current early-out fraction, copied from `collector+4` at setup and refreshed from the collector after a child callback. |
| `+0x58` | Output-record pointer used by the sibling path. Entry I clears the word, selecting the collector path. |
| `+0x5c`, `+0x60`, `+0x64` | Collector, parent collision body and child-shape collection pointers. |

At a terminal (`0x0030156c..0x0030160f`) the optional filter can reject
the key. On acceptance the VM resolves a child through collection slot
`+0x34`. It calls child slot `+0x24` with the copied ray input, a temporary
child body and the collector. The child body carries the child shape, key,
word at `parent_body+8` and parent body pointer. The VM reloads the
collector's fraction before traversal resumes. Intervening padding in the
VM is untouched by setup. Root segment coordinates use packed subtraction
of the code origin followed by packed multiplication by the stored root
frame scale. That scale is a separate x87 product of the code scale and
the guest scalar at `0x0045dcac`.

**Box query** (E, `0x002df659`, `0x00300150`; P, `mopp::boxvm`).
The tree shape's slot `+0x30` (`0x002df640`) supplies the box VM on its
stack. F writes the key-array pointer at VM `+0` and the code pointer at
`+0x30`. For x, y, z in that order it converts the minimum then the
maximum coordinate (AABB offsets `+0` and `+0x10` for x). Each conversion
subtracts the corresponding code origin and multiplies by the code scale
in x87 working precision, stores to binary32, reloads, converts with
`fistp` and decrements with 32-bit wraparound. The VM stores the minimum
integers at `+0x20..+0x28` and the maximum integers at `+0x10..+0x18`,
without writing a fourth lane. F starts the DFS of section 4.3 at
`code+0x30`. Keys are appended to the guest
`hkArray`. If `size + keys > capacity & 0x7fff_ffff` the kernel bails
(`grow`) and the lifted code runs and grows the array itself.
`collision.stats` counts those bails for `mopp_box`; if they are above 1%
of calls, the adapter becomes `async` and awaits `0x002d91d0` at the same
points retail calls it. The six conversions run as `det::Chain`s in one
checked region; the adapter sets PE in the x87 status word when a
conversion rounds, as retail's `fistp` does, and accepts PE clear.

**Cluster flood** (E, `0x0014a5b0`; P for the portal test,
`sim_physics/spatial.rs`): write the cluster to the output while capacity
remains, mark it, then for each portal of the cluster take the cluster on
the other side; if unmarked and the sphere passes the portal test, recurse.
The portal test is plane distance (x87, compared unrounded), bounding
sphere (SSE), then projection on the plane's dominant axis and the 2D
polygon test `0x0023a160` with radius `sqrt(r*r - d*d)` (x87). S5 ports
`0x00120850` and `0x0023a160` into the kernel. The returned count includes
clusters visited after the output filled (the caller clamps it), and the
flood uses the epoch its caller set: it does not bump `[0x4e7414]` itself.

One caller, `0x001cac60`, links a handle into the spatial partition. Both
the object connector (`0x000b8710`, supplying a 0x14-byte reference
payload) and the light-list insert (`0x000c30f0`, no payload) use it. On
its sphere-flood path it increments the epoch, sets the active byte, calls
G at `0x001cace3` with room for 256 clusters and clears the byte. It links
each accepted cluster into the handle's list and each handle into the
cluster's list (`0x001cad30..0x001cadce`). [Object and light list callers](object-update.md)

**Object visit** (P, `sim_physics/visit.rs`): for each flooded cluster walk
the collideable list (heads `[0x4de2e0]`, datum array `[0x4de2e4]`) and the
other list (`[0x4de2d8]`, through `0x000b8a10`); per reference: type-mask
test, visit mark test and set (set before the bounds test), sphere overlap
`dist2 <= (r_ref + r)^2` with the axis order the port records for each list,
append the handle, stop when the output is full.

### 5.4 The collision query (entry J) (E)

`0x001697c0`, in retail order:

1. Default the flags (`flags & 0x1800000 == 0` sets both bits). Initialise
   the result: type 0, `t = 1`, both locations `{leaf -1, cluster -1, bsp}`,
   `+0x38`, `+0x3c`, `+0x40` to `-1`.
2. Structure trace: entry C on the structure BSP with the filter bits from
   `0x168ae0`, breakable count 0x100 and the structure's breakable mask
   (`[0x4ed280] + 32 * bsp + 1`).
3. On a hit with flag bit 0: type 1, the material through the structure's
   material block, the plane (negated when flipped), the R words.
4. If the leaf list is non-empty: first and last leaf and their clusters go
   to `+0x14..+0x22`.
5. If flags `& 0xe` and not (hit and `0x10000000`): increment `[0x4e7414]`
   and set `[0x4e7411]`; under bit 2 increment `[0x4e7c1c]` and set
   `[0x4e7c18]`; under bit 3 increment `[0x4de2fc]`, set `[0x4de2f8]`, and
   default the object type mask (`0x1fff0`) when empty.
6. For each leaf in list order, take its cluster; skip a cluster already
   marked in `[0x4e7418]`, else mark it. Then:
   - bit 1 tests the cluster's liquid boundary (`0x00169a9c..0x00169c28`).
     Cluster byte `+0x70` must have its high bit set and differ from
     `0xff`. Its low seven bits index a 0x18-byte record through structure
     `+0x68`. The plane starts at record `+4`. Its material at `+2` must
     differ from `-1`. With signed starting distance `s` and
     `v = n·delta`, the tests require different `> 0` results,
     `abs(v) > abs(s)` and `abs(v)` at least the guest threshold at
     `0x45dbdc`. For a smaller fraction `0 - (s / v)`, write type 2,
     fraction, material at result `+0x24`, plane at `+0x28` (each component
     subtracted from zero if `s < 0`), and record index at `+0x38`.
     The effect classifier at `0x00178d3b` uses the same record to
     distinguish air from liquid. A cluster with a valid material and
     high bit clear is wholly in that medium, so it has no boundary to
     hit. [Particle medium classification](../../crates/extensions/game/src/h1_particle_physics.rs)
   - bit 2: entry E for the cluster;
   - bit 3: the object references of the cluster (`b8940`, `b89b0`): mark
     test and set in `[0x4de300]`, type mask, the reference flags against
     `0x16b500`'s bits, the [Rust sphere pretest](collision-queries.md#object-reference-sphere-pretest),
     the two ignore handles, then the
     object test `0x00169510`.
   Under `0x10000000` the first hit ends the loops.
7. Clear `[0x4de2f8]` (bit 3), `[0x4e7c18]` (bit 2) and `[0x4e7411]`.
8. Position `= delta * t + start` at `+0x8`.
9. Under `0x20000000` with a hit: the nudge along the plane and the leaf
   re-location (`0x169e71..0x16a026`, which calls A and B).

The planner in `engine::sim_collision::query` implements these steps.
Entry J runs it at entry and compares the lifted result at return.
`collision.trace` runs the same code against live inputs without
scattering the result or marks. J’s guest-visible register and stack
contract gates authority.

### 5.5 Guest readers that must keep being fed

| Reader | What it reads | Fed by |
|---|---|---|
| Weapons and projectiles (`1a4c00`, `1a5370`, `1a67f0`), damage, decals `17e670`, camera `171830` | the 1697c0 result record | J, or C/E inside the lifted J |
| Sound listener `127400` → `11bed0`; transmission `221e30` → `1de630` | location triple; R | B, C |
| Observer `16f280`, effects `17c0e0`, `211060` | location triple, leaf | A, B |
| Unit and player updates (`109580`, `1508e0`, `151110`), damage `d6c80` | visit output list and count | H |
| Every later visit or query | epoch counters `[0x4e7414]`, `[0x4e7c1c]`, `[0x4de2fc]`, flags `[0x4e7411]`, `[0x4e7c18]`, `[0x4de2f8]`, mark arrays `0x4e7418`, `0x4e7c20`, `0x4de300` | E, G, H, J write them exactly as retail does, in order |
| Havok BV-tree agent `2e9d90`, structure shape `1eea50` | the key array | F |
| Havok ray casts | the ray VM object and whatever the callees write | I |

The mark arrays are compared with the current epoch by code outside this
family (the camera and render walks share the object marks), so "written
exactly" means the same set of slots, each with the epoch value retail
would store, not merely a final state that dedupes the same way.

## 6. Batching, SIMD and threads

### 6.1 What is exact

- **Lanes over independent items.** Each lane runs the same scalar
  expression in the retail order, with IEEE `f32` add, sub, mul, div or
  integer ops. No FMA, no reassociation. Under the checked region this is
  bit-identical on NEON and SSE2.
- **Order-free reductions only.** The ring test ("any edge rejects") and
  mask tests are order-free. Anything with a retail order (which surface is
  accepted, which object is tested first, key order) is decided by a scalar
  pass over lane results in retail order.
- **Speculation is allowed, effects are not.** A kernel may compute tests
  retail would have skipped (a sphere pretest for a reference after an early
  out). It may not *commit* their effects: the scatter replays marks, list
  appends and result updates in retail order and stops where retail stops.

### 6.2 Where lanes go

| Site | Lanes | Notes |
|---|---|---|
| Ring inside test | 4 edges per group | 4.4. One or two NEON/SSE2 groups per short ring. |
| BSP node | start and delta dot products as 2 lanes of 4 | Same products and sums as retail. |
| Point to leaf, two levels at once | node and both children in 3 lanes | Optional (S8): needs a 3-plane treelet array; measure first. |
| Object references of a cluster (J step 6, H) | 4 or 8 references | Gather the list into SoA (`centre xyz`, `radius`, `type`, `flags`, `handle`), run every sphere pretest or overlap in lanes, then replay marks and hits in list order. |
| Instances of a cluster | the sphere pretests of all keys a walk produced | Only when the walk is not fraction-dependent; see below. |
| MOPP box packets | 4 or 8 boxes through one tree | Per node a mask of boxes that visit it. Every box sees its keys in DFS order, so each key list is the serial one. For phase 4's broadphase. |
| MOPP ray | endpoints as packed `f32x4`, as retail | One ray at a time. The walk order and the clipping depend on the direction and on hits found so far, so rays are not packed together. |

### 6.3 Batched queries

The Rust batch APIs plan answers in input order. A later query may read
an earlier query’s generation or mark, including at wraparound.
`engine::sim_collision::query::trace_batch` overlays staged mark writes
by physical address so an alias sees the same bytes as a serial trace.
An overlap with static decode sources declines. Each result carries the
ordered cluster, instance and object mark lists.

`sim_collision::batch::visit_batch` threads the visit marks and epochs
through the object lists. Standalone G uses its caller’s flood epoch.
`box_batch` runs independent boxes in packets of up to eight and preserves
the serial key order, depth limit and per-query failures. Batch planning
makes no guest writes. A declined batch returns no partial results.

### 6.4 Threads

The live inputs and mark dependencies keep trace and visit batches
serial. Box packets share a tree walk on the calling thread. Phase 2 has
synchronous guest callers. Later object and physics callers can supply a
batch once they have gathered independent inputs.

## 7. Lifetime and invalidation

| State | Kind | Built | Dropped |
|---|---|---|---|
| `Bsp`, `Tree`, `Structure` | static | lazily, from guest-resident tag bytes, on first use of a pointer | map load (`CACHE_TAGS_LOAD`), structure switch (`BSP_SWITCH`) and snapshot load advance the collision epoch |
| Gathered reference lists, breakable words, key lists | transient | per query | end of the call |
| `Stats` | diagnostic | - | never read by a kernel |

There is no derived cache in phase 2: nothing between "static" and
"transient". Checkpoint revert, full restore, snapshot load and tick
regression therefore need no action, because every piece of arena state a
query depends on (object references, marks, epochs, breakable masks) is read
live.

A cross-query cache of object references is possible at the runtime
adapter. A clean 4 KiB page bitmap alone cannot validate one. Under the same
guest memory guard as the gather, `Guest::watch_virtual_read` can subscribe
to every source span, including table pointers, cluster heads (even empty
heads), datum layout and backing pointers, links, handles and bounds. The
subscription sees writes through physical aliases and publishes pending
generated-code stores. Reuse requires the same guest subscription, map
generation and `write_watch_epoch()` (virtual layout and RAM replacement
revision), no read trap, and clean pages for every dependency. A changed
list head or link requires a fresh traversal and registration of its new
dependencies. Failure to watch a range requires a live gather. Marks and
query parameters remain per call. The renderer's section cache exercises
alias writes and a COW refresh that clears dirty bits. H and J use live
gathers until a cache implements this contract.
[Virtual-range watches](../../crates/runtime/runtime/src/guest.rs),
[physical-page tracking](../../crates/runtime/runtime/src/watch.rs),
[renderer cache](../../crates/rewrites/render-bridge/src/native_world/section.rs)

Entry B reads the live leaf-table descriptor and caches the packed
cluster column under a physical write subscription. Dirty pages, a new
layout or a RAM replacement change the table version. A provider that
cannot validate the source reads the selected word directly.

The runtime adapter watches the physical pages behind the BSP header,
all eight tag blocks and the decode constants. An edit through any alias,
a changed memory layout or a RAM replacement starts a fresh generation.
An unreadable or unwatchable source declines before native execution.
Each decoded BSP also stores a 64-bit source hash. The cache-cold arm
(ladder step 5) can rebuild and compare those hashes independently.

Known writers of nearby tag memory, none of which touch a decoded field:
the Havok slot initialisers (`0x246c60`, vtables and reference counts), the
MOPP reference count at code `+0x24` (`0x3009b0`), and the temporary shape
`0x244980` builds on its stack. Eliding the temporary reference requires a nonzero original count and
isolation from other reads and writes. On a decrement to zero,
`0x3009c0` calls the guest allocator. A populated E executes this path in the guest. Its predictor in J
models terminal marks, placement filtering and BSP hits.

The runtime collision world can reuse immutable decoded columns from the
user cache. The content key covers source bytes and format inputs.
Guest addresses, watch generations and the breakable state stay local.
The lazy decoder handles small, unsupported or unavailable cache entries.
See `sim-collision/src/store.rs` and the [entry contracts](collision-queries.md).

## 8. Verification

### 8.1 S0 contract checks (once per entry, static)

For each entry in 5.2, recorded in `docs/engine/collision-queries.md`:

- the ABI, frame depth and the set of registers the original writes;
- every call site (direct calls from the analysis database, plus vtable
  slots for F and I), and for each, that no register in the clobber set and
  no stack byte in the dead window is read after the call before it is
  written. Follow forwarded state through the outer returns as well;
- every `stmxcsr`, `fnstenv`, `fnsave` and `fxsave` site in the image and
  what it does with the status bits, to close rule 6;
- the x87 control word and MXCSR seen at each entry over one Waterworks and
  one campaign run (a counter in `Stats`, read through the agent API).

### 8.2 The ladder

1. **Unit tests** (`cargo test -p sim-collision`, synthetic data only):
   - BSP: hand-built trees for every leaf-state transition, both hit
     directions, two-sided leaves, the fall-through, a full leaf list, depth
     256, breakable bits set and clear, each projection row, rings of 3 to 8
     vertices and a 9-vertex ring on the scalar path;
   - MOPP decode: every opcode class; a pc reached with two states; the
     fail-closed cases; `Tree` box results equal `mopp::boxvm::query_box`
     key for key on random programs from `mopp::build` and on hand-written
     programs with diagonals, rescales and `29..2b`;
   - ray walker: every opcode family by hand, misses, ends on a threshold,
     the fraction cutting a walk short, and random programs against a
     recursive transcription of the interpreter, in single and double
     precision;
   - `det`: each flag forces a bail on this host; `Chain` against the `x87`
     crate for both precision modes; lanes against the scalar loop;
   - batch: `trace_batch` equals serial calls, including the mark lists;
     1, 2 and 8 workers.
2. **Differential tests**
   (`cargo test -p engine --features differential --test it sim_collision_differential::`,
   owner's XBE, skipped without it). Per entry: synthetic guest memory, the
   lifted entry on the IR interpreter and the adapter from the same state;
   compare the return register, all memory outside the dead-stack window,
   and all registers outside the clobber set. Seeds cover ties (a point on a
   plane, a segment ending on a plane, `t` equal to the limit), the bail
   paths (NaN, infinity, subnormal, unsupported control words), and both
   lane settings. The existing ports run as a third arm on the entries they
   cover: `point_to_leaf`, `bsp::segment`, `collision::query`,
   `mopp_query::{box_query, ray_query}`, `geometry_test`, `spatial::portal`,
   `visit::objects`. Entry I runs natively against the lifted query with
   invented callbacks; each callback's arguments and the collection's keys
   must match.
3. **Disc sweep** (`-- --ignored`, owner's disc, results local). Every
   structure BSP, every `coll` BSP, every MOPP program (structure, cluster,
   `coll` slot, `phmo`):
   - decode succeeds or is `guest_only`, with counts reported;
   - 10,000 random segments and 10,000 random points per structure BSP, and
     1,000 per object BSP, against the lifted entries C and A;
   - 400 boxes per program against `mopp::boxvm::query_box` (the mopp.md
     sampling), 400 rays per cluster program against the lifted
     `0x301800`;
   - every instance's inverse transform against the lifted `0x141590`.
4. **Run parity.** The sim-budget A/B of the native world against the
   lifted build: Lockout local, Waterworks local, Waterworks mixed, seed 7,
   1,800 ticks, `impl.sim.arena-hash` on. Every `state_hashes_v1` row and
   arena block equal; `h2 sim diff` prints nothing. One headless campaign
   level through the agent API with arena hashes.
5. **Cache-cold arm.** The same runs with the world dropped and rebuilt
   every tick, and the source hashes compared.
6. **Cross-architecture.** The Waterworks case on x86-64 and arm64: equal
   hash series, and equal `Stats` bail counts (a bail that happens on one
   architecture only would be a determinism bug even with equal hashes).
7. **Bail audit.** `Stats` reports bails by entry and reason. Exit needs
   under 0.1% of calls bailing on the three cases, with each reason named.

### 8.3 Agent API

A lane needs to see bail counts, decode sizes and float-environment
histograms on the user's running serve, and later lanes will need the same
view, so the API is expanded: `collision.stats` (the `Stats` struct) and
`collision.trace { start, delta, flags }` (run entry J's kernel read-only
and return the result and mark lists without scattering). Both go in
[agent-api.md](../runtime/agent-api.md) with tests. `collision.trace` is
also how a behaviour suite checks a level's collision without a weapon.

## 9. Slices

Each is one lane. "Share" is the with-services samples of the functions the
slice stops running (section 2). A slice lands with ladder steps 1 to 3 clean; steps 4 to 7 run
on the coordinator's integration pass.

| # | Slice | Delivers | Share | Tests |
|---|---|---|---:|---|
| S0 | Contracts and core | `sim-core::det` (`checked`, `Chain`, guards), `Epoch`, lanes moved; `sim-collision` skeleton with `GuestRead`, `Bail`, `Stats`; `collision-queries.md` with the 8.1 checks for entries A to E; the differential harness helper (dead window, clobber set); `collision.stats` | - | `det` unit tests on both architectures |
| S1 | BSP decode, point, location | `bsp/decode`, `bsp/point`, `Structure::leaf_cluster`; entries A and B; per-map size report | 514 | unit, differential A and B, disc sweep points |
| S2 | BSP segment and matcher | `bsp/segment`, `bsp/ring`; entries C and D; R pinned in `layout.rs` | 824 | unit, differential C and D with the `bsp::segment` port as third arm, disc sweep segments |
| S3 | MOPP decode, ray walker, instance trace | `mopp/decode`, `mopp/ray`, `structure` instances, `instance`; entry E | ≥ 437 visible; most of the 1,644 family | unit, differential E, disc sweep rays and inverses |
| S4 | MOPP box | `mopp/boxq` single and packet; entry F; bail-rate report | 509 (`2ff8b0`, `300150`) | unit against `boxvm`, differential F, disc sweep boxes |
| S5 | Cluster flood and object visit | `flood`, `objects`; entries G and H; `120850` and `23a160` in the kernel | 1,012 | unit, differential G and H with the two ports as third arms; native exits and lifted-return comparisons |
| S6 | General MOPP ray | `mopp::ray` (`RayWalk` over the bytecode); entry I's parity arm, VM and callback comparison, isolated collector differential; `collision.stats` `ray` | none until I answers natively | unit, recursive transcription, differential I with invented callees, a deliberate mismatch |
| S7 | Whole query | Rust object-reference sphere pretest; port `169510`, `1eff40` and helpers (object models through node matrices); entry J as one call; `collision.trace`; batch API with the reference-list lanes | 1,025 | unit, differential J, run parity |
| S8 | Layout tuning | DFS re-layout with a guest-index map, two-level point treelets, only where a profile pair shows a gain | - | cold/warm and re-layout on/off give equal bits |
| S9 | Census entries | `1ddea0`, `16a8e0`, the structure child shapes `1ef950`/`1edd00` (with the Havok lane), by campaign census | 318 + campaign | as S2 |
| S10 | Shared store | `store.rs`, mmapped per map | host RSS | stored equals lazily built, byte for byte |

Order: S0, then S1 and S3's decoder in parallel, S2, S3, S4 and S5 in
parallel, S6, S7. S2 before S3 because the instance trace calls the segment
kernel. A single collision switch selects these entries. The point,
sphere, BSP segment, whole query, general ray, visit and portal
register ports remain available to the differential harness.
The separate integer MOPP interpreter belongs to the Havok world step:
The Havok world step compares its box query with the guest as a
capture arm, and no per-function `native_mopp_box` switch remains.

Lane ownership: new `crates/rewrites/sim-collision`, `crates/rewrites/engine/src/sim_collision`,
`crates/rewrites/engine/tests/sim_collision_differential.rs`,
`docs/engine/collision-queries.md`; edits to `crates/rewrites/sim-core` (S0),
`crates/rewrites/sim-bridge/src/sim_native.rs`, `crates/hooking/guest-layout/src/points.rs`, the agent
crates for 8.3, and the workspace manifests. Nothing owned by Live, bots,
theater or film.

## 10. Interfaces for the later phases

- **Phase 3 (objects).** `trace`, `trace_batch`, `visit`, `visit_batch`,
  `locate(point) -> Location` as plain Rust calls on gathered state, with
  the mark lists returned for the caller to scatter. Projectile and biped
  updates then batch their traces per walk.
- **Phase 4 (Havok).** `box_query`, `box_batch` over any `Tree`;
  `RayWalk`; `Bsp::surface_ring(surface)` for the structure child shapes;
  `Tree` for `phmo` and `coll` slot programs.
- **Halo 1 content.** Converted maps go through the same decoders. Their
  MOPP programs come from `mopp::build`, which emits no diagonals, so every
  node of a converted level takes the lifted compare path.
- **Stats and tools.** `collision.trace` and `Stats` through the agent API.

## 11. Risks

| Risk | Effect | Handling |
|---|---|---|
| A caller reads a register or dead stack byte the original left behind | wrong guest behaviour, caught by `h2 sim diff` at best | the S0 call-site scan per entry; an entry that fails it keeps the affected register or frame write in its adapter |
| The guest runs these entries with x87 precision control extended | every x87 kernel bails; no win on the MOPP ray, flood and box setup | S0 measures it first; if so, `Chain` gains an extended mode on exact-sum checks, or those three sequences call the `x87` crate |
| Host float flags are not a sound detector on some target | a non-finite or subnormal case slips through | the `det` tests force each flag on both architectures; inputs are also range-checked; Windows arm64 and x86-64 run the same tests |
| The compiler moves float work across the flag read | missed bail | results go through `black_box` before the read; the forced-flag tests are shaped like the kernels |
| The `hkArray` grows often | F bails too much to pay | measured in S4; the `async` grow path is specified |
| MOPP programs share subtrees under different states and the unfolded tree blows up | decode bound hit, program `guest_only` | bound and count reported by the disc sweep before S3 lands |
| Something writes decoded tag bytes at run time | stale static data | source hashes in the cold arm and the debug round robin |
| `169510` and `1eff40` reach more guest state than read so far (damage regions, node matrices mid-update) | S7 grows | S7 starts with its own contract step; J stays lifted until it passes, and the smaller entries already carry about three quarters of the share |
| Sticky MXCSR or x87 exception bits differ from retail after a kernel | the CRT can observe exception flags | preserve observable flags and compare them in the differential oracle |
| An instance's hoisted inverse differs from the per-query one | wrong instance hits | disc sweep compares every placement; any mismatch moves the computation back into the query |

## 12. Not resolved

- The x87 control words and MXCSR values reached in Waterworks and campaign. The per-entry histograms in `collision.stats` collect both.
- Who calls `0x002df720` outside `0x00244980` on Waterworks (the stacks are
  cut by trampolines); S6 does not depend on the answer.
- Campaign cost of this family: no census yet.

## Summary

1. Phase 2 replaces the lifted collision queries with `crates/rewrites/sim-collision`:
   BSPs and MOPP programs decoded once per map into flat, mmap-ready arrays.
2. Measured share: 6,058 of 84,358 Waterworks samples (7.18%, 1.01 ms/tick),
   target ≥ 0.8 ms/tick removed.
3. Ten entries have native query kernels (`14a280`, `11bed0`, `1de630`, `1de6d0`,
   `244980`, `300150`, `14a5b0`, `bb050`, `301800`, `1697c0`), each as one
   Rust call with no guest calls.
4. Kernels compute first and commit after; anything inexact returns `Bail`
   and the lifted body runs, so exactness never depends on a hard case.
5. Exactness comes from retail operation order, a host float-flag checked
   region, finite-input guards, and x87 chains run in the guest's precision.
6. The integer MOPP box test lifts to full-precision thresholds at decode;
   the float ray keeps the retail sequence on a decoded tree.
7. SIMD goes where order does not matter (ring edges, sphere pretests, box
   packets); every ordered decision is replayed in retail order.
8. Trace and visit batches forward pending mark and epoch writes in call
   order. Independent boxes share a packet walk.
9. No derived caches: static data per map, everything else read live, so
   revert and snapshots need nothing.
10. Slices S0 to S7 are lane-sized and ordered; the contracts, float core and
    differential harness support S1. Section 12 lists outstanding measurements.
