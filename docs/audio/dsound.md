# DirectSound (guest side)

`crates/dsound` is the guest-facing half of audio: it takes the calls Halo 2
makes into its statically linked Xbox DirectSound (`DSOUND` section,
`0x00379c60`..`0x00387374`, XDK 5849) and answers them from Rust. It reads
and writes guest memory through a small trait and drives `crates/audio`
(ADPCM, mixer, cpal), which is described in [audio.md](audio.md). Only
`register.rs` depends on `crates/runtime`. Nothing depends on the kernel,
which is reached through the `register::Events` trait.

All addresses below are for the retail 5849 `default.xbe`. The call-site
survey was done on the disassembly (`h2 disasm`, `h2 analyze --functions`,
`h2 symbols --list` with the XbSymbolDatabase from
`scripts/setup-xbsymboldb.sh`): every `call`/`jmp rel32` in the other code
sections whose target lies inside `DSOUND`, plus every 32-bit word in the
data sections that points into it (vtables).

## What the game reaches

Halo 2 never calls a `CMcpx*`, `CHRTF*` or `CDirectSound3DCalculator`
function directly. Every edge from game code (sound core `0x21e4b0`..
`0x2229d0`, the `0x2ae...` and `0x333...`..`0x335...` stream users, `0x1913..`
effects setup, Bink's audio at `0x3e3...`) into `DSOUND` lands on one of the
flat `IDirectSound*_` wrappers below, on a flat stream thunk, or goes through
the stream vtable. Those are the 59 matched entry points; 4 more reached
addresses have no name (see "Unmatched").

Calling convention: **every one of them is plain `stdcall`** (arguments on
the stack, `this` first, callee pops with `ret N`, result in `EAX`). No LTCG
register arguments appear on any entry: the thunks only shuffle EAX/ECX/EDX
internally to adjust `this` (`this - 8` for `IDirectSound`, `this - 0x1c` for
`IDirectSoundBuffer`, null stays null) before calling the `CDirectSound*`
method, which is also `stdcall`. Floats are passed as their IEEE bit
pattern in a stack dword (`SetDistanceFactor` is called with `0x4043126f`,
3.048). Because the replacements sit at the flat-wrapper addresses, the
pointer adjustment is irrelevant: whatever value `ppX` received is what comes
back as `this`.

`args` below counts stack dwords including `this`; the callee pops `4 * args`.
`calls` is the number of call sites outside `DSOUND`.

| address | symbol | args | calls | call sites |
|---|---|--:|--:|---|
| `0x379f2a` | `IDirectSound_Release` | 1 | 6 | 21eb8c 333466 3339c7 334232 3e39f8 3e3d14 |
| `0x379f40` | `IDirectSoundBuffer_Unlock` | 5 | 2 | 3e3304 3e36bf |
| `0x379f45` | `IDirectSoundBuffer_Release` | 1 | 3 | 21eb53 21eb68 3e39c0 |
| `0x379f5b` | `DirectSoundUseLightHRTF4Channel` | 0 | 1 | 21e5ff |
| `0x37b5ae` | `IDirectSound_GetCaps` | 2 | 1 | 21e536 |
| `0x37b5ca` | `IDirectSound_GetSpeakerConfig` | 2 | 1 | 21e590 |
| `0x37b5e6` | `IDirectSound_GetEffectData` | 5 | 5 | 1913d0 1913e6 1913fb 191411 191451 |
| `0x37b60d` | `IDirectSound_SetEffectData` | 6 | 10 | 19128f 1912a7 1912be 1912d6 191581 19159d 1915b9 1915d4 21fa74 333ad9 |
| `0x37b637` | `IDirectSound_SetMixBinHeadroom` | 3 | 4 | 21e5df 21f4ba 21f4d8 33390a |
| `0x37b657` | `IDirectSound_SynchPlayback` | 1 | 1 | 21f485j |
| `0x37b66f` | `IDirectSoundBuffer_SetVolume` | 2 | 13 | 21e9e9 21f19b 21f88f 21ffab 220c32 2215a7 2aec90 2aeca1 2aede2 2aedf3 2aef2d 2aef3e |
| `0x37b68b` | `IDirectSoundBuffer_SetFilter` | 2 | 3 | 221477 2aefb7 2aefc8 |
| `0x37b6a7` | `IDirectSoundBuffer_SetHeadroom` | 2 | 2 | 221598 3e318e |
| `0x37b6c3` | `IDirectSoundBuffer_SetMixBinVolumes` | 2 | 2 | 2217d2 3e3253 |
| `0x37b6df` | `IDirectSoundBuffer_Play` | 4 | 9 | 21e83d 21e8b5 21ea24 220028 220cb0 220db4 2215b4 3e35d6 3e3634 |
| `0x37b703` | `IDirectSoundBuffer_Stop` | 1 | 3 | 3e389c 3e3901 3e39ad |
| `0x37b71b` | `IDirectSoundBuffer_StopEx` | 4 | 1 | 220052 |
| `0x37b73f` | `IDirectSoundBuffer_Pause` | 2 | 4 | 21f581 21f5b1 21f795 21f86f |
| `0x37b75b` | `IDirectSoundBuffer_GetStatus` | 2 | 4 | 1296aa 22000e 22003b 3e38ad |
| `0x37b777` | `IDirectSoundBuffer_GetCurrentPosition` | 3 | 6 | 3e331f 3e332f 3e333f 3e35f2 3e3608 3e3650 |
| `0x37b797` | `IDirectSoundBuffer_SetCurrentPosition` | 2 | 2 | 21f887 3e38bc |
| `0x37b7b3` | `IDirectSoundBuffer_Lock` | 8 | 3 | 21f7b2 3e328e 3e3573 |
| `0x37b7e3` | `IDirectSoundBuffer_GetVoiceProperties` | 2 | 2 | 21ff51 21ff62 |
| `0x37b7ff` | `IDirectSoundStream_SetVolume` | 2 | 1 | 21fc09 |
| `0x37b804` | `IDirectSoundStream_SetPitch` | 2 | 1 | 2201c4 |
| `0x37b809` | `IDirectSoundStream_SetLFO` | 2 | 4 | 220508 22065f 2206d0 220704 |
| `0x37b80e` | `IDirectSoundStream_SetEG` | 2 | 1 | 2ae812 |
| `0x37b813` | `IDirectSoundStream_SetFilter` | 2 | 2 | 2203e9 220698 |
| `0x37b818` | `IDirectSoundStream_SetHeadroom` | 2 | 3 | 2ae69e 335c06 335ee2 |
| `0x37b81d` | `IDirectSoundStream_SetMixBinVolumes` | 2 | 4 | 2216b2 333b63 335b89 335bc3 |
| `0x37b822` | `IDirectSoundStream_Pause` | 2 | 4 | 21f45c 21f638 21f8de 2ae4d3 |
| `0x37b827` | `IDirectSoundStream_FlushEx` | 4 | 5 | 2ae4e2 333ef6 335744 33598d 335d13 |
| `0x37b83f` | `IDirectSoundStream_GetVoiceProperties` | 2 | 1 | 21fb38 |
| `0x37b844` | `DirectSoundDoWork` | 0 | 4 | 21ec37 33574c 335be4 335bfc |
| `0x37b86d` | `XAudioDownloadEffectsImage` | 4 | 2 | 1913a4 191647 |
| `0x37ba6f` | `XAudioSetEffectData` | 3 | 2 | 21ee6f 21fa07 |
| `0x37c5c8` | `IDirectSoundBuffer_SetFrequency` | 2 | 1 | 3e314e |
| `0x37c5e4` | `IDirectSoundBuffer_SetMixBins` | 2 | 9 | 21ea16 220b24 220ca5 220d7e 2217f7 2aec82 2aedd4 2aef1f 3e321a |
| `0x37c600` | `IDirectSoundBuffer_SetConeOutsideVolume` | 3 | 1 | 220b69 |
| `0x37c620` | `IDirectSoundBuffer_SetMaxDistance` | 3 | 3 | 21e898 220b39 220d96 |
| `0x37c644` | `IDirectSoundBuffer_SetMinDistance` | 3 | 3 | 21e8a7 220b48 220da5 |
| `0x37c668` | `IDirectSoundBuffer_SetPosition` | 5 | 1 | 220ea1 |
| `0x37c69d` | `IDirectSoundBuffer_SetDopplerFactor` | 3 | 1 | 220b5e |
| `0x37c6c1` | `IDirectSoundBuffer_SetRolloffFactor` | 3 | 1 | 220b53 |
| `0x37c6e5` | `IDirectSoundBuffer_SetI3DL2Source` | 3 | 2 | 221422 2aef65 |
| `0x37c705` | `IDirectSoundStream_SetOutputBuffer` | 2 | 2 | 21f911 21f923 |
| `0x37c70a` | `IDirectSoundStream_SetMixBins` | 2 | 6 | 220136 2216d7 333f7e 335a73 335b1c 335cba |
| `0x37cc4a` | `IDirectSoundBuffer_SetBufferData` | 3 | 2 | 22158d 3e3bb8 |
| `0x37d141` | `IDirectSound_CommitDeferredSettings` | 1 | 1 | 21f1fc |
| `0x37d4be` | `IDirectSound_CreateSoundBuffer` | 4 | 7 | 21e82b 21e885 21e9db 220ac3 220c21 220d1d 3e3b92 |
| `0x37d4e2` | `IDirectSound_CreateSoundStream` | 4 | 1 | 2ae68d |
| `0x37d506` | `IDirectSound_SetDistanceFactor` | 3 | 1 | 21e556 |
| `0x37d52a` | `IDirectSound_SetDopplerFactor` | 3 | 1 | 21e5fa |
| `0x37d54e` | `IDirectSound_SetOrientation` | 8 | 1 | 22088c |
| `0x37d598` | `IDirectSound_SetPosition` | 5 | 1 | 2207ba |
| `0x37d5cd` | `IDirectSound_SetRolloffFactor` | 3 | 1 | 21e572 |
| `0x37d797` | `DirectSoundCreate` | 3 | 4 | 21e516 33334f 3338b0 334167 |
| `0x37d7de` | `DirectSoundCreateBuffer` | 2 | 1 | 221539 |
| `0x37d835` | `DirectSoundCreateStream` | 2 | 1 | 335ed3 |

Notes on the table:

- `args` for `IDirectSoundStream_*` flat entries are taken from the
  `jmp` target (`CDirectSoundStream_*`) and the XDK signatures; the thunks
  themselves are bare `jmp`s. `XAudioSetEffectData` is not a thunk: it is a
  real function at `0x37ba6f` that returns with `ret 0Ch` (at `0x37bc6b`), so
  it takes 3 stack arguments (Halo's calls at `0x21ee6f`/`0x21fa07` push
  three).
- `IDirectSoundBuffer_Unlock` is verified as `xor eax,eax; ret 14h`: on Xbox
  `Unlock` does nothing and the buffer memory is the guest's.
- `IDirectSound_Release`, `IDirectSoundBuffer_Release` call `[vtable+8]`
  (slot 2, `Release`) on the adjusted pointer.

### Unmatched DSOUND entries that are reached

| address | from | what it is |
|---|---|---|
| `0x37c20a` | `0x3321c8` (`sub ecx,4; jmp`) | secondary-vtable adjustor thunk (vtable at `0x417160`) |
| `0x37f0b8` | `0x3321d0` (`sub ecx,8; jmp`) | same, vtable at `0x4171fc`/`0x417200` |
| `0x37f38a`, `0x37f3a8` | `0x410eb5`, `0x410ddc`, `0x410e5e`, `0x410ef4` (`XPP`); also `0x37f850` (stream flush) | the XMEDIAPACKET completion routine. These ARE on the sound path (stream `Flush` reaches `0x37f3a8` from `0x37f850`); they are shared with USB/voice code. See "Packet completion" below |

The `0x3321xx` thunks live in `.text` and are only reached through a vtable.
Nothing else needs a decision until a path that uses them shows up.

### Called through vtables, not flat functions

`IDirectSoundStream` is an XMO. The game calls `AddRef`, `Release`,
`GetInfo`, `GetStatus`, `Process`, `Discontinuity` and `Flush` as virtual
methods. The static vtable is at `0x00417170`, 7 slots in XMO order (checked
by `ret N` and body of each):

| slot | address | method | stack args |
|--:|---|---|--:|
| 0 | `0x37ab40` | `AddRef` | 1 (increments `[this+8]`) |
| 1 | `0x37ab87` | `Release` | 1 |
| 2 | `0x37abd5` | `GetInfo(XMEDIAINFO*)` | 2 |
| 3 | `0x37acd4` | `GetStatus(DWORD*)` | 2 |
| 4 | `0x37ad25` | `Process(src, dst)` | 3 |
| 5 | `0x37ac3c` | `Discontinuity()` | 1 |
| 6 | `0x37ac89` | `Flush()` | 1 |

The stream object's first dword is that vtable pointer, so the shim writes
`Layout::stream_vtable` (default `0x417170`) at offset 0 of every stream it
creates; the override layer replaces the seven slot targets. The real object
keeps its refcount at `+8` and a `CMcpxStream*` at `+0x24`; the shim keeps
all state host-side, so only the vtable word is guest-visible.
Other vtables in `.rdata` (`0x4170e0`..`0x4172c8`, one per `CDirectSound*`
and `CMcpx*` class) are not reached by the game.

## Guest objects

- **`IDirectSound`**: a singleton. `DirectSoundCreate(pguid, ppDS, pOuter)`
  (`ret 0Ch`) creates it, later calls AddRef the same object. Returned
  pointer is `object + 8` in the original; the shim returns an opaque
  guest-allocated block instead.
- **`IDirectSoundBuffer`**: `CreateSoundBuffer(this, DSBUFFERDESC*, ppBuf,
  pOuter)`. Descriptor (6 dwords, `dwSize = 0x18` verified at every call
  site): `dwSize, dwFlags, dwBufferBytes, lpwfxFormat, lpMixBins,
  dwInputMixBin`. Two shapes appear in Halo 2:
  1. flags `0x100000`, `0x100010`, `0x2010`, no format, `dwBufferBytes = 0`,
     `dwInputMixBin` set to bins `0x17..0x19` (init `0x21e82b`/`0x21e885`,
     sound core `0x220ac3`, `0x220c21`): mix-in/FX buffers that carry no
     samples. `CMcpxBuffer_Play` forces looping when `flags & 0x182000`.
  2. standalone `DirectSoundCreateBuffer` (`0x221539`): format set, data
     supplied with `SetBufferData(buf, pv, bytes)` pointing into the game's
     own memory (allocated from its arena at `0x510800`), then
     `SetHeadroom(0)`, `SetVolume(-10000)`, `Play(0,0,flags)`.
- **`IDirectSoundStream`**: `CreateSoundStream(this, DSSTREAMDESC*, ppStream,
  pOuter)`; descriptor `{ dwFlags, dwMaxAttachedPackets, lpwfxFormat,
  lpfnCallback, lpvContext, lpMixBins }`. The stream at `0x2ae68d` ORs
  `0x20000000` into the caller's flags; the one at `0x335ed3` uses
  `0x40000000`. The shim stores flags and does not interpret them.
- **`XMEDIAPACKET`**, 6 dwords (verified: `CMcpxStream_Process` copies six
  dwords, zeroes completed size, writes `E_PENDING`):
  `pvBuffer, dwMaxSize, pdwCompletedSize, pdwStatus, hCompletionEvent |
  dwCompletionContext, prtTimestamp`.
- **`WAVEFORMATEX`**: `wFormatTag u16, nChannels u16, nSamplesPerSec u32,
  nAvgBytesPerSec u32, nBlockAlign u16, wBitsPerSample u16, cbSize u16`.
  Xbox ADPCM is tag `0x69`, 4 bits, `nBlockAlign = 36 * channels`, with a
  trailing `wSamplesPerBlock` (64) the shim does not read.
- **`DSMIXBINS`**: `{ dwMixBinCount, lpMixBinVolumePairs }`, pairs
  `{ dwMixBin, lVolume }` (verified: Halo passes count 5 with bins 5..10).

### Positioned sound

Halo 2 positions sound through mix-in buffers, not through 3D streams. Its
sound core (`0x220b90`, with `0x220a70`) creates a `DSBCAPS_CTRL3D` mix-in
(flags `0x80010`, `0x100010` or `0x2010`, no data) whose `dwInputMixBin` is
an FX send (`0x17` and up). The mix-in's own mix bins are the default 3D set
`{6, 8, 7, 9, 10}`; its min and max distance are `FLT_MAX`, rolloff, doppler
and cone outside volume 0. A stream (music, dialogue) is routed into the
mix-in by `SetMixBins`/`SetMixBinVolumes` on that send bin. `SetPosition`
at `0x220ea1` (to `IDirectSound3DBuffer_SetPosition`, `0x37c668`) moves the
mix-in. Halo also calls `GetStatus`, `Play` and `StopEx` on mix-ins.

The shim gives every mix-in buffer an `audio::MixIn` on its input bin
(removed on `Release`). Every buffer and stream gets its mix bins from
creation and from `SetMixBins`/`SetMixBinVolumes`; position, distances,
cone, rolloff and the listener reach `audio` through `refresh_3d`. A stream
sending to a playing mix-in is heard panned by the mix-in's position against
the listener (the model is in `docs/audio/audio.md`, "Positional model"); a mix-in
that is not playing, or a send bin no mix-in owns, is silent, as an unused
FX send was on the MCPX. With Halo's parameters (rolloff 0, `FLT_MAX`
distances) the shim's own distance curve is unity: Halo attenuates by
distance itself, through the voice's mix-bin volumes (below), which is what
the hardware did.

