//! R2's shadow arm (docs/graphics/retire-d3d.md, "R2's shadow arm"): the
//! guest state that blocks replacing the sorted dispatch `0x0001e370`,
//! made in Rust beside the guest and compared with what the guest made.
//!
//! - **Stamps.** Each outermost call of the particle systems' per-view
//!   update `0x00174420` ([`view_update`]): the model's writes (system
//!   `+0xd` bit 2, child `+0xc` bit `0x400`, particle `+0x30`, the
//!   particle-model singletons, the sorted entries it queues) against the
//!   systems, particles, singletons, list and seeds at its return.
//! - **Dispatch.** The sorted loop ([`dispatch`]): the Rust sort's order
//!   against the guest's, each handler named, and the seeds, systems,
//!   particles and list rows unchanged across the loop.
//! - **Emission.** Each particle's emission ([`emission`]): the
//!   callback `0x00224550`'s words, vectors and input cache, made by
//!   `engine::particle_emission`, against the guest's at its emitter call.
//! - **Ribbon.** Each contrail's vertices ([`ribbon`]): the builder
//!   `0x00042cf0`'s vertices, AL and writes, made by
//!   `engine::contrail_ribbon`, against the guest's at its return.
//! - **Effects seed.** No path under either reads the shared random seeds
//!   (`[0x004e7408]`): both predict that the seeds do not move.
//!
//! One frame in [`SAMPLE`] is compared (the render frame counter
//! `0x004ba034`). The gate answers after [`CLEAN`] sampled frames in a
//! row compared clean, and stops answering at the first difference or
//! refusal. While it answers, the display copy draws the plan's particle
//! and decal runs ([`RunHost`]). `render.stats` counts every outcome as
//! `layer_plan_15_effects_shadow_<outcome>`.
//!
//! On with the native renderer; `frame-scene.effects-shadow=off` leaves
//! only the guest.

mod answer;
pub mod dispatch;
mod emission;
pub mod geometry;
pub mod mirror;
mod overlay;
mod ribbon;
pub mod view_update;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{Builder, Flow};

/// The argument bytes `0x00224550` pops (`ret 0x1c`).
fn pe_pop() -> u32 {
    engine::particle_emission::POP - 4
}

use dispatch::{Kind, Plan};
use view_update::{
    LIST_END, LIST_ENTRIES, LIST_FIRST, LIST_OVERFLOW, LIST_STRIDE, PARTICLE, PARTICLES,
    Prediction, SINGLETON_BYTES, SINGLETONS, SYSTEM, SYSTEMS,
};

/// The `frame-scene` option that turns the arm off.
pub const ID: &str = "frame-scene.effects-shadow";
/// One frame in this many is compared.
pub const SAMPLE: u32 = 64;
/// Clean sampled frames in a row before the gate answers.
pub const CLEAN: u32 = 8;
/// What `0x00174420` pops: its return address and four arguments.
const VIEW_UPDATE_POP: u32 = 0x14;

/// `render.stats`: `layer_plan_15_effects_shadow_<outcome>`.
fn count(outcome: &str, n: u64) {
    #[cfg(test)]
    TESTED.with(|t| *t.borrow_mut().entry(outcome.to_owned()).or_default() += n);
    #[cfg(not(test))]
    crate::native_layers::count(15, &format!("effects_shadow_{outcome}"), n);
}

// Tests run in parallel and the outcome counters are process-wide: each
// test counts its own thread's outcomes, read back with `tested`.
#[cfg(test)]
thread_local! {
    static TESTED: RefCell<std::collections::BTreeMap<String, u64>> = const { RefCell::new(std::collections::BTreeMap::new()) };
}

/// This test thread's count of `outcome`.
#[cfg(test)]
fn tested(outcome: &str) -> u64 {
    TESTED.with(|t| t.borrow().get(outcome).copied().unwrap_or(0))
}

