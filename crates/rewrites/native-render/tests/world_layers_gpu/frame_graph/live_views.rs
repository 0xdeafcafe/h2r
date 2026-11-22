//! A retained in-place view follows this replay's writes at each draw.

use super::*;
use d3d::scene_sink::{FrozenSample, ReplayTexture};
use d3d::{Backend, ScaledBackend};
use gfx::state::{CompareFunc, DepthBufferMode};
use native_render::frame_graph::{
    draw_display_on, draw_scaled_display_on, freeze_display_inputs, freeze_scaled_inputs,
};
use std::collections::HashMap;

const AUX: TargetRef = TargetRef::Aux(0);

fn live_frame(binding: &FrozenSample) -> ReplayFrame {
    let mut frame = replay();
    frame.aux.push(frame.colour.unwrap());
    op(&mut frame, Slot::Opaque, clear());
    op(&mut frame, Slot::Opaque, bind());
    for (x, colour) in [(2., [0., 1., 0., 1.]), (18., [1., 1., 0., 1.])] {
        op(
            &mut frame,
            Slot::Opaque,
            ReplayOp::Clear {
                colour: AUX,
                depth: TargetRef::None,
                clear: Clear {
                    color: Some(colour),
                    ..Clear::NONE
                },
            },
        );
        let mut draw = triangle(x, [1.; 4], 0, true);
        draw.state.render.depth_mode = DepthBufferMode::Disabled;
        draw.state.render.depth_func = CompareFunc::Always;
        draw.state.render.stencil.enable = false;
        op(
            &mut frame,
            Slot::Opaque,
            ReplayOp::Draw {
                draw: Arc::new(draw),
                textures: [
                    Some(ReplayTexture::Live {
                        target: AUX,
                        binding: binding.clone(),
                    }),
                    None,
                    None,
                    None,
                ],
                camera: CameraUse::Unknown,
            },
        );
    }
    frame
}

fn check_draws(ctx: &GfxContext, frame: &ReplayFrame, scale: u32) {
    let mut private = GfxBackend::new(ctx.clone());
    for lighting in [Lighting::Retail, Lighting::E3] {
        let capture = draw_private(&mut private, &sealed(), frame, scale, lighting).unwrap();
        assert_eq!(rgba(&capture, scale * 4, scale * 4), [0, 255, 0, 255]);
        assert_eq!(rgba(&capture, scale * 20, scale * 4), [255, 255, 0, 255]);
        assert!(private.last_presented().is_none());
    }
}

#[test]
fn a_live_auxiliary_follows_replay_writes_and_rejects_a_reused_mapping() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(error) => {
            eprintln!("skipping live view GPU test: {error}");
            return;
        }
    };
    let mut gpu = GfxBackend::new(ctx.clone());
    let main = gpu.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let z = gpu.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let aux = gpu.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    let replacement = gpu.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    let view = gpu.surface_texture(aux).unwrap().unwrap();
    gpu.present(main, 0).unwrap();
    let binding = gpu.retain_scene_sample(&view).unwrap();

    // The draw clears every input itself. Replacing the mapped source
    // with an identically sized allocation must still refuse private replay.
    let wrong = live_frame(&binding);
    let stale = HashMap::from([(MAIN, main), (Z, z), (AUX, replacement)]);
    assert!(!gpu.matches_scene_targets(&wrong, &stale));
    assert!(freeze_display_inputs(&mut gpu, &wrong, &stale).is_err());
    assert!(!wrong.live_bindings_validated());
    assert!(draw_private(&mut gpu, &sealed(), &wrong, 1, Lighting::Retail).is_err());

    let frame = live_frame(&binding);
    let actual = HashMap::from([(MAIN, main), (Z, z), (AUX, aux)]);
    draw_display_on(
        &mut gpu,
        &sealed(),
        &frame,
        Lighting::Retail,
        0,
        None,
        None,
        &actual,
    )
    .unwrap();
    assert!(frame.live_bindings_validated());
    gpu.clear(
        aux,
        None,
        Clear {
            color: Some([1., 0., 1., 1.]),
            ..Clear::NONE
        },
    )
    .unwrap();
    gpu.flush().unwrap();
    gpu.destroy_surface(aux).unwrap();
    drop((binding, view));
    check_draws(&ctx, &frame, 1);
}

#[test]
fn scaled_live_sources_match_the_actual_display_projection_before_replay() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(error) => {
            eprintln!("skipping live view GPU test: {error}");
            return;
        }
    };
    for scale in [1, 4] {
        let mut gpu = ScaledBackend::new(
            GfxBackend::new(ctx.clone()),
            GfxBackend::new(ctx.clone()),
            scale,
        )
        .unwrap();
        let main = gpu.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
        let z = gpu.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
        let aux = gpu.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
        let other = gpu.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
        let source = gpu.surface_texture(aux).unwrap().unwrap();
        gpu.present(main, 0).unwrap();
        let binding = gpu.retain_scene_sample(&source).unwrap();
        let stale = HashMap::from([(MAIN, main), (Z, z), (AUX, other)]);
        let wrong = live_frame(&binding);
        assert!(!gpu.can_draw_scene(&wrong, &stale));
        assert!(freeze_scaled_inputs(&mut gpu, &wrong, &stale).is_err());
        assert!(!wrong.live_bindings_validated());

        let targets = HashMap::from([(MAIN, main), (Z, z), (AUX, aux)]);
        let frame = live_frame(&binding);
        assert!(gpu.can_draw_scene(&frame, &targets));
        draw_scaled_display_on(&mut gpu, &sealed(), &frame, None, &targets).unwrap();
        gpu.clear(
            aux,
            None,
            Clear {
                color: Some([1., 0., 1., 1.]),
                ..Clear::NONE
            },
        )
        .unwrap();
        gpu.present(main, 0).unwrap();
        check_draws(&ctx, &frame, scale);

        if scale == 1 {
            gpu.set_lighting(Lighting::E3);
            let unrelated = gpu.surface_texture(other).unwrap().unwrap();
            let divergent = d3d::scaled::ScaledTexture {
                reference: source.reference,
                view: unrelated.view,
            };
            let wrong_view = live_frame(&gpu.retain_scene_sample(&divergent).unwrap());
            assert!(!gpu.can_draw_scene(&wrong_view, &targets));
            assert!(freeze_scaled_inputs(&mut gpu, &wrong_view, &targets).is_err());
            assert!(!wrong_view.live_bindings_validated());
        }
    }
}
