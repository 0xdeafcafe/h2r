# Live voice: chat, voice messages and the mute list

Status: design only. Nothing in this document is built yet. It is written for
review before any voice code lands. Player reports (slice 1) are built and are
described in [live-api.md](live-api.md) and [live-admin.md](live-admin.md).

This covers four things:

1. how the title does voice today, traced from the recompiled code;
2. what the game has to see for its own voice UI to work;
3. the host design: capture and playback, codec, transport, voice messages
   on the server, the mute list, privacy;
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
original voice codec's budget; it is far too small for Opus at any useful
quality, which shapes the transport design below.

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

For the game's own UI to work (talking icons, muted icons, the voice option
being offered, the voice icon on friends), with no change to game code:

1. **A communicator on each port that has voice.** Answer XPeekDevices,
   XGetDevices and XGetDeviceChanges for the microphone and headphone types
   with a bit for every port whose player has voice, driven by host mic
   presence and `voice.mic_muted` (see 4). Insertions and removals arrive
   through the same device-change path as gamepads, so the engine calls
   CommunicatorStatusUpdate itself.
2. **An engine that behaves like XHV.** Either the real engine runs on top of
   an HLE media object at 0x411143, or the XHV wrappers are replaced. This
   design replaces the wrappers (section 3.1): the host owns the audio, and
   the game gets the callbacks it expects.
3. **Talking state.** The local talking flag set when the local player's voice
   activity detector fires; IsRemoteTalking true while a remote peer's audio is
   playing.
4. **The mute list.** Add, Remove and Get backed by the server, byte exact
   (section 3.6). The game then builds its matrix and the muted icons by
   itself.
5. **Presence flag 0x8.** Falls out of 0x536b0 once 1 is in place. Our presence
   HLE already forwards the flags the game sets.
6. **Account voice permission.** Bit 16 of the XUID flags stays clear unless
   the account has voice turned off (an account setting; see 3.7).

## 3. Host design

### 3.1 Replace XHV, keep its shape

The host replaces the XHV wrappers listed in 1.1 with HLE entries, the same way
the XOnline entries are intercepted. The game keeps its voice object, its
packet code and its mute matrix; only the engine underneath changes.

| Wrapper | Host behaviour |
|---|---|
| Create, Release | create or drop the host voice engine; hand the game a fake engine pointer |
| SetCallbackInterface | remember the game's ITitleXHV object at 0x476fc8 |
| Register / UnregisterLocalTalker | open or close capture for that port |
| Register / UnregisterRemoteTalker | add or drop a playback stream keyed by the synthetic talker id |
| SetProcessingMode | chat or voice mail for a port |
| SetVoiceMask | ignored at first (voice masks are a voice-changer effect) |
| SubmitIncomingVoicePacket | treat the 10 bytes as a token (3.3), not audio |
| IsRemoteTalking | true while that talker's playback has audio |
| DoWork | drain host events and call the game's callbacks on the game thread |
| voice mail record / stop / play | record to, or play from, a RIFF/WAVE buffer (3.5) |

Callbacks into the game run only from DoWork, which the game calls once a
frame on its own thread. The host's audio threads never touch guest memory.

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

macOS first, on cpal over CoreAudio like `crates/audio/src/output.rs`. A fake
`VoiceIo` (scripted frames in, recorded frames out) is what the tests use.
Other ports implement the trait.

Playback has two routes: the voice output device when `voice.output_device`
names one (a headset), or mixed into the game's own output when it is "same as
game". The second route uses the DirectSound output thread's side sink
(`crates/dsound/src/output.rs`), so voice follows the game's volume and device
without a second stream.

Voice activity detection is a simple energy gate with hang time. It drives the
local talking flag and decides whether a frame is sent at all. In push-to-talk
mode the gate is the button.

The audio path reads each `voice.*` setting through its `Setting<T>` handle:
one atomic load per 20 ms frame, no locks, no store lookups.

