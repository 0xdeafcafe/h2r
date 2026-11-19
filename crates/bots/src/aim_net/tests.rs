use super::*;
use crate::mind::Skill;

/// A settled hand on a target eight units away, with the skill's numbers.
fn rest(skill: Skill) -> Features {
    let aim = skill.base_aim();
    Features {
        since_acquired: 1.0,
        distance: 8.0,
        looseness: 0.5,
        gain: aim.gain,
        spread: aim.error,
        lead: aim.lead,
        ..Features::default()
    }
}

fn tolerance(f: &Features) -> f32 {
    let x = f.encode();
    TOL_SPREAD * x[input::SPREAD] + TOL_SIZE * x[input::SIZE] + TOL_LOOSE * x[input::LOOSENESS]
}

fn yaw(skill: Skill, error: f32) -> f32 {
    turn(&Features {
        error: [error, 0.0],
        ..rest(skill)
    })[0]
}

#[test]
fn the_same_inputs_give_the_same_bits() {
    let f = Features {
        error: [0.31, -0.07],
        error_rate: [-1.2, 0.4],
        target_rate: [0.6, -0.1],
        stick: [0.4, -0.2],
        pending: [0.05, -0.01],
        noise: [0.004, -0.002],
        since_acquired: 0.2,
        ..rest(Skill::Heroic)
    };
    let first = turn(&f);
    for _ in 0..4 {
        let again = turn(&f);
        assert_eq!(first[0].to_bits(), again[0].to_bits());
        assert_eq!(first[1].to_bits(), again[1].to_bits());
    }
    assert_eq!(hidden(&f.encode()), hidden(&f.encode()));
    assert!(first[0] > 0.0 && first[1] < 0.0, "{first:?}");
}

#[test]
fn no_error_asks_for_no_stick() {
    for skill in Skill::ALL {
        let f = rest(skill);
        assert_eq!(turn(&f), [0.0, 0.0]);
        assert_eq!(hidden(&f.encode()), [0.0; HIDDEN], "{skill:?}");
        // Inside the dead band the thumb rests too.
        let inside = tolerance(&f) - 0.001;
        assert!(inside > 0.0);
        assert_eq!(yaw(skill, inside), 0.0);
        assert_eq!(yaw(skill, -inside), 0.0);
    }
}

#[test]
fn each_axis_turns_its_own_way() {
    for skill in Skill::ALL {
        let left = turn(&Features {
            error: [0.2, 0.0],
            ..rest(skill)
        });
        assert!(left[0] > 0.3 && left[1] == 0.0, "{left:?}");
        let right = turn(&Features {
            error: [-0.2, 0.0],
            ..rest(skill)
        });
        assert!((right[0] + left[0]).abs() < 1e-6 && right[1] == 0.0);
        let up = turn(&Features {
            error: [0.0, 0.2],
            ..rest(skill)
        });
        assert!(up[1] > 0.3 && up[0] == 0.0, "{up:?}");
        let down = turn(&Features {
            error: [0.0, -0.2],
            ..rest(skill)
        });
        assert!((down[1] + up[1]).abs() < 1e-6 && down[0] == 0.0);
    }
}

#[test]
fn the_turn_saturates_and_never_falls_as_the_error_grows() {
    for skill in Skill::ALL {
        let mut prev = 0.0;
        for i in 0..=320 {
            let now = yaw(skill, i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&now));
            assert!(now >= prev - 1e-6, "{skill:?} at {i}: {now} < {prev}");
            prev = now;
        }
        assert_eq!(prev, 1.0);
        assert_eq!(yaw(skill, -3.0), -1.0);
    }
    // Nothing a caller passes leaves the stick's range or makes a NaN.
    let wild = Features {
        error: [f32::NAN, f32::INFINITY],
        error_rate: [1e9, -1e9],
        target_rate: [f32::NEG_INFINITY, 1e9],
        stick: [9.0, -9.0],
        pending: [f32::NAN, 40.0],
        since_acquired: -3.0,
        distance: 0.0,
        looseness: 7.0,
        gain: 99.0,
        spread: 99.0,
        lead: -2.0,
        noise: [8.0, f32::NAN],
    };
    for v in turn(&wild) {
        assert!((-1.0..=1.0).contains(&v), "{v}");
    }
}

#[test]
fn a_higher_gain_turns_harder_on_the_same_error() {
    let turns = Skill::ALL.map(|s| yaw(s, 0.2));
    assert!(turns.windows(2).all(|w| w[1] > w[0] + 0.05), "{turns:?}");
    assert!((0.45..0.62).contains(&turns[0]), "{turns:?}");
    assert!((0.85..1.0).contains(&turns[3]), "{turns:?}");
    // Far out every skill is at full stick, so only the band differs.
    assert_eq!(Skill::ALL.map(|s| yaw(s, 0.7)), [1.0; 4]);
}

#[test]
fn a_small_error_gets_a_small_correction_past_the_dead_zone() {
    for skill in Skill::ALL {
        let f = rest(skill);
        let edge = tolerance(&f);
        // The stick's own dead zone is 0.13: the correction clears it
        // within about a degree and stays a nudge.
        let nudge = yaw(skill, edge + 0.02);
        assert!((0.15..0.3).contains(&nudge), "{skill:?}: {nudge}");
        let h = hidden(
            &Features {
                error: [edge + 0.03, 0.0],
                ..f
            }
            .encode(),
        );
        assert!(h[unit::YAW_BREAKOUT_CAP] > 0.0);
        assert_eq!(h[unit::YAW_BREAKOUT_CAP + 1], 0.0);
    }
    // A sniper rifle at range has a tighter band than a shotgun up close.
    let far = Features {
        distance: 25.0,
        looseness: 0.0,
        ..rest(Skill::Legendary)
    };
    let near = Features {
        distance: 2.0,
        looseness: 1.0,
        ..rest(Skill::Legendary)
    };
    assert!(tolerance(&far) < 0.003, "{}", tolerance(&far));
    assert!(tolerance(&near) > 3.0 * tolerance(&far));
    // And a wider spread a wider one.
    assert!(tolerance(&rest(Skill::Easy)) > tolerance(&rest(Skill::Legendary)) + 0.004);
}

