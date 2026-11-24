# Rules language: declared labels and stronger types

Status: Design, with slices 1 to 9 built: the IR's label and tag tables, their verifier rules, the VM's bound on a labelled set and `rules::labels::check` ("The IR", "The start gate"); the checker, declared labels, `T?` with narrowing, `zone` and `weapon` (`lang/check.rs`); the census builders, the host's gate and the lobby's gate ([rules-start-gate.md](rules-start-gate.md)); scenario maps, the check report's `labels` and the generator's minimal map ("The workbench"); Live's hopper gate, a map census and `admin::check_pairs` ([rules-start-gate.md](rules-start-gate.md#hoppers-system-link-offline-and-theater)); the editor's Rust half: `rules::ide` on the checker's `Checked` script, every diagnostic with its fixes, code actions and rename in `ide/fix.rs`, `h2 rules lsp` and the `ide` route ("Diagnostics and the editor"); `time`, constants, `match`, value functions ("Stronger typing"); the web workbench: the test map, its problems and "Add what the script needs", "What a map needs", quick fixes, rename and the Insert menu's label groups ("The workbench", "Diagnostics and the editor"). "The built language" below is the language before slice 2, which the gap table describes.
Code: `crates/formats/rules/src/lang` (lexer, parser, `check.rs`, which gives every diagnostic, `lower.rs`, which lowers a checked script), `crates/formats/rules/src/ide`, `crates/formats/rules/src/ir.rs`, `crates/formats/rules/src/verify.rs`, `crates/formats/rules/src/labels.rs`; the workbench's web half in `web/src/lib/rules.ts`, `web/src/lib/rulesIde.ts`, `web/src/stats/rulesMonaco.ts` and `web/src/stats/RulesTest.tsx`. Tests: `cargo test -p rules`; `bun test test/rules.test.ts test/rulesIde.test.ts` in `web/`.
Switch: none.

A Forge label is a declared, typed name, and the checker catches the
mistakes the built language leaves to a silent `none`: a handle that may
be missing, ticks compared with seconds, a zone test on a crate. Types
live in the front end and are erased before the IR; the IR gains only
what a host needs to check a map before the game starts. The rule that
must hold: a game whose map lacks or breaks a label the script declares
does not start, so a script that compiles never reads a label its map
lacks, and the verifier and the VM bound every blob without trusting
either check.

The language, the IR and Live's rules routes have one form each, the
current one. Nothing reads an older script or blob: a change to the
language changes the compiler, the built-ins, the examples and the tests
in the same slice ("Implementation plan").

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Everything not marked **Code** is **Design**.

## What the built language leaves unchecked

**Code**, `crates/formats/rules/src/lang/lower.rs` and the built-ins in
`content/core/rules/` and `content/modes/rules/`:

| Gap | Where it shows |
|---|---|
| A label is a string interned at each use (`Cx::label`). A typo is an empty set, and nothing says what a map must hold. | `siege.rules`: `first(objects labelled "core")` is `none` on a map with no core, and `create` gets `none` |
| `player`, `team`, `object` and `tag` always admit `none` (`Ty::nullable`); a read through `none` gives the slot's initial value. | `koth.rules`: `lead.team` is read when `first(...)` found nobody |
| Ticks and seconds are both `int`. | every timer and counter relies on the author writing `seconds(...)` |
| Every placed thing is `object`. | `inside(p, o)` accepts a crate; `on enter(zone, o)` gives two `object`s |
| Any tag path fits any `tag` parameter. | `set_loadout` accepts a vehicle's path |
| The compiler stops at its first error, and `ide/model.rs` infers types a second time for the editor. | one problem at a time in the workbench |

## How it works

### The front end

The compiler is four steps: `lex`, `parse`, **check**, `lower`. The
checker (`lang/check.rs`, new) resolves every name, gives every
expression a front-end type, collects every error (at most 50) and
returns a `Checked` script: the tree, a type per expression, a definition
per name use, the narrowing facts and the diagnostics. `lower.rs` reads a
`Checked` script that has no errors and emits IR; it makes no type
decision. `rules::ide` reads the same `Checked` script, so the editor and
the compiler cannot disagree.

