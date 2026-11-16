//! The Steam Controller's trackpads and the rest of its state, as the Xbox
//! pad the game sees.
//!
//! * **Right trackpad: aim.** Finger travel between reports is relative
//!   motion, like a mouse: [`TrackpadAim`] turns it into look units, with a
//!   small deadzone against jitter, a sensitivity and an acceleration curve,
//!   and `steam_pad` feeds them into [`crate::look::LookMotion`], the path
//!   mouse look takes. The pad's position is never read as a stick
//!   deflection: lifting the thumb stops the turn, and moving it turns at
//!   the speed it moves.
//! * **Left trackpad**: the d-pad on a click (the direction from where the
//!   thumb is), or a second movement stick ([`LeftPadMode`]), the two ways
//!   Steam's own gamepad template offers.
//! * Buttons, sticks and triggers map onto the standard layout in
//!   [`to_host`]. See `docs/steam-controller.md`.

use std::time::{Duration, Instant};

use crate::host::{HostButton, HostPadState};
use crate::steam::{Buttons, Model, PadState, Touch};

/// What the left trackpad does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeftPadMode {
    /// A click is a d-pad press toward where the thumb is (Steam's default).
    #[default]
    Dpad,
    /// The thumb's position is a stick: it moves, as the left stick does,
    /// and a click is the left stick click.
    Stick,
}

/// Right-trackpad aim settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimTuning {
    /// Multiplies every motion (the Steam Pad Sensitivity setting, 5 of 10
    /// is 1.0).
    pub sensitivity: f64,
    /// Extra gain at speed: the gain is `1 + acceleration * min(speed /
    /// ACCEL_SPEED, ACCEL_CAP)`.
    pub acceleration: f64,
    /// Travel per report below this many pad units is jitter and dropped;
    /// above it, this much is taken off so motion starts smoothly.
    pub deadzone: f64,
}

impl Default for AimTuning {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            acceleration: 0.5,
            deadzone: 24.0,
        }
    }
}

/// The trackpad speed (pad units per second; the pad is 65536 units across)
/// at which acceleration has added `acceleration` to the gain.
pub const ACCEL_SPEED: f64 = 100_000.0;
/// Acceleration stops growing at this many times [`ACCEL_SPEED`].
pub const ACCEL_CAP: f64 = 2.0;
/// Look deflection per look unit per 1/60 s ([`crate::look::LOOK_FRAME`]):
/// half the pad in a quarter of a second turns at about 0.7 of full speed
/// at sensitivity 1.
pub const LOOK_K: f64 = 1.0 / 3000.0;
/// A jump this far between two reports is a lost lift and a new touch,
/// not motion.
pub const MAX_JUMP: f64 = 16_384.0;
/// The report interval assumed for the speed when reports are further
/// apart than this (a pause) or closer than [`MIN_INTERVAL`].
pub const MAX_INTERVAL: Duration = Duration::from_millis(50);
pub const MIN_INTERVAL: Duration = Duration::from_millis(1);

/// The Steam Pad Sensitivity setting (1 to 10) as a multiplier: 5 is 1.0.
pub fn sensitivity_multiplier(setting: u8) -> f64 {
    f64::from(setting.clamp(1, 10)) / 5.0
}

/// Relative motion from the right trackpad.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackpadAim {
    last: Option<(i16, i16, Instant)>,
}

