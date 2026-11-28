# Animation sampling (`0x2798a0` and below)

Phase 1a of the [simulation rewrite](sim-rewrite.md). Native only, no
switch. Code:
[`crates/rewrites/sim-anim`](../../crates/rewrites/sim-anim/src/lib.rs) (clips, codecs,
sampler), [`crates/rewrites/sim-core`](../../crates/rewrites/sim-core/src/lib.rs) (float
rules, lanes, epoch) and the adapter
[`crates/rewrites/engine/src/sim_anim.rs`](../../crates/rewrites/engine/src/sim_anim.rs).

Status: Rust + adapter + differential test. Not yet through the run
ladder (A/B, cache-cold arm, cross-arch, timing).

## Boundary

The plan put the boundary at the three evaluation wrappers. Step 0 found
they are not pure animation code:

| Wrapper | Calls before the sampler |
|---|---|
| `0x279d80` plain frame | `0x1daea0` (graph to element), `0x1ddb40` (builds the 12-byte view; reaches the resource cache through `0x1dd7c0` and `0x1237e0`), a prefetch loop over the blob |
| `0x279e40` positional blend | the same two, then `divss`, CRT `0x321099` and `0x320fc6` on x87 to split a position into two frames and weights |
| `0x27a060` interpolated | the same two |

All three take the graph in `eax` and stdcall stack arguments, and end by
calling `0x2798a0` with `esi` pointing at the view in their own frame. So
the native body replaces `0x2798a0` instead: it is called only from those
three, it calls no guest code outside the family, and everything below it
(the stream dispatcher `0x279860`, the per-mode dispatchers
`0x27a3c0..0x27a580`, the ~60 evaluators and every codec) goes. The
wrappers, the resolver and the resource cache stay guest. The other caller
of the codec table, `0x1c73a0`, stays guest too; it writes its own globals
before each codec call (see "Globals").

## `0x2798a0` convention

`ret 0x28`. Callee-saved `ebx`, `esi`, `edi`, `ebp`. Clobbers `eax`,
`ecx`, `edx`, EFLAGS, XMM0-7 and, when an int16 codec ran, the MMX/x87
registers (ends with `emms`: TOP 0, every tag empty).

| In | Meaning |
|---|---|
| `esi` | view: `+0` blob pointer, `+4` sizes pointer (`element + 0x3c`), byte `+8` node count |
| `xmm1` low | frame (f32) |
| `xmm4` low | weight (f32) |
| stack 1 | graph (passed on to `0x27a100` only) |
| stack 2 | element: byte `+0x10` type, byte `+0x13` node count, i16 `+0x14` frame count, `+0x64`/`+0x68` direct-test block |
| stack 3 | node cap |
| stack 4 | remap (`0` for none; byte `+0x1c` bit 0 picks kind 2 over 1) |
| stack 5 | node mask words (`0` for none) |
| stack 6 | output records |
| stack 7 (byte) | interpolate |
| stack 8 (byte) | second frame present |
| stack 9 | second frame (f32) |
| stack 10 | second-frame blend (f32) |

Sizes record (first 16 bytes): signed byte `+0` offset of the animated
stream's flag arrays after the static stream's; i16 `+6` static stream
size; u32 `+0xc` animated stream size. The blob is the static stream, the
animated stream, then three flag arrays (rotation, translation, scale)
per stream, `((nodes + 31) >> 3) & ~3` bytes each, one bit per node, LSB
first.

## Setup

In guest order; `SMALL` is `[0x45dbdc]`, `NEAR_ONE` `[0x45dc78]`, `ONE`
`[0x45dbc0]`.

1. Inactive unless `weight > SMALL`: return, nothing written.
2. `i = clamp(cvttss2si(frame), 0, last)`, `last = frames - 1`. Frames
   A = B = `i`, fraction 0, mode weight `ONE`.
3. Second frame: `j = cvttss2si(frame2)`; second A = `clamp(j)`, B =
   `clamp(j + 1)`, fraction `frame2 - float(A2)`. Primary B =
   `clamp(i + 1)`, fraction `frame - float(i)`. The interpolate byte is
   left as passed.
4. Otherwise, interpolating: if `i == last` stop interpolating. Else B =
   `clamp(i + 1)`, `f = frame - float(i)`; if `SMALL > f` the fraction is
   0 and interpolation stops; if `f > NEAR_ONE`, A = `i + 1`, fraction 0,
   interpolation stops; else the fraction is `f`.
5. Kind: 0 without a remap, else 1 or 2. Count: kind 0 takes the node
   cap when it is at most the element's node count (signed), else the
   node count; kinds 1 and 2 take the node count.
6. Mode: type 1 gives 2 (add) when `weight > NEAR_ONE`, else 3 (scaled
   add), mode weight = weight. Otherwise mode 1 (blend) with the weight
   when `weight <= NEAR_ONE`, else mode 0 (replace).
7. Static stream when type is 0 and the static size is nonzero: integer
   codec slots. Animated stream when its size is nonzero: in mode 0 first
   `0x27a100`, which returns at once when `[element + 0x64] <= 0` and may
   switch to mode 4 otherwise; then the interpolating slots if still
   interpolating, else the integer ones.

