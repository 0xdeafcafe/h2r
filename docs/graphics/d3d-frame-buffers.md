# D3D frame buffers as textures

Status: Built. How a texture over a frame buffer or the depth buffer is sampled, since both exist only on the host.
Code: `crates/platform/d3d/src/device/raster.rs`, `crates/platform/d3d/src/device/frame_snaps.rs`. Tests: `crates/platform/d3d/tests/passes.rs`, `crates/platform/d3d/tests/depth_textures.rs`, `crates/platform/d3d/tests/gpu_depth_alias.rs`.
Switch: none. Part of [d3d.md](d3d.md).

## Frame buffers as textures

| Texture's `Data` is in | Sampled from | Counter |
|---|---|---|
| the back buffer (`BACK_BUFFER_DATA`) | a copy of the image being drawn (same-frame feedback: the draw reads what was there before it) | `Stats::frame_snapshots` |
| the display surface (`DISPLAY_BUFFER_DATA`) | a copy of the image last presented (back0 on a copy-swap device whose shown image still waits there) | `Stats::previous_frame_snapshots` |
| the depth buffer (`DEPTH_BUFFER_DATA`) | the depth/stencil bits decoded as the colour format the header names, or the depth-alias colour surface | `Stats::depth_reads`, `Stats::depth_alias_uploads` |
| a frame buffer, in another format, size or offset | the draw is skipped with `Event::DrawSkipped` when it reads that stage | `Event::DrawSkipped` |

The frame buffers and the depth buffer exist only on the host. Their headers carry `Data` values above any Xbox RAM that name them. Each frame buffer keeps one `Data` for its life, whichever header names it:

| Constant | Value | Names |
|---|---|---|
| `d3d::device::BACK_BUFFER_DATA` | `0x0C000000` | the back buffer at creation |
| `DISPLAY_BUFFER_DATA` | `0x0E000000` | the display surface at creation |
| `THIRD_BUFFER_DATA` | `0x0F000000` | the third buffer (two back buffers only) |
| `DEPTH_BUFFER_DATA` | `0x0D000000` | the depth buffer |

The back and display headers are distinct (the display header and a third live at `device::FRAME_HEADERS_OFFSET` in the scratch range). Guest memory at those addresses is never read or written by the device: a CPU read of a frame buffer through `Data` faults instead of returning stale bytes. A new device (CreateDevice after teardown) starts with cleared frame buffers and the headers back at their first `Data`, so nothing the old device presented reads as the previous frame. Halo rebuilds its textures from the new headers (`0x12b9e`).

### How Halo uses them

Halo builds its frame-buffer textures at `0x14600`, called at boot (`0x127c1`, just before `0x14850`) and again from `0x12b9e` and `0x137f2e`.

| Object | Global | Built as |
|---|---|---|
| current / previous frame textures | `0x509354` / `0x509358` | `LIN_A8R8G8B8` 640x480 headers (`Common` `0x40001`, format `0x11229`, size `0x271DF27F`) over the back-buffer (`+0x1A14`) and display-surface (`+0x1A18`) `Data` as they are then |
| depth as colour | `0x509368` | `LIN_A8R8G8B8` texture over the depth surface's `Data` |
| depth as swapped colour | `0x50936C` | format `0x3F` (`LIN_A8B8G8R8`) over the same `Data` |
| depth-aliased colour surface | `0x509370` | a copy of the back-buffer header with the depth `Data`, selected as a target by `0x14bc0` |

The global texture switch (`0x14f80`, table `0x15114`, results in `0x51F3D8`) binds them: `0x509354[frame & 1]` (current) at `0x14fe9`, `0x509354[(frame - 1) & 1]` (previous) at `0x14fc1` and `0x150a7` (frame counter `0x485AA0`), `0x509368` at `0x15020` and `0x50936C` at `0x15032`. Because every swap flips, the two textures name the buffer being drawn and the buffer last presented in turn, and the frame parity picks between them. The intro reaches the crossfade evaluator `0x002ab270`, which reads the previous parity.

### Copies of a frame buffer

A texture whose `Data` is a frame buffer's, `LIN_A8R8G8B8` or `LIN_X8R8G8B8`, 2D, the buffer's size, is sampled from a copy of that buffer. `Backend::snapshot_surface` makes one and `Backend::snapshot_surface_into` refills one (`GfxBackend` uses `gfx::SurfaceStore::snapshot` and `snapshot_into`). If the copy to the front was lost (`copy_surface` unsupported), the draw is skipped. A backend without `snapshot_surface` skips too.

