//! The iOS touch controls' side of the C API (docs/runtime/ios.md): the
//! frame Swift fills in, and the tuning the settings give.
//!
//! Everything that decides what a touch does is in `input::touch`; this is
//! only the C layout and the settings lookup.

use input::touch::{CrouchMode, TouchFrame, TouchTuning};

/// `H2TouchFrame` in h2host.h: one report from the on-screen controls.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct H2TouchFrame {
    /// `H2_TOUCH_*` bits held (`input::touch::bits`).
    pub buttons: u32,
    /// A thumb is on the move side; `move_x`, `move_y` are its offset in
    /// points from where it landed (+y down).
    pub move_active: bool,
    pub move_x: f32,
    pub move_y: f32,
    /// Look drag since the previous call, in points (+y down).
    pub look_dx: f32,
    pub look_dy: f32,
    /// Rotation rate in radians per second, screen terms (+x turns right,
    /// +y looks up); 0 with gyro aiming off.
    pub gyro_x: f32,
    pub gyro_y: f32,
}

impl H2TouchFrame {
    pub fn frame(&self) -> TouchFrame {
        TouchFrame {
            buttons: self.buttons,
            move_offset: self.move_active.then_some((self.move_x, self.move_y)),
            look: (self.look_dx, self.look_dy),
            gyro: (self.gyro_x, self.gyro_y),
        }
    }
}

use boot::setting_defs::{
    TOUCH_CROUCH, TOUCH_CROUCH_HOLD as CROUCH_HOLD, TOUCH_CROUCH_TOGGLE as CROUCH_TOGGLE,
    TOUCH_GYRO, TOUCH_LOOK,
};

/// The tuning for the rows' values: look speed 1 to 10 (5 is the default
/// speed, each step about 20% of it), crouch toggle or hold, gyro 0 (off)
/// to 10 (5 is a 1x gain). Out-of-range values are clamped. `base` keeps
/// what no row sets.
pub fn tuning_from(base: TouchTuning, look: i64, crouch: i64, gyro: i64) -> TouchTuning {
    TouchTuning {
        look_sensitivity: look.clamp(1, 10) as f64 / 5.0,
        crouch: if crouch == CROUCH_HOLD {
            CrouchMode::Hold
        } else {
            CrouchMode::Toggle
        },
        gyro_sensitivity: gyro.clamp(0, 10) as f64 / 5.0,
        ..base
    }
}

