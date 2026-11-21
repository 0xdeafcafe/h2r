//! Desired lightbar colours, indexed by Xbox controller port.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::device::{PORT_COUNT, PortIndex};

pub type Rgb = [u8; 3];
pub type PortColours = [Option<Rgb>; PORT_COUNT];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    pub game: PortColours,
    pub overrides: PortColours,
}

impl State {
    pub fn desired(&self) -> PortColours {
        if self.enabled {
            std::array::from_fn(|port| self.overrides[port].or(self.game[port]))
        } else {
            [None; PORT_COUNT]
        }
    }
}

#[derive(Clone, Debug)]
pub struct Lightbars(Arc<Mutex<State>>);

impl Default for Lightbars {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            enabled: true,
            game: [None; PORT_COUNT],
            overrides: [None; PORT_COUNT],
        })))
    }
}

impl Lightbars {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn state(&self) -> State {
        *self.lock()
    }

    pub fn update_game(&self, enabled: bool, game: PortColours) {
        let mut state = self.lock();
        state.enabled = enabled;
        state.game = game;
    }

    /// A session-only override. `None` makes this port follow the game again.
    pub fn set_override(&self, port: PortIndex, rgb: Option<Rgb>) -> State {
        let mut state = self.lock();
        state.overrides[port.get()] = rgb;
        *state
    }
}
