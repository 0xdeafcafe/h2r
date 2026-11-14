# DirectSound (guest side)

`crates/dsound` is the guest-facing half of audio: it takes the calls Halo 2
makes into its statically linked Xbox DirectSound (`DSOUND` section,
`0x00379c60`..`0x00387374`, XDK 5849) and answers them from Rust. It reads
and writes guest memory through a small trait and drives `crates/audio`
(ADPCM, mixer, cpal), which is described in [audio.md](audio.md). It has no
dependency on `crates/runtime` or the kernel layer.

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
  interrupt/DPC. The shim has no thread of its own: completions are written
  by `DirectSoundDoWork`, and also straight after `Process` (zero-length
  packets) and `Flush`. The host should call `DirectSound::do_work` after
  each mixer render it drives if it wants completions on the audio clock.
  Events are `EventSink::signal_event(handle)`; callbacks are queued with
  `EventSink::stream_callback(StreamCallback)` and run by the kernel agent as
  a guest call at a safe point.
- **Buffers have no notifications in Halo 2.** `SetNotificationPositions`,
  `SetPlayRegion` and `SetLoopRegion` are not matched and have no call sites.

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
| `SetVolume`, `SetFrequency`, stream `SetPitch` (1/4096 octave about 48 kHz), 3D position/min/max distance, listener position/orientation/factors | real, applied to `audio` |
| stream `Process`, `GetStatus`, `Flush`, `FlushEx(0)`, `Pause`, `SetVolume`, `Discontinuity` (counted, no mixer state to reset), `DirectSoundDoWork` | real |
| `SetMixBins`, `SetMixBinVolumes`, `SetHeadroom`, `SetMixBinHeadroom`, `SetFilter`, `SetLFO`, `SetEG`, `SetOutputBuffer`, `SetCone*`, `SetDoppler/RolloffFactor` (buffer), `SetI3DL2Source`, `SetEffectData`, `UseLightHRTF4Channel` | stored; counted by name in `DirectSound::ignored()` (timbre and routing only) |
| `GetCaps`, `GetSpeakerConfig` | answered (`DSCAPS` layout from memory: free 2D/3D voices 64, SGEs `0x7fff`, 0; stereo speakers) |
| mix-in/FX buffers | created and `Play` accepted (silent, counted); no DSP |
| `XAudioDownloadEffectsImage` (flags 1, name `"DSPImage"`, dry image), `Get/SetEffectData` | real, against the dry image (see "Dry effects image") |
| `XAudioSetEffectData`, `XAudioDownloadEffectsImage` with another name, `GetVoiceProperties`, `GetInfo`, `StopEx` with a timestamp, `FlushEx` with any non-zero flags or timestamp, `Pause(3)` on an inactive voice, `Lock(FROMWRITECURSOR)`, data buffer without a format, `XAudioDownloadEffectsImage` with flags other than 1 | **stop** (`Stop::Unimplemented`) |
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
  first dword is the only part of the layout the evidence supports. The shim
  also allocates 15 zero-filled 16-byte entries after it
  (`EFFECT_IMAGE_DESC_SIZE = 4 + 15*16`): that is placeholder padding we
  chose, not a derived layout.
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
  shim keeps a per-effect byte range (up to `0x1000` bytes), returns zeros for
  never-written ranges, and round-trips `SetEffectData` writes (Halo writes
  back at `0x19128f`). Index at or above 15, or offset+size beyond `0x1000`,
  is `E_INVALIDARG`; any use before the download is `DSERR_INVALIDCALL`.
- **Zero initial effect state is an assumption.** The real initial state
  lives in the DSP image, which is not available (the section is zeros on
  disc). Zero is not derived from evidence; revisit it if sound init
  misbehaves.
- **`XAudioSetEffectData` is a stop.** It takes 3 arguments; the binary reads
  the descriptor, converts parameters, updates effect state and arg 3 is an
  output pointer it may write. None of this is modelled, so it returns
  `Stop::Unimplemented` rather than success.

This is enough for the init path to proceed. What the game then does with
reverb and crosstalk (routing through DSP effects) is not modelled; the audio
output is dry.

## Overrides

`overrides.rs` is marked `TODO-register`: it holds `ENTRIES` (address, name,
argument count) and `invoke(ds, host, va, args) -> Result<Return, Stop>`.
The registration glue is: for each entry, register a handler that reads
`argc` dwords above the return address, calls `invoke`, writes `Return::eax`,
pops `Return::pop_bytes` and returns. The 7 stream-vtable entries use the
same table and are registered at their slot target addresses, not at the
vtable. A `Stop` must surface as a harness stop.

## Tests

`cargo test -p dsound`. `tests/guest.rs` builds a synthetic guest (flat
memory plus a bump allocator, `testing::TestHost`) and drives everything
through `invoke` with the real addresses: ADPCM blob to `SetBufferData` to
`Play` to mixed frames (hand-decoded samples 1000, 1011, 981), `Lock` wrap
and `Unlock`, pause/resume/position, stream packets completing in order with
status, completed size and events, callbacks, flush and capacity, release
freeing, 3D panning, stored-state setters and every stop path.

## Not verified

Layouts and values taken from XDK headers rather than the binary:
`DSCAPS`, `DSSTREAMDESC` flag meanings, `DSBCAPS` bit names (only `0x10`,
`0x2000`, and the `0x182000` mask are tied to behaviour), `Play` flag names
(bits 0x1/0x2/0x4 are used as loop/from-start/synch; 2 and 4 are verified as
distinct tests in `CMcpxBuffer_Play`, 1 is assumed to be looping),
`GetStatus` bit names (decoded from `CMcpxBuffer_GetStatus`: 1 playing, 2
paused, 4 looping), the `Pause` mode 3 semantics, the stream status bits
`0x10000`/`0x40000`/`0x80000`, and the dry effects image's default effect
state (zeros assumed; the image descriptor beyond its count is unread by Halo
and left zero). The packet completion rule and the pause/synch voice bits are
verified.

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
5. Per-voice gain and routing for mix bins and headroom (today stored only),
   and a send/return for the mix-in buffers (reverb).
6. Loop and play regions and buffer position notifications if another title
   path needs them (Halo 2 does not call them).
7. Wrap-around reads for streams of non-block-aligned PCM, and WMA
   (`WMADEC` section) if the game's music uses it.
