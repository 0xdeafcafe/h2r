//! The chain's rule on lists of ticks: bounces in a row, what ends a
//! chain, the medals at each count, Bounced, and the bots.
//!
//! With the `rules` feature, every scenario's [`Chain`] is the twin of
//! `superbounce_chain_twin.rs`: the Rust chain with the built-in
//! Superbounce script beside it, fed the chain's physics events, which
//! must score and count as the chain does on every tick.

use hud_kit_core::views::superbounce::Medal::*;
use superbounce::Kind;

use super::*;

#[cfg(feature = "rules")]
#[path = "superbounce_chain_twin.rs"]
mod twin;
#[cfg(feature = "rules")]
use twin::Chain;

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
        crouching: false,
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
            "bounced",
            "monster-bounce"
        ]
    );
    // A Monster earns its medal beside the bounce's own, last, so its
    // line is the one shown.
    assert_eq!(bounce_medals_with(2, false), [Superbounce, DoubleBounce]);
    assert_eq!(
        bounce_medals_with(2, true),
        [Superbounce, DoubleBounce, Monster]
    );
    assert_eq!(bounce_medals_with(9, true), [Superbounce, Monster]);
    assert!(bounce_medals_with(0, true).is_empty());
    assert_eq!(highest(&bounce_medals_with(6, true)), Some(Monster));
}

/// A bounce that comes out over `MONSTER_VZ` is a Monster; the measured
/// launches (+12 here, as Ascension's) are not. Both count in the chain.
#[test]
fn a_launch_over_the_monster_speed_is_a_monster_bounce() {
    let monster = || {
        let mut v = vec![
            falling(STRIP),
            tick(STRIP, MONSTER_VZ + 2.0, false),
            tick(STRIP, MONSTER_VZ + 1.8, false),
        ];
        v.extend((0..10).map(|i| tick(STRIP, MONSTER_VZ - i as f32, true)));
        v
    };
    let mut c = Chain::default();
    let mut ticks = bounce_and_fly();
    ticks.extend(monster());
    let events = run(&mut c, &ticks);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(events[0], Event::Bounce { monster: false, strips: 1, .. }));
    assert!(matches!(events[1], Event::Bounce { monster: true, chain: 2, .. }));
    assert_eq!(events[1].medals(), [Superbounce, DoubleBounce, Monster]);
    assert_eq!(events[1].strips(), Some(1));
    // Just at the speed is not over it.
    let mut c = Chain::default();
    let at = [
        falling(STRIP),
        tick(STRIP, MONSTER_VZ, false),
        tick(STRIP, MONSTER_VZ - 0.2, false),
        tick(STRIP, MONSTER_VZ - 0.4, true),
        tick(STRIP, MONSTER_VZ - 0.6, true),
    ];
    let got = run(&mut c, &at);
    assert!(matches!(got[..], [Event::Bounce { monster: false, .. }]), "{got:?}");
    // The award carries it; the medal test files one on asking.
    let mut m = Medals::new();
    let a = m.test(0, Some(0), 10, 3, false, true);
    assert!(a.monster());
    assert_eq!(a.medals, [Superbounce, TripleBounce, Monster]);
    assert!(!m.test(0, Some(0), 11, 4, false, false).monster());
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

/// Each bounce names its strip by the strip's geometry, the id a rules
/// script's `on bounce` hears: the same id for the same strip, whatever
/// else is in the list, and never `none`.
#[test]
fn a_bounce_names_its_strip_by_a_stable_id() {
    let strip = spots()[0];
    let id = spot_id(&strip);
    assert_eq!(id & 0x8000_0000, 0);
    let mut c = Chain::default();
    let events = run(&mut c, &bounces(2));
    assert!(events.iter().all(|e| e.spot() == Some(id)), "{events:?}");
    // Another spot ahead of it in the list leaves the strip's id alone.
    let crouch = Spot {
        kind: Kind::Crouch,
        edge: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        ..Spot::default()
    };
    let mut c = Chain::default();
    let list = [crouch, strip];
    let got: Vec<Event> = bounces(1)
        .iter()
        .filter_map(|t| c.step(Some(t), &list))
        .collect();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].spot(), Some(id));
    assert_ne!(spot_id(&crouch), id);
    assert_eq!(Event::Bounced { chain: 2 }.spot(), None);
    // The awards carry it; a medal test's has none.
    let mut m = Medals::new();
    let seen: Vec<Vec<Seen>> = bounces(1)
        .into_iter()
        .map(|t| vec![Seen { slot: 3, user: None, tick: Some(t) }])
        .collect();
    let awards: Vec<Award> = seen
        .iter()
        .enumerate()
        .flat_map(|(i, s)| m.step(i as u32, s, &spots()))
        .collect();
    assert_eq!(awards.len(), 1);
    assert_eq!((awards[0].slot, awards[0].spot), (3, Some(id)));
    assert_eq!(awards[0].strips, Some(1));
    let t = m.test(3, None, 99, 2, false, false);
    assert_eq!((t.spot, t.strips), (None, None));
}

