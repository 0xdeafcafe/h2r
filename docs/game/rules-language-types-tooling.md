# Rules language types: diagnostics, workbench and IR

Diagnostics and the editor, the workbench, the IR and the implementation plan. Part of [rules-language-types.md](rules-language-types.md).

## Diagnostics and the editor

Every diagnostic has a stable `code`, a range, a message and, where one
exists, fixes: text edits the editor applies. `ide::Diagnostic` carries
the checker's `code` and its `fixes` (`ide/fix.rs`); `ide::actions`
returns the fixes at a range (LSP `textDocument/codeAction`), and
`ide::rename` renames a declaration and its uses. `h2 rules lsp`
(`crates/apps/h2-tools/src/rules_lsp.rs`) answers both, and Live's
`POST /v1/rules/ide` has them as the `actions` and `rename` operations.
The web workbench's editor (**Code**, `web/src/stats/rulesMonaco.ts`)
asks `actions` over the selection for Monaco's quick fixes (Ctrl/Cmd+.,
the first preferred) and `rename` for F2, which shows a refused name's
reason; the edits are the route's (`applyEdits` in
`web/src/lib/rulesIde.ts` applies them the same way). The check's
warnings are markers beside its errors and are listed under them.

The editor, the workbench's check and `h2 rules check` list every
diagnostic `rules::diagnose` gives, errors and warnings, not only the
first error. `rules::ide` reads the types from the checker's `Checked`
script; `ide/model.rs` resolves names and works out no type of its own.
A fix is offered only where its edit is plain: `Wrap in for each` only
for a statement on one line, `Add numbered` only when the label is not
numbered.

| Code | Message | Fixes |
|---|---|---|
| `label.unknown` | `no label named barier. Did you mean barrier?` | Change to `barrier`; Declare label `barier` |
| `label.set-as-one` | `barrier is a set of objects, and delete takes one. Loop with "for each barrier b", or take one with first(barrier) or barrier[n].` | Wrap in `for each` |
| `label.one-as-set` | `goal is one zone, not a set. Use it directly, or declare it as zones.` | Change the declaration to `zones` |
| `label.kind` | `core is an object, and inside takes a zone. If the map's core is a volume, declare it "label core: zone".` | Change the declaration to `zone` |
| `label.spawns` | `attack names spawn areas, which have no objects to visit. Pass it to spawn_areas.`; `run_in names a flight path, whose points are not objects to visit. Pass it to fly.` | none |
| `label.index` | `barrier is not numbered, so barrier[n] has no meaning. Add "numbered" to its declaration.` | Add `numbered` |
| `label.same-text` | `capture and zone both name the map label "zone". Keep one.` | none |
| `label.unused` (warning) | `label core is declared but never used; a map would still need one.` | Remove the declaration |
| `null.use`, `null.arg` | "A handle that may be none" | Wrap in `if let`; Add `!= none and` |
| `null.var` | `a player variable starts as none, so its type is player?.` | Add `?` |
| `null.never` (warning) | `p is never none here.` | Remove the check |
| `unit.mix`, `unit.literal` | "Time"; `0.01s is not a whole number of ticks (30 a second).` | Add `s`; Wrap in `seconds(...)` |
| `tag.group` | "Zones and weapons" | Change the column to `weapon` |
| `body.unknown` | `objects/characters/elite/elite is not a player body; set_body takes objects/characters/masterchief/masterchief_mp or objects/characters/elite/elite_mp` | none |
| `field.path` | `"Vitals Grade" is not a field name: lower-case letters, digits and underscores, starting with a letter` | none |
| `zone.delete` (warning) | `goal is a zone. A flag cannot be deleted, so on a map where goal is a flag this does nothing.` | none |
| `match.missing` | "Enums and match" | Add the missing arms |
| `const.range`, `const.zero` | "Constants" | none |
| `text.hole` | "Messages" | none |
| `cap.missing`, `cap.unused` (warning) | "Capabilities" | Add to `requires`; Remove from `requires` |
| `event.args`, `event.label` | `on enter gives 2 arguments (zone, object); this handler names 3.` `core is an object; on enter takes a zone label.` | none |
| `setting.unknown` | `no setting named score_limt. Did you mean score_limit?` | Change to `score_limit` |
| `setting.type` | `juggernaut_movement is a Juggernaut setting; this script's record is Slayer's` | Remove the setting (or the mark) |
| `setting.twice` | `lives is set twice; it is first set on line 5` | Remove the repeated setting |
| `setting.value` | `time_limit is a time: write 300s for 300 seconds. time_limit takes a time in whole seconds, 0s to 32767s, or none.` | none |
| `quick.many` | `Quick Options has five rows; this line names 6` | Drop the marks past the fifth |
| `quick.unknown` | `bounce_points is no setting and no option of this script` | Change to the nearest name; Remove the mark |
| `quick.twice` | `lives is marked twice` | Remove the repeated mark |
| `option.setting` | `option lives has a setting's name; an option needs a name of its own` | none |
| `uses.handlers` | `on kill has no place here: with uses slayer the built-in game scores, and the script holds its header lines, settings and quick only` | Remove `uses slayer` |
| `scoring.none` | ``this script scores nothing. Write `uses slayer` to play Slayer's rules.`` | Add `uses <T>` |
| `header.retired` | ``list is gone: write `type party`, which files the game in its row`` (and the like for `carrier` and `scoring`) | Replace the line with the one it names |

