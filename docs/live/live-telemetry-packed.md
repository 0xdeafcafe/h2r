# Live telemetry: packed frames

## Packed frames

### Version 8 live wire

The game reads discovery when it opens or reconnects its ingest socket.
It selects v8 if the server advertises `ingest_frame_versions: [8]` and
falls back to v7 otherwise. Native and Worker servers both advertise v8.

The viewer requests `frames=packed&frame_version=8`. Subscribers that
omit the version keep receiving v7. Readers dispatch on each record's
version nibble, including when a replay contains records from different
versions.

The current H2CK writer and standalone frame pages still use v7.
[Discovery and stream API](live-api-social.md#streams-websocket)

V8 keeps the length-prefixed container and v7 field grids. Each key
embeds a complete v7 key including the known machine travel. Between
keys, a forward byte-range coder writes one packet per sample, using
causal field predictions, grouped unchanged fields and adaptive symbol
frequencies.

The key resets the field history, dictionary and symbol
tables. A subscriber starts at a key and receives the following deltas
in order. An existing stream has a key at least every 150 samples.

The seeded 16-player match used about 81 bytes per player per second on
v8 and about 145 on v7, including keys and record lengths.
[Measurement workload](notes/2026-11-19-telemetry-precision.md#match-and-measurement)

After the usual ULEB record length, the first byte is
`8 | (key << 4) | (min(sid, 7) << 5)`. Slot 7 adds ULEB `sid - 7`.
A ULEB sequence follows.

A key carries one bare v7 key of stream 0,
sequence 0. A delta carries ULEB tick increment and one terminated range
payload. Each stream maintains independent prediction and probability
state.

A successful key supersedes earlier versions for that stream ID.

Frames travel bit-packed: game to server (`/v1/ingest`, binary messages),
server to readers (`/v1/stream?frames=packed`), recorded frames at rest
(frame chunks, [live-api.md](live-api-events.md#frame-chunks); before them
`GameFrame.data.packed`) and the replay fetch (`/v1/games/{id}/chunks`,
and `/frames` for older games). JSON readers can request `?format=json`
on an individual chunk or the legacy frame page. Live JSON readers use
`/v1/stream?frames=json`.

See the [precision and transfer measurements](notes/2026-11-19-telemetry-precision.md).
`crates/live/live-proto/src/pack.rs` writes and reads it;
`web/src/api/pack.ts` reads it; the golden vectors in
`web/test/fixtures/pack` (written by the Rust tests from invented frames,
read by the TypeScript tests) keep the two in step.

### Bits and numbers (v1–v7)

Bits are written least significant first into bytes, first byte first.

| code | bits |
|---|---|
| `u(n)` | `n` bits, unsigned |
| `eg(k)` | Exp-Golomb of order `k`: for `v`, let `x = v + 2^k` have `z + k + 1` bits; `z` zeros, a one, then the low `z + k` bits of `x` (least significant first). `v` below `2^44` |
| `seg(k)` | signed: zig-zag (`0, -1, 1, -2, ...` to `0, 1, 2, 3, ...`) then `eg(k)` |
| `str` | literal: `eg(3)` byte length (at most 255), then each byte `u(8)`; UTF-8 |

A container holds several records: each a LEB128 length then that many
bytes. Binary socket messages and the `application/x-h2-frames` replay
body are containers; `GameFrame.data.packed` is one record, base64url
without padding.

### Values

| value | wire | quantisation |
|---|---|---|
| position | three `seg(16)` | 1/1024 world unit (about 3 mm), coordinates under 10^9 |
| position in a delta | each axis gets one nonzero bit followed by `seg(1)` for a nonzero residual against the base | same grid |
| yaw | `u(14)` absolute | a turn in 16,384 steps, read back in (-π, π] |
| direction (aim, forward, up) | two absolute `u(14)` components | octahedral x and y, folded at z < 0, mapped to 0..16382. The midpoint 8191 gives exact cardinal axes |
| speed | `eg(6)` | 1/16 unit a second |
| hit amount | `eg(6)` | 1/256 of a vitality |
| shots | `eg(1)` | |
| stance | `u(3)` | `stand`, `crouch`, `airborne`, `vehicle`, `dead` |
| actions | `eg(0)` count (at most 8), then `u(3)` each | `grenade`, `melee`, `reload`, `jump`, `switch`, `pickup` |
| grenade | `u(1)` | `frag`, `plasma` |
| legacy device position | `u(10)` | 0..1 in 1023 steps |
| velocity | three `seg(8)` absolute. With a base, each axis gets one nonzero bit and `seg(1)` for a nonzero difference | 1/128 unit a second |
| fire | `eg(1)` | a tick mask, bit k a shot k ticks before the frame's tick (0 is not sent) |
| body | `u(6)` | 0..1 in 63 steps |
| shield | `eg(3)` | 1/32 of a shield, at most 4 |
| seat | `eg(0)` | at most 63 |
| ticks ago | `eg(0)` count (at most 8), then `eg(1)` each | one per action, at most 900 |
| hit ago | `eg(1)` | at most 900 |
| anim name | `str` | `[a-z0-9_:-]{1,64}` |
| anim frame | `eg(4)` | 1/16 of a frame, at most 32767 |
| string | `eg(3)`: index into the stream's string table | |
| id | `u(2)` form: 0 datum, `u(32)` (exactly 8 lowercase hex digits in JSON); 1 player `p<n>`, `eg(2)`; 2 anything else, a string index | |
| object kind | `u(4)` | the order of the kind table above, with `bipd` 0, `vehi` 1, `weap` 2, `eqip` 3, `scen` 4, `mach` 5, `ctrl` 6, `lifi` 7, `ssce` 8, `bloc` 9, `crea` 10, `netgame` 11 |
| object index | `eg(6)` | at most `0x7fff` |

An optional field (marked `opt` below) is a presence bit, then the value
when present.

### A frame

| field | wire |
|---|---|
| version | `u(4)`, 7 (6 has the same lists with the earlier quantisers and delta grammar. 5 still reads: a cinematic frame of version 4's, with the bit; 4: no projectiles, items or vehicle velocity, no prediction; 3: no anim; 2: none of version 3's fields either; 1: vehicles without parent and aim) |
| keyframe | `u(1)` |
| stream id | `eg(0)` |
| sequence | `eg(8)`, one more each frame of the stream |
| **keyframe:** game id | `str` literal |
| tick | `eg(20)` |
| bsp | `u(1)` present, then `eg(1)` |
| **delta:** tick | `eg(2)`, the difference from the previous frame (never negative) |
| bsp | `u(1)` changed; if so `u(1)` present, then `eg(1)` |
| **both (version 3):** cut | `u(1)` present, then `eg(16)`: the engine tick a campaign revert or restart came back to; on that one frame only |
| **both (versions 5–7):** cinematic | `u(1)`: a cinematic is playing ([Cinematics](live-telemetry-frames.md#cinematics)); every version 6 or 7 frame has it, version 5 was written only for cinematic frames (always 1) |
| new strings | `eg(1)` count (at most 1024), then each a `str` literal, appended to the table (a keyframe starts an empty one) |
| players | list |
| vehicles | list |
| actors | list |
| hits | always whole: `eg(2)` count, then each entity |
| machines | list |
| objects | list |
| projectiles (version 6) | list |
| items (version 6) | list |
| machine travel (v7) | optional section, then zero padding to the byte boundary |

A keyframe list starts with an `eg(2)` count followed by each entity's
fields in schema order. A delta also starts with the count. V7 follows with
these opcodes, shown in the order the decoder reads their bits.

| bits | entity |
|---|---|
| `1` | Edit the previous entity at the cursor. |
| `00` | Run of predicted or unchanged entities beginning at the cursor. `eg(2)` gives the run length minus one. |
| `010` | New entity with all its fields. |
| `011` | Edit a previous entity at a signed `seg(0)` displacement from the cursor. |

After an edit or run the cursor moves past the last referenced entity.
Before v7, a run starts with `0`, a new entity with `10`, and every edit
with `11` and an explicit displacement. Each edit sends its mask and the
values of the selected fields in schema order.

Entity keys match an edit to its base even after a reorder. V7 remembers the
previous matched edit mask in each list, initially zero. Runs and new entities
leave that mask intact.

For players and actors each five-bit group carries four
bits of the XOR of the current and previous masks, least significant group
first. The fifth bit requests another group. The last of five possible groups
must end the mask.

An unchanged mask costs a single zero group. The field
orders, using the zero-based indices from the entity table, are:

| list | four fields per group, first group on the left |
|---|---|
| player | 14,16,3,8 / 13,2,6,11 / 20,9,10,7 / 1,4,5,12 / 15,17,18,19 |
| actor | 14,16,3,6 / 13,2,4,11 / 20,9,10,5 / 1,7,8,12 / 15,17,18,19 |

For the other lists a zero bit reuses the preceding edit mask and a one bit
starts an explicit mask, one bit per non-key field in schema order.

**Version 7 prediction.** For a gap of 1–30 ticks, advance the positions of
players, actors, vehicles and projectiles with known velocity. With velocity
`a` in 1/128-unit/s quanta, a constant velocity gives a displacement of
`round_half_away(a * 8 * dt / 30)` position quanta per axis.

For players and datum-ID actors, two equal successive spans allow an additional
prediction when the same numeric key occupied the previous slot both times.
Keep that slot's actual yaw, aim and velocity from one sample earlier. Predict
each field as twice the previous value minus the older one.

Wrap yaw modulo
16384 and clamp each oct coordinate to 0..16382. Aim and velocity each require
two present values. When velocity can be predicted, use the mean of the last
actual velocity and predicted velocity for displacement, or
`round_half_away((a + b) * 4 * dt / 30)`.

A keyframe clears all history.
A gap greater than 30 ticks disables prediction for that span.

Runs inherit the predicted state. Edits correct every difference from the
quantised endpoint. A changed yaw or oct component with a base uses
`0, seg(0)` for differences in −64..63 or `1, u(14)` for an absolute
component.

Without a base it uses `u(14)`. Yaw differences wrap at one turn.
A matched unit's present animation phase with a present base and a span of
1–30 ticks uses one bit after the presence bit.

A one advances the base by
16 quanta per tick. A zero precedes an absolute `eg(4)`.

V7 hit positions can reuse the exact quantised position of a player in the
same frame. When there are players, a one bit followed by a four-bit ordinal
selects one from the current wire-order list. A zero bit precedes the three
absolute coordinates for other hits.

When the player list is empty the three
absolute coordinates follow directly.

**Versions 1–6.** Position uses 1/512 unit, velocity 1/64 unit/s, yaw 12
bits and each oct component 12 bits (maximum 4094). All edits send an
explicit mask. Plain position differences use three `seg(7)`, velocity
differences three `seg(4)`, and directions and yaw carry absolute codes.

**Prediction (version 6).** In a delta, the changed position of an entity
whose previous version had a velocity (players, actors, vehicles,
projectiles) is coded as its error from a prediction, three `seg(3)`, when
the frame's tick difference `dt` is at most 30 and each of the previous
velocity's components is at most 2^23 (in 1/64 unit a second); otherwise
as the plain difference, three `seg(7)`. Both sides know which before the
bits: it depends only on the previous entity and `dt`. The prediction, in
integer quanta, per axis: `p + floor((4 (a + b) dt + 15) / 30)`, where `p`
is the previous position, `a` the previous velocity and `b` the entity's
velocity in this frame (written later in the entity, so a decoder reads the
error first and adds the prediction once the entity is read; `a` again when
the new one is absent or past 2^23).

That is the mean velocity over `dt`
ticks, rounded half up: exact for constant acceleration (a grenade's arc),
and a unit walking steadily codes 0 or ±1 per axis (4 bits) where the
plain difference of 4 units a second at 15 Hz is 10 bits, and a still axis
4 bits where it was 8. `web/src/api/pack.ts` does the same arithmetic in
doubles (exact: every term stays under 2^31).

| list | fields (key in bold) |
|---|---|
| player | **index `eg(2)`**, alive `u(1)`, pos, yaw, opt vehicle id, opt species string, opt aim, opt weapon string, opt shots, opt stance, opt speed, opt actions, opt grenade; version 3 adds opt velocity, opt fire, opt body, opt shield, opt seat, opt ticks ago; version 4 opt anim name, opt anim frame (both or neither) |
| vehicle | **id**, kind string, pos, forward, up, occupied `u(1)`, opt parent id, opt aim (version 2; version 1 ends at occupied), opt velocity (version 6) |
| actor | **id**, species string, pos, yaw, opt aim, opt weapon string, opt shots, opt team string, opt vehicle id, opt stance, opt speed, opt actions, opt grenade; version 3 adds the same six as a player |
| hit | opt from id, opt to id, opt weapon string, pos, opt amount, opt kind string; version 3 adds opt hit ago |
| machine | opt node string, **id**, kind string, pos, forward, up, opt legacy device position |
| object | **kind**, **index**, gone `u(1)`, opt pos, opt forward, opt up |
| projectile (version 6) | **id**, kind string, pos, opt velocity, opt parent id |
| item (version 6) | **id**, kind string, pos, forward, up, equipment `u(1)` |

### Machine travel in v7

Machine travel is an optional section after the items list at the current
bit cursor. Its first field is `eg(2)` count minus one (1–255 entries).
Each entry is `eg(3)` placement gap, then its absolute `u(8)` position.

Placements are strictly ascending from 0 to 32767. The gap is
`index - previous_index - 1` with previous index initially -1.

The section is absent when empty and takes at least 15 bits when present.
A decoder uses the current record's length. With at most seven bits left,
only zero padding is allowed.

With more it reads the section, then
requires at most seven zero padding bits. Old v7 records end with the
same padding and still decode. The legacy 10-bit field in the
`machines` list remains part of the body.

A v7 keyframe replaces the known travel baseline. An absent section on a
keyframe means unknown travel for all devices. Travel of zero is explicit.

On a delta, an omitted index holds its latest value. A campaign cut
starts a new baseline. The server carries changes from throttled frames
and seeds the next keyframe after a reconnect and at each recorded chunk
start, including keyframes used to seek.

Playback samples adjacent
delivered frames and holds quiet devices. It draws no telemetry during
cinematics, though a keyframe can still carry the known baseline for
playback afterward.

An idle frame adds zero bytes. A single change at placement 0–7 uses
15 bits, or 1–2 additional frame bytes depending on padding. The furthest
placement in a 206-machine map takes 23 bits as a single change, or 2–3
additional bytes.

In a batch, a gap of 0–7 costs 12 bits per entry plus the
shared count.

### Streams, keyframes and gaps

An encoder starts with a keyframe and sends one again every N frames (the
game: 150, v7 subscriber stream: 15, v8 subscriber stream: 150). It also
sends one when the game ID changes, ticks go backwards, a checkpoint cuts
the recording, or the string table would pass 4096 entries.
A decoder keeps the last frame and string table per stream id; a delta
whose sequence is not the next one it expects (a dropped frame, or joining
mid-stream) is a gap: it drops deltas until the next keyframe.

The reader
stream sends a joining or lagging reader every frame since the last
keyframe first, so it starts decoding at once. A frame that cannot be
packed (non-finite numbers, unknown names, too many strings) leaves the
encoder as it was.

### Compatibility

The first four bits select the decoder. Version 7 writes all seven
entity lists introduced through v6. A delta requires the preceding sequence
number and a base of the same version.

Live subscribers recover at a
keyframe, and each recorded chunk begins with one. Rust and web fixtures
cover versions 1 through 8, including old v7 records without machine travel
and v8 keys and deltas. Recorded chunks keep their original bytes.

An old JSON `GameFrame` and a later base64-packed one are both readable
through the legacy frame page.

Deploy the web reader first, then the server and Worker, then reinstall the
game sender. Existing senders work with the updated readers. Clients that
consume JSON can keep using the JSON representations.

### Finished recordings

The viewer requests the archive-capable chunk index when a match ends.
Until publication it uses the original recording. Publication switches the
reader to independently decodable H2AR segments and keeps the current
playhead, speed and pause state.

Each segment index entry may carry a
`machine_entry` seed with the last known device travel from its run. A cut,
rewind or complete travel baseline starts fresh. The seed restores quiet
doors and lifts on a seek without inserting synthetic frame rows.

The
index maps source ticks across checkpoint rewinds, so a time link can fetch its target segment directly. The viewer
loads a neighbour near a segment edge to interpolate through it and releases
distant segments after a seek.

Local servers run the offline codec on the blocking pool. The Worker runs
one bounded segment on its single-threaded Durable Object per alarm or admin
step. The raw originals and their signed index remain available after
publication and rollback. [Archive format, limits and measurements](notes/2026-11-19-telemetry-precision.md#finished-replay-archives)
and [migration procedure](replay-migration.md).
