# Halo 2 engine films

Status: Design. The film file's tick chunks, the game start block, the action and body capture at the simulation update and the stats container are built; the recorder writes a checkpoint of the film file every 150 ticks. A default recording writes one film for each game, `<id>.h2film` with `<id>.h2stats` beside it, opened at the game's activation and sealed at its end (slice 6). Extension blocks are built, with the Forge session's edits their first user; the file still carries a seed, and a `--film` or host recording is one film from boot to its first game's end ([theater-design.md](theater-design.md#slices)). It carries no call, machine keyframe, disk manifest or boot record, and the stats container carries no call tape, arena hash or mark.
Code: `crates/live/film/src/file.rs` (`FilmWriter`, `FilmReader`), `crates/live/film/src/start.rs` (`GameStart`), `crates/live/film/src/blam` (the stats container), `crates/extensions/game/src/theater/record.rs` (the recorder), `crates/extensions/game/src/theater/game_start.rs` (the capture). Tests: `crates/live/film/src/file/tests.rs`, `crates/live/film/src/start/tests.rs`, `crates/live/film/src/tests.rs`, `crates/extensions/game/src/theater/game_start/tests.rs`.
Switch: `film-autorecord` (on), listed in [switches.md](../runtime/switches.md).

An `.h2film` is a film of one game: the game options the game started
from and one simulation update for every tick after. Theater loads the
map with those options in the running game and lets the engine advance
its own objects, physics and rules from the updates
([theater-design.md](theater-design.md)). The `.h2stats` file beside it
is the stats stream: events, scores and poses for Live and the web
viewer. This page is both files and the rules for recording them. The
rule that must hold: a film holds nothing of the machine that recorded
it, no disk file, profile, controller read, packet or memory image.

## Inspired by Halo 3 theater

| | Halo 3 ([notes/halo3-film.md](notes/halo3-film.md)) | Here |
|---|---|---|
| Start state | the game options block: map, both variants by value, the players' configuration, the seed, the tick rate | the engine's own options block by value, with the two random words |
| Data | one simulation-update record a tick | the update the engine builds each tick, read at `SIM_UPDATE_APPLY` |
| Mid-game start | one game state, in a clip | a start point: the game-state arena |
| Seek points in the file | none | none |
| Version gate | executable type, determinism version, map signature | XBE digest, simulation contract, hook digest, map digest |
| Playback | load the map with the options, apply the updates | the same, as a local game |

A clip is the film's id and two ticks. Saving, sharing or changing its
camera copies nothing.

## Making and opening a film

`h2 play` records every game into the film library
([Recording every game](#recording-every-game)), and a leased dedicated
host records each match. To choose the file, pass `--film FILE` to
`h2 play`, `h2 run` or `h2 serve`, or set `H2_FILM_OUT`. The destination
must be a new file; its stats container is the same name with
`.h2stats`. The film opens at boot, with tick 1 the session's first tick,
and seals at the end of the session's first game.

```sh
h2 play --film match.h2film --level ascension
h2 theater play match.h2film
```

Watch in the game's Theater plays a film in the running game.
`h2 theater play` starts the game with the film as its first map. Both
need the same build and the film's map; neither reads the recorder's
data directory, and there is no `--data`.

## Recording every game

`h2 play` records each game into the film library (`H2_FILM_DIR`, or
`<data>/films`) when no `--film` or `H2_FILM_OUT` names a file. The
decision and the library rules live in
`crates/extensions/game/src/theater/autorecord.rs`.
`H2_FEATURES=-film-autorecord` turns it off.

- **One film per game.** A film opens when a game's map is activated and
  seals when the game ends ([One film per game](#one-film-per-game)). A
  session that plays no game (menus, Theater, settings) writes nothing.
- **Names.** `auto-YYYYMMDD-HHMMSS-mmm.h2film` and the same stem with
  `.h2stats`, from the UTC time the game started. Recent sorts by that
  time, newest first.
- **The library cap.** Before a game's film is opened the library keeps
  the 24 newest unsaved default films, so the new one makes 25, the
  Recent tab's length. Saved films, films named with `--film` and
  downloaded films are never removed.
- **Recording changes no feature.** A session records with the plan it
  plays with: Live session links, the Custom Game Browser, Firefight's
  director, the rules start gate and Forge stay on. A film's stamp names
  the simulation's hooks, not the session's features
  ([Determinism and verification](#determinism-and-verification)).
- **One film observer.** A recording session plans the film observer
  (`theater-film`, `theater::OBSERVER_FEATURE`) beside the in-game
  Theater's screens (`theater-films`, `theater-film-detail`). No other
  feature takes its id; `registration/tests.rs` builds that plan.
- **Outside changes.** A Live session link joined changes nothing the
  film replays: the recording carries on and `theater.status` lists the
  reason in `notes`. An editor write or an agent API write changes a
  running game's ticks from outside the update, with no record: the game
  in play's film ends at the tick before the write and is kept to there,
  the rest of that game is not recorded, `notes` says where it ended, and
  the next game records as usual (`Session::outside_change`). A change a
  feature records as an extension block keeps the film
  ([The film file](#the-film-file)).
- **A film being watched is not recorded.**
- **Sessions that skip recording.** An `h2 lab` job (its frame times stay
  comparable; `H2_FILM_OUT` records one on purpose) prints
  `theater: not recording this session: <reason>` once at boot. A
  recording session prints `theater: recording this session; each game's
  film goes to <dir>`.
- **The session says what it kept.** At the session's end it prints one
  line: `theater: N films saved to <dir>`, or `theater: no film: <reason>`.
  `theater.status` reports the same while it runs: whether a game is
  being recorded, the films sealed, the last discard reason, the notes
  and any failure
  ([agent API](../runtime/agent-api.md#theater-transport)).
- **Local films** stay on the player's computer until the player shares
  one or the account library's background upload takes it
  ([theater-design.md](theater-design.md#account-film-library)). A
  dedicated host uploads its match through its lease.
- **An exit without a teardown still seals.** The boot loop holds a seal
  (`theater::Session::seal_on_drop`) across its slice loop, so an error
  exit or a panic that skips the final finish seals the game in play as a
  clean exit does. A SIGTERM, a window close and Cmd-Q all end the run
  through the loop's teardown. A hard kill (SIGKILL, a power loss) leaves
  an unsealed film. The recorder writes a checkpoint of the open chunk
  every 150 ticks (`FilmWriter::checkpoint`), so the film opens as it
  stands and loses at most 150 ticks; the library lists it without
  scores. Before the next game's film opens, the library prune seals an
  unsealed default film untouched for an hour at its last checkpoint
  (`film::file::recover`), or deletes it with its stats container when it
  holds no game start or no tick (`autorecord::prune`).
- **A game in progress is not listed.** The recorder holds an exclusive
  lock on the film file it writes (`film::file::hold`) until the film is
  sealed and closed. The library does not list an unsealed film a
  recorder holds, and the prune leaves it alone however long it is idle
  (`film::file::being_written`). A crash or a kill ends the process and
  its hold, so the film it left lists at once.

Recent lists up to 25 unsaved games. Saving a film moves it to Saved.
Opening Theater reads the local library even when Live has no entry for
the game.

| The game | Recent |
| --- | --- |
| Offline, split screen, co-op over Live | The game's film, recorded on this machine. |
| A game this machine hosts (System Link or Live) | The game's film, recorded on this machine. |
| A System Link or Live game another player hosts | No film from this machine: a client is sent state, not updates. The host's film when the account library has it. |
| A Live game on a dedicated host | The host's film from the account library, when signed in. |

Recording costs time on every tick. The update hook copies the update's
body (`theater/actions.rs`). The observer after the tick hashes the
gameplay state and writes the stats records, and hashes the simulation
arena every 256 ticks (`Session::checkpoint`, `Recording::post_tick` in
`theater/record.rs`). Nothing is recorded outside a game, and no host
call, controller read or memory image is recorded in one. Every world
frame of a recording draws inside the render's private random stream
(`render_rng.rs`), as every frame of a playback does.

### One film per game

A film is one game, from the activation of its map to its end.

1. **Open.** When `GAME_ACTIVATE` returns success for a scenario that is
   not the main menu, with options other than the game in play's, the
   recorder seals the game before it, makes room in the library and
   creates `<id>.h2film` and `<id>.h2stats` with their headers
   (`State::open_game`). A campaign revert activates the same options and
   is the same game.
2. **The first update** takes the game start, with the random words and
   start hash, and writes it just before tick 1. A game gone before its
   first update ends, or one whose guest stops inside it, has no film:
   both files are deleted. A game whose arena came from a saved checkpoint, or one already
   running when recording starts, gets a start point block here.
3. **Each update** adds a record; every 256 make a tick chunk.
4. **Seal.** The game-over latch, a map unload or the session's exit
   writes the open chunk, the trailer, the length and the sealed flag,
   and closes the stats file with its summary.

Tick 1 is the game's first update. A film of a session's third game holds
nothing of the first two. The next game starts once the guest leaves the
ended one: a map unload, or a new game whose game-over latch is clear. A
game left in play, multiplayer or campaign, is sealed when the guest
leaves its map: its film is what was played.

## The film file

All numbers are little-endian. The file is read front to back.

```text
header, 64 bytes
   0   8  ASCII "H2FILM5!"
   8   u16 flags (bit 0 sealed, bit 1 begins at a start point), u16 zero
  12   u8 ticks per second, u8 seat limit (16), u16 updates per chunk (256)
  16  16  film id
  32   u32 length in updates, u32 stored bytes of the game start
  40   u64 offset of the trailer
  48   u32 the game's tick count at the first update, 12 bytes zero
game start      one zstd frame of sections
blocks          each: u8 kind, u32 stored bytes, then one zstd frame
                   0 tick chunk          256 updates
                   2 start point         at most one, before the first chunk
                   3 checkpoint          the open chunk, in an unsealed file
                   5 extension           one feature's input at one tick
trailer         u32 chunk count, then a u64 offset per tick chunk
```

`film::file` writes and reads it. A file with another magic does not
open. The length, the trailer offset and the sealed flag are zero while a
film is written; a reader of such a file walks the blocks and takes a
checkpoint block as its last chunk. Chunk i covers updates 256 i + 1 to
256 i + 256, so an update's chunk is found without a search.

**The game start** is a u32 section count and that many sections, each a
four-byte tag, a u32 length and that many bytes (`film::start`), inside
one zstd frame. A reader refuses a missing required section, a repeated
or unknown one, a length past the frame, bytes after the last section and
an options block that is not 0x1118 bytes. The file stores it as a block of
kind 4 (`FilmWriter::game_start`); a film holds one, written just before
tick 1, the game's first update. The writer refuses a second, and a
reader refuses a file that holds a second. Until the
seed goes the block sits among the other blocks and the header's flag bit 3
says the file has one.

| Section | Holds | Required |
|---|---|---|
| `STMP` | JSON: the XBE digest, the simulation contract, the hook digest, the build that recorded | yes |
| `GAME` | JSON: the start time, the film tick of the first update, the engine's tick count there, and the match descriptor the recorder observed (`blam::MatchHeader`: the map's name, origin and SHA-256, the game type and mode, the difficulty, the skulls, the roster with each seat's kind, the host) | yes |
| `OPTS` | the game options block, as `GAME_ACTIVATE` made it active | yes |
| `RAND` | the two shared random words at the first update | yes |
| `HASH` | the arena hash at entry to the first update | yes |
| `SETS` | JSON: the value of every simulation-scope setting, and the feature switch spec | yes |
| `VARI` | the rules script of the variant: its eight-byte prefix, then its blob | when the variant names a rules script |
| `MAPV` | the Forge map variant | when the game has one; read and written, and no recorder fills it |
| `HDET` | what the start hash is made of: the extension records' digest and a hash of each 4 KiB page of the arena | written by the recorder; when a start hash does not reproduce, playback compares it and names the differing pages and extension state |

The selector reads the header, `STMP` and `GAME` and nothing else. The
options block carries the game variant and the 16 player entries by
value; the profile is not read
([theater-design.md](theater-design.md#the-start-state)).

**A tick chunk** holds, for each of its updates, the action block as
columns, the update's tail, and 16 bits of the gameplay digest, and ends
with the whole gameplay digest of its last tick
([input-film.md](input-film.md#layout)). The update's head and stamp are
not stored: the record's index is the update, and playback requires the
engine's tick count to equal the header's first tick count plus the
index.

**A start point** is the tagged arena and extension records of
[input-film.md](input-film.md#start-points).

**An extension block** (kind 5) is a u32 update index, a four-byte
feature tag and one zstd frame of the feature's bytes, at most 1 MiB
inflated (`film::file::MAX_EXTENSION_BYTES`). A film holds at most 65,536
(`MAX_EXTENSIONS`) and 16 MiB stored in all (`MAX_EXTENSION_TOTAL`). The
block precedes the tick chunk that holds its update, in update order: a
reader refuses one in the wrong chunk or out of order. The writer takes a
record only for the next update it will write
(`FilmWriter::extension`), and a full chunk the next tick would close is
closed first. The recorder writes the records a game tick applied before
that tick's first film tick (`theater::extension`, `record.rs`). Playback
reads the records of film tick k when tick k - 1 ends, and at the first
update for the game's first tick, and hands each to its feature
(`FilmActions::extensions_at`, `theater::extension::deliver`); the
feature applies them in the game tick that follows. A record the writer
cannot keep (too large, or past a cap) ends the film at the tick before
it. A shared copy and a recovered film keep the blocks
(`share_copy`, `recover`). The tags:

| Tag | Feature | Bytes |
|---|---|---|
| `FRGE` | a shared Forge session | the welcome that opens the session (`forge::net::Welcome`) or one stream item (`forge::net::Stamped`) |
| `FFGT` | Firefight | one entry of the host log, its canonical JSON (`host_log::Entry::canonical_bytes`) |
| `RULE` | the rules engine | a script reloaded into the running game |

A film with a tag the build does not know is unavailable, with the
feature named.

## Unified container, version 3

The stats stream, `<id>.h2stats`. All fixed-width numbers are little-endian. Offsets start at byte zero. The
first 32 bytes and the final 40 bytes have fixed locations. A web client
can read the front metadata in two short requests, then read the footer
and chronological directory when the viewer opens.

| Absolute byte | Field |
| --- | --- |
| 0 | Eight ASCII bytes `H2BLAM3!` |
| 8 | u32 version 3 |
| 12 | u32 front capacity, 131072 |
| 16 | u32 front JSON byte length (1 through capacity) |
| 20 | u32 flags = 0, u64 reserved = 0 |
| 32 | UTF-8 JSON, followed by zero padding up to byte 131104 |
| 131104 | First private `HEAD` envelope, followed by time chunks and their private twins |
| At `cidx_offset` | `CIDX` envelope containing the public time directory |
| At `indx_offset` | `INDX` envelope containing the private directory |
| EOF − 40 | Eight ASCII bytes `H2RANGE3` |
| EOF − 32 | u64 `cidx_offset`, u64 `indx_offset`, u64 complete file length |
| EOF − 8 | u32 inflated `CIDX` byte length, u32 flags = 0 |

The front object contains `id`, `image_stamp`, `runtime_stamp`,
`started_ms` (Unix milliseconds), `map_sha256`, `map_origin`, optional
`match_start_tick` and `ticks_per_second`, initial `roster` (seat, name, team and kind), and
`summary` (level, game type, mode, variant name, duration and player
results). The menu reads the front and the footer regardless of match
length. Web reads just the used prefix and JSON.
Zero padding reserves room for the final scoreboard and may be sparse
on disk. The writer puts a provisional front (identity, empty roster and
results) in the file when it opens it, rewrites it when the descriptor
arrives, and again with the roster, map and scores so far when the game is
described and every 30 ticks (`Writer::refresh_front`), so a recording a
crash or kill stops still lists. It writes the sealed front, then the
footer, last. A front that
would pass the capacity is shrunk until it fits (`Front::encode`): the
dropped-sample counts go, names are cut to 64 characters, then the medals
go. A reader names why a front is unusable: length 0 is a recording that
stopped before the front was written, and a length over the capacity is
reported with its size. Truncation makes a stats file unreadable; its film
still plays.

Each envelope uses a four-byte ASCII tag and a u32 compressed size,
followed by exactly that many bytes of one checksummed zstd frame.
The writer pledges the inflated size in the frame header. A public time
chunk uses tag `TICK`, with at most 8 MiB inflated and 8 MiB + 64 KiB
compressed. Fetch the chronological directory with the inclusive HTTP
range `bytes=cidx_offset-(indx_offset−1)`. Its inflated JSON is bounded
at 8 MiB (16 MiB compressed) and has this shape:

```json
{"version":1,"chunks":[{"first":1,"last":128,"scene":1,"offset":132000,"size":470,"raw_size":1280}],"keyframes":[]}
```

Each row uses inclusive **film ticks**: tick 1 is the game's first update. `offset` points at
the four-byte tag, `size` excludes the eight-byte envelope prefix and
`raw_size` is the inflated byte length. Fetch one chunk with
`bytes=offset-(offset+size+7)`. Rows start at tick 1 and cover consecutive ticks without gaps. Each row
covers at most 128 ticks. Seek by binary search. The first ULEB in the
inflated payload gives `first`, and the second gives `last − first + 1`.
Each following record has a ULEB tick delta, one-byte type, ULEB payload
length and payload bytes. The first delta is relative to `first`, then
deltas are relative to the preceding record. Unknown types can be
skipped by length. An event's payload may identify an earlier guest tick
than its chronological observation.

The optional row member `clock` is an array of pairs
`[offset_from_first_tick, guest_game_tick_or_null]`. At a known anchor the
guest clock advances once per subsequent film tick through the next change.
Before the first anchor, or following a null anchor, no guest match clock was
observed. The writer mirrors clock records (type 6, one ULEB or an empty
payload) from the same time chunk into its index row. This maps a clip's
guest tick to a film tick without decoding any game state. After a
campaign revert, the earliest recorded occurrence wins.

`scene` gives the first tick of the most recent public scene checkpoint
chunk. To draw a requested tick, load the chunks from `scene` through
the chunk containing it and apply the sparse pose changes in order.
The replay window is at most 16 chunks and 2048 ticks inclusive. Stats
consumers skip pose records and can decode each event payload directly.
`keyframes` is empty.
Live validates public ranges against physical envelopes before authorising
access. The private index includes all envelopes through `CIDX`, then
the footer identifies both directories.

### Private envelopes

The private `INDX` is a JSON array in physical envelope order. Each row
stores `tag` (four byte values), `offset` (u64) and `size` (u32). It
lists the `HEAD` envelopes, each time chunk's `IPRV` twin, `PSUM` and
the `TIDX` blocks. The stats stream holds no simulation input, no
digest and no machine state; those are in the film file or nowhere.

Live ingest validates the envelope types. HTTP access uses the public
`CIDX` time spans checked against the actual uploaded `TICK` envelopes.
Private envelopes stay outside those spans, including when an adjacent
public chunk is selected for a clip.

### Public and private identity fields

A public join contains the seat, name, kind and team. A public host migration
contains only its event kind. The public stats roster retains appearance and
player slots. Host identifiers, XUIDs, console names, session identifiers,
private simulation settings and bot parameters remain in private envelopes.
The public summary omits player XUIDs. When those exist, a private `PSUM`
envelope stores the full summary JSON.

When a time chunk contains private fields, the writer immediately precedes
its public `TICK` with a private `IPRV` envelope. Both frames have the same
first tick, count and record framing. `IPRV` holds the original lifecycle
and closing batches and all six original stats-import event kinds in order.
It can also preserve records with unrecognised extensions. Public records
use the recognised display fields. The matching private `TIDX` block stores
`identity_sha256`, the SHA-256 of the uncompressed `IPRV` bytes. Live imports the original stats from the pair
once. A chunk with entirely public content can omit `IPRV`.

Live records a positive public-range validation decision on the upload receipt.
A legacy receipt without that decision, or a film carrying an unrecognised
or identity-bearing public record, remains available only through private
storage. User-supplied display names, map names and chapter labels are public
display text. The first supplemental v8 sample in each time chunk which
changes appearance starts a fresh predictor. This lets Live check the
bounded binary pose grammar and consume each packed sample fully before
admitting its raw bytes as a public range. Object transforms still use the
sparse scene checkpoints in `CIDX`.

### Tagged records

| Tag | Name | Payload |
| ---: | --- | --- |
| 1 | (unused) | Not written. Actions are in the film file. |
| 2 | Event | One-byte subtype and its payload. Subtype 0 is an ordered JSON array of engine lifecycle events. Subtype 1 is a single UTF-8 JSON object with `tick`, `kind` and `data`. Subtype 2 is a lifecycle array observed after the final completed tick and its hash. |
| 3 | Pose | A presentation operation, including sparse object changes or a seek baseline. See the pose layout below. |
| 4 | (unused) | Not written. Digests are in the film file. |
| 5 | Marker | UTF-8 chapter name. The recording marks its start and the first active map. |
| 6 | Clock | One ULEB for the current guest match tick, or empty while unavailable. The index mirrors clock discontinuities for clip lookup. |
| 7 | Forge entry | A shared Forge session's presentation: one-byte subtype, then the message. Subtype 2 is a seat's monitor look (JSON) and subtype 3 its monitor pose (27 bytes, with its tick); both repeat at each seek checkpoint ([forge-monitor-appearance.md](forge-monitor-appearance.md), "Theater"). The session's welcome and stream items are simulation input and are the film file's `FRGE` extension blocks. The record is private: `film::privacy` never makes it public. |
| 8 | (unused) | Not written. A Firefight host log entry is the film file's `FFGT` extension block ([firefight.md](firefight.md#theater)). |

The event envelope carries both the film observation time and, in subtype 1,
the originating match clock. A late hit, medal or final scoreboard retains
its original time. The game statistics projection consumes GameStarted,
PlayerJoined, PlayerLeft, PlayerKilled, GameMedal, Superbounce, Bounced,
Teabag, ScoreChanged and GameEnded (`Record::decode_telemetry`); a reader passes over a kind it
does not know, and `film::privacy` keeps a record it cannot decode private,
so a film with bounces reads in an older reader as one without them.
Highlights include actor kills, additional weapon fire, damage, assists,
medals, item and vehicle transitions and objective changes. A highlight
that duplicates a canonical event only contributes once to stats.

`ActorKilled` carries the match tick and salted unit object datum, with
the character tag datum, credited player slot and damage-source key when
available. It comes from the report that depleted an actor unit's body.
It is public stats data, with no input, score or digest attached. Player
kill totals come from `PlayerKilled`.

`GameMedal` carries the match tick, the player's slot and `medal`, the id
of a medal the game itself awarded, one of the 24 in
`film::events::medals::OFFICIAL` (`double-kill` to `bomb-diffused`,
[live-gamestate.md](../live/live-gamestate.md#medals)). It is a public
stats record, imported into the game's events like a kill. The Tracker
writes a `Medal` record for it at the same tick and counts it in the
summary's `medals` map. Kill medals are not worked out from the kills:
the game's own award is the only source.

`Superbounce` carries the match tick, the player's slot, `chain`, the
bounce's place in its chain (1 and up), and `monster`, true for a Monster
bounce and left out when false (absent reads as false); `Bounced` carries the match tick,
slot and ended chain's count (0 if none) of a death falling out of the map while trying a bounce
([superbounce.md](superbounce.md#recording)). Both are public stats
records, imported into the game's events like a kill. The Tracker writes,
at the same tick, a `Medal` record for each medal the bounce earns
(`film::events::medals::bounce_medals_with`: `superbounce`, then
`double-bounce`, `triple-bounce`, `quad-bounce`, `chain-bounce` or
`bouncetacular` at chain 2 to 6, then `monster-bounce` for a Monster) or
`bounced` for the death, and counts them
in the summary's `medals` map. The film's version stays 3.

`Teabag` carries the match tick, the crouching player's slot (`player`)
and the slot of the player whose body it was (`victim`)
([teabag.md](teabag.md#recording)), a public stats record like a bounce.
At `GameEnded` (reason `finished`) the Tracker writes an `incel` `Medal`
at the end's tick for each player still in the game who was in it before
its first kill and in no teabag (`film::events::medals::Incel`,
[teabag.md](teabag.md#the-medal)), and counts it in the summary.

## Keyframes and seeking

Neither file holds a keyframe or a seek index for the simulation. The
player reaches a tick by simulating to it, from the game start or from a
game-state snapshot it took earlier in the same viewing session
([theater-design.md](theater-design.md#seek)). The stats stream's `scene`
checkpoints are for the web viewer's poses and restore nothing in the
engine.

## Determinism and verification

A film plays when four things equal the recording's: the XBE (SHA-256),
the simulation contract, the simulation hook digest, and the map's bytes
(SHA-256). The variant, the players and the simulation-scope settings are
in the film and are applied from it. A converted Halo 1 or mod map with
the same display name can have different bytes; the digest decides.

- **The simulation contract** is `compatibility::SIMULATION_ABI`. It
  covers what the simulation computes from a start state and its
  updates. A change that alters that must change the contract.
- **The hook digest** (`compatibility::hook_digest`) is a SHA-256 of the
  installed simulation hook chains in execution order and the core game,
  clock, player and object table addresses. The player compares it after
  installing its features.
- **Not compared:** the native image stamp and target, the render hook
  groups, the telemetry observers, the scheduler's budget, the features
  that do not hook the simulation, and anything of the recorder's disk.
  Editing Live, the web viewer, codecs, diagnostic labels or
  documentation leaves a film playable.

The checks in playback, in order:

1. The start hash, before the first update is applied.
2. The tick count against the record's index, at every update.
3. Sixteen bits of the gameplay digest after every tick. It covers the
   gameplay the sim harness observes, including the random word and
   player state.
4. The whole 64-bit gameplay digest at the end of every chunk.

Playback reports the first observed divergence and its tick, and stops
there. The render steps the shared random word, so a recording and a
playback both draw every world frame inside the private stream
(`render_rng.rs`); a film checks the same at any render cadence,
`--render-every 0` included.

Live clip endpoints use the guest match clock. A film tick is the guest's
tick count minus the header's first tick count plus one, so the in-game
player translates an endpoint with one subtraction. Local clip markers
use film ticks.

## Choosing a player's view

In a local GPU recording the guest renders its original camera, first-person weapon
and HUD. Its exact 640x480 reference provides guest readbacks, while
the viewer chooses the window resolution. A local film can display
one split-screen viewport. Its guest-pixel edges map onto the displayed
raster so the selection tracks resolution changes. The slot is the player's
seat in the film roster.

```sh
h2 agent theater.camera '{"mode":"first_person","seat":0}'
```

The selection takes effect at presentation and survives a seek. A local
seat is available while the game draws its first-person director. During
a cinematic, death or vehicle camera the window shows the recorded frame.
`theater.camera {"mode":"default"}` returns to the whole local frame.

A playerless host has no local director, first-person weapon or HUD. For
a rendered host film the desktop player opens a separate GPU view at the
viewer's selected resolution. Native follows the physical viewport, including
a resize while paused, with a 4K pixel budget in fullscreen. The presented aspect sets the observer's horizontal
field of view. The BSP and posed objects use the same projection for drawing
and culling. The observer uses the selected unit’s posed camera
marker, aim and unzoomed field of view. When the marker is unavailable,
it estimates eye height from the biped’s collision capsule and crouch state.
The scene uses the active BSP with its base textures and lightmaps, and
the posed opaque models of the visible objects. First-person mode hides
the selected unit and its held weapons. The independent view omits retail
HUD and weapon overlays, transparent effects, sky and post-processing.

Rendered host playback starts with a free camera at the first available
player. Select `first-person` with a roster name or an absolute seat to
follow a particular player's eye. The spelling `first_person` also works.
The `default` host view follows the first available eye. A targeted
first-person view clears during death or disconnect and resumes when the
seat has a camera again. Follow, orbit and free modes keep their last pose
when a subject disappears. The player carries the rig, target, clip marks
and discovered kill markers through a seek.

```sh
h2 agent theater.camera '{"mode":"orbit","seat":3,"look":[0.25,0],"zoom":-1}'
```

Changing the view or the display cadence adds no draw to the
simulation's random stream. Compare digests for a headless and a visible
playback of the same film when verifying this path.

## Missing observations

The front summary's `dropped_samples` maps each affected source to its count.
A failed guest read can leave a gap in a pose, pickup or damage event. The
recorder retains each participant's last readable score and team. When the
engine signals game over with an unreadable final scoreboard, the event keeps
the finish time and leaves its winner and final rows unavailable.

Within each completed tick the writer allows up to 512 KiB of canonical Live
events and 512 KiB of highlights, including framing. The same budgets apply
to the final batch after the last tick. Each event also has a 128 KiB limit.
An oversized event increments `stats_encoding` or `highlight_encoding`.
Overflowing either batch increments `stats_budget` or `highlight_budget` for
each omitted record. The film file is unaffected.

## Weapon fire, damage and projectile endpoints

Each observed retail barrel call produces one `WeaponFire` record. It carries the
specific weapon datum and definition, barrel index, holder and player identity
when present. The shooter's origin and aim precede spread and aim assistance.
A held weapon can fire between two pose samples, including the second weapon
of a dual-wielding player.

A `Damage` event carries the established Live hit for stats and the target
position. The accompanying `Impact` carries the guest damage report's origin,
direction, distance, victim, source object, packed reporting code and damage
effect definition. Area damage can name the same blast origin for several
victims. A viewer can use the origin to place one explosion.

A `ProjectileEnded` event captures the last transform position and velocity
at object deletion. This includes bullets that spawn and disappear between
sampled frames. Coordinates are relative to the parent when a projectile is
attached. An endpoint alone does not identify a wall hit, its surface normal
or material. The engine also deletes projectiles at maximum range and during
clean-up. These films preserve that distinction by leaving the cause unknown.
Particle, flash, decal and explosion instances have no object-pose records.

## Objective and power-up events

An `ObjectiveState` event carries an `action` and the fields for that action.
`item` gives the registered flag, bomb or ball, its salted object datum,
designator, current carrying unit and player slot. The designator identifies
the owning team (8 for neutral) or the ball number in Oddball. The change
is `registered`, `carrier_changed` or `unregistered`. A carrier change with
an empty unit records a drop. Object salts distinguish respawns that reuse a
slot. At unregistration the event retains the last observed carrier.

`bomb_state` gives the arming, armed and planted flags for the bomb's team.
`hill` gives the active hill index and 16-bit scoring-player mask. Its
geometry lives in the pose record. `territory` gives a site's owner, team
and occupants, and `juggernauts` carries the current role mask. Each bit
corresponds to a roster slot. These are completed-tick observations.
A mod using a retail game engine inherits that engine's observations.
A custom scoring mechanism needs its own observer to supply named actions.
Score changes still come from the retail scoreboard.

`PowerupTaken` records the salted equipment, its definition tag, the
recipient unit and its player slot when present. The host observes the
successful application before the engine deletes the equipment, including
for equipment outside a scenario placement. [Retail objective and pickup
layouts](bots.md#37-objectives-traced)

## Tag 3 presentation records

The sealed summary's optional `dropped_samples` object counts rejected
observations by source. An unreadable object header table holds the whole
scene until the next census. When the table identifies a live object but its
body, transform or animation is invalid, the viewer holds that object's
previous pose. Other objects still move, appear and disappear. An invalid hill
sample holds the last polygon. A failed objective-registry read retains the
last object roles. Supplemental player and actor rows recover independently.
The recorder continues writing simulation inputs and post-tick hashes.

The enclosing `TICK` supplies the time. A client seeks via the `scene` tick in
`CIDX` and applies pose records from that checkpoint in file order. The host
starts a new scene at a chunk boundary after about 1,800 ticks (one minute at
30 Hz), or earlier to keep each seek within 16 chunks. The indexed window is
shorter than 2,048 ticks. A checkpoint carries the last observed
scene before that tick's changes. An unchanged scene takes no further pose
records until the next checkpoint.

The first payload byte selects one of three layouts. Counts, lengths, object
operation headers and lane masks use canonical unsigned LEB128. Signed
integers use ZigZag then unsigned LEB128. A reader rejects trailing bytes.

| Byte | Remaining payload |
| ---: | --- |
| 0 | Complete object list and hill in the edit format below. Then a packed-frame byte length and that many bytes, zero for an absent supplement. A present supplement is a Live v8 key, stream 0, sequence 0. Replace the scene and restart the packed-frame predictor. |
| 1 | Object edits and hill edit. Apply to the current scene. |
| 2 | One Live v8 key or delta for stream 0 occupying the rest of the payload. It updates the browser's supplemental player/actor fields. |

The object list begins with the operation count. Within a list, datums
increase. Each operation begins with `(datum − previous_datum) × 4 + opcode`,
starting with previous datum zero. These are 32-bit **salted** handles. A
reused slot gets a new identity. Opcodes 0 (create) and 1 (update) carry a
changed-lane mask. Opcode 2 (destroy) ends at the operation header. A full
list contains creations only. The engine object pool has 2,048 slots, so a
delta can contain up to 4,096 deletions and creations combined.

For each set mask bit 0–23 in ascending order, add the signed lane delta
to its previous value. A new object starts with zeroes except for lane 11,
which starts at 16,384 (world up). Optional integer handles and indices use
zero for absent and the underlying value plus one for present as indicated
in the table. Mask bits 24 and 25 are reserved. Bit 26 replaces the tag name
with a byte length and UTF-8 bytes, up to 256 bytes. Zero length clears it.
Higher bits are reserved.

| Lane | Value |
| --- | --- |
| 0 | Guest object class, in order from 0: `bipd vehi weap eqip garb proj scen mach ctrl lifi ssce bloc crea`. |
| 1–4 | Definition datum + 1, scenario placement + 1, multiplayer equipment spawn + 1, parent datum + 1. |
| 5–7 | Signed 32-bit X, Y, Z in centimetres. One guest world unit is 304.8 cm. Attached objects retain parent-local transforms. Largest rounding error is 0.5 cm per axis. |
| 8–11 | Forward yaw and pitch, then up yaw and pitch. One turn is 65,536 units. Yaw deltas wrap through signed 16 bits and decode modulo 65,536. Pitch is signed, −16,384 to +16,384. Each angle rounds to within half a unit. |
| 12–15 | Vehicle seat + 1, aim-present flag, aim yaw and pitch in the same units. |
| 16–19 | Animation graph datum + 1, signed inherited graph and animation indices, signed animation frame in sixteenths. An absent graph clears all three following lanes. |
| 20–23 | Normalised device travel rounded to 0–255 then plus one, depleted flag, controlling player datum + 1, objective kind (0 ordinary, 1 flag, 3 ball, 9 bomb). |

After the object operations, a hill byte of 0 leaves the current hill alone,
1 clears it and 2 sets it. A set carries one byte of hill index, a corner
count (at most 256), and three signed absolute centimetre coordinates for
each corner in boundary order. On a baseline the initial hill is empty.

A supplemental frame supplies the BSP, cinematic and checkpoint state,
animation names, weapon and stance, vitality, velocity and observed actions.
The dictionary supplies matched objects' transforms and animation time. A
player whose unit has gone can retain the last known body location. Seeking
starts with persistent supplemental state, with transient actions cleared.
The browser reads weapon fire and damage from the event records to draw hit
and firing effects. A slow projectile (under 100 world units per second)
or attached projectile gets a pose immediately. A faster projectile gets
a pose once its salted handle survives to the next presentation sample.
Particles, decals, muzzle flash and hitscan traces have no pose objects.
[Live packed frames](../live/live-telemetry.md#packed-frames)

## Size targets

These are engineering budgets, in bytes per player-minute, for a 30 Hz match
with 15 Hz presentation sampling. They are estimates pending measurements on
sealed matches. Event rate, active AI, and object churn all affect size. Fixed and match-wide costs are shown separately so
that an eight-player and a one-player game can be compared.

| Contents | Budget per player-minute | Basis |
| --- | ---: | --- |
| Event | 1,000–8,000 compressed | Roughly 60–250 fire, hit, scoring and lifecycle records. Event type and field names repeat within a zstd frame. |
| Pose, player | 9,000–21,000 before zstd | Up to 900 moving samples, including animation and aim. Shared objects are accounted for below. |
| Marker | <100 ÷ players at one short chapter per minute | Cost is proportional to the UTF-8 label and its framing. |
| Clock | About 75–120 ÷ players | One anchor per chunk during continuous 30 Hz play, plus anchors at clock discontinuities. |
| Front and directories | Fixed 131,104 bytes per film for the front, plus index rows per chunk | Divide the fixed cost by match minutes and players. A menu client can request just the prefix and occupied JSON bytes. |

For the world pose dictionary, these budgets are **per object**, before zstd,
excluding infrequent full seek baselines. A resting object generates zero
movement updates. Creation carries identity, class, initial transform and
observed state. Destruction costs roughly 1–5 bytes for the identifier plus
shared operation framing. A respawn gets its own salted object identity.

| Object-table class | Typical changed sample | Example marginal budget per object-minute |
| --- | ---: | ---: |
| 0 biped (`bipd`), including aim and animation | 12–24 bytes | 10,800–21,600 at 15 Hz continuously moving |
| 1 vehicle (`vehi`), including orientation and available turret aim | 10–22 bytes | 4,500–9,900 moving half the minute, plus 3–10 per occupant attachment change |
| 2 weapon (`weap`), including registered flag, ball or bomb | 5–10 bytes for translation, 3–10 for an attachment change | 75–150 for one second of free movement plus each pickup/drop |
| 3 equipment (`eqip`), including powerups | 5–10 bytes for translation | 75–150 for one second of free movement, plus creation and destruction |
| 4 garbage (`garb`) | 5–10 bytes for translation | 75–150 for one second of movement |
| 5 projectile (`proj`), including grenades | 5–10 bytes in flight | 75–150 per second of flight, plus creation and destruction |
| 6 scenery (`scen`) | 5–10 bytes for translation, 10–22 with rotation | 375–1,650 for five moving seconds |
| 7 machine (`mach`), including doors, lifts and gates | 5–9 bytes for travel alone | 375–675 for five seconds of travel |
| 8 control (`ctrl`) | 5–9 bytes for a device-state change | 10–18 for two state transitions |
| 9 light fixture (`lifi`) | 5–9 bytes for a device-state change | 10–18 for two state transitions |
| 10 sound scenery (`ssce`) | 5–10 bytes for translation | Zero while stationary, 75–150 for one moving second |
| 11 crate (`bloc`), including movable or destructible physics props | 5–10 bytes for translation, 10–22 with rotation | 375–1,650 for five moving seconds |
| 12 creature (`crea`) | 5–10 bytes for translation plus changed orientation/animation | 4,500–9,900 at 15 Hz moving half the minute with rotation |
| Active hill (scenario state) | Index, count and three signed coordinates per vertex | About 28–40 for a four-corner hill changing once, at two to three bytes per coordinate |
| Decals, particles, muzzle flash and explosions | 0 pose bytes | Viewer effects use the timestamped event records |

The presentation rows share chunk framing, seek baselines and compression.
The film file's sizes are in [input-film.md](input-film.md#size-step-by-step).

## Live storage

A dedicated host sends the sealed film and its stats stream under the
canonical lease game ID through `POST /v1/games/{game}/film`
(`film::upload`). Live stores the film as sent, reads its header and game
start, and inflates no tick chunk. Recent and saved film listings expose
`has_host_film` and the summary after publication. Live derives the
player and game statistics from the stats stream's canonical event
records. If an idle timeout or another recorded event disagrees with the
existing signed stats, Live keeps the film, publishes its summary and
identifies the first disagreement in `stats_discrepancy`. The existing
ledger remains the source of ranking awards. After a failed upload the
supervisor keeps the sealed files in its `pending-films` cache or prints
the original lease path if recovery storage is unavailable.
[Live film API](../live/live-api.md)
