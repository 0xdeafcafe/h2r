# Native particles and effect geometry

Status: first standalone particle slice, behind
`impl.render.native_particles` (off by default). Source implementation and
synthetic tests; **not built, tested or run by this lane**. Integration must
install the confirmed immediate-draw adapter before this replaces game
pixels. Without that adapter every draw stays on D3D. This is not evidence
of native coverage or speedup.

## Particle boundary

Code evidence from the owner's executable, inspected with `h2 disasm`:

- `0x00174420` updates particle/effect state while rendering. It is **never
  skipped** here; neither are property evaluation or emitter iteration.
- `0x002243c0` sets up the particle material, selects the vertex program
  through `0x0023ea80`, then walks particles with callback `0x00224550`.
- `0x00224550` evaluates colour, alpha, rotation, size and sprite windows;
  `0x00224cd2` calls `0x0023ed30` with eight stack words and an EBX pointer.
- `0x0023ed30` writes dynamic attribute registers selected at
  `0x00470a3c..0x00470a58`, then one four-corner quad. The two-float writes
  reset z/w to 0/1. Packed colour is little-endian RGBA, **not** ARGB.
  `0x0023ecd0` initializes the register assignments; we read the live
  assignments and refuse overlaps or register 0 rather than copying that
  table. `0x00470a38` gates emission.
- The inline corner payload is eight words for four vertices. The first
  slice admits only a *live declaration proven by the adapter* to be v0
  SHORT4, no other arrays. It expands (0,0), (1,0), (1,1), (0,1), z/w both
  zero. A different declaration refuses; the payload alone is not proof
  of SHORT4. Quad triangulation uses the D3D diagonal (0,1,2), (0,2,3).

`native_render::particles::read_emission` reads the helper inputs without
writing guest memory. The feature's context-only hook always returns
Continue. Keeping the helper running preserves allocator failure behavior,
push-buffer put pointer, shader state and the last attribute values that a
later native or D3D draw inherits. The adapter replaces only the captured
primitive after those effects have occurred. This slice saves submission
work only; it does not remove the per-particle property evaluation.

## Integration contract (lane-local assumptions)

New files own all implementation. The sorted-transparent dispatcher stays
with `lane-sol-transparents`; this slice neither sorts nor deduplicates nor
collects draws across a D3D boundary. Its hook follows guest call order,
including late and first-person invocations of the particle helper.

1. Install `boot::native_particles_glue::install(Arc<dyn
   ImmediateAdmission>)`. `arm_particle` receives the helper's ESP,
   return address, independently decoded emission and Native/Compare mode.
   Arm a **call-scoped capture** with a real call identity. An unrelated
   quad of the same shape must not pair. Cancel at helper return if its
   allocator failed or no primitive was emitted. Refuse interleaving or
   partial capture. Do not implement this with the old global next-draw
   swallow mechanism.
2. At that primitive's original issue boundary, drain earlier guest work,
   apply its state/attribute updates, then retain the original draw.
   Construct `ImmediateDrawSnapshot` for each reference/display target
   copy. `DrawIdentity` includes device, map generation, view, call and
   copy. `TargetKey` includes generation-bearing attachment identities,
   formats and size. Views/samplers must belong to that device and current
   generation. Snapshot microcode includes the XVU header; pixel shader
   identity retains `explicit_final`, not merely its 240-byte serialization.
3. Resolve the **actual** selected shaders, inherited pipeline state,
   all vertex and pixel uniforms, defaults, live declaration and resident
   textures. No tag-level approximate material: use the resident mip chain,
   palette, signed conversion, BC upload, sampler and texture dimensions
   D3D would use. Resolve W/Z mode and range, clamp, alpha test, fog,
   stipple, blend, stencil/ref, cull/winding, bias, scissor and target-copy
   scaling. Incomplete state refuses. Sample count must be one; target
   views must be correctly formatted render attachments. Sampled views
   must be filterable colour views of the declared dimension. Shadow
   sampling refuses in this slice. No sampled view may alias an output
   attachment; resolve such reads beforehand or refuse them.
