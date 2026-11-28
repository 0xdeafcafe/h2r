# Marker transforms (`b8d30`'s lookup)

Status: Partly built. The group scan `1d8f00` and the transform fill
`1d8f50` run as one Rust call wherever the marker port runs; the unit
pose, aim and first-person parts of phase 1c are not. Code:
`crates/rewrites/sim-anim/src/markers.rs` (`find_group`, `select`,
`basis`, `model_markers`), `crates/rewrites/engine/src/sim_anim/markers.rs`
(`run`, `fill`). Tests: `crates/rewrites/sim-anim/src/markers_tests.rs`,
`crates/rewrites/engine/src/sim_anim/markers/tests.rs`,
`crates/rewrites/engine/tests/marker_differential.rs`.
Native only, no switch. The marker port is the label
`sim.port.marker_transforms`; it installs where no subsystem body owns
its point.

Phase 1c of the [simulation rewrite](sim-rewrite.md). `b8d30` finds a
model's marker groups by string ID and turns a group's instances into
marker records for the caller: where a weapon attaches, where a grenade
leaves, where a seat sits. The register-level port
(`engine::sim_native::marker`) takes the entry, walks the parent chain in
guest code and reads the model; where it would call `1d8f50`, the
adapter `engine::sim_anim::markers` gathers the inputs, the kernel
builds every record in Rust, and the scatter writes what `1d8f50` writes.

## How it works

1. **Gather** (`plan`). From the port's registers at the call: the
   render model's tag handle in `ecx` (the definition's model tag `+0x38`
   names the tag whose `+4` is this handle), the name in `edi`, the
   object's node array in `esi`, the node matrices in `ebp`, the output
   in `ebx`, the mirror flag at `[esp + 0xc]` and the capacity as the
   signed word at `[esp + 0x20]`. The render model's tag data holds the
   group count at `+0x58` and the group array at `+0x5c`, twelve bytes a
   group: name, instance count, instance pointer. The name-filter window
   is the 256 bytes around the node array (`base - 128`, because the
   filter indexes the array with a signed byte); the matrices are
   gathered to the largest node the selection names.
2. **Kernel** (`sim_anim::markers`). `find_group` is the scan `1d8f00`:
   the first group whose name matches, with its guards (a `-1` handle or
   name, a zero name, no groups). `select` walks the instances in
   `1d8f50`'s order: an instance naming node `i8` passes the filter when
   the window's byte `128 + i8` equals its name byte, an instance naming
   no node (`0xff`) always passes, and a full output (the count reaches
   the capacity) stops the walk rather than skipping on. Then
   `model_markers` builds each record: the local transform (the basis
   leaf below, the scale 1.0, the instance's translation), the world
   transform (`skeleton::compose`, the same `142a60` the skeleton walk
   uses, of the node's matrix and the local one), and the instance's
   trailing float.
3. **Scatter** (`scatter`). The `0x70`-byte records - node `u16` at
   `+0`, local matrix `+4`, world matrix `+0x38`, the trailing float at
   `+0x6c` - and the count in `eax`. The execution continues at
   `b8ded`, the instruction after the guest call, with the stack at the
   port's own frame: the six arguments the guest pushes and `1d8f50`'s
   frame are dead bytes below `esp`, written by neither side of the
   comparison.

A mirrored object (the object's flags bit 10) negates the left column of
the world transform as `0 - x` through [`Fp`](../../crates/rewrites/sim-core/src/det.rs),
not a sign flip: a zero column stays zero the way the guest's `subss`
keeps it.

### The basis leaf

`141e10` turns the instance's quaternion into the local transform's nine
basis floats, scalar `mulss`/`addss`/`subss` in the guest's dataflow:
the squared norm `((q0² + q1²) + q2²) + q3²`; at or below `[0x45dbdc]`
the scale factor is zero (and the basis comes out the identity), else
`[0x45dc0c] / norm` (2 divided by the norm, the kernel's one `divss`,
range-proved in `sim_core::det`); every product multiplies a component
into an already-scaled factor, and the columns are

| Column | Values |
|---|---|
| forward | `1-(q1²+q2²)s`, `(q0q1+q2q3)s`, `(q0q2-q1q3)s` |
| left | `(q0q1-q2q3)s`, `1-(q0²+q2²)s`, `(q1q2+q0q3)s` |
| up | `(q0q2+q1q3)s`, `(q1q2-q0q3)s`, `1-(q0²+q1²)s` |

with each `s`-product and each sum one rounded binary32 operation.

## Interfaces

- Entered only from the marker port's model path, by the port and the
  subsystem entry in `sim_native`'s `install_anim`; nothing else calls
  it. The port's fallback (no model, no group) and parent resolution
  stay guest.
- Writes nothing outside the marker records and `eax`; the dead stack
  below the port's frame stays unwritten.
- `engine::sim_anim::markers::stats()` counts fills and records.
- The kernel models the call as `b8d30` makes it: no remap table (the
  wrapper `1d90b0` is the only caller that passes one, and it stays
  guest), so a selection's node is the instance's byte and never
  negative.

## Rules and limits

The fill declines, writing nothing, and the port panics
`anim marker: <reason>` (no guest `1d8f50` call runs; the guest call is
the path of the switched-off kernel), when:

- MXCSR is not masked, round-to-nearest and PE-sticky (`det::fast_env`),
  or any operand of any operation is off the fast path (`Float`);
- a guest read faults - an unmapped instance pointer or node window
  (`Fault`; the window is read eagerly, so a read the guest would not
  make can only decline, never change a result);
- the model has more groups than the port's scan bound (256) and none
  matched (`Scan`);
- the group has more instances than the kernel gathers, 4096 (`Count`);
- the output overlaps a range the call reads, as virtual addresses
  (`Alias`; the guest interleaves its writes with those reads).

Under those checks every operation is one correctly rounded binary32
add, subtract, multiply or divide, so host results equal the guest's on
arm64 and x86-64 and MXCSR does not change.

## Evidence

- `markers_tests.rs` (sim-anim): the scan, the selection's filter and
  capacity cut, the basis against the formula written out in plain
  `f32`, the identity branch, the fill against the leaves called
  instance by instance, the mirror negation, the record layout.
- `markers/tests.rs` (engine, no image): the port against the kernel
  called by hand on the same synthetic world - records, count,
  continuation and registers - plus every decline leaving the port
  exactly as it is with the kernel off, and the switch.
- `marker_differential.rs` (`H2_XBE`, its own test binary because it
  flips the kernel's process-global switch on): the port, with the
  kernel inside, against the lifted `b8d30` with nothing stubbed,
  `1d8f00`, `1d8f50`, `141e10` and `142a60` all run on the IR
  interpreter. Final guest state outside the dead stack, the
  callee-saved registers, `esp`, the next pc, MXCSR and the x87 words,
  on host lanes and the scalar loop, over synthetic groups, instances,
  matrices, mirrors, capacities and names.
- `tests/sim_native_differential.rs` (`marker_transforms`) keeps the
  port itself - including the handoff it makes when the kernel declines
  - equal to the lifted entry with `1d8f50` stubbed at the call
  boundary.

## Not verified

- Run parity: the sim-budget match against the lifted build, identical `state_hashes_v1` and an empty `h2 sim diff`; the
  cross-architecture arm; the decline rate on real models, and the time
  saved per tick on Waterworks.

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1c.
- [skeleton-walk.md](skeleton-walk.md): the `compose` leaf's contract.
- [anim-sampling.md](anim-sampling.md): the node records the matrices
  are built from.
