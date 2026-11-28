# Retiring D3D: movies, frames outside a game view and the display (R8)

Status: Partly built. The frame step, the movie's picture, the publish (in the frame step and the three frame loops outside it), the frame tail, the frame's finish and the view setup and view end are native, the frame wait calls no D3D entry, and every render target and depth surface is the frame graph's, by role; a frame tail or view setup it does not cover and a frame with a carried slot keep the guest's D3D calls, each under a named `render.residual` row.
Code: `crates/rewrites/render-bridge/src/render_frame.rs` (the frame step), `native_targets.rs` (the finish), `render_frame/publish.rs`, `d3d::Device::publish` and `game::d3d_glue::SinkDrain` (the publish), `crates/platform/bink-hle/src/display.rs` and `crates/rewrites/render-bridge/src/native_movie.rs` (the movie), `crates/extensions/game/src/movie_display_glue.rs` and `hud_overlay::place_movie` (the glue), `render_frame::loop_present` (the frame loops' present), `frame_tail.rs` with `hud_overlay::place_tail` and `frame_scene::OverlayKind::Hold` (the frame tail), `view_setup.rs` (the view setup and view end), `native-render/src/frame_graph/targets.rs`, `native_targets::role`, `d3d::Device::make_target` and `d3d::threaded::owner::TargetStore` (the graph's targets), `residual_census::REGISTRY` (the rows). Tests: in each, `the_frame_finish_makes_no_d3d_entry` (`native_targets.rs`), `a_frame_with_a_screen_effect_makes_no_d3d_entry` and `uncovered_tails_are_refused_by_name` (`frame_tail.rs`), `the_view_setup_makes_no_d3d_entry`, `uncovered_setups_are_refused_by_name` and `a_check_compares_the_guest_bodys_state` (`view_setup/tests.rs`), `the_graph_owns_its_targets_across_frames` (`frame_graph/targets.rs`), `a_held_texture_survives_into_the_next_frame` (`native-render/src/ui_gpu.rs`), `the_frame_wait_is_outside_the_render` and `the_frames_own_calls_are_named_rows_inside_and_outside_the_frame_step` (`residual_census.rs`), `a_loop_frame_from_the_scene_is_published_with_no_d3d_entry` (`render_frame.rs`), `a_native_display_takes_the_picture_and_leaves_the_guest_texture_alone` (`crates/platform/bink-hle/tests/synthetic.rs`) and `a_published_frame_presents_with_the_frame_steps_ramp_and_the_next_vblank_reports_its_swap` (`crates/platform/d3d/tests/round7.rs`).
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md). Off, the frame is the guest's and the movie fills and draws the guest's texture.

Slice R8 of [retire-d3d.md](retire-d3d.md). `render_frame_and_present` `0x00223240` is Rust: the frame step calls each part of the guest's frame in its order, so each part goes native in place. The exit: from boot to gameplay and back to the menu, `render.residual`'s `d3d_entries` grow only under `outside_render`. R7 owns the menus, R6 the HUD; R8 takes the frame itself, the movie, and what neither draws.

```mermaid
flowchart LR
  F[0x00223240 frame step] -->|world| V[0x002234e0 views]
  F -->|otherwise| O[0x002232a0 outside a view]
  O --> M[0x00156040 movie tick]
  M --> D[native movie draw]
  D --> OV[overlay pass]
  F --> W[0x0012b2e0 frame wait]
  W -->|caches idle| PB{every slot from the scene}
  PB -->|yes| N[publish: native present, ramp, swap]
  PB -->|no, counted| P[0x00013f10 present]
```

## The frame step

