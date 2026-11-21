# rules

A multiplayer game type is a rules script. Source compiles to a small IR
blob named by its SHA-256. A verifier reads only the blob, and a VM runs a
verified blob against a `Game` trait. The crate also holds the fixed
capability list, the numbered engine API and the built-in scripts.

## Where it sits

The `formats` domain. It has no I/O and no guest types, so Studio, the
cli, Live's upload route and the host runtime all link the same code. It
uses `content` for SHA-256. The host runtime (`crates/extensions/game`,
not built) implements `vm::Game` over `engine::entry::Cx` and keeps
`RulesExt` in `SimExt`.

## Pipeline

- `lang::compile` lexes, parses, type checks and lowers source to `Ir`.
  Functions are inlined and recursion is refused. Strings, labels and
  tags are interned in first-use order, so the same source always gives
  the same bytes. Errors carry `line:col`. `build` compiles and verifies
  in one call.
- `Ir::encode` and `Ir::decode` are the canonical codec: decode accepts
  only the bytes encode writes, and refuses trailing bytes and blobs over
  64 KiB. `Ir::id` is the SHA-256 of the encoding.
- `verify` checks every op, including dead branches: register and slot
  types, forward jumps inside their loop body, set and table indices,
  labels, tags, strings, option bits (at most 32), slot budgets, and API
  entry numbers. It also checks that each call's capability is in the
  header's `requires` and that each grant is of a declared, grantable
  capability. An unknown capability name is refused by that name. The
  `Report` holds each handler's worst-case op count and the worst-tick
  estimate. `unhostable` names the first entry whose status is not
  `Named` or `VmOnly`; a host refuses the script by that name.
- `Vm` runs handlers: `fire` for an event, `update` for timers (in slot
  order) and then `on tick`, and `flush_queue` for queued calls at the
  next tick boundary. A tick runs at most 20,000 ops and 64 zone events.
  An overflow is a script error: the VM stops at that op, clears its
  queue and calls `Game::script_error`. Division by zero gives 0 and a
  `Warning`. Players are visited by slot, teams by index and labelled
  objects by placement id. `pick` draws from SplitMix64, seeded with the
  match seed XOR the first 8 bytes of the blob's hash.
- `RulesExt` is all VM state: generator, slots, object variables, grants
  and the queue. `RulesExt::encode` writes it canonically in at most
  16 KiB, and `Vm::resume` continues from it.
- Grants are `(player, capability, uses, palette)` records, sorted by
  player and then capability. They end at `revoke`, at the player's quit
  and at the round's end. `Vm::check_proposal` is the host's check
  before it stamps a Forge proposal: not ranked, a grant with uses left,
  a tag in the palette (or a match-made target when there is no
  palette), and never a protected target. `Vm::spend` takes one use.

## Built-ins

`builtin::builtin(engine_index)` returns the built-in source for a
retail engine index: Slayer, Oddball, King of the Hill, Juggernaut,
Territories, and one `ctf.rules` body serving CTF and Assault, each under
its own header. `builtin::examples()` lists the payoff modes (Infection,
Gun Game, VIP, Siege, Race, Bounce Practice). Each script's header names
the entries a host cannot make yet. `fake::FakeGame` is a table-backed
`Game` that logs calls, for tests and `h2 rules run`; it also holds an
objective registry, object teams, label numbers and `inside` answers.
`builtin::ALL` lists each port with its name and engine index.

## Workbench

`workbench::check` compiles, verifies and measures a script (hash, errors
with line and column, budget, slots, IR bytes, capabilities in plain
words, unhostable entries). `workbench::run` steps a `Scenario` over
`FakeGame` into a per-tick `Trace`, within 36,000 ticks, 4,096 events and
4,000,000 ops. Live's `/v1/rules/check` and `/v1/rules/run` serve them to
the web app ([Workbench](../../../docs/game/rules-engine.md#workbench)).

## Tests

`cargo test -p rules` runs the following. The design page's Slayer,
Infection and Holdout scripts compile, verify and run on `FakeGame`,
with assertions on the calls they make. `tests/engines.rs` checks every
built-in and example: it compiles, verifies and fits a tick, its header
lists exactly the unhostable entries it calls, and each engine port's
core rule holds on `FakeGame`. There is a refusal test for
each verifier rule. The codec round-trips and survives corruption, the
blob hash is stable, a budget overflow stops at the same op on every
run, and `pick` is deterministic. Grants are checked against the
declaration, the palette and the budget, and `RulesExt` round-trips and
resumes. All inputs are invented.

## Docs

- [Rules engine](../../../docs/game/rules-engine.md)
