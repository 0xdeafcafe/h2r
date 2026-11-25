# Facial sequences: the engine read

Evidence for the facial-sequence layout in [sound.md](../sound.md) and
for slice A5 of [rich-editors.md](../../editor/rich-editors.md).

## 2026-11-24: the reader, the sampler and the consumer

Commit: branch `rich-a5`, from `dbb711ffa`. Method: `h2 disasm
disc/default.xbe <addr> <count>` over the functions below, and the
float constants read from the XBE's sections by an offline script kept
outside the repository. Nothing was run; no retail sound was opened, so
whether any retail permutation has a non-zero facial-sequence length is
not known (**S**).

### The call

`0x18c720` (lip sync for a speaking unit) samples the mouth curve
(`0x218e50`), then calls `0x18ca20` with the unit, the sound index, the
permutation and the audio time in seconds. `0x18ca20` reads the
permutation's descriptor (signed byte `+5`, 16-byte stride) from the
resolved extra-info resource: `+0x8` facial-sequence offset into the
data buffer, `+0xc` facial-sequence length. A length of zero or less
ends it there (`0x18ca67`). It then needs the byte at `[0x4ed28c]+0x29`
set and the object to be a unit (`0xbadc0` with type 3), and calls
`0x10e480(unit, sound, &{length, pointer}, time)`.

`0x10e480` stores the sound index in the unit's face state, picks the
smoothing factor and calls the reader `0x10de40`. The face state is at
`unit + [unit+0x33e]`; the reader's `edx` is that `+0x30`:

| Face state | Type | Use |
|---|---|---|
| `+0x34` | u32 | the facial animation handle; its high half `+0x36` is `ffff` when there is none, and then nothing is read (`0x10e4db`) |
| `+0x38` | u32 | the sound index (`0x10e4e4`) |
| `+0x3c` | f32 | the time last sampled (`0x10de46`) |
| `+0x40` | f32 | a fade the consumer multiplies every weight by (`0x10e6cc`) |
| `+0x44` | i8[12] | the kept tracks' indices |
| `+0x50` | i8[12] | the kept tracks' weights, value × 128 clamped to `-128..127` (`0x10e430..0x10e471`) |

The smoothing factor is the unit tag's `+0x1e4`
(`lip_sync_onset_weight`, `defs/unit.txt`) when it is above zero, else
0.95 (`0x10e48a..0x10e4d5`).

### The layout

The reader `0x10de40` refuses a sequence of 16 bytes or less
(`0x10de53`), a null pointer, and one whose `+0x02` is not `0x096c`
(`0x10de6f`). It reads nothing at `+0x04..+0x0b`.

| Offset | Type | Meaning |
|---|---|---|
| `+0x00` | i16 | track count N |
| `+0x02` | u16 | `0x096c`, the version the reader accepts |
| `+0x04` | 8 bytes | not read |
| `+0x0c` | f32 | seconds per key-time unit (`0x10e0b3`) |
| `+0x10` | N × 4 bytes | per track: i16 key count, i16 index of its first key (`0x10e044`, `0x10e078`) |
| `+0x10 + 4N` | 4 bytes each | keys: i16 time (× `+0x0c`, seconds), i16 value (× 0.0001) |

The keys of all tracks share one array; a track's keys are the
`count` keys from its first index. The reader does not check that an
index or count stays inside the sequence's length.

### The sample at time t

For each track in order (`0x10e035..0x10e40f`):

1. A track with no keys is skipped. Tracks 18, 19 and 20 are skipped
   when the byte at `[0x4de2f4]+0x81` is set and `0x11b930` returns
   true (`0x10e4e9..0x10e507`, `0x10e063..0x10e072`); what that
   condition is was not traced.
2. With one key, or t at or before the first key, the value is the
   first key's; at or after the last key, the last key's
   (`0x10e0ec..0x10e10a`).
3. Between keys it finds the first key later than t and blends the two
   around t by `3u² − 2u³`, u the fraction of the gap, clamped to 0..1
   (`0x10e1c8..0x10e236`). A gap of 0.0001 s or less takes the earlier
   key. A result under 0.0001 in size is zero.
4. It moves the track's previous weight toward that value by the
   smoothing factor: `prev + (v − prev) × k` (`0x10e258..0x10e271`).
   The previous weights are the face state's kept tracks, each byte ×
   the float at `0x547648` (filled at run time), for track indices under
   24 (`0x10dea0..0x10dfa8`); a track not kept last time starts from 0.
5. A result under 0.0001 in size is dropped. The first 12 that are not
   are kept; after that a new one replaces the kept track with the
   smallest size when it is larger (`0x10e28b..0x10e3f6`).

The kept tracks are written to `+0x44` and `+0x50`, and the unused slots
are zero.

### What the tracks drive

The animation update `0x10e530` resolves the animation at `+0x34`
(`0x10e663..0x10e69f`), then for each of the 12 slots takes the track
index as a **frame** of that animation and its weight × the float at
`0x547648` × the fade at `+0x40`. A frame index at or past the
animation's frame count (`+0x14`), or a weight under 0.0001 in size,
is skipped; the rest are blended in as overlay frames
(`0x10e6b0..0x10e71e`, `0x1c6560`, `0x1c6f60`). So a facial sequence
is a set of weight curves, one per frame of a facial pose animation:
track i is frame i, at most 12 frames at once. Where `+0x34` is set
(which graph entry it names) was not traced.

### How to reproduce

```
h2 disasm disc/default.xbe 0x18c720 70
h2 disasm disc/default.xbe 0x18ca20 60
h2 disasm disc/default.xbe 0x10e480 50
h2 disasm disc/default.xbe 0x10de40 330
h2 disasm disc/default.xbe 0x10e663 60
```
