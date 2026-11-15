//! Which input device the player touched last.
//!
//! The combined player-one pad ([`crate::backend::fallback::KeyboardFallback`])
//! records it on every poll; the runner reads it to choose which button
//! icons the game draws (`docs/button-icons.md`).
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::gamepad::Gamepad;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    /// An Xbox-style pad, or one that could not be told apart.
    Xbox,
    PlayStation,
}

/// A shared cell holding the last-used device. Clones see the same value.
#[derive(Clone, Debug, Default)]
pub struct ActiveDevice(Arc<AtomicU8>);

impl ActiveDevice {
    pub fn set(&self, kind: DeviceKind) {
        let v = match kind {
            DeviceKind::Keyboard => 1,
            DeviceKind::Xbox => 2,
            DeviceKind::PlayStation => 3,
        };
        self.0.store(v, Ordering::Relaxed);
    }

    /// `None` until some device has been used.
    pub fn get(&self) -> Option<DeviceKind> {
        match self.0.load(Ordering::Relaxed) {
            1 => Some(DeviceKind::Keyboard),
            2 => Some(DeviceKind::Xbox),
            3 => Some(DeviceKind::PlayStation),
            _ => None,
        }
    }
}

/// The kind of a physical pad, from the name its backend reports.
pub fn pad_kind(name: &str) -> DeviceKind {
    let n = name.to_ascii_lowercase();
    let sony = [
        "dualshock",
        "dualsense",
        "playstation",
        "sony",
        "ps4",
        "ps5",
    ];
    if sony.iter().any(|s| n.contains(s)) {
        DeviceKind::PlayStation
    } else {
        DeviceKind::Xbox
    }
}

/// Whether a pad state shows deliberate input: a button down, a trigger or
/// face button pressed past a light touch, or a stick well off centre (so a
/// drifting stick on an idle pad does not claim the device).
pub fn is_active(pad: &Gamepad) -> bool {
    const PRESSURE: u8 = 32;
    const STICK: i32 = 12_000;
    let stick = |x: i16, y: i16| i32::from(x).abs() > STICK || i32::from(y).abs() > STICK;
    pad.buttons.0 != 0
        || pad.analog.iter().any(|p| *p > PRESSURE)
        || stick(pad.thumb_lx, pad.thumb_ly)
        || stick(pad.thumb_rx, pad.thumb_ry)
}

/// Fresh deliberate pad activity. Held buttons and small stick jitter do not
/// take the prompts back after a keyboard or mouse event.
pub fn new_activity(previous: &Gamepad, now: &Gamepad) -> bool {
    if !is_active(now) {
        return false;
    }
    if !is_active(previous) || now.buttons.0 & !previous.buttons.0 != 0 {
        return true;
    }
    if now
        .analog
        .iter()
        .zip(previous.analog)
        .any(|(n, p)| i16::from(*n) - i16::from(p) > 32)
    {
        return true;
    }
    [
        (
            (now.thumb_lx, now.thumb_ly),
            (previous.thumb_lx, previous.thumb_ly),
        ),
        (
            (now.thumb_rx, now.thumb_ry),
            (previous.thumb_rx, previous.thumb_ry),
        ),
    ]
    .into_iter()
    .any(|((x, y), (px, py))| {
        (i32::from(x).abs() > 12_000 || i32::from(y).abs() > 12_000)
            && ((i32::from(x) - i32::from(px)).abs() > 8_000
                || (i32::from(y) - i32::from(py)).abs() > 8_000)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::Buttons;

    #[test]
    fn cell_starts_empty_and_clones_share_it() {
        let a = ActiveDevice::default();
        let b = a.clone();
        assert_eq!(a.get(), None);
        for kind in [
            DeviceKind::Keyboard,
            DeviceKind::Xbox,
            DeviceKind::PlayStation,
        ] {
            b.set(kind);
            assert_eq!(a.get(), Some(kind));
        }
    }

    #[test]
    fn sony_pads_are_recognised_by_name() {
        for name in [
            "DualSense Wireless Controller",
            "Sony DUALSHOCK 4",
            "PS5 Controller",
        ] {
            assert_eq!(pad_kind(name), DeviceKind::PlayStation, "{name}");
        }
        for name in ["Xbox Wireless Controller", "Generic USB Joystick", ""] {
            assert_eq!(pad_kind(name), DeviceKind::Xbox, "{name}");
        }
    }

    #[test]
    fn activity_needs_a_real_press_or_deflection() {
        let idle = Gamepad::default();
        assert!(!is_active(&idle));
        // Drift and a resting finger are not activity.
        assert!(!is_active(&Gamepad {
            thumb_lx: 3000,
            thumb_ry: -9000,
            ..idle
        }));
        let mut touch = idle;
        touch.analog[6] = 20;
        assert!(!is_active(&touch));
        assert!(is_active(&Gamepad {
            buttons: Buttons::START,
            ..idle
        }));
        assert!(is_active(&Gamepad {
            thumb_rx: -20_000,
            ..idle
        }));
        assert!(is_active(&Gamepad {
            thumb_ly: i16::MIN,
            ..idle
        }));
        let mut press = idle;
        press.analog[0] = 255;
        assert!(is_active(&press));
    }
}
