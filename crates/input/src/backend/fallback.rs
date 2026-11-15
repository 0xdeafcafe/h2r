//! Keyboard/mouse and the first physical controller share player one.
use std::collections::HashMap;

use crate::HostBackend;
use crate::backend::keyboard::KeyboardMouse;
use crate::gamepad::{Gamepad, Rumble};
use crate::host::{ConvertConfig, HostPad, HostPadId, HostPadState};

/// One persistent virtual pad plus the remaining physical controllers.
/// The virtual pad accepts feedback even with no motors attached. With the
/// keyboard disabled, attach physical backends directly for native hotplug.
pub struct KeyboardFallback {
    keyboard: KeyboardMouse,
    physical: Option<Box<dyn HostBackend>>,
    convert: ConvertConfig,
    primary: Option<HostPadId>,
    ids: HashMap<HostPadId, HostPadId>,
    next_id: u64,
    rumble: Rumble,
}

impl KeyboardFallback {
    pub fn new(
        keyboard: KeyboardMouse,
        physical: Option<Box<dyn HostBackend>>,
        convert: ConvertConfig,
    ) -> Self {
        Self {
            keyboard,
            physical,
            convert,
            primary: None,
            ids: HashMap::new(),
            next_id: 1,
            rumble: Rumble::default(),
        }
    }
}

fn merge(mut pad: Gamepad, keyboard: Gamepad) -> Gamepad {
    pad.buttons = pad.buttons | keyboard.buttons;
    for (pressure, key) in pad.analog.iter_mut().zip(keyboard.analog) {
        *pressure = (*pressure).max(key);
    }
    // Pick a whole stick by deflection; preserve its direction. Keyboard wins
    // a tie, so a full key press can override a pad held at its endpoint.
    let strength = |x: i16, y: i16| i64::from(x).pow(2) + i64::from(y).pow(2);
    if strength(keyboard.thumb_lx, keyboard.thumb_ly) >= strength(pad.thumb_lx, pad.thumb_ly) {
        (pad.thumb_lx, pad.thumb_ly) = (keyboard.thumb_lx, keyboard.thumb_ly);
    }
    if strength(keyboard.thumb_rx, keyboard.thumb_ry) >= strength(pad.thumb_rx, pad.thumb_ry) {
        (pad.thumb_rx, pad.thumb_ry) = (keyboard.thumb_rx, keyboard.thumb_ry);
    }
    pad
}

impl HostBackend for KeyboardFallback {
    fn name(&self) -> &str {
        "keyboard-fallback"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        let mut pads = self.physical.as_mut().map_or_else(Vec::new, |p| p.poll());
        let selected = self
            .primary
            .and_then(|id| pads.iter().position(|p| p.id == id))
            .or_else(|| (!pads.is_empty()).then_some(0));
        let primary = selected.map(|i| pads.remove(i));
        let new_id = primary.as_ref().map(|p| p.id);
        if self.primary != new_id {
            if let Some(backend) = &mut self.physical {
                if let Some(old) = self.primary {
                    backend.set_rumble(old, Rumble::default());
                }
                if let Some(pad) = &primary
                    && pad.supports_rumble
                {
                    backend.set_rumble(pad.id, self.rumble);
                }
            }
            self.primary = new_id;
        }
        self.ids
            .retain(|id, _| pads.iter().any(|p| p.id == *id) || Some(*id) == self.primary);
        let keyboard = self
            .keyboard
            .poll()
            .pop()
            .map_or_else(Gamepad::default, |pad| {
                pad.state.to_gamepad(&self.convert, false)
            });
        let gamepad = primary.as_ref().map_or_else(Gamepad::default, |p| {
            p.state.to_gamepad(&self.convert, p.physical)
        });
        let mut out = vec![HostPad {
            id: HostPadId(0),
            name: "Player one (pad + keyboard/mouse)".into(),
            state: HostPadState::from_gamepad(merge(gamepad, keyboard)),
            physical: false,
            supports_rumble: true,
        }];
        for mut pad in pads {
            pad.id = *self.ids.entry(pad.id).or_insert_with(|| {
                let id = HostPadId(self.next_id);
                self.next_id += 1;
                id
            });
            out.push(pad);
        }
        out
    }

