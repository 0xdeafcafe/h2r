# Telemetry cost: frame sizes and CPU per version

Evidence for [Live telemetry](../live-telemetry.md#packed-frames). Nothing
here states what the code does today; the design page does. Measured on the
development Mac in a release build, the version 6 rows estimated before they
were run; commit not recorded (moved from the design page).

## 2026-11-19: sizes, CPU and the benchmark script


Measured on the deterministic invented games in
`crates/live/live-proto/src/samples.rs` (a 16-player Slayer: 16 players, 4
vehicles, hits, weapon spawns; a campaign scene: 1 player, 60 actors, 8
vehicles, 12 machines, 25 crates moved, 16 objects gone), 450 frames each,
release build on the development Mac. Run
`cargo test --release -p live-proto frame_sizes -- --nocapture` and
`cargo test --release -p live-proto measure -- --ignored --nocapture`, and
`cargo test --release -p live-telemetry measure_frame_cost -- --ignored --nocapture`.

### Version 6 (2026-11-19, estimated)

Written on a machine with no cargo runs, so these are worked out from the
codes above, not measured; `cargo test --release -p live-proto
frame_sizes -- --nocapture` and the bench below print the real ones.

| per frame, live (deltas) | version 4 | version 6, estimate | what moved |
|---|---:|---:|---|
| samples, 16-player MP | 324 B | about 295 B | 16 players' positions predicted (about 28 to 12 bits each); adds a grenade in flight two thirds of the time, 4 to 5 dropped weapons and the flag (a run), 4 vehicles' velocity |
| samples, campaign | 931 B | about 850 B | 46 moving units predicted; adds plasma grenades (one or two in flight), 12 dropped weapons, 8 vehicles' velocity |
| bench `mp16` | about 375 B | about 340 B | 16 players and 12 driven vehicles predicted; adds 4 grenades in flight and 8 dropped weapons |
| bench `campaign100` | about 1610 B | about 1425 B | about 95 moving units and 16 vehicles predicted; adds 8 grenades, 40 dropped weapons |
| keyframe, samples MP / campaign | | +140 B / +290 B | the new lists whole (an item is about 18 B) |

At 15 Hz that is about 4.4 KB/s for a 16-player game (90 KB/s as JSON,
4.9 KB/s on version 4) and about 12.8 KB/s for a busy
campaign (14 KB/s on version 4): a viewer gets grenades, rockets, dropped
weapons and the flag for less than it paid before. The bench fixture's
units now carry their motion's own velocity (it set a fixed 4 units a
second on a path whose speed swings from 2 to 4, which a predictor would
pay for); the game's velocity is what the engine moves positions by.

Game thread: the world scan reads, for each projectile, its definition and
pose (40 bytes), velocity (12) and parent (4), and for a stuck one its
parent's position; for each loose weapon or equipment one pose read; the
loose test reuses the placement read. A few dozen reads more in a busy
scene, about 1 µs; no allocation once the kept lists have grown
(`the_game_threads_tick_does_not_allocate` covers the new lists). Worker:
the prediction is a few integer operations per moving entity.

| per frame | JSON (before) | packed | packed + deflate |
|---|---:|---:|---:|
| 16-player MP, live frame | 6026 B | 219 B (keyframe every 15) | |
| 16-player MP, keyframe / recorded | | 511 B | 514 B |
| campaign 60 actors, live frame | 24678 B | 650 B | |
| campaign, keyframe / recorded | | 2098 B | 2089 B |
| MP ingest at 15 Hz | 90 KB/s | 3.3 KB/s | |
| campaign ingest at 15 Hz | 370 KB/s | 9.8 KB/s | |
| MP stored `GameFrame` (signed event, 1 Hz) | 8629 B | 1498 B | |
| campaign stored `GameFrame` | 33105 B | 4332 B | |
| MP storage per hour | 31 MB | 5.4 MB | |
| campaign storage per hour | 119 MB | 15.6 MB | |

