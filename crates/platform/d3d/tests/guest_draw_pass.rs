//! A real GPU copy and sampled guest draw in a private frame. All shaders,
//! streams and render state come from the synthetic fixed-function rig.

use crate::common::*;
use crate::render_scale::textured_quad;
use d3d::gfx_backend::GuestDrawPass;
use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
use d3d::{GfxBackend, RecordingBackend, UploadPolicy};
use frame_scene::carried::CameraUse;
use frame_scene::{AuxTarget, TargetRef};
use gfx::{Clear, D3DFormat, GfxContext};

fn colour() -> AuxTarget {
    AuxTarget { width: W, height: H, format: D3DFormat::LinA8R8G8B8 as u32, depth: false }
}

fn clear(colour: TargetRef, rgba: [f32; 4]) -> ReplayOp {
    ReplayOp::Clear {
        colour,
        depth: TargetRef::None,
        clear: Clear { color: Some(rgba), ..Clear::NONE },
    }
}

#[test]
fn a_carried_copy_samples_scene_colour_as_it_was_before_the_copy() {
    let (mut recording, mut memory) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut recording, &mut memory);
    texture(&mut memory, 0x7f);
    recording.set_texture(&mut memory, 0, TEX_HEADER).unwrap();
    // Draw only on the left. The right side must retain the later clear.
    textured_quad(&mut recording, &mut memory, (0., 0., W as f32 / 2., H as f32));
    let draw = (*recording.backend.draws()[0]).clone();
    assert!(draw.texture_info[0].is_some());
    let scene = TargetRef::SceneColour;
    let aux = TargetRef::Aux(0);
    let frame = ReplayFrame {
        aux: vec![colour()],
        colour: Some(colour()),
        presented: scene,
        ..Default::default()
    };
    let ops = [
        clear(scene, [1., 0., 0., 1.]),
        ReplayOp::Copy { from: scene, to: aux },
        clear(scene, [0., 0., 1., 1.]),
        ReplayOp::Sample { of: aux, id: 0, how: ReplaySample::InPlace },
        ReplayOp::Bind { colour: scene, depth: TargetRef::None },
        ReplayOp::Draw {
            draw: Box::new(draw),
            textures: [Some(ReplayTexture::Sample(0)), None, None, None],
            camera: CameraUse::Screen,
        },
    ];
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
        for op in &ops {
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
    let frame = ReplayFrame {
        colour: Some(colour()),
        presented: scene,
        ..Default::default()
    };
    for _ in 0..2 {
        let mut display = GuestDrawPass::new_display(&mut gpu, &frame, 1).unwrap();
        display.apply(&clear(scene, [0., 1., 0., 1.])).unwrap();
        display.present(4, None).unwrap();
    }
    let shown = gpu.last_presented().unwrap();
    let mut private = GuestDrawPass::new(&mut gpu, &frame, 1).unwrap();
    private.apply(&clear(scene, [1., 0., 0., 1.])).unwrap();
    let (_, rgba) = private.read_rgba().unwrap();
    assert_eq!(&rgba[..4], [255, 0, 0, 255]);
    assert_eq!(gpu.last_presented(), Some(shown));
    assert_eq!(&gpu.read_color(shown).unwrap().0[..4], [0, 255, 0, 255]);
    assert_eq!(gpu.batch_stats().swap_waits, 0);
    assert_eq!(*flags.lock().unwrap(), [4, 4]);
}
