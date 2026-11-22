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
    MapData {
        nav: NavMesh::new(vec![floor, island], vec![]),
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
    let started = |e: &[PracticeEvent]| e.iter().any(|e| matches!(e, PracticeEvent::Started { .. }));
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
    assert!(events.is_empty(), "never started, never tallied: {events:?}");
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
