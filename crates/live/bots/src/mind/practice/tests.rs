use super::*;
use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};
use crate::glitches::Bounce;
use crate::mind::{Doing, MELEE, Skill};
use crate::nav::{NavMesh, Poly};
use crate::spots::{Crouch, Known, Site};
use superbounce::Status;

fn map() -> MapData {
    let floor = Poly::new(vec![
        [-40.0, -40.0, 0.0],
        [40.0, -40.0, 0.0],
        [40.0, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ]);
    MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    }
}

fn site(stand: Vec3, status: Status) -> Site {
    Site {
        bounce: Bounce {
            map: "",
            name: "test",
            stand,
            spread: 0.03,
            drift: [1.0, 0.0],
            source: "scan",
            takeoff: None,
            landing: None,
        },
        launch: 12.3,
        fall: 0.0,
        status,
        ..Site::default()
    }
}

fn known() -> Known {
    Known {
        sites: vec![site([10.0, 0.0, 0.0], Status::Likely)],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    }
}

fn armed() -> Arms {
    Arms {
        held: Some(Gun {
            def: 0xa001_0001,
            ammo: Ammo::Rounds {
                loaded: 36,
                capacity: 36,
                reserve: 108,
                reserve_max: 108,
            },
            profile: Profile {
                trigger: Trigger::Automatic,
                zoom: 1,
                pellets: 1,
                shot_speed: 400.0,
                ..Profile::default()
            },
        }),
        other: None,
        grenades: [2, 0],
        ..Arms::default()
    }
}

fn scene(me: Me, tick: u32) -> Perception {
    Perception {
        me: Some(me),
        // An enemy right in front, in the open.
        others: vec![Seen {
            id: 7,
            position: [6.0, 0.0, 0.0],
            team: 1,
        }],
        teams: false,
        tick,
        ..Perception::default()
    }
}

fn bot(shield: f32) -> Me {
    Me {
        position: [0.0; 3],
        aim: [1.0, 0.0, 0.0],
        team: 0,
        shield,
        health: 1.0,
        arms: Some(armed()),
        ..Me::default()
    }
}

fn attacking(pad: &HostPadState) -> bool {
    pad.right_trigger > 0.0 || pad.left_trigger > 0.0 || pad.buttons.contains(MELEE)
}

#[test]
fn a_practising_bot_never_fights_even_shot_at_and_goes_to_bounce() {
    let m = map();
    // The control: the same bot, not practising, shoots the enemy.
    let mut fighter = Mind::new(Skill::Legendary, 31);
    fighter.known = known();
    let shoots = (0..60).any(|t| attacking(&fighter.step(&scene(bot(1.0), t), None)));
    assert!(shoots, "the control fights");

    let mut mind = Mind::new(Skill::Legendary, 32);
    mind.known = known();
    mind.set_practice(true);
    assert!(mind.practice_view().is_some());
    let mut routed = false;
    for t in 0..300 {
        // Shot at every few ticks: the shields keep dropping.
        let shield = 1.0 - (t % 50) as f32 / 60.0;
        let pad = mind.step(&scene(bot(shield), t), Some(&m));
        assert!(!attacking(&pad), "tick {t}: {pad:?}");
        assert!(!matches!(mind.doing, Doing::Fight(_)), "tick {t}");
        let v = mind.practice_view().unwrap();
        if v.state == "route" {
            routed = true;
            assert_eq!(v.site, Some([10.0, 0.0, 0.0]));
            assert_eq!(mind.order_view().state, "running");
        }
    }
    assert!(routed, "it went to practise a bounce");
}

#[test]
fn practice_off_drops_its_route_and_its_view() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 33);
    mind.known = known();
    mind.set_practice(true);
    let mut t = 0;
    while mind.practice_view().unwrap().state != "route" && t < 200 {
        mind.step(&scene(bot(1.0), t), Some(&m));
        t += 1;
    }
    assert_eq!(mind.order_view().state, "running");
    mind.set_practice(false);
    let v = mind.order_view();
    assert_eq!((v.state, v.error), ("failed", Some("practice_off")));
    assert!(v.practice.is_none());
    assert!(mind.practice_view().is_none());
}

