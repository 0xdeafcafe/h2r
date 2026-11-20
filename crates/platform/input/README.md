# input

The Xbox controller model for h2recomp. Bottom to top: the gamepad data the
game sees (`XINPUT_GAMEPAD`, `XINPUT_STATE`, rumble), the conversion from
normalised host reports into Xbox units with deadzones, the four-port
device model, and host backends (gilrs, Apple GameController, raw HID for
the DualSense, Steam controllers, touch and trackpad).

## Where it sits

Domain `platform`; it has no workspace dependencies. `xapi` serves it to the
guest, and `boot`, `game`, `app`, `h2host`, `agent`, `lockstep`,
`debug-menu`, `bots` and `cli` use it.

## Layout

- `src/gamepad.rs`, `src/host.rs`, `src/device.rs`: the gamepad types, the
  host report and the `InputManager`.
- `src/backend/`: backends, including a scripted one for tests.
- `src/bindings.rs`, `src/layouts.rs`, `src/look.rs`: bindings and look
  shaping.
- `src/dualsense.rs`, `src/pad_hid.rs`, `src/steam.rs`, `src/steam_pad.rs`:
  DualSense and Steam controller support.
- `src/touch*.rs`, `src/trackpad.rs`, `src/touchpad.rs`, `src/haptics.rs`.

## Features

- `gilrs` (default): physical pads through gilrs.
- `game-controller` (default): Apple GameController, with hot-plug,
  pressure buttons and rumble.
- `hid` (default): the DualSense over raw HID.

## Tests

`cargo nextest run -p input`.

## Docs

- [docs/input/steam-controller.md](../../../docs/input/steam-controller.md)
- [docs/runtime/debug-menu.md](../../../docs/runtime/debug-menu.md)
