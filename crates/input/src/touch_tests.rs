use super::*;
use crate::host::ConvertConfig;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn frame(buttons: u32) -> TouchFrame {
    TouchFrame {
        buttons,
        ..TouchFrame::default()
    }
}

#[test]
fn move_stick_has_a_rescaled_radial_deadzone_and_flips_y() {
    assert_eq!(move_stick((0.0, 0.0), 50.0, 0.1), (0.0, 0.0));
    assert_eq!(
        move_stick((4.0, 0.0), 50.0, 0.1),
        (0.0, 0.0),
        "inside the deadzone"
    );
    // Just past the deadzone starts near zero, not at 0.1.
    let (x, _) = move_stick((6.0, 0.0), 50.0, 0.1);
    assert!(x > 0.0 && x < 0.05, "{x}");
    // Half way through the live range.
    let (x, y) = move_stick((27.5, 0.0), 50.0, 0.1);
    assert!(close(x, 0.5) && y == 0.0, "{x} {y}");
    // Thumb dragged up the screen (-y) pushes the stick up (+y).
    let (_, y) = move_stick((0.0, -50.0), 50.0, 0.1);
    assert!(close(y, 1.0), "{y}");
    // Past the radius: full deflection, direction kept.
    let (x, y) = move_stick((300.0, 300.0), 50.0, 0.1);
    assert!(close(x.hypot(y), 1.0) && close(x, -y), "{x} {y}");
    // Garbage in, centred out.
    assert_eq!(move_stick((f32::NAN, 1.0), 50.0, 0.1), (0.0, 0.0));
    assert_eq!(move_stick((10.0, 0.0), 0.0, 0.1), (0.0, 0.0));
}

#[test]
fn look_gain_grows_with_speed_up_to_a_cap() {
    assert_eq!(look_gain(0.0, 0.6), 1.0);
    assert!((look_gain(ACCEL_SPEED, 0.6) - 1.6).abs() < 1e-9);
    assert!((look_gain(ACCEL_SPEED * 10.0, 0.6) - (1.0 + 0.6 * ACCEL_CAP)).abs() < 1e-9);
    assert_eq!(look_gain(1e9, 0.0), 1.0, "no acceleration");
    assert_eq!(look_gain(f64::NAN, 0.6), 1.0);
}

#[test]
fn the_curve_lifts_small_deflections_and_keeps_direction() {
    let (x, y) = curve((0.25, 0.0), 0.5);
    assert!((x - 0.5).abs() < 1e-9 && y == 0.0);
    let (x, y) = curve((0.3, 0.4), 1.0);
    assert!((x - 0.3).abs() < 1e-9 && (y - 0.4).abs() < 1e-9, "linear");
    let (x, y) = curve((3.0, 4.0), 0.75);
    assert!(
        (x.hypot(y) - 1.0).abs() < 1e-9 && (x / y - 0.75).abs() < 1e-9,
        "clamped"
    );
    assert_eq!(curve((0.0, 0.0), 0.75), (0.0, 0.0));
    assert_eq!(curve((f64::NAN, 0.0), 0.75), (0.0, 0.0));
}

#[test]
fn gyro_is_a_rate_and_off_at_zero_sensitivity() {
    assert_eq!(gyro_stick((1.0, 1.0), 0.0), (0.0, 0.0));
    let (x, y) = gyro_stick((1.5, -0.75), 1.0);
    assert!((x - 0.5).abs() < 1e-6 && (y + 0.25).abs() < 1e-6);
    assert_eq!(gyro_stick((100.0, f32::NAN), 1.0), (1.0, 0.0));
}

#[test]
fn every_bit_maps_to_its_pad_control() {
    let t0 = Instant::now();
    let mut p = TouchPad::new(TouchTuning {
        crouch: CrouchMode::Hold,
        ..TouchTuning::default()
    });
    p.update(&frame(bits::ALL), t0);
    let g = p.state(t0).to_gamepad(&ConvertConfig::default(), false);
    use crate::gamepad::{AnalogButton, Buttons};
    for b in [
        AnalogButton::A,
        AnalogButton::B,
        AnalogButton::X,
        AnalogButton::Y,
        AnalogButton::Black,
        AnalogButton::White,
        AnalogButton::LeftTrigger,
        AnalogButton::RightTrigger,
    ] {
        assert_eq!(g.analog[b.index()], 255, "{b:?}");
    }
    for b in [
        Buttons::START,
        Buttons::BACK,
        Buttons::LEFT_THUMB,
        Buttons::RIGHT_THUMB,
    ] {
        assert!(g.buttons.contains(b), "{b:?}");
    }
    // Opposite d-pad directions cancel, as on any pad.
    assert!(!g.buttons.contains(Buttons::DPAD_UP));
    p.update(&frame(bits::DPAD_UP | bits::DPAD_LEFT), t0);
    let g = p.state(t0).to_gamepad(&ConvertConfig::default(), false);
    assert!(g.buttons.contains(Buttons::DPAD_UP) && g.buttons.contains(Buttons::DPAD_LEFT));
    assert_eq!(g.analog[AnalogButton::RightTrigger.index()], 0);
}

