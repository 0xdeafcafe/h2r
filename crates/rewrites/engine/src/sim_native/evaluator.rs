//! Node evaluators: per channel (rotation, translation, scale), walk the
//! nodes whose group bit is set, call the channel codec through its slot,
//! and fold the result into the pose. Each variant is a separate guest
//! function selected by the stream state.
//!
//! A slot holding one of the native codecs runs that body here, with the
//! same return address pushed, so no guest call is made per node. Any other
//! slot is a guest call that resumes at its return site; all loop state is
//! in guest registers, the frame and the stream globals, as in the guest.
use super::pose::{FRAME_A, FRAME_B, ROTATION_OUT, ROTATION_WEIGHT};
use super::*;

const NODE_COUNT: u32 = 0x0050_445c;
const MASK: u32 = 0x0050_447c;
const SLOTS: [u32; 3] = [0x0050_4484, 0x0050_4488, 0x0050_448c];
const POSE: u32 = 0x0050_449c;
const GROUPS: [u32; 3] = [0x0050_4490, 0x0050_4494, 0x0050_4498];
const SECONDARY: [u32; 3] = [0x0050_44a4, 0x0050_44a8, 0x0050_44ac];
const SECONDARY_WEIGHT: u32 = 0x0050_44b0;
const TRACKS: [u32; 3] = [0x0050_44b4, 0x0050_44b8, 0x0050_44bc];
/// Codec outputs: primary sample, then the secondary one.
const PRIMARY_OUT: u32 = 0x0050_4430;
const SECONDARY_OUT: u32 = 0x0050_4410;
const SIGN_MASK: u32 = 0x0047_ffc0;

type Codec<M> = fn(&mut CpuAbi, &mut M) -> Result;

/// The codecs that have a native body.
fn native_codec<M: Memory>(target: u32) -> Option<Codec<M>> {
    use guest_layout::points::*;
    let body: Codec<M> = match target {
        t if t == ANIM_ROTATION_CODEC.base => super::pose::rotation_codec,
        t if t == ANIM_STATIC_ROTATION.base => super::codec::static_rotation,
        t if t == ANIM_FRAME_ROTATION.base => super::codec::frame_rotation,
        t if t == ANIM_BLEND_ROTATION.base => super::codec::blend_rotation,
        t if t == ANIM_KEYED_TRANSLATION.base => super::codec::keyed_translation,
        _ => return None,
    };
    Some(body)
}

/// `call dword ptr [slot]`: a native codec runs inline with the same
/// return address pushed; anything else is a guest call that resumes at
/// `resume`. The MMX codecs need the x87 state the leaves require.
fn codec<M: Memory>(
    c: &mut CpuAbi,
    g: &mut M,
    slot: u32,
    resume: u32,
) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    let target = g.u32(slot)?;
    match native_codec::<M>(target) {
        Some(body) if supported(c) => {
            push(c, g, resume)?;
            let back = body(c, g)?;
            debug_assert_eq!(back.target_pc, resume);
            Ok(None)
        }
        _ => call(c, g, target, resume).map(Some),
    }
}

/// `movaps` and packed memory operands: 16-byte aligned.
fn load16(g: &impl Memory, a: u32) -> std::result::Result<Vec4, GuestFault> {
    let mut b = [0; 16];
    g.read_bytes(a, &mut b, 16)?;
    Ok(unpack(u128::from_le_bytes(b)))
}

fn store16(g: &mut impl Memory, a: u32, v: Vec4) -> std::result::Result<(), GuestFault> {
    g.write_bytes(a, &pack(v).to_le_bytes(), 16)
}

/// `shufps d, s, imm`.
fn shuf(d: Vec4, s: Vec4, imm: usize) -> Vec4 {
    [
        d[imm & 3],
        d[(imm >> 2) & 3],
        s[(imm >> 4) & 3],
        s[(imm >> 6) & 3],
    ]
}

fn xor(a: Vec4, b: Vec4) -> Vec4 {
    std::array::from_fn(|i| a[i] ^ b[i])
}

fn frame(c: &CpuAbi, off: u32) -> u32 {
    at(c.esp, off)
}

