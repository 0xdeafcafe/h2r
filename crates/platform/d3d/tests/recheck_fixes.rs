//! Regression tests for the second independent review ("recheck"), one or
//! more per finding (r1..r6), plus the documentation correction. The
//! reproductions are the reviewer's, adapted.

use crate::common::{self, *};
use d3d::abi::Regs;
use d3d::pushbuf::{PushError, method};
use d3d::{D3dError, GfxBackend, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::GfxContext;

const DEV: u32 = 0x0040_4FE0;
const VP: u32 = MISC + 0x7000;

fn rec() -> (d3d::Device<RecordingBackend>, SparseMemory) {
    create(RecordingBackend::new(), UploadPolicy::Tracked)
}

fn quad(d: &mut d3d::Device<RecordingBackend>, m: &mut SparseMemory) {
    quad_vb(m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    bind_quad(d, m);
}

fn push_words(m: &mut SparseMemory, words: &[u32]) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

fn simple<B: d3d::Backend>(
    d: &mut d3d::Device<B>,
    m: &mut SparseMemory,
    header: u32,
    value: u32,
) -> Result<(), D3dError> {
    call(
        d,
        m,
        0x3f5cf0,
        Regs {
            ecx: header,
            edx: value,
            ..Regs::default()
        },
        &[],
    )
    .map(|_| ())
}

fn viewport(m: &mut SparseMemory, at: u32, v: [u32; 4], z: [f32; 2]) {
    for (i, w) in [v[0], v[1], v[2], v[3], z[0].to_bits(), z[1].to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(at + 4 * i as u32, *w).unwrap();
    }
}

fn is_unimplemented<T: std::fmt::Debug>(r: &Result<T, D3dError>) -> bool {
    matches!(r, Err(D3dError::Unimplemented { .. }))
}

// r1: push-buffer payloads, in order -----------------------------------------

/// Simple with a header that is not a one-word state method stops.
#[test]
fn r1_simple_with_an_unknown_method_stops() {
    let (mut d, mut m) = rec();
    assert!(is_unimplemented(&simple(&mut d, &mut m, 0x0004_0b00, 1)));
    assert!(is_unimplemented(&simple(&mut d, &mut m, 0x0008_0304, 1)));
    // 0x1D90 is the placeholder several slots share: not invertible.
    assert!(is_unimplemented(&simple(&mut d, &mut m, 0x0004_1d90, 1)));
}

// r2: SetViewport --------------------------------------------------------------

/// Halo `0x14ac0`: save device+0xEE0, full-screen viewport, clear, restore.
#[test]
fn r2_viewport_save_restore_through_the_device_fields() {
    let (mut d, mut m) = rec();
    quad(&mut d, &mut m);
    viewport(&mut m, VP, [0, 0, W, H], [0.0, 1.0]);
    d.set_viewport(&mut m, VP).unwrap();
    let mut saved = vec![0; 24];
    m.read(DEV + 0xEE0, &mut saved).unwrap();
    m.write(VP + 0x20, &saved).unwrap();
    viewport(&mut m, VP + 0x40, [0, 0, 640, 480], [0.0, 1.0]);
    d.set_viewport(&mut m, VP + 0x40).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    d.set_viewport(&mut m, VP + 0x20).unwrap();
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    // The stored words are the original's: X, Y, clipped W, H, MinZ, MaxZ.
    viewport(&mut m, VP, [8, 4, 1000, 10], [0.25, 0.5]);
    d.set_viewport(&mut m, VP).unwrap();
    let w = m.read_words(DEV + 0xEE0, 6).unwrap();
    assert_eq!(
        w,
        vec![8, 4, W - 8, 10, 0.25f32.to_bits(), 0.5f32.to_bits()]
    );
    assert_eq!(m.read_u32(0x0040_7490).unwrap() & 0x100, 0x100, "dirty");
}

#[test]
fn r2_null_viewport_keeps_the_previous_viewport() {
    let (mut d, mut m) = rec();
    viewport(&mut m, VP, [8, 8, 16, 16], [0.0, 1.0]);
    d.set_viewport(&mut m, VP).unwrap();
    let saved = d.viewport();
    d.set_viewport(&mut m, 0).unwrap();
    assert_eq!(d.viewport(), saved);
}

// r3: Clear and the viewport ----------------------------------------------------

#[test]
fn r3_clear_clips_to_the_viewport() {
    let (mut d, mut m) = rec();
    let clears = |d: &d3d::Device<RecordingBackend>| {
        d.backend
            .calls
            .iter()
            .filter(|c| matches!(c, d3d::Recorded::Clear { .. }))
            .count()
    };
    let rect_clears = |d: &d3d::Device<RecordingBackend>| -> Vec<Vec<d3d::PixelRect>> {
        d.backend
            .calls
            .iter()
            .filter_map(|c| match c {
                d3d::Recorded::ClearRects { clear, .. } => Some(clear.rects.clone()),
                _ => None,
            })
            .collect()
    };
    let vp_rect = d3d::PixelRect {
        x: 8,
        y: 8,
        width: 16,
        height: 16,
    };
    viewport(&mut m, VP, [8, 8, 16, 16], [0.0, 1.0]);
    d.set_viewport(&mut m, VP).unwrap();
    // Count = 0 means the viewport: a rectangle clear of the viewport.
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    assert_eq!(rect_clears(&d), vec![vec![vp_rect]]);
    // A full-target rectangle is clipped to the viewport too.
    let rect = VP + 0x100;
    for (i, w) in [0u32, 0, W, H].iter().enumerate() {
        m.write_u32(rect + 4 * i as u32, *w).unwrap();
    }
    d.clear(&mut m, 1, rect, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(rect_clears(&d).last(), Some(&vec![vp_rect]));
    // A rectangle outside the viewport clears nothing (and does not stop).
    for (i, w) in [40u32, 40, 60, 60].iter().enumerate() {
        m.write_u32(rect + 4 * i as u32, *w).unwrap();
    }
    d.clear(&mut m, 1, rect, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(clears(&d), 0);
    // Full viewport: Count = 0 clears the whole target.
    viewport(&mut m, VP, [0, 0, W, H], [0.0, 1.0]);
    d.set_viewport(&mut m, VP).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(clears(&d), 1);
}

#[test]
fn r3_gpu_clear_never_touches_pixels_outside_the_viewport() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    eprintln!("GPU: {:?}", ctx.adapter.get_info());
    let (mut d, mut m) = create(GfxBackend::new(ctx), UploadPolicy::Tracked);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    viewport(&mut m, VP, [8, 8, 16, 16], [0.0, 1.0]);
    d.set_viewport(&mut m, VP).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    let (px, order) = d
        .backend
        .read_color(d.back_buffer().unwrap())
        .unwrap();
    let p = pixel(&px, 0, 0);
    let p = match order {
        gfx::encode::HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
        _ => p,
    };
    assert_eq!(p, [0, 0, 255, 255], "pixel outside the viewport stays blue");
    let q = pixel(&px, 8, 8);
    let q = match order {
        gfx::encode::HostPixelOrder::Bgra8 => [q[2], q[1], q[0], q[3]],
        _ => q,
    };
    assert_eq!(
        q,
        [255, 0, 0, 255],
        "pixel inside the viewport is cleared red"
    );
    let r = pixel(&px, 24, 24);
    let r = match order {
        gfx::encode::HostPixelOrder::Bgra8 => [r[2], r[1], r[0], r[3]],
        _ => r,
    };
    assert_eq!(
        r,
        [0, 0, 255, 255],
        "first pixel past the viewport stays blue"
    );
}

// r4: inline vertex cap ----------------------------------------------------------

/// One Begin held open while the game rewinds the buffer through
/// MakeRequestedSpace: the cap holds across drains.
#[test]
fn r4_inline_vertices_are_capped_across_rewinds() {
    let (mut d, mut m) = rec();
    push_words(&mut m, &[(1 << 18) | method::BEGIN_END, 5]);
    let mut remaining = d3d::device::MAX_VERTICES + 1;
    let mut failed = None;
    while remaining > 0 {
        let n = remaining.min(2047);
        let mut words = vec![1.0f32.to_bits(); n as usize + 1];
        // Non-incrementing VERTEX4F.w: every word emits a vertex.
        words[0] = 0x4000_0000 | (n << 18) | (method::VERTEX4F + 12);
        push_words(&mut m, &words);
        if let Err(e) = d.make_requested_space(&mut m, 0, 0) {
            failed = Some(e);
            break;
        }
        remaining -= n;
    }
    let e = failed.expect("MAX_VERTICES + 1 inline vertices were accepted");
    assert!(
        matches!(e, D3dError::Push(PushError::Immediate { .. })),
        "{e}"
    );
    // The device latched instead of dropping the excess silently.
    assert!(d.fault().is_some());
}

#[test]
fn r4_api_set_vertex_data_shares_the_cap() {
    let (mut d, mut m) = rec();
    d.begin(&mut m, 5).unwrap();
    for _ in 0..d3d::device::MAX_VERTICES {
        d.set_vertex_data(&mut m, -1, [0.0, 0.0, 0.0, 1.0]).unwrap();
    }
    let e = d.set_vertex_data(&mut m, -1, [0.0, 0.0, 0.0, 1.0]);
    assert!(e.is_err());
}

// r5: header arithmetic ------------------------------------------------------------

#[test]
fn r5_a_header_at_the_top_of_the_address_space_is_an_error() {
    let (mut d, mut m) = rec();
    let addr = 0xffff_ffec;
    m.map(0xffff_f000, 0x1000);
    // The 20-byte pixel container fits; Parent at +20 would wrap.
    for (i, w) in [
        0x0005_0001u32,
        0,
        0,
        (2 << 4) | (0x12 << 8) | (1 << 16),
        63 | (63 << 12) | (3 << 24),
    ]
    .iter()
    .enumerate()
    {
        m.write_u32(addr + 4 * i as u32, *w).unwrap();
    }
    let r = d.set_render_target(&mut m, addr, 0);
    assert!(matches!(r, Err(D3dError::Guest(_))), "{r:?}");
    assert!(d3d::resource::SurfaceHeader::read(&m, addr).is_err());
    let h = d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 1, 1, 64, 0);
    assert!(h.write(&mut m, addr).is_err());
    // Nothing was written: the first 20 bytes are unchanged.
    assert_eq!(m.read_u32(addr).unwrap(), 0x0005_0001);
}

// r6 is tests/registration.rs (real runtime types).

// Documentation ------------------------------------------------------------------

/// `docs/graphics/d3d.md` once listed `0x0A20` / `0x1A20` at `0x52b89` as inline push
/// methods; they are PSDEF stores. The method table must not regain them,
/// and the device must not pretend to execute them.
#[test]
fn doc_inline_method_table_has_no_psdef_stores() {
    let doc = include_str!("../../../../docs/graphics/d3d.md");
    let table: Vec<&str> = doc
        .lines()
        .skip_while(|l| !l.starts_with("| method | NV2A meaning"))
        .take_while(|l| l.starts_with('|'))
        .collect();
    assert!(table.len() > 5, "inline method table not found");
    for row in &table {
        assert!(
            !row.contains("0x0A20") && !row.contains("0x1A20"),
            "PSDEF store listed as a push method: {row}"
        );
    }
    assert!(doc.contains("`0x00052b89`") && doc.contains("PSDEF"));
    // Pushed, 0x0A20 is NV097_SET_VIEWPORT_OFFSET (D3D's 0x3f6190 pushes
    // it); it lands in the register state, not in a PSDEF.
    let (mut d, mut m) = rec();
    push_words(&mut m, &[(1 << 18) | 0x0A20, 2.0f32.to_bits()]);
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().viewport_offset[0], 2.0);
    push_words(&mut m, &[(1 << 18) | 0x1D70, 0]);
    assert!(matches!(
        d.commit(&mut m),
        Err(D3dError::Push(PushError::UnknownMethod {
            method: 0x1D70,
            ..
        }))
    ));
}

// Third review: teardown ------------------------------------------------------------

/// `0x3fc670..0x3fc68d` drops the stream references, `0x3fc704..0x3fc710`
/// zeroes `D3D__Streams`; textures and palettes are released too.
#[test]
fn t1_teardown_releases_bound_resources_and_clears_the_streams() {
    let (mut d, mut m) = rec();
    quad(&mut d, &mut m);
    texture(&mut m, 0x80);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    let vb = m.read_u32(VB_HEADER).unwrap();
    let tex = m.read_u32(TEX_HEADER).unwrap();
    assert_eq!(vb, 0x0108_0001);
    let streams = d3d::GuestLayout::HALO2.streams;
    assert_eq!(m.read_u32(streams + 8).unwrap(), VB_HEADER);
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(m.read_u32(VB_HEADER).unwrap(), 0x0100_0001);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), tex - 0x0008_0000);
    assert_eq!(m.read_words(streams, 48).unwrap(), vec![0; 48]);
}

