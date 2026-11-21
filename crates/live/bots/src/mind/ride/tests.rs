//! The mind with vehicles (docs/game/bots.md, Vehicles): going to take
//! one and holding X beside its seat, boarding, driving and shooting from
//! a seat, getting out, and the rocket launcher's lock.
//!
//! The sticks are eased (mind::look's hand, mind::pad's feet) and the
//! reaction is sampled (tactics): the tests run a few ticks before reading
//! a stick, and look for a shot over a run rather than on one tick.

use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};
use crate::brain::{Me, Perception, Seen, Vec3};
use crate::cache::MapData;
use crate::los::Walls;
use crate::mind::{Doing, Mind, RELOAD, Skill};
use crate::nav::{NavMesh, Poly};
use crate::tactics::MIN_REACTION;
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

fn enter(g: &Vehicle, attempt: u32) -> Me {
    let pick = plan::Pick {
        why: plan::Why::Take,
        vehicle: g.id,
        root: g.id,
        role: SeatRole::Driver,
    };
    let (at, entry) = plan::approach(g, pick, g.position, attempt, None).unwrap();
    let mut me = bot(at);
    me.aim = entry.forward.map(|v| -v);
    me
}

#[test]
fn a_calm_bot_walks_to_a_free_ghost_and_holds_x_beside_its_seat() {
    let mut m = Mind::new(Skill::Normal, 1);
    let g = ghost(100, [5.0, 0.0, 0.0]);
    m.see_vehicles(on_foot(vec![g.clone()]));
    let mut pad = HostPadState::default();
    for t in 0..4 {
        pad = step(&mut m, bot([0.0; 3]), vec![], t, None);
        assert_eq!(m.doing, Doing::Mount(100));
        assert!(!holds_x(&pad), "not there yet");
    }
    assert!(pad.left_stick.1 > 0.5, "walks to it: {:?}", pad.left_stick);
    // Beside the driver's seat: X.
    let pad = step(&mut m, enter(&g, 0), vec![], 4, None);
    assert!(holds_x(&pad));
    assert_eq!(pad.left_stick, (0.0, 0.0), "it stands to get in");
}

#[test]
fn a_seat_that_will_not_take_it_is_retried_closer_then_given_up() {
    let mut m = Mind::new(Skill::Normal, 1);
    let g = ghost(100, [5.0, 0.0, 0.0]);
    m.see_vehicles(on_foot(vec![g.clone()]));
    step(&mut m, bot([0.0; 3]), vec![], 0, None);
    let first = enter(&g, 0);
    let other = enter(&g, 1);
    let mut t = 1;
    while t <= plan::ENTER_TICKS {
        assert!(holds_x(&step(&mut m, first, vec![], t, None)), "tick {t}");
        t += 1;
    }
    // This side is done: it goes round to the other.
    let pad = step(&mut m, first, vec![], t, None);
    assert!(!holds_x(&pad));
    assert_eq!(m.vehicles.going.map(|g| g.side), Some(1));
    let start = t + 1;
    for t in start..start + plan::ENTER_TICKS {
        assert!(holds_x(&step(&mut m, other, vec![], t, None)), "tick {t}");
    }
    step(&mut m, other, vec![], start + plan::ENTER_TICKS, None);
    // Both sides failed: given up, and left alone.
    step(&mut m, other, vec![], start + plan::ENTER_TICKS + 1, None);
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

/// Steps ticks `0..=last` with one enemy at `foe`: the last pad, and
/// whether the trigger was down on any tick (the reaction is sampled, and
/// a burst may be between shots on the last one).
fn run(m: &mut Mind, me: Me, foe: Vec3, last: u32) -> (HostPadState, bool) {
    let mut pad = HostPadState::default();
    let mut fired = false;
    for t in 0..=last {
        pad = step(m, me, vec![enemy(1, foe)], t, None);
        fired |= pad.right_trigger > 0.0;
    }
    (pad, fired)
}

#[test]
fn a_ghost_driver_runs_infantry_over_and_shoots_them() {
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(driving(ghost(100, [0.0; 3])));
    let (pad, fired) = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 30);
    assert_eq!(m.doing, Doing::Drive(100));
    assert!(
        pad.left_stick.1 > 0.9,
        "full ahead at them: {:?}",
        pad.left_stick
    );
    assert!(fired, "the Ghost's guns");
    assert!(!holds_x(&pad));
}

