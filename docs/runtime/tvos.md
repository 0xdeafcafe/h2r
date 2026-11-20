# Apple TV (tvOS)

Status: Partly built. A spike: the app boots to the menu and plays the bundled levels slowly ("Known gaps").
Code: `platform/tvos/`, `crates/apps/apple-host`, `crates/platform/input/src/backend/game_controller/remote.rs`. Tests: in `remote.rs`.
Switch: `H2_EMIT_TARGET=tvos` and the build variables, see [switches.md](switches.md).

The game runs on an Apple TV as an app built from `platform/tvos/`. It is a
spike: expect a few frames a second, and only the menu, The Heretic and
Zanzibar on board. It needs a real Apple TV (arm64, tvOS 17 or later); there
is no simulator build.

## How it is put together

| Piece | What it is | Built by |
| --- | --- | --- |
| `crates/apps/apple-host` | the runtime as a static library (`libh2host.a`), shared with the iOS app ([ios.md](ios.md)): `boot::run_hosted` as `h2 play` runs it, inside `app::embedded::Embedded`, the shell for a host that owns the window | `cargo build -Zbuild-std --target aarch64-apple-tvos` |
| `h2game.framework` | the generated module (`libh2game.dylib` built for `arm64-apple-tvos17.0`), embedded and signed by Xcode, loaded with `dlopen` as on the Mac | `boot::build` with `H2_EMIT_TARGET=tvos` (`crates/apps/apple-host/examples/pack.rs --platform tvos`) |
| `disc/` | the disc files the app needs, as a directory (`vfs::DirDiscBackend` serves it as D:) | `pack` (`h2host::layout::DISC_FILES`) |
| `bootcache/` | the XBE's analysis, copied to Caches on first launch so the fallback interpreter need not redo it | `pack` |
| `platform/tvos/H2TV` | a UIKit host: a `GCEventViewController` whose view is a `CAMetalLayer`; a 30 Hz `CADisplayLink` calls `h2host_redraw` | Xcode |
| `H2TV/Assets.xcassets` | the app icon and Top Shelf images: `App Icon & Top Shelf Image.brandassets`, with the layered App Icon (400x240 @1x/@2x) and App Store (1280x768) stacks, Front, Middle and Back, and Top Shelf Image and Top Shelf Image Wide at @1x/@2x ([art/app-icon](../../art/app-icon/README.md)) | Xcode (actool), named by `ASSETCATALOG_COMPILER_APPICON_NAME` in `project.yml` |

winit has no tvOS backend, so the Swift host owns the window and hands the
layer to Rust (`platform/apple/h2host.h`, shared with iOS); wgpu renders into it through Metal. On tvOS
`app::run` and the winit shell are compiled out; the frame handoff, compositor,
pacing and guest thread are the same as `h2 play`'s.

Writes go to the app's Caches directory: `h2data/` (the hard disk, settings
and **saves**) and `h2boot-cache/`. tvOS may purge Caches when it is short of
space, so saves can disappear. The pipeline cache goes to
`Library/Caches/h2recomp` as on the Mac.

## Building

You need Xcode with the tvOS SDK, the Rust toolchain the repository pins with
its `rust-src` component (`rustup component add rust-src`; Homebrew's `rust`
ships it), and the disc image. `aarch64-apple-tvos` is a tier 3 Rust target:
`build.sh` (a wrapper for `platform/apple/build.sh tvos`) builds `std` from source with `-Zbuild-std` and
`RUSTC_BOOTSTRAP=1`, on the pinned compiler. The Mac build is unaffected.

```sh
platform/tvos/build.sh /abs/path/to/disc/halo2.iso
```

This fills `platform/tvos/build/` (not committed):

- `libh2host.a` (about 70 MB);
- `h2game.framework`, about 345 MB (the lift, emit and clang of the whole
  XBE for tvOS, like `h2 build`: about 560 MB of C and 600 MB of objects, 15
  minutes of clang on a busy 10-core Mac; the C and objects stay in
  `$TMPDIR/h2tvos-gen`, which you can delete afterwards);
- `disc/` (about 920 MB) and `bootcache/` (20 MB).

`build.sh --no-module` keeps an existing framework. Rebuild it whenever the
generator, the runtime ABI or `Cargo.lock` changes (the boot cache's analysis
is keyed by `Cargo.lock` too): the app compares the module's stamp
(`h2game.stamp`, which for tvOS names the `arm64-apple-tvos17.0` target) and
refuses a stale one at start-up, with a message on screen. Leave `H2_FASTMEM`,
`H2_X87_NATIVE`, `H2_PASSES` and the other generator switches unset; the app
computes the stamp with their defaults, and `pack` refuses to run with any set.

`H2TV.xcodeproj` is generated from `project.yml` by `xcodegen generate` (in
`platform/tvos`); both are committed, so only Xcode is needed to build.
Regenerate it after adding or removing files under `H2TV` (xcodegen needs
`build/disc` and `build/bootcache` to exist, even empty).

## Signing and installing