The codec table at `0x47fb24` has 40 bytes per codec (the stream's first
byte): integer rotation, translation, scale functions at `+0`, the
interpolating ones at `+12`.

## Evaluators

Per stream, per channel (rotation, translation, scale), the walk visits
flagged nodes below the count in node order, skipping zero flag bytes.
A node's track is its rank among the stream's flagged nodes for that
channel, counted whether or not the mask lets it through. A masked-out
node is skipped. Records are 32 bytes: rotation `+0`, translation `+0x10`,
scale `+0x1c`.

With a second frame, the codec samples both frames and blends first:
rotations by `nlerp(p, q, blend)`, vectors by `(q - p) * blend + p`.
Then, per mode, with record `r`, sample `s` and mode weight `w`:

| Mode | Rotation | Translation | Scale |
|---|---|---|---|
| 0 | `s` | `s` | `s` |
| 1 | `nlerp(r, s, w)` | `(s - r) * w + r` | same |
| 2 | `r ⊗ s` | `r + s` | `r * s` |
| 3 | `r ⊗ s'` | `w * s + r` | `((s - ONE) * w + ONE) * r` |

`s' = [s0*w, s1*w, s2*w, w*(s3 - c) + c]`, `c = ONE | (s3 & sign)`.

Operation orders (each step one rounding):

- `nlerp(a, b, w)`: `m = a*b` per lane; `d = (m0 + m2) + (m1 + m3)`;
  flip `b` by the sign of `d` (`d & [0x47ffc0]`, XOR into every lane);
  `r = (b - a) * w + a`; `n = (r0² + r2²) + (r1² + r3²)`; `r * rsqrt(n)`.
- `a ⊗ b`: `x = [a3;4] * b`; `x += [b3,b3,b3,b0] * [a0,a1,a2,a0]` with
  lane 3 negated; `x += [b2,b0,b1,b1] * [a1,a2,a0,a1]` with lane 3
  negated; `x - [a2,a0,a1,a2] * [b1,b2,b0,b2]`.
- Codec normalisation: `q²` per lane; `p = q² + [q²2, q²3, q²0, q²1]`;
  `t = p + [p1, p0, p1, p0]`; `q * rsqrt(t)` per lane.

`rsqrt` and `rcp` are the Pentium III estimates (`simd` crate tables).

### Broken variants

The mode-0 evaluators with a second frame (`0x27a6e0` masked, `0x27ace0`
unmasked) lose their frame registers to an inline-assembly clobber: the
first live rotation is sampled, a junk blend is computed, the frame
globals are left at the second frame (carrying into the next stream),
and the walk ends; a live translation or scale node runs into padding or
the next function. The kind-1 and kind-2 mode-0 second-frame variants
(`0x27b1d0`, `0x27b920`, `0x27bfe0`, `0x27c750`) also fall into `int3`.
The native body reproduces such a call only when it reaches no live node
(nothing written); otherwise the guest runs it.

## Codec functions

Offsets are from the stream start. Header: `+0xc` translation base,
`+0x10` scale base, `+0x14/+0x18/+0x1c` per-track strides (frame codecs)
or key-time bases (keyed codecs), `+0x20/+0x24/+0x28` key-value bases,
`+0x30` the rotation track words. A keyed track word is
`count | first << 12`.

| Function | Channel | Reads | Computes |
|---|---|---|---|
| `28c470` | rotation | int16 ×4 at `0x20 + 8k` | normalise |
| `28c960` | rotation | int16 at `[+0x14]*k + 0x20 + 8A` | normalise |
| `28c790` | rotation | int16 at A and B | `qA*(ONE - w) + qB*w`, normalise |
| `28c710`, `2c4d60` | rotation | f32 ×4 at `[+0x14]*k + 0x20 + 16A` | copy |
| `28c530` | rotation | f32 ×4 at A and B, `movaps` | `(qB - qA)*w + qA`, normalise |
| `28cdb0` / `28d560` | rotation | keyed, u8 / u16 times | key at A: normalise; else `q0*float(t1-A) + q1*float(A-t0)`, normalise (no bound check on key `lo + 1`) |
| `28c9e0` / `28d180` | rotation | keyed, u8 / u16 times | if `lo + 1 < count`: `t = float(A) + w`, `q0*(float(t1) - t) + q1*(t - float(t0))`, normalise; else normalise `q0` |
| `28c4e0` / `28c510` | translation / scale | `[base] + size*k` | copy |
| `28c750`, `2c4da0` / `2c4de0` | translation / scale | `[base] + [stride]*k + size*A` | copy |
| `28c880`, `28c5d0` / `28c6b0` | translation / scale | A and B | `(B - A)*w + A` |
| `28cf40`, `28d6f0` / `28d090`, `28d840` | translation / scale | keyed, u8, u16 | `v0` if last key or on key; else `rcp(float(t1-t0)) * (float(A) - float(t0))` as `s`, `(v1 - v0)*s + v0` |
| `28cb70`, `28d320` / `28ccc0`, `28d470` | translation / scale | keyed, u8, u16 | `v0` if last key; else `s = rcp(float(t1-t0)) * ((float(A) + w) - float(t0))`, same lerp |

