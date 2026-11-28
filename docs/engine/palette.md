# The first-person bone palette (`16760c`'s palette step)

Status: Partly built. The identity fill and the model-node overlay run as
one Rust call wherever the first-person pose port runs; the layer
blending after the palette and the rest of phase 1c do not (the filter
step is [first-person-filters.md](first-person-filters.md)). Code: `crates/rewrites/sim-anim/src/palette.rs` (`build`),
`crates/rewrites/engine/src/sim_anim/palette.rs` (`run`). Tests:
`crates/rewrites/sim-anim/src/palette_tests.rs`,
`crates/rewrites/engine/src/sim_anim/palette/tests.rs`,
`crates/rewrites/engine/tests/sim_physics_differential.rs`
(`first_person_pose_palette_kernel`).
Native only, no switch. The first-person port is the label
`sim.port.first_person_pose`, which installs where no subsystem body
owns its point.

Phase 1c of the [simulation rewrite](sim-rewrite.md). A first-person
seat draws its weapon and arms from a palette of 32-byte node records,
a rotation, a translation and a scale - addressed by slot. The palette
step of `16760c` fills the palette from the identity record, then walks
two models' nodes and lays each over the slot its palette index names.
The register-level port (`engine::sim_physics::first_person`) stays the
vehicle; where it runs the step, the adapter `engine::sim_anim::palette`
gathers the inputs and the kernel builds the palette span in Rust.

## How it works

1. **Gather** (`plan`). From the port's registers at the step: the node
   count in `ax` of `eax`; the seat's arguments at `[ebp + 8]` and
   `[ebp + 0xc]`, whose `(a + 2b) << 12` over the table `[0x4e9bc4]`
   gives the palette base; the identity record through the pointer at
   `[0x4687d8]`; the two models at `[ebp - 0xc]` and `[ebp - 0x1c]`,
   each a node count at `+0x48` and a node array at `+0x4c` (96 bytes a
   node: translation `+0xc`, rotation `+0x18`); and the palette index
   tables at `ebx + 0xfc` and `ebx + 0x1fc`, one `u16` stride 4.
2. **Kernel** (`sim_anim::palette::build`). Every slot `0..count` takes
   the identity record; then each model's nodes are laid over the slots
   their indices name (`0xffff` names none), the later model winning, a
   record being the node's rotation, its translation and the image's 1.0
   `[0x45dbc0]` as the scale. An index is a signed word, so a model can
   name slots below the palette; the span runs from the first slot
   written to one past the last.
3. **Scatter** (`scatter`). The palette span; the node count at
   `[ebx + 0x300]`; the palette base at `[ebp - 4]`; the final counter
   at `[ebp + 8]` (the second model's count, or zero); the 8 the fill's
   `push 8` / `pop ecx` pair leaves below `esp`; and the registers the
   loops leave: `eax` the graph pointer `[ebp - 8]`, `esi` the second
   model, `edi` the saved value when the fill ran or the first model
   landed a node, `ecx` the last node pointer of the last loop that ran
   (the node count, then zero from the fill's `rep movsd`, then each
   loop's last node), `edx` and the arithmetic flags from the second
   loop's last compare (or its entry compare against zero), and `xmm0`
   the scale constant when either loop landed a node. The call to
   `0xb6780` that ends the step is made either way.

## Interfaces

- Entered only from the first-person pose port's palette step, by the
  port; nothing else calls it. The step's caller (`0xb6780`, the node
  matrices) and everything around it stay as they were.
- Writes nothing outside the palette span and the frame slots named
  above; the step is pure copying, no float arithmetic, so there is no
  fast-path decline and MXCSR cannot change.
- `engine::sim_anim::palette::stats()` counts steps, records and
  declines by reason.

## Rules and limits

The step declines, writing nothing (the port's loops run), when:

- a guest read faults (`Fault`: an unmapped identity, node or index
  address);
- the fill count or a model's node count passes 4096 (`Count`: the
  guest's loops would walk further than the kernel gathers - including
  the three-and-a-half turns the counter's signed-word compare takes
  past `0x7fff`);
- a write overlaps another write or a read, as virtual addresses
  (`Alias`);
- the direction flag is set (`Direction`): the guest's `rep movsd` and
  word copies then run downward from each record's first word and write
  below it, a window the kernel does not model.

## Evidence

- `palette_tests.rs` (sim-anim): the fill, the overlay order and
  layout, the sentinel, the later model winning, negative slots, the
  span without a fill.
- `palette/tests.rs` (engine, no image): the port's step with the
  kernel on against the port's own loops - RAM byte for byte and the
  whole CPU - over fill counts (including negative), model counts,
  index tables and the direction flag; every decline equal to the
  kernel-off run; the switch.
- `sim_physics_differential.rs` (`first_person_pose_palette_kernel`,
  `H2_XBE`): the port, with the kernel on, against the lifted `16760c`
  at every boundary - all memory and registers - over twelve fixtures
  with varying fills, counts and indices. `first_person_pose_update`
  keeps the port itself equal to the lifted guest with the kernel off.

## Not verified

- Run parity: the sim-budget match against the lifted build, identical `state_hashes_v1` and an empty `h2 sim diff`; the
  cross-architecture arm; the decline rate on real seats (`stats()`),
  and the time saved per tick on Waterworks.

## Related

- [sim-rewrite.md](sim-rewrite.md), section 5, phase 1c.
- [markers.md](markers.md): the marker slice of phase 1c.
- [first-person-filters.md](first-person-filters.md): the seat filters and
  weapon-slot update.
- [anim-sampling.md](anim-sampling.md): the node records the palette's
  shape comes from.
