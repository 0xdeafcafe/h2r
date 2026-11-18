//! Host input backends. Each lives in its own module so a tvOS or Xbox
//! build can swap them out without touching the device model.

pub mod fallback;
pub mod keyboard;
pub mod multi;
pub mod remote;
pub mod scripted;

#[cfg(feature = "gilrs")]
pub mod gilrs_pads;

#[cfg(feature = "game-controller")]
pub mod game_controller;

use crate::gamepad::Rumble;
use crate::host::{HostPad, HostPadId};

/// A source of host pads.
pub trait HostBackend: Send {
    fn name(&self) -> &str;

    /// Refresh and return every currently connected pad, in stable order.
    /// Called once per [`InputManager::sync`](crate::InputManager::sync).
    fn poll(&mut self) -> Vec<HostPad>;

    /// The guest read pad `id`'s state through `XInputGetState` since the last
    /// sync, so whatever that state carried has been seen. Called at the start
    /// of the next [`InputManager::sync`](crate::InputManager::sync), before
    /// `poll`, at most once per sync. Default: nothing to consume.
    fn state_read(&mut self, _id: HostPadId) {}

    /// Drive the pad's rumble motors. Default: no rumble.
    fn set_rumble(&mut self, _id: HostPadId, _rumble: Rumble) {}

    /// Pad `id` is on controller `port` (0 to 3). Called at the end of every
    /// [`InputManager::sync`](crate::InputManager::sync) for each of this
    /// backend's pads that has a port. Default: nothing to note.
    fn placed(&mut self, _id: HostPadId, _port: usize) {}

    /// The port (0 to 3) pad `id` should go on when it is inserted, if that
    /// port is free; otherwise it takes the lowest free one. Default: no
    /// preference.
    fn preferred_port(&self, _id: HostPadId) -> Option<usize> {
        None
    }
}
