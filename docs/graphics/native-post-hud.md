# Native renderer stage 5: post and HUD

Status: Partly built. The colour-pass glow, the scenario screen effects, layer 17's items, lens-flare colour and visibility fans, and the UI and HUD draw natively. Pixel parity with D3D needs a live check.
Code: `crates/rewrites/render-bridge/src/native_post.rs`, `native_hud.rs`, `native_hud/hud_overlay.rs`, `crates/extensions/game/src/hud_overlay.rs`, `crates/extensions/game/src/native_post_glue.rs`, `native_post_load.rs`, `native_hud_glue.rs`, `crates/rewrites/scene-extract/src/layout/post.rs`, `glow.rs`, `screen_look.rs`, `screen_gpu.rs`, `hud.rs`, `ui_gpu.rs`, `crates/rewrites/engine/src/colour_pass.rs`, `glow.rs`, `screen_effects.rs`, `screen_grid.rs`, `function_eval.rs`. Tests: `crates/rewrites/render-bridge/tests/native_post.rs`, `native_hud.rs`, `crates/rewrites/engine/tests/colour_pass_differential.rs`, `glow_differential.rs`, `screen_effects_differential.rs`, `screen_grid_differential.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Part of [native-renderer.md](native-renderer.md).

## Stage 5: post and HUD

Stage 5 is everything the game draws after a view's scene: the glow and the screen effects, the lens flares, the fade, the HUD and the UI. Each piece is taken at its guest producer and drawn natively, or left to the guest's draw packets under a counted reason.

| Piece | Page | Taken at |
|---|---|---|
| Call order, hook points | [native-post-frame.md](native-post-frame.md) | `render_screen_effects` `0x00013870` and the view's tail |
| Colour pass glow, screen effect bodies, native passes | [native-post.md](native-post.md) | `0x000214f0`, `0x0002d140`, Rust bodies |
| Scenario screen effects, convolutions | [native-screen-effects.md](native-screen-effects.md) | `0x0002d140` |
| UI and HUD draw primitives, producers, fallback | [native-ui-producers.md](native-ui-producers.md) | the sorted widget call `0x00235445`, the quad producers |
| HUD bitmaps, text, glyph pages | [native-hud-overlay.md](native-hud-overlay.md) | the shader quad `0x0022a664` |
| Menus, bitmap widgets, fade, bars, loading, dialog blur | [native-menus-overlay.md](native-menus-overlay.md) | the sorted list `0x002353a5`, `0x00040890`, `0x00235756` |

The headings below stay at these paths because code and other pages cite them. Each says where its content is.

### The path (Code, `h2 disasm`)

The frame, one view, the scene passes' tail, `render_screen_effects` and what follows the scene: [native-post-frame.md](native-post-frame.md).

### Hook point and order

The colour pass (`COLOUR_PASS`, `0x000214f0`) and each screen effect (`SCREEN_EFFECT`, `0x0002d140`) run their guest bodies; the entry hook records the plan and the return continuation closes it ([native-post-frame.md](native-post-frame.md#hook-point-and-order)).

### What the native post draws

The colour pass glow, the screen effects and the lens flares' colour fans: [native-post.md](native-post.md#what-the-native-post-draws).

### The UI and HUD (Code, `h2 disasm`)

Where the UI and HUD are drawn, the three draw primitives and the end of the frame: [native-ui-producers.md](native-ui-producers.md).

#### Native UI

The producers Rust reads (screen quads, shader bitmaps, fade, bars, loading, motion sensor, model widgets, outlines, host-drawn widgets), the fallback and the `native_ui_*` counters: [native-ui-producers.md](native-ui-producers.md#native-ui).

#### The HUD on the overlay

HUD bitmaps, text widgets, shader state and glyph pages: [native-hud-overlay.md](native-hud-overlay.md).

#### The menus on the overlay

The sorted widget list, reading and placing a run, and what stays with the guest: [native-menus-overlay.md](native-menus-overlay.md#the-menus-on-the-overlay).

##### Bitmap widgets

The bitmap widget draw `0x002b0540` and how the overlay reads it: [native-menus-overlay.md](native-menus-overlay.md#bitmap-widgets).

##### The fade, the bars and the loading screen

Taken at their producers with the guest's own state: [native-menus-overlay.md](native-menus-overlay.md#the-fade-the-bars-and-the-loading-screen).

##### The blur under a dialog

The Kawase blur `0x000211a0` called from `0x00235756`, drawn as the overlay's `Blur` item: [native-menus-overlay.md](native-menus-overlay.md#the-blur-under-a-dialog).

### What panics

Every post call the native passes do not cover: [native-post.md](native-post.md#what-panics).

### The post hooks

`feature::native_post` bodies: [native-post.md](native-post.md#the-bodies).

### The native passes

Slots by role and the raster from the plan: [native-post.md](native-post.md#the-native-passes).

### The colour pass

The port of `0x000214f0`, the quad and the native glow: [native-post.md](native-post.md#the-colour-pass).

### The screen effects

`0x0002d140`, its function evaluator, plans, convolutions, look and GPU pass: [native-screen-effects.md](native-screen-effects.md).

### The Halo 1 cinematic pass

The Halo 1 campaign's screen effect (blur, light, desaturation, tint and the
video effect) is a stage of its own, `native_render::cinematic_gpu`. It takes a
colour target and plain parameters (`Params`, `Target`, `Video`) and names no
device type. `h1_cinematics` reads the campaign's globals into `Params` at
`RENDER_SCREEN_EFFECTS` and hands a `Request` to the installed drawer;
`native_cinematic_glue` is that drawer, queuing the stage over the device with
`Device::external_pass_now`.

## Rules and limits

- Each post call is one native pass; a plan with an unsupported input panics, `post: <cause>`.
- Under render scaling, a replacement that uses a reference-only intermediate together with a live display target falls back.
- Screen-effect residency follows the loaded effect set, including its feedback copies and convolution programs. The screen and glow drawers cap the raster variants they retain per program. The UI draw cache follows the active device and caps its shader and pipeline variants.
- `0x0002df00` and `0x0002dd30` separate flare colour from visibility. The native flare stage records eligible colour and counted visibility fans. The tests' counts are answered through `GetVisibilityTestResult` from the backend's counter ([d3d.md](d3d.md), "Visibility tests"), and an owned frame's graph replays the counted draws and installs its counts under the guest's tickets ([framescene.md](framescene.md), slice 6).
- Layer 17's items are drawn as the other scene layers' are; the clear before them is the guest's. The frame tail draws supported copy and composite fans through the native screen-space pass. The frame's finish (`0x00013cd0`) is made on the device ([retire-d3d-display.md](retire-d3d-display.md#the-frames-finish)); a kept frame's present (`0x00013f10`) uses D3D.

## Not verified

- HUD pixel parity on the lab ([native-hud-overlay.md](native-hud-overlay.md#not-verified)).
- Pixel identity of the screen effects and the glow with `impl.render.native` on and off, and the cost of `render_screen_effects` against a D3D measurement ([native-screen-effects.md](native-screen-effects.md#not-verified)).
- That the D3D path's pixel centres match a quad whose corners are the window's (S).

## Related

[native-renderer.md](native-renderer.md), [native-transparents.md](native-transparents.md), [formats/ui.md](../formats/ui.md), [d3d.md](d3d.md), [debug-menu.md](../runtime/debug-menu.md).
