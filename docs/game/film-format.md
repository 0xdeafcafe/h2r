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
  ([theater-design.md](theater-design-recording-playback.md#account-film-library)). A
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

## Pages

| Page | What |
|---|---|
| [Film format: the unified container and its records](film-format-container.md) | The version 3 container, keyframes, determinism, event records, size targets and live storage. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Unified container, version 3

See [Film format: the unified container and its records](film-format-container.md#unified-container-version-3).

## Keyframes and seeking

See [Film format: the unified container and its records](film-format-container.md#keyframes-and-seeking).

## Determinism and verification

See [Film format: the unified container and its records](film-format-container.md#determinism-and-verification).

## Choosing a player's view

See [Film format: the unified container and its records](film-format-container.md#choosing-a-players-view).

## Missing observations

See [Film format: the unified container and its records](film-format-container.md#missing-observations).

## Weapon fire, damage and projectile endpoints

See [Film format: the unified container and its records](film-format-container.md#weapon-fire-damage-and-projectile-endpoints).

## Objective and power-up events

See [Film format: the unified container and its records](film-format-container.md#objective-and-power-up-events).

## Tag 3 presentation records

See [Film format: the unified container and its records](film-format-container.md#tag-3-presentation-records).

## Size targets

See [Film format: the unified container and its records](film-format-container.md#size-targets).

## Live storage

See [Film format: the unified container and its records](film-format-container.md#live-storage).
