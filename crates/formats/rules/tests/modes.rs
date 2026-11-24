//! The scripted modes of the built-in list, run over scenarios on the fake
//! game (`rules::scenario`): what each one does in a whole round. The
//! JSON files under `scenarios/` are the ones `h2 rules run` takes.

use rules::builtin;
use rules::fake::FakeSpawns;
use rules::ir::OptionValue;
use rules::scenario::{self, RunResult, Scenario, ScenarioEvent, ScenarioTick};
use rules::vm::Arg;

const SIEGE_ATTACKERS_WIN: &str = include_str!("scenarios/siege_attackers_win.json");
const SIEGE_RESPAWN_WAVES: &str = include_str!("scenarios/siege_respawn_waves.json");

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

/// Each side spawns near its line, and the lines move with the phases:
/// the attackers from their flag stand (20) to each site they take (10,
/// then 11), the defenders from the site still to take (10, then 11) to
/// their flag stand (21) for the final assault.
#[test]
fn siege_moves_each_sides_spawns_up_with_the_phases() {
    let (ir, _) = siege(&[]);
    let s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    // The ticks of "Sites to take", "The attackers took a site" and
    // "Final assault".
    let shown: Vec<u32> = calls(&r, "message_all").iter().map(|&(t, _)| t).collect();
    let (start, first, last) = (shown[0], shown[1], shown[2]);
    let near = |tick: u32, team: u8, zone: u32| {
        (
            tick,
            vec![Arg::Team(Some(team)), Arg::Object(Some(zone)), Arg::Int(4)],
        )
    };
    let made: Vec<(u32, Vec<Arg>)> = calls(&r, "spawn_near")
        .into_iter()
        .map(|(t, a)| (t, a.to_vec()))
        .collect();
    assert_eq!(
        made,
        [
            near(start, 0, 20),
            near(start, 1, 10),
            near(first, 0, 10),
            near(first, 1, 11),
            near(last, 0, 11),
            near(last, 1, 21),
        ]
    );
    // The fake game keeps each side's last line.
    let rule = |zone| FakeSpawns::Near { zone, count: 4 };
    assert_eq!(r.game.spawns.get(&0), Some(&rule(11)));
    assert_eq!(r.game.spawns.get(&1), Some(&rule(21)));
}

/// A map with no flag stands lifts both sides' lines where a stand would
/// be: the attackers start on the engine's own spawns, and the defenders
/// keep to the next site until the last is taken.
#[test]
fn siege_with_no_flag_stands_leaves_those_lines_to_the_engine() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().retain(|t| t.label != "ctf_flag_spawn");
    s.ticks.truncate(2);
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        calls(&r, "spawn_near")
            .into_iter()
            .map(|(_, a)| a.to_vec())
            .collect::<Vec<_>>(),
        [
            vec![Arg::Team(Some(0)), Arg::Object(None), Arg::Int(4)],
            vec![Arg::Team(Some(1)), Arg::Object(Some(10)), Arg::Int(4)],
        ]
    );
    assert_eq!(r.game.spawns.get(&0), None, "no stand: the engine's spawns");
    assert_eq!(r.game.spawns.get(&1), Some(&FakeSpawns::Near { zone: 10, count: 4 }));
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

/// The variant's one life holds each death's respawn — a spawn event the
/// engine would make mid-wave changes nothing and runs no handler — and
/// each side's dead come back together when its own wave runs out: team
/// 0's first death at tick 1 brings it back at 300, team 1's at 3 at 302.
#[test]
fn siege_holds_each_death_for_its_sides_wave() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_RESPAWN_WAVES).unwrap();
    s.ticks.resize(311, ScenarioTick::default());
    s.ticks[50].events = vec![ScenarioEvent::Spawn(0), ScenarioEvent::Spawn(2)];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    // The tick-0 spawns gave each side its loadout; the held ones at 50
    // ran no handler.
    assert_eq!(
        calls(&r, "set_loadout"),
        [
            (0, &[Arg::Player(Some(0)), tag(BR), tag(MAGNUM)][..]),
            (0, &[Arg::Player(Some(1)), tag(CARBINE), tag(PLASMA_RIFLE)]),
            (0, &[Arg::Player(Some(2)), tag(BR), tag(MAGNUM)]),
        ]
    );
    assert!(
        r.ticks[50].fired.iter().all(|(e, _)| *e != "spawn"),
        "{:?}",
        r.ticks[50].fired
    );
    // Each side's dead respawn together, on its own wave.
    assert_eq!(
        calls(&r, "respawn"),
        [
            (300, &[Arg::Player(Some(0))][..]),
            (300, &[Arg::Player(Some(2))]),
            (302, &[Arg::Player(Some(1))]),
        ]
    );
    for (_, p) in &r.game.players {
        assert!(p.alive);
        assert_eq!(p.lives, 1, "the wave gave the life back");
    }
    assert_eq!(lines(&r), ["Sites to take: 2. Then the defenders' flag"]);
}

