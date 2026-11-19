//! The vehicles' plain data: who sits where, a turret's vehicle, the
//! memory's speeds, flips and exits. The helpers build vehicles for the
//! other tests (`vehicle::*`, `mind::ride`).

use super::*;

/// A vehicle of `class` at `at`, facing +X, upright and whole, with empty
/// seats of these roles.
pub(crate) fn vehicle(id: u32, class: Class, at: Vec3, seats: &[SeatRole]) -> Vehicle {
    Vehicle {
        id,
        class,
        position: at,
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        health: 1.0,
        on: None,
        seats: seats
            .iter()
            .map(|&role| Seat { role, rider: None })
            .collect(),
    }
}

pub(crate) fn ghost(id: u32, at: Vec3) -> Vehicle {
    vehicle(id, Class::Scout, at, &[SeatRole::Driver])
}

/// A Warthog `id` (driver, passenger) and its gun `id + 1` (a gunner's
/// seat) mounted on it.
pub(crate) fn warthog(id: u32, at: Vec3) -> [Vehicle; 2] {
    let hog = vehicle(
        id,
        Class::Jeep,
        at,
        &[SeatRole::Driver, SeatRole::Passenger],
    );
    let mut gun = vehicle(id + 1, Class::Turret, at, &[SeatRole::Gunner]);
    gun.on = Some(id);
    gun.position = [at[0] - 1.0, at[1], at[2] + 0.5];
    [hog, gun]
}

/// Player `player` of `team` in seat `seat`.
pub(crate) fn sit(v: &mut Vehicle, seat: usize, player: u32, team: i16) {
    v.seats[seat].rider = Some(Rider { player, team });
}

#[test]
fn the_type_names_the_class_in_the_tags_order() {
    let all: Vec<Class> = (0..8).map(Class::from_type).collect();
    assert_eq!(
        all,
        vec![
            Class::Tank,
            Class::Jeep,
            Class::Boat,
            Class::Plane,
            Class::Scout,
            Class::Fighter,
            Class::Turret,
            Class::Unknown
        ]
    );
    // Ghosts, Warthogs and Scorpions are driven; the Banshee is later.
    for c in [Class::Scout, Class::Jeep, Class::Tank] {
        assert!(c.spec().drive, "{c:?}");
    }
    for c in [Class::Fighter, Class::Turret, Class::Boat, Class::Unknown] {
        assert!(!c.spec().drive, "{c:?}");
    }
    assert!(
        !Class::Jeep.spec().driver_guns,
        "a Warthog's driver only drives"
    );
    assert!(Class::Scout.spec().driver_guns);
}

#[test]
fn a_turret_belongs_to_its_vehicle_and_its_gunner_is_crew() {
    let [mut hog, mut gun] = warthog(10, [0.0; 3]);
    sit(&mut hog, 0, 3, 1);
    sit(&mut gun, 0, 4, 1);
    let s = Sight {
        riding: Some(Riding {
            vehicle: 11,
            seat: 0,
            role: SeatRole::Gunner,
        }),
        vehicles: vec![hog, gun],
    };
    let gun = s.get(11).unwrap();
    assert_eq!(s.root(gun).id, 10);
    assert_eq!(s.parts(10).count(), 2);
    let crew: Vec<_> = s.crew(10).collect();
    assert_eq!(crew.len(), 2);
    assert!(crew.contains(&(Rider { player: 4, team: 1 }, SeatRole::Gunner)));
    // The gunner rides the Warthog, not "the turret".
    assert_eq!(
        s.ridden_by(4).map(|(v, r)| (v.id, r)),
        Some((10, SeatRole::Gunner))
    );
    assert_eq!(s.ridden_by(3).map(|(v, _)| v.id), Some(10));
    assert_eq!(s.ridden_by(5), None);
    assert_eq!(s.mine().map(|v| v.id), Some(10));
}