/// The gate: which frame is being compared, its verdict so far, and the
/// run of clean sampled frames.
#[derive(Debug, Default)]
pub struct Gate {
    frame: Option<u32>,
    sampled: bool,
    clean: bool,
    compared: u32,
    streak: u32,
    answering: bool,
}

impl Gate {
    /// The frame a hook runs in. A new frame ends the last one's verdict.
    /// Answers whether this frame is compared.
    pub fn frame(&mut self, frame: u32) -> bool {
        if self.frame != Some(frame) {
            self.finish();
            self.frame = Some(frame);
            self.sampled = frame % SAMPLE == 0;
            self.clean = true;
            self.compared = 0;
            if self.answering {
                count("answering_frames", 1);
            }
        }
        self.sampled
    }

    fn finish(&mut self) {
        if self.frame.is_none() || !self.sampled {
            return;
        }
        count("frames_sampled", 1);
        if !self.clean {
            count("frames_unclean", 1);
            return;
        }
        if self.compared == 0 {
            // Nothing to compare: the streak neither grows nor breaks.
            count("frames_empty", 1);
            return;
        }
        count("frames_clean", 1);
        self.streak += 1;
        if self.streak >= CLEAN && !self.answering {
            self.answering = true;
            count("opened", 1);
        }
    }

    /// The frame is not clean: the streak ends and the gate stops answering.
    fn spoil(&mut self) {
        self.clean = false;
        self.streak = 0;
        if self.answering {
            self.answering = false;
            count("fallback", 1);
        }
    }

    pub fn matched(&mut self, arm: &str) {
        self.compared += 1;
        count(&format!("{arm}_matched"), 1);
    }

    pub fn differed(&mut self, arm: &str, what: &str) {
        self.compared += 1;
        count(&format!("{arm}_mismatched_{what}"), 1);
        self.spoil();
    }

    pub fn refused(&mut self, arm: &str, why: &str) {
        count(&format!("{arm}_refused_{why}"), 1);
        self.spoil();
    }

    pub fn answering(&self) -> bool {
        self.answering
    }
}

#[cfg(not(test))]
static GATE: Mutex<Option<Gate>> = Mutex::new(None);

#[cfg(not(test))]
fn with_gate<R>(f: impl FnOnce(&mut Gate) -> R) -> R {
    f(GATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Gate::default))
}

// Tests run in parallel: each keeps its own gate.
#[cfg(test)]
thread_local! {
    static GATE: RefCell<Gate> = RefCell::new(Gate::default());
}

#[cfg(test)]
fn with_gate<R>(f: impl FnOnce(&mut Gate) -> R) -> R {
    GATE.with(|g| f(&mut g.borrow_mut()))
}

/// Whether the gate answers: the last [`CLEAN`] sampled frames compared
/// clean and none since differed.
pub fn answering() -> bool {
    with_gate(|g| g.answering())
}

/// Guest bytes as they were.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Region {
    addr: u32,
    bytes: Vec<u8>,
}

impl Region {
    fn read(m: &dyn GuestMemory, addr: u32, len: u32) -> Result<Self, MemFault> {
        Ok(Self {
            addr,
            bytes: m.bytes(addr, len as usize)?,
        })
    }

    fn contains(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.addr) < self.bytes.len() as u32
    }

    /// Whether the bytes now are the bytes then with `writes` applied.
    fn holds(&self, m: &dyn GuestMemory, writes: &BTreeMap<u32, u8>) -> bool {
        let mut want = self.bytes.clone();
        let end = self.addr.saturating_add(want.len() as u32);
        for (&a, &b) in writes.range(self.addr..end) {
            want[(a - self.addr) as usize] = b;
        }
        m.bytes(self.addr, want.len()).is_ok_and(|now| now == want)
    }
}

/// The state both arms compare, read before the guest runs.
#[derive(Clone, Debug)]
struct Snapshot {
    seeds: Option<Region>,
    systems: Region,
    particles: Region,
    singletons: Vec<Region>,
    list: Region,
    overflow: Region,
    /// The live rows, for the dispatch.
    rows: Option<Region>,
}

