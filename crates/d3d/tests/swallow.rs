//! `Device::swallow` (docs/graphics/native-renderer.md, stage 5): the
//! glow's quads the native renderer draws itself, dropped on the device
//! while the game's own code runs and leaves every state. A pass of three
//! inline quads (two into a texture level, the last into the back
//! buffer, as the glow's down-sample, blur and composite), then a quad
//! after it (a screen effect's). The swallow drops exactly the planned
//! draws and nothing after, places its pass after the last; a draw that
//! is not the next planned ends it (that draw and the rest are drawn, the
//! pass is not placed); observing draws everything and still places the
//! pass. On the device's thread and through the render thread. The pass
//! is colour-only: issued with no depth bound (the composite binds none).
//! Dropping is refused up front, nothing dropped, when the pass could not
//! be issued: a backend that takes no external passes (a headless run), or
//! a pass that needs depth. Synthetic device and recording backend (which
//! takes the pass when asked to, recording its surfaces; the device hands
//! it its constants when it is placed).

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use common::*;
use d3d::{
    Backend, Device, ExternalPass, ExternalRefusal, ExternalTarget, GuestMemory, RecordingBackend, SparseMemory,
    Swallow, ThreadedBackend, UploadPolicy,
};
use gfx::vertex::PrimitiveType;

const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const SIZE: u32 = 32;
const LEN: usize = (SIZE * SIZE * 4) as usize;

/// A 32x32 `LIN_A8R8G8B8` texture at `TEX_A` with its level at `LEVEL_A`
/// (pitch 128), as `rtt_direct` makes them.
fn level(m: &mut SparseMemory) {
    m.map(0x8000_0000 | DATA_A, LEN as u32);
    m.write(0x8000_0000 | DATA_A, &vec![0x30; LEN]).unwrap();
    let format = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = (SIZE - 1) | ((SIZE - 1) << 12) | ((128 / 64 - 1) << 24);
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, DATA_A, 0, format, size].iter().enumerate() {
        m.write_u32(TEX_A + 4 * i as u32, *w).unwrap();
    }
    for (i, w) in [1 | (5 << 16) | 0x0100_0000, DATA_A, 0, format, size, TEX_A].iter().enumerate() {
        m.write_u32(LEVEL_A + 4 * i as u32, *w).unwrap();
    }
}

/// Counts the times it is placed (handed the view constants); colour
/// only unless it says otherwise.
struct Placed(Arc<AtomicU32>, bool);

impl ExternalPass for Placed {
    fn encode(&mut self, _t: ExternalTarget<'_>) {}
    fn view_constants(&mut self, _c: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn colour_only(&self) -> bool {
        self.1
    }
}

/// A recording backend that takes external passes.
fn takes() -> RecordingBackend {
    let mut b = RecordingBackend::default();
    b.external_passes = true;
    b
}

fn setup<B: Backend>(b: B) -> (Device<B>, SparseMemory, u32) {
    let (mut d, mut m) = create(b, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    bind_quad(&mut d, &mut m);
    level(&mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    (d, m, back)
}

fn quad(m: &mut SparseMemory) {
    inline_quad(m, 0.0, 0.0, 8.0, 8.0, [255, 255, 255, 255]);
}

/// The pass: two quads into the level (slot 1), the last into the target
/// bound when armed (slot 0).
fn plan(drop: bool, placed: &Arc<AtomicU32>) -> Swallow {
    Swallow {
        targets: vec![1, 1, 0],
        primitive: PrimitiveType::QuadList,
        vertices: 4,
        shapes: Vec::new(),
        drop,
        pass: Box::new(Placed(Arc::clone(placed), true)),
    }
}

/// A quad before arming, the pass as planned, a quad after it. Returns
/// the draws handed to the backend at each step: (before, after the
/// level's two, after the last, after the one following).
fn as_planned<B: Backend>(b: B, drop: bool) -> (Device<B>, [u64; 4], Arc<AtomicU32>) {
    let (mut d, mut m, back) = setup(b);
    let placed = Arc::new(AtomicU32::new(0));
    quad(&mut m);
    assert!(d.swallow(&mut m, plan(drop, &placed)).unwrap());
    let before = d.stats().draws;
    assert_eq!(before, 1, "what was queued before drew first");
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m);
    quad(&mut m);
    d.set_render_target(&mut m, back, 0).unwrap();
    let level = d.stats().draws;
    assert_eq!(placed.load(Ordering::Relaxed), 0, "not before the last draw");
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let last = d.stats().draws;
    assert_eq!(placed.load(Ordering::Relaxed), 1, "in place of the last");
    assert!(!d.swallowing());
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let after = d.stats().draws;
    (d, [before, level, last, after], placed)
}

fn drops_exactly_the_plan<B: Backend>(b: B) -> Device<B> {
    let (d, draws, placed) = as_planned(b, true);
    assert_eq!(draws, [1, 1, 1, 2], "the three planned dropped; the one after drawn");
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches, s.swallow_refusals), (3, 0, 0));
    assert_eq!(placed.load(Ordering::Relaxed), 1, "placed once");
    assert_eq!((s.external_passes, s.external_passes_dropped), (1, 0), "issued, with no depth bound");
    d
}

#[test]
fn the_swallow_drops_exactly_the_plans_draws_and_nothing_after() {
    let d = drops_exactly_the_plan(takes());
    let taken = &d.backend.external;
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].0, taken[0].1, "colour only: the colour as its own depth");
}