#[test]
fn a_landing_off_the_strip_ends_the_chain() {
    let mut c = Chain::default();
    let mut ticks = bounces(2);
    // Down on the floor beside the strip: a touchdown, not a bounce.
    ticks.push(falling(BESIDE));
    ticks.extend((0..=LAUNCH_TICKS).map(|_| tick(BESIDE, 0.0, false)));
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
    ticks.extend((0..LAUNCH_TICKS).map(|_| tick(STRIP, 0.0, false)));
    ticks.extend(bounces(1));
    let counts: Vec<u32> = run(&mut c, &ticks).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1, 1]);
}

#[test]
fn the_airborne_bit_clear_from_the_third_tick_ends_it() {
    // Clear on the touchdown tick and the two after: the launch's, and
    // the bounce is not judged yet.
    let mut c = Chain::default();
    let ticks = vec![
        falling(STRIP),
        tick(STRIP, 12.0, false),
        tick(STRIP, 11.8, false),
        tick(STRIP, 11.5, false),
    ];
    assert!(run(&mut c, &ticks).is_empty(), "the window is open");
    // The third tick after, airborne: the bounce is filed and live.
    let mut flying = c.clone();
    let got = run(&mut flying, &[tick(STRIP, 11.2, true)]);
    assert_eq!(got.len(), 1);
    assert!(flying.live(), "airborne on the third tick");
    // The third, clear and rising (no touchdown): filed, and the chain
    // ends there.
    let got = run(&mut c, &[tick(STRIP, 11.2, false)]);
    assert_eq!(got.iter().map(|e| e.chain()).collect::<Vec<_>>(), [1]);
    assert!(!c.live());
    let counts: Vec<u32> = run(&mut c, &bounces(1)).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1]);
}

/// The push can show a tick or two after the contact: a touchdown that
/// reads about 0 and launches on the next tick or the one after is a
/// bounce, filed when the window closes.
#[test]
fn a_launch_a_tick_or_two_after_touchdown_is_a_bounce() {
    for late in 1..=2usize {
        let mut c = Chain::default();
        let mut vz = [0.0f32; 4];
        vz[late] = 11.0;
        let mut ticks = vec![falling(STRIP)];
        ticks.extend(vz.iter().map(|v| tick(STRIP, *v, true)));
        ticks.extend((0..8).map(|i| tick(STRIP, 10.0 - i as f32, true)));
        let events = run(&mut c, &ticks);
        assert_eq!(events.len(), 1, "late {late}: {events:?}");
        assert_eq!(events[0].chain(), 1);
        assert_eq!(events[0].spot(), Some(spot_id(&spots()[0])));
    }
    // A launch of 4, under the old 8 a tick: still a bounce.
    let mut c = Chain::default();
    let mut ticks = vec![falling(STRIP)];
    ticks.extend([0.0, 4.0, 3.9, 3.8].map(|v| tick(STRIP, v, true)));
    assert_eq!(run(&mut c, &ticks).len(), 1);
}

