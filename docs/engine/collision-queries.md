# Collision queries

The native collision world is always installed (`sim_native::install_collision`)
and answers every call of entries A to J in Rust. No entry runs the lifted
function to compare at its return. A call no kernel can answer panics with
`collision: <entry>: <cause>` (`env` names the MXCSR and the x87 words);
only a debugger trap runs the original.

| Entry | Exits |
|---|---|
| C, D | straight walk and [crossing exits](#crossing-exits) replay their frame; the matcher path commits R, D's context, AL and PE |
| E | an empty cluster replays its frame; a populated one commits the marks, the hit and AL |
| G | the flood with no portal test replays its frame; a contested flood commits the list, the marks and both PE flags |
| I | the walk runs in Rust; each terminal's callbacks are guest calls |
| J | commits the result, the flags argument and the mark state |

The adapters live in
`crates/rewrites/engine/src/sim_collision`, the kernels in
`crates/rewrites/sim-collision` and the installer in
`crates/rewrites/sim-bridge/src/sim_native.rs`.
[Switch table](../runtime/switches.md) · [Phase 2 design](sim-rewrite-collision.md)

An answering adapter gathers the inputs and checks the writable ranges
before committing. A decline writes nothing before the panic.

## Entries

| | Entry | Point | ABI | Writes |
|---|---|---|---|---|
| A | `0x14a280` point to leaf | `COLLISION_POINT_TO_LEAF` | `ecx` BSP, `eax` node, `edx` point; leaf in `eax`, `ret` | leaf, scratch registers and two saved words (below) |
| B | `0x11bed0` location | `COLLISION_LOCATION` | `edx` point, `esi` the 8-byte location; `ret` | `[esi]` leaf `u32`, `+4` cluster `i16`, `+6` BSP index `i16` |
| C | `0x1de630` vector test | `COLLISION_VECTOR_TEST` | `eax` R, `ecx` query bits, `edx` BSP, `xmm0` limit; stack: breakable count (`u16`), breakable mask, start, delta; `ret 0x10`; hit in `al` | R, `al`, x87 PE |
| D | `0x1de6d0` vector walk | `COLLISION_BSP_SEGMENT` | stack: context, node, `t0`, `t1`; `ret 0x10`; hit in `al` | R, the context's `+0x1c`, `+0x20`, `+0x24`, `al`, x87 PE |
| E | `0x244980` instance trace | `COLLISION_INSTANCE_TRACE` | `eax` cluster, `esi` start, `edi` delta, three stack arguments, `ret 0xc` | `al`, terminal marks and hit-result fields described below |
| F | `0x300150` MOPP box query | `COLLISION_MOPP_BOX` | `ecx` box VM; stack: code, AABB, key array; `ret 0xc` | the VM's `+0`, `+0x10..+0x28`, `+0x30`; keys at `data[size..]` and `size` (below) |
| G | `0x14a5b0` cluster flood | `COLLISION_CLUSTER_FLOOD` | stack: cluster (`i16`), centre, radius, capacity, out; `ret 0x14`; count in `ax` | the `u16` cluster list, the cluster marks |
| H | `0xbb050` object spatial visit | `OBJECT_SPATIAL_VISIT` | stack: list bits, type mask, location, centre, radius, out, capacity; `ret 0x1c`; count in `ax` | the handle list, the epochs, active bytes and marks described below |
| I | `0x301800` MOPP ray | `COLLISION_MOPP_RAY` | `ecx` ray VM; stack: collection, code, ray input, parent body, collector; `ret 0x14` | the VM; the callbacks are guest calls and write what they write |
| J | `0x1697c0` whole query | `SIM_COLLISION_QUERY` | stack: flags, start, delta, ignore A, ignore B, result; `ret 0x18`; hit in `al` | result, flags argument, generations, active bytes and visit marks |

The per-function point, sphere, BSP segment, whole query, ray, visit and
portal ports serve as differential oracles. The subsystem owns runtime
dispatch. The hooks that update world caches, subscriptions and
counters run on the owning executor.

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

### Entry E - instance trace of one cluster

In [collision-queries-entries.md](collision-queries-entries.md).

### Entry F - MOPP box query

In [collision-queries-entries.md](collision-queries-entries.md).

### Entry I - general MOPP ray

In [collision-queries-entries.md](collision-queries-entries.md).

### Entries G and H - cluster flood and object visit

In [collision-queries-entries.md](collision-queries-entries.md).

### Entry J - whole segment query

In [collision-queries-entries.md](collision-queries-entries.md).

### Object-reference sphere pretest

In [collision-queries-entries.md](collision-queries-entries.md).

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
  with bit `mask[b >> 5] & (1 << (b & 31))`. Short cached rings use
  four-edge groups with precomputed deltas and the reference's projection
  row. A packed range failure replays the scalar test in guest order to
  preserve early rejection. Long or uncached rings use the scalar test.

## Floats

A, B, C, D and the sphere pretest run their SSE arithmetic and compares
in `sim_core::det::Fp::guest(mxcsr)`, for any operand. An operation whose
operands are zero or of magnitude in `[2^-63, 2^63)` takes the host
result, which rounds once under nearest-even on both host architectures;
every other one (NaN, infinity, subnormal, huge) runs on the `simd`
crate's SSE model, which rounds and raises flags as the MXCSR says. Each
`comiss` goes through `Fp::comiss`: its operands raise IE or DE as the
instruction does, and an unordered result branches as ZF, PF and CF all
set do (`jb` and `jbe` taken, `ja` and `jae` not; `bsp::non_negative`,
`bsp::above`, the sphere's tests). The `sim-collision` tests
`the_walks_run_operands_off_the_fast_path_under_the_guest_state` and
`guest_state_runs_exceptional_inputs_as_the_compares_branch` cover a
plane component of `1e-30` and NaN inputs. `Fp::new` keeps the
exact-or-decline rule (`float`) for E and J.

The matcher's projection row is decided once per BSP: it takes absolute
values and compares them in x87, which raises no flag on zero, normal or
infinite components. A NaN or subnormal component makes the row `BAD`
(`data`). A packed ring test stands only when it raised no flag the walk
had not; otherwise the scalar test runs in guest order.

The split's divide, sign change and binary32 store can round.
`det::x87_neg_quotient_under(control, a, b)` takes operands off the
fast path to the `x87` crate's model under the guest's control word and
returns the stored bits and the flags raised; the adapter ORs them into
the x87 status word (`Outcome::x87_flags`). On the fast path
`det::x87_neg_quotient` computes it as a host f32 division. That gives the
same bits in all three precision controls: the quotient is rounded once to
24, 53 or 64 bits and then to 24, and since 53 >= 2*24 + 2 the double
rounding equals a single rounding. PE is set when the f32 quotient times the
divisor, computed exactly in f64, differs from the dividend. The adapter
then ORs bit 5 into the x87 status word. The `sim-core` test
`negated_quotient_matches_the_x87_in_every_precision` checks this against
the `x87` crate for controls `0x007f`, `0x027f` and `0x037f`.

The bridge first checks the MXCSR and x87 exception masks, the x87 error summary and the reserved precision setting for every native entry. The arithmetic has further requirements.

- MXCSR has every exception masked and rounds to nearest
  (`det::guest_env`); PE may be clear, as in the live game's `0x1f80`.
  The kernels' SSE arithmetic runs in `det::Fp::guest(mxcsr)` (A, B, C,
  D and the sphere pretest; FTZ and DAZ act as the MXCSR says) or a
  checked region that samples the host's inexact flag for its SSE
  operations (`Checked::sse_inexact`, G and H, which need FTZ and DAZ off,
  `det::checked_guest_env`). An answer leaves the MXCSR with the flags
  retail's instructions raise.
- For F and G: the x87 control word masks every exception, rounds to
  nearest and selects single or double precision (`det::Chain`; extended
  bails `float`), and one (F) or two (G) registers below the top are
  free. PE may be clear. A chain's arithmetic, `fst m32` and `fistp`
  sample the host's inexact flag (`Checked::x87_inexact`), and the
  commit sets the status word's PE when one rounded, as retail does. The
  adapters model nothing else of the status word.
- E, I and J leave both PE flags as they found them: their walks'
  inexactness is not tracked.
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
precision decline general chains.
F runs its six conversions as chains inside one checked region: an
operand that is not zero or normal, a non-normal result, or a value
`fistp` cannot represent bails `float`.

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
| F | 4 for EBP, up to 15 of alignment, 0xa4 to the root interpreter's saved registers, 0x54 per self-call | none |
| G | 0x14, or 0x460 + 0x2c·k when a portal test ran at depth k | within ESP+4 through ESP+0x17 |
| H | 0x448, plus G's figure when it floods | within ESP+4 through ESP+0x1f |

A handled D call has at most 255 nested calls beneath its initial frame.
The deepest leaf has 0x30 bytes through entry to the matcher, another
0x48 through entry to the ring test and 0x18 in that test. The four-byte
return slot remains in the comparison for both entries.

F preserves EBX, EBP, ESI and EDI and returns with ESP 0x10 above its
entry value. Its epilogue (`pop esi; mov esp, ebp; pop ebp; ret 0xc`)
changes no flags, so EFLAGS are those of the root interpreter's
`add esp, 0x38` at `A − 0x94`, where A is the entry ESP − 4 rounded down
to 16. The adapter replays them. EAX, ECX, EDX and the x87 condition
codes hold the interpreter's last scratch values and are not replayed.
A native query nests at most 256 self-calls (`depth` beyond that). The adapter
checks the window for the nesting the query actually reaches.

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
| F | 2df659 |
| G | bb0e4, 132dbc, 14a68b (itself), 17e4e9, 180bb8, 1cace3 |
| H | d6d09, 1095c2, 1155ff, 119cf3, 11c757, 150954, 1511b7, 1ac697, 2c0f4e |

All six direct C callers test AL immediately. The outer vector wrapper
at `0x1de6b8` forwards D's AL and restores its frame. The recursive
caller at `0x1de7d8` and the model trace at `0x1f01e4` test AL.
An immediate test of AL leaves the rest of the volatile state live.
For example, a C miss can forward ECX, EDX, upper EAX and XMM values
through `0x16942b` into the callback that returns at `0x244c9b`.
The audio path can forward XMM1–7 through `0x222146` and `0x221d99`.
D's model trace can forward EDX and XMM registers through `0x1f0228`.
A whitelist of these direct return addresses would still admit those
paths.

| Path | What the answer leaves |
|---|---|
| C or D unsplit walk, no surface lookup | every register and stack write replayed; a direct D leaf keeps the incoming XMM values |
| C or D crossing planes, no surface lookup | the [crossing exit](#crossing-exits) |
| C or D reaching the matcher | AL, R, D's context, PE and the epilogue's flags (`engine::sim_collision::commit`); ECX, EDX and XMM0–7 keep their entry values, not the matcher's scratch the callers above could forward |
| G contested, E populated | their outputs the same way, without the scratch state |

A/B have no caller-liveness prerequisite.

F's direct caller `0x2df640` returns at once through `mov esp, ebp; pop
ebp; ret 8`, which reads none of F's scratch state. It is a virtual
method: the shape vtable slots at `0x455ee8` and `0x455f88` hold it, and
the structure shape's override `0x1eea50` (slot at `0x456190`) calls it
directly at `0x1eea6d` and next reads only EDI, which F preserves. Callers
through the vtable follow the thiscall convention of a method with no
result, under which EAX, ECX, EDX, EFLAGS and the x87 condition codes do
not survive the call. F is installed on that basis.

### Crossing exits

A crossing exit is a C or D walk that splits at one or more planes and
reaches every leaf without the reference matcher. Every such walk misses.
The kernel records the guest's steps in order
(`sim_collision::bsp::segment::Step`). The adapter
(`crates/rewrites/engine/src/sim_collision/crossing.rs`) replays them over
the instruction sequence of `0x1de630` and `0x1de6d0` and rebuilds:

| State | Source in the replay |
|---|---|
| EAX, ECX, EDX | the last plane test, crossing, far step or leaf tail |
| XMM0–3 | the last plane test's four values, or the split a frame stopped on |
| EBX, EBP, ESI, EDI, XMM4–7, MXCSR | entry values (saved and restored, or untouched) |
| EFLAGS | the outer `add esp, 8` (D) or `add esp, 0x28` (C) |
| x87 | PE on an inexact split; TOP and tags unchanged |
| Stack | every frame's saved registers, locals, argument slots and return addresses, the wrapper's context, in guest order |

The exit answers every crossing call. The differential compares it with
the lifted entry over AL, R, D's context, all general registers and ESP,
EFLAGS on the defined bits, MXCSR, XMM0–7, the x87 control word, status
outside C0–C3, tags and occupied registers, and the stack from 0x40 below
the deepest frame write to the argument slots.

Contents of empty x87 registers and the condition codes are not replayed,
as for the other C and D exits. D under the wrapper (return `0x1de6bd`)
or its own recursion (`0x1de7dd`) takes the matcher path's commit, since
the frame it would replay is its caller's. `collision.stats` counts the
crossing exits under `vector.answered` and `context.answered`.

### Status-instruction census

The census counts instruction starts claimed by the static analyser
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

The analyser claims two `fnsave` and no `fnstenv` or `fxsave`. Both CRT
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

A bail in any entry panics `collision: <entry>: <reason>` (`sim-bridge`
`collision_unhandled`; an `env` names the MXCSR and the x87 words); no
lifted body runs the call. Only `trap` runs the original, for the
debugger. A bail in I after a callback has run panics the same way: the
walk cannot be handed back to the lifted interpreter mid-query. Each
bail is also the thread's last (`sim_collision::take_bail`), which the
hook reads for the panic.

| Reason | When |
|---|---|
| `env` | MXCSR or the x87 environment is outside the checks above |
| `float` | E or J: an arithmetic operand outside `Fp::new`'s range; F–I: an operand or result a checked region declines |
| `fault` | an unreadable input, unwritable output or unusable stack range |
| `data` | invalid block sizes or indices, a ring that cannot be decoded, a walk exceeding the available node count, or an epoch change during decode; for F, a bad program size, a reached trap, a `guest_only` program, or an inverted or far box |
| `depth` | the walk went deeper than 256 levels, or F's query nests more than 256 self-calls |
| `alias` | overlap, or a physically contiguous span cannot be established |
| `trap` | the debugger observes guest memory accesses and needs the lifted instruction sequence |
| `budget` | the essential decoded BSP and its index entry exceed the remaining map budget, or the index is full; for F, the decoded tree does not fit or 4,096 programs are held |
| `grow` | F's answer has more keys than the array has room for: the original grows it |

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

`Stats.bytes` adds these charges to the allocated array capacities, the
packed cluster table and the MOPP trees (nodes and the kept bytecode). The 8 MiB limit covers live map allocations
owned by the cache. Decode scratch, completed generations held by callers,
process-wide counters and runtime write-watch infrastructure are separate.
The first overflow drops expanded ring arrays across the active world.
Borrowers keep the same essential BSP arrays. The scalar matcher reads
edges and vertices in guest order and validates bounded closure. A
structure table that cannot fit uses the live cluster word. A BSP whose
essential storage cannot fit declines before writing guest state.
Admission runs again after concurrent decodes finish. The agent API serves
the byte totals, fallback counts and FP distributions through
[`collision.stats`](../runtime/agent-api-checks.md#collision-the-native-collision-world).

## Tests

```sh
cargo test -p sim-core
cargo test --release -p sim-core det::
cargo test -p sim-collision
cargo test -p engine --features runtime-glue --lib sim_collision::
cargo test -p sim-bridge sim_native
cargo test -p agent collision
cargo test -p guest-layout
H2_XBE=/path/to/default.xbe cargo test -p engine --features differential --test it sim_collision_differential::
H2_XBE=/path/to/default.xbe cargo test -p engine --features differential --test it sim_collision_differential::mopp_ray -- --nocapture
```

The G and H cases (`cluster_flood_matches_the_guest`,
`cluster_flood_ties_match_the_guest`, `object_visit_matches_the_guest`)
run the `sim_physics` spatial ports as a third arm, with the lifted entry
calling the port on a copy of the guest. Half the random floods and the
single-precision tie floods enter with the x87 PE clear, and the status
word's PE must match the guest's. `spatial::contested_spatial_calls_match_the_lifted_guest_from_pe_clear`
runs the installed dispatch on a contested flood and a flooding visit
from the live game's MXCSR `0x1f80` and a clear status word.

The synthetic differential runs each entry against the lifted instructions
on the IR interpreter, with a cold and a warm world and the earlier
per-function port where one exists. It verifies that a declined call
preserves the entire CPU and memory. Cases exercise plane ties, unusual
float inputs, all precision controls and a partially occupied x87 stack.
Every C and D case with a crossing exit also compares that exit with the
guest's return. `crossing::crossing_exits_match_every_register_and_stack_byte`
writes the exit ungated on plane chains up to six deep, both ways through
(far-child loops and nested near calls), with stopped frames, full leaf
lists and solid ends, and compares every register and stack byte.

F's cases are random programs over every opcode class
(`sim_collision::synth::random_mopp`) and `mopp::build` programs, boxes
on frame-cell edges, and each decline path: extended or directed
rounding, NaN, inverted and far boxes, a full array, and a key area over
the VM, the code or the dead stack. Half enter with the x87 PE clear;
`mopp_box::tests::pe_clear_is_admitted_and_set_only_by_a_rounding_conversion`
checks that the commit sets it exactly when a conversion rounds.

I's cases (`mopp_ray::check_cast`) run the setup and the interpreter
lifted with invented callbacks answered by the run loop: the instanced
collection method or another, a child ray method that lowers the
collector's fraction for some keys, and a filter that rejects some. The
same callbacks then answer `CollectorCast`'s guest calls on a copy of
the fixture. Every callback's target, registers, arguments and temporary
body, the keys the collection got and every byte above the stack must
match. Programs are random and hand-written over every opcode family,
with rays on and across their thresholds, in single and double
precision, and some callbacks change the VM's inputs.

E's populated cases (`populated_instance_cluster_against_the_lifted_callbacks`)
run `instance::trace` beside the lifted entry and compare AL, the
callee-saved registers, the result, every instance mark slot, the code's
reference count and `fs:[0]`. J's cases
(`structure_liquid_and_reference_order_match_the_lifted_query`) run
`query::query` and compare AL, the callee-saved registers and every byte
the guest left outside its frames, the flags argument included.

The ignored BSP sweeps compare points and segments on structure and object
BSPs, and locations on structure BSPs. The segment sweep compares the C
record, samples D’s full context, and checks all registers and frame bytes
on a straight native exit and on each crossing exit. It reports per-map decoded storage and
bails and compares the largest active structure plus all local object BSPs against the map budget. Give the test the owner's disc and XBE. No disc payload is emitted.

```sh
H2_DISC=/path/to/halo2.iso H2_XBE=/path/to/default.xbe cargo test --release -p engine --features differential --test it sim_collision_differential::disc::every_disc_bsp_point_matches_the_guest -- --exact --ignored --nocapture
```

The MOPP sweep covers structure, cluster, `coll` and `phmo` programs. It compares 400 boxes per
program with `mopp::boxvm` (the mopp.md sampling) and runs one box in ten
through the adapter and the lifted entry. It prints each program's
bytecode and node counts and the bails by reason.

```sh
H2_DISC=/path/to/halo2.iso H2_XBE=/path/to/default.xbe cargo test --release -p engine --features differential --test it sim_collision_differential::mopp_box::every_disc_mopp_box_matches_the_guest -- --exact --ignored --nocapture
```

The owner-disc ray sweep compares 400 rays per cluster program. The
placement sweep checks each instance’s inverse against the lifted helper.
Run the ignored collision suite with the owner’s ISO and XBE.

```sh
cargo test --release -p engine --features differential --test it sim_collision_differential:: -- --ignored --nocapture
```

## Gaps

The matcher path, the contested flood, the populated cluster and J
return with the callers' volatile registers at their entry values, not
the lifted scratch some callers forward ([Call-site scan](#call-site-scan)).
E, I and J do not track their walks' PE. A crash names either.
