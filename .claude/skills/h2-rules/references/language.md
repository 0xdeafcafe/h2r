# Rules language cheat sheet

Taken from `crates/formats/rules/src/` (`lang/parse.rs`, `lang/lower.rs`,
`ir.rs`, `ty.rs`, `caps.rs`). `h2 rules caps` prints the entry table from
the same source. Where this page and
[the design](../../../../docs/game/rules-engine.md) differ, the code wins.

## File shape

```text
rules "Name" 1            // name 1 to 128 bytes, version 0 to 65535
type slayer               // required: the game type
teams 2                   // optional, 1 to 8
points "Kills"            // optional, 1 to 32 bytes; the type's word by default
uses slayer               // optional: play that built-in game's rules
requires player.weapons, hud.message   // optional, at most 32, no repeats
```

Then any order: `const`, `option`, `palette`, `enum`, `label`, `var`, `table`,
`fn`, `on`.
`//` starts a comment. A statement ends at a newline, `;` or `}`. Strings
take `\"`, `\\` and `\n`. A number with a fraction (`0.5`, `1.25`) is a
`fixed`; a whole number is an `int`. A number followed by `s` or `t` is a
`time`: `5s`, `300t`, `1.5s` (a whole number of ticks, 30 a second).

Keywords (not usable as names): `rules teams requires option
palette enum global var table fn on if else for each where let and or not
const match true false none label is text budget`. Field and enum-variant names may be
keywords (`enum Role { none, vip }`). Type and kind words (`zone`,
`weapon`, `objects`, `numbered`, `per`) are read by place and stay names.

Types (`rules::types::TABLE`): `slayer ctf assault oddball koth juggernaut
territories` (the retail ones, each with a built-in script `uses` may name)
and `research party firefight siege`. `type`, `uses` and `points` are header
words only at the start of a line; in a handler they are names. A script
with `uses` holds header lines, `settings`, `quick` and `about` only.
`carrier`, `list` and `scoring` lines are refused (`header.retired`).

## Types

| Type | Meaning |
|---|---|
| `int` | 32-bit, saturating |
| `fixed` | 16.16; literals like `0.5`; `*`, `/` work, `%` takes ints only |
| `bool` | `true`, `false` |
| `player`, `team`, `object` | handles; never `none` (see `T?`) |
| `zone` | an object with a volume (a Forge trigger volume or a netgame flag); goes where an `object` is wanted, not the reverse |
| `squad` | a kit squad slot 0 to 15, `squad(n)` |
| `time` | a duration in ticks: `5s`, `300t`; `seconds(n)` from an `int`, `whole_seconds(t)` and `ticks(t)` to an `int` |
| `timer` | a global variable of ticks left; 0 is stopped; read and written as `time` |
| `tag` | a tag path string (a table column or `set_loadout` argument) |
| `weapon` | a tag path that must be a weapon's (checked against the weapon group at load); goes where a `tag` is wanted |
| `T?` | `player?`, `team?`, `object?`, `zone?`, `tag?`, `weapon?`: a handle that may be `none` |
| `Class` | a kit class name string, for `place_squad` and `on actor_kill` |
| `text` | a string-table index: `text 3` or a string |
| enum | declared with `enum Name { a, b }`, read as `Name.a` (1 to 64 variants, at most 32 enums) |

Literals: `team 0` to `team 7` (a team), `text 9`, `none`. A string
literal takes the type its place wants (a tag, a weapon, a class or a
text); a string is never a label. Compare any two values of one type with `==` and `!=`;
`<`, `<=`, `>`, `>=` take `int`, `fixed` and `time` only. `time` adds,
subtracts, takes `%` and compares with `time`, multiplies and divides by
an `int`, and `time / time` is an `int`; any other mix of `time` and
`int` is an error that gives the fix (`Write 1s for a second or 30t for
ticks.`). `and`, `or`, `not` take
bools and short-circuit; `and` binds tighter than `or`; `not` is below
comparisons. Precedence, lowest first: `or`, `and`, `not`, comparison,
`+ -`, `* / %`, unary `-`, then `.field`, `[index]`, calls.

## Declarations

