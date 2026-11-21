//! The chain's rule on lists of ticks: bounces in a row, what ends a
//! chain, the medals at each count, Bounced, and the bots.

use hud_kit_core::huds::superbounce::Medal::*;
use superbounce::Kind;

use super::*;

const MINUS_3: u32 = 0xffff_fffd;
const UNIT: u32 = 0xe001_0007;

/// A strip at (10, 0, -3), its edge along Y.
fn spots() -> Vec<Spot> {
    let stand = [10.0, 0.0, -3.0];
    vec![Spot {
        kind: Kind::Bounce,
        edge: [[10.0, -1.0, -3.0], [10.0, 1.0, -3.0]],
        stand,
        fall: 1.0,
        launch: 17.9,
        score: 1.0,
        ..Spot::default()
    }]
}

const STRIP: Vec3 = [10.0, 0.0, -2.8];
const FLOOR: Vec3 = [0.0, 0.0, 0.0];
/// Floor beside the strip, over 0.5 from its edge.
const BESIDE: Vec3 = [11.5, 0.0, -2.8];
/// Below the strip, off its edge, where a fall out of the map goes.
const BELOW: Vec3 = [10.5, 0.0, -4.0];

fn tick(at: Vec3, vz: f32, airborne: bool) -> PlayerTick {
    PlayerTick {
        unit: UNIT,
        position: at,
        velocity: [0.0, 0.0, vz],
        airborne,
        word: Some(MINUS_3),
    }
}

/// Falling onto `at`, primed.
fn falling(at: Vec3) -> PlayerTick {
    tick(at, -6.0, true)
}

/// The ticks of one bounce off the strip and the flight up after it: the
/// fall, the launch (the airborne bit clear on its tick and the next, as
/// a launch may leave it), then airborne.
fn bounce_and_fly() -> Vec<PlayerTick> {
    let mut v = vec![
        falling(STRIP),
        tick(STRIP, 12.0, false),
        tick(STRIP, 11.8, false),
    ];
    v.extend((0..10).map(|i| tick(STRIP, 11.0 - i as f32, true)));
    v
}

/// Steps `ticks`, returning the events.
fn run(c: &mut Chain, ticks: &[PlayerTick]) -> Vec<Event> {
    let spots = spots();
    ticks
        .iter()
        .filter_map(|t| c.step(Some(t), &spots))
        .collect()
}

fn bounces(n: usize) -> Vec<PlayerTick> {
    (0..n).flat_map(|_| bounce_and_fly()).collect()
}

#[test]
fn the_medals_at_each_count_each_tier_once() {
    assert!(bounce_medals(0).is_empty());
    assert_eq!(bounce_medals(1), &[Superbounce]);
    assert_eq!(bounce_medals(2), &[Superbounce, DoubleBounce]);
    assert_eq!(bounce_medals(3), &[Superbounce, TripleBounce]);
    assert_eq!(bounce_medals(4), &[Superbounce, QuadBounce]);
    assert_eq!(bounce_medals(5), &[Superbounce, ChainBounce]);
    assert_eq!(bounce_medals(6), &[Superbounce, Bouncetacular]);
    assert_eq!(bounce_medals(7), &[Superbounce]);
    assert_eq!(bounce_medals(40), &[Superbounce]);
    // A chain of seven: seven Superbounces and each tier once.
    let mut all: Vec<Medal> = (1..=7).flat_map(|n| bounce_medals(n).to_vec()).collect();
    all.sort();
    assert_eq!(all.iter().filter(|m| **m == Superbounce).count(), 7);
    for tier in [
        DoubleBounce,
        TripleBounce,
        QuadBounce,
        ChainBounce,
        Bouncetacular,
    ] {
        assert_eq!(all.iter().filter(|m| **m == tier).count(), 1, "{tier:?}");
    }
    assert!(!all.contains(&Bounced));
    // The line shows the highest.
    assert_eq!(highest(bounce_medals(5)), Some(ChainBounce));
    assert_eq!(highest(bounce_medals(7)), Some(Superbounce));
    // The ids are the film's and Live's.
    let ids: Vec<&str> = medals_by_id().into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        [
            "superbounce",
            "double-bounce",
            "triple-bounce",
            "quad-bounce",
            "chain-bounce",
            "bouncetacular",
            "bounced"
        ]
    );
}

#[test]
fn bounces_with_no_ground_between_count_up() {
    let mut c = Chain::default();
    let events = run(&mut c, &bounces(7));
    let counts: Vec<u32> = events.iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1, 2, 3, 4, 5, 6, 7]);
    assert!(events.iter().all(|e| matches!(e, Event::Bounce { .. })));
    assert!(c.live());
    // Four and six earn their tiers; seven Superbounce alone.
    assert_eq!(events[3].medals(), [Superbounce, QuadBounce]);
    assert_eq!(events[5].medals(), [Superbounce, Bouncetacular]);
    assert_eq!(events[6].medals(), [Superbounce]);
}

