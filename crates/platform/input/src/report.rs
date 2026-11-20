//! The host pads as the input layer sees them, for `h2 agent hostpads`
//! (`input.host`, docs/runtime/agent-api.md).
//!
//! Two views, refreshed at every [`InputManager::sync`]:
//!
//! * [`HostReport::devices`]: each controller as the backend that reads it
//!   reports it, before any merging: for GameController, the profile class
//!   (DualSense, DualShock 4, plain extended, micro) and whether it has a
//!   touchpad ([`HostBackend::devices`]).
//! * [`HostReport::pads`]: each pad the manager polled, as the
//!   [`PadGate`] (the debug menu) is handed it, and the port it is on. With
//!   the keyboard on, player one is one merged pad here, so a touchpad that
//!   shows under `devices` but not under `pads` was lost in between.
//!
//! [`InputManager::sync`]: crate::InputManager::sync
//! [`HostBackend::devices`]: crate::HostBackend::devices
//! [`PadGate`]: crate::PadGate

use std::sync::{Arc, Mutex, MutexGuard};

use crate::host::HostPadState;

/// One controller as its own backend reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    /// The backend that reads it (`game-controller`, ...).
    pub backend: String,
    /// Its id within that backend.
    pub id: u64,
    pub name: String,
    /// The backend's own word for the kind of pad: for GameController the
    /// profile class (`dualsense`, `dualshock4`, `extended`, `micro`).
    pub profile: Option<String>,
    /// The platform's product category (GameController's
    /// `productCategory`, such as "DualShock 4").
    pub category: Option<String>,
    pub state: HostPadState,
}

/// One pad as the manager polled it, before the gate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadInfo {
    /// The top-level backend's name (`keyboard-fallback`, `multi`, ...).
    pub backend: String,
    /// The gate's key: backend index and the pad's id within it.
    pub key: (usize, u64),
    pub name: String,
    pub physical: bool,
    /// The controller port (0 to 3) it is on, if any.
    pub port: Option<usize>,
    /// What the gate was handed.
    pub state: HostPadState,
}

/// Both views, as of the last sync.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostReport {
    /// Syncs since the manager was made.
    pub syncs: u64,
    pub devices: Vec<DeviceInfo>,
    pub pads: Vec<PadInfo>,
}

/// The report, shared between the manager (which writes it) and whoever
/// reads it. Cloning shares it.
#[derive(Clone, Debug, Default)]
pub struct HostReportHandle(Arc<Mutex<HostReport>>);

impl HostReportHandle {
    fn lock(&self) -> MutexGuard<'_, HostReport> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of the last sync's report.
    pub fn get(&self) -> HostReport {
        self.lock().clone()
    }

    pub(crate) fn set(&self, devices: Vec<DeviceInfo>, pads: Vec<PadInfo>) {
        let mut r = self.lock();
        r.syncs += 1;
        r.devices = devices;
        r.pads = pads;
    }
}
