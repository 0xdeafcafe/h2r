//! The scene sink (docs/graphics/framescene.md, slice 6): the replay it
//! keeps of the draws, copies, samples and admissions it is given, and
//! what a replaying frame needs to be drawn privately. Packets come from
//! the device's draw boundary ([`guest_draw`]); no GPU unless gated.

use std::sync::atomic::Ordering;

use crate::common::*;
use d3d::{DrawAdmission, DrawSnapshot, Device, RecordingBackend, ResolvedDraw, SceneSink, SparseMemory, UploadPolicy};
use frame_scene::{Slot, SlotAt, TargetRef};

fn quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    quad_vb(m, 0.0, 0.0, 32.0, 32.0, color);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// The packet the device resolves for a guest quad. A guest draw no
/// native stage takes reaches no backend (`d3d::gap`), so the packet is
/// taken from the draw boundary: the admission is offered the resolved
/// draw and declines it.
fn guest_quad(x0: f32, y0: f32, x1: f32, y1: f32, color: u32) -> ResolvedDraw {
    use gfx::vertex::PrimitiveType;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Capture(Mutex<Option<ResolvedDraw>>);
    impl DrawAdmission for Capture {
        fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            *self.0.lock().unwrap() = Some(snapshot.draw.clone());
            None
        }
    }
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut memory) = create(backend, UploadPolicy::Always);
    set_states(&mut device, &mut memory);
    quad_vb(&mut memory, x0, y0, x1, y1, color);
    bind_quad(&mut device, &mut memory);
    let capture = Capture::default();
    assert!(!device.try_native_array_draw(&mut memory, PrimitiveType::TriangleStrip, 0, 4, &capture).unwrap());
    capture.0.into_inner().unwrap().expect("the admission was offered the draw")
}

fn guest_draw(color: u32) -> ResolvedDraw {
    guest_quad(0.0, 0.0, 32.0, 32.0, color)
}

#[test]
fn a_sink_with_no_slot_records_nothing_and_changes_nothing() {
    fn off() -> Option<SlotAt> {
        None
    }
    let (mut d, mut m) = create(SceneSink::new(RecordingBackend::new(), off).keeping_draws(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let (mut plain, mut pm) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut plain, &mut pm);
    for _ in 0..2 {
        quad(&mut d, &mut m, 0xFF12_3456);
        d.swap(&mut m, 0).unwrap();
        quad(&mut plain, &mut pm, 0xFF12_3456);
        plain.swap(&mut pm, 0).unwrap();
    }
    assert!(d.backend.last_frame().ops.is_empty());
    assert_eq!(d.backend.inner().calls, plain.backend.calls, "the sink passes the same calls on");
}

/// A pooled frame-buffer copy refilled in the frame: a new sample, which
/// names the earlier sample whose texture it refills, so replay copies
/// into that texture rather than make one per sample. Draws before the
/// refill bind the first sample, draws after it the second.
#[test]
fn a_refilled_copy_is_a_new_sample_that_names_the_one_it_refills() {
    use d3d::{Backend, Recorded};
    use d3d::scene_sink::{ReplayOp, ReplaySample, ReplayTexture};
    use gfx::{Clear, D3DFormat};
    use std::sync::{Arc, Mutex};

    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::Opaque, view: 0 }) }
    let draw = guest_draw(0xFF19_3264);

    let frames = Arc::new(Mutex::new(Vec::new()));
    let output = frames.clone();
    let mut sink = SceneSink::new(RecordingBackend::new(), slot)
        .with_replay(move |frame| output.lock().unwrap().push(frame));
    let fmt = D3DFormat::LinA8R8G8B8;
    let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
    let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
    let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();

    sink.clear(colour, Some(depth), Clear::all([0.1, 0.2, 0.3, 1.0])).unwrap();
    let mut copy = sink.snapshot_surface(colour, false).unwrap().unwrap();
    sink.draw(colour, Some(depth), &draw, [Some(&copy), None, None, None]).unwrap();
    assert!(sink.snapshot_surface_into(colour, false, &mut copy).unwrap());
    sink.draw(colour, Some(depth), &draw, [Some(&copy), None, None, None]).unwrap();
    assert!(sink.copy_surface(colour, shown).unwrap());
    sink.present(shown, 0).unwrap();

    // The backend refilled the texture it made.
    let calls = &sink.inner().calls;
    let made: Vec<u32> = calls.iter().filter_map(|c| match c {
        Recorded::Snapshot { id, .. } => Some(*id),
        _ => None,
    }).collect();
    assert_eq!(made.len(), 1);
    assert!(calls.iter().any(|c| matches!(c, Recorded::SnapshotInto { id, .. } if *id == made[0])));

    let frame = frames.lock().unwrap().pop().unwrap();
    let samples: Vec<_> = frame.ops.iter().filter_map(|(_, op)| match op {
        ReplayOp::Sample { id, how, into, .. } => Some((*id, how.clone(), *into)),
        _ => None,
    }).collect();
    let [(first, ReplaySample::Snapshot { opaque: false }, None),
        (second, ReplaySample::Snapshot { opaque: false }, Some(refills))] = samples.as_slice()
    else { panic!("unexpected samples: {samples:?}") };
    assert_eq!(refills, first);
    assert_ne!(second, first);
    let bound: Vec<_> = frame.ops.iter().filter_map(|(_, op)| match op {
        ReplayOp::Draw { textures, .. } => Some(textures[0].clone()),
        _ => None,
    }).collect();
    assert!(
        matches!(bound.as_slice(), [Some(ReplayTexture::Sample(a)), Some(ReplayTexture::Sample(b))] if a == first && b == second),
        "{bound:?}"
    );
}

