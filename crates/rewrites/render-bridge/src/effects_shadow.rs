//! R2 (docs/graphics/retire-d3d-r2.md, "R2's shadow arm"): the guest
//! bodies under the sorted dispatch `0x0001e370`, made in Rust.
//!
//! - **Stamps.** Each outermost call of the particle systems' per-view
//!   update `0x00174420` ([`view_update`]): system `+0xd` bit 2, child
//!   `+0xc` bit `0x400`, particle `+0x30`, the particle-model singletons
//!   and the sorted entries it queues.
//! - **Dispatch.** The sorted loop ([`dispatch`]): the decal runs drawn
//!   between its entries.
//! - **Emission and ribbon.** Each particle's emission and each contrail's
//!   vertices, made by `engine::particle_emission` and
//!   `engine::contrail_ribbon` ([`answer`]).
//!
//! The stamps are the render's, not the arena's ([`stamps`]): where the
//! answer runs `0x00174420` the model's stamps are held in host memory,
//! and where the guest body runs it reads the held stamps lent to the
//! arena and they go back when it returns
//! (docs/engine/sim-render-split.md, "6. Render scratch and stamps").
//!
//! Rust answers every frame; no frame is the guest's for comparison. A
//! body the answer does not take runs as the guest's, counted by name
//! (`scene_kept_15_<kind>_<cause>`). The display copy draws the plan's
//! decal runs ([`RunHost`]). `render.stats` counts every outcome as
//! `layer_plan_15_effects_shadow_<outcome>`.
//!
//! On with the native renderer; `frame-scene.effects-shadow=off` leaves
//! only the guest.

mod answer;
pub mod dispatch;
#[cfg(test)]
mod fixtures;
pub mod geometry;
pub mod mirror;
mod overlay;
pub mod stamps;
pub mod view_update;

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use hooks::{Builder, Flow};

/// The argument bytes `0x00224550` pops (`ret 0x1c`).
fn pe_pop() -> u32 {
    engine::particle_emission::POP - 4
}

use dispatch::{Kind, Plan};

/// The `frame-scene` option that turns the arm off.
pub const ID: &str = "frame-scene.effects-shadow";
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

/// This test thread's count of every outcome starting `prefix`.
#[cfg(test)]
fn tested_prefix(prefix: &str) -> u64 {
    TESTED.with(|t| t.borrow().iter().filter(|(k, _)| k.starts_with(prefix)).map(|(_, n)| n).sum())
}

/// The gate: every frame is the answer's. It counts the frames and the
/// named refusals; nothing is compared.
#[derive(Debug, Default)]
pub struct Gate {
    frame: Option<u32>,
}

impl Gate {
    /// The frame a hook runs in.
    pub fn frame(&mut self, frame: u32) {
        if self.frame != Some(frame) {
            self.frame = Some(frame);
            count("answering_frames", 1);
        }
    }

