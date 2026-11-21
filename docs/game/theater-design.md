# Theater: the full in-game player

Status: Design. The parts under "What it builds on" are built; everything else on this page is not.
Code: `crates/extensions/game/src/theater.rs` (`Session`), `crates/extensions/game/src/theater/actions.rs` (the per-tick inputs), `crates/live/film/src/tape.rs` (the HLE tape), `crates/apps/cli/src/theater_cmd.rs` (the player process), `crates/live/film` (the container). Tests: `crates/extensions/game/src/theater/tests.rs`, `crates/live/film/src/tests.rs`.
Switch: none.

This is the design for a Halo 3-style Theater inside the game: play a
film, pause, rewind, jump to any point, change speed, fly a camera, follow
any player, hide the HUD, take screenshots and clips, and share a film
through Live so a link on live.forbes.red opens it in the game. One
principle organises it: **stats is the summary, the film is the
simulation.** A film is what Halo 3's is: the state a simulation starts
from and every seat's input on every tick. The game simulates it again to
show it. It is small, and it plays until the simulation changes. The page
sits on the recorder and player that [theater.md](theater.md) and
[film-format.md](film-format.md) describe;
[input-film.md](input-film.md) says what a film holds and how it is
encoded. The rule
that must hold: no Theater control changes what the replayed simulation
computes, and every tick proves it with the film's recorded digests.

Evidence labels: **Code** (read in this repository, path given), **Test**
(a named test), **E** (read from the retail XBE, as recorded in the linked
page), **S** (supposed, with what would verify it), **Estimate**
(arithmetic from cited numbers), **Design** (our choice).

## Stats and film

Two separate things are recorded, and neither holds the other's data.

| | Stats stream | Film |
|---|---|---|
| Answers | what happened | how to make it happen again |
| Holds | events (kills, medals, scores, joins, objectives), the scoreboard and carnage report, positions and poses for the web viewer, heat maps | the seed, each seat's input on each tick, seat joins and leaves, a digest for each tick, and only those host answers that replay cannot do without |
| Does not hold | inputs, digests | poses, events, stats |
| Read by | Live, the web, the stats ledger, on any build; nothing is simulated | the engine only, on a build whose simulation contract matches |
| Made by | the host of a Live game as it plays; any machine, by playing a film | every machine, for the simulation that machine ran |
| After a breaking simulation change | still reads | refused |

Watching a Live session online is the web viewer drawing the stats stream
([web-viewer.md](../live/web-viewer.md)). It is not a film and not
Theater: no engine runs, and the game never draws it. Theater is the
game simulating a film.