impl Snapshot {
    fn take(m: &dyn GuestMemory, rows: bool) -> Result<Self, view_update::Refusal> {
        let seeds = match m.u32(points::RANDOM_SEEDS.base)? {
            0 => None,
            p => Some(Region::read(m, p, 8)?),
        };
        let (data, len) = view_update::array_span(m, SYSTEMS, SYSTEM)?;
        let systems = Region::read(m, data, len)?;
        let (data, len) = view_update::array_span(m, PARTICLES, PARTICLE)?;
        let particles = Region::read(m, data, len)?;
        let singletons = SINGLETONS
            .iter()
            .map(|&(_, at, _)| Region::read(m, at, SINGLETON_BYTES))
            .collect::<Result<_, _>>()?;
        let rows = if rows {
            let first = m.u32(LIST_FIRST)?;
            let end = m.u32(LIST_END)?;
            let base = m
                .u32(LIST_ENTRIES)?
                .wrapping_add(first.wrapping_mul(LIST_STRIDE));
            Some(Region::read(
                m,
                base,
                end.saturating_sub(first).wrapping_mul(LIST_STRIDE),
            )?)
        } else {
            None
        };
        Ok(Self {
            seeds,
            systems,
            particles,
            singletons,
            list: Region::read(m, LIST_END, 4)?,
            overflow: Region::read(m, LIST_OVERFLOW, 1)?,
            rows,
        })
    }

    /// The first part of the state that is not as predicted: the snapshot
    /// with `writes` applied. `singletons` false leaves them out.
    fn differs(
        &self,
        m: &dyn GuestMemory,
        writes: &BTreeMap<u32, u8>,
        singletons: bool,
    ) -> Option<&'static str> {
        let none = BTreeMap::new();
        if let Some(seeds) = &self.seeds {
            if !seeds.holds(m, &none) {
                return Some("seed");
            }
        }
        if !self.systems.holds(m, writes) {
            return Some("systems");
        }
        if !self.particles.holds(m, writes) {
            return Some("particles");
        }
        if singletons && !self.singletons.iter().all(|s| s.holds(m, writes)) {
            return Some("singletons");
        }
        if !self.list.holds(m, writes) || !self.overflow.holds(m, writes) {
            return Some("list");
        }
        if let Some(rows) = &self.rows {
            if !rows.holds(m, &none) {
                return Some("rows");
            }
        }
        // What the model writes outside these: the rows it queues.
        let regions = [&self.systems, &self.particles, &self.list, &self.overflow];
        let outside = writes.iter().filter(|(a, _)| {
            !regions.iter().any(|r| r.contains(**a))
                && !self.singletons.iter().any(|r| r.contains(**a))
        });
        for (&a, &b) in outside {
            if m.u8(a).ok() != Some(b) {
                return Some("entries");
            }
        }
        None
    }
}

/// An outermost `0x00174420` call being compared.
struct OpenUpdate {
    esp: u32,
    before: Snapshot,
    prediction: Prediction,
}

/// A sorted dispatch being compared, and the runs it draws.
struct OpenDispatch {
    esp: u32,
    before: Option<Snapshot>,
    runs: Vec<dispatch::Run>,
}

thread_local! {
    static UPDATE: RefCell<Option<OpenUpdate>> = const { RefCell::new(None) };
    static DISPATCH: RefCell<Option<OpenDispatch>> = const { RefCell::new(None) };
}

/// All SSE exceptions masked, round to nearest, gradual underflow: the
/// model's single-precision arithmetic is the guest's.
fn sse_supported(mxcsr: u32) -> bool {
    mxcsr & !0x3f == 0x1f80
}

