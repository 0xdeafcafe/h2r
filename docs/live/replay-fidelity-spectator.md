# Replay fidelity: the spectator track

Part of [Replay fidelity: what the viewer gets, and a better track (design)](replay-fidelity.md). Covers: Proposal: the spectator track.

## Proposal: the spectator track

### Principles

1. **Game ticks, not wall time.** Every sample and event carries the game
   tick. The server never picks samples by arrival time. A replay plays at
   30 ticks a second and is exact to the tick at any speed.
2. **Keyframes and deltas, recorded in chunks.** The live stream already
   is a keyframe plus deltas (15 per keyframe, [live-telemetry.md](live-telemetry.md),
   "Packed frames"). Record that stream as it is, cut into chunks, rather
   than repacking one lone keyframe a second.
3. **Each thing at the rate it needs.** Players every tick, actors and
   vehicles less often by distance, scenery and machines only when they
   change.
4. **Events at their tick.** Shots, hits, actions and kills get the tick
   they happened on, not their frame's.
5. **Cuts are said, not guessed.** The frame after a revert says so, and
   gives the engine tick it resumed at.

### What a sample carries

On top of today's fields, all optional (an old sender still sends a valid
frame):

| field | why | cost when it changes |
|---|---|---|
| `vel` (velocity, 1/64 unit/s, delta-coded) | Hermite interpolation; dead reckoning for live | about 2 bytes |
| `pitch` folded into `aim` (already there) | first-person view | 0 |
| `anim` (animation graph state and frame, when known) | the exact pose instead of a guess | 2–3 bytes, only on change |
| `seat` (seat index in the vehicle) | driver, gunner, passenger poses | 1 byte, on change |
| `vitality` (body and shield, 6 bits each) | health bars, low-shield flash | 1–2 bytes, on change |
| `fire` (a bitmask of the ticks in this frame that fired) | each shot at its tick | 1 byte when firing |
| `powerup` (camo, overshield) | | on change |
| `cut` on the frame header: `{engine_tick}` | an explicit run boundary | once per revert |

Hits and actions get a tick offset within their frame (2 bits at 15 Hz,
0 bits at 30 Hz). Grenades in flight and slow projectiles (rockets, plasma,
needles, the brute shot) become short-lived entities with an id, a place
and a velocity, sent like vehicles. Fast bullets stay synthesized from
`fire` and aim; their flight time is one or two ticks.

### Rates

| entity | rate | notes |
|---|---|---|
| players | 30 Hz (every tick) | today 15 Hz live, 1 Hz recorded |
| vehicles | 15 Hz, 30 Hz when a player is in one | |
| actors within 30 units of a player | 15 Hz | |
| actors further away | 5 Hz | the existing nearest-first order picks who's near |
| projectiles | 30 Hz while they exist | a few at once |
| machines, scenery | when they change | the original recorder repeated them every recorded second |

The game reads every tick already. Sending players every tick means
building a frame every tick (about 15–60 µs on the worker today, under
0.2% of a core at 30 Hz) rather than every second tick.

### Recording: chunks in the blob store

The server keeps the live stream's packed frames for each game (as it does
now for readers who join, `since_key`). Every 10 seconds of game time, and
at the game's end, it writes them out as one **chunk**:

- The chunk's bytes, a keyframe and then deltas (a container, as today's
  replay body), go to the `BlobStore` at `frames/<game id>/<first tick>`.
  On the Worker that's R2; natively the filesystem store. It's immutable
  and cacheable for a year.
- The log gets a small signed `FrameChunk` event: `{from_tick, to_tick,
  frames, bytes, sha256, cuts}`. It's under 200 bytes, instead of 18 KB a
  second.
- Replays read the chunk events (a few per minute) and fetch the chunks,
  in parallel and around the seek point first. The signature covers the
  hash, so the chunks stay verifiable.

