//! Particle comparison and preparation helpers. The concrete adapter in
//! `native_particles_admission` binds the lane to D3D's DrawAdmission.
//! Original guest updates and attribute carry always execute.

use feature::native_particles::Mode;
use native_render::particle_gpu::{ImmediateDrawSnapshot, ParticleRenderer, PreparedDraw, Refused};
use native_render::particles::{Declaration, Emission, QUAD_INDICES, differing_registers};

pub use crate::native_particles_admission::install;

/// Compare the independently decoded emission with the captured D3D draw.
/// Input `snapshot.vertices` is the D3D declaration's effective registers,
/// not the native expansion. Identical shaders alone do not pair a draw.
/// The adapter must establish the call identity before invoking this.
pub fn compare(
    snapshot: &ImmediateDrawSnapshot,
    emission: &Emission,
    declaration: Declaration,
) -> Result<u16, Refused> {
    if snapshot.raw_vertices.is_some()
        || snapshot.primitive.topology != wgpu::PrimitiveTopology::TriangleList
    {
        return Err(Refused::Geometry);
    }
    let expected = emission
        .vertices(declaration, snapshot.vertex_uniforms.defaults)
        .map_err(|_| Refused::Geometry)?;
    if snapshot.indices != QUAD_INDICES {
        return Err(Refused::Geometry);
    }
    differing_registers(&expected, &snapshot.vertices).map_err(|_| Refused::Geometry)
}

/// Called at the original draw's ordered issue boundary for each target
/// copy. Compare mode prints the exact identity and register difference
/// mask, then returns no replacement. Native mode refuses every mismatch.
/// This never records, suppresses, or mutates the guest's particle state.
pub fn prepare_particle(
    renderer: &mut ParticleRenderer,
    device: &wgpu::Device,
    snapshot: &ImmediateDrawSnapshot,
    emission: &Emission,
    declaration: Declaration,
    mode: Mode,
) -> Result<Option<PreparedDraw>, Refused> {
    let result = compare(snapshot, emission, declaration);
    if mode == Mode::Compare {
        eprintln!(
            "native-particles-compare: identity={:?} target={:?} geometry={result:?} vertices={} indices={} textures={:?}",
            snapshot.identity,
            snapshot.target,
            snapshot.vertices.len(),
            snapshot.indices.len(),
            snapshot.textures.each_ref().map(|t| t.as_ref().map(|t| (
                t.identity,
                t.generation,
                t.dimension
            )))
        );
        return result.map(|_| None);
    }
    match result? {
        0 => renderer.prepare(device, snapshot).map(Some),
        _ => Err(Refused::Geometry),
    }
}