| Form | Notes |
|---|---|
| `const name = expr` | folded at compile time from `int` and `time` literals, other constants, `len(table)` and `+ - * / %`; stands wherever a literal does (option ranges and defaults, initial values, table cells, label counts) |
| `option name: int lo..hi = d "Label" "Description"` | an `int` option needs a range; also `time lo..hi` (whole seconds: `time 5s..60s = 20s`, the bits of the `int` option of the same range, label ending ` (seconds)`), `bool` and an enum type; label and description are optional strings on the same line; at most 32 options and 32 bits in all; read as a bare `name`, read-only |
| `palette "name" = ["tag/path", ...]` | 1 to 64 tags; used by `grant`; at most 255 palettes |
| `enum Name { a, b, c }` | commas or newlines |
| `global var x: type = const` | the initial value is a constant; none means the type's zero (`none` for handles) |
| `player var`, `team var`, `object var` | per-owner variables, read as `p.x`, `t.x`, `o.x` |
| `table name: (c1: type, c2: type) = [(v, v), ...]` | at most 16 columns and 256 rows; constants only; no timers |
| `fn name(a: type, b: type) { ... }` | inlined; no return value; no recursion; nests at most 16 deep; a parameter is not a `timer` |
| `fn name(a: type) -> type = expr` | a value function: one expression, inlined at each call; called only for its value |
| `label name: kind requirements = "map text"` | a Forge label the map supplies; below |
| `on event(args) { ... }` | several handlers of one event run in source order; trailing arguments may be left out; naming more than the event gives is refused |

Variable limits: 64 global, 16 per player, 8 per team, 8 per object (at
most 256 objects of a label hold object variables). A variable may not share
the name of a built-in field of its owner (`p.team`, `p.score`, ...) or of
an option. Scope words are `global`, `player`, `team`, `object`.

Tables: `waves[wave]` indexes a row by an `int`; `points[class]` finds the
first row whose first column holds that value (and reads zero values for a
key no row holds). Read a column as `waves[i].gap`. `let w = waves[i]` then
`w.gap` works. A row out of range is a script error. `len(table)` is the row
count.

## Labels

```text
label core: object                          // exactly one; `object?` if the map may have none
label goal: zone
label barrier: objects                      // a set; `zones`, `spawns` likewise
label capture: zones numbered 0..1 = "zone" // map text "zone"; numbers 0 and 1 each present
label flag_spawn: zones at most 16 per team
label race_start: spawns                    // starting locations, for spawn_areas only
label run_in: path                          // a flight: points numbered from 0, at least 2, for fly only
```

Requirements on a set: `at least n`, `at most n`, `exactly n`, `n..m`,
`numbered` (every member numbered), `numbered a..b` (each number in the
range present, no other), `per team`. A game whose map breaks a
declaration does not start. At most 64 labels.

A label is a typed name: `core` is an `object`; `for each barrier b`;
`count(barrier)`; `capture[n]` is the member numbered `n` and
`flag_spawn[t]` the member of team `t`, each `zone?` (a plain `zone` when
`n` is a constant inside `numbered a..b`); `x is goal` tests membership
and narrows `x` to a `zone`. A handler may pick its things by label:
`on enter(goal, o)`, or `on enter(capture z, o)` to name the zone. An
undeclared name is an error with the nearest label offered.

## `T?` and narrowing

The killer of `on kill`, `first(...)`, `o.carrier`, `o.team`,
`player_of(o)`, `capture[n]` and a handle column read by key are `T?`. A
handle variable that starts as `none` is declared `T?` (`global var
juggernaut: player? = none`). A `T?` goes where a `T` is wanted only after
a check: `if killer != none { ... }`, the `else` of `killer == none`,
through `and`, `or` and `not`, or `if let k = killer [and cond] { ... }`.
A `let`, a parameter, a loop member and a handler argument narrow; a
global never does (copy it with `if let`).

## Statements and expressions

```text
let name = expr                 // not reassignable
name = expr                     // a global var; or  p.var = expr  for a scoped var
if cond { ... } else if cond { ... } else { ... }
for each players p where cond { ... }
for each teams t { ... }
for each objectives o { ... }                 // the objective registry, at most 8
for each barrier o where cond { ... }        // a declared objects or zones label
if let k = killer and k != victim { ... }    // binds a T? as T, then tests
for each pick(players, n) p { ... }           // n members, drawn with the VM generator
match victim.role {                           // an enum value
    Role.vip { ... }
    Role.a, Role.b { ... }                    // several variants in one arm
    else { ... }                              // the rest; without it every variant needs an arm
}
```

