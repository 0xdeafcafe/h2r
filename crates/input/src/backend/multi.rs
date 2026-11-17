//! Several physical backends as one: the platform's pads (GameController or
//! gilrs) and the Steam Controllers (`crate::steam_pad`) behind the one
//! physical slot `KeyboardFallback` takes.

use crate::backend::HostBackend;
use crate::gamepad::Rumble;
use crate::host::{HostPad, HostPadId};

/// The backends in order; their pads in that order. A pad's id carries its
/// backend's index in the top byte, so ids stay unique and stable.
pub struct Multi {
    backends: Vec<Box<dyn HostBackend>>,
}

const SHIFT: u32 = 56;
const LOW: u64 = (1 << SHIFT) - 1;

impl Multi {
    pub fn new(backends: Vec<Box<dyn HostBackend>>) -> Self {
        Self { backends }
    }

    fn route(&mut self, id: HostPadId) -> Option<(&mut Box<dyn HostBackend>, HostPadId)> {
        let i = (id.0 >> SHIFT) as usize;
        self.backends.get_mut(i).map(|b| (b, HostPadId(id.0 & LOW)))
    }
}

impl HostBackend for Multi {
    fn name(&self) -> &str {
        "multi"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        let mut out = Vec::new();
        for (i, b) in self.backends.iter_mut().enumerate() {
            for mut pad in b.poll() {
                pad.id = HostPadId((i as u64) << SHIFT | (pad.id.0 & LOW));
                out.push(pad);
            }
        }
        out
    }

    fn state_read(&mut self, id: HostPadId) {
        if let Some((b, id)) = self.route(id) {
            b.state_read(id);
        }
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        if let Some((b, id)) = self.route(id) {
            b.set_rumble(id, rumble);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostPadState;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<(&'static str, u64)>>>;

    struct Fake {
        name: &'static str,
        ids: Vec<u64>,
        log: Log,
    }

    impl HostBackend for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn poll(&mut self) -> Vec<HostPad> {
            self.ids
                .iter()
                .map(|&id| HostPad {
                    id: HostPadId(id),
                    name: self.name.into(),
                    state: HostPadState::default(),
                    physical: true,
                    supports_rumble: true,
                })
                .collect()
        }
        fn state_read(&mut self, id: HostPadId) {
            self.log.lock().unwrap().push((self.name, id.0));
        }
        fn set_rumble(&mut self, id: HostPadId, _: Rumble) {
            self.log.lock().unwrap().push((self.name, id.0 + 1000));
        }
    }

    #[test]
    fn pads_keep_their_order_and_route_back() {
        let log = Log::default();
        let fake = |name, ids: &[u64]| -> Box<dyn HostBackend> {
            Box::new(Fake {
                name,
                ids: ids.to_vec(),
                log: Arc::clone(&log),
            })
        };
        let mut m = Multi::new(vec![fake("gc", &[1, 2]), fake("steam", &[1])]);
        let pads = m.poll();
        let names: Vec<_> = pads.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["gc", "gc", "steam"]);
        assert_ne!(
            pads[0].id, pads[2].id,
            "the same inner id, different backends"
        );
        m.state_read(pads[2].id);
        m.set_rumble(pads[1].id, Rumble::default());
        m.state_read(HostPadId(9 << SHIFT)); // no such backend
        assert_eq!(*log.lock().unwrap(), [("steam", 1), ("gc", 1002)]);
    }
}
