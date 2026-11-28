# Live voice: chat, voice messages and the mute list

Status: design, with the user's decisions of 2026-11-18 folded in (section
6). No voice code is built yet. Player reports and bans (slices 1 and 1b)
are built and are described in [live-api.md](live-api.md) and
[live-admin.md](live-admin.md).

The decisions in short: keep the original voice codec and engine, emulating
only the communicator underneath it, so the game's own packets carry the
real audio; one microphone and one set of speakers per machine; voice
messages kept until deleted; no account-level voice switch.

This covers four things:

1. how the title does voice today, traced from the recompiled code;
2. what the game has to see for its own voice UI to work;
3. the host design: the communicator under the title's own engine and
   codec, capture and playback, voice messages on the server, the mute
   list, privacy;
4. how this plugs into the pause-settings lane's `voice.*` settings and device
   trait, which this lane consumes rather than duplicates.

All addresses are guest virtual addresses in the retail XBE. Nothing here is
copied from the disc; the doc names functions and fields, not data.

## 1. How the title does voice

### 1.1 The voice engine (XHV)

The title links the XHV voice engine and the voice-mail library into `.text`.
There is no separate XVOICE or VOICMAIL section. The XHV code sits roughly at
0x3321c5 to 0x333130.

Its exports are thin stdcall wrappers over a COM-style
engine:

| Wrapper | Address | Notes |
|---|---|---|
| XHVEngineCreate | 0x333055 | Release at 0x3330f8 |
| SetCallbackInterface | 0x332d94 | |
| EnableProcessingMode | 0x332d8f | |
| SetMaxDecodePackets | 0x332e7c | called with 15 |
| RegisterLocalTalker / Unregister | 0x332da3 / 0x332f27 | |
| SetProcessingMode | 0x332da8 | 2 = chat, 3 = voice mail |
| communicator status | 0x332db2 | |
| SetVoiceMask | 0x332dad | |
| Register / UnregisterRemoteTalker | 0x332db7 / 0x332dd3 | |
| GetRemoteTalkers | 0x332def | |
| SubmitIncomingVoicePacket | 0x332df4 | size 10 |
| IsRemoteTalking | 0x332e18 | |
| SetPlaybackPriority | 0x332e58 | |
| (unnamed) | 0x332e34 | |
| DoWork | 0x332f22 | per frame, from 0x53610 via 0x53580 |
| voice mail record / stop / play | 0x332e81 / 0x332e8a / 0x332e93 | |

The media object factory XVoiceCreateMediaObjectEx is at 0x411143 in the
XPP area. Nothing in the host implements it today.

The game wraps the engine in one voice object at 0x476fc8. Its vtable at
0x45095c is the title's ITitleXHV callback set:

- [0] 0x555f0, LocalChatDataReady: the engine has an encoded frame for a local
  talker;
- [1] 0x555b0, CommunicatorStatusUpdate: status 0 sets the per-port headset
  flag, 1 clears it;
- [4] 0x55640, voice-mail operation complete.

Fields the rest of the game reads: +0x10 the engine, +0x64 (0x47702c) the
per-port headset flag, +0x8c (0x477054) the local "talking" flag. A global gate
at 0x4c99b8 turns voice on and off as a whole.

Startup is 0x533a0 then 0x55810: 4 local talkers, up to 15 remote talkers in
chat mode. The kernel side already runs the engine's 20 ms DPC (0x332f4c on
timer 0x484248, armed from 0x332b6d; see the kernel README). The boot's old
0x55810 voice-off override is gone (docs/runtime/BOOT.md "Retired").

### 1.2 Communicator presence

The engine finds headsets itself. Its hot-plug poll at 0x332b97 calls
XPeekDevices (0x409884) for two XPP device types, 0x4088b0 (microphone) and
0x4088bc (headphone). Our `crates/platform/xapi/src/controllers.rs` answers
peek_devices, get_devices and get_device_changes with 0 for any type that is
not a gamepad.

So today the engine never sees a communicator, never calls
CommunicatorStatusUpdate with status 0, and the headset byte stays clear.

Everything downstream keys off that byte. The "can this player use voice"
check 0x536b0 needs:

- the headset byte for the port;
- 0x4c99a8 not equal to 3;
- profile bit 5 (the voice option in the player's profile);
- the account check 0x1906da: the player is not a guest and bit 16 of the
  XUID flags (voice not allowed) is clear.

### 1.3 Chat packets

Outgoing: LocalChatDataReady (0x555f0) goes to 0x53a20, which queues the frame
at 0x56130 (queue at 0x525a00). The per-peer packet builder (call site
0x76484) asks 0x765c0 for up to 0x200 bytes of voice inside a 0x516-byte frame.
0x53b00 and 0x56380 serialise the queued frames with a target mask built from
the mute words.

Incoming: the session message handler at 0x93577 reads a voice field (length
at +0x620, data at +0x624) and passes it to 0x54810. 0x55e10 walks up to 16
entries behind a flags byte and a count. Each entry is a byte below 2, an
optional u16 forward mask, then a 10-byte frame (3.3). 0x53890 and 0x553b0 then
call SubmitIncomingVoicePacket. The host of a game forwards frames to other
peers using the forward mask (0x55b40).

The game never looks inside the 10 bytes. They are the engine's codec frames:
a 2-byte sequence header and 8 bytes of payload per 20 ms, so 4 kbps (traced,
section 3.3). That is the original voice codec's budget, and this design
keeps it: the frames are the real audio.

Remote talkers are registered under synthetic XUIDs built from the 32-bit
session player id, not from account XUIDs.

### 1.4 Talking and muted icons

- Talking: 0x53750 reads the local flag 0x477054, or IsRemoteTalking for a
  remote player.
- Muted: 0x53850 reads the 16x16 matrix at 0x4c9968.
- Can voice: 0x536b0, above.

The matrix is rebuilt by 0x541a0 from the mute list against the session's
players (player stride 0x13c, XUID at +0). 0x57d60 checks both directions, so a
mute is bidirectional: if A mutes B, neither hears the other. That is the
game's rule and we keep it.

### 1.5 The mute list

The title uses three XOnline calls:

- XOnlineMutelistAdd at 0x3a084c (called from 0x58c45) and
  XOnlineMutelistRemove at 0x3a086e (from 0x58c95). Four words: controller,
  then the 12-byte XUID by value. No task. The caller only checks for a
  negative HRESULT.
- The wrappers 0x58c10 and 0x58c60 require a Live connection and a loaded
  list, then tail-jump to the get at 0x58b70.
- Get: `(controller, 250, 0, &task, buffer, &count)`. The buffer is at
  0x4c99d4 + controller * 0xfa0, 16 bytes an entry (12-byte XUID with flags,
  4 unused). The count is at 0x4cd854 + controller * 4; -1 means "not loaded".
- The list is first fetched as `(0, &task)` from 0x6c1a0 for each signed-in
  player who is not a guest.

The UI: player details (menu 0x2b55ae, handler 0x2b5d3b). Action 14 is Mute
(Add), 15 is Unmute (Remove). 0x58cb0 decides whether the player shows as
muted; it also wants bit 5 of the profile word at 0x54e8e0 + controller *
0xc70.

Because 0x541a0 compares XUIDs byte for byte, the HLE must return exactly the
12 bytes it was given, flags included, and Get must reflect an Add or Remove
straight away.

### 1.6 Voice messages

Recording: the record screen (0x2c9012, 0x2c9532) calls record 0x539a0 with a
15000 ms limit, stop 0x539e0, play 0x538e0. These go through the voice object
to the voice-mail thunks at 0x332e81, 0x332e8a and 0x332e93 (vtable +0x60,
+0x64, +0x68). The recording lands in one global buffer at 0x504948, capacity
0x499a bytes, size at 0x504940, duration at 0x5092e4.

Sending (0x2b7e69): size and duration must be non-zero, the buffer must start
with a RIFF/WAVE/fmt header (checked by 0x53db0), and 0x8ef60 rejects 0x499a
bytes or more. The message gets these properties:

| Property | Meaning |
|---|---|
| 0x9c1 | voice blob |
| 0x3c2 | codec, WORD, always 1 |
| 0x4c3 | duration, DWORD |
| 0x6c4 | text |
| 0x4c5 | language |
| 0x581 | 8 bytes, on game invites |
| 0x681 | team name, on team recruits |

MessageCreate flags: 4 has voice, 8 has text, 0x40 for team recruits. All four
message kinds can carry voice: friend request (0x8f6e0), team recruit
(0x8f500), game invite (0x8f7e0) and plain message (0x8f8b0). The HLE already
copies attachments up to 1 MiB and 16 per message
([live.md](live.md), outgoing messages) but does not deliver them.

Receiving: the message screen at 0x2c7410 shows text only if 0x1906da allows
it, requires codec 1, and reads the voice either inline (0x3a9d65) or by
download (0x3a9d74, 0x3a9d83, done when received equals total). It plays with
0x538e0(controller, buffer, size). The game checks the RIFF header and that
size is over 16 bytes; it never decodes the audio itself.

With no headset,
playback goes to the speakers.

### 1.7 Presence

0x6d040 builds the presence flags: 2 playing, 0x10 joinable, and 0x8 voice if
0x536b0 says the player can use voice. Friend rows (stride 0x56, state at
+0x1c) map 0x8 to an internal bit 2 via 0x8cf00, for the friends list's voice
icon.

## 2. What the game must see

For the game's own voice to work (talking icons, muted icons, the voice
option being offered, the voice icon on friends, and the audio itself), with
no change to game code:

1. **A communicator on each port that has voice.** Answer XPeekDevices,
   XGetDevices and XGetDeviceChanges for the microphone and headphone types
   with a bit for every port whose player has voice, driven by host mic
   presence and `voice.mute_microphone` (see 4). Insertions and removals arrive
   through the same device-change path as gamepads, so the engine calls
   CommunicatorStatusUpdate itself.
2. **The communicator's media objects.** The title's own XHV engine and its
   codec run recompiled. Underneath them the host implements the media
   object factory at 0x411143 (XVoiceCreateMediaObjectEx): a microphone
   object that fills capture packets from the host mic, and a headphone
   object that plays the decoded packets (section 3.1).
3. **Talking state.** Falls out of the real engine: it sets the local
   talking flag from its own voice detection on the captured PCM, and
   IsRemoteTalking from the frames it decodes.
4. **The mute list.** Add, Remove and Get backed by the server, byte exact
   (section 3.6). The game then builds its matrix and the muted icons by
   itself.
5. **Presence flag 0x8.** Falls out of 0x536b0 once 1 is in place. Our presence
   HLE already forwards the flags the game sets.
6. **Account voice permission.** Bit 16 of the XUID flags stays clear for
   every account. There is no account-level voice switch (section 6, decision
   4); `voice.mute_microphone` and `voice.mute_everyone` are the player's controls.

## 3. Host design

Moved to [Live voice: host design](live-voice-host.md#3-host-design).

### 3.1 Keep the engine, emulate the communicator

### 3.2 Capture and playback

### 3.3 Codec and the 10-byte frames

### 3.4 Transport

### 3.5 Voice messages

### 3.6 The mute list HLE

### 3.7 Privacy

## 4. Integration with pause-settings

[Player Settings](../game/settings-menu.md#pause-menu) opens the Voice screen
from the pause menu. Its rows edit these host settings:

| Setting | Read where |
|---|---|
| `voice.microphone` | capture open (3.2) |
| `voice.output` | playback route: headset device or "same as game" |
| `voice.talk_mode` (open mic, or push-to-talk and `voice.talk_button`) | microphone XMO gate, each frame (3.2) |
| `voice.mute_microphone` | microphone XMO gate, each frame; also the communicator answer (2.1) |
| `voice.mute_everyone` | headphone XMO gate, each frame |
| `voice.microphone_volume` | capture gain before the engine, each frame |
| `voice.volume` | playback gain after the engine, each frame |
| per-player mute list, if it lands there | not used; the Live mute list (3.6) is the one list |

Only one input and one output device are chosen per machine (3.2). It
also adds a host device-enumeration trait (`voice::Devices` or similar,
CoreAudio through cpal on macOS, plus a fake). This design builds `VoiceIo`
(3.2) beside that trait: `Devices` lists and names devices, `VoiceIo` opens
them, and both take the same `DeviceId`.

If the two end up in the same
module, `VoiceIo` can be a supertrait of `Devices`.

Integration points:

1. The host voice engine takes `Setting<T>` handles at creation, from the
   same `settings::Store` the pause menu writes. It never keeps its own copy.
2. A device change (setting written) is picked up at the next 20 ms frame: the
   capture or playback stream is reopened on the audio thread.
3. The communicator answer (2.1) reads `voice.mute_microphone` and whether
   `voice.microphone` resolves to a present device.
4. If pause-settings adds a per-player mute row, it should write through to
   the Live mute list (3.6) when signed in, rather than keep its own list. I
   will raise this with that lane before either side builds it.

## 6. Decisions and open questions

Decided by the user on 2026-11-18:

1. **Codec: keep the original.** Emulate the communicator under the title's
   own voice engine and its ~4 kbps codec, so the voice frames in the
   game's packets are the real audio (3.1, 3.3). No Opus and no side
   channel, so the direct path and the relay need no extra channel (3.4).
   This replaces the earlier design that swapped the XHV wrappers for an
   Opus engine with 10-byte tokens.
2. **Devices: one microphone and one set of speakers for everyone on the
   machine**, for now (3.2).
3. **Retention: voice messages are kept until deleted.** No expiry and no
   per-account cap; the game's own size check stays (fewer than 0x499a
   bytes, 15 s) (3.5).
4. **No account-level voice switch.** Bit 16 of the XUID flags stays clear
   (2.6, 3.7).

Still open, each with this design's recommendation marked **proposed**:

1. What port value 4 means in the engine calls. The gates are traced
   (3.3, items 6 and 7). **Proposed:** leave both as the game sets them.
2. Which UI reads the friend-row voice bit, and what tag 0x581 holds on game
   invites. **Proposed:** leave until slice 5; presence already forwards
   the flag.
3. If pause-settings adds a per-player mute row: **proposed** that it writes
   through to the Live mute list when signed in rather than keeping its own
   (4).
