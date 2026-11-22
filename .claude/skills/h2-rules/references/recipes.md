# Recipes

Patterns that follow the built-in scripts in
`crates/formats/rules/src/builtin/*.rules`. Each is a fragment: add a
header (`rules "Name" 1`, `carrier slayer`, `requires ...`) around it, run
`h2 rules check`, then a scenario. Copy from a built-in when a recipe is
close; those files compile and have tests (`crates/formats/rules/tests/`).
Entries a host cannot make yet are marked; see
[the language page](language.md) for the status of each.

## Score on kill

The retail rule, from `slayer.rules`. `score` adds the team score and tests
the score limit, so the script has no limit check.

```text
option kill_points: int 1..5 = 1 "Kill points" "Points for each kill"
option suicide_costs: bool = true "Suicide penalty" "A suicide costs a point"

on kill(victim, killer) {
    if killer == none or killer == victim {
        if suicide_costs { score(victim, -1) }
    } else if variant.team_play and killer.team == victim.team {
        score(killer, -1)
    } else {
        score(killer, kill_points)
    }
}
```

Scenario: two players, a kill, a suicide, then a betrayal. A Slayer
carrier's stock record is free-for-all, so the betrayal branch needs
`"variant": {"team_play": 1}`.

```json
{"players": [{"slot": 0, "team": 0}, {"slot": 1, "team": 0}],
 "ticks": [{}, {"events": [{"kill": {"victim": 1, "killer": 0}}]},
           {"events": [{"kill": {"victim": 0, "killer": 0}}]}]}
```

Expected: player 0 ends on 0 (+1, then -1).

## Score for time held (a per-second counter)

A counter in a player variable, paid when it reaches a second. Oddball and
King of the Hill use this; the objective reads are not hostable yet.

```text
player var held: int = 0

on tick {
    for each players p where p.alive {
        p.held = p.held + 1
        if p.held >= seconds(1) {
            score(p, 1)
            p.held = 0
        }
    }
}
```

Eight `on tick` ops per player stay far inside the budget. Scenario: ask
for 31 ticks (`"ticks": [{}, {}, ...]`) and check the score is 1.

## A timer

A `timer` is a global of ticks left; it counts down, fires `on timer` at 0
and stays stopped until set again.

```text
option wave_seconds: int 3..15 = 8 "Wave" "Seconds between respawn waves"
global var wave: timer

on round_start { wave = seconds(wave_seconds) }

on timer(wave) {
    for each players p where not p.alive { respawn(p) }
    wave = seconds(wave_seconds)
}
```

Scenario: start the run, put a dead player in (`"alive": false`) and run
`wave_seconds * 30 + 1` ticks (write the `ticks` array with a one-line
script, not by hand). `h2 rules run` shows a `respawn` call on the
tick the timer fires. A round clock is the same shape:
`on timer(clock) { end_round(team 1) }`.

## Rounds and the end

`end_round(winner)` takes a team, or `none` for a draw. A scenario counts
`round_time_left()` down one a tick from the `round_time_left` it gives.
This is Infection's tail:

```text
on tick {
    if count(players where not infected) == 0 { end_round(team 1) }
    else if round_time_left() == 0 { end_round(team 0) }
}
```

Elimination between two teams:

```text
on tick {
    let a = count(players p where p.alive and p.team == team 0)
    let b = count(players p where p.alive and p.team == team 1)
    if a == 0 { end_round(team 1) } else if b == 0 { end_round(team 0) }
}
```

Reset per-round state in `on round_start`, not in a variable's initial
value: `for each players p { p.level = 0 }`. A run in the CLI does not stop
at `end_round`, so read `result:` for the tick it was first called.

Lives: `lives(p)` or `p.lives` reads them, `set_lives(p, n)` writes, and
`respawn(p)` brings a player back (all `core`, all hostable).

```text
global var pool: int = 7

on death(p) {
    if pool > 0 { pool = pool - 1; respawn(p) }
    else if count(players q where q.alive) == 0 { end_round(none) }
}
```

## Team play

Read `variant.team_play`, a player's `p.team`, and `t.score`. A team value
is `team 0` to `team 7`; a comparison of teams is `==`.

```text
on kill(victim, killer) {
    if killer != none and killer != victim and killer.team != victim.team {
        score(killer, 1)
    }
}

on tick {
    for each teams t where t.score >= 50 { end_round(t) }
}
```