#[test]
fn it_leads_a_moving_target_as_far_as_the_skill_allows() {
    let lead = |skill, rate: f32| {
        turn(&Features {
            target_rate: [rate, 0.0],
            ..rest(skill)
        })[0]
    };
    for skill in Skill::ALL {
        // On the target but the target is moving: turn with it.
        assert!(lead(skill, 0.3) > 0.1, "{skill:?}");
        assert!((lead(skill, -0.3) + lead(skill, 0.3)).abs() < 1e-6);
    }
    // A fast crossing is past easy's cap and within legendary's.
    assert!(lead(Skill::Easy, 2.0) < lead(Skill::Normal, 2.0) - 0.2);
    assert!(lead(Skill::Normal, 2.0) < lead(Skill::Legendary, 2.0));
    let h = hidden(
        &Features {
            target_rate: [2.0, 0.0],
            ..rest(Skill::Easy)
        }
        .encode(),
    );
    assert!(h[unit::YAW_LEAD_CAP] > 0.0);
    // Pitch leads too, with nothing on yaw.
    let up = turn(&Features {
        target_rate: [0.0, 0.5],
        ..rest(Skill::Heroic)
    });
    assert!(up[1] > 0.1 && up[0] == 0.0, "{up:?}");
}

#[test]
fn it_brakes_for_the_turn_already_on_its_way() {
    for skill in Skill::ALL {
        let blind = Features {
            error: [0.3, 0.0],
            ..rest(skill)
        };
        let aware = Features {
            pending: [0.2, 0.0],
            ..blind
        };
        assert!(turn(&aware)[0] < turn(&blind)[0] - 0.2, "{skill:?}");
        // Well past the target once the pending turn lands: it reverses
        // before the view shows it.
        let past = Features {
            pending: [0.6, 0.0],
            ..blind
        };
        assert!(turn(&past)[0] < -0.2, "{skill:?}");
        // A closing error is damped, an opening one pushed.
        let closing = Features {
            error: [0.1, 0.0],
            error_rate: [-3.0, 0.0],
            ..rest(skill)
        };
        let opening = Features {
            error_rate: [3.0, 0.0],
            ..closing
        };
        let still = yaw(skill, 0.1);
        assert!(turn(&closing)[0] < still - 0.05, "{skill:?}");
        assert!(turn(&opening)[0] > still + 0.05, "{skill:?}");
    }
}

#[test]
fn a_fresh_flick_follows_through_and_a_settled_hand_does_not() {
    let push = |skill, since: f32, stick: f32| {
        turn(&Features {
            error: [0.1, 0.0],
            stick: [stick, 0.0],
            since_acquired: since,
            ..rest(skill)
        })[0]
    };
    for skill in Skill::ALL {
        let settled = push(skill, 1.5, 1.0);
        assert_eq!(settled, yaw(skill, 0.1), "{skill:?}");
        assert!(push(skill, 0.1, 1.0) > settled + 0.3, "{skill:?}");
        // A thumb near the centre is not carried anywhere.
        assert_eq!(push(skill, 0.1, 0.05), settled, "{skill:?}");
    }
    // Easy carries further than legendary, in both directions.
    let carry = |skill| push(skill, 0.1, 1.0) - push(skill, 1.5, 1.0);
    assert!(carry(Skill::Easy) > carry(Skill::Legendary) + 0.15);
    let back = turn(&Features {
        stick: [-1.0, 0.0],
        since_acquired: 0.1,
        ..rest(Skill::Easy)
    });
    assert!(back[0] < -0.3 && back[1] == 0.0, "{back:?}");
}

#[test]
fn the_tables_are_the_documented_shape() {
    assert_eq!(LAYER1.len(), UNIT_ROLES.len());
    assert_eq!(unit::PITCH_LEAD + 2, HIDDEN);
    assert_eq!(input::NOISE + 2, INPUTS);
    // Every unit reads something and feeds exactly one output.
    for (i, row) in LAYER1.iter().enumerate() {
        assert!(row[..INPUTS].iter().any(|w| *w != 0.0), "{}", UNIT_ROLES[i]);
        let fed: Vec<_> = LAYER2.iter().map(|o| o[i] != 0.0).collect();
        let yaw = UNIT_ROLES[i].starts_with("yaw");
        assert_eq!(fed, [yaw, !yaw], "{}", UNIT_ROLES[i]);
    }
    // Mirrored pairs: the second unit is the first with the signed inputs
    // flipped and the skill inputs kept.
    for first in (0..HIDDEN).step_by(2) {
        let (a, b) = (LAYER1[first], LAYER1[first + 1]);
        for i in 0..=INPUTS {
            let signed = i < input::SINCE || (input::NOISE..INPUTS).contains(&i);
            let want = if signed { -a[i] } else { a[i] };
            assert_eq!(b[i], want, "{} input {i}", UNIT_ROLES[first]);
        }
        for o in &LAYER2 {
            assert_eq!(o[first + 1], -o[first]);
        }
    }
    assert_eq!(LAYER2[0][HIDDEN], 0.0);
    assert_eq!(LAYER2[1][HIDDEN], 0.0);
}
