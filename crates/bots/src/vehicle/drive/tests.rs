use super::*;

fn line() -> Route {
    Route::new(
        (1..=10).map(|i| [i as f32 * 2.0, 0.0, 0.0]).collect(),
        [20.0, 0.0, 0.0],
    )
}

#[test]
fn the_carrot_is_the_furthest_point_in_reach_ahead() {
    let mut r = line();
    // From the start: points 2 and 4 are in reach (4), 6 is not.
    assert_eq!(r.carrot([0.0; 3]), Some([4.0, 0.0, 0.0]));
    // Passed points drop out.
    assert_eq!(r.carrot([7.0, 0.0, 0.0]), Some([10.0, 0.0, 0.0]));
    // Near the end it aims at the end, then it is there.
    assert_eq!(r.carrot([17.0, 0.0, 0.0]), Some([20.0, 0.0, 0.0]));
    assert!(!r.done());
    assert_eq!(r.carrot([18.5, 0.0, 0.0]), None);
    assert!(r.done());
}

#[test]
fn a_far_next_point_is_still_the_carrot() {
    let mut r = Route::straight([50.0, 0.0, 0.0]);
    assert_eq!(r.carrot([0.0; 3]), Some([50.0, 0.0, 0.0]));
    assert_eq!(r.goal, [50.0, 0.0, 0.0]);
}

#[test]
fn the_throttle_eases_for_a_sharp_turn() {
    assert_eq!(throttle(0.0), 1.0);
    assert!(throttle(0.8) < 1.0 && throttle(0.8) > TURN_THROTTLE);
    assert!((throttle(-3.0) - TURN_THROTTLE).abs() < 1e-6);
    assert!(throttle(0.4) > throttle(1.0));
}

#[test]
fn a_vehicle_that_does_not_move_under_throttle_backs_up_then_gives_up() {
    let mut u = Unstick::default();
    let mut t = 0;
    // Moving: fine. Not pushing: fine however slow.
    assert_eq!(u.step(t, 5.0, true), Unstuck::Drive);
    assert_eq!(u.step(t + 100, 0.0, false), Unstuck::Drive);
    let mut reverses = 0;
    let gave_up = loop {
        t += 1;
        match u.step(t, 0.0, true) {
            Unstuck::Drive => {}
            Unstuck::Reverse => reverses += 1,
            Unstuck::GiveUp => break t,
        }
        assert!(t < 1000, "never gave up");
    };
    assert_eq!(reverses, 2 * REVERSE_TICKS as i32);
    assert!(gave_up < STUCK_WINDOW, "{gave_up}");
}

#[test]
fn stuck_spells_far_apart_do_not_add_up() {
    let mut u = Unstick::default();
    let mut gave_up = false;
    for spell in 0..6u32 {
        let start = spell * (STUCK_WINDOW + 100);
        for t in start..start + STUCK_TICKS + REVERSE_TICKS + 2 {
            gave_up |= u.step(t, 0.0, true) == Unstuck::GiveUp;
        }
        // Then it drives off.
        u.step(start + STUCK_TICKS + REVERSE_TICKS + 3, 5.0, true);
    }
    assert!(!gave_up);
}

fn ride() -> Ride {
    Ride {
        health: 1.0,
        flipped: 0,
        driverless: 0,
        driver: true,
        stuck: false,
        there: false,
    }
}

#[test]
fn it_gets_out_of_a_wreck_a_flip_a_stuck_one_or_a_stranded_gun() {
    assert_eq!(leave(&ride()), None);
    assert_eq!(
        leave(&Ride {
            health: 0.1,
            ..ride()
        }),
        Some(Leave::Wrecked)
    );
    assert_eq!(
        leave(&Ride {
            flipped: FLIPPED_TICKS,
            ..ride()
        }),
        Some(Leave::Flipped)
    );
    assert_eq!(
        leave(&Ride {
            stuck: true,
            ..ride()
        }),
        Some(Leave::Stuck)
    );
    assert_eq!(
        leave(&Ride {
            there: true,
            ..ride()
        }),
        Some(Leave::There)
    );
    // A driver with nobody else driving is not stranded; a gunner is.
    let gun = Ride {
        driver: false,
        driverless: DRIVERLESS_TICKS,
        ..ride()
    };
    assert_eq!(leave(&gun), Some(Leave::Stranded));
    assert_eq!(
        leave(&Ride {
            driver: true,
            ..gun
        }),
        None
    );
}

#[test]
fn infantry_in_front_and_in_reach_is_run_over() {
    let at = [0.0; 3];
    let x = [1.0, 0.0];
    assert!(splatter(true, at, x, [8.0, 1.0, 0.0], SPLATTER_RANGE));
    assert!(!splatter(false, at, x, [8.0, 1.0, 0.0], SPLATTER_RANGE));
    assert!(
        !splatter(true, at, x, [-8.0, 0.0, 0.0], SPLATTER_RANGE),
        "behind"
    );
    assert!(
        !splatter(true, at, x, [30.0, 0.0, 0.0], SPLATTER_RANGE),
        "far"
    );
}

#[test]
fn cached_lookahead_still_checks_the_live_span_ahead() {
    use crate::los::Walls;
    use crate::nav::{NavMesh, Poly};
    use crate::vehicle::Class;

    let floor = Poly::new(vec![
        [-1., -4., 0.],
        [10., -4., 0.],
        [10., 4., 0.],
        [-1., 4., 0.],
    ]);
    let wall = [[3., -2., -1.], [3., 2., -1.], [3., 2., 3.], [3., -2., 3.]];
    let map = MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        walls: Walls::from_polygons([wall.as_slice()]),
        ..Default::default()
    };
    let spec = Class::Scout.spec();
    let goal = [7., 3., 0.];
    let mut route = Route::straight(goal);
    assert_eq!(
        route.steer(&map, spec, [0., 3., 0.], 0., 0, Some(0), 3),
        Some(goal)
    );
    assert!(!clear(&map, spec, [0.; 3], goal));
    assert_eq!(
        route.steer(&map, spec, [0.; 3], 0., 1, Some(0), 3),
        Some(goal)
    );
    assert_eq!(
        route.steer(&map, spec, [2.5, 0., 0.], 0., 2, Some(0), 3),
        None
    );
}
