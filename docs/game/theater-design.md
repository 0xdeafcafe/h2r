# Theater: the full in-game player

Status: Design. The per-update action and body capture and apply, the film file and its column and tail codec, the game start block the recorder writes for each game, start points, the camera rig, the film selector and the Live film routes are built; a film plays from its game start, in the running game and in a film player process, one film tick for each application of the update (slices 1b, 2, 3, 4 and 5); the snapshot ring is not built, and the code still holds the machine film for films that have no game start ("Slices", 7a and 7b list it).
Code: `crates/extensions/game/src/theater.rs` (`Session`), `crates/extensions/game/src/theater/actions.rs` (the update hook), `crates/extensions/game/src/theater/record.rs` (the recorder's tick), `crates/live/film/src/columns.rs` (the chunk codec), `crates/extensions/game/src/theater/film_play.rs` (play from the game start), `crates/extensions/game/src/theater/watch.rs` (watching in the running game), `crates/extensions/game/src/theater/pacing.rs`, `crates/live/film/src/file.rs` (the film file), `crates/extensions/game/src/theater/start_point_state.rs` (arena state). Tests: `crates/extensions/game/src/theater/tests.rs`, `crates/extensions/game/src/theater/film_play/tests.rs`, `crates/extensions/game/src/theater/watch/tests.rs`, `crates/live/film/src/file/tests.rs`.
Switch: [Theater ring](../runtime/switches.md).

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
- **The start hash** is `lockstep::hash_sim(SimRegion::HALO2)` at entry
  to the first update. Playback compares it before it applies the first
  record and refuses a film whose start it did not reproduce.

### The update stream

`game_tick` builds one simulation update for every tick of every game,
local games included, and `SIM_UPDATE_APPLY` applies it. It is the record
the engine's synchronous protocol sends whole to a client, which then
simulates from nothing else (**E**:
[live-coop.md](../live/live-coop.md#13-the-synchronous-machinery)). Co-op
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
  (`stamp_follows`), and cuts the game's film at the tick before an
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
  applied it: a shared Forge session's edits, Firefight's host log, a
  rules script reload ([film-format.md](film-format.md#the-film-file)). A change with no record (an editor write, an agent
  API write) ends the film at the tick before it
  (`Session::outside_change`).

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

## Recording

`h2 play` records every game into the film library
([film-format.md](film-format.md#recording-every-game)).

1. **A game starts.** When `GAME_ACTIVATE` returns success for a scenario
   that is not the main menu, the recorder opens `<id>.h2film` and
   `<id>.h2stats` and writes the stamp and the game start.
2. **Each update.** The hook at `SIM_UPDATE_APPLY` hands the update's body
   to the film; the observer after `GAME_TICK` adds the tick's digest and
   the stats records (`record::Recording::post_tick`).
3. **The game ends.** The game-over latch, a map unload or the session's
   exit seals both files. A hard-killed recording leaves an unsealed film
   that reads up to its last checkpointed chunk
   (`film::file::FilmWriter::checkpoint`).

A session that plays no game writes nothing. Nothing is recorded outside
a game, so a film never depends on what the session did before it. A film
being watched is not recorded.

## Playback

### Starting a film in the running game

Watch does what the game's own menus do to start a game. The session that
records the games a player plays also plays films
(`theater/watch.rs`); a film player process is for verification and clip
export only.

1. `theater.watch` and the Theater menu's Watch plan the film
   (`Session::plan_watch`): the film has a game start, its stamp is this
   build's XBE and simulation contract ("Film compatibility", checked by
   the caller), it was recorded with the feature switches this session
   runs, and the options block names a scenario. One film is watched at a
   time.
2. The film's options are queued as the next game through the agent's
   guest calls (`agent::sim::calls::Call::QueueGame`, the path `game.load`
   takes), the recorded block with its session kind byte set to local (a
   campaign's, 0, stays 0). Once the calls have queued them the film is
   armed (`Session::arm_watch`): its simulation-scope settings are applied
   by value (`settings::Store::run_as`) and the viewer's running values are
   kept.
3. The game tears down, loads the map and activates it as it does for any
   game. At the activation the session moves from recording to playing at
   the game boundary (`Session::game_activated`): the recorder's state is
   parked whole and the player's state takes its place. A game in play is
   left as it is when its player quits to the menu; no film is made of it.
   An activation of another game while the film is armed drops the film and
   puts the settings back.
4. At the first update the player writes the two random words and compares
   the arena hash with the film's start hash. Each update after is fed from
   the film until the last record. The player then holds the last tick
   (`Session::holds_in_game`, answered as zero ticks at `GAME_UPDATE_TICKS`)
   with the bar showing the end.
5. **End Film**, `theater.end`, or B at the end queues the main menu's
   options (`game.menu`'s calls). The film's game is gone when the main
   menu activates: the recorder's state returns, the viewer's settings are
   put back and the film page opens (`Theater::reopen_film`). The next
   game is recorded from the tick the recording stopped at; none of the
   film's ticks are in any film file. A session that ends while a film
   plays drops the film and seals the recorder's own games.

No process is started and nothing is booted to watch a film or to seek in
one. `h2 theater play FILE`, `h2 theater verify` and the clip export
worker are game processes the user or the watching game starts for one
film: they boot the game once and answer its first map request with the
film's options, as `--level` does
(`level_boot.rs`), then play as above.

### What the guest is given while a film plays

| Input to the guest | During a film |
|---|---|
| Simulation updates | the film's, written over the engine's at `SIM_UPDATE_APPLY` |
| Controllers | a neutral pad on each port a player entry in the options names, and no other. Theater's own controls are read by the host and never reach the guest (`theater/controls.rs`) |
| Local look | the recorded action's facing, written to each local user's look state on its update, so a first-person view follows the film ("Cameras and the HUD") |
| Storage | reads see the mounted disc and maps. Writes (a campaign checkpoint, the profile at a game's end) go to a private scratch the film's reverts read back; it is dropped when the film ends. The player's profile and saves are never written |
| Network and Live | the game is a local game and opens no socket. The Live session of the running game stays signed in and shows "In Theater" |
| Ticks | the player decides each frame's tick count ("Pause, speed and step") |
| Features of ours | the film's extension records, applied on their ticks; a feature that would decide from live input stands down |

### Pause, speed and step

The engine asks once a frame how many ticks to run and runs them
(`GAME_UPDATE_TICKS`; **E**:
[high-framerate.md](high-framerate.md)). The player answers that
question while a film plays: none while paused, one for a step, the count
the speed and the frame's time give at 0.25× to 4×, and as many as fit in
a frame's budget while it runs forward to a jump's target. Nothing in the
game state is written to pause or change speed, so the digests do not
move. The renderer keeps drawing while the tick count is zero, so the
camera moves and screenshots work in a pause. The hook is
`theater/pacing.rs`; the answer comes from `FrameClock::frame_ticks` in
`theater/transport.rs`, and the hook writes only the count and the frame's
elapsed seconds.

### Seek

A request for tick T takes the cheapest route:

| Route | When | Cost |
|---|---|---|
| **Forward** | T is ahead of the tick shown and no ring entry is nearer | simulates on from here, unrendered (`State::seek_in_game`) |
| **Ring** | an entry at or before T exists | loads the entry in the running game, then at most 150 ticks |
| **Start** | T is behind and no entry is at or before it, or the caller asked for `source: "boot"` | queues the film's options again (`Session::requeue`): one map load, then every tick to T. Until the queued game activates, the game in play is not the film's and its updates are left alone |

After the restore the player runs forward to T unrendered, checking every
digest on the way, then holds or plays as asked. `theater.state` reports
the route (`forward`, `ring` or `boot` for the start route), the ticks left and the tick of the first divergence. A jump
while a jump is running replaces its target. Cancel returns to the tick
the jump left. The film file holds no seek point, as Halo 3's does not.

### The session ring

The ring holds game-state snapshots of the film being watched.

- **An entry** is the game-state arena, compressed, the extension
  records, the tick, and the film cursor: the next chunk and the column
  decoder's seat rows (`film::columns::ChunkDecoder::seat_rows`). It is
  a start point without its tag (`start_point_state::capture`). It holds
  no guest RAM outside the arena and no device state. About 0.26 MB an
  entry (**Estimate**: the retail join compresses the arena 16 to 1,
  [live-coop.md](../live/live-coop.md#141-what-the-retail-engine-already-has)).
- **When.** One entry every 150 ticks the player passes, playing or
  running forward, so one pass makes the whole film seekable.
- **Policy.** The ring keeps every entry until it reaches its byte cap
  (256 MiB, the `theater.ring_mib` setting), then drops every second
  entry of its older half.
- **Restore.** At a tick boundary the player writes the arena back
  (`start_point_state::load`), runs the engine's own step after a
  game-state restore, the one its checkpoint revert and its synchronous
  join run
  ([checkpoints.md](../runtime/checkpoints.md), live-coop.md section
  14.9), loads the extension records and resets the film cursor. The
  arena hash after the load must equal the entry's, and the next digests
  must match.
- **Failure.** An entry that fails either check is dropped with the rest
  of the ring, the jump takes the start route, and the ring stays off for
  that film in this session.
- **The ring is not in the film** and is dropped when the film ends.
  `H2_THEATER_RING=0` turns it off.

### Cameras and the HUD

One camera rig serves every film (`theater::ReplayCamera`): mode, subject
seat, pose, orbit radius and field of view. It survives seeks and is
reported in `theater.state`.

| Film | Picture | Free, follow, orbit | First person | HUD |
|---|---|---|---|---|
| Recorded on a machine with a player | the guest's renderer | the rig replaces the renderer's view argument and can set its lens; modified draws use the private random stream (`camera.rs`, `render_rng::Seeds`) | a local seat's own view; any other player gets follow | the game's HUD for the local seats, hidden by `theater.hud {game: false}` |
| Recorded by a dedicated host (no local player) | the independent view (`host_view.rs`): BSP, lightmaps, posed opaque models | the same rig | the unit's eye; no weapon model | none |

The render steps the shared random word, so every world frame of a
recording and of a playback draws inside the private stream
(`render_rng.rs`); what a view draws never chooses the simulation's next
random number. A local seat's first-person view reads the look state the
player feeds from the recorded actions, the same fields co-op's view
prediction reads (`coop_look/view.rs`).

A host film's picture gains effects, sky, transparents and a HUD when the
renderer owns the frame and can draw from a presentation snapshot
([framescene.md](../graphics/framescene.md)).

### Screenshots and clips

- **Screenshot.** The shell saves the published game frame before
  Theater's plates are drawn (`crates/apps/app/src/theater_overlay.rs`,
  `screenshot`). A scale above 1 renders one frame at a higher internal
  resolution while the tick count is held at zero.
- **Clip export.** `theater.clip.export` starts a windowless game process
  on the same film. It runs forward to the in mark and plays to the out
  mark unpaced with a frame sink that receives every presented frame and
  the mixer's tapped audio. Frames go to a PNG sequence, or to the user's
  own ffmpeg for an MP4, the way `h2 movies` runs it
  (`crates/apps/cli/src/movies_cmd.rs`). The export carries the rig.
- A clip on Live is a tick range of its film; a video file is a local
  export.

### The Theater bar and menus

The bar and the pause menu are pages of the UI builder
([ui-builder.md](ui-builder.md)), drawn in the retail skin by the same
kit as the film selector. The host captures page input before it reaches
the guest.

- **Bar** (bottom of the screen, hidden with Back or H): play state,
  speed asked and reached, time and length, a timeline with kill markers
  from the stats file, clip marks and the stretch the ring covers, the
  camera mode and the subject's name and emblem colour. During a jump the
  timeline shows the target and fills towards it.
- **Pause menu** (Start or Escape): Resume, Restart Film, Change Player,
  Camera, Take Screenshot, Mark Clip, Export Clip, Save Film, Share, End
  Film. Lists open as a dialog over the menu.
- **Controls** are the table in
  [theater.md](theater.md#in-game-theater), with D-pad Down or `.` to
  step one tick while paused, and B or Escape during a jump to cancel it.

### Film compatibility

| State | Condition | Playback |
|---|---|---|
| **Playable** | the XBE digest, the simulation contract and the hook digest equal this build's; the map with the film's digest is installed | from the game start, every digest checked |
| **Unavailable** | a different contract, hook digest or XBE, or the map is missing | the reason: the film predates a change to the simulation, or which map to install |

`theater.compat` and the film page report the state before Watch
(`theater/compatibility.rs`). A film is never played against a simulation
whose contract differs, and there is no pose fallback in the game; the
game's stats stream still opens on the web. The contract covers the
simulation's results only. A film depends on no scheduler budget, device
answer or host call order, so a build that changes those plays it.

### Kinds of game

| Game | Who records | Start | Notes |
|---|---|---|---|
| Offline multiplayer, split screen, bots | this machine | options | bots are phantom seats; their actions are in the update |
| Campaign, a new mission | this machine | options | checkpoints and reverts are replayed |
| Campaign, resumed from a saved checkpoint | this machine | options and a start point | |
| Co-op over Live (synchronous) | every machine: each applies every update | options; a start point for a player who joined in progress | each machine's film is the same simulation |
| System link or Live game hosted by a player | the host's machine | options | a client's machine records no film |
| Live game on a dedicated host | the host | options | the host uploads it; every participant's library lists it |
| Firefight, Forge, scripted game types | whichever machine simulates | options, with the script or map variant by value | Forge edits are extension records |

A client of a distributed game watches the host's film, fetched from its
account library after the game ("Account film library"), with its own
seat selected. It shows the authoritative game, not what the client's
machine predicted. The host can also forward the film to its clients as
it is made ([input-film.md](input-film.md#the-action-side-stream)), which
gives a client the film without Live.

### Sharing through Live

- **The same file.** Live stores a film as the bytes it was sent and
  serves it by byte range; it reads the header and the game start and
  inflates no tick chunk
  (`crates/live/live-server/src/films/upload/film_file.rs`). A shared
  film is the recorded file: there is no disk payload to strip and no
  share copy to make.
- **What a film reveals.** The players' names and appearance as they
  were in the game, the map and the variant. No profile, no save, no
  account identifier beyond the gamertags the game showed.
- **A Live game.** A dedicated host uploads the film and the stats stream
  when the game ends (`film::upload`). Live imports the stats through its
  ledger import.
- **Upload.** `POST /v1/films` takes a film and its stats stream from a
  signed-in player under `films.write`, in numbered parts, and gives them
  a film id.
- **Download.** The game fetches a film into the library once, by range
  requests it can resume (`film::share`), checks its digest and plays the
  file from disk.
- **Visibility.** `private`, `link` or `public`. Public range reads serve
  the stats stream. The film goes only to signed-in accounts the
  visibility admits.
- **Both homes.** The routes sit behind the same storage traits on the
  native server and on Workers.

### Account film library

Every signed-in Live account has a film library, projected by both Live
homes from the same signed journal.

- **Ownership.** A game appears in every participant's library when Live
  accepts the host's film. A machine queues its own offline and LAN films
  for private background upload after the game seals them; the upload
  resumes from its last acknowledged part.
- **Playback.** Watch uses the local file when there is one, and
  otherwise reads the film in ranges into the local library.
- **Sync.** Live owns membership. Pins and deletes are signed changes in
  server commit order. Each machine keeps its queue in
  `<data>/films-sync/<server>/<account>`.
- **Quota.** 1 GB an account. A host film uses account space only while
  the player pins it. The oldest unpinned uploads go first.

### The link between the web and the game

A share link is the web address of the film or clip:
`/theater/<film>` or `/theater/<film>/clip/<clip>`
(`web/src/routes.ts`, `theaterHref`), with optional `t=<tick>`,
`seat=<n>` and `camera=<mode>`. The page's **Open in Halo 2** button
opens `h2recomp://theater/<film>?clip=<clip>&t=<tick>&seat=<n>&camera=<mode>&server=<origin>`.
`theater.open` lands on the film page with Watch focused and plays
nothing.

## Numbers

| Quantity | Value | Source |
|---|---|---|
| Game options block | one block, a few kilobytes compressed | **E**: [live-gamestate.md](../live/live-gamestate.md) |
| Game-state arena | 4,186,112 bytes | **Code**: `crates/platform/xapi/tests/checkpoints.rs` |
| Tick rate | 30 Hz | [high-framerate.md](high-framerate.md) |
| Film bytes | about 0.12 MB (one player) to 0.5 MB (16) for 10 minutes | **Estimate**: [input-film.md](input-film.md#size-step-by-step) |
| Ring entry | about 0.26 MB; one every 150 ticks; 120 entries, 31 MB, for a 10-minute film | **Estimate** |
| Ring cap | 256 MiB | **Design** |
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
| `theater.state` | `{}` | `tick`, `length`, `paused`, `speed`, `speed_reached`, `seek` (`{target, from_tick, source: "forward" \| "ring" \| "start", remaining_ticks, restore_ms}` or null), `verified_through`, `divergence` (`{tick, kind}` or null), `camera_pose`, `hud`, `bar` |
| `theater.play`, `theater.pause`, `theater.speed`, `theater.step` | `{}`, `{}`, `{speed}`, `{ticks}` | transport state |
| `theater.seek` | `{tick, source?: "start"}` or `{cancel: true}` | transport state |
| `theater.end` | `{}` | queues the main menu; the film page opens when it loads |
| `theater.keyframes` | `{}` | `{ring: [tick], ring_bytes, ring_cap}` |
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
| 2 | **The whole update.** The recorder stores the update's body beyond the action block as changes against the update before, zero at each chunk's start, and the player writes it back; one record for each application of the update; the tick-stamp check. | after 1b | `columns`: round trip of bodies with a player-set change, events and a save flag; a chunk decodes without the chunk before. `actions`: capture and apply leave the head and stamp untouched; two applications in one tick give two records and replay as two ticks; a stamp that skips cuts the film. |
| 3 | **Pacing in the guest.** A hook on `GAME_UPDATE_TICKS` gives the player the frame's tick count: pause, speeds, step, and the unrendered forward run with its budget. | after 1b | `theater::tests`: zero ticks while paused and the digests of a synthetic film unchanged; a step runs one tick; 4× asks for four ticks a frame and reports the count reached; a forward run stops exactly at its target. |
| 4 | **The storage guard.** While a film plays, guest writes go to a private scratch that later reads see, dropped when the film ends (`crates/runtime/vfs/src/host.rs`). | any time | `vfs`: a write under the guard leaves the real file's bytes and time unchanged, a read after it returns the written bytes, dropping the guard removes the scratch; a file created under the guard does not exist after. |
| 5 | **In the running game.** `theater.watch` and Watch queue the film's options through `Call::QueueGame`; `Session` moves between recording and playing at game boundaries; the film's settings are applied and put back; End Film and the film's end return to the film page; a start-route seek queues the options again. Watch starts no process. | after 3 and 4 | `theater::tests`: record a game, watch it, leave, record another, in one synthetic session, and both recordings are whole; a film being watched is not recorded; a start-route seek reaches its tick; the viewer's settings are back after the film. Agent-proto round trips. Run: `theater.watch` on a film from the main menu and from a running game. |
| 6 | **One film for each game.** The recorder opens the film at the game's activation with tick 1 its first update and seals it at the game's end; no session tape, no cut, no fork; `<id>.h2film` is the film file and `<id>.h2stats` the stats container, which holds no machine section. Recovery reads an unsealed film to its last checkpoint. | after 5 | `autorecord`: three games give three films and a session of menus gives none; a hard-killed film opens up to its checkpoint; the library lists a film from its header alone. Stats reader tests pass on a container with no machine section. |
| 7a | **Delete the machine film: Theater and the film crate.** The start manifest and disk tracker (`film/src/manifest.rs`, `manifest/`), machine envelopes and tape recovery (`machine.rs`, `recover.rs`), machine keyframes and their sidecar (`film/src/sidecar.rs`, `theater/sidecar.rs`, `warm.rs`, `warm_transport.rs`, the capture points in `record.rs`, the keyframe restore in `replay.rs`), boot facts and startup (`theater/startup.rs`), the restart loop and disk rebuild in `theater_cmd.rs`, `theater/prepare.rs`, the image match in `compatibility.rs`, and their settings, switches and agent fields. The page pool of `theater/ring.rs` and `keyframe_scope` move to their other user, the bounce search ([bounce-search.md](../runtime/bounce-search.md)). | after 6 | The crates build with those modules gone; `theater`, `film` and `cli` tests pass; the bounce search's fork tests pass on the moved pool; `registration/tests.rs` builds one plan for a recording and a playing session. |
| 7b | **Delete the call tape.** `film/src/tape.rs` and `tape/`, `film::tape::Tap` and every user of it: taped pads, keyboard, bots, lobby and network (`theater/local_pads.rs`, `bot_tape.rs`, `keyboard_glue.rs`, `bot_glue/retail_input.rs`, `coop_live/film.rs`, `firefight/film.rs`, `live-hle/src/network/film.rs`, `film_service.rs`, the taps in `live-hle/src/lib.rs`, `impl LobbyTape for Session`), the tape table of `h2 film stat` (`film_cmd.rs`) and Live's machine-run inspection (`live-server/src/films/inspect.rs`). | after 7a | `live-hle`, `live-server`, `boot` and `cli` tests pass with no tap; a network call in a test makes the same guest writes as before. |
| 8 | **The session ring.** Run first: `theater.start_point.capture` and `load` in a multiplayer game and a campaign level, then 3,000 ticks of digests, with and without the engine's after-restore step. Then the ring of this page, the ring route in seek and the timeline's covered stretch. | the run after 5; the code after it | The run in a dated note. `theater::tests`: an entry taken at tick k and loaded at tick m gives the digests of a straight run, twice in a row; the cap thins the older half; a failed hash drops the ring and takes the start route. |
| 9 | **First person from the film.** The recorded action's facing written to each local user's look state on its update. | after 5 | `coop_look` and `theater` tests over `FlatMemory`: the look state equals the action's facing after each update; a free camera leaves it alone. Run: a first-person film looks where the recording looked. |
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
