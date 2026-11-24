//! Scenarios: a script run over a recorded list of events on the
//! [`FakeGame`], with no game. `h2 rules run` reads the JSON form here;
//! Live's `/v1/rules/run` builds a [`crate::workbench::Scenario`]. Both
//! step every tick through [`step`] over the same game model
//! ([`FakeGame::apply`]), so a scenario plays the same in either.
//! See `docs/game/rules-engine.md`, "Tooling".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fake::{FakeGame, FakePlayer};
use crate::ir::{Event, LabelKind, Scope};
use crate::labels::{self, Census, Class, Member};
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
    /// The map: what the game holds that a script labels. When absent,
    /// [`labels::minimal`] supplies one ([`resolve_map`]); when present, it
    /// must pass [`labels::check`] or the run refuses with the problems.
    /// A file may still call it `labelled`.
    #[serde(default, alias = "labelled", skip_serializing_if = "Option::is_none")]
    pub map: Option<Vec<ScenarioLabel>>,
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

/// One thing of the map: a labelled object, zone or starting location.
/// `objects labelled "<label>"` visits it, and `o.number` reads its number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioLabel {
    pub label: String,
    pub object: u32,
    /// Its number under the label; a numbered label needs one on every
    /// member, and `o.number` reads 0 when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
    /// Its placement id, the order a set visits it in; the object's handle
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<u32>,
    /// The team it belongs to (a flag's spawn, a base), for `o.team`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// What it is on the map, which a label's kind is checked against.
    /// When absent, what the script's label of that text is declared as:
    /// a zone label's member is a volume, a spawn label's a starting
    /// location, any other (and a label the script does not declare) an
    /// object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<Class>,
}

impl ScenarioLabel {
    /// A thing of `label` with the object handle `object` and nothing else
    /// set.
    pub fn new(label: &str, object: u32) -> ScenarioLabel {
        ScenarioLabel {
            label: label.to_string(),
            object,
            number: None,
            placement: None,
            team: None,
            class: None,
        }
    }

    /// Its class: the one it states, or the one its label's kind in `ir`
    /// suggests.
    pub fn class_in(&self, ir: &Ir) -> Class {
        self.class.unwrap_or_else(|| {
            ir.labels
                .iter()
                .find(|d| d.text == self.label)
                .map_or(Class::Object, |d| match d.kind {
                    LabelKind::Object | LabelKind::Path => Class::Object,
                    LabelKind::Zone => Class::Volume,
                    LabelKind::Spawn => Class::Spawn,
                })
        })
    }
}

/// The first object handle of a map [`labels::minimal`] supplies: the
/// things take `MAP_BASE`, `MAP_BASE + 1`, ... in label order.
pub const MAP_BASE: u32 = 0x100;

/// The census of a list of things: what [`labels::check`] reads.
pub fn census_of(ir: &Ir, things: &[ScenarioLabel]) -> Census {
    let mut census = Census::default();
    for t in things {
        census.labels.entry(t.label.clone()).or_default().push(Member {
            class: t.class_in(ir),
            number: t.number,
            team: t.team,
        });
    }
    census
}

/// A census as things, numbered from `base`: its labels in order, each
/// label's members in order.
pub fn things_of(census: &Census, base: u32) -> Vec<ScenarioLabel> {
    let mut out = Vec::new();
    for (label, members) in &census.labels {
        for m in members {
            out.push(ScenarioLabel {
                label: label.clone(),
                object: base + out.len() as u32,
                number: m.number,
                placement: None,
                team: m.team,
                class: Some(m.class),
            });
        }
    }
    out
}

/// The things a run places: the scenario's own map when it has one, after
/// [`labels::check`] for `teams`; else [`labels::minimal`]. `Err` is each
/// problem's text, in the check's order.
pub fn resolve_map(
    ir: &Ir,
    map: Option<&[ScenarioLabel]>,
    teams: &[u8],
) -> Result<Vec<ScenarioLabel>, Vec<String>> {
    match map {
        Some(things) => {
            let problems = labels::check(ir, &census_of(ir, things), teams);
            if problems.is_empty() {
                Ok(things.to_vec())
            } else {
                Err(problems.into_iter().map(|p| p.text).collect())
            }
        }
        None => Ok(things_of(&labels::minimal(ir, teams), MAP_BASE)),
    }
}

fn yes() -> bool {
    true
}