#[test]
fn proven_sites_weigh_more_and_a_claimed_or_just_tried_one_less() {
    let at = [0.0; 3];
    let likely = site([10.0, 0.0, 0.0], Status::Likely);
    let proven = site([10.0, 5.0, 0.0], Status::Proven);
    assert!(weight(&proven, at, false, None) > weight(&likely, at, false, None));
    assert!(weight(&likely, at, true, None) < weight(&likely, at, false, None) - 2.0);
    assert!(
        weight(&likely, at, false, Some(likely.bounce.stand)) < weight(&likely, at, false, None)
    );
    // Further is a little worse.
    let far = site([70.0, 0.0, 0.0], Status::Likely);
    assert!(weight(&far, at, false, None) < weight(&likely, at, false, None));
}

#[test]
fn bots_spread_over_the_sites_by_their_claims() {
    let a = site([20.0, 0.0, 0.0], Status::Likely);
    let b = site([-20.0, 0.0, 0.0], Status::Likely);
    let mut mind = Mind::new(Skill::Normal, 0x5_9101);
    mind.known = Known {
        sites: vec![a, b],
        crouches: vec![Crouch::at([0.0, 3.0, 0.0])],
    };
    // A board of the test's own: the process's is the other tests' too.
    static BOARD: Board = Mutex::new(Vec::new());
    mind.practice.board = &BOARD;
    // Another bot practises at a.
    let tick = 9_000;
    claim(&BOARD, 0x5_9102, Some(a.bounce.stand), tick);
    assert!(claimed_by_other(&BOARD, 0x5_9101, a.bounce.stand, tick));
    let m = map();
    for _ in 0..8 {
        let Pick::Site(pick, _) = mind.pick_practice([0.0; 3], tick, Some(&m)) else {
            panic!("a pick");
        };
        assert_eq!(pick.bounce.stand, b.bounce.stand);
    }
    // The claim lapses once it is not renewed.
    assert!(!claimed_by_other(
        &BOARD,
        0x5_9101,
        a.bounce.stand,
        tick + CLAIM_TICKS
    ));
    // Its own claim does not keep it off.
    claim(&BOARD, 0x5_9101, Some(b.bounce.stand), tick);
    assert!(!claimed_by_other(&BOARD, 0x5_9101, b.bounce.stand, tick));
}

#[test]
fn with_no_crouch_spot_to_prime_at_it_waits() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 34);
    mind.known = Known {
        sites: known().sites,
        crouches: Vec::new(),
    };
    mind.set_practice(true);
    for t in 0..120 {
        let pad = mind.step(&scene(bot(1.0), t), Some(&m));
        assert!(!attacking(&pad));
    }
    assert_eq!(mind.practice_view().unwrap().state, "waiting");
}

#[test]
fn the_tally_counts_tries_bounces_and_why_by_site() {
    let mut p = Practice::new(1);
    p.record([1.0, 0.0, 0.0], true, None);
    p.record([1.0, 0.0, 0.0], false, Some("too_slow"));
    p.record([1.0, 0.0, 0.0], false, Some("too_slow"));
    p.record([5.0, 0.0, 0.0], false, Some("prime_failed"));
    let v = p.view();
    assert_eq!((v.attempts, v.bounces), (4, 1));
    assert_eq!(v.spots.len(), 2);
    assert_eq!(v.spots[0].attempts, 3);
    assert_eq!(v.spots[0].why.get("too_slow"), Some(&2));
    assert_eq!(v.spots[1].why.get("prime_failed"), Some(&1));
}

