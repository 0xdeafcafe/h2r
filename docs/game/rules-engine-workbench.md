# Rules engine: the workbench

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: Studio and web tooling for writing and testing scripts.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## Workbench

**Code.** The web app's Game Types page (`statsPath("/rules")`,
`web/src/stats/Rules.tsx`, its test half in `web/src/stats/RulesTest.tsx`,
its pure half in `web/src/lib/rules.ts`) is where a script is written,
checked, tested and saved on the web (Studio's Game type script window
is the desktop editor, over the same `rules::ide` and workbench). The browser runs
no compiler: Live's [Rules scripts](../live/live-api-content.md#rules-scripts)
routes call `rules::workbench` in both homes.

The **Workbench** tab is one screen. The editor is on the left with the
check beside it (Problems, What a map needs, Budget, Capabilities); under them the test
(generated matches and their summary) sits beside one match's results.
Under 1100 px the parts stack in that order. The page's other tabs are **My Scripts** and **My
Game Types**. The layout, its states and its copy are described in
[web-app.md, "Game type scripts"](../live/web-app-pages.md#game-type-scripts).

- **Check.** `workbench::check` compiles, verifies and measures source:
  the IR hash, errors with line and column (0 for a verifier refusal or
  the budget), the worst-tick estimate against the 100,000-op budget, each
  handler's worst case, slots per scope against their limits, IR bytes
  against 256 KiB, the declared capabilities in the lobby's words, and
  every entry the script calls that no host can make on its carrier. The
  page checks 450 ms after typing stops and has no Check button; each
  error and warning with a place is a marker in the editor, and its place in the
  Problems list selects it. The check's `labels` show as **What a map
  needs** ([rules-language-types.md, "The workbench"](rules-language-types.md#the-workbench)). A
  check the server refuses (a source over 256 KiB) is listed as a problem
  of the whole script.
- **Editor.** The script box is Monaco (`web/src/stats/RulesEditor.tsx`),
  loaded by a dynamic import on this page only, so it is a chunk of its
  own: the core editor (`monaco-editor/esm/vs/editor/edcore.main`, no
  bundled languages) and its one editor worker. `stats/rulesMonaco.ts`
  registers the `h2rules` language: a Monarch grammar from the lexer's
  tokens and the parser's words (comments, keywords, `const` and its
  name, `match`, `on` and its event, scoped `var`, `for each`, `team n`,
  `text n`, the type words, a label's kinds and
  requirement words, time literals such as `30s`, `1.5s` and `300t`, and
  the other literals), brackets,
  auto-closing pairs, indentation and folding, and a dark theme from the
  stats site's colours. Completion, hover, signature help, go to
  definition (F12), find references (Shift+F12), the outline
  (Ctrl/Cmd+Shift+O), semantic tokens, Format Document (Shift+Alt+F),
  quick fixes (Ctrl/Cmd+., the `actions` op over the selection) and
  rename (F2, the `rename` op; a refused name shows the server's reason)
  ask Live's `POST /v1/rules/ide` (`{source, op, line, character}`,
  0-based line, UTF-16 character, with `end_line` and `end_character`
  for `actions` and `new_name` for `rename`; LSP field names back) and
  `GET /v1/rules/ide/legend`; each op waits for typing to pause, a newer
  request cancels the older one, and answers are kept per model version
  (a rename is never kept; `web/src/lib/rulesIde.ts`, the pure half). A built-in opens read-only
  until **Use as start**. The **Insert** buttons above the editor open a
  picker each (Declarations, with its Labels and "In the test map"
  groups; Events; Actions and reads by capability in
  the lobby's words; Control) whose rows insert a snippet at the caret;
  the events, the API and the built-in flag labels come from the
  completion route at a blank top-level line, a blank statement and the
  name of a new `label` line, over a list the page keeps.
- **The map.** A scenario's `map` (`scenario::ScenarioLabel`) lists
  things: `label` (text), `object` (the handle), `number?`, `placement?`,
  `team?` and `class?`, one of `object`, `flag`, `volume` or `spawn`.
  A thing with no `class` takes the one its label's kind suggests in the
  script's label table: a volume for a zone label, a starting location
  for a spawn label, an object for any other label and for a label the
  script does not declare. A scenario with no `map` runs on
  `labels::minimal(ir, teams)`: things numbered from `scenario::MAP_BASE`
  (0x100) in label order, with `teams` the scenario's, or 0 and 1. A
  scenario with a `map` is checked first, by `scenario::resolve_map`
  (`labels::check` over `scenario::census_of`); a problem is
  `RunError::Map` with the check's text, and nothing runs. An empty `map`
  is a map with nothing on it, not an absent one. See
  [rules-start-gate.md](rules-start-gate.md#the-check) for the texts.
  The page edits a hand match's map under its players and offers **Add
  what the script needs** ([rules-language-types.md, "The
  workbench"](rules-language-types.md#the-workbench)).
- **Run.** `workbench::run` steps a scenario (seated players with their
  units, variant fields, the map, options, seed, round time,
  and a list of events by tick) over `fake::FakeGame`, through the same
  `scenario::step` that `h2 rules run` takes. Each tick: the queue is
  flushed, the tick's events fire in order, then `update` runs timers
  and `on tick`. A kill marks
  its victim dead and counts a kill and a death, a spawn marks the player
  alive unless the engine holds it (the variant's `lives` positive and
  none left), a join seats one and a quit removes one after its handler. The
  trace keeps the ticks in which a handler other than `on tick` ran, a
  call was made, a variable or player row changed (a timer counting
  down is no change), a warning was raised or the script stopped. The run stops at a script error, after the tick
  that calls `end_round`, or past 12,000,000 ops, and the same input gives
  the same trace. The page shows a trace as final standings and a
  timeline (`rules.ts`, `timeline` and `standings`): each tick the
  scenario put an event in or the trace kept, with the event as a
  sentence, the handlers that ran, their calls, and each score, team,
  lives count or variable that changed.
- **The fake world.** `workbench::Runner` is the same run one tick at a
  time, each through `scenario::step`. The shared game model
  (`FakeGame::apply`) also keeps what a script reads back about
  objectives: a `pickup` or `drop` of a registered objective sets or
  clears its carrier, and a kill, death or quit takes the player out of
  every zone and drops what they carried. A labelled object may name
  its `team` (`ScenarioLabel::team`), which `o.team` reads. After each
  tick the Runner makes the objectives the script asked for:
  `spawn_objective` makes one numbered from `SPAWNED_BASE` (0x4000) with
  its team, and `reset_objective` clears its carrier; `h2 rules run`
  does not, so a scenario whose script spawns objectives plays in full
  only through the workbench. The limits a scenario's `variant` sets
  end the run: `score_limit` (a player's score, or a team's sum with
  `team_play`) and `time_limit` in seconds; the trace's `limit` says
  which. A limit the scenario leaves to the stock record ends
  nothing. `round_winner` is the team `end_round` named, `worst_tick_ops`
  the costliest tick and `handlers_ran` every handler that ran.
- **Generated matches.** `sim::simulate` makes a match up from the
  script and a setup (players 1 to 16, teams, a seed, a length in
  seconds or the carrier's time limit, a score limit) and plays it on a
  `Runner` as it goes, so what happens next follows what the script did.
  It reads the IR for what the script handles and calls, and makes only
  those events: kills at a steady rate with a few suicides, betrayals
  and deaths to the world, respawns after five seconds unless the script
  respawns players itself, players walking into and out of the labelled
  objects the script names, objectives picked up and dropped, and
  `bounce`, `edit`, `press`, `actor_kill`, `actor_assist` and
  `squad_dead` only for a script with that handler. The objects it
  walks into are the things of the script's minimal map
  (`labels::minimal` for the match's teams, a free-for-all match reading
  teams 0 and 1), apart from starting locations: each label has its
  count, its numbers and a member for each team it asks for, so a
  generated match is never a blocked one. The seed fixes the
  whole match. The match stops when the script ends the round, a limit
  is reached, the script fails, or 4,096 events or 36,000 ticks pass.
  Its output is a `workbench::Scenario` and the trace of running it;
  `workbench::run` on that scenario gives the same scores.
  `sim::batch` plays up to 100 seeds in a row, starting no match past
  24,000,000 ops or 400,000 ticks, and sums them up: who won how often,
  how the matches ended, the average and best top score, the costliest
  tick against the budget with its seed, each script error with its
  seed, and flags with a seed that shows each (the game never ends,
  nobody scores, a score below zero, a handler that never ran, every
  match running out the clock). `sim::to_file` turns a match into the
  CLI's scenario file, whole. A generated match seats each player with
  the unit `UNIT_BASE` (0x1000) plus the slot; a player who quits and
  joins again has none and stays out of zones. The page plays 25 matches by itself, 350 ms
  after the checked script's hash or the setup changes; a match opened
  with **Edit this match** runs through `workbench::run` when it
  changes.
- **Editor.** `POST /v1/rules/ide` answers one editor request (complete,
  hover, signature, definition, references, tokens, symbols, format or
  diagnostics) from `rules::ide`, in LSP's field names, so the page's
  editor needs no language server of its own ("Language intelligence"
  under "Tooling").
- **Save.** A signed-in account saves a script that checks clean (it
  verifies and fits the tick budget and the IR cap); Live keeps its
  source under the IR hash and compiles it again to serve the blob.
- **Game variants.** **Make a game variant** turns the current script
  into a game variant: a name, a description, a visibility and a value
  for each option the script declares (the check report lists them),
  over the stock record of the script's type
  (`hopper::variant::VariantRecord::stock`). The record's block gets mode
  2, the IR's hash prefix and the options packed in declaration order
  (`game_variants::pack_options`). **Download** asks Live's
  `POST /v1/variants/bundle` for the `.h2variant` file and works signed
  out; **Publish** posts it to the account's game variants
  ([live-api.md](../live/live-api-content.md#game-variants)). The **My Game
  Types** tab lists them beside My Scripts, and a player's page lists
  their public ones; each has a page (`statsPath("/variants/:id")`) with
  its settings, its script (opened read-only here) and the file.
- **Drafts.** The page keeps the source, the generator's setup and the
  match edited by hand in `localStorage`, and starts from a blank starter, an engine port
  (`builtin::ALL`), an example (`builtin::examples`, marked "example")
  or one of the player's saved scripts. `GET /v1/rules/builtin` lists
  the ports and then the examples.
