# Halo 1 Xbox sounds (`snd!`) and the Halo 2 sound gestalt

How Halo 1's Xbox maps store sounds, read by `crates/h1sounds` through
`h1map`'s public API, decoded with the audio crate's Xbox ADPCM decoder (the
one Halo 2's mixer uses), and converted to Halo 2 `snd!` tags plus their
entries in the map's `ugh!` sound gestalt. The cache file around them is in
[h1-map-format.md](h1-map-format.md); Halo 2's sound tags are in
[sound.md](sound.md). All integers are little-endian, offsets hexadecimal.

Evidence labels, as in h1-map-format.md:

- **V — verified-on-disc:** checked by the `#[ignore]`d tests in
  `crates/h1sounds/tests/real_disc.rs` on every map of the owner's Halo 1
  disc (24 maps, 15,698 `snd!` tags) and every map of the Halo 2 disc (30).
- **S — from-source:** a public description (c20's Halo 1 *sound* tag page,
  <https://c20.reclaimers.net/h1/tags/sound/>, and the Halo 2 tag definitions
  credited in sound.md) gives the name or meaning; the reader relies on it
  but the disc doesn't prove the meaning.
- **Design:** an implementation choice.

No samples, tag dumps or tag-name lists are in the repository. Tests invent
their sounds; the disc tests read `$H1_ISO` and `$H2_DISC` at run time.

## Where the samples are

**V:** a permutation's samples data reference holds a **file offset in the
decompressed map** (`+8`) and a size (`+0`), exactly as bitmap pixels do.
Flags (`+4`) are zero. On every map the samples are one run after the
bitmaps and before the tag data (beavercreek `0x16e7200..0x22ab698`, tag
data from `0x22ab800`); no two spans overlap (sounds that share samples
share the span), and none reaches the tag data. Xbox Halo 1 has no
`sounds.map`; every sample is in the map that plays it.

The lip-sync (`+0x54`) and subtitle (`+0x68`) data references carry a
running offset that looks like a position in a PC-style resource file (it
advances by the samples' size plus the tag's size); it is not a file offset
in the Xbox map and is not read. 34,315 permutations have lip-sync data,
none have subtitles (V for the counts).

## `snd!` header (`0xa4` bytes)

| Offset | Field | Evidence |
|---|---|---|
| `00` | `u32` flags (bit 0 fit to ADPCM block size, bit 1 split long sound into permutations) | values 0, 1, 2, 3 seen (V); names S |
| `04` | `u16` class (table below) | V: every value's tag paths match its name |
| `06` | `u16` sample rate: 0 22,050 Hz, 1 44,100 Hz | V: only 0 and 1 |
| `08`, `0c` | `f32` minimum, maximum distance | S |
| `10` | `f32` skip fraction | S |
| `14` | `f32` × 2 random pitch bounds | S (1.0, 1.0 typical) |
| `1c`, `20` | `f32` inner, outer cone angle, radians; `2π` is no cone | S |
| `24` | `f32` outer cone gain (linear) | S |
| `28` | `f32` gain modifier | S |
| `2c` | `f32` maximum bend per second | S |
| `3c`, `40`, `44` | `f32` skip fraction, gain, pitch modifier at scale 0 | S |
| `54`, `58`, `5c` | the same at scale 1 | S |
| `6c` | `u16` encoding: 0 mono, 1 stereo | V |
| `6e` | `u16` compression: 0 none, 1 Xbox ADPCM, 2 IMA ADPCM, 3 Ogg | V: only 1 on the disc |
| `70` | `snd!` reference: promotion sound | V: 258 sounds set it |
| `80` | `u16` promotion count | S |
| `84`..`98` | runtime values | not read |
| `98` | pitch ranges block | V |

### Pitch range (`0x48`)

| Offset | Field |
|---|---|
| `00` | name, 32 bytes (`default` on most) |
| `20` | `f32` natural pitch |
| `24` | `f32` × 2 bend bounds |
| `2c` | `u16` actual permutation count |
| `30`..`3c` | runtime |
| `3c` | permutations block |

### Permutation (`0x7c`)

| Offset | Field |
|---|---|
| `00` | name, 32 bytes |
| `20` | `f32` skip fraction |
| `24` | `f32` gain (linear) |
| `28` | `u16` compression (V: always the sound's) |
| `2a` | `u16` next permutation, `ffff` none |
| `2c`..`40` | runtime |
| `40` | samples data reference (20 bytes) |
| `54` | lip-sync data reference |
| `68` | subtitle data reference |

**Playable permutations (V):** the first *actual permutation count*
permutations of a range start a sound; one whose `next` is set is continued
by that permutation's samples, and so on (10,807 of 72,558 permutations
continue another). `Sound::chains` follows them and refuses chains that
leave the range or loop. A playable permutation's samples are its chain's,
concatenated; every chain is whole ADPCM blocks.

## Decoding

`decode::decode_chain` concatenates a chain's samples and decodes them with
`audio::adpcm::decode` (64 samples per 36-byte block per channel, the
hardware reading; see [sound.md](sound.md)). `Pcm::wav` writes 16-bit PCM.

```
h2 h1 sound <iso|map> [map] <tag> --out DIR [--force]
```

prints the sound's class, rate, channels, codec and ranges, and writes each
playable permutation as `<tag>.r<range>p<permutation>.wav` (the permutation
is the chain's first; the same scheme as `h2 sound`).

### Census (V, whole disc)

`every_h1_sound_decodes`: 24 maps, 15,698 sounds, 72,558 permutations,
61,751 playable, **23.0 hours**, every one decoded with no error.

| Rate | Channels | Compression | Sounds | Hours |
|---|---|---|---|---|
| 22,050 | 1 | Xbox ADPCM | 14,567 | 18.40 |
| 22,050 | 2 | Xbox ADPCM | 677 | 0.52 |
| 44,100 | 2 | Xbox ADPCM | 454 | 4.14 |

No uncompressed, IMA or Ogg sound is on the disc, and no 44,100 Hz mono
sound. 53 sounds have more than one pitch range (engines); 258 promote.

## The Halo 2 gestalt (`ugh!`)

sound.md describes the four blocks `crates/tags` reads. A converter has to
rewrite the whole tag, so `h1sounds::gestalt` knows all eleven, including the
three with child blocks. **V:** on all 30 maps of the Halo 2 disc the model
accounts for every byte of the tag exactly once (`SoundGestalt::decode`
refuses anything else), and a map rebuilt with its gestalt as a built tree
decodes to the same model and every `snd!` still resolves
(`retail_gestalts_decode_and_rebuild`). Every retail gestalt is named
`i've got a lovely bunch of coconuts`.

| Block | Stride | Contents | Evidence |
|---|---|---|---|
| 0 | `0x38` | Playback parameters, indexed by `snd!` `+6` | stride V; fields S (below) |
| 1 | `0x14` | Scales, indexed by `snd!` `+0xb` | stride V; fields S |
| 2 | 4 | Import names: string IDs | V |
| 3 | 10 | Pitch range parameters | stride V; fields S. Retail maps leave the two padding bytes after an odd count unzeroed |
| 4 | 12 | Pitch ranges | V (sound.md) |
| 5 | 16 | Permutations | V (sound.md) |
| 6 | `0x34` | Custom playbacks: child blocks at `+0x14` (stride `0x48`) and `+0x1c` (`0x30`) | V by coverage |
| 7 | 1 | Runtime permutation flags, one bit per permutation | V (below) |
| 8 | 12 | Chunks | V (sound.md) |
| 9 | `0x1c` | Promotions: child blocks at `+0` (`0x10`) and `+8` (4) | V by coverage |
| 10 | `0x2c` | Extra infos (lip-sync): child blocks at `+0` (no element seen; refused if present) and `+0x18` (`0x10`); a raw pointer at `+8` and an owner datum at `+0x20` | V by coverage |

Children follow their parent's elements, depth first: the order
`map::build` writes, so a rebuilt gestalt has the retail shape.

What a converted sound adds (Design, checked by read-back):

- `snd!` (20 bytes): flags, class, rate, encoding, compression (`1`), and
  indices: playback `+6`, first pitch range `+8`, range count `+0xa`, scale
  `+0xb`, promotion `+0xc` (`ff`), custom playback `+0xd` (`ff`), extra info
  `+0xe` (`ffff`), max play time `+0x10` (floor ms of the longest
  permutation). **V:** no retail `snd!` index byte points outside its block,
  `ff`/`ffff` excepted.
- One playback element and one scale element, shared with any identical
  existing element (scales are indexed by a byte).
- Import names: each range and permutation name, interned as a string ID.
- Per range: a parameters element (zeros for single-range sounds, cents
  otherwise), and a pitch range `{name, parameters, +4, +6, first
  permutation, count}`. **V:** retail ranges use two forms for `+4/+6`. With
  one permutation, `+4 = 0`, `+6 = ffff` (all but 598 of 16,942). With
  several, either `+4 = ffff` and `+6` a bit offset into block 7, every run
  inside the block and none overlapping another (all 19,183), or `+4 = 0`
  and a `+6` that isn't such an offset (29,978; not interpreted). The
  converter writes the first form: one fresh byte of block 7 per eight
  permutations.
- Per playable Halo 1 permutation, one Halo 2 permutation: name, skip
  fraction (× 32767, Design), gain (dB), its index in the range, size, and
  its chunks.
- Chunks: the samples as stored, cut every `0x7ff8` bytes (retail Halo 2's
  streaming chunk size, a whole number of mono and stereo blocks), each a
  raw resource of the map. Bit 31 of a chunk's size is never set. **V
  (retail):** 6,904 of 310,088 retail chunks have it: 4,358 are a
  permutation's only chunk, 2,487 its first or last, 59 in the middle, and
  none is `0x7ff8` long. Its meaning is open.

### Field meanings used for playback and scale (S)

Playback (`0x38`): min distance `00`, max distance `04`, skip fraction `08`,
max bend per second `0c`, gain base (dB) `10`, gain variance `14`, random
pitch bounds (`i16` cents × 2) `18`, inner and outer cone angle `1c`, `20`,
outer cone gain (dB) `24`, flags `28`, azimuth `2c`, positional gain `30`,
first-person gain `34`. Halo 1's cone angles are whole angles and halved
(retail Halo 2's omnidirectional value is π); linear gains become dB, pitch
multipliers cents.

Scale (`0x14`): gain bounds (dB) `00`, pitch bounds (`i16` cents) `08`,
skip fraction bounds `0c`, from Halo 1's scale-0 and scale-1 modifiers.

Pitch range parameters (10): natural pitch, bend bounds, max gain pitch
bounds, all `i16` cents.

## API

```rust
use h1sounds::{convert_sound, install_sounds, h1::Sound};

let s = Sound::read(&h1, tag)?;                 // Halo 1 tag, via h1map
let pcm = h1sounds::decode_all(&s, h1.bytes())?; // (range, permutation, Pcm)
let c = convert_sound(&s, h1.bytes())?;         // ConvertedSound: map-independent
h1sounds::convert::make_index_room(&mut set);   // a set from TagSet::from_map only
let ids = install_sounds(&mut set, vec![(name, c)])?;
```

- `ConvertedSound` holds the header with its indices unset, the playback and
  scale elements, and per range its parameters and permutations with their
  chunks. `dropped` lists what had no Halo 2 place (promotions, lip-sync).
- `ConvertedSound::add_to(gestalt, set) -> h1models::Converted` appends the
  entries to a `SoundGestalt`, adds the chunks to the set as raw resources,
  and returns the 20-byte `snd!` in the `Converted { group, root, raw }`
  shape; `Converted::install` adds it.
- `SoundGestalt::from_set(set)` decodes the set's pinned `ugh!` (or starts
  empty); `install(set, name)` writes it back as a built tree in the same
  slot (same datum, so references to it hold), or adds one.
- `install_sounds` does the three for a list.

### Proof (V)

`converted_sounds_read_back_sample_identical` converts onto the Halo 2
beavercreek map, builds it, and reads every converted sound back through
`tags::sound` (`read_header`, `Gestalt::resolve`, `read_encoded`) and
`audio::adpcm::decode`. Every permutation's PCM equals the Halo 1 decode,
sample for sample (ADPCM to ADPCM, nothing re-encoded); the max play time
matches; the base's own sounds still resolve to the same chunks.

| Source | Sounds | Permutations | Samples | Audio |
|---|---|---|---|---|
| beavercreek, every sound | 316 | 902 | 21,532,928 | 837.7 s |
| a30 dialogue (classes 19, 44, 46, 47; every 4th) | 257 | 1,283 | 36,806,272 | 1,669.2 s |

## Merging a moved sound (`h1sounds::merge_sound`)

A `snd!` moved to another map (`tags::schema::transplant`) still holds
indices into its source map's gestalt.
`merge_sound(src_gestalt, snd_bytes, dst_gestalt, owner, map)` appends the
entries it names to the destination gestalt and returns the `snd!` with
new indices:

- playback, scale and pitch range parameters, deduplicated;
- pitch ranges and permutations, appended in order, with import names
  mapped through `MergeMap::name` (source string ID to destination);
- chunks appended, each pointer mapped through `MergeMap::raw` (the
  caller copies local raw, or keeps shared-cache pointers);
- runtime permutation flag bits: when `+4` is `ffff`, each range gets a
  fresh byte-aligned run with the source bits copied; the other form
  (`+4` = 0) keeps `+6` as it is;
- custom playback and extra info appended whole, and the extra info's
  owner set to `owner`. An extra info's raw pointer may only be mapped to
  a stored pointer;
- sounds with a promotion (`+0xc` not `ff`) are refused
  (`Error::Unsupported`), since the promotion rules hold indices this
  doesn't map.

**Proof.** Two tests:

- `a_moved_sound_takes_its_gestalt_entries_and_flag_bits` uses invented
  gestalts.
- `every_retail_sound_merges` (**V**, 2026-11-17) merges every `snd!` of
  every retail map, one by one, into an empty gestalt. Each merged sound
  names the same entries as it did in its own gestalt: bytes, names,
  chunks and flag bits. 65,078 sounds merge; 231 with promotions are
  refused.

```sh
H2_DISC=disc/halo2.iso cargo test -p h1sounds --release --test real_disc -- --ignored every_retail_sound_merges --nocapture
```

### A whole tree with its sounds (`h1sounds::transplant`)

`h1sounds::transplant(schema, src, src_gestalt, root, dst_set,
dst_gestalt)` runs `tags::schema::transplant::transplant` and then merges
every `snd!` the tree copied into `dst_gestalt`, which the caller installs
once at the end (`SoundGestalt::install`). The `MergeMap` maps import
names through the source's string table into the set's (`TagSet::intern`)
and copies a chunk whose pointer is in the source's own file as a local
raw resource, once per pointer; shared-cache pointers stay. Two things
merging refuses are cut first instead: a promotion (the header's index
set to `ff`), and an extra info whose raw is in the source file. The
copied `snd!` is rewritten with the new indices; its extra info's owner
is its new datum.

A gestalt the set already holds as a built tag reads back through
`SoundGestalt::from_block` (the inverse of `to_block`, local chunks
included), so a set's gestalt can grow in more than one pass: donors
first, then the converted script sounds (`h1campaign`).

**Proof.** `a_built_gestalt_reads_back` (invented gestalt), and
`a_transplanted_tree_takes_its_sounds` (**V**, 2026-11-17): 04b's flood
combat elite onto 05a, built, reopened. Each of its 42 moved sounds names
the source's playback and scale, and per permutation the same import
name as a string and the same chunk bytes (354 chunks; this tree's are
all in the shared caches, so the pointers stay). One promotion is cut.

```sh
H2_DISC=disc/halo2.iso cargo test -p h1sounds --release --test real_disc -- --ignored a_transplanted_tree_takes_its_sounds --nocapture
```

## Classes

Both games number classes in one list that Halo 2 extended, so most keep
their number (`class::to_h2`). **V:** each value's tag paths on both discs
match the names below (dialogue under `sound\dialog`, weapons under
`weapons`, and so on). Halo 1 classes Halo 2 lacks go to the nearest:

| Halo 1 | Sounds | Halo 2 |
|---|---|---|
| 15 slow_particle_impacts | 63 | 14 particle_impacts |
| 27 device_force_field | 10 | 28 device_machinery |
| 29 device_nature | 0 | 29 device_stationary |
| 30 device_computers | 31 | 28 device_machinery |
| 35 ambient_computers | 0 | 34 ambient_machinery |
| 39 first_person_damage (shield hit, charge, low) | 115 | 50 game_event |
| 44 scripted_dialog_player | 913 | 47 mission_dialog |
| 45 scripted_effect (includes the multiplayer announcer) | 468 | 49 scripted_cinematic_foley |
| 46 scripted_dialog_other | 0 | 47 mission_dialog |
| 47 scripted_dialog_force_unspatialized | 33 | 48 cinematic_dialog |

Same number and name: projectile impact and detonation, weapon fire,
ready, reload, empty, charge, overheat and idle, object and particle
impacts, unit footsteps and dialog, vehicle collision and engine, device
door and machinery, music, ambient nature and machinery, game event.
Halo 2 classes with no Halo 1 source: projectile flyby, weapon melee and
animation, unit and vehicle animation, huge_ass, object looping, cinematic
music, Cortana, UI.

## Open questions

- **Adding tags to a retail base.** `map::build` never writes over the
  uninterpreted bytes after a pinned map's tag entries, so a set from
  `TagSet::from_map` cannot gain tags. `convert::make_index_room` zeroes
  those bytes (as `PinnedImage::clear_index_tail` does) and lets the entries
  use the whole index region; the read-back proof uses it. Whether the game
  needs those bytes is unknown; the map builder owns the answer.
- **Flags.** Every retail Halo 2 `snd!` has bit 12 (`0x1000`) set, most
  also `0x0c00`, and nearly all bit 1; the converter writes only Halo 1's bits
  0–1. Untested in game.
- **Pitch range `+4 = 0` form** (29,978 retail multi-permutation ranges):
  what `+6` holds there.
- **Chunk size bit 31**: set on some retail chunks, never `0x7ff8` ones; meaning open.
- **Playback, scale and pitch range parameter fields** are S; the
  conversions (dB, cents, half cone angles) are untested in game.
- **Promotions** (258 Halo 1 sounds) and **lip-sync** (34,315 permutations)
  are dropped and listed in `ConvertedSound::dropped`. Halo 2 has blocks for
  both (9, 10); filling them needs their element layouts.
- **Class 45** holds the Halo 1 multiplayer announcer as well as scripted
  effects; whether Halo 2's mixer treats `scripted_cinematic_foley` well for
  announcer lines is unchecked.
- **Gestalt limits:** pitch range, permutation and chunk indices are 16
  bits. A campaign base already holds up to 19,963 chunks; a whole Halo 1
  campaign level's sounds on top may not fit (`install_sounds` fails cleanly
  if so).
