//! The D3D fragments game code reaches directly and the display entries,
//! with synthetic values. Each test calls the entry through its LTCG
//! adapter and checks the push-buffer words, the device fields and the
//! returned registers against the original's effects. The addresses in the
//! comments are the XBE evidence (see `src/device/fragments.rs` and
//! `src/device/display.rs`). The image's pass-through programs are stood in
//! for by made-up words; no disc bytes are used.

use crate::common::*;
use d3d::abi::{Outcome, Regs};
use d3d::layout::device_field as f;
use d3d::{
    D3dError, Device, GuestLayout, GuestMemory, Recorded, RecordingBackend, SparseMemory,
    UploadPolicy,
};

const DEV: u32 = 0x0040_4FE0;
const L: GuestLayout = GuestLayout::HALO2;
const ZS: f32 = 16_777_215.0; // D24S8 (0x3fd970)

fn rec() -> (Device<RecordingBackend>, SparseMemory) {
    create(RecordingBackend::new(), UploadPolicy::Always)
}

fn fl(m: &SparseMemory, a: u32) -> f32 {
    f32::from_bits(m.read_u32(a).unwrap())
}

fn words(m: &SparseMemory, at: u32, n: u32) -> Vec<u32> {
    m.read_words(at, n as usize).unwrap()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|f| f.to_bits()).collect()
}

fn set_rs(m: &mut SparseMemory, slot: u32, v: u32) {
    m.write_u32(L.render_state_slot(slot), v).unwrap();
}

/// Viewport `{8, 4, 32, 16, 0.25, 0.75}` straight into `+0xEE0`.
fn viewport(m: &mut SparseMemory) {
    for (i, w) in [8, 4, 32, 16, 0.25f32.to_bits(), 0.75f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(DEV + f::VIEWPORT + 4 * i as u32, *w).unwrap();
    }
}

fn put(m: &SparseMemory) -> u32 {
    m.read_u32(DEV + f::PUT).unwrap()
}

/// `0x3f6190`: EAX = put, ESI = device; returns the new put.
fn push_viewport(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, at: u32) -> u32 {
    let o = call(
        d,
        m,
        0x3f6190,
        Regs {
            eax: at,
            esi: DEV,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    o.eax.expect("returns put")
}

#[test]
fn create_device_sets_up_the_state_the_fragments_read() {
    let (mut d, mut m) = rec();
    // 0x3fc421..: identity transforms, default FVF object, static object
    // cleared with flags 0x10, multisample 0 stored as 0x11, scales 1.
    assert_eq!(fl(&m, DEV + f::PROJECTION), 1.0);
    assert_eq!(fl(&m, DEV + f::PROJECTION + 4), 0.0);
    assert_eq!(
        m.read_u32(DEV + f::VERTEX_SHADER).unwrap(),
        L.default_vertex_shader
    );
    assert_eq!(m.read_u32(L.static_vertex_shader + 4).unwrap(), 0x10);
    assert_eq!(m.read_u32(DEV + f::MULTISAMPLE_TYPE).unwrap(), 0x11);
    for a in [f::SUPERSAMPLE_X, f::SUPERSAMPLE_Y, f::BACK_BUFFER_SCALE_X] {
        assert_eq!(fl(&m, DEV + a), 1.0);
    }
    assert_eq!(fl(&m, DEV + f::Z_SCALE), ZS);
    assert_eq!(fl(&m, DEV + f::SCREEN_OFFSET_X), 0.53125);
    assert_eq!(fl(&m, DEV + f::SCREEN_OFFSET_Y), 0.53125);
    assert_eq!(
        (
            m.read_u32(DEV + f::TARGET_WIDTH).unwrap(),
            m.read_u32(DEV + f::TARGET_HEIGHT).unwrap()
        ),
        (W, H)
    );
    // The viewport push reached the registers.
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().viewport_offset, [0.53125, 0.53125, 0.0, 0.0]);
    assert_eq!((d.hw().clip_min, d.hw().clip_max), (0.0, ZS));
}

#[test]
fn viewport_fragment_fixed_function() {
    let (mut d, mut m) = rec();
    viewport(&mut m);
    let at = put(&m);
    let end = push_viewport(&mut d, &mut m, at);
    let lo = (0.25f32 as f64 * ZS as f64) as f32;
    let hi = (0.75f32 as f64 * ZS as f64) as f32;
    let mut want = vec![0x0010_0A20];
    want.extend(bits(&[8.53125, 4.53125, 0.0, 0.0]));
    want.push(0x0008_0394);
    want.extend(bits(&[lo, hi]));
    assert_eq!(words(&m, at, 8), want);
    assert_eq!(end, at + 32);
    // The clip shadow at +0x1C08.
    assert_eq!(words(&m, DEV + f::CLIP_FIXED, 2), bits(&[lo, hi]));
    // Game-owned clip (+0x1C18 bit 0): pushed as it is, not rewritten.
    m.write_u32(DEV + f::CLIP_FIXED, 3.0f32.to_bits()).unwrap();
    m.write_u32(DEV + f::CLIP_FLAGS, 1).unwrap();
    let end = push_viewport(&mut d, &mut m, end);
    assert_eq!(words(&m, end - 8, 2), bits(&[3.0, hi]));
    assert_eq!(fl(&m, DEV + f::CLIP_FIXED), 3.0);
    m.write_u32(DEV + f::PUT, end).unwrap();
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().viewport_offset, [8.53125, 4.53125, 0.0, 0.0]);
    assert_eq!((d.hw().clip_min, d.hw().clip_max), (3.0, hi));
    // Not the device object: an error, not a guess.
    let e = call(
        &mut d,
        &mut m,
        0x3f6190,
        Regs {
            eax: end,
            esi: DEV + 4,
            ..Regs::default()
        },
        &[],
    );
    assert!(matches!(e, Err(D3dError::Invalid { .. })), "{e:?}");
}

