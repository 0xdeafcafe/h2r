# Theater: saved films, free camera and spectating

For the unified host recording, binary layout, reader API and current replay path, see
[the film format](film-format.md). The tier-B v1 design and its baseline
experiments follow below.

This is the design for a Halo 3-style Theater in h2recomp: record a game
as a film, then watch it again with pause, fast forward, rewind, a free
camera, another player's view, clips and screenshots. It also covers the
"event" side: watching a game live as a spectator, with a player list and
scoreboard. **The in-game selector and host-film player are implemented. The
real-host hash and camera experiments are pending.** Playback uses the determinism
harness from the high-frame-rate work
([high-framerate.md](high-framerate.md), phase 1, built) and on
levels running at a steady 30 fps ([gameplay-budget.md](../perf/notes/gameplay-budget.md)).

In short:

- **A film is a recording of everything the guest can't compute itself.**
  The recompiled game already runs on a logical clock, so its schedule
  depends only on its own work and its inputs. Record the inputs (pad
  reads, network receives, host randomness, the boot conditions) and the
  same build replays the session exactly. The harness (`crates/extensions/lockstep`)
  already records and replays pad reads by read index.
- **That exact replay is the first slice**: record a game, watch it back
  from the start with pause, fast forward and slow motion, and export
  screenshots and clips by re-rendering offline. It needs no game
  knowledge beyond what the harness has, and it works for every mode.
- **A free camera and rewind need a second layer that Halo itself
  provides.** Halo 2 keeps its whole simulation in one fixed 4 MiB
  game-state block (the block its checkpoints save), and its camera
  director lives outside that block, in static data. A film can carry
  compressed copies of the block as keyframes and the per-tick player
  actions. Playback restores a keyframe and feeds the actions in, so the
  camera and the rendering can change without the simulation noticing.
  This is how Halo 3's Theater is generally described. One experiment
  decides whether it holds for Halo 2 on our runtime; it comes before any
  free-camera work.
- **Halo 2 multiplayer is not lockstep.** The task brief assumed it was.
  The retail XBE has both a "synchronous" and a "distributed" simulation
  mode, and community sources say multiplayer is host-authoritative and
  distributed, with lockstep only for campaign and co-op. So a film made
  on a networked *client* can be replayed exactly (it records what that
  client received) but can't be re-simulated from player actions alone.
  The host's film can.
- **The game's own director already has the Theater camera set.** A
  per-local-player camera record selects an update function; the retail
  name table lists following, orbiting, flying, editor, first person and
  scripted kinds. Theater should drive these cameras rather than invent a
  host-side one, because the guest renderer culls for its own camera.
- **Spectating comes in three sizes.** A web scoreboard and top-down map,
  fed by the live telemetry that exists today, is small. A delayed live
  stream of a film to other machines comes after keyframes work. A
  spectator seat inside the game session isn't worth building.

Evidence labels used throughout:

- **[local]**: read from this repository's docs or code, or from a static
  reading of the owner's retail XBE with the repo's tools (`h2 disasm`,
  name tables, cross-references). No game was built or run for this page.
- **[community]**: public sources, cited and summarised.
- **[guess]**: my inference or speculation; needs a check before anyone
  builds on it.

## 0. Theater tiers

Dedicated hosts change this design. A dedicated host is authoritative
and has **no local players**. Its only inputs from outside are network
receives and randomness. That makes its film the simplest one to record
and the most complete one: everyone's game, from the machine that
decided it. So Theater is built in three tiers. Each tier is useful on
its own.

| tier | what you get | built from | state |
|---|---|---|---|
| **A. Web Theater** | Every Live game, in the browser: a film library, the 3D viewer with a transport, player POVs, camera modes, kill bookmarks, clips with share links | indexed pose and event records in the host film, with [spectator archives](../live/replay-fidelity.md) for older games | implemented |
| **B. Host films** | Exact `.h2film` films recorded by dedicated hosts, played back in the game with its own renderer and director cameras | the [film format](film-format.md) and the in-game selector | implemented. Real-host verification pending |
| **C. Live spectating** | A tier-B film streamed with a 30–120 s delay | section 4B | later |

### Tier A: Web Theater (simple, now)

Web Theater reads timed pose and event records from a version 3 host film.
The viewer fetches the chunks covering the requested time range. Older
recordings and games without a host film use the archived telemetry track.

The server keeps:

- **Recent films**: each player's last 25 games, derived from the games
  they played. Nothing is copied.
- **Saved films**: a player pins a game. It's kept past archive pruning.
- **Clips**: `{id, game, from_tick, to_tick, title, by, pov?, camera?}`.
  A clip is a range of a film, not a copy.

Routes:

| route | auth | does |
|---|---|---|
| `GET /v1/films?player=<xuid>&kind=recent\|saved` | public (redaction as games) | film list: game id, map, variant, date, length, result for that player |
| `POST /v1/films/{game}/save`, `DELETE /v1/films/{game}/save` | the player | pin or unpin |
| `GET /v1/films/{game}/clips` | public | the game's clips |
| `POST /v1/films/{game}/clips` | a signed-in player | body `{from_tick, to_tick, title, pov?, camera?}`, 30 s to 5 min, 20 per player per day |
| `GET /v1/clips/{id}`, `DELETE /v1/clips/{id}` | public / owner or admin | one clip |

Pages:

- **Watch in Theater** and the player's **Films** tab open the replay at
  `/games/halo2/games/<game>#game-viewer`. Legacy `/theater/<game>` links
  redirect there.
- A shared clip at `/theater/<game>/clip/<id>` opens its range on a loop and
  has its own link preview. Both routes use the Game Details viewer.
- Spectator archives with projectile samples draw trails from recorded
  positions. The host film draws projectile objects from its pose records.

### Tier B: original host film design (version 1)

The following design describes the first recording format. The current
writer, tick records, keyframes and directory are specified in the
[version 3 film format](film-format.md).

The dedicated host records a machine-level film from `h2 host run`. Its
streams, from section 2:

- `HEAD`: the image stamp, disc identity, map, variant, lease id and roster.
- `DATA`: the start manifest.
- `NETR`: every receive and HLE result, by call index.
- `RAND`: randomness.
- `HASH`: the simulation hash every 30 ticks.
- `INDX`: the index.

