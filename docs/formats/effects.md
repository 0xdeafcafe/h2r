# Halo 2 effects family (`effe`, `prt3`, `pmov`, `jpt!`, `cont`, `deca`, `ligh`, `lens`)

Implementation: `crates/tags/src/effects/` (hand-written fixed codecs, not in
`schema.rs`, like [sound.md](sound.md)). Tool: `h2 effect` in
`crates/cli/src/effects.rs`. Evidence labels as in [map-format.md](map-format.md):
**V** verified on the owner's retail disc, **E** corroborated by the engine's own
code (XBE disassembly, addresses in [engine-map.md](../engine/engine-map.md)), **S** a name only,
unverified. No map bytes, tag names, tag dumps or extracted assets are
committed; tests are synthetic.

## Provenance rules used here

Layouts are derived from the disc and from the engine's own code. Scalar sizes,
block strides, and tag-reference positions were established by (1) the engine
instructions that read each field (recorded as addresses, never as listings),
and (2) structural checks over every instance on the disc: pointers resolve,
strides tile the data, references hit the expected classes. A field with no engine or disc evidence for its meaning stays
`unknown_<offset>` (or an opaque span) and round-trips as bytes. Field meaning
labelled **S** is a name only.

## Shared conventions

(See `crates/tags/src/effects/mem.rs`.)

- **Addressing.** Every type is decoded from the virtual address of its root
  record (`EffectTag::decode_at`), through the whole ordinary metadata space of
  the map, not from the tag's recorded payload. **V:** 590 `effe`, 8 `pmov`
  and some `jpt!` tags record size 0 (a tag sharing another tag's data keeps the
  same address), and 531 `effe` payloads hold only the 0x30-byte root while every
  child block points elsewhere in the map. Decoding from the payload would fail
  on these.
- **Aliasing.** The cache builder de-duplicates identical child blocks and
  function blobs, so a block pointer may land in another tag's memory, and two
  properties may share one blob. Each followed block is charged to a
  [`Budget`] (default 4 MiB and 65,536 elements per tag, counted with
  multiplicity), so hostile counts or aliasing cannot expand without bound.
  A block with count 0 never has its address read.
- **Tag references** are `{class, datum}` (8 bytes, class stored as its
  little-endian FourCC). A null reference has datum `0xffffffff` and a class that
  is whatever the author left (`lsnd`, or all `0xff` for emitters with no
  movement); both are preserved.
- **Properties** (`Property`, 16 bytes): `i16 input`, `i16 range_input`,
  `u16 output_modifier` (1 add, 2 multiply), `i16 modifier_input`, then a data
  reference `{size, address}` to the function bytes. **E:** `0x00246cd0` reads
  exactly these fields. **V:** every property of every `effe` emitter decodes
  and its function size matches its type byte.
- **Layout tiling check (V).** Cache blocks are packed depth-first: an element's
  array, then each of its child blocks in field order, element by element.
  Where a tag's payload is not aliased this is exact, so a layout hypothesis can
  be checked by walking it and requiring the visited blocks to tile the payload
  with no gap, overlap or tail (identical shared blobs counted once). All
  14,195 `effe` instances with a recorded payload tile exactly under the layout
  below; that is the evidence for every stride in the `effe` section.

## Function (curve) data

Implementation: `effects/function.rs`. Evidence: **E** engine evaluator
(`0x0013b390`, helpers `0x0013bec0`, `0x0017c900`, `0x0017ca10`, property
wrapper `0x00246cd0`; table in [engine-map.md](../engine/engine-map.md)), **V** disc,
**S** vocabulary only. A *property* (16 bytes: `i16 input`, `i16 range input`,
`u16 modifier mode`, `i16 modifier input`, data reference) points at a
*function blob*.

| Offset | Field | Evidence |
|---|---|---|
| `+0` | type byte (0..=10 evaluated, see below) | E; V: only 0,1,2,3,4,5,7,10 occur |
| `+1` | flags: bit 0 ranged (use set 1, blend by range input); any of `0xf0` = output used raw, not mapped onto the header range | E; V: bits `0x01`, `0x10`, `0x20` observed |
| `+2`, `+3` | wave / shape selector for set 0 and set 1 (types 2 and 3 only) | E; V |
| `+4`, `+8` | output range (min, max), mapped when `flags & 0xf0 == 0` | E (`0x00246cd0`); V |
| `+0xc..+0x14` | not read by the engine (kept as `unknown_0c`; observed zero, `ffffffff`, or stale bytes) | E |
| `+0x14` | two sets of `T[type]` words each | E, V |

Blob length is exactly `0x14 + 8 * T[type]` words-per-set: 20, 28, 36, 52, 68,
180, 116 (types 7 and 10), 44 (type 9, engine-derived only, never on disc). **V:**
all 3,600 distinct blobs reachable from `effe` emitters on the disc (399,177
property uses over the 30 maps) have exactly this length and parse and round-trip
through `Function::parse`/`to_bytes`; types 6, 8, 9 and above 10 do not occur.