/// A map with no territory starts at the final assault: the attackers
/// hold the defenders' flag stand and win.
#[test]
fn siege_with_no_territories_goes_straight_at_the_flag_stand() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().retain(|t| t.label != "territory");
    s.ticks = vec![ScenarioTick::default(); 1000];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 21, object: 100 },
    ];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 0. Then the defenders' flag",
            "The attackers took the flag",
        ]
    );
    let end = r.ended_at.expect("the flag ends it");
    assert!((449..=451).contains(&end), "{end}");
    assert_eq!(calls(&r, "end_round").len(), 1);
    assert_eq!(r.game.players[&0].score, 1);
    assert_eq!(r.game.players[&1].score, 0);
}

/// A map with no flag stand stops at the sites: taking them all wins it
/// for the attackers.
#[test]
fn siege_with_no_flag_stand_wins_at_the_last_site() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().retain(|t| t.label != "ctf_flag_spawn");
    s.ticks = vec![ScenarioTick::default(); 1000];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 10, object: 100 },
    ];
    s.ticks[451].events = vec![
        ScenarioEvent::Leave { zone: 10, object: 100 },
        ScenarioEvent::Enter { zone: 11, object: 100 },
    ];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 2",
            "The attackers took a site",
            "The attackers took every site",
        ]
    );
    let end = r.ended_at.expect("the last site ends it");
    assert!((899..=902).contains(&end), "{end}");
    assert_eq!(calls(&r, "end_round").len(), 1);
    assert_eq!(r.game.players[&0].score, 1);
    assert_eq!(r.game.players[&1].score, 0);
}

/// A map with neither label starts and draws at once: nothing to take.
#[test]
fn siege_draws_at_once_on_a_map_with_nothing_to_take() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().clear();
    s.ticks = vec![ScenarioTick::default(); 3];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        ["This map has nothing to take: Siege needs territories or a flag stand"]
    );
    assert_eq!(calls(&r, "end_round"), [(0, &[Arg::Team(None)][..])]);
    assert_eq!(r.game.players[&0].score, 0);
    assert_eq!(r.game.players[&1].score, 0);
}

// ---- Gun Game ----

const GUN_GAME_CLIMB: &str = include_str!("scenarios/gun_game_climb.json");

/// The ladder, bottom to top, as `gun_game.rules` lists it.
const LADDER: [&str; 12] = [
    "objects/weapons/rifle/smg/smg",
    "objects/weapons/rifle/battle_rifle/battle_rifle",
    "objects/weapons/rifle/covenant_carbine/covenant_carbine",
    "objects/weapons/rifle/plasma_rifle/plasma_rifle",
    "objects/weapons/rifle/shotgun/shotgun",
    "objects/weapons/rifle/sniper_rifle/sniper_rifle",
    "objects/weapons/rifle/beam_rifle/beam_rifle",
    "objects/weapons/support_low/brute_shot/brute_shot",
    "objects/weapons/support_high/rocket_launcher/rocket_launcher",
    "objects/weapons/pistol/needler/needler",
    "objects/weapons/pistol/magnum/magnum",
    "objects/weapons/melee/energy_blade/energy_blade",
];

fn gun_game(options: &[(&str, bool)]) -> (rules::Ir, Option<u32>) {
    let ir = rules::compile(builtin::GUN_GAME).unwrap();
    let given: Vec<(String, OptionValue)> = options
        .iter()
        .map(|(k, v)| (k.to_string(), OptionValue::Bool(*v)))
        .collect();
    let bits = (!options.is_empty()).then(|| ir.pack_options(&given).unwrap());
    (ir, bits)
}

