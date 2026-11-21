use super::*;
use agent_proto::route::Jump;
use bots::nav::{NavMesh, Poly};

fn biped() -> Biped {
    Biped {
        unit: 0x10000,
        position: [0.0; 3],
        velocity: [0.0; 3],
        aim: [1.0, 0.0, 0.0],
        airborne: false,
        supported: true,
        crouch: 0.0,
        crouch_request: false,
        ticks_per_second: 30.0,
    }
}

fn run(legs: Vec<Leg>, timeout_ticks: u32) -> Run {
    Run::new(RouteParams {
        player: 0,
        legs,
        timeout_ticks,
    })
}

#[test]
fn waits_share_the_route_budget_and_finish_on_the_last_allowed_tick() {
    let nav = MapData::default();
    let mut route = run(
        vec![
            Leg::Wait { wait: 0 },
            Leg::Wait { wait: 1 },
            Leg::Wait { wait: 1 },
        ],
        2,
    );
    assert!(route.tick(&nav, biped(), 10).is_some());
    assert_eq!(route.status.ticks, 0);
    assert_eq!(route.status.leg, Some(1));
    assert!(route.tick(&nav, biped(), 11).is_some());
    assert_eq!(route.status.leg, Some(2));
    assert!(route.tick(&nav, biped(), 12).is_none());
    assert_eq!(route.status.state, State::Completed);
    assert_eq!(route.status.ticks, 2);
    assert_eq!(
        route
            .status
            .legs
            .iter()
            .map(|l| (l.ok, l.ticks))
            .collect::<Vec<_>>(),
        [(Some(true), 0), (Some(true), 1), (Some(true), 1)]
    );
}

#[test]
fn timeout_fails_the_current_leg_and_skips_the_rest() {
    let nav = MapData::default();
    let mut route = run(
        vec![
            Leg::Wait { wait: 1 },
            Leg::Wait { wait: 5 },
            Leg::Wait { wait: 4 },
        ],
        2,
    );
    assert!(route.tick(&nav, biped(), 10).is_some());
    assert!(route.tick(&nav, biped(), 11).is_some());
    assert!(route.tick(&nav, biped(), 12).is_none());
    assert_eq!(route.status.state, State::Failed);
    assert_eq!(route.status.legs[0].ok, Some(true));
    assert_eq!(route.status.legs[1].reason, Some("timeout"));
    assert_eq!(route.status.legs[1].ticks, 1);
    assert_eq!(route.status.legs[2].reason, Some("not_run"));
}

#[test]
fn stop_releases_the_current_leg_and_preserves_finished_results() {
    let nav = MapData::default();
    let mut route = run(vec![Leg::Wait { wait: 1 }, Leg::Wait { wait: 5 }], 10);
    route.tick(&nav, biped(), 10);
    route.tick(&nav, biped(), 11);
    route.stop(State::Stopped, "stopped");
    assert!(route.tick(&nav, biped(), 12).is_none());
    assert_eq!(route.status.state, State::Stopped);
    assert_eq!(route.status.legs[0].ok, Some(true));
    assert_eq!(route.status.legs[1].reason, Some("stopped"));
}

#[test]
fn missing_mesh_says_which_map_to_generate() {
    let err = load_mesh(PathBuf::new(), "ascension", 0, Stamp::now(123)).unwrap_err();
    assert_eq!(err.code, code::UNAVAILABLE);
    assert!(err.message.contains("h2 bots nav ascension"));
}

#[test]
fn crouch_releases_the_click_before_holding_and_turning_out() {
    let nav = MapData::default();
    let mut route = run(
        vec![Leg::Crouch {
            crouch_into: [0.1, 0.0, 0.0],
            hold_ticks: 2,
            exit_face: -66.0,
        }],
        30,
    );
    let mut biped = biped();
    assert!(route.tick(&nav, biped, 0).unwrap().crouch);

    biped.crouch = 1.0;
    biped.crouch_request = true;
    for tick in 1..5 {
        assert!(route.tick(&nav, biped, tick).unwrap().crouch);
    }
    for tick in 5..7 {
        let pad = route.tick(&nav, biped, tick).unwrap();
        assert!(!pad.crouch);
        assert!(pad.speed > 0.0);
        assert_eq!(pad.yaw, None);
    }
    let exit = route.tick(&nav, biped, 7).unwrap();
    assert!(!exit.crouch && exit.speed > 0.0);
    let (sin, cos) = exit.yaw.unwrap().sin_cos();
    biped.aim = [cos, sin, 0.0];
    biped.crouch = 0.0;
    biped.crouch_request = false;
    assert!(route.tick(&nav, biped, 8).is_none());
    assert_eq!(route.status.state, State::Completed);
}

#[test]
fn jump_keeps_incoming_velocity_when_the_player_rebounds() {
    let floor = Poly::new(vec![
        [-2.0, -2.0, 0.0],
        [2.0, -2.0, 0.0],
        [2.0, 2.0, 0.0],
        [-2.0, 2.0, 0.0],
    ]);
    let nav = MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    };
    let mut route = run(
        vec![Leg::Jump {
            jump: Jump {
                from: [0.0; 3],
                land: [0.0; 3],
            },
        }],
        40,
    );
    let mut biped = biped();
    route.tick(&nav, biped, 10);
    route.tick(&nav, biped, 11);
    assert!(route.tick(&nav, biped, 12).unwrap().jump);
    biped.airborne = true;
    biped.position[2] = 0.08;
    biped.velocity[2] = 1.9;
    route.tick(&nav, biped, 13);
    biped.position[2] = 0.36;
    biped.velocity[2] = 0.0;
    route.tick(&nav, biped, 14);
    biped.position[2] = -0.07;
    biped.velocity[2] = -2.0;
    route.tick(&nav, biped, 15);
    biped.position[2] = 0.35;
    biped.velocity[2] = 12.6;
    assert!(route.tick(&nav, biped, 16).is_none());
    assert_eq!(route.status.state, State::Completed);
    let jump = route.status.legs[0].jump.as_ref().unwrap();
    assert_eq!(jump.takeoff.unwrap().position, [0.0; 3]);
    assert_eq!(jump.apex.unwrap().position[2], 0.36);
    assert_eq!(jump.landing.unwrap().velocity[2], -2.0);
}