#[test]
fn replay_retains_the_sample_version_draw_bindings_and_distinct_copy_targets() {
    use d3d::Backend;
    use d3d::scene_sink::{ReplayOp, ReplaySample, ReplayTexture};
    use gfx::texture::{TextureDesc, TextureKind, decode_texture};
    use gfx::{Clear, D3DFormat};
    use std::sync::{Arc, Mutex};

    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::Opaque, view: 0 }) }
    let draw = guest_draw(0xFF19_3264);

    let frames = Arc::new(Mutex::new(Vec::new()));
    let output = frames.clone();
    let mut sink = SceneSink::new(RecordingBackend::new(), slot)
        .with_replay(move |frame| output.lock().unwrap().push(frame));
    let fmt = D3DFormat::LinA8R8G8B8;
    let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
    let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
    let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
    let desc = TextureDesc::new(fmt, TextureKind::D2, 1, 1, 1, 1);
    let decoded = decode_texture(&desc, &[11, 22, 33, 44], None, Default::default()).unwrap();
    let image = sink.upload_texture(&decoded).unwrap();

    sink.clear(colour, Some(depth), Clear::all([0.1, 0.2, 0.3, 1.0])).unwrap();
    let snap = sink.snapshot_surface(colour, true).unwrap().unwrap();
    sink.clear(colour, Some(depth), Clear::all([1.0, 0.0, 0.0, 1.0])).unwrap();
    sink.draw(colour, Some(depth), &draw, [Some(&image), None, Some(&snap), None]).unwrap();
    assert!(sink.copy_surface(colour, shown).unwrap());
    sink.present_with_gamma(shown, 17, Some(&[12; 768])).unwrap();

    let frame = frames.lock().unwrap().pop().unwrap();
    assert_eq!(frame.present, 1);
    assert_eq!(frame.flags, 17);
    assert_eq!(frame.gamma.as_deref(), Some(&[12; 768]));
    assert_eq!(frame.colour.unwrap().width, 32);
    assert!(frame.guest_reference_complete());
    assert_eq!(frame.private_target_refusal(), None);
    let ops: Vec<_> = frame.ops.iter().map(|(at, op)| {
        assert_eq!(Some(*at), slot());
        op
    }).collect();
    let [ReplayOp::Clear { colour: side, .. },
        ReplayOp::Sample { of, id, how: ReplaySample::Snapshot { opaque: true }, into: None },
        ReplayOp::Clear { .. }, ReplayOp::Bind { colour: bound, .. },
        ReplayOp::Draw { draw: kept, textures, .. },
        ReplayOp::Copy { from, to: TargetRef::SceneColour }] = ops.as_slice()
    else { panic!("unexpected replay: {ops:?}") };
    assert!(matches!(side, TargetRef::Aux(_)));
    assert_eq!(side, of);
    assert_eq!(side, bound);
    assert_eq!(side, from);
    assert_eq!(kept.as_ref(), &draw);
    assert!(matches!(&textures[0], Some(ReplayTexture::Upload(asset)) if **asset == decoded));
    assert!(matches!(textures[2], Some(ReplayTexture::Sample(bound)) if bound == *id));

    // An old snapshot id cannot silently resolve to this frame's first
    // sample. Its origin includes the frame it came from.
    sink.clear(shown, Some(depth), Clear::all([0.0; 4])).unwrap();
    let _ = sink.snapshot_surface(shown, true).unwrap().unwrap();
    sink.draw(shown, Some(depth), &draw, [Some(&snap), None, None, None]).unwrap();
    sink.present(shown, 0).unwrap();
    let next = frames.lock().unwrap().pop().unwrap();
    assert_eq!(next.refusals, ["unavailable_frozen_sample"]);

    sink.inner_mut().sample_surfaces = true;
    let live = sink.surface_texture(shown).unwrap().unwrap();
    sink.present(shown, 0).unwrap();
    sink.clear(shown, Some(depth), Clear::all([1.0; 4])).unwrap();
    sink.draw(shown, Some(depth), &draw, [Some(&live), None, None, None]).unwrap();
    sink.present(shown, 0).unwrap();
    assert_eq!(frames.lock().unwrap().last().unwrap().refusals, ["unavailable_in_place_binding"]);
}

#[test]
fn a_private_playback_requires_the_contents_a_copy_or_draw_inherits() {
    use d3d::Backend;
    use gfx::texture::{TextureDesc, TextureKind};
    use gfx::D3DFormat;
    use std::sync::{Arc, Mutex};

    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::AfterPost, view: 0 }) }
    let frames = Arc::new(Mutex::new(Vec::new()));
    let output = frames.clone();
    let mut sink = SceneSink::new(RecordingBackend::new(), slot)
        .with_replay(move |frame| output.lock().unwrap().push(frame));
    let fmt = D3DFormat::LinA8R8G8B8;
    let source = sink.create_texture_target(fmt, 2, 2).unwrap();
    let target = sink.create_surface(fmt, 2, 2, false).unwrap();
    sink.copy_surface(source, target).unwrap();
    sink.present(target, 0).unwrap();
    assert_eq!(frames.lock().unwrap().pop().unwrap().private_target_refusal(), Some("frame_needs_previous_target_contents"));

    let desc = TextureDesc::new(fmt, TextureKind::D2, 2, 2, 1, 1);
    let mut bytes = vec![7; 16];
    sink.write_surface(source, &desc, &bytes).unwrap();
    bytes.fill(0);
    sink.copy_surface(source, target).unwrap();
    sink.present(target, 0).unwrap();
    let frame = frames.lock().unwrap().pop().unwrap();
    assert_eq!(frame.private_target_refusal(), None);
    assert!(matches!(&frame.ops[0].1, d3d::scene_sink::ReplayOp::Upload { bytes, .. } if **bytes == [7; 16]));
}

#[test]
fn a_later_clear_defines_the_final_image_but_not_an_earlier_pass_snapshot() {
    use d3d::scene_sink::{ReplayFrame, ReplayOp};
    use frame_scene::AuxTarget;
    use gfx::{Clear, D3DFormat};

    let side = TargetRef::Aux(0);
    let main = TargetRef::SceneColour;
    let small = AuxTarget { width: 3, height: 4, format: D3DFormat::LinA8R8G8B8 as u32, depth: false };
    let large = AuxTarget { width: 8, ..small };
    let at = SlotAt { slot: Slot::AfterLights, view: 0 };
    let clear = |colour| ReplayOp::Clear { colour, depth: TargetRef::None, clear: Clear::all([0.2, 0.3, 0.4, 1.0]) };
    let frame = ReplayFrame {
        presented: main,
        colour: Some(large),
        aux: vec![small],
        ops: vec![(at, clear(side)), (at, ReplayOp::Copy { from: side, to: main }), (at, clear(main))],
        ..Default::default()
    };
    assert_eq!(frame.private_read_refusal(1, side), None);
    assert!(frame.private_read_refusal(2, main).is_some(), "a small copy leaves old pixels around it");
    assert_eq!(frame.private_target_refusal(), None);
}

#[test]
fn the_worker_adopts_only_the_actual_immutable_draw_packet_including_unused_nan_bits() {
    use d3d::{Backend, ThreadedBackend};
    use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplayTexture};
    use gfx::texture::{TextureDesc, TextureKind, decode_texture};
    use d3d::threaded::owner::{Candidate, FrameOwner, Submission, Outputs};
    use std::sync::{Arc, Mutex};

    let mut packet = guest_draw(0xFF00_8080);
    Arc::make_mut(&mut packet.shader.constants)[0][0] = f32::NAN;
    let packet = Arc::new(packet);

    let pending = Arc::new(Mutex::new(None::<ReplayFrame>));
    let input = Arc::clone(&pending);
    let owner = FrameOwner {
        select: Box::new(move |present, _| {
            let mut replay = input.lock().unwrap().take()?;
            if present == 2 {
                for (_, op) in &mut replay.ops {
                    if let ReplayOp::Draw { draw, .. } = op {
                        // A separately allocated packet is not the one queued by the device.
                        *draw = Arc::new(draw.as_ref().clone());
                    }
                }
            }
            if present == 3 {
                for (_, op) in &mut replay.ops {
                    if let ReplayOp::Draw { textures, .. } = op
                        && let Some(ReplayTexture::Upload(data)) = &mut textures[0]
                    {
                        // Identical texels from a different upload still have a different lifetime.
                        *data = Arc::new(data.as_ref().clone());
                    }
                }
            }
            let target = replay.presented;
            Some(Submission::Native(Candidate {
                frame: Arc::new(replay),
                before_original: None,
                eligible: Box::new(|_, _| true),
                draw: Box::new(move |gpu: &mut RecordingBackend, targets| {
                    gpu.present(targets[&target], 0)?;
                    Ok(Outputs::default())
                }),
            }))
        }),
        events: std::sync::Arc::new(()),
    };
    let device = ThreadedBackend::new(RecordingBackend::new()).unwrap().with_frame_owner(owner);
    let mut sink = SceneSink::new(device, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
        .with_replay(move |frame| *pending.lock().unwrap() = Some(frame));
    let surface = sink.create_surface(gfx::D3DFormat::LinA8R8G8B8, 32, 32, false).unwrap();
    let desc = TextureDesc::new(gfx::D3DFormat::LinA8R8G8B8, TextureKind::D2, 1, 1, 1, 1);
    let data = decode_texture(&desc, &[12, 34, 56, 78], None, Default::default()).unwrap();
    let texture = sink.upload_texture(&data).unwrap();
    for expected_legacy in [0, 1, 2] {
        sink.draw_shared(surface, None, Arc::clone(&packet), [Some(&texture), None, None, None]).0.unwrap();
        sink.present(surface, 0).unwrap();
        let legacy_draws = sink.inner_mut().with_inner(|gpu| gpu.draws().len()).unwrap();
        assert_eq!(legacy_draws, expected_legacy);
    }
}

