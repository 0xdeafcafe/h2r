//! Host input backends. Each lives in its own module so a tvOS or Xbox
//! build can swap them out without touching the device model.

pub mod fallback;
pub mod keyboard;
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
}
