# Halo 1 Xbox sounds (`snd!`) and the Halo 2 sound gestalt

How Halo 1's Xbox maps store sounds, read by `crates/tools/h1-assets` through
`h1map`'s public API, decoded with the audio crate's Xbox ADPCM decoder (the
one Halo 2's mixer uses), and converted to Halo 2 `snd!` tags plus their
entries in the map's `ugh!` sound gestalt. The cache file around them is in
[map-format.md](map-format.md); Halo 2's sound tags are in
[sound.md](../formats/sound.md). All integers are little-endian, offsets hexadecimal.

Evidence labels, as in map-format.md:

- **V — verified-on-disc:** checked by the `#[ignore]`d tests in
  `crates/tools/h1-assets/tests/sounds/real_disc.rs` on every map of the owner's Halo 1
  disc and every map of the Halo 2 disc. The census counts are in
  [notes/sounds.md](notes/sounds.md).
- **S — unverified:** the reader relies on the name or meaning but the
  disc doesn't prove it.
- **Design:** an implementation choice.

No samples, tag dumps or tag-name lists are in the repository. Tests invent
their sounds; the disc tests read `$H1_ISO` and `$H2_DISC` at run time.

## Where the samples are

**V:** a permutation's samples data reference holds a **file offset in the
decompressed map** (`+8`) and a size (`+0`), exactly as bitmap pixels do.
Flags (`+4`) are zero. On every map the samples are one run after the
bitmaps and before the tag data; no two spans overlap (sounds that share samples
share the span), and none reaches the tag data. Xbox Halo 1 has no
`sounds.map`; every sample is in the map that plays it.

The mouth data reference at permutation `+0x54` holds the byte count.
Its virtual address at `+0x60` resolves in the Xbox tag data. The
separate resource-file offset at `+0x5c` is unused on Xbox. Permutations on the
disc have mouth data and none have subtitles.

### Dialogue mouth curves (Built)

On a10 the sounds contain authored mouth curves. Each byte
gives the aperture for one 30 Hz tick. H1 clamps the tick to the curve,
reads the byte through the permutation's tag-memory pointer and scales it
by 1/255 (`0x2821f..0x2828d`). The unit updates its aperture at `+0x298`
(`0x3b7a0`). Its pose update reads the current unit mode's slot 10 and
blends frame zero of that overlay with this weight (`0x3ee2a..0x3ee70`).

H2 requests the sound's extra-info resource for a speaking unit when its
sound class enables lip sync (`0x18c630`). It converts elapsed audio
time to 30 Hz ticks, updates the unit aperture at `+0x208` and checks for
an optional facial sequence (`0x18c720`, `0x18c9b0`, `0x18ca20`). The
signed descriptor index at permutation `+5` picks an element containing
mouth offset, mouth length, facial-sequence offset and facial-sequence
length. The sampler clamps the tick, reads the aperture byte and scales
by 1/255 (`0x218e50`). With no facial sequence, the unit looks up `talk`
in the overlay block and mixes it at that aperture (`0x1cc150`,
`0x112335..0x1123b5`). Retail 01b's marine has a one-frame talk overlay
in both combat and patrol. Actor-bound H1 conversation lines use
the native biped sound callback.

Conversion copies the H1 curve bytes into the H2 extra-info resource.
The first locator relocates the data buffer and the second relocates the
16-byte descriptors. Their indices span all pitch ranges of the sound.
An unvoiced permutation has a zero-length mouth entry when another
permutation in the sound is voiced. H1 supplies aperture and an authored
graph pose, so the H2 facial-sequence length is zero. The resource keeps
its owning `ugh!` datum through installation, transplant and compaction.
No campaign sound carrying mouth data uses a split permutation chain.
Conversion reports a split voiced chain because its segment timing has
no retail example. The ignored `a10_dialogue_mouth_curves_reach_the_retail_resource_layout`
check reads the retail locators, then compares every exported a10 curve
through the installed sound's pitch range and permutation.

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
| `70` | `snd!` reference: promotion sound | V |
| `80` | `i16` promotion count | E: `0x24eb5` and `0x24f11` |
| `84` | milliseconds credited per request | E: `0x24ed7` and `0x24f01` |
| `88`, `8c` | live promotion credit and last request time | E: `0x24ec3..0x24f3a` |
| `98` | pitch ranges block | V |

### Impulse promotion (Built)

Halo 1 checks promotion once an impulse has passed playback eligibility
(`0x26c7c..0x26c90`). `0x24ea0` subtracts elapsed mixer milliseconds from
its credit, floors the result at zero, and adds the duration at `snd! +0x84`.
A request goes through at or below count × duration. Above that threshold,
the mixer resets the credit and retries with the datum at `+0x7c`
(`0x26e09..0x26e28`). With a null datum it rejects the request and retains
the decayed credit. A count of zero bypasses the calculation.

Halo 2's `0x126ec0` promotion selects a range inside one sound
(`0x218f10`). Converted H1 sounds keep the native header and gestalt with
an appended rule containing the target datum, count, duration and two
initially zero counter words. The boot hook uses the H2 mixer clock
(`0x126a34`) and routes an eligible impulse at `0x126000`. Native impulse
allocation and permutation selection then use the selected sound. A rate
limit returns the allocator's no-sound datum. The H1 mixer has no promotion
call in its looping playback path.

The installer follows each promotion target to its source sound and assigns
all datums before installing the target fixups. This handles the retail
cycles in the small, medium and large rock impact sounds. Fresh map
construction and gestalt compaction retain the fixups. The ignored `every_retail_halo1_promotion_round_trips` test rebuilds them
all and checks the target, count and duration. The campaign and multiplayer
read-back audit checks rules on sounds reached by each converted map.
The boot unit tests cover decay, threshold crossing and rate limiting.

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
by that permutation's samples, and so on (many permutations
continue another). `Sound::chains` follows them and refuses chains that
leave the range or loop. A playable permutation's samples are its chain's,
concatenated; every chain is whole ADPCM blocks.

