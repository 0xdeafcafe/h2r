//! The effects' host (`render_bridge::native_particles`): particles,
//! contrails, effect quads, the immediate effect leaves and the flares'
//! fans. A cut setup's Rust material is made and held
//! ([`material_proof`]); the draws it takes come from the material's
//! guest render words and program, which no native stage draws yet, so
//! each draw site panics by name.

use std::sync::Arc;

use engine::memory::GuestMemory;
use native_render::effect_geometry::DynamicCall;
use native_render::effect_immediate::Site;
use native_render::flares;
use render_bridge::effect_materials::Kind;
use render_bridge::native_particles::{Call, Host, Producer};

mod material_proof;
pub(crate) mod mirror_host;

struct Effects;

impl Host for Effects {
    fn try_emit(&self, _: &mut dyn GuestMemory, _: &Call) -> bool {
        let Some(_) = material_proof::held(Kind::Particle) else {
            unimplemented!("particle_material: an emission before any particle setup");
        };
        unimplemented!("particles: an emission drawn from the held particle material")
    }

    fn try_dynamic(&self, _: &mut dyn GuestMemory, _: u32, _: Option<u32>) -> bool {
        unimplemented!("particles: a contrail or effect quad drawn from its generated stream")
    }

    fn material_cut(&self, mem: &mut dyn GuestMemory, kind: Kind) -> Result<(), &'static str> {
        material_proof::hold(kind, material_proof::cut(mem, kind)?);
        Ok(())
    }

    fn arm(&self, _: &mut dyn GuestMemory, _: &Call) -> bool {
        unimplemented!("particles: a particle call's guest draw")
    }

    fn arm_dynamic(&self, _: &mut dyn GuestMemory, call: &DynamicCall) -> bool {
        unimplemented!("particles: a {:?} call's guest draw", call.kind)
    }

    fn arm_effect(&self, _: &mut dyn GuestMemory, site: Site) -> bool {
        unimplemented!("particles: the immediate effect leaf {site:?}'s guest draw")
    }

    fn arm_flare(&self, _: &mut dyn GuestMemory, site: flares::Site) {
        unimplemented!("particles: the flare fan at {site:?}'s guest draw")
    }

    fn publish(&self, _: &mut dyn GuestMemory, _: Producer, _: u32) {}

    fn finish(&self, _: &mut dyn GuestMemory, _: Producer) {}
}

/// Installs the effects' host and the particle mirror's.
pub fn install() {
    render_bridge::native_particles::set_host(Arc::new(Effects));
    mirror_host::install();
}