#[test]
fn tiled_masked_clears_need_every_pixel_of_every_written_channel() {
    use d3d::backend::{ChannelMask, PixelRect, RectClear};
    use d3d::scene_sink::{ReplayFrame, ReplayOp};

    let main = TargetRef::SceneColour;
    let at = SlotAt { slot: Slot::Opaque, view: 0 };
    let rgba = ChannelMask { red: true, green: true, blue: true, alpha: true };
    let rgb = ChannelMask { alpha: false, ..rgba };
    let alpha = ChannelMask { alpha: true, ..ChannelMask::default() };
    let rect = |x, y, width, height| PixelRect { x, y, width, height };
    let clear = |rects, mask| (at, ReplayOp::ClearRects {
        colour: main, depth: TargetRef::None,
        clear: RectClear { rects, color: Some(([0.4, 0.2, 0.1, 0.8], mask)), depth: None, stencil: None },
    });
    let mut frame = ReplayFrame {
        colour: Some(frame_scene::AuxTarget { width: 6, height: 5, format: gfx::D3DFormat::LinA8R8G8B8 as u32, depth: false }),
        presented: main, ..Default::default()
    };
    frame.ops.push(clear(vec![rect(0, 0, 4, 5), rect(2, 0, 4, 3)], rgba));
    frame.ops.push(clear(vec![rect(4, 3, 2, 2)], rgb));
    assert!(frame.private_target_refusal().is_some(), "the bottom right alpha is missing");
    frame.ops.push(clear(vec![rect(4, 3, 2, 1), rect(4, 4, 1, 1)], alpha));
    assert!(frame.private_target_refusal().is_some(), "one uncovered pixel");
    frame.ops.push(clear(vec![rect(5, 4, 1, 1)], alpha));
    assert_eq!(frame.private_target_refusal(), None);
    assert!(frame.all_targets_defined());
}

#[test]
fn a_frozen_sample_needs_the_earlier_pixels_and_an_in_place_view_sees_the_later_clear() {
    use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
    use frame_scene::{AuxTarget, carried::CameraUse};
    use gfx::{Clear, D3DFormat};

    let mut draw = guest_draw(0xFF55_8822);
    draw.state.render.color_write = gfx::state::ColorWriteMask::ALL;
    let main = TargetRef::SceneColour;
    let side = TargetRef::Aux(0);
    let at = SlotAt { slot: Slot::AfterLights, view: 0 };
    let desc = AuxTarget { width: W, height: H, format: D3DFormat::LinA8R8G8B8 as u32, depth: false };
    let clear = |colour| (at, ReplayOp::Clear { colour, depth: TargetRef::None, clear: Clear::all([0.2, 0.5, 0.7, 1.0]) });
    let frame = |how| ReplayFrame {
        presented: main, colour: Some(desc), aux: vec![desc], ops: vec![
            clear(main), (at, ReplayOp::Sample { id: 0, of: side, how, into: None }), clear(side),
            (at, ReplayOp::Bind { colour: main, depth: TargetRef::None }),
            (at, ReplayOp::Draw { draw: std::sync::Arc::new(draw.clone()), textures: [Some(ReplayTexture::Sample(0)), None, None, None], camera: CameraUse::Screen, external: None }),
        ], ..Default::default()
    };
    let live = frame(ReplaySample::InPlace);
    assert_eq!(live.private_target_refusal(), None);
    assert!(live.all_targets_defined());
    let mut snap = frame(ReplaySample::Snapshot { opaque: false });
    assert!(snap.private_target_refusal().is_some());
    snap.ops.push(clear(main));
    assert_eq!(snap.private_target_refusal(), None, "the final display no longer uses that sample");
    assert!(!snap.all_targets_defined(), "another caller can still hold that frozen sample");
}

#[cfg(feature = "gpu")]
#[test]
fn an_opaque_full_target_rhw_strip_defines_colour_only_when_it_covers_every_sample() {
    use d3d::backend::PixelRect;
    use d3d::scene_sink::{ReplayFrame, ReplayOp};
    use frame_scene::{AuxTarget, carried::CameraUse};
    use gfx::state::{ColorWriteMask, RenderState, CullMode, DepthBufferMode, TextureOp};

    let mut draw = guest_quad(-0.5, -0.5, W as f32 - 0.5, H as f32 - 0.5, 0xE05F_15F0);
    draw.state.render = RenderState { cull_mode: CullMode::None, depth_mode: DepthBufferMode::Disabled, ..Default::default() };
    draw.state.stages[0].color_op = TextureOp::Disable;
    std::sync::Arc::make_mut(&mut draw.raw).render.fill(0);
    draw.hw.stipple_control = Some(0);
    let main = TargetRef::SceneColour;
    let at = SlotAt { slot: Slot::Overlay, view: 0 };
    let frame = |draw: &ResolvedDraw| ReplayFrame {
        presented: main,
        colour: Some(AuxTarget { width: W, height: H, format: draw.target.format as u32, depth: false }),
        ops: vec![(at, ReplayOp::Bind { colour: main, depth: TargetRef::None }),
            (at, ReplayOp::Draw { draw: std::sync::Arc::new(draw.clone()), textures: [None, None, None, None], camera: CameraUse::Screen, external: None })],
        ..Default::default()
    };
    let defined = |draw| frame(draw).private_target_refusal().is_none();
    assert!(defined(&draw));
    let mut clipped = draw.clone();
    clipped.clip = Some(PixelRect { x: 1, y: 0, width: W - 1, height: H });
    assert!(!defined(&clipped));
    let mut blended = draw.clone();
    blended.state.render.blend_enable = true;
    assert!(!defined(&blended));
    let mut masked = draw.clone();
    masked.state.render.color_write = ColorWriteMask { alpha: false, ..ColorWriteMask::ALL };
    assert!(!defined(&masked));
    let mut stippled = draw.clone();
    stippled.hw.stipple_control = Some(1);
    assert!(!defined(&stippled));

    let ctx = match gfx::GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => { eprintln!("skipping GPU readback: {e}"); return; }
    };
    let mut gpu = d3d::GfxBackend::new(ctx);
    let frame = frame(&draw);
    for scale in [1, 4] {
        let mut private = d3d::gfx_backend::GuestDrawPass::new(&mut gpu, &frame, scale).unwrap();
        for (_, op) in &frame.ops { private.apply(op).unwrap(); }
        let ([w, h], pixels) = private.read_rgba().unwrap();
        assert_eq!([w, h], [W * scale, H * scale]);
        assert!(pixels.chunks_exact(4).all(|p| p == [0x5f, 0x15, 0xf0, 0xe0]));
    }
}