One capture device serves the whole machine. Split-screen players share it; the
host attributes speech to the port whose controller has push-to-talk held, or,
in open-mic mode, to the first signed-in player with voice. That is a
limitation worth stating, not solving now.

### 3.3 Codec and the 10-byte frames

Opus, 48 kHz mono, 20 ms frames, VBR around 24 kbps, with in-band FEC on.
The Opus packets do not fit in the game's 10-byte voice frames, so they do not
travel inside the game's packets. Two channels run side by side:

- **The game's packets keep carrying 10-byte frames**, so all the game's
  routing, forward masks and mute checks run unchanged. When the host engine
  has a 20 ms Opus packet ready for a local talker, it raises
  LocalChatDataReady with a 10-byte token: a version byte, the sending
  machine's voice stream id (4 bytes), a sequence number (4 bytes) and a
  flags byte. The game sends the token to the peers it chooses; anyone it
  does not route to (mutes, team chat) never gets a token.
- **The Opus packets travel on a host side channel** (3.4) keyed by stream id
  and sequence number. A receiving host plays a packet only after the game has
  submitted the matching token through SubmitIncomingVoicePacket. A packet
  with no token within 200 ms is dropped.

So the game still decides who hears whom, including the bidirectional mute
rule and team-only voice, and the host decides only how the audio sounds.
Each side-channel packet is also checked against the host's own mute list
before playback, as a second gate.

A jitter buffer of 60 ms (3 frames, adaptive up to 200 ms) sits in front of
each remote talker's decoder. Opus packet loss concealment covers gaps.

### 3.4 Transport

Primary: direct UDP between the machines, on the same path the session
already uses. The XNet layer has each peer's address once the session is up
(see live-coop.md 10.2); the voice side channel binds its own socket on the
next port and learns peers from the same session membership.

Fallback: the session relay. Today `/v1/sessions/{id}/relay` is co-op only
and two members (crates/live-server/src/relay.rs). Voice needs:

- relay open to any session member, not only `coop` sessions;
- more than two members, up to the session's player count, with a per-message
  destination (one member, or all);
- a voice channel byte on each relayed message, so voice and co-op traffic do
  not share a rate bucket;
- `Policy::relay_rate` sized for voice: 50 packets a second per talker at up
  to 80 bytes each, inside the existing MAX_RELAY_MESSAGE.

The relay stays opaque: the server never decodes voice. It runs in both
homes: native `h2 live serve` and the Worker (Durable Object WebSocket), with
the same tests as the co-op relay.

Selection follows `coop.transport`'s rule: try direct, fall back to the relay
when no direct packet arrives within 2 s. A new `voice.transport` setting is
not needed; the voice channel uses whatever the session uses.

Nothing is stored. Live chat is never recorded on the server.

### 3.5 Voice messages

Recording: the voice-mail record thunk records from the capture device at the
rate in the WAVE header the game expects, writes RIFF/WAVE into the game's
buffer at 0x504948, and stops at 15 s or 0x499a bytes, whichever comes first.
The buffer is small: 0x499a bytes for 15 s is about 1.2 KB a second, so the
host writes the retail voice-mail format (the format the game's header check
accepts; see open question 2). Playback: decode that format to PCM and play it
through the voice output route.

Sending: MessageSend and the friend request, team recruit and game invite
paths already hand the HLE the attachment. The HLE uploads the bytes as is
with the message. The server stores the voice blob in the **private** blob
store (crates/live-server/src/blobs.rs; the data dir natively, private R2 in
the Worker) under `voice/{message_id}`, never in a public bucket. The message
event carries the blob key, size and duration, not the bytes.

Receiving: once the inbox HLE (MessageEnumerate, Details, Download) delivers
messages, Details reports the voice property, and Download streams the blob
through the existing task. Blobs are fetched with the recipient's token; the
server checks the caller is the message's recipient or sender.