Keyframes every 10 s bound a seek to one chunk fetch plus decoding at most
10 s of deltas, which is under a millisecond.

The chunk is cut when a frame's game tick crosses the next 10 s boundary,
on that frame's ingest. A game's end and the existing lost-game sweep
flush what's left. **No timer and no background loop**, so it runs the
same in both homes. If a Durable Object is evicted mid-chunk, at most 10 s
is lost. The live stream doesn't depend on recording.

Old games keep their `GameFrame` events. The replay reads whichever the
game has, and the viewer's `GameClock` covers both.

### Budget per minute of game

Estimates from the packed format's code sizes; the first build measures
them.

| part | 1 player campaign | 16 player MP |
|---|---|---|
| players at 30 Hz, about 6 bytes a delta moving and 1 bit at rest | 11 KB | 170 KB |
| actors (30 near at 15 Hz, 60 far at 5 Hz, about 5 bytes) | 230 KB | 0 |
| vehicles (8 at 15 Hz) | 40 KB | 40 KB |
| keyframes every 10 s (5 KB, scenery included) | 30 KB | 15 KB |
| events, fire, hits | 5 KB | 20 KB |
| **total, before zstd** | **about 320 KB** | **about 250 KB** |
| total with zstd on the chunk (guess: 0.6–0.7×) | about 210 KB | about 170 KB |
| today: recorded packed / log events / viewer download | 320 KB / 1.1 MB / 1.4 MB | |

So the same bytes as today's packed recording buy 30 times the time
resolution. The viewer downloads about a seventh of what it does now. A
one-hour campaign session is about 13–20 MB in R2. That's small, but games
pile up, so:

- **Retention: everything, at full detail** (decided 2026-11-18). There's
  no thinning. The answer to games piling up is compression, below.
- **R2 costs.** About 6 puts a minute a game (Class A), and reads that
  mostly hit the CDN cache. Storage at the sizes above is pennies a month
  at current traffic.
- **The Worker.** Cutting a chunk is concatenating bytes the server has
  already packed for the stream, then one hash. That's less CPU than
  today's once-a-second keyframe repack and JWS. DO memory holds up to
  10 s of one game's packed frames (about 50–100 KB). DO SQLite loses
  the 18 KB-a-second rows, which today are most of the log's growth.
- **Native home.** The same `BlobStore` trait on the filesystem, and the
  same event in the SQLite log. The live parity suite should get a case
  that records a game, reads it back as chunks in both homes and compares
  the bytes.

### Compression (decided: the work goes here)

Every game is kept at full detail, so a chunk has to be small at rest and
quick to read back. The estimates above are a starting point. The first
chunk build measures real sizes and decode times, and the targets below
are what it has to meet:

| target | value |
|---|---|
| stored size, 1-player campaign (30 actors near) | at most 120 KB a minute |
| stored size, 16-player multiplayer | at most 100 KB a minute |
| decoding one 10 s chunk in the browser (V8, one core) | at most 2 ms |
| decoding one 10 s chunk natively | at most 0.5 ms |
| seeking to any tick | one chunk index read (cached), one chunk fetch, decoding at most 10 s |
| encoding a chunk on the Worker | at most 1 ms CPU |

Benches prove them, in CI-sized form as tests that fail past the target:

- A deterministic synthetic corpus in the repo. It has players walking,
  strafing, jumping, driving and dying, actors in groups, vehicles, and
  scenery knocked about, at 30 Hz, for a multiplayer and a campaign shape.
  Nothing in it comes from the disc or a recorded game.
- A Rust bench that encodes and decodes it, reports bytes a minute for
  each stream and decode time for each chunk, and asserts the targets
  (`--release`).
- A Bun bench and test that decode the same chunks (the golden vectors)
  in TypeScript and assert the browser target. The decoder is plain typed
  arrays, with no WASM.
- A local tool, not committed data, that runs the codec over recordings
  read from the public API, to check the synthetic corpus is honest.

