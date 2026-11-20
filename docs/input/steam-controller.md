# Steam Controller

Status: Built. Unit tested on synthetic reports and used on a controller
by the owner; "Not verified" lists what the tests and that use do not show.
Code: `crates/platform/input/src/steam.rs`, `trackpad.rs`, `haptics.rs`, `steam_pad.rs`; the table below has the rest. Tests: in those files.
Switch: `H2_STEAM_PAD`, `H2_STEAM_SPEAKER`, `H2_STEAM_SENSITIVITY`, `H2_STEAM_LEFT_PAD`, `H2_PAD_SPEAKER_DEVICE`, see [switches.md](../runtime/switches.md).

Both Valve controllers are read over raw HID, wired, through their wireless
receivers and over Bluetooth LE. The game sees an Xbox pad. The right
trackpad aims like a mouse, the left one is the d-pad or a second stick, and
the trackpad actuators play rumble and, when there is no DualSense speaker,
the controller-speaker sounds as haptics.

The code:

| Part | Where |
|---|---|
| Report parsing, lizard mode, haptic reports | `crates/platform/input/src/steam.rs` |
| Trackpad aim, left-pad modes, mapping to the Xbox pad | `crates/platform/input/src/trackpad.rs` |
| Sound and rumble to pulse trains | `crates/platform/input/src/haptics.rs` |
| The HID thread and its `HostBackend` | `crates/platform/input/src/steam_pad.rs` |
| macOS IOKit host | `crates/platform/input/src/steam_pad_iokit.rs` |
| Mouse-look motion shared with the keyboard backend | `crates/platform/input/src/look.rs` |
| Platform pads and Steam pads as one backend | `crates/platform/input/src/backend/multi.rs` |
| The speaker sink | `crates/extensions/game/src/steam_speaker.rs` |
| Settings | `crates/extensions/game/src/setting_defs.rs` (`steam_pad.*`) |

## Sources

Everything at the byte level follows two public drivers. Only what they
document is implemented; nothing came from captures.

- SDL, commit `6b564dd838b9fbb41b6cc80321b8949f43be82f6`:
  `src/joystick/hidapi/SDL_hidapi_steam.c`,
  `src/joystick/hidapi/SDL_hidapi_steam_triton.c`,
  `src/joystick/hidapi/steam/controller_constants.h`,
  `src/joystick/hidapi/steam/controller_structs.h`,
  `src/joystick/usb_ids.h` and `src/joystick/controller_list.h`.
- Linux, commit `4dd10fbbeb9c33709edc89916e19c03c780a0751`:
  `drivers/hid/hid-steam.c`.

## Devices

Vendor `0x28de`. Every interface of these products is matched, and all but
the controller's are skipped (`steam::is_controller_interface`): the
keyboard and mouse interfaces lizard mode uses (generic desktop usages 6
and 2), and the vendor interface with usage page `0xff00`, usage 2, which
SDL skips too.

| Model | Wired | Receiver | Bluetooth LE |
|---|---|---|---|
| Steam Controller (2015, "D0G") | `0x1102` | `0x1142` (dongle, four slots) | `0x1105`, `0x1106` |
| Steam Controller (2026, "Triton"/"Ibex") | `0x1302` | `0x1304` (puck), `0x1305` | `0x1303` |

The 2015 controller's wired and dongle reports carry no report ID. Each
starts with the header SDL calls `ValveInReportHeader_t` (version 1, type,
length). Type 1 is state, type 3 is a dongle slot connecting or
disconnecting, and type 7 is Bluetooth state. Over Bluetooth, the reports
arrive as report `0x03` segments of 20 bytes, and the parser reassembles
them. The 2026 controller numbers its reports: `0x42`/`0x45` and `0x47` are
state, and `0x79`/`0x46` are receiver status.

Each connected controller is one pad, named `Steam Controller`. The button
icons follow it ([button-icons.md](button-icons.md), set `steam`).

## Lizard mode

Without a driver, the controller acts as a keyboard and mouse: the right pad
moves the pointer and the buttons type keys. While we hold a controller,
that is off. When we let go, or the run ends, it is restored.

- **2015, off:** feature `0x81` (clear the digital mappings), then `0x8E`,
  then `0x87`. The `0x87` report sets the left and right trackpad modes to
  none, smooth absolute mouse off, and wireless packet version 2.
- **2015, on:** `0x85` (default mappings), then `0x8E`, then `0x87` with the
  right trackpad mode back to mouse (0).
