//! Scenarios: a script run over a recorded list of events on the
//! [`FakeGame`], with no game. `h2 rules run` reads the JSON form here;
//! Live's `/v1/rules/run` builds a [`crate::workbench::Scenario`]. Both
//! step every tick through [`step`] over the same game model
//! ([`FakeGame::apply`]), so a scenario plays the same in either.
//! See `docs/game/rules-engine.md`, "Tooling".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fake::{FakeGame, FakePlayer};
use crate::ir::{Event, Scope};
use crate::ty::Ty;
use crate::vm::{Arg, Fired, Input, ScriptError};
use crate::{Ir, Refusal, Vm, caps};

/// A scenario file: who is playing, then what happens tick by tick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default)]
    pub seed: u64,
    /// Option bits; the blob's defaults when absent.
    #[serde(default)]
    pub options: Option<u32>,
    #[serde(default)]
    pub players: Vec<ScenarioPlayer>,
    /// Team indices in play; `[0, 1]` when absent.
    #[serde(default)]
    pub teams: Option<Vec<u8>>,
    /// Retail record fields by name; the rest read the stock record of the
    /// script's carrier ([`crate::fake::stock_variant`]).
    #[serde(default)]
    pub variant: BTreeMap<String, i32>,
    /// Labelled objects the map holds.
    #[serde(default)]
    pub labelled: Vec<ScenarioLabel>,
    /// `round_time_left()` at tick 0, counted down one a tick to 0.
    #[serde(default)]
    pub round_time_left: i32,
    #[serde(default)]
    pub ticks: Vec<ScenarioTick>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioPlayer {
    pub slot: u8,
    #[serde(default)]
    pub team: u8,
    #[serde(default = "yes")]
    pub alive: bool,
    #[serde(default)]
    pub score: i32,
    #[serde(default)]
    pub lives: i32,
    /// The player's unit object: `player_of` reads it, and an `enter` or
    /// `leave` naming it moves the player in or out of the zone.
    #[serde(default)]
    pub unit: Option<u32>,
}

/// A labelled object: `objects labelled "<label>"` visits it, and
/// `o.number` reads its number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioLabel {
    pub label: String,
    pub object: u32,
    #[serde(default)]
    pub number: i32,
    /// Its placement id, the order a set visits it in; the object's handle
    /// when absent.
    #[serde(default)]
    pub placement: Option<u32>,
    /// The team it belongs to (a flag's spawn, a base), for `o.team`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioTick {
    #[serde(default)]
    pub events: Vec<ScenarioEvent>,
}

/// One event the host reports within a tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioEvent {
    Kill {
        victim: u8,
        #[serde(default)]
        killer: Option<u8>,
    },
    Spawn(u8),
    Death(u8),
    Join(u8),
    Quit(u8),
    Enter {
        zone: u32,
        object: u32,
    },
    Leave {
        zone: u32,
        object: u32,
    },
    Press {
        player: u8,
        #[serde(default)]
        object: u32,
    },
    Pickup {
        player: u8,
        object: u32,
    },
    Drop {
        player: u8,
        object: u32,
    },
    Bounce {
        player: u8,
        #[serde(default)]
        object: u32,
    },
    Edit {
        player: u8,
        object: u32,
    },
    SquadDead(u8),
    ActorKill {
        #[serde(default)]
        killer: Option<u8>,
        class: String,
    },
    ActorAssist {
        #[serde(default)]
        killer: Option<u8>,
        #[serde(default)]
        assist: Option<u8>,
        class: String,
    },
    RoundEnd,
}

impl ScenarioEvent {
    pub fn input(&self) -> Input {
        match self.clone() {
            ScenarioEvent::Kill { victim, killer } => Input::Kill { victim, killer },
            ScenarioEvent::Spawn(p) => Input::Spawn(p),
            ScenarioEvent::Death(p) => Input::Death(p),
            ScenarioEvent::Join(p) => Input::Join(p),
            ScenarioEvent::Quit(p) => Input::Quit(p),
            ScenarioEvent::Enter { zone, object } => Input::Enter { zone, object },
            ScenarioEvent::Leave { zone, object } => Input::Leave { zone, object },
            ScenarioEvent::Press { player, object } => Input::Press(player, object),
            ScenarioEvent::Pickup { player, object } => Input::Pickup(player, object),
            ScenarioEvent::Drop { player, object } => Input::Drop(player, object),
            ScenarioEvent::Bounce { player, object } => Input::Bounce(player, object),
            ScenarioEvent::Edit { player, object } => Input::Edit(player, object),
            ScenarioEvent::SquadDead(s) => Input::SquadDead(s),
            ScenarioEvent::ActorKill { killer, class } => Input::ActorKill { killer, class },
            ScenarioEvent::ActorAssist {
                killer,
                assist,
                class,
            } => Input::ActorAssist {
                killer,
                assist,
                class,
            },
            ScenarioEvent::RoundEnd => Input::RoundEnd,
        }
    }
}