`for each teams t` visits the teams in play (`teams n` in the header, or the
scenario's `teams`). Per-team state is `team var`, read as `t.x`.

## A role with powers

From `vip.rules`. A role is an enum in a player variable; a power is a
grant made when the role changes. Needs the capabilities in `requires`, a
palette of tag paths for what the player may place or touch, and a budget
on each grant. Grants are not hostable by the host runtime today, so this
is proven by scenario only.

```text
requires forge.move, forge.delete, forge.place
palette "vip" = ["objects/forge/block_wall/block_wall", "objects/forge/barrier/barrier"]
palette "wrecker" = ["objects/vehicles/warthog/warthog"]

enum Role { none, vip }
player var role: Role = Role.none

fn crown(p: player) {
    p.role = Role.vip
    grant(p, forge.place, budget 20, palette "vip")
    grant(p, forge.move, budget 40, palette "vip")
}

fn uncrown(p: player) {
    p.role = Role.none
    revoke(p, forge.place)
    revoke(p, forge.move)
}

on round_start {
    for each players p { uncrown(p) }
    for each teams t {
        let v = first(players p where p.team == t)
        if v != none { crown(v) }
    }
}

on quit(p) { if p.role == Role.vip { uncrown(p) } }
```

A grant ends at `revoke`, at quit and at the round's end, never exceeds
255 uses, and a ranked session refuses it. `on edit(p, o)` runs when a
granted power was used. A scenario can run only the grant bookkeeping:
`rules::Vm` exposes the proposal check to Rust tests
(`crates/formats/rules/tests/scripts.rs`), not to `h2 rules run`.

## A table-driven ladder

From `gun_game.rules`: a table gives per-rung data, and `min` and `len` keep
the index in range (a row out of range is a script error).

```text
table ladder: (weapon: tag) = [
    ("objects/weapons/rifle/smg/smg"),
    ("objects/weapons/rifle/battle_rifle/battle_rifle"),
    ("objects/weapons/pistol/magnum/magnum"),
]
player var level: int = 0

on spawn(p) { set_loadout(p, ladder[min(p.level, len(ladder) - 1)].weapon, none) }   // player.weapons

on kill(victim, killer) {
    if killer != none and killer != victim {
        killer.level = killer.level + 1
        score(killer, 1)
        if killer.level >= len(ladder) { end_round(killer.team) }
    }
}
```

A loadout per team is the same with `loadouts[p.team]`, a row found by a
`team` first column (see `siege.rules`).

## An objective

Objectives are the registry's objects (at most 8). `for each objectives b`
visits them, `is_objective(o)` tests one, `b.carrier` is the carrying
player (`none` when dropped). `oddball.rules` scores a carrier per second
and resets an idle ball; the structure:

```text
object var idle: int = 0
player var held: int = 0

on pickup(p, o) { if is_objective(o) { o.idle = 0 } }

on tick {
    for each objectives b {
        let p = b.carrier
        if p != none {
            p.held = p.held + 1
            if p.held >= seconds(1) {
                score(p, 1)
                p.held = 0
            }
        } else {
            b.idle = b.idle + 1
        }
    }
}
```

Two statements share a line only with `;`: `score(p, 1); p.held = 0`.
Spawning and returning objectives (`spawn_objective`, `reset_objective`) and
asking whether a player stands in a zone (`inside`) are not hostable yet;
the CTF, Oddball, King and Territories scripts call them and say so in
their headers. In a scenario, seat the objective with `pickup` events.

## Zones and labelled objects

Labelled objects come from Forge labels and the built-in per-flag-type
table. `for each objects labelled "hill_corner" c where c.number == hill`
visits them; `first(...)` finds one. `on enter(zone, o)` and `on leave`
report an object in a zone (at most 64 a tick together). A scenario
declares them in its `map` (`{"label": "hill_corner",
"object": 40, "number": 1}`; none, and the script's smallest map is used) and a player's unit in `unit`, so an `enter`
naming the unit puts the player in the zone. `ticks_held(zone, team)`
counts the ticks a team alone holds a zone. Expect `label_number`,
`label_of`, `zone_id`, `inside` and `ticks_held` to be refused by a
host.

## Randomness

```text
player var chosen: bool = false
global var roll: int = 0

on round_start {
    for each players p { p.chosen = false }
    for each pick(players, 1) p { p.chosen = true }
    roll = random(6)
}
```

`pick(set, n)` and `random(n)` use the VM's seeded generator; the scenario's
`seed` fixes the outcome, and the same seed gives the same draw on every
machine. `random(0)` is 0 with no draw.

## A scenario with options and a round time

```json
{
  "options": 0,
  "round_time_left": 900,
  "players": [{"slot": 0}, {"slot": 1, "team": 1}],
  "ticks": [{}, {}, {"events": [{"kill": {"victim": 1, "killer": 0}}]}, {}, {}]
}
```

`options` of 0 is every option at its `lo`, not its default; leave it out
to take defaults. A script with an option `phase_time: int 60..600` and one
`bool` has `ceil(log2(541)) = 10` bits plus 1: its raw bits are
`(phase_time - 60) | (flag << 10)`.
