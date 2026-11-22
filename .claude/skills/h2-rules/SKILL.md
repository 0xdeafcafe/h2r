---
name: h2-rules
description: Use when writing, checking, testing or shipping a game type script in the rules language, a `.rules` file, a rules scenario, a game variant or a `.h2variant` file. Covers h2 rules check/build/run/sim/caps/builtin/lsp/editors, the VS Code and Zed extensions, h2 variants port, capabilities and requires, the tick budget, Studio's script editor and reloading a script into a running game (rules.reload), and Live's /v1/rules and /v1/variants routes.
---

# Write, test and ship a game type script

A game type is a `.rules` script in our own small language. It compiles
to a verified IR blob whose SHA-256 names it, and it plays as a game of one
retail engine (its carrier). You write it, check it, run it over a scenario
on a fake game, then ship it as a game variant. The design is
[docs/game/rules-engine.md](../../../docs/game/rules-engine.md); this
skill is the working loop.

Don't build or run the game to test a script. The scenario is the test:
`h2 rules` is pure over its inputs and needs no disc and no game. The host
runtime is partly built and off by default (`rules` in `H2_FEATURES`), and
a play session shows little that a scenario run does not. Don't add a debug
hook for a script. The [agent API](../h2-agent-api/SKILL.md) is for the
running game, not for scripts.

References beside this file:

- [references/language.md](references/language.md): every declaration,
  event, read, action, type and limit.
- [references/recipes.md](references/recipes.md): worked patterns that
  follow the built-in scripts.

## The loop

```sh
h2 rules check my_mode.rules                        # compile, verify, measure
h2 rules sim my_mode.rules --matches 25             # play generated matches, summed up
h2 rules run my_mode.rules --scenario my_mode.json  # step it on a fake game
h2 rules run my_mode.rules --scenario my_mode.json --trace
h2 rules build my_mode.rules -o my_mode.rir         # the verified blob and its SHA-256
```

1. **Start from a built-in.** `h2 rules builtin` lists the seven engine
   scripts (`engine <index>: <name>`: Capture the Flag, Slayer, Oddball,
   King of the Hill, Juggernaut, Territories, Assault), then the payoff
   examples (`example <key>: <name>`: infection, gun_game, vip, siege,
   race, bounce_practice). `h2 rules builtin slayer` prints one, by name
   (any case), engine index or example key.
2. **Write the `.rules` file**: a header (`rules "Name" 1`, `carrier
   slayer`, optional `teams n` and `requires ...`), options, variables,
   functions, handlers. A new mode carries `slayer`.