#[test]
fn a_landing_off_the_strip_ends_the_chain() {
    let mut c = Chain::default();
    let mut ticks = bounces(2);
    // Down on the floor beside the strip: a touchdown, not a bounce.
    ticks.push(falling(BESIDE));
    ticks.push(tick(BESIDE, 0.0, false));
    ticks.extend(bounces(1));
    let counts: Vec<u32> = run(&mut c, &ticks).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1, 2, 1]);
}

#[test]
fn a_landing_on_the_strip_that_does_not_bounce_ends_it() {
    let mut c = Chain::default();
    let mut ticks = bounces(1);
    ticks.push(falling(STRIP));
    ticks.push(tick(STRIP, 2.0, false));
    ticks.extend(bounces(1));
    let counts: Vec<u32> = run(&mut c, &ticks).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1, 1]);
}

#[test]
fn the_airborne_bit_clear_from_the_third_tick_ends_it() {
    // Clear on the launch tick and the two after: still live.
    let mut c = Chain::default();
    let ticks = vec![
        falling(STRIP),
        tick(STRIP, 12.0, false),
        tick(STRIP, 11.8, false),
    ];
    run(&mut c, &ticks);
    assert!(c.live(), "the launch's ticks");
    // The second tick after the bounce, clear: still the launch's.
    run(&mut c, &[tick(STRIP, 11.5, false)]);
    assert!(c.live(), "the second tick after");
    // The third, clear and rising (no touchdown): the chain ends.
    run(&mut c, &[tick(STRIP, 11.2, false)]);
    assert!(!c.live());
    let counts: Vec<u32> = run(&mut c, &bounces(1)).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1]);
}

#[test]
fn a_teleport_or_a_new_unit_ends_it() {
    let mut c = Chain::default();
    run(&mut c, &bounces(1));
    assert!(c.live());
    let far = [STRIP[0] + 4.0, STRIP[1], STRIP[2] + 5.0];
    run(&mut c, &[tick(far, 5.0, true)]);
    assert!(!c.live(), "moved 3 units and more in a tick");
    let mut c = Chain::default();
    run(&mut c, &bounces(1));
    let other = PlayerTick {
        unit: UNIT + 0x10000,
        ..tick(STRIP, 5.0, true)
    };
    assert!(run(&mut c, &[other]).is_empty());
    assert!(!c.live(), "a vehicle or a new life");
}

#[test]
fn an_unprimed_landing_by_the_strip_is_no_bounce() {
    // An explosion beside the strip: out fast, but the word was not -3.
    let mut c = Chain::default();
    let mut fall = falling(STRIP);
    fall.word = Some(0);
    let events = run(&mut c, &[fall, tick(STRIP, 12.0, false)]);
    assert!(events.is_empty());
    assert!(!c.live());
}

#[test]
fn a_fall_out_of_the_map_during_a_bounce_is_bounced() {
    let spots = spots();
    // In a chain, falling, then gone: Bounced, with the chain's count.
    let mut c = Chain::default();
    run(&mut c, &bounces(2));
    run(&mut c, &[falling(BELOW)]);
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 2 }));
    assert_eq!(Event::Bounced { chain: 2 }.medals(), [Bounced]);
    assert!(!c.live());
    // Primed and falling, no chain: Bounced at 0.
    let mut c = Chain::default();
    run(&mut c, &[falling(BELOW)]);
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 0 }));
    // Standing when it went: a death, not a fall.
    let mut c = Chain::default();
    run(&mut c, &[tick(FLOOR, 0.0, false)]);
    assert_eq!(c.step(None, &spots), None);
    // Falling, but neither primed nor in a chain.
    let mut c = Chain::default();
    let mut f = falling(BELOW);
    f.word = Some(0);
    run(&mut c, &[f]);
    assert_eq!(c.step(None, &spots), None);
    // Gone and back as a new unit on the same tick, falling: Bounced too.
    let mut c = Chain::default();
    run(&mut c, &[falling(BELOW)]);
    let respawned = PlayerTick {
        unit: UNIT + 0x10000,
        ..tick(FLOOR, 0.0, false)
    };
    assert_eq!(run(&mut c, &[respawned]), [Event::Bounced { chain: 0 }]);
}

