# Theater: the full in-game player

Status: Design. The per-update action and body capture and apply, the film file and its column and tail codec, the game start block, start points, the camera rig, the film selector, the Live film routes and the recorded look in first person (`coop_look::follow_film`, slice 9) are built; a film plays from its game start, in the running game and in a film player process, one film tick for each application of the update (slices 1b, 2, 3, 4 and 5). A default recording writes one film for each game, `<id>.h2film` and `<id>.h2stats`, opened at the game's activation with tick 1 its first update and sealed at its end; there is no session tape, cut or fork, and the library prune seals a hard-killed film at its last checkpoint (slice 6). The machine film is gone from Theater and the film crate: a film without a game start does not open, a seek runs in the process that plays the film, and nothing is restored from a keyframe (slice 7a). The call tape is gone: no HLE call, pad read, keyboard command, bot outcome, lobby call or network result is recorded or answered from a film, the stats container holds no machine section, arena hash or mark, and the film file holds no machine part (slice 7b). A dedicated host uploads its film file and stats stream, a machine uploads its own films as the host does, and a participant's Watch reads the host's film file from Live by range; Live stores the file as sent, reads the film file with the lease's token only, serves it to any signed-in account that can see the game, and charges a pinned game for it (slice 12; the run of a host film's arena hashes on a local playback is not made). The session ring is built: a seek behind the tick shown loads the latest entry at or before its target in the running game and runs forward from it, `theater.state` reports the route `ring` and `theater.keyframes` the entries (slice 8; its run, the after-restore step and the timeline's covered stretch are not made). Extension records are built, with a shared Forge session's edits their first user, and an outside change with no record ends the film at the tick before it (slice 11). A film plays the Forge monitor looks and poses its stats container holds, a game tick at a time (`film_play::Looks`), a start-route seek reads them again from the start, and a ring jump puts back the entry's and reads on from its tick.
Code: `crates/extensions/game/src/theater.rs` (`Session`), `crates/extensions/game/src/theater/actions.rs` (the update hook), `crates/extensions/game/src/theater/record.rs` (the recorder's tick), `crates/live/film/src/columns.rs` (the chunk codec), `crates/extensions/game/src/theater/film_play.rs` (play from the game start), `crates/extensions/game/src/theater/watch.rs` (watching in the running game), `crates/extensions/game/src/theater/pacing.rs`, `crates/live/film/src/file.rs` (the film file), `crates/extensions/game/src/theater/start_point_state.rs` (arena state). Tests: `crates/extensions/game/src/theater/tests.rs`, `crates/extensions/game/src/theater/film_play/tests.rs`, `crates/extensions/game/src/theater/watch/tests.rs`, `crates/live/film/src/file/tests.rs`.
Switch: [the film player's `H2_THEATER_*`](../runtime/switches.md).

Theater plays a film in the running game: pause, rewind, jump, change
speed, fly a camera, follow any player, take screenshots and clips, and
share a film through Live. A film is what Halo 3's is
([notes/halo3-film.md](notes/halo3-film.md)): the game options a game
started from, and one simulation update for every tick after. The game
loads the map with those options and simulates the updates again. A film
holds no disk file, no profile, no menu, no controller read and no
machine state, so it plays on any machine with the same build and map,
whatever that machine's saves are. [film-format.md](film-format.md) is
the file; [input-film.md](input-film.md) is the encoding of its tick
chunks. The rule that must hold: nothing reaches the replayed simulation
but the film's start state and its updates, and every tick proves it with
the film's digest.

Evidence labels: **Code** (read in this repository, path given), **Test**
(a named test), **E** (read from the retail XBE, as recorded in the linked
page), **Run** (seen in a running game, with the note), **S** (supposed,
with what would verify it), **Estimate** (arithmetic from cited numbers),
**Design** (our choice).

## Stats and film

Two separate things are recorded, in two files, and neither holds the
other's data.

| | Stats stream (`<id>.h2stats`) | Film (`<id>.h2film`) |
|---|---|---|
| Answers | what happened | how to make it happen again |
| Holds | events (kills, medals, scores, joins, objectives), the scoreboard and carnage report, positions and poses for the web viewer | the game options, each tick's simulation update, a digest for each tick |
| Does not hold | updates, digests | poses, events, scores |
| Read by | Live, the web, the stats ledger, on any build; nothing is simulated | the engine only, on a build whose simulation contract matches |
| After a breaking simulation change | still reads | refused |

Watching a game on the web is the web viewer drawing the stats stream
([web-viewer.md](../live/web-viewer.md)). No engine runs. Theater is the
game simulating a film. The stats stream is the `H2BLAM3!` container of
[film-format.md](film-format.md#unified-container-version-3).

## What the player gets

| Feature | Behaviour |
|---|---|
| Play a film | From the main menu's Theater, a film page's **Watch**, or a link. The game loads the film's map and plays; leaving returns to the film page. |
| Pause | Holds the simulation. The camera, HUD toggle and screenshots keep working while paused. |
| Speed | 0.25×, 0.5×, 1×, 2×, 4×. The bar shows the speed reached when the machine cannot keep up. |
| Rewind and jump | Tap for 5 s, hold for 30 s, or pick a point on the timeline. A jump to a stretch already watched lands in about two seconds; a jump ahead simulates forward unrendered and shows a progress bar that can be cancelled. |
| Step | One tick forward while paused. One tick back is a jump to the tick before. |
| Cameras | Recorded view, first person for a chosen player, follow, orbit, free flight. |
| HUD | Game HUD on or off; Theater's own bar on or off. |
| Screenshot | A clean frame without Theater's bar, at the window's internal resolution or a multiple of it. |
| Clip | Mark in and out. A clip is a range of the film; **Export** renders it to a video file. |
| Share | **Share** uploads the film and its stats stream once and gives a link. The link opens the web viewer; its **Open in Halo 2** button opens the film in the game. |
| Films from other builds | Played when the simulation contract matches, refused with the reason otherwise. The film page says which before the player presses Watch. |

## The film

| Part | Holds | Source |
|---|---|---|
| **Stamp** | the XBE digest, the simulation contract and hook digest, the build | `theater/compatibility.rs` |
| **Game start** | the game options block by value, the two shared random words, the map's identity and digest, the simulation-scope settings, the arena hash before the first update | the active options at `GAME_ACTIVATE`; "The start state" |
| **Update stream** | one record for each simulation update the game applied: the update's body, and 16 bits of the gameplay digest after its tick | `SIM_UPDATE_APPLY`; "The update stream" |
| **Start point**, only in a film that begins mid-game | the game-state arena and extension records at the tick before the first update | `theater/start_point.rs`; "Films that begin mid-game" |

### The start state

The start state is the engine's own game options and nothing from disk.

- **The options block** is the block every game starts from: `OPTIONS_NEW`
  clears and defaults it, the menu's campaign start or the session's game
  start fills it, and `GAME_ACTIVATE` copies it into the game globals
  (**E**: [level-boot.md](../runtime/level-boot.md#how-the-game-starts-its-first-map),
  [live-gamestate.md](../live/live-gamestate.md)). It holds the scenario
  kind and simulation mode, the campaign and map ids and scenario path,
  the campaign difficulty, the tick rate, the seed words, the whole game
  variant by value, and the player block with its 16 player entries. The
  recorder copies the active block whole when `GAME_ACTIVATE` returns
  success (`metadata::active_options`).
- **Players** are the block's player entries, by value: each player's
  name, appearance, team and controller, as the menu or the session wrote
  them from the profiles at game start (**E**: level-boot.md, the menu
  campaign start and `OPTIONS_PLAYERS`). The film reads no profile, and a
  profile saved or deleted after the game changes nothing.
  **S**: the entry holds everything of a profile the simulation reads;
  the start hash on a machine with another profile verifies it.
- **The random words** are the two shared seeds
  (`sim_glue::RANDOM_SEED_PTR`), read at the first update and written back
  there in playback.
- **The map** is named by its scenario path and identified by the SHA-256
  of its bytes (`film::blam::MapIdentity`). A disc map, a converted Halo 1
  level, a mod map and a Forge map are all a digest; the player's own
  library supplies the bytes, and Live serves a map it hosts by digest
  ([maps.md](maps.md)). The film holds no map bytes.
- **The game variant** is in the options by value. A portable variant
  record with a rules script, and a Forge map variant, travel by value in
  their own sections ([film-format.md](film-format.md#the-film-file)).
- **Simulation-scope settings** are the values of every setting whose
  scope is `Sim` (skulls, co-op options) and the feature switch spec, by
  value. Playback applies them for the film's game and puts the viewer's
  back after.
- **The start hash** is `lockstep::hash_gameplay(SimRegion::HALO2)` at
  entry to the first update: the arena and the extension records' digest,
  with the arena's render-side data arrays (`particles`, `particle_*`,
  `effect`, `effect location`, `lights`, `decals`, `cached object render
  states`, `object looping sounds`, `cloth`, `liquid`; `lockstep::gameplay`)
  the free slots of every data array (`datum_delete` clears an element's
  salt and nothing else, so a free slot holds what an earlier game left)
  two words the simulation does not read (`+0x28d8`, `+0x28f8` of the
  arena: the time globals' neighbours) and a 45 KB buffer at `+0x3ea000`
  (2872 records of three floats and a packed word, written as a game
  plays, in the pool of the last array; its owner is not identified)
  zeroed first. The per-tick digest of
  a campaign, which is the whole arena's hash, has the same coverage. The
  hash is also of a game that starts from what a fresh process has: before
  every `GAME_ACTIVATE` the salt counter of each data array is set to what
  `data_delete_all` seeds it with (`data_salts`, feature `data-salts`),
  because an array a map load empties one datum at a time keeps the salts
  it had reached, and every handle a game stores carries one. The object
  memory pool (the `loopobjects` header after the `object` array) is not
  zeroed when objects are freed either, so before `GAME_ACTIVATE` it is
  zeroed when no object is live (`object-pool-clear`), past its first block
  header (the pool is a chain of blocks, each a 0x20-byte header and a body,
  and the chain cannot be walked after games have been through it); with live objects it is left, counted and logged
  once. The object globals just past the pool (its end `+0x28` to `+0x80`:
  the next object sequence number, which every object's body stores, and
  the placement scratch after it) are zeroed with it, because a map load
  does not reset them. Twelve bytes in the pool of the `noncollideable
  object cluster refs` array and a word after the `flocks` array's bitmap
  are stale slack in a game after another and are masked
  (`lockstep::gameplay::SLACK`). Playback compares it before it applies the first
  record and refuses a film whose start it did not reproduce.

### The update stream

`game_tick` builds one simulation update for every tick of every game,
local games included, and `SIM_UPDATE_APPLY` applies it. It is the record
the engine's synchronous protocol sends whole to a client, which then
simulates from nothing else (**E**:
[live-coop.md](../live/live-coop-lockstep.md#13-the-synchronous-machinery)). Co-op
over Live runs campaign on that path (**Run**: live-coop.md, the status
at its head). The film stores that record, as Halo 3's film does.

| Part of the update | Holds | In the film |
|---|---|---|
| Head | the update number | not stored; the record's index is the update |
| Action block | a mask of player slots and each one's action, after pads, remote arrivals and phantom actions are resolved | stored, as columns ([input-film.md](input-film.md#the-encoding)) |
| Player entries, the player-set change, the event list, the save flag | players whose properties change, a join or leave with its identifiers, queued simulation events, "save the game state on this tick" | stored, as the bytes that differ from the update before |
| Stamp | the tick count and the game's random word | the tick is checked, not stored; the random word is covered by the digest |

- **The recorder** reads the action block and the body (the bytes from
  the player-state mask at `+0x5cc` to the stamp at `+0x4040`) at entry
  to `SIM_UPDATE_APPLY` (`actions.rs`, `capture`, `capture_body`), once
  for each application, and never writes the head or the stamp. A game
  tick that applies two updates, as a synchronous client catching up
  does, gives two records, each a film tick (`record.rs`,
  `Session::record_update`); the game tick's digest and events sit on the
  last of them. The chunk stores a body as the bytes that differ from the
  record before it, zeros for a chunk's first
  ([input-film.md](input-film.md#layout)). The recorder requires each
  update's tick stamp (`capture_stamp`) to be the tick of the update
  before, the next, or an earlier one (a revert puts the clock back)
  (`stamp_follows`), and seals the game's film at the tick before an
  update that is further on: that update and every after it in the game
  are not kept.
- **The player** runs the film's game as a local game and writes the
  recorded action block and body over the ones the engine built, at the
  same hook (`actions.rs`, `apply`, `apply_body`). The engine's own apply
  then adds and removes players, applies property changes and events, and
  runs the tick. Each application takes the next film tick, so a game
  tick that applies the update twice covers two, and the digest that is
  checked is the one of the film tick the game tick ends on
  (`film_play.rs`, `tick_done`); a game tick with no application covers
  one film tick that must hold no action.
- **A film is recorded by a machine that simulates the whole game**: a
  local game, either side of a synchronous game, or the host of a
  distributed game. A client of a distributed game is sent state, not
  updates, so its machine records no film; its film is the host's
  ("Kinds of game").
- **Extension records.** A feature of ours that changes the simulation
  from outside the update writes its input into the film on the tick it
  applied it: the film file's extension blocks, tagged by feature
  ([film-format.md](film-format.md#the-film-file)). A shared Forge
  session's welcome and edits are the first user (`FRGE`); Firefight's
  host log and a rules script reload have tags and are not wired. The
  recorder writes a game tick's records before its first film tick; the
  player hands them to their feature when the tick before ends, and the
  feature applies them in the game tick that follows
  (`theater/extension.rs`). The Forge monitor looks and poses are
  presentation, not outside input: they stay in the stats container, and
  the player hands them to `theater::forge_film` as each game tick ends
  (`film_play::Looks`), counted in game ticks because the container holds
  one tick for each game tick. A film whose stats container is not on
  this machine plays without them. A change with no record (an editor write, an
  agent API write) ends the film at the tick before it and keeps it to
  there (`Session::outside_change`). A film with records of a feature
  this build does not have does not open.

### Films that begin mid-game

A film begins at a start point when its first update is not the game's
first: a campaign mission resumed from a saved checkpoint, a player who
joined a running synchronous game, a recording started during a game. The
start point is the game-state arena and the extension records at the tick
before, compressed, with the arena hash
([input-film.md](input-film.md#start-points); `theater/start_point.rs`,
`start_play.rs`). The player loads the map from the game options, loads
the arena, checks the hash and continues from the next update. Halo 3
does the same for a clip that starts mid-game.

A campaign revert inside a film needs none: the checkpoint it returns to
was saved by an earlier update of the same film, and playback saves and
reverts as the recording did.

### What a film does not hold

| Not in a film | Why it is not needed |
|---|---|
| The guest disk, the profile, saved games | the options carry the players by value; a resumed mission carries a start point |
| Menus, the lobby, boot | the film starts when the map is activated and ends when the game does |
| Controller reads, keyboard input | the update's actions are what the pads became |
| Network packets, Live service answers | remote players' actions are in the update |
| Guest RAM, device state, keyframes | the player makes its own snapshots as it plays ("The session ring") |
| Poses, events, scores | the stats stream |

### Size

**Estimate** ([input-film.md](input-film.md#size-step-by-step)): 2,700 to
7,700 bytes per player-minute of actions and 3,650 bytes a minute of
digests; the game start is a few kilobytes compressed, most of it the
variant and the player entries. A 10-minute game is about 0.12 MB for
one player and 0.5 MB for 16. The bytes of the update beyond the actions
are not measured. Halo 3 stores 45,000 to 63,000 bytes per player-minute
([notes/halo3-film.md](notes/halo3-film.md)).

## Pages

| Page | What |
|---|---|
| [Theater design: recording and playback](theater-design-recording-playback.md) | How a film is recorded and played back. |
| [Theater design: numbers, interfaces and limits](theater-design-interfaces.md) | Numbers, interfaces, slices, rules, evidence and risks. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Recording

See [Theater design: recording and playback](theater-design-recording-playback.md#recording).

## Playback

See [Theater design: recording and playback](theater-design-recording-playback.md#playback).

### Starting a film in the running game

See [Theater design: recording and playback](theater-design-recording-playback.md#starting-a-film-in-the-running-game).

### Pause, speed and step

See [Theater design: recording and playback](theater-design-recording-playback.md#pause-speed-and-step).

### The session ring

See [Theater design: recording and playback](theater-design-recording-playback.md#the-session-ring).

## Numbers

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#numbers).

## Interfaces

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#interfaces).

## Slices

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#slices).

## Rules and limits

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#rules-and-limits).

## Evidence

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#evidence).

## Not verified

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#not-verified).

## Risks

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#risks).

## Related

See [Theater design: numbers, interfaces and limits](theater-design-interfaces.md#related).
