use super::*;

const ONE: u32 = 0x3f80_0000;

fn f(v: f32) -> u32 {
    v.to_bits()
}

fn aim() -> WeaponAim {
    WeaponAim {
        cone_angle: f(0.05),
        cone_range: f(12.0),
        pull_angle: f(0.09),
        pull_range: f(8.0),
        bend_angle: f(0.12),
        class: 1,
        needs_zoom: false,
        zoom: ZoomRange {
            levels: 2,
            min: f(2.0),
            max: f(8.0),
        },
    }
}

/// The x87 half answered from a table: level 0 gives `min`, level 1 `max`.
fn table(max: u32, min: u32, t: u32) -> Option<u32> {
    Some(match t {
        0 => min,
        0x3f80_0000 => max,
        _ => return None,
    })
}

fn input<'a>(zoom: i16, primary: Hand<'a>, secondary: Hand<'a>) -> Input<'a> {
    Input {
        one: ONE,
        zoom,
        hands: Some([primary, secondary]),
    }
}

#[test]
fn the_unarmed_exits_write_nothing() {
    let none = Input {
        one: ONE,
        zoom: 0,
        hands: None,
    };
    assert_eq!(cone(&none, &mut table), Answer::Unarmed(Unarmed::NoUnit));
    assert_eq!(
        cone(&input(0, Hand::NoSlot, Hand::NoSlot), &mut table),
        Answer::Unarmed(Unarmed::NoSlot)
    );
    let empty = input(0, Hand::Empty { index: 1 }, Hand::NoSlot);
    assert_eq!(
        cone(&empty, &mut table),
        Answer::Unarmed(Unarmed::Empty { index: 1 })
    );
    let w = Weapon {
        aim: WeaponAim {
            needs_zoom: true,
            ..aim()
        },
        triggers: &[],
    };
    assert_eq!(
        cone(&input(-1, Hand::Held(w), Hand::NoSlot), &mut table),
        Answer::Unarmed(Unarmed::Unzoomed)
    );
    // With a level asked for, the same weapon is armed.
    assert!(matches!(
        cone(&input(0, Hand::Held(w), Hand::NoSlot), &mut table),
        Answer::Armed { .. }
    ));
}