How to get there, past zstd on the whole chunk, in the order to try:

1. **Prediction, not plain deltas.** At 30 Hz a moving player's position
   is close to `2·p[t−1] − p[t−2]`. Coding the error of that prediction
   rather than the difference makes most samples zero or ±1 in quantised
   units. The same goes for aim and vehicle orientation (predict the
   angular velocity) and for speed.
2. **Quantisation sized to the viewer.** Positions at 1/256 unit (about
   1 cm, half today's precision, and invisible at any camera distance).
   Aim and orientation at 10–11 bits an axis, not 12. Speed is dropped
   where `vel` is sent. Each scale is a named constant, with the viewer's
   error budget written beside it.
3. **Columns, not records.** Inside a chunk each field is its own stream
   (all players' x errors, then y, then aim, then flags, then events).
   Each stream holds values of one kind, so its statistics are tight.
4. **An entropy coder per stream.** rANS with a static frequency table
   per stream per chunk (the table costs tens of bytes). Decoding it is a
   table lookup per symbol, fast in TypeScript and Rust alike. Runs of
   "unchanged" (players standing still, actors idle) become run lengths
   before entropy coding.
5. **A trained dictionary for keyframes.** Keyframes carry the slow state
   (scenery out of place, machines, the roster, strings). A zstd
   dictionary trained on synthetic keyframes for each mode (campaign,
   multiplayer) shrinks the first keyframe of each chunk, and it ships in
   the code. If keyframes stay a small share after steps 1–4, this step is
   skipped.
6. **Transport compression stays the CDN's.** Chunks are served with a
   `Content-Encoding` only when that is smaller than the coded bytes.
   It usually won't be, after rANS.

The chunk keeps a small index at its head (each keyframe's byte offset and
tick, and each stream's offset), so a reader seeks within a chunk without
decoding what comes before the keyframe it needs. Keyframes stay every
10 s, and the 10 s spacing is a constant to tune against the size target.

### Chunk container, index and reads

Built: the chunk container, recording, the index and the reads
([live-api.md](live-api-events.md#frame-chunks)), with the codec slot for the
work above. What differs from the plan:

- **The index is the log.** Each chunk's `FrameChunk` event is its index
  entry (window, frames, size, hash, packed versions, players present,
  cuts, hits and shots); `GET /v1/games/{id}/chunks` reads them in one
  store query. One keyframe heads each chunk, so no in-chunk index.
- **Codec 1 is raw deflate over the packed records, opaque to the frame
  version** (other lanes are changing the frame: version 5's fields, a
  cinematic marker). The browser inflates it with `DecompressionStream`.
  Steps 1 to 4 above need the codec to know the fields, so they are a
  codec 2, written against the frame version once it settles. The header's
  codec byte lets old and new chunks sit side by side.
- **Each chunk starts with a key.** It repeats the latest known machine
  poses and travel so a reader can start there.
- **`GameFrame` recording stopped** for new frames. The viewer reads
  chunks and uses the legacy `/frames` endpoint for games recorded before
  chunks. The endpoint still has a JSON view for those games.

Measured and expected sizes:

| | multiplayer, 16 players | campaign, 60 actors | bench scene (100 actors, 64 vehicles, 200 objects) |
|---|---|---|---|
| packed, per frame (deltas; `pack_tests`) | about 300 B | about 900 B | 1690 B (measured) |
| chunk raw, a minute at 15 Hz | about 260 KB | about 800 KB | 1.56 MB (measured) |
| stored (deflate) | about 0.85–0.9× raw: 220–240 KB | 700–750 KB | 0.93× (measured) |
| target | 100 KB | 120 KB | |
| original `GameFrame` log events a minute (1 Hz) | about 190 KB | about 550 KB | |

The first two columns are estimates from `pack_tests`' per-frame sizes;
`cargo test -p live-proto --release chunk::tests::chunk_sizes_per_minute
-- --nocapture` prints the real numbers, per deflate level. Deflate gains
little on bit-packed records (0.93× on the bench scene), so chunks store
about what the 1 Hz log did, at fifteen times the time resolution, and
miss the targets by two to six times. Meeting them is codec 2.

Decoding a ten second chunk in the browser (`bun run bench:telemetry`,
`chunk` row, the bench scene): 39 ms median on a loaded machine, 150
frames at the frame decoder's 70 us each plus about 1 ms to inflate.
Against the 2 ms target, the frame decoder, not the inflate, is the cost.
Codec 2's columns and prediction make the per-frame work a table lookup
per value, which is what that target assumes.

Query latency, expected: the index is one store read by stream and type
(DO SQLite on the Worker, SQLite natively; well under a millisecond plus
the request), a chunk read is one R2 or file get (R2 tens of ms cold;
anonymous reads are immutable, so the CDN keeps them after the first).
A seek is the two in series; a whole replay is the index then the chunks,
six at a time.

### Interpolation and extrapolation in the viewer

- **Replays** sample the chunk track at the clock's fractional tick, with
  cubic Hermite interpolation using each sample's velocity. Aim and
  orientation use nlerp/slerp. There's no exponential smoothing on top:
  with samples every tick, smoothing only adds lag. A sample marked as a
  cut, a respawn or a teleport (a jump past what its velocity explains)
  snaps instead.
- **Live** keeps a small delay (100–150 ms of game time at a real-time
  stream's pace, `FrameBuffer.delayTicks`, scaled by `streamPace`) and
  extrapolates past the newest sample by velocity for at most 250 ms, then
  holds. Light smoothing stays for live corrections only.
- **One-shots** play at their own tick: a shot's tracer leaves the muzzle
  on the tick it fired, an action's clip starts on its tick, and a hit
  flashes on its tick.
- **Seeking** fetches the chunk that holds the target, decodes from its
  keyframe and snaps. Scrubbing decodes keyframes only, until the user
  lets go.

**Vehicles and damage in the viewer (built 2026-11-19).** Recorded frames
come irregularly, 2 to 24 ticks apart (a Cyclotron game: 45 frames in a
290-tick chunk), and vehicles had no velocity, so a vehicle went in
straight lines between them and was then eased (rate 18/s) behind that.
`state/frames.ts` `vehiclePath` now follows the game's velocities when
both frames carry them (`FrameVehicle.vel`, packed v6, the telemetry
lane's) with the players' Hermite curve, and otherwise a Catmull-Rom curve
through the frames either side (straight past `HERMITE_MAX_TICKS` or a
neighbour a jump away); the 3D view takes the place as sampled
(`smoothVehicles` `exact`), easing only the axes. Hits that did no damage
(`amount` 0) are dropped from the markers (`map/events.ts` `mergeHits`):
in that Cyclotron game 1358 of 1580 hits were `guardians`, amount 0, no
attacker, runs a few ticks apart on one player, each drawn as a "0".

Asked of telemetry (the viewer reads them when they come; nothing here
changes the wire format):

- `guardians` hits with amount 0: say what they are (a soft kill
  volume's tick?) or leave them out at capture. Their `weapon` is the
  victim's own (`smg_trigger`), not the cause.
- `from` is missing on most hits (222 of 1580 in that game; 71 of 221 in
  a Blood Gulch one), so damage lines can't be drawn from the attacker:
  vehicle collisions (`vehicle`, 52), explosions and grenades especially.
  The thrower of a grenade or the driver of a vehicle would do.
- Vehicle orientation between frames is nlerped from 5 Hz samples: an
  angular velocity (or the v6 `vel` plus a heading change rate) would let
  a turning vehicle's nose follow its curve.
- `anim` names say `combat:rifle:*` for the SMG; if the graph really is
  the rifle one, fine; if not, the weapon class in the name would let the
  viewer pick the right clips.
