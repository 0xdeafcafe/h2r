# Firefight

Status: Partly built. The definition format, validator, three built-in wave
sets, director (FF2), stage-one offline kit and probe recipe are
implemented. Retail play and the network probes await verification.
Code: `crates/formats/firefight` defines and validates wave sets and holds
the director (`src/director.rs`) and kill scoring (`src/scoring.rs`).
`crates/tools/firefight-kit` builds the map kit. The offline command lives
in `crates/apps/cli/src/firefight_cmd.rs`. The slice table lists the planned
host runtime, HUD and Live code. Tests use invented data.
Switch: the `firefight` feature of `H2_FEATURES` (off until slice FF4 is seen in the game); it joins [switches.md](../runtime/switches.md) with the code.

Firefight is a co-operative mode on multiplayer maps. One team holds
out against waves of AI, with shared lives, rounds, skulls and a score.
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

## What the player gets

- A **Firefight** row in Game Setup and in matchmaking: pick a map, a
  wave set, a difficulty, lives and skulls.
- Waves arrive from the map's edges. Five waves make a round, three
  rounds a set; each round switches on another skull. A boss wave ends a
  round.
- Players share a pool of lives. A dead player respawns from the pool at
  the end of the wave when the pool is empty, and the match ends when
  everyone is dead with no lives left.
- Kills score points by enemy, difficulty and skulls. The scoreboard,
  the post-game report and the Live stats page show them.
- Clankers can fill seats: they are players ([bots.md](bots.md)).

## How it works

### The game engine underneath

A Firefight match is a retail **Slayer** game on one team, with our
variant extension naming the mode. That gives, with no new replication:

| Need | Retail part | Evidence |
|---|---|---|
| Scores on every machine's scoreboard and report | the statborg entity (type 7) | **E**, [notes/ai-networking.md](notes/ai-networking.md) |
| Lives per player | the game-engine player entity carries lives remaining | **E** (a field label in the image); **S** that Slayer's lives setting drives it |
| Rounds and the round timer | the game engine globals entity | **E** (field labels) |
| Respawn, weapons on the map, vehicles | the variant | **Code**, `crates/formats/variants` |

The host turns off what Slayer would do by itself: the score to win is
unreachable, team killing scores nothing, and the game ends only when
the director says so (a `Finish` command through the engine's own game
end).

### The definition

`firefight::Def` is a versioned document. `Def::canonical_json` uses
`content`'s canonical form, as Forge does. `Def::id` is the SHA-256
of those UTF-8 bytes, including `format`. Object key order and input
whitespace can vary. Array order is part of the definition. Enemies are
named by **class** (`grunt`, `elite`, `jackal`, `hunter`, ...) and the
kit maps each class to a character on the machine.

```
Def      { format: 1, name, rounds: [Round], lives: Lives, caps: Caps, scoring: Scoring }
Round    { waves: [Wave], skulls_on: [Skull] }
Wave     { squads: [SquadSpec], boss: bool, gap_ticks }
SquadSpec{ class, count: {per_player, min, max}, drop: Drop, weapon_tier }
Lives    { pool_start, per_round_bonus, shared: bool }
Caps     { live_actors_by_players: [(players, actors)] }
Scoring  { by_class, difficulty_multiplier, skull_multipliers, assist }
```

`Def::from_bytes(bytes, &Limits)` admits wire data. It checks the 64 KiB
limit before parsing, bounds collections as it reads them, and refuses
unknown fields and duplicate object keys. `validate(&Def, &Limits)`
applies the same format and admission rules to an authored definition.
`Limits` supplies the target kit's class list. Every named class must be
in that list and every squad class must have a score entry. Canonical
output and hashes depend on the definition alone.

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
Stand. Each has three rounds and uses the stage-one kit's Elite class.

### The director

`firefight::Director` (`crates/formats/firefight/src/director.rs`) is a
state machine with no I/O:

```rust
pub fn new(def: Def, settings: Settings) -> Result<Director>;   // Settings { seed, difficulty, skulls }
pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command>;
pub struct Observed { players: Vec<PlayerSeen { slot, alive }>,
                      actors_alive_by_squad: [u16; 16],
                      zones: Vec<ZoneSeen { seen, distance }>,
                      kills: Vec<Kill { class, killer, assist }> }
pub enum Command { PlaceSquad { slot, class, count, zone, weapon_tier }, Hunt { slot },
                   EraseSquad { slot }, Round { index }, Wave { index }, Skull { id, on },
                   Lives { player, pool }, Score { player, points, reason },
                   Respawn { player }, Finish { outcome } }
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
  the players' centre in any fixed integer unit.
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
  Clearing the last round's boss wave issues `Finish { won }`.
- Every command is a host log entry (`host_log::Entry<Command>`,
  ai-networking.md, "How it works"). Commands serialize to integer-only
  JSON, so the log's canonical form applies. The runtime applies entries
  at the next tick in sequence order; the director never touches the
  game.
- Its state (`director::State`: phase, round, wave, the reserve's next
  squad, slots, pool, players, active skulls, generator) is the mode
  blob the actor channel sends as `State`. `Director::resume` continues
  from it.

### The host runtime

`crates/extensions/game/src/firefight/host.rs`, on a distributed server
(and on every machine of a synchronous session):

1. At map load: check the map carries the kit (below); set the AI
   globals' on byte; put actors on a team no player holds.
2. Each tick, after the game tick: build `Observed` through `sim_state`
   and `world.ai`'s actor view; call `Director::step`; append the
   commands to the log.
3. At the start of each tick: apply the entries due. `PlaceSquad` and
   `EraseSquad` go through the squad entry shared with Forge
   (`actor_net/squads.rs`). `Score` credits the statborg through the
   engine's score entry. `Lives` and `Respawn` write through the game
   engine's player entries. `Skull` goes to the skulls layer.
4. `SimEvent::ActorKilled` (ai-networking.md, slice AN6) feeds scoring:
   the kill becomes a `Score` command on the next step.

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
h2 firefight kit coagulation --probe
```

