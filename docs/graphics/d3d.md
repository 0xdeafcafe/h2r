# D3D: the guest-facing Direct3D 8 replacement

Status: Built. Index and design summary for `crates/platform/d3d`; the detail is in the sibling pages below.
Code: `crates/platform/d3d/src/`. Tests: `crates/platform/d3d/tests/`.
Switch: `H2_RENDER_THREAD=0` and the diagnostics in [switches.md](../runtime/switches.md).

| Page | Covers |
|---|---|
| this page | summary, the inline sites game code uses instead of D3D, wiring into the runtime, rules and limits |
| [d3d-entries.md](d3d-entries.md) | every D3D function game code reaches: address, convention, status, notes |
| [d3d-device.md](d3d-device.md) | guest structures, device fields, state ordering, draws, clears, swap, resource destruction, gamma ramp |
| [d3d-render-targets.md](d3d-render-targets.md) | render-to-texture, depth levels, depth textures, overlapping levels |
| [d3d-frame-buffers.md](d3d-frame-buffers.md) | textures over the frame buffers and the depth buffer, depth aliases on the GPU |
| [d3d-caches.md](d3d-caches.md) | texture cache, vertex cache, the invalidation contract |
| [d3d-timing.md](d3d-timing.md) | vertical blank, fences and the busy check, push-buffer callbacks |
| [d3d-nv2a.md](d3d-nv2a.md) | fragments and NV2A registers, the register window, port-I/O guards, the boot path |
| [d3d-visibility.md](d3d-visibility.md) | visibility tests and lens flares |

`crates/platform/d3d` replaces the Direct3D 8 that Halo 2 links statically (XDK 5849, `D3D8LTCG`, the `D3D` section) at the API level, on top of `crates/platform/gfx`. It does not emulate the NV2A. The rule that holds throughout: game code does much of D3D's work inline, so the device reads guest state at use and drains a real push buffer, rather than intercepting every site.

The analysis is static, from `h2 analyze` and `h2 symbols --list --no-xcheck` over the retail XBE (identity in [engine-map.md](../engine/engine-map.md)). Matches from XbSymbolDatabase identify a function; Cxbx's LTCG patch list was used for cross-reference only. Conventions are read off call-site setup plus register use at entry. No executable bytes or disassembly listings are included.

## Summary

