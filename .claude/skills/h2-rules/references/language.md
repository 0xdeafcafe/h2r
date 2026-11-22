# Rules language cheat sheet

Taken from `crates/formats/rules/src/` (`lang/parse.rs`, `lang/lower.rs`,
`ir.rs`, `ty.rs`, `caps.rs`). `h2 rules caps` prints the entry table from
the same source. Where this page and
[the design](../../../../docs/game/rules-engine.md) differ, the code wins.

## File shape

```text
rules "Name" 1            // name 1 to 128 bytes, version 0 to 65535
carrier slayer            // required
teams 2                   // optional, 1 to 8
requires player.weapons, hud.message   // optional, at most 32, no repeats
```

Then any order: `option`, `palette`, `enum`, `var`, `table`, `fn`, `on`.
`//` starts a comment. A statement ends at a newline, `;` or `}`. Strings
take `\"`, `\\` and `\n`. A number with a fraction (`0.5`, `1.25`) is a
`fixed`; a whole number is an `int`.

Keywords (not usable as names): `rules carrier teams requires option
palette enum global var table fn on if else for each where let and or not
true false none labelled text budget`. Field and enum-variant names may be
keywords (`enum Role { none, vip }`).

Carriers (`caps::CARRIERS`): `ctf` 1, `slayer` 2, `oddball` 3, `king` 4,
`juggernaut` 7, `territories` 8, `assault` 9.

## Types

| Type | Meaning |
|---|---|
| `int` | 32-bit, saturating |
| `fixed` | 16.16; literals like `0.5`; `*`, `/` work, `%` takes ints only |
| `bool` | `true`, `false` |
| `player`, `team`, `object` | handles; may be `none` |
| `squad` | a kit squad slot 0 to 15, `squad(n)` |
| `timer` | a global variable of ticks left; 0 is stopped; read and written as `int` |
| `tag` | a tag path string (a table column or `set_loadout` argument); may be `none` |
| `Class` | a kit class name string, for `place_squad` and `on actor_kill` |
| `label` | `labelled "x"`, a Forge label |
| `text` | a string-table index: `text 3` or a string |
| enum | declared with `enum Name { a, b }`, read as `Name.a` (1 to 64 variants, at most 32 enums) |

Literals: `team 0` to `team 7` (a team), `text 9`, `labelled "hill"`,
`none`. A string literal takes the type its place wants (a tag, a label, a
class or a text). Compare any two values of one type with `==` and `!=`;
`<`, `<=`, `>`, `>=` take `int` and `fixed` only. `and`, `or`, `not` take
bools and short-circuit; `and` binds tighter than `or`; `not` is below
comparisons. Precedence, lowest first: `or`, `and`, `not`, comparison,
`+ -`, `* / %`, unary `-`, then `.field`, `[index]`, calls.

## Declarations

| Form | Notes |
|---|---|
| `option name: int lo..hi = d "Label" "Description"` | an `int` option needs a range; also `bool` and an enum type; label and description are optional strings on the same line; at most 32 options and 32 bits in all; read as a bare `name`, read-only |
| `palette "name" = ["tag/path", ...]` | 1 to 64 tags; used by `grant`; at most 255 palettes |
| `enum Name { a, b, c }` | commas or newlines |
| `global var x: type = const` | the initial value is a constant; none means the type's zero (`none` for handles) |
| `player var`, `team var`, `object var` | per-owner variables, read as `p.x`, `t.x`, `o.x` |
| `table name: (c1: type, c2: type) = [(v, v), ...]` | at most 16 columns and 256 rows; constants only; no timers |
| `fn name(a: type, b: type) { ... }` | inlined; no return value; no recursion; nests at most 16 deep; a parameter is not a `timer` |
| `on event(args) { ... }` | one handler per event (and per timer) |

Variable limits: 64 global, 16 per player, 8 per team, 8 per object (at
most 256 labelled objects hold object variables). A variable may not share
the name of a built-in field of its owner (`p.team`, `p.score`, ...) or of
an option. Scope words are `global`, `player`, `team`, `object`.

Tables: `waves[wave]` indexes a row by an `int`; `points[class]` finds the
first row whose first column holds that value (and reads zero values for a
key no row holds). Read a column as `waves[i].gap`. `let w = waves[i]` then
`w.gap` works. A row out of range is a script error. `len(table)` is the row
count.

## Statements and expressions

```text
let name = expr                 // not reassignable
name = expr                     // a global var; or  p.var = expr  for a scoped var
if cond { ... } else if cond { ... } else { ... }
for each players p where cond { ... }
for each teams t { ... }
for each objectives o { ... }                 // the objective registry, at most 8
for each objects labelled "x" o where cond { ... }
for each pick(players, n) p { ... }           // n members, drawn with the VM generator
```

A bare name in a `where` over `players`, `teams` or labelled objects also
reads that member's variables (`count(players where not infected)`), and a
set argument may name its member: `count(players p where p.team == t)`.

Set-valued built-ins, only with a set as their first argument:
`count(set)` (int), `first(set)` (member or `none`, in `for each` order),
`pick(set, n)` (a set, only inside `for each` or `count`).

Value built-ins: `clamp(v, lo, hi)`, `min(a, b)`, `max(a, b)`, `len(table)`,
`seconds(n)` (n x 30 ticks), `squad(n)`, `now()` (the tick), `random(n)` (a
draw modulo `n`; 0 with no draw for `n <= 0`).

