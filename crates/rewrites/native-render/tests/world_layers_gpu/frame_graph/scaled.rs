//! The frame borrows the authoritative render-scale image. The next
//! guest view must see its contents, reports and sampled versions.

use super::*;
use d3d::{Backend, GuestLayoutKind, ScaledBackend};
use native_render::frame_graph::draw_scaled_display_on;
use std::collections::HashMap;

fn scaled(ctx: &GfxContext, scale: u32, single: bool) -> ScaledBackend<GfxBackend, GfxBackend> {
    let backend = || {
        GfxBackend::new(GfxContext {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
            caps: ctx.caps,
        })
    };
    ScaledBackend::with_single_copy(backend(), backend(), scale, single).unwrap()
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
    let mut backend = scaled(&ctx, 4, true);
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
    Arc::make_mut(draw).hw.zpass_count_enable = true;
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
    let physical = backend.view_surface(&colour).unwrap();
    assert_eq!(
        backend.view().surface_extent(physical),
        Some((SIZE * 4, SIZE * 4))
    );
    // At this Swap the menu asked for the exact 1x reference.
    backend.follow_scale(|| 1);
    backend.follow_lighting(|| Lighting::Retail);
    assert!(backend.can_draw_scene(&frame, &targets));
    let (report, mut delivered) =
        draw_scaled_display_on(&mut backend, &sealed(), &frame, None, &targets).unwrap();
    assert_eq!(report.lighting, Lighting::E3);
    assert_eq!(backend.reference().batch_stats().draws, 0);
    assert_eq!(backend.view().last_presented(), Some(physical));
    assert_eq!((backend.scale(), backend.stats().view_frames), (1, 1));
    assert_eq!(backend.stats().display_draws, 1);
    assert!(
        backend.can_draw_scene(&frame, &targets),
        "the 1x retail frame can keep drawing on the imported image"
    );
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
fn four_x_mixed_grid_clear_and_upload_preserve_the_rest_of_the_frame() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    if !ctx.caps.depth32_stencil8 {
        return;
    }
    let aux = TargetRef::Aux(0);
    let side = SIZE / 2;
    for single in [false, true] {
        let mut backend = scaled(&ctx, 4, single);
        let colour = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
        let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
        let small = backend.create_texture_target(FORMAT, side, side).unwrap();
        let targets = HashMap::from([(MAIN, colour), (Z, depth), (aux, small)]);
        let mut frame = replay();
        frame.aux.push(AuxTarget {
            width: side,
            height: side,
            ..frame.colour.unwrap()
        });
        op(&mut frame, Slot::Opaque, clear());
        // An unscaled aux colour and scaled Z on the same clear.
        op(
            &mut frame,
            Slot::Opaque,
            ReplayOp::Clear {
                colour: aux,
                depth: Z,
                clear: Clear {
                    color: Some([1., 0., 0., 1.]),
                    depth: Some(1.),
                    stencil: Some(0x32),
                },
            },
        );
        let mut desc =
            gfx::TextureDesc::new(FORMAT, gfx::texture::TextureKind::D2, side, side, 1, 1);
        desc.pitch = side * 4;
        op(
            &mut frame,
            Slot::AfterOpaque,
            ReplayOp::Upload {
                to: aux,
                desc,
                bytes: [0, 255, 0, 255].repeat((side * side) as usize).into(),
            },
        );
        op(
            &mut frame,
            Slot::AfterOpaque,
            ReplayOp::Bind {
                colour: aux,
                depth: Z,
            },
        );
        let mut triangle = guest_triangle(2., [0., 0., 1., 1.], None);
        let ReplayOp::Draw { draw, .. } = &mut triangle else {
            unreachable!()
        };
        let d = Arc::make_mut(draw);
        d.target.width = side;
        d.target.height = side;
        d.viewport.width = side;
        d.viewport.height = side;
        d.clip = Some(d3d::PixelRect {
            x: 0,
            y: 0,
            width: side,
            height: side,
        });
        op(&mut frame, Slot::AfterOpaque, triangle);
        op(
            &mut frame,
            Slot::AfterOpaque,
            ReplayOp::Copy {
                from: aux,
                to: MAIN,
            },
        );
        assert_eq!(backend.can_draw_scene(&frame, &targets), single);
        if !single {
            assert!(backend.view().last_presented().is_none());
            continue;
        }
        draw_scaled_display_on(&mut backend, &sealed(), &frame, None, &targets).unwrap();
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
            &bytes[inside..inside + 4],
            &[255, 0, 0, 255],
            "mixed-grid triangle reaches the scaled scene"
        );
        assert_eq!(&bytes[0..4], &[0, 255, 0, 255]);
        assert_eq!(&bytes[(side * 4) as usize..][..4], &[0, 0, 0, 255]);
        let z = backend
            .read_depth(
                depth,
                DEPTH,
                SIZE,
                SIZE,
                GuestLayoutKind::Linear { pitch: SIZE * 4 },
            )
            .unwrap()
            .unwrap();
        assert_eq!(u32::from_le_bytes(z[..4].try_into().unwrap()), 0xffff_ff32);
        assert_eq!(backend.reference().batch_stats().draws, 0);
    }
}