#[test]
fn a_warthog_driver_drives_at_them_and_does_not_shoot() {
    let mut m = Mind::new(Skill::Normal, 1);
    let [hog, _] = warthog(100, [0.0; 3]);
    m.see_vehicles(driving(hog));
    let (pad, fired) = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 30);
    assert!(pad.left_stick.1 > 0.9, "{:?}", pad.left_stick);
    assert!(!fired);
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
    let (pad, fired) = run(&mut m, bot([0.0; 3]), [8.0, 0.0, 0.0], 30);
    assert_eq!(m.doing, Doing::Ride(100));
    assert!(fired);
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
        places: Default::default(),
        nav: NavMesh::new(vec![floor], vec![]),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(driving(ghost(100, [2.0, 2.0, 0.0])));
    let mut pad = HostPadState::default();
    for t in 0..10 {
        pad = step(&mut m, bot([2.0, 2.0, 0.0]), vec![], t, Some(&map));
    }
    assert_eq!(m.doing, Doing::Drive(100));
    // The one polygon's middle is up and to the left of the aim (+X): the
    // eased sticks have come round to it.
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

#[test]
fn seeded_driver_emits_the_same_pads_with_warm_and_cold_searches() {
    let floor = |x| {
        Poly::new(vec![
            [x, 0., 0.],
            [x + 8., 0., 0.],
            [x + 8., 8., 0.],
            [x, 8., 0.],
        ])
    };
    let map = std::sync::Arc::new(MapData {
        nav: NavMesh::new(vec![floor(0.), floor(18.)], vec![]),
        ..Default::default()
    });
    let start = [2., 2., 0.];
    let mut warm = Mind::new(Skill::Normal, 13);
    let mut cold = warm.clone();
    warm.vehicles.paths.bind_map(&map);
    for tick in 0..48 {
        let id = if tick < 32 { 100 } else { 200 };
        // The target crosses between disconnected floors. A warmed
        // exhaustive miss and a newly reachable destination both occur.
        let at = if (16..32).contains(&tick) {
            [7., 6., 0.]
        } else {
            [20., 6., 0.]
        };
        let sight = driving(ghost(id, start));
        warm.see_vehicles(sight.clone());
        cold.see_vehicles(sight);
        let think = |m: &mut Mind| step(m, bot(start), vec![enemy(1, at)], tick, Some(&map));
        assert_eq!(think(&mut warm), think(&mut cold), "pad at tick {tick}");
        assert_eq!(warm.doing, Doing::Drive(id));
        assert_eq!(warm.vehicles.route, cold.vehicles.route);
    }
    assert!(warm.vehicles.paths.searches < cold.vehicles.paths.searches);
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
        ..Arms::default()
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
    // The reaction is sampled (tactics::Fight::focus), at most 18 ticks.
    let at_infantry = first_rocket(Skill::Normal, false).expect("fires at infantry");
    assert!((MIN_REACTION..=18).contains(&at_infantry), "{at_infantry}");
    assert!(at_infantry < k.hold());
    let at_ghost = first_rocket(Skill::Normal, true).expect("fires at the Ghost");
    assert_eq!(at_ghost, k.hold(), "held from first sight until locked");
    // Easy does not wait.
    let easy = first_rocket(Skill::Easy, true).expect("fires");
    assert!((MIN_REACTION..=22).contains(&easy), "{easy}");
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
    for t in 0..20 {
        pad = step(&mut m, me, vec![enemy(1, [10.0, 0.0, 0.0])], t, None);
    }
    assert!(
        pad.right_stick.1 < -0.2,
        "down to it: {:?}",
        pad.right_stick
    );
}

#[test]
fn a_standalone_gunner_stays_on_the_gun_until_flanked() {
    let mut m = Mind::new(Skill::Normal, 1);
    let mut gun = crate::vehicle::tests::vehicle(
        100,
        crate::vehicle::Class::Turret,
        [0.0; 3],
        &[SeatRole::Gunner],
    );
    sit(&mut gun, 0, 0, 0);
    m.see_vehicles(Sight {
        riding: Some(Riding {
            vehicle: 100,
            seat: 0,
            role: SeatRole::Gunner,
        }),
        vehicles: vec![gun],
    });
    for tick in 0..140 {
        assert!(!holds_x(&step(
            &mut m,
            bot([0.0; 3]),
            vec![enemy(1, [8.0, 0.0, 0.0])],
            tick,
            None
        )));
    }
    let pad = step(
        &mut m,
        bot([0.0; 3]),
        vec![enemy(1, [0.0, 2.0, 0.0])],
        140,
        None,
    );
    assert!(holds_x(&pad));
    assert_eq!(
        m.vehicles.leaving,
        Some(crate::vehicle::drive::Leave::Flanked)
    );
    assert_eq!(pad.right_trigger, 0.0);
}

