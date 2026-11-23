//! The scripted modes of the built-in list, run over scenarios on the fake
//! game (`rules::scenario`): what each one does in a whole round. The
//! JSON files under `scenarios/` are the ones `h2 rules run` takes.

use rules::builtin;
use rules::ir::OptionValue;
use rules::scenario::{self, RunResult, Scenario, ScenarioEvent, ScenarioTick};
use rules::vm::Arg;

const SIEGE_ATTACKERS_WIN: &str = include_str!("scenarios/siege_attackers_win.json");

const BR: &str = "objects/weapons/rifle/battle_rifle/battle_rifle";
const MAGNUM: &str = "objects/weapons/pistol/magnum/magnum";
const CARBINE: &str = "objects/weapons/rifle/covenant_carbine/covenant_carbine";
const PLASMA_RIFLE: &str = "objects/weapons/rifle/plasma_rifle/plasma_rifle";

/// Each call to `entry`, with its tick.
fn calls<'a>(r: &'a RunResult, entry: &str) -> Vec<(u32, &'a [Arg])> {
    r.ticks
        .iter()
        .flat_map(|t| {
            t.calls
                .iter()
                .filter(move |c| c.entry == entry)
                .map(move |c| (t.tick, c.args.as_slice()))
        })
        .collect()
}

/// The lines every player was shown, in order.
fn lines(r: &RunResult) -> Vec<&str> {
    r.game.messages.iter().map(|m| m.line.as_str()).collect()
}

fn tag(path: &str) -> Arg {
    Arg::Tag(Some(path.into()))
}

fn siege(options: &[(&str, i64)]) -> (rules::Ir, Option<u32>) {
    let ir = rules::compile(builtin::SIEGE).unwrap();
    let given: Vec<(String, OptionValue)> = options
        .iter()
        .map(|(k, v)| (k.to_string(), OptionValue::Int(*v)))
        .collect();
    let bits = (!options.is_empty()).then(|| ir.pack_options(&given).unwrap());
    (ir, bits)
}

/// The attackers stand on each territory alone for the capture time, then
/// on the defenders' flag stand: three phases, and they win at the last.
#[test]
fn siege_attackers_take_the_sites_then_the_flag_stand() {
    let (ir, _) = siege(&[]);
    let s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    // Each side's loadout at its spawn.
    assert_eq!(
        calls(&r, "set_loadout"),
        [
            (0, &[Arg::Player(Some(0)), tag(BR), tag(MAGNUM)][..]),
            (0, &[Arg::Player(Some(1)), tag(CARBINE), tag(PLASMA_RIFLE)][..]),
        ]
    );
    // 15 s on each site (450 ticks, counted from the tick after a zone is
    // first named), then 15 s on the flag stand.
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 2. Then the defenders' flag",
            "The attackers took a site",
            "Final assault: the defenders' flag",
            "The attackers took the flag",
        ]
    );
    assert_eq!(r.ended_at, Some(1351));
    assert_eq!(calls(&r, "end_round").len(), 1);
    assert_eq!(r.game.players[&0].score, 1, "the winners score");
    assert_eq!(r.game.players[&1].score, 0);
}

/// A defender on the site holds the attackers off: the time counts only
/// while one team is on it alone, and the phase clock runs out.
#[test]
fn siege_defenders_hold_a_contested_site_until_the_clock_runs_out() {
    // A one-minute phase and a five-second capture.
    let (ir, options) = siege(&[("phase_time", 60), ("capture_time", 5)]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.options = options;
    s.ticks = vec![ScenarioTick::default(); 1900];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 10, object: 100 },
        ScenarioEvent::Enter { zone: 10, object: 101 },
    ];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        ["Sites to take: 2. Then the defenders' flag", "The defenders held"]
    );
    // The clock is 60 s from the round's start.
    let end = r.ended_at.expect("the clock ends the round");
    assert!((1795..=1801).contains(&end), "{end}");
    assert_eq!(r.game.players[&1].score, 1);
    assert_eq!(r.game.players[&0].score, 0);
}

/// A map with one territory asks for one site, whatever the option says.
#[test]
fn siege_asks_for_no_more_sites_than_the_map_has() {
    let (ir, options) = siege(&[("sites", 4)]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.options = options;
    s.map.as_mut().unwrap().retain(|t| t.object != 11);
    s.ticks.truncate(2);
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(lines(&r), ["Sites to take: 1. Then the defenders' flag"]);
}

/// A map with no territory, or no flag stand for a team, is no Siege map.
#[test]
fn siege_needs_a_territory_and_each_teams_flag_stand() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().retain(|t| t.label != "territory" && t.object != 21);
    match scenario::run(ir, &s) {
        Err(scenario::RunError::Map(problems)) => {
            assert_eq!(problems.len(), 2, "{problems:?}");
            assert!(problems[0].contains("territory"), "{problems:?}");
            assert!(problems[1].contains("ctf_flag_spawn"), "{problems:?}");
        }
        other => panic!("{:?}", other.map(|r| r.ended_at)),
    }
}
