//! Synthetic post calls over the real D3D device and push buffer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::common::*;
use d3d::{
    Backend, Device, ExternalPass, ExternalRefusal, ExternalTarget, GfxBackend, GuestMemory,
    Recorded, RecordingBackend, SparseMemory, Swallow, SwallowCall, SwallowPass, SwallowRefusal,
    UploadPolicy,
};
use gfx::vertex::PrimitiveType;

const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const LEVEL_BYTES: u32 = 32 * 32 * 4;
const CALL: SwallowCall = SwallowCall {
    entry: 0x0002_14f0,
    esp: STACK_TOP - 0x100,
    ret: 0x0001_39ba,
};
const END_SITE: u32 = 0x0002_d8b2;

fn setup<B: Backend>(backend: B) -> (Device<B>, SparseMemory, u32) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    bind_quad(&mut d, &mut m);
    m.write_u32(CALL.esp, CALL.ret).unwrap();

    m.map(0x8000_0000 | DATA_A, LEVEL_BYTES);
    m.write(0x8000_0000 | DATA_A, &vec![0x30; LEVEL_BYTES as usize]).unwrap();
    let format = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = 31 | (31 << 12) | (1 << 24);
    let texture = [1 | (4 << 16) | 0x0100_0000, DATA_A, 0, format, size];
    let level = [1 | (5 << 16) | 0x0100_0000, DATA_A, 0, format, size, TEX_A];
    for (header, words) in [(TEX_A, &texture[..]), (LEVEL_A, &level[..])] {
        for (i, word) in words.iter().enumerate() {
            m.write_u32(header + 4 * i as u32, *word).unwrap();
        }
    }
    let back = d.bound_target()[0];
    (d, m, back)
}

fn recorder() -> RecordingBackend {
    let mut b = RecordingBackend::new();
    b.external_passes = true;
    b.sample_surfaces = true;
    b
}

fn finish<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    d.finish_swallow(m, CALL.entry, CALL.esp).unwrap();
    assert!(!d.swallowing());
}

fn quad(m: &mut SparseMemory, i: u32) {
    inline_quad(m, i as f32 * 12.0, 0.0, i as f32 * 12.0 + 8.0, 8.0, [50 + i as u8, 180, 72, 255]);
}

/// Two different quads into the intermediate, then one into the back buffer.
fn quads<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, back: u32) {
    d.set_render_target(m, LEVEL_A, 0).unwrap();
    quad(m, 0);
    quad(m, 1);
    d.set_render_target(m, back, 0).unwrap();
    quad(m, 2);
    assert_eq!(d.drain_until_callback(m).unwrap(), None);
}

fn draws(d: &Device<RecordingBackend>) -> Vec<Recorded> {
    d.backend.calls.iter().filter(|c| matches!(c, Recorded::Draw { .. })).cloned().collect()
}

struct Capture;

impl ExternalPass for Capture {
    fn encode(&mut self, _: ExternalTarget<'_>) {}
    fn colour_only(&self) -> bool { true }
}

impl SwallowPass for Capture {}

fn plan(drop: bool, pass: impl SwallowPass + 'static) -> Swallow {
    Swallow {
        call: Some(CALL),
        end_returns: Vec::new(),
        headers: vec![None, Some(LEVEL_A)],
        targets: vec![1, 1, 0],
        primitive: PrimitiveType::QuadList,
        vertices: 4,
        shapes: Vec::new(),
        drop,
        pass: Box::new(pass),
    }
}

#[test]
fn a_mismatch_at_each_position_replays_the_original_packets_in_order() {
    let (mut baseline, mut m, back) = setup(recorder());
    quads(&mut baseline, &mut m, back);
    let expected = draws(&baseline);
    for mismatch in 0..expected.len() {
        let (mut d, mut m, back) = setup(recorder());
        let mut s = plan(true, Capture);
        s.shapes = vec![(PrimitiveType::QuadList, 4); expected.len()];
        s.shapes[mismatch] = (PrimitiveType::TriangleFan, 4);
        assert!(d.swallow(&mut m, s).unwrap());
        quads(&mut d, &mut m, back);
        finish(&mut d, &mut m);
        assert_eq!(draws(&d), expected, "mismatch at draw {mismatch}");
        assert_eq!(d.stats().swallow_draws_replayed, mismatch as u64);
        assert_eq!(d.stats().swallow_mismatches, 1);
        assert_eq!(d.stats().swallow_draws_dropped, 0);
        assert_eq!(d.stats().swallow_commits, 0);
    }
}

#[test]
fn a_depth_scale_change_replays_the_held_post_draws() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    let scale = d3d::GuestLayout::HALO2.device + d3d::layout::device_field::Z_SCALE;
    m.write_u32(scale, 64.0f32.to_bits()).unwrap();
    quad(&mut m, 0);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    m.write_u32(scale, 32.0f32.to_bits()).unwrap();
    quad(&mut m, 1);
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    assert!(!d.swallowing());
    let stats = d.stats();
    assert_eq!(stats.swallow_reasons[SwallowRefusal::Resource.index()], 1);
    assert_eq!(stats.swallow_draws_replayed, 1);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
}