The first command installs `<data>/maps/firefight-coagulation`. The second
builds the `firefight-probe` recipe, which puts two Elites in one squad
and adds a startup script that calls `ai_place` for it. The regular kit
keeps its sixteen squads empty. The [test-map API](test-maps.md) can
also create and list the probe on a running host.

`firefight-kit/src/nav.rs` generates pathfinding for every structure BSP.
It chooses 6–12 boundary sectors on a connected component accessible from
player starts. A route to its middle must use shared sector edges with
enough clearance for the bots' walk mesh. Each zone has an area and
separated firing positions sampled off the player starts. The kit refuses
maps without six suitable drops or without the required local tags.

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
- In the game: the statborg holds each player's score, so the scoreboard
  and the carnage report need nothing new.
- In the stats stream: each `Score` command is a film event
  (`live_proto::film::events::Event::ObjectiveScore` with
  `game_type: "firefight"`), and `ActorKilled` is its own event. The
  Live projection gains a Firefight summary: set, rounds cleared, score
  by player, kills by class, lives used.
- Firefight scores do not enter ranks. Bots stay game-only, as today.

### Lives, rounds and skulls

- **Lives.** The pool is the director's. A player's death takes one life
  from the pool at respawn (`Respawn`, then `Lives` with the new pool);
  with the pool at zero the player waits for the wave to end and
  respawns then without a charge. Between waves an empty pool does not
  hold a respawn. `Lives.shared = false` gives each player their own
  count instead, announced when the player first appears. The engine's
  own respawn stays off; a player respawns only on `Respawn`.
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

### Live

- **Variant.** The Slayer record carries two of the variant extension's
  reserved bytes ([game-options.md](game-options.md), "The extension"):
  a mode id and a definition-hash prefix. The definition itself travels
  in the session data beside the extension (`NewSession.firefight`), in
  both homes.
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
| `firefight.state` | read | round, wave, state, reserve, pool, live actors against the cap, the log head |
| `firefight.command` | act | append one command to the log on a host (place a squad, skip a wave); refused on a client and in playback |
| `firefight.kit` | read | whether the loaded map carries the kit, its version, zones and classes |

### Guest boundary

Research points to trace before FF4; each is an entry the engine already
has.

| Point | Start from |
|---|---|
| Place a squad's actors, with a count | the two-argument `ai_place` evaluator at `0x2a5010` ([ai-control.md](../halo1/ai-control.md)) |
| Erase a squad | the `ai_erase` evaluator |
| Credit a player's score | the statborg's writers ([live-gamestate.md](../live/live-gamestate.md) section 5) |
| End the game | the engine's own game end |
| Line of sight from a player to a point | the visibility test the AI uses |

## Rules and limits