The `setting.*`, `quick.*`, `option.setting`, `uses.*` and
`scoring.*` codes check the settings block, the `quick` line and the
`uses` line against `rules::settings::TABLE`
([rules-engine.md](rules-engine.md#the-language)). A settings value is
not a front-end type: each row's kind (bool, int, seconds, choice,
percent) says what it takes, and the checker folds it as it folds a
constant.

**Hover** shows a type on every name: `lead: player?`. Where the
checker narrows it, a second line says so:
``Here `lead: player` (not none here: checked on line 41).``
A label's hover is its declaration, then what the script sees, what a
map needs in Forge's words, and its uses:

```text
label capture: zones numbered 0..1 = "zone"
A set of zones. Members have number, team, id, and this script's object
variables.
A map needs: trigger volumes or flags labelled "zone", numbered 0 and 1,
at least one of each.
Used 2 times.
```

An API entry's hover gives the front-end signature
(`inside(player, zone) -> bool`, from `lang::sig`).

**Completion** works out the type wanted at the caret from the call's
front-end signature (`lang::sig`) or the user function's parameter, and
ranks by it; nothing is hidden. In
`inside(p, |)`: zone labels, then locals and members of type `zone`,
then `zone?` values (their detail says "may be none"), then set labels
with `[`, then the rest. After `for each`, `count(`, `first(` and
`pick(`: the built-in sets and the set labels. After `is`: labels. After
`on enter(`: zone labels. After `label <name>:`: the kinds; after a
kind: the requirements. At the name of a new `label` line: the built-in
flag labels as whole declarations (`label territory: zones`,
`label hill_corner: zones numbered`), so a port never types their text.

**Semantic tokens.** A label's name is the `label` token type
(`ide::LEGEND`, index 13) at its declaration and at every use; the
legend does not change. `zone`, `time`, `weapon` and the set words are
`type`.

**Definition and references** go to and from the `label` line.
**Rename** on a label keeps the map text: renaming `goal` to `finish`
writes `label finish: zone = "goal"`.

## The workbench

The scenario declares its map's labelled things, and the page checks it
with the start gate's check.

- **Map.** **Code**, `scenario.rs`. A scenario's `map`
  (`scenario::ScenarioLabel`) lists things, each with `object` (its
  handle), `label`, `number?`, `placement?`, `team?` and `class?`, one of
  `object`, `flag`, `volume`, `spawn`; with no `class`, the one its
  label's kind in the script suggests. `scenario::census_of` turns the
  list into a `labels::Census`; `scenario::resolve_map` runs
  `labels::check` over it for the scenario's teams, and `FakeGame`
  receives the things (`labelled`, `numbers`, `object_teams`) only if it
  passes. A problem is `RunError::Map` with the check's text, and
  nothing runs: `workbench::run` and `Runner::new`, `scenario::run`
  (`h2 rules run --scenario`) and `POST /v1/rules/run` (`{ok: false,
  errors: [], map: [text]}`) all give it. A scenario with no `map` takes
  `labels::minimal(ir, teams)` as things from `scenario::MAP_BASE`.
  `labelled` is the older name of `map` in a file and a request.
- **Generated matches.** **Code**, `sim.rs`. `labels::minimal(ir, teams)`
  gives the smallest census the check accepts: one of each single label,
  the minimum of each set, each number, one per team. The match
  generator (`sim::simulate`, `POST /v1/rules/simulate`) uses it as the
  map of a match it invents (`sim::objects`), with a member for each
  team in play added to each label it leaves empty, so a generated match is never a blocked
  one and an Oddball match has its ball; its players walk into every thing but a starting
  location. The match's scenario carries the map, so a saved match runs
  again.
- **The page.** **Code**, `web/src/stats/RulesTest.tsx` (`MapEditor`
  in the hand editor) over `web/src/lib/rules.ts`. A hand-edited match
  with no map of its own plays on the script's minimal map, and **Make
  it my map** copies that map in to edit. The map's things are listed
  under the players, grouped by label (`mapGroups`), each with its
  number, team and class (blank: the one its label suggests,
  `suggestedClass`); **Add thing** adds one under a label with the next
  free number (`newThing`), and **Use the smallest map** drops the map
  again. Each requirement the map misses is a problem: the run's `map`
  texts once it answers, and before that the page's own reading of the
  same check (`mapShort`, for teams 0 and 1, the fake game's). **Add
  what the script needs** merges the report's `minimal` things into the
  map (`withNeeds`): a minimal thing the map has already (same label,
  number and team, of a class the label takes) is kept, the rest are
  added with handles past the map's own, and a per-team label gets a
  thing for each team in play. A run the map fails shows the problems
  in the match's results and nothing ran. An event's zone or object is
  picked by label and number ("capture 0", "base (team 1)", "barrier
  #2"; `thingName`, `thingChoices`), with the seated players' own
  objects for an `enter` or `leave`; with no map of its own the names
  are the minimal map's. The draft keeps the map, an empty one too.
- **The check report** (`workbench::CheckReport`, **Code**) has `labels`
  (`workbench::LabelLine`): each declared label's text, kind, `min`,
  `max` (absent when unbounded), `single`, `numbered`, number range,
  `per_team` and `needs`, its sentence in Forge's words
  (`labels::describe`), read from the IR's label table; and `minimal`,
  the smallest passing map as scenario things. It also has `settings`
  (`workbench::SettingLine`: the table's name, label and kind, the
  stored value and its words, in table order) and `quick`
  (`workbench::QuickLine`: each Quick Options row's kind, name, label and
  starting value, in the script's order). The CLI's `h2 rules
  check` prints the sentences under "a map needs". The workbench page
  shows it as "What a map needs" beside the problems (**Code**,
  `MapNeeds`): each label's text, its kind in words and its sentence.
  The variant page is to show it too (**Design**).
- **Insert.** **Code**, `web/src/lib/rulesIde.ts`. The Declarations
  menu has a Labels group (`LABEL_ITEMS`): one zone, one object, a set
  of objects, a numbered set of zones, spawn areas, and the built-in
  flag labels as whole declarations, from the completion route at the
  name of a new `label` line (`MENU_PROBES`, `flagLabelItems`) or the
  page's copy of them (`FLAG_LABELS`). A third group, "In the test
  map", offers a declaration for each label of the test map the script
  has not declared (`mapLabelItems`): the kind the things' class
  suggests, a set when there are several, `numbered` when they all have
  numbers, and the map text when it is not a name.

## The IR

**Code**, `crates/formats/rules/src/ir.rs`, `verify.rs`, `vm.rs`.
Types are erased. The IR's ops, register types and type codes do not
change. Two tables change in place, and `ir::IR_VERSION` and
`caps::API_VERSION` stay as they are: a blob is the current form or it
does not verify.

| Table | Holds, in encoding order |
|---|---|
| label (`LabelDef`) | text (a string); kind, a byte (`LabelKind`: 1 object, 2 zone, 3 spawn, 4 path); `min` and `max` members, two `u16`s, `max` at most 256 (`ty::MAX_LABELLED`, which states no limit); flags, a byte (`ir::label_flags`: 1 single, 2 numbered, 4 number range, 8 per team); the number range `lo` and `hi`, two `i32`s, inclusive, both 0 without the range flag |
| tag (`TagDef`) | path (a string); group, a byte (`TagGroup`: 0 any object, 1 weapon, 2 sound) |

A single label is `max` 1 with the single flag and no other, and `min` 0
when it is optional (`object?`). Decoding refuses an unknown kind or
group.

- `Ir.labels` is a list of `LabelDef`, at most 64 (`ir::MAX_LABELS`).
  The verifier (`verify::labels`) refuses unknown flags,
  `min > max` or `max > 256`, a single label with another requirement,
  a spawn label that is single or per team, a number range without
  numbered, unordered, of more than 256 numbers or of more numbers than
  `max`, a range without its flag, label text outside `ir::label_char`
  (`forge_doc::label_char`'s set), and one text twice; and
  `Fault::SpawnSet` for a `ForEach` over a `spawn` label.
  `SetBase::bound` is the label's `max`, which `verify::cost` charges
  in the worst-tick estimate, and `Vm::members` takes at most `max`, so
  the bound holds on any map, checked or not.
- `lower.rs` writes each label from its declaration (`check::Label`):
  kind, `min`, `max`, the numbered and per-team flags, and its map text.
  A tag path used as a weapon is `TagGroup::Weapon`; every other is
  `TagDef::any`. The checker refuses a label outside `label_char` and a
  65th label.
- A front end that writes IR directly declares its labels the same way:
  Firefight's `ZONE_LABEL` (`crates/formats/firefight/src/script.rs`) is
  kind object.
- `caps::ENTRIES` holds the `fmt_*` entries, numbers 60 to 64
  ("Messages"); the entry table changes in nothing else.
- A changed table changes every blob's bytes and hash. The built-ins
  compile at first use (`rules::builtin`), so theirs follow the
  compiler; a variant or a saved script is made again from its source.

## The built-ins, rewritten

The 12 scripts in `content/core/rules/` and `content/modes/rules/`, the examples in
rules-engine.md and the h2-rules skill, and the scripts in
`crates/formats/rules/tests` are rewritten by hand in the slice that
changes the form they use (slices 2 and 5). Their tests keep the calls
and scores they expect, and the map texts they place. Siege's labels
and phases (`siege.rules`):

```text
const last_phase = 2
label barrier: objects numbered
label attack: spawns numbered 0..last_phase
label defend: spawns numbered 0..last_phase
label capture: zones numbered 0..1 = "zone"
label core: object
label goal: zone

for each barrier b where b.number < n { delete(b) }
spawn_areas(team 0, attack, n)
if n == last_phase { create("<core tag path>", core) }
if let zone = capture[phase] and ticks_held(zone, team 0) >= capture_time { open(phase + 1) }
on enter(goal, o) {
    if phase == last_phase and is_objective(o) { end_round(team 0) }
}
```

- `ctf.rules` serves CTF and Assault, so its four sets require no
  members and a map needs only its own game's flags:
  `label flag_spawn: zones at most 16 = "ctf_flag_spawn"`, and the same
  for `flag_return`, `bomb_spawn` and `bomb_plate`. `at most 16` is a
  map requirement and the loop bound: three lookups an objective a tick
  at the default 256 would not fit the tick budget. A team's spawn is
  `flag_spawn[mine]` (a `zone?`), a neutral bomb's
  `first(bomb_spawn s where s.team == none)`, and
  `if let f = o.team { f.at_home = true }` replaces the write through
  `none`. `claim_spawn`, `claim_plate` and the `team var`s they fill go.
- `territories.rules` declares `label territory: zones 1..8`, an
  `object var owner: player?` and a `time` progress, and loops
  `for each objectives m where m is territory` with the leader bound by
  `if let`.
- `koth.rules` declares `label hill_corner: zones numbered` and reads
  `hill_corner[hill]`; `oddball.rules` `label oddball_spawn: objects`;
  `race.rules` `label checkpoint: zones numbered at least 3 at most 64
  = "hill_corner"`, with `on enter(checkpoint z, o)`.
- The six scripts with no label change only where a handle may be
  `none` (`juggernaut.rules`' `crown(p: player?)`) and where a count is
  `time`.

## Rules and limits

- New keywords: `label`, `const`, `match`, `is`. Type and kind words
  (`zone`, `time`, `weapon`, `objects`, `numbered`, `per`) are read by
  place and stay usable as names, so `let zone = ...` compiles.
- A script declares at most 64 labels. A label's members are at most
  256. Each read of a label costs its `at most` in the worst-tick
  estimate, so a script that reads a set every tick declares one; the
  budget error names the label and says so.
- The checker reports at most 50 errors, in source order.
- `lower.rs` refuses a `Checked` script with an error; it has no error
  messages of its own beyond the IR's size limits.
- Compiling stays deterministic: labels and tags are interned in
  declaration order, then first use.
- The formatter (`ide/format.rs`) keeps its rule: it refuses its own
  output if the blob would change.

## Implementation plan

Every slice leaves the tree green and lands by itself. Paths are under
`crates/formats/rules/src` unless they start with `crates/`, `web/` or
`docs/`. "The gate" is [rules-start-gate.md](rules-start-gate.md).

| # | Slice | Owns | Tests | Done when | After |
|---|---|---|---|---|---|
| 1 (built) | The IR's tables and the check | `ir.rs`, `verify.rs`, `vm.rs`, `ty.rs`, `labels.rs` (new: `check`, `minimal`), `crates/formats/firefight/src/script.rs`; in `lang/lower.rs` only the label and tag writers (a string label is an unbounded `objects` set) | each verifier refusal; `members` stops at `max`; `check` gives each text of the gate's "The check" from a hand-built census; `check(ir, minimal(ir))` passes for every built-in; the IR fuzz seeds are remade | every existing test passes on the changed tables | the scenario-events lane |
| 2 (built) | The checker and labels | `lang/check.rs`, `lang/sig.rs`, `lang/types.rs` (new); `lang/ast.rs`, `lang/parse.rs`, `lang/lex.rs`, `lang/lower.rs`, `lang/mod.rs`; `builtin/*.rules`, `builtin/mod.rs`, `crates/formats/rules/tests/*`; the scripts in `docs/game/rules-engine.md` and `.claude/skills/h2-rules/*` | one test per diagnostic code for labels, `T?` and narrowing, `zone`, `weapon`, `is` and handlers by label; `sig` erases to `caps::ENTRIES`; a script with five errors reports five; every built-in compiles, verifies, fits the budget and passes its scenario tests | the string label forms are gone from the grammar and the tree; a built-in's label table carries its declarations; `lower.rs` makes no type decision | 1 |
| 3 (built) | The host gate | `crates/extensions/game/src/rules/runtime.rs`, `rules/guest.rs`, `rules/feature.rs`; `crates/formats/forge-doc/src/labels.rs` (`of_file`, `of_game`, which build a `rules::labels::Census`); `crates/extensions/agent-proto/src/rules.rs` (`blocked`); `crates/apps/boot/src/registration.rs` (the `film` argument) | over invented guest memory and a Forge document: a map that lacks `goal` gives `Status::Blocked`, one `Call::End` and no VM; a map that has it starts; `film` skips the check; a Forge session refuses without ending; `of_file` equals `of_game` on one fixture; `rules.state` lists the problems | `Runtime::start` runs the check and a blocked game ends in its first tick | 1 |
| 4 (built) | Scenario maps | `scenario.rs`, `workbench.rs`, `fake.rs`, `sim.rs`; the `run`, `check` and `simulate` wire types in `crates/live/live-proto/src/rules.rs` and their use in `crates/live/live-server/src/rule_scripts.rs` | a scenario with a map runs Siege's phases on the fake; a missing label is a `map` problem and no run; the check report lists `labels` and `minimal` from a hand-built blob; a generated match passes the check | `h2 rules run`, `POST /v1/rules/run` and `/v1/rules/simulate` take a map with classes; the report has `labels` | 1; the scenario lane and the generator lane |
| 5 (built) | Time, constants, match, value functions | the `lang/` files, the built-ins, the tests and the doc scripts of slice 2 | one test per diagnostic code; a `time` option's bits equal an `int` option's of the same range, and its label ends ` (seconds)`; the built-ins' scenario tests | the fixtures of "Stronger typing" compile or fail with the texts on this page; the built-ins use `time`, `const` and `match` | 2 |
| 6 (built) | The editor's Rust half | `ide/*`, `crates/apps/h2-tools/src/rules_lsp.rs`; the `ide` route's `actions` and `rename` in `rule_scripts.rs` | hover, completion order in `inside(p, ` and after `is`, tokens, definition, references and rename for a label; each fix applied gives a script without that error; `ide/model.rs` holds no type inference | `rules::ide` reads `Checked`; `h2 rules lsp` answers `codeAction` and `rename` | 2; beside 5 |
| 7 (built) | The lobby gate | `crates/extensions/game/src/rules/gate.rs` (new), `rules/mod.rs`, `crates/extensions/game/src/lobby_mode.rs` (`start_refused`, the `LOBBY_VARIANT_PANEL` hook), `lobby_status.rs`, `lobby_mode/tests.rs` | over invented guest memory and a map fixture: a changed map, Forge map or variant runs the check once; a blocked pair answers `REFUSED_STATE` and the status line's text, with `(and n more)`; Forge mode is not held; a host block becomes the lobby's verdict; `rules.state` lists the problems in the lobby | Start Game does nothing on a blocked pair and the status line says why | 3 |
| 8 (built) | The web workbench | `web/src/lib/rules.ts`, `web/src/lib/rulesIde.ts`, `web/src/stats/rulesMonaco.ts`, `web/src/stats/RulesTest.tsx`, `web/src/stats/Rules.tsx`, `web/src/stats/RulesEditor.tsx` and their tests | pure-half tests: the scenario map round-trips, "Add what the script needs" satisfies the report's labels, events name things by label, the Insert groups; a code action applies its edits | the page shows map problems, quick fixes, rename and "What a map needs" | 4, 6 |
| 9 (built) | The hopper gate | `crates/live/live-server/src/admin.rs` (`check_pairs`), `crates/live/live-proto/src/map_census.rs` (new), the census route in `crates/live/live-server/src/content.rs` and `http.rs`, `crates/apps/cli/src/live_maps.rs` (`census`), `docs/live/live-api.md` | an edit with a blocked pair, an unknown script or a map with no census is refused by name, and one with good pairs publishes; a new census lists the entries it blocks; the census event applies in both homes | no published hopper holds a blocked pair | 1, 3 (`Census`); the context-policy lane |
| 10 (built) | Message templates | `lang/check.rs`, `lang/lower.rs` (the template), `caps.rs` (the `fmt_*` entries), `verify.rs` (the run rule), `vm.rs`, `fake.rs` | each `text.hole` error; a `fmt_*` run without its `message`, or with the wrong types, is refused; the fake logs a message with its arguments | `message(to, "{p} scored")` reaches the fake with the player | 5; the message lane |

Slice 1 runs first; then 2, 3 and 4 side by side; then 5, 6, 7 and 9;
then 8 and 10. Slices 4 and 6 each edit `rule_scripts.rs`, 3 and 7 the
game crate's `rules/`, and 4 and 10 `fake.rs`: each pair is in different
waves. Slice 2 rewrites the built-ins in the slice that removes the
string forms, and slice 5 in the slice that adds `time`, so no branch
holds a compiler its own scripts do not pass.

Where the slices meet the lanes in flight:

- **The message and scenario-events lane** (`message(to, text)`; update
  counts on `kill` and `spawn`; `bounce`, `edit`, `actor_kill` and
  `actor_assist`; `round_time_left`; `variant.*`; scenario labelled
  objects; the context policy for ranked, hopper and custom games).
  Slice 1 starts from its `caps::ENTRIES` and `scenario.rs`. Slice 2's
  `sig.rs` types its events and reads, and `variant` fields keep the
  types it gives them ("Time"). Slice 4 adds `class` to its labelled
  objects. Slice 9 puts `check_pairs` beside its hopper policy. Slice 10
  types the holes of its `message`.
- **The match generator lane** (`sim.rs`, `/v1/rules/simulate`). Slice 4
  gives a generated match `labels::minimal` as its map, and is the one
  slice here that edits `sim.rs`.

## Not verified

- That a netgame flag of every traced type has a volume `inside` can
  test. The flag class satisfies `zone` on that assumption;
  rules-engine.md lists the zone tests as not built.
- Whether a deleted labelled object leaves its label set on a host. The
  fake keeps what the scenario lists.
- How the in-game variant editor lays out a label ending ` (seconds)`.
- The gate's open items are on [its page](rules-start-gate.md#not-verified).

## Related

- [rules-start-gate.md](rules-start-gate.md): the check a map must pass before a script game starts, and where it runs.
- [rules-engine.md](rules-engine.md): the engine, IR, VM, API and workbench.
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): the Forge document and what a label sits on.
- [live-api.md](../live/live-api-content.md#rules-scripts): the routes slices 4, 6 and 9 change.
- [web-app.md](../live/web-app-pages.md#game-type-scripts): the workbench's layout and copy.
