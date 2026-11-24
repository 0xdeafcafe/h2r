//! The runtime over invented guest memory: a multiplayer game on a kit map
//! with one slot, one drop zone and one player, and fake engine callees.

use std::sync::{Arc, Mutex};

use agent::firefight::Refused;
use engine::entry::{EntryFuture, Regs, testing};
use engine::testing::FakeGuest;
use engine::view::fake;
use engine::view::objects::{Body, Header};
use engine::view::players::Player;
use guest_layout::Datum;
use serde_json::json;
use sim_state::Damage;

use super::*;
use crate::firefight::feature::Shared;

const GAME_GLOBALS: u32 = 0x1000_0000;
const GAME_TIME: u32 = 0x1000_1000;
const AI: u32 = 0x1000_2000;
const WORLD: u32 = 0x1000_3000;
const SCNR: u32 = 0x1001_0000;
const SQUADS: u32 = 0x1002_0000;
const STARTS: u32 = 0x1002_1000;
const ZONES: u32 = 0x1002_2000;
const AREAS: u32 = 0x1002_3000;
const FIRING: u32 = 0x1002_4000;
const PALETTE: u32 = 0x1002_5000;
const ORDERS: u32 = 0x1002_6000;
const PLAYERS: u32 = 0x1003_0000;
const OBJECTS: u32 = 0x1004_0000;
const UNIT: u32 = 0x1005_0000;
const SCRATCH: u32 = 0x1006_0000;
/// The variant's flags: `[GAME_GLOBALS] + 8 + 0x17c`, team play in bit 0.
const VARIANT_FLAGS: u32 = GAME_GLOBALS + 8 + 0x17c;
/// The Elite's invented character tag datum.
const ELITE_TAG: u32 = 0xe123_0042;
const TICK: u32 = 10;

fn named(m: &mut FakeGuest, at: u32, name: &str) {
    m.put(at, name.as_bytes()).put_u8(at + name.len() as u32, 0);
}

fn pair(m: &mut FakeGuest, at: u32, count: u32, address: u32) {
    m.put_u32(at, count).put_u32(at + 4, address);
}

/// A multiplayer game (world `mode`) at tick [`TICK`] on `firefight-probe`
/// with AI set up but off. With `kit`: squad `ff_slot_00` (team 3, two
/// starts), zone `ff_drop_00` (two firing positions, area mean (40, 0, 0)),
/// one palette row (the Elite) and the `ff_hunt` order. Player 0 is on team
/// 0 with a unit at (5, 0, 0).
fn guest(kit: bool, world: u32) -> (FakeGuest, u32) {
    let mut m = FakeGuest::new();
    m.put_u32(points::GAME_GLOBALS.base, GAME_GLOBALS)
        .put_u32(GAME_GLOBALS + 8, MODE_MULTIPLAYER)
        .put_u32(points::GAME_TIME.base, GAME_TIME)
        .put_u32(GAME_TIME + 8, TICK)
        .put_u32(points::AI_GLOBALS.base, AI)
        .put_u8(AI, 0)
        .put_u8(AI + 1, 1)
        .put_u32(points::SIM_GLOBALS.base, WORLD)
        .put_u32(WORLD + 8, world)
        .put_u32(guest_layout::global::ACTORS.base, 0)
        .put(
            guest_layout::global::CACHE_MAP_NAME.base,
            b"firefight-probe\0",
        )
        .put_u32(points::SCENARIO.base, SCNR);
    m.map(SCNR, 0x300).map(SCRATCH, 0x20);
    if kit {
        pair(&mut m, SCNR + 0x160, 1, SQUADS);
        m.map(SQUADS, 0x74);
        named(&mut m, SQUADS, "ff_slot_00");
        m.put_u16(SQUADS + 0x24, 3);
        pair(&mut m, SQUADS + 0x48, 2, STARTS);
        m.map(STARTS, 2 * 0x64);
        pair(&mut m, SCNR + 0x168, 1, ZONES);
        m.map(ZONES, 0x38);
        named(&mut m, ZONES, "ff_drop_00");
        pair(&mut m, ZONES + 0x28, 2, FIRING);
        pair(&mut m, ZONES + 0x30, 1, AREAS);
        m.map(AREAS, 0x88).put_f32(AREAS + 0x24, 40.0);
        m.map(FIRING, 2 * 0x20);
        pair(&mut m, SCNR + 0x178, 1, PALETTE);
        m.put_u32(PALETTE + 4, ELITE_TAG);
        pair(&mut m, SCNR + 0x240, 1, ORDERS);
        m.map(ORDERS, 0x7c);
        named(&mut m, ORDERS, "ff_hunt_00");
    }
    m.put_u32(points::PLAYERS.base, PLAYERS)
        .put_u32(points::OBJECTS.base, OBJECTS);
    let players = fake::datums::<Player>(&mut m, PLAYERS, 16);
    let objects = fake::datums::<Header>(&mut m, OBJECTS, 8);
    let (unit, header) = fake::push(&mut m, &objects);
    m.map(UNIT, 0x400);
    header.set_body(&mut m, UNIT).unwrap();
    Body::at(UNIT)
        .set_position(&mut m, [5.0, 0.0, 0.0])
        .unwrap();
    let (_, p0) = fake::push(&mut m, &players);
    p0.set_unit(&mut m, Some(unit)).unwrap();
    p0.set_team(&mut m, 0).unwrap();
    (m, unit.raw())
}

