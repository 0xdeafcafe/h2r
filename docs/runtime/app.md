# App: the host shell

Status: Built.
Code: `crates/apps/app/src` (`shell.rs`, `embedded.rs`, `lib.rs`), `crates/apps/cli/src/play_cmd.rs` (`h2 play`). Tests: `crates/apps/app/tests/`.
Switch: `H2_FPS` (30), the rest in [switches.md](switches.md).

`crates/apps/app` is the platform layer the game runs inside on the desktop: a window
with a wgpu surface, presentation of the guest's frames, live keyboard,
mouse and controller input, the audio device, and the guest thread's
lifecycle. Everything the recompiled game touches (D3D, XAPI, DirectSound,
the kernel) stays in its own crate; the shell only hands those crates a
device, an input manager and a mixer, and takes frames back.

A host that owns its own window replaces `src/shell.rs` and
`src/audio_out.rs` (the winit and cpal code) with [`app::embedded`](#embedded-hosts).
The handoff, pacing, scaling and input pump are platform-free.

## Pages

| Page | Holds |
|---|---|
| [App: frame stats and the watermark](app-overlays.md) | The `--frame-stats` overlay and the watermark. |
| [App: following a branch and watching the checkout](app-follow.md) | `h2 play --follow` and `h2 play --watch`. |

## Threads

```
 main thread (macOS UI rule)                 guest thread ("guest", 64 MiB stack)
 ---------------------------                 ------------------------------------
 winit event loop                            your closure: FnOnce(GuestHost)
 window + wgpu surface                         Machine::run(chunk) loop
 cpal output stream (not Send)                 d3d GfxBackend on host.gfx_context()
 keys/mouse -> KeyboardMouse  ---(Arc)--->     InputManager (pad + keyboard fallback)
                                               xapi::Xapi ... app::pump_input
 audio callback <- audio::Mixer <--(Arc)--     dsound::DirectSound on host.mixer()
 redraw: FrameSource::latest  <--3 slots--     Swap -> present hook -> FrameSink
         Compositor -> surface                 host.tick(clock) paces / pauses / stops
```

One wgpu device serves both threads. The shell creates it (with the window
surface as the compatibility hint) and the guest gets another handle to it
through `GuestHost::gfx_context()`. Every guest GPU object must be made on
that device, so build `d3d::GfxBackend` from it and from nothing else.

The shell can draw a host loading picture while the guest switches maps.
The view and its status have their own short lock, so the window can take
the picture even while the guest frame mailbox is publishing. The loading
picture is independent of guest frame counts and clears after a new guest
present. The desktop shell hides the performance watermark while this
picture is up. Embedded hosts draw it through the same frame source.

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

`h2 play` has no `.app` bundle, so on macOS the shell sets the Dock icon
itself once the window is open (`app_icon::set_dock_icon`, the plated Mac
icon from [art/app-icon](../../art/app-icon/README.md)).

`GuestHost` (moved into the guest thread; `Send`):

| method | what it is for |
|---|---|
| `GuestHost::headless(&AppConfig)` | Same interface with no window: own headless device, frames accepted and counted, a 48 kHz mixer consumed in real time by a `DiscardRenderer`. Use for headless runs and tests so the guest-side code has one path. |
| `gfx_context() -> GfxContext` | The shared device. `d3d::GfxBackend::new(host.gfx_context())`. |
| `present_hook()` | `impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static`, exactly what `GfxBackend::on_present` takes. The last argument borrows the current RGB gamma ramp for this present. The handoff applies it once when copying the swapped surface (resolving MSAA and forcing alpha opaque for X8/R5G6B5 formats). Errors are printed, never returned to the guest. Custom hooks must apply the ramp themselves; `gfx::surface::present_surface` is a raw helper and does not apply it. |
| `frame_sink() -> FrameSink` | For frames that are not gfx surfaces: `submit_rgba(size, &pixels)`, `submit_view(&view, size, opaque)`, `submit_surface(&store, id)`. |
| `loading_screen()` | Prepares the window's return-to-menu picture. Its callback wakes the shell immediately to show it and hands back to the guest on a subsequent present. |
| `take_input() -> Option<InputManager>` | Once. Player one combines keyboard/mouse with the first physical pad; scripts replace live sources. Build `xapi::Xapi` on it. |
| `keyboard() -> Option<&KeyboardMouse>` | The handle the window feeds; absent in timeline mode. |
| `mixer() -> Arc<Mutex<audio::Mixer>>`, `sample_rate()` | The mixer the device plays, created at the device rate. Build `dsound::DirectSound` on it. |
| `tick(guest_ticks) -> Control` | Call between scheduler chunks with `kernel.clock().counter()`. Blocks while paused, sleeps while guest time is ahead of wall time, returns `Control::Stop` once a close was requested. |
| `lifecycle() -> Lifecycle`, `should_stop()` | Stop/pause flags for checks from inside overrides (`Send + Sync`, cloneable). |
| `frame_stats()`, `pace_stats()` | Counters. |

Also public: `app::pump_input` (below), `frame::{channel, FrameSink,
FrameSource, Compositor}`, `pacing::{Pacer, PacePolicy, VBlank}`,
`scale::{place, fit, DisplayAspect, Scaling}`, `keymap`, `lifecycle`.

`AppConfig` (all have defaults): `title`, `window_size` (1280x960 points, fitted to the primary monitor aspect by `h2 play`),
`fullscreen`, `aspect` (`Standard` 4:3), `scaling` (`Fit`, `Integer`,
`Stretch`), `filter` (`Linear`, `Nearest`), `vsync` (true), `fps`
(`fps::FpsTarget::Thirty`; `H2_FPS`, below), `pause_on_focus_loss`
(false), `mouse_capture` (true), `gamepads` (true), `keyboard` (true),
`bindings` (`input::bindings::Bindings`, from the TOML in docs/input/input.md),
`audio` (true), `pacing` (`PacePolicy::REAL_TIME`), `keep_open_on_guest_exit`
(true), `shutdown_timeout` (3 s), `log_status` (false), `banner` (none),
`watermark` (none; `h2 play` sets one, see "Watermark" below).

## Playing: `h2 play`

`h2 play` (crates/apps/cli `play_cmd.rs`) runs the boot driver inside the shell:

```sh
CARGO_TARGET_DIR=$TMPDIR/app-play-target cargo build --release -p cli
$TMPDIR/app-play-target/release/h2 play /abs/path/to/disc/halo2.iso
```

The default `--tier auto` uses a matching clang module from `--out`
(default `$TMPDIR/h2boot`). When the module is missing or its build stamp
differs, `crates/runtime/recomp/src/tier.rs` builds it before the window
opens. `--tier bridge` (or `H2_TIER=bridge`) instead selects the Cranelift
image and starts the clang build in the background. The window banner
(`play_cmd::tier_banner`) reports when the full-speed build is ready
or has failed. Restart to use the finished clang build.

Flags: `--out DIR`, `--data DIR`, `--slices N` (unlimited by
default), `--transfers N`, `--aspect 4:3|16:9|source` (default play shape follows the drawable), `--scaling
fit|integer|stretch`, `--nearest`, `--fullscreen`, `--no-audio`,
`--no-gamepads`, `--no-vsync`, `--unpaced`, `--fps 30|60|120|uncapped`,
`--no-watermark`, `--no-mappack` (or `H2_MAPPACK=0`; a `mappack.iso` beside the
disc is otherwise installed into `--data` once, docs/game/map-pack.md), `--bindings FILE` (docs/input/input.md
TOML), `--icons auto|xbox|xbox360|playstation|keyboard|remote|steam` (button pictures in
prompts; `auto`, the default, follows the device last used, see
[button-icons.md](../input/button-icons.md)), `--input-timeline FILE` (an input timeline, docs/input/input.md, played on
guest time in place of live input), `--tier auto|fast|clang` ([BOOT.md](BOOT.md)), `--movies DIR` / `--no-movies`, `--no-halo1`, `--film FILE`, `--follow`, `--tile`, `--title` (see `play_cmd.rs` for the last three), `--status` (frame rates once a second,
the slice count every 1000 slices), `--frame-stats` (frame-time stats, below),
`--level MAP [--difficulty easy|normal|heroic|legendary]` (start that
campaign level, or a local Slayer game on that multiplayer map, in place of
the main menu, no title, profile or menus; a bad
name lists the disc's levels before anything is built; `H2_LEVEL` and
`H2_DIFFICULTY` do the same, see [level-boot.md](level-boot.md)), `--editor [--window]` (the game a tool holds, Studio's own game: it needs `--level`, creates its window hidden unless `--window` is given, records no film, never pauses on a focus change, and refuses the Live flags, `--film` and `--input-timeline`; `play_cmd/editor.rs`, [game-view.md](../editor/game-view.md)), `--set
KEY=VALUE` (any of our settings for this run, ahead of its variable and
`<data>/settings.toml`, and its row locked; `h2 settings list` shows them,
[settings-menu.md](../game/settings-menu.md) "Settings as a schema"), and Live: off
with no flag (nothing is contacted); `--live` signs in to
`https://live.forbes.red` (`$H2_LIVE_SERVER` names another, `off` keeps it
offline), `--live-server URL` to that service, and `--live-local` runs one
in the process (`--offline` or `--no-live` say "no Live" outright;
`--gamertag`, `--invite`, `--signin-terminal`, `--live-listen` and
`--live-tunnel` and its name and URL flags are in [live.md](../live/live.md)
"Playing on Live"). `--join URL` opens a [Live session link](../live/live-custom-games.md#join-in-halo-2) on the selected service. Signed in, the account's saves are merged into U:
before the game starts and pushed as it writes them
([live-cloud-saves.md](../live/live-cloud-saves.md)). The boot's environment variables still
apply; `H2_BOOT_INPUT` presses keys on the window's keyboard pad, so a
scripted run goes through the same `KeyboardMouse`, `InputManager` and XAPI
as a person's keys. Hold keys for one present at the menu's frame rate
(`START@f330+2,DOWN@f455+1,Enter@f475+1`): the menus autorepeat, and at one
or two presents a second a 4-present hold moves the cursor twice.
`H2_BOOT_GFX` is ignored: frames go to the window. As for `h2 run`,
consecutive draws to one target share a render pass under the loose
rule (a few edge pixels in a level can differ from one render pass per
draw; docs/graphics/graphics.md, "Merged passes").

How it is wired (all through `boot::run_hosted` and `boot::Host`, docs/runtime/BOOT.md
"Hosted runs"):

1. `main` calls `app::run` on the main thread; the closure runs on the guest
   thread and calls `boot::run_hosted`.
2. **D3D**: `d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook())`,
   installed with `d3d_glue::install_with` (same write barrier, clocked
   vblanks, diagnostics and stopgaps as the headless run).
3. **Audio**: boot's DirectSound output (audio-live) with **logical pacing
   into a real device sink**: `Host::audio_sink` opens
   `audio::output::DeviceSink::open_default()` on the output thread (a
   `NullSink` if there is no device or with `--no-audio`). Audio time is the
   kernel's logical time, advanced by boot after every slice exactly as in a
   headless run, so packet completions and events are ordered with the
   game's own calls and the run stays deterministic; the output thread only
   plays rendered PCM and never touches guest memory, so there is no pump
   competing for the guest-memory lock. The shell's pacer holds guest time
   to wall time, so the device gets audio at its own rate; a host slower
   than real time is heard as underruns (gaps), never as a different
   schedule. Device pacing was rejected: completions would follow host
   thread timing. The shell's own `AudioOut` is off (`AppConfig::audio =
   false`): boot's output owns the device. Consequence: Cmd+P pauses the
   guest, and the sound with it only because audio time stops; the device
   plays out what is queued (at most 250 ms).
4. **XAPI**: `host.take_input()` (player one is the keyboard and mouse
   combined with the first GameController pad) through
   `xapi_glue::register_with`; `xapi::service_input` runs after every slice,
   as in the headless run.
5. **Run loop**: after each slice or audio-only idle advance, `host.tick(kernel.clock().counter())`
   paces guest time to the wall clock and blocks while paused; on a close it
   returns `Stop`, the run ends as `HostStop`, prints its usual report, and
   the guest thread returns (`shutdown_timeout` is 10 s for the report).
   The kernel's own `Machine::pacer` stays off: it sleeps inside
   `Machine::run`, where a pause or close cannot interrupt it.
6. **Trace**: only the last 4096 kernel calls (and at most 4096 older open ones) stay in memory
   (`Host::trace_limit`), so a long session does not grow; no TSV is
   written.

Bindings are input's defaults plus Enter for A and Backspace for B, so the
menus work with arrows and Enter. Escape is START.

| Key | Pad |
| --- | --- |
| Enter or Space | A |
| Backspace or V | B |
| E / Q | X / Y |
| Escape / Tab | Start / Back |
| Arrows | D-pad |
| WASD, mouse (click to capture) | Left stick, look |

In a local Forge session, the play window draws the monitor HUD and dialogs
over the game frame using the UI kit and the fonts and art on the disc.
Opening a dialog releases mouse capture. A click selects the row under
the pointer in the fitted viewport. The monitor controls are listed in
[forge-design.md](../game/forge-design.md#the-ui).

Cmd+F toggles fullscreen and Cmd+P pauses.

Quitting: Cmd+Q or the close button stop the guest through the lifecycle
(the run ends as `HostStop` and prints the boot report), then the window
closes. Ctrl-C or SIGTERM in the terminal does the same from outside the
window through an `app::Closer`: it stops the guest (waking it from a
pause) and closes the window, also when the guest has already exited and
the window only shows its outcome. A second signal exits at once, and 20 s
after the first the process exits regardless. (A job started with a shell's `&` has SIGINT ignored; use SIGTERM.)

Where the input pump runs: after every slice, not once per guest frame. Mouse
motion is held until the game's `XInputGetState` has read it and is scaled by
the read interval, so mouse look does not depend on the slice rate or the
game's frame rate (docs/input/input.md, "Keyboard and mouse"). A manager
with no backend, no gate and no pad (a sim's playerless host) has nothing to
pull: its sync only counts in the host report (`InputManager::is_idle`), and
`xapi::service_input` locks XAPI a second time only when a signal is owed.

### Frame stats: `--frame-stats`

See [App: frame stats and the watermark](app-overlays.md).

### Watermark

See [App: frame stats and the watermark](app-overlays.md).

## Following a branch: `h2 play --follow`

See [App: following a branch and watching the checkout](app-follow.md).

## Watching the checkout: `h2 play --watch`

See [App: following a branch and watching the checkout](app-follow.md#watching-the-checkout-h2-play---watch).

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
- **Scaling** (`scale.rs`). Play renders for the drawable’s shape by default, including at fixed internal resolutions.
  Native uses physical viewport pixels. Fullscreen targets larger than 4K
  use a 3840 × 2160 pixel budget.
  `GuestHost::native_target` publishes the physical drawable, fitted viewport
  and fullscreen state together. `GuestHost::display_size` reports where a
  given raster is presented.
  With Fit scaling, `--aspect 4:3` and `16:9` fit those shapes inside
  the window. `--aspect source` fits a fixed raster at its own aspect and
  follows the drawable in Native resolution. The host loading picture
  keeps its source shape by default. `Integer` takes the largest
  whole multiple of the frame height that fits, and `Stretch` fills the
  drawable.
- **Resize** (`crates/apps/app/src/shell.rs`, `crates/apps/app/src/surface.rs`)
  reconfigures the surface and redraws the latest frame in the resize
  callback. On macOS, AppKit also requests redraws during live resize.
  Both paths read the current backing size from the view. The Metal
  presenter uses aspect-preserving contents gravity and presents the
  resized drawable in a Core Animation transaction
  (`crates/apps/app/src/metal_resize.rs`). The game applies the latest
  Native target at the next render frame boundary. DPI changes request a
  redraw and minimizing preserves the last nonzero size.
- **Vsync**: `AutoVsync` (Fifo) by default, `AutoNoVsync` with
  `vsync: false`. This only affects the window; guest pacing is separate.
- **Display rate** (`AppConfig::fps`, `H2_FPS=30|60|120|uncapped`, `h2 play
  --fps`, the Video screen's Frame Rate row, `graphics.fps` in
  `settings::Store`; `fps.rs`): at 30, the default, the window redraws when the guest
  presents. At 60 and 120 it also redraws at every publish, and its
  `DisplaySchedule` adds a redraw only when no frame was published for
  one interval (`DisplaySchedule::published` books the grid's next slot one
  interval after each publish). Uncapped redraws every event-loop turn
  with vsync off. A gap-filling redraw repeats the newest picture. The
  render thread's display frames between scenes are each drawn once, at
  their publish: a window grid of its own beside the render thread's
  would beat against it and drop one made frame and repeat another
  several times a second. [Display frames](../game/high-framerate.md).
- Surface format: the first non-sRGB format (`Bgra8Unorm` on Metal). The Xbox
  pipeline is gamma-encoded throughout, so no conversion happens anywhere.

## Pacing

The kernel counts guest work in 100 ns units and advances to logical
deadlines when threads are idle. Its periodic vblank source runs at 60 Hz.
A normal gameplay frame spans two vblanks, about 33.3 ms.

The shell's `Pacer` compares elapsed kernel time with elapsed wall time
after each scheduler slice and each audio-only idle advance. A lead over
2 ms makes the guest thread sleep for the difference. A lag over 100 ms
or a pause rebases the clocks. All four Frame Rate choices use the same
policy. The explicit benchmark option `--unpaced` selects
`PacePolicy::Unpaced` and skips these sleeps.

`VBlank` is the refresh arithmetic in guest ticks (`NTSC` 60000/1001 Hz,
`HZ60`, `PAL50`): `count_at(ticks)`, `time_of(n)`, `next_after(ticks)`. The
game's own vblanks do not use it: `crates/platform/d3d` delivers them as a kernel
periodic source on the logical clock (d3d.md, "Vertical blank").

## Input

The window feeds `input::backend::keyboard::KeyboardMouse` (`keymap.rs` maps
winit's physical `KeyCode` to the W3C names the bindings use, so WASD is
positional on any layout). GameController reads pads on macOS; gilrs handles
other desktop hosts. Nothing reaches the guest until the guest thread pumps: `InputManager::sync` runs inside
`Xapi::sync_input`, so the guest sees a consistent snapshot per frame.

- Default bindings are docs/input/input.md's: WASD move, mouse look, Space A,
  V or middle mouse B, E X, Q Y, F Black, R White, G LT, left mouse RT, Ctrl
  crouch, right mouse zoom, Escape Start, Tab Back, arrows d-pad.
- Mouse: click in the window to capture (cursor locked and hidden; that click
  is not forwarded). Holding Cmd or losing focus releases it. While Cmd is
  held, clicks neither capture nor reach the pad (`keymap::mouse_action`). Raw device
  motion drives look while captured.
- Host shortcuts, with Cmd held (never forwarded): Cmd+Q quit, Cmd+F or
  Cmd+Enter borderless fullscreen, Cmd+P pause.
- Focus loss releases every held key and button.

Configuration and deterministic input: [input-default.toml](../input/input-default.toml)
contains the full keyboard/mouse mapping and drift filter settings. Load it
into `AppConfig::bindings`, or use the demo's `--input-config PATH`.
`AppConfig::scripted_input = Some(timeline.playback())` replaces all live
sources, including in `GuestHost::headless`. Keep a playback clone and advance
it to relative guest ticks before each input pump. See [input.md](../input/input.md)
for the timeline format, clock contract, next-transition scheduling and the
boot runner integration. `--input-script PATH` wires that flow in the demo.
The app uses GameController on macOS, with physical haptics when available.
With keyboard enabled the first controller and keyboard share player one;
turn keyboard off to expose physical controller hotplug directly.

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
  or the window closes. `h2 play` closes it for a run with a planned end
  (`--slices`, `H2_BOOT_STOP_AT_PRESENT`) and for every lab job, so the
  process exits by itself (`close_on_guest_exit` in
  `crates/apps/cli/src/play_cmd.rs`).

### When the game stops

When the guest thread ends and the window stays up, the window draws a
red notice over the last frame (`overlay::stopped_notice`, set in
`shell.rs` on `GuestExited`): "The game crashed." with the first six
lines of the error, wrapped to 88 columns ("The game stopped." when the
guest returned), and "Press F5 to start the game again, or close the
window to quit." The full message stays in the terminal. F6 hides the
notice. F5 closes the window with `ExitReport::restart_requested` set. Under
`h2 play --follow` the process then exits with `RESTART_EXIT` for the
follower to start again; on its own, `h2 play` starts itself again in place
(`restart_in_place`: the same executable and arguments by `exec`, with the
pid-named agent socket freed and SIGINT/SIGTERM unblocked first).

The title bar shows guest presents per second, in-between frames the
render thread made per second (`made`, only while there are any:
publishes with present flags `frame::MADE_FRAME_FLAGS`, counted in
`FrameStats::made`), frames shown per second (one decimal), and frames
dropped, all over the last 5 seconds
(`pacing::WindowedRate`), refreshed once a second. Lab game windows keep their
job title throughout the run. In both modes `log_status` also prints
it. A one-second window read 0.0 for a game presenting 1.4 times a second.

## Tests

`cargo test -p app` (GPU tests skip themselves without an adapter):

- `scale.rs`: fit, pillarbox, letterbox, 16:9, integer scaling, stretch,
  degenerate sizes.
- `pacing.rs`: tick conversions, NTSC/60/50 Hz vblank arithmetic and round
  trips, sleep on lead, lead allowance, rebase on lag, reset, a simulated
  10 s 60 Hz loop staying within the allowance, the rate meter, and the
  windowed rate (a 1.4 fps producer reads 1.4; old bursts age out).
- `fps.rs`: `H2_FPS` parsing and default, 30 never self-scheduled, 60 and
  120 redraws a second on the fake display clock, late redraws skip slots,
  the phase-2 `alpha`. `tests/fps_lockstep.rs`: the toy guest's lockstep
  trace is identical at all four settings with a presenter on the fake
  clock (high-framerate.md, section 6).
- `mailbox.rs`: handoff order, newest wins, drop counting, and a randomised
  interleaving checking the producer never writes the slot on screen.
- `lifecycle.rs`: stop interrupts sleeps and pauses; a pause ends a sleep;
  pause blocks until resumed.
- `audio_out.rs`: with no device, a non-looping buffer finishes and a stream
  packet completes; the discard renderer runs at real time with no burst,
  renders nothing while paused and owes nothing afterwards, follows the
  guest's pause and stop, and stops promptly on drop.
- `overlay.rs`: F6 hides a notice until a different one arrives, only a
  ready notice offers F5 (also when hidden), glyphs on the plate, long text
  cut, and (GPU) the banner blended over a frame in the top-left only.
- `watermark.rs`: UTC dates, the build line, the stats line with and
  without frame stats, the note and map, F3's shared toggle, cutting to the
  image width, bottom-left placement and scale, light text over a soft
  shadow, the middle dot glyph, and (GPU) the watermark drawn twice over a
  gradient frame changes only the bottom rows, and stays inside a
  pillarboxed image.
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
`--no-gamepads`, `--pause-on-blur`, `--fullscreen`, `--input-config PATH`,
`--input-script PATH`.

A synthetic guest drives the real shell the way the game does: 60 Hz of
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

## Embedded hosts

`app::embedded` (`embedded.rs`, built on Apple and Windows targets only) is
the shell for a native host that owns the window and the run loop: the tvOS
and iOS apps ([tvos.md](tvos.md), [ios.md](ios.md); the host crate is
`crates/apps/h2host`), and later the UWP app for Xbox
([windows.md](windows.md)). The host passes its surface (`host_surface`: a
`CAMetalLayer` on Apple, a XAML `SwapChainPanel` on Windows) and calls
`Embedded::redraw` from its display link on the main thread. The guest
thread, frame handoff, compositor and pacing are the windowed shell's.
Input is GameController (pads, the Apple TV remote, iOS's on-screen controls)
behind player one's pad; no key or mouse events arrive, except text from the
system keyboard through `Embedded::keyboard`
([host-keyboard.md](../input/host-keyboard.md)). `device_config()` is the
`AppConfig` the apps start from: fullscreen, mouse capture off, pads on,
and `audio: false` (boot's output owns the device).

## Known gaps

- HiDPI: the surface is in physical pixels and the frame is scaled into it;
  nothing is done about the backing scale beyond that.
- Pacing granularity is the chunk size (see Pacing).
- On-screen stats are the watermark's line and `--frame-stats`, drawn in
  the banner's corner; rates are also in the title bar.
- The watermark's map is the last level map opened, which is what is
  loaded only as long as the game opens each level's map when it loads it.
- Window position, size and fullscreen are not remembered between runs.
