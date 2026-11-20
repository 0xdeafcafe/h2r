//! The particle stage's device adapter lives in `native_particles_admission`,
//! which binds the lane to D3D's DrawAdmission. Original guest updates and
//! attribute carry always execute.

pub use crate::native_particles_admission::install;