type Skulls = Arc<Mutex<Vec<BTreeSet<Skull>>>>;

fn shared(film: bool) -> (Arc<Shared>, Skulls) {
    let skulls: Skulls = Arc::default();
    let seen = Arc::clone(&skulls);
    let sink = move |on: &BTreeSet<Skull>, _: &mut dyn GuestMemory| {
        seen.lock().unwrap().push(on.clone());
        Ok(())
    };
    (Arc::new(Shared::new(film, Box::new(sink))), skulls)
}

/// Runs the tick's async body; the callee answers the sight test with
/// `sees` and every other call with 0.
fn drive(
    shared: &Arc<Shared>,
    m: FakeGuest,
    work: Work,
    sees: bool,
) -> (Vec<GuestCall>, FakeGuest) {
    let s = Arc::clone(shared);
    let entry = move |cx: Cx| -> EntryFuture {
        Box::pin(run_work(cx, Arc::clone(&s), work.clone(), SCRATCH))
    };
    let run = testing::run(&entry, m, Regs::default(), |c, _| Regs {
        eax: u32::from(c.target == points::AI_LINE_OF_SIGHT.base && sees),
        ..Default::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    (run.calls, run.guest)
}

/// The fixture's map with both teams' base zones, `ff_base_00` at the
/// origin and `ff_base_01` at (80, 0, 0), after its drop zone, and a
/// variant with team play `on` or off.
fn two_team_map(m: &mut FakeGuest, team_play: bool) {
    pair(m, SCNR + 0x168, 3, ZONES);
    for (i, x) in [(1u32, 0.0f32), (2, 80.0)] {
        let (zone, area) = (ZONES + i * 0x38, AREAS + i * 0x88);
        named(m, zone, &format!("ff_base_{:02}", i - 1));
        pair(m, zone + 0x30, 1, area);
        m.map(area, 0x88).put_f32(area + 0x24, x);
    }
    m.put_u32(VARIANT_FLAGS, u32::from(team_play));
}

/// Game Setup chose Crossfire, the two-team wave set.
fn crossfire(it: &Arc<Shared>) {
    it.lock().set_options(Box::new(|| {
        Ok(Options {
            wave_set: 3,
            ..Options::default()
        })
    }));
}

fn wire_command(value: serde_json::Value) -> wire::CommandParams {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_map_without_the_kit_is_refused_with_its_reason() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(false, 1);
    let line = shared.lock().start(&mut m, false);
    assert_eq!(
        line,
        "firefight: off on firefight-probe: the map does not carry the Firefight kit"
    );
    let state = shared.lock().state();
    assert!(!state.running);
    assert_eq!(
        state.refused.as_deref(),
        Some("the map does not carry the Firefight kit")
    );
    assert_eq!(state.map.as_deref(), Some("firefight-probe"));
    assert!(shared.lock().pre_tick(&mut m).is_none());
    assert_eq!(m.u8(AI).unwrap(), 0, "the AI switch is left alone");
    let e = shared
        .lock()
        .command(wire_command(json!({"command": {"respawn": {"player": 0}}})))
        .unwrap_err();
    assert_eq!(
        e,
        Refused::NotRunning("the map does not carry the Firefight kit".into())
    );
}

#[test]
fn outside_a_multiplayer_game_the_mode_is_idle() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    m.put_u32(GAME_GLOBALS + 8, 3);
    assert_eq!(
        shared.lock().start(&mut m, false),
        "firefight: idle: not a multiplayer game"
    );
    assert_eq!(
        shared.lock().state().refused.as_deref(),
        Some("not a multiplayer game")
    );
    assert!(shared.lock().pre_tick(&mut m).is_none());
}

#[test]
fn a_kit_map_starts_the_host_switches_ai_on_and_looks_at_each_zone() {
    let (shared, _) = shared(false);
    let (mut m, unit) = guest(true, 1);
    let line = shared.lock().start(&mut m, false);
    assert_eq!(
        line,
        "firefight: on on firefight-probe as Host: 1 zones, actors on team 3"
    );
    let state = shared.lock().state();
    assert!(state.running);
    assert_eq!(state.role, Some(wire::Role::Host));
    assert_eq!(state.team, Some(3));
    assert_eq!(state.director.as_ref().unwrap().phase, "warmup");
    let work = shared.lock().pre_tick(&mut m).expect("a sight pass is due");
    assert_eq!(m.u8(AI).unwrap(), 1, "the AI switch is on before the tick");
    assert_eq!(work.tick, TICK + 1);
    assert!(work.calls.is_empty());
    assert_eq!(
        work.sight,
        Some(Sight {
            from: vec![([5.0, 0.0, 0.0], unit)],
            zones: vec![[40.0, 0.0, 0.0]],
        })
    );
    // One boundary per tick: the tick's chain may run this hook again.
    assert!(shared.lock().pre_tick(&mut m).is_none());
    let (calls, _) = drive(&shared, m.clone(), work, true);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].target, points::AI_LINE_OF_SIGHT.base);
    assert_eq!(
        (calls[0].esi, calls[0].edi),
        (Some(SCRATCH), Some(SCRATCH + 12))
    );
    assert_eq!(calls[0].edx, Some(unit), "the player's own unit is ignored");
    assert_eq!(
        calls[1].target,
        points::GAME_TICK.base,
        "then the tick itself"
    );
    assert_eq!(shared.lock().running().unwrap().sight(), [true]);
    assert!(!shared.reentry.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn an_operator_command_is_the_engine_call_before_the_next_tick() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    shared.lock().start(&mut m, false);
    shared.lock().post_tick(&mut m, &[]);
    let r = shared
        .lock()
        .command(wire_command(json!({
            "command": {"erase_squad": {"slot": 0}}, "director": "hold"
        })))
        .unwrap();
    let erase = r.appended.unwrap();
    assert_eq!(erase.tick, TICK + 1);
    assert!(r.held);
    let score = shared
        .lock()
        .command(wire_command(json!({
            "command": {"score": {"player": 5, "points": 100, "reason": "kill"}}
        })))
        .unwrap()
        .appended
        .unwrap();
    let work = shared.lock().pre_tick(&mut m).expect("two entries are due");
    assert_eq!(
        work.calls,
        [
            (erase.seq, Call::Squad(SquadOp::Erase { slot: 0 })),
            (
                score.seq,
                Call::Score {
                    player: 5,
                    points: 100
                }
            ),
        ]
    );
    assert_eq!(work.sight, None, "a held director needs no sight");
    let (calls, _) = drive(&shared, m.clone(), work, false);
    // The erase takes the slot's scenario squad; slot 5 has no player, so
    // the score makes no call and is reported with its entry.
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].target, points::AI_ERASE.base);
    assert_eq!(calls[0].args, [0]);
    assert_eq!(calls[1].target, points::GAME_TICK.base);
    let state = shared.lock().state();
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.errors[0].seq, Some(score.seq));
    assert_eq!(state.errors[0].tick, TICK + 1);
    assert_eq!(state.errors[0].error, "no player in slot 5");
    assert_eq!(state.log.unwrap().applied, score.seq);
}

