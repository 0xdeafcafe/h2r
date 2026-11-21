# Firefight

Status: Design. Nothing here is built.
Code (planned): `crates/formats/firefight` (the definition, the director), `crates/tools/firefight-kit` (the map kit), `crates/extensions/game/src/firefight/` (the host runtime, the HUD), `crates/live/live-proto/src/firefight.rs`. Tests: each crate's own, on invented data.
Switch: the `firefight` feature of `H2_FEATURES` (off until slice FF4 is seen in the game); it joins [switches.md](../runtime/switches.md) with the code.

Firefight is a co-operative mode on multiplayer maps: up to 16 players on
one team hold out against waves of AI, with shared lives, rounds, skulls
and a score. It runs on Live and on dedicated hosts in the distributed
model with host actors ([ai-networking.md](ai-networking.md)). The rule
that must hold: **the director is a pure function of the match**. Given
the definition, the seed and what it reads from the game each tick, it
issues the same commands on every run, so a film replays and a
synchronous session agrees.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

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

`firefight::Def` is a document in canonical JSON, hashed the way a Forge
document is (`content`'s canonical form). It holds no game data: enemies
are named by **class** (`grunt`, `elite`, `jackal`, `hunter`, ...) and
the kit maps a class to a character on the machine.

```
Def      { name, rounds: [Round], lives: Lives, caps: Caps, scoring: Scoring }
Round    { waves: [Wave], skulls_on: [Skull] }
Wave     { squads: [SquadSpec], boss: bool, gap_ticks }
SquadSpec{ class, count: {per_player, min, max}, drop: Drop, weapon_tier }
Lives    { pool_start, per_round_bonus, shared: bool }
Caps     { live_actors_by_players: [(players, actors)] }
Scoring  { by_class, difficulty_multiplier, skull_multipliers, assist }
```

`validate(&Def, &Limits)` refuses a definition whose worst wave exceeds
the live actor cap of [ai-networking.md](ai-networking.md) ("CPU on the
host") or names a class outside the kit's list.

### The director

`firefight::Director` is a state machine with no I/O:

```rust
pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command>;
pub struct Observed { players_alive, players_total, actors_alive_by_squad, zones_seen_by_players }
pub enum Command { PlaceSquad { slot, class, count, zone }, EraseSquad { slot },
                   Round { index }, Wave { index }, Skull { id, on }, Lives { pool },
                   Score { player, points, reason }, Respawn { player }, Finish { outcome } }
```

- States: `Warmup`, `Spawning`, `Fighting`, `Cleanup` (the last few
  actors are sent at the players, then erased after a timeout),
  `Intermission`, `Done`.
- A wave's squads are placed a few at a time so that live actors stay at
  or under the cap for the current player count; the rest wait in a
  reserve and enter as others die.
- **Where a squad enters.** The kit gives each map drop zones. The
  director picks, with its seeded generator, among zones no player has
  line of sight to (`zones_seen_by_players`, which the host fills from
  the engine's own visibility test) and prefers the one farthest from
  the players' centre.
- Every command is a host log entry (`host_log::Entry<Command>`,
  ai-networking.md, "How it works"). The runtime applies entries; the
  director never touches the game.
- Its state is small (round, wave, reserve, pool, generator state) and
  is the mode blob the actor channel sends as `State`.

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
([maps.md](maps.md)) whose id is derived from the base map's hash and
the kit's version, so every machine that builds it gets the same id.

| Part | Built with | Notes |
|---|---|---|
| Sectors in each `sbsp` | `pathfind::generate` | the generator the bots' mesh starts from, which has run on every retail multiplayer map (**Code**, `crates/live/bots/src/cache.rs`); the engine has not yet loaded its output (**S**) |
| Drop zones | the bots' nav mesh (`bots::autonav`): sectors near the map's edge, off the players' spawn clusters, with a path to the middle | 6 to 12 a map; written as zones and areas |
| Zones, areas, firing positions | sampled from the sectors: cover is a position with a wall toward the middle | the block writers of `h1campaign::ai` (`ScenarioBlocks`) |
| Squad slots | 16 empty squads the director fills (class, count and zone are set at place time) | the same writers |
| Characters, stage 1 | `char` tags we author from a tuning table of ours, on bipeds and weapons the multiplayer map already has | Elites only; no tag is copied from another map |
| Characters, stage 2 | `tags::schema::transplant` from the player's own campaign maps, with the sound gestalt merge | each class costs tag headroom; `validate` refuses a class that does not fit and the builder reports the cost |

Tag headroom on multiplayer maps is 0.1 to 1.5 MiB and one map has none
(**S**, forge-design.md, "The kit"). The kit builder prints what each
part costs, and a map with no room for stage 1 is not offered.

Every machine in a session loads the kit map: the host's entities name
tags by index, so the indices must agree (ai-networking.md, "Rules and
limits").

### Scoring and stats

- `Scoring` gives points by class; the multiplier is difficulty times
  the product of the active skulls' multipliers. Assists share a
  fraction when two players damaged the actor in its last five seconds.
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
  from the pool at respawn; with the pool at zero the player waits for
  the wave to end. `Lives.shared = false` gives each player their own
  count instead.
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
- **Hoppers.** A Firefight hopper entry is a kit map and the Firefight
  variant; `hopper-edit` offers the built-in definitions. Party size 1
  to 4 for the first playlist, 16 for a "Big Team" one once the host
  budget allows it.
- **Dedicated host.** A Firefight match asks for a host of its own class
  with a whole core (ai-networking.md, "CPU on the host"). The host
  builds or downloads the kit map at boot like any map package; the
  server-map strip keeps the AI blocks and sectors.
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
- No creatures; no class the kit could not fit.
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
| **FF2** | The director: states, the reserve and cap, zone choice, lives, scoring, commands | M | FF1, AN1 | `crates/formats/firefight/src/{director,scoring}.rs` | `a_scripted_match_issues_the_golden_command_list`, `live_actors_never_exceed_the_cap`, `no_zone_in_sight_is_chosen_while_another_exists`, `same_seed_same_commands`, `the_pool_at_zero_holds_respawns_until_the_wave_ends`, `everyone_dead_and_no_lives_finishes_lost` | 1,000 seeded matches against a scripted `Observed` never break the cap and always end |
| **FF3** | The kit, stage 1: sectors, drop zones, zones and firing positions, squad slots, authored Elite characters, the package and its id; the `firefight-probe` test map recipe (one squad of two) | L | nothing | `crates/tools/firefight-kit/`, `crates/apps/cli/` (`h2 firefight kit`), the test-map recipe list ([test-maps.md](test-maps.md)) | `two_builds_of_one_map_are_byte_equal`, `squad_blocks_read_back_through_h1campaign`, `every_drop_zone_has_a_path_to_the_middle`, `a_map_without_headroom_is_refused_with_the_cost`, `the_probe_recipe_places_one_squad` | on invented maps the kit's blocks read back; the coordinator's build on a retail map and probe P2 go in notes |
| **FF4** | The host runtime: the AI switch, team setup, `Observed`, applying commands, the squad, score and game-end entries (traced first) | M | FF2, FF3, AN6, P2 | `crates/extensions/game/src/firefight/host.rs`, `crates/extensions/game/src/actor_net/squads.rs`, `points.rs` | `place_calls_the_squad_entry_with_class_and_count`, `a_client_applies_no_command`, `score_credits_the_killer`, `entries_apply_at_their_tick_in_seq_order`, `a_map_without_the_kit_refuses_the_mode` | over a fake game, a scripted match produces the expected engine calls in order |
| **FF5** | Agent API: `firefight.state`, `firefight.command`, `firefight.kit` | S | FF4 | `crates/extensions/agent/src/firefight.rs`, [agent-api.md](../runtime/agent-api.md) | `state_reports_the_director`, `command_is_refused_on_a_client_and_in_playback` | the methods answer over a fake game |
| **FF6** | Stats: `Score` and `ActorKilled` to film events, the Live projection's Firefight summary, the web stats page | M | FF2, AN6 | `crates/live/live-proto/src/{film/events,firefight}.rs`, `crates/live/live-server/`, `crates/live/live-worker/`, `web/` | `a_scripted_match_projects_to_the_golden_summary`, `firefight_scores_do_not_touch_ranks`, the parity suite | both homes give the same summary |
| **FF7** | Skulls: the host-only AI set, the round schedule, multipliers | S | FF4 | `crates/extensions/game/src/skulls_mp.rs`, `firefight/host.rs` | `ai_skulls_apply_on_the_host_only`, `a_round_switches_its_skulls_on`, `multipliers_multiply` | skulls.md's table gains a Firefight column, each cell with a test |
| **FF8** | HUD and screens: wave banner, enemies left, lives, the Game Setup row and its options page | M | FF2, AN4, AN5 | `crates/extensions/game/src/firefight/hud.rs`, `crates/extensions/game/src/ui_kit/` | `Page` tables for the options page; `the_hud_reads_only_the_mode_blob`; the kit's layout checks in every state | a client with only the mode blob draws the same HUD as the host |
| **FF9** | Live: the variant bytes, `NewSession.firefight`, hopper entries, the lobby's kit check | M | FF1 | `crates/formats/variants/src/ext.rs`, `crates/live/live-proto/`, `crates/live/live-server/`, `crates/live/live-worker/`, `crates/tools/hopper-edit/` | `the_mode_bytes_round_trip_and_leave_the_crc_valid`, `a_joiner_without_the_kit_map_is_refused_before_load`, `an_oversize_def_is_refused`, the parity suite | both homes agree; a retail variant is unchanged |
| **FF10** | Dedicated host: the Firefight host class and admission, the kit in the host's map set, the strip keeping AI data | S | FF3, FF4, P5 | `crates/apps/h2host/`, `crates/live/host-data/`, [dedicated-host.md](../live/dedicated-host.md) | `a_firefight_match_takes_a_whole_slot_class`, `strip_keeps_squads_and_sectors` | admission tests; the coordinator's host run in notes |
| **FF11** | `h2 sim run` Firefight matches: Clankers as the players, a report with rounds, score and tick times | S | FF4 | `crates/live/sim-bench/`, [sim.md](../runtime/sim.md) | `a_firefight_match_toml_parses`, `the_report_has_rounds_and_score` | a match file runs headless (coordinator) and feeds P5 |
| **FF12** | The kit, stage 2: transplanted classes, their cost, the cut order | L | FF3, P2 | `crates/tools/firefight-kit/src/transplant.rs` | `a_class_over_the_headroom_is_refused_with_its_cost`, `a_transplanted_closure_resolves`, `two_machines_build_equal_ids` | on invented maps; the retail census goes in notes |

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
