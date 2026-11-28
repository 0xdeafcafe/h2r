# Retiring D3D: movies, frames outside a game view and the display (R8)

Status: Partly built. The frame step, the movie's picture, the publish (in the frame step and the three frame loops outside it), the frame tail, the frame's finish and the view setup and view end are native, the frame wait calls no D3D entry, and every render target and depth surface is the frame graph's, by role; a frame tail or view setup it does not cover and a frame with a carried slot keep the guest's D3D calls, each under a named `render.residual` row.
Code: `crates/rewrites/render-bridge/src/render_frame.rs` (the frame step), `native_targets.rs` (the finish), `render_frame/publish.rs`, `d3d::Device::publish` and `game::d3d_glue::SinkDrain` (the publish), `crates/platform/bink-hle/src/display.rs` and `crates/rewrites/render-bridge/src/native_movie.rs` (the movie), `crates/extensions/game/src/movie_display_glue.rs` and `hud_overlay::place_movie` (the glue), `render_frame::loop_present` (the frame loops' present), `frame_tail.rs` with `hud_overlay::place_tail` and `frame_scene::OverlayKind::Hold` (the frame tail), `view_setup.rs` (the view setup and view end), `native-render/src/frame_graph/targets.rs`, `native_targets::role` and `game::native_targets_glue::target_ref` (the graph's targets), `residual_census::REGISTRY` (the rows). Tests: in each, `the_frame_finish_makes_no_d3d_entry` (`native_targets.rs`), `a_frame_with_a_screen_effect_makes_no_d3d_entry` and `uncovered_tails_are_refused_by_name` (`frame_tail.rs`), `the_view_setup_makes_no_d3d_entry`, `uncovered_setups_are_refused_by_name` and `a_check_compares_the_guest_bodys_state` (`view_setup/tests.rs`), `a_game_target_is_its_guest_size_at_the_scale` (`frame_graph/targets.rs`), `a_held_texture_survives_into_the_next_frame` (`native-render/src/ui_gpu.rs`), `the_frame_wait_is_outside_the_render` and `the_frames_own_calls_are_named_rows_inside_and_outside_the_frame_step` (`residual_census.rs`), `a_loop_frame_from_the_scene_is_published_with_no_d3d_entry` (`render_frame.rs`), and `a_native_display_takes_the_picture_and_leaves_the_guest_texture_alone` (`crates/platform/bink-hle/tests/synthetic.rs`).
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

The frame wait is outside the render: the wait (`d3d::FrameWait`, a Rust override) calls no D3D entry, and the vblank callback `0x00014280` it delivers only counts. What other threads call while it yields is theirs, not the frame's.

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