#[test]
fn the_practice_reports_each_route_it_starts_and_how_each_try_ended() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 35);
    mind.known = known();
    mind.set_practice(true);
    let mut t = 0;
    while mind.practice_view().unwrap().state != "route" && t < 200 {
        mind.step(&scene(bot(1.0), t), Some(&m));
        t += 1;
    }
    let started = mind.take_practice_events();
    assert!(
        matches!(started.as_slice(), [PracticeEvent::Started { site, .. }] if *site == [10.0, 0.0, 0.0]),
        "{started:?}"
    );
    assert!(mind.take_practice_events().is_empty(), "taken once");
    // A failed route is a try with its reason.
    mind.cancel_order("timeout");
    mind.step(&scene(bot(1.0), t), Some(&m));
    let ended = mind.take_practice_events();
    assert!(
        ended.iter().any(|e| matches!(
            e,
            PracticeEvent::Tried {
                bounced: false,
                why: Some("timeout"),
                ..
            }
        )),
        "{ended:?}"
    );
    // At most EVENTS wait for the host; the oldest go first.
    let mut p = Practice::new(2);
    for i in 0..EVENTS + 5 {
        p.event(PracticeEvent::Started {
            site: [0.0; 3],
            tick: i as u32,
        });
    }
    assert_eq!(p.events.len(), EVENTS);
    assert!(matches!(
        p.events[0],
        PracticeEvent::Started { tick: 5, .. }
    ));
}

// ---- sites a route reaches, and sites it does not ----

/// A floor from x -40 to `edge` (y -40 to 40, z 0), and an island floor
/// from x 50 to 60 no link reaches.
fn edge_map(edge: f32) -> MapData {
    let floor = Poly::new(vec![
        [-40.0, -40.0, 0.0],
        [edge, -40.0, 0.0],
        [edge, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ]);
    let island = Poly::new(vec![
        [50.0, -5.0, 0.0],
        [60.0, -5.0, 0.0],
        [60.0, 5.0, 0.0],
        [50.0, 5.0, 0.0],
    ]);
    // The collision's floor goes on 2 past the mesh's edge.
    let collision = [
        [-40.0, -40.0, 0.0],
        [edge + 2.0, -40.0, 0.0],
        [edge + 2.0, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ];
    MapData {
        nav: NavMesh::new(vec![floor, island], vec![]),
        walls: crate::los::Walls::from_polygons([&collision[..]]),
        ..MapData::default()
    }
}

/// Steps a practising mind standing at the origin until `until` says
/// stop or `ticks` pass, returning every practice event.
fn practise(
    mind: &mut Mind,
    m: &MapData,
    ticks: u32,
    until: impl Fn(&[PracticeEvent]) -> bool,
) -> Vec<PracticeEvent> {
    let mut events = Vec::new();
    for t in 0..ticks {
        mind.step(&scene(bot(1.0), t), Some(m));
        events.extend(mind.take_practice_events());
        if until(&events) {
            break;
        }
    }
    events
}

#[test]
fn a_site_whose_approach_is_just_off_the_mesh_is_picked_and_routed() {
    // The floor ends at x 10; the site stands at 10.2 coming in toward
    // -x, so its approach (0.8 behind it) is 1.0 off the mesh.
    let m = edge_map(10.0);
    let mut s = site([10.2, 0.0, 0.0], Status::Likely);
    s.bounce.drift = [-1.0, 0.0];
    assert!(s.bounce.from()[0] > 10.5);
    let mut mind = Mind::new(Skill::Normal, 36);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    };
    assert!(matches!(
        mind.pick_practice([0.0; 3], 0, Some(&m)),
        Pick::Site(p, Some(c)) if p == s && c.stand == [3.0, 0.0, 0.0]
    ));
    mind.set_practice(true);
    let started =
        |e: &[PracticeEvent]| e.iter().any(|e| matches!(e, PracticeEvent::Started { .. }));
    let events = practise(&mut mind, &m, 200, started);
    assert!(
        matches!(events.as_slice(), [PracticeEvent::Started { site, .. }] if *site == s.bounce.stand),
        "started, and not failed: {events:?}"
    );
    let v = mind.order_view();
    assert_eq!(v.state, "running");
    assert_eq!(v.steps, ["walk_crouched", "prime", "walk", "bounce", "fly"]);
}

#[test]
fn a_site_far_off_the_mesh_is_skipped_not_tried() {
    // The approach is 2.4 off the floor's edge: the mesh snaps to it, but
    // no walk gets there.
    let m = edge_map(10.0);
    let mut s = site([11.6, 0.0, 0.0], Status::Likely);
    s.bounce.drift = [-1.0, 0.0];
    let mut mind = Mind::new(Skill::Normal, 37);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    };
    mind.set_practice(true);
    let events = practise(&mut mind, &m, 600, |_| false);
    assert!(
        events.is_empty(),
        "never started, never tallied: {events:?}"
    );
    let v = mind.practice_view().unwrap();
    assert_eq!((v.state, v.attempts), ("waiting", 0));
    // Found out once, and kept.
    assert!(matches!(
        mind.practice.pairs.get(&stand_key(s.bounce.stand)),
        Some(Ok(Pairing { crouch: None, .. }))
    ));
}

