# Rules engine: scripted game types

Status: Design, partly built: the `rules` formats crate runs on a fake game in tests and in the web workbench, and the host runtime on invented guest memory in tests only. Decisions by the user, 2026-11-21.
Code: `crates/formats/rules` (language, verifier, IR codec, capabilities, VM); `crates/extensions/game/src/rules` (the host runtime). Tests: `cargo test -p rules`, `cargo test -p game rules::`.
Switch: `rules` in `H2_FEATURES`, on by default (`-rules` turns it off) ([switches.md](../runtime/switches.md)); the `rules` cargo feature of `game` and `boot` compiles it in.

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


## Where the sections live

| Page | Covers |
|---|---|
| [rules-engine-carrying.md](rules-engine-carrying.md) | compiler, verifier, the variant record, engine index, blob transport |
| [rules-engine-host.md](rules-engine-host.md) | the VM inside the game, scoring, reload, live variants |
| [rules-engine-language.md](rules-engine-language.md) | syntax, types and statements of the `.rules` language |
| [rules-engine-modes.md](rules-engine-modes.md) | Slayer, Infection, Firefight, the phased Siege mode and the Forge graph front end |
| [rules-engine-ir-api.md](rules-engine-ir-api.md) | the IR and VM, the named engine entries, capabilities and role powers |
| [rules-engine-security.md](rules-engine-security.md) | the VM budgets, threat model and authoring experience |
| [rules-engine-workbench.md](rules-engine-workbench.md) | Studio and web tooling for writing and testing scripts |
| [rules-engine-ports.md](rules-engine-ports.md) | porting the seven engines and the modes built on top |

Each heading below is cited from code and moved to the page named after it.

### Compiler and verifier

Moved to [rules-engine-carrying.md](rules-engine-carrying.md).

### Carrying a script

Moved to [rules-engine-carrying.md](rules-engine-carrying.md).

### Which engine index a script rides

Moved to [rules-engine-carrying.md](rules-engine-carrying.md).

### The host runtime

Moved to [rules-engine-host.md](rules-engine-host.md).

#### One scorer

Moved to [rules-engine-host.md](rules-engine-host.md).

#### Reloading a running script

Moved to [rules-engine-host.md](rules-engine-host.md).

### Live game variants in the game

Moved to [rules-engine-host.md](rules-engine-host.md).

## The language

Moved to [rules-engine-language.md](rules-engine-language.md).

### Slayer

Moved to [rules-engine-modes.md](rules-engine-modes.md).

### Infection: a team switch

Moved to [rules-engine-modes.md](rules-engine-modes.md).

### Firefight: waves of AI

Moved to [rules-engine-modes.md](rules-engine-modes.md).

### Phases: attack and defend

Moved to [rules-engine-modes.md](rules-engine-modes.md).

#### The Siege HUD

Moved to [rules-engine-modes.md](rules-engine-modes.md).

#### Siege options in the lobby

Moved to [rules-engine-modes.md](rules-engine-modes.md).

#### Siege announcer lines

Moved to [rules-engine-modes.md](rules-engine-modes.md).

#### Siege layouts

Moved to [rules-engine-modes.md](rules-engine-modes.md).

### A Forge graph is the same IR

Moved to [rules-engine-modes.md](rules-engine-modes.md).

## The IR and the VM

Moved to [rules-engine-ir-api.md](rules-engine-ir-api.md).

## The engine API

Moved to [rules-engine-ir-api.md](rules-engine-ir-api.md).

## Roles, powers and capabilities

Moved to [rules-engine-ir-api.md](rules-engine-ir-api.md).

## Security and usability

Moved to [rules-engine-security.md](rules-engine-security.md).

### Workbench

Moved to [rules-engine-workbench.md](rules-engine-workbench.md).

## Porting the seven engines

Moved to [rules-engine-ports.md](rules-engine-ports.md).

## Firefight, AI and bounces

Moved to [rules-engine-ports.md](rules-engine-ports.md).

## Payoff

Moved to [rules-engine-ports.md](rules-engine-ports.md).

## Forge labels

