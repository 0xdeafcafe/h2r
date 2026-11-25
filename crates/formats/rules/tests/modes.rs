//! The scripted modes of the built-in list, run over scenarios on the fake
//! game (`rules::scenario`): what each one does in a whole round. The
//! JSON files under `scenarios/` are the ones `h2 rules run` takes. The
//! scripts are the `modes` package's, read through its manifest as a set
//! of packages gives them (`rules::builtin::package_sources`).

use rules::bodies::Body;
use rules::builtin::{Registry, package_sources};
use rules::fake::FakeSpawns;
use rules::ir::OptionValue;
use rules::labels::Class;
use rules::scenario::{self, RunResult, Scenario, ScenarioEvent, ScenarioLabel, ScenarioPick, ScenarioTick};
use rules::vm::Arg;

const SIEGE_ATTACKERS_WIN: &str = include_str!("scenarios/siege_attackers_win.json");
const SIEGE_RESPAWN_WAVES: &str = include_str!("scenarios/siege_respawn_waves.json");

/// The scripts of the `core` and `modes` packages, from their folders.
fn set() -> &'static Registry {
    static SET: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    SET.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content");
        let mut sources = Vec::new();
        for package in ["core", "modes"] {
            let dir = root.join(package);
            let text = std::fs::read_to_string(dir.join("package.toml")).unwrap();
            let manifest = content::package::Manifest::from_toml(&text).unwrap();
            let (mut s, problems) =
                package_sources(&manifest, |name| std::fs::read(dir.join(name)).ok());
            assert_eq!(problems, Vec::<String>::new());
            sources.append(&mut s);
        }
        let r = Registry::from_sources(sources);
        assert_eq!(r.problems, Vec::<String>::new());
        r
    })
}

/// The script of a `modes` item, by the stem of its file.
fn script(key: &str) -> &'static str {
    &set().script(key).unwrap_or_else(|| panic!("the set has no {key}")).source
}

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
    let ir = rules::compile(script("siege")).unwrap();
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

