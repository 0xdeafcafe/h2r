# PlayStation controller audio: speaker, mute button, microphone

Status: Partly built. The speaker routing and the mute button are built; the microphone is a design only.
Code: `crates/platform/dsound/src/padroute.rs`, `crates/extensions/game/src/pad_speaker.rs`, `crates/extensions/game/src/speaker_setting.rs`, `crates/extensions/game/src/speaker_row.rs`, `crates/platform/input/src/pad_hid.rs`. Tests: `crates/platform/dsound/tests/padroute.rs`.
Switch: `H2_PAD_SPEAKER`, `H2_PAD_SPEAKER_EXCLUSIVE`, `H2_PAD_MUTE`, listed in [switches.md](../runtime/switches.md).

Three things were asked for, on a DualSense (and a DualShock 4 where it can):

1. the controller's microphone as the Xbox Communicator's;
2. the mute button toggling mic mute, with its LED showing the state;
3. chosen game sounds (shields, reload, the empty click, skulls, round start)
   played on the controller's speaker, optionally not on the main speakers.

| Part | Over USB | Over Bluetooth | Built here |
|---|---|---|---|
| Speaker | Yes, as a CoreAudio output, once an HID report moves the output path to the speaker | Only with our own Opus encoder speaking a reverse-engineered HID protocol; not attempted | All of it: the sound index from the maps the game opens, the split mix into a second sink, the speaker-path report. Run: works on a USB DualSense. |
| Mute button + LED | Yes, raw HID | Yes, raw HID (78-byte reports with CRC) | All of it: IOKit in shared mode, the toggle, the LED report (`H2_PAD_MUTE`). Run: works on a DualSense. |
| Mic as Communicator | Mic capture works (CoreAudio lists a 2-channel 48 kHz input over USB); the game side needs XHV and Communicator HLE first | Opus over HID; not attempted | Design only (below) |

**Hardware** marks claims that need a check with a real pad. No game data is in this document or in
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
  Mac, and the headset jack works wired ([sony-mac]). **Seen on a real pad
  (macOS, USB):** CoreAudio lists "DualSense Wireless
  Controller" twice, once with 4 output channels and once with **2 input
  channels**, both at 48 kHz. So the mic is capturable on macOS over USB.
  The same pad's HID descriptor declares output report 0x02 as 47 bytes
  (48 with the ID; `MaxOutputReportSize` 48, input report 0x01 63 bytes),
  shorter than the 63 bytes Linux sends, so we send 48.
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
  creates its own DirectSound objects and a stream (`docs/audio/dsound.md`,
  "DSOUND entries"), and its 20 ms timer DPC runs (`crates/runtime/kernel/README.md`).
  The game's voice init (`0x55810`) runs for real since the dry DSP image
  was fixed (`docs/runtime/BOOT.md`).
- **The Communicator is absent.** `crates/platform/xapi/src/controllers.rs` knows
  only `XDEVICE_TYPE_GAMEPAD`; every other device type gets empty
  `XGetDevices`/`XGetDeviceChanges` masks, and `XInputOpen` fails. The real
  XPP (USB) stack is skipped. There is no XHV, voice microphone or voice
  headphone HLE anywhere.

So the mic cannot matter until the game sees a Communicator. That is not a
small HLE: see the design below.

## Routing game sounds to the speaker

### Telling which sound is playing

DirectSound sees buffers and stream packets, not tags. Two ways to label
them were considered.

1. **A guest hook where the engine picks a tag.** The sound core is
   `0x2185d0`..`0x2229d0` (`docs/engine/engine-map.md`), and named entry points
   exist (`sound_cache_initialize` `0x2185d0`, looping sounds `0x21a0c0`,
   `sound_impulse_start` `0x2a9e00` for scripts), but nobody has traced
   where an impulse becomes a stream packet. A hook would need that trace,
   a per-voice side table and the tag datum's name lookup. It would cost too
   much here.
2. **The data's identity (built).** Halo 2 hands DirectSound the encoded
   permutation bytes it loaded from the map: `SetBufferData` at `0x22158d`
   points into its own arena, and stream packets come out of the sound
   cache (`docs/audio/dsound.md`, "Guest objects"). A sound's opening bytes
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

In the style of `docs/game/settings-menu.md` (environment beats file, file beats
default; unknown keys are errors). `dsound::padroute::PadSpeakerConfig`:

| Variable | Meaning | Default |
|---|---|---|
| `H2_PAD_SPEAKER` | `0` off, `1` on with defaults, otherwise a TOML file of the keys below (on unless it says `enabled = false`). Set, it locks the in-game row (below) | unset: the in-game row decides, on until turned off |
| `H2_PAD_SPEAKER_DEVICE` | Output device name (substring, case-insensitive); `auto` tries DualSense, then a DS4, then a Steam Controller's trackpad haptics when one is connected; `haptics` (or `steam`) asks for those alone ([steam-controller.md](../input/steam-controller.md)) | `auto` |
| `H2_PAD_SPEAKER_EXCLUSIVE` | `1` plays routed sounds only on the controller, `0` on both | `1` |
| `H2_PAD_MUTE` | `1` the mute button toggles mute and its LED, `0` leaves the button alone. Set, it locks the Voice screen's Mute Button row (`[pad] mute_button`; a row press reaches the HID thread at once through `PadHid::mute_switch`: on sets the LED for the current state, off unmutes and puts the LED out) | `1` (acts once a DualSense is found) |

```toml
# H2_PAD_SPEAKER=~/pad-speaker.toml
device = "auto"
exclusive = true
routes = ["*/ui/*shield*", "*reload*", "*empty*", "*skull*"]   # tag-name globs
exclude = ["*music*"]
```

