# Retiring D3D: R1's remainder

Status: Partly built. What R1's stages still leave to the guest's draws.
Code: see each section. Tests: named per section.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

A slice of [retire-d3d.md](retire-d3d.md), split from it by topic.

## R1's remainder

Every R1 layer is `partial` (`residual_census::PARTIAL_LAYERS`): its
stages skip the guest draws of the items they take, and the guest's layer
draw `0x00044550` still runs for all of them (kind 2 setup included). What
keeps a guest draw, by the counter that names it (**Code**):

- **The whole-call replacement.** framescene.md slice 4's
  `render_bridge::frame_step` (the layer draws of 1, 3, 5, 7, 8 and 20
  returning after the writes of 5.2) is not built, and the mask plans it
  deletes are the mechanism every stage below uses. 8.2's rule 1 holds
  for no entry.
- **Layers 1, 5, 7, 8 and 20** (the scene's, the alternate view's 1 and
  5, the first person's 1, 7, 8 and 20; `layer_plan_<layer>_refused_*`,
  `_fp<layer>_*`, `_alt<layer>_*`). Structure items: the plan's causes
  (`material`, `instance`, `no_pass`, `pass`, `implementation`, `input`,
  `animation`, `texture_transform`, `object_animation`, `property`,
  `wave`, `material_c18`, `time`, `per_draw`), `program`, `palettes`,
  `state`, `stencil`, `warming`, `device`, `viewport`, `unread`. Object
  items: extraction's causes (`object_shader_flag`: the shader's flag
  bit 0 where the upload of the active structure's parameter block
  (`0x0003c650`, `0x0003d000`) does not compute or the device and the
  engine do not already hold it, since skipping the draw then leaves
  them unset, or in layer 3 a structure lightmap index the guest's draw
  leaves unbound; `object_layer5_stream`, `object_subparts`,
  `object_aux_state`, `object_unread`), then
  `object_models`, `object_w_buffer`, `object_model`, `object_plan_<cause>`
  (`not_compiled`: a pass's program reads a constant or vertex register
  the object draw does not give where its value reaches a live output,
  such as the PRT row `c[69]` outside layer 5, an animated property
  driven by an object function, an engine wave before the game's tables
  are installed (`Tables::install_engine_slab`), or a
  `dot_rflct_spec` stage bound a 2D bitmap where it needs a cube;
  [native-objects.md](native-objects.md), "Object programs"),
  `object_shader_flag_target` (the shader's flag bit 0 before the
  frame's structure target copy, where a pass's program has flag bit 20:
  its draw calls `0x0003bea0`, which clears the shader cache and, with a
  structure parameter block, makes the copy, and no native stage does
  either; `object_program_gpu::makes_target_copy`),
  `object_texaccum` (layer 1, or no target or level),
  `object_aux_depth` (the device does not hold the toggle off),
  `object_fade` (a fade byte with the device's polygon stipple on, which
  the native object draw does not apply: layers 1 and 3 outside view
  kinds 4 to 7), `object_stipple` (a fade byte where the engine's cached
  fade or the device's pattern is not what the epilogue leaves),
  `object_state`, `object_prt`, `object_fog`, `object_stencil`,
  `object_warming`. And `order`: the native pass draws before the
  guest's layer draw, so an item after a kept one is taken only when
  both are order-free and every kept item before it restores what it
  sets (`layer_pass::clear_layer_items`, `order_free`); a blended,
  depth-equal or stencil-counting item, or one after a kept item of
  unknown effect, keeps its guest draw.
- **Layer 3's world** keeps the parts whose material pass does not
  translate or that lack an input of their program
  ([native-world.md](native-world.md)).
- **Layer 3's objects** (`native_objects_walk_*`; the refused parts by
  cause in `walk_part_{aux_state,subparts,shader_flag,argument,program,layout}`,
  [native-objects.md](native-objects.md)): the objects the walk
  returns (`walk_unsupported`: a bit-11 part whose toggle does not read,
  a bit-6 part with a negative first subpart or a mask that does not read,
  a flagged shader with the structure's parameter block or a structure
  lightmap index, an item's `+0x14` or `+0x15`), and a run declined by
  `Model`, `Inputs`, `State` (among its causes the device's stipple on,
  and a faded part where the device does not hold what its epilogue
  leaves), `Unresident` or `Reservation`.
- **Layer 3's decorators** (`scene_decorator_draws_unready`,
  `_draws_state`, `_views_declined`, `_remainder`): a material not ready,
  a pass with an engine-driven state from a per-draw source or a
  lightmap stage whose globals bitmap is not uniform, a skip that would
  change a render state, and a view whose W mode, stage palettes,
  stencil or inherited cull do not read or hold
  ([native-decorators.md](native-decorators.md)).
- **The first person's layer 3** (`native-first-person:` log reasons):
  mixed-list sorting, a part without a compiled pass, a record marking
  the stencil, a fade byte with the device's stipple on or not at rest,
  a flagged shader with the structure's parameter block, a structure
  lightmap index, or a program with flag bit 20 and no target copy yet,
  another pass or level in the
  item, a record toggling the auxiliary state while the device does not
  hold it off,
  and a part sampling the texture accumulation target where none is
  bound or levels are not sampled in place
  ([native-objects.md](native-objects.md)).

R1, R2 and R6 do not depend on each other and can run as three lanes
after R0. framescene.md slice 8 (60 and 120 fps from two scenes) is
independent of all of them once its slice 6 holds.

**R10 deletes**: `pushbuf.rs`, `state.rs`, `nv2a.rs`, `nv2a_draw.rs`,
`nv2a_hot.rs`, `shader.rs`, `vertex_cache.rs`, `dirty.rs`, `regs.rs`,
`glyph.rs`, `pipeline_store.rs`, `device/` except what R11 lists,
`backend.rs`, `gfx_backend.rs` and `gfx_backend/`, `scaled.rs` and
`scaled/`, `threaded.rs` and `threaded/`, `scene_sink.rs` and
`scene_sink/` (with `retarget.rs`, moved to the native side first),
`scene_stage.rs`, `clip_space/`; `DrawAdmission` and every `render_bridge::native_*` hook;
`frame_types::carried` (the `frame-types` crate's carried part); in `gfx`, what only a resolved D3D draw uses
(`state.rs`, `translate.rs`, `xdk.rs`, the D3D pipeline key); the
`render-timing` and `render-census` features and `render.parity`.
`formats/shaders` stays: the native stages translate tag shaders with it.

**R11 deletes** `abi.rs`, `runtime_glue.rs`, `resource.rs`, `layout.rs`,
`guest.rs`, `hw.rs`, `vblank.rs`, `events.rs`, the crate, its dependents'
edges (`boot`, `game`, `cli`, `h2host`, `uwp-host`, `tools/perf`),
`impl.render.native`, and [d3d.md](d3d.md)'s device model. The entry
inventory in d3d.md is a fact about the retail image and stays as a
reference page.


## Related

- [retire-d3d.md](retire-d3d.md): the plan and the other slices.