#[test]
fn a_late_target_mismatch_replays_the_intermediate_before_drawing_the_mismatch() {
    let run = |armed| {
        let (mut d, mut m, back) = setup(recorder());
        if armed {
            assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
        }
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        quad(&mut m, 0);
        d.set_render_target(&mut m, back, 0).unwrap();
        quad(&mut m, 1);
        quad(&mut m, 2);
        assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
        finish(&mut d, &mut m);
        d
    };
    let candidate = run(true);
    assert_eq!(draws(&candidate), draws(&run(false)));
    assert_eq!(candidate.stats().swallow_draws_replayed, 1);
    assert_eq!(candidate.stats().swallow_draws_dropped, 0);
}

fn strip<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, x: f32) {
    d.begin(m, TRIANGLESTRIP).unwrap();
    d.set_vertex_data(m, 3, [1.0, 0.5, 0.2, 1.0]).unwrap();
    for (x, y) in [(x, 0.0), (x + 8.0, 0.0), (x, 8.0), (x + 8.0, 8.0)] {
        d.set_vertex_data(m, -1, [x, y, 0.5, 1.0]).unwrap();
    }
    d.end(m).unwrap();
}

fn strips_plan(drop: bool) -> Swallow {
    Swallow {
        end_returns: vec![Some(END_SITE); 2],
        headers: Vec::new(),
        primitive: PrimitiveType::TriangleStrip,
        targets: vec![0, 0],
        ..plan(drop, Capture)
    }
}

#[test]
fn a_same_shape_end_from_another_caller_cannot_finish_the_plan() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, strips_plan(true)).unwrap());
    d.swallow_caller(&m, CALL.esp - 4, END_SITE, |_| true).unwrap();
    strip(&mut d, &mut m, 0.0);
    assert!(draws(&d).is_empty(), "the expected End is held");

    d.swallow_caller(&m, CALL.esp - 4, END_SITE + 8, |_| true).unwrap();
    strip(&mut d, &mut m, 12.0);
    finish(&mut d, &mut m);
    assert_eq!(d.stats().swallow_draws_replayed, 1);
    assert_eq!(d.stats().swallow_commits, 0);

    let (mut baseline, mut m, _) = setup(recorder());
    strip(&mut baseline, &mut m, 0.0);
    strip(&mut baseline, &mut m, 12.0);
    assert_eq!(draws(&d), draws(&baseline));
}

#[test]
fn a_short_call_replays_before_its_caller_draws_the_matching_suffix() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, strips_plan(true)).unwrap());
    d.swallow_caller(&m, CALL.esp - 4, END_SITE, |_| true).unwrap();
    strip(&mut d, &mut m, 0.0);
    finish(&mut d, &mut m);
    assert_eq!(d.stats().swallow_draws_replayed, 1);
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::CallBoundary.index()], 1);
    strip(&mut d, &mut m, 12.0);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().swallowed_draws, 1);
}