| Type (names **S**) | Set layout (word index into payload, per set) | Evaluation (**E**) |
|---|---|---|
| 0 identity | none | `x` |
| 1 constant | 1 unused word | 0, or `r` when ranged |
| 2 transition | `a, b` | `a + (b - a) * shape[sub](x)` |
| 3 periodic | `freq, phase, lo, hi` | `lo + (hi - lo) * wave[sub](freq * x + phase)` |
| 4 linear | words 0..3 unread; `a, b` at words 4, 5; set 1 `a, b` at words 10, 11 | `a * x + b` |
| 5 linear key | words 9..11 key positions, 13..15 inverse spans, 16 base, 17..19 deltas (set 1 is 20 words later; words 0..8, 12 unread) | `base + sum(delta_k * clamp01((x - key_k) * span_k))` |
| 6 / 8 | no evaluator | 0 |
| 7 spline | words 0..7 unread; words 8..11 = cubic `c3, c2, c1, c0` (set 1 at word 20) | `c3 t^3 + c2 t^2 + c1 t + c0`, `t = x` |
| 9 exponent | `lo, hi, power` | `lo + (hi - lo) * x^power`, 1 if `|power| < 1e-4` (or power < 0 and `|x| < 1e-4`) |
| 10 spline2 | as 7, with words 0..7 = four points `(x, y)` (set 1 at words 12..19) | cubic with `t = clamp(x^(|p0p1| / |p2p3|))` |

Evaluation order: the function value (clamped to `[0, 1]`) for the property's
input and range input; if `flags & 0xf0 == 0` clamp and `lo + (hi - lo) * v`;
then the modifier (mode 1 add, mode 2 multiply, input `modifier input`, ignored
when negative). `Property::evaluate` does the last two steps. An input index
outside the supplied table reads 0.

**Tables.** Waves (12 slots) and shapes (6 slots) are 1024-byte engine tables.
All shapes (linear, `sqrt`, `x^0.25`, `x^2`, `x^4`, cosine ease; names **S**) and waves
1 (zero), 2 (cosine), 4 (triangle) and 6 (sawtooth) are generated procedurally and
match the engine bytes exactly (**V**, all 1024 entries each; only the f32 phase
step of the triangle/sawtooth matters). Waves 3, 5, 7, 8, 9, 10, 11
(variable-period and noise-like ones) are baked data that cannot be derived:
`Tables::with_engine_slab` takes the 18,432 bytes at `0x00468d48` read from the
user's own executable; without them such a wave evaluates as 0. Of the 638 periodic uses in effect emitters, 29 use the procedural
cosine and 609 use the engine-only noise waves 8, 9 and 10, so consumers that
need them must supply the slab at run time.

**Known approximations / open items.** (1) The spline2 remap uses the engine's
exponent-halving square-root trick exactly but an exact reciprocal square root
instead of hardware `rsqrtss` (relative error up to 3.7e-4 on the exponent).
(2) x87 `fistp` rounding is assumed nearest-even (default control word); the
engine's shape lookup relies on it and subtracts 0.1 before rounding (a faithful
quirk). (3) `pow` is computed in f64 and rounded; the engine uses the CRT in
extended precision. (4) Types 6 and 8 are accepted with the sizes the engine's
table implies (276 and 52 bytes) but never occur on the disc. (5) Only the effect
emitter properties were scanned for blobs; `prt3`/`pmov` properties reuse the same
record and evaluator but are validated by their own sections. (6) Names `lo`/`hi`
at header `+4/+8` are the engine's use; the semantics of flag bits `0x10` and
`0x20` beyond "raw output" are unknown.

## effe (effect)

Source for engine claims: addresses in [engine-map.md](../engine/engine-map.md), "Effects:
effect" subsection. Counts below are over all 30 maps (14,785 tags, shared
tags counted per map); every one decodes, every non-null reference resolves to
a tag of the stated class (114,750 references, 27,293 null).

### Root (0x30)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | u32 flags (`flags`; bits up to 0x1a observed) | Meaning unknown. |
| `+0x04` | i16 `loop_start_event` (-1, 0 or 1) | **V** range; name **S**. |
| `+0x06` | u16 `unknown_06` (0..15) | Not an event index (can exceed the event count). |
| `+0x08` | f32 `unknown_08` | Zero on all but 20 tags. |
| `+0x0c` | block of `locations`, stride 4 | **V**; **E**: parts and particle systems index it (`0x0017a380` checks the part location against the count at `+0xc`). Element: a string ID (marker name). |
| `+0x14` | block of `events`, stride 0x38 | **E** (`imul 0x38` in `0x0017a380`, `0x00178eb0`); **V** tiling. |
| `+0x1c` | tag reference `looping_sound` (`lsnd`, null on 13,870) | **V** class. |
| `+0x24` | i16 `unknown_24` (-1, 0, 1, 2, 4) | |
| `+0x26` | u16 always 0 | |
| `+0x28`, `+0x2c` | f32, f32 (0,0 or 10.0, 50.0) | Meaning unknown. |

