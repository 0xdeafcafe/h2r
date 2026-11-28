//! A real GPU copy and sampled guest draw in a private frame. All shaders,
//! streams and render state come from the synthetic fixed-function rig.

use crate::common::*;
use crate::render_scale::textured_quad;
use d3d::gfx_backend::GuestDrawPass;
use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
use d3d::{GfxBackend, RecordingBackend, UploadPolicy};
use frame_scene::carried::CameraUse;
use frame_scene::{AuxTarget, Slot, SlotAt, TargetRef};
use gfx::{Clear, D3DFormat, GfxContext};

fn colour() -> AuxTarget {
    AuxTarget {
        width: W,
        height: H,
        format: D3DFormat::LinA8R8G8B8 as u32,
        depth: false,
    }
}

/// Offered a guest draw, keeps its resolved packet and refuses it.
#[derive(Default)]
struct Capture(std::sync::Mutex<Option<d3d::ResolvedDraw>>);

impl d3d::DrawAdmission for Capture {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        *self.0.lock().unwrap() = Some(snapshot.draw.clone());
        None
    }
}

/// The resolved packet of the one textured quad the guest draws on the
/// left half of the frame.
fn left_quad(device: &mut d3d::Device<RecordingBackend>, memory: &mut d3d::SparseMemory) -> d3d::ResolvedDraw {
    let capture = std::sync::Arc::new(Capture::default());
    device.add_draw_admission(capture.clone());
    textured_quad(device, memory, (0., 0., W as f32 / 2., H as f32));
    capture.0.lock().unwrap().take().expect("the draw was offered to the admission")
}

fn clear(colour: TargetRef, rgba: [f32; 4]) -> ReplayOp {
    ReplayOp::Clear {
        colour,
        depth: TargetRef::None,
        clear: Clear {
            color: Some(rgba),
            ..Clear::NONE
        },
    }
}

#[test]
fn a_carried_copy_samples_scene_colour_as_it_was_before_the_copy() {
    let (mut recording, mut memory) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut recording, &mut memory);
    texture(&mut memory, 0x7f);
    recording.set_texture(&mut memory, 0, TEX_HEADER).unwrap();
    // Draw only on the left. The right side must retain the later clear.
    let draw = left_quad(&mut recording, &mut memory);
    assert!(draw.texture_info[0].is_some());
    let scene = TargetRef::SceneColour;
    let aux = TargetRef::Aux(0);
    let mut frame = ReplayFrame {
        aux: vec![colour()],
        colour: Some(colour()),
        presented: scene,
        ..Default::default()
    };
    let ops = [
        clear(scene, [1., 0., 0., 1.]),
        ReplayOp::Copy { from: scene, to: aux },
        clear(scene, [0., 0., 1., 1.]),
        ReplayOp::Sample {
            of: aux,
            id: 0,
            how: ReplaySample::InPlace,
            into: None,
        },
        ReplayOp::Bind {
            colour: scene,
            depth: TargetRef::None,
        },
        ReplayOp::Draw {
            draw: draw.into(),
            textures: [Some(ReplayTexture::Sample(0)), None, None, None],
            camera: CameraUse::Screen,
            external: None,
        },
    ];
    frame.ops = ops
        .into_iter()
        .map(|op| {
            (
                SlotAt {
                    slot: Slot::Opaque,
                    view: 0,
                },
                op,
            )
        })
        .collect();
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut gpu = GfxBackend::new(ctx);
    for scale in [1, 4] {
        let mut pass = GuestDrawPass::new(&mut gpu, &frame, scale).unwrap();
        for (_, op) in &frame.ops {
            pass.apply(op).unwrap();
        }
        let ([w, h], pixels) = pass.read_rgba().unwrap();
        assert_eq!((w, h), (W * scale, H * scale));
        let pixel = |x, y| {
            let i = ((y * w + x) * 4) as usize;
            &pixels[i..i + 4]
        };
        assert_eq!(pixel(w / 4, h / 2), [255, 0, 0, 255]);
        assert_eq!(pixel(3 * w / 4, h / 2), [0, 0, 255, 255]);
        assert!(gpu.last_presented().is_none());
    }
    assert!(pollster::block_on(validation.pop()).is_none());
}