/// Each site and the final stand show their boundary in the defenders'
/// colour, and a site the attackers take turns to theirs
/// (docs/game/gametype-markers.md, "The boundary in play").
#[test]
fn siege_shows_each_site_in_its_holders_colour() {
    let (ir, _) = siege(&[]);
    let s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let shown: Vec<(u32, Vec<Arg>)> = calls(&r, "show_area").into_iter().map(|(t, a)| (t, a.to_vec())).collect();
    let show = |zone: u32, team: u8| vec![Arg::Object(Some(zone)), Arg::Team(Some(team))];
    let args: Vec<Vec<Arg>> = shown.iter().map(|(_, a)| a.clone()).collect();
    assert_eq!(
        args,
        [
            show(10, 1),
            show(11, 1),
            // The defenders' flag stand.
            show(21, 1),
            // Each site as the attackers take it.
            show(10, 0),
            show(11, 0),
        ]
    );
    assert_eq!(shown[0].0, 0, "at the round's start");
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

/// The attackers spawn as Spartans and the defenders as Elites, from the
/// round's start.
#[test]
fn siege_gives_each_side_its_own_body() {
    let (ir, _) = siege(&[]);
    let s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let start = calls(&r, "message_all")[0].0;
    let made: Vec<(u32, Vec<Arg>)> = calls(&r, "set_body")
        .into_iter()
        .map(|(t, a)| (t, a.to_vec()))
        .collect();
    let body = |team: u8, path: &str| {
        (
            start,
            vec![Arg::Team(Some(team)), Arg::Tag(Some(path.into()))],
        )
    };
    assert_eq!(
        made,
        [
            body(0, "objects/characters/masterchief/masterchief_mp"),
            body(1, "objects/characters/elite/elite_mp"),
        ]
    );
    assert_eq!(r.game.bodies.get(&0), Some(&Body::Spartan));
    assert_eq!(r.game.bodies.get(&1), Some(&Body::Elite));
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
/// while one team is on it alone, and the phase clock runs out. The attacker
/// is still on the site then, so the phase runs on for the overtime, and a
/// contested front does not hold it open past that: the defenders win when
/// the overtime runs out (docs/game/rules-engine.md, "Phases: attack and
/// defend").
#[test]
fn siege_defenders_hold_a_contested_site_until_the_clock_runs_out() {
    // A one-minute phase and a five-second capture.
    let (ir, options) = siege(&[("phase_time", 60), ("capture_time", 5)]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.options = options;
    s.ticks = vec![ScenarioTick::default(); 2800];
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
        [
            "Sites to take: 2. Then the defenders' flag",
            "Overtime: the attackers are still on the objective",
            "The defenders held"
        ]
    );
    // The clock is 60 s from the round's start, then the overtime's 30 s.
    let end = r.ended_at.expect("the overtime ends the round");
    assert!((2695..=2701).contains(&end), "{end}");
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

/// The phase's clock reads on the HUD: `koth.phase`, the third widget,
/// set to the seconds left, once for each second that passes.
#[test]
fn siege_shows_the_phase_clock_a_second_at_a_time() {
    let (ir, _) = siege(&[]);
    let s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let phase: Vec<i32> = calls(&r, "hud_value")
        .into_iter()
        .filter_map(|(_, a)| match a {
            [Arg::Int(2), Arg::Int(left)] => Some(*left),
            _ => None,
        })
        .collect();
    assert!((179..=180).contains(&phase[0]), "{phase:?}");
    // A phase runs 180 s and the first takes 15: the clock counts down a
    // second at a time through them, then the next phase starts again from
    // the full clock.
    assert!(phase.windows(2).take(14).all(|w| w[1] == w[0] - 1), "{phase:?}");
    assert!(phase.contains(&170), "{phase:?}");
    assert!(phase.iter().filter(|&&n| n == 180).count() >= 2, "each phase's clock starts full");
    let shown: Vec<bool> = calls(&r, "hud_show")
        .into_iter()
        .filter_map(|(_, a)| match a {
            [Arg::Int(2), Arg::Bool(on)] => Some(*on),
            _ => None,
        })
        .collect();
    assert_eq!(shown.first(), Some(&false), "hidden at the round's start");
    assert_eq!(shown.last(), Some(&false), "and when the round ends");
    assert!(shown.contains(&true));
}

/// A scenario with one attacker (unit 100) walking into the first site at
/// `tick`, over 60 s phases and 10 s captures.
fn overtime_scenario(options: Option<u32>, enter: usize, ticks: usize) -> Scenario {
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.options = options;
    s.ticks = vec![ScenarioTick::default(); ticks];
    s.ticks[0].events = vec![ScenarioEvent::Spawn(0), ScenarioEvent::Spawn(1)];
    s.ticks[enter].events = vec![ScenarioEvent::Enter { zone: 10, object: 100 }];
    s
}

fn overtime_options() -> (rules::Ir, Option<u32>) {
    siege(&[("phase_time", 60), ("capture_time", 10)])
}

const OVERTIME: &str = "Overtime: the attackers are still on the objective";

/// An attacker midway through a capture when the clock runs out holds the
/// phase open, and finishing the capture takes the site.
#[test]
fn siege_runs_a_phase_on_while_an_attacker_is_capturing() {
    let (ir, options) = overtime_options();
    let s = overtime_scenario(options, 1700, 2100);
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 2. Then the defenders' flag",
            OVERTIME,
            "The attackers took a site",
        ]
    );
    assert_eq!(r.ended_at, None, "the round goes on");
    assert_eq!(calls(&r, "end_round").len(), 0);
    // The overtime banner, said once.
    let banners: Vec<&[Arg]> = calls(&r, "hud_text")
        .into_iter()
        .filter(|(_, a)| a[1] == Arg::Text("OVERTIME".into()))
        .map(|(_, a)| a)
        .collect();
    assert_eq!(banners.len(), 1);
}

/// The attackers leaving the objective in overtime ends the phase for the
/// defenders at once.
#[test]
fn siege_ends_an_overtime_when_the_attackers_leave() {
    let (ir, options) = overtime_options();
    let mut s = overtime_scenario(options, 1700, 2100);
    s.ticks[1900].events = vec![ScenarioEvent::Leave { zone: 10, object: 100 }];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 2. Then the defenders' flag",
            OVERTIME,
            "The defenders held",
        ]
    );
    let end = r.ended_at.expect("the defenders win");
    assert!((1899..=1903).contains(&end), "{end}");
    assert_eq!(r.game.players[&1].score, 1);
}

/// A contested objective stops the capture but not the overtime, which is
/// bounded: it runs its own clock out and the defenders win.
#[test]
fn siege_bounds_an_overtime_the_defenders_contest() {
    let (ir, options) = overtime_options();
    let mut s = overtime_scenario(options, 1700, 2800);
    s.ticks[1700].events.push(ScenarioEvent::Enter { zone: 10, object: 101 });
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 2. Then the defenders' flag",
            OVERTIME,
            "The defenders held",
        ]
    );
    // 60 s of phase, then the 30 s overtime.
    let end = r.ended_at.expect("the overtime runs out");
    assert!((2697..=2703).contains(&end), "{end}");
}

