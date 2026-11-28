//! `sim-frame`: sim mode's world frames without the render front-end
//! (docs/runtime/sim.md §2 and §4). Off unless the boot's sim glue adds it.
//!
//! At `render_frame_and_present` (`0x00223240`) on the game thread:
//!
//! 1. **Decide**: a
//!    world frame is the world path taken (engine globals active, not the
//!    UI's world skip) with both resource-cache countdowns at or below 0.
//!    Menus, loading screens and Bink run the original, so loading drives
//!    the caches as always. A world frame runs the original too on every
//!    `render_every`th one (0: never).
//! 2. Otherwise **skip**: the frame wait `0x0012b2e0` in Rust
//!    (`engine::render_loop::frame_wait`; frame timing turns the vblanks it
//!    counts into the tick's `dt`), yielding the slice to the kernel
//!    (`NtYieldExecution`) until the clocked vblank source reaches the
//!    target, then the frame's tick writes ([`after_wait`]:
//!    `engine::render_loop::tick_writes`, the one source the render frame
//!    step uses too), and return to the game.
//! 3. **Check**: a world frame the original renders is checked after it
//!    returns: the guest's three words against the Rust prediction. A
//!    difference is counted by word and falls back for the session: every
//!    later world frame renders.
//!
//! The other features' hooks on the point (the bots' pads before it and
//! their views back after it) run around this one as they would around the
//! original.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::render_loop::frame_wait::{self, Watch};
use engine::render_loop::tick_writes::{self, Word, Words};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "sim-frame";

/// The guest addresses the skip reads and writes; [`Addrs::HALO2`] for the
/// game, anything for a synthetic test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// The engine frame wait's globals, made in Rust at a skipped frame
    /// (`engine::render_loop::frame_wait`).
    pub wait: frame_wait::Addrs,
    /// `[engine_globals]`, `+0x1120` (byte) and `+2` (word): the world-path
    /// test at the top of `0x00223240`.
    pub engine_globals: u32,
    /// `0x00068290`'s globals, in its order: two bytes and a pointer.
    pub ui_state: (u32, u32, u32),
    /// Resource-cache countdowns: running ones mean loading.
    pub cache_countdowns: (u32, u32),
    /// The frame's tick writes: the present counter, the render frame
    /// counter and the one-shot byte.
    pub ticks: tick_writes::Addrs,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        wait: frame_wait::Addrs::HALO2,
        engine_globals: 0x004e_6948,
        ui_state: (0x004c_f770, 0x004c_f772, 0x004c_f77c),
        cache_countdowns: (0x004e_6470, 0x004e_64a0),
        ticks: tick_writes::Addrs::HALO2,
    };
}

/// What kind of frame the guest is about to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// The world path: skippable.
    World,
    /// A menu, the UI's world skip, or no game.
    NotWorld,
    /// The world, with the resource caches still loading.
    Loading,
    /// The words could not be read.
    Unreadable,
}

