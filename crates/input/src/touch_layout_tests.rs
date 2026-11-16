use super::*;

fn on_foot() -> Context {
    Context {
        seat: Seat::OnFoot,
        ..Context::default()
    }
}

#[test]
fn menus_get_the_whole_plain_set() {
    let p = plan(&Context::default());
    assert_eq!(p.layout, Layout::OnFoot);
    assert_eq!(p.visible, ON_FOOT);
    assert_eq!((p.dimmed, p.flags), (0, 0));
    assert_eq!(p.stick_radius, STICK_RADIUS);
    // Even with stale weapon facts: no game, nothing dimmed.
    let p = plan(&Context {
        dual: true,
        on_target: true,
        ..Context::default()
    });
    assert_eq!((p.visible, p.dimmed, p.flags), (ON_FOOT, 0, 0));
}

#[test]
fn on_foot_dims_zoom_without_a_scope_and_moves_it_with_one() {
    let p = plan(&on_foot());
    assert_eq!(p.visible, ON_FOOT);
    assert_eq!(p.dimmed, bits::RIGHT_THUMB, "no scope: Zoom does nothing");
    assert_eq!(p.flags, 0);
    let p = plan(&Context {
        zoom: true,
        ..on_foot()
    });
    assert_eq!(p.dimmed, 0);
    assert_eq!(p.flags, flags::ZOOM_NEAR_FIRE);
    for b in [bits::A, bits::LEFT_THUMB, bits::LEFT_TRIGGER, bits::B] {
        assert_ne!(p.visible & b, 0, "jump, crouch, grenade and melee on foot");
    }
}

#[test]
fn dual_wielding_gives_the_left_a_trigger_and_drops_zoom() {
    let p = plan(&Context {
        dual: true,
        zoom: true,
        ..on_foot()
    });
    assert_eq!(p.flags & flags::DUAL, flags::DUAL);
    assert_eq!(p.flags & flags::ZOOM_NEAR_FIRE, 0);
    assert_eq!(p.visible & bits::RIGHT_THUMB, 0, "no zoom dual-wielding");
    assert_ne!(p.visible & bits::LEFT_TRIGGER, 0, "the left gun's trigger");
    assert_eq!(p.dimmed, bits::BLACK, "no grenades to swap");
}

#[test]
fn driving_drops_crouch_and_jump_and_widens_the_stick() {
    let p = plan(&Context {
        seat: Seat::Driver,
        zoom: true,
        dual: true,
        ..Context::default()
    });
    assert_eq!(p.layout, Layout::Driver);
    assert_eq!(p.visible, DRIVER);
    for b in [bits::A, bits::LEFT_THUMB, bits::B, bits::Y, bits::BLACK] {
        assert_eq!(p.visible & b, 0, "{b:#x} is not drawn while driving");
    }
    for b in [bits::LEFT_TRIGGER, bits::X, bits::RIGHT_TRIGGER] {
        assert_ne!(p.visible & b, 0, "boost, exit and fire are");
    }
    assert!(p.stick_radius > STICK_RADIUS);
    assert_eq!(p.flags & (flags::DUAL | flags::ZOOM_NEAR_FIRE), 0);
}

#[test]
fn passengers_and_gunners_get_the_turret_set() {
    let p = plan(&Context {
        seat: Seat::Passenger,
        on_target: true,
        ..Context::default()
    });
    assert_eq!(p.layout, Layout::Turret);
    assert_eq!(p.visible, TURRET);
    assert_eq!(p.flags, flags::ON_TARGET);
    assert_eq!(
        p.visible & (bits::A | bits::LEFT_THUMB | bits::LEFT_TRIGGER),
        0
    );
    assert_eq!(p.stick_radius, STICK_RADIUS);
}

#[test]
fn every_layout_keeps_start_back_and_fire() {
    for seat in [Seat::None, Seat::OnFoot, Seat::Driver, Seat::Passenger] {
        let p = plan(&Context {
            seat,
            ..Context::default()
        });
        let must = bits::START | bits::BACK | bits::RIGHT_TRIGGER;
        assert_eq!(p.visible & must, must, "{seat:?}");
        assert_eq!(p.dimmed & !p.visible, 0, "only drawn buttons are dimmed");
    }
}

// ---- auto-fire ----

fn aimed(trigger: Trigger) -> Context {
    Context {
        trigger,
        on_target: true,
        ..on_foot()
    }
}

#[test]
fn auto_fire_needs_the_setting_a_red_reticle_and_a_seat_that_shoots() {
    let ctx = aimed(Trigger::Automatic);
    assert_eq!(fire_mode(true, &ctx), FireMode::Hold);
    assert_eq!(fire_mode(false, &ctx), FireMode::Off, "off by default");
    let off_target = Context {
        on_target: false,
        ..ctx
    };
    assert_eq!(fire_mode(true, &off_target), FireMode::Off);
    for seat in [Seat::None, Seat::Driver] {
        assert_eq!(
            fire_mode(true, &Context { seat, ..ctx }),
            FireMode::Off,
            "{seat:?}"
        );
    }
    let gunner = Context {
        seat: Seat::Passenger,
        ..ctx
    };
    assert_eq!(fire_mode(true, &gunner), FireMode::Hold);
}

#[test]
fn auto_fire_respects_the_weapon() {
    assert_eq!(
        fire_mode(true, &aimed(Trigger::Melee)),
        FireMode::Off,
        "no sword lunges"
    );
    assert_eq!(
        fire_mode(true, &aimed(Trigger::SemiAuto { rate: 5.0 })),
        FireMode::Pulse { period: 6 }
    );
    assert_eq!(
        fire_mode(true, &aimed(Trigger::SemiAuto { rate: 30.0 })),
        FireMode::Pulse { period: 2 },
        "always released for a read between pulls"
    );
    assert_eq!(
        fire_mode(true, &aimed(Trigger::Charge { rate: 6.0 })),
        FireMode::Pulse { period: 5 },
        "a charging weapon is tapped, never held"
    );
    let unknown = FireMode::Pulse {
        period: (READS_PER_SECOND / DEFAULT_RATE).round() as u32,
    };
    assert_eq!(fire_mode(true, &aimed(Trigger::Unknown)), unknown);
    assert_eq!(
        fire_mode(true, &aimed(Trigger::SemiAuto { rate: f32::NAN })),
        unknown
    );
}

#[test]
fn a_pulse_pulls_once_per_period_of_reads() {
    let mode = FireMode::Pulse { period: 3 };
    let mut a = AutoFire::default();
    let mut seen = Vec::new();
    for _ in 0..9 {
        seen.push(a.pulled(mode));
        a.read(mode);
    }
    assert_eq!(
        seen,
        [true, false, false, true, false, false, true, false, false]
    );
    // Off target resets: the next time on target pulls at once.
    a.read(FireMode::Off);
    assert!(a.pulled(mode));
    assert!(a.pulled(FireMode::Hold));
    assert!(!a.pulled(FireMode::Off));
}

#[test]
fn publishing_reports_layout_changes_once() {
    // The only test that touches the shared context.
    let base = generation();
    let driving = Context {
        seat: Seat::Driver,
        ..Context::default()
    };
    publish(Context::default());
    assert_eq!(publish(driving), Some((Layout::OnFoot, Layout::Driver)));
    assert_eq!(publish(driving), None, "no change, no line");
    assert_eq!(context(), driving);
    let g1 = generation();
    assert!(g1 != base);
    assert_eq!(
        publish(Context {
            on_target: true,
            ..driving
        }),
        None,
        "same layout"
    );
    assert_ne!(generation(), g1, "but the plan's flags changed");
    publish(Context::default());
}
