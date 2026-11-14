//! Host-side pad description and the conversion to Xbox units.
//!
//! Backends report a [`HostPadState`] in normalised floats using a
//! Xbox-agnostic "standard gamepad" vocabulary (south/east/west/north face
//! buttons, bumpers, ...). [`HostPadState::to_gamepad`] maps it onto
//! `XINPUT_GAMEPAD`.
//!
//! Mapping decisions:
//!
//! * Face buttons: south/east/west/north become A/B/X/Y (Xbox layout:
//!   A bottom, B right, X left, Y top). The Xbox face buttons are pressure
//!   sensitive (0..=255) but no host API we use exposes pressure, so a
//!   digital press reports 255 and release 0.
//! * Black and White (the Xbox shoulder buttons, which are analog too): the
//!   right bumper is Black, the left bumper is White. This is the usual
//!   emulator convention; it is a policy choice and lives in one place.
//! * Triggers: host analog 0..1 is rescaled to 0..=255 after a small
//!   trigger deadzone. Backends with only digital triggers report 0.0/1.0.
//! * Sticks: a radial deadzone with rescaling, then scaled to +-32767.
//!   `sThumbLY`/`sThumbRY` are positive up, like the host float convention
//!   used here. The real Xbox pad reports raw values and games apply their
//!   own deadzone, so we only remove host-side noise (default 0.08), not a
//!   gameplay deadzone. Virtual pads (keyboard/mouse) skip it.
//! * D-pad: opposing directions cancel each other (a real d-pad cannot
//!   report both, and keyboards can).

use crate::gamepad::{AnalogButton, Buttons, Gamepad};

/// Stable identity of a host pad within one backend while it stays connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostPadId(pub u64);

/// Standard-gamepad button vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostButton {
    South,
    East,
    West,
    North,
    LeftBumper,
    RightBumper,
    Back,
    Start,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

/// Set of [`HostButton`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostButtons(u16);

impl HostButtons {
    pub fn contains(self, b: HostButton) -> bool {
        self.0 & (1 << b as u8) != 0
    }
    pub fn set(&mut self, b: HostButton, down: bool) {
        if down {
            self.0 |= 1 << b as u8;
        } else {
            self.0 &= !(1 << b as u8);
        }
    }
}

/// Normalised pad state. Sticks are -1..=1 with +Y up; triggers 0..=1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostPadState {
    pub buttons: HostButtons,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub left_trigger: f32,
    pub right_trigger: f32,
}

/// One connected host pad as reported by a backend.
#[derive(Clone, Debug)]
pub struct HostPad {
    pub id: HostPadId,
    pub name: String,
    pub state: HostPadState,
    /// Physical hardware (apply the deadzone) versus synthesised input.
    pub physical: bool,
    pub supports_rumble: bool,
}

/// Deadzone settings, all in normalised 0..1 units.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConvertConfig {
    pub stick_deadzone: f32,
    pub trigger_deadzone: f32,
}

/// Invalid configuration value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{field} must be {expected}")]
pub struct ConfigError {
    pub field: &'static str,
    pub expected: &'static str,
}

impl ConvertConfig {
    /// Deadzones must be finite and in `[0, 1)`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let ok = |v: f32| v.is_finite() && (0.0..1.0).contains(&v);
        if !ok(self.stick_deadzone) {
            return Err(ConfigError { field: "stick_deadzone", expected: "a finite number in [0, 1)" });
        }
        if !ok(self.trigger_deadzone) {
            return Err(ConfigError { field: "trigger_deadzone", expected: "a finite number in [0, 1)" });
        }
        Ok(())
    }
}

impl Default for ConvertConfig {
    fn default() -> Self {
        ConvertConfig { stick_deadzone: 0.08, trigger_deadzone: 0.03 }
    }
}

/// One host axis: non-finite (NaN, infinite) becomes 0 for that axis only,
/// finite values are clamped to the legal range.
fn sane(v: f32, lo: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 }
}