/// At `0x00174420`'s entry (`esp` its return address).
fn update_begin(m: &dyn GuestMemory, esp: u32, mxcsr: u32, cw: u16) {
    let Ok(frame) = m.u32(view_update::FRAME) else {
        return;
    };
    if !with_gate(|g| g.frame(frame)) {
        return;
    }
    let nested = UPDATE.with(|u| {
        let mut u = u.borrow_mut();
        let Some(outer) = u.as_ref().map(|o| o.esp) else {
            return false;
        };
        if esp < outer {
            return true;
        }
        // An outer call whose return was never seen.
        count("stamps_unbalanced", 1);
        *u = None;
        false
    });
    if nested {
        return;
    }
    if !sse_supported(mxcsr) {
        with_gate(|g| g.refused("stamps", "mxcsr"));
        return;
    }
    let arg = |i: u32| m.u32(esp.wrapping_add(4 + 4 * i));
    let (Ok(a0), Ok(a1), Ok(a2), Ok(a3)) = (arg(0), arg(1), arg(2), arg(3)) else {
        with_gate(|g| g.refused("stamps", "fault"));
        return;
    };
    let args = [a0, a1, a2, a3];
    let open = view_update::predict(m, args, cw).and_then(|prediction| {
        Ok(OpenUpdate {
            esp,
            before: Snapshot::take(m, false)?,
            prediction,
        })
    });
    match open {
        Ok(open) => UPDATE.with(|u| *u.borrow_mut() = Some(open)),
        Err(r) => with_gate(|g| g.refused("stamps", r.name())),
    }
}

/// At `0x00174420`'s return (`esp` past its arguments).
fn update_end(m: &dyn GuestMemory, esp: u32) {
    let open = UPDATE.with(|u| {
        let mut u = u.borrow_mut();
        if u.as_ref()
            .is_some_and(|o| o.esp.wrapping_add(VIEW_UPDATE_POP) == esp)
        {
            u.take()
        } else {
            None
        }
    });
    let Some(open) = open else { return };
    match open.before.differs(m, &open.prediction.writes, true) {
        None => {
            count(
                "stamps_entries_queued",
                open.prediction.entries.len() as u64,
            );
            with_gate(|g| g.matched("stamps"));
        }
        Some(what) => with_gate(|g| g.differed("stamps", what)),
    }
}

/// After the sort, before the loop (`TRANSPARENT_ORDER_READY`).
fn dispatch_begin(m: &dyn GuestMemory, esp: u32, mxcsr: u32) {
    DISPATCH.with(|d| {
        if d.borrow_mut().take().is_some() {
            count("dispatch_unbalanced", 1);
        }
    });
    let Ok(frame) = m.u32(view_update::FRAME) else {
        return;
    };
    let sampled = with_gate(|g| g.frame(frame));
    if !sampled && !answering() {
        return;
    }
    let Some(plan) = Plan::read(m) else {
        with_gate(|g| g.refused("dispatch", "plan"));
        return;
    };
    let mut before = None;
    if sampled {
        before = dispatch_sample(m, &plan, mxcsr);
    }
    // A difference in the sample just now stops the gate for these runs.
    let runs = if answering() {
        // Particles are the answer's, drawn at their callbacks
        // ([`answer`]); the decal runs are the seam for a host that draws
        // the decal lists' replays from the scene.
        let mut runs = plan.runs();
        runs.retain(|r| r.kind == Kind::Decal);
        for run in &runs {
            count(&format!("entry_runs_planned_{}", run.kind.name()), 1);
        }
        runs
    } else {
        Vec::new()
    };
    if before.is_some() || !runs.is_empty() {
        DISPATCH.with(|d| *d.borrow_mut() = Some(OpenDispatch { esp, before, runs }));
    }
}

