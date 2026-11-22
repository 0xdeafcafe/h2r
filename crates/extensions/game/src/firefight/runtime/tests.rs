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
        named(&mut m, ORDERS, "ff_hunt");
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
    // Holdout's shared pool is announced at the first step.
    let head = shared.lock().state().log.unwrap();
    assert_eq!((head.seq, head.tick), (1, TICK + 1));
    // The same tick again changes nothing.
    shared.lock().post_tick(&mut m, &[]);
    assert_eq!(shared.lock().state().log.unwrap().seq, 1);
    m.put_u32(GAME_TIME + 8, TICK + 1);
    shared
        .lock()
        .post_tick(&mut m, &[killed(0xa000_0001, Datum(0x0001_0000))]);
    let rt = shared.lock();
    let host = &rt.running().unwrap().host;
    let last = host
        .log()
        .since(1)
        .last()
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
        (json!({"hunt": {"slot": 2}}), Command::Hunt { slot: 2 }),
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
    assert_eq!(kit.hunt_order, Some(0));
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
    assert_eq!(director.pool, 3, "the page's pool");
    assert_eq!(director.cap, 24, "Last Stand's cap for one player");
}

/// With nothing chosen the match is the settings' defaults: Holdout.
#[test]
fn a_run_with_no_page_plays_the_defaults() {
    let (it, _) = shared(false);
    let (mut m, _) = guest(true, 1);
    it.lock().start(&mut m, false);
    let director = it.lock().state().director.expect("the director runs");
    assert_eq!(director.pool, 12, "Holdout's pool");
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