#[test]
fn a_private_capture_keeps_the_window_target_and_swap_finish_does_not_wait() {
    use std::sync::{Arc, Mutex};
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let flags = Arc::new(Mutex::new(Vec::new()));
    let sent = flags.clone();
    let mut gpu = GfxBackend::new(ctx).on_present(move |_, _, _, flags, _| sent.lock().unwrap().push(flags));
    let scene = TargetRef::SceneColour;
    let mut frame = ReplayFrame {
        colour: Some(colour()),
        presented: scene,
        ..Default::default()
    };
    let at = SlotAt {
        slot: Slot::Opaque,
        view: 0,
    };
    frame.ops.push((at, clear(scene, [0., 1., 0., 1.])));
    for _ in 0..2 {
        let mut display = GuestDrawPass::new_display(&mut gpu, &frame, 1).unwrap();
        display.apply(&frame.ops[0].1).unwrap();
        display.present(4, None).unwrap();
    }
    let shown = gpu.last_presented().unwrap();
    frame.ops[0].1 = clear(scene, [1., 0., 0., 1.]);
    let mut private = GuestDrawPass::new(&mut gpu, &frame, 1).unwrap();
    private.apply(&frame.ops[0].1).unwrap();
    let (_, rgba) = private.read_rgba().unwrap();
    assert_eq!(&rgba[..4], [255, 0, 0, 255]);
    assert_eq!(gpu.last_presented(), Some(shown));
    assert_eq!(&gpu.read_color(shown).unwrap().0[..4], [0, 255, 0, 255]);
    assert_eq!(gpu.batch_stats().swap_waits, 0);
    assert_eq!(*flags.lock().unwrap(), [4, 4]);
}

#[test]
fn an_in_place_frame_retains_pixels_outside_its_scissor_and_releases_no_live_surface() {
    use d3d::{Backend, ChannelMask, PixelRect, RectClear};
    use std::collections::HashMap;
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let mut gpu = GfxBackend::new(ctx);
    let info = colour();
    let id = gpu
        .create_surface(D3DFormat::LinA8R8G8B8, info.width, info.height, false)
        .unwrap();
    gpu.clear(
        id,
        None,
        Clear {
            color: Some([0., 0., 1., 1.]),
            ..Clear::NONE
        },
    )
    .unwrap();
    let scene = TargetRef::SceneColour;
    let frame = ReplayFrame {
        ops: vec![(
            SlotAt {
                slot: Slot::Opaque,
                view: 0,
            },
            ReplayOp::ClearRects {
                colour: scene,
                depth: TargetRef::None,
                clear: RectClear {
                    rects: vec![PixelRect {
                        x: 1,
                        y: 1,
                        width: 3,
                        height: 3,
                    }],
                    color: Some((
                        [1., 0., 0., 1.],
                        ChannelMask {
                            red: true,
                            green: true,
                            blue: true,
                            alpha: true,
                        },
                    )),
                    depth: None,
                    stencil: None,
                },
            },
        )],
        colour: Some(info),
        presented: scene,
        ..Default::default()
    };
    assert!(
        GuestDrawPass::new(&mut gpu, &frame, 1).is_err(),
        "needs prior scene colour"
    );
    let targets = HashMap::from([(scene, id)]);
    let raster = gpu.measure_present_raster(Some(id));
    let mut pass = GuestDrawPass::on_existing(&mut gpu, &frame, &targets).unwrap();
    pass.apply(&frame.ops[0].1).unwrap();
    let (_, pixels) = pass.read_target(scene).unwrap();
    assert_eq!(pixel(&pixels, 20, 20), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 2, 2), [255, 0, 0, 255]);
    pass.present(4, None).unwrap();
    assert_eq!(raster.finish(gpu.take_presented_extent()).physical, Some(1));
    assert_eq!(gpu.last_presented(), Some(id));
    assert_eq!(gpu.surface_extent(id), Some((W, H)));
    gpu.destroy_surface(id).unwrap();
}

#[test]
fn display_scale_measures_each_owned_or_ordinary_guest_swap() {
    use d3d::Backend;
    use d3d::scaled::ScaledScenePass;
    use d3d::scene_sink::window::PresentScale;
    use std::collections::HashMap;

    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let reference = GfxBackend::new(GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    });
    let mut gpu = d3d::ScaledBackend::with_single_copy(reference, GfxBackend::new(ctx), 4, true).unwrap();
    let back = gpu.create_surface(D3DFormat::LinA8R8G8B8, W, H, false).unwrap();
    let small = gpu.create_surface(D3DFormat::LinA8R8G8B8, W / 2, H / 2, false).unwrap();
    let scene = TargetRef::SceneColour;
    let frame = ReplayFrame {
        colour: Some(colour()),
        presented: scene,
        ops: vec![(SlotAt { slot: Slot::Opaque, view: 0 }, clear(scene, [1., 0., 0., 1.]))],
        ..Default::default()
    };
    let targets = HashMap::from([(scene, back)]);
    let expected = |configured, physical| PresentScale { configured: Some(configured), physical };

    // A menu-sized side target is displayed at 1x in this otherwise 4x run.
    let measure = gpu.measure_present_raster(Some(small));
    gpu.present(small, 0).unwrap();
    assert_eq!(measure.finish(gpu.take_presented_extent()), expected(4, Some(1)));

    // An in-between image alone must leave an armed original Swap unknown.
    let measure = gpu.measure_present_raster(Some(back));
    gpu.present_replay(back, None).unwrap();
    assert_eq!(measure.finish(gpu.take_presented_extent()), expected(4, None));

    // The setting changes *after* this owned display's 4x draw.
    gpu.follow_scale(|| 1);
    let measure = gpu.measure_present_raster(Some(back));
    let mut pass = ScaledScenePass::new(&mut gpu, &frame, &targets).unwrap();
    pass.apply(&frame.ops[0].1).unwrap();
    pass.present(0, None).unwrap();
    assert_eq!(measure.finish(gpu.take_presented_extent()), expected(4, Some(4)));

    let measure = gpu.measure_present_raster(Some(back));
    gpu.present(back, 0).unwrap();
    assert_eq!(measure.finish(gpu.take_presented_extent()), expected(1, Some(1)));
}

