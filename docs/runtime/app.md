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

`AppConfig` (all have defaults): `title`, `window_size` (1280x960 points),
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
default), `--transfers N`, `--aspect 4:3|16:9|source` (default `source` for play), `--scaling
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
`H2_DIFFICULTY` do the same, see [level-boot.md](level-boot.md)), `--set
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

`h2 play --frame-stats`, or `H2_FRAME_STATS=1` for `h2 play` and `h2 run`,
times every frame on the game thread and records the renderer's phases too
(`timeline::phase`, docs/perf/gameplay-budget.md):

| phase | what |
|---|---|
| `cpu` | game code, runtime accessors, kernel, scheduler, HLE calls |
| `dec` / `decode` | push-buffer decode and device state |
| `enc` / `encode` | wgpu on the CPU: draw recording, shader prepare, pass encode, pipeline compiles, texture uploads, the present copy |
| `sub` / `submit` | `queue.submit` |
| `gpu` / `gpu-wait` | blocking GPU round trips: readbacks, `D3DSWAP_FINISH` |
| `pace` | the tick's sleep to wall time; slack, not cost |
| `rt` / `render-wait` | game-thread wait for render-thread replies or the frame-in-flight limit, removed from `cpu`/decode |

Over the last 5 s it shows fps and the p50, p99 and max frame time, and
the mean of each phase: in the window's top-left corner (the banner, so
not with `--follow`, whose banner keeps the corner), and once a second on
stderr. At exit the run prints the whole run's percentiles and its ten
worst frames, each with its phases and what happened in it: draws,
submits, pipelines the encoding thread waited for (docs/graphics/graphics.md "Pipeline compiles"), nv2a shader compiles, texture uploads,
readbacks and bytes read from files. `H2_FRAME_STATS_CSV=<file>` writes
one row per frame; `H2_FRAME_STATS_FROM=N` starts the exit report at
frame N. F6 hides the line until the next refresh.

With the render thread, a queued marker forwards its counters and timings
into the same numbered frame. A row appears after both threads reach its
boundary; the game thread keeps running. Renderer phases overlap the
game thread, so their sum can exceed the frame's wall time. `render-wait_us`
is the game thread's own wait, including queue backlog, CPU encoding and
GPU work needed for a reply. The CSV's `read_bytes` is file I/O, not GPU
readback bytes. An incomplete render-thread tail at shutdown is omitted
and counted in the exit report. See [gameplay-budget.md](../perf/notes/gameplay-budget.md).

Cost: one clock read per phase switch, about six per draw. At 300 draws
that is under 0.1 ms of a 75 ms level frame. Add one short lock per frame. Off, each hook is a relaxed
load and a branch.

### Watermark

`h2 play` draws a development-build watermark in two small panels over
the game image, laid out the way pre-release builds of console shooters
often were (our own wording, not a copy of any real one): a build label
centred near the bottom, and the live run's numbers right-aligned in the
lower right corner. Each keeps to its third of the image width, so they
never overlap.

```
                                                            28.4 fps  ·  35.2 ms
                                                        cpu 24  ·  gfx 5  ·  wait 6
                                                                      draws 1104
                    h2recomp development build                  map 01a_tutorial
               a1987da master  ·  2026-11-15 14:02 UTC   metal, render thread on
```

Both panels sit on the same bottom margin; the live panel grows upwards.

- The label is baked at compile time by `crates/apps/app/build.rs`: the short
  commit, the branch (`detached` off a branch, `unknown` without git) and
  the time the build script last ran (it reruns when HEAD moves or
  `crates/apps/app/src` changes). `H2_BUILD_SHA`, `H2_BUILD_BRANCH` and
  `SOURCE_DATE_EPOCH` override them.
