use super::*;
use crate::nav::{NavMesh, Poly};

fn square(z: f32) -> Poly {
    Poly::new(vec![
        [0.0, 0.0, z],
        [10.0, 0.0, z],
        [10.0, 10.0, z],
        [0.0, 10.0, z],
    ])
}

#[test]
fn a_dot_that_moves_is_the_same_dot_and_four_are_kept() {
    let mut t = Tracker::default();
    t.saw(&[[3.0, 0.0, 0.0]], Some(10.0), 0, 15);
    t.saw(&[[3.5, 0.5, 0.0]], None, 6, 15);
    assert_eq!(t.dots(6), [[3.5, 0.5, 0.0]]);
    let five = [
        [1.0, 0.0, 0.0],
        [-4.0, 0.0, 0.0],
        [0.0, 4.0, 0.0],
        [0.0, -4.0, 0.0],
        [6.0, 6.0, 0.0],
    ];
    t.saw(&five, None, 12, 15);
    assert_eq!(t.dots(12).len(), DOTS);
}

#[test]
fn a_dot_is_kept_a_while_after_the_last_glance_that_showed_it() {
    let mut t = Tracker::default();
    t.saw(&[[3.0, 0.0, 0.0]], None, 0, 15);
    // A glance that shows nothing forgets nothing.
    t.saw(&[], None, 10, 15);
    assert_eq!(t.nearest([0.0; 3], KEEP_TICKS), Some([3.0, 0.0, 0.0]));
    assert_eq!(t.nearest([0.0; 3], KEEP_TICKS + 1), None);
}

#[test]
fn only_a_dot_inside_half_the_disc_puts_it_on_its_guard() {
    let mut t = Tracker::default();
    t.saw(&[[7.0, 0.0, 0.0]], Some(10.0), 0, 15);
    assert!(!t.near([0.0; 3], 1));
    t.saw(&[[4.0, 0.0, 0.0]], Some(10.0), 2, 15);
    assert!(t.near([0.0; 3], 3));
    // Without the disc's size, the default one.
    let mut t = Tracker::default();
    t.saw(&[[RANGE * NEAR - 0.5, 0.0, 0.0]], None, 0, 15);
    assert!(t.near([0.0; 3], 1));
}

#[test]
fn a_new_dot_starts_a_glance_and_a_moved_one_does_not() {
    let mut t = Tracker::default();
    t.saw(&[[3.0, 0.0, 0.0]], None, 0, 15);
    assert!(t.glancing(14));
    assert!(!t.glancing(15));
    t.saw(&[[3.4, 0.0, 0.0]], None, 20, 15);
    assert!(!t.glancing(21), "the same dot, moved");
    t.saw(&[[3.4, 0.0, 0.0], [-3.0, 0.0, 0.0]], None, 26, 12);
    assert!(t.glancing(27), "a second dot");
}

#[test]
fn at_a_dot_with_nobody_there_it_tries_the_floor_over_it_then_gives_it_up() {
    let map = MapData {
        nav: NavMesh::new(vec![square(0.0), square(3.0)], vec![]),
        ..Default::default()
    };
    let mut t = Tracker::default();
    t.saw(&[[5.5, 5.0, 0.0]], None, 0, 15);
    let me = [5.0, 5.0, 0.0];
    assert_eq!(t.goal(me, 1), None, "standing at it");
    t.arrive(me, Some(&map), 1);
    assert_eq!(t.goal(me, 2), Some([5.5, 5.0, 3.0]));
    // The same dot on the next glance keeps the floor it was given.
    t.saw(&[[6.0, 5.0, 0.0]], None, 6, 15);
    assert_eq!(t.goal(me, 7), Some([6.0, 5.0, 3.0]));
    // Up there with nobody in view: no other floor is tried again.
    t.arrive([6.0, 5.0, 3.0], Some(&map), 8);
    assert_eq!(t.goal([6.0, 5.0, 3.0], 9), None);
    assert_eq!(t.nearest(me, 9), None);
    // Without a floor elsewhere the dot is given up at once.
    let flat = MapData {
        nav: NavMesh::new(vec![square(0.0)], vec![]),
        ..Default::default()
    };
    let mut t = Tracker::default();
    t.saw(&[[5.5, 5.0, 0.0]], None, 0, 15);
    t.arrive(me, Some(&flat), 1);
    assert_eq!(t.nearest(me, 2), None);
}

#[test]
fn in_a_fight_only_a_dot_close_behind_with_nobody_seen_there_is_checked() {
    let enemy = |at| Seen {
        id: 1,
        team: 1,
        position: at,
    };
    let aim = [1.0, 0.0, 0.0];
    let check = |dot: Vec3, seen: &[Seen]| {
        let mut t = Tracker::default();
        t.saw(&[dot], None, 10, 15);
        t.check_behind([0.0; 3], aim, seen, 10, 9);
        t.checking(10)
    };
    let behind = [-2.0, 0.5, 0.0];
    assert_eq!(check(behind, &[]), Some(behind));
    assert_eq!(check([-4.0, 0.0, 0.0], &[]), None, "too far");
    assert_eq!(check([2.0, 0.5, 0.0], &[]), None, "in front");
    assert_eq!(check(behind, &[enemy([-2.2, 0.4, 0.0])]), None, "seen");
    // The turn ends.
    let mut t = Tracker::default();
    t.saw(&[behind], None, 10, 15);
    t.check_behind([0.0; 3], aim, &[], 10, 9);
    assert_eq!(t.checking(18), Some(behind));
    assert_eq!(t.checking(19), None);
}
