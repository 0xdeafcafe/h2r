# PlayStation controller audio: speaker, mute button, microphone

Status: feasibility study, plus the parts that unit tests can prove. Three
things were asked for, on a DualSense (and a DualShock 4 where it can):

1. the controller's microphone as the Xbox Communicator's;
2. the mute button toggling mic mute, with its LED showing the state;
3. chosen game sounds (shields, reload, the empty click, skulls, round start)
   played on the controller's speaker, optionally not on the main speakers.

| Part | Over USB | Over Bluetooth | Built here |
|---|---|---|---|
| Speaker | Yes, as a CoreAudio output, once an HID report moves the output path to the speaker | Only with our own Opus encoder speaking a reverse-engineered HID protocol; not attempted | Routing (tag matching, a split mix, picking the device). Not yet wired into the output thread. |
| Mute button + LED | Yes, raw HID | Yes, raw HID (78-byte reports with CRC) | Report parsing, the toggle, report building. No HID device I/O yet. |
| Mic as Communicator | Mic capture maybe (unverified on macOS); the game side needs XHV and Communicator HLE first | Opus over HID; not attempted | Design only (below) |

No PlayStation controller was connected when this was written
(`system_profiler SPUSBDataType SPBluetoothDataType` listed only an Xbox
pad), so nothing below was tried on hardware. **Hardware** marks claims
that need a check with a real pad. No game data is in this document or in
the code; sound tags are matched against the player's own maps at run time.

## What macOS gives us

### USB