/// The last reference goes to D3D_DestroyResource (`0x3fc63c`): the word
/// is decremented, then the buffer is queued for the original to free.
#[test]
fn t1_teardown_destroys_a_resource_only_the_device_held() {
    let (mut d, mut m) = rec();
    quad(&mut d, &mut m);
    // Title reference count 0: the device holds the only reference.
    m.write_u32(VB_HEADER, 0x0108_0000).unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(d.take_pending_destroys(), vec![VB_HEADER]);
    assert_eq!(m.read_u32(VB_HEADER).unwrap(), 0x0100_0000);
    let streams = d3d::GuestLayout::HALO2.streams;
    assert_eq!(m.read_u32(streams + 8).unwrap(), 0);
}

#[test]
fn t2_teardown_destroys_every_device_surface() {
    let (mut d, mut m) = rec();
    assert_eq!(d.backend.live_surfaces(), 3, "back, front, depth");
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(d.backend.live_surfaces(), 0);
    // Create/release cycles do not accumulate surfaces.
    for _ in 0..3 {
        let regs = Regs {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..Regs::default()
        };
        call(&mut d, &mut m, 0x3f5240, regs, &[PARAMS]).unwrap();
        assert_eq!(d.backend.live_surfaces(), 3);
        d.release_device(&mut m, DEV).unwrap();
    }
    assert_eq!(d.backend.live_surfaces(), 0);
}