#[test]
fn viewport_fragment_programmable_and_w_buffered() {
    let (mut d, mut m) = rec();
    viewport(&mut m);
    m.write_u32(DEV + f::VERTEX_SHADER, L.static_vertex_shader)
        .unwrap();
    let flags = m.read_u32(DEV + f::FLAGS).unwrap() & !0x200;
    m.write_u32(DEV + f::FLAGS, flags).unwrap();
    let at = put(&m);
    let end = push_viewport(&mut d, &mut m, at);
    // 0x3f6060: offset = [x + ox + w/2, y + oy + h/2, minZ*zs, 0],
    // scale = [w/2, -h/2, (maxZ-minZ)*zs, 0].
    let z0 = (0.25f32 as f64 * ZS as f64) as f32;
    let range = ((0.75f32 as f64 - 0.25f32 as f64) * ZS as f64) as f32;
    let mut want = vec![0x0010_0A20];
    want.extend(bits(&[24.53125, 12.53125, z0, 0.0]));
    want.push(0x0010_0AF0);
    want.extend(bits(&[16.0, -8.0, range, 0.0]));
    want.push(0x0008_0394);
    want.extend(bits(&[0.0, ZS]));
    assert_eq!(words(&m, at, 13), want);
    assert_eq!(end, at + 52);
    assert_eq!(words(&m, DEV + f::CLIP_PROGRAM, 2), bits(&[0.0, ZS]));
    m.write_u32(DEV + f::PUT, end).unwrap();
    d.commit(&mut m).unwrap();
    // The registers land in constants 59 and 58 too (xemu's model).
    assert_eq!(d.hw().viewport_scale, [16.0, -8.0, range, 0.0]);
    // Device flag 0x200: only the clip range.
    m.write_u32(DEV + f::FLAGS, flags | 0x200).unwrap();
    let at = put(&m);
    assert_eq!(push_viewport(&mut d, &mut m, at), at + 12);
    // W-buffered (ZENABLE = USEW): [944 * 93C * 948, 948].
    m.write_u32(DEV + f::W_NEAR, 2.0f32.to_bits()).unwrap();
    m.write_u32(DEV + f::INV_W_FAR, 0.001f32.to_bits()).unwrap();
    set_rs(&mut m, 143, 2);
    let at = put(&m);
    push_viewport(&mut d, &mut m, at);
    let w = (0.001f32 as f64 * 2.0 * ZS as f64) as f32;
    assert_eq!(words(&m, at, 3), [0x0008_0394, w.to_bits(), ZS.to_bits()]);
}

