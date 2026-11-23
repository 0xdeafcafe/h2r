use super::*;
use crate::bounce_route::{FALL_GRAVITY, OrderRun, Plan, Step, Why, chain_onto};
use crate::cache::MapData;
use crate::glitches::Bounce;
use crate::glitches::prime::PRIMED_WORD;
use superbounce::Status;

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

fn proven(s: Site) -> Site {
    Site {
        status: Status::Proven,
        ..s
    }
}

/// The leg is the flight model's (`superbounce::flight`), by the HUD's
/// second-bounce rule: a 12.3 launch comes back to its height on row 175
/// (5.83 s) at 12.03 units/s, and the full stick carries it 6.33 out from
/// no sideways speed, so the pursuit lands on any target within 6.3.
#[test]
fn a_launch_reaches_the_sites_the_huds_flight_model_reaches() {
    let m = Engine::default();
    let a = site(0.0, 0.0, 12.3);
    // Straight back down onto itself.
    let own = m.leg(&a, &a).expect("itself");
    assert_eq!((own.steer, own.miss), (0.0, 0.0));
    let down = flight::fall(0.0, 12.3, 0.0).unwrap();
    assert_eq!((own.time, own.speed_in), (down.time(), down.speed));
    assert!((own.time - 175.0 / 30.0).abs() < 1e-4, "{own:?}");
    assert!((own.speed_in - 12.03).abs() < 0.01, "{own:?}");
    // The same flight the HUD flies: it ends on the target, and the reach
    // along the steer is the full stick's.
    for x in [1.0, 3.0, 6.0] {
        let leg = m.leg(&a, &site(x, 0.0, 12.3)).expect("in reach");
        let p = flight::pursue([0.0; 3], [0.0, 0.0, 12.3], [x, 0.0], |_| 0.0).unwrap();
        assert_eq!(leg.miss, p.miss);
        assert!(leg.miss < 0.02 && (leg.steer - x).abs() < 1e-5, "{leg:?}");
        assert!((leg.reach - 6.334).abs() < 0.01, "{leg:?}");
        assert_eq!(leg.cost, flight::cost(leg.steer, leg.reach, 0.0, leg.speed_in));
    }
    // Past what the stick can do in the flight: 6.5 ends 0.17 short.
    assert!(m.leg(&a, &site(6.5, 0.0, 12.3)).is_none());
    // Far past it: ruled out with no flight.
    assert!(m.leg(&a, &site(10.0, 0.0, 12.3)).is_none());
    // Over the top of the flight (17.9).
    assert!(m.leg(&a, &site(1.0, 18.0, 12.3)).is_none());
    // Under it, a little longer and faster.
    let low = m.leg(&a, &site(1.0, -1.0, 12.3)).expect("lower");
    assert!(low.time > own.time && low.speed_in > own.speed_in, "{low:?}");
    // A launch that is no launch.
    assert!(m.leg(&site(0.0, 0.0, 0.0), &a).is_none());
    assert!(m.leg(&site(0.0, 0.0, f32::NAN), &a).is_none());
    // A seam that needs a faster touchdown than the flight brings.
    let picky = Site {
        need: 20.0,
        ..site(1.0, 0.0, 12.3)
    };
    assert!(m.leg(&a, &picky).is_none());
    // A measured window rules out the fast ones.
    let windowed = Engine {
        max_speed_in: Some(5.4),
        ..m
    };
    assert!(windowed.leg(&a, &site(1.0, 0.0, 12.3)).is_none());
    // A flight longer than the longest leg.
    let short = Engine { max_time: 5.0, ..m };
    assert!(short.leg(&a, &a).is_none());
    // A tighter catch: the pursuit must end nearer.
    assert!(m.leg(&a, &site(6.3, 0.0, 12.3)).is_some());
    let tight = Engine { catch: 0.0, ..m };
    assert!(tight.leg(&a, &site(6.5, 0.0, 12.3)).is_none());
    // Proven: sure.
    assert!(!own.sure);
    assert!(m.leg(&a, &proven(site(1.0, 0.0, 12.3))).unwrap().sure);
}

#[test]
fn reachable_sites_come_proven_first_then_the_easiest() {
    let m = Engine::default();
    let a = site(0.0, 0.0, 12.3);
    let b = site(2.0, 0.0, 12.3);
    let c = site(1.0, 0.0, 12.3);
    let d = proven(site(4.0, 0.0, 12.3));
    let far = site(40.0, 0.0, 12.3);
    let sites = [a, b, c, d, far];
    let near = reachable(&a, &sites, &m);
    let order: Vec<f32> = near.iter().map(|(s, _)| s.bounce.stand[0]).collect();
    assert_eq!(order, [4.0, 0.0, 1.0, 2.0]);
    // The likely ones by cost: the steer's share of the reach.
    assert!(near[1].1.cost < near[2].1.cost && near[2].1.cost < near[3].1.cost);
}

#[test]
fn the_net_holds_every_leg_between_the_sites_a_route_plays() {
    let m = Engine::default();
    let a = site(0.0, 0.0, 12.3);
    let b = proven(site(2.0, 0.0, 12.3));
    let far = site(40.0, 0.0, 12.3);
    // A candidate crease is the training's until it is proved.
    let candidate = Site {
        candidate: true,
        ..site(1.0, 0.0, 12.3)
    };
    let net = Net::build(&[a, b, far, candidate], &m);
    assert_eq!(net.sites().len(), 3);
    // a and b reach each other and themselves; far only itself.
    assert_eq!(net.legs(), 5);
    let off_a: Vec<f32> = net.from(&a).iter().map(|(s, _)| s.bounce.stand[0]).collect();
    assert_eq!(off_a, [2.0, 0.0]);
    let off_far: Vec<f32> = net.from(&far).iter().map(|(s, _)| s.bounce.stand[0]).collect();
    assert_eq!(off_far, [40.0]);
    // Each leg is the model's.
    for (s, leg) in net.from(&a) {
        assert_eq!(Some(leg), m.leg(&a, &s));
    }
    // A site not in the net reaches nothing.
    assert!(net.from(&candidate).is_empty());
    assert!(net.from(&site(9.0, 0.0, 12.3)).is_empty());
    // Found by its stand, to a hair.
    assert_eq!(net.index(&site(0.01, 0.0, 12.3)), Some(0));
    assert_eq!(net.index(&site(0.0, 5.0, 12.3)), None);
    assert_eq!(Net::build(&[], &m).legs(), 0);
}

#[test]
fn a_chain_plan_walks_the_net_and_stops_when_told() {
    let m = Engine::default();
    let a = site(0.0, 0.0, 12.3);
    let b = site(2.0, 0.0, 12.3);
    let c = site(1.0, 0.0, 12.3);
    let far = site(40.0, 0.0, 12.3);
    let net = Net::build(&[a, b, c, far], &m);
    // Always the last reached: a chain of LINKS hops.
    let mut last = |_: &Site, near: &[(Site, Leg)]| near.len().checked_sub(1);
    assert_eq!(plan(&a, &net, LINKS, &mut last).len(), LINKS);
    // Told to stop: no chain.
    assert!(plan(&a, &net, LINKS, &mut |_: &Site, _: &[(Site, Leg)]| None).is_empty());
    // Out of range: no link.
    assert!(plan(&a, &net, LINKS, &mut |_: &Site, _: &[(Site, Leg)]| Some(99)).is_empty());
    // Off a site nothing reaches from: none.
    assert!(plan(&far, &Net::build(&[a, far], &Engine { max_time: 1.0, ..m }), LINKS, &mut last).is_empty());
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