/// No attacker on the objective at the clock: no overtime, as before.
#[test]
fn siege_gives_no_overtime_to_an_empty_objective() {
    let (ir, options) = overtime_options();
    let s = overtime_scenario(options, 2000, 2100);
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(lines(&r), ["Sites to take: 2. Then the defenders' flag", "The defenders held"]);
    assert!((1795..=1801).contains(&r.ended_at.unwrap()));
}

/// A barrier the author numbered for a phase stands open from that phase:
/// the first at the round's start, the second as the first site falls, the
/// third as the second does. Each opens once, through `set_open`.
#[test]
fn siege_opens_the_map_with_the_phases() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    for (object, number) in [(40, 0), (41, 1), (42, 2)] {
        s.map.as_mut().unwrap().push(ScenarioLabel {
            number: Some(number),
            class: Some(Class::Object),
            ..ScenarioLabel::new("siege_barrier", object)
        });
    }
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    // Each barrier is put below its closed place once, in phase order; the
    // fake reads every place as the origin.
    let drop = -(64 << 16);
    assert_eq!(
        r.game.steps,
        [
            (40, [0, 0, drop], [0; 3]),
            (41, [0, 0, drop], [0; 3]),
            (42, [0, 0, drop], [0; 3]),
        ]
    );
    // The opens come with the phases, at the round's start and as each
    // site falls (a barrier already open is asked again with no effect).
    let first_open = |o: u32| {
        calls(&r, "set_open")
            .into_iter()
            .filter(|(_, a)| a[0] == Arg::Object(Some(o)) && a[1] == Arg::Bool(true))
            .map(|(t, _)| t)
            .min()
            .unwrap()
    };
    assert_eq!(first_open(40), 0);
    assert!((449..=453).contains(&first_open(41)), "{}", first_open(41));
    assert!((899..=905).contains(&first_open(42)), "{}", first_open(42));
}

/// A map with no barriers plays as before, and a barrier numbered past
/// the last phase never opens.
#[test]
fn siege_leaves_a_barrier_for_a_later_phase_closed() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().push(ScenarioLabel {
        number: Some(3),
        class: Some(Class::Object),
        ..ScenarioLabel::new("siege_barrier", 43)
    });
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert!(r.game.steps.is_empty(), "{:?}", r.game.steps);
    assert!(calls(&r, "set_open").iter().all(|(_, a)| a[1] == Arg::Bool(false)));
}

/// The lobby's loadout set picks each side's weapons: close quarters or
/// long range, from the multiplayer list.
#[test]
fn siege_gives_each_side_the_loadout_set_the_lobby_chose() {
    let weapons = |kit: &str| {
        let ir = rules::compile(script("siege")).unwrap();
        let bits = ir
            .pack_options(&[("kits".to_string(), OptionValue::Name(kit.into()))])
            .unwrap();
        let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
        s.options = Some(bits);
        s.ticks.truncate(2);
        let r = scenario::run(ir, &s).unwrap();
        assert_eq!(r.script_error, None);
        calls(&r, "set_loadout").into_iter().map(|(_, a)| a.to_vec()).collect::<Vec<_>>()
    };
    let kit = |slot: u8, a: &str, b: &str| vec![Arg::Player(Some(slot)), tag(a), tag(b)];
    assert_eq!(
        weapons("close"),
        [
            kit(0, "objects/weapons/rifle/shotgun/shotgun", MAGNUM),
            kit(1, "objects/weapons/rifle/brute_plasma_rifle/brute_plasma_rifle", "objects/weapons/pistol/needler/needler"),
        ]
    );
    assert_eq!(
        weapons("ranged"),
        [
            kit(0, "objects/weapons/rifle/sniper_rifle/sniper_rifle", "objects/weapons/rifle/smg/smg"),
            kit(1, "objects/weapons/rifle/beam_rifle/beam_rifle", "objects/weapons/pistol/plasma_pistol/plasma_pistol"),
        ]
    );
    assert_eq!(
        weapons("standard"),
        [kit(0, BR, MAGNUM), kit(1, CARBINE, PLASMA_RIFLE)]
    );
}