#[test]
fn control0_fragment() {
    let (mut d, mut m) = rec();
    let c0 = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory| -> (u32, Outcome) {
        let at = m.read_u32(DEV).unwrap();
        // 0x3f6390: EDX = put, [esp+4] = device, ret 4.
        let o = call(
            d,
            m,
            0x3f6390,
            Regs {
                edx: at,
                ..Regs::default()
            },
            &[DEV],
        )
        .unwrap();
        assert_eq!(o.eax, Some(at + 8));
        assert_eq!(m.read_u32(at).unwrap(), 0x0004_0290);
        (m.read_u32(at + 4).unwrap(), o)
    };
    assert_eq!(c0(&mut d, &mut m).0, 0x0010_0001);
    set_rs(&mut m, 143, 2);
    assert_eq!(c0(&mut d, &mut m).0, 0x0011_0001);
    set_rs(&mut m, 160, 1);
    assert_eq!(c0(&mut d, &mut m).0, 0x1011_0001);
    // A float depth format (F24S8, 0x2B).
    let z = MISC + 0x6000;
    m.write_u32(z + 12, 0x2B << 8).unwrap();
    m.write_u32(DEV + f::DEPTH_STENCIL, z).unwrap();
    set_rs(&mut m, 160, 0);
    set_rs(&mut m, 143, 1);
    assert_eq!(c0(&mut d, &mut m).0, 0x0010_1001);
}

#[test]
fn projection_viewport_fragment_builds_the_composite() {
    let (mut d, mut m) = rec();
    viewport(&mut m);
    // projection = diag(2, 2, 2, 1).
    for (i, v) in [2.0f32, 2.0, 2.0, 1.0].iter().enumerate() {
        m.write_u32(DEV + f::PROJECTION + 20 * i as u32, v.to_bits())
            .unwrap();
    }
    m.write_u32(L.dirty_flags, 0).unwrap();
    call(&mut d, &mut m, 0x3f6840, Regs::default(), &[]).unwrap();
    // Viewport matrix: diag(w/2, -h/2, range, 1), row 3 [w/2, h/2, z0, 1].
    let range = ((0.75f32 as f64 - 0.25f32 as f64) * ZS as f64) as f32;
    let z0 = (ZS as f64 * 0.25f32 as f64) as f32;
    let want = [
        32.0,
        0.0,
        0.0,
        0.0, //
        0.0,
        -16.0,
        0.0,
        0.0, //
        0.0,
        0.0,
        2.0 * range,
        0.0, //
        16.0,
        8.0,
        z0,
        1.0,
    ];
    assert_eq!(words(&m, DEV + f::COMPOSITE, 16), bits(&want));
    assert_eq!(m.read_u32(L.dirty_flags).unwrap() & 0x200, 0x200);
    // W-buffered: everything but the diagonal scales by k = 944 * 948.
    set_rs(&mut m, 143, 2);
    m.write_u32(DEV + f::INV_W_FAR, 0.5f32.to_bits()).unwrap();
    for (i, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
        m.write_u32(DEV + f::PROJECTION + 20 * i as u32, v.to_bits())
            .unwrap();
    }
    call(&mut d, &mut m, 0x3f6840, Regs::default(), &[]).unwrap();
    let k = 0.5 * ZS as f64;
    let c = words(&m, DEV + f::COMPOSITE, 16);
    assert_eq!(c[0], ((16.0 * k) as f32).to_bits());
    assert_eq!(c[5], ((-8.0 * k) as f32).to_bits());
    assert_eq!(c[10], ((k * range as f64) as f32).to_bits());
    assert_eq!(c[15], (k as f32).to_bits());
}

/// Made-up stand-ins for the three pass-through programs (`0x403E40` 48
/// words, `0x403D90` 44, `0x403CD0` 48).
fn passthrough_tables(m: &mut SparseMemory) {
    m.map(0x0040_3000, 0x1000);
    for (t, (addr, n)) in L.passthrough_programs.iter().enumerate() {
        for i in 0..*n {
            m.write_u32(addr + 4 * i, 0xA000_0000 | (t as u32) << 16 | i)
                .unwrap();
        }
    }
}

