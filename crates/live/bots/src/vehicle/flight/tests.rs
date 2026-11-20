use super::*;

fn ask() -> FlightIn {
    FlightIn {
        tick: 0,
        at: [0.0, 0.0, 1.0],
        aim: [1.0, 0.0, 0.0],
        forward: [1.0, 0.0, 0.0],
        destination: [18.0, 0.0, 0.0],
        target: None,
        health: 1.0,
        has_bomb: true,
        land: false,
    }
}

#[test]
fn the_banshee_climbs_to_cruise_and_descends_for_a_landing() {
    let mut flight = Flight::default();
    let mut input = ask();
    let out = flight.step(&input, None);
    assert!(out.look[2] > input.at[2]);
    assert_eq!(out.stick, [0.0, 1.0]);
    input.at = [17.0, 0.0, 3.0];
    input.land = true;
    let out = flight.step(&input, None);
    assert!(out.look[2] < input.at[2] && !out.boost && !out.landed);
    input.at[2] = 0.7;
    assert!(flight.step(&input, None).landed);
}

#[test]
fn a_strafe_run_bombs_then_fires_the_cannon_until_it_peels_away() {
    let mut f = Flight::default();
    let mut i = ask();
    i.at[2] = 4.0;
    i.target = Some([10.0, 0.0, 0.0]);
    i.aim = [0.928477, 0.0, -0.371391];
    assert!(f.step(&i, None).bomb);
    i.tick = 1;
    assert!(f.step(&i, None).cannon);
    i.tick = 2;
    i.target = Some([2.0, 0.0, 1.0]);
    let out = f.step(&i, None);
    assert!(out.boost && !out.cannon && out.look[2] > i.at[2]);
}

#[test]
fn the_banshee_banks_away_and_requests_a_flip_when_hit() {
    let mut f = Flight::default();
    let mut i = ask();
    i.at[2] = 5.0;
    f.step(&i, None);
    i.tick = 1;
    i.health = 0.9;
    let out = f.step(&i, None);
    assert!(out.flip && !out.boost && out.stick[0].abs() > 0.9);
    i.tick = 5;
    let out = f.step(&i, None);
    assert!(!out.flip && out.stick[0].abs() > 0.9);
}
