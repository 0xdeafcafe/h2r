# Rules engine: scripted game types

Status: Design, partly built: the `rules` formats crate runs on a fake game in tests and in the web workbench, and the host runtime on invented guest memory in tests only.
Code: `crates/formats/rules` (language, verifier, IR codec, capabilities, VM); `crates/extensions/game/src/rules` (the host runtime). Tests: `cargo test -p rules`, `cargo test -p game rules::`.
Switch: `rules` in `H2_FEATURES`, on by default (`-rules` turns it off) ([switches.md](../runtime/switches.md)); the `rules` cargo feature of `game` and `boot` compiles it in.

One scriptable engine runs every multiplayer game type. A game type is a
script compiled to a small verified IR, plus option values in the variant
record. The host runs the IR in a Rust VM whose state is `SimExt`
extension state, and the VM changes the game only through named engine
entries, so what it does replicates as ordinary game state. The seven
lifted retail engines are the oracles the ports are proven against, and
are retired once every port has passed. The rule that must hold: the same
IR, options, seed and inputs give the same engine calls at the same ticks
on every machine and platform.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Addresses are guest addresses of the retail XBE. No game data is in this
page.

## Decisions

1. **A scriptable game type engine in the style of Megalo** (Halo: Reach
   onward, "Prior art"), with better syntax, security and usability.