#[test]
fn passthrough_fragment_loads_constants_and_program() {
    let (mut d, mut m) = rec();
    passthrough_tables(&mut m);
    // Not pass-through: nothing pushed, EAX = the shader object
    // (`mov eax,[esi+794h]` at 0x3f6f40).
    let at = put(&m);
    let o = call(
        &mut d,
        &mut m,
        0x3f6f40,
        Regs {
            esi: DEV,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    assert_eq!((o.eax, put(&m)), (Some(L.default_vertex_shader), at));
    // A pass-through object (flags 2).
    let obj = MISC + 0x6100;
    m.write_u32(obj + 4, 2).unwrap();
    m.write_u32(DEV + f::VERTEX_SHADER, obj).unwrap();
    set_rs(&mut m, 93, 0);
    let o = call(
        &mut d,
        &mut m,
        0x3f6f40,
        Regs {
            esi: DEV,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    let (table, n) = L.passthrough_programs[0];
    let program = words(&m, table, n);
    let mut want = vec![0x0004_1EA4, 0, 0x0020_0B80];
    want.extend(bits(&[1.0, 1.0, ZS, 1.0, 0.53125, 0.53125, 0.0, 0.0]));
    want.extend([0x0004_1E9C, 0]);
    // 0x3f6b70: 32 words per header.
    want.push((32 << 18) | 0x0B00);
    want.extend_from_slice(&program[..32]);
    want.push((16 << 18) | 0x0B00);
    want.extend_from_slice(&program[32..]);
    assert_eq!(words(&m, at, want.len() as u32), want);
    let end = at + 4 * want.len() as u32;
    assert_eq!((o.eax, put(&m)), (Some(end), end));
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().program_load, 12, "48 words = 12 slots");
    assert_eq!(d.hw().constant_load, 2);
    // Fog table on with an affine projection: the second program; not
    // affine: the third. USEW makes constant 0.w = 948 * 944.
    set_rs(&mut m, 93, 1);
    set_rs(&mut m, 143, 2);
    m.write_u32(DEV + f::INV_W_FAR, 0.25f32.to_bits()).unwrap();
    let flags = m.read_u32(DEV + f::FLAGS).unwrap();
    for (affine, t) in [(true, 1), (false, 2)] {
        let fl = if affine { flags | 2 } else { flags & !2 };
        m.write_u32(DEV + f::FLAGS, fl).unwrap();
        let at = put(&m);
        call(
            &mut d,
            &mut m,
            0x3f6f40,
            Regs {
                esi: DEV,
                ..Regs::default()
            },
            &[],
        )
        .unwrap();
        let w = words(&m, at, 15);
        assert_eq!(w[6], ((ZS as f64 * 0.25) as f32).to_bits());
        assert_eq!(w[14], m.read_u32(L.passthrough_programs[t].0).unwrap());
    }
}

#[test]
fn zenable_usew_runs_the_fragments() {
    let (mut d, mut m) = rec();
    m.write_u32(DEV + f::INV_W_FAR, 0.5f32.to_bits()).unwrap();
    m.write_u32(DEV + f::W_NEAR, 4.0f32.to_bits()).unwrap();
    // SetRenderState_ZEnable(D3DZB_USEW): stdcall.
    call(&mut d, &mut m, 0x3f54f0, Regs::default(), &[2]).unwrap();
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().control0, Some(0x0011_0001));
    let w = (0.5f64 * 4.0 * ZS as f64) as f32;
    assert_eq!((d.hw().clip_min, d.hw().clip_max), (w, ZS));
    assert_eq!(fl(&m, DEV + f::COMPOSITE + 60), (0.5 * ZS as f64) as f32);
    // Back to TRUE: the plain state again.
    call(&mut d, &mut m, 0x3f54f0, Regs::default(), &[1]).unwrap();
    d.commit(&mut m).unwrap();
    assert_eq!(d.hw().control0, Some(0x0010_0001));
    assert_eq!(d.hw().clip_min, 0.0);
    assert_eq!(fl(&m, DEV + f::COMPOSITE + 60), 1.0);
}

#[test]
fn flicker_filter_updates_the_globals() {
    let (mut d, mut m) = rec();
    let (value, sent) = L.flicker_filter;
    let ff = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory, v: u32| {
        call(
            d,
            m,
            0x3f8c70,
            Regs {
                esi: v,
                ..Regs::default()
            },
            &[],
        )
        .unwrap();
    };
    ff(&mut d, &mut m, 5);
    assert_eq!(
        (m.read_u32(value).unwrap(), m.read_u32(sent).unwrap()),
        (1, 1)
    );
    assert_eq!(d.flicker_filter(), Some(5));
    // Same on/off value already sent: nothing changes.
    ff(&mut d, &mut m, 7);
    assert_eq!(d.flicker_filter(), Some(5));
    ff(&mut d, &mut m, 0);
    assert_eq!(
        (m.read_u32(value).unwrap(), m.read_u32(sent).unwrap()),
        (0, 1)
    );
    assert_eq!(d.flicker_filter(), Some(0));
}

#[test]
fn set_stipple_copies_and_pushes_the_pattern() {
    let (mut d, mut m) = rec();
    let block = MISC + 0x6200;
    let words_in: Vec<u32> = (0..32).map(|i| 0x0101_0101 * i).collect();
    for (i, w) in words_in.iter().enumerate() {
        m.write_u32(block + 4 * i as u32, *w).unwrap();
    }
    let at = put(&m);
    call(&mut d, &mut m, 0x3f8cc0, Regs::default(), &[block]).unwrap();
    assert_eq!(m.read_u32(at).unwrap(), 0x0080_1480);
    assert_eq!(words(&m, at + 4, 32), words_in);
    assert_eq!(put(&m), at + 33 * 4);
    assert_eq!(words(&m, DEV + f::STIPPLE, 32), words_in);
    d.commit(&mut m).unwrap();
    assert_eq!(d.stipple_pattern().to_vec(), words_in);
    // Device flag 0x10: pushed, not copied.
    let flags = m.read_u32(DEV + f::FLAGS).unwrap();
    m.write_u32(DEV + f::FLAGS, flags | 0x10).unwrap();
    m.write_u32(block, 0xFFFF).unwrap();
    call(&mut d, &mut m, 0x3f8cc0, Regs::default(), &[block]).unwrap();
    assert_eq!(m.read_u32(DEV + f::STIPPLE).unwrap(), 0);
    d.commit(&mut m).unwrap();
    assert_eq!(d.stipple_pattern()[0], 0xFFFF);
}

#[test]
fn persist_display_keeps_the_front_buffer() {
    let (mut d, mut m) = rec();
    m.write_u32(L.persist_word, 1).unwrap();
    m.write_u32(L.flicker_filter.1, 1).unwrap();
    let o = call(&mut d, &mut m, 0x3f8ad0, Regs::default(), &[]).unwrap();
    assert_eq!(o.eax, Some(0));
    assert_eq!(m.read_u32(L.persist_word).unwrap(), 0);
    assert_eq!(m.read_u32(L.flicker_filter.1).unwrap(), 0);
    // A copy of the image on screen (the front, before any swap), in a
    // surface of its own.
    let saved = d.persisted_surface().unwrap();
    assert_ne!(Some(saved), d.front_buffer());
    assert!(d.backend.calls.iter().any(|c| matches!(c,
        Recorded::CopySurface { src, dst } if Some(*src) == d.front_buffer() && *dst == saved)));
    // Again: the old copy is released.
    call(&mut d, &mut m, 0x3f8ad0, Regs::default(), &[]).unwrap();
    assert!(
        d.backend
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id } if *id == saved))
    );
    // No device reference: E_FAIL (0x3f8afd).
    m.write_u32(DEV + f::REF_COUNT, 0).unwrap();
    let o = call(&mut d, &mut m, 0x3f8ad0, Regs::default(), &[]).unwrap();
    assert_eq!(o.eax, Some(0x8000_4005));
}