General-purpose compression of a packed record gains nothing (deflate:
+1% to -0.5%; the bits are already near their entropy), so recorded frames
are not compressed at rest and zstd was left out. The stored event is
bigger than its keyframe because of the JWS around it (base64 of the
payload and the signature).

| CPU per frame | JSON (before) | packed |
|---|---:|---:|
| MP, game's sink worker (serialise / pack) | 12.2 µs | 7.7 µs |
| MP, server (parse or decode, validate, pass on) | 40.3 µs | 17.8 µs |
| campaign, game's sink worker | 59.0 µs | 38.7 µs |
| campaign, server | 206 µs | 146 µs |

Packed frames are encoded once per frame for every reader, and since
2026-11-17 so is a JSON reader's `frame` message (`LiveFrame::json_message`):
before, each JSON reader cloned and serialised the frame itself, about
160 µs a frame a reader for a 47 KB campaign-sized frame.

On the game thread, for a campaign scene of 60 actors, 8 vehicles, 12
machines and 90 placed objects (synthetic guest memory): the object-table
scan takes 38 µs without the placement reads and 54 µs with them, the frame
build 14 µs. Since the third telemetry-perf lane the frame build is on the
worker too (see "Threads"), with serialising.
At 15 frames a second that is under 0.1% of a core.

### Benchmarks

`scripts/telemetry-bench.sh [out dir]` runs every stage and writes its
JSON to the directory (default `$TMPDIR/telemetry-bench`). No game data and
no game run are needed. Each stage runs alone as well:

- Game thread, sink worker and wire:
  `cargo test --release -p live-telemetry --lib bench -- --ignored --nocapture --test-threads=1`
  (`crates/live/live-telemetry/src/bench.rs`, `guest.json`). Three synthetic
  scenes in fixture guest memory: `mp16` (16 players, 2 weapons each, 12
  vehicles and 4 turrets, 5 riders, 200 placed scenery and 24 placed
  weapons, 296 objects), `mp16_1000` (the same at 984 objects) and
  `campaign100` (1 player, 100 armed actors with 6 riding, 30 bodies, 16
  vehicles and 6 turrets, 30 machines, 600 scenery, 930 objects). Since
  version 6 each also has grenades in flight (4, 4, 8) and dropped weapons
  (8, 16, 40). Each
  scene runs a minute of 30 Hz ticks through the hooks' own entry points
  (weapon fire and damage hooks, then the tick's observation and publish),
  timed and allocation-counted on the calling thread; the best of five
  runs by median frame tick is kept. After each tick, untimed, the bench
  waits for the worker to finish it (real ticks are 33 ms apart), and
  records the worker's time apart (`worker`). Then the frames are packed as
  the server sink packs them.
- Server: `cargo bench -p live-server --bench telemetry`
  (`crates/live/live-server/benches/telemetry.rs`, `server.json`): packed
  ingest at 15 Hz a game, signed event batches of 1 and 32, and stream
  fan-out to 1, 10 and 100 loopback readers (packed and JSON).
- Web: `cd web && bun run bench:telemetry` (`web/bench/telemetry.bench.ts`,
  `web.json`): packed decode, the frame hub and a repaint's sampling.

`scenes_produce_the_golden_output` (not ignored) hashes every frame of the
first twelve ticks of each scene, so a speed-up cannot change what is sent.

Game thread, median µs per tick and allocations per tick, on a loaded
development Mac (load average 40 to 80, so absolute numbers vary by 10 to
20%), before and after the 2026-11-17 telemetry-perf lane:

| scene | frame tick before | after | allocs before | after | other tick before | after | µs per second at 30 Hz before | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 60.4 | 27.7 | 332 | 284 | 16.4 | 10.2 | 1151 | 567 |
| mp16_1000 | 135.5 | 53.5 | 397 | 344 | 16.0 | 10.4 | 2272 | 958 |
| campaign100 | 213.0 | 103.5 | 922 | 891 | 2.1 | 1.6 | 3227 | 1576 |

What changed: guest scalar reads inline their fast-page case
(`runtime::Guest::read_bytes`), a vector is one 12-byte read, the object
scan reuses its lists between frame ticks (it allocated and freed about
200 KiB a frame), tag names hold their frame ids instead of parsing the
tag path per entity, and frame objects match placements in one pass. The
remaining allocations are the frame itself (an id, species, weapon and
team string per entity).

Wire on that lane's fixture, packed with a keyframe every 150 frames: `mp16` 316 B a frame (278
KiB a minute; JSON would be 7.4 MiB), `mp16_1000` 397 B (349 KiB),
`campaign100` 1251 B (1.1 MiB a minute; JSON 31 MiB). The worker packs a
frame in 13, 20 and 65 µs.