#[test]
fn a_command_the_kit_cannot_take_is_refused_and_appends_nothing() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    shared.lock().start(&mut m, false);
    for (value, why) in [
        (
            json!({"command": {"place_squad": {"slot": 0, "class": "grunt", "count": 2, "zone": 0, "weapon_tier": 0}}}),
            "the kit has no class grunt",
        ),
        (
            json!({"command": {"place_squad": {"slot": 0, "class": "elite", "count": 2, "zone": 4, "weapon_tier": 0}}}),
            "the kit has no zone 4 (it has 1)",
        ),
    ] {
        let e = shared.lock().command(wire_command(value)).unwrap_err();
        assert_eq!(e, Refused::Invalid(why.into()));
    }
    assert_eq!(shared.lock().state().log.unwrap().seq, 0);
}

#[test]
fn a_skull_command_reaches_the_skull_writer() {
    let (shared, skulls) = shared(false);
    let (mut m, _) = guest(true, 1);
    shared.lock().start(&mut m, false);
    for id in ["blind", "mythic"] {
        shared
            .lock()
            .command(wire_command(
                json!({"command": {"skull": {"id": id, "on": true}}, "director": "hold"}),
            ))
            .unwrap();
    }
    assert!(
        shared.lock().pre_tick(&mut m).is_none(),
        "skulls are memory writes"
    );
    assert_eq!(
        *skulls.lock().unwrap(),
        [
            BTreeSet::from([Skull::Blind]),
            BTreeSet::from([Skull::Blind, Skull::Mythic])
        ]
    );
}

#[test]
fn a_client_keeps_the_log_and_refuses_commands() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(true, WORLD_DIST_CLIENT);
    shared.lock().start(&mut m, false);
    let state = shared.lock().state();
    assert_eq!(state.role, Some(wire::Role::Client));
    assert!(state.director.is_none());
    assert!(shared.lock().pre_tick(&mut m).is_none());
    assert_eq!(m.u8(AI).unwrap(), 0, "a client runs no AI");
    let e = shared
        .lock()
        .command(wire_command(
            json!({"command": {"finish": {"outcome": "won"}}}),
        ))
        .unwrap_err();
    assert!(matches!(e, Refused::NotHost(_)), "{e:?}");
}

