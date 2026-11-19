//! The grenade's flight, the aim that lands it, and the thrower.

use super::*;

fn dist(a: Vec3, b: Vec3) -> f32 {
    len(sub(a, b))
}
fn flat(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A floor from -60 to 60, and upright walls: each `(x, y from, y to)`
/// in the plane `x`, 20 high.
fn room(walls: &[(f32, f32, f32)]) -> Walls {
    let mut polys: Vec<Vec<Vec3>> = vec![vec![
        [-60.0, -60.0, 0.0],
        [60.0, -60.0, 0.0],
        [60.0, 60.0, 0.0],
        [-60.0, 60.0, 0.0],
    ]];
    for &(x, y0, y1) in walls {
        polys.push(vec![
            [x, y0, 0.0],
            [x, y1, 0.0],
            [x, y1, 20.0],
            [x, y0, 20.0],
        ]);
    }
    Walls::from_polygons(polys.iter().map(Vec::as_slice))
}

fn ask(at: Vec3, kind: Kind) -> Ask<'static> {
    Ask {
        from: [0.0, 0.0, 0.6],
        at,
        vel: [0.0; 3],
        lead: 0.0,
        ballistics: kind.ballistics(),
        bodies: &[],
        stick: None,
        banks: false,
    }
}

const X: Vec3 = [1.0, 0.0, 0.0];

#[test]
fn a_grenade_falls_as_gravity_says() {
    // Something that sticks where it lands: thrown flat from 1 up at 6 a
    // second, it lands after sqrt(2 / 3.2) s, 6 times that away.
    let b = Ballistics {
        fuse: 0.0,
        ..PLASMA
    };
    let b = Ballistics { speed: 6.0, ..b };
    let f = fly(&Floor(0.0), [0.0, 0.0, 1.0], X, &b, &[]);
    let t = (2.0 / ASSUMED_GRAVITY).sqrt();
    assert!(f.landed);
    assert!((f.blast[0] - 6.0 * t).abs() < 0.05, "{f:?}");
    assert!(f.blast[2].abs() < 1e-3, "{f:?}");
    assert!((f.time - t).abs() < 2.0 * STEP, "{f:?}");
    assert_eq!(f.bounces, 0);
}

#[test]
fn a_frag_bounces_on_and_goes_off_its_fuse_after_the_first_touch() {
    let f = fly(&Floor(0.0), [0.0, 0.0, 1.0], X, &FRAG, &[]);
    let first = f.first.expect("it touched the floor");
    let t = (2.0 / ASSUMED_GRAVITY).sqrt();
    assert!((first[0] - FRAG.speed * t).abs() < 0.05, "{f:?}");
    assert!(f.landed && f.bounces >= 1, "{f:?}");
    assert!((f.time - (t + FRAG.fuse)).abs() < 3.0 * STEP, "{f:?}");
    assert!(f.blast[0] > first[0] + 0.5, "rolled on: {f:?}");
    assert!(f.blast[2] < 0.3, "{f:?}");
}

#[test]
fn a_wall_throws_a_frag_back() {
    let w = room(&[(3.0, -5.0, 5.0)]);
    let f = fly(&w, [0.0, 0.0, 1.0], X, &FRAG, &[]);
    let first = f.first.expect("it hit the wall");
    assert!((first[0] - 3.0).abs() < 0.05, "{f:?}");
    assert!(f.blast[0] < 2.9, "came back off it: {f:?}");
    // Nothing in the way: well past it.
    let f = fly(&room(&[]), [0.0, 0.0, 1.0], X, &FRAG, &[]);
    assert!(f.blast[0] > 4.0, "{f:?}");
}

#[test]
fn a_plasma_sticks_to_a_player_in_its_way() {
    let body = Body {
        at: [3.0, 0.0, 0.0],
        vel: [0.0; 3],
    };
    let f = fly(&Floor(0.0), [0.0, 0.0, 0.6], X, &PLASMA, &[body]);
    assert_eq!(f.stuck, Some(0), "{f:?}");
    assert!(flat(f.blast, body.at) < 0.1, "{f:?}");
    // A frag does not stick: it flies on past.
    let f = fly(&Floor(0.0), [0.0, 0.0, 0.6], X, &FRAG, &[body]);
    assert_eq!(f.stuck, None);
}