#[test]
fn one_hand_scales_angles_down_and_ranges_up_by_the_zoom() {
    let w = Weapon {
        aim: aim(),
        triggers: &[],
    };
    let Answer::Armed { cone: c, lanes } = cone(&input(1, Hand::Held(w), Hand::NoSlot), &mut table)
    else {
        panic!("armed")
    };
    // Level 1 of 2 is the greatest magnification, 8.
    let inv = f(1.0 / 8.0);
    assert_eq!(c.cone_angle, f(0.05 * 0.125));
    assert_eq!(c.cone_range, f(96.0));
    assert_eq!(c.pull_angle, f(0.09 * 0.125));
    assert_eq!(c.pull_range, f(64.0));
    assert_eq!(c.bend, f(0.12 * 0.125));
    assert_eq!(
        (
            c.flags,
            c.flagged_range,
            c.second_cone_angle,
            c.second_cone_range
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        lanes,
        Lanes([
            Lane::Whole(0),
            Lane::Whole(inv),
            Lane::Whole(f(64.0)),
            Lane::Low(f(1.0))
        ])
    );
}

#[test]
fn outside_the_levels_the_magnification_is_one_and_no_lane_changes() {
    let w = Weapon {
        aim: aim(),
        triggers: &[],
    };
    for level in [-1, 2, 7] {
        let Answer::Armed { cone: c, lanes } = cone(
            &input(level, Hand::Held(w), Hand::NoSlot),
            &mut |_, _, _| None,
        ) else {
            panic!("armed without the x87")
        };
        assert_eq!((c.cone_angle, c.cone_range), (f(0.05), f(12.0)));
        assert_eq!(lanes.0[3], Lane::Kept);
    }
}

#[test]
fn a_zoom_bound_not_above_zero_reads_as_one() {
    let mut seen = Vec::new();
    let w = Weapon {
        aim: WeaponAim {
            zoom: ZoomRange {
                levels: 1,
                min: f(-3.0),
                max: 0,
            },
            ..aim()
        },
        triggers: &[],
    };
    let _ = cone(
        &input(0, Hand::Held(w), Hand::NoSlot),
        &mut |max, min, t| {
            seen.push((max, min, t));
            Some(ONE)
        },
    );
    // One level: t is 0 and no division by levels - 1.
    assert_eq!(seen, [(ONE, ONE, 0)]);
}

#[test]
fn the_triggers_set_the_flags_by_class() {
    let idle_tracker = [Trigger {
        state: 0,
        kind: Some(5),
    }];
    let acquiring = [
        Trigger {
            state: 0,
            kind: Some(1),
        },
        Trigger {
            state: 6,
            kind: None,
        },
    ];
    let charging = [Trigger {
        state: 2,
        kind: None,
    }];
    let case = |class: u16, triggers: &[Trigger]| {
        let w = Weapon {
            aim: WeaponAim { class, ..aim() },
            triggers,
        };
        match cone(&input(0, Hand::Held(w), Hand::NoSlot), &mut table) {
            Answer::Armed { cone, .. } => (cone.flags, cone.flagged_range == cone.cone_range),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(case(1, &idle_tracker), (TRACKING, true));
    // Tracking wins over acquisition; a tracking class-2 weapon has no flag.
    assert_eq!(case(2, &idle_tracker).0, 0);
    assert_eq!(case(1, &acquiring), (ACQUIRED_CLASS_1, true));
    assert_eq!(case(2, &acquiring), (ACQUIRED_CLASS_2, true));
    assert_eq!(case(3, &acquiring).0, 0);
    assert_eq!(case(1, &charging).0, ACQUIRED_CLASS_1);
    assert_eq!(case(0, &charging).0, 0, "state 2 needs a class");
    assert_eq!(case(1, &[]).0, 0);
}

#[test]
fn two_hands_keep_the_larger_pull_and_bend_and_the_seconds_own_cone() {
    let first = Weapon {
        aim: aim(),
        triggers: &[],
    };
    let second = Weapon {
        aim: WeaponAim {
            cone_angle: f(0.2),
            cone_range: f(5.0),
            pull_angle: f(0.3),
            pull_range: f(1.0),
            bend_angle: f(0.01),
            zoom: ZoomRange {
                levels: 0,
                min: 0,
                max: 0,
            },
            ..aim()
        },
        triggers: &[],
    };
    let Answer::Armed { cone: c, lanes } =
        cone(&input(0, Hand::Held(first), Hand::Held(second)), &mut table)
    else {
        panic!("armed")
    };
    // The first at magnification 2; the second has no levels, so 1.
    assert_eq!((c.second_cone_angle, c.second_cone_range), (f(0.2), f(5.0)));
    assert_eq!(c.pull_angle, f(0.3), "the second's wider pull");
    assert_eq!(c.pull_range, f(16.0), "the first's longer reach");
    assert_eq!(c.bend, f(0.2), "the second's cone is wider than its bend");
    assert_eq!(
        lanes,
        Lanes([
            Lane::Whole(f(0.2)),
            Lane::Whole(f(0.06)),
            Lane::Whole(ONE),
            Lane::Whole(f(0.045))
        ])
    );
}

#[test]
fn a_tie_is_the_second_hands() {
    let first = Weapon {
        aim: aim(),
        triggers: &[],
    };
    let second = Weapon {
        aim: WeaponAim {
            pull_angle: f(-0.0),
            ..aim()
        },
        triggers: &[],
    };
    let mut w = first;
    w.aim.pull_angle = 0;
    let Answer::Armed { cone: c, .. } =
        cone(&input(-1, Hand::Held(w), Hand::Held(second)), &mut table)
    else {
        panic!("armed")
    };
    assert_eq!(c.pull_angle, f(-0.0));
}

#[test]
fn an_operand_off_the_exact_range_is_inexact() {
    let nan = Weapon {
        aim: WeaponAim {
            bend_angle: f32::NAN.to_bits(),
            ..aim()
        },
        triggers: &[],
    };
    assert_eq!(
        cone(&input(0, Hand::Held(nan), Hand::NoSlot), &mut table),
        Answer::Inexact
    );
    let tiny = Weapon {
        aim: WeaponAim {
            cone_range: 1,
            ..aim()
        },
        triggers: &[],
    };
    assert_eq!(
        cone(&input(0, Hand::Held(tiny), Hand::NoSlot), &mut table),
        Answer::Inexact
    );
    // The x87 half refusing is inexact too.
    let w = Weapon {
        aim: aim(),
        triggers: &[],
    };
    assert_eq!(
        cone(&input(0, Hand::Held(w), Hand::NoSlot), &mut |_, _, _| None),
        Answer::Inexact
    );
}
