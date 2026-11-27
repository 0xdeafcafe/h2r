# The editor's agent API: pairing and previews

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

Pairing with a running game: playtest, live tag editing, the mirror, viewport layers, volumes, collision, navigation, renders, and content previews in the paired game. The index is [agent-api.md](agent-api.md).

  is full and the map editor can't grow it safely (the same
  `GROWTH_REFUSAL` the window shows; `data` has the count and capacity);
  `.delete` where something refers to entries by index. A new entry goes
  to `position`, else the camera focus, in one undo step.
- **Pairing.** The editor pairs with a running game through the game
  link in the bar. [Editor guide](editor-guide.md#play-from-the-editor)
  `pair.list` looks at the games now: the sockets in the agents directory
  where `h2 play` listens by default (`agent_proto::discover`), plus the
  default and `$H2_AGENT` sockets and Play's own. Each entry has its
  `socket`, `pid`, `map`, `phase`, `paused`, `tick`, whether its map is the
  open one (`matches`: `same`, `different`, `unknown`) and whether the
  editor is `paired` with it; a socket nothing answers on is removed from
  the agents directory. It works headless. `pair.status`, `pair.connect`
  and `pair.disconnect` act on the editor's pairing (`api/pair.rs`, through
  `play::pair::Bridge` on the app). The window runs it each frame and
  `h2-editor --serve` between requests (`Api::serve_once`); an `Api` with
  neither answers REFUSED. `pair.connect` answers before the editor has
  paired: poll `pair.status` for `state: "connected"`. `pair.connect` with no `socket`
  takes the one game running, and is refused with none or several.
  `pair.auto {enabled}` reads or sets auto-pair, on by default.
  The `link` object in `pair.status` contains `state`, the pill's
  `text` and its `why` tooltip (string or null). Its states are
  `no_game`, `found`, `connecting`, `reconnecting`, `viewing`, `editing`, `blocked`,
  `other_map` and `lost`. The top-level `state` continues to report the
  socket connection. When a game omits an optional method, `link.unavailable`
  lists the affected features and methods. `link.issues` is an array of
  `{kind,text,target}` rows. The kinds are `waiting`, `reload`,
  `refused` and `mismatch`. Pass a row's `target` directly to `nav.go`
  to inspect the field, placement, tag or Pending tab. A refused player
  move has a target that opens Pending and stays active for up to four seconds.
  A successful move or a change of scope clears the issue.
  A clear link reports
  an empty array. The pill shows the largest count. During reconnect, `link.elapsed_ms` counts from the drop.
