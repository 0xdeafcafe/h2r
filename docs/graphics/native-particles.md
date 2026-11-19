# Native particles and effect geometry

Status: particles, beam emissions, generated contrails and the researched
four-vertex effect builder are connected to `d3d::DrawAdmission`, behind
`impl.render.native_particles` (off by default). The handoff commit
`2fe68835` was merged, not rebased. **No cargo, build or game run was
performed by this lane.** Source checks and synthetic tests are supplied;
coverage, pixel parity and speedup remain coordinator checks.

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

## Contrails and other effects

- `0x00042b20` enumerates contrail records and channels. `0x00043740`
  queues a sorted entry through `0x0001e5e0`, with material setup
  `0x00043720`, builder `0x00042cf0`, stride 24 and two vertices per point.
  The existing `0x00035b00` sorted handler selects the live declaration and
  invokes `0x0004b220`. That dispatcher is unchanged.
- `0x00042cf0` retains ownership of ribbon construction: camera facing,
  endpoint fading, curve evaluation and point state. The native decoder
  checks its completed bytes, with a live FLOAT3/FLOAT2/D3DCOLOR layout at
  offsets 0/12/20. Register locations come from the live declaration.
  Packed BGRA goes to GPU vertex fetch unchanged. Triangle strips retain
  vertex order, duplicate/degenerate points and strip topology.
- `0x0004b220` receives stride in ECX and mode, primitive index, vertex
  count, builder and context on the stack. Only mode zero is admitted.
  The primitive index is resolved through the live table at `0x0043fdc0`;
  it is not assumed to be the NV2A topology. Its push-size limit is checked
  before allocation. The original callback and allocator still run.
- `0x00052040` calls the same upload helper with builder `0x00052020`,
  which copies four 20-byte FLOAT3/FLOAT2 vertices. These effect quads are
  admitted only with a proven declaration and strip/fan/quad topology.
  No semantic label such as lens flare is inferred from this helper alone.
- Effect-event beams pass through `0x00179e80` → `0x00156b60`; the sorted
  and direct beam render paths reach `0x00157020`, whose call at
  `0x0015743d` uses the same `0x0023ed30` emitter as particles. They share
  the particle decoder and admission without skipping the beam parents.
- Other effect builders and unsupported declarations remain D3D-owned.
  In particular, this does not claim native coverage of every handler
  reached through `0x00179880` (lens/MGS2/tdtl). Their draw ownership is not
  established by the researched leaf calls above.

## Bound admission contract

`boot::native_particles_glue::install(&device)` now installs a concrete
`ImmediateAdmission` host and `DrawAdmission` implementation. When the
switch is off it installs neither, avoiding extra D3D snapshot work.

The hooks only observe; six context-only points have no Rust continuation:
particle entry, publication (`0x0023ef77`) and epilogue (`0x0023ef79`), and
dynamic entry, post-builder/pre-publication (`0x0004b286`) and epilogue
(`0x0004b2a3`). At entry the decoder captures the producer inputs. Before
publication, the adapter reads the completed dynamic bytes and drains
only previously published commands, with no admission active. It then
arms exactly one packet. The epilogue drains that published packet and
unconditionally clears the scope. Allocation failure publishes nothing
and is cancelled. A mismatched first draw consumes the scope too: a later
identical quad cannot become owned. Pending interrupt callbacks remain
queued and refuse the scope. No shared sorted dispatcher is modified.

`DrawSnapshot::draw` supplies the actual declaration, streams, defaults,
shaders, constants, state and clipping. Particle corners and every effective
register must agree bitwise with the independent emission decoder.
Generated geometry must agree byte-for-byte with the completed callback
output, and its topology/count/declaration must agree independently.

A small pure helper in `d3d::nv2a::resolved` uses the existing translator's
program-key derivation and state rules for uniforms and samplers. It keeps
the final-combiner latch, inherited defaults, fog, alpha test, signed
channels, mip bias, W clipping and offset, stipple and per-copy window
shift. W clipping remains enabled even when the fragment does not write
depth. Combiner constants are uniforms, not pipeline-cache keys. Neither
this helper nor the native renderer writes the shared upload ring.

`ExternalTarget::stages` supplies the resident effective texture views,
including D3D's palette, mip, format and signed-channel handling. Each
copy gets immutable vertex/index/uniform buffers. `prepare_copy` prepares
that copy, then `record_ranges` records exactly one range into the supplied
isolated encoder. The handoff seals both reference/display recordings
before committing either; a refusal on either discards both and lets D3D
draw. The handoff also owns target-write, RTT and alias bookkeeping.

