# Bots: practising and driving bounces

How bots practise bounces, let players go first, chain and drive a player (section 3.8, continued). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## Practising bounces

While the superbounce mode is on (a game whose script requires
`hud.superbounce`: Superbounce, Bounce Practice, or a variant of
either; the game calls
`bots::set_practice` from `superbounce_mode`, and each brain follows it
at its next sample) a bot fights no one: it picks no target, shoots,
throws and melees nothing, and being shot changes nothing. Its whole pad
is a practice loop (`mind::practice`, ahead of everything else in
`Mind::step`):

1. stand a moment, looking round (20 to 60 ticks at first, then 45 to
   150 between tries, drawn each time), stepping out of any player's
   way ([Letting players go first](#letting-players-go-first));
2. pick a site within 120 units that a route reaches, the best weighed
   first (`practice::weight`):

   | What | Weight |
   |---|---|
   | distance | −1 a 60 units, times the bot's roam (0.4 to 1.6) |
   | a proven site | +2 |
   | a site the learned file calls `learned` | +1 |
   | another bot's claim on it | −3 for each; a claim near it −1.5 next to it, down to 0 at 6 units (`practice::crowd`) |
   | the bot's recent tries of it | −1.6 times the bot's novelty (0.6 to 1.4) for each, fading as `exp(−age / 2700 ticks)`, its paired crouch spot's tries counting half (`practice::recency`) |
   | a player using it or its crouch spot | −2.5 on it, −2.0 on the way onto it |
   | the bot's taste for its shape | −1.2 to +1.2: slots, or creases, Vs and busy points |
   | a Monster site | +3 times the bot's pull to Monsters |
   | the site it goes again at after a miss | +8 |
   | the random part | 0 to 0.8 |

   A try that failed on the way (a walk, a prime, no route) blacklists
   the site for a while (`Stuck::blacklist`), so the next try is
   somewhere else, and counts against what it failed at (below); a
   landing that did not bounce is shrugged off ([Fumbles](#fumbles)).
   With no crouch spot to prime at (and the word not −3) or no site a
   route reaches, it waits 300 ticks and looks again (`waiting`);
3. run the bounce route to the site (`bounce_route::plan_site`: that one
   site, priming at its paired crouch spot, then the jump or the drop,
   the touchdown classified, over once down from the launch; going on
   into a chain when it planned one, [Chains](#chains)), and tally
   it: a try, a bounce when the route ended done by the site, else why
   (the route's error, or the landing's result for one that landed
   without launching).

The memory of recent tries is the bot's own (its last 16 sites and 16
crouch spots), so a bot works round a map's sites rather than cycling
the same few, and the claims spread the bots over it.

**Which sites a route reaches.** The first time a pick looks at a site
it pairs the site with a crouch spot (`bounce_route::pair_site`), and
the bot keeps the pairing while the map and its spots stay the same:
the site's approach (behind it, or its launch floor), and of the 6
crouch spots nearest that approach within 60 units, proven first, the
one whose start a walk reaches and from which a walk reaches the
approach, proven first and then the shortest walk. A site with no approach (a drop with no launch floor, no
launch) or no crouch spot a walk reaches it from is never tried (a
primed bot needs no crouch spot). A paired site is picked when the walk
from the bot to its crouch spot's start (primed: to its approach)
exists; one with none is passed over for 600 ticks
(`practice::OUT_OF_REACH_TICKS`). When the walk that fails is the one
to the paired crouch spot's start, the crouch spot is what is out of
reach (a floor a walk drops down from to the site but none climbs to
from the bot): it is set aside for the same 600 ticks, its sites pair
again without it, and the pick looks again 2 ticks later. A pick makes at
most 4 pairings and 4 of those walks, and with more to look at picks
again 2 ticks later. A practice try is therefore never spent finding
out that a site is out of reach: `no_route` is a site whose route
failed to plan from where the bot stood, which a pairing rules out but
for a bot that moved between the pick and the plan.

**Giving up what keeps failing.** A failed try counts against what it
failed at (`Mind::practice_strike`): a walk to the crouch spot that
failed (`no_path`, `goal_off_mesh`, `tail_stuck`, `path_lost_*`), or
its prime, against the crouch spot; a walk to the approach that failed,
against the site. Strikes lapse: one more than 3600 ticks (2 min,
`practice::STRIKE_TICKS`) after the last starts the count again. A
crouch spot with 2 (`practice::CROUCH_STRIKES`) is given up and the
sites paired with it pair again without it; a site with 3
(`practice::SITE_STRIKES`) is given up. Either comes back after 5400
ticks (3 min, `practice::GIVE_UP_TICKS`). A failed prime also counts
for every bot on the map (`practice::PRIME_FAILS`, by the crouch spot,
lapsing the same way): the geometry that refuses one bot's crouch
refuses them all, so a crouch spot whose primes failed twice between
the bots is given up by each of them. A proven crouch spot
(`superbounce::PROVEN`, or verified) is never given up. When a pick
finds nothing while something is given up, everything given up comes
back at once and the pick looks again: giving up never leaves a map
with nothing to try. A landing that did not bounce counts against
neither. A pick that finds nothing says why once a map in the log
(`bots: bounce practice: nothing to try: ...`: the sites and crouch
spots known, how many crouch spots are set aside as out of reach, and
how many sites had no crouch spot to prime at, no
approach, no walk to them, were resting, given up or too far).

### How each bot practises

Each bot practises its own way (`mind::practice::style::Style`): every
trait a continuous draw from its name's hash (`NameProfile::seed`, the
brain's seed without a name; lanes 40 to 48 of the profile's draws),
placed inside its skill's band, so a name practises the same way every
match and two names differently. The fumbles blend half the name's
try-hard (the profile's own), so a keen name fumbles less, and the
chains half too, so a keen name chains more. Match randomness picks the
moments; the name sets how often.

| Trait | What it changes | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|---|
| Fumbles | share of tries set up a little wrong | .15–.30 | .10–.20 | .06–.14 | .03–.10 |
| Chains | chance it goes for the next link when one is in reach | .15–.45 | .25–.60 | .40–.80 | .55–.95 |
| Monster pull | times +3 on a Monster site | .10–.60 | .15–.75 | .20–.85 | .25–.95 |
| Slot taste, crease taste | 0.5 neutral; times ±1.2 on a site of that shape | 0–1 | 0–1 | 0–1 | 0–1 |
| Roam | the distance weight's factor: 1.6 stays near, 0.4 ranges far | 0.4–1.6 | 0.4–1.6 | 0.4–1.6 | 0.4–1.6 |
| Novelty | how much its recent tries weigh a site down | 0.6–1.4 | 0.6–1.4 | 0.6–1.4 | 0.6–1.4 |
| Retry | chance it goes straight back to a site it just missed | .20–.70 | .20–.70 | .20–.70 | .20–.70 |
| Patience | how long it waits for a player before going elsewhere, ticks | 90–360 | 90–360 | 90–360 | 90–360 |

### Fumbles

A person misses now and then, and a bot does too: at the start of a
try, with its fumble rate's chance, the bot sets the try up a little
wrong (`practice::fumble`), one of the site's knobs off, and then flies
it as it is. Nothing scripts the fall; a fumbled try can still bounce.

- `off_line`: the aim 0.12 to 0.25 units across the seam (along its
  drift, either way; any way round a busy point, which has none). A
  landing more than 0.1 off the seam line does not bounce (the
  sweep of 60 Ascension seams);
- `early`, `late` (a running jump off a launch floor): the takeoff 0.15
  to 0.30 units of run early, coming down short, or 0.10 to 0.20 late,
  coming down long;
- `lazy_air` (any drop): the air stick let go 0.15 to 0.30 units from
  the target, so the landing drifts off.

A standing jump can only be off the line. The drop's height is the
launch floor's, so a fumble never picks a wrong height. After a landing
that did not bounce (`missed`, `too_slow`, `no_bounce`, `not_primed`),
fumbled or not, the bot shrugs: it stands 60 to 120 ticks facing the
site and, with its retry chance, goes straight back to it once (weighed
+8, not blacklisted); otherwise the site rests (`Stuck::blacklist`) and
the next try is elsewhere. The try's line ends `; fumbled <how>`.

### Letting players go first

A practising bot never blocks a player, a person or another bot, and
never treats one as an obstacle to flee or path round
(`mind::practice::yielding`). It tracks every living player's position
and speed tick to tick. A site's floor is the line from its approach
over its target and 1 unit on (the landing), 1.5 units round it; a
crouch spot's, the line from its prime's start to its stand, 1.2 units
round. A player uses a floor while on its level within it (from 1.5
under to 3 over, or in the air over it, a flight off it), or on the way
onto it (moving faster than 0.8 units/s, their course enters it within
1.5 s).

- **Before the jump**: while the route walks to a crouch spot or to the
  approach, and the bot is within 3 units of that floor's reach, a
  player using the floor stops the route (`yield`: neither a try nor a
  strike, and no rest for the site). The bot waits (`yielding` in
  `bots.bounce_status`), facing the player with a glance about now and
  then and a nudge of the stick every 30 ticks so the body does not
  sleep; standing in the floor's reach, it first walks back out of it,
  2 units past. Once the floor has been clear 20 ticks it plans the try
  again from where it stands; past its patience it gives the site up
  for now (counted as a recent try, so the pick looks elsewhere).
  Nothing interrupts a prime, a jump, a drop or a flight.
- **Between tries**: standing, it steps 2 units across the course of a
  moving player whose course passes within 1 unit of it in the next
  1.5 s (from within 8 units, ahead of them), and off the floor of a
  site another bot is on a try at; at most once in 30 ticks, a plain
  walk.
- **Between bots**: a bot on its own try always goes first. Two bots
  waiting for each other do not wait forever: the claims say who waits
  (`practice::claim_as`: the bot's player slot, since when it claimed,
  whether it waits), and the older claim goes first.

A player on a site also weighs it down in the pick (above), so a bot
usually picks another.

### Chains

A chain is the 2nd to 6th bounce with no ground between
([superbounce.md, "The rule"](superbounce.md#the-rule)). At the start
of a try the bot may plan one (`bounce_route::chain`): from the site's
launch, the sites a flight can come down on without touching the
ground, the least steering first (the site itself among them, straight
back down), and with its chain chance a link at a time, at most 3, one
of the 3 easiest, another site than the one it is off 3 times in 4. The
route goes on after the first bounce with a `chain` step for each link
(`bounce_route::Step::Chain`, `chain_onto`): in the air off the last
launch it steers the landing onto the link's target as a drop's air
does, and the link is made when it is thrown up again. A link that comes
down without launching ends the chain; the rest are passed over. The
route's view counts the links tried and made (`chain`), and the try's
line ends `; chain M of N`.

The flight is a trait (`chain::Trajectory`). The one built in,
`chain::Ballistic`, is conservative: the site's launch estimate
straight up, the measured fall gravity (4.15), the measured air control
(1.05 units/s² up to 1.2 units/s) with three quarters of that reach
counted, a touchdown at or over the link's `need`, and a V's arm only
from across its bisector. No flight after a bounce has been traced, and
whether a seam bounces a landing at a chain's speed (8 to 13 units/s
in, against the 5.3 the measured drops come in at) is open
([superbounce.md, "What is known about a second bounce"](superbounce-ghosts-medals.md#what-is-known-about-a-second-bounce)):
`Ballistic::max_speed_in` is where a measured window goes, and until
then a bot tries chains the game may not file.

### Monsters

A Monster Bounce launches over 16 units/s ([superbounce.md, "The
rule"](superbounce.md#the-rule)); its shape is lines converging on the
landing (superbounce.md, "Monster bounces", under [Single
creases](superbounce.md#single-creases)). A site is a Monster site
(`Site::monster`) when it is a V or a busy point, a slot or crease
scoring 0.85 or more (two or more creases converging on its line,
`spots::MONSTER_SCORE`), or its launch is over 16 (a verified best). A
bot weighs one up by +3 times its Monster pull, so a keen name goes for
them often and a cool one now and then. No approach of its own is
documented: a Monster site is flown as any other of its shape.

**The drill.** A sim match's `[drill]` ([sim.md, "The bounce
drill"](../runtime/sim.md#the-bounce-drill)) makes the practice try one
site only (`bots::drill`, `Mind::set_drill`): the known site within 0.5
of the drill's `site`. The bot walks to the drill's start (its `start`,
else the site's approach: a drop's takeoff or launch floor) as a plain
walk, and standing there unprimed asks the host for its word
(`Mind::drill_wants_arm`; the brain passes its unit's datum to
`drill::request_arm`), for 20 ticks before walking 1 unit off and back,
since a body that sleeps has no level pair to write. Primed, it runs the
route by the site from there (no prime: the walk to the approach, then
the jump or the drop), tallied as a practice try, pauses 10 ticks and
walks back to the start. The word is read between tries too
(`Mind::wants_pair`). A route that starts primed from a word read at −3
says so (`OrderRun::primed_read`: `primed_read`, and a practice event
`Primed` with `read` and not `own`). The log says once a drill which
site it found (`bots: bounce drill: site ... (a drop), aim ..., start
..., N sites known`), after any `no known site` said while the map's
spots were still coming in.

**Training.** A sim match's `[train]` ([bounce-search.md](bounce-search.md),
"Scheduling a batch") makes the practice fly the run's board instead
(`Mind::set_train`, from `learn::current`; `learn::shared` keeps the
board, built at the first pick from the known sites once the map's scan
is in, `practice::site_in`: each site's allowed `how`s and the route's own
knobs from `flown`, and its line of the learned file). A free bot takes
the board's pick (`learn::Board::pick`) and flies the round's slots one
after another, each as a drill try: the try's site is the known site
with the search's knobs over its own, the bot walks to its approach (a
drop's launch point), asks for the word as the drill does (the same
`drill_wants_arm` and `drill::request_arm`), and runs the route by the
site from there. One flight is a try: a landing that did not launch ends
the route (`try_over`). The try is the route's first flight
(`learn::Try::from_leap`, the miss from `Site::own_target`); a `died` is a
scored try with a death; an end that says nothing about the knobs is
void (`map_changed`, `practice_off`, `pair_lost`, `no_route`, a walk that
failed, and the training's own `no_site`, `no_launch_floor`, `no_path`
after 3 walks to the start, `not_armed` after 3 wake-ups). When the
search has no slot left the round folds (`Board::fold`), the log says
`bots: train: site (...) round N ...`, and the board's lines go to the
map's learned file (`spots::store_learned`). Each try logs its line with
`train` in place of `drill` and the knobs it flew appended (`; knobs
how=runoff ax=... back=...`, `practice::knobs_text`). `bots.bounce_status`
gives the round (`practice.train`).

**The try's line.** Every tallied try, drilled or practised, logs one
line (`mind::practice::try_line`):

```text
bots: bounce try: bot 0x2f drill site (-10.62, 3.88, -3.19) aim (-10.45, 3.99, -3.19): missed; jumps 3, the last: takeoff (-8.41, 2.49, -0.76) ground 1.21 vz +3.08 word -3; air 62 ticks stick mean (-0.05, +0.04) max 0.31 let go 20; down (-10.30, 4.11, -3.19) miss 0.204 in 5.38 out +0.00: missed; before: missed, too_slow
```

The bot (its seed), drill or practice, the site and its aim, what the
try came to, then the last flight: where it took off, the ground and
vertical speed, the word then (`unread` when it could not be read), the
air stick in world axes, the touchdown (origin the tick before, the
miss, the speeds in and out) and its result; then the results of the
flights before it. A try with no flight ends `no jump, ended at step N
(name)`. A fumbled try adds `; fumbled <how>` and a try that chained
`; chain M of N` (links made of tried). (The numbers above show the
shape, not a run.)

**The drill's drop at Ascension.** What lands in the slot there is a
running jump at full stick: off the upper level at about 2 units/s and
3.08 up, 60 ticks in the air, braked by the air stick to come down
about 0.5 units/s sideways at −5.4 near (−10.13 to −10.3, 3.85 to
4.07). A jump at half the speed (a walk, about 1.05 units/s at 0.6 to
0.85 of stick) comes down on the floor short of the slot at −5.26
whatever the air stick asks, since the stick can't push a flight past
about 1.2 units/s. A landing braked to a standstill over the crease
fell through it once. The numbers are in
[the bounce-practice notes](notes/2026-11-21-bot-bounce-sim.md), "The
drill four ways"; the route's plan for it is the drop above.

The bots spread over the sites by claims on a board the process shares
(`practice::CLAIMS`): a bot renews its claim each tick of its route, and
a claim lapses 300 ticks after it was last renewed. A route an agent
asks for (`bots.bounce_to`, `superbounce.verify`'s routes) runs as usual
and is not tallied. Turning the mode off ends a practice route
(`practice_off`). `bots.bounce_status` gives the tally while practising
(`practice`: the state, the site, attempts, bounces, and by site the
attempts, bounces and reasons; the state is `idle`, `route`,
`yielding` or `waiting`), so a game of practising bots is a live
verifier too.

Each bounce a practising bot makes is recorded by the game as a ghost
run, and the best run of each spot is kept in `<map>.ghosts` beside the
spots and verified files, proving the spot as it goes
([superbounce.md](superbounce.md#ghost-runs)). Nothing in the bots crate
records it: the game reads the bots' units each tick.

The practice also queues an event for each route it starts, the first
time the route's word reads −3, and each try it tallies
(`mind::PracticeEvent`: `Started`, then `Intent` with `monster`, the
chain's `links` planned, its `fumble` and whether it goes `again`;
`Primed` with `read` and `own`; `Chained` with the links `tried` and
`made`; `Tried` with `bounced` and why not), each wait for a player
(`Yielded` with the ticks `waited` and whether it `gave_up`) and each
step out of a player's way (`SteppedAside`); at most 64 between takes,
the oldest dropped. `Mind::take_practice_events` drains them; sim mode counts them
into its report ([sim.md, "Bounce practice"](../runtime/sim.md#bounce-practice)).

## Tuning the bounce

The bounce route's numbers are one struct, `bots::tuning::BounceTuning`,
and its built-in values one constant, `bots::tuning::BOUNCE_DEFAULT`
(`crates/live/bots/src/tuning.rs`): change a default there and every
bot plays it. A brain's `Tuning::bounce` (`Some`: a tuning file's
`[bounce]`, every field left out at the built-in value) replaces it for
that brain; `tuning::bounce()` gives the thinking brain's, held to what
the route can play (`BounceTuning::sane`: sticks 0 to 1, at least one
try and one jump tick, the turn at most 90°). A route reads it once,
when it starts.

| Field | Default | What it moves |
|---|---:|---|
| `approach` | 0.8 | the walk to a standing site ends this far behind it (units; `Bounce::from`) |
| `approach_speed` | 1.0 | the stick on that walk (`OrderRun`) |
| `settle` | 0.05 | within this of the site it stands (units) |
| `settle_ticks` | 90 | settling gives up after this long and jumps |
| `settle_gain`, `settle_min`, `settle_max` | 4.0, 0.12, 0.6 | the settling stick: gain a unit off, held between min and max |
| `still_ticks` | 4 | still this long before the jump |
| `jump_ticks` | 3 | A held this long for the jump (a standing site's, and a drop's running jump) |
| `drift` | 0.15 | the stick toward the drift in the air: the aim onto the crease (at 0.15, inside the move dead zone, the bounce-practice runs show no sideways speed at all) |
| `bounce_tries` | 8 | jumps at a site before giving it up |
| `crouch_first` | 6 | the prime: crouch first, standing still (ticks) |
| `crouch_ticks` | 12 | crouch held under the lip (walking and in place) |
| `release_ticks` | 6 | pushing in after letting go |
| `in_speed` | 0.6 | the stick in |
| `push_in` | 0.06 | how far past the stand it pushes (units) |
| `start_out` | 0.35 | how far out a try starts (units) |
| `approach_turn_deg` | 25 | the second and third tries' turn |
| `out_speed` | 1.0 | the stick back out |
| `line_up` | 0.15 | the drop: lined up on its run-up within this (units) ... |
| `line_up_ticks` | 45 | ... or after this long |
| `run_off_speed` | 0.6 | the stick of a drop's slower run, taken where a full-stick one overshoots (`drop_takeoff`) |
| `air_gain` | 1.2 | the drop's air steering: the change in sideways speed it asks for over the one that just lands it (`air_stick`) |
| `drop_tries` | 3 | drops before giving up |
| `fly_speed` | 1.0 | the stick toward the goal riding a launch |

A tuning file's `[bounce]` (`sim::config::BounceParams`, the same names,
each held to a range) sets them for a sim's bots; `h2 sim sweep` tries
values over maps and seeds and ranks them by bounce rate, then launch
height ([sim.md, "Sweeping the bounce tuning"](../runtime/sim.md#sweeping-the-bounce-tuning)).
A test in the game's `sim_glue` holds the file schema's defaults equal to
`BOUNCE_DEFAULT`, and one in `glitches` that the default tuning plays as
no tuning does.

## Driving a player

The same planner and route runner drive a seat with no brain of its own:
the local player, sent somewhere by an agent (`player.goto`,
docs/runtime/agent-api.md). There is one planner. `bounce_route` holds
the walk length and the launch floor (`path_length`, `launch_floor`) a
bot's orders plan with, the prime's two steps (`prime_steps`), a site's
approach (`approach`: behind it, or its launch floor) and the route by one
named site (`plan_site`: the crouch spot `plan_with` would pair with it,
proven first, no reach limit and no walk to beat; `why` is `asked`).
`goto::plan` picks from these by target and what to do there: a point is
a plain walk (`nav`); a crouch spot is the walk to its start, then its
recipe for `crouch_recipe`; a bounce site is the walk to its approach, its
route up to the approach for `crouch_recipe`, or its whole route
(`plan_site`, ending once down) for `bounce`.

The plan is played by `OrderRun` as a bot's would be, its last walk
ending within the asker's `within` (`OrderRun::arrive_within`).
`goto::Goto` adds a watchdog for a body the brain does not own:

- a walk that strays more than 1.5 units sideways from the leg it walks
  (from the waypoint passed, or where the walk began, to the next one;
  jumps, drops and lifts are left to the follower) plans again from where
  the body is (`strayed`);
- a walk whose distance left along its waypoints has not come down by
  0.5 units in 150 ticks plans again (`stuck`), except while the next
  waypoint is a lift, an elevator or a teleporter;
- a route the runner ends with `path_lost` (the follower's own stuck and
  fallen checks), `stalled` or `pair_lost` plans again, the word read
  again or, unread, the last run's priming kept;
- after 6 plans made again, or the timeout (3600 ticks unless asked),
  it gives up with the last reason (`strayed` reads `stuck`).

A jump is asked for as a fresh press: the pad never holds A two ticks
running. The game's glue (`player_goto`) reads `player.0`, steps the
`Goto` each tick on an agent ticker and holds the first controller's pad
with the motion, the walk turned into the stick against the player's
aim (the player does not turn while it walks). With no bot in the game
nothing has loaded the map's nav mesh (the bots' glue reads it only
while a phantom plays), so the glue asks for it the same way: the nav
worker's load (built when stale), else the `.bots` file in the cache
directory.

**Untested.** The pair-word read, the priming's timings, the air speed
and the whole route are not checked in a game; the pure planner and the
state machines are unit tests (`bounce_route/tests.rs`,
`glitches/tests.rs`, `mind/order/tests.rs`, `mind/practice/tests.rs`,
`goto/tests.rs`).

**Not built:**

- **Butterfly**: a two-player trick (one jumps on the other's head while
  the bottom player, sword in hand, presses X and B), not one bot's
  presses.
- **BXL, LX, RRBX**: less well documented.
- Super bounces off moving vehicles (Burial Mounds' Warthog launch).

One community note matters here. The Bungie.net post says BXR "works as
a reload glitch from any range, but only off host". Clankers play on the
host. Whether the cancels work for a host-side player is the first thing
to check.

**Tests.** `cargo test -p bots glitches` checks the requested timing,
the openings and setting, the show-off clock and super-bounce state
machine. `cargo test -p bots human_input -- --nocapture` checks the
delivered timing, delay, seeded replay and controller limits.
`cargo test -p boot clankers` covers the setting and driver.

**Game run (checklist).** For the user. Use one Clanker against you on
Legendary with Glitches on Lots. The agent API reads what happened:
`world.object player.N physics` (havok_pos, havok_velocity, airborne,
crouch), `havok.contacts {object: player.N}` and `trace.start player.N`
for a row every tick ([agent-api.md](../runtime/agent-api.md)). Each line
is a yes or no:

1. **Off.** Glitches Off: in five minutes no bot does any of the below.
2. **BXR.** Give the bot a battle rifle and let it get close. It melees
   and fires at once, with no melee recovery between. Your shields drop
   on the B and you die on the R. If the shot waits out the melee
   anyway, try `MELEE_CONNECT` 3 to 8; if it never cancels, note "host".
3. **BYYR and BXB.** The same with a part-empty magazine and a second
   gun (two switches, then the shot), and with an empty SMG and no
   reserve (two melees close together).
4. **Double and quad shot.** At mid range with a battle rifle: two bursts
   leave almost together (six rounds), then it reloads. With a second
   gun, two switches and two more bursts. Tune `BURST` if the second pull
   is ignored.
5. **Sword flying.** Give it a battle rifle in hand and the sword as the
   other weapon. Stand 5 to 15 units off. It switches and flies at you
   much further than the sword's own lunge, higher with the A. On Easy
   it mostly just switches.
6. **Crouch jump.** Calm on Lots: now and then a jump with the legs
   tucked at the top.
7. **Super bounce.** Ascension, calm on Lots, nobody near the ramp: the
   bot walks to the crease, jumps in place a few times, and one landing
   throws it up (trace: two opposing contacts against the fixed body on
   the launch tick, then +8 to +14 units a second up). With Some it gives
   up after three tries.
8. **Never.** It does none of these carrying the flag or the ball, with
   its shields down, or with a second enemy close by.