#[test]
fn the_swallow_holds_through_the_render_thread() {
    drops_exactly_the_plan(ThreadedBackend::new(takes()).unwrap());
}

#[test]
fn observing_draws_everything_and_places_the_pass_after_the_last() {
    let (d, draws, placed) = as_planned(takes(), false);
    assert_eq!(draws, [1, 3, 4, 5]);
    assert_eq!((d.stats().swallowed_draws, d.stats().swallow_mismatches), (3, 0));
    assert_eq!(placed.load(Ordering::Relaxed), 1);
    assert_eq!(d.stats().external_passes, 1, "the capture is issued too");
}

/// The pass's three quads, as the game draws them; the draws handed to
/// the backend after them.
fn the_pass<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, back: u32) -> u64 {
    d.set_render_target(m, LEVEL_A, 0).unwrap();
    quad(m);
    quad(m);
    d.set_render_target(m, back, 0).unwrap();
    quad(m);
    assert_eq!(d.drain_until_callback(m).unwrap(), None);
    d.stats().draws
}

/// Headless (no GPU backend) the glow cannot be drawn: dropping is
/// refused before any draw, every quad is drawn, and why is counted.
/// Observing still arms (it drops nothing) and its pass is dropped for
/// the same reason.
#[test]
fn a_pass_that_cannot_be_issued_drops_nothing() {
    let (mut d, mut m, back) = setup(RecordingBackend::new());
    let placed = Arc::new(AtomicU32::new(0));
    assert!(!d.swallow(&mut m, plan(true, &placed)).unwrap(), "not armed");
    assert!(!d.swallowing());
    assert_eq!(the_pass(&mut d, &mut m, back), 3, "every quad drawn");
    let s = d.stats();
    assert_eq!((s.swallow_refusals, s.swallowed_draws), (1, 0));
    assert_eq!(s.external_refusals[ExternalRefusal::Unsupported.index()], 1);
    assert_eq!(placed.load(Ordering::Relaxed), 0);

    assert!(d.swallow(&mut m, plan(false, &placed)).unwrap(), "observing arms");
    assert_eq!(the_pass(&mut d, &mut m, back), 6);
    let s = d.stats();
    assert_eq!(s.swallowed_draws, 3);
    assert_eq!(s.external_refusals[ExternalRefusal::Unsupported.index()], 2, "the capture, dropped");
    assert_eq!(s.external_passes, 0);
}

/// A pass that needs depth is not swallowed for: the last quad's depth is
/// not known when arming (the composite binds none).
#[test]
fn dropping_needs_a_colour_only_pass() {
    let (mut d, mut m, back) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    let s = Swallow {
        pass: Box::new(Placed(Arc::clone(&placed), false)),
        ..plan(true, &placed)
    };
    assert!(!d.swallow(&mut m, s).unwrap());
    assert_eq!(the_pass(&mut d, &mut m, back), 3);
    assert_eq!(d.stats().external_refusals[ExternalRefusal::NoDepth.index()], 1);
    assert!(d.backend.external.is_empty());
}

/// Every reason has its own count and index.
#[test]
fn each_refusal_is_counted_apart() {
    for (i, r) in ExternalRefusal::ALL.iter().enumerate() {
        assert_eq!(r.index(), i);
        assert!(!r.describe().is_empty());
    }
}

/// The game draws one quad into the level where the plan has two: its
/// second planned draw finds the back buffer, the wrong slot.
#[test]
fn a_draw_the_plan_does_not_have_ends_the_swallow() {
    let (mut d, mut m, back) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    assert!(d.swallow(&mut m, plan(true, &placed)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m);
    d.set_render_target(&mut m, back, 0).unwrap();
    quad(&mut m);
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (1, 1));
    assert_eq!(s.draws, 2, "the mismatched draw and the one after are drawn");
    assert_eq!(placed.load(Ordering::Relaxed), 0, "the pass is not placed");
    assert!(!d.swallowing());
}

