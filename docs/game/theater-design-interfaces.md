# Theater design: numbers, interfaces and limits

Numbers, interfaces, slices, rules, evidence and risks. Part of [theater-design.md](theater-design.md).

## Numbers

| Quantity | Value | Source |
|---|---|---|
| Game options block | one block, a few kilobytes compressed | **E**: [live-gamestate.md](../live/live-gamestate.md) |
| Game-state arena | 4,186,112 bytes | **Code**: `crates/platform/xapi/tests/checkpoints.rs` |
| Tick rate | 30 Hz | [high-framerate.md](high-framerate.md) |
| Film bytes | about 0.12 MB (one player) to 0.5 MB (16) for 10 minutes | **Estimate**: [input-film.md](input-film.md#size-step-by-step) |
| Ring entry | about 0.26 MB; one every 150 ticks; 120 entries, 31 MB, for a 10-minute film | **Estimate** |
| Ring cap | 256 MiB, `H2_THEATER_RING_MIB` | **Design** |
| Simulation cost | 14.12 ms a tick on the heaviest profiled case | [sim-budget.md](../perf/sim-budget.md) |
| Unrendered run | 2.4× real time at 14.12 ms a tick | **Estimate**: 33.3 ÷ ms per tick |
| Jump inside the ring | worst 150 ticks: 2.1 s at 14.12 ms | **Estimate** |
| Jump from the start, 10-minute film | a map load, then up to 18,000 ticks: 4.2 min at 14.12 ms | **Estimate** |

### Determinism checks

| Check | When | On failure |
|---|---|---|
| Start hash | before the first update | the film does not start; the reason names it |
| Tick stamp | each update: the engine's tick count equals the record's index plus the film's first tick | playback stops at that tick |
| Gameplay digest, 16 bits in the film | every tick | playback stops with "film diverged at mm:ss" |
| Gameplay digest, 64 bits | the last digested tick of every chunk (256 ticks) | the same |
| Arena hash of a ring entry | after its load | the ring is dropped and turned off for the film |

## Interfaces

### Agent API

`crates/extensions/agent-proto/src/theater.rs`, documented in
[agent-api.md](../runtime/agent-api.md#theater-transport).

| Method | Parameters | Result |
|---|---|---|
| `theater.watch` | `{id \| path, tick?, seat?, camera?}` | queues the film in the running game; `{id, length_ticks, queued}`. The transport answers once the film's game has activated |
| `theater.state` | `{}` | `tick`, `length`, `paused`, `speed`, `speed_reached`, `seek` (`{target, from_tick, source: "forward" \| "ring" \| "start", requested?: "boot", remaining_ticks, restore_ms}` or null), `verified_through`, `divergence` (`{tick, kind}` or null), `camera_pose`, `hud`, `bar` |
| `theater.play`, `theater.pause`, `theater.speed`, `theater.step` | `{}`, `{}`, `{speed}`, `{ticks}` | transport state |
| `theater.seek` | `{tick, source?: "start"}` or `{cancel: true}` | transport state |
| `theater.end` | `{}` | queues the main menu; the film page opens when it loads |
| `theater.keyframes` | `{}` | `{ring: [tick], ring_bytes, ring_cap, off?}`; `off` says why the ring is off for the film |
| `theater.verify` | `{id \| path, from?, to?, ring?: true}` | plays the range unrendered in a second game process and returns `{ticks, digests_checked, arena_hashes_checked, first_divergence}`; `ring` takes and loads an entry every 150 ticks on the way |
| `theater.start_point.capture`, `theater.start_point.load` | a path and a key | a start point of the running game, and its load with `matches` |
| `theater.camera`, `theater.hud`, `theater.screenshot`, `theater.clip.mark`, `theater.clip.export`, `theater.compat`, `theater.share`, `theater.open` | as agent-api.md lists | |

### Live API

[live-api.md](../live/live-api.md#theater).

| Method and path | Token scope | Result |
|---|---|---|
| `POST /v1/films`, `PUT /v1/films/{film}?object=film\|stats&part=<n>` | `films.write` | Upload a film and its stats stream. At most 256 MiB a film and 1 GB an account. |
| `GET /v1/films/{film}` | by visibility | The listing row, with `compat`: the XBE digest, simulation contract and hook digest from the film's stamp. |
| `GET /v1/films/{film}/film` | by visibility | With a range, the stats stream's public ranges. Without one, the film, to a signed-in account. |
| `PUT /v1/films/{film}/visibility`, `DELETE /v1/films/{film}` | `films.write`, the uploader | |

A film of a Live game keeps its game id as its film id.

## Slices

A lane writes the code and its tests; the coordinator builds, tests and
makes the game runs. Slices run in order unless "Starts" says otherwise.
Each slice updates the pages that name what it changes
([theater.md](theater.md), [switches.md](../runtime/switches.md),
[agent-api.md](../runtime/agent-api.md#theater-transport)).

| # | Slice | Starts | Tests |
|---|---|---|---|
| 1a | **The game start in the film.** `film::start::GameStart` (stamp, options block, random words, map identity, settings, start hash, first tick) with its encoding, written as a block of the film file and read by `FilmReader::game_start`. The recorder copies the options when `GAME_ACTIVATE` succeeds and fills the start hash and random words at the game's first update; the map identity, mode and roster come from the observer of that tick, and the block is written just before it. `VARI` is filled from the variant's rules script; `MAPV` is encoded and tested and filled by slice 11. | first | `film`: round trip; every truncation and a fuzzed block are refused; an options block of another size is refused. `theater`: capture over a mapped test guest copies the active options and both random words; a recording of a synthetic session with two games writes one game start for each, at its first update's tick. |
| 1b | **Play from the game start.** The player process takes a film with a game start: a new, empty private data directory (no `DATA` restore, no `--data`), no call tape, the film's options answered to the first map request (`level_boot`), neutral pads on the options' ports, the random words and start hash at the first update, the film's actions applied from the game's first update, digests checked, the film complete after its last tick (`h2 theater play` then leaves; holding the last tick is slice 5's). The player takes pacing and the storage guard of slices 3 and 4: it calls `Session::pace_in_guest` and guards the run's writes (`Vfs::guard_writes`), and never uses the wall-clock pacer. A seek restarts the process and runs forward from the game start. A film without a game start does not open, with that reason. | after 1a | `theater`: over a synthetic machine, a film recorded after menus and a first game plays its second game from the game start to matching digests; a changed start hash stops before the first update; a changed action stops at its tick; no `DATA` item is opened (a manifest that names a missing file plays). `level_boot`: the recorded options are queued byte for byte with the mode set to local. Run: record a game, change a profile setting and save, play the film to its end. |
| 2 | **The whole update.** The recorder stores the update's body beyond the action block as changes against the update before, zero at each chunk's start, and the player writes it back; one record for each application of the update; the tick-stamp check. | after 1b | `columns`: round trip of bodies with a player-set change, events and a save flag; a chunk decodes without the chunk before. `actions`: capture and apply leave the head and stamp untouched; two applications in one tick give two records and replay as two ticks; a stamp that skips ends the film. |
| 3 | **Pacing in the guest.** A hook on `GAME_UPDATE_TICKS` gives the player the frame's tick count: pause, speeds, step, and the unrendered forward run with its budget. | after 1b | `theater::tests`: zero ticks while paused and the digests of a synthetic film unchanged; a step runs one tick; 4× asks for four ticks a frame and reports the count reached; a forward run stops exactly at its target. |
| 4 | **The storage guard.** While a film plays, guest writes go to a private scratch that later reads see, dropped when the film ends (`crates/runtime/vfs/src/host.rs`). | any time | `vfs`: a write under the guard leaves the real file's bytes and time unchanged, a read after it returns the written bytes, dropping the guard removes the scratch; a file created under the guard does not exist after. |
| 5 | **In the running game.** `theater.watch` and Watch queue the film's options through `Call::QueueGame`; `Session` moves between recording and playing at game boundaries; the film's settings are applied and put back; End Film and the film's end return to the film page; a start-route seek queues the options again. Watch starts no process. | after 3 and 4 | `theater::tests`: record a game, watch it, leave, record another, in one synthetic session, and both recordings are whole; a film being watched is not recorded; a start-route seek reaches its tick; the viewer's settings are back after the film. Agent-proto round trips. Run: `theater.watch` on a film from the main menu and from a running game. |
| 6 | **One film for each game.** The recorder opens the film at the game's activation with tick 1 its first update and seals it at the game's end; no session tape, no cut, no fork; `<id>.h2film` is the film file and `<id>.h2stats` the stats container, which holds no machine section. Recovery reads an unsealed film to its last checkpoint. | after 5 | `autorecord`: three games give three films and a session of menus gives none; a hard-killed film opens up to its checkpoint; the library lists a film from its header alone. Stats reader tests pass on a container with no machine section. |
| 7a | **Delete the machine film: Theater and the film crate.** The start manifest and disk tracker (`film/src/manifest.rs`, `manifest/`), tape recovery (`recover.rs`), machine keyframes and their sidecar (`film/src/sidecar.rs`, `theater/sidecar.rs`, `warm.rs`, `warm_transport.rs`, the capture points in `record.rs`, the keyframe restore in `replay.rs`), boot facts and startup (`theater/startup.rs`), the restart loop and disk rebuild in `theater_cmd.rs`, `theater/prepare.rs`, the image match in `compatibility.rs`, and their settings, switches and agent fields. The page pool of `theater/ring.rs` and `keyframe_scope` move to their other user, the bounce search (`sim_bounces/ring.rs`, `sim_bounces::fork_scope`, [bounce-search.md](../runtime/bounce-search.md)). | after 6 | The crates build with those modules gone; `theater`, `film` and `cli` tests pass; the bounce search's fork tests pass on the moved pool; `registration/tests.rs` builds one plan for a recording and a playing session. |
| 7b | **Delete the call tape.** `film/src/tape.rs` and `tape/`, the container's machine section that the tape and the stats reader read (`film/src/machine.rs`), `film::tape::Tap` and every user of it: taped pads, keyboard, bots, lobby and network (`theater/local_pads.rs`, `bot_tape.rs`, `keyboard_glue.rs`, `bot_glue/retail_input.rs`, `coop_live/film.rs`, `firefight/film.rs`, `live-hle/src/network/film.rs`, `film_service.rs`, the taps in `live-hle/src/lib.rs`, `impl LobbyTape for Session`), the tape table of `h2 film stat` (`film_cmd.rs`) and Live's machine-run inspection (`live-server/src/films/inspect.rs`); the film file's machine part (`film::file::machine_mirror`, block kind 1) and Live's check of it (`live-server/src/films/upload/film_file.rs`), which takes a game start block instead. | after 7a | `live-hle`, `live-server`, `boot` and `cli` tests pass with no tap; a network call in a test makes the same guest writes as before. |
| 8 | **The session ring.** The ring of this page (`theater/ring.rs`), the ring route in seek, `theater.keyframes`, and the Forge monitors kept with each entry (`forge_film::ring`): built. Run: in a multiplayer game and a campaign level, watch a film past tick 3,000, jump back to ring entries, and read the digests of the 3,000 ticks after each load and the `restore_ms` of each jump; the engine's after-restore step is wired only if the digests differ without it. Then the timeline's covered stretch from `theater.keyframes`. | the run after 5; the code after it | The run in a dated note. `ring::tests`: one entry for each stretch of 150 ticks and none before the first; the cap thins the older half and keeps the oldest and newest; a load puts back the arena, records and monitors of the latest entry at or before the target; a wrong hash is refused; the switches. `watch::tests`: an entry taken at tick k and loaded at tick m gives the states and digests of a straight run, twice in a row and from the held last tick; no entry at or before the target, or `source: "boot"`, takes the start route, and the ring outlives it; a failed hash and a divergence after a load drop the ring and take the start route; a cancelled ring jump holds the tick it left. `forge_film::tests`: an entry's monitors go back. |
| 9 | **First person from the film.** The recorded action's facing written to each local user's look state on its update. | after 5 | `coop_look` tests over a fake guest: the look state equals the action's facing after each update; a user with no action, an out-of-range angle or another director keeps its look. `theater` `actions` tests: a camera that is not first person writes nothing. Run: a first-person film looks where the recording looked. |
| 10 | **Films that begin mid-game.** The recorder writes a start point when a game's arena comes from a saved checkpoint, when this machine joins a synchronous game in progress and when a recording starts during a game; the player loads it through `start_play::apply`. | after 8's run | `theater`: a film with a start point plays to matching digests on a synthetic machine; one changed byte is refused; a film that flags a start point and holds none is refused. |
| 11 | **Extension records.** A record kind for a feature's outside input, with the Forge session's edits as its first user and `Session::outside_change` ending the film for a change with no record. | after 6 | `film`: round trip and caps. `theater`: a Forge edit recorded at tick k is applied at tick k in playback; an editor write ends the film at the tick before. |
| 12 | **Host films and Live.** A dedicated host and a player's host record the film of this page; `film::upload` sends the film file and the stats stream; share uploads the recorded file unchanged; a participant's Watch fetches the host's film. | after 6 | `live-server`: the upload is stored byte for byte and no tick chunk is inflated; route tests for scope, quota and visibility; the parity suite on both homes. Run: a host film's arena hashes on a local playback. |

Slices 3 and 4 touch files no other slice does and can run beside 1b and
2. Slices 7a and 7b remove more than they add.

## Rules and limits

- A film is the game options and the updates. It holds no disk file, no
  profile, no controller read, no packet, no guest RAM and no keyframe.
- A film starts when its map is activated. Nothing a session did before
  the game is in it.
- Stats is the summary and the film is the simulation. A film holds no
  pose, event or score; a stats stream holds no update or digest.
- A film is played only against a simulation whose contract it matches.
  There is no "try it anyway", and no pose fallback in the game.
- Theater input never reaches the replayed guest. The camera changes
  only the view argument and the private random stream.
- A film never writes the player's profile or saves.
- Only a machine that simulates the whole game records a film.
- A jump while a jump is running replaces its target. The player never
  queues jumps. Reverse play is not offered; rewind is a jump back.
- The ring holds game state only, covers film already passed in this
  session, and is never written to disk.
- A start point is the arena and extension state, at most 1 MiB stored,
  and is refused unless its hash matches.
- A film from before the game start was recorded does not open.
- Clip export needs the user's own ffmpeg for MP4; PNG needs nothing.

## Evidence

- **E**: every tick of every game runs from the simulation update that
  `game_tick` builds; a synchronous client simulates from received
  updates alone, and the update carries the player-set change, the
  per-player entries, the event list and the save flag
  ([live-coop.md](../live/live-coop.md#13-the-synchronous-machinery),
  sections 14.1 and 14.9).
- **Run**: two machines hold a campaign level in step on that path
  (live-coop.md, the status at its head and section 10).
- **E**: every game starts from one options block that holds the variant
  and the players by value
  ([level-boot.md](../runtime/level-boot.md#how-the-game-starts-its-first-map)).
- **Code**: the action block is captured and applied at
  `SIM_UPDATE_APPLY` for every slot in its mask, after remote arrivals
  and phantom actions are resolved (`theater/actions.rs`).
- **Code**: a running game queues new options and loads them at its next
  transition (`game_load.rs`, `agent::sim::calls::Call::QueueGame`,
  [test-maps.md](test-maps.md)).
- **Code**: the arena and the extension records are captured from and
  loaded into a running game (`theater/start_point_state.rs`,
  `start_point_agent.rs`).
- **Code**: the film file holds action columns in chunks with a 16-bit
  digest a tick and reads a tick's actions in any order
  (`crates/live/film/src/file.rs`, `FilmActions`).
- **Code**: a world frame's draws use a private random stream
  (`render_rng.rs`).

## Not verified

- That a game started from the recorded options, in a session with
  another history and another profile, has the arena the start hash
  names. Slice 1b's run answers it. If it does not hold for a kind of
  game, that kind's films begin at a start point.
- That a local game given a recorded update produces the recorded
  digests when the recording was a distributed host or a synchronous
  peer. Slice 12's run answers it for a host.
- Which bytes of the update beyond the action block a local game fills,
  and their size in a film.
- That a player entry written as remote or phantom starts in a local
  game with no session behind it.
- That writing the arena and running the engine's after-restore step
  restores a running game exactly, in multiplayer and in campaign. Slice
  8's run answers it; without it, seeks take the forward and start
  routes.
- That the facing in the action is enough for the first-person view,
  zoom included.
- Which files the game writes during a film, and that the storage guard
  covers a campaign revert.
- That each application of the update advances the tick count by one.
- Every size on this page.
- That a moved camera leaves a film's digests unchanged in the real game.
- How the app bundle receives a URL while a game is running, on each
  host OS.

## Risks

1. **The start does not reproduce.** The options may not be the whole
   start state for some kind of game. The start hash says so before the
   first update, and the start point is the remedy: about 0.26 MB at the
   front of such a film.
2. **No ring.** If a game-state restore is not exact, a jump back costs a
   map load and a run from the start: minutes for a long film. Halo 3's
   campaign films have the same limit.
3. **A host's film played as a local game diverges.** Then a
   distributed host's films are unavailable in the game until the cause
   is found; the per-pool hashes (`sim.hashes`) name the pool that
   parted, and the stats stream still opens on the web.
4. **The simulation contract churns during the sim rewrite.** Every
   breaking phase makes the library unavailable in the game. Accepted:
   the stats stream still opens on the web.
5. **A feature of ours reaches the simulation with no record.** The film
   diverges at that tick and says so. Each such feature gets an extension
   record or ends the film.
6. **A `h2recomp://` link is input from the web.** The parser, the
   server match and the press-to-play rule are the whole defence, and
   each has a test.

## Related

- [film-format.md](film-format.md): the film file, the stats container, recording every game
- [input-film.md](input-film.md): the tick chunk's encoding, sizes and memory, start points
- [theater.md](theater.md): the selector, the controls, the web tier
- [notes/halo3-film.md](notes/halo3-film.md): the Halo 3 reference
- [live-coop.md](../live/live-coop.md): the simulation update and the synchronous path
- [level-boot.md](../runtime/level-boot.md): the options block and how a game starts
- [checkpoints.md](../runtime/checkpoints.md): the engine's game-state save and restore
- [agent-api.md](../runtime/agent-api.md#theater-transport), [live-api.md](../live/live-api.md#theater)
- [web-viewer.md](../live/web-viewer.md): the viewer that draws the stats stream