4. Decode D3D's vertex declaration into effective float4 registers in
   `snapshot.vertices` **independently** of our emission decoder. With
   live v0 SHORT4, call `prepare_particle(renderer, device, snapshot,
   emission, Declaration::CornerShort4, mode)`. A bitwise mismatch of any
   register or index refuses. Compare mode logs the exact identity,
   target, differing-register mask and resident texture identities, then
   returns no replacement. It must leave D3D drawing.
5. Prepare **all target copies** before recording any. Preparation resolves
   the shaders and every texture, creates the pipelines/groups, uploads
   immutable vertex/index/uniform buffers, but records no attachment write.
   Validate every `PreparedDraw::matches` against the still-current target
   before recording any copy. A failed copy means D3D draws all copies.
6. Record in sorted order with `PreparedDraw::record`, which consumes the
   prepared draw and returns one-draw/index-count evidence. Only a receipt
   for every required copy permits suppressing the original submission.
   Preserve the backend's pre-write/COPY bookkeeping, colour and
   depth/stencil generation updates and alias invalidation. No partial
   recording may fall back over an already drawn prefix. Pending probes
   must be invalidated on view, generation, restore and device changes.

The renderer caches translated programs/pipelines per device; clear it on
map generations. It uses owned upload buffers rather than persistent
queue-written ranges, preserving multiple views/copies in one submission.
Rebind those allocations to the handoff lane's submission arena after the
same lifetime can be guaranteed. It uses the translated program's viewport
math against the full target and applies the already clipped host scissor.

No adapter is installed automatically yet. The feature's arming counts are
not replacement counts. `PreparedDraw::record` receipts are the coverage
oracle. A native-enabled A/B with zero receipts proves nothing.

## Diagnostics and coordinator checks

`H2_NATIVE_PARTICLES_COMPARE=N` enables read-only comparison for the first N
helper calls (empty means eight); after the budget, every remaining call
continues on D3D. Invalid/zero values disable comparison. The switch must
also be on. Without an adapter the diagnostic explicitly prints
`adapter=false`, `armed=false`; this is decoder coverage only. With an
adapter, require paired `geometry=Ok(0)` lines. Shader/state/resource values
come from the single captured draw; there is no cross-frame "same shader"
heuristic. Comparison is not a pixel-parity claim.

Coordinator commands (not run by this lane):

```sh
cargo test -p native-render --no-default-features --lib particles::
cargo test -p native-render --test particles_gpu
cargo test -p feature --test native_particles
cargo test -p boot native_particles
```

The GPU test uses invented microcode/geometry, draws two distinguishable
views into one command submission and checks colour readback. It also
checks refusal of a changed attachment identity. It skips if no GPU
adapter is available; report that as a skip. Pure tests cover dynamic
register assignment, colour channel order, z/w reset, NaN/signed zero,
missing memory, alias/overflow refusal, clip/depth mismatch and malformed
geometry. The feature test requires one render-safe leaf hook and none on
`0x00174420`.

Requested level runs after adapter integration, using the coordinator's
existing image and `h2 serve` plus `h2 agent` for control/capture:

- `H2_LEVEL=03a`, `H2_OPT=2`,
  `H2_FEATURES=impl.render.native_particles=on`,
  `H2_NATIVE_PARTICLES_COMPARE=64`. Compare the first 4,000 presents,
  including intro and street, collecting at presents 1,000/2,000/3,000/4,000.
  Require positive paired coverage before switching to Native mode.
- Same save/input, 4,000 presents, compare variable **unset**, particles
  off/on. Keep every other feature fixed. Compare colour/depth plus
  particle/effect evolution at those presents. Require identical guest
  particle updates and positive native receipts. Repeat with transparent
  native geometry enabled to exercise alternating native/D3D entries.
- Repeat the combat/explosion section for 600 ticks at 1x and 2x and two
  views; each target copy needs its own receipt. Include sprite blending,
  near clipping, alpha test, additive particles and D3D-only neighbors.
  Capture 600 ticks through `h2 agent capture 600`, with diagnostics off
  for timings. No performance claim until warm p50/p95 and coverage exist.

Agent API decision: no running game/editor inspected or driven in this
lane (explicit no-run rule). Only static executable inspection and source
changes; no API expansion needed. All future running-game comparisons use
the existing agent API.