- **Playtest.** `pair.playtest {}` queues F5 and reports the step for the
  window's published state: `launch`, `pair`, `resume`, `reload` or
  `ask`. A reload or launch using the saved player place includes
  `place: {pos, yaw, pitch}` (angles in degrees). An unavailable reload
  reports the manual-load instruction in `why`. An `ask` includes its
  integer `prompt` ID. Echo that ID with `confirm` to answer the same
  prompt. Closing it, changing its operation or editing the document
  expires the ID. A bare `confirm` answers the prompt open at the time
  of the call. For `ask: "send"` choose `"send"` or `"cancel"`. For
  `ask: "load"` choose `"load"`, `"open"` (open the game's map here)
  or `"cancel"`. An open or cancel reply carries the choice in `answer`.
  After automatic pairing, poll `pair.status.ask` and `pair.status.prompt`
  for the follow-up prompt. A stale offer or mismatched map identity
  returns `REFUSED` with the reason.
  `pair.settings` reads or sets `pause_while_editing` (default false)
  and `spawn` (`auto`, `level`, `resume`, `camera` or `selection`).
- **Game controls.** `pair.play` queues the same action as F5. `pair.stop` stops
  the editor's own game. `pair.control` accepts `pause`, `resume` or
  `step` as `action`. Step requires a paused game and accepts `ticks`
  from 1 to 600 (default 1). `pair.edit {enabled}` sets the Edit switch
  on the pairing. A game that is already running pairs in View mode.
  Play starts its own game in Edit mode. The four calls need the window
  or `--serve`, queue a request on the pairing and return `{"requested":true}`. Read
  `pair.status` for the result in a later frame. A call that needs a
  game, an open map or a pause returns `REFUSED` when that requirement
  is missing. The pill's **Open** action uses the normal map selection
  path (`doc.open {map}` for a map in the source).
- **Live tag editing.** Pairing starts in View. Play starts the editor's own
  copy in Edit. `pair.edit` returns the requested `mode` or `REFUSED` with
  the reason. The editor checks the loaded file checksum, epoch and session
  from the game's `edit.map`. In Edit, tag fields and placement fields travel
  together in one `edit.batch`, each with its expected `was` value. Tags use
  the `at` form with hex bytes. The optional `known_epoch` on `edit.map`
  refreshes the loaded datum table used to check reference targets. Undo and redo use the same check.
  `mirror.pending` returns `pending` rows with `doc,path,class,effect,text,why` and an optional
  `tag` and `label`: `class` is the change's fan-out class (`field`, `raw`, `shape` or
  `host`, `app::live::Class`, [live-editors.md](live-editors.md)): `raw` for `geometry`,
  `bitmaps` and `collision` rows, `shape` for a block layout or a new map string, `host`
  for a `host` row (a Host change the game has no reload for, or one an attached game
  in View waits to take, with `host` naming its source), else `field`;
  `effect` is when the change takes effect (`now`, `next_use`,
  `next_spawn`, `reload` or `not_known`, `play::tag_sync::Effect`) and `text` its sentence,
  the game’s existing `journal` from `edit.changes`, and `leaving`.
  A placement block with a different layout revision from the loaded copy
  goes to a game that offers `world.placement.edit` as one transaction in
  Edit; its row is a `now` row while the transaction is out and leaves on
  the receipt. Otherwise, or when the game refuses or its answer is lost,
  the block appears as one `Reload` row with the reason (`refused`, and
  `unknown` for a lost answer), and its fields stay out of the batch until
  a transaction, undo, redo, or loading a Play copy brings the revisions
  into agreement. An entry the game wrote without making its object is a
  `Reload` row of its own (`path` `block[index]`). Rows of this path carry
  `placement_edit: true`.
  A structure's level collision edit is one row with `doc` `collision`,
  `path` `structure_collision:<tag>:0` and `rebuild` (whether the commit
  changes indices). In Edit it goes as `edit.collision.stage`, then
  `edit.collision.commit` (realtime-edits.md, "The editor"); a refusal
  makes it a `reload` row with the game's reason, and a `MISMATCH` marks
  it `conflict` with `resolvable:false` until the documents change or
  the user sends again.
  `mirror.apply {what:"send"}` enters Edit. On MISMATCH, `take` and
  `overwrite` require the row’s numeric `at`. `take` reads the field at the
  current game boundary before applying the bytes to the document and baseline.
  A failed read or a relocated field leaves the conflict pending. A row with
  `resolvable:false` opens Pending and disables both resolutions. Take and
  Overwrite require the tag's field path to resolve to the refused address. On rejoin, `adopt` takes
  the game's current bytes as the baseline and preserves the document.
  `revert` uses the game's reported restored and left-alone bytes as the
  baseline for the next send. A System Link host
  confirms the peer count using `confirm` with `peers`, including when
  another map is loaded. Confirmation applies to the paired host, map epoch
  and peer count. Field writes still require the open map. `reload` writes
  a play copy. In Edit, `respawn` applies acknowledged creation-time
  placement edits through `world.object.respawn`. Moving, deleting and
  undoing placements use the same document actions as the window. The
  game link calls the advertised object methods with the current host
  confirmation. Object rows in `mirror.pending` explain changes that
  need a reload or cannot reach a live object.
  `pair.disconnect {leave:"keep"|"restore"}` makes the leave choice over the
  API. During restore, `mirror.pending.leaving` stays true while the editor
  waits for an outstanding batch, reverts the journal, and disconnects on
  acknowledgement. Omitting `leave` opens the prompt in the window. Both `edit.set` and
  `edit.revert` enforce the same game-side session check as `edit.batch`.
- **Game picture.** The Game panel shows the paired game's own frames,
  read from the game's `render.picture` ring ([game-view.md](game-view.md),
  `play/picture.rs`). `game.status` answers `{mode, camera, own, attached,
  picture}`: `mode` is `own` or `attached` (which game the panel follows,
  `game.mode`) and `camera` is `player` (`game.camera` is not built); `own`
  is `{state: "none"}`, or `{state: "starting" | "running", map, socket,
  editor}` for the game this editor started (`running` once its link is up;
  `editor` is whether it was started with `--editor`); `attached` repeats `pair.status`'s socket, map, phase
  and tick with the link plate's state and text, and is null before the
  window has run its game link; `picture` is null with no pairing. Its
  `state` is `connecting`, `open`, `stalled` (no new frame for two
  seconds; `age_ms` says how long), `closed` (the game closed the picture)
  or `failed` (`why`: a game over `tcp://`, whose JPEG stream Studio does
  not decode, a game with no frame source, a lost connection). It also has
  `size` of the newest frame, `asked_fps`, the frames taken, `dropped`
  (frames the game published that no poll took), `fps` over the last
  second, `age_ms`, the frame's game `tick`, `tick_lag` (the game's tick at
  the link's last poll minus the frame's) and `events` (the `picture`
  events the game sent). Reading it takes the newest frame, so the numbers
  are those of a reader that polls once per call.
  `game.snapshot {out?}` takes the newest frame and writes it as a PNG at
  the size the game published (default under the output directory,
  `game/<map>.png`), answering `{path, width, height, frame, tick}`. It is
  `REFUSED` with no picture, with no frame yet, after a failed or closed
  picture, and for a path inside a Git checkout. The picture is opened when
  the pairing learns the game offers `render.picture.open`, and again when
  the game closed it; it ends with the pairing.
  `game.start` leaves a request for the window (or `h2-editor --serve`) to
  start the editor's own game on the open document, as `h2 play --editor`,
  and answers `{requested: true}`; it replaces a game this editor started
  before, and sets the panel to `own` when it starts. It is `REFUSED` with
  no game link yet, with no map open, or while the link is connecting or a
  playtest runs. `game.stop` asks the same for the end of that game
  (`REFUSED` when the editor started none). `game.mode {mode}` is `own` or
  `attached`; `own` is `REFUSED` while there is no own game. The window
  follows it with the mirror and the picture: `own` pairs the game this
  editor started, `attached` pairs the game last paired that it did not
  start (one link, so the other is left). F5 and the Play button go
  through the same start as `game.start`, and set the panel to `own`.
