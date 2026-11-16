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
docs/input/button-icons.md).

**Touch.** With no controller connected, the app draws its own touch
controls (`platform/ios/H2iOS/TouchControls.swift`), plain shapes in the
layout below. They replaced Apple's `GCVirtualController`, which had no
stick clicks (so no crouch or zoom), refused a d-pad beside the left stick
and Menu and Options, and on an iPhone fire did not work. Our side of that
path is tested (`map_snapshot` carries a pad's right trigger through;
`touch_fire_reaches_player_one_as_a_full_right_trigger` now covers touch
end to end), so the fault was in the system overlay on the phone: its
trigger missing, covered, or never reporting. Which one was not
established.

```
 [LIGHT]                 [Back] [Start]

                                              [SWITCH]
                                    [ZOOM]               [RELOAD]
                                              ( FIRE )
   (move stick appears                                    [MELEE]
    where the thumb lands)       [GRENADE]
                         [SWAP GREN]          [CROUCH]   [JUMP]
```

* **Left half: move.** A thumb landing anywhere on the left side, away from
  a button, puts the stick there; its centre follows a thumb that runs past
  the rim, so turning back answers at once. 56 points is full deflection,
  with a 12% radial deadzone.
* **Right half: look.** Dragging anywhere on the right side turns, as a
  mouse or the Steam Controller's trackpad does: the drag's speed is the
  turn rate, with a little acceleration and a response curve that lifts
  slow drags (Halo's own stick response is flat near the centre). **Fire**
  looks too: a thumb on Fire fires and aims at the same time.
* **Buttons**, by Halo 2's Default layout: Fire (right trigger), Grenade
  (left trigger), Jump (A), Melee (B), Reload (X), Switch (Y), Crouch
  (left-stick click), Zoom (right-stick click), Swap Gren (Black), Light
  (White), Back and Start. A finger holds the button it landed on until it
  lifts. Another layout (Southpaw, Boxer, ...) moves the actions, not the
  labels. There is no d-pad: Halo 2 does not use one in play and its menus
  move with the stick (the C API has the bits if one is wanted).
* **Crouch** is a toggle by default (it stays lit while crouched); the
  Touch Crouch setting makes it hold-to-crouch.

The view only tracks fingers. Each change, and once a frame, it sends an
`H2TouchFrame` (held buttons, the move thumb's offset, the look drag since
the last call, the gyro's rate) through `h2host_touch_update`. Rust
(`crates/input/src/touch.rs`, unit-tested) turns it into a pad: the stick
deadzone, the look curve (through `crates/input/src/look.rs`, so look
motion is held until the game reads it, as mouse look is), the crouch
toggle. The keyboard pad merges it into player one
(`KeyboardMouse::touch_update`), so it shares port 0 with every other
device, and the game shows pad prompts while touch is in use. The app logs
each change of the touch pad (`h2host: touch RT look(+0.25,+0.00)`), at
once for buttons and at most four times a second for sticks.

Settings, on the Controls screen (they replace the Steam Controller's rows
on iOS): **Touch Look Speed** 1 to 10 (5), **Touch Crouch** Toggle or Hold,
**Gyro Aim** 0 (off, the default) to 10: turning the device aims as well
(CoreMotion, only running while it is on). **Aim Assist** (Video screen)
is Auto by default, which is Strong while the touch controls show
([docs/game/aim-assist.md](../game/aim-assist.md)).

The overlay goes away when a controller connects and comes back when the last
one disconnects. A controller counts if it has an extended gamepad profile; a
Siri Remote paired to an iPad doesn't. It also hides while the system
keyboard is up. Hiding lets go of everything it held. The rule is
`h2host_pad_counts` and `h2host_overlay_wanted`
(`crates/apple-host/src/overlay.rs`, unit-tested).

## Typing

When the game's virtual keyboard has a field open (a gamertag, a variant
name), the app makes an invisible `UIKeyInput` view first responder and the
system keyboard comes up. Typed text, Delete and Return (Done) go into the
game's field through the same path as typing on the Mac
(docs/input/host-keyboard.md): the game's own insert, Backspace and Done code runs,
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
`--no-module` keeps an existing framework; rebuild it whenever the generator,
the runtime ABI or `Cargo.lock` changes (the app checks the stamp, which names
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

- The touch layout is not adjustable yet (positions, sizes, opacity).
- The same trimmed disc as tvOS: only the menu, The Heretic and Zanzibar.
  Choosing another level or map fails to load it.
- Speed is unmeasured. Older devices (A12 and earlier) will be slow.
- The app may need the increased memory limit entitlement on devices with
  little RAM; it does not ask for it yet.
- No Xbox Live, no controller speaker, no `--level`.
- Gyro aiming's axes assume the device held in landscape like a camera;
  it is untested on hardware.