### Event (0x38)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | u32, always 0 | |
| `+0x04` | f32 `skip_fraction` | name **S**. |
| `+0x08`, `+0x0c` | f32 pair `delay_bounds` | name **S**. |
| `+0x10`, `+0x14` | f32 pair `duration_bounds` (1/30 s on 15,531 of 20,330) | name **S**. |
| `+0x18` | block of `parts`, stride 0x38 | **E**: the event processor `0x0017a380` reads count at event `+0x18` and the array at `+0x1c`; **V** tiling. |
| `+0x20` | block of `beams`, stride 0x3c | **V** tiling only (3 tags use it); no engine consumer traced. |
| `+0x28` | block of `accelerations`, stride 0x14 | **V** tiling (1,253 elements); consumer not traced. |
| `+0x30` | block of `particle_systems`, stride 0x38 | **E**: `0x00178eb0` reads the array at event `+0x34`; **V**. |

### Part (0x38)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | u16 `disposition` (always 0 on the disc) | **E**: a four-way switch (jump table at `0x0017a888`) decides creation. |
| `+0x02` | u16 `create_in` (0, 1) | **E**: compared with the effect's underwater flag (1 skips the part underwater, 2 skips it above water). |
| `+0x04` | i16 `location` | **E**: negative or `>=` the location count skips the part. **V**: always in range. |
| `+0x06` | u16 `flags` | **E**: bit 0 selects a world axis as direction, bit 1 randomises an offset between the two floats at `+0x28`, bits 2 and 4 alter orientation/spawn handling. |
| `+0x08` | FourCC `type_group` | **E**: `0x0017a8a0` dispatches on it. **V**: `deca` 7,451, `snd!` 2,842, `obje` 1,160, `ligh` 834, `lens` 379, `jpt!` 156, `MGS2` 23, `tdtl` 6. |
| `+0x0c` | tag reference `reference` | **E**: datum at `+0x10` is tested for null. **V**: equals `type_group` except `obje`, where it names a concrete object class. |
| `+0x14`, `+0x18` | f32 pair `velocity_bounds` | name **S**. |
| `+0x1c` | f32 `velocity_cone_angle` | name **S**. |
| `+0x20`, `+0x24` | f32 pair `angular_velocity_bounds` | name **S**. |
| `+0x28`, `+0x2c` | f32 pair `radius_modifier_bounds` | **E**: passed to the two-bound random helper at `0x000259d0`. |
| `+0x30` | u32 `a_scalars` (0 or 0x20) | **E**: bit 5 scales by the effect's first scalar. |
| `+0x34` | u32 `b_scalars` (0 on the disc) | **E**: bit 5 scales by the second scalar. |

### Beam (0x3c), acceleration (0x14)

A beam is a `shad` tag reference at `+0`, a u32 at `+8`, and six bare data
references (function blobs) at `+0x0c`..`+0x3c`; it has no per-function input
header, unlike a property. **V**: stride 0x3c from the first function blob
following exactly one element. Acceleration: u16 `create_in` `+0`, u16
`disposition` `+2`, i16 `location` `+4`, `+6` zero, f32 `acceleration` `+8`, f32
`inner_cone_angle` `+0xc`, f32 `outer_cone_angle` `+0x10` (0, 2.0, 180.0 are
typical). Field names **S**; stride and tiling **V**.

### Particle system (0x38)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | tag reference `particle` | **E**: the engine (`0x00173cb0`) accepts only `prt3` and `PRTM`. **V**: `prt3` 41,973, `PRTM` 782. |
| `+0x08` | i32 `location` | **E**: read as a 32-bit value; -1 means none. |
| `+0x0c`, `+0x0e`, `+0x10`, `+0x12` | u16 each (`unknown_0c/0e/10/12`) | **E**: `+0xc != 0` is passed as a flag, `+0xe` to a compatibility check, `+0x12` to the location lookup. Values 0..3. |
| `+0x14` | i16 `unknown_14` (-10..6) | |
| `+0x16` | u16, always 0 | |
| `+0x18..+0x24` | three f32 `fade` | **V**: on every instance `(a, b, 1/b)`. |
| `+0x24..+0x30` | three f32 `lod` | **V**: on every instance `(distance, range, 1/range)`. |
| `+0x30` | block of `emitters`, stride 0xb8 | **V** tiling. |

### Emitter (0xb8)

`+0x00` `pmov` tag reference (null on some). Nine `Property` records at `+0x08,
+0x18, +0x28, +0x38, +0x48, +0x58, +0x68, +0x7c, +0x8c` (**V**: all decode on
all 44,353 emitters). At `+0x78` a u32 `emission_shape` (0, 2, 3, 4, 5 on the
disc; **E**: the emitter initialiser `0x00249170` switches on it through a jump
table at `0x00249a74`). The property at `+0x18` (`properties[1]`) is the
particle lifespan (**E**: the engine takes `1 / max(value, minimum)`), and the
properties at `+0x7c` and `+0x8c` (`properties[7]`, `[8]`) are the shape and
velocity inputs (**E**, same function). The remaining slots are unnamed. Then
five f32 (`+0x9c..+0xb0`, `unknown_9c`) and two u32 (`+0xb0`, `+0xb4`).
Further **E** (the emitter update `0x00247470`, the particle creator
`0x00247950`, the initialiser `0x00249170`):