#[test]
fn a_caller_outside_the_stack_frame_cannot_consume_queued_work() {
    let (mut d, mut m, back) = setup(recorder());
    assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m, 0);
    d.drain_until_callback(&mut m).unwrap();
    assert!(draws(&d).is_empty());

    quad(&mut m, 1);
    d.swallow_caller(&m, CALL.esp, CALL.ret + 4, |_| true).unwrap();
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().swallow_draws_replayed, 1);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::CallBoundary.index()], 1);
    d.set_render_target(&mut m, back, 0).unwrap();
}

#[test]
fn a_new_post_call_without_a_plan_still_interrupts_the_old_one() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, strips_plan(true)).unwrap());
    d.swallow_caller(&m, CALL.esp - 4, END_SITE, |_| true).unwrap();
    strip(&mut d, &mut m, 0.0);
    d.interrupt_swallow().unwrap();
    strip(&mut d, &mut m, 12.0);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().swallow_draws_replayed, 1);
    assert_eq!(d.stats().swallow_commits, 0);
}

#[test]
fn a_backend_that_refuses_preparation_replays_the_complete_packet_set() {
    let (mut baseline, mut m, back) = setup(recorder());
    quads(&mut baseline, &mut m, back);

    let (mut d, mut m, back) = setup(recorder());
    assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
    quads(&mut d, &mut m, back);
    assert!(draws(&d).is_empty());
    finish(&mut d, &mut m);
    assert_eq!(draws(&d), draws(&baseline));
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::Encode.index()], 1);
    assert_eq!(d.stats().swallow_draws_replayed, 3);
    assert_eq!(d.stats().swallow_commits, 0);
}

#[test]
fn only_a_sampled_stage_can_force_a_post_alias_writeback() {
    use shaders::psh::{PixelShaderDef, bits::{TEX_PROJECT2D, texture_modes}};

    for sampled in [false, true] {
        let (mut d, mut m, back) = setup(recorder());
        let shader = MISC + 0x6000;
        let definition = shader + 0x100;
        m.write(definition, &PixelShaderDef {
            texture_modes: texture_modes([0, 0, 0, if sampled { TEX_PROJECT2D } else { 0 }]),
            ..Default::default()
        }.to_bytes()).unwrap();
        for (i, value) in [1, 0, definition].into_iter().enumerate() {
            m.write_u32(shader + 4 * i as u32, value).unwrap();
        }
        assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        quad(&mut m, 0);
        d.set_pixel_shader(&mut m, shader).unwrap();
        // Stage 3 still names the very level this quad is drawing into.
        d.set_texture(&mut m, 3, TEX_A).unwrap();
        quad(&mut m, 1);
        d.set_texture(&mut m, 3, 0).unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        quad(&mut m, 2);
        d.drain_until_callback(&mut m).unwrap();
        finish(&mut d, &mut m);
        let stats = d.stats();
        assert_eq!(stats.swallow_reasons[SwallowRefusal::Resource.index()], u64::from(sampled));
        assert_eq!(stats.swallow_reasons[SwallowRefusal::Encode.index()], u64::from(!sampled));
        assert_eq!(stats.swallow_draws_replayed, if sampled { 1 } else { 3 });
        assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    }
}

#[test]
fn a_clean_overlapping_level_does_not_block_a_direct_post_sample() {
    use shaders::psh::{PixelShaderDef, bits::{TEX_PROJECT2D, texture_modes}};

    const CLEAN_ALIAS: u32 = LEVEL_A - 0x100;
    let (mut d, mut m, back) = setup(recorder());
    for offset in (0..28).step_by(4) {
        let word = m.read_u32(LEVEL_A + offset).unwrap();
        m.write_u32(CLEAN_ALIAS + offset, word).unwrap();
    }
    d.set_render_target(&mut m, CLEAN_ALIAS, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();

    let shader = MISC + 0x6000;
    let definition = shader + 0x100;
    m.write(definition, &PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT2D, 0, 0, 0]),
        ..Default::default()
    }.to_bytes()).unwrap();
    for (i, value) in [1, 0, definition].into_iter().enumerate() {
        m.write_u32(shader + 4 * i as u32, value).unwrap();
    }
    assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m, 0);
    quad(&mut m, 1);
    d.set_render_target(&mut m, back, 0).unwrap();
    d.set_pixel_shader(&mut m, shader).unwrap();
    d.set_texture(&mut m, 0, TEX_A).unwrap();
    quad(&mut m, 2);
    d.drain_until_callback(&mut m).unwrap();

    assert!(draws(&d).is_empty());
    assert_eq!(d.stats().rtt_direct_samples, 1);
    finish(&mut d, &mut m);
    let stats = d.stats();
    assert_eq!(stats.swallow_reasons[SwallowRefusal::Resource.index()], 0);
    assert_eq!(stats.swallow_reasons[SwallowRefusal::Encode.index()], 1);
    assert_eq!(stats.swallow_draws_replayed, 3);
}

