# Collision queries: entry contracts

Status: Built.
Part of [collision-queries.md](collision-queries.md): the per-entry contracts for E, F, G, H, I and J and the object-reference sphere pretest.

### Entry E - instance trace of one cluster

Entry E answers both paths in Rust (`instance::trace`). The signed count
≤ 0 exit replays the saved frame, scratch registers and final compare
flags without reading any query argument. A positive count runs the
cluster's MOPP ray and the terminals below (`instance::populated`) and
writes the marks in visit order, the nearest hit's fields and `al`; the
callee-saved registers, the code's reference count and `fs:[0]` keep
their entry values. The ABI at `0x244980` is a cluster index in `eax`, a
start pointer in `esi`, a delta pointer in `edi`, and three stack words
at entry ESP +4, +8 and +12 (query flags, BSP filter bits, result pointer).
`al` returns whether any terminal hit. Both exits pop the three arguments.
The cluster record is at `[[0x4e0348] + 0xa0] + 0xb0 * cluster`. The signed
count is at cluster +0xa8.

J's planner follows the same positive path through the cluster MOPP,
live breakable words, inverse placement transform and definition BSP,
and carries terminal marks and fraction changes across clusters in
visit order.

With a positive count, E uses the code object at cluster +0xac. It sets
the ray endpoints to start and start + delta (one binary32 add per axis),
and initialises the collector's limit from result +4. The collector path
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
decrement to zero calls the guest allocator. The native path elides the
increment and decrement: it needs a nonzero entry count and these words
apart from the result, marks and inputs, and declines (`data`, `alias`)
before any mark or result write otherwise.

**Caller and frame.** A relative-call and jump scan of the executable
sections finds one direct call, at `0x169c71` in `0x1697c0`. Its return
at `0x169c76` immediately tests `al`. The caller resets `xmm3` on both
the first-hit exit and the continue path. It addresses its own locals
above the consumed arguments. E preserves the callee-saved integer
registers. The taken path can write the other integer registers, arithmetic
flags, XMM0–7 and x87 status. Some volatile values can survive the caller's
return. This call site alone does not establish their liveness outside J. The
native path leaves them at their entry values.

If S is entry ESP and A is S−4 rounded down to 16 bytes, E's local base
after saving EBX is A−0x110. On the supported callback path the first
`0x3009e0` enters at A−0x230. Each nested ray call enters 0x1c0 bytes
below its parent. With d active ray frames (d ≥ 1), entry C in the
terminal callback enters at A−0x710−0x1c0·d. C's dead-window bound applies
below that address. The native E's alias check spans that whole window
(`0x710 + 0x1c0·MAX_FRAMES` plus C's bound) and both walks' sources. E's
three argument words are read-only. The status-save sites are catalogued below. The precision controls reached in a running match require an entry histogram.

### Entry F - MOPP box query

The only direct caller is the MOPP shape's slot `+0x30` (`0x2df640`,
call at `0x2df659`), which passes its code pointer, the AABB and the
`hkArray` of keys, with the VM on its own stack. F writes the array
pointer at VM `+0`, the code pointer at `+0x30`, then for x, y, z
converts the minimum and the maximum coordinate. Each conversion is
`fld p; fsub origin; fmul scale; fstp m32; fld m32; fistp; dec`, with the
origin at code `+0x00` and the scale at `+0x0c`. The minimum corner goes
to VM `+0x20..+0x28` and the maximum to `+0x10..+0x18`; the fourth lanes
are not written. F then builds the interpreter's root state on its stack
(`H = (hi >> 16) + 1`, `L = lo >> 16`, zero offsets, key offset,
bits and property) and calls `0x2ff8b0` on the bytecode at code `+0x30`.