- **Live mirror.** The paired editor reads the game's `objects`,
  `players` and `camera` stream. `mirror.status.stream.refused.camera`
  says why a game has no camera layer. `mirror.status` reports the stream state, layers,
  frame rate, presentation delay, dropped and skipped counts, and the
  store's latest, earliest retained and shown ticks. `mirror.objects`
  returns `{tick, objects, truncated}` for the viewport sample.
  `at_tick` selects an exact game sample in the interpolation window
  and answers `REFUSED` when it has expired. Each object has its salted
  `id`, tag group `kind`, definition `tag`, `pos`, basis, scale,
  velocity, flags, parent, vitality, placement and node count. Positions
  of attached objects are relative to their parents. `near` takes
  `{point: [x,y,z], radius}` in world space. Bind-pose objects have no
  streamed nodes. Pairing and presentation are described in [live-mirror.md](live-mirror.md).
- **Live selection.** `mirror.pick {"from":"aim"}` uses the local player's
  aim and skips that player's unit and held weapon. A viewport click in
  Player's eyes skips the viewed player's unit and held weapon. Supply
  `{"ray":{"origin":[x,y,z],"dir":[dx,dy,dz]}}` for an unrestricted world ray.
  The first live object in front of the level returns
  `{object, placement, tag, point, distance}`. Handles are hex strings,
  `placement` is a linked `{block,index}` or null, and a miss returns null.
  The link uses the object's kind and placement index in the loaded block
  layout. If a structural edit changes that layout, the object stays
  unlinked until undo or a reload restores matching slots. For a preview, the
  current map’s respawn receipt links its full salted ID to its document
  placement, including after a swap-removal changes the row index.
  Picking over the socket leaves selection alone. Clicking in the viewport
  selects the linked placement or the live object and its available definition.
  `mirror.aim_select {enabled?}` returns `{enabled,candidate}`. The
  candidate has the same shape as a pick. Holding the displayed aim on one
  object for 0.6 seconds selects it. A stream cut restarts the dwell timer.
  In View and Edit the paired window renews its selection and candidate
  marks once a second on the game's display. Unpairing clears the link
  plate and marks. Pairing again to the same endpoint waits for the old
  connection's clear before sending either lease. A dropped connection
  lets the three-second leases expire. `pair.status.link.unavailable` reports
  missing presentation methods and the host's display refusal. While the
  game waits for a genuine frame, a `drawn: false` reply keeps the marks
  pending without a warning. Individual refused marks still report their
  indices. A failed clear retries at the renewal interval. The game plate
  shows the largest link issue count, or `Aim select` when aim selection
  is on and the count is zero.
