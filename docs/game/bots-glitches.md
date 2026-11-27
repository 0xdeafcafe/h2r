# Bots: glitches

The glitches a bot performs and the superbounce routes it knows (section 3.8). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 3.8 Glitches (built, not checked in a game)

The user asked: "add setting so that clankers can do glitches (super
bounce, sword fly, etc) if turned on". The Clankers page has a
**Glitches** row (`clankers.glitches`): **Off** (the default), **Some**
(the occasional show-off) and **Lots** (whenever there is an opening).

A glitch is the same button presses a player makes, with the same timing,
on the bot's pad. Nothing in the engine is changed or called. The tricks
work only because the recomp runs retail 1.0's code: no title update has
patched sword flying or the super bounce out (title-updates.md). **Not one
has been seen working in a game yet.** Each needs the checklist below.

**The code.** `bots::glitches` is pure: each trick is a list of timed
presses (`glitches::plan`, a `Macro` of `Beat`s), the rules for when one
fits (`glitches::opening`) and the show-off clock (`glitches::ShowOff`).
The fight tricks run in the noob combo's scheduler
(`weapons::Combo::step`, its `ComboIn::glitch` gate); the show-offs run
in the mind's roaming (`Mind::step`, `Stunt`). The setting follows the
Taunts path: store, `clankers::Want::read`, the driver's shared
`AtomicU8`, then `Mind::glitches` at the phantom's next pad sample.

**The tricks.** Nominal requested presses at 30 ticks a second:

| Trick | Presses (tick: button) | When | Source |
|---|---|---|---|
| BXR | 0 B, 5 X (held), 7 R | in melee reach with a gun whose magazine is full or has no reserve | [Halopedia, BXR](https://www.halopedia.org/BXR): B, then X once the melee lands (it cancels the melee's recovery), then R; full magazine only, else X reloads; [Bungie.net forum](https://www.bungie.net/en/Forums/Post/414304): "hold X while pressing R" |
| BYYR | 0 B, 5 Y, 8 Y, 10 R | the same with a part-empty magazine and a second gun | Halopedia, BXR: "press Y twice instead of pressing X once" |
| BXB | 0 B, 5 X, 7 B | the same with nothing in the magazine and nothing to load | Bungie.net forum: "like BXR, but after X press B again" |
| Double shot | 0 R, 4 R (held), 6 X | a battle rifle, 6 rounds loaded and a reserve, the target in its range | Bungie.net forum: RRX, "holding the second R while pressing X"; "on a last clip it just shoots twice" |
| Quad shot | a double shot, 9 Y, 12 Y, then a double shot from 15 | as the double shot, 12 rounds and a second gun | Bungie.net forum: "RRX, YY, RRX"; YY swaps back before the reload |
| Sword flying | 0 Y, 1 X, 3 R, 11 A | the sword the other weapon, the gun in hand on target (its reticle red) past the sword's lunge, up to 20 units | [Halopedia, Sword flying](https://www.halopedia.org/Sword_flying): Y, X, RT "quickly in that order", the reticle red on the gun first; the window is 0.066 s at 30 fps (two ticks); A in the lunge goes higher |
| Crouch jump | 0 A, 7 crouch (10 ticks) | roaming, now and then | superbounce.md, "The crouch height term": a crouch change in the air lifts the feet |
| Super bounce | prime at a crouch spot (crouch, let go under the low ceiling), then at a spot: stand still, A, hands off but a light push toward +y, land; again from a nudged place | roaming, a spot within 60 units | superbounce.md (traced; the spot's table is `glitches::spots`, the scan `spots`) |

The 5 ticks from B to the melee landing (`MELEE_CONNECT`) and the 4 for
the battle rifle's burst (`BURST`) are not traced. They are starting
points to tune in a game run. The battle rifle is told from the tag: once
per pull, one zoom level, a magazine of 30 or more.

**Skill.** Tricks request the same sequence at every skill. The pad
pipeline scatters its edges (§3.4), so a miss is an ordinary melee,
switch, reload, shot or jump. The trigger's charge and release in the
noob combo and short pulls on other weapons go through the same pipeline.
Skill also scales how often it tries. Easy takes 40% of the setting's
chance, Normal 70%, Heroic 90% and Legendary all of it.

These are timing-window pass rates from 4,000 seeded controller-model trials
per skill and technique, with the right stick at rest. The criterion is that
the delivered presses arrive in order and within a tick of each nominal gap,
keeping required holds such as X at R for BXR. A melee cancel or second BR
pull can't precede its nominal opening. The sword fly needs X one or two
ticks after Y. The plasma overcharge row checks the nominal 24-tick hold.

These rates measure pad timing. Whether the retail engine accepts an attempt
depends on the weapon, target, movement and host behaviour. Super-bounce
landings need a physics run.

| Delivered sequence | Easy | Normal | Heroic | Legendary |
|---|---:|---:|---:|---:|
| BXR | 80.5% | 85.7% | 89.3% | 93.4% |
| BYYR | 66.4% | 74.8% | 80.5% | 87.2% |
| BXB | 79.7% | 85.3% | 89.0% | 93.2% |
| Double shot | 80.4% | 85.6% | 89.2% | 93.4% |
| Quad shot | 49.7% | 61.5% | 69.8% | 80.3% |
| Sword fly | 56.6% | 65.1% | 73.0% | 81.2% |
| Crouch jump | 98.0% | 99.0% | 99.5% | 99.8% |
| Plasma overcharge hold | 84.6% | 87.7% | 90.6% | 93.7% |

**How often.** A fight trick is rolled for at most once a second while
one fits: 15% a roll on Some, 60% on Lots (times the skill's share), and
not again for 2 s after one. A show-off comes every 45 to 90 s of calm on
Some and every 8 to 20 s on Lots (longer at lower skill). A super bounce
gets 3 tries on Some and 8 on Lots, then the bot gives the spot up.

**Never:**

- with the setting Off;
- carrying the flag, the bomb or the ball, or with an objective to play
  (show-offs);
- when it would obviously lose: shields down (recovering), withdrawing,
  or another enemy within 8 units; a sword fly never at a target with
  another enemy within 2.5;
- before its reaction to the target has passed;
- doing anything but fighting or roaming on foot (`mind::glitch_doing`,
  an allowlist: getting in, driving or riding a vehicle is never on it).
  The vehicle work's seat pad returns before the fight and roam code, so
  it must call `Mind::drop_glitches` first;
- a show-off with anyone in view, or within 1.5 s of seeing someone.

While a trick runs the mind's own melee, reload, switch and grenades
wait, and the trigger is the trick's.

**The super bounce spots** (`glitches::spots::SPOTS`) hold only measured
spots. Each spot has a map, the place to stand, a spread to nudge within
between tries, a drift direction, a source and, for a spot a standing
jump can't reach, the takeoff of the running jump onto it
(`Bounce::takeoff`) and where its traced bounces landed, which a route
aims at (`Bounce::landing`, the site's `Site::aim`). There is one:

| Map | Stand at (x, y, z) | Drift | Takeoff | Aim | Source |
|---|---|---|---|---|---|
| `ascension` | (−10.62, 3.88, −3.19), nudged up to 0.04 | +y | (−8.4, 2.5, −0.76) | (−10.45, 3.985, −3.19) | superbounce.md: the crease where the ramp meets the floor; every traced bounce fell onto it at −5.26 to −5.40 from the recipe's running jump off the upper level, a 2.43 drop, landing on x −10.2 to −10.7, y 3.93 to 4.04 (the aim is the middle, which the takeoff heads at along the recipe's (−0.8, 0.6)); the launch is a pinch between faces at y 4.12 and 3.94 |

A bounce needs the player primed first: a forced crouch leaves the level
pair word at -3 and only a landing with it at -3 bounces
([superbounce.md](superbounce.md), "The pair word"), and the landing must
come in fast enough: a standing jump lands at about −3 units/s and never
bounces on Ascension's patch, which needs −5.4. The measured spot is
therefore a drop from its takeoff (`spots::Site::measured`: fall 2.43,
`spots::MEASURED_FALL`), the recipe's running jump (below). The bot primes
before it goes to a spot (below).

On a standing site it walks to 0.8 units short of the spot, steps onto it,
stands still for 4 ticks, jumps with the stick let go, and pushes the stick
toward the drift in the air. Not every landing bounces, so it tries again
from a nudged place. It knows it bounced when it rises 0.4 units again after
starting to fall: a plain landing never does.

Community guides ([Halopedia, Super
bouncing](https://www.halopedia.org/Super_Bouncing)) give spots on most maps
by landmark only. A spot goes in the table only once a game run has found
and traced it; the scan below finds the rest.

### Superbounce routes

`bots::spots` holds what a bot knows of its map's spots: the measured ones
first, then the scan of the map's collision geometry
(`crates/live/superbounce`: **bounce** spots, narrow slots a primed landing
launches out of, and **crouch** spots, where a standing player is forced
into a crouch). The nav worker scans a map when it builds or restores the
nav mesh (`autonav::Worker::keep_spots`) and saves the result beside it as
`<map>.spots` (after the mesh is published and saved, and the scan is caught
if it panics; a failed scan leaves an empty stamped file so it is not
repeated every launch), stamped with the nav generator, the scan version
(`spots::SCAN`) and the map's fingerprint. The worker also reads the map's
floor polygons and publishes them in memory (`spots::read_floors`,
`spots::publish_floors`) for the takeoff gate; they are not another cache
file.

A scanned bounce spot becomes a `glitches::Bounce` (source `scan`, spread
0.03) with its launch speed (`spots::Site`); one at a measured spot's place
is dropped. A site with no fall (0.5 or less) and no takeoff is jumped onto
in place; one that needs a fall is dropped onto from its takeoff or a ledge
(below).

**Verified spots.** The map's verified file (`<data>/bots/<map>.verified`,
written by `superbounce.verify`, [superbounce.md](superbounce.md),
"Verifying spots") is read beside the scan (`spots::verified_for`,
cached; `spots::store_verified` merges a batch in and bumps
`spots::generation`, so a bot's spots are rebuilt). A proven spot is
looked at first and scores 1; a failed one, scanned or measured, is left
out. A proven crouch spot carries where its working trial started and
its way in (`Crouch::start`); a proven strip carries the working landing
point (`Site::aim`) and drop height (`Site::height`). The game sets the
file's directory when it starts (`spots::set_verified_dir`, from
`bots_nav_auto`).

**Learned knobs.** The map's learned file (`<data>/bots/<map>.learned`,
written by a sim's training run, [bounce-search.md](bounce-search.md)) is
read from the same directory (`spots::learned_for`, cached;
`spots::store_learned` replaces a site's line after a training round and
bumps `spots::generation`). `Known::for_map` gives each site its line's
knobs (`Site::knobs`, by `Site::learned`: the knobs of an `open`, `partial`
or `learned` line, `superbounce::learned::knobs_for`) and the line's state
(`Site::line`). A site whose line is `dead` is left out, as a failed one is,
unless it is proven (measured, or proven by a verified entry); a `blocked`
line changes nothing. A knob no line sets is the route's own:
`bounce_route::flown(site, from, tune)` makes each of a site's numbers
(`Flown`: the `how`, the target, the heading off the launch-to-target line
by `yaw`, the run-up `back`, the run's stick, the jump's `early`, the air
stick's `gain` and let-go distance, a standing jump's `drift` and `still`)
from the knob, else the expression below, and the drop, its air steering and
the standing bounce read `Flown` alone for them.

A line's `fx fy fz` is the launch point ahead of the takeoff and the launch
floor (`bounce_route::launch_from`), its `ax ay` the target ahead of the
verified aim and the stand (`Site::target`; `Site::own_target` is the target
without the line), and its `how` says whether the site is a standing jump or
a drop (`Site::standing`). A site with no line plays exactly as it would
without the file.

`bounce_route::plan` plans a route from the bot to a point, purely, from
a walk-length function over the nav mesh:

1. walk to the crouch spot's start: outside the lip on the approach,
   0.35 units from `stand` against the way out, or where its verified
   trial started;
2. **prime** (`glitches::prime::PrimeRun::at`), the recipe: crouch and
   stand still 6 ticks, crouch-walk in under the lip to 0.06 past
   `stand`, hold 12 ticks, let go (still pushing in, 6 ticks), then walk
   out along the way out. Done when the word reads -3 on the way out.
   A word that reads something else for 30 ticks out, or no arrival,
   tries again from the start, along the approach turned 25 degrees and
   then −25 (3 tries, the first along a verified way in), then fails
   (`prime_failed`). When the word cannot be read it is assumed primed
   0.5 units out. A crouch spot with no way out known primes in place:
   crouch 12 ticks, let go, stand up;
3. for a site with no fall, walk to 0.8 units behind it, never
   crouching, then the bounce run (`glitches::BounceRun`) until it
   launches. The run settles onto the stand and jumps; settling that
   moves the body less than 0.002 units a tick for 12 ticks
   (`glitches::SETTLE_STILL_TICKS`) jumps from where it is, since a
   primed body that stands still 38 ticks sleeps and wakes with its word
   at 0 ([superbounce.md](superbounce.md), "Charge"). For a site that needs a fall, walk to its **launch
   point** (`bounce_route::launch_from`): its takeoff when it has one,
   else its **launch floor** (`bounce_route::launch_floor`: the nav
   mesh's point nearest the site's aim sideways, within 3 units, whose
   takeoff gives a 3.45 apex-to-floor drop within the 3.35 to 3.60
   window; a run-off from that height, else a running jump from a jump's
   rise lower), then **drop**. An old measured fall height does not
   choose the floor; an explicit measured or learned takeoff still wins.
   How a drop leaves the floor comes from the flight model
   (`bounce_route::drop_takeoff`): gravity 4.15, a jump 3.08 up, the
   ground speed of the run's stick (`bounce_route::ground_speed`: the
   drill runs' map, nothing in the dead zone, about 1.05 units/s at 0.6
   to 0.85 of stick, 2.0 at 1, **[needs more points]**), and what the air
   stick can do over the fall (`bounce_route::air_reach`: it drives the
   sideways speed toward the stick times 1.2 at 1.05 units/s², so it can
   brake a flight to a stop and push one to 1.2 at most). A **run-off**
   at full stick (no jump) is taken when it comes down at least as fast
   as the site needs (`need`: a fall from rest lands at `sqrt(2 g h)`)
   and the air stick can land it on the aim; else a run-off at the
   tuning's slower `run_off_speed` that can; else a **running jump** at
   full stick, then at `run_off_speed`, that comes down fast enough and
   gets there; failing all, of those fast enough the one whose aim is
   least out of reach (a learned `how` picks the run-off or the jump, and
   `run` the stick). Once lined up, before the run or jump,
   `bounce_route::judge_takeoff` flies the takeoff and asks
   `superbounce::rule::predict` for its outcome. The aim is at most 0.05
   from the nearest floor-polygon edge (moved to 0.02 from it when
   needed), with 0.3 units/s held across it; a takeoff the rule does not
   call a bounce ends as `takeoff_rejected`. Training tries are not
   gated, so they can measure the failures. Either way the drop lines up
   0.6 behind the launch point (a learned `back`; 45 ticks at most) and
   runs at the aim (turned
   by a learned `yaw`) at that stick, steered
   back onto the line through the launch point (60 ticks to leave the
   floor, else `no_drop`). A running jump presses A 0.05 short of the
   launch point plus the ground the body covers in the 5 ticks it takes
   to act on the press (`bounce_route::INPUT_LAG`), holds it 3 ticks and
   keeps running until the body leaves the floor. Ascension's measured
   spot is a full-stick running jump: a run-off from its 2.43 lands at
   4.49, under the 5.4 the traced bounces came in at, and the jump's
   flight, 2 units/s off the floor, reaches 1.9 to 2.8 against the aim's
   2.5. In the air the stick steers where the landing comes down
   (`bounce_route::air_stick`): with the sideways speed held, from where
   the body is once the stick acts and the fall left then, against the
   aim, it asks for the speed that, reached at 1.05 units/s² and then
   held, lands on the aim, 1.2 times the change that is just enough
   (`air_gain`, or a learned `gain`); within 0.03 (`AIR_DEAD`, or a
   learned `letgo`) it holds the speed, and a landing more than
   0.3 past the aim, or a miss it can't make up in the time, gets the
   full stick. Every stick is at least 0.2 or none: under the move dead
   zone (0.15) the body doesn't move. A measured spot aims at its traced
   landing even once a verified drop says where one bounce landed
   (`Site::verified`). It launched when it rises 0.4 over
   its lowest point after the top; a landing that did not launch walks
   back to the launch point and drops again (3 drops, then `no_bounce`);
4. fly: steer toward the goal at full stick until down
   (vertical speed under 0.3 for 2 ticks after the top);
5. walk the rest.

A touchdown during a bounce or a drop is kept (`bounce_route::Touchdown`:
the tick, the origin before, the speeds in and out, how far outside the
slot, the word) and named as the HUD's landings are: `bounce`,
`not_primed`, `missed`, `too_slow` or `no_bounce`. Each jump or drop is a
**flight** (`bounce_route::Flight`, `OrderRun::flights`, the last 8):
from the tick the step goes airborne (the jump pressed, or a run-off
leaving the floor), where it left (for a jump, where A was pressed,
about a quarter of a unit before the body leaves the floor at full
stick), the ground speed then, the best vertical speed in the 10 ticks
after, the word on that tick, the stick
asked in the air (mean, most, and the ticks let go), and the touchdown
it ended in with its result (`no_landing` when none was seen).

The plan is the best bounce under the asker's mode. **Help** (the
default) takes a bounce only where it reaches the goal and the plain walk
cannot (no path, such as a ledge above a jump), or where the whole route,
with 8 units for the priming, is 4 units shorter than the walk. **Fun**
takes, of the spots within 60 units of walking, the highest launch (the
cheaper on a tie) and then goes on to the goal. With no bounce that
qualifies it is the plain walk. A bounce needs a crouch spot to prime at,
unless the bot's pair word reads -3 already.

**Walks to spots off the mesh.** A scanned stand sits on a seam or an
edge, so the point a walk goes to (a prime's start, a bounce's approach, a
drop's launch floor) may be just off the nav mesh, or a little under the
floor beside it. A walk (`nav::RouteWalk`) first puts its goal on the mesh's
floor where a polygon under it has one from 0.3 under it to a standing
height (0.725) over it (`nav::floor_goal`): an approach 0.8 behind a crease,
on the ramp rising from it, keeps the crease's height otherwise, under the
ramp. It has arrived within its tolerance sideways and 0.6 units up or down
(`nav::route::ARRIVE_Z`).

A goal off the mesh within 1.5 units (`nav::TAIL_REACH`) of the corridor's
end, 0.5 units over it to 0.8 under it, with nothing solid between them at
knee height and the collision's floor under the line every 0.25 units
(within 0.3 over it to 0.5 under it, so never off a ledge or a sill), is
walked to by a **last stretch**: the corridor to its end, then straight at
the goal, slowing as it nears it. A last stretch that gets no 0.05 units
nearer in 20 ticks (under the 38 still ticks a primed body sleeps after)
ends a walk to a prime's start or a bounce's approach where the body got to
(the prime and the bounce steer the rest themselves), and fails a walk to a
goal (`tail_stuck`). A goal farther off is `goal_off_mesh`.

Within 0.5 units of its goal on its last leg a walk does not stop for the
standing capsule's headroom ahead (`nav::route::END_HEADROOM`): the probes
reach past the goal, under the lip a crouch spot's start stands beside. A
walk to a crouch spot's start (`RouteWalk::ducking`) does not stop for
headroom at all: under a ceiling the standing body does not fit it crouches
and walks on, since the prime crouches next anyway. Other walks stop there,
standing.

**A lost walk.** When the follower gives a walk up, the route plans the
walk again from where the bot is, 3 times a step
(`bounce_route::WALK_REPLANS`, each logged with the check), then fails
naming the check and the leg (`bounce_route::lost_on`):
`path_lost_<check>_<leg>`. The check is the follower's: `stuck` (no nearer
in 30 ticks, a jump, and no nearer in 30 more), `fell` (its waypoint above
it past 1.2 units plus the distance), `jump` (stuck at a jump or drop link),
`link` (a lift, teleporter, elevator or crouch jump that did not carry it)
or `ended` (no waypoint left). A walk lost at a jump or drop link (`jump`: a
rail or lip the mesh lacks) is planned again around it: every later walk of
the route skips a jump, drop or crouch-jump link within 0.5 units sideways
(`nav::route::AVOID_REACH`) and 1 unit in height of where it gave up
(`RouteWalk::failed_link`, `RouteWalk::new_avoiding`), so it takes another
way down or fails `no_path` rather than pushing at the same link.

The leg is `crouch` (to a crouch spot's start), `approach` (to a bounce's
approach or a drop's launch floor) or `nav` (any other walk). The planner
measures walks the way a walk is taken (`bounce_route::path_length`,
`nav::walk_length`): no teleporter or crouch-jump link, and a goal off the
mesh only within a last stretch's reach, so a plan holds no walk that fails
`no_path` or `goal_off_mesh` when it is walked.

A map can have hundreds of spots, so a plan is bounded: spots farther
than the limit in a straight line (the walk it must beat in Help, 60
units in Fun) are dropped before any path query; of the rest the 4 crouch
spots nearest the bot and the 8 likeliest sites are looked at; each crouch
spot's walk from the bot is asked once, each site is paired with the
crouch spot nearest it; and a plan makes at most 32 path queries
(`bounce_route::MAX_QUERIES`). A show-off plans for the one nearest site
it has not given up on (`Stuck::blocked`), and gives it up when the route
fails.

The arc: up at the site's launch speed, gravity 5.2 units a second
squared (`nav::route`), sideways at 2.5 units a second in the air. The
flight reaches a goal when the horizontal gap is within the time it takes
to come down through the goal's height, times that speed. The air speed
is a **[guess]**; the launch speeds are the scan's estimates and, for the
measured spot, the 12.3 of superbounce.md.

**Priming state.** The brain asks for the player's level pair word
through a reader the host installs (`bots::set_pair_reader`: the unit's
datum in, the word out; the game's reader is `live_telemetry::havok_agents`'s
`fixed_pair_word`). It is read only while a route is asked for or under
way (`Mind::wants_pair`), not on every sample. With it a priming ends when the word reads
-3, a word that reads anything else on the ground after priming
(walking, lining up or running off a drop, or settling on a bounce's
stand before the jump) sends the bot back to the crouch spot (twice at
most), and a priming whose
word never reaches -3 fails (`prime_failed`). Without it the bot assumes
it is primed once the stand-up has had its 12 ticks; `bots.bounce_status`
says which (`primed_read`). `Mind::primed` is true from the priming to
the bounce, and while a route runs its pad has no crouch button.

**Orders.** `Mind::order_bounce(to, mode)` (through
`PadSource::order_bounce`, `Phantoms::order_bounce`) asks for a route;
it is planned at the next sample from where the bot stands and played as
the bot's whole pad (`mind::order`, through `RoutePad`, as an
`input.route` is; a death or a change of map ends it, recorded as `died`
or `map_changed`): the bot fights and picks up nothing until it is over
(the agent API's `bots.bounce_to`, docs/runtime/agent-api.md). A route
gives up after 200 s. `bots.bounce_status` gives its step and the step's
phase and try, the last touchdown, and a log of each step and phase
change (the last 64). `PadSource::hold_pad` (`Phantoms::hold_pad`) holds
a brain's pad at a given state in place of its own until let go: the
verifier and the sim's seam drops drive a bot seat with it. A wrapper
source (a Clanker, the sim's `Metered` seat) forwards it to its brain. With the Glitches setting on, a show-off at a spot
with a crouch spot known is a **Fun** route back to where the bot began;
it ends when the bot sees an enemy.

### Breaking the glass

A crouch spot's walk in can go through glass: Zanzibar's window bounce
crouches on the sill past the pane ([superbounce.md](superbounce.md#zanzibars-window-trace-lab-play)).
That glass is the structure BSP's **breakable surfaces**: collision
surfaces with flag bit 3 (`0x08`) and a breakable index (the surface's
`+5` byte, `0xff` none), each piece of a pane its own surface. The game
skips a piece in collision once its bit in the BSP's live breakable mask
is clear (docs/formats/collision.md, "Breakable mask").

- **Kept with the walls.** The nav build keeps every breakable surface,
  its index and outline, beside the walls (`glass::panes`,
  `los::Walls::glass`, in the `.bots` file). Their triangles stay in the
  walls, solid, so line of sight and the jump sweeps treat glass as whole.
- **In the way.** Before a prime, the route finds the pieces a crouched
  walk from the prime's start to the stand passes through
  (`bounce_route::glass_in_the_way`, `glass::in_the_way`): lines at 0.1,
  0.25 and 0.4 over the floor, the walk's centre and 0.15 either side,
  on 0.175 past the stand. They all run under the crouched capsule's top
  (0.5), so the piece the crouch is forced by stays.
- **Whole or broken.** The bot reads BSP 0's mask each tick while a route
  is asked for or under way, with the pair word (`guest::breakable`:
  `[0x4ed280]`, the enable byte, then 32 bytes a BSP from `+1`). A set bit
  is a whole piece.
- **The break step.** With a piece in the way whole, the prime waits: the
  bot stands at the prime's start, faces the lowest whole piece, aims at
  its centre from eye height and holds the trigger while the aim is
  within 4° (`mind::RoutePad::report_shooting`). The route's phase reads
  `break`, and its log `break_glass`, then `glass_broken` when every
  piece is broken and the prime begins. After 150 ticks
  (`bounce_route::BREAK_TICKS`), or when the mask can't be read, the
  route fails `glass_whole`. A practising bot only shoots glass; it never
  fights.
- **Which spots.** The pieces come from the map's geometry, so any crouch
  spot behind breakable glass gets the step, scanned or measured. A crouch
  spot with no known way out primes in place and has none. With no mask
  read (no pair reader host, `player.goto` routes) the prime runs as
  before.

`h2 agent world.breakable` reads the same mask
([agent-api.md](../runtime/agent-api.md#worldbreakable-broken-glass)).
Tests (`cargo test -p bots`): `glass::tests` (only flagged, indexed
surfaces are panes; the walk in goes through the low piece and under the
lip's; a piece is whole while its bit is set),
`guest::tests::the_breakable_state_reads_each_bsps_mask_after_the_enable_byte`,
`bounce_route::tests` (the window primes from the room floor; a whole
piece is shot out before the prime; glass that never breaks fails
`glass_whole`), `cache::tests` (the glass round trips in the file).