fn ld(g: &impl Memory, c: &CpuAbi, off: u32) -> std::result::Result<u32, GuestFault> {
    g.u32(frame(c, off))
}

/// One group's end: `min(index + 8, count)` in `[esp+0x20]`.
fn group_end(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    index: u32,
) -> std::result::Result<u32, GuestFault> {
    c.ecx = at(index, 8);
    sub_flags(c, c.ecx, c.ebx, 32);
    g.set_u32(frame(c, 0x20), c.ebx)?;
    if c.ecx as i32 <= c.ebx as i32 {
        g.set_u32(frame(c, 0x20), c.ecx)?;
    }
    let end = ld(g, c, 0x20)?;
    sub_flags(c, index, end, 32);
    Ok(end)
}

/// Select the node's mask bit (`test [mask + 4*(i>>5)], 1 << i`).
fn masked(
    c: &mut CpuAbi,
    g: &impl Memory,
    index: u32,
) -> std::result::Result<(u32, u32, bool), GuestFault> {
    let mask = g.u32(MASK)?;
    let bit = 1u32 << (index & 0x1f);
    c.ecx = (index as i32 >> 5) as u32;
    let word = g.u32(at(mask, c.ecx.wrapping_mul(4)))?;
    logic_flags(c, word & bit);
    Ok((mask, bit, word & bit != 0))
}

/// Swap in the secondary frame, weight and output for the second sample.
fn secondary(g: &mut impl Memory) -> std::result::Result<u32, GuestFault> {
    let w2 = g.u32(SECONDARY[2])?;
    g.set_u32(ROTATION_WEIGHT, w2)?;
    g.set_u32(ROTATION_OUT, SECONDARY_OUT)?;
    Ok(w2)
}

pub const ADDITIVE_SITES: [u32; 6] = [0x280b82, 0x280bd2, 0x280dbf, 0x280e0f, 0x280f9e, 0x280ff2];