#[test]
fn every_player_has_a_chain_bots_too() {
    let spots = spots();
    let mut m = Medals::new();
    let ticks = bounces(3);
    let mut awards = Vec::new();
    for (i, t) in ticks.iter().enumerate() {
        let seen = [
            // The local player, user 0, in slot 0.
            Seen {
                slot: 0,
                user: Some(0),
                tick: Some(*t),
            },
            // A bot in slot 5: no local user, the same bounces.
            Seen {
                slot: 5,
                user: None,
                tick: Some(PlayerTick {
                    unit: UNIT + 5,
                    ..*t
                }),
            },
            // A player standing about.
            Seen {
                slot: 2,
                user: None,
                tick: Some(PlayerTick {
                    unit: UNIT + 2,
                    ..tick(FLOOR, 0.0, false)
                }),
            },
        ];
        awards.extend(m.step(i as u32, &seen, &spots));
    }
    assert_eq!(awards.len(), 6);
    let of = |slot: u8| -> Vec<u32> {
        awards
            .iter()
            .filter(|a| a.slot == slot)
            .map(|a| a.chain)
            .collect()
    };
    assert_eq!(of(0), [1, 2, 3]);
    assert_eq!(of(5), [1, 2, 3]);
    assert!(
        awards
            .iter()
            .filter(|a| a.slot == 0)
            .all(|a| a.user == Some(0))
    );
    assert!(
        awards
            .iter()
            .filter(|a| a.slot == 5)
            .all(|a| a.user.is_none())
    );
    // The seq counts up over the game.
    let seqs: Vec<u64> = awards.iter().map(|a| a.seq).collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]));
    // The records: three Superbounces, a Double, a Triple each; nothing
    // for the player who stood.
    for slot in [0, 5] {
        let r = &m.records()[&slot];
        assert_eq!((r.chain, r.live), (3, true));
        assert_eq!(r.counts[&Superbounce], 3);
        assert_eq!(r.counts[&DoubleBounce], 1);
        assert_eq!(r.counts[&TripleBounce], 1);
    }
    assert!(!m.records().contains_key(&2));
    // Recent keeps the last eight... of six.
    assert_eq!(m.recent().count(), 6);
    // A slot that drops out of the list has no unit: its chain ends.
    m.step(999, &[], &spots);
    assert!(!m.chain(0).unwrap().live());
}

#[test]
fn the_test_award_is_a_real_bounces_and_recent_keeps_eight() {
    let mut m = Medals::new();
    let a = m.test(0, Some(0), 10, 5, false);
    assert_eq!(
        (a.chain, a.medals.clone()),
        (5, vec![Superbounce, ChainBounce])
    );
    assert_eq!(a.user, Some(0));
    assert!(m.chain(0).unwrap().live());
    assert_eq!(m.records()[&0].chain, 5);
    let b = m.test(0, Some(0), 11, 0, true);
    assert_eq!(b.medals, [Bounced]);
    assert!(!m.chain(0).unwrap().live());
    for i in 0..10 {
        m.test(1, None, 20 + i, 1, false);
    }
    assert_eq!(m.recent().count(), RECENT);
    assert_eq!(m.recent().last().unwrap().tick, 29);
}

/// A recorder that keeps its calls.
#[derive(Default)]
struct Calls(std::cell::RefCell<Vec<(&'static str, u32, u32)>>);

impl Recorder for Calls {
    fn superbounce(&self, _: &runtime::Guest, player: u32, chain: u32) {
        self.0.borrow_mut().push(("superbounce", player, chain));
    }
    fn bounced(&self, _: &runtime::Guest, player: u32, chain: u32) {
        self.0.borrow_mut().push(("bounced", player, chain));
    }
}

fn award_of(slot: u8, chain: u32, medals: Vec<Medal>) -> Award {
    Award {
        seq: 1,
        slot,
        user: None,
        tick: 40,
        chain,
        medals,
    }
}

/// Each bounce is one `superbounce` call at its count and each Bounced one
/// `bounced` call with the chain it ended, for every player, bots
/// included, on the console that records the game.
#[test]
fn every_award_is_recorded_once_by_its_kind() {
    let calls = Calls::default();
    // A guest with no simulation globals reads no world mode: a local
    // game, which records.
    let guest = runtime::Guest::new();
    let awards = [
        award_of(0, 1, bounce_medals(1).to_vec()),
        award_of(3, 6, bounce_medals(6).to_vec()),
        award_of(2, 4, vec![Bounced]),
        award_of(5, 0, vec![Bounced]),
    ];
    record(&calls, &guest, &awards);
    assert_eq!(
        calls.0.into_inner(),
        vec![
            ("superbounce", 0, 1),
            ("superbounce", 3, 6),
            ("bounced", 2, 4),
            ("bounced", 5, 0),
        ]
    );
}

/// The host and a local game record; a client of either kind does not.
#[test]
fn only_the_console_whose_simulation_is_the_games_records() {
    use crate::firefight::runtime::{WORLD_DIST_CLIENT, WORLD_SYNC_CLIENT};
    assert!(records(None));
    assert!(records(Some(1)));
    assert!(!records(Some(WORLD_SYNC_CLIENT)));
    assert!(!records(Some(WORLD_DIST_CLIENT)));
}