#[test]
fn the_aim_lands_it_at_the_target() {
    for d in [5.0f32, 7.0, 9.0] {
        let target = [d, 0.0, 0.0];
        let t = solve(&Floor(0.0), &ask(target, Kind::Frag)).expect("a throw");
        assert!(t.miss <= ACCEPT && !t.bank, "{d}: {t:?}");
        // Flown again, the same.
        let f = fly(&Floor(0.0), [0.0, 0.0, 0.6], t.dir, &FRAG, &[]);
        assert_eq!(f, t.flight);
        assert!(dist(f.blast, target) <= ACCEPT, "{d}: {f:?}");
    }
    // Beyond any throw: none.
    assert_eq!(solve(&Floor(0.0), &ask([40.0, 0.0, 0.0], Kind::Frag)), None);
}

#[test]
fn a_blocked_arc_is_not_thrown() {
    // A wall right across: no throw goes over or round it.
    let w = room(&[(3.0, -50.0, 50.0)]);
    let a = Ask {
        banks: true,
        ..ask([7.0, 0.0, 0.0], Kind::Frag)
    };
    assert_eq!(solve(&w, &a), None);
}

#[test]
fn a_frag_is_rolled_round_a_corner_when_the_straight_throw_is_blocked() {
    // The target stands just round the end of a wall; the straight line
    // to it runs into the wall, one a little to the side clears its end.
    let w = room(&[(3.0, -6.0, -0.2)]);
    let target = [6.0, -1.0, 0.0];
    assert_eq!(solve(&w, &ask(target, Kind::Frag)), None, "Normal: no bank");
    let a = Ask {
        banks: true,
        ..ask(target, Kind::Frag)
    };
    let t = solve(&w, &a).expect("round the corner");
    assert!(t.bank && t.miss <= ACCEPT, "{t:?}");
    let straight = (-1.0f32).atan2(6.0);
    assert!(t.yaw > straight + 0.05, "to the open side: {t:?}");
}

#[test]
fn a_plasma_is_stuck_on_a_close_target() {
    let bodies = [Body {
        at: [4.0, 0.0, 0.0],
        vel: [0.0; 3],
    }];
    let a = Ask {
        bodies: &bodies,
        stick: Some(0),
        ..ask([4.0, 0.0, 0.0], Kind::Plasma)
    };
    let t = solve(&Floor(0.0), &a).expect("a stick");
    assert_eq!(t.flight.stuck, Some(0), "{t:?}");
    assert_eq!(t.miss, 0.0);
}

#[test]
fn a_moving_target_is_led() {
    let vel = [0.0, 1.0, 0.0];
    let led = Ask {
        vel,
        lead: 1.0,
        ..ask([7.0, 0.0, 0.0], Kind::Frag)
    };
    let t = solve(&Floor(0.0), &led).expect("a throw");
    assert!(t.flight.blast[1] > 0.8, "ahead of it: {t:?}");
    let want = add(led.at, scale(vel, t.flight.time));
    assert!(dist(t.flight.blast, want) <= ACCEPT, "{t:?}");
    // No lead (Easy): at where it stands.
    let t = solve(&Floor(0.0), &Ask { lead: 0.0, ..led }).expect("a throw");
    assert!(t.flight.blast[1].abs() < 0.2, "{t:?}");
}

#[test]
fn never_at_its_own_feet_or_a_teammates() {
    let me = [0.0; 3];
    assert!(!safe([2.0, 0.0, 0.0], me, &[], false));
    assert!(safe([6.0, 0.0, 0.0], me, &[], false));
    let mate = [6.5, 1.0, 0.0];
    assert!(!safe([6.0, 0.0, 0.0], me, &[mate], true));
    assert!(safe([6.0, 0.0, 0.0], me, &[mate], false), "Easy: careless");
    assert!(safe([12.0, 0.0, 0.0], me, &[mate], true));
}