#[test]
fn a_site_with_no_reachable_crouch_spot_is_not_picked() {
    let m = edge_map(40.0);
    // Proven, so weighed first, but the only crouch spot within
    // PAIR_REACH of it is on the island no walk reaches.
    let lonely = site([30.0, 0.0, 0.0], Status::Proven);
    let paired = site([-10.0, 0.0, 0.0], Status::Likely);
    let mut mind = Mind::new(Skill::Normal, 38);
    mind.known = Known {
        sites: vec![lonely, paired],
        crouches: vec![Crouch::at([52.0, 0.0, 0.0]), Crouch::at([-38.0, 0.0, 0.0])],
    };
    for tick in 0..8 {
        let Pick::Site(pick, crouch) = mind.pick_practice([0.0; 3], tick, Some(&m)) else {
            panic!("a pick");
        };
        assert_eq!(pick, paired, "the proven site has nowhere to prime");
        assert_eq!(crouch.map(|c| c.stand), Some([-38.0, 0.0, 0.0]));
    }
    // With only the lonely site there is nothing to practise.
    mind.known.sites = vec![lonely];
    assert_eq!(mind.pick_practice([0.0; 3], 9, Some(&m)), Pick::Nothing);
}

// ---- giving up what keeps failing ----

#[test]
fn a_crouch_spot_that_keeps_failing_is_given_up_and_its_site_paired_again() {
    let m = map();
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let near = Crouch::at([9.0, 1.0, 0.0]);
    let other = Crouch::at([4.0, 0.0, 0.0]);
    let mut mind = Mind::new(Skill::Normal, 39);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![near, other],
    };
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    for n in 0..CROUCH_STRIKES {
        let Pick::Site(_, crouch) = mind.pick_practice([0.0; 3], n, Some(&m)) else {
            panic!("a pick");
        };
        assert_eq!(crouch, Some(near));
        mind.practice.route_crouch = crouch;
        mind.practice_strike(s, &steps, 1, Some("prime_failed"));
    }
    let Pick::Site(_, crouch) = mind.pick_practice([0.0; 3], 9, Some(&m)) else {
        panic!("a pick");
    };
    assert_eq!(crouch, Some(other), "paired again without the failing one");
    // A landing that did not bounce counts against neither.
    mind.practice.route_crouch = crouch;
    for _ in 0..5 {
        mind.practice_strike(s, &steps, 3, Some("not_primed"));
    }
    assert!(matches!(
        mind.pick_practice([0.0; 3], 10, Some(&m)),
        Pick::Site(..)
    ));
}

#[test]
fn a_site_whose_approach_walk_keeps_failing_is_given_up_for_a_while() {
    let m = map();
    // Proven, so picked first while it is there.
    let s = site([10.0, 0.0, 0.0], Status::Proven);
    let other = site([-10.0, 0.0, 0.0], Status::Likely);
    let mut mind = Mind::new(Skill::Normal, 40);
    mind.known = known();
    mind.known.sites = vec![s, other];
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    for n in 0..SITE_STRIKES {
        assert!(matches!(mind.pick_practice([0.0; 3], n, Some(&m)), Pick::Site(p, _) if p == s));
        mind.practice_strike(s, &steps, 2, Some("path_lost_stuck_approach"));
    }
    // Given up: the other site.
    assert!(matches!(mind.pick_practice([0.0; 3], 9, Some(&m)), Pick::Site(p, _) if p == other));
    // Back after a while.
    let later = GIVE_UP_TICKS + 10;
    assert!(matches!(mind.pick_practice([0.0; 3], later, Some(&m)), Pick::Site(p, _) if p == s));
}