The tick `0x00156040` is the movie's: the service's stop path (`0x001565a0`, `0x001565e0`) releases the guest's texture, and the start `0x00156090` creates it.

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
| with `+0x26c` set | `0x48` words from `+0x14c` to `0x0048574c`; `0x00025a10`: 24 words to `c[-84]` (shadowed at `0x004077f8`), the fog colour `0x00131ed0(+0x168)` to `[0x004b8424]` and `SetRenderState_FogColor`, then `0x00027aa0` | `view_setup::fog`; `Host::constants(12, 24)`, `Host::render_state(138, colour)`; with `+0x1b8` not 0 or -1, `0x00027aa0` (the fog's effect: game code, no D3D) through `fog_effect::run`, last |

The view end (**Code** `0x00013c20..0x00013cc6`): with `[0x004ba04c]` above 1, `D3DDevice_Clear(0, 0, 0x80, 0, 1.0, 0)` through `Host::clear`; the view stack `[0x0046701c]` popped; a view left (`0x004850d0 + 0x298 * n`), the setup above for it in Rust (`view_setup::setup`), its kind to `[0x004b9ed4]`, its `+0x14c` to `0x004b9ef0` and `+0x18` to the camera `0x004b9da0`, and its camera `0x0002f970` and `0x0002fd90` (`view_camera::update`, `view_camera::finish`). No guest body runs; it returns with EAX, ECX, EDX and the XMM registers as the camera leaves them.

`render_bridge::view_camera` ports the end's camera bit for bit, the callees included: `update` is `0x0002f970` (the window over the viewport to the 16-byte local, the lens byte `[0x004ba019]`, the `+0x58` lens or `+0x68` shift) and `finish` is `0x0002fd90` (the projection `0x0002fb70` through `engine::view_projection`, the axes and position to `0x004b9e48`, their inverse `0x00141590` to `0x004b9e14`, the offsets and the clip rows to `+0xbc`). Each writes guest memory in the body's order, the local included, and leaves the bodies' dead frames alone. SSE follows x86's NaN rules; the x87 parts run at double precision, where `f64` is exact for these operands, and an x87 NaN is refused (`x87`). Both need MXCSR `0x1f80` (sticky flags aside) and `finish` the x87 control word `0x027f`, else `simd` or `x87`.

`render_bridge::fog_effect::run` ports the fog's effect `0x00027aa0` bit for bit, its callees included: the wind `0x00028580` with its normalise `0x0002b400`, the transforms' inverse `0x00141590` and product `0x00142a60`, and the CRT's `floor` `0x00321099`, `_ftol` `0x00322b0c` and `pow` `0x00372d48`. SSE runs on the `simd` model under the caller's MXCSR, the x87 on the `x87` model under its control word from an empty stack. Every write is staged and made at the end in the body's order, so a refusal writes nothing: `x87` (an unmasked x87 exception, or one the model cannot make), `simd` (an unmasked SSE exception raised), `floor_nan` (a NaN to `floor`, the CRT's error path), `idiv` (a layer count of 0), `unread`, `unwritable`.

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

The frame's own calls:

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
| `0x0001fd20` | `fade_record::update`, EAX 0: every write and register as the guest's body makes them, the stack slots and x87 marks included. The x87 blends run on the x87 crate's model under the caller's control word, the SSE under its MXCSR with x86's NaN rules and sticky flags; an operand either cannot make exactly panics (`x87`, `simd`) |
| no record, or `+0xa4` at or below `[0x0045dbd8]` | return |
| `0x0001dc40`, `0x0001de20`: target `0x12` | never allocated; the cache's drain does not run |
| the copy `0x000351a0` | an `OverlayKind::Hold` of texture `0x12`: the bound target copied whole into a texture of its size and format, kept across frames in the overlay's residency (`ui_gpu::OverlayResidency::hold`), one per target copy |
| `0x0001fd00`, `0x0001fcd0` | the same writes: a placed pass is the copy's success, a pass the host did not place its refusal |
| the composite `0x00022070` | a quad over Halo's window `0x004b9dd0` sampling `OverlayTexture::Held`, each corner its own pixel; the guest's combiner block (`frame_tail::composite_words`, alpha from `frame_tail::composite_alpha`), blend mode 7, all four channels written |

`hud_overlay::place_tail` places the list as one unfitted overlay pass over the bound target. A frame with no copy samples the texture an earlier frame kept.

No guest body of the tail runs. A tail it does not cover panics, named `frame_tail: <cause>`:

|---|---|
| `host` | no host installed |
| `target` | `[0x004858b8]` is not target 0, or target 0's colour `[0x0050935c]` is not the bound one (the select's cache `[0x0051f3f4]`) |
| `size` | target 0 has no size |
| `window` | the window is empty or outside target 0 |
| `blend` | blend mode 7 is past the tables or names the constant colour |
| `unread` | a word that does not read |
| `x87`, `simd` | `0x0001fd20`'s x87 or SSE meets an operand or mode its model cannot make exactly (an unmasked exception, a NaN beside a denormal, a tiny inexact product) |
| `memory` | `0x0001fd20` reads or writes an unmapped word |

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

