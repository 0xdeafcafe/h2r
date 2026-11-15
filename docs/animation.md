# Animation graphs (`jmad`) and animation data

This describes `crates/tags/src/anim.rs` (the `jmad` tag), `crates/tags/src/anim_sample.rs`
(decompression and sampling) and `h2 anim` in `crates/cli`. Like
[sound.md](sound.md) it is hand-written and self-contained: it does not use the
schema generator in `crates/tags`.

No game data is in the repository or in the tests. Tests build invented tags and
streams in memory; the disc check (`crates/tags/examples/anim_survey.rs`) prints
aggregate counts only.

## Evidence labels and provenance

Every layout below was derived from the retail engine's own code and the disc,
not transcribed from a community definition.

- **E** engine: the field is read at that offset by the retail animation code
  (addresses in [engine-map.md](engine-map.md), "Animation graphs").
- **V** disc: checked on every `jmad` tag of the retail disc (2,268 tags,
  77,856 animations, counted per map, so shared graphs repeat). For blocks this
  means counts times stride fit the payload and the modelled blocks never
  overlap one another.
- **S** unconfirmed name or meaning: a name chosen from general Halo-modding
  vocabulary for a field the engine reads but never labels, or for a layout that
  was recognised rather than proven. No community layout file was consulted
  while writing this; where a name is marked S it is only a label, and the
  evidence column says what was actually established.
- **raw** kept verbatim and not interpreted.

Disc validation does not prove semantics. Where it does confirm one, the
consequence is stated (for example, string-ID names of nodes, modes, classes,
types and animations all resolve to sensible words, which confirms the
hierarchy and the string-ID fields).

## Tag layout

All integers are little-endian, pointers are virtual addresses inside the tag's
own metadata region (`address - tag_base`). A block is `{count: u32, address: u32}`;
`0` count means empty and its address is ignored. Header size is `0xbc`.

