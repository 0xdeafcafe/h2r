# Agent API: render checks and residual work

Status: Built. Part of the [agent API](agent-api.md); the other `render.*` methods are in [agent-api-render.md](agent-api-render.md).

### FrameScene checks: compare, capture, scene, parity, hashes

The checks the native renderer's slices gate on
([framescene.md](../graphics/framescene.md), sections 9 and 12). Host
methods; types in `crates/extensions/agent-proto/src/render.rs` and
`crates/extensions/agent-proto/src/sim/hashes.rs`, logic in
`crates/extensions/agent/src/compare.rs`, `crates/extensions/agent/src/render.rs` and
`crates/extensions/agent/src/sim/hashes.rs`, host glue in boot's `agent_glue`,
`agent_frames`, `d3d_diag`, `d3d_glue` and `scene_glue`. Tested in
`crates/extensions/agent` over synthetic images, the frame recorder's rig and the
sim report's tally.

```sh
h2 agent frame compare off/frame-03000.png on/frame-03000.png --diff d.png  # no game
h2 agent render.spawn_view '{"index": 0}'
h2 agent render.capture '{}'
h2 agent render.spawn_view '{"reset": true}'
h2 agent render.view '{"eye": [12.5, -3, 2.2], "yaw": 1.57, "pitch": -0.2, "fov": 70}'
h2 agent render.capture '{}'
h2 agent render.view '{"reset": true}'
h2 agent render.capture '{"tick": 9000}'
h2 agent render.capture '{"pass": 3}'          # the next tick's frame, pass 3
h2 agent render.capture '{"lighting":"retail"}' # latest sealed scene, private draw
h2 agent render.capture '{"lighting":"e3","tick":42}' # use the first reply's tick
h2 agent render.capture '{"lighting":"relight-off","tick":42}' # installed lamp, same scene
h2 agent render.scene '{"dump": true}'
h2 agent sim.hashes '{"from_tick": 3000}'
h2 agent render.parity '{"arm": true}'
```

| Method | Access | Params | Result |
|---|---|---|---|
| `render.compare` | read | `{a, b, diff?}`: two PNG paths, and where to write a difference image | `{size: [w, h], differing, max_channel, mean, first: [x, y] \| null, within_bar, diff?}` |
| `render.spawn_view` | act | `{index, yaw_offset?}` or `{reset: true}` | `{index, count, position: [x,y,z], yaw}` or `{reset: true}` |
| `render.view` | act | `{eye: [x,y,z], yaw, pitch, fov?}` or `{reset: true}` | `{eye, yaw, pitch, fov}` or `{reset: true}` |
| `render.view.current` | read | `{seat?}` | `{eye: [x,y,z], yaw, pitch, fov, private?}` |
| `render.capture` | read | `{tick?, pass?, lighting?, game_id?}` | `{path, tick, present, pass?, lighting?, scene_hash, source?}` |
| `render.scene` | read | `{dump?}` | `{sealed, map_epoch, frame, tick, hash, cut, views, counters, entries?, carried?, carried_census?, carried_activation?, carried_checkpoint?, visibility_first_mismatch?, light_visibility_first_mismatch?, first_person_visibility_first_mismatch?, view_record_first_mismatch?, dump?, display_frame?, private_frame?}` |
| `render.parity` | act | `{arm: true}` | `render.compare`'s fields plus `tick` |
| `sim.hashes` | read | `{from_tick?, max?}` | `{rows: [{tick, gameplay, arena?}], dropped, resets, truncated?}` |

- **`render.compare`.** Reads both files (any PNG colour type, decoded to
  8-bit RGBA) and compares the colour channels; alpha is not compared
  (frames are written opaque). `differing` counts pixels with any channel
  off by more than 2; `max_channel` is the largest difference of one
  channel; `mean` the mean absolute difference over every channel of
  every pixel; `first` the first differing pixel in row order, `null`
  when none. `within_bar` is the parity bar: `max_channel <= 8` and
  `differing` at most 0.01% of the pixels (one in 10,000). With `diff`,
  the difference image goes there (directories made): differing pixels
  in red, brighter the further off; pixels off by 1 or 2 in dark blue;
  the rest the first image at a quarter brightness. Errors are
  `INVALID_PARAMS` with `data.error` naming the kind: `size_mismatch`
  (with `data.a` and `data.b`, each `[w, h]`), `read` or `write` (with
  `data.path`). The decode runs on the game thread; `h2 agent frame
  compare A B [--diff D]` runs the same function in the CLI with no
  socket and exits 1 outside the bar.
