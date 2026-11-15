# App: the host shell

`crates/app` is the platform layer the game runs inside on macOS: a window
with a Metal surface, presentation of the guest's frames, live keyboard,
mouse and controller input, the audio device, and the guest thread's
lifecycle. Everything the recompiled game touches (D3D, XAPI, DirectSound,
the kernel) stays in its own crate; the shell only hands those crates a
device, an input manager and a mixer, and takes frames back.

A tvOS or Xbox port replaces `src/shell.rs` and `src/audio_out.rs` (the
winit and cpal code). The handoff, pacing, scaling and input pump are
platform-free.

## Threads

```
 main thread (macOS UI rule)                 guest thread ("guest", 64 MiB stack)
 ---------------------------                 ------------------------------------
 winit event loop                            your closure: FnOnce(GuestHost)
 window + wgpu Metal surface                   Machine::run(chunk) loop
 cpal output stream (not Send)                 d3d GfxBackend on host.gfx_context()
 keys/mouse -> KeyboardMouse  ---(Arc)--->     InputManager (keyboard pad + gilrs)
                                               xapi::Xapi ... app::pump_input
 audio callback <- audio::Mixer <--(Arc)--     dsound::DirectSound on host.mixer()
 redraw: FrameSource::latest  <--3 slots--     Swap -> present hook -> FrameSink
         Compositor -> surface                 host.tick(clock) paces / pauses / stops
```

One wgpu device serves both threads. The shell creates it (with the window
surface as the compatibility hint) and the guest gets another handle to it
through `GuestHost::gfx_context()`. Every guest GPU object must be made on
that device, so build `d3d::GfxBackend` from it and from nothing else.

## API

```rust
pub fn app::run<F>(config: AppConfig, guest: F) -> anyhow::Result<ExitReport>
where F: FnOnce(GuestHost) -> anyhow::Result<()> + Send + 'static;
```

Call it on the main thread. It opens the window, creates the device, opens
the audio device, spawns the guest thread with a `GuestHost`, runs the event
loop, and returns once the window has closed and the guest has stopped (or
not stopped within `shutdown_timeout`). `ExitReport` holds the `GuestExit`
(`Returned`, `Failed(msg)`, `Panicked(msg)`, `StillRunning`, `NotStarted`),
frame counters and pacing counters.

`GuestHost` (moved into the guest thread; `Send`):

| method | what it is for |
|---|---|
| `GuestHost::headless(&AppConfig)` | Same interface with no window: own headless device, frames accepted and counted, a 48 kHz mixer consumed in real time by a `DiscardRenderer`. Use for `--headless` and tests so the guest-side code has one path. |
| `gfx_context() -> GfxContext` | The shared device. `d3d::GfxBackend::new(host.gfx_context())`. |
| `present_hook()` | `impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32) + Send + 'static`, exactly what `GfxBackend::on_present` takes. Copies the swapped surface into the handoff (resolving MSAA, forcing alpha opaque for X8/R5G6B5 formats like `gfx::surface::present_surface`) and returns at once. Errors are printed, never returned to the guest. |
| `frame_sink() -> FrameSink` | For frames that are not gfx surfaces: `submit_rgba(size, &pixels)`, `submit_view(&view, size, opaque)`, `submit_surface(&store, id)`. |
| `take_input() -> Option<InputManager>` | Once. Keyboard+mouse pad first (port 0), then gilrs pads. Build `xapi::Xapi` on it. |
| `keyboard() -> Option<&KeyboardMouse>` | The handle the window feeds; for scripted input. |
| `mixer() -> Arc<Mutex<audio::Mixer>>`, `sample_rate()` | The mixer the device plays, created at the device rate. Build `dsound::DirectSound` on it. |
| `tick(guest_ticks) -> Control` | Call between scheduler chunks with `kernel.clock().counter()`. Blocks while paused, sleeps while guest time is ahead of wall time, returns `Control::Stop` once a close was requested. |
| `lifecycle() -> Lifecycle`, `should_stop()` | Stop/pause flags for checks from inside overrides (`Send + Sync`, cloneable). |
| `frame_stats()`, `pace_stats()` | Counters. |

Also public: `app::pump_input` (below), `frame::{channel, FrameSink,
FrameSource, Compositor}`, `pacing::{Pacer, PacePolicy, VBlank}`,
`scale::{place, fit, DisplayAspect, Scaling}`, `keymap`, `lifecycle`.

