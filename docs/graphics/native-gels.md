# Native shader gels

The native gels are on by default and go off with the rest of the native
renderer (`impl.render.native=off`). Supported shader
gel quad packets are replaced through `DrawAdmission`. The handoff records
both required target copies transactionally, then commits the commands
before suppressing the original D3D packet. Any preparation, resource,
validation or range-count failure retains the original packet.

No builds or tests were run in this lane. Source formatting and diff
checks are the only checks performed; runtime coverage/parity remain for
the single integration build after all lanes finish.

## Production order

**Code**, independently read with `h2 disasm` on the owner's XBE:

- `0x0004a1e0` finishes its light constants through `0x0003a550`.
- `0x0003a555..0x0003a561` uploads the light constants. A valid `shad`
  reference at the light definition's `+0xdc/+0xe0` sets the byte
  `0x00485a77` while its shader is being generated.
- `0x0003a5ab`, return `0x0003a5b0`, calls
  `0x00016610(shader, 1, -1, 0, 13, 1)`: **shader layer 1 into target
  13**, not shader layer 13. The earlier native-renderer description
  conflated those numbers.
- The template sets up each implementation and loads classification 0.
  Its full-target quad helper `0x00015ec0`, return `0x0001675a`, binds
  target 13, resets viewport/scissors through SetRenderTarget, and draws
  a four-vertex triangle strip per level. v0 is position, v1..v4 are
  two-component UVs, v5/v6 are zero for one level.
- The original clears `0x00485a77`. Engine texture 10 can then sample
  the generated target for this light. While that byte is set, texture
  10 instead takes the definition's bitmap, avoiding feedback.

The feature hooks the template and quad helper, both context-only. It
never hooks a D3D-owned entry. At the helper it drains preceding commands
through `begin_external_boundary` and arms admission for exactly the next
resolved draw. Any mismatch consumes the scope and retains D3D.

`native_gels_replace` checks the bound surface, shape, declaration, four
actual vertices, viewport/scissor, shader and all required resource views.
Its `ExternalPlan` promises one gel range. `prepare_copy` validates every
physical copy before recording. `record_ranges` returns one only after the
complete native quad is recorded into **the real target-13 attachment**.
It declares colour writes and no depth/stencil writes, so the handoff
maintains D3D surface coherency and aliases. No queue submission, queue
write or update to a live texture escapes the isolated command encoder.
The commit and suppression are owned by the core handoff (merged through
`bd56af41`), not by a hook-side boolean.

Guest shader setup, geometry assembly and target/state effects continue.
Consecutive native and D3D gel packets therefore keep their original
order. The actual target retains old pixels for blend, colour masks and
fragment discard. Uniform/vertex buffers belong to each recording.

## Resources and current limits

Replacement consumes the **resolved draw**, including its live microcode,
constant banks, combiner definition, defaults, render/sampler state,
resident texture views and raw float4 vertices. Shader variants and bitmap
identity animation can therefore use their actual evaluated resources;
no static asset or animation-function approximation stands in for them.
Linear texture coordinate scale uses guest dimensions. Signed textures
use the D3D path's already biased views. Clip range/clamp, stipple, blend
constant, bump uniforms and two-sided lighting come from that packet.

Current admission requires a resident single-level target 13, a
four-vertex non-indexed strip, complete float4 v0..v6 streams, full
viewport/scissor, solid Gouraud shading and no depth/stencil/fog/alpha
test. Depth textures, cube/volume stages, colour keying, border sampling,
unknown inputs or missing copies are refused. Nothing partially records
and suppresses a subset of the quad.

Immutable resident programs, texture groups and pipelines are cached by
map generation, exact resident view identity, shader/state/sampler key and
target format. Resident identity includes D3D's palette/residency result.
Repeated frames do not rebuild their pipelines. Per-recording buffers are never reused before submission.
The resident program cache is bounded; eviction retains no live mutable
GPU storage and recorded commands retain their resource references.

## Interface for the lights lane

`boot::native_gels_glue::install(device) -> Arc<Outputs>` installs the
host. Admission is registered once, lazily, only when the enabled feature
first reaches replacement mode after its ordered boundary. Disabled
calls do not add a gels admission or cause extra draw
snapshots/palette reads. Retain that owner when installing the light graph
(the current world installer discards only the extra returned Arc).
`Binding` is re-exported from `native_gels_glue`.

