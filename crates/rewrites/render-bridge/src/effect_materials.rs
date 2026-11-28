//! The cut of the particle and contrail material setups: the Rust
//! materials (`native_render::particle_material`,
//! `native_render::contrail_material`) replace the device state the
//! guest's setups leave (docs/graphics/retire-d3d-r2.md, "R2's materials").
//!
//! Two observers read the setups' inputs at their entries: the particle
//! draw `0x002243c0` (held until the next draw: every particle of the
//! walk is drawn under it) and the contrail setup `0x00043720` (held for
//! the one dynamic draw whose context is its entry data). At every
//! setup's call (`0x0023ea80` from `0x00224419`, `0x00043720` from
//! `0x00035b21`) the host makes the Rust material for the device's
//! supplied draw (`Host::material_cut`) and the setup's body does not
//! run; no setup is the guest's for comparison. What the host refuses
//! keeps the guest's setup, counted `scene_kept_15_<kind>_material_<cause>`
//! and `<kind>_material_refused_<cause>`. A cut setup's draw the device
//! then refuses is dropped by name ([`dropped`]). `render.scene` carries
//! `<kind>_material_{cut,refused_<cause>,dropped_<cause>}`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::memory::GuestMemory;
use engine::points::{Loc, Point, Sig};
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

    /// The `scene_kept_15_<kind>_...` kind.
    pub fn name(self) -> &'static str {
        match self {
            Self::Particle => "particle",
            Self::Contrail => "contrail",
        }
    }
}

/// One kind's tally.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cuts {
    /// The setup running now was cut: its draws take the held material.
    under_cut: bool,
    pub cut: u64,
    /// Setups the host refused, by cause: the guest's setup ran.
    pub refused: BTreeMap<&'static str, u64>,
    /// Draws under a cut setup the supplied draw refused, by cause.
    pub dropped: BTreeMap<&'static str, u64>,
}

impl Cuts {
    const fn new() -> Self {
        Self { under_cut: false, cut: 0, refused: BTreeMap::new(), dropped: BTreeMap::new() }
    }

    /// A draw under a cut setup the device did not draw from the
    /// material: its guest setup did not run, so it is dropped by name.
    fn dropped(&mut self, cause: &'static str) {
        *self.dropped.entry(cause).or_default() += 1;
    }

    /// At a setup's call: `host` makes the Rust material and the setup is
    /// cut, or names why not, which keeps the guest's setup.
    pub fn setup(&mut self, host: impl FnOnce() -> Result<(), &'static str>) -> Result<(), &'static str> {
        self.under_cut = false;
        match host() {
            Ok(()) => {
                self.under_cut = true;
                self.cut += 1;
                Ok(())
            }
            Err(cause) => {
                *self.refused.entry(cause).or_default() += 1;
                Err(cause)
            }
        }
    }

    fn counters(&self, prefix: &str) -> Vec<(String, u64)> {
        let mut out = vec![(format!("{prefix}_cut"), self.cut)];
        out.extend(self.refused.iter().map(|(r, n)| (format!("{prefix}_refused_{r}"), *n)));
        out.extend(self.dropped.iter().map(|(r, n)| (format!("{prefix}_dropped_{r}"), *n)));
        out
    }
}

static CUTS: Mutex<[Cuts; 2]> = Mutex::new([Cuts::new(), Cuts::new()]);
static PARTICLE: Mutex<Option<particle_material::Inputs>> = Mutex::new(None);
static CONTRAIL: Mutex<Option<(u32, contrail_material::Inputs)>> = Mutex::new(None);
fn with<T>(kind: Kind, f: impl FnOnce(&mut Cuts) -> T) -> T {
    let mut p = CUTS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut p[kind as usize])
}

/// Whether the setup of `kind` running now was cut: its draws are drawn
/// from the material the host made at the cut ([`Host::material_cut`]).
///
/// [`Host::material_cut`]: crate::native_particles::Host::material_cut
pub fn under_cut(kind: Kind) -> bool {
    with(kind, |p| p.under_cut)
}

