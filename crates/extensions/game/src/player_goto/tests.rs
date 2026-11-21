use super::*;
use agent_proto::goto::SpotRef;

fn spot(kind: Kind, x: f32, score: f32) -> Spot {
    Spot {
        kind,
        edge: [[x, 0.0, 0.0], [x, 1.0, 0.0]],
        stand: [x, 0.0, 0.0],
        launch: 12.0,
        drift: [1.0, 0.0],
        score,
        ..Spot::default()
    }
}

fn scan() -> Vec<Spot> {
    vec![
        spot(Kind::Bounce, 1.0, 0.5),
        spot(Kind::Crouch, 2.0, 0.4),
        spot(Kind::Bounce, 3.0, 0.9),
        spot(Kind::Crouch, 4.0, 0.8),
    ]
}

fn to_spot(spot: usize, kind: Option<&str>) -> GotoTo {
    GotoTo::Spot(SpotRef {
        spot,
        kind: kind.map(Into::into),
    })
}

fn params(to: GotoTo) -> GotoParams {
    GotoParams {
        to,
        within: None,
        face: None,
        timeout_ticks: None,
        then: None,
    }
}

#[test]
fn a_point_is_taken_as_it_is_and_must_be_finite() {
    let t = target(&GotoTo::Point([1.0, 2.0, 3.0]), &[], &[]).unwrap();
    assert_eq!(t, Target::Point([1.0, 2.0, 3.0]));
    assert!(target(&GotoTo::Point([f32::NAN, 0.0, 0.0]), &[], &[]).is_err());
}

#[test]
fn a_spot_is_counted_among_its_kind_best_first_as_the_verifier_counts() {
    let s = scan();
    // Bounce by default: the best (x 3) is 0.
    match target(&to_spot(0, None), &s, &[]).unwrap() {
        Target::Site(site) => assert_eq!(site.bounce.stand[0], 3.0),
        t => panic!("{t:?}"),
    }
    match target(&to_spot(1, Some("bounce")), &s, &[]).unwrap() {
        Target::Site(site) => assert_eq!(site.bounce.stand[0], 1.0),
        t => panic!("{t:?}"),
    }
    match target(&to_spot(0, Some("crouch")), &s, &[]).unwrap() {
        Target::Crouch(c) => assert_eq!(c.stand[0], 4.0),
        t => panic!("{t:?}"),
    }
    let e = target(&to_spot(2, Some("crouch")), &s, &[]).unwrap_err();
    assert!(e.contains("has 2"), "{e}");
    assert!(target(&to_spot(0, Some("ledge")), &s, &[]).is_err());
    // No scan: the mode must be on.
    let e = target(&to_spot(0, None), &[], &[]).unwrap_err();
    assert!(e.contains("superbounce.set"), "{e}");
}

#[test]
fn the_ask_takes_its_defaults_and_refuses_what_does_not_fit() {
    let s = scan();
    let a = ask_for(&params(GotoTo::Point([1.0, 0.0, 0.0])), &s, &[]).unwrap();
    assert_eq!(
        (a.then, a.within, a.timeout),
        (Then::Stop, bots::goto::WITHIN, bots::goto::TIMEOUT)
    );
    let p = GotoParams {
        within: Some(0.1),
        timeout_ticks: Some(90),
        then: Some("bounce".into()),
        ..params(to_spot(0, None))
    };
    let a = ask_for(&p, &s, &[]).unwrap();
    assert_eq!((a.then, a.within, a.timeout), (Then::Bounce, 0.1, 90));
    let refused = |p: GotoParams| ask_for(&p, &s, &[]).is_err();
    assert!(refused(GotoParams {
        then: Some("dance".into()),
        ..params(to_spot(0, None))
    }));
    assert!(refused(GotoParams {
        then: Some("bounce".into()),
        ..params(GotoTo::Point([0.0; 3]))
    }));
    assert!(refused(GotoParams {
        then: Some("bounce".into()),
        ..params(to_spot(0, Some("crouch")))
    }));
    assert!(refused(GotoParams {
        within: Some(0.0),
        ..params(GotoTo::Point([0.0; 3]))
    }));
    assert!(refused(GotoParams {
        within: Some(50.0),
        ..params(GotoTo::Point([0.0; 3]))
    }));
    assert!(refused(GotoParams {
        timeout_ticks: Some(0),
        ..params(GotoTo::Point([0.0; 3]))
    }));
    assert!(refused(GotoParams {
        face: Some(f32::INFINITY),
        ..params(GotoTo::Point([0.0; 3]))
    }));
}

#[test]
fn a_spot_ends_at_its_approach_before_the_mesh_is_known() {
    let s = scan();
    let Target::Site(site) = target(&to_spot(0, None), &s, &[]).unwrap() else {
        panic!()
    };
    assert_eq!(rough_goal(&Target::Site(site)), site.bounce.from());
    let Target::Crouch(c) = target(&to_spot(0, Some("crouch")), &s, &[]).unwrap() else {
        panic!()
    };
    assert_eq!(
        rough_goal(&Target::Crouch(c)),
        bots::bounce_route::prime_start(&c)
    );
    // A drop's floor is not known without the mesh: its strip.
    let mut d = site;
    d.fall = 2.4;
    assert_eq!(rough_goal(&Target::Site(d)), d.target());
}

#[test]
fn the_states_name_themselves_and_only_the_live_ones_hold_the_pad() {
    use State::*;
    for (s, name, live) in [
        (Loading, "loading", true),
        (Running, "running", true),
        (Facing, "facing", true),
        (Done, "done", false),
        (Failed, "failed", false),
        (Cancelled, "cancelled", false),
    ] {
        assert_eq!((s.name(), s.live()), (name, live));
    }
}
