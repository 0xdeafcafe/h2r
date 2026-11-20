//! `sim-frame`: sim mode's world frames without the render front-end
//! (docs/runtime/sim.md §2 and §4). Off unless the boot's sim glue adds it.
//!
//! At `render_frame_and_present` (`0x00223240`) on the game thread:
//!
//! 1. **Decide** as the pipelined render does
//!    ([`pipeline_render`](crate::pipeline_render), `serial_reason`): a
//!    world frame is the world path taken (engine globals active, not the
//!    UI's world skip) with both resource-cache countdowns at or below 0.
//!    Menus, loading screens and Bink run the original, so loading drives
//!    the caches as always. A world frame runs the original too on every
//!    `render_every`th one (0: never).
//! 2. Otherwise **skip**: run the frame wait `0x0012b2e0` here (frame timing
//!    turns the vblanks it counts into the tick's `dt`), then the stubs the
//!    render's dependency census asks for ([`after_wait`]: the 64-bit
//!    present counter and the render-frame counter one on, the one-shot byte
//!    cleared), and return to the game.
//!
//! The other features' hooks on the point (the bots' pads before it and
//! their views back after it) run around this one as they would around the
//! original. The pipelined render implements the same point, so it must be
//! off when this is on (the plan refuses both).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use engine::memory::{GuestMemory, MemFault};
use engine::points;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "sim-frame";

/// The guest addresses the skip reads and writes; [`Addrs::HALO2`] for the
/// game, anything for a synthetic test. The same words as
/// [`pipeline_render::Addrs`](crate::pipeline_render::Addrs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// The engine frame wait, run on the game thread at a skipped frame.
    pub frame_wait: u32,
    /// `[engine_globals]`, `+0x1120` (byte) and `+2` (word): the world-path
    /// test at the top of `0x00223240`.
    pub engine_globals: u32,
    /// `0x00068290`'s globals, in its order: two bytes and a pointer.
    pub ui_state: (u32, u32, u32),
    /// Resource-cache countdowns: running ones mean loading.
    pub cache_countdowns: (u32, u32),
    /// The 64-bit present counter `rasterizer_present` adds to.
    pub present_counter: u32,
    /// The render-frame counter `render_views` adds to.
    pub frame_counter: u32,
    /// The byte the render clears per view and the sim sets.
    pub launch_clear: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        frame_wait: 0x0012_b2e0,
        engine_globals: 0x004e_6948,
        ui_state: (0x004c_f770, 0x004c_f772, 0x004c_f77c),
        cache_countdowns: (0x004e_6470, 0x004e_64a0),
        present_counter: 0x0048_5aa0,
        frame_counter: 0x004b_a034,
        launch_clear: 0x004e_6098,
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

/// The frame about to run, read from live memory (the prototype's
/// `serial_reason`, as a kind).
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
    render_every == 0 || world_index % u64::from(render_every) != 0
}

/// The stubs after the frame wait of a skipped frame: what the render
/// would have left that the sim reads (docs/runtime/sim.md §2).
pub fn after_wait(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    // The 64-bit present counter, one present on.
    let lo = g.u32(a.present_counter)?;
    let hi = g.u32(a.present_counter.wrapping_add(4))?;
    let v = (u64::from(hi) << 32 | u64::from(lo)).wrapping_add(1);
    g.write(a.present_counter, &v.to_le_bytes())?;
    // One view counted.
    let f = g.u32(a.frame_counter)?;
    g.write(a.frame_counter, &f.wrapping_add(1).to_le_bytes())?;
    // The one-shot byte the render clears.
    g.write(a.launch_clear, &[0])?;
    Ok(())
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
}

#[derive(Default)]
struct Shared {
    world: AtomicU64,
    skipped: AtomicU64,
    rendered_world: AtomicU64,
    other: AtomicU64,
    stub_faults: AtomicU64,
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
        }
    }

    /// The decision for the frame about to run, counted. `true`: skip it.
    pub fn decide(&self, g: &dyn GuestMemory) -> bool {
        let kind = frame_kind(&self.addrs, g);
        let s = &self.shared;
        if kind != FrameKind::World {
            s.other.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let i = s.world.fetch_add(1, Ordering::Relaxed);
        let skipping = skip(kind, i, self.render_every);
        if !skipping {
            s.rendered_world.fetch_add(1, Ordering::Relaxed);
        }
        skipping
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
                    bytes
                },
                move |bytes, _| {
                    let suffix = bytes
                        .strip_prefix(cadence.as_slice())
                        .ok_or("snapshot world render cadence differs")?;
                    let index = u64::from_le_bytes(
                        suffix
                            .try_into()
                            .map_err(|_| "invalid world frame cursor")?,
                    );
                    load.world.store(index, Ordering::Relaxed);
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
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    let me2 = me.clone();
    ctx.call_guest_named(
        cpu,
        &mut mem,
        me.addrs.frame_wait,
        "sim: frame wait in place of the render",
        move |cpu, ctx| {
            let eax = cpu.eax;
            *cpu = saved;
            cpu.eax = eax;
            {
                let rt = Arc::clone(ctx.runtime());
                let mut g = rt.memory();
                me2.skipped(&mut *g);
            }
            // `render_frame_and_present` takes no arguments: pop the
            // return address only.
            cpu.esp = saved.esp.wrapping_add(4);
            Ok(TransferAbi::ret(ret_pc))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const A: Addrs = Addrs {
        frame_wait: 0x1000,
        engine_globals: 0x2000,
        ui_state: (0x2010, 0x2011, 0x2014),
        cache_countdowns: (0x2020, 0x2024),
        present_counter: 0x2030,
        frame_counter: 0x2038,
        launch_clear: 0x203c,
    };
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
            let mut registry = crate::Registry::new(crate::Switches::default());
            registry.add(f).unwrap();
            registry.build(&crate::Base).unwrap()
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
        g.put_u32(A.present_counter, 0xffff_ffff)
            .put_u32(A.present_counter + 4, 7)
            .put_u32(A.frame_counter, 41)
            .put_u8(A.launch_clear, 1);
        after_wait(&A, &mut g).unwrap();
        assert_eq!(g.u32(A.present_counter), Ok(0));
        assert_eq!(g.u32(A.present_counter + 4), Ok(8));
        assert_eq!(g.u32(A.frame_counter), Ok(42));
        assert_eq!(g.u8(A.launch_clear), Ok(0));
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
        assert_eq!(g.u32(A.frame_counter), Ok(3));
        assert_eq!(
            f.stats(),
            Stats {
                skipped: 3,
                rendered_world: 3,
                other: 1,
                stub_faults: 0
            }
        );
        // A clone shares the counts.
        assert_eq!(f.clone().stats().skipped, 3);
        let mut empty = FakeGuest::new();
        f.skipped(&mut empty);
        assert_eq!(f.stats().stub_faults, 1);
    }
}
