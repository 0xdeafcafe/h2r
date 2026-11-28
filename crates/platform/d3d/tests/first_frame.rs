//! First-frame checks over synthetic guest memory: the scaled backend's
//! parity observation, Verify-policy invalidation reports, the stop paths
//! and the Swap ring.

use crate::common::*;
use d3d::abi::Regs;
use d3d::backend::Recorded;
use d3d::{Device, Event, GfxBackend, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::GfxContext;

/// At 1x a sleeping scaled view presents from the reference's store.
#[test]
fn parity_observes_only_guest_swaps_even_when_the_view_sleeps() {
    use d3d::Backend;
    use std::sync::{Arc, Mutex};

    let Ok(ctx) = GfxContext::headless() else { return };
    let reference = GfxBackend::new(GfxContext {
        instance: ctx.instance.clone(), adapter: ctx.adapter.clone(), device: ctx.device.clone(),
        queue: ctx.queue.clone(), caps: ctx.caps,
    });
    let observed = Arc::new(Mutex::new(Vec::new()));
    let out = Arc::clone(&observed);
    let mut view = GfxBackend::new(ctx);
    view.observe_guest_present(move |ordinal, _, store, surface, _| {
        let d = store.get(surface).unwrap().desc();
        out.lock().unwrap().push((ordinal, d.width, d.height));
    });
    let mut scaled = d3d::ScaledBackend::with_single_copy(reference, view, 1, false).unwrap();
    let back = scaled.create_surface(gfx::D3DFormat::LinA8R8G8B8, 16, 8, false).unwrap();
    scaled.present(back, 0).unwrap();
    scaled.present_replay(back, None).unwrap();
    scaled.follow_scale(|| 2);
    scaled.present(back, 0).unwrap();
    scaled.present_replay(back, None).unwrap();
    scaled.present(back, 0).unwrap();
    assert_eq!(&*observed.lock().unwrap(), &[(1, 16, 8), (2, 16, 8), (3, 32, 16)]);
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
    m.write_u32(put, (1 << 18) | 0x1D70).unwrap();
    m.write_u32(put + 4, 0).unwrap();
    m.write_u32(dev, put + 8).unwrap();
    let e = d.commit(&mut m).unwrap_err();
    assert!(
        e.to_string().contains("unimplemented NV2A method 0x1d70"),
        "{e}"
    );

    // An entry with no implementation.
    let e = call(&mut d, &mut m, 0x3f77c0, Regs::default(), &[]).unwrap_err();
    assert!(e.to_string().contains("D3DDevice_Reset"), "{e}");
}

#[test]
fn swap_flips_the_ring_and_reports_presents() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let (back, front) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
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
    // A device made without multisampling or D3DSWAPEFFECT_COPY flips
    // whatever the COPY bit says (0x3f9d75 -> 0x3f9a70).
    assert_eq!(d.front_buffer(), Some(back), "the image shown is the front");
    assert_eq!(d.back_buffer(), Some(front));
    // Without FINISH the original returns the count and does nothing
    // else (0x3f9d7a): COPY (1) and BYPASSCOPY (2) alone neither present
    // nor flip.
    let swap = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory, eax: u32| {
        call(
            d,
            m,
            0x3f9d00,
            Regs {
                eax,
                ..Regs::default()
            },
            &[],
        )
        .unwrap()
        .eax
    };
    let presents = |d: &Device<RecordingBackend>| {
        d.backend
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::Present { .. }))
            .count()
    };
    for flags in [1, 2, 3] {
        assert_eq!(swap(&mut d, &mut m, flags), Some(1), "flags {flags}");
        assert_eq!(presents(&d), 1, "flags {flags}");
        assert_eq!(
            (d.back_buffer(), d.front_buffer()),
            (Some(front), Some(back))
        );
    }
    assert_eq!(
        m.read_u32(d3d::GuestLayout::HALO2.device + 0x2478).unwrap(),
        1
    );
    // FINISH (4) alone flips.
    assert_eq!(swap(&mut d, &mut m, 4), Some(2));
    assert_eq!(presents(&d), 2);
    assert_eq!(
        (d.back_buffer(), d.front_buffer()),
        (Some(back), Some(front))
    );
}
