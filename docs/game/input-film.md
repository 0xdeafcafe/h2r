# The input film

Status: Design. The per-tick input capture, the HLE tape and the container that holds them are built; the film file, its encoding and the engine film are not.
Code: `crates/extensions/game/src/theater/actions.rs` (`capture`, `apply`), `crates/live/film/src/tape.rs` (`Call`, `Batch`, `Playback`), `crates/live/film/src/blam` (the tick track). Tests: `crates/extensions/game/src/theater/actions.rs` (`gathered_actions_follow_sparse_player_slots_and_replay_ignores_pad_only_rows`), `crates/live/film/src/blam/tests.rs`.
Switch: none.

A film is how to make a game happen again: a seed, every seat's input on
every tick, seat joins and leaves, and a digest per tick. The engine
reads it and nothing else does; what happened is the stats stream's
business ([theater-design.md](theater-design.md#stats-and-film)). This
page says what a film must hold for replay to be exact, how it is
encoded, and what it costs in bytes and memory. The rule that must hold:
the encoding is lossless for every byte the simulation is given.

Evidence labels: **Code** (read in this repository, path given), **Test**
(a named test), **E** (read from the retail XBE, as recorded in the linked
page), **S** (supposed, with what would verify it), **Estimate**
(arithmetic from cited numbers), **Design** (our choice).

## Halo 3 reference

From [notes/halo3-film.md](notes/halo3-film.md), which has the sources.

| | Halo 3 | This design |
|---|---|---|
| Start state | game options, both variants by value, seed, machines and 16 player configurations, in a fixed header of about 64 KB; a game state only in a clip | the same content as a compressed seed of about 1 to 2 KB; never a game state |
| Tick record | one bit-packed record per tick: player mask, each present player's action after control mapping and aim assist, AI control, machine update, prediction, camera, two queues | columns per seat per chunk of 256 ticks, holding the same post-mapping action; no AI control, prediction or camera |
| Joins and leaves | a machine update inside the tick record | a seat record on its tick |
| Desync check | update number, game time and random seed in every record: 12 bytes a tick | 16 bits of the gameplay digest a tick, and the 64-bit arena hash every 256 ticks |
| Compression | none; fields are bit-packed | residual columns under zstd |
| Seek points | none in the file; the player snapshots as it plays | none in the file; the player's sidecar and ring |
| Version gate | executable type, determinism version, map signature | XBE digest, simulation contract, map digest |
| Size | 45,000 to 63,000 bytes per player-minute; 133,000 to 720,000 bytes a minute for multiplayer films | about 2,700 (a host's remote seat) to 7,700 (a local seat) bytes per player-minute, and 3,650 a minute for digests (**Estimate**) |

The comparison is not like for like: a Halo 3 record also carries AI
control, prediction, a camera and the simulation queues, and how much of
its multiplayer size is input is not established.

## What a film holds

| Part | Holds | Needed by |
|---|---|---|
| **Seed** | map identity and digest, variant bytes, game options, the random seed, the first tick's seats, the simulation contract | every player |
| **Tick stream** | each seat's 92-byte action on each tick, seat records, tick flags, a digest per tick | every player |
| **Machine part** | boot facts (`HEAD`), startup decisions (`BOOT`, `MOVI`), the initial guest disk (`DATA`), the HLE tape, capture-point marks, arena hashes with the call cursor (`HASH`) | the player until the engine film is proven for that kind of game; absent after |

A film holds no keyframe, no pose, no event and no score.

### The seed

| | The machine film (ships first) | The engine film |
|---|---|---|
| Seed | the boot recipe: the image identity and boot facts the player must match (`film boot facts differ` in `Session::open`), `BOOT`, `DATA`, and the logical clock's epoch | a reference: map, variant bytes, game options, random seed, the first tick's seats |
| Start of the film | boot. Menus and the lobby are replayed from the tape; a direct level start (`StartLevel` in `crates/extensions/game/src/theater/startup.rs`) keeps that short | the first tick of the match |
| Guest RAM in the film | none | none |
| What makes the start state | the player's own disc and image, run from boot | the engine's own game start, given the seed |
| Proof it is the same start | the first arena hash and every digest after | the arena hash of the first tick, stored in the film |

The seed is a reference, never a snapshot. Halo 3's is the same: game
options, both variants by value, the seed and the roster, with a game
state only in a clip that starts mid-game
([notes/halo3-film.md](notes/halo3-film.md)). A campaign game resumed from
a save has the save file in `DATA`, which is the user's own data. If the
engine's game start does not reproduce the recorded arena from the seed
(the first experiment in [theater-design.md](theater-design.md#before-and-after-the-sim-rewrite)), the fallback seed is one compressed arena
keyframe at match start, `sim_state::Keyframe`. That waits for the sim
rewrite and is game state, which Live does not store.

### What the replay depends on

Everything the current replay reads besides the per-tick inputs, from the
code:

| Dependency | What is recorded | Where | Why | Machine film | Engine film |
|---|---|---|---|---|---|
| Pad reads (`PADR`) | every `XInputGetState` status and 22-byte packet, by call index | `FilmPads::read_with_memory` in `theater/local_pads.rs` | the only way input and menu presses reach the guest | needed; binary encoding (slice 14) | cut: the inputs replace it, menus are not replayed |
| Controller ports (`PDEV`) | port topology at each input pump | `record_ports`, same file | the guest sees device changes without reading a pad; a controller arriving is how a split-screen seat joins | needed; sparse | cut: seat records |
| Keyboard (`TEXT`) | virtual-keyboard edits | `crates/extensions/game/src/keyboard_glue.rs` | profile-name entry | needed; rare | cut |
| Host randomness (`RAND`) | `XNetRandom`, `XNetCreateKey` and `Entropy.*` outputs | `Network::filmed`, `state_random` in `crates/live/live-hle/src/network/film.rs` | host entropy written into guest memory | needed; a few calls a session | cut: the game's own random state is guest memory, seeded from the descriptor |
| Sockets and XNet (`NETR`) | every operation of `Op` (`crates/live/live-hle/src/network.rs`, `Network::invoke`): results, received datagrams with sender, `select` sets, QoS tables, and the pure `Htonl`, `Htons`, `FdIsSet` | `Network::filmed`, `slots` | replay never enters the socket code | offline game: derived. A null network answers them (no peers, nothing to receive), and the pure operations are computed. Live client or host: needed, it is how other seats arrive | cut: remote seats' inputs are in the tick stream |
| Live service (`NETR`) | each XOnline call's return, guest writes, arena claims and the account view | `invoke` in `crates/live/live-hle/src/film_service.rs` | sign-in, title storage, matchmaking and advertisement answers | signed out: a handful at boot, kept. Signed in: needed | cut |
| Host observations (`NETR`) | `HostedSession`, `HostsSession`, game settings, co-op lobby pings and joins | `lobby_hosted_session`, `hosts_session`, `game_setting` in `crates/live/live-hle/src/lib.rs`; `LobbyFilm::read` in `crates/extensions/game/src/coop_live/film.rs` | polls whose answers steer guest-facing hooks | hosts and co-op only | cut |
| Phantom seats (`BTAP`, `BOTR`) | each phantom's action-builder result; roster changes at management boundaries | `crates/extensions/game/src/bot_glue/retail_input.rs`; `bot_boundary` in `theater/bot_tape.rs` | a bot's mind is not replayed, its outputs are | needed where there are phantoms | cut: inputs and seat records |
| Lobby operations (`MARK`) | agent lobby calls by executor boundary | `impl agent::LobbyTape for Session` in `theater.rs` | a dedicated lobby is driven through the agent API | hosts only | cut |
| D3D barriers (`MARK`) | slice, tick and outcome of each device capture | `try_keyframe` in `theater/record.rs`; `replay_device_barrier` in `theater/replay.rs` | a capture drains the push buffer and writes render targets back | a film without captures needs none; kept as capture-point marks so a player can make keyframes | cut: an arena keyframe touches no device |
| Initial guest disk (`DATA`) | directories, embedded files up to 16 MiB (`LIMIT` in `crates/live/film/src/manifest.rs`), installed content by digest | `Manifest::capture_for_recording` | the guest reads the profile, settings and saves at boot | needed | cut, except a campaign save |
| Startup (`BOOT`, `MOVI`) | host decisions made before guest entry | `Startup` in `theater/startup.rs` | they select the guest's initial conditions | needed | reduced to the feature switches |
| Boot | nothing | | the only route to the match's start state | needed: the film starts at boot | cut |
| The clock | nothing but the epoch in `HEAD` | [kernel README](../../crates/runtime/kernel/README.md), "Clock and replay" | time is logical: the epoch plus charged work | derived | derived: the tick count |
| File reads | nothing | `metadata::verify_start` checks the mounted map's digest | the disc and the maps are the player's own | derived | derived |
| Guest disk changes | only inside `KEYF` | `DiskTracker` | a restore needs the disk of its tick | not in the film; in the sidecar | none |

So the machine film of an offline game needs, beyond the seed and the inputs:
the pad stream, port changes, keyboard edits and a few entropy and
service calls. The network tape is the large stream and exists only for
Live and system-link games.

**The tape's cost.** Calls are JSON (`Writer::chunk` in
`crates/live/film/src/lib.rs`): a `Call` with its operation name and each
output byte as a decimal number. A batch is closed at 128 calls or 64 KiB,
at every arena hash, and whenever the stream tag changes (`Batch::append`),
so interleaved pad and socket calls make a zstd frame, an envelope and an
`INDX` row each time they alternate. **Estimate**: a pad read is about
165 bytes inflated, an empty socket poll about 100, a received datagram
of N bytes about 90 + 3.6 N. One pad read a frame at 30 Hz is 297 KB a
pad-minute inflated; stored size depends on the alternation and is not
measured. Slice 0 measures calls and bytes per stream; slice 14 replaces
the encoding.

### Joins and leaves

**In the machine film** a join is not a record that playback applies. It
is what the guest does when given the same inputs at the same cursors: a
controller arriving (`PDEV`, by input-pump ordinal) and its presses
(`PADR`, by call index) for a split-screen seat; a roster change (`BOTR`,
by management boundary) for a phantom; packets (`NETR`, by call index) for
a remote peer. The join and leave events the recorder observes (`PlayerJoined`,
`PlayerLeft`) go to the stats stream. Playback never applies them,
because the guest has already made the change. The film's seat records
let playback check that the same seats exist, in place of
`metadata::verify_events` on the events.

**In the engine film** a join is a seat record on the tick it took
effect. The per-tick update the engine builds for itself carries more
than the action block: a player-set change with a seat mask and the
players' identifiers, applied on that tick by the update's own apply
(**E**: [live-coop.md](../live/live-coop.md) section 14.1; Halo 3's tick record has the same machine update, [notes/halo3-film.md](notes/halo3-film.md)). The recorder
reads those fields at `SIM_UPDATE_APPLY` beside the actions, with the
joining seat's session configuration (name, appearance, team), and writes
them as a seat record in the tick's chunk. Playback has no peers,
so it makes the seat the way a phantom is made, as a session peer and
player in memory with nothing on the wire
([bots.md](bots.md#5-way-in-2-phantom-peers)), lets the engine build its
update, and requires the update's player-set fields to equal the recorded
ones before the apply runs. A leave removes the session player the same
way.

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
| Changed seats and a 12-byte change mask per field (tag 1 as built, `crates/live/film/src/blam/ticks.rs`) | idle seats, idle bytes | 180; about 120 after zstd (**S**) | byte loop | replaced: the mask costs more than the changes |
| **Columns**: a structure of arrays per seat inside a chunk, each field as its residual against the previous tick | the masks; puts like bytes together; an idle field is a run of zeros | 91 before the coder | one pass per column | adopted |
| **Predictor per column per chunk**: none, previous, or linear (twice the previous minus the one before), on the field's 32-bit pattern | steady turns and ramps, where the linear residual is a few units | about 20% of the above (**S**) | the same pass | adopted; the encoder tries all three and keeps the smallest |
| **Byte planes**: the residual's four bytes split into four runs | gathers the quiet high bytes for the coder | prepares the next row | a transpose | adopted |
| **zstd per chunk** over the planes | zero runs, skewed high bytes, repeats | 58 to 72 | about 1 GB/s; no code of ours | adopted |
| **Grid models, checked per value**: where a field's values sit on a grid (a remote seat's quantised aim and throttle, a trigger made from an 8-bit pressure), store the grid index; a value off the grid is stored whole | most of a remote seat's aim | 46 to 48 local; about 16 for a remote seat | a multiply and a compare | adopted for fields where slice 13 finds a grid |
| **Dictionary** trained on recorded chunks and shipped in the app | the cold start of each small zstd frame | about 5% on chunks under 4 KB, nothing on large ones (**S**) | none | adopted only if slice 13 measures 5% or more |
| Button edges in place of button state | nothing more: the residual of the buttons field is the set of edges | | | covered by the previous-tick predictor |
| Run lengths for idle seats and idle ticks | nothing more: an absent column costs two mask bits, a zero run a few bytes a chunk | | | covered by the field mask and zstd |
| Bit-packed tick records: fields at exact width, Rice or Elias-gamma residuals, floats as leading-zero count, length and significant bits (the table below) | whole bytes on small fields and on rare events | 88 to 114 local, 24 remote; 1 when idle | a bit reader with a branch per field, 50 to 100 MB/s | not adopted: the `f32` residuals are most of the stream and cost more here (about 32 bits against 22), and a byte-wise coder gains under 3% on a bit-packed stream |
| An adaptive range coder with a model per field | 10 to 15% over zstd on the structured fields (**S**) | about 50 | 20 to 50 MB/s; our code | not adopted: the low bytes of an `f32` residual are noise that no coder shrinks, and they are most of what is left |
| Truncated digests | 8 bytes a tick to 2 | 14,400 to 3,600 bytes a match-minute | none | adopted |
| The seed as a reference | every state byte | | | adopted |
| Recomputing the shot-assist block in playback | about 22 bits an active tick | | playback runs the target search | not adopted: it changes what playback computes; revisit once the engine film plays |
| Recording pads in place of actions | the aim floats | | | not adopted: a pad becomes an action through frame timing and control history, which only the machine reproduces; Halo 3 stores actions too |

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

The film is one file, little-endian, read front to back.

```text
header, 64 bytes
   0   8  ASCII "H2FILM4!"
   8   u16 version (4), u16 flags (bit 0 machine part, bit 1 sealed)
  12   u8 ticks per second, u8 seat limit (16), u16 ticks per chunk (256)
  16  16  film id
  32   u32 length in ticks, u32 stored bytes of the seed
  40   u64 offset of the trailer
  48  16  digest of the dictionary the chunks name, or zero
seed            one zstd frame
blocks          each: u8 kind (0 tick chunk, 1 machine envelope),
                u32 stored bytes, then one zstd frame
trailer         u32 chunk count, then a u64 offset per tick chunk
```

The length, the trailer offset and the sealed flag are zero while a film
is being written or streamed; a reader of such a file walks the blocks.
Chunk i covers ticks 256 i + 1 to 256 i + 256, so a tick's chunk is found
without a search. A tick chunk, inflated:

```text
u16 ticks in the chunk, u16 mask of seats with any column
tick flags      one byte a tick: update applied, paused, seat record
digests         two planes of one byte a tick: the low 16 bits of the
                gameplay digest
per seat in the mask, ascending:
   u16 field mask (the 14 fields above)
   per field in the mask: u8 predictor (none, previous, linear, grid),
      the grid's parameters if any, then the residuals plane by plane,
      zigzag-coded, one byte a tick in each plane
seat records    u8 count; each: u8 tick offset, u8 seat, u8 kind,
                u16 length, bytes
u64             the arena hash after the chunk's last tick
```

- A field absent from the mask holds its value from the chunk before for
  every tick of the chunk. A seat absent from the seat mask is idle or
  empty for the whole chunk.
- Predictor state runs across chunks. The decoder starts at tick 1 or
  from the seat rows a sidecar keyframe stores, which is every place
  playback can start. A film has no seek point of its own.
- Columns run seat by seat and plane by plane, so the encoder's and the
  decoder's inner loops are straight runs over one byte stream each, and
  the coder sees a seat's idle fields as one run of zeros.
- A chunk boundary is a byte boundary and a zstd frame boundary. Live
  and a downloader cut a film only between blocks.

### Size, step by step

**Estimate**, from assumed rates that slice 13 measures on recorded
films. For a local seat that is alive and playing: yaw changes on 70% of
ticks, pitch on 50%, each throttle axis on 50%, a trigger on 5%, a button
on 3%, the shot-assist block on 25% with four of its nine values moving,
the interaction on 1%. A changed `f32` costs about 22 bits after the
coder under the previous-tick predictor. A seat is active for 75% of a
match.

| Step | Bits per active local seat-tick | Bytes per player-minute, active | Remote seat on a host |
|---|---:|---:|---:|
| Full records | 736 | 165,600 | the same |
| Tag 1 as built, before zstd | 180 | 40,500 | |
| Tag 1 as built, after zstd | 120 | 27,000 | |
| Columns, previous-tick residuals, planes, zstd | 72 | 16,200 | |
| Predictor chosen per column | 58 | 13,000 | |
| Grids where found | 48 | 10,800 | 3,800 |
| Dictionary | 46 | 10,300 | 3,600 |
| **At 75% activity** | | **7,700** | **2,700** |

Match-wide, per minute: digests 3,600 bytes and chunk hashes 56, against
19,800 for 8-byte digests with their framing; block headers and frame
overhead about 280. Fixed: the 64-byte header, a seed of 1 to 2 KB, and
8 bytes of trailer per chunk.

| A 10-minute game | Film | Halo 3 ([notes/halo3-film.md](notes/halo3-film.md)) |
|---|---:|---:|
| One player, offline | 77 + 37 + 2 = 116 KB | 930 KB to 1.8 MB (campaign) |
| Two players on a host | 54 + 37 + 2 = 93 KB | 1.3 MB |
| 16 players on a host | 432 + 37 + 3 = 472 KB | 7.2 MB |

While a film has a machine part it also carries that: for an offline
game 3,000 to 10,000 bytes a pad-minute of pad reads in the binary tape
([theater.md](theater.md#size-per-minute)) and the profile; for a Live
client or host 0.3 to 1.5 MB a minute of packets.

### Memory

The whole film is a file on disk: recorded there, or fetched from Live
into the library by range requests and kept. Playback reads and inflates
one chunk at a time and never holds more of the film than that. Nothing
below grows with the length of the game. For 16 seats:

| Recording | Bytes | |
|---|---:|---|
| Two chunk buffers, columns in place: 256 ticks × 16 seats × 93 bytes | 761,856 | one fills on the game thread while the writer thread compresses the other |
| Tick flags and digests, twice | 1,536 | 3 bytes a tick |
| Seat records, twice | 8,192 | room for 16 joins with their configurations |
| The seats' last rows | 1,472 | 16 × 92 |
| zstd compression context | about 840,000 | **Estimate** for a 512 KiB window at the recording level; `ZSTD_estimateCCtxSize` gives the figure |
| Output buffer | 65,536 | written to the file as it fills |
| **Total** | **about 1.6 MiB** | |

The game thread writes one byte into each column a tick and never
compresses or waits. If the second buffer fills before the first is
written, 8.5 s later, the recording stops with that reason; it does not
allocate.

| Playback | Bytes | |
|---|---:|---|
| File read buffer | 65,536 | |
| One inflated chunk, every column present | 386,568 | typically a tenth of that |
| zstd decompression context | about 98,000 | **Estimate**; it inflates straight into the chunk buffer |
| Dictionary and its tables, if adopted | about 93,000 | |
| Seat state: `[SeatState; 16]`, each the 23 current words, the 23 last deltas and the occupied flag | 3,008 | 188 bytes a seat |
| Trailer: 8 bytes a chunk | 568 | for 10 minutes; 3,400 for an hour |
| **Total** | **about 0.62 MiB** | |

After inflating a chunk the decoder undoes each column's predictor in one
forward pass over that column, then serves ticks by reading one byte from
each present column into the seat's row. `apply` in `actions.rs` writes
the rows into the update.

### On Live

Live stores a film as the bytes it was sent and serves it by byte range.
It reads the header and the seed, checks that every block's stored size
stays inside the file, and inflates no tick chunk. Identity is in the
seed's roster only, so the scrub rewrites one small frame and copies the
rest. A clip is a pair of ticks; playing it needs the film from its
first byte to the end of the clip's last chunk, which is one range.

## Rules and limits

- Lossless: decoding gives back every byte of every action the recorder
  read. A model that does not hold for a value stores the value whole.
- A film has no seek point, keyframe, pose, event or score.
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
- **Code**: tag 1 as built is a changed-seat mask, a flags byte and a
  byte-change mask of 12 bytes for an action, then the replacement bytes
  ([film-format.md](film-format.md#tagged-records)).
- **Code**: every call on the tape is JSON, batched by stream
  (`Writer::chunk` in `crates/live/film/src/lib.rs`, `Batch::append` in
  `tape.rs`).
- **E**: the update carries a player-set change applied by its own apply
  ([live-coop.md](../live/live-coop.md) section 14.1); a remote player's
  action is quantised when stored, and a seat can be made in memory with
  nothing on the wire ([bots.md](bots.md#5-way-in-2-phantom-peers)).

## Not verified

- Every rate in "Size, step by step", and so every size on this page.
  Slice 13 measures them on films that exist.
- Whether the linear predictor beats the previous-tick one on real aim.
- The grids: what the action store's quantisation is, and whether a
  local seat's throttle and triggers sit on one.
- Which bytes of the action the consumer reads. The encoding does not
  depend on the answer.
- Whether a remote seat's shot-assist block is empty on a host.
- The three engine-film experiments
  ([theater-design.md](theater-design.md#before-and-after-the-sim-rewrite)).
- The zstd context sizes.

## Related

- [theater-design.md](theater-design.md): the player, the stats and film split, the slices
- [film-format.md](film-format.md): the container as built, which becomes the stats stream
- [theater.md](theater.md): the machine and engine levels of determinism
- [notes/halo3-film.md](notes/halo3-film.md): the Halo 3 reference
- [bots.md](bots.md): the action record and phantom seats
- [live-coop.md](../live/live-coop.md): the simulation update and its player-set change