## Decoding

`decode::decode_chain` concatenates a chain's samples and decodes them with
`audio::adpcm::decode` (64 samples per 36-byte block per channel, the
hardware reading; see [sound.md](../formats/sound.md)). `Pcm::wav` writes 16-bit PCM.

```
h2 h1 sound <iso|map> [map] <tag> --out DIR [--force]
```

prints the sound's class, rate, channels, codec and ranges, and writes each
playable permutation as `<tag>.r<range>p<permutation>.wav` (the permutation
is the chain's first; the same scheme as `h2 sound`).

### Census (V, whole disc)

`every_h1_sound_decodes` decodes every sound of every map with no error.
Every sound is Xbox ADPCM, mono or stereo at 22,050 or 44,100 Hz; no
uncompressed, IMA or Ogg sound and no 44,100 Hz mono sound is on the disc.
A few sounds have more than one pitch range (engines) and some promote.
The counts and hours are in [notes/sounds.md](notes/sounds.md).

## The Halo 2 gestalt (`ugh!`)

sound.md describes the four blocks `crates/formats/tags` reads. A converter has to
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

- `snd!` (20 bytes): flags (below), class, rate, encoding, compression (`1`), and
  indices: playback `+6`, first pitch range `+8`, range count `+0xa`, scale
  `+0xb`, promotion `+0xc` (`ff`), custom playback `+0xd` (`ff`), extra info
  `+0xe` (`ffff`), max play time `+0x10` (floor ms of the longest
  permutation). **V:** no retail `snd!` index byte points outside its block,
  `ff`/`ffff` excepted.
- Flags: Halo 1's bits 0 and 1, plus three the engine reads.
  `0x400` takes the minimum distance from the sound class (class `+0x18`)
  instead of playback `+0x00`, `0x800` the maximum (class `+0x1c`, playback
  `+0x04`) (**E:** `0x00124fc0`, `0x00125010`, `0x0012ac20`). `0x1000`
  skips the cone, so the source's facing is never looked up (**E:**
  `0x0012adc2`). **V (every Halo 2 map):** every sound whose
  playback distances are 0 has `0x400` and `0x800`, no other has either,
  and every one has `0x1000` (all retail cones are the whole sphere). A
  Halo 1 distance of 0 is the class default (most Halo 1 sounds), so it
  gets the class bits; whole-sphere cones (every Halo 1
  sound) get `0x1000`. With the bits clear the engine takes the zeros as
  distances: the minimum is raised to 0.001 and the gain is
  `min / distance` inside a fade that a maximum of 0 leaves at 1
  (**E:** `0x0012aff0`), so a positioned sound is at -60 dB a world unit
  away (`distance_and_cone_flags_follow_the_retail_rule`,
  `tests/fields_disc.rs`).
- One playback element and one scale element, shared with any identical
  existing element (scales are indexed by a byte).