#[test]
fn rebinding_a_rewritten_header_releases_the_held_post_draws_before_destroying_its_old_surface() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, plan(true, Capture)).unwrap());
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
    quad(&mut m, 0);
    d.drain_until_callback(&mut m).unwrap();
    assert!(draws(&d).is_empty());

    m.write_u32(LEVEL_A + 16, 15 | (15 << 12) | (1 << 24)).unwrap();
    d.set_render_target(&mut m, LEVEL_A, 0).unwrap();

    assert!(d.backend.calls.iter().any(|call| matches!(call, Recorded::DestroySurface { .. })));
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::Resource.index()], 1);
    assert_eq!(d.stats().swallow_draws_replayed, 1);
    assert_eq!(d.stats().swallow_commits, 0);
}

#[test]
fn observing_places_the_capture_at_return() {
    let (mut d, mut m, back) = setup(recorder());
    quad(&mut m, 0);
    assert!(d.swallow(&mut m, plan(false, Capture)).unwrap());
    quads(&mut d, &mut m, back);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert!(d.backend.external.is_empty());
    finish(&mut d, &mut m);
    assert_eq!(d.backend.external, [(d.back_buffer().unwrap(), d.back_buffer().unwrap())]);
    assert_eq!(d.stats().swallow_commits, 0, "a capture is not a replacement");
}

#[test]
fn an_unscoped_observer_issues_after_the_matched_end() {
    let (mut d, mut m, _) = setup(recorder());
    assert!(d.swallow(&mut m, Swallow {
        call: None,
        end_returns: Vec::new(),
        targets: vec![0],
        ..strips_plan(false)
    }).unwrap());
    let back = d.back_buffer().unwrap();
    d.swallow_caller(&m, CALL.esp, CALL.ret, |_| panic!("an observer needs no return hook")).unwrap();
    strip(&mut d, &mut m, 0.0);
    assert!(!d.swallowing());
    assert_eq!(d.backend.external, [(back, back)]);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().swallow_commits, 0);
}

#[test]
fn a_pass_that_cannot_be_issued_holds_nothing() {
    let (mut d, mut m, back) = setup(RecordingBackend::new());
    assert!(!d.swallow(&mut m, plan(true, Capture)).unwrap());
    quads(&mut d, &mut m, back);
    assert!(draws(&d).is_empty(), "replayed packets are drawn by nothing (crate::gap)");
    assert_eq!(d.stats().external_refusals[ExternalRefusal::Unsupported.index()], 1);
    assert_eq!(d.stats().swallowed_draws, 0);
}

struct Encode {
    attempts: Arc<AtomicU32>,
    ranges: usize,
}

