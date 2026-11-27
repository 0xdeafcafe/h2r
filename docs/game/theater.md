# Theater: saved films, free camera and spectating

The design of the film and its player is
[theater-design.md](theater-design.md); the files are
[the film format](film-format.md). This page has the selector, the
controls, the web tier and the research the design rests on.

This is the design for a Halo 3-style Theater in h2recomp: record a game
as a film, then watch it again with pause, fast forward, rewind, a free
camera, another player's view, clips and screenshots. It also covers the
"event" side: watching a game live as a spectator, with a player list and
scoreboard.

In short:

- **A film is the game options and the simulation updates.** Every tick
  of every game runs from one update record the engine builds, and a game
  starts from one options block. A film is those two things, and playback
  is the game simulating them again in the running game.
- **A free camera and rewind need a second layer that Halo itself
  provides.** Halo 2 keeps its whole simulation in one fixed 4 MiB
  game-state block (the block its checkpoints save), and its camera
  director lives outside that block, in static data. The player keeps
  compressed copies of the block as it plays, and rewinds by restoring
  one and feeding the updates in, so the camera and the rendering can
  change without the simulation noticing. This is how Halo 3's Theater
  is generally described.
- **Halo 2 multiplayer is not lockstep.** The task brief assumed it was.
  The retail XBE has both a "synchronous" and a "distributed" simulation
  mode, and community sources say multiplayer is host-authoritative and
  distributed, with lockstep only for campaign and co-op. A networked
  *client* is sent state, not updates, so its game can't be simulated
  again from a film. The host's can, and the host's film is the game's.
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

