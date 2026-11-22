use super::*;
use crate::glitches::Bounce;
use crate::mind::Skill;
use crate::nav::{NavMesh, Poly};
use crate::spots::{Crouch, Known, Site};

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

fn known() -> Known {
    Known {
        sites: vec![Site {
            bounce: Bounce {
                map: "",
                name: "test",
                stand: [10.0, 0.0, 0.0],
                spread: 0.03,
                drift: [1.0, 0.0],
                source: "scan",
                takeoff: None,
            },
            launch: 12.3,
            fall: 0.0,
            ..Site::default()
        }],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    }
}

fn perception(at: Vec3, tick: u32) -> Perception {
    Perception {
        me: Some(Me {
            position: at,
            ..Me::default()
        }),
        tick,
        ..Perception::default()
    }
}

#[test]
fn an_order_is_queued_until_the_next_sample_and_then_runs() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.known = known();
    assert_eq!(mind.order_view().state, "none");
    mind.order_bounce([18.0, 0.0, 0.0], Mode::Fun);
    assert_eq!(mind.order_view().state, "queued");

    let pad = mind.order_pad(&perception([0.0; 3], 1), Me::default(), Some(&m), &[]);
    assert!(pad.is_some());
    let v = mind.order_view();
    assert_eq!(v.state, "running");
    assert_eq!(v.why, Some(Why::Fun));
    assert_eq!(
        v.steps,
        ["walk_crouched", "prime", "walk", "bounce", "fly", "nav"]
    );
    assert!(!mind.primed());
    assert_eq!(v.to, Some([18.0, 0.0, 0.0]));
}

#[test]
fn with_nothing_to_bounce_on_the_order_is_the_plain_walk() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.order_bounce([6.0, 0.0, 0.0], Mode::Help);
    mind.order_pad(&perception([0.0; 3], 1), Me::default(), Some(&m), &[]);
    let v = mind.order_view();
    assert_eq!(v.why, Some(Why::Plain));
    assert_eq!(v.steps, ["nav"]);
}

#[test]
fn a_show_off_route_is_given_up_at_the_sight_of_an_enemy_and_an_ordered_one_is_not() {
    let m = map();
    let enemy = [Seen {
        id: 2,
        position: [5.0, 5.0, 0.0],
        team: 1,
    }];
    for (show_off, kept) in [(true, false), (false, true)] {
        let mut mind = Mind::new(Skill::Normal, 1);
        mind.known = known();
        mind.asked = Some(Asked {
            to: [18.0, 0.0, 0.0],
            mode: Mode::Fun,
            show_off,
            site: show_off.then(|| known().sites[0]),
            crouch: None,
            plain: false,
        });
        mind.order_pad(&perception([0.0; 3], 1), Me::default(), Some(&m), &[]);
        assert_eq!(mind.order_view().state, "running");
        let pad = mind.order_pad(&perception([0.0; 3], 2), Me::default(), Some(&m), &enemy);
        assert_eq!(pad.is_some(), kept, "show_off {show_off}");
        if !kept {
            let v = mind.order_view();
            assert_eq!((v.state, v.error), ("failed", Some("enemy")));
        }
    }
}

#[test]
fn a_dead_bot_drops_its_order_and_says_why() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.known = known();
    mind.order_bounce([6.0, 0.0, 0.0], Mode::Help);
    mind.release(5);
    let v = mind.order_view();
    assert_eq!((v.state, v.error), ("failed", Some("died")));
    // Under way too.
    mind.order_bounce([18.0, 0.0, 0.0], Mode::Fun);
    mind.order_pad(&perception([0.0; 3], 6), Me::default(), Some(&m), &[]);
    assert_eq!(mind.order_view().state, "running");
    mind.cancel_order("map_changed");
    let v = mind.order_view();
    assert_eq!((v.state, v.error), ("failed", Some("map_changed")));
    // Nothing to cancel: the last result stands.
    mind.release(9);
    assert_eq!(mind.order_view().error, Some("map_changed"));
}

#[test]
fn a_failed_show_off_gives_its_site_up() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.known = known();
    let site = known().sites[0];
    mind.asked = Some(Asked {
        to: [0.0; 3],
        mode: Mode::Fun,
        show_off: true,
        site: Some(site),
        crouch: None,
        plain: false,
    });
    mind.order_pad(&perception([0.0; 3], 1), Me::default(), Some(&m), &[]);
    assert!(!mind.stuck.blocked(site.bounce.stand, 2));
    let enemy = [Seen { id: 2, position: [5.0, 5.0, 0.0], team: 1 }];
    mind.order_pad(&perception([0.0; 3], 2), Me::default(), Some(&m), &enemy);
    assert!(mind.stuck.blocked(site.bounce.stand, 3));
}

#[test]
fn an_order_without_a_map_fails_by_name() {
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.order_bounce([6.0, 0.0, 0.0], Mode::Help);
    assert!(mind.order_pad(&perception([0.0; 3], 1), Me::default(), None, &[]).is_none());
    let v = mind.order_view();
    assert_eq!((v.state, v.error), ("failed", Some("no_map")));
}

#[test]
fn a_read_word_at_minus_three_skips_the_priming() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.known = known();
    mind.pair_word = Some(crate::glitches::PRIMED_WORD);
    mind.order_bounce([18.0, 0.0, 0.0], Mode::Fun);
    mind.order_pad(&perception([0.0; 3], 1), Me::default(), Some(&m), &[]);
    let v = mind.order_view();
    assert_eq!(v.steps, ["walk", "bounce", "fly", "nav"]);
}

#[test]
fn the_route_pad_never_crouches_once_primed() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 1);
    mind.known = known();
    mind.pair_word = Some(crate::glitches::PRIMED_WORD);
    mind.order_bounce([18.0, 0.0, 0.0], Mode::Fun);
    for tick in 1..30 {
        let pad = mind
            .order_pad(&perception([0.0; 3], tick), Me::default(), Some(&m), &[])
            .unwrap();
        assert!(!pad.buttons.contains(crate::mind::CROUCH));
    }
}
