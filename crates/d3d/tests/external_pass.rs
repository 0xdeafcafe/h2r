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

/// Records its name when given the view constants (at issue).
struct Named(&'static str, Arc<Mutex<Vec<&'static str>>>);

impl ExternalPass for Named {
    fn encode(&mut self, _t: ExternalTarget<'_>) {}
    fn view_constants(&mut self, _c: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.1.lock().unwrap().push(self.0);
    }
}

/// The world's pass and the objects' are asked at the same layer-3 call,
/// before one drain: both are issued, in the order asked (a newer ask
/// once replaced the older, and the world's skipped clusters went
/// undrawn).
#[test]
fn passes_asked_before_one_drain_are_all_issued_in_order() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let order = Arc::default();
    d.request_external_pass(Box::new(Named("world", Arc::clone(&order))));
    d.request_external_pass(Box::new(Named("objects", Arc::clone(&order))));
    assert!(order.lock().unwrap().is_empty(), "not before the drain");
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["world", "objects"]);
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(order.lock().unwrap().len(), 2, "issued once");
}

#[test]
fn an_immediate_reference_marker_follows_pending_passes_and_is_never_reissued() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let order = Arc::default();
    d.request_external_pass(Box::new(Named("pending", Arc::clone(&order))));
    // RecordingBackend declines the GPU operation, but each pass sees its
    // constants immediately, after the commands preceding this boundary.
    assert!(!d.external_pass_now(&mut m, Box::new(Named("marker", Arc::clone(&order)))).unwrap());
    assert_eq!(*order.lock().unwrap(), vec!["pending", "marker"]);
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(order.lock().unwrap().len(), 2);
}

