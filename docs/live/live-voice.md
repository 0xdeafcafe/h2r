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

The codec is in the title (traced, 3.3): XVOICE code linked into .text
encodes and decodes. The host needs no codec of its own.

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

The XMOs resample between the device's 48 kHz and the engine's 16 kHz
16-bit mono packets of 640 bytes (traced, 3.3).

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

**Trace results** (slice 2, from the disc's default.xbe; **C** means
confirmed in the disassembly, **I** means inferred):

1. **Where the codec lives (C).** In the title. The encoder, decoder,
   voice detection and voice-mail writer are XVOICE library code linked
   into .text (0x3345xx to 0x337fxx) and run recompiled. The host provides
   only the two USB media objects.
   - Encoder 0x336d98 (pcm, sequence, out), one call per 20 ms frame, around
     the codec core 0x3377e0. From a DPC (fs:[0x58] set) it runs on the
     stack at [0x484394] with the FP state saved (0x336d39). Encoder state
     is 0x70 bytes, made at 0x3349da and set up at 0x336e7c.
   - Voice detection 0x336ea6: a 200 ms hangover (20 ms per quiet frame
     up to 0xc8). It passes 600 to the detector while quiet and 800 while
     talking (I: a level threshold).
   - Decoder side: 0x335838 splits a buffer into 10-byte frames and queues
     each with 0x336bed. Each tick, 0x336ae5 takes the frame that is due.
   - "SC.01.08.00" sits at 0x417580 beside the WAVE fourccs (I: a
     Voxware SC-family codec, version 1.8).
   - The media-object slots the engine calls: +0x04 Release (0x334bc6) and
     +0x10 Process(this, src, dst) (0x334d0c). The microphone gets a null
     src and a packet as dst; the headphone gets a packet as src. An
     XMEDIAPACKET is 24 bytes: +0x00 buffer, +0x04 max size 0x280, +0x10
     the packet index as context (0x334c43, 0x334c74). Each object has two
     packets (maxAttachedPackets 2, 0x500-byte buffers from 0x3351d7). The
     completion callbacks 0x3351a9 (mic) and 0x3351c0 (headphone) take
     (stream context, packet context, status). They act only on status 0
     and pass the packet to 0x334ca5 or 0x334d1f, which fill or drain it and
     submit it again with Process.
   - The factory 0x411143 takes (type, port, max packets, format, callback,
     context, out), calls XPP's 0x410f0c, then stores the callback at
     object +0x10 and the context at +0x14. 0x410f0c wants a rate from the
     table at 0x408890 within mask 0x1f (8 to 24 kHz). It fails with
     0x8007048f when the port has no device, 0x80070020 when the object is
     already open, and 0x8007000e when XPP's device counts (0x408888 mic,
     0x408884 headphone) are zero. The engine opens the microphone at
     0x335466 with 2 packets, the format at 0x484290, callback 0x3351a9 and
     out [port object +4]. It opens the headphone with 0x3351c0 and out
     [+8], only when [0x484390] is set (0x332f86, from the engine config).
2. **The 10-byte frame (C).** A little-endian u16 header, then 8 bytes of
   codec payload (0x336dd7 sets the payload size to 8).
   - Header = sequence << 4 | spurt. The sequence is 12 bits and wraps at
     0xfff (0x33481b). The spurt is a 4-bit counter at encoder +4; voice
     detection bumps it when talk starts again after silence (0x336f00).
   - The receiver (0x336bed) takes the sequence from the high 12 bits. When
     the low nibble changes, it resets the decoder (0x3365bb).
   - Silence is not sent at all: detection stops the frames. The frame has
     no silence marker (I: none found).
   - The game's voice record (0x55e10): byte 0 is flags and byte 1 is an
     entry count of 1 to 16. Each entry is 11 bytes (u8 talker below 2,
     then the frame) or, when relayed, 13 bytes (u8 talker, u16 forward
     mask, frame). A peer clears its own bit, plays the frame
     (0x53890(frame, 10)) and forwards the rest (0x55b40). This corrects
     1.3: the 16-entry walk is 0x55e10. 0x564c0 walks a different list
     (0x170-byte entries).
3. **The bitrate and PCM (C).** Frames are 20 ms: the DPC 0x332f4c calls
   0x332b6d(20). Ten bytes per 20 ms is 4000 bps on the wire, 3200 bps of
   it payload.
   - The microphone and headphone objects use 16 kHz 16-bit mono PCM: the
     WAVEFORMATEX at 0x484290, filled by the engine init 0x332f67 (tag 1,
     1 channel, 16000 Hz, 32000 bytes/s, align 2, 16 bits).
   - Packets are 640 bytes, which is 320 samples or 20 ms. The encoder
     copies 640 bytes a frame (0x33469f). The headphone packet is built
     from a 32-bit mix, taking the high 16 bits of each sample (0x334d1f).
   - The decoder's own output packets are 320 bytes (0x33586d). (I: the
     codec runs at 8 kHz inside, and the engine converts.)
   - A voice record carries at most 16 frames (0x55e10). (I: one frame per
     talker per tick in practice.)
