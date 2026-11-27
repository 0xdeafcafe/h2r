# Theater design: recording and playback

How a film is recorded and played back. Part of [theater-design.md](theater-design.md).

## Recording

`h2 play` records every game into the film library
([film-format.md](film-format.md#recording-every-game)).

1. **A game starts.** When `GAME_ACTIVATE` returns success for a scenario
   that is not the main menu, with options other than the game in play's,
   the recorder seals the game before it and opens `<id>.h2film` and
   `<id>.h2stats` (`State::open_game`). The game start, with the stamp,
   is written at the first update, which is film tick 1
   (`actions::note_first_update`). A game the observer has not described
   by then is taken again at each next update, and its start is written
   before the first update that has a description (`game_start::Starts::release`);
   one still undescribed after 600 updates keeps no start. A game gone
   before its first update leaves no file.
2. **Each update.** The hook at `SIM_UPDATE_APPLY` hands the update's body
   to the film; the observer after `GAME_TICK` adds the tick's digest and
   the stats records (`record::Recording::post_tick`).
3. **The game ends.** The game-over latch, a map unload or the session's
   exit seals both files (`State::seal_game`). A hard-killed recording
   leaves an unsealed film that reads up to its last checkpointed chunk
   (`film::file::FilmWriter::checkpoint`); the library lists and plays it
   as it stands, and the prune before a later game's film seals it there
   once it has been untouched for an hour (`film::file::recover`,
   `autorecord::prune`).

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
   the caller), its simulation hook digest is this session's
   (`film_play::Play::check_build`; a film of other hooks is refused as
   "recorded by another build"), it was recorded with the feature switches
   this session runs, and the options block names a scenario. One film is
   watched at a time.
2. The film's options are queued as the next game through the agent's
   guest calls (`agent::sim::calls::Call::QueueGame`, the path `game.load`
   takes), the recorded block with its session kind byte set to local (a
   campaign's, 0, stays 0). Once the calls have queued them the film is
   armed (`Session::arm_watch`): its simulation-scope settings are applied
   by value (`settings::Store::run_as`) and the viewer's running values are
   kept, and the viewer leaves any lobby (`lobby_start::clear`): the film's
   game starts from its options alone, and a lobby's Forge mode would move
   a plain Slayer record onto the script index at the activation
   (`rules::record::move_onto_script_index`).
   Until the film's game activates, the transport methods answer
   unavailable ("film ... is loading"), not `{active:false}`
   (`theater/controls.rs`). The playback bar is drawn only once the film's
   game answers the transport (`Session::loaded`, `Theater::film_loaded`);
   until then the menu keeps the pads and nothing is drawn over it. A
   film whose game has not started after 60 s is given up: `theater.end`
   disarms it and queues the main menu (`Session::stall`,
   `Session::end_film`), and the film page opens again over that menu
   saying "The film's game did not start". The main menu activating while
   a film is armed is the film's game failing to load (a failed load
   falls back to the menu, [level-boot.md](../runtime/level-boot.md)): the
   film is disarmed, its settings go back, `theater.status` keeps "The
   film's game did not load" as `watch_failure`, and the film page opens
   again with that line (`Session::watch_boundary`). A transport answer of
   `{active:false}` while the menu waits for a film's game means the same
   to the menu: the bar goes and the page says why.
3. The game tears down, loads the map and activates it as it does for any
   game. At the activation the session moves from recording to playing at
   the game boundary (`Session::game_activated`): the recorder's state is
   parked whole and the player's state takes its place. A game in play is
   left as it is when its player quits to the menu; no film is made of it.
   An activation of another game while the film is armed drops the film and
   puts the settings back.
4. At the first update the player writes the two random words, checks the
   loaded map, variant, difficulty and skulls against the film's
   (`metadata::verify_start`, over the maps the session plays from), and
   compares the arena hash with the film's start hash. Each update after is fed from
   the film until the last record. The player then holds the last tick
   (`Session::holds_in_game`, answered as zero ticks at `GAME_UPDATE_TICKS`)
   with the bar showing the end.
5. **End Film**, `theater.end`, or B at the end queues the main menu's
   options (`game.menu`'s calls). The film's game is gone when the main
   menu activates: the recorder's state returns, the viewer's settings are
   put back and the film page opens (`Theater::reopen_film`) once the main
   menu is the active game and one screen has been on top of it for 20
   menu updates in a row (`theater_ui::link`). Every film the menu's Watch
   started ends on its page, whether the viewer left it, it diverged or
   its game never started (`Session::on_return` gives the film's id and
   the reason, which is the page's status line). The page is the entry
   Watch played (`Theater::play`), so a Live film, which is not in the
   local library, opens its page too. The Theater list opens
   first and the film page over it, so B goes back to the list. The next
   game is recorded from the tick the recording stopped at; none of the
   film's ticks are in any film file. A session that ends while a film
   plays drops the film and seals the recorder's own games.
6. A film that diverges while watched (a start hash that is not
   reproduced, a digest, an update it cannot apply) is left as End Film
   leaves it, from the slice's end (`Session::watch_diverged`): the
   failure is not the guest's, the game goes on, and the reason is kept as
   `theater.status`'s `watch_failure`. The start hash is made of the arena
   and the extension records' digest; the film's `HDET` section holds both
   by page, so the reason names the arena pages or the extension state
   that differ, and the data arrays of the arena that own those pages
   (`lockstep::pools::page_owners`, from the arena's array headers). A
   film of another build (its stamp's `build` is not this session's) says
   so before the hash: the start state is the build's as well as the
   options'. `H2_THEATER_ARENA_DUMP` writes the arena at both starts for a
   byte comparison.

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
| **Ring** | an entry at or before T exists and, for T ahead, it is past the tick shown; the caller did not ask for `source: "boot"` | loads the entry at the next frame's start, then at most 150 ticks |
| **Start** | T is behind and no entry is at or before it, or the caller asked for `source: "boot"` | queues the film's options again (`Session::requeue`): one map load, then every tick to T. Until the queued game activates, the game in play is not the film's and its updates are left alone |

After the restore the player runs forward to T unrendered, checking every
digest on the way, then holds or plays as asked. `theater.state` reports
the route (`forward`, `ring` or `boot` for the start route), the route the caller asked for (`requested`), the ticks left and the tick of the first divergence. A jump
while a jump is running replaces its target. Cancel returns to the tick
the jump left. The film file holds no seek point, as Halo 3's does not.

### The session ring

The ring holds game-state snapshots of the film being watched
(`theater/ring.rs`), in the running game and in a film player process.

- **An entry** is the state at the end of a film tick: the game-state
  arena, zstd at its fastest level, the `SimExt` records and the arena
  hash of both, the tick, the game ticks the film's game had ended by
  then, and the Forge monitor looks and poses read so far
  (`theater::forge_film::ring`). It is a start point without its tag
  (`start_point_state::capture`). It holds no guest RAM outside the arena
  and no device state. The film cursor is the tick: the film file's
  reader (`film::file::FilmActions`) keeps the column decoder's seat rows
  (`ChunkDecoder::seat_rows`) at the start of every chunk it has decoded,
  so it reads any tick the player has passed without the entry holding
  them. About 0.26 MB an entry (**Estimate**: the retail join compresses
  the arena 16 to 1,
  [live-coop.md](../live/live-coop-join.md#141-what-the-retail-engine-already-has)).
- **When.** One entry for each stretch of 150 film ticks the player
  passes, playing or running forward, at the first tick a game tick ends
  on in it (150, 300, ..., or 301 when a game tick applies its update
  twice across 300). The first stretch has none: a jump before tick 150
  takes the start route. A stretch passed again keeps its entry, so one
  pass makes the whole film seekable.
- **Policy.** The ring keeps every entry until their stored bytes pass
  its cap (256 MiB, `H2_THEATER_RING_MIB`), then drops every second entry
  of its older half until it fits; the oldest and the newest stay.
- **Restore.** A ring jump loads its entry at the start of the next frame,
  in the hook that answers the frame's tick count (`theater/pacing.rs`),
  before any of the frame's ticks: the extension records and the arena
  are written back (`start_point_state::load`), the arena hash after the
  load must equal the entry's, the Forge monitors are put back, and the
  film's cursor moves to the entry's tick (`Play::resume_at`), which
  hands the next tick's extension records to their features again. The
  player then runs forward to the target unrendered. No engine step runs
  after the write. Whether the engine's own step after a game-state
  restore (the callbacks its checkpoint revert and its synchronous join
  run, [checkpoints.md](../runtime/checkpoints.md), live-coop.md section
  14.9) is needed is the question of slice 8's run; until it is answered
  the digests decide.
- **Failure.** An entry that does not load or hashes wrong, or whose
  state's digests differ from the film's before the jump's target or the
  next stretch's tick, whichever is later, drops the ring: the jump takes
  the start route, the film is not failed, and the ring stays off for
  that film in this session (`theater.keyframes` names why). A divergence
  after that window is the film's.
- **The ring is not in the film** and goes with the film's state when the
  film ends. A start-route jump keeps it: its entries are the film's, not
  the game's. A verification (`theater.verify`) keeps none.
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
player feeds from the recorded actions: on each update of a film,
`coop_look::follow_film` writes the action's yaw and pitch to the
player-control record's look (`PC_YAW`, `PC_PITCH`) of each local user
whose subject is on foot with the first-person director, and only while the
rig's picture is the recorded view (Default or First person; free, follow
and orbit leave the look alone). The subject and the angles are those
co-op's view prediction reads (`coop_look/view.rs`).

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

- **Loading panel** (`playback_page::loading`, screen `theater.loading`):
  from `theater.watch` until the film's first answer of the transport, a
  centred plate with the film's name, its map, the transport's status
  ("film ... is loading") and a block sweeping a rail. The host draws it
  while `Theater::film_loading`; the pads stay with the menu. The bar and
  the pause menu are drawn only once the film's game is active and the
  transport answers (`Theater::film_loaded`), never over the menu or the
  map load. A load that fails (the game does not start within a minute,
  or `Theater::load_failed`) ends the state, disarms the film
  (`theater.end`) and opens the film page again with the reason as its
  status (`Theater::reopen_film_why`).
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
  film is the recorded file with its seed and its game start's match
  descriptor scrubbed of the recording machine's address, its lease, the
  local map path, the host and every account number
  (`film::file::share_copy`); its start points and tick chunks are the
  recorded bytes.
- **What a film reveals.** The players' names and appearance as they
  were in the game, the map and the variant. No profile, no save, no
  account identifier beyond the gamertags the game showed.
- **A Live game.** A dedicated host uploads the film and the stats stream
  when the game ends (`film::upload`). Live imports the stats through its
  ledger import.
- **Upload.** `POST /v1/films` takes a film and its stats stream from a
  signed-in player under `films.write`, in numbered parts, and gives them
  a film id. Live walks the film file as it walks a host's, inflates only
  its seed, and refuses a seed that names a machine or an account
  (`films/shared/verify.rs`, `film_file::Scan::feed_account`). The film's
  ID is the one its stats stream names.
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
  for private background upload after the game seals them
  (`live-hle` `films/sync.rs`): the job stages the stats container and
  the film's shared copy, sends the stats stream first and the film file
  after, as a host does, and resumes from its last acknowledged part.
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
