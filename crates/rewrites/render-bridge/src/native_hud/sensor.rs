//! The motion sensor in Rust (docs/graphics/native-hud-overlay.md, "The
//! motion sensor in Rust"): at the HUD render's call of `HUD_SENSOR_DRAW`
//! (`0x001a2823`) Rust works out the point the render gives it and every
//! draw the call then asks for (`engine::hud_sensor`), and checks them
//! against the guest's: the point, each blip draw at `HUD_SENSOR_BLIP`
//! and the composite at `HUD_SENSOR_COMPOSITE`, bit for bit, in order.
//!
//! The guest draws the sensor. Every call is checked until one differs,
//! which keeps the sensor unchecked for the map; [`PROOF_DRAWS`]
//! matching calls in a row prove it (`sensor_proven`), the list a Rust
//! draw of the sensor starts from.

use std::cell::RefCell;

use engine::hud_sensor::{Blip, Sensor};

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use super::walk;

/// Matching calls that prove the sensor.
pub const PROOF_DRAWS: u32 = 8;
/// A proven sensor's calls the guest still checks: one in this many.
pub const RECHECK: u32 = 64;

/// Whose a sensor draw is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// The guest draws it, unchecked: a call of it differed on this map.
    Guest,
    /// The guest draws it and the call is checked.
    Shadow,
    /// Proven, and not one of the rechecked.
    Own,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Proof {
    clean: u32,
    failed: bool,
    /// Calls since it was proven, for the recheck.
    seen: u32,
}

impl Proof {
    pub fn arm(&mut self) -> Arm {
        if self.failed {
            return Arm::Guest;
        }
        if self.clean < PROOF_DRAWS {
            return Arm::Shadow;
        }
        self.seen = self.seen.wrapping_add(1);
        if self.seen % RECHECK == 0 { Arm::Shadow } else { Arm::Own }
    }

    /// Whether Rust draws it, asked without counting a call.
    pub fn proven(&self) -> bool {
        !self.failed && self.clean >= PROOF_DRAWS
    }

    /// A checked call's answer. True when this one proved the sensor.
    pub fn checked(&mut self, matched: bool) -> bool {
        if !matched {
            self.failed = true;
            return false;
        }
        self.clean = self.clean.saturating_add(1);
        self.clean == PROOF_DRAWS
    }
}

#[derive(Default)]
struct Proofs {
    generation: Option<u64>,
    /// By the local user.
    users: HashMap<u32, Proof>,
}

static PROOFS: LazyLock<Mutex<Proofs>> = LazyLock::new(Mutex::default);

/// The proof of local user `user`'s sensor draws, kept for the map.
fn sensor_proof<T>(user: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    let mut s = PROOFS.lock().unwrap_or_else(|e| e.into_inner());
    let generation = crate::native_map_work::generation();
    if s.generation != Some(generation) {
        *s = Proofs { generation: Some(generation), ..Proofs::default() };
    }
    f(s.users.entry(user).or_default())
}

/// The return address of the HUD render's call of `HUD_SENSOR_DRAW`.
pub const DRAW_RETURN: u32 = 0x0013_968a;
/// The return address of `0x001a2197`'s call of `HUD_SENSOR_BLIP`.
pub const BLIP_RETURN: u32 = 0x001a_2219;
/// The return address of `HUD_SENSOR_DRAW`'s call of `HUD_SENSOR_COMPOSITE`.
pub const COMPOSITE_RETURN: u32 = 0x001a_2960;

/// What Rust works out at the call's entry: the render's point and the
/// call's draws (`None`: it draws nothing).
pub type Expected = ([i16; 2], Option<Sensor>);

/// One call of `HUD_SENSOR_DRAW` in progress.
struct Call {
    user: u32,
    point: [i16; 2],
    expected: Expected,
    blips: Vec<Blip>,
    composite: Option<([u32; 2], u32)>,
}

thread_local! {
    static CALL: RefCell<Option<Call>> = const { RefCell::new(None) };
}

/// `HUD_SENSOR_DRAW`'s entry from the HUD render, for local user `user`
/// with the guest's `point`: whether the call is checked. `expected` is
/// `None` when what Rust reads does not read. A call still open (its
/// return was not seen) is closed first.
pub fn enter(user: u32, point: Option<[i16; 2]>, expected: Option<Expected>) -> bool {
    leave();
    let (Some(point), Some(expected)) = (point, expected) else {
        walk::count("sensor_read");
        return false;
    };
    if sensor_proof(user, Proof::arm) == Arm::Guest {
        walk::count("sensor_guest");
        return false;
    }
    let call = Call { user, point, expected, blips: Vec::new(), composite: None };
    CALL.with(|c| *c.borrow_mut() = Some(call));
    true
}

/// Whether a call is being checked.
pub fn checking() -> bool {
    CALL.with(|c| c.borrow().is_some())
}

/// The guest's blip draw, from `0x001a2197`.
pub fn blip(b: Blip) {
    CALL.with(|c| {
        if let Some(call) = c.borrow_mut().as_mut() {
            call.blips.push(b);
        }
    });
}

/// The guest's composite, from `HUD_SENSOR_DRAW`.
pub fn composite(centre: [u32; 2], value: u32) {
    CALL.with(|c| {
        if let Some(call) = c.borrow_mut().as_mut() {
            call.composite = Some((centre, value));
        }
    });
}

/// A guest draw of the call did not read: the call is not checked.
pub fn unread() {
    if CALL.with(|c| c.borrow_mut().take()).is_some() {
        walk::count("sensor_read");
    }
}

