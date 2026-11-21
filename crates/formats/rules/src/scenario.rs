//! Scenarios: a script run over a recorded list of events on the
//! [`FakeGame`], with no game. What `h2 rules run` and the web tools share.
//! See `docs/game/rules-engine.md`, "Tooling".

use serde::{Deserialize, Serialize};

use crate::fake::{FakeGame, FakePlayer};
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
    SquadDead(u8),
    RoundEnd,
}

impl ScenarioEvent {
    pub fn input(&self) -> Input {
        match *self {
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
            ScenarioEvent::SquadDead(s) => Input::SquadDead(s),
            ScenarioEvent::RoundEnd => Input::RoundEnd,
        }
    }
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
    game.round_time_left = scenario.round_time_left;
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
    }
    game
}

/// Runs `ir` over `scenario`. Tick 0 reports `init` and `round_start`
/// first; every tick then reports its events and the VM's per-tick group;
/// queued calls are made at the start of the next tick and after the last.
pub fn run(ir: Ir, scenario: &Scenario) -> Result<RunResult, Refusal> {
    let options = scenario.options.unwrap_or_else(|| ir.default_options());
    let mut vm = Vm::new(ir, options, scenario.seed)?;
    let mut game = game_of(scenario);
    let end_name = caps::entry(caps::id::END_ROUND, caps::API_VERSION).map(|e| e.name);
    let mut ticks = Vec::new();
    let mut ended_at = None;
    let mut made = 0usize;
    let count = scenario.ticks.len().max(1);
    for t in 0..count {
        let tick = t as u32;
        vm.begin_tick(tick);
        vm.flush_queue(&mut game);
        let mut log = TickLog {
            tick,
            ..TickLog::default()
        };
        let mut inputs = Vec::new();
        if t == 0 {
            inputs.push(Input::Init);
            inputs.push(Input::RoundStart);
        }
        if let Some(st) = scenario.ticks.get(t) {
            inputs.extend(st.events.iter().map(ScenarioEvent::input));
        }
        for input in inputs {
            let name = input.event().name();
            let fired = vm.fire(&mut game, input);
            log.fired.push((name, fired));
        }
        let fired = vm.update(&mut game);
        log.fired.push(("tick", fired));
        if t + 1 == count {
            vm.flush_queue(&mut game);
        }
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

    #[test]
    fn a_scenario_reads_from_json() {
        let s: Scenario = serde_json::from_str(
            r#"{"seed":7,"players":[{"slot":0},{"slot":1,"team":1}],
                "ticks":[{"events":[{"kill":{"victim":1,"killer":0}},{"spawn":1},{"press":{"player":0}}]}]}"#,
        )
        .unwrap();
        assert_eq!(s.seed, 7);
        assert!(s.players[0].alive);
        assert_eq!(
            s.ticks[0].events[0],
            ScenarioEvent::Kill {
                victim: 1,
                killer: Some(0)
            }
        );
        assert!(serde_json::from_str::<Scenario>(r#"{"nope":1}"#).is_err());
    }

    #[test]
    fn a_kill_scores_for_the_killer() {
        let s = Scenario {
            players: (0..2)
                .map(|slot| ScenarioPlayer {
                    slot,
                    team: slot,
                    alive: true,
                    score: 0,
                    lives: 0,
                })
                .collect(),
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
    fn an_empty_scenario_still_runs_tick_zero() {
        let r = run(ir(), &Scenario::default()).unwrap();
        assert_eq!(r.ticks.len(), 1);
        assert_eq!(r.ticks[0].fired[0].0, "init");
    }
}