Limits, as content rules: 0x499a bytes a message (the game's own limit), the
existing message rate limit, and a per-account cap on stored voice (100
messages). A voice message is deleted with its message, or after 30 days.

Converting to Opus for storage is not worth it: the blobs are already tiny,
and the game wants its own format back.

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
- The game's routing decides who gets tokens (3.3): team voice, proximity and
  the mute matrix are all its rules, unchanged.
- Mutes are bidirectional, per the game.
- The host double-checks its own mute list before playing a side-channel
  packet.
- Accounts can turn voice off ("voice allowed" account setting). The server
  then sets bit 16 in the XUID flags it issues, and the game's own checks keep
  that player out of chat and voice messages, both ways.
- `voice.incoming_muted` silences everyone locally; `voice.mic_muted` stops
  capture before encoding, so nothing leaves the machine.

What is stored: no chat audio, ever. Voice messages are stored privately and
readable only by sender and recipient.

Admins and reports:

- Reports already carry `game_id`. A voice-message report adds the message id:
  `ReportRequest.message` (optional). The report pins the blob: it is kept
  until the report is reviewed, even past 30 days or after the recipient
  deletes it.
- In the dashboard report queue, an admin with `reports.moderate` can play the
  reported voice message. Only reported messages are reachable; there is no
  admin browse of voice messages.
- Every admin playback writes a server-signed `VoiceMessageAccessed { report,
  message, by }` event, visible in the report's history.
- After review, the pin is released and normal deletion applies.

Live chat cannot be reported with evidence, because nothing is recorded. A
chat report is the existing "Screaming" or "Harassment" feedback, as today.

## 4. Integration with pause-settings

The pause-settings lane adds Video and Voice rows to the pause menu and owns
these host settings in `crates/settings` Defs:

| Setting | Read where |
|---|---|
| `voice.input_device` | capture open (3.2) |
| `voice.output_device` | playback route: headset device or "same as game" |
| `voice.mode` (open mic, or push-to-talk and its button) | VAD gate, each frame |
| `voice.mic_muted` | capture gate, each frame; also the communicator answer (2.1) |
| `voice.incoming_muted` | playback gate, each frame |
| `voice.input_volume` | capture gain, each frame |
| `voice.output_volume` | playback gain, each frame |
| per-player mute list, if it lands there | not used; the Live mute list (3.6) is the one list |

It also adds a host device-enumeration trait (`voice::Devices` or similar,
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

1. Mute list: server events, endpoints in both homes, HLE Add, Remove and
   Get, website list. Useful on its own: the game's muted icons work.
2. Communicator presence and the XHV replacement with a fake `VoiceIo`: the
   game offers voice, shows talking icons from a scripted source, and the
   presence voice flag goes out. No real audio yet.
3. Capture and playback on macOS, the Opus side channel over direct UDP.
4. Relay extension for voice in both homes.
5. Voice messages: record and play, blob storage, inbox delivery (this
   depends on the inbox HLE delivering messages at all).
6. Voice-message reports and admin playback.

## 6. Open questions

1. Whether to replace the XHV wrappers (this design) or implement the media
   object at 0x411143 and run the real engine. Replacing is less code and
   gives Opus; running the real engine would keep the 10-byte codec in the
   game's packets and need no side channel, but at about 4 kbps and with a
   codec we would have to reimplement exactly.
2. The voice-mail WAVE format the game writes and checks. The header check at
   0x53db0 and the format at 0x484290 decide what the host must produce; this
   needs one more trace before slice 5.
3. What 0x4c99b8 (the global gate) and 0x4c99a8 (value 3 blocks voice) are set
   by, and what port value 4 means in the engine calls.
4. The game's voice message type and send cadence on the wire, needed to size
   the token budget and jitter buffer.
5. Which UI reads the friend-row voice bit, and what tag 0x581 holds on game
   invites.
6. Whether split-screen players on one machine should be able to talk on
   separate devices. This design says no for now.
7. Retention: 30 days for voice messages and 100 per account are guesses for
   the user to confirm.
