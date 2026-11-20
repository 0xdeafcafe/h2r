//! Animation channel codecs called once per node and channel by the node
//! evaluators. Rotations unpack four int16 lanes through MMX, convert them,
//! optionally blend two frames, then normalize with the Pentium III
//! reciprocal square root. The keyed translation codec binary-searches a
//! track's frame bytes and interpolates between two keys.
//!
//! MMX follows the lifter's contract: every instruction naming an MMX
//! register leaves TOP = 0 and all tags valid, and `emms` empties the tags.
use super::pose::{
    FRAME_A, FRAME_B, ROTATION_BLOCK, ROTATION_OUT, ROTATION_TRACK, ROTATION_WEIGHT,
};
use super::*;
use crate::function_eval::constant::ONE;

pub const TRANSLATION_TRACK: u32 = 0x0050_44b8;

fn set_mm(c: &mut CpuAbi, reg: usize, v: u64) {
    let st = &mut c.x87.st[reg];
    st[..8].copy_from_slice(&v.to_le_bytes());
    st[8] = 0xff;
    st[9] = 0xff;
}

fn enter_mmx(c: &mut CpuAbi) {
    c.x87.status &= !0x3800;
    c.x87.tag = 0;
}

/// `movq mm3, [p]`, `punpcklwd mm1, mm3`, `punpckhwd mm2, mm3`, then
/// `psrad` by 16 on both: each word sign-extended to a dword.
fn unpack_words(c: &mut CpuAbi, q: u64) -> (u64, u64) {
    let w = |i: u32| u64::from((q >> (16 * i)) as u16 as i16 as i32 as u32);
    let (lo, hi) = (w(0) | w(1) << 32, w(2) | w(3) << 32);
    set_mm(c, 3, q);
    set_mm(c, 1, lo);
    set_mm(c, 2, hi);
    enter_mmx(c);
    (lo, hi)
}

fn cvtpi2ps(c: &mut CpuAbi, reg: usize, mm: u64) -> Vec4 {
    let old = u128::from_le_bytes(c.xmm[reg]);
    let v = simd::cvtpi2ps(&mut c.mxcsr, old, mm).expect("masked SSE exceptions");
    c.xmm[reg] = v.to_le_bytes();
    enter_mmx(c);
    unpack(v)
}

/// `movlhps dst, src`: the low pair of each.
fn movlhps(c: &mut CpuAbi, dst: usize, src: usize) -> Vec4 {
    let (d, s) = (xmm(c, dst), xmm(c, src));
    let v = [d[0], d[1], s[0], s[1]];
    put_xmm(c, dst, v);
    v
}

/// The shared tail: square, two shuffled pair sums, reciprocal square
/// root, scale, aligned store. Leaves XMM0, XMM1, XMM3 and XMM4 as the
/// guest does.
fn normalize(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    q: Vec4,
    out: u32,
) -> std::result::Result<(), GuestFault> {
    let squares = vector(c, Mul, q, q);
    let swapped = [squares[2], squares[3], squares[0], squares[1]];
    let pairs = vector(c, Add, squares, swapped);
    let transposed = [pairs[1], pairs[0], pairs[1], pairs[0]];
    let sum = vector(c, Add, pairs, transposed);
    let inv = sum.map(simd::rsqrt_approx);
    let rotation = vector(c, Mul, q, inv);
    put_xmm(c, 3, swapped);
    put_xmm(c, 4, transposed);
    put_xmm(c, 0, inv);
    put_xmm(c, 1, rotation);
    g.write_bytes(out, &pack(rotation).to_le_bytes(), 16)
}

/// Convert one packed int16 quaternion at `p` into XMM1 (and XMM2's low
/// pair), as the single-frame codecs do.
fn quaternion(c: &mut CpuAbi, g: &impl Memory, p: u32) -> std::result::Result<Vec4, GuestFault> {
    let q = g.u64(p)?;
    let (lo, hi) = unpack_words(c, q);
    cvtpi2ps(c, 1, lo);
    cvtpi2ps(c, 2, hi);
    c.x87.tag = 0xffff; // emms
    Ok(movlhps(c, 1, 2))
}