- **2026, off:** `0x87` with setting 9 cleared. The firmware forgets this, so
  the thread resends it every 3 s.
- **2026, on:** `0x85`, then `0x8E`.

Over Bluetooth, the 2015 controller's feature reports go out as `0x03`
segments.

## Mapping

| Steam Controller | Xbox pad |
|---|---|
| A, B, X, Y | A, B, X, Y |
| Right bumper / left bumper | Black / White (as for every pad) |
| Triggers | Triggers (analog; full while the trigger's click is down) |
| Back (View), Start (Menu) | Back, Start |
| Stick | Left stick |
| Stick click, left grip (2026: L4/L5) | Left stick click |
| Right trackpad | Aim (see below) |
| Right trackpad click, right grip (2026: R4/R5), 2026 right stick click | Right stick click |
| 2026 right stick | Right stick, plus the trackpad's aim |
| Left trackpad, d-pad mode | D-pad on click, toward the thumb |
| Left trackpad, stick mode | Left stick (the larger of it and the stick); click is the left stick click |
| 2026 d-pad | D-pad |
| Steam button, Quick Access | Not mapped |

The grips have no Xbox counterpart. Putting them on the stick clicks
(crouch and zoom in the default layout) is a choice: it saves pressing
the trackpad. In-game button layouts apply on top, as for any pad.

## Right trackpad: aim

The pad's position is never read as a stick. Finger travel between reports
is relative motion, as from a mouse, and it goes into the same look path
mouse aim takes (`input::look::LookMotion`): it turns you by the amount the
thumb moves and stops when it lifts. `trackpad::TrackpadAim`:

- **Touch down:** no motion until the second report, so a landing is not a
  jump.
- **Deadzone:** travel under 24 units (of ±32767) in a report is dropped,
  against jitter.
- **Jump guard:** a single step over 16384 units is taken as a re-touch and
  dropped.
- **Acceleration:** gain is `1 + 0.5 * min(speed / 100000, 2)`, with speed
  in units per second.
- **Sensitivity:** Trackpad Aim (1 to 10) over 5 multiplies the result; 5 is
  1.0.
- **Scale:** one unit of travel is 1/3000 of a look unit.

On the 2026 controller, the right stick and the trackpad's aim add up.

## Haptics: rumble and the speaker

Each trackpad has an actuator that buzzes at the rate it is pulsed. A
steady pulse train is a tone; the controller plays its startup tune this
way.

- **Pulse report, 2015:** feature `0x8F`. The fields are the pad (right
  0, left 1, both 2), then the on and off times in microseconds and the
  pulse count, all u16, then a gain byte.
- **Pulse report, 2026:** output report `0x81`, with the same fields.
- **Rumble, 2026:** output `0x80`, two motor levels. The thread resends it
  every 40 ms while it runs, as SDL does.

Everything is produced in 40 ms frames, 25 a second. Each frame sends at
most one pulse report when both pads play the same thing, or one per pad
when they differ. The 2026 controller also gets the rumble report while
its motors run. Linux throttles the 2015 controller to about 20 reports a
second of each kind, so the device should accept this rate; see
"Not verified".

- **Rumble.** On the 2015 controller, the game's left (heavy) motor is a
  35 Hz pulse train on the left pad and the right (light) motor is a 90 Hz
  train on the right pad. The pulse width follows the level, and levels
  under 0.02 are silent. On the 2026 controller, rumble goes to its rumble
  report.
- **Speaker.** These are the sounds the DualSense speaker gets: the
  controller-speaker routes ([playstation-audio.md](../audio/playstation-audio.md)),
  never 3D sounds. DirectSound renders them into a side output, as for the
  DualSense. Here the sink is `boot::steam_speaker::HapticSink`, which plays
  no audio. Each 40 ms of the mix becomes a pitch and a loudness
  (`haptics::analyse`: YIN at about 12 kHz, falling back to the zero-crossing
  rate, 60 to 1000 Hz, silent under 0.01 RMS). That becomes a pulse train
  on both pads: one pulse per period of the pitch, wider the louder it is.
  Each train lasts the frame plus 15 ms, so consecutive frames overlap
  instead of leaving gaps. On the 2015 controller, the tone takes a pad
  when it is at least as loud as that pad's motor; otherwise the motor's
  train plays. On the 2026 controller, rumble takes priority while its
  motors run.
- **Main output.** A haptic buzz is no substitute for the sound, so with
  haptics the routed sounds stay on the main output as well
  (`PadSpeakerConfig::for_haptics`, `VoiceOutput::Both`). With a DualSense
  speaker they move to the controller.

**Which speaker.** `audio::devices::pick_speaker` reads the device list once
at start. A PlayStation controller's audio device wins. If there isn't one
and the Steam Controller thread runs, the haptics take the sounds. The
haptics sink is live while a Steam Controller is connected and Steam Pad
Speaker is on; otherwise the routed sounds play on the main output only.

## Settings and variables

| Setting (`settings.toml`) | Row (Settings > Controls) | Variable | Values |
|---|---|---|---|
| `steam_pad.speaker` | Steam Pad Speaker | `H2_STEAM_SPEAKER` | on (default), off |
| `steam_pad.sensitivity` | Trackpad Aim | `H2_STEAM_SENSITIVITY` | 1 to 10, default 5 |
| `steam_pad.left_pad` | Left Trackpad | `H2_STEAM_LEFT_PAD` | `dpad` (default), `stick` |

The rows sit on the profile's Controls screen, after Controller Speaker.
All three apply at once. The iOS app never shows them; it shows the touch
rows instead. Other switches:

- `H2_STEAM_PAD=0`: don't open Steam Controllers at all (lizard mode stays
  as the controller has it).
