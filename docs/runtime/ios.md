# iPhone and iPad (iOS)

Status: Built. A guide to the iPhone and iPad app; touch, speed and older devices are under "Known gaps".
Code: `platform/ios/`, `crates/apps/h2host`, `crates/platform/input/src/touch.rs`. Tests: `crates/platform/input/src/touch_tests.rs`.
Switch: `H2_TOUCH_LOOK_ACCEL`, `H2_TOUCH_AUTO_FIRE`, `H2_ICONS`, see [switches.md](switches.md).

The game runs on an iPhone or iPad as an app built from `platform/ios/`. It is
the tvOS app's sibling ([tvos.md](tvos.md)): the same Rust runtime, game
module build and trimmed disc, with a UIKit host for touch. It needs a real
device (arm64, iOS 17 or later); there is no simulator build. Landscape only.

## How it is put together

| Piece | What it is | Built by |
| --- | --- | --- |
| `crates/apps/h2host` | the runtime as a static library (`libh2host.a`), shared with tvOS: `boot::run_hosted` inside `app::embedded::Embedded` | `cargo build -Zbuild-std --target aarch64-apple-ios` |
| `h2game.framework` | the generated module built for `arm64-apple-ios17.0`, embedded and signed by Xcode, loaded with `dlopen` | `boot::build` with `H2_EMIT_TARGET=ios` (`crates/apps/h2host/examples/pack.rs --platform ios`) |
| `disc/`, `bootcache/` | the same disc subset and analysis as tvOS (`h2host::layout::DISC_FILES`) | `pack` |
| `platform/ios/H2iOS` | a UIKit host: a view controller whose view is a `CAMetalLayer`, a 30 Hz `CADisplayLink`, the on-screen controls and the keyboard | Xcode |
| `H2iOS/Assets.xcassets` | the app icon: `AppIcon.appiconset`, one 1024 universal image with dark and tinted appearances (iOS 18 and later use them; 17 shows the default) ([art/app-icon](../../art/app-icon/README.md)) | Xcode (actool), named by `ASSETCATALOG_COMPILER_APPICON_NAME` in `project.yml` |
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
layout below. They draw stick clicks (crouch and zoom), a d-pad beside the
left stick, and Menu and Options, which Apple's `GCVirtualController` does
not offer. Touch fire reaches player one as a full right trigger, as
`map_snapshot` and `touch_fire_reaches_player_one_as_a_full_right_trigger`
test.

Layouts follow player one's state (below). On foot:

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
  the rim, so turning back answers at once. 56 points is full deflection
  (80 while driving), with a 12% radial deadzone.
* **Right side: look.** A drag anywhere on the right side turns the camera
  by an angle, the way mouse look feels on the Mac but with no stick in
  between: `crates/platform/input/src/touch_look.rs` turns points into yaw and
  pitch, and the boot's touch hook (`crates/extensions/game/src/touch_glue.rs`) adds
  them to player one's facing in the player-control record once a tick.
  Halo's own stick path (its deadzone, response curve, maximum turn rate)
  is not involved, so slow drags are exact and fast flicks turn far.
  - 0.2 degrees per point at Touch Look Speed 5 (a slow drag across the
    right half of a phone, about 400 points, turns about 80 degrees). The
    speed setting multiplies it (1 to 10, each step 20%).
  - Acceleration: no extra gain below 150 points a second; above it the
    gain rises smoothly to 3x at 1500 points a second (Look Acceleration 5;
    0 turns it off, 10 doubles the extra). No deadzone.
  - Zoomed, the turn is divided by the magnification, so a drag moves the
    scope as far on screen as an unzoomed view.
  - Aim slowdown: while the crosshair is on an aim-assist target (the red
    reticle: the player-control record's target is set with a positive
    cone or pull strength) the turn is 0.55x. Fire turns red too.
  - Every button on the right side looks as well: a finger on Fire looks
    at once, one on any other button once it has moved 8 points (so a tap
    stays a tap), and keeps holding its button.
  - Gyro Aim adds the device's rotation (1:1 at 5) to the same turn.
  - Without the hook (a host with no boot features) touch look falls back
    to round 1's stick path.
* **Buttons**, by Halo 2's Default layout: Fire (right trigger), Grenade
  (left trigger), Jump (A), Melee (B), Reload (X), Switch (Y), Crouch
  (left-stick click), Zoom (right-stick click), Swap Gren (Black), Light
  (White), Back and Start. A finger holds the button it landed on until it
  lifts. Another layout (Southpaw, Boxer, ...) moves the actions, not the
  labels. There is no d-pad: Halo 2 does not use one in play and its menus
  move with the stick (the C API has the bits if one is wanted).
* **Crouch** is a toggle by default (it stays lit while crouched); the
  Touch Crouch setting makes it hold-to-crouch.

### Layouts