There's no `INPT`, because there are no pads. The host uploads the film
to the server's blob store at match end, under the game id.

Playback is `h2 theater play <film>`:

- It boots the same image.
- It replays `NETR`/`RAND` into the HLE.
- It disconnects live sources.
- It reports the first hash mismatch with its tick.
- Controls are pause, 0.25–4× speed and restart.
- Agent methods let tests and the UI drive it.

Two experiments decide the rest:

1. **Rendering during playback.** The light host never renders. The
   render consumes the shared RNG at `0x001032a0`, so a rendered
   playback can diverge from the unrendered recording. The first
   playback runs with `render_every = 0` and checks hashes. Then
   rendered playback runs and is compared. If the hashes diverge, the
   render's RNG draws get their own stream, in recording as well as in
   playback.
2. **Keyframes.** Use machine snapshots (`state.save`/`load`,
   [snapshots.md](../runtime/snapshots.md)) as `KEYF`, every 30 s. The
   light host's clock-only audio holds no samples, so snapshots in that
   profile need the audio section to save its clock alone. Seek means
   loading the nearest snapshot, then replaying `NETR` forward.
   Snapshot-then-run must match the straight run's hashes, which is
   section 5, risk 1.

The rendered player now has fly, follow, orbit and first-person cameras.
Its temporary render view is based on the replayed match and leaves the
simulation's local-player assignments alone. The coordinator can check
camera motion against the same hashes as the original render cadence.

### Tier B implementation and coordinator runs

A leased, playerless `h2 host run` records in its private lease directory
by default. Use `--film FILE` or `H2_FILM_OUT` to retain a copy elsewhere.
The `serve --system-link --host-light` child creates a new file, records
from boot, and seals it at the first slice after the multiplayer end
latch. An orderly earlier exit also seals the recording. The host still
sends its own packets and the recorder observes the results.

In the original version 1 layout:

- `HEAD` carries the exact tier's module stamp, XBE digest and title ID,
  epoch, socket address, scheduler slice budget, feature switches, lobby
  settings and lease ID. Later header chunks supply the map, variant,
  roster and scores from the telemetry readers. Ticks count from boot.
- `DATA` records directories and SHA-256 digests under the guest's
  `disk/` and `metadata/`. Small files are embedded, up to 1 MiB each
  and 16 MiB total. For larger files, supply an unchanged initial copy
  with `--data DIR`. Every playback and seek restores into a fresh
  private directory. The film supplies no host executable or module path.
- `NETR` and `RAND` share a monotonically increasing call index. Each
  entry carries the operation, guest thread, arguments, status/error and
  output buffers. Receive truncation preserves the partial payload and
  sender. Select preserves each returned fd set. QoS preserves the
  allocation and embedded payload pointers. Socket setup, send, close,
  address translation, last error, connection status, FIONREAD,
  random bytes and keys all pass through the same HLE dispatcher.
  Service QoS queries and release, session-id reads and entropy used
  inside other synchronous Live HLE calls are also taped.
- Supervisor lobby calls that were accepted by the agent are recorded
  in `MARK` with the executor boundary that ran them. A state query can
  itself queue a guest readiness check. The player reissues these
  calls at the recorded boundaries. A final mark carries the end slice,
  tick and call count.
- Every 30th completed tick, `HASH` carries
  `lockstep::hash_sim(SimRegion::HALO2)` and the call count. A mismatch
  reports `first divergence at tick T`. A film with no readable arena
  checkpoints cannot pass verification.
- All tags use four bytes, a little-endian compressed length and one
  zstd frame containing a v1 JSON payload. `INDX` ends a finalized
  file. A scan rebuilds the offsets after interruption and drops an
  incomplete trailing chunk. Playback of a prefix ends at its last
  complete checkpoint. `KEYF` is reserved.
- `film::Library::open(dir)` lists the 25 newest films and any older
  saved films. Save/unsave persist pins beside the file. Delete resolves
  the header ID in the library before removing the file. The host UI
  uses `H2_FILM_DIR` or `<data>/films`.

**Record once.** Use a provisioned human-player lease and a LAN host from
the combined dedicated-host build. Export its normal supervisor inputs
(`H2_LEASE`, `H2_LIVE`, `H2_TOKEN`, `H2_DATA_URL`, `H2_DATA_SHA`).
Set `H2_DISC` to the same disc and `H2_IMAGE` to the directory with
the installed module. Keep the same `H2_OPT` and feature switches for
both playbacks. Use an empty host data directory for this baseline.

```sh
export H2_OPT=2
film_run=$(mktemp -d "${TMPDIR%/}/film.XXXXXXXX")
h2 host run --out "$H2_IMAGE" --data "$film_run/host" --film "$film_run/match.h2film"
```

Finish the match with remote players. A normal end should produce scores
and an index. The film stays on disk if the upload fails.

**Experiment 1, original render cadence.** No remote player or running
Live server is needed for playback. The expected result is status 0 and
`hashes matched through tick T`. The player checks the final cursors too.

```sh
h2 theater play "$film_run/match.h2film" --disc "$H2_DISC" --out "$H2_IMAGE" --socket "$film_run/play.sock" --render-every 0 --speed 1
```

**Experiment 2, rendered.** Run the same file and image in a window with
every world frame rendered. Compare the ending tick or retain the first
divergence and its expected/actual hash and call count. A mismatch in
experiment 1 must be resolved before attributing one here to render RNG.

```sh
h2 theater play "$film_run/match.h2film" --disc "$H2_DISC" --out "$H2_IMAGE" --socket "$film_run/play.sock" --render-every 1 --speed 1
```