- **The DualSense is a USB Audio Class device.** CoreAudio shows it as an
  output named "DualSense Wireless Controller", 4 channels at 48 kHz.
  chiaki-ng's log has `Haptics Audio Device 'DualSense Wireless Controller'
  opened with 4 channels @ 48000 Hz` with the coreaudio driver, and its docs
  tell users to set the device to "Quadraphonic" in Audio MIDI Setup
  ([chiaki-ng#709], [chiaki-ng setup]). Channels 1 and 2 feed the speaker
  and headphone path, and channels 3 and 4 the two haptic actuators
  ([dse-haptics], [DS5Tee]). One Mac guide routes the speaker to the back
  pair instead ([jas-guide]), so the channel roles are **Hardware**.
- **The speaker is silent until the host routes it.** The firmware plays
  USB audio on the headphone jack even with nothing plugged in. Linux's
  `hid-playstation` sends an output report that selects output path 3 (mute
  the headphones, play the right channel on the speaker), speaker volume
  `0x64` and +6 dB preamp gain when no headphone is detected
  ([hid-playstation.c], `dualsense_output_worker`). We do the same:
  `input::dualsense::output_report(.., OutputRequest { speaker: Some(true), .. })`.
  Path 3 takes the **right** channel, so the side mix writes its mono signal
  to both channels.
- **The microphone.** One blog says macOS lists a "Wireless Controller"
  input and asks for mic permission ([pitch-midi]); the Mac guide above says
  "0 ins". Sony says the built-in mic and speaker are not supported on a
  Mac, and the headset jack works wired ([sony-mac]). **Hardware.**
- **DualShock 4.** USB audio is the headset jack (Windows calls it "Headset
  Earphone (Wireless Controller)"); psdevwiki says v2 audio is 2 channels at
  32 kHz and that a feature report (0xE0) routes it to the speaker
  ([pcgw-ds4], [psdevwiki-ds4]; both pages fetched only as search snippets).
  There is no built-in mic and no mute button. `audio::devices::pick_controller`
  tries "DualSense" and then "Wireless Controller", so a DS4 is found when
  `auto` is set. The DS4 speaker route is not built.

### Bluetooth

macOS exposes no audio device for either pad over Bluetooth: the audio
travels in vendor HID reports. Linux says "Bluetooth audio is currently not
supported" ([hid-playstation.c]).

- **DualSense speaker:** Opus (48 kHz, about 160 kbit/s, 200-byte frames
  every 10.667 ms) in a large output report (0x36, 398 bytes) with
  sub-packets for haptics, speaker and jack ([LinuxAudio4Dualsense5]).
  DS5Tee uses report 0x39 instead ([DS5Tee]), so the protocol is not
  settled. Haptics-only streaming exists as a proof of concept ([SAxense],
  [dualsense-bt-haptics]).
- **DualSense mic:** Opus mono in input report 0x31, enabled by a config
  write that also disturbs gamepad input ([LinuxAudio4Dualsense5]).
- **DS4:** SBC (32 kHz) in output reports 0x14 to 0x19; the headset mic
  comes back as SBC at 16 kHz ([dsz#26], [DS4-Sound-Link]).

None of these is a library we could link; each is a project-specific
implementation, and Bluetooth audio would mean an Opus or SBC encoder, a
paced HID writer and CRCs on every report. Not attempted. The mute button
and LED, however, work the same over Bluetooth (below).

### Mute button and LED

- **GameController.framework has no mute element.** `GCDualSenseGamepad`
  adds the touchpad and the adaptive triggers only ([objc2-gcdualsense]),
  and there is no API for the mute LED. So both need raw HID.
- **Input:** the mute button is `buttons[2]` bit 2 (`DS_BUTTONS2_MIC_MUTE`),
  byte 10 of USB report 0x01 and byte 11 of Bluetooth report 0x31
  ([hid-playstation.c]). SDL3 reads the same bit and maps it to
  `SDL_GAMEPAD_BUTTON_MISC1` ([SDL_hidapi_ps5.c], [SDL_gamepad.c]). Over
  Bluetooth, the short report 0x01 has no mute bit. The pad switches to the
  full 0x31 report after any output report ([horizon#348], [OpenEmu#4790]).
- **Output:** USB report 0x02 (63 bytes) or Bluetooth report 0x31 (78 bytes:
  sequence nibble, tag `0x10`, and a CRC-32 over `0xA2` plus the first 74
  bytes). `valid_flag1` bit 0 enables the mute LED and bit 1 the power-save
  byte, whose bit 4 mutes the mic in hardware. Linux writes 0 and 1 to the
  LED byte ([hid-playstation.c]); SDL's comment gives 2 as pulse
  ([SDL_hidapi_ps5.c]).
- **Opening the device:** hidapi on macOS seizes the device by default.
  `hid_darwin_set_open_exclusive(0)` (the Rust hidapi crate's
  `macos-shared-device` feature) shares it, and one Rust project reports
  DualSense output reports working next to gilrs that way ([hidapi-darwin],
  [horizon#348]). A failed open with `kIOReturnNotPermitted` means the
  Input Monitoring permission ([apple-forum-744577]). Whether a gamepad
  alone triggers that prompt is **Hardware**.

## How the runtime handles voice today

- XHV (the voice engine, XDK libraries `XVOICE`, `VOICMAIL`) is statically
  linked game code and runs recompiled. `XHVEngineCreate` is `0x333055`; it
  creates its own DirectSound objects and a stream (`docs/dsound.md`,
  "DSOUND entries"), and its 20 ms timer DPC runs (`crates/kernel/README.md`).
  The game's voice init (`0x55810`) runs for real since the dry DSP image
  was fixed (`docs/BOOT.md`).
- **The Communicator is absent.** `crates/xapi/src/controllers.rs` knows
  only `XDEVICE_TYPE_GAMEPAD`; every other device type gets empty
  `XGetDevices`/`XGetDeviceChanges` masks, and `XInputOpen` fails. The real
  XPP (USB) stack is skipped. There is no XHV, voice microphone or voice
  headphone HLE anywhere.
- The Live docs (`docs/live*.md`) do not mention voice. `crates/live-hle`
  carries plain UDP; a test sends a `voice-frame` payload, nothing more.

So the mic cannot matter until the game sees a Communicator. That is not a
small HLE: see the design below.

## Routing game sounds to the speaker

### Telling which sound is playing

DirectSound sees buffers and stream packets, not tags. Two ways to label
them were considered.

1. **A guest hook where the engine picks a tag.** The sound core is
   `0x2185d0`..`0x2229d0` (`docs/engine-map.md`), and named entry points
   exist (`sound_cache_initialize` `0x2185d0`, looping sounds `0x21a0c0`,
   `sound_impulse_start` `0x2a9e00` for scripts), but nobody has traced
   where an impulse becomes a stream packet. A hook would need that trace,
   a per-voice side table and the tag datum's name lookup. It would cost too
   much here.
2. **The data's identity (built).** Halo 2 hands DirectSound the encoded
   permutation bytes it loaded from the map: `SetBufferData` at `0x22158d`
   points into its own arena, and stream packets come out of the sound
   cache (`docs/dsound.md`, "Guest objects"). A sound's opening bytes
   identify it. `dsound::padroute::SoundIndex` maps a fingerprint (FNV-1a
   of the first `PREFIX_BYTES` = 576 bytes, 16 mono Xbox ADPCM blocks, or
   of all the bytes of a shorter permutation, plus its length) to the tag
   names that start that way. `PadRouter` resolves each fingerprint once
   against the route table.

How labels follow a voice (`padroute::next_output`):

- `SetBufferData`: the buffer is labelled from its data.
- `Process`: a packet that starts an indexed sound labels the stream. An
  unknown packet keeps the label while the stream still has packets queued
  (the rest of that sound) and clears it on an idle stream. `Flush` clears it.
- Sounds whose opening is shared by several tags (silence, a common attack)
  are routed only if every owner routes the same way; otherwise they stay on
  the main mix.

Limits: the label is per stream, so a queued packet of the previous sound
already plays on the new label. A sound whose first packet is shorter than
576 bytes (and is not a whole short permutation) is not recognised. WMA
sounds go through the codec boundary and are not matched. Whether Halo's
packets really start at permutation starts is **Speculation** until a trace
shows it; tracing `Process` with `H2_TRACE` on one level answers it.

### Mixing to two devices

- `audio::VoiceOutput` (`Main`, `Both`, `Side`) is a voice parameter.
- `Mixer::render_split(out, Some(side))` adds `Both` and `Side` voices to
  `side` as mono at their volume, headroom and master volume, with no pan,
  3D, mix bins or reverb. `Side` voices are left out of `out`. With no side
  output (`render`) every voice plays on the main mix, so a sound routed to
  an absent controller is never lost.
- `audio::devices::pick_controller` and `DeviceSink::open_named(pattern)`
  open the controller's CoreAudio device by name and stay on it, retrying
  once a second while it is unplugged. `write_frames` leaves channels 3 and
  4 (haptics) silent.

### Settings

In the style of `docs/settings-menu.md` (environment beats file, file beats
default; unknown keys are errors). `dsound::padroute::PadSpeakerConfig`:

| Variable | Meaning | Default |
|---|---|---|
| `H2_PAD_SPEAKER` | `1` on with defaults, `0` off, otherwise a TOML file of the keys below (on unless it says `enabled = false`) | off |
| `H2_PAD_SPEAKER_DEVICE` | Output device name (substring, case-insensitive); `auto` tries DualSense, then a DS4 | `auto` |
| `H2_PAD_SPEAKER_EXCLUSIVE` | `1` plays routed sounds only on the controller, `0` on both | `1` |

```toml
# H2_PAD_SPEAKER=~/pad-speaker.toml
device = "auto"
exclusive = true
routes = ["*shield*", "*reload*", "*empty*", "*skull*"]   # tag-name globs
exclude = ["*music*"]
```

`routes` defaults to `padroute::DEFAULT_ROUTES`. These are generic words,
not tag paths from the disc: check them against your maps with `h2 tags
<map>` (filter `snd!`) and narrow them. Globs ignore case and treat `/` as `\`.
In the settings menu this belongs with the restart-only audio settings
(device) and the live ones (routes, exclusive).

### What is left for the speaker

1. **Build the index.** For each map the game opens (and `shared`,
   `single_player_shared`, `mainmenu`), walk `tags::sound::Gestalt` and each
   `snd!` tag's permutations, read the first chunk's first 576 bytes with
   `ResourceReader`, and `SoundIndex::insert(tag name, bytes)`. Then call
   `DirectSound::set_pad_router`. This needs a hook on map load (the VFS sees
   the file open) or an index of every map built once at start-up.
2. **The output thread** (`crates/dsound/src/output.rs`) renders with
   `render` into one sink. It needs a second sink and `render_split`, both
   paced by the same logical clock (the side ring may underrun separately).
3. **The speaker path report:** send `OutputRequest { speaker: Some(true) }`
   when the side device opens, and `Some(false)` on exit (see below for the
   HID side).
4. A check with a pad in the game: the channel roles, the latency, and
   whether the labels follow Halo's packets.

## Mute button and LED (built: the report logic)

`input::dualsense`:

- `mute_pressed(Transport, report)`: the mute bit, or `None` for a short or
  foreign report.
- `MicMute`: one press toggles, as on a PS5; `led()` is solid while muted.
- `output_report(Transport, OutputRequest { mute_led, speaker }, seq)`:
  USB 0x02 or Bluetooth 0x31 with the CRC. Setting the LED also sets the
  hardware mic mute, as Linux does. Fields left `None` keep their
  valid-flag bits clear, so rumble, lightbar and player LEDs are untouched.

Left: the HID device itself. Neither gilrs nor GameController.framework
passes raw reports, so this needs IOKit (`IOHIDManager` matching
`054c:0ce6`/`0df2`, input report callback, `IOHIDDeviceSetReport`) or hidapi
in shared mode, on a thread of its own next to the existing backends
(`docs/input.md`). Proposed setting `H2_PAD_MUTE=1`, off by default. The mute
state then feeds the voice design below; with no Communicator it only lights
the LED.

## Microphone as the Communicator (design, not built)

What Halo 2 needs before a mic matters:

1. **Device presence.** `XGetDevices(XDEVICE_TYPE_VOICE_MICROPHONE)` and
   `XDEVICE_TYPE_VOICE_HEADPHONE` must report a Communicator in the port of
   the player's pad, with insertion through `XGetDeviceChanges`. The two
   type descriptors' guest addresses are not yet named (only the gamepad's
   `0x4086f8` is), so the first step is to find them next to it in `.data`.
2. **The XMO devices** (XDK names from public XDK descriptions; how Halo
   uses them is Speculation). On the Xbox the Communicator is two XMOs over XPP
   isochronous USB: a microphone XMO (`XVoiceCreateMediaObject` / `Process`
   with capture packets, 8 or 16 kHz 16-bit mono) and a headphone XMO. XHV
   runs above them and the completion routines at `0x37f38a`/`0x37f3a8` are
   reached from XPP code (`0x410ddc`..`0x410ef4`). HLE means replacing the
   XPP open/process path for voice devices with host objects that fill
   capture packets from the host mic and play headphone packets to the
   controller (or the main output).
3. **Codec.** XHV encodes with the XDK voice codec (`XVOICE`) inside game
   code. That runs recompiled and needs no HLE, as long as the packets in
   and out are right.

Host side: open the DualSense's CoreAudio input (USB) through cpal, resample
to the XMO's rate, and gate it with `MicMute`. Over Bluetooth there is no
mic without the Opus input report above.

**Live overlap.** Voice chat sends XHV's encoded frames through the Live
session's voice channel; `crates/live-hle` would carry them like any other
UDP payload. Voice commands (if the game uses them offline) need only steps
1 to 3. Whoever builds Communicator HLE should agree with the Live work on
who owns the voice transport. This document owns the device and capture side.

## Tests

- `cargo test -p audio --lib`: `mixer::tests::side_output_*` (split mix,
  fallback without a side, volume and master), `devices::tests`, and the
  `DeviceSink` tests with the named-target opener.
- `cargo test -p dsound --lib padroute` (fingerprints, globs, shared
  openings, labels, settings) and `cargo test -p dsound --test padroute`
  (streams and buffers through the guest calls, synthetic PCM).
- `cargo test -p input --lib dualsense` (mute bit, toggle, USB and
  Bluetooth reports, the CRC check value).

## Sources

- chiaki-ng#709: <https://github.com/chiaki-ng/chiaki-ng/issues/709>
- chiaki-ng setup: <https://chiaki-ng.github.io/chiaki-ng/setup/controlling/>
- dse-haptics: <https://github.com/iranran666/dse-haptics>
- DS5Tee: <https://github.com/ProfessorTee/DS5Tee>
- jas-guide: <https://www.jesseaustinstewart.com/music-for-playstation-mac-setup-page>
- hid-playstation.c: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/hid/hid-playstation.c>
- pitch-midi: <https://midi.aidxn.com/blog/dualsense-microphone-pitch-detection-midi/>
- sony-mac: <https://www.playstation.com/en-us/support/hardware/pair-dualsense-controller-bluetooth/>
- pcgw-ds4: <https://www.pcgamingwiki.com/wiki/Controller:DualShock_4>
- psdevwiki-ds4: <https://www.psdevwiki.com/ps4/DS4-USB>
- LinuxAudio4Dualsense5: <https://github.com/ebraminio/LinuxAudio4Dualsense5>
- SAxense: <https://github.com/egormanga/SAxense>
- dualsense-bt-haptics: <https://github.com/awalol/dualsense-bt-haptics>
- dsz#26: <https://github.com/MaximumFreak/dsz/pull/26>
- DS4-Sound-Link: <https://github.com/abh1shekbana/DS4-Sound-Link>
- objc2-gcdualsense: <https://docs.rs/objc2-game-controller/latest/objc2_game_controller/struct.GCDualSenseGamepad.html>
- SDL_hidapi_ps5.c: <https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_ps5.c>
- SDL_gamepad.c: <https://github.com/libsdl-org/SDL/blob/main/src/joystick/SDL_gamepad.c>
- horizon#348: <https://github.com/ludovic111/horizon/pull/348>
- OpenEmu#4790: <https://github.com/OpenEmu/OpenEmu/issues/4790>
- hidapi-darwin: <https://github.com/libusb/hidapi/blob/master/mac/hidapi_darwin.h>
- apple-forum-744577: <https://developer.apple.com/forums/thread/744577>

[chiaki-ng#709]: https://github.com/chiaki-ng/chiaki-ng/issues/709
[chiaki-ng setup]: https://chiaki-ng.github.io/chiaki-ng/setup/controlling/
[dse-haptics]: https://github.com/iranran666/dse-haptics
[DS5Tee]: https://github.com/ProfessorTee/DS5Tee
[jas-guide]: https://www.jesseaustinstewart.com/music-for-playstation-mac-setup-page
[hid-playstation.c]: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/hid/hid-playstation.c
[pitch-midi]: https://midi.aidxn.com/blog/dualsense-microphone-pitch-detection-midi/
[sony-mac]: https://www.playstation.com/en-us/support/hardware/pair-dualsense-controller-bluetooth/
[pcgw-ds4]: https://www.pcgamingwiki.com/wiki/Controller:DualShock_4
[psdevwiki-ds4]: https://www.psdevwiki.com/ps4/DS4-USB
[LinuxAudio4Dualsense5]: https://github.com/ebraminio/LinuxAudio4Dualsense5
[SAxense]: https://github.com/egormanga/SAxense
[dualsense-bt-haptics]: https://github.com/awalol/dualsense-bt-haptics
[dsz#26]: https://github.com/MaximumFreak/dsz/pull/26
[DS4-Sound-Link]: https://github.com/abh1shekbana/DS4-Sound-Link
[objc2-gcdualsense]: https://docs.rs/objc2-game-controller/latest/objc2_game_controller/struct.GCDualSenseGamepad.html
[SDL_hidapi_ps5.c]: https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_ps5.c
[SDL_gamepad.c]: https://github.com/libsdl-org/SDL/blob/main/src/joystick/SDL_gamepad.c
[horizon#348]: https://github.com/ludovic111/horizon/pull/348
[OpenEmu#4790]: https://github.com/OpenEmu/OpenEmu/issues/4790
[hidapi-darwin]: https://github.com/libusb/hidapi/blob/master/mac/hidapi_darwin.h
[apple-forum-744577]: https://developer.apple.com/forums/thread/744577

Every source above is cited for facts only; no code was copied. The report
offsets were read from `hid-playstation.c` (GPL-2.0) and SDL (zlib) and
re-expressed as constants.
