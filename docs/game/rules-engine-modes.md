# Rules engine: example modes

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: Slayer, Infection, Firefight, the phased Siege mode and the Forge graph front end.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## Slayer

```text
rules "Slayer" 1
type slayer

option kill_points: int 1..5 = 1
option suicide_costs: bool = true

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

`score` is `SCORE_ADD`, which itself adds the team score and tests the
score limit (**E**, `0x15b7c0`, firefight.md "Guest boundary"), so the
script has no limit check.

## Infection: a team switch

```text
rules "Infection" 1
type slayer
teams 2
requires player.team, player.weapons

option first_zombies: int 1..4 = 1

player var infected: bool = false

fn infect(p: player) {
    p.infected = true
    set_team(p, team 1)
    set_loadout(p, "objects/weapons/melee/energy_blade/energy_blade", none)
}

on round_start {
    for each players p { p.infected = false; set_team(p, team 0) }
    for each pick(players, first_zombies) p { infect(p) }
}

on kill(victim, killer) {
    if killer != none and killer.infected and not victim.infected {
        infect(victim)
        score(killer, 1)
    }
}

on tick {
    if count(players where not infected) == 0 { end_round(team 1) }
    else if round_time_left() == 0t { end_round(team 0) }
}
```

`pick` draws from the VM's generator. `set_team` has no traced engine
entry ("The engine API"); Infection waits on it.

## Firefight: waves of AI

```text
rules "Holdout" 1
type firefight
teams 1
requires ai.squads

table waves: (class: Class, per_player: int, min: int, max: int, gap: time) = [ ... ]
table points: (class: Class, score: int) = [ ... ]
label ff_drop: objects
global var wave: int = 0
global var lives: int = 7
global var next: timer = 5s

on timer(next) {
    let w = waves[wave]
    let s = squad(wave % 16)
    place_squad(s, w.class, clamp(count(players) * w.per_player, w.min, w.max), ff_drop)
    hunt(s)
}
on squad_dead(s) {
    wave = wave + 1
    if wave == len(waves) { end_round(team 0) } else { next = waves[wave - 1].gap }
}
on actor_kill(killer, class) { if killer != none { score(killer, points[class].score) } }
on death(p) {
    if lives > 0 { lives = lives - 1; respawn(p) }
    else if count(players where alive) == 0 { end_round(none) }
}
```

## Phases: attack and defend

```text
const last_phase = 2

label barrier: objects numbered
label attack: spawns numbered 0..last_phase
label defend: spawns numbered 0..last_phase
label capture: zones numbered 0..1 = "zone"
label core: object
label goal: zone

option phase_time: time 60s..600s = 180s
option capture_time: time 5s..60s = 20s

global var phase: int = 0
global var clock: timer