- `+0x08` property: the spawn rate. `0x00247100` evaluates it times 30
  (`[0x45dcfc]`); `0x00247470` adds that times its first argument to an
  accumulator (`+0xc` of the emitter instance) and creates a particle per
  whole unit (`0x0024773b..0x0024779e`). The argument comes from the
  particle system's update (`0x00248970`, from `0x00175270`, from
  `0x00178c80`'s fourth argument, from `0x00179020`), and is the event's
  step in seconds: `0x001792b0` cuts the effect update's `dt` to the
  event's remaining time (`+0x64 - +0x60`), and the effect update
  (`0x00176e40`, in the game tick at `0x00138069`) passes the game
  time's tick length `[0x510c54]+4` (1/30 s; docs/game/high-framerate.md).
  So the rate is particles per tick: an event of `d` seconds spawns
  `rate x 30 x d`, a one-tick event (retail's most common duration)
  `rate`. When the system's `+0xc` bit 9 is set the burst is
  spread evenly over the step (`0x002476d0`).
- `+0x18` lifespan: `1 / max(value, 0.01)` (`[0x45dc7c]`).
- `+0x28` initial speed: it scales the unit direction the shape gives
  (`0x00249a35`).
- `+0x38` initial rotation in degrees: times `1/360` (`[0x45e28c]`) into
  the particle's rotation in turns (`+0x34`).
- `+0x8c` in shape 0 is a cone angle in degrees: times `pi/180`
  (`[0x45dc54]`) against a random azimuth times `2 pi`.

The Halo 1 converter writes Halo 1's effect particles on these slots
(docs/halo1/objects.md, "Effect particles").

`prt3` tags embed the same particle-system and emitter records (`prt3` block at
`+0x84`), decoded with the same types.

### Open items (effe)

- Root `flags`, `unknown_06/24/28/2c`; event `unknown_00`.
- `beams` and `accelerations` have no traced consumer; the names are **S**.
- Particle-system `unknown_0c..16` and the emitter scalars; the roles of
  properties 0 and 2..6.
- `disposition` has only the value 0 on the disc; its other cases are known from
  the engine switch, not observed.
- The part's `create_in` value 2 and the `type_group` `tdtl`/`MGS2` creation paths.

## prt3 (particle) and pmov (particle movement)

Code: `effects/particle.rs` (`Particle`), `effects/movement.rs`
(`ParticleMovement`). Both implement `EffectTag`; decoding starts at the tag's
root address (recorded payload size is 0 for tags that share another tag's
data), and every child block is read through the map's metadata space, so
aliased blocks that live in another tag's bytes work. Validated over all 30
maps: 4,988 `prt3` and 1,791 `pmov` instances decode with no failure, and every
non-null reference resolves to the expected class (below).

### Property (shared, 16 bytes)

`i16 input` (+0), `i16 range_input` (+2), `u16 output_modifier` (+4: 1 add,
2 multiply), `i16 modifier_input` (+6), data reference (+8 size, +0xc address)
holding function bytes. **E**: the property evaluator (`0x00246cd0`) reads
exactly these fields: it evaluates the function (`0x0013b390`) at
`inputs[input]`, `inputs[range_input]`, then applies the modifier with
`inputs[modifier_input]`. Function bytes are decoded in "Function data".
The input table those indices select is built by the particle input provider
(`0x00248df0`, 17 selectors, a jump table): particle age, per-particle random
values, system/effect scalars and the camera-relative value among them. The
table is not decoded further here.

### prt3 root (0xbc bytes)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | `flags: u16` (observed 0, 4, 6, 7, 8, 0xe, 0x10, 0x17, 0x18, 0xc0 ...) | **V** values only; meaning unknown |
| `+0x02` | `unknown_02: u16` (0 or 1) | V values only |
| `+0x04` | `unknown_04: u32` (0, 1, 2) | V values only |
| `+0x08` | `unknown_08: [u16; 2]` (0..=4, 0..=8) | V values only |
| `+0x0c` | `shader_template: TagRef` (class `stem`) | **V**: 4,988/4,988 resolve to `stem` |
| `+0x14` | parameters block, stride `0x28` | **V**: 5,415 elements tile the data; stride from pointer spacing |
| `+0x1c` | five 16-byte properties (`+0x1c, +0x2c, +0x3c, +0x4c, +0x5c`) | **V** stride/refs; **E** field layout (above) |
| `+0x6c` | `ref_6c: TagRef` (observed `foot`, `snd!`, `effe`, or null) | V classes |
| `+0x74` | `ref_74: TagRef` (observed `snd!`, `effe`, or null) | V classes |
| `+0x7c` | block of 4-byte records (0 or 1 on the disc) | V stride (adjacent allocation); content unknown |
| `+0x84` | nested particle systems, stride `0x38` (85 instances) | **V**: each starts with a `prt3` reference; 56 have no emitters, the rest `0xb8`-byte emitters starting with a `pmov` reference |
| `+0x8c` | embedded shader post-process, stride `0x7c`, count 1 | **V**: stride from the first nested block pointer, its own nested blocks are not followed |
| `+0x94` | `unknown_94: u32` (always 0) | V |
| `+0x98` | `flags_98: u32` (bits 1, 4, 8, 0x800 seen) | V values only |
| `+0x9c` | `quad: [f32; 4]` | V values: (-0.5,-0.5,0.5,0.5) in 2,348 of 4,981 distinct, (-1,-1,2,2) in 1,789 |
| `+0xac` | `uv: [f32; 4]` | V values: (0,1,0,1) in 3,500+, otherwise sub-ranges within 0..1 |

Shader parameter (0x28): `+0` string ID name (**V**: nonzero length and index
resolve), `+4` u32 (0/1), `+8` `bitm` reference (class **V** on all 4,970
non-null), `+0x10` f32 value, `+0x14` three f32 (colour-like), `+0x20` an
8-byte block descriptor that is empty everywhere on the disc (kept raw).

Property slots and root fields: **E**, from the per-particle draw
callback `0x00224550` (passed by `0x002243c0` to `0x00175610`, which walks
each emitter's particles at `[0x51EC84]` with the root as context `[0]`;
the `prt3` accessor vtable is at `0x004535f0`, `PRTM`'s at `0x00453598`
with another layout).

| Root field | Meaning | Evidence |
|---|---|---|
| `+0x1c` property | colour (RGB, the colour evaluator `0x00246d80`) | **E** `0x002245ad`; times the emitter tint per channel (`0x00224639`), clamped to 0..1 |
| `+0x2c` property | alpha | **E** `0x002245bb`; times the emitter alpha (`0x002245ca`), clamped |
| `+0x3c` property | size | **E** `0x00224ca7`; times the emitter size (`0x00224caf`); vtable `+0x4c` (`0x00224350`) gives the bound `max(lo, hi)` used for the radius (`0x00248936`) |
| `+0x4c` property | rotation, in turns | **E** `0x00224595`, times 2 pi (`0x002245a0`) |
| `+0x5c` property | sprite frame, 0..1 over the sequence | **E** `0x002248b6`, times the sequence's sprite count, floored; the fraction blends to the next frame |
| `+0x00` flags (u32 with `+0x02`) | `0x2`/`0x4` random U/V mirror; `0x8` frame range ends on the last sprite; `0x10` sequence per particle; `0x20` no frame blend; `0x80`/`0x100` colour also times the particle's own colour x2; `0x200` dies at rest; `0x2000` rotation +90 degrees; `0x10000` (`+0x02` bit 0) the static quad and UV instead of bitmap sprites | **E** `0x002246f0`, `0x00224907`, `0x002245d5`, `0x002242f0`, `0x00224c75`, `0x002246e0` |
| `+0x04` u16 | vertex shader selector (+7 into the table at `0x0023ec09`) | **E** `0x00224405`; its meaning per value not established |
| `+0x08`, `+0x0a` i16 | first sequence and sequence count (with flag `0x10`) | **E** `0x00224856..0x00224891` |
| `+0x6c` reference | collision effect (`effe`, `foot` or `snd!`), once per particle | **E** pmov kind 1 `0x002b953b` |
| `+0x74` reference | death effect (`effe`, `foot` or `snd!`) | **E** `0x00248340`, `0x00246e60` |
| `+0x98` u32 | mask of the particle inputs the functions use | **E** `0x002244f6`, `0x0022456a`, filled by `0x00173ba0` from `0x00248df0` |
| `+0x9c` f32[4] | sprite quad corners | **E** copied at `0x00224714` to `0x0023ed30`; corner order not established |
| `+0xac` f32[4] | UV window (u0, u1, v0, v1) | **E** `0x002246f3..0x002247e7` |

Size, colour and alpha come together per particle: alpha = clamp(`+0x2c`
x emitter alpha), RGB = clamp(`+0x1c` x emitter tint), size = `+0x3c` x
emitter size; the emitter's own properties (`+0x48` size, `+0x58` tint,
`+0x68` alpha) are evaluated per emitter or per particle by its `+0xb0`
bits. `0x0023ed30` writes one particle's vertex (position, size,
rotation, colour, both frames' UV, the quad) for the vertex shader to
expand. pmov movement kinds dispatch at `0x002b8da0` (kind 1 is the
collision bounce, `0x002b9060`).

