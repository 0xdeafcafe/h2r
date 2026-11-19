# Replay fidelity: what the viewer gets, and a better track (design)

The web viewer at live.forbes.red replays ended games from recorded
telemetry frames, in 3D and on the 2D map. It works, but it isn't yet the
best picture of a game: campaign replays flickered, motion is coarse, and
shots, jumps and deaths land up to a second away from where they really
happened. This page measures what we record today, says why the replay
looks the way it does, and proposes a denser, tick-exact "spectator track"
for both homes. **Only the game clock (below) is built.** The rest is a
proposal for review.

It sits next to [theater.md](../game/theater.md), which records a game's
*inputs* for an exact re-simulation. The spectator track here records what
the game *looked like*, so it plays anywhere, a browser included. The two
share a clock and an event model; see [Theater](#the-theater).

## The game clock (built)

The campaign flicker had one cause. A checkpoint revert or a mission
restart continues the same game ([live-telemetry.md](live-telemetry.md),
"Campaign"), but it winds the engine's tick back. Frames and events carried
the engine's tick, so a game that reverted had two or more runs over the
same ticks. The viewer sorted all frames by tick, so the runs interleaved
and a player jumped between them about once a second.

The recordings show it. In `05a_deltaapproach` game `05f93f55…` the ticks go
2, 48 … 230, then 22, 50, then 6, 24 … 958: three runs. Game `3f23bbf0…`
rewinds from 3114 to 12 and from 1342 to 4. It wasn't a (0, 0, 0) read:
none of the sampled campaign frames have a player at the origin.

Fixed at both ends:

- **Capture** (`live-telemetry/src/session.rs`). After a revert, the
  match adds an even offset to the engine's tick, so the first tick sent
  comes after the last one. Frames, kills, scores and the end all use it.
  Frames stay on even ticks.
- **The origin.** A player whose unit couldn't be read, and who had no
  body or last place, used to be sent at (0, 0, 0) (a multiplayer pregame
  player, or a stale unit datum during a load). That player is now left
  out. A living player whose unit is unreadable for one tick stays where
  that same unit was last seen.
- **Viewer** (`web/src/state/replay.ts`, `GameClock`). Old recordings
  still rewind. The timeline reads frames and events in log order, and
  starts each run just after the one before. Given the events' own
  `GameFrame`s it places every event exactly, and a kill logged before its
  run's first recorded frame starts that run itself. The replay now reads
  frames only from `/frames` (step 2), so events are placed into the
  frames' runs by their order and ticks (`GameClock.place`), which is
  exact except for an event in the second before a run's first frame. `ReplayTimeline.cuts`
  lists where the runs begin. Players at exactly the origin facing +x (the
  old placeholder) are dropped from recorded and live frames.

The tests are `a_campaign_revert_does_not_wind_the_games_ticks_back` and
`a_player_with_no_known_place_is_left_out_not_sent_at_the_origin` in
session.rs, and "a recording whose ticks run backwards" in
`web/test/replay.test.ts`. Each failed before its fix.

**The rule from now on:** within a game, the *game tick* only goes up. It
is the engine's tick plus the offset for the run it's in. Every later
proposal on this page uses it.

## What we capture today (measured)

From the code, and from four campaign games and the frame API read on
2026-11-18 (`GET /v1/games/{id}/frames`, `/v1/events?stream=game-…`).

### Rate and timing

| stage | rate | clock |
|---|---|---|
| game reads a snapshot | every tick (30 Hz) | engine tick |
| game builds and sends a frame | every even tick (15 Hz) | engine tick, now game tick |
| server fans out live frames | at most 15 Hz (66 ms throttle, wall clock) | throttle on arrival time |
| server **records** a `GameFrame` | the first frame it sends in each new Unix second | wall clock |

So recorded frames are about 1 Hz, but spaced by wall-clock arrival rather
than game time. Measured gaps between recorded frames, in ticks:

| game | frames | gap range | most common | outliers |
|---|---|---|---|---|
| 05a_deltaapproach `05f93f55…` | 48 | 20–34 | 26, 28 | 2 rewinds |
| 03b_newmombasa `953f4ebc…` (first 200) | 199 | 2–82 | 18–22 | 2, 56, 58, 82 |
| 05a_deltaapproach `3f23bbf0…` (first 200) | 199 | 2–92 | 28–32 | 2 rewinds |
| a10 `57b014ca…` | 36 | 18–60 | 30–32 | 1 rewind |

The gap is 30 ticks (one second) only on average. In a slow scene it runs
from 0.6 s to 3 s, and two frames can land 2 ticks apart across a second
boundary. The viewer interpolates linearly between whatever it has.

### Fields

| thing | captured | sent | notes |
|---|---|---|---|
| position | yes | yes | 1/512 unit when packed |
| yaw | yes | yes | 12 bits |
| aim (look, with pitch) | yes | yes | players, actors and vehicles; octahedral 2 × 12 bits |
| stance | crouch amount, physics mode | `stand`/`crouch`/`airborne`/`vehicle`/`dead` | derived per frame |
| speed | position over time | horizontal only | velocity is read (`Motion.velocity`) but not sent |
| animation state | no | no | the viewer guesses from stance, speed and actions |
| weapon held | yes | tag stem | |
| firing | shot count per weapon per tick | **a count per frame**, summed per recorded second | no tick for each shot |
| grenades | count, type | `grenade` action, type | no grenade object, no arc |
| melee, jump, reload, switch, pickup | inferred from state changes | `actions` per frame | to the frame, not the tick |
| vehicle | parent datum | vehicle id | no seat (driver, gunner, passenger) |
| health and shield | no | no | hits carry damage amounts only |
| deaths | `alive`, `PlayerKilled` with tick and place | yes | actors: `dead` stance |
| projectiles | no | no | the viewer makes them up from shots and ballistics |
| damage | each hit | `hits` per frame | no tick for each hit |
| powerups, objectives | no | no | camo, overshield, flags, bombs |
| scenery out of place | yes | `objects`, all of them in every recorded frame | |
| machines (doors, lifts) | yes | moved ones, all of them in every recorded frame | |

### Size

A recorded frame is stored as one packed keyframe (base64url) in a
`GameFrame` event, which the server signs.

| measure | value |
|---|---|
| packed recorded frame | 4–5.5 KB (5.3 KB in `953f4ebc…`) |
| as a log event on `/v1/events` (base64 data + JWS repeating it + envelope) | about 18 KB |
| as JSON on `/v1/games/{id}/frames` | 44–64 KB |
| share of the JSON frame that is `objects` | 53–93% |
| share that is players | about 1% |
| per minute of game, packed | about 320 KB |
| per minute of game, as log events | about 1.1 MB |

Out-of-place scenery is sent again in full every second (300 to 480
entries per frame in these games). That repetition costs most of the
bytes. The players, the part people watch, cost almost nothing.

### Other findings

- **The viewer downloaded every frame twice** (fixed in step 2). `createReplaySession` fetches
  `/frames?format=packed` and the game's whole event stream, which holds
  the same frames as `GameFrame` events at three times the size. For a
  20-minute mission that's about 6 MB plus 22 MB.
- **`/frames` as JSON fails on big campaign games.** For `06d67d77…` and
  `3f23bbf0…` it returned 500 and 502 with the default page of 600; with
  `limit=100` it took 3–4 s for 4–6 MB. The viewer asks for packed frames
  and isn't affected, but the public JSON read is.
- **`to_tick` assumes ticks only go up.** `recorded_frame_data` stops at
  the first frame past `to_tick`. On an old rewinding recording that drops
  later runs. New games keep their ticks in order. Nothing in the web app
  passes `to_tick`.
- **Recording can stop early.** `05f93f55…` ran for six minutes but has 32
  seconds of frames before it ended as `lost`. Something stopped sending
  (a crash or a quit, most likely). Worth a look on its own; it's out of
  scope here.

## Why the replay looks the way it does

With one sample a second, at uneven times:

- Paths cut corners. A strafe or a circle around a pillar becomes a
  straight line between two points a second apart. Exponential smoothing
  (`smoothPlayers`, rate 18/s) adds a lag on top.
- Short events vanish or move. A jump that takes 0.7 s can fall between
  two samples. Shots, actions and hits are summed over the second and
  drawn at the recorded frame's tick, so a burst shows up as much as a
  second late. A kill is on its own tick, but the victim's last position
  is up to a second older.
- Animation is guesswork. Walk or run comes from speed averaged over a
  second, airborne from a stance that can miss the whole jump, aim from
  one sample.
- Vehicles bank and turn between two orientations a second apart.

No viewer polish fixes this. The track has to get denser first.

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
| machines, scenery | when they change | today the whole set every recorded second |

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

### Chunks as built (first cut, 2026-11-18)

Built: the chunk container, recording, the index and the reads
([live-api.md](live-api.md#frame-chunks)), with the codec slot for the
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
- **Each chunk's first frame lists every machine** (as recorded frames
  did), so a reader that starts at any chunk has every door.
- **`GameFrame` recording stopped** for new frames and the JSON `/frames`
  read is gone; the packed `/frames` read stays for older games. The
  viewer reads the chunks and falls back to `/frames` when a game has none.

Measured and expected sizes:

| | multiplayer, 16 players | campaign, 60 actors | bench scene (100 actors, 64 vehicles, 200 objects) |
|---|---|---|---|
| packed, per frame (deltas; `pack_tests`) | about 300 B | about 900 B | 1690 B (measured) |
| chunk raw, a minute at 15 Hz | about 260 KB | about 800 KB | 1.56 MB (measured) |
| stored (deflate) | about 0.85–0.9× raw: 220–240 KB | 700–750 KB | 0.93× (measured) |
| target | 100 KB | 120 KB | |
| today: `GameFrame` log events a minute (1 Hz) | about 190 KB | about 550 KB | |

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
- **Live** keeps a small delay (100–150 ms, `FrameBuffer.delayTicks`) and
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

## Animation state (found, static reading)

Every object has an animation manager inside its own data, and the frames
read its first channel. Addresses are guest virtual addresses in the
retail XBE, read with `h2 disasm`; no game was run. The reader is
`memory::animation`, and its fixture test is `unit_motion_fields`.

| what | where | evidence |
|---|---|---|
| manager | `object + i16 [object +0x12a]`, -1 none | `object_new` (`0x000b7b40`) builds it there (`0x000b80e0` into the constructor `0x001cafc0`) and gives it the model's graph (`0x001cb0d0`). The custom animation start (`0x001101e0`, reached from `custom_animation_relative` `0x002a3600` through `0x0011b520`) checks the offset against -1 and works on `object + offset`. 113 instructions read it sign-extended. |
| channels | manager `+0x00`, `+0x20`, `+0x40`, `0x20` bytes each | The constructor sets up three alike. The animation start (`0x001ccae0` into `0x001cb410`) passes the manager's own address to the channel set (`0x001c6470`), so the first channel is the one playing; the other two blend out the animations before it. |
| graph | manager `+0x68`, `jmad` datum | `0x001ccae0` and `0x001cb410` refuse a manager whose `+0x68` is -1 and hand it to the channel. |
| channel graph | channel `+0x00`, `jmad` datum | Written by `0x001c6470`; `0x001c6c80` and `0x001ccb40` refuse -1 and index the tag instances (`[0x004e3b44]`, stride `0x10`, data at `+8`) with it. |
| animation | channel `+0x04`: `i16` graph (-1 the channel's own, else a position in its inheritance list), `i16` index (-1 none) | `0x001c6470` refuses a reference whose high half is -1. `0x001daea0` resolves it (docs/engine/engine-map.md, "Animation graphs"). |
| frame | channel `+0x1c`, `f32` | The frame setter `0x001c6560` clamps it to 0..frame count - 1, stores it, and advances it by the tick length times the rate at `+0x18` (1 by default). |

Animations are named on the host from the loaded map's file, as tag names
are: `TagNames` keeps each `jmad`'s animation names (string IDs through
the map's string table) and its inheritance list. The guest is read for
the datum, reference and frame only, at no cost beyond four reads per
unit. Frames carry the result as `anim: {name, frame}`, for example
`{"name": "combat:rifle:any:move_front", "frame": 12.5}` (packed version
4: the name as an interned string, the frame in 1/16). The viewer's clips
come from the same names (docs/live/live-art.md), so it can play the
game's own animation at the game's own frame instead of guessing from
stance and speed.

Not yet established: what channels 2 and 3 hold during a blend, and how
the unit-level overlay block at `object + i16 [object +0x33e]` (aim and
look overlays, read by `0x0010e530`) is laid out. The first channel alone
gives the base pose. Validate on one game: a standing player should read
an `idle` animation whose frame loops, and a running one a `move_*` one.

## Why bodies floated, and the viewer now (built)

Players and AI hovered above the floor, slid, and floated in mid-air in
and around vehicles. Five causes, each fixed where it starts:

1. **Straight lines between sparse samples.** Recorded frames are about a
   second apart, and the sampler drew a straight line between them. A
   runner over a crest went through the air above it; one going down
   stairs or into a dip hung over it. On a synthetic hill (the fixture in
   `web/test/live-motion.test.ts`) the line is more than 0.15 units off
   the ground. Now, when both samples carry `vel`, the path between them
   is a cubic Hermite curve that leaves and arrives at the game's
   velocities (`frames.ts`, `hermite`; gaps up to 2 s; longer ones, and
   older senders without `vel`, stay straight).
2. **Easing on top.** `smoothPlayers` eased every sample towards the last
   shown place (18/s), so a body walking downhill trailed above its slope.
   A sample with a velocity is already on the game's path: it is now taken
   as it is (aim still eased). Older samples keep the easing.
3. **No floor.** Even a good curve between two ground points isn't the
   ground. A biped the game has on its feet (stance `stand` or `crouch`,
   alive, not riding) is now put on the level's surface under it when
   there is one between 0.6 below and 0.35 above where the samples have it
   (`viewer/ground.ts`). The probe is a downward ray on the level glb,
   machines left out, once per biped per 0.08 units moved, at most 16 an
   update, the surface's plane used in between. In the air, dead or
   riding, a body stays exactly where the frames put it. On the fixture
   hill the feet are within 0.02 of the ground at every sample.
4. **The model's own origin.** The extractor stands each biped on its
   lowest vertex, but the game draws a model at its origin, which is what
   the frames' position is. Extractor 22 writes how far it lifted the
   model (`asset.extras.lift`) and the viewer lowers the model by as much
   (`prepareBiped`), so the feet are where the game's are, not on top of
   the lowest vertex. Art before 22 has no `lift` and draws as before.
5. **Riders drawn at the vehicle's origin.** A rider's position is the
   vehicle's (the telemetry replaces it), and the viewer hid riders
   behind the vehicle. A rider is now drawn at the vehicle model's
   `seat_` node for their seat, turned with the vehicle, in the seat's
   pose (below). The node comes from the seat table (`seats` in the art's
   models table, extractor 22, by the frame's `seat` index), else from the
   animation mode's role (`warthog_d` driver, `_g` gunner, `_p` passenger,
   `scorpion_p_lf` the left-front passenger), else (older senders) a
   mounted turret's rider is its gunner and a vehicle's first rider its
   driver. With no node or no seat pose in the glb (art before 22, a
   Phantom with no seat markers) the rider stays hidden, as before.

**The game's animation.** A frame's `anim` (version 4) names the
animation the game plays, `mode:class:type:action`. The viewer maps it to
the glb's clips (`anim.ts`, `gameAnim`): `idle`, `walk`, `run`, the
strafing and backing-up states `run_left`/`run_right`/`run_back` and
`walk_*`, `crouch` and `crouch_run`/`crouch_left`/`crouch_right`/
`crouch_back`, `airborne`, `land`, `land_hard`, `grenade`, `melee`,
`reload`, `fire`, `switch`, `die`, `dead`, and for a rider `seat_<mode>`.
It plays that clip **at the game's frame**: the clip time is
`frame / 30` (a frame a tick), wrapped for loops and held at the end
otherwise, and the clip doesn't run on by itself, so it keeps the game's
pace and stops with the replay (`BipedAnimator.setLoopAt`). Between
samples the frame is interpolated when one animation runs on at about the
game's pace, else carried on from the nearer sample a frame a tick
(`blendAnim`). A strafe or a backward run plays its own clip with the
legs facing the aim; art before extractor 22 falls back to `run`/`walk`
with the legs turned to the movement as before. Without `anim` (older
senders, or a name with no clip state) the stance-and-speed guess stays.
Speed and leg heading come from `vel` when the frames have it.

**Shots on their tick.** A frame with a `fire` mask has its shots drawn
as the clock passes each one's tick (`fireTicks`, `ShotQueue`), the fire
clip with them, instead of the whole frame's count at the frame's tick.
Older frames keep the count.

What the viewer doesn't use yet: `body`/`shield`, `actions_ago` and hits'
`ago`, and live extrapolation past the newest frame.

## The theater

theater.md records a game's **inputs** (pad reads, network receives,
randomness) and, at the engine layer, **state keyframes and per-tick
actions**. It plays them back through the game itself: exact, with the
game's own cameras, but only on a machine that runs the same build. The
spectator track records **observed state**. It's lossy, and it plays
anywhere: the website, a phone, a stream overlay. Your hunch is right. The
spectator track is the web and spectator form, and films are the exact
form. They should share these:

| shared | how |
|---|---|
| **The game tick** | One clock per game: the engine tick plus a per-run offset, cut at each revert, with the cut's engine tick recorded. A film keys its streams by engine tick and run; the spectator track by game tick. The cut records map one to the other, so a film and a web replay of the same game land on the same moment. |
| **The event model** | `PlayerKilled`, `ScoreChanged`, `GameEnded`, plus the cut, on the game tick. The theater uses them as the timeline's marks and bookmarks; the web uses them for the scrubber and the killcam. |
| **Chunking and keyframes** | The film's `KEYF`/`INDX` chunks and the spectator `FrameChunk` both mean "keyframe, then forward". Seeking works the same way in both: nearest keyframe at or before, then decode or simulate forward. |
| **Film to web** | A film can be played headless with the telemetry readers attached, which gives a complete 30 Hz spectator track with everything in it. The theater can export any film to the web that way, even one recorded offline. |
| **The readers** | The telemetry memory readers (`memory.rs`) are the theater's "roster and match facts" source. |
| **The viewer UI** | The web transport (scrubber, cuts, kill marks, speeds) and its camera ideas (follow, first person from aim, killcam) are what a web film page needs. The in-game theater drives the game's own director cameras instead. |

What isn't shared: the spectator track can't re-simulate. Its free camera
looks at reconstructed models in the web's level render, not the game's
renderer. That's fine for a browser, and it's the reason films exist.

A film could also carry a spectator track as one more chunk type (`SPEC`),
so a film opens on the web without being re-run.

## The viewer: what "the best representation" needs

In order of what it would change for someone watching. Items 1–3 need the
denser track; the rest don't.

1. **Motion you can trust.** 30 Hz players with Hermite interpolation and
   no smoothing lag. Animations driven by the real animation state, and
   jumps and lands on their real ticks.
2. **Firing as it happened.** Each shot at its tick with the right
   tracer, slow projectiles flying their real path, grenades arcing, hits
   flashing on the victim when they land, and a health/shield bar over
   each player.
3. **Vehicles that drive.** Orientation at 15–30 Hz, players sitting in
   their seat (driver, gunner, passenger), turrets aiming from their own
   `aim`.
4. **A first-person camera.** Through the followed player's eyes, from
   their aim (pitch included, already captured), with their weapon. Halo
   is a first-person game, and an orbit or chase view doesn't show what
   the player saw.
5. **A killcam and a director.** "Watch kill": jump to 5 s before, from
   the killer's view, at 0.5×. An auto camera that cuts to whoever is
   fighting (hits and kills in the last seconds), for watching without
   steering.
6. **A timeline that tells the story.** Marks for kills, deaths, the
   followed player's deaths, multi-kills, zone (BSP) changes and revert
   cuts (`ReplayTimeline.cuts`, shown as a seam). Hovering shows a
   preview. Frame-step keys (`,` `.`), J/K/L, 0.25×, and jump to the next
   or previous event.
7. **Load once, play at once.** Stop fetching every frame twice (read the
   chunk track, or for old games one of `/frames` or the `GameFrame`
   events, not both). Start playing when the first chunk arrives, and fetch
   the rest around the playhead.
8. **The 2D map as an analyst's view.** Trails (the last 5 s of each
   path), kill lines from killer to victim, a heat map of time spent,
   sightlines from aim.
9. **Say what's missing.** When a game has only 1 Hz data, say "low
   detail recording" in the corner. A debug toggle that draws the raw
   samples as dots shows how much of the motion is interpolated.

## Plan, after review

Each step is test-first and runs in both homes.

1. **Done:** the game clock, the origin fix and the viewer guard.
2. **Done, viewer on today's data:** frames read once (`/frames` packed,
   events with `exclude=GameFrame`, so the browser no longer verifies the
   server's `GameFrame` signatures; the chunk events will be signed),
   kills, deaths, cuts and zone changes on the scrubber, frame and mark
   stepping (`,` `.` `<` `>`), 0.25×, a first-person camera (`Eyes`, key
   `v`), and a killcam (click a kill or death mark, or `k`): 5 s before, at
   0.5×, through the killer's eyes, opening the 3D view if it is closed and
   reaching a popped-out one.
3. **Done, protocol:** `vel`, `seat`, `body` and `shield`, `fire` (a
   32-tick mask), `actions_ago` and hits' `ago`, and the frame's `cut`, in
   JSON and packed format version 3 (versions 2 and 1 still read; a v2
   golden stream proves it on both sides). Dropped frames' one-shots are
   placed again against the delivering frame's tick. The telemetry session
   sends `cut` on the first frame after a revert; the other fields wait for
   capture (step 4). Filled, they cost about 25% on the sample frames
   before any of the compression work.
   **Done, research:** the animation state is found ([Animation
   state](#animation-state-found-static-reading)). It goes out as `anim` in
   packed version 4, because version 3 frames may already be recorded.
   Hits now carry the tick the damage hook saw them on (`ago`).
   **Research, alongside (decided):** find each unit's animation state in
   guest memory (the animation graph, its current state and frame) by
   static reading and the existing memory fixtures, and record it in the
   protocol as `anim`. The viewer stops inferring once senders have it.
4. **Capture:** players every tick, actor rates by distance, projectiles
   and grenades as entities, and the animation state from step 3's reader.
5. **Server:** `FrameChunk` recording to the `BlobStore`, read through the
   API with the same visibility check as `/frames` (decided), the chunk
   list endpoint, the compressed chunk codec and its benches, and the
   parity case. **First cut done** ([Chunks as built](#chunks-as-built-first-cut-2026-11-18)):
   recording, the index, the reads, codec 1 (deflate), `GameFrame`
   recording and JSON `/frames` retired, the viewer on chunks. Left: codec
   2 (steps 1–4 of Compression) and the parity case. Then retire `GameFrame` recording for new games, and
   **remove the public JSON `/frames` read** (decided: the viewer is its
   only user). The packed `/frames` read stays for older games.
6. **Viewer:** the chunk reader, Hermite sampling, one-shots on their tick,
   and live extrapolation.

## Decisions (2026-11-18)

- **Retention:** keep every game at full detail. Put the effort into
  compression ([Compression](#compression-decided-the-work-goes-here)),
  with byte and decode-time targets proved by benches.
- **JSON `/frames`:** removed once chunks exist. The packed read stays for
  games recorded before them.
- **Chunk visibility:** chunks are read through the API with the same
  check as `/frames` (friends-only co-op games stay private), never from a
  public bucket URL.
- **Animation state:** find the reader now, as research beside step 3.
