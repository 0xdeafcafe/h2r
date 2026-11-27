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
//! `<kind>_material_{sampled,matched,mismatched,open,fallback,cut}`,
//! `<kind>_material_mismatched_<field>`,
//! `<kind>_material_refused_<reason>` and
//! `<kind>_material_dropped_<cause>`. At the setup of the draw compared
//! next the guest's state arrays are read ([`take_state`]): the
//! comparison covers the whole render and stage state a cut draw takes.
//!
//! Each kind has a gate: [`CLEAN`] matched comparisons in a row open it,
//! any mismatch or refusal closes it (`fallback` counts the closings of
//! an open gate). While it is open the setup itself is the cut's: at the
//! setup's call (`0x0023ea80` from `0x00224419`, `0x00043720` from
//! `0x00035b21`) one call in [`SAMPLE`] runs the guest's setup and its
//! first draw is compared; the others are cut when the host holds the
//! Rust material for the device's supplied draw (`Host::material_cut`),
//! and what it refuses keeps the guest's setup, counted
//! `scene_kept_15_<kind>_material_<cause>`. A cut setup's draw the device
//! then refuses is dropped ([`dropped`]) and closes the gate.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::memory::GuestMemory;
use engine::points::{Point, Sig};
use hooks::Builder;
use native_render::{contrail_material, effect_material, particle_material};

/// One admitted draw in this many is compared (the first, then every
/// 64th); with the gate open, one setup call in this many runs the guest.
pub const SAMPLE: u64 = 64;
/// Matched comparisons in a row that open a kind's gate.
pub const CLEAN: u32 = 8;
/// The particle setup `0x0023ea80` and the return of its call in the
/// particle draw (`ret 0xc`, AL whether the walk runs).
pub const PARTICLE_SETUP: Point = Point::new("PARTICLE_SETUP", 0x0023_ea80, Sig::Opaque, DOC);
pub const PARTICLE_SETUP_RETURN: u32 = 0x0022_441e;
/// The polygon handler's return from its entry's setup (`call eax`).
pub const CONTRAIL_SETUP_RETURN: u32 = 0x0003_5b23;

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
pub struct Proof {
    seen: u64,
    pub sampled: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub fields: BTreeMap<String, u64>,
    pub refused: BTreeMap<&'static str, u64>,
    run: u32,
    open: bool,
    pub fallback: u64,
    setups: u64,
    /// The setup running now was cut: its draws are not compared.
    under_cut: bool,
    /// The setup running now is the sampled guest's: its first draw is
    /// compared.
    compare_next: bool,
    pub cut: u64,
    /// Draws under a cut setup the supplied draw refused, by cause.
    pub dropped: BTreeMap<&'static str, u64>,
}

impl Proof {
    const fn new() -> Self {
        Self {
            seen: 0,
            sampled: 0,
            matched: 0,
            mismatched: 0,
            fields: BTreeMap::new(),
            refused: BTreeMap::new(),
            run: 0,
            open: false,
            fallback: 0,
            setups: 0,
            under_cut: false,
            compare_next: false,
            cut: 0,
            dropped: BTreeMap::new(),
        }
    }

    /// Whether this draw is to be compared: with the gate closed one in
    /// [`SAMPLE`], with it open the first draw of a sampled guest setup;
    /// never a draw under a cut setup.
    pub fn sample(&mut self) -> bool {
        if self.under_cut {
            return false;
        }
        if self.open {
            return std::mem::take(&mut self.compare_next);
        }
        self.seen += 1;
        (self.seen - 1) % SAMPLE == 0
    }

    pub fn open(&self) -> bool {
        self.open
    }

    /// Whether the next draw is to be compared ([`Self::sample`] without
    /// its count), read at the setup before it.
    fn compares_next(&self) -> bool {
        !self.under_cut && if self.open { self.compare_next } else { self.seen % SAMPLE == 0 }
    }

