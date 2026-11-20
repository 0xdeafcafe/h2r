# Input

Status: Built. A reference for `crates/platform/input`.
Code: `crates/platform/input/src/lib.rs` (`InputManager`), `crates/platform/input/src/backend/mod.rs` (`HostBackend`). Tests: `crates/platform/input/tests/mouse_look.rs`, `crates/platform/xapi/tests/`.
Switch: see the sections below; each is in [switches.md](../runtime/switches.md).

`crates/platform/input` replaces Xbox XInput (XAPI) at the API level. The recompiled
game calls `XInitDevices`, `XGetDevices`, `XGetDeviceChanges`, `XInputOpen`,
`XInputPoll`, `XInputGetState`, `XInputSetState`; the guest adapters in
[`crates/platform/xapi`](../runtime/xapi.md) marshal those calls to `InputManager`. No USB stack
is emulated.

```
host backends (GameController pads, gilrs pads, keyboard+mouse, timeline)
        | HostPad { HostPadState: floats }
        v
host::to_gamepad  (deadzones, Xbox button mapping)
        |
        v
InputManager: 4 ports, packet numbers, polling, rumble
        |
        v
XINPUT_STATE bytes (State::to_bytes) written into guest memory
```

## The gamepad as games see it

`gamepad.rs`. `XINPUT_GAMEPAD` is packed (18 bytes): `wButtons` (u16),
`bAnalogButtons[8]` (u8), `sThumbLX/LY/RX/RY` (i16). `XINPUT_STATE` is
`dwPacketNumber` (u32) plus that (22 bytes). `Gamepad::to_bytes` and
`State::to_bytes` emit the little-endian guest layout.

- Digital `wButtons` bits: d-pad up/down/left/right = 0x01/0x02/0x04/0x08,
  Start 0x10, Back 0x20, left thumb click 0x40, right thumb click 0x80.
- `bAnalogButtons` indices: A 0, B 1, X 2, Y 3, Black 4, White 5, left
  trigger 6, right trigger 7. All are 0..255 pressure values. On a real pad
  the face buttons and Black/White are pressure sensitive too.
- Sticks: signed 16-bit, +Y up. The pad reports raw values and the game does
  its own deadzone.
- `XINPUT_CAPABILITIES`: `SubType` (`XINPUT_DEVSUBTYPE_GC_GAMEPAD` = 1), `In`
  gamepad mask, `Out` rumble mask. `XINPUT_FEEDBACK`: header with `dwStatus`
  plus `XINPUT_RUMBLE` (two u16 motor speeds). `XINPUT_POLLING_PARAMETERS`:
  `fAutoPoll`, `fInterruptOut`, intervals.

Sources, pinned:

- Struct layouts, device type constants, polling parameters, Cxbx-Reloaded
  `Xapi.h`:
  https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.h
- Packet number and error-code behaviour (`dwPacketNumber` incremented when
  new input is read and copied to the state; `ERROR_DEVICE_NOT_CONNECTED`,
  `ERROR_IO_PENDING` for `XInputSetState`, `XGetDeviceChanges` handling of
  remove-then-reinsert), Cxbx-Reloaded `Xapi.cpp`:
  https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.cpp