#[test]
fn a_confirmed_threaded_pass_reports_the_actual_target_refusal_to_the_guest() {
    struct Confirmed;
    impl ExternalPass for Confirmed {
        fn confirm_issue(&self) -> bool { true }
        fn encode(&mut self, _: ExternalTarget<'_>) {}
    }

    let mut backend = RecordingBackend::default();
    backend.external_passes = true;
    let (mut d, mut m) = create(ThreadedBackend::new(backend).unwrap(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    // The worker advertised external passes but declines this request.
    d.backend.with_inner(|b| b.external_passes = false).unwrap();
    assert!(d.can_external_pass());
    assert!(!d.external_pass_now(&mut m, Box::new(Confirmed)).unwrap());
    assert_eq!(d.stats().external_refusals[d3d::ExternalRefusal::Backend.index()], 1);
    d.backend.with_inner(|b| b.external_passes = true).unwrap();
    assert!(d.external_pass_now(&mut m, Box::new(Confirmed)).unwrap());
    assert_eq!(d.stats().external_passes, 1);
}

/// Keeps the depth clip it is given.
struct Clip(Arc<Mutex<Option<d3d::DepthClip>>>);

impl ExternalPass for Clip {
    fn encode(&mut self, _t: ExternalTarget<'_>) {}
    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        *self.0.lock().unwrap() = Some(clip);
    }
}

/// The pass gets the W-buffer clip a W-buffered draw issued then would
/// take: nothing pushed, the whole range; `D3DRS_DEPTHCLIPCONTROL` with
/// `D3DDCC_DEPTHCLAMP` (bits 7:4 = 1) in the guest's array, clamped.
#[test]
fn the_pass_gets_the_depth_clip_at_issue() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let issue = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory| {
        let got = Arc::default();
        d.request_external_pass(Box::new(Clip(Arc::clone(&got))));
        d.drain_until_callback(m).unwrap();
        let clip: Option<d3d::DepthClip> = *got.lock().unwrap();
        clip.expect("given at issue, with or without a backend that draws it")
    };
    let first = issue(&mut d, &mut m);
    let full = d3d::DepthClip { range: [0.0, 16_777_215.0], clamp: false, w_buffered: first.w_buffered };
    assert_eq!(first, full, "nothing pushed: [0, z_max] of D24S8");
    assert_eq!(first.w_buffered, d.hw().control0.map(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0));
    let slot = gfx::xdk::RenderStateTable::default()
        .canonical_to_guest(d3d::nv2a::RS_DEPTHCLIPCONTROL)
        .expect("a guest slot");
    m.write_u32(d3d::GuestLayout::HALO2.render_state_slot(slot), 0x10).unwrap();
    assert_eq!(issue(&mut d, &mut m), d3d::DepthClip { clamp: true, ..full });
    assert!(!d3d::nv2a::clamps_depth(0x01) && d3d::nv2a::clamps_depth(0x11));
}

#[test]
fn a_backend_that_cannot_drops_it() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    assert!(!d.can_external_pass());
    assert_eq!(d.external_ready(&m, false), Err(d3d::ExternalRefusal::Unsupported));
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
    assert_eq!(d.external_ready(&m, false), Err(d3d::ExternalRefusal::NoDepth));
    assert!(d.external_ready(&m, true).is_ok());
    assert_eq!(d.stats().external_passes_dropped, 0, "preflight only reads");
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

fn plan(ranges: usize) -> d3d::ExternalPlan {
    d3d::ExternalPlan { stage: timeline::native::Stage::World, view: 1, ranges, slots: vec![] }
}

struct Counting {
    seen: Arc<Mutex<Vec<d3d::TargetCopy>>>,
    display_ranges: usize,
}

impl ExternalPass for Counting {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    fn encode(&mut self, t: ExternalTarget<'_>) {
        Probe(Arc::default()).encode(t);
    }

    fn record_ranges(&mut self, t: ExternalTarget<'_>) -> usize {
        let copy = t.copy;
        self.seen.lock().unwrap().push(copy);
        self.encode(t);
        if copy == d3d::TargetCopy::Display { self.display_ranges } else { 1 }
    }
}

#[test]
fn refusal_of_the_display_discards_even_the_recorded_reference_clear() {
    let Some(reference) = backend() else { return };
    let ctx = &reference.ctx;
    let display = GfxBackend::new(GfxContext {
        instance: ctx.instance.clone(), adapter: ctx.adapter.clone(), device: ctx.device.clone(),
        queue: ctx.queue.clone(), caps: ctx.caps,
    });
    let (mut d, mut m) = create(d3d::ScaledBackend::new(reference, display, 2).unwrap(), UploadPolicy::Always);
    let back = d.back_buffer().unwrap();
    d.clear(&mut m, 0, 0, 0xf0, 0xff00ff00, 1., 0).unwrap();
    let seen = Arc::default();
    let pass = Counting { seen: Arc::clone(&seen), display_ranges: 0 };
    assert!(d.prepare_external_pass(&mut m, plan(1), Box::new(pass)).unwrap().is_none());
    assert_eq!(*seen.lock().unwrap(), [d3d::TargetCopy::Reference, d3d::TargetCopy::Display]);
    let reference = d.backend.reference_mut().read_color(back.reference).unwrap().0;
    let display = d.backend.view_mut().read_color(back.view.unwrap()).unwrap().0;
    for pixels in [reference, display] { assert_eq!(&pixels[..4], &[0, 255, 0, 255]); }
    assert_eq!(d.stats().external_passes, 0);
}

#[test]
fn cancel_keeps_pixels_and_commit_preserves_the_deferred_display() {
    let Some(b) = backend() else { return };
    let (mut d, mut m) = create_copy_swap(b, UploadPolicy::Always);
    let back = d.back_buffer().unwrap();
    d.clear(&mut m, 0, 0, 0xf0, 0xff00ff00, 1., 0).unwrap();
    let pass = || Box::new(Counting { seen: Arc::default(), display_ranges: 1 });
    drop(d.prepare_external_pass(&mut m, plan(1), pass()).unwrap().unwrap());
    assert_eq!(&d.backend.read_color(back).unwrap().0[..4], &[0, 255, 0, 255]);
    d.swap(&mut m, 0).unwrap();
    let front = d.front_buffer().unwrap();
    d.prepare_external_pass(&mut m, plan(1), pass()).unwrap().unwrap().commit();
    assert_eq!(&d.backend.read_color(front).unwrap().0[..4], &[0, 255, 0, 255]);
    assert_eq!(d.backend.read_color(back).unwrap().0[1], 0, "back was replaced");
    assert_eq!(d.stats().external_passes, 1);
}

#[test]
fn threaded_prepare_waits_for_actual_recording_and_refuses_partial_ranges() {
    let Some(b) = backend() else { return };
    let (mut d, mut m) = create(ThreadedBackend::new(b).unwrap(), UploadPolicy::Always);
    let seen = Arc::default();
    let pass = Counting { seen: Arc::clone(&seen), display_ranges: 2 };
    assert!(d.prepare_external_pass(&mut m, plan(2), Box::new(pass)).unwrap().is_none());
    assert_eq!(*seen.lock().unwrap(), [d3d::TargetCopy::Reference]);
}
