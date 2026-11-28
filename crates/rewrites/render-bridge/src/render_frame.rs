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
//! frame goes native. At the present, [`publish`] shows a frame whose
//! every slot drew from the scene in place of the guest's present, and
//! counts every frame that keeps it by name.
//!
//! Three frame loops outside `0x00223240` run a view, the frame wait and
//! the present themselves: `0x0012b6f0` (`0x0012b784`) and the map load's
//! two (`0x00163a28`, `0x00163adb`). Their present asks the same publish
//! ([`loop_present`]); a kept one runs the guest's body.

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
    /// The guest's presents run.
    pub presents: AtomicU64,
    /// Frames shown from the scene, the guest's present skipped.
    pub published: AtomicU64,
    pub loading: AtomicU64,
    /// A frame loop's presents ([`LOOP_RETURNS`]): the guest's run, and
    /// those shown from the scene.
    pub loop_presents: AtomicU64,
    pub loop_published: AtomicU64,
}

/// Where the present returns in the three frame loops outside
/// `0x00223240` (**Code**: `0x0012b784`, `0x00163a28`, `0x00163adb`, each
/// after `push 0; push 0`).
pub const LOOP_RETURNS: [u32; 3] = [0x0012_b789, 0x0016_3a2d, 0x0016_3ae0];

/// Whether a call of the present with return address `ret` and first
/// argument `readback` is a frame loop's: one of [`LOOP_RETURNS`], with no
/// readback asked. The frame step's own call returns elsewhere.
pub fn loop_frame(ret: u32, readback: u32) -> bool {
    LOOP_RETURNS.contains(&ret) && readback == 0
}

/// The feature: the frame step at `RENDER_FRAME`, with the native
/// renderer on. Not with `sim-frame`, which owns the point in sim mode.
#[derive(Clone)]
pub struct RenderFrame {
    addrs: Addrs,
    stats: Arc<Stats>,
    /// The host's publish; `None` asks [`publish::installed`] each frame.
    publish: Option<Arc<dyn publish::Publish>>,
}

impl Default for RenderFrame {
    fn default() -> Self {
        RenderFrame { addrs: Addrs::HALO2, stats: Arc::default(), publish: None }
    }
}

impl RenderFrame {
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }

    /// Publishes through `publish` rather than the installed host's.
    pub fn with_publish(mut self, publish: Arc<dyn publish::Publish>) -> Self {
        self.publish = Some(publish);
        self
    }
}

