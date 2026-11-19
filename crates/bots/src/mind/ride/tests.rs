//! The mind with vehicles (docs/game/bots.md, section 3.8): going to take
//! one and holding X beside its seat, boarding, driving and shooting from
//! a seat, getting out, and the rocket launcher's lock.

use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};
use crate::brain::{Me, Perception, Seen, Vec3};
use crate::cache::MapData;
use crate::los::Walls;
use crate::mind::{Doing, Mind, RELOAD, Skill};
use crate::nav::{NavMesh, Poly};
use crate::vehicle::tests::{ghost, sit, warthog};
use crate::vehicle::{Riding, SeatRole, Sight, Vehicle, plan};
use input::host::HostPadState;

const X: Vec3 = [1.0, 0.0, 0.0];

fn bot(at: Vec3) -> Me {
    Me {
        position: at,
        aim: X,
        team: 0,
        ..Me::default()
    }
}
fn enemy(id: u32, at: Vec3) -> Seen {
    Seen {
        id,
        position: at,
        team: 1,
    }
}
fn step(m: &mut Mind, me: Me, others: Vec<Seen>, tick: u32, map: Option<&MapData>) -> HostPadState {
    m.step(
        &Perception {
            me: Some(me),
            others,
            teams: true,
            tick,
            ..Perception::default()
        },
        map,
    )
}
fn on_foot(vehicles: Vec<Vehicle>) -> Sight {
    Sight {
        riding: None,
        vehicles,
    }
}
fn holds_x(pad: &HostPadState) -> bool {
    pad.buttons.contains(RELOAD)
}

#[test]
fn a_calm_bot_walks_to_a_free_ghost_and_holds_x_beside_its_seat() {
    let mut m = Mind::new(Skill::Normal, 1);
    let g = ghost(100, [5.0, 0.0, 0.0]);
    m.see_vehicles(on_foot(vec![g.clone()]));
    let pad = step(&mut m, bot([0.0; 3]), vec![], 0, None);
    assert_eq!(m.doing, Doing::Mount(100));
    assert!(pad.left_stick.1 > 0.5, "walks to it: {:?}", pad.left_stick);
    assert!(!holds_x(&pad), "not there yet");
    // Beside the driver's seat: X.
    let side = plan::approach(&g, SeatRole::Driver, 0);
    let pad = step(&mut m, bot(side), vec![], 1, None);
    assert!(holds_x(&pad));
    assert_eq!(pad.left_stick, (0.0, 0.0), "it stands to get in");
}

#[test]
fn a_seat_that_will_not_take_it_is_tried_from_the_other_side_then_given_up() {
    let mut m = Mind::new(Skill::Normal, 1);
    let g = ghost(100, [5.0, 0.0, 0.0]);
    m.see_vehicles(on_foot(vec![g.clone()]));
    step(&mut m, bot([0.0; 3]), vec![], 0, None);
    let first = plan::approach(&g, SeatRole::Driver, 0);
    let other = plan::approach(&g, SeatRole::Driver, 1);
    let mut t = 1;
    while t <= plan::ENTER_TICKS {
        assert!(
            holds_x(&step(&mut m, bot(first), vec![], t, None)),
            "tick {t}"
        );
        t += 1;
    }
    // This side is done: it goes round to the other.
    let pad = step(&mut m, bot(first), vec![], t, None);
    assert!(!holds_x(&pad));
    assert_eq!(m.vehicles.going.map(|g| g.side), Some(1));
    let start = t + 1;
    for t in start..start + plan::ENTER_TICKS {
        assert!(
            holds_x(&step(&mut m, bot(other), vec![], t, None)),
            "tick {t}"
        );
    }
    step(&mut m, bot(other), vec![], start + plan::ENTER_TICKS, None);
    // Both sides failed: given up, and left alone.
    step(
        &mut m,
        bot(other),
        vec![],
        start + plan::ENTER_TICKS + 1,
        None,
    );
    assert_ne!(m.doing, Doing::Mount(100));
    assert!(m.vehicles.shunned(100, start + plan::ENTER_TICKS + 2));
}