#[test]
fn strikes_far_apart_never_give_anything_up() {
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let c = Crouch::at([9.0, 1.0, 0.0]);
    let mut mind = Mind::new(Skill::Normal, 42);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![c],
    };
    mind.practice.route_crouch = Some(c);
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    for n in 0..3 * SITE_STRIKES {
        mind.tick = Some(n * (STRIKE_TICKS + 1));
        mind.practice_strike(s, &steps, 2, Some("path_lost_stuck_approach"));
        mind.practice_strike(s, &steps, 1, Some("prime_failed"));
    }
    assert!(mind.practice.given_up.is_empty() && mind.practice.gone.is_empty());
}

#[test]
fn a_proven_crouch_spot_is_never_given_up() {
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let proven = Crouch {
        status: Status::Proven,
        ..Crouch::at([9.0, 1.0, 0.0])
    };
    let mut mind = Mind::new(Skill::Normal, 43);
    mind.practice.route_crouch = Some(proven);
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    for _ in 0..3 * CROUCH_STRIKES {
        mind.practice_strike(s, &steps, 1, Some("prime_failed"));
    }
    assert!(mind.practice.gone.is_empty());
    // A likely one is.
    mind.practice.route_crouch = Some(Crouch::at([4.0, 0.0, 0.0]));
    for _ in 0..CROUCH_STRIKES {
        mind.practice_strike(s, &steps, 1, Some("prime_failed"));
    }
    assert_eq!(mind.practice.gone.len(), 1);
}

#[test]
fn with_everything_given_up_it_all_comes_back_rather_than_nothing() {
    let m = map();
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let c = Crouch::at([4.0, 0.0, 0.0]);
    let mut mind = Mind::new(Skill::Normal, 44);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![c],
    };
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    // Pick until a site comes back (pairings and walks take a pick or two).
    let pick_again = |mind: &mut Mind, from: u32| {
        (from..from + 20).find_map(|tick| match mind.pick_practice([0.0; 3], tick, Some(&m)) {
            Pick::Site(p, k) => Some((p, k)),
            Pick::Later => None,
            Pick::Nothing => panic!("nothing at tick {tick}"),
        })
    };
    // The only site given up.
    for n in 0..SITE_STRIKES {
        assert!(matches!(
            mind.pick_practice([0.0; 3], n, Some(&m)),
            Pick::Site(..)
        ));
        mind.practice_strike(s, &steps, 2, Some("path_lost_stuck_approach"));
    }
    assert!(!mind.practice.given_up.is_empty());
    assert_eq!(mind.pick_practice([0.0; 3], 20, Some(&m)), Pick::Later);
    assert!(mind.practice.given_up.is_empty());
    assert_eq!(pick_again(&mut mind, 21), Some((s, Some(c))));
    // The only crouch spot given up: the same.
    mind.practice.route_crouch = Some(c);
    for _ in 0..CROUCH_STRIKES {
        mind.practice_strike(s, &steps, 1, Some("prime_failed"));
    }
    assert!(!mind.practice.gone.is_empty());
    assert_eq!(pick_again(&mut mind, 50), Some((s, Some(c))));
    assert!(mind.practice.gone.is_empty());
}