/// A sampled dispatch's order check and the state it compares at the end.
fn dispatch_sample(m: &dyn GuestMemory, plan: &Plan, mxcsr: u32) -> Option<Snapshot> {
    for (kind, n) in Kind::ALL.iter().zip(plan.census()) {
        count(&format!("dispatch_entries_{}", kind.name()), n);
    }
    if !sse_supported(mxcsr) {
        with_gate(|g| g.refused("dispatch", "mxcsr"));
        return None;
    }
    match crate::native_scene_lists::transparent_order(m) {
        None => {
            with_gate(|g| g.refused("dispatch", "order"));
            return None;
        }
        Some(order) if order != plan.order() => {
            with_gate(|g| g.differed("dispatch", "order"));
            return None;
        }
        Some(_) => {}
    }
    match Snapshot::take(m, true) {
        Ok(s) => Some(s),
        Err(r) => {
            with_gate(|g| g.refused("dispatch", r.name()));
            None
        }
    }
}

/// After the loop's entry `ordinal` (`TRANSPARENT_DISPATCH_NEXT`): the run
/// that ends there is drawn.
fn dispatch_next(mem: &mut dyn GuestMemory, esp: u32, ordinal: u32) {
    let Ok(ordinal) = u16::try_from(ordinal) else {
        return;
    };
    let run = DISPATCH.with(|d| {
        let mut d = d.borrow_mut();
        let open = d.as_mut().filter(|o| o.esp == esp)?;
        let ends = open
            .runs
            .first()
            .is_some_and(|r| r.ordinals.last() == Some(&ordinal));
        ends.then(|| open.runs.remove(0))
    });
    if let Some(run) = run {
        draw_run(mem, &run);
    }
}

/// The loop's exit (`SCENE_TRANSPARENTS_END`).
fn dispatch_end(m: &dyn GuestMemory) {
    let Some(open) = DISPATCH.with(|d| d.borrow_mut().take()) else {
        return;
    };
    count("entry_runs_undrawn", open.runs.len() as u64);
    let Some(before) = open.before else { return };
    // The singletons are left out: `0x00137bd0` sets one before every
    // read, so what the loop leaves in them is never read.
    match before.differs(m, &BTreeMap::new(), false) {
        None => with_gate(|g| g.matched("dispatch")),
        Some(what) => with_gate(|g| g.differed("dispatch", what)),
    }
}

/// What draws the plan's particle and decal runs on the display copy.
pub trait RunHost: Send + Sync {
    /// Draws on the display copy what the sorted entries `ordinals`, all
    /// of `kind` and in loop order, add to it, from inside the loop after
    /// the last of them. Returns how many entries it drew; 0 when it
    /// refused, with nothing drawn. The reference view is untouched.
    fn draw_display_entries(
        &self,
        mem: &mut dyn GuestMemory,
        kind: Kind,
        ordinals: &[u16],
    ) -> usize;
}

static RUN_HOST: Mutex<Option<Arc<dyn RunHost>>> = Mutex::new(None);

