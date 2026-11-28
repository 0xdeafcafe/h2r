//! The particle stage's hosts: the effects' (`native_particles_admission`)
//! and the particle walk's (`native_particle_systems`), which publishes the
//! walks answered in Rust as GPU particle systems.

/// Installs both.
pub fn install() {
    crate::native_particles_admission::install();
    crate::native_particle_systems::install();
}