fn loadout(p: u8, path: &str) -> Vec<Arg> {
    vec![Arg::Player(Some(p)), tag(path), Arg::Tag(None)]
}

/// Player 0 kills player 1 twelve times: each kill swaps 0's weapon for
/// the next rung's on the kill's tick, player 1 respawns on the bottom
/// rung, and the kill past the top rung ends the round.
#[test]
fn gun_game_a_kill_climbs_a_rung_at_once_and_the_last_rung_wins() {
    let (ir, _) = gun_game(&[]);
    let s: Scenario = serde_json::from_str(GUN_GAME_CLIMB).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let mut want: Vec<(u32, Vec<Arg>)> = vec![(0, loadout(0, LADDER[0])), (0, loadout(1, LADDER[0]))];
    for (k, rung) in LADDER.iter().enumerate().skip(1) {
        let at = 10 * k as u32;
        want.push((at, loadout(0, rung)));
        want.push((at + 5, loadout(1, LADDER[0])));
    }
    let got: Vec<(u32, Vec<Arg>)> = calls(&r, "set_loadout")
        .into_iter()
        .filter(|(t, _)| *t < 120)
        .map(|(t, a)| (t, a.to_vec()))
        .collect();
    assert_eq!(got, want);
    // The twelfth kill, past the sword, ends it with no new weapon.
    assert!(calls(&r, "set_loadout").iter().all(|(t, _)| *t != 120));
    assert_eq!(lines(&r), ["player 0 finished the ladder"]);
    assert_eq!(r.ended_at, Some(120));
    assert_eq!(calls(&r, "end_round").len(), 1);
    assert_eq!(r.game.players[&0].score, 12);
}

/// A suicide sends the player down a rung for the next spawn, unless the
/// option is off.
#[test]
fn gun_game_a_suicide_demotes_unless_the_option_is_off() {
    let run = |demotes: bool| {
        let (ir, options) = gun_game(&[("suicide_demotes", demotes)]);
        let mut s: Scenario = serde_json::from_str(GUN_GAME_CLIMB).unwrap();
        s.options = options;
        s.ticks = vec![ScenarioTick::default(); 5];
        s.ticks[0].events = vec![ScenarioEvent::Spawn(0), ScenarioEvent::Spawn(1)];
        s.ticks[1].events = vec![ScenarioEvent::Kill { victim: 1, killer: Some(0) }];
        s.ticks[3].events = vec![ScenarioEvent::Kill { victim: 0, killer: Some(0) }];
        s.ticks[4].events = vec![ScenarioEvent::Spawn(0)];
        let r = scenario::run(ir, &s).unwrap();
        assert_eq!(r.script_error, None);
        let at_spawn: Vec<Vec<Arg>> = calls(&r, "set_loadout")
            .into_iter()
            .filter(|(t, _)| *t == 4)
            .map(|(_, a)| a.to_vec())
            .collect();
        at_spawn
    };
    assert_eq!(run(true), [loadout(0, LADDER[0])]);
    assert_eq!(run(false), [loadout(0, LADDER[1])]);
}

/// In a team game a kill of a team mate climbs nothing.
#[test]
fn gun_game_a_team_kill_climbs_nothing() {
    let (ir, _) = gun_game(&[]);
    let mut s: Scenario = serde_json::from_str(GUN_GAME_CLIMB).unwrap();
    s.variant.insert("team_play".into(), 1);
    s.players[1].team = 0;
    s.ticks = vec![ScenarioTick::default(); 3];
    s.ticks[0].events = vec![ScenarioEvent::Spawn(0), ScenarioEvent::Spawn(1)];
    s.ticks[1].events = vec![ScenarioEvent::Kill { victim: 1, killer: Some(0) }];
    let r = scenario::run(ir, &s).unwrap();
    assert!(calls(&r, "set_loadout").iter().all(|(t, _)| *t == 0));
    assert_eq!(r.game.players[&0].score, 0);
}

// ---- Race ----

const RACE_LAPS: &str = include_str!("scenarios/race_laps.json");