Signing is automatic. Set your team once, in `platform/tvos/Local.xcconfig`
(not committed; `Config.xcconfig` includes it):

```
DEVELOPMENT_TEAM = ABCDE12345
// Optional: a bundle id of your own if this one is taken on your team.
PRODUCT_BUNDLE_IDENTIFIER = com.example.h2recomp.tv
```

Your team id is in Xcode (Settings, Accounts, the team) or in
`security find-identity -v -p codesigning` (the parentheses after your name).

### Pairing the Apple TV with Xcode (once)

1. Put the Mac and the Apple TV on the same network.
2. On the Apple TV: Settings > Remotes and Devices > Remote App and Devices.
   Leave that screen open.
3. In Xcode: Window > Devices and Simulators. The Apple TV appears under
   Discovered; select it and click Pair, then type the code the TV shows.
4. Check from a terminal: `xcrun devicectl list devices` lists it as
   `available (paired)`. Use the name it shows below (here, `Living Room`).

### Install from Xcode

Open `platform/tvos/H2TV.xcodeproj`, pick the Apple TV as the run
destination, and Run. The first time, Xcode registers the device and makes the
provisioning profile.

### Install from the command line

```sh
cd platform/tvos
xcodebuild -project H2TV.xcodeproj -scheme H2TV -configuration Release \
  -destination 'platform=tvOS,name=Living Room' \
  -allowProvisioningUpdates -derivedDataPath DerivedData build
xcrun devicectl device install app --device 'Living Room' \
  DerivedData/Build/Products/Release-appletvos/H2TV.app
xcrun devicectl device process launch --console --device 'Living Room' \
  red.forbes.h2recomp.tv
```

(Use your own bundle id in the last line if you set one.) `--console` streams
the game's stderr (`tvos:`, `app:`, the run report) to the terminal; Xcode's
console shows the same when run from there. The app is about 1.2 GB, so the
first install takes a while over the network.

## Controls

A paired controller (Xbox, PlayStation or any MFi extended gamepad) plays as on
the Mac. The Apple TV remote works in the menus:

| Remote | Xbox pad |
| --- | --- |
| click the middle of the clickpad | A |
| click the top, bottom, left or right edge | d-pad that way, held while clicked |
| swipe | one d-pad step per half a clickpad of travel (`input::backend::game_controller::remote`) |
| Back (Menu on older remotes) | B |
| Play/Pause | Start |

The remote has no sticks or triggers, so it cannot play a level. It is merged
into the first controller, so both work at once; on its own it is player one.
The app's view controller is a `GCEventViewController` with controller
interaction off, so Back reaches the game instead of leaving the app; the TV
button still goes home. With `H2_ICONS=auto` (the default here) the prompts
show the remote's buttons after it was last used, and the pad's after the pad
was (`--icons remote` / `H2_ICONS=remote` fixes it; docs/input/button-icons.md).

## Typing

When the game's virtual keyboard has a field open (a gamertag, a variant
name), the app makes an invisible `UIKeyInput` view
(`platform/apple/GameKeyInput.swift`) first responder, which brings up the
system keyboard, and turns controller interaction on so the remote drives
it. What is typed goes into the game's field through the same path as the
Mac's typing (docs/input/host-keyboard.md); Done submits, Back on the game's own
keyboard still cancels.

## Known gaps

- Run on an Apple TV 4K (3rd generation, A15): it boots to the menu with the
  remote, its icons and sound working. Levels and their frame rate have not
  been measured yet.
- The system keyboard for the game's text fields is unverified on tvOS
  (`UIKeyInput` may not bring up the tvOS keyboard).
- Only the files in `h2host::layout::DISC_FILES` are bundled: `mainmenu`,
  `shared`, `single_player_shared`, `00a_introduction` and `zanzibar`, the
  fonts, `media/` and the intro movie. Choosing another level or map makes the
  game fail to open it (it may stop or hang on the loading screen). The map
  pack is not bundled. `just tvos maps=all` bundles the whole disc and the
  map pack instead (7 GB or more; [building.md](building.md)).
- Speed: expect 3 to 4 fps in a level. Nothing is tuned for the A15 yet.
- No BC textures on the Apple TV's GPU: DXT textures are decoded on the CPU
  (`gfx::dxt`), which costs load time and memory.
- Saves and settings live in Caches and can be purged by tvOS.
- Audio uses the default output through cpal; if it fails to open, the game
  runs silent (the log says so).
- Xbox Live is on by default, against `https://live.forbes.red`;
  `live.toml` in the data root (`enabled = false`, or `server = "URL"`)
  or `H2_LIVE_SERVER` in the scheme changes that ([live.md](../live/live.md)
  "Playing on Live"). Signing in needs an account in the data root's
  `live/accounts.json` until the in-game account setup has a screen; with
  one, saves are synced ([live-cloud-saves.md](../live/live-cloud-saves.md)).
  The data root is in Caches here, so tvOS may purge the account key too.
- No controller speaker, no `--level`.
- The app must be rebuilt (with `build.sh`) for every generator change; the
  module is about 345 MB and the bundle 1.2 GB, so installs are slow.