/// A draw under a cut setup that the device's supplied draw refused
/// (`<kind>_material_dropped_<cause>`).
pub fn dropped(kind: Kind, cause: &'static str) {
    with(kind, |p| p.dropped(cause));
}

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

/// `render.scene` rows of both kinds.
pub fn counters() -> Vec<(String, u64)> {
    let p = CUTS.lock().unwrap_or_else(|e| e.into_inner());
    [Kind::Particle, Kind::Contrail].iter().flat_map(|&k| p[k as usize].counters(k.prefix())).collect()
}

/// [`counters`] with static names (each distinct name is leaked once).
pub fn rows() -> Vec<(&'static str, u64)> {
    static NAMES: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    counters()
        .into_iter()
        .map(|(n, v)| (*names.entry(n.clone()).or_insert_with(|| Box::leak(n.into_boxed_str())), v))
        .collect()
}

fn on_particle(mem: &dyn GuestMemory, esp: u32, eax: u32) {
    let inputs = particle_material::read(&|a| mem.u32(a).ok(), esp, eax);
    *PARTICLE.lock().unwrap_or_else(|e| e.into_inner()) = inputs;
}

/// At a setup's call from its handler (`esp` the return address's slot):
/// whether the setup is cut.
fn cut(mem: &mut dyn GuestMemory, kind: Kind, esp: u32, from: u32) -> bool {
    if mem.u32(esp).ok() != Some(from) {
        return false;
    }
    let host = crate::native_particles::HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let decided = with(kind, |p| p.setup(|| host.as_ref().map_or(Err("no_host"), |h| h.material_cut(&mut *mem, kind))));
    match decided {
        Ok(()) => true,
        Err(cause) => {
            crate::native_layers::count_kept_as("", 15, kind.name(), &format!("material_{cause}"), 1);
            false
        }
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
    // The setup's body does not run on a cut call: the particle setup
    // returns AL 1 (`ret 0xc`), the contrail's returns (`ret 4`).
    b.raw()
        .before(&PARTICLE_SETUP, "effect_materials_particle_setup", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            if !cut(&mut *rt.memory(), Kind::Particle, c.cpu.esp, PARTICLE_SETUP_RETURN) {
                return Ok(hooks::Flow::Continue);
            }
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
            if !cut(&mut *mem, Kind::Contrail, c.cpu.esp, CONTRAIL_SETUP_RETURN) {
                return Ok(hooks::Flow::Continue);
            }
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
    fn every_setup_the_host_makes_is_cut() {
        let mut p = Cuts::new();
        assert_eq!(p.setup(|| Ok(())), Ok(()));
        assert!(p.under_cut);
        assert_eq!(p.setup(|| Ok(())), Ok(()));
        assert_eq!(p.cut, 2);
        assert!(p.counters("particle_material").iter().all(|(n, _)| !n.contains("sampled") && !n.contains("fallback")));
    }

    #[test]
    fn a_refused_material_keeps_the_guest_setup_by_name() {
        let mut p = Cuts::new();
        assert_eq!(p.setup(|| Ok(())), Ok(()));
        assert_eq!(p.setup(|| Err("device_state")), Err("device_state"));
        assert!(!p.under_cut, "the guest's setup runs");
        assert_eq!(p.setup(|| Ok(())), Ok(()), "the next setup is cut again");
        assert!(p.counters("particle_material").contains(&("particle_material_refused_device_state".to_owned(), 1)));
    }

    #[test]
    fn a_cut_draw_the_device_refuses_is_dropped_by_name() {
        let mut p = Cuts::new();
        assert_eq!(p.setup(|| Ok(())), Ok(()));
        p.dropped("admission");
        assert!(p.counters("particle_material").contains(&("particle_material_dropped_admission".to_owned(), 1)));
        assert_eq!(p.setup(|| Ok(())), Ok(()), "the next setup is still cut");
        assert!(p.under_cut);
    }

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