A bare name in a `where` over `players`, `teams` or a label's set also
reads that member's variables (`count(players where not infected)`), and a
set argument may name its member: `count(players p where p.team == t)`.

Set-valued built-ins, only with a set as their first argument:
`count(set)` (int), `first(set)` (member or `none`, in `for each` order),
`pick(set, n)` (a set, only inside `for each` or `count`).

Value built-ins: `clamp(v, lo, hi)`, `min(a, b)`, `max(a, b)` (all `int`s or all `time`s), `len(table)`,
`seconds(n)` (n x 30 ticks, a `time`), `whole_seconds(t)`, `ticks(t)`, `squad(n)`, `now()` (the tick, a `time`), `random(n)` (a
draw modulo `n`; 0 with no draw for `n <= 0`).

Powers: `grant(p, forge.place, budget 20, palette "x")` (a palette is
optional), `revoke(p, forge.place)`, `granted(p, forge.place) -> bool`. The
capability must be in `requires` and grantable.

Fields (read through an owner):

| Owner | Fields |
|---|---|
| `player` | `team`, `alive`, `score`, `kills`, `deaths`, `lives` |
| `team` | `score` |
| `object` | `number`, `id`, `carrier` (`player?`), `team` (`team?`) |
| `squad` | `alive` |
| `variant` | `team_play`, `score_limit`, `time_limit`, `lives`, `respawn_time`, `friendly_fire`, `rounds`, `oddball_ball_count`, `king_moving_hill`, `ctf_flag_type`, `ctf_reset_time`, `ctf_touch_return`, `ctf_flag_at_home`, `assault_arm_time`, `territories_count`, `territories_contest_time` |

`variant.<field>` types are bool for `team_play`, `friendly_fire`,
`ctf_touch_return`, `ctf_flag_at_home` and int for the rest. A read through
`none` gives the slot's initial value; a write through `none` does nothing.

## Events

An `on` handler names its arguments in order; the count must match.

| Handler | Arguments | When |
|---|---|---|
| `on init` | | once, at the game's start |
| `on round_start` | | a round begins |
| `on round_end` | | a round ends |
| `on tick` | | every tick, after the timers |
| `on timer(t)` | a global `timer` var | `t` reaches 0 |
| `on kill(victim, killer)` | player, player? (a suicide has killer == victim) | a kill |
| `on spawn(p)`, `on death(p)` | player | |
| `on join(p)`, `on quit(p)` | player | |
| `on pickup(p, o)`, `on drop(p, o)` | player, object | |
| `on press(p, o)` | player, object | at most 10 a second a player |
| `on enter(zone, o)`, `on leave(zone, o)` | zone, object | at most 64 a tick together; a label in the zone's place picks the zones (`on enter(goal, o)`, `on enter(capture z, o)`) |
| `on squad_dead(s)` | squad | |
| `on actor_kill(killer, class)` | player?, Class | |
| `on actor_assist(killer, assist, class)` | player?, player?, Class | after its kill |
| `on bounce(p, spot, chain, monster)` | player, zone, int, bool | `chain` is its place in its chain (1 a single bounce); `monster` a Monster Bounce |
| `on bounced(p)` | player | a fall out of the map during a bounce |
| `on edit(p, o)` | player, object | a granted power was used |

A handler runs to completion. `on timer` handlers run in slot order, then
`on tick`.

## Entries: reads

Cap column is the capability to declare; reads need none. Status is
`named` (hostable), `traced`, `untraced`, `not built` (the last three make
a host refuse the script) or `vm`.