#[test]
fn in_a_fight_it_fights_rather_than_take_a_vehicle() {
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(on_foot(vec![ghost(100, [5.0, 0.0, 0.0])]));
    step(
        &mut m,
        bot([0.0; 3]),
        vec![enemy(1, [4.0, 1.0, 0.0])],
        0,
        None,
    );
    assert_eq!(m.doing, Doing::Fight(1));
}

#[test]
fn an_enemy_ghost_close_and_slow_is_boarded_and_let_go_when_it_speeds_off() {
    let mut m = Mind::new(Skill::Normal, 1);
    let mut g = ghost(100, [1.0, 0.0, 0.0]);
    sit(&mut g, 0, 1, 1);
    m.see_vehicles(on_foot(vec![g.clone()]));
    let pad = step(
        &mut m,
        bot([0.0; 3]),
        vec![enemy(1, [1.0, 0.0, 0.0])],
        0,
        None,
    );
    assert_eq!(m.doing, Doing::Mount(100));
    assert!(holds_x(&pad), "within reach: X");
    // A tick later it is half a unit on: 15 a second, too fast.
    g.position = [1.5, 0.0, 0.0];
    m.see_vehicles(on_foot(vec![g]));
    step(
        &mut m,
        bot([0.0; 3]),
        vec![enemy(1, [1.5, 0.0, 0.0])],
        1,
        None,
    );
    assert_ne!(m.doing, Doing::Mount(100));
}

/// The bot (player 0) driving `v`, seat 0.
fn driving(mut v: Vehicle) -> Sight {
    sit(&mut v, 0, 0, 0);
    Sight {
        riding: Some(Riding {
            vehicle: v.id,
            seat: 0,
            role: SeatRole::Driver,
        }),
        vehicles: vec![v],
    }
}

/// Steps ticks `0..=last` with one enemy at `foe`; the last pad.
fn run(m: &mut Mind, me: Me, foe: Vec3, last: u32) -> HostPadState {
    let mut pad = HostPadState::default();
    for t in 0..=last {
        pad = step(m, me, vec![enemy(1, foe)], t, None);
    }
    pad
}

#[test]
fn a_ghost_driver_runs_infantry_over_and_shoots_them() {
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(driving(ghost(100, [0.0; 3])));
    let pad = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 12);
    assert_eq!(m.doing, Doing::Drive(100));
    assert!(
        pad.left_stick.1 > 0.9,
        "full ahead at them: {:?}",
        pad.left_stick
    );
    assert_eq!(pad.right_trigger, 1.0, "the Ghost's guns");
    assert!(!holds_x(&pad));
}

#[test]
fn a_warthog_driver_drives_at_them_and_does_not_shoot() {
    let mut m = Mind::new(Skill::Normal, 1);
    let [hog, _] = warthog(100, [0.0; 3]);
    m.see_vehicles(driving(hog));
    let pad = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 12);
    assert!(pad.left_stick.1 > 0.9, "{:?}", pad.left_stick);
    assert_eq!(pad.right_trigger, 0.0);
}

#[test]
fn a_gunner_shoots_and_leaves_the_driving_to_its_driver() {
    let mut m = Mind::new(Skill::Normal, 1);
    let [mut hog, mut gun] = warthog(100, [0.0; 3]);
    sit(&mut hog, 0, 2, 0);
    sit(&mut gun, 0, 0, 0);
    m.see_vehicles(Sight {
        riding: Some(Riding {
            vehicle: 101,
            seat: 0,
            role: SeatRole::Gunner,
        }),
        vehicles: vec![hog, gun],
    });
    let pad = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 12);
    assert_eq!(m.doing, Doing::Ride(100));
    assert_eq!(pad.right_trigger, 1.0);
    assert_eq!(pad.left_stick, (0.0, 0.0));
}