#[test]
fn an_earlier_snapshot_survives_guest_release_and_replays_after_either_present_path() {
    use d3d::threaded::owner::{Candidate, FrameOwner, Submission};
    use d3d::{Backend, SceneSink, ThreadedBackend};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    let Ok(ctx) = GfxContext::headless() else { return };
    let (mut device, mut memory) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut device, &mut memory);
    texture(&mut memory, 0x7f);
    device.set_texture(&mut memory, 0, TEX_HEADER).unwrap();
    let draw = left_quad(&mut device, &mut memory);

    for adopt_first in [false, true] {
        let shown = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&shown);
        let latest = Arc::new(Mutex::new(None::<Arc<ReplayFrame>>));
        let select = Arc::clone(&latest);
        let original = ThreadedBackend::new(GfxBackend::new(ctx.clone())).unwrap()
            .with_frame_owner(FrameOwner {
                select: Box::new(move |n, _| {
                    if n == 1 && !adopt_first { return None; }
                    let frame = select.lock().unwrap().clone()?;
                    let draw = Arc::clone(&frame);
                    let count = Arc::clone(&count);
                    Some(Submission::Native(Candidate {
                        frame,
                        before_original: None,
                        eligible: Box::new(|_, _| true),
                        draw: Box::new(move |gpu: &mut GfxBackend, targets| {
                            let mut pass = GuestDrawPass::on_existing(gpu, &draw, targets)?;
                            for (_, op) in &draw.ops { pass.apply(op)?; }
                            let result = pass.present_with_outputs(0, None)?;
                            count.fetch_add(1, Ordering::Relaxed);
                            Ok(result)
                        }),
                    }))
                }),
                events: std::sync::Arc::new(()),
            });
        let keep = Arc::clone(&latest);
        let mut sink = SceneSink::new(original, || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
            .with_replay(move |frame| *keep.lock().unwrap() = Some(Arc::new(frame)));
        let target = sink.create_texture_target(D3DFormat::LinA8R8G8B8, W, H).unwrap();
        sink.clear(target, None, Clear { color: Some([1., 0., 0., 1.]), ..Clear::NONE }).unwrap();
        let red = sink.snapshot_surface(target, true).unwrap().unwrap();
        sink.present(target, 0).unwrap();

        sink.clear(target, None, Clear { color: Some([0., 0., 1., 1.]), ..Clear::NONE }).unwrap();
        let blue = sink.snapshot_surface(target, true).unwrap().unwrap(); // same frame-local sample id
        sink.draw(target, None, &draw, [Some(&red), None, None, None]).unwrap();
        drop((red, blue));
        sink.present(target, 0).unwrap();
        assert_eq!(sink.inner().waits().reads, 0);
        assert_eq!(sink.inner_mut().with_inner(|gpu| gpu.readbacks()).unwrap(), 0);
        assert_eq!(shown.load(Ordering::Relaxed), if adopt_first { 2 } else { 1 });
        sink.destroy_surface(target).unwrap();
        drop(sink);

        let frame = latest.lock().unwrap().take().unwrap();
        assert!(frame.refusals.is_empty());
        assert!(frame.ops.iter().any(|(_, op)| matches!(op, ReplayOp::Draw { textures, .. }
            if matches!(textures[0], Some(ReplayTexture::Frozen(_))))));
        let mut private_gpu = GfxBackend::new(ctx.clone());
        let mut pass = GuestDrawPass::new(&mut private_gpu, &frame, 1).unwrap();
        for (_, op) in &frame.ops { pass.apply(op).unwrap(); }
        let ([width, height], pixels) = pass.read_rgba().unwrap();
        let pixel = |x| {
            let i = ((height / 2 * width + x) * 4) as usize;
            &pixels[i..i + 4]
        };
        assert_eq!(pixel(width / 4), [255, 0, 0, 255]);
        assert_eq!(pixel(3 * width / 4), [0, 0, 255, 255]);
    }
}