**Where the stats stream is.** As built it is spread over three places
([film-format.md](film-format.md), [live-api.md](../live/live-api.md#theater),
[live-telemetry.md](../live/live-telemetry.md#server-ingest)):

1. The public section of the `.h2film` container: the front JSON with the
   summary and scoreboard, the `TICK` chunks' event records (tag 2), pose
   records (tag 3), markers (tag 5) and clock (tag 6), and the `CIDX`
   directory. Its private twins `IPRV` and `PSUM` hold the original
   stats-import events. Live serves the public ranges
   (`GET /v1/games/{game}/film` with a range) and the web viewer reads
   nothing else.
2. Live telemetry: the frames and events a host publishes during play.
   A tag 3 supplement is one of its packed frames.
3. The stats ledger, which Live fills from the canonical event records
   once per game.

The same container also holds the film: the inputs (tag 1) and digests
(tag 4) inside the public time chunks, and the private machine section.

**The split.** The recorder writes two files.

- **The stats stream** keeps the container it has (`H2BLAM3!`, the front,
  `TICK`, `CIDX`, `IPRV`, `PSUM`) and loses tag 1, tag 4 and the machine
  section. The web viewer's reader, Live's public-range checks and the
  ledger import read the same records, so none of them changes. It is
  `<id>.h2stats` in the library and `stats/<game>` on Live.
- **The film** is a new, smaller file ([input-film.md](input-film.md)),
  `<id>.h2film` in the library and `films/<game>` on Live. Live stores
  it and serves it without inflating a chunk.
- **Routes keep their meaning.** A ranged `GET /v1/games/{game}/film`
  reads the stats stream, which is what the web viewer asks for. The
  same path without a range streams the film to a signed-in account, in
  place of the exact recording. A host uploads both through the existing
  numbered parts.
- **A local game** writes both files. The stats file beside a film is a
  companion: the film page reads its scoreboard, and playing the film
  writes it again if it is missing.
- **A container that holds both** is read as it is, by Live and by the
  game, until a breaking simulation change refuses its film. Nothing is
  converted.

## What the player gets

| Feature | Behaviour |
|---|---|
| Play a film | From the main menu's Theater, a film page's **Watch**, or a link. The first watch of a film on a machine that did not record it simulates from the start; **Prepare** does that in the background. |
| Pause | Holds the simulation. The camera, HUD toggle and screenshots keep working while paused. |
| Speed | 0.25×, 0.5×, 1×, 2×, 4×. The bar shows the speed reached when the machine cannot keep up with the one asked for. |
| Rewind and jump | Tap for 5 s, hold for 30 s, or pick a point on the timeline. A jump inside the last minute watched lands in about two seconds; any other jump shows a progress bar and can be cancelled. |
| Step | One tick forward while paused. One tick back is a jump to the tick before. |
| Cameras | Recorded view, first person for a chosen player, follow, orbit, free flight. |
| HUD | Game HUD on or off; Theater's own bar on or off. |
| Screenshot | A clean frame without Theater's bar, at the window's internal resolution or a multiple of it. |
| Clip | Mark in and out. A clip is a range of the film; **Export** renders it to a video file offline. |
| Share | **Share** on a film or clip uploads the film and its stats stream once and gives a link. The link opens the web viewer on the stats; its **Open in Halo 2** button opens the film, range and camera in the game. |
| Films from other builds | Played when the simulation contract matches, refused with the reason otherwise. The film page says which before the player presses Watch. |

## What it builds on

| Part | State | Where |
|---|---|---|
| The `.h2film` container: time chunks, the private machine section, per-tick gameplay digests, arena hashes every 30 ticks | Built | [film-format.md](film-format.md), `crates/live/film` |
| Per-tick inputs: the 92-byte retail action of every seat, read at the simulation-update boundary after pads, remote arrivals and phantom actions are resolved, and written back there in playback | Built | `capture` and `apply` in `crates/extensions/game/src/theater/actions.rs`; `SIM_UPDATE_APPLY` in `crates/hooking/guest-layout/src/points.rs` |
| The HLE tape: every host answer the guest received, on one call index | Built | `crates/live/film/src/tape.rs` (`Call`, `Playback::call`); `impl Tap for Session` in `theater.rs` |
| Capture points every 150 ticks and local sidecar snapshots every sixth point | Recorder and player use the snapshot build key and recorded device boundaries | `crates/extensions/game/src/theater/record.rs`, `crates/live/film/src/sidecar.rs` |
| Machine snapshots: guest RAM, kernel, D3D, audio, XAPI, feature sections, a RAM hash | Built, tested on synthetic machines | [snapshots.md](../runtime/snapshots.md) |
| The player: an isolated child process per film; a seek ends the child and starts another | Built | `crates/apps/cli/src/theater_cmd.rs` (`run`, the restart loop) |
| A film without a usable keyframe plays from boot | Built | `EngineReplay::open` in `crates/extensions/game/src/theater/replay.rs` (`ImageMatch::FromBoot`) |
| Transport over the agent API: `theater.play`, `state`, `pause`, `speed`, `seek`, `camera` | Built | `crates/extensions/agent-proto/src/theater.rs` |
| Cameras: a view replaced in the guest's renderer with a private random seed; a separate GPU view for a playerless host film | Built | `crates/extensions/game/src/theater/legacy_camera.rs` (`Seeds`), `host_view.rs` |
| The film selector and film page in the game's own menus | Built | [theater.md](theater.md#in-game-theater) |
| Compatibility: XBE digest, guest image contract, `h2-sim/1`, the hook digest | Built | `crates/extensions/game/src/theater/compatibility.rs` (`hook_digest`), [film-format.md](film-format.md#determinism-and-verification) |
| Live: host film upload, film lists, pins, clips as tick ranges, public range reads for the web viewer | Built | [live-api.md](../live/live-api.md#theater) |
| Engine-level playback (a film played without its machine section) and arena keyframes | Not built | "Before and after the sim rewrite"; [sim-rewrite.md](../engine/sim-rewrite.md) section 7 (`sim_state::Keyframe`) |

Five gaps follow from that table, and the design is their answers:

1. **A film is mostly keyframes, and carries the stats.** At the
   planning rate a film gains 5.75 MiB of guest RAM a minute, which is
   game data and cannot leave the machine, beside a few kilobytes of
   input. The poses and events in the same file are read by the web and
   never by the engine.
2. **A seek costs a process start.** Every jump boots the image again
   (`theater_cmd.rs`: "Each seek starts a new process so the game, HLE
   allocators and feature statics all restart").
3. **The replay depends on more than inputs.** Playback answers every HLE
   call from the tape, so a film carries received packets, account
   observations and pad reads encoded as JSON.
4. **A film stops being playable when the build changes.** The hook digest
   changes with every hook added or moved. This is accepted: a film plays
   until the next breaking change and is then refused.
5. **Only a leased dedicated host can upload a film**, and only a
   participant can download the exact recording.

## The film

A film is a seed, an input stream and a digest stream, and nothing a
player can rebuild. Until the engine film is proven it also carries a
machine part, which is the host answers replay cannot yet do without.

| Part | Holds | Needed by |
|---|---|---|
| **Seed** | map identity and digest, variant bytes, game options, the random seed, the first tick's seats, the simulation contract | every player |
| **Tick stream** | each seat's 92-byte action on each tick, seat records, tick flags, a digest per tick | every player |
| **Machine part** | boot facts (`HEAD`), startup decisions (`BOOT`, `MOVI`), the initial guest disk (`DATA`), the HLE tape, capture-point marks, arena hashes with the call cursor (`HASH`) | the player until the engine film is proven for that kind of game; absent after |

A film holds no keyframe. Keyframes are a cache beside the film on the
machine that made them ("Sidecar keyframes"). The whole film is a file on
disk, recorded there or fetched from Live into the library, and the
player streams through it a chunk at a time
([input-film.md](input-film.md#memory)).

[input-film.md](input-film.md) has the rest: what the seed is before and
after the engine film, every host answer the replay depends on and
whether it is needed, derived or cut, how a join is recorded, the Halo 3
reference, the encoding, and the memory to record and play.

### Size

[input-film.md](input-film.md#size-step-by-step) has the steps and
the assumptions. All of it is **Estimate** until slice 13's measurement.

| A 10-minute game | Film | Machine part, while needed | The container with keyframes and poses | Halo 3, same length |
|---|---:|---:|---:|---:|
| One player, offline | about 0.12 MB | 0.03 to 0.1 MB of pads, plus the profile | 57.5 MiB of keyframes, 0.1 to 0.2 MB of poses | 0.9 to 1.8 MB (campaign) |
| 16 players, the host's film | about 0.5 MB | 3 to 15 MB of packets | 57.5 MiB of keyframes, 1.4 to 3.4 MB of poses, the same packets | 7.2 MB |

### Live matches

| Machine | Records | Kept | Used for |
|---|---|---|---|
| The host | the film of the authoritative simulation: every seat's accepted action is in its update (`actions.rs`); and the stats stream | streamed to Live | the stats for everyone; the film for Theater on other machines |
| Each player's machine | the same film: the host forwards every seat's actions to each client as it gathers them | `<data>/films/`, never uploaded | Theater on that machine: the whole game, opened on the player's own seat |

A Live client is not sent other seats' inputs by the game: Halo 2's
multiplayer is host-authoritative and replicates state
([theater.md](theater.md#fit)). The host knows every seat's action, so it
sends them beside the game: the film's own tick chunks, forwarded to each
client as they close ([input-film.md](input-film.md#the-action-side-stream)).
A client's film is then the host's film, byte for byte, with no network
tape, at about 6 kbit/s for 16 seats (**Estimate**). Theater plays it
with the player's seat selected, which gives their view and HUD of the
authoritative game; it does not show what their own machine predicted.

A player who joins a running game starts from a start point: the
compressed arena and extension state at the join tick, about 0.26 MB,
authenticated by the host and checked against the film
([input-film.md](input-film.md#start-points)). It is never guest RAM.

Every game gets fresh seat and machine identifiers, issued by Live for a
Live game and by the recorder for an offline one. They name nothing
outside that game, so a film needs no scrub.

## How it works

### One player, three ways to reach a tick

The player keeps one process for the whole viewing session where it can.
A request for tick T chooses the cheapest source at or before T, in this
order:

| Source | What it is | Made by | Restore | Replay distance |
|---|---|---|---|---|
| **Ring** | Keyframes held in the player's memory for the last minute watched | the player, as it plays | in the running process | at most 150 ticks |
| **Sidecar keyframe** | A machine keyframe in the cache beside the film | this machine: the recorder as it records, or the player as it plays | in the running process when the warm rules allow, else a new process | at most 900 ticks |
| **Boot** | The film's seed | nobody | a new process | all of it |

A container recorded with `KEYF` envelopes inside it is read as it is;
its keyframes rank with the sidecar's.

After the restore the player runs forward to T unpaced and unrendered,
checking every recorded hash on the way, then holds or plays as asked.
`theater.state` reports the source, the ticks left and the tick of the
first divergence, so the bar can draw progress and a test can assert the
route taken.

### Capture points

A machine snapshot is not invisible to the guest's devices: capturing the
D3D section drains the push buffer, writes dirty render targets back and
reads visibility counts. The recorder marks every attempt as a
`d3d_barrier` with its scheduler slice, and linear playback performs the
same device work at the same slice (`Session::replay_capture_point`,
`film::tape::Playback::d3d_barrier`). A keyframe taken anywhere else would
make the playback differ from the recording.

The player captures only at slices the film marks. Every **150 ticks**
(5 s), the recorder looks for an unblocked boundary during the first
30 ticks of the interval. It runs the D3D section's device work and
writes a mark with the outcome after flushing the call batch. Every sixth
mark is eligible for a whole-machine snapshot if that device capture
succeeds. The 30 s cadence assumes every interval has an unblocked
boundary. A dedicated host records the marks and omits the sidecar.

Captures take place between two slices, at a boundary the recording
already has. The recorder never asks for an early slice end
(`crates/apps/boot/src/lib.rs`, the loop that calls `try_capture_point`), and
neither does the player, because an early end moves every later slice
([snapshots.md](../runtime/snapshots.md#running-the-probe)).

### The session ring

The ring holds machine keyframes for the stretch of film just watched.

- **Storage.** A page pool: each entry is a table of 16,384 page
  references into a pool of 4 KiB pages counted by use, plus the encoded
  non-RAM sections (`kcore`, `machine`, `d3d`, `dsound`, `audio`,
  `audioout`, `wma`, `xapi`, `features`) and the film cursors (`tick`,
  `slices`, `calls`, `lobby_boundary`, as `film::blam::Keyframe` carries
  them). A capture copies only the pages the guest wrote since the entry
  before, read from a write-watch subscription
  (`crates/runtime/runtime/src/watch.rs`, `Guest::subscribe_writes`).
  Unchanged pages are shared between entries.
- **Policy.** One entry per capture point, the newest 12 kept: a minute
  of film at the 5 s cadence. The pool has a byte cap; when a capture
  would pass it, the oldest entry goes first. The cap is never exceeded
  to keep an entry.
- **Restore.** Copy into live RAM every page that differs from the entry
  (the pages written since the last capture, and the pages whose
  reference differs between the last capture and the target), then load
  the sections in the order `boot::snapshot_glue` uses for `state.load`,
  then reset the film cursors (`Playback::resume_keyframe`).
- **The ring is not in the film** and is dropped when the player leaves.

The ring also gives the paused camera its picture where the guest draws
the frame: moving the camera while paused restores the entry made at the
pause, runs the guest to the next present with the new view, and holds
again. A host film's view is drawn from simulation state by
`host_view.rs` and needs no restore.

### Warm restore and its fallback

A warm restore loads a keyframe into the process that is already playing.
It is allowed when all of these hold; otherwise the player restarts the
child, which always works:

1. The machine is quiescent at the boundary and no part or feature
   reports a blocker (`BootMachine::film_load_blockers`,
   `Machine::quiescent`).
2. The guest disk at the target equals the disk the process holds: the
   disk deltas of the two keyframes are equal. A film whose saves change
   between them (a campaign checkpoint) restarts.
3. The film has not failed a warm check in this session.

The warm check: after any warm restore, the whole-RAM hash
(`lockstep::hash_guest`) must equal the keyframe's `ramhash` before the
first slice, and every digest from there to the target must match. A
mismatch ends the child, restarts it cold at the same target, and turns
warm restores off for that film until the player leaves. The player
shows nothing but a longer bar.

### Sidecar keyframes

Keyframes live beside the library, never in a film:
`<data>/films/cache/<film id>/<build id>.h2keys`, the build id being the
snapshot store's FNV-64 of the build key
([snapshots.md](../runtime/snapshots.md#where-snapshots-live)). The
sidecar uses the film's `KEYF` payload and anchor grouping unchanged, with
its own `TIDX`-shaped directory. `film::blam::TrackReader::seek` reads
film and sidecar keyframes as one ordered list.

- **The recorder writes one** for a film it records for local play: the
  keyframes `try_capture_point` takes go to the sidecar, so the machine that
  played a game can jump in its film at once.
- **The player writes one** as it plays or replays: a keyframe at every
  sixth capture point it passes that the sidecar lacks. A film from
  another machine becomes seekable as far as it has been watched.
  **Prepare** on the film page replays it to the end in the background
  without a window or realtime pacing. The film page gets progress and
  the verified result from `theater.state {id}`.
- A sidecar is rebuilt from the film whenever it is missing. It holds
  game data, is never uploaded, and is removed with the film or when the
  cache passes its cap, least recently watched first. The user setting
  `theater.cache_cap_gib` defaults to 4 GiB.
- A sidecar loads only into the build that wrote it. Another build
  ignores it and writes its own.

### Cameras and the HUD

One camera rig serves every film (`theater::ReplayCamera`): mode, subject
seat, pose, orbit radius and field of view. It survives seeks and is
reported in `theater.state`.

| Film | Picture | Free, follow, orbit | First person | HUD |
|---|---|---|---|---|
| Local (the recorder rendered) | the guest's renderer | the rig replaces the renderer's view argument; the shared random state is swapped for a private seed around the replaced draw (`legacy_camera.rs`, `Seeds`) | a local seat's recorded viewport; any other player gets follow | the recorded HUD, hidden by `hide_hud` |
| Playerless host (the recorder drew nothing) | the independent view (`host_view.rs`): BSP, lightmaps, posed opaque models | the same rig | the unit's eye; no weapon model | none |

A host film's picture gains effects, sky, transparents and a HUD when the
renderer owns the frame and can draw from a presentation snapshot
([framescene.md](../graphics/framescene.md)). Theater does not build a
second renderer for it.

### Screenshots and clips

- **Screenshot.** The shell saves the published game frame before
  Theater's plates are drawn (`crates/apps/app/src/theater_overlay.rs`,
  `screenshot`). A scale above 1 renders one frame at a higher internal
  resolution through the paused-camera path.
- **Clip export.** `theater.clip.export` starts a second, windowless
  player on the same film, seeks to the in mark, and plays to the out
  mark unpaced with a frame sink that receives every presented frame and
  the mixer's tapped audio. Frames go to a PNG sequence, or to the user's
  own ffmpeg as raw video for an MP4, the way `h2 movies` finds and runs
  it (`crates/apps/cli/src/movies_cmd.rs`). The export carries the rig, so
  the clip shows the camera the player chose. The watching session keeps
  running.
- A clip on Live stays a tick range ([film-format.md](film-format.md)); a
  video file is a local export.

### The Theater bar and menus

The bar and the pause menu are pages of the UI builder
([ui-builder.md](ui-builder.md)), drawn in the retail skin by the same
kit as the film selector, with the `theater.playback` sample as the
offline preview. The host captures their input before it reaches the
replayed simulation.

- **Bar** (bottom of the screen, hidden with Back or H): play state,
  speed asked and reached, time and length, a timeline with kill
  markers from the stats file, clip marks, the watched stretch the ring
  covers and the sidecar's keyframes, the camera mode and the subject's name and emblem colour.
  During a jump the timeline shows the target and fills towards it.
- **Pause menu** (Start or Escape): Resume, Restart Film, Change Player,
  Camera, Take Screenshot, Mark Clip, Export Clip, Save Film, Share, End
  Film. Lists open as a dialog over the menu.
- **Controls** are the table in
  [theater.md](theater.md#in-game-theater), with two additions: D-pad
  Down or `.` steps one tick while paused, and B or Escape during a jump
  cancels it and returns to the tick the jump left.
### Film compatibility

The film page shows one of two states, computed from the film's header
and the installed images before any download
(`boot::theater::image_match`, `Session::check_simulation_hooks`):

| State | Condition | Playback |
|---|---|---|
| **Playable** | XBE digest, guest image contract, `h2-sim/1` and hook digest equal; the map and initial files present | from the seed with every digest checked; sidecar keyframes for this build |
| **Unavailable** | a different simulation contract or XBE, or a missing map or initial file | the reason: the film predates a breaking change to the simulation, or what to install |

An equal native stamp earns no faster path, because no film carries
keyframes for it. A film is never played against a simulation whose
contract differs: `check_simulation_hooks` stays a refusal, and the film
page says so. A film plays on every build until the next breaking change
to the simulation contract; after one it is unavailable in the game.
There is no pose fallback in the game. The game's stats stream still
opens on the web.

The engine film narrows what counts as breaking: it depends on the
simulation's results, not on the machine's schedule, so it plays on any
build whose simulation is bit-exact
([sim-rewrite.md](../engine/sim-rewrite.md) section 2).

### Sharing through Live

- **A Live game.** A dedicated host streams two things to the server as
  the game is played: the stats stream and its film. Live imports the
  stats through its existing ledger import. Live stores the film as sent and never
  inflates a chunk. It needs no scrub: its identifiers are the per-game
  ones Live issued, and its roster is the game id and each seat's
  gamertag. A host film that still has a machine part is the exception,
  and is served only to the game's participants until the engine film
  replaces it. A download carries gamertags and the game id, no other
  identity, and no guest RAM. Each player's
  machine builds keyframes as a sidecar.
- **Each player's own film** of a Live game, the host's film as
  forwarded, stays on that player's
  machine. Share on a Live game creates no upload, only the link.
- **Share.** A film recorded on this machine is already free of guest
  RAM, so `film::share` copies it with the profile in `DATA` reduced to
  the fields the guest reads, and sends the stats file beside it. If the
  stats file is missing, the Prepare pass writes it first.
- **Upload.** `POST /v1/films` takes a film and its stats stream from a
  signed-in player under `films.write`, in the same numbered parts and
  continuations as a host upload, and gives them a film id. Live inspects
  the stats stream as it inspects a host's and derives nothing for the
  stats ledger from it.
- **Download.** The game fetches a film into the library once, by range
  requests it can resume, and plays the file from disk.
- **Visibility.** `private` (the uploader), `link` (any signed-in account
  with the link) or `public` (listed on the player's Films tab). Public
  range reads serve the stats stream under the existing rules. The film
  goes only to signed-in accounts the visibility admits.
- **Both homes.** The routes sit behind the same storage traits as the
  host-film routes and run on the native server and on Workers.

### The link between the web and the game

A share link is the web address of the film or clip:
`/theater/<film>` or `/theater/<film>/clip/<clip>`
(`web/src/routes.ts`, `theaterHref`), with optional `t=<tick>`,
`seat=<n>` and `camera=<mode>`. The page's **Open in Halo 2** button
opens `h2recomp://theater/<film>?clip=<clip>&t=<tick>&seat=<n>&camera=<mode>&server=<origin>`.

- The app bundle registers the `h2recomp` scheme, the same scheme a
  Live join link uses. A running game receives the
  URL and opens the film page with **Watch** focused; a game that is not
  running starts and does the same. Nothing plays without a press.
- `h2 theater open <url>` does the same from a shell, and
  `theater.open {url}` through the agent API.
- The game accepts a link only for the Live server it is signed in to,
  compares `server` to it, and treats every field as untrusted text: ids
  match the film id grammar, numbers are bounded by the film's length and
  seat count, and no field reaches a path or a command line.
- The film page's **Copy Link** puts the web address on the clipboard.

## The snapshot design in numbers

| Quantity | Value | Source |
|---|---|---|
| Guest RAM | 64 MiB: 16,384 pages of 4,096 bytes | **Code**: `RAM_SIZE`, `PAGE_SIZE` in `crates/runtime/runtime/src/guest.rs` |
| Game-state arena | `0x3fe000` bytes: 4,186,112 bytes, 1,022 pages, 6.2% of RAM | **Code**: `crates/platform/xapi/tests/checkpoints.rs`; [sim-rewrite.md](../engine/sim-rewrite.md) |
| Tick rate | 30 Hz; 33.3 ms of film per tick | [high-framerate.md](high-framerate.md) |
| Sidecar keyframe cadence | Every sixth capture point, usually 900 ticks (30 s) | **Code**: `POINTS_PER_KEYFRAME` |
| Capture point cadence | 150 ticks (5 s) | **Code**: `CAPTURE_TICKS` |
| Sidecar keyframe bytes | planning: 16 MiB anchor + 7 × 1 MiB deltas per 4 minutes = 5.75 MiB a minute; 57.5 MiB for a 10-minute match plus one lobby anchor | **Estimate** from [film-format.md](film-format.md#keyframe-ram-budget); not measured |
| Film bytes | about 2,700 to 7,700 per player-minute and 3,650 a match-minute of digests; a 16-player 10-minute host film is about 0.5 MB | **Estimate**: [input-film.md](input-film.md#size-step-by-step) |
| Ring entries | 12 (one minute at the capture cadence) | **Design** |
| Ring page-table cost | 64 KiB an entry (16,384 references of 4 bytes) | **Estimate** |
| Ring pool cap | 256 MiB: 65,536 pages, a mean of 5,461 written pages (21 MiB, a third of RAM) for each of 12 entries | **Design**; worst case without the cap is 12 × 64 MiB = 768 MiB |
| Sidecar cache cap | 4 GiB: about 12 hours of film at the planning rate | **Design**, **Estimate** |
| Simulation cost | 14.12 ms a tick on the heaviest profiled case (Waterworks, four bots); 7.5 ms is the rewrite's target | [sim-budget.md](../perf/sim-budget.md), [sim-rewrite.md](../engine/sim-rewrite.md) section 1 |
| Unrendered replay rate | 2.4× real time at 14.12 ms; 4.4× at 7.5 ms | **Estimate**: 33.3 ÷ ms per tick |
| Replay after a ring restore | worst 150 ticks: 2.1 s at 14.12 ms, 1.1 s at 7.5 ms; mean half that | **Estimate** |
| Replay after a sidecar keyframe | worst 900 ticks: 12.7 s at 14.12 ms, 6.8 s at 7.5 ms; mean half that | **Estimate** |
| Replay from the start, 10-minute film | 18,000 ticks: 4.2 min at 14.12 ms, 2.3 min at 7.5 ms | **Estimate** |
| Warm RAM restore | bounded by one 64 MiB copy | **S**: measure in slice 0 |
| Process start, boot and cold keyframe restore | not measured | slice 0 |
| Pages written per 5 s and per 30 s of play | not measured; `guest_written_pages` gives the whole-match count | **Code**: `crates/live/sim-bench/src/metrics.rs`; slice 0 |
| Arena keyframe | 4 MiB plus the extension blob, at most 2 ms to take | [sim-rewrite.md](../engine/sim-rewrite.md) section 1, a target |
| Arena ring | one entry a second: 60 entries for a minute, 240 MiB before page sharing; worst replay 30 ticks, 0.23 s at 7.5 ms | **Estimate** |

The 4× speed setting is reachable only where a tick and its frame cost
under 8.3 ms. The bar shows the speed reached.

### Determinism checks

| Check | When | On failure |
|---|---|---|
| Gameplay digest, 16 bits of it in the film | every tick that has one, in play and in replay (`EngineReplay::completed`) | playback stops at that tick with "film diverged at mm:ss" |
| Arena hash (`lockstep::hash_sim(SimRegion::HALO2)`) | every 30 ticks in the machine part; every 256 in the tick stream | the same |
| HLE call cursor and D3D barrier outcome, while the film has a machine part | at each recorded call and barrier | the same |
| Whole-RAM hash (`lockstep::hash_guest`, per MiB) | after every keyframe restore, sidecar or ring | A sidecar: the file is deleted and written again from the film. A ring entry: cold restart, warm restores off |
| Ring capture leaves the run unchanged | in tests, and in any player started with verification on: the whole-RAM hash at each sidecar keyframe tick equals the one a run without ring captures stored | ring captures off for the session |

### Before and after the sim rewrite

Three things arrive at different times.

| | Ships without the rewrite | Waits for the engine-film experiment (slice 15) | Waits for the rewrite (`sim_state::Keyframe`, slice 11) |
|---|---|---|---|
| The film | the machine film: boot recipe, tape, inputs, digests; no `KEYF` | the engine film: seed, inputs, seat records, digests; no machine part | the same engine film |
| Start | boot, replayed | the engine's game start from the seed | the same; an arena keyframe as the seed only if the experiment requires it |
| Seek accelerator | machine keyframes in the sidecar and the ring, at recorded capture points | the same | arena keyframes: the arena and the extension blob, at any tick boundary |
| Ring cadence | 5 s | 5 s | 1 s |
| Worst replay inside the ring | 2.1 s | 2.1 s | 0.23 s |
| Scrubbing | the bar moves; the picture follows on release | the same | the picture follows the bar |
| Host film without packets | no | yes | yes |
| Across builds | no | yes, while the simulation stays bit-exact | yes |
| Renders a playerless host film with the game's renderer | no | no | needs the renderer to own the frame as well |

The experiment needs no rewrite code. It asks three questions of a
recorded game, each answered by the film's own hashes over 3,000 ticks,
in a campaign level and in a multiplayer game:

1. **Start.** Does the engine's game start, given the seed, produce
   the arena the film's first hash names?
2. **Tick purity.** With the inputs and seat records written at
   `SIM_UPDATE_APPLY` and no tape, do the digests match when the schedule
   and the render cadence differ from the recording's?
3. **Seats.** Does a seat made in memory at its recorded tick give the
   same arena as the join the recording saw?

A no to the first makes the seed an arena keyframe. A no to the second
or third keeps the machine film for that kind of game.

Machine keyframes do not depend on the rewrite. Rust simulation state is
static, derived, transient or extension state
([sim-rewrite.md](../engine/sim-rewrite.md) section 2): static and
derived state is rebuilt or dropped on a snapshot load (the epoch bump),
transient state does not exist between ticks, and extension state is a
feature section in the snapshot. Each rewrite phase therefore owes
Theater two things, both already in its own exit criteria: a
`SnapshotPolicy` for its feature, and an epoch bump on a keyframe load.
A rewrite phase that changes the hook digest is a breaking change: films
recorded before it are unavailable on builds after it.

## Interfaces

### Agent API

Added to `crates/extensions/agent-proto/src/theater.rs` and documented in
[agent-api.md](../runtime/agent-api.md#theater-transport) when built.
Existing methods keep their parameters.

| Method | Parameters | Result |
|---|---|---|
| `theater.state` | `{}` | adds `speed_reached`, `compatibility` (`playable`, `unavailable`), `seek` (`{target, from_tick, source: "ring" \| "keyframe" \| "sidecar" \| "boot", warm, remaining_ticks, restore_ms}` or null), `verified_through` (the last tick whose digest matched), `divergence` (`{tick, kind}` or null), `camera_pose` (`{position, yaw, pitch, fov, radius}`), `hud`, `bar` |
| `theater.seek` | `{tick, cancel?: true, source?: "boot" \| "keyframe"}` | transport state; `source` forces a route for tests |
| `theater.step` | `{ticks: 1}` | runs that many ticks and holds |
| `theater.keyframes` | `{}` | `{film: [tick], sidecar: [tick], ring: [tick], capture_points: [tick], ring_bytes, ring_cap, sidecar_bytes}` |
| `theater.verify` | `{from?, to?, ring?: true}` | replays the range unrendered in a second player and returns `{ticks, digests_checked, arena_hashes_checked, ram_hashes_checked, first_divergence}`; `ring` takes and restores a ring entry at every capture point on the way |
| `theater.prepare` | `{id}` | starts the pass that writes the sidecar, plus a missing stats file once the two streams are separate. Returns `{id, status, tick, length_ticks, error?}`, also available from `theater.state {id}` on the library agent. |
| `theater.hud` | `{game?: bool, bar?: bool}` | transport state |
| `theater.screenshot` | `{scale?: 1..4, path?}` | `{path, width, height, tick}` |
| `theater.clip.mark` | `{in?: tick, out?: tick, clear?: true}` | the marks |
| `theater.clip.export` | `{from, to, fps: 30 \| 60, format: "png" \| "mp4", path?}` | `{job}`; `theater.clip.status {job}` gives `{frames_done, frames_total, path, error}` |
| `theater.compat` | `{id}` | `{state, reasons: [text], film: {xbe, contract, hooks, abi}, installed: [{stamp, state}]}` |
| `theater.share` | `{id, visibility, clip?}` | `{film, url}` after the upload |
| `theater.open` | `{url}` | the film page's `ui.screen`, or a refusal naming the field |

The menus are checked through the existing `ui.screen` and `ui.input`.

### Live API

Added to [live-api.md](../live/live-api.md#theater) when built.

| Method and path | Token scope | Result |
|---|---|---|
| `POST /v1/films`, `PUT /v1/films/{film}?part=<n>` | `films.write` | Upload a film and its stats stream; 202 while inspection continues, 201 with `{film, url}` on publication. At most 256 MiB a film and 2 GiB an account. |
| `GET /v1/films/{film}` | by visibility | The listing row, with `compat`: the XBE digest, guest contract digest, simulation ABI and hook digest from the film's seed. |
| `GET /v1/films/{film}/film` | by visibility | With a range, the stats stream's public ranges, as `GET /v1/games/{game}/film`. Without one, the film, to a signed-in account; byte ranges of the film resume a download. |
| `PUT /v1/films/{film}/visibility` | `films.write`, the uploader | `{visibility}` |
| `DELETE /v1/films/{film}` | `films.write`, the uploader or an admin | 204 |

A film of a Live game keeps its game id as its film id, so every existing
link stays valid.

### Film format

The film's own layout is [input-film.md](input-film.md). The rest,
described in [film-format.md](film-format.md) when built:

- The stats stream is the `H2BLAM3!` container without tag 1, tag 4 and
  the machine section, with an empty `keyframes` array in `CIDX`.
- The recorder writes a `d3d_barrier` mark every 150 ticks into the
  film's machine part. A reader that knows barriers needs no change.
- The sidecar file: the eight bytes `H2KEYS1!`, the film id, the film's
  header digest, the build key, then `KEYF` envelopes and a closing
  directory. Each sidecar keyframe has a `film::tape::SidecarCursor`
  identifying its capture mark. The player resumes at the successor tick
  in the indexed input block. The column decoder in slice 13 extends this
  cursor with its film chunk and seat rows. A recorder's sidecar gets the
  digest when the film is sealed.
- The machine part's call tape is one binary stream in call order: a
  ULEB operation code from a table in the header, the thread, the
  arguments and the result as ULEBs, and each output as a length and
  bytes. A pad read is the bytes that differ from the handle's last
  packet. Batches close at 128 calls, 64 KiB or an arena hash, never on
  a change of stream.
- The pure operations `Htonl`, `Htons` and `FdIsSet` are computed in
  playback and take no call index. This changes the call cursor, so it
  raises the simulation ABI.

## Slices

A lane writes the code and tests; the coordinator builds, runs and
measures. "Starts" says what the slice waits for. Numbers are names, not
an order: 12, 13 and 14 start with 0 to 3.

| # | Slice | Starts | Acceptance |
|---|---|---|---|
| 0 | **Measure the existing player.** `theater.keyframes`, the `seek` block and `restore_ms` in `theater.state`; `h2 film stat FILE` printing each `KEYF`'s tick, stored bytes and anchor, the barrier marks, and for each tape stream its calls, inflated bytes, stored bytes and chunks. Then three films are recorded and played (a leased host match, a local multiplayer game, a campaign level) and a dated note records keyframes published and skipped with the blockers, anchor and delta bytes, capture time, pages written per 5 s and 30 s, child start to first slice, boot to match start, cold restore time, unrendered ticks a second, and the tape table per stream with the operations an offline game makes. | now | Tests on synthetic films for the new fields and `film stat`. The note `docs/game/notes/theater-seek.md` exists and the numbers on this page cite it or are corrected by it. |
| 1 | **Jumps that show their work, on the cold path.** Seek progress and cancel; taps within 250 ms merged into one jump; `speed_reached`; `theater.step`; the timeline's target and fill. | now | `theater::tests`: a cancelled seek returns to the tick it left; five taps make one restart; a step of 1 advances one tick and holds. Agent-proto round trips. A boot-sourced seek on a synthetic film reports falling `remaining_ticks`. |
| 2 | **Sidecar keyframes and Prepare.** The sidecar writer and reader, the merged keyframe list, the cache cap and the film page's background Prepare pass. The writer is one type used by the player and by the recorder. | now | A synthetic film without `KEYF`: play to the end, then a seek reports `source: "sidecar"` and its digests equal the straight run's. The film page and agent report progress and the verified result. A sidecar of another build key is ignored. A damaged or missing sidecar is written again from the film. The cap evicts the least recently watched film's sidecar. |
| 3 | **Capture points every 150 ticks** in the recorder, with no keyframe in the film. | now | `film` tests: a 30 s local recording holds six barrier marks and no `KEYF`, and its sidecar holds one keyframe; a host recording holds the marks and writes no sidecar; a film with barriers only at keyframes still plays. `theater::tests`: linear replay performs each barrier at its slice. |
| 4 | **Warm restore and the ring.** The page pool, ring policy, in-process restore, the warm rules and the cold fallback, the paused-camera redraw. | code now; merges after slice 0 shows keyframes are taken in real films | Over a synthetic machine in the manner of `crates/runtime/kernel/tests/snapshot.rs`: run, capture at k, run to m, restore k in the same machine, run to m, and every slice digest matches; twice in a row. Pool: unchanged pages are shared, the cap evicts oldest first and is never passed. A forced RAM mismatch restarts cold and turns warm restores off. A disk delta between source and target restarts cold. `theater.verify {ring: true}` over each slice 0 film returns no divergence. |
| 5 | **One camera rig.** The rig and `camera_pose` in state for rendered and playerless films, field of view, follow and orbit on rendered films, the game HUD and bar toggles as `theater.hud`. | now | `theater::tests`: the rig survives a seek; a camera change leaves the digests of a synthetic replay unchanged; `first-person` on a seat without a viewport falls to follow and says so in state. In a run: `theater.verify` with a free camera moving on a rendered film returns no divergence. |
| 6 | **Screenshots and clip export.** Scaled screenshots, the frame sink, PNG and ffmpeg output, export jobs. | now for the sink and the job; the scaled screenshot after slice 4 | A headless synthetic replay with the sink yields exactly `(to − from) × fps ÷ 30` frames. The ffmpeg argument list is tested without running ffmpeg. A missing ffmpeg gives the `h2 movies` exit reason and leaves the PNG option. |
| 7 | **The bar and the pause menu** as UI-builder pages. | now | Page-model tests for every row and state, the `theater.playback` preview with a jump in progress, a Prepare in progress, a divergence and both compatibility states; `ui.screen` reports each page; controller and keyboard reach every action. |
| 8 | **Compatibility states.** `theater.compat` and the film page's state line, with the breaking-change reason. | now | `image_match` table tests for the two states; a film from before a simulation contract change is unavailable with that reason; a film of another native target with an equal contract is playable. |
| 9 | **Share.** `film::share`, the film and stats upload routes on both homes, the resumable download into the library, visibility, quotas, the Films tab rows, **Share** and **Copy Link** in the game. | after slice 12 | A shared film holds no `KEYF`, no guest RAM bytes and no identity but gamertags, and replays to the same digests as its source. A share without a stats file writes one first. An interrupted download resumes and the file's digest matches. Route tests: scope, quota, visibility on every read. The parity suite passes on both homes. |
| 10 | **The link.** The `h2recomp` scheme, `h2 theater open`, `theater.open`, the web button. | after slice 9 for end to end; the parser now | Parser tests: every malformed id, bound and server is refused by name. `theater.open` lands on the film page with Watch focused and plays nothing. A web test finds the button on film and clip pages with the encoded target. |
| 11 | **Arena keyframes.** `sim_state::Keyframe` in the ring at 1 s and in the sidecar, scrubbing. | after the sim rewrite's object phase and `sim_state::Keyframe`, and slice 15 | A ring entry taken at any tick restores inside a running match and the next 3,000 digests match. A film recorded on build A plays on build B with matching arena hashes. |
| 12 | **The split.** The recorder writes the stats stream and the film as two files; the film has no `KEYF`, and a local recorder's keyframes go to the sidecar; the selector reads a film's header and the stats file beside it; the player takes a film with no keyframe as the normal case. The film keeps tag 1's existing coding inside its own file until slice 13. | now; the sidecar half after slice 2 | A recording of a synthetic game gives a stats file the existing web reader tests accept unchanged and a film that plays from its seed to the same digests. No pose or event record is in the film; no input or digest is in the stats file. A container holding both still plays. |
| 13 | **The film encoding.** `h2 film stat --fields` over recorded films: per field, how often it changes, the stored bytes under each predictor, and any grid. Then the column encoder and streaming decoder of [input-film.md](input-film.md#the-encoding), the 16-bit digests, the dictionary if it earns its place, and the fixed buffers. | the measurement now, on films that exist; the encoder after slice 12 | The field table in a dated note, with the size steps of input-film.md corrected by it. Round trips: every recorded film re-encodes and decodes to identical action bytes on every tick. A fuzzed chunk is refused, never read out of bounds. The recorder and the decoder allocate nothing after start, shown by a counting allocator in the tests. |
| 14 | **Tape minimisation.** The binary call stream, pad deltas, batches that do not close on a change of stream, pure operations computed in playback, and a null network for an offline game in place of its socket tape. | after slice 0's tape table; one ABI bump for all of it | Round trips for every operation's outputs. A synthetic offline film holds no socket call and replays to the same digests. A film in the JSON encoding is refused as a breaking change. The note gains the stored bytes per stream before and after. |
| 15 | **The engine film.** The three experiments, the seat record in the recorder, playback from the seed with seats made in memory, start points for a mid-game join, then films written without a machine part. | experiments after slice 12; adoption per kind of game as each passes | The three answers, with their hash runs, in a dated note. Then: an offline film with its machine part removed plays to the same digests; a host film with its packets removed does too; a film that begins at a start point plays to the same digests, and a start point with one byte changed is refused. |
| 16 | **Live and the web.** Ingest takes the stats stream and the film as two objects, stores the film without inflating a chunk, issues the per-game identifiers at session creation, and carries the action side stream on the tunnel; the ranged read serves the stats stream and the unranged read the film; the ledger import reads the stats stream. | after slice 12 | The existing public-range, ledger and web viewer tests pass against a stats stream with no film in it. A film upload is stored byte for byte and its tick chunks are never inflated (a counting test double). Two games of the same players share no identifier. A client that drops three forwarded chunks ends with a film equal to the host's. A container holding both is still served. The parity suite passes on both homes. |

Slices 1, 2, 3, 5, 6, 7, 8, 12, 13 and 14 touch separate files and can run as
parallel lanes, except that 3 and 12 share `theater/record.rs`. Slice 4
is the only one whose merge waits on a measurement.

## Rules and limits

- Stats is the summary and the film is the simulation. A film holds no
  pose, event or score; a stats stream holds no input or digest.
- A film is played only against a simulation whose contract it matches.
  There is no "try it anyway", and no pose fallback in the game.
- Theater input never reaches the replayed guest. The camera changes
  only the view argument and a private random seed, or the independent
  view.
- No film holds a keyframe. A sidecar or ring entry loads only into the
  build and switches that wrote it
  ([snapshots.md](../runtime/snapshots.md)).
- Guest RAM never leaves the machine: not in a film, not in an upload.
- A jump while a jump is running replaces its target. The player never
  queues jumps.
- Reverse play is not offered. Rewind is a jump back.
- The ring covers only film already watched in this session. A jump
  ahead uses sidecar keyframes, or simulates from the nearest one.
- A film whose guest disk changes between keyframes restarts the child
  for jumps across the change.
- A film with no capture point is seekable only by simulating from its
  start.
- A Live client records the host's actions, not its own packets. Until
  the engine film plays host games, a client's film keeps its network
  tape.
- A start point is the arena and extension state, at most 1 MiB stored,
  and is refused unless the host's tag and the film's arena hash match.
- Seat and machine identifiers are issued per game and mean nothing
  outside it.
- Clip export needs the user's own ffmpeg for MP4; PNG needs nothing.

## Evidence

- **Code**: a seek restarts the child. `crates/apps/cli/src/theater_cmd.rs`
  loops on `restart.json`; `boot::run_theater` restores the keyframe
  before the first slice (`crates/apps/boot/src/lib.rs`, `take_keyframe`).
- **Code**: the recorder captures between slices when `capture_point_due`,
  with no early slice end, and retries on later ticks within the first
  second of the interval (`record.rs`, `try_capture_point`).
- **Code**: barriers are replayed at their slice and their outcome is
  checked (`replay.rs`, `replay_capture_point`).
- **Test**: `theater/tests/capture_points.rs` covers local and host
  capture points, playback rebuilding the sidecar, and cold seeks using
  a sidecar or an embedded keyframe on synthetic machines.
- **Test**: an in-process load into a running synthetic machine, twice,
  with matching per-slice digests (`crates/runtime/kernel/tests/snapshot.rs`).
- **Code**: `state.load` already loads into a running game through
  `boot::snapshot_glue`; the film player does not use that path.
- **S**: a barrier mark without a published keyframe is accepted by
  every reader. `Playback::d3d_barrier` documents the case; slice 3's test
  verifies it.
- **Code**: playback takes a film with no usable keyframe from tick 1
  (`EngineReplay::open`, `ImageMatch::FromBoot`, and
  `TrackReader::seek` returning `None`).
- **Code**: the inputs are captured after remote arrivals and phantom
  actions are resolved, for every seat in the update's mask
  (`theater/actions.rs`, `capture`), so a host's film has every seat.
- **Code**: the stats stream's records and the film's share a container
  and a time chunk ([film-format.md](film-format.md#tagged-records)).

## Not verified

- Whether a real match publishes keyframes at all. The one probe run
  found the machine never quiescent at a tick boundary, for a cause since
  fixed, and the rerun has not been made
  ([notes/snapshots.md](../runtime/notes/snapshots.md)). Snapshots refuse
  while Live is signed in ([snapshots.md](../runtime/snapshots.md)); the
  film recorder has its own network part (`FilmNetwork` in
  `crates/extensions/game/src/snapshot_glue.rs`), and whether it clears
  that blocker on a leased host is read in slice 0.
- Every byte and millisecond in the numbers table that is marked an
  estimate or not measured.
- That a warm restore in the real game leaves no host state behind
  (feature statics, HLE allocators). Slice 4's `theater.verify` run on
  real films is the check; the cold path is the fallback.
- That a moved camera leaves a local film's hashes unchanged in the real
  game. [theater.md](theater.md) lists it as a pending experiment.
- How the app bundle receives a URL while a game is running, on each
  host OS.
- How long boot to match start takes in an unrendered replay, which is
  the wait before the first watch of a machine film from another machine.
- Which host calls an offline game makes, and so whether a null network
  can stand in for its socket tape.
- The three engine-film experiments, and every size in
  [input-film.md](input-film.md#not-verified).
- Whether the identifiers the simulation reads for a seat identify a
  console outside the game.

## Risks

1. **Few keyframes can be taken.** A machine that is never quiescent at a
   capture point gives no sidecar and no ring entry, and every jump
   simulates from the start. Slice 0 answers it first. The mitigations
   are per blocker: a rebuildable continuation, a feature section, or a
   recorded barrier.
2. **The first watch on another machine is slow.** A film from elsewhere
   has no sidecar, so it simulates from boot before it can jump:
   4.2 minutes for a 10-minute film at 14.12 ms a tick. Prepare runs it
   in the background; the engine film removes the boot and the lobby.
3. **Warm restore leaks host state** in ways synthetic machines do not
   show. Contained by the RAM hash, the digests and the cold fallback;
   the cost is that the ring gives nothing on affected films.
4. **The hook digest churns during the rewrite.** Every breaking phase
   makes the existing library unavailable in the game. Accepted: the
   simulation is expected to settle within days, and the stats stream
   still opens on the web.
5. **Ring captures perturb rendered films** through the D3D section's
   device work even at a recorded barrier, if the playback's render
   cadence differs from the recording's. The verification row in
   "Determinism checks" turns captures off rather than show a diverged
   film.
6. **Per-game identifiers must reach the simulation unchanged.** The
   identifiers the update carries come from the session's peers. If the
   engine derives them from something Live does not issue, a film would
   still name a console. Slice 16 traces the source before it relies on
   them.
7. **The engine film may not hold for a kind of game.** Then that kind
   keeps its machine part, and a host's film keeps its packets.
8. **The size estimates rest on assumed rates.** Slice 13 measures before
   the encoder is written.
9. **Two files can part company.** A film without its stats file plays
   and regenerates it; a stats file without its film shows on the web
   and offers no Watch.
10. **A `h2recomp://` link is input from the web.** The parser, the
    server match and the press-to-play rule are the whole defence, and
    each has a test.

## Open questions

1. **Hosts beyond macOS** that register the `h2recomp` scheme.

Settled: stats is the summary and the film is the simulation, in two
files; a film is a seed, inputs and joins, with no keyframe; films after
a breaking simulation change are refused, not drawn as poses; downloads
serve the film as stored, which needs no scrub because Live issues
fresh identifiers for each game; the host forwards every seat's actions
so each client records the same film; a player who joins mid-game starts
from an authenticated arena start point, never guest RAM; clip video is the user's ffmpeg and
Live stores only tick ranges; the scheme is `h2recomp`; every player in a
Live match records their own film, and the host's is what Live serves to
everyone else; rewind is jumps of 5 s and 30 s until slice 11 brings
scrubbing; the ring (256 MiB) and the sidecar cache (4 GiB) are both
settings, with those defaults.

## Related

- [input-film.md](input-film.md): what a film holds, its encoding, sizes and memory
- [theater.md](theater.md): the tiers, the recorder, the selector, the controls
- [film-format.md](film-format.md): the container as built, which becomes the stats stream
- [notes/halo3-film.md](notes/halo3-film.md): the Halo 3 reference
- [snapshots.md](../runtime/snapshots.md): what a machine snapshot holds and when it can be taken
- [sim-rewrite.md](../engine/sim-rewrite.md): who owns simulation state, `sim_state::Keyframe`
- [second-executor.md](../runtime/second-executor.md): copy-on-write guest RAM
- [agent-api.md](../runtime/agent-api.md#theater-transport), [live-api.md](../live/live-api.md#theater)
- [web-viewer.md](../live/web-viewer.md): the viewer that draws the stats stream
- [framescene.md](../graphics/framescene.md): the renderer that will draw a host film in full