/// The step after the frame wait: [`Step::Presented`] when the frame was
/// shown from the scene, so the guest's present is not called.
pub fn after_wait(step: Step, outcome: Option<publish::Outcome>) -> Step {
    match (step, outcome) {
        (Step::Waited { present: true }, Some(publish::Outcome::Native)) => Step::Presented,
        _ => step,
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
        b.raw().implement(&points::RENDER_PRESENT, "render: a frame loop's present", move |cpu, ctx, addr| {
            loop_present(&me, cpu, ctx, addr)
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

/// The present `0x00013f10`: a frame loop's ([`loop_frame`]) asks the
/// publish and, shown from the scene, returns as the guest's `ret 8` with
/// the present counter's high word in EAX; every other call, and a kept
/// loop frame, runs the guest's body.
fn loop_present(me: &RenderFrame, cpu: &mut CpuAbi, ctx: &mut ExecCtx, addr: u32) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let published = {
        let mut mem = rt.memory();
        let ret = mem.u32(cpu.esp)?;
        let readback = mem.u32(cpu.esp.wrapping_add(4))?;
        if loop_frame(ret, readback) {
            let host = me.publish.clone().or_else(publish::installed);
            let a = publish::Addrs::HALO2;
            let outcome = publish::present(&a, host.as_deref(), &mut *mem).map_err(fault)?;
            if outcome == publish::Outcome::Native {
                me.stats.loop_published.fetch_add(1, Ordering::Relaxed);
                cpu.eax = mem.u32(a.ticks.present_counter.wrapping_add(4))?;
                true
            } else {
                me.stats.loop_presents.fetch_add(1, Ordering::Relaxed);
                false
            }
        } else {
            false
        }
    };
    if published {
        return ctx.memory().ret(cpu, 8);
    }
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(TransferAbi::fault(event))
}

fn run(me: &RenderFrame, frame: Frame, step: Step, cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let outcome = match step {
        Step::Waited { present: true } => {
            let host = me.publish.clone().or_else(publish::installed);
            Some(publish::present(&publish::Addrs::HALO2, host.as_deref(), &mut *mem).map_err(fault)?)
        }
        _ => None,
    };
    if outcome == Some(publish::Outcome::Native) {
        me.stats.published.fetch_add(1, Ordering::Relaxed);
    }
    let step = after_wait(step, outcome);
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
        steps_published(world, skip, idle, None)
    }

    /// The calls, with the publish answering `outcome` at the present.
    fn steps_published(world: bool, skip: u8, idle: bool, outcome: Option<publish::Outcome>) -> Vec<u32> {
        let mut out = Vec::new();
        let mut step = Step::Start;
        loop {
            step = after_wait(step, outcome);
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
    fn a_published_frame_skips_the_guests_present_and_a_kept_one_calls_it() {
        use publish::{Kept, Outcome};
        assert_eq!(
            steps_published(true, 0, true, Some(Outcome::Native)),
            [0x0006_8290, 0x0022_34e0, 0x0012_b2e0],
            "shown from the scene: the frame wait, no 0x00013f10"
        );
        assert_eq!(
            steps_published(true, 0, true, Some(Outcome::Kept(Kept::Slot("opaque")))),
            [0x0006_8290, 0x0022_34e0, 0x0012_b2e0, 0x0001_3f10],
            "a carried slot keeps the guest's present"
        );
        assert_eq!(
            steps_published(true, 0, false, Some(Outcome::Native)),
            [0x0006_8290, 0x0022_34e0, 0x0012_b2e0],
            "loading presents neither way"
        );
    }

    /// The three frame loops' presents ask the publish; the frame step's
    /// own call, a readback and any other caller do not.
    #[test]
    fn only_a_frame_loops_present_asks_the_publish() {
        for ret in LOOP_RETURNS {
            assert!(loop_frame(ret, 0), "{ret:#x}");
            assert!(!loop_frame(ret, 0x0070_0000), "a readback is the guest's");
        }
        assert!(!loop_frame(0x0022_329b, 0), "the frame step's present is decided there");
    }

    /// Each loop's call of the present returns to the instruction after
    /// its `call`: the code there is the image's (`h2 disasm`), and each
    /// pushes two zeros first.
    #[test]
    fn the_loop_returns_follow_their_calls() {
        // `e8 rel32` is five bytes: the return is the call site plus 5.
        assert_eq!(LOOP_RETURNS, [0x0012_b784 + 5, 0x0016_3a28 + 5, 0x0016_3adb + 5]);
    }

    /// A loop frame drawn wholly from the scene is shown by the publish:
    /// no guest D3D entry (the body that calls `Swap` does not run), the
    /// counter moved as `0x000141ae` moves it. A kept one leaves it all
    /// to the guest's body.
    #[test]
    fn a_loop_frame_from_the_scene_is_published_with_no_d3d_entry() {
        use publish::{Open, Outcome, Publish, RAMP_BYTES};
        struct Scene(Open, std::sync::Mutex<u32>);
        impl Publish for Scene {
            fn open(&self, _: &mut dyn GuestMemory) -> Open {
                self.0
            }
            fn present(&self, _: &mut dyn GuestMemory, _: Option<&[u8; RAMP_BYTES]>) -> Result<(), String> {
                *self.1.lock().unwrap() += 1;
                Ok(())
            }
        }
        let a = publish::Addrs::HALO2;
        let mut g = FakeGuest::new();
        g.map(0x0040_7000, 0x1000).map(0x0046_7000, 0x1000).map(0x0048_5000, 0x1000).map(0x0050_9000, 0x1000);
        g.put_u32(a.ticks.present_counter, u32::MAX);
        let scene = Scene(Open::Scene, std::sync::Mutex::new(0));
        assert!(loop_frame(LOOP_RETURNS[1], 0));
        assert_eq!(publish::present(&a, Some(&scene), &mut g).unwrap(), Outcome::Native);
        assert_eq!(*scene.1.lock().unwrap(), 1, "the scene presents, the guest's Swap does not run");
        assert_eq!(g.u32(a.ticks.present_counter).unwrap(), 0);
        assert_eq!(g.u32(a.ticks.present_counter + 4).unwrap(), 1, "the carry, as the guest's adc makes it");
        let kept = Scene(Open::Carried("outside"), std::sync::Mutex::new(0));
        assert_eq!(publish::present(&a, Some(&kept), &mut g).unwrap().name(), "publish_kept_outside");
        assert_eq!(*kept.1.lock().unwrap(), 0);
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
