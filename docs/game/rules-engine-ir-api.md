# Rules engine: IR, engine API and roles

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: the IR and VM, the named engine entries, capabilities and role powers.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## The IR and the VM

**Shape.** The blob is: a header (magic, IR version, API version,
name, version, type byte, the `uses` byte (the retail type, `0xff` for
none), team requirement, the `points` label, the `about` text, and the
required capabilities by name, at most 32, "Roles, powers and
capabilities"); a string table; an option table (type, range, default,
label); the settings (`ir::Setting`: a `u16` index into
`settings::TABLE` and an `i32` value, in ascending index order); the
Quick Options marks (`ir::Quick`: a kind byte, 0 a table index and 1 an
option index, and a `u16`, at most five, in the script's order); a slot
table (scope, type, initial value); a
label table (each label's text, kind, member count `min` and `max`,
flags for single, numbered, number range and per team, and the number
range) and a tag table (each path and its group: any object, or weapon),
which the start gate checks a map against
([rules-language-types.md](rules-language-types.md#the-ir)); constant
tables; a handler table
(event, argument slots); and code. Code is a register bytecode with typed ops:
arithmetic, compare, slot load and store, forward conditional jump,
`ForEach` over a set with a body range, and `Call` of a numbered API
entry. There is no backward jump and no indirect jump, so termination is
a property of the format, and the verifier needs no fixpoint.

**Settings and marks.** `Ir::decode` refuses settings out of order or
named twice, more than five marks, a mark kind other than 0 or 1, a type
byte past the table and a `uses` byte that is not a retail type or `0xff`. The verifier (`verify::settings_and_marks`)
refuses an `about` over 150 UTF-16 units, a setting index past the
table, a setting that does not apply to the header's type or whose
value is outside its row's range, a mark that names no record setting of
the type or no option, a mark twice, and a `uses` blob with
handlers, slots or options. So a blob from anywhere holds to the rules
the compiler applied. `Ir::id` hashes the whole encoding, so the
settings, the marks, the description and the `uses` byte are part of
the script's identity.

**Ops** (IR version 1, `crates/formats/rules/src/ir.rs`): `Const`,
`Move`, `Arith` (add, subtract, multiply, divide, remainder; saturating
`int` or 16.16 `fixed`), `Neg`, `Cmp`, `Not`, `Load` and `Store` (a slot,
through an owner register for a scoped slot), `Opt` (a script option),
`Variant` (a record field), `TableRead`, `TableFind`, `Jump`, `JumpIf`,
`JumpIfNot`, `ForEach`, `Call`, `Grant`, `Revoke` and `Granted`. Each
handler declares the type of every register, so the verifier checks
types op by op, and a jump inside a `ForEach` body cannot leave it. A
read through `none` gives the slot's initial value and a write through
`none` does nothing.

**Budget.** A tick may execute 100,000 ops across all its handlers, about
1.8 ms on an M-series host at 18 ns an op (the `op_cost` measurement in
`crates/formats/rules/tests/scripts.rs`). The
count is always enforced and nothing is marked proven: a handler's worst
case follows from the set bounds (16 players, 8 teams, and a labelled
set's label `max`, at most 256; the VM visits no more than that on any
map), but the engine decides how many kills or zone entries a tick
holds. Events are not capped to make a proof, because a synchronous
handler ("Order") moved to the next tick moves a score off the tick
retail lands it on, and a dropped kill is a wrong score. One cap changes
no retail rule: `on enter` and `on leave` together run at most 64 times
a tick, the rest carried over in zone, then object order. The verifier
reports a **worst-tick estimate** (each handler's worst case times one
event per player, 64 zone events, every timer and `on tick`). Studio
shows it and the allow-list requires it to fit; it assumes one kill,
death or spawn per player a tick, which is not verified. Going over is a
**script error**: it stops the VM, ends the round through `GAME_END`
with the outcome "script error", and is named in the log and the
post-game report. It happens at the same op on every run, so a film
replays it.

**Order.** Handlers run where their event happens, so a script's calls
land at the point in the tick where the lifted engine's did:

| Handlers | Run | Their calls |
|---|---|---|
| `on kill`, and each event the carrier engine has a callback for (per port: death, spawn, pickup, drop, join, quit, round start and end) | synchronously, in place of that callback | made at once |
| `on enter`, `on leave`, `on press`, `on timer` (slot order), then `on tick` | synchronously, in place of the carrier's per-tick update (`0x15c150`) | made at once |
| `on squad_dead`, `on actor_kill`, `on actor_assist`, `on damage`, `on touchdown`, `on launch`, `on grounded`, `on bounce`, `on bounced`; `on init` | after `GAME_TICK`, from what the tick's observers gathered; `init` after `GAME_ACTIVATE` | queued |

An event the carrier has no callback for runs with the per-tick group.
Some calls are queued from any handler and made in program order by the
`before` on the next `GAME_TICK`: squad commands, which are host log
entries that replicas and films apply at the tick boundary (firefight.md
"The director"); and object creation, deletion and teleports, not known
to be legal inside a damage or AI update (`AI_ERASE` only marks an actor
the AI is updating, `0x2736fc`). A read sees every call made so far and
no queued one, and a queued call returns nothing to the script. `for
each` visits players by slot index, teams by index, objects by label
then placement id.

**Determinism.** The VM uses integers and fixed point only. Where the
API must look at floats (a position against a zone), the test runs in
Rust under the float rules of
[sim-rewrite.md](../engine/sim-rewrite.md#float-rules-for-every-rust-sim-kernel-enforced-in-sim-core)
and the script sees a `bool` or a rounded `fixed`. Random draws come
from a SplitMix64 generator in `RulesExt`, seeded from the match seed
and the blob hash; the game's own generator (`[0x4e7408]`) is never
drawn from, so the script adds no draw to retail's call order. No
`HashMap` order, clock or thread count reaches a result. Division by
zero gives 0 and a trace warning.

**Versioning.** A build that can host or join script games advertises
`rules/1` in its presence capabilities, beside `variant-ext/1`
(**Code**, `live_proto::capability::RULES` in
`crates/live/live-proto/src/lib.rs`; a build advertises it while the
`rules` feature is on, `live_glue::capabilities`).
An older build cannot host a script game. As a guest it is **warned
about, not refused**. No guest on any build runs the script, so an older
guest predicts exactly as a newer one does, from the same replicated
state. What it lacks is display: the script's name, option labels and
messages, the objective and HUD observers, and the blob fetch that
brings them; it shows the carrier engine's labels. The warning is the
mechanism that exists: the host's log names the guests lacking a
capability (`variant_ext::session_guests`, `Hle::players_lacking`,
game-options.md "How it reaches every player"). A refusal needs a join
gate in the session and matchmaking path, and none is designed. The IR
version changes when ops change; the API version when an entry's meaning
changes. A blob names both and a VM refuses what it does not know.

**Films.** The blob, the options and the seed are replay inputs, and the
`rules` feature's hooks are part of `simulation_hooks`
([film-format.md](film-format.md#determinism-and-verification)). The VM
re-runs in playback. Script-made objective changes need their own
observer for film events, since the existing one reads retail engine
state (film-format.md "Objective and power-up events").

## The engine API

The API is a numbered list of entries; a script can call nothing else.
An action's **Cap** column is the capability its script must declare
("Roles, powers and capabilities"); `core` needs none, and reads need
none. Status: **named** is a point or layout in `crates/hooking/guest-layout`
or `crates/rewrites/engine`; **traced** is an address in a doc with no
named point yet; **untraced** has no known entry.

Reads:

| Read | From | Status |
|---|---|---|
| player and team score, kills, deaths | statborg, `E+0x304` (`game::STATBORG`, `PLAYER_STATS`, `TEAM_STATS`) | named |
| players: present, alive, team, unit, lives | player datum (`guest-layout` `player.rs`; team `+0xc0`, lives `+0x1ac`) | named |
| team play, score and time limits, type area | variant record `V` (`variants::record`, [live-gamestate.md](../live/live-gamestate.md)) | named |
| `round_time_left()`: the round start tick against the time limit | `E+0x70` (`ROUND_START_TICK`, likely) and `V` | named |
| objects: tag, position, parent | object datum (`guest-layout` `object.rs`) | named |
| objects by label; `zone.label`, `zone.id`, `o.number` | the host's labelled things (`rules/census.rs`): netgame flags in guest memory and the session's Forge document | named |
| `is_objective(o)`; objective records (object, team, carrier); `o.carrier` (`carrier_of`); `for each objectives` | `E+0xc18`, `E+0xc1c` (`engine::objectives`) | named |
| `o.team` (`object_team`): an objective's team, or a flag placement's team through its designator | record `+4` (CTF, Assault); `E+0x10` (`engine::objectives::DESIGNATOR_TEAMS`, bots.md section 3.7) | named |
| `player_of(o)`: the player whose unit an object is | the roster's units (`firefight::runtime::read_players`, the player datum's unit) | named |
| `inside(p, zone)`: a living unit inside a labelled flag's zone: a hill's polygon or a flag's cylinder | zone tests, as below | named |
| zone tests: who is in a labelled zone, behind `inside`, `on enter`, `on leave` and `ticks_held` | the host's zones over the flags' positions (`rules/world/zones.rs`, "The host runtime", "Zones"); the sizes are ours, since no flag's capture volume is traced; Forge trigger volumes are not in the host's census | named |
| netgame flags by type and team | `scnr+0x118` (`engine::objectives`); query `0x19ec40` | named; traced |
| per-engine globals (`E+0xfc`): hill, bomb state, territories, Juggernaut mask | `engine::objectives`; `KOTH_GLOBALS` and its siblings in `global.rs` | named |
| events of the tick | `PLAYER_KILLED` `0x15ce70` for `on kill`; the roster's units each tick for `on spawn` and `on death` (`runtime::tell`); `SimRead::events` once sim-rewrite section 7 exists | named |
| squads: actors alive, `on squad_dead`, `on actor_kill`, `on actor_assist` | `actor_net::squads::count_by_squad` and the kill list Firefight's `Observed` carries (`crates/extensions/game/src/actor_net/squads.rs`, `crates/formats/firefight/src/director.rs`) | named |
| `zone_path(n, kind)`, `ship_ready(kind)`, `squad_ship(class)`: whether kit zone `n` has a path for a kind of drop ship (0 Phantom, 1 Pelican) and whether the kit's ship of the kind is in the world (`ShipSeen`), and the kind a class arrives by; `ff_drop_squad`, `ff_ship_hover`, `ff_ship_unload`, `ff_ship_leave`: the drop ship's commands | `firefight::director::ShipSeen` and `Command::DropSquad`, `ShipHover`, `ShipUnload`, `ShipLeave` | named |
| `zone_seen(z)`, `zone_distance(z)`, `zone_has_role(z, mask)` (whether the zone carries a role of the mask; every zone for mask 0), `zone_team_distance(z, team)`: Firefight's drop zones, the objects labelled `ff_drop` in kit order, and a zone's distance from one player team | the zone visibility and distance Firefight's `Observed` carries (`ZoneSeen`) | named |
| `on bounce(p, spot, chain, monster)`, `on bounced(p)`; `on touchdown(p, down, up)`, `on launch(p, spot, speed)`, `on grounded(p)` | the superbounce mode's medal chain (`superbounce_chain.rs`): its awards, and what each step saw of the player's physics (`Physics`); the touchdown rule and the landing judge of [superbounce.md](superbounce.md#medals) | named; delivered while the superbounce mode runs |
| bounce and crouch spots, as zones | the scanner's spots per map and BSP (`crates/live/superbounce`); candidates, none traced as a real bounce | named |

Actions:

| Action | Through | Status | Cap |
|---|---|---|---|
| `score(p, n)` | `SCORE_ADD` `0x15b7c0` | named | core |
| `end_round(winner)` | `GAME_END` `0x1389c0` ends the game; naming a winner is not traced (winner storage is not recovered, live-gamestate.md) | named; untraced | core |
| `next_round(winner)`: the round ends and the game goes on | `ROUND_END` `0x15b980` with 0 asks the round state machine for the round's end (`E+0xc04` 2) and posts the round-over event; the machine runs the round's end (five seconds) and the gap (one second), then the round reset `ENGINE_ROUND_RESET`, where the runtime ends the VM's round (`Input::RoundEnd`: the `ticks_held` counts, spawn rules, bodies, shown areas, HUD and offers go; no `on round_end` runs on a host) and runs `on round_start` (`Runtime::round_start`). The engine is not told the winner: the script scores it. `rules::guest::next_round` refuses the call by name with no game engine, on a distributed client, after the game's end, and while the round is not in play or its end is already asked for (`E+0x6c` or `E+0xc04` not 1). The engine ends the game itself after 32 rounds (`E+0x6e` 31). The round resolution `0x15b3a0`, which weighs the variant's rounds setting, is not called ([notes/2026-11-27-next-round.md](../engine/notes/2026-11-27-next-round.md)). The fake game ends the round and starts the next at the next tick boundary, with every player alive, the variant's lives, the round's clock from its start and the scores kept (`FakeGame::next_round`, `scenario::step`); a second call in a round is refused there too | named | `game.rounds` |
| lives | player `+0x1ac` and `PLAYER_ENTITY_DIRTY` `0xa7840`; gates `PLAYER_OUT_OF_LIVES` `0x15db30`, `PLAYER_RESPAWN_READY` `0x15dd40` | named | core |
| `respawn(p)` | `PLAYER_SPAWN` `0x14f2e0` | named | core |
| `set_gravity(g)`: the match's gravity, `g` a `fixed` | `variant_ext` hooks (`crates/extensions/game/src/variant_ext.rs`); the host runtime does not make the call yet | named | `game.traits` |
| traits for the match: damage, shield, health | `variant_ext` hooks, as gravity | named | `game.traits` |
| traits per player; movement speed | traits answer `ENGINE_TRAIT_QUERY` `0x24dda0` (vtable `+0x8c`; every engine's own `+0x8c` falls back to it); the match's speed is `variant_ext::Speed` on the speed step `0x15c000` (player `+0x19c`), per player none yet (game-options.md "Movement speed") | named point | `player.traits` |
| `set_team(p, t)` | no entry known; the field is player `+0xc0` | untraced | `player.team` |
| `set_loadout(p, primary, secondary)`: the multiplayer list's weapons, by tag path | `STARTING_WEAPON_PICK` `0x19eda0` for the tag, then the starting profile's give (`0xb7930`, `0xb7b40`, `unit_add_weapon` `0xcd0c0`; `rules/world/loadout.rs`) | named | `player.weapons` |
| `create(tag, at)`, `delete(o)` | `OBJECT_DELETE` `0xb8540`; creation as Forge's ([forge.md](forge.md)) | named; traced | `objects.create`, `objects.delete` |
| `register_objective(o, team)`, unregister, picked up, dropped | `0x15e050` (object, team; no check for a duplicate), `0x15e130`, `0x15e250`, `0x15e460`; every object a host script can name is already registered, since Forge labels are not built | traced | core |
| `spawn_objective(tag, at, team)`: create an objective at a placement and register it | as Oddball's `0x2bc1f0` does: placement `0xb7930`, `0xb7b40`, `0x15e050`, `0xa7870`. Not built: the host resolves no tag path to a tag datum (the loaded map's names are only in its file), and has no `at` but a registered objective | traced | core |
| `reset_objective(o)`: return an objective to where it spawned | Oddball: `ODDBALL_BALL_RETURN` `0x2bc340`, the ball's spawn flag through `OBJECTIVE_RETURN` `0x15e360`. CTF's flag return reaches `0x15e360` at `0x242b41` from a routine whose start is not read. `ctf.rules` and `oddball.rules` do not call it: a script returns an object with `delete` and `spawn_objective` | named on Oddball | core |
| `role_traits(p, on)`: give or take the script role | the VM's player mask and the host's answers at `ENGINE_TRAIT_QUERY` and the retail overrides | named | core |
| barriers and other map changes | `set_open(o, open)` on a labelled barrier object (below); creating and deleting at labels, as above, is not hostable (the host resolves no tag path to a tag datum) | `set_open` named; `create` and `delete` not built | `objects.move`; `objects.create`, `objects.delete` |
| `set_open(o, open)`: a barrier opened or closed | the VM keeps each open barrier's closed place (`RulesExt::gates`, at most 16) and steps it `GATE_DROP` (64 world units) below it, or back, through the same `OBJECT_SET_TRANSFORM` put a `teleport` makes (`rules/world/objects.rs`); the round's end closes every open one. No call of its own; a put the world refuses (a place outside it) closes the barrier in the state again, so the next `set_open(o, true)` tries again. Whether a barrier parked below the floor stops blocking, and whether a client sees it move, are R2 and R3 | named; whether collision follows the put is not researched | `objects.move` |
| `place_squad(s, class, n, label)` from the palette with its team, `hunt(s)`, erase | `AI_SQUAD_PLACE` `0x201520`, `AI_SET_TEAM` `0x275b60`, `AI_SET_ORDERS` `0x275cb0`, `AI_ERASE` `0x2736c0`, through `actor_net::squads::SquadEntry` (firefight.md "Guest boundary") | named; game runs R1, R2 pending | `ai.squads` |
| `fly(v, path, t)`, `hover(v, on)`, `seat(p, v)`, `eject(p)`, `unload(v)`, `place_squad_in(s, class, n, v)`; reads `vehicle_riders(v)`, `vehicle_driver(v)`: a vehicle flown along a labelled `path`, loaded, unloaded and read | [rules-world-api.md](rules-world-api.md#vehicles-and-drop-ships): the host calls the retail hover and unload bodies at the tick boundary | named (`hover`, `unload`), built in Firefight's host (`fly`, `place_squad_in`), traced (the rest) | `vehicles.drive` |
| `spawn_areas(team, label, n)`: the team spawns only at the starting locations labelled `label` and numbered `n` | the rule in the VM's state; `SPAWN_LOCATION_FREE` `0x23bb70`, the free-spot test of `PLAYER_SPAWN_LOCATION` `0x14ef00`, answers "taken" for every other location (`rules/world/spawns.rs`); how the chooser weighs a location is not traced and not needed | named | `player.spawn` |
| `spawn_near(team, zone, n)`: the team spawns only at the `n` starting locations nearest the zone; `none` or 0 lifts it | as `spawn_areas`, over the starting locations' positions (`+0x00`) | named | `player.spawn` |
| loadout chosen at spawn | `on spawn` and `set_loadout`, with `offer_pick`, `pick_row` and `picked` for the player's own choice ([rules-world-api.md](rules-world-api.md#pickers)) | named; the offer reaches a client as a cue and its pick returns on the tunnel's `ActorPick` frame, so a player on a dedicated host's client picks, and a player on a machine with no tunnel gets the lobby's choice | `player.weapons`, `player.pick` |
| `give(p, tag)`: a weapon into a living player's hands | no entry proven. The pickup's attach routine `0x10ca80` (item, new unit) is on record as an observer only (**E**, [bots.md](bots.md) "Weapon taken") | untraced | `player.weapons` |
| `take(p, weapon)` | the inventory removal `0xce920`, reached from the drop path `0xe8de0` (**E**, bots.md, the dual-wield drop); a call outside a drop is not proven | traced | `player.weapons` |
| `set_body(team, tag)`: the team's players spawn as the Spartan or the Elite; `none` lifts it | the rule in the VM's state; the model byte (player `+0x88`) that `PLAYER_SPAWN` `0x14f2e0` and its representation pick `0x14f040` read, written for the spawn and put back after it (`rules/world/body.rs`) | named | `player.disguise` |
| `show_area(zone, team)`, `hide_area(zone)`: a zone's boundary drawn in play in the team's colour (`none`: neutral), at most eight at once | the shown zones in the VM's state (`RulesExt::shown`); the runtime hands their shapes to the world-line source after each tick (`rules/world/walls.rs`, [gametype-markers.md](../game/gametype-markers.md) "When a marker shows"). No engine call | named | none |
| `hide(p)`, `show(p)`: a player's biped not drawn, not hit | no entry known. The native renderer can skip the draw; hitboxes and collision are the engine's | untraced | `player.disguise` |
| `attach(o, to)`: an object follows another | no entry known; the object datum has a parent field (`guest-layout` `object.rs`) | untraced | `objects.move` |
| `camera(p, mode)`: first or third person | no entry known | untraced | `player.camera` |
| `teleport(o, at)`, `move_to(o, at, t)`, `stop(o)`; `o.position`, `o.velocity`, `o.speed`; `on arrive(o)` | `OBJECT_SET_TRANSFORM` `0xb7430` for a put and a mover's step each tick; the movers in the VM's state (`RulesExt::movers`); the object datum's position and velocity for the reads (`rules/world/objects.rs`, [rules-world-api.md](rules-world-api.md#objects)) | named; whether a client sees the move is not researched (R2, R3) | `objects.move` |
| device power | Forge's sinks (forge-features.md) | not built | `objects.move` |
| `p.airborne`, `p.crouching`, `p.primed`; `sees(a, b)`; `on touchdown(p, down, up)`, `on launch(p, spot, speed)`, `on grounded(p)` | a biped's flags (`game::superbounce_charge`), `AI_LINE_OF_SIGHT` awaited at the next tick boundary for the VM's sight table (`RulesExt::sight`, `rules::runtime::run_work`), and the medal chain's physics for every player (`superbounce_chain::Physics`, `game::rules::world::physics`); [rules-world-api.md](rules-world-api-reference.md#physics) | `sees` is hostable with a one-tick delay; the host delivers the three events and answers the biped reads while the superbounce mode runs; scripts using the reads must declare `requires hud.superbounce` | none |
| `message(to, text)`: `to` is a player, a team or `all` (everyone); the compiler picks one of three entries by its type (`message`, `message_team`, `message_all`), all spelled `message`. A text written in the call may hold up to four `{name}` holes, each a player, team, int, time or enum in scope (`{{` is a brace); the call reaches the game with the holes' values after its own arguments ([rules-language-types.md](rules-language-types.md#messages)). The host shows it to its local users in the audience through the notification insert (`hud_kit::NOTIFY`) and sends it to clients as a show cue ([rules-world-api.md](rules-world-api.md#the-show-channel)). Scoreboard text, announcer, waypoint | the notification queue (`0x24cbee`), per local user (hud-kit.md "Messages"); the centred text (`0x24cc73`) is not used | named | `hud.message` |
| `hud_text(w, "…")`, `hud_value(w, n)`, `hud_fill(w, f)`, `hud_style(w, style)`, `hud_show(w, on)`, `hud_value_for(w, p, n)`; `mark(wp, at)`, `mark(wp, at, to)`, `unmark(wp)` | the VM's show state (`RulesExt::show`), drawn from the objective HUDs after each tick (`rules/world/hud.rs`); a mark's audience is a player's slot's bit or the team's seated players' bits, resolved in the VM from the roster (`Game::players_of_team`); the state rides the show channel's `Show` record ([rules-world-api.md](rules-world-api.md#the-show-channel)) | named | `hud.widgets`, `hud.waypoints` |
| `place_squad_at(s, class, n, zone, tier)`: a squad at a kit zone index with a weapon tier | Firefight's `PlaceSquad` command, through the same squad entry | named | `ai.squads` |
| `ff_set`, `ff_round`, `ff_wave`, `ff_skull`, `ff_lives(p, n)` (`none` for the shared pool), `ff_score`, `ff_respawn`, `ff_finish`; with two player teams `ff_target(s, team)`, `ff_ally(s, team)` (after an allied squad's `place_squad_at`), `ff_team_lives(team, n)`, `ff_player_score(p, n)`, `ff_team_finish(winner)` (`none` a draw); `ff_bonus(on, ticks)` for the bonus round | Firefight's host log entries (`firefight::Command`), queued with the squad commands so the log keeps program order | named | `firefight.director` |
| `sound_to(s, p)`, `sound_to_team(s, t)`, `sound_to_all(s)`: a `sound` of the map played in the ears of an audience | `SOUND_START_AT_MARKER` `0x1891d0` at each audience player's unit, and a sound cue to every machine (`rules/world/audio.rs`; [rules-world-api.md](rules-world-api.md#audio)). `say`, `sound_at`, `music`, `music_stop` and `announce` are listed and refused by name (R4, R5) | named | `audio.play` |
| `bot_go(p, at)`, `bot_hold(p, zone)`, `bot_follow(p, q)`, `bot_attack(p, q)`, `bot_free(p)`: an outside order to a phantom bot, its brain's goal while it stands | the phantom registry (`bots::phantom::order`): a hint to the seat's brain on the host, with a zone resolved through the census and the scenario's netgame flags (`rules/world/bots.rs`). No guest entry: the pad replicates as any player's input. An order to a seat no phantom holds is a refused call | named | `bots.orders` |
| `bot_goal_attack(t, zone)`, `bot_goal_defend(t, zone)`, `bot_goal_hold(t, zone)`, `bot_goal_clear(t)`: a place that matters to team `t`'s bots, which they play as their objective in place of the retail engine's, or the team's places cleared | the host's goal table (`bots::objective::script`), set at the tick boundary (`rules/world/bots.rs`, `goal` and `set_goal`) with the zone's own shape, read into every phantom's perception and planned as a retail objective is (`objective::script::plan`). At most 8 places a team; a ninth is refused by name (`bot goal refused: team N has 8 goals already`). A game's start and end clear the table. A film makes none | named | `bots.orders` |
| `forge_move(o, at)`, `forge_delete(o)`, `forge_place(tag, at)`, `forge_scale(o, s)`: an edit through the Forge log | a `forge-doc` op, validated and host-stamped (`crates/formats/forge-doc`, `LogHead::stamp`); edit classes in "Roles, powers and capabilities" | named | `forge.move`, `forge.delete`, `forge.place`, `forge.scale` |

VM built-ins reach no engine entry (status **VM-only**):

| Built-in | What it is |
|---|---|
| `count(set)`, `first(set)`, `len(table)`, `min(a, b)`, `max(a, b)`, `clamp(v, lo, hi)` | over a finite set or a constant table; `first` follows the `for each` order and gives `none` for an empty set; `min`, `max` and `clamp` take all `int`s or all `time`s |
| `vec_length(x, y, z)` (`length(v)`, `distance(a, b)`, `o.speed`) | the length of three 16.16 components as 16.16, the square root of the sum of the raw squares taken whole in 128 bits (`ty::fixed::length`) |
| `pick(set, n)` | `n` members drawn with the VM's generator |
| `seconds(n)`, `whole_seconds(t)`, `ticks(t)` | `n` times the tick rate, as `time`; `t` divided by the tick rate (rounded toward zero), and `t` itself, as `int` |
| `ticks_held(zone, team)` | a counter the VM keeps in `RulesExt` per zone and team: each tick from the first one a script names the zone, the team the game reports holding it alone (`Game::zone_holder`) gains a tick. Counts clear at the round's end; at most 32 zones are counted, and a 33rd is a script error. `none` or a team out of range reads 0. The host answers `zone_holder` with its zone test ("The host runtime", "Zones"). The host delivers no `round_end`, so its counts run for the whole game; a script that ends the round with `end_round` ends the game |
| `squad(n)`, `team n`, `text n` | constants: a kit squad slot, a team index, a string-table index |
| `fmt_player`, `fmt_team`, `fmt_int`, `fmt_time`, `fmt_enum` | a message's hole values, which the compiler emits before a `message` call and a script cannot name; the VM queues at most four for that message, and the verifier refuses a run not shown at once by a message whose holes it matches ([rules-language-types.md](rules-language-types.md#messages)) |
| `now()` | the tick the host began (`Vm::begin_tick`), as `time` |
| `random(n)` | the VM generator's next draw modulo `n`; 0, with no draw, when `n` is at most 0 |
| `grant(p, cap, budget n, palette "x")`, `revoke(p, cap)`, `granted(p, cap)` | a player's powers, records in `RulesExt` ("Roles, powers and capabilities") |

The engine update `ENGINE_UPDATE` (`0x15c150`) and the post-game step
`0x15ca10` are traced (live-gamestate.md) and are where the lifted
engines do their per-tick work; they are not API entries. The engines'
vtables and their rule-making slots are in "Which engine index a script
rides".

## Roles, powers and capabilities

**Design.** A capability is a named group of API actions. The list is
fixed per API version and compiled into every build with one line of
plain words each:

| Capability | Shown in the lobby as | Grantable |
|---|---|---|
| `game.traits`, `player.traits` | changes gravity, damage, shields and speed | no |
| `player.team`, `player.spawn` | moves players between teams; chooses where they spawn | no |
| `player.weapons` | gives and takes weapons | no |
| `player.disguise`, `player.camera` | hides or changes a player's body; changes the camera | no |
| `player.pick` | asks each player to pick from a short list before the round | no |
| `objects.create`, `objects.delete`, `objects.move` | makes, removes and moves objects | no |
| `ai.squads` | places and commands AI | no |
| `vehicles.drive` | flies and loads vehicles | no |
| `bots.orders` | gives bots orders | no |
| `hud.message` | shows messages | no |
| `hud.widgets`, `hud.waypoints` | draws on the HUD; marks places and things | no |
| `hud.objective` | shows objectives, marks and a clock | no |
| `hud.superbounce` | shows the superbounce guide: a priming HUD, marks over the map and bounce medals | no |
| `game.rounds` | plays the game in rounds | no |
| `firefight.director` | runs Firefight rounds, waves, lives and scores | no |
| `forge.move`, `forge.delete`, `forge.place`, `forge.scale` | lets players move, remove, place and resize objects during the match | yes |

**Declared, then checked everywhere.** The header's `requires` list is
the most a script can ever do. The verifier refuses a call, or a
`grant`, whose capability is not declared. The names are in the IR
header, so every party reads them without running anything:

- **The lobby and the variant screen** show the plain words, from the
  build's table. A script supplies no text about its own powers.
- **A host build** that does not know a declared name, or lacks an
  entry behind it, refuses the script and names the capability. Nothing
  runs with a call stubbed out.
- **A hopper** takes a deny list of capability names; a script that
  declares a denied one is not offered in it. `forge.*` denies the
  family. The hopper file keeps no list, so every unranked hopper's is
  empty.
- **Ranked** denies `forge.*` always: Live's allow-list
  ("Security and usability") never lists a script that declares one, so
  no host, modified or not, has a ranked game with edits counted.

**One policy** answers all of them (`rules::policy`,
`crates/formats/rules/src/policy.rs`): a kind of game (`Context`:
custom, hopper with its deny list, ranked, Theater) and a header's
`requires` give the refused capabilities, each named with its reason
(`policy::check`). Custom and Theater refuse none, a hopper refuses its
list, ranked refuses every grantable capability, and a name the build
does not know is refused everywhere. Four places ask it:

| Where | Kind of game | On a refusal |
|---|---|---|
| Live's hopper edit (`PUT /v1/titles/{title}/hopper`, `admin::check_scripts`): each entry of a playlist the patch names whose variant has mode 2 | ranked for hopper types 2, 4 and 5, else a hopper | the patch is refused, naming the playlist, the entry and the capability; a prefix no saved or built-in script has is refused too |
| Live's session (`POST /v1/sessions`, and an update that sets `rules`; `Live::check_session_rules`) | ranked when the session's flags are `ranked` | refused, naming the capability; a ranked session whose `rules` names a script Live does not have is refused |
| Live's publish routes (`game_variants::eligible`) | custom, hopper and ranked | none: publishing stays open, and the variant records `eligible`, the kinds it may play in |
| The host at the game's start (`Runtime::start`, before `host_refusal`) | a hopper when the game is matchmade (`Hle::matchmade`), else custom (`rules/feature.rs`) | the game plays the carrier engine's own rules, and `rules.state` names the capability and the kind of game |

The host does not know whether a matchmade game is ranked; Live holds a
ranked session's script to the ranked policy when the session names it.
A granted power is used in custom and hopper games, never in ranked
ones or in Theater (`policy::edits_allowed`; `Vm::check_proposal` takes
the context).

**Roles and grants.** A role is the script's own state: an `enum` in a
player variable (`p.role = Role.vip`). A power is a grant the script
makes when the role changes:

```text
requires forge.move, forge.delete, forge.place
palette "vip" = ["<block-out tag path>", "<barrier tag path>"]
palette "wrecker" = ["<vehicle tag path>"]

fn crown(p: player) {
    p.role = Role.vip
    grant(p, forge.place, budget 20, palette "vip")
    grant(p, forge.move, budget 40, palette "vip")
}
fn uncrown(p: player) { p.role = Role.none; revoke(p, forge.place); revoke(p, forge.move) }
on spawn(p) { if p.team == team 1 { grant(p, forge.delete, budget 3, palette "wrecker") } }
```

A grant is a record in `RulesExt`: player, capability, uses left,
palette. So checkpoints, snapshots and films carry it, and the sim hash
covers it. A grant never exceeds the declaration, holds at most 255
uses, and ends on `revoke`, at the player's quit and at the round's end.
A palette is a constant list of tag paths, checked against the map at
load. For `forge.place` it is what the player can make. For the other
three it is which tags' objects the player can touch; with no palette
they are only objects that this match's edits made, so no grant reaches
the map's own objects by default. No grant reaches a player's biped, a
held object or a registered objective.

**A granted power in use.** The player's tool sends a Forge proposal
(`{author, base_seq, op}`, [forge-design.md](forge-design.md#the-edit-log-and-multiplayer)).
The host checks it as `forge-doc` checks author and conditions
(`crates/formats/forge-doc/README.md`, "Document and log"), with the
author's live grants as the permission: the session is not ranked, a
grant for the op's capability has uses left, the tag or target is in its
palette, and the op passes Forge's own validation (bounds, engine
limits, Forge's budget). An accepted proposal is stamped
(`LogHead::stamp`), costs one use at its apply tick, and then runs
`on edit(p, o)`. A refused one changes nothing and is answered with its
reason. A script's own `forge_*` calls take the same path with the
reserved author `rules` and no grant. Proposals share `on press`'s rate
limit.

**What an edit can reach** is forge-design.md's
[edit-class table](forge-design.md#what-the-engine-allows-at-runtime),
with no Rebuild in a match:

| Edit | In a match |
|---|---|
| move, turn, delete an object that exists | Live |
| make an object from a tag the map holds | Live, inside the object pool |
| a placement that outlasts the object (it respawns) | only into the kit's spare entries; refused by name when none is left or the map has no kit |
| a tag the map does not hold; structure collision, portals, pathfinding | refused: Rebuild-only |
| scale | the placement's scale field; whether it reaches collision is **S** (forge-design.md "Not verified") |

"Growing geometry" is therefore placing and scaling the kit's block-out
pieces and barriers (forge-features.md
[Invisible barriers](forge-features.md#invisible-barriers)); until scale
is known to reach collision, a size is a piece from the kit's ladder.
The structure BSP never changes in a match.

**Budgets.** A match holds at most 1,024 scripted and granted edits and
64 objects alive from them, under Forge's own budget (forge-features.md
"Objects"). Both are caps of the kind listed in "Security and
usability".

**The log.** A match with a `forge.*` script keeps a Forge edit log
whose first entry names the script's hash. Every scripted edit and every
use of a granted power is an entry: sequence, tick, author (the player,
or `rules`), op, predecessor hash. The film records each entry at its
tick as an editing session's film does (forge-design.md "Films and
determinism"), and the post-game report counts them per player. The
edits belong to the match; the map's document is not changed. The
opening entry is a new op, so it takes the document format number that
labels take ("Forge labels").
