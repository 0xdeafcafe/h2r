# Unit pose and overlays (`112070`)

Status: Built. Disc parity and game timing await verification.
Code: `crates/rewrites/engine/src/sim_native/unit.rs` (`run`) and
`crates/rewrites/sim-bridge/src/sim_native.rs` (`install_anim`).
Tests: `crates/rewrites/engine/tests/sim_native_differential/unit_pose.rs`.
Native only, no switch.

The unit pose selects and weights animation layers for a unit's node
buffer. Rust owns the entry, its 41 call continuations and the final
return. The animation manager, graph lookups and evaluators run through
normal guest calls. Their results can change which layers play next.

## Pose evaluation

- **Layers** (`unit/layers.rs`, **E** `112070..1124e6`). Set up the
  scratch channel, select the base, action and overlay layers, and
  evaluate each active aim channel. The channel weight is scaled and
  clamped with scalar SSE. The comparison with its neutral weight uses
  extended precision.
- **Angles** (`unit/angles.rs`, **E** `1124e6..112769`). After the
  object basis call, form the sideways axis and resolve the two aim
  tracks. Project the aim direction onto the three axes in scalar SSE.
  Yaw uses x87 `fpatan`. Pitch takes the square root of the sum of the
  two squared horizontal projections on the x87 stack before its
  `fpatan`. The selected track supplies the four signed angular extents.
- **Overlays** (`unit/aim_overlays.rs` and `unit/tail.rs`, **E**
  `112769..112db8`). Test the angle magnitudes, resolve and blend the
  active channels, and apply the directional overlay. The final layers
  select according to the unit's seat fields. The epilogue restores the
  saved registers and pops the four arguments.

## Call and fallback boundaries

`unit::run` constructs a plan at entry and at each return from a guest
callee. It uses a private CPU and stages the segment's stores in guest
order. Loads see pending writes to the same physical bytes, including
overlapping stack accesses through a second virtual mapping. Only after
all reads, destination checks and arithmetic succeed does the plan commit
the stores and CPU. Each call pushes its real return address. A recursive
call, snapshot or callback can therefore use the guest frame directly.

A trapped or inaccessible memory range, an access that crosses mappings,
a write into the pose instructions, a full store log, or unsupported
floating-point state leaves the segment to the lifted entry or its
original resume selector. This includes an x87 `fpatan` the emulator
cannot model under directed rounding. If the image lacks a return
selector, the hook installer leaves the entire pose to the image.

## Evidence

`unit_pose_segments` compares 24 seeded synthetic fixtures with the
owner's XBE in the lifted interpreter. It checks the transfer, every
register, flags and their validity, MXCSR, the complete x87 state,
ordered stores and resulting memory before each stub answers. The test
requires all 41 return sites and checks that a declined angle segment
preserves its input. The plan tests cover physical aliases and a
protected stack push after an earlier staged store.

## Not verified

The disc differential, whole-tick state hashes and Waterworks timing.