fn one() -> u32 {
    1
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
    /// A superbounce off strip `object`: `chain` is its place in its
    /// chain (1, a single bounce, when absent), `monster` a Monster Bounce.
    Bounce {
        player: u8,
        #[serde(default)]
        object: u32,
        #[serde(default = "one")]
        chain: u32,
        #[serde(default)]
        monster: bool,
    },
    /// A fall out of the map during a bounce: the Bounced medal.
    Bounced(u8),
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
            ScenarioEvent::Bounce {
                player,
                object,
                chain,
                monster,
            } => Input::Bounce {
                player,
                spot: object,
                chain,
                monster,
            },
            ScenarioEvent::Bounced(p) => Input::Bounced(p),
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
        game.label(&l.label, l.object, l.number.unwrap_or(0), l.placement.unwrap_or(l.object));
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
/// each event is applied to the game ([`FakeGame::apply`]) and, when the
/// game took it, its handler runs ([`FakeGame::after`] then removes a
/// player who quit; a spawn the engine holds, out of lives, runs
/// nothing), and last the per-tick group runs.
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
        if game.apply(input) {
            let fired = vm.fire(game, input.clone());
            out.fired.push((input.event().name(), fired));
        }
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

    /// How the run ended, in a sentence: a script error, the round ended
    /// by the script, or still running.
    pub fn outcome(&self) -> String {
        match (self.script_error, self.ended_at) {
            (Some(e), _) => format!(
                "script error {:?} at tick {}, handler {}, op {}",
                e.kind, e.tick, e.handler, e.pc
            ),
            (None, Some(t)) => format!("the script ended the round at tick {t}"),
            (None, None) => "the round was still running".to_owned(),
        }
    }

    /// The run as `h2 rules run` prints it, and Studio's `rules.run`
    /// answers: each tick's calls (with `trace`, each event and call), the
    /// totals, every player and team, and the outcome.
    pub fn printout(&self, trace: bool) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        for t in &self.ticks {
            let _ = writeln!(s, "tick {}: {} calls", t.tick, t.calls.len());
            if trace {
                for (event, fired) in &t.fired {
                    let _ = writeln!(s, "  on {event}: {fired:?}");
                }
                for c in &t.calls {
                    let args: Vec<String> = c.args.iter().map(|a| format!("{a:?}")).collect();
                    let _ = writeln!(s, "  {}({})", c.entry, args.join(", "));
                }
            }
        }
        let _ = writeln!(s, "total: {} calls over {} ticks", self.total_calls(), self.ticks.len());
        for (slot, p) in &self.game.players {
            let _ = writeln!(
                s,
                "player {slot}: team {}, score {}, lives {}, {}",
                p.team,
                p.score,
                p.lives,
                if p.alive { "alive" } else { "dead" }
            );
        }
        let mut teams = self.game.teams.clone();
        teams.sort_unstable();
        for t in teams {
            let total: i32 = self.game.players.values().filter(|p| p.team == t).map(|p| p.score).sum();
            let _ = writeln!(s, "team {t}: score {total}");
        }
        let _ = writeln!(s, "result: {}", self.outcome());
        s
    }
}

/// Why a scenario did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The VM refused the script.
    Refused(Refusal),
    /// The map fails the script's labels: each problem's text.
    Map(Vec<String>),
}

impl From<Refusal> for RunError {
    fn from(r: Refusal) -> RunError {
        RunError::Refused(r)
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Refused(r) => write!(f, "{r}"),
            RunError::Map(problems) => write!(f, "the map: {}", problems.join("; ")),
        }
    }
}

/// The game a scenario describes, before any tick: its map placed
/// ([`resolve_map`] over `ir`), or the map's problems.
pub fn game_of(ir: &Ir, scenario: &Scenario) -> Result<FakeGame, RunError> {
    let mut game = FakeGame::with_players(0);
    if let Some(teams) = &scenario.teams {
        game.teams = teams.clone();
    }
    game.round_time_left = scenario.round_time_left.max(0);
    game.variant = scenario.variant.clone();
    let things = resolve_map(ir, scenario.map.as_deref(), &game.teams).map_err(RunError::Map)?;
    label_all(&mut game, &things);
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
    Ok(game)
}

