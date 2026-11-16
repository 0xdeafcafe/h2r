# Xbox (Dev Mode, UWP)

The game runs on an Xbox Series X|S in Developer Mode as a UWP app,
built entirely on a Mac. It is the Windows port
([windows.md](windows.md)) packaged as an app: the same runtime and
generated module, cross-compiled for `x86_64-pc-windows-msvc`. Like the
tvOS and iOS apps ([tvos.md](tvos.md), [ios.md](ios.md)), it carries a
trimmed disc and only the menu, The Heretic and Zanzibar are playable.

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
| `AppxManifest.xml`, `Assets/` | the package manifest (`uwp_host::manifest`) and plain placeholder images | `uwp-host` `package` example |

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

This fills `platform/xbox/build/` (not committed). `package/` is the
package's contents. `h2recomp.msix` is the signed package, `h2recomp.cer` its
certificate, and `h2recomp.aumid` the app's id
(`h2recomp.Halo2_<publisher id>!App`, `uwp_host::identity`). The module is the
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

## Known gaps

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
- The same trimmed disc as tvOS: the menu, The Heretic and Zanzibar only.
  No Xbox Live, and no movies.
- The package images are plain placeholders.