### Distance attenuation

Halo 2 computes every gain itself and hands DirectSound millibels; nothing
in the 3D API attenuates (rolloff 0, `FLT_MAX` distances, no listener
rolloff). Per sound channel the update at `0x21fa80` (called from the sound
source update `0x128ca0`, whose spatial inputs `0x12a1b0` copies into the
channel's parameters) takes three paths:

- **Plain streams** (no submix): `SetVolume` with the channel gain, and
  `SetMixBins` (`0x220136`) with pairs from the panner object
  (`[sound globals+0x2ab4]`, vtable methods `0x2af7b0`, `0x2af800`,
  `0x2af920`, `0x2afb80`). The panner sums the channel's dB terms per
  speaker (left and right terms, an overall positional term, a submix term)
  and converts each with `0x12aff0` (clamp to -64..0 dB) to `100·dB` mB.
  Speaker bins are `{0, 1, 4, 5}` for a sound flagged for the speakers,
  otherwise the crosstalk bins `{6, 7, 8, 9}`; bin 10 (I3DL2) carries the
  reverb send.
- **Streams in a submix** (a 3D mix-in from the table at `+0x1a18`, given by
  `SetOutputBuffer` at `0x21f911` when the channel starts): the stream's
  `SetVolume` is 0 and the gain goes to the send bin through
  `SetMixBinVolumes` (`0x2215d0`), or the panner's pairs arrive through
  `SetMixBins` alongside the send.
- **Filters**: `0x2201f0` evaluates the sound's filter curves and calls
  stream `SetFilter` (`0x2203e9`) and `SetLFO`; the submix update
  `0x2211e0` sets the mix-in's `SetI3DL2Source` (obstruction and
  occlusion) and `SetFilter`.

So the attenuation lives in mix-bin volumes, and those must survive
`SetOutputBuffer`. The library keeps a voice's bin list and a per-bin
volume table apart (`crates/dsound/src/mixbins.rs`, from `0x37bc89`
`SetMixBins`, `0x37a4db` `SetMixBinVolumes`, `0x37bd54` `SetOutputBuffer`
and `0x37a500`): `SetOutputBuffer(mix-in)` replaces the list with the
mix-in's input bin at `1 / channels`; a later `SetMixBins` routes the
given pairs and adds the send back; `SetMixBinVolumes` changes volumes
only, never the list; null takes the send out and keeps the rest. The shim
used to send such a stream to the mix-in alone, at unity, so a sound in a
submix played at full level and unpanned however far away it was.

### Notification and callback contracts

- **Packet completion.** `Process` writes `E_PENDING` (`0x8000000A`) to
  `*pdwStatus` and 0 to `*pdwCompletedSize` immediately (verified). When the
  mixer finishes the packet, the shim writes the completed size (the whole
  packet) and `XMEDIAPACKET_STATUS_SUCCESS` (0). A flush completes pending
  packets with `XMEDIAPACKET_STATUS_FLUSHED` (`E_ABORT`, `0x80004004`). The
  completed size written is always the packet's original `dwMaxSize`, even
  for an untouched flushed packet (verified: `0x37f850` calls `0x37f3a8`
  with the size from the packet record).
