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
0x3321c5 to 0x333130. Its exports are thin stdcall wrappers over a COM-style
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
timer 0x484248, armed from 0x332b6d; see the kernel README). The boot notes
record that 0x55810 is skipped under `H2_BOOT_OLD_GLUE`.

### 1.2 Communicator presence

The engine finds headsets itself. Its hot-plug poll at 0x332b97 calls
XPeekDevices (0x409884) for two XPP device types, 0x4088b0 (microphone) and
0x4088bc (headphone). Our `crates/xapi/src/controllers.rs` answers
peek_devices, get_devices and get_device_changes with 0 for any type that is
not a gamepad. So today the engine never sees a communicator, never calls
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
at +0x620, data at +0x624) and passes it to 0x54810. 0x564c0 walks up to 15
entries behind a flags byte. Each entry (0x55e10) is a byte below 2, an
optional u16 forward mask, then a 10-byte frame. 0x53890 and 0x553b0 then
call SubmitIncomingVoicePacket. The host of a game forwards frames to other
peers using the forward mask (0x55b40).

The game never looks inside the 10 bytes. They are the engine's codec frames:
one frame per DPC tick, so about 4 kbps if a frame is 20 ms. That is the
original voice codec's budget, and this design keeps it: the frames are the
real audio (section 3.3). The frame length, and so the bitrate, is still to
be confirmed by trace (section 3.3, "What to trace").

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
size is over 16 bytes; it never decodes the audio itself. With no headset,
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
   presence and `voice.mic_muted` (see 4). Insertions and removals arrive
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
   4); `voice.mic_muted` and `voice.incoming_muted` are the player's controls.

## 3. Host design

### 3.1 Keep the engine, emulate the communicator

The title's XHV engine (section 1.1) and the XDK voice codec it calls run as
recompiled game code, unchanged. The host replaces only what sits under them
on a real Xbox: the communicator, a USB headset that XPP exposes as two
media objects (XMOs). [playstation-audio.md](../audio/playstation-audio.md)
covers the same ground from the audio side; the completion routines at
0x37f38a and 0x37f3a8 are reached from XPP code at 0x410ddc to 0x410ef4.

| Piece | Host behaviour |
|---|---|
| Device presence (XPeekDevices, XGetDevices, XGetDeviceChanges) | report a communicator per port that has voice (2.1) |
| XVoiceCreateMediaObjectEx (0x411143), microphone | an XMO whose Process fills each capture packet with host PCM at the format the engine asks for |
| XVoiceCreateMediaObjectEx (0x411143), headphone | an XMO whose Process takes the engine's PCM packets and plays them (3.2) |
| Packet completion | signal the packet's event and status the way XPP does, on the engine's 20 ms DPC cadence (the kernel already runs that DPC) |

The game keeps its voice object, its packet code and its mute matrix, and
the engine keeps its encoder, decoder, voice detection and voice-mail mode.
Nothing in the XHV wrappers is intercepted. The host's audio threads hand
PCM to and from ring buffers; only the XMO entries, called on guest threads,
touch guest memory.

Whether the codec really is in the title (as playstation-audio.md says) or
is a media object the host must also provide is the first thing to trace
(3.3). If it is not in the title, the host implements it bit-exact as a
codec XMO under the same factory, and the rest of this design stands.

### 3.2 Capture and playback

A host `voice` module beside the audio crate, built on the pause-settings
lane's device trait (section 4):

```
trait Capture: Send {
    /// 20 ms mono frames at 48 kHz, after input volume.
    fn read(&mut self, frame: &mut [i16; 960]) -> bool;
}
trait Playback: Send {
    /// One mixed 20 ms frame, before output volume.
    fn write(&mut self, frame: &[i16; 960]);
}
trait VoiceIo {
    fn open_capture(&self, device: &DeviceId) -> Result<Box<dyn Capture>>;
    fn open_playback(&self, device: &DeviceId) -> Result<Box<dyn Playback>>;
}
```

