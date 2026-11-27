//! `render_frame_and_present` `0x00223240` in Rust
//! (docs/graphics/retire-d3d-display.md, "The frame step"): the guest's
//! frame as a sequence of steps a later slice replaces one at a time.
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
//! [`next`] is that order as data; [`RenderFrame`] runs it, each step a
//! guest call. Nothing here draws: the steps' own replacements (the views'
//! stages, the overlay's producers, the movie's display) are where the
//! frame goes native. The present stays the guest's until the publish
//! replaces `d3d`'s `Swap`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

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
    pub present: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        engine_globals: 0x004e_6948,
        world_skip: 0x0006_8290,
        views: 0x0022_34e0,
        outside_view: 0x0022_32a0,
        cache_countdowns: (0x004e_6470, 0x004e_64a0),
        frame_wait: 0x0012_b2e0,
        present: 0x0001_3f10,
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
    /// The present returned.
    Presented,
}

/// What to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Call `target` with these stack arguments; the next step follows
    /// from `kind` and what it returns.
    Call { target: u32, args: &'static [u32], kind: Called },
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
    Present,
}

/// What the steps read from the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reads {
    /// The engine globals are active.
    pub world: bool,
    /// Both cache countdowns at or below 0.
    pub idle: bool,
}

/// The guest's next call after `step`, given `reads` (taken when the step
/// needs them: `world` at [`Step::Start`], `idle` at [`Step::Viewed`]).
pub fn next(a: &Addrs, step: Step, reads: Reads) -> (Action, Option<bool>) {
    let call = |target, kind| Action::Call { target, args: &[], kind };
    match step {
        Step::Start if reads.world => (call(a.world_skip, Called::WorldSkip), None),
        Step::Start => (call(a.outside_view, Called::OutsideView), None),
        Step::Skipped(0) => (call(a.views, Called::Views), None),
        Step::Skipped(_) => (call(a.outside_view, Called::OutsideView), None),
        Step::Viewed => (call(a.frame_wait, Called::FrameWait), Some(reads.idle)),
        Step::Waited { present: true } => (Action::Call { target: a.present, args: &[0, 0], kind: Called::Present }, None),
        Step::Waited { present: false } | Step::Presented => (Action::Return, None),
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
    pub presents: AtomicU64,
    pub loading: AtomicU64,
}

/// The feature: the frame step at `RENDER_FRAME`, with the native
/// renderer on. Not with `sim-frame`, which owns the point in sim mode.
#[derive(Clone)]
pub struct RenderFrame {
    addrs: Addrs,
    stats: Arc<Stats>,
}

impl Default for RenderFrame {
    fn default() -> Self {
        RenderFrame { addrs: Addrs::HALO2, stats: Arc::default() }
    }
}

impl RenderFrame {
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }
}

impl Feature for RenderFrame {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "render_frame_and_present as the Rust frame step").doc("docs/graphics/retire-d3d-display.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !hooks::switches::native_render_on(b.switches()) {
            return Ok(());
        }
        b.snapshot(SnapshotPolicy::Stateless);
        let me = self.clone();
        b.raw().implement(&points::RENDER_FRAME, "render: the frame step", move |cpu, ctx, _| {
            let frame = Frame { esp: cpu.esp, ret: ctx.memory().u32(cpu.esp)? };
            run(&me, frame, Step::Start, cpu, ctx)
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

fn run(me: &RenderFrame, frame: Frame, step: Step, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let reads = match step {
        Step::Start => Reads { world: world(&me.addrs, &*mem).map_err(fault)?, idle: false },
        Step::Viewed => Reads { world: false, idle: idle(&me.addrs, &*mem).map_err(fault)? },
        _ => Reads { world: false, idle: false },
    };
    let (action, present) = next(&me.addrs, step, reads);
    match action {
        Action::Return => {
            cpu.esp = frame.esp.wrapping_add(4);
            Ok(TransferAbi::ret(frame.ret))
        }
        Action::Call { target, args, kind } => {
            match kind {
                Called::Views => me.stats.views.fetch_add(1, Ordering::Relaxed),
                Called::OutsideView => me.stats.outside_view.fetch_add(1, Ordering::Relaxed),
                Called::Present => me.stats.presents.fetch_add(1, Ordering::Relaxed),
                Called::FrameWait if present == Some(false) => me.stats.loading.fetch_add(1, Ordering::Relaxed),
                _ => 0,
            };
            // Each call starts from the frame's own stack, its arguments
            // pushed right to left as the guest pushes them.
            cpu.esp = frame.esp;
            for &arg in args.iter().rev() {
                cpu.esp = cpu.esp.wrapping_sub(4);
                mem.set_u32(cpu.esp, arg)?;
            }
            let me = me.clone();
            ctx.call_guest_named(cpu, &mut mem, target, "render: the frame step", move |cpu, ctx| {
                let step = match kind {
                    Called::WorldSkip => Step::Skipped(cpu.eax as u8),
                    Called::Views | Called::OutsideView => Step::Viewed,
                    Called::FrameWait => Step::Waited { present: present.unwrap_or(false) },
                    Called::Present => Step::Presented,
                };
                run(&me, frame, step, cpu, ctx)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const A: Addrs = Addrs::HALO2;

    fn steps(world: bool, skip: u8, idle: bool) -> Vec<u32> {
        let mut out = Vec::new();
        let mut step = Step::Start;
        loop {
            let (action, present) = next(&A, step, Reads { world, idle });
            let Action::Call { target, args, kind } = action else { return out };
            out.push(target);
            if kind == Called::Present {
                assert_eq!(args, &[0, 0], "the present takes two zeros");
            }
            step = match kind {
                Called::WorldSkip => Step::Skipped(skip),
                Called::Views | Called::OutsideView => Step::Viewed,
                Called::FrameWait => Step::Waited { present: present.unwrap() },
                Called::Present => Step::Presented,
            };
        }
    }

    #[test]
    fn the_steps_are_the_guests_calls_in_its_order() {
        assert_eq!(steps(true, 0, true), [0x0006_8290, 0x0022_34e0, 0x0012_b2e0, 0x0001_3f10]);
        assert_eq!(steps(true, 1, true), [0x0006_8290, 0x0022_32a0, 0x0012_b2e0, 0x0001_3f10], "the UI's world skip");
        assert_eq!(steps(false, 0, true), [0x0022_32a0, 0x0012_b2e0, 0x0001_3f10], "no world: the skip is not asked");
        assert_eq!(steps(true, 0, false), [0x0006_8290, 0x0022_34e0, 0x0012_b2e0], "loading: waits, does not present");
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
