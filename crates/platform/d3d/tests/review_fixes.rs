//! Regression tests for the first independent review of the D3D HLE, one
//! (or a few) per finding. The numbers refer to that review.

use crate::common::*;
use d3d::abi::{self, Regs};
use d3d::pushbuf::{PushError, method};
use d3d::resource::SurfaceHeader;
use d3d::{D3dError, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::D3DFormat;

const SRC: u32 = MISC + 0x4000;
const DEV: u32 = 0x0040_4FE0;

fn rec() -> (d3d::Device<RecordingBackend>, SparseMemory) {
    create(RecordingBackend::new(), UploadPolicy::Tracked)
}

fn rs(m: &SparseMemory, slot: u32) -> u32 {
    m.read_u32(d3d::GuestLayout::HALO2.render_state_slot(slot))
        .unwrap()
}

fn is_invalid(r: Result<(), D3dError>) -> bool {
    matches!(r, Err(D3dError::Invalid { .. }))
}

fn is_unimplemented<T: std::fmt::Debug>(r: Result<T, D3dError>) -> bool {
    matches!(r, Err(D3dError::Unimplemented { .. }))
}

/// A quad bound and ready to draw.
fn quad(d: &mut d3d::Device<RecordingBackend>, m: &mut SparseMemory) {
    quad_vb(m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_FFFF);
    bind_quad(d, m);
}

// 1 ------------------------------------------------------------------------

#[test]
fn p1_1_not_inline_fast_takes_a_dword_count() {
    let (mut d, mut m) = rec();
    for i in 0..8u32 {
        m.write_u32(SRC + 4 * i, (1.0 + i as f32).to_bits())
            .unwrap();
    }
    // ECX = register + 96, EDX = data, [esp+4] = 6 dwords.
    call(
        &mut d,
        &mut m,
        0x3f7330,
        Regs {
            ecx: 10,
            edx: SRC,
            ..Regs::default()
        },
        &[6],
    )
    .unwrap();
    assert_eq!(d.constant(10), [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(d.constant(11), [5.0, 6.0, 0.0, 0.0], "ends mid-register");
    assert_eq!(d.constant(12), [0.0; 4]);
    // The original only pushes; the shadow is not touched.
    let shadow = d3d::GuestLayout::HALO2.vs_constants;
    assert_eq!(m.read_u32(shadow + 16 * 10).unwrap(), 0);
    // Constant4 does update the shadow.
    call(
        &mut d,
        &mut m,
        0x3f7280,
        Regs {
            ecx: 20,
            edx: SRC,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    assert_eq!(m.read_u32(shadow + 16 * 20).unwrap(), 1.0f32.to_bits());
    assert_eq!(d.constant(21), [5.0, 6.0, 7.0, 8.0]);
    // 8 dwords from register 191 overrun the 192 registers.
    let e = call(
        &mut d,
        &mut m,
        0x3f7330,
        Regs {
            ecx: 191,
            edx: SRC,
            ..Regs::default()
        },
        &[8],
    );
    assert!(matches!(e, Err(D3dError::Invalid { .. })), "{e:?}");
}

// 2 ------------------------------------------------------------------------

#[test]
fn p1_2_guest_sizes_cannot_exhaust_memory_or_overflow() {
    let (mut d, mut m) = rec();
    quad(&mut d, &mut m);
    // A 0xffffffff-index draw fails before allocating anything.
    assert!(is_invalid(d.draw_indexed_vertices(
        &mut m,
        5,
        u32::MAX,
        SRC
    )));
    // Base vertex 0xffffffff + index 1.
    m.write(SRC, &1u16.to_le_bytes()).unwrap();
    m.write_u32(DEV + 0x1C, u32::MAX).unwrap();
    assert!(is_invalid(d.draw_indexed_vertices(&mut m, 5, 1, SRC)));
    m.write_u32(DEV + 0x1C, 0).unwrap();
    // Constant register 0xffffffff + 1, and a count whose x4 overflows.
    assert!(is_invalid(d.set_vertex_shader_constant(
        &mut m,
        u32::MAX,
        SRC,
        1,
        true
    )));
    assert!(is_invalid(d.set_vertex_shader_constant_dwords(
        &mut m,
        u32::MAX,
        SRC,
        1
    )));
    assert!(is_invalid(d.set_vertex_shader_constant(
        &mut m,
        0,
        SRC,
        0x4000_0000,
        true
    )));
    // Stream offset arithmetic: start * stride past 4 GiB.
    assert!(is_invalid(d.draw_vertices(&mut m, 6, u32::MAX, 4)));
    // Vertex count limit.
    assert!(is_invalid(d.draw_vertices(&mut m, 6, 0, u32::MAX)));
    // A stack pointer at the top of the address space.
    let entries = abi::entries::<RecordingBackend>();
    let begin = entries.iter().find(|e| e.addr == 0x3fb440).unwrap();
    let r = abi::call(
        begin,
        &mut d,
        &mut m,
        &Regs {
            esp: 0xffff_fffc,
            ..Regs::default()
        },
    );
    assert!(matches!(r, Err(D3dError::Invalid { .. })), "{r:?}");
    // Nothing above latched the device: a normal draw still works.
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();

    // put = consumed + 1 is not a word boundary.
    let (mut d, mut m) = rec();
    m.write_u32(DEV, PB.addr + 1).unwrap();
    assert!(is_invalid(d.commit(&mut m)));
}

// 3 ------------------------------------------------------------------------

// 5 ------------------------------------------------------------------------

// 6 ------------------------------------------------------------------------

fn psdef(m: &mut SparseMemory, def: u32, handle: u32, base: u32) {
    for i in 0..60u32 {
        m.write_u32(def + 4 * i, base + i).unwrap();
    }
    m.write_u32(handle, 1).unwrap();
    m.write_u32(handle + 4, 0).unwrap();
    m.write_u32(handle + 8, def).unwrap();
}

#[test]
fn p1_6_set_pixel_shader_copies_the_psdef_into_render_state() {
    let (mut d, mut m) = rec();
    let (def, handle) = (MISC + 0x5000, MISC + 0x5100);
    psdef(&mut m, def, handle, 0x1000);
    call(
        &mut d,
        &mut m,
        0x3fa3d0,
        Regs {
            eax: handle,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    for i in 0..57 {
        assert_eq!(rs(&m, i), 0x1000 + i, "slot {i}");
    }
    assert_ne!(rs(&m, 57), 0x1000 + 57, "slot 57 (ZFUNC) is past the copy");
    assert_eq!(rs(&m, 136), 0x1000 + 54);
    assert_eq!(m.read_u32(DEV + 0x784).unwrap(), handle);
    assert_eq!(
        m.read_u32(DEV + 0x788).unwrap(),
        (0x1000 + 8) | (0x1000 + 9)
    );
    assert_eq!(m.read_u32(DEV + 0x78C).unwrap(), (0x1000 + 59) & 0x100);
    assert_eq!(m.read_u32(DEV + 0x790).unwrap(), 0x1000 + 54);

    // With device flag 0x10 (BehaviorFlags & 0x10) the copy is skipped.
    m.write_u32(DEV + 8, 0x10).unwrap();
    psdef(&mut m, def, handle, 0x2000);
    d.set_pixel_shader(&mut m, handle).unwrap();
    assert_eq!(rs(&m, 0), 0x1000);
    assert_eq!(m.read_u32(DEV + 0x790).unwrap(), 0x2000 + 54);
}

// 7 ------------------------------------------------------------------------

#[test]
fn p1_7_zbias_derives_polygon_offset_state() {
    let (mut d, mut m) = rec();
    call(&mut d, &mut m, 0x3f59b0, Regs::default(), &[2]).unwrap();
    assert_eq!(rs(&m, 77), (-0.5f32).to_bits());
    assert_eq!(rs(&m, 78), (-2.0f32).to_bits());
    assert_eq!([rs(&m, 79), rs(&m, 80), rs(&m, 81)], [1, 1, 1]);
    assert_eq!(rs(&m, 149), 2);
    // Zero turns the offsets off again.
    call(&mut d, &mut m, 0x3f59b0, Regs::default(), &[0]).unwrap();
    assert_eq!([rs(&m, 79), rs(&m, 80), rs(&m, 81)], [0, 0, 0]);
}

// 8 ------------------------------------------------------------------------

const PAL_A: u32 = MISC + 0x6000;
const PAL_B: u32 = MISC + 0x6010;
const PAL_DATA: u32 = 0x0030_0000;

fn p8_texture(m: &mut SparseMemory) {
    m.map(0x8000_0000 | PAL_DATA, 0x2000);
    // 4x4 swizzled P8: dims 2, format 0x0B, 1 level, log2 sizes 2 and 2.
    m.write_u32(TEX_HEADER, 1 | (4 << 16) | 0x0100_0000)
        .unwrap();
    m.write_u32(TEX_HEADER + 4, TEX_DATA).unwrap();
    m.write_u32(TEX_HEADER + 8, 0).unwrap();
    m.write_u32(
        TEX_HEADER + 12,
        (2 << 4) | (0x0B << 8) | (1 << 16) | (2 << 20) | (2 << 24),
    )
    .unwrap();
    m.write_u32(TEX_HEADER + 16, 0).unwrap();
    m.write(0x8000_0000 | TEX_DATA, &[1; 16]).unwrap();
    for (h, data) in [(PAL_A, PAL_DATA), (PAL_B, PAL_DATA + 0x1000)] {
        m.write_u32(h, 1 | (3 << 16) | 0x0100_0000).unwrap();
        m.write_u32(h + 4, data).unwrap();
        m.write_u32(h + 8, 0).unwrap();
    }
}

#[test]
fn p1_8_palettes_are_part_of_the_texture_cache_identity() {
    let (mut d, mut m) = rec();
    quad(&mut d, &mut m);
    p8_texture(&mut m);
    // One P8 header on two stages with different palettes.
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    d.set_palette(&mut m, 0, PAL_A).unwrap();
    d.set_texture(&mut m, 1, TEX_HEADER).unwrap();
    d.set_palette(&mut m, 1, PAL_B).unwrap();
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 2, "two palettes, two host textures");
    assert!(d.cached_texture(TEX_HEADER, PAL_A).is_some());
    assert!(d.cached_texture(TEX_HEADER, PAL_B).is_some());
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 2);

    // A reported write to palette A's entries re-decodes that texture only
    // (the palettes sit on different pages; tracking is per 4 KiB page).
    m.write_u32(0x8000_0000 | PAL_DATA, 0xFF00_FF00).unwrap();
    d.note_write(0x8000_0000 | PAL_DATA, 4);
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 3);

    // Palette B shrinks to 128 entries (size bits): its identity changes.
    m.write_u32(PAL_B, 1 | (3 << 16) | 0x0100_0000 | (1 << 30))
        .unwrap();
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 4);
}

/// The palette bound at a stage reads back as its entries (what a native
/// pass expands a P8 texture through), sized by the header's size bits;
/// a stage with none bound has none.
#[test]
fn a_stage_palette_reads_back_its_entries() {
    let (mut d, mut m) = rec();
    p8_texture(&mut m);
    assert_eq!(d.stage_palette(&m, 2), None);
    m.write_u32(0x8000_0000 | PAL_DATA, 0xFF80_7F01).unwrap();
    m.write_u32(0x8000_0000 | (PAL_DATA + 4), 0x0102_0304).unwrap();
    d.set_palette(&mut m, 2, PAL_A).unwrap();
    let p = d.stage_palette(&m, 2).unwrap();
    assert_eq!(p.len(), 256);
    assert_eq!(&p[..3], &[0xFF80_7F01, 0x0102_0304, 0]);
    // 32 entries (size code 3).
    m.write_u32(PAL_A, 1 | (3 << 16) | 0x0100_0000 | (3 << 30)).unwrap();
    assert_eq!(d.stage_palette(&m, 2).map(|p| p.len()), Some(32));
    assert_eq!(d.stage_palette(&m, 4), None);
}

#[test]
fn a_render_state_reads_back_as_set() {
    let (mut d, mut m) = rec();
    let slot = d3d::state::slot::STENCILENABLE;
    d.set_render_state(&mut m, slot, 1).unwrap();
    assert_eq!(d.render_state(&m, slot), Some(1));
    d.set_render_state(&mut m, slot, 0).unwrap();
    assert_eq!(d.render_state(&m, slot), Some(0));
    assert_eq!(d.render_state(&m, 100_000), None, "past the slots");
    // The stencil state as a draw takes it: the structure draw's mode 1
    // (`ALWAYS`, reference 0, `REPLACE` on pass, masks 0xff).
    use gfx::state::RenderStateType as R;
    let table = gfx::xdk::RenderStateTable::default();
    let guest = |t: R| table.canonical_to_guest(t.index()).unwrap();
    for (t, v) in [
        (R::StencilEnable, 1),
        (R::StencilFunc, 0x207),
        (R::StencilRef, 0),
        (R::StencilMask, 0xff),
        (R::StencilWriteMask, 0xff),
        (R::StencilFail, 0x1e00),
        (R::StencilZFail, 0x1e00),
        (R::StencilPass, 0x1e01),
    ] {
        d.set_render_state(&mut m, guest(t), v).unwrap();
    }
    let s = d.stencil_state(&m).unwrap();
    assert!(s.enable);
    assert_eq!(s.func, gfx::state::CompareFunc::Always);
    assert_eq!((s.reference, s.read_mask, s.write_mask), (0, 0xff, 0xff));
    assert_eq!(s.pass, gfx::state::StencilOp::Replace);
    assert_eq!(s.depth_fail, gfx::state::StencilOp::Keep);
    d.set_render_state(&mut m, guest(R::StencilPass), 0x1234).unwrap();
    assert_eq!(d.stencil_state(&m), None, "not a stencil op");
    // The cull as a draw takes it (the native world's inheriting passes).
    use gfx::state::{CullMode, FrontFace};
    d.set_render_state(&mut m, guest(R::CullMode), 0x900).unwrap();
    d.set_render_state(&mut m, guest(R::FrontFace), 0x900).unwrap();
    assert_eq!(d.cull_state(&m), Some((CullMode::Cw, FrontFace::Cw)));
    d.set_render_state(&mut m, guest(R::CullMode), 0).unwrap();
    d.set_render_state(&mut m, guest(R::FrontFace), 0x901).unwrap();
    assert_eq!(d.cull_state(&m), Some((CullMode::None, FrontFace::Ccw)));
    d.set_render_state(&mut m, guest(R::CullMode), 0x1234).unwrap();
    assert_eq!(d.cull_state(&m), None, "not a cull mode");
}

#[test]
fn p1_8_verify_hashes_palette_contents() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Verify);
    quad(&mut d, &mut m);
    p8_texture(&mut m);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    d.set_palette(&mut m, 0, PAL_A).unwrap();
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    m.write_u32(0x8000_0000 | PAL_DATA, 0x1234_5678).unwrap(); // unreported
    d.draw_vertices(&mut m, 6, 0, 4).unwrap();
    assert_eq!(d.backend.uploads(), 2);
    assert!(
        d.take_events()
            .iter()
            .any(|e| matches!(e, d3d::Event::MissedInvalidation { .. }))
    );
}

// 9 ------------------------------------------------------------------------

#[test]
fn p1_9_raster_state_clip_and_back_buffer_scale() {
    let (mut d, mut m) = rec();
    // A 32x32 viewport.
    let vp = MISC + 0x7000;
    for (i, w) in [0, 0, 32, 32, 0.0f32.to_bits(), 1.0f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(vp + 4 * i as u32, *w).unwrap();
    }
    d.set_viewport(&mut m, vp).unwrap();
    assert_eq!((d.viewport().width, d.viewport().height), (32, 32));
    // A null viewport keeps it (0x3f8110); an oversize one is clipped to
    // the target, as the original does.
    d.set_viewport(&mut m, 0).unwrap();
    assert_eq!((d.viewport().width, d.viewport().height), (32, 32));
    m.write_u32(vp + 8, 1000).unwrap();
    m.write_u32(vp + 12, 1000).unwrap();
    d.set_viewport(&mut m, vp).unwrap();
    assert_eq!((d.viewport().width, d.viewport().height), (W, H));

    // Back-buffer scaling stops; a channel-masked clear goes to the
    // backend's clear_rects with the mask.
    assert!(is_unimplemented(d.set_back_buffer_scale(&mut m, 0.5, 1.0)));
    d.set_back_buffer_scale(&mut m, 1.0, 1.0).unwrap();
    d.clear(&mut m, 0, 0, 0x80, 0x8000_0000, 1.0, 0).unwrap();
    match d.backend.calls.last() {
        Some(d3d::Recorded::ClearRects { clear, .. }) => {
            let (_, mask) = clear.color.unwrap();
            assert!(mask.alpha && !mask.red && !mask.green && !mask.blue);
            assert_eq!(clear.depth, None);
        }
        other => panic!("{other:?}"),
    }
}

// 10 -----------------------------------------------------------------------

#[test]
fn p1_10_render_to_texture_writes_back() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    d.backend.readback_fill = 0x5A;
    let surf = MISC + 0x7200;
    let mut h = SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let back = m.read_u32(DEV + 0x1A04).unwrap();
    m.write(0x8000_0000 | TEX_DATA, &[0x11; 256]).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    // The host surface starts from the level's texels.
    assert!(d.backend.calls.iter().any(
        |c| matches!(c, d3d::Recorded::WriteSurface { bytes, .. } if bytes.len() == 256 && bytes.iter().all(|b| *b == 0x11))
    ));
    m.write(0x8000_0000 | TEX_DATA, &[0; 256]).unwrap();
    // Nothing drawn yet: leaving the target writes nothing back.
    d.set_render_target(&mut m, back, 0).unwrap();
    assert_eq!(m.read_u32(0x8000_0000 | TEX_DATA).unwrap(), 0);
    // Drawn into, then left: the level's memory gets the surface's pixels.
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    assert!(
        d.backend
            .calls
            .iter()
            .any(|c| matches!(c, d3d::Recorded::ReadSurface { len: 256, .. }))
    );
    assert_eq!(m.read_u32(0x8000_0000 | TEX_DATA).unwrap(), 0x5A5A_5A5A);
    assert_eq!(
        m.read_u32(0x8000_0000 | (TEX_DATA + 252)).unwrap(),
        0x5A5A_5A5A
    );
    assert_eq!(d.stats().rtt_writebacks, 1);
    // A colour level bound as depth is refused (depth levels are covered
    // in tests/depth_textures.rs).
    let e = d.set_render_target(&mut m, back, surf);
    assert!(
        matches!(&e, Err(D3dError::Unimplemented { detail, .. }) if detail.contains("bound as depth/stencil after being a colour target")),
        "{e:?}"
    );
    // A free-standing surface is still fine.
    h.parent = 0;
    h.write(&mut m, surf).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
}

