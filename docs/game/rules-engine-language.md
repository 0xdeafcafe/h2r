# Rules engine: the language

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: syntax, types and statements of the `.rules` language.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## The language

A source file is a header, a settings block, options, variables,
functions and handlers. The header names the script, its version, its
game type (`type`, which files its variants in a row of the game type
list), the built-in game it plays if it plays one (`uses`), what a point
is called (`points`), its teams, its description, its Quick Options
marks and the capabilities it requires: `requires player.weapons,
hud.message`. The design is
[game-types-without-carriers.md](game-types-without-carriers.md#the-header).

```text
rules "SWAT" 1
type slayer
uses slayer
about "No shields, no motion sensor, battle rifles and magnums. One headshot kills. 50 points to win."
quick score_limit, starting_weapon, secondary_weapon

settings {
    team_play = true
    score_limit = 50
    shields = none
    starting_weapon = battle_rifle
    weapons_on_map = none
}
```

`type`, `uses`, `points`, `settings`, `quick` and `about` are header words only at the
start of a top-level line (`Parser::header_word` in `lang/parse.rs`), so
a handler may still use them as names. Each is given at most once.

**The type.** `type <word>` is required and given once. The word is
a row of the closed table in `crates/formats/rules/src/types.rs`
(`rules::types::TABLE`): `slayer`, `ctf`, `assault`, `oddball`, `koth`,
`juggernaut` and `territories` (the seven retail types) and `party`,
`firefight` and `siege` (ours). A row holds the word, the list
label, what a point is called when the script has no `points` line, where
the start line comes from (retail's announcer, one of ours, or none) and
the retail settings layout. A retail type names its engine's
index (ctf 1, slayer 2, oddball 3, koth 4, juggernaut 7, territories 8,
assault 9); our types use Slayer's layout, 2. `Header::engine` reads
it. Eligible Slayer-layout records use index 6 on disk and in play, except
a team script's, which stays on Slayer's index until the game's start
moves it (the retail lobby builds its team rows from the engine). The
type files a variant under its row of the game type list: `party` under
Party Games, and the other nine under their named rows. At round start the authority posts the
declared type's line, including when `uses` selects a different type's
handlers. See [the list](community-gametypes.md#tweakers) and
[the start event](game-types-without-carriers.md#the-display).
A script with no `type` line is the error "a script names its game type:
type slayer", and an unknown word is refused by name ("unknown type x: a
type is slayer, ctf, ..."). The IR header carries the type as one byte.

**Uses.** `uses <type>` names a retail type and says the game plays that
built-in game's rules under this script's settings (see "Who scores").
`points "Label"` is what a point is called in the scoreboard and the
announcer text, 1 to 32 bytes (`points is 1 to 32 bytes`); with no line
it is the type's.

**Retired lines.** `carrier X`, `list X` and `scoring X` are refused
with `header.retired`, and each message names the line that replaces it
(`carrier is gone: write `type slayer`...`, `list is gone: write `type
party`...`, `scoring carrier is gone: write `uses slayer`...`). `king`
is written `koth`.

**The settings block.** `settings { name = value ... }` names the record
settings a new variant of the script starts with, one a line; a setting
it leaves out has the stock value. The names are the rows of
`rules::settings::TABLE` (`crates/formats/rules/src/settings.rs`), the one
list of settings a script may name: each row has our snake-case name (a
setting a script reads as `variant.<name>` keeps that name), a label, a
kind, where it is stored (a record `SettingId`, the carrier's own score
setting for `score_limit`, CTF's and Assault's paired settings for the
`ctf_` names, or a field of the `hx` block for `gravity`, `damage`,
`shield`, `health`, `speed` and `unlimited_grenades`) and the carriers it applies
to. Rows are appended, never reordered, since a blob names a setting by
its index. The kinds:

| Kind | Written as | Example |
|---|---|---|
| bool | `true`, `false` | `motion_sensor = false` |
| int | a number in the row's range | `score_limit = 50`, `lives = 3` |
| seconds | a time in whole seconds | `time_limit = 300s`, `respawn_time = 3s` |
| choice | a value's name from the row's list, or a number in its range | `starting_weapon = shotgun`, `light_vehicle = none`, `weapons_on_map = 12` |
| percent | a number, 10 to 400 | `gravity = 50` |

`score_limit`, `time_limit` and `king_moving_hill` take `none` for 0.
A value may be a constant (`time_limit = hold * 2`). A game-type setting
carries its type in its name (`juggernaut_movement`,
`oddball_ball_speed`, `ctf_flag_type`) and applies on that type's record only.
The checker (`Cx::settings_block` in `lang/check.rs`) refuses a name not
in the table (`setting.unknown`, with the nearest name), one that does
not apply to the type (`setting.type`: "juggernaut_movement is a
Juggernaut setting; this script's record is Slayer's"), one named twice
(`setting.twice`) and a value not of the row's kind or outside its range
(`setting.value`).

**Quick Options marks.** `quick a, b, c` names what the lobby's Quick
Options shows, in order: at most five (`ir::MAX_QUICK`, `quick.many`),
each a record setting of the table that applies to the type (not an
`hx` field) or one of the script's options (`quick.unknown`), named once
(`quick.twice`). A marked setting need not be in the settings block.
Options and settings share one namespace: an option named like a setting
is `option.setting`.

**Who scores.** With no `uses` line the script is the game's one
scorer. `uses <type>` says the game plays that retail type's built-in
script (run on the host under the variant's record; "The host runtime")
and the script holds header lines, settings, `quick` and `about` only:
any declaration or handler is the error `uses.handlers`. A script with
no handler and no `uses` line scores nothing, the error `scoring.none`
(``this script scores nothing. Write `uses slayer` to play Slayer's
rules.``), which the checker reports once the script has no other error
(`Cx::scorer`).

**About.** `about "..."` is the description of a variant of the script,
at most 150 UTF-16 units (`ir::MAX_ABOUT_UNITS`, the `limit` error) and
256 bytes, the longest string literal.

**Modules.** `use "waves.rules"` on a line of its own at the top level
reads another file's lines in at that line before the checker runs
(`lang::modules::expand`, `lang::diagnose_files`, `lang::compile_files`).
The compile is of the one text the files make, so two files give the
blob of the same text in one file, and nothing after the expansion knows
a module. A name is a plain file name ending `.rules` (letters, digits,
`_`, `-` and `.`, at most 64 bytes, no directory), the files of a script
are one flat set, and a module has no `rules` header. A file is read
once however many files name it; a file that names one that is reading it,
directly or through others, is the error `use` naming both
(`a cycle: a.rules uses b.rules uses a.rules`), at the `use` line in the
file that closes it. The most is 32 files, 8 deep and 256 KiB in all (the
`limit` error). Where the files come from is the caller's: `h2 rules
check|build|run|sim|settings` reads the files beside the script, and a
variant file carries them in `script.modules`
([variants.md](../editor/variants.md)). A finding in a module has its
file (`Diagnostic::file`) and its line in that file; compiling one text
with a `use` is the error `use` ("has no file to read"). The editor's
diagnostics (`ide::diagnostics_with_files`, and `h2 rules lsp`, which
reads the files beside the document on disk) put a module's finding on
the `use` line, its message starting `file:line:column:`.

**Types.** `int` (32-bit, saturating), `fixed` (16.16, for seconds and
distances), `bool`, `player`, `team`, `object`, `zone` (an object with a
volume: a Forge trigger volume, or a netgame flag with its capture
volume), `squad`, `tag` (a tag path), `weapon` (a tag path that must be a
weapon's), `time` (a duration in ticks: `5s`, `300t`, `1.5s`), `timer`
(a global variable that counts ticks down, then fires `on timer`; it
reads and writes as `time`), and `enum` types a script declares. A zone is
an object and a weapon is a tag; nothing else converts. `time` adds,
subtracts and compares with `time`, multiplies and divides by `int`,
and `time / time` is an `int`; `seconds(n)` makes one from an `int`, and
`whole_seconds(t)` and `ticks(t)` make an `int` of one. Mixing `time`
and `int` is an error that gives the fix
(`can't compare time with int in "p.held >= 1". Write 1s for a second or 30t for ticks.`).
A `time` option is declared in whole seconds (`option capture_time: time
5s..60s = 20s`), takes the bits of an `int` option of the same range,
and its label ends ` (seconds)`. `variant` fields stay `int`. A handle that may be `none` is
`T?` (`player?`, `team?`, `object?`, `zone?`, `tag?`, `weapon?`); a `T`
never is. An `object` value is the engine's whole 32-bit datum: a real
object is a salted handle (salt in the high 16 bits, bit 15 set), so it is
at or above 2^31 and reads negative as an `int` register; `none` is
`0xFFFFFFFF` (`ty::NONE`, -1), and a labelled thing without an object uses
`0x4000_0000` plus its placement id (`ty::object_id`/`object_value` convert).
A `table` is constant rows of typed columns, indexed by `int`. There are no floats and no
strings at run time: text is a string-table index with typed arguments.
An `int` indexes a table's rows; a value of the first column's type
finds the first row holding it (`points[class]`), and a key no row holds
reads the columns' zero values. A row out of range is a script error.

**Expressions and control.** Ordinary expressions with precedence and
parentheses (`a = b + c * 2`), `if` / `else if` / `else`, `and` binding
tighter than `or`; both short-circuit. A statement ends at a line break,
`;` or `}`. A `let` or a parameter is not assigned again; a `let` may
state its type (`let heir: player? = none`), and `let x = none` with no
type is refused. `match` takes an enum value and runs the
arm of its variant; an arm may name several (`Role.a, Role.b { }`), and
a variant with no arm and no `else` arm is an error. These are the
things the ReachVariantTool dialect lacks ("Prior art").

**Constants and value functions.** `const last_phase = 2` and
`const hold = 20s` are folded by the checker from literals, other
constants, `len(table)` and `+ - * / %`, and stand wherever a literal
does: option ranges and defaults, initial values, table cells and label
requirements. A constant index out of a table's rows or a label's
`numbered` range is a compile error. A value function,
`fn rival(a: player, b: player) -> bool = <expression>`, is one
expression inlined at each call; a block `fn` has no value.

**`T?` and narrowing.** A value that may be `none` is a `T?`: the killer
of `on kill`, `first(...)`, `o.carrier`, `o.team`, `player_of(o)`, a
label's member by number or team, and a handle column read by key. A
variable of a handle type that starts as `none` is declared `T?`
(`global var juggernaut: player? = none`). A `T?` goes where a `T` is
wanted only where a check proves it is not `none`: inside
`if killer != none { ... }`, in the `else` of `killer == none`, through
`and`, `or` and `not` (`killer == none or killer == victim` leaves
`killer` a `player` in the `else`), and as the name `if let` binds
(`if let k = killer and k != victim { score(k, 1) }`, the name holding
in the condition after `and` and in the block). A check narrows a `let`,
a parameter, a loop member and a handler argument; a global is never
narrowed, as another handler may change it.

**Checking.** The checker (`crates/formats/rules/src/lang/check.rs`)
reads the whole script before anything is lowered and reports every
error it finds, up to 50, in source order, each with a code
(`null.arg`, `label.unknown`), and the warnings (`label.unused`,
`zone.delete`). Lowering (`lang/lower.rs`) runs on a script with no
error and makes no type decision. The codes are those of
[rules-language-types.md](rules-language-types.md#diagnostics-and-the-editor).

**Variables** are declared, named and scoped:
`global|player|team|object var name: type = init`. The compiler assigns
slots and enforces the budget (256 global, 64 per player, 32 per team, 32
per object, at most 256 objects of a label). A scoped variable is read
through its owner: `p.infected`, `t.captures`.

**Labels.** A script declares the Forge labels it reads, and the map
supplies them: `label <name>: <kind> [requirements] [= "map text"]`. The
kind is `object` or `zone` (one value; `object?` or `zone?` when the map
may have none), or `objects`, `zones` or `spawns` (a set; `spawns` are
starting locations, passed to `spawn_areas` and never visited). A set
may require `at least n`, `at most n`, `exactly n` or `n..m` members,
`numbered` (every member has a number) or `numbered a..b` (each number
in the range has a member and no other number appears), and `per team`
(each team in play has a member of its own). The map text is the
label's name unless `= "..."` gives it
(`label capture: zones numbered 0..1 = "zone"`). The declarations fill
the IR's label table in declaration order, which the start gate checks
a map against ([rules-language-types.md](rules-language-types.md#labels)).

A label is a typed name. `core` (declared `object`) is an `object`;
`for each barrier b` visits a set; `capture[n]` is the member numbered
`n` and `flag_spawn[t]` the member of team `t`, each a `zone?`, or a
`zone` when `n` is a constant inside the declared `numbered a..b`;
`count(checkpoint)` counts one; `x is goal` tests that `x` is a member,
and makes `x` a `zone` where it holds. A set label is passed where a set
is taken (`spawn_areas(team 0, attack, n)`, a `fn` parameter declared
`zones`). There is no string label: a label is declared or it is an
error, with the nearest declared name offered.

**Handlers.** `on init`, `on round_start`, `on tick`,
`on kill(victim, killer)`, `on spawn(p)`, `on death(p)`,
`on pickup(p, o)`, `on drop(p, o)`, `on enter(zone, o)`,
`on leave(zone, o)`, `on press(p, o)`, `on timer(t)`,
`on squad_dead(s)`, `on actor_kill(killer, class)`,
`on actor_assist(killer, assist, class)` (after its kill),
`on damage(o, by, amount)` (an object took damage; `by` is the player it
is credited to or `none`, `amount` a `fixed` fraction of full vitality;
at most 16 a tick),
`on bounce(p, spot, chain, monster)` (`chain` the bounce's place in its
chain, 1 for a single bounce; `monster` a Monster Bounce),
`on bounced(p)` (a fall out of the map during a bounce),
`on arrive(o)` (a `move_to` reached its end, on that tick),
`on touchdown(p, down, up)` (a player's biped landed; `down` the speed it
fell at the tick before and `up` its vertical speed now, `fixed` world
units a second; at most 16 a tick),
`on launch(p, spot, speed)` (a landing the host judged a superbounce, as
its launch window closed; `spot` the strip, a `zone?`, `none` off every
marked strip; `speed` the launch, `fixed` world units a second up; at
most 16 a tick),
`on grounded(p)` (the player's run of bounces with no ground between them
ended; at most 16 a tick),
`on edit(p, o)` (a granted power was used),
`on join(p)`, `on quit(p)`, `on round_end`. A handler names its event's
arguments in order and may leave out trailing ones (`on bounce(p, spot)`);
naming more is refused with what the event gives. The killer of
`on kill`, `on actor_kill` and `on actor_assist`, and the assist, are
`player?`; the zone of `on enter`, `on leave` and `on bounce` is a
`zone`. A label in the zone's place (or the object's place of
`on pickup`, `on drop`, `on press` and `on edit`) picks the handler's
things: `on enter(goal, o)` runs for the goal only, and
`on enter(capture z, o)` for a capture zone, which it names `z`. The
handlers of one event, by label or not, are one IR handler and run in
source order. A handler runs to completion.

**Loops** are `for each` over a finite set only: `players`, `teams`,
`objectives` (the objective registry's objects, at most 8, by object
handle) and a declared label's set (`for each barrier b`), each with an
optional `where`. A set
passed to `count`, `first` or `pick` may name its member for the `where`
(`count(players p where p.team == t)`). There is no
`while`, no recursion, and functions are inlined, so every handler has a
static worst-case op count, which the compiler prints.

**Names, not datums.** Engine things are named by tag path
(`"objects/weapons/..."`, checked against the map at load, a path used
as a `weapon` against the weapon group) or by a Forge label the script
declares. A script never sees a datum index or an address.

**Options** a script declares are the rows a variant editor shows for
it: `option name: type range = default`, with a label and a description
in the string table. `variant.<field>` reads a retail record field by
name: `team_play`, `score_limit`, `time_limit`, `lives`,
`respawn_time`, `friendly_fire`, `rounds`, `engine`, and the type-area
fields the ports read (`caps::VARIANT_FIELDS`, which names each one's
`record::SettingId` and offset): `oddball_ball_count`,
`king_moving_hill`, `ctf_flag_type`, `ctf_reset_time`,
`ctf_touch_return`, `ctf_flag_at_home`, `assault_arm_time`,
`territories_count`, `territories_contest_time`. The meanings of
`oddball_ball_count`, `ctf_reset_time`, `ctf_touch_return`,
`ctf_flag_at_home` and `territories_contest_time` are inferred from the
editor's page labels, not from a reader.