The XMOs resample between the device's 48 kHz and the rate the engine's
packets use (traced in 3.3; 8 or 16 kHz 16-bit mono per public XDK
descriptions).

macOS first, on cpal over CoreAudio like `crates/audio/src/output.rs`. A fake
`VoiceIo` (scripted frames in, recorded frames out) is what the tests use.
Other ports implement the trait.

Playback has two routes: the voice output device when `voice.output_device`
names one (a headset), or mixed into the game's own output when it is "same as
game". The second route uses the DirectSound output thread's side sink
(`crates/dsound/src/output.rs`), so voice follows the game's volume and device
without a second stream.

Push-to-talk is a gate in the microphone XMO: with the button up it fills
packets with silence, so the engine's own voice detection never fires and
nothing is encoded. Open mic leaves the engine's detection to decide.

The audio path reads each `voice.*` setting through its `Setting<T>` handle:
one atomic load per 20 ms frame, no locks, no store lookups.

**One microphone and one set of speakers per machine** (section 6, decision
2). Split-screen players share them: the microphone XMO feeds the port whose
controller has push-to-talk held, or, in open-mic mode, the first signed-in
player with voice; the other ports' microphones get silence. Every port's
headphone object plays into the one output. Separate devices per player is
not planned.

### 3.3 Codec and the 10-byte frames

The original codec, at its original bitrate (about 4 kbps), unchanged. The
10-byte frames the game puts in its own session packets (1.3) are the real
encoded audio, and a receiving machine's engine decodes them. There is no
Opus, no side channel and no token scheme: the game's routing, forward
masks, team voice and the bidirectional mute rule decide who hears whom, as
on the original service, because only the packets the game sends carry
audio.

**What to trace before the communicator slice (2) and the voice-message
slice (5):**

1. **Where the codec lives.** Whether the encoder and decoder are recompiled
   title code reached from the XHV engine, or a media object created
   through 0x411143 that the host must provide. Name the entry points
   either way.
2. **The frame layout.** The 10-byte frame: which bytes are codec payload,
   whether any are a header (a sequence, a talker or a silence marker), and
   how silence or a dropped frame is represented. Confirm against the
   reader at 0x55e10 and SubmitIncomingVoicePacket (0x332df4, size 10).
3. **The bitrate.** The frame duration (one frame per 20 ms DPC tick, or
   longer), so the bitrate exactly; the PCM format the engine asks the
   microphone XMO for (rate, bits, packet size); and how many frames a
   session packet carries each tick (the 0x200-byte budget at 0x765c0 and
   the 15-entry walk at 0x564c0).
4. **The voice-message WAVE header.** The RIFF/WAVE `fmt ` chunk the
   voice-mail mode writes into the buffer at 0x504948 and that 0x53db0
   checks: format tag, channels, sample rate, block align, bits per sample,
   any extra bytes, and the `data` chunk; also how the format at 0x484290
   relates, and what message property 0x3c2's codec value 1 names.
5. How the engine reports a frame lost or late, so the headphone XMO keeps
   time when the network does not.

### 3.4 Transport

Voice rides inside the game's own session packets, so it travels wherever
those packets travel: direct UDP between the machines, or the session relay
when that is how the session is connected. The direct path and the relay
need no extra channel, port or message type for voice. At about 4 kbps a
talker inside the game's existing 0x200-byte budget, voice adds nothing the
relay's rate limits need to change for.

Nothing is stored. Live chat is never recorded on the server.

### 3.5 Voice messages

Recording and playback are the engine's own voice-mail mode
(SetProcessingMode 3, the record, stop and play thunks at 0x332e81,
0x332e8a and 0x332e93): it encodes with the same codec and writes the
RIFF/WAVE buffer at 0x504948 itself, and decodes it to the headphone XMO
on playback. The host provides only the microphone and headphone objects,
as for chat. The game's own limits stay: 15 s and fewer than 0x499a bytes
(0x8ef60), checked by the game before sending.

