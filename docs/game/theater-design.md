# Theater: the full in-game player

Status: Design. The parts under "What it builds on" are built; everything else on this page is not.
Code: `crates/extensions/game/src/theater.rs` (`Session`), `crates/extensions/game/src/theater/replay.rs` (`restore_keyframe`), `crates/apps/cli/src/theater_cmd.rs` (the player process), `crates/live/film` (the container). Tests: `crates/extensions/game/src/theater/tests.rs`, `crates/live/film/src/tests.rs`.
Switch: none.

This is the design for a Halo 3-style Theater inside the game: play a
film, pause, rewind, jump to any point, change speed, fly a camera, follow
any player, hide the HUD, take screenshots and clips, and share a film
through Live so a link on live.forbes.red opens it in the game. It sits on
the film recorder and player that [theater.md](theater.md) and
[film-format.md](film-format.md) describe, and adds three things they lack:
keyframes the player makes for itself, a restore that does not restart the
process, and the sharing path. The rule that must hold: no Theater control
changes what the replayed simulation computes, and every seek proves it
with the film's recorded hashes.

Evidence labels: **Code** (read in this repository, path given), **Test**
(a named test), **S** (supposed, with what would verify it), **Estimate**
(arithmetic from cited numbers), **Design** (our choice).

## What the player gets

| Feature | Behaviour |
|---|---|
| Play a film | From the main menu's Theater, a film page's **Watch**, or a link. Local films and Live films use the same player. |
| Pause | Holds the simulation. The camera, HUD toggle and screenshots keep working while paused. |
| Speed | 0.25×, 0.5×, 1×, 2×, 4×. The bar shows the speed reached when the machine cannot keep up with the one asked for. |
| Rewind and jump | Tap for 5 s, hold for 30 s, or pick a point on the timeline. A jump inside the last minute watched lands in about two seconds; any other jump shows a progress bar and can be cancelled. |
| Step | One tick forward while paused. One tick back is a jump to the tick before. |
| Cameras | Recorded view, first person for a chosen player, follow, orbit, free flight. |
| HUD | Game HUD on or off; Theater's own bar on or off. |
| Screenshot | A clean frame without Theater's bar, at the window's internal resolution or a multiple of it. |
| Clip | Mark in and out. A clip is a range of the film; **Export** renders it to a video file offline. |
| Share | **Share** on a film or clip uploads it once and gives a link. The link opens the web viewer; its **Open in Halo 2** button opens the same film, range and camera in the game. |
| Films from other builds | Played exactly when the build matches, verified from the start when the guest contract matches, and shown from the recorded poses otherwise. The film page says which before the player presses Watch. |

## What it builds on

