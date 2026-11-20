//! Physical gamepads through [gilrs](https://crates.io/crates/gilrs).
//!
//! macOS evaluation (gilrs 0.11.2, from its README support table): input and
//! hotplug work (IOKit HID plus the SDL controller mapping database, so Xbox,
//! DualShock/DualSense and most generic pads get the standard layout);
//! **force feedback is not implemented on macOS**. Rumble here is therefore
//! wired through gilrs's effect API and works on Linux and Windows, and is a
//! silent no-op on macOS (`supports_rumble` is false there). If macOS rumble
//! matters, add a second backend on Apple's GameController framework
//! (`GCController`/`CHHapticEngine`); the [`HostBackend`] trait is the seam
//! and that needs Objective-C bindings, which is why it is not done here.

use std::collections::HashMap;

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};

use crate::backend::HostBackend;
use crate::gamepad::Rumble;
use crate::host::{HostButton, HostPad, HostPadId, HostPadState};

/// Per-gamepad connection generation. gilrs reuses a `GamepadId` when the
/// same pad is unplugged and replugged, so the id alone can't tell the device
/// manager that a replug happened between two polls. Every connect and
/// disconnect event bumps the generation, which is folded into the
/// [`HostPadId`] so the manager sees a different pad and reports a
/// removal plus an insertion.
#[derive(Default)]
struct Generations(HashMap<usize, u32>);

impl Generations {
    fn bump(&mut self, index: usize) {
        let g = self.0.entry(index).or_insert(0);
        *g = g.wrapping_add(1);
    }

    fn host_id(&self, index: usize) -> HostPadId {
        let g = self.0.get(&index).copied().unwrap_or(0);
        HostPadId(u64::from(g) << 32 | index as u64)
    }
}

/// Gamepad index from a [`HostPadId`] built by [`Generations::host_id`].
fn index_of(id: HostPadId) -> usize {
    (id.0 & 0xFFFF_FFFF) as usize
}

pub struct GilrsPads {
    gilrs: Gilrs,
    effects: HashMap<GamepadId, Effect>,
    generations: Generations,
}

impl GilrsPads {
    // gilrs::Error is large but public API; boxing it would change the signature.
    #[allow(clippy::result_large_err)]
    pub fn new() -> Result<Self, gilrs::Error> {
        Ok(GilrsPads { gilrs: Gilrs::new()?, effects: HashMap::new(), generations: Generations::default() })
    }
}

fn trigger(g: &gilrs::Gamepad<'_>, b: Button) -> f32 {
    match g.button_data(b) {
        Some(d) => d.value(),
        None => f32::from(u8::from(g.is_pressed(b))),
    }
}

fn read_state(g: &gilrs::Gamepad<'_>) -> HostPadState {
    let map = [
        (Button::South, HostButton::South),
        (Button::East, HostButton::East),
        (Button::West, HostButton::West),
        (Button::North, HostButton::North),
        (Button::LeftTrigger, HostButton::LeftBumper),
        (Button::RightTrigger, HostButton::RightBumper),
        (Button::Select, HostButton::Back),
        (Button::Start, HostButton::Start),
        (Button::LeftThumb, HostButton::LeftStick),
        (Button::RightThumb, HostButton::RightStick),
        (Button::DPadUp, HostButton::DpadUp),
        (Button::DPadDown, HostButton::DpadDown),
        (Button::DPadLeft, HostButton::DpadLeft),
        (Button::DPadRight, HostButton::DpadRight),
    ];
    let mut s = HostPadState::default();
    for (gb, hb) in map {
        s.buttons.set(hb, g.is_pressed(gb));
    }
    s.left_stick = (g.value(Axis::LeftStickX), g.value(Axis::LeftStickY));
    s.right_stick = (g.value(Axis::RightStickX), g.value(Axis::RightStickY));
    s.left_trigger = trigger(g, Button::LeftTrigger2);
    s.right_trigger = trigger(g, Button::RightTrigger2);
    s
}

impl HostBackend for GilrsPads {
    fn name(&self) -> &str {
        "gilrs"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        // Drain events so gilrs updates its cached state and hotplug list.
        while let Some(ev) = self.gilrs.next_event() {
            if matches!(ev.event, EventType::Connected | EventType::Disconnected) {
                self.generations.bump(usize::from(ev.id));
            }
        }
        let mut pads: Vec<HostPad> = self
            .gilrs
            .gamepads()
            .filter(|(_, g)| g.is_connected())
            .map(|(id, g)| HostPad {
                id: self.generations.host_id(usize::from(id)),
                name: g.name().to_string(),
                state: read_state(&g),
                physical: true,
                supports_rumble: g.is_ff_supported(),
            })
            .collect();
        pads.sort_by_key(|p| p.id);
        let live: Vec<GamepadId> = self.gilrs.gamepads().map(|(id, _)| id).collect();
        self.effects.retain(|id, _| live.contains(id));
        pads
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        let Some((gid, _)) = self.gilrs.gamepads().find(|(g, _)| usize::from(*g) == index_of(id)) else {
            return;
        };
        self.effects.remove(&gid); // dropping an effect stops it
        if rumble == Rumble::default() {
            return;
        }
        // Xbox: left motor is the heavy one, right the light one.
        let built = EffectBuilder::new()
            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: rumble.left_motor }, ..Default::default() })
            .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: rumble.right_motor }, ..Default::default() })
            .gamepads(&[gid])
            .finish(&mut self.gilrs);
        if let Ok(effect) = built
            && effect.play().is_ok()
        {
            self.effects.insert(gid, effect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_with_same_gamepad_index_changes_host_id() {
        let mut g = Generations::default();
        let before = g.host_id(3);
        g.bump(3); // disconnect
        g.bump(3); // connect again, same index, between two syncs
        let after = g.host_id(3);
        assert_ne!(before, after);
        assert_eq!(index_of(before), 3);
        assert_eq!(index_of(after), 3);
        assert_eq!(g.host_id(4), Generations::default().host_id(4), "other pads unaffected");
    }
}