| Count | What |
|---|---|
| 237 | functions in the `D3D` section; signatures match 79 of them (84 matches; one address carries four shared `SetRenderState_*` signatures) |
| 91 | D3D functions reached from outside the section; 26 have no signature match and are named from evidence (which state slot they store, what they push, who calls them) |
| 83 / 1 / 7 | of those 91: implemented (adapter plus device method) / stops with `D3dError::Unimplemented` (it carries the entry name and its convention) / left to the original code (`d3d::abi::NATIVE`: four pure helpers, `D3D_DestroyResource`, which the runtime glue runs after the device's part, and two push-buffer writers) |
| 88 | overrides `d3d::register` installs in a `runtime::Dispatch`: 85 implemented and 3 stopping |
| 386 / 432 | `.text` accesses to the render-state and texture-state arrays |
| 199 / 53 | `MakeRequestedSpace` calls, from that many functions |
| 36 / 22 | inline `BEGIN_END` stores, in that many functions |

The 88 overrides go beyond the census by two groups:

- `SetRenderState_PSTextureModes` (`0x3f5cc0`, implemented). It is matched by signature and has an adapter, but no code outside the `D3D` section calls it (game code writes the `PSTEXTUREMODES` slot and device `+0x790` itself).
- The three D3D-internal functions that execute GPIO port I/O (`d3d::abi::PORT_IO`, see [Port I/O guards](#port-io-guards)): one implemented, two stopping. No game code calls them.

Game code also does a lot of D3D's work inline, which no override can intercept: it stores into the render-state and texture-state arrays, writes NV2A methods into the push buffer, and fills in device fields. The device model ([d3d-device.md](d3d-device.md)) is built around that.

## Inline sites (game code doing D3D's work)

LTCG inlined D3D's fast paths into the game, so a large share of D3D behaviour never passes through an entry point.

| Site kind | Count | The device's answer |
|---|---|---|
| stores into `D3D__RenderState` / `D3D__TextureState` | 386 / 432 accesses | the arrays are guest-owned and read at every draw ([state ordering](d3d-device.md#state-ordering)) |
| NV2A methods written into the push buffer | 199 `MakeRequestedSpace` calls in 53 functions | a real guest push buffer, drained in order ([d3d-device.md](d3d-device.md#push-buffer)) |
| device fields (put, threshold, embedded pixel shader) | about 50 functions | the device keeps the fields current ([d3d-device.md](d3d-device.md#guest-structures)) |

### State arrays

386 `.text` accesses to `D3D__RenderState` and 432 to `D3D__TextureState`.

- **Render state written directly** (writes / functions):

  | state | writes | functions |
  |---|---|---|
  | ALPHABLENDENABLE | 44 | 37 |
  | ALPHATESTENABLE | 39 | 36 |
  | COLORWRITEENABLE | 38 | 36 |
  | SRCBLEND | 27 | 24 |
  | DESTBLEND | 24 | 21 |
  | ZFUNC | 19 | 19 |
  | BLENDOP | 19 | 17 |
  | ZENABLE | 18, plus 18 reads | 18 |
  | ZWRITEENABLE | 17 | 16 |
  | DEPTHCLIPCONTROL | 9, plus 18 reads | 26 |
  | PSTEXTUREMODES | 8 | 4 |
  | BLENDCOLOR | 7 | 7 |
  | STIPPLEENABLE | 7 | 5 |
  | SOLIDOFFSETENABLE | 5 | 5 |

  Also written: stencil, alpha func/ref, polygon offset, fog, lighting,
  specular, point-sprite and point-scale states, and the final-combiner
  constants.
- **Indexed render-state writes** (`[reg*4 + base]`): 18 writes in 5
  functions. `0x00017b60` is a game-side render-state dispatcher.
- **Texture-stage state written directly** per stage, each in about 24–27
  functions: ADDRESSU/V/W, MAG/MIN/MIPFILTER, MIPMAPLODBIAS, MAXMIPLEVEL,
  MAXANISOTROPY, COLORSIGN, ALPHAKILL. Stage-indexed variants appear in 2–5
  functions.
- **`D3D__DirtyFlags`:** 167 `.text` references in 50 functions. The game
  sets the bits for the deferred commit.

### Push buffer

Each inline site follows the same pattern:

1. Compare put (`device+0`) with the threshold (`device+4`).
2. If put is past the threshold, call `MakeRequestedSpace`.
3. Store NV2A method headers and data words.
4. Store the advanced put.

`MakeRequestedSpace` is called 199 times from 53 functions. Methods stored
inline, with site/function counts:

| method | NV2A meaning | sites / fns | handled |
|---|---|---|---|
| `0x17FC` | `BEGIN_END` | 36 / 22 | yes |
| `0x1A00 + 16r` | `VERTEX_DATA4F` (register r) | 53 across `0x1A00`–`0x1AF0` | yes |
| `0x1880 + 8r` | `VERTEX_DATA2F` | 35 / 3–4 | yes |
| `0x1900` / `0x1940 + 4r` | `VERTEX_DATA2S` / `VERTEX_DATA4UB` | about 17 | yes |
| `0x1518` | `VERTEX4F` (position) | 8 / 3 | yes |
| `0x030C`, `0x1D78` | state mirrors (also stored in the arrays) | 18 / 18 each | payload applied |
| `0x0100`, `0x0110` | NO_OPERATION (with a software method, see below), WAIT_FOR_IDLE | 1 / 1 | yes |
| `0x1D8C` | `SET_ZSTENCIL_CLEAR_VALUE`/`SET_COLOR_CLEAR_VALUE`, two words: an inlined `InsertCallback` at `0x0002460e` (function `0x24970`, context), then NO_OPERATION 7 | 1 / 1 | yes (push callback) |
| `0x1480` | `SET_STIPPLE_PATTERN`, 32 words (`0x0001e4ae`, in `0x0001e370`; what `SetStipple` pushes) | 1 / 1 | yes (register state) |
| `0x1710` | `BREAK_VERTEX_BUFFER_CACHE`, value 0 (`0x00043a1a`) | 1 / 1 | yes (no-op: vertices are read at every draw) |
| `0x17C8` | `CLEAR_REPORT_VALUE`/`SET_ZPASS_PIXEL_COUNT_ENABLE` (an inlined `BeginVisibilityTest`, `0x00020cec`) | 1 / 1 | yes (restarts the backend's sample count; type 1 only; see [d3d-visibility.md](d3d-visibility.md)) |
| `0x181C` | `SET_EYE_VECTOR`, 3 floats (no literal push header in the XBE; no site known) | 0 / 0 | yes (register state: the eye of `DOT_RFLCT_SPEC_CONST`, `HwState::eye_vector`) |

`0x0A20` and a 1800-word `0x1A20`, both at `0x00052b89`, are not push-buffer headers and are not in the table: they are indexed stores of combiner input/output encodings into the game's PSDEF at `0x00484F68` (fields at `+0x00`, `+0x68`, `+0x88`, `+0xB4`) ahead of `SetPixelShader`, and never enter the push buffer. They are not a stop scenario. (The test `doc_inline_method_table_has_no_psdef_stores` reads this table and this paragraph.)

`D3DDevice_Begin` also calls the commit helper `0x00400fa0` before inline
immediate-mode draws. So do 18 other game functions.

### Device fields

- Put and threshold are accessed in about 50 functions.
- `+0x924..+0x92C` (the embedded pixel-shader object) is accessed in 23.
- The game builds a 60-dword PSDEF at `0x00484F68` and calls
  `SetPixelShader` with `EAX = device + 0x924`.

## Integration

`d3d::register(&mut Dispatch, SharedDevice<B>)` (in `runtime_glue`)
installs every override against the real `crates/runtime/runtime`:

```rust
let cfg = d3d::DeviceConfig {
    upload: d3d::UploadPolicy::Always, // omit for Tracked, once step 3 is wired
    ..d3d::DeviceConfig::halo2(push_buffer, scratch)
};
d3d::runtime_glue::map_device_memory(&mut guest, &cfg)?;
let device = d3d::runtime_glue::shared(cfg, d3d::GfxBackend::new(ctx));
let report = d3d::register(&mut dispatch, device.clone());
let runtime = runtime::Runtime::new(guest, dispatch);
```

1. **Memory.** `push_buffer` (more than 32 KiB; 256 KiB is comfortable) and
   `scratch` (256 bytes, for the back-buffer and depth headers) are
   host-chosen guest ranges nothing else uses; `map_device_memory` maps
   them read/write. Resource `Data` fields are read at `0x80000000 | pa`
   (`AddressMap::XboxContiguous`); use `AddressMap::Identity` or `Offset`
   if the runtime maps contiguous memory elsewhere.
   Set `nv2a_window: Some(base)` to give game code the NV2A register
   window at device `+0x934` (see [d3d-nv2a.md](d3d-nv2a.md), [NV2A register window](d3d-nv2a.md#nv2a-register-window));
   `map_device_memory` then also maps its two pages, `base + 0x3000` and
   `base + 0x400000`. Without it `+0x934` stays zero and the game's
   post-`KickOff` register reads fault.
2. **Overrides.** Each handler copies `CpuAbi` into `abi::Regs`, runs the
   adapter on the locked guest memory, writes `EAX`/`EDX` back if set and
   returns with `Guest::ret(cpu, cleanup)`. Other registers are left as
   they were. Lock order is guest memory, then device.
   - A guest memory fault inside D3D is returned as the runtime's own
     `GuestFault` (the runner records `Event::MemoryFault`).
   - Any other `D3dError` is recorded on the device (`Device::last_stop`,
     a `HleStop` with the entry and the message) and returns
     `TransferAbi::stop(id)`. The default hook emits the runtime's
     `Event::Unsupported { pc: entry, mnemonic: entry name, detail: the
     HleStop text, backtrace: ctx.backtrace(entry) }`, so
     `ExecCtx::report` shows it like any other stop. `register_with` takes
     a different hook.
   - Before the adapter runs, the handler drains the push buffer up to
     the next push-buffer callback and, if there is one, calls it and
     enters the entry again (see [d3d-timing.md](d3d-timing.md), [Push-buffer callbacks](d3d-timing.md#push-buffer-callbacks)).
   - After an adapter succeeds, the handler calls the original
     `D3D_DestroyResource` for each resource the entry destroyed
     (`runtime_glue::destroy_then_return`, see [d3d-device.md](d3d-device.md), [Resource destruction](d3d-device.md#resource-destruction)),
     then delivers pending vblanks, and only then returns to the caller.
   - `abi::NATIVE` addresses get no override.
   `tests/registration.rs` runs Halo's boot device sequence through
   `runtime::Runner` with these overrides.
3. **Write barrier.** `DeviceConfig::halo2` gives `Tracked`, which is only
   correct once the runtime keeps the invalidation contract ([d3d-caches.md](d3d-caches.md)). The boot
   (`boot::d3d_glue`) wires the barrier and runs `Tracked`;
   `H2_BOOT_UPLOAD_ALWAYS` (any value) switches it to `UploadPolicy::Always`, and
   `Verify` finds missed stores. Set `upload` as above while the barrier is not wired:
   - Forward every guest store D3D did not make to
     `Device::note_write(addr, len)` (exact) or
     `Device::note_written_pages(addr, len)` (whole pages). That covers stores from lifted code,
     the interpreter, kernel calls (`NtReadFile`, `memcpy`-like HLE
     helpers) and DMA-like copies.
   - Fold aliases to the `0x80000000 | pa` view first.
   - Report before the next D3D override runs.

   A cheap form is a check against `WriteTracker::watched_bitmap` in the
   store path, deferring the `note_write` calls to the next override
   entry. The device's own stores (render-to-texture write-back included)
   are already noted.
4. **Vertical blank.** See [d3d-timing.md](d3d-timing.md), [Vertical blank](d3d-timing.md#vertical-blank): on the logical clock,
   `kernel.add_periodic(..)` with `d3d::vblank::clocked_vblanks` and
   `d3d::register_frame_wait(.., FrameWait::HALO2_CLOCKED, ..)`; without a
   kernel, `Device::set_vblanks_per_swap(n)` and/or the inline frame wait.
5. **Present.** `GfxBackend::on_present` receives the surface on every
   `Swap`, after the pending pass has been submitted.
   With `d3d::ThreadedBackend` (on by default in `h2 run` and `h2 play`;
   `H2_RENDER_THREAD=0` turns it off) every backend call, the hook
   included, runs on a render thread; see [render-thread.md](render-thread.md). Blit it with
   `gfx::surface::present_surface`.
6. **Shaders.** `GfxBackend::new` installs `Nv2aShaders`; replace it with
   `GfxBackend::with_shaders`.

## Topics on sibling pages

These headings are cited from code comments; each topic lives on the page named.

### Vertical blank

Delivery of the 60 Hz callback on the kernel's logical clock, after `Swap`, or in the frame wait: [d3d-timing.md](d3d-timing.md).

### GPU model: fences and the busy check

The device runs the push buffer synchronously, so the GPU is always idle when guest code looks: [d3d-timing.md](d3d-timing.md).

### Push-buffer callbacks

`InsertCallback` words run guest code between the work queued before and after them: [d3d-timing.md](d3d-timing.md).

### Fragments and NV2A registers

The viewport and transform fragments the device reproduces, and the register methods the drain applies: [d3d-nv2a.md](d3d-nv2a.md).

### NV2A register window

Four read-only PFIFO and PGRAPH words that game wait loops read through device `+0x934`: [d3d-nv2a.md](d3d-nv2a.md).

### Port I/O guards

The three D3D-internal functions that execute GPIO port I/O never run: [d3d-nv2a.md](d3d-nv2a.md).

### Vertex cache

Guest vertex and index windows kept as snapshots under the write-tracker contract: [d3d-caches.md](d3d-caches.md).

### Resource destruction

The device's part of `D3D_DestroyResource` and the original's part the glue runs after it: [d3d-device.md](d3d-device.md).

### Visibility tests

Occlusion counts for the lens-flare pass: [d3d-visibility.md](d3d-visibility.md).

### Frame buffers as textures

Textures whose `Data` names a frame buffer or the depth buffer: [d3d-frame-buffers.md](d3d-frame-buffers.md).

### Overlapping levels on the GPU

The alias mirror for render-target levels that share one allocation: [d3d-render-targets.md](d3d-render-targets.md).

### Depth aliases on the GPU

Colour textures over the depth buffer made in a compute pass: [d3d-frame-buffers.md](d3d-frame-buffers.md).

## Rules and limits

What the device and `gfx` do not do:

| Gap | Detail |
|---|---|
| Counting through the draw's own fragment program | the count pass writes 1 for every sample that passes depth/stencil, so alpha test, `discard` and the polygon stipple discard are not applied to the count |
| Sampling more render targets directly | only whole single-level `A8R8G8B8`/`LIN_A8R8G8B8` textures read a level in place. Other formats (`X8R8G8B8` reads alpha as one; 16-bit formats quantise on the way back), mipmapped textures with a level drawn into, and `COLORSIGN` stages go through guest memory; each would need a view or a shader path that reads the surface as the upload would |
| No guest read trap | lazy write-back trusts the guest to wait on a level (`BlockOnResource`, the busy check, a fence, a callback) before its CPU reads the texels, as the Xbox GPU's own asynchronous writes require. A read that skips every wait sees the level as of its last write-back. A store that skips every wait is merged by page reports' byte differences only; exact store ranges from the runtime, or a trap on the first store to a watched page, would make it exact |
| Depth as a texture on the GPU | `D24S8` depth levels are converted on the GPU (`gfx::depth_bits`), and so are 32-bit colour textures over the depth buffer ([d3d-frame-buffers.md](d3d-frame-buffers.md)). `D16` levels, and aliases in other formats or over a `D16` depth buffer, go through a CPU round trip: a readback, then a decode and an upload. The shadow-map comparison itself is in `crates/formats/shaders` ([shaders.md](shaders.md), "Shadow maps") |

## Not verified

- The calling conventions in `crates/platform/d3d/src/abi.rs` are marked TODO-register. Each one needs a differential check, running the original under the interpreter against the adapter, at its recorded call sites before it is trusted.
- Vertical anchoring of the polygon stipple at heights not divisible by 32 needs a hardware capture.
- `Clear` ignoring the scissors is not checked against hardware.

## Related

- [retire-d3d.md](retire-d3d.md): the plan that removes this crate once the native renderer owns the frame.
- [graphics.md](graphics.md), [framescene.md](framescene.md), [render-thread.md](render-thread.md), [shaders.md](shaders.md).
- [notes/d3d-caches.md](notes/d3d-caches.md): measurements for the caches and visibility polling.
