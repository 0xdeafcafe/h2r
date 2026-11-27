//! The particle stage's device adapter lives in `native_particles_admission`,
//! which binds the lane to D3D's DrawAdmission. Original guest updates and
//! attribute carry always execute. The particle walk's host
//! (`native_particle_systems`) publishes the walks answered in Rust as GPU
//! particle systems.

/// Installs both on `device`.
pub fn install<B: d3d::backend::Backend + 'static>(device: &d3d::abi::SharedDevice<B>) {
    crate::native_particles_admission::install(device);
    crate::native_particle_systems::install();
}
