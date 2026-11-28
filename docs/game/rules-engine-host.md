# Rules engine: the host runtime

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: the VM inside the game, scoring, reload, live variants.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## The host runtime

**Code**, tests only (`crates/extensions/game/src/rules/`): `runtime`
holds one game's script and implements `rules::vm::Game` over guest
memory, `guest` the reads and engine calls behind the API entries (with
`roles` and `objectives`), `scoring` the answers that keep the carrier's
own scoring from running beside the script's, and `feature` the hooks. It follows Firefight's runtime
(`crates/extensions/game/src/firefight/`, [firefight.md](firefight.md#the-host-runtime)).
Boot installs the `rules` feature (`registration::register_core`), on
by default (`H2_FEATURES=-rules` turns it off), and its hooks are on
game-thread points only.

**At the game's start**, an observer after a successful `GAME_ACTIVATE`
(`0x18f000`) reads the active record's block (`G+0x13c+0x10c`) and engine
index (`+0x44`):

| Record | What runs |
|---|---|
| mode 2 | the blob whose SHA-256 starts with the record's 8-byte prefix: a compiled-in script (a port or an example of `rules::builtin`, compiled once at first use, `runtime::compiled_in`), else the game-variant store's (source `local`), else Live's by the full hash a store bundle names for the prefix (below); a System Link game takes a built-in or the store's, never Live's |
| mode 0 | the built-in of the record's engine when the engine is in `runtime::PORTED` (Juggernaut), with the script's default options; on any other engine nothing runs and the engine stays lifted |
| mode 1 | nothing: Firefight's own feature runs it |
| any other | refused: "script mode `n` is unknown to this build" |

**Fetching a blob.** A blob that is neither built in nor in the store
comes from Live, `GET /v1/rules/{hash}/ir`, by the full 64-digit hash:
the record carries only the prefix, so the full hash is the one a store
bundle names for it (a Live variant's bundle names its script by hash).
The start observer fetches it first, with no guest memory held, only on
a Live host or a game alone (`feature::to_fetch`,
`GameScripts::prefetch`), through `Hle::fetch_rules_ir` with a wait of
`game_variants::START_WAIT` (1.5 s, the wait a guest's start gives its
host's options). The blob must hash to the full hash; it is then kept in
the store (`scripts/<hash>.ir`) and the runtime loads it from there, so
its source reads `local`. The runtime never waits on the network: its
fetch only reads the prefetch's outcome (`GameScripts::fetched`). Each
failure refuses the script by name: no bundle names the prefix's full
hash ("its full hash is not known"), the server's silence ("did not
answer within 1500 ms"), its 404 ("has no such script"), a blob of
another hash ("hashes to"), and the load's refusals below. A Live guest
does not fetch: the host runs the script, and the guest's runtime is
dropped at the authority check.

The blob is loaded (`verify::load`) and refused, by name, when its hash
does not start with the record's prefix or it needs an unsupported
operation. The physical record matters for operations that use a retail
object, such as returning a registered Oddball through its ball-return
routine. Role traits are available on every script. Then the start
gate checks the map's labels (`Runtime::gate`, `rules/census.rs`;
[rules-start-gate.md](rules-start-gate.md)), unless the run plays a film:
a map that lacks or breaks a label or tag the script needs blocks the
game (`Status::Blocked`, below), and no rules play, the carrier's
included. Last, a script is refused when it needs what this runtime does
not do yet (`runtime::host_refusal`): an event other than `init`,
`round_start`, `kill`, `quit`, `timer`, `tick`, `bounce`, `bounced`,
`touchdown`, `launch`, `grounded`, `spawn`, `death`, `enter`, `leave`,
`arrive` and `damage`; a read other than the scores,
kills, deaths, `alive`, `team_of`, `lives`, `round_time_left`,
`is_objective`, `carrier_of`, the label reads (`label_number`,
`label_of`, `zone_id`, `object_team`), `inside`, `player_of`, `object_pos` and `object_vel`; an
action other than `score`, `end_round`, `next_round`, `set_lives`, `respawn`,
`role_traits`, `reset_objective`, the bot orders, the messages,
`sound_to`, `sound_to_team`, `sound_to_all`, `set_loadout`, `spawn_areas`, `spawn_near`, `set_body`, `show_area`, `hide_area`, `teleport`, `move_to` and `stop`; a variant field other than `team_play`, `score_limit`,
`time_limit` and `lives`, whose record offsets are not traced; a
`grant`; a script that calls `set_loadout` and names a weapon off the
multiplayer weapon list (`rules::weapons`), by the weapon's path; a script
that plays a sound while the host has no lookup of a sound tag by its
path, by the sound's path.
Then the VM starts with the record's option bits and seed 0 (as
Firefight's, until the session carries one), `on init` runs, then the
first round's `on round_start` (that round's reset ran inside
`GAME_ACTIVATE`, before the start observer), and its state becomes the
`RULES` record. **A refused script is logged
("refused, the game plays the record engine's own rules: ...") and kept
with its reason for `rules.state`, and the game plays as the record's
own engine.** Nothing runs with a call stubbed out.

**A `uses` script** ([rules-variants.md](rules-variants.md#a-script-that-keeps-its-carriers-rules))
runs the built-in script of the type it uses under the variant's own
record. It passes the same prefix and policy checks; then the
runtime takes that built-in (`runtime::used_builtin`, the
compiled-in `rules::builtin::of_type` of `header.uses`) and every
later step is the built-in's: the host checks, the start gate, the VM,
`on init` and the `RULES` record. The VM reads the record's settings as
any script does, and starts with the built-in's default options (the
script has none, so the record's option bits are not read). The
`uses` script's name, hash and source stay with the running game
(`Running::carried`, `runtime::Carried`); everything else reports the
built-in: `rules.state` (`scoring: carrier`, the wire's word for a `uses` script) and `Runtime::hash` (the Live
session's script, which every build has built in). Its `requires` still turn their modes on, since those are read
from the record's script reference
(`rules::header::script_asks_superbounce`). A built-in this host cannot
run yet (`runtime::host_refusal`) is refused before the start gate, so a
map without the built-in's labels does not end the game; the reason
names both scripts ("Sun King uses the built-in
King of the Hill script, which this host cannot run yet: ..."), and the
game plays as a refused script's does. A type with no built-in is
refused as "`<name>` uses `<type>`, which has no built-in script". The
log line is the built-in's with `, for <name>
(<hash>) from <source>, which uses it` after it. Which community
variants run and which are refused is in
[rules-variants.md](rules-variants.md#a-script-that-keeps-its-carriers-rules).

**A blocked start.** `Status::Blocked` keeps the full hash and every
problem's text, which `rules.state` lists as `blocked`. No VM is made and
no record is set; `rules: blocked, the game does not start: ...` is
logged; the first tick boundary asks `GAME_ENGINE_AUTHORITATIVE` and, on
the authority, makes one `end_round` (`firefight::guest::end_game`), so
the game ends in its first tick. A machine that is not the authority goes
idle and ends nothing. In a Forge session (the lobby's mode is Forge,
`lobby_start::Mode::Forge`) the block refuses the script and does not end
the game. The block holds until the next start, and
`feature::blocked()` reads it for the lobby's verdict. An in-place
reload (`Runtime::swap`) is checked against the census the game started
with, and refused with the problems.

**Label reads.** `rules/census.rs` reads the loaded scenario's rows
(each block's count, the netgame flags' type and team at `scnr+0x118`,
the starting locations' team at `scnr+0x100`) and builds the labelled
things with `forge_doc::labelled`: each flag with its type's built-in
label, and what the session's Forge document labels. At a game's start a
session has no document (a Forge session loads its draft at its first
tick), so the host's things are the flags' built-in labels, with one
addition: a script that has a built-in map layout reads it as the
document ("Map layouts" below). The script
holds a labelled thing by a handle of our own, `0x4000_0000` with the
placement id (the block's number times 65,536 plus the final index): a
flag has no object datum. `objects labelled` visits the things with the
label (not starting locations); `o.number` is the label's number (0 when
none); `zone.label` is the script's label of that text; `zone.id` the
placement id; `o.team` a flag's team through its designator
(`E+0x10 + 2d`), or an objective record's `+4` on CTF and Assault, with
neutral as `none`.

**Map layouts.** A built-in layout is a labels-only Forge document
(`Tier::Variant`: label ops on base entries and nothing else), one per
script and map in `crates/formats/rules/src/builtin/layouts/<script>/<map>.json`
(`rules::builtin::layout(script, map)`). `census::things_for` applies the
layout of the running script on the loaded map (`census::layout_script`
finds Siege by its `siege_site` label; the map's short name is the cache
header's), so a site gets its number and a starting location its
`siege_attack` or `siege_defend` label. The lobby's check applies the same
layout (`gate::Sources::census` takes the script), and a Forge map the lobby
picked replaces it, so the check and the match agree. A map with no layout
keeps the flags' built-in labels. A label op on an entry the map lacks is
skipped (`census::layout_doc`), so a layout never blocks a map it does
not fit. Zanzibar's layout names the map's own flags and starts: the
territory flags 17 and 15 are the phase 0 sites and 19 and 13 the phase 1
sites (the final stand is blue's flag stand, flag 3, a `siege_goal`), and the starting
locations are numbered by phase for each side, the attackers' from the
south stand northward and the defenders' from mid-map northward. Readers that
need only the flags' places (`world/walls.rs` on a client, the HUD
publisher, the Forge play forms) read `census::things`, which applies no
layout.

**Zones.** `inside`, `ticks_held`, `on enter` and `on leave` test the
host's zones (`rules/world/zones.rs`), each one a labelled netgame flag:
the host's census has no Forge document at a game's start, so no zone is
a trigger volume. The flags that share a label and a number, three or
more of them (a hill's corners, `hill_corner` 0 to 7), are one zone: the
convex hull of their positions across the floor, from 0.5 world units
under the lowest corner to 1.5 over the highest. Every other flag is an
upright cylinder of radius 1.0 world unit, from 0.5 under it to 1.5 over
it. The sizes are ours and not verified: retail's capture volumes
(Territories' marker `T+0/+0x20/+0x40`) are not traced, and the engines'
own occupancy (King's mask `K+0x1a8`, Territories' site bytes
`T+0x95+8p`) answers only on its own carrier. A body is a living
player's unit (`firefight::runtime::read_players`: the unit's position
while its body is not depleted). `inside(p, zone)` is whether p's body
is in the zone; `ticks_held`'s zone test (`Game::zone_holder`) is the
one team whose bodies alone are in it; `player_of(o)` is the roster
player whose unit `o` is, living or not.

A Live host (`variant_ext::part`) puts the running blob's full hash on
its session at every start (`Hle::set_session_rules`, none when no
script runs, so a stale hash does not linger) and logs each guest whose
presence lacks `rules/1` (`Hle::players_lacking`). `rules/1` is in this
build's presence capabilities only while the feature is on
(`live_glue::capabilities`).

**Events and calls.** Every call a handler makes is kept and made by a
`before` on the next `GAME_TICK`, which answers with an async body
(`runtime::run_work`, the mechanism of Firefight's tick hook): the calls
in the order made, then the VM's queued calls, then the tick, whose chain
lets the hook pass. It sits inside Firefight's tick hook and the hooks
Firefight sits inside.

| Handlers | Run from | Their calls |
|---|---|---|
| `on kill` | an observer before `PLAYER_KILLED` (`0x15ce70`): the attacker and victim player datums at `[esp+4]` and `[esp+8]`, as live telemetry reads them | at the next tick boundary |
| `on round_start`, each round after the first | an observer after `ENGINE_ROUND_RESET` (`0x157ae0`), the engine's round reset, which the round state machine `0x15c550` calls as a round begins | at the next tick boundary |
| `on quit` | an observer before `ENGINE_PLAYER_LEFT` (`0x157de0`): the player's salted datum in ESI, the player already off the roster (player `+2` bit 1) | at the next tick boundary |
| `on touchdown`, `on launch`, `on grounded`, then `on bounce`, `on bounced` | the superbounce mode's tick observer, after `GAME_TICK`: what its medal chain saw of each player's physics that tick, slot by slot (`Runtime::physics`, `world::physics`; [rules-world-api.md](rules-world-api-reference.md#physics)), then each bounce and Bounced the chain files (`rules::feature::bounces`, "Firefight, AI and bounces") | at the next tick boundary |
| `on death`, `on spawn`, `on leave`, `on enter`, then `on timer`, then `on tick` | an observer after `GAME_TICK`: the roster's and the zones' changes since the last tick (`runtime::tell`, `world::zones::events`), then the per-tick group | at the next tick boundary |
| `on init`, then the first `on round_start` | the start observer | at the first tick boundary |

This differs from "Order" in one way, for every handler: a call is made
at the next tick boundary, not inside the event. `PLAYER_KILLED` is
opaque, so a hook there can only observe, and the per-tick group runs
after `GAME_TICK`, not in place of `ENGINE_UPDATE`. So a score lands one
boundary after the kill and a read in the same handler does not see it.
A mode 2 script on a lifted carrier is therefore not a parity game:
running the handlers inside the carrier's callbacks is what makes one.

**The roster's and the zones' events.** After each game tick the runtime
compares the roster and the zones with what it last told the VM
(`world::zones::Seen`, kept outside guest memory) and tells it the
changes in this order: `on death` for each player still listed whose
unit no longer lives, `on spawn` for each whose unit lives again (every
living player at the first tick), `on leave` and then `on enter` for
each unit that left or entered a zone whose label the script declares.
A hill's corners are one zone, which its first corner by handle reports,
so a unit entering a hill is told once. A player that leaves the roster
has no `on death`. A zone event over the VM's 64 a tick is carried and
told again at the next tick. A script with no handler for any of these
events has none of it worked out. The changes are seen at a tick's end,
so a spawn's `set_loadout` lands at the next boundary, a tick after the
unit appears.

The calls: `score(p, n)` is `SCORE_ADD` (`0x15b7c0`) with `n` in EAX,
clamped to ±30,000, and the salted player datum; `set_lives(p, n)`
writes player `+0x1ac` and marks the player entity's lives bit through
`PLAYER_ENTITY_DIRTY` (`0xa7840`); `respawn` and `end_round` are
Firefight's bodies (`firefight::guest::respawn`, `end_game`), and the
winner is not passed, since naming one is not traced. A script error
ends the round the same way. A call the engine refuses is kept, with its
tick and entry name, in `rules.state`'s `errors`.

`set_loadout(p, primary, secondary)` (`rules/world/loadout.rs`) gives
p's living unit the weapons of the multiplayer weapon list by their tag
paths (`rules::weapons`, the starting-weapon values 3 to 19 the variant
editor offers). For each, the game's own `STARTING_WEAPON_PICK`
(`0x19eda0`) answers the value's tag from the map's multiplayer globals,
as the spawn's weapon pass asks it; the weapon is made and added as a
starting profile does (`placement_data_new` `0xb7930`, `object_new`
`0xb7b40`, `unit_add_weapon` `0xcd0c0`), and deleted (`0xb8540`) when
the unit refuses it. The primary goes in mode 2, which empties the
unit's weapon slots first and readies it, the secondary in mode 1 beside
it (mode 2 when there is no primary). The placement is built in 256
bytes of guest memory the feature allocates at install. A unit that is
gone, a value the map's list has no weapon for and a refused weapon are
refused calls. What mode 2 does with the weapons it empties is not
traced, so the listed scripts that give loadouts spawn players unarmed
(`starting_weapon = none`).

`role_traits(p, on)` sets or clears the player's bit in
`RulesExt::role_mask`. The VM clears it on quit. On a round reset it drops the prior round’s
roles before running the new round’s handler. Checkpoints and restores carry the mask with the script
state. The host answers the base and retail overrides of the trait
query (slot `+0x8c`) for a role holder. Other players and unknown trait
IDs continue through the game's query.

On a script record the default role gives extra damage. Scripts can
expose the booleans `role_active_camo`, `role_extra_damage` and
`role_damage_resistance` as ordinary variant options. Their defaults
without an option are false, true and false. A physical Juggernaut
record keeps its edited Juggernaut flags. Its native setter
(`JUGGERNAUT_ROLE_SET`) also announces a newly crowned player and
updates the retail teams and role mask. Distributed viewers on the Live
actor channel receive the membership and trait bits with the selected
script’s prefix. [Actor replication](ai-networking.md).

`spawn_areas(team, label, n)` and `spawn_near(team, zone, n)`
(`rules/world/spawns.rs`) set the team's spawn rule, which the VM keeps
in its state (`rules::vm::Spawns`, so checkpoints, snapshots and films
carry it) until the round's end; neither makes a call at the boundary.
The engine's spawn chooser `PLAYER_SPAWN_LOCATION` (`0x14ef00`) weighs
every starting location of `scnr+0x100` and keeps the heaviest that
passes its free-spot test `SPAWN_LOCATION_FREE` (`0x23bb70`), which
nothing else calls. As the chooser starts, the feature reads the
spawning player's (ECX) team rule; the free-spot test then answers AL 0
for each location outside it, so the chooser picks among the rule's
locations by its own weights and draws. `spawn_areas` keeps the starting
locations labelled `label` and numbered `n`; `spawn_near` keeps the `n`
(at most 255) nearest the zone's middle (a flag's position, a hill's
corners' mean), the lower index first on a tie, and `none` or an `n` of
0 lifts the team's rule. A rule the map has no location for leaves the
engine's own pick. With every location of a rule stood on, the chooser
finds none, and the engine retries the spawn each tick until one is
free. The host's census has no Forge document at a game's start, so a
retail map's starting locations carry no label and the start gate
refuses a script whose `spawns` label needs members; `spawn_near`
needs none. The evidence is in
[notes/2026-11-24-siege-spawns-and-bodies.md](notes/2026-11-24-siege-spawns-and-bodies.md).

`set_body(team, tag)` (`rules/world/body.rs`) sets the body the team's
players spawn in, the Spartan (`masterchief_mp`) or the Elite
(`elite_mp`), which the VM keeps in its state (`RulesExt::bodies`) until
the round's end; `none` lifts it, and any other tag stops the script
with a script error (`rules::bodies`; the checker names a literal one,
`body.unknown`). A multiplayer player's body is the map's player
representation that the active configuration's model byte (player
`+0x88`, 0 the Spartan, 1 the Elite) picks: `PLAYER_SPAWN` (`0x14f2e0`)
asks the representation pick `0x14f040` for the unit type, and the hand
over `0x14cad0` reads the byte again. Before `PLAYER_SPAWN` runs for a
player whose team has a body, the feature writes the body's model byte,
and after it returns it puts the player's own back, so the spawn's every
read sees the team's body and the lobby, the scoreboard and the next
game keep the player's choice. The spawn is the host's; clients get the
unit. The evidence is in the same note.

The objective set (`for each objectives`) and `o.carrier` (`carrier_of`)
read the registry `E+0xc18`/`E+0xc1c` (`rules/objectives.rs`): each
record's object, and the roster slot of the player carrying it.
`reset_objective(o)` is Oddball's own ball return,
`ODDBALL_BALL_RETURN` (`0x2bc340`), with the record's ball number in
EAX: it drops the ball from its carrier and moves it to its spawn flag
through the objective return the engines share (`OBJECTIVE_RETURN`,
`0x15e360`). The ball keeps its datum, so the script's object variables
stay with it. An object that is not registered, a ball number outside
the variant's ball count (`G+0x230`) or a ball with no spawn flag is a
refused call. The evidence is in
[notes/2026-11-22-script-scoring-and-roles.md](../engine/notes/2026-11-22-script-scoring-and-roles.md).

### One scorer

A script game on the script index (`rules::SCRIPT_INDEX`, 6) runs on
the host-owned engine object (`rules/engine_object.rs`;
[game-types-without-carriers.md](game-types-without-carriers.md), "No
retail engine object"). Its kill slot `+0x78` and Slayer's other rule
slots are empty bodies, so no retail engine's scoring or role-making
runs and the script's `score` is the only source of points. The feature
hooks none of the carriers' routines (`SLAYER_KILL`, `JUGGERNAUT_KILL`,
`JUGGERNAUT_TICK`, `SCORE_ADD`); the boot registration test
`the_rules_runtime_hooks_no_retail_scoring_site` holds it to that.

At each multiplayer start every record that names a script, and every
record with no script on a type that has a built-in, starts on the script
index (`rules::record::move_onto_script_index`, a `before` of
`GAME_ACTIVATE` on every machine; `variants::record::onto_script_index`
and `variants::portable::script_record_of_retail`). The record the game is
about to copy changes in memory only: its index becomes 6, its type area is
zeroed (a record with no script has it mapped into the built-in's options
first) and its block names the script; the stored record is not
rewritten. A record that cannot map stays on its retail engine, whose
routines then run and score; the reason is logged once.

`role_traits` gives and takes the Juggernaut role through the engine's
setter on a Juggernaut record. A player who leaves loses the role bit
through the engine's own slot `+0x1c` (`0x1921f0`, from
`ENGINE_PLAYER_LEFT`, which no feature answers). The evidence for the
role and quit paths is in
[notes/2026-11-23-round-start-and-player-quit.md](../engine/notes/2026-11-23-round-start-and-player-quit.md)
and [notes/2026-11-22-script-scoring-and-roles.md](../engine/notes/2026-11-22-script-scoring-and-roles.md).
`PLAYER_KILLED`'s kill feed, medals and statistics run on every index.

**Authority.** The first tick boundary's body also calls
`GAME_ENGINE_AUTHORITATIVE` (`0x15b2f0`). A machine it answers false for
drops the script, its calls and its record ("this machine is not the
game engine's authority"). A film's playback is not told apart: it runs
the script where the replayed engine answers true.

**State.** The `RULES` record (`sim_world::ext::RULES`) holds
`RulesExt::encode`, set at the start, written after every run that
changed it and removed after `GAME_END` and at the next start
([sim-rewrite.md](../engine/sim-rewrite.md#2-who-owns-state)), so
checkpoints, snapshots, film keyframes and the sim hash carry it. Before
each run the record is compared with the bytes last written: a
checkpoint revert or a snapshot load that changed it resumes the VM from
it (`Vm::resume`), and a record that is gone (a full restore) stops the
script with a refusal. A snapshot is refused while calls wait for the
next tick boundary.

The tests (`crates/extensions/game/src/rules/runtime/tests.rs` and
`runtime/tests/hosted.rs`) drive the runtime over invented guest memory
with fake engine callees: the
built-in Slayer starting from a record and setting the record, a kill
making a `SCORE_ADD` call at the next boundary, a suicide's negative
score, a machine that is not the authority, a refusal for an unknown
hash and for System Link, refusals by entry and by event, the
compiled-in Superbounce example found by its prefix, a launch reaching
`on launch` and scoring at the next boundary, the script's chain on the
host's launches and ends of runs with a Monster's double, and a Bounced
reaching `on bounced`, another
engine's record, a record with no mode, the record removed at the game's
end, a reverted record resuming the VM, a stored script running with no
Live client and on System Link, a Live variant's script fetched by its
full hash and run, each fetch failure refused by name, the carrier's
scoring answered only once a running script's authority is known (and
never on a guest or for a refused script), `role_traits` making the
trait-query answer for the current holder and the native role setter's call on Juggernaut, `reset_objective` making Oddball's ball
return with each refusal named, and the objective set and its carriers
read from the registry; the built-in Juggernaut (`round_start_and_quit`)
running for a record with no mode, dealing the role at the start and at
each later round's reset, keeping it on another player's quit and
passing it at the next tick after the Juggernaut's own; and the start
gate (`runtime/tests/gate.rs`): a
map that lacks a label blocks the game with every problem, no VM and one
`end_round` at the first boundary, a map that has it starts, a film
skips the check, a Forge session refuses without ending, a machine that
is not the authority ends nothing, an unreadable census blocks a script
with labels, a swap the map lacks a label for is refused, and the label
reads answer from the labelled flags. Boot's registration tests check that the
feature's four scoring `before`s are in the plan only with the feature
on.

### Reloading a running script

The agent API's `rules.reload`
([agent-api.md](../runtime/agent-api-modes.md#rules-scripts)), and Studio's
script editor through it
([editor-guide.md](../editor/editor-guide.md#game-type-scripts)), put a
new blob into the running game. `crates/extensions/game/src/rules/reload.rs`
decides, purely (`reload::decide`):

1. **Refused** in a Theater film's playback, in a matchmade Live game
   (`Hle::matchmade`, ranked or not), on a guest (System Link or Live:
   only the host runs the script), outside a multiplayer game, and for a
   blob this host cannot run (`runtime::host_refusal`).
2. **In place** when `rules::reload::compatible(old, new)` holds: the
   same carrier, teams, API version and capabilities; the same options
   in the same order with the same kind, range and default; the same
   enums and members; the same variables in each scope with the same
   names, types and order; the same labels and palettes. Handlers,
   initial values, option labels and descriptions may change. Otherwise
   it gives each difference as a sentence ("the script has 1 player
   variables, not 0").
3. **Restart** otherwise, unless the caller asked for in place only, or
   the game cannot restart: a Live game (it restarts from its lobby),
   the replacement needs a different physical record layout, or guest
   calls are unavailable. A restart on index 6 accepts a replacement
   whose generated record also uses index 6.

In place, `Runtime::swap` syncs the `RULES` record, then
`Vm::swap_program` verifies the blob, checks it again, carries the
state over (`rules::reload::carry`: text and tag values remapped to the
new blob's tables, by value) and clears a stopped VM; the record is
written again, and a Live host puts the new hash on its session
(`Hle::set_session_rules`). The active record's script prefix still
names the old blob, so the game's next start runs the old script.

A restart saves the blob in the game-variant store, writes the new
script prefix (and the option bits, kept when the options did not
change) into a copy of the active options block, gives a listed store
variant the same block (`game_variants::inject::retarget`, so
`apply_block` does not undo it at `GAME_ACTIVATE`), and queues the block
through the agent's guest calls (`OPTIONS_QUEUE`), as `map.reload {}`
replays it. The game starts again on the same map, players and variant,
and the start finds the blob in the store. A System Link guest's copy of
the options carries no ext block, so it shows the carrier's own
settings; its runtime is dropped at the authority check either way.

`rules.state`'s `last_reload` keeps the last outcome. The tests:
`crates/formats/rules/src/reload.rs` (each shape change, and a swap
that keeps the kills and runs the new handler),
`crates/extensions/game/src/rules/reload/tests.rs` (refusals, the
decision by mode, the options block a restart writes) and
`runtime/tests.rs` (an in-place swap over the fake game).

## Live game variants in the game

A game variant is a self-contained bundle: a `.h2variant` version 2 file
(`variants::portable`: name, description, author, engine, the 0x130-byte
record with its `hx` block, and the script it names: its name, full hash
and IR). The
game keeps a store of them, `<data>/game-variants`
(`crates/extensions/game/src/game_variants/`, **Code**, tests only):

| Path | What |
|---|---|
| `*.h2variant` | bundles the player put there |
| `live/<id>.h2variant` | the signed-in player's Live variants |
| `live/<id>.pending.json` | a Live variant whose script's IR is not here yet: the server's variant record, with the script's full hash |
| `ported/<key>.h2variant` | saved variants ported at boot |
| `scripts/<hash>.ir` | rules blobs fetched from Live, by full hash |

The store keeps one entry per bundle id (`Bundle::id`, the SHA-256 of
the file's canonical form), and its scripts by full hash with an index by
the record's 8-byte prefix (`store::Store`). Every bundle must hold to the
format when read (`Bundle::integrity`); a file that fails is left out
with a line. A pending Live variant is not a bundle and is not listed in
the game; its full hash is in the prefix index, so a host can still fetch
the script, and once the IR is here (a refresh, or a fetched script found
at the next open) it is written as its bundle and the pending file goes.

- **In the variant list.** At each load of the main menu's tags the
  store's variants join the built-ins after the community variants
  (community-gametypes.md, "The merged block"), named "<name> U+E007".
  Each entry's pairs hold the record settings. When the loader returns,
  `inject::restore_listed` restores the bundle block and record index.
  An index-6 record uses Slayer's loader template and has an empty type
  area. The Change Rules lists resolve its type from the script prefix.
  At activation `inject::apply_block` applies the selection before
  the options reach guests.
- **From Live.** The variant list's load starts a refresh on a thread of
  its own (`game_variants::refresh_live`): `GET /v1/variants?author=<xuid>`
  as the first signed-in account (`Hle::my_game_variants`, 5 s), then each
  script the store lacks by full hash (`Hle::fetch_rules_ir`), each
  variant saved as its bundle with its IR (pending while its script's
  fetch fails). The next load lists them, and
  a later game, offline or on System Link, finds the script on disk.
  Offline or signed out, a line says why and the store keeps what it had.
- **Porting.** At boot, on a thread of its own, every saved variant under
  `UDATA\4D530064` (`variants::save::SaveStore`) not yet ported becomes
  a bundle under `ported/` (`variants::portable::port`), keyed by the
  SHA-256 of its record and
  signature, so an edited save ports again and the rest are skipped. The
  saves are only read. A save with a script reference keeps its named script and packed
  options. Porting resolves the IR from the compiled-in scripts or the
  local store and reports the save name if that lookup fails. Other saves
  take their retail engine's built-in script. One line per run gives the counts ported, skipped and
  refused, with each refusal's name and reason. The retail built-ins and
  the community variants are already in the list, so they are not ported
  at boot; `port::port_builtins` ports them on request.
- **Guests** never need the script: the host runs it. A guest with the
  bundle shows its name and description from the list like any other
  built-in.

The full hash for a prefix comes from the store's bundles, built-ins
first; the host puts the running blob's full hash on its session
(`Hle::set_session_rules`) as for any script.

The tests (`crates/extensions/game/src/game_variants/tests.rs`) cover the
bundle's checks, the store's folders and prefix index, porting a fake
save directory once (and again after an edit) without touching it, the
merged block of retail, community and store entries with their ids and
the 101-built-in limit, the names and descriptions served, the block put
on a started game, and the Live refresh saving each variant with its
script.