#[test]
fn busy_check_clears_lock_unless_bound() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let busy = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory, r: u32| {
        call(
            d,
            m,
            0x3fb710,
            Regs {
                ecx: r,
                ..Regs::default()
            },
            &[],
        )
        .unwrap()
        .eax
        .unwrap()
    };
    m.write_u32(TEX_HEADER + 8, 0x1234).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 0);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 0, "Lock cleared");
    // An internal reference (Common & 0x780000): busy, Lock kept.
    let common = m.read_u32(TEX_HEADER).unwrap();
    m.write_u32(TEX_HEADER, common | 0x8_0000).unwrap();
    m.write_u32(TEX_HEADER + 8, 0x1234).unwrap();
    assert_eq!(busy(&mut d, &mut m, TEX_HEADER), 1);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 0x1234);
    // A surface level: its parent decides, and the parent's Lock is cleared.
    let surf = MISC + 0x6300;
    m.write_u32(surf, 1 | 0x5_0000).unwrap();
    m.write_u32(surf + 8, 0x77).unwrap();
    m.write_u32(surf + 0x14, TEX_HEADER).unwrap();
    assert_eq!(busy(&mut d, &mut m, surf), 1);
    m.write_u32(TEX_HEADER, common).unwrap();
    assert_eq!(busy(&mut d, &mut m, surf), 0);
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 0);
    assert_eq!(m.read_u32(surf + 8).unwrap(), 0x77);
}

/// Halo's clear wrapper `0x14b60`: `EAX` bits to `D3DCLEAR` flags.
fn wrapper_flags(eax: u32) -> u32 {
    (if eax & 1 != 0 { 0xF0 } else { 0 }) | ((eax & 0x1E) << 3) | ((eax >> 5) & 3)
}

