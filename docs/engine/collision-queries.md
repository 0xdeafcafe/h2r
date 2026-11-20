# Collision queries: entries A to D

The contract page for the collision entries taken over under
`impl.sim.native.collision` (default off). Design and slices:
[sim-rewrite-collision.md](sim-rewrite-collision.md). Code:
`crates/rewrites/sim-collision` (kernels, synthetic data only),
`crates/rewrites/engine/src/sim_collision` (adapters, guest layout),
`crates/rewrites/sim-bridge/src/sim_native.rs` (installation). The data comes from the
guest image, and no value from it is copied here or into the code.

Bar E: same bits as retail. Each kernel works out its full answer before
anything is written. If it cannot answer exactly, it returns a `Bail`, writes
nothing, and the lifted entry runs from the untouched state.

## Entries

| | Entry | Point | ABI | Writes |
|---|---|---|---|---|
| A | `0x14a280` point to leaf | `COLLISION_POINT_TO_LEAF` | `ecx` BSP, `eax` node, `edx` point; leaf in `eax`, `ret` | `eax` |
| B | `0x11bed0` location | `COLLISION_LOCATION` | `edx` point, `esi` the 8-byte location; `ret` | `[esi]` leaf `u32`, `+4` cluster `i16`, `+6` BSP index `i16` |
| C | `0x1de630` vector test | `COLLISION_VECTOR_TEST` | `eax` R, `ecx` query bits, `edx` BSP, `xmm0` limit; stack: breakable count (`u16`), breakable mask, start, delta; `ret 0x10`; hit in `al` | R, `al`, x87 PE |
| D | `0x1de6d0` vector walk | `COLLISION_BSP_SEGMENT` | stack: context, node, `t0`, `t1`; `ret 0x10`; hit in `al` | R, the context's `+0x1c`, `+0x20`, `+0x24`, `al`, x87 PE |

Entry E (the MOPP instance trace) arrives with S3 and is not on this page
yet.

The installer gives each of these points to the collision switch. The older
per-function switches (`native_point_to_leaf`, `native_bsp_segment`) are
skipped at these points while the collision switch is on. Their bodies stay
as differential oracles.

### Entry B

The guest reads the `u16` at `0x4686c4` (the active structure BSP index).
`0xffff` gives `{-1, 0xffff, 0xffff}` without a walk. Otherwise B runs
entry A from node 0 on the BSP at `[0x4e033c]` and stores the leaf at
`[esi]`. It then reads the cluster `i16` at
`[[0x4e0348] + 0x30] + 8 * leaf`, or uses `0xffff` for solid, stores it at
`+4`, and stores the index at `+6`. Because the leaf is stored before the
cluster is read, the adapter bails (`alias`) if `[esi]` overlaps any of the
three words the cluster read goes through. The leaf table is read live on
every call and is not cached.

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
  `-(dist0 / (dist1 - dist0))` is still below R's fraction. The context's
  plane word is set to the node's plane (sign-extended) before the far side.
- Leaf states: solid 3, leaf flag bit 0 gives 2, else 1. Query bit 0 with
  a previous state of 1 or 2 entering solid matches against the previous
  leaf. Bit 1 with a previous state of 3 entering 1 or 2 matches against
  this leaf. Without bit 2, a previous state of 2 entering 2 matches two-sided
  (against the previous leaf when bit 0 is set). Each visited leaf is
  listed in R. After 256 entries, R's `+0x418` keeps the last one.
- Matcher: the leaf's references in order. A reference's plane must be the
  split plane, and its 2D tree is walked to a surface. Surfaces with flag 2
  under query bit 3, or with flag 8 under bit 4, are skipped. A one-sided
  match takes the first surface. A two-sided match needs the ring test, and
  the kernel bails if the surface is `-1`.
- Ring test: the projection row is `2*axis + (n[axis] > zero) != flip`.
  `axis` is z if `|nz| >= |ny|` and `|nz| >= |nx|`, then y if
  `|ny| >= |nx|`, else x. The `u`, `v` axes come from the guest's table at
  `0x440b94`, read at decode time. The breakable check is inside it:
  `mask != 0`, surface flag 8, and `(breakable as i16) < (count as i16)`,
  with bit `mask[b >> 5] & (1 << (b & 31))`. Edges are visited in retail
  order, scalar. The 8-lane form is not built yet.

## Floats

`sim_core::det::Fp` is exact-or-decline binary32. Every operand must be
zero or have a magnitude in `[2^-63, 2^63)`. Anything else (NaN, infinity,
subnormal, huge) bails with `float`. Under that bound, sums, differences
and products of the given operands are exact in f32 on both host
architectures, so arm64 and x86-64 agree (contract D).

