# Collision queries

The native collision world answers point and location queries (A/B).
The vector queries and instance trace (C–E) run lifted with entry FP
observers. The differential harness also exercises the C/D kernels. The entry adapters live in
`crates/rewrites/engine/src/sim_collision`, the kernels in
`crates/rewrites/sim-collision` and the installer in
`crates/rewrites/sim-bridge/src/sim_native.rs`. See the
[switch table](../runtime/switches.md) and the [phase 2 design](sim-rewrite-collision.md).

The adapter gathers inputs, checks that the guest writes can succeed and
computes the complete answer before it changes a register or guest byte.
A declined query resumes the lifted entry from its original state.

## Entries

| | Entry | Point | ABI | Writes |
|---|---|---|---|---|
| A | `0x14a280` point to leaf | `COLLISION_POINT_TO_LEAF` | `ecx` BSP, `eax` node, `edx` point; leaf in `eax`, `ret` | leaf, scratch registers and two saved words (below) |
| B | `0x11bed0` location | `COLLISION_LOCATION` | `edx` point, `esi` the 8-byte location; `ret` | `[esi]` leaf `u32`, `+4` cluster `i16`, `+6` BSP index `i16` |
| C | `0x1de630` vector test | `COLLISION_VECTOR_TEST` | `eax` R, `ecx` query bits, `edx` BSP, `xmm0` limit; stack: breakable count (`u16`), breakable mask, start, delta; `ret 0x10`; hit in `al` | R, `al`, x87 PE |
| D | `0x1de6d0` vector walk | `COLLISION_BSP_SEGMENT` | stack: context, node, `t0`, `t1`; `ret 0x10`; hit in `al` | R, the context's `+0x1c`, `+0x20`, `+0x24`, `al`, x87 PE |
| E | `0x244980` instance trace | `COLLISION_INSTANCE_TRACE` | `eax` cluster, `esi` start, `edi` delta, three stack arguments, `ret 0xc` | `al`, terminal marks and hit-result fields described below |

The installer gives both implementations and the three observers to the collision switch. The implementations use the owning executor because they update shared caches and subscriptions. The older
per-function switches (`native_point_to_leaf`, `native_bsp_segment`) are
skipped at these points while the collision switch is on. Their bodies stay
as differential oracles.

### Entry B

The guest saves EDI, then reads the `u16` at `0x4686c4` (the active structure BSP index).
`0xffff` gives `{-1, 0xffff, 0xffff}` without a walk. Otherwise B runs
entry A from node 0 on the BSP at `[0x4e033c]` and stores the leaf at
`[esi]`. It then reads the cluster `i16` at
`[[0x4e0348] + 0x30] + 8 * leaf`, or uses `0xffff` for solid, stores it at
`+4`, and stores the index at `+6`. Because the leaf is stored before the
cluster is read, the adapter bails (`alias`) if `[esi]` overlaps any of the
three words the cluster read goes through. B reads the current structure and leaf-table pointers on each active lookup.
A watched table is decoded into one signed cluster word per leaf.
The version covers its extent and the physical pages behind it. Dirty
pages (including stores through aliases), a remap, a RAM replacement or a
map epoch change require a fresh decode. If the count, range or watch is
unusable, B reads the selected cluster word directly.

### Entry E — instance trace of one cluster

Entry E runs lifted. The ABI at `0x244980` is a cluster index in `eax`, a
start pointer in `esi`, a delta pointer in `edi`, and three stack words
at entry ESP +4, +8 and +12 (query flags, BSP filter bits, result pointer).
`al` returns whether any terminal hit. Both exits pop the three arguments.
The cluster record is at `[[0x4e0348] + 0xa0] + 0xb0 * cluster`. A signed
count at cluster +0xa8 less than or equal to zero returns false before
reading the ray, result or code object.

With a positive count, E uses the code object at cluster +0xac. It sets
the ray endpoints to start and start + delta (one binary32 add per axis),
and initializes the collector's limit from result +4. The collector path
in `0x301800` uses the stack-local collection, child and collector methods
`0x244bb0`, `0x244be0` and `0x244c10`. The early-out fraction feeds back
into the walk after each terminal.

1. At `0x244c21`, the mark index is the **signed low 16 bits** of the
   terminal key. Compare the word at `0x4e7c20 + 4 * index` with the epoch
   at `0x4e7c1c`. A duplicate returns immediately. Otherwise store the
   epoch *before* testing the placement. Within a query, a later terminal
   sees all earlier marks, including marks for misses.
