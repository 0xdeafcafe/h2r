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
| label kind `path` | a flight path: points numbered from 0, passed to `fly` and nowhere else | a label (`LabelKind::Path`) |

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
| `end_round(winner)`, `next_round(winner)`, `ff_team_finish(winner)` | `team?` (`none` is a draw) |
| `register_objective(o, t)`, `spawn_objective(tag, at, t)` | the team is `team?` |
| `set_loadout(p, a, b)`, `give(p, w)` | `player, weapon?, weapon?`; `player, weapon` |
| `spawn_areas(t, areas, n)`, `spawn_near(t, z, n)` | `team, spawns, int`; `team, zone?, int` (`none` lifts the rule) |
| `set_body(t, body)` | `team, tag?` (`none` gives the players their own) |
| `sound_to(s, p)`, `sound_to_team(s, t)`, `sound_to_all(s)`, `sound_at(s, o)`, `music(s)` | `sound, player`; `sound, team`; `sound`; `sound, object`; `sound` |
| `show_area(z, t)`, `hide_area(z)` | `zone, team?` (`none` is neutral); `zone` |
| `hud_value(w, n)`, `hud_fill(w, f)`, `hud_style(w, n)`, `hud_text(w, "…")`, `hud_show(w, on)` | `int` (a widget's number, the name of its `hud` declaration), then `int`; `fixed`; `int`; `text`; `bool` |
| `mark(wp, z)`, `mark(wp, z, to)`, `unmark(wp)` | `int` (a waypoint's number, the name of its `waypoint` declaration), then `zone`; the audience form's `to` a `player`, a `team` or `all` (everyone, the two-argument form) |
| `side_of(p)`, `slot_of(p)` | `player -> int`: the player's team index in a game with teams and 16 and its roster slot in one without; the roster slot |
| `teleport(o, at)`, `move_to(o, at, t)`, `object_pos`, `object_vel` | `object, vec`; `object, vec, time`; `object, int -> fixed` (a `vec` is three `fixed` registers in the IR, `Type::width`; `at` may also be an object, and `teleport(o, at, offset)` an object and a `vec`) |
| `set_open(o, open)` | `object, bool` |
| `offer_pick(n, title)`, `pick_row(n, label)`, `picked(p, n)` | `int, text`; `int, text`; `player, int -> int` |
| `offer(p, picker, count)`, `choice(p)` | `player, int, int`; `player -> int` |
| `objective(to, text)`, `objective_mark(to, zone, kind)`, `meter(to, value, of)`, `countdown(to, ticks)` | `team, text`; `team, zone, int` (kind 1 attack, 2 defend, 0 lifts the mark); `team, int, int` (an `of` of 0 or less lifts it; use `ticks(t)` for a time); `team, time` (0 or less lifts it) |
| `vec(x, y, z)`, `length(v)`, `distance(a, b)`, `v.x`, `o.position`, `o.velocity`, `o.speed` | `fixed, fixed, fixed -> vec`; `vec -> fixed`; `vec, vec -> fixed`; `vec -> fixed`; `-> vec`; `-> vec`; `-> fixed` |
| `on arrive(o)` | `object` |
| `on touchdown(p, down, up)` | `player, fixed, fixed` |
| `on launch(p, spot, speed)`, `on grounded(p)` | `player, zone?, fixed`; `player` |
| `airborne(p)`, `crouching(p)`, `primed(p)` (`p.airborne`, `p.crouching`, `p.primed`), `sees(a, b)` | `player -> bool`; `object, object -> bool` |
| `place_squad(s, c, n, at)` | `squad, class, int, objects` |
| `fly(v, p, t)` | `object, path, time`; the vehicle is an `object` the host checks, since `vehicle` is not a front-end type ([rules-world-api.md](rules-world-api.md#vehicles-and-drop-ships)) |
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
| `path` | the points of a flight, with no members to visit; passed to `fly` | at least 2, numbered from 0 with none missing and none twice; a count requirement may raise the least or state a most |

| Requirement | On | Meaning |
|---|---|---|
| `at least n`, `at most n`, `exactly n`, `n..m` | a set | how many members the map has. A map over a stated `at most` is blocked; with none stated the set visits the first 256 (`ty::MAX_LABELLED`) |
| `numbered` | a set | every member has a number |
| `numbered a..b` | a set | every member's number is in `a..b`, and each number in it has at least one member |
| `per team` | `objects`, `zones` | each team in play has at least one member whose team it is |

A `path` is numbered already: `numbered`, `per team` and `path?` are
declaration errors, and a count below 2 is raised to 2. The start gate
(`labels::check`) names the first number missing below the path's last
(`ProblemKind::PathGap`), a number used twice (`PathDuplicate`) and a point
with no number, so a hole in a flight blocks the start instead of
skipping a leg. A path's points are objects of the map (any placement but
a starting location); where each is comes from the map at run time.

A label's kind is checked against the placement's class, which the map
decides and the label does not store:

| Class | What it is | Satisfies |
|---|---|---|
| object | a placement in an object block (scenery, crates, vehicles, weapons, equipment, machines, netgame equipment), a kit barrier | `object` |
| flag | a netgame flag, with its own label or the built-in one for its type (`forge_doc::builtin_label`) | `object`, `zone` |
| volume | a Forge trigger volume | `object`, `zone` |
| spawn | a starting location | `spawns` |
| object, flag or volume, numbered | a point of a flight | `path` |

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
`spawns`; the argument is a label's name. (A `path` parameter is not
built: `fly` takes a label directly.) Functions are inlined, so the
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

## Pages

| Page | What |
|---|---|
| [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md) | Diagnostics and the editor, the workbench, the IR and the implementation plan. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Diagnostics and the editor

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#diagnostics-and-the-editor).

## The workbench

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#the-workbench).

## The IR

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#the-ir).

## The built-ins, rewritten

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#the-built-ins-rewritten).

## Rules and limits

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#rules-and-limits).

## Implementation plan

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#implementation-plan).

## Not verified

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#not-verified).

## Related

See [Rules language types: diagnostics, workbench and IR](rules-language-types-tooling.md#related).