- Import names: each range and permutation name, interned as a string ID.
- Per range: a parameters element (zeros for single-range sounds, cents
  otherwise), and a pitch range `{name, parameters, +4, +6, first
  permutation, count}`. **V:** retail ranges use two forms for `+4/+6`. With
  one permutation, `+4 = 0`, `+6 = ffff` (nearly all). With
  several, either `+4 = ffff` and `+6` a bit offset into block 7, every run
  inside the block and none overlapping another, or `+4 = 0`
  and a `+6` that isn't such an offset (not interpreted; counts in
  [notes/sounds.md](notes/sounds.md)). The
  converter writes the first form: one fresh byte of block 7 per eight
  permutations.
- Per playable Halo 1 permutation, one Halo 2 permutation: name, skip
  fraction (× 32767, Design), gain (dB), its index in the range, `+6` its
  length in milliseconds (`u16`, saturating), size, and its chunks.
  `+6` is the "language neutral time". **V (retail):** 03a's dialogue
  (classes 19 and 45 to 48) has it on every permutation, at least the
  line's own length and mostly equal; every other class leaves it 0.
  **E:** `0x00218c60` returns it as a playing impulse's length (×0.001,
  seconds) when `0x0020b3c0` finds a campaign game (`[[0x4e6948]+8] == 1`)
  with the byte at options `+0x134` set; otherwise `0x00218ca0` works the
  length out from the size. Scripts read that length through
  `sound_impulse_language_time` (`0x002a9fe0` → `0x0018a240`: the ticks
  left of the sound among the 16 script impulses). A converted sound
  with `+6 = 0` would read as already over.
- Chunks: the samples as stored, cut every `0x7ff8` bytes (retail Halo 2's
  streaming chunk size, a whole number of mono and stereo blocks), each a
  raw resource of the map. Bit 31 of a chunk's size is never set. **V
  (retail):** some retail chunks have it (a permutation's only, first,
  last or, rarely, middle chunk), and none is `0x7ff8` long; counts in
  [notes/sounds.md](notes/sounds.md). Its meaning is open.

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
  chunks, and the source impulse promotion rule when present.
- `ConvertedSound::add_to(gestalt, set) -> h1models::Converted` appends the
  entries to a `SoundGestalt`, adds the chunks to the set as raw resources,
  and returns the 20-byte `snd!` in the `Converted { group, root, raw }`
  shape; `Converted::install` adds it.
- `SoundGestalt::from_set(set)` decodes the set's pinned `ugh!` (or starts
  empty); `install(set, name)` writes it back as a built tree in the same
  slot (same datum, so references to it hold), or adds one.
- `install_sounds` does the three for a list.

### Looping sounds and sound environments (`h1sounds::looping`)

```rust
let l = h1sounds::looping::read_looping(&h1, lsnd_tag)?;   // its sounds converted too
let ids = h1sounds::looping::install_looping(&mut set, &[l])?;
let e = h1sounds::looping::read_environment(&h1, snde_tag)?;
h1sounds::looping::install_environment(&mut set, &snde_tag.name, &e)?;
```

Layouts of both games' `lsnd` (header, tracks, detail sounds) and `snde`
are in the module docs. In short: Halo 1's tracks (`0xa0`) carry linear
gains and 16-byte references at `+0x30` (start), `+0x40` (loop), `+0x50`
(end), `+0x80`/`+0x90` (alternates); Halo 2's (`0x58`) carry decibels and
8-byte references from `+0x14` in the same order. `snde` is the same `0x48`
bytes in both, with four intensities linear in Halo 1 and decibels in Halo
2. `install_looping` installs every sound the looping sounds play in one
gestalt pass, skipping paths the set already has, and reuses an `lsnd` of
the same path. `tests/looping_disc.rs`: every `lsnd` and `snde`
on the Halo 1 disc converts, every named sound with it.

### Campaign loops and cache slots (**Built**)

The converter enumerates every `lsnd` in the level. It builds the tracks and
detail sounds from the Halo 2 definition, converts each distinct referenced
`snd!` once, and writes the original samples to local chunks in the level's
`ugh!`. It retains each loop as an explicit root when rebuilding or pruning
the map. Scripts, objects and BSP palettes can all reference the same datum.

**Disc evidence.** The loop counts and sample spans of the ten source
levels are in [notes/sounds.md](notes/sounds.md). Every populated track or
detail reference resolves to a sound. All ten levels leave continuous damage
empty. On a30, `vehicles\lifepod_entry\lifepod_entry` attaches
`sound\sfx\vehicles\lifeboat` to the object origin with no scale function.
Its track plays `sound\sfx\vehicles\lifeboat_entry`. The other campaign
lifepod variants have no engine attachment in the source.