#[test]
fn the_skills_throw_better_in_order() {
    let s: Vec<GrenadeSkill> = Skill::ALL.iter().map(|&k| GrenadeSkill::of(k)).collect();
    for w in s.windows(2) {
        assert!(w[1].noise < w[0].noise);
        assert!(w[1].lead >= w[0].lead);
        assert!(w[1].per_minute >= w[0].per_minute);
    }
    assert!(!s[0].banks && !s[0].stick && !s[0].careful, "Easy");
    assert!(!s[1].banks && s[1].stick, "Normal");
    assert!(s[2].banks && s[3].banks);
}

/// Runs a thrower for `ticks` with its aim following the throw (a quick,
/// steady hand), else straight ahead. Returns every tick's hand.
fn run(
    th: &mut Thrower,
    skill: Skill,
    mark: Mark,
    grenades: [i8; 2],
    selected: Kind,
    busy: bool,
    ticks: u32,
) -> Vec<Hand> {
    let eye = [0.0, 0.0, 0.6];
    let mut aim = X;
    let mut sel = selected;
    let mut out = Vec::new();
    for tick in 0..ticks {
        let m = Moment {
            tick,
            dt: 1,
            eye,
            feet: [0.0; 3],
            aim,
            mark: Some(mark),
            enemies: &[],
            mates: &[],
            grenades: Some(grenades),
            selected: Some(sel),
            busy,
            pushing: false,
        };
        let h = th.step(&m, &Floor(0.0), skill);
        aim = h.look.map_or(X, |l| unit(sub(l, eye)));
        if h.swap && !out.last().is_some_and(|p: &Hand| p.swap) {
            // The game changes the type at once.
            sel = match sel {
                Kind::Frag => Kind::Plasma,
                Kind::Plasma => Kind::Frag,
            };
        }
        out.push(h);
    }
    out
}

fn presses(hands: &[Hand]) -> Vec<usize> {
    (0..hands.len())
        .filter(|&i| hands[i].throw && (i == 0 || !hands[i - 1].throw))
        .collect()
}

#[test]
fn the_thrower_turns_to_its_arc_then_throws_spaced_out() {
    let mark = Mark {
        at: [7.0, 0.0, 0.0],
        vel: [0.0; 3],
        seen: true,
    };
    let mut th = Thrower::new(11);
    let hands = run(
        &mut th,
        Skill::Legendary,
        mark,
        [4, 0],
        Kind::Frag,
        false,
        3 * 60 * 30,
    );
    let p = presses(&hands);
    assert!(p.len() >= 2, "{p:?}");
    for w in p.windows(2) {
        assert!(w[1] - w[0] >= COOLDOWN as usize, "{p:?}");
    }
    // Each press looks along the throw.
    for &i in &p {
        assert!(hands[i].look.is_some(), "{i}");
    }
    assert!(th.throws as usize + 1 >= p.len(), "{} {p:?}", th.throws);
    let last = th.last.expect("solved");
    assert!(dist(last.flight.blast, mark.at) <= ACCEPT, "{last:?}");
}

#[test]
fn a_plasma_is_chosen_to_stick_a_close_target() {
    let mark = Mark {
        at: [5.0, 0.0, 0.0],
        vel: [0.0; 3],
        seen: true,
    };
    let mut th = Thrower::new(3);
    let hands = run(
        &mut th,
        Skill::Heroic,
        mark,
        [2, 2],
        Kind::Frag,
        false,
        3 * 60 * 30,
    );
    let swap = hands
        .iter()
        .position(|h| h.swap)
        .expect("switches grenades");
    let first = presses(&hands)[0];
    assert!(swap < first, "the plasma in hand first");
    assert_eq!(th.last.unwrap().flight.stuck, Some(0));
    // Easy does not bother: frags only, no switch.
    let mut th = Thrower::new(3);
    let hands = run(
        &mut th,
        Skill::Easy,
        mark,
        [2, 2],
        Kind::Frag,
        false,
        6 * 60 * 30,
    );
    assert!(hands.iter().all(|h| !h.swap));
}