`AppConfig` (all have defaults): `title`, `window_size` (1280x960 points),
`fullscreen`, `aspect` (`Standard` 4:3), `scaling` (`Fit`, `Integer`,
`Stretch`), `filter` (`Linear`, `Nearest`), `vsync` (true), `pause_on_focus_loss`
(false), `mouse_capture` (true), `gamepads` (true), `keyboard` (true),
`bindings` (`input::bindings::Bindings`, from the TOML in docs/input.md),
`audio` (true), `pacing` (`PacePolicy::REAL_TIME`), `keep_open_on_guest_exit`
(true), `shutdown_timeout` (3 s), `log_status` (false).

## Wiring it into `h2 run` (for the boot lane)

The boot crate keeps its headless path and gains a hosted one. Sketch,
against the boot worktree as of this writing:

1. **CLI.** `h2 run` without `--headless` calls, on the main thread,
   ```rust
   let report = app::run(app::AppConfig { title: "Halo 2".into(), ..Default::default() },
       move |host| boot::run_hosted(&xbe, &dylib, &opts, host).map(|_| ()))?;
   ```
   With `--headless`, either keep today's `boot::run`, or call
   `boot::run_hosted(..., app::GuestHost::headless(&cfg)?)` so both modes
   share code (pass `pacing: PacePolicy::Unpaced` for traces).
2. **D3D** (`d3d_glue::install`). Replace `RecordingBackend::new()` with
   ```rust
   d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook())
   ```
   and register `entries::<GfxBackend>()`. `Device<GfxBackend>` is `Send`, so
   the `SharedDevice` shape in abi.rs is unchanged. The hook runs inside
   `D3DDevice_Swap` with the guest-memory guard held; it only takes the
   handoff lock and records one GPU blit, so that is safe.
3. **DirectSound** (`dsound_glue::install`). Replace
   `audio::Mixer::new(48_000)` with `host.mixer()`.
