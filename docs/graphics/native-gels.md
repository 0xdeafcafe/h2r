# Native shader gels

`impl.render.native_gels` is off by default. The first slice produces a
native equivalent of render target 13 alongside the guest draw, before
the consuming light runs. It suppresses no guest draw and does not enable
animated gels in the light pass. The lights lane owns that integration.
No performance improvement or pixel parity is claimed before the compare
has actual native coverage.

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
  normalized two-component UVs, v5/v6 are zero for one level.
- The original clears `0x00485a77`. Engine texture 10 can then sample
  the generated target for this light. While that byte is set, texture
  10 instead takes the definition's bitmap, avoiding feedback.

The feature hooks the template and quad helper, both context-only. It
never hooks a D3D-owned entry. The host arms a one-strip `drop=false`
observer at the quad helper. That drains shader setup before taking its
constant/state snapshot. After the guest strip, the external pass receives
the final view constants and W clip, then renders into its own texture.
Each encode gets immutable uniform buffers and its own output texture.
The D3D target remains intact. An observer mismatch or encode refusal
publishes nothing and leaves the original draw in place.

SSE quad depth arithmetic follows `0x00015f30..0x00015fc1` in
`engine::gel::depth`, using caller MXCSR and constants read from the image.
An unmasked exception refuses production. Guest CPU/FP state is untouched.

## Supported first slice

One layer-1 pass, implementation 0, a single resident target level, a
classification-0 program reading only the helper's v0..v6 inputs, resolved
2D bitmap stages, full colour writes without blending or depth/stencil,
solid Gouraud shading without culling, no fog/alpha test/stipple/two-sided
lighting or fragment discard. The vertex interpreter verifies that all
four corners replace the full target and survive clipping; otherwise
prior target contents would be required and production is refused.
Live vertex constants and evaluated combiner/render/sampler state retain
constant, render-state and stage-state animation without re-evaluating
functions. Bitmap identity animation and combiner variants are refused.
Unknown stage sources are refused; engine texture 10 is allowed only with
its definition bitmap. Linear images, P8 without a palette, cube/volume,
signed textures, colour keying and border sampling remain on D3D. Bitmap
uploads use the existing BC-preserving `StageDecoder`.

The first slice deliberately keeps guest setup and its draw, including
all target/state effects. Resource loading is asynchronous. A loading,
failed or unsupported shader produces no ticket. The loader verifies its
map generation before publication. The production hook checks the map,
definition, shader and target again at use.

## Interface for the lights lane

`boot::native_gels_glue::install(device) -> Arc<Outputs>` installs the
host. Retain that owner when installing the light graph (the current
world installer calls it and discards only the extra returned Arc).

1. At light entry, before its original constants run, retain
   `let before = outputs.checkpoint()`.
2. Keep the original constants/gel setup running. A whole-light skip at
   `0x00031170` bypasses production and therefore cannot consume a native
   shader gel from this slice.
3. At the post-constants boundary, use
   `outputs.produced_after(before, definition_ptr, shader_datum,
   target13_surface) -> Option<Ticket>` and retain that ticket in this
   light's immutable draw plan. The checkpoint rejects the previous
   light's result even if its definition, shader and surface are equal.
4. Inside each light encode, use
   `ticket.output(ticket.key, gel_guest_size, gel_copy_size) -> Option<Output>`.
   Bind `Output.view` in the existing `LightStage::Gel` binding
   (`LightGpu::textures` accepts a gel view). `Output.size` is the
   produced copy's texel size; `Output.guest_size` is target 13's size.
   A missing reference/scaled copy refuses that light. Do not substitute
   the last successful copy or the definition's static bitmap.

A ticket holds a distinct texture for each invocation and copy, so another
light cannot overwrite one already recorded. Producers and consumers must
be recorded in order on the same backend/device. Tickets prove recording,
not measured parity or completion of the GPU submission. Replacing the
original target-13 draw, or accepting a light before its producer is
recorded, requires the core handoff's complete prepare/commit contract.
The pass writes only its private output; it must declare no guest-surface
writes when integrated with the new handoff API.

## Comparison and coordinator validation

`H2_NATIVE_GELS_COMPARE=N` compares the first N armed observer calls,
each reference/scaled copy independently (empty value: 4; unset/0: off).
Copies are read back with mapping scheduled **on submission**, without a
wait. Logs include invocation identity, dimensions, differing bytes,
maximum absolute byte difference and total difference. Alpha is included;
row padding is excluded. An unpaired copy is reported and publishes no output. A later mapping
failure is reported and provides no parity verdict. `native-gels: produced ...` lines show
positive native coverage even without compare enabled. Zero such lines
is not a parity result. Resource/refusal reasons are bounded in the log.

Synthetic checks for the coordinator; this lane did not run cargo, builds
or the game, as required by its preamble:

```sh
cargo test -p engine --lib gel::tests
cargo test -p feature --test native_gels
cargo test -p native-render --lib gels
cargo test -p native-render --test gels_gpu
cargo test -p boot --lib native_gels_glue::tests
cargo test -p boot --lib registration::tests
cargo test -p boot --lib render_thread::tests
```

The GPU fixture uses synthetic WGSL and verifies two differently coloured
gels recorded before one submit, retained output identity, wrong-ticket
rejection and missing scaled-copy rejection. The other tests cover exact
call admission, target readability/levels/visibility, SSE depth and
exception handling, hook placement, quad registers, byte comparison and
stale output checkpoints. rustfmt was run only on the new files;
`git diff --check` is the source-only hygiene check.

Requested runtime proof for the coordinator (use the existing image and
agent API, no lane-built game image):

- `H2_LEVEL=03a_oldmombasa`,
  `H2_FEATURES=impl.render.native_gels=on,impl.render.native_lights=off`,
  `H2_NATIVE_GELS_COMPARE=16`, `H2_RENDER_SCALE=1`,
  `H2_BOOT_STOP_AT_PRESENT=4000`. Cover intro and street; require actual
  producer lines and exact pairs. If 03a has no supported shader gel,
  report zero coverage and choose a level with one before judging parity.
- Repeat at `H2_RENDER_SCALE=2`; require both copy sizes and stable
  producer/refusal counts. Other stage switches should match the first
  run. Compare stays on for parity and off for any later timing run.
- Once the light lane consumes the tickets, repeat with
  `impl.render.native_lights=on` and mixed native/D3D lights, retaining
  original shader-gel production until both copies are admitted.

Agent-API decision: no running game/editor inspection was performed in
this source-only lane, so neither API was used or expanded. Static XBE
reading used the existing disassembler; it is not process inspection.
Runtime proof belongs to the coordinator and must use `h2 agent`.
