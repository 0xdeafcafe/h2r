# Input

`crates/input` replaces Xbox XInput (XAPI) at the API level. The recompiled
game calls `XInitDevices`, `XGetDevices`, `XGetDeviceChanges`, `XInputOpen`,
`XInputPoll`, `XInputGetState`, `XInputSetState`; the guest adapters in
[`crates/xapi`](xapi.md) marshal those calls to `InputManager`. No USB stack
is emulated.

```
host backends (gilrs pads, keyboard+mouse)
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

Guest layouts (`gamepad.rs`, little-endian, packed, 32-bit guest):

- `XINPUT_FEEDBACK` is 70 bytes: header 66 bytes (`dwStatus` @0, `hEvent`
  @4, `Unknown1[4]` @8, `IoCompletedEvent` @12, `Unknown2[48]` @16,
  `bReportId` @64, `bLength` @65) then `XINPUT_RUMBLE` @66. Unknown header
  fields are written as zero.
- `XINPUT_CAPABILITIES` is 25 bytes: `SubType` @0, `Reserved` u16 @1, `In`
  gamepad @3, `Out` rumble @21.
- Layout tests check the exact offsets. Both come from the pinned Cxbx
  `Xapi.h`; I did not check them against an XDK header.

The caller owns the frame cadence: call `sync()` once per host frame (or per
emulated USB poll interval) before running the guest.

## Host backends

`backend/`. The trait is `HostBackend { poll() -> Vec<HostPad>, set_rumble() }`.
Backends are platform-separated modules; a tvOS or Xbox build swaps them.

### Physical pads: gilrs (`backend/gilrs_pads.rs`, feature `gilrs`, default)

Evaluation of gilrs 0.11.2 on macOS, from its README support table and by
building it here: input and hotplug are supported (IOKit HID with the SDL
mapping database, so Xbox, PlayStation and most generic pads arrive in the
standard layout), force feedback is not. I could not test with a physical
pad in this session, only that it initialises and enumerates zero pads
cleanly.

gilrs reuses a gamepad id on replug, so the backend consumes connect and
disconnect events and folds a per-pad generation into the `HostPadId`; the
manager then sees a removal plus an insertion and old handles go stale.

Decision: use gilrs now. It is pure Rust with no unsafe in our code and
covers the Mac target. Rumble is unavailable on macOS: requests return 50
and motor capability masks are zero (`supports_rumble` is false, so the
device model never calls the backend; on Linux and Windows it drives gilrs's effect API, left motor = strong, right = weak). If
macOS rumble matters, add a GameController framework backend
(`GCController` plus `CHHapticEngine`) behind the same trait. That needs
Objective-C bindings (likely `objc2`, with some unsafe) so it is a separate
piece of work. tvOS will probably need it anyway (gilrs's README lists no tvOS support; not
tested).

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
sensitivity = 0.04   # stick deflection per mouse count, per sync
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
Mouse motion accumulated since the last `sync` becomes `delta * sensitivity`
on the right stick, clamped to 1, then resets. The stick is a turn rate in
Halo, so mouse look is frame-rate dependent and will feel different from a
real aim-by-position scheme. Tune `sensitivity` for the sync rate. Gentler
options if it feels bad: smoothing across frames, or an acceleration curve.
Neither is implemented. There is also no "walk" modifier for half stick
deflection yet.

## Analog mapping and deadzones

`host.rs`.

- Face buttons A/B/X/Y and Black/White are 0..255 on Xbox. No host API we use
  exposes face-button pressure, so a digital press is 255 and release 0. The
  trade-off is that games which read the pressure (some use it for weapon
  fire thresholds) get a binary signal. Backends that learn about pressure
  could extend `HostPadState`.
- Layout: south/east/west/north become A/B/X/Y. Right bumper is Black, left
  bumper is White (one place to change: `to_gamepad`).
- Triggers: host 0..1, subtract the trigger deadzone (default 0.03), rescale
  to 0..255. Digital-only triggers give 0 or 255.
- Sticks: radial deadzone (default 0.08) with rescaling so output starts at
  zero at the edge of the deadzone and reaches full scale at 1.0, then +-32767
  (symmetric, -32768 is not produced). Radial rather than per-axis so
  diagonals keep their angle. The Xbox pad itself has no hardware deadzone,
  and Halo applies its own, so this only masks host-side drift. Keep it low or
  Halo's own deadzone stacks on top. Virtual (keyboard/mouse) pads skip it.
- Non-finite host axis values (NaN, infinity) become 0 for that axis only;
  finite values are clamped to range before the deadzone test.
- D-pad: opposing directions cancel (a real d-pad can't press both).

Per-controller calibration, curves and a settings UI are not done.

## Testing

`cargo test -p input` covers struct byte layouts, button masks, stick and
trigger conversion, d-pad rules, the device model (insertion, removal,
change masks, stale handles, port limit, packet numbering, manual poll,
rumble) and the binding table including TOML errors. `cargo run -p input
--example pads` prints live state for every connected pad, one line per port,
whenever it changes (`-- --rumble` attempts one rumble on the first pad;
macOS rejects it as unsupported).
