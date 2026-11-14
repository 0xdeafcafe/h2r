//! Xbox controller input for h2recomp.
//!
//! Layers, bottom to top:
//!
//! * [`gamepad`]: the data the game sees (`XINPUT_GAMEPAD`, `XINPUT_STATE`,
//!   `XINPUT_CAPABILITIES`, rumble/feedback).
//! * [`host`]: what a host backend reports (normalised floats) and the
//!   conversion into Xbox units, including deadzones.
//! * [`backend`]: [`HostBackend`] implementations (gilrs pads, keyboard+mouse).
//! * [`device`]: the four-port device model that the `XInput*` API replacement
//!   sits on top of.
//! * [`bindings`]: the TOML binding table for the keyboard+mouse backend.
//!
//! See `docs/input.md`.

pub mod backend;
pub mod bindings;
pub mod device;
pub mod gamepad;
pub mod host;

pub use backend::HostBackend;
pub use device::{InputManager, PortIndex};
pub use gamepad::{AnalogButton, Buttons, Gamepad, Rumble, State};
pub use host::{HostPad, HostPadId, HostPadState};