- Button bit positions and analog indices, xemu's XID report (`BUTTON_MASK(b)
  = 1 << (b - GAMEPAD_DPAD_UP)`, which fixes d-pad..thumb at bits 0..7):
  https://github.com/xemu-project/xemu/blob/4bdad199102b22ed0cfa998e9af6b35d76238b3e/hw/xbox/xid.h

Not verified against an XDK header (none available here): the Win32 constants
(0, 997, 1167 are the standard winerror.h values) and the default polling
intervals (8). The `Out` capability mask follows each backend's
`supports_rumble`: both motors are all-ones only when supported; otherwise
both are zero. The standard input mask alone does not advertise rumble. The XAPI boundary is `0x00409994`
(capabilities) / `0x00409bdf` (feedback). Memory units and expansion slots
are out of scope.

## Device model

`device.rs`.

- Four ports. A host pad takes the lowest free port when it appears and keeps
  it until it disappears. Backends added first win free ports first; extra
  pads beyond four are ignored.
- Insertion bumps the port's generation and starts a fresh device at packet
  number 0. Handles from `open` carry the generation, so after unplug/replug
  an old handle returns `ERROR_DEVICE_NOT_CONNECTED` (`InputError::win32`).
- `device_changes()` drains raw host `(insertions, removals)` edges. XAPI
  applies the retail Cur/Prev/Chg formulas at `0x004098c5`: insert/remove
  with Prev=Cur=0 reports neither; remove/reinsert with Prev=Cur=1 reports both.
  `connected_mask()` is `XGetDevices`.
- Packet numbers increment only when the published `XINPUT_GAMEPAD` differs
  from the previous one. A pad idling at neutral stays at its packet number.
- Polling: default `fAutoPoll = 1`, state is published on every `sync()`. With
  auto-poll off, state is published only by `poll(handle)`, so `get_state`
  returns the last polled snapshot.
- Rumble: on a capable backend, each `set_state` with a
  `FeedbackRequest { addr, h_event, rumble }` is its own request, identified
  by the guest address of its `XINPUT_FEEDBACK`, and returns `ERROR_IO_PENDING`. `sync()` completes every
  pending request independently with `ERROR_SUCCESS` and the runtime drains
  them with `take_completions()`, writing `dwStatus` at `addr` and signalling
  `h_event`. A backend without rumble returns `InputError::NotSupported`
  (Win32 50) and does not queue feedback. Requests apply in order; the last
  rumble value wins on the backend. If the pad is unplugged first, its pending
  requests complete with `ERROR_DEVICE_NOT_CONNECTED`.
- Raw `device_changes()` edges accumulate in `sync()`, so a pad inserted
  and removed between drains appears in both raw masks. The XAPI adapter
  normalizes these against the last connection mask seen by the guest.
- `InputManager::new` validates the deadzones (finite, in `[0, 1)`).
- Guest reads: `get_state` is `XInputGetState` and marks the device read; the
  next `sync()` calls the backend's `HostBackend::state_read` for it before
  polling, at most once per sync. That is how the keyboard pad spends mouse
  motion only once the game has seen it. Host bookkeeping (XAPI refreshing
  each handle's last state on every sync, `XInputOpen`) uses `peek_state`,
  which does not count as a read.

Guest layouts (`gamepad.rs`, little-endian, packed, 32-bit guest):

- `XINPUT_FEEDBACK` is 70 bytes: header 66 bytes (`dwStatus` @0, `hEvent`
  @4, `Unknown1[4]` @8, `IoCompletedEvent` @12, `Unknown2[48]` @16,
  `bReportId` @64, `bLength` @65) then `XINPUT_RUMBLE` @66. Unknown header
  fields are written as zero.
- `XINPUT_CAPABILITIES` is 25 bytes: `SubType` @0, `Reserved` u16 @1, `In`
  gamepad @3, `Out` rumble @21.
- Layout tests check the exact offsets. Both come from the pinned Cxbx
  `Xapi.h`; they were not checked against an XDK header.

The caller owns the frame cadence: call `sync()` once per host frame (or per
emulated USB poll interval) before running the guest.

## Host backends

`backend/`. The trait is `HostBackend { name(), poll() -> Vec<HostPad>, state_read(), set_rumble() }`;
`state_read` and `set_rumble` default to doing nothing.
Backends are platform-separated modules; a tvOS or Xbox build swaps them.

### DualSense raw HID (`pad_hid`, feature `hid`, default)

Not a `HostBackend`: a thread of its own that opens a DualSense over IOKit
without seizing it, so the backends below keep reading its buttons. It reads
only the mute button and writes only the mute LED and the speaker path
(`H2_PAD_MUTE`, `H2_PAD_SPEAKER`). See
[playstation-audio.md](../audio/playstation-audio.md).

### Steam Controllers (`steam_pad`, feature `hid`, default)

A thread of its own that reads Valve's controllers over raw HID, turns their
lizard mode off and drives their trackpad haptics; its `SteamBackend` sits
beside the platform's pads (`backend/multi.rs`). The right trackpad aims
through the mouse-look path. See [steam-controller.md](steam-controller.md).

### Physical pads: Apple GameController (`backend/game_controller/`, feature `game-controller`, default)

The macOS pad backend. `GameControllerPads::new()` reads
`GCController.controllers()` through `objc2-game-controller` (0.3.2, the
maintained objc2 bindings) and the extended gamepad profile, so Xbox,
DualShock/DualSense, Switch Pro and MFi pads all arrive in one layout.

Split for testability: `game_controller/mod.rs` is portable and holds the
`ControllerSource` trait (a list of `RawPad` plus a rumble sink), the
`Snapshot` of floats, `map_snapshot` and `GameControllerPads<S>` (the
`HostBackend`). `game_controller/apple.rs` (macOS only) is `AppleSource`, the
only code that touches Objective-C. Tests drive the mapping and the
`InputManager` through a fake source: no hardware needed.

- **Ports and hot-plug.** The backend reports pads in connection order, each
  with a never-reused key (so a replug gets a new `HostPadId`). The manager
  assigns the first four to ports 0 to 3, a fifth waits for a free port, and
  `device_changes()` returns the insertion and removal masks, including the
  unplug-and-replug-between-syncs case. Tested.
- **Axes.** Sticks are -1..1 with +Y up; framework-calibrated values are
  scaled independently to -32768..32767 without an extra radial deadzone.
  Triggers are 0..1 pressure, rescaled to 0..=255.
- **The eight analog buttons.** `bAnalogButtons` takes real pressure from
  `GCControllerButtonInput.value`: A, B, X, Y, Black (right shoulder), White
  (left shoulder) via the new `HostPadState::analog_buttons`, and the two
  triggers. The host button bit is set at pressure >= 0.5, while the Xbox
  report retains pressure below that threshold. Face buttons keep their
  physical position (south = A).
- **Menu / Options** are Start / Back; stick clicks are the thumb buttons.
- **Touchpad** (DualSense through `GCDualSenseGamepad`, DualShock 4 through
  `GCDualShockGamepad`): the click and up to two fingers (-1..1, +Y up, (0, 0)
  for no finger) go to `HostPadState::touchpad`. The guest pad never gets
  them. `input::touchpad::Gestures` turns them into swipes, clicks and
  two-finger taps for the debug menu ([docs/runtime/debug-menu.md](../runtime/debug-menu.md)),
  which watches every pad through `InputManager::set_gate`.
- **Rumble.** `GCDeviceHaptics.createEngineWithLocality:` gives one
  CoreHaptics engine per handle: left handle for the heavy motor, right for
  the light one. Each runs a looping 30 s continuous event whose intensity is
  set by a dynamic parameter (`HapticIntensityControl`), 0 stops it. The
  generated objc2 binding for `createEngineWithLocality:` is gated to
  iOS/tvOS although macOS has the API, so `apple.rs` calls it with `msg_send!`.
  `supports_rumble` is true when the controller exposes haptics; pads without
  haptics (many third-party ones) report no motors, as with gilrs.
- **Background input.** `shouldMonitorBackgroundEvents` is set so a windowless
  host keeps receiving input.

Not verified: no physical controller was available, so `AppleSource` is
compile-checked and the mapping is unit tested but live input, haptics output
and connect notifications have not been checked on hardware.
Connect/disconnect discovery requires the main run loop; windowed app hosts
already run it. Headless physical-input hosts must call
`game_controller::service_main_loop` regularly on the main thread. Scripts
have no run-loop dependency. Controllers without an extended profile are
skipped, except the Apple TV remote (a `GCMicroGamepad`), which
`game_controller/remote.rs` (`RemoteNav`) turns into a menu pad: a click near
the clickpad's edge is a d-pad direction, a click in the middle is A, a swipe
steps the d-pad, Back is B and Play/Pause is Start (docs/runtime/tvos.md).
`apple.rs` and the framework dependencies build on every Apple platform
(`target_vendor = "apple"`), so iOS and tvOS use this backend too.

Do not attach both this backend and gilrs on macOS: they see the same pads and
each would take a port. The `pads` example picks GameController on macOS and
gilrs elsewhere. Disable the `game-controller` feature to keep gilrs only.

### Physical pads: gilrs (`backend/gilrs_pads.rs`, feature `gilrs`, default)

Evaluation of gilrs 0.11.2 on macOS, from its README support table and by
building it here: input and hotplug are supported (IOKit HID with the SDL
mapping database, so Xbox, PlayStation and most generic pads arrive in the
standard layout), force feedback is not. It has not been tested with a physical pad, only
that it initialises and enumerates zero pads cleanly.

gilrs reuses a gamepad id on replug, so the backend consumes connect and
disconnect events and folds a per-pad generation into the `HostPadId`; the
manager then sees a removal plus an insertion and old handles go stale.

gilrs remains the backend for non-Apple desktop hosts. Its rumble is
unavailable on macOS: requests return 50 and motor capability masks are zero
(`supports_rumble` is false). On Linux and Windows it drives gilrs's effect
API, left motor = strong, right = weak. On macOS the app selects
GameController, which carries rumble and pressure buttons. tvOS has no
gilrs support (not tested).

### Keyboard and mouse (`backend/keyboard.rs`)

A virtual pad, at the front of the port order if added first. `KeyboardMouse`
is a cloneable handle: clone it, box one clone into
`InputManager::add_backend`, and keep the other in the platform layer. There
is no window dependency: the platform layer calls `key_down("KeyW")`,
`key_up`, `mouse_button`, `mouse_motion(dx, dy)` and `release_all()` on focus
loss. Key names are W3C `KeyboardEvent.code` style, case-insensitive; the
`Key`/`Digit` prefixes are optional.

`InputManager::with_keyboard_pad(bindings)` builds the default host input. The
keyboard pad is the first backend, so it is on port 0 and connected from the
first `sync`, and the call returns the handle to feed. That pad accepts rumble
like a standard controller: it advertises both motors, its requests complete
with status 0, and `KeyboardMouse::rumble()` holds the newest speeds. A plain
`KeyboardMouse::new` has no motors, so requests return 50.
`set_rumble_supported(true)` changes that; call it before the pad's first
`sync`, because capabilities are fixed at insertion.

Bindings are a TOML table (`bindings.rs`, `Bindings::from_toml`), layered on
the defaults, one list of inputs per control:

```toml
[convert]
stick_deadzone = 0.08
trigger_deadzone = 0.03