#[test]
fn single_bits_are_single_controls() {
    use HostButton::*;
    for (bit, hb) in [
        (bits::A, South),
        (bits::B, East),
        (bits::X, West),
        (bits::Y, North),
        (bits::BLACK, RightBumper),
        (bits::WHITE, LeftBumper),
        (bits::START, Start),
        (bits::BACK, Back),
        (bits::RIGHT_THUMB, RightStick),
        (bits::DPAD_DOWN, DpadDown),
        (bits::DPAD_RIGHT, DpadRight),
    ] {
        let mut want = HostButtons::default();
        want.set(hb, true);
        assert_eq!(buttons(bit), want, "{hb:?}");
    }
    assert_eq!(buttons(0), HostButtons::default());
    assert_eq!(
        buttons(bits::LEFT_TRIGGER | bits::RIGHT_TRIGGER),
        HostButtons::default()
    );
}

#[test]
fn fire_is_a_full_right_trigger_while_held() {
    let t0 = Instant::now();
    let mut p = TouchPad::default();
    p.update(&frame(bits::RIGHT_TRIGGER), t0);
    let s = p.state(t0);
    assert_eq!((s.left_trigger, s.right_trigger), (0.0, 1.0));
    p.update(&frame(bits::LEFT_TRIGGER), t0);
    let s = p.state(t0);
    assert_eq!((s.left_trigger, s.right_trigger), (1.0, 0.0));
    p.update(&frame(0), t0);
    assert_eq!(p.state(t0).right_trigger, 0.0);
}

#[test]
fn crouch_toggles_on_each_tap() {
    let t0 = Instant::now();
    let mut p = TouchPad::default();
    assert_eq!(p.tuning().crouch, CrouchMode::Toggle, "toggle by default");
    let crouched = |p: &mut TouchPad| p.state(t0).buttons.contains(HostButton::LeftStick);
    p.update(&frame(bits::LEFT_THUMB), t0);
    assert!(crouched(&mut p));
    p.update(&frame(bits::LEFT_THUMB), t0);
    assert!(crouched(&mut p), "still the same press");
    p.update(&frame(0), t0);
    assert!(crouched(&mut p), "stays crouched after letting go");
    p.update(&frame(bits::LEFT_THUMB | bits::A), t0);
    assert!(!crouched(&mut p), "the second tap stands");
    p.update(&frame(0), t0);
    assert!(!crouched(&mut p));
    // Hiding the controls stands a toggled crouch up.
    p.update(&frame(bits::LEFT_THUMB), t0);
    p.release();
    assert!(!crouched(&mut p) && !p.active());
}

#[test]
fn crouch_can_be_held_instead() {
    let t0 = Instant::now();
    let mut p = TouchPad::default();
    p.update(&frame(bits::LEFT_THUMB), t0);
    p.update(&frame(0), t0);
    assert!(p.crouch_down());
    // Switching to hold stands the latched crouch up.
    p.set_tuning(TouchTuning {
        crouch: CrouchMode::Hold,
        ..p.tuning()
    });
    assert!(!p.crouch_down());
    p.update(&frame(bits::LEFT_THUMB), t0);
    assert!(p.crouch_down());
    p.update(&frame(0), t0);
    assert!(!p.crouch_down());
}

#[test]
fn look_drags_turn_at_their_speed_until_read() {
    let t0 = Instant::now();
    let tune = TouchTuning {
        look_acceleration: 0.0,
        look_curve: 1.0,
        ..TouchTuning::default()
    };
    let mut p = TouchPad::new(tune);
    // A 2-point drag right and up within one 1/60 s frame.
    p.update(
        &TouchFrame {
            look: (2.0, -1.0),
            ..TouchFrame::default()
        },
        t0,
    );
    let s = p.state(t0);
    assert!(
        close(s.right_stick.0, 0.2) && close(s.right_stick.1, 0.1),
        "{:?}",
        s.right_stick
    );
    // Still there until the guest reads it.
    assert_eq!(p.state(t0).right_stick, s.right_stick);
    p.read(t0);
    assert_eq!(p.state(t0).right_stick, (0.0, 0.0));
    assert!(!p.active());
}