/// Hills 1, 3 and 5: a lap is 1, 3, 5 and back to 1. Player 0 runs three
/// and wins; player 1 skips hill 3 and never laps.
#[test]
fn race_laps_run_the_hills_in_number_order_from_the_lowest() {
    let ir = rules::compile(builtin::RACE).unwrap();
    let s: Scenario = serde_json::from_str(RACE_LAPS).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Race: 3 laps. Start at hill 1",
            "Lap 1 of 3",
            "Lap 2 of 3",
            "player 0 won the race",
        ]
    );
    // The third lap ends on hill 1 at tick 21.
    assert_eq!(r.ended_at, Some(21));
    assert_eq!(calls(&r, "end_round").len(), 1);
    assert_eq!(r.game.players[&0].score, 3);
    assert_eq!(r.game.players[&1].score, 0);
}

/// A map whose corners are all one hill has no track: nobody laps.
#[test]
fn race_needs_two_hills() {
    let ir = rules::compile(builtin::RACE).unwrap();
    let mut s: Scenario = serde_json::from_str(RACE_LAPS).unwrap();
    for t in s.map.as_mut().unwrap() {
        t.number = Some(1);
    }
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(lines(&r), ["This map has one hill: Race needs two"]);
    assert!(calls(&r, "score").is_empty());
    assert_eq!(r.ended_at, None);
}

/// A map with fewer than three hill corners is no Race map.
#[test]
fn race_needs_a_hills_corners() {
    let ir = rules::compile(builtin::RACE).unwrap();
    let mut s: Scenario = serde_json::from_str(RACE_LAPS).unwrap();
    s.map.as_mut().unwrap().truncate(2);
    match scenario::run(ir, &s) {
        Err(scenario::RunError::Map(problems)) => {
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert!(problems[0].contains("hill_corner"), "{problems:?}");
        }
        other => panic!("{:?}", other.map(|r| r.ended_at)),
    }
}

// ---- VIP ----

const VIP_SUCCESSION: &str = include_str!("scenarios/vip_succession.json");

/// Players 0 and 2 start as their teams' VIPs. An enemy VIP's kill is
/// worth three, the role passes to a living teammate, then to a dead one
/// when that is all the team has, and the team that reaches the score
/// limit (7 here) wins.
#[test]
fn vip_the_role_passes_on_and_an_enemy_vip_scores_more() {
    let ir = rules::compile(builtin::VIP).unwrap();
    let s: Scenario = serde_json::from_str(VIP_SUCCESSION).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "player 0 is your VIP",
            "player 2 is your VIP",
            "player 1 killed the VIP player 2",
            "player 3 is your VIP",
            "player 0 killed the VIP player 3",
            "player 2 is your VIP",
        ]
    );
    assert_eq!(r.game.players[&0].score, 1 + 3);
    assert_eq!(r.game.players[&1].score, 3);
    assert_eq!(r.ended_at, Some(4));
    assert_eq!(calls(&r, "end_round").len(), 1);
}

/// A VIP who quits hands the role on; a VIP with no teammate left keeps
/// it, and is worth the VIP's points again.
#[test]
fn vip_a_quit_hands_the_role_on_and_a_lone_vip_keeps_it() {
    let ir = rules::compile(builtin::VIP).unwrap();
    let mut s: Scenario = serde_json::from_str(VIP_SUCCESSION).unwrap();
    s.variant.insert("score_limit".into(), 0);
    s.ticks = vec![ScenarioTick::default(); 6];
    s.ticks[0].events = (0..4).map(ScenarioEvent::Spawn).collect();
    s.ticks[1].events = vec![ScenarioEvent::Quit(2)];
    s.ticks[2].events = vec![ScenarioEvent::Kill { victim: 3, killer: Some(0) }];
    s.ticks[3].events = vec![ScenarioEvent::Spawn(3)];
    s.ticks[4].events = vec![ScenarioEvent::Kill { victim: 3, killer: Some(0) }];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "player 0 is your VIP",
            "player 2 is your VIP",
            "player 3 is your VIP",
            "player 0 killed the VIP player 3",
            "player 0 killed the VIP player 3",
        ]
    );
    assert_eq!(r.game.players[&0].score, 6);
    // No score limit: the round runs on.
    assert_eq!(r.ended_at, None);
}