/// A primed bounce off a seam no marked strip is near is a bounce all the
/// same, with no strip to name.
#[test]
fn a_bounce_off_an_unmarked_seam_earns_its_medal_with_no_strip() {
    const SEAM: Vec3 = [40.0, 0.0, 2.0];
    let mut c = Chain::default();
    let mut ticks = vec![falling(SEAM), tick(SEAM, 0.0, true)];
    ticks.extend((0..10).map(|i| tick(SEAM, 9.0 - i as f32, true)));
    let events = run(&mut c, &ticks);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        events[0],
        Event::Bounce {
            chain: 1,
            spot: None,
            strips: 0,
            ..
        }
    ));
    assert_eq!(events[0].medals(), [Superbounce]);
    // A landing there that does not launch is nothing, and ends no chain
    // it was not in.
    let mut c = Chain::default();
    let mut ticks = vec![falling(SEAM)];
    ticks.extend((0..6).map(|_| tick(SEAM, 0.0, false)));
    assert!(run(&mut c, &ticks).is_empty());
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
    let mut ticks = vec![fall];
    ticks.extend([12.0, 11.8, 11.5, 11.2].map(|v| tick(STRIP, v, false)));
    let events = run(&mut c, &ticks);
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
    let a = m.test(0, Some(0), 10, 5, false, false);
    assert_eq!(
        (a.chain, a.medals.clone()),
        (5, vec![Superbounce, ChainBounce])
    );
    assert_eq!(a.user, Some(0));
    assert!(m.chain(0).unwrap().live());
    assert_eq!(m.records()[&0].chain, 5);
    let b = m.test(0, Some(0), 11, 0, true, false);
    assert_eq!(b.medals, [Bounced]);
    assert!(!m.chain(0).unwrap().live());
    for i in 0..10 {
        m.test(1, None, 20 + i, 1, false, false);
    }
    assert_eq!(m.recent().count(), RECENT);
    assert_eq!(m.recent().last().unwrap().tick, 29);
}

/// A recorder that keeps its calls.
#[derive(Default)]
struct Calls(std::cell::RefCell<Vec<(&'static str, u32, u32)>>);

impl Recorder for Calls {
    fn superbounce(&self, _: &runtime::Guest, player: u32, chain: u32, monster: bool) {
        let kind = if monster { "monster" } else { "superbounce" };
        self.0.borrow_mut().push((kind, player, chain));
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
        spot: None,
        strips: None,
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
        award_of(4, 2, bounce_medals_with(2, true)),
        award_of(2, 4, vec![Bounced]),
        award_of(5, 0, vec![Bounced]),
    ];
    record(&calls, &guest, &awards);
    assert_eq!(
        calls.0.into_inner(),
        vec![
            ("superbounce", 0, 1),
            ("superbounce", 3, 6),
            ("monster", 4, 2),
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

// ---- a fall through the strip's floor ----

/// A primed fall onto the strip at -4.3 units/s (the video's): the origin
/// over its edge, 0.18 under the edge on the last tick, short of `SINK`.
fn fall_onto_strip() -> Vec<PlayerTick> {
    [-2.62, -2.76, -2.9, -3.04, -3.18]
        .map(|z| tick([10.0, 0.0, z], -4.3, true))
        .to_vec()
}

/// Under the map, 0.4 under the edge: the first tick is the crossing.
/// Not airborne, no fall speed and no pair (the body rests or slides on
/// something under the world), moving sideways.
fn under_map(n: usize) -> Vec<PlayerTick> {
    (0..n)
        .map(|i| PlayerTick {
            unit: UNIT,
            position: [10.0 + 0.06 * i as f32, 0.0, -3.4],
            velocity: [1.8, 0.0, 0.0],
            airborne: false,
            crouching: false,
            word: None,
        })
        .collect()
}

/// The video's case: a primed fall onto the strip goes through its floor,
/// a second under the map with nothing read of a fall, then the Guardians
/// kill: one Bounced, no chain.
#[test]
fn a_primed_fall_through_the_strips_floor_is_bounced_when_the_unit_goes() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(under_map(31));
    assert!(run(&mut c, &ticks).is_empty());
    assert!(c.armed(), "went through");
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 0 }));
    assert!(!c.armed());
    // One death, one Bounced.
    assert_eq!(c.step(None, &spots), None);
}

/// In a live chain of two, the Bounced carries the chain.
#[test]
fn a_fall_through_in_a_chain_is_bounced_with_its_count() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks = bounces(2);
    ticks.extend(fall_onto_strip());
    ticks.extend(under_map(31));
    let counts: Vec<u32> = run(&mut c, &ticks).iter().map(|e| e.chain()).collect();
    assert_eq!(counts, [1, 2]);
    assert!(!c.live(), "the fall through ends the chain");
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 2 }));
}

/// The touchdown tick first (the fall stops for a tick and a touchdown
/// opens), then the origin sinks through: the touchdown is the seam hit,
/// dropped, and the death is one Bounced.
#[test]
fn a_touchdown_that_then_sinks_through_is_bounced() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks = vec![
        tick([10.0, 0.0, -2.76], -4.3, true),
        tick([10.0, 0.0, -2.9], -4.3, true),
        tick([10.0, 0.0, -3.05], 0.0, false),
    ];
    ticks.extend(under_map(31));
    assert!(run(&mut c, &ticks).is_empty());
    assert!(c.armed());
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 0 }));
}

