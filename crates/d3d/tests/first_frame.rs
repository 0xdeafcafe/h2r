//! The first-frame path over synthetic guest memory: CreateDevice, state,
//! a pre-transformed quad (from a vertex buffer and from inline push-buffer
//! words, as Halo's game code writes them), Swap, and the pixels that come
//! out. Plus CPU-only tests of caching, invalidation and the stop paths.

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::backend::Recorded;
use d3d::{Event, GfxBackend, GuestMemory, RecordingBackend, UploadPolicy};
use gfx::GfxContext;

#[test]
fn first_frame_renders_through_gfx() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (mut d, mut m) = create(GfxBackend::new(ctx), UploadPolicy::Tracked);
    let l = d3d::GuestLayout::HALO2;
    assert_eq!(m.read_u32(PP_DEVICE).unwrap(), l.device);
    assert_eq!(m.read_u32(l.p_device).unwrap(), l.device);

    set_states(&mut d, &mut m);
    // Clear(0, NULL, TARGET | ZBUFFER, blue, 1.0, 0).
    call(
        &mut d,
        &mut m,
        0x3f9fc0,
        Regs::default(),
        &[0, 0, 0xF1, 0xFF00_00FF, 1.0f32.to_bits(), 0],
    )
    .unwrap();

    // Left half red, from a vertex buffer: DrawVertices(EAX = count, ECX = start, [esp+4] = type).
    quad_vb(&mut m, 0.0, 0.0, 32.0, 64.0, 0xFFFF_0000);
    bind_quad(&mut d, &mut m);
    call(
        &mut d,
        &mut m,
        0x3faed0,
        Regs {
            eax: 4,
            ecx: 0,
            ..Regs::default()
        },
        &[TRIANGLESTRIP],
    )
    .unwrap();

    // Bottom-right quadrant green, written inline into the push buffer; it
    // executes at the next D3D entry (Swap).
    inline_quad(&mut m, 32.0, 32.0, 64.0, 64.0, [0, 255, 0, 255]);

    let out = call(&mut d, &mut m, 0x3f9d00, Regs::default(), &[]).unwrap();
    assert_eq!(out.eax, Some(1));
    assert_eq!(m.read_u32(l.device + 0x2478).unwrap(), 1);
    assert_eq!(d.stats().inline_draws, 1);
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );

    let shown = d.backend.last_presented().expect("presented");
    let (px, order) = d.backend.store.read_color(&d.backend.ctx, shown).unwrap();
    let rgba = |p: [u8; 4]| match order {
        gfx::encode::HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
        _ => p,
    };
    assert_eq!(
        rgba(pixel(&px, 8, 8)),
        [255, 0, 0, 255],
        "vertex-buffer quad"
    );
    assert_eq!(
        rgba(pixel(&px, 8, 56)),
        [255, 0, 0, 255],
        "vertex-buffer quad"
    );
    assert_eq!(rgba(pixel(&px, 48, 8)), [0, 0, 255, 255], "clear colour");
    assert_eq!(
        rgba(pixel(&px, 48, 48)),
        [0, 255, 0, 255],
        "inline push-buffer quad"
    );
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

#[test]
fn draws_snapshot_guest_state_arrays() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    // Game code flips blending on by storing into the array only.
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.render_state_slot(RS_ALPHABLENDENABLE), 1)
        .unwrap();
    m.write_u32(l.render_state_slot(RS_ZFUNC), 0x1234).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let draws = d.backend.draws();
    assert_eq!(draws.len(), 1);
    assert!(draws[0].state.render.blend_enable);
    assert_eq!(draws[0].streams[0].data.len(), 80);
    assert!(
        d.take_events()
            .iter()
            .any(|e| matches!(e, Event::BadState(_)))
    );
}

#[test]
fn indexed_draws_are_deindexed_with_the_base_vertex() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFF00_00FF);
    bind_quad(&mut d, &mut m);
    let idx = MISC + 0x3000;
    for (i, v) in [1u16, 0, 2].iter().enumerate() {
        m.write(idx + 2 * i as u32, &v.to_le_bytes()).unwrap();
    }
    m.write_u32(d3d::GuestLayout::HALO2.device + 0x1C, 1)
        .unwrap(); // base vertex
    call(&mut d, &mut m, 0x3fafa0, Regs::default(), &[5, 3, idx]).unwrap();
    let draw = d.backend.draws()[0].clone();
    let x =
        |i: usize| f32::from_le_bytes(draw.streams[0].data[20 * i..20 * i + 4].try_into().unwrap());
    let y = |i: usize| {
        f32::from_le_bytes(
            draw.streams[0].data[20 * i + 4..20 * i + 8]
                .try_into()
                .unwrap(),
        )
    };
    // Indices 1,0,2 + base 1 = vertices 2,1,3: (0,8), (8,0), (8,8).
    assert_eq!(
        [(x(0), y(0)), (x(1), y(1)), (x(2), y(2))],
        [(0.0, 8.0), (8.0, 0.0), (8.0, 8.0)]
    );
}