- `--no-gamepads`, or a scripted input timeline: no Steam Controllers
  either.
- `pad_speaker.enabled` / `H2_PAD_SPEAKER` (Controller Speaker on the
  Controls screen): must be on for the speaker routes to run at all.
- `H2_PAD_SPEAKER_DEVICE=haptics` (or `steam`): use the haptics even when a
  DualSense is attached. Any other device name never falls back to
  haptics.

## Not verified

Checks on hardware that no test makes:

1. Plug the 2015 controller in by USB and start `h2 play`. Its mouse and
   keyboard emulation should stop (the right pad no longer moves the
   pointer). When you quit, they should come back.
2. Check A, B, X, Y, the bumpers, Back and Start, the stick, and both
   triggers: analog, and full on the click.
3. Swipe the right pad. You should turn by the distance swiped, with no
   drift when you lift your thumb. Faster swipes should turn a little
   further. Step Settings > Controls > Trackpad Aim and feel the change.
4. Click the left pad's edges for the d-pad. Switch Left Trackpad to Stick
   and walk with it.
5. Take damage or fire for rumble: the left pad should buzz low, the right
   pad higher.
6. With no DualSense attached and Controller Speaker on, the routed sounds
   (shield, reload, pickups) should buzz the pads as tones, and still play
   on the speakers. Turn Steam Pad Speaker off and the buzz should stop.
7. Repeat 1 and 3 through the wireless dongle, then over Bluetooth (BLE
   firmware).
8. The 2026 controller: wired, puck and Bluetooth. Also check its d-pad and
   right stick, that rumble goes to its motors, and that lizard mode stays
   off for more than a few seconds.

Open questions:

- **IOKit filtering.** Whether the primary usages IOKit reports match
  hidapi's on every link, so that only the controller interface is kept.
  The receiver and Bluetooth interfaces may report other usages.
- **2015 d-pad bits.** Whether the firmware sets the d-pad bits from a
  left-pad touch, or only on a click. In d-pad mode both count.
- **Pulse fields.** Pad 2 ("both") for pulses is from SDL's enum; Linux only
  sends 0 and 1. The gain byte's effect is untested, and we always send 0.
- **2026 pulses.** The side numbering of the 2026 pulse report is taken from
  Linux.
- **Report rates.** Whether 25 frames a second of pulses, up to 50 reports,
  is accepted without dropping. Linux's 20 a second is a throttle it chose,
  not a documented limit.
- **Bluetooth feature reports on macOS.** Whether segmented feature reports
  go through `IOHIDDeviceSetReport` as they do through hidapi.
- **GameController duplicates.** Whether Apple's GameController framework
  also offers a 2026 controller as a pad. If it does, the controller shows
  up twice.
- **Speaker quality.** How recognisable the speaker rendition is.
- **Ten Controls rows.** With the Steam rows, the Controls list shows ten
  rows: the six retail ones, Controller Speaker and the three Steam rows.
  The list is grown the way the speaker row grows it, one item height per
  row with no scrolling. Whether ten rows fit on screen hasn't been seen
  in game.