    /// A body Rust did not make, named.
    pub fn refused(&mut self, arm: &str, why: &str) {
        count(&format!("{arm}_refused_{why}"), 1);
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

/// A sorted dispatch and the decal runs it draws.
struct OpenDispatch {
    esp: u32,
    runs: Vec<dispatch::Run>,
}

/// The outermost `0x00174420` call whose guest body runs on the lent
/// stamps (`esp` its return address).
struct Lending {
    esp: u32,
    lent: stamps::Lent,
}

thread_local! {
    static LENDING: RefCell<Option<Lending>> = const { RefCell::new(None) };
    static DISPATCH: RefCell<Option<OpenDispatch>> = const { RefCell::new(None) };
}

/// All SSE exceptions masked, round to nearest, gradual underflow: the
/// model's single-precision arithmetic is the guest's.
fn sse_supported(mxcsr: u32) -> bool {
    mxcsr & !0x3f == 0x1f80
}

/// At `0x00174420`'s entry (`esp` its return address). True when the
/// answer made the call and it returns (`ret 0x10`); false runs the
/// guest's body, an outermost one on the lent stamps.
fn update_entry(m: &mut dyn GuestMemory, esp: u32, mxcsr: u32, cw: u16) -> bool {
    let Ok(frame) = m.u32(view_update::FRAME) else {
        crate::native_layers::count_kept_as("", 15, "stamps", "frame", 1);
        return false;
    };
    let nested = LENDING.with(|l| {
        let mut l = l.borrow_mut();
        match l.as_ref().map(|o| o.esp) {
            Some(outer) if esp < outer => true,
            Some(_) => {
                // An outer call whose return was never seen.
                count("stamps_unbalanced_lend", 1);
                *l = None;
                false
            }
            None => false,
        }
    });
    if nested {
        return false;
    }
    with_gate(|g| g.frame(frame));
    if answer::view_update(m, esp, mxcsr, cw) {
        return true;
    }
    if let Some(lent) = stamps::with(|s| s.lend(m)) {
        LENDING.with(|l| *l.borrow_mut() = Some(Lending { esp, lent }));
    } else {
        count("stamps_unlent", 1);
    }
    false
}

/// At `0x00174420`'s return (`esp` past its arguments): the outermost
/// guest body's stamps go to the table and the arena's back.
fn update_return(m: &mut dyn GuestMemory, esp: u32) {
    let lending = LENDING.with(|l| {
        let mut l = l.borrow_mut();
        if l.as_ref().is_some_and(|o| o.esp.wrapping_add(VIEW_UPDATE_POP) == esp) {
            l.take()
        } else {
            None
        }
    });
    if let Some(lending) = lending {
        stamps::with(|s| s.take(m, lending.lent));
    }
}

/// After the sort, before the loop (`TRANSPARENT_ORDER_READY`): the
/// plan's decal runs. Particles are the answer's, drawn at their
/// callbacks ([`answer`]); the decal runs are the seam for a host that
/// draws the decal lists' replays from the scene.
fn dispatch_begin(m: &dyn GuestMemory, esp: u32) {
    DISPATCH.with(|d| {
        if d.borrow_mut().take().is_some() {
            count("dispatch_unbalanced", 1);
        }
    });
    let Ok(frame) = m.u32(view_update::FRAME) else {
        return;
    };
    with_gate(|g| g.frame(frame));
    let Some(plan) = Plan::read(m) else {
        with_gate(|g| g.refused("dispatch", "plan"));
        return;
    };
    for (kind, n) in Kind::ALL.iter().zip(plan.census()) {
        count(&format!("dispatch_entries_{}", kind.name()), n);
    }
    let mut runs = plan.runs();
    runs.retain(|r| r.kind == Kind::Decal);
    for run in &runs {
        count(&format!("entry_runs_planned_{}", run.kind.name()), 1);
    }
    if !runs.is_empty() {
        DISPATCH.with(|d| *d.borrow_mut() = Some(OpenDispatch { esp, runs }));
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
fn dispatch_end() {
    let Some(open) = DISPATCH.with(|d| d.borrow_mut().take()) else {
        return;
    };
    count("entry_runs_undrawn", open.runs.len() as u64);
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
    // The arena is the snapshot's: nothing lent is put back, and the
    // render's stamps start over.
    LENDING.with(|l| l.borrow_mut().take());
    stamps::with(|s| s.clear());
    DISPATCH.with(|d| d.borrow_mut().take());
    mirror::forget();
    geometry::forget();
}

/// Installs the arm's observers (the frame step's build, unless [`ID`] is
/// off).
pub fn install(b: &mut Builder) {
    // The stamps: the answer makes the call, or the guest's body runs on
    // the lent stamps and gives them back at its return.
    b.raw()
        .before(&points::PARTICLE_VIEW_UPDATE, "effects_shadow_update", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let cut = update_entry(&mut *rt.memory(), c.cpu.esp, c.cpu.mxcsr, c.cpu.x87.control);
            if cut {
                c.ret(VIEW_UPDATE_POP - 4).map(Flow::Return)
            } else {
                Ok(Flow::Continue)
            }
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(
            &points::PARTICLE_VIEW_UPDATE,
            "effects_shadow_update_end",
            |o| {
                let mut g = o.guest();
                update_return(&mut *g, o.cpu.esp);
            },
        )
        .context_only()
        .optional();
    // R2's answer: the callback's body does not run when the answer
    // takes it.
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
    // R2's geometry answer: an entry is drawn natively and the handler
    // returns (`ret 4`); its build call is the answer's through
    // `native_world`'s hook on the build (`geometry::build`).
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
        .observe_before(
            &points::TRANSPARENT_ORDER_READY,
            "effects_shadow_dispatch",
            |o| {
                let g = o.guest();
                dispatch_begin(&*g, o.cpu.esp);
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
            |_| dispatch_end(),
        )
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::view_update::tests::{PART_DATA, ROWS, SYS_DATA, particle, system, world};
    use super::view_update::{LIST_END, LIST_ENTRIES, LIST_STRIDE, PARTICLE, SYSTEM};
    use super::*;
    use engine::memory::FlatMemory;

    fn counter(name: &str) -> u64 {
        super::tested(name)
    }

    #[test]
    fn the_gate_counts_each_frame_once_and_names_its_refusals() {
        let mut g = Gate::default();
        let frames = counter("answering_frames");
        g.frame(1);
        g.frame(1);
        g.frame(2);
        assert_eq!(counter("answering_frames"), frames + 2);
        let refused = counter("stamps_refused_object_path");
        g.refused("stamps", "object_path");
        assert_eq!(counter("stamps_refused_object_path"), refused + 1);
    }

    /// The world of `view_update`'s tests, with seeds.
    fn seeded_world() -> (FlatMemory, u32) {
        let mut m = world();
        m.set_u32(view_update::FRAME, 1000).unwrap();
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

    fn kept(name: &str) -> u64 {
        crate::native_layers::counters()
            .into_iter()
            .find(|(k, _)| *k == name)
            .map_or(0, |(_, v)| v)
    }

    #[test]
    fn the_frame_step_leaves_the_particle_arrays_as_the_tick_left_them() {
        let (mut m, s) = seeded_world();
        for i in 0..4u32 {
            m.set_u32(ESP + 4 + 4 * i, 0).unwrap();
        }
        let arrays = |m: &FlatMemory| {
            (m.bytes(SYS_DATA, (SYSTEM * 8) as usize).unwrap(), m.bytes(PART_DATA, (PARTICLE * 8) as usize).unwrap())
        };
        let tick = arrays(&m);
        let stamp = |m: &FlatMemory| stamps::with(|st| st.view(m).unwrap().u32(PART_DATA + 0x30).unwrap());

        // Every frame is the answer's: the call returns, its entry is queued.
        for frame in [1001, 1064] {
            m.set_u32(view_update::FRAME, frame).unwrap();
            m.set_u32(LIST_END, 0).unwrap();
            assert!(update_entry(&mut m, ESP, MXCSR, CW), "frame {frame}");
            assert_eq!(arrays(&m), tick);
            assert_eq!(m.u32(LIST_END).unwrap(), 1);
            assert_eq!(m.u32(ROWS + 0x24).unwrap(), s);
            assert_eq!(stamp(&m), frame, "the render's stamp is held");
        }

        // A call the model refuses runs as the guest's body, by name.
        let before = kept("scene_kept_15_stamps_mxcsr");
        assert!(!update_entry(&mut m, ESP, 0, CW));
        update_return(&mut m, ESP + VIEW_UPDATE_POP);
        assert_eq!(kept("scene_kept_15_stamps_mxcsr"), before + 1);
        stamps::with(|st| st.clear());
    }

    #[test]
    fn a_dispatch_counts_its_entries_by_kind() {
        let (mut m, _) = seeded_world();
        for i in 0..4u32 {
            m.set_u32(ESP + 4 + 4 * i, 0).unwrap();
        }
        assert!(update_entry(&mut m, ESP, MXCSR, CW));
        // One queued particle entry, sorted.
        m.set_u32(view_update::LIST_FIRST, 0).unwrap();
        m.set_u32(view_update::LIST_ORDER, 0x006a_0000).unwrap();
        m.set_u16(0x006a_0000, 0).unwrap();
        let particles = counter("dispatch_entries_particle");
        dispatch_begin(&m, ESP);
        dispatch_next(&mut m, ESP, 0);
        dispatch_end();
        assert!(counter("dispatch_entries_particle") > particles);
        stamps::with(|st| st.clear());
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
        m.set_u32(view_update::FRAME, 5).unwrap();
        let host = Arc::new(Recorder::default());
        set_run_host(host.clone());
        dispatch_begin(&m, ESP);
        dispatch_next(&mut m, ESP, 0);
        assert!(host.0.lock().unwrap().is_empty());
        // Another frame's hook is not this dispatch's.
        dispatch_next(&mut m, ESP - 4, 1);
        dispatch_next(&mut m, ESP, 1);
        dispatch_next(&mut m, ESP, 2);
        dispatch_end();
        assert_eq!(
            *host.0.lock().unwrap(),
            [(Kind::Decal, vec![2])],
            "the particles are the answer's" 
        );
    }
}
