# The menus, the fade and the blur on the overlay

Status: Partly built. Containers, plain bitmap widgets, supported text, lists and buttons, the fade, the bars, the loading screen and the blur under a dialog draw from the overlay; meters, scrolling bitmaps and unsupported widget states keep the guest's draws.
Code: `crates/rewrites/render-bridge/src/native_hud/ui_overlay.rs`, `ui_overlay/bitmaps.rs`, `modal_blur.rs`, `crates/extensions/game/src/hud_overlay.rs`, `modal_blur_glue.rs`, `crates/rewrites/scene-extract/src/ui` (`collect`, `compare`, `sort_widgets`, `read_widget`, `draw_list`), `crates/rewrites/native-render/src/ui_gpu.rs`, `glow.rs`. Tests: `crates/rewrites/render-bridge/tests/native_hud.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

| Draw | Taken at | Section |
|---|---|---|
| Containers, plain bitmap widgets, text, lists, buttons, idle model scenes | the sorted list (`0x00235428` to `0x0023545c`) | [The menus on the overlay](#the-menus-on-the-overlay) |
| Shader bitmaps | the shader quad `0x0022a664` | [native-hud-overlay.md](native-hud-overlay.md#bitmap-widgets) |
| Fade, bars, loading screen | their producers | [The fade, the bars and the loading screen](#the-fade-the-bars-and-the-loading-screen) |
| Backdrop blur under a dialog | `0x00235756` | [The blur under a dialog](#the-blur-under-a-dialog) |

## The menus on the overlay

The overlay stage draws a menu widget in place of its draw when the widget lists exactly. One list is one call of `0x002353a5` on one screen (Code, `h2 disasm`):

```mermaid
flowchart TD
  A[0x0023566a: collect the screen's widgets, max 256] --> B[qsort with 0x002356d8]
  B --> C[screen +0x58 slot, then the depth window]
  C --> D[each entry in the window: draw +0x10 at 0x00235445]
  D --> E[screen +0x5c slot]
```

1. `0x0023566a` collects the screen's widgets from the root. A widget whose vtable's `+0x40` slot answers false is skipped with its children; each shown widget is added after its children (first child `+0x14`, next sibling `+0x18`) as an entry of `0xc` bytes (the widget, its depth from `+0x60` by `0x0022e9aa`, its bias from `+0x6a`), at most 256.
2. The C runtime's `qsort` (`0x00321460`: median of three, an explicit stack, partitions of at most 8 by a selection sort) sorts them with `0x002356d8`. Depths more than 0.001 (`[0x0045dc70]`) apart put the deeper first; tied depths put the smaller bias first, then the larger widget type. The sort is not stable.
3. The screen's `+0x58` slot runs, then the window: the near depth `[0x0054d5ac] - [0x0054d5b0]` at `[ebp-8]`, the far `[0x0054d5b4] - [0x0054d5b0]` at `[ebp-0xc]`.
4. Each entry whose depth is inside the window has its draw (`+0x10`) called at `0x00235445` with the user argument, then the screen's `+0x5c` slot.

`scene_extract::ui` ports the collection (`collect`), the comparator (`compare`), the sort (`crt_qsort`, `sort_widgets`) and the window (`in_window`). The hooks read the guest's own sorted list, so their order is the guest's whatever the port does.

### Reading and placing a list

At `0x00235428` (`points::ui_render::LIST_READY`, after the window is taken), `render_bridge::native_hud::ui_overlay` asks whether the host draws menus and checks the viewport and screen before it reads the list. The host checks the UI scope's order (`Host::ui_overlay`). The screen quads' viewport (`0x00485648`, top, left, bottom, right) must cover the full canvas, both floating-point units must round to nearest, and the screen must have no earlier mismatch on this map.

Each drawn widget is read (`scene_extract::ui::read_widget`) and listed on its own (`draw_list`). Consecutive drawn entries that list form a run. An entry outside the window leaves the run open and a refused entry ends it.

After a guest draw, each run refreshes only its own entries at placement so changes to focus, colours or resource state reach the pass. List planning and run refresh time count toward `native_ui_extract_ns`.

At the sorted call of a run's first entry, the host places one overlay pass with the run's items (`Host::ui_pass`, `game::hud_overlay::place_menu`, over the map's overlay assets) and the items join `FrameScene::overlay`. A run with no items needs no pass. Each entry of a placed run resumes at `0x00235448` with its draw skipped, as a host-drawn widget's is. A run whose pass is not placed is the game's.

At `0x0023545c` (`LIST_DONE`) the claim closes. One whose left-out draws are not its claimed ones counts `menu_mismatch`, and its screen is the game's until the next map. A list drawn inside another's draw has its own frame and closes inside out.

### What the list reads

The overlay lists containers and plain bitmap widgets. A widget whose draw is the base's `ret 4` (`0x00333105`: the screen's, list item's, page selector's and table's) has no pixels. A bitmap widget's draw is read by `ui_overlay::bitmaps` ([Bitmap widgets](#bitmap-widgets)).

A shader bitmap's draw (`0x002b11c9`) sets its combiner and constants at its quad, not in memory the list can read. The list leaves it to the game: it ends a run and is not refused, and its quad is drawn from the overlay at the shader quad ([native-hud-overlay.md](native-hud-overlay.md#bitmap-widgets)).

Every other draw is the game's, counted by what is not read (`render.stats`, `native_ui_overlay_menu_*`), except these, which the class readers in `render_bridge::native_hud::ui_overlay` list:

- Text and button labels use the live UTF-16 interface, font, justification, colour, clipping rectangle, shadow and flash. The shaper uses the game font's advances and kerning to wrap and justify each line. Each pass fills the same glyph pages as the HUD before the guest draw is skipped ([text draw and interface reads](notes/2026-11-24-menu-text.md)).
- The list emits scroll arrows using the bound first and last rows, live row occupancy and the active skin. The sorted children carry the row layout and animated highlight. A button's bitmap uses its focus state and precedes its label in the same claim ([list and button draws](notes/2026-11-24-menu-list-button.md)).
- An idle model scene or a selection without objects or lights can produce an empty claim. At the nested camera call an active scene records its viewport, camera and ordered object handles as a `ModelView`, and its geometry still draws through the model material hooks ([model selection and camera](notes/2026-11-24-menu-model-selection.md)).

### What stays with the guest

The remaining guest draws count by reason in `render.stats`:

| Reason | Draw | What keeps it on the guest |
|---|---|---|
| `bitmap_meter` | `0x002b07e2` | the meter's own quads |
| `bitmap_scroll` | `0x00235f31` | the scrolling quad's coordinates from the widget's `+0x7c` state |
| `bitmap_state` | `0x002b0565` | a draw the overlay does not make as the game does: the debug labels on, an image set from elsewhere, an image not resident, a stage constant not white, the quads turned off |
| `text` | text or button label | interface overrides, reveal updates, substitution, custom tabs or an unsupported transform |
| `model` | model scene | active nested view, light processing or unresolved selection semantics |
| `draw` | list, button or custom draw | unsupported class-specific lookup, placement or guest side effects |
| `unlisted`, `pass` | any listed class | layout or pass preparation fails, including unavailable font pages or bitmap assets |

## Bitmap widgets

The bitmap widget's draw `0x002b0540` (Code, `h2 disasm`) draws nothing without a definition (`+0x70`), and draws a meter (`0x002b0a68`: the definition's flags bit 2) through `0x002b07e2`. Otherwise `0x002b0565` runs:

1. The view's offset (`0x002363d4`) from the draw's argument, the view's rectangle (top, left, bottom, right): 20 in a half-width view, left or right by its side, and 16 in a half-height one, up or down. With the debug labels on (`[0x0054d59f]`) a label follows the quad. A definition with no bitmap (`+0x1c` -1) draws no quad.
2. The tint: the alpha at `+0x64` and the colour at `+0x28` to `+0x30`, each times `[0x0045dc38]` and rounded by `fistp` (nearest, ties to even), packed ARGB by shifting left 8 and or-ing the next, with no masks, so a channel past 255 runs into the one above it.
3. The image (`0x002b0b19`): the pointer at `+0x8c` when set, else the bitmap tag's image at `+0x88` when in range (the tag's count at `+0x44`, its images from `+0x48`, `0x74` bytes each), the first otherwise.
4. The rectangle (`0x0022ea81`): the header's, plus the animated offset's X and Y, both corners projected at the offset's Z (`0x0023618e`, `0x002360c3`). The projection depth is the depth plus the camera's distance `[0x0054d5b0]` (1 when under `[0x0045dbdc]` in magnitude), kept at least 1 when not below `[0x0045dbd8]` and at most `[0x0045dbcc]` when below. X is times one over the depth over the distance, plus half the view's width; Y is half the view's height less Y times that. Then the view's offset is added (Y negated).
5. A definition that scrolls (`+0x10` or `+0x14` not 0) draws through `0x00235f31`. Otherwise `0x00235e5e` takes coordinates from 0 to the image's size over that size (at least 1), at most 1, in texels for a linear image. `0x0023675a` makes the four corners (the far ones as the near plus the extent) and a screen-quad record of the image (clamped, linear filtering), with blend mode 1 when the definition's `+0x08` is 1, else 0. It draws through `0x00052040` when the image is resident (`0x00012360`: its `+0x70` past `[0x004e6488]` and its texture set).

For a record of one image and no colour pointers, `0x00052040` writes a two-stage combiner: stage 0 multiplies the image by its first constant, stage 1 multiplies that by the vertex colour, and the final combiner outputs it. The constant is the colour `[0x00468710]` points at with alpha 1; when that colour is white, the combiner is `Combiner::Texture`. The begin's screen transform with its half pixel puts a guest pixel coordinate on the overlay's pixel edge of the same number.

`ui_overlay::bitmaps::read` reads all of this at the sorted list into a `UiContent::Bitmap` (the quad's rectangle and colour as the draw makes them), or refuses the widget: a meter, a scrolling bitmap, or any state above it does not draw as the game does. The image is named by its tag datum and index (`OverlayTexture::Bitmap`), and the menu's pass decodes it from the map before it is placed (`game::hud_overlay::place_menu`, which reads the loaded map off the game thread once a map). While the map loads, or when the image does not decode, the run is not placed and the game draws it.

## The fade, the bars and the loading screen

Each of these draws is taken at its producer ([native-ui-producers.md](native-ui-producers.md#native-ui)) and drawn from the overlay with the guest's own state at the draw (`game::hud_overlay::draw_screen`). That state is the pixel shader definition and its constants from the draw snapshot, the blend and its constant colour, the bound textures, and the vertex program's positions, coordinates and colour, as a HUD widget's quad is. The item names no tag (`Program::Shader` with datum 0) and samples its bound stages. A draw the capture does not take (a raster, texture or combiner state outside it) keeps the UI fan path (`screen_fallback`), counted `scene_kept_ui_<kind>_<cause>` with the capture's cause ([retire-d3d-menus.md](retire-d3d-menus.md#what-is-kept)).

| Draw | Detail (Code, `h2 disasm`) |
|---|---|
| Fade `0x00040890` | Its mode `[0x00485870]` (0 draws nothing) picks one of six cases through the table at `0x00040dc0`. The colour at `0x00485878` and its complement, each times the fraction at `0x00485874`, are packed (`0x00131e50`). Mode 1 blends one with one minus source alpha, added. Mode 2 is one with one, reverse-subtracted. Modes 3 to 5 are one minus the destination colour with one minus the constant colour, by maximum, minimum and addition, the packed colour as `BLENDCOLOR`. Mode 6 is one with one minus the constant colour, added, the packed complement as `BLENDCOLOR`. Each case writes its combiner words into the PSDEF at `0x00484f68` before the fan at `0x00040d86`. The overlay item carries the constant (`OverlayItem::blend_constant`), which the pass sets per batch (`fade_quads`) |
| Bars `0x00052ce7`, in `0x00052040` | Menu fade rectangles, cinematic and progress bars and split-screen dividers. A record with no image writes no combiner (`0x00052857` skips to the upload), so the PSDEF at `0x00484f68` is the one the last record with an image wrote; the snapshot at the draw holds that definition, so the bar inherits it as the game's does (`bar_quads`). A fan, quad list or polygon is taken as drawn, a strip with its corners put round the quad (`hud_overlay::corner_order`, [retire-d3d-menus.md](retire-d3d-menus.md#bars)) |
| Loading screen `0x000289e0` | Its background and sweep bitmaps at `0x0002ae60` and its spinner at `0x0002a1f4`, each with its bound bitmap (`loading_quads`) |

The typed `UiDraw::Fade`, `Rect` and `Bitmap` of `scene_extract::ui` are not filled from these draws: a fade's combiner is not one of the overlay's fixed ones.

## The blur under a dialog

Under a dialog the window stack's render fills the screen with the backdrop colour and calls `0x00235756` (`points::UI_MODAL_BLUR`, from `0x00235128`) with the screen's transition fraction in XMM0 (Code, `h2 disasm`). It multiplies the fraction by `[0x0045de14]` and calls the Kawase blur `0x000211a0` with EAX 0 and the words: those steps, scale 1.0, growth 1.0, dest 1, other -1, last 0, combine 1, 0, the strength `[0x0045dc20]` and `y` 0.0. That call returns to `0x00235794`, and the wrapper then returns with a bare RET.

```mermaid
flowchart LR
  A[target 0] --> B[step 1 into target 1]
  B --> C[step 2 into target 0]
  C --> D[... ping-pong]
  D --> E{odd draw count?}
  E -- yes --> F[copy target 1 to 0: one quad of 0x00034a90]
  E -- no --> G[done]
```

The blur reads target 0 and ping-pongs between targets 1 and 0 (`engine::glow::blur`). Each step's spread is its fraction times the scale, and each scale is the last times `1 - growth + growth * [0x0045dc0c]`, the whole spread factor since the growth is 1. An odd count of draws ends in target 1, which `0x000351a0` copies back to 0 with one more quad of `0x00034a90` (one tap, the texture's colour). Each draw is a full-target quad of four bilinear taps, tap `k` moved by the spread times `[0x0045dbbc]` for `k & 1` in x and `k & 2` in y, else `[0x0045dbc4]`, its negative (`native_render::glow::quad`).

The strength selects `0x00020f30`'s eight-stage combiner (`engine::glow::blur_combiner`). Its colour is not the taps' mean. Stage 0 makes `clamp(4 * (t0 - t1))`, stage 1 the weight `(d * c + 1) / 2` with `c` the strength packed to a byte in each stage's first constant, and stage 2 `w * t0 + (1 - w) * t1`.

Stages 3 to 5 do the same for `t2` and `t3`; stages 6 and 7 and the final combiner do the same for the two results. Each pair leans to its brighter tap: its weight moves from a half by up to half of `c`. The alpha is the mean of the four. A strength of 0 gives the three-stage plain mean.

### How the overlay takes it

`render_bridge::native_hud::modal_blur` observes the blur's entry when its return is `0x00235794`, reads the ten words and computes the draws and the combiner as the guest will (`read`). The overlay's `Blur` item carries the steps' spreads, the tap and the combiner's constant over 255 (`OverlayBlur::pick`, 0 for the plain form).

The host arms the device (`Host::modal_blur`, `game::modal_blur_glue`). The game's code runs whole, so its target binds, texture stage writes, combiner block and dirty bits are its own. The device holds back its quads, the copy's included (`Device::swallow`: slot 0 the target bound at the call, the UI's, slot 1 target 1).

At the wrapper's return (`Host::finish_modal_blur`) the quads are checked: unblended, every channel written, each sampling the target the plan names, whole, with targets 0 and 1 the same size. The overlay's blur is then drawn into target 0 by `OverlayGpu` in their place: the copy, the steps in scratch targets, the result written back into every channel.

The renderer reuses the scratch pair for each of the two most recent target sizes and formats, so the guest and display copies can alternate each frame. The item joins `FrameScene::overlay`. A quad off the plan, a check that fails or a pass that does not record gives the game's quads back in order (`overlay_modal_blur_replayed`).

### Counters

`render.stats` counts the blurs drawn (`overlay_modal_blurs`), those given back, those the host did not arm (`overlay_modal_blur_refused_pass`: the UI scope's order did not hold) and those not read, by reason:

| Counter | Reason |
|---|---|
| `overlay_modal_blur_refused_arguments` | a word not the one the wrapper pushes |
| `overlay_modal_blur_refused_steps` | past `engine::glow::MAX_BLUR_STEPS` |
| `overlay_modal_blur_refused_combiner` | the `y` form or another stage count |
| `overlay_modal_blur_refused_targets` | a draw outside targets 0 and 1 |

## Related

[native-post-hud.md](native-post-hud.md), [native-ui-producers.md](native-ui-producers.md), [native-hud-overlay.md](native-hud-overlay.md), [native-post.md](native-post.md), [retire-d3d.md](retire-d3d.md), [formats/ui.md](../formats/ui.md).
