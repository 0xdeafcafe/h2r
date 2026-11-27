# Replay fidelity: what the viewer gets, and a better track (design)

The web viewer replays recorded telemetry in 3D and on the 2D map.
Current recordings use game ticks and independent chunks of up to 150
samples. After archive publication, the viewer seeks into independently
decodable H2AR segments.

Older one-frame-per-second recordings are still
readable. The measurements below preserve the baseline that motivated
the spectator track proposal. [Capture and packed formats](live-telemetry-packed.md#packed-frames)

It sits next to [theater.md](../game/theater.md), which records a game's
*inputs* for an exact re-simulation. The spectator track here records what
the game *looked like*, so it plays anywhere, a browser included. The two
share a clock and an event model; see [Theater](#the-theater).

## The game clock (built)

The campaign flicker had one cause. A checkpoint revert or a mission
restart continues the same game ([live-telemetry.md](live-telemetry.md),
"Campaign"), but it winds the engine's tick back. Frames and events carried
the engine's tick, so a game that reverted had two or more runs over the
same ticks.

The viewer sorted all frames by tick, so the runs interleaved
and a player jumped between them about once a second.

The recordings show it. In `05a_deltaapproach` game `05f93f55…` the ticks go
2, 48 … 230, then 22, 50, then 6, 24 … 958: three runs. Game `3f23bbf0…`
rewinds from 3114 to 12 and from 1342 to 4.

It wasn't a (0, 0, 0) read:
none of the sampled campaign frames have a player at the origin.

Fixed at both ends:

- **Capture** (`live-telemetry/src/session.rs`). After a revert, the
  match adds an even offset to the engine's tick, so the first tick sent
  comes after the last one. Frames, kills, scores and the end all use it.
  Frames stay on even ticks.
- **The origin.** A player whose unit couldn't be read, and who had no
  body or last place, used to be sent at (0, 0, 0) (a multiplayer pregame
  player, or a stale unit datum during a load). That player is now left
  out. A living player whose unit is unreadable for one tick stays where
  that same unit was last seen.
- **Viewer** (`web/src/state/replay.ts`, `GameClock`). Old recordings
  still rewind. The timeline reads frames and events in log order, and
  starts each run just after the one before. Given the events' own
  `GameFrame`s it places every event exactly, and a kill logged before its
  run's first recorded frame starts that run itself. The legacy replay reader fetches
  those frames from `/frames`, so events are placed into the
  frames' runs by their order and ticks (`GameClock.place`), which is
  exact except for an event in the second before a run's first frame. `ReplayTimeline.cuts`
  lists where the runs begin. Players at exactly the origin facing +x (the
  old placeholder) are dropped from recorded and live frames.

The tests are `a_campaign_revert_does_not_wind_the_games_ticks_back` and
`a_player_with_no_known_place_is_left_out_not_sent_at_the_origin` in
session.rs, and "a recording whose ticks run backwards" in
`web/test/replay.test.ts`. Each failed before its fix.

**The rule from now on:** within a game, the *game tick* only goes up. It
is the engine's tick plus the offset for the run it's in. Every later
proposal on this page uses it.

## Current recording and playback

The game samples on even ticks (15 Hz at the retail 30 Hz tick rate).
New captures keep the samples admitted by the game and server throttle
in chunks. Shots, actions and hits carry timing offsets where available.

Device travel is tracked separately from world poses. A delta omits quiet
devices and a complete machine-aware key seeds their last known travel.

The live wire uses v8 when negotiated. The recorder currently writes
v7 in H2CK chunks, and the archive reader also accepts sources with a
mix of v7 and v8 records. The live codec does not
change the sampling rate or the captured field grids.

The same decoded
frames go into H2AR for finished games. A segment's index can seed device
travel at its start even if the first frame is sparse. The viewer holds
known travel between updates and starts fresh across cuts and rewinds.
[Live wire and archive handoff](live-telemetry-packed.md#packed-frames) ·
[Archive API](live-api-details.md#finished-replay-archives)

## The original 1 Hz recording

The first recording sampled game state at 1 Hz. Its stage rates, fields,
sizes and the other findings that motivated the chunked recording are
measured in [the original 1 Hz recording note](notes/2026-11-18-replay-original-1hz.md).

## What the 1 Hz recording looked like

With one sample a second, at uneven times:

- Paths cut corners. A strafe or a circle around a pillar becomes a
  straight line between two points a second apart. Exponential smoothing
  (`smoothPlayers`, rate 18/s) adds a lag on top.
- Short events vanish or move. A jump that takes 0.7 s can fall between
  two samples. Shots, actions and hits are summed over the second and
  drawn at the recorded frame's tick, so a burst shows up as much as a
  second late. A kill is on its own tick, but the victim's last position
  is up to a second older.
- Animation is guesswork. Walk or run comes from speed averaged over a
  second, airborne from a stance that can miss the whole jump, aim from
  one sample.
- Vehicles bank and turn between two orientations a second apart.

No viewer polish fixes this. The track has to get denser first.

## Proposal: the spectator track

Moved to [Replay fidelity: the spectator track](replay-fidelity-spectator.md#proposal-the-spectator-track).

### Principles

### What a sample carries

### Rates

### Recording: chunks in the blob store

### Budget per minute of game

### Compression (decided: the work goes here)

### Chunk container, index and reads

### Interpolation and extrapolation in the viewer

## Animation state (found, static reading)

Every object has an animation manager inside its own data, and the frames
read its first channel. Addresses are guest virtual addresses in the
retail XBE, read with `h2 disasm`; no game was run. The reader is
`memory::animation`, and its fixture test is `unit_motion_fields`.

| what | where | evidence |
|---|---|---|
| manager | `object + i16 [object +0x12a]`, -1 none | `object_new` (`0x000b7b40`) builds it there (`0x000b80e0` into the constructor `0x001cafc0`) and gives it the model's graph (`0x001cb0d0`). The custom animation start (`0x001101e0`, reached from `custom_animation_relative` `0x002a3600` through `0x0011b520`) checks the offset against -1 and works on `object + offset`. 113 instructions read it sign-extended. |
| channels | manager `+0x00`, `+0x20`, `+0x40`, `0x20` bytes each | The constructor sets up three alike. The animation start (`0x001ccae0` into `0x001cb410`) passes the manager's own address to the channel set (`0x001c6470`), so the first channel is the one playing; the other two blend out the animations before it. |
| graph | manager `+0x68`, `jmad` datum | `0x001ccae0` and `0x001cb410` refuse a manager whose `+0x68` is -1 and hand it to the channel. |
| channel graph | channel `+0x00`, `jmad` datum | Written by `0x001c6470`; `0x001c6c80` and `0x001ccb40` refuse -1 and index the tag instances (`[0x004e3b44]`, stride `0x10`, data at `+8`) with it. |
| animation | channel `+0x04`: `i16` graph (-1 the channel's own, else a position in its inheritance list), `i16` index (-1 none) | `0x001c6470` refuses a reference whose high half is -1. `0x001daea0` resolves it (docs/engine/engine-map.md, "Animation graphs"). |
| frame | channel `+0x1c`, `f32` | The frame setter `0x001c6560` clamps it to 0..frame count - 1, stores it, and advances it by the tick length times the rate at `+0x18` (1 by default). |

Animations are named on the host from the loaded map's file, as tag names
are: `TagNames` keeps each `jmad`'s animation names (string IDs through
the map's string table) and its inheritance list. The guest is read for
the datum, reference and frame only, at no cost beyond four reads per
unit. Frames carry the result as `anim: {name, frame}`, for example
`{"name": "combat:rifle:any:move_front", "frame": 12.5}` (packed version
4: the name as an interned string, the frame in 1/16).

The viewer's clips
come from the same names (docs/live/live-art.md), so it can play the
game's own animation at the game's own frame instead of guessing from
stance and speed.

Not yet established: what channels 2 and 3 hold during a blend, and how
the unit-level overlay block at `object + i16 [object +0x33e]` (aim and
look overlays, read by `0x0010e530`) is laid out. The first channel alone
gives the base pose. Validate on one game: a standing player should read
an `idle` animation whose frame loops, and a running one a `move_*` one.

## Why bodies floated, and the viewer now (built)

Players and AI hovered above the floor, slid, and floated in mid-air in
and around vehicles. Five causes, each fixed where it starts:

1. **Straight lines between sparse samples.** Recorded frames are about a
   second apart, and the sampler drew a straight line between them. A
   runner over a crest went through the air above it; one going down
   stairs or into a dip hung over it. On a synthetic hill (the fixture in
   `web/test/live-motion.test.ts`) the line is more than 0.15 units off
   the ground. Now, when both samples carry `vel`, the path between them
   is a cubic Hermite curve that leaves and arrives at the game's
   velocities (`frames.ts`, `hermite`; gaps up to 2 s; longer ones, and
   older senders without `vel`, stay straight).
2. **Easing on top.** `smoothPlayers` eased every sample towards the last
   shown place (18/s), so a body walking downhill trailed above its slope.
   A sample with a velocity is already on the game's path: it is now taken
   as it is (aim still eased). Older samples keep the easing.
3. **No floor.** Even a good curve between two ground points isn't the
   ground. A biped the game has on its feet (stance `stand` or `crouch`,
   alive, not riding) is now put on the level's surface under it when
   there is one between 0.6 below and 0.35 above where the samples have it
   (`viewer/ground.ts`). The probe is a downward ray on the level glb,
   machines left out, once per biped per 0.08 units moved, at most 16 an
   update, the surface's plane used in between. In the air, dead or
   riding, a body stays exactly where the frames put it. On the fixture
   hill the feet are within 0.02 of the ground at every sample.
4. **The model's own origin.** The extractor stands each biped on its
   lowest vertex, but the game draws a model at its origin, which is what
   the frames' position is. Extractor 22 writes how far it lifted the
   model (`asset.extras.lift`) and the viewer lowers the model by as much
   (`prepareBiped`), so the feet are where the game's are, not on top of
   the lowest vertex. Art before 22 has no `lift` and draws as before.
5. **Riders drawn at the vehicle's origin.** A rider's position is the
   vehicle's (the telemetry replaces it), and the viewer hid riders
   behind the vehicle. A rider is now drawn at the vehicle model's
   `seat_` node for their seat, turned with the vehicle, in the seat's
   pose (below). The node comes from the seat table (`seats` in the art's
   models table, extractor 22, by the frame's `seat` index), else from the
   animation mode's role (`warthog_d` driver, `_g` gunner, `_p` passenger,
   `scorpion_p_lf` the left-front passenger), else (older senders) a
   mounted turret's rider is its gunner and a vehicle's first rider its
   driver. With no node or no seat pose in the glb (art before 22, a
   Phantom with no seat markers) the rider stays hidden, as before.

**The game's animation.** A frame's `anim` (version 4) names the
animation the game plays, `mode:class:type:action`. The viewer maps it to
the glb's clips (`anim.ts`, `gameAnim`): `idle`, `walk`, `run`, the
strafing and backing-up states `run_left`/`run_right`/`run_back` and
`walk_*`, `crouch` and `crouch_run`/`crouch_left`/`crouch_right`/
`crouch_back`, `airborne`, `land`, `land_hard`, `grenade`, `melee`,
`reload`, `fire`, `switch`, `die`, `dead`, and for a rider `seat_<mode>`.
It plays that clip **at the game's frame**: the clip time is
`frame / 30` (a frame a tick), wrapped for loops and held at the end
otherwise, and the clip doesn't run on by itself, so it keeps the game's
pace and stops with the replay (`BipedAnimator.setLoopAt`).

Between
samples the frame is interpolated when one animation runs on at about the
game's pace, else carried on from the nearer sample a frame a tick
(`blendAnim`). A strafe or a backward run plays its own clip with the
legs facing the aim; art before extractor 22 falls back to `run`/`walk`
with the legs turned to the movement as before. Without `anim` (older
senders, or a name with no clip state) the stance-and-speed guess stays.

Speed and leg heading come from `vel` when the frames have it.

**Shots on their tick.** A frame with a `fire` mask has its shots drawn
as the clock passes each one's tick (`fireTicks`, `ShotQueue`), the fire
clip with them, instead of the whole frame's count at the frame's tick.
Older frames keep the count.

What the viewer doesn't use yet: `body`/`shield`, `actions_ago` and hits'
`ago`, and live extrapolation past the newest frame.

## The theater

theater.md records a game's **inputs** (pad reads, network receives,
randomness) and, at the engine layer, **state keyframes and per-tick
actions**. It plays them back through the game itself: exact, with the
game's own cameras, but only on a machine that runs the same build. The
spectator track records **observed state**.

It's lossy, and it plays
anywhere: the website, a phone, a stream overlay. Your hunch is right. The
spectator track is the web and spectator form, and films are the exact
form.

They should share these:

| shared | how |
|---|---|
| **The game tick** | One clock per game: the engine tick plus a per-run offset, cut at each revert, with the cut's engine tick recorded. A film keys its streams by engine tick and run; the spectator track by game tick. The cut records map one to the other, so a film and a web replay of the same game land on the same moment. |
| **The event model** | `PlayerKilled`, `ScoreChanged`, `GameEnded`, plus the cut, on the game tick. The theater uses them as the timeline's marks and bookmarks; the web uses them for the scrubber and the killcam. |
| **Chunking and keyframes** | The film's `KEYF`/`INDX` chunks and the spectator `FrameChunk` both mean "keyframe, then forward". Seeking works the same way in both: nearest keyframe at or before, then decode or simulate forward. |
| **Film to web** | A film can be played headless with the telemetry readers attached, which gives a complete 30 Hz spectator track with everything in it. The theater can export any film to the web that way, even one recorded offline. |
| **The readers** | The telemetry memory readers (`memory.rs`) are the theater's "roster and match facts" source. |
| **The viewer UI** | The web transport (scrubber, cuts, kill marks, speeds) and its camera ideas (follow, first person from aim, killcam) are what a web film page needs. The in-game theater drives the game's own director cameras instead. |

What isn't shared: the spectator track can't re-simulate. Its free camera
looks at reconstructed models in the web's level render, not the game's
renderer. That's fine for a browser, and it's the reason films exist.

A film could also carry a spectator track as one more chunk type (`SPEC`),
so a film opens on the web without being re-run.

## The viewer: what "the best representation" needs

In order of what it would change for someone watching. Items 1–3 need the
denser track; the rest don't.

1. **Motion you can trust.** 30 Hz players with Hermite interpolation and
   no smoothing lag. Animations driven by the real animation state, and
   jumps and lands on their real ticks.
2. **Firing as it happened.** Each shot at its tick with the right
   tracer, slow projectiles flying their real path, grenades arcing, hits
   flashing on the victim when they land, and a health/shield bar over
   each player.
3. **Vehicles that drive.** Orientation at 15–30 Hz, players sitting in
   their seat (driver, gunner, passenger), turrets aiming from their own
   `aim`.
4. **A first-person camera.** Through the followed player's eyes, from
   their aim (pitch included, already captured), with their weapon. Halo
   is a first-person game, and an orbit or chase view doesn't show what
   the player saw.
5. **A killcam and a director.** "Watch kill": jump to 5 s before, from
   the killer's view, at 0.5×. An auto camera that cuts to whoever is
   fighting (hits and kills in the last seconds), for watching without
   steering.
6. **A timeline that tells the story.** Marks for kills, deaths, the
   followed player's deaths, multi-kills, zone (BSP) changes and revert
   cuts (`ReplayTimeline.cuts`, shown as a seam). Hovering shows a
   preview. Frame-step keys (`,` `.`), J/K/L, 0.25×, and jump to the next
   or previous event.
7. **Load once, play at once.** Stop fetching every frame twice (read the
   chunk track, or for old games one of `/frames` or the `GameFrame`
   events, not both). Start playing when the first chunk arrives, and fetch
   the rest around the playhead.
8. **The 2D map as an analyst's view.** Trails (the last 5 s of each
   path), kill lines from killer to victim, a heat map of time spent,
   sightlines from aim.
9. **Say what's missing.** When a game has only 1 Hz data, say "low
   detail recording" in the corner. A debug toggle that draws the raw
   samples as dots shows how much of the motion is interpolated.

## Plan, after review

Each step is test-first and runs in both homes.

1. **Done:** the game clock, the origin fix and the viewer guard.
2. **Done, viewer on today's data:** frames read once (`/frames` packed,
   events with `exclude=GameFrame`, so the browser no longer verifies the
   server's `GameFrame` signatures; the chunk events will be signed),
   kills, deaths, cuts and zone changes on the scrubber, frame and mark
   stepping (`,` `.` `<` `>`), 0.25×, a first-person camera (`Eyes`, key
   `v`), and a killcam (click a kill or death mark, or `k`): 5 s before, at
   0.5×, through the killer's eyes, opening the 3D view if it is closed and
   reaching a popped-out one.
3. **Done, protocol:** `vel`, `seat`, `body` and `shield`, `fire` (a
   32-tick mask), `actions_ago` and hits' `ago`, and the frame's `cut`, in
   JSON and packed format version 3 (versions 2 and 1 still read; a v2
   golden stream proves it on both sides). Dropped frames' one-shots are
   placed again against the delivering frame's tick. The telemetry session
   sends `cut` on the first frame after a revert; the other fields wait for
   capture (step 4). Filled, they cost about 25% on the sample frames
   before any of the compression work.
   **Done, research:** the animation state is found ([Animation
   state](#animation-state-found-static-reading)). It goes out as `anim` in
   packed version 4, because version 3 frames may already be recorded.
   Hits now carry the tick the damage hook saw them on (`ago`).
   **Research, alongside (decided):** find each unit's animation state in
   guest memory (the animation graph, its current state and frame) by
   static reading and the existing memory fixtures, and record it in the
   protocol as `anim`. The viewer stops inferring once senders have it.
4. **Capture:** players every tick, actor rates by distance, projectiles
   and grenades as entities, and the animation state from step 3's reader.
   **Done (2026-11-19, packed version 6):** projectiles slower than 100
   units a second (grenades, rockets, plasma, needles; stuck ones with
   their parent) and loose weapons and equipment (dropped guns, the flag,
   ball and bomb) as entities, vehicles' velocity, and position
   prediction from velocity in deltas (step 1 of Compression, in the live
   format itself, so chunks get it too). The animation state went out in
   version 4. Left: players every tick (the `partial` frame in
   [live-telemetry.md](live-telemetry.md), "Rate"), actor rates by
   distance, projectile owners (not traced).
5. **Server:** `FrameChunk` recording to the `BlobStore`, read through the
   API with the same visibility check as `/frames` (decided), the chunk
   list endpoint, the compressed chunk codec and its benches, and the
   parity case. **First cut done** ([Chunk container, index and reads](replay-fidelity-spectator.md#chunk-container-index-and-reads)):
   recording, the index, reads, codec 1 (deflate) and chunk playback.
   New recordings use chunks. H2AR now provides the separate compressed,
   seekable format for finished games. The legacy `/frames` read still
   serves older recordings. [Archive migration](replay-migration.md)
6. **Viewer:** the chunk reader, Hermite sampling, one-shots on their tick,
   and live extrapolation.

## Decisions

- **Retention:** keep every game at full detail. Put the effort into
  compression ([Compression](replay-fidelity-spectator.md#compression-decided-the-work-goes-here)),
  with byte and decode-time targets proved by benches.
- **Legacy `/frames`:** the viewer uses chunks or H2AR for newer recordings.
  The legacy endpoint serves older games in packed or JSON form.
- **Chunk visibility:** chunks are read through the API with the same
  check as `/frames` (friends-only co-op games stay private), never from a
  public bucket URL.
- **Animation state:** find the reader now, as research beside step 3.