/// Siege offers a loadout picker at the round's start: one picker of three
/// rows, in the order the lobby's `kits` option names the sets.
#[test]
fn siege_offers_a_loadout_picker_at_the_rounds_start() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.ticks.truncate(2);
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let text = |t: &str| Arg::Text(t.into());
    assert_eq!(calls(&r, "offer_pick"), [(0, &[Arg::Int(0), text("LOADOUT")][..])]);
    let rows: Vec<_> = calls(&r, "pick_row")
        .into_iter()
        .map(|(tick, a)| (tick, a.to_vec()))
        .collect();
    assert_eq!(
        rows,
        [
            (0, vec![Arg::Int(0), text("STANDARD")]),
            (0, vec![Arg::Int(0), text("CLOSE QUARTERS")]),
            (0, vec![Arg::Int(0), text("LONG RANGE")]),
        ]
    );
}

/// A player's pick takes the lobby's set's place from its next spawn; a
/// player who picked nothing keeps the lobby's.
#[test]
fn siege_gives_a_player_the_loadout_it_picked() {
    // Spawned at the start on the lobby's set; player 0 picks long range
    // and player 1 picks standard (over a lobby of close quarters) after
    // tick 1, and both spawn again at tick 3.
    let respawned = |lobby: &str, picks: Vec<ScenarioPick>| {
        let ir = rules::compile(script("siege")).unwrap();
        let bits = ir
            .pack_options(&[("kits".to_string(), OptionValue::Name(lobby.into()))])
            .unwrap();
        let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
        s.options = Some(bits);
        s.ticks.truncate(5);
        s.ticks[1].picks = picks;
        s.ticks[3].events = vec![ScenarioEvent::Spawn(0), ScenarioEvent::Spawn(1)];
        let r = scenario::run(ir, &s).unwrap();
        assert_eq!(r.script_error, None);
        assert!(r.refused_picks.is_empty());
        calls(&r, "set_loadout")
            .into_iter()
            .map(|(t, a)| (t, a.to_vec()))
            .collect::<Vec<_>>()
    };
    let kit = |slot: u8, a: &str, b: &str| vec![Arg::Player(Some(slot)), tag(a), tag(b)];
    let sniper = ("objects/weapons/rifle/sniper_rifle/sniper_rifle", "objects/weapons/rifle/smg/smg");
    let picks = vec![
        ScenarioPick { player: 0, slot: 0, row: 2 },
        ScenarioPick { player: 1, slot: 0, row: 0 },
    ];
    let got = respawned("close", picks);
    let shotgun = "objects/weapons/rifle/shotgun/shotgun";
    let brute = "objects/weapons/rifle/brute_plasma_rifle/brute_plasma_rifle";
    let needler = "objects/weapons/pistol/needler/needler";
    assert_eq!(
        got,
        [
            // The lobby's close quarters at the first spawn.
            (0, kit(0, shotgun, MAGNUM)),
            (0, kit(1, brute, needler)),
            // Then each player's own pick.
            (3, kit(0, sniper.0, sniper.1)),
            (3, kit(1, CARBINE, PLASMA_RIFLE)),
        ]
    );
    // Nobody picks: the lobby's set again at the second spawn.
    let got = respawned("ranged", vec![]);
    assert_eq!(got[0].1, got[2].1);
    assert_eq!(got[1].1, got[3].1);
}

/// A pick that is not one of the three rows is refused, and the player
/// keeps the lobby's set.
#[test]
fn siege_refuses_a_pick_past_its_rows() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.ticks.truncate(5);
    s.ticks[1].picks = vec![ScenarioPick { player: 0, slot: 0, row: 3 }];
    s.ticks[3].events = vec![ScenarioEvent::Spawn(0)];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.refused_picks, [(1, ScenarioPick { player: 0, slot: 0, row: 3 })]);
    let loadouts = calls(&r, "set_loadout");
    assert_eq!(loadouts.len(), 3);
    assert_eq!(loadouts[2].1, loadouts[0].1, "the lobby's standard set");
}

/// Siege's options fit the record's 32 bits and its worst tick fits the
/// op budget with the HUD, overtime and barriers it carries.
#[test]
fn siege_fits_its_option_bits_and_tick_budget() {
    let ir = rules::compile(script("siege")).unwrap();
    let bits: u32 = ir.options.iter().map(rules::ir::OptionDef::bits).sum();
    assert!(bits <= rules::ir::OPTION_BITS, "{bits} bits");
    let report = rules::verify::verify(&ir).unwrap();
    assert!(report.fits(), "worst tick {}", report.worst_tick);
}