Explicit assumptions and limits:

- Publication points above belong to this executable's helper bodies;
  they are not a global next-shape matcher. Guest simulation writes at
  `0x00174420` and all material/property/builder work continue unchanged.
- Attachment/stage views are held by the handoff for the synchronous
  reservation. Native identities use the host device plus a unique call
  epoch and copy, rather than guessing a map or attachment generation.
  Prepared draws never survive that reservation. Pipeline caches contain
  no guest resource handles and are keyed by their shader/state contents.
- The seam does not expose a player-view ordinal. Diagnostics/counters use
  view bucket 4 (other); each draw's actual viewport, constants and scissor
  still come from its own snapshot. No stale per-view matrices are cached.
- Only programmable vertex shaders, one scissor rectangle, a depth-backed
  target and the researched vertex declarations are admitted. Shadow
  sampling, query-counted draws, glyph filtering, unknown shader behavior
  and missing sampled stages refuse. Integer uniform render scale and
  single-sample attachments are the handoff/backend contract here.
- No performance claim yet: this replaces D3D's GPU recording, retaining
  guest geometry construction and resolved-draw capture. Owned uploads
  prioritize cancellation correctness; arena optimization needs measured
  evidence. The feature's armed count is not replacement coverage.

## Diagnostics and coordinator checks

`H2_NATIVE_PARTICLES_COMPARE=N` enables read-only comparison for the first N
particle calls and first N recognized dynamic calls (empty means eight); after the budget, every remaining call
continues on D3D. Invalid/zero values disable comparison. The switch must
also be on. Require paired `geometry=Ok(())` lines from the concrete
admission adapter. Comparison consumes its scope and returns no native
replacement, so D3D continues drawing. This verifies geometry pairing,
not pixel parity. Native coverage comes from `H2_FRAME_STATS=1` counters:
`native_particles_suppressed` and positive reference/display range counts
in view bucket `other`; resource/range refusals must be understood.

Coordinator commands (not run by this lane):

```sh
cargo test -p native-render --no-default-features --lib particles::
cargo test -p native-render --no-default-features --lib effect_geometry::
cargo test -p native-render --test particles_gpu
cargo test -p feature --test native_particles
cargo test -p boot native_particles
cargo test -p d3d --test admission_scope
cargo test -p d3d --test external_pass
```

The GPU test uses invented microcode/geometry, draws two distinguishable
views (a canonical particle and a packed ribbon strip) into one command
submission and checks colour readback. It also
checks refusal of a changed attachment identity. It skips if no GPU
adapter is available; report that as a skip. Pure tests cover dynamic
register assignment, colour channel order, z/w reset, NaN/signed zero,
missing memory, alias/overflow refusal, clip/depth mismatch and malformed
geometry. The feature test requires six render-safe observer points and
none on
`0x00174420`.

Requested level runs after coordinator build, using the coordinator's
existing image and `h2 serve` plus `h2 agent` for control/capture:

- `H2_LEVEL=03a`, `H2_OPT=2`,
  `H2_FEATURES=impl.render.native_particles=on`,
  `H2_NATIVE_PARTICLES_COMPARE=64`. Compare the first 4,000 presents,
  including intro and street, collecting at presents 1,000/2,000/3,000/4,000.
  Include a contrail-bearing projectile and the four-vertex effect builder.
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


## Lane integration record

Shared-file edits made by this lane (apart from the handoff merge):
`crates/native-render/src/lib.rs`, `crates/feature/src/lib.rs`,
`crates/boot/src/lib.rs` (module entries),
`crates/boot/src/registration.rs` (feature registration),
`crates/boot/src/d3d_glue.rs` (one installation call),
`crates/d3d/src/device.rs` and `crates/d3d/src/nv2a.rs` (module entries
only). No sorted-transparent dispatcher changes. The shared native-renderer
document changes come from the handoff merge; lane notes live here.

The lane ran rustfmt checks on its new/already-formatted touched modules
and `git diff --check`. Cargo tests and runtime comparisons above are
unrun, for the coordinator. The two-copy transactional behavior is the
merged handoff contract, exercised by its `external_pass` tests; this lane
adds source-geometry pairing, raw strip upload/readback, final-latch/W-clip
and callback-preservation tests.