/// Declares labelled objects on a game.
pub fn label_all(game: &mut FakeGame, labels: &[ScenarioLabel]) {
    for l in labels {
        game.label(&l.label, l.object, l.number, l.placement.unwrap_or(l.object));
        if let Some(team) = l.team {
            game.object_teams.insert(l.object, team);
        }
    }
}

/// What one tick of a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stepped {
    /// Each event reported (`init` and `round_start` first on a starting
    /// tick 0, then the tick's events), with what the VM did, and last the
    /// per-tick group as `tick`.
    pub fired: Vec<(&'static str, Fired)>,
    /// The timers whose handler the per-tick group ran, as `timer(name)`.
    pub timers: Vec<String>,
}

/// Global timers that reach 0 in this tick's update and have a handler.
fn due_timers(vm: &Vm) -> Vec<String> {
    let ir = vm.ir();
    ir.slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.ty == Ty::Timer && s.scope == Scope::Global)
        .filter_map(|(i, s)| {
            let name = ir.strings.get(s.name as usize)?;
            let due = vm.global(name) == Some(1)
                && ir
                    .handlers
                    .iter()
                    .any(|h| h.event == Event::Timer && h.timer == Some(i as u16));
            due.then(|| format!("timer({name})"))
        })
        .collect()
}

/// One tick of a run over the fake game, as every scenario runner steps
/// it: the VM's tick begins and the round clock counts down, the queued
/// calls are made, `init` and `round_start` fire on tick 0 when `start`,
/// each event is applied to the game ([`FakeGame::apply`]) and its handler
/// runs ([`FakeGame::after`] then removes a player who quit), and last the
/// per-tick group runs.
pub fn step(vm: &mut Vm, game: &mut FakeGame, tick: u32, start: bool, inputs: &[Input]) -> Stepped {
    vm.begin_tick(tick);
    game.begin_tick(tick);
    vm.flush_queue(game);
    let mut out = Stepped::default();
    if tick == 0 && start {
        for input in [Input::Init, Input::RoundStart] {
            let fired = vm.fire(game, input.clone());
            out.fired.push((input.event().name(), fired));
        }
    }
    for input in inputs {
        game.apply(input);
        let fired = vm.fire(game, input.clone());
        out.fired.push((input.event().name(), fired));
        game.after(input);
    }
    if vm.ext().stopped.is_none() {
        out.timers = due_timers(vm);
    }
    let fired = vm.update(game);
    out.fired.push(("tick", fired));
    out
}

/// One action the script made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallLog {
    pub entry: &'static str,
    pub args: Vec<Arg>,
}

/// One tick of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickLog {
    pub tick: u32,
    /// Each event reported (`init` and `round_start` first on tick 0, the
    /// scenario's events, then the per-tick group as `tick`), with what the
    /// VM did.
    pub fired: Vec<(&'static str, Fired)>,
    /// Actions made in this tick, in call order, queued ones included.
    pub calls: Vec<CallLog>,
}

/// What a run ended with.
#[derive(Clone, Debug)]
pub struct RunResult {
    pub ticks: Vec<TickLog>,
    pub game: FakeGame,
    /// The tick the script called `end_round`, if it did.
    pub ended_at: Option<u32>,
    pub script_error: Option<ScriptError>,
}

impl RunResult {
    /// Every action made, in order.
    pub fn total_calls(&self) -> usize {
        self.ticks.iter().map(|t| t.calls.len()).sum()
    }
}

/// The game a scenario describes, before any tick.
pub fn game_of(scenario: &Scenario) -> FakeGame {
    let mut game = FakeGame::with_players(0);
    if let Some(teams) = &scenario.teams {
        game.teams = teams.clone();
    }
    game.round_time_left = scenario.round_time_left.max(0);
    game.variant = scenario.variant.clone();
    label_all(&mut game, &scenario.labelled);
    for p in &scenario.players {
        game.players.insert(
            p.slot,
            FakePlayer {
                team: p.team,
                alive: p.alive,
                score: p.score,
                lives: p.lives,
                kills: 0,
                deaths: 0,
            },
        );
        if let Some(unit) = p.unit {
            game.units.insert(unit, p.slot);
        }
    }
    game
}

