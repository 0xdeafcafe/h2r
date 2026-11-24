---
name: h2-rules
description: Use when writing, checking, testing or shipping a game type script in the rules language, a `.rules` file, a rules scenario, a game variant or a `.h2variant` file. Covers h2 rules check/build/run/sim/caps/builtin/lsp/editors, the VS Code and Zed extensions, h2 variants port, capabilities and requires, the tick budget, Studio's script editor and reloading a script into a running game (rules.reload), and Live's /v1/rules and /v1/variants routes.
---

# Write, test and ship a game type script

A game type is a `.rules` script in our own small language. It compiles
to a verified IR blob whose SHA-256 names it. Its `type` line names its
game type (the list row it is filed in and its start line); a `uses` line
plays a built-in game's rules under the script's settings. You write it, check it, run it over a scenario
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
   race, bounce_hunt). `h2 rules builtin slayer` prints one, by name
   (any case), engine index or example key.
2. **Write the `.rules` file**: a header (`rules "Name" 1`, `type
   slayer`, optional `teams n`, `points "Label"`, `uses <game>` and
   `requires ...`), options, variables, functions, handlers. The type is
   one of `slayer ctf assault oddball koth juggernaut territories research
   party firefight siege`; a new mode picks the row it belongs in.
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
x.rules: "Count" version 1, type slayer, points "Kills", engine 2, 2 teams
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
`ai.squads`, `vehicles.drive`, `hud.message`, `firefight.director`, and the four
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
  need the match edit path, which is not built. The VIP with powers is a
  test script (`tests/scripts.rs`); the listed VIP has none.

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
  `leave` naming it move the player in and out of a zone), `body`
  (`{"airborne": true, "crouching": false, "primed": true}`: what
  `p.airborne`, `p.crouching` and `p.primed` read; a `touchdown` lands
  the player). `sight` is a list of `[from, to]` object pairs with
  nothing solid between them, the only pairs `sees` answers true for. `teams`
  defaults to `[0, 1]`.
- `variant`: record fields by name (`{"team_play": 1, "score_limit":
  25}`); a field left out reads the stock record of the script's engine
  (Slayer's is free-for-all, score limit 25, time limit 480).
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
  left out), `{"bounced": 0}`, `{"touchdown": {"player": 0, "down":
  589824, "up": 786432}}` (raw `fixed`, world units a second),
  `{"launch": {"player": 0, "spot": 77, "speed": 786432}}` (`spot` `none`
  when left out), `{"grounded": 0}`, `{"edit": {"player": 0, "object": 7}}`, `{"squad_dead": 2}`,
  `{"actor_kill": {"killer": 0, "class": "elite"}}`, `{"actor_assist":
  {"killer": 0, "assist": 1, "class": "elite"}}`, `"round_end"`.
- A kill marks the victim dead and counts its death and the killer's
  kill; a spawn marks the player alive, unless the engine holds it (the
  variant's `lives` positive and none left, when only the script's
  `respawn` brings the player back); a quit removes the player. The
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
  script's (`h2 rules check` prints it), or the port fails with `the
  record is engine N; the script's record is engine M`.
- `--name NAME` and `--description TEXT` replace the record's name and the
  input file's description (one input only). `--option key=value`, once
  per option, sets a script option by name: an int (whole seconds for a
  `time` option), `true` or `false`, or an enum value's name. An option the script does not declare, a value
  out of range is refused. Options
  left out take their defaults. A script over the budget is refused.
- `--script builtin` (the default) attaches the record's engine's
  built-in; `x.rules` compiles, verifies and carries the source; `x.rir`
  carries the blob alone. Every port names a script, and the script's
  `settings` are the only `hx` extras the record carries.
- It prints `<id>  <path>` per file; the id is the SHA-256 of the file's
  canonical bytes, and the same input and script give the same id. Exit is
  nonzero if any input failed.
