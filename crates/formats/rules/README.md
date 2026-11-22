# rules

A multiplayer game type is a rules script. Source compiles to a small IR
blob named by its SHA-256. A verifier reads only the blob, and a VM runs a
verified blob against a `Game` trait. The crate also holds the fixed
capability list, the numbered engine API and the built-in scripts.

## Where it sits

The `formats` domain. It has no I/O and no guest types, so Studio, the
cli, Live's upload route and the host runtime all link the same code. It
uses `content` for SHA-256. The host runtime
(`crates/extensions/game/src/rules`) implements `vm::Game` over guest
memory, makes its calls through `engine::entry::Cx` and keeps `RulesExt`
as the `SimExt` blob's `RULES` record.

## Pipeline

- `lang::compile` lexes, parses, type checks and lowers source to `Ir`.
  Functions are inlined and recursion is refused. Strings, labels and
  tags are interned in first-use order, so the same source always gives
  the same bytes. Errors carry `line:col`. `build` compiles and verifies
  in one call; `ship` also refuses a blob over the tick budget or the IR
  cap (`fits`), as `h2 rules build`, Live's save and publish routes and a
  variant file made from source do. `message(to, text)` lowers to one of
  three entries by `to`'s type (a player, a team, or `all`); `caps::spelled`
  gives all three the one name.
- `Ir::encode` and `Ir::decode` are the canonical codec: decode accepts
  only the bytes encode writes, and refuses trailing bytes and blobs over
  256 KiB. `Ir::id` is the SHA-256 of the encoding.
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
  next tick boundary. A tick runs at most 100,000 ops and 64 zone events.
  An overflow is a script error: the VM stops at that op, clears its
  queue and calls `Game::script_error`. Division by zero gives 0 and a
  `Warning`. Players are visited by slot, teams by index and labelled
  objects by placement id. `pick` draws from SplitMix64, seeded with the
  match seed XOR the first 8 bytes of the blob's hash.
- `RulesExt` is all VM state: generator, slots, object variables, grants,
  the queue and the `ticks_held` counts (per zone and team, at most 32
  zones, cleared at the round's end; `update` adds a tick for the team
  `Game::zone_holder` names). `RulesExt::encode` writes it canonically in at most
  64 KiB, and `Vm::resume` continues from it.
- Grants are `(player, capability, uses, palette)` records, sorted by
  player and then capability. They end at `revoke`, at the player's quit
  and at the round's end. `Vm::check_proposal` is the host's check
  before it stamps a Forge proposal: a custom or hopper game
  (`policy::Context`), a grant with uses left,
  a tag in the palette (or a match-made target when there is no
  palette), and never a protected target. `Vm::spend` takes one use.

## Policy

`policy` says which kinds of game take a script by its header's
`requires`: `check(context, requires, api)` lists each refused capability
with its reason, `eligible` names the kinds of `PLAYED` (custom, hopper,
ranked) that take it, and `edits_allowed` is true in custom and hopper
games only. Custom and Theater refuse nothing the build knows, a hopper
refuses its deny list (`forge.*` denies the family), ranked refuses every
grantable capability, and an unknown name is refused everywhere. Live's
hopper and session routes, its publish routes and the host runtime all
ask it.

## Built-ins

`builtin::builtin(engine_index)` returns the built-in source for a
retail engine index: Slayer, Oddball, King of the Hill, Juggernaut,
Territories, and one `ctf.rules` body serving CTF and Assault, each under
its own header. `builtin::examples()` lists the payoff modes (Infection,
Gun Game, VIP, Siege, Race, Bounce Practice). Each script's header names
the entries a host cannot make yet. `fake::FakeGame` is a table-backed
`Game` that logs calls, for tests, `h2 rules run` and the workbench; it
also holds an objective registry, object teams, labelled objects, units
and `inside` answers. `begin_tick` counts the round clock down, `apply`
and `after` keep kills, deaths, `alive`, joins, quits and zone occupants
as the host reports them, a `variant` field it was not given reads the
carrier's stock record (`fake::stock_variant`), and a zone whose living
occupants are all on one team is held by it. `scenario::step` runs one
tick over it for both `scenario::run` and `workbench::run`.
`builtin::ALL` lists each port with its name and engine index.

## Workbench

`workbench::check` compiles, verifies and measures a script (hash, errors
with line and column, budget, slots, IR bytes, capabilities in plain
words, unhostable entries). `workbench::run` steps a `Scenario` over
`FakeGame` into a per-tick `Trace`, within 36,000 ticks, 4,096 events and
4,000,000 ops. Live's `/v1/rules/check` and `/v1/rules/run` serve them to
the web app ([Workbench](../../../docs/game/rules-engine.md#workbench)).

The Firefight director script is built outside this crate, by
`firefight::script` from a wave-set definition; the `firefight.director`
entries, `place_squad_at`, the zone reads, `now`, `random` and
`on actor_assist` are what it calls and answers.

## Language intelligence

`ide` is the editor tooling over source text, in LSP positions (0-based
line, UTF-16 character) and LSP field names: `diagnostics`, `complete`,
`hover`, `signature_help`, `definition`, `references`,
`semantic_tokens` (legend `ide::LEGEND`), `symbols` and `format`. It
reads broken source: `lex::lex_tolerant` skips a bad character and
`parse::parse_recovering` skips a bad statement or declaration and ends
an open block at the next declaration, returning every error. A model
pass (`ide/model.rs`) gives each declaration its span and type and
resolves each name with the compiler's scoping. `h2 rules lsp` and
Live's `/v1/rules/ide` serve it
([Tooling](../../../docs/game/rules-engine.md#security-and-usability)).

## Tests

`cargo test -p rules` runs the following. The design page's Slayer,
Infection and Holdout scripts compile, verify and run on `FakeGame`,
with assertions on the calls they make. `tests/engines.rs` checks every
built-in and example: it compiles, verifies and fits a tick, its header
lists exactly the unhostable entries it calls, and each engine port's
core rule holds on `FakeGame`. There is a refusal test for
each verifier rule. The codec round-trips and survives corruption, the
blob hash is stable, a budget overflow stops at the same op on every
run, and `pick` is deterministic. `tests/firefight_entries.rs` covers
`now`, `random`, the queued `ff_*` entries and `on actor_assist`. Grants are checked against the
declaration, the palette and the budget, and `RulesExt` round-trips and
resumes. The recovering lexer and parser report every error and agree
with the strict ones on clean source. `ide`'s tests drive each feature on
a sample script, and every built-in and example has no errors or
warnings, formats idempotently to the same blob and is covered by
semantic tokens. All inputs are invented.

## Docs

- [Rules engine](../../../docs/game/rules-engine.md)