impl ExternalPass for Encode {
    fn encode(&mut self, _: ExternalTarget<'_>) {}
    fn colour_only(&self) -> bool { true }

    fn record_ranges(&mut self, t: ExternalTarget<'_>) -> usize {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        for slot in t.slots.iter().flatten() {
            drop(t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("synthetic post replacement"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &slot.attachment,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
        }
        self.ranges
    }
}

impl SwallowPass for Encode {}

fn gpu() -> Option<GfxBackend> {
    let ctx = gfx::GfxContext::headless().ok()?;
    ctx.caps.depth32_stencil8.then(|| GfxBackend::new(ctx))
}

fn images(d: &mut Device<GfxBackend>, m: &mut SparseMemory) -> (Vec<u8>, Vec<u8>) {
    d.flush_render_targets(m).unwrap();
    let intermediate = m.read_vec(0x8000_0000 | DATA_A, LEVEL_BYTES as usize).unwrap();
    let back = d.back_buffer().unwrap();
    let (final_colour, _) = d.backend.read_color(back).unwrap();
    (intermediate, final_colour)
}

#[test]
fn a_failed_native_encode_replays_originals_on_both_attachments() {
    let Some(gpu) = gpu() else { return };
    let second = GfxBackend::new(gfx::GfxContext {
        instance: gpu.ctx.instance.clone(),
        adapter: gpu.ctx.adapter.clone(),
        device: gpu.ctx.device.clone(),
        queue: gpu.ctx.queue.clone(),
        caps: gpu.ctx.caps,
    });
    let (mut baseline, mut m, back) = setup(gpu);
    quads(&mut baseline, &mut m, back);
    let expected = images(&mut baseline, &mut m);

    let (mut d, mut m, back) = setup(second);
    let attempts = Arc::new(AtomicU32::new(0));
    assert!(d.swallow(&mut m, plan(true, Encode { attempts: Arc::clone(&attempts), ranges: 1 })).unwrap());
    quads(&mut d, &mut m, back);
    assert_eq!(d.stats().draws, 0);
    finish(&mut d, &mut m);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(d.stats().swallow_draws_replayed, 3);
    assert_eq!(d.stats().swallow_draws_dropped, 0);
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::Encode.index()], 1);
    assert_eq!(images(&mut d, &mut m), expected);
}

#[test]
fn preparing_a_bound_external_pass_replays_held_quads_before_its_commit() {
    let Some(gpu) = gpu() else { return };
    let (mut d, mut m, back_header) = setup(gpu);
    let back = d.back_buffer().unwrap();
    assert!(d.swallow(&mut m, Swallow {
        headers: Vec::new(),
        targets: vec![0, 0, 0],
        ..plan(true, Capture)
    }).unwrap());
    quad(&mut m, 0);
    quad(&mut m, 1);
    d.drain_until_callback(&mut m).unwrap();
    assert_eq!(d.stats().swallowed_draws, 2);
    assert_eq!(d.stats().draws, 0);

    let external = d3d::ExternalPlan {
        stage: timeline::native::Stage::Objects,
        view: 0,
        ranges: 1,
        slots: vec![Some(d3d::ExternalSlot { header: back_header, written: true })],
    };
    let attempts = Arc::new(AtomicU32::new(0));
    let pass = Encode { attempts: Arc::clone(&attempts), ranges: 1 };
    d.prepare_external_bound(&mut m, external, Box::new(pass), vec![Some(back)])
        .unwrap()
        .unwrap()
        .commit();
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(d.stats().swallow_draws_replayed, 2);
    assert_eq!(d.stats().swallow_reasons[SwallowRefusal::Dependency.index()], 1);
    assert!(!d.swallowing());

    // The committed clear covered both replayed quads. Reversing the
    // command order would leave their colours over the green clear.
    let (pixels, _) = d.backend.read_color(back).unwrap();
    assert!(pixels.chunks_exact(4).all(|p| p == [0, 255, 0, 255]));
}

#[test]
fn only_the_return_commits_a_complete_replacement_and_marks_intermediates_written() {
    let Some(gpu) = gpu() else { return };
    let (mut d, mut m, back) = setup(gpu);
    let attempts = Arc::new(AtomicU32::new(0));
    assert!(d.swallow(&mut m, plan(true, Encode { attempts: Arc::clone(&attempts), ranges: 3 })).unwrap());
    quads(&mut d, &mut m, back);
    assert_eq!(attempts.load(Ordering::Relaxed), 0);
    assert_eq!(d.stats().swallow_draws_dropped, 0);
    finish(&mut d, &mut m);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
    assert_eq!(d.stats().swallow_commits, 1);
    assert_eq!(d.stats().swallow_draws_dropped, 3);
    assert_eq!(d.stats().swallow_draws_replayed, 0);
    let (intermediate, final_colour) = images(&mut d, &mut m);
    assert_eq!(&intermediate[..4], [0, 255, 0, 255]);
    assert_eq!(&final_colour[..4], [0, 255, 0, 255]);
}
