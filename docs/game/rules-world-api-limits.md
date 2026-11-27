# Rules world API: scripts, limits and plan

Firefight and Superbounce as scripts, splitting a script, security, performance and the plan. Part of [rules-world-api.md](rules-world-api.md).

## Firefight and Superbounce as scripts

[rules-world-modes.md](rules-world-modes.md) lists what each mode needs
from this API. `content/modes/rules/firefight.rules` holds the Firefight
director. `firefight::ScriptDirector` loads it with the selected wave
set's tables and preserves the host's `Command` stream. The Superbounce script keeps
the medal chain's count and scores on the host's physics events and
matches the Rust chain (`crates/extensions/game/src/superbounce_chain.rs`)
on every tick of its test scenarios ("Physics" above); the Rust chain
still judges each landing against the scan's strips and feeds the HUD's
medals and the records. Engine code is deleted only when its script
gives the same output over that code's own test scenarios, and until
then both run in tests side by side.

## Splitting a script

A built-in is 30 to 120 lines today; a hand-written Firefight director
is several hundred. Three ways to keep one readable:

| Way | What it is | Cost | Recommendation |
|---|---|---|---|
| Sections | `section "Waves"` on a line of its own, between declarations. It compiles to nothing. `ide::symbols` nests the declarations after it under it, editors fold it, and the workbench shows the outline as a side list that jumps to a section | a keyword, the outline and the formatter | build it with slice W2 |
| Modules (**built**, W10) | `use "waves.rules"` pulls another file's declarations in before checking. One blob, one hash, and the bundle stores every file in `script.modules`. The CLI and `h2 rules lsp` read paths beside the script; Live and the workbench keep a script as a small set of named files | the bundle format, Live's script routes, the workbench's tabs, diagnostics with a file | the language, CLI, editor and bundle are built (rules-engine.md "Modules"); Live's routes and the workbench's tabs follow |
| A node view | a read-only graph of handlers, the variables they touch and the calls they make, drawn from the checked script | a new web view | not now. Forge's logic graph is the visual front end and compiles to the same IR (rules-engine.md "A Forge graph is the same IR"); a second node editor would be a third front end |

## Security and limits

[rules-world-limits.md](rules-world-limits.md) is the contract every
slice is held to, and each slice's row below names the tests that show
it. In short:

- **Who controls what** is named for each direction: script to engine,
  host to client, client to host, Live to host and Live to client.
- **The verifier refuses; the runtime does not clamp.** A constant out
  of range, an index outside the blob's tables and a worst tick that
  passes a write cap are refusals before the game. A computed value out
  of range is a refused call, recorded; a cap passed at run time is a
  script error.
- **Every world write has a cap a tick and a cap a second**, and a
  `Viewer` enforces the show channel's caps on what it receives.
- **No script value reaches engine memory unchecked**: no entry takes a
  size, an offset or an address, and a handle is checked live in one
  place.
- **A show record is used only after its tag and every field check**; a
  bad one is dropped and counted.
- **On Live**, the hopper gate checks the verifier's estimates, the
  denied capabilities, the map's labels and sounds, and the HUD layout.
- **Every new entry has a property test**, and the show codec a fuzz
  target.

## Performance

The same page gives each entry's cost in VM ops and what the host pays
outside the VM, the rule that nothing allocates in a tick, the show
channel's batching, the bandwidth table and the bench gates: three
targets common to every slice, and the one in each slice's row below.

## Implementation plan

