# Rules language 2: declared labels and stronger types

Status: Design. Nothing here is built; the language the code compiles is rules language 1 ([rules-engine.md](rules-engine.md#the-language)).
Code: `crates/formats/rules/src/lang` (lexer, parser, `lower.rs`, which type checks and lowers in one pass), `crates/formats/rules/src/ide`, `crates/formats/rules/src/ir.rs`, `crates/formats/rules/src/verify.rs`. Tests: `cargo test -p rules`.
Switch: none.

Rules language 2 makes a Forge label a declared, typed name, and makes the
checker catch the mistakes language 1 leaves to a silent `none`: a handle
that may be missing, ticks compared with seconds, a zone test on a crate.
Types live in the front end and are erased before the IR; the IR gains
only what a host needs to check a map before the game starts. The rule
that must hold: a script that compiles, on a map the start check accepts,
never reads a label the map lacks, and the verifier and the VM bound
every blob without trusting either check.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Everything not marked **Code** is **Design**.

## What language 1 leaves unchecked

**Code**, `crates/formats/rules/src/lang/lower.rs` and the built-ins in
`crates/formats/rules/src/builtin`:

| Gap | Where it shows |
|---|---|
| A label is a string interned at each use (`Cx::label`). A typo is an empty set, and nothing says what a map must hold. | `siege.rules`: `first(objects labelled "core")` is `none` on a map with no core, and `create` gets `none` |
| `player`, `team`, `object` and `tag` always admit `none` (`Ty::nullable`); a read through `none` gives the slot's initial value. | `koth.rules`: `lead.team` is read when `first(...)` found nobody; `ctf.rules`: `f.at_home = true` through a neutral bomb's `none` team |
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

A source file names its language: `language 2`, a header line after
`rules`. A file with no `language` line is language 1, which this
compiler refuses with one error and a fix ("Migration").

### Front-end types

| Type | Values | IR type |
|---|---|---|
| `int`, `fixed`, `bool` | as language 1 | `Int`, `Fixed`, `Bool` |
| `time` | a duration or a tick count, in ticks | `Int` |
| `player`, `team`, `squad` | handles | `Player`, `Team`, `Squad` |
| `object` | anything placed or made that has a handle | `Object` |
| `zone` | an `object` with a volume: a Forge trigger volume, or a netgame flag with its capture volume | `Object` |
| `tag` | a tag path of any object definition | `Tag` |
| `weapon` | a `tag` that must be a weapon | `Tag` |
| `class`, `text` | as language 1 (`Class` is also accepted) | `Class`, `Text` |
| `enum` types | script-declared, plus the built-in `carrier` (`carrier.assault`), which lowers to an `int` constant | `Enum(n)`; `Int` |
| `timer` | a global variable's type; it reads and writes as `time` | `Timer` |
| `T?` | `T` or `none`, for `player`, `team`, `object`, `zone`, `tag`, `weapon` | the IR type of `T` |
| `objects`, `zones`, `spawns`, `players`, `teams` | sets: not values, used where a set is read | none (`SetBase`) |

Three conversions are implicit and nothing else is: `zone` to `object`,
`weapon` to `tag`, and `T` to `T?`. A whole-number literal still reads as
`fixed` where one is wanted.

The API's front-end signatures are a table beside the checker
(`lang/sig.rs`, new), keyed by entry number; `caps::ENTRIES` keeps the IR
types. A test asserts each front-end signature erases to its entry's
`args` and `result`. The rows that differ from "the IR type, not none":

| Entry | Front-end signature |
|---|---|
| `inside(p, z)`, `ticks_held(z, t)` | `player, zone -> bool`; `zone, team -> time` |
| `o.carrier`, `player_of(o)` | `object -> player?` |
| `o.team` | `object -> team?` (neutral is `none`) |
| `end_round(winner)` | `team?` |
| `register_objective(o, t)`, `spawn_objective(tag, at, t)` | the team is `team?` |
| `set_loadout(p, a, b)`, `give(p, w)` | `player, weapon?, weapon?`; `player, weapon` |
| `spawn_areas(t, areas, n)` | `team, spawns, int` |
| `place_squad(s, c, n, at)` | `squad, class, int, objects` |
| `round_time_left()`, `now()`, `seconds(n)` | `-> time`; `int -> time` |
| `on kill(victim, killer)` | `player, player?` |
| `on enter(z, o)`, `on leave(z, o)`, `on bounce(p, spot)` | `zone, object`; `player, zone` |
| `on actor_kill(killer, class)`, `on actor_assist(killer, assist, class)` | the players are `player?` |

Every other parameter and event argument is non-null: a loop member, a
`spawn`, `death`, `join`, `quit`, `pickup`, `drop`, `press` or `edit`
argument, `p.team`.

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
| `at least n`, `at most n`, `exactly n`, `n..m` | a set | how many members the map has. A map over a stated `at most` is refused; with none stated the set visits the first 256 (`ty::MAX_LABELLED`) |
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
| `spawn_areas(team 0, attack, n)`, `place_squad(s, c, n, drops)` | a set argument | the label's index, as language 1's `Label` constant |
| `on enter(goal, o) { }`, `on enter(capture z, o) { }` | a handler for that label's zones only; `z` is the member | below |

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

String labels do not exist in language 2: `objects labelled "x"`,
`labelled "x"` and a string where a label is wanted are each an error
with a fix that declares the label and rewrites every use.

### The start check

**Design.** `rules::labels::check(ir, census, teams)` (`labels.rs`, new,
pure) compares the IR's label table with a census of the map: for each
label text, its members' class, number and team. Three callers share it:

- **The host, at the game's start.** `runtime` builds the census from
  `ForgeDoc::labels` and the built-in flag labels, after `verify::load`
  and beside `verify::unhostable` and `runtime::host_refusal`
  (`crates/extensions/game/src/rules/runtime.rs`). A failure refuses the
  script by rules-engine.md's rule for any refusal: logged, kept for
  `rules.state`, and the game plays the carrier engine's own rules.
- **The lobby**, when a map and a script variant are both chosen: the
  same check over the same census, shown as the reason the pair cannot
  start. The hopper builder runs it per map and variant.
- **The workbench**, over the scenario's map ("The workbench").

Refusal texts name the label, what the map has and what the script
needs:

| Case | Text |
|---|---|
| none of a required label | `the map has no zone labelled "goal"; Siege needs exactly 1` |
| too many or too few | `the map has 3 objects labelled "core"; Siege needs exactly 1` |
| wrong class | `"goal" on this map is a crate; Siege needs a zone (a trigger volume or a flag)` |
| a missing number | `no zone labelled "zone" has the number 1; Siege needs 0 and 1` |
| an unnumbered member | `a zone labelled "hill_corner" has no number; King of the Hill needs every one numbered` |
| a team without one | `team 1 has no zone labelled "ctf_flag_spawn"` |

A non-null label is a promise the start check backs, not one the VM
relies on. A script that deletes its own `goal` reads `none` from it
afterwards, by the VM's rules for `none` (a read gives the slot's initial
value, a write does nothing), with a trace warning.

## Stronger typing

Each change gives the language 1 text, the language 2 text, and the
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
// language 1 (koth.rules): lead.team is read when nobody is on the hill
let lead = first(players p where p.alive and inside(p, zone))
let rivals = count(players q where q.alive and inside(q, zone) and q.team != lead.team)

// language 2
if let lead = first(players p where p.alive and inside(p, zone)) {
    let rivals = count(players q where q.alive and inside(q, zone) and rival(q, lead))
}
```

`lead may be none here: first(...) found nobody. Check it with "if lead != none", or bind it with "if let lead = ...".`
A check that cannot fail is a warning: `p is never none here`.

### Time

`time` counts ticks. Literals: `5s`, `300t`, `1.5s` (a whole number of
ticks at `caps::TICKS_PER_SECOND`, or an error). `seconds(n)` turns an
`int` into `time`; `whole_seconds(t)` and `ticks(t)` turn `time` into
`int`. `time` adds, subtracts and compares with `time`, multiplies and
divides by `int`, and `time / time` is `int`. A `timer` reads and writes
as `time`. A `time` option is declared and stored in whole seconds, so
its bits are those of an `int` option, and reads as `time`.

```text
// language 1 (siege.rules, oddball.rules)
option capture_time: int 5..60 = 20
if ticks_held(zone, team 0) >= seconds(capture_time) { open(phase + 1) }
player var held: int = 0
if p.held >= seconds(1) { score(p, 1) }

// language 2
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
// language 1: both compile, and neither can work
if inside(p, first(objects labelled "core")) { }
set_loadout(p, "objects/vehicles/warthog/warthog", none)

// language 2
label core: object
table loadouts: (side: team, primary: weapon, secondary: weapon) = [ ... ]
```

`core is an object, and inside takes a zone. If the map's core is a volume, declare it "label core: zone".`
`set_loadout takes a weapon, and kit.body is a tag. Declare the column as weapon.`

A string in a `weapon` place is interned with the weapon group, and the
host refuses a map where that path is not a weapon, by name, with the
label check. A `for each objectives o` member is an `object`; a port
that tests a registered marker writes
`for each objectives m where m is territory`, and `m` is a `zone`.

### Enums and match

```text
// language 1 (vip.rules)
if victim.role == Role.vip { ... } else if enemy { score(killer, 1) }

// language 2
match victim.role {
    Role.vip { uncrown(victim) }
    Role.none { if enemy { score(killer, 1) } }
}
```

`match` takes an enum value and one arm per variant, or an `else` arm.
It lowers to compares and forward jumps.
`match on Role has no arm for Role.guard. Add "Role.guard { }" or "else { }".`
`variant.engine` is the built-in enum `carrier`:
`variant.engine == carrier.assault`, not `== 9`.

### Constants

`const last_phase = 2`, `const hold = 20s`. A constant's value is an
expression over literals, other constants, `len(table)` and `+ - * / %`,
folded by the checker. Constants are allowed wherever language 1 wants a
literal: option ranges and defaults, initial values, table cells, label
requirements. Folding makes these compile errors:

`ladder[12]: ladder has 12 rows, 0 to 11.` (a script error at run time in language 1)
`capture[2] is outside "numbered 0..1".`
`division by zero in the constant "step".`

### Functions with a value

`fn rival(a: player, b: player) -> bool = (variant.team_play and a.team != b.team) or (not variant.team_play and a != b)`

A value function is one expression, inlined at each call. A block
function has no value, as in language 1. `koth.rules` and
`territories.rules` each repeat that predicate twice.
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

`message(to, "{p} scored, {n} to go")` checks each hole against the
names in scope. A hole shows a `player`, `team`, `int`, `time` or enum.

`{q} in this message is not a name here. Names in scope: p, n.`
`an object can't be shown in a message. A message shows a player, a team, an int, a time or an enum.`

The text is stored with typed holes (`{player} scored, {int} to go`).
The arguments reach the VM as `fmt_player`, `fmt_team`, `fmt_int`,
`fmt_time` and `fmt_enum` calls directly before the `message` call: new
VM-only entries of API version 2, which queue at most four values the
next `message` takes. The verifier checks that each run of `fmt_*` calls
is followed at once by a `message` whose holes match it in count, order
and type. The entry's own arguments are those of the `message(to, text)`
entry in `caps::ENTRIES`.

### Capabilities

A call whose capability is missing from `requires` is already a compile
error (`Cx::call`). Language 2 adds the fix ("Add player.weapons to
requires"), the capability and its lobby words in the call's hover, and
a warning for one nothing uses:
`objects.delete is required but nothing uses it; the lobby would still say "removes objects".`

### Considered, not adopted

| Idea | Why not |
|---|---|
| String labels as an escape hatch | one unchecked path keeps every problem this page removes; an odd label text has `= "text"` |
| A third handle type for position-only markers | retail registers markers as objectives and tests players against flag spawns (`territories.rules`, `ctf.rules`), so ports would convert at every use; the one mistake it catches, `delete` of a marker, is a no-op with a trace warning |
| `p in goal` for `inside(p, goal)` | two spellings of one test, and `in` reads as membership to anyone who has programmed |
| `for each b in barrier` | language 1 writes `for each players p`; one loop form, not two |
| Separate `seconds` and `ticks` types | one `time` with unit literals removes the mixing; two types add conversions between two right answers |
| `time` on `variant` fields | several fields' units are inferred, not read |
| A unit on an option in the IR, for the variant editor | the option's label carries it ("Capture time (seconds)"); no editor changes |
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
exists, fixes: text edits the editor applies. `ide::Diagnostic` gains
`code` and `fixes`; a new `actions` operation returns the fixes at a
range (LSP `textDocument/codeAction`), and `rename` renames a
declaration and its uses.

| Code | Message | Fixes |
|---|---|---|
| `lang.version` | `this script has no "language" line, so it is rules language 1, which this build does not compile.` | Update the script to language 2 |
| `label.string` | `objects labelled "barrier" is rules language 1. Declare "label barrier: objects" and write barrier.` | Declare the label and rewrite its uses |
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
| `match.missing` | "Enums and match" | Add the missing arms |
| `const.range`, `const.zero` | "Constants" | none |
| `text.hole` | "Messages" | none |
| `cap.missing`, `cap.unused` (warning) | "Capabilities" | Add to `requires`; Remove from `requires` |
| `event.args`, `event.label` | `on enter gives 2 arguments (zone, object); this handler names 3.` `core is an object; on enter takes a zone label.` | none |

**Hover** shows a type on every name: `lead: player?`, and
`lead: player (not none here: checked on line 41)` where it is narrowed.
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
(`inside(p: player, z: zone) -> bool`).

**Completion** asks the checker for the type wanted at the caret
(`Checked::wanted_at`) and ranks by it; nothing is hidden. In
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
with the start check.

- **Map.** A scenario has `map: {things: [{id, label, class, number?,
  team?}]}`, with `class` one of `object`, `flag`, `volume`, `spawn`.
  `workbench::run` puts the things in `fake::FakeGame` (`labelled`,
  `numbers`, `object_teams`) and runs `labels::check` first; a failure is
  a list of problems of kind `map` with the refusal texts, and no run.
  `h2 rules run --scenario` reads the same field (`rules::scenario`).
- **The page** (`web/src/stats/RulesTest.tsx`) lists the map's things
  under the players, grouped by label, and shows each missing
  requirement as a problem with a button: **Add what the script needs**
  fills the map from the declarations (one of each single label, the
  minimum of each set, each number, one per team). An event's zone or
  object is picked by label and number ("capture 0"), not by id.
- **The check report** (`workbench::CheckReport`) gains `labels`: each
  declared label's name, map text, kind, count range, numbering, `per
  team` and its sentence in Forge's words, read from the IR's label
  table. The page and the variant page show it as "What a map needs".
- **Insert.** The Declarations menu (`web/src/lib/rulesIde.ts`,
  `DECLARATIONS`) has a Labels group: one zone, one object, a set of
  objects, a numbered set of zones, spawn areas, and the built-in flag
  labels from the completion route. A second group, "In the test map",
  offers a declaration for each scenario label the script has not
  declared, with the kind its class suggests.

## The IR and compatibility

Types are erased. The IR's ops, register types and type codes do not
change. IR version 2 changes two tables and the VM reads both versions:

| Table | Version 1 | Version 2 |
|---|---|---|
| label | text | text; kind (0 any, 1 object, 2 zone, 3 spawn); `min` and `max` members, `max` at most 256; flags (numbered, number range, per team); the number range |
| tag | path | path; group (0 any object, 1 weapon) |

- `Ir.labels` is a list of `LabelDef`. A version 1 blob decodes to kind
  0, 0 to 256, no flags, and encodes back to its own bytes, so its
  SHA-256 stands. `ir::IR_VERSION` is 2; `verify` accepts 1 and 2.
- The verifier checks, for version 2: a known kind and flags, `min <=
  max <= 256`, an ordered number range of at most 256, label text in
  `forge_doc::label_char`, no text twice, and no `ForEach` over a
  `spawn` label. `SetBase::Labelled(l)` is bounded by the label's `max`
  in the worst-tick estimate, and `Vm::members` takes at most `max`, so
  the bound holds on any map, checked or not.
- A front end that writes IR directly declares its labels the same way:
  Firefight's `ZONE_LABEL` (`crates/formats/firefight/src/script.rs`) is
  kind object.
- API version 2 adds the `fmt_*` entries ("Messages"), and nothing else.
- A build that reads version 2 advertises `rules/2` beside `rules/1`. An
  older guest of a version 2 game is warned about, not refused, by
  rules-engine.md's "Versioning" rule: it runs no script.

**Published scripts.** A blob is named by its hash and no blob changes,
so every variant record, `.h2variant` file, hopper slot and film that
names a version 1 blob still loads and plays. Two things must hold for
that, and both land before the language changes:

- **Live keeps the blob.** `GET /v1/rules/{hash}/ir` compiles the saved
  source again (**Code**, `crates/live/live-server/src/rule_scripts.rs`),
  which a changed compiler breaks. A save stores the IR bytes in its
  event beside the source, the route serves the stored bytes, and each
  script saved before that gets a server-signed `RulesIr` event with the
  blob the language 1 compiler gives, in both homes.
- **The build keeps the built-ins' version 1 blobs.** The built-ins and
  examples (`rules::builtin`) compile to new blobs with new hashes. Their
  version 1 blobs are files in the crate, found by hash as a built-in
  is, so a variant made from one still starts.

A language 1 source is kept as text and is not compiled by any build.
Opening one in the workbench shows it read-only with **Update to
language 2**.

### Migration

`rules::lang::migrate` (new, pure, source to source) does the mechanical
part: it adds `language 2`; collects every label string and writes a
declaration for it (a set, `numbered` when `.number` is read, `spawns`
when it is only passed to `spawn_areas`, `zones` when a member reaches
`inside`, `ticks_held` or an `enter` test); rewrites `objects labelled
"x"`, `labelled "x"` and `.label ==` tests; adds `?` to handle
variables; and turns `variant.engine == 9` into `carrier.assault`. What
is left are the checker's `null.*` and `unit.*` errors, each with its
fix. `h2 rules migrate <file>` and the workbench's button call it.

The 12 script files in `crates/formats/rules/src/builtin` are migrated
by hand from its output. The parts that change most:

```text
// ctf.rules, language 1
team var spawn: object = none
for each objects labelled "ctf_flag_spawn" s { claim_spawn(s) }
let back = (mine.plate != none and inside(p, mine.plate)) or (mine.plate == none and inside(p, mine.spawn))
let f = o.team
f.at_home = true

// ctf.rules, language 2: claim_spawn, claim_plate, team var spawn and
// plate, and global neutral go
label flag_spawn: zones at most 16 = "ctf_flag_spawn"
label flag_return: zones at most 16 = "ctf_flag_return"
label bomb_spawn: zones at most 16 = "assault_bomb_spawn"
label bomb_plate: zones at most 16 = "assault_bomb_return"

fn touching(p: player, z: zone?) -> bool = z != none and inside(p, z)

let mine = p.team
let back = touching(p, flag_return[mine]) or (flag_return[mine] == none and touching(p, flag_spawn[mine]))
if let f = o.team { f.at_home = true }
let bomb = variant.engine == carrier.assault
let neutral = first(bomb_spawn s where s.team == none)
```

The four sets require no members, because one script serves CTF and
Assault and a map needs only its own game's flags. `at most 16` is a
map requirement and the loop bound: three lookups an objective a tick
at the default 256 would not fit the tick budget.

```text
// territories.rules, language 1
object var owner: player = none
object var progress: int = 0
for each objects labelled "territory" m { ... }
for each objectives m {
    let lead = first(players p where p.alive and inside(p, m))
    if lead == none { m.progress = 0 } else { ... m.owner.team == lead.team ... }
}
pay = seconds(score_seconds)

// territories.rules, language 2 (score_time is a time option)
label territory: zones 1..8
object var owner: player? = none
object var progress: time = 0t

fn same_side(a: player, b: player) -> bool = (variant.team_play and a.team == b.team) or (not variant.team_play and a == b)
fn held_by(owner: player?, lead: player) -> bool = owner != none and same_side(owner, lead)

for each territory m { ... }
for each objectives m where m is territory {
    if let lead = first(players p where p.alive and inside(p, m)) {
        let rivals = count(players q where q.alive and inside(q, m) and not same_side(q, lead))
        if rivals > 0 or held_by(m.owner, lead) { m.progress = 0t } else { ... }
    } else {
        m.progress = 0t
    }
}
pay = score_time
```

```text
// siege.rules, language 1
for each objects labelled "barrier" b where b.number < n { delete(b) }
spawn_areas(team 0, labelled "attack", n)
if n == 2 { create("<core tag path>", first(objects labelled "core")) }
let zone = first(objects labelled "zone" z where z.number == phase)
if zone != none and ticks_held(zone, team 0) >= seconds(capture_time) { open(phase + 1) }
on enter(zone, o) {
    if phase == 2 and zone.label == labelled "goal" and is_objective(o) { end_round(team 0) }
}

// siege.rules, language 2 (capture_time is a time option, "Time")
const last_phase = 2
label barrier: objects numbered
label attack: spawns numbered 0..2
label defend: spawns numbered 0..2
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

`koth.rules` declares `label hill_corner: zones numbered` and reads
`hill_corner[hill]`; `oddball.rules` `label oddball_spawn: objects`;
`race.rules` `label checkpoint: zones numbered` and
`label race_start: spawns`, with `on enter(checkpoint z, o)`. The six
scripts with no label change only where a handle may be `none`
(`juggernaut.rules`' `crown(p: player?)`) and where a count is `time`.
The scripts' tests (`crates/formats/rules/tests`) keep the calls and
scores they expect, and the map texts they place.

## Rules and limits

- New keywords: `label`, `language`, `const`, `match`, `is`. `labelled`
  stays a lexer word so its error can name it. Type and kind words
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

The slices start after the lane that changes `message(to, text)`, the
scenario events and the context policy has landed. Slice 0 is deployed,
and its backfill confirmed in both homes, before slice 2 reaches
`master`, because Live deploys from `master`. Paths are under
`crates/formats/rules/src` unless they start with `crates/` or `web/`.

| # | Slice | Owns | Tests | Done when | After |
|---|---|---|---|---|---|
| 0 | Keep published blobs | `crates/live/live-server/src/rule_scripts.rs`, `crates/live/live-proto/src/rules.rs`, `crates/live/live-server/tests/rule_scripts.rs`; `builtin/v1/*.rir`, `builtin/mod.rs` (the table of version 1 blobs) | a save stores the blob and the route serves those bytes with the compiler replaced by one that fails; a log of older saves gains one `RulesIr` each, once; each `v1` blob hashes to what `compile` gives for its source | no route compiles a saved source; every saved script has a stored blob in both homes | the message lane |
| 1 | IR version 2 and the start check | `ir.rs`, `verify.rs`, `vm.rs`, `ty.rs`, `labels.rs` (new), `crates/formats/firefight/src/script.rs`; one line of `lang/lower.rs` (labels as kind 0) | version 1 blobs round-trip byte for byte and verify; each new verifier refusal; `members` stops at `max`; `labels::check` gives each refusal text from a hand-built census; the IR fuzz seeds cover both versions | every existing test passes with the compiler still writing kind 0 labels | 0 |
| 2 | The checker | `lang/check.rs`, `lang/sig.rs`, `lang/types.rs` (new); `lang/ast.rs`, `lang/parse.rs`, `lang/lex.rs`, `lang/lower.rs`, `lang/mod.rs` | one test per diagnostic code for labels, `T?` and narrowing, `zone`, `weapon`, `is`, handlers by label and `language`; `sig` erases to `caps::ENTRIES`; a script with five errors reports five; a snapshot of the IR for three fixtures | a language 2 script with labels compiles to a version 2 blob whose label table carries its declarations; `lower.rs` makes no type decision | 1 |
| 3 | Host census | `crates/extensions/game/src/rules/runtime.rs`, `crates/extensions/game/src/rules/guest.rs`; `crates/formats/forge-doc/src/labels.rs` (a census from `Labels` and the flag types) | over invented guest memory and a Forge document: a map that lacks `goal` refuses with its text and the game keeps the carrier's rules; `rules.state` shows the reason | the runtime runs the start check after `verify::load` | 1 |
| 4 | Scenario maps | `scenario.rs`, `workbench.rs`, `fake.rs`; the `run` and `check` wire types in `crates/live/live-proto/src/rules.rs` and their use in `rule_scripts.rs` | a scenario with a map runs Siege's phases on the fake; a missing label is a `map` problem and no run; the check report lists labels from a hand-built version 2 blob | `h2 rules run` and `POST /v1/rules/run` take `map`; the report has `labels` | 1 |
| 5 | Time, constants, match, value functions | the `lang/` files of slice 2 | one test per diagnostic code; option bits of a `time` option equal an `int` option's of the same range | the fixtures of "Stronger typing" compile or fail with the texts on this page | 2 |
| 6 | The editor's Rust half | `ide/*`, `crates/apps/cli/src/rules_lsp.rs`; the `ide` route's `actions` and `rename` in `rule_scripts.rs` | hover, completion order in `inside(p, ` and after `is`, tokens, definition, references and rename for a label; each fix applied gives a script without that error; `ide/model.rs` holds no type inference | `rules::ide` reads `Checked`; `h2 rules lsp` answers `codeAction` and `rename` | 2; beside 5 |
| 7 | Migration and the built-ins | `lang/migrate.rs` (new), `builtin/*.rules`, `builtin/mod.rs`, `crates/formats/rules/tests/*`, `crates/apps/cli/src/rules_cmd.rs`, `.claude/skills/h2-rules/*` | each language 1 built-in, migrated, differs from the hand-written language 2 file only where a `null.*` or `unit.*` fix was applied; every built-in compiles, verifies, fits the budget and passes its existing scenario tests; the formatter round-trips each | all 12 files are language 2; `h2 rules migrate` exists | 5 |
| 8 | The web workbench | `web/src/lib/rules.ts`, `web/src/lib/rulesIde.ts`, `web/src/stats/rulesMonaco.ts`, `web/src/stats/RulesTest.tsx`, `web/src/stats/Rules.tsx`, `web/src/stats/RulesEditor.tsx` and their tests | pure-half tests: the scenario map round-trips, "Add what the script needs" satisfies the report's labels, events name things by label, the Insert groups; a code action applies its edits | the page shows map problems, quick fixes, rename and "What a map needs" | 4, 6 |
| 9 | Message templates | `lang/check.rs`, `lang/lower.rs` (the template), `caps.rs` (the `fmt_*` entries), `verify.rs` (the run rule), `vm.rs` | each `text.hole` error; a `fmt_*` run without its `message`, or with the wrong types, is refused; the fake logs a message with its arguments | `message(to, "{p} scored")` reaches the fake with the player | 7 |

Slices 2, 3 and 4 run side by side, then 5 and 6, then 7 and 8. Slices
0, 4 and 6 each edit `rule_scripts.rs`, and 0 and 4 `live-proto`'s
`rules.rs`; they are in different waves. Slice 9 edits files of slices
1 and 5 and runs last.

Slices 0, 1, 3 and 4 each leave the tree green and land by themselves.
Slices 2, 5, 6 and 7 share one integration branch and reach `master`
together: from slice 2 the compiler refuses language 1, so the
built-ins and the crate's script tests do not compile until slice 7
has rewritten them. Slices 2 and 5 are tested on language 2 fixtures of
their own. The parser reads the language 1 label forms throughout, so
the checker can name them and the migrator can rewrite them.

## Not verified

- That a netgame flag of every traced type has a volume `inside` can
  test. The flag class satisfies `zone` on that assumption;
  rules-engine.md lists the zone tests as not built.
- Whether a deleted labelled object leaves its label set on a host. The
  fake keeps what the scenario lists.
- The lobby's check needs the map's census before the map loads: from
  the Forge document and the scenario's flag block. No reader gives it
  to the lobby.
- How the in-game variant editor shows a `time` option's unit.
- The number of language 1 scripts saved on Live, which sizes slice 0's
  backfill.

## Related

- [rules-engine.md](rules-engine.md): the engine, IR, VM, API and workbench.
- [forge-design.md](forge-design.md), [forge-features.md](forge-features.md): the Forge document and what a label sits on.
- [live-api.md](../live/live-api.md#rules-scripts): the routes slices 0, 4 and 6 change.
- [web-app.md](../live/web-app.md#game-type-scripts): the workbench's layout and copy.