2. `0x1691a0` uses the **full 32-bit key** for placement addressing
   (stride 0x58) and the result's instance field. The definition index
   is the signed word at placement +0x34 (definition stride 0xc8).
   A native implementation must validate both indices before using them.
   `0x1689b0` consults definition data and, under query flag 0x40000,
   can also consult the active structure's table at +0xc4/+0xc8. Next
   come the sphere test at `0x11e5e0`, the inverse at `0x141590`, the
   start and delta transforms and the live breakable mask at `0x183fc0`.
3. Entry C tests the definition's BSP (+0x70) with breakable count 8
   and the limit read again from result +4. A miss leaves the result
   alone. On a hit, `0x1693d4` sets type 3, stores the mapped material
   as a word at +0x24, sets +0x38 and +0x40 to −1 and writes the key at
   +0x3c. `0x168b40` copies the fraction to +4, transforms the hit plane
   into +0x28..+0x37 and copies R's +8..+0x17 to +0x4c..+0x5b.
   Other result bytes, including padding and the hit position, retain
   their entry values. The caller computes the hit position.
4. A hit sets the collector's hit byte. Under flag 0x10000000 the new
   collector limit is positive zero. Otherwise the new fraction replaces
   the old limit only on a strict ordered decrease (`0x244c72`).

**Transient writes.** The shape constructor and destructor temporarily
change the exception-handler link at `fs:[0]`, the stack-local child
reference count, and the dword at code object +0x24. At `0x3009c0` a
decrement to zero calls the guest allocator. Eliding the increment and
decrement requires a nonzero entry count and isolation of these writes
from result, marks, inputs and callbacks. An unsupported alias or lifetime
must decline before any mark or result write.

**Caller and frame.** A relative-call and jump scan of the executable
sections finds one direct call, at `0x169c71` in `0x1697c0`. Its return
at `0x169c76` immediately tests `al`. The caller resets `xmm3` on both
the first-hit exit and the continue path. It addresses its own locals
above the consumed arguments. E preserves the callee-saved integer
registers. The taken path can write the other integer registers, arithmetic
flags, XMM0–7 and x87 status. Some volatile values can survive the caller's
return. This call site alone does not establish their liveness outside J.

If S is entry ESP and A is S−4 rounded down to 16 bytes, E's local base
after saving EBX is A−0x110. On the supported callback path the first
`0x3009e0` enters at A−0x230. Each nested ray call enters 0x1c0 bytes
below its parent. With d active ray frames (d ≥ 1), entry C in the
terminal callback enters at A−0x710−0x1c0·d. C's dead-window bound applies
below that address. A successful native E therefore needs a depth bound
for its actual traversal and an alias check spanning both walks. E's
three argument words are read-only. The status-save sites are catalogued below. The precision controls reached in a running match require an entry histogram.

### Records

R, the result record (`layout::record`): `+0` fraction, `+4` the hit
plane's address, `+8` leaf, `+0xc` surface, `+0x10` the surface's plane
word sign-extended, `+0x14` flags `u8`, `+0x15` breakable `u8`, `+0x16`
material `i16`, `+0x18` leaves listed, `+0x1c` up to 256 leaf indices,
`+0x418` the last leaf once the list is full. 0x41c bytes.

The context (`layout::context`, 0x28 bytes): `+0` query bits, `+4` BSP,
`+8` breakable count `u16`, `+0xc` mask, `+0x10` start, `+0x14` delta,
`+0x18` R, `+0x1c` previous leaf, `+0x20` previous leaf state `u8`, `+0x24`
the last split's plane.

Entry C sets R's fraction to `max(limit, 0)` and the count to 0. It builds
a context in its own frame and walks from node 0 over `[0, t1]`. `t1` is 0
for a negative limit, the guest's one (`0x45dbc0`) for a limit above it,
and otherwise the limit. Entry D walks from the given node with the context
it is passed.

## Order of operations

The kernel follows retail's order in every float operation and every
compare:

- Point side: `((nz*pz + ny*py) + nx*px) - d`. A result of `>= 0` takes the
  front child.
- Split: the node's side at `t0` and at `t1`. The near child is walked
  first. The far child is walked only if the split fraction
  `-(plane_distance_at_start / dot(delta, normal))` is still below R's fraction. The context's
  plane word is set to the node's plane (sign-extended) before the far side.
- Leaf states: solid 3, leaf flag bit 0 gives 2, else 1. Query bit 0 with
  a previous state of 1 or 2 entering solid matches against the previous
  leaf. Bit 1 with a previous state of 3 entering 1 or 2 matches against
  this leaf. Without bit 2, a previous state of 2 entering 2 matches two-sided
  (against the previous leaf when bit 0 is set). Each non-solid leaf that completes without a hit is
  listed in R. After 256 entries, R's `+0x418` keeps the last one.