- **Shared camera.** `mirror.camera {mode?,target?,view?}` returns
  `{mode,target}`, with `view` in game mode.
  `free` releases the editor viewport, `follow` keeps the eye's orbit offset
  as the local player moves, `eyes` looks along the player's aim from
  eye height, and `game` takes a game view's camera from the `camera`
  layer exactly (eye, basis with roll, vertical field of view, near and
  far; the editor viewport's aspect). `view` names one as
  `<kind>:<player>:<view>` (`game:0:0`, `first_person:0:0`); without it the
  first `game` view is taken. A stream without the layer, a view the game
  did not draw or a `target` in game mode is refused. Leaving game mode
  keeps the eye and restores the editor's field of view. Every mode but
  `game` draws each game view's frustum. An explicit `player.<index>` keeps the camera on that local
  player across samples. While its unit is missing, the viewport holds its
  pose and resumes following when the unit returns. A map change or unpair
  releases the camera. The free camera reports null. The viewport toolbar
  has the same modes. `mirror.player {to,facing?}` queues a move in Edit
  and returns `{"requested":true}`. Give a world position, `"camera"` for
  the ground under the editor camera's focus, or `"cursor"` for the level under the
  viewport pointer. T uses the cursor. Shift+T or `facing:true` also sends
  the camera's yaw and pitch. A missing destination, current player sample,
  permission or game method returns `REFUSED`, as does a pending reload or
  a playtest transition. The control worker checks the latest exact sample
  for the expected epoch and salted unit immediately before the game call.
- **Mirror controls.** `mirror.control {op, ticks?}` pauses, steps or
  resumes the paired game as `pair.control` does (`op` is `pause`,
  `resume` or `step`; `ticks` 1 to 600) and returns
  `{requested, tick}`, `tick` being the store's latest game tick before
  the request. Each step sends one frame per tick, so after a step of `n`
  ticks `mirror.status.store.tick` is `tick + n`.
- **Viewport layers.** `view.layers {}` returns `{layers, xray}`. Each layer
  has a stable `name` for calls, a display `label`, a `group`, the visibility
  switch `on`, a `count` and its legend `color` (RGB, each channel 0 to 1).
  Counts include hidden entries. For example,
  `view.layers {"preset":"everything","set":{"fog":false},"xray":true}`
  shows every layer except fog and draws occluded overlays at quarter
  strength. Presets are `art`, `gameplay`, `collision`, `navigation`,
  `playtest` and `everything`.
  Individual switches apply after the preset. An invalid name or value
  rejects the entire request. The window and `view.snapshot` share these
  settings.
  The Gameplay rows are `spawn_points`, `flags` and `netgame_items`. Their
  counts cover starting locations, netgame flags and netgame equipment,
  respectively. Each uses its matching placement block switch as well.
  Turn off `markers` to hide annotations, including the flag-type and
  equipment respawn labels. The Art preset keeps equipment models visible.
  The Volumes rows are `trigger_volumes`, `kill_volumes` and
  `bsp_switches`. Every trigger volume is in the first; the other two
  count the distinct volumes a kill trigger or a BSP switch names. The
  Gameplay preset shows all three and the Collision preset shows
  `kill_volumes`. The Navigation rows are `pathfinding`, `bot_nav` and
  `bot_spots`, counting sectors, bot polygons and spots; the bot rows
  count 0 until the bots' data has loaded (`nav.bots`).
  The Playtest rows are `playtest_kills`, `playtest_deaths`,
  `playtest_spawns`, `playtest_traffic`, `playtest_flow`,
  `playtest_unvisited`, `playtest_sightlines`, `playtest_weapon_use` and
  `playtest_stuck`; they start hidden and the Playtest and Everything
  presets show them. Each `count` is its marks in the result `playtest.*`
  last published (below), 0 with none; a row also has `refused`, the
  measure's refusal text when it had too few events, else null.
  `diff` draws them as differences: `true` compares the shown run with
  its `compare_to`, else the baseline; a run id or `baseline` names the
  other side; `false` draws the result itself. The reply's `diff` is
  `{a, b, game_type}` while one is drawn, else null; each playtest row's
  `count` is then its difference marks and `refused` says why a layer has
  none (the sightlines have no difference layer). With nothing to compare
  (only the baseline is shown, or no run) the call is `REFUSED` and
  changes nothing.
- **Trigger volumes.** `volume.list {}` returns `{volumes, kill_triggers,
  bsp_switches, selected, error}`. Each volume has its `index` in the
  scenario's trigger volume block, `name` (the string ID's text, or null),
  `label`, `kinds` (what it is drawn as, most specific first: `kill`,
  `bsp_switch`, `trigger`), the box as `origin` (a corner), `forward`,
  `up` and full `size` along forward, left and up, its `center`,
  `parent` (an object name index, or null) and `world`, and `named_by`:
  `[{kind: "kill", index}]` and `[{kind: "bsp_switch", index, from, to}]`.
  A volume with a parent object has `world: false` and a null `center`:
  its frame is relative to an object only a running game places, so the
  viewport does not draw or pick it. `{"kind": "kill"}` keeps the
  volumes with that kind. `error` says why the volume blocks did not read;
  the placements still open. `volume.select {"index": 1, "focus": true}`
  selects a volume, clears the placements and frames the box;
  `volume.select {}` clears it.
