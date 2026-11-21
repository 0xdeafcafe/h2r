# Rules engine: scripted game types

Status: Design. Nothing is built; this page is the plan. Decisions by the user, 2026-11-21.
Code: none. The plan adds a formats crate named `rules` (language, verifier, IR, VM) and a `rules` module in `crates/extensions/game/src`. Tests: none.
Switch: a `rules` feature of `H2_FEATURES`, added to [switches.md](../runtime/switches.md) when it is built.

One scriptable engine runs every multiplayer game type. A game type is a
script compiled to a small verified IR, plus option values in the variant
record. The host runs the IR in a Rust VM whose state is `SimExt`
extension state, and the VM changes the game only through named engine
entries, so what it does replicates as ordinary game state. The seven
lifted retail engines are the oracles the script ports are proven
against, and are retired once nothing needs them. The rule that must
hold: the same IR, options, seed and inputs give the same engine calls at
the same ticks on every machine and platform.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Addresses are guest addresses of the retail XBE, named as research
points. No game data is in this page.

## Decisions

1. **A scriptable game type engine in the style of Megalo** (Halo: Reach
   onward, "Prior art") for multiplayer, with better syntax, security and
   usability than Megalo.
2. **Two front ends, one IR.** Our own small typed language and Forge's
   logic graph ([forge-features.md](forge-features.md#teleporters-buttons-and-triggers))
   both compile to one rules IR. An importer for the ReachVariantTool
   dialect comes later.
3. **All seven retail engines are ported to scripts**: Slayer, Oddball,
   Juggernaut, King of the Hill, Capture the Flag, Assault, Territories.
   The lifted engines stay only for replaying old films and for System
   Link until both are settled, then they are retired. Every retail
   built-in variant and every community variant becomes "engine script +
   option values".
4. **Firefight, AI-driven modes and superbounce modes are scripts too.**
   Firefight's director is the first port. Scripts command AI by squad;
   they observe bounces and never change physics.
5. **A phased attack-and-defend mode** with the mechanics of Halo:
   Reach's Invasion is the showcase the API is sized against ("Payoff").

## Goals, non-goals, done

Goals: new modes without new Rust ("Payoff"); one engine to maintain and
test headless instead of seven lifted ones behind vtables; scripts from
players that are safe to run on a host we own.

Non-goals:

- Campaign scripting. HaloScript stays the campaign's language
  ([hs_vm.md](../engine/hs_vm.md)). `hs::vm` and this VM share no code,
  only the habit of reaching the game through a trait (`Engine` there,
  Firefight's `Game` here).
- Script execution on a guest, or script-driven rendering.
- Compatibility with Megalo binaries. The later importer reads
  ReachVariantTool source text only.
- A general-purpose language: no I/O, heap, floats or unbounded loops.

Done means every multiplayer game on Live, local or on a dedicated host
runs a rules script, and the lifted engines run only where "What stays
lifted" says, until its retirement criteria hold.

## How it works

```text
source (.rules text) ─┐
Forge logic graph ────┼─> compiler + verifier ─> rules IR blob (SHA-256 id, capped)
RVT dialect (later) ──┘        (formats crate `rules`: Studio, cli, Live upload)
                                          │
   variant record hx block: mode + hash prefix + option bits
   Live session: hx block (existing) + full hash; blob fetched by hash
                                          │
   host only: verify again ─> VM, state in SimExt ─> named engine entries
                                          │
                     ordinary game state, replicated as today
```

### Compiler and verifier

**Design.** The formats crate `rules` holds the parser, type checker,
lowering, verifier, IR codec and VM. It has no I/O and no guest types,
like `crates/formats/firefight`, so Studio, the cli (`h2 rules
check|build|run`), Live's upload route and the host runtime link the
same code. Compiling is deterministic, so the IR's SHA-256 names a
script everywhere.

The verifier is separate from the compiler and reads IR, not source. It
runs at upload, at load on any machine about to host, and in Studio as
you type. It checks the whole blob, dead branches included: every op,
slot index, jump target, label, tag path and API call. A blob that fails
is refused whole.

### Carrying a script

Three things travel: which script, its option values, and the blob.

**Which script and its options: the variant record.** The `hx` block
([game-options.md](game-options.md#where-the-extension-lives),
`variants::ext`) has 13 reserved bytes at `+0x11f..+0x12b`. The plan
uses them all and keeps version 1, because `Ext::decode` does not read
the reserved bytes (**Code**, `crates/formats/variants/src/ext.rs`):

| Offset | Size | Content |
|---|---|---|
| `+0x11f` | 1 | mode: 0 none, 1 Firefight, 2 rules script |
| `+0x120` | 8 | the first 8 bytes of the definition's SHA-256 (the rules IR, or Firefight's definition) |
| `+0x128` | 4 | 32 bits of packed script option values, in the order the script declares them |

The block's CRC covers these bytes. Because the block is in the record,
the reference survives every carrier with no new field: saved variants,
`.h2variant` files (`variants::portable`), cloud saves, Studio
(`crates/apps/studio/src/ui/variants.rs`), the `0x130`-byte variant in a
`0x37c`-byte hopper slot (`crates/live/live-proto/src/hopper`) and the
web hopper builder (`web/src/admin/HopperBuilder.tsx`). A script's own
options are limited to those 32 bits; the compiler refuses more.
Retail's settings (score and time limits, lives, the per-engine type
area) stay where they are and a script reads them by name.
[firefight.md](firefight.md#live) plans a mode id and a hash prefix in
two of the same bytes and `variants::ext` assigns none; this table is
the one layout for both.

**To other machines: the Live session.** The wire codec drops the spare
bytes (`0x7cc50`/`0x7d520`, **E**, game-options.md "How a variant reaches
other machines"), so the block already travels as
`NewSession.variant_ext` (**Code**,
`crates/live/live-proto/src/session.rs`). The session gains one field
beside it, `rules`, holding the full 64-digit hash.

**The blob: by hash.** Built-in scripts are compiled into the binary and
found by hash with no network. Any other blob is stored once on Live
under its SHA-256, in both homes, and fetched by a machine that lacks it
when the variant is chosen in the lobby. Only the host executes it;
guests fetch it for its display part (name, option labels, message
strings). A film stores the blob, so a replay needs no server.

**Limits.** IR at most 64 KiB, source 256 KiB, VM state 16 KiB in
`SimExt`. A whole Megalo variant is about 20 KB ("Prior art"); the
larger cap pays for names and strings.

### Which engine index a script rides

A script game is a game of an existing retail engine index, as Firefight
is a Slayer game ([firefight.md](firefight.md#the-game-engine-underneath)).
A new index is not possible without rewriting four retail paths:

| Path | What it does with an unknown index | Evidence |
|---|---|---|
| Engine table `0x55e4d0[index]` | no engine object; `0x1574c0` writes 0 to `E+0xc14` | **E**, [bots.md](bots.md) section 3.7 |
| Record clean-up `0x19d650` | resets the record to default Slayer unless the index is 1..4 or 7..9 | **E**; **Code**, `variants::record::check` ([variants.md](../editor/variants.md)) |
| Wire codec decode `0x7d520` | decodes the type area per engine through a jump table, then runs the clean-up; a changed record is a failed decode | **E**, game-options.md |
| Lobby protocol map `0x161c90` | synchronous protocol for the two deprecated indexes and anything out of range | **E**, [live-coop.md](../live/live-coop.md) |

**Design.** The script names its carrier:

- **A new mode rides Slayer (index 2).** Slayer has the least behaviour
  of its own, scores through the statborg, and gets the distributed
  protocol.
- **A port rides its own engine's index.** A saved CTF variant stays a
  CTF record, its type-area settings keep their meaning and their place
  in the editor, and the client keeps that engine's replicated globals
  entity (types 0 to 5, **E**, [notes/ai-networking.md](notes/ai-networking.md))
  for whatever its HUD draws from it.

On the host, in a game whose block says mode 2, the `rules` feature
replaces the carrier engine's rule-making callbacks with ones that do
nothing, and the VM makes the rules. Which vtable slots that is per
engine is part of each port. How much each engine's client-side display
depends on its own vtable rather than on replicated state is not traced
("Not verified"); it decides how much display work each port carries.

### The host runtime

**Design.** The runtime follows Firefight's
(`crates/extensions/game/src/firefight/`, [firefight.md](firefight.md#the-host-runtime)):

- an observer after a successful `GAME_ACTIVATE` (`0x18f000`) reads the
  block, finds the blob by hash, verifies it and starts the VM, or keeps
  a refusal with its reason;
- a `before` on `GAME_TICK` makes the engine calls the last step queued;
- an observer after `GAME_TICK` gathers the tick's events and reads, and
  runs the VM step.

All hooks are on game-thread points. The VM runs on the authoritative
machine only (`GAME_ENGINE_AUTHORITATIVE`, `0x15b2f0`) and in a film's
replay, where it re-runs from the recorded inputs. Its variables, timers
and generator state live in a `RulesExt` record in the `SimExt` blob
([sim-rewrite.md](../engine/sim-rewrite.md#2-who-owns-state)), so
checkpoints, snapshots, film keyframes and the sim hash carry them.
`SimExt` is not built; it is the first prerequisite ("Roadmap").

## The language

A source file is a header, options, variables, functions and handlers.

**Types.** `int` (32-bit, saturating), `fixed` (16.16, for seconds and
distances), `bool`, `player`, `team`, `object`, `squad`, `timer` (counts
ticks down, then fires `on timer`), and `enum` types a script declares.
`player`, `team` and `object` values may be `none`. A `table` is constant
rows of typed columns, indexed by `int`. There are no floats and no
strings at run time: text is a string-table index with typed arguments.

**Expressions and control.** Ordinary expressions with precedence and
parentheses (`a = b + c * 2`), `if` / `else if` / `else`, `and` binding
tighter than `or`. These are the things the ReachVariantTool dialect
lacks ("Prior art").

**Variables** are declared, named and scoped:
`global|player|team|object var name: type = init`. The compiler assigns
slots and enforces the budget (64 global, 16 per player, 8 per team, 8
per labelled object, at most 256 labelled objects). A scoped variable is
read through its owner: `p.infected`, `t.captures`.

**Handlers.** `on init`, `on round_start`, `on tick`,
`on kill(victim, killer)`, `on spawn(p)`, `on death(p)`,
`on pickup(p, o)`, `on drop(p, o)`, `on enter(zone, o)`,
`on leave(zone, o)`, `on press(p, o)`, `on timer(t)`,
`on squad_dead(s)`, `on actor_kill(killer, class)`,
`on bounce(p, spot)`, `on join(p)`, `on quit(p)`, `on round_end`. A
handler runs to completion.

**Loops** are `for each` over a finite set only: `players`, `teams`,
`objects labelled "x"`, each with an optional `where`. There is no
`while`, no recursion, and functions are inlined, so every handler has a
static worst-case op count, which the compiler prints.

**Names, not datums.** Engine things are named by tag path
(`"objects/weapons/..."`, checked against the map at load) or by Forge
label. A script never sees a datum index or an address.

**Options** a script declares are the rows a variant editor shows for
it: `option name: type range = default`, with a label and a description
in the string table. `variant.<field>` reads a retail record field by
its `variants::schema` name.

### Slayer

```text
rules "Slayer" 1
carrier slayer

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

`score` is `SCORE_ADD`, which itself adds the team score and tests the
score limit (**E**, `0x15b7c0`, firefight.md "Guest boundary"), so the
script has no limit check.

### Infection: a team switch

```text
rules "Infection" 1
carrier slayer
teams 2

option first_zombies: int 1..4 = 1

player var infected: bool = false

fn infect(p: player) {
    p.infected = true
    set_team(p, team 1)
    set_loadout(p, "objects/weapons/melee/energy_blade/energy_blade", none)
}

on round_start {
    for each players p { p.infected = false; set_team(p, team 0) }
    for each pick(players, first_zombies) p { infect(p) }
}

on kill(victim, killer) {
    if killer != none and killer.infected and not victim.infected {
        infect(victim)
        score(killer, 1)
    }
}

on tick {
    if count(players where not infected) == 0 { end_round(team 1) }
    else if round_time_left() == 0 { end_round(team 0) }
}
```

`pick` draws from the VM's generator. `set_team` has no traced engine
entry ("The engine API"); Infection waits on it.

### Firefight: waves of AI

```text
rules "Holdout" 1
carrier slayer
teams 1

table waves: (class: Class, per_player: int, min: int, max: int, gap: int) = [ ... ]
table points: (class: Class, score: int) = [ ... ]
global var wave: int = 0
global var lives: int = 7
global var next: timer = 150

on timer(next) {
    let w = waves[wave]
    let s = squad(wave % 16)
    place_squad(s, w.class, clamp(count(players) * w.per_player, w.min, w.max), labelled "ff_drop")
    hunt(s)
}
on squad_dead(s) {
    wave = wave + 1
    if wave == len(waves) { end_round(team 0) } else { next = waves[wave - 1].gap }
}
on actor_kill(killer, class) { score(killer, points[class].score) }
on death(p) {
    if lives > 0 { lives = lives - 1; respawn(p) }
    else if count(players where alive) == 0 { end_round(none) }
}
```

### Phases: attack and defend

```text
global var phase: int = 0
global var clock: timer

fn open(n: int) {
    phase = n
    clock = seconds(phase_time)
    for each objects labelled "barrier" b where b.number < n { delete(b) }
    spawn_areas(team 0, "attack", n)
    spawn_areas(team 1, "defend", n)
    if n == 2 { register_objective(create("<core tag path>", first(objects labelled "core")), team 0) }
}
on round_start { open(0) }
on tick {
    if phase < 2 and ticks_held(labelled "zone" phase, team 0) >= seconds(capture_time) { open(phase + 1) }
}
on enter(zone, o) {
    if phase == 2 and zone.label == "goal" and is_objective(o) { end_round(team 0) }
}
on timer(clock) { end_round(team 1) }
```

### A Forge graph is the same IR

The Forge front end turns the document's nodes and links into the same
slots and handlers. A trigger volume linked to a counter linked to a
message:

```text
volume#4.entered ──> counter#7.start (limit 3) ──> message#9.fire

    global var n7: int = 0
    on enter(zone, o) {
        if zone.id == 4 { n7 = n7 + 1; if n7 == 3 { message(all, text 9); n7 = 0 } }
    }
```

A node's state is a global slot named by node id; a link is a call, in
link-id order. The graph's budget rule (node visits a tick, the rest
next tick, forge-features.md) lowers to a queue in slots, so the Forge
step is this VM. Links that retail data expresses whole (a teleporter
pair) stay retail data. A map's graph and the game type's script are two
blobs in one VM; the game type's handlers run first.

## The IR and the VM

**Shape.** The blob is: a header (magic, IR version, API version,
carrier, team requirement); a string table; an option table (type,
range, default, label); a slot table (scope, type, initial value); a
label table and a tag-path table; constant tables; a handler table
(event, argument slots); and code. Code is a register bytecode with typed ops:
arithmetic, compare, slot load and store, forward conditional jump,
`ForEach` over a set with a body range, and `Call` of a numbered API
entry. There is no backward jump and no indirect jump, so termination is
a property of the format, and the verifier needs no fixpoint.

**Budget.** A VM step may execute 20,000 ops. The verifier computes each
handler's worst case from the set bounds (16 players, 8 teams, 256
labelled objects); a script whose worst tick fits is marked proven and
the run-time check cannot fire. For any other script the count is
enforced: going over is a **script error**. A script error stops the VM,
ends the round through `GAME_END` with the outcome "script error", and
is named in the log and the post-game report. It happens at the same op
on every run, so a film replays it.

**Order.** One step runs, in this order: `init` or `round_start` when
due; the tick's events in the engine's event order; expired timers in
slot order; `on tick`. `for each` visits players by slot index, teams by
index, objects by label then placement id. Engine calls are queued in
program order and made before the next tick, as Firefight's are. Reads
during a step see the state at the tick boundary, not the queue.

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
(**Code**, `live_proto` capabilities in `crates/live/live-proto/src/lib.rs`).
An older build is **refused** from a script game, where a build without
`variant-ext/1` is only warned about. The difference: variant extras
change a few scalars and the game stays the same game; a script changes
who scores, who is on which team and when the round ends, and an older
guest would show and predict the carrier engine's rules for the whole
match. The IR version changes when ops change; the API version when an
entry's meaning changes. A blob names both and a VM refuses what it does
not know.

**Films.** The blob, the options and the seed are replay inputs, and the
`rules` feature's hooks are part of `simulation_hooks`
([film-format.md](film-format.md#determinism-and-verification)). The VM
re-runs in playback. Script-made objective changes need their own
observer for film events, since the existing one reads retail engine
state (film-format.md "Objective and power-up events").

## The engine API

The API is a numbered list of entries; a script can call nothing else.
Status: **named** is a point or layout in `crates/hooking/guest-layout`
or `crates/rewrites/engine`; **traced** is an address in a doc with no
named point yet; **untraced** has no known entry.

Reads:

| Read | From | Status |
|---|---|---|
| player and team score, kills, deaths | statborg, `E+0x304` (`game::STATBORG`, `PLAYER_STATS`, `TEAM_STATS`) | named |
| players: present, alive, team, unit, lives | player datum (`guest-layout` `player.rs`; team `+0xc0`, lives `+0x1ac`) | named |
| team play, score and time limits, type area | variant record `V` (`variants::record`, [live-gamestate.md](../live/live-gamestate.md)) | named |
| round start tick | `E+0x70` (`ROUND_START_TICK`, likely) | named |
| objects: tag, position, parent | object datum (`guest-layout` `object.rs`) | named |
| objects by label | Forge labels | not built |
| objective records (object, team, carrier) | `E+0xc18`, `E+0xc1c` (`engine::objectives`) | named |
| netgame flags by type and team | `scnr+0x118` (`engine::objectives`); query `0x19ec40` | named; traced |
| per-engine globals (`E+0xfc`): hill, bomb state, territories, Juggernaut mask | `engine::objectives`; `KOTH_GLOBALS` and its siblings in `global.rs` | named |
| events of the tick | hooks on `PLAYER_KILLED` `0x15ce70`, `PLAYER_SPAWN` `0x14f2e0`; `SimRead::events` once sim-rewrite section 7 exists | named; not built |
| squads: actors alive, `on squad_dead`, `on actor_kill` | `actor_net::squads::count_by_squad` and the kill list Firefight's `Observed` carries (`crates/extensions/game/src/actor_net/squads.rs`, `crates/formats/firefight/src/director.rs`) | named |
| `on bounce(p, spot)` | a detector of the traced signature: capsule below the floor, then +8 to +13 units/s upward the next tick ([superbounce.md](superbounce.md#summary), **Trace**); the launch rule itself is Open there | traced; detector not built |
| bounce and crouch spots, as zones | the scanner's spots per map and BSP (`crates/live/superbounce`); candidates, none traced as a real bounce | named |

Actions:

| Action | Through | Status |
|---|---|---|
| `score(p, n)` | `SCORE_ADD` `0x15b7c0` | named |
| `end_round(winner)` | `GAME_END` `0x1389c0` ends the game; advancing a round and naming a winner are not traced (winner storage is not recovered, live-gamestate.md) | named; untraced |
| lives | player `+0x1ac` and `PLAYER_ENTITY_DIRTY` `0xa7840`; gates `PLAYER_OUT_OF_LIVES` `0x15db30`, `PLAYER_RESPAWN_READY` `0x15dd40` | named |
| respawn a player | `PLAYER_SPAWN` `0x14f2e0` | named |
| traits for the match: gravity, damage, shield, health | `variant_ext` hooks (`crates/extensions/game/src/variant_ext.rs`) | named |
| traits per player; movement speed | the engine's trait query `0x24dda0` (vtable `+0x114`) has no direct call site; speed has no reader found (game-options.md "Not built") | untraced |
| `set_team(p, t)` | no entry known; the field is player `+0xc0` | untraced |
| `set_loadout` | starting-weapon routine `0x19eda0` | traced |
| create and delete objects | `OBJECT_DELETE` `0xb8540`; creation as Forge's ([forge.md](forge.md)) | named; traced |
| objective register, unregister, picked up, dropped | `0x15e050`, `0x15e130`, `0x15e250`, `0x15e460` | traced |
| barriers and other map changes | create and delete at labels, as above; a barrier is a labelled placement (forge-features.md "Invisible barriers") | not built |
| squad: place from the palette at a label, team, orders, erase | `AI_SQUAD_PLACE` `0x201520`, `AI_SET_TEAM` `0x275b60`, `AI_SET_ORDERS` `0x275cb0`, `AI_ERASE` `0x2736c0`, through `actor_net::squads::SquadEntry` (firefight.md "Guest boundary") | named; game runs R1, R2 pending |
| `spawn_areas(team, label, n)`: which starting locations are live | answer a hook on `PLAYER_SPAWN_LOCATION` `0x14ef00` (picks from `scnr+0x100`) with labelled locations only; how the routine weighs locations is not traced | named point; untraced rule |
| loadout chosen at spawn | `set_loadout`, plus a picker screen of ours | traced; picker not built |
| player body per team (Spartan, Elite) | `PLAYER_SPAWN` chooses the unit type; what it reads is not traced | untraced |
| teleport, device power | Forge's sinks (forge-features.md) | not built |
| HUD message, scoreboard text, announcer, waypoint | not traced; the replicated game engine event (type 7) is known only as a type | untraced |

The engine update `0x15c150` and the post-game step `0x15ca10` are
traced (live-gamestate.md) and are where the lifted engines do their
per-tick work; they are not API entries.

Two layouts of the engine vtables are on record. bots.md section 3.7
gives the vtables as `0x45c6d8` (Slayer) and `0x453d38` (Juggernaut);
game-options.md gives `0x45c650` and `0x453cb0` and puts the trait query
at `+0x114` of those. The two differ by `0x88`. firefight.md's kill
callback at `+0x78` uses the bots.md base. Each port reads its engine's
vtable again before relying on a slot number.

## Security and usability

- **Host only.** A guest never executes a script and reaches the VM only
  through its ordinary inputs.
- **Verified twice**, at upload and at load, by the same code. Live
  stores only blobs that verify.
- **Capability-limited.** The API table is the whole world: no file,
  network, clock, address, datum or other process. A tag the map lacks
  or a label it does not define is a load error naming it.
- **Bounded.** IR, state, ops per tick, labelled objects, objects made
  per tick and objects alive all have caps; the engine's object and tag
  limits are checked before a create.
- **Text.** Labels are at most 32 bytes and messages 128, sanitised by
  the rules for player names (game-options.md section 4) and formatted
  only with typed arguments.
- **Input rate.** `on press` fires at most 10 times a second per player;
  the rest are dropped the same way on every run.
- **Clean failure.** A script error ends the round and never leaves the
  engine mid-call.

Tooling:

- **Studio**: a rules editor with errors as you type, the option table
  as a form, and a budget meter (worst-case ops, slots, IR bytes) that
  names the costliest handler. The blob carries an optional line table
  for messages; the VM ignores it.
- **Scenario tests**: `h2 rules run` steps a script over a recorded
  event list with no game; `h2 sim` runs it in a headless match. Each
  built-in script has scenario tests beside its source.
- **Agent API**: `rules.state` (script, hash, refusal, budget use),
  `rules.vars`, `rules.trace` (handlers run and calls made per tick),
  `rules.step` and `rules.load` (swap a verified blob between rounds).
  They join [agent-api.md](../runtime/agent-api.md) when built.

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

**Parity method.** The lifted engine is the oracle. A headless match is
recorded once (seed and per-seat inputs), then run twice from the same
start: on the lifted engine, and on the script with the lifted engine's
rule callbacks silenced. The two runs must agree on the statborg rows,
the objective event stream the theater observer emits
(`crates/extensions/game/src/theater/objectives.rs`), the end tick and
the result, with the script's timeline one tick behind: the lifted
engine acts inside the tick, the VM's calls are made at the start of the
next. A port is proven per built-in variant of its engine, on at least
three maps, with phantom seats filling the roster. The arena hash is not
compared.

**Variants.** The 39 retail built-ins and the 14 community variants
(thirteen classics and Superbounce, `community_variants::VARIANTS`)
keep their records. Each gains mode 2 and its engine script's hash
prefix in the block; its settings are already option values in the
record. Rules that were honour rules ([community-gametypes.md](community-gametypes.md),
Zombies' team switch) become script variants of their own.

**What stays lifted, and why.**

- **Old films.** A film replays only with the hook set and variant
  bytes it was recorded with (`simulation_hooks`, film-format.md). A
  film recorded on a lifted engine replays on the lifted engine.
- **System Link.** The block is off there, because no Live session
  carries it (game-options.md "How it reaches every player"), so a
  System Link game cannot name a script and plays the lifted engine.

**Retirement criteria**, all of them: every engine's port has passed
parity; script games have been played on Live in every engine; System
Link either carries the block or is decided to stay retail-only on a
frozen build; films recorded on lifted engines are either converted,
expired by policy, or served by a frozen player build. Then the lifted
engines' hooks, observers and bot readers that parse per-engine globals
(`crates/rewrites/engine/src/objectives.rs`,
`crates/live/bots/src/objective.rs`) move to reading rules state.

## Firefight, AI and bounces

**Firefight is the first port.** `firefight::Director`
(`crates/formats/firefight/src/director.rs`) and its scoring
(`src/scoring.rs`) become one rules script: waves, rounds, lives, skulls
and scoring. **Design:** the wave-set format stays the authoring format.
`firefight::Def` is already validated, bounded and hashed, and the hopper
tool offers definitions; a small front end compiles a `Def` to IR as the
director script with the definition's rounds, caps and scores as constant
tables. So a wave set is one blob with one hash, mode 1 folds into mode
2, and the built-in sets need no rewrite by hand. The Rust director and
its tests are the oracle: the script must give the same `Command` log for
the same `Observed` sequence.

**AI is commanded by squad.** A script places a squad from the kit's
palette at a label, sets its team and orders, reads how many of its
actors live, and hears `on squad_dead` and `on actor_kill`. These are the
host actor entries Firefight uses ([ai-networking.md](ai-networking.md)).
A script never runs per actor: actor brains stay the engine's, for the op
budget and for determinism. Phantom bots ([bots.md](bots.md),
`crates/live/bots`) are players like any other to a script; it does not
drive their brains.

**Bounces are observed, not made.** Physics stays the engine's: Havok at
an exact 30 Hz tick ([high-framerate.md](high-framerate.md)). A script
hears `on bounce(p, spot)`, reads the scanner's spots as zones, scores
and times them, and sets traits. That makes Bounce Practice and a trick
or parkour mode real scripts, and the superbounce flag (bit 2 of the
block's flags) a script's own choice. The detector's rule is a traced
signature, not the decoded launch rule, which superbounce.md lists as
Open; a detector can miss or over-report until it is checked in play.

## Payoff

- **Race.** Halo 1's race track points are dropped at conversion because
  Halo 2 has no race ([multiplayer.md](../halo1/multiplayer.md)); a Race
  script with labelled checkpoints gives them a use.
- **Infection** with a real team switch, **Gun Game**, and other modes
  that were honour rules.
- **Halo 1's per-spawn game types**, dropped for want of a field
  (multiplayer.md): starting locations carry labels and a script filters
  on them.
- **A phased attack-and-defend mode**, with the mechanics of Halo:
  Reach's Invasion and none of its assets or text. "Invasion" is Reach's
  name; ours is open (working name: Siege).

The phased mode is the check that the API is wide enough:

| Mechanic | In a script | API status |
|---|---|---|
| up to three phases, a timer each | a global `int` and a `timer` | the VM alone |
| capture zones, then a core carried to a goal | zone tests as Territories; the objective registry as CTF and Assault | traced |
| the map opens per phase | delete and create barriers at labels | not built (Forge labels, barriers) |
| spawn areas switch per phase | `spawn_areas` | named point; rule untraced |
| loadouts per team, chosen at spawn | `set_loadout` and traits; Halo 2 has no armor abilities, so a loadout is weapons, grenades and traits | traced; picker screen not built |
| team respawn waves | hold respawns with the lives gates, release on a timer | named |
| two species teams | Halo 2's multiplayer has Spartan and Elite bodies (live-gamestate.md); forcing one per team | untraced |

## Forge labels

A prerequisite. `crates/formats/forge-doc` has teleporter and device
links (`links.rs`) and no way to tell a script which placement is which.

**Design.** A label is a string of at most 32 bytes plus an optional
number (`"checkpoint" 3`, `"hill" 0`). It is a field of a placement in
the Forge document, set through the same edit log as any other field, so
it has no scenario bytes and costs nothing in a session that runs no
script. At map load the labels become a table in `RulesExt`: label,
number, placement id, and the object's datum once it exists. Scripts
read it through `objects labelled`. Retail maps with no Forge document
get labels from a built-in table per netgame flag type, so
`objects labelled "ctf_flag_spawn"` works on every map.

## Roadmap

Each phase lands by itself and ends on its exit criteria.

| # | Phase | Exit criteria |
|---|---|---|
| 1 | `SimExt` side arena (forge-design.md slice F7; sim-rewrite.md section 2) | a test record survives checkpoint, revert, snapshot and a film keyframe, and changes the sim hash |
| 2 | Forge labels | labels round-trip through the document and the edit log; the built-in flag-type table resolves on a disc map (`#[ignore]` test) |
| 3 | Language, verifier, IR, VM, host runtime | the compiler and verifier pass their suites; a fuzzer finds no panic in the verifier or VM; a script scores a fake game through the `Game`-style trait |
| 4 | Firefight port | every built-in definition, compiled to IR, gives the Rust director's `Command` log in the director's own tests; `firefight.state` reads the same from both |
| 5 | Slayer port | parity on every retail Slayer built-in on three maps; a script Slayer game played on Live |
| 6 | The other six ports, in the order Juggernaut, Oddball, King, CTF with Assault, Territories | parity per engine; the API rows each needs are named points |
| 7 | AI squads for any script | a script outside Firefight places, orders and erases squads on a kit map; `on squad_dead` fires on the tick the count reaches 0 in a test over the fake squad entry |
| 8 | Bounce events | the detector agrees with stepped traces of known bounces and reports none on a recorded match without one; Bounce Practice runs as a script |
| 9 | Spawn-area control and the loadout picker | the `PLAYER_SPAWN_LOCATION` rule is traced; a labelled subset is the only one chosen in a headless match; the picker sets a loadout before a spawn |
| 10 | Editors: Studio's rules editor, and in-game option rows from the script's option table | a variant made in Studio with a custom script plays from a hopper; the in-game editor changes a script option |
| 11 | Retire the lifted engines | the retirement criteria above |

Phases 7 to 9 are the phased mode's prerequisites and do not block the
ports. The Forge graph front end follows phase 3 on Forge's own schedule;
the ReachVariantTool importer follows phase 10.

## Not verified

- **HUD, scoreboard and announcer.** How each engine shows objective
  state, score labels, messages and announcer lines on a client is not
  traced: whether from the replicated engine-globals entity, the game
  engine event, or the client's own engine vtable. This is the largest
  unknown; it decides the display half of every port and of every new
  mode.
- **The vtable layout.** The `0x88` difference between bots.md and
  game-options.md is not reconciled.
- **Silencing a carrier engine.** Which vtable slots make rules, per
  engine, and whether replacing them on the host leaves the replicated
  globals entity consistent.
- **The one-tick lag.** The lifted engine scores inside `PLAYER_KILLED`;
  the VM's score lands at the start of the next tick. Whether a retail
  outcome depends on it (two kills on the limit in one tick, a kill on
  the last tick) needs the parity runs.
- **Round end and the winner.** Only the game-end latch is known.
- **`set_team`, per-player traits, movement speed, the body a player
  spawns in.** No entry is known.
- **Spawn areas.** How `PLAYER_SPAWN_LOCATION` weighs starting locations,
  and whether answering it with a subset is enough.
- **The bounce detector.** Its rule is a signature from traces of one
  patch on one map; the launch rule is not decoded (superbounce.md
  "Open").
- **Squad entries in play.** Firefight's runs R1 to R4 (firefight.md
  "What a run settles") settle them for scripts too.
- **Host migration.** Whether a session of ours moves hosts, and how
  `RulesExt` would reach the new one. There is no handler for it.
- **System Link.** Whether it can carry the block and a hash; built-in
  scripts would need no blob transfer.
- **The ReachVariantTool importer.** What of Megalo's slots, object
  types and trigger kinds cannot map onto this API is not surveyed.
- **The phased mode's name**, and VM time per tick with 16 players.

## Prior art

- **Megalo.** Every Halo: Reach game type is a Megalo script, as are
  Halo 4's and Halo 2: Anniversary's multiplayer. A script is triggers
  of conditions and actions, run each tick or on events (init, pregame,
  host migration, object death). Variables are fixed slots of five types
  (number, object, player, team, timer) in four scopes (global, player,
  object, team), each with a network priority; nesting is two levels; a
  variant has about 20 KB.
  [Introduction to Megalo](https://github.com/XboxChaos/nitrogen/wiki/Introduction-to-Megalo),
  [MegaloEvolved](https://github.com/craftycodie/MegaloEvolved),
  [the Reach editing kit](https://c20.reclaimers.net/hr/hr-ek/) (it
  includes MegaloEdit).
- **The ReachVariantTool dialect** has no expressions (`a = b + c` is
  not valid), no parentheses, no `else` (`alt`, `altif`), `or` binding
  before `and`, and no user-declared variables.
  [Syntax](https://davidjcobb.github.io/ReachVariantEditor/script/syntax.html),
  [if](https://davidjcobb.github.io/ReachVariantEditor/script/if.html).
- **Project Reclaimer** brings Megalo to Halo 3: host-only execution,
  presses limited to 10 a second, the whole file validated before a
  round, labels up to 32 bytes, and the built-in modes kept with script
  layered on top. [Megalo in Project Reclaimer](https://projectreclaimer.dev/megalo).
  This design takes the first four and replaces the built-in engines
  instead.

No source derived from a leaked build is used.

## Related

- [firefight.md](firefight.md): the template for a host-side director over engine entries
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): `SimExt`, `ForgeExt`, the logic graph
- [game-options.md](game-options.md): the variant record, the `hx` block and its Live transport
- [community-gametypes.md](community-gametypes.md): the built-in and community variants
- [sim-rewrite.md](../engine/sim-rewrite.md): extension state, float and RNG rules, `SimRead`
- [film-format.md](film-format.md): what a film checks, objective events
- [live-gamestate.md](../live/live-gamestate.md): engine globals, the variant, kill and score hooks
- [bots.md](bots.md): section 3.7, the per-engine objective state
- [notes/ai-networking.md](notes/ai-networking.md): replicated entity and event types
- [hs_vm.md](../engine/hs_vm.md): the HaloScript VM, a separate runtime