| Offset | Meaning | Evidence |
|---|---|---|
| `+0x00` | tag reference to a `jmad` (group, datum), null on most graphs | V (raw meaning S) |
| `+0x08` | version-like word, `0x0006350x` on all but a few tags | V, raw |
| `+0x0c` | nodes, stride `0x20` | V; stride E (node count equals the animations' node count) |
| `+0x14` | sound references, stride `0x0c`: group, datum, u16 flags | E (`0x001c7450`: datum at `+4`, flags at `+8`) |
| `+0x1c` | effect references, stride `0x0c`, same shape | E |
| `+0x24` | blend screens, stride `0x1c` | V (size); field names S |
| `+0x2c` | animations, stride `0x6c` | E |
| `+0x34` | modes, stride `0x14` | E |
| `+0x3c`, `+0x44` | two blocks present on a minority of graphs (72 and 36 of 2,268), stride unknown | V (start pointer only), raw |
| `+0x4c` | inheritance list, stride `0x20`, datum at `+4` | E |
| `+0x54` | pairs of string IDs, stride 8 (count >= 1 on every graph) | V (disjointness), meaning unknown |
| `+0x5c..+0x88` | words, kept as `(offset, value)` pairs when nonzero | V, raw |
| `+0xac` | data resources, stride `0x14` | E |
| `+0xb4` | static poses, stride `0x18` | E |

`check_disjoint` requires every block with a modelled stride to occupy storage of
its own. A wrong stride would make neighbours overlap, so across all 2,268
graphs this is evidence for the strides listed as V.

### Nodes (stride `0x20`)

`+0x00` string-ID name, `+0x04` next sibling, `+0x06` first child, `+0x08`
parent (i16 node indices, -1 for none), `+0x0a` u16 flags, `+0x0c` u32 flags,
`+0x10` three floats, `+0x1c` float. Names are V (every one resolves to a
string such as a bone name), the links form a valid tree. Field names other than the
links are S; the three floats are zero on every node examined.

### Modes, classes, types, actions

Three nested blocks, then a leaf list, confirmed by the engine's lookup
(`0x001db170`) and by the resolved names (a mode name, a weapon class name and a weapon type name).

| Record | Stride | Fields |
|---|---|---|
| mode | `0x14` | `+0` name, `+4` block of classes, `+0xc` block (count kept, elements not modelled) |
| class | `0x14` | `+0` name, `+4` block of types, `+0xc` block (count kept) |
| type | `0x34` | `+0` name, `+4` block of actions, blocks at `+0xc`, `+0x14`, `+0x1c` (counts kept), int lists at `+0x24` and `+0x2c` (resource indices the engine preloads) |
| action | 8 | `+0` name, `+4` i16 graph index (-1: this graph, otherwise a position in the inheritance list), `+6` i16 animation index |

On the disc 70,817 actions were decoded and every non-null animation index is in
range for its graph or inheritance list.

### Animation record (stride `0x6c`)

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | string-ID name | V |
| `+0x04` | `node_list_checksum`; constant within a graph | V, S name |
| `+0x08`, `+0x0c` | two checksums | raw |
| `+0x10` | type: 0 base, 1 overlay, 2 replacement | E (0 and 1 are distinguished), 2 S; counts on disc 55,150 / 17,627 / 5,079 |
| `+0x11` | frame-info (movement) type, 0 to 3 | E (passed to the movement decoder), meanings S |
| `+0x12` | unexplained byte | raw |
| `+0x13` | node count | E |
| `+0x14` | frame count (i16); valid frames are `0..frame_count` | E |
| `+0x16` | flags; `0x10`: data lives in a cache resource | E |
| `+0x24`, `+0x28` | resource index, byte offset of the animation's blob inside it | E |
| `+0x2c` | static pose index, -1 for none | E |
| `+0x30`, `+0x32` | two animation indices the engine preloads (S: parent, next) | E |
| `+0x34`, `+0x38` | inline data size and pointer, used when flag `0x10` is clear | E; never taken on the disc |
| `+0x3c..+0x4c` | data sizes, below | E |
| `+0x4c` | frame events, stride 4: type i16, frame i16 | E |
| `+0x54` | sound events, stride 8: sound ref index i16, frame i16, raw u32 | E |
| `+0x5c` | effect events, stride 4: effect ref index i16, frame i16 | E |
| `+0x64` | a block whose count is kept | raw |

Data sizes (16 bytes at `+0x3c`): byte `+0` size of the static flag set, byte `+1`
size of the animated flag set (both `0x18` for 33 nodes: three arrays of 8
bytes), bytes `+2`, `+3`, u16 `+4` and u32 `+8` are unexplained (the engine's
sampling path does not read them), u16 `+6` is the size of the static stream and
u32 `+0xc` the size of the animated stream.

### Data resources (stride `0x14`) and static poses

Resource record: `+0` owning datum, `+4` size, `+8` raw pointer (top two bits
select the file, as for bitmaps and sounds, see [map-format.md](map-format.md)),
`+0xc` runtime handle (`0xffffffff`), `+0x10` load flag. Raw pointers on the disc
select the current map, so animation data is local; the API takes any
`ResourceReader`, as sound does. The engine reads a resource in whole,
rounded up to `0x200` bytes. One resource holds many animations.

A static pose is four i16 (rotation, normalised), three floats, one float. The
engine uses it for one transform of an animation (`0x001dd8f0`); this crate
decodes it and does not use it.

## Animation data

An animation's blob starts at `resource + data_offset` and is laid out as:

```text
static stream     static_size bytes   (present when type == base and static_size != 0)
animated stream   animated_size bytes (present when animated_size != 0)
static flag set   3 arrays            (follows the streams; read only for a static stream)
animated flag set 3 arrays            (static_flags bytes after the first set)
(movement / frame-info data follows; not decoded)
```

Each flag set holds three bit arrays (rotation, translation, scale) of
`((nodes + 31) >> 3) & !3` bytes, bit `n % 8` of byte `n / 8` for node `n`. A
set bit means the node has that channel in that stream; its **index** inside the
stream is the number of set bits before it. This was confirmed on the disc: the
channel counts in each stream header (bytes 1 to 3) equal the bit counts in every
one of the 77,856 animations (`count-mismatch 0`), which is also a hard
precondition in the decoder.

A node with a channel in neither stream has no data for it (`has_* = false`); a
caller falls back to the skeleton's default for that channel. The engine applies
the static stream first and the animated stream after, so the animated value wins
if both exist (never observed).

### Codecs

The first byte of a stream selects a record of the engine's codec table (names are
the engine's own). Disc counts are per stream, summed over all maps:

| Id | Engine name | Streams on disc |
|---|---|---|
| 1 | `uncompressed_static_data` | 55,150 (always the static stream) |
| 2 | `uncompressed_animated_data` | 0 |
| 3 | `8byte_quantized_rotation_only` | 23,055 |
| 4 | `byte_keyframe_lightly_quantized` | 22,564 |
| 5 | `word_keyframe_lightly_quantized` | 65 |
| 6 | `reverse_byte_keyframe_lightly_quantized` | 26,283 |
| 7 | `reverse_word_keyframe_lightly_quantized` | 384 |
| 8 | `blend_screen` | 5,505 |

The most common animated codec is 6 (26,283 streams), then 3, then 4. Codec 2 is
implemented from the engine code and unit-tested on invented data only.

Common header: byte 0 codec, bytes 1 to 3 the rotation, translation and scale
channel counts. Offsets are relative to the stream start. Rotations are written
`x, y, z, w` and are normalised after reading, and a quantised quaternion is four
raw i16 words (the engine normalises; it does not divide by 32767).

**1, static.** Rotation `i` at `0x20 + i*8` (i16x4), translation `i` at
`[+0x0c] + i*12`, scale `i` at `[+0x10] + i*4`. The frame is ignored.

**2, uncompressed animated; 8, blend screen.** Per-channel strides at `+0x14`
(rotation), `+0x18` (translation), `+0x1c` (scale). Rotation as four floats at
`0x20 + i*stride + frame*16`, translation as three floats at
`[+0x0c] + i*stride + frame*12`, scale as a float at
`[+0x10] + i*stride + frame*4`. Codec 8's translation function is `0x0028c750`
(shared with codec 3), not codec 2's `0x002c4da0`, and its scale function is
codec 2's `0x002c4de0`; the layout the functions read is the same, which the disc
confirms for codecs 3 and 8 (codec 2 itself never occurs).

Codec 8 clips are **aim grids**, not time series: their "frames" are the cells
of a yaw/pitch grid, blended by aim weights at runtime. `AnimationClip::is_aim_grid`
reports them; a viewer should not play them as an animation (41% of them show
rotation steps over 90 degrees between consecutive cells, against 3% to 9% for the
time-based codecs).

**3, 8-byte quantised rotation only.** Rotation as i16x4 at
`0x20 + i*[+0x14] + frame*8`; translation and scale as floats exactly as in 2.

**4 and 6, byte keyframes; 5 and 7, word keyframes.** Every channel owns a
variable number of keys. A channel word `w`: `count = w & 0xfff`, `first = w >> 12`.

| Channel | Words | Key times | Values |
|---|---|---|---|
| rotation | inline at `+0x30`, 4 bytes per channel | at `[+0x14]` + `first` | i16x4 at `[+0x20]` + `first*8` |
| translation | at `[+0x0c]` | at `[+0x18]` + `first` | floats x3 at `[+0x24]` + `first*12` |
| scale | at `[+0x10]` | at `[+0x1c]` + `first` | float at `[+0x28]` + `first*4` |

Times are u8 (4, 6) or u16 (5, 7), `first` counts keys, so it is multiplied by the
time width for the time array. The key used at a frame is the last one with
`time <= frame` (bisection); a frame between two keys interpolates linearly
(rotation: the two raw quaternions weighted by the opposite time distances, then
normalised; the others by the usual fraction). Past the last key the value holds.

On all 498,239 (codec 4), 1,046 (5), 636,811 (6) and 5,377 (7) keyed channels on the
disc the key times are strictly ascending, the first key is at frame 0 and the
last at the final frame. Codecs 6 and 7 call exactly the same engine functions as
4 and 5; the meaning of "reverse" is not established (open item below), and
treating them identically decodes every channel with that structure.

Where this crate deliberately differs from the engine:

- The engine reads `static_size` as a signed 16-bit value (`movsx` at `0x279bd2`
  and `0x279c72`). The field is kept as `u16` and values of `0x8000` and up are
  rejected.
- Before the first key (`t_lo > frame`) the engine's weights at `0x28cdb0` go
  negative and extrapolate; this crate holds the first key. Past the last key the
  engine's rotation code reads the time at index `lo + 1` without checking
  against the key count (`0x28ce1e`); this crate holds the last key. Neither
  happens on the disc (all channels start at frame 0 and end at the last frame).
- A zero or non-finite quaternion is an error (`degenerate rotation`), not
  silently replaced by identity, so damaged data shows up in `h2 anim --all`.
- Every read made for a stream must lie inside that stream's declared bytes
  (`static_size` / `animated_size`); retail data satisfies this everywhere.
- The animation's node count must equal the graph's node count.

### Sampling

`AnimationClip::sample_frame(n)` evaluates frame `n` per node. `sample(f)` clamps
`f` to `[0, frame_count - 1]` and, for a fractional frame, blends the two
neighbouring frames (rotations along the shorter arc, then normalised; the engine
itself does not flip the sign in this case, and no clip on the disc needs it).
The engine's own interpolated functions are the same piecewise-linear blend.

## Using the decoder

```rust
use tags::anim::AnimationGraph;
use tags::sound::ResourceReader; // supplies raw resource bytes; see crates/cli/src/sound.rs

let graph = AnimationGraph::read(&mut map, tag_id)?;
let idx = graph.find_animation(map.strings(), "<mode>:<action>").unwrap();
let clip = graph.load_clip(idx, &mut resources)?;       // reads the resource, builds the clip
let pose = clip.sample(12.5)?;                           // Pose { nodes: Vec<NodeTransform> }
for (node, t) in graph.nodes.iter().zip(&pose.nodes) {
    let m = t.matrix();                                  // column-major T * R * S
    // t.has_rotation / has_translation / has_scale say whether the animation
    // supplied the channel; otherwise t holds identity for it
}
```

A viewer that draws many animations of one graph should call
`read_resource` once (it returns an `Arc<[u8]>`, capped at `MAX_RESOURCE_BYTES`,
256 MiB) and `clip_from_shared` per animation: the clips borrow the resource and
copy nothing. `clip_from_resource(&[u8])` copies only the animation's declared
span (`AnimationClip::declared_len`).

Per-frame work needs no allocation: `sample_into(f, &mut pose)` and
`sample_frame_into(n, &mut pose)` reuse the pose's storage.
`Pose::world_matrices(&parents)` composes the local transforms up the node tree
(`parents` is `graph.nodes[i].parent`, negative for roots, any node order) into
column-major world matrices. Check `clip.is_aim_grid()` before playing a clip
(codec 8, see above). `NodeTransform` is plain data:
`rotation: [f32; 4]` (x, y, z, w), `translation: [f32; 3]`, `scale: f32`.
These are the animation's own channels in the node's *local* space, with
the node tree given by `graph.nodes[i].parent`. The skeleton in the render model
or the node block's base data is not combined here.

## `h2 anim`

```sh
h2 anim <map> <jmad tag>                     # summary: nodes, modes, animation list with codecs
h2 anim <map> <jmad tag> --actions           # also list every action and its animation
h2 anim <map> <jmad tag> --anim NAME         # one animation in detail
h2 anim <map> <jmad tag> --anim NAME --json PATH [--force]   # sampled transforms
h2 anim <map> <jmad tag> --json PATH         # every animation of the graph
h2 anim --all [<map>]                        # decode and sample every frame; exit status 1 on any failure (a decoder panic counts as a failure)
```

JSON is written to the path the user gives, never over an existing file without
`--force`, and not through symlinks (the same `OutDir` guard as the other exporters).
It is streamed through a buffered writer one frame at a time, so memory stays
small for any graph; a failure part-way removes the partial file.
Each animation lists its nodes (`index`, `parent`, `name`) and then, per frame, one
object per node: `rotation` (`[x,y,z,w]`), `translation`, `scale` and a `has`
object saying which channels the animation supplied. Dumps of retail graphs are
game-derived data: keep them out of the repository (`$RUSH_KEEP`, not `$TMPDIR`
or the tree).

## Validation on the retail disc

`cargo run --release -p tags --example anim_survey -- <disc.iso>` and
`h2 anim --all` decode every graph on every map, load each animation from its
resource, build the clip (which also samples the first and last frame) and sample
**every frame**. Results across the 30 maps (counted per map instance, so graphs
shared by several maps repeat):

- 2,268 graphs, 77,856 animations, 3,352,998 frames sampled, **0 failures**;
- 0 non-finite values; 0 channel-count mismatches; 0 animations with inline
  data or without data; 0 static streams on non-base animations;
- 6,432 modes, 70,817 actions, none out of range;
- clips by `(static, animated)` codec: (none, 3) 9,238; (none, 4) 3,599;
  (none, 5) 2; (none, 6) 4,362; (none, 8) 5,505; (1, 3) 13,817; (1, 4) 18,965;
  (1, 5) 63; (1, 6) 21,921; (1, 7) 384. Codec 2 never occurs.
- largest animation: 72 nodes, 1,362 frames.

Rotation continuity: the fraction of clips with a single-frame rotation step over
90 degrees is 8% for codec 3, 3% to 9% for the keyed codecs and 42% for the blend
screens (whose cells are not adjacent in time). This supports the decoding but is
not a proof of every value.

## Open items

- `reverse_*` codecs: same functions as the plain keyframe codecs. The two ints in
  each codec table record (`(1,0)`, `(2,1)`, `(2,2)`) and the engine's reason for
  separate ids are not explained.
- The movement/frame-info data after the flag sets (`frame_info_type` 1 to 3), the
  last two data-size bytes, `unknown_4`/`unknown_8`, and the blend-screen field
  semantics (only their size and finite values are checked) are not decoded.
- The blocks at `+0x3c`, `+0x44`, the `+0x54` string-ID pairs, `+0x5c..+0x88`,
  the `ik` blocks inside modes and classes, and the three unmodelled blocks of a
  weapon type are counted, not decoded.
- Inline animation data (flag `0x10` clear) is rejected: no animation on the disc
  uses it, so its layout is unverified.
- Overlay (type 1) animations are exposed as raw per-node channels; additive
  application is a runtime concern and is not implemented here.
- Quaternion handedness and the coordinate frame relative to the render model
  skeleton are not established until a viewer draws a posed model.