Sending: MessageSend and the friend request, team recruit and game invite
paths already hand the HLE the attachment. The HLE uploads the bytes as is
with the message. The server stores the voice blob in the **private** blob
store (crates/live-server/src/blobs.rs; the data dir natively, private R2 in
the Worker) under `voice/{message_id}`, never in a public bucket. The message
event carries the blob key, size and duration, not the bytes. The server
checks the size is under 0x499a bytes and the buffer starts with the traced
RIFF/WAVE header (3.3); it does not decode the audio.

Receiving: once the inbox HLE (MessageEnumerate, Details, Download) delivers
messages, Details reports the voice property, and Download streams the blob
through the existing task. Blobs are fetched with the recipient's token; the
server checks the caller is the message's recipient or sender.

**Retention** (section 6, decision 3): a voice message is kept until it is
deleted, with its message, by its recipient (or its sender, before it is
read). There is no expiry and no per-account cap beyond the existing
message rate limit; each message is at most 0x499a bytes, so storage stays
small.

Converting the audio for storage is not worth it: the blobs are already
tiny, and the game wants its own format back.

### 3.6 The mute list HLE

Server: a per-account mute list as private events on the account's stream,
`PlayerMuted { by, target }` and `PlayerUnmuted { by, target }`, written by
the server from authenticated requests, readable only by the owner (and
admins, for moderation context). Capped at 250 entries, the game's own Get
size. Endpoints:

- `GET  /v1/accounts/me/mutes` (scope `mutes.read`)
- `PUT  /v1/accounts/me/mutes/{xuid}` (scope `mutes.write`)
- `DELETE /v1/accounts/me/mutes/{xuid}`

The server stores the plain 8-byte XUID. The HLE keeps the 12 bytes the game
passed (flags included) next to it, so Get hands back exactly what Add was
given. For mutes the game did not add this session (a list loaded at sign-in,
or a mute made on the website), the HLE fills the flag word from the
friends/presence record for that XUID, or zero.

HLE:

- Get (startup, `(0, &task)`, and the full form): a task that fetches the
  list once per sign-in and then fills the buffer and count from the cached
  copy.
- Add and Remove: update the cache at once and queue the server write. They
  return S_OK straight away, as the game does not wait on a task. A failed
  write is retried on the next sign-in.
- When the cache changes, the game rebuilds its matrix the next time it calls
  Get; the wrappers at 0x58c10/0x58c60 already do that.

The website gets a mute list under the account's privacy settings, so a player
can review and remove mutes outside the game.

Blocking, as distinct from muting, is not a game concept. If a block list is
wanted later it should feed the same mute list (blocked implies muted) plus
the friends and messages checks.

### 3.7 Privacy

Who hears whom:

- Voice goes only to players in the same session.
- The game's routing decides which machines get voice frames (3.3): team
  voice, proximity and the mute matrix are all its rules, unchanged.
- Mutes are bidirectional, per the game, and the game's matrix is built
  from the server-backed mute list (3.6), so a mute holds on every machine
  in the session.
- There is no account-level voice switch (section 6, decision 4); bit 16 of
  the XUID flags stays clear. A player who wants no voice uses the settings
  below, and admins have bans (live-admin.md §9).
- `voice.incoming_muted` silences everyone locally (the headphone XMO plays
  nothing); `voice.mic_muted` stops capture in the microphone XMO and
  removes the communicator (2.1), so nothing is encoded and nothing leaves
  the machine.

What is stored: no chat audio, ever. Voice messages are stored privately and
readable only by sender and recipient.

Admins and reports:

- Reports already carry `game_id`. A voice-message report adds the message id:
  `ReportRequest.message` (optional). The report pins the blob: it is kept
  until the report is reviewed, even after the recipient deletes the
  message.
- In the dashboard report queue, an admin with `reports.moderate` can play the
  reported voice message. Only reported messages are reachable; there is no
  admin browse of voice messages.
- Every admin playback writes a server-signed `VoiceMessageAccessed { report,
  message, by }` event, visible in the report's history.
- After review, the pin is released, and a message already deleted is
  removed then.