/// Another draw between planned ones (not inline), or a planned one of
/// another shape, ends it too.
#[test]
fn another_draw_or_shape_ends_the_swallow() {
    let (mut d, mut m, _) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    let single = |placed: &Arc<AtomicU32>| Swallow {
        targets: vec![0, 0],
        primitive: PrimitiveType::QuadList,
        vertices: 4,
        shapes: Vec::new(),
        drop: true,
        pass: Box::new(Placed(Arc::clone(placed), true)),
    };
    assert!(d.swallow(&mut m, single(&placed)).unwrap());
    quad(&mut m);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (1, 1), "the draw between ends it");
    assert_eq!(placed.load(Ordering::Relaxed), 0);

    let wrong = Swallow {
        primitive: PrimitiveType::TriangleFan,
        ..single(&placed)
    };
    assert!(d.swallow(&mut m, wrong).unwrap());
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stats().swallow_mismatches, 2, "a quad list where a fan is planned");
    assert_eq!(d.stats().swallowed_draws, 1);
}

/// A draw's own shape stands for it; the draws past `shapes` take the
/// swallow's.
#[test]
fn each_draw_can_have_its_own_shape() {
    let (mut d, mut m, _) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    let mixed = |shapes: Vec<(PrimitiveType, usize)>| Swallow {
        targets: vec![0, 0],
        primitive: PrimitiveType::TriangleFan,
        vertices: 4,
        shapes,
        drop: true,
        pass: Box::new(Placed(Arc::clone(&placed), true)),
    };
    let quads = vec![(PrimitiveType::QuadList, 4); 2];
    assert!(d.swallow(&mut m, mixed(quads)).unwrap());
    quad(&mut m);
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (2, 0), "both quads are their own shape");
    assert!(d.swallow(&mut m, mixed(vec![(PrimitiveType::QuadList, 4)])).unwrap());
    quad(&mut m);
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (3, 1), "the second takes the fan");
}

/// Arming over a swallow its pass never finished counts it as a mismatch.
#[test]
fn a_swallow_left_unfinished_is_a_mismatch() {
    let (mut d, mut m, _) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    assert!(d.swallow(&mut m, plan(true, &placed)).unwrap());
    assert!(d.swallow(&mut m, plan(true, &placed)).unwrap());
    assert_eq!(d.stats().swallow_mismatches, 1);
    assert_eq!(placed.load(Ordering::Relaxed), 0);
}

/// A pass that binds its own target plans no slot 0: its first draw's
/// target is its slot's whatever was bound when armed, the same one or
/// another (a screen effect binds target 0 itself, `0x00016610`).
#[test]
fn a_pass_without_slot_0_takes_the_target_its_first_draw_finds() {
    let (mut d, mut m, back) = setup(takes());
    let placed = Arc::new(AtomicU32::new(0));
    let own = |placed: &Arc<AtomicU32>| Swallow {
        targets: vec![1, 1],
        primitive: PrimitiveType::QuadList,
        vertices: 4,
        shapes: Vec::new(),
        drop: true,
        pass: Box::new(Placed(Arc::clone(placed), true)),
    };
    assert!(d.swallow(&mut m, own(&placed)).unwrap());
    quad(&mut m);
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (2, 0), "the target bound when armed");
    assert_eq!(placed.load(Ordering::Relaxed), 1);
    assert!(d.swallow(&mut m, own(&placed)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m);
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    let s = d.stats();
    assert_eq!((s.swallowed_draws, s.swallow_mismatches), (4, 0), "another, bound after arming");
    d.set_render_target(&mut m, back, 0).unwrap();
    // With slot 0 planned, a draw into another target ends it, and says
    // how.
    let pinned = Swallow { targets: vec![0], ..own(&placed) };
    assert!(d.swallow(&mut m, pinned).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stats().swallow_mismatches, 1);
    let why = d.swallow_mismatch().unwrap();
    assert!(why.starts_with("draw 0 of 1: planned QuadList x4 into slot 0") && why.ends_with("into another target"), "{why}");
    assert!(d.swallow(&mut m, own(&placed)).unwrap());
    assert!(d.swallow(&mut m, own(&placed)).unwrap());
    assert_eq!(d.swallow_mismatch(), Some("armed over it at draw 0 of 2"));
}