| Part | State | Where |
|---|---|---|
| The `.h2film` container: inputs, HLE tape, per-tick gameplay digests, arena hashes every 30 ticks, machine keyframes | Built | [film-format.md](film-format.md), `crates/live/film` |
| Machine keyframes every 30 s of guest time: one anchor with the whole 64 MiB RAM, then seven deltas against it | Built | `KEYFRAME_SECONDS` in `crates/extensions/game/src/theater/record.rs`; [film-format.md](film-format.md#keyframe-ram-budget) |
| Machine snapshots: guest RAM, kernel, D3D, audio, XAPI, feature sections, a RAM hash | Built, tested on synthetic machines | [snapshots.md](../runtime/snapshots.md) |
| The player: an isolated child process per film; a seek ends the child and starts another at the keyframe | Built | `crates/apps/cli/src/theater_cmd.rs` (`run`, the restart loop) |
| Transport over the agent API: `theater.play`, `state`, `pause`, `speed`, `seek`, `camera` | Built | `crates/extensions/agent-proto/src/theater.rs` |
| Cameras: a view replaced in the guest's renderer with a private random seed; a separate GPU view for a playerless host film | Built | `crates/extensions/game/src/theater/legacy_camera.rs` (`Seeds`), `host_view.rs` |
| The film selector and film page in the game's own menus | Built | [theater.md](theater.md#in-game-theater) |
| Compatibility: XBE digest, guest image contract, `h2-sim/1`, the hook digest | Built | `crates/extensions/game/src/theater/compatibility.rs` (`hook_digest`), [film-format.md](film-format.md#determinism-and-verification) |
| Live: host film upload, film lists, pins, clips as tick ranges, public range reads for the web viewer | Built | [live-api.md](../live/live-api.md#theater) |
| Arena keyframes and engine-level playback | Not built | [sim-rewrite.md](../engine/sim-rewrite.md) section 7 (`sim_state::Keyframe`) |

Four gaps follow from that table, and the design is their answers:

1. **A seek costs a process start.** Every jump boots the image again
   (`theater_cmd.rs`: "Each seek starts a new process so the game, HLE
   allocators and feature statics all restart"), restores the keyframe,
   then replays up to 30 s of film.
2. **A film may hold no keyframe the player can use.** A film played on
   another native target or a fuller image replays from boot
   (`film::image::ImageMatch::FromBoot`). A keyframe the recorder could
   not take is absent. Each seek in such a film replays from tick 1.
3. **A film stops being playable when the build changes.** The hook digest
   changes with every hook added or moved, and the sim rewrite adds hooks
   in every phase.
4. **Only a leased dedicated host can upload a film**, and only a
   participant can download the exact recording.

## How it works

### One player, three ways to reach a tick

The player keeps one process for the whole viewing session where it can.
A request for tick T chooses the cheapest source at or before T, in this
order:

| Source | What it is | Made by | Restore | Replay distance |
|---|---|---|---|---|
| **Ring** | Keyframes held in the player's memory for the last minute watched | the player, as it plays | in the running process | at most 150 ticks |
| **Film or sidecar keyframe** | A machine keyframe in the film, or one the player wrote beside it | the recorder, or the player | in the running process when the warm rules allow, else a new process | at most 900 ticks |
| **Boot** | The film's start conditions | nobody | a new process | all of it |

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
same device work at the same slice (`Session::replay_device_barrier`,
`film::tape::Playback::d3d_barrier`). A keyframe taken anywhere else would
make the playback differ from the recording.

So the player captures only at slices the film marks. The recorder
attempts a barrier every **150 ticks** (5 s) and publishes RAM with every
sixth, which keeps the 30 s keyframe cadence and its file size and gives
the player a capture point every 5 s. A film recorded with barriers only
at its keyframes gets a ring entry every 30 s; it still gains the warm
restore.

Captures take place between two slices, at a boundary the recording
already has. The recorder never asks for an early slice end
(`crates/apps/boot/src/lib.rs`, the loop that calls `try_keyframe`), and
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

While it plays or replays, the player writes a machine keyframe at every
sixth capture point it passes that the film has no usable keyframe for,
into a sidecar beside the library:
`<data>/films/cache/<film id>/<build id>.h2keys`, the build id being the
snapshot store's FNV-64 of the build key
([snapshots.md](../runtime/snapshots.md#where-snapshots-live)). The
sidecar uses the film's `KEYF` payload and anchor grouping unchanged, with
its own `TIDX`-shaped directory. `film::blam::TrackReader::seek` reads
film and sidecar keyframes as one ordered list.

- A film replayed from boot becomes seekable as far as it has been
  watched. **Prepare** on the film page replays it to the end in the
  background and reports progress.
- A sidecar is a cache: it holds game data, is never uploaded, and is
  removed with the film or when the cache passes its cap, least recently
  watched first.
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
  markers, clip marks, the watched stretch the ring covers and the
  keyframes, the camera mode and the subject's name and emblem colour.
  During a jump the timeline shows the target and fills towards it.
- **Pause menu** (Start or Escape): Resume, Restart Film, Change Player,
  Camera, Take Screenshot, Mark Clip, Export Clip, Save Film, Share, End
  Film. Lists open as a dialog over the menu.
- **Controls** are the table in
  [theater.md](theater.md#in-game-theater), with two additions: D-pad
  Down or `.` steps one tick while paused, and B or Escape during a jump
  cancels it and returns to the tick the jump left.

### Film compatibility

The film page shows one of three states, computed before any download of
the exact recording from the film's front JSON and the installed images
(`boot::theater::image_match`, `Session::check_simulation_hooks`):

| State | Condition | Playback |
|---|---|---|
| **Exact** | native stamp and target equal (`ImageMatch::Native`) | film keyframes; warm seeks |
| **Verified** | guest image contract, `h2-sim/1` and hook digest equal (`ImageMatch::FromBoot`) | from boot with every hash checked; sidecar keyframes after the first pass |
| **Unavailable** | a different simulation contract or XBE, or a missing map or initial file | the reason: the film predates a breaking change to the simulation, or what to install |

A film is never played against a simulation whose contract differs:
`check_simulation_hooks` stays a refusal, and the film page says so. A
film plays on every build until the next breaking change to the
simulation contract; after one it is unavailable in the game. There is no
pose-only fallback in the game. The web viewer still draws the pose
track, which is decoded without the guest
([film-format.md](film-format.md#tag-3-presentation-records)).

Engine-level films change this once they exist: an arena keyframe and
the per-tick actions the film already records (tag 1, the 92-byte retail
action per seat) replay on any build whose simulation is bit-exact,
because the arena keeps the retail layout
([sim-rewrite.md](../engine/sim-rewrite.md) section 2).

### Sharing through Live

- **Live film.** A Live game streams its film to the server as it is
  played. Ingest scrubs the private section as the parts arrive: the HLE
  tape's sender addresses and account identifiers are replaced by the
  game id and each seat's gamertag, and keyframes (guest RAM) are dropped.
  When the game ends, a compaction pass rewrites the scrubbed film into
  its smallest playable form under the game id: the per-tick actions,
  barrier marks, the pose and event chunks and the `CIDX` directory, and
  nothing a client rebuilds itself. That compact film is what every
  download serves, so a download carries gamertags and the game id and no
  other identity. Each player's machine builds keyframes as a sidecar.

- **Share copy.** `film::share` writes a copy of a sealed film without
  its `KEYF` envelopes and their disk deltas, keeping the barrier marks,
  and with the identity rules of the public and private sections
  unchanged. It is what leaves the machine: a keyframe holds guest RAM,
  which is game data, and the recipient's player rebuilds keyframes as a
  sidecar.
- **Upload.** `POST /v1/films` takes a share copy from a signed-in player
  under `films.write`, in the same numbered parts and continuations as a
  host upload, and gives it a film id. Live inspects it as it inspects a
  host film and derives nothing for the stats ledger from it. A film of a
  Live game already has its host film; Share on one of those creates no
  upload, only the link.
- **Visibility.** `private` (the uploader), `link` (any signed-in account
  with the link) or `public` (listed on the player's Films tab). Public
  range reads serve the pose and event chunks under the existing rules.
  The exact recording goes only to signed-in accounts the visibility
  admits.
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
| Film keyframe cadence | 900 ticks (30 s) | **Code**: `KEYFRAME_SECONDS` |
| Capture point cadence | 150 ticks (5 s) | **Design** |
| Film keyframe bytes | planning: 16 MiB anchor + 7 × 1 MiB deltas per 4 minutes = 5.75 MiB a minute; 57.5 MiB for a 10-minute match plus one lobby anchor | **Estimate** from [film-format.md](film-format.md#keyframe-ram-budget); not measured |
| Share copy bytes | under 47 KB per player-minute of input, events and poses, plus the HLE tape; a 16-player 10-minute match is 7.5 MB before the tape | **Estimate** from [film-format.md](film-format.md#size-targets) |
| Ring entries | 12 (one minute at the capture cadence) | **Design** |
| Ring page-table cost | 64 KiB an entry (16,384 references of 4 bytes) | **Estimate** |
| Ring pool cap | 256 MiB: 65,536 pages, a mean of 5,461 written pages (21 MiB, a third of RAM) for each of 12 entries | **Design**; worst case without the cap is 12 × 64 MiB = 768 MiB |
| Sidecar cache cap | 4 GiB: about 12 hours of film at the planning rate | **Design**, **Estimate** |
| Simulation cost | 14.12 ms a tick on the heaviest profiled case (Waterworks, four bots); 7.5 ms is the rewrite's target | [sim-budget.md](../perf/sim-budget.md), [sim-rewrite.md](../engine/sim-rewrite.md) section 1 |
| Unrendered replay rate | 2.4× real time at 14.12 ms; 4.4× at 7.5 ms | **Estimate**: 33.3 ÷ ms per tick |
| Replay after a ring restore | worst 150 ticks: 2.1 s at 14.12 ms, 1.1 s at 7.5 ms; mean half that | **Estimate** |
| Replay after a film keyframe | worst 900 ticks: 12.7 s at 14.12 ms, 6.8 s at 7.5 ms; mean half that | **Estimate** |
| Replay from boot, 10-minute film | 18,000 ticks: 4.2 min at 14.12 ms, 2.3 min at 7.5 ms | **Estimate** |
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
| Gameplay digest (tag 4) | every tick that has one, in play and in replay (`EngineReplay::completed`) | playback stops at that tick with "film diverged at mm:ss" |
| Arena hash (`lockstep::hash_sim(SimRegion::HALO2)`) | every 30 ticks | the same |
| HLE call cursor and D3D barrier outcome | at each recorded call and barrier | the same |
| Whole-RAM hash (`lockstep::hash_guest`, per MiB) | after every keyframe restore, film, sidecar or ring | a film keyframe: the film is damaged, reported. A sidecar: the file is deleted and written again. A ring entry: cold restart, warm restores off |
| Ring capture leaves the run unchanged | in tests, and in any player started with verification on: the whole-RAM hash at each film keyframe tick equals the one the film stored | ring captures off for the session |

### Before and after the sim rewrite

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

| | Before the rewrite | After `sim_state::Keyframe` exists and the keyframe experiment passes |
|---|---|---|
| Keyframe | the machine: 64 MiB RAM and every device | the arena and the extension blob |
| Restore | sections loaded in order; devices rebuilt | the engine's revert path (`0x123f60` or `0x124380`) inside a running match |
| Ring cadence | 5 s, at recorded capture points | 1 s, at any tick boundary |
| Worst replay inside the ring | 2.1 s | 0.23 s |
| Scrubbing | the bar moves; the picture follows on release | the picture follows the bar |
| Across builds | no | yes, while the simulation stays bit-exact |
| Renders a playerless host film with the game's renderer | no | needs the renderer to own the frame as well |

The keyframe experiment is the one in
[theater.md](theater.md#smallest-first-slice-worth-building): restore an
arena keyframe at tick k inside a running match, feed the recorded
actions, and match the recorded hashes for 3,000 ticks, in a campaign
level and in a multiplayer game.

## Interfaces

### Agent API

Added to `crates/extensions/agent-proto/src/theater.rs` and documented in
[agent-api.md](../runtime/agent-api.md#theater-transport) when built.
Existing methods keep their parameters.

| Method | Parameters | Result |
|---|---|---|
| `theater.state` | `{}` | adds `speed_reached`, `compatibility` (`exact`, `verified`, `poses`), `seek` (`{target, from_tick, source: "ring" \| "keyframe" \| "sidecar" \| "boot", warm, remaining_ticks, restore_ms}` or null), `verified_through` (the last tick whose digest matched), `divergence` (`{tick, kind}` or null), `camera_pose` (`{position, yaw, pitch, fov, radius}`), `hud`, `bar` |
| `theater.seek` | `{tick, cancel?: true, source?: "boot" \| "keyframe"}` | transport state; `source` forces a route for tests |
| `theater.step` | `{ticks: 1}` | runs that many ticks and holds |
| `theater.keyframes` | `{}` | `{film: [tick], sidecar: [tick], ring: [tick], capture_points: [tick], ring_bytes, ring_cap, sidecar_bytes}` |
| `theater.verify` | `{from?, to?, ring?: true}` | replays the range unrendered in a second player and returns `{ticks, digests_checked, arena_hashes_checked, ram_hashes_checked, first_divergence}`; `ring` takes and restores a ring entry at every capture point on the way |
| `theater.prepare` | `{id}` | starts the sidecar pass; `theater.state` of that id reports its progress |
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
| `POST /v1/films`, `PUT /v1/films/{film}?part=<n>` | `films.write` | Upload a share copy; 202 while inspection continues, 201 with `{film, url}` on publication. At most 256 MiB a film and 2 GiB an account. |
| `GET /v1/films/{film}` | by visibility | The listing row, with `compat`: the XBE digest, guest contract digest, simulation ABI and hook digest from the film's header. |
| `GET /v1/films/{film}/film` | by visibility | Public ranges as `GET /v1/games/{game}/film`. Without a range, the share copy, to a signed-in account. |
| `PUT /v1/films/{film}/visibility` | `films.write`, the uploader | `{visibility}` |
| `DELETE /v1/films/{film}` | `films.write`, the uploader or an admin | 204 |

A film of a Live game keeps its game id as its film id, so every existing
link stays valid.

### Film format

Additive, described in [film-format.md](film-format.md) when built:

- The recorder writes a `d3d_barrier` mark every 150 ticks. A reader that
  knows barriers needs no change.
- The sidecar file: the eight bytes `H2KEYS1!`, the film id, the film's
  `identity` digest of its `HEAD`, the build key, then `KEYF` envelopes
  and a closing directory.
- A share copy sets `share: true` in the front JSON and has an empty
  `keyframes` array in `CIDX`.

## Slices

In build order. A lane writes the code and tests; the coordinator builds,
runs and measures. "Starts" says what the slice waits for.

| # | Slice | Starts | Acceptance |
|---|---|---|---|
| 0 | **Measure the existing player.** `theater.keyframes`, the `seek` block and `restore_ms` in `theater.state`; `h2 film stat FILE` printing each `KEYF`'s tick, stored bytes and anchor, and the barrier marks. Then three films are recorded and played (a leased host match, a local multiplayer game, a campaign level) and a dated note records keyframes published and skipped with the blockers, anchor and delta bytes, capture time, pages written per 5 s and 30 s, child start to first slice, cold restore time, and unrendered ticks a second. | now | Tests on synthetic films for the new fields and `film stat`. The note `docs/game/notes/theater-seek.md` exists and the numbers table above cites it or is corrected by it. |
| 1 | **Jumps that show their work, on the cold path.** Seek progress and cancel; taps within 250 ms merged into one jump; `speed_reached`; `theater.step`; the timeline's target and fill. | now | `theater::tests`: a cancelled seek returns to the tick it left; five taps make one restart; a step of 1 advances one tick and holds. Agent-proto round trips. A boot-sourced seek on a synthetic film reports falling `remaining_ticks`. |
| 2 | **Sidecar keyframes and Prepare.** The sidecar writer and reader, the merged keyframe list, the cache cap. | now | A synthetic film without `KEYF`: play to the end, then a seek reports `source: "sidecar"` and its digests equal the straight run's. A sidecar of another build key is ignored. A damaged sidecar is deleted and rewritten. The cap evicts the least recently watched film's sidecar. |
| 3 | **Capture points every 150 ticks** in the recorder. | now | `film` tests: a 30 s recording holds six barrier marks and one keyframe; a film with barriers only at keyframes still plays. `theater::tests`: linear replay performs each barrier at its slice. |
| 4 | **Warm restore and the ring.** The page pool, ring policy, in-process restore, the warm rules and the cold fallback, the paused-camera redraw. | code now; merges after slice 0 shows keyframes are taken in real films | Over a synthetic machine in the manner of `crates/runtime/kernel/tests/snapshot.rs`: run, capture at k, run to m, restore k in the same machine, run to m, and every slice digest matches; twice in a row. Pool: unchanged pages are shared, the cap evicts oldest first and is never passed. A forced RAM mismatch restarts cold and turns warm restores off. A disk delta between source and target restarts cold. `theater.verify {ring: true}` over each slice 0 film returns no divergence. |
| 5 | **One camera rig.** The rig and `camera_pose` in state for both film kinds, field of view, follow and orbit on local films, the game HUD and bar toggles as `theater.hud`. | now | `theater::tests`: the rig survives a seek; a camera change leaves the digests of a synthetic replay unchanged; `first-person` on a seat without a viewport falls to follow and says so in state. In a run: `theater.verify` with a free camera moving on a local film returns no divergence. |
| 6 | **Screenshots and clip export.** Scaled screenshots, the frame sink, PNG and ffmpeg output, export jobs. | now for the sink and the job; the scaled screenshot after slice 4 | A headless synthetic replay with the sink yields exactly `(to − from) × fps ÷ 30` frames. The ffmpeg argument list is tested without running ffmpeg. A missing ffmpeg gives the `h2 movies` exit reason and leaves the PNG option. |
| 7 | **The bar and the pause menu** as UI-builder pages. | now | Page-model tests for every row and state, the `theater.playback` preview with a jump in progress, a divergence and each compatibility state; `ui.screen` reports each page; controller and keyboard reach every action. |
| 8 | **Compatibility states.** `theater.compat` and the film page's state line, with the breaking-change reason. | now | `image_match` table tests for the three states; a film from before a simulation contract change is unavailable with that reason. |
| 9 | **Share.** `film::share`, ingest scrubbing and the end-of-game compaction under the game id, the upload routes on both homes, visibility, quotas, the Films tab rows, **Share** and **Copy Link** in the game. | now | A share copy and a compacted Live film hold no `KEYF`, no guest RAM bytes and no identity but the game id and gamertags, and replay from boot to the same digests as their source; the compacted film is smaller than the streamed one. Route tests: scope, quota, visibility on every read, private ranges refused. The parity suite passes on both homes. |
| 10 | **The link.** The `h2recomp` scheme, `h2 theater open`, `theater.open`, the web button. | after slice 9 for end to end; the parser now | Parser tests: every malformed id, bound and server is refused by name. `theater.open` lands on the film page with Watch focused and plays nothing. A web test finds the button on film and clip pages with the encoded target. |
| 11 | **Arena keyframes.** The keyframe experiment, then `sim_state::Keyframe` in the ring at 1 s, engine-level playback, scrubbing, cross-build films. | after the sim rewrite's object phase and `sim_state::Keyframe` | The experiment's 3,000 ticks on a campaign level and a multiplayer game. Then: a film recorded on build A plays on build B with matching arena hashes. |

Slices 1, 2, 3, 5, 6, 7, 8 and 9 touch separate files and can run as
parallel lanes. Slice 4 is the only one whose merge waits on a
measurement.

## Rules and limits

- A film is played only against a simulation whose contract it matches.
  There is no "try it anyway" for the exact player.
- Theater input never reaches the replayed guest. The camera changes
  only the view argument and a private random seed, or the independent
  view.
- A keyframe, sidecar or ring entry loads only into the build and
  switches that wrote it ([snapshots.md](../runtime/snapshots.md)).
- Guest RAM leaves the machine only in a host film uploaded by its
  leased host. A share copy carries none.
- A jump while a jump is running replaces its target. The player never
  queues jumps.
- Reverse play is not offered. Rewind is a jump back.
- The ring covers only film already watched in this session. A jump
  ahead uses film or sidecar keyframes.
- A film whose guest disk changes between keyframes restarts the child
  for jumps across the change.
- A film with no capture point (recorded before barriers were marked
  between keyframes, with no keyframe published) is seekable only
  through its sidecar.
- Clip export needs the user's own ffmpeg for MP4; PNG needs nothing.

## Evidence

- **Code**: a seek restarts the child. `crates/apps/cli/src/theater_cmd.rs`
  loops on `restart.json`; `boot::run_theater` restores the keyframe
  before the first slice (`crates/apps/boot/src/lib.rs`, `take_keyframe`).
- **Code**: the recorder captures between slices when `keyframe_due`,
  with no early slice end, and retries each tick while blocked
  (`record.rs`, `try_keyframe`).
- **Code**: barriers are replayed at their slice and their outcome is
  checked (`replay.rs`, `replay_device_barrier`).
- **Test**: `seek_restores_machine_and_replays_only_the_remaining_ticks`
  (`theater/tests.rs`) covers the cold keyframe path on a synthetic
  machine.
- **Test**: an in-process load into a running synthetic machine, twice,
  with matching per-slice digests (`crates/runtime/kernel/tests/snapshot.rs`).
- **Code**: `state.load` already loads into a running game through
  `boot::snapshot_glue`; the film player does not use that path.
- **S**: a barrier mark without a published keyframe is accepted by
  every reader. `Playback::d3d_barrier` documents the case; slice 3's test
  verifies it.

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
- That a local v3 film carries pose records complete enough for pose
  playback of campaign levels.
- How the app bundle receives a URL while a game is running, on each
  host OS.

## Risks

1. **Real films hold few or no keyframes.** Then every jump is from boot
   until the sidecar exists, and the sidecar meets the same blockers.
   Slice 0 answers it first. The mitigations are per blocker: a
   rebuildable continuation, a feature section, or a recorded barrier.
2. **Warm restore leaks host state** in ways synthetic machines do not
   show. Contained by the RAM hash, the digests and the cold fallback;
   the cost is that the ring gives nothing on affected films.
3. **The hook digest churns during the rewrite.** Every breaking phase
   makes the existing library unavailable in the game until slice 11.
   Accepted: the simulation is expected to settle within days, and the
   web viewer still draws the pose track.
4. **Ring captures perturb rendered local films** through the D3D
   section's device work even at a recorded barrier, if the playback's
   render cadence differs from the recording's. The verification row in
   "Determinism checks" turns captures off rather than show a diverged
   film.
5. **A shared exact film discloses more than its poses**: the HLE tape
   holds received packets and their senders, and the initial disk holds a
   profile. Live scrubs these at ingest and serves only the compacted
   film; the scrub needs a test for each tape field.
6. **Replay is slow on heavy maps.** At 14.12 ms a tick a 30 s gap is
   12.7 s. The ring and the 5 s cadence bound the common case; the
   rewrite's 7.5 ms halves the rest.
7. **A `h2recomp://` link is input from the web.** The parser, the server
   match and the press-to-play rule are the whole defence, and each has a
   test.
8. **Storage.** Player uploads are new stored bytes on Live. The quota
   and the absence of keyframes bound them.

## Open questions

1. **Host film picture.** Most shared films are playerless host films,
   drawn without effects, sky or HUD. Is that acceptable for the first
   release, or should clients in a Live match also record their own
   film, which replays with the full picture and that player's HUD?
2. **Budgets.** 256 MiB for the ring and 4 GiB for sidecars by default,
   both settings?
3. **Rewind feel.** Jumps of 5 s and 30 s, as Halo 3 had, or is
   tick-accurate backward stepping worth its cost before slice 11?
4. **Hosts beyond macOS** that register the `h2recomp` scheme.

Settled: downloads serve the scrubbed, compacted Live film (sharing
above); films after a breaking simulation change are refused, not drawn
as poses (film compatibility); clip video is the user's ffmpeg and Live
stores only tick ranges; the scheme is `h2recomp`.

## Related

- [theater.md](theater.md): the tiers, the recorder, the selector, the controls
- [film-format.md](film-format.md): the container, keyframes, compatibility fields
- [snapshots.md](../runtime/snapshots.md): what a machine snapshot holds and when it can be taken
- [sim-rewrite.md](../engine/sim-rewrite.md): who owns simulation state, `sim_state::Keyframe`
- [second-executor.md](../runtime/second-executor.md): copy-on-write guest RAM
- [agent-api.md](../runtime/agent-api.md#theater-transport), [live-api.md](../live/live-api.md#theater)
- [web-viewer.md](../live/web-viewer.md): the viewer a share link opens
- [framescene.md](../graphics/framescene.md): the renderer that will draw a host film in full