#[cfg(feature = "gpu")]
#[test]
fn inherited_in_place_bindings_name_the_live_surface_and_read_it_at_the_draw() {
    use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplayTexture};
    use d3d::{Backend, ThreadedBackend};
    use gfx::{Clear, D3DFormat};
    use std::sync::{Arc, Mutex};

    let mut draw = guest_draw(0xff42_6503);
    draw.state.render.color_write = gfx::state::ColorWriteMask::ALL;
    let mut original = RecordingBackend::new();
    original.sample_surfaces = true;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&frames);
    let mut sink = SceneSink::new(ThreadedBackend::new(original).unwrap(), || {
        Some(SlotAt { slot: Slot::Opaque, view: 0 })
    }).with_replay(move |frame| keep.lock().unwrap().push(frame));
    let side = sink.create_texture_target(D3DFormat::LinA8R8G8B8, W, H).unwrap();
    let main = sink.create_texture_target(D3DFormat::LinA8R8G8B8, W, H).unwrap();
    sink.draw(side, None, &draw, [None, None, None, None]).unwrap();
    let old_view = sink.surface_texture(side).unwrap().unwrap();
    sink.present(main, 0).unwrap();

    let take = || -> ReplayFrame { frames.lock().unwrap().pop().unwrap() };
    let _ = take();
    for cleared in [false, true] {
        sink.clear(main, None, Clear::all([0.4, 0.3, 0.7, 1.0])).unwrap();
        if cleared { sink.clear(side, None, Clear::all([0.1, 0.9, 0.2, 1.0])).unwrap(); }
        sink.draw(main, None, &draw, [Some(&old_view), None, None, None]).unwrap();
        sink.present(main, 0).unwrap();
        let frame = take();
        assert!(frame.refusals.is_empty());
        let target = frame.ops.iter().find_map(|(_, op)| match op {
            ReplayOp::Draw { textures, .. } => match &textures[0] {
                Some(ReplayTexture::Live { target, .. }) => Some(*target), _ => None,
            },
            _ => None,
        }).unwrap();
        assert!(matches!(target, TargetRef::Aux(_)));
        assert_eq!(frame.live_bindings().count(), 1);
        let final_read = (frame.ops.len(), frame.presented);
        let required = if cleared { vec![] } else { vec![(target, 15)] };
        assert_eq!(frame.private_seed_requirements(&[final_read]), Some(required));
        assert_eq!(frame.private_target_refusal(), Some("unvalidated_in_place_binding"));
    }
    sink.destroy_surface(side).unwrap();
    sink.draw(main, None, &draw, [Some(&old_view), None, None, None]).unwrap();
    sink.present(main, 0).unwrap();
    assert_eq!(take().refusals, ["released_in_place_source"]);
}

/// Each object shadow's ops follow an `ObjectShadow` mark and a `None`
/// mark ends them. Without the marks the replay is the same op for op as
/// one recorded with no shadow source, and a source that declares no
/// shadow writes none.
#[test]
fn object_shadow_marks_wrap_each_shadows_ops_and_change_nothing_else() {
    use d3d::Backend;
    use d3d::scene_sink::{ReplayFrame, ReplayOp, ShadowSource};
    use gfx::{Clear, D3DFormat};
    use std::sync::atomic::AtomicU32;
    use std::sync::{Arc, Mutex};

    static SHADOW: AtomicU32 = AtomicU32::new(u32::MAX);
    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::Opaque, view: 0 }) }
    fn shadow() -> Option<u32> {
        let v = SHADOW.load(Ordering::Relaxed);
        (v != u32::MAX).then_some(v)
    }
    fn never() -> Option<u32> { None }
    let draw = guest_draw(0xFF19_3264);

    let record = |marking: Option<ShadowSource>| -> ReplayFrame {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut sink = SceneSink::new(RecordingBackend::new(), slot);
        if let Some(shadow_at) = marking {
            sink = sink.with_object_shadows(shadow_at);
        }
        let mut sink = sink.with_replay(move |frame| output.lock().unwrap().push(frame));
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let map = sink.create_surface(D3DFormat::LinD24S8, 16, 16, true).unwrap();
        let map_colour = sink.create_texture_target(fmt, 16, 16).unwrap();
        SHADOW.store(u32::MAX, Ordering::Relaxed);
        sink.clear(colour, Some(depth), Clear::all([0.0, 0.0, 0.0, 1.0])).unwrap();
        sink.draw(colour, Some(depth), &draw, [None; 4]).unwrap();
        for n in 0..2 {
            SHADOW.store(n, Ordering::Relaxed);
            sink.clear(map_colour, Some(map), Clear::all([1.0; 4])).unwrap();
            sink.draw(map_colour, Some(map), &draw, [None; 4]).unwrap();
            sink.draw(colour, Some(depth), &draw, [None; 4]).unwrap();
        }
        SHADOW.store(u32::MAX, Ordering::Relaxed);
        sink.draw(colour, Some(depth), &draw, [None; 4]).unwrap();
        sink.present(colour, 0).unwrap();
        frames.lock().unwrap().pop().unwrap()
    };
    let unmarked = record(None);
    let marked = record(Some(shadow as ShadowSource));
    let marks: Vec<Option<u32>> = marked
        .ops
        .iter()
        .filter_map(|(_, op)| match op {
            ReplayOp::ObjectShadow(s) => Some(*s),
            _ => None,
        })
        .collect();
    assert_eq!(marks, [Some(0), Some(1), None]);
    // Every op between a shadow's mark and the next is that shadow's.
    let mut current = None;
    let mut drawn = [0; 2];
    for (_, op) in &marked.ops {
        match op {
            ReplayOp::ObjectShadow(s) => current = *s,
            ReplayOp::Draw { .. } => {
                if let Some(n) = current {
                    drawn[n as usize] += 1;
                }
            }
            _ => {}
        }
    }
    assert_eq!(drawn, [2, 2]);
    let ops = |frame: &ReplayFrame| -> Vec<String> {
        frame.ops.iter().filter(|(_, op)| !op.is_marker()).map(|(at, op)| format!("{at:?} {op:?}")).collect()
    };
    assert_eq!(ops(&marked), ops(&unmarked), "the marks are the only difference");
    assert_eq!(marked.refusals, unmarked.refusals);
    let quiet = record(Some(never as ShadowSource));
    assert!(!quiet.ops.iter().any(|(_, op)| op.is_marker()));
    assert_eq!(ops(&quiet), ops(&unmarked));
}

/// A typed admission offered with the guest's one draw (docs/graphics/
/// framescene.md): the draw is issued once, as any guest draw is, and the
/// replay keeps the admission beside it only when it is exact for that draw.
mod retained_admission {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use d3d::backend::{ExternalPassRequest, ExternalPlan, ExternalSurfaces, RetainedAdmission};
    use d3d::scene_sink::{ReplayOp, ReplayTexture, RetainedExternal};
    use d3d::{Backend, DrawOutcome, ExternalPass};
    use gfx::D3DFormat;
    use timeline::native::Stage;