#[test]
fn a_film_and_a_synchronous_peer_replay_as_replicas() {
    for (film, world) in [(true, 1), (false, WORLD_SYNC_CLIENT)] {
        let (shared, _) = shared(film);
        let (mut m, _) = guest(true, world);
        shared.lock().start(&mut m, film);
        assert_eq!(shared.lock().state().role, Some(wire::Role::Replica));
        let e = shared
            .lock()
            .command(wire_command(json!({"director": "hold"})))
            .unwrap_err();
        assert!(matches!(e, Refused::NotHost(_)), "{e:?}");
        // A replica applies entries, so it switches AI on.
        assert!(shared.lock().pre_tick(&mut m).is_none());
        assert_eq!(m.u8(AI).unwrap(), 1);
    }
}

fn damage(tick: u32, victim: u32, player: u32) -> SimEvent {
    SimEvent::Damage(Damage {
        tick: Some(tick),
        victim: Datum(victim),
        victim_kind: 0,
        victim_player: Datum::NONE,
        owner_player: Datum(player),
        owner_object: Datum::NONE,
        owner_team: 0,
        code: 0,
        effect: Datum::NONE,
        flags: 0,
        body: 0.5,
        shield: 0.0,
        direction: [0.0; 3],
        origin: [0.0; 3],
        position: None,
    })
}

fn killed(unit: u32, killer: Datum) -> SimEvent {
    SimEvent::ActorKilled {
        unit: Datum(unit),
        character: Datum(ELITE_TAG),
        killer,
        weapon: 0,
    }
}

#[test]
fn a_kill_credits_its_killer_and_a_second_player_as_the_assist() {
    const ELITE_UNIT: u32 = 0xa000_0001;
    let mut assists = Assists::default();
    assert!(
        assists
            .take(100, &[damage(100, ELITE_UNIT, 0x0001_0001)])
            .is_empty()
    );
    let kills = assists.take(
        140,
        &[
            damage(140, ELITE_UNIT, 0x0001_0000),
            killed(ELITE_UNIT, Datum(0x0001_0000)),
        ],
    );
    assert_eq!(kills, [(ELITE_TAG, Some(0), Some(1))]);
    // Damage older than five seconds makes no assist, nor does the
    // killer's own.
    let mut assists = Assists::default();
    assists.take(0, &[damage(0, ELITE_UNIT, 0x0001_0001)]);
    let kills = assists.take(
        ASSIST_TICKS + 1,
        &[
            damage(ASSIST_TICKS + 1, ELITE_UNIT, 0x0001_0000),
            killed(ELITE_UNIT, Datum(0x0001_0000)),
        ],
    );
    assert_eq!(kills, [(ELITE_TAG, Some(0), None)]);
    let kills = assists.take(ASSIST_TICKS + 2, &[killed(ELITE_UNIT, Datum::NONE)]);
    assert_eq!(kills, [(ELITE_TAG, None, None)]);
}