/// A real bounce dips 0.2 under the edge and launches: a bounce, never a
/// fall through, and a death long after on the floor earns nothing.
#[test]
fn a_bounces_dip_is_no_fall_through() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks = vec![
        tick([10.0, 0.0, -2.6], -6.0, true),
        tick([10.0, 0.0, -2.8], -6.0, true),
        tick([10.0, 0.0, -3.2], 9.0, true),
        tick([10.0, 0.0, -2.9], 8.8, true),
        tick([10.0, 0.0, -2.6], 8.6, true),
        tick([10.0, 0.0, -2.3], 8.4, true),
    ];
    let events = run(&mut c, &ticks);
    assert!(matches!(events[..], [Event::Bounce { chain: 1, .. }]), "{events:?}");
    assert!(!c.armed());
    ticks = (0..100).map(|_| tick([10.0, 0.0, -2.3], 0.0, false)).collect();
    assert!(run(&mut c, &ticks).is_empty());
    assert_eq!(c.step(None, &spots), None);
}

/// Armed for `BOUNCED_TICKS`: alive past it, the player dropped to a floor
/// below and lived; a bounce after it, or a teleport, disarms it.
#[test]
fn a_fall_through_survived_or_bounced_out_of_is_no_bounced() {
    let spots = spots();
    // Alive 299 ticks after the crossing: still armed.
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(under_map(1 + 299));
    run(&mut c, &ticks);
    assert!(c.armed());
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 0 }));
    // Alive 301: disarmed, and the death is a plain one.
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(under_map(1 + 301));
    run(&mut c, &ticks);
    assert!(!c.armed());
    assert_eq!(c.step(None, &spots), None);
    // Through, then down to a floor below and a primed bounce off it.
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(under_map(5));
    let below = |z: f32, vz: f32| tick([10.3, 0.0, z], vz, true);
    ticks.extend([-3.6, -3.8, -4.0, -4.2, -4.4, -4.6, -4.8].map(|z| below(z, -6.0)));
    ticks.extend([(-5.0, 9.0), (-4.7, 8.8), (-4.4, 8.6), (-4.1, 8.4)].map(|(z, vz)| below(z, vz)));
    let events = run(&mut c, &ticks);
    assert!(
        matches!(events[..], [Event::Bounce { chain: 1, spot: None, .. }]),
        "{events:?}"
    );
    assert!(!c.armed(), "the bounce disarms");
    assert_eq!(c.step(None, &spots), None);
    // Through, then moved: a teleport disarms.
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(under_map(3));
    ticks.push(tick([20.0, 0.0, 0.0], 0.0, false));
    run(&mut c, &ticks);
    assert!(!c.armed());
    assert_eq!(c.step(None, &spots), None);
}

/// A drop past the strip 0.6 to the side of its edge (a ledge beside it)
/// is no fall through.
#[test]
fn a_drop_beside_the_strip_is_no_fall_through() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks: Vec<PlayerTick> = [-2.62, -2.76, -2.9, -3.04, -3.18, -3.4]
        .map(|z| tick([10.6, 0.0, z], -4.3, true))
        .to_vec();
    assert!(run(&mut c, &ticks).is_empty());
    assert!(!c.armed());
    // Down on the ledge's floor below, standing, then a death.
    ticks = (0..10).map(|_| tick([10.6, 0.0, -3.5], 0.0, false)).collect();
    assert!(run(&mut c, &ticks).is_empty());
    assert_eq!(c.step(None, &spots), None);
}

/// Neither primed nor in a chain: going through the floor is no attempt.
#[test]
fn an_unprimed_fall_through_is_no_bounced() {
    let spots = spots();
    let mut c = Chain::default();
    let mut ticks: Vec<PlayerTick> = fall_onto_strip()
        .into_iter()
        .map(|t| PlayerTick { word: Some(0), ..t })
        .collect();
    ticks.extend(under_map(31));
    assert!(run(&mut c, &ticks).is_empty());
    assert!(!c.armed());
    assert_eq!(c.step(None, &spots), None);
}

