use super::*;
use crate::bounce_route::{OrderRun, Plan, Step, Why, chain_onto};
use crate::cache::MapData;
use crate::glitches::Bounce;
use crate::glitches::prime::PRIMED_WORD;

fn site(x: f32, z: f32, launch: f32) -> Site {
    Site {
        bounce: Bounce {
            map: "",
            name: "test",
            stand: [x, 0.0, z],
            spread: 0.03,
            drift: [1.0, 0.0],
            source: "scan",
            takeoff: None,
            landing: None,
        },
        launch,
        ..Site::default()
    }
}

#[test]
fn a_launch_reaches_the_sites_its_air_stick_can_steer_onto() {
    let m = Ballistic::default();
    let a = site(0.0, 0.0, 12.3);
    // Straight back down onto itself, and a seam a few units off.
    let own = m.leg(&a, &a).expect("itself");
    assert_eq!(own.steer, 0.0);
    // Up and back to the same height under the measured fall gravity.
    assert!((own.time - 2.0 * 12.3 / FALL_GRAVITY).abs() < 1e-3);
    assert!((own.speed_in - 12.3).abs() < 1e-3);
    assert!(m.leg(&a, &site(3.0, 0.0, 12.3)).is_some());
    // Too far to steer to in the flight.
    assert!(m.leg(&a, &site(10.0, 0.0, 12.3)).is_none());
    // Over the top of the flight.
    assert!(m.leg(&a, &site(1.0, 30.0, 12.3)).is_none());
    // A launch that is no launch.
    assert!(m.leg(&site(0.0, 0.0, 0.0), &a).is_none());
    // A seam that needs a faster touchdown than the flight brings.
    let picky = Site {
        need: 20.0,
        ..site(1.0, 0.0, 12.3)
    };
    assert!(m.leg(&a, &picky).is_none());
    // A measured window rules out the fast ones.
    let windowed = Ballistic {
        max_speed_in: Some(5.4),
        ..m
    };
    assert!(windowed.leg(&a, &site(1.0, 0.0, 12.3)).is_none());
    // The reach grows with the time in the air, slowly at first.
    assert!(m.reach(0.2) < m.reach(1.0) && m.reach(1.0) < m.reach(5.0));
    assert!((m.reach(5.0) - AIR_TOP * (5.0 - AIR_TOP / AIR_ACCEL / 2.0)).abs() < 1e-4);
}

#[test]
fn reachable_sites_come_least_steering_first_and_a_plan_stops_when_told() {
    let m = Ballistic::default();
    let a = site(0.0, 0.0, 12.3);
    let b = site(2.0, 0.0, 12.3);
    let c = site(1.0, 0.0, 12.3);
    let far = site(40.0, 0.0, 12.3);
    let sites = [a, b, c, far];
    let near = reachable(&a, &sites, &m);
    let order: Vec<f32> = near.iter().map(|(s, _)| s.bounce.stand[0]).collect();
    assert_eq!(order, [0.0, 1.0, 2.0]);
    // Always the farthest reached: a chain of LINKS hops.
    let mut far_most = |_: &Site, near: &[(Site, Leg)]| Some(near.len() - 1);
    assert_eq!(plan(&a, &sites, &m, LINKS, &mut far_most).len(), LINKS);
    // Told to stop: no chain.
    assert!(plan(&a, &sites, &m, LINKS, &mut |_: &Site, _: &[(Site, Leg)]| None).is_empty());
    // Out of range: no link.
    assert!(plan(&a, &sites, &m, LINKS, &mut |_: &Site, _: &[(Site, Leg)]| Some(99)).is_empty());
}

#[test]
fn a_chain_goes_on_after_the_first_bounce_and_ends_with_the_ride_down() {
    let a = site(0.0, 0.0, 12.3);
    let b = site(2.0, 0.0, 12.3);
    let mut p = Plan {
        steps: vec![
            Step::Walk {
                to: [-0.8, 0.0, 0.0],
                crouch: false,
            },
            Step::Bounce(a),
            Step::Fly { to: a.target() },
        ],
        why: Why::Asked,
        site: Some(a),
        cost: None,
    };
    chain_onto(&mut p, &[]);
    assert_eq!(p.steps.len(), 3, "no links: as it was");
    chain_onto(&mut p, &[b, a]);
    let names: Vec<&str> = p.steps.iter().map(Step::name).collect();
    assert_eq!(names, ["walk", "bounce", "chain", "chain", "fly"]);
    assert_eq!(p.steps[4], Step::Fly { to: a.target() });
}

/// Steps `run` through a flight off z 0 at `vz` (up, under the fall
/// gravity) until it comes back to 0 at `x`, then `stay` ticks standing
/// there. The run's end, if it ended.
fn fly(
    run: &mut OrderRun,
    tick: &mut u32,
    x: f32,
    vz: f32,
    stay: u32,
) -> Option<Result<(), &'static str>> {
    let m = MapData::default();
    let dt = 1.0 / 30.0;
    let (mut z, mut v) = (0.0f32, vz);
    loop {
        v -= FALL_GRAVITY * dt;
        z += v * dt;
        if z <= 0.0 {
            break;
        }
        if let Some(done) = run.step(&m, [x, 0.0, z], *tick, Some(PRIMED_WORD)).done {
            return Some(done);
        }
        *tick += 1;
    }
    for _ in 0..stay {
        if let Some(done) = run.step(&m, [x, 0.0, 0.0], *tick, Some(PRIMED_WORD)).done {
            return Some(done);
        }
        *tick += 1;
    }
    None
}

#[test]
fn a_chain_step_counts_its_links_and_ends_at_the_first_that_does_not_launch() {
    let b = site(0.0, 0.0, 12.3);
    let c = site(0.0, 0.0, 12.3);
    let plan = Plan {
        steps: vec![
            Step::Chain(b),
            Step::Chain(c),
            Step::Chain(b),
            Step::Fly { to: b.target() },
        ],
        why: Why::Asked,
        site: Some(b),
        cost: None,
    };
    let mut run = OrderRun::new(plan, true, 0, 1);
    let mut tick = 0;
    // Off the launch before it, down onto b ...
    assert_eq!(fly(&mut run, &mut tick, 0.0, 12.0, 0), None);
    // ... thrown up again (the first link made), straight into the next,
    // and down onto c.
    assert_eq!(fly(&mut run, &mut tick, 0.0, 12.0, 0), None);
    assert_eq!(run.status().chain, (2, 1));
    // On c it stays down: the chain is over, the last link passed over,
    // and nothing to ride down.
    let done = fly(&mut run, &mut tick, 0.0, 0.0, 30);
    assert_eq!(done, Some(Ok(())));
    assert_eq!(run.status().chain, (2, 1));
    let flights = run.flights();
    assert_eq!(flights.len(), 2);
    assert!(flights[0].landing.is_some_and(|t| t.bounced()));
    assert!(flights[1].landing.is_some_and(|t| !t.bounced()));
}