#[test]
fn t2_gpu_teardown_empties_the_surface_store() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    eprintln!("GPU teardown: {:?}", ctx.adapter.get_info().backend);
    let (mut d, mut m) = create(GfxBackend::new(ctx), UploadPolicy::Tracked);
    assert_eq!(d.backend.store.len(), 3);
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(d.backend.store.len(), 0);
}

// The encode table comes from the image -------------------------------------

/// CreateDevice reads `D3DSIMPLERENDERSTATEENCODE` from guest memory; a
/// table that is not there, or not well formed, stops creation with a named
/// error before anything is written or created.
#[test]
fn create_device_stops_on_a_missing_or_malformed_encode_table() {
    let at = d3d::GuestLayout::HALO2.simple_encode;
    for corrupt in [
        // Slot 3 is a two-word header.
        Box::new(|m: &mut SparseMemory| m.write_u32(at + 12, (2 << 18) | 0x304).unwrap())
            as Box<dyn Fn(&mut SparseMemory)>,
        // Zeroed (image not loaded there).
        Box::new(|m: &mut SparseMemory| m.write(at, &[0; 92 * 4]).unwrap()),
    ] {
        let mut m = memory();
        corrupt(&mut m);
        let mut d = d3d::Device::new(
            d3d::DeviceConfig::halo2(PB, SCRATCH),
            RecordingBackend::new(),
        );
        common::write_params(&mut m);
        let err = call(
            &mut d,
            &mut m,
            0x3f5240,
            Regs {
                eax: 0x40,
                ecx: PP_DEVICE,
                ..Regs::default()
            },
            &[PARAMS],
        )
        .unwrap_err();
        assert!(matches!(err, D3dError::Unimplemented { .. }), "{err}");
        assert!(
            err.to_string().contains("D3DSIMPLERENDERSTATEENCODE"),
            "{err}"
        );
        assert_eq!(m.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap(), 0);
        assert_eq!(m.read_u32(PP_DEVICE).unwrap(), 0);
        assert_eq!(d.backend.live_surfaces(), 0);
    }
}
