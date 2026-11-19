//! `Device::request_external_pass` (docs/graphics/native-renderer.md): a
//! host pass drawn into the bound colour target and depth with the
//! current viewport, recorded after the next drain, on the device's
//! thread, through the render thread, through the interpolating backend
//! (which passes it on and holds the frame) and under the render scale
//! (both copies). Dropped without a backend that can. Synthetic device;
//! skipped without an adapter or without `DEPTH32FLOAT_STENCIL8`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use d3d::{
    Backend, Device, ExternalPass, ExternalTarget, GfxBackend, GuestMemory, InterpBackend, RecordingBackend,
    SparseMemory, SwapFinish, ThreadedBackend, UploadPolicy,
};
use gfx::GfxContext;

#[derive(Clone, Debug, PartialEq)]
struct Seen {
    viewport: [f32; 4],
    z_scale: f32,
    color: wgpu::TextureFormat,
    depth_is_depth: bool,
    sizes: ((u32, u32), (u32, u32)),
}

type Got = Arc<Mutex<Vec<Seen>>>;

/// Records what it was given and clears the colour target red.
struct Probe(Got);

impl ExternalPass for Probe {
    fn encode(&mut self, t: ExternalTarget<'_>) {
        self.0.lock().unwrap().push(Seen {
            viewport: t.viewport,
            z_scale: t.z_scale,
            color: t.color_format,
            depth_is_depth: t.depth_format.is_depth_stencil_format(),
            sizes: (t.guest_size, t.size),
        });
        drop(t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("probe"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: t.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
    }
}

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

fn wait<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, got: &Got) {
    for _ in 0..200 {
        if !got.lock().unwrap().is_empty() {
            return;
        }
        d.swap(m, 0).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no pass after 200 presents");
}

fn run<B: Backend>(b: B) {
    let (mut d, mut m) = create(b, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    assert!(d.can_external_pass());
    let got: Got = Arc::default();
    d.request_external_pass(Box::new(Probe(Arc::clone(&got))));
    assert!(got.lock().unwrap().is_empty(), "not before the drain");
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert_eq!(d.stats().external_passes, 1);
    wait(&mut d, &mut m, &got);
    let seen = got.lock().unwrap()[0].clone();
    assert_eq!(seen.viewport, [0.0, 0.0, 64.0, 64.0]);
    assert_eq!(seen.z_scale, 16_777_215.0, "D24S8 full scale");
    assert!(seen.depth_is_depth);
    assert_eq!(seen.sizes, ((64, 64), (64, 64)), "guest and texel sizes at 1x");
    assert!(!seen.color.is_depth_stencil_format());
    let events = d.take_events();
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn the_pass_draws_into_the_bound_targets_after_the_drain() {
    let Some(b) = backend() else { return };
    run(b);
}

#[test]
fn the_pass_comes_through_the_render_thread() {
    let Some(b) = backend() else { return };
    run(ThreadedBackend::new(b).unwrap());
}

#[test]
fn the_interpolating_backend_passes_it_on() {
    let Some(b) = backend() else { return };
    run(ThreadedBackend::new(InterpBackend::new(b, || 0)).unwrap());
}

/// Keeps the view constants it is given.
struct Constants(Arc<Mutex<Option<Vec<[f32; 4]>>>>);

impl ExternalPass for Constants {
    fn encode(&mut self, _t: ExternalTarget<'_>) {}
    fn view_constants(&mut self, c: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        *self.0.lock().unwrap() = Some(c.to_vec());
    }
}

#[test]
fn the_pass_gets_the_view_constants_after_the_drain() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let src = MISC + 0x3000;
    for (i, v) in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0].into_iter().enumerate() {
        m.write_u32(src + 4 * i as u32, v.to_bits()).unwrap();
    }
    // c[0] and c[1] (hardware numbering), queued before the request.
    d.set_vertex_shader_constant(&mut m, 0, src, 2, true).unwrap();
    let got = Arc::default();
    d.request_external_pass(Box::new(Constants(Arc::clone(&got))));
    d.drain_until_callback(&mut m).unwrap();
    let c = got.lock().unwrap().clone().expect("given at issue, with or without a backend that draws it");
    assert_eq!(c.len(), d3d::VIEW_CONSTANTS);
    assert_eq!((c[0], c[1]), ([1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]));
}

#[test]
fn a_backend_that_cannot_drops_it() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    assert!(!d.can_external_pass());
    let got: Got = Arc::default();
    d.request_external_pass(Box::new(Probe(Arc::clone(&got))));
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!((d.stats().external_passes, d.stats().external_passes_dropped), (0, 1));
    assert_eq!(d.stats().external_refusals[d3d::ExternalRefusal::Unsupported.index()], 1, "counted by reason");
    assert!(got.lock().unwrap().is_empty());
}

/// A [`Probe`] that draws colour only.
struct ColourOnly(Probe);

impl ExternalPass for ColourOnly {
    fn encode(&mut self, t: ExternalTarget<'_>) {
        self.0.encode(t);
    }
    fn colour_only(&self) -> bool {
        true
    }
}

/// A colour-only pass (the glow, the A/B's capture) is issued with no
/// depth bound and gets a scratch depth of the colour's size; a pass that
/// needs depth is dropped there, for that reason.
fn colour_only<B: Backend>(b: B) {
    let (mut d, mut m) = create(b, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    let needs: Got = Arc::default();
    d.request_external_pass(Box::new(Probe(Arc::clone(&needs))));
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().external_refusals[d3d::ExternalRefusal::NoDepth.index()], 1);
    let got: Got = Arc::default();
    d.request_external_pass(Box::new(ColourOnly(Probe(Arc::clone(&got)))));
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().external_passes, 1);
    wait(&mut d, &mut m, &got);
    let seen = got.lock().unwrap()[0].clone();
    assert!(seen.depth_is_depth, "a scratch depth");
    assert_eq!(seen.sizes, ((64, 64), (64, 64)));
    assert!(needs.lock().unwrap().is_empty());
}

#[test]
fn a_colour_only_pass_needs_no_depth() {
    let Some(b) = backend() else { return };
    colour_only(b);
}

#[test]
fn a_colour_only_pass_comes_through_the_render_thread() {
    let Some(b) = backend() else { return };
    colour_only(ThreadedBackend::new(b).unwrap());
}