The ocean (Code, `0x0003c3ff..0x0003c632`) is `render_bridge::water_end::ocean::run`, called inline by the water end, which then returns as the body does (`mov al, 1`, `ret 8`, the caller's EBX, ESI, EDI and EBP). It makes no guest call: the body's draw-state helpers, its shader begin `0x0001bbf0` and its quad call are replaced by one native draw of the ocean's pass compiled from the map.

| Step | Guest | Made by |
|---|---|---|
| grid | `0x0003ca90(eax = definition)` with `0x0003c9a0` and `0x00030bf0`: a zero `+0x7c` returns; else the camera's basis (`0x004c1b68`), the grid's model rows (`0x004c1b38`) and visibility quad (`0x004c1b08`) from the raster's aspect ([high-resolution-widescreen.md](high-resolution-widescreen.md)) | Rust (`engine::ocean_grid::grid`, SSE and x87 models under the caller's MXCSR and control word), the aspect widened by `ocean::grid_lens` (render-aspect's view lens) |
| occlusion gate | `0x0003c270(definition)`: the water's entry (class 5) in the flare visibility array (`0x000209b0`), closed when its test two frames back saw at most `[0x0045dbd8]` of it; then this frame's test, the walker `0x00020bb0(view, 0x20, &entry, 1, 1, 0x0003bf20)` over the grid's four corners at `0x004c1b08` | Rust (`ocean::gate`, `Device::visible`) over the native flare query: `native_flare_query::register`, the byte and the read set's bit, `walk_listed` with the corners' screen rectangle at the nearest corner's depth in tiles of 256 pixels (never fewer samples than the corners' quad passes; a corner behind the eye is all seen with no pass); the reader `0x00020770` carries the count as for a flare |
| residency gate | `0x0012de70(ecx = definition + 0x10, eax = 3)`: a loaded block (`+0x1e`) stamps its cache entry, an empty one (`+8` at most 0, `+0xc` below 0) is not resident | Rust (`ocean::cache::resident`); a block still loading is not resident, so the ocean waits a frame |
| shader | `[definition + 4]` (-1: return) to its implementation's flags `+0x118` | Rust (`ocean::shader`); for a shader of group `MTRP` or `3trp`, `0x00137bd0` hands the index to the group's object (`0x00479868`, `0x00479874`: index at `+4`, data at `+8`) and its template block is 3trp's data `+0x90`, or the `+0x24` of the shader MTRP's data `+0x1c` names (`ocean::template_block`) |
| copy | bit 20, `0x0003bea0`: the frame's first call (`[0x00509430]` clear) writes the open shader's restore list and clears the shader cache (`0x00016b10`), copies the bound colour into target `0x16` (`0x000352e0(0x16, 1)`; the BSP's water count above 0, always so in the ocean) and sets `[0x00509430]` and the draw byte `[0x004670bc]`; later calls return | Rust (`ocean::first_copy`): the restore list through `Host::restore`, the copy a one-slot `Stage::Water` pass on `Game(0x16)` (`snapshot_gpu::Snapshots::into_target`, as the layers' structure copy) ordered ahead of the quads through `Host::copy_view`, the flags and `clear_cache` written as `FirstCopy::finish` does |
| pass | bit 19, `0x0003bd00([definition + 0x84], 0x00131ed0(definition + 0x58))`: the draw state reset (`0x00016b10`); target `0x15` bound with no depth (`0x00014bc0`, case `0x00014d25`) on its whole surface and cleared to the colour; the context's camera mirrored (its third axis negated, its point's height `h × [0x0045dc0c] − z`, its inverse made again by `0x00141590`) with a 512 by 256 window, uploaded by `0x00012fa0`; the sky `0x0003ede0`; the view's target bound again and its constants made again | Rust (`ocean::reflection::pass`): the bind and clear through `native_targets::Host` (`surface_size` for the viewport), the constants from `view_setup::transform`, the inverse `engine::light_frusta::inverse_words`, the sky the native cut's replay on the bound target (`native_sky::draw_cut`, `Device::sky`) |
| alpha clear | the second argument's byte set: `0x00014b60(eax = 0x10, 0x00ffffff, 0, 0)`, the bound target's alpha to 0 | `native_targets::cut_clear` |
| draw | the vertex type, the grid rows to `c[0x32]`, the stream (`0x0001cbb0`, `0x000151c0`, `0x0001cda0`, `0x0001c710`), the state block, the shader begin `0x0001bbf0(0, 3, 0, 0, 100000.0)`, the viewport and structure upload (`0x00012d50`, `0x0003bf00`), and the quads `0x0001c8a0(8, [geometry + 0x24], [geometry + 0x20])`, geometry `[definition + 0xc]` | Rust: `ocean::draw_of` reads the frame's words (`scene_extract::late::ocean`: the rows at `0x004c1b38`, the definition's parameter block, the flags) and the stream record `[geometry + 0x3c]`; `scene_step::set_ocean` files `frame_scene::late::Ocean` and the quads in the keyed view; `OceanHost::draw` (`native_ocean_glue`) draws them |
| stamps | each bitmap of the shader's `+0x24` block (stride `0xc`, -1 skipped), each non-null image (`+0x48`, stride `0x74`): `0x0003bcb0(eax = image)`, done for an image past the texture stamp `[0x004e6488]`, else the texture cache's lookup `0x0012ccf0(edx = 0xe)` and its request `0x0012ce00` | Rust (`ocean::stamps`, `ocean::cache::stamp`): the lookup's level-0 entry stamped and, loaded, its texture taken; an image the cache does not hold yet is left for a later frame (the native draw takes its bitmaps from the map) |