fn stick_to_i16(stick: (f32, f32), deadzone: f32) -> (i16, i16) {
    // Each axis is sanitised and clamped before the magnitude is compared to
    // the deadzone, so a bad Y never poisons a good X.
    let (x, y) = (sane(stick.0, -1.0), sane(stick.1, -1.0));
    let mag = x.hypot(y);
    if mag <= deadzone || mag == 0.0 {
        return (0, 0);
    }
    let dz = deadzone.clamp(0.0, 0.99);
    let scaled = ((mag.min(1.0) - dz) / (1.0 - dz)).clamp(0.0, 1.0);
    let f = scaled / mag;
    let conv = |v: f32| (v * f * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    (conv(x), conv(y))
}

fn trigger_to_u8(v: f32, deadzone: f32) -> u8 {
    let v = sane(v, 0.0);
    if v <= deadzone {
        return 0;
    }
    let dz = deadzone.clamp(0.0, 0.99);
    (((v - dz) / (1.0 - dz)) * 255.0).round().clamp(0.0, 255.0) as u8
}

impl HostPadState {
    /// Convert to the Xbox representation. `apply_deadzone` is false for
    /// virtual pads.
    pub fn to_gamepad(&self, cfg: &ConvertConfig, apply_deadzone: bool) -> Gamepad {
        use HostButton::*;
        let mut g = Gamepad::default();
        let b = self.buttons;

        let digital = |hb: HostButton, ab: AnalogButton, g: &mut Gamepad| {
            g.analog[ab.index()] = if b.contains(hb) { 255 } else { 0 };
        };
        digital(South, AnalogButton::A, &mut g);
        digital(East, AnalogButton::B, &mut g);
        digital(West, AnalogButton::X, &mut g);
        digital(North, AnalogButton::Y, &mut g);
        digital(RightBumper, AnalogButton::Black, &mut g);
        digital(LeftBumper, AnalogButton::White, &mut g);

        let (sdz, tdz) = if apply_deadzone { (cfg.stick_deadzone, cfg.trigger_deadzone) } else { (0.0, 0.0) };
        g.analog[AnalogButton::LeftTrigger.index()] = trigger_to_u8(self.left_trigger, tdz);
        g.analog[AnalogButton::RightTrigger.index()] = trigger_to_u8(self.right_trigger, tdz);
        (g.thumb_lx, g.thumb_ly) = stick_to_i16(self.left_stick, sdz);
        (g.thumb_rx, g.thumb_ry) = stick_to_i16(self.right_stick, sdz);

        let (up, down) = (b.contains(DpadUp), b.contains(DpadDown));
        let (left, right) = (b.contains(DpadLeft), b.contains(DpadRight));
        g.buttons.set(Buttons::DPAD_UP, up && !down);
        g.buttons.set(Buttons::DPAD_DOWN, down && !up);
        g.buttons.set(Buttons::DPAD_LEFT, left && !right);
        g.buttons.set(Buttons::DPAD_RIGHT, right && !left);
        g.buttons.set(Buttons::START, b.contains(Start));
        g.buttons.set(Buttons::BACK, b.contains(Back));
        g.buttons.set(Buttons::LEFT_THUMB, b.contains(LeftStick));
        g.buttons.set(Buttons::RIGHT_THUMB, b.contains(RightStick));
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ConvertConfig {
        ConvertConfig { stick_deadzone: 0.1, trigger_deadzone: 0.1 }
    }

    #[test]
    fn digital_faces_map_to_full_pressure() {
        let mut s = HostPadState::default();
        s.buttons.set(HostButton::South, true);
        s.buttons.set(HostButton::RightBumper, true);
        s.buttons.set(HostButton::Start, true);
        let g = s.to_gamepad(&cfg(), true);
        assert_eq!(g.analog(AnalogButton::A), 255);
        assert_eq!(g.analog(AnalogButton::B), 0);
        assert_eq!(g.analog(AnalogButton::Black), 255);
        assert_eq!(g.analog(AnalogButton::White), 0);
        assert!(g.buttons.contains(Buttons::START));
        assert!(!g.buttons.contains(Buttons::BACK));
    }

    #[test]
    fn stick_deadzone_and_scaling() {
        let mut s = HostPadState::default();
        s.left_stick = (0.05, 0.05);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_lx, 0);
        s.left_stick = (1.0, 0.0);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_lx, 32767);
        s.left_stick = (-0.55, 0.0); // halfway through the live range
        let g = s.to_gamepad(&cfg(), true);
        assert!((g.thumb_lx as i32 + 16384).abs() <= 1, "{}", g.thumb_lx);
        s.left_stick = (0.0, 1.0);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_ly, 32767);
        // Corner values are clamped to the unit circle, not left at sqrt(2).
        s.right_stick = (1.0, 1.0);
        let g = s.to_gamepad(&cfg(), true);
        let m = f64::from(g.thumb_rx).hypot(f64::from(g.thumb_ry));
        assert!(m <= 32767.5);
    }

    #[test]
    fn virtual_pads_skip_deadzone() {
        let mut s = HostPadState::default();
        s.right_stick = (0.02, 0.0);
        s.right_trigger = 0.05;
        let g = s.to_gamepad(&cfg(), false);
        assert_eq!(g.thumb_rx, 655);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 13);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_rx, 0);
    }

    #[test]
    fn triggers_rescale() {
        let mut s = HostPadState::default();
        s.left_trigger = 1.0;
        assert_eq!(s.to_gamepad(&cfg(), true).analog(AnalogButton::LeftTrigger), 255);
        s.left_trigger = 0.1;
        assert_eq!(s.to_gamepad(&cfg(), true).analog(AnalogButton::LeftTrigger), 0);
        s.left_trigger = 0.55;
        assert_eq!(s.to_gamepad(&cfg(), true).analog(AnalogButton::LeftTrigger), 128);
    }

    #[test]
    fn non_finite_axes_zero_only_that_axis() {
        let mut s = HostPadState::default();
        s.left_stick = (f32::NAN, 0.5);
        s.right_stick = (f32::INFINITY, -1.0);
        s.left_trigger = f32::NAN;
        s.right_trigger = 1.0;
        let g = s.to_gamepad(&cfg(), true);
        assert_eq!(g.thumb_lx, 0);
        assert!(g.thumb_ly > 0, "y survives a NaN x");
        assert_eq!(g.thumb_rx, 0);
        assert_eq!(g.thumb_ry, -32767);
        assert_eq!(g.analog(AnalogButton::LeftTrigger), 0);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 255);
    }

    #[test]
    fn out_of_range_is_clamped_before_deadzone() {
        let mut s = HostPadState::default();
        s.left_stick = (100.0, 0.0);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_lx, 32767);
        s.left_stick = (-5.0, 0.0);
        assert_eq!(s.to_gamepad(&cfg(), true).thumb_lx, -32767);
        s.right_trigger = -3.0;
        assert_eq!(s.to_gamepad(&cfg(), true).analog(AnalogButton::RightTrigger), 0);
    }

    #[test]
    fn config_validation() {
        assert!(ConvertConfig::default().validate().is_ok());
        for bad in [-0.1, 1.0, 2.0, f32::NAN, f32::INFINITY] {
            assert!(ConvertConfig { stick_deadzone: bad, trigger_deadzone: 0.0 }.validate().is_err());
            assert!(ConvertConfig { stick_deadzone: 0.0, trigger_deadzone: bad }.validate().is_err());
        }
    }

    #[test]
    fn opposing_dpad_cancels() {
        let mut s = HostPadState::default();
        s.buttons.set(HostButton::DpadUp, true);
        s.buttons.set(HostButton::DpadDown, true);
        s.buttons.set(HostButton::DpadLeft, true);
        let g = s.to_gamepad(&cfg(), true);
        assert_eq!(g.buttons, Buttons::DPAD_LEFT);
    }
}
