# Sim mode: setting up a match

Status: Built. The playerless host and start, the match file, rules scripts, Firefight and bot parameters. Part of [Sim mode](sim.md).

## 5. Setting up a match by script

### The playerless System Link host

The headless host opens a System Link squad and adds each bot as a
phantom peer, with one player per peer. The host occupies one of the
sixteen peer slots and leaves all controller ports free, so a match
can have up to fifteen bots. File order gives the logical seat numbers
starting at zero.
Each phantom receives its own machine identity, marked name, appearance
and team, because the host has no local profile to copy.

In a `[[bot]]` entry, omit `kind` or set `kind = "phantom"`. A match
file with `kind = "local"` fails validation because controller ports
are reserved for people.

The host creates a System Link squad (`0x199df9(0,0)`, `0x199a03(2)`)
through retail's lobby setters for the map and variant (`0x19a6f2`,
`0x19a728`), and the lobby's readiness checks and machine load request
(`0x5a220`) stand in for the controller countdown. Each process
reserves its own loopback XNet port block. Headless runs reach the
regular phantom uplink and remote gather.

| Match field | How it reaches the game |
|---|---|
| `map` | the lobby's map row, resolved against the disc |
| `game_type` | the lobby's variant built from the type's built-in list index (`VARIANT_DEFAULTS`, `0x19d220`) |
| `time_limit_s`, `score_limit`, `team_play`, more `settings` | edits of the variant record with `variants::GameVariant::set` (setting 10 the time limit, the type's score setting, 91 team play, any other id) |
| `seed` | written to the engine's shared random seed `[0x004e7408]+0` at the first tick, to the game options' two spawn words (`[[0x004e6948]+0x10]`, `+0x14`; `sim_glue::spawn_words`) ahead of the game's first spawn, and folded into each brain's seed |

### The playerless start

**Code**: `sim_glue/hosted.rs` and
`host_lobby/retail.rs`. One step per game tick at the start of the tick:

1. **Create and configure.** All four local profiles stay signed out.
   With the menu and network session objects initialised, the host invokes
   the Available Games browser's Create row. `0x199df9(0,0)` creates a
   class-1 System Link squad via `0x59890(1,0,0)`. The host selects custom
   mode with `0x199a03(2)` and records the new squad (`SquadCreated`).
   The menu cannot retire this squad just because there is no controller
   to enter the pregame screen. The host waits for retail's pregame
   lifecycle (type 1 or 2), local host state 5 and stage 1
   (`SessionStarted`). It applies the requested map, variant and bot-count
   player limit. Their readback and distributed protocol 2 enable `Start`.
2. **Gate.** The squad must be live (state 5), hosted here, distributed,
   on the match's map, with one player per seat and one peer per machine:
   this host's own peer plus one per phantom (`start_gate`).
3. **Eligibility.** The lobby's own checks (`Eligibility::blocker`): the
   pregame phase, party leader, map, protocol, player limit, the variant
   open applied, and every peer's map status above 2. The last is the
   countdown finisher's test (`0x6e410` at `0x6e4b8`); retail's start
   check lets a status still loading through for the party leader, and
   the playerless path stands in for the finisher.
4. **Retail's start check** `0x19a2ce(0)` must answer 9.
5. **Start.** The controller countdown `0x19a78e` (EAX port 0) refuses on
   this host (port 0's local-user index is −1), so the host makes the
   finisher's own request: `0x5a220` with the session in ESI and stage 2
   in EDI (register arguments, plain `ret`, as `0x6e4d7` calls it). Session
   `+0x4980` becomes 2 and the machines load the map
   (`sim: System Link host Start -> Loading`).

Each change in wait reason emits `sim: System Link host waits at <step>: <reason>`.
A wait at `Start` can report `3 of 4 players` or a peer's map status.
If the squad retires or is replaced during setup, the host releases ownership
and returns to `Create`. If another network squad is still active, creation
reports that a network squad already exists.
The co-op hooks (`SESSION_GAME_LIMITS` "peer limit 2",
`SESSION_GAME_START` "one peer for the campaign check",
`LOBBY_START_STATE` "co-op over 2 to N consoles") act on campaign
sessions only and leave this lobby as retail has it (tested).

For four phantoms on Lockout the expected host transitions are
`Create -> SquadCreated`, `SquadCreated -> SessionStarted`,
`SessionStarted -> Start` and `Start -> Loading`. Phantom joins and
map-readiness waits can interleave with these lines. Map activation logs
`sim: match started at tick N`. The agent's `match.status.lobby` gives
the last setup step, wait reason, session type and squad state.

`match.status` enters `binding` on map activation. Combat sampling
starts once the distributed server has a player and a spawned unit for
every phantom machine. A run that cannot bind the roster writes its
diagnostic report and exits with an error.
Startup allows up to 180 wall-clock seconds, subject to a shorter
`max_wall_s`. Distributed runs boot a fresh session for each match.
Verify that `match.status` reaches `playing` with every logical seat in
`bound_seats` and that the match report lists just those players.

### Match file

TOML (or JSON), parsed by `sim::config::MatchConfig` (tested):

```toml
map = "lockout"
game_type = "slayer"        # slayer oddball juggernaut king ctf assault territories
time_limit_s = 300          # 0: none
score_limit = 25
team_play = false
seed = 7
max_ticks = 9000            # stop after this many game ticks (5 min at 30 Hz)
max_wall_s = 600            # and after this long on the clock
render_every = 0
pace = "unpaced"
components = true           # false: no state_components_v1 in the report (timing arms)
detail = "full"             # "training": no components; sparse digests and a rolling digest
                            # "spatial": the training report and the spatial trace beside it
bot_threads = 3             # bot think workers: a count, "auto" or "off"; unset: see below
bot_pipeline = false        # true: thinks overlap the guest's tick (a different match)

[settings]                  # any variant setting by id (variants::record)
83 = 1                      # weapons on map

[[bot]]
name = "A1"
skill = "legendary"
team = 0
params = "tuning/aggressive.toml"   # optional; relative to this file

[[bot]]
name = "B1"
skill = "legendary"
team = 1
[bot.tuning]                # or inline
aggression = 0.9
reaction = 5
```

A `[rules]` table makes the match play a rules script
([Rules scripts](#rules-scripts)). `[rules] script = "superbounce"` plays the
Superbounce script, which turns the superbounce mode on: the bots practise bounces instead of fighting
([Bounce practice](sim-bounce.md#bounce-practice)). A `[drill]` table (with
`[rules] script = "superbounce"`) makes them try one site only
([The bounce drill](sim-bounce.md#the-bounce-drill)). A `[train]` table (with
the Superbounce script, and no `[drill]`) makes the run one shard of a
bounce site training batch ([Training the bounce sites](sim-bounce.md#training-the-bounce-sites)). A
`[drops]` table (without the Superbounce script) drops every seat onto candidate
seams ([Seam drops](sim-bounce.md#seam-drops)). `game_type = "firefight"` with a
`[firefight]` table plays Firefight ([Firefight](#firefight)).

### Rules scripts

A `[rules]` table puts a script on the game's record at its start, the
way a lobby choosing that game type does
(`sim_rules::block_for`, then `game_variants::inject::set_sim_block`):

```toml
[rules]
script = "siege"                           # a built-in's key or name, any case, or "path/to/mode.rules"
options = { round_count = 2, phase_time = 60 }  # by name: a number, a bool, or an enum value's name
```

- **The script** is a built-in by key (`siege`, `gun_game`) or by name
  (`Gun Game`), or a path ending `.rules`, read relative to the match
  file. A file is compiled and verified as `h2 rules build` does (the
  tick budget included) and kept in the game-variant store's memory, where
  the runtime finds it by the record's hash prefix.
- **The options** pack as `h2 variants port --option` does
  (`variants::portable::record_for`); one left out takes its default.
  The match and the script still set the record's other fields
  (`score_limit`, `time_limit_s`, `team_play`).
- **Refused, by name, when the sim sets up the game:** a script that is
  neither built in nor a readable `.rules` file (the message lists the
  built-ins), one that does not compile or verify, an option the script
  does not declare (the message lists the declared ones), and a value out
  of range or an enum value the option does not have. `rules` with an
  empty `script`, or in a Firefight match, is refused when the file is
  read.
- **`script = "superbounce"`** (any case) is the bounce practice, with
  everything [Bounce practice](sim-bounce.md#bounce-practice) says.
- **The scores at each round** are in the report's `rounds`
  ([Rounds](#rounds)).

[sim-examples/siege-zanzibar-rounds.toml](sim-examples/siege-zanzibar-rounds.toml)
is two-round Siege on Zanzibar, two phantoms a side and a minute a phase.
The run makes Zanzibar's nav mesh in its data first when it is not there
("The nav mesh" in [Determinism](sim.md#8-determinism)):

```sh
h2 sim run docs/runtime/sim-examples/siege-zanzibar-rounds.toml
```

#### Rounds

The report's `rounds` lists the score of every player and every team at
the start of each round of the script's game, in round order
(`sim_bench::metrics::RoundScores`; absent without a script). Each entry
holds `round` (1 for the first), `tick` (the script's tick), `players`
(`slot`, the bot's `name`, `team`, `player_score`) and `teams` (`team`,
`score`). The runtime reads them from the game's statborg, the reads a
script's `player_score` and team score make, at the round reset and before
the script's own `on round_start` runs (`rules::runtime::Runtime::round_start`,
`sim_rounds`): the first round's at the script's start. The scores are
the game's so far, the earlier rounds' included: the engine's round reset
zeroes each row's round score and adds it into the row's earlier rounds'
score, and the reads add the two (`guest_layout::game::game_score`).

The report's `result` reads the scores as the game ended: a script's
last round's points and its `end_round` are made at the same tick
boundary, after the last tick's sample, so the sim takes the scores
again when the game's end returns (`sim_glue`, `on_end`).

### Firefight

`game_type = "firefight"` plays a Firefight match
([firefight.md](../game/firefight.md)) on a map that carries the kit:
a Slayer game with team play on and, unless the file sets one, no time
limit. The bots are the players.

```toml
map = "firefight-beavercreek"   # the kit package `h2 firefight kit beavercreek` installs
game_type = "firefight"

[firefight]                     # every key optional
set = "holdout"                 # holdout (default), attrition, last_stand, crossfire
difficulty = "normal"           # easy, normal (default), heroic, legendary
teams = 1                       # 1 or 2; unset: the set's own (Crossfire 2, the rest 1)
lives = 5                       # 1, 3, 5, 8, 12, 20 or 30; unset: the set's own
skulls = true                   # each round switches on its skull; unset: on

[[bot]]
team = 0
```

- **The map.** A disc map name resolves as for any match. Otherwise the
  name is an installed kit package, as the lobby's map select lists it:
  its package directory (`firefight-<map>`) or its list name
  (`Firefight – <map>`), and the lobby is opened on its map list slot
  (`sim_glue::kit_request`).
- **The seat rule.** Bots sit on the player teams only: team 0 with one
  team, 0 or 1 with two. The AI's team is never a seat, so any other
  `team` is refused by name (`bot 3: team 2: Firefight seats are on the
  player teams 0..1`). A bot with no `team` takes the player team with
  the fewest bots so far, the lower on a tie, after the named ones
  (`sim_bench::firefight::Firefight::balance`). Every bot on one team is
  no problem here.
- **The choices.** At the sim's start the table becomes the Firefight
  page's settings (`firefight.enabled`, `firefight.waveset`,
  `firefight.difficulty`, `firefight.lives`, `firefight.skulls`,
  `firefight.teams`; 0 for lives and teams is the set's own) in a
  settings store of the run's own, and the runtime reads them when the
  game starts, as it reads Game Setup's (`sim_glue::firefight_store`).
- **Refused with it:** `team_play = false`, `[rules]`, `[drill]`,
  `[train]`, `[drops]` and `[bounces]`; a `[firefight]` table without
  `game_type = "firefight"`.
- **The report** gains a `firefight` section
  (`sim_bench::firefight::FirefightReport`): the set, the teams, whether
  the director ended the match and with what `outcome` (`won`, `lost`,
  `team_won` with its `team`, or `draw`), `rounds_cleared`, each player
  team's `scores` and `lives_used`, from the runtime's result
  (`sim_glue::note_firefight`). A match a limit stopped first has
  `ended: false`, no outcome, and the player teams' scores as the game
  last showed them.

[sim-examples/firefight-beavercreek.toml](sim-examples/firefight-beavercreek.toml)
is Holdout with two Clankers on team 0;
[sim-examples/firefight-beavercreek-crossfire.toml](sim-examples/firefight-beavercreek-crossfire.toml)
is Crossfire with two Clankers on each team:

```
h2 firefight kit beavercreek
h2 sim run docs/runtime/sim-examples/firefight-beavercreek.toml
h2 sim run docs/runtime/sim-examples/firefight-beavercreek-crossfire.toml
```

A `firefight-<base>` map that is not in `<data>/maps` is built before the run:
a built-in kit (Coagulation, Zanzibar) by `firefight_kit::builtin::ensure`, any
other by `h2 firefight kit <base> --disc <disc> --data <data>` run as a child of
the same `h2` (`firefight_kits::ensure_map`). A build that fails stops the run,
naming the kit and the exit status. A package that does not stage
(`packages::staged_or_named`) stops it with its package, item and reason.

### Bot parameters from a file

`sim::config::BotParams` (TOML or JSON) holds what a bot is tuned by:

- `skill`: easy, normal, heroic, legendary (`bots::mind::Skill`);
- the aim table's numbers for that skill, each optional: `reaction`
  (ticks), `error` (radians), `lead`, `gain`, `fire_cone_deg`,
  `burst_on`/`burst_off` (ticks an automatic fires and rests past its
  gun's band; inside the band it is held; `burst = false` holds it at
  every range),
  `grenades_per_minute`;
- `awareness` (0..1; the decisions' sense of the fight, per skill now);
- the personality, each optional (0..1): `aggression`, `caution`,
  `teamwork`, `objective_focus` (otherwise derived from the gamertag);
- human input timing: `perception_ticks` and `motor_ticks` (0–30 ticks),
  `button_sigma_ms` (0–12 ms), `stick_step` (0.1–0.6 stick radii per
  tick), and `thumb_switch_ticks` (1–6 ticks);
- phantom transport: `phantom_ping_ms` (0–1000 ms round trip, default 0)
  and `phantom_jitter_ms` (0–50 ms bounded one-way variation for positive
  ping, default 4);
- host work intervals in game ticks (1–30 each): `foot_plan_ticks`
  (default 6), `vehicle_plan_ticks` (8) and `vehicle_lookahead_ticks` (3).
  Older files can still specify `player_los_ticks`. Player sight is
  sampled each tick. See [Bot navigation performance](sim-determinism.md#bot-navigation-performance)
  for the active intervals;
- `aim_model`: `code` (the hand-written tracking, the default) or `net`
  (the aim net; [bots.md §3.4.1](../game/bots.md#341-the-aim-net-compared-the-code-aim-stays));
- `[bounce]`: the bounce route's numbers, each optional
  (`sim::config::BounceParams`, held to a range each; the names and what
  they move are in [bots.md, "Tuning the bounce"](../game/bots.md#tuning-the-bounce)).

Anything left out keeps the built-in value. The bots crate takes them as
`bots::tuning::Tuning` through `Brain::with_tuning`. The aim table and awareness are
read through `Skill::aim()` and `awareness()`, which consult the tuning of
the brain that is thinking (a scoped override on the game thread, so the
call sites stay as they are); the personality replaces the seeded one,
and again after each new life's mind. So a sweep edits a file, not the
code, and never rebuilds the image or `h2`.