| Entry | Type | Status |
|---|---|---|
| `player_score(p)`, `kills(p)`, `deaths(p)`, `lives(p)` | player -> int | named |
| `team_score(t)` | team -> int | named |
| `alive(p)` | player -> bool | named |
| `team_of(p)` | player -> team | named |
| `round_time_left()` | -> time | named |
| `is_objective(o)` | object -> bool | named |
| `squad_alive(s)` | squad -> int | named |
| `carrier_of(o)` | object -> player | named |
| `zone_seen(o)`, `zone_distance(o)` | object -> bool, int | named (Firefight's `ff_drop` zones) |
| `object_team(o)` | object -> team | named |
| `player_of(o)` | object -> player | named |
| `label_number(o)`, `zone_id(o)` | object -> int | named |
| `ticks_held(o, t)` | object, team -> time | vm (below) |
| `inside(p, o)` | player, object -> bool | named (the host's zone test, below) |

The field forms (`p.score`, `o.number`, `o.id`, `o.carrier`,
`o.team`, `s.alive`) call these.

`ticks_held(zone, team)` is counted by the VM: from the first tick the
script names the zone, each tick the team holds it alone adds one. The
counts clear at the round's end; at most 32 zones are counted. `none` or
a team out of range reads 0. On a host the zone test is
`game::rules::world::zones`: a labelled flag is an upright cylinder
(radius 1.0, 0.5 under to 1.5 over), and three or more flags sharing a
label and a number (a hill's corners) are one polygon. A scenario's fake
game answers it from its `enter` and `leave` events.

## Entries: actions

| Action | Arguments | Cap | Order | Status |
|---|---|---|---|---|
| `score` | player, int | core | now | named |
| `end_round` | team (may be `none`) | core | now | named |
| `set_lives` | player, int | core | now | named |
| `respawn` | player | core | now | named |
| `register_objective` | object, team | core | now | traced |
| `spawn_objective` | tag, object, team | core | queued | traced |
| `reset_objective` | object | core | queued | untraced |
| `role_traits` | player, bool | core | now | untraced |
| `set_gravity` | fixed | `game.traits` | now | named |
| `set_team` | player, team | `player.team` | now | untraced |
| `set_loadout` | player, tag, tag | `player.weapons` | now | named (the multiplayer weapon list's tags only, `rules::weapons`) |
| `give` | player, tag | `player.weapons` | now | untraced |
| `take` | player, object | `player.weapons` | now | traced |
| `create` | tag, object | `objects.create` | queued | traced |
| `delete` | object | `objects.delete` | queued | named |
| `spawn_areas` | team, spawns label, int | `player.spawn` | now | named (labelled starting locations; a retail map has none) |
| `spawn_near` | team, zone (may be `none`), int | `player.spawn` | now | named (the `n` starting locations nearest the zone; `none` or 0 lifts it) |
| `set_body` | team, tag (may be `none`) | `player.disguise` | now | named (`objects/characters/masterchief/masterchief_mp` or `objects/characters/elite/elite_mp`; `none` gives the players their own) |
| `show_area` | zone, team (may be `none`) | none | now | named (the zone's boundary drawn in play in the team's colour, neutral for `none`; at most eight at once, a ninth is a script error; the round's end clears them) |
| `hide_area` | zone | none | now | named (takes the zone's boundary off) |
| `message` | player, team or `all`; text | `hud.message` | now | untraced |
| `place_squad` | squad, Class, int, label | `ai.squads` | queued | named |
| `place_squad_at` | squad, Class, int, int, int | `ai.squads` | queued | named |
| `hunt`, `erase_squad` | squad | `ai.squads` | queued | named |
| `fly` | object, path label, time | `vehicles.drive` | queued | not built (a vehicle flown along a `path` label in the stated ticks, facing its tangent; the host refuses it, by name) |
| `hover` | object, bool | `vehicles.drive` | queued | untraced |
| `seat` | player, object | `vehicles.drive` | queued | traced (the vehicle's first free seat, the driver's first) |
| `eject` | player | `vehicles.drive` | queued | traced |
| `place_squad_in` | squad, Class, int, object | `vehicles.drive` | queued | traced (`place_squad` seating the squad in the vehicle's free seats) |
| `unload` | object | `vehicles.drive` | queued | untraced |
| `vehicle_riders`, `vehicle_driver` | object | none | now | traced (reads: everyone aboard; the driver's player or `none`) |
| `forge_move` | object, object | `forge.move` | queued | named |
| `forge_delete` | object | `forge.delete` | queued | named |
| `forge_place` | tag, object | `forge.place` | queued | named |
| `forge_scale` | object, fixed | `forge.scale` | queued | named |
| `ff_round`, `ff_wave` | int | `firefight.director` | queued | named |
| `ff_skull` | int, bool | `firefight.director` | queued | named |
| `ff_lives` | player, int | `firefight.director` | queued | named |
| `ff_score` | player, int, bool | `firefight.director` | queued | named |
| `ff_respawn` | player | `firefight.director` | queued | named |
| `ff_finish` | bool | `firefight.director` | queued | named |

A `message` text written in the call may hold holes: `message(victim,
"{k} scored, {left} to go")`. A hole is one name in scope (a local, a
global, an option or a constant) whose value is a `player`, `team`,
`int`, `time` (shown as `m:ss`) or enum (shown by its variant's name); a
`player?` shows once narrowed (`if let k = killer { ... }`). At most four
holes; `{{` and `}}` write a brace. The compiler stores the text as
`{player} scored, {int} to go` and passes the values through VM-only
`fmt_player`, `fmt_team`, `fmt_int`, `fmt_time` and `fmt_enum` calls,
which a script cannot name. `h2 rules run` and the workbench list the
`message` call with the template and then the values; the fake game
(`FakeGame::messages`) also keeps the line a player reads
(`player 1 scored`).

An action has no value: `let x = score(p, 1)` is an error (`score has no
value`). A function call has none either. Passing `none` is allowed where
the type may be `none` (`set_loadout(p, "tag", none)`).

## Script errors at run time

The VM stops, the round ends with "script error", the same op every run:
the tick's 100,000 ops ran out (`Budget`), a table row out of range
(`TableRow`), more than 256 objects with object variables (`Objects`), more
than 128 queued calls (`Queue`), `squad(n)` outside 0 to 15 (`Squad`),
more than 32 zones asked about by `ticks_held` (`Zones`), a `set_body` tag
that is no player body (`Body`; one written in the call is a compile
error, `body.unknown`).
`h2 rules run` prints `script error <kind> at tick T, handler H, op P` and
exits 1.

## Common compile errors

| Message | Cause |
|---|---|
| `a script names its game type: type slayer` | no `type` line |
| ``carrier is gone: write `type koth`...`` | a retired `carrier`, `list` or `scoring` line |
| `with uses X the built-in game scores, ...` | handlers, variables or options in a `uses` script |
| `unknown capability X` / `capability X is not in requires` | misspelled name, or a `grant` of one not declared |
| `<entry> needs capability <cap>: add it to requires` | call without the declaration |
| `capability X cannot be granted` | only `forge.*` can be |
| `unknown name`, `unknown function`, `unknown variable` | a typo, or a name out of scope (inside a `fn`, handler locals are not visible) |
| `X is a let or a parameter and cannot be assigned` / `option X is read-only` | assigned a `let`, a parameter or an option |
| `a set is only allowed in for each, count, first and pick` | a set used as a value |
| `fn x calls itself; recursion is not allowed` | recursion |
| `an int option needs a range: int 1..5` | `option n: int = 3` |
| `can't compare time with int in "p.held >= 1". Write 1s for a second or 30t for ticks.` (`unit.mix`) | a `time` mixed with an `int`; also a `timer` set from an `int` option |
| `0.01s is not a whole number of ticks (30 a second).` (`unit.literal`) | a time literal between ticks |
| `match on Role has no arm for Role.guard. ...` (`match.missing`) | a `match` with no arm for a variant and no `else` |
| `ladder[12]: ladder has 12 rows, 0 to 11.` (`const.range`) | a constant index out of a table's rows or a label's `numbered` range, or a constant outside the `int` range |
| `division by zero in the constant "step".` (`const.zero`) | a constant divided by zero |
| `{q} in this message is not a name here. Names in scope: p, n.` (`text.hole`) | a message hole names nothing in scope; the other `text.hole` errors are a value a message cannot show (`an object can't be shown in a message.`), a `player?` not narrowed, a hole that is not one name (`{p.team}`), an unpaired brace, and more than four holes |
| `the options take N bits; a variant carries 32` | too many or too wide options |
| `a grant needs a budget: budget n` | `grant` with no `budget` |
| `on kill takes 2 arguments` | wrong argument count |
| `a second handler for on kill` | two handlers for one event |
| `expected the end of the line` | two statements on a line without `;` |
| `the handler has more than 32768 ops` / `256 registers` | one handler too large; split work across events or tables |