Live chat cannot be reported with evidence, because nothing is recorded. A
chat report is the existing "Screaming" or "Harassment" feedback, as today.

## 4. Integration with pause-settings

The pause-settings lane adds Video and Voice rows to the pause menu and owns
these host settings in `crates/settings` Defs:

| Setting | Read where |
|---|---|
| `voice.input_device` | capture open (3.2) |
| `voice.output_device` | playback route: headset device or "same as game" |
| `voice.mode` (open mic, or push-to-talk and its button) | microphone XMO gate, each frame (3.2) |
| `voice.mic_muted` | microphone XMO gate, each frame; also the communicator answer (2.1) |
| `voice.incoming_muted` | headphone XMO gate, each frame |
| `voice.input_volume` | capture gain before the engine, each frame |
| `voice.output_volume` | playback gain after the engine, each frame |
| per-player mute list, if it lands there | not used; the Live mute list (3.6) is the one list |

Only one input and one output device are chosen per machine (3.2). It
also adds a host device-enumeration trait (`voice::Devices` or similar,
CoreAudio through cpal on macOS, plus a fake). This design builds `VoiceIo`
(3.2) beside that trait: `Devices` lists and names devices, `VoiceIo` opens
them, and both take the same `DeviceId`. If the two end up in the same
module, `VoiceIo` can be a supertrait of `Devices`.

Integration points:

1. The host voice engine takes `Setting<T>` handles at creation, from the
   same `settings::Store` the pause menu writes. It never keeps its own copy.
2. A device change (setting written) is picked up at the next 20 ms frame: the
   capture or playback stream is reopened on the audio thread.
3. The communicator answer (2.1) reads `voice.mic_muted` and whether
   `voice.input_device` resolves to a present device.
4. If pause-settings adds a per-player mute row, it should write through to
   the Live mute list (3.6) when signed in, rather than keep its own list. I
   will raise this with that lane before either side builds it.

At the time of writing the pause-settings branch has no commits beyond master,
so these names are the ones agreed in the brief. They will be checked against
the branch before voice code is written.

## 5. Build order

Each is its own slice, with tests:

1. **Mute list**: server events, endpoints in both homes, HLE Add, Remove
   and Get, website list. Useful on its own: the game's muted icons work,
   and it needs nothing from the codec.
2. **Trace the codec** (3.3, "What to trace") and write it up here: where it
   lives, the frame layout, the bitrate, the PCM format and the voice-mail
   WAVE header. No code.
3. **Communicator presence and the media objects with a fake `VoiceIo`**:
   the game offers voice, the real engine encodes scripted PCM into frames
   that go out in its packets, a second machine's engine decodes them to
   the fake output, and the presence voice flag goes out. Tested with two
   HLE instances and no real audio.
4. **Capture and playback on macOS**, through the pause-settings device
   trait and settings; push-to-talk and mic mute in the microphone XMO.
5. **Voice messages**: the voice-mail mode on the same objects, blob storage
   with the size and header check, inbox delivery (this depends on the
   inbox HLE delivering messages at all).
6. **Voice-message reports and admin playback.**

If the trace (2) finds the codec is not in the title, slice 3 grows a
bit-exact codec object before anything else in it.

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

1. The codec trace (3.3, "What to trace"): where the codec lives, the frame
   layout, the bitrate and PCM format, and the voice-mail WAVE header.
   **Proposed:** do it as its own slice (5, step 2) before any media-object
   code, and record the results in this document.
2. What 0x4c99b8 (the global gate) and 0x4c99a8 (value 3 blocks voice) are
   set by, and what port value 4 means in the engine calls. **Proposed:**
   trace alongside the codec; until then leave both as the game sets them.
3. Which UI reads the friend-row voice bit, and what tag 0x581 holds on game
   invites. **Proposed:** leave until slice 5; presence already forwards
   the flag.
4. A website mute list under the account's privacy settings (3.6).
   **Proposed:** build it with the mute-list slice.
5. If pause-settings adds a per-player mute row: **proposed** that it writes
   through to the Live mute list when signed in rather than keeping its own
   (4).