- Matcher: the leaf's references in order. A reference's plane must be the
  split plane, and its 2D tree is walked to a surface. Surfaces with flag 2
  under query bit 3, or with flag 8 under bit 4, are skipped. A one-sided
  match takes the first surface. A two-sided match needs the ring test, and
  the kernel bails if the surface is `-1`.
- Ring test: the projection row is `2*axis + ((n[axis] > zero) != flip)`.
  `axis` is z if `|nz| >= |ny|` and `|nz| >= |nx|`, then y if
  `|ny| >= |nx|`, else x. The `u`, `v` axes come from the guest's table at
  `0x440b94`, read at decode time. The breakable check is inside it:
  `mask != 0`, surface flag 8, and `(breakable as i16) < (count as i16)`,
  with bit `mask[b >> 5] & (1 << (b & 31))`. Edges are visited in retail
  order, scalar. The 8-lane form is not built yet.

## Floats

`sim_core::det::Fp` is exact-or-decline binary32. Every operand must be
zero or have a magnitude in `[2^-63, 2^63)`. Anything else (NaN, infinity,
subnormal, huge) bails with `float`. These bounds keep each add, subtract and multiply within binary32 range. Each operation rounds once under nearest-even on both host architectures. The entry checks host rounding and exception masks.

The matcher loads normal components, takes absolute values and compares them in x87. Those steps are exact on the accepted range. The split's divide, sign change and binary32 store can round.
`det::x87_neg_quotient` computes it as a host f32 division. That gives the
same bits in all three precision controls: the quotient is rounded once to
24, 53 or 64 bits and then to 24, and since 53 >= 2*24 + 2 the double
rounding equals a single rounding. PE is set when the f32 quotient times the
divisor, computed exactly in f64, differs from the dividend. The adapter
then ORs bit 5 into the x87 status word. The `sim-core` test
`negated_quotient_matches_the_x87_in_every_precision` checks this against
the `x87` crate for controls `0x007f`, `0x027f` and `0x037f`.

The bridge first checks the MXCSR and x87 exception masks, the x87 error summary and the reserved precision setting for every native entry. The arithmetic has further requirements.

- MXCSR has every exception masked, rounds to nearest, and already has PE
  set. PE is sticky, so the kernel's SSE work cannot leave it different
  from retail's. FTZ and DAZ do not matter because no operand is subnormal.
- For the C and D candidate adapters: the x87 control word masks every exception,
  rounds to nearest and does not use the reserved precision setting; the
  status word has no error summary; and at least two registers below the
  top of the stack are free in the tag word.

`det::checked` supplies scalar operations that execute inside a scoped
host status check. It restores the incoming status on return and unwind.
Invalid, divide-by-zero, overflow and underflow decline the result. The
operand guard accepts normals and signed zeros. The result guard also
declines the smallest normal to cover differences in tininess detection.
`det::Chain` keeps a single- or double-precision x87 working value
through arithmetic, comparison and conversion. Reserved and extended
precision decline general chains. A–D use the narrower range proof above.

`collision.stats` reports the complete MXCSR and x87 control words
observed at A–E, including calls declined at dispatch. Differences between
two snapshots give counts for the intervening level. An active native B also counts the A entry it executes internally. Waterworks and
campaign distributions require the coordinator's runs.

## Clobbers and dead stack

A and B preserve or reproduce **every guest register and stack byte**.
A writes ECX (the selected node cursor), EAX (the leaf), XMM0–2
(zero, signed distance, x product) and the final logical flags. Its
pushes save ESI at entry ESP−4 and EDI at ESP−8. B saves EDI at ESP−4.
When it calls A it also leaves the return PC `0x11befb` at ESP−8, ESI at
ESP−12 and EDI with its low word replaced by the BSP index at ESP−16.
The no-structure, solid and leaf exits each reproduce the final flags.
On a leaf, B returns with EAX holding the sign-extended cluster index,
ECX the structure and EDX the leaf table.

C and D return the hit in AL. The candidate adapters reproduce the
callee-saved registers, return address, MXCSR, live x87 registers, tags,
TOP and x87 exception flags, including PE on an inexact divide or store.
Their last flag-producing instruction adds 0x28 (C) or 8 (D) to ESP.
The differential excludes ECX, EDX, the high 24 bits of EAX, XMM0–7,
x87 C0–C3, x87 bookkeeping pointers/opcode and the contents of empty x87
slots. The frame exclusions are bounded as follows.