4. **XAPI.** Build `xapi::Xapi::new(Config::for_gamepad(addr),
   host.take_input().unwrap(), storage)` and hold it in a `Mutex` shared by
   the XAPI overrides. Once per guest frame, outside any override:
   ```rust
   app::pump_input(&xapi, OwnedGuestMemory(rt.memory()), &mut KernelEvents(&k))?;
   ```
   `pump_input` takes the guest-memory adapter by value (it must own the
   guard, unlike today's borrowed `GuestAdapter<'a>`), runs `sync_input` and
   `take_signals` under memory then XAPI, drops both, delivers the event
   signals, then relocks XAPI to `acknowledge_signals` (docs/xapi.md step 5).
   `KernelEvents` implements `xapi::Events` over the kernel's event signal.
5. **The run loop.** Replace the single `m.run(opts.max_slices)` with
   chunks, so the host gets control between them:
   ```rust
   loop {
       let out = m.run(256)?;                       // a few ms of guest work
       if !matches!(out, kernel::RunOutcome::SliceLimit) { break out; }
       app::pump_input(&xapi, ...)?;               // or on Swap, see below
       if host.tick(k.clock().counter()) == app::Control::Stop { break out; }
   }
   ```
   Pick the chunk size so a chunk is well under a frame; `tick` cannot sleep
   in the middle of a chunk.

Where to pump input: once per guest frame is what docs/input.md assumes for
mouse sensitivity. Counting Swaps (the D3D call count or `frame_stats()`)
and pumping when it changes is the closest match; pumping every chunk also
works but makes mouse look depend on chunk rate.

## Presentation

`D3DDevice_Swap` -> `GfxBackend::present` -> present hook -> `FrameSink`:
one blit of the back buffer into the back slot of a triple buffer
(`mailbox.rs`), then publish. The window thread, woken through an
`EventLoopProxy` (once per batch of frames, not per frame), takes the newest
slot at each redraw and `Compositor` draws it into a viewport of the window
surface, clearing the rest to black.

- **Never blocks the guest.** If the guest presents faster than the display,
  older frames are replaced and counted as dropped. If it presents slower,
  the window keeps showing the last frame (also across resizes, pauses and
  after the guest exits).
- **Ordering** comes from the single shared queue: the copy is submitted
  before the slot is published, and the window's read is submitted before
  the slot can return to the guest. No fences.
- **Scaling** (`scale.rs`): the display aspect is a setting, not the
  back-buffer shape. A 640x480 frame is 4:3 (`Standard`); Halo 2's widescreen
  mode is anamorphic 640x480 shown at 16:9 (`Widescreen`); `Source` means
  square pixels. `Fit` letterboxes or pillarboxes, `Integer` uses the largest
  whole multiple of the frame height that fits, `Stretch` fills.
- **Resize** reconfigures the surface and redraws; zero sizes (minimised) are
  ignored.
- **Vsync**: `AutoVsync` (Fifo) by default, `AutoNoVsync` with
  `vsync: false`. This only affects the window; guest pacing is separate.
- Surface format: the first non-sRGB format (`Bgra8Unorm` on Metal). The Xbox
  pipeline is gamma-encoded throughout, so no conversion happens anywhere.

## Pacing

The kernel HLE's clock is logical (`kernel::Clock`): 100 ns ticks that only
move when every guest thread is waiting (the scheduler jumps to the next
timer deadline) or when the host calls `Kernel::advance_time`. Guest compute
costs no guest time. Halo 2 creates its device with `PresentationInterval =
0x80000000` (immediate), so Swap does not wait for vblank either; the game
paces itself on timers and the performance counter.

`Pacer` (`pacing.rs`) holds guest time to wall time from the outside:
between chunks, guest ahead of wall by more than `max_lead` (2 ms) sleeps the
difference; guest behind by more than `max_lag` (100 ms) rebases, so a slow
host plays in slow motion instead of fast-forwarding to catch up later. A
pause resets the origin. `PacePolicy::Unpaced` never sleeps (traces,
benchmarks). This needs no kernel change; if chunk granularity proves too
coarse, the clean fix is a kernel hook called before each idle
`advance_to(deadline)`, where the pacer would sleep until that deadline.

`VBlank` is the refresh arithmetic in guest ticks (`NTSC` 60000/1001 Hz,
`HZ60`, `PAL50`): `count_at(ticks)`, `time_of(n)`, `next_after(ticks)`. It is
for the D3D lane's `BlockUntilVerticalBlank`, `GetDisplayFieldStatus` and
vertical-blank callbacks, which should be kernel timers at
`next_after(now)` so the logical clock advances to them like any other wait.

## Input

The window feeds `input::backend::keyboard::KeyboardMouse` (`keymap.rs` maps
winit's physical `KeyCode` to the W3C names the bindings use, so WASD is
positional on any layout). gilrs reads pads on its own. Nothing reaches the
guest until the guest thread pumps: `InputManager::sync` runs inside
`Xapi::sync_input`, so the guest sees a consistent snapshot per frame.

- Default bindings are docs/input.md's: WASD move, mouse look, Space A,
  V or middle mouse B, E X, Q Y, F Black, R White, G LT, left mouse RT, Ctrl
  crouch, right mouse zoom, Escape Start, Tab Back, arrows d-pad.
- Mouse: click in the window to capture (cursor locked and hidden; that click
  is not forwarded). Holding Cmd or losing focus releases it. While Cmd is
  held, clicks neither capture nor reach the pad (`keymap::mouse_action`). Raw device
  motion drives look while captured.
- Host shortcuts, with Cmd held (never forwarded): Cmd+Q quit, Cmd+F or
  Cmd+Enter borderless fullscreen, Cmd+P pause.
- Focus loss releases every held key and button.

## Audio

`AudioOut` opens the default device with cpal, creates the `audio::Mixer` at
the device rate and renders it from the callback (`try_lock`, silence if the
guest holds the mixer at that instant, as in `audio::output`). It adds
pause/resume so a paused guest does not leave a looping buffer droning.

With no device (`audio: false`, no hardware, or a failed open) a
`DiscardRenderer` thread renders the 48 kHz mixer in real time and throws
the samples away. Without it, DirectSound buffers would never finish and
stream packets would never complete. It pauses with the guest without owing
time for the pause afterwards, and ends on a stop request or when dropped.
Headless hosts use the same renderer. The cpal stream is not `Send` on macOS and
stays on the main thread.

## Lifecycle

- **Close** (window close button, Cmd+Q): mouse released, audio paused,
  then `GuestWorker::stop` (`worker.rs`): `Lifecycle::request_stop`, then up
  to `shutdown_timeout` for the guest thread to return. `tick` and every pause or pacing wait wake at once on a
  stop request; the guest must also check between chunks. A guest that does
  not stop is reported as `StillRunning` and ends with the process.
- **Pause** (Cmd+P, or focus loss with `pause_on_focus_loss`): `tick` blocks,
  audio pauses, the window keeps redrawing the last frame. A pause that
  arrives during a pacing sleep ends the sleep, and `tick` waits instead of
  returning. The pacer resets on resume, so the paused time is not slept off.
- **Guest exits** (returns, errors or panics): the outcome goes in the title
  and the window stays up with the last frame (`keep_open_on_guest_exit`),
  or the window closes.

The title bar shows guest presents per second, frames shown per second, and
frames dropped in the last second (`log_status` also prints it).

## Tests

`cargo test -p app` (GPU tests skip themselves without an adapter):

- `scale.rs`: fit, pillarbox, letterbox, 16:9, integer scaling, stretch,
  degenerate sizes.
- `pacing.rs`: tick conversions, NTSC/60/50 Hz vblank arithmetic and round
  trips, sleep on lead, lead allowance, rebase on lag, reset, a simulated
  10 s 60 Hz loop staying within the allowance, the rate meter.
- `mailbox.rs`: handoff order, newest wins, drop counting, and a randomised
  interleaving checking the producer never writes the slot on screen.
- `lifecycle.rs`: stop interrupts sleeps and pauses; a pause ends a sleep;
  pause blocks until resumed.
- `audio_out.rs`: with no device, a non-looping buffer finishes and a stream
  packet completes; the discard renderer runs at real time with no burst,
  renders nothing while paused and owes nothing afterwards, follows the
  guest's pause and stop, and stops promptly on drop.
- `keymap.rs`: every mapped key is accepted by `input::bindings`, names match
  the W3C spelling, Cmd is not forwarded, default bindings through the map,
  mouse routing (a Cmd-click neither recaptures nor forwards).
- `tests/handoff.rs` (GPU): CPU frames arrive bit-exact, newest wins with drop
  counts, the frame on screen keeps its pixels across later publishes (CPU
  upload and GPU blit paths), bad sizes rejected, slots reallocate on size change, one wake per
  batch, a 200-frame producer thread against a consumer, the present hook on
  real gfx surfaces (A8R8G8B8 keeps alpha, X8R8G8B8 forced opaque), the
  compositor's pillarbox, orientation and clear, `GuestHost::headless`, and
  real-time `tick` sleeping to wall time.
- `tests/input_pump.rs`: window keys through `KeyboardMouse`, `InputManager`
  and `Xapi` into `XINPUT_STATE` only after a pump; rumble feedback signals
  delivered with neither the memory nor the XAPI lock held, then
  acknowledged so the buffer can be reused; `sync_input` errors returned.
- `tests/lifecycle.rs` (GPU, real guest threads): close during a pacing
  sleep, close during a pause, a pause arriving during a pacing sleep holds
  the guest and owes no sleep after resume, a guest that ignores stop times
  out as `StillRunning`, and normal return, error and panic outcomes.

Mutation checks: removing `request_stop` from `GuestWorker::stop` fails three
lifecycle tests; dropping the pause recheck in `tick` fails one; writing the
back frame into the front slot fails three handoff tests.

## Demo

```
cargo run -p app --release --bin h2-app-demo
cargo run -p app --release --bin h2-app-demo -- --aspect 16:9 --scaling integer --nearest
cargo run -p app --release --bin h2-app-demo -- --frames 300    # closes itself after 5 s
```

Flags: `--frames N`, `--fps N`, `--aspect 4:3|16:9|source`, `--scaling
fit|integer|stretch`, `--nearest`, `--no-vsync`, `--no-audio`,
`--no-gamepads`, `--pause-on-blur`, `--fullscreen`.

A synthetic guest drives the real shell exactly like the game will: 60 Hz of
logical time through `tick`, input through `InputManager`, frames through a
`FrameSink`, a tone through the shared mixer. Check:

- a 640x480 pattern at 4:3 with black bars whatever the window shape; the big
  circle stays round; corners red (top left), green (top right), blue (bottom
  left), yellow (bottom right); a 1 px white border on all four sides;
- a white bar sweeping across every two seconds, a binary frame counter at the
  top, and about 60 guest and 60 shown fps in the title;
- buttons light the boxes along the bottom (A B X Y Black White LT RT, then
  Up Down Left Right Start Back LS RS), sticks move the dots, and every
  change is printed;
- a quiet 220 Hz tone: left stick pans it, right stick Y bends it an octave
  either way, A toggles it, Cmd+P pauses it with the guest.

Checked on an M1 Max: window, Metal surface, 59-60 fps guest and display,
pattern geometry and audio device all as above. Physical controllers and
mouse capture were not exercised in this session.

## Known gaps

- Rumble: macOS gilrs has none (docs/input.md); `GCController` would need its
  own backend.
- HiDPI: the surface is in physical pixels and the frame is scaled into it;
  nothing is done about the backing scale beyond that.
- Pacing granularity is the chunk size (see Pacing).
- No on-screen overlay; status is in the title bar.
- Window position, size and fullscreen are not remembered between runs.