- **`render.spawn_view`.** Selects a player starting location from the
  loaded scenario by its zero-based index. The eye is 0.64 world units
  above the start, looking along its facing. `yaw_offset` turns it in
  radians. The reply includes the number of starts and the eye position
  and heading used for the picture. The method answers when the world
  renderer accepts the private view. A following `render.capture {}`
  takes the next tick's frame. The camera stays selected until reset or
  the next game activation. A new selection interrupts a pending request.
  An invalid index gives `INVALID_PARAMS`, a missing or unreadable start
  gives `UNAVAILABLE`, and a session that draws no world frames times out.
  `crates/extensions/game/src/agent_spawn_view.rs` shares Theater's
  playerless world view in `spectator_view.rs`.
- **`render.view`.** Places the same private camera at an explicit eye
  in world units. `yaw` is the heading about +z in radians, from +x
  towards +y; `pitch` is up from level, within ±1.55. `fov` is the
  horizontal lens in degrees, 1 to 150; without it the view keeps the
  drawn view's own lens and the reply gives `fov: null`. The reply
  echoes the pose drawn, with `yaw` wrapped into 0 to 2π. It answers
  when the world renderer accepts the view, and a following
  `render.capture {}` takes the next tick's frame from that eye. The
  camera is drawn only while the scenario it was placed in is loaded.
  `render.view` and `render.spawn_view` select one camera: either
  replaces the other, and a pending request answers `INTERRUPTED` when
  it is replaced, reset (by either method) or released by the next game
  activation. Missing, non-finite or out-of-range params, or `reset`
  with a pose, give `INVALID_PARAMS`; no loaded scenario gives
  `UNAVAILABLE`. Types: `ViewParams` and its `check` in
  `crates/extensions/agent-proto/src/render.rs`.