Moved to [rules-engine-ports.md](rules-engine-ports.md).

## Roadmap

Each phase lands by itself and ends on its exit criteria.

| # | Phase | Exit criteria |
|---|---|---|
| 1 | `SimExt` side arena (forge-design.md slice F7; sim-rewrite.md section 2) | a test record survives checkpoint, revert, snapshot and a film keyframe, and changes the sim hash |
| 2 | Forge labels | labels round-trip through the document and the edit log (built); the built-in flag-type table resolves on a disc map (`#[ignore]` test, not built) |
| 3 | Language, verifier, IR, VM, host runtime, capabilities | the compiler and verifier pass their suites; a fuzzer finds no panic in the verifier or VM; a script scores a fake game through the `Game`-style trait; a call or grant outside `requires` is refused, and a blob naming an unknown capability is refused by that name |
| 4 | Firefight port | the host runtime runs the definition's script (`ScriptDirector`), and `firefight.state` reads its `Command` log and `State`. Built in tests: `crates/formats/firefight/tests` replays every scenario through a fresh script for the same commands |
| 5 | Slayer port | parity on every retail Slayer built-in on three maps; a script Slayer game played on Live |
| 6 | The other six ports, in the order Juggernaut, Oddball, King, CTF with Assault, Territories | parity per engine; the API rows each needs are named points |
| 7 | AI squads for any script | a script outside Firefight places, orders and erases squads on a kit map; `on squad_dead` fires on the tick the count reaches 0 in a test over the fake squad entry |
| 8 | Bounce events | the detector agrees with stepped traces of known bounces and reports none on a recorded match without one; Bounce Practice runs as a script |
| 9 | Spawn-area control and the loadout picker | the `PLAYER_SPAWN_LOCATION` rule is traced; a labelled subset is the only one chosen in a headless match; the picker sets a loadout before a spawn |
| 10 | Editors: Studio's rules editor, and in-game option rows from the script's option table | a variant made in Studio with a custom script plays from a hopper; the in-game editor changes a script option |
| 11 | Retire the lifted engines | the retirement criteria above |
| 12 | Capabilities on Live: the lobby's plain-words list, hopper deny lists, the allow-list's `forge.*` rule, in both homes | a script declaring a denied capability is not offered in that hopper; the parity suite passes in both homes |
| 13 | Research: `give`, and `take` called outside a drop | each is a named point proven by a differential test |
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
- **The script index's engine object.** The slots that make rules are
  on record ("Which engine index a script rides"), and a script game on
  the script index calls none of them ("One scorer"); no run has shown
  that its replicated globals entity and each client's HUD stay
  consistent. Of the
  Slayer kill callback's arguments, the flag's meaning (when
  `PLAYER_KILLED` sets it) and the kind's are not read, nor the
  CTF and Assault capture path's slot at `0x242741`.
- **Queued calls.** Whether object creation, deletion and teleports are
  legal inside each callback is not traced. A port that needs one in the
  same tick as retail fails parity there.
- **A join gate and the allow-list.** Neither is designed: the session
  and matchmaking path only lets a host log who lacks a capability, and
  where Live keeps the allow-list and which routes consult it is open.
  The gate for a map that breaks a script's labels is a separate design,
  [rules-start-gate.md](rules-start-gate.md).
- **The session's `rules` field** is checked in `NewSession::validate`
  and `Session::apply`, which both homes run (`live-server` and the
  Cloudflare worker); the worker's own run of the test is not seen.
- **The winner of a round or a game.** The game-end latch and the round
  end are known; where the engine keeps the winner is not. That a
  statborg row's `+0x02` is the earlier rounds' score, which the
  post-game shows, is **S**: the round reset adds the round's `+0x00`
  into it and zeroes `+0x00`, and the game's end adds it in and keeps it
  ([notes/2026-11-27-next-round.md](../engine/notes/2026-11-27-next-round.md)).
- **`set_team`, per-player traits, movement speed.** No entry is known.
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
  patch on one map; the launch is the island solve's, and the
  sideways normal the convex agent picks is not decoded (superbounce.md
  "The launch").
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

