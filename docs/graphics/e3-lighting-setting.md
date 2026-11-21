# The Lighting row on the D3D path

Status: Built. This page describes the scaled D3D backend. The native frame graph accepts the same setting and draws the game's retail light passes in both modes.
Code: `crates/platform/d3d/src/scaled.rs` (`Lighting`, `follow_lighting`, `shadow_scale`), `crates/formats/shaders/src/psh/shadow.rs` (`PixelOptions::hard_shadow`). Tests: `crates/platform/d3d/tests/render_scale.rs`, `crates/formats/shaders/tests/shadow.rs`.
Switch: `H2_LIGHTING` (`retail`), listed in [switches.md](../runtime/switches.md).

With the Video screen's Lighting row on E3, the scaled backend
([high-resolution.md](high-resolution.md)) draws the guest's own shadow
maps larger and compares them with one tap, in the shown image only. It
changes no other part of the picture: not the lights drawn, not object
shadows, not the lightmaps. The design for E3 lighting as a whole, on the
native renderer, is in [e3-lighting.md](e3-lighting.md). The scaled
shadow maps live on the path FrameScene removes
([framescene.md](framescene.md), section 11: `scaled.rs` goes in slice 10).
While the row is on E3 the native shadow stage stands down for the scaled
copies ([native-shadows.md](native-shadows.md), "Rules and limits"). No
further work lands here.

## How it works

Retail shadows are depth textures: the shadow-generate layer (10) draws the
casters into a depth level, and the shadow-apply layer (11) samples it as
`LIN_D24S8` with the NV2A's hardware comparison and a 2x2 percentage-closer
filter ([e3-lighting.md](e3-lighting.md), "Phase 0 findings"). E3 lighting makes that edge hard and sharp in
the view only. It runs on the scaled backend ([high-resolution.md](high-resolution.md)):
a 1x reference answers every read the guest makes and a scaled view draws what
is shown, and in-place depth sampling (`Backend::depth_level_texture`,
`gfx::depth_bits`; [d3d.md](d3d.md) "Lazy depth levels") means a depth level
is sampled with no read-back. The shadow comparison indexes texels by the
bound texture's real size (`textureDimensions`) and takes the coordinate in
the guest's size (`tex_scale`), so a larger map needs no change there.

- **Scaled shadow maps in the view** (`d3d::scaled`). Every `D24S8` depth
  surface not of the frame's size is a shadow level. With E3 lighting the
  view keeps a copy of it at `shadow_scale` times its size, made at first
  use, with a colour surface of that size to draw through. A draw that writes
  the level into a guest-size target (layer 10) is drawn again into the copy
  with colour writes off (`ScaleStats::shadow_draws`); a clear of it is
  repeated; a draw that samples it (layer 11) gets a texture of the copy
  (`shadow_textures`) instead of the reference's 1x one. What the view cannot
  repeat (guest bytes, a copy, a fill from the alias mirror, a draw it cannot
  make) marks the copy stale per aspect; a stale copy is filled from the
  reference (each texel repeated, `shadow_refills`) at its next draw, or
  skipped for sampling (the 1x texture stands in), unless a clear makes it
  fresh first. Halo's shadow pass clears the map before it draws, so the
  mirror refill at each binding costs nothing in the view.
- **The factor** is tied to the render scale, at least 2
  (`d3d::scaled::shadow_scale`: 2, 2, 3, 4, 5, 6 for scales 1 to 6; 1 with
  retail lighting). At the render scale a shadow texel covers no more host
  pixels than it covers guest pixels at 1x, so a one-tap edge steps no coarser
  on screen than retail's filtered one; the minimum of 2 makes E3 sharper than
  retail at 640x480 too. One setting and one rescale point.
- **One comparison tap** (`shaders::psh::PixelOptions::hard_shadow`, set by
  `Nv2aShaders` only when the view is told `set_hard_shadows`). A hard stage
  reads the nearest texel and compares once: no 2x2 filter. If a pass samples
  one map on several stages (taps the combiners average), each compares at
  the mean of their coordinates, so the average is one tap too. Whether
  retail's layer 11 does that is not known (S); the code covers both.
  Retail's translation is unchanged (`retail_shadow_lookups_are_unchanged`
  compares against a golden file).
- **At render scale 1** the view sleeps with retail lighting. With E3 it is
  awake: the frame at 1x, the shadow maps at 2x.
- **Live**: `ScaledBackend::follow_lighting` reads the row at every present
  and changes there, as the scale does. Copies at another factor are dropped
  at that boundary and made again at their next use (`lighting_changes_at_a_frame_boundary`).
- **The guest sees retail**: the reference keeps every level at 1x and answers
  every read (`read_depth`, `read_surface`, counts, the judge), so game
  memory, read-backs and the simulation are the 1x run's.

## Tests

No game run: `crates/formats/shaders/tests/shadow.rs` (retail and E3 golden
WGSL; one tap on the interpreter and the GPU; the mean of two stages over one
map) and the E3 section of `crates/platform/d3d/tests/render_scale.rs`:
`e3_shadow_maps_scale_in_the_view_and_the_guest_reads_retail_bytes`,
`retail_lighting_leaves_shadow_maps_to_the_reference`,
`e3_shadows_are_one_tap_and_follow_the_caster_closer`,
`lighting_changes_at_a_frame_boundary`,
`e3_shadow_maps_survive_halos_render_target_pool` (the order Halo uses: shadow
pass, a colour level over the map's bytes, swap),
`the_lighting_ab_draws_retail_and_e3_from_one_run` and
`a_frame_covered_by_a_small_level_shows_no_lighting_difference`.

## Diagnostics

The run report's `render scale` line ends with `shadow maps
at Kx: D draws, T textures, R refills`. In a level with shadows, `T` above 0
is the scaled map being sampled; `D` above 0 with `T` at 0 means the level went
through guest memory and the view sampled the 1x upload, still one tap. The
draw diagnostics list frames that drew with a depth texture bound
(`shadow-map frames K  a-b, c, ...`) and `H2_BOOT_GFX_SHADOWS=N` writes the
first N lit ones. Two headless runs are not the same run this deep (polled
visibility tests answer by host timing), so the comparison is made inside one
run: `H2_BOOT_GFX_LIGHTING_AB=1` (`d3d::LightingAb`) puts a second view over
the scaled backend; the inner view draws retail and the outer draws E3 from
the same calls, and each captured frame is written as `frame-N-retail.png`
and `frame-N-e3.png`. The report ends with `lighting A/B: K of M shadow-map
frames differ between the views (first: ...)`. Results of such runs are in
[notes/e3-lighting.md](notes/e3-lighting.md).

## Rules and limits

Not built, and not planned: a separate shadow-map factor; float depth levels (`F24S8`, `F16`)
and `D16` maps stay 1x (their texture comes through guest memory); a per-axis
scale. The setting does not touch the lightmaps (layer 3), so the outdoor
shadows baked into a level are unchanged.

## Related

[e3-lighting.md](e3-lighting.md), [high-resolution.md](high-resolution.md),
[d3d.md](d3d.md), [settings-menu.md](../game/settings-menu.md),
[notes/e3-lighting.md](notes/e3-lighting.md).
