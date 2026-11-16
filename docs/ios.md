# iPhone and iPad (iOS)

The game runs on an iPhone or iPad as an app built from `platform/ios/`. It is
the tvOS app's sibling ([tvos.md](tvos.md)): the same Rust runtime, game
module build and trimmed disc, with a UIKit host for touch. It needs a real
device (arm64, iOS 17 or later); there is no simulator build. Landscape only.

## How it is put together

| Piece | What it is | Built by |
| --- | --- | --- |
| `crates/apple-host` | the runtime as a static library (`libh2host.a`), shared with tvOS: `boot::run_hosted` inside `app::embedded::Embedded` | `cargo build -Zbuild-std --target aarch64-apple-ios` |
| `h2game.framework` | the generated module built for `arm64-apple-ios17.0`, embedded and signed by Xcode, loaded with `dlopen` | `boot::build` with `H2_EMIT_TARGET=ios` (`crates/apple-host/examples/pack.rs --platform ios`) |
| `disc/`, `bootcache/` | the same disc subset and analysis as tvOS (`h2host::layout::DISC_FILES`) | `pack` |
| `platform/ios/H2iOS` | a UIKit host: a view controller whose view is a `CAMetalLayer`, a 30 Hz `CADisplayLink`, the on-screen controls and the keyboard | Xcode |
| `platform/apple` | what both apps share: the C API (`h2host.h`), the keyboard view (`GameKeyInput.swift`) and `build.sh` | |

As on tvOS, the winit shell is compiled out and the Swift host hands its layer
to Rust. The app uses the scene lifecycle (tvOS 27 refuses to launch without
it), hides the status bar and home indicator, and defers system edge gestures,
so a swipe from the edge reaches the controls first and a second one still
opens Control Centre or goes home.

Saves, settings and the Xbox hard disk (`h2data/`) go to the app's Documents
directory, which lasts. The boot cache and built pipelines go to Caches.

## Controls

**A controller** (Xbox, PlayStation, or any MFi/extended gamepad) plays as on
the Mac. The button prompts follow the pad's type (`H2_ICONS=auto`;
docs/button-icons.md).

**Touch.** With no controller connected, the app shows Apple's on-screen
controls (`GCVirtualController`): both sticks, A, B, X, Y, both triggers, and
both shoulders as White (left) and Black (right). There is no d-pad: the
system's touch controller refuses one alongside the left stick, and the
game's menus move with the stick. The game sees
them as one more gamepad, so nothing in the game's input path is
touch-specific and the prompts show Xbox buttons. The touch controller also
refuses Menu and Options, so Start and Back are two small buttons of the
app's own at the top centre: `h2host_button` holds the key bound to Start or
Back on the player-one pad (`KeyboardMouse::press_control`), which shares
port 0 with every other device.

The overlay goes away when a controller connects and comes back when the last
one disconnects. A controller counts if it has an extended gamepad profile and
is not the overlay itself; a Siri Remote paired to an iPad doesn't. It also
hides while the system keyboard is up. The rule is `h2host_pad_counts` and
`h2host_overlay_wanted` (`crates/apple-host/src/overlay.rs`, unit-tested).

## Typing

When the game's virtual keyboard has a field open (a gamertag, a variant
name), the app makes an invisible `UIKeyInput` view first responder and the
system keyboard comes up. Typed text, Delete and Return (Done) go into the
game's field through the same path as typing on the Mac
(docs/host-keyboard.md): the game's own insert, Backspace and Done code runs,
with its own limits and checks. Only committed text arrives, never marked
(composing) text. A hardware keyboard on an iPad types into the field the same
way, and Escape cancels it.

Hiding the system keyboard does not cancel the field: the controls come back,
B or Back backs out as the game's keyboard does, and tapping the screen brings
the system keyboard back. When the game closes the field, the keyboard goes
away by itself. The edge detection and the text-to-edit mapping are
`crates/apple-host/src/text.rs` (unit-tested).

## Building

You need Xcode with the iOS SDK, the pinned Rust toolchain with its `rust-src`
component (Homebrew's `rust` ships it, but no prebuilt iOS `std`), and the disc
image. `build.sh` builds `std` from source with `-Zbuild-std` and
`RUSTC_BOOTSTRAP=1`, as for tvOS.

```sh
platform/ios/build.sh /abs/path/to/disc/halo2.iso
```

This fills `platform/ios/build/` (not committed): `libh2host.a`,
`h2game.framework` (the full lift, emit and clang for iOS, about 15 minutes;
the C and objects stay in `$TMPDIR/h2ios-gen`), `disc/` and `bootcache/`.
`--no-module` keeps an existing framework; rebuild it whenever the generator or
the runtime ABI changes (the app checks the stamp, which names
`arm64-apple-ios17.0`, and refuses a stale module). If you already have the
tvOS `build/disc`, clone it first to skip the extraction:
`cp -cR platform/tvos/build/disc platform/ios/build/disc`.

`H2iOS.xcodeproj` is generated from `project.yml` with `xcodegen generate` (in
`platform/ios`); both are committed.

## Signing and installing

Put your team in `platform/ios/Local.xcconfig` (not committed):

```
DEVELOPMENT_TEAM = ABCDE12345
```

The bundle id is `red.forbes.h2recomp.ios`; override
`PRODUCT_BUNDLE_IDENTIFIER` there if that one is taken for your team.

Connect the iPhone or iPad by cable once (or pair it in Xcode's Devices and
Simulators window), trust the Mac, and turn on Developer Mode on the device
(Settings > Privacy & Security > Developer Mode). `xcrun devicectl list
devices` should then show it as available. Then, from `platform/ios`:

```sh
xcodebuild -project H2iOS.xcodeproj -scheme H2iOS -configuration Release \
  -destination 'generic/platform=iOS' -allowProvisioningUpdates \
  -derivedDataPath DerivedData build
xcrun devicectl device install app --device '<device name>' \
  DerivedData/Build/Products/Release-iphoneos/H2iOS.app
xcrun devicectl device process launch --console --device '<device name>' red.forbes.h2recomp.ios
```

The last command streams the game's log (boot, `present N: ... fps` lines, and
why it stopped). The first launch of a newly signed app may need the
developer profile trusted on the device (Settings > General > VPN & Device
Management).

## Known gaps

- The touch controls have no d-pad and no stick clicks (crouch, zoom).
- The same trimmed disc as tvOS: only the menu, The Heretic and Zanzibar.
  Choosing another level or map fails to load it.
- Speed is unmeasured. Older devices (A12 and earlier) will be slow.
- The app may need the increased memory limit entitlement on devices with
  little RAM; it does not ask for it yet.
- No Xbox Live, no controller speaker, no `--level`.
- No touch aiming or gyro: the right stick is the overlay's thumbstick.