#[test]
fn one_x_e3_wrapper_records_once_and_resumes_the_menu_with_a_live_view() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    if !ctx.caps.depth32_stencil8 {
        return;
    }
    let mut backend = scaled(&ctx, 1, true);
    backend.set_lighting(Lighting::E3);
    let colour = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let targets = HashMap::from([(MAIN, colour), (Z, depth)]);
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(
        &mut frame,
        Slot::Opaque,
        guest_triangle(2.0, [0., 1., 0., 1.], None),
    );
    assert!(
        !backend.can_draw_scene(&frame, &targets),
        "legacy 1x E3 may have two different prior images"
    );
    frame.ops.insert(
        0,
        (
            SlotAt {
                slot: Slot::Opaque,
                view: 0,
            },
            clear(),
        ),
    );
    op(
        &mut frame,
        Slot::AfterOpaque,
        ReplayOp::Sample {
            of: MAIN,
            id: 6,
            how: ReplaySample::InPlace,
        },
    );
    assert!(backend.can_draw_scene(&frame, &targets));
    let (report, mut outputs) =
        draw_scaled_display_on(&mut backend, &sealed(), &frame, None, &targets).unwrap();
    assert_eq!(report.lighting, Lighting::E3);
    assert_eq!(backend.reference().batch_stats().draws, 0);
    assert_eq!(backend.view().batch_stats().draws, 1);
    frame.ops.remove(0);
    assert!(
        backend.can_draw_scene(&frame, &targets),
        "previous native images agree on old pixels"
    );
    let live = outputs.samples.remove(&6).unwrap();
    // The original guest and E3 view have separate live allocations.
    let original = backend.surface_texture(colour).unwrap().unwrap();
    assert_eq!(live.reference.texture, original.reference.texture);
    assert_eq!(live.view.unwrap().texture, original.view.unwrap().texture);
    let shown = backend.view_surface(&colour).unwrap();
    let (shown_pixels, _) = backend.view_mut().read_color(shown).unwrap();
    let exact = backend
        .read_surface(
            colour,
            FORMAT,
            SIZE,
            SIZE,
            GuestLayoutKind::Linear { pitch: SIZE * 4 },
        )
        .unwrap();
    let i = (4 * (4 * SIZE + 4)) as usize;
    assert_eq!(&shown_pixels[i..i + 4], &[0, 255, 0, 255]);
    assert_eq!(exact, shown_pixels);
    backend.present(colour, 0).unwrap();
    assert!(
        !backend.can_draw_scene(&frame, &targets),
        "legacy presentation retires the 1x coherence proof"
    );
    assert_eq!(backend.view().last_presented(), Some(shown));
}
