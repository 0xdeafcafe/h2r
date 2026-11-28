# High resolution: widescreen projection and the display fit

Status: Built. The window's aspect drives the projection and the UI fit; the gather lens and display supplement keep every window up to 12:5 (and wider with the supplement) showing the same objects.
Code: `crates/extensions/game/src/render_aspect.rs`, `crates/platform/d3d/src/device.rs`, `crates/platform/d3d/src/screen_rect.rs`, `crates/rewrites/render-bridge/src/ui_layout.rs`. Tests: named per section.
Switch: HUD layout (`Centered` or `Pinned`), listed in [switches.md](../runtime/switches.md#settings).

This page is the detail behind "Widescreen projection" in [high-resolution.md](high-resolution.md). The guest builds its camera for 640x480. The host widens the horizontal terms to the window's aspect, so the picture is Hor+ at any window shape, and fits menus and HUD inside a uniform safe area.

| Part | What it does | Where |
|---|---|---|
| Projection rule | horizontal field of view from the vertical one and the presented aspect | the builder at `0x0002fb70` |
| Exact-mode lens | scales the horizontal clip coordinate of recognised perspective draws | `render_aspect.rs` |
| Lens fit | widens the guest's own projection terms per view | `VIEW_PROJECTION_TERMS`, `widen_terms` |
| Gather lens | a fixed 12:5 frustum for the visibility, portal and object bounds | `GATHER_ASPECT` |
| Display supplement | draws what the gather lens leaves out of a wider window | `DisplayVisibility`, `VisibilityLens::Display` |
| UI fit | uniform scale in a centred safe area; background art keeps the full view | `render_bridge::ui_layout`, `Device::set_ui_fit` |
| Full-frame passes | drop the fit where a quad spans the frame | `spans_the_fitted_frame`, `lies_over_the_frame` |
| Ocean grid | widens the camera-attached water grid | `widen_ocean_grid` |

## The projection rule

For a vertical field of view `v` and a presented viewport aspect `a`, the horizontal field of view is `2 * atan(tan(v / 2) * a)`. The Xbox projection builder (`0x0002fb70`) applies this to its 640x480 viewport and multiplies its aspect by the retail widescreen factor at `0x0045dfdc` when the anamorphic state is set. A 16:9 image therefore fits the same 640x480 allocation as a 4:3 one.

`render_aspect.rs` reads the anamorphic state at both the game and the alternate view entries. In exact mode, for a recognised perspective draw, the host scales the horizontal clip coordinate by the ratio of the guest's pixel aspect to the displayed pixel aspect. For translated shaders the recogniser follows the X, Y and W rows through partial position writes. It accepts a four-row perspective transform, or a far-plane surface that writes W to Z. Programs with an unrecognised clip transform keep their positions.

With a retail camera:

- The flare pass translates its occlusion query to the projected display column, keeping the footprint behind the expected count, and applies the lens to the reflection billboards and final composite fans.
- Light volumes and light trails use the same CPU projector. Both billboard emitters take the display transform about the centre of the player's viewport.
- The depth billboard supplies its guest window position twice. The vertex translator applies the transform to its depth lookup in register 1 and to its raster position. Its sprite UVs keep their authored range.
- A perspective draw that samples scene colour or depth with a bump matrix applies the lens to both coefficients that move the lookup horizontally, including when the guest camera already holds the correction. A frame-sized render level such as texaccum is identified by its backing address, format and dimensions. Asset-space bump matrices keep their material values.

The exact reference draws both at the retail coordinates. The display keeps the simulation camera's vertical field of view and depth.

## Menus and the UI fit

Orthographic menu widgets use a uniform fit inside each player's viewport (see "The game's 640x480 assumptions" in [high-resolution.md](high-resolution.md)). A menu quad is **background art**, and keeps the full viewport as on a widescreen Xbox, when along the fitted axis:

| Case | Rule | Example |
|---|---|---|
| Edge to edge | it reaches both edges of the viewport | a full-screen backdrop |
| Long bitmap | it reaches one edge and its whole bitmap at its drawn scale is at least as long as the viewport | the main menu's scrolling strips: 1024-texel bitmaps drawn as two pieces that meet where the texture wraps |
| Panel or tab | shorter bitmap at an edge | keeps the fit |

Only the UI rectangle site gives the bitmap's texture coordinates. An emblem draws a unit quad through the shader quad site, placed by its transform, so the shader and screen quad sites use the first case alone. The choice is made before the native or guest draw at the shader quad (`HUD_QUAD_READY`), screen quad (`UI_SCREEN_QUAD_READY`) and UI rectangle sites.

The screen and rectangle shaders use raster-local coordinates, so their art tests use the width and height of their raster rectangle. The UI rectangle (`0x00052040`) is read at the dynamic upload helper's `EFFECT_QUAD_READY` when it returns to `RECT_SUBMIT_RETURN`. The next quad or widget (`UI_ITEM_CALL`) restores the fit.

The full-view fades in `UI_RENDER_LAYERS`, the start-screen transition and the window stack's dialog dim all call `UI_MENU_VIEW_FILL`. It subtracts the supplied rectangle's origin and takes its quad's width and height from the same raster rectangle as the rectangle shader (`0x00485648`).

The fill uses the full view through submission, then restores the widget fit. The current view can be a split-screen viewport. Tests in `render_aspect.rs`: `menu_view_fades_use_the_rectangle_their_shader_projects_over`, `screen_art_uses_raster_local_bounds_in_the_right_hand_view`, `quads_are_read_at_the_shader_screen_and_rect_sites`.

The dialog then blurs the screen under it with `SCREEN_CONVOLUTION_BLUR` (called by the modal blur at `0x00235756`), which copies whole 640x480 targets back and forth. The device draws any draw that samples a render target the size of its own 640x480 target without the fit, so a frame copy covers the frame (`crates/platform/d3d/src/device.rs`, test `a_frame_copy_in_a_fitted_ui_scope_keeps_the_whole_frame`). With the fit, each copy would shrink the picture into the centred box and leave stale target contents at the sides.

### Full-frame passes

A screen-space quad whose positions reach both edges of the frame along the axis the fit shrinks is a full-frame pass: the cutscene tints, the title screen's backdrop fill, the wide convolution's grid rows, the colour pass's quads, and any menu fill a submission site did not read. The device takes the window position of each unindexed vertex and drops the widget fit for the draw (`device.rs`, `spans_the_fitted_frame`).

A vertex program's vertices go through the program, so positions from constant rows, passed through, or written as inline Begin/End words and `D3DDevice_End` strips (the title screen's backdrop blur and tint) land where the GPU puts them. Pre-transformed vertices are read as given. The reading is `crates/platform/d3d/src/screen_rect.rs`, the same one [`render.frame_layout`](../runtime/agent-api-render.md#renderframe_layout) reports as `window_xy`. Tests in `crates/platform/d3d/tests/passes.rs`: `full_frame_quads_span_the_display_in_a_fitted_ui_scope`, `immediate_program_quads_over_the_frame_span_the_display_in_a_fitted_ui_scope`.

A quad that covers the cinema window (full width, letterboxed height) spans the display along the fitted axis and keeps the fit on the other.

A projected-effect fit (the flare, light volume and billboard scopes in `render_aspect.rs`) keeps frame copies and frame-edge strips, since a sprite can be bigger than the view. One draw kind drops the fit inside such a scope: four vertices whose window bounds match the frame's edges to within a guest pixel on both axes, sampling an image the device rendered (a render target, the depth buffer or a frame snapshot).

That is a screen pass, not a sprite (the title screen's depth of field, blur composite and frame-snapshot passes). See `lies_over_the_frame` in `device.rs` and test `a_screen_pass_in_a_projected_effect_scope_spans_the_display`. A depth billboard, whose fit also moves its screen-position input, keeps the fit.

Corner HUD groups can use the fitted rectangle or the viewport edges. If the game already uses the Xbox widescreen HUD layout, its artwork receives only the remaining aspect correction. At 16:9 the Pinned layout keeps the retail widescreen fit.

## The lens fit

Every session that draws in a window, single-copy or exact, fits the horizontal projection terms in the guest's active and saved display cameras at `VIEW_PROJECTION_TERMS` to the window's aspect, view by view, so a resize or fullscreen change takes effect at the next view. Lockstep and input tapes do not register the fit. A game of more than one machine (System Link, Live, co-op, or a bot's phantom machine; `render_rng::networked`) keeps the retail terms, since its peers simulate the same world through retail cameras. [Determinism](high-resolution.md#determinism) says why the fit leaves the simulation alone.

The fit writes two lenses (`widen_terms`):

| Lens | Takes | Feeds |
|---|---|---|
| Window lens | the window's aspect, in the projection's bias and scale | the camera matrix and the CPU pixel scale |
| Gather lens | a fixed 12:5 frustum (`GATHER_ASPECT`) about the same centre, in the four tangent bounds | the root portal bounds, the portal walk and the object planes |

In every window up to 12:5 (4:3, 16:10, 16:9 and the 21:9 monitors: 2560x1080, 3440x1440, 3840x1600) the guest therefore lists, sets up and draws through all of its layers every cluster and object the picture can show, the same ones whatever the window. A window wider than 12:5 (32:9) takes its own aspect for the bounds too (`frustum_factor`); the objects it lists outside the gather lens come from the display supplement ([Determinism](high-resolution.md#determinism), "Object setup"). The cost is the guest's setup and draws for what the gather lens holds outside a narrower window.

Every wide view starts with the display supplement's lens. The hook swaps in the guest lens once it has fitted the view, and a view it does not fit keeps the supplement (`starting_lens`). `render.scene`'s `scene_visibility_lens_*` counts the views by lens, and `scene_guest_lens_fit_*` counts the hook's calls by outcome: applied, networked, another caller (with the last one's address), unchanged. The site is one instruction past the builder call's continuation, because a return resumes the caller's own code and a hook on a continuation never runs.

Water and fog geometry built with this camera, the sky and CPU-projected effects share its horizontal lens. The BSP flood clips each open portal to its parent window. An object is listed while its sphere intersects one of its cluster's window frusta, at the sides, top and bottom alike (test `a_sphere_is_listed_until_it_is_wholly_off_a_wide_pitched_screen` in `crates/rewrites/scene-extract/src/visibility/frusta.rs`). The observer supplies the vertical field of view.

### The ocean grid

The ocean of a BSP with water definitions (the `water_ocean` of a bay level) is a grid attached to the camera. Its setup (`0x0003ca90`, called by the water teardown `0x0003c3c0`) scales the grid's sideways vector by `tan(min(fov / 2, 0.7807)) * aspect` with 13 % to spare, where `fov` is the horizontal field of view at `0x00485640` and `aspect` is the raster rectangle's (640x480, times 1.5 for an anamorphic lens). It builds the grid's model rows (`c[50..52]`) and its visibility quad from that. The aspect is not the projection's, so a fitted view wider than about 2:1 would show past the grid: a hard edge on either side, with the scene under the water beyond it.

At `OCEAN_GRID_ASPECT` (`0x0003cbf8`, where the setup's two paths meet after storing the aspect), every view wider than 4:3 divides the stored aspect by the projection's factor (`widen_ocean_grid`, the rule `engine::ocean_grid::widen_aspect`), so the grid spans the window with retail's margin at 4:3.

Under the native water end the grid is Rust (`engine::ocean_grid`, [retire-d3d-display.md](retire-d3d-display.md), "The water end"), which applies the same rule at the same point from the view lens render-aspect hands it (`render_bridge::water_end::ocean::set_grid_lens`); the site is not reached there.

That holds whichever lens displays the view: the fitted guest camera, or the device's per-draw lens on a view that keeps the retail camera (a game of several machines, including a bot's phantom machine), which widens the grid's draws as it does the world's. The grid is render geometry; nothing simulated reads it. Unwidened under either lens, the grid's curved sides show inside a 16:9 or 21:9 window.

The setup caps its half angle at 0.7807 (`0x0045e2d4`); the widened aspect also carries `tan(half) / tan(cap)`, so a lens past the cap is covered. A 4:3 window, and lockstep (which does not register the hook), keep the retail grid. `render.scene` counts the setup's calls as `scene_guest_lens_ocean_widened` and `scene_guest_lens_ocean_retail`. Tests: `the_ocean_grid_reaches_the_sides_of_every_window`, `the_ocean_grid_covers_a_networked_view_drawn_through_the_display_lens`, `the_ocean_grid_keeps_retail_where_the_lens_is_not_wider`.

## The display supplement in exact mode

Exact mode keeps the guest gather and visibility readbacks at the retail lens. The scene step walks the open portals with the stored gather frustum and a display copy whose tangent bounds widen around the same lens centre. It excludes geometry already admitted by the retail lens or submitted by the guest.

| Content | Admitted when |
|---|---|
| Rigid BSP placement | it intersects a portal window in a cluster that references it |
| Layers 6 and 16 | the guest layer is empty and the cluster, wholly inside one portal window, has one supported part; the live material is visible with a single shader LOD |
| Fog | the cluster's atmosphere and the planar gate select the loaded implementation |
| Visible water surface | the sorted transparent handler on layer 15 has the guest's sorted entry; a cluster ID alone cannot supply a missing surface |
| Decorator placements | they come from the guest's per-view list |
| Layer 3 | resident cluster parts and fully opaque rigid placements |

A layer-6 material count describes the edge blend. A display-only section is drawn whole or not at all. A cluster is kept only when every visible material of it, over all its LODs, draws in no layer but 3, 6 and 16 (`native_world::display_cluster_complete`, `DISPLAY_LAYERS` in `crates/rewrites/render-bridge/src/native_world/section.rs`), the layers the supplement has a display draw for. A part that also draws in layer 1 (the accumulation a combined layer-3 shader samples), an overlay layer (5, 7, 8, 20, 2, 17), a layer-15 pool surface or a sorted layer (18, 19, 21) leaves its whole cluster out, with its water edge and fog.

A placement or cluster is also left out when any of its parts is not loaded or samples the accumulation target (`GpuWorld::display_complete_parts` in `crates/rewrites/native-render/src/gpu.rs`). `render.scene`'s `scene_visibility_display_incomplete` counts the clusters left out.

For objects, the supplement builds the model sections and node palettes on a private copy of the record writes. Materials without native layer-3 support, fades, attachments and objects that need guest animation code can still leave gaps at the sides. The guest's flare visibility count comes from its own reference depth and query.

To locate a missing BSP cluster, compare its ID in the guest visibility list, the structure list and the layer-3 list for the same view. The sealed scene includes all three and its camera. The display frame recorder includes the final scissor and projection factor. See [FrameScene reads](../runtime/agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes) and [display raster](../runtime/agent-api-render.md#renderframe_layout).

## Other views

- **First person.** Draws use the weapon camera and the section, parts and node palette selected by the guest's first-person records. On the native layer-3 path the material's fog and specular state resolve over the device state at the draw boundary (`crates/extensions/game/src/native_first_person_glue.rs`).
- **Theater observer.** The playerless observer has the whole BSP and object array. It uses the same Hor+ lens for projection and its own frustum test on both. See [the host film view](../game/film-format.md#choosing-a-players-view).
- **Highest.** The setting requests the finest model tiers at any resolution; the structure walks keep their retail projected sizes and pixel radii. To compare raster cost with retail geometry choices, record both settings with the timings ([lab benchmarks](../runtime/lab.md#sim-benchmarks)). In offline play the guest walk selects retail geometry whatever the setting ([projected size and level of detail](occlusion-culling.md#projected-size-and-level-of-detail)).

## Related

[high-resolution.md](high-resolution.md), [native-visibility.md](native-visibility.md), [native-world.md](native-world.md), [native-water-fog.md](native-water-fog.md), [settings-menu.md](../game/settings-menu.md).
