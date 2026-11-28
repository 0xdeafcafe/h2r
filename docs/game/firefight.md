# Firefight

Status: Partly built. The definition format, validator, the built-in
wave sets and a map-specific one, director (FF2), stage-one offline kit, its home bases, drops,
firing positions and hunt orders (FF14), probe recipe, the host
runtime (FF4) with its `Game` over the guest bodies of the engine
entries and the `firefight` feature that runs it, the agent API
(FF5), the Game Setup row and options page, the HUD's mode blob, view and
its HUD kit HUD (FF8), the kit's stage two (FF12: transplanted classes,
the retail class table and `h2 firefight kit`'s class options), the
kit's steps to fit a map short of tag headroom, the mode state in the host log and the host log
in films (AN14), and the dedicated host's whole-pack
admission and bundled kits (FF10), and `h2 sim run` Firefight matches with
the 30 s game-thread CPU p99 (FF11), and Clankers on player teams holding,
falling back to and regrouping on their home base and rearming between
waves (FF16, `crates/live/bots/src/firefight.rs`) are implemented, tests only. "Guest boundary" lists the runs that
settle what the disassembly leaves open. Retail play and the network probes await verification.
Code: `crates/formats/firefight` defines and validates wave sets, loads
their data into `content/modes/rules/firefight.rules` (`src/script.rs`,
run by `ScriptDirector`) and holds the kill-point math its tests check against
(`src/scoring.rs`) and the director's interface types (`src/director.rs`).
`crates/tools/firefight-kit` builds the map kit. The offline command lives
in `crates/apps/h2-tools/src/firefight_cmd.rs`. The host runtime is
`crates/extensions/game/src/firefight/` with the shared squad entry in
`crates/extensions/game/src/actor_net/squads.rs`; the agent API's
methods are `crates/extensions/agent/src/firefight.rs`. The HUD is
`firefight/hud.rs` (the blob, the view and the feed) and
`crates/rewrites/hud-kit-core/src/views/firefight.rs` (its states) and
`content/modes/huds/firefight.hud.yaml` (its kit elements),
the Game Setup row and options page `firefight/options.rs`.
Live carries the match in the session data, seats Clankers on player
teams and takes the built-in sets in hoppers ("Live"); the slice table
lists the rest of the planned Live code. Tests use invented data.
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
  the same offline builder. A dedicated host's kits come in the owner's
  game data bundle, which `h2 host bundle` builds from the owner's disc.
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

