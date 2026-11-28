//! `render_frame_and_present` `0x00223240` in Rust
//! (docs/graphics/retire-d3d-display.md, "The frame step"): the guest's
//! frame as a sequence of steps, none of which enters guest code that
//! touches D3D.
//!
//! The guest's body (Code, `0x00223240..0x0022329f`):
//!
//! 1. A game view when engine globals are active (`[0x004e6948]` set, its
//!    byte `+0x1120` set, its word `+2` not `0xffff`) and the UI's world
//!    skip `0x00068290` returns 0: `0x002234e0`, the views. Otherwise the
//!    frame outside a game view, `0x002232a0` (menus, loading, movies).
//! 2. The frame wait `0x0012b2e0`, always.
//! 3. The present `0x00013f10(0, 0)` when both resource-cache countdowns
//!    (`0x004e6470`, `0x004e64a0`), read after the view, are at or below
//!    0: a loading frame does not present.
//!
//! [`next`] is that order as data; [`RenderFrame`] runs it. The world skip
//! and the views are guest calls: their stages are the native renderer's
//! (each stage's own replacement). The frame wait is the Rust override in
//! [`crate::frame_wait`]. The present is [`publish::present`], in Rust:
//! the swap is the scene's seal (`scene_step`'s observer after
//! `RENDER_FRAME`), so the guest's present, which calls `Swap`, never runs.
//!
//! Three frame loops outside `0x00223240` run a view, the frame wait and
//! the present themselves: `0x0012b6f0` (`0x0012b784`) and the map load's
//! two (`0x00163a28`, `0x00163adb`). [`RenderFrame`] implements the
//! present for them ([`loop_present`]): the same writes, and the swap
//! counted for the vertical blank ([`crate::frame_wait::swapped`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub mod blit;
pub mod publish;

pub const ID: &str = "render-frame";

/// The guest addresses the frame step reads and calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    pub engine_globals: u32,
    pub world_skip: u32,
    pub views: u32,
    pub outside_view: u32,
    pub cache_countdowns: (u32, u32),
    pub frame_wait: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        engine_globals: 0x004e_6948,
        world_skip: 0x0006_8290,
        views: 0x0022_34e0,
        outside_view: 0x0022_32a0,
        cache_countdowns: (0x004e_6470, 0x004e_64a0),
        frame_wait: 0x0012_b2e0,
    };
}

/// Where the frame is in its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Entered: the globals decide whether the world skip is asked.
    Start,
    /// The world skip returned AL.
    Skipped(u8),
    /// The view or the frame outside one returned.
    Viewed,
    /// The frame wait returned; `present` is the countdowns' answer.
    Waited { present: bool },
    /// The present was made.
    Presented,
}

/// What to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Call the guest's `target`; the next step follows from `kind` and
    /// what it returns.
    Call { target: u32, kind: Called },
    /// The present `0x00013f10(0, 0)`, made in Rust ([`publish::present`]).
    Present,
    /// Return to the frame's caller.
    Return,
}

/// Which call an [`Action::Call`] is, for the next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Called {
    WorldSkip,
    Views,
    OutsideView,
    FrameWait,
}

/// What the steps read from the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reads {
    /// The engine globals are active.
    pub world: bool,
    /// Both cache countdowns at or below 0.
    pub idle: bool,
}

/// The frame's next action after `step`, given `reads` (taken when the
/// step needs them: `world` at [`Step::Start`], `idle` at [`Step::Viewed`]).
/// The second value is the countdowns' answer, carried to
/// [`Step::Waited`].
pub fn next(a: &Addrs, step: Step, reads: Reads) -> (Action, Option<bool>) {
    let call = |target, kind| Action::Call { target, kind };
    match step {
        Step::Start if reads.world => (call(a.world_skip, Called::WorldSkip), None),
        Step::Start => (call(a.outside_view, Called::OutsideView), None),
        Step::Skipped(0) => (call(a.views, Called::Views), None),
        Step::Skipped(_) => (call(a.outside_view, Called::OutsideView), None),
        Step::Viewed => (call(a.frame_wait, Called::FrameWait), Some(reads.idle)),
        Step::Waited { present: true } => (Action::Present, None),
        Step::Waited { present: false } | Step::Presented => (Action::Return, None),
    }
}

/// The step a returned call leads to: `al` is the guest's AL on return.
pub fn after_call(kind: Called, al: u8, present: Option<bool>) -> Step {
    match kind {
        Called::WorldSkip => Step::Skipped(al),
        Called::Views | Called::OutsideView => Step::Viewed,
        Called::FrameWait => Step::Waited { present: present.unwrap_or(false) },
    }
}

/// `0x00223240`'s globals test.
pub fn world(a: &Addrs, g: &dyn GuestMemory) -> Result<bool, MemFault> {
    let w = g.u32(a.engine_globals)?;
    Ok(w != 0 && g.u8(w.wrapping_add(0x1120))? != 0 && g.u16(w.wrapping_add(2))? != 0xffff)
}

