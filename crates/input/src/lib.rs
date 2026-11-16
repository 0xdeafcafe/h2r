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
//! * [`layouts`]: Halo 2 button layouts (retail and added) as action tables.
//! * [`dualsense`]: DualSense HID reports for the mute button, mute LED and
//!   speaker path (`docs/audio/playstation-audio.md`).
//! * [`pad_hid`]: the DualSense opened over raw HID next to the gamepad
//!   backends, sending those reports (IOKit on macOS).
//! * [`steam`], [`steam_pad`], [`trackpad`], [`haptics`]: Steam Controllers
//!   over raw HID as pads of their own, the right trackpad aiming through
//!   [`look`] (the mouse-look path), and the trackpad actuators as rumble
//!   and a speaker (`docs/input/steam-controller.md`).
//!
//! See `docs/input/input.md`.

pub mod active;
pub mod backend;
pub mod bindings;
pub mod device;
pub mod dualsense;
pub mod gamepad;
pub mod haptics;
pub mod host;
pub mod layouts;
pub mod look;
pub mod pad_hid;
pub mod steam;
pub mod steam_pad;
pub mod trackpad;

pub use backend::HostBackend;
pub use device::{InputManager, PortIndex};
pub use gamepad::{AnalogButton, Buttons, Gamepad, Rumble, State};
pub use host::{HostPad, HostPadId, HostPadState};