`Stats::frame_copy_textures` counts copies that made a texture. An `X8` texture reads alpha as one: the copy's alpha is cleared to one with an alpha-only masked clear, as uploads of `X8` texels force it to 255.

**The pool** (`src/device/frame_snaps.rs`, `Device::frame_copy` in `src/device/raster.rs`). The copies live in a fixed pool of textures: at most three per surface size and format (`FRAME_COPY_SLOTS`) and eight in all (`FRAME_COPY_TEXTURES`). A slot's texture is made by its first copy and refilled by a GPU `copy_texture_to_texture` after that, so the number of draws that sample the frame never raises the memory held. This matters because particles blended into the frame they sample write it with every draw, and every draw then needs a fresh copy.

- Each copy and the draw that reads it are recorded in order, the copy after every draw recorded before it (the pending pass is encoded first). A draw before a refill still reads the old contents and a draw after it the new; a draw that reads the frame it writes refills the slot the draw before it read.
- The slots one draw binds are pinned until the next draw, so its other stages never refill them. A draw binding more distinct copies of one size than the pool adds a slot, up to its four stages.
- A slot that holds nothing any more is refilled first, then the least recently used of its size. A new size past eight replaces the least recently used slot of another.
- Draws bind the slot's one texture and view, so bind groups are cached once per slot. When the backend cannot refill a slot (a resolution change gave the surface another size), a new copy replaces the slot's texture.

**Sharing.** A copy is shared between draws while its surface is unwritten. Each colour surface has a colour-write generation, moved on by every drawn draw or pass that writes its colour, every colour clear, the owed copy into the front of a copy-swap device, a render-to-texture fill or mirror load, and a display-only scene stage over it. A write also ends what the surface's slots hold.