- **`render.view.current`.** Reads the pose of the view the game draws for
  local player `seat` (0 to 3, default 0), at the tick boundary, in
  `render.view`'s conventions: `eye` in world units, `yaw` the heading
  about +z from +x towards +y wrapped into 0 to 2π, `pitch` up positive
  (within ±1.55), `fov` the horizontal lens in degrees. The reply also
  carries the usual `at` stamp. A user who sees a rendering fault in their
  own game gives the coordinator this pose, and `render.view` with its
  `eye`, `yaw`, `pitch` and `fov` (not `at` or `private`) draws the same
  view in a test boot. The pose comes from the seat's view record in the
  game's view table (`spectator_view::drawn_view`: the render camera's
  position, forward and vertical lens, the lens converted to horizontal);
  yaw and pitch derive from the forward vector (`CurrentView::looking`).
  While a `render.view` or `render.spawn_view` camera is selected and its
  scenario is loaded, the reply is that camera's pose with `private: true`
  (its lens is the one given, else the game view's); `private` is absent
  otherwise. No view record for the seat (no world loaded, a cinematic,
  a seat without a screen) gives `UNAVAILABLE`; a `seat` outside 0 to 3
  or unknown params give `INVALID_PARAMS`. A Theater camera is not
  reported: the reply is the underlying game view. Types:
  `CurrentViewParams` and `CurrentView` in
  `crates/extensions/agent-proto/src/render.rs`, the method in
  `crates/extensions/game/src/agent_spawn_view.rs`. The private camera is a spectator view: a pose replayed from the player's eye also draws that player's own body, which can cover part of the picture.
- **`render.capture`.** With `lighting: "retail" | "e3" | "relight-on" |
  "relight-off"`, the native
  renderer draws a sealed scene into a private target. The default is
  the latest sealed scene. An explicit `tick` selects that exact scene
  tick, up to 240 ticks ahead of the latest seal. The draw uses the
  resolution shown for that particular present, including a frame the
  backend showed from its 1x reference, on the display's own grid when
  `RenderScale::replayable_display` accepts it (3440x1440 and 5120x1440
  included; `docs/graphics/high-resolution.md`). The answer includes the tick,
  present, summary hash, lighting and a fresh PNG path. The most recent
  successful private capture retains its scene for another request with
  the same tick. A failed original Swap gives `UNAVAILABLE` with
  `reference present failed` for that scene. Other retained ticks stay
  available. Pass both paths to `render.compare` to compare the modes
  while play continues.
  A renderer reset releases the retained scene. A missing private
  renderer or an unreplayable scene gives `UNAVAILABLE`. Combining
  `lighting` with `pass` gives `INVALID_PARAMS`. The override applies
  only to the private draw. The relight modes require a run started with
  either reconstruction selected and a matching installed bake. Each
  successfully recorded world stage counts selected ranges that bind a page
  whose baked states differ.
  Zero such ranges across the whole frame gives `UNAVAILABLE` with
  `no native world range in this scene uses the installed lightmap`.
  `lighting` in a successful reply echoes the requested state.
  See the [three-render comparison](../graphics/relighting-review.md).
  Tests: `private_captures_draw_the_same_pinned_scene_with_each_requested_mode`
  and `retained_stage_uses_the_current_private_attachments_on_every_draw`.

  Without `lighting`, the frame is taken for game tick `tick` (as `at.tick`
  counts it; default the next one, at most 240 ahead). On the D3D path
  the frame of a tick is the present that follows it: the first guest
  present after the tick has ended. `tick` in the answer is the last tick
  that ended before that present, which is later than the one asked for
  when the game ran two ticks in one frame; `present` numbers it as
  `render.frame` and `H2_BOOT_GFX`'s `frame-N.png` do. Without `pass`,
  the image is the presented frame at the render scale, the same image
  `H2_BOOT_GFX` writes as `frame-N.png`, written as
  `frames/capture-<pid>-<seq>-tick-<tick>.png` under the cache directory. That
  needs a run that renders on the GPU with `H2_BOOT_GFX` (use
  `H2_BOOT_GFX_EVERY=never` to write nothing else); without it the answer
  is `UNAVAILABLE`, and under `H2_BOOT_GFX_LIGHTING_AB` too. With `pass`,
  the frame recorder (`render.frame`) is armed at the end of the tick
  before and reads that pass back at 1x as `render.frame`'s `snap` does
  (the file is the snapshot's); it replaces a `render.frame` arming not
  yet done, and one made while it waits makes it answer `INTERRUPTED`. `scene_hash` is the sealed scene's
  summary hash at that present (from `render.scene`'s source); `null`
  with no source registered. The answer waits for the
  frame (up to the agent's 300-tick limit for deferred answers); a
  paused game holds it. A new `render.capture` replaces one still waiting,
  which answers `INTERRUPTED`. Unasked, the cost is one relaxed load per
  tick and per present. Capture does not wait for unresident scene
  content (framescene.md 9.2).
  For a Live gallery capture, pass the capture session's `game_id` after
  selecting a spawn view. The method waits for a whole frame that draws
  that view at or after the requested tick. The result includes
  `source: {game_id, map_id, map, camera, digest}`. The game resolves the
  draw's cache alias to the installed package's full `MapId` and base map name.
  `camera` gives the world-space `position`, `forward` and `up` the renderer
  uses. `digest` is the SHA-256 of the exact PNG the display writer saves.
  The Live CLI adds the capture permit when it submits the image.
  `game_id` cannot be combined with `pass` or `lighting`.
- **`render.scene`.** Map activation or snapshot load clears the last
  scene and any private capture pin immediately. `sealed` stays false
  until a frame on the new timeline seals. The last sealed `FrameScene`
  comes from the source the frame step registers (`agent::render::set_scene_source`, in boot's
  `scene_glue` over `render_bridge::scene_step`): whether
  one has been sealed, its map epoch, frame, tick, summary hash and cut
  reasons; per view (`key` is `kind:player:view`, `game:0:0`, the first
  person's `first_person:0:0`) whether it
  cut, its clusters, instances, parts, rows, guest-selected lights
  (`lights`), all light candidates including those selections
  (`lights_candidates`), candidates with both constant sets and any
  required projection (`lights_with_constants`), shadow maps (its
  lights that cast their shadow), its lights by each layer their passes
  draw (`light_layers`, `"10"` to `"14"`), the items of its typed layer
  draws by layer (`layer_items`) and those the guest still draws
  (`layer_remainder`), the
  instances the guest still draws (`remainder`), its parts by how they
  draw beyond their material (`part_flags`: `stencil_mark`, `strip`,
  `subparts`, `aux_depth`, `fade`, `shader_flag`,
  `structure_parameters`, `target_uncopied`;
  framescene.md, slice 3), its model decorator draws (one per
  placement), their instances and the draws the guest still draws
  (`decorators`, `decorator_instances`, `decorator_remainder`;
  native-decorators.md), its late passes when they ran (framescene.md,
  slice 5): `fog` (layer 16) and `water` (layer 6), each `{draws, items,
  materials, parts, drawn, kept, device_words}` (whether the layer's own
  gate held, its items in the list, the materials and parts the native
  stage draws, whether the stage took the layer, the items the guest
  still draws, the device fog words of 7 that read), and `post`,
  `{colour, late, view_kind, screen_effects, screen_effects_gated,
  colour_pass, effects}` (`render_screen_effects`' arguments and gates,
  whether the colour pass ran, the screen effects drawn under it); each
  absent when its draw did not run in the view; `carried`, its ops by
  slot from the scene sink's last frame (empty without
  `frame-scene.sink`, and for a first-person view), and `camera_use`
  counts the retained guest draws for that scene and game view,
  including its overlay. `screen` is pretransformed or passes screen
  positions through, `view` has the view's verified projection block
  and viewport, and `unknown` covers the other draws. The map is
  empty for other view kinds or without a retained matching scene.
  For a game view, `visibility_clusters` counts the visible clusters,
  `structure_clusters` the subsequent BSP structure list and
  `visibility_objects` the view's object list; `visibility_source` is
  `rust` when the Rust visibility supplied the clusters and objects (the
  default) and `guest` when it declined for the view
  ([native-visibility.md](../graphics/native-visibility.md)). All four
  are absent if extraction cannot read the main gather at layer 3.
  A zero is a successfully read empty list. With `{"dump":true}`, the scene
  file has the BSP cluster IDs in `cluster_gathers[i].visible`, the
  object handles in `.objects` (the guest's order, or ascending when
  `.source` is `rust`) and `.structure` for `views[i]`. Its `mode` is 0
  for a portal flood, 1 for a sphere and 2 for the potentially visible
  set. The view's `clusters` range selects the
  layer-3 IDs from the top-level `clusters` array. `views[i].camera` gives
  the vertical FOV in radians, guest projection terms and viewport.
  `visibility_first_mismatch` names the first game view since the map
  loaded whose Rust visibility differed from the guest's: its key, frame
  and tick, the first difference (`cluster 12 guest only`, `object
  0x00050002 rust only`) and the counts each way.
  `light_visibility_first_mismatch` does the same for the first light
  whose Rust visibility differed from its struct's lists.
  `first_person_visibility_first_mismatch` identifies the first differing
  first-person slot count, handle, record count or record in replay order.
  Its counters are `scene_first_person_visibility_checked`, `_equal`,
  `_differ`, `_declined_unread` and `_declined_layout`.

  `view_record_first_mismatch` names the first game view whose
  Rust-built records differed from the guest's and the first field that
  did (`position`, `forward`, `up`, `fov`, `viewport`, `near`, `far`,
  `projection`, `start cluster`, `mode`).
  The top-level `counters` include
  the visibility differential's `scene_visibility_checked` (sampled game
  views whose Rust sets were made and compared), `scene_visibility_equal`,
  `scene_visibility_differ`, the shadow arms'
  `scene_visibility_<arm>_matched`, `_differ`, `_answered`, `_guest`,
  `_declined`, `_proven` and `_fallen` for `clusters`, `frusta` and
  `objects` ([native-visibility.md](../graphics/native-visibility.md#the-shadow-arms)),
  `scene_visibility_rust_taken` (views that carry the Rust sets), `scene_visibility_ns` (time in the Rust
  computation), `scene_visibility_lens_retail`, `_guest` and `_display`
  (game views by the lens their visibility used: retail, the single-copy
  camera that `VIEW_PROJECTION_TERMS` widened, or the display supplement;
  a single-copy run whose views count under `_display` has views the hook
  did not fit), `scene_visibility_display_views`, `_clusters` and
  `_objects` (what the display supplement added),
  `scene_visibility_display_incomplete` (display-only clusters left out
  whole because a part draws in a layer the supplement has no display
  draw for) and
  `scene_visibility_declined_<reason>` (`unread`,
  `layout`, `mode`, `frusta`, `overflow`, `camera`, `not_main`: views it
  made no sets for); the light views' `scene_light_visibility_checked`,
  `_equal`, `_differ`, `_ns` and `_declined_<reason>` (the same reasons);
  the complete world/model geometry lists' `scene_geometry_lists_equal`,
  `scene_geometry_lists_differ` and `scene_geometry_lists_taken` (whole lists
  dispatched from a verified Rust result for the current inputs);
  and the view records' `scene_view_record_checked`, `_equal`, `_differ`,
  `_guest_lens_applied`, `_guest_lens_retail` and `_guest_lens_other`
  (single-copy views the hook fitted, by whether the guest's own
  horizontal projection scale is the widened one, the retail one, or
  neither) and `_declined_<reason>` (`unread`, `layout`, `cinematic`,
  `no_observer`, `override`). They also include
  (`scenes_sealed`, `scenes_unsealed`, `scene_views`, `scene_no_camera`,
  `scene_refused_writes`, `scene_layers`, `scene_layer_items`,
  `scene_layer_remainder`, `scene_layers_unread` (layer draws whose list
  did not read), `scene_decorator_draws`, `scene_decorator_instances`,
  `scene_decorator_remainder`, and the decorator placement source by view
  (`scene_decorator_placements_equal`, `_differ`, `_no_windows`, `_unread`,
  `_nonresident`, `_virtual_shader`). The grid producer reports
  `scene_decorator_grid_compared`, `_differ` and `_native`.
  The decorator stage reports
  `scene_decorator_views`, `scene_decorator_views_native`,
  `scene_decorator_items_skipped`, `scene_decorator_draws_unready`,
  `scene_decorator_draws_state` and `scene_decorator_views_declined`
  (native-decorators.md),
  `scene_light_extract_views`, `scene_light_extract_ns` and
  `scene_light_extract_unread` (candidate extraction attempts, their
  elapsed nanoseconds and attempts that return no candidates because
  the input could not be read), `scene_light_groups`, `scene_light_groups_whole` (the
  gathered light and shadow layers in the scene, and those with every
  sorted entry in it), `scene_light_layers_alone` (light layers drawn
  from the scene with no handler run), `scene_light_objects_alone`
  (their object entries: model parts drawn with no handler),
  `scene_first_person_light_layers_alone` (of those layers, the
  first-person lights'),
  `scene_host_lights_drawn`, `scene_host_lights_kept` and
  `scene_host_lights_unread` (the host's own lights, ML5, drawn by the
  light passes, made but kept, and whose constants did not read),
  `scene_e3_lights_drawn`, `scene_e3_lights_kept`,
  `scene_e3_lights_unread` and `scene_e3_lights_shadowed` (E3's chosen
  lights beyond the guest's, queued as E3 stages, ready but not queued,
  with no constants to draw from, and handed to the host with their
  shadowed implementations over the renderer's cone shadow),
  `scene_e3_first_person_lights_drawn`, `scene_e3_first_person_lights_kept`,
  `scene_e3_first_person_lights_unread` and
  `scene_e3_first_person_lights_far` (E3's chosen first-person lights
  retail's first-person pass did not record: queued as E3 stages over the
  first-person parts, ready but not queued, with no constants, and whose
  sphere does not reach the eye), `scene_e3_first_person_parts` and
  `scene_e3_first_person_parts_left` (the first-person object parts
  offered with them, and those left out: subparts, auxiliary depth or no
  rows),
  `scene_light_entries` and
  `scene_light_entry_remainder_<cause>` (sorted entries left to the
  guest's handlers, by `scene_extract::light_groups::Cause`: `memory`,
  `callback`, `owner`, `section`, `range`, `state`, `subparts`,
  `material`, `pose`; framescene.md, slice 4),
  `scene_shadow_groups` and `scene_shadow_groups_whole` (the shadow
  groups and those with every sorted entry), also per layer as
  `scene_shadow_groups_<layer>` and `scene_shadow_groups_whole_<layer>`
  for 10, 11 and 14, `scene_shadow_layers_alone_10`, `_11` and `_14`
  (caster, receiver and opacity groups drawn from the scene with no
  handler), `scene_dropped_shadow_layers_<cause>` (`remainder`, `stream`,
  `pose`, `program`, `state`, `target`, `callback`, `device`),
  `scene_dropped_shadow_layers`, `scene_shadow_filters_native` (object
  filters replaced by native quads) and
  `scene_dropped_shadow_filter_<cause>` (`program`, `state`, `target`,
  `callback`, `device`), `scene_shadow_chains_private` (object masks
  drawn into a private chain) and `scene_shadow_chains_spilled` (of
  those, chains copied back to target 10 for the guest's filter)
  ([native-shadow-layers.md](../graphics/native-shadow-layers.md)),
  `scene_fog_views`, `scene_fog_gate_held`, `scene_fog_drawn`,
  `scene_fog_parts` and the same four for `scene_water_` (late layers
  written, of those whose gate held, those the native stage took, and
  the parts it took), `scene_post_views`, `scene_post_colour`,
  `scene_post_effects` and `scene_post_unplaced` (post calls whose words
  did not read, and colour passes or screen effects with no post call of
  their view; slice 5), and from `d3d::scene_stage`
  `scene_stages_<stage>`, the scene stages queued to the render thread,
  and `scene_dropped_<stage>`, those it could not record after the guest
  skipped their items, for `world` and `objects` (framescene.md, slice
  2) and `fog` and `water` (slice 5). The counters
  `layer_plan_{6,16}_refused_<cause>` count fog and water batch refusals.
  Item causes are `build`, `handler`, `record`, `object`, `subparts`, `material` and
  `order`. View causes include `stencil_view`, `palette_view`,
  `fog_words_view` and the water stage's `target_view`.
  The water loader counts refused placement parts by `placement_rows_load`,
  `rigid_program_load` and `placement_streams_load`.
  Sorted range admission reports `native_{fog,water}_walk_examined`,
  `walk_suppressed`, `walk_refused_inputs`, `walk_refused_resume` and
  `walk_refused_reservation` under each stage. The counters
  `scene_submitted_flares_reference_draws` and
  `scene_submitted_flares_display_draws` accumulate flare ranges as
  `GfxBackend::commit_external` appends them to each copy's command
  stream. Private replays also contribute. These counters are available
  with frame statistics and the residual census disabled. The corresponding
  `scene_submitted_sky_{reference,display}_draws` count committed sky strips.
  `sky_plan_admitted` counts strips offered to the device after extraction and
  material checks. `scene_kept_sky_<cause>` counts strips left to their guest
  draw, by cause ([native-world-walk.md](../graphics/native-world-walk.md)). GPU reservation and
  replay outcomes appear under `native_sky_*` in the frame counters.
  `scene_kept_ui_<kind>_<cause>` counts UI draws that do not draw from the
  scene's overlay, totals since start
  ([retire-d3d-menus.md](../graphics/retire-d3d-menus.md#what-is-kept)).
  `post_colour_calls`, `post_colour_glows`, `post_screen_effect_calls`,
  `post_screen_effect_ended` and `post_screen_effect_loading` count the post
  bodies; an uncovered call panics, `post: <cause>`.
  Unless
  `frame-scene.layers=off` is in `H2_FEATURES`, `layer_plan_asked`, `layer_plan_drawn`,
  `layer_plan_items` and the plan's outcomes by layer,
  `layer_plan_<layer>_taken` and `layer_plan_<layer>_refused_<cause>`,
  the alternate view's as `layer_plan_alt<layer>_*`, the first person's as `layer_plan_fp<layer>_*`, object items' causes
  named `object_*` (`render_bridge::native_layers`; framescene.md,
  slice 4), and the cut world layers' items left to their guest draw,
  `scene_kept_<site><layer>_<kind>_<cause>` (site empty or `alt`, kind
  `world`, `object` or `unreadable`; layers 1, 2, 3, 5, 6, 7, 8, 16, 17 and 20; causes in
  [native-world-walk.md](../graphics/native-world-walk.md)); a later slice adds
  `scene_held_draws`). With the [scene sink](../graphics/framescene.md#slice-6-the-renderer-owns-game-view-frames)
  enabled, the device files
  each guest D3D op by the frame's slot (`opaque`, `after_opaque`,
  `after_lights`, `transparents`, `after_post`, `overlay`, `outside`)
  for both the original renderer and the frame owner. `carried` describes
  the last recorded frame:
  `slots`, each slot's `{draws, binds, clears, copies, uploads,
  samples}` (slots with none left out), `side_targets`, each surface
  other than the scene's presented colour and its depth that a game
  view's ops name, `{aux, width, height, format, depth, ops}` (`ops` by
  slot), and `readbacks` and `readbacks_outside`, the guest's reads of
  what was drawn in and outside a game view by kind (`surface`,
  `depth`, `mirror`, `pixel_count`); and the counters add
  `scene_carried_frames`, `scene_carried_draws`, `scene_carried_ops`,
  `scene_side_target_frames` (frames that named a side target in a
  game view), `scene_readback` and `scene_readback_<kind>` (in a game
  view, since the start). The frame step's publish adds
  `publish_native` and the kept frames by cause, `publish_kept_<slot>`,
  `publish_kept_untracked`, `publish_kept_border_<cause>`,
  `publish_kept_display`, `publish_kept_host` and `publish_failed`, and
  `publish_blit_native` for a published frame that drew the back buffer's blit
  ([retire-d3d-display.md](../graphics/retire-d3d-display.md#the-final-display));
  each appears once it has counted. `carried_census` keeps transient uses across
  all recorded frames. `frames` counts sink presents from one and
  `game_frames` counts those with guest D3D ops in a game view.
  `targets` groups game-view side surfaces by `width`, `height`,
  `format`, `depth`, `slot` and `operation`. Operations distinguish
  attachment binds and clears, the source and destination of a copy,
  uploads, snapshots, in-place samples and depth samples. `reads`
  groups guest readbacks by `slot` and `kind`, including the overlay
  and outside slots. Each row gives its `count`, `first_frame` and
  `last_frame`. Compare counts at two boundaries to isolate a window.
  On a direct level boot, `carried_activation` records the loaded map, its
  activation present, sink frame and a snapshot of process counters. A row
  whose `last_frame` exceeds `carried_activation.frame` occurred after loading.
  `carried_checkpoint` freezes the census corpus at original Swap 3000.
  It contains `present`, `sealed`, optional `inventory` and optional
  `counters`. It appears once the device has submitted the boundary.
  `sealed` records whether the active map already had a sealed scene.
  The original census supplies an `inventory` with the same schema as
  `carried_census` for original Swaps 1 through 3000 inclusive.
  A device or ordinal gap leaves that inventory absent. When the display
  worker completes Swap 3000, `counters` preserves its `scene_window_*`
  readings for the interval after level activation through that Swap.
  Missing or late activation, a second game start within the interval,
  or a missing original or worker ordinal leaves the checkpoint without
  counters. Later game activity does not modify the frozen values.
  A threaded GPU sink also reports `scene_window_*` counters from one device.
  `start` is its original Swap at activation and `through` is the last
  completed Swap. The interval excludes `start` and includes `through`.
  `owner_enabled` is 1 when this device selects a native frame owner.
  `gaps` counts missing or out-of-order joins. `games` counts completed
  frames that entered a game view or issued a game-view operation.
  `owned` and `ordinary` count those game frames by actual display path.
  `failed` counts failed presentations across the interval. `stage_drops`
  includes guest-side and worker-side stage drops. For game frames,
  `unresident` counts omitted instances and `unknown_residency` counts
  frames without a complete observation. `palette_views` counts game-view
  entries sampled for native map-palette residency. `palette_pending_frames`
  counts game frames where a view entered with a known incomplete palette.
  `palette_unknown_frames` counts game frames where a view lacked a valid
  palette observation or game work had no recorded view entry. Each frame
  retains observations from every split view. The current map's
  `scene_palette_definitions` counts distinct scenario object palette
  definitions. Each is in one of `scene_palette_pending`,
  `scene_palette_no_model` (an explicit null model reference),
  `scene_palette_failed`, `scene_palette_partial` or
  `scene_palette_resident`. Resident means every drawable model part
  has its uploaded geometry and its layer-3 material implementations.
  These gauges appear after a valid palette manifest is available.
  `read_surface`, `read_depth`,
  `read_mirror` and `read_pixel_count` count guest requests in game views.
  `flare_queries` counts original game-view reports installed by the
  owner. `unmatched_flare_frames` counts owned frames where that number
  differed from the original requests on the same Swap. Each read of
  these counters covers a single completed interval, even when the
  worker is drawing a later frame. The guest continues running.
  `configured_scale_<bucket>` counts the raster setting used on each
  completed original Swap. `physical_scale_<bucket>` counts the scale
  of its presented image relative to that original surface's guest
  extent. `game_physical_scale_<bucket>` counts that same observation
  for game-view originals. Buckets use `1x`, `4x` or the other observed
  integer factor. `unknown` counts missing observations. Empty buckets
  are omitted. These histograms also appear in the frozen checkpoint.
  The per-frame `aux` number appears only under `carried.side_targets`.
  The cumulative census is available on the census-only path. Rendering
  workers omit this extra scan. Both paths count
  `scene_carried_game_frames`, the guest presents with a D3D op inside
  a game view. With an installed frame owner, `scene_owned_presented`
  counts successful native presentations and `scene_owned_declined`
  counts presentations that take the ordinary backend path, including
  frames outside game views. `scene_owned_flare_queries` counts
  game-view pixel reports whose native result the owner installed.
  `scene_owned_candidates` counts sealed game frames paired with their
  guest presents and checked by the display worker. For each rejected
  candidate the worker records the first cause. `scene_owned_barriers`
  counts frames whose original commands had to run before Swap, for example
  for a guest read. `scene_owned_mismatches` counts disagreement between
  the seal and the queued commands or Swap parameters.
  `scene_owned_target_refusals` counts missing physical attachments and
  frames the physical backend cannot reproduce at the active scale or mode.
  On a scaled display, `scene_owned_scaled_whole` and
  `scene_owned_scaled_fractional` count owned frames on whole-number and
  fractional grids, and `scene_owned_scaled_refused_<reason>` counts the
  scaled owner's refusals by their first reason: `grid` (an axis
  minifies or passes the scale limit), `two_copies`, `busy`, `sealed`,
  `history` (1x E3 with older pixels), `targets`, `live_views`,
  `samples` and `stages` ([framescene.md](../graphics/framescene.md),
  slice 8, "Fractional grids").
  The private renderer reports its own publication queue in
  `scene_private_published`, `scene_private_taken`, `scene_private_replaced`
  and `scene_private_waiting`. In paced mode a newer seal can replace an
  unread private scene. In locked mode at most two scenes wait and each
  reaches the reader. These counts describe capture availability.
  With the sink and a render worker, `scene_residency_frames` counts
  presents where the worker observed a draw, a committed scene stage,
  an owned frame or a failure. `scene_unresident` counts distinct
  object handles per present whose dropped work lacked the planned
  GPU model, geometry buffer or material program.
  `scene_residency_unknown_frames` counts frames with an unclassified
  drop, a skipped guest packet or a worker failure. The counters appear
  after the first observation. A post-load zero requires a positive
  frame delta and zero growth in both omission counters. Private
  captures have no effect. Legacy refusals can keep their guest draws.
  Check native ownership across the same game views to measure native
  coverage.
  The raw `scene_readback_pixel_count` includes these requests.
  Compare raw and native counts at the same recording boundaries
  to identify requests that still need the reference.
  Both carried records are absent until the sink records a frame.
  Without the option the sink counters are also absent. With
  `frame-scene.census` in `H2_FEATURES`, `entries` is the entry census
  (`render_bridge::entry_census`; framescene.md, slice 4), totals since
  the start: `by_entry`, keyed by guest entry (`layer.<site>.<layer>`
  with site `scene`, `alternate`, `first_person`, `light`,
  `object_shadow` or `other`, the late passes' fog and water calls
  being `layer.scene.16` and `layer.scene.6`; `light_passes`,
  `object_shadow`, `screen_effects`, and under it `colour_pass` and
  `screen_effect`), each `{calls, d3d_draws, seed_steps,
  effects_seed_steps, seed_unmatched, animation_gets,
  particle_updates}`: the guest draws handed to the D3D backend under
  the entry, the steps of the shared random seeds `+0` and `+4`, calls
  whose seed was not a run of steps, and calls of
  `animation_resource_get` and of the particle systems' per-view update
  under it. Entries nest and each counts what is under it.
  `unbalanced` counts entries whose exit never ran and exits with
  nothing open; `draws_counted` is false when no D3D draw counter is
  registered. Without the option `entries` is absent. With `dump`, the whole scene is written as JSON
  under the cache directory (`scenes/scene-<frame>-<tick>.json`) and
  `dump` names the file. With no source
  registered: `UNAVAILABLE`, "no scene has been built".
- **`render.parity`.** `{"arm": true}` requests one game frame from the
  sealed scene's private renderer and the D3D display. The sink retains
  the original guest draws while armed. A frame that already admitted
  a typed stage before arming gives way to the next complete guest
  frame. The reference readback uses the ordinary display surface,
  before gamma, at its display scale. The reference requires Retail display
  lighting. An E3 display returns `UNAVAILABLE` because its scaled shadow
  maps use a different filter from the sealed renderer's baseline. The
  worker draws that present's sealed inputs on fresh targets at the
  observed scale. Smaller auxiliary levels retain their guest dimensions.
  The reply carries the sealed scene's `tick` and the pixel comparison
  returned by `render.compare`. The comparison covers the carried
  guest commands. Pixel parity of a typed substitution needs a separate
  reference run exercising that stage.
  The request completes at a tick boundary after both render paths
  finish. A new arming interrupts one still waiting. Unavailable
  inputs or a missing reference or private renderer give `UNAVAILABLE`.
  When the scaled display also fails to show its final 1× fallback,
  the matching capture and parity request report the failed Swap. A
  later Swap can still supply its own image.
  A request that receives no game frame within the deferred request
  limit times out. `{"arm": false}` gives `INVALID_PARAMS`.
- **`sim.hashes`.** The running game's post-tick digests in the rows sim
  mode writes to its report as `state_hashes_v1` (sim.md §6): `tick`,
  `gameplay` (16 hex digits) and, under `impl.sim.arena-hash`, `arena`
  (the game-state arena's hash). Boot's `agent-frames` tick observer
  computes them as sim mode does (the same digest function and the same
  `impl.sim.metrics-projection` switch) on every tick where the players
  can be read, on any run with an agent socket, rendered or not. It keeps
  the last 108,000 rows (an hour of ticks, about 2.6 MB), dropping the
  oldest (`dropped` counts them); a tick before the last, or more than 30
  after it (a new map, a snapshot load), starts the series again
  (`resets`). `from_tick` and `max` pick rows; `truncated` says more
  matched. Compare two launches row by row:

  ```sh
  diff <(jq -c '.rows[]' off-hashes.json) <(jq -c '.rows[]' on-hashes.json) | head
  ```
