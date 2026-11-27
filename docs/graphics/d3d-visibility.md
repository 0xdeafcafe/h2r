# D3D visibility tests and lens flares

Status: Built. Visibility tests (occlusion counts) and the lens flares that use them.
Code: `crates/platform/d3d/src/device/visibility.rs`, `crates/platform/gfx/src/occlusion.rs`. Tests: `crates/platform/d3d/tests/visibility_async.rs`, `crates/platform/d3d/tests/round7.rs`.
Switch: none. Part of [d3d.md](d3d.md).

## Visibility tests

```mermaid
sequenceDiagram
  participant G as guest (flare pass 0x20bb0)
  participant D as device
  participant B as backend
  G->>D: push CLEAR_REPORT_VALUE 1 and SET_ZPASS_PIXEL_COUNT_ENABLE 1 (inline, 0x20cec)
  G->>D: draw the occlusion quad through a callback
  D->>B: counted draws (gfx::Draw::count_samples)
  G->>D: EndVisibilityTest (0x20d0f)
  D->>B: report_pixel_count, ticket
  G->>D: GetVisibilityTestResult, two or three frames later
  D->>B: try_pixel_count
  D-->>G: count, or D3DERR_TESTINCOMPLETE
```

Halo's one user is the lens-flare/sun visibility pass (`0x20bb0`). Per query it pushes `CLEAR_REPORT_VALUE` 1 and `SET_ZPASS_PIXEL_COUNT_ENABLE` 1 inline (`0x20cec`), draws through a callback that returns the pixel count it expects, and calls `EndVisibilityTest` (`0x20d0f`). The index rotates through three sets (`frame % 3`; object `i` of set `s` is `s + 3i`), and a bit per slot (`0x5233F0`, set at `0x20d51`) records that a test was ended there.

`GetVisibilityTestResult` has two callers:

| Caller | Reads | On `D3DERR_TESTINCOMPLETE` (any non-zero `EAX`) |
|---|---|---|
| reader (`0x207b0`, call at `0x20840`) | set `(frame + 1) % 3`, the tests ended two frames before; divides the count by the expected one, clamps to `[0, 1]` (`0x20861`) and smooths the object's visibility byte towards it | skips the object, which keeps last frame's value (`0x20847`) |
| writer (`0x20bb0`, call at `0x20cbb`) | before reusing a slot that had a test (set `frame % 3`, ended three frames before), asks for its result and discards the count | skips the object for the frame: no new test, the old one stays (`0x20cc4`) |

Neither retries: both jump forward to the next object (`0x2096d`, `0x20d5e`), so an incomplete answer costs one frame of staleness for one flare, never a spin. A constant answer would be wrong somewhere, so the counts are real. The ignored `visibility_conventions_in_the_image` test checks these branches.

### The original

`EndVisibilityTest` stamps report slot `index & 0xFF` of page `index >> 8` (64 pages of 256 16-byte slots at device `+0x7D4`) with the GPU time and pushes `SET_ZPASS_PIXEL_COUNT_ENABLE` 0 and `GET_REPORT`. `GetVisibilityTestResult` returns `D3DERR_TESTINCOMPLETE` (`0x88760828`) while the slot's status word is `0xFFFFFFFF`, writing nothing, after a `D3D_SetFence(0)` when the GPU time (`+0x2C`) has not moved since the end (`0x3f8659..0x3f8660`). Otherwise it copies out the count and timestamp.

The pages are D3D-private: only `0x3f8570`, `0x3f8630` and the teardown (`0x3fc69d`) touch `+0x7D4`. The device keeps the results itself, by index, and allocates no guest pages.

### The device

- **Counting.** While `SET_ZPASS_PIXEL_COUNT_ENABLE` is on (`HwState::zpass_count_enable`), every draw is a counted draw (`gfx::Draw::count_samples`). `CLEAR_REPORT_VALUE` type 1 calls `Backend::clear_pixel_count`, and `EndVisibilityTest` drains, takes a ticket from `Backend::report_pixel_count` and turns counting off. Any other report type stops. A backend that cannot count stops at the end.
- An index never ended, or at or above `0x4000` (`VISIBILITY_INDICES`), is an invalid call. Ending an index again replaces its result. Teardown forgets every test. `Stats::visibility_tests` counts them.

| Mode | Set by | Answer |
|---|---|---|
| Polled (default) | none | `GetVisibilityTestResult` asks the backend without waiting (`Backend::try_pixel_count`). A count that is back is answered and kept. One that is not, for a test ended at most `VISIBILITY_PENDING_SWAPS` (2) `Swap`s before, answers `D3DERR_TESTINCOMPLETE` with the original's fence kick (`Stats::visibility_incomplete`). An older test waits for its count (`Backend::read_pixel_count`) |
| Waited | `Device::set_visibility_poll(false)` | every call waits for the count and `D3DERR_TESTINCOMPLETE` is never returned, so the answers are the same whatever the host's timing; that is what the hardware answers once caught up |

Polled, a test is never left incomplete past the point where the render thread's two-frame limit guarantees its count has been reached. In Halo's terms the reader (two frames) may see incomplete and keep last frame's value, and the writer (three frames) never does. Which calls answer incomplete depends on host timing, as the hardware's answers depend on the GPU's. Waiting for every count stalled the game thread behind everything the render thread had queued ([measurement](notes/d3d-caches.md)).

### Counts across the render thread and gfx