4. **The voice-message WAVE header (C).** XVOICE writes it at 0x337004
   (called from 0x33482b). It is 92 bytes:
   - `RIFF`, size, `WAVE`, `fmt ` with chunk size 64.
   - Format tag 0x000a, 1 channel, 8000 Hz, 625 bytes/s (5 kbps), the
     codec's block align, 16 bits, cbSize 46.
   - 46 bytes of codec parameters, then `data` and its size.

   0x3370ca sets 8000 Hz, 1 channel, 625 bytes/s and a parameter of 5
   before it opens the voice-mail codec (0x337ece). 0x53db0 checks only
   `RIFF`, `WAVE`, `fmt ` and a length over 16.

   0x484290 is not the message format: it is the headset's 16 kHz PCM
   (item 3). (I: property 0x3c2's value 1 is Live's codec id for this
   format, not the WAVE tag, which is 0x000a.)

   (I: 15 s at 625 bytes/s is 9375 bytes plus the 92-byte header, well
   under 0x499a. 0x499a is 92 bytes plus 30 s.)
5. **Lost and late frames (C).** 0x336384 places a frame by its sequence,
   in a window worked out from the clock (0x320d00 divides the time by the
   frame period at +0x0c). Frames outside the window are counted at
   +0xa4; enough of them in a row reset the timeline to the new sequence
   (0x336a85). Old frames from the same spurt are counted at +0x88.

   Each tick, 0x336ae5 takes the frame that is due. If it is missing, the
   previous frame is played once more with its sequence plus one (flag
   +0xa0, 0x336b23). After that there is nothing, so silence, until frames
   come again.

   The engine keeps its own time, so the host must complete headphone and
   microphone packets on a steady 20 ms cadence and need not report loss.
6. **The gates.**
   - 0x4c99b8 (C) is set to 1 at 0x5327f, at the end of the game's voice
     init 0x53210 (called from the game init at 0x8dd36). Every voice
     wrapper checks it, together with 0x476fcc.
   - 0x4c99a8 (C) is one dword per controller, written by 0x54fc0 and read
     by 0x54f90. 0x536b0 refuses voice when it is 3. (I: 3 means voice is
     off or banned for that player.)

7. **More from a second, independent read of the same code** (it agrees
   with items 1 to 6 wherever they overlap):
   - **Remote voice reaches the headphone through DirectSound (C).** The
     decoder plays each remote talker into a DirectSound stream:
     DirectSoundCreateStream at 0x335ed3, PCM mono 8000 Hz, two 320-byte
     packets, completion 0x335d82 or 0x335d99. Its mixbins are the
     per-port voice effect, DSP effect 0xb + port (0x333330). The
     headphone feed 0x334d1f does not read the decoder. It reads that
     effect's output ring through 0x332c9c (map +0x18 base, +0x1c size,
     state +0x34 write position), keeps the high 16 bits of each 32-bit
     sample, and fills the 640-byte packet. If less than 0x500 bytes are
     ready, it moves the read position to write − 0x280. So remote voice
     reaches a headphone object only if the host's DirectSound runs those
     effects and their output rings. That is a slice 4 dependency on
     crates/dsound; the slice 3 objects do not depend on it.
   - **The XMO vtable is at 0x417544 (C).** Slots: AddRef 0x41058f,
     Release 0x4112dc, GetInfo 0x4105ab, GetStatus 0x410603, Process
     0x410e77, Discontinuity 0x4105fe (S_OK) and Flush 0x4105f6
     (E_NOTIMPL). The engine calls only Release and Process.
   - **Process queues a 24-byte copy of the packet (C).** Process
     (0x410e77) marks the packet pending (completed size 0, status
     0x8000000a, at 0x37f38a). 0x37f3a8 later writes the completed size
     and the status through the packet's pointers, when they are set, and
     calls callback(context, packet +0x10, status). With no callback, it
     sets the event at +0x10 instead.
   - **Encode and decode limits (C).** At most 2 encodes and 15 decodes a
     tick (0x33355f, 0x3335e0). Up to 5 frames are buffered per talker.
     DoWork 0x334fd2 hands each frame to the title as (port, 10, frame).
   - **The voice-mail codec is a second, larger codec (C).** Its encoder
     is set up at 0x3370ca and encodes at 0x339a62. It averages 16 kHz
     down to 8 kHz first (0x336f26). Its parser 0x33739f checks every
     header field: tag 0xa, 1 channel, 16 bits, cbSize 0x2e, and data
     size = RIFF size − 0x54. The server's header check (3.5) should
     match 0x33739f, not just the game's looser 0x53db0. Recording length
     is rounded down to 20 ms (0x335017).
   - **Codec names (I).** Tag 0x000a is WAVE_FORMAT_WMAVOICE9 in mmreg.h,
     so the voice-mail codec may be WMA Voice. That would make item 1's
     Voxware guess apply to the chat codec only, if at all.
   - **Who writes 0x4c99a8 (C, I).** 0x54fc0 is called from 0x18fbfc,
     with the value from profile +0x14c, and from the settings screen at
     0x2c600b. So 3 is most likely the profile's voice-off option (I).
   - **Still untraced.** How 0x765c0 fills its 0x200-byte budget; what
     the talker byte below 2 means; the jitter delay (0x3367bd, arguments
     10 and 0x20); the final block align from 0x337ece.