- The live panel comes from the frame stats' rolling window
  (`timeline::phase::window_summary`): fps, the mean frame time, then in
  ms per frame the game thread's `cpu`, `gfx` (decode + encode + submit +
  `gpu-wait`, on whichever thread does them; with the render thread on it
  overlaps `cpu`) and `wait` (`render-wait`: the game thread waiting for
  the render thread, 0 with it off), and draws per frame (the render
  thread's included, joined to their frame by `timeline::phase::WorkerFrame`). The map is the level map the game
  opened last (`kernel::level::last_map`: `maps\<name>.map`, the shared
  maps left out). Then the wgpu backend and whether the render thread is
  on. With `--frame-stats` the window is its 5 s one; otherwise `h2 play`
  starts frame stats itself with a quiet 2 s window
  (`timeline::phase::Config::live`: no log, no CSV, no exit report). An
  embedder without frame stats gets fps and frame time from the shell's
  present count.
- Drawing is host-side only, at present time, over the scaled game image
  (inside any letterbox or pillarbox), never in guest memory. The text
  changes at most four times a second; only then is it rasterised on the
  CPU (`font8x8`, the banner's public-domain letters, with a soft
  one-pixel shadow) into one texture a panel. Every present draws each as
  one alpha-blended quad. Glyphs are 8x8 font pixels, 2x from a target
  1600 pixels tall (Retina), and lines too long for their panel are cut
  with `..`.
- On by default. `--no-watermark` or `H2_WATERMARK=0` turns it off; F3
  toggles it while playing (F3 then never reaches the guest).
- The superbounce game type's HUD is not the window's: the game's own
  HUD draws it, through the HUD kit
  ([hud-kit.md](../game/hud-kit.md),
  [superbounce.md](../game/superbounce.md#the-superbounce-game-type)).
- In code: `app::Watermark` (shared on/off, note, map callback) on
  `AppConfig::watermark`; `app::watermark` has the formatting, layout and
  rasteriser, and `WatermarkRenderer` the pass. The banner and the
  watermark share `overlay::Sprite`.

## Following a branch: `h2 play --follow`

```sh
h2 play --follow /abs/path/to/disc/halo2.iso
```

Plays as `h2 play` does, and meanwhile follows `origin/master`: when a new
commit lands it builds it in the background, then a banner offers the
update. F5 restarts into it; F6 hides the banner. Closing the window ends
it. Everything after the follow flags is passed to `h2 play` as is.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--repo PATH` | the checkout containing the current directory | Repository to follow. |
| `--branch B`, `--remote R` | `master`, `origin` | Branch to watch. |
| `--no-fetch` | off | Watch the local `refs/heads/B` instead of fetching. |
| `--poll SECS` | 45 (minimum 5) | How often to fetch or look. |
| `--fresh-data` | off | Each launch gets an empty data dir under the cache (otherwise the user's `--data`). |
| `--cache DIR` | `~/Library/Caches/h2recomp/follow` | Where everything below lives. |

How it works (crates/apps/cli `follow.rs`):

- **Supervisor.** The parent process runs the game as a child (`h2 play`
  with the same arguments, plus `--out`, `--data` and the hidden
  `--follow-status FILE`). The first child is the `h2` you started, and the
  branch's head at startup is taken to be what it runs, so the first
  update is the next commit. Every process the supervisor starts (the game,
  build steps, git) runs in its own process group with SIGINT and SIGTERM
  unblocked (the supervisor blocks them for its signal thread, and a
  child would inherit that). A Ctrl-C in the terminal reaches only the
  supervisor, which asks the game to stop with one SIGTERM, cancels any
  build, and exits with the game's code. A second Ctrl-C sends the game's
  group a second SIGTERM (`h2 play` then exits at once) and SIGKILL after
  15 s. Nothing it started outlives it: the build's group gets SIGKILL 3 s
  after its SIGTERM, and both are reaped before the supervisor exits and
  drops its lock. Whenever a group's leader exits, normally or not, the
  rest of its group is killed before the leader is reaped. The watcher's
  poll sleep wakes on shutdown, and its thread (and the startup read's,
  after an interrupt) is joined, with any fetch stopped and reaped, before
  the supervisor exits.
- **Watching.** Every `--poll` seconds: `git fetch <remote> <branch>`, which
  only moves the remote-tracking ref, then `rev-parse`. With `--no-fetch` it
  reads the local branch. A failed fetch, or one with no answer in 2
  minutes (prompts are off: `GIT_TERMINAL_PROMPT=0`), is logged and the
  last fetched ref is used. The startup read runs on a thread too, so a
  Ctrl-C during a hanging first fetch stops it and exits at once.
- **Building.** One commit at a time. Heads that arrive mid-build queue, and
  the latest one wins. A commit that failed is not retried until the head
  moves again. If the branch rolls back (a force-push) to the running or the
  ready build, the queue is dropped, the build in flight is discarded when
  it finishes, and rolling back to the running build withdraws the offer.
  A build the branch merely moved forward from is still offered while the
  newer one builds. Each build runs in a detached worktree the supervisor
  owns (`<cache>/checkout`, added with `git worktree add --force --detach`,
  which only re-adds that path's own entry if its directory went missing;
  there is no global `worktree prune`). Before reusing it (`checkout
  --force`, then `clean`) it must be a real directory, not a symlink, at
  exactly `<cache>/checkout`, the top of its work tree, registered in the
  repository's `git worktree list` at that path as a linked worktree (not
  the main checkout), with the repository's git dir, and a `.git` file
  selecting that worktree's own private git dir (`<common>/worktrees/<name>`,
  whose `gitdir` names this `.git`), never the main git dir or another
  worktree's. Anything else there is refused and left alone. It runs `nice -n 15 cargo build
  --release -p cli` with `CARGO_TARGET_DIR=<cache>/target`, and
  `CARGO_BUILD_JOBS` set to half the cores unless you have set it.
  A rustc wrapper (`RUSTC_WRAPPER`, such as kache or sccache) and the rest of your environment pass through.
  The new `h2` decides whether the newest generated game is still its own:
  `h2 build <disc> --out <previous> --check` compares the dylib's `.stamp`
  (image hash, generator sources hash including the boot bridge, module
  ABI: `boot::build_stamp`) with its own. If not (or the `h2` is too old to
  know `--check`), the game is generated again with `nice h2 build <disc>
  --out <cache>/gen/<hash>`. The first previous game is the first child's
  `--out`. Objects go to `H2_OBJ_CACHE`, which defaults to
  `<cache>/objcache` and persists, so an emit only compiles C that changed.
  The finished `h2` is copied to `<cache>/builds/<hash>/`. Builds and gen
  dirs no longer in use are pruned before each build.
- **Banner.** The supervisor writes `<cache>/status` (one line,
  `info|ready|error<TAB>text`, replaced atomically). The child reads it
  twice a second into an `app::Banner`, which the shell draws in the
  top-left corner of the window over the game frame. It is the 8x8
  public-domain font at 2x on the debug menu's dark plate, with a strip in
  the notice's colour down its left edge, a second render pass with alpha
  blending (`app::overlay`). While the debug menu is open the notice is a
  line inside the menu instead ([debug-menu.md](debug-menu.md#layout)).
  The messages:
  "Building <hash>...";
  "Update ready: <subject, or N commits> - press F5 to restart" (with
  "(building <hash>...)" or "(<hash> failed to build)" when a newer commit
  is in progress or failed);
  "Update failed to build: <first error line>". After a failure the game
  keeps running the current build.
- **Restart.** With a banner configured, F5 and F6 never reach the game's
  pad. F5 works only while an update is ready, even with the banner hidden.
  It closes the window like Cmd+Q (the run ends as `HostStop` and prints
  its report), and `h2 play` exits with code 75 (`follow::RESTART_EXIT`).
  The supervisor then starts the new `h2` with the new `--out`, which boots
  fresh. Any other exit code ends the supervisor.
- **Safety.** Your checkout's working tree, index and branches are never
  touched. The only repository-side effects are the fetch's remote-tracking
  ref and the cache worktree's entry under `.git/worktrees`. Generated code,
  builds and logs stay in the cache: `logs/follow.log` holds the
  supervisor's events, and `logs/build-<hash>.log` holds each build's cargo
  and emit output. A lock file allows only one follower per cache. Pruning
  only touches `<cache>/builds` and `<cache>/gen` when they are plain
  directories, and unlinks (never follows) symlinks inside them. To
  remove it all: `git worktree remove --force <cache>/checkout`, then
  delete the cache dir.

Tests: `follow.rs` unit-tests the state machine (idle, building, ready,
failed, restart, queueing, rollbacks, close and interrupt). Against scratch
repositories and plain shell processes (no game, no cargo), it checks:

- a checkout symlinked to the user's checkout, the user's main checkout at
  the cache path, a foreign repository there, and a registered checkout
  whose `.git` file selects the main git dir or another worktree's are all
  refused, with the user's edits, staged changes, untracked files and
  branch intact;
- a leader that exits normally leaves nothing behind in its group;
- the first build leaves another worktree's missing entry alone, and
  re-adds its own;
- a build step started from a thread with the signals blocked still dies
  of SIGTERM;
- a cancel stops a step that ignores SIGTERM, and its background children,
  within the grace period, and a cancel before the spawn starts nothing;
- a second interrupt kills and reaps a game child that ignores SIGTERM
  before `drive` returns;
- a hanging fetch times out to the last fetched ref, and an interrupt
  during the startup fetch ends it at once; a transport helper that
  ignores SIGTERM is killed and reaped before the startup read returns,
  and before the watcher's thread ends on shutdown, mid-poll or mid-fetch. It also runs the event
loop with a fake builder and a fake child: build, F5, failure, fix, F5,
close. The status file format, the first-error pick and the argument split
are covered too. An ignored test (`cargo test -p cli --bin h2 -- --ignored
real_builder`) drives the real git, cargo, worktree and child code against
a scratch repository whose `cli` crate is a stand-in `h2`:

- the branch moves;
- the build generates the game because the previous one's stamp is not the
  new `h2`'s;
- a script child waits for "ready" and exits 75;
- the new build runs with the expected arguments;
- the user checkout stays clean;
- a broken commit reports `error[E0308]`;
- an h2-only commit reuses the generated game (`--check` passes);
- a bridge-only commit, whose `h2` sources are unchanged, generates it again;
- a clone follows its origin by fetching.

`app::overlay` tests dismissal, the restart offer and rasterising. A GPU test
draws the banner over a frame and reads back the blend.

## Watching the checkout: `h2 play --watch`

```sh
h2 play --watch /abs/path/to/disc/halo2.iso
```

Plays as `h2 play` does, and meanwhile watches your own checkout: its
working tree and its git state. When a burst of edits, or a `git pull`,
has settled, it does only what the changed files need: a rules script is
reloaded into the running game, the live web app is rebuilt, and a Rust or
generator change rebuilds `h2` in the checkout (and the game code when the
new `h2` says it must), then offers the same "Update ready" banner as
`--follow`. F5 restarts into it; F6 hides the banner. Everything after the
watch flags is passed to `h2 play` as is.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--repo PATH` | the checkout containing the current directory | Checkout to watch. |
| `--quiet MS` | 1500 (minimum 200) | How long the files must be quiet before a change set is taken. |
| `--fresh-data` | off | Each launch gets an empty data dir under the cache (otherwise the user's `--data`). |
| `--cache DIR` | `watch/<hash of the checkout's path>` in the cache directory | Where logs, builds and generated games live. |

`--watch` and `--follow` are separate supervisors; giving both is an
error.

What each changed path asks for (`watch::classify`, relative to the
checkout's top; the first row that matches wins):

| Paths | Action |
| --- | --- |
| any `target/`, `node_modules/`, `buck-out/`, `__pycache__/`, `.direnv/`, `.devenv*` component; `.claude/worktrees/`, `web/dist/`, `out/`, `extracted/`, `disc/`, `.tools/`; `.git/objects/`, `logs/`, `worktrees/`, `modules/`, `lfs/`; editor droppings (`*~`, `*.swp`, `.#*`, `4913`, `.DS_Store`) | ignored |
| the rest of `.git` (HEAD, refs, the index, operation markers) | holds the settle timer open; builds nothing |
| `content/core/rules/*.rules`, `content/modes/rules/**/*.rules` | hot: `rules.reload`; also rebuilds `h2`, which bakes it |
| any other `*.rules` | hot: `rules.reload` |
| `docs/`, `*.md` | nothing |
| `web/test/`, `web/bench/`, `web/mock/` | nothing |
| the rest of `web/` | hot: `bun install --frozen-lockfile`, `bun run build` (with `--live-local`) |
| a `tests`, `benches` or `examples` component; `tests.rs`, `*_tests.rs`, `*_test.rs` | nothing |
| `crates/formats/xbe/src/`, `crates/recompiler/`, `crates/runtime/runtime/include/`, `crates/runtime/recomp/c/`, `crates/runtime/recomp/build.rs`, `crates/runtime/recomp/src/render_entries.rs` | game image: rebuild `h2`, then the game code if its stamp moved |
| the rest of `crates/`, `assets/`, `.cargo/`; `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | host: rebuild `h2` |
| anything else (`scripts/`, `deploy/`, `editors/`, ...) | nothing |

The game-image rows are the directories `crates/runtime/recomp/build.rs`
hashes into the generator's stamp; the two lists are kept in step.

How it works (crates/apps/cli `watch.rs`):

- **Supervisor.** As `--follow`'s: the game runs as a child with
  `--follow-status`, in its own process group, and the same Ctrl-C, close
  and restart rules apply (`follow::ChildLauncher`, `follow::StatusFile`,
  `follow::watch_signals`). The first child is the `h2` you started.
- **Watching.** `notify` watches the checkout recursively: FSEvents on
  macOS, inotify on Linux. A linked worktree's own git dir, which lies
  outside it, is watched too (not recursively). Paths in the git dir count
  as `.git/...`.
- **Settling.** Every event that is not ignored, git's own included,
  restarts a quiet period of `--quiet` milliseconds (`watch::Debounce`).
  While the git dir holds `index.lock`, `HEAD.lock`, `MERGE_HEAD`,
  `rebase-merge`, `rebase-apply`, `CHERRY_PICK_HEAD` or `REVERT_HEAD`,
  nothing is taken, and the quiet period starts again once they are gone,
  so a pull or a rebase settles once, after its last write. A merge left
  with conflicts waits until it is committed or aborted. The changed paths
  are then one change set, classified into a plan (`watch::Plan`). When the
  watcher reports lost events (FSEvents' rescan flag), the plan rebuilds
  `h2` and checks the game code.
- **Hot, no restart.** One thread runs hot work in order, against the
  game's agent socket: `$H2_AGENT` when it names one, none when it is off,
  else the child's `agents/<pid>.sock`
  ([agent-api.md](agent-api.md#finding-a-game)). A rules script is
  compiled first (a compile error is the banner's), then, if the game's
  `rules.state` runs a script of the same name, sent with `rules.reload`
  (`{source, mode: auto}`): in place when its state keeps its shape, else
  the game restarts on the same map and variant
  ([rules-engine.md](../game/rules-engine.md#reloading-a-running-script)).
  `ctf.rules` makes both Capture the Flag and Assault
  (`rules::builtin::sources_of`). The banner says "Reloaded <name> (in
  place)" or "(restarted)", or "Reload failed: <error>". A web change runs
  bun in the checkout's `web/` when the game was started with
  `--live-local` and no `H2_LIVE_WEB`; the embedded server reads
  `web/dist` at each request, so the next page load has it ("Web app
  rebuilt"). That is so only while the running `h2` serves this checkout's
  `web/dist`: one built from this checkout does.
- **Building.** One build at a time. Changes that arrive during a build
  are merged into one queued plan, built when it finishes, and a failed
  build's plan is carried into the next one. A build runs `nice -n 15 cargo
  build --release -p cli` in the checkout, with its own target dir
  (`CARGO_TARGET_DIR`, else `target`), so it is incremental and uses the
  machine's rustc wrapper (kache); `CARGO_BUILD_JOBS` is half the cores
  unless you have set it. The new `h2 build <disc> --out <current game>
  --check` then says whether the current game code is still its own. If
  not, it runs `nice h2 build <disc> --out <cache>/gen/<n>`: a new dir,
  since the running game has the old dylib mapped, seeded with an APFS
  clone of the last game's `src/` so the emit rewrites only changed files,
  and with `H2_OBJ_CACHE` (default `obj` in the cache directory, shared
  with your other builds), so only changed shards compile
  ([BUILD-CACHE.md](../lifter/BUILD-CACHE.md)). `H2_OPT`, `H2_TIER` and the
  rest of your environment pass through. The new `h2` is copied to
  `<cache>/builds/<n>/` and offered: "Update ready: local changes (h2) -
  press F5 to restart", or "(h2 and the game code)". A build that left
  `h2` as it was and the game code current offers nothing. "Rebuilding
  h2..." or "Rebuilding h2 and the game code..." shows meanwhile.
- **Failures.** A failed build shows "Local build failed: <first error
  line>" and the game keeps running; `logs/build-<n>.log` has the whole
  output.
- **Safety.** It never writes to the checkout's sources, index or refs; the
  only writes there are cargo's target dir and, for the web app, `web/dist`
  and `web/node_modules`. A lock file in the cache allows one watcher per
  checkout. Its cache is not `--follow`'s, so the two can run side by side.
  `logs/watch.log` holds the supervisor's events.

Tests: `watch/tests.rs` covers the classifier table above, the plans, the
paths of a linked worktree's git dir, the debounce (a burst, a git
operation in progress, git or ignored events alone, a rescan), the
operation markers, and the supervisor over fakes: docs only (nothing), a
rules script (hot, no restart), a built-in script (hot and a build), a
host change (build, offer, F5), a failure (the game keeps running; the
plan is carried), changes merged into a queued build, an unchanged build,
close and interrupt, and `drive` end to end. The watcher, cargo, `h2
build` and the agent socket are not exercised by tests.

Hot reloading `h2` or the game code in place is not built. The game code
is a dylib whose functions the guest's threads are always inside, and
whose static data (dispatch tables, the guest's code addresses) the host
holds pointers into, so a swap needs every guest thread parked at a known
point and the host's tables rebuilt; the boundary where that holds is the
one machine snapshots use, between ticks with the guest parked
([snapshots.md](snapshots.md)). Restoring a snapshot taken by the old
module into a new one is only sound when the guest memory layout is the
same, which the stamp does not promise. Host Rust has no stable ABI, so
swapping it means a process restart or a narrow C-ABI plugin boundary.

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
- **Scaling** (`scale.rs`). Play uses square output pixels by default.
  Native follows the drawable up to the default 4K pixel budget.
  `GuestHost::native_size` publishes the physical drawable size for Native and
  `GuestHost::display_size` reports where a given raster is presented.
  `--aspect 4:3` and `16:9` fit those shapes inside the window. `Fit`
  adds bars as needed. In Native resolution with Source aspect, Fit uses
  the drawable's aspect for the game frame. `Integer` takes the largest
  whole multiple of the frame height that fits, and `Stretch` fills the
  drawable.
- **Resize** (`crates/apps/app/src/shell.rs`, `crates/apps/app/src/surface.rs`)
  reconfigures the surface and redraws the latest frame in the resize
  callback. On macOS, AppKit also requests redraws during live resize.
  Both paths read the current backing size from the view. The Metal
  presenter uses aspect-preserving contents gravity and presents the
  resized drawable in a Core Animation transaction
  (`crates/apps/app/src/metal_resize.rs`). The game reallocates its Native
  raster after the drawable size holds steady for 150 ms. Minimizing
  preserves the last nonzero size.
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