    /// A draw under a cut setup the device did not draw from the
    /// material: the gate closes, and the draw, whose guest setup did not
    /// run, is dropped by name.
    fn dropped(&mut self, cause: &'static str) {
        *self.dropped.entry(cause).or_default() += 1;
        self.close();
    }

    /// At a setup's call: whether it is cut. A closed gate or the sampled
    /// call keeps the guest's setup (the latter's first draw compared);
    /// otherwise `host` draws from the Rust material or names why not,
    /// which keeps the guest's setup (`material_<cause>`) and closes the
    /// gate.
    pub fn setup(&mut self, host: impl FnOnce() -> Result<(), &'static str>) -> Result<bool, &'static str> {
        self.under_cut = false;
        self.compare_next = false;
        if !self.open {
            return Ok(false);
        }
        self.setups += 1;
        if (self.setups - 1) % SAMPLE == 0 {
            self.compare_next = true;
            return Ok(false);
        }
        match host() {
            Ok(()) => {
                self.under_cut = true;
                self.cut += 1;
                Ok(true)
            }
            Err(cause) => {
                self.close();
                Err(cause)
            }
        }
    }

    fn close(&mut self) {
        if self.open {
            self.fallback += 1;
        }
        self.open = false;
        self.run = 0;
    }

    /// A compared draw's differing fields, or why the material was not
    /// made.
    pub fn record(&mut self, outcome: Result<&[String], &'static str>) {
        self.sampled += 1;
        match outcome {
            Err(why) => {
                *self.refused.entry(why).or_default() += 1;
                self.close();
            }
            Ok([]) => {
                self.matched += 1;
                self.run = (self.run + 1).min(CLEAN);
                self.open |= self.run >= CLEAN;
            }
            Ok(fields) => {
                self.mismatched += 1;
                self.close();
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
            (format!("{prefix}_open"), u64::from(self.open)),
            (format!("{prefix}_fallback"), self.fallback),
            (format!("{prefix}_cut"), self.cut),
        ];
        out.extend(self.fields.iter().map(|(f, n)| (format!("{prefix}_mismatched_{f}"), *n)));
        out.extend(self.refused.iter().map(|(r, n)| (format!("{prefix}_refused_{r}"), *n)));
        out.extend(self.dropped.iter().map(|(r, n)| (format!("{prefix}_dropped_{r}"), *n)));
        out
    }
}

static PROOFS: Mutex<[Proof; 2]> = Mutex::new([Proof::new(), Proof::new()]);
static PARTICLE: Mutex<Option<particle_material::Inputs>> = Mutex::new(None);
static CONTRAIL: Mutex<Option<(u32, contrail_material::Inputs)>> = Mutex::new(None);
/// The guest's state arrays at the call of the setup whose draw is
/// compared next, by kind.
static BEFORE: Mutex<[Option<effect_material::State>; 2]> = Mutex::new([None, None]);

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

/// Whether the setup of `kind` running now was cut: its draws are drawn
/// from the material the host made at the cut ([`Host::material_cut`]).
///
/// [`Host::material_cut`]: crate::native_particles::Host::material_cut
pub fn under_cut(kind: Kind) -> bool {
    with(kind, |p| p.under_cut)
}

/// A draw under a cut setup that the device's supplied draw refused
/// (`<kind>_material_dropped_<cause>`); the gate closes.
pub fn dropped(kind: Kind, cause: &'static str) {
    with(kind, |p| p.dropped(cause));
}

/// The state arrays at the setup of the draw compared now, taken.
pub fn take_state(kind: Kind) -> Option<effect_material::State> {
    BEFORE.lock().unwrap_or_else(|e| e.into_inner())[kind as usize].take()
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

/// At a setup's call from its handler (`esp` the return address's slot):
/// whether the setup is cut.
fn cut(mem: &mut dyn GuestMemory, kind: Kind, esp: u32, from: u32) -> bool {
    if mem.u32(esp).ok() != Some(from) {
        return false;
    }
    let host = crate::native_particles::HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (decided, compares) = with(kind, |p| {
        let d = p.setup(|| host.as_ref().map_or(Err("no_host"), |h| h.material_cut(&mut *mem, kind)));
        (d, p.compares_next())
    });
    let state = compares.then(|| effect_material::State::read(&|a| mem.u32(a).ok())).flatten();
    BEFORE.lock().unwrap_or_else(|e| e.into_inner())[kind as usize] = state;
    match decided {
        Ok(cut) => cut,
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

    /// A gate opened by `CLEAN` matched comparisons.
    fn proven() -> Proof {
        let mut p = Proof::new();
        for _ in 0..CLEAN {
            p.record(Ok(&[]));
        }
        assert!(p.open());
        p
    }

    #[test]
    fn a_proven_material_cuts_the_handler_setup() {
        let mut p = Proof::new();
        assert_eq!(p.setup(|| Ok(())), Ok(false), "a closed gate keeps the guest's setup");
        let mut p = proven();
        assert_eq!(p.setup(|| Ok(())), Ok(false), "the first call is the sample");
        assert_eq!(p.setup(|| Ok(())), Ok(true));
        assert!(!p.sample(), "a draw under a cut setup is not compared");
        assert_eq!(p.cut, 1);
    }

    #[test]
    fn one_draw_in_64_keeps_the_guest_setup_and_compares() {
        let mut p = proven();
        let cuts: Vec<bool> = (0..2 * SAMPLE).map(|_| p.setup(|| Ok(())).unwrap()).collect();
        assert_eq!(cuts.iter().filter(|c| !**c).count(), 2);
        assert!(!cuts[0] && !cuts[SAMPLE as usize] && cuts[1]);
        // The sampled setup's first draw only.
        let mut p = proven();
        assert_eq!(p.setup(|| Ok(())), Ok(false));
        assert!(p.sample());
        assert!(!p.sample());
        // A mismatch there closes the gate and falls back.
        p.record(Ok(&["texture0".to_owned()]));
        assert!(!p.open());
        assert_eq!(p.fallback, 1);
        assert_eq!(p.setup(|| Ok(())), Ok(false));
    }

    #[test]
    fn a_refused_material_keeps_the_guest_setup_by_name() {
        let mut p = proven();
        assert_eq!(p.setup(|| Ok(())), Ok(false));
        assert_eq!(p.setup(|| Err("device_state")), Err("device_state"));
        assert!(!p.open());
        assert_eq!((p.fallback, p.cut), (1, 0));
        // A refusal recorded by a comparison closes it too.
        let mut p = proven();
        p.record(Err("tags"));
        assert_eq!((p.open(), p.fallback), (false, 1));
    }

    #[test]
    fn a_cut_draw_the_device_refuses_is_dropped_by_name_and_closes_the_gate() {
        let mut p = proven();
        assert_eq!(p.setup(|| Ok(())), Ok(false));
        assert_eq!(p.setup(|| Ok(())), Ok(true));
        assert!(p.under_cut);
        p.dropped("admission");
        assert_eq!((p.open(), p.fallback), (false, 1));
        assert!(p.counters("particle_material").contains(&("particle_material_dropped_admission".to_owned(), 1)));
        assert_eq!(p.setup(|| Ok(())), Ok(false), "the next setup runs the guest's");
        assert!(!p.under_cut);
    }

    #[test]
    fn the_state_is_read_at_the_setup_of_the_draw_compared_next_only() {
        let mut p = Proof::new();
        assert!(p.compares_next(), "a closed gate compares its first draw");
        assert!(p.sample());
        assert!(!p.compares_next());
        let mut p = proven();
        assert_eq!(p.setup(|| Ok(())), Ok(false));
        assert!(p.compares_next(), "the sampled guest setup");
        assert_eq!(p.setup(|| Ok(())), Ok(true));
        assert!(!p.compares_next(), "a cut setup's draws are not compared");
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