**Engine evidence.** The retail header validator at `0x12289b` accepts up to
520 MiB and calls `0x214ed0` to look up the lower per-type limit. Campaign
slots 4 and 5 have a 280 MiB capacity in the row at `0x448fc8`. The allocator
at `0x2139d4` and selector at `0x214bd0` both read that row. At boot,
`boot::campaign_cache` takes the largest converted campaign map installed
across both asset variants and loose overrides. For a level over 280 MiB
it sizes both slots to the next whole MiB, up to 520 MiB, and supplies that
capacity to the validator for Halo 1 campaign headers. The utility volume
has room for both slots. The regular cache copy, checksum and sound streamer
consume the entire declared file. A conversion over 520 MiB reports its
required size.

The synthetic test `two_unreferenced_loops_share_one_sample_and_keep_existing_datums`
covers shared samples, stable references and a corrupted detail datum. The
ignored campaign disc audit follows every source loop back through the built
tracks and details, then compares each sound's permutation bytes. The cache
table and header selection have separate synthetic and disc checks.
Whether a built campaign level plays its loops in a game is not verified.

### Proof (V)

`converted_sounds_read_back_sample_identical` converts onto the Halo 2
beavercreek map, builds it, and reads every converted sound back through
`tags::sound` (`read_header`, `Gestalt::resolve`, `read_encoded`) and
`audio::adpcm::decode`. Every permutation's PCM equals the Halo 1 decode,
sample for sample (ADPCM to ADPCM, nothing re-encoded); the max play time
matches; the base's own sounds still resolve to the same chunks.

The runs cover every sound of beavercreek and a sample of a30's dialogue
(classes 19, 44, 46 and 47); sizes are in [notes/sounds.md](notes/sounds.md).

### Every sound of a built map (`h1sounds::audit`)

`audit(built, h1, retail)` follows every `snd!` of a built map through
its gestalt as the engine does and checks it plays what it was made
from: indices inside their blocks, no pitch range without permutations
and no permutation without chunks, sizes the sum of their chunks and
whole blocks, chunks inside the file. A sound named as a Halo 1 sound
must have the header, playback, scale, permutation elements and bytes
`convert_sound` makes (so it plays as long as in Halo 1); any other must
be byte for byte the same-named sound of the base or a donor. A sound
pointing at the wrong or too few bytes plays as a short repeating buzz.

`every_campaign_sound_plays_what_it_was_made_from` (h1campaign,
`H1_LEVELS`, default all ten) and `every_multiplayer_sound_plays_what_it_was_made_from`
(h1convert, `H1_MAPS`, default bloodgulch) run it on converted maps.
**V:** a10 on 05a_deltaapproach, every sound is either one `convert_sound`
makes or the base's or a donor's byte for byte, with no problem (figures in
[notes/sounds.md](notes/sounds.md)). The bytes are not the cause of a
repeated whirr in a10; that was a script ([scripts.md](scripts.md),
"Sleeping forever").

`level_loops_and_their_character` (`tests/looping_disc.rs`, `$H1_LEVEL`,
default a10) prints the length, RMS, zero crossings and the
difference-to-RMS ratio `hf` of every permutation of a level's loops, near
0 for a low drone. a10's opening cinematic and background sound are in
[notes/sounds.md](notes/sounds.md).

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
  owner set to the destination gestalt datum. Its resource can be local or shared;
- promotions carry their original rules, timers and initial state into
  a fresh destination entry. The `snd! +0xc` index points to that entry.

**Built.** Halo 2's `0x126f60` updates the timer for each rule. At
`0x218f10` the selected rule supplies a pitch range relative to the
sound, with `-1` selecting range zero. `0x219070` adds the sound's first
pitch range in the gestalt. The merge rebuilds the pitch ranges and their
permutations in source order, so the rule still selects the same samples.
The destination gets a separate timer block for each copied sound. Invalid
targets or timer blocks and promotion indices outside the signed-byte
range produce errors. Every retail promotion has one owner, one timer per rule and
sound-relative targets inside its owner (counts in
[notes/sounds.md](notes/sounds.md)).

`native_promotion_selects_the_same_samples_after_its_sound_moves`
checks the selected chunk after both indices change.
`every_retail_sound_merges` checks all retail rules and timers alongside
the existing playback, range, permutation and chunk comparisons.