#[test]
fn textures_upload_lazily_and_invalidate_on_reported_writes() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    texture(&mut m, 0x80);
    // SetTexture(EAX = stage 0, [esp+4] = texture) takes an internal reference.
    call(
        &mut d,
        &mut m,
        0x3f8320,
        Regs {
            eax: 0,
            ..Regs::default()
        },
        &[TEX_HEADER],
    )
    .unwrap();
    assert_eq!(m.read_u32(TEX_HEADER).unwrap() & 0x0078_0000, 0x0008_0000);
    assert_eq!(d.backend.uploads(), 0, "binding does not upload");

    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 1, "second draw hits the cache");

    // A write elsewhere on an unwatched page does not invalidate.
    d.note_write(MISC + 0x5000, 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 1);

    // A reported write to the texels does.
    m.write(0x8000_0000 | TEX_DATA, &[0x10; 4]).unwrap();
    d.note_write(0x8000_0000 | TEX_DATA, 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 2);

    // So does a header change (new format/size/data), with no write report.
    m.write_u32(TEX_HEADER + 16, 15 | (1 << 12)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 3);
    let tex = d.backend.calls.iter().rev().find_map(|c| {
        if let Recorded::Draw { textures, .. } = c {
            Some(*textures)
        } else {
            None
        }
    });
    assert!(tex.unwrap()[0].is_some());
}

#[test]
fn verify_policy_reports_missed_invalidations() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Verify);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    texture(&mut m, 0x80);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    m.write(0x8000_0000 | TEX_DATA, &[0x11; 4]).unwrap(); // not reported
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 2, "Verify re-uploads changed data");
    assert!(d.take_events().iter().any(|e| matches!(
        e,
        Event::MissedInvalidation {
            header: TEX_HEADER,
            ..
        }
    )));
}

#[test]
fn make_requested_space_executes_and_rewinds() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    bind_quad(&mut d, &mut m);
    inline_quad(&mut m, 0.0, 0.0, 4.0, 4.0, [255, 255, 255, 255]);
    let out = call(
        &mut d,
        &mut m,
        0x3faae0,
        Regs {
            eax: 0x100,
            ..Regs::default()
        },
        &[0x4000],
    )
    .unwrap();
    assert_eq!(out.eax, Some(PB.addr));
    assert_eq!(m.read_u32(d3d::GuestLayout::HALO2.device).unwrap(), PB.addr);
    let draws = d.backend.draws();
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].vertex_count, 4);
    assert_eq!(draws[0].primitive, gfx::vertex::PrimitiveType::QuadList);
}

#[test]
fn unsupported_paths_stop_with_clear_errors() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    // A partial clear goes to the backend's clear_rects; a backend without
    // one stops (the trait default).
    let rect = MISC + 0x4000;
    for (i, v) in [0u32, 0, 16, 16].iter().enumerate() {
        m.write_u32(rect + 4 * i as u32, *v).unwrap();
    }
    call(
        &mut d,
        &mut m,
        0x3f9fc0,
        Regs::default(),
        &[1, rect, 0xF0, 0, 0, 0],
    )
    .unwrap();
    assert!(matches!(
        d.backend.calls.last(),
        Some(Recorded::ClearRects { .. })
    ));

    // An NV2A method the device does not know, written inline.
    let dev = d3d::GuestLayout::HALO2.device;
    let put = m.read_u32(dev).unwrap();
    m.write_u32(put, (1 << 18) | 0x1710).unwrap();
    m.write_u32(put + 4, 0).unwrap();
    m.write_u32(dev, put + 8).unwrap();
    let e = d.commit(&mut m).unwrap_err();
    assert!(
        e.to_string().contains("unimplemented NV2A method 0x1710"),
        "{e}"
    );

    // An entry with no implementation.
    let e = call(&mut d, &mut m, 0x3f77c0, Regs::default(), &[]).unwrap_err();
    assert!(e.to_string().contains("D3DDevice_Reset"), "{e}");
}

#[test]
fn swap_flips_the_ring_and_reports_presents() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let back = d.back_buffer().unwrap();
    // Flags 0 means COPY | FINISH as compiled.
    assert_eq!(
        call(&mut d, &mut m, 0x3f9d00, Regs::default(), &[])
            .unwrap()
            .eax,
        Some(1)
    );
    assert!(d.backend.calls.contains(&Recorded::Present {
        surface: back,
        flags: 5
    }));
    assert_eq!(d.back_buffer(), Some(back), "COPY keeps the back buffer");
    // FLIP (2) rotates.
    assert_eq!(
        call(
            &mut d,
            &mut m,
            0x3f9d00,
            Regs {
                eax: 2,
                ..Regs::default()
            },
            &[]
        )
        .unwrap()
        .eax,
        Some(2)
    );
    assert_eq!(d.front_buffer(), Some(back));
    assert_ne!(d.back_buffer(), Some(back));
}
