//! `render-loop`: stage 7a's Rust orchestrators placed as implementations
//! (docs/graphics/render-loop.md). Each is switched by `impl.render.<name>`, falling
//! back to `impl.render`; all are off by default.
//!
//! Built: `impl.render.views`, the Rust `render_views`
//! ([`engine::render_loop::Machine`]); `impl.render.layer_draw` and
//! `impl.render.layer_entries`, the layer draw `0x00044550` and its entry
//! loop `0x0003bc30` ([`engine::render_loop::layer`], docs/graphics/layer-draw.md);
//! `impl.render.game_view` and `impl.render.scene_passes`, `render_game_view`
//! `0x0002b790` and `render_scene_passes` `0x0002bcd0`
//! ([`engine::render_loop::scene`]); `impl.render.layer_sort`, the layer
//! entries' qsort `0x0013da70` with the layer comparators in Rust
//! ([`engine::render_loop::sort`]); `impl.render.states`, the shader
//! draw's render-state list `0x0001a170` ([`engine::render_loop::states`]),
//! exact, and `impl.render.states_skip`, the same with the redundant
//! `SetRenderState_Simple` calls skipped against the D3D shadow `0x00407498`
//! (it wins over `impl.render.states`); `impl.render.transparent_sort`, the
//! transparents' index sort `0x0013df80` with comparator `0x0001e700` in Rust
//! ([`engine::render_loop::sort_indexed`]).
//! The driver runs the port to each
//! guest call, makes it with `call_guest` (the callee returns into a
//! continuation that resumes the port), and returns to the caller when the
//! port does. The only guest-visible difference from the original is the
//! return address the callees see (the continuation token), as for every
//! chain with an after hook.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::render_loop::layer::{Draw, Entries};
use engine::render_loop::scene::{GameView, ScenePasses};
use engine::render_loop::sort::Sort;
use engine::render_loop::sort_indexed::IndexSort;
use engine::render_loop::states::{Counts, Mode, RenderStates};
use engine::render_loop::{Machine, Regs, Step};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "render-loop";

/// Each native path this feature can place, by its implementation's name.
pub const PATHS: [&str; 8] = [
    "render_views",
    "layer_draw",
    "layer_entries",
    "render_game_view",
    "render_scene_passes",
    "layer_sort",
    "render_states",
    "transparent_sort",
];

static CALLS: [AtomicU64; PATHS.len()] = [const { AtomicU64::new(0) }; PATHS.len()];

fn called(i: usize) {
    CALLS[i].fetch_add(1, Ordering::Relaxed);
}

/// Calls into each native path since the process started (0 for a path
/// that is off): which `impl.render.*` paths a frame took, for the agent's
/// `render.stats` and `render.frame`.
pub fn path_calls() -> Vec<(&'static str, u64)> {
    PATHS
        .iter()
        .zip(&CALLS)
        .map(|(&n, c)| (n, c.load(Ordering::Relaxed)))
        .collect()
}

pub struct RenderLoop;

impl Feature for RenderLoop {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Rust render orchestrators (stage 7a)").doc("docs/graphics/render-loop.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if b.switches().implementation("render.views", false) {
            b.raw()
                .implement(&points::RENDER_VIEWS, "render_views", |cpu, ctx, _| {
                    called(0);
                    run(cpu, ctx, Machine::start)
                });
        }
        if b.switches().implementation("render.layer_draw", false) {
            b.raw()
                .implement(&points::LAYER_DRAW, "layer_draw", |cpu, ctx, _| {
                    called(1);
                    run(cpu, ctx, Draw::start)
                });
        }
        if b.switches().implementation("render.layer_entries", false) {
            b.raw()
                .implement(&points::LAYER_ENTRIES, "layer_entries", |cpu, ctx, _| {
                    called(2);
                    run(cpu, ctx, Entries::start)
                });
        }
        if b.switches().implementation("render.game_view", false) {
            b.raw().implement(
                &points::RENDER_GAME_VIEW,
                "render_game_view",
                |cpu, ctx, _| {
                    called(3);
                    run(cpu, ctx, GameView::start)
                },
            );
        }
        if b.switches().implementation("render.scene_passes", false) {
            b.raw().implement(
                &points::RENDER_SCENE_PASSES,
                "render_scene_passes",
                |cpu, ctx, _| {
                    called(4);
                    run(cpu, ctx, ScenePasses::start)
                },
            );
        }
        if b.switches().implementation("render.layer_sort", false) {
            b.raw()
                .implement(&points::LAYER_SORT, "layer_sort", |cpu, ctx, _| {
                    called(5);
                    run(cpu, ctx, Sort::start)
                });
        }
        let skip = b.switches().implementation("render.states_skip", false);
        if skip || b.switches().implementation("render.states", false) {
            let mode = if skip {
                Mode::SkipRedundant
            } else {
                Mode::Exact
            };
            b.raw().implement(
                &points::RENDER_STATES,
                "render_states",
                move |cpu, ctx, _| {
                    called(6);
                    let fcw = cpu.x87.control;
                    run(cpu, ctx, |r, m| RenderStates::start(r, m, fcw, mode))
                },
            );
        }
        if b.switches()
            .implementation("render.transparent_sort", false)
        {
            b.raw().implement(
                &points::TRANSPARENT_SORT,
                "transparent_sort",
                |cpu, ctx, _| {
                    called(7);
                    run(cpu, ctx, IndexSort::start)
                },
            );
        }
        // A port's state lives in the continuation it is parked in, which
        // the machine's quiescence check reports.
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

fn regs(cpu: &CpuAbi) -> Regs {
    Regs {
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esp: cpu.esp,
        ebp: cpu.ebp,
        esi: cpu.esi,
        edi: cpu.edi,
        xmm0: u128::from_le_bytes(cpu.xmm[0]),
        xmm1: u128::from_le_bytes(cpu.xmm[1]),
    }
}

fn store(cpu: &mut CpuAbi, r: &Regs) {
    cpu.eax = r.eax;
    cpu.ecx = r.ecx;
    cpu.edx = r.edx;
    cpu.ebx = r.ebx;
    cpu.esp = r.esp;
    cpu.ebp = r.ebp;
    cpu.esi = r.esi;
    cpu.edi = r.edi;
    cpu.xmm[0] = r.xmm0.to_le_bytes();
    cpu.xmm[1] = r.xmm1.to_le_bytes();
}

fn fault(f: engine::memory::MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

/// A port the driver can resume after each guest call.
trait Port: Copy + Send + 'static {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault>;
    /// Called once when the port returns to its caller.
    fn done(&self) {}
}

impl Port for Machine {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        Machine::resume(self, r, m)
    }
}
impl Port for Draw {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        Draw::resume(self, r, m)
    }
}
impl Port for Entries {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        Entries::resume(self, r, m)
    }
}

