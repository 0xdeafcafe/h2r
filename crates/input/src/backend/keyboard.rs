//! Keyboard and mouse as a virtual Xbox pad.
//!
//! This backend has no window of its own: the platform layer forwards key and
//! mouse events (`key_down("KeyW")`, `mouse_motion(dx, dy)`, ...) and the
//! backend turns the held set into a [`HostPadState`] using [`Bindings`].
//!
//! * Keys are digital: buttons report 255, stick keys full deflection
//!   (diagonals are normalised to the unit circle), trigger keys 1.0.
//! * Mouse motion drives the right stick when `mouse.look` is on. Motion
//!   accumulated since the last `poll` becomes a stick deflection of
//!   `delta * sensitivity`, clamped to 1, then resets. Halo treats the stick
//!   as a turn *rate*, so look speed is frame-rate dependent; tune
//!   `sensitivity` for the sync rate you use (60 Hz default).
//! * The virtual pad is not "physical": no deadzone is applied, so tiny mouse
//!   deltas still register.
//! * Rumble is off by default (`XInputSetState` returns 50, like a pad without
//!   motors). [`KeyboardMouse::set_rumble_supported`] makes the pad advertise both
//!   motors and accept feedback like a standard controller; requests then
//!   complete and the newest motor speeds are readable from
//!   [`KeyboardMouse::rumble`]. Set it before the manager's first `sync`: a
//!   pad's capabilities are fixed when it is inserted.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::backend::HostBackend;
use crate::bindings::{Bindings, Control, Input, MouseButton, canonical_key};
use crate::gamepad::Rumble;
use crate::host::{HostButton, HostPad, HostPadId, HostPadState};

/// Cloneable handle to one virtual keyboard+mouse pad. Clone it before
/// boxing the backend into [`InputManager::add_backend`](crate::InputManager::add_backend):
/// the platform layer keeps one clone to feed events, the manager owns the
/// other, and both see the same state.
#[derive(Clone)]
pub struct KeyboardMouse {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    bindings: Bindings,
    keys: HashSet<String>,
    mouse: HashSet<MouseButton>,
    delta: (f64, f64),
    enabled: bool,
    rumble_supported: bool,
    rumble: Rumble,
}

impl Inner {
    fn release_all(&mut self) {
        self.keys.clear();
        self.mouse.clear();
        self.delta = (0.0, 0.0);
    }

    fn held(&self, c: Control) -> bool {
        self.bindings.inputs(c).iter().any(|i| match i {
            Input::Key(k) => self.keys.contains(k),
            Input::Mouse(m) => self.mouse.contains(m),
        })
    }
}

