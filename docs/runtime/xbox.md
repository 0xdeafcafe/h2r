# Xbox (Dev Mode, UWP)

The game runs on an Xbox Series X|S in Developer Mode as a UWP app,
built entirely on a Mac. It is the Windows port
([windows.md](windows.md)) packaged as an app: the same runtime and
generated module, cross-compiled for `x86_64-pc-windows-msvc`. Like the
tvOS and iOS apps ([tvos.md](tvos.md), [ios.md](ios.md)), it carries a
trimmed disc by default (the menu, The Heretic and Zanzibar); `--maps all`
packs every map, and the Multiplayer Map Pack with `--mappack`
("Building").

**Status (2026-11-16):** it runs on an Xbox Series X in Dev Mode, in App
mode (not yet switched to Game). It finds the console's GPU (`SraKmd_arden`,
DX12, 1920x1080 Bgra8, BC textures), loads the module through
`LoadPackagedLibrary`, mounts the disc and the hard disk, opens audio
(WASAPI, 48 kHz) and presents frames at 5 to 8 fps while the title boots.
Pads and levels are not checked yet. In App mode the process has about
1.7 GB of memory, and only about 130 MB was free once booted, with 512 MB
of dedicated GPU memory. **Switch it to Game in Dev Home** (see
"Installing and launching") before loading a level.

## How it is put together

| Piece | What it is | Built by |
| --- | --- | --- |
| `crates/uwp-host` (`h2uwp.exe`) | the app: a `CoreApplication` view (no XAML) whose render loop runs `h2host::start_in`, the same shell and `boot::run_hosted` the Apple apps use | `cargo build -Zbuild-std --target x86_64-pc-windows-msvc`, static CRT, `/APPCONTAINER` |
| `h2game.dll` (+ `h2game.dll.stamp`) | the generated module, loaded with `LoadPackagedLibrary` | `boot::build` with `H2_EMIT_TARGET=windows` (`apple-host` `pack --platform xbox`) |
| `disc/` | the disc files the app needs (`h2host::layout::DISC_FILES`), served as D: by `vfs::DirDiscBackend` | `pack` |
| `bootcache/` | the XBE's analysis for this build, copied to LocalState on first launch | `pack` |
| `AppxManifest.xml`, `Assets/` | the package manifest (`uwp_host::manifest`) and the app icon's tile, logo and splash images, embedded from `platform/xbox/Assets` ([art/app-icon](../../art/app-icon/README.md)) | `uwp-host` `package` example |

### The package images

`platform/xbox/Assets` holds the icon art the manifest names:
`Square150x150Logo`, `Square44x44Logo`, `Wide310x150Logo`,
`Square310x310Logo`, `StoreLogo` (the package `Logo`) and `SplashScreen`,
on the icon's background `#061323` for the tiles and the splash. Each is
the art's scale-200 export under a plain name. Qualified names
(`.scale-100`, `.scale-200`, `.targetsize-24_altform-unplated`) need a
`resources.pri` to choose between them, and only `makepri` on Windows makes
one; makemsix on a Mac does not. So the manifest names each file directly
and the shell scales the 2x image down where it shows a smaller one. There
is no `LockScreen` badge: it needs badge notifications and a background
task, which the app does not have. `uwp_host::manifest`'s tests check that
each image decodes at its size and that the folder holds exactly the files
the manifest names.

### Rendering without XAML

The `windows` crate has no XAML projection, and nothing on a Mac compiles
C++/WinRT. wgpu 30's DX12 backend has no `CoreWindow` target either. It has
`SwapChainPanel`, `CompositionVisual` (DirectComposition, desktop only) and
`SurfaceHandle`. The app uses the `SwapChainPanel` target without XAML.
`uwp_host::shell::Panel` is a COM object that implements
`ISwapChainPanelNative` itself. When wgpu calls `SetSwapChain` with its
composition swap chain, the panel wraps it with
`ICompositorInterop::CreateCompositionSurfaceForSwapChain` and sets it as
the brush of a `Windows.UI.Composition` sprite that fills the window
(`Compositor::CreateTargetForCurrentView`). A XAML `SwapChainPanel` does the
same thing internally. `app::embedded::host_surface` stays the only
platform seam, unchanged.