#[test]
fn it_gets_out_of_a_wreck_and_does_not_get_straight_back_in() {
    let mut m = Mind::new(Skill::Normal, 1);
    let mut g = ghost(100, [0.0; 3]);
    g.health = 0.1;
    m.see_vehicles(driving(g.clone()));
    let pad = step(&mut m, bot([0.0; 3]), vec![], 0, None);
    assert!(holds_x(&pad), "out");
    assert_eq!(pad.left_stick, (0.0, 0.0));
    // Out, and (say) the Ghost is mended: still left alone for now.
    g.health = 1.0;
    m.see_vehicles(on_foot(vec![g]));
    step(&mut m, bot([0.5, 0.0, 0.0]), vec![], 1, None);
    assert_ne!(m.doing, Doing::Mount(100));
}

#[test]
fn with_nobody_about_the_driver_follows_a_route_turning_by_the_camera() {
    let floor = Poly::new(vec![
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 20.0, 0.0],
        [0.0, 20.0, 0.0],
    ]);
    let map = MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(driving(ghost(100, [2.0, 2.0, 0.0])));
    let pad = step(&mut m, bot([2.0, 2.0, 0.0]), vec![], 0, Some(&map));
    assert_eq!(m.doing, Doing::Drive(100));
    // The one polygon's middle is up and to the left of the aim (+X).
    assert!(
        pad.right_stick.0 < -0.5,
        "turns left: {:?}",
        pad.right_stick
    );
    let (x, y) = pad.left_stick;
    assert!(
        x < 0.0 && y > 0.0,
        "ahead and left, eased: {:?}",
        pad.left_stick
    );
    assert!((x * x + y * y).sqrt() < 0.95);
}

fn rockets() -> Arms {
    Arms {
        held: Some(Gun {
            def: 1,
            ammo: Ammo::Rounds {
                loaded: 2,
                capacity: 2,
                reserve: 4,
                reserve_max: 8,
            },
            profile: Profile {
                trigger: Trigger::Single,
                zoom: 1,
                pellets: 1,
                shot_speed: 40.0,
                ..Profile::default()
            },
        }),
        other: None,
        grenades: [0, 0],
    }
}

/// The first tick (of 0..90) a bot of `skill` holding rockets fires at an
/// enemy 10 ahead, on foot or (`in_ghost`) driving a Ghost.
fn first_rocket(skill: Skill, in_ghost: bool) -> Option<u32> {
    let mut m = Mind::new(skill, 1);
    let mut g = ghost(100, [10.0, 0.0, 0.0]);
    sit(&mut g, 0, 1, 1);
    m.see_vehicles(on_foot(if in_ghost { vec![g] } else { vec![] }));
    let me = Me {
        arms: Some(rockets()),
        ..bot([0.0; 3])
    };
    (0..90)
        .find(|&t| step(&mut m, me, vec![enemy(1, [10.0, 0.0, 0.0])], t, None).right_trigger > 0.0)
}

#[test]
fn rockets_at_a_vehicle_wait_for_the_lock_and_at_infantry_do_not() {
    let k = crate::vehicle::Wheels::of(Skill::Normal).lock;
    let at_infantry = first_rocket(Skill::Normal, false).expect("fires at infantry");
    assert_eq!(at_infantry, Skill::Normal.aim().reaction);
    let at_ghost = first_rocket(Skill::Normal, true).expect("fires at the Ghost");
    assert_eq!(at_ghost, k.hold(), "held from first sight until locked");
    // Easy does not wait.
    let easy = first_rocket(Skill::Easy, true).expect("fires");
    assert_eq!(easy, Skill::Easy.aim().reaction);
}

#[test]
fn rockets_aim_at_the_vehicles_middle_without_lead() {
    let mut m = Mind::new(Skill::Legendary, 1);
    let mut g = ghost(100, [10.0, 0.0, 0.0]);
    sit(&mut g, 0, 1, 1);
    m.see_vehicles(on_foot(vec![g]));
    // Aimed a little high: the stick brings it down to the middle.
    let me = Me {
        arms: Some(rockets()),
        aim: [0.98, 0.0, 0.2],
        ..bot([0.0; 3])
    };
    let mut pad = HostPadState::default();
    for t in 0..10 {
        pad = step(&mut m, me, vec![enemy(1, [10.0, 0.0, 0.0])], t, None);
    }
    assert!(
        pad.right_stick.1 < -0.2,
        "down to it: {:?}",
        pad.right_stick
    );
}