/// Static rotation: one quaternion per track.
pub fn static_rotation(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let frame = c.esp.wrapping_sub(8);
    c.esp = frame;
    let track = g.u32(ROTATION_TRACK)?;
    let block = g.u32(ROTATION_BLOCK)?;
    c.edx = at(at(block, track.wrapping_mul(8)), 0x20);
    c.eax = g.u32(ROTATION_OUT)?;
    g.set_u32(at(frame, 4), c.edx)?;
    g.set_u32(frame, c.eax)?;
    c.ecx = g.u32(at(frame, 4))?;
    c.eax = g.u32(frame)?;
    let q = quaternion(c, g, c.ecx)?;
    normalize(c, g, q, c.eax)?;
    add_flags(c, frame, 8);
    c.esp = at(frame, 8);
    ret(g, c, 0)
}

/// Animated rotation at the current frame, without interpolation.
pub fn frame_rotation(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let frame = c.esp.wrapping_sub(8);
    c.esp = frame;
    let block = g.u32(ROTATION_BLOCK)?;
    c.edx = g.u32(at(block, 0x14))?.wrapping_mul(g.u32(ROTATION_TRACK)?);
    c.ecx = g.u32(ROTATION_OUT)?;
    c.edx = at(c.edx, block);
    c.eax = g.u32(FRAME_A)?;
    g.set_u32(frame, c.ecx)?;
    c.ecx = at(at(c.edx, c.eax.wrapping_mul(8)), 0x20);
    g.set_u32(at(frame, 4), c.ecx)?;
    c.ecx = g.u32(at(frame, 4))?;
    c.eax = g.u32(frame)?;
    let q = quaternion(c, g, c.ecx)?;
    normalize(c, g, q, c.eax)?;
    add_flags(c, frame, 8);
    c.esp = at(frame, 8);
    ret(g, c, 0)
}

/// Animated rotation between frames A and B by the stream weight.
pub fn blend_rotation(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let frame = c.esp.wrapping_sub(0x14);
    c.esp = frame;
    let block = g.u32(ROTATION_BLOCK)?;
    c.ecx = g.u32(at(block, 0x14))?.wrapping_mul(g.u32(ROTATION_TRACK)?);
    c.edx = g.u32(ROTATION_OUT)?;
    let weight = g.u32(ROTATION_WEIGHT)?;
    put_xmm(c, 0, scalar(weight));
    let one = g.u32(ONE)?;
    c.eax = at(at(c.ecx, block), 0x20);
    c.ecx = g.u32(FRAME_B)?;
    g.set_u32(at(frame, 8), c.edx)?;
    c.edx = at(c.eax, c.ecx.wrapping_mul(8));
    c.ecx = g.u32(FRAME_A)?;
    g.set_u32(at(frame, 12), c.edx)?;
    c.edx = at(c.eax, c.ecx.wrapping_mul(8));
    let rest = op(c, Sub, one, weight);
    put_xmm(c, 1, scalar(rest));
    g.set_u32(at(frame, 4), rest)?;
    g.set_u32(frame, weight)?;
    g.set_u32(at(frame, 16), c.edx)?;
    c.ecx = g.u32(at(frame, 16))?;
    c.edx = g.u32(at(frame, 12))?;
    c.eax = g.u32(at(frame, 8))?;

    let (lo, hi) = unpack_words(c, g.u64(c.ecx)?);
    cvtpi2ps(c, 1, lo);
    cvtpi2ps(c, 2, hi);
    let (lo, hi) = unpack_words(c, g.u64(c.edx)?);
    let rest = g.u32(at(frame, 4))?;
    let weight = g.u32(frame)?;
    put_xmm(c, 5, [rest; 4]);
    put_xmm(c, 6, [weight; 4]);
    cvtpi2ps(c, 3, lo);
    cvtpi2ps(c, 4, hi);
    let a = movlhps(c, 1, 2);
    let b = movlhps(c, 3, 4);
    let a = vector(c, Mul, a, [rest; 4]);
    let b = vector(c, Mul, b, [weight; 4]);
    let q = vector(c, Add, a, b);
    c.x87.tag = 0xffff; // emms
    c.eax = g.u32(at(frame, 8))?;
    normalize(c, g, q, c.eax)?;
    add_flags(c, frame, 0x14);
    c.esp = at(frame, 0x14);
    ret(g, c, 0)
}

