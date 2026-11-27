//! The proofs of the Rust particle and contrail materials
//! (`native_render::particle_material`, `native_render::contrail_material`)
//! against the device state the guest's setups leave
//! (docs/graphics/retire-d3d-r2.md, "R2's materials").
//!
//! Two observers read the setups' inputs at their entries: the particle
//! draw `0x002243c0` (held until the next draw: every particle of the
//! walk is drawn under it) and the contrail setup `0x00043720` (held for
//! the one dynamic draw whose context is its entry data). The game's
//! native admission compares one admitted draw in [`SAMPLE`] of each kind
//! and records the outcome here: `render.scene` carries
//! `<kind>_material_{sampled,matched,mismatched}`,
//! `<kind>_material_mismatched_<field>` and
//! `<kind>_material_refused_<reason>`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::memory::GuestMemory;
use engine::points::{Point, Sig};
use hooks::Builder;
use native_render::{contrail_material, particle_material};

/// One admitted draw in this many is compared (the first, then every
/// 64th).
pub const SAMPLE: u64 = 64;

const DOC: &str = "docs/graphics/retire-d3d-r2.md";
pub const PARTICLE_MATERIAL: Point = Point::new("PARTICLE_MATERIAL", particle_material::DRAW, Sig::Opaque, DOC);
pub const CONTRAIL_MATERIAL: Point = Point::new("CONTRAIL_MATERIAL", contrail_material::SETUP, Sig::Opaque, DOC);

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

/// One kind's tally.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proof {
    seen: u64,
    pub sampled: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub fields: BTreeMap<String, u64>,
    pub refused: BTreeMap<&'static str, u64>,
}

impl Proof {
    const fn new() -> Self {
        Self { seen: 0, sampled: 0, matched: 0, mismatched: 0, fields: BTreeMap::new(), refused: BTreeMap::new() }
    }

    /// Whether this draw is to be compared.
    pub fn sample(&mut self) -> bool {
        self.seen += 1;
        (self.seen - 1) % SAMPLE == 0
    }

    /// A compared draw's differing fields, or why the material was not
    /// made.
    pub fn record(&mut self, outcome: Result<&[String], &'static str>) {
        self.sampled += 1;
        match outcome {
            Err(why) => *self.refused.entry(why).or_default() += 1,
            Ok([]) => self.matched += 1,
            Ok(fields) => {
                self.mismatched += 1;
                for f in fields {
                    *self.fields.entry(f.clone()).or_default() += 1;
                }
            }
        }
    }

    fn counters(&self, prefix: &str) -> Vec<(String, u64)> {
        let mut out = vec![
            (format!("{prefix}_sampled"), self.sampled),
            (format!("{prefix}_matched"), self.matched),
            (format!("{prefix}_mismatched"), self.mismatched),
        ];
        out.extend(self.fields.iter().map(|(f, n)| (format!("{prefix}_mismatched_{f}"), *n)));
        out.extend(self.refused.iter().map(|(r, n)| (format!("{prefix}_refused_{r}"), *n)));
        out
    }
}

static PROOFS: Mutex<[Proof; 2]> = Mutex::new([Proof::new(), Proof::new()]);
static PARTICLE: Mutex<Option<particle_material::Inputs>> = Mutex::new(None);
static CONTRAIL: Mutex<Option<(u32, contrail_material::Inputs)>> = Mutex::new(None);

fn with<T>(kind: Kind, f: impl FnOnce(&mut Proof) -> T) -> T {
    let mut p = PROOFS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut p[kind as usize])
}

/// Whether this admitted draw of `kind` is to be compared.
pub fn sample(kind: Kind) -> bool {
    with(kind, Proof::sample)
}

/// Records a compared draw ([`Proof::record`]).
pub fn record(kind: Kind, outcome: Result<&[String], &'static str>) {
    with(kind, |p| p.record(outcome));
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

/// `render.scene` rows of both kinds.
pub fn counters() -> Vec<(String, u64)> {
    let p = PROOFS.lock().unwrap_or_else(|e| e.into_inner());
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
    b.raw()
        .observe_before(&CONTRAIL_MATERIAL, "effect_materials_contrail", |o| {
            let mem = o.guest();
            on_contrail(&*mem, o.cpu.esp);
        })
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_draw_in_sixty_four_is_compared_and_each_outcome_is_counted_by_name() {
        let mut p = Proof::new();
        assert!(p.sample());
        assert!((1..SAMPLE).all(|_| !p.sample()));
        assert!(p.sample());
        p.record(Ok(&[]));
        p.record(Ok(&["render_0x3b".to_owned(), "texture0".to_owned()]));
        p.record(Err("tags"));
        assert_eq!((p.sampled, p.matched, p.mismatched), (3, 1, 1));
        let rows = p.counters("particle_material");
        for (name, n) in [
            ("particle_material_matched", 1),
            ("particle_material_mismatched_render_0x3b", 1),
            ("particle_material_mismatched_texture0", 1),
            ("particle_material_refused_tags", 1),
        ] {
            assert!(rows.contains(&(name.to_owned(), n)), "{name}");
        }
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