    fn state_read(&mut self, id: HostPadId) {
        if id == HostPadId(0) {
            self.keyboard.state_read(HostPadId(0));
            if let Some(backend) = &mut self.physical
                && let Some(primary) = self.primary
            {
                backend.state_read(primary);
            }
        } else if let Some(backend) = &mut self.physical
            && let Some((host, _)) = self.ids.iter().find(|(_, mapped)| **mapped == id)
        {
            backend.state_read(*host);
        }
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        if id == HostPadId(0) {
            self.rumble = rumble;
            if let Some(backend) = &mut self.physical
                && let Some(primary) = self.primary
            {
                backend.set_rumble(primary, rumble);
            }
        } else if let Some(backend) = &mut self.physical
            && let Some((host, _)) = self.ids.iter().find(|(_, mapped)| **mapped == id)
        {
            backend.set_rumble(*host, rumble);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::scripted::Timeline;
    use crate::bindings::Bindings;
    use crate::device::FeedbackRequest;
    use crate::{InputManager, PortIndex};

    #[test]
    fn keyboard_and_pad_share_port_zero_and_survive_hotplug() {
        let script = Timeline::from_toml("[[frame]]\nat_ticks=0\nbuttons=['b']\nleft_stick=[10000,0]\n[[frame]]\nat_ticks=10\nconnected=false\n[[frame]]\nat_ticks=20\nbuttons=['x']").unwrap().playback();
        let keyboard = KeyboardMouse::new(Bindings::default());
        let mut manager = InputManager::new(ConvertConfig::default()).unwrap();
        manager.add_backend(Box::new(KeyboardFallback::new(
            keyboard.clone(),
            Some(Box::new(script.clone())),
            ConvertConfig::default(),
        )));
        keyboard.key_down("Space");
        keyboard.key_down("W");
        manager.sync();
        let h = manager.open(PortIndex::new(0).unwrap()).unwrap();
        let pad = manager.get_state(h).unwrap().gamepad;
        assert_eq!(pad.analog[..2], [255, 255]);
        assert_eq!((pad.thumb_lx, pad.thumb_ly), (0, 32767));
        keyboard.release_all();
        manager.sync();
        assert_eq!(manager.get_state(h).unwrap().gamepad.thumb_lx, 10000);
        manager
            .set_state(
                h,
                FeedbackRequest {
                    addr: 1,
                    h_event: 0,
                    rumble: Rumble {
                        left_motor: 123,
                        right_motor: 456,
                    },
                },
            )
            .unwrap();
        manager.sync();
        assert_eq!(
            script.rumble(),
            Rumble {
                left_motor: 123,
                right_motor: 456
            }
        );
        assert_eq!(manager.take_completions()[0].status, 0);
        manager.device_changes();
        script.advance_to(10).unwrap();
        manager.sync();
        assert_eq!(manager.get_state(h).unwrap().gamepad, Gamepad::default());
        assert_eq!(manager.connected_mask(), 1);
        assert_eq!(
            manager.device_changes(),
            (0, 0),
            "keyboard pad stays connected"
        );
        script.advance_to(20).unwrap();
        manager.sync();
        assert_eq!(manager.get_state(h).unwrap().gamepad.analog[2], 255);
        assert_eq!(
            script.rumble().left_motor,
            123,
            "restore last output on attach"
        );
    }
    #[test]
    fn extra_ports_keep_identity_and_rumble_routes_to_their_physical_pad() {
        use std::sync::{Arc, Mutex};
        #[derive(Default)]
        struct Shared {
            pads: Vec<HostPad>,
            rumble: Vec<(HostPadId, Rumble)>,
        }
        struct Physical(Arc<Mutex<Shared>>);
        impl HostBackend for Physical {
            fn name(&self) -> &str {
                "test"
            }
            fn poll(&mut self) -> Vec<HostPad> {
                self.0.lock().unwrap().pads.clone()
            }
            fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
                self.0.lock().unwrap().rumble.push((id, rumble));
            }
        }
        let shared = Arc::new(Mutex::new(Shared {
            pads: (0..5)
                .map(|i| HostPad {
                    id: HostPadId(i),
                    name: format!("pad{i}"),
                    state: HostPadState::from_gamepad(Gamepad {
                        thumb_lx: i as i16 * 1000,
                        ..Default::default()
                    }),
                    physical: true,
                    supports_rumble: true,
                })
                .collect(),
            ..Default::default()
        }));
        let keyboard = KeyboardMouse::new(Bindings::default());
        keyboard.set_enabled(false);
        let mut manager = InputManager::new(Default::default()).unwrap();
        manager.add_backend(Box::new(KeyboardFallback::new(
            keyboard,
            Some(Box::new(Physical(shared.clone()))),
            ConvertConfig::default(),
        )));
        manager.sync();
        assert_eq!(manager.connected_mask(), 15);
        let h0 = manager.open(PortIndex::new(0).unwrap()).unwrap();
        let h1 = manager.open(PortIndex::new(1).unwrap()).unwrap();
        let h2 = manager.open(PortIndex::new(2).unwrap()).unwrap();
        manager.device_changes();
        let rumble = Rumble {
            left_motor: 321,
            right_motor: 456,
        };
        manager
            .set_state(
                h2,
                FeedbackRequest {
                    addr: 1,
                    h_event: 0,
                    rumble,
                },
            )
            .unwrap();
        manager.sync();
        assert!(
            shared
                .lock()
                .unwrap()
                .rumble
                .contains(&(HostPadId(2), rumble))
        );
        shared.lock().unwrap().pads.remove(0);
        manager.sync();
        assert_eq!(
            manager.device_changes(),
            (2, 2),
            "waiting fifth pad fills the promoted pad's old port"
        );
        assert!(manager.get_state(h1).is_err());
        assert_eq!(manager.get_state(h0).unwrap().gamepad.thumb_lx, 1000);
        assert_eq!(manager.get_state(h2).unwrap().gamepad.thumb_lx, 2000);
        shared.lock().unwrap().pads.reverse();
        manager.sync();
        assert_eq!(
            manager.get_state(h0).unwrap().gamepad.thumb_lx,
            1000,
            "retain selected primary across enumeration reordering"
        );
    }
}