The second lane (same day) moved the scenes' fixture off `sin`/`cos`
(release builds fuse them into `sincos`, a ulp apart from debug, so the
golden hash differed between profiles), which shifts every number a
little; its before is measured on the new fixture, at a load average of
10 to 25:

| scene | frame tick before | after | allocs before | after | other tick before | after | allocs before | after | µs per second at 30 Hz before | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 26.3 | 22.0 | 284 | 219 | 9.1 | 5.7 | 85 | 20 | 531 | 416 |
| mp16_1000 | 51.7 | 46.0 | 344 | 279 | 9.5 | 5.9 | 85 | 20 | 918 | 779 |
| campaign100 | 92.2 | 77.7 | 891 | 883 | 1.5 | 1.2 | 21 | 13 | 1406 | 1184 |

What changed: object lookups read the objects array's header once per
observation, not once per datum; the session compares the roster in place
and copies it only when it changed; the roster read keeps each name from
the last tick while the player's datum and name units are unchanged; frame
ids are written without the formatter and actors lend their object state
instead of copying it. The remaining allocations are the frame itself (an
id, species, weapon, stance and team string per entity).

Wire, packed with a keyframe every 150 frames: `mp16` 325 B a frame (286
KiB a minute; JSON would be 7.4 MiB), `mp16_1000` 407 B (357 KiB),
`campaign100` 1330 B (1.2 MiB a minute; JSON 30 MiB). The worker packs a
frame in 13, 20 and 63 µs.

The third lane moved the frame build to a worker thread (see "Threads"),
so the game thread's tick is the guest reads and a queue push. Same
fixture; before is master at bbbbfe40, both at a load average
of 60 to 120 (p99 is mostly preemption at that load: master's own swings
from 0.1 to 4 ms between runs):

| scene | frame tick before | after | allocs before | after | other tick before | after | allocs before | after | µs per second at 30 Hz before | after | worker per frame |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| mp16 | 22.8 | 10.6 | 219 | 4 | 5.9 | 4.9 | 20 | 4 | 431 | 233 | 14.7 |
| mp16_1000 | 47.3 | 21.2 | 279 | 4 | 6.1 | 4.7 | 20 | 4 | 801 | 389 | 22.2 |
| campaign100 | 78.4 | 25.6 | 883 | 4 | 1.2 | 1.0 | 13 | 4 | 1194 | 399 | 54.8 |

Also in that lane: a body's position, forward and up are one guest read
and a placed object's parent and placement index another; placed kinds
come from a table; and the frame's placed-object test skips normalising
an object that is in place beyond doubt (a squared-cosine test with a
1e-9 margin; near the edge the exact test decides, so frames are
unchanged). The world scan, timed into kept lists: 4.6, 15.5 and 22.4 µs.
What is left on the game thread is reading the guest: the snapshot (about
5 µs for 16 players) and the scan, which in the fixture is mostly the
first touch of each object's body (the fixture spaces bodies 4 KiB apart;
the game packs them).

The telemetry-store lane (2026-11-18) took the last of the game thread's
per-tick costs off it. It was not measured on the machine it was written
on (no cargo runs there); the numbers below are what to expect and what to
check with the bench command above:

| scene | allocs per tick before | after | frame tick before | expected | other tick before | expected |
|---|---:|---:|---:|---:|---:|---:|
| mp16 | 4 | 0 | 10.6 | 8–10 | 4.9 | 4–5 |
| mp16_1000 | 4 | 0 | 21.2 | 18–21 | 4.7 | 4–5 |
| campaign100 | 4 | 0 | 25.6 | 22–25 | 1.0 | 1.0 |