#[test]
fn alpha_only_clears_from_the_wrapper_and_0x13c3f() {
    assert_eq!(wrapper_flags(0x10), 0x80);
    assert_eq!(wrapper_flags(1), 0xF0);
    assert_eq!(wrapper_flags(0x61), 0xF3);
    let (mut d, mut m) = rec();
    // 0x3c515: EAX = 0x10, color 0x00FFFFFF, Z 0, stencil 0.
    // 0x13c3f: Clear(0, 0, 0x80, 0, 1.0, 0).
    for (color, z) in [(0x00FF_FFFF, 0.0f32), (0, 1.0)] {
        call(
            &mut d,
            &mut m,
            0x3f9fc0,
            Regs::default(),
            &[0, 0, wrapper_flags(0x10), color, z.to_bits(), 0],
        )
        .unwrap();
        match d.backend.calls.last() {
            Some(Recorded::ClearRects { clear, .. }) => {
                let (rgba, mask) = clear.color.unwrap();
                assert!(mask.alpha && !mask.red && !mask.green && !mask.blue);
                assert_eq!(rgba[3], 0.0);
                assert_eq!((clear.depth, clear.stencil), (None, None));
                assert_eq!(clear.rects.len(), 1);
                assert!(clear.rects[0].is_whole(W, H));
            }
            other => panic!("{other:?}"),
        }
    }
    // The whole target with all channels stays a plain clear.
    call(
        &mut d,
        &mut m,
        0x3f9fc0,
        Regs::default(),
        &[0, 0, wrapper_flags(0x61), 0, 1.0f32.to_bits(), 0],
    )
    .unwrap();
    assert!(matches!(
        d.backend.calls.last(),
        Some(Recorded::Clear { .. })
    ));
}

fn gpu() -> Option<(Device<d3d::GfxBackend>, SparseMemory)> {
    gpu_with(create)
}

/// A copy-swap device on the GPU.
fn gpu_copy_swap() -> Option<(Device<d3d::GfxBackend>, SparseMemory)> {
    gpu_with(create_copy_swap)
}

fn gpu_with(
    make: fn(d3d::GfxBackend, UploadPolicy) -> (Device<d3d::GfxBackend>, SparseMemory),
) -> Option<(Device<d3d::GfxBackend>, SparseMemory)> {
    match gfx::GfxContext::headless() {
        Ok(ctx) => Some(make(d3d::GfxBackend::new(ctx), UploadPolicy::Always)),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

#[test]
fn gpu_alpha_only_clear_keeps_the_colour_channels() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    d.clear(&mut m, 0, 0, 0xF0, 0xFF10_2030, 1.0, 0).unwrap();
    let back = d.back_buffer().unwrap();
    // The A8R8G8B8 back buffer reads back in memory order: B, G, R, A.
    let (px, _) = d.backend.read_color(back).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0x30, 0x20, 0x10, 0xFF]);
    // 0x3c515's clear: alpha to 0 (colour 0x00FFFFFF), RGB untouched.
    d.clear(&mut m, 0, 0, wrapper_flags(0x10), 0x00FF_FFFF, 0.0, 0)
        .unwrap();
    let (px, _) = d.backend.read_color(back).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0x30, 0x20, 0x10, 0]);
    assert_eq!(pixel(&px, W - 1, H - 1), [0x30, 0x20, 0x10, 0]);
}

/// Eagerly the level reaches guest memory when the target changes; lazily
/// when the guest waits on the texture (`BlockOnResource`), and not before.
#[test]
fn gpu_render_to_texture_reaches_guest_memory() {
    for lazy in [false, true] {
        let Some((mut d, mut m)) = gpu() else {
            return;
        };
        d.set_rtt_direct(lazy);
        texture(&mut m, 0);
        let surf = MISC + 0x7200;
        let mut h =
            d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
        h.parent = TEX_HEADER;
        h.write(&mut m, surf).unwrap();
        let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
        d.set_render_target(&mut m, surf, 0).unwrap();
        d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        if lazy {
            d.swap(&mut m, 0).unwrap();
            d.kick_off(&mut m).unwrap();
            assert_eq!(d.stats().rtt_writebacks, 0, "kept on the host");
            assert_eq!(m.read_u32(0x8000_0000 | TEX_DATA).unwrap(), 0);
            d.block_on_resource(&mut m, TEX_HEADER).unwrap();
        }
        // A8R8G8B8 in memory: B, G, R, A.
        for i in [0u32, 63] {
            let a = 0x8000_0000 | (TEX_DATA + 4 * i);
            assert_eq!(
                m.read_u32(a).unwrap(),
                0xFFFF_0000,
                "texel {i}, lazy {lazy}"
            );
        }
        assert_eq!(d.stats().rtt_writebacks, 1);
    }
}