fn open(n: int) {
    phase = n
    clock = phase_time
    for each barrier b where b.number < n { delete(b) }
    spawn_areas(team 0, attack, n)
    spawn_areas(team 1, defend, n)
    if n == last_phase { create("<core tag path>", core) }
}
on pickup(p, o) { if phase == last_phase and not is_objective(o) { register_objective(o, team 0) } }
on round_start { open(0) }
on tick {
    if phase < last_phase {
        if let zone = capture[phase] and ticks_held(zone, team 0) >= capture_time { open(phase + 1) }
    }
}
on enter(goal, o) {
    if phase == last_phase and is_objective(o) { end_round(team 0) }
}
on timer(clock) { end_round(team 1) }
```

The built-in Siege (`content/modes/rules/siege.rules`,
listed in Party Games) is the part of this a host runs. Its zones come
from every Territories and CTF map's own flags, so it needs no kit: the
attackers take `sites` of the map's territories (each held alone for
the capture time in all, by `ticks_held`), then hold the defenders'
final stand to win. The sites are the author's `siege_site` flags
(`label site: zones = "siege_site"`) and, when none is placed, the map's
territory flags ("Forge labels"); the final stand is a placed
`siege_goal` (`label goal: zone? = "siege_goal"`), else the defenders'
CTF flag stand (`label base: zones = "ctf_flag_spawn"`, `base[defenders]`). Each phase has its own
clock, and a clock that runs out wins it for the defenders. Each side
spawns with its own loadout (`on spawn`, `set_loadout`), and a round's
winners score a point each, so the post-game names them.
A game is `round_count` rounds (1 to 8, two by default), and the sides
swap at each: team 0 attacks in the first, third and so on, team 1 in
the second, fourth and so on (`attackers` and `defenders`, set in `on
round_start` from `round`, the rounds played, which like every global
carries across rounds). A round ends on the objective or on the clock
(`win`): the winners score, their team's `wins` goes up, and while
rounds are left the script calls `next_round` (`game.rounds`), so the
engine runs its round's end and the next round starts with every
player back, the scores kept and the VM's round state gone. The engine's
round reset zeroes each statborg row's round score and adds it into the
row's earlier rounds' score (`0x1e97b0`); `player_score` and team score
read the game's, the two added (`rules::guest::player_score`,
`guest_layout::game::game_score`), so a round's points carry and the
post-game's scores count every round. The last
round calls `end_round` with the team that won the more rounds, or
`none` for an even count; a game of one round ends with the round's
winner. Each round of a game of more than one opens with its number
on the banner (`ROUND 1`, ..., `FINAL ROUND` for the last) and a
message to each side (`Round 2 of 2: you attack`, `... you defend`),
and the bodies, loadout rows, lines and HUD colours follow the sides. No label states a
requirement, but the lobby's start gate refuses a map with neither a
site (a `siege_site` or a territory) nor a final stand (a `siege_goal` or
the defenders' flag stand) by name
([rules-start-gate.md](rules-start-gate.md), `gate.rs` `map_problems`),
for the picker and for Start Game. On a map with either, with no
territory the assault goes straight to the flag stand and with no flag
stand taking every site wins it. A run the gate does not guard (a
scenario) with neither draws the round at its start with a message. The
variant's `lives = 1` holds each death's respawn (the engine's own
lives gate), and each side's dead come back together when its wave
timer (`wave_time`) runs out (`respawn`); a death is told "Reinforcements
in N seconds" (`hold`), the seconds left of its side's wave. Each side spawns near its
line (`spawn_near`, the four starting locations nearest it), and the
lines move with the phases: the attackers start at their own flag stand
(`base[attackers]`) and spawn at each site they take, and the defenders
hold a site still to take, then fall back to their flag stand for the
final assault. A `siege_line` flag the author placed for a side and a
phase (its number is the phase, its team the side: 0 the attackers and
1 the defenders, whichever team that is in the round;
`label placed_line: zones at most 16 = "siege_line"`) replaces the
phase's line for that side. A line the map lacks (no flag stand) leaves
that side the engine's own spawns. The attackers spawn as Spartans and the defenders
as Elites (`set_body`), whatever each player chose. The HUD is the Siege
kit's ("The Siege HUD" below): the
mark and the front row (`siege.front`) stay on the current untaken site while
any living player occupies it. When that site empties or is taken, the
mark moves to the first untaken site with living attackers, including a
contested site. With no attackers on a site it marks the first untaken
site, and in the final phase it marks the stand. The row reads open,
a side on it or contested. The banner (`siege.banner`) says a site
taken, the final assault, an overtime, and the last 30 and 10 seconds of a
phase; the phase's seconds left are beside the viewer's side (`siege.clock`,
"ATTACK 2:59"). A phase whose clock runs out with an attacker on its front goes
into overtime (`overtime_time`, 30 s by default): the defenders win when
the attackers leave the front, or when the overtime runs out (a contested
front does not hold it open past that), and a capture that completes
first takes the site as it would have. The map opens with the phases: the
objects the author labelled `siege_barrier`, numbered by the phase they
open in (a barrier numbered 0 is open from the start), are opened with
`set_open` as each phase begins (`label barrier: objects at most 8`; a
placement's label is set from Forge's Properties page, and a map with none
plays as before). Each side has a table of loadout rows, lightest first
(`attack_rows`, `defend_rows`), and `tier_rows` says how many a phase
opens: two in the first, four in the second, all five in the final
assault (a map with no numbered sites stays on the first). At the round's
start and as each phase begins the script `offer`s every player the rows of
their side's tier (`offer(p, picker, count)`; the two sides' labels are
the pickers 0 and 1), and `on spawn` gives the row `choice(p)` reads. A
local player picks in a select dialog that opens when their window grows
and at each spawn of theirs (`rules/world/choice.rs`, `spawned`, over the
`kit.picker.game` dialog titled ATTACKER LOADOUT or DEFENDER LOADOUT):
a row above the tier is refused by the VM (`Vm::choose_loadout`) and the
last pick stands. A row picked while the player is alive is equipped by
the next tick (`on tick` in `siege.rules` sets the kit of a player whose
`kit` differs from `choice(p)`), so the pick need not wait for a death. The pick never blocks a spawn: a player who picks nothing keeps
their last row, row 0 at first, and so does a player on another machine.
Siege has no core to carry.

### The Siege HUD

The layout is that of Halo: Reach's Invasion HUD (the side, the clock and
the objective's state in one block by the score, a mark over the
objective), drawn as Halo 2 draws a HUD: lines of the HUD font above the
score plate, as the Firefight block is, with no art of its own
(`content/modes/huds/siege.hud.yaml`, `hud_kit_core::views::objective::siege`).

```text
                                 ROUND 1 OF 2 PHASE 2 OF 3   -100
                                              SITES 1 OF 2    -86
                                         SITE UNDER ATTACK    -72
                                               DEFEND 2:59    -58
                                        [ score plate ]   bottom right