- A Firefight game type in the custom game lobby ([the
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
Def      { format: 3, name, teams, rounds: [Round], set: SetRules, lives: Lives, caps: Caps, scoring: Scoring,
           bonus: Option<Bonus> }
SetRules { endless: bool, scale, skulls: [Skull] }
Round    { waves: [Wave], skulls_on: [Skull], allies: [SquadSpec] }
Wave     { squads: [SquadSpec], boss: bool, gap_ticks }
SquadSpec{ class, count: {per_player, min, max}, drop: Drop, weapon_tier, side: enemy|ally }
Lives    { pool_start, per_round_bonus, shared: bool, zero_holds: bool }
Bonus    { ticks, squad: SquadSpec, lives_by_points: [(points, lives)] }
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
  and unseen. `{"role":"line"|"flank"|"heavy"}` keeps only the zones
  whose `ZoneSeen::roles` carry the role, then applies the unseen and
  farther-half rule to them as for `"edge"`; `validate` refuses a squad
  whose role no zone of the kit carries (`Limits::zone_roles`). An auto-built kit tags every zone with all three
  roles. Weapon tiers range from 0 to 3.
- **Sets.** `rounds` describes one set. A finite definition (`set.endless`
  false, `scale` 0, no set skulls) plays it once and ends `Won`. An
  endless one has one team and exactly three rounds. When the last round's
  boss wave is cleared the next set begins: `Set{index}`, the round
  skulls switch off, the set's skull switches on, and its first `Round`
  follows. The game ends only `Lost`. Set *n* (zero-based) scales each
  squad's count by `1 + n * scale / 1,000`, capped by the actor cap
  (`scale` is at most 500). `set.skulls` lists up to three skulls that
  arrive at the start of sets 2, 3 and 4, in that order, and stay on;
  none arrives after the list ends. A set skull is listed once and is not
  a round skull. A skull the match began with is never switched off.
- A round's `skulls_on` adds skulls to those already enabled; in an
  endless definition a new set switches them off again. Each skull
  appears at most once in the schedule. The JSON names are
  `assassins`, `thunderstorm`, `famine`, `black_eye`, `ghost`, `catch`,
  `mythic`, `angry` and `low_gravity`. Blind is not a Firefight skull.
- `zero_holds` (default false, left out of the JSON when false) makes a
  dead player at zero lives wait for the next grant (a round's end, the
  bonus round's end) instead of returning free at the wave's end.
- **Allies.** A round's `allies` (default none, left out of the JSON when
  empty; `side` likewise left out when `enemy`) are squads on the
  players' side that arrive when the round starts and again after a boss
  wave for any that fell. Each is `side: ally`, a wave's squad is
  `enemy`, and `validate` refuses the other way round. An ally class has
  no score entry and is no wave's enemy class, so killing an ally scores
  nothing, and a kill by an ally has no credited player. A round has at
  most 4 ally actors at every player count (`MAX_ALLIES`), and their
  actors are added to each wave's when the cap is checked
  (`WaveOverCap`), since they count toward the live-actor cap. With two
  teams a round has at most two ally squads, the first on team 0 and the
  second on team 1. `Limits::with_factions` gives the kit's class
  factions: an ally class listed there must be `Human`, and one not
  listed is admitted.
- `bonus` (default absent) is the bonus round played after each set
  ("The bonus round"). `ticks` is 1 to 9,000. `squad` is checked as a
  wave's squad is: a scored class, bounds, and under the actor cap at
  every player count. `lives_by_points` has up to six rows, points
  strictly ascending up to 1,000,000, 1 to 1,000 lives each.
- `iron` is a JSON skull name too, but only the bonus round turns it on:
  `skulls_on` and `skull_multipliers` refuse it.
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
Stand for one team, Crossfire for two (a player kill scores 50), then
Endless for one team: round skulls Ghost, Catch and Black Eye, set skulls
Thunderstorm, Famine and Mythic (two of them stand in for ODST's two
skulls Halo 2 lacks, so they are data to swap), 7 starting lives, one more
each round, squads growing 10 percent a set, and one to two Marine allies
in every round. Outbreak, the sixth, is one team's three rounds of Flood
waves (`flood_combat_human`, `flood_combat_elite`, `flood_carrier`), each
closed by a boss wave of the authored Elite with a Flood Elite, with two to
three Marine allies a round. A set names the classes it needs
(`builtin::required_classes`) and a kit that lacks one cannot play it
(`builtin::missing_classes`); `outbreak_validates_on_a_full_kit` and
`every_new_class_has_a_score` in `crates/formats/firefight/tests/definition.rs` hold both.
They are files of the `modes` package, `content/modes/firefight/*.firefight.json`
(`firefight::builtin::SETS` embeds them), and the package lists each as a
`rules` item: `h2 package check` reads it as a `Def`, builds the director
script from it (`script::compile`) and refuses a set the lobby would
refuse (`workspace::wave_sets` lists a set's). The reference test in
`builtin.rs` holds the files to the sets written as Rust.
Each has three rounds. The first four use the stage-one kit's Elite class
alone; Endless and Outbreak also need the Marine, and Outbreak the Flood classes. Holdout's
round skulls are Ghost, Famine and Thunderstorm.
`builtin::with_teams(def, n)` plays a set with `n` teams: with two, a
player kill scores the set's own points or `builtin::PLAYER_KILL_POINTS`
(50); with one, nothing.

#### Map-specific sets

A set can be tied to a kit map. Its file lives in the `modes` package at
`content/modes/firefight/maps/<map>/<set>.firefight.json`, where `<map>`
is the base map's name — the same name the kit package is built from
(`Firefight – <map>`) — and it is listed as a `rules` item like any set,
so `h2 package check` reads it the same way.
`firefight::builtin::MAP_SETS` embeds them, and `builtin::map_sets(map)`
gives a map's own. Beaver Creek's is **Basin**: the map's tight bowl
keeps the waves small and quick — three rounds of at most twelve
actors, gaps of 450 ticks, ten lives with three back a round, Catch,
Angry and Ghost. Squads still drop at the edges, away from both bases;
a map set carries the map's tuning, not its own drop zones.

A map variant built in Forge can carry one set of its own in its
document, played by the **Map Waves** entry, listed after the built-in
sets (`builtin::MAP_WAVES`; [forge-firefight.md](forge-firefight.md),
"Playing it"). It is no hopper entry: a hopper refuses a prefix that names
no built-in set.

Change Rules → Firefight lists the built-in sets and each map-specific
variant. The record identifies its wave set by hash. Change Map filters by
that record, so Basin offers the Beaver Creek kit. At start the director
resolves the same hash. The direct-boot setting `firefight.waveset` uses
the built-in index (0 to 3), or `builtin::MAP_SET_BASE` plus the map-specific
index. See [Live](#live) for hopper records.

### The director

`firefight::ScriptDirector` (`crates/formats/firefight/src/script.rs`)
loads the director script with the definition and runs it on the rules VM. It
is a state machine with no I/O:

```rust
pub fn new(def: Def, settings: Settings) -> Result<ScriptDirector, ScriptRefusal>;
                                       // Settings { seed, difficulty, skulls }
pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command>;
pub struct Observed { players: Vec<PlayerSeen { slot, alive, team }>,
                      actors_alive_by_squad: [u16; 16],
                      zones: Vec<ZoneSeen { seen, distance, team_distance: [u32; 2] }>,
                      kills: Vec<Kill { class, killer, assist }>,
                      player_kills: Vec<PlayerKill { killer, victim }>,
                      ship: ShipSeen { ready, paths: Vec<bool> } }
pub enum Command { PlaceSquad { slot, class, count, zone, weapon_tier },
                   DropSquad { slot, class, count, zone, weapon_tier },
                   ShipHover { on }, ShipUnload, ShipLeave { zone }, Target { slot, team },
                   Hunt { slot }, EraseSquad { slot }, Round { index }, Wave { index },
                   Skull { id, on }, Lives { player, pool }, TeamLives { team, pool },
                   Score { player, points, reason }, Respawn { player },
                   Bonus { on, ticks }, Finish { outcome } }
pub enum Outcome { Won, Lost, TeamWon { team }, Draw }
```

- `ScriptDirector::new` refuses a definition that is not structurally
  valid, and one whose script does not build or verify. The VM's generator
  (SplitMix64) starts from the match seed XORed with the first eight bytes
  of `Def::id`. The director reads no clock, uses no floating point and
  iterates only ordered collections, so the same
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
- States (`Phase`): `Warmup` until the first wave's gap has passed, or
  one second (`first_lead`) if that is sooner: a wave's gap is the
  breather before it, and the game's start has nothing to breathe from;
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
- **Allies.** A round's `allies` are placed (`PlaceSquad`, on foot, at a
  zone chosen as for any squad) when the round starts and after a boss
  wave, one a tick, each only if no squad of that entry is alive and the
  cap has room (live actors, allies included, plus its count); a tick
  that is placing a wave places no ally. The wave's clear, its cleanup and
  its `Hunt` and `EraseSquad` look at enemy slots only, so a wave clears
  while allies live, and a standing ally stays through the next wave.
  `Slot::Placed::ally` marks an ally's slot, and its `target` is the
  team it serves. No `Target` follows an ally's `PlaceSquad`; an
  `Ally { slot, team }` does (`ff_ally(s, team)` in the script), and only
  an ally's. The host answers it with `SquadOp::Ally`: the engine team of
  the first player on Firefight team `team` (`firefight_team`), then the
  kit's `ff_hold_NN` order for base `team`, so the squad stands with its
  players' side and holds that base. It is refused, and the squad stays
  as placed, when the kit has no base or no hold order for that team, or
  no player is on it. The director counts allies apart:
  `ScriptDirector::live_actors` is enemies only (the last wave's clear
  reads it) and `live_allies` is the allies, shown as `live_allies` in
  `firefight.state`. A Clanker drops the actors on its own player team
  from what it sees, so it never targets an ally (docs/game/bots.md,
  "Firefight").
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
  the actor channel sends as `State`. `ScriptDirector::state` reads it
  back from the script's variables, and `ScriptDirector::resume` continues
  from the VM state (`RulesExt`).

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

`content/modes/rules/firefight.rules` defines the director's handlers.
The runtime loads it with a wave set, and the director's tests are its
scenarios ([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)).

- `script::load(&def)` validates the definition, supplies the squads
  (with each one's zone-role mask), waves, round skull schedule, set
  skulls, set constants (`endless`, `set_scale`), actor caps, points and
  multipliers as a data module, then compiles and verifies the script. A definition with
  more than 256 squads is refused by name, since a table holds 256 rows.
- Global variables hold match progress, player variables hold lives
  and registration, and object variables on the host's 16 `ff_slots`
  records hold squad state. The wave-set data and source determine the
  blob's hash.
- The difficulty and the skulls on from the start are script options
  (`script::options`). The VM seeds its generator from the match seed
  and the blob's hash; `script::match_seed` hands it a seed that starts
  where the director's generator starts, and `random(n)` draws by
  SplitMix64, modulo `n`.
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

- Sets run in the script: the global `set_index`, `end_wave` starting the
  next set when `endless` is 1, and `begin_set` queuing `ff_set(n)`, a
  `ff_skull(id, false)` for each scheduled skull the match did not begin
  with (`base_skulls`), and the set's skull. `zone_has_role(z, mask)`
  reads `ZoneSeen::roles` (every zone fits mask 0). `director::State::set`
  reads `set_index` back. The director is the script alone, so the sets
  tests (`tests/sets.rs`) hold it to itself: a fresh script over what a
  match observed gives the commands the match was played with.

- Allies run in the script: the tables `ally_squads` and `ally_rounds`
  (each round's first row and count), the globals `allies_due`,
  `ally_next` and `ally_count`, `arrive_allies` in `next_round` and
  `place_ally` in the tick. A slot's `squad_ally` is 0 for an enemy squad
  and the ally's row in its round plus one otherwise. `observe_squads`
  sums an ally's actors into `ally_count`, not `live_count`; `place_next`
  checks `live_count + ally_count`. The tests (`tests/script.rs`) are
  `a_wave_clears_while_allies_live`, `allies_count_toward_the_actor_cap`,
  `an_ally_kill_scores_nothing`,
  `allies_arrive_at_round_start_and_after_a_boss` and
  `ally_rules_give_the_same_commands`; `tests/factions.rs` holds the
  admission rules.

- Two teams run in the script too. Each team's pool and score are
  globals (`pool_0`, `pool_1`, `score_0`, `score_1`), the player's team
  a per-player variable `on join` fixes from `team_of`, and a slot's
  target sits in its `squad_side` field. `on kill`
  scores player kills (only in a two-team blob with `player_kill` above
  0). The entries `zone_team_distance(z, team)`, `ff_target(s, team)`,
  `ff_ally(s, team)` (an ally squad's team, after its `place_squad_at`),
  `ff_team_lives(team, n)`, `ff_player_score(p, n)` and
  `ff_team_finish(winner)` (`none` a draw) stand for the two-team parts
  of `Observed` and `Command`. The blob's header has `teams` 1 or 2.

The director's tests (`crates/formats/firefight/tests`) drive the script
over their scenarios, and every match the fake world plays is replayed
through a fresh script for the same commands: the golden match, the empty
pool, the lost match, unshared lives, assists, kill points at the
multiplier limits, zone choice, 48 random matches over the quick built-in
sets at every player count (Crossfire among them, with player kills), the
built-in sets as authored, starting skulls, unshared lives, zero gaps,
zones all in sight and ties, duplicate and absent players, a resumed
state, and two teams: 24 random two-team matches with shared and own
lives, every two-team end, player kills of each kind with a team change
after first sight, and targets with zones on each side. `scoring.rs`'s
kill-point math is what the scoring tests check the script's limbs
against.

Limits of the script:

- A worst-tick estimate that fits. The zone choice ranks the zones with
  a nested `for each` over the labelled set, and the verifier bounds a
  labelled set at 256 members, so `Report::fits` is false. The VM's run
  time budget (20,000 ops a tick) holds for the kit's 6 to 12 zones.
- Player slots of 16 and over, which the VM addresses as `none`: such a
  player is not scored or registered.
- Zone distances over 2^31 - 1 are read clamped.
- A tick with more than 128 commands, past the VM's queue, is a script
  error.

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

- a `before` on `GAME_ACTIVATE`, while the options block (EAX) is not
  yet the game's (`Runtime::activate`). With Firefight on and a map that
  carries the kit it sets the variant's lives to 1
  (`OPTIONS_VARIANT_LIVES`, `with_held_respawns`), so the engine holds
  every respawn for the director ("Lives, rounds and skulls"), and with
  two teams it turns the variant's team play on. Firefight off, another
  mode, a film's playback and a map without the kit leave the options
  alone. So a two-team match is a team game without the player setting
  Team Play, and no variant's own lives reach a Firefight match.
- a `before` at the multiplayer gate of `ai_initialize_for_new_map`
  (`AI_NEW_MAP_MULTIPLAYER`, `0x1c79e9`, in
  `guest_layout::points::ai_gates`). Retail clears the AI globals'
  initialised byte and returns there in every multiplayer game, and while
  that byte is clear `AI_SQUAD_PLACE` places nothing and `ai_update`
  skips its tick. On a map that carries the kit
  (`runtime::initialises_ai`) the hook jumps to the campaign's path
  (`AI_NEW_MAP_INITIALISE`, `0x1c79f6`), which initialises the AI for
  the map and sets the byte. Both addresses are image entries
  (`boot::image_entries`).
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

The machine's role is `Replica` in a film's playback and while a film
named with `--film` records. Otherwise the simulation world's mode
decides (docs/live/live-coop.md): `Replica` for a synchronous client (3),
`Client` for a distributed client (5), and `Host` otherwise. A session's
[default recording](film-format.md#recording-every-game) and its replay
take the role by the mode. A film's playback gets the host's entries from
the film ("Theater"); no transport carries them to a synchronous peer
yet, so a peer applies none. After each tick the host publishes its
log head (`actor_net::publish_log_head`) for the actor channel's
digest. A tick boundary's number is the game
time's tick count plus one before the tick and the count after it, so
the director's entries for the next tick are due at the next boundary.
A snapshot is refused while a match runs.

| Role | Runs the director | Applies entries |
|---|---|---|
| `Host`: the distributed server, or the host of a synchronous session | yes | yes |
| `Replica`: a synchronous peer, or a film's playback | no | the entries it receives |
| `Client`: a distributed client | no | none; the actor channel brings the mode blob, the host's log head and the actors' presentation ([ai-networking.md](ai-networking.md#on-a-client-each-tick)) |

1. **At map load**, `Host::start` refuses the mode on a map without the
   kit, on a kit with no drop zones, and for a definition that names a
   class the kit lacks (`firefight::validate` against the kit's classes).
   A two-team definition is refused on a variant whose team play the
   start could not turn on or cannot be read (`runtime::teams_refusal`),
   and on a kit with
   fewer bases than teams (`Refusal::Bases`). The actors take the kit's
   team unless a player holds it, else the lowest team no player holds;
   with two player teams that is a third team. The host log starts at a genesis hash of
   the definition's id and the match seed.
2. **The AI switch.** The kit map's AI is initialised by the gate above.
   At the first tick boundary after the AI globals are
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
   director's (`ScriptDirector::set_lives`, `ScriptDirector::set_team_pool`;
   the director's own set the count it holds, an operator's changes it), and
   the director spends a team's respawns from its pool and holds them at
   0, through the same `Respawn` entry as one team; the HUD reads the
   pools from the mode blob. `Finish` ends the game through the engine's
   own game end.
   `Round` and `Wave` change nothing in the game: the HUD reads them from
   the mode blob.
4. **A wave nobody can fight.** The host keeps the current wave's
   placements (`PlaceSquad`, a `DropSquad` on foot or by ship, a pod's
   landing; a `Wave` entry starts the next) and the engine's refusal of
   each, made at once or by the runtime after the tick's calls
   (`Host::refused_entry`). When the director has placed the whole wave
   (`Fighting` or `Cleanup`) and the engine refused every placement,
   the wave has no actor to clear. The director would count it as
   cleared at its slot timeout and announce the next, so the host stops
   it there instead (`Host::stopped`): it steps the director no more,
   and one error row names the refusal ("round 1 wave 1 is refused, not
   cleared: the engine placed none of its 1 squad (...); the director is
   stopped"). The match has no result. One placement the engine takes is
   enough for the wave to be fought.
5. **After each game tick** (`Host::post_tick`, the host only), `Observed`
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
6. **Received entries** (`Host::receive`) are checked against the chain.
   The host refuses them: it writes its own log.
7. **The mode state** rides in the log. After each step the host
   appends the HUD's mode blob as a `ModeState` entry for the next tick
   when it differs from the last one it appended (`Host::note_mode`,
   from `runtime::post_tick`). Applying one makes no engine call; it
   sets the blob the machine holds (`Host::mode_state`), which a replica
   publishes for its HUD after each pre-tick. The operator cannot append
   one: the agent API's commands have no such variant.
8. **The Clankers** are told of the match (`runtime::bot_context`,
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
| Sectors in each `sbsp` | `pathfind::generate`, then `pathfind::trim` to the sectors the AI can use ("Sectors the AI can use", below) | the generator the bots' mesh starts from, which has run on every retail multiplayer map (**Code**, `crates/live/bots/src/cache.rs`); the engine has not yet loaded its output (**S**) |
| Drop zones | the bots' nav mesh (`bots::autonav`): sectors near the map's edge, off the players' spawn clusters, with a path to the middle | 6 to 12 a map; written as zones and areas |
| Zones, areas, firing positions | sampled from the sectors: cover is a position with a wall toward the middle | the block writers of `h1campaign::ai` (`ScenarioBlocks`) |
| Squad slots | 16 empty squads the director fills (class, count and zone are set at place time), then the class record: one empty squad `ff_class_<name>` per palette row, on its class's faction's team | the same writers |
| Characters, stage 1 | `char` tags we author on bipeds and weapons the multiplayer map already has. With no parent, the Elite carries a row of every property block the engine reads unchecked (`ai/character.rs`, `PROPERTY_ROWS`: an empty movement block faulted at `0x1f911a` (`0x1f90f0`, reading the row `0x1e4a50` returns) on an actor's first update). Each row is the retail campaign Elite's (`objects\characters\elite\ai\elite`, walking its parents), read from the base, the disc map `ELITE_DONOR_MAP` (`03a_oldmombasa`) or a class donor (the built-in kits, `h2 firefight kit`, host data, a Forge document's ground and a `map` item's ground all give the disc's), with our tuning table's fields set over it and the donor's tag references null. No map carrying it, or a row missing, refuses the build by name. Every placement checks it again in the game (`guest::character_ready`, from `place_squad` and a ship's boarding): a palette row naming no character, or one whose character and parents have no row of `guest::UNCHECKED_ROWS` (the movement row), is refused by its row before the squad is written, whatever built the kit (a class tag the base already had is reused unchecked) | Elites only; no tag is copied from another map, only the Elite's rows |
| Dropship | the Phantom by stage-2 transplant (the map's own Spirit on a Halo 1-built map), one parked `ff_ship_phantom` placement and, when a donor has it, a `ff_ship_pelican` one, a point set `ff_path_NN` for each drop zone with a clear column ("The dropship", below) | cut before any class when headroom is short |
| Drop pods | Halo 2's `insertion_pod` by stage-2 transplant when the map lacks it, pools `ff_pod_00..03` (squads) and `ff_ppod_00..07` (players), a point set `ff_fall_NN` per drop zone and `ff_pfall_NN` per starting location with an open column ("Drop pods", below) | cut after the ship, before any class |
| Characters, stage 2 | `firefight_kit::Classes` (`src/transplant.rs`): `tags::schema::transplant` of a donor's `char` tag and its closure, from the player's own campaign maps, with the sound gestalt merge (`h1_assets::sounds::transplant_from`); the retail class table (`src/retail.rs`) names the donors | each class costs tag headroom; the builder refuses a build that does not fit, naming every class's cost, or cuts classes from the end of the list |

Stage 1 runs against a map on the configured disc or extracted disc tree:

```sh
h2 firefight kit coagulation
h2 firefight kit beavercreek
h2 firefight kit beavercreek --home blue --replace
h2 firefight kit coagulation --probe
h2 firefight kit beavercreek --classes my-classes.txt --cut
h2 firefight kit zanzibar --retail-classes --cut
h2 firefight kit zanzibar --dropship none
```

The first command installs `<data>/maps/firefight-coagulation`. `--home
red|blue` (`firefight_kit::Config { home }`, red by default) says which
map team's base is the home base. `--probe` builds the `firefight-probe`
recipe, which puts two Elites in one squad and adds a startup script
that calls `ai_place` for it. The regular kit keeps its sixteen slot squads
empty. The [test-map API](test-maps.md) can also create and list the
probe on a running host. There is no separate list of Firefight maps:
the lobby offers every installed kit package, so building a map's kit
offers it.

Stage 2 (`firefight_kit::build_package_with`) takes a `Classes`: the
donor maps (name and file bytes), the schema, and the class list, each a
donor's `char` tag path. The classes are copied after the Elite, so
palette row 0 stays the Elite and row N+1 is class N of the list. A tag
the base already has (same group and name) is reused, so a class costs
what its closure adds over the base and the classes before it; sounds
bring their entries into the base's `ugh!` gestalt, and a class with
sounds is refused when the base or the donor has no gestalt. The
package id follows the base, the kit version and the list in order, so
two machines with the same donors and list build the same id.

**The class record.** The kit writes, after the sixteen slots, one empty
squad per palette row, named `ff_class_<name>` with the row as its
character (`firefight_kit::ai::class_squads`): `ff_class_elite` for row 0,
then each kept class in list order. A class the build cut has no row and
no record. A name is a lowercase identifier of at most 22 characters, and
one named twice is refused when the kit is built. The game reads the class
table from these squads (`read_kit`, `KitLayout::classes`), so the director
places each class on the row the kit built it on, whatever the list was. A
squad naming a row outside the palette, a repeat of a name and an empty
name record nothing, and a map with no record has no classes: the Elite is
not assumed. Definitions are admitted against the record. A definition
whose scoring table, which every wave squad's class must have an entry in,
names a class the map lacks is refused with that class's name
(`Refusal::UnknownClass`), so a wave never falls back to the Elite. A
caller that knows which classes the build cut (`Package::cut`) passes them
as `Limits::with_cut`, and the refusal then says the kit cut the class
(`Refusal::CutClass`) rather than that it never had it.

**Factions.** A class has a faction (`firefight::Faction`: Covenant, Flood,
Human; `ClassSpec::faction`). The kit writes it as the team of the class's
record squad: Covenant 3, Flood 4, humans 2, the engine's own numbers
(`firefight::teams`, [ai.md](../halo1/ai.md)), and the Elite is Covenant.
`read_kit` reads it back into `KitLayout::factions` by palette row; a
record on any other team is no class, so the map provides none and a set
naming it is refused by name. `place_squad` sets the placed squad's team
from its class's faction (`KitLayout::placement_team`): a Covenant class
takes the team the host picked for the actors no player holds (the slot
squads' team, 3), Flood and humans their own. A palette row with no class
record is an error. Every faction is on its own team, so under Slayer's
"teams differ" test each is hostile to the others and to the players with
no table of ours (**V**: that actors choose targets by that test and not
the campaign allegiance table; the coordinator's run R5 settles it). Enemy
humans are legal in a custom set; allies are a round's `allies`
("The definition"), which the director places but the game does not yet
put on the home team or give a hold order. The kit version
(`KIT_VERSION`) is 6, the first whose Elite carries every property row
the engine reads unchecked; 5 was the first whose record carries the
faction. The version names the kit's output: the built-in kits' stamps,
a `map` item's ground and a Forge document's ground all key on it, so a
change to the bytes a build writes raises it, or kits built before the
change stay current. `the_kit_version_names_the_built_bytes` fails when
the fixture's build changes under the same version.

The Flood and human classes are named in `retail::FACTION_CLASSES`, with
their `char` tags and donor maps as constants in `retail::donors`, every one
**V** until the census step (P1) reads them off the disc: `flood_combat_elite`
(**V**: tag-schema.md says the transplant refuses its tree),
`flood_combat_human`, `flood_carrier`, `marine`. `retail::choose_factions`
takes each class's first donor on the disc and refuses a class none of whose
donors is there by its name and donors; nothing stands in for a missing
class. `retail::KEEP_ORDER` is the whole keep order, last cut first: the
Elite, `flood_combat_human`, `marine`, the pod, the Phantom and the Pelican,
`flood_combat_elite`, `flood_carrier`, then the Covenant table; the vehicles
are not copied yet, and each is cut before the first Flood class. A class
may name its faction on the command line: `--class NAME@flood=DONOR:CHAR_PATH`
(`covenant` when omitted).

`h2 firefight kit` builds the list from the player's own disc
(`crates/apps/h2-tools/src/firefight_cmd.rs`). Each class is `--class
NAME=DONOR:CHAR_PATH`, where DONOR is a campaign map on the disc and
CHAR_PATH the `char` tag's name in it; `--classes FILE` reads one such
class per line (blank lines and `#` comments skipped). The order of the
flags and lines is the keep order, `--cut` sets `cut_to_fit`, and the
command prints each class's cost and what it cut. Donor maps are read
from the disc at run time and the schema is `Schema::builtin()`; a class
name listed twice, or `--probe` with classes, is refused.
`--retail-classes` puts the retail class table
(`firefight_kit::retail::CLASSES`) before any other class: `grunt`,
`jackal`, `drone`, `brute` and `hunter`, in that keep order, each a
`char` tag name and the campaign maps that carry it, the first on the
player's disc being the donor (`retail::choose`). The table is names
only, from a census of the disc's tag names
([notes/2026-11-26-firefight-class-census.md](notes/2026-11-26-firefight-class-census.md));
the tags are copied from the player's own maps. Outskirts
(`03a_oldmombasa`) gives four of the five, so their shared tags come
once. The host runtime reads each class's palette row from the kit's
class record (`read_kit`, below).

**Fitting a map short of headroom.** A build over the tag headroom
takes the kit's own steps first (`firefight_kit::Fit`, in this order,
each kept once taken): `MergeSectors` joins coplanar neighbouring
sectors after trimming (`pathfind::merge::coplanar`,
[pathfinding.md](../formats/pathfinding.md), "Merging"), the same
ground in fewer sectors, links and vertices; `FewerDrops` keeps the first
six drop zones chosen, which alternate between the bases' sides and
spread farthest first (placed `ff_drop` labels are never dropped). A
map with room takes neither, so its bytes and package id are those of
the plain build. Classes are cut only after both steps, so a player's
class outlives the kit's extra drops. `Package::fitted` and the command's
`fit:` lines name the steps taken; a refusal says it took every step,
and the cost report gives the merged sector count (`sectors_merged`).

**The cost and the cut order.** `Package::classes` gives each class's
tags, sounds and tag bytes (the copied tags and their index entries; the
gestalt's growth and alignment are in the build's exact `tag_growth`),
and `Costs::classes` their sum. The list is in keep order: the first
class is the last to go. When the build is over the tag headroom, the
default is a refusal that names the excess, the biggest part and every
class's cost. With `Classes::cut_to_fit` the builder retries without the
last class until the build fits, and `Package::cut` names what it cut,
first cut first. A refusal by the schema (a tag it cannot copy) or a
missing `char` is an error naming the class, never a cut.

**The dropship (rules-world-api.md, "Vehicles and drop ships").** A kit
carries a ship the host flies squads in on (`src/ship.rs`,
`h2 firefight kit MAP --dropship auto|phantom|spirit|none`, `auto` by
default, `firefight_kit::Config::dropship`):

- **The tag.** The Phantom, `objects\vehicles\phantom\phantom`, copied in by the
  stage-two transplant from the first donor that carries it
  (`transplant::add`, a `vehi` root; the command adds Outskirts,
  `03a_oldmombasa`, as a donor when the disc has it and no class named it).
  A map built from Halo 1's campaign already holds Halo 1's Covenant
  dropship (the Spirit, `vehicles\c_dropship\c_dropship`; `h1campaign::h2assets`
  keeps it as a set piece) and uses it with no donor. `auto` takes the
  Spirit, then a Phantom the map holds, then a donor's; with none to be had it
  builds the kit as before and says why. `phantom` and `spirit`
  fail when their ship cannot be had; `none` writes nothing.
- **Ship kinds.** A kind is `firefight::ShipKind`: the Phantom (the Spirit
  on a Halo 1 map) for Covenant squads and the Pelican,
  `objects\vehicles\pelican\pelican` (the map's own, else a donor's,
  `05a_deltaapproach` on the command line), for human squads
  (`ShipKind::for_faction`; the Flood have none and arrive on foot). The kit
  parks one of each kind it keeps. A Pelican no donor has, or one no zone is
  wide enough for, is cut (`Package::pelican` says `pelican: cut (reason)`)
  and the Phantom is kept; the Pelican is cut before the Phantom when
  headroom is short.
- **The placement.** One vehicle placement per kind kept, named
  `ff_ship_<kind>` (an object name),
  on a new vehicle palette row for the ship, placed at the start (not
  "not at start") at the first path's run-in, with that path's yaw. The base's
  own vehicles, names and palette rows keep their indices; the new ones
  follow them (`ship::author`).
- **The paths.** For each drop zone `NN` with a clear column (a thin ray from the floor to the sky, and the ship's reach clear from its hover point up, so a rise beside the floor below the hover does not close it), a point set
  `ff_path_NN` in the scenario's scripting data with the points `run_in`,
  a `hover_<kind>` for each kind kept and `run_out`, in flying order. A kind's
  size is read from its own `vehi` tag (`ship::dims_in`): its reach is the
  object's bounding radius, its hover, when the command line has surveyed the campaign, the median height
  over the collision floor of the ship's held waypoints in the retail scenarios
  (`ship::HoverSurvey`: the points with a `_facing` partner in point sets whose
  name holds `phantom` or `pelican`, on all fourteen campaign maps; the report
  names the maps and counts), else the height of its render model's lowest position
  under the root (the dropship tags have no lift points or flight settings), over the zone's first area's mean, and its seats the seat block's
  count (`ship::tag_seats` reads it again in the game; it counts every seat, the driver,
  gunners and cargo included, so it caps a load and the engine seats fewer). The path is one per
  zone, cleared for the widest ship kept: each ray is also cast the kind's
  reach to each side, and the Pelican is kept only when some zone has a path
  at its width. The
  runs are at the sky height, 10 units over the tallest collision, and
  outside the collision's horizontal bounds by at least 20. `ship::plan`
  casts rays on the arena BSP's collision without its sky shell
  (`bots::cache::walls_without_sky`: the lid and walls that reach the
  highest surface close the world and are no roof) (`bots::los::Walls::clear`):
  from just above the zone's floor to the sky (a roof over the zone
  fails it), and, for the first of 24 compass directions, every 15 degrees (starting
  away from the map's middle) where both clear, from the run-in down to
  the hover and from the hover up to the run-out, which lie on opposite sides.
  A zone with no clear long run is tried again with runs 80, 50, 30, 15 and 5 units
  long, steeper dives for a canyon, the last nearly vertical over the column.
  A zone's run is the widest kept ship's where it has one, else the next
  widest's; a kind the run does not clear at its own hover is left off that
  zone's path.
  A zone with no such direction gets no point set, and its squads spawn on foot.
  The paths are not placed by hand; a map keeps what the rays find.
- **Fit.** The ship is cut before any class: with a map short of headroom
  the builder takes the `Fit` steps with the ship, then tries without it,
  and only then drops classes (`fit_within`). A kit with no ship has
  the bytes of the kit before it. `Package::ship` (`ShipReport`) and the
  command's `dropship:` line say `kept (tag, tags copied, bytes, zone
  paths)` or `cut (reason)`: over the tag headroom, no donor, or no clear
  column; `Costs::ship` is the bytes (the copied tags, the placement, the
  palette row, the name and the point sets). A probe kit has no ship.

**Drop pods.** A kit carries Halo 2's `insertion_pod` vehicle (`src/sky.rs`),
copied in by stage two from a donor when the map lacks it
(`transplant::add`; a donor without it, or a refused copy, is a
`PodRefused` and the build goes on without pods). `Config::no_pods` leaves
them out. The kit writes parked placements `ff_pod_00` to `ff_pod_03` for
squads and `ff_ppod_00` to `ff_ppod_07` for players, and two kinds of point
set with the points `top` and `ground`: `ff_fall_NN` for drop zone `ff_drop_NN`
(its first area's mean) and `ff_pfall_NN` for starting location NN (its
index in the scenario). The pods go in the same vehicle blocks as the ship
(`sky::author` extends them) and the same scripting block as its paths.

The sky test (`sky::plan`) is a kit-build flag, the same on every machine,
with no runtime ray. From 0.2 above the ground, five rays go up to the top of
the collision: one straight up and four on a 0.6 ring. A ray is open when it
hits nothing, or when the first surface it hits is sky (`sky::Roof`: the
solid surfaces and the sky ones, split by collision material index). A
material is sky when its shader's path has a component `sky`, or ends in
`sky` or `sky_...` (`sky::sky_materials`, over the structure BSP's
materials block at `+0xa4`, 0x20 bytes an element with the shader at `+8`),
and `Config::sky_materials` adds more. The fall is the lower of 40 and the
way up to the top of the collision, and under 12 is no fall. The retail
multiplayer maps have no sky material in their collision but close their world
in a lid at the sky height: a map with no sky material tests against the walls
without that lid (`bots::cache::walls_without_sky`, as the ship's paths do), so
open sky is a ray that hits nothing else
([note](notes/2026-11-27-firefight-sky-materials.md)). With no open
zone or start there are no pods. `Package::pods` (`PodReport`) says `kept
(tag, tags copied, bytes, zone falls); pod starts: N of M open` or `cut
(reason)`; `Costs::pods` is the bytes. The game reads them with the kit
(`guest::read_kit`): `KitLayout::{pods, player_pods, falls, player_falls}`.
Pods are cut right after the ship and before any class when headroom is
short.

**Players fall in them** (`firefight/pods.rs`). The pod is layered on a
spawn the engine has already made. While pods are on, a spawn prefers the
team's open starts: `PLAYER_SPAWN_LOCATION` starts with that filter and the
free-spot test `SPAWN_LOCATION_FREE` answers "taken" for any other start
(`pods::pick`; with no open start, or pods off, the engine's own pick stands).
`PodDrops` is a pure machine the runtime steps after each tick over the
players and the pool `ff_ppod_NN`; a player who has just spawned within 2
units of a fall's ground takes the first free pod, the first of a tick
at once and the rest 10 ticks apart. Its acts are made at the next tick
boundary as engine calls (`Call::PodDrop`, `PodEject`, `PodPark`):

1. the line of sight from the fall's top to its ground is checked
   (`AI_LINE_OF_SIGHT`, which includes objects), the pod is put at the top and
   the unit is seated in its first seat (`UNIT_ENTER_VEHICLE`, `0x11ade0`: the
   vehicle in EBX, the unit and the seat label on the stack; the label is
   read from the vehicle tag's seats block). The routine seats no one, and
   says nothing, when the rider fails the seat test at `0xc8200`: its
   animation graph needs a mode of the seat's label (`0x1cb920`), unless the
   unit's byte at `+0xaa` is 1. The host makes the same test first
   (`pods::takes_seat`) and names that refusal;
2. on success the pod flies the two points in 75 ticks (`Flights`), counted in
   `pod_spawns`;
3. when it is within a unit of the ground, 15 ticks later the rider is ejected
   (`0xe68c0`, action `0x1e`), and 300 ticks after that the pod parks again;
4. a pod that has not landed 30 ticks after its fall should have ejects its
   rider and parks (the watchdog).

Every failure leaves the player standing at the start and counts in
`pod_fallbacks` by reason: `no_fall` (no fall at the spawn), `blocked`,
`no_free_pod`, `seat_refused`, `pod_gone`, `never_landed`, `gone` (the player
died first). A wave test drops no pods. The entry routine's registers are read
from the disassembly and are not yet confirmed by a run (R6), and so is
whether the engine's seat camera reads well in the fall. The drops are engine
calls, not host log entries, so a film does not replay them.

**Squads fall in them** (`firefight/squad_pods.rs`). A squad's `arrival`
(`SquadSpec::arrival`: `auto`, `foot`, `ship`, `pod`; `Arrival::number` in the
director's tables) picks how it reaches its zone. `auto` is a pod for a class
that is not Covenant (`Kit::pod_classes`) and the ship otherwise. The script
(`pod_class`, `zone_fall`, `pods_free`) sends a pod squad with
`ff_drop_squad(..., 3)` (`Command::DropSquad { arrival: Pod }`) when the zone
has a fall and a squad pod is free, and places it on foot otherwise, so a wave
never waits. The host (`SquadPods`, stepped by every role that applies
entries) flies the first free `ff_pod_NN` down the zone's fall in
`pods::FALL_TICKS`; when the fall is over the host logs a `PlaceSquad` for the
squad, so a film replays the same arrival at the same tick. The pod is a
wreck for `pods::WRECK_TICKS` after the squad is placed, then parks. An
erased squad's pod parks the same way. Fallbacks count as `no_fall` and
`no_free_pod`.

The kit only places the ship parked and gives it the points to fly. The
placement carries no hover flag: the director turns the hold on
(`ShipHover`), and the host flies and loads the ship ("The drop ship",
below).

**The drop ship.** The guest reads the kit's ship and paths with the rest
of the kit (`guest::read_kit`): `KitLayout::ships` is, by kind, the vehicle placement
the object name `ff_ship_<kind>` points at, and `KitLayout::paths` holds, for
each drop zone in kit order, its `ff_path_NN` point set's `run_in`,
`run_out` and a hover for each `hover_<kind>` key (`ship::ShipPath`), or none
when the zone has no set or the set misses its run points or every hover. A game reports the ship as live when
exactly one vehicle of that placement is in the world
(`ship::placed_ship`); its seats are the vehicle tag's seat count (`ship::tag_seats`).

The director (`content/modes/rules/firefight.rules`) holds the ship's state
in three globals, `ship_state` (parked, inbound, unloading, outbound),
`ship_tick` and `ship_zone`. Each tick the host tells it whether the ship
of each kind is in the world (`Observed::ship.kinds[k].ready`, read
`ship_ready(kind)`), which zones have a path for it (`zone_path(zone, kind)`),
and by which kind each class arrives (`squad_ship(class)`: 0 Phantom, 1
Pelican, -1 none; the class's faction, `ShipSeen::class_ships`). `ship_kind`
names the ship out; one ship is out at a time.

- A squad whose chosen zone has a path for its kind, with no ship out and
  that ship in the world, is placed by `ff_ship_hover(kind, true)` and
  `ff_drop_squad(..., kind)`
  (`Command::ShipHover`, `Command::DropSquad`); the state is inbound.
  When the warmup cut the set's first gap (`opening_cut`), the game's
  first wave (round 0, wave 0 of the first set) takes no ship: its squads
  drop by pod or arrive on foot, so the first enemies land within seconds
  of the start and not after the ship's 10 s run.
  Every other squad, including each squad of a wave while the ship is
  out, is placed on foot by `place_squad_at` as before, so a wave
  never waits on the ship, and a map with no ship or no path has the
  commands it had.
- `DropSquad` makes the host fly the ship from the path's run-in to its
  hover point in `ship::FLY_TICKS` (240, 8 s) and place the squad into
  it (`Game::fly`, `Game::drop_squad`, `ship::drop_squad`): up to the
  ship's free seats ride, the rest of the squad spawns on foot at the
  zone. A `DropSquad` for a zone the host finds no ship or path for
  places on foot, as `PlaceSquad`.
- 240 ticks after the drop the script calls `ff_ship_unload(kind)`
  (`ShipUnload`: `VEHICLE_UNLOAD`, every seat); 60 ticks later
  `ff_ship_leave(kind, zone)` (`ShipLeave`: a flight from the hover point to the
  run-out in `FLY_TICKS`); 240 ticks after that `ff_ship_hover(kind, false)` and the
  ship is parked again. The ship's puts come before the tick's other
  calls so a squad placed into a ship that begins its flight finds it
  at the run-in.
- `squad_alive` and the slot timeout read as before: riders are actors the
  moment they are placed.

The build is `firefight_kit::ground::build`, which takes the labels
placed on the map and the recipe, and gives each label's place to the
generator below ([game-types-without-carriers.md](game-types-without-carriers.md),
"The build step"); with no labels it makes what this section describes.

**Bases.** `firefight-kit/src/bases.rs` finds two bases from the
scenario: the mean of each team's CTF flag stands (netgame flags at
`scnr+0x118`, type 0, team at `+0x12`, teams 0 and 1); else the mean of
each team's starting locations (`scnr+0x100`, team designator at
`+0x10`); else all starts split into two clusters seeded at the pair
farthest apart; `ff_base` labels, when placed, are the bases instead.
Base 0 is the home base, base 1 the other. Each starting
location's team designator is rewritten to its nearer base (a tie goes
home), so team 0 spawns at the home base and team 1 at the other; the
rows are rewritten in place, or as a new block at `scnr+0x100` (costed
as `starts`) when they are not in the scenario's own bytes.

`firefight-kit/src/nav.rs` generates pathfinding for every structure BSP.
It chooses 6–12 boundary sectors on the walkable component that holds
both bases, so every drop has a path to each base over shared sector
edges with enough clearance for the bots' walk mesh. A drop's centre
stays at least 3 units from every start and a quarter of the bases'
separation (2 to 8 units) from each base; picks alternate between the
bases' sides, spread farthest first. Each zone has an area and separated
firing positions, preferring cover that faces the nearer base. After the
drops come `ff_base_00` and `ff_base_01`, each one area with up to 8
firing positions around its base, and the orders `ff_hunt_00` and
`ff_hunt_01`, whose primary areas are that base's area only, so a squad
hunted at team N pushes into base N, then `ff_hold_00` and `ff_hold_01`,
whose one primary area is the first area of the base's zone: the order an
allied squad is given, to stay on that ground (`firefight-kit/src/ai.rs`
`hold_orders`; the engine's reading of the one-area order is not run). The kit refuses maps without six
suitable drops or without the required local tags.

**Built-in layouts.** A map with an authored layout takes its drops from
it, not from the generator. Layouts are code in
`crates/tools/firefight-kit/src/builtin/{mod,coagulation,zanzibar}.rs`;
`builtin::for_map(name)` finds one and `Config::layout` carries it.
`Layout { map, home, two_teams, base, drops, cover }` names each drop by an
`Anchor` on the map's own data, never a coordinate: `Flag { kind, team,
nth }` (netgame flags, `scnr+0x118`: CTF stand, oddball, teleporter source
and exit, territory, hill corner), `Starts { team }` (the mean of a team's
starting locations), `Item { suffix, nth }` (a weapon pad by the end of its
tag name: a netgame equipment entry, `scnr+0x120`, where retail multiplayer
maps place their weapons, then a scenario weapon placement), `Between(a, b, t)` and `Or(a, b)` (the first that
resolves). A drop is its anchor, an offset, a snap radius, its `Roles` and
its `Sides`. The base anchor replaces the home base's point and caps the
base zone's firing positions (4 on Zanzibar, which is short of headroom).

`nav::choose_zones_with` snaps each drop to the nearest sector within its
radius that is on the walkable area both bases share, outside the base
exclusion, not on an earlier drop's sector and with room for two firing
positions. Drops keep the layout's order (`ff_drop_NN`). A drop that
resolves nowhere or finds no such sector is refused by name, with the
reason (`drop 'west_cave': no usable sector within 4.0`, `... (inside the
base exclusion)`, `... (no path to both bases)`), and the build fails
with every refusal; the generator never picks in its place. The package
label carries the layout's digest (`Layout::digest`), so the package id
changes with the layout, and a new `KIT_VERSION` changes it for every kit.

Each drop zone carries its roles in its flags above the engine's bits
(`firefight::roles`: `LINE`, `FLANK`, `HEAVY`, from bit 8). A kit with no
layout gives every drop zone all three, so a wave set plays as it does
without roles. The game reads them back into `KitZone::roles`
(`guest.rs::read_kit`); base zones have none. The director does not filter
by role yet.

Positions the layouts cannot know without the disc map (the wall offsets,
pad suffixes, snap radii and which team's stand the Zanzibar base is) are
named constants marked `V` in the layout files. `h2 firefight kit MAP
--layout-report` is the census that settles them: every flag, start
cluster and weapon pad with its kind, team and index, then each anchor's
resolved point, the sector it snapped to, its distance to each base, each
refusal and the cost table. `--layout` builds the kit with the map's
layout. Coagulation's layout has two teams (Crossfire is offered); Zanzibar's
has one.

**Sectors the AI can use.** The generator gives every walkable collision
surface a sector, including floors no player reaches: decorative
islands, rooftops, ledges and terrain past the boundary walls. Their
links and vertices are most of the pathfinding bytes. After choosing the
zones, `nav.rs` (`usable_sectors`) keeps, on the arena's BSP, the sectors
the arena's firing positions reach over two-sided links between sectors
in the map. Two-sided links are the only links the engine's path search
has in generated data, so no AI path leaves that set. In the map is the
bots' classification (`bots::autonav::mark_in_map`, bots.md §3.2, "In the
map"): what the player starts and weapon pads reach with walks, steps,
drops, jumps and the scenario's teleporters and lifts. A BSP with no
arena keeps its in-map sectors. `pathfind::trim` drops the rest with
their links and vertices and gives their surfaces' refs `ffffffff`, as
retail data leaves surfaces with no sector
([pathfinding.md](../formats/pathfinding.md), "Trimming"); firing
positions take their sectors' new indices.

The pinned map writer keeps existing datums, BSP load addresses and raw
resources in place. The scenario and each BSP point to their new arrays
in ordinary tag memory. The new `char` tag's entry does not fit before a
retail index's tail, so the builder moves the group table (1.4 KB) to
the end of the metadata, not the entry table (16 bytes a tag, 80 KB on
Zanzibar; `TagSet::relocate_groups`,
[map-builder.md](../formats/map-builder.md), "Adding tags to a pinned
map"). The package has one deterministic file patch, bound to the base
hash. It carries the kit version in the map's string table, so the
standard package `MapId` includes it through the patch hash.

The cost report counts each part's bytes, the sectors generated, kept
and (when merged) merged, the tag index bytes moved to the metadata, and the padding
(alignment and the page rounding of `+0x1c`), which together make the
growth in tag memory. It compares the growth with the available space
before anything is installed. A refusal says by how much the kit is over
and names its biggest part.

Available is `writer::max_metadata_size(Campaign)` (`0xc24000`, the
largest retail campaign map's, plus the arena's 8 MiB growth: the kit's
`KIT_METADATA_BUDGET`) minus the map's `+0x1c`. It is not a loader
constant: the loader takes `+0x1c` from one physical arena that is the same
size for every map type (`0x38e4000` in our runtime, retail's `0x30e4000`
plus 8 MiB), and growth comes out of the pool that gets the arena's free
pages after the load (map-format.md, "The tag arena" and "Growing the
metadata"). A campaign map loads that much beside its two shared maps,
which is more than a multiplayer map at that total beside its one shared
map, so the kit takes the retail campaign maximum rather than the
multiplayer one (`0x62c000`, the largest retail multiplayer map, which
the map editor keeps). Before the kit, on the owner's disc (**V**,
docs/halo1/notes/shared-map.md), the room under the multiplayer maximum was:
Waterworks none, Zanzibar 132 KiB, Headlong 340 KiB, Coagulation 876 KiB.
Under the retail campaign maximum Zanzibar had 6.1 MiB and Waterworks
was 164 KB short after every fit step; the arena's growth adds 8 MiB to
each. Zanzibar keeps the Phantom with its drops (`h2 firefight kit zanzibar` reports the growth and the room).
The numbers are in
[notes/2026-11-23-firefight-kit-headroom.md](notes/2026-11-23-firefight-kit-headroom.md).

Every machine in a session loads the kit map: the host's entities name
tags by index, so the indices must agree (ai-networking.md, "Rules and
limits").

#### The Flood pod

Flood squads arrive in a pod of our own, not a disc model: a fleshy seed
that falls root first and bursts open, drawn in
[content/flood-pod](../../content/flood-pod/README.md) and built by
`crates/tools/content-kit` (`kit::install`) as `bitm`, `shad`, `mode`,
`phmo`, `hlmt` and `vehi` named `objects\vehicles\flood_pod\flood_pod`.
The object is a seatless stationary vehicle, the class of Halo 2's
insertion pod. Its model has a `shell` node and four `petal` nodes hinged
on it (`content_kit::flood_pod::RIG` gives each hinge's axis and the open angle),
an `impact` marker at the tip and `squad_exit_0` to `squad_exit_3` on the
ground beside it. `h2 flood-pod kit MAP --out FILE` puts it into a disc
multiplayer map to look at in Studio. A kit with a Flood class and a zone fall builds it in
(`content_kit::flood_pod::kit::install`, in `assemble` after the classes) and parks it
as its own pool `ff_fpod_NN`; `ff_pod_NN` (human squads) and `ff_ppod_NN`
(players) stay the `insertion_pod`. A squad takes a free pod of its faction
(`Kit::flood_classes`, `PodsSeen::free_flood`, the script's `pod_flood` and
`pods_free(class)`) and falls on foot when none is free; a Flood squad never
takes an insertion pod.
Its tags count in the pods' bytes and `PodReport::tags`. A build that cannot
install it is a `PodRefused` and goes without pods. The engine has not loaded
the vehicle (**S**).

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
  `Finish`, makes the result (`Ended`: the set's name, the outcome, the
  team count, each team's score, rounds cleared, sets cleared, respawns
  issued; an endless match counts rounds across sets, and a set it was lost
  in the bonus round of is played through). The
  runtime reports each ([`runtime::Report`]) to Live telemetry, which
  writes two public game events the server takes only from the game's
  host (`live_proto::firefight`):
  - `FirefightScore`: `tick`, `player` slot, `points`, and `reason`
    `kill`, `assist` or `player_kill`. A `kill` carries the actor's
    `class`; an assist and a player kill carry none, and a player kill's
    points are the definition's `player_kill`.
  - `FirefightEnded`: `tick`, `set`, `teams` (1 or 2), `outcome`, `team`,
    `scores`, `rounds_cleared`, `sets_cleared` (left out when 0) and
    `lives_used`. `scores` has one entry
    per team, team 0's first. `outcome` is `won` (one team only), `lost`
    (one team out of lives, or the only team present of two),
    `team_won` (two teams, with `team` 0 or 1) or `draw` (two teams);
    only `team_won` names a `team`.

  In the film the tracker turns each `FirefightScore` into a
  `live_proto::film::events::Event::FirefightScore` and the player's
  running total into an `ObjectiveScore` with `game_type: "firefight"`.
  `ActorKilled` stays its own event.
- The Live projection (`live-server`'s `stats`, which the Worker
  composes, so both homes give the same answer) folds them into the
  summary's `firefight` section (`live_proto::firefight::Summary`):
  finished matches (`games`), one-team wins (`won`), rounds cleared,
  lives used, sets cleared (`sets_cleared`, with a count in each set's
  row), points (`score`), player kills (`player_kills`, every
  `player_kill` credit, bots' included), matches by wave set, kills by
  class, and a row per account (score, kills, assists, player kills,
  matches). Every finished match counts in `games`; a two-team match
  counts again in `versus` (`games`, `wins` by team, `draws`), where a
  two-team `lost` counts in `games` only. A bot has no row; its points,
  kills and player kills count in the totals and classes. A second
  `FirefightEnded` for one game counts nothing. The stats home shows the
  section once a match has been played in the window.
- Firefight scores do not enter ranks: a game that carries either event
  is never ranked (`ranks::rank_game`). Bots stay game-only, as today.
  A Firefight match is not multiplayer's kills or deaths.

### The bonus round

A definition with a `bonus` plays it after each set's last round (a set
is the definition's rounds: `script.rs::set_rounds`), before the next
set or the finish. It is part of the director script
(`content/modes/rules/firefight.rules`: `start_bonus`, `end_bonus`,
`grant_side`), so the host log carries it as commands and a replayed log
agrees.

- **Start.** When the set's last boss wave is cleared the director emits
  `Bonus { on: true, ticks }` and the phase is `Phase::Bonus { until }`.
  No `Wave` or `Round` command follows. The tick's score is noted per
  team, so the bonus round's points are what is scored after it.
- **Skulls.** Every skull of `BONUS_SKULLS` (Ghost, Catch, Thunderstorm,
  Famine, Black Eye, Mythic) that is not already on is switched on with `Skull`,
  and Iron (`Skull::Iron`, 11) with them. The script remembers which it
  turned on; at the end it turns exactly those off, so the set's own
  skulls are what remain. Iron is a director rule, not an engine skull:
  `Command::Skull { id: Iron }` is not applied to the engine
  (`host.rs`), and it takes no score multiplier.
- **Enemies.** The bonus squad is the last row of the script's `squads`
  and `waves` tables. Whenever the actor cap and a free slot allow, one is
  placed again, at most one a tick, until the time is up.
- **Deaths.** Iron means no respawn and no charge: the script issues
  neither `Respawn` nor `Lives` while it runs, and defeat is not checked.
  When every player present is dead the bonus round ends, not the game.
- **End.** On time, or when all players are dead: the squads are erased,
  the skulls it switched on go off, `Bonus { on: false }` follows, lives
  are granted by the points the team scored in it, every dead player
  respawns free, and the director goes on as after a round: the next
  round, or the finish.
- **Lives by points.** Each team takes the lives of the last
  `lives_by_points` row its bonus points reach (rows do not add). A shared
  pool gets them (`Lives` or `TeamLives`); with each player's own lives,
  every player of the team gets them (`Lives { player }`).

`tests/bonus.rs` covers the window and its length, the grant table by
kills, a bonus death, all dead, the skulls on and restored, Iron, two
teams, `zero_holds`, the validation, and 48 random matches replayed
through a fresh script for the same commands.

### Lives, rounds and skulls

- **Lives.** The pool is the director's. A player's death takes one life
  from the pool at respawn (`Respawn`, then `Lives` with the new pool);
  with the pool at zero the player waits for the wave to end and
  respawns then without a charge. Between waves an empty pool does not
  hold a respawn. With `Lives.zero_holds` the player instead waits for
  the next grant (a round's `per_round_bonus`, or the bonus round's lives,
  or its end), and between waves an empty pool holds the respawn. `Lives.shared = false` gives each player their own
  count instead, announced when the player first appears. The engine's
  own respawn stays off: the runtime sets the variant's lives to 1 at
  activation ("The host runtime"), so a death leaves the player with none
  and the engine holds the respawn, and `Respawn` gives back the one life
  ("Guest boundary"). With the variant's unlimited lives the engine
  respawned every player itself, whatever the pool said.
- **Rounds.** Five waves, the last a boss wave; an intermission between
  rounds refills ammunition crates (a kit placement the director
  re-enables) and adds `per_round_bonus` lives. An endless definition's
  last round is followed by its bonus round, then the next set's first
  round, with the same bonus.
- **Sets and the bonus round on the host.** The host runs the definition's
  rules script (`ScriptDirector`) and applies what it logs
  (`Host::apply`). `Set { index }` and `Round`, `Wave` change nothing in the
  engine: the mode blob carries them to the HUD. The round skulls a new
  set switches off arrive as `Skull { on: false }`, which the guest game
  takes out of its set and writes to the skull array. `Bonus { on, ticks }`
  sets the host's bonus flag, and while it is on the host refuses a
  `Respawn` by name ("no respawn is made in the bonus round (Iron)"): the
  script issues none and charges no life in it, and the grant at its end
  comes after `Bonus { on: false }`. `Skull { id: Iron }` is a rule, not an
  engine skull, and is not written. `Ended.sets_cleared` counts the sets
  played through (`progress`); `Ended.rounds_cleared` of an endless match
  counts every set's rounds.
- **Skulls.** Two kinds ([skulls.md](skulls.md), "Where each effect
  lives"). AI-scoped skulls (Mythic, Angry, Catch, Thunderstorm, Ghost,
  Assassins) change what actors do, and only the host runs actors, so
  only the host applies them: the reader's campaign gate is answered as
  "on" for a Firefight host. Player-scoped skulls reach every machine by
  the path multiplayer skulls already use (`live_proto::skulls`, the
  session data). The round schedule is in the definition, and an endless
  match's next set switches the previous set's round skulls off. Black
  Eye (8) is shared and `Reach::Simulation`: every machine answers its
  gate and holds its array byte whatever its role (`sim_bits`,
  `black_eye_reaches_every_machine`); Iron is a director rule with no
  engine byte (`FirefightScope::Rule`). A skull the director switches off
  leaves the array, the gate set and the multiplayer set's active bits
  (`a_skull_switched_off_leaves_the_array`).
  `skulls_mp::firefight_scope` names where each takes effect;
  `firefight_ai` gives the set a machine runs (every AI skull on a
  host or a lone machine, none on a guest), `firefight_shared` the
  player-scoped set that joins the multiplayer one, and
  `MpSkulls::write_firefight_array` writes the host's AI set into the
  engine's skull array. Each AI reader tests the options mode inline and
  jumps past its byte when it is not the campaign; a hook at each of
  those thirteen jumps (`engine::points::skull_gates`) answers the gate
  "on" for the skulls the host runs, and the native active camouflage
  asks `engine::skull_gate` ([skulls.md](skulls-modes.md#the-campaign-gate)).

### Which maps are built in

A map is built in when `h2 firefight kit MAP` makes a package for it from the
retail disc: six or more drop zones off both bases and the starts, a walkable
path to both bases, and the tag headroom for the sectors, zones and squads.
The refused map is listed in `builtin::REFUSED`; it is not built at
start, reports its reason in the lobby's kit line, and
`builtin::refusal(map)` names it. Tests: `the_builtin_list_is_the_eleven_maps_the_builder_makes_a_kit_for`,
`a_refused_retail_map_is_named_with_its_reason_and_never_read`
(`firefight-kit/src/builtin/ensure/tests.rs`). The survey is the builder's
own result on the retail disc, by `h2 firefight kit`; a map the disc does
not hold (the Multiplayer Map Pack's) is left to the user's own kit.

| Map (file) | Built in | Reason |
| --- | --- | --- |
| Coagulation (`coagulation`) | yes | authored layout, two teams |
| Zanzibar (`zanzibar`) | yes | authored layout, one team, 4 base positions |
| Ascension (`ascension`) | yes | generated, 12 drop zones |
| Beaver Creek (`beavercreek`) | yes | generated, 12 drop zones |
| Burial Mounds (`burial_mounds`) | yes | generated, 12 drop zones |
| Colossus (`colossus`) | yes | generated, 12 drop zones |
| Foundation (`foundation`) | yes | generated, 9 drop zones |
| Headlong (`headlong`) | yes | generated, 12 drop zones; 340 KB of headroom left |
| Lockout (`lockout`) | yes | generated, 12 drop zones |
| Midship (`midship`) | yes | generated, 12 drop zones |
| Ivory Tower (`cyclotron`) | no | no six drop zones: the bases' area has room for 5, and the area of 6 polygons with 2 starts has no walkable path |
| Waterworks (`waterworks`) | yes | generated; fits the grown tag arena (`waterworks_fits_the_grown_tag_arena`, disc tier) |

### Making your own kit

| Tool | Command or screen | Output |
| --- | --- | --- |
| `h2 firefight kit` | `h2 firefight kit MAP [--home red\|blue] [--disc ISO] [--data DIR] [--replace]` | `<data>/maps/firefight-MAP`, a map package |
| Studio | the map's ground: `ground.build`, then `ground.save {name?}` ([editor agent API](../editor/agent-api.md)) | `maps/<name>/map.toml` and `ground.yaml` in the edit target, and the built ground |
| Forge | place drops, bases and cover, then save the map variant ([forge-firefight.md](forge-firefight.md)) | a map variant with a ground recipe, which Change Map lists for Firefight |

### Game Setup

A custom game's Game Setup gets a **Firefight Options...** row
(`firefight/options.rs`, `ROW_ACTION` 13; the row is added the way
[bots.md](bots.md)'s Clanker Options row is, to every list that has Change
Map). `lobby_mode::arrange` keeps it only in the lobby's Firefight mode,
where it follows Change Map; Multiplayer and Forge modes hide it
([The lobby](#the-lobby)). It opens
the Firefight page, a kit screen on Quick
Options' dialog with seven choice rows, each backed by a setting
(`firefight.*`, in `setting_defs::ALL`): Firefight (on or off), Teams
(`firefight.teams`: the wave set's own, one team or two), Wave set
(`firefight.waveset`: Endless first, then the lobby map's own sets, then
the other built-ins in their order (`options::wave_menu`) — [Map-specific
sets](#map-specific-sets)). When the lobby knows the kit's classes
(`lobby_start::kit_classes`: a game on a kit map records the classes it read,
`lobby_start::learn_kit_classes`, and the lobby panel looks the map up,
`lobby_mode::panel_ids`; a map no game has played on is not known), a set that needs one the kit lacks shows the
reason in its row (`options::wave_entry`: "the map's kit lacks marine") and
the pick is refused; where they are not known every set is offered and the
host refuses the match by class name, Difficulty, Starting
lives (the wave set's own, or 1 to 30), Skulls (whether each round
switches its skull on) and Drop pods (`firefight.pods`, on by default:
players fall to their spawns in pods where the kit has them; the page always
shows the row, and `firefight.state`'s `pods.hidden` says why pods are off on a
map with no pod or no open start). A on a row opens the value picker over the page; left and right step
as a shortcut. A on Wave set pushes a carousel of cards instead
(`firefight.wavesets`, `options::wave_sets_page`): a card a set, with the
lobby map's picture, its rounds, waves and lives (`options::wave_stats`) and
its help; a set the kit lacks a class for says why and is not stored
([ui-screens.md](ui-screens.md#cards-and-the-card-row)). No row is an arrow row.

`Options::read` reads the store when a game starts and the runtime plays
that match: `Options::definition` is the wave set the value names on the
map the game started on (`builtin::resolve`, [Map-specific
sets](#map-specific-sets)), with the chosen starting
pool, played with the chosen teams (`builtin::with_teams`) and, with
skulls off, no round skull and no set skull; the difficulty goes to the
director's settings, where it scales kill points. Firefight off in Game
Setup leaves the mode idle with that reason in `firefight.state`. A
choice the store cannot give refuses the mode with the setting named. The
page and its row exist only where the `firefight` feature is on and a
settings store is present.

The choices are the host's. A client of a distributed session does not read
its own page: the definition and difficulty reach it in the session data
(FF9), and its own page's choices are not read.

### The lobby

In Custom Games, choose **Game Setup → Change Rules → Firefight** and
select a wave set. The variant record carries its definition hash. Both
activation and the director read that record, including its team count.
**Change Map** offers installed kit packages that support the wave set.
A map-specific set requires its own base map. Start Game checks the
selected map before starting the countdown.

Clankers sit only on player teams (`clankers::firefight_seat_teams`,
which tells the bots the team count with
`bots::firefight::set_seat_teams` twice a second, nothing outside
Firefight): with one team every Clanker is on team 0; with two each new
Clanker joins the team with fewer players, team 0 on a tie. The AI's
team is never a seat.
With no compatible kit installed, the map picker is empty and Start
Game reports the missing kit. Change Rules remains available.

**The built-in kits build themselves.** Eleven retail multiplayer maps are
built in (`firefight_kit::builtin::MAPS`); Coagulation and Zanzibar have
authored layouts (`firefight_kit::builtin`), the other nine are built by the
kit's generator from their own flags and starts (`Config::default()`, home
team red, no layout). No one runs `h2 firefight kit` for them. At the start of `h2 run`, `h2 serve` and `h2 play`,
`crates/apps/cli/src/firefight_kits.rs` asks
`firefight_kit::builtin::pending` which of the eleven packages
(`<data>/maps/firefight-<map>`) is missing or stale and, if any is, runs
`firefight_kit::builtin::ensure` on a worker thread while the game boots.
A package is current when its manifest is on disk and the stamp beside it
(`<data>/maps/.firefight-kit-<map>.stamp`) names the kit version and the
layout's hash; a package with no stamp, such as one `h2 firefight kit`
built, is replaced. Nothing is built, and the disc is not read, when both
are current. The kits carry the Elite only (no classes), and a map that
does not fit its tag headroom takes the fit steps of "The kit" before it is
refused. Each map's result is a report: current, built, or refused with a
reason short enough for a panel line (`Zanzibar: 6 KB over, sectors`); one
map's refusal does not stop the others. When the worker is done it reads
the map list again (`map_select::publish_packages`, which also has the VFS
read the maps folder), so the packages join the catalogue as a Live
download does.

While the worker runs (`lobby_start::KitBuild::Building`) a Firefight
lobby on a map without the kit shows `Reason::KitBuilding` ("Preparing
Firefight maps.") on the status line and "Preparing Firefight maps" over
"Start Game works when they are ready" in the variant panel; Start Game is
held. "No installed map supports Firefight yet" appears only when no kit map
is installed and no build is running, and after a failed build its second
line is the first refusal in place of the hint to run `h2 firefight kit`.
Switching to Firefight puts the lobby on Coagulation's kit, else
Zanzibar's, else the first kit map (`lobby_start::landing_map`); a lobby
already on a kit map stays. A lobby that is in Firefight on another map
when the build finishes lands the same way, once, on the next game tick
(`map_select`, "land a Firefight lobby on its kit map"), unless the game is
starting or counting down. Hosts keep the bundle path
([Live](#live)).

A map variant whose document has the `ai_ground` recipe carries the kit
once it is built, so the lobby counts the map it is held on as a kit map
(`lobby_start::KitMaps::for_pick`, from `map_variant::ground_on`), as it
counts a kit package (`lobby_start::is_kit_map`). A base map without such
a pick is refused with "Pick a map variant with AI ground, or a Firefight
map." Change Map lists the base maps that have such a variant after the
kit packages, and the Map Variant picker lists Original and a variant
without ground with "No AI ground." and does not choose them
([forge-firefight.md](forge-firefight.md), "Playing it").

### The HUD

The HUD shows what Halo 3: ODST's Firefight HUD shows, in Halo 2's pieces
and pictures of ours: the score, the lives, the set's numeral with a circle
a round and a bar a wave, a meter of the enemies left, a slot for each skull
on and a centred line for what begins
(`firefight/hud.rs`). All of it is a function of the **mode blob**,
`hud::Mode`: the stage (warmup, spawning, fighting, cleanup,
intermission, won, lost, team 0 won, team 1 won, draw), the round and
wave with their totals, whether the wave is a boss wave, the actors
alive plus those still in the reserve while a wave is on, the skulls on
(a bit a session skull index, only the ten of `hud::FIREFIGHT_SKULLS`: the nine with an engine effect and Iron),
the set (and whether the definition is endless), the bonus round's ticks
left, the wave's total actors (the enemies meter's full length; in a bonus
round its length in ticks), the one-team score, the team count, and the
lives (the shared pool, each player's own up to eight players, or each
team's pool). Each player's own lives are a mask of their roster slots
and a count each, in slot order. With two teams the blob carries both scores and a mask of
team 1's slots, so each viewer sees its own team's pool. Version 4
encodes to 26 bytes with one shared pool, 42 with eight players' own
lives, 34 with two team pools and 48 with two teams and eight players'
own lives, inside the 51 bytes left of the actor channel's 63-byte mode
state by the script role's 12-byte header
(`blob_v4_round_trips_at_every_size`). `Mode::of` leaves `bonus_left` at
0 until the host runs a bonus round. The banners for the end are "RED TEAM WINS", "BLUE TEAM WINS"
and "DRAW".

- **The host** builds the blob after each tick from the director and its
  log (`Mode::of`; the outcome is the last `Finish` in the log) and
  publishes it (`hud::slot`). The channel's capture sends it as a `State`
  record when it changes and whole in every key
  (`actor_net::host::Host::set_mode`).
- **A distributed client** keeps the mode state it receives
  (`actor_net::client::Client::mode_state`) and publishes it
  (`hud::received`) at each tick boundary.
- **Either machine** draws `hud::current()`: the blob, decoded, and
  `hud::view_on`, a pure function of the bytes, the roster slot, the scope,
  the screen and the banner up. A blob the build cannot read (wrong version, wrong length, a field out
  of range) is an error, not a guess. `the_hud_reads_only_the_mode_blob`
  plays a whole match and requires the client's view to equal the host's
  at every tick.

**The layout** (`hud/layout.rs`, `hud::layout::layout`) is a pure function
of the blob, the viewer's roster slot, the scope and the screen (full
640x480, half 640x240, quarter 320x240). The block sits at the bottom
right in the frame (x 48..592, y 36..444 at full size), right-justified,
in 14 px lines stacked up from `PLATE_TOP` (44, assumed) above the retail
score plate:

| Element | Full and half | Quarter |
|---|---|---|
| `score` | `SCORE 1234567`; two teams `RED a BLUE b`; hidden if the plate shows the total (`PLATE_SHOWS_TOTAL`) | hidden |
| `lives` | `LIVES 7`, `LAST LIFE` at 1, `NO LIVES` (warning look) at 0; the viewer's own count when unshared; none in a test or at the end | one line down |
| `progress` | words only for what the pips cannot say: `BONUS 0:42` in a bonus round, `ROUND 7 OF 12` in a definition with over three rounds (`layout::progress_text`) | one line down |
| `enemies` | a 92 by 8 meter (the battery strip): actors left over the wave's total; in a bonus round the time left | hidden |
| `set`, `round_0..2`, `wave_0..4` | the pips' row between the meter and the skulls, right-justified: the set's numeral (endless only), a circle a round (8 by 10 on a 9 px pitch; only with three rounds or fewer), a slanted bar a wave (6 by 10 on 7; none in a bonus round). Lit for the one on and those done, faint for those to come | one line down |
| `skull_0..6` | seven fixed slots over the pips, 16 px on an 18 px pitch from the right (14 px on 14 at half): Ghost, Catch, Black Eye, Thunderstorm, Famine, Mythic, Iron (no skull of ours yet). A skull on is shown in its slot, off is hidden; a slot never moves (`a_skull_slot_never_moves`). The picture is 14 px square in either size | none |
| `banner` | the game's own timed centred line, 40 px above the frame's bottom, over the respawn timer: the game places, draws and fades it | hidden |

```
                      WAVE 3                       <- banner (the game's line)
                   Respawn in 4                    <- the game's own

                                [s][s][s][s][s][s][s]   skull slots
                                    2  o o o  / / / / /   set, rounds, waves
                                   [=========----]      enemies left
                                           BONUS 0:42   only when the pips cannot say it
                                              LIVES 7
                                           SCORE 3400
                                        [ score plate ]
```

`Screen::retail` holds the weapon, grenades, tracker and plate
rectangles, assumed from the census and marked to verify in the code;
`layout::check` fails a layout with an element outside the frame, on one
of them or on another, or a skull off its slot
(`every_state_passes_the_layout_checks`,
`no_element_overlaps_a_retail_rectangle`). A monitor sees the banner
alone, a biped no lives (`the_pips_show_the_set_the_round_and_the_wave`
holds the pips in each).

**Banners** (`hud/banners.rs`). `banners::events` reads what began between
two blobs, in the order set, round, skull, wave, after what completed
(`WAVE CLEAR`, `ROUND COMPLETE`, `SET COMPLETE`) and the bonus round
(`BONUS ROUND`, `BONUS OVER 2 LIVES`); `LAST LIFE` comes before the wave.
A skull is `CATCH ON`, from its name. The match's end (`VICTORY`,
`GAME OVER`, `RED TEAM WINS`, `BLUE TEAM WINS`, `DRAW`) stays published;
the game shows its line once ("Motion").
`Banners::tick` shows each for 3 s at 30 ticks a second, easing in and
out over 200 ms; events from a newer tick replace the banner up and the
queue (`banners_queue_set_round_skull_wave`).

The HUD is the HUD kit's `firefight` HUD
(`hud_kit_core::views::firefight`, `content/modes/huds/firefight.hud.yaml`;
[hud-kit.md](hud-kit.md), "The Firefight HUD"), drawn by Halo 2's own HUD
code. It is twenty-one elements, one layout: `ff.score`, `ff.lives` and
`ff.progress` (text, right-justified, in the HUD font), `ff.enemies` (the
battery strip as a meter), `ff.set` (text), `ff.round.0` to `ff.round.2`
and `ff.wave.0` to `ff.wave.4` (gauges of two pictures, faint and lit),
`ff.skull.0` to `ff.skull.6` (gauges of two pictures, off and on) and
`ff.banner` (the kit's `Banner` channel: the game's timed centred line).
The offsets in the YAML are the layout's numbers; the half screen gives the
lines the same places, a quarter gives only the lives and progress lines
and the pips, one line lower.
`hud::state_of` reads the layout's elements into the kit's `State` and
`hud::view_on` into its `Values`, for one roster slot, scope and screen.

**Skull pictures** are ours. `font::strokes` holds each skull's glyph (the
strokes the skull menu's model picture also uses, `game::skull_glyphs`);
`font::skulls` rasterises the seven slots' skulls to 14 px, off faint and on
lit, at U+E600 to U+E60D, and the font rewrite adds them to every font file
(`bot_mark::GearRewrite`), as it adds the charge ring
(`skull_pictures_come_from_our_strokes`). The pips are ours too
(`font::skulls::Pip`, U+E60E to U+E611: a disc for a round, five discs in a
leaning row for a wave's bar; `a_pip_is_a_circle_or_a_slanted_bar_off_or_on`).
No disc or ODST art: ODST gives what is shown and in what order, not a pixel.

**Motion.** A skull switched on blinks three times in half a second and
stays lit (`Motion::Arrive`; `a_skull_blinks_three_times_when_switched_on`):
the kit restamps a value whose shown, fill or motion changed, so a skull
that goes from hidden to shown starts the blink and the others keep their
own. A pip blinks the same way as its round or wave begins, and so does
the line at the last life; no lives left shows in the warning colour, and
the meter eases. The banner is the game's timed line, so the game fades it:
`Banners` publishes each text for its three seconds, the kit writes the line
when a user's text changes (`hud_kit::banner_for`), and the game shows it for
`BANNER_TICKS`. The match's end is written once and fades like any other.

The kit's checks (`h2 hud preview firefight --state all --mode all`) hold
it in every state and screen mode, and the kit's runtime draws it. The HUD
is never shown with the superbounce game's HUDs
(`the_firefight_hud_is_never_shown_with_superbounce`), and it compiles to
records in every mode (`the_firefight_hud_compiles_to_records_in_every_mode`).
The feed (`hud::install`, the feature `firefight-hud`, added with the
`firefight` feature) registers it with the kit and, each tick, reads
`hud::current` once and views it at the roster slot of each local user's
player (`hud::local_slots`), in the screen the local user count gives (one
user full, two half, three or four quarter), one `hud::Feed` a user
(`hud::tick_all`): each feed keeps its user's last blob and banner queue.
Local user 0's values are the HUD's shared ones and each other local
user's are its own (`Handle::publish_for`), so each split-screen player
sees its own lives and its own banners (the banner channel reads each user's
own values, `a_local_users_banner_is_its_own_values`). With no match, no player for the
user or a blob it cannot read, that user's elements are hidden and its
banners forgotten; the reason for an unreadable blob is logged once. A
wave test gives the biped no lives and no "LAST LIFE" banner. A replica, a
film's playback among them, has no director: it draws the blob of the
last `ModeState` entry it applied, one tick after the host's own HUD
shows it, since the entry is for the next tick. A distributed client
gets the blob over the actor channel.

### The announcer

The HUD's mode blob diff that words the banners also picks the announcer's
lines (`firefight/hud/banners.rs`, `lines`), spoken through the queue our
medals use ([audio.md](../audio/audio.md#our-medals-announcer-lines)). Each
machine says them from the blob it reads, so a client, a replica and a
film's playback hear the same ones; split screen says them once. The clips
are `art/announcer/<id>.wav`, the owner's recordings.

| Event | Line (clip) |
|---|---|
| The match starts (round start cue) | FireFight (`firefight`) |
| A wave or boss wave starts | New Wave (`new-wave`) |
| A wave is cleared | Wave Complete (`wave-complete`) |
| A round is cleared | Round Complete (`round-complete`) |
| A set is cleared | Set Complete (`set-complete`) |
| The bonus round starts | Bonus Round (`bonus-round`) |
| The bonus round ends | Bonus Round Complete (`bonus-round-complete`) |
| The viewer's lives rise | Lives Added (`lives-added`) |
| The viewer is down to one life | Last Life (`last-life`) |
| The match is lost | Game Over (`game-over`) |
| Skulls switch off (a set or round's) | Skulls Off (`skulls-off`) |
| A skull switches on | `<name>-on`: Assassins, Thunderstorm, Famine, Ghost, Black Eye, Catch, Mythic, Angry, Iron (the bonus round's) |
| Every skull with an engine effect is on | All Skulls On (`all-skulls-on`), in place of the tick's skull lines; Iron is not counted |

Low Gravity has no line. Envy, Grunt Birthday Party, IWHBYD, Blind,
Sputnik and Bandana have clips but are not Firefight skulls: Halo 2's
readers for them are campaign-only, mid-function or local to a machine
([skulls-modes.md](skulls-modes.md#in-firefight)), so the director has
nothing to apply and they never switch on. Skulls on at the match's first blob are the
lobby's and are not said. Siege's lines are in
[rules-engine.md](rules-engine.md#siege-announcer-lines).

### Live

- **Variant.** The Slayer record names the mode in the variant
  extension's reserved bytes ([game-options.md](game-options.md), "The
  extension"), in the layout of
  [rules-engine.md](rules-engine.md#carrying-a-script): mode 1 and the
  8-byte prefix of the definition's hash. `firefight::builtin::by_prefix`
  finds the built-in set a prefix names.
- **Session data.** At each multiplayer start the runtime names the match
  the game plays (`runtime::session_firefight`: the set's name, the
  definition's id, its teams, the difficulty and `firefight::KIT_VERSION`),
  and boot hands it to a Live host's session
  (`live_hle::Hle::set_session_firefight`) as `NewSession.firefight`; a
  game that is not Firefight clears it (`clear_firefight`). Both homes
  validate, keep and return it ([live-api.md](../live/live-api.md)). A
  guest reads its host's with `Hle::host_firefight`; its runtime still
  plays its own Game Setup choices.
- **Clankers.** Live names a team only for a Clanker in a Firefight
  session (one whose advertisement carries `firefight`), and only a player
  team: `roster_bots` seats each bot by
  `live_server::bots::firefight_seats`, every one on team 0 with one team, and with
  two each on the team with fewer bots, team 0 on a tie. Any other session
  names no team, and the host balances by skill. Both homes run the same
  handler (`firefight_sessions_seat_clankers_on_player_teams_only`,
  `a_firefight_session_seats_balanced_clankers_on_the_worker`).
- **Hoppers.** A Firefight hopper entry identifies a kit map and a mode-1
  variant naming a built-in set. `hopper-edit` offers the four built-in
  sets (`choices::firefight_record`: Slayer with lives 1, no score or time
  limit, Crossfire with team play), and both `hopper-edit`'s
  `model::needs_script` and Live's hopper checks (`admin::unscripted`,
  `check_scripts_of`, `blocked_pairs`) take a mode-1 entry whose prefix
  names a built-in set, with team play for a two-team set; an unknown
  prefix is refused, and a Firefight entry has no script for the policy or
  the census to check. FF9 ships the 1-to-4-player playlist. FF13 adds the
  16-player playlist once P5 meets the host target at the chosen actor
  cap.
- **Dedicated host.** A Firefight lease names its match
  (`HostLease.firefight`) and takes a whole pack: the supervisor starts it
  only in an empty pack and admits nothing beside it, so on a small pack
  the match has a whole vCPU (ai-networking.md, "CPU on the host";
  dedicated-host.md, "Packed hosts"). The owner's bundle carries every
  multiplayer map's kit, built from their disc (`firefight/`), and the
  host places the lease's map's package before the game starts, refusing
  a map without a kit, another kit version or another package ID.
  The server-map strip blanks only bitmap and sample payloads, so a
  kit map keeps its AI blocks, sectors and moved group table
  (dedicated-host.md, "Game data bundle" and "Server maps"). Nothing in
  Live makes a Firefight lease yet: matchmaking's dedicated playlists and
  custom hosts leave `firefight` empty.
- **Join in progress** follows ai-networking.md: lobby only until probe
  P6 answers.

### Theater

The host's film replays the match, actors included, and holds every
host log entry the machine applied, the mode state among them
(`game::firefight::film`). Each is a Firefight entry record
(`live_proto::film::stream::FIREFIGHT`, tag 8): the entry's canonical
JSON at the film tick it was applied. The runtime keeps the entries each
pre-tick applies and the theater's recorder writes them into that tick
(`Tick::firefight`); a machine recording no film keeps the latest 4,096.
The public view of a film leaves them out.

A film's playback runs the mode as a replica: no director, nothing
appended. When a film tick completes the playback reads the next one and
hands its Firefight records over (`film::deliver`); the runtime's next
pre-tick takes them into the replica's log (`Host::receive`), which
applies each at its own tick, so the entries apply on the tick they did
on the host. The Theater HUD draws the recorded mode blob.
`a_film_replays_entries_at_their_ticks` plays a scripted match, records
it, writes the records through a film time chunk and replays them: the
same engine calls, the same log and, at every tick, the host's blob.
Playback starts from the match's first entry: the replica's log is
checked against a genesis made from the local Game Setup's definition
and seed, so a film whose match differs is refused at its first entry,
and a seek or rewind inside a match does not restore the replica's
log.

## Interfaces

### Agent API

| Method | Access | Purpose |
|---|---|---|
| `firefight.state` | read | whether the mode runs and why not, the role, set, round, wave, phase (the bonus round included), reserve, pool, live actors against the cap, the log head, and the entries the engine refused, and the placements no actor was ever seen for |
| `firefight.command` | act | append one director command to the log on a host, or hold or run the director; refused on a client and in playback |
| `firefight.kit` | read | whether the loaded map carries the kit, its slots, zones, classes and hunt order |

[agent-api.md](../runtime/agent-api-modes.md#firefight) has the params and
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
| Place a squad with a count | `AI_SQUAD_PLACE` (`0x201520`), behind `HS_AI_PLACE_COUNTED` (291) | `place_squad` | traced; run R1 | Evaluator 291 resolves its two arguments, keeps the `ai` value only for a plain squad (top two bits clear, `0x2a5045`), and calls `0x201520` with the count in EAX and four stack words: the squad index, start -1, 0 and 1 (`0x2a5052`); it then stores the `ai` value at AI globals `+0x364`. The one-argument overload (290) reaches the same routine through `0x273480` with the count -1, which takes the squad's normal or legendary count by difficulty (`0x2015c5`). The routine returns at once while AI globals `+1` is clear (`0x20152e`), which retail leaves clear on every multiplayer map (`ai_initialize_for_new_map`, `0x1c79e5`); the `firefight` feature's gate hook initialises a kit map's AI ("The host runtime"). Start -1 lets `AI_CHOOSE_SQUAD_START` pick each actor's start, and the pool of starts refills when it runs out (`0x203ac5`), so a count over the squad's starts reuses them. The third word, when set, skips the given start's availability check and its emitter's deferred placement action. The fourth goes to `AI_SPAWN_AT_SQUAD_START` as its flags. The later seat attempt chooses a vehicle by palette. `place_squad` counts the squad's AI units (`sim_state::visit_ai_units`, by scenario squad index, with or without a replication entity, since a local game publishes none) before and after the call and refuses a placement that added none, naming the slot, the class row and the zone; the runtime's `read_actors` counts the same way for `Observed`, and `Host::post_tick` reports a placement whose slot shows no actor 90 ticks later (`placements_unseen`). [Retail trace](notes/2026-11-24-vehicle-actions.md). |
| ... its character and zone | scenario squad `+0x36` and `+0x38`, its starts' positions | `place_squad` | traced | Nothing chooses them at the call. `AI_SPAWN_AT_SQUAD_START` reads, for each actor, the start's character (`+0x20`) or else the squad's (`+0x36`), as an index into the character palette (scenario `+0x178`, `0x2037cc`); a squad with neither places nobody. The squad's areas while it has no order come from its zone `+0x38` (`0x204f93`), and its actors appear at its starts' positions. So `place_squad` writes the palette index, the zone, and the zone's firing positions into the slot squad's starts, then places: tag memory every applying machine writes the same way. The weapon tier is not applied, since the stage-one kit authors one weapon |
| Actors' team | `AI_SET_TEAM` (`0x275b60`), behind `HS_AI_SET_TEAM` (322) | `place_squad`, after the placement | traced | The evaluator passes the `ai` value in EAX and the team in BX (`0x2a587a`). The routine accepts a squad or a group, writes the runtime squad's team byte (`+0x76`, `0x275bf4`), then each actor's team (`+0x24`, which `world.ai` reports) and its unit's (`+0x138`, `0x275c99`). Placement copies a nonzero squad team byte onto each new unit (`0x20187a`), but team 0 reads as unset there, so the body sets the team after every placement |
| Hunt | `AI_SET_ORDERS` (`0x275cb0`), behind `HS_AI_SET_ORDERS` (333) | `hunt` | built | The evaluator passes the `ai` value in EAX and the order index in CX (`0x2a5c39`). The routine ignores an index outside the scenario's order block (`+0x240`, `0x275ce6`) and hands a squad to `0x202e90`, a group to `0x203120`. There is no built-in hunt: it is an order the kit authors, which `hunt` finds by its name `ff_hunt`. The kit writes it as order 0 of an empty block (`ai::hunt_order`): no style, script or follow target, and every area of every drop zone as its primary area set, so a squad sent to it engages from across the arena. A base map that already has orders is refused, since the kit does not merge into another block's child arrays. A kit with no zone areas is refused with the Hunt order named |
| An ally's team and hold | `AI_SET_TEAM` (`0x275b60`) then `AI_SET_ORDERS` (`0x275cb0`), as above | `ally` | built; not run | The kit's `ff_hold_NN` order, found by name as `ff_hunt_NN` is (`KitLayout::hold_orders`). The team is the players' engine team, not a faction's. A base with no such order is an error naming it |
| Erase a squad | `AI_ERASE` (`0x2736c0`), behind `HS_AI_ERASE` (297) | `erase_squad` | traced; run R2 for bodies | `ai_erase` passes the `ai` value as its one stack argument (`0x2a523b`). For each actor the routine deletes the actor (`0x1e2a90`) and then its unit (`OBJECT_DELETE`) or swarm (`0x28e200`); an actor the AI is updating at that moment is marked instead (`+0x0d`, `0x2736fc`). It then walks the objects the squad placed (runtime squad `+0x70`, linked through object `+0x3a4`) and deletes each one `0xbeb30` does not keep. Whether a dead actor's body is still on that list is what R2 settles |
| Fly a ship along its path | `OBJECT_SET_TRANSFORM` (`0xb7430`), the pose of `rules::vehicles::Flight` | `fly` (`Game::fly`), `Flights` (`rules/world/vehicles/flight.rs`), `put_ship` | built; rules-world R7 and R3 | The host flies the ship along a keyed path with no AI pilot. `Game::fly` hands the match the ship, the path's points and the ticks; the match begins a flight on the tick it applies the entry and every tick after puts the ship at the flight's pose, facing the path's tangent, as a put at the boundary (`put_ship`: the object's own `up` made perpendicular to the tangent, `notify` set). The pose is 16.16 integer math, so every machine and a film put the ship in the same place. A flight that has run its ticks puts the ship on the last point once and is reported as an arrival (`Match::take_arrived`). At most 4 ships fly at once; a second flight of a ship replaces its first with no arrival. The next physics step can move the body again, with riders aboard and with contacts, which only a run settles ([R7](notes/2026-11-24-vehicle-actions.md)). Whether a client sees the ship move is rules-world R3 |
| Place a squad into a ship | `VEHICLE_LOAD` (`0x11b0c0`), `OBJECT_LIST_FROM_AI` (`0x272ea0`), `OBJECT_DELETE` | `place_squad_in` (`Game::drop_squad` through `drop_squad`) | built; rules-world R7 | Retail placement cannot seat into a given ship ([R6](notes/2026-11-24-vehicle-actions.md)), so the body places `min(count, free seats)` actors with every start of the slot squad at the ship's position, then loads the squad's actor list into the ship through the retail `vehicle_load_magic` body with the all-seats mapping. The free seats are the capacity the caller names less the bipeds attached to the ship. An actor the load left unattached is deleted, so none stands under the ship; a load that seats none is refused. The actors are read from the actors pool and not from the replicated ones, since a new actor has no entity until the tick's replication. The load's layout is read from the call at `0x2a411c`: the ship in EDX, the mapping and the list on the stack. The list cast takes the squad on the stack, as its HS caller at `0x2a5b5b` pushes it, and writes the list datum over that slot (`0x272ebf`). Every Firefight guest call is checked against its point's signature before it is made (`GuestCall::shape_error`), and a call of the wrong shape is refused by name, so a callee never reads or writes the entry's own frame |
| Hold the ship, unload it | `VEHICLE_HOVER` (`0xf0fd0`), `VEHICLE_UNLOAD` (`0x11b2b0`, mapping -1) | `ship::hover`, `ship::unload` (`Game::hover`, `Game::unload`) | named; rules-world R7 | The director's `ShipHover` and `ShipUnload`, made at the boundary as the rules runtime makes `hover` and `unload`; each is refused for a ship no longer in the world |
| Land the rest on foot | `AI_SQUAD_PLACE` | `drop_squad` | built | `drop_squad` places the remainder of the squad (`count` less those aboard) as `place_squad` does, in the same slot squad, so a wave never waits on the ship: a ship with no free seat, not in the world or whose load the engine refuses leaves the whole squad on foot, and the reason is printed |
| Credit a player's score | `SCORE_ADD` (`0x15b7c0`) | `credit_score` | traced | Slayer's kill callback is its engine's vtable slot `+0x78` (engine 2 at `0x55e4d8`), which `0x15ce70` calls last (`0x15d6a4`); it calls `0x15b7c0` with the score change in EAX and the killer's player datum on the stack (`0x2bc05d`). The routine adds the change's low 16 bits, signed, to row `+0x00` at `+0x308 + 0x1c * slot` of the game engine globals, clamps to ±30,000 (`0x15b849`), marks the statborg entity's player bit for replication (`0x15b88a`), updates the team score in a team game, checks the score limit when it is nonzero, and returns true. It refuses, returning false, without a game engine, while engine `+0x6c` is not 1, and on a machine where engine `+0xc04` is not 1 (unless game globals `+0x0c` is 4). `credit_score` passes the salted player datum and treats false as an error |
| Lives and respawn | `PLAYER_OUT_OF_LIVES` (`0x15db30`), `PLAYER_RESPAWN_READY` (`0x15dd40`), `PLAYER_ENTITY_DIRTY` (`0xa7840`), `PLAYER_SPAWN` (`0x14f2e0`) | `engine_holds_respawns`, `respawn`; `Lives` writes nothing | traced; run R3 | No routine sets lives: the engine writes player `+0x1ac` itself. Round start sets it to the variant's lives setting (game globals `+0x1b8`, 0 meaning unlimited, `0x157d03`), and a death takes one on the authority (`0x15d01e`, gated by `GAME_ENGINE_AUTHORITATIVE`), then marks the player entity's bit `0x20` for clients (`0x15d03d`; a client applies it at `0x24e257`). The players update spawns a waiting player through `PLAYER_SPAWN` only when `PLAYER_RESPAWN_READY` says so (`0x14d430`), which refuses a player `PLAYER_OUT_OF_LIVES` reports: a positive lives setting, no unit and no lives left. So the hold is the variant's lives setting at 1, and `respawn` gives one life and marks bit `0x20`; the engine respawns the player when its respawn delay runs out and takes the life at the next death. The pool stays in the mode blob, so `Lives` changes nothing in the game. A team whose players are all out of lives leaves the playing-teams mask (`0x1925c6`); whether that ends the Slayer round before the director's `Finish` is part of R3 |
| End the game | `GAME_END` (`0x1389c0`, no arguments, `ret`) | `end_game` | traced; run R4 | The routine latches game globals `+0x1128` once, sets the end delay at `+0x112c` from the tick rate and clears `+0x1129`; it checks nothing else. The engine's own round-over path ends a game in three steps (`0x15b5f9` to `0x15b624`): engine state `+0xc04` to 2, `GAME_END` unless latched, then the engine's changed flag (`+0`, bit `0x20`), skipping the first two while game globals `+0x0c` is 4. A client ends when the engine globals entity carries bit 4 (`0x24e52a`). `end_game` takes the same three steps and refuses in mode 4; whether a client ends with it is R4 |
| Line of sight from a player to a point | `AI_LINE_OF_SIGHT` (`0x29dd50`) | `line_of_sight`, from the unit's origin | traced; the eye point is open | The routine takes the two points in ESI and EDI and two objects to ignore in EDX and ECX, and runs the collision query `0x1697c0` with flags `0x1808c2d`: structure, instanced geometry and objects, the set the actor code passes at `0x1c1ab1`, `0x1f970e` and `0x292747` ([sim-rewrite-collision.md](../engine/sim-rewrite-collision.md) 5.4 for the bits). It is true when nothing is hit, or the hit is within about a third of a world unit of the target (`0x29de01`). The engine's own sight cone, `0xcc2b0` behind `objects_can_see_flag` (113), tests only the angle from a marker of the viewer it looks up through `0xb8d30` (**S**: the head). `sees` should start from the same marker; that lookup is not traced, so the sight pass starts from the unit's origin and ignores the unit itself |
| The kit on the loaded map | scenario blocks at `+0x160` squads, `+0x168` zones, `+0x178` character palette, `+0x240` orders | `read_kit`, `KitLayout::kit` | built | Squads named `ff_slot_00` to `ff_slot_15` give the slots and the team (`+0x24`); zones named `ff_drop_NN` give each zone's point, its first area's mean (`+0x24` of area 0), and its firing positions; squads named `ff_class_<name>` give each class's palette row (`+0x36`) and faction (the team, `+0x24`: 2 human, 3 Covenant, 4 Flood), kept only while the row is in the palette's row count and the team is a faction's. The offsets are the kit builder's (`h1campaign::ai::h2::layout`), and the engine reads them at the addresses cited above. The class names are the kit's, not a table in the game. The `firefight_kit_vN` string ID is not checked: the game has no reader for the loaded map's string IDs |

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

The drop ship's two checks are the rules world's R7 and R3 ([rules-world-api.md](rules-world-api.md#vehicles-and-drop-ships)),
run on the lab host with a kit map that carries a ship: R7 flies the ship
(`Game::fly`) with a squad aboard and reads the ship's position every tick
(`sim.objects`) against `Flights::step`, and the riders' parents
(`world.ai`) after the flight; R3 reads whether a second machine shows the
ship at the flown position. The load's register and stack layout is
confirmed by the same run: `place_squad_in` seats the free seats and no
unattached actor stands under the ship.

Kills come from `SimEvent::ActorKilled` (AN6), which is filled only while
the object update runs in Rust. The killer's
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
| **FF1** | `firefight`: `Def`, canonical form and hash, `validate`, the built-in definitions and the map-specific sets | S | nothing | `crates/formats/firefight/src/{def,validate,builtin}.rs` | `golden_hash_for_the_builtin_defs`, `a_wave_over_the_cap_is_refused`, `an_unknown_class_is_refused`, `canonical_form_is_stable_under_key_order`, `each_map_set_validates_compiles_and_round_trips`, `the_menu_offers_the_maps_sets_first_then_the_built_ins`, `a_store_value_resolves_per_map_and_falls_back_to_the_built_ins` | every validator rule has a refusing case; built-ins and map sets validate |
| **FF2** | The director: states, the reserve and cap, zone choice, lives, scoring, commands | M | FF1, AN1 | `crates/formats/firefight/src/{director,scoring}.rs` | `a_scripted_match_issues_the_golden_command_list`, `live_actors_never_exceed_the_cap`, `no_zone_in_sight_is_chosen_while_another_exists`, `same_seed_same_commands`, `the_pool_at_zero_holds_respawns_until_the_wave_ends`, `everyone_dead_and_no_lives_finishes_lost`, `a_replay_of_the_observed_sequence_issues_the_same_commands`, `a_resumed_mode_blob_issues_the_same_commands` | 1,000 seeded matches against a scripted `Observed` never break the cap and always end |
| **FF3** | The kit, stage 1: sectors, drop zones, zones and firing positions, squad slots, authored Elite characters, the package and its id; the `firefight-probe` test map recipe (one squad of two) | L | nothing | `crates/tools/firefight-kit/`, `crates/apps/cli/` (`h2 firefight kit`), the test-map recipe list ([test-maps.md](test-maps.md)) | `two_builds_of_one_map_are_byte_equal`, `squad_blocks_read_back_through_h1campaign`, `every_drop_zone_has_a_path_to_the_middle`, `a_map_without_headroom_is_refused_with_the_cost`, `only_the_arenas_walkable_area_keeps_sectors`, `a_live_index_tail_moves_only_the_group_table`, `the_probe_recipe_places_one_squad`, `the_kit_authors_one_named_hunt_order_over_every_area`, `hunt_spans_every_zone_and_refuses_an_arena_without_areas` | on invented maps the kit's blocks read back; the coordinator's build on a retail map and probe P2 go in notes |
| **FF4** | The host runtime over the `Game` interface: the AI switch, team setup, `Observed`, applying commands; then the squad, score and game-end entries behind it ("Guest boundary" gives each one's state); the `Game` over the guest bodies and the `firefight` feature at the tick boundary | M | FF2, FF3, AN6, P2 | `crates/extensions/game/src/firefight/{host,guest,runtime,feature}.rs`, `crates/extensions/game/src/actor_net/squads.rs`, `points.rs`, `crates/apps/boot/src/registration.rs` | `place_calls_the_squad_entry_with_class_and_count`, `a_client_applies_no_command`, `score_credits_the_killer`, `entries_apply_at_their_tick_in_seq_order`, `a_map_without_the_kit_refuses_the_mode`, `a_scripted_match_makes_the_expected_engine_calls_in_order`, `a_map_without_the_kit_is_refused_with_its_reason`, `an_operator_command_is_the_engine_call_before_the_next_tick`, `a_kit_map_starts_the_host_switches_ai_on_and_looks_at_each_zone`, `the_plan_builds_with_every_hook_switch_on` | over a fake game, a scripted match produces the expected engine calls in order; over invented guest memory, the runtime makes them through the guest bodies before the tick; boot's plan builds with the feature on |
| **FF5** | Agent API: `firefight.state`, `firefight.command`, `firefight.kit`; `lives` and `score` on `sim.view`'s `player` root | S | FF4 | `crates/extensions/agent/src/firefight.rs`, `crates/extensions/agent-proto/src/firefight.rs`, `crates/extensions/agent/src/sim/state.rs`, [agent-api.md](../runtime/agent-api-modes.md#firefight) | `state_reports_the_director`, `command_is_refused_on_a_client_and_in_playback`, `kit_reads_the_loaded_scenario`, `the_view_reports_each_players_lives`, `the_view_reports_each_players_score` | the methods answer over a fake game |
| **FF6** | Stats: `Score` and `ActorKilled` to film events, the Live projection's Firefight summary, the web stats page | M | FF2, AN6 | `crates/live/live-proto/src/{film/events,firefight}.rs`, `crates/live/live-server/`, `crates/live/live-worker/`, `web/` | `a_scripted_match_projects_to_the_golden_summary`, `the_firefight_summary_is_the_golden_one_and_players_cannot_write_it`, `the_firefight_summary_is_the_same_on_the_worker`, `firefight_scores_do_not_touch_ranks`, `a_two_team_match_projects_wins_by_team`, `credits_pair_each_kill_with_its_class_and_assist`, `a_firefight_score_is_a_game_event_and_a_running_objective_score_in_the_film`, `firefight_scores_become_running_objective_scores`, `reads the server's golden summary as it is` (web) | both homes give the same summary |
| **FF7** | Skulls: the host-only AI set, the round schedule, multipliers, the AI readers' campaign gate | S | FF4 | `crates/extensions/game/src/skulls_mp.rs`, `crates/formats/firefight/tests/skulls.rs`, `crates/hooking/guest-layout/src/points/skull_gates.rs`, `crates/rewrites/engine/src/skull_gate.rs` | `ai_skulls_apply_on_the_host_only`, `a_round_switches_its_skulls_on`, `multipliers_multiply` | skulls.md's "In Firefight" table gives each skull's scope with a test. Built, tests only, with the campaign gate answered at each AI reader's `jne` on the host (`a_hosts_ai_skulls_pass_each_readers_campaign_gate`, `every_ai_skull_gate_has_the_multiplayer_skulls_before_unless_turned_off`, `assassins_cloaks_an_actor_in_the_campaign_and_where_the_gate_is_answered`, and the XBE checks in `crates/apps/boot/tests/skull_gates_xbe.rs`). Also tested: `the_skull_array_gets_exactly_the_hosts_ai_set`, `every_firefight_skull_has_a_scope_and_an_engine_index`, `a_skull_already_on_is_not_switched_on_again`, `a_round_skull_scores_from_its_first_wave` |
| **FF8** | HUD and screens: wave banner, enemies left, lives, the set, round and wave pips, the Game Setup row and its options page (**built**) | M | FF2, AN4, AN5 | `crates/extensions/game/src/firefight/{hud,options}.rs`, `crates/rewrites/hud-kit-core/src/views/firefight.rs`, `crates/extensions/game/src/ui_kit/` | `Page` tables for the options page (`the_options_page_steps_and_opens_pickers`, `the_wave_set_row_offers_the_maps_set_first_on_its_map`, `a_map_set_value_plays_on_its_map_and_falls_back`); `the_hud_reads_only_the_mode_blob`; the kit's checks in every state and mode (`the_firefight_hud_draws_and_passes_on_the_synthetic_sheet`, `every_blob_gives_the_kits_one_layout`, `the_firefight_hud_compiles_to_records_in_every_mode`, `the_pips_show_the_set_the_round_and_the_wave`, `the_pips_fill_from_the_left`, `a_local_users_banner_is_its_own_values`) | a client with only the mode blob draws the same HUD as the host |
| **FF9** | Live: the variant bytes, `NewSession.firefight`, 1-to-4-player hopper entries, the lobby's local kit check | M | FF1 | `crates/formats/variants/src/ext.rs`, `crates/live/live-proto/`, `crates/live/live-server/`, `crates/live/live-worker/`, `crates/tools/hopper-edit/` | `the_mode_bytes_round_trip_and_leave_the_crc_valid`, `a_joiner_without_the_kit_map_is_refused_before_load`, `first_playlist_accepts_one_to_four_players`, `an_oversize_def_is_refused`, the parity suite; built: `firefight_match_round_trips_and_is_left_out_when_none`, `firefight_match_is_validated`, `sessions_carry_the_hosts_firefight_match`, `guests_read_the_firefight_match_their_host_advertises`, `each_multiplayer_start_names_its_match_for_the_session`, `each_builtin_is_found_by_its_hash_prefix`, `a_hopper_takes_the_builtin_firefight_sets`, `a_crossfire_variant_is_accepted_by_its_hash_prefix`, `the_builtin_firefight_sets_are_offered_crossfire_with_teams`, `firefight_seats_balance_two_teams_and_fill_one`, `firefight_sessions_seat_clankers_on_player_teams_only`, `a_firefight_session_seats_balanced_clankers_on_the_worker` | both homes offer 1 to 4 players and refuse unsupported sizes and missing local kits. A retail variant is unchanged |
| **FF10** | Dedicated host: the Firefight host class and admission, the kit in the host's map set, the strip keeping AI data | S | FF3, FF4, P5 | `crates/live/live-proto/src/{hosts,host_pack}.rs`, `crates/apps/cli/src/host_cmd/{pack,process}.rs`, `crates/apps/cli/src/server_map.rs`, `crates/live/host-data/`, [dedicated-host.md](../live/dedicated-host.md) | `a_firefight_match_takes_a_whole_slot_class`, `strip_keeps_squads_and_sectors`; built: `a_whole_slot_class_needs_an_empty_pack_and_keeps_it`, `a_firefight_lease_takes_the_whole_slot_class`, `the_bundle_carries_each_maps_firefight_kit_and_the_host_places_it`, `only_kit_packages_and_their_index_are_firefight_payload` | admission tests; the coordinator's host run in notes |
| **FF11** | `h2 sim run` Firefight matches: Clankers as the players, on one team or balanced across two (the `[firefight]` table: `set`, `difficulty`, `teams`, `lives`, `skulls`; examples `docs/runtime/sim-examples/firefight-beavercreek.toml` and `firefight-beavercreek-crossfire.toml`), a report with rounds, score and tick times. `host.metrics` reports `game_cpu_ms_per_tick_p99` over the last 30 steady gameplay seconds from per-tick game-thread CPU deltas | M | FF4 | `crates/live/sim-bench/`, `crates/extensions/game/src/host_metrics.rs`, [sim.md](../runtime/sim.md), [agent-api.md](../runtime/agent-api-host.md#hostmetrics) | `a_firefight_match_toml_parses`, `firefight_bots_are_on_player_teams_only`, `two_team_firefight_balances_omitted_teams`, `the_report_has_rounds_and_score`, `a_firefight_match_finds_its_kit_package`, `a_firefight_table_becomes_the_pages_choices`, `the_firefight_examples_load`, `rolling_p99_counts_individual_gameplay_intervals_and_expires_at_30s` | a match file runs headless (coordinator) and feeds P5. The p99 comes from individual gameplay samples and resets between matches |
| **FF12** | The kit, stage 2: transplanted classes, their cost, the cut order, the retail class table, `h2 firefight kit`'s `--class`, `--classes`, `--retail-classes` and `--cut`; the fit steps for a map short of headroom (**built**) | L | FF3, P2 | `crates/tools/firefight-kit/src/{transplant,retail,lib,nav,ai,testkit}.rs`, `crates/formats/pathfind/src/merge.rs`, `crates/apps/h2-tools/src/firefight_cmd.rs` | `a_class_over_the_headroom_is_refused_with_its_cost`, `a_transplanted_closure_resolves`, `two_machines_build_equal_ids`, `the_cut_order_drops_the_last_class_first`, `a_class_the_schema_cannot_copy_is_refused_by_name`, `the_table_names_each_class_once_by_its_char_path`, `each_class_takes_its_first_donor_on_the_disc`, `the_retail_table_comes_first_from_the_discs_own_maps`, `merging_keeps_the_ground_in_fewer_sector_bytes`, `a_map_short_of_headroom_takes_the_fit_steps_in_order`, `coplanar_neighbours_merge_into_one_convex_sector`, `a_merge_stops_at_a_bend_a_concave_union_and_the_ring_cap` | on invented maps and tag sets; the census is in notes. The coordinator's Zanzibar build with `--retail-classes --cut` goes in notes; the game's class names for the transplanted palette rows remain |
| **FF13** | The 16-player playlist at the live-actor cap measured by the CPU probe | S | FF9, FF10, FF11, P5 passes at 16 seats | `crates/tools/hopper-edit/`, `crates/live/live-server/`, `crates/live/live-worker/` | `sixteen_player_playlist_requires_the_measured_host_class`, `sixteen_player_definition_honours_the_measured_actor_cap`, the parity suite | publish in both Live homes only with a P5 result at or below 28 ms p99 game-thread CPU at the chosen live-actor cap on the intended host class. Admission uses that class and cap |
| **FF14** | Home bases in the kit: bases from flag stands, team starts or clusters; drops away from both with a path to each; firing positions facing the nearer base; `ff_base_NN` zones and `ff_hunt_NN` orders; starts rewritten to their base's team; `--home`; Beaver Creek's kit | M | FF3 | `crates/tools/firefight-kit/src/{bases,nav,ai,lib}.rs`, `crates/apps/h2-tools/src/firefight_cmd.rs`, `crates/extensions/game/src/firefight/guest.rs` | `home_base_is_read_from_team_starting_points`, `home_base_prefers_the_ctf_flag_stand`, `the_home_option_swaps_the_bases`, `bases_split_untagged_starts_into_two_clusters`, `starting_locations_take_their_base_team`, `drops_are_chosen_away_from_the_home_base`, `every_drop_zone_has_a_path_to_each_base`, `firing_positions_face_the_nearer_base`, `each_base_has_a_hunt_order_over_its_area`, `the_kit_writes_each_start_its_base_team`, `read_kit_finds_the_bases_and_hunt_orders` | on invented maps the bases, drops and orders read back; the coordinator's Beaver Creek build goes in notes |
| **FF15** | Two player teams: `Def::teams` and `player_kill` (format 2), validation, Crossfire, per-team pools and scores, targets, team-side zones, two-team ends, the rules-script form and its entries | M | FF2 | `crates/formats/firefight/src/{def,validate,builtin,director,script}.rs`, `crates/formats/rules/src/{caps.rs,lang/sig.rs}`, `crates/formats/firefight/tests/teams.rs` | `two_teams_start_with_a_pool_each_and_spend_their_own`, `a_team_out_of_lives_loses_to_the_other`, `both_teams_out_at_once_go_to_the_higher_score`, `both_teams_out_level_on_score_and_lives_is_a_draw`, `the_last_round_goes_to_the_higher_score_then_lives_then_a_draw`, `player_kills_score_across_the_teams_only`, `squads_are_sent_at_both_teams`, `the_team_fewer_actors_press_gets_the_next_squad`, `a_squad_drops_on_its_target_teams_side`, `a_players_team_is_fixed_when_first_seen`, `two_team_matches_always_end_with_a_team_result`, `team_counts_and_player_kill_points_are_refused_out_of_range`, `two_team_matches_give_the_same_commands`, `two_team_ends_give_the_same_commands`, `a_team_pool_set_from_outside_is_the_one_its_respawns_spend`, `pools_set_from_outside_are_refused_where_the_match_keeps_none` | the director and the script give the same commands over two-team scenarios; every two-team match ends with a team result |
| **FF16** | Clankers in Firefight: player-team seats only, balanced with two teams; actors as enemies; home-base hold, fall back, rearm, no lone drop-zone trips, regroup on low lives (**built**) | M | FF4, FF15 | `crates/live/bots/src/firefight.rs` (new), hooks in `crates/live/bots/src/{lib,mind,mind/strategy,perceive,phantom/session}.rs`, [bots.md](bots.md#310-firefight) | `firefight_seats_are_player_teams_only`, `actors_are_seen_as_targets`, `teammates_are_never_targeted`, `two_team_bots_target_the_other_team_and_actors`, `bots_hold_near_their_home_base`, `bots_regroup_when_the_pool_runs_low`, `bots_do_not_wander_to_a_drop_zone_alone`, `actors_are_read_from_the_pool` | on invented perception, bots never take the AI's team and never target a teammate |
| **FF17** | Two teams in the game: the host's team checks and third AI team, `Observed` per team and player kills, `Target`/`TeamLives`/hunt by base, HUD blob v2 with both scores, the Teams row, Clanker seats, the bots' context, the agent API's two-team state and commands | M | FF4, FF5, FF8, FF15, FF16 | `crates/extensions/game/src/firefight/{host,runtime,hud,options}.rs`, `crates/extensions/game/src/{clankers,lobby_mode,actor_net/squads}.rs`, `crates/extensions/agent-proto/src/firefight.rs`, [agent-api.md](../runtime/agent-api-modes.md#firefight) | `observed_carries_each_players_team_and_player_kills`, `zone_team_distance_is_from_each_teams_centre`, `hunt_goes_to_the_targets_base`, `the_actors_take_a_third_team_with_two_player_teams`, `two_teams_need_a_base_each`, `two_teams_need_team_play_and_a_base_each`, `the_clankers_are_told_the_teams_bases_and_lives`, `the_end_report_carries_each_teams_score_and_the_winner`, `two_teams_show_each_viewer_its_teams_lives`, `the_teams_choice_reaches_the_definition`, `clankers_never_take_the_ai_team`, `two_team_firefight_balances_clankers`, `state_reports_each_teams_pool_and_score`, `command_takes_the_two_team_commands`, `an_operator_team_pool_is_the_one_the_director_spends`, `two_teams_turn_team_play_on_before_the_game_starts`, `only_two_firefight_teams_on_a_kit_map_turn_team_play_on` | over a fake game a two-team match makes the expected calls; a client's HUD equals the host's in a two-team match |

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
- Tag headroom per map with the kit applied, and whether Zanzibar takes
  a fit step.
- That the engine paths over merged sectors as over retail ones.
- That each AI skull changes the host's actors in a match once its gate
  is answered, and that a guest sees the host's actors' result.
- Everything ai-networking.md lists.
- That a film's playback starts the match before the first Firefight
  record is handed over, and that the hook order puts the hand-over
  before the next tick's pre-tick in a real playback.
- That actors on a third team fight both player teams of a team game,
  and that a squad's `ff_hunt_NN` order makes it push into that base.
- That the engine spawns each team at the starting locations whose team
  designator the kit rewrote, and what team a CTF flag's `+0x12` field
  names on retail maps.
- That Beaver Creek yields six drops away from both bases.
- Whether team 1 reads as blue in the game, as the HUD's "BLUE" assumes.
- The HUD in a game: the lines, the meter, the pips, the skull slots and
  the banner (that the game's timed line draws over the respawn timer
  without covering it, and for a user in monitor mode)
  through `hud.render` and a live match on the lab Mac, clear of the retail
  score plate, weapon, grenades and tracker in every screen mode (the
  rectangles and the plate's 44 px are assumed from the census), and the
  battery strip's 92 by 8 on a multiplayer map.
- That team play turned on at `GAME_ACTIVATE` (the scenario is loaded
  then, so the kit reads) makes a team game of a lobby that was free for
  all, which team each person then plays on, and that a guest's game
  agrees with its host's.
- That the sim finds the kit package's scenario path and the bots load
  Beaver Creek's nav mesh on it.
- That the game's actor positions, as the Clankers read them, match what
  a client sees, and the bots' home and regroup radii in real matches.

## Related

- [ai-networking.md](ai-networking.md): the network model, budgets and probes.
- [forge-design.md](forge-design.md): the host log's other user.
- [forge-firefight.md](forge-firefight.md): Firefight maps built in Forge: placed drops, bases and cover, a wave set in the map variant, a wave test in the session.
- [skulls.md](skulls.md), [game-options.md](game-options.md), [bots.md](bots.md), [maps.md](maps.md).
- [halo1/ai.md](../halo1/ai.md): the AI blocks the kit writes.
- [dedicated-host.md](../live/dedicated-host.md), [live-matchmaking.md](../live/live-matchmaking.md).
