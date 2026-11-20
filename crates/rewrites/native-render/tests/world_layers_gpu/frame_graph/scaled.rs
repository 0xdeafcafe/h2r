//! The frame borrows the authoritative render-scale image. The next
//! guest view must see its contents, reports and sampled versions.

use super::*;
use d3d::{Backend, GuestLayoutKind, ScaledBackend};
use native_render::frame_graph::draw_display_on;
use std::collections::HashMap;

fn scaled(ctx: &GfxContext, single: bool) -> ScaledBackend<GfxBackend, GfxBackend> {
    let backend = || {
        GfxBackend::new(GfxContext {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
            caps: ctx.caps,
        })
    };
    ScaledBackend::with_single_copy(backend(), backend(), 4, single).unwrap()
}

#[test]
fn four_x_present_hands_the_menu_its_pixels_snapshot_and_guest_count() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    if !ctx.caps.depth32_stencil8 {
        return;
    }
    let mut backend = scaled(&ctx, true);
    backend.set_lighting(Lighting::E3);
    let colour = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    backend
        .clear(colour, Some(depth), Clear::all([1.; 4]))
        .unwrap();

    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(&mut frame, Slot::Opaque, ReplayOp::BeginFlareQuery);
    let mut counted = guest_triangle(2.0, [0.0, 1.0, 0.0, 1.0], None);
    let ReplayOp::Draw { draw, .. } = &mut counted else {
        unreachable!()
    };
    draw.hw.zpass_count_enable = true;
    op(&mut frame, Slot::Opaque, counted);
    op(&mut frame, Slot::Opaque, ReplayOp::EndFlareQuery(123));
    op(
        &mut frame,
        Slot::AfterLights,
        ReplayOp::Sample {
            of: MAIN,
            id: 5,
            how: ReplaySample::Snapshot { opaque: false },
        },
    );

    let targets = HashMap::from([(MAIN, colour), (Z, depth)]);
    let attachments = backend.scene_targets(&frame, &targets).unwrap();
    let mode = backend.lighting();
    let (report, outputs) = backend
        .with_scene_view(|view| {
            draw_display_on(view, &sealed(), &frame, mode, 0, None, None, &attachments)
        })
        .unwrap();
    assert_eq!(report.lighting, Lighting::E3);
    assert_eq!(backend.reference().batch_stats().draws, 0);
    assert_eq!(
        backend.view().last_presented(),
        attachments.get(&MAIN).copied()
    );
    assert_eq!(
        backend.view().surface_extent(attachments[&MAIN]),
        Some((SIZE * 4, SIZE * 4))
    );

    // On this same Swap the menu asked for the exact 1x reference.
    backend.follow_scale(|| 1);
    backend.follow_lighting(|| Lighting::Retail);
    let mut delivered = backend.scene_presented(&frame, outputs);
    assert_eq!((backend.scale(), backend.stats().view_frames), (1, 1));
    assert_eq!(backend.stats().display_draws, 1);
    assert!(backend.scene_targets(&frame, &targets).is_none());
    let [(guest, ticket)] = delivered.tickets[..] else {
        panic!("missing flare query")
    };
    assert_eq!(guest, 123);
    let count = backend.read_pixel_count(ticket).unwrap();
    // A right triangle with twelve guest pixels on each leg. On the
    // diagonal a GPU may either include or exclude the border pixel.
    assert!((66..=78).contains(&count), "guest-grid count {count}");

    let snapshot = delivered.samples.remove(&5).unwrap();
    let snap = snapshot.view.unwrap().texture;
    assert_eq!(snap, snapshot.reference.texture);
    assert_eq!((snap.width(), snap.height()), (SIZE * 4, SIZE * 4));
    let bytes = backend
        .read_surface(
            colour,
            FORMAT,
            SIZE,
            SIZE,
            GuestLayoutKind::Linear { pitch: SIZE * 4 },
        )
        .unwrap();
    let inside = (4 * (4 * SIZE + 4)) as usize;
    assert_eq!(
        &bytes[..4],
        &[255; 4],
        "pre-existing white outside the triangle"
    );
    assert_eq!(&bytes[inside..inside + 4], &[0, 255, 0, 255]);

    // An outside/menu Swap follows the original backend path.
    backend
        .clear(colour, None, Clear::all([0., 0., 1., 1.]))
        .unwrap();
    backend.present(colour, 0).unwrap();
    let bytes = backend
        .read_surface(
            colour,
            FORMAT,
            SIZE,
            SIZE,
            GuestLayoutKind::Linear { pitch: SIZE * 4 },
        )
        .unwrap();
    assert_eq!(&bytes[inside..inside + 4], &[255, 0, 0, 255]);
    let old = gfx::readback::read_texture(
        &ctx,
        &snap,
        wgpu::TextureAspect::All,
        4,
        (SIZE * 4, SIZE * 4),
    );
    let index = (4 * (16 * SIZE * 4 + 16)) as usize;
    assert_eq!(&old[index..index + 4], &[0, 255, 0, 255]);
}

#[test]
fn four_x_mixed_attachments_and_two_copy_frames_keep_their_original_commands() {
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    for single in [false, true] {
        let mut backend = scaled(&ctx, single);
        let colour = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
        let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
        let mut targets = HashMap::from([(MAIN, colour), (Z, depth)]);
        let mut frame = replay();
        op(&mut frame, Slot::Opaque, clear());
        assert_eq!(backend.scene_targets(&frame, &targets).is_some(), single);
        let small = backend
            .create_texture_target(FORMAT, SIZE / 2, SIZE / 2)
            .unwrap();
        let desc = frame.colour.unwrap();
        frame.aux.push(AuxTarget {
            width: SIZE / 2,
            height: SIZE / 2,
            ..desc
        });
        targets.insert(TargetRef::Aux(0), small);
        assert!(backend.scene_targets(&frame, &targets).is_none());
        assert_eq!(backend.view().last_presented(), None);
        assert_eq!(backend.stats().view_frames, 0);
    }
}
