//! Particle lane's integration seam. An adapter supplied by the confirmed
//! D3D handoff installs `ImmediateAdmission`; this module never uses the old
//! enqueue-is-success or prefix-dropping swallow contracts.
//!
//! Required adapter guarantees are in docs/graphics/native-particles.md.
//! Until installed the feature only decodes; all guest draws survive.

use engine::memory::GuestMemory;
use feature::native_particles::{Call, Host, Mode};
use native_render::particle_gpu::{ImmediateDrawSnapshot, ParticleRenderer, PreparedDraw, Refused};
use native_render::particles::{Declaration, Emission, QUAD_INDICES, differing_registers};
use std::sync::Arc;

/// Rebind this small interface to the handoff lane at integration. Capture
/// must be scoped by view + call/return, with no draw matched by shape alone.
pub trait ImmediateAdmission: Send + Sync {
    fn arm_particle(&self, mem: &mut dyn GuestMemory, call: &Call, mode: Mode) -> bool;
}
struct Glue(Arc<dyn ImmediateAdmission>);
impl Host for Glue {
    fn arm(&self, mem: &mut dyn GuestMemory, call: &Call, mode: Mode) -> bool {
        self.0.arm_particle(mem, call, mode)
    }
}
pub fn install(adapter: Arc<dyn ImmediateAdmission>) {
    feature::native_particles::set_host(Arc::new(Glue(adapter)));
}

/// Compare the independently decoded emission with the captured D3D draw.
/// Input `snapshot.vertices` is the D3D declaration's effective registers,
/// not the native expansion. Identical shaders alone do not pair a draw.
/// The adapter must establish the call identity before invoking this.
pub fn compare(
    snapshot: &ImmediateDrawSnapshot,
    emission: &Emission,
    declaration: Declaration,
) -> Result<u16, Refused> {
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