- **Editing volumes.** `volume.set {index, ...}` gives a trigger volume a
  new box as one undo step and returns its `volume` row: `forward`, `up`
  and `size` over its own (forward and up are made unit and square),
  then the corner `origin`, or the `center` (its own centre when neither
  is given, so a resize or turn keeps it). `volume.add {}` adds a 2-unit
  cube at the camera focus (or the box the same fields give) and selects
  it; `{"kill": true}` also adds a kill trigger naming it, in the same
  undo step, and returns `{index, kill_trigger, volume}`. Adding past a
  block's source size is refused (`REFUSED`, with `{kind, count,
  capacity}`) while growth is off, as `scenario.add` is.
  `volume.remove {index}` removes a trigger volume (the last one takes its
  slot, and the kill triggers and BSP switches that named it follow);
  `{"kind": "kill", "index": 0}` removes a kill trigger. A volume a kill
  trigger or BSP switch names is refused with a message naming them and
  `{named_by}`; so is a volume the map came with when it has compiled
  scripts, which name volumes by index. BSP switches are read only. The
  three blocks are the editor's own list (`scenario_edit::volumes::VolumeBlock`),
  not `map::scenario_layout` blocks, so `scenario.*` does not list them.
- **Level collision.** `collision.surfaces` lists every surface of each
  structure BSP's collision BSP (`scenario_edit::collision_view`) with
  its plane (`[nx, ny, nz, d]`, the flip bit applied), raw `flags`
  (`breakable` is bit 3; the other bits are not named), material, ring
  and `class`: `collision_only` when no drawn triangle of the structure
  or its instances lies on its plane (within 0.01 units, the normal
  within about five degrees either way) over it, else `drawn`, with the
  share `covered`. `{"class": "collision_only"}` lists the invisible
  walls; `offset` and `limit` (200) page the list, `matching` counts it,
  and `errors` names a BSP whose collision did not read.
  `collision.at {origin, dir}` returns the first surface a ray meets and
  `{point}` the one a point lies on (within `within`, 0.05 units), each
  whatever layers are shown; `select` selects it. `collision.select
  {index, focus}` selects a surface and frames it. Instanced geometry's
  collision is in the same list, placed in the world, with the
  placement's index as `instance` (`null` for the structure's own).
  `collision.objects` lists each placed object tag's model, `coll` and
  `phmo` (`models`), and every shape a placement places (`shapes`: a
  `coll` BSP or a `phmo` sphere, pill, box or polyhedron, with the `hlmt`
  node it is posed by, its centre and radius in world space); `block` and
  `index` keep one placement's. Read only: the barrier edits change
  collision. The four collision layers (Level collision, Collision only,
  Instance collision, Object collision) start hidden; the Collision and
  Everything presets show them.
- **Navigation.** (`scenario_edit::nav_view`; docs/editor/editor.md,
  "Navigation".) `nav.pathfinding` reads each structure BSP's own AI
  pathfinding data: per BSP whether it has any (`present`), its sector,
  link, 2D node, vertex and ref counts, `orphan_links` and the sectors
  left out because their ring does not close (`broken`); then the
  drawable `sectors` (`index`, `bsp`, `sector`, `flags`, `hint`, `floor`,
  the ring's `links`, `ring`, `center`), filtered by `bsp` and paged
  with `offset` and `limit` (200). A map with none (retail multiplayer
  maps) answers empty with a `note` saying why. `nav.bots` reports the
  bots' nav mesh and superbounce spots (`studio::app::bot_nav`): it
  starts the load if nothing has, and `wait` blocks until it is done
  (a mesh built in memory on a large map takes a while). `state` is
  `no_map`, `idle`, `loading`, `ready` or `failed` (with `error`);
  `origins` names each BSP's cache file, or `built`; `counts` has the
  polygons, those out of the map (`out_of_map`), links, wall triangles
  and spots; `links_by_kind`,
  `spots_by_kind` (bounce, strip, crouch) and `spots_by_state` (proven,
  failed, dead, learned, training, likely) count them; `notes` says what
  was built or scanned instead of read, and which file did not read.
  `polys`, `links` and `spots` list them, each paged by `offset` and
  `limit`; a polygon row's `out_of_map` is true for one only a glitch
  reaches ([bots.md](../game/bots.md) §3.2, "In the map"). `nav.pick {origin, dir}` or `{point, within}` (0.3 units)
  returns what a ray meets or a point names: a bot spot or link first,
  else the nearer bot polygon or pathfinding sector, whatever layers are
  shown unless `shown`; `layer` keeps one layer's. `select` selects it;
  `{kind, index, focus}` selects one directly (`sector`, `poly`, `link`
  or `spot`, indices as the lists give them). The three layers
  (`pathfinding`, `bot_nav`, `bot_spots`) start hidden; the Navigation
  and Everything presets show them.
- **Playtest's static checks.** (`studio::app::playtest_static`;
  [playtest.md](playtest.md#static-checks).) `playtest.static` checks the
  open map as edited, saved or not, with no match. It starts the bots'
  mesh load as `nav.bots` does, and `wait` blocks until it is done; `nav`
  is that load's state. `problems` lists each finding as `{kind, text,
  target}`: `no_mesh` (no mesh to check against, and why), `off_mesh` (a
  starting location, netgame item or netgame flag over no polygon, from
  0.1 units under its floor to 1 over it), `unreachable` (a starting
  location the mesh's links do not lead from to another), `island`
  (polygons no starting location reaches, grouped by their links, 1
  square unit or more), `one_way` (a link from the reached mesh into
  polygons with no way back, one per trap) and `in_sight` (two starting
  locations not of one team with a clear line between their eyes, 0.6
  units up and at most 40 apart, through the level collision). A
  `target` is `{block, index}` for `nav.go`, `{kind: "poly", index}` for
  `nav.pick`, or `null`. `ok` is true when there is no problem. `ready`
  has one row per built-in game type (`rules::builtin::ALL`): its
  `game_type`, `engine` and `ok`, and `missing`: the start gate's
  problems with the map's census for teams 0 and 1, two starting
  locations on the mesh, and for Capture the Flag a `ctf_flag_spawn`
  flag per team, for Assault an `assault_bomb_spawn` flag per team (or a
  neutral one) and an `assault_bomb_return` flag per team, and for both a
  starting location each team can use (its own or one with no team).
  Without a map it is `NO_DOCUMENT`.
- **Playtest's runs.** (`studio::app::playtest`;
  [playtest.md](playtest.md#what-a-run-is).) `playtest.run` plays the
  open map through a sim runner: it checks the map as `playtest.static`
  does, builds the project, makes the bots' mesh for the build, writes
  `match.json` and `batch.json` under `<cache>/playtest/<run>/` and
  starts the batch, then answers at once with `{run, build, matches,
  parallel, cached}`. `preset` is `probe` (the default: seeds 1 to 4 of
  the first ready game type, 2 game minutes), `standard` (seeds 1 to 8
  of every ready game type, 5 minutes) or `soak` (seeds 1 to 32 of every
  ready game type, each played twice with the sides swapped, to the
  score limit). `seeds` (1 to 256) replaces the count, `minutes` (1 to
  120) the length, `bots` (2 to 16, default 4) the seats, and
  `game_types` names the ones to play (any name `h2 sim` takes); one
  that is not ready is refused. A run with the same build, match, seeds,
  bots' version and image as a finished run with no failed match answers
  with that run and `cached: true`, and starts nothing. A newer run
  cancels one still going. With no sim runner connected (none is wired
  to the window yet) it is `REFUSED`; with no game type ready it is
  `FAILED` and says what each lacks; a bad param is `INVALID_PARAMS`.
  `playtest.status` gives `{run, state, preset, build, compare_to, done, total,
  speedup, stale, coarse, failed}`: `state` is `none` (no run yet),
  `running`, `done`, `failed` (every match failed) or `cancelled`;
  `speedup` is game seconds played per wall second; `stale` is true once
  a newer run's build is another build; `coarse` is true under 8 seeds;
  `failed` lists `{game_type, seed, swapped, why}`. `playtest.result`
  answers over the matches that ended so far, for one `game_type` (the
  first played by default): the run's `build`, `map`, `match_file`,
  `seeds` and `game_types`, `traces` and `seconds` counted, `partial`
  while matches are still to come, and `measures`: each of `heat`,
  `spawns`, `flow`, `sight`, `items` and `stuck` as `{ok: true}` or
  `{ok: false, refused: {measure, what, have, need}}` when its events are
  too few. With `measure` it answers that one in full: `ok` and `value`
  (its bins) or `refused`. `playtest.cancel` stops the running run, or
  the one named, and answers `{cancelled}`; what came back stays. `compare_to` on `run` is a run's
  id or `baseline` (the first run of the session with a match that
  ended), kept on the run. `playtest.compare` compares two runs over the
  same seeds, length, seats and sides (otherwise `INVALID_PARAMS`): `a`
  defaults to the newest run's `compare_to`, else `baseline`, `b` to the
  newest run, `game_type` to the first both played. It answers `measures`
  (each scalar's `a` and `b` means over seeds, `spread`, `n_a`, `n_b` and
  `changed`, true only when the means differ by more than the spread; the
  spread is null with fewer than two seeds), `changed` (their names),
  `layers` (each difference layer's `marks`, the five biggest as `top`,
  or `refused`) and `static` (`gained` and `lost` problems), or
  `{compared: false, why}` when the newest run is the baseline or no
  match has ended. A call
  names a run by its id (`r1`), or the newest. Each of `run`, `status`,
  `result` and `cancel` publishes the newest run's result (for the
  `game_type` of a `result` call, else the one already shown, else the
  first played) to the viewport's playtest layers and the Playtest panel
  (`app::playtest_shown`); `view.layers` counts them and `view.snapshot`
  draws them.
- **Renders.** `ui.render` draws on the CPU (no GPU needed); `view.snapshot`
  renders the open map offscreen through the viewport's own renderer
  (wgpu, no window) and answers `NO_GPU` on a machine without an adapter.
- **Render stats.** `view.stats` (short form `render-stats`) says what the
  viewport draws for the open map, with no GPU: `lightmap_pages`, each
  BSP's `lighting` (`lightmapped`, `no lightmap`, `empty lightmap group`,
  `lightmap of another BSP`, `lightmap does not decode`), the world's
  `triangles` by blend with `solid_triangles` and `lightmapped_triangles`,
  every material (`shader`, `template`, `blend`, `blend_from_passes`,
  `emissive`, `base`, `details`, and `mask`: the coverage mask a
  see-through layer multiplies in, or null), the `sky` drawn and the `atmosphere`
  read from it (`sun` direction, colour and power; `fog` colour, density,
  start and opaque distance; `fallback_rgb`). `{"scene": false}` while
  the viewport shows the grey world (no art built). `native` is the
  level on the native renderer (`scenario_edit::native_view`): the
  `lighting` mode, each BSP's `bsp`, `draws`, `batches`, `materials`,
  `lightmap_pages`, `untextured` (draws left out for a missing picture),
  `failed_sections` and `lightmap_error`, its `base_look`
  (`translated` when the base look draws as a translated pass; else the
  `fallback` cause, such as a placement with no model rows or a program
  that does not compile, with `world.wgsl` drawing it and `layers_off`
  true: no world layer draws over it), its world `layers` (null
  when not read from the map: `unresolved`, the world materials that
  did not resolve by cause with how many, and `by_layer`, for each of
  layers 1, 3, 5, 7, 8 and 20 the `layer`, the materials whose passes
  `compiled`, those `drawn` and their `parts`, every other material's
  cause with how many, `refused`, as `cause: detail`: `program` for the
  load's refusals, such as a view constant the camera does not give or
  a layer-3 stage bitmap that is not 2D, then the plan's causes,
  `animation` for an unsupported target, `fog`, and for layer 1 `layer 1: stencil mode not checked
  against the game`; and `parts_refused`, layer 3's parts of a drawn
  material that its pass does not take, by cause with how many, which
  draw the base look; they draw in the `game` mode only), the `errors` of BSPs that
  did not load, and the placed `objects`: how many object tags have a
  native model (`models`), how many placements draw one (`placed`), and
  each tag left to the scene's model with why (`errors`, `tag` and
  `error`). What each field means is in [editor.md](editor.md),
  "What the viewport shows".
- **Lighting.** `view.lighting {}` returns `{mode, modes}`;
  `view.lighting {"mode":"flat"}` sets it for the window and
  `view.snapshot`: `game` (base map times lightmap), `flat` (no
  lightmap), `lightmap` (the lightmap on white), `collision` (flat at a
  third). Another mode is `INVALID_PARAMS`. To check a
  map: `h2 studio open mainmenu`, `h2 studio render-stats`, then
  `h2 studio snapshot` and look at the PNG.

## The preview switch

`preview.target {doc?, target?}` reads or sets where a document's preview
shows (docs/editor/studio-redesign.md, "Preview and the paired game";
`app/preview_switch.rs`, `api/preview.rs`). `doc` is a `shell.tabs` id
(the front tab without it). `target` is `studio` (the native view or the
kind's own renderer, the default), `game` (a game Studio started) or
`attached` (a game someone else started). A game target needs a paired
game of that kind, else it is `REFUSED` with the reason. The answer is
`{doc, target, targets: [{id, label, available, why?}], game, why?, in_game: {available, why?}}`.
`game` is the `pair.preview` params the document's kind takes, or null
with `why`:

| Document | `game` |
|---|---|
| a `weap` tag | `{what: "weapon", target: <tag name>}` |
| a `vehi`, `bipd`, `scen` or `eqip` tag | `{what: "object", target: "<group>:<tag name>"}` |
| a `wgit` or `wgtz` tag, or the screen in the UI Screens tool | `{what: "ui", target: <screen name>}` |
| a `nhdt` tag, or the HUD tool's document | `{what: "hud"}` |
| a `snd!` tag | `{what: "sound", target: <tag name>}` |
| any other tag (`lsnd` and `ugh!` too), the map, a script, a variant or a playlist file | null, with why |

The header and **Preview** menu both hold the three targets and **In
game** (Cmd/Ctrl+Enter). `pair.preview {doc?}` runs that action on a tab
from `shell.tabs`, or the front tab without `doc`. The document form
takes only `doc`; use the explicit `{what, ...}` form to customize a
paired preview. `preview.target` gives its paired params in `game` and
the availability of **In game** in `in_game`.

- A paired preview answers with its job id. The Game panel shows the
  state and any refusal or warning from `pair.result`.
- A variant queues the same own-game launch as `game.start`, on the open
  multiplayer map with the variant's unsaved record. A named rules
  script travels with the record when Studio has its IR.
- A script compiles the open buffer and queues `rules.reload` on Studio's
  own game. If needed, Studio starts an own game on the open multiplayer
  map with a record for the script's carrier. The reload waits for the
  own game's gameplay phase and uses the editor's reload mode.

The variant and script forms answer `{requested: true}`. The Game
panel, `game.status` and `rules.get` show the launch or reload outcome.

```sh
h2 studio preview.target '{"target":"attached"}'
h2 studio pair.preview '{}'
```

The tests are `api/preview/tests.rs`' `switch` module, on the invented
game.

## Previewing content in the paired game

`pair.preview` puts content into the paired game and reads it back, so a
weapon, an object, a menu screen or the HUD is checked in the running game
rather than found in play (`crates/apps/studio/src/api/preview.rs`). It
answers at once with `{id, state}`. `pair.result {id?}` reads the job
(the latest one without `id`). One job runs at a time and the last eight
are kept.

| `what` | `target` | The game calls, in order |
|---|---|---|
| `weapon` | a `weap` tag's name or datum (required) | `sim.give` to `player` (default 0; `keep: true` adds it as a spare), `session.wait`, `world.object` for the held weapon, `pogo.state`, `render.scene`, the capture |
| `object` | an object tag's name or datum (required; `vehi:<name>` names the group) | `world.object` for the player, `sim.spawn` at `pos`, else `distance` (default 3) world units in front of the player and facing them, `session.wait`, `world.query` near it, `render.scene`, the capture |
| `animation` | an object tag with its group (required: `scen:<name>`, `vehi:`, `mach:`, `bloc:` or `crea:`), and `animation`, a name in the object's own graph (required) | `world.object` for the player, `sim.spawn` as for `object`, `world.object.animate`, `session.wait`, `world.query` near it, `render.scene`, the capture |
| `ui` | the screen expected (optional) | `ui.audit`, `ui.stack` and `ui.screen` where listed, the capture |
| `hud` | a `<name>.state` method to read with it (optional: `superbounce`, `pogo`) | `cheat.list`, `cheat.set hide_hud false` when it was on, `session.wait`, the state, `render.scene`, the capture, then `hide_hud` back on |
| `sound` | a `snd!` tag's name or datum (required) | `audio.play` to `player`, `audio.voices` every 100 ms for `seconds` (0.1 to 30, default 2), `audio.loops` |

An `animation` preview checks the game lists `world.object.animate`
before it spawns anything, and fails the job when it does not. `ticks`
is the wait between the animation's start and the one capture; the
result's `applied` holds the `spawn` and `animate` answers, and `reads`
the object's `world.query` row and the animation's start
([`world.object.animate`](../runtime/agent-api-input.md#worldobjectanimate)).

A `sound` preview has no picture. Beside the game's readings (`applied`,
the answer of `audio.play`, and `audio`, as `pair.audio` gives it) the
result has `editor`: Studio's own `sound.measure` of the tag (levels and
verdict against the open map's sounds), or `{why}` when the tag is not in
the map the Tags tool shows. A game that does not list `audio.play` fails
the job. The sound starts at the player's unit through the game's own sound
start ([`audio.play`](../runtime/agent-api-input.md#audioplay)).

The other params are `ticks` to let run before the reads (0 to 600;
default 30, and 0 for `ui`), `view` for the picture, `out` to copy the
PNG to, and `edits` (`send`, the default, or `skip`). `view` is `render`
(the default: `render.capture`, falling back to `host.screenshot`) or
`screen` (`host.screenshot`, the window's frame as the player sees it).

Pending edits reach the game first, through the pairing's own path. With
edits waiting and the editor in View, the job queues `mirror.apply
send`'s request. It is `sending` until the window has sent every waiting
row (up to 15 s, else `failed`). A conflict, a refused write or an edit
that needs a reload stays in Pending. The result lists those rows under
`edits.left`, with a warning. A preview of a paused game is `REFUSED`,
except for `ui`.

The game calls run on a thread and a socket connection of their own, so
the window keeps drawing. A method the game does not list is skipped.
A missing method the preview needs (`sim.give`, `sim.spawn`, `ui.audit`
or `ui.stack`) fails the job, and so does a game refusal, with the game's
message in `why`.

`pair.result` gives `state` (`sending`, `running`, `done`, `failed`),
`why`, `elapsed_ms`, `steps` (the game calls made so far) and `edits
{pending, sent, left}`. Once the job is done it also gives:

- `applied`: the game's answer to `sim.give` or `sim.spawn`.
- `reads.weapon`: `object`, `tag`, `name`, `held` (the player's held
  weapon's tag is the given one after the wait) and `held_tag`.
- `reads.pogo`, where `pogo.state` is listed: `stick` (the weapon is one
  of the kit's three), `active`, the `rider` holding it and the chase
  `camera` (`on`, `drawn`, `room`, `share`).
- `reads.camera`, where `render.scene` is listed: the last sealed scene's
  `views` (`key`, `instances`, `cut`) and `first_person {views, drawn}`.
  `drawn` is true when a `first_person:` view drew an instance.
- `reads.object`: the spawned object as `world.query` lists it, or null.
- `reads.ui`: `shown` (the names and ids of `ui.audit`'s active surface
  and menu, and `ui.screen`'s class and screen), `matches`, and the raw
  `audit`, `stack` and `screen`.
- `reads.hud {hidden_before, shown}` and `reads.state`.
- `capture`: `path`, `method`, `tick`, `present`, `width`, `height`,
  `mean_luma` (Rec. 709, 0 to 1), `dark_share` (pixels below 4% luma) and
  `black` (a `dark_share` of 98% or more).
- `warnings`: a black frame, a weapon not held, no first-person view
  drawn (except while the Pogo chase camera draws), no rider for a Pogo
  stick, a chase camera asked for and not drawn, a spawned object not
  found, another screen shown, `ui.audit` issues, edits left in Pending.

`pair.preview` does not open a menu screen. The game API has no call that
opens a screen by name: the guest opens screens from its own UI code
(`game::ui_kit::dialog_tag::open_over`), and a host call into it would
need a guest call queue like `sim.give`'s. Drive the menu to the screen
with the game's `ui.press`, then preview `ui`.

`pair.audio {seconds?, every_ms?, call?}` makes `call` (`{method,
params?}`, a game method that makes the sound, such as `pogo.test`),
samples the game's `audio.voices` every `every_ms` (20 to 1000, default
100) for `seconds` (0.1 to 30, default 3), then reads `audio.loops`.
`pair.result` gives `audio`:

- `samples`, `voices` (distinct voice ids) and `most_at_once`.
- `peak_gain` and `peak_db`: the largest `applied_gain`, the level a
  voice's direct path leaves the mixer with, before bin volumes and pan.
- `loudest`: the eight loudest voices, with `id`, `kind`, `peak_gain`,
  `peak_db`, `volume_mb`, `distance_units` and `samples`.
- `loops`: the looping sounds playing, with `tag_name`, `tag_datum` and
  `segment`.

These are mixer gains, not the samples' own level. The game has no output
meter (`H2_AUDIO_STATS` reports playout health, not level), so a sample
that is too loud at full gain shows here only as a voice at 0 dB. A
sound's own level is read in the editor from its tag.

```sh
h2 studio pair.preview '{"what":"weapon","target":"objects\\weapons\\multiplayer\\pogo\\unsc\\pogo_unsc","out":"/tmp/pogo.png"}'
h2 studio pair.result '{"id":1}'
h2 studio pair.audio '{"seconds":3,"call":{"method":"pogo.test","params":{"trick":"backflip"}}}'
h2 studio pair.result
```