The four allocations were the roster walk's list (`Datums::live` collects
the live player slots; the read now walks the slots in place) and the shots
map's B-tree node (a `BTreeMap` frees its nodes on `clear`; `event::Shots`
is a sorted list that keeps its room, iterated in the same order, so frames
are unchanged). The frame tick also no longer wakes the worker (the worker
polls in a game, see "Threads"): a futex wake is 1–5 µs on a loaded
machine, more when it lands on a sleeping core. An unchanged tick now reads
the clock and stops, where it read the whole roster first. The zero is
enforced by `bench::the_game_threads_tick_does_not_allocate`; the µs are
expected, so read them from `guest.json` (`frame_tick.total.p50_us`,
`other_tick.total.p50_us`, `*.allocs_per_tick`).

The fourth lane worked on the server and the web page. Server, before and
after, both at a load average of 177 to 188, so read the numbers as
directions, not measurements (the CPU-only `open+verify` step, whose code
did not change, ran 3x faster in the after run):

| stage | before | after |
|---|---:|---:|
| ingest, 64 games at 15 Hz | 1874 µs/frame (534/s) | 672 µs/frame (1489/s) |
| ingest, one game flat out | 681 µs/frame | 343 µs/frame |
| packed keyframe decode | 618 µs | 413 µs |
| events, batch of 1 | 2150 µs/event | 1487 µs/event |
| events, batch of 32 | 922 µs/event (29.5 ms/batch) | 621 µs/event (19.9 ms/batch) |
| fan-out, packed, 1 / 10 / 100 readers | 64.7 / 18.6 / 11.5 µs/frame/reader | 29.7 / 12.8 / 11.9 |

What changed: ingest takes a lock per game rather than one for all games,
and the read model's write lock once a second, not every frame. A stream
reader's token and batch timers are made once. The packed decoder reads a
word at a time, moves unchanged entities out of the previous frame instead
of cloning them, and sizes its lists up front. A batch's JWS is stored
once (`batches`) rather than in each of its events' rows: a batch of 32
writes about 15 KiB of JWS, not 490 KiB, and in memory its events share
one copy.

Web (`bun`, unloaded): decode 110 → 72 µs a frame (keyframe 232 → 159),
push 7 µs either way, repaint 160 → 95 µs (vehicles 33 → 12, machines
36 → 8, objects 43 → 26; actors 39 µs either way). Converted entities are
cached on the decoded entity, and a frame's sampled lists are made once
per frame and shared across repaints.

The fifth lane batches packed frames once for every reader: ingest appends
each game's packed frames to a pending buffer, and every 100 ms the server
publishes one tick (every game's batch, and their bytes back to back) to
readers of all games, and each game's batch alone to readers of that game,
who are not woken by the others (`crates/live/live-server/src/packed.rs`). A
reader that may see every game and follows every stream sends the tick's
bytes as they are; otherwise it copies the records that follow what it was
sent. Readers never get a frame twice or an older one (the sequence rule
in `Follow`), so a reader that joins mid-game or falls behind is sent each
stream from its keyframe, and the ticks that repeat it are trimmed; the
wire is unchanged. Fan-out CPU per frame above the no-reader run, 8 games
at 15 Hz, before and after back to back at a load average of 11 to 15:

| packed readers | before µs/frame/reader | after | total before | after |
|---|---:|---:|---:|---:|
| 1, every game | 34.3 | 17.7 | 34 | 18 |
| 10, every game | 26.8 | 11.9 | 268 | 119 |
| 100, every game | 17.5 | 9.3 | 1747 | 933 |
| 8, one game each | 14.1 | 8.2 | 113 | 65 |
| 80, one game each | 14.4 | 4.2 | 1155 | 340 |

JSON readers are unchanged (60 to 67 µs a frame a reader at 100). What
remains for a packed reader is a WebSocket send each tick and, in this
in-process bench, the reader's own receive.