/// Through the floor and still falling primed when the unit goes: both
/// rules hold, and the death is one Bounced, with the chain it was in.
#[test]
fn both_rules_on_one_death_give_one_bounced() {
    let spots = spots();
    let falling_under = [
        tick([10.0, 0.0, -3.4], -4.3, true),
        tick([10.0, 0.0, -3.55], -4.3, true),
    ];
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(falling_under);
    assert!(run(&mut c, &ticks).is_empty());
    assert!(c.armed());
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 0 }));
    assert_eq!(c.step(None, &spots), None);
    // In a chain of two.
    let mut c = Chain::default();
    let mut ticks = bounces(2);
    ticks.extend(fall_onto_strip());
    ticks.extend(falling_under);
    assert_eq!(run(&mut c, &ticks).len(), 2);
    assert_eq!(c.step(None, &spots), Some(Event::Bounced { chain: 2 }));
    // Gone and back as a new unit on one tick: one Bounced too.
    let mut c = Chain::default();
    let mut ticks = fall_onto_strip();
    ticks.extend(falling_under);
    ticks.push(PlayerTick {
        unit: UNIT + 0x10000,
        ..tick(FLOOR, 0.0, false)
    });
    assert_eq!(run(&mut c, &ticks), [Event::Bounced { chain: 0 }]);
    assert!(!c.armed());
}

// ---- the physics a script hears ----

/// A bounce's ticks give its touchdown, then its launch when the window
/// closes; a landing off the strip after it gives a touchdown and the end
/// of the run.
#[test]
fn a_step_says_what_it_saw_of_the_physics() {
    let spots = spots();
    let mut c = Chain::default();
    let mut seen = Vec::new();
    let mut ticks = bounce_and_fly();
    ticks.push(falling(BESIDE));
    ticks.extend((0..=LAUNCH_TICKS).map(|_| tick(BESIDE, 0.0, false)));
    for t in &ticks {
        c.step(Some(t), &spots);
        seen.extend(c.physics().iter().copied());
    }
    let id = spot_id(&spots[0]);
    assert_eq!(
        seen,
        [
            Physics::Touchdown {
                down: 6.0,
                up: 12.0
            },
            Physics::Launch {
                spot: Some(id),
                speed: 12.0
            },
            Physics::Touchdown {
                down: 6.0,
                up: 0.0
            },
            Physics::Grounded,
        ]
    );
    // Each step's list is its own.
    c.step(Some(&tick(BESIDE, 0.0, false)), &spots);
    assert!(c.physics().is_empty());
}

/// A launch whose run ends on the same step (the third tick clear) gives
/// the launch, then the end; a death in a run gives the end.
#[test]
fn the_end_of_a_run_follows_its_last_launch() {
    let spots = spots();
    let mut c = Chain::default();
    for t in [
        falling(STRIP),
        tick(STRIP, 12.0, false),
        tick(STRIP, 11.8, false),
        tick(STRIP, 11.5, false),
    ] {
        c.step(Some(&t), &spots);
    }
    c.step(Some(&tick(STRIP, 11.2, false)), &spots);
    assert!(matches!(
        c.physics(),
        [Physics::Launch { .. }, Physics::Grounded]
    ));
    let mut c = Chain::default();
    for t in bounces(2) {
        c.step(Some(&t), &spots);
    }
    c.step(Some(&falling(BELOW)), &spots);
    assert!(c.physics().is_empty());
    c.step(None, &spots);
    assert_eq!(c.physics(), [Physics::Grounded]);
    // A fall through a strip's floor in a run ends it too.
    let mut c = Chain::default();
    let mut ticks = bounces(1);
    ticks.extend(fall_onto_strip());
    ticks.extend(under_map(2));
    let mut ends = 0;
    for t in &ticks {
        c.step(Some(t), &spots);
        ends += c.physics().iter().filter(|p| **p == Physics::Grounded).count();
    }
    assert_eq!(ends, 1);
}

/// The medals' step gives every player's physics, slot by slot.
#[test]
fn every_players_physics_is_heard_by_slot() {
    let spots = spots();
    let mut m = Medals::new();
    let mut heard = Vec::new();
    for (i, t) in bounce_and_fly().into_iter().enumerate() {
        let seen = [
            Seen {
                slot: 5,
                user: None,
                tick: Some(PlayerTick {
                    unit: UNIT + 5,
                    ..t
                }),
            },
            Seen {
                slot: 0,
                user: Some(0),
                tick: Some(t),
            },
        ];
        m.step(i as u32, &seen, &spots);
        heard.extend(m.physics().iter().map(|(slot, p)| (*slot, std::mem::discriminant(p))));
    }
    let touchdown = std::mem::discriminant(&Physics::Touchdown { down: 0.0, up: 0.0 });
    let launch = std::mem::discriminant(&Physics::Launch {
        spot: None,
        speed: 0.0,
    });
    assert_eq!(
        heard,
        [(0, touchdown), (5, touchdown), (0, launch), (5, launch)]
    );
}