/// A large map as a driver on a small pocket of it sees it: the pocket the
/// vehicle is on, and a wide polygon beyond a link it cannot drive over
/// (an infantry jump). Every roam goal the mesh offers the chassis is
/// unreachable, as on a big vehicle map whose infantry routes climb away
/// from the roads.
fn pocket_map(link: crate::nav::Kind) -> MapData {
    let pocket = Poly::new(vec![
        [0.0, 0.0, 0.0],
        [4.0, 0.0, 0.0],
        [4.0, 4.0, 0.0],
        [0.0, 4.0, 0.0],
    ]);
    let beyond = Poly::new(vec![
        [0.0, 20.0, 0.0],
        [60.0, 20.0, 0.0],
        [60.0, 60.0, 0.0],
        [0.0, 60.0, 0.0],
    ]);
    let nav = NavMesh::new(
        vec![pocket, beyond],
        vec![crate::nav::Edge {
            from: 0,
            to: 1,
            a: [2.0, 4.0, 0.0],
            b: [2.0, 20.0, 0.0],
            kind: link,
        }],
    );
    MapData {
        nav,
        walls: Walls::default(),
        ..MapData::default()
    }
}

/// Steps `ticks` as the driver of a Ghost on the pocket at `[2, 2]`, the
/// vehicle held still: the last pad, whether X was ever held, whether the
/// right stick ever swept, and the first tick X was held.
fn park_pocket(m: &mut Mind, map: &MapData, ticks: u32) -> (HostPadState, bool, bool, Option<u32>) {
    let at = [2.0, 2.0, 0.0];
    let mut x = false;
    let mut swept = false;
    let mut first_x = None;
    let mut pad = HostPadState::default();
    for t in 0..ticks {
        m.see_vehicles(driving(ghost(100, at)));
        pad = step(m, bot(at), vec![], t, Some(map));
        swept |= pad.right_stick != (0.0, 0.0);
        if holds_x(&pad) {
            first_x = first_x.or(Some(t));
            x = true;
        }
    }
    (pad, x, swept, first_x)
}

#[test]
fn a_driver_with_nowhere_to_drive_gets_out_and_roams_on_foot_instead() {
    let map = pocket_map(crate::nav::Kind::Jump);
    let mut m = Mind::new(Skill::Legendary, 7);
    let (pad, _, swept, first_x) = park_pocket(&mut m, &map, 200);
    assert_eq!(m.doing, Doing::Drive(100));
    let t = first_x.expect("holds X to get out");
    // Not at once (it tried: a goal, a plan, a wait), and well within the
    // window a parked player would give up in.
    assert!((60..=190).contains(&t), "when it got out: {t}");
    assert_eq!(
        m.vehicles.leaving,
        Some(crate::vehicle::drive::Leave::Nowhere)
    );
    assert_eq!(pad.left_stick, (0.0, 0.0));
    assert_eq!(pad.right_trigger, 0.0);
    assert!(swept, "while it waited it looked around");
    // The goal the chassis cannot reach is not offered again at once.
    let beyond = map.nav.polys[1].center;
    assert!(m.stuck.blocked(beyond, t));

    // Out: the vehicle is left alone well past the usual shun, so it
    // fights on foot rather than parking again.
    let out = 200;
    m.see_vehicles(on_foot(vec![ghost(100, [2.0, 2.0, 0.0])]));
    step(&mut m, bot([2.0, 2.0, 0.0]), vec![], out, Some(&map));
    assert_eq!(m.vehicles.planless, 0);
    assert!(m.vehicles.shunned(100, out + crate::vehicle::plan::SHUN_TICKS));
    assert!(!m.vehicles.shunned(100, out + crate::vehicle::NOWHERE_SHUN_TICKS));
    for t in out + 1..out + 60 {
        m.see_vehicles(on_foot(vec![ghost(100, [2.0, 2.0, 0.0])]));
        step(&mut m, bot([2.0, 2.0, 0.0]), vec![], t, Some(&map));
        assert_ne!(m.doing, Doing::Mount(100), "it does not get back in");
    }
}

