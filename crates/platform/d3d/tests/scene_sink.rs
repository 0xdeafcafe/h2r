//! The scene sink (docs/graphics/framescene.md, slice 6) on the device's
//! draw rig: it carries the device's draws in the order the backend gets
//! them, each at the slot that was current, names the presented surface
//! the scene's colour, and passes every draw on to the backend under it.
//! Synthetic device over the recording backend; no GPU.

use std::sync::atomic::{AtomicU16, Ordering};

use crate::common::*;
use d3d::{Device, RecordingBackend, ResolvedDraw, SceneSink, SparseMemory, UploadPolicy};
use frame_scene::{GuestOp, Slot, SlotAt, TargetRef};

/// This file's slot: `u16::MAX` off, else `slot << 8 | view`.
static AT: AtomicU16 = AtomicU16::new(u16::MAX);

fn source() -> Option<SlotAt> {
    let v = AT.load(Ordering::Relaxed);
    (v != u16::MAX).then(|| SlotAt { slot: Slot::ALL[usize::from(v >> 8)], view: v as u8 })
}

fn at(slot: Slot, view: u8) {
    AT.store(((slot.index() as u16) << 8) | u16::from(view), Ordering::Relaxed);
}

fn quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    quad_vb(m, 0.0, 0.0, 32.0, 32.0, color);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

#[test]
fn the_sink_carries_the_devices_draws_in_order_by_slot_and_the_backend_still_draws_them() {
    let sink = SceneSink::new(RecordingBackend::new(), source).keeping_draws();
    let (mut d, mut m) = create(sink, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    // Frames until every buffer of the swap chain has been presented once.
    at(Slot::Outside, 0);
    for _ in 0..3 {
        quad(&mut d, &mut m, 0xFF00_0000);
        d.swap(&mut m, 0).unwrap();
    }
    at(Slot::Opaque, 1);
    quad(&mut d, &mut m, 0xFFFF_0000);
    quad(&mut d, &mut m, 0xFF00_FF00);
    at(Slot::Transparents, 1);
    quad(&mut d, &mut m, 0xFF00_00FF);
    at(Slot::Overlay, 1);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    d.swap(&mut m, 0).unwrap();

    let frame = d.backend.last_frame();
    let carried: Vec<(Slot, &ResolvedDraw)> = frame
        .ops
        .iter()
        .filter_map(|o| match &o.op {
            GuestOp::Draw(Some(draw)) => Some((o.at.slot, &**draw)),
            _ => None,
        })
        .collect();
    assert_eq!(carried.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [
        Slot::Opaque,
        Slot::Opaque,
        Slot::Transparents,
        Slot::Overlay
    ]);
    let recorded = d.backend.inner().draws();
    assert!(recorded.len() >= carried.len());
    let tail = &recorded[recorded.len() - carried.len()..];
    assert!(
        carried.iter().map(|(_, draw)| *draw).eq(tail.iter().copied()),
        "the carried draws are the backend's, in its order"
    );
    assert!(frame.ops.iter().all(|o| o.at.view == 1));
    // One bind per slot: the attachments do not change, the slot does.
    let binds: Vec<(Slot, TargetRef)> = frame
        .ops
        .iter()
        .filter_map(|o| match o.op {
            GuestOp::Bind { colour, .. } => Some((o.at.slot, colour)),
            _ => None,
        })
        .collect();
    assert_eq!(binds, [
        (Slot::Opaque, TargetRef::SceneColour),
        (Slot::Transparents, TargetRef::SceneColour),
        (Slot::Overlay, TargetRef::SceneColour)
    ]);
    assert!(frame.aux.is_empty(), "nothing but the back buffer is drawn into: {:?}", frame.aux);
    let summary = frame.summary();
    assert_eq!(summary.slots[Slot::Opaque.index()].draws, 2);
    assert_eq!(summary.views.len(), 1);
    assert!(summary.side_targets.is_empty());
    assert_eq!(summary.readback_total(), 0);
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

#[test]
fn replay_retains_the_sample_version_draw_bindings_and_distinct_copy_targets() {
    use d3d::Backend;
    use d3d::scene_sink::{ReplayOp, ReplaySample, ReplayTexture};
    use gfx::texture::{TextureDesc, TextureKind, decode_texture};
    use gfx::{Clear, D3DFormat};
    use std::sync::{Arc, Mutex};

    fn slot() -> Option<SlotAt> { Some(SlotAt { slot: Slot::Opaque, view: 0 }) }
    let (mut device, mut memory) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut device, &mut memory);
    quad(&mut device, &mut memory, 0xFF19_3264);
    let draw = device.backend.draws().pop().unwrap().clone();

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
        ReplayOp::Sample { of, id, how: ReplaySample::Snapshot { opaque: true } },
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
    assert_eq!(next.refusals, ["uncaptured_texture"]);
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
    use d3d::scene_sink::{ReplayFrame, ReplayOp};
    use d3d::threaded::owner::{Candidate, FrameOwner, Outputs};
    use std::sync::{Arc, Mutex};

    let (mut device, mut memory) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut device, &mut memory);
    quad(&mut device, &mut memory, 0xFF00_8080);
    let mut packet = device.backend.draws().pop().unwrap().clone();
    Arc::make_mut(&mut packet.shader.constants)[0][0] = f32::NAN;
    let packet = Arc::new(packet);

    let pending = Arc::new(Mutex::new(None::<ReplayFrame>));
    let input = Arc::clone(&pending);
    let owner = FrameOwner {
        select: Box::new(move |present| {
            let mut replay = input.lock().unwrap().take()?;
            if present == 2 {
                for (_, op) in &mut replay.ops {
                    if let ReplayOp::Draw { draw, .. } = op {
                        // A separately allocated packet is not the one queued by the device.
                        *draw = Arc::new(draw.as_ref().clone());
                    }
                }
            }
            let target = replay.presented;
            Some(Candidate {
                frame: Arc::new(replay),
                eligible: Box::new(|_, _| true),
                draw: Box::new(move |gpu: &mut RecordingBackend, targets| {
                    gpu.present(targets[&target], 0)?;
                    Ok(Outputs::default())
                }),
            })
        }),
        declined: |_| {},
        failed: |_| {},
        stopped: || {},
    };
    let device = ThreadedBackend::new(RecordingBackend::new()).unwrap().with_frame_owner(owner);
    let mut sink = SceneSink::new(device, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
        .with_replay(move |frame| *pending.lock().unwrap() = Some(frame));
    let surface = sink.create_surface(gfx::D3DFormat::LinA8R8G8B8, 32, 32, false).unwrap();
    for native in [true, false] {
        sink.draw_shared(surface, None, Arc::clone(&packet), [None; 4]).0.unwrap();
        sink.present(surface, 0).unwrap();
        let legacy_draws = sink.inner_mut().with_inner(|gpu| gpu.draws().len()).unwrap();
        assert_eq!(legacy_draws, usize::from(!native));
    }
}