### Front-end types

| Type | Values | IR type |
|---|---|---|
| `int`, `fixed`, `bool` | numbers and truth values | `Int`, `Fixed`, `Bool` |
| `time` | a duration or a tick count, in ticks | `Int` |
| `player`, `team`, `squad` | handles | `Player`, `Team`, `Squad` |
| `object` | anything placed or made that has a handle | `Object` |
| `zone` | an `object` with a volume: a Forge trigger volume, or a netgame flag with its capture volume | `Object` |
| `tag` | a tag path of any object definition | `Tag` |
| `weapon` | a `tag` that must be a weapon | `Tag` |
| `sound` | a `tag` that must be a sound (`snd!`); a string in a `sound` place is interned with the sound group, and the start gate blocks a map whose tag index lacks it, by name | `Tag` |
| `class`, `text` | an AI class; a string-table text | `Class`, `Text` |
| `enum` types | script-declared | `Enum(n)`; `Int` |
| `timer` | a global variable's type; it reads and writes as `time` | `Timer` |
| `T?` | `T` or `none`, for `player`, `team`, `object`, `zone`, `tag`, `weapon` | the IR type of `T` |
| `objects`, `zones`, `spawns`, `players`, `teams` | sets: not values, used where a set is read | none (`SetBase`) |

Three conversions are implicit and nothing else is: `zone` to `object`,
`weapon` to `tag`, and `T` to `T?`. A whole-number literal reads as
`fixed` where one is wanted.

**Flags and markers.** Placed things have two handle types, `object` and
`zone`, and no third for a position-only marker. A netgame flag is both:
it satisfies an `object` label and a `zone` label, registers as an
objective and is tested with `inside`. `delete` of one compiles. When
the argument's type is `zone` the checker warns (`zone.delete`), and on
a host a `delete` of a flag does nothing and leaves a trace warning.

The API's front-end signatures are a table beside the checker
(`lang/sig.rs`, new), keyed by entry number; `caps::ENTRIES` keeps the IR
types. A test asserts each front-end signature erases to its entry's
`args` and `result`. The rows that differ from "the IR type, not none":