- One team of players. Versus variants are out of scope.
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
| **FF3** | The kit, stage 1: sectors, drop zones, zones and firing positions, squad slots, authored Elite characters, the package and its id; the `firefight-probe` test map recipe (one squad of two) | L | nothing | `crates/tools/firefight-kit/`, `crates/apps/cli/` (`h2 firefight kit`), the test-map recipe list ([test-maps.md](test-maps.md)) | `two_builds_of_one_map_are_byte_equal`, `squad_blocks_read_back_through_h1campaign`, `every_drop_zone_has_a_path_to_the_middle`, `a_map_without_headroom_is_refused_with_the_cost`, `the_probe_recipe_places_one_squad` | on invented maps the kit's blocks read back; the coordinator's build on a retail map and probe P2 go in notes |
| **FF4** | The host runtime: the AI switch, team setup, `Observed`, applying commands, the squad, score and game-end entries (traced first) | M | FF2, FF3, AN6, P2 | `crates/extensions/game/src/firefight/host.rs`, `crates/extensions/game/src/actor_net/squads.rs`, `points.rs` | `place_calls_the_squad_entry_with_class_and_count`, `a_client_applies_no_command`, `score_credits_the_killer`, `entries_apply_at_their_tick_in_seq_order`, `a_map_without_the_kit_refuses_the_mode` | over a fake game, a scripted match produces the expected engine calls in order |
| **FF5** | Agent API: `firefight.state`, `firefight.command`, `firefight.kit` | S | FF4 | `crates/extensions/agent/src/firefight.rs`, [agent-api.md](../runtime/agent-api.md) | `state_reports_the_director`, `command_is_refused_on_a_client_and_in_playback` | the methods answer over a fake game |
| **FF6** | Stats: `Score` and `ActorKilled` to film events, the Live projection's Firefight summary, the web stats page | M | FF2, AN6 | `crates/live/live-proto/src/{film/events,firefight}.rs`, `crates/live/live-server/`, `crates/live/live-worker/`, `web/` | `a_scripted_match_projects_to_the_golden_summary`, `firefight_scores_do_not_touch_ranks`, the parity suite | both homes give the same summary |
| **FF7** | Skulls: the host-only AI set, the round schedule, multipliers | S | FF4 | `crates/extensions/game/src/skulls_mp.rs`, `firefight/host.rs` | `ai_skulls_apply_on_the_host_only`, `a_round_switches_its_skulls_on`, `multipliers_multiply` | skulls.md's table gains a Firefight column, each cell with a test |
| **FF8** | HUD and screens: wave banner, enemies left, lives, the Game Setup row and its options page | M | FF2, AN4, AN5 | `crates/extensions/game/src/firefight/hud.rs`, `crates/extensions/game/src/ui_kit/` | `Page` tables for the options page; `the_hud_reads_only_the_mode_blob`; the kit's layout checks in every state | a client with only the mode blob draws the same HUD as the host |
| **FF9** | Live: the variant bytes, `NewSession.firefight`, 1-to-4-player hopper entries, the lobby's local kit check | M | FF1 | `crates/formats/variants/src/ext.rs`, `crates/live/live-proto/`, `crates/live/live-server/`, `crates/live/live-worker/`, `crates/tools/hopper-edit/` | `the_mode_bytes_round_trip_and_leave_the_crc_valid`, `a_joiner_without_the_kit_map_is_refused_before_load`, `first_playlist_accepts_one_to_four_players`, `an_oversize_def_is_refused`, the parity suite | both homes offer 1 to 4 players and refuse unsupported sizes and missing local kits. A retail variant is unchanged |
| **FF10** | Dedicated host: the Firefight host class and admission, the kit in the host's map set, the strip keeping AI data | S | FF3, FF4, P5 | `crates/apps/h2host/`, `crates/live/host-data/`, [dedicated-host.md](../live/dedicated-host.md) | `a_firefight_match_takes_a_whole_slot_class`, `strip_keeps_squads_and_sectors` | admission tests; the coordinator's host run in notes |
| **FF11** | `h2 sim run` Firefight matches: Clankers as the players, a report with rounds, score and tick times. `host.metrics` reports `game_cpu_ms_per_tick_p99` over the last 30 steady gameplay seconds from per-tick game-thread CPU deltas | M | FF4 | `crates/live/sim-bench/`, `crates/extensions/game/src/host_metrics.rs`, [sim.md](../runtime/sim.md), [agent-api.md](../runtime/agent-api.md#hostmetrics) | `a_firefight_match_toml_parses`, `the_report_has_rounds_and_score`, `game_cpu_p99_uses_individual_tick_deltas` | a match file runs headless (coordinator) and feeds P5. The p99 comes from individual gameplay samples and resets between matches |
| **FF12** | The kit, stage 2: transplanted classes, their cost, the cut order | L | FF3, P2 | `crates/tools/firefight-kit/src/transplant.rs` | `a_class_over_the_headroom_is_refused_with_its_cost`, `a_transplanted_closure_resolves`, `two_machines_build_equal_ids` | on invented maps; the retail census goes in notes |
| **FF13** | The 16-player playlist at the live-actor cap measured by the CPU probe | S | FF9, FF10, FF11, P5 passes at 16 seats | `crates/tools/hopper-edit/`, `crates/live/live-server/`, `crates/live/live-worker/` | `sixteen_player_playlist_requires_the_measured_host_class`, `sixteen_player_definition_honours_the_measured_actor_cap`, the parity suite | publish in both Live homes only with a P5 result at or below 28 ms p99 game-thread CPU at the chosen live-actor cap on the intended host class. Admission uses that class and cap |

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

## Related

- [ai-networking.md](ai-networking.md): the network model, budgets and probes.
- [forge-design.md](forge-design.md): the host log's other user; a Forge wave graph and the director issue the same squad command.
- [skulls.md](skulls.md), [game-options.md](game-options.md), [bots.md](bots.md), [maps.md](maps.md).
- [halo1/ai.md](../halo1/ai.md): the AI blocks the kit writes.
- [dedicated-host.md](../live/dedicated-host.md), [live-matchmaking.md](../live/live-matchmaking.md).
