# Rules world API: Firefight and Superbounce as scripts

Status: Design, with Superbounce's chain built. `content/modes/rules/superbounce.rules` keeps the medal chain's count and its scores on the host's physics events (`on launch`, `on grounded`); `firefight.rules` does not exist, and the Firefight sketch and the rest of the Superbounce sketch wait on the slices of [rules-world-api.md](rules-world-api.md#implementation-plan).
Code: `superbounce.rules`, with `crates/extensions/game/src/superbounce_chain_twin.rs` (the script beside the Rust chain in every chain scenario). The plan names `content/modes/rules/firefight.rules` (new). Tests: `crates/formats/firefight/tests` and `crates/extensions/game/src/superbounce_chain_tests.rs`, the modes' own.
Switch: none of its own; the `rules` feature of `H2_FEATURES` ([switches.md](../runtime/switches.md)).

Firefight's game rules run as a generated script today
([firefight.md](firefight.md#the-director-as-a-rules-script)); what is
engine-specific is its compiler and host, its HUD view and its entries.
Superbounce's landing judge and Bounced test are engine code, and its
chain's count and scores are a script on the host's physics events.
Over the world API of
[rules-world-api.md](rules-world-api.md) each is a script like any
other, written by hand over the generic entries. This page lists what
each needs from that API and sketches each
in the language as it is, with the new entries. The rule that must hold:
engine code is deleted only when a script gives the same output over
that code's own test scenarios.

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
| Firefight | the compiler that builds its IR and runs it (`crates/formats/firefight/src/script.rs`, `ScriptDirector`); the interface types (`src/director.rs`) and the kill-point math its tests check against (`src/scoring.rs`); the host and its log (`crates/extensions/game/src/firefight/host.rs`); the mode blob and HUD view (`firefight/hud.rs`); the sight pass | `ff_round`, `ff_wave`, `ff_skull`, `ff_lives`, `ff_score`, `ff_respawn`, `ff_finish`, `ff_target`, `ff_team_lives`, `ff_player_score`, `ff_team_finish`, `zone_seen`, `zone_distance`, `zone_team_distance`, `place_squad_at`; capability `firefight.director` |
| Superbounce | the landing judge, the fall through a strip's floor and the Bounced test of the medal chain, and the medals the HUD and the records show (`crates/extensions/game/src/superbounce_chain.rs`); the mode's HUD states and navpoint choice (`superbounce_mode.rs`, `superbounce_hud.rs`); the mode switch behind `hud.superbounce`, a capability that gates no entry | `on launch`, `on grounded`, `on bounced` (and `on bounce`, which Bounce Hunt and Bounce Practice use) |

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
type firefight
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
exact kill multipliers; firefight.md "The director"), and is generated
IR: `firefight::script::compile` builds the whole blob from a `Def`.
Slice W6 writes it by hand as `firefight.rules` with the wave set as its
tables, which `firefight::script::compile` then fills instead of building
the whole IR; the hand-written script must match the generated director
on every scenario of `crates/formats/firefight/tests` before the `ff_*`
entries and `firefight.director` are deleted.

## Superbounce

| The mode needs | From the API |
|---|---|
| a landing, with its speeds | `on touchdown(p, down, up)` (**built**, from the host while the mode runs) |
| a landing the host judged a bounce, its strip and its launch | `on launch(p, spot, speed)` (**built**) |
| the end of a run of bounces | `on grounded(p)` (**built**) |
| whether the player was primed the tick before | `p.primed`, kept in a player variable each tick |
| whether the landing was on a strip | the scanner's strips as a built-in label (`label strip: zones = "sb_strip"`), a label source beside the netgame flags in `rules/census.rs`; `distance` |
| chains and a fall out of the map | player variables, `on launch`, `on grounded`, `on bounced` (**built**: `superbounce.rules`) |
| the priming pill, the next spot's navpoint | `hud_show`, `mark` |
| medals | `medal` |

The built script (`content/modes/rules/superbounce.rules`) keeps the
chain on the host's verdicts: `on launch` adds one to the player's
`chain` while `flying` and starts it at 1 otherwise, `on grounded` clears
`flying`, and a launch over 16 world units a second is a Monster. Its
counts and scores equal the Rust chain's on every tick of
`superbounce_chain_tests.rs` (`superbounce_chain_twin.rs`). The sketch
below is the step after it, which judges a landing in the script from
`on touchdown`, `p.primed` and the strips as a label; it waits on the
reads' host half and the `sb_strip` label source.

```text
rules "Superbounce" 1
type research
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

- The generated Firefight director stays until slice W6 of the API page
  shows a hand-written script its equal on every scenario. The Rust
  medal chain stays while the HUD's medals, the records, the ghosts and
  the Pogo Stick read its awards and while the script cannot judge a
  landing against the strips. Nothing is deleted on a sketch.
- `firefight.rules` passes the verifier's worst-tick op and write
  estimates with a `ff_drop` label declared `at most 12`; the generated
  director of `firefight::script` does not fit today (firefight.md "The
  director as a rules script").
- Superbounce's `on tick` writes one HUD value and one waypoint per
  player, 32 writes with 16 players: the HUD cap of a tick exactly. A
  shipped script writes only on a change.

## Not verified

- That a hand-written director reproduces the generated director's zone
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