#[test]
fn the_director_steps_after_the_tick_and_scores_a_kill_of_a_kit_class() {
    let (shared, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    shared.lock().start(&mut m, false);
    shared.lock().post_tick(&mut m, &[]);
    // Holdout's shared pool is announced at the first step, then the mode
    // state the step leaves.
    let head = shared.lock().state().log.unwrap();
    assert_eq!((head.seq, head.tick), (2, TICK + 1));
    {
        let rt = shared.lock();
        let ops: Vec<_> = rt.running().unwrap().host.log().since(0).to_vec();
        assert!(matches!(ops[0].op, Command::Lives { .. }), "{ops:?}");
        assert!(matches!(ops[1].op, Command::ModeState { .. }), "{ops:?}");
    }
    // The same tick again changes nothing.
    shared.lock().post_tick(&mut m, &[]);
    assert_eq!(shared.lock().state().log.unwrap().seq, 2);
    m.put_u32(GAME_TIME + 8, TICK + 1);
    shared
        .lock()
        .post_tick(&mut m, &[killed(0xa000_0001, Datum(0x0001_0000))]);
    let rt = shared.lock();
    let host = &rt.running().unwrap().host;
    let last = host
        .log()
        .since(2)
        .iter()
        .rev()
        .find(|e| !matches!(e.op, Command::ModeState { .. }))
        .expect("a score entry")
        .op
        .clone();
    assert!(matches!(last, Command::Score { player: 0, .. }), "{last:?}");
    assert_eq!(rt.state().director.unwrap().players, 1);
}

#[test]
fn every_wire_command_is_the_directors_own() {
    use ::firefight::ScoreReason;
    let pairs = [
        (
            json!({"place_squad": {"slot": 1, "class": "elite", "count": 3, "zone": 0, "weapon_tier": 2}}),
            Command::PlaceSquad {
                slot: 1,
                class: "elite".into(),
                count: 3,
                zone: 0,
                weapon_tier: 2,
            },
        ),
        (
            json!({"target": {"slot": 2, "team": 1}}),
            Command::Target { slot: 2, team: 1 },
        ),
        (json!({"hunt": {"slot": 2}}), Command::Hunt { slot: 2 }),
        (
            json!({"team_lives": {"team": 1, "pool": 7}}),
            Command::TeamLives { team: 1, pool: 7 },
        ),
        (
            json!({"score": {"player": 2, "points": 50, "reason": "player_kill"}}),
            Command::Score {
                player: 2,
                points: 50,
                reason: ScoreReason::PlayerKill,
            },
        ),
        (
            json!({"finish": {"outcome": {"team_won": {"team": 1}}}}),
            Command::Finish {
                outcome: Outcome::TeamWon { team: 1 },
            },
        ),
        (
            json!({"finish": {"outcome": "draw"}}),
            Command::Finish {
                outcome: Outcome::Draw,
            },
        ),
        (
            json!({"erase_squad": {"slot": 0}}),
            Command::EraseSquad { slot: 0 },
        ),
        (json!({"round": {"index": 1}}), Command::Round { index: 1 }),
        (json!({"wave": {"index": 2}}), Command::Wave { index: 2 }),
        (
            json!({"skull": {"id": "low_gravity", "on": true}}),
            Command::Skull {
                id: Skull::LowGravity,
                on: true,
            },
        ),
        (
            json!({"lives": {"player": 1, "pool": 4}}),
            Command::Lives {
                player: Some(1),
                pool: 4,
            },
        ),
        (
            json!({"score": {"player": 0, "points": 25, "reason": "assist"}}),
            Command::Score {
                player: 0,
                points: 25,
                reason: ScoreReason::Assist,
            },
        ),
        (
            json!({"respawn": {"player": 3}}),
            Command::Respawn { player: 3 },
        ),
        (
            json!({"finish": {"outcome": "lost"}}),
            Command::Finish {
                outcome: Outcome::Lost,
            },
        ),
    ];
    for (value, want) in pairs {
        let wire: wire::Command = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(command(wire).unwrap(), want, "{value}");
        assert_eq!(serde_json::to_value(&want).unwrap(), value);
    }
}

#[test]
fn the_kit_reply_reads_the_scenario() {
    let (m, _) = guest(true, 1);
    let kit = kit_reply(&m).unwrap();
    assert!(kit.kit);
    assert_eq!(kit.slots[0], Some(0));
    assert!(kit.slots[1..].iter().all(Option::is_none));
    assert_eq!(kit.team, Some(3));
    assert_eq!(kit.zones.len(), 1);
    assert_eq!(kit.zones[0].point, [40.0, 0.0, 0.0]);
    assert_eq!(kit.zones[0].starts, 2);
    assert_eq!(kit.classes, BTreeMap::from([("elite".to_string(), 0)]));
    assert_eq!(kit.hunt_orders, [Some(0)]);
    assert!(kit.bases.is_empty(), "the fixture marks no base zone");
    let (m, _) = guest(false, 1);
    assert_eq!(kit_reply(&m).unwrap(), wire::Kit::default());
}

#[test]
fn game_setup_can_turn_the_mode_off() {
    let (it, _) = shared(false);
    it.lock().set_options(Box::new(|| {
        Ok(Options {
            enabled: false,
            ..Options::default()
        })
    }));
    let (mut m, _) = guest(true, 1);
    assert_eq!(
        it.lock().start(&mut m, false),
        "firefight: idle: Firefight is off in Game Setup"
    );
    let state = it.lock().state();
    assert!(!state.running);
    assert_eq!(
        state.refused.as_deref(),
        Some("Firefight is off in Game Setup")
    );
    assert!(it.lock().pre_tick(&mut m).is_none());
}

/// The wave set, the lives and the difficulty Game Setup chose are the
/// match the director starts from.
#[test]
fn the_pages_choices_start_the_match() {
    let (it, _) = shared(false);
    it.lock().set_options(Box::new(|| {
        Ok(Options {
            wave_set: 2,
            lives: Some(3),
            difficulty: ::firefight::Difficulty::Legendary,
            ..Options::default()
        })
    }));
    let (mut m, _) = guest(true, 1);
    it.lock().start(&mut m, false);
    let state = it.lock().state();
    let director = state.director.expect("the director runs");
    assert_eq!(director.pools, [3], "the page's pool");
    assert_eq!(director.teams, 1);
    assert_eq!(director.scores, [0]);
    assert_eq!(director.cap, 24, "Last Stand's cap for one player");
}

#[test]
fn the_custom_game_wave_record_starts_its_director_on_a_kit_map() {
    crate::lobby_start::set_mode(crate::lobby_start::Mode::Multiplayer);
    let wave = crate::community_variants::listed_named("Last Stand").unwrap();
    let (it, _) = shared(false);
    it.lock().set_options(Box::new(|| Ok(Options { enabled: false, ..Options::default() })));
    let (mut guest, _) = guest(true, 1);
    guest.put(GAME_GLOBALS + bots::guest::OPTIONS + crate::variant_ext::OPTIONS_VARIANT, wave.record.bytes());
    it.lock().start(&mut guest, false);
    assert_eq!(it.lock().state().director.expect("Last Stand starts").cap, 24);
}

/// With nothing chosen the match is the settings' defaults: Holdout.
#[test]
fn a_run_with_no_page_plays_the_defaults() {
    let (it, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    it.lock().start(&mut m, false);
    let director = it.lock().state().director.expect("the director runs");
    assert_eq!(director.pools, [12], "Holdout's pool");
    assert_eq!(director.cap, 12);
}

#[test]
fn a_choice_the_store_cannot_give_refuses_the_mode_by_name() {
    let (it, _) = shared(false);
    it.lock().set_options(Box::new(|| {
        Err("firefight.waveset is None, not a choice".into())
    }));
    let (mut m, _) = guest(true, 1);
    let line = it.lock().start(&mut m, false);
    assert_eq!(
        line,
        "firefight: off on firefight-probe: reading Game Setup's Firefight choices: \
         firefight.waveset is None, not a choice"
    );
    assert!(!it.lock().state().running);
}

/// A player killed by a player is the director's `PlayerKill`, its killer
/// and victim resolved to roster slots as an actor kill's killer is.
#[test]
fn player_kills_come_from_the_sim_events() {
    let kill = |attacker: Datum, victim: Datum| {
        SimEvent::Kill(sim_state::Kill {
            tick: Some(40),
            attacker,
            victim,
            code: 0,
            victim_unit: Datum::NONE,
            attacker_unit: Datum::NONE,
            position: None,
            attacker_position: None,
        })
    };
    let events = [
        kill(Datum(0x0001_0000), Datum(0x0002_0003)),
        damage(40, 0xa000_0001, 0x0001_0000),
        // A fall: nobody is credited.
        kill(Datum::NONE, Datum(0x0003_0001)),
        // No victim player: left out.
        kill(Datum(0x0001_0000), Datum::NONE),
        killed(0xa000_0001, Datum(0x0001_0000)),
    ];
    assert_eq!(
        player_kills(&events),
        [
            PlayerKill {
                killer: Some(0),
                victim: 3
            },
            PlayerKill {
                killer: None,
                victim: 1
            },
        ]
    );
    assert_eq!(outcome_name(Outcome::TeamWon { team: 1 }), "team_won");
    assert_eq!(outcome_name(Outcome::Draw), "draw");
}

/// The end report Live takes: one score per team, and the winner named
/// only by a two-team win.
#[test]
fn the_end_report_carries_each_teams_score_and_the_winner() {
    let ended = |outcome, teams| crate::firefight::host::Ended {
        tick: 900,
        set: "Crossfire".into(),
        outcome,
        rounds_cleared: 2,
        lives_used: 4,
        teams,
        scores: [300, 150],
    };
    let won = ended_report(ended(Outcome::TeamWon { team: 1 }, 2));
    assert_eq!(won.validate(), Ok(()));
    assert_eq!(
        (won.outcome.as_str(), won.team, won.teams, won.scores.as_slice()),
        ("team_won", Some(1), 2, &[300, 150][..])
    );
    let draw = ended_report(ended(Outcome::Draw, 2));
    assert_eq!(draw.validate(), Ok(()));
    assert_eq!((draw.outcome.as_str(), draw.team), ("draw", None));
    let one = ended_report(ended(Outcome::Won, 1));
    assert_eq!(one.validate(), Ok(()));
    assert_eq!((one.teams, one.scores.as_slice()), (1, &[300][..]));
}

/// Two player teams need a team game; with one and the kit's two bases
/// the match runs.
#[test]
fn two_teams_need_team_play_and_a_base_each() {
    let (it, _) = shared(false);
    crossfire(&it);
    let (mut m, _) = guest(true, 1);
    two_team_map(&mut m, false);
    assert_eq!(
        it.lock().start(&mut m, false),
        "firefight: off on firefight-probe: two Firefight teams need a team game, \
         and Team Play did not turn on for this game"
    );
    assert!(!it.lock().state().running);
    // Team play on, but a kit with no bases.
    let (mut m, _) = guest(true, 1);
    m.put_u32(VARIANT_FLAGS, 1);
    let line = it.lock().start(&mut m, false);
    assert!(line.ends_with("marks 0 bases, and 2 player teams need one each"), "{line}");
    // Both.
    let (mut m, _) = guest(true, 1);
    two_team_map(&mut m, true);
    let line = it.lock().start(&mut m, false);
    assert!(line.starts_with("firefight: on on firefight-probe as Host"), "{line}");
    let director = it.lock().state().director.expect("the director runs");
    assert_eq!((director.teams, director.pools), (2, vec![10, 10]));
    assert_eq!(teams_refusal(1, None), None, "one team needs no team game");
    assert!(teams_refusal(2, None).is_some());
}

/// The options block the game is about to copy at activation, its mode
/// and its variant's flags.
const OPTIONS: u32 = 0x1007_0000;

fn options_block(m: &mut FakeGuest, mode: u32, flags: u32) {
    m.map(OPTIONS, 0x200)
        .put_u32(OPTIONS, mode)
        .put_u32(OPTIONS + OPTIONS_VARIANT_FLAGS, flags);
}

/// Two Firefight teams on a kit map turn the variant's team play on before
/// the game copies its options, so the match starts as teams instead of
/// being refused. Other flags stay; a variant already on is left alone.
#[test]
fn two_teams_turn_team_play_on_before_the_game_starts() {
    let (it, _) = shared(false);
    crossfire(&it);
    let (mut m, _) = guest(true, 1);
    two_team_map(&mut m, false);
    options_block(&mut m, MODE_MULTIPLAYER, 0x40);
    assert_eq!(
        it.lock().activate(&mut m, OPTIONS, false).as_deref(),
        Some("firefight: two teams: Team Play is on for this game")
    );
    let flags = m.u32(OPTIONS + OPTIONS_VARIANT_FLAGS).unwrap();
    assert_eq!(flags, 0x41);
    assert_eq!(
        it.lock().activate(&mut m, OPTIONS, false),
        None,
        "already on"
    );
    // The game copies the options into its globals; the match runs.
    m.put_u32(VARIANT_FLAGS, flags);
    let line = it.lock().start(&mut m, false);
    assert!(
        line.starts_with("firefight: on on firefight-probe as Host"),
        "{line}"
    );
    assert_eq!(it.lock().state().director.unwrap().teams, 2);
}

/// One team, Firefight off, a map without the kit, another mode and a
/// film's playback leave team play as the variant has it; Teams set to two
/// on a one-team wave set turns it on.
#[test]
fn only_two_firefight_teams_on_a_kit_map_turn_team_play_on() {
    let unchanged = |options: Options, kit: bool, mode: u32, film: bool| {
        let (it, _) = shared(false);
        it.lock().set_options(Box::new(move || Ok(options.clone())));
        let (mut m, _) = guest(kit, 1);
        options_block(&mut m, mode, 0);
        let line = it.lock().activate(&mut m, OPTIONS, film);
        let flags = m.u32(OPTIONS + OPTIONS_VARIANT_FLAGS).unwrap();
        assert_eq!(line.is_none(), flags == 0, "{line:?}");
        flags == 0
    };
    let two = Options {
        wave_set: 3,
        ..Options::default()
    };
    let mp = MODE_MULTIPLAYER;
    assert!(unchanged(Options::default(), true, mp, false), "one team");
    assert!(unchanged(two.clone(), false, mp, false), "no kit");
    assert!(unchanged(two.clone(), true, 1, false), "a campaign game");
    assert!(unchanged(two.clone(), true, mp, true), "a film");
    let off = Options {
        enabled: false,
        ..two.clone()
    };
    assert!(unchanged(off, true, mp, false), "Firefight off");
    let one = Options {
        teams: Some(1),
        ..two
    };
    assert!(unchanged(one, true, mp, false), "Crossfire by one team");
    let holdout_two = Options {
        teams: Some(2),
        ..Options::default()
    };
    assert!(wants_team_play(&holdout_two, true));
    assert!(!unchanged(holdout_two, true, mp, false), "Teams set to two");
    assert_eq!(with_team_play(1, true), None);
    assert_eq!(with_team_play(0, false), None);
    assert_eq!(with_team_play(0x10, true), Some(0x11));
}

/// Each multiplayer start publishes the match it plays for the session
/// data (a Live host's `NewSession.firefight`): the set, its id, teams,
/// difficulty and the kit version on a kit map with Firefight on, none
/// otherwise, so a stale match does not linger. A film publishes nothing.
#[test]
fn each_multiplayer_start_names_its_match_for_the_session() {
    type Sent = Arc<Mutex<Vec<Option<live_proto::session::Firefight>>>>;
    let start = |options: Options, kit: bool, film: bool| {
        let (it, _) = shared(false);
        let sent: Sent = Arc::default();
        let seen = Arc::clone(&sent);
        it.lock().set_options(Box::new(move || Ok(options.clone())));
        it.lock()
            .set_session(Box::new(move |f| seen.lock().unwrap().push(f)));
        let (mut m, _) = guest(kit, 1);
        options_block(&mut m, MODE_MULTIPLAYER, 1);
        it.lock().activate(&mut m, OPTIONS, film);

        sent.lock().unwrap().clone()
    };
    let crossfire = Options {
        wave_set: 3,
        difficulty: ::firefight::Difficulty::Heroic,
        ..Options::default()
    };
    let sent = start(crossfire.clone(), true, false);
    let def = crossfire.definition().unwrap();
    let expected = live_proto::session::Firefight {
        set: "Crossfire".into(),
        id: def.id().unwrap().to_string(),
        teams: 2,
        difficulty: 2,
        kit: ::firefight::KIT_VERSION,
    };
    assert_eq!(sent, [Some(expected.clone())]);
    assert_eq!(expected.validate(), Ok(()));
    assert_eq!(session_firefight(&crossfire, true), Some(expected));
    // Holdout: one team.
    let one = start(Options::default(), true, false);
    assert_eq!(
        one[0].as_ref().map(|f| (f.set.as_str(), f.teams)),
        Some(("Holdout", 1))
    );
    // No kit, or Firefight off: the session names no match.
    assert_eq!(start(crossfire.clone(), false, false), [None]);
    let off = Options {
        enabled: false,
        ..crossfire.clone()
    };
    assert_eq!(start(off, true, false), [None]);
    assert!(start(crossfire, true, true).is_empty(), "a film");
}

/// The Clankers are told the match: the player teams, the actors' team,
/// the bases, the drop zones, each team's pool and whether a wave is on.
#[test]
fn the_clankers_are_told_the_teams_bases_and_lives() {
    use bots::firefight::{Context, Phase as BotPhase};
    let (it, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    it.lock().start(&mut m, false);
    let one = bot_context(&it.lock().running().unwrap().host);
    assert_eq!(
        one,
        Some(Context {
            player_teams: vec![0],
            ai_team: 3,
            bases: Vec::new(),
            lives: vec![Some(12)],
            phase: BotPhase::Between,
            drop_zones: vec![[40.0, 0.0, 0.0]],
        })
    );
    crossfire(&it);
    let (mut m, _) = guest(true, 1);
    two_team_map(&mut m, true);
    it.lock().start(&mut m, false);
    let two = bot_context(&it.lock().running().unwrap().host).unwrap();
    assert_eq!(two.player_teams, [0, 1]);
    assert_eq!(two.ai_team, 3, "never a player team");
    assert_eq!(two.bases, [[0.0, 0.0, 0.0], [80.0, 0.0, 0.0]]);
    assert_eq!(two.lives, [Some(10), Some(10)]);
    assert_eq!(two.phase, BotPhase::Between, "warmup");
}

/// The HUD's blob comes from the host's director and its log, and says so
/// when the host's log has finished the game; a replica has no director and
/// so no blob.
#[test]
fn the_hud_blob_comes_from_the_hosts_director_and_log() {
    let (it, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    it.lock().start(&mut m, false);
    it.lock().post_tick(&mut m, &[]);
    let mode = {
        let rt = it.lock();
        let game = rt.running().unwrap();
        let blob = mode_blob(&game.host, game.players).unwrap().unwrap();
        hud::Mode::decode(&blob).unwrap()
    };
    assert_eq!(mode.stage, hud::Stage::Warmup);
    assert_eq!((mode.round, mode.rounds, mode.waves), (0, 3, 5));
    assert_eq!(mode.lives, hud::Lives::Shared(12));
    it.lock()
        .command(wire_command(
            json!({"command": {"finish": {"outcome": "won"}}}),
        ))
        .unwrap();
    let won = {
        let rt = it.lock();
        let game = rt.running().unwrap();
        mode_blob(&game.host, game.players).unwrap().unwrap()
    };
    assert_eq!(hud::Mode::decode(&won).unwrap().stage, hud::Stage::Won);

    let (replica, _) = shared(true);
    let (mut m, _) = guest(true, 1);
    replica.lock().start(&mut m, true);
    let rt = replica.lock();
    let game = rt.running().unwrap();
    assert!(mode_blob(&game.host, game.players).is_none());
}

#[cfg(feature = "rules")]
#[test]
fn the_kit_director_announces_its_type_only_for_its_own_record() {
    use crate::rules::guest::{VARIANT, VARIANT_ENGINE};
    use variants::ext::{Ext, Mode, Script};
    let (mut m, _) = guest(true, 1);
    for (engine, mode, owns) in [
        (2, Mode::Firefight, true),
        (2, Mode::None, true),
        (2, Mode::Rules, false),
        (7, Mode::None, false),
    ] {
        m.put_u32(GAME_GLOBALS + VARIANT + VARIANT_ENGINE, engine);
        let ext = Ext { script: Script { mode, prefix: [9; 8], options: 0 }, ..Ext::default() };
        m.put(GAME_GLOBALS + crate::variant_ext::BLOCK, &ext.encode());
        assert_eq!(director_owns_line(&m), owns, "{engine}, {mode:?}");
    }
}

#[test]
fn change_rules_crossfire_starts_its_director_with_the_stored_setting_off() {
    use crate::pause_change_map::{Draft, game_types};

    let listed = crate::community_variants::listed_named("Crossfire").unwrap();
    let row = game_types::listed(listed.record.clone()).unwrap();
    let mut draft = Draft::default();
    draft.pick_rules(&row);
    let variant = draft.variant.unwrap();
    let (shared, _) = shared(false);
    shared.lock().set_options(Box::new(|| Ok(Options { enabled: false, ..Options::default() })));
    let (mut m, _) = guest(true, 1);
    two_team_map(&mut m, true);
    m.put(GAME_GLOBALS + bots::guest::OPTIONS + crate::variant_ext::OPTIONS_VARIANT, variant.bytes());
    shared.lock().start(&mut m, false);
    let state = shared.lock().state();
    assert!(state.running, "{:?}", state.refused);
    assert_eq!(state.director.unwrap().teams, 2);
}