impl KeyboardMouse {
    pub fn new(bindings: Bindings) -> Self {
        KeyboardMouse {
            inner: Arc::new(Mutex::new(Inner {
                bindings,
                keys: HashSet::new(),
                mouse: HashSet::new(),
                delta: (0.0, 0.0),
                enabled: true,
                rumble_supported: false,
                rumble: Rumble::default(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn bindings(&self) -> Bindings {
        self.lock().bindings.clone()
    }

    /// Show or hide the virtual pad (it occupies a port while enabled).
    pub fn set_enabled(&self, on: bool) {
        let mut i = self.lock();
        i.enabled = on;
        if !on {
            i.release_all();
        }
    }

    /// Advertise motors and accept rumble feedback (see the module docs).
    /// Takes effect for the next insertion of the pad.
    pub fn set_rumble_supported(&self, supported: bool) {
        self.lock().rumble_supported = supported;
    }

    /// Newest motor speeds the guest asked for (zero until then).
    pub fn rumble(&self) -> Rumble {
        self.lock().rumble
    }

    /// Key pressed. Returns false if the name is not a known key.
    pub fn key_down(&self, name: &str) -> bool {
        canonical_key(name)
            .map(|k| self.lock().keys.insert(k))
            .is_some()
    }

    pub fn key_up(&self, name: &str) -> bool {
        canonical_key(name)
            .map(|k| self.lock().keys.remove(&k))
            .is_some()
    }

    pub fn mouse_button(&self, b: MouseButton, down: bool) {
        let mut i = self.lock();
        if down {
            i.mouse.insert(b);
        } else {
            i.mouse.remove(&b);
        }
    }

    /// Relative mouse motion in counts (screen space: +y is down). Non-finite
    /// values are ignored.
    pub fn mouse_motion(&self, dx: f64, dy: f64) {
        if dx.is_finite() && dy.is_finite() {
            let mut i = self.lock();
            i.delta.0 += dx;
            i.delta.1 += dy;
        }
    }

    /// Focus lost: release everything.
    pub fn release_all(&self) {
        self.lock().release_all();
    }

    /// Build the pad state for the current held set, consuming mouse motion.
    pub fn state(&self) -> HostPadState {
        let mut i = self.lock();
        use Control::*;
        let mut s = HostPadState::default();
        let buttons = [
            (A, HostButton::South),
            (B, HostButton::East),
            (X, HostButton::West),
            (Y, HostButton::North),
            (Black, HostButton::RightBumper),
            (White, HostButton::LeftBumper),
            (DpadUp, HostButton::DpadUp),
            (DpadDown, HostButton::DpadDown),
            (DpadLeft, HostButton::DpadLeft),
            (DpadRight, HostButton::DpadRight),
            (Start, HostButton::Start),
            (Back, HostButton::Back),
            (LeftThumb, HostButton::LeftStick),
            (RightThumb, HostButton::RightStick),
        ];
        for (c, hb) in buttons {
            s.buttons.set(hb, i.held(c));
        }
        let axis = |neg: bool, pos: bool| f32::from(u8::from(pos)) - f32::from(u8::from(neg));
        s.left_trigger = f32::from(u8::from(i.held(LeftTrigger)));
        s.right_trigger = f32::from(u8::from(i.held(RightTrigger)));
        s.left_stick = (
            axis(i.held(LeftStickLeft), i.held(LeftStickRight)),
            axis(i.held(LeftStickDown), i.held(LeftStickUp)),
        );
        s.right_stick = (
            axis(i.held(RightStickLeft), i.held(RightStickRight)),
            axis(i.held(RightStickDown), i.held(RightStickUp)),
        );
        let m = i.bindings.mouse;
        if m.look {
            let sign = if m.invert_y { 1.0 } else { -1.0 }; // mouse up (dy < 0) looks up
            let k = f64::from(m.sensitivity);
            s.right_stick.0 = (f64::from(s.right_stick.0) + i.delta.0 * k).clamp(-1.0, 1.0) as f32;
            s.right_stick.1 =
                (f64::from(s.right_stick.1) + sign * i.delta.1 * k).clamp(-1.0, 1.0) as f32;
        }
        // Synthesised directions have no physical gate. Normalise here so
        // calibrated hardware and scripted reports can retain independent axes.
        for stick in [&mut s.left_stick, &mut s.right_stick] {
            let magnitude = stick.0.hypot(stick.1);
            if magnitude > 1.0 {
                stick.0 /= magnitude;
                stick.1 /= magnitude;
            }
        }
        i.delta = (0.0, 0.0);
        s
    }
}

impl HostBackend for KeyboardMouse {
    fn name(&self) -> &str {
        "keyboard+mouse"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        if !self.lock().enabled {
            return Vec::new();
        }
        vec![HostPad {
            id: HostPadId(0),
            name: "Keyboard + Mouse".into(),
            state: self.state(),
            physical: false,
            supports_rumble: self.lock().rumble_supported,
        }]
    }

    fn set_rumble(&mut self, _id: HostPadId, rumble: Rumble) {
        self.lock().rumble = rumble;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::{AnalogButton, Buttons};
    use crate::host::ConvertConfig;

    fn gp(k: &KeyboardMouse) -> crate::gamepad::Gamepad {
        k.state().to_gamepad(&ConvertConfig::default(), false)
    }

    #[test]
    fn keys_drive_buttons_and_sticks() {
        let k = KeyboardMouse::new(Bindings::default());
        k.key_down("Space");
        k.key_down("KeyW");
        k.key_down("D");
        k.mouse_button(MouseButton::Left, true);
        let g = gp(&k);
        assert_eq!(g.analog(AnalogButton::A), 255);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 255);
        // W+D diagonal is normalised, not sqrt(2) long.
        let m = f64::from(g.thumb_lx).hypot(f64::from(g.thumb_ly));
        assert!((m - 32767.0).abs() < 2.0, "{m}");
        assert!(g.thumb_ly > 0 && g.thumb_lx > 0);
        k.key_up("space");
        assert_eq!(gp(&k).analog(AnalogButton::A), 0);
    }

    #[test]
    fn opposite_keys_cancel() {
        let k = KeyboardMouse::new(Bindings::default());
        k.key_down("a");
        k.key_down("d");
        assert_eq!(gp(&k).thumb_lx, 0);
    }

    #[test]
    fn mouse_motion_is_consumed_and_signed() {
        let k = KeyboardMouse::new(Bindings::default());
        k.mouse_motion(5.0, -10.0); // right, up
        let g = gp(&k);
        assert_eq!(g.thumb_rx, (5.0f32 * 0.04 * 32767.0).round() as i16);
        assert_eq!(g.thumb_ry, (10.0f32 * 0.04 * 32767.0).round() as i16);
        let g = gp(&k);
        assert_eq!((g.thumb_rx, g.thumb_ry), (0, 0));
        k.mouse_motion(1000.0, 0.0);
        assert_eq!(gp(&k).thumb_rx, 32767);
    }

    #[test]
    fn invert_y_and_disabled() {
        let b = Bindings::from_toml("[mouse]\ninvert_y = true").unwrap();
        let mut k = KeyboardMouse::new(b);
        k.mouse_motion(0.0, -10.0);
        assert!(gp(&k).thumb_ry < 0);
        k.key_down("Escape");
        assert!(gp(&k).buttons.contains(Buttons::START));
        k.set_enabled(false);
        assert!(k.poll().is_empty());
        k.set_enabled(true);
        assert!(
            !gp(&k).buttons.contains(Buttons::START),
            "released on disable"
        );
    }

    #[test]
    fn unknown_key_names_are_reported() {
        let k = KeyboardMouse::new(Bindings::default());
        assert!(!k.key_down("Nonsense"));
        assert!(k.key_down("KeyQ"));
    }
}