1. At light entry, before original constants run, retain
   `let before = outputs.checkpoint()`.
2. Keep the original constants/gel setup running. A whole-light skip at
   `0x00031170` bypasses production and must not take a stale binding.
3. At the post-constants boundary use
   `outputs.binding_after(before, definition_ptr, shader_datum,
   target13_surface) -> Option<Binding>`. Retain the binding in the
   light's immutable plan. Its checkpoint rejects the previous light,
   even when definition, shader and surface are equal.
4. `let gel_slot = binding.append(&mut plan)` adds a read-only surface
   slot to the consuming light's `ExternalPlan`. Equivalently append
   `Some(binding.slot())`. Drain/prepare at the ordered light boundary,
   so pending producer commands are recorded before its consumer.
5. In light recording use `binding.sample(&target, gel_slot)` as the gel
   view for `LightGpu::textures`. Each copy gets its own live target-13
   sample view; a missing slot refuses the light's whole transaction.
6. Generated target 13 is **linear**. For each sampled `LightStage::Gel`,
   use `binding.tex_scale()` in `PixelUniforms::tex_scale`: reciprocal
   **guest** width/height, not the scaled physical dimensions or static
   bitmap coordinate scale.

This binding means “target 13 as the producer leaves it,” including D3D
fallback or later D3D gel packets. It does not claim a particular packet
committed natively. `native_gels_suppressed` and the per-copy range counters
in `H2_FRAME_STATS` establish actual replacement. Lights should use
`binding_after`.

## End-of-integration checks

Commands for the coordinator's single integration build/test phase;
**not run by this lane**:

```sh
cargo test -p engine --lib gel::tests
cargo test -p feature --test native_gels
cargo test -p native-render --lib gels
cargo test -p native-render --test gels_gpu
cargo test -p boot --lib native_gels_glue::tests
cargo test -p boot --lib native_gels_replace::tests
cargo test -p boot --lib registration::tests
cargo test -p boot --lib render_thread::tests
```

The ignored `boot --test render_points_xbe` list now includes
`TEMPLATE_QUAD`. Synthetic cases cover admission identities, float4 stream
ordering/refusal, SSE depth, output/cache generation, preserved masked
channels on the live target and stale-light binding checkpoints. Core
handoff fixtures cover rejection of a display copy after reference
recording without committing either copy.

Requested runtime proof, only after integration:

- `H2_LEVEL=03a_oldmombasa`, `H2_RENDER_SCALE=1`,
  `H2_FRAME_STATS=1`, `H2_BOOT_STOP_AT_PRESENT=4000`, against the same
  run with `H2_FEATURES=impl.render.native=off`. Require positive gel
  suppression/range counters. If 03a has no supported gel, report zero coverage and choose a
  level with one before judging parity.
- Repeat at `H2_RENDER_SCALE=2`; require both copy counts, stable refusal
  counts and no partial recording invariants. Keep other switches equal.
- Check mixed native/D3D lights with the light lane's binding.

Agent-API decision: no running game/editor inspection was performed.
Neither API was used or expanded. Static XBE reading used the existing
disassembler. Coordinator runtime proof must use `h2 agent`.

## Shared files edited by this lane

- `Cargo.lock`, `crates/boot/Cargo.toml`: direct shaders dependency.
- `crates/boot/src/lib.rs`: gel modules.
- `crates/boot/src/native_world_glue.rs`: install gel host.
- `crates/boot/src/registration.rs`: register default-off feature.
- `crates/boot/src/registration/tests.rs`, `crates/boot/src/render_thread.rs`:
  include the switch in all-on hook checks.
- `crates/boot/tests/render_points_xbe.rs`: ignored quad discovery point.
- `crates/engine/src/lib.rs`, `crates/engine/src/points.rs`: gel module and
  named quad hook.
- `crates/feature/src/lib.rs`, `crates/native-render/src/lib.rs`: module lists.

Other shared-file changes on the branch belong to the explicitly requested
handoff merge `f7918d3e`; they were not independently edited by this lane.