// 11, 12 -------------------------------------------------------------------

fn push_words(m: &mut SparseMemory, words: &[u32]) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

#[test]
fn p2_11_a_push_buffer_failure_latches_instead_of_skipping() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    bind_quad(&mut d, &mut m);
    inline_quad(&mut m, 0.0, 0.0, 8.0, 8.0, [255, 0, 0, 255]);
    let bad = m.read_u32(DEV).unwrap();
    push_words(&mut m, &[(1 << 18) | 0x1D70, 0]);
    inline_quad(&mut m, 8.0, 8.0, 16.0, 16.0, [0, 255, 0, 255]);
    let e = d.commit(&mut m).unwrap_err();
    assert!(
        matches!(
            e,
            D3dError::Push(PushError::UnknownMethod { method: 0x1D70, .. })
        ),
        "{e}"
    );
    assert!(d.fault().unwrap().contains(&format!("{bad:#010x}")));
    // A retry does not skip the failing command and run the rest.
    let e = d.commit(&mut m).unwrap_err();
    assert!(matches!(e, D3dError::Faulted(_)), "{e}");
    assert!(matches!(
        d.draw_vertices(&mut m, 6, 0, 4),
        Err(D3dError::Faulted(_))
    ));
}

#[test]
fn p2_12_reserved_header_bits_are_rejected() {
    let (mut d, mut m) = rec();
    push_words(&mut m, &[0x8004_0100, 0]); // "NOP" with bit 31 set
    let e = d.commit(&mut m).unwrap_err();
    assert!(
        matches!(
            e,
            D3dError::Push(PushError::Reserved {
                word: 0x8004_0100,
                ..
            })
        ),
        "{e}"
    );
    let (mut d, mut m) = rec();
    push_words(&mut m, &[(1 << 18) | method::NOP | 0x0002_0000, 0]);
    assert!(matches!(
        d.commit(&mut m),
        Err(D3dError::Push(PushError::Reserved { .. }))
    ));
}

