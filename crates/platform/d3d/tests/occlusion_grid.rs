//! `Device::request_depth_grid` (docs/graphics/occlusion-culling.md): the
//! occlusion grid of the device's depth buffer as the draws before the ask
//! left it, recorded after the next drain and handed to the sink with the
//! W-buffer's parameters, on the device's thread, through the render
//! thread, and through the interpolating backend (which passes it on).
//! Not recorded when the depth is not W-buffered or another depth is
//! bound. Synthetic device; skipped without an adapter or without
//! `DEPTH32FLOAT_STENCIL8`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use d3d::layout::device_field as field;
use d3d::{
    Backend, DepthGridResult, DepthScale, Device, GfxBackend, GuestLayout, GuestMemory,
    InterpBackend, SparseMemory, SwapFinish, ThreadedBackend, UploadPolicy,
};
use gfx::GfxContext;

const L: GuestLayout = GuestLayout::HALO2;
const RECTS: u32 = MISC + 0x7100;

type Got = Arc<Mutex<Vec<(DepthGridResult, DepthScale)>>>;

fn backend() -> Option<GfxBackend> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8");
        return None;
    }
    let mut b = GfxBackend::new(ctx);
    b.set_swap_finish(SwapFinish::Full);
    Some(b)
}

/// Game code's inline `SET_CONTROL0` (W-buffered with `w`).
fn control0(m: &mut SparseMemory, w: bool) {
    let dev = L.device;
    let mut put = m.read_u32(dev).unwrap();
    for word in [
        (1 << 18) | 0x0290,
        0x0010_0001 | if w { 0x1_0000 } else { 0 },
    ] {
        m.write_u32(put, word).unwrap();
        put += 4;
    }
    m.write_u32(dev, put).unwrap();
}

/// The W parameters `SetTransform` leaves: w far 500.
fn w_params(m: &mut SparseMemory) {
    let dev = m.read_u32(L.p_device).unwrap();
    m.write_u32(dev + field::INV_W_FAR, (1.0f32 / 500.0).to_bits())
        .unwrap();
}

/// The left half of the 64x64 target at depth 0.25, the rest at 1.
fn fill<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    d.clear(m, 0, 0, 0x03, 0, 1.0, 0).unwrap();
    m.write(RECTS, &[0u32, 0, 32, 64].map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(m, 1, RECTS, 0x03, 0, 0.25, 0).unwrap();
}

fn ask<B: Backend>(d: &mut Device<B>, tag: u64, got: &Got) {
    let got = Arc::clone(got);
    d.request_depth_grid(
        tag,
        8,
        4,
        Arc::new(move |r, s| got.lock().unwrap().push((r, s))),
    );
}

/// Presents until the grid lands (the render thread delivers on its own
/// time).
fn wait<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, got: &Got, n: usize) {
    for _ in 0..200 {
        if got.lock().unwrap().len() >= n {
            return;
        }
        d.swap(m, 0).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no grid after 200 presents");
}

fn run<B: Backend>(b: B) {
    let (mut d, mut m) = create(b, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    w_params(&mut m);
    let got: Got = Arc::default();

    // Not W-buffered: nothing recorded.
    control0(&mut m, false);
    fill(&mut d, &mut m);
    ask(&mut d, 1, &got);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stats().depth_grids_skipped, 1);

    // W-buffered: the grid of the depth the clears left.
    control0(&mut m, true);
    fill(&mut d, &mut m);
    ask(&mut d, 2, &got);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stats().depth_grids, 1);
    // Drawing after the ask does not reach the grid.
    d.clear(&mut m, 0, 0, 0x03, 0, 0.75, 0).unwrap();
    wait(&mut d, &mut m, &got, 1);
    let (r, s) = got.lock().unwrap()[0].clone();
    assert_eq!((r.tag, r.width, r.height), (2, 8, 4));
    for y in 0..4 {
        for x in 0..8 {
            let v = r.depth[y * 8 + x];
            let want = if x < 4 { 0.25 } else { 1.0 };
            assert!((v - want).abs() < 1e-6, "cell ({x}, {y}) {v}, want {want}");
        }
    }
    assert_eq!(s.inv_w_far, 1.0 / 500.0);
    assert_eq!(s.z_scale, 16_777_215.0, "D24S8 full scale");
    let events = d.take_events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn the_grid_holds_the_depth_drawn_before_the_ask() {
    let Some(b) = backend() else { return };
    run(b);
}

#[test]
fn the_grid_comes_through_the_render_thread() {
    let Some(b) = backend() else { return };
    run(ThreadedBackend::new(b).unwrap());
}

#[test]
fn the_interpolating_backend_passes_the_grid_on() {
    let Some(b) = backend() else { return };
    run(ThreadedBackend::new(InterpBackend::new(b, || 0)).unwrap());
}