`render_bridge::render_frame::RenderFrame` implements `RENDER_FRAME` (**Code**, the guest's body at `0x00223240..0x0022329f`):

| Step | Guest call | When |
|---|---|---|
| 1 | `0x00068290`, the UI's world skip | engine globals active: `[0x004e6948]` set, its `+0x1120` byte set, its `+2` word not `0xffff` |
| 2 | `0x002234e0`, the views | the skip returned 0 |
| 2 | `0x002232a0`, the frame outside a view | otherwise |
| 3 | `0x0012b2e0`, the frame wait | always |
| 4 | `0x00013f10(0, 0)`, the present | both cache countdowns (`0x004e6470`, `0x004e64a0`), read after step 2, at or below 0, and the publish keeps the frame ([below](#the-final-display)) |

`render_frame::next` is the table as data, tested against the guest's order; `render_frame::after_wait` skips step 4 for a published frame. Each step is a `call_guest_named` from the frame's own stack; the frame returns to its caller after the last. Every other feature's hooks on `RENDER_FRAME` (render timing, the bots' pads, the map slots) still run around it. A light host's `sim-frame` owns the point, so the frame step is not added there (`crates/apps/boot/src/lib.rs`).

`render.residual` files the frame wait under `outside_render` (`residual_census::frame_wait`, called around step 3): the wait (`d3d::FrameWait`, a Rust override) calls no D3D entry, and the vblank callback `0x00014280` it delivers only counts. What other threads call while it yields is theirs, not the frame's.

Besides the present, the step replaces nothing by itself. What makes a frame native is the step's callees: the views' stages ([retire-d3d.md](retire-d3d.md), R1 to R5), the overlay's producers (R6, R7) and the movie (below).

## Movies

With the native renderer on, `movies_glue` sets `BinkHle::use_native_display`, and playback mode adds two overrides to the Bink API's ten ([bink-hle.md](../audio/bink-hle.md)):

| Guest | Guest body | Native body |
|---|---|---|
| `0x001568d0` decode and blit | `BinkDoFrame`, `BinkNextFrame`, lock the texture (`0x003fad50`, `0x003fdbc0`), `BinkCopyToBuffer` | `BinkDoFrame`'s and `BinkNextFrame`'s bodies on `[0x004e9194]`; EAX 0 |
| `0x00156960` draw | one screen quad (`0x00035b90` with the record at `0x004e91c4`, `0x000363a0`), then `0x0001e930(1)` when `0x004c1a18` is set | the quad and the decoded picture to the host (`bink_hle::MovieDisplay`) |

The guest's texture is never locked or written. The quad (`bink_hle::display::movie_quad`, **Code** `0x00156960..0x00156a79`) is the guest's integer arithmetic over the rectangle at `0x00485a8a` (top, left, bottom, right): with start flag bit 4 the rectangle's edges, otherwise the movie at its own size, centred, `(span - size) / 2` to `(span + size) / 2` from 0, halved toward zero. Every movie the game starts (intro `0x1c6`, attract `0xe`, credits `0x40c`) is centred. Corner 0 is the top left, then clockwise; coordinates run from 0 to the movie's size in texels; each corner is white.

`render_bridge::native_movie::overlay` reads the record as the begin reads it (`native_render::hud::Record`) and makes one `MovieFrame` item over the 640 x 480 canvas: corners through the begin's screen transform and the full-screen viewport (`native_render::hud::text_quad`), coordinates as the texels of the guest's texture over the picture's size, the record's blend, filter and address mode, `Combiner::Texture`, red, green and blue writes. `hud_overlay::place_movie` places it as an overlay pass at the guest's point in the frame, with the UI fit the guest's draws carry there; the pass uploads the picture first (`OverlayResidency::sync_movie`: a `Bgra8Unorm` texture of the picture's size, written when the frame number moves, kept across a map change).

A draw that is not placed shows nothing, since no guest texture is behind it. Each is counted in `render.stats`:

| Counter | Cause |
|---|---|
| `native_ui_movie_placed` | placed |
| `native_ui_movie_refused_record` | the record or its bitmap does not read, keeps the caller's state, binds another stage, has a second constant block or a pixel offset |
| `native_ui_movie_refused_frame` | the screen quads' viewport `0x00485648` is not the full screen |
| `native_ui_movie_refused_picture` | no frame decoded for showing (logged once) |
| `native_ui_movie_refused_host` | no display installed (logged once) |
| `native_ui_movie_refused_pass` | the device did not take the pass |
| `native_ui_overlay_movie_frames`, `native_ui_overlay_refused_movie` | items the overlay drew, and those with no upload |

`render.residual`'s registry has the tick `0x00156040` as `movie`, `partial`: the service's stop path (`0x001565a0`, `0x001565e0`) releases the guest's texture, and the start `0x00156090` creates it.

## Frames outside a game view

`0x002232a0` draws what is not a game view: the menus, the loading screen and the movie. The tick `0x00156040` has two callers, `0x0002bc31` and `0x0002c99d` (**Code**; **S**: which views they sit in). Its draws are the overlay producers' ([native-post-hud.md](native-post-hud.md#the-fade-the-bars-and-the-loading-screen), [native-menus-overlay.md](native-menus-overlay.md)) and the movie's. Its own setup (the target, the clear, the viewport, through `0x0002c8a0`) is the view setup's ([below](#the-view-setup-and-view-end)).

The `views`, `outside_view` and `frame_loop` rows reach D3D only through two shared calls, the view setup `0x000132d0(view)` (`ret 4`; callers `0x0002c990` in `0x0002c8a0`, `0x0002cafd` in `0x0002ca00`, `0x00013c60` in the view end, and `0x0002bc1b`, `0x0002c755`, `0x0002c863` in the game views) and the view end `0x00013c20()` (`ret`; the tail of each caller above: `0x0002bc36`, `0x0002bcb8`, `0x0002c76e`, `0x0002c889`, `0x0002c9e4`, `0x0002cb0f`). Both are Rust ([below](#the-view-setup-and-view-end)), so the three rows are `kept`: their work is only what a refused setup's body does.

## The view setup and view end

`render_bridge::view_setup` implements both (`points::RENDER_VIEW_SETUP`, `RENDER_VIEW_END`), placed by `native_targets` with its host. The setup is planned from guest memory with nothing written (`plan`), then made (`apply`): the game-side words, then the device's calls in the body's order through `native_targets::Host`, staged in the guest stack below the call as the body's frame would hold them (**Code** `0x000132d0..0x0001344d`):

| Guest | What it does | The cut |
|---|---|---|
| `rep movsd` | the view's `0x298` bytes to the render context `0x00485600`; `[0x004858b8]` = `0x12` for view kind `+2` = 1, else 0 | the same writes; kind 1 refused (`target`) |
| `0x00015370` (BX 0, its case 0 only) | `[0x004b843c]` = 0, `SetRenderState_StencilEnable(0)`, word `[0x00467010]` = 0 | the writes, `Host::render_state(144, 0)` |
| | `[0x005093b4]` = `0x901`, `[0x005093b8]` = `0x900` | the same writes |
| `0x00012fa0` (ECX `+0x18`, EAX `+0x8c`, byte `+7`, word `+2`), `ret 8` | `0x00012d50` with BL 0: the device's clip range `+0x1c10` = `[0x0045dbc0] * [0x00485adc]`, `+0x1c14` = `[0x00485ae0]`, `+0x1c18` bit 1, the viewport push `0x003f6190`, the view-projection rows; then 48 words to `c[-96]` (`SetVertexShaderConstantNotInlineFast`), shadowed at `0x00407738` unless `[0x00404fe8]` bit 4 | `view_setup::transform` (the rows as `aux_depth::rows` orders them, the view's words from `+0xc4` and `+0x18`, the window's mapping); the clip words, `Host::push_viewport`, `Host::constants(0, 48)` |
| `0x00014bc0(1, [0x004858b8], 0)` | view kind 1's target `0x12`: `0x0001cd30` with the cache's surface `0x0001dcf0(0x12)` (`0x004b4b58 + 0x98 * 0x12`, its `+0x18` when `+0x88` is set, `+0x95` clear and `+0x78` above 0, else null), `[0x00509364]`, flag 0; any other kind's target 0: `[0x0050935c]`, `[0x00509364]`, flag 1. The viewport: 128 square for view kind 2; 640 x 480 on the screen target `0x00025960` (`0x14`, `0x12` or -1) with `+7`; else the window `0x00485648` times the scale (1.0 on the screen target), each `_ftol`'d | `native_targets::cut_select` with `view_setup::cached`; `Host::viewport` with `view_setup::viewport` |
| `0x00014b60`, with `+8` set | flags 1, or `0x61` with the colour `0x00131ed0(0x0048560c)` when `[0x004670cd]` is 0; Z 1.0. At target 0 with `+7`, the prelude `0x00014ac0` first: the device's viewport `+0xee0` saved, `SetViewport(0, 0, 640, 480, 0, 1)`, `Clear(0xf0, 0)`, the saved viewport set again | `native_targets::cut_clear` and `native_targets::prelude`, through `Host::viewport` and `Host::clear` |
| the raw push | method `0x1af0`, four words: 0, `[0x0045dbbc]`, `[0x0045dbc0]`, `[0x0045e3b0]` (`MakeRequestedSpace` at the limit) | the same words at put, `Host::make_space` at the limit |
| `[0x004b8428]` = `0x1b02` | `SetRenderState_FillMode(0x1b02)` | the write, `Host::render_state(139, 0x1b02)` |
| `[0x004670bc]` = 1, `0x00016b10` (ESI `0x00485b48`) | the draw state's reset; `0x00018900` when `+0x24` is set: the 5-byte entries (slot, value) its `+0x11c` names, below slot `0x5c` `SetRenderState_Simple` with the header at `0x0043e6f0` and the slot's word at `0x00407498`, below `0x88` the dirty bits at `0x0043e4d0` or'd into `[0x00407490]` and the word, the rest their setter | the same writes; `view_setup::restore_list`, `Host::simple_state`, `Host::render_state`, after the fill mode |
| with `+0x26c` set | `0x48` words from `+0x14c` to `0x0048574c`; `0x00025a10`: 24 words to `c[-84]` (shadowed at `0x004077f8`), the fog colour `0x00131ed0(+0x168)` to `[0x004b8424]` and `SetRenderState_FogColor`, then `0x00027aa0` | `view_setup::fog`; `Host::constants(12, 24)`, `Host::render_state(138, colour)`; with `+0x1b8` not 0 or -1, `0x00027aa0` (the fog's effect: game code, no D3D) as a guest call, last |

The view end (**Code** `0x00013c20..0x00013cc6`): with `[0x004ba04c]` above 1, `D3DDevice_Clear(0, 0, 0x80, 0, 1.0, 0)` through `Host::clear`; the view stack `[0x0046701c]` popped; a view left (`0x004850d0 + 0x298 * n`), the guest call of the setup above for it, its kind to `[0x004b9ed4]`, its `+0x14c` to `0x004b9ef0` and `+0x18` to the camera `0x004b9da0`, and the guest calls of its camera `0x0002f970` and `0x0002fd90`, which call no D3D entry. It returns with EBX, ESI, EDI and EBP as they came in.

No guest body of the setup or the end runs. A case the cut cannot make panics, named `view_setup: <cause>` or `view_end: <cause>`: `host` (none installed), `device` (a refused call), `unread`, `unwritable`. `render.scene` counts `view_setup_calls` and `view_end_calls`.

The bind and the clear land on the frame graph's targets ([below](#the-graphs-targets)).

**S**: that the FPU's precision makes the product of two floats exact in `0x00131ed0`'s `fistp` (it is in 53 or 64 bits); that nothing reads the guest stack below the call that the cut stages its arguments in.

Three frame loops run outside `0x00223240` (**Code**):

| Loop | View | Then |
|---|---|---|
| `0x0012b6f0` (`ret 4`) | `0x0002ca00` with the callback `0x002233a0` | the frame wait, the present at `0x0012b784` |
| the map load, movie playing (`[0x004e9194]` and `[0x004e9188]` set) | `0x002232a0` at `0x00163a1a` | the frame wait, the present at `0x00163a28` |
| the map load, after it | `0x002232a0` at `0x00163acd` | the frame wait, the present at `0x00163adb` |

Each present asks the publish as the frame step's does (`render_frame::loop_present`, an implementation of `points::RENDER_PRESENT` that acts only for these three return addresses and no readback): shown from the scene, it returns as the guest's `ret 8` with no guest D3D entry, the counter moved; kept, the guest's body runs. `render_frame::Stats` counts both (`loop_published`, `loop_presents`); `publish_*` counts the outcome by name.

`render.residual` names the frame's own calls so none of their work is `unknown` in the frame step or `outside_render` in a loop:

| Row | Entry | Status | What it still holds |
|---|---|---|---|
| `views` | `render_views` `0x0002b5d0` | `kept` | a refused view setup's or view end's body |
| `outside_view` | `0x002232a0` | `kept` | the same, for `0x0002c8a0`'s view |
| `frame_loop` | `0x0012b6f0` | `kept` | the same, for its view `0x0002ca00` |
| `present` | `0x00013f10` | `kept` | a kept frame's `Swap` |

## The final display

At step 4 the frame step asks the publish (`render_frame::publish::present`) first. The present with first argument 0 asks no readback and runs from `0x000140e6` (**Code**):

| Guest | What | The publish |
|---|---|---|
| `0x00013d80` | with an image `[0x005093c8]` set, its CPU blit into the rectangle at `0x005093c0` of the back buffer ([below](#the-back-buffers-blit)) | one overlay picture over the back buffer |
| `0x003f9df0(1, 1)`, `Swap(0)`, `0x003f9df0(1, 1)` | the back buffer scale and the swap | `d3d::Device::publish`: the back buffer to the backend with the frame step's ramp, the swap counted |
| `0x00014110..0x00014194` | `[0x00485acc]` against `[0x00467008]`, byte `[0x00485ad0]` against `[0x0046700c]`; when either differs, the TV encoder option, the flicker filter `0x003f8c70` and the latch | kept |
| `0x00014196..0x000141ae` | the present counter `0x00485aa0`, 64-bit, +1 | `engine::render_loop::tick_writes::present_counter` |

```text
present step
  host's open frame (SceneSink, after the push buffer drains)
    a slot with a guest op ........ kept: publish_kept_<slot>
    a write with no slot .......... kept: publish_kept_untracked
  TV option or flicker changed .... kept: publish_kept_display
  blit not placed ................. kept: publish_kept_border_<cause>
  no host ......................... kept: publish_kept_host
  host's present fails ............ kept: publish_failed
  otherwise ....................... publish_native, 0x00013f10 skipped
```

A kept frame runs the guest's `0x00013f10(0, 0)` as before. Every outcome is counted in `render.scene` by the names above; `<slot>` is the first slot in the frame's order with a guest op (`opaque` to `outside`, as `frame_types::Slot::name`).

- **The open frame.** `d3d::scene_sink` marks each slot it files a guest op in, and a write while no slot is declared, from one present to the next (`scene_sink::open_frame`). A native producer's draw files nothing, so a frame drawn only by the scene's stages and the overlay has no mark. The host (`game::d3d_glue::SinkDrain`) runs the device's push buffer before it reads the marks.
- **The present.** `Device::publish` presents the back buffer the scene drew into through the backend with flags 5, so the scene sink ends the frame and the frame owner takes it as at a `Swap`. The frame-buffer headers do not rotate and no fence is set: no guest op drew into the frame.
- **The vblank.** `Device::publish` counts the swap (`+0x2478`), so the next vertical blank's record carries the swap count and `D3DVBLANK_SWAPDONE`, which `0x00014280` records ([d3d-timing.md](d3d-timing.md#vertical-blank)).
- **The gamma ramp.** The frame step reads the ramp the brightness update (`0x00012c71..0x00012d08`) last loaded into the miniport: device `[0x00407488]`, miniport `+0x1c28`, ramp `+0x1dc + 0x300 * [+0x7e4]`. An all-zero ramp is none loaded. The device's own copy (`Device::gamma_ramp`) is not used for a published frame.
- **The frame wait** is step 3 and runs before the publish either way.
- **Readbacks.** None is asked: the present's four callers (`0x00223296`, `0x0012b784`, `0x00163a28`, `0x00163adb`, **Code**) all push `(0, 0)`, so its surface lock and copy (`0x00013f21..0x000140e4`) never run. The other three are the frame loops [above](#frames-outside-a-game-view), published the same way.

### The back buffer's blit

`0x00013d50` (the disc-error screen's three calls, `0x002134ef..0x0021359d`) sets an image of one byte a pixel `[0x005093c8]`, its rectangle `0x005093c0` (top, left, bottom, right, 16 bits each) and a scale `[0x005093cc]` (`0x55`). The present's `0x00013d80` (**Code** `0x00013d80..0x00013f0e`) then locks the back buffer and writes each byte times the scale, clamped to 0..255, into all four bytes of its pixel, when the rectangle is inside 640 x 480.

`render_frame::blit::read` makes the same pixels as a BGRA picture; `blit::overlay` draws it as one `MovieFrame` item over the 640 x 480 canvas, point-sampled, no blend (picture id `0x00013d80`, its frame a hash of the texels so the upload moves only with the picture). The publish asks `Publish::blit` after the display check, so a kept frame leaves the whole blit to the guest; the host (`hud_overlay::place_blit`) places it unfitted over the bound target. Counted `publish_blit_native`; not placed, the frame keeps the guest's present, `publish_kept_border_<cause>`:

| Cause | When |
|---|---|
| `host` | no GPU overlay |
| `target` | the bound colour is not the device's back buffer `[device + 0x1a14]` |
| `pass` | the device did not take the pass |

**S**: that the guest's own checks on the back buffer (its size against `width * height * 4`, the lock) always pass, since the device's back buffer is the linear guest-size one.

## Rules and limits

- No guest fallback for a movie frame: the native draw skips the copy, so a refusal is a counted blank frame.
- The combiner is `Combiner::Texture` with white corners. **S**: that the begin's combiner reduces to the texture for this record (the text path makes the same assumption).
- `0x004c1a18` is left as it is: the native draw sets no state, so it has none to restore.
- The frame step holds a named continuation across each guest call of the frame, as `sim-frame` does.
- A published frame skips `0x003f9df0(1, 1)`. **S**: that nothing sets another back buffer scale for the next frame to inherit.
- Without a kernel clock, the vertical blanks a published swap counts (`Device::set_vblanks_per_swap`) are delivered at the next `Swap`'s return, not at the publish.
- The frame tail `0x00013b90` runs inside `render_views` before the present, and does nothing unless `0x0001fd20` returns a record with `+0xa4` above `[0x0045dbd8]`. A native tail's overlay files nothing; a kept tail's native fans file nothing, and a refused fan carries `overlay` and keeps the frame ([below](#the-frame-tail)).

## The frame tail

`0x00013b90` (`ret`, **Code**) is a freeze-frame cross-fade: the frame is copied once into target `0x12` and then blended over the frames after it while the fade lasts.

| Guest | What | D3D entries |
|---|---|---|
| `0x0001fd20` | the record `[0x005093e0]` (`+0xac` live), its fade `+0xa4` from the clock `[0x004858a0]` and `+0xb8`, `+0xbc`; no calls | none |
| `+0xa4` at or below `[0x0045dbd8]` | return | none |
| `+0xa0` set, `[0x004b5690]` 0 | `0x0001dc40(640, 480)`, EBX `0x12`: the cache's allocation `0x0012d400` (evicts with `0x0012c600`; when full, within the budget `0x00223bb0`, `CDevice_KickOff` `0x003fac30` and `SwitchToThread`), a fill `0x002d15da`, the header `0x0001df20` | `KickOff` when the cache is full |
| `+0xa0` set, target `0x12` live (`0x0001de20`) | the copy `0x000351a0([0x004858b8], 0x12)`, then `0x0001fd00` (`+0xa0` = 0); refused: `0x0001fcd0` (the record zeroed, `[0x004b569d]` = 1 when `[0x004b5690]` is set) | the target switch `0x00014bc0` (through the native select `0x0001cd30`, then `0x003fdb20` and `SetViewport` `0x003f8100`), `SetTexture` through `0x00014f60`, render states `0x000222d0` (`0x3b`, `0x3c`, `0x90`), `0x003f59b0(0)`, `0x003f53d0(0)`, the combiner `0x0001ccf0`, the vertex program `0x0001c590`, then `0x00034a90`'s states and fan |
| always, after | the composite `0x00022070(+0xa4, +0xa8)`: an alpha from both through `0x00372d48` (`pow`), packed `<< 24` into the PSDEF constant `0x00485014`; target `0x12` over `[0x004858b8]` with blend mode 7 | `SetTexture`, the combiner, `0x00034a90`'s states and fan |

`render_bridge::frame_tail` implements `0x00013b90` (`RENDER_FRAME_TAIL`, placed by `native_targets`) with no guest D3D entry:

| Guest | Rust |
|---|---|
| `0x0001fd20` | the guest's call, EAX 0 (it has no D3D entry) |
| no record, or `+0xa4` at or below `[0x0045dbd8]` | return |
| `0x0001dc40`, `0x0001de20`: target `0x12` | never allocated; the cache's drain does not run |
| the copy `0x000351a0` | an `OverlayKind::Hold` of texture `0x12`: the bound target copied whole into a texture of its size and format, kept across frames in the overlay's residency (`ui_gpu::OverlayResidency::hold`), one per target copy |
| `0x0001fd00`, `0x0001fcd0` | the same writes: a placed pass is the copy's success, a pass the host did not place its refusal |
| the composite `0x00022070` | a quad over Halo's window `0x004b9dd0` sampling `OverlayTexture::Held`, each corner its own pixel; the guest's combiner block (`frame_tail::composite_words`, alpha from `frame_tail::composite_alpha`), blend mode 7, all four channels written |

`hud_overlay::place_tail` places the list as one unfitted overlay pass over the bound target. A frame with no copy samples the texture an earlier frame kept.

No guest body of the tail runs. A tail it does not cover panics, named `frame_tail: <cause>`:

| Cause | When |
|---|---|
| `host` | no host installed |
| `target` | `[0x004858b8]` is not target 0, or target 0's colour `[0x0050935c]` is not the bound one (the select's cache `[0x0051f3f4]`) |
| `size` | target 0 has no size |
| `window` | the window is empty or outside target 0 |
| `blend` | blend mode 7 is past the tables or names the constant colour |
| `unread` | a word that does not read |

`frame_tail_calls`, `frame_tail_native`, `frame_tail_copies` and `frame_tail_composites` count the tails. `frame_tail_refused_pass` counts a pass the host did not place (a pending copy is then refused as `0x0001fcd0` refuses it); `frame_tail_refused_hold` a copy with no texture or over a target that cannot be copied, `frame_tail_refused_held` a composite whose held texture or combiner the GPU did not take.

**S**: that nothing else reads target `0x12`; that the state the guest's copy and composite leave (texture stage 0, the stage caches, the viewport) is not read before the next frame sets its own; that a fade whose copy was native is not composited by a kept tail (it would sample target `0x12`, never filled).

## The frame's finish

`0x00013cd0` (`ret`, three callers: `render_views` `0x0002b6f7` after the tail, `0x0002c9e9`, `0x0002cb14`) is Rust with the native targets (`native_targets::cut_finish`, `points::RENDER_FRAME_FINISH`), with no guest D3D entry:

| Guest | The cut |
|---|---|
| `[0x0051f3d8..0x0051f3e8]` = 0, `[0x00467004]` = -1 | the same writes |
| `0x0001cf50`: `D3DDevice_SetTexture(stage, 0)` for each stage whose `[0x0051f3c8 + 4 * stage]` is not 0, then that word 0 | `Device::set_texture` through the host, then the word |
| `[0x00408338]` = 0; the index buffer `[device + 0x7a0]` loses `0x80000`, `D3D_DestroyResource` when no reference is left; `+0x7a0` and `+0x1c` 0 | the same writes; the last reference (its destroy) is not made here |
| tail jump to `D3DDevice_SetPixelShader(0)` | `Device::set_pixel_shader` through the host |

No guest body of the finish runs. A finish the cut cannot make panics, named `frame_finish: <cause>`: `host`, `device` (a refused set), `release` (the last reference), `unread`, `unwritable`. `render.scene` counts `frame_finish_calls`.

## The water end

`0x0003c3c0` (two stack arguments, `ret 8`, AL 1), the late passes' call after layer 6 (`0x0002c132`), is Rust with the native renderer (`render_bridge::water_end`, placed by `native-water`, `points::WATER_TEARDOWN`). The late passes pass `(1, al)`, AL the setup wrapper `0x0003ba90`'s, when bit 4 of `0x004b9fe0` is clear, and `(0, 0)` when it is set. No guest body runs.

| Guest (Code, `0x0003c3c0..0x0003c3f6`) | Rust |
|---|---|
| the active BSP `[0x004e0348]`'s water definition count `+0x214` at or below 0 | `End::NoWater`: return |
| the first definition `[bsp + 0x218]`'s `+0x7c` equal (`ucomiss`) to `[0x0045dbd8]`, 0.0 | `End::Still`: return |
| the first argument's byte 0 | `End::NotDrawn`: return |
| otherwise the ocean | `End::Ocean`: the ocean's steps |

A gate's return leaves EAX the BSP pointer under AL 1 and ECX the count, as the guest's does.

The ocean (Code, `0x0003c3ff..0x0003c632`) is `render_bridge::water_end::ocean`. `Ocean::next` makes the body's steps in its order and hands back each helper the body calls, with its registers and stack arguments; the water end calls it as a guest call whose continuation steps again, then returns as the body does (`mov al, 1`, `ret 8`, the caller's EBX, ESI, EDI and EBP).

| Step | Guest | Made by |
|---|---|---|
| grid | `0x0003ca90(eax = definition)`: the camera-attached grid's rows and aspect ([high-resolution-widescreen.md](high-resolution-widescreen.md)) | guest call |
| occlusion gate | `0x0003c270(definition)`: the water's entry (class 5) in the flare visibility array (`0x000209b0`), closed when its test two frames back saw at most `[0x0045dbd8]` of it; then this frame's test, the walker `0x00020bb0(view, 0x20, &entry, 1, 1, 0x0003bf20)` over the grid's four corners at `0x004c1b08` | Rust (`ocean::gate`, `Device::visible`) over the native flare query: `native_flare_query::register`, the byte and the read set's bit, `walk_listed` with the corners' screen rectangle at the nearest corner's depth in tiles of 256 pixels (never fewer samples than the corners' quad passes; a corner behind the eye is all seen with no pass); the reader `0x00020770` carries the count as for a flare |
| residency gate | `0x0012de70(ecx = definition + 0x10, eax = 3)`: a loaded block (`+0x1e`) stamps its cache entry, an empty one (`+8` at most 0, `+0xc` below 0) is not resident | Rust (`ocean::cache::resident`); a block to stream is `texture_cold` |
| shader | `[definition + 4]` (-1: return) to its implementation's flags `+0x118` | Rust (`ocean::shader`); a shader of group `MTRP` or `3trp` is `shader_group` |
| copies | bit 20, `0x0003bea0` (the view's target into target `0x16`); bit 19, `0x0003bd00(definition + 0x84, 0x00131ed0(definition + 0x58))` (a pass into target `0x15`) | guest calls; their binds reach `Game(0x16)` and `Game(0x15)` through `surface_for` |
| alpha clear | the second argument's byte set: `0x00014b60(eax = 0x10, 0x00ffffff, 0, 0)`, the bound target's alpha to 0 | `native_targets::cut_clear` |
| vertex type, grid constants, stream | `0x0001cbb0(eax = 1, edi = the implementation's `+0x100`, 1)` (the kind `0x0001cb20`, the program select `0x0001c590` on the stream record `0x0051f0f0`), `0x000151c0` (`c[0x32]`, 12 words of `0x004c1b38`), `0x0001cda0(ecx = [geometry + 0x3c])` (`0x0001c620`: the record's next slot) | Rust over `native_targets::Host` (`ocean::state`; `vertex_program`) |
| stream commit | `0x0001c710(0x0051f0f0)`: a dirty format rebuilt by `0x0001c330` (each register's stream, offset and type by the vertex type's semantics); dirty inputs set (`SetVertexShaderInput`, the static shader `0x00408518` with the format copied in); a dirty program or format selected (`SelectVertexShaderDirect`); the count cleared | Rust over `native_targets::Host` (`ocean::state::stream_commit`; `vertex_input`, `select_vertex_program`) |
| state block, decompression | the 30 words at `0x004c1a90` zeroed, `0x0001bd50(eax = 0x004c1a90)` (the cached block, five dirty words or-ed), `0x0001cdd0(eax = 0, 0)` (the identity block to `c[0xaa]` while `[0x00485af1]` is set) | Rust (`ocean::state`); a compressed type is `position_compression` |
| shader begin | `0x0001bbf0(0, 3, 0, 0, 100000.0)`, EDI the shader | guest call |
| viewport, structure, stage commit | `0x00012d50(eax = 0, bl = 0, 0)` (the device's depth words, the viewport push, the projection of `0x0048568c` to `c[0..4]` while `[0x0055e6bc]` is set), `0x0003bf00(eax = definition)` (`0x0003c650`, `0x0003d000`: `engine::render_loop::structure_parameters::upload`, made as stages 1 to 3's bump matrices, then `c[21]`, `c[23]`, `c[24]`, the fog colour, `c[22]`, `c[25]`), the quad wrapper's `0x0001cf50` (each changed stage's `SetTexture`) | Rust (`ocean::state`; `registers`, `texture_stage_state`) |
| quads | `0x0001c8a0(8, [geometry + 0x24], [geometry + 0x20])`, geometry `[definition + 0xc]`: a quad list of `[geometry + 0x20]` indices at `[geometry + 0x24]` | `scene_step::water_composite`, then `world_layers_gpu::IndexedPass::water_quads` through the transparents' walk host (`Stage::Water`) |
| stamps | each bitmap of the shader's `+0x24` block (stride `0xc`, -1 skipped), each non-null image (`+0x48`, stride `0x74`): `0x0003bcb0(eax = image)`, done for an image past the texture stamp `[0x004e6488]`, else the texture cache's lookup `0x0012ccf0(edx = 0xe)` and its request `0x0012ce00` | Rust (`ocean::stamps`, `ocean::cache::stamp`): the lookup's level-0 entry stamped and, loaded, its texture taken; an image to load is `texture_request` |

A step the stage cannot make panics, `water ocean: <cause>`: `unread`, `unwritable`, `shader_group`, `host` (no native targets host), `clear` (the device refused it), `device` (a draw-state call refused), `vertex_kind` (a kind past `0x0001cb20`'s table), `position_compression`, `structure` (the upload's float model refused), `quads` (no keyed view's composite), `draw` (the walk host refused the quads), `flare_host` (no native flare query host), `placed` (the occlusion gate's count pass not placed), `vertex_declaration` (a semantic or register past `0x0001c330`'s tables), `vertex_format` (a format code past `0x00035790`'s), `texture_cold`, `texture_request`. A gate word that does not read is `water end: unread`.

`render.scene` counts `water_end_calls` and `water_ocean_calls`.

Tests: `the_gates_end_with_no_work`, `a_drawn_definition_is_the_ocean`, `the_return_leaves_the_guests_registers`, `an_unported_ocean_step_is_named`, `an_unread_bsp_panics` (`water_end.rs`); `the_ocean_makes_the_bodys_steps_in_order`, `the_shader_flags_pick_the_copies`, `a_closed_gate_goes_to_the_stamps`, `the_stamps_skip_what_the_guest_skips`, `unmade_steps_are_named` (`water_end/ocean/tests.rs`); `the_first_call_opens_and_places_the_test`, `the_byte_read_two_frames_back_closes_the_gate`, `an_unbounded_or_empty_rectangle_needs_no_pass`, `no_entry_closes_the_gate` (`water_end/ocean/gate/tests.rs`); `a_block_is_resident_empty_or_cold`, `a_stamp_takes_the_cached_texture_or_names_the_request`, `an_uncached_image_is_its_texture` (`water_end/ocean/cache/tests.rs`); `register_finds_the_key_or_takes_a_free_entry`, `a_listed_test_sums_its_quads` (`native_flare_query.rs`); `the_commit_sets_the_stages_that_changed`, `a_vertex_type_loads_its_program_once`, `the_vertex_kind_follows_the_jump_table`, `a_stream_fills_the_next_slot`, `the_state_block_ors_its_masks_once`, `the_identity_decompression_uploads_when_pending`, `the_grid_constants_upload_twelve_words`, `the_projection_is_the_sse_sums`, `the_viewport_pushes_and_uploads_the_projection_once`, `the_structure_upload_is_made_in_the_guests_order`, `the_stream_commit_builds_the_format_then_sets_the_inputs_and_program`, `the_stream_commit_without_streams_sets_no_inputs`, `a_tags_vertex_type_maps_its_semantics` (`water_end/ocean/state/tests.rs`).

## The graph's targets

Every render target and depth surface the device makes is the frame graph's, keyed by what it is to the game, not by the guest header or the frame's `TargetRef`. A `TargetRef::Aux(n)` is numbered by first use in its frame, so `Aux(0)` is the freeze frame's target in one frame and the post colour's in the next; a `TargetRole` is the same in every frame.

| Role | What | Made at |
|---|---|---|
| `Display(n)` | swap-chain buffer `n` (the back buffers, then the display surface) | `Direct3D_CreateDevice` |
| `DisplayDepth` | the device's depth buffer | `Direct3D_CreateDevice` |
| `Game(n)` | entry `n` of Halo's target table `0x004b4b58` (stride `0x98`): its surface header `+0x18` | the first bind of the header |
| `GameLevel(n)` | a level of entry `n`'s texture (`+4`) bound as a target | the first bind of the level |

```mermaid
flowchart LR
  G[guest bind or clear, header] --> D[d3d::Device::surface_for]
  D -->|DeviceConfig::roles| R[native_targets::role]
  R --> T[Backend::create_target role]
  T --> S[SceneSink: the frame's roles]
  S --> W[ThreadedBackend worker]
  W --> ST[frame_graph::targets::Store]
  ST --> O[targets::Owned, one surface per role]
  O -->|TargetStore::map at the present| P[the replay's targets]
```

- **The role table.** `render_bridge::native_targets::role(header, parent)` names the table's entries (`TARGET_COUNT` of them, **S**: that there are no more). Boot's glue sets it as `d3d::DeviceConfig::roles` with the native renderer on. The device asks it in `surface_for` and `rtt_surface`, and makes the surface with `Backend::create_target`; a header it does not name panics, `d3d targets: <header> (texture <parent>) has no role`. One role has one live device surface: a second panics, `d3d targets: two live surfaces for <role>`. A rewritten header gives its old surface back (`Backend::release_target`) before the new one is made.
- **The frame's roles.** The scene sink keeps each surface's role and the recorder files them per frame, `ReplayFrame::roles` (`ReplayFrame::role`), after the present's renaming. `targets::declare` puts each target's role in its `Declared`.
- **The store.** `ThreadedBackend::with_target_store` installs `frame_graph::targets::Store` (a `d3d::threaded::owner::TargetStore`) on the render thread, so the worker's `create_target` and `release_target` go to it. `targets::Owned` holds one surface per role across frames and devices: the same shape keeps its surface, a new shape makes another. The device's handles for a role come and go; a surface a new shape replaces while a handle to it is live is retired and freed when that handle comes back, since commands queued before the new one can still draw on it. A role's surface outlives its handle, so a device made again after a teardown gets the same surfaces.
- **The replay.** At the present the worker takes the graph's map, `TargetStore::map`: each declared target's surface by its role. The guest's commands drew on the same surfaces, so a frame's carried work and its replay share them. A declared target with no role, or a role the store does not hold, panics, `graph targets: <cause>`.
- **The post and the screen effects.** The colour pass `0x000214f0` and the screen effects `0x0002d140` are still guest work. Their binds, clears and external slots (target 1, the stand-ins `0x19` to `0x1b`, the bound colour) reach the device through `surface_for`, so each is drawn on its role's graph surface. The water end's ocean clears the bound target's alpha with `native_targets::cut_clear` and draws its quads natively; its copies are guest calls whose binds go the same way ("The water end").

- **The water.** The water teardown `0x0003c3c0` with its alpha clear `0x0003c515` is still guest work. Its binds and clears reach the device through `surface_for`, so each is drawn on its role's graph surface.
- **The post.** The colour pass and the screen effects name their slots by role ([below](#the-post-and-the-screen-effects)).

`Owned` counts `graph_targets_made`, `_kept`, `_released` and `graph_targets_refused_<cause>` (`unsized`, `unroled`, `allocation`) for a frame it owns directly (`Owned::own`). Without a role table (the `d3d` crate's own tests) the device makes each target as any surface, and without a store the worker makes them on its backend.

Tests: `a_game_target_is_named_by_its_table_entry` (`native_targets.rs`), `each_frame_names_its_targets_roles` (`d3d/src/scene_sink/replay.rs`), `a_role_keeps_its_surface_when_its_number_moves`, `the_store_frees_a_replaced_surface_when_its_handle_comes_back` and `the_graph_owns_its_targets_across_frames` (`frame_graph/targets.rs`).

## The post and the screen effects

The colour pass `0x000214f0` and the screen effect `0x0002d140` are Rust bodies (`render_bridge::native_post`): no guest body runs, so none of their binds, clears or draws reach the device. Each drawn call is one native pass of `Stage::Post` (`Device::prepare_external_pass`), its slots named by role and resolved through `surface_for` and the role table:

| Pass | View (target 0's colour) | Roles |
|---|---|---|
| Glow | read and written | `Game(7)`, `Game(8)` |
| Screen call | read; written by a pass into it | `Game(1)`, `Game(0x19)` to `Game(0x1b)` (the stand-ins) |

The bodies make the guest's writes the rest of the game reads: target 1 and the stand-ins' sizes in the table and both headers (`0x0001ddd0`), the saved sizes back at the end, the template cache reset, the results and the end flag. Raster state is the plan's, not a guest draw's. A call the bodies or the passes do not cover panics, `post: <cause>`. The page: [native-post.md](native-post.md).

Tests: `a_resize_writes_the_table_and_both_headers`, `the_end_writes_the_saved_sizes_back_in_the_originals_order` (`render-bridge/tests/native_post.rs`), `post_slots_name_the_view_and_table_roles` (`native_post_glue.rs`).

## Not verified

- A movie on screen with the native display: its placement against the guest's draw at 1x and at render scale.
- That `d3d_entries` grow only under `outside_render` from boot to gameplay and back (the R8 exit); it needs R1 to R7.
- The view setup in a game: its frames on screen against the guest's, a view of kind 1 and a scaled view among them.
- The graph's targets in a game: which headers panic `d3d targets: <header> has no role`, and whether two levels of one entry's texture are ever bound at once.
- The frame tail in a game: a screen effect's freeze frame on screen against the guest's, at 1x and at render scale, and `frame_tail_native` against `frame_tail_calls`; a tail with a view of kind 1 panics (`target`).
- A frame loop's published frame on screen, and its `loop_published` against `loop_presents` over a map load.
- The frame's finish in a game: whether a `release` panics.
- The water end's ocean in a game (Delta Towers' bay is one): its quads on screen against the guest's, its copies on `Game(0x16)` and `Game(0x15)`, and which steps panic `water ocean: <cause>`.
- How many frames publish in a game: the `publish_*` counters over the census corpus.
- A published frame on screen: its pixels and brightness against the guest's present.
- Snapshot quiescence with the frame step's continuations.

## Related

- [retire-d3d.md](retire-d3d.md): the plan, the census and the other slices.
- [bink-hle.md](../audio/bink-hle.md): the movie wrapper and the Bink overrides.
- [native-renderer.md](native-renderer.md): the overlay stage.
- [render-loop.md](render-loop.md): the frame's call tree.