The draw is `native_render::ocean_gpu`:

| Piece | Made from |
|---|---|
| pass | the water definition's shader (`native_render::load::ocean_shader`, the BSP's `+0x214`/`+0x218`), layer 3 pass 0, implementation 0, compiled by `object_program::object_layer_pass` at the world's load (`native_ocean_glue::load`) and for the stream's format (`object_streams::StreamFormats`, the record's type byte) at the first draw |
| vertex program | the pass's `vrtx`, rigid classification |
| `c[0..18]`, W clip | the view's, from the device |
| `c[50..53]` | the grid's rows |
| `c[21..25]`, bump matrices of stages 1 to 3, fog colour | the definition's parameter block (`frame_scene::StructureParameters`) |
| stages | map bitmaps; engine textures 14 and 15 the rasterizer's; 19 and 20 targets `0x15` and `0x16`, the frame graph's surfaces for `Game(0x15)` and `Game(0x16)` sampled in place, in texels of their guest size (`ocean_gpu::sampled_target`, `Stage::samples`; ["The graph's targets"](#the-graphs-targets)) |
| geometry | the stream's vertices (the buffer's `+4` in the contiguous alias, plus the record's `+0x10`) and the quads as triangles on the Xbox diagonal (`ocean_gpu::quad_triangles`) |

It is a `Stage::Water` scene stage on the bound target (`Device::external_stage_now`), its pass and grid kept for a replay.

A step the stage cannot make panics, `water ocean: <cause>`; the copies' causes read `water ocean copy: <cause>`: `cache` (the open shader's list does not read), `restore` (a state the device does not take), `target` (target `0x16` has no live header, or the device refused the copy pass), `reflection` (target `0x15` has no live surface or size), `bind`, `viewport`, `clear`, `constants` (the device refused the reflection's call), `sky` (the native sky drew nothing: no complete guest pass kept, or the device refused it; the log names why), `view_target` (the view's target is not 0 or `0x12`). The others are `unread`, `unwritable`, `host` (no native targets host, or no ocean host), `clear` (the device refused it), `structure` (the parameter block does not read), `quads` (no keyed view's composite), `world` (no loaded world), `program` (the pass did not load, compile or upload; the log names why), `stream` (the stream's format or vertices do not read), `palette` (a bitmap stage's palette changed since the load), `target` (target `0x15` or `0x16` has no live header or size, or aliases an attachment), `view`, `uniforms`, `pipeline`, `textures`, `grid` (the recording could not make them), `device`, `draw` (the device refused the stage), `flare_host` (no native flare query host), `placed` (the occlusion gate's count pass not placed). The grid's panic `water ocean grid: <cause>`: `unread`, `unwritable`, `simd` (an unmasked SSE exception), `x87` (an unmasked x87 exception, or an operation the model stops on). A gate word that does not read is `water end: unread`.

`render.scene` counts `water_end_calls` and `water_ocean_calls`, and each built grid as `scene_guest_lens_ocean_widened` or `scene_guest_lens_ocean_retail`.

Tests: `the_gates_end_with_no_work`, `a_drawn_definition_is_the_ocean`, `the_return_leaves_the_guests_registers`, `an_unported_ocean_step_is_named`, `an_unported_copy_is_named`, `an_unread_bsp_panics` (`water_end.rs`); `the_ocean_makes_no_shader_begin`, `the_ocean_draws_from_its_record`, `the_grid_is_built_before_the_gates`, `an_unmasked_grid_exception_is_unimplemented`, `the_shader_flags_pick_the_copy`, `the_copies_name_what_they_cannot_make`, `a_closed_gate_goes_to_the_stamps`, `the_stamps_skip_what_the_guest_skips`, `a_virtual_templates_group_reaches_the_same_implementation`, `unmade_steps_are_named` (`water_end/ocean/tests.rs`); `the_reflection_camera_mirrors_about_the_height`, `the_reflection_draws_the_sky_into_0x15_then_binds_the_view_again`, `the_reflection_names_what_it_cannot_make` (`water_end/ocean/tests/reflection.rs`); `the_first_call_opens_and_places_the_test`, `the_byte_read_two_frames_back_closes_the_gate`, `an_unbounded_or_empty_rectangle_needs_no_pass`, `no_entry_closes_the_gate` (`water_end/ocean/gate/tests.rs`); `a_block_is_resident_empty_or_cold`, `a_stamp_takes_the_cached_texture_or_names_the_request`, `an_uncached_image_is_its_texture` (`water_end/ocean/cache/tests.rs`); `register_finds_the_key_or_takes_a_free_entry`, `a_listed_test_sums_its_quads` (`native_flare_query.rs`); `the_ocean_extract_reads_rows_structure_and_flags`, `an_unread_definition_is_no_ocean` (`scene-extract/src/late.rs`); `the_ocean_record_belongs_to_its_view`, `the_ocean_record_changes_the_view_hash` (`frame-scene/src/late.rs`); `engine_textures_19_and_20_are_targets_0x15_and_0x16`, `the_grid_quads_keep_corners_and_diagonal`, `a_stream_stride_counts_attributes_and_skips`, `the_grid_gathers_the_declared_registers_per_vertex`, `an_unsupplied_stage_is_named`, `the_reflection_and_refraction_stages_keep_their_targets`, `a_material_without_layer_three_is_named` (`native-render/src/ocean_gpu.rs`); `the_ocean_shader_is_the_first_water_definitions` (`native-render/src/load.rs`); `guest_bytes_reads_a_tail_shorter_than_a_word`, `a_grid_key_follows_the_stream_and_the_quads` (`native_ocean_glue.rs`); the grid's unit tests (`engine/src/ocean_grid/tests.rs`) and `the_grid_matches_the_lifted_original` (`engine/tests/ocean_grid_differential.rs`: `0x0003ca90` lifted on the IR interpreter, every word and register equal).

## The graph's targets

Every render target and depth surface the device makes is the frame graph's, keyed by what it is to the game, not by the guest header or the frame's `TargetRef`. A `TargetRef::Aux(n)` is numbered by first use in its frame, so `Aux(0)` is the freeze frame's target in one frame and the post colour's in the next; a `TargetRole` is the same in every frame.

| Role | What | Made at |
|---|---|---|
| `Display(n)` | swap-chain buffer `n` (the back buffers, then the display surface) | `Direct3D_CreateDevice` |
| `DisplayDepth` | the device's depth buffer | `Direct3D_CreateDevice` |
| `Game(n)` | entry `n` of Halo's target table `0x004b4b58` (stride `0x98`): its first surface header `+0x18` (`0x0001dcf0`) | the first bind of the header |
| `GameLevel { target, level }` | surface `level` (1 to 3) of entry `target`, `+0x18 + 0x18 * level` (`0x0001dd20`: target `0xd`'s cube faces) | the first bind of the header |
| `Slot(global)` | the target whose header the game keeps at `global` (`native_targets::TARGET_SLOTS`, below) | the first bind of the header |

The game binds every target through its select `0x00014bc0(index)` (Code; the case bytes at `0x00014f30`, the jumps at `0x00014ed0`); the other `SetRenderTarget` callers are the XDK's own, on the device's buffers. By index:

| Index | Colour | Role |
|---|---|---|
| 0, 4 | `[0x0050935c]`, `[0x00509360]`: the device's back buffer and display surface | `Display(n)` |
| 1, 5 to 8, `0xa` to `0xc`, `0xf`, `0x12` to `0x16`, `0x19` to `0x1b` | the table entry's first surface | `Game(n)` |
| `0xd` | the table entry's surface of the call's second argument | `Game(0xd)`, `GameLevel` |
| 3 | `[0x00509370]`: a colour surface over the depth buffer's memory (`0x00014600`) | `Slot(0x00509370)` |
| `0x10`, `0x1d`; `0x11` | `[0x00509378]`, `[0x00509380]`: level 0 of the 64 x 64 textures `[0x00509374]`, `[0x0050937c]` (`0x00014850` through `0x000158f0` and `0x003fb7a0`, on the heap) | `Slot(..)` |
| 9 | `[0x00509384]`: the static surface `0x00484f50`, entry 9's memory as linear A8R8G8B8 | `Slot(0x00509384)` |
| `0x21` to `0x26` | `[0x00509390]` to `[0x005093a4]`, depth `[0x005093a8]`; no code names them as written | `Slot(..)` |
| 2, `0xe`, `0x17`, `0x18`, `0x1c`, `0x1e` to `0x20` | none | |

The depth is `[0x00509364]` (`DisplayDepth`) for indices 0, 1, 3, 5 to 8, `0xf` and `0x12`, entry 9's surface for 9 (whose data and size words index 9 copies into `0x00484f50` first), else none; AL 0 drops it. The viewport follows the bind: 128 square for context kind 2, the context's window (`view_setup::viewport`) for 0, 1, 2, `0x12` and the screen target, else the colour's whole level-0 surface (`GetDesc`, `0x003fd9c0`).

`render_bridge::target_bind` is this select in Rust (`bind`: `surfaces`, `native_targets::cut_select`, `viewport`, `Host::viewport`). An index the tables send nowhere is `index`; a viewport with no colour to size it by is `colour`. Tests: `each_index_binds_its_tables_surfaces`, `index_9_points_the_static_surface_at_target_9`, `the_viewport_is_the_window_for_the_view_and_the_surface_for_others`, `a_swizzled_surface_is_sized_by_its_format`.

```mermaid
flowchart LR
  G[guest bind, clear or copy, header] --> R[native_targets::role]
  R --> T[native_targets_glue::target_ref]
  T -->|TargetRef::Game role, guest size| F[FrameOp Bind, Clear, Copy]
  S[Stage::samples] --> W
  F --> W[frame_graph walk]
  W --> ST[frame_graph::targets::Targets, one surface per role]
```

- **The role table.** `render_bridge::native_targets::role(header, word)` names the table's surfaces (`TARGET_COUNT` entries, `TARGET_LEVELS` surfaces each) by address, and a header one of the select's globals holds (`word` reads it) as that global's `Slot`; `native_targets::surface(role)` is its inverse for a table entry.
- **The frame's ops.** `native_targets_glue::target_ref` names a header with a role as `TargetRef::Game(GameTarget { role, size, depth })`, its size the header's `X_D3DPixelContainer` (`header_size`); a header with no role is the scene's colour or depth. A role whose header has no size panics, `targets: <role>'s surface has no size`. The binds, clears and copies of `native_targets::Host` (the target select, the clears, `copy_view`) push frame ops on those refs.
- **The surfaces.** `frame_graph::targets::Targets` keeps one surface per role across frames at its guest size times the render scale (`game_size`), colour `Rgba8Unorm` or depth `Depth24PlusStencil8`, made again when the role's shape changes. A stage on a colour-only bind gets a depth of the colour's size: the scene's for the scene's colour, else one the graph keeps per size (`Targets::unbound_depth`).
- **Sampling.** A stage names the targets it samples in place (`Stage::samples`); the walk hands their surfaces to its record as `ExternalAttachments::sampled`, in that order. A stage that samples its own attachment panics, `frame graph: stage <name> samples <target>, its own attachment`. The ocean samples `Game(0x15)` and `Game(0x16)` (`native_ocean_glue::sampled_targets`), the layer-6 water the targets its passes' engine stages name (`native_water_glue::sampled_targets`).
- **Some channels.** A D3D clear of some colour channels (`0x10` red to `0x80` alpha, not all four) is a stage after the frame clear: one triangle over the bound colour whose blend writes the clear colour into those channels only (`native_render::masked_clear`). The ocean's alpha clear is one.

The copy is a texel copy (`copy_texture_to_texture`) of the smaller of the two sizes; the guest's `0x000352e0` is a filtered quad draw.

Tests: `a_game_target_is_named_by_its_table_entry_or_its_global` (`native_targets.rs`), `a_game_target_is_its_guest_size_at_the_scale` (`frame_graph/targets.rs`), `a_game_targets_header_is_its_role_at_its_size`, `a_game_target_with_no_size_is_named`, `a_clear_of_some_channels_is_the_masked_stage` (`native_targets_glue.rs`), `the_d3d_channel_bits_name_their_channels` (`masked_clear.rs`), `the_reflection_and_refraction_are_their_roles_frame_targets`, `a_target_not_made_is_the_target_cause` (`native_ocean_glue.rs`), `a_sampled_target_is_its_roles_frame_target` (`native_water_glue.rs`).

## The post and the screen effects

The colour pass `0x000214f0` and the screen effect `0x0002d140` are Rust bodies (`render_bridge::native_post`): no guest body runs, so none of their binds, clears or draws reach the device. Each drawn call is one native pass of `Stage::Post` (`Device::prepare_external_pass`), its slots named by role and resolved through `surface_for` and the role table:

| Pass | View (target 0's colour) | Roles |
|---|---|---|
| Glow | the view's target, bound by its index (`0x00014bc0`), read and written | `Game(7)`, `Game(8)` |
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
- The water end's ocean in a game (Delta Towers' bay is one): its pass on screen against the guest's, its copy on `Game(0x16)`, and which steps panic `water ocean: <cause>` (bit 19's reflection: the sky in `Game(0x15)` mirrored, and whether the native sky's replay draws with no depth bound).
- How many frames publish in a game: the `publish_*` counters over the census corpus.
- A published frame on screen: its pixels and brightness against the guest's present.
- Snapshot quiescence with the frame step's continuations.

## Related

- [retire-d3d.md](retire-d3d.md): the plan, the census and the other slices.
- [bink-hle.md](../audio/bink-hle.md): the movie wrapper and the Bink overrides.
- [native-renderer.md](native-renderer.md): the overlay stage.
- [render-loop.md](render-loop.md): the frame's call tree.