During either run the agent exposes the [Theater transport](../runtime/agent-api.md#theater-transport).
Seek restarts the child at a compatible keyframe, or at boot when one is
unavailable, and removes host pacing until the target. It compares hashes throughout. The
coordinator should also seek backwards across a received packet while
paused and resume. `h2 film stat FILE` lists embedded keyframes, recorded
D3D barriers and per-stream tape counts, byte totals and operations. The
[measurement procedure](notes/theater-seek.md) uses both tools.

At match end the host uploads the sealed recording under the lease's
canonical game ID. Live checks `telemetry.ingest` and lease ownership.
Small uploads fit in one request. Larger films use numbered parts and
continuations, up to 2 GiB per film. [Upload protocol](../live/live-api.md#theater)
The supervisor gives the child time to finish uploading before reporting
the lease ended. The upload and both hash experiments still need a real
dedicated match.

### In-game Theater

Select **THEATER** directly below **Xbox Live** on the main menu. The
entry in `crates/extensions/game/src/layout_glue.rs` opens the film
library for the controller that selects it. Left and Right switch between
the **Recent**, **Saved** and **Clips** tabs from any row. The active tab
and its film count sit above the list and the details pane.

**Sort**, **Map** and **Game type** show their current values at the top of
the list. Press A to open a picker over the library. A on an entry applies
the choice and returns to the option. B closes the picker and keeps the
current value. Choose **All** in a filter to clear it.

Film rows show the map and game type or the clip's title. Highlight a row
to see its map picture, recording date, running time and match information
in the details pane on the right. Empty libraries use that pane to explain
how to record a film, save one or mark a clip. An empty filtered list keeps
its options in reach. The browser geometry lives in
`crates/extensions/game/src/theater_ui/pages.rs`. Its preview includes an
empty library and a scrolled 25-film list.

Press A on a film to open its lobby. The header shows its map, variant,
mode, date, duration and result when known. The Players table shows the
recorded names and available team, score and kill/death columns. A score
without a join record uses the numbered player seat. Left
and Right page through longer rosters. A summary without a recording
shows one reason in the action area. **Watch** appears for playable films.

Recent combines the local library and the signed-in Live account's
archive. Saved includes local and account pins. Clips includes local
in/out ranges and clips authored by the account. Use **Save** or **Unsave**
to change a pin. Local films and authored clips offer **Delete** with a
second press to confirm. The selector takes its data from the library
and the Live listing. Older recordings and archives can leave some fields
blank.

**Watch** starts an isolated instance of the recompiled game that
replays the recording. For an account film, the exact host recording
downloads first. It needs a compatible installed game image and the
same initial files where the film references them by digest. An archive
with only telemetry still has a film page and scoreboard. The selector
reads the version 3 summary for local and host recordings and opens the
exact player for supported films.

Rendered host playback opens in **Fly cam**. Local playback opens in the
recorded retail view. **Orbit** circles the selected
player and **Follow** looks over their shoulder. **First person** uses
a selected local player's recorded viewport or the replayed host unit's
position and aim. A playerless host has no recorded first-person weapon
view or local HUD. Roster controls
cycle round the available players. The **Default** camera restores the
full local frame or selects the first available player in a version 3 host film.

| Action | Controller | Keyboard and mouse |
|---|---|---|
| Play or pause | A | Space |
| Advance one tick while paused | D-pad Down | `.` |
| Speed (0.25×, 0.5×, 1×, 2×, 4×) | LT / RT | `[` / `]` |
| Seek backwards or forwards | D-pad Left / Right. Tap for 5 s, hold for 30 s | Left / Right, with the same timing, or click the timeline |
| Cycle camera | Y | C |
| Cycle player | LB / RB | Up / Down |
| Fly horizontally | Left stick | W / A / S / D |
| Fly down / up | L3 / R3 | Q / E |
| Look in Fly or Orbit | Right stick | Hold right mouse button and drag |
| Change orbit radius | Left stick forward / back | Mouse wheel |
| Mark clip in, then out | X | X |
| Toggle transport and scoreboard | Back | H |
| Toggle game HUD | Start | J |
| Save a screenshot | D-pad Up | F12 |
| Cancel an active seek or leave playback | B | Escape |

The timeline marks kills as the replay encounters them. During a seek
it marks the destination and fills up to the verified replay cursor.
Skip taps that arrive within 250 ms of one another accumulate into one
jump. Cancelling a jump returns to its departure tick and holds there.
The bar includes the measured speed if playback falls behind the
requested speed. Seeking carries kill markers and any in/out marks
across the restart. Completed clip
ranges appear in Clips and loop when watched. Screenshots are saved under
`<data>/films/screenshots`. Pad and keyboard controls stay with the
playback window while the overlay is hidden. Paused camera changes apply
to the next rendered frame.

The library uses the retail settings list beneath a tab strip and preview.
The film page uses the pregame lobby's action skin, text and row spacing.
Both resolve map art from the multiplayer and campaign UI level lists.
The corners of the preview frame remain visible when the map has no
picture. The guest and editor preview use the same Theater page model.
See [UI builder](ui-builder.md) and [editor](../editor/editor.md). `game::theater_ui::adapter` draws its
metadata from `film::Library` and the signed Live listing. Live's
`GET /v1/games/{id}/film` streams the private recording to authenticated
participants. The menu and playback window use
[the Theater agent methods](../runtime/agent-api.md#theater-transport).

Offline samples are `theater.films`, `theater.film` and
`theater.playback` in `h2 ui preview`. The browser includes empty Recent,
Saved, Clips and filtered states. Film details include a summary without
a recording and a paged roster with names only. The main-menu selection
dialog reports `kit.picker` through `ui.screen`.

### Web Theater

The player page has a Films tab with Recent, Saved and Clips. The web
viewer reads the host film’s presentation and event records, with spectator
archives for older matches. It has its own transport, scoreboard, camera
controls and clip share links.

### Lanes (code only, one combined test pass)

| lane | owns |
|---|---|
| `theater-server` | films/clips routes and storage in live-server (both homes), the saved-film archive pin, tests |
| `theater-web` | `/theater/*` pages, the Films tab, camera chips, the clip flow, tests |
| `theater-film` | a `film` crate: `.h2film` writer/reader (chunks above), the host recorder taps (`NETR`, `RAND`, `HASH`) behind `H2_FILM_OUT` / `h2 host run --film`, upload at match end, `h2 theater play` exact playback + agent methods `theater.play/state/pause/speed/seek/camera` |
| `theater-ui` | the in-game Theater row, Films screen, film page and playback overlay, against the `film` crate's `Library` API and the agent methods |

## 1. How Halo 3's Theater works, and whether Halo 2 fits

### Halo 3 (2007)

- **Films record inputs, not video.** Theater records the button inputs of
  players and AI and rebuilds the game by playing them back through the
  engine, which is what makes a free camera possible. Matches are recorded
  automatically; up to 100 films can be kept. [community: [Halopedia,
  Theater](https://www.halopedia.org/Theater)]
- **Films are tiny.** Notes from a Bungie talk put 2.5 hours of saved film
  at about 3 MB, roughly 20 KB a minute. The same notes say films expose
  bugs that go unnoticed in live play. [community, second-hand: reported in
  search results for Bungie talk notes; the original talk wasn't located]
- **Campaign was the weaker case.** Campaign films could be watched, but
  not rewound and not clipped; multiplayer films could do both.
  [community: Halopedia, Theater] My reading of that: rewinding needs
  periodic state snapshots, and a campaign's state is much larger and more
  varied than a multiplayer map's, so campaign films only played forward.
  [guess]
- **Camera.** A free camera that roams like Forge's, bounded by the map,
  and the players' own views. [community: Halopedia, Theater]
- **Versioning.** Theater fixes shipped in Halo 3's first title update.
  [community: [Halopedia, Halo 3 Title
  Updates](https://www.halopedia.org/Halo_3_Title_Updates)] Input-based
  replays in general break when the update loop changes, which is why
  other projects stamp each replay with the build that made it. [community,
  general practice] MCC has Theater for Halo 3, ODST, Reach and Halo 4,
  and for Halo 2 Anniversary multiplayer (a different engine), but not for
  classic Halo 2. [community: Halopedia, Theater]

What a Halo 3-style film needs from an engine, then: a simulation that is
a pure function of its state and per-tick inputs; a way to snapshot and
restore that state; and rendering and camera code that can change without
touching the simulation.

### What Halo 2 has

**A deterministic fixed tick.** [local: engine-map.md, high-framerate.md
section 1] `game_tick` (`0x00137fe0`) runs at a fixed rate from a budget
(`0x00146660`) that turns frame time into whole ticks. Input is polled once
per main-loop iteration, before the ticks. The director (`0x001554b0`) and
observer (`0x0016f280`) run once per frame with a variable time step,
*after* the ticks and outside them.

**One block of simulation state.** [local: engine-map.md, checkpoints.md]
`game_state_arena_initialize` (`0x00123b30`) clears a `0x3fe000`-byte
(about 4 MiB) arena; pools are bump-allocated from it by name
(`0x00123dd0`). Checkpoints copy exactly this block: the writer
(`0x2151f0`), revert to a slot (`0x123f60`) and full restore (`0x124380`)
are exercised byte-exactly by `crates/platform/xapi/tests/checkpoints.rs`. This is
the classic Blam design: the game's state is one relocatable blob that is
saved by copying memory. It's the strongest reason Halo 2 fits a
Halo 3-style Theater.

**Camera state outside that block.** [local: static reading for this page]
The director keeps one record per local player at `0x004E8C44`, stride
`0x140`, with the active camera's update function at `+0xC`. That address
is in the XBE's static `.data`, not in the game-state arena, so a
checkpoint doesn't save it and changing it can't change saved state.
`0x00155760` classifies the active camera by comparing that pointer with
first person (`0x0023c110`), following (`0x0023cbb0`), scripted
(`0x0016c840`) and `0x0023d090`, the last only when the byte at
`0x0051EC10` is set. `0x0023d090` sits in a stretch of the camera family
(`0x0023b060..0x0023ea79`) that the Ghidra project hasn't split into
functions. [guess] It's the flying (debug) camera, gated by a debug flag.

**A director camera name table.** [local] `.rdata` holds pointers at
`0x00468cfc..0x00468d14` to the names following, orbiting, flying, editor,
first person, camera and a sync camera. No code references the table
directly; it's probably for debug output. Its presence says the retail
director still knows the camera kinds a Theater needs. Whether the flying
and orbiting update functions survive intact in retail code is unverified.

**Synchronous and distributed simulation.** [local] The network layer
names its simulation modes in a table at `0x00468c64` (none, local,
sync-client, sync-server, dist-client, dist-server). `0x00081780` creates
two data arrays named "simulation view" and "simulation distributed view".
The message serializer registration near `0x000b2b36..0x000b2c0e` names
synchronous gamestate, join, actions and update messages, and nearby names
cover replication of entities, events and game results, and losing
authority. live-gamestate.md has the mode byte at `G+0x0c`.
[community: [halo.bungie.org network
guide](https://halo.bungie.org/misc/networkguide.html), [Halopedia,
Connection Host](https://www.halopedia.org/Connection_Host), [Slashdot,
Taking Halo 2 to Xbox Live](https://games.slashdot.org/story/04/12/14/189243/taking-halo-2-to-xbox-live)]
Halo 2 multiplayer has one console host the game; clients accept the
host's version of damage and spawns. Community guides describe campaign
and co-op as synchronous lockstep and multiplayer as asynchronous and
distributed. Standby cheating (the host's game runs on while clients see
"reconnecting", other players sliding in straight lines) fits a host with
authority and dead reckoning, not lockstep. Bungie's later Reach talk
traces their model to Tribes-style replication. [community: [GDC 2011, "I
Shot You First"](https://www.youtube.com/watch?v=h47zZrqjgLc)]

So: **lockstep in Halo 2 is the campaign/co-op path; system link and Live
multiplayer use the distributed model.** [community + local names;
runtime confirmation is cheap: read `G+0x0c` in a system link game, step
0 of phase 1 below]

**Film remnants.** [local] Among the session's end reasons (a table at
`0x0046715c`: stopped, reset failed, fatal error, lost connection, out of
sync, film ended), "film ended" sits beside "out of sync". [guess] Halo 2's
simulation layer had, or planned, film playback as a session source: the
model Halo 3 shipped. No other film-related names show up in the XBE. No
code that produces that reason has been located yet; finding it is a
research point, and may show a dormant playback path.

**Input recordings for units.** [local] HaloScript has a type named
"cutscene recording" and a game-state allocation called "recorded
animations" (referenced at `0x001fb30c`). [community, Blam modding
knowledge, guess for Halo 2 specifically] In Blam engines these are
scenario-authored control streams that drive a unit's input, used for
scripted vehicle and character moves. It's more evidence that unit
control is a compact per-tick record that can be fed from data.

**A debug camera.** See the director above. No debug-camera script
functions survive: retail stubbed 306 of 911 HaloScript descriptors and
dropped the builtin names (engine/hs_vm.md).

### Fit

| need | Halo 2 | verdict |
|---|---|---|
| fixed tick, state + inputs determine the next state | fixed tick; lockstep campaign/co-op relies on it, and an out-of-sync end reason exists to catch failures | good for campaign and local games; unproven for a networked host |
| snapshot and restore of state | 4 MiB arena, real save/revert/restore paths, tested here | good |
| camera that doesn't touch state | director records in static `.data`, run per frame after ticks | good |
| free and third-person cameras | following, orbiting, flying kinds in the director | likely; flying's retail code unverified |
| multiplayer re-simulation from inputs | distributed model: a client sees replicated state, not everyone's inputs | client: exact replay only; host: probably, after research |

**Our runtime adds a second, stronger determinism.** [local:
high-framerate.md section 1, kernel README "Clock and replay"] The whole
machine runs on one logical clock. Generated code charges guest time for
its memory accesses (kernel README; app.md still says compute is free,
which looks stale). Threads run cooperatively on a single host thread,
guest stacks live in guest RAM, and wall time only ever makes the host
wait. So the same build, given the same inputs at the same points in its
execution, runs the same instructions on the same schedule. That's
machine-level determinism, and it covers every mode, networked clients
included, as long as everything nondeterministic is recorded.

The two layers have different failure modes, and the design uses both:

| | machine-level (our runtime) | engine-level (Halo's tick) |
|---|---|---|
| what is recorded | pad reads by index, network receives, host randomness, boot state | per-tick player actions, game-state keyframes |
| what it reproduces | the whole machine, bit for bit | the simulation (game-state arena) |
| survives a camera or render change | **no**: a different camera means different guest work, a different schedule, and pad reads landing in different ticks | **yes**: actions are keyed by tick, and the camera is outside the arena |
| survives a rebuild | no (charged time shifts) | maybe (only if the simulation code's results are unchanged) |
| networked client | yes | no (distributed) |
| seeking | replay from boot, or in-memory machine forks | restore a keyframe, then simulate forward |

## 2. Recording

### What the recorder captures (machine level)

Everything that reaches the guest from outside its own computation, each
keyed by **call index** at its HLE boundary, the way `xapi::tap` keys pad
reads. Call indexes make replay independent of when the host pumped
anything.

| source | boundary | today |
|---|---|---|
| pad reads | `XInputGetState` result and status (22-byte `XINPUT_STATE`) | `xapi::tap::Recorder` |
| device arrival, capabilities | `XGetDeviceChanges`, capabilities | not tapped (the harness replays with the same kind of pad); a film should record them |
| network receives | `recv`/`recvfrom` results (payload, source address, error), `select` and FIONREAD results, `XNetGetConnectStatus`, QoS results | not tapped; live.md "Client HLE" lists the entries |
| host randomness | `XNetRandom`, `XNetCreateKey` (host OS randomness, live.md) | not tapped |
| Live HLE answers | logon, presence, title storage (MOTD and playlists), stats | not tapped; record their results by call index like a receive |
| time | NT epoch (`Config::epoch_100ns`) | configurable; record it |
| start state | profile, settings and save files on U: and E: that the game reads; N: cache contents | record a manifest with hashes, and embed the small user files (they're the user's own data) |
| build | image stamp (generated code, override set, passes), disc identity (title ID, XBE hash) | stamp exists for frame comparisons (BOOT.md); reuse it |

Network *sends* aren't inputs. Record a hash of each so playback can
report the first divergence (a cheap desync detector).

### What the recorder adds (engine level)

These are written during recording, or later by an offline pass over an
exact replay, which gives the same result:

- **Per-tick simulation hash.** A hash of the game-state arena at the end
  of every tick, from the observing wrapper on `0x00137fe0` that the
  `lockstep` crate already uses for its whole-RAM hash. Write one every
  30 ticks. The high-fps harness hashes all of RAM in use
  (`lockstep::hash_guest`). A narrower hash over the arena alone,
  `lockstep::hash_sim` with `SimRegion::HALO2` (the co-op probe's, and
  `lockstep::pools` cuts it per datum array), already exists. Theater
  needs it per named allocation,
  so a camera-driven change elsewhere doesn't count as a desync. If some
  arena allocations turn out to hold view state, add them to an explicit
  exclude list.
- **Keyframes.** A copy of the arena at a tick boundary, compressed, as an
  XOR against the previous keyframe. On disk every 30 s; playback builds
  finer ones in memory as it goes. A keyframe is taken by reading guest
  memory from Rust (uncharged, no guest write), so recording doesn't
  change the run.
- **Per-tick player actions**: the control record each player's unit
  consumes in the tick, for every player, local or remote. Finding it is
  research: player-control preparation `0x001870a0` turns local input into
  per-player controls, and players-before `0x0014d3a0` consumes them. The
  player array is 16 entries of stride `0x21c` (`0x0014b4b0`). Capture it
  at the point of use, after network delivery, so it doesn't matter where
  an action came from.
- **Roster and match facts** for the UI (names, teams, variant), from the
  telemetry readers that exist (live-telemetry.md, live-gamestate.md).

### File format (`.h2film`)

Ours, versioned, chunked, append-only so a crash loses at most the open
chunk. Little-endian. Each chunk is `tag (4) | length (u32) | payload`,
with zstd per chunk (the `zstd` crate is already in Cargo.lock).

| chunk | contents |
|---|---|
| `HEAD` | format version; image stamp; disc identity; map, mode and variant summary; NT epoch; recorder settings; mode (campaign, local MP, system link host or client, Live host or client); local player slots |
| `DATA` | start-state manifest and embedded user files |
| `INPT` | pad reads: per handle, delta against the previous packet, varint read index gaps |
| `NETR` | network receives and other HLE results, by call index |
| `RAND` | host randomness outputs |
| `HASH` | (tick, simulation hash, whole-RAM hash, input read index, receive index) every 30 ticks; the receive and read indexes map ticks to the machine streams |
| `ACTN` | per-tick player actions (engine layer) |
| `KEYF` | (tick, arena XOR previous keyframe), zstd |
| `MARK` | user bookmarks, clip ranges, camera paths (added in Theater) |
| `INDX` | written last: offsets of every `KEYF` and `HASH`; rebuilt by a scan if missing |

A film opens only on a build whose image stamp matches. On any other build
it offers engine-level playback from keyframes ("may drift"), with a
visible warning, and resyncs at each keyframe.

### Size per minute

All of this is an estimate to measure in phase 1; none of it is measured.

| stream | raw | compressed (guess) |
|---|---|---|
| one pad, 30 reads/s at 22 bytes | 40 KB/min | 3-10 KB/min (most packets repeat or change slightly) |
| network receives, system link client | 0.5-1 MB/min (guess: 8-16 KB/s down) | 0.3-0.8 MB/min (game packets are bit-packed and don't compress well) |
| network receives, host of 16 | similar or more, from 15 senders | 0.5-1.5 MB/min |
| actions, 16 players at 30 ticks/s | ~1.4 MB/min at ~48 bytes each | 50-150 KB/min (deltas) |
| hashes | 1 KB/min | 1 KB/min |
| keyframes, 4 MiB arena every 30 s | 8 MiB/min | 0.3-1 MB/min (first one larger; deltas smaller) |

So a campaign or split-screen film is about 0.5-1 MB a minute, mostly
keyframes; a networked film is about 1-2.5 MB a minute. A ten-minute
match is 10-25 MB. Halo 3's ~20 KB a minute is the floor for inputs alone;
we pay for keyframes and raw packets to buy exactness and seeking.

### Multiplayer recording from one machine

- **Any machine** can record a machine-level film. A **client's** film
  replays exactly what that client saw, including its prediction errors
  and lag, but only exactly. There's no free camera, because a free
  camera changes guest work (section 3).
- **The host's** film is authoritative. If research shows that the host's
  simulation of remote players runs on received actions (rather than, say,
  accepted client positions), its engine layer is complete and supports
  the full Theater. If remote players are driven by replicated positions,
  `ACTN` needs those too, captured where the host applies them.
- On our Live service, every game has a known host. A later phase can have
  the host upload its film to the server after the match, as Halo 3 shared
  films through its file share. live.md's title storage path is the
  obvious channel.
- Recording must never reach the network: no recorder message is sent,
  and nothing changes what the game sends.

## 3. Playback

### Exact playback (machine level)

Boot the same image with the recorded start state, install replaying taps
at every boundary in section 2, and run. That's the high-fps phase 1
harness, generalised. Live sources (pads, sockets, Live) are disconnected,
and the recorded results go in their place. Each `HASH` record is checked
as it's reached; the first mismatch stops playback with the tick, the
stream indexes and the 1 MiB RAM chunk where the runs part (the `lockstep`
comparison already reports that).

What works without touching the guest:

- **Pause.** The host pause (Cmd+P) blocks the guest thread; the logical
  clock doesn't move. Nothing new.
- **Slow motion and fast forward.** The pacer only decides how long the
  host waits, never what the guest does. Scaling its ratio (0.25x to 1x)
  gives slow motion; `PacePolicy::Unpaced` gives fast forward at whatever
  the host can do. The guest runs identically either way. While the game
  is below 30 fps, fast forward isn't fast.
- **Restart** from the beginning.
- **Screenshots** at native resolution from the present hook, and at a
  higher internal resolution once [high-resolution.md](../graphics/high-resolution.md)
  lands, since internal resolution is host-side.
- **Clips**: export a tick range to PNGs or MP4 by replaying headless
  through `GuestHost::headless` with a frame sink. MP4 encoding uses the
  user's own ffmpeg, as `h2 movies` does. With the high-fps mode, a clip
  can be 60 fps, because host interpolation doesn't touch the guest.
- **Rewind** only by restarting and running forward, unless the machine
  can be forked (below).

**In-memory machine forks** (later, optional). Rewind within a session by
keeping copies of the whole machine every N seconds while playing: guest
RAM (64 MiB), each thread's CPU state, the kernel's scheduler, timers and
objects, VFS handles, the D3D device model, audio voices and the clock
(`Clock::resume` exists). The runner keeps host stacks bounded and guest
stacks in guest RAM (LIFTER.md), so a fork at a scheduler boundary is
possible in principle. Two things stand in the way: Rust continuations
(saved closures for overrides that call guest code) can't be cloned, and
every HLE crate would need a snapshot method. Host GPU caches would be
dropped and rebuilt from guest memory. It's a big, cross-cutting job
(estimate below). Engine-level keyframes give rewind more cheaply, so
forks come last, if at all.

### Theater playback (engine level)

Boot the same map with the recorded options, then:

1. **Restore a keyframe** at a tick boundary: write the arena from the
   film, then run the engine's own post-load work. The safest route is to
   go through the engine's revert path: present the keyframe as the N:
   checkpoint slot (`0x123f60`) or as a full restore (`0x124380`). Those
   paths rebuild whatever the arena doesn't hold (caches, physics world,
   sound) the way the game does after a revert.
2. **Feed actions per tick.** Override the action source so each player's
   control record for tick *t* comes from `ACTN`. Live pads and the
   network don't drive the simulation. In local mode, the session stays
   single-machine. For a networked film, the restored roster has remote
   players with no session behind them; whether the engine accepts that
   (Halo 3 plays films this way) is the main open question for networked
   Theater.
3. **Check the simulation hash** every 30 ticks against `HASH`. On a
   mismatch, re-restore at the next keyframe and say so in the UI ("film
   drifted at 03:12, resynced"). That keeps films watchable across a
   rebuild, at the cost of jumps.

Because actions are keyed by tick, rendering is free to change: a frame
that takes longer just runs two ticks next time, each with its own
recorded action.

**Seeking.** Restore the nearest keyframe at or before the target, then
simulate forward with rendering skipped: a host override returns early
from the frame's render and present step while seeking. On disk,
keyframes are 30 s apart; during playback, keep in-memory ones every 2-5 s
of watched film. The latency is restore time plus (distance / simulation
rate). The simulation-only rate isn't measured; at 5-10 times real time,
a seek is 1-6 s from a disk keyframe and well under a second from an
in-memory one.

**Speed.** Use the game's own speed field: game time `+0x0c` scales the
tick budget (`0x00146660`). Speed 0 pauses the simulation while the camera
keeps running (Halo 3's paused free camera); 0.25-0.5 is slow motion; 2-4
is fast forward, within the budget's per-frame tick cap. Speed only changes
how many ticks a frame runs, not what a tick does. Writing it is safe in
playback because the simulation hash is checked per tick, not per frame.
Exact playback keeps using the pacer instead.

### Free camera

**Why not a host-side camera.** The high-fps design can render a frame
again with a different camera, by substituting view constants in the
recorded draws. That's fine for an in-between camera a few centimetres
from the real one. It isn't a free camera: the guest has already culled
the frame for its own camera (BSP cluster visibility, object LOD, the
portals the player can see), so a host camera that turns around finds
nothing drawn behind it. A host-side orbit close to the player is the most
it can do. It's usable as a cheap "nudge" view in exact playback, and
that's all.

**Use the guest's director.** In engine-level playback, put the viewing
local player's director record (`0x004E8C44 + i*0x140`) into the flying
camera, or orbiting or following, and drive its position and orientation
from host input each frame. The guest then culls, sorts and renders for
that camera. This is what Halo 3's Theater looks like from the outside.
The director runs per frame outside the tick, and its records are outside
the arena, so this can't reach the simulation hash. Research needed:

- Confirm `0x0023d090` is the flying camera, and what its flag
  `0x0051EC10` gates. Find the orbiting camera's update function and the
  director setter that installs each kind (`0x00155380`, `0x00155d80` and
  `0x00155dd0` reference the camera functions).
- Find the camera record layout (position, forward, up, field of view) the
  flying camera reads, so the host can write it.
- Sound follows the camera (the listener). That's fine and probably
  wanted.
- Things the guest might do per frame from the camera that write the
  arena: effects spawned by distance, object activation, sound
  occlusion. The simulation hash finds them; each one is either excluded
  as view state or kept out with a guard.

**HUD.** Hide it in free camera (the UI layer `0x00147af1` can be skipped
host-side), and show the followed player's HUD in first person.

**Switching POV.** Following and orbiting cameras take a target unit, so
"watch player k" is a director target change. A first-person view of a
non-local player needs the first-person camera (`0x0023c110`) and the
first-person weapon render pointed at another unit, without remapping the
local player (the local-player map is simulation input). That's the
hardest camera; it goes after free and third-person cameras.

**Exact playback and cameras.** Machine-level films get only the
host-side nudge. "Watch this client film in Theater" means converting it:
an offline exact replay that writes `HASH` and `KEYF` (and `ACTN` where
the mode allows), then engine-level playback from those.

### Presentation

The guest UI kit builds the film browser and detail page. The desktop
compositor draws the transport, roster, camera choices and clip marks
during playback. The host captures playback controls before they reach
the recorded simulation. See [In-game Theater](#in-game-theater) for the
controls and current rendering path.

## 4. Event and spectator view

"Event-style theater" means watching a game you aren't playing in, live
or after, with an observer camera and broadcast overlays (player list,
scores, kill feed). Three options, smallest first:

**A. Web event view (no video).** [local: live-telemetry.md,
live-gamestate.md, live.md] Live telemetry already observes the retail
game state: match start with roster, a frame every second tick (up to
15 Hz) with player positions, kills with the victim position, scores, and
the final scoreboard, streamed to the Live server's `/v1/stream`
WebSocket. A web page on the server can show a live scoreboard, a kill
feed and a top-down map with player dots (live-gamestate.md was written
for exactly that), in the Bungie.net stats style the project already uses
for its web side. It works for every Live game today, live or after the
fact, and needs no rendering or determinism. This is the smallest event
mode, and it's useful on its own.

**B. Live film stream (delayed spectating).** Once engine-level Theater
works: the host streams its film (`ACTN`, `HASH`, a keyframe every 10 s)
to the Live server, which relays it to spectators. A spectator's client
opens the growing film: it restores the latest keyframe and follows the
action stream, always some seconds behind the live game. That gives the
full Theater (free camera, any POV, overlays from section 3) for any
number of spectators. It never adds a player to the session and never
changes what the players' consoles send. Add a deliberate 30-120 s delay
so a spectator can't call out positions to a player (standard practice
at tournaments). It needs the networked-host questions from section 2
answered first. Bandwidth is the action stream plus keyframes, well under
50 KB/s.

**C. A spectator seat in the session.** Halo 2 has no spectator role. An
observer would join as a player (taking one of 16 slots, spawning, and
appearing in the roster) unless the game were changed, and keeping it out
of play would change the simulation. Not recommended.

The "observer" code in the XBE (`0x0016f280`) is camera smoothing for the
local view, not a spectator mode. [local: engine-map.md, "likely"]

## 5. Risks, prerequisites, phases

### Prerequisites

1. **Determinism harness** (high-framerate.md phase 1, built):
   per-tick RAM hash, tick-keyed pad recording, run comparison. Theater
   reuses it directly. One naming note: that crate is called `lockstep`,
   which is easy to confuse with network lockstep in this document.
2. **Steady 30 fps** in levels ([gameplay-budget.md](../perf/notes/gameplay-budget.md)).
   Below that, playback is slow motion and fast forward is slower than
   real time. Exact playback works at any speed; it's just slow.
3. **Multiplayer reachable** (a local multiplayer game, then system link)
   for the multiplayer phases.

### Risks, most likely first

1. **Restore-then-run may differ from running straight through.** If the
   engine's revert rebuilds the physics world or caches differently from a
   continuous run (contact caches, warm starts, cache residency feeding
   back into the simulation), engine-level playback drifts from the
   recording after every restore. Halo's campaign revert is designed to be
   *playable*, not bit-identical to an unbroken run. Phase 2's experiment
   measures exactly this before anything else is built on it. If it fails,
   the fallback is a restore of more than the arena (the arena plus the
   physics heap), or machine forks.
2. **Networked hosts.** If the host drives remote players from accepted
   client positions rather than actions, networked engine films need more
   streams and more research. Client films stay exact-only either way.
3. **Build fragility.** Exact films only play on the build that recorded
   them. This project rebuilds daily, so exact films are short-lived; that
   risk is the reason for engine-level keyframes and resync. A change to
   floating-point results (the x87/SSE work, fast trig) can also break
   engine-level films; resync at keyframes contains the damage.
4. **Camera side effects.** If the guest does arena writes per frame from
   the camera, the free camera can perturb the simulation. The simulation
   hash catches it; each case needs a guard.
5. **Seek speed** depends on the simulation-only rate, which isn't
   measured.
6. **Nondeterminism not yet found in the runtime** (host randomness that
   isn't tapped, a host-timed callback, a wall-clock read). The harness's
   "same run twice" control finds these first.
7. **Size** if keyframes compress worse than guessed; then lengthen the
   on-disk interval.

### Phases

Rough sizes for one person who knows the runtime, after the
prerequisites.

| phase | work | size | exit test |
|---|---|---|---|
| 1 | **Exact films, local games.** Read `G+0x0c` in a local game to settle the mode. `.h2film` writer and reader (`HEAD`, `DATA`, `INPT`, `RAND`, `HASH`, `INDX`); tap device changes and `XNetRandom`; start-state manifest; `h2 play --film out.h2film` and `h2 theater film.h2film`; pause, pacer slow motion and fast forward, restart; desync report; screenshot | 1.5-2 weeks | a recorded Heretic run replays with every `HASH` matching, twice; a film refuses to open on another image stamp |
| 2 | **Clips** (headless re-render of a tick range to PNG/MP4 through the user's ffmpeg; 60 fps with the high-fps mode, high resolution when that lands) and the host overlay's transport bar | 1-1.5 weeks | an exported clip's frames match the frames presented in a live replay of the same ticks |
| 3 | **Keyframe experiment, then engine-level playback.** Simulation hash per allocation; keyframe capture; restore through the engine's revert; find and override the action source; seek; resync | 1 week experiment, then 2-4 weeks | from a keyframe at tick k, simulation hashes match the recording for 3000+ ticks in a campaign level and a local multiplayer game |
| 4 | **Cameras**: flying, follow and orbit through the director; host camera control; HUD hide; POV switch; then first person of another player | 2-4 weeks (+1-2 for first person) | the simulation hash is unchanged with the free camera moving all film; the camera can look anywhere |
| 5 | **Networked films**: tap receives and Live results; exact client films; host films with engine layer after the remote-player research | 3-5 weeks | a system link client film replays exactly; a host film plays in Theater with matching hashes |
| 6a | **Web event view** from telemetry (scoreboard, kill feed, top-down map) | 1-2 weeks, mostly web | a Live game shows live on the page |
| 6b | **Live film stream** to spectators through the Live server, with a delay | 3-4 weeks after 3 and 5 | a spectator joins mid-game and follows within the delay |
| 7 | (optional) in-memory machine forks for rewinding exact films | 4-8 weeks | fork, run, restore: identical RAM and hash sequence |

Total: roughly 15-25 weeks for phases 1-6, plus 4-8 if forks are wanted.
Phase 6a doesn't depend on the others and can run in parallel at any time.

### Smallest first slice worth building

**Phase 1: exact films of local games.** It's mostly the high-fps
harness plus a file format and a command. It gives a real feature (watch
your game back, pause, slow motion, fast forward, screenshot). It's also
the strongest determinism test the project can have: every film is a
long-run replay test, and a desync is a runtime bug report with a tick
and a RAM chunk attached. Everything after it (clips, keyframes, cameras,
spectating) reuses its format and checks.

The **first experiment** to run after it, before any camera work: restore
a keyframe at tick k, feed recorded actions, compare simulation hashes
over a few thousand ticks. It decides whether the free camera rides on
engine-level playback (the cheap route) or needs machine forks (the
expensive one).

## Research points

| address | what | label |
|---|---|---|
| `0x00123b30`, `0x00123dd0` | game-state arena (`0x3fe000`) and named pools | local, confirmed (engine-map.md) |
| `0x2151f0`, `0x123f60`, `0x124380` | checkpoint write, revert to slot, full restore | local, tested (checkpoints.md) |
| `0x00137fe0`, `0x00146660`, game time `+0x0c` | tick, budget, speed | local, confirmed |
| `0x001870a0`, `0x0014d3a0` | player-control preparation; players-before (action consumption) | local, likely; the action record is unfound |
| `0x004E8C44`, stride `0x140`, `+0xC` | director record per local player; camera update function | local, read for this page |
| `0x00155760` | camera kind classifier | local, read for this page |
| `0x0023c110`, `0x0023cbb0`, `0x0016c840` | first-person, following, scripted camera updates | local (engine-map.md); `0x0023cbb0` unverified |
| `0x0023d090`, `0x0051EC10` | candidate flying (debug) camera and its gate flag | guess |
| `0x00155380`, `0x00155d80`, `0x00155dd0` | director functions that install camera kinds | local, unread |
| `0x00468cfc` | director camera kind names | local |
| `0x00468c64`, `G+0x0c` | simulation mode names; mode byte | local |
| `0x00081780` | simulation view and distributed view arrays | local |
| `0x000b2b36..0x000b2c0e` | synchronous message serializers | local |
| `0x0046715c` | session end reasons, including film ended | local; the producer is unfound |
| `0x001fb30c` | "recorded animations" game-state allocation | local |
| `0x0016f280` | observer (camera smoothing) | local, likely |
| `0x0015ce70`, `0x001389c0` | kill and game-end hooks for markers and scoreboards | local, confirmed (live-gamestate.md) |