/// Keyed translation: find the last key at or before frame A, copy it,
/// then interpolate towards the next key when there is one.
pub fn keyed_translation(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let frame = c.esp.wrapping_sub(0x14);
    c.esp = frame;
    let block = g.u32(ROTATION_BLOCK)?;
    c.eax = block;
    c.ecx = g.u32(at(block, 0xc))?;
    c.edx = g.u32(TRANSLATION_TRACK)?;
    c.ecx = at(c.ecx, c.edx.wrapping_mul(4));
    c.ecx = g.u32(at(c.ecx, block))?;
    let saved = c.ebx;
    push(c, g, saved)?;
    c.edx = c.ecx;
    c.ecx &= 0xfff;
    let saved = c.ebp;
    push(c, g, saved)?;
    let saved = c.esi;
    push(c, g, saved)?;
    c.esi = c.ecx;
    c.ecx = g.u32(at(block, 0x18))?;
    c.edx >>= 12;
    let saved = c.edi;
    push(c, g, saved)?;
    c.edi = g.u32(at(block, 0x24))?;
    c.ecx = at(c.ecx, c.edx);
    c.edx = c.edx.wrapping_mul(3);
    c.edi = at(c.edi, c.edx.wrapping_mul(4));
    c.ecx = at(c.ecx, block);
    c.edi = at(c.edi, block);
    c.eax = 0;
    g.set_u32(at(c.esp, 0x10), c.esi)?;
    if c.esi as i32 > 1 {
        c.ebx = g.u32(FRAME_A)?;
        loop {
            c.edx = (at(c.esi, c.eax) as i32 >> 1) as u32;
            c.ebp = u32::from(g.u8(at(c.edx, c.ecx))?);
            if c.ebp <= c.ebx {
                c.eax = c.edx;
            } else {
                c.esi = c.edx;
            }
            c.edx = at(c.eax, 1);
            if c.esi as i32 <= c.edx as i32 {
                break;
            }
        }
    }
    c.edx = c.eax.wrapping_mul(3);
    c.esi = at(c.edi, c.edx.wrapping_mul(4));
    c.edx = at(g.u32(ROTATION_OUT)?, 0x10);
    c.edi = c.esi;
    c.ebx = c.edx;
    for off in [0, 4] {
        c.ebp = g.u32(at(c.edi, off))?;
        g.set_u32(at(c.ebx, off), c.ebp)?;
    }
    c.edi = g.u32(at(c.edi, 8))?;
    c.ecx = at(c.ecx, c.eax);
    g.set_u32(at(c.ebx, 8), c.edi)?;
    c.edi = g.u32(at(c.esp, 0x10))?;
    c.eax = at(c.eax, 1);
    if (c.eax as i32) < (c.edi as i32) {
        c.eax = u32::from(g.u8(c.ecx)?);
        c.ecx = u32::from(g.u8(at(c.ecx, 1))?);
        let to_float = |c: &mut CpuAbi, reg: usize, v: u32| {
            let old = u128::from_le_bytes(c.xmm[reg]);
            let r = simd::cvtsi2ss(&mut c.mxcsr, old, v).expect("masked SSE exceptions");
            c.xmm[reg] = r.to_le_bytes();
            r as u32
        };
        let current = to_float(c, 1, g.u32(FRAME_A)?);
        let mut lane = xmm(c, 1);
        let time = op(c, Add, current, g.u32(ROTATION_WEIGHT)?);
        let start = to_float(c, 0, c.eax);
        c.ecx = c.ecx.wrapping_sub(c.eax);
        lane[0] = op(c, Sub, time, start);
        put_xmm(c, 1, lane);
        let span = to_float(c, 0, c.ecx);
        g.set_u32(at(c.esp, 0x10), span)?;
        let r = simd::rcp_approx(g.u32(at(c.esp, 0x10))?);
        g.set_u32(at(c.esp, 0x14), r)?;
        let r = g.u32(at(c.esp, 0x14))?;
        let t = op(c, Mul, r, lane[0]);
        put_xmm(c, 0, scalar(t));
        c.esi = at(c.esi, 0xc);
        c.eax = g.u32(c.esi)?;
        c.ecx = g.u32(at(c.esi, 4))?;
        g.set_u32(at(c.esp, 0x18), c.eax)?;
        c.eax = g.u32(at(c.esi, 8))?;
        for (off, slot) in [(0, 0x18), (4, 0x1c), (8, 0x20)] {
            match off {
                4 => g.set_u32(at(c.esp, slot), c.ecx)?,
                8 => g.set_u32(at(c.esp, slot), c.eax)?,
                _ => {}
            }
            let key = g.u32(at(c.esp, slot))?;
            let d = op(c, Sub, key, g.u32(at(c.edx, off))?);
            let m = op(c, Mul, d, t);
            let v = op(c, Add, m, g.u32(at(c.edx, off))?);
            g.set_u32(at(c.edx, off), v)?;
            put_xmm(c, 1, scalar(v));
        }
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    add_flags(c, frame, 0x14);
    c.esp = at(frame, 0x14);
    ret(g, c, 0)
}