Key search: `lo = 0, hi = count`; while `hi > lo + 1`, `mid = (lo + hi)
>> 1`, `lo = mid` if `time[mid] <= A` (unsigned) else `hi = mid`. A
count of 0 still reads key 0. Any other function pointer in a slot the
call reaches makes the native body decline.

## Writes

The original writes, besides the records: the family's globals
`0x504410..0x5044c4` (frames, mode, kind, count, slots, flag pointers,
track counters, output pointer), `0x55e530..0x55e590` (node masks, zeroed
in setup), `0x55ebb8..0x55ebe8` (evaluator scratch), `0x502430` (only
`0x27a100`), its argument slots 7 and 8, and its dead stack. A
cross-reference census of `.text` found no reader of any of these outside
the family; `0x1c73a0` writes `0x504464/68/6c/80` and `0x5044b8/c0`
itself before each codec call. So the native body leaves all of them
unwritten and writes only the record fields the guest writes.

## Native body

`engine::sim_anim::stream` reads everything first, then writes:

1. The call runs under the guest's MXCSR (`sim_core::det::Fp::guest`):
   an operation whose MXCSR passes `sim_core::det::fast_env` and whose
   operands are zero or of magnitude in `[2^-63, 2^63)` takes the host
   binary32 result, which is the guest's and changes no flag; every other
   one (PE not yet sticky, a subnormal, a directed rounding) takes the
   `simd` crate's SSE model, which rounds and raises flags as the
   instruction would. A `cvttss2si` of a fraction raises PE. The body
   writes the MXCSR back.
2. Setup as above.
3. The clip: `ClipStore` keyed by the sizes address, checked against the
   16 sizes bytes and the view's node count, built from the blob the
   first time (`Clip::decode`: the bytes plus per-stream, per-channel
   `(node, track)` lists). `engine::sim_anim::invalidate` moves
   `sim_core::epoch::SIM` and drops the store at once
   (`ClipStore::invalidate`) on map load (`0x1228f0`), checkpoint revert
   (`0x123f60`), full restore (`0x124380`) and snapshot load (the
   feature's empty snapshot section), so a map's clips are not held
   through a main menu that looks none up. A lookup that finds the epoch
   moved drops it too, and so does growth past 256 MiB.
4. `sim_anim::sample` into a copy of the records, no guest access.
5. Write back the touched fields; MMX effects on x87; `ret 0x28`.

It declines, writing nothing, on: an SSE exception the MXCSR leaves
unmasked (the guest's `#XM`); a guest read fault; no frames or a
frame `cvttss2si` cannot take; a remap (kinds 1 and 2); mode 0 with
direct-test entries (mode 4); a shape without a layout (negative sizes,
over 16 MiB); records misaligned or overlapping the inputs, stack or
scratch; an element node count above the view's; anything the kernel
declines (`sim_anim::Refusal`: an unknown codec function, a key or value
past the blob, a misaligned `movaps` key, the broken variants, each with
its stream, channel, node and track). `stream` returns the `Decline` and
the hook panics with `anim stream: <reason>`, the kernel's refusal
spelled out; no guest runs the
call. `engine::sim_anim::stats()` counts native and inactive calls.

## Deviations

- The boundary is `0x2798a0`, not the wrappers (see "Boundary").
- The family's globals and dead stack slots are not written ("Writes").
- After an MMX codec, x87 TOP and tags match (`emms`), but the contents
  of the (empty) x87 registers are not reproduced.
- The fast-path check covers live lanes. The guest's packed operations
  also compute dead lanes (garbage from `movhlps` shuffles), which could
  set MXCSR status flags the native body does not. MXCSR status is only
  saved and restored around rounding changes elsewhere (46 `stmxcsr`
  sites), never tested.

## Evidence

- `cargo test -p sim-core -p sim-anim`: codecs on synthetic streams
  (int16, float, keyed u8/u16 at, between and past keys), `movaps`
  alignment, every mode, second frames, masks across byte and word
  boundaries, the count limit, broken-variant declines, off-path
  operands, store epoch and rebuild, cold equals warm on both lane paths.
- `cargo test -p engine --features differential --test
  sim_anim_differential` (`H2_XBE`): 600 random synthetic calls over every
  codec the table holds; the lifted `0x2798a0` subtree runs on the IR
  interpreter with nothing stubbed. Final memory outside the uncompared
  ranges, callee-saved registers, `esp`, return target, MXCSR and x87
  control/status/tag must match, cold on host lanes and warm on the
  scalar loop. Declined calls must write nothing.
- The same file's ignored `every_disc_animation_matches_the_guest`
  (`H2_XBE`, `H2_DISC`): every resource animation on the disc at frame 0,
  middle, last and an interpolated fraction. Aggregate counts only.
- The retired per-function ports (the five rotation codecs, the stream
  call and the additive evaluator `0x280ac0`) remain as oracles in
  `sim_native_differential` and `sim_physics_differential`
  (`sim_physics::ORACLES`). Their switches are deleted; the codecs and the
  stream are not in `sim_native::ENTRIES`.