[mouse]
look = true          # mouse motion drives the right stick
sensitivity = 0.04   # stick deflection per mouse count per 1/60 s
invert_y = false

[bind]
a = ["Space"]
right_trigger = ["MouseLeft"]
left_stick_up = ["W"]
b = []               # empty list unbinds the control
```

Config values are validated: deadzones must be finite and in `[0, 1)`,
`mouse.sensitivity` finite and >= 0, and unknown keys in `[convert]` and
`[mouse]` are errors.

Controls: `a b x y black white left_trigger right_trigger dpad_up dpad_down
dpad_left dpad_right start back left_thumb right_thumb` and
`left_stick_{up,down,left,right}`, `right_stick_{up,down,left,right}`.
Unknown controls, unknown input names and unknown top-level keys are errors.
Defaults: WASD move, Space A, V or middle mouse B, E X, Q Y, F Black, R
White, left mouse right trigger, G or mouse-back left trigger, Ctrl or C
left-thumb (crouch), right mouse right-thumb (zoom), Escape Start, Tab Back,
arrow keys d-pad.

Behaviour: keys are digital (buttons 255, triggers 1.0, stick keys full
deflection, opposing keys cancel, diagonals normalised to the unit circle).
Mouse look: motion stays on the right stick until the game has read a
state carrying it (`state_read`), however many syncs pass first. The
deflection is the mouse's speed over the game's read interval:
`delta * sensitivity * (1/60 s) / interval`, with the interval counted as
at least 1/60 s, then scaled back onto the unit circle if it is longer than
1 (direction is kept). At a 60 Hz read rate that is
`delta * sensitivity`: deflection per mouse count moved per 1/60 s. Halo
treats the stick as a turn rate, so turning per mouse count does not depend
on the game's frame rate or on how often the host syncs. Motion the game leaves unread for more than
a second (`STALE_MOTION`) is dropped rather than delivered late as a flick.

Motion is not spent by `sync`: `h2 play` syncs after every guest slice,
far more often than the game polls, and a per-sync spend would leave almost
every `XInputGetState` a centred right stick. `crates/platform/input/tests/mouse_look.rs` and `crates/platform/xapi/tests/mouse_look.rs`
cover this at slice-rate syncs and game-rate reads.

The virtual pad applies no deadzone, but Halo applies its own after reading
XInput. Very slow mouse movement can fall inside it; raise `sensitivity` if
small movements do not turn. Mouse look still feels different from a real
aim-by-position scheme. Gentler options if it feels bad: smoothing across
frames, or an acceleration curve. Neither is implemented. There is also no "walk" modifier for half stick
deflection yet.

### On-screen touch controls (`touch.rs`, iOS)

The iOS app's own touch overlay reports held buttons, the move thumb's
offset and the look drag (`TouchFrame`); `TouchPad` turns them into a pad:
a radial deadzone on the move stick, a crouch toggle on the left-stick
click and Touch Auto Fire (`touch_layout`). Look is direct when the boot's
touch hook is installed (always in the game): a drag becomes yaw and pitch
in radians (`touch_look`), added to player one's facing once a tick, with
no stick in between. Without the hook it falls back to `look::LookMotion`
(held until the guest reads it) with acceleration, sensitivity and a
response curve, plus optional gyro.
`KeyboardMouse::touch_update` merges it into the keyboard pad, so it joins
player one on port 0 (buttons add, triggers and the move stick take the
stronger, look adds to the mouse's). Using it marks the last-used device as
a pad, for pad prompts. See docs/runtime/ios.md, "Controls".

## Button layouts

Halo 2's retail layouts (Default, Southpaw, Boxer, Green Thumb), the added
ones (Bumper Jumper, Recon, Walkie Talkie), how the game implements them and
what an in-game menu entry needs are in [controller-layouts.md](controller-layouts.md).
`input::layouts` holds the symbolic tables. The host does not remap for
layouts: menu confirm and cancel would move with them.

## Analog mapping and deadzones

`host.rs`.

- Face buttons A/B/X/Y and Black/White are 0..255 on Xbox. GameController
  carries each element's reported pressure; digital-only hosts report 255
  for a press and 0 for release. Hardware that exposes only on/off buttons
  still produces only those endpoints.
- Layout: south/east/west/north become A/B/X/Y. Right bumper is Black, left
  bumper is White (one place to change: `to_gamepad`).
- Triggers: host 0..1, subtract the trigger deadzone (default 0.03), rescale
  to 0..255. Digital-only triggers give 0 or 255.
- Sticks span -32768..32767, with +Y up. Raw physical backends use the
  configured radial drift deadzone (default 0.08) and rescale the live range.
  Set it to 0 for raw independent axes. GameController is already calibrated,
  so it skips that filter and preserves both axes, including full diagonals.
  Scripted Xbox values also pass through exactly. Keyboard/mouse normalises
  its own synthetic diagonals before conversion. Halo applies its gameplay
  deadzone after reading XInput; the host must not apply that deadzone twice.
- Non-finite host axis values (NaN, infinity) become 0 for that axis only;
  finite values are clamped to range before the deadzone test.
- D-pad: opposing directions cancel (a real d-pad can't press both).

Per-controller calibration, curves and a settings UI are not done.

## Testing

`cargo test -p input` covers struct byte layouts, button masks, stick and
trigger conversion, d-pad rules, the device model (insertion, removal,
change masks, stale handles, port limit, packet numbering, manual poll,
rumble) and the binding table including TOML errors. `tests/mouse_look.rs`
syncs at host rate and reads at game rate, checking that every read after
motion sees it, that deflection is proportional to mouse speed at 60, 30 and
5 reads per second, and that a read spends the motion. `cargo run -p input
--example pads` prints live state for every connected pad, one line per port,
whenever it changes (`-- --rumble` attempts one rumble on the first pad;
gilrs on macOS rejects it as unsupported).

## Campaign host configuration

`h2 play` takes `--bindings FILE` (a table like the one above, replacing the
play defaults, which are the Halo defaults plus Enter for A and Backspace for
B) and `--input-timeline FILE` (a timeline, below, played on guest time
instead of live input; `--no-gamepads` and a timeline both keep Steam
Controllers out).

[`input-default.toml`](input-default.toml) is a complete, tested binding
config. Load it with `Bindings::from_toml(&std::fs::read_to_string(path)?)`
and assign `AppConfig::bindings`; the shell demo accepts `--input-config PATH`.
Omitted bindings retain defaults, and an empty list unbinds a control.
Physical pad mapping follows the GameController layout above; `[bind]`
configures keyboard/mouse controls.

The app's `gamepads` feature selects GameController on macOS and gilrs on
other desktop hosts. With `AppConfig::keyboard = true`, `KeyboardFallback`
combines the first physical pad and keyboard/mouse into one persistent pad
on port 0. Pressure uses the greater value, digital buttons combine (opposed
d-pad directions cancel), and each stick uses the source with the greater
whole-stick deflection (keyboard wins ties). Remaining pads take ports 1–3.
This prevents the keyboard pad from occupying player one while a real pad
can only control player two. The same primary pad is retained until unplugged.

The combined pad also records which half the player used last
(`input::active::ActiveDevice`, from `KeyboardMouse::active_device`): the
keyboard and mouse, an Xbox-style pad, a PlayStation pad, the Apple TV remote or
a Steam Controller (by the pad's name).
Fresh presses and motion select the device; held buttons, key autorepeat and
small pad jitter do not take the selection back. Slow mouse look counts too. The
runner uses it to pick the button icons ([button-icons.md](button-icons.md)).

The combined virtual pad remains connected when hardware disappears and
accepts/completes feedback even without physical motors. Rumble is forwarded
to the selected physical pad and restored on attach. To expose actual
physical insertion/removal, disable `keyboard`; physical devices then use
native identities, stale-handle rules and capability masks. `InputManager`
without this wrapper always exposes those native semantics.

## Deterministic timelines for boot and headless runners

`backend::scripted::{Timeline, ScriptedInput}` supplies one virtual controller
and accepts rumble with the same asynchronous completion path as hardware.
It needs no window, GPU, run loop or controller. The tested
[`input-menu-example.toml`](input-menu-example.toml) shows A, release, down,
release, A. Its timings are illustrative, not a verified retail Campaign route.

Each `[[frame]]` is a **complete state** at absolute `at_ticks` (100 ns guest
ticks; 10,000,000 = one second). Omitted buttons and axes are released; the
state is held until the next timestamp, including after the final frame.
Before the first frame the pad is connected and neutral. Include a final
neutral frame to release the last press. Unknown fields/buttons, out-of-range
integers, empty timelines and non-increasing timestamps are errors.

`buttons` accepts the eight analog names (`a`, `b`, `x`, `y`, `black`,
`white`, `left_trigger`, `right_trigger`) and eight digital names (`dpad_up`,
`dpad_down`, `dpad_left`, `dpad_right`, `start`, `back`, `left_thumb`,
`right_thumb`).
`analog = [1, 32, 64, 96, 128, 160, 192, 255]` optionally replaces all eight
pressures, in A/B/X/Y/Black/White/LT/RT order. `left_stick = [-32768, 32767]`
and `right_stick = [32767, -32768]` are exact Xbox axis values, including
small values that Halo's own deadzone evaluates. `connected = false` removes
the pad; a later connected frame creates a fresh identity. A disconnect and
reconnect skipped by a coarse clock advance still invalidates old handles.

For a runner without the app shell:

```rust,ignore
let timeline = input::backend::scripted::Timeline::from_toml(&text)?;
let script = timeline.playback();
let mut input = input::InputManager::new(Default::default())?;
input.add_backend(Box::new(script.clone()));
// Supply input to Xapi::new, register its verified entries, then per slice:
script.advance_to(relative_guest_ticks)?;
xapi::service_input(&runtime, &xapi, &mut kernel_events)?;
// Run the next bounded scheduler slice.
```

For the app shell, set `AppConfig::scripted_input = Some(script.clone())`
before `app::run` or `GuestHost::headless`. It replaces all live backends,
including keyboard/mouse, so headless results cannot depend on human input.
Take the manager with `host.take_input()`, build XAPI as usual, and retain the
other script clone in the guest closure. Advance it to guest time before
`app::pump_input` or `xapi::service_input`. Use ticks relative to a fixed run
or menu-ready epoch; wall time is deliberately not consulted. `host.tick`
paces the guest but does not advance the script itself.

Playback polls never consume a step. Repeated init, sync, GetState or Poll
calls at one timestamp see the same input. `advance_to` rejects a backwards
clock; use `timeline.playback()` for a fresh run. `next_transition_ticks()`
lets a runner limit its slice to the next transition. Pump at least once
inside every desired press interval: jumping straight past a press and its
release cannot make the guest observe the press. Longer holds or starting
the timeline after a known menu-ready condition handle slow boot/loading.
Do not rewind input on checkpoint/revert; use monotonic runner time or
start a new playback epoch explicitly.

The shell demo accepts `--input-script docs/input/input-menu-example.toml` and
advances it from the demo's logical clock. This validates wiring; entering
retail Campaign is the boot runner's job (`H2_BOOT_INPUT`, docs/runtime/BOOT.md),
which chooses menu-ready timing and pumps while running the generated game. Tests establish XAPI values,
not completion of Armory's generated pitch/movement gates.

`tests/input_semantics.rs` in xapi covers every control's press/hold/release,
all 256 pressures, exact stick extremes/small values, packet changes,
manual polling, capabilities, rumble completion and disconnect/reconnect.
The registered-runtime test also crosses the verified capabilities/state/
feedback addresses. Input unit tests cover packet wrapping and timeline
validation; app tests load both checked-in TOML examples without a GPU.


## Host text in the game's virtual keyboard

While the game's virtual keyboard is focused, host typing edits it directly: see [host-keyboard.md](host-keyboard.md).
