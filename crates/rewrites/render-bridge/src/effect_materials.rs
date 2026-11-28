//! The particle and contrail material setups, in Rust: the materials
//! (`native_render::particle_material`, `native_render::contrail_material`)
//! replace the device state the guest's setups leave
//! (docs/graphics/retire-d3d-r2.md, "R2's materials").
//!
//! Two observers read the setups' inputs at their entries: the particle
//! draw `0x002243c0` (held until the next draw: every particle of the
//! walk is drawn under it) and the contrail setup `0x00043720` (held for
//! the one dynamic draw whose context is its entry data). At every
//! setup's call the host makes the Rust material for the device's
//! supplied draw (`Host::material_cut`) and the setup's body does not
//! run. The guest's setup never runs: a material the host cannot make,
//! or a call from a caller not modelled, panics with its cause
//! (`unimplemented!("<kind>_material: <cause>")`).

use std::sync::Mutex;

use guest_layout::memory::GuestMemory;
use guest_layout::points::{Loc, Point, Sig};
use hooks::Builder;
use native_render::{contrail_material, particle_material};

/// The particle setup `0x0023ea80` and the return of its call in the
/// particle draw (`ret 0xc`, AL whether the walk runs).
pub const PARTICLE_SETUP: Point = Point::new(
    "PARTICLE_SETUP",
    0x0023_ea80,
    Sig::Fn { args: &[("a0", Loc::Stack(0)), ("a1", Loc::Stack(1)), ("a2", Loc::Stack(2))], ret: Some(Loc::Al), pop: 12 },
    DOC,
);
pub const PARTICLE_SETUP_RETURN: u32 = 0x0022_441e;
/// The polygon handler's return from its entry's setup (`call eax`).
pub const CONTRAIL_SETUP_RETURN: u32 = 0x0003_5b23;

const DOC: &str = "docs/graphics/retire-d3d-r2.md";
pub const PARTICLE_MATERIAL: Point = Point::new("PARTICLE_MATERIAL", particle_material::DRAW, Sig::Opaque, DOC);
pub const CONTRAIL_MATERIAL: Point =
    Point::new("CONTRAIL_MATERIAL", contrail_material::SETUP, Sig::Fn { args: &[("entry", Loc::Stack(0))], ret: None, pop: 4 }, DOC);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Particle,
    Contrail,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Particle => "particle_material",
            Self::Contrail => "contrail_material",
        }
    }
}

static PARTICLE: Mutex<Option<particle_material::Inputs>> = Mutex::new(None);
static CONTRAIL: Mutex<Option<(u32, contrail_material::Inputs)>> = Mutex::new(None);

/// The inputs of the particle draw running now; `None` before the first
/// or when they did not read.
pub fn particle_inputs() -> Option<particle_material::Inputs> {
    *PARTICLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The contrail setup's inputs for the draw whose builder context is
/// `context` (the setup's entry data), taken: a later draw does not see
/// them.
pub fn take_contrail_inputs(context: u32) -> Option<contrail_material::Inputs> {
    let mut held = CONTRAIL.lock().unwrap_or_else(|e| e.into_inner());
    match held.take() {
        Some((e, inputs)) if e == context => Some(inputs),
        _ => None,
    }
}

/// The contrail setup's inputs held now, not taken.
pub fn contrail_inputs() -> Option<contrail_material::Inputs> {
    let held = *CONTRAIL.lock().unwrap_or_else(|e| e.into_inner());
    held.map(|(_, i)| i)
}

fn on_particle(mem: &dyn GuestMemory, esp: u32, eax: u32) {
    let inputs = particle_material::read(&|a| mem.u32(a).ok(), esp, eax);
    *PARTICLE.lock().unwrap_or_else(|e| e.into_inner()) = inputs;
}

/// At a setup's call (`esp` the return address's slot): the host makes
/// the material and the setup is cut. A call from another caller than
/// the handler's (`from`), or a material the host cannot make, panics.
fn cut(mem: &mut dyn GuestMemory, kind: Kind, esp: u32, from: u32) {
    let prefix = kind.prefix();
    let caller = mem.u32(esp).ok();
    if caller != Some(from) {
        unimplemented!("{prefix}: setup called from {caller:x?}, not {from:#x}");
    }
    let host = crate::native_particles::HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { unimplemented!("{prefix}: no_host") };
    if let Err(cause) = host.material_cut(&mut *mem, kind) {
        unimplemented!("{prefix}: {cause}");
    }
}

fn on_contrail(mem: &dyn GuestMemory, esp: u32) {
    let word = |a: u32| mem.u32(a).ok();
    let held = esp
        .checked_add(4)
        .and_then(word)
        .and_then(|e| contrail_material::read(&word, e).map(|i| (e, i)));
    *CONTRAIL.lock().unwrap_or_else(|e| e.into_inner()) = held;
}

/// The two observers.
pub(crate) fn build(b: &mut Builder) {
    b.raw()
        .observe_before(&PARTICLE_MATERIAL, "effect_materials_particle", |o| {
            let mem = o.guest();
            on_particle(&*mem, o.cpu.esp, o.cpu.eax);
        })
        .context_only()
        .optional();
    // The setup's body never runs: the particle setup returns AL 1
    // (`ret 0xc`), the contrail's returns (`ret 4`).
    b.raw()
        .before(&PARTICLE_SETUP, "effect_materials_particle_setup", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            cut(&mut *rt.memory(), Kind::Particle, c.cpu.esp, PARTICLE_SETUP_RETURN);
            c.cpu.eax = (c.cpu.eax & !0xff) | 1;
            c.ret(12).map(hooks::Flow::Return)
        })
        .context_only()
        .optional();
    b.raw()
        .before(&CONTRAIL_MATERIAL, "effect_materials_contrail", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            on_contrail(&*mem, c.cpu.esp);
            cut(&mut *mem, Kind::Contrail, c.cpu.esp, CONTRAIL_SETUP_RETURN);
            drop(mem);
            c.ret(4).map(hooks::Flow::Return)
        })
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contrails_inputs_reach_only_the_draw_of_its_entry_data_once() {
        let i = contrail_material::Inputs { kind: 1, flags: 0, blend: 0, map_flags: 0, bitmap: 2, image: 0, options: 0 };
        *CONTRAIL.lock().unwrap() = Some((0x100, i));
        assert_eq!(take_contrail_inputs(0x200), None);
        *CONTRAIL.lock().unwrap() = Some((0x100, i));
        assert_eq!(take_contrail_inputs(0x100), Some(i));
        assert_eq!(take_contrail_inputs(0x100), None);
    }
}