/// Both resource-cache countdowns at or below 0.
pub fn idle(a: &Addrs, g: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(g.i32(a.cache_countdowns.0)? <= 0 && g.i32(a.cache_countdowns.1)? <= 0)
}

/// Frames so far, by what they drew.
#[derive(Debug, Default)]
pub struct Stats {
    pub views: AtomicU64,
    pub outside_view: AtomicU64,
    /// The frame step's presents.
    pub presents: AtomicU64,
    pub loading: AtomicU64,
    /// The frame loops' presents ([`loop_present`]).
    pub loop_presents: AtomicU64,
}

/// The feature: the frame step at `RENDER_FRAME` and the present at
/// `RENDER_PRESENT`, with the native renderer on. Not with `sim-frame`,
/// which owns the point in sim mode.
#[derive(Clone)]
pub struct RenderFrame {
    addrs: Addrs,
    present: publish::Addrs,
    stats: Arc<Stats>,
    /// The host's blit placer; `None` asks [`publish::installed_blit`]
    /// each present.
    blit: Option<publish::PlaceBlit>,
}

impl Default for RenderFrame {
    fn default() -> Self {
        RenderFrame { addrs: Addrs::HALO2, present: publish::Addrs::HALO2, stats: Arc::default(), blit: None }
    }
}

impl RenderFrame {
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }

    /// Places the present's blit with `place` rather than the installed
    /// host's.
    pub fn with_blit(mut self, place: publish::PlaceBlit) -> Self {
        self.blit = Some(place);
        self
    }

    fn present(&self, g: &mut dyn GuestMemory) -> Result<publish::Presented, MemFault> {
        publish::present(&self.present, self.blit.or_else(publish::installed_blit), g)
    }
}

impl Feature for RenderFrame {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "render_frame_and_present as the Rust frame step").doc("docs/graphics/retire-d3d-display.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        let me = self.clone();
        b.raw().implement(&points::RENDER_FRAME, "render: the frame step", move |cpu, ctx, _| {
            let frame = Frame { esp: cpu.esp, ret: ctx.memory().u32(cpu.esp)? };
            run(&me, frame, Step::Start, cpu, ctx)
        });
        let me = self.clone();
        b.raw().implement(&points::RENDER_PRESENT, "render: a frame loop's present", move |cpu, ctx, _| {
            loop_present(&me, cpu, ctx)
        });
        Ok(())
    }
}

/// The frame's entry: its stack pointer (at the return address) and
/// where it returns.
#[derive(Clone, Copy)]
struct Frame {
    esp: u32,
    ret: u32,
}