- **Completion signal (verified, `0x37f3a8`).** The routine writes the
  completed size and status, then, if the stream has a callback, calls
  `lpfnCallback(lpvContext, packetContext, status)` (stdcall); otherwise it
  signals the event handle in the fifth dword through the kernel import at
  `0x411640` with `(handle, 0)`.
- **Ordering.** Packets complete in submission order, including empty ones:
  an empty packet queued behind a pending packet waits for it (and a later
  flush aborts it). `Process` on a stream
  that is not ready returns `DSERR_INVALIDCALL` (`0x88780032`, verified:
  `Process` tests `GetStatus` bit 0 and substitutes that code). The shim
  reports "not ready" once `dwMaxAttachedPackets` are outstanding.
- **Who runs it.** The real library completes packets from the APU
  interrupt/DPC. The shim has no thread of its own by default, so a
  completion is written wherever the game can observe it: `Process` and
  stream `GetStatus` collect first (a game that polls capacity or status sees
  the interrupt's result), `Flush`/`FlushEx` and zero-length packets complete
  at once, and `DirectSoundDoWork` collects everything. The mixer is
  what consumes packets, so something must render it. See "Audio clock".
  Events are `EventSink::signal_event(handle, token)`, and callbacks are queued
  with `EventSink::stream_callback(StreamCallback)`. Through `register`,
  events reach the kernel after the guards drop. Callbacks run as guest
  calls before the override returns (see "Runtime registration"), or, for
  completions a host render produced, from the completion DPC (below).
- **The completion DPC.** On the Xbox a stream callback runs from the APU's
  DPC as soon as its packet completes. `Hle::completion_dpc()` is a kernel
  periodic-source handler that does the same on logical time: while
  callbacks are queued it calls `DirectSoundDoWork()`
  (`register::COMPLETION_DPC_TARGET`, `0x37b844`) on the DPC stack, whose
  override runs every queued callback, in order, before it returns; with
  none queued it calls nothing. `boot::dsound_glue` adds it at one period
  per audio quantum (`COMPLETION_RATE`, 187.5 Hz) from the output's own
  origin, so its deadlines are the grid's. Those deadlines also stop the
  scheduler's idle advance at every quantum, so the runner renders at each
  grid point and a callback runs at most two quanta (10.7 ms) after its
  packet completed, whatever the game's frame rate. Before this, a
  callback waited for the game's next DirectSound call on a guest thread:
  Halo makes those in its once-a-frame sound update, so a voice stream's
  refill came up to a frame and more after the completion, past the one
  20 ms packet it had left, and the stream starved about every packet.
  Events and polled statuses were already current when the game looked:
  they do not depend on this.
- **Order of effects (verified, `0x37f3a8`).** Completed size and status are
  written to guest memory first, then the event is signalled or the callback
  queued. Completion order is submission order. Neither happens while a guest
  memory or `DirectSound` guard is held.
- **Event references.** A packet queued with an event (stream without a
  callback) calls `EventSink::retain_event(handle, token)`, where `token` is unique to
  the packet. Through `register` that
  becomes `Events::reference(handle)`, which a kernel sink implements with
  `Kernel::reference_dispatcher` so the completion still reaches the event if
  the guest closes its handle first. Exactly one `signal_event` follows per
  retained packet, or one `release_event` if the stream is released with the
  packet still pending (the library drops those silently, leaving the status
  `E_PENDING`). Pins are matched by packet token, not by handle or queue
  position, so a packet whose `reference` failed (it has no pin) can never
  consume or drop another packet's pin: it falls back to `Events::signal(handle)`,
  as does any sink that does not pin.
- **Delivery order.** The shim collects each call's event operations
  (retain, signal, release) as one batch and `register` publishes the batch to
  an ordered outbox while the `DirectSound` lock is still held, so batches
  are ordered like the state changes that produced them. They are drained after
  every guard is dropped, one drainer at a time (the pump cannot signal a
  packet before the `Process` that queued it has pinned it, and a concurrent
  close cannot lose a notification).
- **Completion writes are retried, not lost.** A packet stays at the head of
  its stream until its size and status writes succeed. A guest fault in either
  write leaves the packet (and its pin) queued; the next `Process`,
  `GetStatus` or `DoWork` completes it.
- **Buffers have no notifications in Halo 2.** `SetNotificationPositions`,
  `SetPlayRegion` and `SetLoopRegion` are not matched and have no call sites.

### How Halo feeds its streams

Two feeders, read from the disassembly:

- **Sound core streams** (music and dialogue, created at `0x2ae68d`) are
  polled. The once-a-frame sound update (`0x21ee80`, from the frame at
  `0x125b40`) walks the streams (`0x21f22f` → `0x2ae820`): while stream
  `GetStatus` reports room, it asks the source (vtable `0x45711c`, slot 3,
  `0x2ae290`) for a packet, `Process`es it, then calls `Discontinuity`.
  The source keeps two packet slots in one `0x20000`-byte block and reads
  their status words directly; a slot is free once its status is no longer
  `E_PENDING`. The packets carry no event and the stream no callback. The
  packet size is the word at `0x47f0e0`: `0x10000`, or `0x4000` unless
  `[[0x4e6948]+8] == 3` (`0x125797`). At 44.1 kHz stereo PCM16, 16 KB is
  93 ms, so two packets ride out a poll that comes a frame late, but not
  two frames late at 30 Hz with jitter. A source with data waiting also
  feeds at once from the read path (`0x22077d`).
- **Voice streams** (`DirectSoundCreateStream` at `0x335ed3`, flags
  `0x40000000`) are 8 kHz mono PCM16, `dwMaxAttachedPackets = 2`, and are
  refilled only from their completion callback: `0x335d82` → `0x335d38`
  submits the next 320-byte (20 ms) packet; `0x335d99` → `0x3358e5`
  rotates three 320-byte packets and decodes more every third. The
  callback must run within 20 ms of a completion or the stream starves.
  The rest of that code runs from a kernel timer DPC (`KeSetTimer` at
  `0x333c28` and `0x333e3b`, 1 ms or `19 - n` ms) and raises IRQL around
  its stream state, as code whose callbacks come at DPC level must.

## Audio clock

Buffers only finish, and stream packets only complete, when the mixer is
rendered. Exactly one consumer may render a given mixer, or audio time runs
twice. The shim enforces it among its own, under the `DirectSound` lock.
A thread instance (the output thread, a render pump) takes an exclusive
`Lease` (`DirectSound::lease_consumer`): it is refused while anything holds
the mixer, including another thread of the same kind, and only that lease
gives it back, so dropping one handle never frees the mixer under another.
The shared kinds (`DoWork` for `set_frames_per_do_work`, `Manual` for
`advance_audio`/`tick`) use `claim_consumer`, which the same kind may
repeat. Every refused path reports `ConsumerConflict { held, wanted }`
(`spawn_output`, `spawn_pump(.., true)` and `set_frames_per_do_work` return
it, a manual `advance_audio` or `tick` is a `Stop::Consumer`). It cannot see a
renderer outside the crate (`crates/app`'s `AudioOut`/`DiscardRenderer`, or
an `audio::Pump` of the caller's own): pick one.

| consumer | who renders | who reports completions to the game |
|---|---|---|
| `Hle::spawn_output`, device pacing (the default for a runner, see below) | its thread, into a sink (a device) | the same thread, each wake, and the game's own `DoWork`/`GetStatus`/`Process` |
| `Hle::spawn_output`, logical pacing (deterministic runs) | `Output::advance_to`, called by the runner at its scheduler points; its thread only plays the result | `advance_to`, and the game's own calls |
| `crates/app`'s `AudioOut` (legacy) | the app | `Hle::spawn_pump(runtime, period, false)` or the game's calls |
| nothing (headless boot, tests) | `Hle::spawn_pump(runtime, period, true)`, or `DirectSound::set_frames_per_do_work`, or `Hle::tick`/`advance_audio` by hand | the same call |

### Consumers and the output thread

`Hle::spawn_output(runtime, open, OutputConfig) -> Result<Output,
ConsumerConflict>` leases the mixer as `Consumer::Output` and starts a
thread. `open` runs on it (a `DeviceSink` is not `Send` on macOS) and
returns the `audio::AudioSink` to play into. Who renders depends on
`Pacing`; completion order does not: each completed packet's size and
status reach guest memory, in submission order, before its event is
published, events are delivered with no guard held, and stream callbacks
(guest calls) run on a guest thread, in order, at the next override call or
from the completion DPC (see "Notification and callback contracts").

- `Device { target_latency_ms }` (default 40). Each wake
  (`OutputConfig::period`, default 5 ms) the thread renders enough to keep
  that much queued in the sink (`audio::Pump`), under the mixer lock alone,
  then runs `Hle::do_work`. Audio time is the device's, which is real time,
  and the mixer follows the sink's rate, including after a device change.
  When a packet completes relative to the game's own operations depends on
  host scheduling, so this is not for deterministic runs.
- `Logical { origin, max_queued }`. The runner drives audio time:
  `Output::advance_to(logical)` at its scheduler points (each logical tick,
  vblank, or before a guest thread resumes), with logical time in 100 ns
  ticks (the kernel's `Clock::counter`; `origin` is the same clock's value
  at spawn). Frame `n` is due `n / rate` after `origin`, at the mixer's rate
  at spawn, and `advance_to` renders every whole quantum now due
  (`output::QUANTUM`, 256 frames) and completes packets after each, before
  it returns. It takes the guest memory and `DirectSound` locks exactly as
  an override call does, so rendering and completion are ordered with the
  game's DirectSound calls by the guest's own execution, and the result
  depends only on the order of guest operations and `advance_to` calls: a
  packet submitted at 32 ms is pending after a 32 ms `advance_to` that came
  first, and complete after one that came second, whatever the host
  threads did. The thread never touches the mixer, guest memory or events:
  it plays the rendered PCM, resampling if the sink's rate differs. At most
  `max_queued` of rendered audio waits for it; older audio beyond that is
  dropped and counted (`OutputStats::dropped_frames`), so a runner ahead of
  real time does not grow the queue. Audio time stands still between
  `advance_to` calls, so a runner that stops calling it while the game
  waits on a stream event with no timeout would hang the game.

`Output::stats()` reports rendered and dropped frames, wakes, late wakes,
the playout's state, the rate,
whether it halted, and the sink's counters. Dropping the `Output` stops and
joins the thread, drops the sink and releases the lease. With device
pacing a `Stop` from `do_work` ends the thread and is recorded by
`take_stops` as `Hle::output`; with logical pacing `advance_to` returns it.

The thread does not know when the runner faults or stops (the runtime has
no shared terminal state), and with device pacing left alone it would keep
writing packet statuses and signalling events. `Output::halt()` stops
that: it waits for a completion step in progress, and once it returns
neither the thread nor `advance_to` touches guest memory or events again
(the thread exits at its next wake; `advance_to` returns 0; the mixer stays
leased until the `Output` drops). `OutputConfig::halt_when`, polled before
every completion step, does the same from a runner's own "faulted or
stopped" flag. Call `halt` before taking a frozen snapshot or tearing down
the event bridge, and not while holding the guest memory guard.

Streaming during loading: stream packets complete and their events are
signalled whether or not the game calls any DirectSound entry (by the
output thread with device pacing, by the runner's `advance_to` with logical
pacing), so music keeps flowing while the game sits in a loading loop that
waits on those events (Halo's streams use events and poll
`GetStatus`/`Process`).

- `DirectSound::advance_audio(host, frames)` renders `frames` output frames,
  discards them (in chunks of at most 1024) and completes whatever that
  finished. `Hle::advance_audio(runtime, frames)` is the host-driven form.
- `AudioClock::new(rate, max_catch_up)` turns elapsed host time into whole
  frames, carrying the fraction and capping a stall (a stuck host does not
  owe seconds of audio at once). It holds no clock: callers pass the time.
  `Hle::tick(runtime, &mut clock, elapsed)` does both.
- `DirectSound::set_frames_per_do_work(Some(n))` advances `n` frames on every
  game `DirectSoundDoWork` (for example `rate / 60`): audio time follows the
  game's own call rate, deterministic and with no host clock.
- `Hle::spawn_pump(runtime, period, render)` runs a thread that every
  `period` calls `Hle::do_work`, or with `render` first advances by the real
  time elapsed (an `AudioClock` capped at 250 ms). It is for the case where
  the game blocks waiting on a stream event: the completion and event
  delivery do not depend on the game calling `DoWork`. Guest callbacks cannot
  run on a host thread, so they wait for the next override call on a guest
  thread, as for any host-driven `do_work`. Dropping the `Pump` stops and
  joins it; a `Stop` ends the thread and is recorded by `take_stops` as
  `Hle::pump`.

Never combine a render pump with a device, the output thread or the app's
discard renderer. `frames_rendered()` counts what the shim's manual,
`DoWork` and pump paths rendered (not the output thread: see its stats).

### Playout: the jitter buffer (logical pacing)

`Pacing::Logical { playout: Some(Playout) }` is what `h2 play` uses
(`boot::dsound_glue`). The runner renders in a burst at each slice
boundary, and logical time trails wall time by the frame's work. The app's
pacer lets logical time lead wall time by at most 2 ms. The device pulls at
its own steady rate. Before the buffer, the thread wrote each burst straight
into the device ring. The ring then held about one burst (a mean of about
18 ms in the simulation below), and any frame that ran long was a dropout.
Each dropout was a click, and only those dropouts built any cushion. The
playout:

- holds playback until `target_ms` (default 50) of audio is waiting in the
  sink plus the queue, at the start and again after the device runs dry, so
  a dropout is one clean gap;
- after a dropout, raises the target by `step_ms` (25), up to
  `max_target_ms` (150), and after `relax_after` (30 s) of clean play lowers
  it by 1 ms a second;
- trims the playback rate by at most `MAX_RATE_TRIM` (0.5%, about 9 cents)
  to hold the average fill at the target. This absorbs device clock drift
  and bleeds off the latency a hitch leaves behind. The trim is a linear
  resample (`LinearResampler::set_step`). It also converts the mixer's
  48 kHz to the device rate.

`H2_AUDIO_LATENCY_MS` (10..=500) sets the starting target. `None` keeps the
old write-at-once behaviour, which the exact-sample tests use. Guest-visible
behaviour does not change: packets still complete when they are rendered on
the logical clock. They are heard about one target later.

`output::sim` tests it on a virtual clock. The device pulls 512-frame
callbacks; the output thread wakes every 5 ms with jitter and a 15 ms stall
every 500 ms (a device probe); a paced guest renders at 60 Hz (3-15 ms of
work) or 30 Hz (8-31 ms). Immediate playout underruns. The playout has no
underrun, discontinuity or drop at 48 kHz or 44.1 kHz, with a mean fill
under 50 ms. With a 100 ms hitch every 4 s it costs at most two clean gaps
before the target covers the hitch.

### Audio stats

`H2_AUDIO_STATS=1` (`boot::dsound_glue::AudioReport`) prints one line every
5 s on stderr. Counters are the change over that window:

```
audio: 5.0s fill 48ms (30-70) target 75ms trim -1200ppm | dropouts +1 | dev underruns +2 (512 frames) | dropped +0 | late wakes +1 (max gap 21.5ms) | stream starves +2 (900 frames) unrouted +0 frames | callbacks waiting max 3 | rendered 100% of real time
```

- `fill (min-max)`: audio waiting for the device, now and over the window.
  `(buffering)` means the playout is waiting to reach its target.
- `target`, `trim`: the playout's target and its rate trim in ppm
  (+ is faster).
- `dropouts`: times the device ran dry while playing, one gap each.
  `dev underruns` counts the device callbacks that came up short; a gap
  spans several.
- `dropped`: rendered audio thrown away (the 250 ms queue overflowed, or
  the sink refused it).
- `late wakes`, `max gap`: the output thread was held up for more than
  10 ms past its period (a device probe, the host).
- `stream starves`: a stream ran out of packets and was fed again within
  250 ms (`audio::StreamHealth`). The game fed it late, and the gap is
  in the audio itself, before the device. Each is one cut to silence and one
  restart, so this is also the count of clicks at gap edges. With the
  completion DPC a guest that keeps up does not starve a stream: expect
  `+0`. A nonzero count with `rendered` at or above 100% means a feed path
  the DPC does not cover.
- `unrouted`: frames streams consumed, completing their packets, whose
  mix bins reached nothing: an empty list, or sends only to mix-ins that
  are not playing. The game believes those played. A dialogue line that is
  skipped but still counted shows here.
- `callbacks waiting max`: the most stream callbacks queued at once
  between slices, waiting for the completion DPC (or the next override
  call). The DPC runs them every quantum, so expect at most the packets that
  complete in one quantum: 1 or 2 per callback stream. A larger number
  means callbacks are not being delivered (no DPC installed, or the game
  held at raised IRQL).
- `rendered % of real time`: logical audio time over wall time. Under
  100% means the guest ran slower than real time, and the device must run
  dry.
- `pad ...`: the controller speaker's sink, when configured.

A headless run plays into `audio::NullSink`, which nothing drains: `fill`
sits at its ring plus the 250 ms queue (about 590 ms), `trim` at its
limit, and almost everything rendered is `dropped`. That is the sink only.
The output thread's queue and playout never feed back into logical time,
rendering or completions, so the stream counters are still the game's.

## Memory model

Xbox buffers *are* guest memory: `Lock` returns pointers into it, `Unlock` is
a no-op, and `SetBufferData` points a buffer at memory the game owns. The
shim keeps that contract:

- `dwBufferBytes != 0`: the shim allocates the memory from the guest
  (`GuestMemory::alloc`), filled with silence (`0x80` for PCM8).
- `Lock` returns `base + offset` (and `base` for the wrapped second region),
  never a host copy. `FROMWRITECURSOR` is a stop.
- The decoded float copy inside `audio::SoundBuffer` is refreshed from guest
  memory at `Unlock`, `Play` and `SetBufferData`. A game that rewrites a
  *playing* buffer without `Unlock` is heard at the next `Play` or `Unlock`.
  Closing that gap needs `audio` to read through a shared view (see the
  audio-crate wishes below).
- Stream packets are copied out of guest memory at `Process`.

## What is implemented

| area | state |
|---|---|
| `DirectSoundCreate`, `DirectSoundCreateBuffer/Stream`, `CreateSoundBuffer/Stream`, refcounts, `Release` freeing | real |
| `SetBufferData`, `Lock`, `Unlock`, `Play` (looping, from-start), `Stop`, `StopEx` (immediate), `Pause`, `GetStatus`, `Get/SetCurrentPosition` | real, mapped to `audio::SoundBuffer` |
| `SetVolume`, `SetFrequency`, stream `SetPitch` (1/4096 octave about 48 kHz), 3D position/min/max distance, cone angles/orientation/outside volume, rolloff factor (buffer and listener), listener position/orientation | real, applied to `audio` |
| `SetMixBins`, `SetMixBinVolumes` (buffer and stream) | real: routing through `audio::route`, FX sends into mix-ins |
| stream `Process`, `GetStatus`, `GetInfo`, `Flush`, `FlushEx` (any flags, timestamp 0), `Pause`, `SetVolume`, `Discontinuity` (counted, no mixer state to reset), `DirectSoundDoWork` | real (see "Effects, voice entries and FlushEx") |
| buffer and stream `SetFilter`, stream `SetLFO` and `SetEG` (amplitude generator), `SetHeadroom`, buffer `SetI3DL2Source`, `SetDopplerFactor` (buffer and listener), `SetOutputBuffer`, `XAudioSetEffectData` type 12 on the reverb index | real, applied to `audio` (see "Voice effects") |
| `SetMixBinHeadroom`, `SetEffectData`, distance factor, `UseLightHRTF4Channel`, `SetEG` generator 0 (pitch/filter envelope) | stored; counted by name in `DirectSound::ignored()` where counted |
| `GetCaps`, `GetSpeakerConfig` | answered (`DSCAPS` layout from memory: free 2D/3D voices 64, SGEs `0x7fff`, 0; stereo speakers) |
| mix-in/FX buffers | real buses (`audio::MixIn`, see "Positioned sound"); `Play` counted as `mix-in buffer Play`; I3DL2 source and headroom apply |
| the game's WMA codec (`0x38a0c5`..`0x38a1cc`) | native, `dsound::xwma` over `wma::boundary` (see `docs/audio/wma.md`, "Runtime boundary") |
| `XAudioDownloadEffectsImage` (flags 1, name `"DSPImage"`, dry image), `Get/SetEffectData` | real, against the dry image (see "Dry effects image") |
| `XAudioSetEffectData` | real, dry (see "Effects, voice entries and FlushEx") |
| buffer and stream `GetVoiceProperties` | real: mix-bin routing; **stop** is gone |
| `XAudioDownloadEffectsImage` with another name, `StopEx` or `FlushEx` with a timestamp, `Pause(3)` on an inactive voice, `Lock(FROMWRITECURSOR)`, data buffer without a format, `XAudioDownloadEffectsImage` with flags other than 1 | **stop** (`Stop::Unimplemented`) |
| `SynchPlayback` | real: `Play(SYNCHPLAYBACK)` and `Pause(2)` hold the voice (`Gate::Held`) until `SynchPlayback` releases it; `CommitDeferredSettings` is immediate |

### Pause, synch and status

Voice state word bits (`CMcpxVoiceClient+0x12`): `0x3` active, `0x4` paused,
`0x40` held until `SynchPlayback`, `0x400` hardware-paused, `0x20` pending
synch start. Buffer and stream pause keep the caller's intent: `Play`,
`Pause(1)`, `Pause(1)`, `Pause(0)` resumes playback (with its looping and
from-start choice). `Pause(2)` holds until `SynchPlayback`; `Pause(3)` on an
inactive voice is a stop request in the original and is a `Stop` here.

Stream `GetStatus`: bit 0 (`ACCEPT_INPUT_DATA`) and `0x20000` (paused,
produced at `0x37f591`; Halo tests it at `0x2ae4c4` before `Pause(0)` at
`0x2ae4d3`) are produced. The documented `0x10000`, `0x40000` and `0x80000`
bits are not produced.

## Dry effects image

Halo's sound init (`0x191300`, called from `0x21e610`) calls
`XAudioDownloadEffectsImage("DSPImage", pImageLoc, 1, &pDesc)` at `0x1913a4`
and then reads effect state with `GetEffectData`. The real call uploads a DSP
program to the Xbox's GP/EP DSP. The shim models the image as dry: the
effects exist and their state is plain storage; nothing is processed.

Evidence for the model (all from Halo's code, not from the library's
internals, which are unmatched):

- **Descriptor layout.** After the call Halo reads the first dword of `*ppDesc`
  and proceeds down the effect-state path only if it equals 15 (check at
  `0x19142f`; the disassembly at `0x191420..0x19142f` shows only this). That
  first dword is the only part of the layout the evidence supports. The rest of
  the layout is read elsewhere: `0x37e39e` computes the map for effect `i`
  as `desc + 8 + i * 32` (maps are 32 bytes, bounds-checked against the
  count at `+0`), `GetEffectData` copies from `[map + 8] + offset`
  (`0x37e5f2`) and the XHV voice engine (`XHVEngineCreate`, `0x333055`, its
  callee `0x333330`) dereferences `[map + 8]` and reads the dword at `+0x14`
  of that state (`0x33337a`, `0x33337d`) for the effects at its start index
  (`0xb`) onwards. The shim therefore builds `8 + 15 * 32` descriptor bytes
  with `lpvStateSegment` (map `+8`) pointing into a separate zeroed
  `15 * 0x1000` byte state block in guest memory; the other map fields are
  not read by anything found and stay zero. The block is the effect state
  itself: `GetEffectData`, `SetEffectData` and `XAudioSetEffectData` read and
  write it, so a guest read through the pointer and the API agree. The
  dword at state `+0x14` is set to `0x1400`, which XHV turns into a zero
  index (`(v - 0x1400) >> 5`); that value is invented, as the real one is a
  DSP address.
- **`pImageLoc`.** The caller passes `{ reverbIndex = 9, crosstalkIndex = 10 }`
  (two dwords). Both must be below the count of 15; otherwise `E_INVALIDARG`.
- **Flags and name.** Halo always passes flags 1 and the name `"DSPImage"`.
  The shim reads the name (an unmapped pointer faults), accepts only
  `"DSPImage"`, and stops on any other name and on flags other than 1 (the
  file-based form). The `DSPImage` section in the XBE
  is zeros on disc, which is consistent with the effects being a stand-in.
- **Effect indices read.** `GetEffectData(ds, effect, offset, buf, size)` is
  called for effects 4..7 at offset `0x20`, size 8 (`0x1913d0..0x191411`),
  and effects 1 and `0xb..0xe` at offset 0, size `0x3c` (`0x191451`). The
  shim backs each effect with `0x1000` bytes of guest memory, zero
  except for the dword above, and round-trips `SetEffectData` writes (Halo
  writes back at `0x19128f`). An index at or above the count is
  `DSERR_INVALIDCALL` (`0x37e39e`); offset+size beyond `0x1000` is
  `E_INVALIDARG` (this model's own limit, the library has none); any use
  before the download is `DSERR_INVALIDCALL`.
- **Zero initial effect state is an assumption.** The real initial state
  lives in the DSP image, which is not available (the section is zeros on
  disc). Zero is not derived from evidence; revisit it if sound init
  misbehaves.
- **`XAudioSetEffectData` is dry too.** See the next section.
- **The XHV voice effects run on the host.** Effects 0xb..=0xe are the
  per-port voice effects. On an Xbox each writes its port's voice mix into
  a ring that the headphone feed (`0x334d1f`, through `0x332c9c`) reads:
  the ring base is at map +0x18, its size at map +0x1c, and the write
  offset (bytes) at state +0x34. The feed divides by the size, so the
  descriptor gives each of these effects a 0x1400-byte ring (80 ms of
  16 kHz 32-bit samples); the other effects keep zeros there. The engine's
  voice streams (`0x335ed3`) take their one mix bin from `0x333330`, as
  `(state +0x14 - 0x1400) >> 5` of effect `0xb + port`. With `0x1400` for
  every effect, every port's remote voice landed on bin 0 and played on
  the left speaker. Each voice effect now gets its own invented base,
  `0x1400 + (bin << 5)` with bins `0x1b..=0x1e` (the top four FX sends,
  clear of the mix-ins' `0x17..=0x19`), so each port's streams have a bin
  of their own. The mixer captures those bins instead of mixing them
  (`audio::Mixer::set_taps`): remote voice is not on the speakers, as on an
  Xbox. After every render (`service_all`, so on every consumer's clock)
  `voice_rings::VoiceRings::pump` converts what was captured to 16 kHz,
  writes it into the port's ring as `sample << 16`, and moves state +0x34
  on. The rings advance by exactly the frames rendered, voice or not. The
  capture leaves out the master volume (it is the speakers'). Tested by
  reading the ring the way `0x334d1f` does (`tests/guest.rs`,
  `remote_voice_reaches_the_ports_ring_not_the_speakers`).

This is enough for the init path to proceed. The guest-visible image stays
dry: the DSP program is never run and the converted DSP words are not
computed. The reverb is a host-side stand-in instead: an
`XAudioSetEffectData` type 12 (`DSI3DL2LISTENER`) on the effect index the
game passed as `reverbIndex` configures `audio::Mixer::set_reverb`, and
mix bin 10 (the I3DL2 send) feeds it (see "Voice effects"). Crosstalk
routing through the DSP is not modelled.

## Effects, voice entries and FlushEx

Everything here was read from the retail disassembly (`h2 disasm`), not from
headers. Halo's reach into each entry is given so the model is no wider than
the use.

### `XAudioSetEffectData(index, pDesc, pRaw)` (`0x37ba6f`, `ret 0Ch`)

Callers: `0x21ee6f` (from `0x21ece0`, which `0x21ee80` runs once per effect
whose dirty byte at sound globals `+0x2a08 + i` is set, clearing it
afterwards) and `0x21fa07` (a per-effect record, index from a bit scan).
Both pass `pRaw = 0`. `0x21ece0` builds a `DSI3DL2LISTENER` for index 9 on
its stack and sets its first dword (the type) to 12 at `0x21ece3`.

Contract, in the order the binary does it:

1. `[0x387198]` is the live `CDirectSound` (set at `0x37a144`, cleared at
   `0x37a19f`). With none, the result is `DSERR_INVALIDCALL` before the
   descriptor is read. The shim tests for the root object.
2. `*pDesc` is the type. `4`, `10` and `12` are handled; anything else is
   `DSERR_INVALIDCALL` (`0x37baca`).
3. Parameters follow the type: type 4 reads 7 dwords (`+4..+0x1c`), type 10
   reads 3 (`+4..+0xc`), type 12 copies the 12 dwords of `DSI3DL2LISTENER`.
4. An effect index at or above the image's effect count fails inside
   `0x37e39e`. Type 12 does this before touching the parameters (its first
   call is the state read `0x37a34f`) and the failed read jumps to the
   epilogue with the return local still 0 (`0x37ba78`, `0x37baef`): the call
   returns **`S_OK`** and reads, writes and outputs nothing. Types 4 and 10
   read their floats first (an unreadable descriptor faults), store the
   failed write's `DSERR_INVALIDCALL` (`0x37bc4b`) and still copy their
   converted buffer to a non-null `pRaw` (the type word, then zeros here).
5. Type 12 reads the effect's first `0x118` state bytes, ORs `4` into the
   dword at `+0x10`, writes that dword back (apply = 1) and writes `0x108`
   converted bytes at `+0x118` (the I3DL2 converter, `0x3838a4`). Types 4
   and 10 write `0x2c` and `0x14` converted bytes at `+0x1c` (biquad
   coefficients, `0x37a032`, with `fsin`/`fcos`).
6. A non-null `pRaw` receives the type and then the converted bytes
   (`0x108`, `0x2c` or `0x14`).

The dry model keeps the observable parts: the checks and error codes above,
the `+0x10 |= 4` flag, and the descriptor (`DirectSound::effect_desc(index)`).
The converted DSP words are **not** computed: the converter and the biquad
generator need the DSP image's constants and floating-point behaviour, and
the dry image is zeros. So the destination range is left as the effect state
holds it (zeros unless the game wrote it with `SetEffectData`), and `pRaw`
receives exactly that range. This keeps one invariant: after the call,
`*pRaw` equals `GetEffectData(index, dest_offset, dest_len)`. Do not take
`pRaw` as real coefficients; Halo never asks for it.

`GetEffectData`/`SetEffectData` with an index past the image now return
`DSERR_INVALIDCALL` (the same `0x37e39e` check) instead of `E_INVALIDARG`.
The real code has no offset or size check; the `0x1000` cap stays as this
model's own limit (`E_INVALIDARG`).

### `GetVoiceProperties` (`0x37b7e3` buffer, `0x37b83f` stream)

Both reach `0x3818e5` with the voice client. Halo's readers are
`0x21fb38` (stream, once) and `0x21ff51`/`0x21ff62` (two voices of a
stereo pair); they hand the output to `0x2215d0`/`0x2216f0`, which compare
the count and every `{bin, volume}` pair with what Halo wants and call
`SetMixBins`/`SetMixBinVolumes` when they differ. Halo ignores the result,
so a failed call leaves uninitialised stack in its comparison.

- State bit 0 of the voice word at `+0x12` clear (never started, stopped,
  flushed) is `DSERR_INVALIDCALL` and nothing is written. The shim: a stream
  is active from its first `Process` until a flush; a buffer while playing or
  paused; a mix-in buffer while "played".
- Output, `0x5c` bytes: `+0x00` mix-bin count; `+0x04` eight pairs
  `{dwMixBin, lVolume}` (volume is `-((hw * 100) >> 6)`), unused slots are
  `{0xffffffff, 0xffffd8f0}`; `+0x44` a signed 16-bit voice word from voice
  RAM `+0x7c` (written as 0, not modelled); `+0x48..+0x5b` five dwords from
  the 3D/LFO state object (0 unless the voice has one; not modelled).
- The shim reports the routing the game set, in millibels. The hardware
  quantises to 1/64 dB steps; that is not reproduced.
- Default routing, now materialised when a voice is created or `SetMixBins`
  is called with a null `DSMIXBINS` (default tables read at `0x38590c`..
  `0x38595c`, chosen at `0x37bc89`): mono and stereo `{0, 1}`, 4 channels
  `{0, 1, 4, 5}`, 6 channels `{0..5}`, all volume 0; a voice with
  `flags & 0x200010` (3D) gets `{6, 8, 7, 9, 10}`; mix-in buffers get none.

### `GetInfo` (`0x37abd5`)

`XMEDIAINFO { dwFlags, dwInputSize, dwOutputSize, dwMaxLookahead }`: flags 5,
input size the low byte of `nBlockAlign`, output 0, lookahead
`2 * max(nBlockAlign, ((bits * channels) >> 3) << 5)` (`0x379eeb`), with 4
bits for ADPCM (tag `0x69`). No call site in the XBE reaches it: the
`[reg+8]` virtual calls in the stream users (`0x33400b`, `0x33446e`) are on
Halo's own objects. It is implemented because the vtable slot is overridden
and the derivation is short.

### `FlushEx` and `StopEx`

`FlushEx(this, tsLo, tsHi, flags)` is reached from five sites, all with a zero
timestamp: `0x2ae4e2` (flags 3), `0x333ef6`, `0x335744`, `0x335d13` (flags 1)
and `0x33598d` (flags 1, taken from `cmp edi, 1`). The old shim stopped on
flags 1 and 3, so streams could not even be shut down.

With flags 0 the library takes the plain flush (`0x37fbfe`). Otherwise it
runs `CMcpxStream_Stop` (`0x37f51f`), which tests only bits 2 and 4
(`ENVELOPE`, `IMMEDIATE`): they pick an envelope release or an immediate
voice stop, and (without `IMMEDIATE`) queue a "complete" command through
schedule slot `0x37f8dc`. `ASYNC` (1) is not tested. The shim has no envelope
and no scheduler, so **every flags value flushes at once**: packets complete
with `XMEDIAPACKET_STATUS_FLUSHED` before the call returns, statuses and
sizes first, then events or callbacks. The call is counted as
`FlushEx(flags)` in `ignored()`. A non-zero timestamp is scheduled on the DSP
clock by the library (`0x37f9ee`); nothing in the XBE passes one, and it
stays a stop.

`StopEx(this, tsLo, tsHi, flags)` has a single call site, `0x220052`, which
pushes `ebp` three times with `ebp` zeroed at `0x21ff35`: timestamp 0, flags
0. A timestamp stays a stop for the same reason.

## Voice effects

What reaches `audio` (`crates/audio/src/fx.rs`, `reverb.rs`), where the layouts
came from, and what is assumed. Descriptor layouts were read from how Halo
builds them (`h2 disasm`; constants read from the retail image and reduced to
the scalar meanings below, none copied). Halo's sites: the stream sound
builder (`0x220200..0x220700`) fills `SetFilter` (`0x2203e9`) and two
`SetLFO` calls (`0x220508`, `0x22065f`..); the fade helper `0x2ae7a0` fills
`SetEG` (`0x2ae812`); the occlusion code at `0x221300..0x221422` fills
`SetI3DL2Source`; `0x21f8f0` calls `SetOutputBuffer` with a mix-in or null.

| call | descriptor | model |
|---|---|---|
| `SetFilter(pDesc)` | `DSFILTERDESC`: `dwMode`, `dwQCoefficient`, 4 coefficient dwords; null = bypass | mode 1 (DLS2): low pass, cutoff word `c0`, damping word `c1` (`c2`, `c3`, a second pair Halo also fills, ignored). Mode 2 (PARAMEQ): two peaking bands `(c0, c1)`, `(c2, c3)` = (cutoff word, gain word), Q index `dwQCoefficient` 0..7. Mode 3 (MULTI): cutoff `c0`, damping `c1`, **response chosen by `dwQCoefficient` bits (1 low, 2 band, 4 high; 5 notch; others approximate or bypass)**. Mode 0 bypass |
| `SetLFO(pDesc)` | `dwLFO`, `dwDelay`, `dwDelta`, `lPitchModulation`, `lFilterCutOffRange`, `lAmplitudeModulation` | `dwLFO` 1 = pitch LFO (Halo's first call, pitch only), otherwise the filter/amplitude LFO (second call). Delay `/1500` s, rate `delta·1500/65536` Hz, triangle from 0 after the delay. Depth units **assumed**: pitch 1/128 octave, cutoff 1/16 octave, amplitude 1/16 dB (Halo scales its tag floats by 128, 16, 16 and clamps to a signed byte) |
| `SetEG(pDesc)` | `dwEG`, `dwMode`, `dwDelay`, `dwAttack`, `dwHold`, `dwDecay`, `dwRelease`, `dwSustain`, `lPitchScale`, `lFilterCutOff` | generator 1 = amplitude (Halo's fade: mode 2, attack and release set, sustain `0xff`); modes 1..5 pick the start stage, 0 bypasses. Times in 1/93.75 s (Halo multiplies seconds by 93.75). Release not entered. Generator 0 (pitch/filter) counted, not applied |
| `SetHeadroom(mB)` | scalar | the voice is attenuated by `mB` (0..6000); Halo passes 0 |
| `SetI3DL2Source(pDesc, apply)` | `DSI3DL2BUFFER`, 9 dwords: direct, directHF, room, roomHF, roomRolloff, obstruction (HF, LF ratio), occlusion (HF, LF ratio) | direct level = `direct + Σ HF·LFratio` (broadband gain), HF level = `direct + directHF + obstruction + occlusion` as a one-pole high-frequency shelf referenced to 5 kHz; room send gain = `room + occlusion·LFratio`. `roomHF`, `roomRolloff` unused |
| `SetOutputBuffer(pBuffer)` | guest pointer or null | a mix-in replaces the stream's bins with its input bin at `1 / channels`; later `SetMixBins` pairs play alongside it and `SetMixBinVolumes` scales it; null takes the send out; a non-mix-in is counted (see "Distance attenuation") |
| `SetDopplerFactor` (buffer, listener) | float | scales the Doppler shift (see `docs/audio/audio.md`). No guest call sets a velocity (Halo never calls `SetVelocity`), so the shift is 1 for Halo; the machinery is tested in `audio` |
| `XAudioSetEffectData(9, type 12)` | `DSI3DL2LISTENER`, 12 dwords | `Mixer::set_reverb` (index = the `reverbIndex` of the last `XAudioDownloadEffectsImage`) |

Corrections after review: a cut in a PARAMEQ gain word is biased (`0xc000 -
v`, `0xc000..=0xdfff`), a boost is `v` up to `0x7fff` (an ignored test
`parameq_gain_converter_uses_the_biased_cut_encoding` checks the converter's
limits against `$H2_XBE`); obstruction and the direct HF level shape the
direct path only (speakers and mix-in sends), never the reverb send; source
descriptors are clamped to the I3DL2 ranges and Doppler factors, velocities
and the final ratio are limited so nothing non-finite reaches the mix;
recursive filter, shelf and diffuser state is flushed to zero when tiny;
`SetOutputBuffer` holds a reference on its target (balanced on replace,
clear and stream destruction; a stale pointer is a `BadHandle` stop).
A second review fixed four more: the direct shelf runs over only the frames a
voice produced in a render (its state used to advance over block padding, so
tails depended on block size; tests `direct_shelf_does_not_depend_on_render_block_size`
and `direct_shelf_survives_starvation_and_resume_identically`); a stream keeps
the last 514 frames of completed packets in a preallocated ring so a negative
read offset reads real history across a packet join (test
`packet_boundaries_do_not_change_a_stream_under_receding_doppler`); the deepest
PARAMEQ cut `0xdfff` decodes to -78.27 dB (the old -60 dB floor is gone; the
ignored xbe pin now asserts it); and the phase-bound test drives receding and
approaching Doppler and checks the stream's real offset saturates at +-512.

Determinism and timing. Each voice's effects advance on a frame clock that
the voice itself counts (stream clocks do not advance while starved, a
`Flush` or a buffer `Play` from position 0 starts them over), so the output
is the same for any `Output::advance_to` slicing and any render block size
(`audio` tests render the same stream in blocks of 97, 512 and 24000
frames). Nothing changes how many frames a voice consumes, so packet
completion order and timing are the same with every effect on (tests
`effects_leave_packet_completion_alone`, `fx_do_not_change_packet_completion`).
On a **stream** the pitch LFO and Doppler do not change consumption either:
packets are consumed at the nominal (`SetPitch`) rate, and the modulation
moves only the read position, by an offset accumulating `(ratio - 1)` per
frame and bounded to +-512 frames, so a vibrato is faithful until its swing
exceeds that window and then saturates (a constant Doppler shift therefore
shows as a bounded phase shift, not a pitch change). Test
`pitch_lfo_leaves_packet_completion_alone` services the stream and compares
status and completed size frame by frame. On a **buffer** (no packets) they
change the playback rate like `SetFrequency`, so a buffer's end moves.

Left out, on purpose:

- **Envelope release** (`FlushEx`/`StopEx` with the envelope flag): Halo's
  `FlushEx(.., 3)` at `0x2ae4e2` asks the hardware to fade out first. Fading
  asynchronously would hold packets pending and change completion timing, so
  stops stay immediate. The fade-in is audible; the fade-out is a cut.
- **Mix-bin headroom** (`SetMixBinHeadroom`): stored (`mix_bin_headroom`) but
  without a level effect; on the MCPX the bin gives back what it takes, and
  the float mix does not clip until the output stage.
- **Pitch and filter envelope**, **second DLS2 cutoff/damping pair**, **LFO
  waveforms other than triangle**, **room rolloff** and **`roomHF`** of a
  source, **crosstalk-cancellation**, **HRTF**.
- **Doppler on mix-ins**: a mix-in has no samples to resample.

Assumptions to check against hardware or xemu: the MULTI selector bits,
the PARAMEQ Q table (`0.5..8`), the LFO depth units, the `dwEG` generator
numbering, the `DSEG_MODE` stage numbers beyond attack (2), triangle LFO
shape and the 5 kHz default I3DL2 HF reference for a source. The coefficient
word encodings (cutoff, damping, gain, EG and LFO time units) are read from
Halo's converters and are not assumptions.

The reverb is a clean-room approximation from the published I3DL2 parameter
semantics (see the `audio::reverb` module docs); the Xbox DSP image and its
converter (`0x3838a4`) are not used. Tests check decay time (Schroeder RT60
within 20%), first-reflection and late-onset timing, levels in millibels,
HF damping, block-size independence and stability.

## Overrides

`overrides.rs` holds `ENTRIES` (address, name, argument count) and
`invoke(ds, host, va, args) -> Result<Return, Stop>`. It does not depend on
the runtime. The 7 stream-vtable entries are registered at their slot target
addresses, not at the vtable.

### Runtime registration

`dsound::register` (`src/register.rs`) installs all 66 entries in one
dispatch edit, or none of them:

```rust
pub trait EventRef: Send {
    fn signal(&mut self) -> Result<(), String>;      // drop releases the pin
}
pub trait Events: Send {
    fn signal(&mut self, guest_handle: u32) -> Result<(), String>;
    /// Default: no pinning.
    fn reference(&mut self, guest_handle: u32) -> Option<Result<Box<dyn EventRef>, String>> { None }
}
impl Hle {
    pub fn new(ds: DirectSound, events: Box<dyn Events>) -> Arc<Hle>;
    pub fn do_work(&self, runtime: &runtime::Runtime) -> Result<(), Stop>;
    pub fn advance_audio(&self, runtime: &runtime::Runtime, frames: u64) -> Result<(), Stop>;
    pub fn tick(&self, runtime: &runtime::Runtime, clock: &mut AudioClock, elapsed: Duration) -> Result<u64, Stop>;
    pub fn spawn_pump(self: &Arc<Self>, runtime: Arc<Runtime>, period: Duration, render: bool)
        -> Result<Pump, ConsumerConflict>;
    pub fn spawn_output<S: AudioSink + 'static, F: FnOnce() -> S + Send + 'static>(
        self: &Arc<Self>, runtime: Arc<Runtime>, open: F, config: OutputConfig,
    ) -> Result<Output, ConsumerConflict>;
    /// A kernel periodic-source handler that runs queued stream callbacks.
    pub fn completion_dpc(self: &Arc<Self>) -> impl FnMut(&mut Guest, u32, u64)
        -> Result<Option<GuestInterrupt>, GuestFault> + Send + 'static;
    pub fn pinned_events(&self) -> usize;
    pub fn take_stops(&self) -> Vec<StopRecord>;
    pub fn take_signal_failures(&self) -> Vec<SignalFailure>;
    pub fn direct_sound(&self) -> MutexGuard<'_, DirectSound>;
}
pub fn register(dispatch: &mut runtime::Dispatch, hle: &Arc<Hle>)
    -> Result<Registration, RegisterError>;
pub fn unregister(dispatch: &mut runtime::Dispatch, registration: &Registration);
```

- **Conflicts.** If any address already has an override, `register`
  installs nothing and returns `RegisterError::Occupied` with every conflict.
  Registering twice reports all 66 with `existing == ours`. `unregister`
  disables the entries, because `Dispatch` cannot remove them, and a later
  `register` reports them. Override names are `dsound::<name>`.
- **One call.** Locks are taken in a fixed order: guest memory (`ctx.memory()`),
  then `DirectSound`, then the arena. The adapter reads the return PC and
  `argc` dwords above it and runs `invoke` over a bridge `Host`. The bridge
  queues events and callbacks instead of delivering them. Then it drops every
  guard and delivers the queued events through `Events::signal`, which may
  relock guest memory as the kernel does. A refused signal is kept in
  `take_signal_failures`; it does not stop the game.
- **Leaving.** On success the adapter writes EAX and sets
  `ESP += 4 + 4 * argc`. If stream callbacks are queued, it then runs each
  one as a guest stdcall `callback(stream_context, packet_context, status)`
  through `ExecCtx::call_guest`. The callbacks run in order, as if called
  from the original return site, and EAX is restored before the final
  `TransferAbi::ret`. Callbacks produced by a host-driven `Hle::do_work` (or
  an output's render) wait and run at the next override call, which is the
  completion DPC's `DirectSoundDoWork` when one is installed.
- **Stops and faults.** On a `Stop` the CPU and stack stay untouched. The
  stop is recorded in `take_stops` (address, name, args, `Stop`) and the
  runner stops on `Event::Unsupported { mnemonic: "dsound::<name>",
  detail: "hle: <Stop>" }`. The runtime has no HLE-specific event yet. A
  guest memory fault (`Stop::Memory`) is a runtime FAULT instead, and keeps
  the runtime's `GuestFault` and reason. Callbacks already owed by
  packets that completed before a stop or fault are queued, and they run at
  the next override call.
- **Guest memory.** `register::Arena` maps zeroed RW chunks of at least
  64 KiB on demand, inside `ARENA = 0xa080_0000..0xa100_0000`. That is the
  upper half of the HLE window that `bink-hle` documents as `HLE_RESERVED`.
  It skips existing mappings and never adopts one. Freed blocks are reused
  first-fit and zeroed again.

### Boot integration

`boot::dsound_glue::install` does this wiring for the booted game (`install`, `install_with_sink`,
`install_with_sinks`).
What follows is the same shape for a runner of your own, including the audio
clock and event pinning. Don't register both: `register` reports every
address the glue owns as occupied.

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// dsound::Events over the kernel. Every method runs with no guest memory
/// or DirectSound guard held, so the kernel may relock memory.
struct KernelEvents(Arc<kernel::Kernel>);

/// A kernel event pinned by `Kernel::reference_dispatcher`. It signals even
/// if the guest has closed the handle; dropping it releases the pin.
struct Pin(kernel::DispatcherReference);

impl dsound::EventRef for Pin {
    fn signal(&mut self) -> Result<(), String> {
        self.0.signal_event().map(drop).map_err(|e| e.to_string())
    }
}

impl dsound::Events for KernelEvents {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        self.0.signal_event(handle).map(drop).map_err(|e| e.to_string())
    }
    fn reference(&mut self, handle: u32) -> Option<Result<Box<dyn dsound::EventRef>, String>> {
        Some(
            self.0
                .reference_dispatcher(handle)
                .map(|r| Box::new(Pin(r)) as Box<dyn dsound::EventRef>)
                .map_err(|e| e.to_string()),
        )
    }
}

// After Kernel::for_image, before Machine::new. The mixer is the shim's
// own: the output thread below is its one consumer. Do not also start the
// app's AudioOut/DiscardRenderer on it.
let mixer = Arc::new(Mutex::new(audio::Mixer::new(48_000)));
let ds = dsound::DirectSound::new(Arc::clone(&mixer), dsound::Layout::default());
let hle = dsound::Hle::new(ds, Box::new(KernelEvents(Arc::clone(&k))));
loaded.runtime.edit_dispatch(|d| dsound::register(d, &hle))?;

// The game's WMA codec (menu and level music), natively. Tables come from
// the XBE bytes at runtime; a non-retail XBE fails here, and the game then
// keeps its own codec.
let wma = dsound::xwma::WmaHle::from_xbe(&xbe_bytes)?;
loaded.runtime.edit_dispatch(|d| dsound::xwma::register(d, &wma))?;

// Sound out: render into the default device and report completions, on its
// own thread (keeps streams flowing through loading screens).
let output = hle.spawn_output(
    Arc::clone(&loaded.runtime),
    || audio::output::DeviceSink::open_default().expect("audio device"),
    dsound::OutputConfig::default(),
)?;
// When the runner faults or stops, before collecting diagnostics:
// output.halt();   (or set OutputConfig::halt_when to the runner's flag)
// Deterministic runs (headless, replays): logical pacing, into any sink
// (audio::NullSink when there is no device), driven by the scheduler:
// let output = hle.spawn_output(runtime, open, dsound::OutputConfig {
//     period: Duration::from_millis(5),
//     pacing: dsound::Pacing::Logical {
//         origin: k.clock().counter(),
//         max_queued: Duration::from_millis(250),
//         playout: Some(dsound::Playout::default()), // None: exact samples
//     },
//     halt_when: None,
// })?;
// ... and at every scheduler point (logical tick or vblank), with no guest
// memory guard held:
// output.advance_to(k.clock().counter())?;
// ... with the completion DPC on the same origin, so callbacks run at DPC
// level within two quanta of their packet (see "The completion DPC"):
// k.add_periodic(kernel::periodic::Source {
//     name: "dsound completions",
//     rate: kernel::periodic::Rate { num: 48_000, den: dsound::output::QUANTUM },
//     max_catch_up: 64,
//     handler: Box::new(hle.completion_dpc()),
// })?;

// On a stop whose event is Unsupported with a "dsound::" mnemonic:
for s in hle.take_stops() {
    eprintln!("dsound stop at {:#010x} {}{:x?}: {}", s.addr, s.name, s.args, s.stop);
}
for f in hle.take_signal_failures() {
    eprintln!("dsound event {:#x}: {}", f.handle, f.error);
}
```

Without a sink at all (a test harness), use one of these instead of the
output thread (pick one; the others then return `ConsumerConflict`):

```rust
// Real time, own thread: renders the mixer and reports completions.
let _pump = hle.spawn_pump(Arc::clone(&loaded.runtime), Duration::from_millis(5), true)?;
// or deterministic, tied to the game's own calls (48 kHz at 60 DoWork/s):
hle.direct_sound().set_frames_per_do_work(Some(800))?;
// or by hand between slices:
let mut clock = dsound::AudioClock::new(48_000, Duration::from_millis(250));
hle.tick(&loaded.runtime, &mut clock, elapsed)?;
```

`do_work`, `advance_audio` and `tick` take the guest memory guard themselves:
call them from a host thread or between runner slices, never from inside an
override. Stream callbacks are guest calls, so they run at the next override
call on a guest thread (see "Runtime registration"), or from the completion
DPC where the runner installs it, as `boot` does.

## Tests

`cargo test -p dsound`. `tests/guest.rs` builds a synthetic guest (flat
memory plus a bump allocator, `testing::TestHost`) and drives everything
through `invoke` with the real addresses: ADPCM blob to `SetBufferData` to
`Play` to mixed frames (hand-decoded samples 1000, 1011, 981), `Lock` wrap
and `Unlock`, pause/resume/position, stream packets completing in order with
status, completed size and events, callbacks, flush and capacity, release
freeing, 3D panning, stored-state setters and every stop path. It also
covers, with synthetic data only (no disc bytes):

- XAudioSetEffectData: type 12 flag OR and the `pRaw == GetEffectData`
  invariant, types 4 and 10, unknown types, the index-before-read ordering
  per type, no-DirectSound and no-image errors, and a `pRaw` fault after
  the state changed.
- GetVoiceProperties on buffers and streams: inactive voices write nothing,
  the `0x5c`-byte layout, unused slots, explicit and default routing
  (stereo, 3D), `SetMixBins(NULL)`, pause, stop and flush transitions.
- GetInfo for PCM mono/stereo, Xbox ADPCM mono/stereo.
- FlushEx with flags 1, 3, 2, 4, 7 completes packets as flushed, and a
  timestamp stays a stop.
- Progress: polling `GetStatus` or `Process` collects completions without
  `DoWork`; `advance_audio` finishes a buffer and two packets with no sink;
  `set_frames_per_do_work`; `AudioClock` rounding and the stall cap.
- Notification contract: event references taken per packet and returned when
  a stream is released with packets pending; status and size are final in
  guest memory at the moment of each event and callback.

`tests/register.rs` goes through the real `runtime::Runner` and `Dispatch`:

- The installed set equals 66 literal address/argc pairs, written
  independently of `ENTRIES`.
- Every address is called with HARNESS_EXIT as its return, against objects
  that were themselves created through the runner. Calls that return check
  ESP rising by exactly `4 + 4 * argc` and observable argument decoding: the
  Lock wrap pointers and sizes, GetCaps, GetSpeakerConfig, Set/GetEffectData
  round trip, listener position and orientation, mix-bin headroom,
  Set/GetCurrentPosition, GetStatus bits, AddRef/Release counts, packet
  status and size, the Discontinuity count, XAudioSetEffectData (type 12,
  index 9, no pOut), both GetVoiceProperties and GetInfo, FlushEx flags 1
  and 3. The documented stop paths (StopEx and FlushEx with a timestamp)
  leave the frame untouched and record their arguments.
  The test fails if any address is left unexercised.
- A completion event is delivered after the guards drop. The test's
  `Events` relocks guest memory on another thread with a timeout, and
  delivering under the guard makes it fail.
- Stream callbacks run as guest calls in order, with their three
  arguments, and EAX is restored.
- Guest memory faults are runtime FAULTs, not stops. Callbacks owed by
  packets that completed before the fault are kept and run at the next
  override call (two packets: the first completes, the second's status
  write faults). Freed objects are reused.
- Stops leave the whole stack mapping byte-identical.
- After `unregister`, every address goes to the runner's fallback
  (UnsupportedTarget) without touching the stack.
- Registering twice, registering over a foreign override, and registering
  after `unregister` are all reported, and nothing is installed.
- Event references (`PinEvents`, which also relocks memory from `reference`
  and `EventRef::signal`): a pinned event signals after the guest closed its
  handle and the pin is then dropped; pins are per packet in queue order; a
  zero-length packet retains then signals in one call; releasing a stream
  drops its pins; a sink that does not pin gets the handle signal; a failed
  `reference` never steals or drops another packet's pin; a faulting
  completion write keeps the packet and its pin and completes on retry; a
  concurrent pump and `Process` never signal a packet before it is pinned.
- Positioned sound: a stream sent to a 3D mix-in through its FX bin is
  panned by the mix-in's position and follows `SetPosition`; rolloff 0
  removes the shim's distance attenuation; doppler is counted.
- Consumers: only one of output thread, render pump, `set_frames_per_do_work`
  and manual `advance_audio` may hold the mixer; dropping the holder frees it.
  A second output thread or render pump is refused, and a stale lease
  releases nothing.
- Output (`CaptureSink`): `advance_to` completes packets in order, in whole
  quanta, with status final at each event and before it returns, and the
  thread plays them; a full sink bounds the queue (512 frames rendered, 256
  dropped at a 32 ms bound) without delaying completions; one, two or four
  steps to the same logical time give identical audio and completions; the
  same guest history (a packet submitted at 32 ms, the runner's 32 ms point
  before or after it) gives identical statuses, events and audio whether
  the output thread ran freely or could not run at all; device pacing
  follows the sink's fill and rate; callbacks it completes run at the next
  override call; once halted (by hand or by `halt_when`) nothing renders,
  no status is written and nothing is signalled.
- Host-driven clock: `Hle::tick` completes packets, delivers the event and
  leaves the callback owed to the next override call. `spawn_pump` with
  `render: true` completes a packet on a sinkless mixer, and with
  `render: false` reports what an external consumer rendered, without
  rendering anything.

`tests/stream_feed.rs` runs Halo's two stream feeders on the real kernel
scheduler, the registered overrides and a logical-pacing output, with the
boot loop (`m.run(1)`, then `advance_to` the clock). A guest frame at 60 Hz
or 30 Hz, its sound update jittered within the frame, sleeps, refills and
calls `DirectSoundDoWork`. A callback-fed stream (two 20 ms 8 kHz packets,
resubmitted from the callback, as the voice streams are) starves without the
completion DPC and never with it, and the callback queue never holds more
than two. A polled stream with Halo's 16 KB 44.1 kHz stereo packets,
refilled when a status is no longer pending, never starves either way.

`tests/xwma.rs` drives the WMA boundary through the runner, with synthetic
tables and packets and the game's input callback as a guest override
serving 128-byte chunks: both retail profiles decode to the same PCM as
`wma::Decoder` directly, a reader that runs dry between packets loses no
frames (the end is only taken from the container's reader offsets), every callback gets the container's context, a read
stalled mid-packet resumes, `destroy` clears the handle word, unknown
handles are `E_INVALIDARG`, other formats are refused with `0x80040000`, and
a second registration fails closed. An ignored test decodes two real disc
streams through the boundary and compares them with `wma::decode_asf`:

```sh
H2_XBE="$PWD/disc/default.xbe" \
  cargo test --release -p dsound --test xwma -- --ignored
```

`tests/xbe_pins.rs` is ignored by default. In the real XBE it walks each of
the 66 addresses (iced-x86, following direct jumps, so the flat `jmp`
thunks reach their `CDirectSoundStream_*` bodies). It requires the
reachable `ret` immediates to be exactly `4 * argc` and the address to lie
in `DSOUND`. It also checks that the 7 words of the stream vtable at
`Layout::default().stream_vtable` (`0x417170`) are the 7 vtable entries,
and pins the six `dsound::xwma::ENTRIES` the same way (`ret 2Ch`, `4`, `4`,
`4`, `4`, `10h`):

```sh
H2_XBE="$PWD/disc/default.xbe" \
  cargo test -p dsound --test xbe_pins -- --ignored
```

## Not verified

Layouts and values taken from XDK headers rather than the binary:
`DSCAPS`, `DSSTREAMDESC` flag meanings, `DSBCAPS` bit names (only `0x10`,
`0x2000`, and the `0x182000` mask are tied to behaviour), `Play` flag names
(bits 0x1/0x2/0x4 are used as loop/from-start/synch; 2 and 4 are verified as
distinct tests in `CMcpxBuffer_Play`, 1 is assumed to be looping),
`GetStatus` bit names (decoded from `CMcpxBuffer_GetStatus`: 1 playing, 2
paused, 4 looping), the `Pause` mode 3 semantics, the stream status bits
`0x10000`/`0x40000`/`0x80000`, and the dry effects image's default effect
state (zeros assumed, apart from the invented `+0x14` dword; the map fields
other than `lpvStateSegment` are unread by anything found and left zero). The packet completion rule and the pause/synch voice bits are
verified. Also not reproduced, though the binary defines them: the I3DL2
converter (`0x3838a4`) and biquad generator (`0x37a032`) behind
`XAudioSetEffectData`'s DSP words (the host reverb and filters are
approximations from public semantics instead, see "Voice effects"), the voice
level word and 3D/LFO state in `GetVoiceProperties`, hardware volume
quantisation, envelope release and timestamp scheduling for
`FlushEx`/`StopEx`, and a stream's "active" state
after its queue drains naturally (the shim keeps it active). Whether
`GetInfo` is ever called is unproven; no site was found. The `pRaw` and
default-routing facts above come from small constants read at the cited
addresses.

## Wanted in `crates/audio`

1. A way to replace a buffer's data and format in place (today `SetBufferData`
   to a different size removes and re-adds the voice, which changes its
   `BufferId`, and `Mixer` would need to preserve params).
2. A `GuestView`-style source so a playing buffer reads guest memory without
   `Unlock` (or a dirty-range API on `SoundBuffer`).
3. `PacketStatus::Flushed` should carry the frames already consumed, so
   `*pdwCompletedSize` can be exact for a half-played flushed packet.
4. `SoundBuffer::pause` that preserves the playing/looping intent, and a
   `stopped_at_end` flag so `GetStatus` can distinguish a finished buffer.
5. (Done: mix-bin routing, mix-in buses, voice headroom and a reverb on the
   I3DL2 send.) Mix-bin headroom clipping.
6. Loop and play regions and buffer position notifications if another title
   path needs them (Halo 2 does not call them).
7. Wrap-around reads for streams of non-block-aligned PCM. (WMA: done, in
   `dsound::xwma`.)

## Controller-speaker routing

`dsound::padroute` labels buffers (at `SetBufferData`) and streams (at
`Process`, cleared by `Flush`) from the opening bytes of their data, and
marks chosen sounds for a second host output with `audio::VoiceOutput`. Off
unless `DirectSound::set_pad_router` is given a router (`boot::pad_speaker`
installs one per map the game opens). The output thread plays those voices
into a second sink (`OutputConfig::side`, `Mixer::render_split`) while it is
connected, and keeps them on the main mix otherwise. See
[playstation-audio.md](playstation-audio.md).