| Entry | Below entry ESP | Argument slots used as scratch |
|---|---|---|
| C | 0x3c to D plus D's bound | first argument at ESP+4 |
| D | 0x28 per nested near-child call, plus 0x90 at the deepest matcher and ring test | within ESP+4 through ESP+0x13 |

A handled D call has at most 255 nested calls beneath its initial frame.
The deepest leaf has 0x30 bytes through entry to the matcher, another
0x48 through entry to the ring test and 0x18 in that test. The four-byte
return slot remains in the comparison for both entries.

`preflight::Outputs` checks the writable output ranges and the span used
by the lifted stack, and gathers the return address before commit. It
compares physical backing ranges so virtual aliases cannot hide overlap
with the BSP, point, segment, lookup tables or the mask words actually
read. On an alias decline the world invalidates cached BSPs before the
original can write into their sources.

### Call-site scan

Direct calls in the executable text, identified by instruction address:

| Entry | Direct call sites (hex) |
|---|---|
| A | 2b734, 86d6d, 86d81, 86de8, c093c, 11bef6, 11bf9e, 11bfcc, 125a2b, 168dfa, 168f5a, 169eb5, 169ede, 169f46, 16a01c, 16cf12, 16cf7a, 16f317, 173f78, 178a85, 17b9f8, 17c31c, 17d3b3, 1a5396, 1af6db, 1e16b4, 1eff02, 21de58, 261835, 26d901, 26da7b, 272b47 |
| B | 2c579, b863d, b864f, bf540, bf583, c309a, c30cd, fce26, fceca, fd12b, 1167b2, 11c72a, 1259b7, 127805, 132c74, 138a76, 14f165, 1694f7, 1697b0, 169f2c, 16a000, 16ee56, 16f052, 16f148, 171752, 171859, 17273c, 17c455, 189ca9, 18c54d, 18cca5, 18cce8, 1d5007, 1e15e7, 2022bd, 2115b4, 21a166, 21ce0c, 228651, 24704b, 24707b, 2a04a1, 2a070c, 2a081d, 2a08ff, 2c0f20 |
| C | 1693c0, 16987c, 1f52a7, 1f5358, 221ed9, 221f3d |
| D | 1de6b8, 1de7d8, 1f01e4 |
| E | 169c71 |

All six direct C callers test AL immediately. The outer vector wrapper
at `0x1de6b8` forwards D's AL and restores its frame. The recursive
caller at `0x1de7d8` and the model trace at `0x1f01e4` test AL.
An immediate test of AL leaves the rest of the volatile state live.
For example, a C miss can forward ECX, EDX, upper EAX and XMM values
through `0x16942b` into the callback that returns at `0x244c9b`.
The audio path can forward XMM1–7 through `0x222146` and `0x221d99`.
D's model trace can forward EDX and XMM registers through `0x1f0228`.
A whitelist of these direct return addresses would still admit those
paths. The collision installer keeps C/D lifted until all surviving
state is replayed or proved dead through their outer callers. A/B
have no caller-liveness prerequisite.

### Status-instruction census

The census counts instruction starts claimed by the static analyzer
(`h2 fpstats --root 0x12190`). A linear decode also interprets embedded
data and unclaimed section bytes as instructions. For example, the apparent
x87 saves at `0x397565` and `0x39a8b0` in WMADEC have no instruction claim
in `h2 analyze --explain`.

All 23 `stmxcsr` sites save to a stack slot and reload a modified word.
Static disassembly gives three uses of the saved value:

| Effect on reload | Store instruction addresses |
|---|---|
| Mask all six exceptions | `0x535a2`, `0x53f60`, `0x182292`, `0x18294b`, `0x183983`, `0x1c2cc4`, `0x1c3dbd`, `0x1c5337`, `0x1d4926`, `0x1d5145` |
| Clear all six sticky exception flags | `0x535db`, `0x53f81`, `0x1822b7`, `0x182976`, `0x1838d4`, `0x183bce`, `0x1c2d0a`, `0x1c3de2`, `0x1c5374`, `0x1d50da`, `0x1d541c` |
| Set flush-to-zero, preserving the remaining bits | `0x339acf`, `0x339e9f` |

The saved bits reach guest memory at each store. Preserving MXCSR at A–D
keeps those writes exact. The FTZ sites establish that code can enable
flush-to-zero. Only a run can say which MXCSR reaches a collision query.

