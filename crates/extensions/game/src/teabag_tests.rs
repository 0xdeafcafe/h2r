//! The teabag rule on lists of ticks: two crouches over a fresh body, and
//! what is not one; the bots' emote; the record.

use std::collections::HashMap;

use super::*;

/// The victim's unit, and the crouching player's.
const BODY: u32 = 0xe001_0003;
const BAGGER: u32 = 0xe002_0005;
/// Slots: the crouching player and the dead one.
const GIVER: u8 = 0;
const VICTIM: u8 = 1;
/// Where the body lies, and a spot beside it.
const AT: Vec3 = [10.0, 5.0, 2.0];
const BESIDE: Vec3 = [10.8, 5.6, 2.1];

/// A game of two players: `victim` dies at tick 10 at [`AT`]; the giver
/// stands at `stand` and crouches on the ticks `crouched` says.
struct Scene {
    watch: Watch,
    units: HashMap<u32, UnitRead>,
    tick: u32,
    out: Vec<Teabag>,
}

fn unit(position: Vec3, alive: bool, crouch: bool) -> UnitRead {
    UnitRead {
        position,
        alive,
        crouch,
    }
}

impl Scene {
    fn new() -> Scene {
        let mut s = Scene {
            watch: Watch::new(),
            units: HashMap::new(),
            tick: 0,
            out: Vec::new(),
        };
        s.units.insert(BODY, unit(AT, true, false));
        s.units.insert(BAGGER, unit(BESIDE, true, false));
        // Both alive for ten ticks, then the victim dies.
        for _ in 0..10 {
            s.step(true);
        }
        s.units.insert(BODY, unit(AT, false, false));
        s.step(false);
        s
    }

    /// One tick; `victim_listed` keeps the dead unit as the victim's.
    fn step(&mut self, victim_listed: bool) {
        let seen = [
            Seen {
                slot: GIVER,
                unit: self.units.get(&BAGGER).map(|r| (BAGGER, *r)),
            },
            Seen {
                slot: VICTIM,
                unit: victim_listed
                    .then(|| self.units.get(&BODY).map(|r| (BODY, *r)))
                    .flatten(),
            },
        ];
        let units = self.units.clone();
        let got = self
            .watch
            .step(self.tick, &seen, &|u| units.get(&u).copied());
        self.out.extend(got);
        self.tick += 1;
    }

    /// The giver at `at`, crouching or not, for `ticks` ticks.
    fn hold(&mut self, at: Vec3, crouch: bool, ticks: u32) {
        let alive = self.units[&BAGGER].alive;
        self.units.insert(BAGGER, unit(at, alive, crouch));
        for _ in 0..ticks {
            self.step(false);
        }
    }

    /// `n` crouches at `at`: three ticks down, three up.
    fn crouches(&mut self, at: Vec3, n: u32) {
        for _ in 0..n {
            self.hold(at, true, 3);
            self.hold(at, false, 3);
        }
    }
}

#[test]
fn two_crouches_over_a_fresh_body_are_a_teabag() {
    let mut s = Scene::new();
    s.crouches(BESIDE, 2);
    assert_eq!(
        s.out,
        [Teabag {
            tick: s.out[0].tick,
            player: GIVER,
            victim: VICTIM
        }]
    );
    // The second crouch's release made it.
    assert_eq!(s.out[0].tick, 10 + 1 + 9);
    // More crouches over the same body are the same teabag.
    s.crouches(BESIDE, 4);
    assert_eq!(s.out.len(), 1);
    assert_eq!(s.watch.bodies().collect::<Vec<_>>(), [(VICTIM, AT)]);
}

#[test]
fn one_crouch_is_not_a_teabag() {
    let mut s = Scene::new();
    s.crouches(BESIDE, 1);
    // Held down, never let go: no second crouch.
    s.hold(BESIDE, true, 60);
    assert!(s.out.is_empty());
}

#[test]
fn a_crouch_too_far_from_the_body_is_not_over_it() {
    let mut s = Scene::new();
    // 1.6 across the floor: past RADIUS.
    s.crouches([11.6, 5.0, 2.0], 3);
    // Beside it across the floor, a floor up.
    s.crouches([10.5, 5.0, 4.0], 3);
    assert!(s.out.is_empty());
    // The press beside, the release far: no crouch over it.
    let mut s = Scene::new();
    for _ in 0..2 {
        s.hold(BESIDE, true, 3);
        s.hold([14.0, 5.0, 2.0], false, 3);
    }
    assert!(s.out.is_empty());
}

#[test]
fn a_stale_body_and_slow_crouches_are_not_a_teabag() {
    // Over ten seconds after the death.
    let mut s = Scene::new();
    s.hold(BESIDE, false, BODY_TICKS);
    s.crouches(BESIDE, 3);
    assert!(s.out.is_empty());
    assert_eq!(s.watch.bodies().count(), 0, "the body went stale");
    // Two crouches more than three seconds apart.
    let mut s = Scene::new();
    s.crouches(BESIDE, 1);
    s.hold(BESIDE, false, WINDOW_TICKS);
    s.crouches(BESIDE, 1);
    assert!(s.out.is_empty());
}