#[test]
fn look_jitter_is_dropped_and_inversion_flips_y() {
    let t0 = Instant::now();
    let mut p = TouchPad::default();
    p.update(
        &TouchFrame {
            look: (0.1, 0.1),
            ..TouchFrame::default()
        },
        t0,
    );
    assert_eq!(p.state(t0).right_stick, (0.0, 0.0));
    let mut p = TouchPad::new(TouchTuning {
        invert_y: true,
        ..TouchTuning::default()
    });
    p.update(
        &TouchFrame {
            look: (0.0, -3.0),
            ..TouchFrame::default()
        },
        t0,
    );
    assert!(p.state(t0).right_stick.1 < 0.0, "dragging up looks down");
}

#[test]
fn faster_drags_and_more_sensitivity_turn_faster_but_never_past_full() {
    let t0 = Instant::now();
    let look = |tune: TouchTuning, d: f32| {
        let mut p = TouchPad::new(tune);
        p.update(
            &TouchFrame {
                look: (d, 0.0),
                ..TouchFrame::default()
            },
            t0,
        );
        p.state(t0).right_stick.0
    };
    let base = TouchTuning::default();
    let slow = look(base, 1.0);
    let fast = look(base, 3.0);
    assert!(slow > 0.0 && fast > slow);
    let sensitive = look(
        TouchTuning {
            look_sensitivity: 2.0,
            ..base
        },
        1.0,
    );
    assert!(sensitive > slow);
    assert!(close(look(base, 500.0), 1.0));
    // The curve lifts a slow drag above linear.
    let linear = look(
        TouchTuning {
            look_curve: 1.0,
            ..base
        },
        1.0,
    );
    assert!(slow > linear, "{slow} {linear}");
}

#[test]
fn gyro_adds_to_look_only_when_on() {
    let t0 = Instant::now();
    let f = TouchFrame {
        gyro: (1.5, 0.0),
        ..TouchFrame::default()
    };
    let mut off = TouchPad::default();
    off.update(&f, t0);
    assert_eq!(off.state(t0).right_stick, (0.0, 0.0));
    let mut on = TouchPad::new(TouchTuning {
        gyro_sensitivity: 1.0,
        ..TouchTuning::default()
    });
    on.update(&f, t0);
    assert!(close(on.state(t0).right_stick.0, 0.5));
    assert!(on.active());
}

#[test]
fn the_move_stick_follows_the_thumb_and_centres_on_lift() {
    let t0 = Instant::now();
    let mut p = TouchPad::default();
    p.update(
        &TouchFrame {
            move_offset: Some((0.0, -200.0)),
            ..TouchFrame::default()
        },
        t0,
    );
    let s = p.state(t0);
    assert!(close(s.left_stick.1, 1.0) && s.left_stick.0 == 0.0);
    assert!(s.prefiltered, "no second deadzone downstream");
    p.update(&TouchFrame::default(), t0);
    assert_eq!(p.state(t0).left_stick, (0.0, 0.0));
}

#[test]
fn the_shown_flag_is_shared() {
    set_controls_shown(true);
    assert!(controls_shown());
    set_controls_shown(false);
    assert!(!controls_shown());
}

#[test]
fn the_log_reports_button_changes_at_once_and_sticks_slowly() {
    let t0 = Instant::now();
    let mut log = ChangeLog::default();
    let mut s = HostPadState::default();
    assert_eq!(log.line(&s, t0).as_deref(), Some("idle"));
    assert_eq!(log.line(&s, t0), None, "unchanged");
    s.right_trigger = 1.0;
    s.buttons.set(HostButton::LeftStick, true);
    assert_eq!(log.line(&s, t0).as_deref(), Some("LS RT"));
    s.left_stick = (0.0, 0.98);
    assert_eq!(
        log.line(&s, t0 + Duration::from_millis(10)),
        None,
        "too soon for a stick"
    );
    assert_eq!(
        log.line(&s, t0 + LOG_EVERY).as_deref(),
        Some("LS RT move(+0.00,+1.00)")
    );
    s.left_stick = (0.0, 0.99);
    assert_eq!(log.line(&s, t0 + LOG_EVERY * 3), None, "same quarter");
    s.buttons.set(HostButton::South, true);
    assert_eq!(
        log.line(&s, t0 + LOG_EVERY * 3).as_deref(),
        Some("A LS RT move(+0.00,+1.00)")
    );
}
