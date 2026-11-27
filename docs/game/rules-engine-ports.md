# Rules engine: ports, Firefight, payoff and Forge labels

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: porting the seven engines and the modes built on top.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## Porting the seven engines

| Engine (index) | The script must reproduce | Needs |
|---|---|---|
| Slayer (2) | kill, suicide and betrayal scoring through the kill callback's path to `SCORE_ADD`; team scores; the limit | nothing beyond the VM |
| Oddball (3) | ball spawns (netgame flag type 4), time-held scoring, carrier traits, ball reset, several balls | objective registry actions; per-player traits; Oddball globals `[0x51ecc4]` |
| King of the Hill (4) | hill from flag types 11 to 18, the hill in play, moving hills, time on the hill, contesting | zone tests on flag polygons; hill state in the replicated globals entity |
| Juggernaut (7) | the role mask, transfer to the killer (`0x192970`), Juggernaut traits, scoring by role | per-player traits; Juggernaut globals `[0x510c9c]` |
| CTF (1) and Assault (9) | flag and bomb spawn and return (flag types 0 to 3), carry, touch return and reset timers, per-team state bits and arming timer, one-bomb sides, scoring within reach of the plate, sudden death | registry actions; team timers; arming volumes. One port serves both, as one engine object does (`0x459d18`) |
| Territories (8) | markers (flag type 10), capture volumes, owner and occupants, contest time, scoring by territories held | zone tests; markers registered by territory index |

All layout facts are **E** from bots.md section 3.7.