- **The render thread** (`ThreadedBackend`) lands counts on the guest's side (`Waits::landed_counts`): after each batch and each present it asks the inner backend for the reports in order, up to the first not back (`GfxBackend` maps a report the first time it is asked, and at each `Swap`). Its `try_pixel_count` answers from those; a miss (`Waits::pending_counts`) answers at once and hands the pending batch over with a kick, so the count lands at that batch's end. Its `read_pixel_count` answers a landed count at once and otherwise waits for everything queued before it.
- **Sealed game frames.** The scene sink retains each counter clear and the ticket for each report alongside the counted draws. At a present the frame owner matches those commands and tickets against the worker queue. For a matching frame the native pass reads the GPU counter at each report and the worker connects the original guest ticket to that GPU result. A read that needs the original draws before the present flushes them in queue order. At display scale the counter samples one host pixel per guest pixel. A private capture leaves the displayed frame's counter and reports untouched.
- **The count in gfx** (`gfx::occlusion`). wgpu occlusion queries are boolean on Metal, so they cannot give a count. Each counted draw is drawn a second time first, into an `R16Float` target with additive blending and a fragment shader that writes 1, against a scratch copy of the depth/stencil attachment with the draw's full depth/stencil state, writes included. Overlapping triangles of one draw are rejected in the count as in the real draw, and the real attachment is written once. A compute pass then adds up the texels inside the draw's scissor into a `u32` (`PixelCounter`). With multisampling it counts samples, as the NV2A does.
  - The depth texture needs `COPY_SRC` (`SurfaceStore` depth surfaces have it; `DrawError::CountDepthNotCopyable` otherwise). Replacing a registered shader drops the cached count pipelines along with the others.
  - W-buffered draws (all of Halo's 3D, `ZEnable(2)`) store w from the fragment stage, so a W-buffered program's count pass runs the program's own count entry (`shaders::psh::COUNT_DEPTH_ENTRY`, `ShaderProgram::count_entry`), which writes the same depth. Without it the count tests the rasterized z against stored w and every flare in front of geometry counts 0 (see [Lens flares](#lens-flares)).
  - Limit: the count pass does not run the draw's own fragment program, so alpha test, `discard` and the polygon stipple discard do not reject samples there. The flare's occlusion draws do not depend on them.

Tests:

- `tests/visibility_async.rs`: Halo's reader and writer against a synchronous backend, a caught-up, a lagging and a held render thread, the incomplete path through the ABI, and the synthetic ticket and display-scale cases. `cargo bench -p d3d --bench visibility_wait` prints the guest's time in the call with a lagging render thread, waiting and polling.
- `tests/round7.rs` (device and runner) and `gfx`'s `pixel_counter_*` tests (within-draw depth and stencil at 1x and 4x, a replaced shader, a depth that cannot be copied).
- The ignored `visibility_conventions_in_the_image` checks the call sites and conventions in the image.

## Lens flares

```mermaid
flowchart LR
  S["submit: 0x2dba0 registers the flare (0x51f40c)"] --> T["test pass 0x2dd30: occlusion quad, expected count = its area"]
  T --> C[counted draw]
  C --> R["reader 0x20770: count / area, clamped, x256 byte, smoothed"]
  R --> V[visibility byte]
  V --> D["draw pass 0x2df00: skips the reflection loop at zero"]
```

The visibility tests feed lens flares, and through them some scripted lights. All addresses are guest VAs, from a static trace.

- A `lens` attachment on an object (definition `+0x94`, classes `lens`, `MGS2`, `tdtl`) is not instantiated by the attachment constructor; the object render walk `0x41c80 -> 0x41c20 -> 0x41980` submits it every frame at its marker through `0x2dba0`. Lights (`ligh +0x94`, `0xc3340`), the sky's sun (`0x3eec0`) and effect parts (`0x179d23`) submit theirs the same way.
- `0x2dba0` registers the flare in the visibility array at `0x51f40c` (32-byte entries, `0x209b0`). The test pass `0x2dd30` (called from `0x2c2e4`) sets `ZEnable(2)`, colour writes off, `LESSEQUAL` without depth writes, and runs `0x20bb0` with the callback `0x2e540`: a quad of half-size `[2, 128]` pixels around the projected flare, pushed inline (a triangle fan with four position-register writes) through a game vertex program, its expected count the quad's area.
- The reader `0x20770` divides the landed count by that area, clamps it to `[0, 1]`, scales it to a byte (x256, at most 255) and smooths it into the entry's visibility byte. The draw pass `0x2df00` (from `0x13b7e`, callback `0x2e9e0`) reads the visibility byte and can skip the reflection loop when the computed visibility is zero. Its billboards and the optional filtered fans are in [Retail lens-flare draws](notes/2026-11-24-lens-flares.md).

A count of 0 is an invisible flare. Under the W-buffer the quad's rasterized z is about 1 (or 0.5 with depth clamp), above every stored w except the cleared far plane, so without the count entry only flares seen against the sky show.

Scripted lights depend on it: the Armory's look test creates `looker_light_top_red` / `looker_light_bottom_red`, model-less `mach` objects whose one attachment is a `lens`. With a count of 0 the light never lights. The script's look gates pass on `objects_can_see_object` or `player0_looking_up`/`_down` regardless. Test: `shaders_render.rs` `w_buffered_visibility_test_counts_against_the_stored_w`.
