//! The iOS touch controls' side of the C API (docs/runtime/ios.md): the
//! frame Swift fills in, and the tuning the settings give.
//!
//! Everything that decides what a touch does is in `input::touch`; this is
//! only the C layout and the settings lookup.

use input::touch::{CrouchMode, TouchFrame, TouchTuning};
use input::touch_layout::{self, Plan};

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
    TOUCH_ACCEL, TOUCH_AUTO_FIRE, TOUCH_CROUCH, TOUCH_CROUCH_HOLD as CROUCH_HOLD,
    TOUCH_CROUCH_TOGGLE as CROUCH_TOGGLE, TOUCH_GYRO, TOUCH_LOOK,
};

/// The rows' values ([`tuning_from`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rows {
    pub look: i64,
    pub crouch: i64,
    pub gyro: i64,
    pub accel: i64,
    pub auto_fire: bool,
}

impl Default for Rows {
    fn default() -> Self {
        Rows {
            look: 5,
            crouch: CROUCH_TOGGLE,
            gyro: 0,
            accel: 5,
            auto_fire: false,
        }
    }
}

/// The tuning for the rows' values: look speed 1 to 10 (5 is the default
/// speed, each step about 20% of it), crouch toggle or hold, gyro 0 (off)
/// to 10 (5 is a 1x gain), look acceleration 0 (none) to 10 (5 is the
/// default), auto-fire. Out-of-range values are clamped. `base` keeps what
/// no row sets.
pub fn tuning_from(base: TouchTuning, rows: Rows) -> TouchTuning {
    TouchTuning {
        look_sensitivity: rows.look.clamp(1, 10) as f64 / 5.0,
        crouch: if rows.crouch == CROUCH_HOLD {
            CrouchMode::Hold
        } else {
            CrouchMode::Toggle
        },
        gyro_sensitivity: rows.gyro.clamp(0, 10) as f64 / 5.0,
        direct_acceleration: rows.accel.clamp(0, 10) as f64 / 5.0,
        auto_fire: rows.auto_fire,
        ..base
    }
}

/// The tuning `store` gives now; rows it lacks keep their defaults.
pub fn tuning(store: &settings::Store, base: TouchTuning) -> TouchTuning {
    let d = Rows::default();
    let int = |k: &str, d: i64| store.get(k).and_then(|v| v.as_int()).unwrap_or(d);
    tuning_from(
        base,
        Rows {
            look: int(TOUCH_LOOK.key, d.look),
            crouch: int(TOUCH_CROUCH.key, d.crouch),
            gyro: int(TOUCH_GYRO.key, d.gyro),
            accel: int(TOUCH_ACCEL.key, d.accel),
            auto_fire: store
                .get(TOUCH_AUTO_FIRE.key)
                .and_then(|v| v.as_bool())
                .unwrap_or(d.auto_fire),
        },
    )
}

/// The tuning that follows the game's state: direct look while the boot's
/// hook takes the turn, and the move stick's radius from the layout.
pub fn live(tuning: TouchTuning, plan: &Plan, direct: bool) -> TouchTuning {
    TouchTuning {
        direct_look: direct,
        stick_radius: plan.stick_radius,
        ..tuning
    }
}

/// `H2TouchLayout` in h2host.h: what the on-screen controls should draw
/// (`input::touch_layout::Plan`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct H2TouchLayout {
    /// `H2_TOUCH_LAYOUT_*`: 0 on foot, 1 driver, 2 turret (passenger or
    /// gunner).
    pub layout: u32,
    /// `H2_TOUCH_*` bits of the buttons to draw.
    pub visible: u32,
    /// Of those, the ones drawn faint (they do nothing now).
    pub dimmed: u32,
    /// `H2_TOUCH_FLAG_*`.
    pub flags: u32,
    /// Thumb travel for a full move stick, points.
    pub stick_radius: f32,
    /// Changes whenever any of the above does.
    pub generation: u32,
}

impl H2TouchLayout {
    pub fn from_plan(plan: &Plan, generation: u32) -> Self {
        H2TouchLayout {
            layout: plan.layout as u32,
            visible: plan.visible,
            dimmed: plan.dimmed,
            flags: plan.flags,
            stick_radius: plan.stick_radius,
            generation,
        }
    }
}