/// The frame about to run, read from live memory.
pub fn frame_kind(a: &Addrs, g: &dyn GuestMemory) -> FrameKind {
    let world = (|| -> Result<bool, MemFault> {
        let w = g.u32(a.engine_globals)?;
        if w == 0 || g.u8(w.wrapping_add(0x1120))? == 0 || g.u16(w.wrapping_add(2))? == 0xffff {
            return Ok(false);
        }
        // 0x00068290: true (skip the world) when its first byte is set, its
        // second clear, and the record it points at is active and not in
        // state 4.
        let (on, off, rec) = a.ui_state;
        if g.u8(on)? != 0 && g.u8(off)? == 0 {
            let r = g.u32(rec)?;
            if g.u32(r.wrapping_add(8))? != 0 && g.u32(r.wrapping_add(0x18))? != 4 {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    match world {
        Err(_) => return FrameKind::Unreadable,
        Ok(false) => return FrameKind::NotWorld,
        Ok(true) => {}
    }
    let (c0, c1) = a.cache_countdowns;
    match (g.i32(c0), g.i32(c1)) {
        (Ok(x), Ok(y)) if x <= 0 && y <= 0 => FrameKind::World,
        (Ok(_), Ok(_)) => FrameKind::Loading,
        _ => FrameKind::Unreadable,
    }
}

/// Whether to skip a frame of `kind`, the `world_index`th world frame of
/// the run (from 0), rendering every `render_every`th world frame (0:
/// never). Only world frames are skipped.
pub fn skip(kind: FrameKind, world_index: u64, render_every: u32) -> bool {
    if kind != FrameKind::World {
        return false;
    }
    render_every == 0 || !world_index.is_multiple_of(u64::from(render_every))
}

/// The stubs after the frame wait of a skipped frame: what the render
/// would have left that the sim reads (docs/runtime/sim.md §2), and nothing
/// else. A fault writes nothing.
pub fn after_wait(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    tick_writes::frame(&a.ticks, g)
}

/// Whether the caches are idle after a frame, so it presented.
pub fn idle_after(a: &Addrs, g: &dyn GuestMemory) -> bool {
    let (c0, c1) = a.cache_countdowns;
    matches!((g.i32(c0), g.i32(c1)), (Ok(x), Ok(y)) if x <= 0 && y <= 0)
}

/// Frames so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// World frames skipped.
    pub skipped: u64,
    /// World frames rendered (`render_every`).
    pub rendered_world: u64,
    /// Other frames, all rendered (menus, loading).
    pub other: u64,
    /// Skipped frames whose stubs could not be written.
    pub stub_faults: u64,
    /// Rendered world frames whose guest words were checked against
    /// [`Words::predict`].
    pub checked: u64,
    /// Checked frames whose guest word differed, by word.
    pub mismatch_frame_counter: u64,
    pub mismatch_one_shot: u64,
    pub mismatch_present_counter: u64,
    /// A check differed: every later world frame renders.
    pub fell_back: bool,
}

#[derive(Default)]
struct Shared {
    world: AtomicU64,
    skipped: AtomicU64,
    rendered_world: AtomicU64,
    other: AtomicU64,
    stub_faults: AtomicU64,
    checked: AtomicU64,
    mismatch: [AtomicU64; 3],
    fell_back: AtomicBool,
    /// The words before a rendered world frame, for the check after it.
    pending: Mutex<Option<Words>>,
    said_skipping: AtomicBool,
    said_fault: AtomicBool,
}

type OnSkip = dyn Fn() + Send + Sync;

/// The feature. Cheap to clone; clones share the counts.
#[derive(Clone)]
pub struct SimFrame {
    addrs: Addrs,
    render_every: u32,
    shared: Arc<Shared>,
    on_skip: Option<Arc<OnSkip>>,
}

impl SimFrame {
    /// Skips every world frame but each `render_every`th (0: all).
    pub fn new(render_every: u32) -> Self {
        Self::with(Addrs::HALO2, render_every)
    }

    pub fn with(addrs: Addrs, render_every: u32) -> Self {
        SimFrame {
            addrs,
            render_every,
            shared: Arc::default(),
            on_skip: None,
        }
    }

    /// `f` runs on the game thread after each skipped frame's stubs (the
    /// host's present count moves on there).
    pub fn on_skip(mut self, f: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_skip = Some(Arc::new(f));
        self
    }

    pub fn render_every(&self) -> u32 {
        self.render_every
    }

    pub fn stats(&self) -> Stats {
        let s = &self.shared;
        Stats {
            skipped: s.skipped.load(Ordering::Relaxed),
            rendered_world: s.rendered_world.load(Ordering::Relaxed),
            other: s.other.load(Ordering::Relaxed),
            stub_faults: s.stub_faults.load(Ordering::Relaxed),
            checked: s.checked.load(Ordering::Relaxed),
            mismatch_frame_counter: s.mismatch[0].load(Ordering::Relaxed),
            mismatch_one_shot: s.mismatch[1].load(Ordering::Relaxed),
            mismatch_present_counter: s.mismatch[2].load(Ordering::Relaxed),
            fell_back: s.fell_back.load(Ordering::Relaxed),
        }
    }

    /// The decision for the frame about to run, counted. `true`: skip it.
    pub fn decide(&self, g: &dyn GuestMemory) -> bool {
        let kind = frame_kind(&self.addrs, g);
        let s = &self.shared;
        *s.pending.lock().unwrap() = None;
        if kind != FrameKind::World {
            s.other.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let i = s.world.fetch_add(1, Ordering::Relaxed);
        let skipping = !s.fell_back.load(Ordering::Relaxed) && skip(kind, i, self.render_every);
        if !skipping {
            s.rendered_world.fetch_add(1, Ordering::Relaxed);
            *s.pending.lock().unwrap() = Words::read(&self.addrs.ticks, g).ok();
        }
        skipping
    }

    /// After a frame: a rendered world frame's guest words against the
    /// prediction from the words before it. Reads only. A frame that left
    /// the caches loading did not present and is not checked.
    pub fn check(&self, g: &dyn GuestMemory) {
        let s = &self.shared;
        let Some(before) = s.pending.lock().unwrap().take() else {
            return;
        };
        if !idle_after(&self.addrs, g) {
            return;
        }
        let Ok(after) = Words::read(&self.addrs.ticks, g) else {
            return;
        };
        s.checked.fetch_add(1, Ordering::Relaxed);
        let diff = after.diff(before.predict());
        for w in &diff {
            let at = Word::ALL.iter().position(|x| x == w).expect("a word");
            s.mismatch[at].fetch_add(1, Ordering::Relaxed);
        }
        if !diff.is_empty() && !s.fell_back.swap(true, Ordering::Relaxed) {
            let names: Vec<_> = diff.iter().map(|w| w.name()).collect();
            eprintln!(
                "sim: a rendered frame's tick writes differ from the skip's ({}); every world frame renders from here",
                names.join(", ")
            );
        }
    }

    /// The stubs, counted, and the callback.
    pub fn skipped(&self, g: &mut dyn GuestMemory) {
        let s = &self.shared;
        if let Err(e) = after_wait(&self.addrs, g) {
            s.stub_faults.fetch_add(1, Ordering::Relaxed);
            if !s.said_fault.swap(true, Ordering::Relaxed) {
                eprintln!("sim: a skipped frame's stubs faulted ({e:?}); counted in the report");
            }
        }
        if !s.said_skipping.swap(true, Ordering::Relaxed) {
            eprintln!(
                "sim: skipping the world render (every frame{})",
                if self.render_every == 0 {
                    String::new()
                } else {
                    format!(" but each {}th", self.render_every)
                }
            );
        }
        s.skipped.fetch_add(1, Ordering::Relaxed);
        if let Some(f) = &self.on_skip {
            f();
        }
    }
}

impl Feature for SimFrame {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Sim mode: world frames without the render front-end")
            .doc("docs/runtime/sim.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Every nth world frame takes a different guest path. Restoring
        // this index preserves the same render/skip choice after a seek.
        if self.render_every > 1 {
            let save = self.shared.clone();
            let load = self.shared.clone();
            let cadence = self.render_every.to_le_bytes();
            b.snapshot(SnapshotPolicy::section(
                move || {
                    let mut bytes = Vec::from(cadence);
                    bytes.extend(save.world.load(Ordering::Relaxed).to_le_bytes());
                    bytes.push(u8::from(save.fell_back.load(Ordering::Relaxed)));
                    bytes
                },
                move |bytes, _| {
                    let suffix = bytes
                        .strip_prefix(cadence.as_slice())
                        .ok_or("snapshot world render cadence differs")?;
                    let (&fell_back, index) =
                        suffix.split_last().ok_or("invalid world frame cursor")?;
                    let index = u64::from_le_bytes(
                        index.try_into().map_err(|_| "invalid world frame cursor")?,
                    );
                    load.world.store(index, Ordering::Relaxed);
                    load.fell_back.store(fell_back != 0, Ordering::Relaxed);
                    Ok(())
                },
            ));
        } else {
            b.snapshot(SnapshotPolicy::Stateless);
        }
        let me = self.clone();
        b.raw().implement(
            &points::RENDER_FRAME,
            "sim: world frames skip the render",
            move |cpu, ctx, addr| frame(&me, cpu, ctx, addr),
        );
        let me = self.clone();
        b.raw().observe_after(
            &points::RENDER_FRAME,
            "sim: a rendered frame's tick writes against the skip's",
            move |o| me.check(&*o.guest()),
        );
        Ok(())
    }
}

/// The implementation at `render_frame_and_present`.
fn frame(
    me: &SimFrame,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let skipping = {
        let g = rt.memory();
        me.decide(&*g)
    };
    if !skipping {
        let original = rt
            .resolve_original(addr)
            .expect("the plan installs this point over a generated original");
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let saved = *cpu;
    let ret_pc = {
        let mut mem = rt.memory();
        frame_wait::enter(&me.addrs.wait, &mut *mem).map_err(fault)?;
        mem.u32(cpu.esp)?
    };
    wait(me.clone(), Watch::default(), saved, ret_pc, cpu, ctx)
}

/// `NtYieldExecution`: the kernel service a waiting frame gives its slice
/// up through, so other threads run or the clock moves to the next
/// vblank's deadline.
const NT_YIELD_EXECUTION: u32 = 238;

/// The wait's globals faulted: the guest's own access would have.
fn fault(e: MemFault) -> GuestFault {
    GuestFault {
        addr: e.addr,
        len: e.len as usize,
        access: if e.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// One check of the skipped frame's wait: over, the exit's writes, the
/// tick writes and the return; else a yield and the next check. A wait no
/// vblank will end stops by name.
fn wait(
    me: SimFrame,
    mut watch: Watch,
    saved: CpuAbi,
    ret_pc: u32,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    // No memory lock is held here: the kernel's clock takes the kernel lock.
    let now = rt.guest_time();
    let mut mem = rt.memory();
    let a = me.addrs.wait;
    if let Some(exit) = frame_wait::exit(&a, &mut *mem).map_err(fault)? {
        *cpu = saved;
        cpu.eax = exit.eax;
        me.skipped(&mut *mem);
        // `render_frame_and_present` takes no arguments: pop the return
        // address only.
        cpu.esp = saved.esp.wrapping_add(4);
        return Ok(TransferAbi::ret(ret_pc));
    }
    let (counter, target) = frame_wait::read(&a, &*mem).map_err(fault)?;
    if let Err(why) = watch.step(counter, target, now) {
        panic!("sim frame: frame wait: {why}");
    }
    *cpu = saved;
    let token = runtime::kernel::function_token(NT_YIELD_EXECUTION);
    ctx.call_guest_named(cpu, &mut mem, token, "sim: the frame wait's yield", move |cpu, ctx| {
        wait(me, watch, saved, ret_pc, cpu, ctx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const A: Addrs = Addrs {
        wait: frame_wait::Addrs {
            counter: 0x2040,
            target: 0x2048,
            entry_count: 0x2050,
            exit_count: 0x2058,
            frame_gate: 0x2060,
            late_flag: 0x2064,
        },
        engine_globals: 0x2000,
        ui_state: (0x2010, 0x2011, 0x2014),
        cache_countdowns: (0x2020, 0x2024),
        ticks: tick_writes::Addrs {
            present_counter: 0x2030,
            frame_counter: 0x2038,
            one_shot: 0x203c,
        },
    };
    const T: tick_writes::Addrs = A.ticks;
    const GLOBALS: u32 = 0x3000;
    const UI_REC: u32 = 0x5000;

    /// A guest in a world frame: globals active, no UI skip, caches idle.
    fn world() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x2000, 0x100)
            .map(GLOBALS, 0x2000)
            .map(UI_REC, 0x100)
            .put_u32(A.engine_globals, GLOBALS)
            .put_u8(GLOBALS + 0x1120, 1)
            .put_u16(GLOBALS + 2, 0)
            .put_u32(A.ui_state.2, UI_REC);
        g
    }

    #[test]
    fn a_world_frame() {
        assert_eq!(frame_kind(&A, &world()), FrameKind::World);
    }

    #[test]
    fn menus_and_loading_are_not() {
        let mut g = world();
        g.put_u32(A.engine_globals, 0);
        assert_eq!(frame_kind(&A, &g), FrameKind::NotWorld);
        let mut g = world();
        g.put_u8(GLOBALS + 0x1120, 0);
        assert_eq!(frame_kind(&A, &g), FrameKind::NotWorld);
        let mut g = world();
        g.put_u16(GLOBALS + 2, 0xffff);
        assert_eq!(frame_kind(&A, &g), FrameKind::NotWorld);
        // The UI's world skip: first byte set, second clear, its record
        // active and not in state 4.
        let mut g = world();
        g.put_u8(A.ui_state.0, 1)
            .put_u32(UI_REC + 8, 1)
            .put_u32(UI_REC + 0x18, 2);
        assert_eq!(frame_kind(&A, &g), FrameKind::NotWorld);
        g.put_u32(UI_REC + 0x18, 4);
        assert_eq!(frame_kind(&A, &g), FrameKind::World);
        g.put_u32(UI_REC + 0x18, 2).put_u8(A.ui_state.1, 1);
        assert_eq!(frame_kind(&A, &g), FrameKind::World);
        // A countdown running: loading. Negative is done.
        let mut g = world();
        g.put_u32(A.cache_countdowns.1, 3);
        assert_eq!(frame_kind(&A, &g), FrameKind::Loading);
        g.put_u32(A.cache_countdowns.1, (-2i32) as u32);
        assert_eq!(frame_kind(&A, &g), FrameKind::World);
        // Globals pointing nowhere.
        let mut g = world();
        g.put_u32(A.engine_globals, 0x9000_0000);
        assert_eq!(frame_kind(&A, &g), FrameKind::Unreadable);
    }

    #[test]
    fn render_every() {
        let w = FrameKind::World;
        assert!((0..10).all(|i| skip(w, i, 0)));
        let rendered: Vec<u64> = (0..10).filter(|&i| !skip(w, i, 3)).collect();
        assert_eq!(rendered, [0, 3, 6, 9]);
        assert!((0..5).all(|i| !skip(w, i, 1)));
        for k in [
            FrameKind::NotWorld,
            FrameKind::Loading,
            FrameKind::Unreadable,
        ] {
            assert!(!skip(k, 1, 0), "{k:?}");
        }
    }

    #[test]
    fn restoring_a_world_frame_cursor_resumes_on_the_same_render() {
        let plan = |f: &SimFrame| {
            let mut registry = hooks::Registry::new(hooks::Switches::default());
            registry.add(f).unwrap();
            registry.build(&hooks::Base).unwrap()
        };
        let g = world();
        let original = SimFrame::with(A, 3);
        let source = plan(&original);
        assert!(!original.decide(&g)); // frame 0
        let sections = source
            .save_sections()
            .into_iter()
            .map(|(name, bytes)| (name.to_owned(), bytes))
            .collect::<Vec<_>>();
        let replay = SimFrame::with(A, 3);
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        plan(&replay).load_sections(&sections, &rt).unwrap();
        for _ in 0..5 {
            assert_eq!(original.decide(&g), replay.decide(&g));
        }
        assert!(
            plan(&SimFrame::with(A, 2))
                .load_sections(&sections, &rt)
                .is_err()
        );
    }

    #[test]
    fn stubs_after_the_wait() {
        let mut g = world();
        g.put_u32(T.present_counter, 0xffff_ffff)
            .put_u32(T.present_counter + 4, 7)
            .put_u32(T.frame_counter, 41)
            .put_u8(T.one_shot, 1);
        after_wait(&A, &mut g).unwrap();
        assert_eq!(g.u32(T.present_counter), Ok(0));
        assert_eq!(g.u32(T.present_counter + 4), Ok(8));
        assert_eq!(g.u32(T.frame_counter), Ok(42));
        assert_eq!(g.u8(T.one_shot), Ok(0));
        // Unmapped: a fault, nothing half written.
        let mut empty = FakeGuest::new();
        assert!(after_wait(&A, &mut empty).is_err());
    }

    #[test]
    fn decisions_are_counted() {
        let skipped = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&skipped);
        let f = SimFrame::with(A, 2).on_skip(move || {
            seen.fetch_add(1, Ordering::Relaxed);
        });
        let mut g = world();
        let mut menu = world();
        menu.put_u32(A.engine_globals, 0);
        assert!(!f.decide(&menu));
        let mut skips = 0;
        for _ in 0..6 {
            if f.decide(&g) {
                f.skipped(&mut g);
                skips += 1;
            }
        }
        assert_eq!(skips, 3);
        assert_eq!(skipped.load(Ordering::Relaxed), 3);
        assert_eq!(g.u32(T.frame_counter), Ok(3));
        assert_eq!(
            f.stats(),
            Stats {
                skipped: 3,
                rendered_world: 3,
                other: 1,
                ..Stats::default()
            }
        );
        // A clone shares the counts.
        assert_eq!(f.clone().stats().skipped, 3);
        let mut empty = FakeGuest::new();
        f.skipped(&mut empty);
        assert_eq!(f.stats().stub_faults, 1);
    }

    /// The words the headless step may write: the three tick words, nothing
    /// else on their pages or elsewhere.
    #[test]
    fn the_headless_step_makes_only_tick_writes() {
        let mut g = world();
        g.put_u32(T.present_counter, 9)
            .put_u32(T.frame_counter, 4)
            .put_u8(T.one_shot, 1);
        let mut before = Vec::new();
        for page in [0x2000u32, GLOBALS, GLOBALS + 0x1000, UI_REC] {
            before.push(g.bytes(page, 0x1000).unwrap());
        }
        let f = SimFrame::with(A, 0);
        assert!(f.decide(&g));
        f.skipped(&mut g);
        let ticks = [
            (T.present_counter, 8),
            (T.frame_counter, 4),
            (T.one_shot, 1),
        ];
        for (page, was) in [0x2000u32, GLOBALS, GLOBALS + 0x1000, UI_REC]
            .into_iter()
            .zip(before)
        {
            let now = g.bytes(page, 0x1000).unwrap();
            for (at, (a, b)) in was.iter().zip(&now).enumerate() {
                let addr = page + at as u32;
                let tick = ticks.iter().any(|&(w, len)| (w..w + len).contains(&addr));
                assert!(tick || a == b, "the step wrote {addr:#x}, not a tick word");
            }
        }
        assert_eq!(
            Words::read(&T, &g).unwrap(),
            Words {
                frame_counter: 5,
                one_shot: 0,
                present_counter: 10
            }
        );
    }

    /// A rendered world frame is checked: equal words pass, a word the guest
    /// left otherwise is counted by name and every later world frame renders.
    #[test]
    fn a_rendered_frame_that_differs_falls_back_for_the_session() {
        let f = SimFrame::with(A, 2);
        let mut g = world();
        // Frame 0 renders: the guest makes the three writes.
        assert!(!f.decide(&g));
        tick_writes::frame(&T, &mut g).unwrap();
        f.check(&g);
        assert_eq!((f.stats().checked, f.stats().fell_back), (1, false));
        // Frame 1 skips; frame 2 renders and the guest presents twice.
        assert!(f.decide(&g));
        f.skipped(&mut g);
        f.check(&g);
        assert_eq!(f.stats().checked, 1, "a skipped frame is not checked");
        assert!(!f.decide(&g));
        tick_writes::frame(&T, &mut g).unwrap();
        tick_writes::present_counter(&T, &mut g).unwrap();
        f.check(&g);
        let s = f.stats();
        assert_eq!(
            (
                s.checked,
                s.mismatch_present_counter,
                s.mismatch_frame_counter,
                s.mismatch_one_shot,
                s.fell_back
            ),
            (2, 1, 0, 0, true)
        );
        assert!((0..4).all(|_| !f.decide(&g)), "every world frame renders");
        // A frame that leaves the caches loading did not present: unchecked.
        let f = SimFrame::with(A, 0);
        let mut g = world();
        f.shared.fell_back.store(true, Ordering::Relaxed);
        assert!(!f.decide(&g));
        g.put_u32(A.cache_countdowns.0, 5);
        f.check(&g);
        assert_eq!(f.stats().checked, 0);
    }
}