    struct Flare(bool);

    impl ExternalPass for Flare {
        fn replays_scene(&self) -> bool { self.0 }
    }

    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::Opaque, view: 0 }) }

    struct Outcome {
        result: Result<DrawOutcome, String>,
        /// Draws the sink's backend received for the one call.
        issued: usize,
        external: Option<Box<RetainedExternal>>,
        replay_draws: usize,
        refusals: Vec<&'static str>,
        /// The draw the guest submitted, as the replay must see it.
        submitted: ResolvedDraw,
    }

    /// One admitted draw through a real sink: `case` may break the
    /// admission, the submitted draw or the sink's mode.
    fn capture(
        reference: bool,
        case: impl FnOnce(&mut RetainedAdmission<u32>, &mut ResolvedDraw, [u32; 3]),
    ) -> Outcome {
        let draw = guest_draw(0xFF40_8020);

        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut sink = SceneSink::new(RecordingBackend::new(), slot)
            .with_replay(move |frame| output.lock().unwrap().push(frame))
            .with_guest_reference_request(move || reference);
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let mut admission = RetainedAdmission {
            packets: Arc::from([Arc::new(draw.clone())]),
            surfaces: ExternalSurfaces { color: colour, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest {
                rect: (0, 0, 32, 32),
                surface_size: (32, 32),
                z_scale: 1.0,
                display_projection_factor: 1.0,
            },
            plan: ExternalPlan { stage: Stage::Flares, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        let mut submitted = draw;
        case(&mut admission, &mut submitted, [colour, depth, shown]);
        let before = sink.inner().draws().len();
        let kept = submitted.clone();
        let (result, _) = sink.draw_admitted(colour, Some(depth), submitted, [None; 4], admission);
        let issued = sink.inner().draws().len() - before;
        assert!(sink.copy_surface(colour, shown).unwrap());
        sink.present(shown, 0).unwrap();

        let mut frame = frames.lock().unwrap().pop().unwrap();
        let refusals = frame.refusals.clone();
        let mut external = None;
        let mut replay_draws = 0;
        for (_, op) in &mut frame.ops {
            if let ReplayOp::Draw { external: e, .. } = op {
                replay_draws += 1;
                external = e.take();
            }
        }
        Outcome { result, issued, external, replay_draws, refusals, submitted: kept }
    }

    #[test]
    fn an_exact_admission_rides_the_one_draw_and_replay_keeps_it() {
        let o = capture(false, |_, _, _| {});
        assert_eq!(o.result, Ok(DrawOutcome::Drawn));
        assert_eq!((o.issued, o.replay_draws), (1, 1), "issued once, carried once");
        let external = o.external.expect("exact for this draw");
        assert_eq!(**external.packet(), o.submitted);
        assert_eq!(external.plan.stage, Stage::Flares);
        assert_ne!(external.surfaces.color, external.surfaces.depth, "the draw's own depth, not scratch");
        assert!(o.refusals.is_empty(), "{:?}", o.refusals);
    }

    /// A sorted transparent range is a native producer's commit with no
    /// guest draw: the sink carries it (`carries_native_stage`) as one
    /// replay draw holding the pass, which resolves on the display copy's
    /// attachments as it does on the reference's.
    #[test]
    fn a_transparent_run_replays_on_the_display_copy() {
        let (begun, ops, refusals) = native_carry(Stage::Transparents, false, |_| {});
        assert!(begun, "the sink carries a transparent range");
        assert!(refusals.is_empty(), "{refusals:?}");
        let external = ops
            .iter()
            .find_map(|op| match op {
                ReplayOp::Draw { external, .. } => external.as_deref(),
                _ => None,
            })
            .expect("the replay keeps the pass beside its packet");
        assert_eq!(external.plan.stage, Stage::Transparents);
        let draw = external.packet().clone();
        let (colour, depth) = (external.surfaces.color, external.surfaces.depth);
        let display = HashMap::from([(colour, 7u32), (depth, 8)]);
        assert!(external.resolve(&draw, &display, (7, Some(8)), true, 1u32).is_some());
        // A guest reference request is a frame of guest draws only.
        let (begun, _, _) = native_carry(Stage::Transparents, true, |_| {});
        assert!(!begun);
    }

    /// An object part's native range is carried like a transparent one: one
    /// replay draw holding the pass, resolvable on the display copy.
    #[test]
    fn object_parts_in_a_kept_scene_commit_and_replay() {
        let (begun, ops, refusals) = native_carry(Stage::Objects, false, |_| {});
        assert!(begun, "the sink carries an object part's range");
        assert!(refusals.is_empty(), "{refusals:?}");
        let external = ops
            .iter()
            .find_map(|op| match op {
                ReplayOp::Draw { external, .. } => external.as_deref(),
                _ => None,
            })
            .expect("the replay keeps the pass beside its packet");
        assert_eq!(external.plan.stage, Stage::Objects);
        let draw = external.packet().clone();
        let display = HashMap::from([(external.surfaces.color, 7u32), (external.surfaces.depth, 8)]);
        assert!(external.resolve(&draw, &display, (7, Some(8)), true, 1u32).is_some());
    }

    /// A pass the sink does not carry is refused by the sink, and the sink
    /// says so (`Backend::take_scene_refusal`, the device's `Sink`); a
    /// carried one is not, and the answer is cleared by asking.
    #[test]
    fn the_sink_names_its_own_refusal_of_a_reservation() {
        let mut sink = SceneSink::new(RecordingBackend::new(), slot).with_replay(|_| {});
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let reserve = |sink: &mut SceneSink<RecordingBackend>, stage| {
            let surfaces = ExternalSurfaces { color: colour, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] };
            let request = ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 };
            let plan = ExternalPlan { stage, view: 0, ranges: 1, slots: vec![] };
            let pass: d3d::SharedReplacement = Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>));
            sink.prepare_external(surfaces, request, [None; 4], &plan, d3d::TargetCopy::Reference, pass).unwrap();
        };
        assert!(!sink.take_scene_refusal());
        reserve(&mut sink, Stage::Particles);
        assert!(sink.take_scene_refusal(), "a particle reservation is not carried");
        assert!(!sink.take_scene_refusal(), "asking clears it");
    }

    #[test]
    fn sorted_water_retains_the_resolved_packet() {
        let o = capture(false, |admission, _, _| admission.plan.stage = Stage::Water);
        assert_eq!(o.external.unwrap().plan.stage, Stage::Water);
        assert_eq!((o.issued, o.replay_draws), (1, 1));
    }

    /// One native producer's water commit through a real sink: the frame
    /// the replay keeps, and what the sink answered at each step.
    fn native_water(stage: Stage, reference: bool) -> (bool, Vec<ReplayOp>, Vec<&'static str>) {
        native_carry(stage, reference, |_| {})
    }

    /// A UI batch's packets: the head and `siblings` more, each its own
    /// draw (the siblings' geometry differs, as an indexed range's does).
    fn batch(siblings: usize) -> impl FnOnce(&mut Vec<ResolvedDraw>) {
        move |packets| {
            for n in 0..siblings {
                let mut sibling = packets[0].clone();
                sibling.vertex_count += 1 + n as u32;
                packets.push(sibling);
            }
        }
    }

    /// Makes `draw` write the depth it is bound with.
    fn write_depth(draw: &mut ResolvedDraw) {
        draw.target.depth_format = Some(D3DFormat::LinD24S8);
        draw.state.render.stencil.enable = true;
        draw.state.render.stencil.write_mask = 0xff;
        draw.state.render.stencil.pass = gfx::state::StencilOp::Replace;
    }

    /// [`native_water`] with the admission's packets as `packets` leaves
    /// them (the producer's one draw to start).
    fn native_carry(
        stage: Stage, reference: bool, packets: impl FnOnce(&mut Vec<ResolvedDraw>),
    ) -> (bool, Vec<ReplayOp>, Vec<&'static str>) {
        let mut draws = vec![guest_draw(0xFF40_8020)];
        packets(&mut draws);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut sink = SceneSink::new(RecordingBackend::new(), slot)
            .with_replay(move |frame| output.lock().unwrap().push(frame))
            .with_guest_reference_request(move || reference);
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let ranges = draws.len();
        let admission = RetainedAdmission {
            packets: draws.into_iter().map(Arc::new).collect(),
            surfaces: ExternalSurfaces { color: colour, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage, view: 0, ranges, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        let begun = sink.begin_native_carry(colour, Some(depth), &admission);
        if begun {
            sink.record_native_carry(colour, Some(depth), [None; 4]);
        }
        sink.end_native_carry();
        assert!(sink.copy_surface(colour, shown).unwrap());
        sink.present(shown, 0).unwrap();
        let frame = frames.lock().unwrap().pop().unwrap();
        let refusals = frame.refusals.clone();
        (begun, frame.ops.into_iter().map(|(_, op)| op).collect(), refusals)
    }

    /// A menu UI quad from a native producer in a replaying frame, offered
    /// to a sink over a backend that queues as the threaded one does:
    /// `carry` begins the native carry first, as the device does. What the
    /// sink answered, what its backend queued and the replay's carried
    /// stages.
    fn queued_ui_quad(carry: bool) -> (bool, Vec<(Stage, usize)>, Vec<Option<Stage>>) {
        let draw = Arc::new(guest_draw(0xFF40_8020));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut inner = RecordingBackend::new();
        inner.queue_external_draws = true;
        let mut sink = SceneSink::new(inner, slot).with_replay(move |frame| output.lock().unwrap().push(frame));
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
        let surfaces = ExternalSurfaces { color: colour, depth: colour, sampled: vec![], slots: vec![], slot_sizes: vec![] };
        let request = ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 };
        let plan = ExternalPlan { stage: Stage::Ui, view: 4, ranges: 1, slots: vec![] };
        let pass: d3d::backend::SharedReplacement = Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>));
        let admission = RetainedAdmission {
            packets: Arc::from([Arc::clone(&draw)]),
            surfaces: surfaces.clone(),
            request,
            plan: plan.clone(),
            pass: Arc::clone(&pass),
        };
        let begun = carry && sink.begin_native_carry(colour, None, &admission);
        let queued = sink.queue_external_draw(
            d3d::backend::QueuedExternalDraw {
                surfaces, request, plan, pass, fallback: draw, fallback_depth: None, finished: Box::new(|_| {}),
            },
            [None; 4],
        );
        if begun && queued {
            sink.record_native_carry(colour, None, [None; 4]);
        }
        sink.end_native_carry();
        assert!(sink.copy_surface(colour, shown).unwrap());
        let held = sink.inner().queued.clone();
        sink.present(shown, 0).unwrap();
        let frame = frames.lock().unwrap().pop().unwrap();
        let carried = frame.ops.into_iter().filter_map(|(_, op)| match op {
            ReplayOp::Draw { external, .. } => Some(external.map(|e| e.plan.stage)),
            _ => None,
        }).collect();
        (queued, held, carried)
    }

    /// A colour-only UI pass binds its colour as its depth (a scratch
    /// depth): its carry names no depth, whatever the guest's draw had
    /// bound, or the sink cannot match its attachments and keeps nothing,
    /// and the quad goes back to a render-thread round trip.
    #[test]
    fn a_colour_only_carry_names_no_depth() {
        let draw = Arc::new(guest_draw(0xFF40_8020));
        let mut sink = SceneSink::new(RecordingBackend::new(), slot).with_replay(|_| {});
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let admission = RetainedAdmission {
            packets: Arc::from([draw]),
            surfaces: ExternalSurfaces { color: colour, depth: colour, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage: Stage::Ui, view: 4, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        assert!(!sink.begin_native_carry(colour, Some(depth), &admission), "the bound depth is not the pass's");
        sink.end_native_carry();
        assert!(sink.begin_native_carry(colour, None, &admission));
        sink.end_native_carry();
    }

    /// The game thread hands a menu quad to the render thread and moves
    /// on: with its carry begun, a replaying sink queues it and keeps the
    /// carried quad in its slot, so the device need not prepare it in a
    /// round trip (`ThreadedBackend::prepare_external` blocks for one).
    #[test]
    fn a_ui_quad_in_a_replay_is_queued_with_its_carry() {
        let (queued, held, carried) = queued_ui_quad(true);
        assert!(queued);
        assert_eq!(held, [(Stage::Ui, 1)]);
        assert_eq!(carried, [Some(Stage::Ui)]);
    }

    /// Without a carry a replaying sink would lose the quad from its frame,
    /// so it declines to queue it and the device prepares it instead.
    #[test]
    fn a_ui_quad_in_a_replay_without_a_carry_is_not_queued() {
        let (queued, held, carried) = queued_ui_quad(false);
        assert!(!queued);
        assert!(held.is_empty());
        assert!(carried.is_empty());
    }

    #[test]
    fn a_native_water_commit_is_retained_as_an_admitted_draw() {
        let (begun, ops, refusals) = native_water(Stage::Water, false);
        assert!(begun);
        let carried: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                ReplayOp::Draw { external, .. } => Some(external.as_ref().map(|e| e.plan.stage)),
                _ => None,
            })
            .collect();
        assert_eq!(carried, [Some(Stage::Water)]);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// A sorted pool's guest draw is replaced by its pass and carried as
    /// that one draw: the device begins a carry for a Water admission of a
    /// guest draw as it does for the native producer's, so a replaying sink
    /// keeps one replay draw holding the pass, not the guest's packet and a
    /// second, and refuses nothing.
    #[test]
    fn a_sorted_pools_replacement_is_carried_as_one_replay_draw() {
        let (begun, ops, refusals) = native_water(Stage::Water, false);
        assert!(begun);
        let draws: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                ReplayOp::Draw { external: Some(e), .. } => Some((e.plan.ranges, e.plan.stage)),
                ReplayOp::Draw { external: None, .. } => panic!("the carried draw lost its pass"),
                _ => None,
            })
            .collect();
        assert_eq!(draws, [(1, Stage::Water)]);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    #[test]
    fn water_flares_and_ui_commits_are_each_retained_as_an_admitted_draw() {
        for stage in [Stage::Water, Stage::Flares, Stage::Ui] {
            let (begun, ops, refusals) = native_water(stage, false);
            assert!(begun, "{stage:?}");
            let carried: Vec<_> = ops
                .iter()
                .filter_map(|op| match op {
                    ReplayOp::Draw { external, .. } => Some(external.as_ref().map(|e| (e.plan.stage, e.at, e.packets.len()))),
                    _ => None,
                })
                .collect();
            assert_eq!(carried, [Some((stage, 0, 1))], "{stage:?}");
            assert!(refusals.is_empty(), "{stage:?}: {refusals:?}");
        }

        // A UI batch with siblings: one replay draw per packet, in order,
        // each the packet it holds, all under the one admission.
        let (begun, ops, refusals) = native_carry(Stage::Ui, false, batch(2));
        assert!(begun);
        let held: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                ReplayOp::Draw { draw, external: Some(e), .. } => Some((draw, e)),
                ReplayOp::Draw { external: None, .. } => panic!("a batch draw lost its admission"),
                _ => None,
            })
            .collect();
        assert_eq!(held.len(), 3);
        for (at, (draw, external)) in held.iter().enumerate() {
            assert_eq!((external.plan.stage, external.at, external.packets.len()), (Stage::Ui, at, 3));
            assert!(Arc::ptr_eq(draw, external.packet()), "draw {at} holds its own packet");
            assert_eq!(external.follows(), at > 0);
            assert!(Arc::ptr_eq(&external.pass, &held[0].1.pass), "one pass for the run");
            assert!(Arc::ptr_eq(&external.packets, &held[0].1.packets));
        }
        assert_eq!(held[1].0.vertex_count, held[0].0.vertex_count + 1);
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    #[test]
    fn a_flare_that_writes_depth_still_declines() {
        let (begun, ops, _) = native_carry(Stage::Flares, false, |p| write_depth(&mut p[0]));
        assert!(!begun);
        assert!(!ops.iter().any(|op| matches!(op, ReplayOp::Draw { .. })));
        // Any packet of a run that writes depth declines the run.
        let (begun, _, _) = native_carry(Stage::Flares, false, |p| {
            batch(1)(p);
            write_depth(&mut p[1]);
        });
        assert!(!begun);
        // A UI pass may write the depth it is bound with.
        let (begun, _, _) = native_carry(Stage::Ui, false, |p| write_depth(&mut p[0]));
        assert!(begun);
    }

    #[test]
    fn a_guest_draws_admission_for_several_packets_is_not_kept() {
        // A guest draw is one packet: an admission naming more is not exact
        // for it, and the draw is carried as its packet alone.
        let o = capture(false, |admission, draw, _| {
            admission.packets = Arc::from([Arc::new(draw.clone()), Arc::new(draw.clone())]);
        });
        assert_eq!((o.issued, o.replay_draws), (1, 1));
        assert!(o.external.is_none());
    }

    #[test]
    fn other_native_stages_and_any_guest_reference_are_not_carried() {
        // Post's glow and screen effects are carried as sealed scene
        // stages, not admitted draws; the rest are carried only as the
        // guest's own admitted draws. A transparent range is carried by
        // `a_transparent_run_replays_on_the_display_copy`.
        for (stage, reference) in [
            (Stage::Post, false),
            (Stage::Particles, false),
            (Stage::Sky, false),
            (Stage::Water, true),
            (Stage::Flares, true),
            (Stage::Ui, true),
            (Stage::Transparents, true),
        ] {
            let (begun, ops, _) = native_water(stage, reference);
            assert!(!begun, "{stage:?} reference={reference}");
            assert!(!ops.iter().any(|op| matches!(op, ReplayOp::Draw { .. })), "{stage:?}");
        }
    }

    #[test]
    fn the_sky_strip_keeps_its_stencil_write_on_replay() {
        let o = capture(false, |admission, draw, _| {
            admission.plan.stage = Stage::Sky;
            draw.target.depth_format = Some(D3DFormat::LinD24S8);
            draw.state.render.stencil.enable = true;
            draw.state.render.stencil.write_mask = 0xff;
            draw.state.render.stencil.pass = gfx::state::StencilOp::Replace;
            admission.packets = Arc::from([Arc::new(draw.clone())]);
        });
        assert!(o.submitted.writes_depth_stencil());
        assert_eq!(o.external.unwrap().plan.stage, Stage::Sky);
        assert_eq!((o.issued, o.replay_draws), (1, 1));
    }

    #[test]
    fn every_inexact_admission_still_draws_the_packet_once_with_its_own_outcome() {
        type Case = fn(&mut RetainedAdmission<u32>, &mut ResolvedDraw, [u32; 3]);
        let cases: [(&str, bool, Case); 6] = [
            ("one-shot pass", false, |a, _, _| {
                a.pass = Arc::new(Mutex::new(Box::new(Flare(false)) as Box<dyn ExternalPass>));
            }),
            ("packet differs from the submitted draw", false, |_, d, _| d.entry = "adapted"),
            ("unretained stage: world", false, |a, _, _| a.plan.stage = Stage::World),
            ("sampled levels need their own replay samples", false, |a, _, s| a.surfaces.sampled.push(s[2])),
            ("admission bound other attachments", false, |a, _, s| a.surfaces.depth = s[2]),
            ("guest reference requested", true, |_, _, _| {}),
        ];
        for (name, reference, case) in cases {
            let o = capture(reference, case);
            assert_eq!(o.result, Ok(DrawOutcome::Drawn), "{name}");
            assert_eq!(o.issued, 1, "{name}: no double issue");
            assert!(o.external.is_none(), "{name}: the admission was kept");
            assert_eq!(o.replay_draws, 1, "{name}: the packet is still carried");
        }
    }

    #[test]
    fn without_a_replay_the_hook_is_the_plain_draw() {
        fn off() -> Option<SlotAt> { None }
        let draw = guest_draw(0xFF11_2233);
        let mut sink = SceneSink::new(RecordingBackend::new(), off);
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let depth = sink.create_surface(D3DFormat::LinD24S8, 32, 32, true).unwrap();
        let admission = RetainedAdmission {
            packets: Arc::from([Arc::new(draw.clone())]),
            surfaces: ExternalSurfaces { color: colour, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage: Stage::Flares, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        let (result, _) = sink.draw_admitted(colour, Some(depth), draw.clone(), [None; 4], admission);
        assert_eq!(result, Ok(DrawOutcome::Drawn));
        assert_eq!(sink.inner().draws(), [&draw]);
        assert!(sink.last_frame().ops.is_empty());
    }

    #[test]
    fn a_default_backend_ignores_the_admission_and_draws_owned() {
        let draw = guest_draw(0xFF11_2233);
        let mut backend = RecordingBackend::new();
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = backend.create_texture_target(fmt, 32, 32).unwrap();
        let admission = RetainedAdmission {
            packets: Arc::from([Arc::new(draw.clone())]),
            surfaces: ExternalSurfaces { color: colour, depth: colour, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage: Stage::Flares, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        let (result, _) = backend.draw_admitted(colour, None, draw.clone(), [None; 4], admission);
        assert_eq!(result, Ok(DrawOutcome::Drawn));
        assert_eq!(backend.draws(), [&draw]);
    }

    #[test]
    fn replay_uses_the_admission_only_for_the_draw_and_attachments_it_was_made_for() {
        let o = capture(false, |_, _, _| {});
        let external = o.external.expect("exact");
        let draw = Arc::new(o.submitted.clone());
        let (colour, depth) = (external.surfaces.color, external.surfaces.depth);
        let targets = HashMap::from([(colour, 7u32), (depth, 8)]);

        assert!(external.resolve(&draw, &targets, (7, Some(8)), true, 1u32).is_some());
        // Same packet in another allocation, as a rewrite that did not change it leaves it.
        assert!(external.resolve(&Arc::new(o.submitted.clone()), &targets, (7, Some(8)), true, 1u32).is_some());
        // A rewrite of the draw (a camera warp, a tile) drops the admission.
        let mut rewritten = o.submitted.clone();
        rewritten.entry = "tiled";
        assert!(external.resolve(&Arc::new(rewritten), &targets, (7, Some(8)), true, 1u32).is_none());
        // Other attachments than the ones the draw is bound to.
        assert!(external.resolve(&draw, &targets, (9, Some(8)), true, 1u32).is_none());
        assert!(external.resolve(&draw, &targets, (7, Some(9)), true, 1u32).is_none());
        assert!(external.resolve(&draw, &targets, (7, None), true, 1u32).is_none());
        // A target the replay does not hold.
        assert!(external.resolve(&draw, &HashMap::from([(colour, 7u32)]), (7, Some(8)), true, 1u32).is_none());
        // A private capture is native at 1x only; a live display's backend answers for its scale.
        assert!(external.resolve(&draw, &targets, (7, Some(8)), false, 4u32).is_none());
        assert!(external.resolve(&draw, &targets, (7, Some(8)), false, 1u32).is_some());
        assert!(external.resolve(&draw, &targets, (7, Some(8)), true, 4u32).is_some());
    }

    #[test]
    fn a_colour_only_draw_binds_its_colour_as_depth() {
        let draw = guest_draw(0xFF11_2233);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut sink = SceneSink::new(RecordingBackend::new(), slot).with_replay(move |f| output.lock().unwrap().push(f));
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
        let admission = RetainedAdmission {
            packets: Arc::from([Arc::new(draw.clone())]),
            surfaces: ExternalSurfaces { color: colour, depth: colour, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage: Stage::Flares, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        sink.draw_admitted(colour, None, draw, [None; 4], admission).0.unwrap();
        assert!(sink.copy_surface(colour, shown).unwrap());
        sink.present(shown, 0).unwrap();
        let frame = frames.lock().unwrap().pop().unwrap();
        let external = frame.ops.iter().find_map(|(_, op)| match op {
            ReplayOp::Draw { external, .. } => external.as_deref(),
            _ => None,
        }).expect("retained");
        assert_eq!(external.surfaces.color, external.surfaces.depth);
        let draw = external.packet().clone();
        let targets = HashMap::from([(external.surfaces.color, 3u32)]);
        assert!(external.resolve(&draw, &targets, (3, None), true, 1u32).is_some());
        assert!(external.resolve(&draw, &targets, (3, Some(4)), true, 1u32).is_none());
    }

    #[test]
    fn the_draws_own_texture_inputs_stay_with_it() {
        use gfx::texture::{TextureDesc, TextureKind, decode_texture};
        let draw = guest_draw(0xFF11_2233);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut sink = SceneSink::new(RecordingBackend::new(), slot).with_replay(move |f| output.lock().unwrap().push(f));
        let fmt = D3DFormat::LinA8R8G8B8;
        let colour = sink.create_texture_target(fmt, 32, 32).unwrap();
        let shown = sink.create_surface(fmt, 32, 32, false).unwrap();
        let desc = TextureDesc::new(fmt, TextureKind::D2, 1, 1, 1, 1);
        let decoded = decode_texture(&desc, &[1, 2, 3, 4], None, Default::default()).unwrap();
        let image = sink.upload_texture(&decoded).unwrap();
        let admission = RetainedAdmission {
            packets: Arc::from([Arc::new(draw.clone())]),
            surfaces: ExternalSurfaces { color: colour, depth: colour, sampled: vec![], slots: vec![], slot_sizes: vec![] },
            request: ExternalPassRequest { rect: (0, 0, 32, 32), surface_size: (32, 32), z_scale: 1.0, display_projection_factor: 1.0 },
            plan: ExternalPlan { stage: Stage::Flares, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(Flare(true)) as Box<dyn ExternalPass>)),
        };
        sink.draw_admitted(colour, None, draw, [Some(&image), None, None, None], admission).0.unwrap();
        assert!(sink.copy_surface(colour, shown).unwrap());
        sink.present(shown, 0).unwrap();
        let frame = frames.lock().unwrap().pop().unwrap();
        let (textures, external) = frame.ops.iter().find_map(|(_, op)| match op {
            ReplayOp::Draw { textures, external, .. } => Some((textures, external)),
            _ => None,
        }).unwrap();
        assert!(external.is_some());
        assert!(matches!(&textures[0], Some(ReplayTexture::Upload(a)) if **a == decoded));
    }
}

/// An objects run reserved as a scene stage in a kept scene (docs/graphics/
/// framescene.md, slice 2): `ObjectsPass` replays on the scene
/// (`replays_scene`), so a replaying sink takes the reservation and keeps it.
#[cfg(feature = "gpu")]
mod kept_objects {
    use std::sync::{Arc, Mutex};

    use super::*;
    use d3d::scene_sink::ReplayOp;
    use d3d::{ExternalPass, ExternalPlan, ThreadedBackend};
    use timeline::native::Stage;

    /// A pass that owns what it records from, as `ObjectsPass` does.
    struct Owned;

    impl ExternalPass for Owned {
        fn replays_scene(&self) -> bool { true }
    }

    #[test]
    fn an_objects_stage_reserved_in_a_kept_scene_commits_and_replays() {
        let captured = Arc::new(Mutex::new(None));
        let out = Arc::clone(&captured);
        let mut recorder = RecordingBackend::new();
        recorder.external_passes = true;
        let sink = SceneSink::new(ThreadedBackend::new(recorder).unwrap(), || {
            Some(SlotAt { slot: Slot::Opaque, view: 0 })
        })
        .with_replay(move |frame| *out.lock().unwrap() = Some(frame));
        let (mut d, mut m) = create(sink, UploadPolicy::Always);
        set_states(&mut d, &mut m);
        quad(&mut d, &mut m, 0xff00_ff00);
        assert!(d.begin_external_boundary(&mut m).unwrap());
        let plan = ExternalPlan { stage: Stage::Objects, view: 0, ranges: 2, slots: Vec::new() };
        d.reserve_scene_stage(&mut m, plan, Box::new(Owned))
            .unwrap()
            .expect("a replaying sink takes a pass that replays")
            .commit();
        d.swap(&mut m, 0).unwrap();
        let guard = captured.lock().unwrap();
        let frame = guard.as_ref().expect("the sink captured the swap");
        let stages: Vec<_> = frame
            .ops
            .iter()
            .filter_map(|(_, op)| match op {
                ReplayOp::Stage(stage) => Some(stage.plan.stage),
                _ => None,
            })
            .collect();
        assert_eq!(stages, [Stage::Objects]);
        assert!(frame.refusals.is_empty(), "{:?}", frame.refusals);
    }
}