/// The plan for the context the boot last published.
pub fn current_plan() -> (Plan, u32) {
    (
        touch_layout::plan(&touch_layout::context()),
        touch_layout::generation(),
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
            if rest.starts_with("LAYOUT_") || rest.starts_with("FLAG_") {
                continue;
            }
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

    fn tuning_from(base: TouchTuning, look: i64, crouch: i64, gyro: i64) -> TouchTuning {
        super::tuning_from(
            base,
            Rows {
                look,
                crouch,
                gyro,
                ..Rows::default()
            },
        )
    }

    #[test]
    fn rows_set_acceleration_and_auto_fire() {
        let base = TouchTuning::default();
        let d = super::tuning_from(base, Rows::default());
        assert_eq!((d.direct_acceleration, d.auto_fire), (1.0, false));
        let t = super::tuning_from(
            base,
            Rows {
                accel: 0,
                auto_fire: true,
                ..Rows::default()
            },
        );
        assert_eq!((t.direct_acceleration, t.auto_fire), (0.0, true));
        let t = super::tuning_from(
            base,
            Rows {
                accel: 99,
                ..Rows::default()
            },
        );
        assert_eq!(t.direct_acceleration, 2.0, "clamped to 10");
        let store = settings::Store::defaults(boot::setting_defs::ALL);
        store
            .set(TOUCH_AUTO_FIRE.key, settings::Value::Bool(true))
            .unwrap();
        store
            .set(TOUCH_ACCEL.key, settings::Value::Int(10))
            .unwrap();
        let t = tuning(&store, base);
        assert_eq!((t.direct_acceleration, t.auto_fire), (2.0, true));
    }

    #[test]
    fn the_layout_follows_the_plan() {
        use input::touch_layout::{Context, Seat, flags};
        let p = touch_layout::plan(&Context {
            seat: Seat::Driver,
            on_target: true,
            ..Context::default()
        });
        let t = live(TouchTuning::default(), &p, true);
        assert!(t.direct_look);
        assert_eq!(t.stick_radius, touch_layout::DRIVE_STICK_RADIUS);
        let c = H2TouchLayout::from_plan(&p, 7);
        assert_eq!(c.layout, 1);
        assert_eq!(c.visible, touch_layout::DRIVER);
        assert_eq!(c.flags, flags::ON_TARGET);
        assert_eq!(c.generation, 7);
        // u32 x4, f32, u32.
        assert_eq!(std::mem::size_of::<H2TouchLayout>(), 24);
        assert_eq!(std::mem::offset_of!(H2TouchLayout, stick_radius), 16);
    }

    #[test]
    fn the_header_layout_ids_and_flags_are_rusts() {
        use input::touch_layout::{Layout, flags};
        let h = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/apple/h2host.h"),
        )
        .unwrap();
        let value = |name: &str| -> u32 {
            let line = h
                .lines()
                .find(|l| l.trim().starts_with(&format!("{name} = ")))
                .unwrap_or_else(|| panic!("{name} in h2host.h"));
            let v = line
                .split(" = ")
                .nth(1)
                .unwrap()
                .trim_end_matches(',')
                .trim();
            let v = v.split("//").next().unwrap().trim().trim_end_matches(',');
            match v.strip_prefix("1u << ") {
                Some(shift) => 1 << shift.parse::<u32>().unwrap(),
                None => v.parse().unwrap(),
            }
        };
        assert_eq!(value("H2_TOUCH_LAYOUT_ON_FOOT"), Layout::OnFoot as u32);
        assert_eq!(value("H2_TOUCH_LAYOUT_DRIVER"), Layout::Driver as u32);
        assert_eq!(value("H2_TOUCH_LAYOUT_TURRET"), Layout::Turret as u32);
        assert_eq!(value("H2_TOUCH_FLAG_ZOOM_NEAR_FIRE"), flags::ZOOM_NEAR_FIRE);
        assert_eq!(value("H2_TOUCH_FLAG_DUAL"), flags::DUAL);
        assert_eq!(value("H2_TOUCH_FLAG_ON_TARGET"), flags::ON_TARGET);
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
