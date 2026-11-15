//! Host input backends. Each lives in its own module so a tvOS or Xbox
//! build can swap them out without touching the device model.

pub mod keyboard;

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

    /// Drive the pad's rumble motors. Default: no rumble.
    fn set_rumble(&mut self, _id: HostPadId, _rumble: Rumble) {}
}
