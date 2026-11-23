# Rules world API: Firefight and Superbounce as scripts

Status: Design. `crates/formats/rules/src/builtin/superbounce.rules` exists (the mode's switch, its options and bounce scoring); `firefight.rules` does not, and the sketches below wait on the slices of [rules-world-api.md](rules-world-api.md#implementation-plan).
Code: `superbounce.rules` only. The plan names `crates/formats/rules/src/builtin/firefight.rules` (new). Tests: `crates/formats/firefight/tests/script.rs` and `crates/extensions/game/src/superbounce_chain_tests.rs`, the oracles' own.
Switch: none of its own; the `rules` feature of `H2_FEATURES` ([switches.md](../runtime/switches.md)).

Firefight and Superbounce are engine code today: a director, a medal
chain, a HUD view and entries only they use. Over the world API of
[rules-world-api.md](rules-world-api.md) each is a script like any
other. This page lists what each needs from that API and sketches each
in the language as it is, with the new entries. The rule that must hold:
a Rust oracle is deleted only when its script gives the same output over
the oracle's own test scenarios.

Everything here is **Design** unless labelled
([docs-style.md](../meta/docs-style.md)). The sketches use entries that
do not exist yet; the API page gives each one's capability, cost and
status. Both scripts are held to that page's
[limits](rules-world-limits.md): their
worst-tick writes fit the caps, and neither needs a capability a
matchmade hopper denies.

## What is engine code today

| Mode | Engine-specific code | Entries only it uses |
|---|---|---|
| Firefight | the director and scoring (`crates/formats/firefight/src/director.rs`, `scoring.rs`); the generator that builds its IR (`src/script.rs`); the host and its log (`crates/extensions/game/src/firefight/host.rs`); the mode blob and HUD view (`firefight/hud.rs`); the sight pass | `ff_round`, `ff_wave`, `ff_skull`, `ff_lives`, `ff_score`, `ff_respawn`, `ff_finish`, `zone_seen`, `zone_distance`, `place_squad_at`; capability `firefight.director` |
| Superbounce | the medal chain and its rule (`crates/extensions/game/src/superbounce_chain.rs`); the mode's HUD states and navpoint choice (`superbounce_mode.rs`, `superbounce_hud.rs`); the mode switch behind `hud.superbounce`, a capability that gates no entry | `on bounce`, `on bounced` |

## Firefight

| The mode needs | From the API |
|---|---|
| waves, rounds, gaps, the reserve and the cap | tables, timers and variables: the VM alone |
| a squad placed at an unseen, far zone | `place_squad_at`, `sees`, `distance` |
| squads sent at the players, erased in cleanup | `hunt`, `go_to`, `erase_squad` |
| waves by drop ship | `place_squad_in`, `fly`, `on arrive`, `unload` |
| kill points and assists | `on actor_kill`, `on actor_assist`, `score` |
| shared lives and respawn | `set_lives`, `respawn`, `on death` |
| skulls by round | `set_skull(id, on)`, a new entry of capability `game.skulls` over the skull flags Firefight's `Skull` command writes |
| round, wave, enemies left and lives on every HUD | `hud_text`, `hud_fill`: the show state replaces the mode blob |
| the wave's stinger and an Elite's taunt | `sound`, `say` |
| the end of the game | `end_round` |

```text
rules "Holdout" 1
carrier slayer
teams 1
requires ai.squads, vehicles.drive, hud.widgets, hud.message, audio.play

table waves: (class: Class, per_player: int, min: int, max: int, gap: time, by_air: bool) = [ ... ]
table points: (class: Class, score: int) = [ ... ]
const per_round = 5

label ff_drop: zones at least 6
label ship: object
label run_in: path
label run_out: path

hud wave_line: text at top_right
hud lives_line: text at under_score

option start_lives: int 1..20 = 7

global var wave: int = 0
global var pool: int = 7
global var next: timer = 5s

fn hidden(z: zone) -> bool = count(players p where p.alive and sees(p, z)) == 0

fn show() {
    let r = wave / per_round + 1
    let w = wave % per_round + 1
    hud_text(wave_line, "Round {r}, wave {w}")
    hud_text(lives_line, "Lives {pool}")
}

on init { pool = start_lives; show() }

on timer(next) {
    let w = waves[wave]
    let s = squad(wave % 16)
    let n = clamp(count(players) * w.per_player, w.min, w.max)
    if w.by_air {
        place_squad_in(s, w.class, n, ship)
        move_along(ship, run_in, 12s)
    } else if let z = first(ff_drop z where hidden(z)) {
        place_squad_at(s, w.class, n, z.id, 0)
        hunt(s)
    }
    message(all, "Wave incoming")
    show()
}

on arrive(o) {
    if o == ship {
        unload(ship)
        hunt(squad(wave % 16))
        move_along(ship, run_out, 12s)
    }
}

on squad_dead(s) {
    wave = wave + 1
    if wave == len(waves) { end_round(team 0) } else { next = waves[wave - 1].gap }
    show()
}

on actor_kill(killer, class) {
    if killer != none { score(killer, points[class].score) }
}

on death(p) {
    if pool > 0 { pool = pool - 1; respawn(p) }
    else if count(players q where q.alive) == 0 { end_round(none) }
    show()
}
```

The shipped director is larger (the reserve, cleanup, the zone ranking,
exact kill multipliers; firefight.md "The director"). It is written by
hand as `firefight.rules` with the wave set as its tables, and
`firefight::script::compile` fills the tables from a `Def` instead of
building the whole IR. The Rust director stays the oracle in tests until
the script has matched it on every scenario of
`crates/formats/firefight/tests/script.rs`; then the `ff_*` entries and
`firefight.director` are deleted.

## Superbounce

| The mode needs | From the API |
|---|---|
| a landing, with its speeds | `on touchdown(p, down, up)` |
| whether the player was primed the tick before | `p.primed`, kept in a player variable each tick |
| whether the landing was on a strip | the scanner's strips as a built-in label (`label strip: zones = "sb_strip"`), a label source beside the netgame flags in `rules/census.rs`; `distance` |
| chains and a fall out of the map | player variables, `on kill` |
| the priming pill, the next spot's navpoint | `hud_show`, `mark` |
| medals | `medal` |

```text
rules "Superbounce" 1
carrier slayer
list research
requires hud.widgets, hud.waypoints

label strip: zones = "sb_strip"

hud primed_pill: text at above_weapon
waypoint next_spot

option bounce_points: int 0..5 = 1
option monster_double: bool = true
const launch = 8.0
const monster = 13.0

player var was_primed: bool = false
player var chain: int = 0
player var flying: bool = false

on init { hud_text(primed_pill, "Primed") }

on touchdown(p, down, up) {
    if let s = first(strip z where distance(p, z) < 0.5) and p.was_primed and up > launch {
        p.chain = p.chain + 1
        p.flying = true
        let n = p.chain
        let points = bounce_points * min(n, 6)
        if up > monster and monster_double { score(p, points * 2) } else { score(p, points) }
        medal(p, "Superbounce x{n}")
    } else {
        p.chain = 0
        p.flying = false
    }
}

on kill(victim, killer) {
    if killer == none and victim.flying { score(victim, -1); medal(victim, "Bounced") }
    victim.chain = 0
    victim.flying = false
}

on tick {
    for each players p {
        hud_show(primed_pill, p, p.primed)
        if let s = first(strip z where distance(p, z) < 20.0) { mark(next_spot, s, p) } else { unmark(next_spot, p) }
        p.was_primed = p.primed
    }
}
```

The two speeds are placeholders for the values superbounce.md measures.
What stays Rust is what is not a rule: the scanner
(`crates/live/superbounce`), the floor marks and lock-on drawn over the
map (`superbounce_world.rs`), the fall assist, ghosts and the verifier.
The marks stay behind `hud.superbounce` until a script can draw lines in
the world, which this page does not design.

## Rules and limits

- The Rust director and the Rust medal chain stay, and stay the tests'
  oracles, until slices W6 and W4 of the API page show each script equal
  to its oracle on every scenario. Nothing is deleted on a sketch.
- `firefight.rules` passes the verifier's worst-tick op and write
  estimates with a `ff_drop` label declared `at most 12`; the generated
  director of `firefight::script` does not fit today (firefight.md "The
  director as a rules script").
- Superbounce's `on tick` writes one HUD value and one waypoint per
  player, 32 writes with 16 players: the HUD cap of a tick exactly. A
  shipped script writes only on a change.

## Not verified

- That a hand-written director reproduces the Rust director's zone
  ranking and exact kill multipliers inside the op budget.
- How many strips a map's scan yields against the 256-member bound of a
  label, and whether `distance` to each on every touchdown fits.
- The two speeds in the Superbounce sketch; superbounce.md has the
  measured ones.

## Related

- [rules-world-api.md](rules-world-api.md): the API, its limits and the slices
- [firefight.md](firefight.md): the director, its tests and the kit
- [superbounce.md](superbounce.md): the rule, the medals and the scanner
- [rules-engine.md](rules-engine.md#firefight-ai-and-bounces): the generated director and the bounce events these replace