Each tick the boot reads player one's unit, seat and held weapon from guest
memory (`touch_glue::context`; offsets and evidence in that file) and
publishes an `input::touch_layout::Context`. `touch_layout::plan` picks
the layout (unit-tested); the app reads it with `h2host_touch_layout` each
frame and animates a change (0.25 s). The log says `touch: layout on foot
-> driver` once per change.

| State | Layout |
|---|---|
| menus, no unit, dead | the full on-foot set, nothing dimmed |
| on foot | the full set; Zoom dimmed with no scope; a scoped weapon (zoom levels at `weap+0x1fe`) moves a large Zoom next to Fire |
| dual-wielding (unit `+0x213`) | a second Fire ("FIRE L", the left trigger) on the left side; Grenade and Zoom gone (Halo 2 throws no grenades and has no zoom then), Swap Gren dimmed |
| driving (seat flag bit 2) | Fire, Boost (left trigger), Exit (hold X), Zoom, Back, Start; no Jump, Crouch, Melee, Grenade or Switch; the move stick is 80 points |
| passenger or gunner | turret: Fire, Exit (hold X; a tap still reloads), Zoom, Back, Start |

```
 driving                                    turret (passenger, gunner)
 [LIGHT]        [Back] [Start]              [Back] [Start]
                                [EXIT]                          [EXIT]
                         [ZOOM]
                                 ( FIRE )         [ZOOM] ( FIRE )
  (wider move stick)
                                   [BOOST]
```

### Touch Auto Fire

Off by default. On, the touch controls pull the trigger while the reticle
is on a target (the same test as aim slowdown), on foot or in a passenger
seat, never while driving: an automatic weapon (trigger behaviour 0) is
held, a semi-automatic one (1, 4, 5) is pulled for one game read and
released until its next shot is due (one over the barrel's time between
shots, `+0x20`), a charging one (the plasma pistol) is tapped so it never
charges, and the sword (no triggers) never lunges by itself. The decision
is `touch_layout::fire_mode` and `AutoFire` (unit-tested). In
multiplayer it is as allowed as the player's own thumb: it only presses the
trigger.

The view only tracks fingers. Each change, and once a frame, it sends an
`H2TouchFrame` (held buttons, the move thumb's offset, the look drag since
the last call, the gyro's rate) through `h2host_touch_update`. Rust
(`crates/platform/input/src/touch.rs`, unit-tested) turns it into a pad (the stick
deadzone, the crouch toggle, auto-fire) and a queued turn
(`touch_look`). The keyboard pad merges it into player one
(`KeyboardMouse::touch_update`), so it shares port 0 with every other
device, and the game shows pad prompts while touch is in use. The app logs
each change of the touch pad (`h2host: touch RT look(+0.25,+0.00)`), at
once for buttons and at most four times a second for sticks.

Settings, on the Controls screen (they replace the Steam Controller's rows
on iOS): **Touch Look Speed** 1 to 10 (5), **Touch Crouch** Toggle or Hold,
**Gyro Aim** 0 (off, the default) to 10: turning the device aims as well
(CoreMotion, only running while it is on), **Look Acceleration** 0 to 10
(5; `H2_TOUCH_LOOK_ACCEL`), **Touch Auto Fire** Off or On (Off;
`H2_TOUCH_AUTO_FIRE`) and **Aim Assist**, which
is Auto by default, which is Strong while the touch controls show
([docs/game/aim-assist.md](../game/aim-assist.md)).

The overlay goes away when a controller connects and comes back when the last
one disconnects. A controller counts if it has an extended gamepad profile; a
Siri Remote paired to an iPad doesn't. It also hides while the system
keyboard is up. Hiding lets go of everything it held. The rule is
`h2host_pad_counts` and `h2host_overlay_wanted`
(`crates/apps/h2host/src/overlay.rs`, unit-tested).

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
`crates/apps/h2host/src/text.rs` (unit-tested).

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
`platform/ios`); both are committed. Regenerate it after adding or removing
files under `H2iOS` (xcodegen needs `build/disc` and `build/bootcache` to
exist, even empty).

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
  Choosing another level or map fails to load it. `just ios maps=all`
  bundles the whole disc and the map pack instead (7 GB or more, and in
  Documents, so it counts towards backups; [building.md](building.md)).
- Speed is unmeasured. Older devices (A12 and earlier) will be slow.
- The app may need the increased memory limit entitlement on devices with
  little RAM; it does not ask for it yet.
- Xbox Live is on by default, against `https://live.forbes.red`;
  `live.toml` in the data root (`enabled = false`, or `server = "URL"`)
  or `H2_LIVE_SERVER` in the scheme changes that ([live.md](../live/live.md)
  "Playing on Live"). Signing in needs an account in the data root's
  `live/accounts.json` until the in-game account setup has a screen; with
  one, saves are synced ([live-cloud-saves.md](../live/live-cloud-saves.md)).
- No controller speaker, no `--level`.
- Gyro aiming's axes assume the device held in landscape like a camera;
  it is untested on hardware.