/// The first part of the guest's call that is not Rust's, `None` when
/// none is, or `Some("unseen")` when the guest drew nothing Rust expected
/// a draw of (another feature skipped the call).
fn verdict(call: &Call) -> Option<&'static str> {
    let (point, sensor) = &call.expected;
    if *point != call.point {
        return Some("point");
    }
    let Some(sensor) = sensor else {
        return (!call.blips.is_empty() || call.composite.is_some()).then_some("drawn");
    };
    if call.blips.is_empty() && call.composite.is_none() {
        return Some("unseen");
    }
    if call.blips != sensor.blips {
        return Some("blips");
    }
    if call.composite != Some((sensor.centre, sensor.value)) {
        return Some("composite");
    }
    None
}

/// `HUD_SENSOR_DRAW`'s return: the guest's draws against Rust's.
pub fn leave() {
    let Some(call) = CALL.with(|c| c.borrow_mut().take()) else { return };
    match verdict(&call) {
        None => {
            walk::count("sensor_matched");
            if sensor_proof(call.user, |p| p.checked(true)) {
                walk::count("sensor_proven");
            }
        }
        Some("unseen") => walk::count("sensor_unseen"),
        Some(part) => {
            walk::count("sensor_differed");
            walk::count(match part {
                "point" => "sensor_differed_point",
                "drawn" => "sensor_differed_drawn",
                "blips" => "sensor_differed_blips",
                _ => "sensor_differed_composite",
            });
            sensor_proof(call.user, |p| p.checked(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINT: [i16; 2] = [73, 414];

    fn blip(n: u32) -> Blip {
        Blip { position: [n, n + 1], shade: 0x3f80_0000, size: 0x4040_0000, colour: 0x0044_5320 + 12 * n, pulse: false }
    }

    fn sensor() -> Sensor {
        Sensor { centre: [73.0f32.to_bits(), 414.0f32.to_bits()], blips: vec![blip(1), blip(2)], value: 0x3f40_0000 }
    }

    /// One call for `user` whose guest draws are `drawn`: whether it was
    /// checked.
    fn call(user: u32, point: [i16; 2], drawn: Option<&Sensor>) -> bool {
        let checked = enter(user, Some(point), Some((POINT, Some(sensor()))));
        if let Some(s) = drawn {
            for b in &s.blips {
                super::blip(*b);
            }
            composite(s.centre, s.value);
        }
        leave();
        checked
    }

    fn proven(user: u32) -> bool {
        sensor_proof(user, |p| p.proven())
    }

    #[test]
    fn the_sensors_draws_are_proven_by_matching_calls_in_a_row() {
        let user = 0xe0;
        for n in 0..PROOF_DRAWS {
            assert!(!proven(user), "call {n}");
            assert!(call(user, POINT, Some(&sensor())), "call {n}");
        }
        assert!(proven(user));
        // Every call is still checked: the guest draws the sensor.
        assert!(call(user, POINT, Some(&sensor())));
    }

    #[test]
    fn a_call_that_differs_leaves_the_sensor_unchecked_for_the_map() {
        let user = 0xe1;
        let mut other = sensor();
        other.blips.swap(0, 1);
        assert!(call(user, POINT, Some(&other)));
        for _ in 0..2 * PROOF_DRAWS {
            assert!(!call(user, POINT, Some(&sensor())));
        }
        assert!(!proven(user));
        // Another user's sensor is its own.
        assert!(call(0xe2, POINT, Some(&sensor())));
    }

    #[test]
    fn each_part_of_the_call_is_checked_in_turn() {
        let at = |point: [i16; 2], expected: Option<Sensor>, drawn: Option<Sensor>| {
            let mut c = Call { user: 0, point, expected: (POINT, expected), blips: Vec::new(), composite: None };
            if let Some(s) = drawn {
                c.blips = s.blips;
                c.composite = Some((s.centre, s.value));
            }
            verdict(&c)
        };
        assert_eq!(at(POINT, Some(sensor()), Some(sensor())), None);
        assert_eq!(at([73, 415], Some(sensor()), Some(sensor())), Some("point"));
        assert_eq!(at(POINT, None, None), None, "a camera that draws no sensor");
        assert_eq!(at(POINT, None, Some(sensor())), Some("drawn"));
        assert_eq!(at(POINT, Some(sensor()), None), Some("unseen"));
        let mut s = sensor();
        s.blips[1].pulse = true;
        assert_eq!(at(POINT, Some(sensor()), Some(s)), Some("blips"));
        let mut s = sensor();
        s.value = 0;
        assert_eq!(at(POINT, Some(sensor()), Some(s)), Some("composite"));
    }

    #[test]
    fn a_skipped_call_is_no_verdict_and_an_open_call_closes_at_the_next() {
        let user = 0xe3;
        // Another feature hid the sensor: the guest drew nothing.
        assert!(call(user, POINT, None));
        assert!(!proven(user));
        // A call whose return was not seen is judged at the next entry.
        assert!(enter(user, Some(POINT), Some((POINT, Some(sensor())))));
        let s = sensor();
        for b in &s.blips {
            super::blip(*b);
        }
        composite(s.centre, s.value);
        assert!(call(user, POINT, Some(&sensor())));
        // Draws outside a checked call are not the sensor's.
        super::blip(blip(9));
        assert!(!checking());
        for _ in 2..PROOF_DRAWS {
            assert!(call(user, POINT, Some(&sensor())));
        }
        assert!(proven(user), "the skipped call counted for neither side");
    }

    #[test]
    fn a_call_rust_cannot_read_is_not_checked() {
        assert!(!enter(0xe4, None, Some((POINT, None))));
        assert!(!enter(0xe4, Some(POINT), None));
        assert!(!checking());
        // A guest draw that does not read drops the call, with no verdict.
        assert!(enter(0xe4, Some(POINT), Some((POINT, Some(sensor())))));
        unread();
        assert!(!checking());
        leave();
        assert!(call(0xe4, POINT, Some(&sensor())));
    }
}