`routes` defaults to `padroute::DEFAULT_ROUTES`: the player's own sounds
(shields, reloads, the empty click, skulls, round-start cues, the player
character's pain, death, boarding, melee, jumps and landings, and
first-person weapon handling: melee, raising, inspection poses, dry fire,
grenade throws, zoom, overheating). Walking and running footsteps and
gunfire stay on the main output. Shields are the HUD's warnings, under a
`ui` folder: an enemy's shield (a Jackal's, an Elite's) is a character,
weapon or material sound and stays on the main output. Routing goes by
sound name, with one rule on top: a voice the game positions in 3D (a
`DSBCAPS_CTRL3D` buffer, or a stream whose output buffer is one) is never
routed, whatever it is called. Routing knows no player or port: on a
console with local Clankers (docs/game/bots.md §4.3, "Listeners") a
bot's own sounds reached the controller while its user counted as a
listener; it is not routed. These are generic words,
not tag paths from the disc: check them against your maps with `h2 tags
<map>` (filter `snd!`) and narrow them. Globs ignore case and treat `/` as `\`.
In the settings menu this belongs with the restart-only audio settings
(device) and the live ones (routes, exclusive). On and off is already in the
game's own Controls screen (below).

### The sound index (built)

`boot::pad_speaker` builds the index at run time from the player's own
files:

- **Where maps load.** The VFS calls an open observer
  (`vfs::Vfs::set_open_observer`) after every file it opens, with the file's
  host bytes (`vfs::HostFile`: the image path and the entry's offset and
  length for the DVD, the host file for the hard disk and installed map
  packs; `None` for a rewritten file). A `.map` with host bytes is queued
  for the indexer thread; the observer does nothing else under the VFS lock.
- **What is read.** The thread opens the region read-only with
  `map::Map::from_region`, reads the `ugh!` gestalt and each named `snd!`
  tag, resolves its permutations and reads only the first `PREFIX_BYTES`
  (or all of a shorter permutation) across its chunks, from the map itself
  or from `mainmenu.map`, `shared.map` and `single_player_shared.map`
  (located once through the VFS at `\Device\CdRom0\maps\`). A bad tag or
  chunk is skipped and counted; a file that is not a map is reported once.
- **When.** The shared maps are indexed at start, then each map once, the
  first time the game opens it. The index only grows. After each map the
  thread builds a `PadRouter` and installs it with
  `DirectSound::set_pad_router`. A router only decides which host output
  hears a voice, so installing it from another thread changes nothing the
  guest sees.

It runs only when the controller speaker is on (the default; `H2_PAD_SPEAKER=0`
turns it off) and the
host gives an audio sink (`h2 play`); headless runs never read the maps.

### The two outputs (built)

`dsound::OutputConfig::side` takes a `dsound::SideOutput`: an opener run on
the output thread that returns the controller's sink, or `None` when the
device is absent, and an `on_live` callback.

- While the side sink is open and `AudioSink::connected` (a `DeviceSink`
  says false once its stream errors, until it is reopened), the mixer renders
  with `render_split`: logical pacing in `Output::advance_to`, device pacing
  through `audio::Pump::pump_split`. The side mix goes to a queue of its own
  and a resampler to the side device's rate, played by the output thread.
  Both are rendered from the same frames, so they share one clock; the side
  sink can underrun on its own.
- Absent or unplugged, the mixer renders one output and every voice plays on
  the main mix. The side queue is dropped when the side goes down.
- With `H2_PAD_SPEAKER_EXCLUSIVE=1` (the default) the routes label voices
  `Side`, left out of the main mix; with `0` they are `Both`.
- `on_live(true)` when the side goes live, `on_live(false)` when it goes down
  or the output is dropped. `h2 play` wires it to the HID thread, which sends
  `OutputRequest { speaker: Some(true) }` (path 3, volume, preamp) and later
  `Some(false)`.
- `OutputStats::side` counts what the side output rendered and dropped.

`h2 play` opens the side sink with `DeviceSink::open_named(device)` and says
on stderr which device it found, or that every sound stays on the main
output.

Left for a pad in the game: the channel roles, the latency, and whether the
labels follow Halo's packets (below).

### In-game setting (built)

Settings, a profile, Controls has a seventh row, **Controller Speaker**,
under the vibration row (the Steam Controller rows, when shown, come after it), with the game's own on and off text (the strings
the vibration row uses) as its value. Pressing A on it flips the setting in
place; there is no dialog.

Why there: it is a per-controller hardware option like vibration, the
Controls list already has the label-and-value skin a toggle needs, and the
Controls row text function is already wrapped for the button layouts
(docs/input/controller-layouts.md). A new dialog screen would need a screen class
we do not have (docs/game/settings-menu.md).

Hook points (retail XBE, all function entries in our analysis):

| Address | Role | What we do |
|---|---|---|
| `0x2c84b8` | Controls list handler constructor: datum array `0x45d298`, element size 4, count 6 in EAX, six embedded item widgets | nothing; it shows the convention |
| `0x1a480d` | datum array constructor | for the name `0x45d298` and EAX 6, EAX becomes 7 |
| `0x2c8548` | row text (`ret 8`, item at `[esp+4]`, row at `[item+0x70]`): label text 0 and value text 1 of the row | row 6: label is our "Controller Speaker" (our string id, English only), value is the retail on/off ids `0x02000181` / `0x03000182`, the ones the vibration row uses, so the value is localized. Rows 0 to 5 run the original |
| `0x2c8719` | row chosen (thiscall, `ret 8`, row pointer at `[esp+8]`); rows past 4 open the vibration dialog | row 6: toggle and return; rows 0 to 5 run the original |
| `0x1a02d0` | string lookup | our id returns our text |

| `0x24bb60` | list handler "add items" (vtable slot 17, thiscall, plain `ret`): links the six embedded item widgets (`+0x88`, `0x80` bytes each) as children | for the Controls handler (vtable `0x45d240`): after the original, one item widget of ours per added row (`0x24abae` constructs it, `0x22ee92` sets its local player, `0x22eb18` appends it), in a slot of our page `0xa2f20000` per open screen; an item already linked only moves back to the end |
| `0x22f871` | screen tag (EAX = screen, returns its definition) | for screen 39, the list block's visible count (`i16` at `+6`) goes from 6 to 6 plus the rows we add, in every pane, once |
| `0x2b48c2`, `0x2b49e6`, `0x2b4a4d` | the smaller Controls screen the pause menu opens (screen 200, `controller_settings`: datum array `0x45b8b0`, one text per row, a value word per datum mapped to a string table) | its array also grows from 6 to 7; after its constructor we give the new datum value 6 (`0x16b840`, new datum), draw it as "Controller Speaker: On" or "Off" (our ids, `ID_BASE+0x30`, `+0x31`) and toggle on it |

So the main Controls list shows every row at once (seven) instead of scrolling: the list lays out one row per child
item widget up to the tag's visible count (`0x24bc12`), so adding the item
widgets and raising the count together is what it takes.

The row text is redrawn every update (the item's update method `0x24abe0`
calls the screen's row text slot), so the value changes as soon as the row
is pressed. Code: `crates/extensions/game/src/speaker_row.rs` (logic over the `Gm`
trait), registered by `layout_glue::install` because both features share
those entries.

What a toggle does (`boot::speaker_setting::SpeakerSetting` over the
`pad_speaker.enabled` setting in `settings::Store`, one switch shared by the
row, the sound index thread and the output thread; the row steps the store
and the store's change callback applies it):

- **Off:** `DirectSound::set_pad_router(None)`, so no voice is labelled for
  the controller and every new sound plays on the main output, and the HID
  speaker path goes back to the jack (`SpeakerSwitch::set(false)`). New
  routers from the sound index are kept, not installed. A sound already
  labelled for the controller when you turn it off (a long loop) finishes
  where it started; the next one plays on the main output.
- **On:** the newest router is installed again and, if the controller's
  output is live, the speaker path is switched on.
- No PlayStation controller: the row still shows and toggles. With no live
  side output a routed voice plays on the main mix anyway, and nothing is
  sent to a HID device that is not there.

**Persistence.** The choice is saved in `settings.toml` in `h2 play`'s data
root (`--data`, default `~/Library/Application Support/h2recomp/data`), the directory that already
holds the guest's `disk/` tree, but outside that tree:

```toml
[pad_speaker]
enabled = false
```

Written atomically (temporary file, rename), keeping any other keys. A file
that is not TOML, or a value that is not a boolean, is reported on stderr,
treated as absent (on) and never overwritten. The guest profile is not
touched. `--fresh-data` starts from a new data root, so it forgets the
choice along with the saves.

**The environment wins.** With `H2_PAD_SPEAKER` unset (or empty) the row and
the file decide. With it set, its value (`0` off, `1` on, a TOML file's
`enabled`) is used, the row shows that value, pressing it does nothing and
the file is left alone. Audio off (`--no-audio`) or a bad speaker setting
locks the row off. With `H2_PAD_SPEAKER=0` the controller output is not
opened at all, so the speaker cannot be turned on during that run.

Headless runs (`h2 run`) never add the row.

## Mute button and LED (built)

`input::dualsense`:

- `mute_pressed(Transport, report)`: the mute bit, or `None` for a short or
  foreign report.
- `MicMute`: one press toggles, as on a PS5; `led()` is solid while muted.
- `output_report(Transport, OutputRequest { mute_led, speaker }, seq)`:
  USB 0x02 or Bluetooth 0x31 with the CRC. Setting the LED also sets the
  hardware mic mute, as Linux does. Fields left `None` keep their
  valid-flag bits clear, so rumble, lightbar and player LEDs are untouched.

`input::pad_hid` opens the pad over raw HID, next to the gamepad backends:

- **IOKit, not hidapi.** An `IOHIDManager` matching `054c:0ce6`/`0df2`,
  opened with `kIOHIDOptionsTypeNone`. That does not seize the device, and
  gilrs opens it the same way, so GameController.framework and gilrs keep
  reading the buttons. It needs no new library: `objc2-io-kit` is already in
  the tree through gilrs. Input reports arrive through the manager's
  callback on the HID thread's run loop; output reports go out with
  `IOHIDDeviceSetReport`. Transport (USB or Bluetooth) comes from the
  device's `Transport` property. This part (`pad_hid_iokit.rs`) is thin and
  untested without hardware.
- **`PadLink`**, the state machine: on open it sends one report with the
  speaker path (if the controller output is playing) and the LED for the
  current mute state (if mute is on); a press of the mute button flips the
  state and sends the LED; `set_speaker` sends the path only when it changes.
  Only those valid-flag bits are set.
- **The thread** (`pad_hid::spawn`) looks for a pad once a second, follows
  the button and the `SpeakerSwitch` from `on_live`, and keeps the mute and
  speaker state across an unplug. Dropping it puts the path back to the jack.
- **`H2_PAD_MUTE`**: `1` (default) or `0`. On does nothing until a DualSense
  is found. With no Communicator the toggle only lights the LED and mutes the
  mic in the controller; the mute state is there for the voice design below.

If macOS refuses the open (`kIOReturnNotPermitted`, Input Monitoring), the
thread keeps retrying and input is unaffected.

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
session's voice channel; `crates/live/live-hle` would carry them like any other
UDP payload. Voice commands (if the game uses them offline) need only steps
1 to 3. Whoever builds Communicator HLE should agree with the Live work on
who owns the voice transport. This document owns the device and capture side.

## Tests

- `cargo test -p audio --lib`: `mixer::tests::side_output_*` (split mix,
  fallback without a side, volume and master), `sink::tests::pump_split_*`
  and `boxed_sinks_*`, `devices::tests`, and the `DeviceSink` tests with the
  named-target opener (including `connected` across a lost stream).
- `cargo test -p dsound --lib padroute` (fingerprints, globs, shared
  openings, labels, settings) and `cargo test -p dsound --test padroute`
  (streams and buffers through the guest calls, synthetic PCM).
- `cargo test -p dsound --test register`: the side output with fake sinks:
  `routed_sounds_play_only_on_the_controller_when_exclusive`, `..._on_both_
  when_not_exclusive`, `without_the_controller_device_...`,
  `an_unplugged_controller_hands_its_sounds_back_...` and
  `device_pacing_feeds_the_side_output_too`.
- `cargo test -p vfs --test storage opens_are_observed rewritten_files`: the
  open observer and host-file lookups on a synthetic XISO and host files.
- `cargo test -p boot --lib pad_speaker`: synthetic map files (invented tag
  index, gestalt and sample bytes) indexed from whole files, from regions
  of an image, with a chunk in the shared map, with the shared map missing,
  and through a VFS open.
- `cargo test -p boot --lib speaker_setting speaker_row layout_glue video_screen`: on by
  default, a router installed once it arrives, toggling off, on, off live
  (router removed and restored, HID path off and on), routers held back while
  off, toggling with no controller, the file round trip across "runs",
  other keys kept, bad files reported and not overwritten, the environment
  locking the row both ways; the Controls list growing by exactly one row
  (through EAX), row 6's label and value following the setting, rows 0 to 5
  and unreadable memory going to the original, a locked row swallowing the
  press, and the row read from the handler's second argument.
- `cargo test -p input --lib dualsense pad_hid` (mute bit, toggle, USB and
  Bluetooth reports, the CRC check value; `PadLink` report bytes and the
  thread over a fake HID device, including an unplug).

## Checking with a real pad

The routed sounds play from a USB DualSense's speaker, and the speaker is on
by default. To re-check, run `h2
play` with the pad plugged in, and check:

1. **The speaker plays the chosen sounds.** stderr says `controller speaker
   on <device>`. Shields, reloads, the empty click and the other default
   routes come out of the pad. If the speaker is silent but the jack plays
   them, the speaker-path report is not reaching the pad; if nothing plays,
   check the channel roles (the side mix is mono on channels 1 and 2) and
   the device's format in Audio MIDI Setup.
2. **The main speakers do not also play them** (the default
   `H2_PAD_SPEAKER_EXCLUSIVE=1`), and with `H2_PAD_SPEAKER_EXCLUSIVE=0` they
   do. A routed sound that still plays on the main output means its label
   did not follow Halo's packets (trace `Process` with `H2_TRACE`).
3. **The mute LED toggles** on each press of the mute button (lit while
   muted), and `H2_PAD_MUTE=0` leaves it alone. If macOS asks for Input
   Monitoring, say so here.
4. **Normal input still works**: sticks, buttons, triggers and rumble through
   GameController.framework, with the HID thread running, and after
   unplugging and replugging the pad.

Also worth noting: the latency between the two outputs, and what happens
when the pad is unplugged mid-sound (the sound should move to the main
output).

The in-game row:

5. Settings, a profile, Controls: a seventh row "Controller Speaker" under
   the vibration row, showing the game's "on" value, and "Video"
   under it: all eight rows visible, no scrolling. stderr says
   `layouts: list 0x45d298 grows from 6 to 8 rows`,
   `speaker row: controls list shows 8 rows` and
   `speaker row: row text active`. In a game, the pause menu's controller
   settings dialog has a last row "Controller Speaker: On" that toggles the
   same setting.
6. A on it: the value flips to the "off" text at once, no dialog opens, stderr
   says `pad speaker: off`. Reload sounds come from the TV. A again:
   back to "on", they come from the pad. Vibration and the other five rows
   open their usual dialogs.
7. Quit and start again: the row shows the last value, and `settings.toml`
   in the data root has it. With `H2_PAD_SPEAKER=0` the row shows off
   and A does nothing.

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