/// Installs the host that draws the runs (boot).
pub fn set_run_host(host: Arc<dyn RunHost>) {
    *RUN_HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn draw_run(mem: &mut dyn GuestMemory, run: &dispatch::Run) {
    let kind = run.kind.name();
    let host = RUN_HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else {
        count(&format!("entry_runs_no_host_{kind}"), 1);
        // Their guest replays draw them.
        crate::native_layers::count_kept_as("", 15, kind, "replay", run.ordinals.len() as u64);
        return;
    };
    match host.draw_display_entries(mem, run.kind, &run.ordinals) {
        0 => count(&format!("entry_runs_refused_{kind}"), 1),
        n => {
            count(&format!("entry_runs_drawn_{kind}"), 1);
            count(&format!("entry_runs_entries_{kind}"), n as u64);
        }
    }
}

/// A snapshot load: calls and dispatches open across it never end.
pub fn forget() {
    UPDATE.with(|u| u.borrow_mut().take());
    DISPATCH.with(|d| d.borrow_mut().take());
    emission::forget();
    mirror::forget();
    ribbon::forget();
    geometry::forget();
}

/// Installs the arm's observers (the frame step's build, unless [`ID`] is
/// off).
pub fn install(b: &mut Builder) {
    b.raw()
        .observe_before(
            &points::PARTICLE_VIEW_UPDATE,
            "effects_shadow_update",
            |o| {
                let g = o.guest();
                update_begin(&*g, o.cpu.esp, o.cpu.mxcsr, o.cpu.x87.control);
            },
        )
        .context_only()
        .optional();
    b.raw()
        .observe_after(
            &points::PARTICLE_VIEW_UPDATE,
            "effects_shadow_update_end",
            |o| {
                let g = o.guest();
                update_end(&*g, o.cpu.esp);
            },
        )
        .context_only()
        .optional();
    // R2's answer: the callback's body does not run when the answer
    // takes it; on a sampled frame or a closed gate the arm opens.
    b.raw()
        .before(&points::PARTICLE_EVALUATE, "effects_shadow_emission", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let cut = answer::particle(&mut *rt.memory(), c.cpu.esp, c.cpu.mxcsr, c.cpu.x87.control);
            if cut {
                c.ret(pe_pop()).map(Flow::Return)
            } else {
                Ok(Flow::Continue)
            }
        })
        .context_only()
        .optional();
    // The particle draw's return: the mirror's held batch draws.
    b.raw()
        .observe_after(&crate::effect_materials::PARTICLE_MATERIAL, "effects_shadow_mirror_end", |o| {
            let mut g = o.guest();
            mirror::finish(&mut *g);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_before(&points::PARTICLE_EMIT, "effects_shadow_emission_emit", |o| {
            let g = o.guest();
            emission::emit(&*g, o.cpu.esp, o.cpu.ebx);
        })
        .context_only()
        .optional();
    b.raw()
        .before(&points::CONTRAIL_RIBBON, "effects_shadow_ribbon", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let al = answer::contrail(&mut *rt.memory(), c.cpu.esp, c.cpu.mxcsr, c.cpu.x87.control);
            match al {
                Some(al) => {
                    c.cpu.eax = (c.cpu.eax & !0xff) | u32::from(al);
                    c.ret(engine::contrail_ribbon::POP - 4).map(Flow::Return)
                }
                None => Ok(Flow::Continue),
            }
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::CONTRAIL_RIBBON, "effects_shadow_ribbon_end", |o| {
            let g = o.guest();
            ribbon::end(&*g, o.cpu.esp, o.cpu.eax);
        })
        .context_only()
        .optional();
    // The standard handler's own entry and return; its build call is the
    // arm's through `native_world`'s hook on the build (`geometry::build`).
    b.raw()
        .observe_before(&points::TRANSPARENT_GEOMETRY, "effects_shadow_geometry", |o| {
            let g = o.guest();
            geometry::begin_handler(&*g, o.cpu.esp);
        })
        .context_only()
        .optional();
    // R2's geometry answer: a proven BSP cluster entry is drawn natively and
    // the handler returns (`ret 4`).
    b.raw()
        .before(&points::TRANSPARENT_GEOMETRY, "effects_shadow_geometry_draw", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if geometry::answer_handler(&mut *rt.memory(), c.cpu.esp) {
                c.ret(4).map(Flow::Return)
            } else {
                Ok(Flow::Continue)
            }
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::TRANSPARENT_GEOMETRY, "effects_shadow_geometry_end", |o| {
            geometry::end_handler(o.cpu.esp);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::WORLD_GEOMETRY_BUILD, "effects_shadow_geometry_build_end", |o| {
            let g = o.guest();
            geometry::end_build(&*g, o.cpu.esp, o.cpu.eax);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_before(
            &points::TRANSPARENT_ORDER_READY,
            "effects_shadow_dispatch",
            |o| {
                let g = o.guest();
                dispatch_begin(&*g, o.cpu.esp, o.cpu.mxcsr);
            },
        )
        .context_only()
        .optional();
    b.raw()
        .observe_before(
            &points::scene_effects::TRANSPARENT_DISPATCH_NEXT,
            "effects_shadow_dispatch_next",
            |o| {
                let mut g = o.guest();
                dispatch_next(&mut *g, o.cpu.esp, o.cpu.esi);
            },
        )
        .context_only()
        .optional();
    b.raw()
        .observe_before(
            &points::scene_effects::SCENE_TRANSPARENTS_END,
            "effects_shadow_dispatch_end",
            |o| {
                let g = o.guest();
                dispatch_end(&*g);
            },
        )
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::view_update::tests::{PART_DATA, ROWS, particle, system, world};
    use super::*;
    use engine::memory::FlatMemory;

    fn counter(name: &str) -> u64 {
        super::tested(name)
    }

    #[test]
    fn the_gate_answers_after_clean_sampled_frames_and_falls_back_at_a_difference() {
        let mut g = Gate::default();
        // Frames between samples are not compared.
        assert!(!g.frame(1));
        for k in 1..=CLEAN {
            assert!(g.frame(k * SAMPLE));
            g.matched("stamps");
            assert!(!g.answering());
        }
        // The verdict of the last sampled frame lands with the next frame.
        g.frame(CLEAN * SAMPLE + 1);
        assert!(g.answering());
        // An empty sampled frame keeps the gate as it is.
        g.frame((CLEAN + 1) * SAMPLE);
        g.frame((CLEAN + 1) * SAMPLE + 1);
        assert!(g.answering());
        // A difference stops it at once, and the streak starts again.
        g.frame((CLEAN + 2) * SAMPLE);
        g.differed("stamps", "particles");
        assert!(!g.answering());
        g.frame((CLEAN + 2) * SAMPLE + 1);
        assert_eq!(g.streak, 0);
        // A refusal is not clean either.
        g.frame((CLEAN + 3) * SAMPLE);
        g.matched("stamps");
        g.refused("stamps", "object_path");
        g.frame((CLEAN + 3) * SAMPLE + 1);
        assert_eq!(g.streak, 0);
    }

    /// The world of `view_update`'s tests, with seeds, at a sampled frame.
    fn sampled_world() -> (FlatMemory, u32) {
        let mut m = world();
        m.set_u32(view_update::FRAME, SAMPLE * 1000).unwrap();
        let seeds = 0x0068_0000;
        m.set_u32(points::RANDOM_SEEDS.base, seeds).unwrap();
        m.set_u32(seeds, 11).unwrap();
        m.set_u32(seeds + 4, 22).unwrap();
        let s = system(&mut m, 0, 0);
        particle(&mut m, 0, 4.0, u32::MAX);
        m.set_u16(s + 0xc, 0x400).unwrap();
        (m, s)
    }

    const ESP: u32 = 0x0069_0000;
    const CW: u16 = 0x027f;
    const MXCSR: u32 = 0x1f80;

    /// The guest's call as the model predicts it.
    fn guest_runs(m: &mut FlatMemory) {
        let p = view_update::predict(&*m, [0, 0, 0, 0], CW).unwrap();
        for (&a, &b) in &p.writes {
            m.set_u8(a, b).unwrap();
        }
    }

    #[test]
    fn a_call_the_guest_makes_as_the_model_does_matches_and_a_stray_write_differs() {
        let (mut m, s) = sampled_world();
        for i in 0..4u32 {
            m.set_u32(ESP + 4 + 4 * i, 0).unwrap();
        }
        let matched = counter("stamps_matched");
        update_begin(&m, ESP, MXCSR, CW);
        // A nested call is the outer call's.
        update_begin(&m, ESP - 0x80, MXCSR, CW);
        update_end(&m, ESP - 0x80 + VIEW_UPDATE_POP);
        guest_runs(&mut m);
        update_end(&m, ESP + VIEW_UPDATE_POP);
        assert!(counter("stamps_matched") > matched);
        assert_eq!(m.u32(PART_DATA + 0x30).unwrap(), SAMPLE * 1000);
        assert_eq!(m.u32(ROWS + 0x24).unwrap(), s);

        // The guest stamps a particle the model did not.
        let (mut m, _) = sampled_world();
        let differed = counter("stamps_mismatched_particles");
        update_begin(&m, ESP, MXCSR, CW);
        guest_runs(&mut m);
        m.set_u32(PART_DATA + 0x34 + 0x30, 9).unwrap();
        update_end(&m, ESP + VIEW_UPDATE_POP);
        assert!(counter("stamps_mismatched_particles") > differed);

        // The guest draws from the effects seed.
        let (mut m, _) = sampled_world();
        let differed = counter("stamps_mismatched_seed");
        update_begin(&m, ESP, MXCSR, CW);
        guest_runs(&mut m);
        m.set_u32(0x0068_0004, 23).unwrap();
        update_end(&m, ESP + VIEW_UPDATE_POP);
        assert!(counter("stamps_mismatched_seed") > differed);
    }

    #[test]
    fn a_dispatch_that_leaves_the_state_matches_and_one_that_writes_a_row_differs() {
        let (mut m, _) = sampled_world();
        guest_runs(&mut m);
        // One queued particle entry, sorted.
        m.set_u32(LIST_FIRST, 0).unwrap();
        m.set_u32(view_update::LIST_ORDER, 0x006a_0000).unwrap();
        m.set_u16(0x006a_0000, 0).unwrap();
        let matched = counter("dispatch_matched");
        dispatch_begin(&m, ESP, MXCSR);
        dispatch_next(&mut m, ESP, 0);
        dispatch_end(&m);
        assert!(counter("dispatch_matched") > matched);
        assert!(counter("dispatch_entries_particle") >= 1);

        let differed = counter("dispatch_mismatched_rows");
        dispatch_begin(&m, ESP, MXCSR);
        m.set_u32(ROWS + 0x30, 5).unwrap();
        dispatch_end(&m);
        assert!(counter("dispatch_mismatched_rows") > differed);
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(Kind, Vec<u16>)>>);

    impl RunHost for Recorder {
        fn draw_display_entries(
            &self,
            _: &mut dyn GuestMemory,
            kind: Kind,
            ordinals: &[u16],
        ) -> usize {
            self.0.lock().unwrap().push((kind, ordinals.to_vec()));
            ordinals.len()
        }
    }

    #[test]
    fn an_answering_gate_draws_each_decal_run_after_its_last_entry() {
        let mut m = FlatMemory::new(0x004b_0000, 0x0003_0000);
        let rows = 0x004c_0000;
        let order = 0x004d_0000;
        m.set_u32(LIST_ENTRIES, rows).unwrap();
        m.set_u32(view_update::LIST_ORDER, order).unwrap();
        m.set_u32(LIST_END, 3).unwrap();
        let p = view_update::PARTICLE_HANDLER;
        for (i, h) in [p, p, dispatch::DECAL_HANDLERS[0]].into_iter().enumerate() {
            m.set_u32(rows + i as u32 * LIST_STRIDE + 0x20, h).unwrap();
            m.set_u16(order + 2 * i as u32, i as u16).unwrap();
        }
        // Not a sampled frame; the gate answers.
        m.set_u32(view_update::FRAME, 5).unwrap();
        let host = Arc::new(Recorder::default());
        set_run_host(host.clone());
        with_gate(|g| {
            g.frame(5);
            g.answering = true;
        });
        dispatch_begin(&m, ESP, MXCSR);
        dispatch_next(&mut m, ESP, 0);
        assert!(host.0.lock().unwrap().is_empty());
        // Another frame's hook is not this dispatch's.
        dispatch_next(&mut m, ESP - 4, 1);
        dispatch_next(&mut m, ESP, 1);
        dispatch_next(&mut m, ESP, 2);
        dispatch_end(&m);
        assert_eq!(
            *host.0.lock().unwrap(),
            [(Kind::Decal, vec![2])],
            "the particles are the answer's" 
        );
        with_gate(|g| g.answering = false);
    }
}