impl TrackpadAim {
    /// Feeds one report's touch. Returns look units in screen space (+x
    /// right, +y down, as mouse counts are): zero on the first report of a
    /// touch, after a lift, or within the deadzone.
    pub fn update(&mut self, t: Touch, now: Instant, tune: &AimTuning) -> (f64, f64) {
        if !t.touched {
            self.last = None;
            return (0.0, 0.0);
        }
        let Some((lx, ly, at)) = self.last.replace((t.x, t.y, now)) else {
            return (0.0, 0.0);
        };
        let dx = f64::from(t.x) - f64::from(lx);
        // Pad +y is up; screen +y is down.
        let dy = f64::from(ly) - f64::from(t.y);
        let mag = dx.hypot(dy);
        if mag <= tune.deadzone || mag > MAX_JUMP {
            return (0.0, 0.0);
        }
        let dt = now
            .saturating_duration_since(at)
            .clamp(MIN_INTERVAL, MAX_INTERVAL)
            .as_secs_f64();
        let speed = mag / dt;
        let gain = 1.0 + tune.acceleration.max(0.0) * (speed / ACCEL_SPEED).min(ACCEL_CAP);
        let scale = (mag - tune.deadzone) / mag * gain * tune.sensitivity.max(0.0);
        (dx * scale, dy * scale)
    }
}

/// -32768..=32767 to -1..=1.
fn axis(v: i16) -> f32 {
    f32::from(v) / if v < 0 { 32768.0 } else { 32767.0 }
}

/// A radial deadzone with the live range rescaled to 0..1.
fn radial(x: f32, y: f32, dz: f32) -> (f32, f32) {
    let mag = x.hypot(y);
    if mag <= dz || mag == 0.0 {
        return (0.0, 0.0);
    }
    let f = ((mag.min(1.0) - dz) / (1.0 - dz)) / mag;
    (x * f, y * f)
}

/// Stick noise cut on the physical sticks (as `host::ConvertConfig`'s
/// default); the state is reported `prefiltered` so look motion added to
/// the right stick does not fall in a second one.
pub const STICK_DEADZONE: f32 = 0.08;
/// Where on the left pad a click is no direction (its centre), and where a
/// touch starts moving the stick in [`LeftPadMode::Stick`].
pub const PAD_CENTRE: f32 = 0.25;
/// A direction is pressed while the thumb is within 67.5 degrees of it, so
/// the diagonals press two (Steam's "four way overlap").
const DPAD_OVERLAP: f32 = 0.382_683_4; // sin 22.5 degrees

/// The d-pad a left-pad click gives: (up, down, left, right).
pub fn dpad_from_pad(t: Touch) -> (bool, bool, bool, bool) {
    if !t.clicked {
        return (false, false, false, false);
    }
    let (x, y) = (axis(t.x), axis(t.y));
    let mag = x.hypot(y);
    if mag < PAD_CENTRE {
        return (false, false, false, false);
    }
    let (nx, ny) = (x / mag, y / mag);
    (
        ny > DPAD_OVERLAP,
        ny < -DPAD_OVERLAP,
        nx < -DPAD_OVERLAP,
        nx > DPAD_OVERLAP,
    )
}

/// The left pad as a stick: the thumb's position, its centre a deadzone.
pub fn stick_from_pad(t: Touch) -> (f32, f32) {
    if !t.touched {
        return (0.0, 0.0);
    }
    radial(axis(t.x), axis(t.y), PAD_CENTRE)
}