#[test]
fn crouching_while_dead_and_over_your_own_body_are_not_teabags() {
    // The giver is dead too: its crouch bit counts for nothing.
    let mut s = Scene::new();
    s.units.insert(BAGGER, unit(BESIDE, false, false));
    s.crouches(BESIDE, 3);
    assert!(s.out.is_empty());

    // The victim, back in a new unit, crouches over its own body.
    let mut s = Scene::new();
    const RESPAWNED: u32 = 0xe003_0009;
    s.units.insert(RESPAWNED, unit(BESIDE, true, false));
    for crouch in [false, true, true, false, false, true, true, false, false] {
        s.units.insert(RESPAWNED, unit(BESIDE, true, crouch));
        let seen = [Seen {
            slot: VICTIM,
            unit: Some((RESPAWNED, s.units[&RESPAWNED])),
        }];
        let units = s.units.clone();
        let got = s.watch.step(s.tick, &seen, &|u| units.get(&u).copied());
        s.out.extend(got);
        s.tick += 1;
    }
    assert!(s.out.is_empty());
}

#[test]
fn a_crouch_pressed_before_the_death_is_not_over_the_body() {
    let mut s = Scene {
        watch: Watch::new(),
        units: HashMap::new(),
        tick: 0,
        out: Vec::new(),
    };
    s.units.insert(BODY, unit(AT, true, false));
    s.units.insert(BAGGER, unit(BESIDE, true, false));
    s.step(true);
    // Pressed at tick 1, held through the death at tick 3.
    s.units.insert(BAGGER, unit(BESIDE, true, true));
    s.step(true);
    s.step(true);
    s.units.insert(BODY, unit(AT, false, false));
    s.step(false);
    s.hold(BESIDE, false, 3);
    // One more crouch after the death: one, not two.
    s.crouches(BESIDE, 1);
    assert!(s.out.is_empty());
    s.crouches(BESIDE, 1);
    assert_eq!(s.out.len(), 1, "two after the death");
}

/// A unit that goes without dying (a player who left) leaves no body, and
/// a unit swapped for another while alive (a vehicle seat) is no death.
#[test]
fn only_a_depleted_unit_is_a_death() {
    let mut w = Watch::new();
    let mut units = HashMap::from([(BODY, unit(AT, true, false))]);
    let seen = |u: Option<u32>, units: &HashMap<u32, UnitRead>| {
        [Seen {
            slot: VICTIM,
            unit: u.and_then(|u| Some((u, *units.get(&u)?))),
        }]
    };
    w.step(0, &seen(Some(BODY), &units), &|u| units.get(&u).copied());
    units.remove(&BODY);
    let snapshot = units.clone();
    w.step(1, &seen(None, &units), &|u| snapshot.get(&u).copied());
    assert_eq!(w.bodies().count(), 0);
    units.insert(BODY, unit(AT, true, false));
    units.insert(BAGGER, unit(AT, true, false));
    let snapshot = units.clone();
    w.step(2, &seen(Some(BODY), &units), &|u| snapshot.get(&u).copied());
    w.step(3, &seen(Some(BAGGER), &units), &|u| {
        snapshot.get(&u).copied()
    });
    assert_eq!(w.bodies().count(), 0);
}

/// A bot's emote (bots::emote, docs/game/bots.md §3.5) is a teabag: the
/// emoter hears a betrayal with the body at its feet and plays its burst
/// with the real crouch, three to six presses at four to six ticks each,
/// which the watch counts. Every seed's burst counts, so every bot teabag
/// does.
#[test]
fn a_bots_emote_over_a_body_is_a_teabag() {
    assert!(
        bots::emote::NEAR <= RADIUS && bots::emote::NEAR <= HEIGHT,
        "the emote gate is no wider than the teabag's"
    );
    for seed in 0..50 {
        let mut s = Scene::new();
        let mut e = bots::emote::Emoter::new(1.0, seed);
        e.hear(&[bots::emote::Kill {
            seq: 0,
            tick: s.tick,
            attacker: Some(u32::from(GIVER)),
            victim: u32::from(VICTIM),
            betrayal: true,
            at: Some(AT),
        }]);
        // The bot walks up from four units away.
        let mut me = [AT[0] + 4.0, AT[1], AT[2]];
        for _ in 0..120 {
            let crouch = e.step(me, false, false, s.tick);
            if crouch.is_none() && !e.busy() {
                me[0] = (me[0] - 0.2).max(AT[0] + 0.5);
            }
            s.hold(me, crouch.unwrap_or(false), 1);
        }
        assert_eq!(
            s.out
                .iter()
                .map(|t| (t.player, t.victim))
                .collect::<Vec<_>>(),
            [(GIVER, VICTIM)],
            "seed {seed}"
        );
    }
}

/// A recorder that keeps its calls.
#[derive(Default)]
struct Calls(std::cell::RefCell<Vec<(u32, u32)>>);

impl Recorder for Calls {
    fn teabag(&self, _: &runtime::Guest, player: u32, victim: u32) {
        self.0.borrow_mut().push((player, victim));
    }
}

/// Each teabag is one call, for every player, bots included, on the
/// console that records the game (a guest with no simulation globals reads
/// no world mode: a local game, which records).
#[test]
fn every_teabag_is_recorded_once() {
    let calls = Calls::default();
    let guest = runtime::Guest::new();
    record(
        &calls,
        &guest,
        &[
            Teabag {
                tick: 40,
                player: 0,
                victim: 1,
            },
            Teabag {
                tick: 40,
                player: 3,
                victim: 0,
            },
        ],
    );
    assert_eq!(calls.0.into_inner(), vec![(0, 1), (3, 0)]);
}