impl Port for GameView {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        GameView::resume(self, r, m)
    }
}
impl Port for ScenePasses {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        ScenePasses::resume(self, r, m)
    }
}
impl Port for Sort {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        Sort::resume(self, r, m)
    }
}
impl Port for IndexSort {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        IndexSort::resume(self, r, m)
    }
}
impl Port for RenderStates {
    fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        RenderStates::resume(self, r, m)
    }
    fn done(&self) {
        STATE_COUNTS.add(&self.counts);
    }
}

/// Running totals of the render-state lists applied, logged every
/// [`StateCounts::EVERY`] lists.
struct StateCounts {
    lists: AtomicU64,
    entries: AtomicU64,
    simple: AtomicU64,
    skipped: AtomicU64,
    deferred: AtomicU64,
    setters: AtomicU64,
}

static STATE_COUNTS: StateCounts = StateCounts {
    lists: AtomicU64::new(0),
    entries: AtomicU64::new(0),
    simple: AtomicU64::new(0),
    skipped: AtomicU64::new(0),
    deferred: AtomicU64::new(0),
    setters: AtomicU64::new(0),
};

impl StateCounts {
    const EVERY: u64 = 100_000;

    fn add(&self, c: &Counts) {
        let r = Ordering::Relaxed;
        self.entries.fetch_add(c.entries.into(), r);
        self.simple.fetch_add(c.simple.into(), r);
        self.skipped.fetch_add(c.skipped.into(), r);
        self.deferred.fetch_add(c.deferred.into(), r);
        self.setters.fetch_add(c.setters.into(), r);
        let lists = self.lists.fetch_add(1, r) + 1;
        if lists.is_multiple_of(Self::EVERY) {
            let simple = self.simple.load(r);
            let skipped = self.skipped.load(r);
            let calls = simple + skipped;
            eprintln!(
                "render-states: lists {lists} entries {} simple {simple} skipped {skipped} ({:.1}%) deferred {} setters {}",
                self.entries.load(r),
                if calls == 0 {
                    0.0
                } else {
                    100.0 * skipped as f64 / calls as f64
                },
                self.deferred.load(r),
                self.setters.load(r),
            );
        }
    }
}

/// An implementation: entered at the function's first instruction, as the
/// original would be.
fn run<P: Port>(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    start: impl FnOnce(&mut Regs, &mut dyn GuestMemory) -> Result<(P, Step), MemFault>,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let mut r = regs(cpu);
    let (m, step) = start(&mut r, &mut *mem).map_err(fault)?;
    store(cpu, &r);
    follow(m, step, cpu, ctx, &mut mem)
}

/// Makes the call `step` asks for, or returns.
fn follow<P: Port>(
    mut m: P,
    step: Step,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
) -> Result<TransferAbi, GuestFault> {
    match step {
        Step::Return { to } => {
            m.done();
            Ok(TransferAbi::ret(to))
        }
        Step::Call { target, .. } => ctx.call_guest(cpu, mem, target, move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let mut r = regs(cpu);
            let step = m.resume(&mut r, &mut *mem).map_err(fault)?;
            store(cpu, &r);
            follow(m, step, cpu, ctx, &mut mem)
        }),
    }
}