### The app's life

- `main` joins the multithreaded apartment (`RoInitialize`) before
  `CoreApplication::Run`, as C++/WinRT's `init_apartment` does. Without it,
  `Run` fails, the view never starts, and the console reports the launch as
  timed out (`0x8027025B`). A probe package found this: it reached `main`
  under both Rust targets but never reached the view.
- `IFrameworkView::Run` is the render loop on the UI thread. It processes
  window events, then redraws. The present waits for vblank, so it paces
  itself. While the window is hidden it blocks on events instead.
- **Suspending:** the handler takes a deferral. The loop pauses the guest
  (`Embedded::set_paused`), flushes the hard disk's pending metadata, then
  completes the deferral. **Resuming** unpauses.
- **Back** (B on the pad) is marked handled, so the system never treats it as
  leaving the app; the game sees B.
- **Bounds:** `ApplicationViewBoundsMode::UseCoreWindow`, so the game gets the
  whole screen and not the Xbox's TV-safe area. The 4:3 picture is fitted
  (`device_config`).
- **Files:** the app container refuses `std::fs::canonicalize` and
  `File::open` on a directory, even inside the app's own LocalState
  (ERROR_ACCESS_DENIED). On Windows the hard disk (`vfs::HostBackend`)
  therefore uses absolute paths as they are and skips directory syncs.
  The co-op screen's level list (`boot::coop_live::disc_levels`) reads a
  disc directory's `maps/mainmenu.map` file, found by listing, instead of
  opening the disc path as an image (it logged `opening ...\disc: Access
  is denied` and offered no levels).
  File syncs open the file for writing, because `FlushFileBuffers` needs
  write access. If the game cannot start, the log lists each file-system
  step with its OS error (`h2uwp: check ...`).
- **Logs:** a UWP process has no console. stderr and stdout are pointed at
  `LocalState\h2uwp.log` (`SetStdHandle`). Read it in the Device Portal's
  File explorer (LocalAppData, the package, LocalState).

### Paths

| | Where |
| --- | --- |
| disc (D:) | `<package>\disc` (read-only) |
| module | `<package>\h2game.dll`, opened by `LoadPackagedLibrary("h2game.dll")` |
| hard disk, settings, saves | `LocalState\h2data` (`ApplicationData.Current.LocalFolder`; survives updates) |
| boot cache | `LocalState\h2boot-cache`, seeded from `<package>\bootcache` |
| pipeline cache | `LocalState\cache\pipelines-v2.jsonl` (`H2_PIPELINE_CACHE`) |

`boot::load_module` uses `LoadPackagedLibrary` whenever the process has
package identity (`GetCurrentPackagePath`) and the module is inside the
package (`boot::package_relative`). Otherwise, on desktop Windows and
everywhere else, it uses libloading as before.

### Input and audio

- **Pads:** gilrs, whose Windows backend is Windows.Gaming.Input, the pad
  API UWP has. They sit behind the shell's player-one pad, as in `h2 play`.
  There is no Xbox-specific mapping code. Whether gilrs' WGI backend sees
  pads inside the app container is unverified. If it does not, the fallback
  is a direct `Windows.Gaming.Input.Gamepad` backend behind
  `input::HostBackend`.