- With no record at hand, use Live's `/v1/variants/bundle`, which starts
  from the script's engine's stock record.
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
| `GET /v1/rules/builtin` | read | `{items: [{name, engine, source, hash, example, key?}]}`: the engine ports, then the examples (`example: true`, `engine` their record's engine) |
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
hover and the check report. **Open in VS Code** and **Open in Zed** save it
and open the file there at the cursor; Studio's own edits go to the file a
second after typing stops. The extension's **H2: Reload Script in Game** and
**H2: Run Scenario Test...** call Studio over its socket (`rules.reload
{path}`, `rules.run`). **Open in editor** opens the file
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
h2 studio rules.run '{"scenario":"case.json"}'    # the open script over a scenario, as h2 rules run prints it
h2 studio rules.external '{"editor":"code","launch":false}'  # its file and the command that opens it
```

Studio sends the verified blob to the paired game's `rules.reload`. A
running game can also take one straight from the game's agent API:

```sh
h2 agent rules.reload '{"source":"<the script text>"}'
h2 agent rules.state     # last_reload: mode, hash, tick, reasons
```

- **In place** when the new script keeps the old one's state shape: the
  same type, `uses`, teams, capabilities, options (kind, range, default),
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
- **A script names a type, not an engine.** `type` files the variant and
  picks its start line; `points` names what a point is. A retail type's
  record is that game's engine (`ctf`, `assault`, `oddball`, `koth`,
  `juggernaut`, `territories`); `research`, `party`, `firefight` and
  `siege` are on Slayer's, and a variant file's record must be that
  engine. `carrier`, `list` and `scoring` lines are refused
  (`header.retired`), each naming its replacement
  ([docs/game/game-types-without-carriers.md](../../../docs/game/game-types-without-carriers.md)). Retail settings (score and time limits, lives, team play) stay in
  the record: read them as `variant.score_limit` and so on rather than
  re-implementing them. `score(p, n)` already adds the team score and
  tests the score limit.
- **Order.** `score`, `end_round`, `set_lives`, `respawn` and the other
  `Now` actions are made where the handler runs. The `Queued` ones
  (`create`, `delete`, `place_squad`, `hunt`, `erase_squad`, `forge_*`,
  `spawn_objective`, `reset_objective`, `ff_*`, `bot_*`) are made in order at the
  next tick boundary, and a read never sees a queued call. `h2 rules caps`
  does not print the order; `crates/formats/rules/src/caps.rs` does.
- **Known runtime gaps.** A host refuses a script, by name, if it calls an
  entry that is not `named` or VM-only: `set_team`, `give`,
  `take`, `create`, `register_objective`,
  `spawn_objective`, and the physics reads `airborne`, `crouching`,
  `primed` and `sees` (their game side is not built). `on touchdown`,
  `on launch` and `on grounded` come from the superbounce mode's medal
  chain, so a host delivers them only to a script that `requires
  hud.superbounce`. `push` and
  `launch` have no entry until R9. `check` lists these as unhostable. The host runtime
  also delivers only `init`, `round_start`, `kill`, `quit`, `timer`,
  `tick`, `bounce`, `bounced`, `touchdown`, `launch`, `grounded`, `spawn`,
  `death`, `enter`, `leave`, `arrive` and `damage`;
  reads only scores, kills, deaths, `alive`, `team_of`, `lives`,
  `round_time_left`, `is_objective`, `carrier_of`, the label reads,
  `inside`, `player_of`, `object_pos` and `object_vel`; makes only `score`, `end_round`, `set_lives`,
  `respawn`, `role_traits`, `reset_objective`, `sound_to`, `sound_to_team` and `sound_to_all` (a `sound` of the map, played at the audience's units), `set_loadout` (the
  multiplayer weapon list's tags only), `spawn_areas` and `spawn_near`
  (a team's spawn rule, read at each spawn), `set_body` (a team's body,
  Spartan or Elite, at each spawn), `show_area` and `hide_area` (a zone's
  boundary drawn in play, at most eight), `teleport` (an object put at a
  `vec` or at another object's place; a position outside the world is a
  refused call), `move_to` and `stop` (a mover of at most 16, stepped each
  tick, with `on arrive(o)` on its end tick; `face`, `move_along`, `attach`
  and a player's `teleport` are refused by name) and the `bot_*` orders
  (a `bot_*` order to a seat no phantom holds is a refused call), and
  `message`, `message_team` and `message_all`, which it shows to the host's
  own players and sends to clients as a show cue
  (docs/game/rules-world-api.md, "The show channel"); reads only
  `variant.team_play`, `score_limit`, `time_limit`
  and `lives`; and refuses Forge labels and any `grant`. A zone is a
  labelled netgame flag (`game::rules::world::zones`). Calls land at the
  next tick boundary, so a score read back in the same handler is the
  old one; a script that ends on a total counts it in a variable. While
  a mode 2 script scores, the engine's own scoring is answered in place
  (docs/game/rules-engine.md, "One scorer"). The listed scripted modes
  (Superbounce, Siege, Gun Game, Race, VIP) run on a host; the rest is
  proven by scenario. Name the entries
  your script waits on in a comment at the top, as the built-ins do.
- **Vehicles and drop ships.** `label run_in: path` is a flight: at least
  two placements numbered from 0, none missing and none twice (a gap
  blocks the start, naming the number). `fly(ship, run_in, 12s)` flies it
  in the stated ticks facing its tangent; `seat`, `eject`, `unload`,
  `place_squad_in(s, class, n, ship)` and `vehicle_riders` and
  `vehicle_driver` load and read it. All of them run on the fake game and
  a host refuses a script that calls one, by name, until the research in
  docs/game/rules-world-api.md ("Vehicles and drop ships") names them.
  A vehicle entry on an object that is no vehicle is a refused call.
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