/// The tuning `store` gives now; rows it lacks keep their defaults.
pub fn tuning(store: &settings::Store, base: TouchTuning) -> TouchTuning {
    let int = |k: &str, d: i64| store.get(k).and_then(|v| v.as_int()).unwrap_or(d);
    tuning_from(
        base,
        int(TOUCH_LOOK.key, 5),
        int(TOUCH_CROUCH.key, CROUCH_TOGGLE),
        int(TOUCH_GYRO.key, 0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_c_frame_becomes_a_touch_frame() {
        let c = H2TouchFrame {
            buttons: input::touch::bits::RIGHT_TRIGGER,
            move_active: true,
            move_x: 3.0,
            move_y: -4.0,
            look_dx: 1.5,
            look_dy: 2.5,
            gyro_x: 0.25,
            gyro_y: -0.5,
        };
        let f = c.frame();
        assert_eq!(f.buttons, input::touch::bits::RIGHT_TRIGGER);
        assert_eq!(f.move_offset, Some((3.0, -4.0)));
        assert_eq!(f.look, (1.5, 2.5));
        assert_eq!(f.gyro, (0.25, -0.5));
        assert_eq!(
            H2TouchFrame {
                move_active: false,
                ..c
            }
            .frame()
            .move_offset,
            None
        );
    }

    #[test]
    fn the_c_layout_is_what_h2host_h_declares() {
        // uint32_t, bool (padded to 4), then six floats.
        assert_eq!(std::mem::size_of::<H2TouchFrame>(), 4 + 4 + 6 * 4);
        assert_eq!(std::mem::offset_of!(H2TouchFrame, move_x), 8);
        assert_eq!(std::mem::offset_of!(H2TouchFrame, gyro_y), 28);
    }

    #[test]
    fn the_header_bits_are_the_touch_bits() {
        use input::touch::bits;
        let h = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/apple/h2host.h"),
        )
        .unwrap();
        let mut seen = 0u32;
        for line in h.lines() {
            let Some(rest) = line.trim().strip_prefix("H2_TOUCH_") else {
                continue;
            };
            let (name, value) = rest.split_once(" = 1u << ").unwrap();
            let shift: u32 = value.split(',').next().unwrap().parse().unwrap();
            let want = match name {
                "A" => bits::A,
                "B" => bits::B,
                "X" => bits::X,
                "Y" => bits::Y,
                "BLACK" => bits::BLACK,
                "WHITE" => bits::WHITE,
                "LEFT_TRIGGER" => bits::LEFT_TRIGGER,
                "RIGHT_TRIGGER" => bits::RIGHT_TRIGGER,
                "START" => bits::START,
                "BACK" => bits::BACK,
                "LEFT_THUMB" => bits::LEFT_THUMB,
                "RIGHT_THUMB" => bits::RIGHT_THUMB,
                "DPAD_UP" => bits::DPAD_UP,
                "DPAD_DOWN" => bits::DPAD_DOWN,
                "DPAD_LEFT" => bits::DPAD_LEFT,
                "DPAD_RIGHT" => bits::DPAD_RIGHT,
                other => panic!("unknown H2_TOUCH_{other}"),
            };
            assert_eq!(1 << shift, want, "H2_TOUCH_{name}");
            seen |= want;
        }
        assert_eq!(seen, bits::ALL, "every bit is declared");
    }

    #[test]
    fn rows_set_look_speed_crouch_and_gyro() {
        let base = TouchTuning::default();
        let t = tuning_from(base, 5, CROUCH_TOGGLE, 0);
        assert_eq!(
            t,
            TouchTuning {
                look_sensitivity: 1.0,
                ..base
            },
            "defaults are the defaults"
        );
        assert_eq!(
            tuning_from(base, 10, CROUCH_TOGGLE, 0).look_sensitivity,
            2.0
        );
        assert_eq!(
            tuning_from(base, 0, CROUCH_TOGGLE, 0).look_sensitivity,
            0.2,
            "clamped"
        );
        assert_eq!(
            tuning_from(base, 5, CROUCH_HOLD, 0).crouch,
            CrouchMode::Hold
        );
        assert_eq!(tuning_from(base, 5, 7, 0).crouch, CrouchMode::Toggle);
        assert_eq!(tuning_from(base, 5, CROUCH_TOGGLE, 5).gyro_sensitivity, 1.0);
        assert_eq!(
            tuning_from(base, 5, CROUCH_TOGGLE, -3).gyro_sensitivity,
            0.0
        );
    }

    #[test]
    fn the_settings_store_drives_the_tuning() {
        let base = TouchTuning::default();
        let store = settings::Store::defaults(boot::setting_defs::ALL);
        assert_eq!(tuning(&store, base), tuning_from(base, 5, CROUCH_TOGGLE, 0));
        store
            .set(TOUCH_CROUCH.key, settings::Value::Int(CROUCH_HOLD))
            .unwrap();
        store.set(TOUCH_LOOK.key, settings::Value::Int(8)).unwrap();
        store.set(TOUCH_GYRO.key, settings::Value::Int(5)).unwrap();
        let t = tuning(&store, base);
        assert_eq!(t, tuning_from(base, 8, CROUCH_HOLD, 5));
        assert_eq!((t.crouch, t.gyro_sensitivity), (CrouchMode::Hold, 1.0));
    }
}