fn fault(f: MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// The present `0x00013f10` called from guest code: the frame loops'
/// (`0x0012b784`, `0x00163a28`, `0x00163adb`; the frame step makes its own
/// in Rust). The present's writes, the swap counted for the vertical
/// blank, then the guest's `ret 8` with the counter's high dword in EAX.
fn loop_present(me: &RenderFrame, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    {
        let mut mem = rt.memory();
        let arg = mem.u32(cpu.esp.wrapping_add(4))?;
        if publish::readback(&*mem, arg).map_err(fault)? {
            unimplemented!("render: the present's readback 0x00013f21 (front buffer to bitmap through D3D surfaces)");
        }
        cpu.eax = me.present(&mut *mem).map_err(fault)?.eax;
    }
    crate::frame_wait::swapped();
    me.stats.loop_presents.fetch_add(1, Ordering::Relaxed);
    ctx.memory().ret(cpu, 8)
}

fn run(me: &RenderFrame, frame: Frame, step: Step, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let mut step = step;
    loop {
        let reads = match step {
            Step::Start => Reads { world: world(&me.addrs, &*mem).map_err(fault)?, idle: false },
            Step::Viewed => Reads { world: false, idle: idle(&me.addrs, &*mem).map_err(fault)? },
            _ => Reads { world: false, idle: false },
        };
        let (action, present) = next(&me.addrs, step, reads);
        match action {
            Action::Present => {
                // The guest's present returns the counter's high dword,
                // which the frame leaves in EAX.
                cpu.eax = me.present(&mut *mem).map_err(fault)?.eax;
                me.stats.presents.fetch_add(1, Ordering::Relaxed);
                step = Step::Presented;
            }
            Action::Return => {
                cpu.esp = frame.esp.wrapping_add(4);
                return Ok(TransferAbi::ret(frame.ret));
            }
            Action::Call { target, kind } => {
                match kind {
                    Called::Views => me.stats.views.fetch_add(1, Ordering::Relaxed),
                    Called::OutsideView => me.stats.outside_view.fetch_add(1, Ordering::Relaxed),
                    Called::FrameWait if present == Some(false) => me.stats.loading.fetch_add(1, Ordering::Relaxed),
                    _ => 0,
                };
                // Each call starts from the frame's own stack; none takes
                // stack arguments.
                cpu.esp = frame.esp;
                let me = me.clone();
                return ctx.call_guest_named(cpu, &mut mem, target, "render: the frame step", move |cpu, ctx| {
                    run(&me, frame, after_call(kind, cpu.eax as u8, present), cpu, ctx)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const A: Addrs = Addrs::HALO2;

    /// The frame's actions in order: guest calls by target, the present
    /// as `PRESENT`.
    const PRESENT: u32 = 0x0001_3f10;

    fn steps(world: bool, skip: u8, idle: bool) -> Vec<u32> {
        let mut out = Vec::new();
        let mut step = Step::Start;
        loop {
            let (action, present) = next(&A, step, Reads { world, idle });
            step = match action {
                Action::Return => return out,
                Action::Present => {
                    out.push(PRESENT);
                    Step::Presented
                }
                Action::Call { target, kind } => {
                    out.push(target);
                    after_call(kind, skip, present)
                }
            };
        }
    }

    #[test]
    fn the_steps_are_the_guests_calls_in_its_order() {
        assert_eq!(steps(true, 0, true), [0x0006_8290, 0x0022_34e0, 0x0012_b2e0, PRESENT]);
        assert_eq!(steps(true, 1, true), [0x0006_8290, 0x0022_32a0, 0x0012_b2e0, PRESENT], "the UI's world skip");
        assert_eq!(steps(false, 0, true), [0x0022_32a0, 0x0012_b2e0, PRESENT], "no world: the skip is not asked");
        assert_eq!(steps(true, 0, false), [0x0006_8290, 0x0022_34e0, 0x0012_b2e0], "loading: waits, does not present");
    }

    /// The present is never a guest call: the frame never reaches
    /// `0x00013f10`'s body and the `Swap` in it.
    #[test]
    fn the_present_is_made_in_rust_not_called() {
        for (world, skip, idle) in [(true, 0, true), (true, 1, true), (false, 0, true), (true, 0, false)] {
            let mut step = Step::Start;
            let mut presents = 0;
            loop {
                let (action, present) = next(&A, step, Reads { world, idle });
                step = match action {
                    Action::Return => break,
                    Action::Present => {
                        presents += 1;
                        Step::Presented
                    }
                    Action::Call { target, kind } => {
                        assert_ne!(target, PRESENT, "no guest call of the present");
                        after_call(kind, skip, present)
                    }
                };
            }
            assert_eq!(presents, usize::from(idle), "one present a frame, none while loading");
        }
    }

    #[test]
    fn the_wait_carries_the_countdowns_read_after_the_view() {
        assert_eq!(after_call(Called::FrameWait, 0, Some(true)), Step::Waited { present: true });
        assert_eq!(after_call(Called::FrameWait, 1, Some(false)), Step::Waited { present: false });
        assert_eq!(after_call(Called::WorldSkip, 3, None), Step::Skipped(3), "the skip's AL");
        assert_eq!(next(&A, Step::Presented, Reads { world: true, idle: true }).0, Action::Return);
    }

    /// The frame step's present, as `run` makes it: the present's writes
    /// with no guest D3D entry, the counter's high dword for EAX.
    #[test]
    fn the_frame_steps_present_moves_the_counter_with_no_guest_call() {
        let a = publish::Addrs::HALO2;
        let mut g = FakeGuest::new();
        g.map(0x0040_7000, 0x2000).map(0x0046_7000, 0x1000).map(0x0048_5000, 0x1000).map(0x0050_9000, 0x1000);
        g.put_u32(a.ticks.present_counter, u32::MAX);
        let frame = RenderFrame::default();
        let p = frame.present(&mut g).unwrap();
        assert_eq!(g.u32(a.ticks.present_counter).unwrap(), 0);
        assert_eq!(p.eax, 1, "the carry, as the guest's adc makes it");
        assert!(!p.blit && !p.display);
    }

    #[test]
    fn the_globals_and_the_countdowns_read_as_the_guest_tests_them() {
        let mut g = FakeGuest::new();
        g.map(0x004e_6000, 0x1000).map(0x0070_0000, 0x2000);
        assert!(!world(&A, &g).unwrap(), "no globals");
        g.put_u32(A.engine_globals, 0x0070_0000);
        assert!(!world(&A, &g).unwrap(), "inactive");
        g.put_u8(0x0070_1120, 1);
        assert!(world(&A, &g).unwrap());
        g.put_u16(0x0070_0002, 0xffff);
        assert!(!world(&A, &g).unwrap(), "no map");
        assert!(idle(&A, &g).unwrap());
        g.put_u32(A.cache_countdowns.1, (-3i32) as u32);
        assert!(idle(&A, &g).unwrap(), "below 0 is done");
        g.put_u32(A.cache_countdowns.0, 2);
        assert!(!idle(&A, &g).unwrap());
    }
}