The kernel is `sim_collision::mopp`. `CollisionWorld::mopp` decodes a
program once, by code pointer, into a tree of splits, diagonals, bounds,
terminals and traps with the interpreter state folded into full-precision
thresholds (the table in [the design](sim-rewrite-collision.md#43-mopp-tree)).
`Tree::query_box` walks it low side first and returns the keys in the
interpreter's order with the deepest nesting of its self-calls. The
adapter (`engine::sim_collision::mopp_box`) appends the keys at
`data + 4 * size` and stores the new size when there is at least one key.
The interpreter appends one key at a time and calls the array's grow
routine `0x2d91d0` when `size == capacity & 0x7fffffff`; an answer that
does not fit the room left bails with `grow` before any write. The key
write span must also be physically disjoint from the VM, array descriptor,
return slot and guest frame.

Two conditions keep the folded thresholds exact and are checked per call.
Every full-precision corner lies in `[-2^28, 2^28)`, so no 32-bit frame
value or diagonal sum the interpreter forms can wrap. Every axis has
`lo <= hi`, because the one-plane split takes its low side whenever
`H <= S` without looking at `L`. A box outside either is `data`. A
reached opcode the interpreter faults or asserts on (outside its table, a
property slot past its state, an operand past the program's size, a
rescale past shift 0) is a trap node and bails `data`. Property 0 writes
(`60`, `64`, `68`) land in the stack state and never reach a key, so
they fold away. A program that unfolds past 16 nodes per bytecode byte is
held as `guest_only` and every query on it bails `data`.

The world keys each tree by code pointer and compares the origin, scale
and size words on every call; the reference count at `+0x24` is not part
of it. The runtime adapter watches the pages under each program with its
own subscription (`collision-mopp-sources`). The reference count shares
those pages and moves whenever the engine wraps the code in a shape, so a
dirty page does not drop the trees: it moves a generation, and a held
tree compares its bytes with the guest's before its next use. A changed
program decodes again.

### Entry I - general MOPP ray

The tree shape's ray slot (`0x2df720`) builds the ray VM on its stack and
calls I with the shape's collection and code and its own three arguments.
I fills the VM (sim-rewrite-collision.md, "Ray VM at entry I"), computes
the root frame and segment, and calls the float interpreter `0x3009e0`
with `ecx` the VM and the stack holding the frame, the bytecode pointer
(`code + 0x30 + pc`) and the segment; `ret 0xc`. Each terminal calls the
optional filter (input `+0x24`), the collection's child method (vtable
slot `+0x34`, `key` and a buffer, `ret 8`) and the child's ray method
(slot `+0x24`, `ret 0xc`), then reloads the fraction from the collector.

The kernel is `sim_collision::mopp::ray`. `Ray::root` is the setup:
`one / scale` stored, the root scale `scale * [0x45dcac]` stored, and both
ends as `(p - origin) * root_scale` in packed binary32. `RayWalk` walks the
bytecode the world holds for the code pointer (the same `Tree` and
revision as F) and yields each interpreter call (`Event::Enter`: pc, frame,
segment) and each terminal key; the caller passes the fraction the VM
holds before each step. It replays the original's float sequences in the
guest's x87 precision (single or double; extended declines) and bails
`float` on any operand or result off the exact path (NaN, infinity,
subnormal, an underflowed product or quotient).

I runs in Rust (`engine::sim_collision::mopp_ray::CollectorCast`, driven
by `sim_bridge::sim_native::mopp_ray`):

1. Check MXCSR, the x87 environment (three free registers) and the
   precision, read the arguments, the code's tree, the constants, the
   ray and the collector's fraction, and write the 0x68-byte VM as the
   setup does.
2. Walk to the next terminal. Its stack address is the root
   interpreter's (entry ESP − 4, rounded down to 16, − 0x210) less 0x1c0
   per open interpreter level.
3. Run the filter (input `+0x24`) when there is one, then, if its byte
   accepts the key, the collection's child method into the buffer at
   terminal `+0xa0`, then the child's ray method with the temporary body
   at terminal `+0x60`. Each runs in Rust
   (`engine::sim_collision::mopp_ray::callbacks`), chosen by the address
   its vtable slot holds (table below). Reload the VM's fraction from the
   collector and continue the walk.
4. When the walk ends, return with `ret 0x14` and the caller's EBX, ESI,
   EDI, EBP.

No callback runs in the guest. A slot that resolves anywhere else is
`unimplemented!("collision: mopp_ray: <slot> <address>")`. The targets
(**E**, `h2 disasm` and the XBE's vtables):

| Slot | Address | Object | Port |
|---|---|---|---|
| filter `vt[0]` | `0x2df8f0` | the group filter's ray interface (filter `+0x10`, vtables `0x41258c`, `0x4555d8`) | bit `info & 31` of row `[ray input +0x20] & 31` at `+0x0c`; the info is the collection's `vt[+0x38]`, `0x11ca70`, always 0 |
| collection `vt[+0x34]` | `0x1ef950` | the level (`0x4560f8`, tree `0x456160`) | key filter `0x1ef3e0` (types 1 to 5); a rejected key gets the empty shape `0x456134`; type 1 the surface polygon (`0x1edd00`, flag 1, owner -1, tail the key) |
| | `0x1ee4d0` | an object BSP (`0x456194`, tree `0x455f58`) | the surface polygon, owner `+0x30` |
| | `0x244bb0` | instanced geometry (`0x459e0c`, entry E's tree) | the shape `0x459de4` naming `+0x0c` and the key |
| child `vt[+0x24]` | `0x2df110` | the polygon `0x4560b8` | `vt[+0x20]` `0x1edf50` casts at the collector's fraction; a hit's normal is turned by `[body+8] + 0x20` |
| | `0x2dae30` | the empty shape | returns |
| | `0x244be0` | an instance key's shape | a hit at fraction one to the collector |
| collector `vt[0]` | `0x1829b0` | Halo's phantom ray collector (`0x453b88`, from `0x182800`) | keeps a facing hit below one and its result's fraction, not on the ignored object |

The polygon cast `0x1edf50` takes the plane from the first three
vertices, `(v1 - v0) x (v2 - v0)`, normalised unless its x87 length is
below `[0x45dbdc]`; a ray parallel to it has fraction 0. The fraction must
be below the hit's and in `[0, 1]`, and the hit point, projected by the
normal's major axis and sign (`0x440b94`), inside the ring (`0x23a220`,
no edge with the point on its right). Its projected ring sits 0x40 below
its entry ESP, so a ring past eight vertices declines (`data`).

Not ported, each named when reached: the level's key types 2 (an
instance's transform shape, whose ray `0x2deb20` nests I again through the
welding wrapper `0x2fc0a0`), 5 (`0x1ee500`) and the rest (`0x1ef070`);
entry E's instance collector `0x244c10`, reached when E's native trace
declines; any other convex cast or collector.

A callback can change the bytecode, the VM's endpoints or the walk's
constants; the next step then declines `data`. Queries nest (a child
shape's ray can call I again) on the guest stack below the callback.
A decline after a callback has run cannot be handed back to the lifted
interpreter, and panics like any other.

### Entries G and H - cluster flood and object visit

G floods from a cluster through the portals a sphere reaches, depth first.
The cluster record is `[[0x4e0348]+0xa0] + 0xb0c·i`: `+0x8c` portal
count, `+0x90` the `i16` portal indices. A portal is 36 bytes at
`[sbsp+0x60]`: `+0` and `+2` its two clusters, `+4` its plane index,
`+8` a bounding sphere, `+0x1c` the vertex count, `+0x20` the vertices.
The far cluster is the second when the first is the current one, else the
first. G marks each cluster it enters with the current flood epoch
(`[0x4e7418 + 4·cluster] = [0x4e7414]`), writes it to `out[k]` while the
low half of `capacity − k` is positive, and counts it even when the
output is full. A far cluster already marked is skipped. Otherwise the
portal test `0x14a370` decides:

1. The plane distance on x87, `((ny·cy + nz·cz) + cx·nx) − d`, the plane
   taken from the structure's plane block; the comparison is
   `r > |dist|` on the unrounded value.
2. The bounding sphere on SSE, `((dz² + dx²) + dy²) < (r_p + r)²`.
3. Only then the projection: the normal's dominant axis (`0x120850`: z
   when `|z| ≥ |y|` and `|z| ≥ |x|`, else y when `|y| ≥ |x|`, else x),
   the projection row `2·axis + (n[axis] > 0)`, the foot
   `n·(0 − dist) + c` and the cross radius `sqrt(r·r − d·d)` on x87,
   stored single.
4. The polygon test `0x23a160` over the vertices copied into a
   128-entry frame array: per edge `len2 = ex² + ey²`, skipped when 0,
   `cross = x0·ey − y0·ex`; outside when `cross > 0` and
   `cross² > len2·rr`. A count of 0 or less passes.

G returns the count in EAX and restores the active structure pointer in ECX. Its
flags come from `test ecx, ecx` when that count is 0 or less, and
otherwise from the loop's last compare of the count with itself.

H reads the location's cluster word (`+4`). When it is not `0xffff` and
the radius is above the guest's zero (`0x45dbd8`), H bumps the flood
epoch, sets `[0x4e7411]`, floods with capacity 512 into a local array and
clears the byte. Otherwise it visits the location's one cluster, or none.
It bumps the visit generation `[0x4de2fc]` and holds `[0x4de2f8]` at 1
for the walk. Zero list bits or type mask means all. Per cluster the
collideable list (heads `[0x4de2e0]`, array `[0x4de2e4]`) runs before the
second list (`[0x4de2d4]`, `[0x4de2d8]`). A record is
`data(+0x44) + (datum & 0xffff)·size(+0x24)`: `+4` handle, `+8` next,
`+0xc` type byte, `+0x10` centre, `+0x1c` radius. A reference passes the
type mask `1 << (type & 31)`, then its object is tested and marked at
`0x4de300 + 4·(handle & 0xffff)` before the overlap test, so a missed
object is not tested again through another reference. The overlap is
`(r_ref + r)² ≥ sum`, the collideable list summing `(zz + yy) + xx` and
the second `(zz + xx) + yy`. A full output stops the walk and returns
`(handle & 0xffff0000) | count`; otherwise H returns the count. A next
record whose handle is −1 ends a list. The flags come from
`add esp, 0x420`.

The kernel (`sim_collision::flood`, `sim_collision::objects`) decodes the
portal graph once per structure and version, and reads the reference
lists live on every call. It bounds clusters below 512 and object slots
below 0x800, so the marks it writes cannot reach other globals.

G and H answer every admitted call (`sim_collision::spatial::dispatch`).
A flood with no portal test (a cluster with no portals, or one whose far
neighbours are all marked) and a visit of location cluster −1 replay
registers, scratch stack words, flags and output writes. Every other
call writes the list, the marks, the generations and active bytes, the
return register and both PE flags. The portal test's x87 sequences set
the status word's PE when one rounds (`Checked::x87_inexact`), so a flood
needs no PE on entry.

### Entry J - whole segment query

`engine::sim_collision::query` follows the structure BSP, liquid boundaries,
instance terminals and live object references in guest order. In
`sim_collision::model`, `169510` visits an accepted object’s children
before its next sibling. Its model ray (`1eff40`) selects the live damage
permutation and transforms the ray into each part’s node space. A nearer
part supplies the material and world-space plane. J then computes the
position and applies the requested relocation.

J is answered in Rust (`query::query`): it plans the whole query, then
writes all 0x5c result bytes, the flags argument (the BSP sides and
object bits J sets in place), and the epoch, marks and cleared active
byte of each flood, instance trace and object visit it ran, sets AL to
the hit and returns `ret 0x18`. None of E, G, H or the sphere pretest
runs below it. `collision.trace` runs the same planner read-only and
returns the ordered mark lists. Both decline on unsupported arithmetic
or data. See the
[query order](sim-rewrite-collision.md#54-the-collision-query-entry-j-e) and the
[agent methods](../runtime/agent-api-checks.md#collision-the-native-collision-world).

### Object-reference sphere pretest

The call at `0x169d3a` in J passes the sphere centre in EAX, the segment
start in ECX, the delta in EDX and the radius at ESP+4. The result is AL
and the function pops the radius. The adapter in
`crates/rewrites/engine/src/sim_collision/sphere.rs` selects this call by
its return PC. Other callers use the lifted function.

`crates/rewrites/sim-collision/src/sphere.rs` computes the relative start
and the squared distance from the sphere, with products summed in Z, Y, X
order. A start strictly inside accepts without reading the delta. From
outside, the projection, discriminant and endpoint comparisons keep the
guest's rounding order and reject tangency and first contact exactly at
the endpoint. The adapter restores the scratch registers and epilogue
flags and replays the three direction spills when the delta is read.
Unsupported float inputs, unreadable arguments and unsafe spill frames
decline before any writes.

Synthetic unit tests cover the strict boundaries and rounding order. The
engine tests compare the transfer, CPU and memory with the per-function
port and check that declined calls leave both CPU and memory intact.


## Related

- [collision-queries.md](collision-queries.md): the page this was split from.