- **Audio:** cpal's WASAPI host, opened by the guest's DirectSound
  (`h2host`'s sink opener). cpal enumerates devices through
  `IMMDeviceEnumerator`, which an app container may refuse. If it cannot
  open a device, the game runs silent and the log says
  `h2host: no audio device`.

### The C runtime

The Xbox has no desktop `vcruntime140.dll`. UWP apps normally get
`vcruntime140_app.dll` from the `Microsoft.VCLibs.140.00` framework package,
but the xwin splat has no import libraries for it. So both binaries link the
CRT statically:

- the executable through `-C target-feature=+crt-static`;
- the module through `-fms-runtime-lib=static` (`emit::compile::windows_target_args`,
  for every Windows build now).

The module carries its own copy. The generated C calls only the memory and
math functions and passes no CRT state across the boundary. Neither binary
depends on another package.

The x86_64-uwp-windows-msvc Rust target (tier 3) was not used. The tier 1
msvc target with the static CRT gives the same container-safe binaries, and
std, wgpu, gilrs and cpal are tested on it. The workspace already
cross-checks it. `build.sh` checks the result directly: the **API check**
(`uwp_host::imports`, the `apicheck` example) lists every function either
binary imports that `OneCoreUAP.lib` does not export. `OneCoreUAP.lib` is the
SDK's umbrella library for APIs present on every Windows 10 device family,
the Xbox included. An import outside it may not exist on the console, and
then the app fails to start with no log. The check fails the build.

What the check found, and what was done:

- wgpu's GL backend imports `opengl32.dll` (`wgl*`) and two `gdi32`
  pixel-format calls. The shell's instance is DX12 only on Windows
  (`app::embedded::host_surface::backends`), and the executable
  delay-loads both DLLs (`/DELAYLOAD`, `delayimp.lib`), so the loader
  never looks for them. The check skips delay-loaded imports.
- Five imports remain outside `OneCoreUAP.lib`, all from Rust's std for
  the msvc target: `bcryptprimitives!ProcessPrng`, and `ntdll!NtOpenFile`,
  `NtReadFile`, `NtWriteFile` and `RtlNtStatusToDosError`. The app imports
  them and launches on the console, so they are allowed
  (`uwp_host::imports::PROVEN_ON_CONSOLE`). Anything else outside the
  library fails the build. `H2_XBOX_API_CHECK=warn` packages anyway.
- `h2game.dll` imports only `KERNEL32.dll`, and all of it is inside the
  library.

## Building

On the Mac, you need:

- the Windows cross toolchain from [windows.md](windows.md) (llvm@22, lld@22,
  an xwin splat in `H2_XWIN`);
- `brew install osslsigncode jq`;
- `makemsix` from Microsoft's msix-packaging, built with pack support:

  ```sh
  git clone --depth 1 https://github.com/microsoft/msix-packaging "$TMPDIR/msix-src"
  cd "$TMPDIR/msix-src" && mkdir .vs && cd .vs
  cmake -DCMAKE_BUILD_TYPE=MinSizeRel -DXML_PARSER=xerces -DSKIP_BUNDLES=off \
    -DUSE_VALIDATION_PARSER=on -DMSIX_PACK=on -DMSIX_SAMPLES=off -DMSIX_TESTS=off \
    -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_TOOLCHAIN_FILE=../cmake/macos.cmake \
    -DUSE_MSIX_SDK_ZLIB=on -DMACOS=on -DCMAKE_POLICY_VERSION_MINIMUM=3.5 ..
  make -j6   # bin/makemsix and lib/libmsix.dylib; point H2_MAKEMSIX at makemsix
  ```

Then:

```sh
platform/xbox/build.sh /abs/path/to/disc/halo2.iso [--no-module]
```

Or `just xbox maps=all` (with `mappack=` for the map pack disc; `just`
finds `disc/mappack.iso` itself and warns about the size).

```sh
platform/xbox/build.sh /abs/path/to/disc/halo2.iso --maps all --mappack /abs/path/to/disc/mappack.iso
```

`--maps demo` (the default) packs the menu, The Heretic and Zanzibar
(about 1 GB). `--maps all` packs the whole disc. `--mappack ISO` (only with
`--maps all`) runs `h2 import-mappack` into the package's
`bootcache/h2data/`: the nine map pack content packages, which the app
copies into `LocalState\h2data` at its first launch, because the app
container can't clone or link out of the package and NTFS has no clones.
Expect a package of 7 GB or more, and the same again in LocalState once
launched. None of this changes the module.

This fills `platform/xbox/build/` (not committed). `package/` is the
package's contents. `h2recomp.msix` is the signed package, `h2recomp.cer` its
certificate, and `h2recomp.aumid` the app's id
(`h2recomp.Halo2_<publisher id>!App`, `uwp_host::identity`). `symbols/`
keeps this build's `h2uwp.exe`, its lld linker map `h2uwp.map` and
`h2game.dll`, which `xbox.sh profile` needs to name a console profile's
frames ("Profiling on the console"). The module is the
slow part: the whole-XBE lift and emit, then about 16 minutes of clang.
`--no-module` keeps an existing one. Rebuild it after any change to the
generator, the runtime ABI or `Cargo.lock`. Build it from the same tree as
the executable, because the stamp and the boot cache's analysis are keyed by
both. The app refuses a stale module at start-up and logs why.

Each build gets a higher version (`1.0.<yyddd>.<minute of the day>`, UTC),
so installing over an earlier build is an update and keeps LocalState.

**Signing.** `build.sh` makes a self-signed code-signing certificate on first
use. Its subject is `CN=h2recomp-dev`, which is the manifest's `Publisher`;
the two must match exactly. It keeps the certificate in `H2_XBOX_KEYS`,
default `$RUSH_KEEP/xbox-signing`, else
`~/Library/Application Support/h2recomp/xbox-signing`. Never commit it.
osslsigncode signs with the package's block-map hash (SHA-256).

## Installing and launching

Put the console in Developer Mode, open Dev Home, and turn on the Device
Portal with a username and password. Store the password in the Mac's
keychain:

```sh
security add-generic-password -s h2-xbox-device-portal -a afr -w
```

Then:

```sh
H2_XBOX=<console IP> platform/xbox/deploy.sh            # install and launch
H2_XBOX=<console IP> platform/xbox/deploy.sh --no-launch
```

`deploy.sh` (helpers in `portal.sh`) talks to the Device Portal REST API at
`https://<console>:11443`. The console's certificate is self-signed, so
curl runs with `-k`.

1. `GET /api/os/info` checks the credentials and receives the
   `CSRF-Token` cookie. Every later write sends the cookie back with an
   `X-CSRF-Token` header carrying the same value.
2. `POST /api/app/packagemanager/package?package=h2recomp.msix` sends a
   multipart upload of the package and `h2recomp.cer`, because the console
   has to trust the self-signed certificate.
3. It polls `GET /api/app/packagemanager/state`, which answers 204 while
   installing and 200 with the result.
4. `GET /api/app/packagemanager/packages` supplies the package's full name
   and `PackageRelativeId` (the AUMID).
5. `POST /api/taskmanager/app?appid=<base64 AUMID>&package=<base64 full name>`
   launches the app.

The password goes from `security` straight into a mode-600 netrc file in a
temporary directory, which is deleted on exit. It never appears on a command
line or in the output. `platform/xbox/tests/portal_test.sh` (run by
`cargo test -p uwp-host`) checks the requests, the CSRF handling and the
netrc file against stand-in `curl` and `security` commands.

**Game, not App.** A UWP app on Xbox gets the small app resource share
(about 1 GB of memory and a slice of the GPU) unless it is marked as a game.
The Device Portal REST API has no documented call for that, so it is done by
hand once per install: in Dev Home, select the app, then View details, then
change App type to **Game**. Without it the game may run out of memory
loading a level.

## Debugging on the console

- `deploy.sh --launch-only` relaunches what is installed.
- The log is `LocalState\h2uwp.log`:
  `GET /api/filesystem/apps/file?knownfolderid=LocalAppData&packagefullname=<full>&filename=h2uwp.log&path=%5CLocalState`.
- Memory and GPU use: `GET /api/resourcemanager/processes` and
  `/api/resourcemanager/systemperf`.
- A launch error of `0x8027025B` ("didn't start in the required time") with
  an empty LocalState means the process died or hung before the view ran.
  Bisect with a tiny exe in its own package that only writes a file to
  LocalState; a package can list several `Application`s, one per exe.

## Where frame time goes

The app logs one **host perf line** every 100 guest frames or 5 s
(`boot::host_perf`), on by default. A UWP app gets no environment, so the
switch is a file: an empty `LocalState\host-perf-off` turns it off (and
`host-perf-on` turns it on if the default in `uwp_host::switches` is ever
off). The app sets `H2_HOST_PERF=1` for the game, which works the same way
for `h2 run`, `h2 play` and the Apple apps, so a Mac run gives the same line
to compare.

It reuses frame stats (`timeline::phase`, [app.md](app.md) "Frame stats"),
started quietly (no line a second, but the whole-run report at exit), and
adds `timeline::host`: the UI thread's swap-chain waits, the pacer's sleeps,
and each thread's CPU time. Off, every hook is a relaxed load. On, it is two
clock reads per timed call.

Once at start-up:

```
app: dx12 presentation ..., frame latency waitable object Wait
app: present mode AutoVsync (surface offers [Fifo, ...]), max frame latency 2, alpha ..., 6 logical CPUs
host perf: on (H2_HOST_PERF), ...; 6 logical CPUs; render thread on; ...; host timer: a 1 ms condvar wait took 1.05-1.20 ms, a 1 ms thread::sleep 1.01-1.10 ms
```

The host timer probe answers whether Windows' 15.6 ms timer tick rounds
waits up: on a timer at its default rate the condvar wait (what the pacer
sleeps on) takes about 15.6 ms instead of 1. `logical CPUs` is what the
process may use (App mode gets a share of the console; Game mode more).

Then, each interval:

```
host perf: frames 100 in 13.91 s, 7.19 fps; frame ms 139.1/152.0
  | game cpu 118.2/131.0 d3d 4.1/6.0 decode 0.3/0.6 render-wait 9.0/30.1 pace 0.0/0.0 input 0.05/0.30
  | render encode 21.0/25.2 submit 2.1/3.0 gpu-wait 70.2/80.1
  | ui 60.0/s acquire 15.1/16.9 missed 0 submit 0.1/0.4 present 0.4/1.2
  | sleeps 0 0.0 ms over 0.0/0.0
  | fallback 2.0/frame, 310 insns/frame
  | draws 300 submits 26 read 1.2 KB/frame
  | cpu game 97% (kernel 3%) render 61% ui 9% process 180% of 6
```

(one line in the log). `a/b` is mean/max in ms.

| part | what it is |
| --- | --- |
| `frame ms` | wall time per guest frame (present to present) |
| `game cpu` | the game thread running guest code, the runtime, the kernel and HLE: the guest's CPU time per frame |
| `game d3d`, `decode` | our D3D entries and the push-buffer decode on the game thread: the D3D translation |
| `game render-wait` | the game thread blocked on the render thread: a read-back, or the frame-in-flight limit (two presents ahead) |
| `game pace` | the game thread sleeping because the guest is ahead of wall time (the shell's pacer). Slack, not cost |
| `game input` | the input pump between slices, per frame (sum), and its longest call |
| `render encode`, `submit`, `gpu-wait` | the render thread per frame: wgpu recording and uploads, `queue.submit`, and blocking GPU waits (`D3DSWAP_FINISH`, read-backs, fences). `backend (game thread)` instead when the render thread is off |
| `ui` | redraws a second, then per call: swap-chain acquire (`get_current_texture`; with vsync, the wait for the display), acquires that gave no texture, the compositor's submit, `present` |
| `sleeps` | pacing sleeps in the interval, their total, and the oversleep (taken minus asked) per sleep: the host timer's granularity |
| `fallback` | fallback interpreter entries and instructions per frame. Compare with a Mac run: hot here and not there would point at the module |
| `draws`, `submits`, `read` | per frame: draws, `queue.submit`s, file bytes the guest read |
| `cpu` | each thread's CPU time over the interval's wall time (`GetThreadTimes`; the process's with `GetProcessTimes`), and the CPUs available. `kernel` is the game thread's kernel-mode share (system calls, page faults), Windows only |

How to read it:

- `cpu game` near 100% and `game cpu` near `frame ms`: the game thread is
  busy all frame. The guest's CPU work is the limit (a slower CPU, or a
  smaller CPU share in App mode).
- `cpu game` well under 100% with `game cpu` near `frame ms`: the thread
  waits somewhere frame stats does not time (a lock, the OS taking the core
  away). Check `process` against the CPU count.
- `render-wait` large: the render thread is the limit. Its `encode` and
  `gpu-wait` say whether that is CPU recording or the GPU.
- `pace` or `sleeps` with large `over`: timer granularity is stretching the
  pacer's sleeps.
- `ui acquire` near 16.7 ms is the display's vsync on the UI thread. It does
  not hold the game thread up by itself, but wgpu holds the device's locks
  across a DX12 present, so a slow `present` can stall the render thread's
  submits.

Guest waits (`KeWaitForSingleObject` timeouts, `KeDelayExecutionThread`,
timers, the vblank and `D3DDevice_Present`'s frame wait) run on the
kernel's logical clock: the scheduler jumps the clock to the next deadline
when every thread waits, so they never wait on a host timer. The only
host-timed waits on the game thread are the pacer's sleep (`pace`, which
only sleeps when the guest is ahead of wall time) and the render thread's
replies (`render-wait`).

## Remote control

The app runs the agent API ([agent-api.md](agent-api.md)) on TCP port
4720 (`agent_proto::XBOX_PORT`), so the Mac drives the console the way it
drives `h2 serve`. It is on by default (`uwp_host::switches`); an empty
`LocalState\agent-off` turns it off, `agent-on` back on if the default
ever changes. The log says `h2uwp: agent API on tcp://0.0.0.0:4720` at start.
A port that won't bind is logged and the game runs without it.

The manifest asks for `internetClientServer` and
`privateNetworkClientServer` (with `internetClient`), which a UWP app
needs to accept connections at all. **There is no authentication**: anyone
on the LAN who reaches the port can press buttons and read the game. It is
for a dev console on a home network only.

```sh
export H2_XBOX=10.0.3.29
cargo h2 agent --xbox status                 # or --xbox 10.0.3.29; tcp://HOST:4720
cargo h2 agent --xbox press a                # A for 150 ms
cargo h2 agent --xbox press a+start 300
cargo h2 agent --xbox stick left 0 1 2000    # full forward for 2 s
cargo h2 agent --xbox pad '{"steps":[{"buttons":["a"],"ms":100},{"ms":500},{"buttons":["b"],"reads":2}]}'
cargo h2 agent --xbox unpress                # drop what is queued, release all
cargo h2 agent --xbox padstate
cargo h2 agent --xbox hostperf               # the last host perf line
cargo h2 agent --xbox watch phase map
```

`h2 agent --socket tcp://HOST:PORT` (or `H2_AGENT=tcp://...`) reaches any
TCP endpoint, not only the console's.

**The pad.** `input.pad` queues steps for player one (`input::backend::remote`),
merged over a real pad and the keyboard. A step holds its buttons and
sticks for `ms` and until the game has read the pad `reads` times (default
once), so a short press is never lost to a slow frame. Buttons: `a b x y
black white lt rt` (analog, fully pressed), `up down left right start back
ls rs`; sticks are -1 to 1 (`left`, `right`), triggers 0 to 1 (`lt`, `rt`).
`append` adds to what is queued, otherwise it replaces it. When the queue
empties the pad is released. Pressing needs control (`session.claim`)
only while another connection holds it.

**Files and launching.** `platform/xbox/xbox.sh` wraps the Device Portal
(the same credentials as `deploy.sh`) and `h2 agent`:

```sh
platform/xbox/xbox.sh level 03a_oldmombasa heroic   # boot straight into a level next launch, and relaunch
platform/xbox/xbox.sh level menu                    # back to the menu
platform/xbox/xbox.sh env H2_FRAME_STATS=1 H2_LEVEL=05a_deltaapproach
platform/xbox/xbox.sh log | tail
platform/xbox/xbox.sh get h2uwp.log /tmp/h2uwp.log
platform/xbox/xbox.sh relaunch
platform/xbox/xbox.sh agent press a
```

A UWP app gets no environment, so the app reads `LocalState\h2env` at
launch: `H2_NAME=VALUE` lines (comments, blank lines, `export` and quotes
allowed; anything not `H2_` is ignored and logged). `xbox.sh env` writes
it, with no pairs deletes it; `level` writes `H2_LEVEL` and
`H2_DIFFICULTY` and relaunches. Stopping is
`DELETE /api/taskmanager/app?package=<base64 full name>&forcestop=yes`;
LocalState files go through `/api/filesystem/apps/file` with
`knownfolderid=LocalAppData` and `path=\LocalState`.

## Profiling on the console

The app has a sampling profiler (`perf::sample`, Windows x86-64 in
`sample_windows.rs`). Each sample suspends the game thread, reads its
registers and copies its stack into a buffer allocated beforehand, and
resumes it (a few tens of microseconds). The copy is then unwound with
`RtlLookupFunctionEntry` and `RtlVirtualUnwind` over each module's
`.pdata`, as Chromium's sampler does: no lock is taken with the thread
stopped. CPU-or-not comes from `QueryThreadCycleTime`. All of it is in
`OneCoreUAP.lib`.

The console can't name its frames, so it writes them module-relative
(`perf::remote`, `h2prof 1`): `LocalState\profile.h2prof`. The Mac names
them against the same build's `build/symbols/`: for `h2game.dll`, its
exported `h2gen_module` table (each lifted function's guest address) and
`.pdata` extents, with the names from `disc/ghidra-functions.tsv`; for
`h2uwp.exe`, the linker map. No PDB is needed.

```sh
platform/xbox/xbox.sh profile 20         # 20 s at 1000 Hz, fetched and symbolised
platform/xbox/xbox.sh profile 10 4000
platform/xbox/xbox.sh symbolize /tmp/xbox-20261117-101500.h2prof
cargo h2 agent --xbox sample-start 1000  # by hand
cargo h2 agent --xbox sample-status
cargo h2 agent --xbox sample-stop
```

`xbox.sh profile` prints the top table (guest names where known) and
writes `profile.folded` (for a flame graph), `profile.txt` and
`profile.json` next to the dump (`$TMPDIR`, or `H2_XBOX_PROFILE_DIR`). The
symboliser is `cargo run -p perf --example remote_profile -- DUMP --image
h2game.dll --image h2uwp.exe --map h2uwp.map --names TSV [--top N --cpu
--by-category --full-stacks]`. A dump only resolves against the exact
build that made it: rebuild, redeploy, profile again.

Without the agent (it is off, or the port isn't reachable), the same run
is two `h2env` lines: `H2_PERF_FROM=level` (or a present count) and
`H2_SAMPLE_SECONDS=20` sample from the level's start and write the dump;
then `xbox.sh get profile.h2prof` and `xbox.sh symbolize`.

## Known gaps

- Remote control and the profiler are untested on the console: whether the
  console's firewall lets port 4720 in for a Dev Mode app is unknown.
  `xbox.sh agent status` is the first check; the `h2env` route above is
  the fallback.
- Pads (gilrs' WGI backend in the app container) are unverified. With no pad
  the log shows nothing, and with a failed backend it shows
  `app: gamepads unavailable`.
- App mode memory: switch to Game in Dev Home. Levels are untested.
- Speed: 5 to 8 fps while booting in App mode, where the CPU share is small.
  Game mode is the first thing to measure.
- If the game stops, the reason goes to the log only, not the screen.
- Suspend does not trim GPU memory or save the game.
- Other `canonicalize` users (`vfs::map_override` for `H2_MAP_DIR` and map
  packages, `content`) may hit the same container refusal when used.
- By default the same trimmed disc as tvOS (the menu, The Heretic and
  Zanzibar); `--maps all` for the rest. No Xbox Live, and no movies.
- The map pack is copied into LocalState at the first launch after an
  install that carries it, which takes a while and doubles its space.
