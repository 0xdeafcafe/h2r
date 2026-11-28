# Agent API: bots, routes and simulated players

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## Bot combat

`h2 agent bots.stats` reads the host's brain-driven phantoms at a tick
boundary. It takes no parameters. The result contains `at` and a `bots`
array. Each row has the seat's `handle`, `name`, `player` (null until
bound to a game player), `skill`, `profile`, `combat` and `accuracy`.

`maintenance` measures the host's phantom roster work on render frames with
configured seats or pending removals. `frames` counts calls, `total_ns` sums
their wall time, and `max_ns` is the longest call. `unchanged_lobby_frames`
counts frames where the seated custom lobby's peer and player records matched
the previous sample. Subtract two readings in the same session to get a
window's mean cost (`Δtotal_ns / Δframes`) and cache hit fraction.

The write counters show how often the roster work changes the session's
membership, which makes the game rebuild what depends on it.
`write_frames` counts frames that wrote the membership and moved its
counters: a join, a removal, or a repair. One frame's repairs share one
bump. `props_writes` counts phantom peer properties rewritten to follow
this console's. `identity_writes` counts phantom names or looks written
back into the session's player records. `handicap_writes` counts phantom
handicaps put back to none there ([bots.md](../game/bots.md), "A phantom's
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
([bots.md](../game/bots.md), "The facing release"). It rises by about
one per melee lunge. A bot whose aim stops following its right stick
while this stays flat points at a facing write the release does not
cover.

`profile` contains the card's `summary`, its `archetype`, `favorite_weapon`, the name's
`try_hard` score and the `weights` active for that difficulty and style.
The full vector contains `radar_use`, `noob_combo`, `dual_wield`,
`aggression`, `grenades`, `melee`, `weapon_preference`, `range_preference`,
`strafe`, `jump`, `reaction_jitter` and `retreat`. Each value ranges
from zero to one. Higher reaction jitter favours the later part of the
difficulty’s reaction window.

| Field in `combat` | Meaning |
|---|---|
| `tick`, `observation_tick` | Most recent think tick and the world sample the decision used. The sample can be older because perception takes time. |
| `goal`, `target`, `range` | Current activity, visible fight target's player index and distance in world units. The target and range are null outside a visible fight. |
| `weapon`, `weapon_role` | Held weapon's map definition datum and handling class, null before an inventory read. |
| `last_decisions` | Up to eight goal changes and combat input decisions, oldest first, each with `tick` and `action`. |
| `melee_presses`, `switch_presses`, `grenade_requests` | Attack or switch inputs chosen by the brain, counted once per press. |
| `weapon_switches`, `grenades_used` | Observed exchanges of the two carried weapon definitions and decreases in grenade inventory within a life. |
| `last_reaction_ticks`, `total_reaction_ticks`, `reactions` | Ticks from the selected target's first fight observation to the first attack decision on that target. The mean is the total divided by the sample count. |
| `think_ms`, `total_think_ms`, `think_ticks` | Wall time for the latest brain decision and controller update, accumulated time and sample count. Guest reads, worker queueing and retail input processing fall outside this interval. |
| `goal_decisions`, `standing_decisions` | Decisions per goal for the match, and those whose left stick (under 0.2) stood the bot still. `standing_decisions.recover / goal_decisions.recover` is the share of shield recovery spent standing. |
| `look_asked`, `look_stalled` | Decisions after one whose right stick asked for a turn (over 0.3), and those whose observed aim had not moved since. A healthy bot stalls only for its input latency. A stall share near one means the look is not reaching the unit ([bots.md](../game/bots.md), "The facing release"). |

`accuracy` groups the game's registered shots and credited enemy hits by
weapon and decision range at fire time. Close is under 3 world units,
mid is 3 to under 12 and long is 12 or more. An unknown range means
the shot had no sampled visible target distance. Each row includes
`damage_code`, `weapon`, `range`, `shots`, `hits` and their ratio
`accuracy`. One shot can earn one hit credit. An empty list means the
host has no registered shots to report for that seat.

## Bounce routes

A brain-driven bot can be sent somewhere by way of a superbounce
([bots.md](../game/bots.md#superbounce-routes), docs/game/superbounce.md).
The API has no other method that sends a bot to a point; these are the way.

`bots.bounce_to {bot, to, mode?}` asks a bot for a route to `to`
(`[x, y, z]`, world units). `bot` is a `handle` from `bots.stats`, a
gamertag (any case) or a `player.<n>` id. It must be a brain seat: a
script or a recording takes no orders (`INVALID_PARAMS`). `mode` is
`help` (default: a bounce only where it gets the bot there when walking
cannot, or much sooner, else the plain walk) or `fun` (the highest launch
within reach, then on to `to`). The bot starts from where it stands: put it
at `x` first with `sim.state.set` (its `pos`), then ask. The reply
is `{bot, handle, mode, to}`; the route is planned at the bot's next
sample.

`bots.bounce_status {bot}` reads it back: `state` (`none`, `queued`,
`running`, `done`, `failed`), `to`, `mode`, `why` (`helps`, `fun`, or
`plain` when no bounce helped), `steps` (`walk_crouched`, `prime`, `walk`,
`bounce`, `drop`, `fly`, `nav`), `step` (the index it is on), `phase` (the
step's own: a prime's `start`, `in`, `release`, `out`; a drop's `line`,
`run`, `air`), `tries` (the step's try, from 1), `primed` (the level
pair word is at -3, so a landing bounces), `primed_read` (read from the
game, or assumed after the walk out because it could not be), `landing`
(the last touchdown in a bounce or a drop: `tick`, `at`, `speed_in`,
`vz_out`, `miss`, `result` `bounce`, `not_primed`, `missed`, `too_slow`
or `no_bounce`), `log` (each step and phase change, `{tick, step, what,
at}`, the last 64) and, for a failed route, `error`: `timeout`, `no_map`,
`no_path`, `goal_off_mesh`, `tail_stuck`, `path_lost_<check>_<leg>`
(the follower's check and the walk's leg, [bots.md, "Superbounce
routes"](../game/bots.md#superbounce-routes)), `prime_failed`, `pair_lost`,
`no_bounce`, `no_drop`, `stalled`, `state` (an internal fault), `enemy`,
`no_route`, `practice_off`, `died` or `map_changed`. While a route runs
the bot fights and picks up nothing. While the superbounce mode is on the
bots practise bounces instead of fighting, and `practice` gives the tally:
`state` (`idle`, `route`, `waiting`), `site`, `attempts`, `bounces` and
`spots` (each site's `stand`, `attempts`, `bounces` and `why`, by count)
([bots.md](../game/bots.md#practising-bounces)). In a sim's training run
([bounce-search.md](../game/bounce-search.md)) `practice.train` gives the
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

`superbounce.spots {map?, kind?}` lists the spots the bots know for a map
(default the loaded one): `map`, `scanned` (the nav worker has made or
read a scan; the measured spots are listed regardless) and `spots`, the
measured ones first, then the scan best first. Each has `kind` (`bounce`
or `crouch`; `kind` in the request filters), `source` (`measured` or
`scan`), `edge` (two points: a bounce spot's collision edge, its two
end vertices to the float, a busy point's vertex twice), `stand`,
`fall`, `launch`, `drift`, `score`, `need` and `fall` (measured
only: 0 for a scanned spot, which the game reads as the one threshold
every seam shares, [superbounce.md, "Speed"](../game/superbounce-spots.md#speed);
a measured spot with a recipe takeoff falls 2.43 from it), `status` (`proven` for a crouch spot at a measured priming place
or one the map's verified file proved, `failed` for one it failed, else
`likely`), `out` (a crouch spot's way out from `stand`, X and Y) and
`lip` (a crouch spot's edges overhead at `stand`, up to 8) and `shape`
(`slot`, or `crease` or `vee` for a single-crease bounce candidate, or
`busy` for a busy-point candidate). A bounce spot
with a line in the map's learned file has `learned`: `state` (`open`,
`learned`, `partial`, `dead`, `blocked`), `knobs` (as in
`bots.bounce_status`), `tries`, `hits`, `n`, `ok`, `score`, `best` and,
for a `dead` or `blocked` line, `why` (`SpotLearned`). A `dead` site is
listed, but the bots leave it out unless it is proven.

```sh
# a bot standing at x, sent to y over a ledge, bouncing on the way
h2 agent sim.state.set '{"player":2,"pos":[-12.0,3.0,-3.2]}'
h2 agent bots.bounce_to '{"bot":"Clanker","to":[-10.6,9.0,2.4],"mode":"fun"}'
h2 agent bots.bounce_status '{"bot":"Clanker"}'
h2 agent superbounce.spots '{"map":"ascension","kind":"crouch"}'
```

## `player.goto`: drive the player somewhere

`player.goto` drives `player.0` to a point or a spot with the bots'
route brain, in one call instead of stepping the pad
([bots.md](../game/bots.md#driving-a-player); the glue is
`crates/extensions/game/src/player_goto.rs`, the autopilot
`bots::goto`). It plans over the map's nav mesh, follows the path
(jumps and drops included), plans again when the player strays or stops
getting nearer, and holds the first controller's pad through the pad
tap until it is over. It works with no bots in the game: the map's nav
mesh is loaded for it (built by the nav worker when stale, else read
from the cache; `h2 bots nav <map>` builds one by hand).

Params: `to`, a point `[x, y, z]` (world units) or a spot
`{"spot": n, "kind": "bounce" | "crouch"}` (default `bounce`), by its
index among the mode's scanned spots of that kind, best first: the
indices `superbounce.verify` uses (the mode must be on and its scan
ready). `within` (world units, default 0.3, 0.05 to 10) is how near a
point counts as there; a spot ends at its approach (a crouch spot's
start, behind a bounce strip or on its launch floor). `face` turns the
player to that yaw (radians from +X toward +Y) on arriving, with the
game's teleport and no move. `timeout_ticks` (default 3600). `then`:
`stop` (default), `crouch_recipe` (prime at the crouch spot, or at the
one the planner picks for a bounce spot, then walk to its approach) or
`bounce` (a bounce spot's whole route: prime unless primed, the jump or
drop onto the strip, the ride down). The reply is `{job, kind, goal,
then, map, nav}` (`nav` is `ready` or `loading`). A new `player.goto`
replaces a running one. It is refused while a `superbounce.verify` batch
drives `player.0`, and the batch is refused while a drive runs; without
the pad tap (the `superbounce-assist` feature off) it is `UNAVAILABLE`.

`player.goto_status {wait_ticks?}` reads it back (waiting up to 290
ticks for the end): `state` (`none`, `loading`, `running`, `facing`,
`done`, `failed`, `cancelled`), `job`, `kind`, `goal`, `then`, `steps`
(`walk`, `walk_crouched`, `prime`, `bounce`, `drop`, `fly`, `nav`),
`step`, `phase`, `primed`, `position`, `distance` (left along the walk
under way, else straight to the goal), `path` (the player, then the
waypoints ahead, at most 32), `replans`, `replanned` (`strayed`,
`stuck`, `path_lost_<check>_<leg>`, `stalled`, `pair_lost`), `ticks` and, for a failed
drive, `error` (the route's reasons as in `bots.bounce_status`, `stuck`,
`timeout`, `no_route`, `no_launch_floor`, a missing nav mesh, a changed
map). `player.goto_cancel` stops it and lets go of the pad. The player
strafes along the path: it does not turn while it walks.

```sh
h2 agent player.goto '{"to":[-10.6,9.0,2.4],"within":0.2}'
h2 agent player.goto_status '{"wait_ticks":280}'
h2 agent player.goto '{"to":{"spot":0,"kind":"crouch"},"then":"crouch_recipe"}'
h2 agent player.goto_cancel
```

## `player.bot`: a Clanker plays the player's seat

`player.bot {on: true}` hands `player.0`, the local human seat, to a
Clanker brain for benchmarks and spectating: it moves, fights and picks
up weapons on the first controller's pad, through the pad tap
`player.goto` drives. `on: false`, or any real press on that pad (a
button, a trigger, a stick past half), hands it back at once; a press
`input.pad` makes does not (the tap reads the pad as it was before the
agent's steps, `agent::live_pad`). The
glue is `crates/extensions/game/src/player_bot.rs`.

| | |
|---|---|
| The brain | the phantoms' (`bots::Brain`), sampled once a game tick at the pad read, finding its player by the seat's machine id |
| Personality | hashed from the seat's own name, as a phantom's is from its gamertag ([bots.md](../game/bots.md#personality-and-temperament)) |
| The seat | stays the player's: no sign-in, no bot account, and the game record does not mark it a bot |
| Where it plays | multiplayer games only; elsewhere (menus, loads, campaign) the real pad goes through and the brain waits |
| Nav mesh | asked for as `player.goto` asks, so no phantom need be in the game |

Params: `on`; with `on: true`, `difficulty` (`easy`, `normal` (default),
`heroic`, `legendary`) and `style`, the Clankers page's Aggression
(`calm`, `normal` (default), `wild`). No params reads the state; a new
ask starts a new brain.

The reply, every time, is `{on, driving, difficulty, style, name,
ticks, off}`:

- `driving`: the brain's pad reached the game at the last read.
- `name`: the name its personality comes from.
- `ticks`: game ticks it has played.
- `off`: why it last stopped: `agent`, `pad`, `setting`, or a seat it
  can't play (one sharing its machine with another local player, split
  screen).

It is refused while a `player.goto` drive or a `superbounce.verify`
batch holds `player.0`, and `player.goto` is refused while it plays.
Without the pad tap it is `UNAVAILABLE`.

The `player.bot` setting does the same from the start of a run, so a
lab run can begin with it on: `h2 play --set player.bot=true`
(default difficulty and style). Turning the setting off hands the seat
back; a press does too, and the setting stays as it was.

```sh
h2 agent player.bot '{"on":true,"difficulty":"heroic","style":"wild"}'
h2 agent player.bot
h2 agent player.bot '{"on":false}'
```

### Saved places: `player.mark` and `player.recall`

`player.mark {slot?, player?}` keeps a player's position and aim under a
slot; `player.recall {slot?, player?}` puts a player back there through
[`sim.state.set`'s teleport](#the-games-own-functions-teleport-give-spawn).
Game methods in `crates/extensions/agent/src/sim/mod.rs`, wire types
`agent_proto::sim::{MarkParams, Mark}`.

- `slot`: a name of 1 to 32 characters, or a number taken as its decimal
  name; default `quick`. At most 32 slots; marking an existing slot
  replaces it.
- `player`: `player.<index>` or `sim.<n>`; default the player on local
  user 0 (for a recall with none found, the mark's own player).

`player.mark` answers at once: `{slot, player, pos, forward?, yaw?,
pitch?, map, slots}`, read at the tick boundary as `world.query` reads
the player (`forward` is the unit's aiming vector), `slots` every slot
kept. `player.recall` answers after the next tick with `sim.state.set`'s
teleport result (`{player, wrote, teleport, session}`) plus `slot` and
`mark`; the facing is the mark's aiming vector, else its yaw and pitch.

Refusals: a player with no position (between lives) or no local player
is `GONE`, as is an empty slot on recall; a bad slot name, a 33rd slot or
a `player` that is not a player is `INVALID_PARAMS`. A mark made on
another map is `INVALID_REQUEST` on recall. A recall is a host write: a
co-op or client session refuses it as `sim.state.set` does, and it needs
the `agent-calls` feature. Marks live as long as the process.

```sh
h2 agent player.mark '{"slot":"ledge"}'
h2 agent player.recall '{"slot":"ledge"}'
```

## `sim`: simulated players and direct state

`sim.*` lets an agent test co-op, multiplayer and Live flows from one
running game: players that are not people, game state written directly,
and assertions that answer pass or fail with what was seen. It never
starts another game instance. They are game methods
(`Agent::add_game_method`, `crates/extensions/agent/src/sim/`): answered on the
game's thread with guest memory at hand, and like `world.*` at a tick
boundary while the game ticks. The wire types are `agent_proto::sim`.

| Method | Access | Params and result |
|---|---|---|
| `sim.player.add` | act | `{kind: "phantom", identity: {xuid, gamertag}, source: script\|recording\|brain, team?, steps?, recording?}` adds a phantom peer and returns its `sim.<n>` id. `kind: "local"` is `INVALID_PARAMS` |
| `sim.player.remove` | act | `{id}` or `{all: true}` |
| `sim.player.list` | read | `players` (simulated players and their game player `player.<i>`) and `clankers` (the host's brain-driven bots and their brain state) |
| `sim.player.pad` | act | `{id, steps, append?}` sets or appends pad steps for a phantom with a script source, in `input.pad`'s step format |
| `sim.state.set` | act | `{player, pos?, yaw?, pitch?, facing? (or forward?), team?, health?, shield?, grenades?, ammo?}`: host writes; `pos` and the facing go through the game's own teleport, answered after the next tick |
| `sim.give` | act | `{player, weapon, ammo?, keep?}`: the game makes a weapon from a `weap` tag (datum or name) and gives it to the player's unit; answered after the next tick |
| `sim.spawn` | act | `{tag, pos, yaw?}`: the game makes an object from its tag at `pos`; answered after the next tick |
| `sim.view` | read | `{roots?}`: the game as one JSON document |
| `sim.expect` | read | `{checks}`: expectations judged against `sim.view`, at one tick |
| `sim.session` | read | the session kind and whether writes are allowed |
| `sim.objects` | read | `{kinds?, max?}`: every live object in slot order through the simulation's typed views, `{objects, truncated}` |
| `sim.events` | read | `{kinds?}`: the last tick's simulation events in order (`deleted`, `damage`, `kill`, `actor_killed`), `{events}` |
| `sim.ledger` | read | `{}`: the guest phases of the game tick the Rust driver still calls, `{tick, ticks, phases: [{entry, name, owner, calls, ns, last_ns}], guest_total, tick_ns, tick_last_ns, loop_ns, loop_last_ns, loops, sound}`, with nanoseconds per phase (total and last tick) and for the whole loop; answered on the connection's thread |
| `sim.render_writes` | read | `{}`: the render-writes census ([sim-render-split.md](../engine/sim-render-split.md), slice 0), process counters since the game started, `{selector_in_tick, selector_in_frame, gameplay_draws_in_frame, setup_pose_refusals, ticks_scanned, live_objects, bit30_objects, pending_objects, pending_max, pending_sum, guarded_pages_dirtied}`. "In frame" is outside the tick driver. `pending_*` count object bodies with flag bit 30 set and bit 29 clear at tick end (last scan, largest, sum over scans); `guarded_pages_dirtied` is 0 until the frame guard exists. Diff two answers for a span; answered on the connection's thread |

**Brains.** For a brain-driven simulated player, `sim.player.add`,
`sim.player.list` and the `sim` root of `sim.view` include `brain` when
the backend provides it. The list's `clankers` array includes brain-driven
phantom bots added by the game, headless matches and sim; each entry has
`name`, `kind` and `brain`, and once joined `player`. Script and recording
sources have no brain state. The `brain` object has `skill`,
`profile` as described under [Bot combat](#bot-combat),
`personality` (the nine 0 to 1 traits in
[the Clankers trait table](../game/bots.md#personality-and-temperament) and
`favorite_weapon`: `spray`, `precise`, `scatter`, `sniper`, `blast` or
`blade`) and `mood` (`confidence`, `tilt`, `fear`, `streak`, and an
optional `revenge_target` `player.<i>` while a killer is remembered).

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

**`sim.objects`** reads the objects pool through `sim-state`
(`crates/rewrites/sim-state`, the read side of the object update rewrite,
[sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 8), so
it shows what the simulation's own views see, offsets and all. `kinds`
filters by tag group (`bipd`, `vehi`, `weap`, `eqip`, `garb`, `proj`,
`scen`, `mach`, `ctrl`, `lifi`, `ssce`, `bloc`, `crea`); `max` caps the
list (default and cap 2048) and `truncated` says more matched. Each object
has `id` (its salted handle, `0x` and eight hex digits), `kind` (`null`
outside the type table), `definition`, `flags` (the header flags set:
`active`, `awake`, `post`, `new`, `deleting`, `defer`, `attach`, `child`),
`parent`, `position`, `forward`, `up` and `scale` (parent-relative under a
parent), `velocity` (units a second), `sphere {center, radius}`,
`vitality {body, shield, depleted}`, `nodes` (node matrix count) and, for
bipeds and vehicles, `unit {player, actor, team, seat, weapons, held}`.
The wire types are `agent_proto::sim::objects`.

**`sim.ledger`** reads the tick driver's ledger
(`sim_bridge::tick::LEDGER`, `engine::sim_tick`,
[sim-rewrite.md](../engine/sim-rewrite.md) "The tick driver"). `sound` is
the 30th phase's counters by name, the sound table's `sound.*`
([sound-instances.md](../engine/sound-instances.md), "Counters"); it is
left out while nothing ran. `phases`
lists the 29 phases of `game_tick` in retail order with each phase's guest
`entry` (`0x` and eight hex digits), `name`, `owner` (`guest` while the
driver calls the lifted original, `rust` once a Rust body runs it) and
`calls`, the guest calls made since the game started. `guest_total` is the
number of phases still owned by the guest, the number
`crates/rewrites/sim-bridge/tick-ledger.toml` bounds per map kind; the goal
is 0. `ticks` is how many ticks the driver ran and `tick` the game tick
count after the last of them. `ns` and `last_ns` are the nanoseconds a
phase took over every tick and in the last one, on the monotonic clock;
`tick_ns` and `tick_last_ns` are the same for the tick body, and the phases'
`ns` sum to at most `tick_ns`. `loop_ns` over `loops` is the whole loop: the
span from one tick's entry to the next's, which holds what the game thread
does between ticks (sound runs outside the 29 phases), so ms per tick is
`loop_ns / loops / 1e6`. A pause or a menu lengthens it; read it over a
window of play. The driver reads the clock once per phase and three times
per tick. It reads no guest memory, so it answers at any time, not only at a
tick boundary.

The same numbers reach `profile.counters` in `counts` as
`sim_tick_phase_<n>_ns` (phase `n`, 1 to 29, mean ns per tick since the game
started), `sim_tick_ns` and `sim_loop_ms`, and the sim report's
`tick_phases` block (`ticks`, `tick_ms_per_tick`, `loop_ms_per_tick`,
`phases: [{name, ms_per_tick}]`) over the match.

**`sim.events`** reads the simulation's event buffer through `sim-state`
(`sim_state::events`, [sim-rewrite-objects.md](../engine/sim-rewrite-objects.md)
section 8). The game answers at a tick boundary, so the events are the
tick that just ran, in the order they happened. The buffer is emptied when
each tick starts. `kinds` filters by type. An unknown name returns
`INVALID_PARAMS`. Datums are `0x` and eight hex digits,
or `null` for none. Each event has a `type`:

- `deleted`: finalize deleted an object. `id`, `kind` (its tag group) and
  `definition`.
- `damage`: the damage report applied one damage instance. `tick`,
  `victim`, `victim_kind`, `victim_player`, `owner_player`,
  `owner_object`, `owner_team`, `code` with its halves `damage_type` (low
  6 bits) and `modifier` (high 2), `effect` (the damage effect tag),
  `flags`, `depleting` (the body was depleted), `body` and `shield`
  (damage as fractions of full vitality), `direction`, `origin` and
  `position` (the victim's).
- `kill`: the game credited a kill. `tick`, `attacker` and `victim` (player
  datums), `code`, `damage_type`, `modifier`, `victim_unit`,
  `attacker_unit`, `position` and `attacker_position`.
- `actor_killed`: a damage report depleted an actor unit's body.
  `unit` is the object datum, `character` the character tag datum,
  `killer` the credited player datum, and `weapon` the packed damage code.

Events are always recorded. The records and where they are read are in
[object-update.md](../engine/object-update.md), "Slice 8 body". The wire
types are `agent_proto::sim::events`.

**`sim.view`'s roots.** `at` (the stamp), `session`, `ui` (`ui.screen`'s
answer, `null` without `H2_UI_WATCH=1`), `player` (by index: the `world`
object plus `lives` and `score`, and while the player has a unit
`health`, `shield`, `grenades {frag, plasma}` and the held weapon's
`ammo {loaded, reserve}`), `object` (every `world` object by id;
only when asked for), `count` (by kind, `team` counting players per team,
`total`), `sim` (simulated players by `<n>`, and `count`) and `input`
(`input.state`). Pointers read straight off it: `/player/1/team`,
`/object/<id>/occupied`, `/count/team/1`, `/sim/1/player`.

`lives` is the player record's lives remaining (`+0x1ac`, an `i16` the
engine writes; `null` while unlimited, -1). `score` is the player's row
in the game engine's stat rows (`+0x308 + 0x1c * index`, its `+0x00`, the
value `SCORE_ADD` changes), `null` without a game engine (the menu, a
campaign). Both are there between lives
(`crates/extensions/agent/src/sim/state.rs`, `lives` and `score`;
[firefight.md](../game/firefight.md#guest-boundary)).

**Expectations** are `h2 agent run`'s: `"<pointer> <op> [JSON]"` or
`{"pointer", "op", "want"}`, ops `==` `!=` `<` `<=` `>` `>=` `contains`
`exists` `missing` and `near {"to": number or array, "within": d}` (for
positions and vitality, read from `f32`s). One implementation,
`agent_proto::expect`, serves both: `h2 agent run` judges the last answer
on the client, `sim.expect` judges `sim.view` in the game. Its answer is
the verdict's fields, `{"pass", "failed", "expects": [{expect, pass, want,
got}], "at"}`.

### `h2 agent sim` and scripts

There is one script format: `h2 agent run`'s. Any line may be a `sim`
verb, and `h2 agent sim run <script>` is `h2 agent run` that also removes
the simulated players the script added (`"cleanup"` in the verdict). Run
this against a game this console hosts; replace `ROSTER_XUID` and
`ROSTER_GAMERTAG` with a bot identity issued by the local Live server.

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

The verbs in `crates/apps/cli/src/agent_sim.rs`: `add phantom [source]
xuid=X gamertag=G [key=JSON...]`, `remove <sim.N|all>`, `list`, `pad
<sim.N> <steps> [append]`, `set <player.N|sim.N> key=JSON...`, `give
<player.N|sim.N> <weap tag> [loaded=N] [reserve=N] [keep]`, `spawn <tag>
pos=[x,y,z] [yaw=DEG]`, `view [root...]`, `expect <expectation>`, `until
<ticks> <expectation>` (`sim.expect` every 5 ticks until it passes; a
timeout fails with the last verdict), `session`, `lobby ...` and `run`. In
`h2 agent run`, a failed `sim until` is a failed call, so it stops the
script.

**Lobby steps** (`agent_proto::sim::menu`) press player one's pad
(`input.pad`) and check each step on `ui.screen`, with a timeout, on the
client, since a step spans many ticks:

- `lobby mode <campaign|xbox_live|split_screen|system_link>`: from the main
  menu, down to the row and A; expects the campaign screen or the players'
  screen in that mode.
- `lobby host live_coop`: from the players' screen in Xbox Live mode (or
  the Live menu): A, then down from the restored row to Co-op Campaign
  (row 4), A; expects the Live menu to go.
- `lobby invite [ROW]`: Y in a lobby, then A on friend row ROW.
- `lobby start`: A on Start Game (row 0); expects the screen to go.
- `lobby press <b[+b...]> [JSON expect]`: any presses, then wait for
  `{"screen", "main_menu", "mode", "not"}`.

Their screen ids and rows are the ones `h2 coop test --drive` uses
(`agent_proto::sim::menu`, re-exported by `coop_drive`). They assume a menu
opens on row 0; a wrong row shows as the wrong screen and the step fails
saying what it saw.

### Determinism

`sim.session` reads the simulation mode (`coop_probe::read_sim`,
[live-coop.md](../live/live-coop.md) 1.1) and the squad session
(`coop_probe::read_session`) through boot's probe
(`boot::agent_glue::session_view`):

| Mode | Kind | `sim.state.set` |
|---|---|---|
| 0 | `none` (menus) | allowed (no players to write) |
| 1 | `local` (campaign, split screen) | allowed |
| 2, 3 | `coop_host`, `coop_guest` (synchronous) | refused: a write on one machine desyncs the others and trips the sim hash |
| 4 | `mp_host` (distributed server world) | allowed: host-authoritative |
| 5 | `mp_client` (distributed client world) | refused: the host owns the state and overwrites it |
| unreadable | `unknown` | refused |

A refusal is `INVALID_REQUEST` with the session in `data`. In co-op, drive
players with pads instead (`sim.player.pad`, `input.pad`): pads are
inputs, which the pad schedule carries to every machine. Writes happen at
a tick boundary only (`UNAVAILABLE` between slices), and every value is
checked before the first write, so a refused set writes nothing. A `h2
lockstep` comparison run is local (mode 1) and is not detected: a write
there makes the runs differ by design.

### What `sim.state.set` writes

Teleport and facing go through the game ([below](#the-games-own-functions-teleport-give-spawn)).
The other fields are host writes at the addresses Live telemetry reads
(`live_telemetry::layout`, `engine::view`): the player record's team
(`+0xc0`); the unit's team (`+0x138`), body and shield vitality (`+0xec`,
`+0xf0`, [health.md](../halo1/health.md); `engine::view::objects::Body`),
grenade counts (`+0x23e`) and the held weapon's first magazine (`+0x22c`
loaded, `+0x22a` reserve). Spawning a player between lives is not a
write; it needs the game's respawn and is not provided.

### The game's own functions: teleport, give, spawn

A host write of a position cannot move a player: the biped's physics puts
the body back at the next tick. So a move goes through the game
(`crates/extensions/agent/src/sim/calls.rs`), and its physics, cluster and vehicle
seat hear of it.

A game method answers in the tick observer, after the tick, where nothing
can call into the guest. So it queues a call and defers its answer
(`GameCtx::defer`, the executor's deferred answers: polled at each tick
boundary from the next, `TIMEOUT` after 30 ticks, held while paused). The
`agent-calls` feature (a `before` on `game_tick`, first;
`H2_FEATURES=-agent-calls` turns it off) finds the queue full at the start
of the next tick, makes the calls, then runs the tick itself, as our
skulls do. The answer comes after that tick. A snapshot is refused while
calls are queued.

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
  [creature probe](../game/ai-networking.md#creature-slot-probe) enrolls a
  new creature after allocation. Any object group `object_new` knows (`bipd vehi weap eqip
  garb proj scen mach ctrl lifi ssce bloc crea`). A bare name is looked for
  among those groups; if it names tags in more than one, the answer asks for
  the group (`vehi:<tag name>`). The answer: `object`, `pos` (where it was
  made), `tag`.

All three are host writes: refused in a co-op session or on a multiplayer
client, and only at a tick boundary, as `sim.state.set`.

```sh
h2 agent sim set player.0 'pos=[36.7,-80.8,1.7]' yaw=90 pitch=-10
h2 agent query '{"kind":["player"],"fields":["pos","forward","yaw","pitch"]}'
h2 agent sim give player.0 <weap tag name> reserve=108
h2 agent sim spawn <vehi tag name> 'pos=[40,-80,2]' yaw=180
```

### Object edits

The first authorized object edit or `map.reload` adopts the game for
editing and installs its lifecycle observers. Starting the agent,
listing methods and pairing with the game leave the ordinary hooks in
place. The host needs the agent tick and guest-call features. Film
recording and playback, or a boot configured to record or replay input,
exclude these object edits.

These methods share the tick and session checks of `sim.state.set` and
the peer-count confirmation of `edit.batch` (`confirm_peers` on a System
Link host with other machines). An unknown multiplayer peer count, a Live
matchmade game or a Live game another console hosts refuses the request
before queueing (a Live custom game this console hosts is a host like a
System Link one). Send the
`epoch` from `edit.map` to pin a request to that edit session. At dispatch
and again on the game thread before the engine call, a stale `epoch`
returns `INVALID_REQUEST` with “the map epoch changed”, as for a tag
edit. The hook also checks the cache identity, current session and peer
count. A timed-out request loses its place in the queue.
Object ids use the salted spelling from `world.object`. The hook checks
the id at execution, so a removed object or reused slot returns
`GONE`. Code: `crates/extensions/agent/src/sim/edits.rs` and
`crates/extensions/agent/src/sim/calls.rs`.

| Method | Params | Result after the tick |
|---|---|---|
| `world.object.set` | `{id, pos?, forward?, up?, epoch?, confirm_peers?}` | `{at, object}` with the moved object's id |
| `world.object.delete` | `{id, epoch?, confirm_peers?}` | `{at, deleted}` with the deleted id |
| `world.object.respawn` | `{id, epoch?, confirm_peers?}`, `{placement: {block, index}, epoch?, confirm_peers?}` or `{definition, epoch?, confirm_peers?}` | `{at, old, object, linked}` with the old id (null for an empty placement) and the new id; for a definition, `{at, definition, respawned, skipped, engine_made}` |

- **Respawn by definition.** `definition` is a tag datum handle in hex.
  Every placement of the loaded scenario whose palette entry names it and
  that a live object occupies is remade by the sequence of "Linked
  creation" below (`crates/extensions/agent/src/sim/calls/placement/definition.rs`):
  the replacement first, with the placement index written into the
  placement data, then the old object's deletion. `respawned` lists each
  as `{placement: {block, index}, old, object, linked}`. A placement the
  engine path cannot remake (held or attached occupant, a named or flagged
  record, extra creation fields) or that no live object occupies is listed
  in `skipped` with `{placement, why}` and left to its next creation. A
  live object of the definition that no placement made is listed by id in
  `engine_made` and not touched.
- **Set** passes the position pointer in EAX, forward in ECX and up in
  EDX to `OBJECT_SET_POSITION` (`0xb75a0`), with `(datum, 0, 0)` on the
  stack. The projectile step loads all three registers before its call
  to `OBJECT_SET_TRANSFORM`. [Retail call sites](../../crates/rewrites/engine/src/sim_objects/projectile_step.rs#L114),
  [register setup and call](../../crates/rewrites/engine/src/sim_objects/projectile_step.rs#L1767).
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
  [Placement index evidence](../live/live-telemetry.md)

### Placement edits

`world.placement.edit` is one transaction on one scenario placement block,
at one tick boundary. The editor computes it from its document diff, so an
add, a remove and their undos are the same call (design:
[realtime-edits.md](../editor/realtime-edits.md), "Placements"). It takes the
session, epoch, identity and `confirm_peers` checks of
[Object edits](#object-edits). Code:
`crates/extensions/agent/src/sim/calls/placement/edit.rs`; wire types in
`crates/extensions/agent-proto/src/sim/edits.rs`.

| Param | Meaning |
|---|---|
| `block` | A `placement::BLOCKS` name (`scenery`, `vehicles`, `weapons`, `equipment`, `machines`, `controls`, `crates`, `creatures`), or `sound_scenery`, `starting_locations`, `netgame_flags`, `netgame_equipment`, `starting_equipment` |
| `was_count`, `count` | The root count the edit started from and the count after it |
| `records` | `[{index, bytes, was?}]`: the record at `index` of the new numbering, exactly one stride in hex. `was` is the slot's current bytes in hex, or `{hash}`, their FNV-1a 64 as 16 hex digits |
| `names` | `[{name, placement}]`: point scenario object name `name` at `placement` (-1: none); object blocks only |
| `delete` | `[index]` in the old numbering: delete the live objects of these entries |
| `relink` | `[{from, to}]`: give the live objects of old entry `from` the placement index `to` |
| `create` | `[index]` in the new numbering: make an object for the entry; object blocks only |

The result is `{at, count, capacity, moved?, deleted, relinked, created}`,
where `deleted` and `relinked` are object ids and `created` is
`[{index, object?, linked, created, why?}]`. `moved` is `{address,
capacity}` when the block grew into the edit arena.

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
  ([Block moves](agent-api-edit.md#block-moves)). The host records the loaded count at the
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
- **Not reverted by `edit.revert`.** Objects were made and deleted: the
  editor sends the inverse transaction.

### Phantom backend

`boot::sim_bots::SimBots` implements `agent::sim::bots::Bots` over the
process-wide phantom registry. It places a player through
`bots::phantom::add(identity, team, source)` using a
`bots::pad_source::PadSource`. The bots feature must be enabled; a light
host also needs `--host-bots` to accept simulated players. A script source
uses the same `input::backend::remote::Remote` step queue as `input.pad`;
phantom gather samples it once per game tick. A recording supplies
successful reads of the input log's first pad. A brain supplies its own
pad. `sim.player.remove` removes players created through
`sim.player.add`. Phantoms join multiplayer games this console hosts; in a
level-boot `h2 serve` session, drive the existing player with `input.pad`,
`sim.state.set` or `sim.give`.

### Live

Sim never talks to a Live server and never makes accounts or sessions. A
phantom needs `identity: {xuid, gamertag}`, and the only place to get one
is the local embedded server (`h2 play --live-local`,
`live_server::start_embedded`), which issues bot identities and accepts
`bot: true` only for xuids it issued. Never point a sim run at the public
Live server.