| Entry | Front-end signature |
|---|---|
| `inside(p, z)`, `ticks_held(z, t)` | `player, zone -> bool`; `zone, team -> time` |
| `o.carrier`, `player_of(o)` | `object -> player?` |
| `o.team` | `object -> team?` (neutral is `none`) |
| `end_round(winner)`, `ff_team_finish(winner)` | `team?` (`none` is a draw) |
| `register_objective(o, t)`, `spawn_objective(tag, at, t)` | the team is `team?` |
| `set_loadout(p, a, b)`, `give(p, w)` | `player, weapon?, weapon?`; `player, weapon` |
| `spawn_areas(t, areas, n)`, `spawn_near(t, z, n)` | `team, spawns, int`; `team, zone?, int` (`none` lifts the rule) |
| `set_body(t, body)` | `team, tag?` (`none` gives the players their own) |
| `sound_to(s, p)`, `sound_to_team(s, t)`, `sound_to_all(s)`, `sound_at(s, o)`, `music(s)` | `sound, player`; `sound, team`; `sound`; `sound, object`; `sound` |
| `show_area(z, t)`, `hide_area(z)` | `zone, team?` (`none` is neutral); `zone` |
| `hud_value(w, n)`, `hud_fill(w, f)`, `hud_style(w, n)`, `hud_text(w, "…")`, `hud_show(w, on)` | `int` (a widget's number, the name of its `hud` declaration), then `int`; `fixed`; `int`; `text`; `bool` |
| `mark(wp, z)`, `unmark(wp)` | `int` (a waypoint's number, the name of its `waypoint` declaration), then `zone`; `int` |
| `side_of(p)`, `slot_of(p)` | `player -> int`: the player's team index in a game with teams and 16 and its roster slot in one without; the roster slot |
| `teleport(o, at)`, `move_to(o, at, t)`, `object_pos`, `object_vel` | `object, vec`; `object, vec, time`; `object, int -> fixed` (a `vec` is three `fixed` registers in the IR, `Type::width`; `at` may also be an object, and `teleport(o, at, offset)` an object and a `vec`) |
| `vec(x, y, z)`, `length(v)`, `distance(a, b)`, `v.x`, `o.position`, `o.velocity`, `o.speed` | `fixed, fixed, fixed -> vec`; `vec -> fixed`; `vec, vec -> fixed`; `vec -> fixed`; `-> vec`; `-> vec`; `-> fixed` |
| `on arrive(o)` | `object` |
| `place_squad(s, c, n, at)` | `squad, class, int, objects` |
| `round_time_left()`, `now()`, `seconds(n)` | `-> time`; `int -> time` |
| `on kill(victim, killer)` | `player, player?` |
| `on enter(z, o)`, `on leave(z, o)`, `on bounce(p, spot, chain, monster)` | `zone, object`; `player, zone, int, bool` |
| `on actor_kill(killer, class)`, `on actor_assist(killer, assist, class)` | the players are `player?` |

Every other parameter and event argument is non-null: a loop member, a
`spawn`, `death`, `join`, `quit`, `pickup`, `drop`, `press` or `edit`
argument, `p.team`. The table follows `caps::ENTRIES`: an entry or event
argument added there (the scenario-events lane's update counts on `kill`
and `spawn`, its `round_time_left`) gets its row in the slice that first
compiles against it.

## Labels

### Declaring a label

```text
label goal: zone
label core: object
label barrier: objects numbered
label capture: zones numbered 0..1 = "zone"
label attack: spawns numbered 0..2
label flag_spawn: zones per team = "ctf_flag_spawn"
label bonus: object?
```

`label <name>: <kind> [requirements] [= "<map text>"]`. The name is an
identifier and is the map's label text unless `= "..."` gives another
(label text allows `-`, `.` and a leading digit, which a name does not;
`forge_doc::label_char`). Two declarations may not name one map text.

| Kind | The script sees | The map must have |
|---|---|---|
| `object`, `zone` | one value, never `none` | exactly one |
| `object?`, `zone?` | one value or `none` | none or one |
| `objects`, `zones` | a set | any number, unless a requirement says otherwise |
| `spawns` | a set of starting locations, with no members to visit | any number, unless a requirement says otherwise |

| Requirement | On | Meaning |
|---|---|---|
| `at least n`, `at most n`, `exactly n`, `n..m` | a set | how many members the map has. A map over a stated `at most` is blocked; with none stated the set visits the first 256 (`ty::MAX_LABELLED`) |
| `numbered` | a set | every member has a number |
| `numbered a..b` | a set | every member's number is in `a..b`, and each number in it has at least one member |
| `per team` | `objects`, `zones` | each team in play has at least one member whose team it is |

A label's kind is checked against the placement's class, which the map
decides and the label does not store:

| Class | What it is | Satisfies |
|---|---|---|
| object | a placement in an object block (scenery, crates, vehicles, weapons, equipment, machines, netgame equipment), a kit barrier | `object` |
| flag | a netgame flag, with its own label or the built-in one for its type (`forge_doc::builtin_label`) | `object`, `zone` |
| volume | a Forge trigger volume | `object`, `zone` |
| spawn | a starting location | `spawns` |

Squads, emitters and devices get kinds of their own when a script can
act on one; the kind byte has room.

### Using a label

| Form | Type | Lowers to |
|---|---|---|
| `goal` (declared `zone`) | `zone` | `ForEach` over the label keeping the first member; the bound is 1 |
| `bonus` (declared `object?`) | `object?` | the same |
| `for each barrier b where b.number < n { }`, `count(barrier)`, `first(barrier b where ...)`, `pick(barrier, 2)` | member `object` | `SetBase::Labelled`, bounded by the label's `at most` |
| `capture[n]`, `n` an `int`, on a `numbered` set | `zone?`; `zone` when `n` is a constant inside `numbered a..b` | `ForEach` with `label_number(m) == n`, keeping the first |
| `flag_spawn[t]`, `t` a `team`, on any set of objects or zones | `zone?` | `ForEach` with `object_team(m) == t`, keeping the first |
| `x is goal`, `x is barrier` | `bool`; in the true branch a `let`, parameter or loop member `x` has the label's member type | `label_of(x)` compared with the label's index |
| `spawn_areas(team 0, attack, n)`, `place_squad(s, c, n, drops)` | a set argument | the label's index, as a `Label` constant |
| `on enter(goal, o) { }`, `on enter(capture z, o) { }` | a handler for that label's zones only; `z` is the member | below |

**One loop form, one zone test.** A loop is `for each <set> <name>`, for
a label as for a built-in set: `for each players p`,
`for each barrier b`. A zone test is `inside(p, goal)`. There is no `in`
form of either.

A member reads `number` (`int`; 0 with no number), `team` (`team?`), `id`
and the script's `object var`s. `o.label` is not a field: `is` replaces
it.

**Handlers by label.** `on enter`, `on leave` and `on bounce` may name a
zone label in the zone's place, and `on pickup`, `on drop`, `on press`
and `on edit` an object label in the object's place. A script may have
one handler per label and one without a label. The compiler merges them
into the event's one IR handler, in source order, each body under
`label_of(arg) == <index>`, so the IR's rule of one handler per event
stands. A handler may leave trailing arguments unnamed: `on enter(goal)`.

**A fn takes a set** when its parameter's type is `objects`, `zones` or
`spawns`; the argument is a label's name. Functions are inlined, so the
set is still a constant index. Sets are not variables or table columns.

A label is named only by its declaration. `objects labelled "x"`,
`labelled "x"` and a string where a label is wanted are not in the
grammar, and `labelled` is not a word.

### The start gate

A map that lacks or breaks a label requirement blocks the game: it does
not start, and it does not fall back to the carrier engine's rules.
`rules::labels::check(ir, census, teams)` (**Code**, `labels.rs`, pure)
compares the IR's label and tag tables with a census of the map
(`labels::Census`) and returns every problem with its text; no caller
runs it yet. The lobby runs it when the map or
the variant changes and holds Start Game, the host runs it at the game's
start as the final authority, and Live runs it on each hopper entry when
a hopper is edited. [rules-start-gate.md](rules-start-gate.md) has the
check's texts, each caller's function or hook, and what a blocked start
shows.

A non-null label is a promise the gate backs, not one the VM relies on.
A script that deletes its own `goal` reads `none` from it afterwards, by
the VM's rules for `none` (a read gives the slot's initial value, a
write does nothing), with a trace warning.

## Stronger typing

Each change gives the built language's text, this design's text, and the
error a wrong script gets. The checker's messages say what is wrong and
what to write.

### A handle that may be none

`first(...)`, `o.carrier`, `o.team`, `player_of(o)`, `killer`,
`capture[n]`, an optional label and a `T?` variable are `T?`. A `T?`
cannot be read through or passed where `T` is wanted until it is
narrowed. Narrowing applies to a `let`, a parameter and a loop member,
which are never assigned again, so it is sound without tracking writes:

- `if x != none { }` narrows in the block; `if x == none { } else { }`
  narrows in the `else`.
- `a and b` narrows in `b` what `a` proves; `x == none or b` narrows in `b`.
- `if let x = <expr> { }` binds `x` as `T` when the value is not `none`,
  for a field or a global, which a call can change. It takes one binding
  and an optional `and <cond>`.

A variable of a handle type starts as `none`, so its type is `T?`.

```text
// built (koth.rules): lead.team is read when nobody is on the hill
let lead = first(players p where p.alive and inside(p, zone))
let rivals = count(players q where q.alive and inside(q, zone) and q.team != lead.team)

// this design
if let lead = first(players p where p.alive and inside(p, zone)) {
    let rivals = count(players q where q.alive and inside(q, zone) and rival(q, lead))
}
```

`lead may be none here: first(...) found nobody. Check it with "if lead != none", or bind it with "if let lead = ...".`
A check that cannot fail is a warning: `p is never none here`.

### Time

**Code**, `lang/check.rs` (`time_arith`, `Cx::option`), `lang/lex.rs`
(`time_ticks`), `lang/lower.rs`.

`time` counts ticks. Literals: `5s`, `300t`, `1.5s` (a whole number of
ticks at `caps::TICKS_PER_SECOND`, or an error). `seconds(n)` turns an
`int` into `time`; `whole_seconds(t)` and `ticks(t)` turn `time` into
`int`. `time` adds, subtracts, takes `%` and compares with `time`,
multiplies and divides by `int`, and `time / time` is `int`. `min`,
`max` and `clamp` take all `int`s or all `time`s and give the same.
`ticks_held`, `round_time_left` and `now` give `time`. A `timer` reads
and writes as `time`. A `time` option is declared in whole seconds,
holds whole seconds in the record's option bits exactly as an `int`
option of the same range does, and reads as `time`: lowering multiplies
the read by 30. The IR stores no unit for it: the compiler ends a `time`
option's label with ` (seconds)` (`check::SECONDS_SUFFIX`, "Capture time
(seconds)"), or the option's name when it has no label, and every
variant editor shows the label.

```text
// without time
option capture_time: int 5..60 = 20
if ticks_held(zone, team 0) >= seconds(capture_time) { open(phase + 1) }
player var held: int = 0
if p.held >= seconds(1) { score(p, 1) }

// with time (siege.rules, oddball.rules)
option capture_time: time 5s..60s = 20s
if ticks_held(zone, team 0) >= capture_time { open(phase + 1) }
player var held: time = 0t
if p.held >= 1s { score(p, 1) }
```

`can't compare time with int in "p.held >= 1". Write 1s for a second or 30t for ticks.`
`wave is a timer and takes time; wave_seconds is an int. Write seconds(wave_seconds), or declare the option as time.`

`variant` fields stay `int`: the units of several are inferred, not
read (rules-engine.md, "The language"), so a script still writes
`seconds(variant.ctf_reset_time)`.

### Zones and weapons

```text
// built: both compile, and neither can work
if inside(p, first(objects labelled "core")) { }
set_loadout(p, "objects/vehicles/warthog/warthog", none)

// this design
label core: object
table loadouts: (side: team, primary: weapon, secondary: weapon) = [ ... ]
```

`core is an object, and inside takes a zone. If the map's core is a volume, declare it "label core: zone".`
`set_loadout takes a weapon, and kit.body is a tag. Declare the column as weapon.`

A string in a `weapon` place is interned with the weapon group, and the
start gate blocks a map where that path is not a weapon, by name. A `for each objectives o` member is an `object`; a port
that tests a registered marker writes
`for each objectives m where m is territory`, and `m` is a `zone`.

### Enums and match

**Code**, `Cx::match_stmt` in `lang/check.rs`; `Stmt::Match` in
`lang/lower.rs`.

```text
// without match
if victim.role == Role.vip { ... } else if enemy { score(killer, 1) }

// with match (vip.rules)
match victim.role {
    Role.vip { uncrown(victim) }
    Role.none { if let k = killer and enemy(k, victim) { score(k, 1) } }
}
```

`match` takes an enum value and one arm per variant, or an `else` arm
for the rest. An arm may name several variants, comma-separated
(`Mode.b, Mode.c { }`); a variant in two arms is an error. It lowers to
compares and forward jumps.
`match on Role has no arm for Role.guard. Add "Role.guard { }" or "else { }".`

### Constants

**Code**, `Cx::eval_const` and `Cx::fold` in `lang/check.rs`.

`const last_phase = 2`, `const hold = 20s`. A constant's value is an
expression over `int` and `time` literals, other constants,
`len(table)`, unary `-` and `+ - * / %` with the units of "Time",
folded by the checker in 64 bits and checked against the `int` range.
Constants are allowed wherever a literal is: option ranges and defaults,
initial values, table cells, label requirements. A constant that refers
to itself, or names anything but a constant, is a `decl` error. A
constant index into a table or a numbered label is checked against its
rows or numbers. Folding makes these compile errors (`const.range`,
`const.zero`):

`ladder[12]: ladder has 12 rows, 0 to 11.` (a script error at run time without constants)
`capture[2] is outside "numbered 0..1".`
`division by zero in the constant "step".`

### Functions with a value

`fn rival(a: player, b: player) -> bool = (variant.team_play and a.team != b.team) or (not variant.team_play and a != b)`

A value function states its type after `->` and is one expression
after `=`, inlined at each call; its arguments are checked as a block
function's are. A block function has no value: using one as a value is
an error, as is calling a value function as a statement.
`koth.rules` and `territories.rules` each declare `rival`, and
`vip.rules` `enemy`.
`rival takes a player, and lead may be none here.`

### let, options, variables, tables, sets

- `let` may state its type: `let heir: player? = none`. A bare
  `let x = none` is an error that says so.
- A `let` or loop member that shadows a label, a global, an option or a
  constant is a warning.
- Options are `int a..b`, `bool`, an enum, or `time a..b`.
- Scoped variables take any value type, with `T?` for handles:
  `object var owner: player? = none`. An `object var` belongs to zones
  too.
- A table column may be `weapon` or `T?`. A keyed read
  (`loadouts[p.team]`) of a handle column is `T?`, because a key no row
  holds reads `none`; it is `T` when the key is an enum and every
  variant has a row. A constant `int` index out of range is an error.
- A set's `where` is checked with the member in scope at the member's
  type, and `where m is <label>` narrows the member for the body.

### Messages

**Code**: `lang/check.rs` (`Cx::template`, `Cx::hole`), `lang/lower.rs`
(`Cx::message`), `caps.rs` (`Hole`, `pieces`, `holes`, `fill`),
`verify.rs` (`message_runs`), `vm.rs` (`Run::hole`), `fake.rs`
(`FakeMessage`).

`message(to, "{p} scored, {n} to go")` checks each hole against the
names in scope. A hole holds one name: a local, a global, an option or
a constant. It shows a `player`, `team`, `int`, `time` or enum; a
`player?` or `team?` shows once narrowed. A message has at most four
holes, and `{{` and `}}` write a brace. Each problem is a `text.hole`
error at the hole:

`{q} in this message is not a name here. Names in scope: p, n.`
`an object can't be shown in a message. A message shows a player, a team, an int, a time or an enum.`
`killer may be none here, and a message shows a player. Show it inside "if let" or after a test that it is not none.`
`{p.team} is not a name. A hole holds one name, as {p}; write {{ for a brace.`
`this { opens a hole with no }. Write {{ for a brace.`
`a message shows at most 4 values, and this one has 5.`

The text is stored with typed holes (`{player} scored, {int} to go`),
literal braces doubled; a text with no holes is stored as written. The
lowering evaluates the holes' names, then writes the template's string
index, then emits one `fmt_player`, `fmt_team`, `fmt_int`, `fmt_time` or
`fmt_enum` call per hole, in order, then the `message` call, with
nothing between them. The `fmt_*` entries are VM-only entries in
`caps::ENTRIES` with no result that a script cannot call by name
(`caps::is_form`). The VM queues each value, at most four, and hands
them to the next `message` after the entry's own arguments:
`message(to, text)` reaches the game as `[to, text, values...]`
(`[text, values...]` for `all`). A player, team or int reaches it as
itself, a time as its ticks, and an enum as its variant's name
(`Arg::Text`); the game reads which by the template's holes.
`caps::fill` fills a template and `caps::show_time` shows a time as
`m:ss`. The fake logs each message in `FakeGame::messages`: to whom, the
template, the values and the line (`player 1 scored`).

The verifier's message rule (`verify::message_runs`): each run of
`fmt_*` calls directly follows the `Const` that writes the text of the
`message` call directly after it; no jump or loop end lands inside the
run or on the message; the run has at most four calls; and the text is a
template whose holes match the run in count, order and type. A message
with no run whose text a `Const` writes just before it shows a template
with no holes. `fmt_enum` takes a register of any enum type. The
refusals are `Fault::Unshown`, `Holes`, `Template`, `IntoHoles`,
`NotTemplate` and `HoleTypes`. The entry's own arguments are those of
`message(to, text)` as the message lane left it in `caps::ENTRIES`; this
design adds only the holes.

### Capabilities

A call whose capability is missing from `requires` is already a compile
error (`Cx::call`). The checker adds the fix ("Add player.weapons to
requires"), the capability and its lobby words in the call's hover, and
a warning for one nothing uses:
`objects.delete is required but nothing uses it; the lobby would still say "removes objects".`

### Considered, not adopted

| Idea | Why not |
|---|---|
| String labels beside declared ones | one unchecked path keeps every problem this page removes; an odd label text has `= "text"` |
| A third handle type for position-only markers | retail registers markers as objectives and tests players against flag spawns (`territories.rules`, `ctf.rules`), so ports would convert at every use; the one mistake it catches, `delete` of a marker, is a warning and a no-op ("Front-end types") |
| `p in goal` for `inside(p, goal)` | two spellings of one test, and `in` reads as membership to anyone who has programmed |
| `for each b in barrier` | every loop is `for each <set> <name>`; one loop form, not two |
| Separate `seconds` and `ticks` types | one `time` with unit literals removes the mixing; two types add conversions between two right answers |
| `time` on `variant` fields | several fields' units are inferred, not read |
| A unit on an option in the IR, for the variant editor | the option's label carries it ("Capture time (seconds)"); no editor changes |
| Falling back to the carrier's rules on a map that breaks a label | the players get a game that is not the one they chose, with nothing on screen saying so; the gate says why before the game |
| Reading older scripts or blobs: a language line, a second IR version, a migrator, frozen built-in blobs | nothing is published; one form each keeps the compiler, the verifier and Live's routes single-path |
| `vehicle` and other tag groups | no entry takes one; each is added with the first entry that does |
| Records, tuples and mutable rows | a constant `table` and `let row = t[i]` cover the corpus; a mutable record needs slots per field, which scoped variables already are |
| Sets as values, `map`, `filter`, closures | no heap; `where`, `count`, `first` and `pick` cover the corpus |
| `any(set)`, `all(set)` | `first(...) != none` and `count(...) == 0` say it; `ForEach` cannot stop early, so neither would be cheaper |
| Block functions with `return` | a jump cannot leave a `ForEach` body (`ir.rs`), so `return` in a loop cannot be lowered |
| Generic functions and user-declared generic types | set parameters cover the one use |
| Integer ranges as types (`int 0..2`) | constants catch the constant cases; the rest need flow analysis the errors would not explain well |
| Type annotations on handler arguments | the event fixes them; hover shows them |
| A capability as a type of value | a missing capability is already an error at the call |

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
| `label.spawns` | `attack names spawn areas, which have no objects to visit. Pass it to spawn_areas.` | none |
| `label.index` | `barrier is not numbered, so barrier[n] has no meaning. Add "numbered" to its declaration.` | Add `numbered` |
| `label.same-text` | `capture and zone both name the map label "zone". Keep one.` | none |
| `label.unused` (warning) | `label core is declared but never used; a map would still need one.` | Remove the declaration |
| `null.use`, `null.arg` | "A handle that may be none" | Wrap in `if let`; Add `!= none and` |
| `null.var` | `a player variable starts as none, so its type is player?.` | Add `?` |
| `null.never` (warning) | `p is never none here.` | Remove the check |
| `unit.mix`, `unit.literal` | "Time"; `0.01s is not a whole number of ticks (30 a second).` | Add `s`; Wrap in `seconds(...)` |
| `tag.group` | "Zones and weapons" | Change the column to `weapon` |
| `body.unknown` | `objects/characters/elite/elite is not a player body; set_body takes objects/characters/masterchief/masterchief_mp or objects/characters/elite/elite_mp` | none |
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
| label (`LabelDef`) | text (a string); kind, a byte (`LabelKind`: 1 object, 2 zone, 3 spawn); `min` and `max` members, two `u16`s, `max` at most 256 (`ty::MAX_LABELLED`, which states no limit); flags, a byte (`ir::label_flags`: 1 single, 2 numbered, 4 number range, 8 per team); the number range `lo` and `hi`, two `i32`s, inclusive, both 0 without the range flag |
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
- [live-api.md](../live/live-api.md#rules-scripts): the routes slices 4, 6 and 9 change.
- [web-app.md](../live/web-app.md#game-type-scripts): the workbench's layout and copy.
