# Halo 2 sound tags (`snd!`, `ugh!`)

Implementation: `crates/tags/src/sound.rs` (self-contained, hand-written fixed
codecs; not in `schema.rs`). Export and validation: `h2 sound` in
`crates/cli/src/sound.rs`. Evidence labels as in [map-format.md](map-format.md):
**V** verified on the owner's retail disc, **E** corroborated by the engine's
own code (XBE disassembly), **S** public-source description only.
No map bytes, tag names or audio are committed; tests are synthetic.

```sh
cargo run -p cli --release -- sound mainmenu 'sound\ui\cursor1' --out DIR --disc disc/halo2.iso
cargo run -p cli --release -- sound --all 01b_spacestation --disc disc/halo2.iso   # validate, no files
cargo run -p cli --release -- sound --all mainmenu --out DIR                       # validate and write all
```

## Model

A retail Xbox `snd!` tag is **20 bytes**. All pitch/permutation/chunk data lives
in the map's single **`ugh!`** tag (sound cache file gestalt); each map has its
own, covering that map's `snd!` tags. Sample data is raw resource bytes outside
tag memory, addressed with the same top-two-bit file selector as bitmap LODs
(map-format.md, "Raw/shared resources"). Chains: `snd!` -> pitch ranges ->
permutations -> chunks -> bytes in current map / `mainmenu` / `shared` /
`single_player_shared`.

## Tables and provenance

Public descriptions consulted (offsets/names as hypotheses; no XML or code was
copied): Assembly `Plugins/Halo2/snd!.xml` and `Plugins/Halo2/ugh!.xml` at
[`fd14ef3c46135e9422966c6a64cd926fb0bd23fb`][snd] / [ugh][ugh] (GPL-3: cross-reference
only, per the licence table in map-format.md).