/// Runs `ir` over `scenario`, every tick through [`step`]; queued calls
/// are also made after the last tick. The run does not stop at
/// `end_round`. A map that fails the script's labels is a
/// [`RunError::Map`] and nothing runs.
pub fn run(ir: Ir, scenario: &Scenario) -> Result<RunResult, RunError> {
    let options = scenario.options.unwrap_or_else(|| ir.default_options());
    let mut game = game_of(&ir, scenario)?;
    game.carrier = ir.header.engine();
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
                entry,
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
type slayer

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
                    {"actor_assist":{"killer":0,"assist":1,"class":"elite"}},
                    {"bounce":{"player":1,"object":9,"chain":3,"monster":true}},{"bounced":1}]}]}"#,
        )
        .unwrap();
        assert_eq!(s.seed, 7);
        assert!(s.players[0].alive);
        assert_eq!(s.players[0].unit, Some(100));
        assert_eq!(s.variant["score_limit"], 10);
        let map = s.map.as_ref().unwrap();
        assert_eq!(map[0].number, Some(2));
        assert_eq!((map[0].team, map[1].team), (None, Some(1)));
        assert_eq!(game_of(&ir(), &s).unwrap().object_teams.get(&41), Some(&1));
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
        // A bounce is a single, normal one unless it says otherwise.
        assert_eq!(
            s.ticks[0].events[3].input(),
            Input::Bounce {
                player: 0,
                spot: 0,
                chain: 1,
                monster: false
            }
        );
        assert_eq!(
            s.ticks[0].events[7].input(),
            Input::Bounce {
                player: 1,
                spot: 9,
                chain: 3,
                monster: true
            }
        );
        assert_eq!(s.ticks[0].events[8].input(), Input::Bounced(1));
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
        let src = "rules \"K\" 1\ntype slayer\nglobal var seen: int = 0\n\
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
        let mut g = game_of(&crate::build(src).unwrap().0, &s).unwrap();
        step(&mut vm, &mut g, 0, true, &[s.ticks[0].events[0].input()]);
        assert_eq!(vm.global("seen"), Some(11));
    }

    #[test]
    fn variant_fields_and_the_round_clock_read_the_scenario() {
        let src = "rules \"V\" 1\ntype slayer\nglobal var limit: int = 0\nglobal var left: time = 0t\n\
                   on tick { limit = variant.score_limit; left = round_time_left() }\n";
        let s = Scenario {
            round_time_left: 10,
            ticks: vec![ScenarioTick::default(); 4],
            ..Scenario::default()
        };
        let ir = crate::build(src).unwrap().0;
        let mut vm = Vm::new(ir.clone(), 0, 0).unwrap();
        let mut g = game_of(&ir, &s).unwrap();
        g.carrier = ir.header.engine();
        for t in 0..4 {
            step(&mut vm, &mut g, t, true, &[]);
        }
        // The stock Slayer record's score limit, and three ticks counted.
        assert_eq!(vm.global("limit"), Some(25));
        assert_eq!(vm.global("left"), Some(7));
        let mut set = s.clone();
        set.variant.insert("score_limit".into(), 50);
        let mut g = game_of(&ir, &set).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        step(&mut vm, &mut g, 0, true, &[]);
        assert_eq!(vm.global("limit"), Some(50));
    }

    #[test]
    fn ticks_held_counts_a_zone_held_alone() {
        let src = "rules \"Z\" 1\ntype slayer\nlabel zone: zone\nglobal var held: time = 0t\n\
                   on tick { held = ticks_held(zone, team 0) }\n";
        let mut s = Scenario {
            players: vec![seat(0, 0), seat(1, 1)],
            map: Some(vec![ScenarioLabel {
                number: Some(0),
                ..ScenarioLabel::new("zone", 50)
            }]),
            ..Scenario::default()
        };
        s.players[0].unit = Some(100);
        s.players[1].unit = Some(101);
        let mut vm = Vm::new(crate::build(src).unwrap().0, 0, 0).unwrap();
        let mut g = game_of(&crate::build(src).unwrap().0, &s).unwrap();
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

    /// A Siege-shaped table, built by hand so these tests do not read a
    /// built-in's source.
    fn siege() -> Ir {
        use crate::ir::label_flags::*;
        use crate::ir::{LabelDef, LabelKind};
        let mut ir = ir();
        ir.header.name = "Siege".into();
        ir.labels = vec![
            LabelDef { text: "goal".into(), kind: LabelKind::Zone, min: 1, max: 1, flags: SINGLE, lo: 0, hi: 0 },
            LabelDef { text: "capture".into(), kind: LabelKind::Zone, min: 0, max: 256, flags: NUMBERED | RANGE, lo: 0, hi: 1 },
            LabelDef { text: "flag_spawn".into(), kind: LabelKind::Zone, min: 0, max: 16, flags: PER_TEAM, lo: 0, hi: 0 },
        ];
        crate::verify(&ir).unwrap();
        ir
    }

    fn thing(label: &str, object: u32, number: Option<i32>, class: Class) -> ScenarioLabel {
        ScenarioLabel { number, class: Some(class), ..ScenarioLabel::new(label, object) }
    }

    #[test]
    fn no_map_runs_on_the_minimal_one() {
        let ir = siege();
        let game = game_of(&ir, &Scenario::default()).unwrap();
        // goal, capture 0 and 1, and one spawn a team in play.
        assert_eq!(game.labelled["goal"].len(), 1);
        assert_eq!(game.labelled["capture"].len(), 2);
        assert_eq!(game.labelled["flag_spawn"].len(), 2);
        let first = game.labelled.values().flatten().map(|p| p.object).min().unwrap();
        assert_eq!(first, MAP_BASE);
        assert_eq!(
            census_of(&ir, &things_of(&labels::minimal(&ir, &[0, 1]), MAP_BASE)),
            labels::minimal(&ir, &[0, 1])
        );
        assert!(run(ir, &Scenario::default()).is_ok());
    }

    #[test]
    fn a_map_that_misses_a_label_is_refused_and_nothing_runs() {
        let ir = siege();
        let s = Scenario {
            map: Some(vec![thing("goal", 40, None, Class::Volume)]),
            ..Scenario::default()
        };
        let Err(RunError::Map(problems)) = run(ir.clone(), &s) else { panic!("ran") };
        assert_eq!(
            problems,
            vec![
                "no zone labelled \"capture\" has the number 0; Siege needs 0 and 1",
                "no zone labelled \"capture\" has the number 1; Siege needs 0 and 1",
                "team 0 has no zone labelled \"flag_spawn\"",
                "team 1 has no zone labelled \"flag_spawn\"",
            ]
        );
        // A one-team scenario asks only for team 0's.
        let one = Scenario { teams: Some(vec![0]), ..s };
        let Err(RunError::Map(problems)) = run(ir, &one) else { panic!("ran") };
        assert_eq!(problems.len(), 3);
        assert!(RunError::Map(problems).to_string().starts_with("the map: no zone"));
    }

    #[test]
    fn a_map_with_a_class_runs_and_a_wrong_class_is_named() {
        let ir = siege();
        let mut map = things_of(&labels::minimal(&ir, &[0, 1]), 500);
        let s = Scenario { map: Some(map.clone()), ..Scenario::default() };
        let game = game_of(&ir, &s).unwrap();
        assert_eq!(game.labelled["goal"][0].object, map.iter().find(|t| t.label == "goal").unwrap().object);
        // A flag stands for a zone; an object does not.
        map.iter_mut().find(|t| t.label == "goal").unwrap().class = Some(Class::Flag);
        assert!(game_of(&ir, &Scenario { map: Some(map.clone()), ..Scenario::default() }).is_ok());
        map.iter_mut().find(|t| t.label == "goal").unwrap().class = Some(Class::Object);
        let Err(RunError::Map(problems)) = game_of(&ir, &Scenario { map: Some(map), ..Scenario::default() }) else {
            panic!("placed")
        };
        assert_eq!(
            problems,
            vec!["\"goal\" on this map is an object; Siege needs a zone (a trigger volume or a flag)"]
        );
    }

    #[test]
    fn a_thing_without_a_class_takes_its_labels_kind() {
        let ir = siege();
        assert_eq!(ScenarioLabel::new("goal", 1).class_in(&ir), Class::Volume);
        assert_eq!(ScenarioLabel::new("unlisted", 1).class_in(&ir), Class::Object);
        // Those with no class in a file pass the check on that footing.
        let s: Scenario = serde_json::from_str(
            r#"{"map":[{"label":"goal","object":1},{"label":"capture","object":2,"number":0},
                       {"label":"capture","object":3,"number":1},
                       {"label":"flag_spawn","object":4,"team":0},{"label":"flag_spawn","object":5,"team":1}]}"#,
        )
        .unwrap();
        assert!(game_of(&ir, &s).is_ok());
        // `labelled` is the older name of `map`, and an absent map is not an empty one.
        let old: Scenario = serde_json::from_str(r#"{"labelled":[]}"#).unwrap();
        assert_eq!(old.map, Some(vec![]));
        assert_eq!(serde_json::from_str::<Scenario>("{}").unwrap().map, None);
        assert!(game_of(&ir, &old).is_err());
    }

    #[test]
    fn an_empty_scenario_still_runs_tick_zero() {
        let r = run(ir(), &Scenario::default()).unwrap();
        assert_eq!(r.ticks.len(), 1);
        assert_eq!(r.ticks[0].fired[0].0, "init");
    }
}
