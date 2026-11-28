# Agent API: driving the running game without pixels: detail moved off the page

Evidence and detail for [agent-api.md](../agent-api.md).

## 2026-11-26: detail moved off the design page

Read at commit `c7ed1e40c`. These blocks are the page's text at that commit, verbatim, moved here when the page was cut down to its design. Reproduce with `git show c7ed1e40c:docs/runtime/agent-api.md`. They describe the code as it was at that commit; the page states the current design.

### Connecting: socket permissions and stale files

  Local clients use an AF_UNIX stream on Unix and on Windows 10 version 1803
  or later. Unix sockets have mode 0600. Windows socket files inherit the
  directory's ACL, so custom socket paths belong in the user's private directory.
  A live listener retains its path. A stale socket file can be replaced at
  startup. On Unix, `h2 serve` removes its socket on Ctrl-C or SIGTERM.

### Connecting: settings of a headless run

  says so. `--settings DIR` on `serve` and `run` builds the store as
  `h2 play` does (`boot::setting_defs::load`, `crates/apps/cli/src/boot_cmd.rs`):
  `DIR/settings.toml`, then the `H2_*` variables, then the defaults. A
  setting changed in a menu is written back to that file, so pass a copy.

### Finding a game: per-process socket details

selected by `Host::discoverable_agent`). On Unix its directory has mode 0700,
so only the user can connect. On Windows it inherits the profile directory's
ACL. A windowed macOS game with an enabled agent also listens at this
per-process socket when it has a custom endpoint, so the [lab](../lab.md) can
find it. Both sockets serve the same agent. A game started any other way
(`h2 serve`, `h2 run`, the Xbox app) opens a socket only where `H2_AGENT` or
`--socket` says. `H2_AGENT=off` keeps `h2 play` from opening one
([switches.md](../switches.md)). A client lists the directory
and asks each socket `session.status` (`discover::scan`); a file nothing

### Wire: stamp activation rules

After adoption, `activation` identifies the full game start the editor
observes. A reload receipt reserves its own value. Only the successful
start for that transition publishes it. A checkpoint revert, a restore
or another start cannot satisfy the wait. `map_ticks` counts from the
start and its first tick reports one. A machine snapshot saves and
restores that count. Before adoption, `activation` is zero and
`map_ticks` counts from the map name change.

### Wire: phase details

  cleared when the map changes). A level's first tick comes before its
  scripts start the opening cutscene, so ask for
  `{"phase":"gameplay","map_ticks":30}` to mean "playing". A fade leaves the phase unchanged.
  `ui.audit` reports menu transition state. During a load the mode reads as none.

### Wire: boundary details

  agent feature observes `GAME_TICK` last, after telemetry and lockstep
  (`crates/apps/boot/tests/feature_plan.rs`).

### Live mirror: frame format, camera layer, capture budget

Deltas identify records by salted object handle, player index or view key and carry
field masks and removals. A player joining or changing name requires a
key. The length on each layer lets readers skip unknown layers. The
binary layout and blocking decoder are in `agent_proto::live`. A frame
starts with a little-endian u32 payload length, capped at 8 MiB. The
payload starts with a kind byte, three u64s (sequence, game tick, map
epoch) and a flags byte (paused, cut). Each section has a u8 layer id
and a u32 byte length.

Objects carry their definition, type, header flags, parent, transform,
velocity, vitality and scenario placement index. A child's transform is
relative to its parent, as in `sim.objects`. Players carry their name,
team, unit, held weapon, aim and local user.

`camera` (layer id 7) carries each view of the last sealed frame scene
(`frame_scene::View`; [framescene.md](../../graphics/framescene.md)
section 4). A view's key is `kind << 16 | player << 8 | view`, the kind
indexing `game`, `cinematic`, `alternate`, `first_person`, which
`agent_proto::live::View::name` writes as `render.scene` does
(`game:0:0`). Each view has the scene's game tick, position, forward, up,
vertical field of view in radians, near, far, the viewport (top, left,
bottom, right in guest pixels), the cut flag and the salted handles of
the objects it drew, in draw order. The stream worker reads the scene,
not the game thread, so a camera can be a rendered frame behind the tick
it travels with. The game's scene glue registers the source
(`Agent::set_camera_source`, `scene_glue::install_cameras`). Without one,
or when boot reports that this run seals no scene (native rendering or
the frame scene off; `Agent::set_camera_unavailable`), `refused.camera`
gives the reason. Before the first sealed scene the layer is empty. The game thread fills reusable
buffers. A worker diffs and encodes them and each socket has a bounded
writer queue. `skipped` counts missed captures and `dropped` counts
frames lost on the way to the writer. Capture percentiles are in
microseconds over the last 150 successful captures, against a 150 µs budget. They are null before the first capture.

A game in a remote Live match refuses a subscription and closes an
existing one on entry. The embedded service used by `--live-local`
supports the mirror. For exact-tick comparison and a replay with and
without a subscriber, use `scripts/check-live-mirror.py` with a gameplay
snapshot and its matching input log. It prints the hash comparison,
object count, byte rate and capture timings.

### Lab consent: request details

The lab uses `session.takeover_info` to distinguish a windowed game
from a headless agent. Windowed player runs register
`session.request_takeover` and `session.takeover_status`. Windowed
replays and lockstep runs report the window without offering a prompt.
The request
uses the usual agent action access, so an unrelated connection holding
control can block it. `name` is the job's name. `operation` is
`"build"`, `"run"` or `"sim"`, and `builds_image` says the job builds the
image first. `source` is `{"kind": "branch", "name", "commit"}`,
`{"kind": "rev", "rev", "commit"}` or `{"kind": "current", "commit"}`
(the installed image; `commit` may be null). `level` is the run's level
or null, and `own_data` says the run plays on the player's own data.
`estimate_seconds` is an estimate, with zero for an unknown duration.
`timeout_seconds` is wall-clock time from acceptance, from 1 to 3600.
The request is refused with `INVALID_PARAMS` without a requester, a name
or a source. The game shortens the requester, name, level and source for
display. A second request returns `BUSY`
while a prompt is queued or open.

### Lab consent: alongside

`alongside` (a boolean, true when left out) says whether the dialog offers
**Start alongside**. The lab sends `false` for a job whose numbers need a
quiet machine (a benchmark).

### Lab consent: status and dialog rows

alongside** answers `alongside`: consent to start the job at once with the
game left open, so the lab doesn't call `host.quit`. The dialog's rows are
**Not now**, **Start alongside** and **Close and start** (screen
`lab.takeover.alongside`, class `lab-takeover-alongside-dialog`), or only
**Not now** and **Close and start** (`lab.takeover`) when `alongside` is
false; a game never answers `alongside` to a request that withheld it. A request expires even if the UI is still loading. The game
dismisses the expired dialog at the next UI update. The status method is
read-only and answers `GONE` for an unknown or superseded id.

`ask` is `"start"` when left out: the consent prompt above. With
`"ask": "running"` the request asks about a job already running ([When the
player opens a game](../lab.md#when-the-player-opens-a-game)), in its own
dialog (`lab.running`, class `lab-running-dialog`). Its rows are **Finish
this one**, **Keep running** and **Stop and pause**, answering
`finish_this`, `keep_running` and `stop_and_pause`; Back also answers
`finish_this`, and an unanswered question `timed_out`.

### Window pause and frame step: intro

`host.pause {on}` is the window's own pause, the one Cmd+P toggles, asked
for over the API; `host.pause {}` reads the state, changing nothing.
`host.step {ticks?}` runs game ticks under the pause. They are
meant for `h2 play` and other windowed games, where `session.pause` also
stops the window's event loop. The window stays live: it keeps drawing
its last frame, the debug menu and overlays keep working, and the audio
pauses as for Cmd+P.

### Window pause and frame step: field notes

`paused` is the window's pause (Cmd+P's, whoever set it); `held` says
the game thread has stopped at its pause wait; `tick` is the game's tick
at the last game tick. A pause waits up to 2 s for the window and up to
2 s more for the hold, so `held` can be false during a long load. Cmd+P
and `host.pause` change the same pause, so either undoes the other.
`stepped` counts the game ticks the step ran. A frame that runs several
ticks before the game stops again (catch-up after a slow frame) gives a
`stepped` above `ticks`.

### Window pause and frame step: refusals, handshake, examples

Refusals: no window (a headless game) is `UNAVAILABLE`, naming
`session.pause`; `host.step` while not paused is `INVALID_REQUEST`, with
a step already running `BUSY`, and `ticks` out of range `INVALID_PARAMS`.
A resume (Cmd+P or `host.pause {"on": false}`) or a closed window during
a step answers it with `INTERRUPTED`. Ticks that do not run within 0.2 s
each plus 5 s (at most 2 minutes) answer `TIMEOUT` with `data.stepped`;
the step then ends. Focus pause (`pause_on_focus_loss`) is not the
window's pause: a game paused only by losing focus refuses a step.

Both are answered on the connection's own thread
(`Agent::add_connection_read`), since a held game thread answers
nothing, so they skip the controller check that **act** methods have.
The in-process debug menu sends from the window's own thread, which
cannot wait for itself: there `host.pause` answers at once with the pause
asked for and `held` false, and `host.step` answers at once without
`stepped` while the step runs on (a second step is `BUSY` until it ends). The
window shell (`crates/apps/app/src/shell.rs`) applies each request as
Cmd+P does. The game thread's pause wait (`crates/apps/app/src/lifecycle.rs`)
lets the game run while a step has ticks left and reports when it is held.
An agent ticker counts the game ticks (`crates/extensions/agent/src/sim/pause.rs`).
They meet in `agent_proto::host_pause`. The game stops at the next
slice boundary after the step's last tick.

Every other method is answered on the game thread, so while the game is
held it waits. A request sent while held is answered at the next tick a
step runs; `session.pause` and `session.step` are the way to read the
world between ticks without running any.

```sh
h2 agent host.pause '{"on":true}'
h2 agent host.step '{"ticks":10}'
h2 agent host.pause '{"on":false}'
```

### Window screenshots: detail

`host.screenshot {path?}` writes the window's newest published frame, the
one it is showing, without the debug menu or the overlays, as a PNG, and
answers `{path, width, height}` with what it wrote. With no `path` it
names a new file in the window's screenshots directory (`h2 play`'s is
`screenshots/` under the data root).

Like `host.pause` it is answered on the connection's own thread, with the
window shell's capture (`crates/apps/app/src/shell.rs`, over
`agent_proto::host_shot`): the frame source's newest frame read back and
encoded on the asking thread, so the debug menu's own call works while
the game is paused, and a call from `h2 agent` never waits on the window's
event loop. A headless game has no window: `UNAVAILABLE`, naming
`render.capture` for a frame there. A window still starting, one with no
frame yet, a write that fails, and a default-path shot where the window
has no screenshots directory are `UNAVAILABLE` with the reason.

### Close the window: detail

`host.quit {}` closes the game window as its close button and Cmd+Q do:
the window shell stops the guest and leaves its event loop, and the
process exits once the guest has stopped. It answers `{closing: true}` at
once, before the window has closed; the caller watches the process to
see it go. It takes no params (`{}` or none), and a repeat is harmless.
The lab calls it when the player answers **Close and start** to a
[consent request](../agent-api.md#lab-consent) ([Player consent before a
job](../lab.md#player-consent-before-a-job)).

The game has no save step of its own to run first: a campaign keeps what
its last checkpoint saved, as with the close button. Like `host.pause` it
is answered on the connection's own thread, through the window shell's
hub (`crates/apps/app/src/shell.rs`, over `agent_proto::host_quit`), so a
paused game takes it too. A headless game has no window: `UNAVAILABLE`.

### Resize the window: intro

`host.window.resize {width, height}` sets the game window's inner size,
in physical pixels (the size the game renders at), and
`host.window.resize {fullscreen}` takes the window in or out of
borderless fullscreen on its display, as Cmd+F does. An aspect or
field-of-view check drives a live window through the shapes it covers
with it. A size leaves fullscreen first; `{width, height, fullscreen:
false}` says the same thing, and a size with `fullscreen: true` is
refused, since a fullscreen window takes its display's size.

### Resize the window: detail

`scale` is physical pixels per point (2 on a Retina display). The call
waits up to 5 s for the window to take the shape: a fullscreen change
animates on macOS, and a size asked for while leaving fullscreen is asked
again once the window is out. A shape the window does not take in time
(larger than the display, or a fullscreen size other than the one asked)
answers `TIMEOUT`, with the window's state as `data`.

Like `host.pause` it is answered on the connection's own thread, through
the window shell's hub (`crates/apps/app/src/shell.rs`, over
`agent_proto::host_window`): the shell applies the request on its event
loop and reports the window's shape after it and on every resize. A
headless game has no window: `UNAVAILABLE` (its render size comes from
its command line). Missing or partial params (a width with no height),
a side out of range and unknown fields are `INVALID_PARAMS`.

### world: sources

Objects come from `live-telemetry::memory` and `live_telemetry::things`,
read at the tick boundary. Ids are `player.<index>` or `<kind>.<datum>`
with the datum handle as 8 hex digits, salt included, so a slot reused by
another object answers `GONE` and never another object's data.

### world: kinds, query, tag names, fields detail