### Tier A: Web Theater

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
  The recorded feature switches preserve the
  [replication admission](ai-networking.md#creature-entity) plan during
  offline playback.
- `DATA` records directories and SHA-256 digests under the guest's
  `disk/` and `metadata/`. Small files are embedded, up to 1 MiB each
  and 16 MiB total. For larger files, supply an unchanged initial copy
  with `--data DIR`. Every playback and seek restores into a fresh
  private directory. The film supplies no host executable or module path.
- No HLE call, network result, random draw, lobby call or arena hash is
  in the container: a film plays from its game start and checks the
  digests in its film file ([theater-design.md](theater-design.md#playback)).
- All tags use four bytes, a little-endian compressed length and one
  zstd frame containing a v1 JSON payload. `INDX` ends a finalized
  file. A scan rebuilds the offsets after interruption and drops an
  incomplete trailing chunk. Playback of a prefix ends at its last
  complete checkpoint. `KEYF` is reserved.

`film::Library::list` shows the 25 newest films and any older saved
films. A recorded film is two files, `<id>.h2film` and `<id>.h2stats`;
the row comes from the film file's header and game start, with the map,
players and scoreboard from a sealed stats container of the same film
when there is one (`LibraryEntry::read`), and no tick chunk is read. An
unsealed film a recorder still holds (`film::file::hold`) is a game in
progress and is not listed.
Save and Unsave manage pins beside the film. Delete resolves a film by
its header ID and removes its pin, the film file and its stats
container. The host UI uses `H2_FILM_DIR` or `<data>/films`.

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
`hashes matched through tick T`.

```sh
h2 theater play "$film_run/match.h2film" --disc "$H2_DISC" --out "$H2_IMAGE" --socket "$film_run/play.sock" --render-every 0 --speed 1
```

**Experiment 2, rendered.** Run the same file and image in a window with
every world frame rendered. Compare the ending tick or retain the first
divergence and its expected and actual digest. A mismatch in
experiment 1 must be resolved before attributing one here to render RNG.

```sh
h2 theater play "$film_run/match.h2film" --disc "$H2_DISC" --out "$H2_IMAGE" --socket "$film_run/play.sock" --render-every 1 --speed 1
```

**Experiment 3, another window.** A local game renders on its own random
stream and its guest camera follows the window
([Determinism](../graphics/high-resolution.md#determinism)), so a film
recorded in one window replays in any other. Record offline in a wide
window and play the film back at 4:3, and the other way round; each
playback must report `hashes matched through tick T`.

```sh
H2_RESOLUTION=2922x1662 h2 run "$H2_DISC" --out "$H2_IMAGE" --data "$film_run/wide" --film "$film_run/wide.h2film"
H2_RESOLUTION=640x480 h2 theater play "$film_run/wide.h2film" --disc "$H2_DISC" --out "$H2_IMAGE" --socket "$film_run/play.sock" --render-every 1 --speed 1
```

During either run the agent exposes the [Theater transport](../runtime/agent-api.md#theater-transport).
`theater.state` reports the seek route and remaining ticks. A seek behind
the tick shown loads the latest [session ring](theater-design.md#the-session-ring)
entry at or before its target and runs forward from it (`theater.keyframes`
lists the entries); with no entry for it, it queues the film's game again
in the same process and runs forward from its start. A seek ahead runs
forward from the tick shown. Fast-forward
checks the film's digests and runs without host pacing. The coordinator
should also seek backwards across a received packet while paused and
resume. `h2 film stat FILE` lists a stats container's stored keyframes
(given a film file, its `.h2stats`),
and `h2 film stat --fields FILE...` the change rate and stored bytes of
each action field. The
[measurement procedure](notes/theater-seek.md) uses both tools.

At match end the host uploads the sealed recording under the lease's
canonical game ID. Live checks `telemetry.ingest` and lease ownership.
Small uploads fit in one request. Larger films use numbered parts and
continuations, up to 2 GiB per film. [Upload protocol](../live/live-api.md#theater)
The supervisor gives the child time to finish uploading before reporting
the lease ended. The upload and both hash experiments still need a real
dedicated match.

### In-game Theater

Select **THEATER** between **System Link** and **Settings** on the main menu. The
main menu and **Game Setup → Theater** in the post-game party lobby open
the same library for the controller that selects it. Left and Right switch between
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

Press A on a film to open its lobby. Its actions (Watch, Save, Delete,
Share, Copy Link) and facts are on the left: clip title, map, game, mode,
date, length and result, where known. The map picture and the player list
are on the right. The list shows the scoreboard, else the film's own
roster, ten players a page in two columns, with each player's team name
(else score) beside the name; team games sort by team. Left and Right page
through longer lists. When no players are known the area reads
"Player list unavailable." An unavailable film has no **Watch** action and
shows the reason in the status line at the bottom left, including when a film
predates a breaking simulation change. A summary without a recording also
shows its reason there. **Watch** appears for playable films and host films
awaiting their first download.

Recent combines the newest 25 unsaved local games and the signed-in Live
account's unsaved games. `h2 play` records each game into the local library as a film
of its own, so each game played on this console appears there and
opens at its own start; the account's archive adds
games whose dedicated host recorded a film
([Recording every game](film-format.md#recording-every-game) lists what
each kind of game shows). Saved includes local and account pins. Clips includes local
in/out ranges and clips authored by the account. Use **Save** or **Unsave**
to move the film between Recent and Saved. Local films and authored clips offer **Delete** with a
second press to confirm. The selector takes its data from the library
and the Live listing. Older recordings and archives can leave some fields
blank.

**Watch** plays a film that has a game start in the running game: it
queues the film's game as the next game, the game loads the film's map, and
the film's updates play in it (`theater.watch`, [theater-design.md](theater-design.md#starting-a-film-in-the-running-game)).
End Film returns to the film page. No second process starts. A film
recorded before game starts does not open and says why. The library
checks the cached header for an account film and checks the exact
recording after the first download. Playback needs an installed game
image of the film's XBE with the film's simulation contract; it reads
nothing of the recorder's disk. An archive with only telemetry still has
a film page and scoreboard. A recording's scoreboard comes from the
adjacent `.h2stats` file.

Rendered host playback opens in **Fly cam**. Local playback opens in the
recorded retail view. **Orbit** circles the selected
player and **Follow** looks over their shoulder. **First person** uses
a selected local player's recorded viewport or the replayed host unit's
position and aim; the guest's look follows the recorded action's facing on each
update, so the view looks where the player looked. A rendered film follows the selected player when they
have no recorded viewport. A playerless host has no recorded first-person weapon
view or local HUD. Roster controls
cycle round the available players. The **Default** camera restores the
full local frame or selects the first available player in a version 3 host film.
The camera, its field of view and HUD visibility carry across a seek.

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
| Toggle game HUD | L3 + R3 | J |
| Save a screenshot | D-pad Up | F12 |
| Step one tick while paused | D-pad Down | `.` |
| Open the pause menu | Start | Escape |
| Cancel an active jump | B | Escape |
| Choose a pause menu item | D-pad Up/Down, A | Up/Down, Enter |

The playback bar shows the current and total time, requested and achieved
speed, camera and subject. Its timeline uses the adjacent stats file for
kills.
When the stats file is missing, playback still works and the pause menu
shows that no match stats are available. During a jump the rail highlights
progress toward the target. Completed clip ranges appear in Clips and loop
when watched.

The pause menu offers **Resume**, **Restart Film**, **Change Player**,
**Camera**, **Take Screenshot**, **Mark Clip**, **Export Clip**, **Save
Film**, **Share** and **End Film**. Press A or Enter on a choice row to open
its picker. B or Escape dismisses a picker and returns to the same row.
The playback window captures these inputs before the replayed guest sees
them. Screenshots omit the host pages and are written under
`<data>/films/screenshots`.

Skip taps that arrive within 250 ms of one another accumulate into one
jump. Cancelling a jump returns to its departure tick and holds there.
While paused the game runs no tick and keeps drawing frames, so a camera
change shows at once.

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

Offline samples are `theater.films`, `theater.film`, `theater.playback`
and `theater.pause-menu` in `h2 ui preview`. The playback samples include
jumping, divergence and unavailable states. The browser includes empty Recent,
Saved, Clips and filtered states. Film details include a summary without
a recording, a roster-only film and a two-page roster with names only. The main-menu selection
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
| `theater-film` | a `film` crate: `.h2film` writer/reader (chunks above), the host recorder taps (`NETR`, `RAND`, `HASH`) behind `H2_FILM_OUT` / `h2 host run --film`, upload at match end, `h2 theater play` exact playback and the Theater agent methods |
| `theater-ui` | the in-game Theater row, Films screen, film page and playback overlay, against the `film` crate's `Library` API and the agent methods |

## Pages

| Page | What |
|---|---|
| [Theater research: Halo 3, recording, playback and risks](theater-research.md) | How Halo 3 Theater works and whether Halo 2 fits, recording, playback, event view, risks and research points. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## 1. How Halo 3's Theater works, and whether Halo 2 fits

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#1-how-halo-3s-theater-works-and-whether-halo-2-fits).

## 2. Recording

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#2-recording).

### What the recorder adds (engine level)

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#what-the-recorder-adds-engine-level).

## 3. Playback

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#3-playback).

## 4. Event and spectator view

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#4-event-and-spectator-view).

## 5. Risks, prerequisites, phases

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#5-risks-prerequisites-phases).

## Research points

See [Theater research: Halo 3, recording, playback and risks](theater-research.md#research-points).