Every slice leaves the tree green, lands by itself and ends with
something to play. Paths under `rules/` are in
`crates/formats/rules/src`, under `game/` in
`crates/extensions/game/src`. Each slice adds its rows to
`caps::ENTRIES`, its signatures to `lang/sig.rs`, its fake to `fake.rs`
and its scenario events to `scenario.rs`, and updates
[rules-engine.md](rules-engine.md#the-engine-api)'s tables and the
`h2-rules` skill in the same branch. A slice whose engine entry is not
named starts with its research row below, and ends with the point named
in `points.rs`. Every slice's tests include a row of
`tests/world_props.rs` for each entry it adds, the verifier's refusals
and caps of [rules-world-limits.md](rules-world-limits.md) for those
entries, and that page's zero-allocation test. The **Bench gate** is the slice's
own target on top of the three common ones, as `rules.world_ns` at the
99th percentile unless it says otherwise.

| # | Slice | Owns | Tests | Done when | After | Bench gate |
|---|---|---|---|---|---|---|
| W0 | Roles: `Authority`, `Replica`, `Viewer` | `game/rules/runtime.rs`, `game/rules/feature.rs`; the blob as a start condition in `game/rules/gate.rs` | over invented guest memory: a mode 3 peer keeps its VM and makes the host's calls; a mode 5 client makes none; a peer without the blob blocks the start by name | a script game scores the same on both machines of a synchronous session | the host runtime running in a game; R1 | a script Slayer game against the lifted one: the tick's 99th percentile within 0.1 ms |
| W1 | Messages everywhere: cues (**built**) | `rules/show.rs` (`Cue`, the codec), `game/rules/world/show.rs`, `crates/live/actor-net/src/wire.rs` and `budget.rs` (`Cue`), `game/actor_net/host.rs`, `client.rs`; `message*` named | the codec round-trips and refuses each bad bound; a cue in three packets shows once; the budget drops the rest the same way twice; the fake logs each message with its audience | `message(all, "…")` from Siege shows on the host and on a Live guest | W0 | 8 cues every tick to 15 peers: 0.05 ms, and 600 bytes a second to each |
| W2 (built but for the workbench's HUD strip on the web, whose run route does not pass the trace's `hud` rows yet) | Script HUD: show state, `hud` and `waypoint` declarations, sections | `rules/show.rs` (`State`, `view`), `lang/*` (the declarations, `section`), `ir.rs` (the header table), `ide/*` (outline, folding), `game/rules/world/hud.rs` (new, over `game/hud_kit.rs`), `wire.rs` (`Show`, the digest), the workbench's HUD strip in `web/src/stats/RulesTest.tsx` | a declared layout that overlaps is a compile error; `view` of the same bytes is the same on host and client at every tick of a fake match; a `Key` restores the state after loss | a script's wave counter and a waypoint draw on host and guest; Firefight's HUD view reads the show state | W1; the HUD kit's runtime drawing | 32 HUD writes every tick: 0.05 ms; `Show` at most 5 a second in `actors.net` |
| W3 (**built**, but a client's view of a host move waits on R2, R3 and R9) | Objects in motion: `vec`, position reads, `teleport`, movers, `on arrive` | `lang/types.rs`, `lang/check.rs`, `lang/lower.rs` (`vec`), `vm.rs` (movers in `RulesExt`), `game/rules/world/objects.rs` (new) | `vec` arithmetic and each type error; a mover reaches its end on the stated tick and survives a checkpoint revert; a teleport outside the world is a refused call; a film of a mover replays to the same `RulesExt` hash | Race with moving platforms and a teleporter pad plays on Live | W0; R2, R3 | 16 movers: 0.15 ms |
| W4 (the events built with their host half, and the Superbounce chain a script, checked by `superbounce_chain_twin.rs` on every chain scenario; `airborne`, `crouching` and `primed` read `Medals::bodies`, `sees` runs at the host boundary; `push` and `launch` wait on R9) | Physics reads, events and pushes: `airborne`, `crouching`, `primed`, `on touchdown`, `on launch`, `on grounded`, the sight table, `push`, `launch` | `vm.rs` (the sight table), `game/rules/world/physics.rs` (the events' inputs), `game/superbounce_chain.rs` (`Physics`, for every player), the `sb_strip` label source in `game/rules/census.rs`; `content/modes/rules/superbounce.rules`, `content/modes/rules/bounce_hunt.rules` | the Superbounce script scores and counts as the chain on every tick of `superbounce_chain_tests.rs`'s scenarios; a `sees` pair answers one tick after it is asked and never from a handler; a push over the bound is a refused call | the Superbounce game type's chain and scores come from the script; the Rust chain stays for what the script cannot do: judging a landing against the scan's strips, a fall through a strip's floor and the Bounced test, which need the map's geometry, and the HUD's medals, the film and Live's records, the ghosts and the Pogo Stick, which read its awards; `on bounce` stays for Bounce Hunt and Bounce Practice | W2, W3; R9 | 32 traces every tick: 0.25 ms |
| W5 | Audio (**partly built**): `sound_to`, `sound_to_team`, `sound_to_all`, with the host finding a sound by the loaded map's tag index; `say`, `sound_at`, `music`, `announce` refused by name until R4 and R5 | `game/rules/world/audio.rs` (new), the `sound` and `line` types in `lang/*`, the tag table's sound group in `ir.rs` and `verify.rs` | a sound that is not in the map blocks the start by name; a `sound_to` cue reaches only its audience; the music slot starts a late machine at the right tick | a wave's stinger and an Elite's taunt play on every machine of a Live Firefight | W1; R4, R5 | 4 sounds every tick: 0.05 ms |
| W6 (director built) | Handwritten director with the wave set as rules data. Generic AI orders and skull writes are planned. | `content/modes/rules/firefight.rules`, `crates/formats/firefight/src/script.rs` (wave-set module and command adapter) | `crates/formats/firefight/tests` covers the director's command stream, `crates/formats/rules/tests/firefight.rs` steps the source on a fake host | Firefight runs from `firefight.rules` through its host adapter. Generic replacement of the `ff_*` and zone entries depends on AI orders, skull writes and replicated HUD state. | W2, W4, W5 for generic host entries | at 24 actors, `rules.vm_ns` at most 0.4 ms |
| W7 | Bot orders (**built**) | `crates/live/bots/src/brain.rs`, `decisions.rs` (an outside order as the goal), `pad_source.rs`, `lib.rs`, `phantom.rs` (delivery), `game/rules/world/bots.rs` (new) | a phantom with `bot_hold` plans to the zone on a nav fixture (`bots::mind::tests`); playback makes no order (`rules::runtime::tests`); an order changes no `RulesExt` byte (`rules::vm::tests`, `rules::runtime::tests`) | Clankers hold the zones a Firefight script sends them to | W0 | 16 orders every tick: 0.02 ms on the game thread |
| W8 | Vehicles and drop ships: `vehicle`, `path`, `fly`, `seat`, `place_squad_in`, `unload` (built on the fake, with host hover and unload; see "Vehicles and drop ships") | `rules/labels.rs` (`path`), `game/rules/world/vehicles.rs`, the kit's stage 2 and its labelled ship and paths in `crates/tools/firefight-kit` | a `path` with a gap blocks the start; a ship flies a path in the stated ticks, facing its tangent; riders are placed and unloaded on the fake | a Firefight wave arrives by drop ship on a kit map | W3, W6; R6, R7, R8 | a drop ship with 8 riders in flight: 0.2 ms |
| W9 | Advanced data: tag reads, squad state, `on damage`, squads by name | `game/rules/world/data.rs` (new), the field resolver over `tags::schema` | a field the schema lacks is a load error naming it; a tag read is the same on two fake machines | a boss wave's bar shows the boss's shield | W2 | 64 tag reads every tick: 0.02 ms |
| W10 | Modules: `use` (language, CLI, editors and bundle **built**) | `lang/*`, `crates/formats/variants` (the bundle's files), Live's script routes, the workbench's tabs | two files compile to the blob of the same text in one; a cycle is an error naming both files | Firefight's director and its wave tables are two files | W6 | none in the game; `h2 rules check` on 256 KiB of source in under 50 ms |
| W11 | The show channel on System Link | a LAN carrier beside `crates/live/live-hle/src/tunnel_host.rs` and `tunnel_client.rs`: `crates/live/show-lan` (**built**, carrier only; wiring waits on R10) | two invented peers over an in-memory LAN and over loopback UDP exchange a `Key` and a `Cue`; a forged tag, a replay, a foreign host key and a flood are dropped and counted (`show_lan::tests`) | a System Link guest sees a script's HUD and hears its sounds | W2; R10 | W2's gate over the LAN carrier |

W0 is first. W1, W3 and W7 then run side by side; W2 follows W1; W4 and
W5 follow W2; W6 closes Firefight, W8 adds the drop ship, and W9 to W11
follow in any order. W2, W3 and W4 each edit `vm.rs` and `lang/check.rs`
and are in different waves.

Research, each a row that makes an entry named before its slice builds
on it:

| # | Question | Settles |
|---|---|---|
| R1 | What `GAME_ENGINE_AUTHORITATIVE` answers on a synchronous peer, and whether its `SCORE_ADD` applies there | W0 |
| R2 | Whether `OBJECT_SET_TRANSFORM` marks the position flag for a host-moved object, by its `notify` argument | W3 |
| R3 | Which object groups get an entity when the host makes or moves one (forge-design.md slice F3) | W3, W8 |
| R4 | The body behind `sound_impulse_start` (`0x2a9e00`) for a positioned and an unpositioned one-shot; a table of a map's dialogue names for `line` | W5 |
| R5 | The start and stop of a looping sound; how a retail game event is raised on the host with a player | W5 |
| R6 | [Traced](notes/2026-11-24-vehicle-actions.md). Load (232) calls `0x11b0c0`, unload (233) calls `0x11b2b0`, hover (621) calls `0xf0fd0`. Squad placement selects a seat vehicle by palette and can spawn one. The start's emitter only defers placement. | W8 |
| R7 | [Traced](notes/2026-11-24-vehicle-actions.md). Notified transforms reach eligible Havok bodies through `0x1c4a80`. The next physics step can change the pose. Exact path following with riders and contacts needs a run. | W8 |
| R8 | The kit's stage 2 transplant of a drop ship and its tag headroom on each map | W8 |
| R9 | How a client's own predicted biped takes a host-set position or velocity | W3, W4 |
| R10 | A datagram of ours between System Link peers through the HLE's sockets | W11 |
| R11 | `objects_attach`, the device setters and an order's follow target | `attach`, devices and `follow`, which no slice above needs |

## Not verified

- Driver-first seat order, hover replication and end-of-tick path following
  with riders and contacts.
- The open questions in "Research" above.
- That a generic object's client extrapolates by the replicated
  velocity between updates, so a mover looks smooth at 10 updates a
  second.
- That 16.16 holds every coordinate of every map, converted Halo 1
  levels included.
- That the game's navpoint list draws more than one slot of ours.
- How many labelled strips a map's scan yields against the 256-member
  bound of a label.
- That a `Replica`'s VM and the host's stay equal through a join in
  progress in a synchronous session.
- Every cap and every bench target of rules-world-limits.md: each is a
  first value to set from a run and from probe P4.
- That the tunnel and the LAN carrier deliver a peer's hello to the host
  unchanged, and where a System Link discovery reply has room for the
  host's key.
- The cost of one HMAC per packet and of 32 traces on a dedicated
  host's core.

## Related

- [rules-world-limits.md](rules-world-limits.md): security, caps, per-entry costs and bench gates
- [rules-world-modes.md](rules-world-modes.md): Firefight and Superbounce as scripts over this API
- [rules-engine.md](rules-engine.md): the language, the VM, capabilities, the engine API and the authority model this page extends
- [rules-language-types.md](rules-language-types.md): front-end types, labels and message holes
- [rules-start-gate.md](rules-start-gate.md): the check a `path` label and a sound tag join
- [ai-networking.md](ai-networking.md): session models, the actor channel, the host log, budgets and probes
- [firefight.md](firefight.md): the director, the kit, the guest boundary and the HUD's mode blob
- [superbounce.md](superbounce.md): the touchdown and bounce rules, the scanner and the medals
- [hud-kit.md](hud-kit.md): the HUD model, the message and navpoint channels
- [bots.md](bots.md): phantoms, the planner and the route runner
- [havok-runtime.md](../engine/havok-runtime.md), [sim-rewrite-havok.md](../engine/sim-rewrite-havok.md): what of Havok is readable and why it is not written
- [film-format.md](film-format.md), [theater.md](theater.md): tagged records and what a film replays
- [dedicated-host.md](../live/dedicated-host.md), [lab.md](../runtime/lab.md): the host tunnel, and the bench the gates run on