The only x87 operation is the split's `fdiv`, `fchs`, `fstp`.
`det::x87_neg_quotient` computes it as a host f32 division. That gives the
same bits in all three precision controls: the quotient is rounded once to
24, 53 or 64 bits and then to 24, and since 53 >= 2*24 + 2 the double
rounding equals a single rounding. PE is set when the f32 quotient times the
divisor, computed exactly in f64, differs from the dividend. The adapter
then ORs bit 5 into the x87 status word. The `sim-core` test
`negated_quotient_matches_the_x87_in_every_precision` checks this against
the `x87` crate for controls `0x007f`, `0x027f` and `0x037f`.

Environment checks (`env` bail):

- MXCSR has every exception masked, rounds to nearest, and already has PE
  set. PE is sticky, so the kernel's SSE work cannot leave it different
  from retail's. FTZ and DAZ do not matter because no operand is subnormal.
- For entries C and D only: the x87 control word masks every exception,
  rounds to nearest and does not use the reserved precision setting; the
  status word has no error summary; and at least two registers below the
  top of the stack are free in the tag word.

`Stats.precision` counts the precision control seen at C and D. The spec's
8.1 measurement over a Waterworks and a campaign run is still to be done.
Read it through `collision.stats`.

## Clobbers and dead stack

Exact: the result register, `ebx`, `esi`, `edi`, `ebp`, `esp`, the return
address, every guest byte outside the dead window, MXCSR, and the x87
control word, tag word, stack and PE.

Declared clobbered and left as they were: `ecx`, `edx`, the upper 24 bits
of `eax` for C and D, the XMM registers, EFLAGS, x87 C0 to C3, the x87
last-instruction and operand words, and the dead stack. The dead window for
C and D is `[esp - 0x3000, esp + 4 + 0x10)`. That covers 0x2c bytes per
recursion level to the 256-level depth limit, the wrapper and matcher
frames, and the argument slots the entries write into.

Aliasing (`alias` bail): R and the context must not overlap each other,
the BSP's blocks, start, delta, the guest one, or the dead window. A mask
word the walk read must not overlap an output.

### Call-site scan

There are 87 direct calls: 46 to B, 32 to A, 6 to C and 3 to D. A forward
scan from each return address, tracking registers at byte granularity,
finds no read of a clobbered register or dead-window byte before it is
written. The hits it flagged were checked by hand: `fcompi`/`lahf`
sequences that write the flags first, a `movsx` that writes its
destination, the `lea ecx, [ecx]` padding, and void epilogues. Indirect
callers were not searched for.

### Status-instruction census

A linear sweep finds 23 `stmxcsr`, 23 `ldmxcsr`, 25 `fnsave`, 20
`fnstenv`, 9 `fldenv` and 5 `fnclex`. MXCSR is kept exactly, so the
`stmxcsr` sites see what retail sees. The x87 PE flag is reproduced. The
condition-code and last-instruction words are not, and the `fnsave` and
`fnstenv` sites that could observe them sit mostly in the libraries at
`0x370000` and above. They have not been audited one by one. Run parity
(ladder step 4) is the check.

## Bails and stats

| Reason | When |
|---|---|
| `env` | MXCSR or the x87 environment is outside the checks above |
| `float` | a coordinate, plane or limit outside `Fp`'s range |
| `fault` | an unmapped guest read during the query or decode |
| `data` | the decoder refused the BSP: counts above `1 << 22`, more than 128 MiB, blocks out of range, bad child or plane indices, a ring longer than 64 edges or one that does not close |
| `depth` | the walk went deeper than 256 levels |
| `alias` | an output overlaps an input, the BSP or the dead window |

`CollisionWorld` decodes each BSP on first use and keys it by address. It
compares the 16 header words on every lookup and drops everything when
`EPOCH` advances (map load, structure switch, snapshot load). Failed
decodes are cached as `data`, except faults. `Stats` counts native calls
and bails by entry and reason, decodes, decode failures, flushes, BSPs
held, their bytes, and precision controls. The agent API serves it as
[`collision.stats`](../runtime/agent-api.md#collision-the-native-collision-world).

## Tests

```sh
cargo test -p sim-core
cargo test -p sim-collision
cargo test -p sim-bridge sim_native
cargo test -p agent collision
H2_XBE=/path/to/default.xbe cargo test -p engine --features differential --test it sim_collision_differential::
```

The differential test runs each entry three ways from the same synthetic
guest memory and CPU: the lifted entry on the IR interpreter, the adapter
(cold, then warm), and the older port where one exists (`point_to_leaf`,
`bsp::segment`). It compares every register outside the clobber set, the
x87 status bits `0x38ff`, and all memory outside the dead window. Seeds
cover ties, NaN, infinity, subnormal, out-of-range values, unsupported
control words and every precision control.

## Gaps

- The disc sweep (ladder step 3) is not written. Steps 4 to 7 are run by
  the coordinator.
- The ring test runs scalar. The 8-lane form is not built.
- The structure leaf-cluster table is read live, not cached.
- Declared clobbers are not reproduced (see above).
- The precision control has not been measured on a run.
- The debug round-robin re-hash of decoded BSPs is not built.