```sh
H2_DISC=disc/halo2.iso cargo test -p h1sounds --release --test real_disc -- --ignored every_retail_sound_merges --nocapture
```

### A gestalt cut to its sounds (`h1convert::compact`)

A map built from scratch keeps only the `snd!` tags something reaches;
`compact::gestalt` merges each of them, in index order, into an empty
gestalt and rewrites its header. Chunk and extra-info raw keep their
resources and import names their strings (carried as tokens across
`SoundGestalt`, which keeps stored words). Compaction uses the same
merge to place the promotion entry beside its rebuilt ranges.
`compact::entries` describes a sound's entries without an index in them,
so a map and its cut compare sound by sound.

### A whole tree with its sounds (`h1sounds::transplant`)

`h1sounds::transplant(schema, src, src_gestalt, root, dst_set,
dst_gestalt)` runs `tags::schema::transplant::transplant` and then merges
every `snd!` the tree copied into `dst_gestalt`, which the caller installs
once at the end (`SoundGestalt::install`). The `MergeMap` maps import
names through the source's string table into the set's (`TagSet::intern`)
and copies each local chunk or extra-info resource once per pointer.
Shared-cache pointers keep their file selector. The transplant report counts
sounds whose promotion entries were carried over. A same-name promoted
sound may be reused when its destination promotion entry matches. A bad
entry or incompatible reuse names the donor map and sound in the error.

A gestalt the set already holds as a built tag reads back through
`SoundGestalt::from_block` (the inverse of `to_block`, local chunks
included), so a set's gestalt can grow in more than one pass: donors
first, then the converted script sounds (`h1campaign`).

The ignored `a_transplanted_tree_takes_its_sounds` check transplants 04b's
flood combat elite onto 05a and compares the moved sounds after
reopening the map, including the promotion. The synthetic
`a_reused_donor_sound_must_keep_its_promotion` check covers a same-name
sound whose destination lacks the rule.

```sh
H2_DISC=disc/halo2.iso cargo test -p h1sounds --release --test real_disc -- --ignored a_transplanted_tree_takes_its_sounds --nocapture
```

## Classes

Both games number classes in one list that Halo 2 extended, so most keep
their number (`class::to_h2`). **V:** each value's tag paths on both discs
match the names below (dialogue under `sound\dialog`, weapons under
`weapons`, and so on). Halo 1 classes Halo 2 lacks go to the nearest:

| Halo 1 | Halo 2 |
|---|---|
| 15 slow_particle_impacts | 14 particle_impacts |
| 27 device_force_field | 28 device_machinery |
| 29 device_nature | 29 device_stationary |
| 30 device_computers | 28 device_machinery |
| 35 ambient_computers | 34 ambient_machinery |
| 39 first_person_damage (shield hit, charge, low) | 50 game_event |
| 44 scripted_dialog_player | 47 mission_dialog |
| 45 scripted_effect (includes the multiplayer announcer) | 49 scripted_cinematic_foley |
| 46 scripted_dialog_other | 47 mission_dialog |
| 47 scripted_dialog_force_unspatialized | 48 cinematic_dialog |

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
- **Flags.** Bits 10 to 12 are settled (above). Nearly every retail sound
  also has bit 1; the converter keeps Halo 1's bits 0 and 1 as they are.
- **Scale gain at 0.** Halo 1 dialogue and impact sounds store a gain
  modifier of 0 at scale 0 (every `unit_dialog` sound on a10);
  `decibels` writes 0 dB for it, not silence. Retail Halo 2 impacts use
  -64 dB there; which scale Halo 2 plays unscaled dialogue at is unchecked,
  so silence there could mute dialogue.
- **Pitch range `+4 = 0` form** (many retail multi-permutation ranges):
  what `+6` holds there.
- **Chunk size bit 31**: set on some retail chunks, never `0x7ff8` ones; meaning open.
- **Playback, scale and pitch range parameter fields** are S; the
  conversions (dB, cents, half cone angles) are untested in game.
- **Class 45** holds the Halo 1 multiplayer announcer as well as scripted
  effects; whether Halo 2's mixer treats `scripted_cinematic_foley` well for
  announcer lines is unchecked.
- **Gestalt limits:** pitch range, permutation and chunk indices are 16
  bits. A campaign base already holds many chunks (the largest count is in
  [notes/sounds.md](notes/sounds.md)); a whole Halo 1
  campaign level's sounds on top may not fit (`install_sounds` fails cleanly
  if so).
