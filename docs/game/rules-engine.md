# Rules engine: scripted game types

Status: Design, partly built: the `rules` formats crate runs on a fake game in tests and in the web workbench, and the host runtime on invented guest memory in tests only. Decisions by the user, 2026-11-21.
Code: `crates/formats/rules` (language, verifier, IR codec, capabilities, VM); `crates/extensions/game/src/rules` (the host runtime). Tests: `cargo test -p rules`, `cargo test -p game rules::`.
Switch: `rules` in `H2_FEATURES`, off by default ([switches.md](../runtime/switches.md)); the `rules` cargo feature of `game` and `boot` compiles it in.

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
variants, `.h2variant` files of either version (`variants::portable`), cloud saves, the
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

**A game variant file carries the script whole.** A version-2
`.h2variant` ([variants.md](../editor/variants.md#portable-files)) is a
whole game variant: its record carries the prefix that names the script,
and the file carries the script that prefix names, as its IR blob
(base64) and, when it was made from source, the source. Reading one
checks that the IR hashes to the file's hash, that the hash starts with
the record's prefix, that `rules::verify::load` takes the IR and that
its carrier is the record's engine; a record with mode 2 and no script
is refused. So a variant file plays with no server, and Live keeps game
variants as these files ([live-api.md](../live/live-api.md#game-variants)).

**To other machines: the Live session.** The wire codec drops the spare
bytes (`0x7cc50`/`0x7d520`, **E**, game-options.md "How a variant reaches
other machines"), so the block already travels as
`NewSession.variant_ext` (**Code**,
`crates/live/live-proto/src/session.rs`). The session has one field
beside it, `rules`, holding the full 64-digit hash: lowercase hex, checked
by `NewSession::validate` (`valid_rules_hash`), kept by an update and
listed wherever `variant_ext` is. The host publishes it with
`Hle::set_session_rules` and a guest reads it with `Hle::host_rules`
(**Code**, `crates/live/live-proto/src/session.rs`,
`crates/live/live-hle/src/sessions.rs`).

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
the lifted body ("The host runtime"). No guest memory is patched.

The engine table `0x55e4d0[index]` holds one static object per engine,
and the object's `+0` is its vtable: CTF and Assault `0x459d18`, Slayer
`0x45c6d8`, Oddball `0x45c7a8`, KotH `0x45c878`, Juggernaut `0x453d38`,
Territories `0x45c948`, 51 slots each. Every slot is thiscall (ECX the
object, stack arguments the callee pops). The slots that make rules
(**E**, [notes/2026-11-21-engine-vtables.md](../engine/notes/2026-11-21-engine-vtables.md)):

| Slot | Called from | What it does | Named points |
|---|---|---|---|
| `+0x78` kill callback | last in `PLAYER_KILLED` (`0x15d6a4`): attacker, victim, flag, kind; `ret 0x10` | Slayer's and Juggernaut's kill scoring; Oddball, KotH and CTF post events only; Territories does nothing | `SLAYER_KILL` `0x2bc000`, `JUGGERNAUT_KILL` `0x1922a0` |
| `+0x48` per tick | `ENGINE_UPDATE` `0x15c150` (`0x15c2dc`), no arguments | Oddball, KotH and Territories score from it; Slayer's is the shared empty `ret` | `ODDBALL_TICK`, `KOTH_TICK`, `TERRITORIES_TICK`, `JUGGERNAUT_TICK`, `CTF_TICK` |
| `+0x3c` per player | `ENGINE_UPDATE` (`0x15c247`), the player | CTF and Assault capture from it (`0x242e00`); Slayer's and Territories' are empty | none |
| `+0x54`, `+0x58`, `+0x5c`, `+0x60` | objective registered, unregistered, picked up, dropped (`0x15e050`, `0x15e130`, `0x15e250`, `0x15e460`) | the objective engines' own handling | none |
| `+0x6c` | `PLAYER_KILLED` and the spawn-location code: two teams | true when they are enemies; Juggernaut has its own | none |
| `+0x8c` trait query | per player in `ENGINE_UPDATE` (`0x15b28e`): player, trait | true when the variant gives the player the trait | `ENGINE_TRAIT_QUERY` `0x24dda0` (Slayer's, and every other engine's fallback) |

The round resolution `0x15b3a0`, the lives checks and `PLAYER_SPAWN`
make no slot call. A Slayer carrier's tick has no body of its own to
answer, since its `+0x48` and `+0x3c` are bodies every engine shares; its
per-tick handlers answer at `ENGINE_UPDATE`. How much each engine's
client-side display depends on its own vtable rather than on replicated
state is not traced ("Not verified"); slot `+0x9c` gives the replicated
engine-globals entity's type (Slayer 0 to Juggernaut 5).

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
A System Link game runs any other script only when its blob is on the
host's disk, in the game-variant store ("Live game variants in the
game"); it never fetches one.

### The host runtime

**Code**, tests only (`crates/extensions/game/src/rules/`): `runtime`
holds one game's script and implements `rules::vm::Game` over guest
memory, `guest` the reads and engine calls behind the API entries, and
`feature` the hooks. It follows Firefight's runtime
(`crates/extensions/game/src/firefight/`, [firefight.md](firefight.md#the-host-runtime)).
Boot registers the `rules` feature (`registration::register_core`), off
unless `H2_FEATURES` names it, and its hooks are on game-thread points
only (`render_thread::REFUSED`).

**At the game's start**, an observer after a successful `GAME_ACTIVATE`
(`0x18f000`) reads the active record's block (`G+0x13c+0x10c`) and engine
index (`+0x44`):

| Record | What runs |
|---|---|
| mode 2 | the blob whose SHA-256 starts with the record's 8-byte prefix: a built-in (`rules::builtin`, compiled once at first use), else the game-variant store's (source `local`), else Live's by the full hash a store bundle names for the prefix (below); a System Link game takes a built-in or the store's, never Live's |
| mode 0 | the built-in of the record's engine when the engine is in `runtime::PORTED`, which is empty: nothing runs, and Slayer stays lifted until its port passes parity |
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
does not start with the record's prefix, when it rides another engine
than the record's, when it calls an entry no host can make
(`verify::unhostable`), or when it needs what this runtime does not do
yet (`runtime::host_refusal`): an event other than `init`, `kill`,
`timer` and `tick`; a read other than the scores, kills, deaths,
`alive`, `team_of`, `lives`, `round_time_left` and `is_objective`; an
action other than `score`, `end_round`, `set_lives` and `respawn`; a
variant field other than `team_play`, `score_limit`, `time_limit` and
`lives`, whose record offsets are not traced; a `grant`; a Forge label.
Then the VM starts with the record's option bits and seed 0 (as
Firefight's, until the session carries one), `on init` runs, and its
state becomes the `RULES` record. **A refused script is logged
("refused, the game plays the carrier engine's own rules: ...") and kept
with its reason for `rules.state`, and the game plays as the record's
own engine.** Nothing runs with a call stubbed out.

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
| `on timer`, then `on tick` | an observer after `GAME_TICK` | at the next tick boundary |
| `on init` | the start observer | at the first tick boundary |

This differs from "Order" in one way, for every handler: a call is made
at the next tick boundary, not inside the event. `PLAYER_KILLED` is
opaque, so a hook there can only observe; the carrier's kill callback
(`SLAYER_KILL` and the engines' other named slots, "Which engine index a
script rides") has no hook in the feature yet; and the per-tick group
runs after `GAME_TICK`, not in place of `ENGINE_UPDATE`. So a score
lands one boundary after the kill, a read in the same handler does not
see it, and the lifted carrier's own kill scoring still runs beside a
script's. A mode 2 script on a lifted carrier is therefore not a parity
game; answering the carrier's callbacks in place ("Which engine index a
script rides") is what makes one.

The calls: `score(p, n)` is `SCORE_ADD` (`0x15b7c0`) with `n` in EAX,
clamped to ±30,000, and the salted player datum; `set_lives(p, n)`
writes player `+0x1ac` and marks the player entity's lives bit through
`PLAYER_ENTITY_DIRTY` (`0xa7840`); `respawn` and `end_round` are
Firefight's bodies (`firefight::guest::respawn`, `end_game`), and the
winner is not passed, since naming one is not traced. A script error
ends the round the same way. A call the engine refuses is kept, with its
tick and entry name, in `rules.state`'s `errors`.

**Authority.** The first tick boundary's body also calls
`GAME_ENGINE_AUTHORITATIVE` (`0x15b2f0`). A machine it answers false for
drops the script, its calls and its record ("this machine is not the
game engine's authority"). A film's playback is not told apart: it runs
the script where the replayed engine answers true.

**State.** The `RULES` record (`sim_ext::RULES`) holds
`RulesExt::encode`, set at the start, written after every run that
changed it and removed after `GAME_END` and at the next start
([sim-rewrite.md](../engine/sim-rewrite.md#2-who-owns-state)), so
checkpoints, snapshots, film keyframes and the sim hash carry it. Before
each run the record is compared with the bytes last written: a
checkpoint revert or a snapshot load that changed it resumes the VM from
it (`Vm::resume`), and a record that is gone (a full restore) stops the
script with a refusal. A snapshot is refused while calls wait for the
next tick boundary.

The tests (`crates/extensions/game/src/rules/runtime/tests.rs`) drive the
runtime over invented guest memory with fake engine callees: the
built-in Slayer starting from a record and setting the record, a kill
making a `SCORE_ADD` call at the next boundary, a suicide's negative
score, a machine that is not the authority, a refusal for an unknown
hash and for System Link, refusals by entry and by event, another
engine's record, a record with no mode, the record removed at the game's
end, a reverted record resuming the VM, a stored script running with no
Live client and on System Link, a Live variant's script fetched by its
full hash and run, and each fetch failure refused by name.

#### Reloading a running script

The agent API's `rules.reload`
([agent-api.md](../runtime/agent-api.md#rules-scripts)), and Studio's
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
   another engine than the record's, or no agent guest calls.

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

### Live game variants in the game

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
  Each entry's pairs are its record's settings, and at `GAME_ACTIVATE`,
  before the game copies the options, a variant built from one of these
  entries gets its bundle's block written into the options' record
  (`inject::apply_block`, feature `game-variants`, ahead of the options'
  transport to guests). So choosing one in Custom Games makes the game a
  script game of that script, and its extras apply.
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
  saves are only read. `variants.port.script` decides the script:
  `keep` (the record as it is, its engine's lifted rules) or `builtin`
  (the engine's built-in script named in the block and carried). A save
  whose record already names a script is refused by name: its IR is not
  in the save. One line per run gives the counts ported, skipped and
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

## The language

A source file is a header, options, variables, functions and handlers.
The header names the script, its version, its carrier, its teams and the
capabilities it requires: `requires player.weapons, hud.message`.

**Types.** `int` (32-bit, saturating), `fixed` (16.16, for seconds and
distances), `bool`, `player`, `team`, `object`, `squad`, `timer` (a global variable that
counts ticks down, then fires `on timer`), and `enum` types a script declares.
`player`, `team` and `object` values may be `none`. A `table` is constant
rows of typed columns, indexed by `int`. There are no floats and no
strings at run time: text is a string-table index with typed arguments.
An `int` indexes a table's rows; a value of the first column's type
finds the first row holding it (`points[class]`), and a key no row holds
reads the columns' zero values. A row out of range is a script error.

**Expressions and control.** Ordinary expressions with precedence and
parentheses (`a = b + c * 2`), `if` / `else if` / `else`, `and` binding
tighter than `or`; both short-circuit. A statement ends at a line break,
`;` or `}`. A `let` or a parameter is not assigned again. These are the
things the ReachVariantTool dialect lacks ("Prior art").

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
`on actor_assist(killer, assist, class)` (after its kill),
`on bounce(p, spot)`, `on edit(p, o)` (a granted power was used),
`on join(p)`, `on quit(p)`, `on round_end`. A handler runs to
completion.

**Loops** are `for each` over a finite set only: `players`, `teams`,
`objectives` (the objective registry's objects, at most 8, by object
handle) and `objects labelled "x"`, each with an optional `where`. A set
passed to `count`, `first` or `pick` may name its member for the `where`
(`count(players p where p.team == t)`). There is no
`while`, no recursion, and functions are inlined, so every handler has a
static worst-case op count, which the compiler prints.

**Names, not datums.** Engine things are named by tag path
(`"objects/weapons/..."`, checked against the map at load) or by Forge
label. A script never sees a datum index or an address.

**Options** a script declares are the rows a variant editor shows for
it: `option name: type range = default`, with a label and a description
in the string table. `variant.<field>` reads a retail record field by
name: `team_play`, `score_limit`, `time_limit`, `lives`,
`respawn_time`, `friendly_fire`, `rounds`, `engine`, and the type-area
fields the ports read (`caps::VARIANT_FIELDS`, which names each one's
`record::SettingId` and offset): `oddball_ball_count`,
`king_moving_hill`, `ctf_flag_type`, `ctf_reset_time`,
`ctf_touch_return`, `ctf_flag_at_home`, `assault_arm_time`,
`territories_count`, `territories_contest_time`. The meanings of
`oddball_ball_count`, `ctf_reset_time`, `ctf_touch_return`,
`ctf_flag_at_home` and `territories_contest_time` are inferred from the
editor's page labels, not from a reader.

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
    if n == 2 { create("<core tag path>", first(objects labelled "core")) }
}
on pickup(p, o) { if phase == 2 and not is_objective(o) { register_objective(o, team 0) } }
on round_start { open(0) }
on tick {
    let zone = first(objects labelled "zone" z where z.number == phase)
    if phase < 2 and ticks_held(zone, team 0) >= seconds(capture_time) { open(phase + 1) }
}
on enter(zone, o) {
    if phase == 2 and zone.label == "goal" and is_objective(o) { end_round(team 0) }
}
on timer(clock) { end_round(team 1) }
```

The whole mode, with respawn waves and loadouts per team, is
`crates/formats/rules/src/builtin/siege.rules`.

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
| `on squad_dead`, `on actor_kill`, `on actor_assist`, `on bounce`; `on init` | after `GAME_TICK`, from what the tick's observers gathered; `init` after `GAME_ACTIVATE` | queued |

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
| objects by label; `zone.label`, `zone.id`, `o.number` | Forge labels | not built |
| `is_objective(o)`; objective records (object, team, carrier); `o.carrier` (`carrier_of`); `for each objectives` | `E+0xc18`, `E+0xc1c` (`engine::objectives`) | named |
| `o.team` (`object_team`): an objective's team, or a flag placement's team through its designator | record `+4`; `E+0x10` (bots.md section 3.7) | traced |
| `player_of(o)`: the player whose unit an object is | unit `+0x13c` (bots.md section 3.7) | traced |
| `inside(p, zone)`: a living unit inside a labelled volume, a territory marker's capture volume, or a hill corner's hill | zone tests, as below | not built |
| zone tests: who is in a labelled zone, behind `on enter`, `on leave` and `ticks_held` | flag polygons and labelled volumes, in Rust under the float rules | not built |
| netgame flags by type and team | `scnr+0x118` (`engine::objectives`); query `0x19ec40` | named; traced |
| per-engine globals (`E+0xfc`): hill, bomb state, territories, Juggernaut mask | `engine::objectives`; `KOTH_GLOBALS` and its siblings in `global.rs` | named |
| events of the tick | hooks on `PLAYER_KILLED` `0x15ce70`, `PLAYER_SPAWN` `0x14f2e0`; `SimRead::events` once sim-rewrite section 7 exists | named; not built |
| squads: actors alive, `on squad_dead`, `on actor_kill`, `on actor_assist` | `actor_net::squads::count_by_squad` and the kill list Firefight's `Observed` carries (`crates/extensions/game/src/actor_net/squads.rs`, `crates/formats/firefight/src/director.rs`) | named |
| `zone_seen(z)`, `zone_distance(z)`: Firefight's drop zones, the objects labelled `ff_drop` in kit order | the zone visibility and distance Firefight's `Observed` carries (`ZoneSeen`) | named |
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
| traits per player; movement speed | answer `ENGINE_TRAIT_QUERY` `0x24dda0` (vtable `+0x8c`; every engine's own `+0x8c` falls back to it); speed has no reader found (game-options.md "Not built") | named point | `player.traits` |
| `set_team(p, t)` | no entry known; the field is player `+0xc0` | untraced | `player.team` |
| `set_loadout` | starting-weapon routine `0x19eda0` | traced | `player.weapons` |
| `create(tag, at)`, `delete(o)` | `OBJECT_DELETE` `0xb8540`; creation as Forge's ([forge.md](forge.md)) | named; traced | `objects.create`, `objects.delete` |
| `register_objective(o, team)`, unregister, picked up, dropped | `0x15e050`, `0x15e130`, `0x15e250`, `0x15e460` | traced | core |
| `spawn_objective(tag, at, team)`: create an objective at a placement and register it | as the CTF and Assault flag spawn `0x2422d0` and Oddball's `0x2bc1f0` do | traced | core |
| `reset_objective(o)`: return an objective to where it spawned | the lifted engines' return path is not traced | untraced | core |
| `role_traits(p, on)`: the carrier engine's role traits from the variant's type area (ball or flag carrier, the Juggernaut and its bit of the role mask `[0x510c9c]+0`) | each engine's trait query, vtable `+0x8c` (Juggernaut `0x1926c0`; the shared `ENGINE_TRAIT_QUERY` `0x24dda0`); which traits each role answers is not read | traced | core |
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
| `place_squad_at(s, class, n, zone, tier)`: a squad at a kit zone index with a weapon tier | Firefight's `PlaceSquad` command, through the same squad entry | named | `ai.squads` |
| `ff_round`, `ff_wave`, `ff_skull`, `ff_lives`, `ff_score`, `ff_respawn`, `ff_finish` | Firefight's host log entries (`firefight::Command`), queued with the squad commands so the log keeps program order | named | `firefight.director` |
| `forge_move(o, at)`, `forge_delete(o)`, `forge_place(tag, at)`, `forge_scale(o, s)`: an edit through the Forge log | a `forge-doc` op, validated and host-stamped (`crates/formats/forge-doc`, `LogHead::stamp`); edit classes in "Roles, powers and capabilities" | named | `forge.move`, `forge.delete`, `forge.place`, `forge.scale` |

VM built-ins reach no engine entry (status **VM-only**):

| Built-in | What it is |
|---|---|
| `count(set)`, `first(set)`, `len(table)`, `clamp(v, lo, hi)` | over a finite set or a constant table; `first` follows the `for each` order and gives `none` for an empty set |
| `pick(set, n)` | `n` members drawn with the VM's generator |
| `seconds(n)` | `n` times the tick rate, in ticks |
| `ticks_held(zone, team)` | a counter the VM keeps per labelled zone and team over the zone test: ticks the team has held it alone |
| `squad(n)`, `team n`, `text n` | constants: a kit squad slot, a team index, a string-table index |
| `now()` | the tick the host began (`Vm::begin_tick`) |
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
| `objects.create`, `objects.delete`, `objects.move` | makes, removes and moves objects | no |
| `ai.squads` | places and commands AI | no |
| `hud.message` | shows messages | no |
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

- **Studio**: the Game type script window
  ([editor-guide.md](../editor/editor-guide.md#game-type-scripts);
  `crates/apps/studio/src/app/rules_edit.rs`) colours the text by
  `rules::ide`'s semantic tokens, lists its diagnostics as you type, and
  offers completion, hover, signature help and format; its check report
  is `rules::workbench::check` (worst-tick estimate against the budget,
  the costliest handler, slots, IR bytes, capabilities). It reloads the
  script into the paired game ("Reloading a running script"). The blob
  carries an optional line table for messages; the VM ignores it.
- **Command line**: `h2 rules` is pure over its inputs and needs no
  disc (`crates/apps/cli/src/rules_cmd.rs`).
  - `h2 rules check <file.rules>` compiles and verifies. Errors print as
    `file:line:col: message` and the exit status is 1. The report gives
    the worst-tick estimate against the budget, the costliest handler,
    the IR bytes, the slots by scope, the blob's id, what the script can
    do in the words of `caps::CAPABILITIES`, and the entries a host
    cannot make.
  - `h2 rules build <file.rules> -o <out.rir>` writes the verified blob
    and prints its SHA-256.
  - `h2 rules run <file.rules> --scenario <scenario.json> [--trace]`
    steps the script over a scenario on the fake game, printing the calls
    made each tick (with `--trace`, each event and call), the final
    scores and the result; a script error exits 1.
  - `h2 rules caps` lists the capabilities and the API entries with their
    status; `h2 rules builtin [name]` lists the built-in scripts or
    prints one.
  - `h2 rules lsp` is a language server on stdin and stdout
    (`crates/apps/cli/src/rules_lsp.rs`, over `lsp-server`). It keeps
    each open document whole (full sync), publishes diagnostics on open
    and change, and answers completion, hover, signature help,
    definition, references, `semanticTokens/full`, document symbols and
    formatting from `rules::ide`.
- **Language intelligence**: `rules::ide` (`crates/formats/rules/src/ide/`)
  is pure over source text, and the language server, Live's
  `POST /v1/rules/ide`, the web editor and Studio share it. Positions are LSP's:
  a 0-based line and a UTF-16 `character`. The lexer and the parser
  recover (`lex::lex_tolerant`, `parse::parse_recovering`): a bad
  character is skipped, a bad statement is skipped to the end of its
  line, a bad declaration to the next line that starts one, and a block
  left open ends at the next declaration. Every feature works from what
  parsed.
  - `diagnostics`: every syntax error; when the source parses, the
    compiler's first error, a verifier refusal, the tick budget and the
    IR size (the last three on the first line); a warning for a `let`, a
    set member or a `fn` never used; and a note at each call of an
    entry no host can make yet, naming its status.
  - `complete` reads the tokens before the place: declarations and
    their snippets at the start of a top-level line; event names, with
    their arguments as a snippet, after `on`; carriers after `carrier`;
    capabilities on a `requires` line and as the second argument of
    `grant`, `revoke` and `granted`; types after `:`; after a `.`, the
    `variant` fields, an enum's variants, a table row's columns, or a
    player's, team's or object's variables and built-in fields; labels
    after `labelled`, palettes after `palette`, and sets after `each`
    and in `count`, `first` and `pick`. In a body it offers the locals
    in scope, the script's globals, options, tables, enums and
    functions, every API entry and language built-in as a snippet, and
    the statements. Each item's documentation is the hover text.
  - `hover`: a name's declaration and type; an API entry's signature,
    whether it reads or acts (and whether it is queued), the capability
    it needs in the lobby's words, and its status in plain words; an
    event's arguments; a `variant` field, a built-in field, a carrier, a
    capability, a type or a keyword.
  - `signature_help` for API entries, language built-ins, a script's
    functions and an `on` handler's arguments, with the argument the
    place is in.
  - `definition` and `references` for locals, arguments, parameters,
    variables, options, enums and their variants, tables and columns,
    functions, palettes and labels (a label's first use is its
    definition).
  - `semantic_tokens` classifies every comment, literal, keyword, name
    and operator by the legend `ide::LEGEND`: keyword, event, function,
    api, variable, parameter, enum, enumMember, type, number, string,
    comment, operator, label, capability. A dotted capability is one
    token.
  - `symbols` is the outline: the header, then each declaration and
    handler, with an enum's variants and a table's columns as children.
  - `format` lays a script that parses out canonically: four-space
    indents by bracket depth, one statement per line (a block's braces
    and `;` break lines), one space around operators and after commas
    and colons, at most one blank line in a row, comments where they
    are. It moves line breaks only where the grammar ignores them and
    refuses its own output if a script that compiled would compile to
    another blob. Every built-in formats to itself on a second pass and
    to the same blob.
- **Editors**: point a generic LSP client at `h2 rules lsp` for files
  ending `.rules`.
  - VS Code: an extension that starts the server, or a generic LSP
    client extension, with the command `h2`, the arguments
    `["rules", "lsp"]` and the document selector `{"pattern":
    "**/*.rules"}`. The custom token types (`api`, `label`,
    `capability`) take colours from `semanticTokenColorCustomizations`.
  - Helix, in `languages.toml`:

    ```toml
    [language-server.h2-rules]
    command = "h2"
    args = ["rules", "lsp"]

    [[language]]
    name = "h2-rules"
    scope = "source.h2rules"
    file-types = ["rules"]
    comment-token = "//"
    indent = { tab-width = 4, unit = "    " }
    language-servers = ["h2-rules"]
    ```

  - Neovim (0.11 or later):

    ```lua
    vim.filetype.add({ extension = { rules = "h2rules" } })
    vim.lsp.config("h2_rules", {
      cmd = { "h2", "rules", "lsp" },
      filetypes = { "h2rules" },
      root_markers = { ".git" },
    })
    vim.lsp.enable("h2_rules")
    ```
- **Scenario tests**: a scenario is JSON read by `rules::scenario`:
  `seed`, `options` (bits; the blob's defaults when absent), `players`
  (`slot`, `team`, `alive`, `score`, `lives`), `teams`,
  `round_time_left`, and `ticks`, each with `events` of `kill`
  (`victim`, `killer`), `spawn`, `death`, `join`, `quit`, `enter` and
  `leave` (`zone`, `object`), `press`, `pickup` and `drop` (`player`,
  `object`), `squad_dead` and `round_end`. Tick 0 reports `init` and
  `round_start` first; each tick ends with the per-tick group; queued
  calls are made at the start of the next tick and after the last.
  `h2 sim` runs a script in a headless match. Each built-in script has
  scenario tests beside its source.
- **Agent API**: `rules.state` (script, hash, refusal, budget use),
  `rules.vars`, `rules.trace` (handlers run and calls made per tick),
  `rules.step` and `rules.load` (swap a verified blob between rounds).
  They join [agent-api.md](../runtime/agent-api.md) when built.

### Workbench

**Code.** The web app's Game Types page (`statsPath("/rules")`,
`web/src/stats/Rules.tsx`, its test half in `web/src/stats/RulesTest.tsx`,
its pure half in `web/src/lib/rules.ts`) is where a script is written,
checked, tested and saved on the web (Studio's Game type script window
is the desktop editor, over the same `rules::ide` and workbench). The browser runs
no compiler: Live's [Rules scripts](../live/live-api.md#rules-scripts)
routes call `rules::workbench` in both homes.

The **Workbench** tab is one screen. The editor is on the left with the
check beside it (Problems, Budget, Capabilities); under them the test
scenario sits beside the results of running it. Under 1100 px the parts
stack in that order. The page's other tabs are **My Scripts** and **My
Game Types**. The layout, its states and its copy are described in
[web-app.md, "Game type scripts"](../live/web-app.md#game-type-scripts).

- **Check.** `workbench::check` compiles, verifies and measures source:
  the IR hash, errors with line and column (0 for a verifier refusal or
  the budget), the worst-tick estimate against the 20,000-op budget, each
  handler's worst case, slots per scope against their limits, IR bytes
  against 64 KiB, the declared capabilities in the lobby's words, and
  every unhostable entry the script calls. The page checks 450 ms after
  typing stops and has no Check button; each error with a place is a
  marker in the editor, and its place in the Problems list selects it. A
  check the server refuses (a source over 256 KiB) is listed as a problem
  of the whole script.
- **Editor.** The script box is Monaco (`web/src/stats/RulesEditor.tsx`),
  loaded by a dynamic import on this page only, so it is a chunk of its
  own: the core editor (`monaco-editor/esm/vs/editor/edcore.main`, no
  bundled languages) and its one editor worker. `stats/rulesMonaco.ts`
  registers the `h2rules` language: a Monarch grammar from the lexer's
  tokens and the parser's words (comments, keywords, `on` and its event,
  scoped `var`, `for each`, `team n`, `text n`, literals), brackets,
  auto-closing pairs, indentation and folding, and a dark theme from the
  stats site's colours. Completion, hover, signature help, go to
  definition (F12), find references (Shift+F12), the outline
  (Ctrl/Cmd+Shift+O), semantic tokens and Format Document (Shift+Alt+F)
  ask Live's `POST /v1/rules/ide` (`{source, op, line, character}`,
  0-based line, UTF-16 character; LSP field names back) and
  `GET /v1/rules/ide/legend`; each op waits for typing to pause, a newer
  request cancels the older one, and answers are kept per model version
  (`web/src/lib/rulesIde.ts`, the pure half). A built-in opens read-only
  until **Use as start**. The **Insert** buttons above the editor open a
  picker each (Declarations, Events, Actions and reads by capability in
  the lobby's words, Control) whose rows insert a snippet at the caret;
  the events and the API come from the completion route at a blank
  top-level line and a blank statement, over a list the page keeps.
- **Run.** `workbench::run` steps a scenario (seated players, variant
  fields, options, seed, round time, and a list of events by tick) over
  `fake::FakeGame`. Each tick: the queue is flushed, the tick's events
  fire in order, then `update` runs timers and `on tick`. A kill marks
  its victim dead and counts a kill and a death, a spawn marks the player
  alive, a join seats one and a quit removes one after its handler. The
  trace keeps the ticks in which a handler other than `on tick` ran, a
  call was made, a variable or player row changed (a timer counting
  down is no change), a warning was raised or the script stopped. The run stops at a script error, after the tick
  that calls `end_round`, or past 4,000,000 ops, and the same input gives
  the same trace. The page runs the scenario by itself, 350 ms after the
  checked script's hash or the scenario changes, and shows the trace as
  final standings and a timeline (`rules.ts`, `timeline` and
  `standings`): each tick the scenario put an event in or the trace
  kept, with the event as a sentence, the handlers that ran, their
  calls, and each score, team, lives count or variable that changed.
- **Editor.** `POST /v1/rules/ide` answers one editor request (complete,
  hover, signature, definition, references, tokens, symbols, format or
  diagnostics) from `rules::ide`, in LSP's field names, so the page's
  editor needs no language server of its own ("Language intelligence"
  under "Tooling").
- **Save.** A signed-in account saves a script that checks clean; Live
  keeps its source under the IR hash and compiles it again to serve the
  blob.
- **Game variants.** **Make a game variant** turns the current script
  into a game variant: a name, a description, a visibility and a value
  for each option the script declares (the check report lists them),
  over the stock record of the script's carrier engine
  (`hopper::variant::VariantRecord::stock`). The record's block gets mode
  2, the IR's hash prefix and the options packed in declaration order
  (`game_variants::pack_options`). **Download** asks Live's
  `POST /v1/variants/bundle` for the `.h2variant` file and works signed
  out; **Publish** posts it to the account's game variants
  ([live-api.md](../live/live-api.md#game-variants)). The **My Game
  Types** tab lists them beside My Scripts, and a player's page lists
  their public ones; each has a page (`statsPath("/variants/:id")`) with
  its settings, its script (opened read-only here) and the file.
- **Drafts.** The page keeps the source and the scenario in
  `localStorage`, and starts from a blank starter, a built-in
  (`builtin::ALL`) or one of the player's saved scripts.

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
`crates/formats/rules/src/builtin/`, found by `builtin::builtin(engine
index)`: `slayer.rules`, `oddball.rules`, `koth.rules`,
`juggernaut.rules`, `territories.rules`, and `ctf.rules`, which serves
CTF and Assault (`builtin/mod.rs` gives it a header per engine, and the
script reads `variant.engine`). Each declares no capability: the
objective entries and `role_traits` are `core`, since they do only what
the retail engine does with the record's own settings. Each file's header
names the entries a host cannot make yet and what the port does not
reproduce; `tests/engines.rs` checks that list against the blob, and
checks each engine's core rule over `fake::FakeGame` (Oddball: time held
scores; Juggernaut: the killer takes the role; King: time on the hill
scores, a contest stops it; CTF: a capture scores and the flag returns,
by reset time and by touch; Assault: a bomb armed on the enemy plate
scores; Territories: a territory held pays each interval). They run in
tests only: no port has a parity run, and only Slayer's entries are all
hostable.

| Script | Waits on | Not reproduced |
|---|---|---|
| Slayer | nothing | |
| Oddball | `spawn_objective`, `reset_objective`, `role_traits` | the Oddball globals; the reset delay is a script option |
| King of the Hill | `label_number`, `inside` | the KotH globals; how the next hill is chosen (the script takes the next number); the type area's other King settings |
| Juggernaut | `role_traits` | who takes the role after a suicide or a quit (the script draws one); whether the kill that takes the role scores |
| CTF and Assault | `object_team`, `inside`, `spawn_objective`, `reset_objective`, `role_traits` | sudden death; the CTF and Assault globals; a team's second plate; detonation after arming |
| Territories | `register_objective`, `inside` | the Territories globals; the marker's index as its registration team; a capture's decay; the score interval is a script option |

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
Zombies' team switch) become script variants of their own. To travel as
a game variant file, a record is ported with `variants::portable::port`
(`h2 variants port`, [variants.md](../editor/variants.md#portable-files)):
`PortScript::Builtin` attaches its engine's built-in script with the
script's default option values (the built-ins read the retail settings
from the record by name, so nothing else is packed) and sets mode 2 with
the script's prefix; `PortScript::Keep` leaves it with no mode. The
retail built-ins port from the disc (`--all-builtins`); the community
variants are built by the game at boot (`community_variants.rs` is in
the game crate) and are not ported by the tool.

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
(`src/scoring.rs`) compile to one rules script: waves, rounds, lives,
skulls and scoring. The wave-set format stays the authoring format.
`firefight::Def` is already validated, bounded and hashed, and the hopper
tool offers definitions; `firefight::script::compile`
(`crates/formats/firefight/src/script.rs`) builds the director script's
IR from a `Def`, with the definition's squads, waves, rounds, caps,
points and multipliers as constant tables. It builds IR, not source: the
loops over the 16 squad slots unroll at build time and the blob depends
on nothing but the definition, so a wave set is one blob with one hash,
mode 1 folds into mode 2, and the built-in sets need no rewrite by hand.
The Rust director and its tests are the oracle: for the same `Observed`
sequence `firefight::ScriptDirector` gives the same `Command` log and the
same director state ([firefight.md](firefight.md#the-director-as-a-rules-script)).

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

The modes below that have a script are examples in
`crates/formats/rules/src/builtin/`, listed by `builtin::examples()`
rather than by engine: `infection.rules`, `gun_game.rules`,
`vip.rules`, `siege.rules`, `race.rules` and `bounce_practice.rules`.
Each compiles, verifies and fits a tick (`tests/engines.rs`), and each
header names the entries a host cannot make yet. VIP and Bounce
Practice call none, but wait on the match edit log and the bounce
detector. Prop Hunt has no script.

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
links (`links.rs`) and labels (`labels.rs`) to tell a script which
placement is which. Status: the document side is built; the `RulesExt`
table and `objects labelled` are not.

**Design.** A label is a string of at most 32 bytes plus an optional
number (`"checkpoint" 3`, `"hill" 0`). It is a field of a placement in
the Forge document, set through the same edit log as any other field, so
it has no scenario bytes and costs nothing in a session that runs no
script. At map load the labels become a table in `RulesExt`: label,
number, placement id, and the object's datum once it exists. Scripts
read it through `objects labelled`. Retail maps with no Forge document
get labels from a built-in table per netgame flag type, so
`objects labelled "ctf_flag_spawn"` works on every map.

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

A label is a change of the document format. The format is one number
(`forge_doc::FORMAT`, 1; `FORMAT_LABELS`, 2). A document takes 2 when
any op is a label op and 1 otherwise, and
`crates/formats/forge-doc/src/validate.rs` refuses the other number, so
a document has one spelling: every document without labels keeps its
bytes and its identity, and a fold that retracts the last label
returns to format 1. This build reads both; an older build refuses a
labelled document whole.

## Roadmap

Each phase lands by itself and ends on its exit criteria.

| # | Phase | Exit criteria |
|---|---|---|
| 1 | `SimExt` side arena (forge-design.md slice F7; sim-rewrite.md section 2) | a test record survives checkpoint, revert, snapshot and a film keyframe, and changes the sim hash |
| 2 | Forge labels | labels round-trip through the document and the edit log (built); the built-in flag-type table resolves on a disc map (`#[ignore]` test, not built) |
| 3 | Language, verifier, IR, VM, host runtime, capabilities | the compiler and verifier pass their suites; a fuzzer finds no panic in the verifier or VM; a script scores a fake game through the `Game`-style trait; a call or grant outside `requires` is refused, and a blob naming an unknown capability is refused by that name |
| 4 | Firefight port | every built-in definition, compiled to IR, gives the Rust director's `Command` log in the director's own tests; `firefight.state` reads the same from both. Built in tests: `crates/formats/firefight/tests/script.rs` compares commands and `State` tick by tick. Not built: the host runtime running the script, and `firefight.state` reading it |
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
- **Answering a carrier engine's callbacks.** The slots that make rules
  are on record ("Which engine index a script rides"); whether each can
  be answered at its entry with an async body, and whether doing so
  leaves the replicated globals entity consistent, is not. Of the
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
