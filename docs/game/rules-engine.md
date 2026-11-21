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
against, and are retired once every port has passed. The rule that must
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
   The lifted engines are retired once the ports pass: System Link runs
   the built-in scripts, and films recorded on a lifted engine no longer
   replay. Every retail
   built-in variant and every community variant becomes "engine script +
   option values".
4. **Firefight, AI-driven modes and superbounce modes are scripts too.**
   Firefight's director is the first port. Scripts command AI by squad;
   they observe bounces and never change physics.
5. **A phased attack-and-defend mode** with the mechanics of Halo:
   Reach's Invasion is the showcase the API is sized against ("Payoff").
6. **A script declares its capabilities and can grant players powers by
   role**: a VIP who moves, turns, deletes and places objects mid-match.
   Script-granted edits run in any custom or hopper match and never in a
   ranked one, inside a budget, each one logged
   ("Roles, powers and capabilities").

## Goals, non-goals, done

Goals: new modes without new Rust ("Payoff"); one engine to maintain and
test headless instead of seven lifted ones behind vtables; scripts from
players that are safe to run on any machine: players' consoles host Live
games and film viewers re-run the VM, so safety rests on the VM
("Security and usability"), never on who owns the host.

Non-goals:

- Campaign scripting. HaloScript stays the campaign's language
  ([hs_vm.md](../engine/hs_vm.md)). `hs::vm` and this VM share no code,
  only the habit of reaching the game through a trait (`Engine` there,
  Firefight's `Game` here).
- Script execution on a guest, or script-driven rendering.
- Compatibility with Megalo binaries. The later importer reads
  ReachVariantTool source text only.
- A general-purpose language: no I/O, heap, floats or unbounded loops.

Done means every multiplayer game, on Live, System Link, local or on a
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

**Design.** The formats crate `rules` holds the parser, type checker,
lowering, verifier, IR codec and VM. It has no I/O and no guest types,
like `crates/formats/firefight`, so Studio, the cli (`h2 rules
check|build|run`), Live's upload route and the host runtime link the
same code. Compiling is deterministic, so the IR's SHA-256 names a
script everywhere.

The verifier is separate from the compiler and reads IR, not source. It
runs at upload, at load on any machine about to host, and in Studio as
you type. It checks the whole blob, dead branches included: every op,
slot index, jump target, label, tag path and API call, and that each
call's capability is one the header declares
("Roles, powers and capabilities"). A blob that fails is refused whole.

### Carrying a script

Three things travel: which script, its option values, and the blob.

**Which script and its options: the variant record.** The `hx` block
([game-options.md](game-options.md#where-the-extension-lives),
`variants::ext`) has 13 reserved bytes at `+0x11f..+0x12b`. The plan
uses them all:

| Offset | Size | Content |
|---|---|---|
| `+0x11f` | 1 | mode: 0 none, 1 Firefight, 2 rules script |
| `+0x120` | 8 | the first 8 bytes of the definition's SHA-256 (the rules IR, or Firefight's definition) |
| `+0x128` | 4 | 32 bits of packed script option values, in the order the script declares them |

The block's CRC covers these bytes, and the table is the one layout for
Firefight ([firefight.md](firefight.md#live)) and scripts. A carrier
that copies the record keeps the reference with no new field: saved
variants, `.h2variant` files (`variants::portable`), cloud saves, the
`0x130`-byte variant in a `0x37c`-byte hopper slot
(`crates/live/live-proto/src/hopper`) and the web hopper builder
(`web/src/admin/HopperBuilder.tsx`).

An editor does not copy: it decodes the block to `variants::ext::Ext`
and encodes it again, and `Ext::encode` writes the reserved bytes as
zero and an all-zero block for default extras (**Code**,
`crates/formats/variants/src/ext.rs`; the in-game page in
`crates/extensions/game/src/variant_page.rs` and `VariantEditor::set_ext`
in `crates/formats/variants/src/panel.rs` both write through it). So
`Ext` carries the reference: a `script` field (mode, hash prefix, option
bits) that `decode` reads and `encode` writes, with `is_default` false
while the mode is not 0. A gravity edit keeps the script, and a script
variant with no other extras still has a block. The version stays 1: a
version 2 would make an older build ignore the extras too
(`Why::Version`) and protect nothing, since an older editor encodes
version 1 from its own struct.

**An older build that edits a script variant drops the reference**, and
no newer build can stop it: its `Ext` has no `script` field, so its
`encode` zeroes the bytes. What is left is a valid record of the carrier
engine with the edited extras. It plays as the carrier's game ("A record
with no mode"), its script name and option rows are gone, and that build
shows no warning. The copy cannot be repaired; the variant is made again
from the script, which stays on Live under its hash.

A script's own options are limited to the 32 bits; the compiler refuses
more. Retail's settings (score and time limits, lives, the per-engine
type area) stay where they are and a script reads them by name.

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

On the host, in a script game, the `rules` feature answers each of the
carrier engine's rule-making callbacks itself: a `before` hook on the
callback's entry runs the script's handler for that event in place of
the lifted body ("The host runtime"). No guest memory is patched. Which
callbacks those are per engine is part of each port; the one on record
is Slayer's kill callback, vtable slot `+0x78`, which `PLAYER_KILLED`
(`0x15ce70`) calls last (**E**, firefight.md "Guest boundary"). How much
each engine's client-side display depends on its own vtable rather than
on replicated state is not traced ("Not verified"); it decides how much
display work each port carries.

**A record with no mode.** On a host with the `rules` feature on, a
record whose block has mode 0 (all zero, or extras only), on an engine
whose port has passed parity, runs that engine's built-in script, chosen
by engine index from a table compiled in. Saved variants, hopper slots
and copies an older build edited are script games with no byte changed.
The host puts the built-in's hash in the session's `rules` field as for
any script, and a film records which ran: its `simulation_hooks` SHA
differs with the feature and it stores the blob. The rule covers Live,
local, dedicated-host and System Link games: the built-in scripts are
compiled in, so a System Link game needs no blob and no session field.
A System Link game cannot name any other script.

### The host runtime

**Design.** The runtime follows Firefight's
(`crates/extensions/game/src/firefight/`, [firefight.md](firefight.md#the-host-runtime)):

- an observer after a successful `GAME_ACTIVATE` (`0x18f000`) reads the
  block, finds the blob by hash, verifies it and starts the VM, or keeps
  a refusal with its reason;
- a `before` on each rule callback of the carrier engine, and on its
  per-tick update, answers with an async body (as Firefight's
  `GAME_TICK` hook does) that runs the handler and makes its engine
  calls in place, through `engine::entry::Cx`;
- an observer after `GAME_TICK` gathers the events no engine callback
  reports (squads, actor kills, bounces) and runs their handlers;
- a `before` on `GAME_TICK` makes the queued calls ("Order").

All hooks are on game-thread points. The VM runs on the authoritative
machine only (`GAME_ENGINE_AUTHORITATIVE`, `0x15b2f0`) and in a film's
replay, where it re-runs from the recorded inputs. Its variables, timers
and generator state live in a `RulesExt` record in the `SimExt` blob
([sim-rewrite.md](../engine/sim-rewrite.md#2-who-owns-state)), so
checkpoints, snapshots, film keyframes and the sim hash carry them.
`SimExt` is not built; it is the first prerequisite ("Roadmap").

## The language

A source file is a header, options, variables, functions and handlers.
The header names the script, its version, its carrier, its teams and the
capabilities it requires: `requires player.weapons, hud.message`.

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
`on bounce(p, spot)`, `on edit(p, o)` (a granted power was used),
`on join(p)`, `on quit(p)`, `on round_end`. A handler runs to
completion.

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
requires player.team, player.weapons

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
requires ai.squads

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
option phase_time: int 60..600 = 180
option capture_time: int 5..60 = 20

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
next tick, forge-features.md) lowers to a queue in slots. Links that
retail data expresses whole (a teleporter pair) stay retail data. A
map's graph and the game type's script are two blobs in one VM; the game
type's handlers run first.

## The IR and the VM

**Shape.** The blob is: a header (magic, IR version, API version,
carrier, team requirement, and the required capabilities by name, at
most 32, "Roles, powers and capabilities"); a string table; an option table (type,
range, default, label); a slot table (scope, type, initial value); a
label table and a tag-path table; constant tables; a handler table
(event, argument slots); and code. Code is a register bytecode with typed ops:
arithmetic, compare, slot load and store, forward conditional jump,
`ForEach` over a set with a body range, and `Call` of a numbered API
entry. There is no backward jump and no indirect jump, so termination is
a property of the format, and the verifier needs no fixpoint.

**Budget.** A tick may execute 20,000 ops across all its handlers. The
count is always enforced and nothing is marked proven: a handler's worst
case follows from the set bounds (16 players, 8 teams, 256 labelled
objects), but the engine decides how many kills or zone entries a tick
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
| `on squad_dead`, `on actor_kill`, `on bounce`; `on init` | after `GAME_TICK`, from what the tick's observers gathered; `init` after `GAME_ACTIVATE` | queued |

An event the carrier has no callback for runs with the per-tick group.
Some calls are queued from any handler and made in program order by the
`before` on the next `GAME_TICK`: squad commands, which are host log
entries that replicas and films apply at the tick boundary (firefight.md
"The director"); and object creation, deletion and teleports, not known
to be legal inside a damage or AI update (`AI_ERASE` only marks an actor
the AI is updating, `0x2736fc`). A read sees every call made so far and
no queued one. `for each` visits players by slot index, teams by index,
objects by label then placement id.

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
| objects by label; `zone.label`, `zone.id`, `o.number` | Forge labels | not built |
| `is_objective(o)`; objective records (object, team, carrier) | `E+0xc18`, `E+0xc1c` (`engine::objectives`) | named |
| zone tests: who is in a labelled zone, behind `on enter`, `on leave` and `ticks_held` | flag polygons and labelled volumes, in Rust under the float rules | not built |
| netgame flags by type and team | `scnr+0x118` (`engine::objectives`); query `0x19ec40` | named; traced |
| per-engine globals (`E+0xfc`): hill, bomb state, territories, Juggernaut mask | `engine::objectives`; `KOTH_GLOBALS` and its siblings in `global.rs` | named |
| events of the tick | hooks on `PLAYER_KILLED` `0x15ce70`, `PLAYER_SPAWN` `0x14f2e0`; `SimRead::events` once sim-rewrite section 7 exists | named; not built |
| squads: actors alive, `on squad_dead`, `on actor_kill` | `actor_net::squads::count_by_squad` and the kill list Firefight's `Observed` carries (`crates/extensions/game/src/actor_net/squads.rs`, `crates/formats/firefight/src/director.rs`) | named |
| `on bounce(p, spot)` | a detector of the traced signature: capsule below the floor, then +8 to +13 units/s upward the next tick ([superbounce.md](superbounce.md#summary), **Trace**); the launch rule itself is Open there | traced; detector not built |
| bounce and crouch spots, as zones | the scanner's spots per map and BSP (`crates/live/superbounce`); candidates, none traced as a real bounce | named |

Actions:

| Action | Through | Status | Cap |
|---|---|---|---|
| `score(p, n)` | `SCORE_ADD` `0x15b7c0` | named | core |
| `end_round(winner)` | `GAME_END` `0x1389c0` ends the game; advancing a round and naming a winner are not traced (winner storage is not recovered, live-gamestate.md) | named; untraced | core |
| lives | player `+0x1ac` and `PLAYER_ENTITY_DIRTY` `0xa7840`; gates `PLAYER_OUT_OF_LIVES` `0x15db30`, `PLAYER_RESPAWN_READY` `0x15dd40` | named | core |
| `respawn(p)` | `PLAYER_SPAWN` `0x14f2e0` | named | core |
| traits for the match: gravity, damage, shield, health | `variant_ext` hooks (`crates/extensions/game/src/variant_ext.rs`) | named | `game.traits` |
| traits per player; movement speed | the engine's trait query `0x24dda0` (vtable `+0x114`) has no direct call site; speed has no reader found (game-options.md "Not built") | untraced | `player.traits` |
| `set_team(p, t)` | no entry known; the field is player `+0xc0` | untraced | `player.team` |
| `set_loadout` | starting-weapon routine `0x19eda0` | traced | `player.weapons` |
| `create(tag, at)`, `delete(o)` | `OBJECT_DELETE` `0xb8540`; creation as Forge's ([forge.md](forge.md)) | named; traced | `objects.create`, `objects.delete` |
| `register_objective(o, team)`, unregister, picked up, dropped | `0x15e050`, `0x15e130`, `0x15e250`, `0x15e460` | traced | core |
| barriers and other map changes | create and delete at labels, as above; a barrier is a labelled placement (forge-features.md "Invisible barriers") | not built | `objects.create`, `objects.delete` |
| `place_squad(s, class, n, label)` from the palette with its team, `hunt(s)`, erase | `AI_SQUAD_PLACE` `0x201520`, `AI_SET_TEAM` `0x275b60`, `AI_SET_ORDERS` `0x275cb0`, `AI_ERASE` `0x2736c0`, through `actor_net::squads::SquadEntry` (firefight.md "Guest boundary") | named; game runs R1, R2 pending | `ai.squads` |
| `spawn_areas(team, label, n)`: which starting locations are live | answer a hook on `PLAYER_SPAWN_LOCATION` `0x14ef00` (picks from `scnr+0x100`) with labelled locations only; how the routine weighs locations is not traced | named point; untraced rule | `player.spawn` |
| loadout chosen at spawn | `set_loadout`, plus a picker screen of ours | traced; picker not built | `player.weapons` |
| `give(p, tag)`: a weapon into a living player's hands | no entry proven. The pickup's attach routine `0x10ca80` (item, new unit) is on record as an observer only (**E**, [bots.md](bots.md) "Weapon taken") | untraced | `player.weapons` |
| `take(p, weapon)` | the inventory removal `0xce920`, reached from the drop path `0xe8de0` (**E**, bots.md, the dual-wield drop); a call outside a drop is not proven | traced | `player.weapons` |
| player body per team (Spartan, Elite) | `PLAYER_SPAWN` chooses the unit type; what it reads is not traced | untraced | `player.disguise` |
| `hide(p)`, `show(p)`: a player's biped not drawn, not hit | no entry known. The native renderer can skip the draw; hitboxes and collision are the engine's | untraced | `player.disguise` |
| `attach(o, to)`: an object follows another | no entry known; the object datum has a parent field (`guest-layout` `object.rs`) | untraced | `objects.move` |
| `camera(p, mode)`: first or third person | no entry known | untraced | `player.camera` |
| teleport, device power | Forge's sinks (forge-features.md) | not built | `objects.move` |
| `message(to, text)`, scoreboard text, announcer, waypoint | not traced; the replicated game engine event (type 7) is known only as a type | untraced | `hud.message` |
| `forge_move(o, at)`, `forge_delete(o)`, `forge_place(tag, at)`, `forge_scale(o, s)`: an edit through the Forge log | a `forge-doc` op, validated and host-stamped (`crates/formats/forge-doc`, `LogHead::stamp`); edit classes in "Roles, powers and capabilities" | named | `forge.move`, `forge.delete`, `forge.place`, `forge.scale` |

VM built-ins reach no engine entry (status **VM-only**):

| Built-in | What it is |
|---|---|
| `count(set)`, `first(set)`, `len(table)`, `clamp(v, lo, hi)` | over a finite set or a constant table; `first` follows the `for each` order and gives `none` for an empty set |
| `pick(set, n)` | `n` members drawn with the VM's generator |
| `seconds(n)` | `n` times the tick rate, in ticks |
| `ticks_held(zone, team)` | a counter the VM keeps per labelled zone and team over the zone test: ticks the team has held it alone |
| `squad(n)`, `team n`, `text n` | constants: a kit squad slot, a team index, a string-table index |
| `grant(p, cap, budget n, palette "x")`, `revoke(p, cap)`, `granted(p, cap)` | a player's powers, records in `RulesExt` ("Roles, powers and capabilities") |

The engine update `0x15c150` and the post-game step `0x15ca10` are
traced (live-gamestate.md) and are where the lifted engines do their
per-tick work; they are not API entries.

Two layouts of the engine vtables are on record and not reconciled:
bots.md section 3.7 gives `0x45c6d8` (Slayer) and `0x453d38`
(Juggernaut), the base of firefight.md's kill callback at `+0x78`;
game-options.md gives `0x45c650` and `0x453cb0`, `0x88` lower, with the
trait query at `+0x114`. Each port reads its engine's vtable again.

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
| `objects.create`, `objects.delete`, `objects.move` | makes, removes and moves objects | no |
| `ai.squads` | places and commands AI | no |
| `hud.message` | shows messages | no |
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
- **A hopper** carries a deny list of capability names; a script that
  declares a denied one is not offered in it. `forge.*` denies the
  family.
- **Ranked** denies `forge.*` always: Live's allow-list
  ("Security and usability") never lists a script that declares one, so
  no host, modified or not, has a ranked game with edits counted.

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

## Security and usability

- **The VM is the boundary.** A blob runs on whichever machine hosts,
  often a player's, and wherever the film is replayed. The verified IR,
  a VM in memory-safe Rust, the bounds below and an API with no I/O make
  that safe. A guest never executes a script and reaches the VM only
  through its ordinary inputs.
- **Allow-listed where results count.** Ranked hoppers and Live's stats
  and ranks accept a game only when its script hash is on Live's
  allow-list. A modified host can already report any score; a script
  must not be a second way to do it.
- **Verified twice**, at upload and at load, by the same code. Live
  stores only blobs that verify.
- **Capability-limited.** The API table is the whole world: no file,
  network, clock, address, datum or other process, and a script reaches
  only the rows whose capability it declares, which the lobby, the
  hopper and the allow-list read first
  ("Roles, powers and capabilities"). A tag the map lacks or a label it
  does not define is a load error naming it.
- **Bounded.** IR, state, ops per tick, labelled objects, objects made
  per tick, objects alive, grants and edits in a match all have caps; the engine's object and tag
  limits are checked before a create.
- **Text.** Labels are at most 32 bytes and messages 128, sanitised by
  the rules for player names (game-options.md section 4) and formatted
  only with typed arguments.
- **Input rate.** `on press` fires at most 10 times a second per player;
  the rest are dropped the same way on every run.
- **Clean failure.** A script error ends the round; the engine call in
  progress returns first.

Tooling:

- **Studio**: a rules editor with errors as you type, the option table
  as a form, and a budget meter (worst-tick estimate, slots, IR bytes)
  that names the costliest handler. The blob carries an optional line
  table for messages; the VM ignores it.
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
(thirteen classics and Superbounce, `community_variants::VARIANTS`)
keep their records, byte for byte: each has no mode and runs its
engine's built-in script once the port has passed parity ("A record with
no mode"). Its settings are already option values in the record. Rules
that were honour rules ([community-gametypes.md](community-gametypes.md),
Zombies' team switch) become script variants of their own.

**Until a port passes.** A no-mode record on an engine whose port has
not passed parity plays the lifted engine, on every kind of session.

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
actors live, and hears `on squad_dead` and `on actor_kill`: the host
actor entries Firefight uses ([ai-networking.md](ai-networking.md)). A
script never runs per actor; actor brains stay the engine's, for the op
budget and for determinism. Phantom bots ([bots.md](bots.md)) are
players like any other to a script.

**Bounces are observed, not made.** Physics stays the engine's: Havok at
an exact 30 Hz tick ([high-framerate.md](high-framerate.md)). A script
hears `on bounce(p, spot)`, reads the scanner's spots as zones, scores
and times them, and sets traits. That makes Bounce Practice and a trick
or parkour mode real scripts, and the superbounce flag (bit 2 of the
block's flags) a script's own choice. The detector's rule is a traced
signature, not the launch rule (Open in superbounce.md), so it can miss
or over-report until it is checked in play.

## Payoff

- **Race.** Halo 1's race track points are dropped at conversion
  ([multiplayer.md](../halo1/multiplayer.md)); labelled checkpoints and
  a Race script give them a use. So do **Halo 1's per-spawn game
  types**: starting locations carry labels and a script filters on them.
- **Infection** with a real team switch, and other modes that were
  honour rules.
- **Gun Game.** `on kill` takes the killer's weapon and gives the next
  in a constant table: the weapon progression the variant editor cannot
  express ([community-gametypes.md](community-gametypes.md)). `take` is
  traced and `give` is not, so until it is the swap happens at the
  killer's next spawn through `set_loadout`.
- **VIP with powers.** The VIP holds a grant to place and move block-out
  pieces; the hunters hold one to delete vehicles.
- **Prop Hunt**, the hardest mode here. A hider's biped is hidden, a
  prop object is attached to the player, damage to the prop is forwarded
  to the player, and the camera is third person. `hide`, `attach`, the
  body swap and `camera` are all untraced, and damage forwarding needs
  an event the API lacks. The native renderer can skip drawing a biped;
  its hitboxes and collision stay until the engine work is done.
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

A label is a change of the document format: the format is one number
(`forge_doc::FORMAT`, 1), the document's structs refuse unknown fields
and `crates/formats/forge-doc/src/validate.rs` refuses any other number,
so a labelled document takes the next number and an older build refuses
it whole.

## Roadmap

Each phase lands by itself and ends on its exit criteria.

| # | Phase | Exit criteria |
|---|---|---|
| 1 | `SimExt` side arena (forge-design.md slice F7; sim-rewrite.md section 2) | a test record survives checkpoint, revert, snapshot and a film keyframe, and changes the sim hash |
| 2 | Forge labels | labels round-trip through the document and the edit log; the built-in flag-type table resolves on a disc map (`#[ignore]` test) |
| 3 | Language, verifier, IR, VM, host runtime, capabilities | the compiler and verifier pass their suites; a fuzzer finds no panic in the verifier or VM; a script scores a fake game through the `Game`-style trait; a call or grant outside `requires` is refused, and a blob naming an unknown capability is refused by that name |
| 4 | Firefight port | every built-in definition, compiled to IR, gives the Rust director's `Command` log in the director's own tests; `firefight.state` reads the same from both |
| 5 | Slayer port | parity on every retail Slayer built-in on three maps; a script Slayer game played on Live |
| 6 | The other six ports, in the order Juggernaut, Oddball, King, CTF with Assault, Territories | parity per engine; the API rows each needs are named points |
| 7 | AI squads for any script | a script outside Firefight places, orders and erases squads on a kit map; `on squad_dead` fires on the tick the count reaches 0 in a test over the fake squad entry |
| 8 | Bounce events | the detector agrees with stepped traces of known bounces and reports none on a recorded match without one; Bounce Practice runs as a script |
| 9 | Spawn-area control and the loadout picker | the `PLAYER_SPAWN_LOCATION` rule is traced; a labelled subset is the only one chosen in a headless match; the picker sets a loadout before a spawn |
| 10 | Editors: Studio's rules editor, and in-game option rows from the script's option table | a variant made in Studio with a custom script plays from a hopper; the in-game editor changes a script option |
| 11 | Retire the lifted engines | the retirement criteria above |
| 12 | Capabilities on Live: the lobby's plain-words list, hopper deny lists, the allow-list's `forge.*` rule, in both homes | a script declaring a denied capability is not offered in that hopper; the parity suite passes in both homes |
| 13 | Research: `give`, and `take` called outside a drop | each is a named point proven by a differential test; Gun Game swaps weapons on the kill's tick |
| 14 | Forge in a match: grants, the match edit log, the player's tool (after forge-design.md slices F5, F8 and F15) | over a fake game, a granted proposal is stamped and costs one use, and one past the budget, outside the palette or in a ranked session is refused; a film with entries replays to the same `RulesExt` hash |
| 15 | Research: `hide`, `attach`, the body swap, `camera`, damage forwarding | each is a named point; Prop Hunt plays in a headless match |

Phases 7 to 9 are the phased mode's prerequisites and do not block the
ports. Phases 12 to 15 follow phase 3 and block no port; 13 feeds Gun
Game and 15 feeds Prop Hunt. The Forge graph front end follows phase 3 on Forge's own schedule;
the ReachVariantTool importer follows phase 10.

## Not verified

- **HUD, scoreboard and announcer.** How each engine shows objective
  state, score labels, messages and announcer lines on a client is not
  traced: whether from the replicated engine-globals entity, the game
  engine event, or the client's own engine vtable. This is the largest
  unknown; it decides the display half of every port and of every new
  mode.
- **Answering a carrier engine's callbacks.** Which callbacks make
  rules, per engine; whether each can be answered at its entry with an
  async body; and whether doing so leaves the replicated globals entity
  consistent. Only Slayer's kill callback is on record.
- **Queued calls.** Whether object creation, deletion and teleports are
  legal inside each callback is not traced. A port that needs one in the
  same tick as retail fails parity there.
- **A join gate and the allow-list.** Neither is designed: the session
  and matchmaking path only lets a host log who lacks a capability, and
  where Live keeps the allow-list and which routes consult it is open.
- **The session's `rules` field** needs the shape checks the server runs
  on `variant_ext` (`NewSession::validate`,
  `crates/live/live-proto/src/session.rs`) in both homes: `live-server`
  and the Cloudflare worker (`crates/live/live-worker`).
- **Round end and the winner.** Only the game-end latch is known.
- **`set_team`, per-player traits, movement speed, the body a player
  spawns in.** No entry is known.
- **`give`, `hide`, `attach`, `camera`, and `take` outside a drop.** No
  entry is proven. `0x10ca80` is known only as an observer of a pickup,
  and `0xce920` only on the drop path. `set_loadout`'s routine
  `0x19eda0` takes no player, so a loadout per player rests on knowing
  the spawning player from the enclosing `PLAYER_SPAWN` (**S**).
- **Edits in a match.** Whether a placement's scale reaches collision
  (forge-design.md); which object groups the distributed protocol
  replicates when the host makes one (forge-design.md slice F3); the
  player's tool outside a monitor body; where a hopper keeps its deny
  list.
- **Spawn areas.** How `PLAYER_SPAWN_LOCATION` weighs starting locations,
  and whether answering it with a subset is enough.
- **The bounce detector.** Its rule is a signature from traces of one
  patch on one map; the launch rule is not decoded (superbounce.md
  "Open").
- **Squad entries in play.** Firefight's runs R1 to R4 (firefight.md
  "What a run settles") settle them for scripts too.
- **Host migration.** Whether a session of ours moves hosts, and how
  `RulesExt` would reach the new one. There is no handler for it.
- **The ReachVariantTool importer.** What of Megalo's slots, object
  types and trigger kinds cannot map onto this API is not surveyed.
- **The phased mode's name**, and VM time per tick with 16 players.

## Prior art

- **Megalo.** Every Halo: Reach game type is a Megalo script, as are
  Halo 4's and Halo 2: Anniversary's multiplayer: triggers of conditions
  and actions, run each tick or on events. Variables are fixed slots of
  five types (number, object, player, team, timer) in four scopes, each
  with a network priority; nesting is two levels; a variant has about
  20 KB.
  [Introduction to Megalo](https://github.com/XboxChaos/nitrogen/wiki/Introduction-to-Megalo),
  [MegaloEvolved](https://github.com/craftycodie/MegaloEvolved),
  [the Reach editing kit](https://c20.reclaimers.net/hr/hr-ek/).
- **The ReachVariantTool dialect** has no expressions (`a = b + c` is
  not valid), no parentheses, no `else`, `or` binding before `and`, and
  no user-declared variables.
  [Syntax](https://davidjcobb.github.io/ReachVariantEditor/script/syntax.html),
  [if](https://davidjcobb.github.io/ReachVariantEditor/script/if.html).
- **Project Reclaimer** brings Megalo to Halo 3: host-only execution,
  presses limited to 10 a second, the whole file validated before a
  round, labels up to 32 bytes, and the built-in modes kept with script
  layered on top. [Megalo in Project Reclaimer](https://projectreclaimer.dev/megalo).
  This design takes the first four and replaces the built-in engines.

No source derived from a leaked build is used.

## Related

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
