# The input film

Status: Design. The per-update action capture, the action-column codec, the film file's tick chunks with the update's tail, start points and the side stream's frames are built; the tunnel frame that carries the side stream is not ([theater-design.md](theater-design.md#slices)).
Code: `crates/extensions/game/src/theater/actions.rs` (`capture`, `apply`, `capture_body`, `apply_body`), `crates/live/film/src/columns.rs` (`ChunkEncoder`, `ChunkDecoder`, `FieldStats`), `crates/live/film/src/file.rs` (`FilmWriter`, `FilmReader`), `crates/live/film/src/side.rs`. Tests: `crates/extensions/game/src/theater/actions.rs` (`gathered_actions_follow_sparse_player_slots_and_replay_ignores_pad_only_rows`, `capture_and_apply_leave_the_head_and_stamp_untouched`), `crates/live/film/src/columns/tests.rs` (the tails), `crates/live/film/src/file/tests.rs`.
Switch: none.

A film is how to make a game happen again: the game options it started
from, the simulation update of every tick, and a digest per tick. The
engine reads it and nothing else does; what happened is the stats
stream's business ([theater-design.md](theater-design.md#stats-and-film)).
[film-format.md](film-format.md#the-film-file) has the file. This page
says what the update stream must hold for replay to be exact, how a tick
chunk is encoded, and what it costs in bytes and memory. The rule that
must hold: the encoding is lossless for every byte the simulation is
given.

Evidence labels: **Code** (read in this repository, path given), **Test**
(a named test), **E** (read from the retail XBE, as recorded in the linked
page), **S** (supposed, with what would verify it), **Estimate**
(arithmetic from cited numbers), **Design** (our choice).

## Halo 3 reference

From [notes/halo3-film.md](notes/halo3-film.md), which has the sources.

| | Halo 3 | This design |
|---|---|---|
| Start state | game options, both variants by value, seed, machines and 16 player configurations, in a fixed header of about 64 KB; a game state only in a clip | the engine's options block by value, a few kilobytes compressed; a game state only in a film that begins mid-game |
| Tick record | one bit-packed record per tick, beginning with an update number; its other fields are not covered by the sources | the engine's simulation update: its action block as columns per seat per chunk of 256, the rest as the bytes that changed |
| Desync check | not covered by the sources | 16 bits of the gameplay digest a tick, and the whole 64-bit digest every 256 ticks |
| Compression | none; fields are bit-packed | residual columns under zstd |
| Seek points | none in the file; campaign films cannot be rewound | none in the file; the player's ring of game-state snapshots |
| Version gate | executable type, determinism version, map signature | XBE digest, simulation contract, map digest |
| Size | 45,000 to 63,000 bytes per player-minute; 133,000 to 720,000 bytes a minute for multiplayer films | about 2,700 (a host's remote seat) to 7,700 (a local seat) bytes per player-minute, and 3,650 a minute for digests (**Estimate**) |

The comparison is not like for like: what a Halo 3 record holds beyond
its update number, and so how much of its size is input, is not
established.

## What a film holds

| Part | Holds | Home |
|---|---|---|
| **Game start** | the stamp, the game options block, the random words, the map's digest, the simulation-scope settings, the start hash | [film-format.md](film-format.md#the-film-file), [theater-design.md](theater-design.md#the-start-state) |
| **Update stream** | each update's action block and tail, and a digest per tick | this page |
| **Start point** | the arena, in a film that begins mid-game | "Start points" |
| **Extension blocks** | a feature's outside input on its tick | film-format.md |

A film holds no keyframe, no pose, no event, no score, no disk file, no
controller read and no host call.

### What the replay depends on

Everything that reaches a simulation from outside it, and what stands for
it in a film:

| Input to the simulation | Source in a played game | In a film |
|---|---|---|
| Controller reads, ports, keyboard | `XInputGetState` and the input pump | not recorded. The update's actions are what the pads became; playback gives the guest neutral pads on the options' ports |
| Remote players | packets through the socket and session code | not recorded. A remote seat's action is in the update when it is applied (`capture` in `actions.rs`) |
| Phantom seats | each phantom's action builder (`crates/extensions/game/src/bot_glue/retail_input.rs`) | not recorded. The built action is in the update |
| Players joining and leaving, property changes | the session's player array | the update's tail: the player-set change and the player entries |
| Queued simulation events, checkpoint saves | the engine's own queues | the update's tail: the event list and the save flag |
| The start state | menus, the lobby, the profile, saved games | the game options block; a start point for a resumed mission |
| Random numbers | the shared random words | the two words at the first update; after that the simulation's own draws, with the render's kept apart (`render_rng.rs`) |
| The clock | logical time | the tick count |
| Map and tag data | the disc and installed maps | the map's digest; the bytes are the player's own |
| Simulation-scope settings, the rules script | the settings store, the variant record | by value in the game start |
| A feature's outside input (Forge edits, Firefight's host log, a script reload) | the feature | an extension block on its tick |
| Live service answers, host observations, lobby operations | host calls | not recorded. None reaches a running game's simulation except through the update |

`h2 film stat FILE` prints each chunk's bytes by field.

### Joins and leaves

A join is part of the update on the tick it took effect. The update the
engine builds carries a player-set change, a mask of slots and the
players' identifiers, which the update's own apply runs on every machine
that receives it (**E**: [live-coop.md](../live/live-coop.md) sections
14.1 and 14.9), and per-player entries for the players whose properties
change. The recorder stores both as the update's tail, and playback
writes them back before the apply. A leave is the same change with the
slot removed. The seat records of a chunk (join, leave) repeat the roster
change for the Theater bar and are checked against the update's mask;
playback applies nothing from them.

**S**: the apply finds a joining player's name and appearance in the
update's player entry, with no session behind it. A join in a recorded
synchronous game replayed as a local game verifies it.

## The encoding

### What is encoded

One action is 92 bytes, read at `SIM_UPDATE_APPLY` after pads, remote
arrivals and phantom actions are resolved (`capture` in `actions.rs`).
Its fields, from the trace in [bots.md](bots.md) ("The action from a
pad"), are the encoder's columns:

| # | Field | Offset | Bytes | Kind | Changes when |
|---:|---|---|---:|---|---|
| 0 | in the update's mask | | 1 | flag | the seat joins, leaves or has no update |
| 1 | buttons | `+0x00` | 4 | bit set | a press or a release |
| 2, 3 | yaw, pitch | `+0x04`, `+0x08` | 4, 4 | `f32` angle | the player looks |
| 4, 5 | throttle | `+0x0c`, `+0x10` | 4, 4 | `f32`, −1 to 1 | the move stick moves |
| 6, 7 | primary and secondary trigger | `+0x14`, `+0x18` | 4, 4 | `f32`, 0 to 1 | a trigger moves |
| 8 | hold word, weapon-set generation | `+0x1c` | 4 | bits, counter | use, switch, a weapon change |
| 9 | weapon indices, grenade | `+0x20` | 4 | small integers | a switch |
| 10 | zoom, two untraced bytes | `+0x24` | 4 | small integer | a zoom |
| 11 | interaction: kind, detail, object, melee target | `+0x28` | 12 | handles | something to use comes in reach |
| 12 | shot assist: target and strengths | `+0x34` | 36 | handle, `f32` | a target is near the reticle |
| 13 | untraced tail | `+0x58` | 4 | opaque | not known |

Every byte is carried, traced or not, so the encoding is lossless without
knowing what the consumer reads. A seat whose action reached a host over
the network was quantised by the engine's action store on the way in
(**E**: [bots.md](bots.md#5-way-in-2-phantom-peers)); the bytes recorded
are the quantised ones, and nothing finer is stored because nothing finer
exists.

### Methods weighed

Bits per seat-tick for a local seat that is alive and playing, under the
rates in "Size, step by step". An idle seat costs under one bit a tick in
every row but the first.

| Method | Saves | Bits per active seat-tick | Decode | Verdict |
|---|---|---:|---|---|
| Full records | nothing | 736 | copy | the baseline |
| Changed seats and a 12-byte change mask per field | idle seats, idle bytes | 180; about 120 after zstd (**S**) | byte loop | replaced: the mask costs more than the changes |
| **Columns**: a structure of arrays per seat inside a chunk, each field as its residual against the previous tick | the masks; puts like bytes together; an idle field is a run of zeros | 91 before the coder | one pass per column | adopted |
| **Predictor per column per chunk**: none, previous, or linear (twice the previous minus the one before), on the field's 32-bit pattern | steady turns and ramps, where the linear residual is a few units | about 20% of the above (**S**) | the same pass | adopted; the encoder tries all three and keeps the smallest |
| **Byte planes**: the residual's four bytes split into four runs | gathers the quiet high bytes for the coder | prepares the next row | a transpose | adopted |
| **zstd per chunk** over the planes | zero runs, skewed high bytes, repeats | 58 to 72 | about 1 GB/s; no code of ours | adopted |
| **Grid models, checked per value**: where a field's values sit on a grid (a remote seat's quantised aim and throttle, a trigger made from an 8-bit pressure), store the grid index; a value off the grid is stored whole | most of a remote seat's aim | 46 to 48 local; about 16 for a remote seat | a multiply and a compare | adopted for fields where a measurement on recorded films finds a grid |
| **Dictionary** trained on recorded chunks and shipped in the app | the cold start of each small zstd frame | about 5% on chunks under 4 KB, nothing on large ones (**S**) | none | adopted only if it measures 5% or more on recorded films |
| Button edges in place of button state | nothing more: the residual of the buttons field is the set of edges | | | covered by the previous-tick predictor |
| Run lengths for idle seats and idle ticks | nothing more: an absent column costs two mask bits, a zero run a few bytes a chunk | | | covered by the field mask and zstd |
| Bit-packed tick records: fields at exact width, Rice or Elias-gamma residuals, floats as leading-zero count, length and significant bits (the table below) | whole bytes on small fields and on rare events | 88 to 114 local, 24 remote; 1 when idle | a bit reader with a branch per field, 50 to 100 MB/s | not adopted: the `f32` residuals are most of the stream and cost more here (about 32 bits against 22), and a byte-wise coder gains under 3% on a bit-packed stream |
| An adaptive range coder with a model per field | 10 to 15% over zstd on the structured fields (**S**) | about 50 | 20 to 50 MB/s; our code | not adopted: the low bytes of an `f32` residual are noise that no coder shrinks, and they are most of what is left |
| Truncated digests | 8 bytes a tick to 2 | 14,400 to 3,600 bytes a match-minute | none | adopted |
| The game start as a reference | every state byte | | | adopted |
| Recomputing the shot-assist block in playback | about 22 bits an active tick | | playback runs the target search | not adopted: it changes what playback computes |
| Recording pads in place of actions | the aim floats | | | not adopted: a pad becomes an action through frame timing and control history, which a film does not reproduce |

The bit-packed record, for the comparison:

| Bits | Field | Coding |
|---:|---|---|
| 1 | any seat changed this tick | a clear bit ends the record |
| seats | changed-seat mask | one bit per occupied seat |
| 8 | field groups changed, per changed seat | buttons, yaw, pitch, throttle, triggers, selectors, interaction, assist |
| 7 per edge | buttons | Elias-gamma count, then a 5-bit index per toggled bit |
| 10 + n | yaw, pitch, each throttle axis | 5-bit leading zeros and 5-bit length of the XOR with the previous value, then n significant bits (about 22); on a grid, a Rice-coded zigzag delta of 6 to 9 bits |
| 9 | a trigger | flag and 8-bit pressure on the grid, else 33 |
| 2 to 6 | each selector | exact width |
| 97 | interaction | flag and the 12 bytes |
| 9 + 4 × 32 | shot assist | changed-dword mask, then each changed dword as a float |

### Layout

The file's header, game start, block kinds and trailer are in
[film-format.md](film-format.md#the-film-file). `film::file` writes and
reads them; `FilmActions` reads a film's actions by tick, in any order.
A tick's digest in a chunk is the low 16 bits of its gameplay digest and
the chunk's closing word is the whole gameplay digest of its last tick. The
recorder feeds the file one update at a time and writes a checkpoint of
the open chunk every 150 ticks (`FilmWriter::checkpoint`), which a reader
of an unsealed file takes as its last chunk. A host seals its film at the
match's end, and `film::upload` sends it to Live with the stats stream.

Chunk i covers ticks 256 i + 1 to 256 i + 256, so a tick's chunk is found
without a search. A tick chunk, inflated (`film::columns`; the whole chunk is one zstd frame
with a checksum, and a frame without one is refused):

```text
u16 ticks in the chunk (1 to 256), u16 mask of seats that are in the
                game on at least one of them
tick flags      one byte a tick
digests         two planes of one byte a tick: the low 16 bits of the
                gameplay digest
per seat in the mask, ascending:
   u16 field mask: bit 0 the seat's presence (absent: in the game on
      every tick), bit i + 1 action field i of the 13 above
   presence plane if bit 0: one byte a tick
   per field in the mask: u8 predictor (none, previous, linear, grid),
      the grid's denominator (u32) if a grid, then four residual planes
      for each 32-bit word of the field, lowest byte first, one byte a
      tick in each plane; a grid is followed by its exceptions (u8
      count, then a u8 tick and the u32 bits of each value that is off
      the grid)
seat records    u8 count; each: u8 tick offset, u8 seat, u8 kind,
                u16 length, bytes (8,192 in all, 255 at most)
tails           per tick: ULEB run count; each run a ULEB skip, a ULEB
                length and that many bytes
u64             the gameplay digest of the chunk's last tick
```

The 13 action fields are the rows 1 to 13 of the table above; the first
is the one row of 14 that the update's mask makes, which the seat mask
and presence bit say.

- A residual is the value's 32-bit pattern minus its prediction,
  zigzag-coded (none: the pattern itself). Linear predicts twice the last
  tick's value minus the one before. A grid is for a one-word field of
  `f32` that sits on 1/den for one of 63, 64, 127, 128, 255, 256, 1023,
  1024, 4095, 4096, 32767 and 65535: the residual is the step from the
  last value's nearest step, and a value that is not exactly its step's
  float (a stray, a NaN, negative zero) is listed whole. The encoder
  tries every predictor for each field of each seat in each chunk and
  keeps the one with the fewest non-zero and changing bytes; a grid with
  more than a quarter of its ticks as exceptions is not tried.
- A field absent from the mask holds its value from the chunk before for
  every tick of the chunk. A seat absent from the seat mask is in the
  game on no tick of the chunk and holds its rows. A seat that is not in
  the game on a tick keeps its last value, so its columns see no change
  there.
- Predictor state runs across chunks: the last two values of each word of
  each seat, 188 bytes a seat. The decoder starts at tick 1 or from the
  seat rows a ring entry stores (`ChunkDecoder::seat_rows`,
  `set_seat_rows`), which is every place playback can start. A film has no
  seek point of its own.
- Columns run seat by seat and plane by plane, so the encoder's and the
  decoder's inner loops are straight runs over one byte stream each, and
  the coder sees a seat's idle fields as one run of zeros.
- A tail is the update's bytes from the end of the action block to the
  stamp (`0x5cc` through `0x403f`, 14,964 bytes, `BODY_BYTES`): the player
  entries, the player-set change, the event list and the save flag. A
  tick's runs are the bytes that differ from the tick before; the
  chunk's first tick differs from zeros, so a chunk's tails decode without
  the chunk before. A tick whose tail did not change, or that was given
  none, is one zero byte. Two changed bytes with at most three unchanged
  between them are one run. A chunk's tails are at most 512 KiB
  (`TAIL_LIMIT`), and the encoder refuses a tail that would pass it. The
  decoder checks every run lies inside the body and reads a tick's body by
  laying the runs of the ticks up to it over zeros (`ChunkDecoder::body`),
  one tick's runs for each tick read in order.
- A chunk boundary is a byte boundary and a zstd frame boundary. Live
  and a downloader cut a film only between blocks.

### Size, step by step

**Estimate**, from assumed rates, for the action block alone; the
tails are not measured. For a local seat that is alive and playing: yaw changes on 70% of
ticks, pitch on 50%, each throttle axis on 50%, a trigger on 5%, a button
on 3%, the shot-assist block on 25% with four of its nine values moving,
the interaction on 1%. A changed `f32` costs about 22 bits after the
coder under the previous-tick predictor. A seat is active for 75% of a
match.

| Step | Bits per active local seat-tick | Bytes per player-minute, active | Remote seat on a host |
|---|---:|---:|---:|
| Full records | 736 | 165,600 | the same |
| A change mask per field, before zstd | 180 | 40,500 | |
| A change mask per field, after zstd | 120 | 27,000 | |
| Columns, previous-tick residuals, planes, zstd | 72 | 16,200 | |
| Predictor chosen per column | 58 | 13,000 | |
| Grids where found | 48 | 10,800 | 3,800 |
| Dictionary | 46 | 10,300 | 3,600 |
| **At 75% activity** | | **7,700** | **2,700** |

Match-wide, per minute: digests 3,600 bytes and chunk hashes 56, against
19,800 for 8-byte digests with their framing; block headers and frame
overhead about 280. Fixed: the 64-byte header, a game start of a few kilobytes, and
8 bytes of trailer per chunk.

| A 10-minute game | Film | Halo 3 ([notes/halo3-film.md](notes/halo3-film.md)) |
|---|---:|---:|
| One player, offline | 77 + 37 + 2 = 116 KB | 930 KB to 1.8 MB (campaign) |
| Two players on a host | 54 + 37 + 2 = 93 KB | 1.3 MB |
| 16 players on a host | 432 + 37 + 3 = 472 KB | 7.2 MB |

### Memory

The whole film is a file on disk: recorded there, or fetched from Live
into the library by range requests and kept. Playback reads and inflates
one chunk at a time and never holds more of the film than that. Nothing
below grows with the length of the game. For 16 seats:

| Recording (`ChunkEncoder`) | Bytes | |
|---|---:|---|
| The chunk's words: 256 ticks × 16 seats × 23 words | 376,832 | rows as `u32`, seat by seat |
| Presence, flags and digests | 5,376 | |
| Seat records | 9,722 | 8,192 of payload and the 255 headers |
| The tails | 512,000 | at most 512 KiB of runs, and a zero byte for each tick |
| The running body | 14,964 | what the next tail is a change against |
| The inflated chunk, with every column present and the tails full | 459,271 plus the tails | the most a chunk can be |
| Candidate planes and exceptions | about 22,000 | |
| The frame | 196,608 | a chunk that does not compress to this is refused |
| The seats' last two rows | 2,944 | 16 × 2 × 23 words |
| zstd compression context | **Estimate** | `ZSTD_estimateCCtxSize` at level 3 for a chunk-sized source gives the figure |
| **Total, without the zstd context** | **about 1.0 MiB** | |

The encoder takes a tick's rows with `push_tick` and compresses in
`finish`, on the caller's thread. The recorder hands each
closed chunk's frame to a writer thread, and keeps a second encoder so
the game thread never compresses or waits; if the second fills before the
first is written, 8.5 s later, the recording stops with that reason. The
encoder allocates nothing after `new`; its test counts allocations with a
global counting allocator over the second and third chunks.

| Playback (`ChunkDecoder`) | Bytes | |
|---|---:|---|
| File read buffer | 65,536 | the reader's |
| The inflated chunk | 459,271 | the most a chunk can be; typically a tenth of that is used |
| The chunk's words, undone | 376,832 | one pass per column undoes its predictor into the rows |
| Presence, flags, digests and the residual scratch | 6,400 | |
| Seat records | 3,060 | |
| The seats' last two rows | 2,944 | 188 bytes a seat |
| zstd decompression context | about 98,000 | **Estimate** |
| Dictionary and its tables, if adopted | about 93,000 | |
| Trailer: 8 bytes a chunk | 568 | for 10 minutes; 3,400 for an hour |
| **Total** | **about 1.0 MiB** | |

After inflating a chunk the decoder undoes each column's predictor in one
forward pass over that column, then serves a tick by writing a seat's
words as the 92 bytes into a buffer the caller owns (`ChunkDecoder::action`).
`apply` in `actions.rs` writes the rows into the update. A chunk that is
refused (a zstd checksum, a tick count, a field, a grid, a seat record or
a length that does not hold, or bytes after the end) is refused with its
tick range and leaves the seat rows as they were; its tests cut the chunk
at every length and corrupt it at random.

### The action side stream

In a Live game the host forwards the film to every client as it is made,
so each client holds the host's film.

- **What is sent.** The film's bytes, unchanged, as numbered pieces: the
  header and game start when a client joins, then what the file gained as each
  tick chunk closed (the chunk and any extension block before it), every
  256 ticks, and at the seal the trailer and the sealed header. Nothing
  is encoded twice, and a client's file equals the host's. `film::side`
  holds the frame format and both ends: `Forwarder` on the host and
  `Assembler` on the client.
- **Where it rides.** A frame kind of the Live datagram tunnel
  (`live_proto::tunnel`, `crates/live/live-hle/src/tunnel_host.rs`,
  `tunnel_client.rs`), beside the game's datagrams and outside the
  guest's sockets. The guest sends and receives what it did before, so
  the simulation contract does not move. A frame carries the film id, the
  piece index, the piece's offset and length in the file, a part number
  and the bytes, in at most one tunnel payload. The tunnel frame kind and
  the 16 KiB a second pacing are not built.
- **Bandwidth** (**Estimate**, from the sizes above). 16 seats: 43,200
  bytes a minute of actions, 3,650 of digests and 280 of framing, 47,100
  in all: 785 bytes a second, 6.3 kbit/s to each client, and 94 kbit/s
  from a host with 15 clients. A chunk is about 6.7 KB every 8.5 s. Two
  seats: 1.2 kbit/s.
- **Bound.** A client is sent at most 16 KiB a second of film. A chunk
  over 64 KiB stored is not forwarded. The host keeps the last eight
  stored chunks, at most 512 KiB, for resends.
- **Loss.** A client that sees a gap in the piece index asks the host for
  that piece again. For a piece the host no longer holds, or would not
  send, the host names its byte range, and the client fetches it after
  the game from the host's film on Live. A film with a gap is unsealed until the gap is
  filled, and is not offered for Watch.
- **Check.** Each chunk ends with the whole gameplay digest, and the sealed film's
  digest is in the game's Live record; the client compares both.
- **The view.** Theater plays the whole film with the player's seat
  selected. It shows the host's authoritative game from that seat, not
  the client's own prediction.

### Start points

A film whose first update is not its game's first begins at a start
point: a player who joins a running synchronous game is given one by the
host, a campaign mission resumed from a saved checkpoint gets one at its
first update, and so does a recording started during a game.

- **What it is.** The game-state arena (`0x3fe000` bytes) and the
  extension blob at the end of the join tick, compressed, with the tick,
  the arena hash and the per-game identifiers. Never guest RAM, device
  state or the disk. The join's own seat record is on the next tick, so
  the state is one every machine already has
  ([live-coop.md](../live/live-coop.md) section 14).
- **How small.** 4,186,112 bytes of arena; the retail join compresses it
  into a 256 KiB buffer, about 16 to 1 (**E**, a guess recorded in
  live-coop.md section 14.1). So about 0.26 MB, against about 1.2 MB for
  the game state in a Halo 3 clip. The stored limit is 1 MiB for the
  arena and 64 KiB for the extension blob.
- **How it is made safe.** The host tags it with a MAC over the tick, the
  lengths, the arena hash and the SHA-256 of the bytes, under the key the
  joiner's tunnel session already holds. The joiner inflates into a
  buffer of exactly the arena's size, refuses any other length, and after
  loading compares `lockstep::hash_sim` with the tagged hash. The host
  writes the same arena hash into the join's seat record, so the film on
  Live confirms the start point after the game. Nothing in it is an
  address the loader follows: the arena goes in through the engine's own
  restore path.
- **In a film.** A film that begins at a start point has the header's
  start point flag and a block of kind 2 before its first tick chunk. A
  start point a machine writes into its own film is tagged under the
  film's id. A joiner's film stays on the joiner's machine; the joiner
  can instead fetch the earlier chunks and hold the whole game from its
  game start.
- **The envelope.** `crates/extensions/game/src/theater/start_point.rs`
  writes and reads the tagged form: lengths, arena hash and identifiers,
  the SHA-256 of both stored frames, and an HMAC-SHA-256 over all of it.
  The reader checks the tag, the digest, its own arena size and the film's
  arena hash before it inflates, then inflates into exactly the stated
  sizes. The caller compares `lockstep::hash_sim` after loading with
  `StartPoint::check_loaded`.
- **Capture and load.** `theater.start_point.capture` and
  `theater.start_point.load` ([agent-api.md](../runtime/agent-api.md#theater-transport))
  take a start point from a running game and put one back, through
  `theater/start_point_state.rs`: the arena is the bytes behind
  `SimRegion::HALO2`'s pointer word and the records go through the
  `SimExt` checkpoint section. The load writes the arena whole and then
  hashes it as `lockstep::hash_sim` does; `matches: false` means what
  reads back is not what the tag names. Whether the arena is the whole
  state is answered by the `sim.hashes` rows after the load. `theater/start_play.rs` reads the
  film's start point block (the header's flag, exactly one block of kind
  2), refuses a block the flag does not name, applies it with the same
  load and returns the tick the film resumes at (the start tick plus one).
- **In playback.** The player loads the film's map from its game
  options, calls `start_play::apply` at the first update, then reads tick
  chunks from the resume tick. The same capture and load are the session
  ring's entries ([theater-design.md](theater-design.md#the-session-ring)).

### On Live

Live stores a film as the bytes it was sent and serves it by byte range
(`crates/live/live-server/src/films/upload/film_file.rs`). It reads the
header and the game start, checks that every block's stored size stays inside
the file and that the trailer lists exactly the tick chunks it found, and
inflates no tick chunk. A film needs no scrub: it holds the names and
appearance the game showed and no disk file, and Live issues the seat
identifiers afresh for each game. A clip is a pair of
ticks; playing it needs the film from its first byte to the end of the
clip's last chunk, which is one range.

## Rules and limits

- Lossless: decoding gives back every byte of every action the recorder
  read. A model that does not hold for a value stores the value whole.
- A film has no seek point, keyframe, pose, event, score, disk file,
  controller read or host call.
- The recorder and the decoder allocate nothing after they start.
- A chunk that fails its zstd checksum, names a seat above the limit,
  or holds a column of the wrong length is refused with its tick range;
  playback stops there.
- The dictionary changes only with the simulation ABI, so a playable
  film never names a dictionary the app lacks.
- 16 bits of digest a tick means a divergence is reported on its own
  tick with probability 1 − 2⁻¹⁶ and within two ticks otherwise, as long
  as the states stay different.

## Evidence

- **Code**: the action block is the mask and 16 records of `0x5c` bytes;
  `capture` reads it and `apply` writes it at `SIM_UPDATE_APPLY`
  (`actions.rs`, `ACTION_BYTES`, `ACTION_BLOCK`).
- **E**: the update carries a player-set change applied by its own apply
  ([live-coop.md](../live/live-coop.md) section 14.1); a remote player's
  action is quantised when stored, and a seat can be made in memory with
  nothing on the wire ([bots.md](bots.md#5-way-in-2-phantom-peers)).

## Not verified

- Every rate in "Size, step by step", and so every size on this page.
- The size of the update's tail, and which of its bytes a local game
  fills.
- Whether the linear predictor beats the previous-tick one on real aim.
- The grids: what the action store's quantisation is, and whether a
  local seat's throttle and triggers sit on one.
- Which bytes of the action the consumer reads. The encoding does not
  depend on the answer.
- Whether a remote seat's shot-assist block is empty on a host.
- What a game run must show before a film plays
  ([theater-design.md](theater-design.md#not-verified)).
- The zstd context sizes.
- That the tunnel can carry a frame kind the guest never sees, on every
  kind of host.
- The compressed size of an arena, and the key a start point's MAC uses.
- That a seat that was remote in the recording gets a HUD when selected
  in playback.
- That per-game identifiers issued by Live are the ones the update
  carries.

## Related

- [theater-design.md](theater-design.md): the player, the stats and film split, the slices
- [film-format.md](film-format.md): the film file, the stats container, recording every game
- [theater.md](theater.md): the selector, the controls, the web tier
- [notes/halo3-film.md](notes/halo3-film.md): the Halo 3 reference
- [bots.md](bots.md): the action record and phantom seats
- [live-coop.md](../live/live-coop.md): the simulation update and its player-set change