/// A 16x4 level of the test texture (texels 0x11) bound as the target.
fn rtt(upload: UploadPolicy) -> (Device<RecordingBackend>, SparseMemory, u32, u32) {
    let (mut d, mut m) = create(RecordingBackend::new(), upload);
    texture(&mut m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    m.write(0x8000_0000 | TEX_DATA, &[0x11; 256]).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    (d, m, surf, back)
}

fn filled_with(d: &Device<RecordingBackend>, v: u8) -> bool {
    d.backend
        .calls
        .iter()
        .any(|c| matches!(c, Recorded::WriteSurface { bytes, .. } if bytes.iter().all(|b| *b == v)))
}

#[test]
fn rtt_rebind_refills_from_changed_guest_texels() {
    for policy in [
        UploadPolicy::Always,
        UploadPolicy::Tracked,
        UploadPolicy::Verify,
    ] {
        let (mut d, mut m, surf, back) = rtt(policy);
        assert!(filled_with(&d, 0x11), "{policy:?}");
        d.set_render_target(&mut m, back, 0).unwrap();
        // The guest rewrites the level and reports it.
        m.write(0x8000_0000 | TEX_DATA, &[0x22; 256]).unwrap();
        d.note_write(0x8000_0000 | TEX_DATA, 256);
        d.backend.calls.clear();
        d.set_render_target(&mut m, surf, 0).unwrap();
        assert!(filled_with(&d, 0x22), "{policy:?}");
        assert!(d.take_events().is_empty(), "{policy:?}");
    }
    // Unchanged texels: only Always refills.
    for (policy, refill) in [
        (UploadPolicy::Always, true),
        (UploadPolicy::Tracked, false),
        (UploadPolicy::Verify, false),
    ] {
        let (mut d, mut m, surf, back) = rtt(policy);
        d.set_render_target(&mut m, back, 0).unwrap();
        d.backend.calls.clear();
        d.set_render_target(&mut m, surf, 0).unwrap();
        assert_eq!(filled_with(&d, 0x11), refill, "{policy:?}");
    }
    // Verify catches an unreported write.
    let (mut d, mut m, surf, back) = rtt(UploadPolicy::Verify);
    d.set_render_target(&mut m, back, 0).unwrap();
    m.write(0x8000_0000 | TEX_DATA, &[0x33; 256]).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    assert!(filled_with(&d, 0x33));
    assert!(matches!(
        d.take_events().as_slice(),
        [d3d::Event::MissedInvalidation { .. }]
    ));
}

#[test]
fn rtt_written_back_is_not_refilled_by_its_own_write() {
    let (mut d, mut m, surf, back) = rtt(UploadPolicy::Tracked);
    d.backend.readback_fill = 0x5A;
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
    let uploads = d.stats().rtt_uploads;
    d.set_render_target(&mut m, surf, 0).unwrap();
    assert_eq!(d.stats().rtt_uploads, uploads);
}

#[test]
fn teardown_writes_back_and_releases_rtt_surfaces() {
    let (mut d, mut m, _, _) = rtt(UploadPolicy::Always);
    let surface = d
        .backend
        .calls
        .iter()
        .find_map(|c| match c {
            Recorded::WriteSurface { surface, .. } => Some(*surface),
            _ => None,
        })
        .unwrap();
    d.backend.readback_fill = 0x5A;
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert_eq!(
        m.read_u32(0x8000_0000 | TEX_DATA).unwrap(),
        0x5A5A_5A5A,
        "dirty texels reach memory"
    );
    assert!(
        d.backend
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id } if *id == surface))
    );
}

#[test]
fn passthrough_fragment_without_a_push_returns_the_shader_object() {
    let (mut d, mut m) = rec();
    let vs = m.read_u32(DEV + f::VERTEX_SHADER).unwrap();
    let o = call(
        &mut d,
        &mut m,
        0x3f6f40,
        Regs {
            eax: 0x1234_5678,
            esi: DEV,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    assert_eq!(o.eax, Some(vs), "mov eax,[esi+794h] at 0x3f6f40");
}

#[test]
fn gpu_persist_display_keeps_what_a_copy_swap_showed() {
    let Some((mut d, mut m)) = gpu_copy_swap() else {
        return;
    };
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    let shown = d.back_buffer().unwrap();
    d.swap(&mut m, 0).unwrap(); // COPY | FINISH
    assert_eq!(d.backend.last_presented(), Some(shown));
    // A COPY swap leaves the ring as it is: the image is back0, not front.
    d.persist_display(&mut m).unwrap();
    let saved = d.persisted_surface().unwrap();
    let (px, _) = d.backend.read_color(saved).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0, 0, 255, 255], "B, G, R, A: red");
    d.release_device(&mut m, DEV).unwrap();
    let (px, _) = d.backend.read_color(saved).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0, 0, 255, 255], "kept through teardown");
}