- **Kinds** (`agent_proto::Kind`): `player`, `actor` (AI biped), `vehicle`,
  `machine`, `body` (a dead biped), `scenery`, `weapon` and `equipment`
  (lying in the world; held ones are their unit's `weapon`), `control` (a
  device's switch or panel), `crate`, `creature`. The last six come from a
  second pass over the object header table by the header's type byte
  (`Kind::of_object_type`). Projectiles, effects, particles, garbage,
  light fixtures and sound scenery are not listed.
- **Query.** `near` is `{point | player, radius}` and sorts nearest first,
  adding `distance`. `fields` trims each object (`id` and `kind` stay).
  `tag` keeps one definition tag: a datum (8 hex digits, `0x` optional) or
  text its name contains (any case, `/` for `\`). Pages default to 50, at
  most 500.
- **Tag names.** An object with a `tag` also has `tag_name`, the
  definition's path from the loaded map file's names (as `edit.tags`
  reads them); it is absent when the names cannot be read. A name filter
  then matches nothing. The map file is found as `boot::map_override::map_file`
  finds it: by file name first, then by the cache header's name (`+0x198`)
  among the maps folder's files, so a converted map served under a disc
  map's file name is the one read. With two files of the same header name
  the first in folder order wins. A map installed as content on the data
  partition is not searched.
- **Fields.** `pos`, `velocity`, `team`, `tag`, `tag_name`, `vehicle`,
  `weapon`, `name` and `local_user` and `unit` (players), `mounted_on` (a
  turret is a vehicle with `mounted_on` set; its `pos` and `forward` are
  relative to that vehicle), `occupied`, `placement` (the scenario
  placement that made it), `position` (a machine's device position, 0 to
  1) and `physics`.

### world: facing

- **Facing.** `forward` is where an object faces. For a unit it is the
  aiming vector (unit `+0x168`, the vector the held weapon's targeting ray
  follows, `0x000cf3d0`), which turns with the camera; a biped's body turns
  only as it moves, so its own forward (`+0x70`) is `body_forward`. `yaw`
  (degrees about +z from +x, 0 to 360) and `pitch` (degrees up) are
  `forward` as angles. A unit whose aim is not finite or not unit length
  falls back to the body's forward.

### world: physics

- **`physics`** (units): `crouch`, `crouch_request` (unit flags bit 23,
  which stays set after release until there is room to stand), `mode`,
  `airborne` (biped `+0x348` bit 0, likely) and `airborne_ticks`
  (`+0x399`), `hitbox_offset_ticks` and `hitbox_offset` (the offset a melee
  lunge leaves, `+0x3aa`; [superbounce.md](../../game/superbounce.md)),
  `unit_flags`, `biped_flags`, `havok_pos` and `havok_velocity` (the first
  Havok body; [`havok`](../agent-api.md#havok-the-physics-world) has every body and
  contact), and for bipeds `support`: what the biped physics update
  (`0x000dd990`) keeps of the ground (layout in `live-telemetry`'s
  `layout.rs`, `BIPED_SUPPORT_*` and `CHARACTER_CONTACT_*`). `support.flags`
  is biped `+0x349` (`copied`: its bit 4, set when this tick's update copied
  a support contact, `0x000ddeda`); `words` are `+0x354..+0x364` (all
  `ffffffff` for nothing); `point`, `normal`, `contact_point` are `+0x368`,
  `+0x374`, `+0x380`; `ground_state` is `+0x3ec`, the vector the ground
  test `0x001f19c0` measures first; `contacts` are the character contacts
  that test walks (physics component `+0x88`, count `+0x8c`, `0x48` each, at
  most 32): `key`, `other`, `point`, `normal`, `value` (the first pass
  wants at least 1e-4), `flags`. The airborne bit is decided from these,
  unlike `havok.contacts`, which lists the solver's contact constraints.
  Bipeds also get `standing_fits`: whether a standing capsule fits at the
  origin, the test that decides whether letting go of crouch clears
  `crouch_request`. It is the superbounce mode's capsule test against the
  loaded map's collision (`crates/extensions/game/src/superbounce_capsule.rs`),
  so it is there only while that mode is on and its scan is ready
  ([superbounce.md](../../game/superbounce-landings.md#charge)).
  Poll `object player.0` on one `h2 agent -` connection and keep one reply
  per stamped tick for a movement trace, or use [a trace](../agent-api.md#tick-traces-and-the-flight-recorder).

### world.breakable: intro

`world.breakable` reads one structure BSP's breakable-surface state at the
tick boundary: which pieces of breakable glass are broken. The bots'
break step reads the same state (`bots::guest::breakable`;
[bots.md](../../game/bots.md#breaking-the-glass)).

### world.breakable: bullets

- `enabled` is the byte `breakable_surfaces_enable` writes, the first byte
  of the state at `[0x4ed280]`.
- `mask` is the BSP's 32 bytes from `[0x4ed280] + 1 + 32 * bsp`, lowest
  breakable index first, in hexadecimal. A set bit is a whole surface.
  The game fills the mask with ones at map reset and clears a bit when its
  surface breaks ([collision.md](../../formats/collision.md#what-the-engine-does-with-a-bsp)).
- `broken` lists the indices whose bits are clear, ascending. A surface's
  index is its collision surface's `+5` byte, used when flag bit 3 is set.
- `UNAVAILABLE` while the state's pointer is null (no map) or the read
  faults. A stray parameter is `INVALID_PARAMS`.

### world.ai: detail

`h2 agent world.ai` returns `{at, globals, actors}` at one game boundary and takes
no parameters (`crates/extensions/agent/src/ai.rs`). `globals` is `{on, initialised}`,
the two bytes of the AI globals (`[0x4f55d0]`, `+0` and `+1`) that
`ai_update` (`0x1c83c0`) needs set before it runs any actor, or null
before the globals exist. Each allocated actor has its
salted AI `datum`, its `unit` ID (for `world.object`), character tag
datum, team, scenario squad index, current behaviour ID, its retail name
when available (`behavior_name`), and combat status. A missing unit,
squad or behaviour is JSON null. The actor datum and unit ID have separate
lifetimes: compare the same unit across ticks to measure movement, weapon
and vehicle changes.

`sector` is the actor's cached sector index in the active BSP when in
range, otherwise null; `location_cached` distinguishes a pending refresh
from a completed lookup. The observer only reads this cache. Retail
`0x0026c180` refreshes it, and `0x00297e15` passes its sector to the mesh
walk at `0x0026c590`, which bounds it against the mesh's sector count. The
behaviour ID comes from the top of the four-frame stack used at
`0x001a813d`; its label from the loaded executable's parallel name table
at `0x0046f0c8`. Combat status is the signed value the `ai_combat_status`
builtin reads per actor (`0x00276070`). `focus_object` follows the
actor's selected perception record to the object datum used at
`0x001b368a`; the fight behaviour reads the same record at `0x001af0c2`.
Compare it with the player's `unit` in `world.query` to see attention on
the player. A remembered object can stay selected after contact is lost.

### net.replication: rates and feature

Event and traffic counts accumulate over the process lifetime. Divide
the difference between two calls by the ticks between their `at` stamps
to get a rate:

```sh
h2 agent net.replication > before.json
h2 agent wait '{"map_ticks":900}' 60000
h2 agent net.replication > after.json
```

The event counts come from the `agent-net` feature, an observer on the
engine's event entry (`SIM_EVENT_GENERATE`, `0xb5a70`);
`H2_FEATURES=-agent-net` leaves them at zero and the rest of the answer
unchanged. The engine calls that entry only in a distributed game. On a
host and a client of one game, `objects` answers whether an object the
host made reached the client: the host's `with_entity` rises and so does
the client's `objects` for that group.

### actors.net: intro

**`actors.net`**

`h2 agent actors.net` takes no parameters and returns the actor channel
of [ai-networking.md](../../game/ai-networking.md) as this machine ran it
on the last tick (`crates/extensions/agent/src/actors.rs`). The game
publishes the report once a tick (`game::actor_net::report`): a
distributed host after it schedules its peers, a distributed client

### actors.net: client detail and example

On a client, `actors` counts the actors in the replica and `puppets`
lists those whose body has arrived through retail replication as
`{entity, character, squad, team, posture, unit}`, with `unit` the local
salted handle. `waiting` is the actors whose body has not arrived and
`lines_waiting` the dialogue lines waiting for their speaker.
`digest_misses` is the current run of disagreeing digests,
`key_requested` a key wanted and not yet asked for, and `diverged` the
channel closed for the match (`actor_channel_diverged`).
`host_log_head` is the last digest's host log head in 64 hex digits,
null before the first digest.

To check that both ends of a Live Firefight agree, read the host's
`firefight.state` `log.hash` and the client's `client.host_log_head`
within 30 ticks of each other:

```sh
h2 agent --socket "$HOST" actors.net
h2 agent --socket "$CLIENT" actors.net
```

### `havok`: the physics world, superbounce polling recipe

For a superbounce, poll `havok.contacts {object: player.0}` once a tick on
one `h2 agent -` connection next to `object player.0`: the contact with
`b.fixed` is the level, and `impulse` over the body's `mass` is the
velocity it gave. A type-`0x17` wrapper reports `kind: "other"` in
`havok.bodies`; its current pill is the pointer at `shape+0x30`
([Shapes](../../engine/havok-runtime.md#shapes)). A direct biped capsule's
`shape.height` tells standing from crouching.

### `havok.agents` field bullets and bounds

- `entry`, `island` and `collidables: [a,b]` are guest addresses in native
  pair order. `object_side` is `a` or `b`, and `entity` is the requested
  object's participating rigid body. `other` identifies the partner
  entity, with its `datum`, object ID and fixed-island flag where
  available; it is omitted for a partner without an entity. `pair_word`
  keeps the last four bytes of the pair record.
- `cache` has `base`, `vtable`, `manager`, `kind` and `hex`. Addresses,
  keys and raw words are eight lowercase hex digits. `hex` is two digits
  per guest byte from `base`, including the allocation header and padding.
- A `bv_tree` or `welded_bv_tree` also has `tree`: `min` and `max` (the
  cached query box in the tree's frame) and, per immediate child, `entry`,
  `key`, raw `word_04` and its own `cache`. A type-1 structure MOPP key
  also has `surface`, the numeric collision BSP surface index. A filtered
  child is `kind: "empty"` with its shared header.
- A `convex` or `swept_convex` cache has a `convex` block: `vertices_a`
  and `vertices_b` (active support vertex IDs in that agent's dispatch
  order; for convex against tree, A is the convex body and B the selected
  tree shape), `contact_ids` (the child manifold's IDs in the shared
  manager), `separating_axis` and `separating_distance` (the bound used to
  skip a closest-feature search while positive; non-finite values are
  null). `swept_sides` on the swept agent records A in bit 0 and B in
  bit 1.
- Unrecognised agents are `kind: "other"` with their inline bytes. The
  bounds are 32,768 scanned pairs, 512 children per tree, 4 KiB per
  allocation and 256 KiB of raw cache bytes per request; a failed read or
  an exceeded bound is `UNAVAILABLE`.

### `havok.agents` reading notes

To compare ticks around a crouch, match a child by its partner and
complete `key`, and record both agent bases to see replacements.
Allocators can reuse an address between samples, and the raw dump
contains pointers, unused slots and padding. A cached surface with an
empty manifold is still a candidate in the query box.

### `superbounce`: state row `targets`

| `targets` | the seam targets ([superbounce.md](../../game/superbounce.md#targets)): `mode` (`common`, `likely` or `all`: the option's value, or the kinds the player switched to with a double tap of the flashlight), `count` (the seams `mode` shows on this map), `tag` (the tagged seam's point; absent with none) `marked` (the seams with a target navpoint, two at most: `second`'s in the flight after a bounce, none otherwise; the set's other seams are lines on the map) and `second` (absent when empty: in the flight after a bounce, the seams it can reach, the best first, each with `at`, `time`, `steer`, `reach`, `speed`, `need`, `cost` and `caught`; [superbounce.md](../../game/superbounce-game-type.md#the-second-bounce)) |

### `superbounce`: state row `guide`

| `guide` | the [approach guide](../../game/superbounce.md#the-approach-guide) on the seam the player is going for, absent while it is not shown: `seam` (its landing point), `call` (`off_line`, `lined_up`, `too_shallow`, `too_high`, `off_edge`, `no_drift`, `crouching`, `will_bounce`), `jump` (the player is on the ground: the landing is the jump's from the takeoff's line), `impact` (the pipper), `row` (the last free row), `ticks` to it and `speed` on it (units/s down), `caught` (within the catch), `deep` with `depth` (the margin under the depth cut; negative is too shallow) and `past` (the row's place past the edge along the drift), `across` (the speed along the drift), `chance` (the bounce's predicted [chance](../../game/superbounce-guide.md#the-chance) in a fall, 0 to 1: the call's confidence) and `chance_capped` (the seam has no measured landing and `chance` was cut to 0.6), on the ground the [jump cue](../../game/superbounce.md#the-jump-cue): `jump_in` (game ticks until the box says `Jump now`, 0 on the line, while it counts), `cue` (the box's words, settled: `Climb up`, `Walk to line`, `Jump in 0.7 s`, `Jump now` or `Walk off`) and `takeoff` (the [takeoff](../../game/superbounce.md#the-takeoff): `at`, the line's centre on its floor, `heading`, the way to run over it, `half`, half the band along it, `speed`, the ground speed it is for, and `walk`, the cue is not a count), and `strength` (the fade, 0 to 1). The fields from `impact` to `speed` are absent with no landing by the seam, `deep`, `depth`, `past` and `across` on a seam with no drift, `chance` on the ground, while not primed, on a seam with no drift, with no landing by the seam and in a fall slower than 5.2 units/s, `impact` to `speed` on the ground too unless the cue counts, `takeoff` and `cue` in the air, while not primed and where no takeoff is found for the seam, and `jump_in` too while the cue is not a count. The guide is absent while not primed |

### `superbounce`: state row `medals`

| `medals` | every player's medals this game, bots included ([superbounce.md](../../game/superbounce.md#medals)): `players`, a row for each slot that has a chain or a medal (`player` the slot, `chain` its last chain's count, `live` whether its next bounce adds to it, `counts` each medal id's total), and `recent`, the last 8 awards, oldest first (`player`, `tick`, `chain`, `medals`: the ids, lowest first; and for a bounce `strips`, how many bounce strips were within 0.5 of its touchdown). The ids are `superbounce`, `double-bounce`, `triple-bounce`, `quad-bounce`, `chain-bounce`, `bouncetacular`, `bounced` and `monster-bounce` |

### `superbounce`: medal_test example outputs

```sh
h2 agent superbounce.medal_test '{"chain":6}'
# {"player":0,"chain":6,"medals":["superbounce","bouncetacular"],"line":"\ue2a5 Bouncetacular!"}
h2 agent superbounce.medal_test '{"chain":2,"monster":true}'
# {"player":0,"chain":2,"medals":["superbounce","double-bounce","monster-bounce"],"line":"\ue2a8 Monster bounce!"}
h2 agent superbounce.medal_test '{"bounced":true}'
```

### `superbounce`: verify_status fields and per-kind trial fields

`trials`, `ok`, `status` `proven`, `failed` or `untried`, its `best`
trial and `why` the others failed, by count), `trials` (left out with
`brief`), `trace`, `lines` (one a spot), `saved` (the verified file
written) and `error`. With `wait_ticks` (at most 290) it answers when the
batch ends or that many ticks pass. A crouch trial carries
`crouched_first`, `under`, `forced`, `primed_tick`, `primed_at` and
`closest`; a drop `contact_tick`, `speed_in`, `vz_out`, `landed_at`,
`word_before` and `landing`; a route `route`, `steps` and its
touchdown. `superbounce.verify_cancel` (access: act; no params) stops the
batch, lets go of the seat and answers with the report; a cancelled batch
writes nothing.

### `superbounce`: reload behaviour and answer fields

their spots again), and the mode's own copy of the loaded map's spots,
which its marks, HUD, assist and tracker read. The `.spots` file is taken
when it is the current scanner's (its map fingerprint is not checked); a
missing, damaged or other-version one leaves the scan as it was, and
`spots_file` says so. The mode keeps its own scan of the loaded BSP, so a
changed `.spots` reaches the bots and a changed `.verified` or `.learned`
reaches everything. The call runs on the game thread at a tick boundary.
The answer is `map`, `dir`, `spots_file` (`read`, `absent` or
`unread: <why>`), `before` and `after` (the scan as the bots keep it with
the files applied: `total`, `crouch`, `slot`, `crease`, `vee`, `busy`,
`likely`, `proven`, `failed` and `marked`), `verified_entries`,
`learned_entries`, `ghosts_before`, `ghosts_after` and `mode_updated`.

### `superbounce`: tune accessor detail

more). A key left out stays as it is and `null` clears its override. Every
key is checked before any is applied, and an unknown key is refused. The
model reads each constant through one accessor
(`superbounce::speed::bounce_speed`, `excess_gain` and
`superbounce::catch`, over `superbounce::tune`), so an override reaches
the scan's `need`, the launch, the landing verdict, the assist and the
bots at once. Overrides live in memory only and are gone with the

### `profile`: capture, causes, native counters, setup counters, fallbacks, flat RAM, faults, sampling detail

- **`profile.capture`** keeps every frame-stats row over the window and
  answers `frames`, `fps`, `frame_ms` and `busy_ms` (frame less pacing) as
  mean, p50, p95, p99, max and total; the same for each phase (`cpu`,
  `decode`, `encode`, `submit`, `gpu-wait`, `pace`, `render-wait`, `d3d`,
  and `render` when the render-timing feature measures it); mean, max and
  total of each counter (`draws`, `passes`, `uploads`, ...); `over_budget`,
  the frames longer than `budget_ms` (default 33.3); and `spikes`, the
  worst of them (20 by default, at most 500), each with its present
  number, game tick and map, its own phases and counters, its three
  biggest phases (pacing left out) and `causes`. Every frame is also
  written as a CSV row (present, end, tick, map, total and busy us, each
  phase in us, each counter) under the cache directory's `captures/`, and
  the reply gives its path. A frame belongs to the last tick that had ended
  when it did. By ticks the window is 1 to 36000 (it gives up after 600 s
  with `complete: false`); by `ms`, 1 to 600000. One capture at a time; it
  is read-only. With frame stats off it answers `UNAVAILABLE`
  (`crates/extensions/agent/src/capture.rs`).
- **Causes.** The reply's `causes` counts every frame over budget, listed
  or not, by cause (`crates/extensions/agent/src/capture.rs`, `causes`):
  `shader_compile` and `pipeline_compile` (any on the critical path),
  `texture_uploads` and `gpu_readback` (more than twice the window's
  median, and at least one), `file_io` (the guest's file calls took at
  least 2 ms of host time in the frame, or at least a tenth of its busy
  time: the `file_io_us` counter; bytes read alone never count), `gpu`,
  `render_thread` and `pushbuf_decode` (that phase is at least 40% of the
  frame's busy time), else `guest_cpu` when the guest's own time is the
  biggest phase. These are what the counters show, not a proof.
- **Native renderer counters.** `profile.counters` and `profile.capture`
  use the field names of `H2_FRAME_STATS_CSV`. Each response and capture
  CSV includes native columns with activity in its window; the streaming
  CSV writes a fixed header at startup. Each `native_<stage>_` prefix has
  examined and suppressed guest items, refusals by reason, prepare and
  encode nanoseconds, GPU buffers and bind groups created, upload bytes and
  pipeline misses; per-view columns count reference and display ranges
  attempted and committed and invariant failures. For example
  `native_world_v1_display_recorded` can be positive while
  `native_world_v1_display_ranges` is zero when the display copy fails
  validation. Frame captures also include `native_lights_worker_round_trips`
  and `native_lights_reference_passes` / `native_lights_display_passes`.
  Definitions: [native-renderer.md](../../graphics/native-renderer.md#frame-counters).
- During map preparation, `setup_<stage>_worker_ns` sums elapsed time on workers
  between presents for world, lights, fog, water, decorators, objects, layers,
  pipelines, nav and visibility. Concurrent jobs can contribute more than one
  frame's duration. `setup_over_33ms_frames` counts presents more than 33⅓ ms
  apart while measured work overlaps that interval. Use the capture's phase
  breakdown to distinguish a blocked frame from work running alongside it.
  `setup_dispatch_ns` times foreground handoffs for BSP activation and
  publication, object and light models, first-person models, resident decorator
  geometry snapshots and bot map changes.
  `setup_dispatch_stalled_frames` counts over-budget presents whose handoffs
  alone took more than 33⅓ ms.
  `d3d_entries` counts guest D3D entry calls that ran, `d3d_residual_draws`
  the guest draws `d3d` handed to its backend (what no native replacement
  claimed) and `d3d_push_words` the push-buffer words it decoded. They are
  always on: an integer add into the calling thread's frame, or one relaxed
  load when frame stats are off
  ([retire-d3d.md](../../graphics/retire-d3d.md), slice R0;
  [`render.residual`](../agent-api.md#renderresidual-residual-d3d-work) breaks them down).
  For inactive map slots, `map_slot_vfs_wait_ns` times the render-frame hook
  acquiring the shared VFS and `map_slot_check_ns` times its checks for
  changed source files after acquisition. A slot check runs at most twice a
  second and requires an inactive candidate.
  `load.timeline` also sums these counters for the stall and settling periods.
- **Fallbacks.** `profile.fallbacks {top}` (20 by default, 1 to 500)
  returns the fallback interpreter's totals and its entry addresses ranked
  by instructions interpreted, each with hits, x87 instructions and the
  function it is in (the nearest analysed start at or below it). Kept per
  address in `boot::fallback::Stats::by_site`, at most 4096 addresses
  (`sites_full` says when later ones count in the totals only). Slim host
  modules also report `cold_entries` (interpreter invocations for omitted
  owners) and `cold_sites` (distinct owner/selector pairs); the boot log
  names each pair on first use.
- **Flat RAM window.** `profile.memory` answers the counters of every
  runtime's flat RAM window in the process, summed
  (`runtime::window::process_stats`, answered by boot's agent glue;
  [contract-thin-transfers-flat-ram.md](../contract-thin-transfers-flat-ram.md),
  section 4.1). They are always on. `windows` is how many there are and
  `aliased` how many hold RAM (guest RAM is aliasable once a flat-RAM
  module is registered, or from the start with `H2_FLAT_RAM=1` at run
  time). `fixups` counts faulting accesses sent to their slow path; the
  null window a context uses with fast memory off is not counted. `pages_outside` counts mapped guest pages the window does not
  hold: non-congruent with their backing, sharing a host page with one,
  byte-granular or armed by the access trap. Each costs a fault and the
  helper per access once flat accesses are emitted. `congruence_skipped`
  is the physical RAM, in bytes, the congruent allocator jumped over to
  give a fresh allocation the residue of its address. A slow page shows as
  `fixups` rising or `pages_outside` above zero.
- **Faults.** `profile.faults {top}` (20 by default, 1 to 500) is the
  fault census (`runtime::fault_census`,
  [block-clock-imprecise-faults.md](../block-clock-imprecise-faults.md)):
  `generated` (memory faults raised by an accessor generated code called)
  and `runtime` (raised by the runtime or a Rust handler); `ended` (faults
  a run returned to its caller with); `continued` (times guest execution
  went on, on the same context, from the state a fault left) and
  `continued_generated` (of those, generated faults);
  `precise_exits_needed`, true when `continued_generated` is not 0; and
  the `top` fault PCs by count, each with `source`, `count`, `continued`
  and the first fault's `addr`, `len`, `access` and `reason`. Kept per PC,
  at most 4096 (`sites_full`). It counts from the process's start. It also
  names the image's choices: `clock` (`accesses` or `blocks`),
  `imprecise_faults` (module feature 128), and `continued_imprecise`, the
  continuations from a fault whose EFLAGS were inexact (should be 0).
- **Sampling.** `sample: true` also runs the in-process sampler
  (`profile.sample.*`, `boot::sampling`) over the window, and `sampling` is
  what `profile.sample.stop` answers: the module-relative dump
  (`profile.h2prof` under `H2_SAMPLE_DIR`, which `h2 profile remote`
  resolves) and its sample count. Only an h2 built with `--features perf`
  on a host with a sampler has it; otherwise, or while a
  `profile.sample.start` run holds it, the capture still runs and
  `sampling.unavailable` says why ([xbox.md](../xbox.md), "Profiling on the
  console").

### render.frame: recorder with a native frame owner

draw diagnostics use one relaxed load per draw or clear in the D3D
wrapper. On a device with a native frame owner, the recorder waits for
the worker's presentation decision. An original presentation supplies
the D3D report. A native presentation supplies the executed graph's
attachment runs, slots, typed stages, snapshots and `display_frame`
counters. The worker keeps a requested original pass read at its position
among the guest commands. If the native graph takes the frame, its own
pass observer supplies the snapshot. Arming a snapshot preserves the
worker's choice of renderer.

### render.frame: draw depth fields

  can be picked out); and `depth`: `mode` (`D3DRS_ZENABLE`: `off`, `z`,
  `w`), `w` (W-buffered as drawn: `SET_CONTROL0`'s z-perspective bit once
  pushed, else `mode`), `func` and `write` (`D3DRS_ZFUNC`,
  `D3DRS_ZWRITEENABLE`), `viewport` (`[MinZ, MaxZ]`), `scale`
  (`SET_VIEWPORT_SCALE` z and `SET_VIEWPORT_OFFSET` z), `clip`
  (`[SET_CLIP_MIN, SET_CLIP_MAX]`), `full` (the depth target's full scale;
  0 with no depth target) and `clamp` (`D3DRS_DEPTHCLIPCONTROL` clamps
  rather than culls; shown only when on). A W-buffered draw of a vertex
  program stores its w clipped (or clamped) to `clip`, over `full`; a
  z-buffered one its z over `full`.

### render.frame: snapshot file names

  `frame-<pid>-<seq>-pass-<n>.png`. A retry after a failed Swap or
  renderer replacement adds `-retry-<attempt>` after the sequence. Native passes use a `display-` or
  `private-` prefix. Read the path or error in `snaps`. An original pass
  with no known colour size reports that it cannot be read. A completed
  report retains its files. Replacing a pending capture releases its
  provisional snapshots. A snapshot incurs a GPU round trip.

### render.frame: private capture report

The next completed private draw supplies the report. Each row covers
draws and clears on the same attachments, with their view/slot pairs
in `slots` and typed stage names in `stages`. A copy ends a row even
when the following draw uses the same attachments. The `draws` list and
vertex totals describe carried guest draws, and `snaps` reads the chosen
passes before the following graph operations. `private_frame` identifies
the source scene, present and lighting mode. Its `passes` counts logical
attachment runs. The backend's `render_passes`, `backend_draws` and
`submits` include work caused by requested snapshots. The private
recorder measures wall time from graph entry to final readback.

### render.frame_layout: layout and sampled fields, backdrop workflow

`frame.draws[].layout` gives the guest target size and the selected host
colour and depth allocation sizes. The viewport and `clip` use guest
pixels. `display_clip` applies the draw's aspect and scissor transform
and rounds the result to the host allocation's pixel grid. The row gives
the draw's UI transform, vertex-position classification and horizontal
projection correction when present. `window_xy` is the min and max vertex
position before the display fit and subpixel adjustment for supported
small, unindexed draws: the reading the device's UI fit uses to tell a
full-frame pass from a widget (`crates/platform/d3d/src/screen_rect.rs`).

Each `sampled` entry gives the stage, texture header and texture ID,
source surface when known, guest and host sizes, and U and V address
modes. The sampled surface names match the handles in `frame.passes` and
`frame.ops`. For small draws with supported vertex inputs, `uv` gives
the min and max normalized coordinates at the vertices for direct 2D
projective lookups. It includes division by the projective coordinate and
the guest texel-size conversion for linear textures. Dependent lookups,
such as fog distortion, expose their bindings and address modes. Their
coordinates also depend on earlier fragment samples.

Pass snapshots read the selected host colour allocation at its actual
size. The capture records D3D draws and accepted native reservations
that expose a resolved draw packet, including the native menu fans.
Higher-level native stages can appear only in the images. Attachment
sizes describe backing allocations. A draw can use a temporary
attachment, such as the guest-size pair used for mixed-grid draws.
Capturing can synchronize the render worker and flush the GPU for the
requested snapshots.

For backdrop or modal-blur issues, arm a layout frame on each affected
screen, then look up the passes that write the sampled targets. Use
`snap` on those pass indices in a second capture to find the first image
with the artifact. Compare a wide main-menu frame and a 4:3 Choose Profile
frame. The source and destination sizes, vertex positions, normalized UVs
and address modes show where a copy magnifies a smaller raster or samples
beyond the image. The snapshot before that copy shows whether the edge
artifact arrives in its input.

### render.stats: counter explanations

`native_ui_overlay_*` counters. The `_native` and `_fallback` fields and
every overlay counter include zeroes. Another refusal reason appears when
its window count is positive. The overlay's counters are its items by kind
(`items`, `quads`, `text_runs`, `fills`, `glyphs`, `blurs`, `model_views`),
the draws recorded (`batches`) and the items dropped, by reason
(`refused_material`, `refused_clip`, `refused_glyph`, `refused_texture`,
`refused_model_view`, `refused_blur`;
[retire-d3d.md](../../graphics/retire-d3d.md#what-replaces-each-subsystem)).
The HUD counters measure reservations of bitmap quads (`hud_tag_quads`)
and glyph quads (`hud_text_quads`), with `hud_tag_fallback` and
`hud_text_fallback` for declined submissions. `hud_tag_recorded` counts
display-copy recordings for both kinds and `hud_tag_record_failed` counts
failed attempts per target copy, for every draw the overlay captures.
See [the HUD on the overlay](../../graphics/native-post-hud.md#the-hud-on-the-overlay).
Menu shader bitmaps' quads count as `menu_shader_quads`, with
`menu_shader_fallback` for those the UI fan path kept. The fade's, the
bars' and the loading screen's draws count as `fade_quads`, `bar_quads`
and `loading_quads`, with `screen_fallback` for those the UI fan path kept
([the fade, the bars and the loading screen](../../graphics/native-post-hud.md#the-fade-the-bars-and-the-loading-screen)).
The menus' (`menu_*`) are the widget draws claimed at their sorted list
(`menu_claimed`), those left out (`menu_skipped`), the passes placed for
runs with items (`menu_passes`), lists whose left-out draws did not match
(`menu_mismatch`), lists the game draws whole (`menu_refused_frame`,
`_mismatched`), drawn bitmap widgets the game keeps (a meter,
`menu_refused_bitmap_meter`; a scrolling bitmap, `_bitmap_scroll`; any
other state the overlay does not draw as the game does, `_bitmap_state`),
other drawn widgets the game keeps by the draw not read
(`menu_refused_text`, `_model`, `_draw`, `_unreadable`,
`_unlisted`), and lists or runs whose pass was not placed
(`menu_refused_pass`;
[native-post-hud.md](../../graphics/native-post-hud.md#the-menus-on-the-overlay)).
A list's scroll arrows and a button's image and label belong to the
widget's claim. A failed glyph-page fill or bitmap decode leaves the
whole run on the game renderer and counts under `menu_refused_pass`.
An active model widget can contribute a camera and object handles to the
scene overlay while its geometry draws in the nested model view.
The blur under a dialog counts as `modal_blurs` where the overlay's blur
replaced its quads, `modal_blur_replayed` where the game's quads were
given back, `modal_blur_refused_pass` where the host did not arm it, and
`modal_blur_refused_arguments`, `_steps`, `_combiner` and `_targets` for
calls not read
([the blur under a dialog](../../graphics/native-post-hud.md#the-blur-under-a-dialog)).
The object-shadow list, row gather and caster each count calls to their own
entry ([native-light-gather.md](../../graphics/native-light-gather.md), L7).
`counters` is `null` where the frame-stats window is not running.

### render.stats: E3 counter meanings

record. The per-frame counters are `lights_lit`, `lights_shadowed`,
`lights_over_cap`, `lights_beyond_guest`, `sun_tiles`, `sun_tiles_redrawn`, `sun_tiles_offscreen`, `atlas_halved`,
`tiles_refused`, `cones_shadowed`, `cones_flagged_off`, `flashlight_shadowed`,
`shadows_over_cap`, `cones_refused`, `cones_drawn` and `cones_kept`. They
count the E3 renderer's selections and atlas work.
The graph draws the guest's shadow maps as tiles
([e3-lighting.md](../../graphics/e3-lighting.md), "Shadows from local
lights"): `e3.tiles` counts the caster draws into them, `e3.marks` the
receivers compared with the Sharp edge, `lights_shadowed` the tiles (one
per shadowed light of the guest's) and `tiles_refused` the shadow maps
left at the guest's size. The display on the guest's live attachments
draws no tiles. `lights_lit` and `lights_over_cap` are the renderer's
own choice of lights from the scene's candidates (e3-lighting.md,
"Lights: which, and how many"): the lights it chose, and the candidates
its cap of 16 left out. `lights_beyond_guest` counts the chosen game
lights the guest did not draw, and `e3.lights` the ranges of the stages
that draw them (layers 12 and 13 over the structure each light meets),
which only an E3 draw records. The five cone counters are the renderer's
choice of shadowed lights from those it lit (e3-lighting.md, "Built: the
shadowed cones"): the cones it shadows itself, the shadowed lights whose
content flag was off, the views whose flashlight holds a slot, the lit
cones its four slots left out, and the lit lights whose frustum makes no
projection. A chosen light the renderer shadows reads its cone's term:
`cones_drawn` counts the cone tiles drawn for those lights (their caster
draws join `e3.tiles`, their receivers `e3.marks`) and `cones_kept` the
lights left undrawn because their cone could not be. `sun_tiles_offscreen` counts offscreen casters for which the graph
submits a receiver draw, summed across views. A cached tile can have
receiver draws with no redraw. The
guest's light selections are in `render.scene.views`.
Retail graph draws omit `e3`.

### render.capture_clip: switch and writer limits

Clip capture needs headless GPU rendering. The [graphics size switch](../switches.md)
sets the clip's output dimensions. The writer takes one frame per tick,
so `fps` above 30 is refused: display frames between scenes are not
written to clip slots. PNG readback and encoding may slow capture.

### Picture feed: ring slot protocol and event example

picture. It holds a 256-byte header (the newest frame number, the
reader's acknowledgement, a closed flag, each slot's sequence number,
frame number, tick, present, time and size) and three RGBA8 slots
(`agent_proto::picture::Ring` has the layout, the writer and the reader).
Frame `n` goes into slot `n % 3`, written while its sequence number is
odd. A reader copies the newest slot and keeps the copy only if the
sequence was even and unchanged across it (`Ring::read_newest`), then
writes the frame number back as its acknowledgement. The opening
connection gets, after the reply, one event per frame:

```json
{"jsonrpc":"2.0","method":"event","params":{"kind":"picture","id":1,"frame":42,
 "tick":48211,"present":96400,"time_ms":700.4,"width":1280,"height":720}}
```

Events go only to that connection, whatever it subscribed to, and are
dropped (not queued) when its queue is full. The frame is in the ring
either way.

### Picture feed: cost and the idle skip

**Cost and the idle skip.** A worker thread (`agent-picture`), running
while any picture is open, reads the newest presented frame back once per
due frame of the fastest picture, scales it on the CPU by averaging, and
writes it to every picture that is due. A ring whose newest frame stays
unacknowledged for a second (`IDLE_AFTER`) and a stream whose queue is
full are skipped; with nothing due, nothing is read back. The readback
runs on the worker, not the game thread, on the GPU queue the game
presents on. Its cost at 60 Hz is not measured.

### render.marks: marks-stay-on-world check

A headless host has no window and returns no presents. To check that
marks stay on the world at 60 fps, turn at a steady rate with a mark in
view and read twice: an anchor's `x` steps by a similar amount on every
present, genuine and synthetic alike
(`crates/platform/gfx/src/world_lines/probe.rs`,
`crates/apps/app/src/shell.rs`, `crates/extensions/game/src/world_lines.rs`).

### render.markers: field explanations

- `shown` counts the markers the capture drew and `hidden_by_budget` the
  ones in range that the 256 a view left out, the nearest first. A marker
  past its distance fade is in neither. The counts cover every kind; `kind`
  filters only `markers`, which lists the nearest first.
- `thing` names what the marker stands for: `<block>:<index>` for an entry
  of the map, `added:<op>` for one the document added, `problem:<label>`
  for a ground label the last build could not honour.
- `team` is 0 to 7, or null. `anchor` is where the label sits, in world
  units; `render.marks` gives window pixels for the groups the window
  visited. `lines` is the marker's line count. `faded` is a marker of
  another game type, drawn at a quarter.
- With no Forge session (or the monitor off), no wall shown and no present
  since, `shown` is 0 and `markers` is empty. A headless host has no window and draws none.

### render.particles.refusals: example answer

```json
{"refused": 428,
 "samples": {"primitive": {
   "label": "Geometry/primitive", "producer": "Particle", "count": 428,
   "shape": {"entry": "D3DDevice_DrawVertices", "primitive": "TriangleList",
             "vertex_count": 6, "indexed": false, "pretransformed": false,
             "layouts": 1, "streams": 1, "layout_strides": [8],
             "stream_strides": [8], "stream_bytes": [48],
             "attribute_count": 1,
             "attributes": [{"stream": 0, "register": 0, "data_type": "Short4", "offset": 0}],
             "provided_inputs": 1}}}}
```

### render.coop_look: sample timing detail

fell behind. One frame can contain several local users. The render hook
posts each draw and the game thread logs it at its next co-op hook, so a
sample shows up a tick after its frame.

### render.coop_look: guest draws and replays

`director`, `vehicle`, or `no local control`. These are guest draws;
additional display-rate replays reuse the guest frame.

### render.compare: difference image colours

  the difference image goes there (directories made): differing pixels
  in red, brighter the further off; pixels off by 1 or 2 in dark blue;
  the rest the first image at a quarter brightness. Errors are

### render.view.current: use and derivation

  carries the usual `at` stamp. A user who sees a rendering fault in their
  own game gives the coordinator this pose, and `render.view` with its
  `eye`, `yaw`, `pitch` and `fov` (not `at` or `private`) draws the same
  view in a test boot. The pose comes from the seat's view record in the
  game's view table (`spectator_view::drawn_view`: the render camera's
  position, forward and vertical lens, the lens converted to horizontal);
  yaw and pitch derive from the forward vector (`CurrentView::looking`).

### render.view.current: spectator caveat

  `crates/extensions/game/src/agent_spawn_view.rs`. The private camera is a spectator view: a pose replayed from the player's eye also draws that player's own body, which can cover part of the picture.

### render.capture: private draw resolution and retention

  resolution shown for that particular present, including a frame the
  backend showed from its 1x reference. The answer includes the tick,
  present, summary hash, lighting and a fresh PNG path. The most recent
  successful private capture retains its scene for another request with
  the same tick. A failed original Swap gives `UNAVAILABLE` with
  `reference present failed` for that scene. Other retained ticks stay
  available. Pass both paths to `render.compare` to compare the modes
  while play continues.

### render.capture: tick/present rules, file, pass and arming detail

  Without `lighting`, the frame is taken for game tick `tick` (as `at.tick`
  counts it; default the next one, at most 240 ahead). On the D3D path
  the frame of a tick is the present that follows it: the first guest
  present after the tick has ended. `tick` in the answer is the last tick
  that ended before that present, which is later than the one asked for
  when the game ran two ticks in one frame; `present` numbers it as
  `render.frame` and `H2_BOOT_GFX`'s `frame-N.png` do. Without `pass`,
  the image is the presented frame at the render scale, the same image
  `H2_BOOT_GFX` writes as `frame-N.png`, written as
  `frames/capture-<pid>-<seq>-tick-<tick>.png` under the cache directory. That
  needs a run that renders on the GPU with `H2_BOOT_GFX` (use
  `H2_BOOT_GFX_EVERY=never` to write nothing else); without it the answer
  is `UNAVAILABLE`, and under `H2_BOOT_GFX_LIGHTING_AB` too. With `pass`,
  the frame recorder (`render.frame`) is armed at the end of the tick
  before and reads that pass back at 1x as `render.frame`'s `snap` does
  (the file is the snapshot's); it replaces a `render.frame` arming not
  yet done, and one made while it waits makes it answer `INTERRUPTED`. `scene_hash` is the sealed scene's
  summary hash at that present (from `render.scene`'s source); `null`
  with no source registered. The answer waits for the
  frame (up to the agent's 300-tick limit for deferred answers); a
  paused game holds it. A new `render.capture` replaces one still waiting,
  which answers `INTERRUPTED`. Unasked, the cost is one relaxed load per
  tick and per present. Capture does not wait for unresident scene
  content (framescene.md 9.2).

### render.scene: per-view fields, mismatch details, counter lists, carried and census detail

- **`render.scene`.** Map activation or snapshot load clears the last
  scene and any private capture pin immediately. `sealed` stays false
  until a frame on the new timeline seals. The last sealed `FrameScene`
  comes from the source the frame step registers (`agent::render::set_scene_source`, in boot's
  `scene_glue` over `render_bridge::scene_step`): whether
  one has been sealed, its map epoch, frame, tick, summary hash and cut
  reasons; per view (`key` is `kind:player:view`, `game:0:0`, the first
  person's `first_person:0:0`) whether it
  cut, its clusters, instances, parts, rows, guest-selected lights
  (`lights`), all light candidates including those selections
  (`lights_candidates`), candidates with both constant sets and any
  required projection (`lights_with_constants`), shadow maps (its
  lights that cast their shadow), its lights by each layer their passes
  draw (`light_layers`, `"10"` to `"14"`), the items of its typed layer
  draws by layer (`layer_items`) and those the guest still draws
  (`layer_remainder`), the
  instances the guest still draws (`remainder`), its parts by how they
  draw beyond their material (`part_flags`: `stencil_mark`, `strip`,
  `subparts`, `aux_depth`, `fade`, `shader_flag`;
  framescene.md, slice 3), its model decorator draws (one per
  placement), their instances and the draws the guest still draws
  (`decorators`, `decorator_instances`, `decorator_remainder`;
  native-decorators.md), its late passes when they ran (framescene.md,
  slice 5): `fog` (layer 16) and `water` (layer 6), each `{draws, items,
  materials, parts, drawn, kept, device_words}` (whether the layer's own
  gate held, its items in the list, the materials and parts the native
  stage draws, whether the stage took the layer, the items the guest
  still draws, the device fog words of 7 that read), and `post`,
  `{colour, late, view_kind, screen_effects, screen_effects_gated,
  colour_pass, effects}` (`render_screen_effects`' arguments and gates,
  whether the colour pass ran, the screen effects drawn under it); each
  absent when its draw did not run in the view; `carried`, its ops by
  slot from the scene sink's last frame (empty without
  `frame-scene.sink`, and for a first-person view), and `camera_use`
  counts the retained guest draws for that scene and game view,
  including its overlay. `screen` is pretransformed or passes screen
  positions through, `view` has the view's verified projection block
  and viewport, and `unknown` covers the other draws. The map is
  empty for other view kinds or without a retained matching scene.
  For a game view, `visibility_clusters` counts the visible clusters,
  `structure_clusters` the subsequent BSP structure list and
  `visibility_objects` the view's object list; `visibility_source` is
  `rust` when the Rust visibility supplied the clusters and objects (the
  default) and `guest` when it declined for the view
  ([native-visibility.md](../../graphics/native-visibility.md)). All four
  are absent if extraction cannot read the main gather at layer 3.
  A zero is a successfully read empty list. With `{"dump":true}`, the scene
  file has the BSP cluster IDs in `cluster_gathers[i].visible`, the
  object handles in `.objects` (the guest's order, or ascending when
  `.source` is `rust`) and `.structure` for `views[i]`. Its `mode` is 0
  for a portal flood, 1 for a sphere and 2 for the potentially visible
  set. The view's `clusters` range selects the
  layer-3 IDs from the top-level `clusters` array. `views[i].camera` gives
  the vertical FOV in radians, guest projection terms and viewport.
  `visibility_first_mismatch` names the first game view since the map
  loaded whose Rust visibility differed from the guest's: its key, frame
  and tick, the first difference (`cluster 12 guest only`, `object
  0x00050002 rust only`) and the counts each way.
  `light_visibility_first_mismatch` does the same for the first light
  whose Rust visibility differed from its struct's lists.
  `first_person_visibility_first_mismatch` identifies the first differing
  first-person slot count, handle, record count or record in replay order.
  Its counters are `scene_first_person_visibility_checked`, `_equal`,
  `_differ`, `_declined_unread` and `_declined_layout`.

  `view_record_first_mismatch` names the first game view whose
  Rust-built records differed from the guest's and the first field that
  did (`position`, `forward`, `up`, `fov`, `viewport`, `near`, `far`,
  `projection`, `start cluster`, `mode`).
  The top-level `counters` include
  the visibility differential's `scene_visibility_checked` (game views
  whose Rust sets were made and compared), `scene_visibility_equal`,
  `scene_visibility_differ`, `scene_visibility_clusters_differ`,
  `scene_visibility_objects_differ`, `scene_visibility_rust_taken` (views
  that carry the Rust sets), `scene_visibility_ns` (time in the Rust
  computation), `scene_visibility_lens_retail`, `_guest` and `_display`
  (game views by the lens their visibility used: retail, the single-copy
  camera that `VIEW_PROJECTION_TERMS` widened, or the display supplement;
  a single-copy run whose views count under `_display` has views the hook
  did not fit), `scene_visibility_display_views`, `_clusters` and
  `_objects` (what the display supplement added),
  `scene_visibility_display_incomplete` (display-only clusters left out
  whole because a part draws in a layer the supplement has no display
  draw for) and
  `scene_visibility_declined_<reason>` (`unread`,
  `layout`, `mode`, `frusta`, `overflow`, `camera`, `not_main`: views it
  made no sets for); the light views' `scene_light_visibility_checked`,
  `_equal`, `_differ`, `_ns` and `_declined_<reason>` (the same reasons);
  the complete world/model geometry lists' `scene_geometry_lists_equal`,
  `scene_geometry_lists_differ` and `scene_geometry_lists_taken` (whole lists
  dispatched from a verified Rust result for the current inputs);
  and the view records' `scene_view_record_checked`, `_equal`, `_differ`,
  `_guest_lens_applied`, `_guest_lens_retail` and `_guest_lens_other`
  (single-copy views the hook fitted, by whether the guest's own
  horizontal projection scale is the widened one, the retail one, or
  neither) and `_declined_<reason>` (`unread`, `layout`, `cinematic`,
  `no_observer`, `override`). They also include
  (`scenes_sealed`, `scenes_unsealed`, `scene_views`, `scene_no_camera`,
  `scene_refused_writes`, `scene_layers`, `scene_layer_items`,
  `scene_layer_remainder`, `scene_layers_unread` (layer draws whose list
  did not read), `scene_decorator_draws`, `scene_decorator_instances`,
  `scene_decorator_remainder`, and the decorator placement source by view
  (`scene_decorator_placements_equal`, `_differ`, `_no_windows`, `_unread`,
  `_nonresident`, `_virtual_shader`). The grid producer reports
  `scene_decorator_grid_compared`, `_differ` and `_native`.
  The decorator stage reports
  `scene_decorator_views`, `scene_decorator_views_native`,
  `scene_decorator_items_skipped`, `scene_decorator_draws_unready`,
  `scene_decorator_draws_state` and `scene_decorator_views_declined`
  (native-decorators.md),
  `scene_light_extract_views`, `scene_light_extract_ns` and
  `scene_light_extract_unread` (candidate extraction attempts, their
  elapsed nanoseconds and attempts that return no candidates because
  the input could not be read), `scene_light_groups`, `scene_light_groups_whole` (the
  gathered light and shadow layers in the scene, and those with every
  sorted entry in it), `scene_light_layers_alone` (light layers drawn
  from the scene with no handler run), `scene_light_objects_alone`
  (their object entries: model parts drawn with no handler),
  `scene_first_person_light_layers_alone` (of those layers, the
  first-person lights'),
  `scene_host_lights_drawn`, `scene_host_lights_kept` and
  `scene_host_lights_unread` (the host's own lights, ML5, drawn by the
  light passes, made but kept, and whose constants did not read),
  `scene_e3_lights_drawn`, `scene_e3_lights_kept`,
  `scene_e3_lights_unread` and `scene_e3_lights_shadowed` (E3's chosen
  lights beyond the guest's, queued as E3 stages, ready but not queued,
  with no constants to draw from, and handed to the host with their
  shadowed implementations over the renderer's cone shadow),
  `scene_e3_first_person_lights_drawn`, `scene_e3_first_person_lights_kept`,
  `scene_e3_first_person_lights_unread` and
  `scene_e3_first_person_lights_far` (E3's chosen first-person lights
  retail's first-person pass did not record: queued as E3 stages over the
  first-person parts, ready but not queued, with no constants, and whose
  sphere does not reach the eye), `scene_e3_first_person_parts` and
  `scene_e3_first_person_parts_left` (the first-person object parts
  offered with them, and those left out: subparts, auxiliary depth or no
  rows),
  `scene_light_entries` and
  `scene_light_entry_remainder_<cause>` (sorted entries left to the
  guest's handlers, by `scene_extract::light_groups::Cause`: `memory`,
  `callback`, `owner`, `section`, `range`, `state`, `subparts`,
  `material`, `pose`; framescene.md, slice 4),
  `scene_shadow_groups` and `scene_shadow_groups_whole` (the shadow
  groups and those with every sorted entry), also per layer as
  `scene_shadow_groups_<layer>` and `scene_shadow_groups_whole_<layer>`
  for 10, 11 and 14, `scene_shadow_layers_alone_10`, `_11` and `_14`
  (caster, receiver and opacity groups drawn from the scene with no
  handler), `scene_shadow_alone_refused_<cause>` (`remainder`, `stream`,
  `pose`, `program`, `state`, `target`, `callback`, `device`),
  `scene_dropped_shadow_layers`, `scene_shadow_filters_native` (object
  filters replaced by native quads) and
  `scene_shadow_filter_refused_<cause>` (`program`, `state`, `target`,
  `callback`, `device`), `scene_shadow_chains_private` (object masks
  drawn into a private chain) and `scene_shadow_chains_spilled` (of
  those, chains copied back to target 10 for the guest's filter)
  ([native-shadow-layers.md](../../graphics/native-shadow-layers.md)),
  `scene_fog_views`, `scene_fog_gate_held`, `scene_fog_drawn`,
  `scene_fog_parts` and the same four for `scene_water_` (late layers
  written, of those whose gate held, those the native stage took, and
  the parts it took), `scene_post_views`, `scene_post_colour`,
  `scene_post_effects` and `scene_post_unplaced` (post calls whose words
  did not read, and colour passes or screen effects with no post call of
  their view; slice 5), and from `d3d::scene_stage`
  `scene_stages_<stage>`, the scene stages queued to the render thread,
  and `scene_dropped_<stage>`, those it could not record after the guest
  skipped their items, for `world` and `objects` (framescene.md, slice
  2) and `fog` and `water` (slice 5). The world stipple counters
  `scene_world_stipple_records`, `scene_world_stipple_solid`,
  `scene_world_stipple_faded` and `scene_world_stipple_unreadable` count
  the records the sorted handler enters with a placement fade below full
  opacity, and of those how many the display detail drew solid (LOD
  Distance; [occlusion-culling.md](../../graphics/occlusion-culling.md),
  "Projected size and level of detail"), how many still drew a stipple
  pattern, and how many fell back to the record's own byte because the
  placement did not read. They are session totals: the delta between two
  `render.scene` replies is a frame's. The counters
  `layer_plan_{6,16}_refused_<cause>` count fog and water batch refusals.
  Item causes are `build`, `handler`, `record`, `object`, `subparts`, `material` and
  `order`. View causes include `stencil_view`, `palette_view`,
  `fog_words_view` and the water stage's `target_view`.
  The water loader counts refused placement parts by `placement_rows_load`,
  `rigid_program_load` and `placement_streams_load`.
  Sorted range admission reports `native_{fog,water}_walk_examined`,
  `walk_suppressed`, `walk_refused_inputs`, `walk_refused_resume` and
  `walk_refused_reservation` under each stage. The counters
  `scene_submitted_flares_reference_draws` and
  `scene_submitted_flares_display_draws` accumulate flare ranges as
  `GfxBackend::commit_external` appends them to each copy's command
  stream. Private replays also contribute. These counters are available
  with frame statistics and the residual census disabled. The corresponding
  `scene_submitted_sky_{reference,display}_draws` count committed sky strips.
  `sky_plan_admitted` counts strips offered to the device after extraction and
  material checks. `sky_plan_refused_<cause>` counts failures by site, geometry,
  material, transform, view, range, stencil or device. GPU reservation and
  replay outcomes appear under `native_sky_*` in the frame counters.
  `post_effect_refused_<cause>` counts screen-effect preflight refusals.
  A call that needs the view but has no view strip increments `view_target`. A missing or empty
  target 1 size increments `pass_target`.
  Unless
  `frame-scene.layers=off` is in `H2_FEATURES`, `layer_plan_asked`, `layer_plan_drawn`,
  `layer_plan_items` and the plan's outcomes by layer,
  `layer_plan_<layer>_taken` and `layer_plan_<layer>_refused_<cause>`,
  the alternate view's as `layer_plan_alt<layer>_*`, the first person's as `layer_plan_fp<layer>_*`, object items' causes
  named `object_*` (`render_bridge::native_layers`; framescene.md,
  slice 4); a later slice adds
  `scene_held_draws`). With the [scene sink](../../graphics/framescene.md#slice-6-the-renderer-owns-game-view-frames)
  enabled, the device files
  each guest D3D op by the frame's slot (`opaque`, `after_opaque`,
  `after_lights`, `transparents`, `after_post`, `overlay`, `outside`)
  for both the original renderer and the frame owner. `carried` describes
  the last recorded frame:
  `slots`, each slot's `{draws, binds, clears, copies, uploads,
  samples}` (slots with none left out), `side_targets`, each surface
  other than the scene's presented colour and its depth that a game
  view's ops name, `{aux, width, height, format, depth, ops}` (`ops` by
  slot), and `readbacks` and `readbacks_outside`, the guest's reads of
  what was drawn in and outside a game view by kind (`surface`,
  `depth`, `mirror`, `pixel_count`); and the counters add
  `scene_carried_frames`, `scene_carried_draws`, `scene_carried_ops`,
  `scene_side_target_frames` (frames that named a side target in a
  game view), `scene_readback` and `scene_readback_<kind>` (in a game
  view, since the start). `carried_census` keeps transient uses across
  all recorded frames. `frames` counts sink presents from one and
  `game_frames` counts those with guest D3D ops in a game view.
  `targets` groups game-view side surfaces by `width`, `height`,
  `format`, `depth`, `slot` and `operation`. Operations distinguish
  attachment binds and clears, the source and destination of a copy,
  uploads, snapshots, in-place samples and depth samples. `reads`
  groups guest readbacks by `slot` and `kind`, including the overlay
  and outside slots. Each row gives its `count`, `first_frame` and
  `last_frame`. Compare counts at two boundaries to isolate a window.
  On a direct level boot, `carried_activation` records the loaded map, its
  activation present, sink frame and a snapshot of process counters. A row
  whose `last_frame` exceeds `carried_activation.frame` occurred after loading.
  `carried_checkpoint` freezes the census corpus at original Swap 3000.
  It contains `present`, `sealed`, optional `inventory` and optional
  `counters`. It appears once the device has submitted the boundary.
  `sealed` records whether the active map already had a sealed scene.
  The original census supplies an `inventory` with the same schema as
  `carried_census` for original Swaps 1 through 3000 inclusive.
  A device or ordinal gap leaves that inventory absent. When the display
  worker completes Swap 3000, `counters` preserves its `scene_window_*`
  readings for the interval after level activation through that Swap.
  Missing or late activation, a second game start within the interval,
  or a missing original or worker ordinal leaves the checkpoint without
  counters. Later game activity does not modify the frozen values.
  A threaded GPU sink also reports `scene_window_*` counters from one device.
  `start` is its original Swap at activation and `through` is the last
  completed Swap. The interval excludes `start` and includes `through`.
  `owner_enabled` is 1 when this device selects a native frame owner.
  `gaps` counts missing or out-of-order joins. `games` counts completed
  frames that entered a game view or issued a game-view operation.
  `owned` and `ordinary` count those game frames by actual display path.
  `failed` counts failed presentations across the interval. `stage_drops`
  includes guest-side and worker-side stage drops. For game frames,
  `unresident` counts omitted instances and `unknown_residency` counts
  frames without a complete observation. `palette_views` counts game-view
  entries sampled for native map-palette residency. `palette_pending_frames`
  counts game frames where a view entered with a known incomplete palette.
  `palette_unknown_frames` counts game frames where a view lacked a valid
  palette observation or game work had no recorded view entry. Each frame
  retains observations from every split view. The current map's
  `scene_palette_definitions` counts distinct scenario object palette
  definitions. Each is in one of `scene_palette_pending`,
  `scene_palette_no_model` (an explicit null model reference),
  `scene_palette_failed`, `scene_palette_partial` or
  `scene_palette_resident`. Resident means every drawable model part
  has its uploaded geometry and its layer-3 material implementations.
  These gauges appear after a valid palette manifest is available.
  `read_surface`, `read_depth`,
  `read_mirror` and `read_pixel_count` count guest requests in game views.
  `flare_queries` counts original game-view reports installed by the
  owner. `unmatched_flare_frames` counts owned frames where that number
  differed from the original requests on the same Swap. Each read of
  these counters covers a single completed interval, even when the
  worker is drawing a later frame. The guest continues running.
  `configured_scale_<bucket>` counts the raster setting used on each
  completed original Swap. `physical_scale_<bucket>` counts the scale
  of its presented image relative to that original surface's guest
  extent. `game_physical_scale_<bucket>` counts that same observation
  for game-view originals. Buckets use `1x`, `4x` or the other observed
  integer factor. `unknown` counts missing observations. Empty buckets
  are omitted. These histograms also appear in the frozen checkpoint.
  The per-frame `aux` number appears only under `carried.side_targets`.
  The cumulative census is available on the census-only path. Rendering
  workers omit this extra scan. Both paths count
  `scene_carried_game_frames`, the guest presents with a D3D op inside
  a game view. With an installed frame owner, `scene_owned_presented`
  counts successful native presentations and `scene_owned_declined`
  counts presentations that take the ordinary backend path, including
  frames outside game views. `scene_owned_flare_queries` counts
  game-view pixel reports whose native result the owner installed.
  `scene_owned_candidates` counts sealed game frames paired with their
  guest presents and checked by the display worker. For each rejected
  candidate the worker records the first cause. `scene_owned_barriers`
  counts frames whose original commands had to run before Swap, for example
  for a guest read. `scene_owned_mismatches` counts disagreement between
  the seal and the queued commands or Swap parameters.
  `scene_owned_target_refusals` counts missing physical attachments and
  frames the physical backend cannot reproduce at the active scale or mode.
  On a scaled display, `scene_owned_scaled_whole` and
  `scene_owned_scaled_fractional` count owned frames on whole-number and
  fractional grids, and `scene_owned_scaled_refused_<reason>` counts the
  scaled owner's refusals by their first reason: `grid` (an axis
  minifies or passes the scale limit), `two_copies`, `busy`, `sealed`,
  `history` (1x E3 with older pixels), `targets`, `live_views`,
  `samples` and `stages` ([framescene.md](../../graphics/framescene.md),
  slice 8, "Fractional grids").
  The private renderer reports its own publication queue in
  `scene_private_published`, `scene_private_taken`, `scene_private_replaced`
  and `scene_private_waiting`. In paced mode a newer seal can replace an
  unread private scene. In locked mode at most two scenes wait and each
  reaches the reader. These counts describe capture availability.
  With the sink and a render worker, `scene_residency_frames` counts
  presents where the worker observed a draw, a committed scene stage,
  an owned frame or a failure. `scene_unresident` counts distinct
  object handles per present whose dropped work lacked the planned
  GPU model, geometry buffer or material program.
  `scene_residency_unknown_frames` counts frames with an unclassified
  drop, a skipped guest packet or a worker failure. The counters appear
  after the first observation. A post-load zero requires a positive
  frame delta and zero growth in both omission counters. Private
  captures have no effect. Legacy refusals can keep their guest draws.
  Check native ownership across the same game views to measure native
  coverage.
  The raw `scene_readback_pixel_count` includes these requests.
  Compare raw and native counts at the same recording boundaries
  to identify requests that still need the reference.
  Both carried records are absent until the sink records a frame.
  Without the option the sink counters are also absent. With
  `frame-scene.census` in `H2_FEATURES`, `entries` is the entry census
  (`render_bridge::entry_census`; framescene.md, slice 4), totals since
  the start: `by_entry`, keyed by guest entry (`layer.<site>.<layer>`
  with site `scene`, `alternate`, `first_person`, `light`,
  `object_shadow` or `other`, the late passes' fog and water calls
  being `layer.scene.16` and `layer.scene.6`; `light_passes`,
  `object_shadow`, `screen_effects`, and under it `colour_pass` and
  `screen_effect`), each `{calls, d3d_draws, seed_steps,
  effects_seed_steps, seed_unmatched, animation_gets,
  particle_updates}`: the guest draws handed to the D3D backend under
  the entry, the steps of the shared random seeds `+0` and `+4`, calls
  whose seed was not a run of steps, and calls of
  `animation_resource_get` and of the particle systems' per-view update
  under it. Entries nest and each counts what is under it.
  `unbalanced` counts entries whose exit never ran and exits with
  nothing open; `draws_counted` is false when no D3D draw counter is
  registered. Without the option `entries` is absent. With `dump`, the whole scene is written as JSON
  under the cache directory (`scenes/scene-<frame>-<tick>.json`) and
  `dump` names the file. With no source
  registered: `UNAVAILABLE`, "no scene has been built".

### render.parity: reference readback and scale details

  before gamma, at its display scale. The reference requires Retail display
  lighting. An E3 display returns `UNAVAILABLE` because its scaled shadow
  maps use a different filter from the sealed renderer's baseline. The
  worker draws that present's sealed inputs on fresh targets at the
  observed scale. Smaller auxiliary levels retain their guest dimensions.
  The reply carries the sealed scene's `tick` and the pixel comparison
  returned by `render.compare`. The comparison covers the carried
  guest commands. Pixel parity of a typed substitution needs a separate
  reference run exercising that stage.

### render.parity: failed Swap fallback

  When the scaled display also fails to show its final 1× fallback,
  the matching capture and parity request report the failed Swap. A
  later Swap can still supply its own image.
  A request that receives no game frame within the deferred request

### sim.hashes: row computation and window detail

  `impl.sim.metrics-projection` switch) on every tick where the players
  can be read, on any run with an agent socket, rendered or not. It keeps
  the last 108,000 rows (an hour of ticks, about 2.6 MB), dropping the
  oldest (`dropped` counts them); a tick before the last, or more than 30
  after it (a new map, a snapshot load), starts the series again

### render.residual totals prose

- **`totals`.** `draws` (guest draws `d3d` resolved, inline ones
  included), `claimed` (of those, a native replacement committed: an
  admission's recording or a post call's held originals), `residual`
  (reached the backend: the growth of `d3d::device::backend_draws`, so it
  equals the capture's backend draw count over the same window),
  `entries` (D3D entry calls that ran), `push_words` (decoded),
  `state_words` (render-state and texture-state words the deferred-state
  commit applied), `clears`, `binds` (target, texture and palette binds),
  `copies` (frame-buffer copies), `uploads` (texture uploads), `locks`
  (`BlockOnResource`, `BlockOnTime`, `IsBusy`), `queries` (visibility
  tests and results), `presents`, and `d3d_ms` (time in the adapter).
  Totals and producer rows also count committed native rectangle plans
  (`native_rect_clears`) and longer lists using the linear path
  (`clear_refused_rect_count`). Title binds that complete without resource
  destruction increment `native_guest_binds`. Final releases increment
  `guest_bind_refused_destroy`. The helper rows are `scene_clear` and
  `target_select`.

### render.residual callers detail

  the analysis is built. `sites` lists the return addresses, `entries`
  the calls by entry name, `draws` what the entries resolved themselves
  and `inline_draws` what the drain at its entries decoded (words written
  before the call, possibly by other code). At most 4096 return
  addresses are kept; later ones count in `totals` only and
  `sites_full` is true.

### render.residual producers: registry, status and layer lists

  guest_ms}`. Only with `frame-scene.census` in `H2_FEATURES`, which
  places an entry and an exit observer on each producer in
  `residual_census::REGISTRY` (the views, scene passes, sky, object shadows,
  decals, lights, water teardown, clears, target selection, transparents,
  screen effects, post calls, HUD, UI, fade, frame end and loading screen)
  and on the layer draw (`layer.<site>.<layer>`, as `render.scene`'s entry
  census names them). The census switch controls whether `producers` is
  present. Work goes to the innermost
  open producer, so the rows sum to `totals`. Inside a render frame with
  none open it is `unknown` (status `unknown`); outside every render
  frame it is `outside_render`. At each boundary the device first decodes
  the push buffer, so an inline draw is charged to the producer open when
  its words were written. `status` is `native` or `d3d` from the
  registry. Sky, clear, target selection, screen effects and `water_teardown`
  are `partial`. Water teardown can hand off its indexed composite and still
  owns its guest state changes and alpha clear. A layer draw is `partial`
  when a native stage draws some items from the scene and skips their guest
  draws. `residual_census::PARTIAL_LAYERS` lists the scene's layers 1, 2, 3,
  5, 6, 7, 8, 16, 17 and 20, the alternate view's 2 and the first person's
  1, 2, 3, 7, 8 and 20. The `residual` is what the stages leave. `slot` is
  the carried slot the work lands in. `guest_ms` is the producer's exclusive
  time less its D3D time, `null` without
  `render-timing`; it ranks producers and is not a frame time.
  `unbalanced` counts producer exits with nothing to end and producers
  dropped unended.

### game.code: JSON example

```json
{"tier": "fast", "full_speed": false,
 "module": "/…/h2boot/…/fast/libh2game.dylib",
 "stamp": "<the module stamp's first line>",
 "background": {"state": "building", "log": "/…/clang-build.log",
                "elapsed_s": 230, "typical_s": 900,
                "step": "emit: 2.6s, 18662 owners"},
 "restart": {"ok": false, "why": "the full-speed version is still building"}}
```

### Returning to the main menu: wait example

```sh
h2 agent wait '{"phase":"gameplay","map_ticks":1800}' 120000
h2 agent game.menu
h2 agent wait '{"map":"mainmenu"}' 120000 && h2 agent loads 1
```

### Matchmaking transitions: sampling points

`game::agent_glue::matchmaking` samples the lifecycle at `RENDER_FRAME`,
the matchmaking result after `0x706f0`, and the target before the key
cleanup at `0x5fb60`. The hosting update `0x71730` compares the target's
player and roster counts and IDs at `0x71982` before checking map and
arbitration readiness at `0x71a58` and `0x71b0d`.

### host.metrics: reading notes

A platform counter that is unavailable is `null`. Compare active
gameplay windows after loading; a stale sample can survive a gap in ticks.
The CPU fields exclude sleeping; the wall field shows the pace the host
sustained.

### Explain memory growth: how to use the history

Use the memory history to compare the menu, level activation, settled
play, and the return to the menu. For a short peak between recorder rows,
read `host.memory` during the load. `host.memory.heap` breaks out live
Rust allocations from the `native.*.load` and `assets.map_rewrite`
scopes and can sample their allocation stacks.

### Finding a leak after the fact: evidence bullets

- **Flight recorder.** Every 10 s the process appends one JSON line to
  `mem-<pid>.ndjson` in its report directory (`H2_MEM_REPORT_DIR`, else
  `logs/mem/` under the data directory). A row holds `t_ms` (watchdog
  uptime), `unix_ms`, `footprint_bytes`, `soft`, `probes`, `tags` (live
  Rust heap bytes per scope tag), `stacks` (sampled stacks, when
  sampling is on), `gpu` (the census below), `level`, `tick`,
  `agent_connections` and `agent_calls`. Each line is written straight
  to the file, so a kill loses at most a torn final line, which readers
  skip. Segments rotate hourly or at 8 MiB, seven per process. An orderly
  exit appends a `{"closed_cleanly":true}` footer.
- **Reports.** A soft-limit, sustained-growth or hard-limit trigger
  writes `mem-<pid>-<nanos>-<serial>.json` beside the recorder (written
  to a `.partial` file, then renamed, before the OS diagnostics run). The
  hard limit's report is written before the process exits with code 86.
  Under `h2 play --follow --fresh-data` and `--watch --fresh-data`, whose
  data dir (`<cache>/data/current`) is emptied at every launch, the
  child's report directory is `<cache>/data/mem` unless
  `H2_MEM_REPORT_DIR` is set, so a game the watchdog stopped leaves its
  evidence for `h2 mem postmortem <file>` (`crates/apps/cli/src/follow.rs`).
- **Recovery.** A starting recorder checks the directory for streams
  whose owner lock is free. A stream without the footer, or whose last
  row is over the soft limit, moves to `reports/mem-<pid>-<nanos>/` and
  the process logs `memory recorder: recovered pid N (...)`. A clean
  stream is deleted. Recorders and preserved streams share a 512 MiB
  budget per directory. When it is full, a new process runs without a
  recorder and logs where to move the evidence from.
- **Heap tags.** `h2` and `h2-editor` allocate through
  `mem_watch::TaggedAllocator`. A one-word trailer on each allocation
  names the scope tag current on the allocating thread, so a free on
  any thread credits the same tag. Scope guards
  (`let _scope = mem_watch::scope("assets.bitmap");`) sit at the
  suspects: asset and bitmap loads, pipeline creation, the native world
  pass, agent requests and replies, trace and input recording, the perf
  sampler and the editor's map loads and frames. Allocations outside a
  guard count as `unscoped`. The counts are requested bytes; foreign
  heaps (the GPU driver, C libraries) show only in the OS footprint.
- **Sampled stacks.** With `H2_MEM_SAMPLE` set to a mean interval such
  as `1M`, the allocator records the stack of a Poisson-sampled
  allocation and keeps it until the allocation is freed. Large
  allocations are proportionally more likely to be sampled. Unset, no
  stack is captured.
- **GPU census.** Each renderer registers its wgpu device. The census is
  wgpu's `generate_report` counts of live user-held resources
  (`wgpu.texture_count`, `wgpu.buffer_count` and the rest), HAL byte
  counters on Vulkan and D3D12, Metal's `currentAllocatedSize` on macOS
  as `metal.current_allocated_bytes`, and the renderer cache probes
  (`gfx.*`, `native.*`).

### Finding a leak after the fact: postmortem sources

A postmortem prefers the newest dead flight recorder, then falls back to
watchdog and lab reports (`mem-<pid>-*.json`, `lab-host.json`) for a
process that died before its first row. `report` carries the newest
matching report's identifying fields; the full report, with `vmmap` and
`footprint` output, stays at its path in `reports`.

### Finding a leak after the fact: lab and typical hunt

For a lab job, `h2 lab mem <id>` fetches the job's `mem/` directory and
prints the largest growers ([lab.md](../lab.md#memory-limits-and-reports)). A typical hunt:
read `host.memory` for the growth rate, `host.memory.diff` from a time
before the climb to name the growing tag or GPU count, then restart with
`H2_MEM_SAMPLE=1M` and read `host.memory.heap` for the stacks inside it.

### ui: ui.screen detail, dialog names, findings

  In a paused game, Player Settings reports screen 196 or 200. Its Video
  Settings and Voice children report classes `video` and `voice` on
  screen 200. Press B or Back with `h2 agent press` (`input.pad`) to
  check that the previous screen returns on the same window and slot. See the
  [pause Settings walkthrough](../../game/settings-menu-screens.md#to-check-in-game).
  The main menu's one row of ours, Theater, follows System Link, so
  Campaign, Xbox Live, Split Screen and System Link keep their retail rows
  0 to 3 and lobby scripts press them there. `main_menu_theater` and
  `main_menu_rows` (rows of ours between Xbox Live and Split Screen) are
  absent; `agent_proto::sim::menu` still reads them from older logs. `dialog` is the dialog up (the first screen on windows 0 to
  4, any slot), or `null`: `{id, name, window, slot}`, with `name` the dialog's
  recognized name (`dialog_live_signin` for the account picker,
  `confirmation_dialog`, `error_dialog_ok`, ...; `boot::ui_watch::dialog_name`)
  or `null` for an id not named yet. The party leader picker reports
  `dialog.name = party_leader_picker` with id `0xb1`. The Clanker player
  card reports `clanker_player_options`. The retail-style value picker
  used by Theater reports `kit.picker` in the main menu and
  `kit.picker.game` in game. These names identify the active kit class.
  Their numeric ids identify the shared retail base class. Dialogs read as
  `"screen": null`, so this
  is how a driver tells one from another. The `ui: screen none` log line
  names it the same way ([live-coop.md](../../live/live-coop.md), 12.6).
  `layout_findings` contains visible text and row rectangles outside the
  title-safe area or their container. Each entry has `screen`, `widget`,
  `rect` ([top, left, bottom, right] in UI units), `text` when readable,
  and an `issue` with a `kind` (`unsafe`, `outside_panel` or
  `outside_container`). It samples the input screen and the active dialog.
  `content_findings` reports broken strings, dead controls and missing
  content. Every entry has `screen` and `issue: {kind, id, detail}`.
  Findings from instantiated widgets include `widget` and `text`.
  Findings from a registered Rust page model include
  `source: "page_model"` and `screen_instance`. Model findings use the
  currently focused row and the installed fonts when available. The
  read does not activate controls. The
  [UI builder](../../game/ui-builder.md#content-findings) checks each named
  state and exercises offline focus and input routes.

### ui: ui.campaign pick, list and press detail

  `select_level` and `co_op`; other pages report an empty row list. For
  example, a fresh profile on Halo 2 has `"page":"options"`, `"game":"halo2"`
  and three rows. With a saved Halo 2 checkpoint, Resume is the first of
  four rows.
  On the picker and Campaign Options, `pick` identifies the most recent
  press during this visit. It is null before a game is picked. Its
  `campaign` is the picked numeric id, and `request` is null until retail
  receives the options screen request. The request gives the player
  `mask`, `window`, `slot` and `loader` arguments. The active `page`
  shows whether the options screen opened.
  On those two pages `list` reports guest addresses for the `screen`,
  its list `handler`, the `row_array` and the linked `children` in order.
  `row_count` reads the number of active datums. On Halo 2 with a checkpoint
  the four actions scroll through three linked item widgets. The handler's
  `press_record` holds a `press_target` (the list) and a `press_function`
  (the row-chosen callback). An unreadable field is null. On other pages
  `pick` and `list` are null.
  From the main menu's Campaign row, `h2 agent press a` opens the picker for
  fresh and returning profiles. Main menu row zero requests the native
  campaign dialog from `0x230888`; the picker intercepts it before the
  later profile-dependent solo branch at `0x2b16ad`. With UI watch on,
  `lobby mode campaign` also recognizes the installed picker.

### Menu inspection and navigation: audit and transition detail

Inspect `panes[].labels` and `panes[].lists[].rows` for the text at the
widget. The audit reads the UTF-16 buffer *after* retail's localized
string lookup. A row includes its datum index, text and individual labels.
On the retail profile picker and action menu, `navigation` identifies a
verified inspection action in the current list mode.
A missing widget, an empty string and an unreadable text interface have
distinct statuses. The list's `item_count` includes off-screen items.
Rows describe the materialized viewport. Moving focus exposes the other
rows of a scrolling list. The Y menu exposes its materialized tabs and
marks the selected pane. Source and widget layout are in
[`ui_audit.rs`](../../../crates/extensions/game/src/ui_audit.rs) and
[User-interface tags](../../formats/ui.md).

`issues` identifies affected surfaces, panes and rows. It includes
missing or empty row text, empty labels, unreadable data, empty panes
and undersized lists. `transition` includes the startup fade time
remaining, scene fade mode and weight, and the active, pending and
outgoing layers of the menu compositor. A modal can request a blurred
backdrop for as long as it is open. `stuck_blur` means a layer requesting
that backdrop still carries its input-blocking transition more than
one second past its authored delay and duration. Other overdue animations
produce `stuck_transition`. `stuck_fade` means audits sampled the same
unfinished main-menu startup fade and clock reading at least 150 game ticks
apart. Unknown backdrop accessors give `requests_backdrop: null`.
These diagnostics describe compositor inputs. The final image still
requires a rendering check. [Transition reader](../../../crates/extensions/game/src/ui_transition.rs)

### Crawling the menus: crawl behaviour and verdict

It skips game starts, quit, unverified actions and changes to settings or
accounts. `--from-current` starts at the open menu or dialog for a
focused check. Depth zero audits only the current surface. The defaults
are six menu transitions and a two-minute wall-clock budget.

For a parity run, use `--require-clanker` so missing seed history or an
unreached card fails the run. The JSON verdict lists visited paths,
findings, skipped actions, Clanker cards and navigation errors. Findings
or a failed traversal produce a nonzero exit. Save the report outside the
repository because resolved text can contain profile names and disc strings.
[CLI implementation](../../../crates/apps/cli/src/agent_ui_crawl.rs)

### audio: voices spatial and routing detail

`spatial` is null for a 2D voice. For a positioned one it gives the
DirectSound 3D terms the mixer applies against the listener:
`position` as the game set it, `relative` to the listener, `min_distance`
and `max_distance` (null when not finite), `rolloff` (listener times
source), `distance_gain` and `cone_gain` (linear), their product `gain`,
`pan` (left and right gains), `front` (the front crosstalk share) and
`pitch` (the Doppler ratio). `applied_gain` is `direct_gain` times that
`gain`: the level the voice's direct path leaves with, before bin volumes
and pan. Halo's submixes have `FLT_MAX` distances and rolloff 0, so their
`distance_gain` is 1. The game's distance attenuation is the gain on the
stream's bin into the submix, so one emitter's level at a distance is that
bin `gain` times the submix's `applied_gain`.

`direct_gain` and `room_gain` are linear low-frequency gains including
voice volume and headroom. `direct_hf_db` and `room_hf_db` give the
additional loss at `reference_hz`. Each ordered entry in `bins` has its
raw `bin`, `volume_mb` and its linear `gain` including that pair's volume
and path gain. These are routing settings before spatial panning, master
gain and the reverb return. A disabled environment or unconnected bus can
have a configured send gain and deliver silence. Plain stereo routing has
`bins: null`.

### audio.loops: transitions, menu, output detail

`transitions` keeps the last 64 changes in starts, stops, segments and
permutation selection, sampled after the sound frame service. A null
`from` denotes a start and a null `to` denotes a stop. The stamp includes
the retail `tick` when available, the guest `present` counter,
`logical_100ns` and `elapsed_us`. The last field uses a monotonic wall clock
from audio-output creation. History starts over with a new map or a
rewound guest clock. `records_transitions` says whether the observer runs.

In the main menu, `menu` gives `ui_ms`, `idle_ms`, the music gate and
requested and applied music state, the attract timeout, the music stop
threshold from the current map, and whether a movie is active.
`menu_changes` keeps the last 64 changes in the gate, music state, movie
state or timer reset. The threshold applies on the screens eligible for
the attract movie. See [the attract timer](../../audio/bink-hle.md#attract-timer).

`output` reports cumulative sink underruns and silent frames, playout
starves, dropped frames and mixer stream starves. Their last-change times
use the same `elapsed_us` clock as the transitions. The output thread
samples the sink counters as it services the device. Stream starves are
counted when a dry stream receives data again. Optional WMA counters show
codec restarts, errors and dropped partials. The playout fill and target
give the approximate delay between the mixer and the device.

Sample the main menu before and after an audible change. Compare present
numbers in `transitions` and `menu_changes` with the output counters and
their last-change times to identify the selection change, idle-triggered
restart or gap at the device.

### audio.meter: window, play, levels, errors bullets

- **A window.** Meters the latest `window_ms` of output. The capture
  starts with the first request and keeps 10 seconds, so the first call
  answers once that much has been rendered.
- **`play`.** Starts a host clip in the mixer as the game plays it:
  `pogo/compress`, `pogo/spring`, `pogo/settle` or `announcer/<medal id>`
  (an unknown name's error lists them). It answers when the clip has
  played, metering from the call to its end plus 20 ms, with anything
  else the game played meanwhile. `window_ms` is that span. The clip is
  removed from the mixer when the method answers or times out.
- **Levels.** Peak and RMS in dBFS, integrated loudness and the loudest
  400 ms block in LUFS (ITU-R BS.1770-4, the meter Studio's
  `sound.measure` uses), each to 0.1 dB; silence reads -120. `clipped`
  counts samples at or past full scale.
- **Errors.** A clock-only run (a light host or sim) renders no samples:
  `UNAVAILABLE`. So is a clip still a Git LFS pointer, or one over
  5 seconds. Bad params and unknown clips are `INVALID_PARAMS`. Audio
  time is logical, so a paused game does not answer until it runs.

### world.object.animate: call, host only, unconfirmed

- **The call.** The name becomes a string ID of the loaded map; the
  graph's animation block (`jmad` `+0x2c`) is searched for it, and the
  object's first channel (the manager's own `+0x0`) is pointed at the
  entry: the animation word at `+4` (graph -1, the channel's own, and the
  index), the frame at `+0x1c` set to 0 and the rate at `+0x18` to 1.
  It is made at the start of the next tick, like `sim.give`
  ([`sim`](../agent-api.md#sim-simulated-players-and-direct-state)). The animation is
  the graph's own: an inherited one is not found.
- **Host only.** An object's pose is simulation state, so a co-op
  (synchronous) session refuses it as `sim.spawn` is refused.
- **Errors.** A bad or player id, or a name the map has no string ID for,
  is `INVALID_PARAMS`; an id that no longer names the object is `GONE`; a
  graph with no such animation is `INVALID_PARAMS`; an object with no
  animation manager or no loaded graph is `UNAVAILABLE`.
- **Not yet confirmed in a running game.** The channel's fields are the
  ones the engine's channel set and frame setter read and write
  (`live-telemetry` `layout`), but a biped's own animation state restarts
  its channel as its names change, so the method is for scenery, machines,
  crates, creatures and vehicles; the answer's `frame` and a later
  `anim`/`world.object` read are how a run checks it plays.

### recording.play: call flags and errors

- **The call.** The recording player (`0x1fb360`; EAX the unit, CX the
  recording's index, the play flags as its one stack argument, `ret 4`,
  AL 0 when it starts nothing), made at the start of the next tick like
  `sim.give` ([`sim`](../agent-api.md#sim-simulated-players-and-direct-state)). The
  flags are the ones the three evaluators push (`0x2a2d1a`, `0x2a2d7a`,
  `0x2a2dda`): 0 for `play`, 8 for `delete`
  (`recording_play_and_delete`), `0x10` for `hover`
  (`recording_play_and_hover`). The unit moves, so a co-op session may
  not ask (`INVALID_REQUEST`, as for `sim.give`).
- **Errors.** A name the scenario lacks, or a mode that is none, is
  `INVALID_PARAMS` (the first lists the names the scenario has); a
  player with no unit is `GONE`; a recording the player starts nothing
  for (the unit already plays one, or no free slot) is `UNAVAILABLE`.
  A disc multiplayer map has no recordings. A paused game answers when
  it runs.

### `hs.eval` snapshot, builtins and refusals

the scenario's tables (`hs::Scenario::read_memory`), the builtin table
from the running image (`hs::Catalog::from_memory`, named by the map's own
calls) and the live cell of each scenario global. The text is wrapped in
a static script of a copy of the tables (`hs::compile`) and run on
`hs::vm`, the native evaluator ([hs_vm.md](../../engine/hs_vm.md)); nothing
is written back to the game, so a `set` of a global changes the snapshot
only. Control flow, arithmetic, comparison and reads of the map's globals
run. A builtin that needs the game's objects, AI or sound is refused by
name until the guest-backed engine exists, and an expression that sleeps
is refused: the answer comes at one boundary.

### `input`: act like a player, tap order and ports

runs the game through boot's XAPI has it: `h2 serve` (headless pad),
`h2 play` and the Xbox app. It wraps the tap those variables set: a replay
runs first (it stands in for the pad) and agent steps go on top; a
recording runs last, so it logs agent steps too. Taps are told which port
each handle opened on (`InputTap::opened`), which is how a step for
`player` 1 finds controller port 1. Without XAPI input there is no tap and
the input methods are not listed.

### `input.pad` packet number, queue lifetime, open false

  whenever the pad the game gets changes; until the first step, reads pass
  untouched, so a run with the agent on and no steps is byte for byte the
  same input. Queues outlive the connection that filled them (so a
  one-shot `h2 agent press` works) and are emptied when the map changes,
  since the tick starts again. The reply says `open: false` when the game
  has not opened that controller: the steps are kept, but nothing reads
  them.

### Routes: Ascension superbounce setup

**Ascension superbounce setup.** Begin at approximately `(−10.85, 1.16)`
next to the low lip on Ascension. Face −120.7° for the crouch entry. The
route releases the button while pushing into the lip, turns to −66° to
exit, then walks to the upper ledge and jumps toward the floor seam.

```sh
h2 agent route '[
  {"face":-120.7},
  {"crouch_into":[-10.91,0.72,-1.16],"hold_ticks":15,"exit_face":-66},
  {"move_to":[-8.0,2.2,-0.7],"crouch":false},
  {"jump":{"from":[-8.42,2.51,-0.76],"land":[-10.39,4.00,-3.19]}}
]'
```

Compare the recorded jump with the reference takeoff heading `(-0.8, 0.6)`
at about 1.6 horizontal units/s and the landing's incoming vertical
velocity of about −5.4 units/s. See [Superbounce](../../game/superbounce.md) for the traces and
collision geometry.

### `input.host` field detail and notes

- `devices`: each controller as its own backend reads it, before any
  merging (`input::HostBackend::devices`). From Apple GameController:
  `backend` (`game-controller`), `id`, `name` (the vendor name), `profile`
  (the extended profile's class: `dualsense`, `dualshock4`, `extended`, or
  `micro` for an Apple TV remote), `category` (the framework's product
  category), and the state below. Other backends do not describe their
  pads yet; theirs show under `pads`.
- `pads`: each pad the input manager polled, as the debug menu's gate is
  handed it: `backend` (the top-level one, such as `keyboard-fallback`),
  `key` (backend index and id, the gate's key), `name`, `physical`, `port`
  (0 to 3, or null) and the state. With the keyboard on, player one is a
  single pad that merges the first controller with the keyboard.
- The state, in both lists: `touchpad` (null without one, else `click`,
  and `primary` and `secondary` as `[x, y]` from -1 to 1 with +y up, or
  null for a lifted finger), `buttons` (`south east west north
  left_bumper right_bumper back start left_stick right_stick dpad_up
  dpad_down dpad_left dpad_right`; GameController's menu button is
  `start`, Options is `back`), `left_stick`, `right_stick`, `triggers`.
- `gate`: null with no debug menu, else `open`, `syncs`, and per pad `key`,
  `touchpad`, `click`, `chord` (click and Start together), `chord_pending`
  (Start held back waiting for the click), `opened_by_press` (the click
  that opened the menu is still down) and `latched` (buttons hidden until
  let go) ([debug-menu.md](../debug-menu.md)).
- `notes`: where a touchpad goes missing, in words: a plain `extended`
  profile, a DualShock 4 profile without touchpad elements, a touchpad
  under `devices` but on no pad the gate sees, no debug menu.

### `input.lightbar.set` apply timing

ownership rules for actions. The desired colour applies at input sync
when that port has a supported controller. It describes the host request
independently of what the hardware displays.

### `edit` and hot `render`: map file identity and Live sign-in

`edit.map.file` and the telemetry map identity follow the **source path
recorded by the active cache slot**. The host resolves a direct `T:` or
`W:` path through the guest's mounted drive link. A DVD slot can carry a
map with a different header name. An installed package with that header
name cannot shadow the DVD source. For an editor copy, `file` names the
immutable host generation for the current activation. When the slot
cannot identify an ordinary map, the host looks up the cache name. Ask again after a
reload before opening a path from an earlier reply.
After the first successful file binding, external Live sign-in gives the
reason “An editor map was loaded. Restart the game to sign in to Xbox
Live.” This applies even after switching maps.

### Raw edits: checks, write order, eviction and refresh

A batch checks every field and raw `was` before its first write. A raw
write is checked against the file view (the map file through the overlay)
and, when the owner's section is paged in, against the resident copy. With
no `was`, the resident copy must equal the file view. A write the game
already holds succeeds. Otherwise error 8 (`MISMATCH`) returns
`data.fields` and `data.raw`, the second with `owner`, `offset`,
`holds_hash`, `resident_holds_hash` and `why`, and nothing is written.

At the boundary the batch writes the tag bytes, then each raw entry into
the overlay and into the resident copy when there is one, then the
evictions. `resident` says whether a resident copy was written; pixels,
sections not paged in and BSPs not loaded take the overlay alone, which
their next page-in or the switch to the BSP reads; sound chunks always take
the overlay alone. An eviction clears the section's loaded flag, cache
handle and runtime section block, or the sound chunk's runtime word, so the
guest's next lookup pages it in from the file view; pixels cannot be
evicted. `refresh` names the native refresh queued on
`render_bridge::native_edits`: `section` (with `grown` when the batch
evicts it), `model` or `image`, and is `null` for a sound chunk and for a
`bsp` owner, whose switch loads the world from the overlaid file.

### Raw edits: overlay lifetime and slot files

The overlay lives for one map. A map change drops it, the raw journal and
every stage at the first tick of the new map, and a play-copy reload drops
the overlay when it binds. The guest reads a map's raw data from the cache
slot file the map was copied into, so the overlay is served on those slot
files and never on the copy's source. A slot copy, a cache template and a

### Collision edits: activation detail and what the commit rebuilds

switch, which `BSP_SWITCH` itself skips for the BSP already loaded and
active: at the next tick, through the agent's guest calls
(`Call::ReactivateStructure`, `crates/extensions/agent/src/sim/calls.rs`),
`STRUCTURE_BSP_DEACTIVATE` (`0x137da0`) then `STRUCTURE_BSP_ACTIVATE`
(`0x137d40`), so every game system rebuilds its per-BSP state (physics
its Havok world over the structure, objects their locations) and the
BSP's tag data, with the new pairs, is not loaded again. A game with no
activation (no agent guest calls) refuses these commits (`REFUSED`,
nothing written); one whose activation does not start or fails (no
structure loaded and active) puts the old pairs back, frees the stage,
and answers `REFUSED` with the reason. The arena extents the old pairs named are freed after the next
activation, listed in `freed`; until then Havok's shape may still read
them. A `coll` BSP's old extents are kept until the map changes, since a
live body's shape points at them until the object is made again
(`world.object.respawn`).

The lifted BSP walks follow the pairs on every query, so rays, sweeps
and point location answer from the new blocks on the next tick, and
`sim_collision`'s page watches over the pairs start a new generation. A
structure commit hands the new BSP to `debug.view`'s `collision_geometry`
and `invisible_barriers`, which are built from the map file and would
otherwise show the old level. A commit on the active structure also
rebuilds the bots' line-of-sight walls from the new BSP on a thread of
their own (`bots::autonav::collision_edited`); their nav mesh keeps the
old shape until the map loads again. The journal does not hold a commit: `edit.revert` leaves it, and the
editor undoes it with the inverse stage and commit. Sessions are those
of the other edit writes; with `confirm_peers` on a host the change stays
local, as tag pokes do.

### World debug views: capture, traces

The window registers the debug views as a world-line source before boot.
Changing a switch, range or lifetime affects subsequent genuine images.
A captured image keeps its geometry, including the object poses and
traces sampled for it. The overlay projects the sample with the image's
split-screen cameras and draws hidden edges at full strength. The source
and capture code live in `crates/extensions/game/src/debug_views.rs` and
`crates/extensions/agent/src/debug_views/mod.rs`. See
[Drawing world-space lines over the frame](../agent-api-edit.md#drawing-world-space-lines-over-the-frame).

Traces observe the named `SIM_COLLISION_QUERY` entry and its return while
enabled. A true hit draws the world position from the result record. The
query's ABI and result layout are in [Collision rewrite, §5.4](../../engine/sim-rewrite-collision.md).
See [Debug views](../debug-menu.md#debug-views) for the data sources, draw caps
and limits. The wire types are in `crates/extensions/agent-proto/src/debug.rs`.

### `editor`: marks capture, drawn reasons, pause behaviour

Each genuine guest image captured while a mark lease is active carries
the bounds sampled from the guest state that renders it, along with
each view's camera and aspect factor.
This applies to serial rendering and repeated
scanout of the same image. The window projects marks over the finished
frame. Hidden edges show at full strength, with the same styling as
visible edges. The candidate style uses dashes. `xray` has no effect
(`crates/rewrites/render-bridge/src/scene_step.rs`,
`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/frame.rs`, `crates/apps/app/src/overlay/marks.rs`).

`drawn: true` reports a current genuine projection. The window presents
a genuine image with at least one shape from the current set and a
usable camera. Individual edges still depend on clipping. `drawn: false`
gives a reason such as a headless host, native rendering or scene
capture being off, a loading picture, a raw submission, an unusable
world camera, or waiting for a render with the set. Marks are also
drawn on in-between frames (60 and 120 fps), from the source image's
samples if it carries the current lease's marks, with the camera the
frame drew each view with
([high-framerate.md](../../game/high-framerate.md), "World marks on made frames";
`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/cli/src/play_cmd.rs`, `crates/apps/app/src/shell.rs`).

A shell pause or paused Theater playback blocks the guest loop, so
editor methods wait to be serviced. An agent pause continues to service
them, and a newly sent mark appears only with the next genuine render.
The window still expires the plate and marks on wall time
(`crates/apps/app/src/lifecycle.rs`,
`crates/extensions/agent/src/exec/poll_theater.rs`,
`crates/extensions/agent/src/exec.rs`, `crates/apps/app/src/shell.rs`).

### Drawing world-space lines over the frame: how the window draws edges

The window draws every edge as an anti-aliased screen-space quad, with the
core, halo and dashes in the fragment shader, and the ribbons, arrowheads
and label pixels as triangles in the same vertex buffer. A HUD-font glyph
is a textured quad sampling the font's atlas, bound once per draw. All of
it is one pipeline and one draw per frame
(`crates/apps/app/src/overlay/marks.rs`). A label's opacity is the
style's `alpha` times its fade and pulse.

### Drawing world-space lines over the frame: in-between frames, no depth test, cost when inactive

Genuine guest images captured with samples carry their own per-view
cameras and aspect factors. Repeated scanout keeps the same samples
and cameras. An in-between frame draws the samples of the newest
genuine image, or of the one before while a blended replay is under
half way, with each view's camera moved onto the one the frame drew
the world with ([high-framerate.md](../../game/high-framerate.md), "World
marks on made frames"). The window draws
all visible samples together in a 2D pass over the finished frame.
There is no depth test, so hidden edges draw at full strength
(`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/overlay/marks.rs`, `crates/apps/app/src/shell.rs`).

An inactive producer costs an atomic poll. When all producers are
inactive, there is no capture, guest read for the overlay, or sample
allocation. The inline collection holds two additional samples
before growing. Active sources read only the supplied guest view
and shared host data
(`crates/extensions/game/src/world_lines.rs`,
`crates/rewrites/render-bridge/src/scene_step.rs`).

### Bot combat: write counters, facing_releases, profile weights

The write counters show how often the roster work changes the session's
membership, which makes the game rebuild what depends on it.
`write_frames` counts frames that wrote the membership and moved its
counters: a join, a removal, or a repair. One frame's repairs share one
bump. `props_writes` counts phantom peer properties rewritten to follow
this console's. `identity_writes` counts phantom names or looks written
back into the session's player records. `handicap_writes` counts phantom
handicaps put back to none there ([bots.md](../../game/bots.md), "A phantom's
player configuration"). `team_moves` counts team
rebalance moves. `props_diff_chunks` is a mask of the 8-byte chunks of
the `0xc8`-byte peer properties that differed on any rewrite: bit `i`
covers properties bytes `8i` to `8i + 7`, so bit 12 is the map status at
`+0x60`. In a settled lobby `Δwrite_frames` stays near zero. If it grows
with `Δframes`, the game is undoing a phantom write every frame. The log
then says `bots: phantoms: the session's membership was rewritten on 30
frames in a row (...)` once per run of such frames
(`crates/live/bots/src/phantom.rs`, `churn_line`).

`facing_releases` is counted on the action gather, not on render frames.
It counts built actions after which the host cleared a phantom biped's
facing-set flag, as retail's control update does for a local player
([bots.md](../../game/bots.md), "The facing release"). It rises by about
one per melee lunge. A bot whose aim stops following its right stick
while this stays flat points at a facing write the release does not
cover.

`profile` contains the card's `summary`, its `archetype`, `favorite_weapon`, the name's
`try_hard` score and the `weights` active for that difficulty and style.
The full vector contains `radar_use`, `noob_combo`, `dual_wield`,
`aggression`, `grenades`, `melee`, `weapon_preference`, `range_preference`,
`strafe`, `jump`, `reaction_jitter` and `retreat`. Each value ranges
from zero to one. Higher reaction jitter favors the later part of the
difficulty’s reaction window.

### Bot combat: long combat field rows

| `last_reaction_ticks`, `total_reaction_ticks`, `reactions` | Ticks from the selected target's first fight observation to the first attack decision on that target. The mean is the total divided by the sample count. |
| `think_ms`, `total_think_ms`, `think_ticks` | Wall time for the latest brain decision and controller update, accumulated time and sample count. Guest reads, worker queueing and retail input processing fall outside this interval. |
| `goal_decisions`, `standing_decisions` | Decisions per goal for the match, and those whose left stick (under 0.2) stood the bot still. `standing_decisions.recover / goal_decisions.recover` is the share of shield recovery spent standing. |
| `look_asked`, `look_stalled` | Decisions after one whose right stick asked for a turn (over 0.3), and those whose observed aim had not moved since. A healthy bot stalls only for its input latency. A stall share near one means the look is not reaching the unit ([bots.md](../../game/bots.md), "The facing release"). |

### Bot combat: accuracy ranges and rows

mid is 3 to under 12 and long is 12 or more. An unknown range means
the shot had no sampled visible target distance. Each row includes
`damage_code`, `weapon`, `range`, `shots`, `hits` and their ratio
`accuracy`. One shot can earn one hit credit. An empty list means the
host has no registered shots to report for that seat.

### Bounce routes: practice tally, training round, knobs

([bots.md](../../game/bots.md#practising-bounces)). In a sim's training run
([bounce-search.md](../../game/bounce-search.md)) `practice.train` gives the
round the bot flies: `site` (the stand its learned line keys), `stage`
(`arms`, `shift`, `compass`, `scatter`), `axis` (in `compass`: `aim_along`,
`aim_side`, `from_side`, `yaw`, `back`, `run`, `early`, `gain`, `letgo`,
`drift`, `still`), `slot` (the try's, from 0) and `tries` (the site's
scored tries). `knobs` is what the route's site is flown with, by the
learned file's keys (`how`, `fx` `fy` `fz`, `ax` `ay`, `yaw`, `back`,
`run`, `early`, `gain`, `letgo`, `drift`, `still`; only those set): its
learned line's, or a training try's; `null` for the route's own. The
types are `crates/extensions/agent-proto/src/bot_bounce.rs`
(`LearnedKnobs`, `PracticeTrain`), filled in
`crates/extensions/game/src/sim_bots.rs`.

### Bounce routes: example commands

```sh
# a bot standing at x, sent to y over a ledge, bouncing on the way
h2 agent sim.state.set '{"player":2,"pos":[-12.0,3.0,-3.2]}'
h2 agent bots.bounce_to '{"bot":"Clanker","to":[-10.6,9.0,2.4],"mode":"fun"}'
h2 agent bots.bounce_status '{"bot":"Clanker"}'
h2 agent superbounce.spots '{"map":"ascension","kind":"crouch"}'
```

### player.goto: example commands

```sh
h2 agent player.goto '{"to":[-10.6,9.0,2.4],"within":0.2}'
h2 agent player.goto_status '{"wait_ticks":280}'
h2 agent player.goto '{"to":{"spot":0,"kind":"crouch"},"then":"crouch_recipe"}'
h2 agent player.goto_cancel
```

### Saved places: example commands

```sh
h2 agent player.mark '{"slot":"ledge"}'
h2 agent player.recall '{"slot":"ledge"}'
```

### sim: clankers[].aim fields

For a joined phantom, `h2 agent sim list` includes `clankers[].aim` once
the host has gathered input for its current unit. `tick` is the gather
tick. `stick` (right, up) and `trigger` are the brain's latest report;
`arrived_stick` and `arrived_trigger` are the report delivered by the
uplink for that gather. `action_deg` is the accepted slot record's
absolute [yaw, pitch] in degrees, or null if the gather was rejected.
`fire_primary` is the accepted primary fire bit and `assist_strength` the
two weapon strengths; a rejected gather reports false and zeros.
`unit_forward` is the unit's actual aiming vector at query time.
`assist_player` identifies the player controlling the selected aim-assist
target when one exists. To verify damage, read the target's shield and
health with `h2 agent sim view player`.

### sim: sim.objects field list

has `id` (its salted handle, `0x` and eight hex digits), `kind` (`null`
outside the type table), `definition`, `flags` (the header flags set:
`active`, `awake`, `post`, `new`, `deleting`, `defer`, `attach`, `child`),
`parent`, `position`, `forward`, `up` and `scale` (parent-relative under a
parent), `velocity` (units a second), `sphere {center, radius}`,
`vitality {body, shield, depleted}`, `nodes` (node matrix count) and, for
bipeds and vehicles, `unit {player, actor, team, seat, weapons, held}`.
The wire types are `agent_proto::sim::objects`.

### sim: sim.events buffer notes

section 8). The game answers at a tick boundary, so the events are the
tick that just ran, in the order they happened. The buffer is emptied when
each tick starts. `kinds` filters by type. An unknown name returns
`INVALID_PARAMS`. Datums are `0x` and eight hex digits,
or `null` for none. Each event has a `type`:

### sim: sim.events recording switch

Events are recorded only while `impl.sim.native.objects` is on; with it
off the list is empty. The records and where they are read are in
[object-update.md](../../engine/object-update.md), "Slice 8 body". The wire
types are `agent_proto::sim::events`.

### h2 agent sim and scripts: example script

```sh
cat > bot-team.txt <<'X'
sim session
expect /kind == "mp_host"
sim add phantom script xuid=ROSTER_XUID gamertag=ROSTER_GAMERTAG
wait {"phase":"gameplay","map_ticks":30}
sim until 600 /sim/1/player exists
sim set sim.1 team=1 health=0.5 pos=[12.0,-3.5,2.0]
expect /wrote contains "pos"
sim view sim
expect /sim/1/player exists
sim pad sim.1 [{"left":[0,1],"ticks":60}]
X
h2 agent sim run bot-team.txt
```

### The game's own functions: teleport, give, spawn detail

- **Teleport** (`sim.state.set` with `pos`, `yaw`/`pitch` or `facing`) is
  HaloScript `object_teleport`'s body (`0x29ffb0`) for a player's unit,
  with our position in place of a cutscene flag's. The position goes to
  the co-op placement `PLAYER_PLACE_NEAR` (`0x14e970`; EBX the player, ESI
  the point, anchor -1, as `0x2a01ec` calls it), which takes the unit out
  of a vehicle (`0xe68c0` action `0x1e`) and places it through `0x14e200` a
  little above the point. The facing: the unit's desired facing, aiming and
  looking vectors (`+0x150`, `+0x15c`, `+0x180`, as `0x2a019e..0x2a01d9`
  writes them), a local player's view (`0x1874b0`: yaw and pitch into the
  player controls, `[0x4ed284] + 0x14 + 0x94 * user`, `+0x10`, `+0x14`),
  then `object_set_position` (`0xb75a0`) with no position for the
  orientation. A seated unit's facing is left (its seat's); the answer says
  so. Other fields of the same call are written at once. The answer:
  `wrote` (with `pos`, `facing`) and `teleport` (`pos` and `body_forward`
  where the game put it, `faced`, `unit`). `yaw` is degrees about +z from
  +x and `pitch` degrees up (-89 to 89), as `world.query` reports them; or
  `facing: [x,y,z]` (also taken as `forward`, `world.query`'s name for it),
  so a check can turn the player to face what a query found.
- **`sim.give`** gives a weapon as a starting profile does (`0xcce78`) and
  as an AI's equipment does (`0x1e05c8`): `placement_data_new` (`0xb7930`,
  owner the unit), `object_new` (`0xb7b40`), then `unit_add_weapon`
  (`0xcd0c0`, stdcall `(unit, weapon, mode)`), deleting the weapon
  (`0xb8540`) if the unit refuses it. Default mode 2: the unit's weapon
  slots are emptied first (`0xccff0`) and it readies the new one. `keep:
  true` is mode 1, the starting profile's: added, held only if the unit's
  own weapon choice (`0xcdeb0`) picks it. `ammo` writes the new weapon's
  first magazine once it is made. The tag is a datum or a name with `/` or
  `\` (names come from the map file, as `edit.tags` lists them: `h2 agent
  edit tags weap`). A bare name is looked for among `weap` tags only, since
  a weapon shares its name with its model, render model, collision and
  animations (`hlmt mode coll jmad`); `group:name` or `name.group` gives
  the group outright. The answer: `weapon` (`tag`, `group`, `name`),
  `object`, `mode`, `held`.
- **`sim.spawn`** makes an object as the netgame equipment spawn does
  (`0x15ab94..0x15abe6`): `placement_data_new` with no owner, the position
  at placement `+0x1c`, forward and up at `+0x28`/`+0x34`, then
  `object_new`. A distributed host with the
  [creature probe](../../game/ai-networking.md#creature-slot-probe) enrolls a
  new creature after allocation. Any object group `object_new` knows (`bipd vehi weap eqip
  garb proj scen mach ctrl lifi ssce bloc crea`). A bare name is looked for
  among those groups; if it names tags in more than one, the answer asks for
  the group (`vehi:<tag name>`). The answer: `object`, `pos` (where it was
  made), `tag`.

### The game's own functions: example commands

```sh
h2 agent sim set player.0 'pos=[36.7,-80.8,1.7]' yaw=90 pitch=-10
h2 agent query '{"kind":["player"],"fields":["pos","forward","yaw","pitch"]}'
h2 agent sim give player.0 <weap tag name> reserve=108
h2 agent sim spawn <vehi tag name> 'pos=[40,-80,2]' yaw=180
```

### Object edits: set, delete, respawn detail

- **Set** passes the position pointer in EAX, forward in ECX and up in
  EDX to `OBJECT_SET_POSITION` (`0xb75a0`), with `(datum, 0, 0)` on the
  stack. The projectile step loads all three registers before its call
  to `OBJECT_SET_TRANSFORM`. [Retail call sites](../../../crates/rewrites/engine/src/sim_objects/projectile_step.rs#L114),
  [register setup and call](../../../crates/rewrites/engine/src/sim_objects/projectile_step.rs#L1767).
  The orientation-only teleport uses a null position.
  A position-only move leaves the orientation pointers null. A partial
  turn reads the missing axis from the body and requires the resulting
  axes to be perpendicular unit vectors.
- **Delete** calls `OBJECT_DELETE` (`0xb8540`) with the datum, as the
  starting-profile weapon refusal does in `sim.give`.
- **Respawn** reads the entry's palette index, salted definition tag,
  position and yaw/pitch/roll. It prepares and creates the replacement
  with the ownerless `placement_data_new` and `object_new` sequence of
  `sim.spawn`. Only then does it delete the old occupant. A creation
  refusal leaves the old object in the game. The method accepts unnamed
  records with zero placement flags, default scale and zero type-specific fields. Other creation fields
  return `REFUSED` before deletion. The preview has `linked: false`.
  The host keeps the preview's salted id and placement in memory. Another
  respawn replaces it, including when addressed by the returned id. A
  fresh multiplayer activation or a change of scenario clears the records.
  A campaign checkpoint can restore an older preview on the same map.
  The host keeps its placement history and refuses snapshots until the
  scenario changes.
  A held weapon or equipment item is absent from the world-object view,
  so its salted id returns `GONE`. Selecting a placement with a held
  or attached occupant returns `REFUSED`. Multiple occupants and previews
  that cannot be identified after a checkpoint also return `REFUSED`.
  Supported blocks are
  `scenery`, `vehicles`, `weapons`, `equipment`, `machines`, `controls`,
  `crates` and `creatures`. Unsupported blocks return `REFUSED` before deletion.
  The placement and palette offsets and record stride come from
  `tags::object::OBJECT_TYPES`. The entry fields and palette stride follow
  `crates/tools/scenario-edit/src/layout.rs`. The guest reads are in
  `crates/extensions/agent/src/sim/calls/placement.rs`.
  The placement builder's index flow is known. Calling it with a full
  scenario record requires its register and stack conventions.
  [Placement index evidence](../../live/live-telemetry.md)

### Placement edits: order, room, linked creation, peers

- **Order.** The host checks everything first: the block's root count
  equals `was_count`, each record's `was` equals the slot's bytes, each name
  exists and is of the block's type, and no object to delete or relink is
  held or attached. A count that disagrees or a stale record answers
  `MISMATCH` (`data.count`, or `data.records: [{index}]`) with nothing
  changed. Then it deletes the occupants of `delete`, rewrites the placement
  index of the occupants of `relink`, writes the records, the name
  repoints and the count, and creates the objects of `create`. A respawn
  preview of a deleted entry is deleted with it, and one of a relinked entry
  follows it.
- **Room.** A block holds as many records as the map loaded it with, or
  the capacity of the edit arena extent it moved to
  ([Block moves](../agent-api.md#block-moves)). The host records the loaded count at the
  block's first edit of a map activation. A removed entry's slot stays in
  the block, so an add after a remove reuses it. A `count` past the room
  moves the block into the arena with twice the room (at least `count`,
  at most the per-type limit): the extent is allocated before anything is
  written, the records and the dead tail are copied into it, and the root
  is repointed with the records and the count. An extent the block left
  is freed. A full arena, or a game without one, answers `REFUSED` with
  `data.need` (entries) and `data.capacity`, and from a full arena also
  `data.need_bytes` and `data.largest_free`; nothing is written.
  `count` past the block's per-type limit (`map::scenario_layout`)
  answers `REFUSED` with `data.limit`.
- **Linked creation.** An object is made by the placement-data and
  `object_new` sequence of `world.object.respawn`, with the entry's
  placement index written into the placement data at `+0x10` before the
  call, so the object carries its placement as a loaded one does.
  `linked` is read back from the made object. The honoured record is the
  one respawn honours: an unnamed entry with zero placement flags, default
  scale and zero type-specific fields. Any other record is written and its
  index comes back `created: false` with the reason; it shows after a
  reload.
- **Blocks the engine walks itself.** Starting locations, netgame
  equipment, netgame flags and starting equipment take records and a count
  only; `create`, `delete`, `relink` and `names` are invalid for them.
- **Count changes with other people.** A transaction that changes the count
  is refused when the session is a System Link host with other people in
  it: a client checks the placement index of a network object creation
  against its own count. Phantom bots do not count. Each is a machine with
  one player in the session's peer count, so the people are the session's
  peers less the host and the phantoms that have joined
  (`Bots::joined_phantoms`). The refusal's `data` carries `people`, `peers`
  and `phantoms`. A transaction that leaves the count alone (a move, a
  relink) is not refused for peers. A Live custom game this console hosts
  follows the same rule. Matchmaking, a client and an unreadable Live session
  are refused before any method runs, by the Live edit check
  (`agent_glue::live_edit_refusal`, installed with `Agent::set_live_edit_probe`);
  without that probe every Live game is refused.

### Local Forge session: forge.status field detail

| `forge.status` (read) | `{}` or omitted | The local session, role, log head, budget, monitor camera, aimed and held IDs, screen and focus, pending call or file work, and last notice. Tool settings: `rotation_snap` (degrees), `movement_snap` (world units, 0 for off) and `axes` (`world` or `local`). `overlays`: each overlay (`markers`, `monitors`, `gizmo`, `nav`) to whether it is on. Links fields: `tool` (`objects`, `links` or `nav`), `aimed_link` and `link_from` (`{teleporter: flag}`, `{flag: id}` for a teleporter the session added, or `{object: id}`), `teleporters` (`{flag, sender, channel, position}` with the channel the document gives), `added_teleporters` (`{id, sender, channel, position}`, the teleporter flags the session added, with the channel a link gives), `links` (`{id, kind:"teleporter", sender, receiver, channel}` or `{id, kind:"device", machines, applies:"rebuild"}`, final indices) and `pending_rebuild`, the active device links. Palette fields: `palette` (the Objects palette's item count) and `palette_refused` (each scenario palette entry left out, as `"path: reason"`). `parked`: the hex handle of the biped the monitor parks and hides, or `null` while none is. Groups fields: `groups` (`{id, name, locked, pivot, members}`, `pivot` `centre` or `first`, `members` the things' endpoints) and `selection` (the endpoints chosen to group). Labels and prefabs: `labels` (`{thing, text, number}`, the active labels) and `prefabs` (`{name, pieces, grouped, fits, reason}`, the saved prefabs, and why one is refused on this map). Things fields: `aimed_thing` and `held_thing`, the thing's endpoint as `forge.things` gives it (an own entry's or an added one's; `aimed` and `held` are an added object's op ID only), `game` (the variant record's engine index the tool reads, or `null`) and `show` (`this_game_type` or `all_game_types`). `variant`: `{name, title, id, tier, dirty}`, the session's document as a map variant saves it: `name` the file it was saved or loaded as (`null` before), `id` the play form's identity (`null`, with `id_refused`, when it does not compute), `tier` `variant` or `map`, and `dirty` edits since the last save or load. `ground`: `{state, building, available, loaded, markers, problems, zones?, squads?}`, the AI ground in brief (`forge.ground` has the rest): `state` is `not_built`, `current` or `stale`, `available` whether this game has a builder, `loaded` whether the last build's map is the running one, `markers` how many labels are placed and `problems` how many labels the last build could not honour. `nav`: `{state, available, generating, problems, sectors?, out_of_map?}`, the last **Generate nav** in brief (`forge.nav` has the rest). An inactive session returns `{session:"inactive", reason}` |

### Local Forge session: ground, nav, things field detail

| `forge.ground` (read) | `{}` or omitted | The AI ground (docs/game/forge-map-variants.md, "AI ground"): `{state, building, available, loaded, recipe, counts, markers, problems, report}`. `state` is `not_built`, `current` or `stale`; `building` is true while a build runs (edits wait for it); `available` is whether this game has a builder; `loaded` is whether the running map was reloaded onto a build of the ground the document asks for now (false after an edit makes it stale, and while the game cannot reload the map). `recipe` is `{kit_version, home, probe}` or null before the first build. `counts` maps each label (`ff_drop`, `ff_base`, `ff_cover`, `siege_site`, `siege_goal`, `siege_line`, `siege_barrier`) to how many are placed (a `siege_barrier` is a label on a placed object, not a flag, so it is counted and is not in `markers`), and `markers` lists the flags as `{id, label, number, position, facing, team}`. `problems` is each label the last build could not honour, `{label, position, reason}`. `report` is null before a build, else `{zones, squads, problems, receipt}`, with `receipt` `{base, labels, kit_version}` as hex digests of the inputs the build covers |
| `forge.ground.build` (act) | `{if_stale?}` or omitted | **Build AI ground**, or with `if_stale: true` **Test waves** (build only when the ground is stale or was never built). Answers when the build has ended with `forge.ground`'s reply plus `built`, whether it built. The build runs off the game thread over a copy of the session, and edits wait meanwhile. Refused as `forge_refused` when this game has no builder (`This game cannot build AI ground.`), in a shared session, while another edit or file job waits, and with the build's own sentence when it fails (a map with no room for the ground names its cost and its biggest part); a failed build changes nothing |
| `forge.nav` (act) | `{generate?}` or omitted | **Generate nav** ([forge-nav.md](../../game/forge-nav.md)): `{state, available, generating, counts, problems, overlay}`. `state` is `not_built`, `current` or `stale`; `available` is whether this game has a generator; `generating` is true while one runs (edits wait for it). `counts` is null before a generation, else `{sectors, links, out_of_map, solids}`, `links` mapping each link kind (`walk`, `drop`, `jump`, `crouch_jump`, `teleporter`, `lift`, `elevator`) to its count. `problems` lists `{kind, what, position, reason}`, `kind` one of `start_off_mesh`, `teleporter_off_mesh` and `solid_left_out`. `overlay` lists `{kind, a, b}` lines: each sector's sides (`sector`, or `out_of_map` where no start reaches) and each link but a walk, by its kind. With `generate: true` (a local multiplayer tick) it generates first and answers when it has ended; refused as `forge_refused` when this game has no generator (`This game cannot generate nav.`), in a shared session, while another edit or file job waits, and with the generator's sentence when it fails; a failed generation changes nothing |
| `forge.things` (read) | `{kind?, changed?}` or omitted | `{things, left_out}`. `things` is every thing of the session (the map's own entries in block and index order, then the added ones), or those of one `kind` (`object`, `item_spawner`, `starting_point`, `goal`, `teleporter`), or with `changed` those an entry op changes (`true`, which includes every added one) or not (`false`). Each is `{id, block, kind, own, changed, position, rotation, fields}`: `id` is the endpoint an entry op's `target` names, `{base:{block, index, print}}` or `{added: op}`; `fields` maps each layout field name to its value as the document writes one. Each netgame flag also has `area` in the document spelling, or `null` for an unshaped flag. An active area marks its flag as changed. A removed entry is not listed. `left_out` is each block of the map that did not read, as `"block: reason"`; its entries are not things |

### Local Forge session: look and monitors detail

| `forge.look` (read) | `{seat?}` | The monitor look: `{session, look, drawn_as, saved, catalogue, locks}`, and `notice`, `catalogue_note` and `problems` when there are any. `look` is the session's look, or with no session the saved one as this machine can use it. `drawn_as` is what other players see: `{as: "model" \| "marker" \| "hidden", reason?}`. `saved` is `{look, source}` as the profile holds it. `catalogue` is each model of the census: `{key, name, source, sha256, variants, colours, light, nodes, triangles, available, refusal?}`. `locks` maps each locked field to its reason. A `seat` other than 0 is refused: offline there is one seat |
| `forge.look.set` (act) | `{reset?, save?, model?, variant?, primary?, secondary?, tint?, light?, light_colour?}` | `{look, changed, saved, drawn_as}`: the look as accepted. `reset: true` is the default look (the marker in the seat's colour) and takes no field. `variant: null` clears the variant. `save: true` also writes the profile (`not_saved` says why when the file was not written). A refusal is an error whose `data` is `{code, message, ...fields}`: `monitor_look_invalid` (`field`, `reason`), `monitor_look_locked` (`field`, `reason`) or `monitor_look_no_session` outside a Forge session; the look stays as it was |
| `forge.status.monitors` (read) | `{}` or omitted | `{monitors: [{seat, name, monitor, pose, look, drawn_as, peers_see, seen_ticks_ago}]}`. Offline it holds seat 0 once a session is open and is empty before. `pose` is `{position, yaw, pitch}` while the monitor camera exists. `drawn_as` is how the seat looks in its own view, always `hidden` with the reason; `peers_see` is what another machine draws |

### Local Forge session: input queue per-action behaviour

The input queue accepts `toggle`, `accept`, `select`, `back`, `delete`, `palette`,
`properties`, `tools`, `settings`, `overlays`, `undo`, `redo` and `link`. `palette`
opens the object menu and `properties` the aimed or held thing's
Properties, as X does by what is aimed at. In the Objects tool `accept`
picks up the thing aimed at, own or added, mark or body, and drops it.
With empty hands `select` toggles selection of the ungrouped thing under
the reticle, like holding A. `delete` removes the thing; `tools` opens the Tools
picker, `settings` the Tool settings and `overlays` the Overlays page. `link` switches to the Links
tool; there `accept` starts a link on the aimed teleporter flag or
placed machine and makes it on the second, `delete` removes the aimed
thing's latest link, `properties` opens its Link dialog, and `link` or
`back` cancels a started link. `link` with nothing aimed returns to the
Objects tool. `back` with nothing to cancel does nothing. In player mode
only `toggle` enters the monitor. The screens are the game's: while one
is up (ours or the game's), `forge.input` is refused with "A screen is
up; drive it with ui.press." and the screen is driven with `ui.press`.
`menu` is refused with "The Forge menu is the game's GAME MENU; open it
with Start.". An analog request supplies one 1/30-second sample
with at least one axis. Movement is [right, forward, up], look is [yaw,
pitch] and roll turns a preview with `rotating:true`. Each axis is in
[-1, 1]. Read `forge.status` for the resulting monitor and document
state: it adds `fine` (fine movement on), `ui_up` (a screen is up) and
`dirty` (edits not saved), and `screen` is the Forge screen of ours that
is up, with its rows and focus.

### Local Forge session: area draft intro

Choose **Edit area** in a goal's Properties before sending area
input. While a draft is active, `forge.status.area_edit` gives
`{target, shape, below, above, radius?, points?}` with numeric world

### Local Forge session: area input acknowledgement

`forge.input` acknowledges the queued input. Read `forge.status.notice`
for geometry refusals and `forge.log` for the area op written on save.

### Local Forge session: proposal handling detail

A proposal uses the [Forge document's](../../game/forge-design.md#the-document)
portable op and optional prior-value digest. The local host supplies the
author and sequence. Placement waits for the engine's creation result.
A saved document can create several objects in one tick. If the engine refuses
a creation, the call releases objects it made for that load. A refusal leaves
the log head in place. Moves, deletes and restores, including those of
undo and redo, run at the next tick through the `world.object.set` and
`world.object.delete` bodies and the ownerless spawn. A live object that
no longer resolves by kind and salt is refused as `forge_object_gone`
before any guest call. A block with no `world.object` kind (sound
scenery) is refused as `forge_object_kind`.

### Local Forge session: entry op detail

An entry op edits one of the map's own things
([forge-map-variants.md](../../game/forge-map-variants.md), "Applying an
edit in a session"): read its `id` from `forge.things`, whose print the
session checks against the row it kept when it opened. A set of its
position or rotation moves it, a removal makes its row inert and a reset
puts the kept row back, at the next tick. A refused input or proposal
carries its name in `data.code` (`agent_proto::forge::refusal`):

### Local Forge session: kit session rows

A kit session adds those rows: the document's `Add` of such an entry is
one `Add` base step, which writes the block's next spare row and raises
its root count, and its removal or undo is one `Drop` step, which writes
the spare form back and lowers the count (`agent::forge::base`). A batch
is checked in order; a refusal changes nothing.

### Local Forge session: area op and teleporter link detail

An area op names a netgame flag, own or added, and sets its
[document area](../../game/gametype-markers.md#in-the-document). Send
`"area": null` to clear it. Radius, offsets and heights use the
document's eight-digit lowercase hexadecimal float spelling. An area
edit changes the document and can be undone through the edit log.
`forge_refused` carries the area validator's sentence, including when
a path crosses itself, a flag has no area to clear, or a hill already
has corners. The `was` precondition applies to field sets.

A teleporter link or unlink, or the undo of one, writes each changed
flag's identifier into the loaded scenario at the same tick, after the
batch's objects. The call checks the flag index against the scenario's
count and its type (7 or 8) before any guest write. A device link reaches
the game only through a rebuild and counts in `pending_rebuild`. Deleting
a linked machine ends its links in the same undo step.

### Forge probes: capacity, replication, sync, AI detail

**Capacity and kit.** `headroom_bytes` is the difference between the
two physical arena cursors at the stamp's tick. It includes space
that cache pools may need. Each block reports `count`, the known
`limit` or `null`, and `kit_left`. The candidate spare count is
the contiguous marked tail of a block. The markers are palette index
`-1` for placements, placement index `-1` for object names, player
type `-1` for starting locations, flag type `0xffff` for netgame flags
and a null item datum for netgame equipment. A block without a known
marker reports `null`. The kit probe adds a runtime object census by
group and the live actor count for base-versus-kit comparisons. The
candidates' behaviour is measured in the game.

**Replication.** The host must be in a ticking `mp_host` session. The
group must be one that [`sim.spawn`](../agent-api-sim.md#the-games-own-functions-teleport-give-spawn)
accepts. A tag may be a datum or name in that group. With the group alone,
the probe uses its first valid loaded tag in index order. With `pos`
omitted, it uses one world unit above the first eligible scenario
starting location. Pass an explicit `pos` to control the spawn location.
The probe queues a single `sim.spawn` engine call. The returned
`object` is the host's salted handle in the same form as `sim.objects`.
The client can enumerate that group through `sim.objects` immediately
afterward. A refused or missing spawn leaves replication unmeasured.

**Synchronous protocol.** `session` uses the same reader as
`sim.session`. `scenario_type` is the loaded tag's kind (multiplayer
is 1). `variant_engine` shows the engine in the active game options
(none is 0). `desync` is the simulation world's desync latch. Fields
whose backing globals are absent are `null`. The probe supplies the
current multiplayer map's [`sim.hashes`](../agent-api.md#framescene-checks-compare-capture-scene-parity-hashes)
rows up to the sampled tick. `from_tick` is inclusive. `max` defaults
to 4096 and accepts 1 to 4096. During pregame, or when the hash observer
has no samples for the current map, `rows` is empty. Compare matching
ticks across peers and check the latch on both.

**AI.** `enabled` is the flag the engine's `ai` builtin sets and
`ai_update` reads. `active` is the adjoining activity flag used by
the campaign AI extension. A missing AI-global pointer produces
`null` for both, and a missing actor pool produces zero actors.
Capacity, AI and kit reads require a gameplay tick.

### Firefight: state, command, kit detail

**State.** `running` is false outside a multiplayer game, on a map
without the kit, and with the feature off; `refused` names which (a map
without the kit: "the map does not carry the Firefight kit"). `role` is
`host` (the distributed server, a synchronous host, or a game with no
session), `replica` (a synchronous peer, or a film's playback) or
`client` (a distributed client). `team` is the actors' team, never a
player team (with two player teams it is a third). `director`, on the
host only: `phase` (`warmup`, `spawning`, `fighting`, `cleanup`,
`intermission`, `done`) with `phase_tick` for the timed ones, `round`
and `wave` (zero-based), `reserve` (actors of the wave not yet placed),
`teams` (the player teams, 1 or 2), `pools` and `scores` (each team's
shared lives and points, one entry per team), `live_actors` against
`cap`, `players`, `skulls` and `held`.
`log` is the head (`seq`, `tick`, `hash` in hex) and `applied`, the last
entry applied here. `errors` holds the latest 16 entries the engine
refused, each with its `seq` (none for the AI switch), `tick` and
`error`.

**Command.** `command` is one director command in the host log's shape
(externally tagged, snake_case): `place_squad {slot, class, count, zone,
weapon_tier}`, `target {slot, team}`, `hunt {slot}`, `erase_squad
{slot}`, `round {index}`, `wave {index}`, `skull {id, on}`, `lives
{player?, pool}`, `team_lives {team, pool}`, `score {player, points,
reason: kill | assist | player_kill}`, `respawn {player}` and `finish
{outcome: won | lost | {team_won: {team}} | draw}`. `target` picks the
player team a slot's squad goes for, and its next `hunt` heads for that
team's base (team 0's until one is set); `team_lives` sets one team's
shared pool and `lives` the one-team pool or a player's own lives, in
the director, so its next respawns spend from the new count (a team at 0
waits for the wave to end). It is appended at the tick after the last one
the runtime stepped, with the author `agent`, and applied before that
tick like the director's own. `director: "hold"` stops the director
stepping, so only appended commands reach the game; `"run"` starts it
again. A director change applies before the command. Refusals:
`UNAVAILABLE` when the mode does not run, `REFUSED` (11) on a client or
a replica (only the host writes the log), `INVALID_PARAMS` for a command
of another shape, a class or zone the kit lacks, a `target` team the
kit has no base for, a `team_lives` team above 1, and params with
neither field. An engine refusal comes later, in `firefight.state`'s `errors`,
as does a `lives` or `team_lives` the match keeps no such count for (a
player's own lives with shared lives, a shared pool with lives each
player's own, a player the director has not seen).

**Kit.** Read from the loaded scenario on any machine: `slots` gives
each of the sixteen slots' scenario squad index (`null` where the kit
reserves none), `zones` each drop zone's scenario index, point and
firing position count, `classes` each kit class's character palette row,
`characters` the palette's rows, `bases` each player team's base point
(base 0 the home base, from the `ff_base_NN` zones) and `hunt_orders`
the scenario order for each base's hunt (`ff_hunt_00`, `ff_hunt_01`;
`null` where the kit has none).
A map without the kit answers `{"kit": false, ...}` with empty lists.
The kit's version string is not read: the game has no reader for the
loaded map's string IDs.

### Rules scripts: state field detail and zones

`running` is false with the switch off, outside a multiplayer game, for
a record that names no script, on a machine that is not the game
engine's authority, after the game's end, and for a refused script;
`refused` says which, and for a refusal names what the script needs
("the script calls set_team, whose engine entry is untraced", "no
built-in script has hash prefix ..., and fetching a blob from Live is
not built"). `hash` is the blob's SHA-256 in hex, or the record's
16-digit prefix when no blob was found. While a script runs: `script`
and `version` are its header's, `source` is `builtin` or `live`,
`carrier` the active variant record's physical index (6 on a script-index
record), `scoring` who scores the game,
`options` the record's option bits, and `authority` what
`GAME_ENGINE_AUTHORITATIVE` answered at the first tick boundary (absent
before it). `scoring` is `script`: the running script's VM scores. A
script with a `uses` line has `scoring` `carrier`; its game
([rules-variants.md](../../game/rules-variants.md#a-script-that-keeps-its-carriers-rules))
runs the used type's built-in script, so `script`, `hash`, `source` and
`options` are the built-in's. `stopped` names the script error that
stopped the VM. `budget` is the VM's tick (`tick`), the ops it has used
(`ops`) against `limit` (20,000), and the verifier's `worst_tick`
estimate. `pending` counts the calls waiting for the next tick boundary.
`errors` holds the latest 16 calls the engine refused, each with its
`tick`, `call` (the entry's name) and `error`. `last_reload` is the
last `rules.reload` on this run: its `mode` (`in_place`, `restarted` or
`refused`), `hash`, `tick` and `reasons` (for a refusal, why).

`blocked` lists every problem the host's start gate found with the map
([rules-start-gate.md](../../game/rules-start-gate.md)), in the check's
order, while the block holds: the game was ended in its first tick (or,
in a Forge session, the script does not run), until the next game
starts. In the lobby, with no script running and no block of the
host's, `blocked` lists the problems of the lobby's own check
(`rules::gate`, the lobby's map file against the variant's script), and
`refused` says Start Game is held (in Forge mode, that the script will
not run); the lobby's check fills no `labelled`. `labelled` is the map's labelled things as the host counted
them, for a blocked script or a running one that declares labels: each
with its `label`, `class` (`object`, `flag`, `volume` or `spawn`),
`number`, `team` (the placement's designator), `id` (the placement id
`zone.id` reads) and `declared` (the script declares that label). To
check a map against a script, choose both in the lobby as its leader,
or start a game with them, and read `rules.state`; there is no separate
census method, since the host's census is the loaded game's.

`rules.zones` lists the zones the host built for the running script, in
`id` order (empty when no script runs): each netgame flag's `id` (the
placement id `zone.id` reads), `label` and `number`, `shape` (the step
of the zone order that made it: the flag's own `area`, a hill's
`corners`, or the `default` cylinder), `area` in plain numbers
(`{radius, below, above}` or `{path: [[x, y], ...], below, above}`, a
path in offsets from `at`), `at` (the centre marker in world units) and
`reports` (this zone reports its enters and leaves; a hill's other
corners name the same zone and report none)
([gametype-markers.md](../../game/gametype-markers.md), "In a script";
`Runtime::zones` in `crates/extensions/game/src/rules/runtime.rs`).

### Rules scripts: reload detail

`source` to compile; exactly one. With `mode` `auto` (the default) a
script that keeps the running one's state shape goes in place between
ticks, and keeps every variable's value; any other restarts the game on
the same map and variant with the new script, and `reasons` says why it
could not go in place. `in_place` refuses rather than restart;
`restart` always restarts. A restart answers once the game has taken
the queued options (up to 30 ticks later), and needs the agent's guest
calls (the `agent-calls` feature). It is refused (`REFUSED`, with
`data.why`) for a Theater film, a matchmade Live game (ranked or not),
a guest, outside a multiplayer game, for a blob that does not verify or
that this host cannot run; a Live custom game reloads in place only.

### Game variants: row and set detail

own copy); with neither it is `UNAVAILABLE`. `script` is the script the
record's block names, when this machine has it (a store variant's
bundle block counts, as at the start gate), and `hash` its SHA-256 in
hex; absent for a variant with no script, whose panel keeps retail's
rows. `shown` is true when the panel shows these rows: the
`variant-quick` switch is on ([switches.md](../switches.md)) and the script
was found. `rows` are the script's `quick` marks in its order, at most
five, empty for a script with no marks: `slot` is the row's place, `kind`
a record setting of `rules::settings::TABLE` or one of the script's
options, `value` the record's value (an option's from the block's option
bits), `text` its text and `line` the panel's "Label: value". `help` is
an option's description. `values` are what the row may take: for a
setting, the values the editor offered at the last variant list load
(none before one); for an option, its range. The panel's lines
themselves are in [`ui.lobby_options`](../agent-api.md#ui-what-the-menus-show).

`variants.quick.set` sets row `slot` of the lobby variant to `value`,
one of that row's `values`: a record setting, or the option's bits in
the block in effect (the block's CRC made again). The record is checked
with `record::refused`, then queued; the next UI update writes it and
calls the lobby variant setter (`LOBBY_SET_VARIANT`), and the answer
waits for that (`TIMEOUT` past the deferred limit). It answers with the
rows as they are then. A variant with no script this machine has is
`UNAVAILABLE`, as is a record the setter refuses ("the game refused the
variant"); a slot past the script's marks, a value the row does not
offer and a record `record::refused` refuses are `INVALID_PARAMS`, with
the reason. It changes the lobby's variant, not a save, as the dialog
does ([rules-variants.md](../../game/rules-variants.md#quick-options-in-the-lobby)).

### Sim matches: lobby step detail

`match.status.lobby.step` advances through `Create`, `SquadCreated`,
`SessionStarted`, `Start` and `Loading`. Its `wait` field gives the current
setup blocker or `null`. The numeric `session_type` (retail lifecycle)
and `squad_state` come from the same setup tick. A missing reading is
`null`. An allocated idle squad reads 0, and a hosted pregame squad reads
5 with session type 1 or 2. These fields retain the last setup observation
during play. Use `phase` and `bound_seats` for gameplay progress.

### Theater transport: e2e script and watch prose

From a signed-in local multiplayer lobby, `python3 scripts/theater-e2e.py` starts and ends a game, opens its Recent film from Game Setup, exercises both cameras and the transport, and closes the player. Pass the map’s display name if it differs from the recorded map filename.

A game that records its own games plays a film in itself. `theater.watch`
queues the film's game options as the next game, as `game.load` queues a
level, and the game loads the film's map and plays the film's updates; no
process is started. The transport methods below then answer from that game
until `theater.end`, and they answer with an unavailable error until the
film's game has activated. Only one film is watched at a time. The
session's recording is parked while the film plays and resumes at the next
game, so a film being watched is not recorded. The film's simulation-scope
settings are applied when it is queued and the viewer's put back when it
ends. The film holds its last tick when it ends; `theater.end` queues the
main menu and the game returns to it. A co-op or networked session refuses
`theater.watch`, as it refuses `game.load`.

### Theater transport: theater.seek row detail

| `theater.seek` | `{"tick":900}`, optionally `"clip_marks":[300,null]` and `"source":"boot"` | a tick ahead of the one shown runs forward from it (`seek.source` is `forward`); a tick behind, or `"source":"boot"`, queues the film's options again in the same game, and the film plays from its game's first update to the target (`seek.source` is `boot`). The camera and timeline state carry across |

### Theater transport: start_point.load and theater.status row detail

| `theater.start_point.load` | `{"path":"/tmp/a.sp","key":"text","film_hash":"<16 hex>"}`; `film_hash` is optional | on the game agent: checks the tag, digest, size and hash before touching the game, writes the arena and records back, then returns `{tick, arena_hash, loaded_hash, matches, error?}`. `matches` is `StartPoint::check_loaded`; a file that loads but hashes differently reports `matches:false` and the game keeps the loaded state. A refused file changes nothing |
| `theater.status` | `{}` | `{default_recording, recording, in_game, films, last_discard, notes, failure, watch_failure}` on any game or playback agent: whether the session records every game by default, the film file being written (`null` outside a game), whether a game is in play, the films sealed so far (film file paths, oldest first), why the last game went without a film, the outside changes the recording carried on through, and the failure that stopped it. `watch_failure` is why the last film watched in this game stopped before the viewer left it (a start or digest divergence): the film is left like `theater.end`, the game goes on |

### Theater transport: start point methods prose

The start point methods are in `crates/extensions/agent-proto/src/theater.rs`
and `crates/extensions/game/src/theater/start_point_agent.rs`, over the
envelope in `theater/start_point.rs` and the state read and write in
`theater/start_point_state.rs`. They serve the experiments of slice 15 in
[theater-design.md](../../game/theater-design.md): capture at tick N, run on,
load, and compare the `sim.hashes` rows that follow with the first run's.
Use them on a local game; a load writes the arena of a running match, so a
co-op or Live session would desync. Without `film_hash`, load takes the
hash the file declares, so only the tag and digest are checked.

### Theater transport: verify

Verification counts the ticks and the gameplay digests checked within the
inclusive tick bounds. The verifier plays from the film's game start and
reports the first divergence even when it prevents reaching the requested
range. The active player's transport remains usable. A clean run returns
`first_divergence: null`. A mismatch returns its tick and reason.

### Theater transport: screenshots

Screenshots use scale 1 by default and save the latest published game picture.
Scales 2 through 4 render the view at a larger internal raster. The independent
observer can redraw from its current state. When the redraw is
unavailable, the method reports why. The default output is a new PNG in the
film library's `screenshots` directory.

### Theater transport: export jobs

An export job copies the current camera rig into a windowless replay. The
range is half-open, in ticks from boot. The tick at `to` is excluded. Output
uses a 640×480 canvas and contains exactly `(to - from) * fps / 30` frames.
At 60 fps, a tick with one presented frame uses it twice. The timeline holds
the previous picture through skipped draws. MP4 includes the available
48 kHz stereo game mix. Host recordings with clock-only audio export with
silence.

The default destination is under the film library's `exports` directory.
A PNG export creates a directory of numbered frames starting at
`frame-000000.png`. MP4 uses the user's `ffmpeg` on `PATH`. When ffmpeg is
missing, the job reports the same reason as `h2 movies` (exit 3) and PNG
export remains available. Use a fresh destination path for either format.
`finished` becomes true after the output is published or the job fails.
`error` is null for a successful job. The job ID remains valid across seeks.
These files stay on the user's machine.

### Theater transport: compat

`theater.compat` is a read method on the library agent. It accepts a local
film ID or `live:<game-id>` for a cached host film. States are `playable` and
`unavailable`. Reasons are strings, and `contract` and `hooks` are null
when the recording supplies no digest. The preflight checks the XBE, the
simulation ABI and that the film has a game start; any installed image of
the XBE plays a film that passes. The replay verifies installed hooks
before entering the guest.

### Theater transport: camera rig and monitor looks

The rendered player starts at the clip's first tick and loops at its end.
Its overlay uses the player's agent socket for transport, camera and HUD
controls. HUD and camera changes apply to the next rendered frame.

Both picture paths use the same rig. Rendered local playback starts in
`default` with the whole retail frame. Host playback starts in `free`;
`default` on a playerless film uses the first available player's eye.
Use `orbit`, `follow` or `first-person` with a player name or film seat
to select a subject. On rendered films, first person selects the recorded
viewport. If the subject has no viewport, state reports `camera: "follow"`
and `camera_fallback: "no-recorded-viewport"` until one is drawn.
On playerless films, first person uses the unit's eye.
A Forge monitor seat that the film recorded can be named by its seat number
in `follow`, `orbit` and `first-person` even when the roster does not list
it; first person looks from the monitor's recorded pose. `theater.state`
and `theater.compat` list `monitor_looks` (`{seat, map, reason:
"monitor_look_source"}`) the machine draws as the marker; they never change
`state`.

### Theater transport: camera fields and transport state

`look` adjusts yaw and pitch in radians in free and orbit modes. In free
mode, `movement` adjusts strafe, forward and rise in world units. In orbit
mode, `zoom` adjusts the radius in world units. `fov` sets the horizontal
field of view at 4:3 in degrees (20 to 140). Omitting it keeps the current
lens. Headless playback (`--render-every 0`) accepts only `default`.
Playerless playback has no game HUD.

Transport state carries `id`, `tick`, `length_ticks`, `paused`, `speed`, `speed_reached`,
`seeking`, `compatibility`, effective `camera`, optional `camera_player` and `camera_seat`,
`hud`, `bar` and `camera_pose`. The pose has `position`, `yaw`, `pitch`,
`fov` and `radius` once there is a view. State also carries discovered kill
marker ticks in `kills` and the usual `at` stamp.
Tick zero is boot. `speed` is the requested multiplier. `speed_reached`
is completed ticks per wall second divided by the film's tick rate and
capped at the requested speed. It is null before a half-second sample
or during a seek or step, and zero while held. A paused seek holds at
its target until play. Fast-forward checks the film's gameplay digests
([theater.md](../../game/theater.md)).

### Theater transport: seek object

The `seek` object records `target`, `from_tick`, `source` and
`remaining_ticks`. `seeking` is true while the film's game is queued
again or the player is running forward. A replacement target keeps the
original departure tick. The seek object remains available at the target
with zero ticks left. Linear playback starts with `seek: null`. `source`
is `forward` for a run on from the tick shown and `boot` for one from
the film's game start.

### Theater transport: measurements fields

`theater.measurements` is a read on the recorder's or playback child's
own socket when the run uses the Theater measurement switch
([switches.md](../switches.md#tools)). Its fields are:

- `child_to_first_slice_ms` from the playback child entry to its first
  guest scheduler slice.
- `boot_to_match_start_ms` from configured boot to the first recorded
  match tick, and `match_start_tick`. These stay null when the run
  does not pass through that tick.
- `fast_forward_ms` and `fast_forward_ticks` from the first scheduler
  slice after boot to the seek target. Both are null until a forward
  replay reaches its target.
- `page_windows` with `{seconds, from_tick, to_tick, pages}` counts
  distinct physical guest pages written in each 5 s and 30 s window of
  match ticks. `page_windows_omitted` counts entries older than the 256
  retained.

### Theater transport: agent examples

```sh
h2 agent theater.state '{}' --socket "$film_run/play.sock"
h2 agent theater.pause '{}' --socket "$film_run/play.sock"
h2 agent theater.seek '{"tick":30,"source":"boot"}' --socket "$film_run/play.sock"
h2 agent theater.step '{"ticks":1}' --socket "$film_run/play.sock"
h2 agent theater.play '{}' --socket "$film_run/play.sock"
```

### anim: arm and manager field semantics, examples

`arm` has `checked`, `matched` and `mismatched` calls, `dropped` plans
(calls that entered and never returned), `declined` by reason (`float`,
`fault`, `count`, `queue`, `alias`, `parent`; the guest ran unchecked)
and `first`: the first mismatch since start, or absent. `first.what`
names the register or byte (`ecx`, `xmm3`, `matrix 2 +0x10`, `stack
entry-0x3f8 (queue entry 1)`), `plan` and `guest` give both values in hex,
and `entry` the general registers and MXCSR at that call's entry. `body`
counts calls the Rust body answered without the guest (`native`, `nodes`
posed, `declined`); it stays zero while the arm is installed. The counters
are zero with the switch off. Sample before and after a scenario and
subtract to isolate that run.

```sh
h2 agent anim.graph_walk
```

Parity is `mismatched` 0 over the run ladder's matches, with `declined`
small against `checked`.

`anim.manager` counts segments starting at a function entry or resuming
after a call. For each observed function, `matched` and `differed` count
completed comparisons. `declined` counts segments the Rust arm could
not predict. `interrupted` counts predictions for which the guest exited
before a comparison boundary. The first difference for each function is
retained in `first`, with the starting selector (`pc`), differing field
(`what`), and both values (`plan` and `guest`). An entry with `first` set
to `null` has no recorded difference. Compare counts before and after
the scenario to establish coverage. See [animation-manager.md](../../engine/animation-manager.md).

```sh
h2 agent anim.manager
```

### sim.arms: fields, sources, zero mapping, example

Each subsystem has `name`, `switch` (`impl.sim.native.<name>`), `on` (the
switch in this process), `arms` and `clean`: `checked > 0 && mismatched
== 0`, summed over the arms. Each arm has `name`, `checked`, `matched`,
`mismatched`, `answered` (calls Rust answered in the guest's place) and
`declined` (calls left to the guest unchecked, summed over reasons). The
sources are `sim_physics::parity::status`, `sim_objects::functions::parity::status`,
the graph walk's `arm_stats` and `stats`, `sim_anim::manager::arm::stats`,
`sim_collision::WORLD.stats` and the shadow registry's `sim.seats` and
`sim.effects` groups. A source without a counter reports 0 for that
field: physics and collision have no `checked` (it is `matched` plus
`mismatched`) and no `answered`; `anim.manager` has no `answered` and its
`differed` is `mismatched`; shadow ports have no `answered` and their
`skipped` is `declined`. A subsystem with its switch off reads zeros, so
`clean` is false.

```sh
h2 agent sim.arms
```

Run the scenario, read `sim.arms`, and flip a phase on only when its
subsystem is `clean` with `checked` large enough for the scenario.

### collision: trace reply, entries, comparison counters, bail audit, examples

`collision.trace` evaluates entry J at a tick boundary on the active
structure. The segment ends at `start + delta`. Both ignore handles are
empty. The reply gives the tick stamp, numeric `hit_type` (zero for a
miss), `fraction`, world `position`, optional `material` and `plane`, and
the first and last `locations` as `{leaf, cluster, bsp}`. The arrays
`cluster_marks`, `instance_marks` and `object_marks` give the slots J
would mark in traversal order. The query leaves guest memory untouched.
A path outside the native kernel returns `UNAVAILABLE` with the bail
name in `error.data.reason`.

```sh
h2 agent collision.trace '{"start":[0,0,1],"delta":[0,0,-2],"flags":1}'
```

Each row in `entries` gives the `name` and eight-digit guest `addr`, the
count of `native` answers, and `bails` by reason. Two frequency maps count
the control words at entry, including calls that fall back at the first
environment check. Keys are lowercase hexadecimal strings with leading
zeros: eight digits for `mxcsr` and four for `x87_control`. Each value is
a call count. Each map counts its own register independently. `cluster_flood` includes recursive entries in
its environment census. The `mopp_ray` and `query` rows count the
environment while their comparison records report the lifted outcomes.
An image without an observed entry leaves its row empty. The `object_sphere` row counts native answers at the
whole query's object-reference pretest. `array_bytes` counts the decoded
BSP allocations, the packed cluster table, the portal graph and the decoded MOPP trees (`mopps`
of them; `decodes` and `decode_failures` include MOPP programs, a failure
being one left to the guest). `bytes` adds the reserved space for BSP owners and address-index
capacity. `budget_bytes` limits that total to 8 MiB per loaded map.
`ring_fallbacks` counts transitions to scalar ring reads, and
`cluster_budget_reads` counts lookups that need the live cluster word
because the packed table would exceed the budget. Sample before and after
a scenario and subtract the counters to isolate that run.

`vector` counts entry C’s `matched` and `mismatched` returns and
`unverified` queries by bail reason. `first_mismatch` identifies AL or the
first differing byte offset in the result record, with expected and guest values.
`context` uses the same counters for entry D. It compares AL, R and the
40-byte context, counting each outer walk once across its recursive calls.

`instance`, `flood`, `visit` and `query` use the same comparison counters.
An admitted native exit counts under `entries[].native`. A comparison
counts only after the guest returns. A declined prediction is
`unverified`. For J the comparison covers AL, flags, the result including
padding, and the active mark tables and generations.

`ray` counts the MOPP ray queries the native walk followed while the
guest answered them: `matched` (every interpreter call, and every key
where the instanced collection shows it, equal), `mismatched` with
`first_mismatch` describing the first since start (the observation index
and both sides), `unverified` by bail reason (the walk could not follow
the query exactly, or its environment is one it does not model), and
`fraction_moved` (no key was visible and a terminal lowered the fraction).
Read `matched`, `mismatched` and `unverified` together to distinguish
a covered query from one the walker could not follow.

```sh
h2 agent collision.stats
```

The bail audit (under 0.1% of calls bailing) reads `bails` against
`native`. For `mopp_box`, `grow` counts answers that did not fit the
key array's room. A reason that appears on one architecture only is a determinism
bug.

### Checking a cold collision world: sources detail and capture script

At a tick boundary, `collision.sources` returns `sources` sorted by
`kind` and guest `address`, with `cached` and `live` 64-bit hexadecimal
fingerprints. A BSP fingerprint covers the live header, all eight blocks,
the projection table and the zero used by the matcher. A MOPP fingerprint
covers origin, scale, program length and bytecode. A failed live read
returns `live: null` and the `bail` reason. The reply counts `mismatched`
and `unreadable` separately. An empty list means no BSP or MOPP is
resident. A debugger memory trap makes the method unavailable.

For the cache-cold parity arm, `collision.sources.clear` gives the
pre-clear readings and drops the BSPs, MOPPs, leaf table and portal graph.
The next queries repopulate the world from current guest sources. The
read-only form leaves the warm world's entries in place.

Start each arm from the same match state and input with
`impl.sim.native.collision` and `impl.sim.arena-hash` enabled.
Use one claimed connection per arm. This example captures 1,800 cold ticks.
Replace `collision.sources.clear` with `collision.sources` for the warm
run and save its JSON lines separately.

```sh
{
  echo claim
  echo pause
  echo collision.sources.clear
  for tick in $(seq 1 1800); do
    echo 'step 1'
    echo collision.sources.clear
  done
  echo sim.hashes
  echo release
} | h2 agent -
```

Compare tick stamps and the sampled gameplay and arena rows from
`sim.hashes`. At each tick, match the cold sources to warm sources of the
same kind and address, and compare fingerprints. A warm world may retain
entries from earlier ticks. Inspect unreadable sources, missing counterparts
and cached/live differences before treating a run as clean.

### Tick traces and the flight recorder: field detail and examples (trimmed)

Polling `world.object` from outside (`h2 agent -`) drops ticks when the
game or the client is busy. `trace.*` and `flight.*` take the rows inside
the game, in the tick observer, before queued requests are served, so each
tick gives exactly one row. Code: `crates/extensions/agent/src/trace.rs` (tests in
`trace/tests.rs`), wire types and `show` in `crates/extensions/agent-proto/src/trace.rs`,
CLI in `crates/apps/cli/src/agent_flight.rs`. File readback lives in
`crates/extensions/agent/src/outputs.rs`.

The game puts `traces/`, `flights/` and `inputs/` under the
[agent output root](../switches.md#tools), which defaults to its cache
directory. For a [lab job](../lab.md#runs), that root is `runs/<id>` and
`h2 lab fetch` includes the files.

```sh
h2 agent trace start player.0 physics,pos          # rows to traces/trace-<ms>.jsonl
h2 agent trace stop                                # rows, bytes, first/last tick
h2 agent flight start player.0 physics,pos,velocity 20
h2 agent flight dump crouch-bug.jsonl              # flights/crouch-bug.jsonl
h2 agent trace.list                                # saved trace and flight file names
h2 agent flight show crouch-bug.jsonl --remote --field physics.crouch
h2 agent flight show ~/Library/Caches/h2recomp/flights/crouch-bug.jsonl \
    --from 4100 --to 4160 --field physics.crouch --field physics.airborne --field input
```

- **Rows.** One JSON line per tick: `{tick, ticks, map, objects: {id:
  Object}, gone, error}`. `tick` is the stamp's game tick, `ticks` the
  agent's count (never resets, one more each row). `objects` holds each
  asked-for id in the world, trimmed to `fields` (`world.query`'s names;
  `id` and `kind` always kept; all when absent). Ids not in the world that
  tick are in `gone`; a failed world read gives `error` and still a row.
  The world is read once a tick when either recorder runs, and not at all
  otherwise. A paused game writes no rows; each `session.step` tick does.
- **`trace.start {objects, fields?, path?}`.** 1 to 64 ids. Streams rows to
  a bare file name in `traces/` (default `trace-<ms>.jsonl`).
  An existing file is replaced. The writer flushes every 30 rows, at
  `trace.stop` and at exit. Memory use stays constant with file length.
  `trace.stop` and `trace.status` give `rows`, `bytes`, `first_tick`,
  `last_tick`, `path` and `error` (a write failure ends the trace there).
  One trace at a time; the last one's counts stay for `trace.status`.
- **`flight.start {objects, fields?, seconds?}`.** Keeps the last `seconds`
  (default 30, 1 to 600) x 30 rows in memory, at most 64 MiB of JSON (the
  oldest go first). Each row also has `input`: every `XInputGetState`
  result the game got since the last tick, taken at the agent's input tap
  after agent steps merge in (`{port, handle, status, packet, buttons,
  left, right, raw}`, `raw` the 18 pad bytes in hex), at most 64 a tick
  (`input_dropped` counts the rest, as during a load). Without XAPI input
  there is no tap and rows have no `input`. `flight.dump {path?}` writes
  the ring to a bare file name in `flights/` (default `flight-<ms>.jsonl`)
  and keeps recording. `flight.stop` drops the ring. `flight.status` gives
  `capacity`, `rows`, `bytes`, `ticks` seen and the ring's tick range.
- **`trace.list`** returns `{traces, flights}`, with sorted file names
  from each directory.
- **`trace.read {path, kind?, from_tick?, to_tick?, offset?, limit?}`**
  returns `{rows, next}`. Pass a bare file name. Set `kind` to `trace` or
  `flight` to select a directory, or search `traces/` first and
  `flights/` second by leaving it out. The tick bounds are inclusive.
  Each row is a complete JSON line without its newline. The limit is in
  rows (default 64, maximum 256), and file scanning is bounded per call.
  Start at offset 0, then pass the returned byte cursor as `offset` until
  `next` is `null`. Follow the cursor even after a short or empty page.
  Read a finished trace or flight dump to keep the file stable across pages.
- **Scrub.** `h2 agent flight show <file>` opens a local file when it exists.
  A missing file or `--remote` reads through the game socket with
  `trace.read`. Use `--kind trace|flight` to choose a socket directory.
  The command prints whole rows or selected fields such as
  `--field physics.crouch` and controller reads with `--field input`.
  Use `--from` and `--to` to select game ticks. To re-simulate a stretch,
  start from a [snapshot](../agent-api.md#state-machine-snapshots).
- **Access.** These methods require no claim and are answered between
  slices as well as at tick boundaries.

### Shadow checks: port selection detail

are checked: a port's `impl.` switch puts it in the plan, and
`shadow.enable` is the only switch for the checks.

Select ports by switch name (`sim.native_unit_pose`), object point name
(`OBJECT_FUNCTION_VALUE`), group (`sim.native`, `sim.physics`,
`sim.native.objects`) or `all`. The object group samples individual entries
and resumes, including those reached inside a fused walk.

### Shadow checks: scripted disc gate

`scripts/shadow-gate.sh` is the scripted disc gate over these methods: a
headless sim match with the ports installed, checks on for N ticks, and a
failure on any mismatch ([shadow-exec.md](../../engine/shadow-exec.md#the-disc-gate)).

### Scripted scenarios with a verdict: verdict detail

`h2 agent run <script|->` (`crates/apps/cli/src/agent_run.rs`) is the `h2
agent -` script on one connection, plus `expect <pointer> <op> [JSON]` on
the last answer (a JSON pointer; ops `==`, `!=`, `<`, `<=`, `>`, `>=` on
numbers, `contains` for a substring, an array element or an object key,
`exists`, `missing`, and `near {"to", "within"}`) and `keep` (put the last
answer in the verdict). The expectations are `agent_proto::expect`, which
`sim.expect` judges in the game too. The verdict is `{"pass", "failed",
"ms", "calls": [{line, call, ms, ok, error}], "expects": [{line, expect,
want, pass, got}], "kept", "stopped"}`. A failed expect is recorded and
the script goes on; a failed call stops it. Answers are not printed
except kept ones, so a long scenario costs a few lines of context.

### Test maps: method table cell detail

| `maps.load` | act | `{map_id}` → `{name, map {id, path}, check, queued, expect}`. Queues a local Slayer game on the installed multiplayer package with this full hash. The VFS verifies the list slot's package identity. When the id is not found, the offers are installed again and the VFS's map packages reloaded before the id is refused, so a package written while the game runs is found ([the tag workspace](../../editor/tag-workspace.md)). Wait for gameplay on the basename of the returned path before driving the camera. |
| `maps.test.list` | read | `{name?}` → `{recipes, maps}`: the built-in recipes (`name`, `title`, `base`, `fidelity`, `purpose`, `note`) and each test map (`name`, `base`, `state` `building`/`ready`/`failed`, `dir`, `error`, `map_id`, `listed {id, path}` once it has a multiplayer slot, `built`: the build report with `anchor`, `placed`, `counts`, `notes`, `expect`) |
| `maps.test.create` | act | `{recipe: <built-in name> \| <recipe object>, base?, replace?}` → `{name, state: "building", dir}` at once; the build runs on its own thread, then the offers are installed again. `base` builds on another disc map (`<recipe>-<base>`) |
| `maps.test.load` | act | `{name}` → after the next tick, `{name, map {id, path}, check, queued, expect}`: a local Slayer game on the map is queued through the `agent-calls` feature (`Call::QueueGame`, the level boot's steps). Wait for it with `session.wait` or `h2 agent maps load` |
| `maps.test.remove` | act | `{name}` → `{name, removed}` |

### Test maps: map.reload detail

`map.reload {}` queues the active campaign or multiplayer map again and
returns `{queued, map, activation}` with the map name, the activation
reserved for this request and the executor's tick stamp. It uses the
[object edit session rules and hooks](../agent-api.md#object-edits). Film and input
recording or playback refuse both forms of reload.
`Sim::reload_map` in `crates/extensions/agent/src/sim/edits.rs` copies the active options block into
`Call::QueueGame`'s scratch. This preserves the scenario path, players,
variant and difficulty. `OPTIONS_QUEUE` takes the scratch address in EAX
and copies the block to the pending transition. See
[level-boot.md](../level-boot.md#addresses) for the active flag, block and
entry. The answer confirms queueing. Pass its `activation` and `map` to
`session.wait` together with `phase: "gameplay"` and `map_ticks: 1`.
For a reply naming activation 4 on `lockout`, the wait is
`{"until":{"activation":4,"map":"lockout","phase":"gameplay","map_ticks":1}}`.
Use the returned value even when the file checksum stays the same.
A checkpoint revert cannot publish a queued reload's activation.

`map.reload {"file":"/absolute/path/<map>.map"}` loads a play copy for the
active DVD map. Both the filename and the map name in its Xbox cache header
must match the active name, ignoring ASCII case. A worker prepares a
private generation and checks its cache type, slot capacity, tag index
and retail payload checksum. A disconnect or timeout releases the preparation
and the worker checks for cancellation between chunks. At the tick boundary
the agent rechecks the session and active source, validates the pending
options storage, reserves the DVD file and retires the old cache keys
before calling the guest's options queue.
The VFS publishes the copy when the guest accepts the transition. If the
guest call is interrupted, the host checks the pending options before the
loader uses them. Matching options publish the copy. Otherwise the host
restores the retired keys. The result adds the requested `file` and the
header's numeric `checksum`.
After the activation wait, check that `live.status.map.checksum` or
`edit.map.checksum` matches the returned checksum. Both reload forms accept `confirm_peers`
on a System Link host. This total includes the host. Each peer needs its
own matching file. A writable game with the agent enabled can accept a play
copy even if it started without an override directory.

Forge's **Rebuild map** and the reload after **Build AI ground** run the
same reload from the game thread (`agent::sim::MapReloader`, which calls
`Sim::start_reload` with the checks `map.reload {file}` makes), so the
rules and refusals above apply to them as well
([forge-design.md](../../game/forge-design.md), "Rebuild map").

A light host, a server-map mount, a scenario loaded from a content path,
or a host without the editor map mount returns `REFUSED` with `data.why`.
A film named with `--film`, film playback and external Live sign-in also
exclude file binding. A session's
[default recording](../../game/film-format.md#recording-every-game) allows
it: the first authorized editor write discards the film of the game in play; films of games already ended stay. After a file binding succeeds, restart
the game before signing in to external Live. Machine snapshots and
snapshot-based rewind are unavailable while a play copy is bound.
See [Map overrides](../storage.md#map-overrides) and [Changed maps](../cache-slots.md#changed-maps).

### Test maps: examples

```sh
h2 agent maps                          # recipes and test maps
h2 agent maps create superbounce       # waits for the build
h2 agent maps load superbounce         # waits until the new map is in gameplay
h2 agent sim expect '/player/0/pos near {"to":[-10.5,3.9,-3.15],"within":1.5}'
```

### Test maps on disk: operations detail

`h2 testmap <source.map> <script.json | -> --out <new.map> [--force]`
(`crates/apps/h2-tools/src/testmap_cmd.rs`) is a host-only tool for a repro that
needs one thing in an empty room. It runs a JSON edit script
(`scenario_edit::script`, `crates/tools/scenario-edit/src/script.rs`) over the
source's placement blocks through the editor's own `Session`, so every
edit is checked as the editor checks it, and saves a new map (the source
is never written). It prints what each operation did and the entries left
per block. Operations: `clear` (blocks, or `"all"`; a block the editor
will not empty is reported and left), `keep` (these entries of a block,
renumbered from 0), `add` (a copy of `like`, or a zeroed entry with the
palette's first item, then `set`), `set` (fields by the editor's names:
`position` `[x, y, z]`, `rotation`, `facing`, `team`, palette `type`, ...),
`remove`. `allow_growth` lets a block grow past its source size, as the
editor's switch. Blocks: `scenery`, `vehicles`, `equipment`, `weapons`,
`machines`, `sound_scenery`, `crates`, `starting_locations`,
`netgame_flags`, `netgame_equipment`, `starting_equipment`. AI (squads and
their spawn points) and tag references other than through a palette index
are not covered. The user or coordinator loads the result (`H2_MAP_DIR`);
agents do not.

### Content packages: field semantics

The game's content packages: ours baked into `h2` from `content/<id>/`,
then the user's under `<data>/packages/<id>/`, ordered and switched by
`<data>/packages/set.toml` and the game's features, and what each map the
game read got from them ([studio-redesign.md](../../editor/studio-redesign.md),
"How the game consumes packages"). Registered by
`crates/apps/boot/src/packages.rs`; the set and the rows are
`workspace::GameSet`, the maps `workspace::PackageRewrite`'s record.

| Method | Access | Params and result |
|---|---|---|
| `packages.list` | read | `{}` → `{packages, problems, content_hash, local_only, maps}` |

- `packages`: every package read, those on in load order, then those off
  by id. Each row has `id`, `name`, `package_id` (the `PackageId`),
  `origin` (`baked` or `local`), `dir` for a local one, `on`, `why` when
  off (switched off in `set.toml`, a feature that is off, a `requires`
  that is missing or off, a set that cannot be ordered), `always_on`
  (`core` and `ui`), `signed` (baked packages are; a local one is not and
  loads in local games only), `features`, `items` (a count by kind) and
  `scopes` (`sim`, `client`, `live`, `per-feature`).
- `problems`: the lines the boot printed: a package that did not read, a
  local package with a baked one's id, a `set.toml` that is not used (it
  does not parse, or it switches `core` or `ui` off), a cycle.
- `content_hash`: the SHA-256 of the content set (the `PackageId`s of the
  packages on with a `sim` item), or `null` when the set did not order.
- `local_only`: why a Live or System Link session would be refused with
  this set (an unsigned package with a `sim` item, named), or `null`.
- `maps`: by map name, each map the rewrite was asked for (or, with
  `store-maps`, each level opened from the store): `scope` (its
  header's class), `packages` applied in order, `held` (packages whose
  tags the map has already, left out), `refused` when packages are off
  for this map (each `package` and `why`: a `from`, `was` or `replaces`
  that does not match what the map holds, a clash with an earlier
  package, or a `requires` that is off), `key` (the cache key; with
  `store-maps`, the level's `LevelId`, empty for a level with nothing
  over it), `hits` and `builds`, `added` and `changed` tag counts, and
  `error` when the map was served without its packages.

```sh
h2 agent packages.list
```

### The tag store: field semantics

The tag store at `<data>/tags` and the maps it can give back byte for
byte from their skeletons and extent tables
([asset-store.md](../../engine/asset-store.md)). With the `store-maps`
feature on, the game reads the disc's maps from it and never from the
disc. Registered by `crates/apps/boot/src/store_status.rs`; the maps are
`vfs::store_map::status`, read from the extent tables.

| Method | Access | Params and result |
|---|---|---|
| `store.status` | read | `{}` → `{root, exists, served, maps, stale, broken, images}` |

- `root`: the store's directory; `exists`: whether it is a tag
  repository.
- `served`: whether this boot serves the disc's maps from it
  (`store-maps`).
- `maps`: each map it can serve: `map`, `length`, `sha256` (the file it
  was extracted from), `skeleton_bytes` (the skeleton file, compressed),
  `extents` and `extent_bytes`.
- `stale`: maps `extract.toml` records with no skeleton or extent table
  (`h2 tags extract --update` takes them again).
- `broken`: maps whose extent table does not read, with `why`.
- `images`: the level images under `<data>/store/images`
  (`workspace::level`): `level_id`, `length` and `sha256` of the map file
  each gives, `disk_bytes` (the image and its table), `extents` and
  `extent_bytes`.

```sh
h2 agent store.status
```

### Not verified: itemised list

- Nothing here was checked by running the game unless a section says so:
  the tests above use fake readers, synthetic guest memory, synthetic pads,
  synthetic frames and a fake sampler.
- `ui.audit`, `ui.press` and the menu crawl against a live game. Their regression
  cases use synthetic memory, input and agent replies.
- `input.route` and the Ascension recipe against a running game.
- `state.*` against the real machine: the tests run over fakes.
- `sim.give`, `sim.spawn` and teleport against a running game: the callees
  are played by the tests.
- Lobby steps `invite` and `start` have not been pressed through by `sim`
  in a run.
- `render.frame`, `load.timeline`, `world.raw`, `havok.*` and the
  `trace`/`flight` recorder against a running game.
- `shadow.*` and `scripts/shadow-gate.sh` against the disc: the tests run
  over synthetic code and memory.
- `render.compare`, `render.capture`, `render.scene`, `render.parity`,
  `render.residual` and `sim.hashes` against a running game.
- `render.picture` against a running game, and its readback's cost to the
  game's frame time: the tests use a synthetic frame source.
- `maps.test.*` against a running game: the build, the slot and the
  queued load ([test-maps.md](../../game/test-maps.md#not-verified)).
- Not provided: effect, projectile and particle counts; per-subsystem
  guest time; owner attribution of frame cost; kernel I/O seeks and cache
  hits in the load timeline; a widget-level HUD; setting a feature switch through the API; loading a named level
  other than a test map (`maps.test.load`).

### Behaviour probes: request and result detail, hook addresses

| `probe.weapon` | Give the weapon with `sim.give` and wait until it is held. Send `{"player":0,"object":"e1230042","ticks":60,"steps":[{"rt":1,"ticks":55},{"ticks":5}]}`. The initial sample and one row per subsequent tick contain first-magazine loaded/reserve counts, magazine state and heat. Each tick's `shots` lists barrel events and the projectiles launched by each. Each projectile has its datum, definition datum, applied cone in radians, barrel error fraction (`bloom`), direction vector and measured deflection in radians. Two trigger steps separated by a neutral step exercise two presses. A step with `"buttons":["x"]` exercises a reload. |
| `probe.animation` | Send `{"unit":"e1230043","path":["combat","rifle","ar","fire_1"],"overlay":false}`. The four strings are mode, class, weapon type and action. Set `overlay` for an overlay lookup. Returns the unit's graph datum/name, requested path, selected mode/class/type string IDs and names, inherited graph index, animation index, selected clip name and frame count. The native selector runs against the unit's current animation manager. This samples the answer to a state request without changing the unit's playing channel. |
| `probe.hit` | Send `{"projectile":"proj:weapons/assault rifle/bullet","target":"e1230043","from":[3,0,1],"to":[2,0,1],"ticks":12}`. Coordinates are world-space, or target node-local when `node` is supplied. The projectile starts at `from` and travels in the direction of `to` with the projectile definition's velocity and range. Optional `shield` sets the target's shield fraction at launch. Returns the first collision of this projectile, including kind, position, object datum, region, node and global material index. Object hits include vitality `before` and `after` as [body, shield]. `damage` contains the game's damage reports for the specified victim during that projectile's impact. A wall or a different victim is a reportable hit. A miss through the entire window is an error. Damage and shield preparation affect the world. |
| `probe.round` | Send `{}`. Returns the engine's round-start tick, per-player first applied control and first movement/fire ticks, and any observation error. Capture starts at map activation, so callers can ask after the countdown. Join player indices to `sim.player.list.clankers` to identify bots. |

Weapon and hit captures share one slot. Their window is 1–290 ticks. A
changed weapon, missing object, unreadable fields or discontinuous weapon
trace fails the request. Machine snapshots preserve round history and refuse
an in-progress capture. Take a snapshot before a destructive experiment and
load it afterwards.

The simulation hooks measure the barrel fire call at `0x104150`, the launch
call at `0x1052c5`, projectile collision at `0xfaa60` and its nested damage
report. The animation selectors are `0x1db170` and `0x1dbc80`, with entry
resolution at `0x1daea0`. The round observer uses the two non-neutral
per-player callers of `0xc6de0`. None of the probes requires rendering.

### Lines condensed on the page

Single lines whose names or values the page no longer carries.

- (`H2_BOOT_REALTIME=1`); it is the shared instance agents use. `h2 play`
- `h2 lab run --serve` and `h2 lab bench` launch. Frame cost measured on
- The socket.** `--socket PATH` selects the endpoint, followed by
- `$H2_AGENT`, then `agent.sock` in the user's cache directory
- (`agent_proto::default_socket`; the directory per host is in
- `tcp://HOST:PORT` selects unauthenticated TCP, including the Xbox app's
- `h2 agent --xbox HOST` endpoint on port 4720 ([xbox.md](../xbox.md)).
- Another Mac.** `h2 lab agent --host H <run id> <call>` runs `h2 agent
- `render` and `counters` in one command ([lab.md](../lab.md#driving-a-run-the-agent-api)).
- The CLI.** `h2 agent <method> [JSON]` calls any method; short forms
- exist for common ones (`h2 agent --help` lists them). `h2 agent -` reads
- <script>` is the same with `expect` lines and one JSON verdict
- `h2 agent poke ...` takes a claim for its connection. A result prints as
- dispatch transfers as `lockstep`'s does: compare runs that all have it,
- `agents/<pid>.sock` under the cache directory (`agent_proto::discover`,
- answers on, left by a game that was killed, is removed. `h2 agent games`
- prints each game's socket, pid, map and phase, and `h2 agent <call>` with
- no socket named, no `$H2_AGENT` and nothing at the default socket uses
- JSON-RPC 2.0, one JSON object per line each way (`crates/extensions/agent-proto/src/lib.rs`).
- A request with an `id` gets one response with that `id`. Valid JSON that is
- not a request gets `-32600`; params fields a method does not know are
- refused; batches are not supported. Method names are `domain.verb`.
- The stamp.** Every result carries `at` (`agent_proto::At`):
- `map_ticks` ticks since the map loaded, `present` frames presented.
- Phase.** `loading` (no tick for 500 ms: `agent::TICK_STALE`), `menu`
- (mode `main_menu`, always: its background runs `cinematic_start` too),
- `gameplay` (campaign or multiplayer), `cinematic` (between HaloScript's
- `cinematic_start` and `cinematic_stop`, observed by the agent feature;
- simulation state. `slice`: read between scheduler slices with no tick for
- Errors** (`agent_proto::code`):
| 2 | `TIMEOUT` | `session.wait` or a deferred answer ran out of time |
| 5 | `SHUTDOWN` | the server is stopping |
| 7 | `BUSY` | `state.save` or `state.load` never found a quiescent boundary; `data.blockers` says what stayed live |
| 11 | `REFUSED` (`agent_proto::sim::REFUSED`) | the running session, placement or map cannot support the requested edit |
- Connections.** A client that half-closes (`nc -N`) still gets answers to
- 4096 lines for its client (`OUT_QUEUE` in `crates/extensions/agent/src/server.rs`); a
| `live.subscribe` | `{layers: ["objects", "players", "camera"], every?, key_every?}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch, activation}, layers, refused}`. The stream id is numeric. An unavailable checksum is null. Each unsupported layer has a reason under `refused`. |
| `live.status` | none | `{map: {name, checksum, epoch, activation}, streams, capture_us: {p50, p99, budget}, shed}`. Stream fields are `stream`, `layers`, `every`, `frames`, `bytes`, `queue`, `dropped` and `skipped`. Frames and bytes count complete writes, including the length prefix. |
| `live.close` | `{stream}` | `{closed: true}` if the stream existed. Disconnecting the binary socket also ends its subscription. |
- `every` samples one tick in 1–30 (default 1). `key_every` requests a
| `session.status` | none | `at`, `protocol`, `fps` (frame-stats window, when collected), `controller` (this connection holds control), `claimed` (any does) |
| `session.claim`, `session.release` | none | take or give up control; closing the connection releases it |
| `session.wait` | `{until, timeout_ticks?, timeout_ms?}` | `{at, ticks}` once every field of `until` holds |
| `session.subscribe` | `{events: ["phase","map","bsp","pause"]}` | then `event` notifications: `phase {from,to}`, `map {from,to}`, `bsp {from,to}`, `paused`, `resumed {why: request, timeout or released}` |
| `session.takeover_info` | none | `{window, pid}`, whether the game has a window and its process id |
| `session.request_takeover` | `{ask?, requester, name, operation, builds_image, source, level, own_data, estimate_seconds, timeout_seconds}` | `{id}` for the consent prompt, or for the running job's question with `"ask": "running"` |
| `session.takeover_status` | `{id}` | `{decision}` for the request |
| `features.list` | none | `{at, plan, features}`: the feature plan text the game printed at start, and each feature as `{id, on, reason?}` (`reason` when the plan turned it off), read from the plan's `feature <id>: on` and `feature <id>: off (<reason>)` lines (`agent_proto::FeatureState`) |
- `until` takes `tick`, `ticks` (more ticks than when the wait began),
- `activation` (exact match), `map_ticks`, `present`, `phase`, `map` and `paused`; all given must hold,
- answers at once. `timeout_ms` defaults to 30 s, at most 600 s; the failure
- `session.wait` replaces every "sleep and screenshot" loop. Switches do not
- The status result is `pending`, `allowed`, `alongside`, `declined`,
- `timed_out` or `unavailable` (the dialog failed to open). **Not now** and
- Back both decline. **Close and start** answers `allowed`: consent for that
- `session.pause`, `session.resume` and `session.step` need `session.claim`
- observer, still answering requests. It resumes by itself after `max_ms`
- `step {ticks}` (1 to 1800) runs that many ticks and answers once paused
| `host.pause` | `{on: bool}`, or `{}` to read | `{paused, held, tick}` once the window has applied it |
| `host.step` | `{ticks?}`: 1 to 1800, default 1 | `{paused, held, tick, stepped}` once the ticks ran and the game is held again |
| `host.window.resize` | `{width, height}` (each 64 to 16384), `{fullscreen: bool}`, or both with `fullscreen: false` | `{width, height, fullscreen, scale}` once the window has that shape |
| `world.count` | `{group_by: "kind" \| "team"}` | `{at, total, groups}`: the cheap first call |
| `world.query` | `{kind?, near?, tag?, fields?, limit?, offset?}` | `{at, total, offset, objects, more}` |
| `world.object` | `{id}` | `{at, object}`: every field the reader has for one object |
| `{bsp?}` (default 0) | `{bsp, enabled, broken, mask}` |
- `h2 agent net.replication` takes no parameters and returns, at one game
- (`crates/extensions/agent/src/net.rs`). It is the measuring method for the
| `world_mode`, `mode` | the simulation world's mode (`[0x4cf77c]+8`) and its name: `none`, `local`, `sync-server`, `sync-client`, `dist-server`, `dist-client`; null before the simulation exists |
| `distributed` | true in modes 4 and 5 |
| `objects` | per object tag group (`bipd`, `vehi`, `proj`, ...; `other` for a type outside the table): `{objects, with_entity}`, the live objects and how many have a replication entity (object body `+0xd4` is not -1) |
| `entities` | the sum of `with_entity` |
| `creature_type` | the registered creature probe slot, 17, or null when the definition is absent |
| `creatures` | live bindings `[{entity, type, object, updates}]`. `entity` includes the generation, and `object` is the local salted handle in the format used by `sim.objects`. `updates` counts movement operations the host writes to peer replication batches or the client applies over this binding's lifetime |
| `events` | `{total, by_type: [{type, name, count}]}`: replication events queued since the process started, by the engine's event type; types never seen are left out, and a type outside the table is counted under `other` with a null `type` |
| `traffic` | `{sent, sent_bytes, received, received_bytes, largest}`: the title's datagrams since the process started (`live_hle::traffic`) |
| `role` | `host` on a distributed server, `client` on a distributed client, `off` elsewhere or with `-actor-net` |
| `tick` | the simulation tick of the report; null when `role` is `off` |
| `peers` | on the host, each Live tunnel peer with the channel, in id order: `{peer, bytes_per_second, packets, bytes, keys, digests, records, dropped}`. `bytes_per_second` is the bytes charged in the last 30 host ticks, headers included; the rest count since the peer connected. `records` includes repeated copies of lines and cues; `dropped` counts offered records the budget left out. Empty on a client and on a System Link host |
| `client` | on a client, `{actors, puppets, waiting, lines_waiting, mode_state_bytes, digest_misses, key_requested, diverged, host_log_head}`; null elsewhere |
- base moved, is listed, not compared. It reads what `h2 agent raw` prints,
- or a `-` script's last line (`crates/apps/cli/src/agent_raw.rs`).
- `world.poke {id, part?, offset, hex}` (access: claim) writes 1 to 64 bytes
- at a tick boundary. `id` and `part` use `world.raw`'s names; `part`
- defaults to `object`. `offset` is a nonnegative byte offset from the
- The connection must hold `session.claim`. `sim.session` reports whether
- the session allows host writes; a session refusal is `INVALID_REQUEST`
- with `data.session`, and a request served between slices is `UNAVAILABLE`
- when the session allows writes. The result has `id`, `datum`, `part`,
- `base` (the part's first address), `offset`, `at`, and `before` and
- `after` (the bytes at the offset, lowercase hex).
- Game methods (`agent::havok::install`), read only, answered at a tick
| `havok.bodies` | `{near?, radius?, limit?}` | rigid bodies: `entity` (address, the key contacts use), `state` (`active`, `inactive`, `fixed`), `object` and `datum`, `pos`, `velocity`, `angular_velocity`, `inv_mass`, `mass`, `friction`, `restitution`, and `shape` where it reads: `addr`, `vtable`, `kind` (`capsule`: `a`, `b`, `radius`, `height`; `sphere`: `radius`; `transform`: `translation`, `child`; else `other`). `near` (an object id) keeps bodies within `radius` (default 5) of its body, nearest first, with `distance`; `limit` defaults to 64 |
| `havok.agents` | `{object}` | collision pairs and cached state ([below](../agent-api.md#havokagents)) |
| `havok.contacts` | `{object?}` | contact constraints: `a`, `b` (`entity`, `object`, `datum`, `fixed` for the level and static bodies) and `points`: `pos`, `normal` (from `b` toward `a`, **S**), `distance` (negative is penetration), `impulse` (what the solver applied on its last step), `friction`, `restitution`. With `object`, only its contacts, with it as `a` |
| `havok.digest` | `{}` | the ordered heap walk (sim-rewrite-havok.md §9.2): `digest` (16 hex digits, FNV-1a over every live byte and address), `bodies`, `constraints`, `pairs`, and `heap_used` (bytes in occupied pool slots). The arena hash does not cover the Havok heap, so this digest is what proves a physics rewrite; the sim report carries the same walk as its `havok.world` component |
- `object` is required. The reply has `object`, a stamp (`at`) and `agents`.
- malformed selector is `INVALID_PARAMS`; an absent object or a player
- between lives is `GONE`.
- `havok.pair.set {object, entry, word}` (access: claim) overwrites one
- at a tick boundary. `entry` is the pair's `entry` from `havok.agents` for
- against a fixed body (the level). `word` is eight hex digits, as
- `pair_word` prints them. It writes only a pair `havok.agents` would list
- is `GONE`, and so is an absent object or a player between lives. The reply has `object`, `entry`, `before` and
- `after`. It needs the same claim and host-write session as `world.poke`
- (`agent::havok::install_writes`; writer `set_pair_word` in
- `crates/live/live-telemetry/src/havok_agents.rs`). It exists to test
- `superbounce.state` (access: read; it takes no parameters and refuses any
- with `INVALID_PARAMS`) answers from the state the
- (`crates/extensions/game/src/superbounce_mode.rs`; wire types in
- `crates/extensions/agent-proto/src/superbounce.rs`; registered in
- `crates/extensions/game/src/agent_glue.rs`). The mode is described in
| `active` | the mode is on: the game's script requires `hud.superbounce` (Superbounce, Bounce Practice) and `superbounce.set` has not forced it off. Everything below is empty or `idle` while it is off |
| `primed` | the local player's level pair word is -3 |
| `word`, `word_signed` | the pair word as eight hex digits and as a signed number; absent without a level pair (between lives) |
| `position` | the local player's position, world units |
| `nearest` | the bounce spot nearest the player: `distance` to its edge, `launch` (units/s), `fall` (units) and its `edge` |
| `scan` | `status` (`idle`, `scanning`, `ready`, `failed`), `map`, `error`, and the `bounce` and `crouch` spot counts |
| `airborne` | the local player's biped is off the ground (biped flags bit 0, as `world.object`'s `physics.airborne`); absent without a player |
| `vz` | the local player's vertical velocity, world units a second, up positive |
| `yaw` | the way the local player's unit aims, radians from +X toward +Y: the first-person camera's yaw |
| `velocity` | the local player's velocity, world units a second; absent without a player |
| `options` | the HUD's pieces this game, the Superbounce script's options as the variant set them: `targets` (`common`, `likely`, `all`) and the bools `velocity`, `alignment`, `miss_info`, `medals`, `second_bounce`, `approach_guide`, `charge_segments` ([superbounce.md](../../game/superbounce.md#the-huds-options)) |
| `charge` | the charge meter: `phase` (`near`, `under`, `held`, `primed`, `lost`), `fill` (0, 1/3, 2/3, then 1 less `still_ticks` over 38 while primed, 0 while lost) and `still_ticks`; absent while hidden ([superbounce.md](../../game/superbounce-landings.md#charge)) |
| `verb` | the prompt line's verb: `let_go_of_crouch` while `under`, `walk_out_to_stand` while `held`, `keep_moving` while primed and still for 19 ticks or more, `charge_again` for 150 ticks after a loss while no crouch step has begun |
| `lost_reason` | why the charge went, while `lost` and while the verb is `charge_again`: `stood_still` (the word left −3 after 30 still ticks or more), `crouched` (a crouch while primed), `unknown` (the word left −3 for another reason); absent otherwise ([superbounce.md](../../game/superbounce-landings.md#charge)) |
| `nav_target` | the navpoint's target: `at` (the spot's stand point), `kind` (`crouch` while not primed, `strip` while primed: the chosen strip, [superbounce.md](../../game/superbounce.md#floor-marks)), `distance` (world units), `fall` (a strip's drop, world units; 0 for a crouch spot) |
| `landing` | while airborne and falling toward a strip, the landing the assist predicts: `at` (at the strip's floor height), `verdict` (`on_slot`, `fixable`, `miss`), `speed` (the touchdown speed, units/s), the strip's `need`, `miss` (world units outside the slot, 0 inside) and `locked` (in the slot at `need` or faster: the [lock-on](../../game/superbounce.md#lock-on) is red) |
| `last_landing` | the last touchdown within 0.5 of a strip, or primed bounce anywhere, filed when its launch window (the touchdown tick and the 3 after) closes, or a primed fall through a strip's floor, filed on the tick it crosses: `result` (`bounce`, `not_primed`, `missed`, `too_slow`, `no_bounce`, `through`), the touchdown's game `tick` (for `through`, the touchdown's it followed, else the crossing's) and the origin `at` on the tick before ([superbounce.md](../../game/superbounce-landings.md#landings)) |
| `off_spot` | the local player's last 16 bounces no marked strip accounts for (none within 0.5, or outside the catch of the one there is), oldest first; absent when none: the touchdown's `tick`, the origin `at`, the `edge` of the scanned bounce spot within 0.5 (absent with none), `speed_in` and `launch` ([superbounce.md](../../game/superbounce-landings.md#landings)) |
| `standing_fits` | whether the standing capsule fits at the player's origin, as `world.object`'s `physics.standing_fits`; absent before the scan is ready |
- `superbounce.set {"on": false|null}` (access: act) forces the mode off in
- any game (`false`), or follows the game again (`null` or `{}`: its
- script). It cannot turn the mode on: `true` is refused with
- `invalid_params`, because only a game whose script requires
- `hud.superbounce` runs the mode. The answer is `{"mode": "off"|"game"}`.
- The tick's on/off check runs every 30 ticks, so `superbounce.state` shows
- `superbounce.medal_test {"chain": N}` or `{"bounced": true}` (access: act)
- files a bounce at chain count `N` (1 and up), or a Bounced, for the first
- (`superbounce_chain::bounce_medals_with`); `"monster": true` with `chain`
- files it as a Monster Bounce. The chain is left at `N` and live,
- medal line and notification) and `superbounce.state`'s `medals` as a real
- one does. The answer is `{"player", "chain", "medals", "line"}`: the slot,
- 0, with both or neither of `chain` and `bounced`, or with `monster` and
- `superbounce.assist` (access: read; no parameters) says what the hold-X fall
- `crates/extensions/game/src/superbounce_assist.rs`).
| `active` | the superbounce mode is on; the assist works only then |
| `enabled` | the player's `superbounce.fall_assist` setting is on; the assist is opt in and never steers while it is off |
| `test_hold` | `superbounce.assist_test` is holding the button |
| `engaged` | the assist steered the player at its last read |
| `reason` | why. Engaged, how the strip was chosen: `kept` (the strip the read before steered onto, still in reach), `marked` (the strip the HUD marks) or `nearest` (the strip the predicted landing misses by least, one the fall is fast enough for first). Held and not engaged: `not_falling`, `no_strip` (none within 6 units), `refused` (every strip near was) or `no_body`. Else `not_held`, `off` (the setting) or `inactive` (the mode) |
| `target` | the chosen strip's stand point, world units |
| `point` | the point of that strip being landed on: the stand point moved along the strip to where the fall comes down |
| `aim` | the point being aimed at, a little before `point` along the drift (X, Y) |
| `landing` | where the player lands if the stick is let go (X, Y) |
| `fall_time` | seconds until touchdown at the strip's height |
| `stick` | the left stick sent to the guest, `[x (right), y (forward)]`, -1 to 1 |
| `from`, `velocity` | the player's origin and velocity at the read; absent while the button is up |
| `candidates` | the strips judged at the read, while the button is held: the chosen one, the others in reach, then the refused, nearest first, at most 12 |
| `chose` | the read the assist last engaged on, the choice at the press: `target`, `point`, `reason`, `from`, `velocity` and `candidates`. Kept after the button is let go, until the next engagement |
- `target` to `stick` are absent while not engaged. A candidate:
| `stand` | the strip's stand point, world units |
| `verdict` | `chosen`; `passed` (in reach, another was chosen); or why it was refused: `above` (not below the player), `long` (a fall over 3 s), `far` (over 3 units sideways), `out_of_reach` (the stick cannot bring the landing onto it in the time left), `covered` (a floor is between the player and it) or `walled` (something solid is between) |
| `distance` | from the player to the strip's edge, sideways, world units |
| `point`, `fall_time`, `miss` | in reach only: the point of the strip to land on, the seconds to it, and how far the coasting landing misses the aim |
- A wrong pull shows in `chose`: the strip it took, why, and what it made
- `superbounce.assist_test {"hold": true|false}` (access: act) holds the
- a strip in reach is steered while it is `true`. The answer is
- `{"hold": bool}`. It holds until set `false`. The setting is turned on and
- off with `settings.set`, live.
- `superbounce.verify {kind, ...}` (access: act) starts a batch of trials of
- agent ticker (`Agent::add_ticker`), and answers at once with the plan
- `crates/extensions/game/src/superbounce_verify.rs`; wire types
- `agent_proto::superbounce::Verify*`). One batch runs at a time. It needs
- the mode on and its scan ready, except over explicit `points`; the
- `player.<n>` seat needs the `superbounce-assist` feature's pad tap, and
- every kind but `route` needs the `agent-calls` feature's teleport
| `kind` | `crouch` (crouch outside the lip, walk in, let go under it, walk out: does the word turn −3?), `drop` (a fall onto a strip with the word set: does it launch?) or `route` (a bot's whole `bots.bounce_to` route) |
| `spots` | the scan's spots of that kind by index, best score first (the order `dry_run` lists); default the first `max_spots` (12) that have not failed |
| `points` | explicit spots instead: `{stand, out?, edge?, drift?, need?}` (a drop's hit is the one catch either side of `edge`, [superbounce.md, "The target: the collision edge"](../../game/superbounce.md#the-target-the-collision-edge)); never saved |
| `seat` | `player.<n>` (default `player.0`, driven through the first controller's pad) or a brain-driven bot by gamertag, handle or `player.<n>` (driven by holding its pad); `route` needs a bot |
| `turns`, `distances`, `offsets` | crouch: approach turns in degrees from straight in (default `[0,25,-25]`), starts outside the lip along it (default `[0.3,0.6]`), and X/Y offsets (default `[[0,0]]`) |
| `across`, `along`, `heights` | drop: offsets across the spot's target line toward the drift (default `[-0.1,-0.05,0,0.05,0.1]`), along it (`[0]`), and heights over the strip (`[1.5,2.5,3.5]`) |
| `prime` | drop: `set` (−3, default), `none` (0, the control) or `keep` |
| `drift_stick` | drop: the stick along the drift while falling, 0 to 1 (default 0) |
| `settle_ticks`, `hold_ticks`, `out_ticks`, `fall_ticks`, `route_ticks` | 20, 15, 30, 120, 3600 |
| `to`, `mode`, `repeat` | route: the target (default 6 up from each strip's stand), `help` or `fun` (default), and the routes a spot (3) |
| `trace` | keep the per-tick trace of this trial index (`phase`, `pos`, `vz`, `airborne`, `request`, `crouch_button`, `crouch`, `standing_fits`, `word`) |
| `save` | write the map's verified file at the end (default true; never for `points` or `route`) |
| `dry_run` | plan and list, run nothing |
- dry_run}`: each spot's `index`, `kind`, `stand`, `status` and `score`, and
- each trial's `spot`, `start`, `yaw`, `distance`, `offset` and `height`.
- `superbounce.verify_status {wait_ticks?, brief?}` (access: read) reports
- on the newest batch: `state` (`none`, `running`, `done`, `failed`,
- `cancelled`), `job`, `kind`, `seat`, `map`, `done` and `total` trials,
- `current` (the trial and its phase), `spots`, `results` (each spot's
- `superbounce.view` (access: read; no parameters) says what the bounce
- `crates/extensions/game/src/superbounce_view.rs`; wire types in
- `crates/extensions/agent-proto/src/superbounce_view.rs`), as of the last
| `active` | the superbounce mode is on; everything below is empty while off |
| `mode` | `none`, `pulse` (a tap's pulse still shows) or `hold` |
| `button` | the flashlight action at the last tick |
| `test_hold` | `superbounce.view_test` is holding |
| `pulse_age`, `pulse_front` | seconds since the pulse left, and its ring's distance from the player in world units; absent without a pulse |
| `grow` | the tracker's growth into the map, 0 to 1 |
| `spots`, `marks`, `hidden`, `tested` | the spots the view knows, those marked in the world now, of those the ones behind a wall, and those with a line-of-sight answer |
| `tracker` | `enabled` (the variant shows the tracker), `range` (world units) and `blips`, the next spot and then the one after it, at most two: each `kind` (`bounce` or `crouch`), `proven`, `x` and `y` (right and ahead over the range; on the unit circle when out of range), `level` (`same`, `above`, `below`), `rim` (out of range) and `nav` (the next spot) |
| `map` | while held or shrinking: `floors`, `edges` and `pairs` drawn, and its `blips` over the map's 50 m |
- `superbounce.view_test {"mode": "pulse"|"hold"|"none"}` (access: act)
- drives the view with no pad: `pulse` fires a pulse at the next tick,
- `hold` holds until `none`, `none` lets go and stops the pulse. The answer
- is `{mode, test_hold}`. Refused while the mode is off.
- `superbounce.ghosts` (access: read; no parameters) lists the loaded
- each spot, kept in `<map>.ghosts` in the bots directory
- `crates/extensions/agent-proto/src/superbounce.rs`; the game side is
- `crates/extensions/game/src/superbounce_ghost.rs`.
| `active` | the mode is on |
| `map` | the map the runs are for, once the mode has ticked on one |
| `file` | the ghosts file's path, once there is a bots directory |
| `error` | why the file could not be read (another version, a bad line); it is not written over then |
| `ghosts` | each run: `index` (what `superbounce.ghost_show` takes), `at` (the spot's stand point), `landing` (the touchdown origin), `source` (`bot`, `verify` or `player`), `ticks` (30 a second), `launch` (units/s up out of the bounce), `crouch_primed` (it primed itself with a crouch), `presses` (`frame` and `button`, `crouch` or `jump`) |
| `looked_at` | the run whose spot the first local player looks at |
| `prompt` | the prompt line's ghost verb, as the HUD writes it (X's glyph, U+E102; B's, U+E101, on the Bumper Jumper layout) |
| `playing` | the playback: `index`, `seconds` since it started by the clock, the run's `tick` then (with the fraction to the next), the figure's `at`, and `done` once it holds on its last frame; absent when none plays |
| `recorded` | runs kept (a new best for their spot) since the game started |
- `superbounce.ghost_show {spot?, stop?}` (access: act) plays run `spot`
- once from its start, or the run looked at without `spot`; `{"stop":
- true}` stops the playback. It answers `playing`, the `index` and the
- `superbounce-ghost` feature, which is off by default.
- `superbounce.reload {map?}` (access: act) re-reads a map's `<map>.spots`,
- `.learned`, `.verified` and `.ghosts` from the bots directory, for the
- loaded map without `map`, and republishes them the way a map load does,
- Refused with no map and no `map`, or with no bots directory. Game side:
- `crates/extensions/game/src/superbounce_reload.rs`,
- `bots::spots::reload_in`.
- `superbounce.tune {bounce_speed?, catch?, launch_gain?}` (access: act)
- seam needs (`superbounce::speed::BOUNCE_SPEED`, above 0), the catch
- (`superbounce::CATCH`, world units, 0 up to but not including 1) and the
- launch gained per unit/s over the threshold (`speed::EXCESS_GAIN`, 0 or
- process. The answer is `bounce_speed`, `catch` and `launch_gain`, each
- `{value, source}` with `source` `default` or `override`.
- `pogo.state` (access: read; no parameters) says what the Pogo Stick is
- `crates/extensions/game/src/pogo_agent.rs`; wire types in
- `crates/extensions/agent-proto/src/pogo.rs`), as of the last tick:
| `active` | the superbounce mode is on; `riders` is empty while off |
| `option`, `style` | the script's `pogo` and `pogo_style` options as the variant set them: whether a spawn is given a stick, and which look (`unsc`, `covenant`, `forerunner`) |
| `map` | the loaded map's name; absent without one |
| `weapons` | the three sticks' weapon tags in the loaded map, in the order `unsc`, `covenant`, `forerunner`; empty when the map does not carry the kit (the `pogo` feature off, or the map refused), and then no stick is given |
| `riders` | every player holding a stick, the local players first: `player` (the slot), `local`, `unit` and `weapon` (object handles for `world.object`), `style`, `phase` (`carried`, `mount`, `ride`, `compress`, `spring`, `dismount`), `mounted` (0 in the hands to 1 on the stick), `compress` (0 the piston out to 1 in), `flare` (1 on a bounce's tick, fading), `tuck`, `trick` (absent without one: `id`, `name`, `progress` 0 to 1, half at the apex) and `posed` (the riding pose is drawn over the game's own) |
| `camera` | the chase camera: `on` (a local player holds a stick), `drawn` (the last frame was drawn from it; false headless, in split screen and under Forge's or Theater's camera), `room` (the share of the boom the level leaves room for), `share` (the boom's share now, eased) and `fallback_frames` (frames placed from the body's eye and aim because the game's own view did not read as a camera, since the game started) |
| `tricks` | the ids `pogo.test` takes |
- `pogo.test {"trick": "<id>"}` (access: act) shows a trick from its start
- The answer is `{player, trick, name}`. Refused while the superbounce mode
| `profile.counters` | none | the frame-stats rolling window (`timeline::phase`), the numbers `H2_FRAME_STATS` prints: `frames`, `fps`, frame ms, `phase_ms`, `counts`. When nothing else collects them the glue starts a quiet 5 s window |
| `profile.capture` | `{ticks \| ms, budget_ms?, spikes?, sample?}` | percentiles, phases, counters and spikes over a window (below) |
| `profile.fallbacks` | `{top?}` | where the fallback interpreter runs |
| `profile.faults` | `{top?}` | the fault census: every guest memory fault and whether execution went on from it |
| `profile.sample.start`, `.stop`, `.status` | `{hz?}`, `{inline?}`, none | the in-process sampler |
- `render.frame` and `render.stats` are host methods (types in
- `crates/extensions/agent-proto/src/render.rs`, logic in `crates/extensions/agent/src/render.rs`).
- with its target (`640x480 X8R8G8B8`), clears, draws, vertices, outcomes,
- Omitting `source` or specifying `"display"` reads or arms the display
- recorder independently. A request for `"private"` returns `UNAVAILABLE`
- `render.frame_layout {"arm":true,"max_draws":2000,"snap":[0,3]}`
- same limits as `render.frame`. Call `render.frame_layout {}` for the recorder
- The host records the layout in `crates/extensions/game/src/d3d_diag.rs` and
- `crates/extensions/game/src/d3d_diag/layout.rs`. The display allocations and
- readbacks come from `crates/platform/d3d/src/scaled.rs` and
- `crates/platform/d3d/src/gfx_backend.rs`.
- No params. `display_rate` is the frame owner's display-rate loop, the
- ([framescene.md](../../graphics/framescene.md), section 7). `counters.line` is the frame-stats window's
- summary. `counters` also reports the per-frame means for `shadow_walk_layers`,
- each `native_scene_lists_*` stage and the overlay stage's
- `ui` is the layout the menus and the HUD are drawn with
| `output` | `[width, height]`, the window the UI is drawn at; the guest's 640x480 canvas before the first menu scope |
| `scale` | the menu fit's `[sx, sy]`, guest window units: guest `p` draws at `p * scale + offset` |
| `offset` | the fit's translation, guest window units |
| `anchor` | how the HUD's corner groups anchor: `pinned` to the window's edges at their physical scale, or `centered` on the safe area's edges |
| `atlas_scale` | the whole multiple of a glyph's own size the text atlas rasterises at (0 before the first text run) |
| `atlas_page` | the side of a glyph page in texels |
- The fit is `render_bridge::ui_layout`'s one rule: the safe area is the
- largest centred 4:3 box (`min(w/640, h/480)` display pixels per guest
- window (`host.window.resize`), reads `render.stats`'s `ui`, and expects
- `scale` to hold the new window's uniform fit while `anchor` and
- `atlas_scale` stay.
- `display_rate` is present while a paced frame owner runs it: a window
- or GPU serve with the render thread on, `frame-scene.sink` not off, and no
- locked mailbox (`H2_BOOT_STOP_AT_PRESENT`, an unpaced display or a
| `rate_hz` | the rate `graphics.fps` asks for; 0 at 30 |
| `shown_hz` | the rate after the headroom cap |
| `capped` | display frames do not fit, so the rate is held below `rate_hz` |
| `swaps` | owned presents the loop saw |
| `blended` | display frames drawn with blended cameras, at a Swap or between |
| `swap_blends` | of `blended`, the ones shown in place of a Swap's scene |
| `between_newest` | frames between Swaps drawn as the newest scene (alpha 1, a cut or a gap) |
| `failed` | display frames that failed; the window kept the frame before |
| `resets` | pairs dropped by a Swap without the graph |
| `avg_cost_us` | mean render-thread cost of a display frame |
| `moved_draws` | carried draws drawn on a blended camera |
| `moved_stages` | typed stages drawn on a blended camera |
| `held_stages` | typed stages in a game view drawn at their tick's camera |
| `marks_moved` | views on display frames whose world marks were moved onto the camera the frame drew them with |
| `marks_held` | views on display frames whose world marks kept their image's camera (no drawn view at their viewport) |
- At `--fps 60` and above with the frame owner on, `blended` rises with
- play; at 30 it stays 0 and `swaps` still counts. The loop is in
- `crates/rewrites/frame-scene/src/display.rs` and
- `crates/rewrites/native-render/src/frame_graph/display_rate.rs`.
- for these counters. `render.scene` includes them when the map epoch,
- after a renderer reset. `render.scene` includes the same field when its
- `e3.tiles`, `e3.sun_tiles`, `e3.sun`, `e3.marks` and `e3.lights`. Each row has
- `name`, submitted `draws` and `gpu_ns` (null for an empty or unmeasured
- `H2_BOOT_GFX_BETWEEN=N` writes the first N display frames between
- `render.capture_clip {"tick":9000,"ticks":180,"fps":120}` arms a clip on
- the game thread. `tick` is 1 to 240 ticks ahead, `ticks` is 1 to 300, and
- `fps` is 30, 60 or 120. The reply contains `id`, `path`, `frames` and
- `written`. The GPU hooks write `frame-00000.png` onward under `path`, a
- `render.capture_clip.status {"id":1}` returns the same fields for the
- latest clip. When `written` equals `frames`, all PNG files are closed.
- The state is in `crates/extensions/agent/src/render/clip.rs`. The GPU
- writer is in `crates/extensions/game/src/d3d_glue/clip_png.rs`.
- ([game-view.md](../../editor/game-view.md)) and `h2 agent picture`. Frames
- come from the same handoff `host.screenshot` reads (the window's newest
- `render.capture`.
| `render.picture.open` | read | `{width?, fps?, transport?}` | `{id, transport, path?, size, format, fps, at}`; then a `picture` event per frame (ring) or binary frames (stream) |
| `render.picture.close` | read | `{id}` | `{closed}`: whether the picture was open |
- `transport` is `ring` (the default) or `stream`. `width` is the widest
- frame wanted, 16 to 2560; a frame keeps its aspect, fits a `width` x
- `width` box and is never scaled up. `fps` is 1 to 60. A ring defaults to
- 1280 wide at 60, a stream to 640 wide at 15. `size` is the largest frame,
- `[width, width]`; each frame's own size is in its slot and its event. A
- game with no frame source (no window: `h2 serve`, the Xbox app) answers
- `UNAVAILABLE`.
- Ring.** For a client on the same machine. `path` is a file under the
- cache directory's `pictures/`, mode 0600, named for the process and the
- Stream.** For a game reached over `tcp://`, which cannot share memory.
- switches to length-prefixed binary frames, as `live.subscribe`'s does,
- events or has requests in flight is refused with `BUSY`, and an
- in-process connection with `UNAVAILABLE`. Each frame is a little-endian
- `picture` event's fields as JSON, and a JPEG (quality 75, no alpha)
- (`agent_proto::picture::read_stream_frame`). At most three frames wait
- Lifetime.** `render.picture.close`, the opening connection closing or
- The methods are answered on the executor (`crates/extensions/agent/src/exec/picture.rs`);
- `crates/extensions/agent/src/render/picture.rs`; the wire types, the ring
- and the stream codec in `crates/extensions/agent-proto/src/picture.rs`.
- `agent_proto::picture::hub` (`crates/apps/app/src/picture.rs`). The client
- is `crates/apps/cli/src/agent_picture.rs`.
- `render.marks` (no params) reads where the window drew the world marks
- returns `armed: false` with no presents. Unarmed, the window pays one
- atomic load a present. Each later call returns `armed: true` and
- `presents`, oldest first. Each present has:
- `seq`, counting the window's draws since the probe was armed;
- `age_ms`, how long ago the window drew it;
- `frame`: `guest` (a genuine image), `synthetic` (an in-between frame
- at 60 or 120 fps), `no_world_view`, `raw` or `loading`;
- `fraction`: a synthetic frame's blend fraction, 0 the frame before and
- `shifted`: the synthetic frame drew some view with another camera than
- `anchors`: the first 16 mark groups' anchors in window pixels through
- `render.markers` reads the Forge game type markers the window's last
- game"). Params are `{}` or `{"kind": "spawn"}`, a kind name of
- `game_markers::Kind::name` (`spawn`, `flag_stand`, `hill`, `ff_drop`, ...) or `wall`;
- a name that is not one is refused with `invalid params` and the list of
- The walls a running rules script shows in play (`show_area`) are rows
- too: `kind` is `wall`, `thing` is `zone:<hex of the zone object>` and
- `team` the colour of its wall; they count in `shown`, are not held to
- The method is the host's (`crates/extensions/game/src/forge_glue/markers_world.rs`,
- `add_method`; the name is `agent_proto::method::RENDER_MARKERS`).
- `render.particles.refusals` says why the native particle path left draws to
- log also carries (`Particle/Geometry/primitive=428`). The answer keeps, per
- params; any field is refused with `invalid params`.
- The checks, in the order they run: `indexed`, `pretransformed`,
- `layouts` (not one stream layout), `streams` (not one stream),
- `stride` (stream index, layout against stream, or the producer's
- stride), `attributes` (not exactly one for a particle),
- `attribute_slot` (offset or register), `primitive`, `vertex_count`,
- `data_len`, `data_type`, `inputs` (the shader's provided registers),
- `corner_vertices`, `corner_compare`, `corner_differs` (the stream
- differs from the published corner values), `unpublished`,
- `data_differs` (a dynamic call's bytes), `indices`, and the ribbon
- bytes' `raw_bytes`, `raw_attributes` and `raw_vertex_count`.
- `count` totals the run, not one present; the per-window counters are in
- the log. `producer` is the producer of the first draw (`Particle`,
- `Dynamic`, `Immediate`).
- The method is the host's (`crates/extensions/game/src/native_particles_admission.rs`,
- `add_refusals_method`; the name is `agent_proto::method::RENDER_PARTICLES_REFUSALS`).
- feature (`crates/extensions/game/src/coop_look.rs`). With no parameters it returns
- the most recent 256 rendered local views, oldest first; `{"after":123}`
- returns samples newer than that cursor. `latest` is the most recent
- Each entry in `frames` has a monotonic `sample`, the guest render `frame`,
- the local `user` (0 to 3), and a `status` such as `active`, `waiting`
- (awaiting a baseline), `host`, `setting off`, `cinematic`, `dead`,
| `sent` | Latest outgoing retail look, also recorded with the setting off for A/B timing |
| `confirmed` | Local player's yaw and pitch from the last applied update carrying that player's action |
| `predicted` | Current absolute target from the still-pending sends, or confirmed aim once they have drained |
| `camera`, `drawn` | First render camera before and after the adjustment; all aims are `{yaw,pitch}` in radians, or null when unavailable |
| `correction_rad` | Largest reconciliation error for this draw in radians (Euclidean length of shortest yaw difference and pitch difference); a pending turn's lead is measured by comparing predicted and confirmed |
| `update`, `last_send`, `matched_send`, `matched_update` | Last applied update, last sent sequence, and the sequence/update pair most recently matched by quantized aim; null before the event |
| `pending`, `dropped` | Unmatched sends, and the number superseded or expired since this continuous camera began; identical quantized actions have indistinguishable echoes |
| `render.compare` | read | `{a, b, diff?}`: two PNG paths, and where to write a difference image | `{size: [w, h], differing, max_channel, mean, first: [x, y] \| null, within_bar, diff?}` |
| `render.spawn_view` | act | `{index, yaw_offset?}` or `{reset: true}` | `{index, count, position: [x,y,z], yaw}` or `{reset: true}` |
| `render.view` | act | `{eye: [x,y,z], yaw, pitch, fov?}` or `{reset: true}` | `{eye, yaw, pitch, fov}` or `{reset: true}` |
| `render.view.current` | read | `{seat?}` | `{eye: [x,y,z], yaw, pitch, fov, private?}` |
| `render.capture` | read | `{tick?, pass?, lighting?, game_id?}` | `{path, tick, present, pass?, lighting?, scene_hash, source?}` |
| `render.scene` | read | `{dump?}` | `{sealed, map_epoch, frame, tick, hash, cut, views, counters, entries?, carried?, carried_census?, carried_activation?, carried_checkpoint?, visibility_first_mismatch?, light_visibility_first_mismatch?, first_person_visibility_first_mismatch?, view_record_first_mismatch?, dump?, display_frame?, private_frame?}` |
| `render.parity` | act | `{arm: true}` | `render.compare`'s fields plus `tick` |
| `sim.hashes` | read | `{from_tick?, max?}` | `{rows: [{tick, gameplay, arena?}], dropped, resets, truncated?}` |
- (frames are written opaque). `differing` counts pixels with any channel
- off by more than 2; `max_channel` is the largest difference of one
- channel; `mean` the mean absolute difference over every channel of
- every pixel; `first` the first differing pixel in row order, `null`
- when none. `within_bar` is the parity bar: `max_channel <= 8` and
- `differing` at most 0.01% of the pixels (one in 10,000). With `diff`,
- `INVALID_PARAMS` with `data.error` naming the kind: `size_mismatch`
- (with `data.a` and `data.b`, each `[w, h]`), `read` or `write` (with
- `data.path`). The decode runs on the game thread; `h2 agent frame
- `render.spawn_view`.** Selects a player starting location from the
- above the start, looking along its facing. `yaw_offset` turns it in
- renderer accepts the private view. A following `render.capture {}`
- An invalid index gives `INVALID_PARAMS`, a missing or unreadable start
- playerless world view in `spectator_view.rs`.
- drawn view's own lens and the reply gives `fov: null`. The reply
- `render.capture {}` takes the next tick's frame from that eye. The
- `render.view` and `render.spawn_view` select one camera: either
- activation. Missing, non-finite or out-of-range params, or `reset`
- with a pose, give `INVALID_PARAMS`; no loaded scenario gives
- `UNAVAILABLE`. Types: `ViewParams` and its `check` in
- `render.view.current`.** Reads the pose of the view the game draws for
- local player `seat` (0 to 3, default 0), at the tick boundary, in
- While a `render.view` or `render.spawn_view` camera is selected and its
- scenario is loaded, the reply is that camera's pose with `private: true`
- a seat without a screen) gives `UNAVAILABLE`; a `seat` outside 0 to 3
- or unknown params give `INVALID_PARAMS`. A Theater camera is not
- `CurrentViewParams` and `CurrentView` in
- `lighting` with `pass` gives `INVALID_PARAMS`. The override applies
- `no native world range in this scene uses the installed lightmap`.
- Tests: `private_captures_draw_the_same_pinned_scene_with_each_requested_mode`
- and `retained_stage_uses_the_current_private_attachments_on_every_draw`.
- For a Live gallery capture, pass the capture session's `game_id` after
- `source: {game_id, map_id, map, camera, digest}`. The game resolves the
- draw's cache alias to the installed package's full `MapId` and base map name.
- uses. `digest` is the SHA-256 of the exact PNG the display writer saves.
- `game_id` cannot be combined with `pass` or `lighting`.
- `render.parity`.** `{"arm": true}` requests one game frame from the
- limit times out. `{"arm": false}` gives `INVALID_PARAMS`.
- `sim.hashes`.** The running game's post-tick digests in the rows sim
- mode writes to its report as `state_hashes_v1` (sim.md §6): `tick`,
- `gameplay` (16 hex digits) and, under `impl.sim.arena-hash`, `arena`
- (the game-state arena's hash). Boot's `agent-frames` tick observer
- (`resets`). `from_tick` and `max` pick rows; `truncated` says more
- `crates/extensions/agent-proto/src/render.rs` (`ResidualParams`,
- `ResidualReply`), logic in `crates/extensions/agent/src/render/residual.rs`,
- `crates/rewrites/render-bridge/src/residual_census.rs`, the adapter's
- watch in `crates/platform/d3d/src/residual.rs` and
- `crates/platform/d3d/src/runtime_glue.rs`, glue in boot's `scene_glue`
| `render.residual` | read | `{arm?: {frames}, top?}`: `frames` 1 to 3600, `top` 1 to 500 (50) | `{state, window?, totals?, producers?, callers?, unattributed?, sites_full?, unbalanced?}` |
- Arming.** `{"arm": {"frames": N}}` starts a window at the next guest
- `Swap` and ends it after N more; a new arming replaces one in progress.
- `{}` reads. `state` is `idle`, `armed` (waiting or counting) or `done`.
- carry the window before it. `h2 agent residual N [top K]` arms, polls
- until a new window is done, and adds `per_frame`: each total over the
- `window`.** `{frames, game_frames, presents: [first, last], ticks:
- process started; `ticks` the last tick that had ended at each end
- entered a game view, through the `game_view` producer, so it is zero
- without `frame-scene.census`. `timing` is true on a `render-timing` run.
- `callers`**, at most `top`, by `residual`, then calls:
- `{function, sites, entries, draws, inline_draws, residual, d3d_ms}`.
- addresses (read at `[esp]`), named as `profile.fallbacks` names one
- (`recomp::fallback::function_at`), or the return address itself before
- `producers`**, at most `top`, by `residual`, then `entries`:
- `unattributed`.** `{draws, entries}`: with producers, the `unknown`
- Armed, it reads `[esp]`, copies the device's tally three times and
- armed. The always-on frame counters `d3d_entries`, `d3d_residual_draws`
- and `d3d_push_words` are in [`profile`](../agent-api.md#profile-where-a-frame-goes).
- Tested in `crates/rewrites/render-bridge` (the window, caller rows,
- draw rig (`tests/residual_tally.rs`: the drain and the entry's split
- `load.timeline {"last": N}` is a core method (8 by default, 1 to 32 kept;
- `crates/extensions/agent/src/loads.rs`).
- What a load is.** Read at each tick: the map changed (`map`), the BSP
- changed (`bsp`), or no tick came for 500 ms (`stall`, the point the
- `loading` phase starts). The run's first tick ends a `boot` load timed
- from when the agent started. Each load has a `stall` segment (the last
- tick before to the first after) and a `settle` segment (the next 30
- ticks); a load cut short by the next one says `settled: false`.
- totals (`timeline::phase::totals`) across it: frames, ms per phase
- (`cpu`, `gpu-wait`, `render-wait`, ...) and each counter that moved
- (`read_bytes`, `uploads`, `shaders`, `pipelines`, `readbacks`, ...).
- off, `no_frame_stats` says the counts are missing.
- Now.** While no tick has come for 500 ms, `current` is the stall so far,
- and the host time of each file call: `read_bytes` and `file_io_us`).
- `game.code` (read, no params) says which game code this start of `h2 play`
- full-speed module, or the Cranelift fast tier (`--tier fast`, or
- `bridge` while clang builds the full-speed one in the background). Host method,
- `crates/extensions/game/src/game_code.rs`; `h2 play` supplies it, other
- `background.state` is `building`, `ready`, `failed` (from the log: its
- `clang (…): N compiled` summary line, or an `Error:` line), or `none` when
- `--tier` forced one). `elapsed_s` is the log's age; `typical_s` is the
- estimate the banner counts down from; `step` is the log's last line.
- `game.code.restart` (act, no params) starts `h2 play` again in this
- process: the same executable and arguments by `exec`, with the
- (`UNAVAILABLE`, with `restart.why`'s text) unless this start runs the fast
- `game.menu` (act, no params) leaves the running campaign or multiplayer
- (`LEAVE_SESSIONS`) and call `request_mainmenu(0)`, so the return-to-menu
- `crates/extensions/game/src/game_menu.rs`; it needs the `agent-calls`
- `session.wait`, then measure it:
- `game.load {scenario, difficulty?, variant?}` (act) queues any campaign
- menu or a running game. `scenario` is a map file's stem
- (`03a_oldmombasa`, `lockout`) or a scenario path, any case, or a level of
- a campaign installed under the data root (`a30`), resolved as
- `h2 play --level` resolves it (`level_boot::resolve_in`). `difficulty`
- (campaign only) is `easy`, `normal` (default), `heroic` or `legendary`.
- `variant` (multiplayer only) is a game type, `slayer` (default),
- `oddball`, `juggernaut`, `king`, `ctf`, `assault` or `territories`: a
- `VARIANT_DEFAULTS` as the level boot builds Slayer's
- agent's guest calls at the next tick (`Call::QueueGame`), as
- `maps.test.load` queues a test map. Host method,
- `crates/extensions/game/src/game_load.rs`; it needs the `agent-calls`
- `check` the game's options check and `queued` whether the options went to
- the next transition. Wait for the level with `session.wait` on its `map`
- and `phase: "gameplay"`.
- Refusals: an unknown level is `INVALID_PARAMS` listing the disc's
- levels; so are a `difficulty` for a multiplayer map, a `variant` for a
- machine plays its game, so leave it first with `game.menu`. Without the
- `agent-calls` feature, `UNAVAILABLE`.
- `game.levels` (read, no params) lists what `scenario` takes:
- `live.join.open {"url":"..."}` submits a website link to the same inbox as
- the macOS app and `h2 play --join`. It returns `{"id":1}` on acceptance.
- Malformed links return `INVALID_PARAMS`. Reopening the same pending link
- returns its existing ID. A different link during a join returns `BUSY`.
- `live.join.status` takes no parameters. Its `join` field is null until the
- first request. It then contains `id`, `server`, `session` and `phase`.
- `queued` means the game has yet to claim the request. `signing_in` waits
- for the account picker, `resolving` checks admission, `confirm` includes
- the host's gamertag when known, and `joining` waits for the title to connect.
- The last outcome stays available as `joined`, `cancelled` or `failed`
- with a human-readable `reason` on failure. Use `ui.press` to operate the
- result A)` once, from `game::agent_glue::matchmaking::result_reason`:
- The newest host perf line (`boot::host_perf`): `{line, age_ms}`;
- `UNAVAILABLE` until one exists (`H2_HOST_PERF=1`; one every 100 frames or
- 5 s). `h2 agent hostperf`.
- `h2 agent hostmetrics`; no parameters. It reports the latest complete
- window of 30 intervals between returns from `GAME_TICK` (`0x00137fe0`).
- answers `UNAVAILABLE` ([`crates/extensions/game/src/host_metrics.rs`](../../../crates/extensions/game/src/host_metrics.rs)).
| `ticks` | Tick intervals in the sample (30) |
| `game_cpu_ms_per_tick` | CPU time on the game thread per tick interval, including work between ticks |
| `process_cpu_ms_per_tick` | CPU time on all process threads per tick interval |
| `wall_ms_per_tick` | Elapsed wall time per tick interval, including pacing sleeps and any pause or load crossed by the window |
| `wall_ms_per_tick_p99` | On a light host, nearest-rank p99 of individual multiplayer tick intervals ending in the last 30 s. The first tick on entering gameplay starts the clock. An empty window gives `null`. |
| `game_cpu_ms_per_tick_p99` | On a light host, nearest-rank p99 of per-tick game-thread CPU deltas (the thread clock's change between consecutive ticks) over the same 30 s window; it clears when gameplay ends, so each match starts empty. `null` when the window is empty or the platform has no thread clock. |
| `late_ticks` | Number of intervals in that same window longer than 36 ms. Both interval fields are `null` outside the light profile. |
| `cpu_percent` | Whole-process CPU over the wall interval; 100 is one fully used core |
| `cpu_seconds` | User and system CPU seconds consumed by this game process through the request |
| `package_idle_wakeups`, `interrupt_wakeups` | Process-lifetime counts from macOS `proc_pid_rusage` (`ri_pkg_idle_wkups` and `ri_interrupt_wkups`). Each successful request samples both counters together. |
| `wakeups_at_ns` | Monotonic sample time in nanoseconds, or `null` when wakeup counters are unavailable. For two readings from the same process, divide the change in each count by the change in `wakeups_at_ns` and multiply by 1e9 for wakeups per second. |
| `rss_bytes` | Resident bytes at the end of the interval on macOS and Linux, including resident shared pages |
| `pss_bytes`, `peak_pss_bytes` | Current and highest sampled Linux proportional resident bytes. Each `host.metrics` request samples the process. |
| `private_bytes`, `shared_bytes` | Private and shared resident mappings in Linux smaps, each including clean and dirty pages |
| `private_dirty_bytes`, `shared_clean_bytes` | Linux smaps Private_Dirty and Shared_Clean in bytes, for comparing copy-on-write and file sharing |
| `age_ms` | Time since the 30-tick CPU sample ended |
| `host_light` | Whether the process uses the light host profile |
- `h2 agent hostmemory` reads the process watchdog. `h2 agent hostmemory report`
- sends `{"report":true}`, collects a diagnostic report on the game host, and
| `footprint_bytes`, `peak_bytes` | Latest physical footprint and highest footprint sampled by the watchdog, in bytes |
| `soft`, `hard` | Process limits in bytes. A zero disables that limit. |
| `growth_bytes_per_min` | Estimated footprint change per minute |
| `samples` | Last 60 samples, oldest first, as `[milliseconds_since_watchdog_start, bytes]` |
| `native.map_retired_payloads` | Detached native payloads queued for or being destroyed by the retirement worker |
| `native.decoded_image_bytes` | Capacity of shared native RGBA, mip and BC payloads, counted once per allocation |
| `native.light_model_entries` | Models retained for standalone light and shadow passes |
| `native.world_cpu_geometry_bytes`, `native.object_geometry_buffer_bytes` | Retained world vertex/index vector capacity and live object vertex/index buffer size |
| `native.world_buffer_cache_bytes`, `native.world_section_payload_bytes`, `native.transparent_packed_bytes` | Indexed draw caches and packed transparent geometry |
| `native.world_section_windows`, `native.transparent_packed_bindings` | Live indexed windows and packed binding entries |
| `gfx.pipeline_cache_entries`, `gfx.shader_cache_entries`, `gfx.texture_group_cache_entries` | Entries across live renderers or binding sets |
| `gfx.pipeline_draw_jobs`, `gfx.pipeline_prewarm_jobs` | Queued compile slots, including cancelled slots awaiting removal |
| `d3d.nv2a_program_entries`, `d3d.nv2a_archive_pending_keys`, `d3d.nv2a_archive_pipeline_records` | Translated program results, unregistered archive keys and archive pipeline records |
| `gfx.upload_ring_staging_bytes`, `gfx.upload_ring_buffer_bytes` | CPU capacity and GPU buffer size for live upload rings |
| `gfx.upload_idle_cpu_bytes`, `gfx.upload_idle_gpu_bytes` | The idle subset of the corresponding upload ring totals |
| `assets.rewritten_map_cache_bytes`, `assets.rewritten_shared_map_cache_bytes` | Rewritten level and shared resource payloads in the VFS cache |
- Every watched process (`h2 play`, `run`, `serve`, `h2 lab host` and
- (`crates/platform/mem-watch/src/recorder.rs`):
| `host.memory.history` | `{"since_ms"?}` | `{"records":[row],"next_since_ms"}`: up to 24 rows or 512 KiB from `since_ms`; call again with `next_since_ms` until it is `null` |
| `host.memory.heap` | `{"top"?: 1..128, default 20}` | `{"sample_interval_bytes","dropped_samples","tags":{name:{"live_bytes","live_allocations"}},"stacks":[{"live_bytes","live_allocations","frames":[{"ip","symbol"}]}]}`; `UNAVAILABLE` unless `H2_MEM_SAMPLE` was set at start |
| `host.memory.gpu` | `{}` | the census, `{name: value}`; names ending `_bytes` are bytes, the rest counts |
| `host.memory.diff` | `{"from_ms"}` | `{"from_ms","to_ms","footprint_delta","growers":[change]}` from the row at or before `from_ms` to the newest; `UNAVAILABLE` outside the retained history |
| `host.memory.postmortem` | `{"pid"?}` | the newest dead process's evidence in this process's report directory: `{"pid","file","diff","reports","report"}` |
- A `change` is `{"category","name","unit","before","after","delta"}`.
- `category` is `tag`, `stack`, `gpu` or `probe`; `unit` is `bytes` or
- `count`. Growers are ranked by `delta` within each unit, bytes first.
- Set `H2_HOST_AUDIT=/path/to/audit.json` before boot to register this
- method (`crates/extensions/game/src/host_audit.rs`). `h2 agent hostaudit` takes no
- changes dispatch and memory-watch costs, so use `host.metrics` for sizing.
- A System Link light host started with `H2_HOST_LOBBY=1` registers these
- four game methods (`crates/extensions/game/src/host_lobby.rs`, types
- `crates/extensions/agent-proto/src/host_lobby.rs`). Each result also carries `at`.
| `host.lobby.open` | `{"map":"<short name>","variant":"<name>","max_players":8,"live":false}` | System Link: an empty object. Live: the advertised `session_id` |
| `host.lobby.state` | `{}` | `{"phase":"lobby","players":[{"slot":3,"peer":1,"name":"<name>"}],"ready":true}` |
| `host.lobby.start` | `{}` | an empty object when retail accepts the start request; observe `state` for load and play |
| `host.lobby.close` | `{}` | an empty object once the network lobby has closed and the host is back at the menu |
- The host looks up `map` by short name, scenario path or English display
- name. The optional `variant` defaults to Slayer; it accepts a disc
- `INVALID_PARAMS`. `max_players` is an integer from 1 to 16. The required
- `live` boolean selects System Link or a Live custom game; Live needs a
- Set `"synchronous":true` with `"live":false` and omit `variant` for a
- The phases are `menu`, `lobby`, `loading`, `playing` and `postgame`.
- reports `"players":[]` and `"ready":false`. Readiness needs a remote
- mutations respect `session.claim`, and a concurrent mutation is `BUSY`.
- `ui.screen` needs `H2_UI_WATCH=1`. Other UI methods read guest widgets
- `ui.press`.
- `ui.screen`** (`h2 agent ui`): the menu screen player one's input goes
- (`game::ui_watch`): `{screen, window, slot, class?, live_menu_row, mode,
- dialog, age_ms, layout_findings, content_findings}`, or `UNAVAILABLE` before the first read. `class` names
- `ui.stack`** (`h2 agent ui.stack`, no parameters): a tick-boundary
- (`history`, nearest parent first). Each screen has `id`, `instance`
- (a guest address valid for that screen's lifetime), and `parent` where
- distinct `mode` values (`folders`, `maps`, `pack`), with the open
- category or pack named in `folder`. A lobby variant list shows Change Game Type as its parent, including that selector's Game Setup parent when present. A pack's parent is the Mods list. All stages show the active picker row in `focused_row`
- A custom variant list identifies its group in `variant_category` and a kit
- screen identifies its class in `class`. Both fields are null elsewhere.
- `input` uses the same settled player-one target as `ui.audit`. `back` gives the instance that closes and its parent where
- known, with action `close`, `reopen` (recreate a parent), or `wait`
- value picker's `close` target identifies the settled screen underneath it.
- `ui.stack` is available without UI watch.
- `ui.campaign`** (`h2 agent ui.campaign`, no parameters): a tick-boundary
- watch (`crates/extensions/game/src/campaign_menu.rs`). With no installed Halo 1
- campaign it returns `{"installed":false}`. With one it reports `page`
- (`picker`, `options`, `levels`, `difficulty`, `dialog` or `other`),
- `game` (the selected campaign key on the three game-specific pages),
- `rows`, `focused_row` (zero-based) and `focused`; unknown selection or
- With a focused picker row, `card.description` is the UTF-8 text in
- `card.picture_datum` is the selected bitmap datum, or null when the
- artwork area is empty. `card` is null on other pages or without a
- focused row. On Campaign Options the action keys are `resume`, `new_game`,
- `ui.browser.filters`** (`h2 agent ui.browser.filters`, no parameters):
- is closed. The result is `{"filters":{...}}` with the same field names
- `{"filters":{}}`. After opening Game Filters with Y and choosing a row
- `ui.lobby_options`** (`h2 agent ui.lobby_options`, no parameters):
- `{"lines":[...]}` with the active custom game panel's visible Quick
- widgets at a game boundary (`crates/extensions/game/src/lobby_options.rs`).
- `host.peer_rtts`** (`h2 agent host.peer_rtts '{}'`) reads the active simulation
- `players` with the same slot, peer, name, bars and nullable RTT as
- `ui.lobby.players`. Local players, phantoms and unsampled routes have
- Compare a remote client's `rtt_ms` to its matchmaking assignment's regional
- `ui.lobby.players`** (`h2 agent ui.lobby.players`, no parameters):
- (`crates/extensions/game/src/lobby_quality.rs`). Each entry has its
- membership `slot`, console `peer`, `name`, `bars` (1 to 6) and `rtt_ms`
- For example, `{"players":[{"slot":1,"peer":1,"name":"Clanker","bars":6,"rtt_ms":null}]}`.
- `ui.lobby.tabs`** (`h2 agent ui.lobby.tabs`, no parameters): the
- read on a game boundary (`crates/extensions/game/src/lobby_tabs/header.rs`).
- The result is `{place, leader, selected, header, tabs, clanker, notice}`:
- `place` is `split_screen`, `system_link` or `xbox_live`; `leader` is
- its X is retail's); `selected` is the mode the lobby is in, and
- `header` the text the lobby's update gives its header (`CUSTOM GAMES`,
- `FORGE`, ...). `tabs` lists the modes present, each `{tab, label,
- state, reason}` with `tab` one of `multiplayer`, `custom_games`,
- `forge`, `co_op`, `state` one of `selected`, `on`, `off`,
- and `reason` the status-line sentence of a mode that cannot run here
- (else null). `clanker` is, in MULTIPLAYER, whether the lobby's
- another mode). `notice` is the reason the status line shows after a
- `{"tab":"multiplayer","label":"MULTIPLAYER","state":"off","reason":"Matchmaking needs Xbox Live."}`.
- share one list-screen implementation (`crates/extensions/game/src/list_class.rs`).
- `ui.audit {}` inspects the instantiated widgets at one game boundary.
- `screen` is the underlying menu. `dialogs` contains the active dialog
- controller. `active` is the surface that accepts player one's input.
- `target` is its opaque token, and `focused_row` is the zero-based focused
- has a numeric `id`, `window`, controller `slot`, current `pane` and
- `ui.press` accepts `{target, action, row?, phase?}`. Copy the token from
- the latest audit. Actions are `up`, `down`, `left`, `right`, `choose` (A),
- `back` (B), `friends` (Y), `open_live_menu` (A on the Live players
- screen in mode 3), and `change_mode` (X in the pregame lobby: the GAME
- picker is then a kit dialog in `ui.audit`, and `ui.lobby.tabs` reads
- the mode a pick switches to). Move focus, audit again, then send `choose` with the
- `ui.audit` after the menu transition settles to check what the press opened.
- `session.methods` lists `ui.press` when the process has an XAPI input tap.
- Clanker at the main menu. Set `H2_AGENT` to its socket path.
- Omit `--socket` to use the normal agent endpoint. The crawl claims one
- `h2 agent ui.attract` takes no parameters. Boot registers it when the Bink
- movie flags at a game boundary (`timer`). The `state` says whether the
- `asset` reports preparation or unavailability and `blocked_reason` gives
- `last_attempt` gives the UI time, idle time and present at the deadline.
- Its `outcome` says whether the wrapper started playback, the guard skipped
- `returned_ui_ms` is filled when the wrapper closes the attract movie after
- `h2 agent audio.listeners` takes no parameters. At a game boundary it reads
- the guest's active world-sound listeners. Each row in `listeners` reports
- the local `user` index, its salted `player_datum` (null for an unmapped
- slot), whether the mapped player is a `phantom` (null without a resolved
- player), and the listener's world-space `position` as `[x, y, z]`.
- `h2 agent audio.voices` takes no parameters and is registered with
- DirectSound (`crates/extensions/game/src/dsound_glue.rs`). The result has
- `distance_reverb` (host enhancement enabled), `reverb_active` (audible
- environment configured), and `voices`. Each playing buffer, queued
- unpaused stream and playing mix-in has a mixer `id`, `kind`, `volume_mb`
- and `distance_units` (listener-relative world units, null for an
- unpositioned voice). `sends_to` lists the ids of the playing mix-ins the
- submix's `input_bin` is a host bin (`0x100` and up), which only its own
- streams carry. The guest sees bin `0x1f` in their place
- `h2 agent audio.loops` takes no parameters. With DirectSound installed it
- Each row in `loops` gives the salted `loop_datum`, `tag_datum`, resolved
- `tag_name` when the map names the tag, zero-based `track`, `segment`,
- `sound_instance` and `sound_datum`. `segment` is `in`, `loop`, `out`, `alt`,
- `alt_in`, `alt_transition_out`, `alt_out` or `unknown`. Overlapping instances
- of a track get separate rows. `pitch_range`, `permutation` and `chunk` come
- from the engine's source state. `play_packet` and `play_position_frames`
- `crates/extensions/game/src/mixer_meter.rs`; types in
- `crates/extensions/agent-proto/src/audio.rs`.
| `{window_ms?}`: 50 to 5000, default 1000 | `{window_ms, frames, rate, channels, peak_dbfs, rms_dbfs, lufs, max_momentary_lufs, clipped}` |
| `{play}`: a host clip | the same, with `play: {clip, duration_ms}` |
- back (`audio.voices`, `audio.meter`) without the game having a reason to
- (`crates/extensions/agent/src/sim/mod.rs`, `Sim::audio_play`; types in
- `crates/extensions/agent-proto/src/sim/mod.rs`).
| `{sound, player?}`: a `snd!` tag by datum or name (as `edit.tags` lists them); `player.<n>` or `sim.<n>`, default `player.0` | `{player, sound: {tag, group, name}, instance}` |
- The call.** The game's own `sound_start_at_marker`
- (`SOUND_START_AT_MARKER`, `0x1891d0`) with the unit's head marker, the
- session may ask. `instance` is the sound instance the game made.
- Errors.** A tag that is not a `snd!` or is not found is
- of the sim provider (`crates/extensions/agent/src/sim/mod.rs`,
- `Sim::object_animate`; the write in `crates/extensions/agent/src/probe/animation.rs`,
- `start`; types in `crates/extensions/agent-proto/src/sim/mod.rs`).
| `{id, animation}`: an object id as `world.object` spells it (not a player); the animation's name in the graph the object's manager holds | `{object, graph, animation, index, frames, frame, session}` |
- unit, as the script call `recording_play` does: the unit's control
- method of the sim provider (`crates/extensions/agent/src/sim/mod.rs`,
- `Sim::recording_play`, the block read in `sim/recording.rs`; types in
- `crates/extensions/agent-proto/src/sim/mod.rs`).
| `{recording, player?, mode?}`: a name in the scenario's recorded animations (without case); `player.<n>` or `sim.<n>`, default `player.0`; `play` (default), `delete` or `hover` | `{player, recording, index, unit}` |
- Three game methods with an empty parameter object (`crates/extensions/agent/src/hs_state.rs`)
- `hs.threads` returns a `threads` array of live slots. Each row has
- `slot`, numeric thread `category` and signed `wake` tick. Scenario script
- threads also have their definition's `script` name and `kind`; both are
- `hs.globals` returns a `globals` array in scenario declaration order.
- Each row has `name`, `type`, `type_index` and `value.bits`; scalar types
- also have `boolean`, `real` or signed `number`. Scenario declarations
- `hs.player_actions` returns three unsigned masks from local player
- control. `latched` records inputs seen since the script's last
- action-test reset; `capture` identifies inputs claimed by script tests;
- `until_release` retains suppression through button-up after a reset. Bit
- `h2 agent hs.parity` takes no parameters. It returns the switch name
- (`impl.sim.native.hs`) and a `checks` object keyed by VM operation.
- Each entry counts agreements (`matched`), disagreements (`mismatched`)
- and observations the Rust arm could not compare (`declined`).
- `first` holds the first mismatch description or `null`. The counters
- expression (a `(` it never closes is named at its own place). An unknown
- from a person. The agent's tap is an `xapi::tap::InputTap` (the layer
- `H2_INPUT_RECORD` and `H2_INPUT_REPLAY` use, `crates/extensions/agent/src/tap.rs`
- over `input::backend::remote`'s step queue): it sees each
- `XInputGetState` result just before the guest gets it. Every host that
- `input.pad {player, at_tick, steps, append}`.** Each step is
- `{buttons, left, right, lt, rt, ms, ticks, reads}`. `player` is the
- step holds for `ms` of wall time, `ticks` of game time (the stamp's
- `tick`, the same on every run), or both, and until the game has read the
- pad `reads` times (default once), so a short press is never lost to a
- slow frame; with neither `ms` nor `ticks` it holds until replaced.
- `at_tick` (a tick, or `"now"`, the default) holds the first step back
- until the game's tick reaches it. `append` adds to the queue, otherwise
- (`input::backend::fallback::merge`): buttons are or'd, a stick or trigger
- the step moves wins. Buttons are `a b x y black white lt rt` (analog,
- fully pressed), `up down left right start back ls rs`; sticks `[x, y]`
- `input.release {player}`** empties one port's queue, or all of them.
- `input.state`** gives `pad` (player one), `pads` (every port: `active`,
- `queued`, `done`, what is held), `open` ports, `tick`, and the recording
- `input.record {name, replace}`** logs every read (status, handle, the
- 22 bytes, the game tick) to `inputs/<name>.input` under the
- [agent output root](../switches.md#tools). This is the `xapi::tap` text format
- that `H2_INPUT_RECORD` writes. The log is saved at `input.stop` or when
- the game exits. Names use 1 to 64 letters, digits, `.`, `_` or `-`
- and may not start with a dot. An existing log is kept unless `replace`
- `input.list`** returns `{names}`, the sorted names of saved input logs
- without their `.input` suffix.
- `input.log.read {name, offset?, limit?}`** returns `{text, next}` from
- a saved log. Use the same name as for `input.record`. The first offset
- defaults to 0. Pass `next` as the offset for the next page and stop at
- `input.replay {name}`** answers reads from a log read for read (keyed
- by read index, as `H2_INPUT_REPLAY`). `input.state` and `input.stop` give
- its counts: `reads` in the log, `replayed`, `matched` (same handle as
- recorded), `handle_mismatches` (the runs have parted) and `past_end`
- `input.route {player?, legs, timeout_ticks?}` starts a sequence on a local
- through the same tap as `input.pad`. It uses the default stick and button
- active BSP's bot nav mesh from the game's `bots` data directory, even
- `h2 bots nav <map>`. See [Clankers' navigation](../../game/bots.md#32-navigation).
- `BUSY`. `input.pad` also returns `BUSY` while the port runs a route.
| `{"move_to":[x,y,z],"crouch":false,"tolerance":0.1,"speed":1}` | Follow the bot nav mesh with the left stick and turn with the right stick. Stop within `tolerance` of the goal (default 0.1 map units). `speed` limits stick magnitude from 0 to 1 (default 1). `crouch: false` keeps the crouch button released. |
| `{"face":yaw_deg}` or `{"face":[x,y]}` | Turn in place to a world yaw in degrees from +x toward +y, or toward a world point. |
| `{"jump":{"from":[x,y,z],"land":[x,y,z]}}` | Use the bot jump arc to plan a run-up, press A at takeoff and steer toward the landing. The leg succeeds within 0.15 map units of `land` and reports the observed points and speeds. |
| `{"crouch_into":[x,y,z],"hold_ticks":N,"exit_face":yaw_deg}` | Hold the left-stick click and creep toward the point until movement stalls. Release the click, keeping pressure into the ceiling for `hold_ticks`, then turn toward `exit_face` with the stick forward until the player stands. |
| `{"wait":ticks}` | Leave the pad idle for the given number of game ticks. |
- `input.route.status {player?}` returns `player`, `state` (`idle`,
- `running`, `completed`, `failed` or `stopped`), elapsed `ticks`, the
- current zero-based `leg` (or null) and `legs`. Each leg reports
- `{ok, ticks, reason}`. `ok` is null while the leg is pending or running.
- After a failure or stop, legs the route never started get `ok: false`
- and `reason: "not_run"`. A jump leg includes its available `takeoff`,
- `apex` and `landing` samples in `jump`, each with `position`, `velocity`
- and `speed` in map units per second. The landing position comes from
- Use `input.route.stop {player?}` to stop the route and release its port.
- `input.release` also stops routes on the ports it releases. The final
- status stays available until a new route starts on that port. The `route`
- `input.host` (`h2 agent hostpads`, read-only, no params) shows the
- answers from the last input sync (`syncs` counts them), which runs
- input (`crates/extensions/game/src/debug_menu_glue.rs`, `crates/extensions/agent/src/hostpads.rs`).
- work without a controller connected. `input.lightbar` is read-only and
- takes no params. It returns `enabled` and four `ports`, each with a
- zero-based `port`, the last sampled `game_rgb`, the temporary
- `override_rgb`, and the desired `rgb`. Each colour is an array of three
- `rgb` is null and the game gives up control of the light.
- `input.lightbar.set {port, rgb}` sets an override for one port and returns
- the same state. A null `rgb` clears the override. The override lasts until
- `crates/extensions/agent/src/lightbar.rs`.
- `state.save`, `load`, `list`, `drop` and `fork` (types in
- `crates/extensions/agent-proto/src/state.rs`, the executor's part in
- `crates/extensions/agent/src/exec/snap.rs`, the machine in
- `crates/extensions/game/src/snapshot_glue.rs`). A snapshot is the whole machine at one
| `state.save` | `{name, replace?, cow?, max_ticks?}` | guest RAM, the kernel and its threads, D3D, audio, XAPI with the input cursor, every feature's section, and a RAM hash, at the first quiescent slice boundary after a tick. `cow: true` also writes a sparse raw RAM image `<name>.ram` beside it |
| `state.load` | `{name, cow?, max_ticks?}` | back to exactly that tick; the RAM hash (`lockstep::hash_guest`) is checked against the saved one, naming the MiBs that differ. `cow: true` maps that RAM image private copy-on-write in place of decoding RAM in; it is refused, naming the file, when the save wrote no image |
| `state.list`, `state.drop {name}` | | this build's snapshots, answered at once |
| `state.fork` | `{from, name, replace?}` | load `from` and save it again as `name` |
- Names are `a-z 0-9 . _ -`, 1 to 64, not starting with a dot. A save or
- live, for `max_ticks` (default 90, at most 1800), then answers `BUSY` with
- the blockers in `data.blockers`. One sent while the game is paused runs at
- Calls other than `state.list` need control when another connection holds
- it. A snapshot is refused while `agent-calls` writes are queued.
- (`crates/extensions/agent/src/hot.rs`, names in `crates/extensions/agent-proto/src/hot.rs`).
- is `$H2_RENDER_OVERRIDES`, else `render-overrides` in the cache directory.
| `edit.map` | read | `{known_epoch?}` | `{at,name,file,checksum,scenario,epoch,session,bsp}` for the loaded map. File and checksum can be null. `bsp` is the loaded structure BSP's index in the scenario's block (`-1` before one loads, null when unreadable): the one BSP whose region is in guest memory. Session includes `writes` and the refusal reason in `why`. A multiplayer host with an unknown peer count reports `writes:false` and `why:"multiplayer host peer count is unknown"`. |
| `edit.tags` | read | `{group?, name?, limit?}` | the loaded map's tags: `tag`, `group`, `name`, `address`, `size`, `defined` |
| `edit.get` | read | `{tag, group?, path?, epoch?}` | a field by path (`postprocess[0].colors[1].color`), or an element's fields; `tag` is a datum or a name |
| `edit.read` | read | `{epoch,at,len}` | Reads `len` bytes (1–524288) at numeric address `at` inside one loaded tag. Returns `{at,holds_bytes}` with lowercase hex bytes. |
| `edit.batch` | act | `{writes,label?,epoch?,confirm_peers?}` | `{id,applied,changes}` at one boundary. Each batch has one journal entry with its ID, optional label and changed `fields`. |
| `edit.set` | act | `{tag, group?, path, value, epoch?, confirm_peers?}` | writes the field now, journalled; returns the change |
| `edit.changes` | read | none | the current epoch’s journal, including original `was_bytes`, `new_bytes` and current `holds` as hex |
| `edit.revert` | act | `{id?,epoch?,confirm_peers?}` | restores a change or batch, or the whole journal in reverse order. `reverted[]` gives `id,at,tag,path,value,holds_bytes` for each restored field. `dropped[]` gives `id,at,tag,path,why,holds_bytes` for each field left alone. Addresses are numeric. Observed bytes are hex, or null when the field belongs to another map. |
| `render.overrides` | read | none | the override directory, what is loaded and how each fared |
| `render.reload` | act | `{dir?}` | reads the directory again (`dir` moves it) |
| `render.programs` | read | `{vertex?, limit?}` | the translated programs, by id |
| `render.dump` | act | `{program?, to?}` | writes translated WGSL, the start of an override |
| `render.param` | act | none, `{name}`, `{name, value}` | lists, reads, sets (`value: null` resets) a parameter |
| `render.toggle` | act | none, `{name, on}` | lists, sets (`on: null` resets) a toggle |
- count. A client can pass `known_epoch:null` to `edit.map` to include the loaded
- tag datums in `loaded`, then send its cached epoch to receive the table only
- after a cut. An epoch pins a write or field read to the map reported by `edit.map`.
- the game holds them and appear in `dropped`.
- Each batch write takes either `{tag,group?,path,value,was?}` with typed values,
- or `{at,value,was?}` with hex bytes. An address is a u32 or a `0x` string.
- If it holds neither value, error 8 (`MISMATCH`) returns `data.fields` with
- `at`, `tag`, `path`, `was`, `holds` and `holds_bytes`. The entire batch and
- Tag names come from the map file the game loaded ([`world`](../agent-api.md#world-what-is-in-the-level)).
- are in `crates/extensions/agent/src/hot.rs`, the editor half in
- `crates/extensions/hot-tags/src/raw.rs`, the overlay in
- `crates/runtime/vfs/src/map_override/overlay.rs` and the game's glue in
- `crates/extensions/game/src/hot_glue.rs`.
| `edit.batch` | act | adds `raw: [{owner, offset, bytes, was?}]` and `evict: [owner]`; `writes` may be left out when either is given | adds `raw: [{owner, offset, resident, refresh}]` and `evicted: [owner]` |
| `edit.raw.append` | act | `{file?, bytes, align?, epoch?, confirm_peers?}` | `{id, pointer, len}` |
| `edit.stage.put` | act | `{stage?, bytes}` | `{stage, len}` |
| `edit.overlay` | read | none | `{entries, bytes, appended, appended_bytes, metadata, metadata_bytes, files, owners}`: raw runs, appended extents and metadata runs (copies of tag writes; docs/editor/realtime-edits.md, "The edit overlay") |
- `owner` is `{section: {tag, kind, index}}`, with `kind` one of `cluster`,
- `instance_def` and `mode`, `{pixels: {tag, image, lod}}`, with `lod`
- 0 to 2, `{sound: {tag, range, permutation, chunk}}`, one sample chunk
- of a `snd!` permutation, each index counted within the sound
- (docs/editor/realtime-edits.md, "Sound samples"), or `{bsp: {index}}`, the
- scenario's structure BSP `index`. `tag` is a datum or a name. `offset`
- descriptor), and the bytes must fit inside it. A `bsp` owner is refused
- while it is the loaded BSP (`edit.map`'s `bsp`): its region is guest
- memory then, written with `at`. `bytes`
- is base64, or `{stage, at?, len?}` naming bytes `edit.stage.put` holds.
- `was` is base64 bytes or `{hash}`, their FNV-1a 64 as a number or a hex
- `edit.raw.append` reserves an extent past the end of `file` (`local`,
- `mainmenu`, `shared` or `single_player_shared`; `local` by default),
- aligned to `align` (a power of two, 16 by default). It is invisible until
- a tag points at it: `pointer` is the raw pointer to write into a block
- `edit.changes` lists raw acts under `raw`, by journal id. `edit.revert`
- and appended extents dropped. Reverted raw entries appear in `reverted[]`
- with `raw`, `evict` or `append`.
- A stage belongs to the connection that made it. `stage` in
- `edit.stage.put` appends to an existing one. A request that names a stage
- A tag block is a `(count, address)` pair and a run of elements with
- allocator is `crates/extensions/hot-tags/src/arena.rs`, the batch half
- `crates/extensions/hot-tags/src/batch.rs`, the methods
- `crates/extensions/agent/src/hot.rs` and the pages
- `crates/extensions/game/src/hot_glue.rs`.
| `edit.batch` | act | adds `moves: [{root, stride, count, capacity?, bytes, was}]`; `writes` may be left out when it is given | adds `moves: [{root, tag, path, count, address, capacity}]` |
| `edit.arena` | read | none | `{base, len, mapped, used, largest_free, physical_free, physical_floor, moves: [{tag, name, root, address, capacity, stride, len}]}` |
- `root` names the pair: `{tag, group?, path}` with a path that ends at a
- block field (`postprocess[0].colors`) or at an 8-byte raw field
- (`+0x20:u32[2]`), or `{at}`, the pair's address, inside a loaded tag or
- inside a block an earlier move put in the arena. `stride` is the bytes of
- one element and must equal a defined block's element size. `bytes` holds
- the `count` new elements (base64 or `{stage, at?, len?}`), `capacity`
- (default `count`) the elements of room. `was` is `{count, address}`, the
- pair before the move; `address` is a number or a `0x` string.
- holds another pair than `was` answers error 8 (`MISMATCH`) with
- `data.moves: [{root, tag, path, was, holds}]`. A write over a move's root
- `data.why`, `data.root`, `data.need` (bytes) and `data.largest_free`, and
- block's bytes go into the extent, the room past `count` is zeroed, and
- in its `bytes` or in a later batch.
- A moved block belongs to its tag: an `{at}` write and `edit.read` inside
- writes it. `edit.changes` lists moves under `moves`. `edit.revert` puts
- A field write inside a block that has moved back appears in `dropped`.
- `edit.arena` reports the window (`base`, `len`, 4080 KiB), the bytes
- (`physical_floor`, 2 MiB). `world.placement.edit` grows placement blocks
- `(count, address)` pairs, so a live collision edit stages the new blocks
- the methods are `crates/extensions/agent/src/hot/collision.rs`.
| `edit.collision.stage` | act | `{target, blocks: {name: {count, bytes}}, leaves?: {count, bytes}, mopp?, epoch, confirm_peers?}` | `{stage, bytes, blocks: {name: {count, address}}, arena: {used, largest_free}, activation}` |
| `edit.collision.commit` | act | `{stage, mode: "append" \| "rebuild", activate?, was: {name: {count, address}}, epoch, confirm_peers?}` | `{at, stage, mode, pairs: {name: {count, address}}, activated, freed: [address]}` |
- `target` is `{structure: tag}` (the structure's level collision,
- `sbsp+0x14`), `{instance_def: {tag, index}}` (an instanced geometry
- definition's BSP, at the definition's `+0x70`) or `{coll: {tag, region,
- permutation, bsp?}}` (`bsp` defaults to 0); `tag` is a datum or a name.
- `blocks` holds all eight blocks by the field names of
- `tags::collision::Bsp` (`nodes`, `planes`, `leaves`, `refs`, `nodes_2d`,
- `surfaces`, `edges`, `vertices`), each `count` elements laid out as
- `Bsp::encode_blocks` lays them (`collision_build::coll::stage_blocks`).
- For a structure, `leaves` is the per-leaf block (`sbsp+0x2c`, 8 bytes a
- leaf, one per BSP leaf) and `mopp` the bytes of the `sbsp+0x1e0` block;
- base64 or `{stage, at?, len?}` naming `edit.stage.put` bytes. A stage of
- `pathfinding` is refused: its blocks hold pointers the host would have to
- The stage decodes the blocks and runs `Bsp::validate` against the target's
- full arena answers `REFUSED` (11) with `data.block`, `data.need` and
- `data.largest_free`, and holds nothing. A second stage of the same target
- before a commit replaces the first and frees its extents. `activation`
- The commit names in `was` every pair it repoints: the eight block names,
- `leaf_clusters` for the per-leaf block and `mopp`, each `{count,
- address}` before the commit (`address` a number or a `0x` string). A pair
- that holds another answers error 8 (`MISMATCH`) with `data.pairs: [{name,
- the first write, and all are written at the same boundary. `append` is
- elements than the one it replaces is refused. `rebuild` changes indices:
- With `activate: true`, and always for `rebuild`, the commit asks the game
- with `activated: true`. The activation is the second half of a BSP
- `crates/extensions/debug-menu/src/sources/mod.rs` (`method`).
| `settings.list` | read | `{prefix?}` | settings (`settings.toml`) and how each applies (`crates/extensions/agent/src/menu.rs`) |
| `settings.set` | act | `{key, value}` | saves the value and applies it as the setting applies; `UNAVAILABLE` when something else set it for this run |
| `perf.overlay` | act | `{}` reads, `{on}` sets | the perf overlay (the watermark) |
| `cheat.list` | read | `{}` | `{refused, pending, cheats}`: each cheat's `name`, `label`, `about`, `kind` (`bool`, or `choice` with `choices`), `value`, `refused` (why the session refuses it) and `pending` (why it does nothing yet: on while no campaign or multiplayer game runs; it takes hold when one starts); the top-level `pending` says the same with no game running (`crates/extensions/game/src/cheats.rs`) |
| `cheat.set` | act | `{name, value}` | sets one, for example `{"name":"hide_hud","value":true}` or `{"name":"game_speed","value":0.1}`; answers with `cheat.list`'s answer. Cheats: `god`, `infinite_ammo`, `bottomless_clip`, `game_speed` (0.1, 0.25, 0.5, 1, 1.5, 2, 4), `fly`, `noclip`, `hide_hud`, `ai_off` ([debug-menu.md](../debug-menu.md#cheats)) |
| `debug.views` | read | none or `{}` | `{views, range, trace_ticks}` |
| `debug.view` | act | one of `{name, on}`, `{range}` or `{trace_ticks}` | the updated listing |
- Each row has `name`, `label`, `on` and an optional `why`. The names are
- `geometry_edges`, `trigger_volumes`, `kill_volumes`, `invisible_barriers`,
- `collision_geometry`, `hitboxes` and `traces`. For example,
- `h2 agent debug.view '{"name":"hitboxes","on":true}'` enables the same
- or “Editor · edits blocked”. Optional `text` appears on the second line.
- (`crates/apps/cli/src/play_cmd.rs`, `crates/apps/app/src/shell.rs`).
| `editor.link` | read | `{editor, mode, text?, ttl_ms?}` or `{clear: true}` | `{shown, why?, expires_ms}` |
| `editor.focus` | act | none | `{focused, why?}` |
| `editor.marks` | read | `{marks, ttl_ms?}` | `{held, drawn, why?, refused}` |
- `editor` is a string. `mode` is `view`, `edit` or `blocked`. Each call
- replaces the lease, with a `ttl_ms` of 500 to 10000 (default 3000).
- `text` accepts up to 64 characters. Invalid arguments return
- `INVALID_PARAMS`. `expires_ms` gives the milliseconds remaining when the
- Clearing returns `shown: false`. The link is available in every session
- kind (`crates/extensions/agent-proto/src/editor.rs`,
- `crates/extensions/agent/src/editor.rs`).
- `editor.focus` returns `focused: true` when it queues a request for the
- both methods are listed and answer with `shown: false` or `focused: false`
- and `why: "this host has no window"` (`crates/apps/boot/src/lib.rs`).
- `editor.marks` replaces the mark set for 500 to 10000 ms (default 3000).
- one geometry key: `{object: "<eight hex digits>"}`,
- `{box: {origin, forward, up, size}}` or `{point: [x, y, z]}`. Box size
- An entry takes `color: [r, g, b]` with components from 0 to 1,
- optional `style` (`selected` or `candidate`), optional `label`, and
- optional reserved `xray`. An invalid entry leaves the previous set intact.
- `held` counts accepted entries. Stale object handles and object entries
- `refused: [{index, why}]`. The tick observer retires handles when their
- (`crates/extensions/agent/src/editor/scene.rs`,
- A paired editor's `pair.preview` and `pair.audio` are clients of this API
- `crates/apps/studio/src/api/preview/run.rs`). They add no game method. A
- preview calls `session.methods`, then `sim.give` or `sim.spawn` (or
- `cheat.set hide_hud`), `session.wait`, `world.object`, `world.query`,
- `pogo.state`, `render.scene`, `ui.audit`, `ui.stack`, `ui.screen`,
- `render.capture` or `host.screenshot`, and `audio.voices` and
- `audio.loops`, each where the game lists it. A game that leaves one of
- own rules: `sim.give` and `sim.spawn` are refused in a co-op session and
- window's mean cost (`Δtotal_ns / Δframes`) and cache hit fraction.
- at `x` first with `sim.state.set` (its `pos`), then ask. The reply
- at}`, the last 64) and, for a failed route, `error`: `timeout`, `no_map`,
- `{"spot": n, "kind": "bounce" | "crouch"}` (default `bounce`), by its
- then, map, nav}` (`nav` is `ready` or `loading`). A new `player.goto`
- pitch?, map, slots}`, read at the tick boundary as `world.query` reads
- the player (`forward` is the unit's aiming vector), `slots` every slot
- game's thread with guest memory at hand, and like `world.*` at a tick
- list (default and cap 2048) and `truncated` says more matched. Each object
- value `SCORE_ADD` changes), `null` without a game engine (the menu, a
- positions and vitality, read from `f32`s). One implementation,
- this against a game this console hosts; replace `ROSTER_XUID` and
- `ROSTER_GAMERTAG` with a bot identity issued by the local Live server.
- `h2 agent run`, a failed `sim until` is a failed call, so it stops the
- `lobby mode <campaign|xbox_live|split_screen|system_link>`: from the main
- as `{placement: {block, index}, old, object, linked}`. A placement the
- `[{index, object?, linked, created, why?}]`. `moved` is `{address,
- running now, read at a tick boundary. `crates/extensions/agent/src/rules.rs`
- (`game::rules::feature::AgentSource`, over
- `crates/extensions/agent-proto/src/rules.rs`. The methods are registered in
- builds with the `rules` cargo feature; with the `rules` switch off
| `rules.state` (read) | `{}` or omitted | `{running, refused?, script?, version?, hash?, source?, carrier?, scoring?, options?, authority?, stopped?, budget?, pending, errors, last_reload?, blocked?, labelled?}` |
| `rules.zones` (read) | `{}` or omitted | `{zones: [{id, label, number?, shape: "area" \| "corners" \| "default", area, at, reports}]}` |
| `rules.reload` (act) | `{ir_b64 \| source, mode?: auto \| in_place \| restart}` | `{mode: in_place \| restarted, hash, reasons, tick}` |
- `crates/extensions/game/src/rules/reload.rs`). Give the blob
- (`ir_b64`, the IR's bytes in base64, as Studio sends it) or the
- `crates/extensions/agent/src/variants.rs` checks the params, holds the
- (`game::variant_quick::AgentSource`, over
- `crates/extensions/game/src/variant_quick.rs` and
- `variant_quick/page.rs`).
| `variants.quick` (read) | `{}` or omitted | `{variant, engine, script?: {name, hash}, shown, rows: [{slot, kind: setting \| option, name, label, value, text, line, help?, values: [{value, text}]}]}` |
| `variants.quick.set` (act) | `{slot, value}` | `variants.quick`'s result, read after the change |
- Host methods while sim mode is on (`H2_SIM=<match file>`, `h2 sim run
- The `sim.*` methods above are a different thing: simulated players added to
- an ordinary run. `sim.guest_census` is the one `sim.*` method of a sim
| `match.status` | read | `match` (the file), `map`, `game_type`, `bots`, `phase` (`waiting` until the match's game starts, `binding` until every distributed seat has a player and unit, `playing`, `over`), `lobby` (startup progress, below), `bound_seats` (logical seat to player index for a distributed match), `ticks` (game ticks counted since the start), `max_ticks`, `game_seconds`, `wall_s` (since the boot), `match_s` (since the first tick), `speedup` (game over match wall time), `frames` (`skipped` world frames, `rendered_world` by `render_every`, `other` menu and loading frames, `stub_faults`), `end` (`game_over`, `tick_limit`, `wall_limit`, `stopped`; `null` while it plays), `drops` (a [seam drops](../sim.md#seam-drops) match's `planned` and `done` drops, its `control` (`off`, `running`, `passed`, `failed`) and its `error`; `null` without a `[drops]` table), `bounces` (a [bounce search](../bounce-search.md)'s `planned` and `done` trials, `sites`, `batches`, where it is (`phase`, `batch`, `variant`), its forks (`captures`, `restores`, `waves`, `busy`, and `last_busy`, what the last busy answer named) and its `error`; `null` without a `[bounces]` table) |
| `match.report` | read | the report as it would be written now (`h2-sim-report/1`, sim.md section 6): players, kills so far, a note while the match has not ended (its `result.end` is then `aborted`) |
| `match.stop` | act | end the match: `end` becomes `stopped` (unless it had ended), the boot loop stops at the next slice and the report is written. `{stopping, end, already_over}` |
| `sim.guest_census` | read | no parameters (any other is `INVALID_PARAMS`). `{now, match}`: `now` is the guest census read at this boundary, `{local_users, local_visibility_empty, arrays: {effect, particle_system, particles, decals, lights}}`, each `null` where the read failed (`null` as a whole with no guest); `match` is the report's `guest_census` block so far (`ticks`, `local_users`, `local_visibility`, `arrays` with `samples`, `mean`, `peak`), `null` before the match's first counted tick. sim.md section 6 names every address (`game::sim_census`) |
- `session.pause` and `session.step N` step a match.
- Under `impl.sim.native.anim` the animation graph's node walk `1dd1c0`
- `crates/extensions/agent/src/anim.rs`, wire types in
- `crates/extensions/agent-proto/src/anim.rs`.
| `anim.graph_walk` | read | No parameters. Returns `switch`, `addr` (`001dd1c0`), `arm` and `body`. |
| `anim.manager` | read | No parameters. Returns `switch` and `entries`, keyed by the eight-digit guest address of each observed manager or graph function. |
- `sim.arms` reads every native-sim shadow and authority arm in one call:
- it needs no guest memory. Code: `crates/extensions/agent/src/sim_arms.rs`,
- wire types in `crates/extensions/agent-proto/src/sim_arms.rs`.
| `sim.arms` | read | No parameters. Returns `subsystems`: `physics`, `objects`, `anim`, `collision`, `seats`, `effects`, in that order. |
| `collision.stats` | read | No parameters. Returns `switch`, `entries`, `decodes`, `decode_failures`, `flushes`, `bsps`, `mopps`, `bytes`, `array_bytes`, `budget_bytes`, `ring_fallbacks`, `cluster_budget_reads`, `precision` (aggregate single, reserved, double and extended x87 calls at C and D), `vector` (C’s result parity), `context` (D’s outer walk parity), `instance`, `flood`, `visit`, `query` (E, G, H and J’s return comparisons), and `ray` (I’s walk and callbacks). |
| `collision.trace` | read | `{start, delta, flags}` where `start` and `delta` are three finite binary32 world coordinates and `flags` is an unsigned 32-bit whole-query mask. |
| `collision.sources` | read | No parameters. Checks the source fingerprints of each resident native BSP and MOPP against guest RAM. |
| `collision.sources.clear` | claim | No parameters. Returns the same check and drops all resident entries from the collision world at that tick boundary. |
- (`agent::shadow`, registered by boot's agent glue) over
- `feature::shadow`'s registry. A native sim port is checked against the
| `shadow.enable` | act | `{ports, every?, step_limit?}`: checks on for those ports, their counts and mismatches reset. `every` checks one call in that many (default 1); `step_limit` caps the original's instructions per check (default 5,000,000). The first call builds the lifted original from the image's cached analysis (a few seconds on the game thread). → `{enabled, not_installed, every, step_limit}`. Unknown names: invalid params; no image in the process: unavailable |
| `shadow.disable` | act | `{ports?}`: checks off (no ports: all). → `{disabled}`. Counts and mismatches stay |
| `shadow.status` | read | `{on, ready, every, step_limit, calls, checked, matched, mismatched, skipped, ports, mismatches, mismatches_dropped}`. Each port: `name`, `group`, `point`, `function`, `installed`, `on`, `calls`, `checked`, `matched`, `mismatched`, `skipped`, `skip_reasons`. Each mismatch (at most 64 kept): `port`, `group`, `point`, `function`, `pc` (entry or resume), `tick`, `call` (which call since `enable`), `kind` (`transfer`, `registers`, `memory`, `returned`, `nondeterministic`), `summary`, `transfer` (`port`, `original`), `registers` (`name`, `port`, `original`), `memory` (runs of bytes: `address` folded physical, `virtual_addresses`, `before`, `port`, `original`), `memory_bytes`, `steps`, `entry` (every register at entry, hex) |
- `crates/extensions/game/src/test_maps.rs` (the `test-maps` cargo feature of
- `boot`, which supplies the recipes in `crates/apps/boot/src/test_maps_recipes.rs`); wire types `agent_proto::test_maps`.
- `h2 agent maps` (`crates/apps/cli/src/agent_maps.rs`) reads a recipe file
- (`.toml` or `.json`) on the client and sends it as the recipe object; its
- verbs work in `h2 agent -` and `h2 agent run` scripts.
- changes `H2_OPT`. A client that needs new code writes a test instead.
- feature plan refuses to build; the `agent` feature declares its order
- against `lockstep`, co-op and telemetry.
- directory (`agent_proto::cache_dir`), never in the repository.
- `input.*` method without XAPI input, `audio.voices` without
- DirectSound, `host.lobby.*` without the light host) are absent from
- `crates/extensions/agent/src/exec/tests.rs` (wire, control, wait, deferred answers),
- `crates/extensions/agent/src/sim/tests.rs` (`sim.*`, with the callees played by the
- tests), `crates/extensions/agent/src/trace/tests.rs`, `crates/extensions/agent/src/havok.rs` and
- `crates/extensions/agent/src/raw.rs` (synthetic guest memory), `crates/extensions/agent-proto`
- (wire types), `crates/apps/boot/tests/feature_plan.rs` (the observer order),
- `crates/extensions/agent/src/render/picture/tests.rs` and `exec/picture.rs`
- (`render.picture` over a synthetic frame source).
- `agent-probes`. Their names and wire types are in `agent-proto`'s `probe` module (`crates/extensions/agent-proto/src/probe.rs`); the implementation is `crates/extensions/agent/src/probe/`. Discover them with `session.methods`. The three acting
- are full eight-digit hex datums, as returned by `sim.give`, `sim.spawn` or
- a player's `world.query` row.
- [high-framerate.md](../../game/high-framerate.md): `lockstep` and `xapi::tap`
- [lab.md](../lab.md): `h2 lab`, runs on a second Mac over SSH with replays and frame stats
- [hot-iteration.md](../../graphics/hot-iteration.md): `edit.*` and hot `render.*`
- [docs/live](../../live/): the Live service, the other reader of `live-telemetry`
