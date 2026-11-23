# Firefight

Status: Partly built. The definition format, validator, three built-in wave
sets, director (FF2), stage-one offline kit, probe recipe, the host
runtime (FF4) with its `Game` over the guest bodies of the engine
entries and the `firefight` feature that runs it, the agent API
(FF5), the Game Setup row and options page, and the HUD's mode blob and
view (FF8, without the drawing) are implemented, tests only. "Guest boundary" lists the runs that
settle what the disassembly leaves open. Retail play and the network probes await verification.
Code: `crates/formats/firefight` defines and validates wave sets and holds
the director (`src/director.rs`) and kill scoring (`src/scoring.rs`).
`crates/tools/firefight-kit` builds the map kit. The offline command lives
in `crates/apps/cli/src/firefight_cmd.rs`. The host runtime is
`crates/extensions/game/src/firefight/` with the shared squad entry in
`crates/extensions/game/src/actor_net/squads.rs`; the agent API's
methods are `crates/extensions/agent/src/firefight.rs`. The HUD is
`firefight/hud.rs`, the Game Setup row and options page `firefight/options.rs`.
The slice table lists the planned Live code. Tests use invented data.
Switch: the `firefight` feature of `H2_FEATURES`, on by default ([switches.md](../runtime/switches.md)).

Firefight is a mode on multiplayer maps against waves of AI, with
lives, rounds, skulls and a score. One team of players holds out
together, or two teams each hold their own base against the AI and
against each other.
It runs on Live and on dedicated hosts in the distributed model with
host actors ([ai-networking.md](ai-networking.md)). The director is a
pure function of the match. Given the definition, the seed and what it
reads from the game each tick, it issues the same commands on every run,
so a film replays and a synchronous session agrees.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## Decisions

- **Kit map.** Every machine builds the kit map from its own disc using
  the same offline builder, including dedicated hosts.
