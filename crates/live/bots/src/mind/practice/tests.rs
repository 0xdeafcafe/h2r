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
    for _ in 0..8 {
        let pick = mind.pick_practice([0.0; 3], tick).unwrap();
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