#[test]
fn a_driver_with_a_way_over_keeps_driving_and_never_holds_x() {
    let map = pocket_map(crate::nav::Kind::Walk);
    let mut m = Mind::new(Skill::Legendary, 7);
    // The vehicle rolls ahead at half a unit a tick, as the pad asks.
    let mut x = false;
    let mut pad = HostPadState::default();
    for t in 0..150u32 {
        let at = [2.0, 2.0 + 0.5 * t as f32, 0.0];
        m.see_vehicles(driving(ghost(100, at)));
        pad = step(&mut m, bot(at), vec![], t, Some(&map));
        x |= holds_x(&pad);
    }
    assert!(!x, "nowhere to leave over a drivable link");
    assert_eq!(m.vehicles.leaving, None);
    assert!(m.vehicles.planless < crate::vehicle::drive::NOWHERE_TICKS);
    assert_eq!(m.doing, Doing::Drive(100));
    assert!(pad.left_stick != (0.0, 0.0), "it steers: {:?}", pad.left_stick);
}

#[test]
fn the_mind_builds_a_flight_pad_for_the_banshee_driver() {
    let mut m = Mind::new(Skill::Normal, 1);
    let mut banshee = crate::vehicle::tests::vehicle(
        100,
        crate::vehicle::Class::Fighter,
        [0.0, 0.0, 3.0],
        &[SeatRole::Driver],
    );
    m.see_vehicles(driving(banshee.clone()));
    let pad = step(&mut m, bot(banshee.position), vec![], 0, None);
    assert_eq!(m.doing, Doing::Drive(100));
    assert!(pad.left_stick.1 > 0.9);
    banshee.health = 0.05;
    m.see_vehicles(driving(banshee.clone()));
    let pad = step(&mut m, bot(banshee.position), vec![], 1, None);
    assert!(holds_x(&pad));
    assert_eq!(pad.left_stick, (0.0, 0.0));
}

fn pickup(id: u32, at: Vec3) -> crate::arms::Pickup {
    crate::arms::Pickup {
        id,
        position: at,
        gun: crate::arms::Gun::default(),
    }
}

/// On a large map the weapons plan can want a gun far across the map. A
/// near one is walked to and rules the vehicle out as before (§3.6);
/// a far one is what a vehicle is for: ridden to, then taken on foot
/// (the errand in `vehicle::Memory`).
#[test]
fn a_far_weapon_seek_takes_a_vehicle_a_near_one_does_not() {
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(on_foot(vec![ghost(100, [5.0, 0.0, 0.0])]));
    let p = Perception {
        me: Some(bot([0.0, 0.0, 0.0])),
        teams: true,
        tick: 0,
        ..Perception::default()
    };
    let mut friends = Vec::new();
    m.kit = crate::arms::Kit::Seek(pickup(7, [120.0, 0.0, 0.0]));
    assert_eq!(
        m.mount_plan(&p, None, &bot([0.0; 3]), None, true, None, &mut friends),
        Some(100),
        "the far gun is ridden to"
    );
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(on_foot(vec![ghost(100, [5.0, 0.0, 0.0])]));
    m.kit = crate::arms::Kit::Seek(pickup(8, [6.0, 0.0, 0.0]));
    let mut friends = Vec::new();
    assert_eq!(
        m.mount_plan(&p, None, &bot([0.0; 3]), None, true, None, &mut friends),
        None,
        "the near gun is walked to"
    );
}

/// The errand ride itself: the driver's route is to the sought gun, and
/// within [`crate::vehicle::drive::ON_FOOT`] of it the bot gets out to
/// finish on foot.
#[test]
fn a_driver_rides_to_its_errand_and_gets_out_to_take_it() {
    let floor = Poly::new(vec![
        [0.0, 0.0, 0.0],
        [40.0, 0.0, 0.0],
        [40.0, 10.0, 0.0],
        [0.0, 10.0, 0.0],
    ]);
    let map = MapData {
        places: Default::default(),
        nav: NavMesh::new(vec![floor], vec![]),
        walls: Walls::default(),
    };
    let gun = [36.0, 2.0, 0.0];
    let mut m = Mind::new(Skill::Normal, 1);
    m.see_vehicles(driving(ghost(100, [2.0, 2.0, 0.0])));
    m.vehicles.errand = Some(gun);
    for t in 0..3 {
        step(&mut m, bot([2.0, 2.0, 0.0]), vec![], t, Some(&map));
        assert_eq!(m.doing, Doing::Drive(100), "tick {t}");
    }
    assert_eq!(
        m.vehicles.route.as_ref().map(|r| r.goal),
        Some(gun),
        "the route is to the gun"
    );
    // Within ON_FOOT of the gun: out, on foot, for the weapon.
    m.see_vehicles(driving(ghost(100, [35.0, 2.0, 0.0])));
    let pad = step(&mut m, bot([35.0, 2.0, 0.0]), vec![], 3, Some(&map));
    assert!(holds_x(&pad), "X to get out");
    assert_eq!(
        m.vehicles.leaving,
        Some(crate::vehicle::drive::Leave::There)
    );
}