/// Runs `ir` over `scenario`, every tick through [`step`]; queued calls
/// are also made after the last tick. The run does not stop at
/// `end_round`.
pub fn run(ir: Ir, scenario: &Scenario) -> Result<RunResult, Refusal> {
    let options = scenario.options.unwrap_or_else(|| ir.default_options());
    let mut game = game_of(scenario);
    game.carrier = ir.header.carrier;
    let mut vm = Vm::new(ir, options, scenario.seed)?;
    let end_name = caps::entry(caps::id::END_ROUND, caps::API_VERSION).map(|e| e.name);
    let mut ticks = Vec::new();
    let mut ended_at = None;
    let mut made = 0usize;
    let count = scenario.ticks.len().max(1);
    for t in 0..count {
        let tick = t as u32;
        let inputs: Vec<Input> = scenario
            .ticks
            .get(t)
            .map(|st| st.events.iter().map(ScenarioEvent::input).collect())
            .unwrap_or_default();
        let stepped = step(&mut vm, &mut game, tick, true, &inputs);
        if t + 1 == count {
            vm.flush_queue(&mut game);
        }
        let mut log = TickLog {
            tick,
            fired: stepped.fired,
            calls: Vec::new(),
        };
        for (entry, args) in &game.calls[made..] {
            if ended_at.is_none() && Some(*entry) == end_name {
                ended_at = Some(tick);
            }
            log.calls.push(CallLog {
                entry: *entry,
                args: args.clone(),
            });
        }
        made = game.calls.len();
        ticks.push(log);
    }
    Ok(RunResult {
        ticks,
        script_error: game.script_error,
        ended_at,
        game,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
rules "Count" 1
carrier slayer

on kill(victim, killer) {
    if killer == none or killer == victim { score(victim, -1) } else { score(killer, 1) }
}
"#;

    fn ir() -> Ir {
        crate::build(SRC).unwrap().0
    }

    fn seat(slot: u8, team: u8) -> ScenarioPlayer {
        ScenarioPlayer {
            slot,
            team,
            alive: true,
            score: 0,
            lives: 0,
            unit: None,
        }
    }

    #[test]
    fn a_scenario_reads_from_json() {
        let s: Scenario = serde_json::from_str(
            r#"{"seed":7,"players":[{"slot":0,"unit":100},{"slot":1,"team":1}],
                "variant":{"score_limit":10},
                "labelled":[{"label":"hill","object":40,"number":2},{"label":"base","object":41,"team":1}],
                "ticks":[{"events":[{"kill":{"victim":1,"killer":0}},{"spawn":1},{"press":{"player":0}},
                    {"bounce":{"player":0}},{"edit":{"player":0,"object":5}},
                    {"actor_kill":{"killer":0,"class":"elite"}},
                    {"actor_assist":{"killer":0,"assist":1,"class":"elite"}}]}]}"#,
        )
        .unwrap();
        assert_eq!(s.seed, 7);
        assert!(s.players[0].alive);
        assert_eq!(s.players[0].unit, Some(100));
        assert_eq!(s.variant["score_limit"], 10);
        assert_eq!(s.labelled[0].number, 2);
        assert_eq!((s.labelled[0].team, s.labelled[1].team), (None, Some(1)));
        assert_eq!(game_of(&s).object_teams.get(&41), Some(&1));
        assert_eq!(
            s.ticks[0].events[0],
            ScenarioEvent::Kill {
                victim: 1,
                killer: Some(0)
            }
        );
        assert_eq!(
            s.ticks[0].events[6].input(),
            Input::ActorAssist {
                killer: Some(0),
                assist: Some(1),
                class: "elite".into()
            }
        );
        assert!(serde_json::from_str::<Scenario>(r#"{"nope":1}"#).is_err());
    }

    #[test]
    fn a_kill_scores_for_the_killer() {
        let s = Scenario {
            players: (0..2).map(|slot| seat(slot, slot)).collect(),
            ticks: vec![
                ScenarioTick { events: vec![] },
                ScenarioTick {
                    events: vec![ScenarioEvent::Kill {
                        victim: 1,
                        killer: Some(0),
                    }],
                },
            ],
            ..Scenario::default()
        };
        let r = run(ir(), &s).unwrap();
        assert_eq!(r.ticks.len(), 2);
        assert_eq!(r.game.players[&0].score, 1);
        assert_eq!(r.total_calls(), 1);
        assert_eq!(r.ticks[1].calls[0].entry, "score");
        assert_eq!(r.script_error, None);
    }

    #[test]
    fn a_kill_counts_kills_deaths_and_alive() {
        let src = "rules \"K\" 1\ncarrier slayer\nglobal var seen: int = 0\n\
                   on kill(victim, killer) { if killer != none and not victim.alive { seen = killer.kills * 10 + victim.deaths } }\n";
        let s = Scenario {
            players: vec![seat(0, 0), seat(1, 1)],
            ticks: vec![ScenarioTick {
                events: vec![ScenarioEvent::Kill {
                    victim: 1,
                    killer: Some(0),
                }],
            }],
            ..Scenario::default()
        };
        let r = run(crate::build(src).unwrap().0, &s).unwrap();
        assert_eq!(r.game.players[&0].kills, 1);
        assert_eq!(r.game.players[&1].deaths, 1);
        assert!(!r.game.players[&1].alive);
        let mut vm = Vm::new(crate::build(src).unwrap().0, 0, 0).unwrap();
        let mut g = game_of(&s);
        step(&mut vm, &mut g, 0, true, &[s.ticks[0].events[0].input()]);
        assert_eq!(vm.global("seen"), Some(11));
    }

    #[test]
    fn variant_fields_and_the_round_clock_read_the_scenario() {
        let src = "rules \"V\" 1\ncarrier slayer\nglobal var limit: int = 0\nglobal var left: int = 0\n\
                   on tick { limit = variant.score_limit; left = round_time_left() }\n";
        let s = Scenario {
            round_time_left: 10,
            ticks: vec![ScenarioTick::default(); 4],
            ..Scenario::default()
        };
        let ir = crate::build(src).unwrap().0;
        let mut vm = Vm::new(ir.clone(), 0, 0).unwrap();
        let mut g = game_of(&s);
        g.carrier = ir.header.carrier;
        for t in 0..4 {
            step(&mut vm, &mut g, t, true, &[]);
        }
        // The stock Slayer record's score limit, and three ticks counted.
        assert_eq!(vm.global("limit"), Some(25));
        assert_eq!(vm.global("left"), Some(7));
        let mut set = s.clone();
        set.variant.insert("score_limit".into(), 50);
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = game_of(&set);
        step(&mut vm, &mut g, 0, true, &[]);
        assert_eq!(vm.global("limit"), Some(50));
    }

    #[test]
    fn ticks_held_counts_a_zone_held_alone() {
        let src = "rules \"Z\" 1\ncarrier slayer\nglobal var held: int = 0\n\
                   on tick { held = ticks_held(first(objects labelled \"zone\"), team 0) }\n";
        let mut s = Scenario {
            players: vec![seat(0, 0), seat(1, 1)],
            labelled: vec![ScenarioLabel {
                label: "zone".into(),
                object: 50,
                number: 0,
                placement: None,
                team: None,
            }],
            ..Scenario::default()
        };
        s.players[0].unit = Some(100);
        s.players[1].unit = Some(101);
        let mut vm = Vm::new(crate::build(src).unwrap().0, 0, 0).unwrap();
        let mut g = game_of(&s);
        let enter = |object| Input::Enter { zone: 50, object };
        // Tick 0 names the zone; ticks 1 to 3 player 0 holds it alone.
        step(&mut vm, &mut g, 0, true, &[enter(100)]);
        assert_eq!(vm.global("held"), Some(0));
        for t in 1..4 {
            step(&mut vm, &mut g, t, true, &[]);
        }
        assert_eq!(vm.global("held"), Some(3));
        // Contested: no team gains.
        step(&mut vm, &mut g, 4, true, &[enter(101)]);
        assert_eq!(vm.global("held"), Some(3));
        assert_eq!(vm.ticks_held(50, 1), 0);
        // The round's end clears the counters.
        step(&mut vm, &mut g, 5, true, &[Input::RoundEnd]);
        assert_eq!(vm.ticks_held(50, 0), 0);
    }

    #[test]
    fn an_empty_scenario_still_runs_tick_zero() {
        let r = run(ir(), &Scenario::default()).unwrap();
        assert_eq!(r.ticks.len(), 1);
        assert_eq!(r.ticks[0].fired[0].0, "init");
    }
}