- **Player count.** Firefight ships for 1 to 4 players. The 16-player
  playlist requires a passing CPU probe. [P5](ai-networking.md#probes)
- **Home base.** The kit gives every map two bases, read from the map's
  own data: the CTF flag stands, else each team's starting locations,
  else the starts split into two clusters. Base 0 is the home base (the
  players' base with one team, team 0's with two); the kit's `--home`
  option says which map team's base it is, red by default. Drops keep
  away from both bases and each has a path to both; players spawn at
  their team's base ([The kit](#the-kit)).
- **Two teams.** A definition has one or two player teams (`Def::teams`).
  The actors are a third team, hostile to both. With two, each team has
  its own pool of lives and its own score, players can kill each other,
  and squads are spread across both teams ([Two teams](#two-teams)).
- **Versus scoring.** A kill of a player on the other team scores the
  definition's fixed `player_kill` points for the killer's team (50 in
  the built-in sets), not scaled by difficulty or skulls. A team kill or
  a suicide scores nothing. AI kills score by class, as with one team.
- **The end with two teams.** A team out of lives loses to the other. A
  team that clears the last round's boss wave alongside the other wins on
  the higher score, then on more lives left; level on both is a draw.
  Both teams out in the same tick is decided the same way.

## What the player gets

- A Firefight mode in the custom game lobby ([the
  lobby](#the-lobby)), a **Firefight Options...** row in a custom game's
  Game Setup, and a Firefight entry in matchmaking (FF9): pick a map, a
  wave set, a difficulty, the starting lives and whether rounds switch on
  skulls.
- Waves arrive from the map's edges, away from the bases. Five waves
  make a round, three rounds a set; each round switches on another
  skull. A boss wave ends a round.
- Players share a pool of lives. A dead player respawns from the pool at
  the end of the wave when the pool is empty, and the match ends when
  everyone is dead with no lives left.
- Kills score points by enemy, difficulty and skulls. The scoreboard,
  the post-game report and the Live stats page show them.
- With two teams (the Crossfire set, or any set with Teams set to two),
  each team holds its own base with its own pool and score, the waves
  press both, and the players fight each other too. The scoreboard shows
  both teams' scores.
- Beaver Creek plays Firefight once its kit is built
  (`h2 firefight kit beavercreek`): one of its two bases is the
  players' home, or each team holds one.
- Clankers fill seats on the player teams, never the AI's: with one team
  they join the people on it; with two they fill either team, keeping
  them even, and fight the AI and the other team. They treat the actors
  as enemies through the same sight and aim as players, never target a
  teammate, hold near their team's home base (`ff_base_00` or
  `ff_base_01`), fall back to it when hurt, rearm near it between waves,
  never walk alone to a drop zone, and regroup at home when the team's
  lives run low ([bots.md](bots.md#310-firefight)). The runtime
  describes the match to them with `bots::firefight::set_context` and
  the lobby's team count with `set_seat_teams`.

## How it works

### The game engine underneath

A Firefight match is a retail **Slayer** game, on one team or, with two
Firefight teams, as a team game (team play on) with the players on
teams 0 and 1, and our variant extension naming the mode. The actors
are on a third team. That gives, with no new replication:

| Need | Retail part | Evidence |
|---|---|---|
| Scores on every machine's scoreboard and report | the statborg entity (type 7) | **E**, [notes/ai-networking.md](notes/ai-networking.md) |
| Lives per player | the game-engine player entity carries lives remaining | **E** (a field label in the image); **S** that Slayer's lives setting drives it |
| Rounds and the round timer | the game engine globals entity | **E** (field labels) |
| Respawn, weapons on the map, vehicles | the variant | **Code**, `crates/formats/variants` |

The host turns off what Slayer would do by itself: the score to win is
unreachable, team killing scores nothing, and the game ends only when
the director says so (a `Finish` command through the engine's own game
end). A player kill scores only through the director's `Score`
commands.

### The definition

`firefight::Def` is a versioned document. `Def::canonical_json` uses
`content`'s canonical form, as Forge does. `Def::id` is the SHA-256
of those UTF-8 bytes, including `format`. Object key order and input
whitespace can vary. Array order is part of the definition. Enemies are
named by **class** (`grunt`, `elite`, `jackal`, `hunter`, ...) and the
kit maps each class to a character on the machine.

```
Def      { format: 2, name, teams, rounds: [Round], lives: Lives, caps: Caps, scoring: Scoring }
Round    { waves: [Wave], skulls_on: [Skull] }
Wave     { squads: [SquadSpec], boss: bool, gap_ticks }
SquadSpec{ class, count: {per_player, min, max}, drop: Drop, weapon_tier }
Lives    { pool_start, per_round_bonus, shared: bool }
Caps     { live_actors_by_players: [(players, actors)] }
Scoring  { by_class, player_kill, difficulty_multiplier, skull_multipliers, assist }
```

`Def::from_bytes(bytes, &Limits)` admits wire data. It checks the 64 KiB
limit before parsing, bounds collections as it reads them, and refuses
unknown fields and duplicate object keys. `validate(&Def, &Limits)`
applies the same format and admission rules to an authored definition.
`Limits` supplies the target kit's class list. Every named class must be
in that list and every squad class must have a score entry. Canonical
output and hashes depend on the definition alone.

- `teams` is 1 or 2. `player_kill` is 0 to 1,000 points and must be 0
  with one team. Two teams need a cap tier for at least two players.
  With `shared` lives each team has its own pool of `pool_start`;
  otherwise each player has their own.
- A definition has 1 to 12 rounds, each with five waves and a boss on the
  last wave. A wave has 1 to 16 squads and a gap of up to 9,000 ticks.
  A squad has 1 to 32 actors. Its count at each player count is
  `players * per_player` clamped to `min..=max`. Starting lives and
  the round bonus each range from 0 to 1,000.
- Cap tiers use ascending inclusive upper bounds on player count from
  1 to 16. The last tier sets the largest supported lobby. Admission
  checks the sum of each wave's squads at every supported player count
  against the author cap. The author cap stays within the host budget
  in [AI networking](ai-networking.md#cpu-on-the-host).
- `"edge"` lets the director choose among the kit's edge zones.
  `{"prefer_zone":n}` prefers zone `n` (0 through 11) when present
  and unseen. Weapon tiers range from 0 to 3.
- A round's `skulls_on` adds skulls to those already enabled. Each skull
  appears at most once in the schedule. The JSON names are
  `assassins`, `thunderstorm`, `famine`, `blind`, `ghost`, `catch`,
  `mythic`, `angry` and `low_gravity`.
- Class scores range from 1 to 1,000 points. The four difficulty
  multipliers (`easy`, `normal`, `heroic`, `legendary`) and the optional
  multiplier for each skull are integers in thousandths, from 1 to
  2,000. An omitted skull multiplier is 1,000. The assist fraction is
  0 to 1,000 thousandths.
- The name is nonblank text of at most 128 UTF-8 bytes without control
  characters. Class names start with a lowercase ASCII letter and use
  lowercase ASCII letters, digits or underscores, up to 32 bytes. A
  scoring table holds up to 32 classes.

`firefight::builtin::definitions()` returns Holdout, Attrition and Last
Stand for one team, then Crossfire for two (a player kill scores 50).
Each has three rounds and uses the stage-one kit's Elite class.
`builtin::with_teams(def, n)` plays a set with `n` teams: with two, a
player kill scores the set's own points or `builtin::PLAYER_KILL_POINTS`
(50); with one, nothing.

### The director

`firefight::Director` (`crates/formats/firefight/src/director.rs`) is a
state machine with no I/O:

```rust
pub fn new(def: Def, settings: Settings) -> Result<Director>;   // Settings { seed, difficulty, skulls }
pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command>;
pub struct Observed { players: Vec<PlayerSeen { slot, alive, team }>,
                      actors_alive_by_squad: [u16; 16],
                      zones: Vec<ZoneSeen { seen, distance, team_distance: [u32; 2] }>,
                      kills: Vec<Kill { class, killer, assist }>,
                      player_kills: Vec<PlayerKill { killer, victim }> }
pub enum Command { PlaceSquad { slot, class, count, zone, weapon_tier }, Target { slot, team },
                   Hunt { slot }, EraseSquad { slot }, Round { index }, Wave { index },
                   Skull { id, on }, Lives { player, pool }, TeamLives { team, pool },
                   Score { player, points, reason }, Respawn { player }, Finish { outcome } }
pub enum Outcome { Won, Lost, TeamWon { team }, Draw }
```

- `Director::new` refuses a definition that is not structurally valid.
  Its generator (SplitMix64) starts from the match seed XORed with the
  first eight bytes of `Def::id`. The director reads no clock, uses no
  floating point and iterates only ordered collections, so the same
  definition, settings and observed sequence give the same commands.
- The host fills `Observed` after each game tick. `kills` are the tick's
  `SimEvent::ActorKilled` events ([ai-networking.md](ai-networking.md),
  slice AN6) with the character mapped to its kit class, the credited
  player as `killer`, and the second player who damaged the actor in its
  last five seconds as `assist`. `distance` is each zone's distance from
  the players' centre in any fixed integer unit, and `team_distance` from
  each Firefight team's players' centre (`u32::MAX` for a team with no
  unit). A player's `team` is its game team; `player_kills` are the
  tick's players killed by players.
- States (`Phase`): `Warmup` until the first wave's gap has passed,
  `Spawning` while the wave has squads in the reserve, `Fighting` once
  all are placed, `Cleanup`, `Intermission` for the next wave's gap, and
  `Done`. `Round` (with the round's new `Skull` commands) and `Wave` are
  issued when a wave starts. A wave ends when no actor is alive.
- **Cleanup.** Once some of the wave has died and at most three actors
  are alive, each remaining squad gets `Hunt` (it is sent at the
  players). Sixty seconds later what is left gets `EraseSquad` and the
  wave ends.
- **The reserve and the cap.** A wave's squads are placed in definition
  order, at most two a step, while the next one fits: live actors plus
  its count stay at or under the cap for the current player count, and
  a squad slot is free. The rest wait in the reserve and enter as others
  die. The player count and the cap are clamped to the definition's
  tiers, and each squad's count is fixed when it is placed. A placed
  squad counts as its full size until the host reports it, then as the
  lowest count reported. An unreported squad is treated as gone after
  three seconds. An erased slot is reused only once the host reports it
  empty or three seconds pass.
- **Where a squad enters.** The kit gives each map drop zones. The
  director honours an unseen preferred zone when the kit has it.
  Otherwise it takes the zones no player has line of sight to (all zones
  when every one is in sight), orders them farthest first from the
  players' centre, and picks with its generator among the farther half.
  A step with no zones places nothing.
- **Lives.** See "Lives, rounds and skulls" below. When everyone present
  is dead with no respawn coming, the director issues `Finish { lost }`.
  Clearing the last round's boss wave issues `Finish { won }`. With two
  teams the end is in [Two teams](#two-teams).
- Every command is a host log entry (`host_log::Entry<Command>`,
  ai-networking.md, "How it works"). Commands serialize to integer-only
  JSON, so the log's canonical form applies. The runtime applies entries
  at the next tick in sequence order; the director never touches the
  game.
- Its state (`director::State`: phase, round, wave, the reserve's next
  squad, slots with each squad's target, each team's pool and score,
  players with their team, active skulls, generator) is the mode blob
  the actor channel sends as `State`. `Director::resume` continues from
  it.

#### Two teams

With `Def::teams` 2 the director keeps a pool and a score per team
(`State::pools`, `State::scores`) and the actors press both teams.
With one team none of this applies: no `Target` or `TeamLives` is
issued, every squad's target is team 0, and the commands are those of
a one-team match.

- **Teams.** A player's Firefight team is fixed the tick the director
  first sees it: game team 0 is team 0, every other game team is team 1
  (`director::firefight_team`). A team's score is the sum of its
  players' `Score` commands; a kill credited to a slot the director has
  not seen counts for team 0.
- **Lives.** With shared lives the start announces each team's pool
  (`TeamLives`), a respawn spends from the player's team's pool, and the
  round bonus goes to both. Own lives work as with one team.
- **Player kills.** A `PlayerKill` whose killer and victim are on
  different teams issues `Score { reason: player_kill }` with the
  definition's `player_kill` points, after the tick's actor kills. A
  suicide, a team kill, a kill with no killer, or `player_kill` 0 scores
  nothing.
- **Targets.** Each placed squad is sent at a team, and `Target { slot,
  team }` follows its `PlaceSquad`: the only team present; else the team
  fewer live actors are sent at; else, level, even squads of the wave at
  team 0 and odd at team 1. Its cleanup `Hunt` goes to its target's base
  (the kit's `ff_hunt_NN` order).
- **Where it drops.** The zone choice narrows the unseen zones (all of
  them when every one is in sight) to those at least as near the target
  team as the other (when any is), ranks them by the target team's
  distance, farthest first, and picks among the farther half as with
  one team. Squads land on their target's side, out of sight and away
  from its players.
- **The end.** A team is out when it has players present and none alive
  or with a respawn coming. With both teams present, one out issues
  `Finish { team_won }` for the other; both out in one tick is decided.
  A lone team present that is out has lost (`Finish { lost }`).
  Clearing the last round: a lone team present wins; otherwise decided.
  **Decided** is the higher score, then more lives left (the team's
  pool, or its players' own lives), then `Finish { draw }`.

### The director as a rules script

`firefight::script` (`crates/formats/firefight/src/script.rs`) compiles
a definition to a rules blob that does what `Director` does, and runs it
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)). The
host runtime still runs the Rust director; the script is checked against
it in tests.

- `compile(&def)` builds the IR directly. The squads, waves, rounds
  (each round's skull schedule), actor caps, class points, difficulty
  and skull multipliers are constant tables. The director's state is
  global and per-player script variables, and the loops over the 16
  squad slots are unrolled. The blob depends only on the definition, so
  its hash is stable. A definition with more than 256 squads is refused
  by name, since a rules table holds 256 rows.
- The difficulty and the skulls on from the start are script options
  (`script::options`). The VM seeds its generator from the match seed
  and the blob's hash; `script::match_seed` hands it a seed that starts
  it where the director's generator starts, and `random(n)` draws as the
  director does (SplitMix64, modulo `n`).
- Kill points are formed exactly: the class points, the difficulty
  multiplier and nine skull multipliers (1,000 for a skull that is off)
  are multiplied in base-1,000 limbs and divided by 1,000^10 once.
- `ScriptDirector::step` is the host side. It begins the VM's tick, fires
  `on init` once, `on join` for each player present for the first time,
  `on actor_kill` and `on actor_assist` for each kill, then the per-tick
  group, and flushes the queue. Every director command is a queued call
  (`place_squad_at`, `hunt`, `erase_squad` and the `ff_*` entries), so
  the log keeps the director's order. Zones are the objects labelled
  `ff_drop`, with placement id equal to the kit zone index.
- `ScriptDirector::state` reads `director::State` back from the script's
  variables and the VM generator. `ScriptDirector::resume` continues
  from `RulesExt`.

- Two teams run in the script too. Each team's pool and score are
  globals (`pool_0`, `pool_1`, `score_0`, `score_1`), the player's team
  a per-player variable `on join` fixes from `team_of`, and a slot's
  target sits in its info word (times 256, above the count). `on kill`
  scores player kills (only in a two-team blob with `player_kill` above
  0). The entries `zone_team_distance(z, team)`, `ff_target(s, team)`,
  `ff_team_lives(team, n)`, `ff_player_score(p, n)` and
  `ff_team_finish(winner)` (`none` a draw) stand for the two-team parts
  of `Observed` and `Command`. The blob's header has `teams` 1 or 2.

The parity tests (`crates/formats/firefight/tests/script.rs`) replay each
scenario of the director's tests through both and compare commands and
state tick by tick: the golden match, the empty pool, the lost match,
unshared lives, assists, kill points at the multiplier limits, zone
choice, 48 random matches over the quick built-in sets at every player
count (Crossfire among them, with player kills), the built-in sets as
authored, starting skulls, unshared lives, zero gaps, zones all in sight
and ties, duplicate and absent players, a resumed state, and two teams:
24 random two-team matches with shared and own lives, every two-team
end, player kills of each kind with a team change after first sight,
and targets with zones on each side.

Limits of the script against the director:

- A worst-tick estimate that fits. The zone choice ranks the zones with
  a nested `for each` over the labelled set, and the verifier bounds a
  labelled set at 256 members, so `Report::fits` is false. The VM's run
  time budget (20,000 ops a tick) holds for the kit's 6 to 12 zones.
- Player slots of 16 and over, which the VM addresses as `none`: the
  director would still score and register them.
- Zone distances over 2^31 - 1, which the script reads clamped.
- A tick with more than 128 commands, past the VM's queue: a script
  error, where the director has no limit.

### The host runtime

`game::firefight::host::Host` (`crates/extensions/game/src/firefight/host.rs`)
runs one match on one machine over a `Game`: the running game as the
runtime reads and changes it, one method per engine entry. The guest
bodies of those entries are in `game::firefight::guest` ("Guest
boundary"). `game::firefight::runtime::GuestGame` is the `Game` over
them, and the `firefight` feature (`game::firefight::feature`) runs the
runtime at the game's start and at each tick boundary. The tests drive
`Host` over a fake game that records every call, and the runtime over
invented guest memory with fake engine callees.

**The feature.** `firefight` is on by default (`-firefight` turns it off). Boot registers it
(`registration::register_core`) and its hooks are on game-thread points
only:

- an observer after a successful `GAME_ACTIVATE` starts the mode in a
  multiplayer game (options mode 2) through `Host::start`. Outside one
  the runtime is idle; on a map without the kit the refusal is kept with
  its reason, "the map does not carry the Firefight kit", for
  `firefight.state`. Until the session data carries them (FF9) the
  definition is Holdout, the seed 0 and the difficulty Normal.
- a `before` on `GAME_TICK`, in the default order class and inside the
  co-op lobby setups' and our skulls' (`coop-probe`, `coop-live`,
  `coop-screens`, `skulls-custom`), runs `Host::pre_tick`. Memory reads
  and writes happen at once; engine calls are kept with their entry's
  sequence number. When there are calls, or a sight pass is due, the
  hook answers with an async body (`runtime::run_work`) that makes them
  in order, runs the sight pass, and then runs the tick itself, whose
  chain lets the hook pass. A call the engine refuses is reported with
  its sequence number in `firefight.state`.
- an observer after `GAME_TICK` runs `Host::post_tick` over the guest,
  the tick's `SimEvent`s and the last sight pass.

The machine's role is `Replica` in a film's playback, else by the
simulation world's mode (docs/live/live-coop.md): `Replica` for a
synchronous client (3), `Client` for a distributed client (5), and
`Host` otherwise. No transport carries the host's entries to a replica
yet, so a replica applies none. A tick boundary's number is the game
time's tick count plus one before the tick and the count after it, so
the director's entries for the next tick are due at the next boundary.
A snapshot is refused while a match runs.

| Role | Runs the director | Applies entries |
|---|---|---|
| `Host`: the distributed server, or the host of a synchronous session | yes | yes |
| `Replica`: a synchronous peer, or a film's playback | no | the entries it receives |
| `Client`: a distributed client | no | none; it keeps the log for the digest |

1. **At map load**, `Host::start` refuses the mode on a map without the
   kit, on a kit with no drop zones, and for a definition that names a
   class the kit lacks (`firefight::validate` against the kit's classes).
   A two-team definition is refused on a variant without team play, when
   team play cannot be read (`runtime::teams_refusal`), and on a kit with
   fewer bases than teams (`Refusal::Bases`). The actors take the kit's
   team unless a player holds it, else the lowest team no player holds;
   with two player teams that is a third team. The host log starts at a genesis hash of
   the definition's id and the match seed.
2. **The AI switch.** At the first tick boundary after the AI globals are
   initialised, a machine that applies entries sets the globals' on byte
   if it is off, and never writes it again
   (`game::firefight::guest::switch_ai_on`, `[0x4f55d0]+0`). Probe P2
   ([ai-networking.md](ai-networking.md#probes)) says whether a
   distributed host needs the write; the check makes the runtime correct
   either way.
3. **Before each game tick** (`Host::pre_tick`), every entry due by the
   tick is applied once, in sequence order. An entry the engine refuses is
   reported with its sequence number and is not retried. `PlaceSquad`,
   `Hunt` and `EraseSquad` go through the squad entry
   (`actor_net::squads::SquadEntry`, shared with Forge). A placement
   carries the kit's character slot for the class, the count, the zone and
   the actors' team; a class or zone the kit lacks, a slot outside the 16
   or a count outside 1 to 32 is an error and no call. `Score` credits the
   killer through the score entry; a player kill carries no class.
   `Lives`, `Respawn` and `Skull` have an entry each. `Target` records the
   slot's team, and a slot's `Hunt` goes to that team's base order
   (`SquadOp::Hunt { slot, base }`, the kit's `ff_hunt_NN`). The engine
   keeps no pool, one team's or two, so `Lives` and `TeamLives` set the
   director's (`Director::set_lives`, `Director::set_team_pool`; the
   director's own set the count it holds, an operator's changes it), and
   the director spends a team's respawns from its pool and holds them at
   0, through the same `Respawn` entry as one team; the HUD reads the
   pools from the mode blob. `Finish` ends the game through the engine's
   own game end.
   `Round` and `Wave` change nothing in the game: the HUD reads them from
   the mode blob.
4. **After each game tick** (`Host::post_tick`, the host only), `Observed`
   is built from the game: each player's roster slot, game team and
   whether it is alive, live actors by kit squad slot, each zone's
   visibility and its distance in hundredths of a world unit from the
   mean position of the players who have a unit (and from each Firefight
   team's players), the tick's actor kills, and the tick's players killed
   by players from the sim events (`runtime::player_kills`). A kill's character
   is mapped to its kit class; a kill of a character the kit does not
   name scores nothing and is left out. A player is alive while its unit's
   body is not depleted. A zone is seen when some player's unit has line
   of sight to its point in the last sight pass, which runs every 15
   ticks on the host while the director is stepped. The director's commands are
   appended as host log entries at the next tick, so clients and films
   replay them.
5. **Received entries** (`Host::receive`) are checked against the chain.
   The host refuses them: it writes its own log.
6. **The Clankers** are told of the match (`runtime::bot_context`,
   `bots::firefight::set_context`): the player teams, the actors' team,
   the bases, each team's lives and whether a wave is on. The runtime
   sends it at the start and whenever it changes, and clears it when the
   match is done.

### The kit

A multiplayer map has no characters, no squads and no sectors (**Code**,
[pathfinding.md](../formats/pathfinding.md); bots.md 2.4). The kit adds
them offline, on each machine, from that machine's own disc:
`firefight-kit` takes a base map and writes a map package
([maps.md](maps.md)). The base map hash and kit version yield the same
package id on every machine with matching inputs. Session data names
that id, and each machine loads its own build.

| Part | Built with | Notes |
|---|---|---|
| Sectors in each `sbsp` | `pathfind::generate` | the generator the bots' mesh starts from, which has run on every retail multiplayer map (**Code**, `crates/live/bots/src/cache.rs`); the engine has not yet loaded its output (**S**) |
| Drop zones | the bots' nav mesh (`bots::autonav`): sectors near the map's edge, off the players' spawn clusters, with a path to the middle | 6 to 12 a map; written as zones and areas |
| Zones, areas, firing positions | sampled from the sectors: cover is a position with a wall toward the middle | the block writers of `h1campaign::ai` (`ScenarioBlocks`) |
| Squad slots | 16 empty squads the director fills (class, count and zone are set at place time) | the same writers |
| Characters, stage 1 | `char` tags we author from a tuning table of ours, on bipeds and weapons the multiplayer map already has | Elites only; no tag is copied from another map |
| Characters, stage 2 | `tags::schema::transplant` from the player's own campaign maps, with the sound gestalt merge | each class costs tag headroom; `validate` refuses a class that does not fit and the builder reports the cost |

Stage 1 runs against a map on the configured disc or extracted disc tree:

```sh
h2 firefight kit coagulation
h2 firefight kit beavercreek
h2 firefight kit beavercreek --home blue --replace
h2 firefight kit coagulation --probe
```

The first command installs `<data>/maps/firefight-coagulation`. `--home
red|blue` (`firefight_kit::Config { home }`, red by default) says which
map team's base is the home base. `--probe` builds the `firefight-probe`
recipe, which puts two Elites in one squad and adds a startup script
that calls `ai_place` for it. The regular kit keeps its sixteen squads
empty. The [test-map API](test-maps.md) can also create and list the
probe on a running host. There is no separate list of Firefight maps:
the lobby offers every installed kit package, so building a map's kit
offers it.

**Bases.** `firefight-kit/src/bases.rs` finds two bases from the
scenario: the mean of each team's CTF flag stands (netgame flags at
`scnr+0x118`, type 0, team at `+0x12`, teams 0 and 1); else the mean of
each team's starting locations (`scnr+0x100`, team designator at
`+0x10`); else all starts split into two clusters seeded at the pair
farthest apart. Base 0 is the home base, base 1 the other. Each starting
location's team designator is rewritten to its nearer base (a tie goes
home), so team 0 spawns at the home base and team 1 at the other; the
rows are rewritten in place, or as a new block at `scnr+0x100` (costed
as `starts`) when they are not in the scenario's own bytes.

`firefight-kit/src/nav.rs` generates pathfinding for every structure BSP.
It chooses 6–12 boundary sectors on the walkable component that holds
both bases, so every drop has a path to each base over shared sector
edges with enough clearance for the bots' walk mesh. A drop's centre
stays at least 4 units from every start and a quarter of the bases'
separation (2 to 8 units) from each base; picks alternate between the
bases' sides, spread farthest first. Each zone has an area and separated
firing positions, preferring cover that faces the nearer base. After the
drops come `ff_base_00` and `ff_base_01`, each one area with up to 8
firing positions around its base, and the orders `ff_hunt_00` and
`ff_hunt_01`, whose primary areas are that base's area only, so a squad
hunted at team N pushes into base N. The kit refuses maps without six
suitable drops or without the required local tags. `KIT_VERSION` is 2.

The pinned map writer keeps existing datums, BSP load addresses and raw
resources in place. The scenario and each BSP point to their new arrays
in ordinary tag memory. The package has one deterministic file patch,
bound to the base hash. It carries the kit version in the map's string
table, so the standard package `MapId` includes it through the patch hash.
The cost report counts each part's bytes and compares the final growth
in tag memory (including the index and page rounding) with the available
space before anything is installed.

Tag headroom on multiplayer maps is 0.1 to 1.5 MiB and one map has none
(**S**, forge-design.md, "The kit"). The kit builder prints what each
part costs, and a map with no room for stage 1 is not offered.

Every machine in a session loads the kit map: the host's entities name
tags by index, so the indices must agree (ai-networking.md, "Rules and
limits").

### Scoring and stats

- `Scoring` gives points by class; the multiplier is difficulty times
  the product of the active skulls' multipliers
  (`firefight::scoring::kill_points`). The product is formed exactly in
  thousandths and rounded down once. A skull without an entry counts
  1,000. A kill with no credited player, or of a class without a score,
  scores nothing.
- An assist earns the `assist` fraction of the kill's points, rounded
  down, on top of the killer's points (`ScoreReason::Assist`).
- With two teams, a kill of a player on the other team earns the
  definition's `player_kill` points (`ScoreReason::PlayerKill`), with no
  multiplier. A team's score is the sum of its players' points.
- In the game: the statborg holds each player's score, so the scoreboard
  and the carnage report need nothing new.
- In the stats stream: the host's `Host::post_tick` pairs each `Score`
  command with the class of the kill it was for (`Credit`) and, on
  `Finish`, makes the result (`Ended`: the set's name, the outcome,
  rounds cleared, respawns issued). The runtime reports each
  ([`runtime::Report`]) to Live telemetry, which writes two public game
  events the server takes only from the game's host
  (`live_proto::firefight`): `FirefightScore` (`tick`, `player` slot,
  `points`, `reason` `kill`, `assist` or `player_kill`, `class` on a
  kill) and `FirefightEnded` (`tick`, `set`, `outcome` `won`, `lost`,
  `team_won` or `draw`, `teams`, `team` on `team_won`, `scores` one per
  team, `rounds_cleared`, `lives_used`). `won` needs one team,
  `team_won` and `draw` two. In the film the tracker turns each `FirefightScore`
  into a `live_proto::film::events::Event::FirefightScore` and the
  player's running total into an `ObjectiveScore` with
  `game_type: "firefight"`. `ActorKilled` stays its own event.
- The Live projection (`live-server`'s `stats`, which the Worker
  composes, so both homes give the same answer) folds them into the
  summary's `firefight` section: finished matches and wins, rounds
  cleared, lives used, points, matches by wave set, kills by class, and
  a row per account (score, kills, assists, player kills, matches).
  `won` counts one-team wins against the AI; two-team matches also
  count in a `versus` row (games, wins by team, draws), where a two-team
  `lost` counts in games only. A bot has no row; its points, kills and
  player kills count in the totals and classes. A second
  `FirefightEnded` for one game counts nothing. The stats home shows the
  section once a match has been played in the window.
- Firefight scores do not enter ranks: a game that carries either event
  is never ranked (`ranks::rank_game`). Bots stay game-only, as today.
  A Firefight match is not multiplayer's kills or deaths.

### Lives, rounds and skulls

- **Lives.** The pool is the director's. A player's death takes one life
  from the pool at respawn (`Respawn`, then `Lives` with the new pool);
  with the pool at zero the player waits for the wave to end and
  respawns then without a charge. Between waves an empty pool does not
  hold a respawn. `Lives.shared = false` gives each player their own
  count instead, announced when the player first appears. The engine's
  own respawn stays off: the variant's lives setting is 1, so a death
  leaves the player with none and the engine holds the respawn, and
  `Respawn` gives back the one life ("Guest boundary").
- **Rounds.** Five waves, the last a boss wave; an intermission between
  rounds refills ammunition crates (a kit placement the director
  re-enables) and adds `per_round_bonus` lives.
- **Skulls.** Two kinds ([skulls.md](skulls.md), "Where each effect
  lives"). AI-scoped skulls (Mythic, Angry, Catch, Thunderstorm, Ghost,
  Assassins) change what actors do, and only the host runs actors, so
  only the host applies them: the reader's campaign gate is answered as
  "on" for a Firefight host. Player-scoped skulls reach every machine by
  the path multiplayer skulls already use (`live_proto::skulls`, the
  session data). The round schedule is in the definition.
  `skulls_mp::firefight_scope` names where each takes effect;
  `firefight_ai` gives the set a machine runs (every AI skull on a
  host or a lone machine, none on a guest), `firefight_shared` the
  player-scoped set that joins the multiplayer one, and
  `MpSkulls::write_firefight_array` writes the host's AI set into the
  engine's skull array. What remains is the campaign gate itself: each AI
  reader tests the options mode inline, so answering it "on" needs a
  hook per reader, which the AI-skull rows of [skulls.md](skulls.md#in-firefight)
  list.

### Game Setup

A custom game's Game Setup gets a **Firefight Options...** row after the
Clankers row (`firefight/options.rs`, `ROW_ACTION` 13; the row is added
the way [bots.md](bots.md)'s Clanker Options row is, to every list that
has Change Map). In the lobby's Firefight mode the row follows Change
Map, and in Forge mode it is hidden ([The lobby](#the-lobby)). It opens
the Firefight page, a kit screen on Quick
Options' dialog with six choice rows, each backed by a setting
(`firefight.*`, in `setting_defs::ALL`): Firefight (on or off), Teams
(`firefight.teams`: the wave set's own, one team or two), Wave set (the
four built-in definitions, Crossfire the fourth), Difficulty, Starting
lives (the wave set's own, or 1 to 30) and Skulls (whether each round
switches its skull on). A on a row opens the value picker over the page; left and right step
as a shortcut. No row is an arrow row.

`Options::read` reads the store when a game starts and the runtime plays
that match: `Options::definition` is the wave set with the chosen starting
pool, played with the chosen teams (`builtin::with_teams`) and, with
skulls off, no round skull; the difficulty goes to the
director's settings, where it scales kill points. Firefight off in Game
Setup leaves the mode idle with that reason in `firefight.state`. A
choice the store cannot give refuses the mode with the setting named. The
page and its row exist only where the `firefight` feature is on and a
settings store is present.

The choices are the host's. A client of a distributed session does not read
its own page: the definition and difficulty reach it in the session data
(FF9), and a definition other than a built-in one is not offered by the
page.

### The lobby

Firefight is a mode of the party lobby
([party-lobby.md](party-lobby.md), [forge-firefight-ui.md](forge-firefight-ui.md#the-lobbies)),
in the GAME MODE picker and on the Xbox Live menu's Firefight row while
the `firefight` feature is on and a settings store holds the choices,
whatever maps are installed. Switching to it sets the page's
Firefight choice on and puts the lobby on a kit map; Game Setup then
lists the Firefight rows, and the variant panel shows the wave set, the
difficulty and the starting lives in place of Quick Options' lines (the
first reads "{set} waves, 2 teams" with two teams). The lobby does not
switch team play on for two teams; a variant without it is refused with
its reason.

Clankers sit only on player teams (`clankers::firefight_seat_teams`,
which tells the bots the team count with
`bots::firefight::set_seat_teams` twice a second, nothing outside
Firefight): with one team every Clanker is on team 0; with two each new
Clanker joins the team with fewer players, team 0 on a tie. The AI's
team is never a seat.
With no kit map installed the mode still opens and the lobby keeps its
map: the panel says "No installed map supports Firefight yet" and "Make
one with h2 firefight kit", and Start Game is held with "No installed
map supports Firefight yet." on the status line
(`lobby_mode::start_refusal`). A Firefight lobby on a map without the
kit while one is installed is held the same way, with "This map does
not support Firefight."

| Row | A does |
|---|---|
| **Change Map** | Opens the map select's Firefight category ([map-select.md](map-select.md)): the installed maps that carry the kit. The catalogue lists a kit package as `Firefight – <map>` (what `firefight-kit` names it, `lobby_start::is_kit_map`); the probe's `Firefight Probe – <map>` is not offered. With none installed the retail map list opens, and the log says why. |
| **Firefight Options...** | Opens the Firefight page ([Game Setup](#game-setup)), the row after Change Map. |
| **Quick Options**, **Clanker Options** | As in a Multiplayer lobby. |

The game type row is hidden: the runtime starts the mode from the page's
choices on a kit map ([The host runtime](#the-host-runtime)), and the
game itself starts as the lobby's Start Game starts any custom game.

### The HUD

The HUD shows the round and wave, a banner for the stage, the enemies left
and the lives (`firefight/hud.rs`). All of it is a function of the **mode
blob**, `hud::Mode`: the stage (warmup, spawning, fighting, cleanup,
intermission, won, lost, team 0 won, team 1 won, draw), the round and
wave with their totals, whether the wave is a boss wave, the actors
alive plus those still in the reserve while a wave is on, the team
count, and the lives (the shared pool, each player's own up to eight
players, or each team's pool). With two teams the blob carries both
scores and a mask of team 1's slots, so each viewer sees its own team's
pool. Version 2 encodes to 13 bytes with one shared pool, 36 with eight
players' own lives, 25 with two team pools and 46 with two teams and
eight players' own lives, inside the actor channel's 63-byte mode
state. The banners for the end are "RED TEAM WINS", "BLUE TEAM WINS"
and "DRAW"; a two-team score line, "RED a BLUE b", stays up after the
end.

- **The host** builds the blob after each tick from the director and its
  log (`Mode::of`; the outcome is the last `Finish` in the log) and
  publishes it (`hud::slot`). The channel's capture sends it as a `State`
  record when it changes and whole in every key
  (`actor_net::host::Host::set_mode`).
- **A distributed client** keeps the mode state it receives
  (`actor_net::client::Client::mode_state`) and publishes it
  (`hud::received`) at each tick boundary.
- **Either machine** draws `hud::current(local_slot)`: the blob, decoded
  and viewed by `view`, a pure function of the bytes and the roster slot.
  A blob the build cannot read (wrong version, wrong length, a field out
  of range) is an error, not a guess. `the_hud_reads_only_the_mode_blob`
  plays a whole match and requires the client's view to equal the host's
  at every tick.

The layout is one fixed list of six elements (banner, round, wave,
enemies, lives, and the score, hidden with one team) in the HUD kit's terms (the kit's one layout per HUD, with its `Element`,
`Place` and `Values`), and `hud::check` runs its rules over
every state: the same elements in each, each text within its element's
widest, HUD copy in capitals, digits and single spaces, no two elements
overlapping and none off the screen in the whole, half and quarter screen
modes.

The kit's runtime is what draws an element on Halo 2's own HUD, and it is
not built, so the view is published and checked but not drawn. When it is,
`hud::LAYOUT` and `hud::view` register as the kit's `Hud` and `Values`,
and the placements take the kit's retail rectangles. Replicas and films
have no director and so no blob until the host log carries the mode state
(AN7).

### Live

- **Variant.** The Slayer record names the mode in the variant
  extension's reserved bytes ([game-options.md](game-options.md), "The
  extension"), in the layout of
  [rules-engine.md](rules-engine.md#carrying-a-script): mode 1 and the
  8-byte prefix of the definition's hash. The definition itself travels
  in the session data beside the extension (`NewSession.firefight`), in
  both homes.
- **Clankers.** Live's roster names no team for a bot
  (`live-server/src/bots.rs`, `roster_bots`); the host seats each one
  on a player team by the Firefight seat rule, never on the AI's
  (`firefight_sessions_seat_clankers_on_player_teams_only`).
- **Hoppers.** A Firefight hopper entry identifies a kit map and the
  Firefight variant. `hopper-edit` offers the built-in definitions. FF9
  ships the 1-to-4-player playlist. FF13 adds the 16-player playlist
  once P5 meets the host target at the chosen actor cap.
- **Dedicated host.** A Firefight match asks for a host of its own class
  with a whole core (ai-networking.md, "CPU on the host"). The host
  builds the kit map from its own disc. The server-map strip keeps the
  AI blocks and sectors.
- **Join in progress** follows ai-networking.md: lobby only until probe
  P6 answers.

### Theater

The host's film replays the match, actors included, and holds every
director command as a recorded log entry (ai-networking.md, "Theater").
The Theater HUD reads round, wave, lives and score from the recorded
mode blob.

## Interfaces

### Agent API

| Method | Access | Purpose |
|---|---|---|
| `firefight.state` | read | whether the mode runs and why not, the role, round, wave, phase, reserve, pool, live actors against the cap, the log head, and the entries the engine refused |
| `firefight.command` | act | append one director command to the log on a host, or hold or run the director; refused on a client and in playback |
| `firefight.kit` | read | whether the loaded map carries the kit, its slots, zones, classes and hunt order |

[agent-api.md](../runtime/agent-api.md#firefight) has the params and
results.

### Guest boundary

The entries behind `Game`. Each goes through a routine the engine
already has, named in `guest-layout`'s `points.rs`, and has a body in
`game::firefight::guest` (`crates/extensions/game/src/firefight/guest.rs`):
a read or write of guest memory, or an `async` body over `engine::entry::Cx`
that makes the guest calls from inside a guest entry at the tick
boundary. Every address below is **E** unless the row says otherwise.
`game::firefight::runtime::GuestGame` is the `Game` over these bodies.

| Entry | Points | Body | State | Evidence |
|---|---|---|---|---|
| AI on | `AI_GLOBALS` `+0` | `switch_ai_on` | built | the script switch 278 stores its boolean there (`0x2a4d31`, [ai-control.md](../halo1/ai-control.md)) |
| Place a squad with a count | `AI_SQUAD_PLACE` (`0x201520`), behind `HS_AI_PLACE_COUNTED` (291) | `place_squad` | traced; run R1 | Evaluator 291 resolves its two arguments, keeps the `ai` value only for a plain squad (top two bits clear, `0x2a5045`), and calls `0x201520` with the count in EAX and four stack words: the squad index, start -1, 0 and 1 (`0x2a5052`); it then stores the `ai` value at AI globals `+0x364`. The one-argument overload (290) reaches the same routine through `0x273480` with the count -1, which takes the squad's normal or legendary count by difficulty (`0x2015c5`). The routine returns at once while AI globals `+1` is clear (`0x20152e`). Start -1 lets `AI_CHOOSE_SQUAD_START` pick each actor's start, and the pool of starts refills when it runs out (`0x203ac5`), so a count over the squad's starts reuses them. The third word, a byte, skips the given start's availability check and the vehicle-seat attempt; the fourth goes to `AI_SPAWN_AT_SQUAD_START` as its flags |
| ... its character and zone | scenario squad `+0x36` and `+0x38`, its starts' positions | `place_squad` | traced | Nothing chooses them at the call. `AI_SPAWN_AT_SQUAD_START` reads, for each actor, the start's character (`+0x20`) or else the squad's (`+0x36`), as an index into the character palette (scenario `+0x178`, `0x2037cc`); a squad with neither places nobody. The squad's areas while it has no order come from its zone `+0x38` (`0x204f93`), and its actors appear at its starts' positions. So `place_squad` writes the palette index, the zone, and the zone's firing positions into the slot squad's starts, then places: tag memory every applying machine writes the same way. The weapon tier is not applied, since the stage-one kit authors one weapon |
| Actors' team | `AI_SET_TEAM` (`0x275b60`), behind `HS_AI_SET_TEAM` (322) | `place_squad`, after the placement | traced | The evaluator passes the `ai` value in EAX and the team in BX (`0x2a587a`). The routine accepts a squad or a group, writes the runtime squad's team byte (`+0x76`, `0x275bf4`), then each actor's team (`+0x24`, which `world.ai` reports) and its unit's (`+0x138`, `0x275c99`). Placement copies a nonzero squad team byte onto each new unit (`0x20187a`), but team 0 reads as unset there, so the body sets the team after every placement |
| Hunt | `AI_SET_ORDERS` (`0x275cb0`), behind `HS_AI_SET_ORDERS` (333) | `hunt` | built | The evaluator passes the `ai` value in EAX and the order index in CX (`0x2a5c39`). The routine ignores an index outside the scenario's order block (`+0x240`, `0x275ce6`) and hands a squad to `0x202e90`, a group to `0x203120`. There is no built-in hunt: it is an order the kit authors, which `hunt` finds by its name `ff_hunt`. The kit writes it as order 0 of an empty block (`ai::hunt_order`): no style, script or follow target, and every area of every drop zone as its primary area set, so a squad sent to it engages from across the arena. A base map that already has orders is refused, since the kit does not merge into another block's child arrays. A kit with no zone areas is refused with the Hunt order named |
| Erase a squad | `AI_ERASE` (`0x2736c0`), behind `HS_AI_ERASE` (297) | `erase_squad` | traced; run R2 for bodies | `ai_erase` passes the `ai` value as its one stack argument (`0x2a523b`). For each actor the routine deletes the actor (`0x1e2a90`) and then its unit (`OBJECT_DELETE`) or swarm (`0x28e200`); an actor the AI is updating at that moment is marked instead (`+0x0d`, `0x2736fc`). It then walks the objects the squad placed (runtime squad `+0x70`, linked through object `+0x3a4`) and deletes each one `0xbeb30` does not keep. Whether a dead actor's body is still on that list is what R2 settles |
| Credit a player's score | `SCORE_ADD` (`0x15b7c0`) | `credit_score` | traced | Slayer's kill callback is its engine's vtable slot `+0x78` (engine 2 at `0x55e4d8`), which `0x15ce70` calls last (`0x15d6a4`); it calls `0x15b7c0` with the score change in EAX and the killer's player datum on the stack (`0x2bc05d`). The routine adds the change's low 16 bits, signed, to row `+0x00` at `+0x308 + 0x1c * slot` of the game engine globals, clamps to ±30,000 (`0x15b849`), marks the statborg entity's player bit for replication (`0x15b88a`), updates the team score in a team game, checks the score limit when it is nonzero, and returns true. It refuses, returning false, without a game engine, while engine `+0x6c` is not 1, and on a machine where engine `+0xc04` is not 1 (unless game globals `+0x0c` is 4). `credit_score` passes the salted player datum and treats false as an error |
| Lives and respawn | `PLAYER_OUT_OF_LIVES` (`0x15db30`), `PLAYER_RESPAWN_READY` (`0x15dd40`), `PLAYER_ENTITY_DIRTY` (`0xa7840`), `PLAYER_SPAWN` (`0x14f2e0`) | `engine_holds_respawns`, `respawn`; `Lives` writes nothing | traced; run R3 | No routine sets lives: the engine writes player `+0x1ac` itself. Round start sets it to the variant's lives setting (game globals `+0x1b8`, 0 meaning unlimited, `0x157d03`), and a death takes one on the authority (`0x15d01e`, gated by `GAME_ENGINE_AUTHORITATIVE`), then marks the player entity's bit `0x20` for clients (`0x15d03d`; a client applies it at `0x24e257`). The players update spawns a waiting player through `PLAYER_SPAWN` only when `PLAYER_RESPAWN_READY` says so (`0x14d430`), which refuses a player `PLAYER_OUT_OF_LIVES` reports: a positive lives setting, no unit and no lives left. So the hold is the variant's lives setting at 1, and `respawn` gives one life and marks bit `0x20`; the engine respawns the player when its respawn delay runs out and takes the life at the next death. The pool stays in the mode blob, so `Lives` changes nothing in the game. A team whose players are all out of lives leaves the playing-teams mask (`0x1925c6`); whether that ends the Slayer round before the director's `Finish` is part of R3 |
| End the game | `GAME_END` (`0x1389c0`, no arguments, `ret`) | `end_game` | traced; run R4 | The routine latches game globals `+0x1128` once, sets the end delay at `+0x112c` from the tick rate and clears `+0x1129`; it checks nothing else. The engine's own round-over path ends a game in three steps (`0x15b5f9` to `0x15b624`): engine state `+0xc04` to 2, `GAME_END` unless latched, then the engine's changed flag (`+0`, bit `0x20`), skipping the first two while game globals `+0x0c` is 4. A client ends when the engine globals entity carries bit 4 (`0x24e52a`). `end_game` takes the same three steps and refuses in mode 4; whether a client ends with it is R4 |
| Line of sight from a player to a point | `AI_LINE_OF_SIGHT` (`0x29dd50`) | `line_of_sight`, from the unit's origin | traced; the eye point is open | The routine takes the two points in ESI and EDI and two objects to ignore in EDX and ECX, and runs the collision query `0x1697c0` with flags `0x1808c2d`: structure, instanced geometry and objects, the set the actor code passes at `0x1c1ab1`, `0x1f970e` and `0x292747` ([sim-rewrite-collision.md](../engine/sim-rewrite-collision.md) 5.4 for the bits). It is true when nothing is hit, or the hit is within about a third of a world unit of the target (`0x29de01`). The engine's own sight cone, `0xcc2b0` behind `objects_can_see_flag` (113), tests only the angle from a marker of the viewer it looks up through `0xb8d30` (**S**: the head). `sees` should start from the same marker; that lookup is not traced, so the sight pass starts from the unit's origin and ignores the unit itself |
| The kit on the loaded map | scenario blocks at `+0x160` squads, `+0x168` zones, `+0x178` character palette, `+0x240` orders | `read_kit`, `KitLayout::kit` | built | Squads named `ff_slot_00` to `ff_slot_15` give the slots and the team (`+0x24`); zones named `ff_drop_NN` give each zone's point, its first area's mean (`+0x24` of area 0), and its firing positions; the palette's row count bounds the classes. The offsets are the kit builder's (`h1campaign::ai::h2::layout`), and the engine reads them at the addresses cited above. Stage one maps its one class, `elite`, to palette row 0. The `firefight_kit_v1` string ID is not checked: the game has no reader for the loaded map's string IDs |

#### What a run settles

Each run is on the `firefight-probe` map (`h2 agent maps create
firefight-probe`, then `h2 agent maps load firefight-probe`) with
the default features. R1 needs nothing new. R2 to R4 hold the
director first (`h2 agent firefight.command '{"director":"hold"}'`), so
only the run's own entries reach the game, and append them with
`firefight.command`. R3 reads each player's lives and score from
`sim.view`'s `player` root (`lives`, player `+0x1ac`, null when -1;
`score`, stat row `+0x00`).

| Run | Command | Expected |
|---|---|---|
| R1 | `h2 agent world.ai` after load | two actors with `squad` 0, the Elite's character datum and `team` 3: the probe's startup `ai_place` reached `AI_SQUAD_PLACE` and the character came from squad `+0x36` |
| R2 | after one Elite is killed, `h2 agent firefight.command '{"command":{"erase_squad":{"slot":0}}}'`, then `h2 agent world.ai` and `h2 agent sim.objects '{"kinds":["biped"]}'` | no actors; whether the dead Elite's biped is gone says if bodies are on the squad's list |
| R3 | with the variant's lives at 1, kill player 0, read `h2 agent sim.view '{"roots":["player"]}'`, then `h2 agent firefight.command '{"command":{"respawn":{"player":0}}}'` | the player stays dead with `lives` 0 until the command, then respawns after its respawn delay with `lives` 0 again after the next death; with every player dead the round does not end before `Finish` |
| R4 | `h2 agent firefight.command '{"command":{"finish":{"outcome":"won"}}}'` on a host with one client | both machines show the post-game screen with the scores the statborg held (`score` in `sim.view` on each) |

Kills come from `SimEvent::ActorKilled` (AN6), which is filled only while
the object update runs in Rust (`impl.sim.native.objects`). The killer's
player datum maps to its roster slot, and the kill's character tag to
its row in the scenario's character palette. The assist is the latest
other player in the victim's `SimEvent::Damage` history over the last
five seconds (`runtime::Assists`).

## Rules and limits

- One or two player teams; the actors are always a third team. A team is
  0 or 1; a definition for more teams is refused.
- Clankers take player-team seats only: team 0 with one team, teams 0
  and 1 balanced with two. The AI's team is never a seat.
- The live actor cap is the definition's, bounded by the host table.
- The kit admits classes that fit within the map's tag headroom.
  Creatures use the [creature entity](ai-networking.md#creature-entity)
  in distributed sessions.
- A machine without the kit map cannot join; the session says which map
  to build.
- The director never reads a wall clock and draws only from its own
  generator.

## Slices

Lanes write code and tests on invented data and run nothing. AN numbers
are slices of [ai-networking.md](ai-networking.md); P numbers its probes.

| # | Slice | Size | Needs | Files | Named tests | Acceptance |
|---|---|---|---|---|---|---|
| **FF1** | `firefight`: `Def`, canonical form and hash, `validate`, three built-in definitions | S | nothing | `crates/formats/firefight/src/{def,validate,builtin}.rs` | `golden_hash_for_the_builtin_defs`, `a_wave_over_the_cap_is_refused`, `an_unknown_class_is_refused`, `canonical_form_is_stable_under_key_order` | every validator rule has a refusing case; built-ins validate |
| **FF2** | The director: states, the reserve and cap, zone choice, lives, scoring, commands | M | FF1, AN1 | `crates/formats/firefight/src/{director,scoring}.rs` | `a_scripted_match_issues_the_golden_command_list`, `live_actors_never_exceed_the_cap`, `no_zone_in_sight_is_chosen_while_another_exists`, `same_seed_same_commands`, `the_pool_at_zero_holds_respawns_until_the_wave_ends`, `everyone_dead_and_no_lives_finishes_lost`, `a_replay_of_the_observed_sequence_issues_the_same_commands`, `a_resumed_mode_blob_issues_the_same_commands` | 1,000 seeded matches against a scripted `Observed` never break the cap and always end |
| **FF3** | The kit, stage 1: sectors, drop zones, zones and firing positions, squad slots, authored Elite characters, the package and its id; the `firefight-probe` test map recipe (one squad of two) | L | nothing | `crates/tools/firefight-kit/`, `crates/apps/cli/` (`h2 firefight kit`), the test-map recipe list ([test-maps.md](test-maps.md)) | `two_builds_of_one_map_are_byte_equal`, `squad_blocks_read_back_through_h1campaign`, `every_drop_zone_has_a_path_to_the_middle`, `a_map_without_headroom_is_refused_with_the_cost`, `the_probe_recipe_places_one_squad`, `the_kit_authors_one_named_hunt_order_over_every_area`, `hunt_spans_every_zone_and_refuses_an_arena_without_areas` | on invented maps the kit's blocks read back; the coordinator's build on a retail map and probe P2 go in notes |
| **FF4** | The host runtime over the `Game` interface: the AI switch, team setup, `Observed`, applying commands; then the squad, score and game-end entries behind it ("Guest boundary" gives each one's state); the `Game` over the guest bodies and the `firefight` feature at the tick boundary | M | FF2, FF3, AN6, P2 | `crates/extensions/game/src/firefight/{host,guest,runtime,feature}.rs`, `crates/extensions/game/src/actor_net/squads.rs`, `points.rs`, `crates/apps/boot/src/registration.rs` | `place_calls_the_squad_entry_with_class_and_count`, `a_client_applies_no_command`, `score_credits_the_killer`, `entries_apply_at_their_tick_in_seq_order`, `a_map_without_the_kit_refuses_the_mode`, `a_scripted_match_makes_the_expected_engine_calls_in_order`, `a_map_without_the_kit_is_refused_with_its_reason`, `an_operator_command_is_the_engine_call_before_the_next_tick`, `a_kit_map_starts_the_host_switches_ai_on_and_looks_at_each_zone`, `the_plan_builds_with_every_hook_switch_on` | over a fake game, a scripted match produces the expected engine calls in order; over invented guest memory, the runtime makes them through the guest bodies before the tick; boot's plan builds with the feature on |
| **FF5** | Agent API: `firefight.state`, `firefight.command`, `firefight.kit`; `lives` and `score` on `sim.view`'s `player` root | S | FF4 | `crates/extensions/agent/src/firefight.rs`, `crates/extensions/agent-proto/src/firefight.rs`, `crates/extensions/agent/src/sim/state.rs`, [agent-api.md](../runtime/agent-api.md#firefight) | `state_reports_the_director`, `command_is_refused_on_a_client_and_in_playback`, `kit_reads_the_loaded_scenario`, `the_view_reports_each_players_lives`, `the_view_reports_each_players_score` | the methods answer over a fake game |
| **FF6** | Stats: `Score` and `ActorKilled` to film events, the Live projection's Firefight summary, the web stats page | M | FF2, AN6 | `crates/live/live-proto/src/{film/events,firefight}.rs`, `crates/live/live-server/`, `crates/live/live-worker/`, `web/` | `a_scripted_match_projects_to_the_golden_summary`, `the_firefight_summary_is_the_golden_one_and_players_cannot_write_it`, `the_firefight_summary_is_the_same_on_the_worker`, `firefight_scores_do_not_touch_ranks`, `a_two_team_match_projects_wins_by_team`, `credits_pair_each_kill_with_its_class_and_assist`, `a_firefight_score_is_a_game_event_and_a_running_objective_score_in_the_film`, `firefight_scores_become_running_objective_scores`, `reads the server's golden summary as it is` (web) | both homes give the same summary |
| **FF7** | Skulls: the host-only AI set, the round schedule, multipliers | S | FF4 | `crates/extensions/game/src/skulls_mp.rs`, `crates/formats/firefight/tests/skulls.rs` | `ai_skulls_apply_on_the_host_only`, `a_round_switches_its_skulls_on`, `multipliers_multiply` | skulls.md's table gains a Firefight column, each cell with a test |
| **FF8** | HUD and screens: wave banner, enemies left, lives, the Game Setup row and its options page | M | FF2, AN4, AN5 | `crates/extensions/game/src/firefight/{hud,options}.rs`, `crates/extensions/game/src/ui_kit/` | `Page` tables for the options page (`the_options_page_steps_and_opens_pickers`); `the_hud_reads_only_the_mode_blob`; the kit's layout checks in every state (`every_state_passes_the_layout_checks`) | a client with only the mode blob draws the same HUD as the host |
| **FF9** | Live: the variant bytes, `NewSession.firefight`, 1-to-4-player hopper entries, the lobby's local kit check | M | FF1 | `crates/formats/variants/src/ext.rs`, `crates/live/live-proto/`, `crates/live/live-server/`, `crates/live/live-worker/`, `crates/tools/hopper-edit/` | `the_mode_bytes_round_trip_and_leave_the_crc_valid`, `a_joiner_without_the_kit_map_is_refused_before_load`, `first_playlist_accepts_one_to_four_players`, `an_oversize_def_is_refused`, the parity suite | both homes offer 1 to 4 players and refuse unsupported sizes and missing local kits. A retail variant is unchanged |
| **FF10** | Dedicated host: the Firefight host class and admission, the kit in the host's map set, the strip keeping AI data | S | FF3, FF4, P5 | `crates/apps/h2host/`, `crates/live/host-data/`, [dedicated-host.md](../live/dedicated-host.md) | `a_firefight_match_takes_a_whole_slot_class`, `strip_keeps_squads_and_sectors` | admission tests; the coordinator's host run in notes |
| **FF11** | `h2 sim run` Firefight matches: Clankers as the players, on one team or balanced across two (the `[firefight]` table: `set`, `difficulty`, `teams`, `lives`, `skulls`; examples `docs/runtime/sim-examples/firefight-beavercreek.toml` and `firefight-beavercreek-crossfire.toml`), a report with rounds, score and tick times. `host.metrics` reports `game_cpu_ms_per_tick_p99` over the last 30 steady gameplay seconds from per-tick game-thread CPU deltas | M | FF4 | `crates/live/sim-bench/`, `crates/extensions/game/src/host_metrics.rs`, [sim.md](../runtime/sim.md), [agent-api.md](../runtime/agent-api.md#hostmetrics) | `a_firefight_match_toml_parses`, `firefight_bots_are_on_player_teams_only`, `two_team_firefight_balances_omitted_teams`, `the_report_has_rounds_and_score`, `a_firefight_match_finds_its_kit_package`, `a_firefight_table_becomes_the_pages_choices`, `game_cpu_p99_uses_individual_tick_deltas` | a match file runs headless (coordinator) and feeds P5. The p99 comes from individual gameplay samples and resets between matches |
| **FF12** | The kit, stage 2: transplanted classes, their cost, the cut order | L | FF3, P2 | `crates/tools/firefight-kit/src/transplant.rs` (new) | `a_class_over_the_headroom_is_refused_with_its_cost`, `a_transplanted_closure_resolves`, `two_machines_build_equal_ids` | on invented maps; the retail census goes in notes |
| **FF13** | The 16-player playlist at the live-actor cap measured by the CPU probe | S | FF9, FF10, FF11, P5 passes at 16 seats | `crates/tools/hopper-edit/`, `crates/live/live-server/`, `crates/live/live-worker/` | `sixteen_player_playlist_requires_the_measured_host_class`, `sixteen_player_definition_honours_the_measured_actor_cap`, the parity suite | publish in both Live homes only with a P5 result at or below 28 ms p99 game-thread CPU at the chosen live-actor cap on the intended host class. Admission uses that class and cap |
| **FF14** | Home bases in the kit: bases from flag stands, team starts or clusters; drops away from both with a path to each; firing positions facing the nearer base; `ff_base_NN` zones and `ff_hunt_NN` orders; starts rewritten to their base's team; `--home`; Beaver Creek's kit | M | FF3 | `crates/tools/firefight-kit/src/{bases,nav,ai,lib}.rs`, `crates/apps/cli/src/firefight_cmd.rs`, `crates/extensions/game/src/firefight/guest.rs` | `home_base_is_read_from_team_starting_points`, `home_base_prefers_the_ctf_flag_stand`, `the_home_option_swaps_the_bases`, `bases_split_untagged_starts_into_two_clusters`, `starting_locations_take_their_base_team`, `drops_are_chosen_away_from_the_home_base`, `every_drop_zone_has_a_path_to_each_base`, `firing_positions_face_the_nearer_base`, `each_base_has_a_hunt_order_over_its_area`, `the_kit_writes_each_start_its_base_team`, `read_kit_finds_the_bases_and_hunt_orders` | on invented maps the bases, drops and orders read back; the coordinator's Beaver Creek build goes in notes |
| **FF15** | Two player teams: `Def::teams` and `player_kill` (format 2), validation, Crossfire, per-team pools and scores, targets, team-side zones, two-team ends, the rules-script form and its entries | M | FF2 | `crates/formats/firefight/src/{def,validate,builtin,director,script}.rs`, `crates/formats/rules/src/{caps.rs,lang/sig.rs}`, `crates/formats/firefight/tests/teams.rs` | `two_teams_start_with_a_pool_each_and_spend_their_own`, `a_team_out_of_lives_loses_to_the_other`, `both_teams_out_at_once_go_to_the_higher_score`, `both_teams_out_level_on_score_and_lives_is_a_draw`, `the_last_round_goes_to_the_higher_score_then_lives_then_a_draw`, `player_kills_score_across_the_teams_only`, `squads_are_sent_at_both_teams`, `the_team_fewer_actors_press_gets_the_next_squad`, `a_squad_drops_on_its_target_teams_side`, `a_players_team_is_fixed_when_first_seen`, `two_team_matches_always_end_with_a_team_result`, `team_counts_and_player_kill_points_are_refused_out_of_range`, `two_team_matches_give_the_same_commands`, `two_team_ends_give_the_same_commands`, `a_team_pool_set_from_outside_is_the_one_its_respawns_spend`, `pools_set_from_outside_are_refused_where_the_match_keeps_none` | the director and the script give the same commands over two-team scenarios; every two-team match ends with a team result |
| **FF16** | Clankers in Firefight: player-team seats only, balanced with two teams; actors as enemies; home-base hold, fall back, rearm, no lone drop-zone trips, regroup on low lives | M | FF4, FF15 | `crates/live/bots/src/firefight.rs` (new), hooks in `crates/live/bots/src/{lib,mind,mind/strategy,perceive,phantom/session}.rs`, [bots.md](bots.md#310-firefight) | `firefight_seats_are_player_teams_only`, `actors_are_seen_as_targets`, `teammates_are_never_targeted`, `two_team_bots_target_the_other_team_and_actors`, `bots_hold_near_their_home_base`, `bots_regroup_when_the_pool_runs_low`, `bots_do_not_wander_to_a_drop_zone_alone`, `actors_are_read_from_the_pool` | on invented perception, bots never take the AI's team and never target a teammate |
| **FF17** | Two teams in the game: the host's team checks and third AI team, `Observed` per team and player kills, `Target`/`TeamLives`/hunt by base, HUD blob v2 with both scores, the Teams row, Clanker seats, the bots' context, the agent API's two-team state and commands | M | FF4, FF5, FF8, FF15, FF16 | `crates/extensions/game/src/firefight/{host,runtime,hud,options}.rs`, `crates/extensions/game/src/{clankers,lobby_mode,actor_net/squads}.rs`, `crates/extensions/agent-proto/src/firefight.rs`, [agent-api.md](../runtime/agent-api.md#firefight) | `observed_carries_each_players_team_and_player_kills`, `zone_team_distance_is_from_each_teams_centre`, `hunt_goes_to_the_targets_base`, `the_actors_take_a_third_team_with_two_player_teams`, `two_teams_need_a_base_each`, `two_teams_need_team_play_and_a_base_each`, `the_clankers_are_told_the_teams_bases_and_lives`, `the_end_report_carries_each_teams_score_and_the_winner`, `two_teams_show_both_scores_and_each_viewer_its_teams_lives`, `the_teams_choice_reaches_the_definition`, `clankers_never_take_the_ai_team`, `two_team_firefight_balances_clankers`, `state_reports_each_teams_pool_and_score`, `command_takes_the_two_team_commands`, `an_operator_team_pool_is_the_one_the_director_spends` | over a fake game a two-team match makes the expected calls; a client's HUD equals the host's in a two-team match |

FF1 and FF3 start at once, beside AN0, AN1, AN2 and AN6. FF2 follows
FF1 and AN1. FF9's format half can run beside FF2. FF4 is the join
point: it needs the kit, the director and probe P2.

## Evidence

- **E** The statborg, player and engine-globals entity types and their
  field labels: [notes/ai-networking.md](notes/ai-networking.md).
- **Code** The scenario AI block writers:
  `crates/tools/h1campaign/src/ai/convert.rs`. Sector generation:
  `crates/formats/pathfind/src/generate.rs`. Transplant:
  `crates/formats/tags/src/schema/transplant.rs`. The variant
  extension: `crates/formats/variants/src/ext.rs`.
- **S** Every row marked so above.

## Not verified

- That the engine accepts generated sectors and added `char` tags on a
  multiplayer map, and that actors placed there fight.
- That actors and players on different teams damage each other under
  Slayer, and what a player killed by an actor is credited as.
- That Slayer's lives and rounds can be driven from outside as described.
- Tag headroom per map with the kit applied.
- Everything ai-networking.md lists.
- That actors on a third team fight both player teams of a team game,
  and that a squad's `ff_hunt_NN` order makes it push into that base.
- That the engine spawns each team at the starting locations whose team
  designator the kit rewrote, and what team a CTF flag's `+0x12` field
  names on retail maps.
- That Beaver Creek yields six drops away from both bases.
- Whether team 1 reads as blue in the game, as the HUD's "BLUE" assumes.
- That the sim finds the kit package's scenario path and the bots load
  Beaver Creek's nav mesh on it.
- That the game's actor positions, as the Clankers read them, match what
  a client sees, and the bots' home and regroup radii in real matches.

## Related

- [ai-networking.md](ai-networking.md): the network model, budgets and probes.
- [forge-design.md](forge-design.md): the host log's other user; a Forge wave graph and the director issue the same squad command.
- [skulls.md](skulls.md), [game-options.md](game-options.md), [bots.md](bots.md), [maps.md](maps.md).
- [halo1/ai.md](../halo1/ai.md): the AI blocks the kit writes.
- [dedicated-host.md](../live/dedicated-host.md), [live-matchmaking.md](../live/live-matchmaking.md).
