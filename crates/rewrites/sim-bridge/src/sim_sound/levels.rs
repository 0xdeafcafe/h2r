//! The sound phase's [`Levels`](sim_sound::instances::Levels): `0x1251e0`
//! read from guest memory (docs/engine/sound-instances.md, "Levels").
//! [`GuestLevels`] lives in `engine::sim_sound::levels`, beside the lifted
//! original its differential case runs against; the phase takes it from here.

pub use engine::sim_sound::levels::GuestLevels;