/// The controller's state as a standard pad, without the look motion
/// (`steam_pad` adds that to the right stick).
///
/// * Face buttons, bumpers, Back and Start as labelled; the right bumper is
///   Black and the left White, as for every pad (`host::to_gamepad`).
/// * Triggers: the analog value, or full while the trigger's click is down.
/// * Left stick click: the stick click, the left grip (or L4/L5) and, in
///   stick mode, a left-pad click. Right stick click: the 2026
///   controller's stick click, a right-pad click and the right grip (or
///   R4/R5).
/// * D-pad: the 2026 controller's own d-pad; in d-pad mode, a left-pad
///   click toward a direction (and the 2015 controller's own d-pad bits,
///   which its firmware derives from the same pad).
/// * Steam and the quick access button are left to Steam.
pub fn to_host(model: Model, s: &PadState, mode: LeftPadMode) -> HostPadState {
    let b = s.buttons;
    let mut h = HostPadState::default();
    let set = |h: &mut HostPadState, hb: HostButton, on: bool| {
        if on {
            h.buttons.set(hb, true)
        }
    };
    set(&mut h, HostButton::South, b.has(Buttons::A));
    set(&mut h, HostButton::East, b.has(Buttons::B));
    set(&mut h, HostButton::West, b.has(Buttons::X));
    set(&mut h, HostButton::North, b.has(Buttons::Y));
    set(&mut h, HostButton::LeftBumper, b.has(Buttons::LB));
    set(&mut h, HostButton::RightBumper, b.has(Buttons::RB));
    set(&mut h, HostButton::Back, b.has(Buttons::BACK));
    set(&mut h, HostButton::Start, b.has(Buttons::START));
    let stick_mode = mode == LeftPadMode::Stick;
    set(
        &mut h,
        HostButton::LeftStick,
        b.has(Buttons::L3) || b.has(Buttons::LGRIP) || (stick_mode && s.left_pad.clicked),
    );
    set(
        &mut h,
        HostButton::RightStick,
        b.has(Buttons::R3) || b.has(Buttons::RGRIP) || s.right_pad.clicked,
    );
    let own_dpad = model == Model::Triton || !stick_mode;
    let (pu, pd, pl, pr) = if stick_mode {
        (false, false, false, false)
    } else {
        dpad_from_pad(s.left_pad)
    };
    set(
        &mut h,
        HostButton::DpadUp,
        pu || own_dpad && b.has(Buttons::DPAD_UP),
    );
    set(
        &mut h,
        HostButton::DpadDown,
        pd || own_dpad && b.has(Buttons::DPAD_DOWN),
    );
    set(
        &mut h,
        HostButton::DpadLeft,
        pl || own_dpad && b.has(Buttons::DPAD_LEFT),
    );
    set(
        &mut h,
        HostButton::DpadRight,
        pr || own_dpad && b.has(Buttons::DPAD_RIGHT),
    );
    let full = |v: f32, click: bool| if click { 1.0 } else { v.clamp(0.0, 1.0) };
    h.left_trigger = full(s.left_trigger, b.has(Buttons::LT_FULL));
    h.right_trigger = full(s.right_trigger, b.has(Buttons::RT_FULL));
    let (lx, ly) = s.left_stick;
    h.left_stick = radial(axis(lx), axis(ly), STICK_DEADZONE);
    if stick_mode {
        let pad = stick_from_pad(s.left_pad);
        if pad.0.hypot(pad.1) > h.left_stick.0.hypot(h.left_stick.1) {
            h.left_stick = pad;
        }
    }
    if let Some((rx, ry)) = s.right_stick {
        h.right_stick = radial(axis(rx), axis(ry), STICK_DEADZONE);
    }
    h.prefiltered = true;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(x: i16, y: i16) -> Touch {
        Touch {
            touched: true,
            clicked: false,
            x,
            y,
        }
    }

    const NO_ACCEL: AimTuning = AimTuning {
        sensitivity: 1.0,
        acceleration: 0.0,
        deadzone: 24.0,
    };

    #[test]
    fn the_first_touch_and_a_lift_give_no_motion() {
        let t0 = Instant::now();
        let mut a = TrackpadAim::default();
        assert_eq!(a.update(touch(1000, 1000), t0, &NO_ACCEL), (0.0, 0.0));
        let ms = |n| t0 + Duration::from_millis(n);
        let (dx, dy) = a.update(touch(1524, 1000), ms(8), &NO_ACCEL);
        assert!((dx - 500.0).abs() < 1e-9 && dy == 0.0, "{dx} {dy}");
        // Lifted, then touched somewhere else: no jump.
        assert_eq!(a.update(Touch::default(), ms(16), &NO_ACCEL), (0.0, 0.0));
        assert_eq!(
            a.update(touch(-20_000, 9_000), ms(24), &NO_ACCEL),
            (0.0, 0.0)
        );
        // A jump past MAX_JUMP without a lift is dropped too.
        assert_eq!(
            a.update(touch(20_000, 9_000), ms(32), &NO_ACCEL),
            (0.0, 0.0)
        );
    }

    #[test]
    fn up_on_the_pad_is_up_on_screen_and_the_deadzone_cuts_jitter() {
        let t0 = Instant::now();
        let mut a = TrackpadAim::default();
        a.update(touch(0, 0), t0, &NO_ACCEL);
        // Thumb up (pad +y) is screen -y, as a mouse moved up.
        let (dx, dy) = a.update(touch(0, 1024), t0 + Duration::from_millis(8), &NO_ACCEL);
        assert!(dx == 0.0 && (dy + 1000.0).abs() < 1e-9, "{dy}");
        // Jitter inside the deadzone does nothing, and does not pile up.
        for i in 0..10 {
            let j = if i % 2 == 0 { 10 } else { -10 };
            let m = a.update(
                touch(j, 1024),
                t0 + Duration::from_millis(16 + i * 8),
                &NO_ACCEL,
            );
            assert_eq!(m, (0.0, 0.0));
        }
    }

    #[test]
    fn sensitivity_scales_and_acceleration_grows_with_speed() {
        let t0 = Instant::now();
        let step = |tune: &AimTuning, d: i16, ms: u64| {
            let mut a = TrackpadAim::default();
            a.update(touch(0, 0), t0, tune);
            a.update(touch(d, 0), t0 + Duration::from_millis(ms), tune)
                .0
        };
        let base = step(&NO_ACCEL, 1024, 10);
        let double = step(
            &AimTuning {
                sensitivity: 2.0,
                ..NO_ACCEL
            },
            1024,
            10,
        );
        assert!((double - 2.0 * base).abs() < 1e-9);
        assert_eq!(sensitivity_multiplier(5), 1.0);
        assert_eq!(sensitivity_multiplier(10), 2.0);
        assert_eq!(sensitivity_multiplier(0), 0.2, "clamped to 1");
        // The same travel, fast versus slow: more per unit when fast.
        let accel = AimTuning::default();
        let slow = step(&accel, 1024, 50) / 1000.0;
        let fast = step(&accel, 1024, 2) / 1000.0;
        assert!(fast > slow * 1.4, "{fast} vs {slow}");
        // Capped: at any speed the gain is at most 1 + 0.5 * 2.
        let wild = step(&accel, 16_000, 1) / (16_000.0 - 24.0);
        assert!(wild <= 2.0 + 1e-9, "{wild}");
        // The look path: half the pad in a quarter second at sensitivity 1
        // (no acceleration) is about 0.7 of full turn per 1/60 s.
        let per_frame = 32_768.0 / 15.0;
        assert!((per_frame * LOOK_K - 0.728).abs() < 0.01);
    }

    #[test]
    fn left_pad_dpad_on_click_only() {
        let click = |x, y| Touch {
            touched: true,
            clicked: true,
            x,
            y,
        };
        assert_eq!(
            dpad_from_pad(touch(0, 30_000)),
            (false, false, false, false),
            "touch alone"
        );
        assert_eq!(dpad_from_pad(click(0, 30_000)), (true, false, false, false));
        assert_eq!(
            dpad_from_pad(click(0, -30_000)),
            (false, true, false, false)
        );
        assert_eq!(
            dpad_from_pad(click(-30_000, 0)),
            (false, false, true, false)
        );
        assert_eq!(
            dpad_from_pad(click(30_000, 2_000)),
            (false, false, false, true)
        );
        assert_eq!(
            dpad_from_pad(click(20_000, 20_000)),
            (true, false, false, true),
            "a diagonal"
        );
        assert_eq!(
            dpad_from_pad(click(1_000, -2_000)),
            (false, false, false, false),
            "the centre"
        );
    }

    #[test]
    fn left_pad_as_a_stick() {
        assert_eq!(stick_from_pad(Touch::default()), (0.0, 0.0));
        assert_eq!(
            stick_from_pad(touch(2_000, 2_000)),
            (0.0, 0.0),
            "the centre"
        );
        let (x, y) = stick_from_pad(touch(32_767, 0));
        assert!((x - 1.0).abs() < 1e-6 && y == 0.0);
        let (x, y) = stick_from_pad(touch(0, -16_384));
        assert!(x == 0.0 && (y + (0.5 - 0.25) / 0.75).abs() < 1e-4, "{y}");
    }

    #[test]
    fn the_original_controller_as_a_standard_pad() {
        let mut s = PadState {
            buttons: Buttons(
                Buttons::A | Buttons::RB | Buttons::BACK | Buttons::RGRIP | Buttons::RT_FULL,
            ),
            left_stick: (32_767, 0),
            left_trigger: 0.4,
            right_trigger: 0.2,
            ..PadState::default()
        };
        let h = to_host(Model::Original, &s, LeftPadMode::Dpad);
        assert!(h.buttons.contains(HostButton::South));
        assert!(h.buttons.contains(HostButton::RightBumper));
        assert!(h.buttons.contains(HostButton::Back));
        assert!(h.buttons.contains(HostButton::RightStick), "the right grip");
        assert!(!h.buttons.contains(HostButton::LeftStick));
        assert_eq!((h.left_trigger, h.right_trigger), (0.4, 1.0));
        assert_eq!(h.left_stick, (1.0, 0.0));
        assert_eq!(h.right_stick, (0.0, 0.0), "aim is added by the backend");
        assert!(h.prefiltered);
        // The right pad's click is the right stick click.
        s.buttons = Buttons(0);
        s.right_pad = Touch {
            touched: true,
            clicked: true,
            x: 0,
            y: 0,
        };
        let h = to_host(Model::Original, &s, LeftPadMode::Dpad);
        assert!(h.buttons.contains(HostButton::RightStick));
        // A left-pad click up is d-pad up in d-pad mode...
        s.left_pad = Touch {
            touched: true,
            clicked: true,
            x: 0,
            y: 30_000,
        };
        s.left_stick = (0, 0);
        let h = to_host(Model::Original, &s, LeftPadMode::Dpad);
        assert!(h.buttons.contains(HostButton::DpadUp));
        assert!(!h.buttons.contains(HostButton::LeftStick));
        assert_eq!(h.left_stick, (0.0, 0.0));
        // ...and in stick mode the stick pushed up and its click.
        s.buttons = Buttons(Buttons::DPAD_UP);
        let h = to_host(Model::Original, &s, LeftPadMode::Stick);
        assert!(
            !h.buttons.contains(HostButton::DpadUp),
            "the firmware's d-pad bit too"
        );
        assert!(h.buttons.contains(HostButton::LeftStick));
        assert!(h.left_stick.1 > 0.8 && h.left_stick.0 == 0.0);
        // The physical stick wins when pushed further.
        s.left_stick = (-32_768, 0);
        s.left_pad.y = 10_000;
        let h = to_host(Model::Original, &s, LeftPadMode::Stick);
        assert_eq!(h.left_stick, (-1.0, 0.0));
        // A small stick drift is cut.
        s.left_stick = (1_500, -1_000);
        s.left_pad = Touch::default();
        assert_eq!(
            to_host(Model::Original, &s, LeftPadMode::Dpad).left_stick,
            (0.0, 0.0)
        );
    }

    #[test]
    fn the_2026_controller_keeps_its_own_dpad_and_right_stick() {
        let s = PadState {
            buttons: Buttons(Buttons::DPAD_LEFT | Buttons::R3 | Buttons::LGRIP | Buttons::START),
            right_stick: Some((0, -32_768)),
            ..PadState::default()
        };
        for mode in [LeftPadMode::Dpad, LeftPadMode::Stick] {
            let h = to_host(Model::Triton, &s, mode);
            assert!(h.buttons.contains(HostButton::DpadLeft), "{mode:?}");
            assert!(h.buttons.contains(HostButton::RightStick));
            assert!(h.buttons.contains(HostButton::LeftStick), "L4/L5");
            assert!(h.buttons.contains(HostButton::Start));
            assert_eq!(h.right_stick, (0.0, -1.0));
        }
    }
}