**The scripts.** Every engine has a script in
`content/core/rules/` (the `core` package's `rules` items), found by `builtin::builtin(engine
index)`: `slayer.rules`, `oddball.rules`, `koth.rules`,
`juggernaut.rules`, `territories.rules`, and `ctf.rules`, which serves
CTF and Assault (`builtin/mod.rs` puts a header in front of it for each,
`type ctf` or `type assault`, and sets `const assault` to 0 or 1, which
the script reads). Each declares no capability: the
objective entries and `role_traits` are `core`, since they do only what
the retail engine does with the record's own settings. Each file's header
names the entries a host cannot make yet and what the port does not
reproduce; `tests/engines.rs` checks that list against the blob, and
checks each engine's core rule over `fake::FakeGame` (Oddball: time held
scores; Juggernaut: the killer takes the role; King: time on the hill
scores, a contest stops it; CTF: a capture scores and the flag returns,
by reset time and by touch; Assault: a bomb armed on the enemy plate
scores; Territories: a territory held pays each interval). No port has
a parity run. Only Slayer's and Juggernaut's entries are all hostable on
their carriers, and Juggernaut's events are all delivered, so it runs
for a no-mode Juggernaut record (`runtime::PORTED`); the others run in
tests only.

| Script | Waits on | Not reproduced |
|---|---|---|
| Slayer | nothing | |
| Oddball | `spawn_objective` | the Oddball globals; the carrier's speed and toughness; the reset delay and the ball count are script options; a dropped ball returns by `delete` and a new ball at its spawn |
| King of the Hill | the record's `king_moving_hill`, whose offset the host does not read | the KotH globals; how the next hill is chosen (the script takes the next number); the type area's other King settings |
| Juggernaut | nothing | who takes the role after a suicide or a quit (the script draws one); whether the kill that takes the role scores; the record's own Juggernaut settings (the script's options take their defaults); a Juggernaut out of lives keeps the role |
| CTF and Assault | `spawn_objective` | sudden death; the CTF and Assault globals; a team's second plate; detonation after arming; the flag type, reset time, touch return, flag at home and arming time are script options, and a flag or bomb returns by `delete` and a new one at its stand |
| Territories | `register_objective` | the Territories globals; the marker's index as its registration team; a capture's decay; the score interval is a script option |

**Parity method.** The lifted engine is the oracle. A headless match is
recorded once (seed and per-seat inputs), then run twice from the same
start: on the lifted engine, and on the script with the lifted engine's
rule callbacks answered by the script. Because the handlers make their
calls where the lifted callbacks did, the bar is run parity as in
[sim-rewrite.md](../engine/sim-rewrite.md#verification-ladder-every-phase-climbs-all-of-it):
with `impl.sim.arena-hash` on, every `state_hashes_v1` row and arena
block matches and `h2 sim diff` prints nothing, the `RulesExt` record
left out because the lifted run has none. A port therefore writes its
engine's globals (`E+0xfc`) as the lifted engine does; a block it cannot
match is a named exclusion in the port's parity note, with the reason. A
port is proven per built-in variant of its engine, on at least three
maps, with phantom seats filling the roster.

**Variants.** The 39 retail built-ins and the 14 community variants
differ: each retail record keeps its bytes, has no mode and runs its
engine's built-in script once the engine is in `runtime::PORTED` ("A
record with no mode"). The community variants (`rules::builtin::LISTED`) are built
from their scripts by `variants::portable::record_for`, and each record
names its script (mode 2, [rules-variants.md](rules-variants.md)). Rules
that were honour rules ([community-gametypes.md](community-gametypes.md),
Zombies' team switch) become script variants of their own. To travel as
a game variant file, a record is ported with `variants::portable::port`
(`h2 variants port`, [variants.md](../editor/variants.md#portable-files)):
`PortScript::Builtin` attaches its engine's built-in script with the
script's default option values (the built-ins read the retail settings
from the record by name, so nothing else is packed) and sets mode 2 with
the script's prefix; `PortScript::Keep` leaves it with no mode.
`--name` and `--description` replace the variant's name and description
for one input, and each `--option key=value` sets a script option by
name (an int, `true` or `false`, or an enum value's name), packed
against the attached script's options (`portable::port_with_options`,
`Ir::pack_options`, which Live's variant routes pack through too); an
option the script does not declare, a value out of its range and any
option with `keep` are refused. The bundle checks the script against
the physical record's settings layout ("Carrying a script"). The
retail built-ins port from the disc (`--all-builtins`); the community
variants are built by the game at boot (`community_variants.rs` is in
the game crate) and are not ported by the tool.

**Until a port passes.** A no-mode record on an engine not in
`runtime::PORTED` plays the lifted engine, on every kind of session.

**Old films do not replay.** A film replays only with the hook set and
variant bytes it was recorded with (`simulation_hooks`, film-format.md).
A film recorded on a lifted engine is refused with that reason, named in
the theater; it is never played on the script engine, where it would
drift. No frozen build keeps them.

**Retirement criteria**, all of them: every engine's port has passed
parity; script games have been played in every engine on Live and on
System Link. Then the `rules` feature is always on, nothing dispatches
a lifted engine's rule callbacks, and the lifted
engines' hooks, observers and bot readers that parse per-engine globals
(`crates/rewrites/engine/src/objectives.rs`,
`crates/live/bots/src/objective.rs`) move to reading rules state.

## Firefight, AI and bounces

**Firefight runs as a script.** `content/modes/rules/firefight.rules`
owns waves, rounds, lives, skulls and scoring. `firefight::ScriptDirector`
(`crates/formats/firefight/src/script.rs`) supplies the validated
`firefight::Def` as a module of constant tables and tuning, then steps
the compiled script each tick. The script and wave set determine the
blob's hash. The host adapts observations and queued calls to the
director's `Command` stream, which the scenarios exercise
([firefight.md](firefight.md#the-director-as-a-rules-script)).

**AI is commanded by squad.** A script places a squad from the kit's
palette at a label, sets its team and orders, reads how many of its
actors live, and hears `on squad_dead` and `on actor_kill`: the host
actor entries Firefight uses ([ai-networking.md](ai-networking.md)). A
script never runs per actor; actor brains stay the engine's, for the op
budget and for determinism. Phantom bots ([bots.md](bots.md)) are
players like any other to a script.

**Bounces are observed, not made.** Physics stays the engine's: Havok at
an exact 30 Hz tick ([high-framerate.md](high-framerate.md)). A script
hears `on bounce(p, spot, chain, monster)` and `on bounced(p)`, or the
physics events under them (`on touchdown`, `on launch`, `on grounded`,
[rules-world-api.md](rules-world-api-reference.md#physics)), reads the scanner's spots as zones, scores
and times them, and sets traits. That makes Bounce Practice and a trick
or parkour mode real scripts, and the superbounce mode a script's own
choice (`requires hud.superbounce`). The detector's rule is a traced
signature; the launch is the island solve's
([superbounce.md](superbounce.md#the-launch-code-trace)), whose sideways
normal is not decoded, so it can miss or over-report until it is checked in play.

**Code**, tests only. The detector is the superbounce mode's medal chain
(`superbounce_chain`, [superbounce.md](superbounce.md#medals)): each
tick its observer after `GAME_TICK` steps every player's chain. What
each step saw of the player's physics goes to the running script first,
as `on touchdown`, `on launch` and `on grounded` (`Runtime::physics`);
then each bounce it files goes as `on bounce`, and each
Bounced (a fall out of the map during a bounce) as `on bounced`
(`rules::feature::bounces`, `Runtime::bounces`), on that tick, with
their calls made at the next tick boundary as `on kill`'s are. A
`superbounce.medal_test` award reaches neither. `p` is the player's
slot; `chain` is the award's chain count, 1 for a single bounce, 2 for a
double; `monster` is whether the award holds Monster Bounce
([superbounce.md](superbounce.md#the-rule)); `spot` is the strip's id,
`superbounce_chain::spot_id`: FNV-1a over the bits of the strip's edge,
in 31 bits so it is never `none`. The scan is a function of the map's
geometry, so a strip has one id in every game on its map, on every
machine and in a film, and a script keeps per-strip state in object
variables keyed by it (at most `vm::MAX_OBJECT_RECORDS`, 256, at once).
The chain runs only while the superbounce mode is on, so a script hears
bounces only in a game where the mode is on; and the mode is
presentation-only (a film's hook digest leaves it out), so a script that
scores bounces is not yet a parity game.

**The Superbounce game type is a script**,
`content/modes/rules/superbounce.rules`, an example
(`builtin::examples`): `type party`,
`requires hud.superbounce`, no kill rule (a kill scores nothing), and
four options: `suicide_costs` (default on), a point off for a death with
no killer or by the player's own hand; `bounce_points` (0 to 5, default 1), times the
bounce's place in its chain up to 6; `bounced_costs` (default on), a
point off for a Bounced; and `monster_double` (default on), a Monster
Bounce's points doubled after the cap
([superbounce.md](superbounce.md#the-superbounce-game-type) has the
table). It keeps the chain itself, on the host's physics events: `on
launch` adds one to the player's `chain` while `flying` and starts it at
1 otherwise, `on grounded` clears `flying`, and a launch over 16 world
units a second is a Monster; its counts and scores match the Rust
chain's on every tick of the chain's test scenarios
([rules-world-api.md](rules-world-api-reference.md#physics)). The built-in Superbounce variant's
record names it (mode 2, its hash prefix and default options); the
record comes from `record_for` on the script, written into the built-in
entry at list load (`community_variants::write_blocks`), and its other
settings (no limits, overshields, a magnum) are the script's `settings`. The game reads the header
through `rules::header`:

| Header | What it turns on | Game side |
|---|---|---|
| `requires hud.superbounce`, a capability that gates no entry | the superbounce mode: HUD, marks, priming, medals | `header::script_asks_superbounce` over the block's script reference, in `variant_ext::Decision::superbounce`, so a guest that applies its host's block turns the mode on too, and the mode needs no running script |
| `type` | the variant list row and round-start line | `community_variants::filed_type` and `rules::start_line` use the type from the script header |

Both find the script by the record's prefix among the compiled-in
scripts (`runtime::compiled_in`) and in the game-variant store. With the
`rules` feature off the HUD still turns on and Slayer's lifted rules
score the game; with it on, the script runs beside them ("The host
runtime"), so on a retail index its kill scoring adds to the lifted
carrier's ("One scorer").

**Options the host reads.** After its scoring options the Superbounce
script declares the HUD's pieces: `targets` (an enum, `common`, `likely`
or `all`; default `likely`) and the bools `velocity`, `alignment`,
`miss_info`, `medals`, `second_bounce`, `approach_guide` and
`charge_segments`, all on, then the Pogo Stick's `pogo` (a bool, on) and
`pogo_style` (an enum, `unsc`, `covenant` or `forerunner`; default
`unsc`; [pogo-stick.md](pogo-stick.md)). The
script itself never reads them:
they are in the record's option bits for the host, which looks each up
by name (`Ir::option_value(bits, name)`, the value at the option's
offset; `rules::header::superbounce_options`, into
`superbounce_options::Options`) where it decides the mode is on
([superbounce.md](superbounce.md#the-huds-options)). A variant turns a
piece of the HUD off as it sets any option, and a script without them
gets the defaults.

## Payoff

The modes below that have a script are examples in
`content/modes/rules/`, listed by `builtin::examples()`
rather than by engine: `infection.rules`, `gun_game.rules`,
`vip.rules`, `siege.rules`, `race.rules`, `bounce_hunt.rules` and
`superbounce.rules`. Each compiles, verifies and fits a tick
(`tests/engines.rs`), and each header names the entries a host cannot
make yet. VIP, Bounce Hunt, Superbounce, Siege, Gun Game and Race call
none. Bounce Practice waits on the host runtime's `round_start` and
`set_gravity`. Superbounce, Siege, Gun Game, Race and VIP run, and are
listed (`builtin::LISTED`, [rules-variants.md](rules-variants.md#built-in-list-entries)).
The examples are compiled into the game, so a record names one by its
hash prefix with no store or fetch; a record with no mode never runs
one. `tests/modes.rs` plays the listed modes over scenarios on the fake
game (the JSON files in `tests/scenarios/`). Prop Hunt has no script.

- **Race** (listed in Party Games). The map's King of the Hill hills are
  the checkpoints (`label checkpoint: zones numbered at least 3 at most
  64 = "hill_corner"`), so every map with hills races with no kit: a
  hill is the polygon of its corners, and `on enter` with `player_of`
  follows each player round them in number order from the lowest, a lap
  back at the lowest scoring a point, and `laps` of them winning. Each
  corner's next hill is worked out at the round's start (an `object
  var`), so an entry costs no loop. Halo 1's race
  track points are dropped at conversion
  ([multiplayer.md](../halo1/multiplayer.md)); labelled checkpoints
  would give them a use. So do **Halo 1's per-spawn game
  types**: starting locations carry labels and a script filters on them.
- **Infection** with a real team switch, and other modes that were
  honour rules.
- **Gun Game** (listed in Party Games). `on kill` gives the killer the
  next weapon of a constant table on the kill's tick through
  `set_loadout`, whose mode 2 empties the slots first, and `on spawn`
  gives the player's own rung: the weapon progression the variant editor
  cannot express ([community-gametypes.md](community-gametypes.md)).
  Players spawn unarmed and the map has no weapons; a weapon a dead
  player drops can still be picked up, so leaving it is an honour rule.
  Every rung is on the multiplayer weapon list (`rules::weapons`).
- **VIP** (listed in Party Games). Each team has a VIP; a kill scores a
  point and an enemy VIP's `vip_points`, and the role passes on a
  VIP's death or quit to a teammate, living ones first, with a message
  to the team. The script counts each team's points in a `team var`
  and ends the round at `variant.score_limit`, since a score it makes
  is read back a tick late. **With powers**, the VIP would hold a grant to
  place and move block-out pieces and the hunters one to delete
  vehicles; grants wait on the match edit log, so that version is the
  VIP test script in `tests/scripts.rs`.
- **Prop Hunt**, the hardest mode here. A hider's biped is hidden, a
  prop object is attached to the player, damage to the prop is forwarded
  to the player, and the camera is third person. `hide`, `attach`, the
  body swap and `camera` are all untraced, and damage forwarding needs
  an event the API lacks. The native renderer can skip drawing a biped;
  its hitboxes and collision stay until the engine work is done.
- **Siege**, a phased attack-and-defend mode with the mechanics of
  Halo: Reach's Invasion and none of its assets or text ("Phases: attack
  and defend").

The phased mode is the check that the API is wide enough:

| Mechanic | In a script | API status |
|---|---|---|
| up to three phases, a timer each | a global `int` and a `timer` | the VM alone |
| capture zones, then a goal | the host's zone tests and `ticks_held`, over the map's territory and CTF flags | named; the built-in Siege |
| a core carried to a goal | the objective registry as CTF and Assault | traced |
| the map opens per phase | `set_open` on the `siege_barrier` objects, numbered by phase (a barrier is put below its place and back; `delete` and `create` are not hostable) | named; the built-in Siege |
| spawn areas switch per phase | `spawn_areas` over labelled starting locations; `spawn_near` over a zone, which needs no labels | named; the built-in Siege (`spawn_near`) |
| loadouts per team, chosen at spawn | `on spawn` and `set_loadout`; Halo 2 has no armor abilities, so a loadout is weapons, grenades and traits; a player picks a row of their side's table among the rows the phase's tier opens (`offer`, `choice`), row 0 until they pick | named; the built-in Siege; the pick is the select dialog `kit.picker.game` (`content/ui/screens/kit.picker.game.yaml`), opened over the player's controller, and a player on a dedicated host's client picks through the offer cue and the tunnel's `ActorPick` frame |
| team respawn waves | hold respawns with the lives gates, release on a timer | named; the built-in Siege |
| two species teams | `set_body` per team at the round's start | named; the built-in Siege |
| bots that attack and defend | `bot_goal_attack` and `bot_goal_defend` on the front the HUD marks, set again as it moves | named; the built-in Siege |
| the objective and its state on the HUD | the Siege kit's mark, rows and banner (`hud_style`, `hud_value`, `mark`, `hud_text`; "The Siege HUD") | named; the built-in Siege; the phase clock is the kit's `siege.clock` row |

## Forge labels

A prerequisite. `crates/formats/forge-doc` has teleporter and device
links (`links.rs`) and labels (`labels.rs`) to tell a script which
placement is which. Status: the document side is built, and the host
reads labelled things (`rules/census.rs`, "The host runtime"); the
`RulesExt` table a script's declared labels read is not, since the
host's table is rebuilt from the map at each start.

**Design.** A label is a string of at most 32 bytes plus an optional
number (`"checkpoint" 3`, `"hill" 0`). It is a field of a placement in
the Forge document, set through the same edit log as any other field, so
it has no scenario bytes and costs nothing in a session that runs no
script. At map load the labels become a table in `RulesExt`: label,
number, placement id, and the object's datum once it exists. Scripts
read it through the labels they declare ("The language"). Retail maps
with no Forge document get labels from a built-in table per netgame flag
type, so `label flag_spawn: zones = "ctf_flag_spawn"` works on every
map.

**Built.** `Forge::Label { target, label }` sets or clears a label on
a placement named as a link names one (`Endpoint`: a base entry by
index, or the entry an add op made), so it follows the entry through
appends and swap-removes. The text is 1 to 32 bytes of lowercase ASCII,
digits, `_`, `-` and `.`; the number is an `i32`. A placement has one
label and a later set replaces it. Labels are not unique: the number
tells placements with one text apart, and a script asks for all of them.
The validator refuses bad text, an entry that is missing or removed, a
clear of an unlabelled entry and the removal of a labelled entry.
`ForgeDoc::labels` resolves the active labels to final indices and
`replay` hands them to `Apply::labels` after the links.

`forge_doc::builtin_label(flag_type)` is the built-in table: 0
`ctf_flag_spawn`, 1 `ctf_flag_return`, 2 `assault_bomb_spawn`, 3
`assault_bomb_return`, 4 `oddball_spawn`, 7 `teleporter_sender`, 8
`teleporter_receiver`, 10 `territory`, and 11 to 18 `hill_corner` with
the hill number 0 to 7 as its number. Types 0, 1, 4, 7, 8, 10 and 11 to
18 are traced (bots.md 3.7); the names for 2 and 3 are the less certain
(bots.md calls them "bomb and plate"). Types 5, 6 and 9 have no label.
The `RulesExt` table applies a placement's own label first and the
built-in label of its flag type otherwise.

The Forge session sets, replaces and clears a label from the Properties
page (`Session::set_label`; "Selection, groups and prefabs" in
[forge-editing.md](forge-editing.md)), reads them with `Session::label_of`,
and a saved prefab carries its pieces' labels.

**Ground labels.** The labels Forge and Studio place for a scripted game
(`forge_doc::GroundLabel`: `ff_drop`, `ff_base`, `ff_cover`,
`siege_site`, `siege_goal`, `siege_line`; "Maps for scripted games" in
[game-types-without-carriers.md](game-types-without-carriers.md)) are
ordinary labels on netgame flags, so a script declares the ones it reads
as it declares any, and the start gate refuses a map short of a required
one by name. One label has a default: a script's `siege_site` reads the
map's `territory` flags while the map places no `siege_site`
(`labels::fallback`, `labels::reads`; the census check, `Table::labelled`
and `FakeGame::labelled` read through it), so a retail map plays Siege as
before. Firefight's director takes its drops from the kit's `ff_drop_NN`
zones, which `ground::build` makes one for each placed `ff_drop`; the
director's own `ff_drop` label requires nothing, since the generator
supplies the drops a map has none placed for.

A label is a change of the document format. The format is one number
(`forge_doc::FORMAT`, 1; `FORMAT_LABELS`, 2; `FORMAT_ENTRIES`, 3 for
the entry edits and prints of
[forge-map-variants.md](forge-map-variants.md#the-ops); `FORMAT_GROUPS`, 4
for a group op, [forge-editing.md](forge-editing.md#selection-groups-and-prefabs);
`FORMAT_WAVES`, 5 for a document with a wave set,
[forge-firefight.md](forge-firefight.md#the-wave-set)). A document
takes the highest number its ops need: 2 when any op is a label op and
no op needs 3 or 4, and 1 otherwise. `crates/formats/forge-doc/src/validate.rs`
refuses any other number, so a document has one spelling: every
document without labels keeps its bytes and its identity, and a fold
that retracts the last label returns to format 1. This build reads all
three; an older build refuses a newer document whole. A label's target
may carry its entry's print, which makes the document format 3.