3. **`h2 rules check`.** Fix every error and read the report.
4. **`h2 rules sim`** first: it plays whole matches made up from what
   the script handles and says who won, how each match ended, the
   costliest tick, and what looks wrong (the game never ends, nobody
   scores, a score below zero, a handler that never ran), each with a
   seed. `--players N` (1 to 16, default 8), `--teams N`, `--seed S`,
   `--length SECONDS`, `--score-limit N`; `--matches N` (at most 100)
   plays seeds `S` to `S + N - 1`; `--json` prints it all as JSON; `-o
   case.json` writes the seed's match as a scenario file for `h2 rules
   run`. A script error in any match exits 1. Run a flagged seed again
   alone (`--seed 14 -o case.json`) to get its scenario.
5. **Write a scenario** for an exact case and run it. Read the final scores and the calls;
   `--trace` adds each event with what the VM did and each call (arguments
   print in Rust debug form). Write one scenario per rule: a kill, a
   suicide, a betrayal, a timer firing, the round ending.
6. **`h2 rules build`** writes the blob and prints `<sha256>  <path>
   (<n> bytes)`. The same source gives the same hash. It writes nothing for
   a script `check` refuses, over the budget included, and refuses to
   overwrite its own source file.
7. **Ship it** as a game variant or save it on Live.

`h2 rules caps` lists every capability and API entry with its types and
status; run it when unsure whether an entry exists or a host can make it.
`h2 rules lsp` is a language server on stdin and stdout for files ending
`.rules` (diagnostics, completion, hover, signature help, definitions,
references, highlights, semantic tokens, outline, folding, formatting,
quick fixes, rename). A person edits scripts in VS Code or Zed through
it: `h2 rules editors install` installs the VS Code extension and prints
the Zed steps, and `h2 rules editors status` says what is installed and
whether the server starts
([docs/editor/rules-editors.md](../../../docs/editor/rules-editors.md)).
The extensions live in `editors/`; their tests and packaging run in CI
(`.github/workflows/editors.yml`), not in a lane.

## Read the check report

```text
x.rules: "Count" version 1, carrier engine 2, 2 teams
worst tick: 112 ops of 100000 budget (fits)
costliest handler: on kill (7 ops a run)
IR: 214 bytes of 65536
slots: 0 global, 1 per player, 0 per team, 0 per object
id: <64 hex digits>
can do: core only (scores, reads of the game; no powers)
unhostable entries: none
```

- **Errors** print as `file:line:col: message` on stderr, exit status 1.
  A verifier refusal prints `file: refused: <reason>` with no place.
- **worst tick** is the static estimate: each handler's worst case times
  the events a tick can hold (16 for each per-player event, 64 for `enter`
  and `leave`, 16 for `squad_dead`, 1 otherwise), summed. The VM runs at
  most 100,000 ops a tick across all handlers; over that is a script error
  at run time, which stops the VM and ends the round. Over it, `check`
  prints `OVER BUDGET`, then `file: refused: the worst tick costs N ops,
  over the budget of 100000`, and exits 1; `build` writes nothing. The IR
  cap is refused the same way. Live's check reports `ok: false`, and
  Live's save and publish refuse the script.
- **costliest handler** is where to cut. Loops multiply: `for each
  players` inside `for each objectives` is 16 x 8 bodies. Hoist a repeated
  read into a `let`.
- **IR** is at most 256 KiB; source is at most 256 KiB.
- **slots** are the variable limits: 256 global, 64 per player, 32 per team,
  32 per object. Options share 32 bits: an option takes `ceil(log2(hi - lo
  + 1))` bits (a bool takes 1) and the compiler refuses more.
- **can do** lists the declared capabilities in the lobby's words.
  **unhostable entries** are calls no host can make yet; a host refuses
  such a script (see Pitfalls).

## Capabilities

A script declares what it may do: `requires player.weapons, hud.message`.
The compiler refuses a call to an entry whose capability is not declared
(`<name> needs capability <cap>: add it to requires`), and the verifier
checks the blob the same way, so no undeclared call runs. Reads and the
`core` actions (`score`, `end_round`, `set_lives`, `respawn`,
`register_objective`, `spawn_objective`, `reset_objective`,
`role_traits`) need none. The lobby and variant screen show the build's
plain words for each declared name; a script supplies no text about its
own powers.

The fixed list (`h2 rules caps`): `game.traits`, `player.traits`,
`player.team`, `player.spawn`, `player.weapons`, `player.disguise`,
`player.camera`, `objects.create`, `objects.delete`, `objects.move`,
`ai.squads`, `hud.message`, `firefight.director`, and the four
**grantable** `forge.move`, `forge.delete`, `forge.place`, `forge.scale`.

- Only a `forge.*` capability can be given to a player:
  `grant(p, forge.place, budget 20, palette "vip")`. It must be in
  `requires`, the grant needs `budget n` (at most 255 uses), and a palette
  is a list of tag paths declared with `palette "name" = [...]`.
  `revoke(p, cap)` and `granted(p, cap)` end and read it. A grant also ends
  at the player's quit and the round's end.
- A grant carries a budget: each accepted edit spends one use. A match
  holds at most 1,024 scripted and granted edits and 64 objects alive from
  them.
- **Ranked and Theater never take edits.** One policy (`rules::policy`)
  says which kinds of game take a script, by its `requires`: custom games
  take any capability the build knows, a hopper refuses its deny list (the
  hopper file keeps none, so unranked hoppers refuse nothing), ranked
  refuses every `forge.*`, and an unknown name is refused everywhere.
  Live refuses a ranked hopper entry (types 2, 4, 5) or a ranked session
  whose script declares `forge.*`, naming the capability; publishing stays
  open and the variant's `eligible` lists `custom`, `hopper`, `ranked` as
  allowed. The host refuses at the game's start, by the same policy, with
  the reason in `rules.state`. The VM refuses every granted proposal in a
  ranked game or in Theater. Declare the least a mode needs.
- The host runtime refuses, today, any script that calls `grant`: grants
  need the match edit path, which is not built. VIP is a test script.

## The scenario

`h2 rules run` reads JSON (`rules::scenario`); unknown fields are an
error (`scenario: unknown field ...`) and every field is optional.

```json
{
  "seed": 1,
  "teams": [0, 1],
  "round_time_left": 1800,
  "players": [
    {"slot": 0, "team": 0},
    {"slot": 1, "team": 1, "alive": true, "score": 0, "lives": 3}
  ],
  "ticks": [
    {},
    {"events": [{"kill": {"victim": 1, "killer": 0}}]},
    {"events": [{"kill": {"victim": 0, "killer": 0}}, {"spawn": 1}]},
    {"events": [{"kill": {"victim": 0}}]}
  ]
}
```

- `ticks[i]` is tick `i`; the run is `ticks.len()` ticks, at least 1.
  Tick 0 fires `init` then `round_start` first. Each tick ends with the
  per-tick group: timers, then `on tick`. Queued calls are made at the
  start of the next tick and after the last.
- `options` is the raw packed option bits: each option's `value - lo`, in
  declaration order from bit 0. Absent, every option takes its default.
- `players`: `slot` (0 to 15), `team` (default 0), `alive` (true), `score`
  (0), `lives` (0), `unit` (the player's unit object, so `enter` and
  `leave` naming it move the player in and out of a zone). `teams`
  defaults to `[0, 1]`.
- `variant`: record fields by name (`{"team_play": 1, "score_limit":
  25}`); a field left out reads the stock record of the script's carrier
  (a Slayer carrier is free-for-all, score limit 25, time limit 480).
- `map`: `[{"label": "hill", "object": 40, "number": 1, "class":
  "volume"}]`, what the game holds that the script labels, and what
  the members of the script's `label hill: ...`
  visit (`placement` orders them; the
  object's handle when absent; `team` is what `o.team` reads). `class`
  is `object`, `flag`, `volume` or `spawn`, the one the label's kind
  suggests when absent. With no `map` the run uses the smallest map the
  script's labels accept (handles from 256); with one, a label it fails
  stops the run, `h2 rules run` prints `map: <problem>` for each, and
  `h2 rules check` lists what a map needs. `labelled` is the old name.
- `round_time_left` counts down one a tick to 0.
- Events: `{"kill": {"victim": 1, "killer": 0}}` (omit `killer` for a
  death with no killer), `{"spawn": 1}`, `{"death": 1}`, `{"join": 2}`,
  `{"quit": 2}`, `{"enter": {"zone": 3, "object": 9}}`, `{"leave": {...}}`,
  `{"press": {"player": 0, "object": 0}}`, `{"pickup": {"player": 0,
  "object": 5}}`, `{"drop": {...}}`, `{"bounce": {"player": 0, "object":
  0, "chain": 2, "monster": false}}` (`chain` 1 and `monster` false when
  left out), `{"bounced": 0}`, `{"edit": {"player": 0, "object": 7}}`, `{"squad_dead": 2}`,
  `{"actor_kill": {"killer": 0, "class": "elite"}}`, `{"actor_assist":
  {"killer": 0, "assist": 1, "class": "elite"}}`, `"round_end"`.
- A kill marks the victim dead and counts its death and the killer's
  kill; a spawn marks the player alive; a quit removes the player. The
  dead and the departed leave every zone and drop what they carry; a
  `pickup` or `drop` of an objective sets or clears its carrier. A zone
  that living players of one team alone are inside is held by it, which
  is what `ticks_held(zone, team)` counts.
- Output: `tick N: K calls`, the final player rows (team, score, lives,
  alive), team scores, then `result:` one of `the round was still running`,
  `the script ended the round at tick T`, or `script error <kind> at tick
  T, handler H, op P` (exit status 1). The run does not stop at
  `end_round`.

The CLI and Live's run step the same fake game (`rules::scenario::step`);
only their JSON differs (Live's is below). `h2 rules sim` plays on it too, and the scenario
it writes with `-o` holds the whole match: the record's fields, the
labelled objects, each player's unit and every event.

## Ship it

**A game variant** is a version-2 `.h2variant` file: name, description,
the 0x130-byte record whose `hx` block names the script (mode 2 and the
first 8 bytes of its hash), and the script's IR and source. It plays with
no server.

```sh
h2 variants port my_record.bin --script my_mode.rules -o out/
```

- The input is a record: a raw 0x130-byte record, a saved-variant file, a
  hopper slot (0x37c bytes) or a `.h2variant`. Its engine must be the
  script's carrier, or the port fails with `the record is engine N; the
  script rides engine M`.
- `--name NAME` and `--description TEXT` replace the record's name and the
  input file's description (one input only). `--option key=value`, once
  per option, sets a script option by name: an int (whole seconds for a
  `time` option), `true` or `false`, or an enum value's name. An option the script does not declare, a value
  out of range, or any option with `--script keep` is refused. Options
  left out take their defaults. A script over the budget is refused.
- `--script builtin` (the default) attaches the record's engine's
  built-in; `keep` leaves it unscripted; `x.rules` compiles, verifies and
  carries the source; `x.rir` carries the blob alone.
- It prints `<id>  <path>` per file; the id is the SHA-256 of the file's
  canonical bytes, and the same input and script give the same id. Exit is
  nonzero if any input failed.
- With no record at hand, use Live's `/v1/variants/bundle`, which starts
  from the carrier engine's stock record.
- A `.h2variant` placed in the game's `game-variants` data folder joins
  Custom Games' variant list with its script (the in-game store is built
  and tested, not yet played).

**Through Live**, when the CLI is not at hand. Bodies are JSON. The
workbench routes need no sign-in and share a bucket of 20 requests per
client address, refilled at 2 a second (429 past it).

| Route | Auth | Does |
|---|---|---|
| `POST /v1/rules/check` `{source}` | none | `{ok, hash, name, errors: [{line, col, message}], report, options}`; line 0 col 0 is about the whole script |
| `POST /v1/rules/run` `{source, scenario}` | none | a trace, or `{ok: false, errors}` when it does not build; a bad scenario is 400 |
| `POST /v1/rules/simulate` `{source, players, teams, seed, matches}` | none | generated matches: `{ok, teams, summary, rows, match}`; `match` holds the first seed's `scenario` (the run's shape) and `trace`; a batch costs 2 tokens and 1 more per further 25 matches |
| `POST /v1/rules/ide` `{source, op, line, character}` | none | one editor answer (complete, hover, signature, definition, references, tokens, symbols, format, diagnostics) |
| `GET /v1/rules/builtin` | read | `{items: [{name, engine, source, hash, example, key?}]}`: the engine ports, then the examples (`example: true`, `engine` their carrier) |
| `POST /v1/rules` `{source}` | token, `content.write` | saves a script whose check is `ok` (it fits the budget); 201 new, 200 saved already |
| `GET /v1/rules/{hash}`, `/{hash}/ir`, `GET /v1/rules?author=` | read | the source, the blob, an author's list |
| `POST /v1/variants/bundle` `{script_hash or source, name, description, options}` | none | `{id, file_name, bundle}`, unpublished |
| `POST /v1/variants/from-script` (same body, plus `visibility`) | token, `content.write` | builds and publishes; 201 |
| `POST /v1/variants` `{bundle, visibility}` | token, `content.write` | publishes a `.h2variant`; or `{name, description, record_hex, script_hash}` |
| `GET /v1/variants/{id}`, `/{id}/h2variant`, `GET /v1/variants?author=`; `DELETE /v1/variants/{id}` | read; delete by author | read, download, remove |

Live's run scenario is `{players: [{slot, team, lives, unit}], teams,
variant: {field: n}, map: [{label, object, number, placement, team, class}],
options, seed, round_time, ticks, start, timeline: [{tick, events}]}`,
with each event `{type: "kill", victim, killer}`, `{type: "spawn",
player}`, `enter {zone, object}`, `squad_dead {squad}`, `actor_kill
{killer, class}`, `actor_assist {killer, assist, class}`, `bounce
{player, object, chain, monster}`, `bounced {player}` and so on: the handler's name plus its arguments as
fields. `ticks` is the last tick run
(at most 36,000), `start` defaults true, at most 4,096 events. The trace
lists only ticks in which something happened, with `ran`, `calls`,
`globals`, `players`, and the run's `scores`; `cut` says it stopped early
(past 12,000,000 ops), and `limit` that the variant's `score_limit` or
`time_limit` ended the game (only limits the scenario's `variant` sets
end a run). A map thing may name its `team`, which `o.team` reads. A run
whose map fails the script's labels answers `{ok: false, errors: [],
map: [text]}`; the check's `report.labels` and `report.minimal` say what
a map needs.

`options` in `from-script` and `bundle` maps an option's name to a number,
a bool or an enum value's name; an omitted one takes its default; an
unknown name or out-of-range value is 400. A check that is `ok` lists the
options for this form. A published `Variant` lists `eligible`, the kinds
of game its script may play in. A variant name is 1 to 31 characters with no
leading or trailing space and no `|`; a description is at most 150; an
account publishes at most 200. Full reference:
[docs/live/live-api.md](../../../docs/live/live-api.md#rules-scripts).

## Edit it in Studio, reload it into a game

Studio's **Game type script** window (a tab beside the Variants tool's
settings) edits one `.rules` file with colouring, problems, completion,
hover and the check report. **Open in editor** saves it and opens the file
in VS Code or Zed when one has the rules extension (else `$VISUAL`,
`$EDITOR`, `code`, `zed`, `open -t`); each save there comes back into
Studio, and into the paired game while Reload on save is ticked. Agents drive the same state over the editor
socket (`h2 studio`, [docs/editor/agent-api.md](../../../docs/editor/agent-api.md)):

```sh
h2 studio rules.open '{"starter":"slayer"}'      # or {"path":...} / {"source":...}
h2 studio rules.set '{"source":"..."}'
h2 studio rules.get                               # problems, check, reload state
h2 studio rules.save '{"path":"/tmp/mine.rules"}' # reloads too while reload on save is on
h2 studio rules.reload '{"mode":"auto"}'          # answers {queued}; rules.get's reload has the game's answer
```

Studio sends the verified blob to the paired game's `rules.reload`. A
running game can also take one straight from the game's agent API:

```sh
h2 agent rules.reload '{"source":"<the script text>"}'
h2 agent rules.state     # last_reload: mode, hash, tick, reasons
```

- **In place** when the new script keeps the old one's state shape: the
  same carrier, teams, capabilities, options (kind, range, default),
  enums, variables per scope (names, types, order), labels and palettes.
  New handler code and new starting values are fine; every value and
  score carries on.
- **Restart** otherwise (with `mode` `auto`): the same map and variant
  start again with the new script; `reasons` says what changed, one
  sentence each ("the script has 1 player variables, not 0").
- `mode: in_place` refuses rather than restart; `restart` always
  restarts.
- Refused: Theater, a matchmade Live game, a guest, no multiplayer game,
  and a script this host cannot run. A Live custom game reloads in place
  only. In place leaves the record naming the old script, so the next
  game start runs the old one; save it as a variant to keep it.

Design: [rules-engine.md, "Reloading a running script"](../../../docs/game/rules-engine.md#reloading-a-running-script).

## Pitfalls

- **The verifier rejects the unbounded.** No `while`, no recursion, no
  floats, no strings at run time, no backward jump. Functions are inlined;
  a block `fn` returns nothing, a value `fn f(a: T) -> R = expr` is one
  expression, and both see only their parameters, globals, constants and
  options. A `let` or parameter is not assigned again; options are read-only; a
  `timer` is a global only. A set (`players`, `teams`, `objectives`,
  a declared `objects` or `zones` label, `pick(set, n)`) appears only in
  `for each`, `count`, `first` and `pick`.
- **Determinism.** The same IR, options, seed and inputs give the same
  calls on every machine. Draw randomness only with `random(n)` or
  `pick(set, n)` (a SplitMix64 stream seeded from the match seed and the
  blob hash); `now()` is the tick; there is no clock. `int` saturates, and
  division by zero gives 0 with a warning.
- **Host-only.** Only the host runs the script. Guests fetch it only to
  show its name, option labels and messages, and a film stores the blob.
- **A script game rides an engine index.** A new mode carries `slayer`; a
  port carries its own engine (`ctf`, `oddball`, `king`, `juggernaut`,
  `territories`, `assault`), and a variant file's record must be that
  engine. Retail settings (score and time limits, lives, team play) stay in
  the record: read them as `variant.score_limit` and so on rather than
  re-implementing them. `score(p, n)` already adds the team score and
  tests the score limit.
- **Order.** `score`, `end_round`, `set_lives`, `respawn` and the other
  `Now` actions are made where the handler runs. The `Queued` ones
  (`create`, `delete`, `place_squad`, `hunt`, `erase_squad`, `forge_*`,
  `spawn_objective`, `reset_objective`, `ff_*`) are made in order at the
  next tick boundary, and a read never sees a queued call. `h2 rules caps`
  does not print the order; `crates/formats/rules/src/caps.rs` does.
- **Known runtime gaps.** A host refuses a script, by name, if it calls an
  entry that is not `named` or VM-only: `set_team`, `give`, `message`,
  `spawn_areas`, `reset_objective`, `role_traits`, `inside`, `ticks_held`,
  `label_number`, `zone_id`, `set_loadout`, `take`, `create`,
  `register_objective`, `spawn_objective`, `player_of`.
  `check` lists these as unhostable. The host runtime also, today, delivers
  only `init`, `kill`, `timer` and `tick`; reads only scores, kills,
  deaths, `alive`, `team_of`, `lives`, `round_time_left` and
  `is_objective`; makes only `score`, `end_round`, `set_lives` and
  `respawn`; reads only `variant.team_play`, `score_limit`, `time_limit`
  and `lives`; and refuses Forge labels and any `grant`. Calls land at the
  next tick boundary, and a mode 2 script on a lifted carrier does not
  replace the carrier's own scoring. The one script playable today is
  Slayer-shaped; everything else is proven by scenario. Name the entries
  your script waits on in a comment at the top, as the built-ins do.
- **`message(to, text)`.** `to` is a player, a team or `all`; `h2 rules
  caps` lists the three forms under the one name. The text may hold up
  to four `{name}` holes (a player, team, int, time or enum in scope;
  `{{` is a brace), passed through the VM-only `fmt_*` entries a script
  cannot name ([language.md](references/language.md)). `ticks_held(zone,
  team)` is the VM's count, and a host refuses it until it has a zone
  test. If the design page and the code disagree, follow the code and
  say so; `h2 rules caps` is the truth.
- **Syntax.** A statement ends at a newline, `;` or `}`. Keep a call's `(`
  and an index's `[` on the same line as the name. `else` may follow a `}`
  on the next line. Comments are `//`.
- **Tags** are string literals checked against the map when the game
  loads the script; `h2 rules check` does not find a typo in one.
  **Labels** are declared (`label goal: zone`, `label barrier: objects
  at least 1`), so a misspelt label is a check error with the nearest
  name offered, and a map that breaks a declaration does not start.
- **`T?`.** The killer of `on kill`, `first(...)` and `o.carrier` may be
  `none`: use them as a `player` only after `if killer != none`, in the
  `else` of `killer == none`, or through `if let k = killer`. A handle
  global that starts as `none` is declared `player?`. The checker finds
  every error (up to 50, each with a code, `rules::diagnose`); `h2 rules
  check` prints the first.

## Report what you did

Name the file, the hash from `build`, the budget line, the unhostable
entries, the scenarios you ran and what each showed, and where the variant
file or Live script is. If you did not ship it, say why.