#[test]
fn persist_display_survives_device_teardown() {
    let (mut d, mut m) = rec();
    d.persist_display(&mut m).unwrap();
    let saved = d.persisted_surface().unwrap();
    d.release_device(&mut m, DEV).unwrap();
    assert!(
        !d.backend
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id } if *id == saved))
    );
    assert_eq!(d.persisted_surface(), Some(saved));
}

#[test]
fn gpu_render_to_texture_starts_from_the_guest_texels() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    texture(&mut m, 0);
    // Red texels (A8R8G8B8 in memory: B, G, R, A).
    for i in 0..64u32 {
        m.write_u32(0x8000_0000 | (TEX_DATA + 4 * i), 0xFFFF_0000)
            .unwrap();
    }
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    d.set_render_target(&mut m, surf, 0).unwrap();
    // Clear only {0, 0, 4, 4} (D3DRECT x1, y1, x2, y2) to green.
    let rect = MISC + 0x7300;
    for (i, v) in [0u32, 0, 4, 4].iter().enumerate() {
        m.write_u32(rect + 4 * i as u32, *v).unwrap();
    }
    d.clear(&mut m, 1, rect, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    d.block_on_resource(&mut m, TEX_HEADER).unwrap();
    let texel = |m: &SparseMemory, x: u32, y: u32| {
        m.read_u32(0x8000_0000 | (TEX_DATA + 64 * y + 4 * x))
            .unwrap()
    };
    assert_eq!(texel(&m, 0, 0), 0xFF00_FF00, "cleared");
    assert_eq!(texel(&m, 3, 3), 0xFF00_FF00, "cleared");
    assert_eq!(texel(&m, 8, 0), 0xFFFF_0000, "kept from guest memory");
    assert_eq!(texel(&m, 15, 3), 0xFFFF_0000, "kept from guest memory");
}

#[test]
fn gpu_indexed_draw_renders_through_the_index_list() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    // Strip-order corners (0,0), (8,0), (0,8), (8,8) as a triangle list.
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFF00_00FF);
    bind_quad(&mut d, &mut m);
    let idx = MISC + 0x7400;
    for (i, v) in [0u16, 1, 2, 1, 3, 2].iter().enumerate() {
        m.write(idx + 2 * i as u32, &v.to_le_bytes()).unwrap();
    }
    m.write_u32(DEV + 0x1C, 0).unwrap(); // base vertex
    call(&mut d, &mut m, 0x3fafa0, Regs::default(), &[5, 6, idx]).unwrap();
    assert!(d.take_events().is_empty());
    let back = d.back_buffer().unwrap();
    let (px, _) = d.backend.read_color(back).unwrap();
    assert_eq!(pixel(&px, 2, 6), [255, 0, 0, 255], "blue (B, G, R, A)");
    assert_eq!(pixel(&px, 6, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&px, 12, 12), [0, 0, 0, 255], "outside");
}

#[test]
fn gpu_persist_display_keeps_a_copy_swap_image_drawn_over_later() {
    let Some((mut d, mut m)) = gpu_copy_swap() else {
        return;
    };
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 0).unwrap(); // COPY | FINISH: red is on screen
    // The next frame starts in the same back buffer, without a Swap.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.persist_display(&mut m).unwrap();
    let saved = d.persisted_surface().unwrap();
    let (px, _) = d.backend.read_color(saved).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0, 0, 255, 255], "the red image shown");
    let back = d.back_buffer().unwrap();
    let (px, _) = d.backend.read_color(back).unwrap();
    assert_eq!(pixel(&px, 0, 0), [0, 255, 0, 255], "back0 keeps drawing");
}

#[test]
fn copy_swap_preserves_the_shown_image_once_on_the_first_write() {
    let (mut d, mut m) = create_copy_swap(RecordingBackend::new(), UploadPolicy::Always);
    let back = d.back_buffer().unwrap();
    let front = d.front_buffer().unwrap();
    let copies = |d: &Device<RecordingBackend>| {
        d.backend
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::CopySurface { src, dst } if *src == back && *dst == front))
            .count()
    };
    d.swap(&mut m, 0).unwrap(); // COPY
    // Swaps that nothing draws over between cost nothing.
    d.swap(&mut m, 0).unwrap();
    assert_eq!(copies(&d), 0);
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(copies(&d), 1, "once, before the first write");
    // PersistDisplay now copies the front, which holds the shown image.
    d.persist_display(&mut m).unwrap();
    assert!(matches!(
        d.backend.calls.last(),
        Some(Recorded::CopySurface { src, .. }) if *src == front
    ));
}