/// Guest memory that refuses writes at or above `limit`, like a protected
/// page.
struct WriteLimit {
    inner: SparseMemory,
    limit: u32,
}

impl GuestMemory for WriteLimit {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.inner.read(addr, out)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        // Like the runtime's `write_bytes`: the whole access is checked
        // before any byte changes.
        if addr as u64 + data.len() as u64 > self.limit as u64 {
            return Err(d3d::GuestError {
                addr,
                len: data.len() as u32,
            });
        }
        self.inner.write(addr, data)
    }
}

/// A constant upload whose shadow runs into an unwritable page stores the
/// words before the boundary and faults on the first word past it (the
/// per-word behaviour), not on the whole access.
#[test]
fn constant_shadow_fault_keeps_the_words_before_it() {
    let (mut d, m) = rec();
    let shadow = d3d::GuestLayout::HALO2.vs_constants;
    // Register 140's shadow is 16 bytes at 0x407ff8; the page at 0x408000
    // is protected.
    let (reg, at) = (140u32, shadow + 16 * 140);
    assert_eq!(at, 0x0040_7FF8);
    let mut m = WriteLimit {
        inner: m,
        limit: 0x0040_8000,
    };
    for (i, w) in [0x1111_1111u32, 0x1111_1112, 0x1111_1113, 0x1111_1114]
        .iter()
        .enumerate()
    {
        m.inner.write_u32(SRC + 4 * i as u32, *w).unwrap();
    }
    let err = d
        .set_vertex_shader_constant(&mut m, reg, SRC, 1, true)
        .unwrap_err();
    assert!(
        matches!(
            err,
            D3dError::Guest(d3d::GuestError {
                addr: 0x0040_8000,
                len: 4
            })
        ),
        "{err:?}"
    );
    assert_eq!(m.inner.read_u32(0x0040_7FF8).unwrap(), 0x1111_1111);
    assert_eq!(m.inner.read_u32(0x0040_7FFC).unwrap(), 0x1111_1112);
    // The host copy was updated first, as before.
    assert_eq!(d.constant(reg as usize)[0].to_bits(), 0x1111_1111);
}