Powers: `grant(p, forge.place, budget 20, palette "x")` (a palette is
optional), `revoke(p, forge.place)`, `granted(p, forge.place) -> bool`. The
capability must be in `requires` and grantable.

Fields (read through an owner):

| Owner | Fields |
|---|---|
| `player` | `team`, `alive`, `score`, `kills`, `deaths`, `lives` |
| `team` | `score` |
| `object` | `number`, `label`, `id`, `carrier`, `team` |
| `squad` | `alive` |
| `variant` | `team_play`, `score_limit`, `time_limit`, `lives`, `respawn_time`, `friendly_fire`, `rounds`, `engine`, `oddball_ball_count`, `king_moving_hill`, `ctf_flag_type`, `ctf_reset_time`, `ctf_touch_return`, `ctf_flag_at_home`, `assault_arm_time`, `territories_count`, `territories_contest_time` |

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
| `on kill(victim, killer)` | player, player (killer may be `none`; a suicide has killer == victim) | a kill |
| `on spawn(p)`, `on death(p)` | player | |
| `on join(p)`, `on quit(p)` | player | |
| `on pickup(p, o)`, `on drop(p, o)` | player, object | |
| `on press(p, o)` | player, object | at most 10 a second a player |
| `on enter(zone, o)`, `on leave(zone, o)` | object, object | at most 64 a tick together |
| `on squad_dead(s)` | squad | |
| `on actor_kill(killer, class)` | player, Class | |
| `on actor_assist(killer, assist, class)` | player, player, Class | after its kill |
| `on bounce(p, spot)` | player, object | needs the bounce detector, not built |
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
| `round_time_left()` | -> int | named |
| `is_objective(o)` | object -> bool | named |
| `squad_alive(s)` | squad -> int | named |
| `carrier_of(o)` | object -> player | named |
| `zone_seen(o)`, `zone_distance(o)` | object -> bool, int | named (Firefight's `ff_drop` zones) |
| `object_team(o)` | object -> team | traced |
| `player_of(o)` | object -> player | traced |
| `label_number(o)`, `label_of(o)`, `zone_id(o)` | object -> int, label, int | not built |
| `ticks_held(o, t)` | object, team -> int | vm, not built on a host (below) |
| `inside(p, o)` | player, object -> bool | not built |

The field forms (`p.score`, `o.number`, `o.label`, `o.id`, `o.carrier`,
`o.team`, `s.alive`) call these.

`ticks_held(zone, team)` is counted by the VM: from the first tick the
script names the zone, each tick the team holds it alone adds one. The
counts clear at the round's end; at most 32 zones are counted. `none` or
a team out of range reads 0. A host refuses a script that calls it until
the host has a zone test; a scenario's fake game answers it.

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
| `set_loadout` | player, tag, tag | `player.weapons` | now | traced |
| `give` | player, tag | `player.weapons` | now | untraced |
| `take` | player, object | `player.weapons` | now | traced |
| `create` | tag, object | `objects.create` | queued | traced |
| `delete` | object | `objects.delete` | queued | named |
| `spawn_areas` | team, label, int | `player.spawn` | now | untraced |
| `message` | player, team or `all`; text | `hud.message` | now | untraced |
| `place_squad` | squad, Class, int, label | `ai.squads` | queued | named |
| `place_squad_at` | squad, Class, int, int, int | `ai.squads` | queued | named |
| `hunt`, `erase_squad` | squad | `ai.squads` | queued | named |
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

An action has no value: `let x = score(p, 1)` is an error (`score has no
value`). A function call has none either. Passing `none` is allowed where
the type may be `none` (`set_loadout(p, "tag", none)`).

## Script errors at run time

The VM stops, the round ends with "script error", the same op every run:
the tick's 100,000 ops ran out (`Budget`), a table row out of range
(`TableRow`), more than 256 objects with object variables (`Objects`), more
than 128 queued calls (`Queue`), `squad(n)` outside 0 to 15 (`Squad`),
more than 32 zones asked about by `ticks_held` (`Zones`).
`h2 rules run` prints `script error <kind> at tick T, handler H, op P` and
exits 1.

## Common compile errors

| Message | Cause |
|---|---|
| `a script names its carrier: carrier slayer` | no `carrier` line |
| `unknown capability X` / `capability X is not in requires` | misspelled name, or a `grant` of one not declared |
| `<entry> needs capability <cap>: add it to requires` | call without the declaration |
| `capability X cannot be granted` | only `forge.*` can be |
| `unknown name`, `unknown function`, `unknown variable` | a typo, or a name out of scope (inside a `fn`, handler locals are not visible) |
| `X is a let or a parameter and cannot be assigned` / `option X is read-only` | assigned a `let`, a parameter or an option |
| `a set is only allowed in for each, count, first and pick` | a set used as a value |
| `fn x calls itself; recursion is not allowed` | recursion |
| `an int option needs a range: int 1..5` | `option n: int = 3` |
| `the options take N bits; a variant carries 32` | too many or too wide options |
| `a grant needs a budget: budget n` | `grant` with no `budget` |
| `on kill takes 2 arguments` | wrong argument count |
| `a second handler for on kill` | two handlers for one event |
| `expected the end of the line` | two statements on a line without `;` |
| `the handler has more than 32768 ops` / `256 registers` | one handler too large; split work across events or tables |