#[test]
fn a_crouch_spot_no_walk_reaches_is_set_aside_and_its_site_paired_with_another() {
    // Zanzibar: every site paired first with a proven crouch spot on a
    // floor a walk drops from but never climbs to, so the walk from the
    // bot to it failed and no site was ever tried. The main floor (x -40
    // to 15, z 0) and a high one (x 15 to 25, z 2) that drops onto it.
    let main = Poly::new(vec![
        [-40.0, -40.0, 0.0],
        [15.0, -40.0, 0.0],
        [15.0, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ]);
    let high = Poly::new(vec![
        [15.0, -5.0, 2.0],
        [25.0, -5.0, 2.0],
        [25.0, 5.0, 2.0],
        [15.0, 5.0, 2.0],
    ]);
    let drop = crate::nav::Edge {
        from: 1,
        to: 0,
        a: [15.0, -5.0, 2.0],
        b: [15.0, 5.0, 2.0],
        kind: crate::nav::Kind::Drop,
    };
    let m = MapData {
        nav: NavMesh::new(vec![main, high], vec![drop]),
        ..MapData::default()
    };
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let up = Crouch {
        status: Status::Proven,
        ..Crouch::at([22.0, 0.0, 2.0])
    };
    let near = Crouch::at([3.0, 0.0, 0.0]);
    let mut mind = Mind::new(Skill::Normal, 45);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![up, near],
    };
    // Proven, it pairs first: a walk from it drops to the site.
    let walk = |a: Vec3, b: Vec3| path_length(&m, a, b);
    let ledge = |s: &Site| launch_floor(&m, s);
    let paired = pair_site(&s, &mind.known, &walk, &ledge, &|_: &Crouch| true).unwrap();
    assert_eq!(paired.crouch, Some(up));
    assert_eq!(walk([0.0; 3], prime_start(&up)), None);
    let pick = (0..10).find_map(|tick| match mind.pick_practice([0.0; 3], tick, Some(&m)) {
        Pick::Site(p, c) => Some((p, c)),
        Pick::Later => None,
        Pick::Nothing => panic!("nothing at tick {tick}"),
    });
    assert_eq!(
        pick,
        Some((s, Some(near))),
        "paired again with the one in reach"
    );
    // Set aside for a while, not given up: from elsewhere a walk may
    // reach it.
    assert!(mind.practice.is_out(&up));
    assert!(mind.practice.gone.is_empty());
}