#[test]
fn seats_free_and_points_in_the_vehicles_frame() {
    let mut g = ghost(1, [10.0, 0.0, 2.0]);
    assert_eq!(g.free(SeatRole::Driver), Some(0));
    sit(&mut g, 0, 0, 0);
    assert_eq!(g.free(SeatRole::Driver), None);
    assert!(g.has(SeatRole::Driver) && !g.has(SeatRole::Gunner));
    // Facing +Y: forward is +Y and left is -X.
    g.forward = [0.0, 1.0, 0.0];
    let p = g.local([1.0, 2.0]);
    assert!(
        (p[0] - 8.0).abs() < 1e-5 && (p[1] - 1.0).abs() < 1e-5,
        "{p:?}"
    );
    assert_eq!(p[2], 2.0);
    g.up = [0.0, 0.8, 0.2];
    assert!(!g.upright());
}

#[test]
fn the_skills_use_vehicles_more_in_order() {
    let w: Vec<Wheels> = Skill::ALL.iter().map(|&s| Wheels::of(s)).collect();
    for p in w.windows(2) {
        assert!(p[1].take_range > p[0].take_range);
        assert!(p[1].board_range >= p[0].board_range);
        assert!(p[1].board_speed >= p[0].board_speed);
    }
    assert_eq!(w[0].board_range, 0.0, "Easy never boards");
    assert!(!w[0].lock.waits, "Easy fires rockets unlocked");
    assert!(w[3].lock.hold() < w[1].lock.hold());
}

fn memory(vehicles: Vec<Vehicle>, riding: Option<Riding>) -> Memory {
    Memory {
        sight: Sight { riding, vehicles },
        ..Memory::default()
    }
}

#[test]
fn the_memory_works_out_speeds_from_positions() {
    let mut m = memory(vec![ghost(1, [0.0; 3])], None);
    m.update(100);
    assert_eq!(m.speed(1), 0.0, "first seen");
    m.sight.vehicles[0].position = [1.0, 0.0, 0.0];
    m.update(115);
    assert!((m.speed(1) - 2.0).abs() < 1e-4, "1 unit in half a second");
    assert_eq!(m.speed(2), 0.0, "not tracked");
}

#[test]
fn getting_out_shuns_the_vehicle_for_a_while() {
    let me = Riding {
        vehicle: 1,
        seat: 0,
        role: SeatRole::Driver,
    };
    let mut g = ghost(1, [0.0; 3]);
    sit(&mut g, 0, 0, 0);
    let mut m = memory(vec![g.clone()], Some(me));
    m.going = Some(Going {
        pick: plan::Pick {
            why: plan::Why::Take,
            vehicle: 1,
            root: 1,
            role: SeatRole::Driver,
        },
        since: 0,
        side: 0,
        holding: Some(5),
    });
    m.update(10);
    assert_eq!(m.going, None, "in: the going is done");
    assert!(!m.shunned(1, 10));
    // Out.
    g.seats[0].rider = None;
    m.sight = Sight {
        riding: None,
        vehicles: vec![g],
    };
    m.update(20);
    assert!(m.shunned(1, 21));
    assert!(!m.shunned(1, 20 + plan::SHUN_TICKS));
    m.update(20 + plan::SHUN_TICKS);
    assert!(!m.shunned(1, 20 + plan::SHUN_TICKS + 1), "forgotten");
}

#[test]
fn the_memory_counts_flipped_and_driverless_ticks() {
    let [hog, mut gun] = warthog(10, [0.0; 3]);
    sit(&mut gun, 0, 0, 0);
    let me = Riding {
        vehicle: 11,
        seat: 0,
        role: SeatRole::Gunner,
    };
    let mut m = memory(vec![hog, gun], Some(me));
    m.update(0);
    m.update(30);
    assert_eq!(m.driverless, 31, "nobody drives");
    assert_eq!(m.flipped, 0);
    sit(&mut m.sight.vehicles[0], 0, 5, 0);
    m.sight.vehicles[0].up = [0.0, 0.0, -1.0];
    m.update(40);
    assert_eq!(m.driverless, 0);
    assert_eq!(m.flipped, 10);
    m.reset();
    assert_eq!((m.flipped, m.driverless, m.going), (0, 0, None));
}