/// `280ac0`: an additive layer. Each channel samples the primary and the
/// secondary frames, blends them by `5044b0`, then multiplies rotations
/// into the pose, adds translations and multiplies scales.
pub fn additive(c: &mut CpuAbi, g: &mut impl Memory, mut pc: u32) -> Result {
    let entry = guest_layout::points::ANIM_ADDITIVE_NODES.base;
    loop {
        pc = match pc {
            _ if pc == entry => {
                push(c, g, c.ebp)?;
                c.ebp = c.esp;
                c.esp = (c.esp & !0xf).wrapping_sub(0x34);
                c.ecx = g.u32(POSE)?;
                c.eax = 0;
                let saved = c.ebx;
                push(c, g, saved)?;
                c.ebx = g.u32(NODE_COUNT)?;
                sub_flags(c, c.ebx, 0, 32);
                let saved = c.esi;
                push(c, g, saved)?;
                c.esi = g.u32(GROUPS[0])?;
                let saved = c.edi;
                push(c, g, saved)?;
                g.set_u32(frame(c, 0x18), c.ebx)?;
                for t in TRACKS {
                    g.set_u32(t, 0)?;
                }
                g.set_u32(frame(c, 0x14), c.ecx)?;
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                g.set_u32(frame(c, 0x10), 0)?;
                if c.ebx as i32 <= 0 {
                    0x280d26
                } else {
                    0x280b10
                }
            }
            // Rotation: group byte.
            0x280b10 => {
                c.edx = u32::from(g.u8(c.esi)?);
                c.esi = at(c.esi, 1);
                logic_flags(c, c.edx);
                g.set_u32(frame(c, 0x24), c.edx)?;
                g.set_u32(frame(c, 0x1c), c.esi)?;
                if c.edx == 0 {
                    c.ecx = ld(g, c, 0x14)?;
                    c.eax = at(c.eax, 8);
                    c.ecx = at(c.ecx, 0x100);
                    g.set_u32(frame(c, 0x10), c.eax)?;
                    g.set_u32(frame(c, 0x14), c.ecx)?;
                    0x280d1e
                } else {
                    let end = group_end(c, g, c.eax)?;
                    if c.eax as i32 >= end as i32 {
                        0x280d1e
                    } else {
                        0x280b53
                    }
                }
            }
            0x280b53 => {
                logic_flags(c, c.edx & 1);
                if c.edx & 1 == 0 {
                    0x280cf8
                } else {
                    let (mask, bit, set) = masked(c, g, c.eax)?;
                    c.edi = mask;
                    c.esi = bit;
                    if !set {
                        0x280cf2
                    } else {
                        match codec(c, g, SLOTS[0], ADDITIVE_SITES[0])? {
                            Some(t) => return Ok(t),
                            None => ADDITIVE_SITES[0],
                        }
                    }
                }
            }
            0x280b82 => {
                let w = g.u32(ROTATION_WEIGHT)?;
                c.edx = g.u32(SECONDARY[0])?;
                c.eax = g.u32(SECONDARY[1])?;
                c.esi = g.u32(FRAME_A)?;
                c.edi = g.u32(FRAME_B)?;
                g.set_u32(frame(c, 0x2c), w)?;
                g.set_u32(FRAME_A, c.edx)?;
                g.set_u32(FRAME_B, c.eax)?;
                let w2 = secondary(g)?;
                put_xmm(c, 0, scalar(w2));
                match codec(c, g, SLOTS[0], ADDITIVE_SITES[1])? {
                    Some(t) => return Ok(t),
                    None => ADDITIVE_SITES[1],
                }
            }
            0x280bd2 => {
                let fraction = g.u32(SECONDARY_WEIGHT)?;
                g.set_u32(frame(c, 0x28), fraction)?;
                c.eax = PRIMARY_OUT;
                c.ebx = SECONDARY_OUT;
                let a = load16(g, PRIMARY_OUT)?;
                let b = load16(g, SECONDARY_OUT)?;
                let product = vector(c, Mul, a, b);
                let old1 = xmm(c, 1);
                let high = [product[2], product[3], old1[2], old1[3]];
                let mut dot = vector(c, Add, product, high);
                let spread = dot[1];
                dot[0] = op(c, Add, dot[0], spread);
                let fraction = [ld(g, c, 0x28)?; 4];
                let sign_mask = load16(g, SIGN_MASK)?;
                let sign = [dot[0] & sign_mask[0]; 4];
                let flipped = xor(b, sign);
                let blend = vector(c, Sub, flipped, a);
                let blend = vector(c, Mul, blend, fraction);
                let blend = vector(c, Add, blend, a);
                let squares = vector(c, Mul, blend, blend);
                let upper = [squares[2], squares[3], a[2], a[3]];
                let mut norm = vector(c, Add, squares, upper);
                let spread = norm[1];
                norm[0] = op(c, Add, norm[0], spread);
                let inverse = [simd::rsqrt_approx(norm[0]); 4];
                let normalized = vector(c, Mul, blend, inverse);
                c.eax = PRIMARY_OUT;
                store16(g, PRIMARY_OUT, normalized)?;
                c.eax = 0;
                for (off, v) in [(0x30, 0), (0x34, 0), (0x38, 0), (0x3c, 0x8000_0000)] {
                    g.set_u32(frame(c, off), v)?;
                }
                // Quaternion product of the pose with the layer.
                c.eax = ld(g, c, 0x14)?;
                let p = load16(g, c.eax)?;
                c.eax = PRIMARY_OUT;
                let r = load16(g, PRIMARY_OUT)?;
                let mut x0 = shuf(p, p, 0x24);
                let x3 = [p[3]; 4];
                let x7 = shuf(r, r, 0x3f);
                let mut x3 = vector(c, Mul, x3, r);
                let x7 = vector(c, Mul, x7, x0);
                x0 = shuf(x0, x0, 0x49);
                let negate = load16(g, frame(c, 0x30))?;
                let x7 = xor(x7, negate);
                let x6 = shuf(r, r, 0x52);
                x3 = vector(c, Add, x3, x7);
                let x6 = vector(c, Mul, x6, x0);
                x0 = shuf(x0, x0, 0x49);
                let x6 = xor(x6, negate);
                let x1 = shuf(r, r, 0x89);
                x3 = vector(c, Add, x3, x6);
                x0 = vector(c, Mul, x0, x1);
                c.eax = ld(g, c, 0x14)?;
                x3 = vector(c, Sub, x3, x0);
                store16(g, c.eax, x3)?;
                let w = ld(g, c, 0x2c)?;
                c.edx = ld(g, c, 0x24)?;
                c.eax = ld(g, c, 0x10)?;
                c.ebx = ld(g, c, 0x18)?;
                g.set_u32(FRAME_A, c.esi)?;
                g.set_u32(FRAME_B, c.edi)?;
                g.set_u32(ROTATION_WEIGHT, w)?;
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                for (i, v) in [
                    (0, scalar(w)),
                    (1, x1),
                    (2, normalized),
                    (3, x3),
                    (6, x6),
                    (7, x7),
                ] {
                    put_xmm(c, i, v);
                }
                0x280cf2
            }
            0x280cf2 => {
                g.set_u32(TRACKS[0], at(g.u32(TRACKS[0])?, 1))?;
                0x280cf8
            }
            0x280cf8 => {
                c.ecx = at(ld(g, c, 0x14)?, 0x20);
                c.edx = (c.edx as i32 >> 1) as u32;
                g.set_u32(frame(c, 0x14), c.ecx)?;
                c.ecx = ld(g, c, 0x20)?;
                c.eax = at(c.eax, 1);
                sub_flags(c, c.eax, c.ecx, 32);
                g.set_u32(frame(c, 0x24), c.edx)?;
                g.set_u32(frame(c, 0x10), c.eax)?;
                if (c.eax as i32) < (c.ecx as i32) {
                    0x280b53
                } else {
                    c.esi = ld(g, c, 0x1c)?;
                    0x280d1e
                }
            }
            0x280d1e => {
                sub_flags(c, c.eax, c.ebx, 32);
                if (c.eax as i32) < (c.ebx as i32) {
                    0x280b10
                } else {
                    0x280d26
                }
            }
            // Translation.
            0x280d26 => {
                c.esi = g.u32(POSE)?;
                c.edi = g.u32(GROUPS[1])?;
                c.eax = 0;
                logic_flags(c, c.ebx);
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                g.set_u32(frame(c, 0x10), 0)?;
                if c.ebx as i32 <= 0 {
                    0x280f14
                } else {
                    0x280d50
                }
            }
            0x280d50 => {
                c.edx = u32::from(g.u8(c.edi)?);
                c.edi = at(c.edi, 1);
                logic_flags(c, c.edx);
                g.set_u32(frame(c, 0x24), c.edx)?;
                g.set_u32(frame(c, 0x1c), c.edi)?;
                if c.edx == 0 {
                    c.eax = at(c.eax, 8);
                    g.set_u32(frame(c, 0x10), c.eax)?;
                    c.esi = at(c.esi, 0x100);
                    0x280f0c
                } else {
                    let end = group_end(c, g, c.eax)?;
                    if c.eax as i32 >= end as i32 {
                        0x280f0c
                    } else {
                        0x280d90
                    }
                }
            }
            0x280d90 => {
                logic_flags(c, c.edx & 1);
                if c.edx & 1 == 0 {
                    0x280eee
                } else {
                    let (mask, bit, set) = masked(c, g, c.eax)?;
                    c.ebx = mask;
                    c.edi = bit;
                    if !set {
                        0x280edd
                    } else {
                        match codec(c, g, SLOTS[1], ADDITIVE_SITES[2])? {
                            Some(t) => return Ok(t),
                            None => ADDITIVE_SITES[2],
                        }
                    }
                }
            }
            0x280dbf => {
                let w = g.u32(ROTATION_WEIGHT)?;
                c.edx = g.u32(SECONDARY[0])?;
                c.eax = g.u32(SECONDARY[1])?;
                c.edi = g.u32(FRAME_A)?;
                c.ebx = g.u32(FRAME_B)?;
                g.set_u32(frame(c, 0x2c), w)?;
                g.set_u32(FRAME_A, c.edx)?;
                g.set_u32(FRAME_B, c.eax)?;
                let w2 = secondary(g)?;
                put_xmm(c, 0, scalar(w2));
                match codec(c, g, SLOTS[1], ADDITIVE_SITES[3])? {
                    Some(t) => return Ok(t),
                    None => ADDITIVE_SITES[3],
                }
            }
            0x280e0f => {
                let ax = g.u32(PRIMARY_OUT + 0x10)?;
                let fraction = g.u32(SECONDARY_WEIGHT)?;
                let bx = g.u32(SECONDARY_OUT + 0x10)?;
                let by = g.u32(SECONDARY_OUT + 0x14)?;
                c.edx = ld(g, c, 0x24)?;
                c.eax = ld(g, c, 0x10)?;
                let x = op(c, Sub, bx, ax);
                let x = op(c, Mul, x, fraction);
                let x = op(c, Add, x, ax);
                let ay = g.u32(PRIMARY_OUT + 0x14)?;
                let y = op(c, Sub, by, ay);
                let y = op(c, Mul, y, fraction);
                let y = op(c, Add, y, ay);
                let az = g.u32(PRIMARY_OUT + 0x18)?;
                g.set_u32(PRIMARY_OUT + 0x14, y)?;
                let bz = g.u32(SECONDARY_OUT + 0x18)?;
                g.set_u32(PRIMARY_OUT + 0x10, x)?;
                let z = op(c, Sub, bz, az);
                let z = op(c, Mul, z, fraction);
                let z = op(c, Add, z, az);
                g.set_u32(PRIMARY_OUT + 0x18, z)?;
                // Add the layer's translation to the pose, reloading the
                // second and third components as the guest does.
                for off in [0x10, 0x14, 0x18] {
                    let layer = if off == 0x10 {
                        x
                    } else {
                        g.u32(PRIMARY_OUT + off)?
                    };
                    let sum = op(c, Add, layer, g.u32(at(c.esi, off))?);
                    g.set_u32(at(c.esi, off), sum)?;
                }
                let w = ld(g, c, 0x2c)?;
                g.set_u32(FRAME_A, c.edi)?;
                g.set_u32(FRAME_B, c.ebx)?;
                g.set_u32(ROTATION_WEIGHT, w)?;
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                for (i, v) in [
                    (0, scalar(w)),
                    (1, scalar(fraction)),
                    (2, scalar(az)),
                    (3, scalar(z)),
                ] {
                    put_xmm(c, i, v);
                }
                0x280edd
            }
            0x280edd => {
                c.ecx = at(g.u32(TRACKS[1])?, 1);
                c.ebx = ld(g, c, 0x18)?;
                g.set_u32(TRACKS[1], c.ecx)?;
                0x280eee
            }
            0x280eee => {
                c.ecx = ld(g, c, 0x20)?;
                c.edx = (c.edx as i32 >> 1) as u32;
                c.esi = at(c.esi, 0x20);
                c.eax = at(c.eax, 1);
                sub_flags(c, c.eax, c.ecx, 32);
                g.set_u32(frame(c, 0x24), c.edx)?;
                g.set_u32(frame(c, 0x10), c.eax)?;
                if (c.eax as i32) < (c.ecx as i32) {
                    0x280d90
                } else {
                    c.edi = ld(g, c, 0x1c)?;
                    0x280f0c
                }
            }
            0x280f0c => {
                sub_flags(c, c.eax, c.ebx, 32);
                if (c.eax as i32) < (c.ebx as i32) {
                    0x280d50
                } else {
                    0x280f14
                }
            }
            // Scale.
            0x280f14 => {
                c.edi = g.u32(POSE)?;
                c.edx = g.u32(GROUPS[2])?;
                c.esi = 0;
                logic_flags(c, c.ebx);
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                if c.ebx as i32 <= 0 {
                    0x281083
                } else {
                    0x280f34
                }
            }
            0x280f34 => {
                c.eax = u32::from(g.u8(c.edx)?);
                c.edx = at(c.edx, 1);
                logic_flags(c, c.eax);
                g.set_u32(frame(c, 0x24), c.eax)?;
                g.set_u32(frame(c, 0x1c), c.edx)?;
                if c.eax == 0 {
                    c.esi = at(c.esi, 8);
                    c.edi = at(c.edi, 0x100);
                    0x28107b
                } else {
                    let end = group_end(c, g, c.esi)?;
                    if c.esi as i32 >= end as i32 {
                        0x28107b
                    } else {
                        0x280f70
                    }
                }
            }
            0x280f70 => {
                logic_flags(c, c.eax & 1);
                if c.eax & 1 == 0 {
                    0x281061
                } else {
                    let (mask, bit, set) = masked(c, g, c.esi)?;
                    c.ebx = mask;
                    c.edx = bit;
                    if !set {
                        0x281050
                    } else {
                        match codec(c, g, SLOTS[2], ADDITIVE_SITES[4])? {
                            Some(t) => return Ok(t),
                            None => ADDITIVE_SITES[4],
                        }
                    }
                }
            }
            0x280f9e => {
                let w = g.u32(ROTATION_WEIGHT)?;
                c.edx = g.u32(FRAME_B)?;
                c.eax = g.u32(SECONDARY[0])?;
                c.ecx = g.u32(SECONDARY[1])?;
                c.ebx = g.u32(FRAME_A)?;
                g.set_u32(frame(c, 0x28), w)?;
                g.set_u32(frame(c, 0x2c), c.edx)?;
                g.set_u32(FRAME_A, c.eax)?;
                g.set_u32(FRAME_B, c.ecx)?;
                let w2 = secondary(g)?;
                put_xmm(c, 0, scalar(w2));
                match codec(c, g, SLOTS[2], ADDITIVE_SITES[5])? {
                    Some(t) => return Ok(t),
                    None => ADDITIVE_SITES[5],
                }
            }
            0x280ff2 => {
                let a = g.u32(PRIMARY_OUT + 0x1c)?;
                let b = g.u32(SECONDARY_OUT + 0x1c)?;
                c.edx = ld(g, c, 0x2c)?;
                c.eax = ld(g, c, 0x24)?;
                let s = op(c, Sub, b, a);
                let s = op(c, Mul, s, g.u32(SECONDARY_WEIGHT)?);
                let s = op(c, Add, s, a);
                g.set_u32(PRIMARY_OUT + 0x1c, s)?;
                let scaled = op(c, Mul, s, g.u32(at(c.edi, 0x1c))?);
                g.set_u32(at(c.edi, 0x1c), scaled)?;
                let w = ld(g, c, 0x28)?;
                g.set_u32(FRAME_A, c.ebx)?;
                g.set_u32(FRAME_B, c.edx)?;
                g.set_u32(ROTATION_WEIGHT, w)?;
                g.set_u32(ROTATION_OUT, PRIMARY_OUT)?;
                put_xmm(c, 0, scalar(w));
                put_xmm(c, 1, scalar(a));
                0x281050
            }
            0x281050 => {
                c.ecx = at(g.u32(TRACKS[2])?, 1);
                c.ebx = ld(g, c, 0x18)?;
                g.set_u32(TRACKS[2], c.ecx)?;
                0x281061
            }
            0x281061 => {
                c.ecx = ld(g, c, 0x20)?;
                c.eax = (c.eax as i32 >> 1) as u32;
                c.edi = at(c.edi, 0x20);
                c.esi = at(c.esi, 1);
                sub_flags(c, c.esi, c.ecx, 32);
                g.set_u32(frame(c, 0x24), c.eax)?;
                if (c.esi as i32) < (c.ecx as i32) {
                    0x280f70
                } else {
                    c.edx = ld(g, c, 0x1c)?;
                    0x28107b
                }
            }
            0x28107b => {
                sub_flags(c, c.esi, c.ebx, 32);
                if (c.esi as i32) < (c.ebx as i32) {
                    0x280f34
                } else {
                    0x281083
                }
            }
            0x281083 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = c.ebp;
                c.ebp = pop(c, g)?;
                return ret(g, c, 0);
            }
            _ => unreachable!("additive evaluator entered at {pc:#x}"),
        };
    }
}