#[test]
fn a_crouch_spot_whose_primes_fail_for_two_bots_is_given_up_by_both() {
    let m = map();
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let near = Crouch::at([9.0, 1.0, 0.0]);
    let other = Crouch::at([4.0, 0.0, 0.0]);
    // Two bots counting failed primes on one board.
    let two = |seed: u64| -> Vec<Mind> {
        let board: &'static mut FailBoard = Box::leak(Box::default());
        let board: &'static FailBoard = board;
        (0..2)
            .map(|n| {
                let mut mind = Mind::new(Skill::Normal, seed + n);
                mind.known = Known {
                    sites: vec![s],
                    crouches: vec![near, other],
                };
                mind.practice.fails = board;
                mind
            })
            .collect()
    };
    let steps = ["walk_crouched", "prime", "walk", "bounce", "fly"];
    // Each bot fails once at `near`, at `step` for `why`; then what each
    // pairs the site with.
    let after_one_each = |bots: &mut Vec<Mind>, step: usize, why: &'static str| {
        for mind in bots.iter_mut() {
            let Pick::Site(_, crouch) = mind.pick_practice([0.0; 3], 0, Some(&m)) else {
                panic!("a pick");
            };
            assert_eq!(crouch, Some(near));
            mind.practice.route_crouch = crouch;
            mind.practice_strike(s, &steps, step, Some(why));
        }
        bots.iter_mut()
            .map(|mind| match mind.pick_practice([0.0; 3], 9, Some(&m)) {
                Pick::Site(_, crouch) => crouch,
                other => panic!("a pick, not {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    // A failed prime: neither failed twice itself, but the second was the
    // spot's second, so both give it up.
    let mut bots = two(46);
    assert_eq!(
        after_one_each(&mut bots, 1, "prime_failed"),
        [Some(other); 2]
    );
    // A walk that failed on the way to it counts for the bot alone.
    let mut bots = two(48);
    assert_eq!(
        after_one_each(&mut bots, 0, "path_lost_stuck_crouch"),
        [Some(near); 2]
    );
}

// ---- the drill ----

#[test]
fn a_drill_walks_to_its_start_asks_for_the_word_and_tries_from_there_primed() {
    use crate::drill::Drill;
    use crate::glitches::prime::PRIMED_WORD;
    let m = map();
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let mut mind = Mind::new(Skill::Normal, 45);
    mind.known = Known {
        sites: vec![s],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    };
    mind.set_practice(true);
    // The start is away from the bot (it stands at the origin): a plain
    // walk there, no prime and no word asked for.
    mind.set_drill(Some(Drill {
        site: [10.1, 0.0, 0.0],
        start: Some([5.0, 0.0, 0.0]),
    }));
    assert!(mind.wants_pair());
    let walking = (0..40).any(|t| {
        mind.step(&scene(bot(1.0), t), Some(&m));
        mind.order_view().state == "running"
    });
    assert!(walking);
    assert!(mind.take_practice_events().is_empty());
    assert_eq!(mind.order_view().steps, ["nav"]);
    assert!(!mind.drill_wants_arm());
    // At the start, not primed: it stands and asks for the word.
    mind.cancel_order("test");
    mind.set_drill(Some(Drill {
        site: [10.1, 0.0, 0.0],
        start: Some([0.3, 0.0, 0.0]),
    }));
    let mut asked = false;
    for t in 40..50 {
        mind.step(&scene(bot(1.0), t), Some(&m));
        asked |= mind.drill_wants_arm();
    }
    assert!(asked, "asks for the word");
    assert!(mind.take_practice_events().is_empty());
    // The host wrote it: the try, from there, with no prime. The ticks go
    // on from where they were (the start's wait runs to them).
    mind.pair_word = Some(PRIMED_WORD);
    let mut events = Vec::new();
    for t in 50..60 {
        mind.step(&scene(bot(1.0), t), Some(&m));
        events.extend(mind.take_practice_events());
        if events.len() >= 2 {
            break;
        }
    }
    // Started, then primed: read from the game, not its own.
    assert!(
        matches!(
            events.as_slice(),
            [
                PracticeEvent::Started { site, .. },
                PracticeEvent::Primed { read: true, own: false, .. },
            ] if *site == s.bounce.stand
        ),
        "{events:?}"
    );
    let v = mind.order_view();
    assert_eq!(v.steps, ["walk", "bounce", "fly"]);
    assert!(v.primed && v.primed_read);
}

#[test]
fn a_tries_line_says_the_site_the_result_and_its_last_jump() {
    use crate::bounce_route::{Leap, Touchdown};
    let s = site([10.0, 0.0, 0.0], Status::Likely);
    let landing = Touchdown {
        tick: 40,
        at: [10.3, 0.1, 0.0],
        speed_in: 5.2,
        vz_out: 0.0,
        miss: 0.32,
        word: None,
    };
    let jump = |end| Leap {
        tick: 10,
        takeoff: [9.0, 0.0, 2.4],
        speed: 1.25,
        vz: 3.08,
        word: Some(crate::glitches::prime::PRIMED_WORD),
        stick_sum: [6.0, 0.0],
        stick_max: 0.5,
        ticks: 30,
        idle: 4,
        landing: Some(landing),
        end: Some(end),
    };
    let mut v = Mind::new(Skill::Normal, 47).order_view();
    v.steps = vec!["walk", "drop", "fly"];
    v.step = 1;
    v.flights = vec![jump("too_slow"), jump("missed")];
    let line = try_line(true, 0x2f, s, Some("missed"), &v);
    assert!(
        line.starts_with("bots: bounce try: bot 0x2f drill site (10.00, 0.00, 0.00)"),
        "{line}"
    );
    for part in [
        ": missed; jumps 2, the last: takeoff (9.00, 0.00, 2.40) ground 1.25 vz +3.08 word -3",
        "air 30 ticks stick mean (+0.20, +0.00) max 0.50 let go 4",
        "down (10.30, 0.10, 0.00) miss 0.320 in 5.20 out +0.00: missed",
        "; before: too_slow",
    ] {
        assert!(line.contains(part), "{part} in {line}");
    }
    assert!(!line.contains('\n'));
    // No jump: the step it ended on.
    v.flights.clear();
    v.step = 0;
    let line = try_line(false, 1, s, Some("no_path"), &v);
    assert!(
        line.ends_with(": no_path; no jump, ended at step 0 (walk)"),
        "{line}"
    );
}

#[test]
fn a_drill_with_no_known_site_near_it_waits() {
    use crate::drill::Drill;
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 46);
    mind.known = known();
    mind.set_practice(true);
    mind.set_drill(Some(Drill {
        site: [-20.0, 0.0, 0.0],
        start: None,
    }));
    let events = practise(&mut mind, &m, 120, |_| false);
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(mind.practice_view().unwrap().state, "waiting");
    assert_eq!(mind.order_view().state, "none");
}