What this means for slice 3: no codec work. The host implements the
factory 0x411143 as two media objects per port with a vtable whose
Process takes 640-byte 16 kHz packets and calls the stored callback with
status 0 when one completes. The XPP code under it (0x410f0c) stays
unused.

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

Built (slice 1). Server: a per-account mute list as private events,
`PlayerMuted { by, target }` and `PlayerUnmuted { by, target }` on
`mutes-<by>`, written by the server from authenticated requests, readable
only by the owner, admins and the operator. Capped at 250 entries, the
game's own Get size. Endpoints ([live-api.md](live-api.md) "Mute list"):

- `GET /v1/accounts/by-xuid/{xuid}/mutes` (a `read` token)
- `PUT /v1/accounts/by-xuid/{xuid}/mutes/{target}` (scope `mutes.write`)
- `DELETE /v1/accounts/by-xuid/{xuid}/mutes/{target}`

The server stores the plain 8-byte XUID. The HLE keeps the flags the game
passed with each Add, so Get hands back exactly the 12 bytes Add was given.
For mutes this machine did not add (a list loaded at sign-in, or a mute made
on another machine), the flags are zero, which is what every h2 live
account's XUID carries.

HLE ([live.md](live.md) "Client HLE: mute list"):

- Startup (`(0, &task)`) loads each signed-in player's list once per
  sign-in; Get fills the buffer and count from the cached copy, or when it
  arrives.
- Add and Remove: update the cache at once and send the server write. They
  return S_OK straight away, as the game does not wait on a task. A failed
  write is sent again before the next load.
- When the cache changes, the game rebuilds its matrix the next time it calls
  Get; the wrappers at 0x58c10/0x58c60 already do that.

The dashboard shows the list on the signed-in player's own screen, with
Unmute on each, and Mute or Unmute on any other player's screen
(`web/src/dash/Mutes.tsx` over `web/src/admin/mutes.ts`), so a player can
review and change mutes outside the game. A change there reaches the game at
its next sign-in.

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
   and Get, and the dashboard list (built). Useful on its own: the game's
   muted icons work, and it needs nothing from the codec.
2. **Trace the codec** (3.3, "Trace results"): where it lives, the frame
   layout, the bitrate, the PCM format and the voice-mail WAVE header. No
   code (done: the codec is in the title).
3. **Communicator presence and the media objects with a fake `VoiceIo`**
   (built). The pieces:
   - **Device types.** `xapi::DeviceTypes::voice` makes the microphone and
     headphone types (0x4088b0, 0x4088bc) answer XGetDevices, XPeekDevices
     and XGetDeviceChanges from `Xapi::set_voice_ports`, with a change
     state per type. Boot sets the types with the mask at 0, so the game
     sees no communicator, as before.
   - **The `voice` crate.**
     - `xmo::MediaObjects` holds the objects: the original's creation
       checks and HRESULTs, Process copying the caller's packet and
       marking it pending, and a 20 ms tick. On each tick the talker's
       port gets the host microphone (the other ports get silence), and
       every headphone object mixes into one output.
     - `resample` converts between 48 kHz and 8, 16 or 24 kHz.
     - `io::FakeVoiceIo` scripts the input and records the output.
     - `hle::register` overrides 0x411143, Process (0x410e77) and Release
       (0x4112dc). It returns the original's static object slots with the
       original vtable. It also wraps the engine DPC 0x332f4c: tick, run
       each completion's callback, then run the original DPC.
   - **Tests.** Tests against the fake devices and through the real
     runtime Runner, with stand-ins for the DPC and the callbacks.
   - **Not done yet.** Running the real engine end to end needs the game,
     so it waits for slice 4 and a user run.
4. **Capture and playback on macOS**, through the pause-settings device
   trait and settings; push-to-talk and mic mute set the talker
   (`MediaObjects::set_talker`). This slice also:
   - registers `voice::hle` in boot;
   - sets the voice port mask from host mic presence;
   - checks that the host's DirectSound runs the per-port voice effects
     and fills their output rings (3.3, item 7), since remote voice
     reaches the headphone only through them.
5. **Voice messages**: the voice-mail mode on the same objects, blob storage
   with the size and header check, inbox delivery (this depends on the
   inbox HLE delivering messages at all).
6. **Voice-message reports and admin playback.**

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