- A draw that samples the surface at the generation a slot was filled at binds that slot with no copy (`Stats::frame_snapshot_reuses`), so a run of distortion draws into their own target that all read the frame costs one copy. A draw that reads the frame after another wrote it gets a fresh one.
- Slots are matched by surface and alpha rule. A destroyed surface forgets its generation. A present ends what every slot holds and keeps the textures (a frame owner replays each frame's samples with that frame), so a long session holds a fixed number (`Device::frame_snapshots_held`).
- While post-process draws are held (swallowing), their writes are not issued yet, so each held draw gets a copy of its own, kept with its packet for the post call.

**Every backend refills in place.**

| Backend | How it refills |
|---|---|
| `ThreadedBackend` (render thread) | queues the refill as a sample of the same proxy and refills the texture it holds for it |
| scaled backend | refills both copies, the reference's and the view's (a view copy made while the view slept, a share of the reference's, is replaced by a scaled one when the view wakes) |
| scene sink | records a refill as a new sample that names the earlier sample of the frame whose texture it refills (`ReplayOp::Sample`'s `into`); a frame owner's replay (`gfx_backend/guest_draw_pass/commands.rs`) refills that sample's texture, so a replayed frame makes no more copies than the device's pool |

Tests: `five_hundred_frame_writing_samples_hold_at_most_the_pool` and the frame-sampling tests near it in `tests/passes.rs`; `a_frame_copy_refill_reaches_the_texture_the_proxy_made` in `tests/threaded.rs`; `a_refilled_copy_is_a_new_sample_that_names_the_one_it_refills` in `tests/scene_sink.rs`.

### Textures over the depth buffer

A texture inside the depth buffer's `FRAME_DATA_SPAN` reads its depth/stencil bits as the colour format it names (`Device::depth_texture` in `src/device/raster.rs`). The depth buffer is read back as the guest words it would hold (`Backend::read_depth`: linear, pitch `width * bpp`, `D24S8` as `z << 8 | stencil`), and the bytes from the texture's offset on are decoded as the texture, as if guest memory held them. So `0x509368` (`LIN_A8R8G8B8`) samples the depth's top byte as alpha, its middle and low bytes as red and green and the stencil as blue, and `0x50936C` (`LIN_A8B8G8R8`) the same with red and blue swapped. The main menu's depth-based fog/distortion pass (about 400 draws a frame, `HILO` dot mappings) reads it this way; the `HILO` modes themselves are the shader translator's.

- The readback is cached by a depth generation, bumped by every drawn draw that can write depth or stencil into the device's depth buffer (the same test as for depth levels: read-only stencil tests and skipped draws do not count) and by every depth or stencil clear of it. Each header and `COLORSIGN` mask is decoded once per readback, so a run of draws that test depth without writing it costs one readback.
- Colour aliases use the same biased signed-channel storage as ordinary textures, so signed and unsigned bindings of one header can coexist. All of a header's host textures are freed when it is destroyed.
- Like the back-buffer copy, a draw sees the buffer as it was before the draw.
- A texture that runs past the end of the buffer, or whose header does not decode, skips with a reason. So does any texture over the depth buffer when the backend has no `read_depth` (`GfxBackend` needs `DEPTH32FLOAT_STENCIL8`; Metal has it), and a multisampled depth buffer.

**The colour surface over the same memory** (`0x509370`, rasterizer target 3: `0x14bc0` -> `0x14c21`; as a texture, the texture switch -> `0x15020`, it is `0x509368`) gets a host colour surface of its own (`raster::DepthColor`, noted in `surface_for`). The memory holds whichever was drawn into last: a drawn draw or clear into that surface makes it newer (and bumps the depth generation), and one that writes the depth buffer's depth or stencil makes the depth newer again.

- While the colour surface is newer, a texture over the depth buffer reads its pixels instead of the depth bits (`Device::depth_color_texture`). The whole surface as a linear 32-bit texture of its size with no `COLORSIGN` (`0x509368`) is a copy (`snapshot_surface`, counted in `Stats::frame_snapshots`). Any other view (`0x50936C`, an offset, a sign) decodes the surface read back as guest bytes (`read_surface`), once per generation.
- The level loading screen needs it: every frame `0x296c0` copies target 1 into target 3 and `0x29370` box-filters `0x509368` back into target 1 with `DESTBLEND = CONSTANTCOLOR`, a trail.
- Not modelled: target 3 does not start from the depth bits when it is bound after depth was drawn (a draw that blends with or only partly covers it sees its own older pixels), and depth tests after target 3 was drawn into use the depth as it was.
- Tests: `textures_over_the_depth_buffer_read_the_colour_surface_drawn_into_last` and the GPU and render-thread ones after it in `tests/depth_textures.rs`.

### Other textures inside a frame buffer

Any other texture inside a frame buffer's `FRAME_DATA_SPAN` (another format or size, or an offset into it) skips the draw with `Event::DrawSkipped` saying why, when the draw reads that stage's image. A draw reads a stage when:

- a pixel shader's `PSTEXTUREMODES` gives the stage a mode that fetches a texel (`TexMode::texture_dim`; not `CLIPPLANE`, `PASSTHRU`, `BRDF`, `DOT_ZW` or `DOTPRODUCT`); or
- fixed function, the stage comes before the first `COLOROP = DISABLE`.

On a stage the draw does not read, a texture over any buffer is left unbound (nothing is copied or read back) and the draw goes ahead.

## Depth aliases on the GPU

| Path | Used when | Cost |
|---|---|---|
| GPU compute pass (`gfx::guest_words::GuestWords::depth_alias`) | `Backend::defers_depth_aliases` (`GfxBackend`); `A8R8G8B8`, `X8R8G8B8`, `A8B8G8R8`, `B8G8R8A8` or `R8G8B8A8`, linear (pitch a multiple of 4) or swizzled, at a 4-byte-aligned offset, over a single-sample `Depth32FloatStencil8` buffer | none on the guest side |
| read-back and CPU decode inside the backend | anything else, or `GfxBackend::set_gpu_depth_aliases(false)` (tests) | one `read_depth` of the whole buffer per depth generation, a decode and an upload; the guest (with the render thread, the render thread) waits for the whole queue |

A texture over the depth buffer's memory (`0x509368`, `0x50936C`: the menu's fog pass, one a frame) would otherwise pay the second row every frame. The compute pass picks, for each texel, the word of the `LIN_D24S8` buffer at its byte address (offset, then the texture's pitch or swizzle). It computes the word from the depth and stencil aspects with `encode_depth`'s rounding, and its bytes become the texel's channels as `decode_texture` orders them for the format, with `COLORSIGN` channels biased.

Tests: `crates/platform/gfx/tests/guest_words.rs` checks the pass against `read_depth_as` plus `decode_texture` byte for byte. `crates/platform/d3d/tests/gpu_depth_alias.rs` draws seven alias layouts with three sign masks over three depth contents with the pass on and off, on the device's thread and through the render thread, and compares the back buffers byte for byte.