2. **Two front ends, one IR.** Our own small typed language and Forge's
   logic graph ([forge-features.md](forge-features.md#teleporters-buttons-and-triggers))
   compile to one rules IR. An importer for the ReachVariantTool dialect
   comes later.
3. **All seven retail engines are ported to scripts**: Slayer, Oddball,
   Juggernaut, King of the Hill, Capture the Flag, Assault, Territories.
   The lifted engines are retired once the ports pass; every retail and
   community variant becomes "engine script + option values".
4. **Firefight, AI-driven modes and superbounce modes are scripts too.**
   Scripts command AI by squad and observe bounces; they never change
   physics.
5. **A phased attack-and-defend mode** (Reach's Invasion) is the showcase
   the API is sized against ("Payoff").
6. **A script declares its capabilities and can grant players powers by
   role.** Script-granted edits run in custom and hopper matches, never
   ranked, inside a budget, each one logged
   ("Roles, powers and capabilities").

## Goals, non-goals, done

Goals: new modes without new Rust ("Payoff"); one engine to maintain and
test headless; scripts from players that are safe on any machine, since
consoles host Live games and film viewers re-run the VM, so safety rests
on the VM ("Security and usability"), never on who owns the host.

Non-goals: campaign scripting (HaloScript stays,
[hs_vm.md](../engine/hs_vm.md)); script execution on a guest or
script-driven rendering; compatibility with Megalo binaries (the importer
reads source text only); a general-purpose language (no I/O, heap, floats
or unbounded loops).

Done means every multiplayer game, on Live, System Link, local or a
dedicated host, runs a rules script, and no game reaches a lifted
engine's rule callbacks.

## How it works

```text
source (.rules text) ─┐
Forge logic graph ────┼─> compiler + verifier ─> rules IR blob (SHA-256 id, capped)
RVT dialect (later) ──┘        (formats crate `rules`: Studio, cli, Live upload)
                                          │
   variant record hx block: mode + hash prefix + option bits
   Live session: hx block (existing) + full hash; blob fetched by hash
                                          │
   host only: verify again ─> VM in the engine's own callbacks,
                              state in SimExt ─> named engine entries
                                          │
                     ordinary game state, replicated as today
```

### Compiler and verifier

The formats crate `rules` holds the parser, type checker, lowering,
verifier, IR codec and VM. It has no I/O and no guest types, so Studio,
the cli (`h2 rules check|build|run`), Live's upload route and the host
runtime link the same code. Compiling is deterministic, so the IR's
SHA-256 names a script everywhere.

The verifier reads IR, not source. It runs at upload, at load on any
machine about to host, and in Studio as you type. It checks the whole
blob (ops, slots, jumps, labels, tag paths, API calls, and that each
call's capability is declared in the header) and refuses a failing blob
whole.

### Carrying a script

Three things travel: which script, its option values, and the blob.

**Which script and its options: the variant record.** The `hx` block
([game-options.md](game-options.md#where-the-extension-lives),
`variants::ext`) has 13 reserved bytes at `+0x11f..+0x12b`:

| Offset | Size | Content |
|---|---|---|
| `+0x11f` | 1 | mode: 0 none, 1 Firefight, 2 rules script |
| `+0x120` | 8 | first 8 bytes of the definition's SHA-256 (rules IR or Firefight's definition) |
| `+0x128` | 4 | 32 bits of packed script option values, in declared order |

The block's CRC covers these bytes; the layout is shared with Firefight
([firefight.md](firefight.md#live)). Carriers that copy the record keep
the reference: saved variants, `.h2variant` files (`variants::portable`),
cloud saves, the `0x130`-byte variant in a hopper slot
(`crates/live/live-proto/src/hopper`) and the web hopper builder
(`web/src/admin/HopperBuilder.tsx`). Editors go through `variants::ext::Ext`
(`crates/formats/variants/src/ext.rs`), whose `script` field carries the
reference. An older build that edits a script variant drops it and plays
the carrier's game. Script options are limited to the 32 bits; retail
settings stay where they are and a script reads them by name.

Detail: see the [detail note](notes/2026-11-26-rules-engine-detail.md),
"Carrying a script".

**A game variant file carries the script whole.** A version-2
`.h2variant` ([variants.md](../editor/variants.md#portable-files)) holds
the IR blob (base64) and, when made from source, the source. Reading one
checks the hashes, `rules::verify::load` and the record's engine index; a
mode 2 record with no script is refused. Live keeps game variants as these
files ([live-api.md](../live/live-api.md#game-variants)).

**To other machines: the Live session.** The block travels as
`NewSession.variant_ext`; the session's `rules` field holds the full
64-digit hash (`NewSession::validate`, `valid_rules_hash`). The host
publishes it with `Hle::set_session_rules`, a guest reads it with
`Hle::host_rules` (`crates/live/live-proto/src/session.rs`,
`crates/live/live-hle/src/sessions.rs`).

**The blob: by hash.** Built-in scripts are compiled into the binary.
Any other blob is stored once on Live under its SHA-256, in both homes,
and fetched by a machine that lacks it. Only the host executes it. A film
stores the blob.

**Limits.** IR at most 256 KiB, source 256 KiB, VM state 64 KiB in
`SimExt`.

### Which engine index a script rides

A new script variant uses index 6 (`rules::SCRIPT_INDEX`,
`variants::GameType::Script`) when its retail settings layout is Slayer
and none of its settings write the type area; other variants keep their
layout's retail index, so a ported CTF variant stays a CTF record. Index 6
has a host-owned engine object, native record clean-up and wire codec, and
the distributed protocol, with common settings and an empty type area
([game-types-without-carriers.md](game-types-without-carriers.md#no-retail-engine-object)).
Four retail paths needed a native answer for it: the engine table
`0x55e4d0[index]`, record clean-up `0x19d650`, wire codec decode `0x7d520`
and the lobby protocol map `0x161c90`.

The engine object's vtable slots that make rules (kill `+0x78`, tick
`+0x48`, per-player `+0x3c`, trait query `+0x8c`) are in
[notes/2026-11-21-engine-vtables.md](../engine/notes/2026-11-21-engine-vtables.md).
The script is the one scorer on index 6; on a retail index the carrier's
routines score beside it, and script calls land at the next tick boundary.

**A record with no mode.** On a host with the feature on, a record with
mode 0 on an engine in `runtime::PORTED` (Juggernaut) runs that engine's
built-in script. Saved variants and hopper slots start as script games
with no stored byte changed; System Link needs no blob for built-ins.

Tables and addresses: see the [detail note](notes/2026-11-26-rules-engine-detail.md),
"Which engine index a script rides".

### The host runtime

`crates/extensions/game/src/rules/`: `runtime` holds one game's script and
implements `rules::vm::Game` over guest memory, `guest` the reads and
engine calls (with `roles` and `objectives`), `scoring` the answers that
stop the carrier scoring beside the script, `feature` the hooks. It
follows Firefight's runtime ([firefight.md](firefight.md#the-host-runtime)).
Boot installs the feature (`registration::register_core`); hooks are on
game-thread points only.

**At the game's start**, an observer after `GAME_ACTIVATE` (`0x18f000`)
reads the active record's block (`G+0x13c+0x10c`) and engine index
(`+0x44`):

| Record | What runs |
|---|---|
| mode 2 | the blob whose SHA-256 starts with the record's prefix: compiled-in (`rules::builtin`, `runtime::compiled_in`), else the game-variant store's, else Live's by full hash; System Link never uses Live's |
| mode 0 | the built-in of the record's engine if in `runtime::PORTED`, with default options; otherwise nothing and the engine stays lifted |
| mode 1 | nothing: Firefight's feature runs it |
| any other | refused: "script mode `n` is unknown to this build" |

**Fetching a blob.** A blob that is not built in or stored comes from
Live, `GET /v1/rules/{hash}/ir`, prefetched before the start
(`feature::to_fetch`, `GameScripts::prefetch`, `Hle::fetch_rules_ir`,
wait `game_variants::START_WAIT`, 1.5 s). It must hash to the full hash
and is kept in the store (`scripts/<hash>.ir`). The runtime never waits
on the network, and a Live guest does not fetch.

**Load, refusals and the start.** The blob is loaded (`verify::load`),
then the start gate checks the map's labels (`Runtime::gate`,
`rules/census.rs`; [rules-start-gate.md](rules-start-gate.md)), unless a
film plays. Last, `runtime::host_refusal` refuses a script that needs what
the host does not do yet (an unsupported event, read, action or variant
field, a `grant`, a weapon off `rules::weapons`, or a sound with no tag
lookup). The VM then starts with the record's option bits and seed 0,
runs `on init` and the first `on round_start`, and its state becomes the
`RULES` record. **A refused script is logged, kept with its reason for
`rules.state`, and the game plays as the record's own engine.**

**A `uses` script** ([rules-variants.md](rules-variants.md#a-script-that-keeps-its-carriers-rules))
runs the built-in of the type it uses under the variant's own record
(`runtime::used_builtin`, `Running::carried`); `rules.state` reports
`scoring: carrier`.

**A blocked start.** `Status::Blocked` keeps every problem's text, which
`rules.state` lists. No VM is made; on the authority the first tick
boundary makes one `end_round`, so the game ends in its first tick. A
Forge session refuses the script without ending the game, and
`feature::blocked()` serves the lobby's verdict. `Runtime::swap` checks a
reload against the start's census.

**Label reads and zones.** `rules/census.rs` builds labelled things from
the scenario's flags and the Forge document (`forge_doc::labelled`); the
script holds one by a handle `0x4000_0000` plus the placement id. Zones
(`rules/world/zones.rs`) are labelled netgame flags: a hill's corners are
a convex hull, any other flag a cylinder of radius 1.0 world unit.

A Live host (`variant_ext::part`) puts the running blob's hash on its
session at every start and logs guests lacking `rules/1`
(`Hle::players_lacking`, `live_glue::capabilities`).

**Events and calls.** Every call a handler makes is kept and made by a
`before` on the next `GAME_TICK` (`runtime::run_work`, the mechanism of
Firefight's tick hook), so a score lands one boundary after the kill.

| Handlers | Run from |
|---|---|
| `on kill` | observer before `PLAYER_KILLED` (`0x15ce70`) |
| `on round_start` (after the first) | observer after `ENGINE_ROUND_RESET` (`0x157ae0`) |
| `on quit` | observer before `ENGINE_PLAYER_LEFT` (`0x157de0`) |
| `on touchdown`, `launch`, `grounded`, `bounce`, `bounced` | the superbounce tick observer after `GAME_TICK` (`Runtime::physics`; [rules-world-api.md](rules-world-api.md#physics)) |
| `on death`, `spawn`, `leave`, `enter`, `timer`, `tick` | an observer after `GAME_TICK` (`runtime::tell`, `world::zones::events`) |
| `on init`, first `on round_start` | the start observer |

A mode 2 script on a lifted carrier is therefore not a parity game:
running handlers inside the carrier's callbacks is what makes one.

The calls, each a refused call when the engine refuses it (kept in
`rules.state`'s `errors`):

| Call | Entry |
|---|---|
| `score(p, n)` | `SCORE_ADD` (`0x15b7c0`), `n` clamped to ±30,000 |
| `set_lives(p, n)` | player `+0x1ac`, `PLAYER_ENTITY_DIRTY` (`0xa7840`) |
| `respawn`, `end_round` | `firefight::guest::respawn`, `end_game` |
| `set_loadout` | `rules/world/loadout.rs`, `STARTING_WEAPON_PICK` (`0x19eda0`) |
| `role_traits(p, on)` | `RulesExt::role_mask`; trait query slot `+0x8c` |
| `spawn_areas`, `spawn_near` | `rules/world/spawns.rs`, `PLAYER_SPAWN_LOCATION` (`0x14ef00`) |
| `set_body` | `rules/world/body.rs`, `PLAYER_SPAWN` (`0x14f2e0`) |
| `reset_objective(o)` | `ODDBALL_BALL_RETURN` (`0x2bc340`), `rules/objectives.rs` |

Evidence: [notes/2026-11-24-siege-spawns-and-bodies.md](notes/2026-11-24-siege-spawns-and-bodies.md),
[notes/2026-11-22-script-scoring-and-roles.md](../engine/notes/2026-11-22-script-scoring-and-roles.md).
Traces: see the [detail note](notes/2026-11-26-rules-engine-detail.md),
"The host runtime".

#### One scorer

A script game on index 6 runs on the host-owned engine object
(`rules/engine_object.rs`), whose rule slots are empty, so the script's
`score` is the only source of points. The feature hooks none of the
carriers' scoring routines; the boot test
`the_rules_runtime_hooks_no_retail_scoring_site` holds it to that. Every
record that names a script, and every record with no script on a type
with a built-in, starts on index 6 (`rules::record::move_onto_script_index`,
a `before` of `GAME_ACTIVATE`); the stored record is not rewritten. A
record that cannot map stays on its retail engine. Kill feed, medals and
statistics run on every index. Evidence:
[notes/2026-11-23-round-start-and-player-quit.md](../engine/notes/2026-11-23-round-start-and-player-quit.md).

**Authority.** The first tick boundary calls `GAME_ENGINE_AUTHORITATIVE`
(`0x15b2f0`); a machine it answers false for drops the script. Film
playback runs the script where the replayed engine answers true.

**State.** The `RULES` record (`sim_ext::RULES`) holds `RulesExt::encode`,
so checkpoints, snapshots, film keyframes and the sim hash carry it
([sim-rewrite.md](../engine/sim-rewrite.md#2-who-owns-state)). A changed
record resumes the VM (`Vm::resume`); a missing one stops the script. A
snapshot is refused while calls wait for the next boundary.

Tests: `crates/extensions/game/src/rules/runtime/tests.rs`,
`runtime/tests/hosted.rs` (the runtime over invented guest memory and fake
callees) and `runtime/tests/gate.rs` (the start gate); boot's registration
tests check the four scoring `before`s are in the plan only with the
feature on.

#### Reloading a running script

The agent API's `rules.reload`
([agent-api.md](../runtime/agent-api.md#rules-scripts)), and Studio's
script editor through it
([editor-guide.md](../editor/editor-guide.md#game-type-scripts)), put a
new blob into the running game. `crates/extensions/game/src/rules/reload.rs`
decides (`reload::decide`):

1. **Refused** in film playback, in a matchmade Live game, on a guest,
   outside a multiplayer game, and for a blob `runtime::host_refusal`
   rejects.
2. **In place** when `rules::reload::compatible(old, new)` holds: same
   carrier, teams, API version, capabilities, options, enums, variables,
   labels and palettes. Handlers, initial values and option text may
   change. Otherwise each difference is given as a sentence.
3. **Restart** otherwise, unless the caller asked for in place only or the
   game cannot restart (a Live game, a different record layout, no guest
   calls).

In place, `Runtime::swap` runs `Vm::swap_program` and carries state over
(`rules::reload::carry`). A restart saves the blob in the game-variant
store, writes the new prefix into the options block
(`game_variants::inject::retarget`, `OPTIONS_QUEUE`) and starts the game
again. `rules.state`'s `last_reload` keeps the last outcome. Tests:
`crates/formats/rules/src/reload.rs`,
`crates/extensions/game/src/rules/reload/tests.rs`, `runtime/tests.rs`.

### Live game variants in the game

A game variant is a self-contained bundle: a `.h2variant` version 2 file
(`variants::portable`: name, description, author, engine, the 0x130-byte
record with its `hx` block, and the script it names: its name, full hash
and IR). The game keeps a store of them, `<data>/game-variants`
(`crates/extensions/game/src/game_variants/`, **Code**, tests only):

| Path | What |
|---|---|
| `*.h2variant` | bundles the player put there |
| `live/<id>.h2variant` | the signed-in player's Live variants |
| `live/<id>.pending.json` | a Live variant whose script's IR is not here yet |
| `ported/<key>.h2variant` | saved variants ported at boot |
| `scripts/<hash>.ir` | rules blobs fetched from Live, by full hash |

`store::Store` keeps one entry per bundle id (`Bundle::id`) and its scripts
by full hash, with an index by the record's 8-byte prefix. A bundle that
fails `Bundle::integrity` is left out with a log line. Store details: see
the [detail note](notes/2026-11-26-rules-engine-detail.md), "Live game
variants in the game".

- **In the variant list.** The store's variants join the built-ins at each
  load of the main menu's tags (`inject::restore_listed`,
  `inject::apply_block`).
- **From Live.** `game_variants::refresh_live` fetches the player's variants
  and any missing scripts on its own thread.
- **Porting.** At boot, `variants::portable::port` ports each saved variant
  under `UDATA\4D530064` once; the saves are only read.
- **Guests** never need the script: the host runs it.

The host puts the running blob's full hash on its session
(`Hle::set_session_rules`). Tests:
`crates/extensions/game/src/game_variants/tests.rs`.

## The language

A source file is a header, a settings block, options, variables,
functions and handlers. The header names the script, its version, its
game type (`type`), the built-in game it plays if any (`uses`), what a
point is called (`points`), its teams, its description, its Quick Options
marks and the capabilities it requires (`requires player.weapons,
hud.message`). The design is
[game-types-without-carriers.md](game-types-without-carriers.md#the-header).

```text
rules "SWAT" 1
type slayer
uses slayer
about "No shields, no motion sensor, battle rifles and magnums. One headshot kills. 50 points to win."
quick score_limit, starting_weapon, secondary_weapon

settings {
    team_play = true
    score_limit = 50
    shields = none
    starting_weapon = battle_rifle
    weapons_on_map = none
}
```

`type`, `uses`, `points`, `settings`, `quick` and `about` are header words
only at the start of a top-level line (`Parser::header_word` in
`lang/parse.rs`). Each is given at most once.

**The type.** `type <word>` is required: a row of
`rules::types::TABLE` (`crates/formats/rules/src/types.rs`). Retail types:
`slayer`, `ctf`, `assault`, `oddball`, `koth`, `juggernaut`, `territories`.
Ours: `research`, `party`, `firefight`, `siege`. `Header::engine` reads the
engine index; our types use Slayer's layout. A missing or unknown type is a
named error. Engine indices and filing rows: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The language".
See [the list](community-gametypes.md#tweakers) and
[the start event](game-types-without-carriers.md#the-display).

**Uses.** `uses <type>` plays that retail type's built-in rules under this
script's settings (see "Who scores"). `points "Label"` names a point, 1 to
32 bytes.

**Retired lines.** `carrier`, `list` and `scoring` are refused with
`header.retired`; the message names the replacement.

**The settings block.** `settings { name = value ... }` names the record
settings a new variant starts with. Names are the rows of
`rules::settings::TABLE` (`crates/formats/rules/src/settings.rs`); rows are
appended, never reordered, since a blob names a setting by its index.

| Kind | Written as | Example |
|---|---|---|
| bool | `true`, `false` | `motion_sensor = false` |
| int | a number in the row's range | `score_limit = 50` |
| seconds | whole seconds | `time_limit = 300s` |
| choice | a name from the row's list, or a number | `starting_weapon = shotgun` |
| percent | 10 to 400 | `gravity = 50` |

`score_limit`, `time_limit` and `king_moving_hill` take `none` for 0. The
checker (`Cx::settings_block` in `lang/check.rs`) reports `setting.unknown`,
`setting.type`, `setting.twice` and `setting.value`.

**Quick Options marks.** `quick a, b, c` names what the lobby's Quick
Options shows: at most five (`ir::MAX_QUICK`), each a record setting or
option (`quick.many`, `quick.unknown`, `quick.twice`). Options and settings
share one namespace (`option.setting`).

**Who scores.** With no `uses` line the script is the game's one scorer. A
`uses` script holds header lines, settings, `quick` and `about` only
(`uses.handlers`). A script with no handler and no `uses` is `scoring.none`
(`Cx::scorer`).

**About.** `about "..."` describes a variant: at most 150 UTF-16 units
(`ir::MAX_ABOUT_UNITS`) and 256 bytes.

**Modules.** `use "waves.rules"` reads another file in before the checker
runs (`lang::modules::expand`, `lang::diagnose_files`,
`lang::compile_files`). The compile is of one text, so nothing after
expansion knows a module. Limits: 32 files, 8 deep, 256 KiB; a cycle is the
error `use`. `h2 rules check|build|run|sim|settings` reads files beside the
script; a variant file carries them in `script.modules`
([variants.md](../editor/variants.md)). Findings carry `Diagnostic::file`;
`ide::diagnostics_with_files` and `h2 rules lsp` report them on the `use`
line.

**Types.** `int` (32-bit, saturating), `fixed` (16.16), `bool`, `player`,
`team`, `object`, `zone`, `squad`, `tag`, `weapon`, `time` (ticks: `5s`,
`300t`, `1.5s`), `timer` (counts down, then fires `on timer`), and declared
`enum` types. `T?` may be `none`. Mixing `time` and `int` is an error that
gives the fix; `seconds(n)`, `whole_seconds(t)` and `ticks(t)` convert. An
`object` is the engine's 32-bit datum; `none` is `0xFFFFFFFF` (`ty::NONE`),
a labelled thing without an object is `0x4000_0000` plus its placement id
(`ty::object_id`, `object_value`). A `table` is constant rows of typed
columns, indexed by `int`. There are no floats or strings at run time.
Time rules, handle encoding and table lookups: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The language".

**Expressions and control.** Ordinary expressions, `if` / `else if` /
`else`, short-circuit `and` / `or`, `match` over an enum. A `let` or
parameter is not reassigned; `let x = none` needs a type. These are what the
ReachVariantTool dialect lacks ("Prior art").

**Constants and value functions.** `const` values fold in the checker and
stand wherever a literal does. `fn name(a: T) -> R = <expression>` is
inlined at each call; a block `fn` has no value.

**`T?` and narrowing.** A `T?` becomes a `T` only where a check proves it
is not `none`: `if x != none`, the `else` of `x == none`, `and`/`or`/`not`,
and `if let k = x`. Checks narrow a `let`, parameter, loop member or handler
argument; a global is never narrowed.

**Checking.** `crates/formats/rules/src/lang/check.rs` reads the whole
script, reports up to 50 errors in source order with codes, and the
warnings. `lang/lower.rs` runs on a clean script and makes no type decision.
Codes: [rules-language-types.md](rules-language-types.md#diagnostics-and-the-editor).

**Variables.** `global|player|team|object var name: type = init`. Budget:
256 global, 64 per player, 32 per team, 32 per object, 256 objects of a
label. A scoped variable is read through its owner (`p.infected`).

**Labels.** `label <name>: <kind> [requirements] [= "map text"]` declares
the Forge labels a script reads. Kinds: `object`, `zone` (and `?`
forms), `objects`, `zones`, `spawns`. Set requirements: counts, `numbered`,
`numbered a..b`, `per team`. Declarations fill the IR's label table in
order, which the start gate checks
([rules-language-types.md](rules-language-types.md#labels)). Indexing,
`is`, `count` and set arguments: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The language".

**Handlers.** Events: `init`, `round_start`, `tick`, `kill`, `spawn`,
`death`, `pickup`, `drop`, `enter`, `leave`, `press`, `timer`, `squad_dead`,
`actor_kill`, `actor_assist`, `damage` (16 a tick), `bounce`, `bounced`,
`arrive`, `touchdown`, `launch`, `grounded` (16 a tick each), `edit`,
`join`, `quit`, `round_end`. A handler names its event's arguments in order
and may leave out trailing ones. A label in the zone or object place picks
the handler's things (`on enter(goal, o)`). Handlers of one event are one IR
handler, in source order, each run to completion. Argument meanings: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The language".

**Loops.** `for each` over `players`, `teams`, `objectives` (at most 8) or a
label's set, with optional `where`. No `while`, no recursion, functions are
inlined, so every handler has a static worst-case op count, which the
compiler prints.

**Names, not datums.** Engine things are named by tag path (checked against
the map at load) or by a declared Forge label. A script never sees a datum
index or an address.

**Options.** `option name: type range = default` declares a variant editor
row. `variant.<field>` reads a retail record field by name
(`caps::VARIANT_FIELDS`, naming each `record::SettingId` and offset). Which
fields have inferred meanings: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The language".

### Slayer

```text
rules "Slayer" 1
type slayer

option kill_points: int 1..5 = 1
option suicide_costs: bool = true

on kill(victim, killer) {
    if killer == none or killer == victim {
        if suicide_costs { score(victim, -1) }
    } else if variant.team_play and killer.team == victim.team {
        score(killer, -1)
    } else {
        score(killer, kill_points)
    }
}
```

`score` is `SCORE_ADD` (**E**, `0x15b7c0`, firefight.md "Guest boundary"),
which adds the team score and tests the score limit.

### Infection: a team switch

Infection uses `set_team`, `set_loadout`, `pick` and `end_round`; it waits
on `set_team`, which has no traced engine entry ("The engine API"). Source:
see the [detail note](notes/2026-11-26-rules-engine-detail.md), "Infection:
a team switch".

### Firefight: waves of AI

Firefight is a `table` of waves, `place_squad`, `on timer`, `on squad_dead`,
`on actor_kill` and `on death`. Source: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Firefight: waves
of AI".

### Phases: attack and defend

Phases use `const` phase counts, numbered `barrier` and `spawns` labels,
`timer` and `time` options, and `ticks_held`. The built-in Siege
(`content/modes/rules/siege.rules`, listed in Party Games) is the part a
host runs: attackers take sites, then the final stand; each phase has a
clock, and the start gate refuses a map with no site or final stand
([rules-start-gate.md](rules-start-gate.md), `gate.rs` `map_problems`).
Siege's HUD is the King of the Hill kit
([rules-world-api.md](rules-world-api.md#the-huds-a-script-feeds)), and its
`kits` pickers use `offer_pick`, `pick_row` and `picked`
([rules-world-api.md](rules-world-api.md#pickers)). Source and Siege's
labels, spawns, overtime and HUD: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Phases: attack and
defend".

### A Forge graph is the same IR

The Forge front end turns the document's nodes and links into the same slots
and handlers: a node's state is a global slot named by node id, and a link is
a call in link-id order. The graph's budget rule lowers to a queue in slots
(forge-features.md). Links that retail data expresses whole stay retail
data. A map's graph and the game type's script are two blobs in one VM; the
game type's handlers run first. Example: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "A Forge graph is
the same IR".

## The IR and the VM

**Shape.** The blob holds a header (magic, IR version, API version, name,
version, type byte, `uses` byte, team requirement, `points`, `about`,
required capabilities, at most 32), a string table, option table, settings
(`ir::Setting`), Quick marks (`ir::Quick`), slot table, label table, tag
table (checked against a map by the start gate,
[rules-language-types.md](rules-language-types.md#the-ir)), constant tables,
handler table and code. Code is register bytecode with typed ops; there is
no backward or indirect jump, so termination is a property of the format.

**Settings and marks.** `Ir::decode` and `verify::settings_and_marks` refuse
malformed settings, marks, type and `uses` bytes, so a blob from anywhere
holds to the compiler's rules. `Ir::id` hashes the whole encoding.

**Ops** (IR version 1, `crates/formats/rules/src/ir.rs`): `Const`, `Move`,
`Arith`, `Neg`, `Cmp`, `Not`, `Load`, `Store`, `Opt`, `Variant`,
`TableRead`, `TableFind`, `Jump`, `JumpIf`, `JumpIfNot`, `ForEach`, `Call`,
`Grant`, `Revoke`, `Granted`. Handlers declare every register's type, and a
jump inside a `ForEach` body cannot leave it. A read through `none` gives the
slot's initial value; a write does nothing.

**Budget.** A tick may execute 100,000 ops across all handlers (about 1.8 ms
at 18 ns an op, `op_cost` in `crates/formats/rules/tests/scripts.rs`). The
count is always enforced. `on enter` and `on leave` together run at most 64
times a tick. The verifier reports a **worst-tick estimate**, which Studio
shows and the allow-list requires to fit. Going over is a **script error**:
it stops the VM, ends the round through `GAME_END` ("script error") and is
logged, at the same op on every run. Reasoning: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The IR and the VM".

**Order.** Handlers run where their event happens, so calls land where the
lifted engine's did.

| Handlers | Run | Their calls |
|---|---|---|
| `on kill` and each event the carrier has a callback for | synchronously, in place of that callback | made at once |
| `on enter`, `on leave`, `on press`, `on timer` (slot order), then `on tick` | synchronously, in place of the per-tick update (`0x15c150`) | made at once |
| `on squad_dead`, `on actor_kill`, `on actor_assist`, `on damage`, `on touchdown`, `on launch`, `on grounded`, `on bounce`, `on bounced`; `on init` | after `GAME_TICK`; `init` after `GAME_ACTIVATE` | queued |

An event with no carrier callback runs with the per-tick group. Squad
commands and object creation, deletion and teleports queue from any handler
and run in program order at the next `GAME_TICK` `before`. A read sees no
queued call. `for each` visits players by slot, teams by index, objects by
label then placement id.

**Determinism.** Integers and fixed point only. Float tests run in Rust under
[sim-rewrite.md](../engine/sim-rewrite.md#float-rules-for-every-rust-sim-kernel-enforced-in-sim-core).
Random draws come from a SplitMix64 generator in `RulesExt`, seeded from the
match seed and blob hash; the game's `[0x4e7408]` generator is never drawn
from. Division by zero gives 0 and a trace warning.

**Versioning.** A build with the `rules` feature advertises `rules/1`
(`live_proto::capability::RULES` in `crates/live/live-proto/src/lib.rs`,
`live_glue::capabilities`). An older build cannot host a script game; as a
guest it is warned (`variant_ext::session_guests`, `Hle::players_lacking`,
game-options.md "How it reaches every player"), not refused, and shows
the carrier engine's labels. The IR version changes when ops change, the API
version when an entry's meaning changes; a VM refuses what it does not know.

**Films.** The blob, options and seed are replay inputs, and the `rules`
hooks are part of `simulation_hooks`
([film-format.md](film-format.md#determinism-and-verification)). The VM
re-runs in playback. Script-made objective changes need their own film
observer (film-format.md "Objective and power-up events").

## The engine API

The API is a numbered list of entries; a script can call nothing else.
An action's **Cap** column is the capability its script must declare
("Roles, powers and capabilities"); `core` needs none, and reads need
none. Status: **named** is a point or layout in `crates/hooking/guest-layout`
or `crates/rewrites/engine`; **traced** is an address in a doc with no
named point yet; **untraced** has no known entry. The full tables, with
every address and the reasoning behind each row, are in the
[detail note](notes/2026-11-26-rules-engine-detail.md), "The engine API".

Reads:

| Read | From | Status |
|---|---|---|
| scores, kills, deaths | statborg `E+0x304` (`game::STATBORG`) | named |
| players: present, alive, team, unit, lives | player datum (`guest-layout` `player.rs`; team `+0xc0`, lives `+0x1ac`) | named |
| team play, score and time limits | variant record `V` (`variants::record`, [live-gamestate.md](../live/live-gamestate.md)) | named |
| `round_time_left()` | `E+0x70` (`ROUND_START_TICK`, likely) and `V` | named |
| objects: tag, position, parent | object datum (`guest-layout` `object.rs`) | named |
| objects by label, `zone.label`, `zone.id`, `o.number` | the host's census (`rules/census.rs`) | named |
| objectives, `o.carrier`, `o.team`, `is_objective` | `E+0xc18`, `E+0xc1c`, record `+4`, `E+0x10` (`engine::objectives`) | named |
| `player_of(o)` | the roster's units (`firefight::runtime::read_players`) | named |
| `inside`, `on enter`, `on leave`, `ticks_held` | the host's zones (`rules/world/zones.rs`); sizes are ours | named |
| netgame flags by type and team | `scnr+0x118`; query `0x19ec40` | named; traced |
| per-engine globals (`E+0xfc`) | `engine::objectives`; `KOTH_GLOBALS` in `global.rs` | named |
| events of the tick | `PLAYER_KILLED` `0x15ce70`; the roster each tick (`runtime::tell`) | named |
| squads, `on squad_dead`, `on actor_kill`, `on actor_assist` | `actor_net::squads::count_by_squad`, Firefight's `Observed` | named |
| `zone_seen`, `zone_distance`, `zone_team_distance` | Firefight's `Observed` (`ZoneSeen`) | named |
| superbounce events (`on bounce`, `on touchdown`, `on launch`, `on grounded`) | `superbounce_chain.rs`; [superbounce.md](superbounce.md#medals) | named; delivered while the mode runs |
| bounce and crouch spots, as zones | `crates/live/superbounce` scanner | named; candidates only |

Actions:

| Action | Through | Status | Cap |
|---|---|---|---|
| `score(p, n)` | `SCORE_ADD` `0x15b7c0` | named | core |
| `end_round(winner)` | `GAME_END` `0x1389c0`; round advance and winner not traced | named; untraced | core |
| lives | player `+0x1ac`, `PLAYER_ENTITY_DIRTY` `0xa7840` | named | core |
| `respawn(p)` | `PLAYER_SPAWN` `0x14f2e0` | named | core |
| `set_gravity(g)`, match traits | `variant_ext` hooks (`crates/extensions/game/src/variant_ext.rs`); the host does not call `set_gravity` yet | named | `game.traits` |
| per-player traits, speed | `ENGINE_TRAIT_QUERY` `0x24dda0`; speed step `0x15c000` | named point | `player.traits` |
| `set_team(p, t)` | none known; field player `+0xc0` | untraced | `player.team` |
| `set_loadout(p, primary, secondary)` | `STARTING_WEAPON_PICK` `0x19eda0`, then the profile's give (`rules/world/loadout.rs`) | named | `player.weapons` |
| `create`, `delete` | `OBJECT_DELETE` `0xb8540`; creation as Forge's | named; traced | `objects.create`, `objects.delete` |
| `register_objective` and kin | `0x15e050`, `0x15e130`, `0x15e250`, `0x15e460` | traced | core |
| `spawn_objective` | as Oddball's `0x2bc1f0`; not built (no tag path resolves to a datum) | traced | core |
| `reset_objective` | Oddball `0x2bc340` via `0x15e360` | named on Oddball | core |
| `role_traits(p, on)` | the VM's player mask and `ENGINE_TRAIT_QUERY` | named | core |
| `set_open(o, open)` | the VM steps a barrier `GATE_DROP` (64 units) through `OBJECT_SET_TRANSFORM` (`rules/world/objects.rs`); at most 16 open | named; collision effect not researched | `objects.move` |
| `place_squad`, `hunt`, erase, `place_squad_at` | `AI_SQUAD_PLACE` `0x201520`, `AI_SET_TEAM` `0x275b60`, `AI_SET_ORDERS` `0x275cb0`, `AI_ERASE` `0x2736c0` | named | `ai.squads` |
| vehicles: `fly`, `hover`, `seat`, `eject`, `unload`, `place_squad_in` | [rules-world-api.md](rules-world-api.md#vehicles-and-drop-ships) | `hover`, `unload` named; `fly` not built | `vehicles.drive` |
| `spawn_areas`, `spawn_near` | `SPAWN_LOCATION_FREE` `0x23bb70` (`rules/world/spawns.rs`) | named | `player.spawn` |
| loadout at spawn, `offer_pick`, `pick_row`, `picked` | [rules-world-api.md](rules-world-api.md#pickers) | named | `player.weapons`, `player.pick` |
| `give(p, tag)` | no entry proven | untraced | `player.weapons` |
| `take(p, weapon)` | `0xce920` from the drop path `0xe8de0` | traced | `player.weapons` |
| `set_body(team, tag)` | model byte (player `+0x88`) around `PLAYER_SPAWN` (`rules/world/body.rs`) | named | `player.disguise` |
| `show_area`, `hide_area` | VM state `RulesExt::shown`; `rules/world/walls.rs`; no engine call | named | none |
| `hide`, `show`, `attach`, `camera` | none known | untraced | `player.disguise`, `objects.move`, `player.camera` |
| `teleport`, `move_to`, `stop`, `o.position`, `on arrive` | `OBJECT_SET_TRANSFORM` `0xb7430`; `RulesExt::movers` | named | `objects.move` |
| device power | Forge's sinks | not built | `objects.move` |
| `p.airborne`, `p.crouching`, `p.primed`, `sees`, physics events | `superbounce_charge`, `AI_LINE_OF_SIGHT` at the tick boundary; [rules-world-api.md](rules-world-api.md#physics) | `sees` hostable with a one-tick delay; scripts using the reads declare `requires hud.superbounce` | none |
| `message(to, text)` | notification queue `0x24cbee` (`hud_kit::NOTIFY`); [rules-language-types.md](rules-language-types.md#messages), [rules-world-api.md](rules-world-api.md#the-show-channel) | named | `hud.message` |
| `hud_*`, `mark`, `unmark` | VM show state `RulesExt::show` (`rules/world/hud.rs`) | named | `hud.widgets`, `hud.waypoints` |
| `ff_*` | Firefight's `firefight::Command` host log | named | `firefight.director` |
| `sound_to`, `sound_to_team`, `sound_to_all` | `SOUND_START_AT_MARKER` `0x1891d0` (`rules/world/audio.rs`) | named | `audio.play` |
| `bot_go`, `bot_hold`, `bot_follow`, `bot_attack`, `bot_free` | `bots::phantom::order` (`rules/world/bots.rs`); no guest entry | named | `bots.orders` |
| `forge_move`, `forge_delete`, `forge_place`, `forge_scale` | `forge-doc` op, host-stamped (`LogHead::stamp`) | named | `forge.move`, `forge.delete`, `forge.place`, `forge.scale` |

VM built-ins reach no engine entry (status **VM-only**):

| Built-in | What it is |
|---|---|
| `count`, `first`, `len`, `min`, `max`, `clamp` | over a finite set or constant table; `first` gives `none` for an empty set |
| `vec_length`, `length`, `distance`, `o.speed` | 16.16 length (`ty::fixed::length`) |
| `pick(set, n)`, `random(n)`, `now()` | the VM's generator; the tick the host began |
| `seconds`, `whole_seconds`, `ticks` | tick-rate conversions |
| `ticks_held(zone, team)` | per zone and team in `RulesExt`; at most 32 zones; clears at round end |
| `squad(n)`, `team n`, `text n` | constants |
| `fmt_*` | message hole values the compiler emits; at most four per message |
| `grant`, `revoke`, `granted` | a player's powers in `RulesExt` |

`ENGINE_UPDATE` (`0x15c150`) and the post-game step `0x15ca10` are where the
lifted engines do their per-tick work; they are not API entries. The engines'
vtables are in "Which engine index a script rides".

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
| `hud.superbounce` | shows the superbounce guide | no |
| `firefight.director` | runs Firefight rounds, waves, lives and scores | no |
| `forge.move`, `forge.delete`, `forge.place`, `forge.scale` | lets players edit objects during the match | yes |

**Declared, then checked everywhere.** The header's `requires` list is
the most a script can ever do. The verifier refuses a call, or a `grant`,
whose capability is not declared. The lobby shows the build's plain words, a
host build refuses a script naming a capability it lacks, a hopper's deny list
keeps a script out (`forge.*` denies the family), and ranked always denies
`forge.*`. Detail: [detail note](notes/2026-11-26-rules-engine-detail.md),
"Roles, powers and capabilities".

**One policy** answers all of them (`rules::policy`,
`crates/formats/rules/src/policy.rs`): a kind of game (`Context`:
custom, hopper with its deny list, ranked, Theater) and a header's
`requires` give the refused capabilities (`policy::check`). Custom and
Theater refuse none, a hopper refuses its list, ranked refuses every grantable
capability, and a name the build does not know is refused everywhere. A granted
power is used in custom and hopper games, never in ranked or Theater
(`policy::edits_allowed`; `Vm::check_proposal`). Four places ask it:

| Where | Kind of game |
|---|---|
| Live's hopper edit (`PUT /v1/titles/{title}/hopper`, `admin::check_scripts`) | ranked for hopper types 2, 4 and 5, else a hopper |
| Live's session (`POST /v1/sessions`, `Live::check_session_rules`) | ranked when the flags say so |
| Live's publish routes (`game_variants::eligible`) | all; records `eligible`, never refuses |
| The host at the game's start (`Runtime::start`, `rules/feature.rs`) | hopper when matchmade (`Hle::matchmade`), else custom |

**Roles and grants.** A role is an `enum` in a player variable; a power is a
`grant(p, cap, budget n, palette "x")` the script makes when the role changes.
A grant is a record in `RulesExt` (player, capability, uses left, palette), so
checkpoints, snapshots and films carry it. It never exceeds the declaration,
holds at most 255 uses, and ends on `revoke`, quit and round end. A palette is a
constant list of tag paths checked at load. Example script: see the note.

**A granted power in use.** The player's tool sends a Forge proposal
(`{author, base_seq, op}`, [forge-design.md](forge-design.md#the-edit-log-and-multiplayer)).
The host checks it as `forge-doc` does, with the author's live grants as the
permission, stamps it (`LogHead::stamp`), charges one use, and runs
`on edit(p, o)`. A refused proposal changes nothing. A script's own `forge_*`
calls take the same path with the reserved author `rules`.

**What an edit can reach** is forge-design.md's
[edit-class table](forge-design.md#what-the-engine-allows-at-runtime), with no
Rebuild in a match: move, turn and delete are live, a tag the map holds can be
made inside the object pool, and structure never changes. The match table is in
the note.

**Budgets.** A match holds at most 1,024 scripted and granted edits and 64
objects alive from them, under Forge's own budget (forge-features.md "Objects").

**The log.** A match with a `forge.*` script keeps a Forge edit log whose first
entry names the script's hash. Every scripted edit and granted use is an entry,
recorded in the film at its tick. The map's document is not changed.

## Security and usability

The VM is the boundary: the verified IR, a VM in Rust, fixed bounds and an API
with no I/O. Each point has one line here and its reasoning in the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Security and
usability".

- **Allow-listed where results count.** Ranked hoppers and Live's stats and
  ranks accept only a script hash on Live's allow-list.
- **Verified twice**, at upload and at load, by the same code.
- **Capability-limited.** The API table is the whole world; a missing tag or
  label is a load error.
- **Bounded.** IR, state, ops per tick, labels, objects, grants and edits have
  caps.
- **Text.** Labels are at most 32 bytes and messages 128.
- **Input rate.** `on press` fires at most 10 times a second per player.
- **Clean failure.** A script error ends the round.

Tooling:

- **Studio**: the Game type script window
  ([editor-guide.md](../editor/editor-guide.md#game-type-scripts);
  `crates/apps/studio/src/app/rules_edit.rs`) uses `rules::ide`, checks with
  `rules::workbench::check`, and reloads the script into the paired game
  ("Reloading a running script").
- **Command line** (`crates/apps/h2-tools/src/rules_cmd.rs`; pure over its
  inputs, no disc):

| Command | What it does |
|---|---|
| `h2 rules check <file.rules>` | compile and verify; errors as `file:line:col: message [code]`, exit 1; report of worst tick, IR bytes, slots, blob id, capabilities |
| `h2 rules build <file.rules> -o <out.rir>` | write the verified blob and print its SHA-256 |
| `h2 rules run <file.rules> --scenario <s.json> [--trace]` | step a scenario on the fake game |
| `h2 rules sim <file.rules> [--players N] [--teams N] [--seed S] [--matches N] [--length SECONDS] [--score-limit N] [--json] [-o <s.json>]` | play generated matches (`rules::sim`) |
| `h2 rules caps` | list capabilities and API entries with status |
| `h2 rules builtin [name]` | list engine ports (`builtin::ALL`) and example scripts, or print one |
| `h2 rules lsp` | language server on stdio (`crates/apps/h2-tools/src/rules_lsp.rs`) |
| `h2 rules editors [status\|install [vscode\|zed] [--build]]` | install the VS Code extension; Zed steps ([rules-editors.md](../editor/rules-editors.md)) |

- **Language intelligence**: `rules::ide` (`crates/formats/rules/src/ide/`) is
  pure over source text; the language server, Live's `POST /v1/rules/ide`, the
  web editor and Studio share it. Positions are LSP's (0-based line, UTF-16
  `character`). The lexer and parser recover (`lex::lex_tolerant`,
  `parse::parse_recovering`), so every feature works from what parsed. Features:
  `diagnostics`, `actions`, `rename`, `complete`, `hover`, `signature_help`,
  `definition`, `references`, `highlights`, `prepare_rename`, `folding_ranges`,
  `semantic_tokens` (legend `ide::LEGEND`), `symbols` and `format`. Diagnostic
  codes: [rules-language-types.md](rules-language-types.md#diagnostics-and-the-editor).
  Each feature's behaviour is in the note.
- **Editors**: VS Code and Zed have extensions in `editors/` that start
  `h2 rules lsp` ([rules-editors.md](../editor/rules-editors.md)). Any other
  editor points a generic LSP client at `h2 rules lsp` for `.rules` files;
  Helix and Neovim configs are in the note.
- **Scenario tests**: a scenario is JSON read by `rules::scenario`: `seed`,
  `options`, `players`, `sight`, `teams`, `variant`, `map`, `round_time_left`
  and `ticks` with `events`. The fake game is the one the workbench steps
  (`scenario::step`). `h2 sim` runs a script in a headless match. Each built-in
  script has scenario tests beside its source. The field list is in the note.
- **Agent API**: `rules.state` (script, hash, refusal, budget use),
  `rules.vars`, `rules.trace` (handlers run and calls made per tick),
  `rules.step` and `rules.load` (swap a verified blob between rounds).
  They join [agent-api.md](../runtime/agent-api.md) when built.

### Workbench

**Code.** The web app's Game Types page (`statsPath("/rules")`,
`web/src/stats/Rules.tsx`, `RulesTest.tsx`, `web/src/lib/rules.ts`) is
where a script is written, checked, tested and saved on the web. Studio's
Game type script window is the desktop editor over the same `rules::ide`
and workbench. The browser runs no compiler: Live's
[Rules scripts](../live/live-api.md#rules-scripts) routes call
`rules::workbench` in both homes.

The **Workbench** tab is one screen: editor, check, generated matches and
one match's results. The other tabs are **My Scripts** and **My Game
Types**. Layout and copy are in
[web-app.md, "Game type scripts"](../live/web-app.md#game-type-scripts).

| Part | What it does | Code |
|---|---|---|
| Check | compiles, verifies and measures source (IR hash, errors, worst-tick estimate against 100,000 ops, slots, IR bytes against 256 KiB, capabilities, entries no host can make); runs 450 ms after typing stops; the `labels` show as **What a map needs** ([rules-language-types.md](rules-language-types.md#the-workbench)) | `workbench::check` |
| Editor | Monaco, a chunk of its own; `h2rules` language, completion, hover, go to definition, references, rename, format, quick fixes through Live's `POST /v1/rules/ide` and `GET /v1/rules/ide/legend` (LSP field names) | `web/src/stats/RulesEditor.tsx`, `rulesMonaco.ts`, `web/src/lib/rulesIde.ts` |
| The map | a scenario's `map` lists labelled things; with none, the script runs on `labels::minimal`; a map is checked first and a problem is `RunError::Map` ([rules-start-gate.md](rules-start-gate.md#the-check)) | `scenario::ScenarioLabel`, `scenario::resolve_map`, `scenario::MAP_BASE` (0x100) |
| Run | steps a scenario over `fake::FakeGame` through `scenario::step`, as `h2 rules run` does; stops at a script error, after `end_round`, or past 12,000,000 ops; same input, same trace | `workbench::run`, `workbench::Runner` |
| Fake world | `FakeGame::apply` keeps objective carriers and zones; the Runner makes spawned objectives (from `SPAWNED_BASE`, 0x4000), `h2 rules run` does not | `FakeGame`, `SPAWNED_BASE` |
| Generated matches | a seeded match made from the IR's handlers and calls, played on a `Runner`; `sim::batch` plays up to 100 seeds and flags problems; the page plays 25 after 350 ms | `sim::simulate`, `sim::batch`, `sim::to_file`, `UNIT_BASE` (0x1000) |
| Save | a signed-in account saves a script that checks clean; Live keeps the source under the IR hash and recompiles it to serve the blob | Live `/v1/rules` |
| Game variants | **Make a game variant** wraps the script in a stock record (mode 2, IR hash prefix, packed options); **Download** uses `POST /v1/variants/bundle`, **Publish** posts to the account ([live-api.md](../live/live-api.md#game-variants)) | `VariantRecord::stock`, `game_variants::pack_options` |
| Drafts | source, setup and hand match live in `localStorage`; starters come from `builtin::ALL`, `builtin::examples` and `GET /v1/rules/builtin` | `builtin` |

Detail: see the [detail note](notes/2026-11-26-rules-engine-detail.md),
"Workbench".

## Porting the seven engines

Each retail engine has a script in `content/core/rules/` found by
`builtin::builtin(engine index)`. The scripts declare no capability: the
objective entries and `role_traits` are `core`.

| Engine (index) | Script | Needs |
|---|---|---|
| Slayer (2) | `slayer.rules` | the VM |
| Oddball (3) | `oddball.rules` | objective registry, traits, globals `[0x51ecc4]` |
| King of the Hill (4) | `koth.rules` | zone tests, replicated globals entity |
| Juggernaut (7) | `juggernaut.rules` | traits, transfer to the killer (`0x192970`), globals `[0x510c9c]` |
| CTF (1), Assault (9) | `ctf.rules` (one port, `type ctf` or `type assault` header, `const assault`) | registry, team timers, arming volumes (`0x459d18`) |
| Territories (8) | `territories.rules` | zone tests, markers by territory index |

Layout facts are **E** from bots.md section 3.7. `tests/engines.rs`
checks each header's list of missing host entries against the blob and
each engine's core rule over `fake::FakeGame`. No port has a parity run.
Only Slayer's and Juggernaut's entries are all hostable, so only
Juggernaut runs for a no-mode record (`runtime::PORTED`); the others run
in tests. What each port waits on and does not reproduce: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Porting the seven
engines".

**Parity method.** The lifted engine is the oracle. A recorded headless
match runs on the lifted engine and on the script, and the bar is run
parity ([sim-rewrite.md](../engine/sim-rewrite.md#verification-ladder-every-phase-climbs-all-of-it)):
with `impl.sim.arena-hash` on, `h2 sim diff` prints nothing. A port is
proven per built-in variant, on at least three maps, with phantom seats.

**Variants.** The 39 retail built-ins keep their bytes, have no mode and
run their engine's script once it is in `runtime::PORTED` ("A record with
no mode"). The 14 community variants (`rules::builtin::LISTED`) are built
by `variants::portable::record_for` and name their script (mode 2,
[rules-variants.md](rules-variants.md)). To travel as a game variant file
a record is ported with `variants::portable::port` (`h2 variants port`,
[variants.md](../editor/variants.md#portable-files)): `PortScript::Builtin`
attaches the built-in script, `PortScript::Keep` leaves no mode. Flags:
`--name`, `--description`, `--option key=value`, `--all-builtins`. Options
pack through `portable::port_with_options` and `Ir::pack_options`.
Rules that were honour rules ([community-gametypes.md](community-gametypes.md))
become script variants.

**Until a port passes.** A no-mode record on an engine not in
`runtime::PORTED` plays the lifted engine, on every kind of session.

**Old films do not replay.** A film replays only with the hook set and
variant bytes it was recorded with (`simulation_hooks`, film-format.md). A
film recorded on a lifted engine is refused with that reason.

**Retirement criteria**, all of them: every port has passed parity, and
script games have been played in every engine on Live and System Link.
Then `rules` is always on and the lifted engines' hooks and bot readers
(`crates/rewrites/engine/src/objectives.rs`,
`crates/live/bots/src/objective.rs`) read rules state.

## Firefight, AI and bounces

**Firefight runs as a script.** `content/modes/rules/firefight.rules` owns
waves, rounds, lives, skulls and scoring. `firefight::ScriptDirector`
(`crates/formats/firefight/src/script.rs`) supplies the validated
`firefight::Def` as constant tables and steps the script each tick
([firefight.md](firefight.md#the-director-as-a-rules-script)).

**AI is commanded by squad.** A script places a squad at a label, sets its
team and orders, reads its live count, and hears `on squad_dead` and
`on actor_kill` ([ai-networking.md](ai-networking.md)). Actor brains stay
the engine's. Phantom bots ([bots.md](bots.md)) are players to a script.

**Bounces are observed, not made.** Physics stays the engine's
([high-framerate.md](high-framerate.md)). A script hears `on bounce(p,
spot, chain, monster)`, `on bounced(p)` and the physics events (`on
touchdown`, `on launch`, `on grounded`,
[rules-world-api.md](rules-world-api.md#physics)). The detector's rule is a
traced signature, so it can miss or over-report until checked in play.

**Code**, tests only. The detector is the superbounce medal chain
(`superbounce_chain`, [superbounce.md](superbounce.md#medals)); events go
through `Runtime::physics` and `Runtime::bounces`
(`rules::feature::bounces`). The spot id is
`superbounce_chain::spot_id`; per-strip state is capped at
`vm::MAX_OBJECT_RECORDS` (256). Events fire only while the superbounce
mode is on. Detail: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Firefight, AI and
bounces".

**The Superbounce game type is a script**,
`content/modes/rules/superbounce.rules`, an example (`builtin::examples`;
options in [superbounce.md](superbounce.md#the-superbounce-game-type)). Its
record comes from `record_for`, written at list load
(`community_variants::write_blocks`). The game reads the header through
`rules::header`:

| Header | Turns on | Game side |
|---|---|---|
| `requires hud.superbounce` | the mode: HUD, marks, priming, medals | `header::script_asks_superbounce`, `variant_ext::Decision::superbounce` |
| `type` | list row and round-start line | `community_variants::filed_type`, `rules::start_line` |

Both find the script by prefix in `runtime::compiled_in` and the
game-variant store.

**Options the host reads.** The HUD's piece options (`targets`,
`velocity`, `pogo`, `pogo_style`, ...) sit in the record's option bits and
are read by name through `Ir::option_value` and
`rules::header::superbounce_options` ([superbounce.md](superbounce.md#the-huds-options),
[pogo-stick.md](pogo-stick.md)); the script never reads them.

## Payoff

The scripted modes are examples in `content/modes/rules/`, listed by
`builtin::examples()`: `infection`, `gun_game`, `vip`, `siege`, `race`,
`bounce_hunt` and `superbounce` (`.rules`). Each compiles, verifies and
fits a tick (`tests/engines.rs`). Superbounce, Siege, Gun Game, Race and
VIP run and are listed (`builtin::LISTED`,
[rules-variants.md](rules-variants.md#built-in-list-entries)). A record
names an example by hash prefix, with no store or fetch.
`tests/modes.rs` plays the listed modes over the JSON scenarios in
`tests/scenarios/`. Prop Hunt has no script.

| Mode | Status |
|---|---|
| Race, Gun Game, VIP | listed, in Party Games |
| Infection | planned with a real team switch |
| Prop Hunt | blocked: `hide`, `attach`, body swap and `camera` untraced |
| Siege | built; the phased-mode API check |

Per-mode mechanics and the Siege API table: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Payoff".

## Forge labels

A prerequisite. `crates/formats/forge-doc` has links (`links.rs`) and
labels (`labels.rs`) to tell a script which placement is which. The
document side is built and the host reads labelled things
(`rules/census.rs`); the `RulesExt` table a script's declared labels read
is not.

**Design.** A label is a string of at most 32 bytes plus an optional
number, a field of a placement set through the edit log. At map load the
labels become a `RulesExt` table. Retail maps with no Forge document get
labels from a built-in table per netgame flag type.

**Built.** `Forge::Label { target, label }` sets or clears a label;
`ForgeDoc::labels` resolves them and `replay` hands them to
`Apply::labels`. `forge_doc::builtin_label(flag_type)` is the built-in
table. The Forge session sets labels from the Properties page
(`Session::set_label`, `Session::label_of`;
[forge-editing.md](forge-editing.md)).

**Ground labels.** `forge_doc::GroundLabel` (`ff_drop`, `ff_base`,
`ff_cover`, `siege_site`, `siege_goal`, `siege_line`;
[game-types-without-carriers.md](game-types-without-carriers.md)) are
ordinary labels on netgame flags. `siege_site` falls back to the map's
`territory` flags (`labels::fallback`, `labels::reads`).

A label is a change of the document format: `forge_doc::FORMAT` 1,
`FORMAT_LABELS` 2, `FORMAT_ENTRIES` 3
([forge-map-variants.md](forge-map-variants.md#the-ops)), `FORMAT_GROUPS` 4
([forge-editing.md](forge-editing.md#selection-groups-and-prefabs)). A
document takes the highest number its ops need
(`crates/formats/forge-doc/src/validate.rs`). Label rules, the flag-type
table and format edge cases: see the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Forge labels".

## Roadmap

Each phase lands by itself and ends on its exit criteria.

| # | Phase |
|---|---|
| 1 | `SimExt` side arena |
| 2 | Forge labels (round-trip built; disc-map test not built) |
| 3 | Language, verifier, IR, VM, host runtime, capabilities |
| 4 | Firefight port (`ScriptDirector`) |
| 5 | Slayer port |
| 6 | The other six ports: Juggernaut, Oddball, King, CTF with Assault, Territories |
| 7 | AI squads for any script |
| 8 | Bounce events |
| 9 | Spawn-area control and the loadout picker |
| 10 | Editors: Studio's rules editor, in-game option rows |
| 11 | Retire the lifted engines |
| 12 | Capabilities on Live |
| 13 | Research: `give`, `take` outside a drop |
| 14 | Forge in a match |
| 15 | Research: `hide`, `attach`, body swap, `camera`, damage forwarding |

Phases 7 to 9 are the phased mode's prerequisites and do not block the
ports. Phases 12 to 15 follow phase 3 and block no port. Exit criteria:
see the [detail note](notes/2026-11-26-rules-engine-detail.md), "Roadmap".

## Not verified

Open points, each named in the
[detail note](notes/2026-11-26-rules-engine-detail.md), "Not verified":

- HUD, scoreboard and announcer display per engine (the largest unknown).
- The script index's engine object and its replicated globals.
- Queued calls inside callbacks; round end and the winner.
- A join gate and the allow-list ([rules-start-gate.md](rules-start-gate.md)).
- `set_team`, per-player traits, `give`, `hide`, `attach`, `camera`.
- Edits in a match, spawn areas, the bounce detector, squad entries in play.
- Host migration, the ReachVariantTool importer, VM time with 16 players.

## Prior art

- **Megalo**: Halo: Reach's game type scripts
  ([Introduction to Megalo](https://github.com/XboxChaos/nitrogen/wiki/Introduction-to-Megalo),
  [MegaloEvolved](https://github.com/craftycodie/MegaloEvolved),
  [the Reach editing kit](https://c20.reclaimers.net/hr/hr-ek/)).
- **The ReachVariantTool dialect**
  ([Syntax](https://davidjcobb.github.io/ReachVariantEditor/script/syntax.html),
  [if](https://davidjcobb.github.io/ReachVariantEditor/script/if.html)).
- **Project Reclaimer** brings Megalo to Halo 3
  ([Megalo in Project Reclaimer](https://projectreclaimer.dev/megalo)).

No source derived from a leaked build is used.

## Related

- [game-types-without-carriers.md](game-types-without-carriers.md): the design behind the `type` and `uses` lines
- [firefight.md](firefight.md): the template for a host-side director over engine entries
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): `SimExt`, `ForgeExt`, the logic graph, edit classes and the edit log
- [game-options.md](game-options.md): the variant record, the `hx` block and its Live transport
- [community-gametypes.md](community-gametypes.md): the built-in and community variants
- [sim-rewrite.md](../engine/sim-rewrite.md): extension state, float and RNG rules, `SimRead`
- [film-format.md](film-format.md): what a film checks, objective events
- [live-gamestate.md](../live/live-gamestate.md): engine globals, the variant, kill and score hooks
- [bots.md](bots.md): section 3.7, the per-engine objective state
- [notes/ai-networking.md](notes/ai-networking.md): replicated entity and event types
- [hs_vm.md](../engine/hs_vm.md): the HaloScript VM, a separate runtime