The analyzer claims two `fnsave` and no `fnstenv` or `fxsave`. Both CRT
paths allocate a local record with an eight-byte result followed by a
108-byte save area. At `0x372e7c` and `0x373825` the preceding x87 stores
supply the last-instruction and operand addresses. The helper at `0x3738a8`
uses its two floating-point arguments and writes its result to the first
eight bytes. Its caller restores the save area at `0x372e88` or `0x373830`,
then loads the result. Neither path decodes the saved condition codes.
The saved words still exist in the local stack image.

Exception flags also have direct consumers. The CRT routine at `0x3212c5`
reads the x87 status, clears exceptions and maps bits 0–5 through `0x32128c`.
The status reader called at `0x3244ef` supplies the low exception bits;
the clear routine called at `0x3245f7` has its return value discarded.
The reads at `0x328040`, `0x328099` and `0x3734d7` test PE. Entries C and D
therefore reproduce PE even with all exceptions masked. This census covers
the claimed save instructions. It does not prove condition-code liveness
through every indirect caller or exception path.

## Bails and stats

| Reason | When |
|---|---|
| `env` | MXCSR or the x87 environment is outside the checks above |
| `float` | an arithmetic operand outside `Fp`'s range |
| `fault` | an unreadable input, unwritable output or unusable stack range |
| `data` | invalid block sizes or indices, a ring that cannot be decoded, a walk exceeding the available node count, or an epoch change during decode |
| `depth` | the walk went deeper than 256 levels |
| `alias` | overlap, or a physically contiguous span cannot be established |
| `trap` | the debugger observes guest memory accesses and needs the lifted instruction sequence |
| `budget` | the essential decoded BSP and its index entry exceed the remaining map budget, or the index is full |

`CollisionWorld` decodes each BSP on first use and keys it by address. It
compares the 16 header words on every lookup. The runtime adapter watches
the physical pages backing those words, the eight blocks, ZERO and the
projection rows. Writes through a virtual alias, a changed layout or a
replacement RAM image discard the previous generation. Expanding the
source index beyond 1,024 distinct BSPs starts a fresh subscription. The cache also drops when
`EPOCH` advances (map load, structure switch, snapshot load). The index
remembers invalid headers and BSPs too large for an empty world. A change
in the header permits a fresh decode. The fixed-capacity charges are 96
bytes per allocated address record and 512 per retained BSP owner. Both
cover the host layout, including padding and Arc bookkeeping. The index
grows geometrically to at most 8,192 named BSP addresses per generation.

`Stats.bytes` adds these charges to the allocated array capacities and
the packed cluster table. The 8 MiB limit covers live map allocations
owned by the cache. Decode scratch, completed generations held by callers,
process-wide counters and runtime write-watch infrastructure are separate.
The first overflow drops expanded ring arrays across the active world.
Borrowers keep the same essential BSP arrays. The scalar matcher reads
edges and vertices in guest order and validates bounded closure. A
structure table that cannot fit uses the live cluster word. A BSP whose
essential storage cannot fit declines before writing guest state.
Admission runs again after concurrent decodes finish. The agent API serves
the byte totals, fallback counts and FP distributions through
[`collision.stats`](../runtime/agent-api.md#collision-the-native-collision-world).

## Tests

```sh
cargo test -p sim-core
cargo test --release -p sim-core det::
cargo test -p sim-collision
cargo test -p engine --features runtime-glue --lib sim_collision::
cargo test -p sim-bridge sim_native
cargo test -p agent collision
H2_XBE=/path/to/default.xbe cargo test -p engine --features differential --test it sim_collision_differential::
```

The synthetic differential runs each entry against the lifted instructions
on the IR interpreter, with a cold and a warm world and the earlier
per-function port where one exists. It verifies that a declined call
preserves the entire CPU and memory. Cases exercise plane ties, unusual
float inputs, all precision controls and a partially occupied x87 stack.

The ignored disc sweep compares points on each structure and object BSP
and locations on structure BSPs. It reports per-map decoded storage and
bails and compares the largest active structure plus all local object BSPs against the map budget. Give the test the owner's disc and XBE. No disc payload is emitted.

```sh
H2_DISC=/path/to/halo2.iso H2_XBE=/path/to/default.xbe cargo test --release -p engine --features differential --test it sim_collision_differential::disc::every_disc_bsp_point_matches_the_guest -- --exact --ignored --nocapture
```

## Gaps

The segment and MOPP disc sweeps, debug round-robin source checks and
packed ring test are later slices. Runtime verification includes arena
parity, cache-cold runs, FP distributions and cross-architecture bail
counts.