#[test]
fn no_throw_dual_wielding_without_grenades_or_out_of_range() {
    let mark = Mark {
        at: [7.0, 0.0, 0.0],
        vel: [0.0; 3],
        seen: true,
    };
    let none = |hands: Vec<Hand>| hands.iter().all(|h| !h.throw && h.look.is_none());
    let mut th = Thrower::new(5);
    assert!(none(run(
        &mut th,
        Skill::Legendary,
        mark,
        [4, 4],
        Kind::Frag,
        true,
        3600
    )));
    assert!(none(run(
        &mut th,
        Skill::Legendary,
        mark,
        [0, 0],
        Kind::Frag,
        false,
        3600
    )));
    let far = Mark {
        at: [20.0, 0.0, 0.0],
        ..mark
    };
    assert!(none(run(
        &mut th,
        Skill::Legendary,
        far,
        [4, 0],
        Kind::Frag,
        false,
        3600
    )));
    assert_eq!(th.throws, 0);
}

#[test]
fn temperament_tilt_increases_throw_odds_without_extending_range() {
    use crate::decisions::{Behavior, MoodView, Personality};

    let skill = Skill::Legendary;
    let mood = Behavior {
        personality: Personality::for_skill(11, skill),
        mood: MoodView {
            tilt: 1.0,
            ..MoodView::default()
        },
    };
    let offers = |distance, rate, mates: &[Vec3]| {
        (0..1024)
            .filter(|&seed| {
                let mut th = Thrower::new(seed);
                let m = Moment {
                    tick: 30,
                    dt: 30,
                    eye: [0.0, 0.0, 0.6],
                    feet: [0.0; 3],
                    aim: X,
                    mark: Some(Mark {
                        at: [distance, 0.0, 0.0],
                        vel: [0.0; 3],
                        seen: true,
                    }),
                    enemies: &[],
                    mates,
                    grenades: Some([4, 0]),
                    selected: Some(Kind::Frag),
                    busy: false,
                    pushing: false,
                };
                th.step_with_left(&m, &Floor(0.0), skill, false, rate);
                th.last.is_some()
            })
            .count()
    };
    assert!(offers(7.0, mood.grenade_rate(), &[]) > offers(7.0, 1.0, &[]));
    assert_eq!(offers(RANGE.1 + 1.0, mood.grenade_rate(), &[]), 0);
    assert_eq!(offers(7.0, mood.grenade_rate(), &[[7.0, 0.0, 0.0]]), 0);
}

#[test]
fn a_solved_grenade_drops_the_left_gun_then_waits_for_the_empty_hand() {
    let mut th = Thrower::new(11);
    let mut dual = true;
    let mut first_drop = None;
    let mut aim = X;
    let eye = [0.0, 0.0, 0.6];
    for tick in 0..5400 {
        let moment = Moment {
            tick,
            dt: 1,
            eye,
            feet: [0.0; 3],
            aim,
            mark: Some(Mark {
                at: [7.0, 0.0, 0.0],
                vel: [0.0; 3],
                seen: true,
            }),
            enemies: &[],
            mates: &[],
            grenades: Some([4, 0]),
            selected: Some(Kind::Frag),
            busy: false,
            pushing: false,
        };
        let hand = th.step_with_left(&moment, &Floor(0.0), Skill::Legendary, dual, 1.0);
        if hand.drop_left {
            assert!(dual && !hand.throw && !hand.swap && th.last.is_some());
            let since = *first_drop.get_or_insert(tick);
            dual = tick < since + 2;
        }
        if let Some(look) = hand.look {
            aim = unit(sub(look, eye));
        }
        if hand.throw {
            assert!(first_drop.is_some() && !dual);
            return;
        }
    }
    panic!("the solved throw never followed the drop");
}
