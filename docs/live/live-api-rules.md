# Live API: rules scripts and game variants

Status: contract. Split from [live-api-content.md](live-api-content.md); the server is `crates/live/live-server`, in both homes.

## Rules scripts

Game-type scripts for the rules engine
([rules-engine.md, "Workbench"](../game/rules-engine.md#workbench)). The
server compiles, verifies and runs them with the `rules` crate, in both
homes (`crates/live/live-server/src/rule_scripts.rs`, wire types in
`crates/live/live-proto/src/rules.rs`).

| endpoint | authentication | result |
|---|---|---|
| `POST /v1/rules/check` | none; rate-limited per client address | `{ok, hash, name, version, errors: [{line, col, message}], warnings?, report}` |
| `POST /v1/rules/run` | as check | a trace, or `{ok: false, errors}` when the script does not build |
| `POST /v1/rules/simulate` | as check | generated matches: `{ok, teams, summary, rows, match}`, or `{ok: false, errors}` when the script does not build |
| `POST /v1/rules/ide` | none; rate-limited per client address, in a bucket of its own | one editor operation's answer, in LSP's field names (below) |
| `GET /v1/rules/ide/legend` | none | `{legend: [...]}`, the semantic token types by index |
| `GET /v1/rules/builtin` | a read | `{items: [{name, engine, source, hash, example, key?}]}`: the engine ports (`rules::builtin::ALL`, `example: false`), then the example scripts (`rules::builtin::examples`, `example: true`, with their `key` and their carrier as `engine`) |
| `POST /v1/rules` | an account's token, `lxm: content.write` | 201 `{ok, saved: true, script}`; 200 `{saved: false, script}` when the hash is saved already |
| `GET /v1/rules/{hash}` | a read | `{hash, name, version, author, gamertag, saved_at, position, source}` |
| `GET /v1/rules/{hash}/ir` | a read | the IR blob (`application/octet-stream`, immutable), compiled again from the saved source |
| `GET /v1/rules?author=` | a read | `{items: [...]}`, the author's scripts newest first; `author` is a gamertag or xuid |

- Bodies are JSON with `source`, at most 256 KiB (413 over it). Check,
  run and simulate share a bucket of 20 tokens per client address,
  refilled at 2 a second (`rule_scripts::RATE`); an empty bucket is 429.
  A check, a run or one generated match costs 1 token; a batch costs 2,
  and 1 more for each further 25 matches (`sim_cost`).
- `ok` on a check means the script compiled, verified, fits the 100,000-op
  tick budget and the 256 KiB IR cap. `hash` is the IR's SHA-256, present
  whenever it compiled. `errors` lists every error the compile finds
  (`rules::diagnose`), in source order, not only the first. An error with
  line and column 0 is about the whole script: a verifier refusal, the
  budget or the IR size. `warnings` (`[{line, col, message}]`, omitted
  when there are none) are the compile's warnings; they leave `ok` alone.
- A check that is `ok` also lists the script's `options` (`[{name,
  label, description, kind, lo, hi, default, values?}]`, omitted when it
  declares none), for the game variant form ([Game
  variants](#game-variants)).
- A check that is `ok` also says what a variant of the script starts
  from: `scoring` (`script`, or `carrier` for a script with a `uses`
  line, whose built-in game scores), `about` (the script's description; omitted when it
  has none), `settings` (`[{name, label, value}]`, the `settings` block
  in table order, `value` as text: `15`, `Magnum`, `On`, `5 minutes`;
  omitted when empty) and `quick` (`[{kind, name, label}]`, the Quick
  Options marks in the script's order, `kind` `setting` or `option`;
  omitted when empty). Both homes serve the same fields from the same
  code (`game_variants::script_settings`, `quick_views`), and the parity
  suite runs the check on each (`tests/support/variant_scripts.rs`).
- `report` holds `worst_tick_ops` and `budget`, `fits`, `costliest`,
  `handlers: [{event, worst_ops}]`, `slots: [{scope, used, limit}]` (global,
  player, team, object), `ir_bytes` and `ir_limit`, `capabilities: [{name,
  words, grantable, known}]` with the lobby's plain words, and
  `unhostable`: the entries the script calls that no host can make yet on
  the script's carrier, `labels` and `minimal`. `labels` is what a map
  needs, one line for each label the script declares, in its table's order:
  `{text, kind: "object" | "zone" | "spawn", min, max?, single, numbered,
  range?: [lo, hi], per_team, needs}`, with `max` absent when the label
  states no limit and `needs` the requirement in Forge's words (`exactly 1
  zone labelled "goal"`). `minimal` is the smallest map that passes, as a
  scenario's `map` lists things (`{label, object, number?, team?, class}`,
  handles from 256); a page that merges it into a map gives the things
  handles of its own.
- A run's `scenario` is `{players: [{slot, team, lives, unit?}], teams,
  variant, map: [{label, object, number?, placement?, team?, class?}],
  options,
  seed, round_time, ticks, start, timeline: [{tick, events}]}`. `unit` is
  the object that is the player's unit, so `enter` and `leave` find its
  player; `map` (`labelled` is its older name) lists the things `objects
  labelled` visits, each with the `team` that `o.team` reads when it has
  one and a `class` (`object`, `flag`, `volume` or `spawn`; the one its
  label's kind suggests when absent). Without a `map` the run is played
  on the script's minimal map; with one, the script's labels check it
  first, and a run it fails answers `{ok: false, errors: [], map:
  ["the map has no zone labelled \"territory\"; Siege needs at least 1", ...]}`
  and runs nothing. A `variant` field left out reads the stock record of the script's
  carrier. An event is `{type, ...}` with the handler's event name:
  `kill {victim, killer}`, `spawn`, `death`, `join`, `quit {player}`,
  `pickup`, `drop`, `press`, `edit {player, object}`, `bounce {player,
  object, chain, monster}` (`chain` 1 and `monster` false when absent),
  `bounced {player}`, `enter`, `leave {zone, object}`, `squad_dead
  {squad}`, `actor_kill {killer, class}`, `actor_assist {killer, assist,
  class}`, `round_start`,
  `round_end`, `init`. A run steps the fake game `h2 rules run` steps
  (`rules::scenario::step`). `start` (default true) fires `init`
  and `round_start` at tick 0; `ticks` is the last tick run, at most
  36,000; a scenario lists at most 4,096 events. A bad scenario is 400.
- The trace is `{ok, hash, options, ticks, last_tick, error, round_ended,
  round_winner, limit, cut, ops, worst_tick_ops, handlers_ran, scores:
  [{slot, score}]}`. `limit` is `"score"` or `"time"` when a `score_limit` or
  `time_limit` the scenario's `variant` sets ended the game, and `round_winner` the
  team `end_round` named. Each entry of `ticks` is a tick in
  which something happened: `{tick, ran, calls: [{entry, args}], ops,
  warnings, globals: [{name, value}], players: [{slot, team, alive, score,
  lives, vars}], error}`. A run stops at a script error, after the tick
  that calls `end_round`, at a limit, or past 12,000,000 ops (`cut` says
  so), and keeps at most 2,000 ticks.
- A simulate body is `{source, players, teams, seed, matches, length,
  score_limit}`: `players` 1 to 16 (default 8), `teams` 0 to 8 (0 or 1
  is no team play; a carrier that needs teams gets them), `seed`
  (default 1), `matches` 1 to 100 (default 1), and optionally `length`
  in seconds (at most 1,200; absent, 300) and `score_limit`. An unknown
  field or a value out of range is 400. Match `i` of a batch plays seed
  `seed + i`; the same body gives the same answer
  (`rules::sim`; [rules-engine.md, "Workbench"](../game/rules-engine.md#workbench)).
- A simulate answer's `match` is the first seed's match in full:
  `{seed, end, why, winner, top_score, scenario, trace}`, with `scenario`
  in the run's shape (posting it to `/v1/rules/run` gives the same
  scores) and `trace` a run's trace holding the ticks in which an event
  fired. `end` is `script`, `score_limit`, `time_limit`, `error` or
  `cap` (the match never ended), and `winner` is `{kind, index}` with
  `kind` `player`, `team`, `draw` or `nobody`. `teams` is the team count
  played. With one match `summary` is null and `rows` empty. A batch's
  `rows` are `[{seed, end, winner, ticks, top_score, ops,
  worst_tick_ops}]` and its `summary` is `{asked, ran, wins: [{winner,
  matches}], ends: [{end, matches}], avg_score, max_score, avg_ticks,
  max_ticks, worst_tick_ops, worst_tick_seed, budget, errors: [{seed,
  message}], flags: [{kind, text, seed, matches}], stopped}`. A flag's
  `kind` is `never_ends`, `nobody_scores`, `negative_score`, `never_ran`
  or `clock`, and its `seed` a match that shows it. A batch starts no
  match past 24,000,000 ops or 400,000 ticks; `ran` is how many it
  played and `stopped` says why it played fewer.
- An editor request is `{source, op, line, character, end_line?,
  end_character?, new_name?}`: `line` (0-based) and `character` (UTF-16
  units) are the place, required by `complete`, `hover`, `signature`,
  `definition`, `references`, `actions` and `rename`. Each op answers
  from `rules::ide` (`ide_answer` in `rule_scripts.rs`;
  [rules-engine.md, "Tooling"](../game/rules-engine.md#security-and-usability)).
  A position is `{line, character}` and a range `{start, end}`. An
  unknown op, a missing place or an unknown field is 400; the source cap
  and the rate (20, refilled at 2 a second, in a bucket apart from check
  and run) are the workbench's.
  - `complete`: `[{label, kind, detail, documentation: {kind: "markdown",
    value}, insertText, insertTextFormat}]`, with LSP's
    `CompletionItemKind` numbers and `insertTextFormat` 1 (plain) or 2
    (snippet).
  - `hover`: `{contents: {kind: "markdown", value}, range}`, or `null`.
  - `signature`: `{signatures: [{label, documentation, parameters:
    [{label}]}], activeSignature, activeParameter}`, or `null` outside a
    call.
  - `definition` and `references`: `[{range}]`; references list the
    declaration first.
  - `tokens`: `{legend, data: [{line, start, length, type, typeName}]}`,
    absolute and in source order; `type` is the index in `legend`.
  - `symbols`: LSP's `DocumentSymbol`s, `[{name, detail, kind, range,
    selectionRange, children}]`.
  - `format`: `{text}`, or `{error, line, col}` for source that does not
    parse (`error` is `line:col: message`).
  - `diagnostics`: `[{range, severity, message, code, source, fixes?}]`,
    every one the source has. `code` is the checker's (`label.unknown`,
    `null.use`, ...; [rules-language-types.md, "Diagnostics and the
    editor"](../game/rules-language-types.md#diagnostics-and-the-editor)),
    or `syntax`, `limit`, `verify`, `unused` and `unhostable`. `fixes`
    (omitted when there are none) is `[{title, edits: [{range,
    newText}]}]`, each fix's edits in source order.
  - `actions`: the fixes of the diagnostics at a range, as LSP's quick
    fix code actions without the document: `[{title, kind: "quickfix",
    diagnostic, edits: [{range, newText}]}]`. The range runs from the
    place to `end_line` and `end_character`, both or neither (the place
    alone).
  - `rename`: `{edits: [{range, newText}]}`, the edits that rename the
    name at the place to `new_name` and every use of it. A label keeps
    its map text (`label finish: zone = "goal"`). A missing `new_name`, a
    name that is not an identifier or is a keyword, one the script
    declares already, or no name at the place is 400.
- A save writes a server-signed `RulesSaved` event (`{hash, name, version,
  author, source}`) on `rules-<hash>`, expecting an empty stream, so a hash
  is saved once and its first saver is its author. Only scripts whose
  check is `ok` are saved, so one over the tick budget or the IR cap is
  400; an account saves at most 200. Nobody posts the event
  by hand.

## Game variants

Game variants players publish: what a player's page lists under Game
Types. A game variant is a whole version-2 `.h2variant` file
([variants.md, "Portable files"](../editor/variants.md#portable-files)):
name, description, author, the 0x130-byte record and, when the record
names a rules script, the script's IR and source. Both homes serve these
routes (`crates/live/live-server/src/game_variants.rs`, wire types in
`crates/live/live-proto/src/game_variants.rs`).

| endpoint | authentication | result |
|---|---|---|
| `POST /v1/variants` | an account's token, `lxm: content.write` | 201 `Variant`; 200 when that file is published already |
| `POST /v1/variants/from-script` | as above | 201 `Variant`, built from a script |
| `POST /v1/variants/bundle` | none; rate-limited as `/v1/rules/check` | `{id, file_name, bundle}`: the file, unpublished |
| `GET /v1/variants/{id}` | a read | `Variant` |
| `GET /v1/variants?author=` | a read; a `read` bearer names the caller | `{items: [Variant]}`, newest first |
| `GET /v1/variants/{id}/h2variant` | a read | the file (`application/json`, an attachment named after the variant) |
| `DELETE /v1/variants/{id}` | `content.write`, its author or an admin | `{ok: true, id}` |

- `Variant` is `{id, name, description, author: {xuid, gamertag}, engine,
  game_type, record_hex, script_hash, script_name, visibility, created,
  bundle_url, settings: [{label, value}], eligible}`. `engine` is the record's
  engine index (including 6 for a script record) and `game_type` our label of it; `script_hash` and
  `script_name` are null for an unscripted variant; `created` is RFC 3339;
  `bundle_url` is the file's path; `settings` is the record's settings in
  words (each setting of its game type we have a name for, then the
  extension options in one row). `eligible` lists the kinds of game the
  variant may play in, by the script policy
  ([rules-engine.md, "Roles, powers and capabilities"](../game/rules-engine.md#roles-powers-and-capabilities)):
  `custom`, `hopper` and `ranked`, in that order, those its script's
  capabilities allow (a script with a `forge.*` capability is not
  `ranked`), every one for an unscripted variant. Publishing is open
  whatever it says.
- `POST /v1/variants` takes `{bundle, visibility}`: the file as an object
  or a string of it, version 1 or 2, and `public` or `unlisted` (default
  public). It also takes `{name, description, record_hex, script_hash?,
  visibility}`: `record_hex` is the record as 608 lowercase hex digits;
  if its block names a rules script (mode 2), `script_hash` (64 hex) is
  required, must start with the block's 8-byte prefix and must be a saved
  script ([Rules scripts](#rules-scripts)) or a built-in, and the server
  attaches that script's IR and source, keeping the record's option bits.
- The server writes the author into the file and runs `Bundle::check`:
  the record decodes and the game would keep it, the name and description
  keep the name rules (1 to 31 characters, at most 150 for a
  description), and a named script hashes to its hash, starts with the
  prefix, verifies and fits the tick budget and the IR cap (`rules::fits`).
  A retail record must match the script's template. A script attached to
  an index-6 record may write common settings and extension fields. A
  variant whose record runs no script
  (mode 0, or Firefight's mode 1) is refused by name: Live lists scripted
  variants only. A refusal is 400 with the reason. The file's id is the SHA-256 of its canonical bytes, author
  included, so the same file published again answers 200 with the same
  variant, and the file from `/bundle` (which has no author) gets another
  id once published.
- `from-script` and `bundle` take `{script_hash | source, name,
  description, visibility, options}`. The record is the script's own
  (`variants::portable::record_for`). When the script uses Slayer's
  template and writes no type-area setting, the record has index 6 and
  a zero type area. Other scripts keep their retail templates. The
  record carries the requested name, the script's settings, its mode-2
  reference and the options packed in declaration order. A `description` left out or blank is the script's
  `about`. `options` maps an option's name to a
  number, a bool or an enum value's name; one left out takes its default;
  an unknown name or a value out of range is 400. A `script_hash` is a
  saved script or a built-in; a `source` must verify. `POST
  /v1/rules/check` lists a script's options for this form (`options:
  [{name, label, description, kind, lo, hi, default, values?}]`, `kind`
  `int`, `bool` or `enum`).
- Lists show public variants, and unlisted ones only to their author (a
  `read` bearer). Any variant, unlisted too, is readable by id.
- A publish writes a server-signed `VariantPublished` event (`{id,
  author, visibility, bundle}`, the file as canonical JSON) on
  `variant-<id>`; a delete writes `VariantDeleted` (`{id, by}`) on the
  same stream, and the variant is gone from every route until it is
  published again. Each append expects the stream's next version. An
  account keeps at most 200 variants (409 past it). Nobody posts these
  events by hand.
