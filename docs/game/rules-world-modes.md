# Rules world API: Firefight and Superbounce as scripts

Status: Firefight's director and Superbounce's medal chain are scripts. The generic world entries for Firefight's host and the script that judges landings against the Superbounce strips are designs.
Code: `content/modes/rules/firefight.rules` and `content/modes/rules/superbounce.rules`. Tests: `crates/formats/firefight/tests`, `crates/formats/rules/tests/firefight.rs` and `crates/extensions/game/src/superbounce_chain_tests.rs`.
Switch: none of its own; see the `rules` and `firefight` features in [switches.md](../runtime/switches.md).

The [world API](rules-world-api.md) supplies the entries scripts use to
read and change a match. Firefight's host supplies the kit and applies the
director's queued commands. Superbounce's host judges landings and feeds
the medal chain with physics events. The remaining designs move those
rules onto generic entries within the [world limits](rules-world-limits.md).

## What is engine code today

| Mode | Engine-specific code | Entries only it uses |
|---|---|---|
| Firefight | the script loader and command adapter (`crates/formats/firefight/src/script.rs`, `ScriptDirector`); the interface types (`src/director.rs`) and the kill-point math its tests check against (`src/scoring.rs`); the host and its log (`crates/extensions/game/src/firefight/host.rs`); the mode blob and HUD view (`firefight/hud.rs`); the sight pass | `ff_set`, `ff_round`, `ff_wave`, `ff_skull`, `ff_lives`, `ff_score`, `ff_respawn`, `ff_finish`, `ff_target`, `ff_ally`, `ff_team_lives`, `ff_player_score`, `ff_team_finish`, `ff_bonus`, `zone_seen`, `zone_distance`, `zone_has_role`, `zone_team_distance`, `place_squad_at`; capability `firefight.director` |
| Superbounce | the landing judge, the fall through a strip's floor and the Bounced test of the medal chain, and the medals the HUD and the records show (`crates/extensions/game/src/superbounce_chain.rs`); the mode's HUD states and navpoint choice (`superbounce_mode.rs`, `superbounce_hud.rs`); the mode switch behind `hud.superbounce`, a capability that gates no entry | `on launch`, `on grounded`, `on bounced` (and `on bounce`, which Bounce Hunt and Bounce Practice use) |

## Firefight

`firefight.rules` owns wave timing, reserve placement and actor caps,
cleanup, zone ranking, lives, skull scheduling and scoring. The loader
passes the validated wave set as `firefight-waves.rules`, an in-memory
module of constants and tables. The kit exposes up to 12 drop zones and
the host supplies 16 squad-slot records. The command stream is the
contract exercised by the Firefight scenarios.

The generic world entries below describe the remaining host work. The
director reads the kit's zone observations and uses the Firefight
command entries for round and wave announcements, skulls, lives,
respawns, scores, team targets and match results.

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

## Superbounce

| The mode needs | From the API |
|---|---|
| a landing, with its speeds | `on touchdown(p, down, up)` (**built**, from the host while the mode runs) |
| a landing the host judged a bounce, its strip and its launch | `on launch(p, spot, speed)` (**built**) |
| the end of a run of bounces | `on grounded(p)` (**built**) |
| whether the player was primed the tick before | `p.primed` (**built** on the host, from the chain's view of the biped), kept in a player variable each tick |
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
`sb_strip` label source.

```text
rules "Superbounce" 1
type party
requires hud.superbounce, hud.widgets, hud.waypoints

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

- The Firefight loader checks the verifier's worst-tick estimate.
  The source bounds the drop-zone ranking to the kit's 12 zones.
- The Rust medal chain stays while the HUD's medals, the records, the
  ghosts and the Pogo Stick read its awards and while the script
  cannot judge a landing against the strips.
- Superbounce's `on tick` writes one HUD value and one waypoint per
  player, 32 writes with 16 players: the HUD cap of a tick exactly. A
  shipped script writes only on a change.

## Not verified

- Firefight’s game-thread cost at 24 live actors.
- How many strips a map's scan yields against the 256-member bound of a
  label, and whether `distance` to each on every touchdown fits.
- The two speeds in the Superbounce sketch; superbounce.md has the
  measured ones.

## Related

- [rules-world-api.md](rules-world-api.md): the API, its limits and the slices
- [firefight.md](firefight.md): the director, its tests and the kit
- [superbounce.md](superbounce.md): the rule, the medals and the scanner
- [rules-engine.md](rules-engine.md#firefight-ai-and-bounces): the director and the bounce events