Names that exist only as **S** (skip/delay/duration, velocity and cone
bounds, beam, acceleration) are vocabulary, not findings: they label fields
whose layout is **V**/**E** but whose meaning is unconfirmed.

### pmov root (0x14 bytes)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | `template: TagRef` (class `pmov`, null on 30 instances) | **V** 1,761/1,761 non-null resolve to `pmov` |
| `+0x08` | `flags: u32` (0, 1, 3, 7, 0xf, 0x1f, 0x87, 0xc3 seen) | V values only |
| `+0x0c` | movements block, stride `0x14` | **V**: 1,652 packed directly after the root, 26 aliased, none out of range |

Movement (0x14): `+0` kind u32 (0, 1, 2, 3 seen; 1,677 / 469 / 30 / 33),
`+4` parameters block (stride `0x14`), `+0xc` u32, `+0x10` u32 (both
uninterpreted; kind 0 usually `7, 0`, kind 1 `3, 0`, kind 2 `0x1ff, 0`,
kind 3 `5, 8`). Parameter (0x14): `+0` id u32, `+4` property. Observed id sets:
kind 0 `{0,1,2}` (and `{0,1,1,2}` once per 30 instances of the default physics
tag), kind 1 `{0,1}`, kind 2 `{0..8}`, kind 3 `{0,1,2}`. **E** for the
property side only; the meaning of kinds and ids (gravity, drag, collision,
turbulence, ...) is not decoded. A movement-stage executor (`0x00247470`
region) was not read far enough to name them.

### Simulating and drawing a particle: what is and is not known

Known: the particle instance record is 0x40 bytes (**E**, array
`particles` at `[0x51EC84]`: next-index at `+4`, age fraction `+8`,
1/lifespan `+0xc`, position `+0x1c`, velocity `+0x28`), a particle's age
advances by `rate * dt` and it dies at age >= 1 (**E** `0x00247140`), and each
property is evaluated per frame from the input table via the evaluator. The
emitter record (in `effe`, stride `0xb8`) supplies lifespan (property at
emitter `+0x18`, the engine takes `1 / max(value, minimum)`), an emission
shape selector at `+0x78` (eight shapes, jump table `0x00249a74`) and the
shape/velocity properties at `+0x7c` and `+0x8c` (**E** `0x00249170`).
The slots and root fields are now named (the table above); the
movement kinds beyond kind 1 (collision) are not.

### Open items

- prt3 flags `0x1`, `0x40`, `0x400`, `0x800`, `0x1000`, `0x4000`; the `+0x04` shader values.
- Contents of the `+0x7c` 4-byte records (8 distinct values in 50 instances).
- The embedded post-process record's nested blocks (bitmaps referenced there
  are not in `references()`).
- Movement kinds/ids; movement `+0xc`/`+0x10`.
- The nested system/emitter records are kept raw here; the `effe` decoder owns
  their field decoding (the same 0x38/0xb8 records).


## jpt!, cont, deca, ligh, lens

Common to these five: each is decoded from the root record's virtual address
(`EffectTag::decode_at`); the recorded tag size is not used (it is 0 for tags
that share another tag's data). Child blocks are read through the map's whole
ordinary metadata space and charged to a budget, because the cache aliases
identical blocks and function bytes into other tags' memory. Every record also
keeps its raw bytes, so unnamed spans round-trip. Reference classes were
checked over all 30 maps: 1,571 `deca`, 1,596 `ligh`, 1,286 `lens`, 562 `cont`
and 2,722 `jpt!` records decode, and every non-null reference resolves to a tag
of the expected class (counts below).

Function data seen in embedded blocks has first byte (type) and length
`0:20, 1:28, 2:36, 3:52, 4:68, 5:180, 7:116, 9:44` (**V**, all lens/jpt!/ligh
instances); bytes are left to [function.rs](#function-curve-data).

### deca (decal), root 0xac, no blocks

| Offset | Field | Evidence |
|---|---|---|
| +0 u16 | `flags` (0, 1, 8, 0x100, 0x108, 0x148, 0x168 ...) | V values; meaning unreviewed |
| +2 u16 | `kind` 0..=3 | V range |
| +4 u16 | `layer` 1..=5 | V range |
| +6 u16 | `max_overlapping` (8 on 1,419 of 1,570) | E: placement code (`0x0017ef10`) skips the decal when zero |
| +8 tag ref | `next_decal`, class `deca` (null on 1,172 of 1,571) | V |
| +0x10/+0x14 f32 | `radius` pair | E: `+0x14` scales the placement test (`0x0017e670`) |
| +0x18 f32 | `unknown_18` (0.75 on 1,543) | E: read as a scale in `0x0017ef10` |
| +0x1c..+0x34 | two RGB triples | S |
| +0x34..+0x44 | four f32 (180/180/20/20 on 773) | unreviewed |
| +0x88 tag ref | `bitmap`, class `bitm` | V, 1,970 refs resolve across `next_decal` and `bitmap` |
| other bytes | zero on every instance (+0x44..+0x88, +0x90..+0xa4, +0xa8) except +0xa4 f32 = 16.0 | V |

### ligh (light), root 0xe4, three function blocks

Evidence: **V** the disc (`h2 lights --all`, all 1,596 records), **E** the
engine's light update and render path ([e3-lighting.md](../graphics/e3-lighting.md),
"The light record"), **D** eight study copies of the Digsite E3 tag set's
light tags ([pstencil.md](../graphics/pstencil.md)), read outside the
repository. An editing-kit `.light` file holds the same fields in the same
order after a 0x40-byte header and a 16-byte field-set header (file offset
0x50 is `+0`), with 16-byte tag references and 12-byte blocks.

| Offset | Field | Meaning | Evidence |
|---|---|---|---|
| +0 u32 | `flags` | bit 3 no shadow; others in e3-lighting.md "Definition flags" | E, D (the only difference in a shadow/no-shadow pair) |
| +4 u16 | `kind` | 0 sphere (1,349 records), 1 orthogonal (2: one tag, `shq_panic`), 2 frustum, a projected cone (245); 3 (pyramid) not on the disc. Names of 1 and 3 S | V, D |
| +0x08 f32[2] | | 1, 1 on most lights | unreviewed |
| +0x18 f32 | `radius` | a sphere's radius, world units; zero on all but 8 cones, and on 125 spheres | V, D |
| +0x1c f32 | `specular_radius` | a second radius, equal to the first on most spheres | V, D; meaning S |
| +0x20 f32 | `near_width` | a cone's width at its apex | V, D |
| +0x24 f32 | `height_stretch` | a cone's height over width (1 on most) | V, D |
| +0x28 f32 | `field_of_view` | a cone's full angle: **radians in the cache, degrees in a kit tag file** (the cache build converts). Cones on the disc run 1 to 90 degrees; every sphere but one holds the kit's 1-degree default | V, D |
| +0x2c f32 | `falloff_distance` | where a cone starts to fade | V, D |
| +0x30 f32 | `cutoff_distance` | a cone's range; never below the falloff distance | V, D |
| +0x34 u32 | `interpolation` | how the colours below follow intensity | E |
| +0x40, +0x4c f32[3] | `specular_colour` | at the lower and upper intensity bound | E (record `+0xe0`), V, D |
| +0x58, +0x64 f32[3] | `diffuse_colour` | at the lower and upper intensity bound | E (record `+0xd4`), V, D |
| +0x70, +0x74 f32 | `brightness` | intensity bounds | E (record `+0xec`), D |
| +0x78 | `bitmap` (`bitm`; null on 1,358 of 1,596) | the gel (projected texture, S) | V |
| +0x90 | `lens_flare` (`lens`; null on 944) | | V |
| +0x9c | `mgs2` (`MGS2`; null on 1,566) | a light volume; the engine's part switch creates this class at `0x0017a8db` | V |
| +0xb0 f32 | `duration` | seconds; above zero makes the light timed | E |
| +0xb6 u16 | `duration_falloff` | the fade curve over the duration | E |
| +0xc4, +0xcc, +0xd4 | three blocks, stride 8, each element a data reference to function bytes | | V (sizes match types, all instances) |
| +0xdc | `shader` (`shad`; null on 1,593) | | V |
| remaining scalars | opaque in `raw` | | unreviewed |

Blocks are mostly empty: of 1,590 distinct records, 115 use the first block, 32 the second, 1 the third.

Every colour on the disc is within 0 to 1 and every scalar above is finite
and non-negative. 1,275 of the 1,596 records have the no-shadow bit.

**Where lights come from.** The scenario's light placements are at
`scnr +0xe8`, 0x6c bytes each with the palette index (i16) first, and their
palette at `scnr +0xf0`, 0x28 bytes each with the `ligh` reference first
(**V**: on all 26 scenarios with both, the placements end exactly where the
palette begins). Object attachments carry lights at markers (headlights,
flashlights), and effect parts of class `ligh` carry the rest (muzzle
flashes, explosions). Over the disc's 30 maps: 428 placements, 1,033
attachments and 834 effect parts.

`h2 lights <map>` lists each `ligh` tag of a map with its shape, shadow flag,
colour, brightness, radius or cone, and those sources. `h2 lights --all`
checks the invariants above over every disc light and prints the value
distributions. Both write nothing.

### lens (lens flare), root 0x64

| Offset | Field | Evidence |
|---|---|---|
| +0/+4 f32 | `angles` radians | V: +8/+0xc equal their cosines on every record (2,570/2,570) |
| +0x20 | `bitmap` (`bitm`) | V, all 1,286 resolve |
| +0x40 | `reflections`, stride 0x30 (`flags` u16 +0, `index` u32 +4; rest raw) | V stride (size = 0x64 + n*0x30 + function data) |
| +0x4c, +0x54, +0x5c | function blocks, stride 8 | V |

Typed definition: `crates/tags/src/defs/lens.txt` (all 160 distinct
records round-trip). **E**: four paths submit a flare through
`0x0002dba0`: sky lights (`0x0003f1e9`), lights (`0x000c34ac`, the
`ligh` datum at `+0x94`), object attachments (`0x00041b23`) and effect
parts at render (`0x00179d23`; the part creation switch skips `lens`,
`0x0017a8bd`). The submission reads `+0x18`/`+0x1c` (no flare past the
far distance; equal values switch the test off), `+0x28` bits 2/3 and
`+0x2a` bit 0, and `+0x16` (nonzero: a second record). Reflection
fields are named from Halo 1 pairs ([tag-definitions.md](tag-definitions.md),
"Lens flares, debris and light fixtures").

### garb (garbage)

The engine's object type 4 (no scenario placement block). The disc has
one `garb` (30 instances), the item layer and `0xa8` zero bytes; no
effect part names it. Retail effects throw objects through `obje`
parts (`bloc` 1,156, `proj` 4) and debris meshes as `PRTM` particle
models. Definition: `crates/tags/src/defs/garb.txt`.

### cont (contrail), root 0xf0

| Offset | Field | Evidence |
|---|---|---|
| +0 u32 | `flags` (0, 1, 3, 0x40) | V values |
| +0x30 | `bitmap` (`bitm`) | V |
| +0x88 | second bitmap slot, datum null on every disc record | V |
| +0xe8 | `points`, stride 0x40; point +0x10 `physics` (`pphy`) | V: sizes are 0xf0 + n*0x40, n = 2..4; refs resolve (1,510 resolved) |

Engine: arrays "contrail" and "contrail point" are created at `0x0017b7cb`.

### jpt! (damage effect), root 0xc8

| Offset | Field | Evidence |
|---|---|---|
| +0xa8 | `sound` (`snd!`); non-null on only 14 of 2,722 records | V |
| +0x6c | `responses`, stride 0x4c, 0..=2 elements | V |
| response +0/+2 u16 | `kind` (0, 1, 2), `unknown_02` | V values |
| response +0x24, +0x30 f32 and +0x28, +0x34 data refs | scalar plus function (sizes 20, 36, 52, 116, 44) | V |
| response +0x44 data ref | function (20 bytes, 44 once) | V |

Engine: case `jpt!` of the part-creation switch (`0x0017a97f`) calls
`0x000d6660` then `0x000d6c80`; controller feedback is serviced in `0x00222a70`.

### Open items for these groups

- Scalar meanings (flags, ranges, colours, responses) are unreviewed;
  decal and lens flare need their creation and render code traced
  (`0x0017e670`/`0x0017ef10` for decals; lens render not located). The light
  path and the `ligh +0` bits it tests are in [e3-lighting.md](../graphics/e3-lighting.md),
  "Phase 0 findings".
- `MGS2` is the class the engine creates; its role is unconfirmed.
- Reference counts (all 30 maps, shared tags counted per map): deca 1,970 resolved,
  ligh 923, lens 1,286, cont 1,510, jpt! 14.

## Validation

Command: `h2 effect --all <map|all> [--disc iso]` decodes every tag of the eight
groups in the map (or in all 30 disc maps), follows every outgoing reference
(non-null references must resolve to a tag of the stated class, or one derived
from it in the map's group hierarchy), parses every non-empty function blob and
evaluates it at four inputs (the output must be finite), prints one row per
group and exits non-zero on any failure. An empty function blob is an absent
function: the engine's evaluator returns 0 for it (`0x0013b3aa`).

All 30 disc maps, shared tags counted once per map:

| group | tags | decoded | failed | refs | null refs | unresolved | wrong class | function blobs | bad functions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `effe` | 14,785 | 14,785 | 0 | 114,750 | 27,293 | 0 | 0 | 399,207 | 0 |
| `prt3` | 4,988 | 4,988 | 0 | 20,545 | 9,899 | 0 | 0 | 22,967 | 0 |
| `pmov` | 1,791 | 1,791 | 0 | 1,791 | 30 | 0 | 0 | 6,373 | 0 |
| `jpt!` | 2,722 | 2,722 | 0 | 2,722 | 2,708 | 0 | 0 | 12,078 | 0 |
| `cont` | 562 | 562 | 0 | 2,449 | 939 | 0 | 0 | 0 | 0 |
| `deca` | 1,571 | 1,571 | 0 | 3,142 | 1,172 | 0 | 0 | 0 | 0 |
| `ligh` | 1,596 | 1,596 | 0 | 6,384 | 5,461 | 0 | 0 | 148 | 0 |
| `lens` | 1,286 | 1,286 | 0 | 1,286 | 0 | 0 | 0 | 370 | 0 |

The unique-tag counts (915, 253, 89, 161, 34, 158, 192, 161) are the distinct
datum/data identities across maps; the `effe` layout also tiles its payload
exactly for all 14,195 instances with a recorded payload (see "Shared
conventions"). `cargo test -p tags effects` runs the synthetic tests (59 `#[test]` functions under `crates/tags/src/effects/`: valid
decode, empty blocks, aliased pointers, truncated and out-of-bounds blocks,
over-budget counts, unknown values preserved, per-type function evaluation); none
needs the disc.
