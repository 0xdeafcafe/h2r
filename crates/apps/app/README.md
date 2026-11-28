# app

The host shell: the window, presentation of the guest's frames, live input, the audio device and the guest thread's lifecycle. `h2 play`, the Apple apps and the Xbox app all open the game in this shell.

## Where it sits

`apps`. Uses `gfx`, `input`, `audio`, `xapi`, `timeline`, `debug-menu` and `lockstep`. Used by `cli`, `h2host` and `uwp-host`.

## Layout

- `src/shell.rs`, `src/lifecycle.rs`: the shell and the guest thread's start, pause and stop.
- `src/worker.rs`, `src/mailbox.rs`, `src/frame.rs`, `src/pacing.rs`, `src/fps.rs`: the hand-off of frames from the guest to the window and their pacing.
- `src/input_pump.rs`, `src/keymap.rs`: keyboard, mouse and pad input into `xapi`.
- `src/audio_out.rs`: the output device.
- `src/overlay.rs`, `src/menu_overlay.rs`, `src/theater_overlay.rs`, `src/watermark.rs`, `src/tile.rs`, `src/scale.rs`: what is drawn over the game.
- `src/embedded.rs`: the shell as a library for the Apple hosts (`h2host`).
- `src/bin/`: the binary entry points.

## Features

- `default` = `gamepads`.
- `gamepads`: GameController on macOS, gilrs elsewhere.
- `game-controller`: GameController alone, as tvOS builds it.

## Tests

`cargo nextest run -p app`. No test needs a disc.

## Docs

- [app.md](../../../docs/runtime/app.md)
- [graphics.md](../../../docs/graphics/graphics.md)
- [host-keyboard.md](../../../docs/input/host-keyboard.md)
- [high-framerate.md](../../../docs/game/high-framerate.md)
