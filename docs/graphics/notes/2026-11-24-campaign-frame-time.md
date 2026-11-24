# Campaign frame time and retained render state

Evidence for [native-renderer.md](../native-renderer.md) and
[graphics.md](../graphics.md).

## 2026-11-24

**Run.** The reported playthrough used `7be057c72` (overlay
`7be057c master 21:04 UTC`) on the user's M3 Pro at 2560×1920 internal
resolution. On `05b_deltatowers`, CPU time measured 58–73 ms and GPU
time measured 23–31 ms in the two reported samples. The rate fell from
about 30 fps to 10.8 fps at present 17,900. The memory watchdog reported
4.45–4.75 GB between roughly presents 7,400 and 8,300.

**Method.** The run used `h2 play disc/halo2.iso --live` at the clang
tier, with the Famine skull. Reproduce with the same settings and
compare the frame overlay, present heartbeat and memory watchdog over
the same campaign route. The source audit compared the play commit
with `332125cde`, 14 first-parent commits later. The lane made no new
timing or memory measurements. The decal counts are covered by a
separate investigation.

**Code.** The transparent log advances in increments of 1,800 accepted
packed bindings across the run. The ordered indexed path built repack
plans for both shader and buffer setup, gathered scissors into a
temporary vector, and cloned buffer and bind-group vectors for
recording. The fallback packer could allocate the interleaved bytes
before checking its budget. These paths still existed on `332125cde`.
`2bdaeef9e` widened the indexed hook to first-person subranges and
preserved the same setup path. See `native_transparents_glue.rs`,
`transparent_streams.rs`, `world_layers_gpu.rs` and `gfx/renderer.rs`.

**Code.** The 19,726 `VisibilityQuery` entries were cumulative calls
to a diagnostic hook. That hook checked a rate limiter on every call.
The query runs on D3D. Particle preparation also rebuilt string keys
and copied geometry and microcode between copies of a draw. Particle
program, pipeline and sampler tables had no eviction. The relevant
effect and particle sources were identical in both commits. See
`native_effects.rs`, `native_particles_admission.rs`, and `particle_gpu.rs`.

**Code.** The light line counted 6 accepted plans in 1,800 cumulative
decisions. The sorted fallback path decoded the rest of the list and
drained D3D before checking whether a gathered model or a missing
cluster made the plan ineligible. The scene path built a redundant
draw plan, and reloaded material variants left dead weak references
in the live edit registry. These paths still existed on `332125cde`.
See `native_lights_glue.rs`, `native_lights_glue/materials.rs`, and
`light_list.rs`.

**Code.** Indexed world sections retained windows even after their key
stopped appearing. Snapshot-buffer eviction did not release the
buffers pinned by those sections. Draw diagnostics kept timestamps
and shadow-frame numbers throughout the run. Render-census totals
and sticky pages survived map activations when the optional access
census was armed. These paths were unchanged across the two commits.
See `world_layers_gpu.rs`, `d3d_diag.rs`, and `render_census.rs`.

**Code.** The shared upload pool retained every returned arena, and
`ProgramBindings` allocated a uniform byte copy whenever constants
changed. Unbounded native pipeline variants and the archive backlog
also persisted. The later HUD, post and frame-tail commits routed
more draws to native code. The same UI and screen caches remained,
and `2bdaeef9e` expanded the glow pipeline key with live color state.
See `gfx/ring.rs`, `gfx/program.rs`, `pipelines.rs`, `ui_gpu.rs`,
`glow.rs`, and `screen_gpu.rs`.

**Code.** Theater hashes gameplay state on recording ticks and captures
an initial and periodic keyframe. The reported `tape call too large`
error is the tape codec's argument or output-count check. At tick
1,877 the recording failure released the RAM anchor and stopped
subsequent samples. The failed batch remains queued and can make the
final tape cut fail again. The recording and codec files relevant to
this failure were unchanged across the two commits. See
`game/src/theater.rs`, `game/src/theater/record.rs`, and
`film/src/tape/codec.rs`.