| Table | Layout used | Evidence |
|---|---|---|
| `snd!` (0x14) | `+0` u16 flags, `+2` class, `+3` sample rate (0=22050, 1=44100, 2=32000), `+4` encoding (0 mono, 1 stereo, 2 codec), `+5` compression (0 BE PCM, 1 Xbox ADPCM, 2 IMA, 3 LE PCM, 4 WMA), `+6` u16 playback index, `+8` u16 first pitch range, `+0xa` u8 pitch range count, `+0x10` u32 max play time | Names/enums **S** from the sources above. **V**: rate 1 and 2 values consistent with every decoded duration; `+8/+0xa` chain to permutations whose chunk sizes sum exactly in all 4,000+ sounds of three campaign maps; `+0x10` is milliseconds (floor of longest permutation's duration; **V**, see below). **E**: engine builds 44.1 kHz ADPCM format (`0x0021e421..0x0021e454`). Class, flags, playback index, `+0xb..+0x10`: uninterpreted. |
| `ugh!` header (0x58) | Eleven `{count, address}` blocks (8 bytes each). Used: `[4]` pitch ranges, `[5]` permutations, `[8]` chunks. | Order **S**; strides **V** from pointer spacing between adjacent blocks (12, 16, 12) and the exact chunk-size sums. Other blocks (playback parameters 0x38, scales 0x14, import names 4, ...) are unread. |
| Pitch range (12) | `+8` u16 first permutation, `+10` u16 count; `+0..+8` raw | Run bounds **V** (all indices in range, sums exact). Remaining fields raw. |
| Permutation (16) | `+8` u32 encoded size, `+12` u16 first chunk, `+14` u16 chunk count; `+0..+8` raw | **V**: size == sum of its chunks' sizes for every permutation. |
| Chunk (12) | `+0` raw pointer (selector in top 2 bits), `+4` size (bit 31 is a flag of unknown meaning, masked), `+8` runtime (0xffffffff) | **V**: all pointers+sizes lie within the selected file; selector values Local and Shared observed in campaign maps. Streaming chunks are 0x7ff8 bytes. |

## Codec facts

- `Mono`/`Stereo` + `XboxAdpcm`: data is whole 36-byte (mono) / 72-byte (stereo)
  blocks. **V**: 0 of 52 (mainmenu), 16,196 (01b_spacestation), 3,842 (shared),
  16,860 (single_player_shared), 3,962 (zanzibar) permutations are not whole blocks.
- `Encoding::Codec` (value 2) is **not** Xbox ADPCM despite compression byte 1 on
  two sounds: the data begins with the ASF header GUID (`30 26 b2 75 8e 66 cf 11`),
  i.e. WMA in ASF. 480 further sounds in the multiplayer/shared maps are
  `Wma`+`Codec`. `h2 sound` decodes the retail WMAv2 profiles to PCM WAV,
  loading codec tables from the owner's verified `default.xbe` on `--disc`.
  See [wma.md](wma.md) for the clean-room provenance and the oracle comparison
  (within 1 LSB, apart from rare 2-LSB float-rounding differences).

## Samples per Xbox ADPCM block: 64, not 65 (**V**, **E**)

1. Engine. Reproduce with `h2 disasm <iso> 21e410 40` (the helper that fills
   the 18-byte format structure for the sound buffers; `[esi]` is the format):

   ```text
   0x0021e421  mov word ptr [esi],69h        ; wFormatTag = XBOX ADPCM
   0x0021e426  mov word ptr [esi+0Eh],4      ; wBitsPerSample = 4
   0x0021e443  mov [esi+2],ax                ; nChannels (helper result)
   0x0021e447  mov dword ptr [esi+4],0AC44h  ; 44100 Hz
   0x0021e44e  mov word ptr [esi+10h],2      ; cbSize = 2
   0x0021e454  mov word ptr [esi+12h],40h    ; wSamplesPerBlock = 64
   0x0021e45a  lea eax,[eax+eax*8]           ; channels*9 ...
   0x0021e45d  shl eax,2                     ; ... *4 = channels*36 = nBlockAlign
   0x0021e460  mov [esi+0Ch],ax
   ```

2. Disc. `h2 sound --all <map> --check-durations --disc <iso>` prints, for the
   sounds that decode as plain ADPCM, how many tags have `max_play_time` equal to
   the floor-ms duration of their longest permutation under 64 and under 65
   samples/block. Counts obtained:

   | Map | 64/block | 65/block |
   |---|---|---|
   | mainmenu | 33/35 | 0/35 |
   | 01b_spacestation | 4027/4173 | 0/4173 |
   | shared | 693/833 | 0/833 |
   | single_player_shared | 4013/4181 | 0/4181 |
   | zanzibar | 696/845 | 0/845 |

   Exemplar: 17,496 bytes of stereo = 243 blocks; 243*64 = 15,552 frames = 352.7
   ms and the tag says 352; 65 would give 358 ms.
3. The sounds that do not match under 64 (146 of 4173 in 01b) also never match
   under 65. The two mainmenu cases show both directions (a tag longer than its
   data; a tag shorter than data it shares with another sound). The counts show
   the mismatches do not support 65; they do not by themselves explain why those
   tags differ (not investigated further).
4. Block-boundary discontinuity is weakly informative: mean |step| at a boundary
   over interior mean is 0.995 to 1.004 under the 64 reading and 1.016 to 1.029
   under the 65 reading (13.4 million boundaries in 01b); printed by the same flag.

Verdict: 64 samples per block per channel; `SamplesPerBlock::Hardware64` is right.

## `h2 sound` behaviour

- Output is always `<tag path>.r<range>p<permutation>.wav`, including retail
  WMAv2, in one scheme for single and bulk export, so names cannot collide.
- Files are never overwritten unless `--force`; only regular files are replaced.
  Directories below `--out` are created one level at a time and refused if they
  are symlinks, and files use exclusive creation, so output cannot leave `--out`
  through a symlink (`crates/cli/src/safe_out.rs`; it cannot defend against a
  concurrent process rewriting the tree).
- `--all` exits non-zero if any sound fails, including an ASF/WMAv2 decoding
  failure or a missing/unrecognized XBE needed for the runtime tables.
- WAV sizes are checked against the RIFF 4 GiB limit before writing.
- Aliased metadata is bounded: at most 4096 permutations and 65,536 chunk
  references per sound, 256 MiB per permutation, and every chunk is checked
  against its resource file's length before any buffer is allocated.

## Output checks

Decoded WAVs (16-bit PCM) for a UI click (stereo 44.1 kHz), mono loop, and mono
32 kHz dialogue have: frame counts divisible by 64, duration equal to the tag's
floor-ms value (to rounding), no NaN (integer path), peaks well under full scale
(clipping fractions in whole-map runs are at most 6.3 percent of samples in loud
permutations, none in the three exemplars beyond 2 of 46,848 samples).

[snd]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Assembly/Plugins/Halo2/snd%21.xml
[ugh]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Assembly/Plugins/Halo2/ugh%21.xml