/// A thing the author placed: a flag with a label, a team and a number.
fn placed(label: &str, object: u32, team: Option<u8>, number: Option<i32>) -> ScenarioLabel {
    ScenarioLabel {
        team,
        number,
        class: Some(Class::Flag),
        ..ScenarioLabel::new(label, object)
    }
}

/// The sites the author placed are the sites, whatever territory flags
/// the map has: one `siege_site` beside two territories is one site to
/// take, the defenders stand at it, and holding it opens the final
/// assault.
#[test]
fn siege_takes_the_sites_the_author_placed() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    s.map.as_mut().unwrap().push(placed("siege_site", 12, None, None));
    s.ticks = vec![ScenarioTick::default(); 600];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 12, object: 100 },
    ];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    assert_eq!(
        lines(&r),
        [
            "Sites to take: 1. Then the defenders' flag",
            "Final assault: the defenders' flag",
        ]
    );
    let first = calls(&r, "spawn_near")
        .into_iter()
        .find(|(_, a)| a[0] == Arg::Team(Some(1)))
        .map(|(_, a)| a.to_vec());
    assert_eq!(first, Some(vec![Arg::Team(Some(1)), Arg::Object(Some(12)), Arg::Int(4)]));
}

/// A placed `siege_goal` is the defenders' final stand, over the map's
/// own CTF flag stand: holding it wins, and the defenders spawn near it.
#[test]
fn siege_goes_for_the_goal_the_author_placed() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let map = s.map.as_mut().unwrap();
    map.retain(|t| t.label != "territory");
    map.push(placed("siege_goal", 40, None, None));
    s.ticks = vec![ScenarioTick::default(); 1000];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 40, object: 100 },
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
    assert_eq!(r.game.spawns.get(&1), Some(&FakeSpawns::Near { zone: 40, count: 4 }));
}

/// A `siege_line` for a side and a phase is where that side spawns in the
/// phase, over the phase's own site or stand; a phase with none placed
/// keeps the default.
#[test]
fn siege_spawns_each_side_at_the_lines_the_author_placed() {
    let (ir, _) = siege(&[]);
    let mut s: Scenario = serde_json::from_str(SIEGE_ATTACKERS_WIN).unwrap();
    let map = s.map.as_mut().unwrap();
    map.push(placed("siege_line", 30, Some(0), Some(0)));
    map.push(placed("siege_line", 31, Some(1), Some(0)));
    map.push(placed("siege_line", 32, Some(0), Some(1)));
    s.ticks = vec![ScenarioTick::default(); 500];
    s.ticks[0].events = vec![
        ScenarioEvent::Spawn(0),
        ScenarioEvent::Spawn(1),
        ScenarioEvent::Enter { zone: 10, object: 100 },
    ];
    let r = scenario::run(ir, &s).unwrap();
    assert_eq!(r.script_error, None);
    let made: Vec<Vec<Arg>> = calls(&r, "spawn_near").into_iter().map(|(_, a)| a.to_vec()).collect();
    let near = |team: u8, zone: Option<u32>| vec![Arg::Team(Some(team)), Arg::Object(zone), Arg::Int(4)];
    assert_eq!(
        made,
        [
            // Phase 0: each side's default, then its placed line.
            near(0, Some(20)),
            near(0, Some(30)),
            near(1, Some(10)),
            near(1, Some(31)),
            // Site 10 taken: the attackers at their phase 1 line, the
            // defenders (no line placed for them) at the next site.
            near(0, Some(10)),
            near(0, Some(32)),
            near(1, Some(11)),
        ]
    );
    assert_eq!(r.game.spawns.get(&0), Some(&FakeSpawns::Near { zone: 32, count: 4 }));
    assert_eq!(r.game.spawns.get(&1), Some(&FakeSpawns::Near { zone: 11, count: 4 }));
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
    let ir = rules::compile(script("gun_game")).unwrap();
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
    let ir = rules::compile(script("race")).unwrap();
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
    let ir = rules::compile(script("race")).unwrap();
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
    let ir = rules::compile(script("race")).unwrap();
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
    let ir = rules::compile(script("vip")).unwrap();
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
    let ir = rules::compile(script("vip")).unwrap();
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

/// The set is what the constants are: the files the tests above read are
/// the ones `rules::builtin` bakes.
#[test]
fn the_scripts_of_the_set_are_the_baked_ones() {
    let baked = Registry::baked();
    assert_eq!(baked.problems, Vec::<String>::new());
    assert_eq!(set().scripts(), baked.scripts());
}