```

| Element | Shows | The script's widget (`siege.rules`) |
|---|---|---|
| `siege.clock` | `ATTACK 2:59` or `DEFEND 2:59`: the viewer's side and the phase's seconds left | `phase_row` (the seconds); `side_row` at `siege.side` (the attacking team) |
| `siege.front` | the front's state, worded for the viewer's side: `SITE OPEN`, `TAKING SITE` or `SITE UNDER ATTACK`, `SITE DEFENDED` or `HOLDING SITE`, `SITE CONTESTED`; `STAND` in the final assault | `front_row`: style 0 open, 1 the side in the value on it alone, 2 contested |
| `siege.sites` | `SITES 1 OF 2`, then `FINAL ASSAULT` once every site needed is taken | `sites_row`: the sites taken, of the style |
| `siege.round` | `ROUND 1 OF 2 PHASE 2 OF 3`; a game of one round shows the phase alone | `round_row` and `stage_row` (at `siege.phase`): a number, of the style |
| `siege.mark` | a navpoint over the front, with the distance | `front_mark` |
| `siege.banner` | the timed centred line: the round, a site taken, the final assault, overtime, 30 and 10 seconds | `banner_row` |

The script sets the counts at the round's start and as each phase begins
(`begin`), so a tick pays nothing for them. Half screen draws the four
lines; a quarter draws the clock and the sites. The capture gauge, each
side's objective line and the second clock are the scripted objective
HUD's (`meter`, `objective`, `countdown`).

Tests: `hud-kit-core/src/views/objective.rs`
(`the_siege_hud_reads_the_side_the_front_and_the_counts`),
`game/src/rules/world/hud.rs`
(`the_built_in_siege_feeds_the_siege_hud_for_each_side`),
`rules/tests/hud_marks.rs` (`the_siege_counts_*`, `the_siege_mark_*`).
Not run in a game.

### Siege options in the lobby

Game Setup's **Quick Options...** is Siege's options page
(`variant_quick::dialog_rows_with`, [rules-variants.md](rules-variants.md#quick-options-in-the-lobby)):
the five marked rows, then the two options the script leaves unmarked.
A opens a row's value picker; a wide range is a ladder
(`variant_quick::ladder`).

| Row | Values the picker lists |
|---|---|
| Respawn Time | the editor's |
| Respawn wave (seconds) | 3, 5, 10, 15, 20, 25, 30 |
| Phase time (seconds) | 60 to 600, a minute apart |
| Capture time (seconds) | 5 to 60, five apart |
| Sites | 1 to 4 |
| Overtime (seconds) | 5 to 60, five apart |
| Rounds | 1 to 8 |

The lobby's panel shows the five marked rows. Tests:
`variant_quick.rs` (`the_dialog_lists_every_option_of_a_script_whose_options_fit`,
`a_wide_range_is_offered_as_a_ladder`), `variant_quick/page/tests.rs`
(`sieges_page_changes_every_option_through_a_picker`).

### Siege announcer lines

`siege.rules` requires `audio.play` and plays the announcer's clips
(`art/announcer/<clip>.wav`) with `sound_to`, `sound_to_team` and
`sound_to_all`, naming a clip as the sound `"announcer/<clip>"`
([rules-world-api.md](rules-world-api.md#audio)).

| Event | Line | Audience |
|---|---|---|
| The game starts | Siege (`siege`, the type's start line, [audio.md](../audio/audio.md#our-medals-announcer-lines)) | all |
| A round starts | Attack (`attack`) / Defend (`defend`) | the attackers / the defenders |
| The last round starts | Final Round (`final-round`) | all |
| A site falls (each, the last before the final assault too) | Site Captured (`site-captured`) | all |
| A side's respawn wave lands, with dead to bring back | Reinforcements (`reinforcements`) | that side |
| A round ends | Round Over (`round-over`) | all |

Tests: `crates/formats/rules/tests/hud_marks.rs` (`siege_says_*`).

### Siege layouts

A map plays ordered phases when its layout numbers its sites, names its
final stand and labels its starts by side and phase ("Map layouts"
above). These maps have a built-in layout
(`crates/formats/rules/src/builtin/layouts/siege/<map>.json`):

| Map | Layout source | Sites (phase 0, phase 1) | Final stand | Barriers |
|---|---|---|---|---|
| Zanzibar | read off the map by hand | territory flags 17, 15; 19, 13 | blue's stand, flag 3 (`siege_goal`) | none |
| Coagulation | `h2 siege layout coagulation`, red attacks | territory flags 20, 19; 21, 53 | blue's stand, flag 6 (`siege_goal`) | none: the map has no loose solid object across a way |
| Headlong | `h2 siege layout headlong --barrier crates:17=2 --barrier crates:70=2`, red attacks | territory flags 51; 62 | blue's stand, flag 0 (`siege_goal`) | two giant tech crates by blue's base, open in the final assault |
| Waterworks | `h2 siege layout waterworks --barrier crates:41=2 --barrier crates:42=2`, red attacks | territory flags 26, 23; 17 | blue's stand, flag 4 (`siege_goal`) | two giant packing crates by blue's base, open in the final assault |

Each layout labels the defenders' stand `siege_goal`, so the final
stand stays at one end of the map when the sides swap (a map with no
`siege_goal` falls back to `base[defenders]`, which moves to the other
stand in the second round). Each layout has a scenario (`crates/formats/rules/tests/scenarios/siege_<map>.json`,
written by `--scenario`) that `rules/tests/modes.rs`
(`siege_plays_each_recipe_layouts_phases`) runs: the scenario's labels are
the layout's, an attacker standing on one site of each phase in turn takes
it, each phase's spawns and loadout tier open in order, the bots are sent
at each site and then at the stand (`bot_goal_attack`), the barriers open
in their phase, and the attackers win at the stand.

The recipe (`forge_doc::siege_layout`) runs the attack line from the
attackers' CTF flag stand to the defenders'. The territories between
them (from 0.12 to 0.88 of the way along, a flag within 4 units of a kept
one dropped), in order along the line, are the sites: the nearer half
numbered 0, the farther half 1, at most two a phase. Each phase's starts
are the ones nearest the attackers' stand, then nearest the sites they
took, on their side of the phase's front, and for the defenders the ones
past the phase's sites, then nearest their stand; `per_side` a side, no
start both sides' in one phase. A barrier is named by its placement and
phase: the recipe does not judge which object blocks a way.

| Tool, command or screen | Output |
|---|---|
| `h2 siege layout <map> [--attack red\|blue] [--per-side N] [--barrier <block>:<index>=<phase>]... [-o layout.json] [--scenario s.json]` (`crates/apps/h2-tools/src/siege_cmd.rs`) | the plan, a line a label; with `-o` the labels-only document in the built-in layouts' form; with `--scenario` the fake-game walk through its phases |
| Studio `siege.layout {attack?, per_side?, barriers?, out?}` (`crates/apps/studio/src/api/siege.rs`) | the same plan over the open map as edited, and the document; with `out` the file. The open map is not changed |
| Studio `scenario.add {block: "label", label: "siege_site" \| "siege_goal" \| "siege_line", ...}` | a Forge-map label: one netgame flag with the label ("Maps for scripted games" in [game-types-without-carriers.md](game-types-without-carriers.md)) |
| Forge's **Siege** page on the object menu (`forge_screens::sheet::SIEGE_PAGE`): **Site**, **Flag stand**, **Spawn line**, **Barrier** | `siege_site` (a site with its area), `siege_goal` and `siege_line` (a marker placed in front of the monitor), `siege_barrier` (the object aimed at, numbered 1; Properties changes it), in the map variant's document |

## A Forge graph is the same IR

The Forge front end turns the document's nodes and links into the same
slots and handlers. A trigger volume linked to a counter linked to a
message:

```text
volume#4.entered ──> counter#7.start (limit 3) ──> message#9.fire

    global var n7: int = 0
    on enter(zone, o) {
        if zone.id == 4 { n7 = n7 + 1; if n7 == 3 { message(all, text 9); n7 = 0 } }
    }
```

A node's state is a global slot named by node id; a link is a call, in
link-id order. The graph's budget rule (node visits a tick, the rest
next tick, forge-features.md) lowers to a queue in slots. Links that
retail data expresses whole (a teleporter pair) stay retail data. A
map's graph and the game type's script are two blobs in one VM; the game
type's handlers run first.
