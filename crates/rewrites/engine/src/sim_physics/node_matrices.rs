//! Object node matrices (`bd090`), the per-object pose root of the post
//! update. It selects the orientation block, lets the animation callees
//! fill it, builds the root matrix, then walks the skeleton: each node's
//! quaternion, translation and scale becomes a matrix (`1421f0`) that is
//! composed in place with its parent's (`142a60`).
//!
//! Both leaves run here through the exact `sim_native` bodies, with the
//! same pushes, frame and return address the guest call would write, so
//! the skeleton walk makes no guest calls at all. Every other callee stays
//! a guest call that resumes here at its return site. The walk itself goes
//! to `crate::sim_anim::skeleton` (phase 1b)
//! first; [`nodes`] runs it when that declines.
use super::*;
use crate::sim_native;

const OBJECTS: u32 = points::OBJECTS.base;
const TAGS: u32 = points::TAG_ENTRIES.base;
const COMPONENTS: u32 = 0x0051_e9b8;
/// Orientation block used when the object has none of its own.
const SHARED_ORIENTATIONS: u32 = 0x004d_c2f0;
/// Pointer to the default orientation copied over the root of a model
/// without nodes.
const DEFAULT_ORIENTATION: u32 = 0x0046_87d8;
/// Static node mask and its one-time initialization bit.
const MASK: u32 = 0x0055_eeb4;
const MASK_INIT: u32 = 0x0055_eed4;
const MATRIX_BYTES: u32 = 0x34;
const NODE_BYTES: u32 = 0x60;
const ORIENTATION_BYTES: u32 = 0x20;
const NONE: u32 = u32::MAX;

// Guest callees, in call order.
const ROOT_SCALE: u32 = 0x000e_1670;
const COMPONENT_NODES: u32 = 0x001d_3920;
const PARENT_NODE: u32 = 0x000b_8bd0;
const SEED_ORIENTATIONS: u32 = 0x0016_d940;
const HAS_ANIMATION: u32 = 0x000b_f5a0;
const ATTACHED: u32 = 0x000b_fc90;
const MASK_EMPTY: u32 = 0x000b_ff10;
const MASK_INVERT: u32 = 0x000b_fe80;
const MASK_TRIM: u32 = 0x000b_fed0;
const ANIMATE: u32 = 0x000b_db60;
const SET_FLAG: u32 = 0x000b_fe20;
const ANIMATION_VALID: u32 = 0x000b_6780;
const ANIMATION_GRAPH: u32 = 0x001c_6440;
const MOTION_POSITION: u32 = 0x000a_98b0;
const MOTION_BASIS: u32 = 0x000a_9940;
const AFTER_ROOT: u32 = 0x0010_90d0;

/// Return sites of the guest callees. The inlined leaves (pose matrix,
/// composition and the root transform) have none.
pub(crate) const SITES: [u32; 16] = [
    0xbd191, 0xbd1ac, 0xbd24c, 0xbd2a6, 0xbd2b4, 0xbd2c3, 0xbd30c, 0xbd328, 0xbd336, 0xbd358,
    0xbd36a, 0xbd377, 0xbd382, 0xbd437, 0xbd467, 0xbd50c,
];

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn ld(g: &Guest, base: u32, off: u32) -> Result<u32> {
    g.u32(at(base, off))
}

fn st(g: &mut Guest, base: u32, off: u32, v: u32) -> Result<()> {
    g.set_u32(at(base, off), v)
}

fn word(g: &Guest, a: u32) -> Result<u32> {
    Ok(g.u16(a)? as i16 as i32 as u32)
}

fn test8(c: &mut CpuAbi, v: u32) -> bool {
    let v = v & 0xff;
    logic(c, v, 0x80);
    v != 0
}

fn cmp_none(c: &mut CpuAbi, v: u32) -> bool {
    arithmetic(c, v, NONE, true);
    v == NONE
}

fn and32(c: &mut CpuAbi, a: u32, b: u32) -> u32 {
    let v = a & b;
    logic(c, v, 1 << 31);
    v
}

fn step(c: &CpuAbi, width: u32) -> u32 {
    if c.eflags & 0x400 == 0 {
        width
    } else {
        width.wrapping_neg()
    }
}

/// `REP STOSD` then `REP STOSB` of EAX at EDI.
fn rep_stos(c: &mut CpuAbi, g: &mut Guest, width: u32) -> Result<()> {
    let d = step(c, width);
    while c.ecx != 0 {
        if width == 4 {
            g.set_u32(c.edi, c.eax)?
        } else {
            g.set_u8(c.edi, c.eax as u8)?
        }
        c.edi = c.edi.wrapping_add(d);
        c.ecx -= 1;
    }
    Ok(())
}

/// `REP MOVSD`, one dword at a time as the guest observes overlap.
fn rep_movsd(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let d = step(c, 4);
    while c.ecx != 0 {
        let v = g.u32(c.esi)?;
        g.set_u32(c.edi, v)?;
        c.esi = c.esi.wrapping_add(d);
        c.edi = c.edi.wrapping_add(d);
        c.ecx -= 1;
    }
    Ok(())
}

/// `call 1421f0` after the caller pushed the pose and the output matrix.
fn pose(c: &mut CpuAbi, g: &mut Guest, resume: u32) -> Result<()> {
    push(c, g, resume)?;
    let back = sim_native::pose_matrix(c, g)?;
    debug_assert_eq!(back.target_pc, resume);
    Ok(())
}

/// `call 142a60` with the parent in ECX, the child in EDX and the output
/// already pushed.
fn compose(c: &mut CpuAbi, g: &mut Guest, resume: u32) -> Result<()> {
    push(c, g, resume)?;
    let back = sim_native::compose(c, g)?;
    debug_assert_eq!(back.target_pc, resume);
    Ok(())
}

/// `call bdc40` after the caller pushed its five arguments.
fn root(c: &mut CpuAbi, g: &mut Guest, resume: u32) -> Result<()> {
    push(c, g, resume)?;
    let back = sim_native::root::transform(c, g)?;
    debug_assert_eq!(back.target_pc, resume);
    Ok(())
}

fn leave(c: &mut CpuAbi, g: &Guest) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0xe8, false);
    g.ret(c, 4)
}

/// Entry and every return site. Each arm is the guest block at that PC;
/// flags are modelled wherever a later call or return can observe them.
/// Shifts and multiplies whose flags are always overwritten first leave
/// them alone.
pub(crate) fn update(c: &mut CpuAbi, g: &mut Guest, mut pc: u32) -> Result<TransferAbi> {
    loop {
        pc = match pc {
            0xbd090 => {
                c.esp = c.esp.wrapping_sub(0xe8);
                for r in [c.ebx, c.ebp, c.esi, c.edi] {
                    push(c, g, r)?;
                }
                c.eax = ld(g, c.esp, 0xfc)? & 0xffff;
                c.ebp = g.u32(TAGS)?;
                c.esi = c.eax.wrapping_mul(3);
                c.eax = g.u32(OBJECTS)?;
                c.ecx = ld(g, c.eax, 0x44)?;
                c.esi <<= 2;
                c.ebx = ld(g, at(c.esi, c.ecx), 8)?;
                c.edx = (g.u32(c.ebx)? & 0xffff) << 4;
                c.ecx = ld(g, at(c.edx, c.ebp), 8)?;
                c.edx = word(g, at(c.ebx, 0x114))?;
                // The node count divides the matrix bytes by 52.
                let wide = 0x4ec4_ec4f_u64 * u64::from(c.edx);
                c.edx = (wide >> 32) as u32;
                c.eax = at(word(g, at(c.ebx, 0x116))?, c.ebx);
                st(g, c.esp, 0x1c, c.eax)?;
                let flags = ld(g, c.ebx, 4)? & 0xbfff_ffff;
                st(g, c.ebx, 4, flags)?;
                st(g, c.esp, 0x20, c.ecx)?;
                c.ecx = byte(c.ecx, g.u8(c.ecx)?);
                c.eax = 1u32.wrapping_shl(c.ecx);
                c.edi = c.edx >> 4;
                st(g, c.esp, 0x24, c.edi)?;
                let biped = test8(c, c.eax & 1);
                g.set_u8(at(c.esp, 0x12), 1)?;
                if biped {
                    c.ecx = g.u32(OBJECTS)?;
                    c.edx = ld(g, c.ecx, 0x44)?;
                    c.eax = ld(g, at(c.esi, c.edx), 8)?;
                    let state = g.u8(at(c.eax, 0x34b))?;
                    super::pose_layers::cmp(c, u32::from(state), 2, 8);
                    if state == 2 { 0xbd5f5 } else { 0xbd128 }
                } else {
                    0xbd128
                }
            }
            0xbd128 => {
                // Clear the node mask on the stack.
                let dwords = (c.edi.wrapping_add(0x1f) as i32 >> 5) as u32;
                c.ecx = dwords << 2;
                c.edx = c.ecx;
                c.ecx >>= 2;
                c.eax = 0;
                c.edi = at(c.esp, 0x70);
                rep_stos(c, g, 4)?;
                c.ecx = and32(c, c.edx, 3);
                rep_stos(c, g, 1)?;
                c.eax = ld(g, c.ebx, 0xb4)?;
                if cmp_none(c, c.eax) {
                    0xbd1b2
                } else {
                    let bits = g.u8(at(c.ebx, 0xc0))? >> 6;
                    c.ecx = byte(c.ecx, bits);
                    if test8(c, u32::from(bits & 1)) {
                        0xbd15e
                    } else {
                        0xbd1b2
                    }
                }
            }
            0xbd15e => {
                c.ecx = ld(g, c.esp, 0x20)?;
                c.ecx = byte(c.ecx, g.u8(c.ecx)?);
                c.edx = g.u32(COMPONENTS)?;
                c.ebp = ld(g, c.edx, 0x44)?;
                c.eax &= 0xffff;
                c.edi = c.eax.wrapping_mul(5);
                c.eax = 1u32.wrapping_shl(c.ecx);
                c.edi = at(c.edi << 5, c.ebp);
                if test8(c, c.eax & 1) {
                    c.eax = ld(g, c.esp, 0xfc)?;
                    return g.call(c, ROOT_SCALE, SITES[0]);
                }
                c.xmm[0] = [0; 16];
                0xbd196
            }
            0xbd191 => 0xbd196,
            0xbd196 => {
                let scale = u32::from_le_bytes(c.xmm[0][..4].try_into().unwrap());
                st(g, c.esp, 0x14, scale)?;
                c.edx = ld(g, c.esp, 0x14)?;
                push(c, g, c.edx)?;
                c.eax = at(c.esp, 0x74);
                push(c, g, c.eax)?;
                push(c, g, c.edi)?;
                return g.call(c, COMPONENT_NODES, SITES[1]);
            }
            0xbd1ac => {
                c.ebp = g.u32(TAGS)?;
                0xbd1b2
            }
            0xbd1b2 => {
                c.ecx = g.u32(OBJECTS)?;
                c.edi = ld(g, c.ecx, 0x44)?;
                c.edx = ld(g, at(c.edi, c.esi), 8)?;
                let block = g.u16(at(c.edx, 0x112))?;
                super::pose_layers::cmp(c, u32::from(block), 0xffff, 16);
                st(g, c.esp, 0x5c, c.edx)?;
                if block == 0xffff {
                    st(g, c.esp, 0x18, SHARED_ORIENTATIONS)?;
                    g.set_u8(at(c.esp, 0x12), 0)?;
                } else {
                    let offset = word(g, at(c.ebx, 0x112))?;
                    c.eax = arithmetic(c, offset, c.ebx, false);
                    st(g, c.esp, 0x18, c.eax)?;
                }
                c.eax = ld(g, c.esp, 0x20)?;
                c.eax = ld(g, c.eax, 0x38)?;
                if cmp_none(c, c.eax) {
                    0xbd593
                } else {
                    c.eax = and32(c, c.eax, 0xffff) << 4;
                    c.eax = ld(g, at(c.eax, c.ebp), 8)?;
                    st(g, c.esp, 0x60, c.eax)?;
                    c.eax = ld(g, c.eax, 4)?;
                    if cmp_none(c, c.eax) {
                        0xbd593
                    } else {
                        c.esi = ld(g, c.ebx, 0x14)?;
                        c.eax = (c.eax & 0xffff) << 4;
                        let orphan = cmp_none(c, c.esi);
                        c.ebp = ld(g, at(c.eax, c.ebp), 8)?;
                        st(g, c.esp, 0x14, c.ebp)?;
                        st(g, c.esp, 0x28, 0)?;
                        g.set_u8(at(c.esp, 0x11), 0)?;
                        g.set_u8(at(c.esp, 0x13), 0)?;
                        if orphan {
                            0xbd26d
                        } else {
                            let node = g.u8(at(c.ebx, 0x18))? as i8 as i16 as u16;
                            c.ecx = (c.ecx & 0xffff_0000) | u32::from(node);
                            c.eax = c.esi;
                            push(c, g, c.ecx)?;
                            return g.call(c, PARENT_NODE, SITES[2]);
                        }
                    }
                }
            }
            0xbd24c => {
                c.esi = and32(c, c.esi, 0xffff);
                st(g, c.esp, 0x28, c.eax)?;
                c.edx = c.esi.wrapping_mul(3);
                c.eax = ld(g, at(c.edi, c.edx.wrapping_mul(4)), 8)?;
                c.eax = ld(g, c.eax, 4)? >> 10;
                c.edx = ld(g, c.esp, 0x5c)?;
                let mirrored = (c.eax as u8) & 1;
                c.eax = byte(c.eax, mirrored);
                logic(c, u32::from(mirrored), 0x80);
                g.set_u8(at(c.esp, 0x11), mirrored)?;
                0xbd26d
            }
            0xbd26d => {
                c.ecx = ld(g, c.edx, 4)? >> 0x1d;
                if test8(c, c.ecx & 1) {
                    0xbd391
                } else {
                    c.ecx = ld(g, c.esp, 0x20)?;
                    c.ecx = byte(c.ecx, g.u8(c.ecx)?);
                    c.eax = 1u32.wrapping_shl(c.ecx);
                    let mut skip = false;
                    if test8(c, c.eax & 1) {
                        c.eax = byte(c.eax, g.u8(at(c.edx, 0x34b))?);
                        skip = test8(c, c.eax);
                    }
                    if skip {
                        0xbd391
                    } else {
                        c.edx = ld(g, c.esp, 0x18)?;
                        push(c, g, c.edx)?;
                        push(c, g, c.ebp)?;
                        return g.call(c, SEED_ORIENTATIONS, SITES[3]);
                    }
                }
            }
            0xbd2a6 => {
                c.esi = ld(g, c.esp, 0xfc)?;
                c.eax = c.esi;
                return g.call(c, HAS_ANIMATION, SITES[4]);
            }
            0xbd2b4 => {
                if !test8(c, c.eax) {
                    0xbd391
                } else {
                    c.ecx = c.esi;
                    return g.call(c, ATTACHED, SITES[5]);
                }
            }
            0xbd2c3 => {
                if test8(c, c.eax) {
                    0xbd391
                } else {
                    c.ebp = word(g, at(c.ebx, 0x12a))?;
                    c.eax = ld(g, at(c.ebp, c.ebx), 0x68)?;
                    c.ebp = arithmetic(c, c.ebp, c.ebx, false);
                    if cmp_none(c, c.eax) {
                        0xbd38d
                    } else {
                        let init = g.u8(MASK_INIT)?;
                        if !test8(c, u32::from(init & 1)) {
                            let v = g.u32(MASK_INIT)? | 1;
                            g.set_u32(MASK_INIT, v)?;
                            logic(c, v, 1 << 31);
                        }
                        c.eax = byte(c.eax, g.u8(at(c.esp, 0x12))?);
                        c.esi = 0;
                        logic(c, 0, 1 << 31);
                        if !test8(c, c.eax) {
                            0xbd33b
                        } else {
                            c.edi = ld(g, c.esp, 0x60)?;
                            c.edi = arithmetic(c, c.edi, 0xa4, false);
                            c.ecx = c.edi;
                            return g.call(c, MASK_EMPTY, SITES[6]);
                        }
                    }
                }
            }
            0xbd30c => {
                if test8(c, c.eax) {
                    0xbd33b
                } else {
                    c.esi = c.edi;
                    c.ecx = 8;
                    c.edi = MASK;
                    c.eax = MASK;
                    rep_movsd(c, g)?;
                    return g.call(c, MASK_INVERT, SITES[7]);
                }
            }
            0xbd328 => {
                c.eax = ld(g, c.esp, 0x24)?;
                c.esi = MASK;
                return g.call(c, MASK_TRIM, SITES[8]);
            }
            0xbd336 => {
                c.esi = MASK;
                0xbd33b
            }
            0xbd33b => {
                c.eax = ld(g, c.esp, 0x18)?;
                c.ecx = ld(g, c.esp, 0x14)?;
                c.edx = ld(g, c.esp, 0xfc)?;
                push(c, g, c.eax)?;
                c.eax = ld(g, c.esp, 0x28)?;
                push(c, g, c.ebp)?;
                push(c, g, c.ecx)?;
                c.ecx = c.esi;
                return g.call(c, ANIMATE, SITES[9]);
            }
            0xbd358 => {
                logic(c, c.esi, 1 << 31);
                c.edx = byte(c.edx, u8::from(c.esi != 0));
                c.ecx = 0x1e;
                c.eax = at(c.ebx, 4);
                return g.call(c, SET_FLAG, SITES[10]);
            }
            0xbd36a => {
                let animation = ld(g, c.ebp, 0x68)?;
                if cmp_none(c, animation) {
                    0xbd38d
                } else {
                    c.eax = c.ebp;
                    return g.call(c, ANIMATION_VALID, SITES[11]);
                }
            }
            0xbd377 => {
                if !test8(c, c.eax) {
                    0xbd38d
                } else {
                    c.ecx = c.ebp;
                    return g.call(c, ANIMATION_GRAPH, SITES[12]);
                }
            }
            0xbd382 => {
                let bit = (g.u8(at(c.eax, 0x16))? >> 1) & 1;
                c.eax = byte(c.eax, bit);
                logic(c, u32::from(bit), 0x80);
                g.set_u8(at(c.esp, 0x13), bit)?;
                0xbd38d
            }
            0xbd38d => {
                c.ebp = ld(g, c.esp, 0x14)?;
                0xbd391
            }
            0xbd391 => {
                c.eax = ld(g, c.ebp, 0x48)?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    c.esi = g.u32(DEFAULT_ORIENTATION)?;
                    c.edi = ld(g, c.esp, 0x18)?;
                    c.ecx = 8;
                    rep_movsd(c, g)?;
                }
                let mask = g.u8(at(c.esp, 0x70))?;
                if test8(c, u32::from(mask & 1)) {
                    0xbd50c
                } else {
                    c.eax = byte(c.eax, g.u8(at(c.esp, 0x13))?);
                    if test8(c, c.eax) {
                        // The animation already placed the root.
                        c.edx = ld(g, c.esp, 0x18)?;
                        c.eax = ld(g, c.esp, 0x1c)?;
                        push(c, g, c.edx)?;
                        push(c, g, c.eax)?;
                        pose(c, g, 0xbd3cb)?;
                        0xbd4fb
                    } else {
                        0xbd3d0
                    }
                }
            }
            0xbd3d0 => {
                c.ecx = at(c.ebx, 0x64);
                c.edx = g.u32(c.ecx)?;
                c.eax = ld(g, c.ecx, 4)?;
                c.ecx = ld(g, c.ecx, 8)?;
                st(g, c.esp, 0x44, c.edx)?;
                st(g, c.esp, 0x48, c.eax)?;
                c.edx = at(c.ebx, 0x70);
                c.eax = g.u32(c.edx)?;
                st(g, c.esp, 0x50, c.eax)?;
                st(g, c.esp, 0x4c, c.ecx)?;
                c.ecx = ld(g, c.edx, 4)?;
                c.edx = ld(g, c.edx, 8)?;
                c.eax = at(c.ebx, 0x7c);
                st(g, c.esp, 0x54, c.ecx)?;
                c.ecx = g.u32(c.eax)?;
                st(g, c.esp, 0x58, c.edx)?;
                c.edx = ld(g, c.eax, 4)?;
                c.eax = ld(g, c.eax, 8)?;
                st(g, c.esp, 0x40, c.eax)?;
                let motion = ld(g, c.ebx, 0xd4)?;
                let none = cmp_none(c, motion);
                st(g, c.esp, 0x38, c.ecx)?;
                st(g, c.esp, 0x3c, c.edx)?;
                if none || !test8(c, u32::from(g.u8(at(c.ebx, 0xd8))? & 1)) {
                    0xbd49b
                } else {
                    c.ecx = ld(g, c.esp, 0xfc)?;
                    c.esi = at(c.esp, 0x2c);
                    return g.call(c, MOTION_POSITION, SITES[13]);
                }
            }
            0xbd437 => {
                if test8(c, c.eax) {
                    c.ecx = ld(g, c.esp, 0x2c)?;
                    c.edx = ld(g, c.esp, 0x30)?;
                    c.eax = ld(g, c.esp, 0x34)?;
                    st(g, c.esp, 0x44, c.ecx)?;
                    st(g, c.esp, 0x48, c.edx)?;
                    st(g, c.esp, 0x4c, c.eax)?;
                }
                c.ecx = ld(g, c.esp, 0xfc)?;
                c.esi = at(c.esp, 0x64);
                c.edi = at(c.esp, 0x2c);
                return g.call(c, MOTION_BASIS, SITES[14]);
            }
            0xbd467 => {
                if test8(c, c.eax) {
                    c.ecx = ld(g, c.esp, 0x2c)?;
                    c.edx = ld(g, c.esp, 0x30)?;
                    c.eax = ld(g, c.esp, 0x34)?;
                    st(g, c.esp, 0x50, c.ecx)?;
                    c.ecx = ld(g, c.esp, 0x64)?;
                    st(g, c.esp, 0x54, c.edx)?;
                    c.edx = ld(g, c.esp, 0x68)?;
                    st(g, c.esp, 0x58, c.eax)?;
                    c.eax = ld(g, c.esp, 0x6c)?;
                    st(g, c.esp, 0x38, c.ecx)?;
                    st(g, c.esp, 0x3c, c.edx)?;
                    st(g, c.esp, 0x40, c.eax)?;
                }
                0xbd49b
            }
            0xbd49b => {
                c.eax = ld(g, c.ebx, 4)?;
                c.edx = ld(g, c.esp, 0x28)?;
                c.ecx = at(c.esp, 0xc4);
                push(c, g, c.ecx)?;
                c.ecx = ld(g, c.ebx, 0xa0)?;
                c.ebx = byte(c.ebx, g.u8(at(c.esp, 0x15))?);
                push(c, g, c.edx)?;
                c.eax = and32(c, c.eax >> 10, 0xffff_ff01);
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
                c.edx = at(c.esp, 0x54);
                push(c, g, c.edx)?;
                c.eax = at(c.esp, 0x4c);
                c.ecx = at(c.esp, 0x64);
                root(c, g, 0xbd4d1)?;
                0xbd4d1
            }
            0xbd4d1 => {
                // Root: local pose, then the root transform as its parent.
                c.eax = ld(g, c.esp, 0x18)?;
                push(c, g, c.eax)?;
                c.ecx = at(c.esp, 0x94);
                push(c, g, c.ecx)?;
                pose(c, g, 0xbd4e3)?;
                c.edx = ld(g, c.esp, 0x1c)?;
                push(c, g, c.edx)?;
                c.edx = at(c.esp, 0x94);
                c.ecx = at(c.esp, 0xc8);
                compose(c, g, 0xbd4fb)?;
                0xbd4fb
            }
            0xbd4fb => {
                c.eax = ld(g, c.esp, 0x1c)?;
                c.ebx = ld(g, c.esp, 0xfc)?;
                push(c, g, c.eax)?;
                return g.call(c, AFTER_ROOT, SITES[15]);
            }
            0xbd50c => {
                c.eax = ld(g, c.ebp, 0x48)?;
                logic(c, c.eax, 1 << 31);
                if c.eax as i32 <= 0 {
                    0xbd5f5
                } else {
                    c.esi = 1;
                    arithmetic(c, c.eax, c.esi, true);
                    if c.eax as i32 <= 1 {
                        0xbd5f5
                    } else {
                        // Phase 1b: the whole walk in one Rust call,
                        // or this loop when it declines (a
                        // `skeleton::Decline`).
                        if !crate::sim_anim::skeleton::walk(c, g)? {
                            nodes(c, g)?;
                        }
                        return leave(c, g);
                    }
                }
            }
            0xbd593 => {
                c.esi = ld(g, c.ebx, 0x14)?;
                c.eax = 0;
                c.edx = byte(c.edx, 0);
                if !cmp_none(c, c.esi) {
                    c.eax = c.esi & 0xffff;
                    c.edx = c.eax.wrapping_mul(3);
                    c.eax = ld(g, c.ecx, 0x44)?;
                    c.ecx = ld(g, at(c.eax, c.edx.wrapping_mul(4)), 8)?;
                    c.edx = g.u8(at(c.ebx, 0x18))? as i8 as i32 as u32;
                    c.eax = word(g, at(c.ecx, 0x116))?;
                    c.edx = at(c.edx.wrapping_mul(MATRIX_BYTES), c.ecx);
                    c.eax = at(c.eax, c.edx);
                    c.edx = ld(g, c.ecx, 4)? >> 10;
                    c.edx = byte(c.edx, (c.edx as u8) & 1);
                }
                c.ecx = ld(g, c.esp, 0x1c)?;
                push(c, g, c.ecx)?;
                c.ecx = ld(g, c.ebx, 0xa0)?;
                push(c, g, c.eax)?;
                c.eax = ld(g, c.ebx, 4)? >> 10;
                c.eax &= 0xffff_ff01;
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
                c.eax = at(c.ebx, 0x7c);
                c.ecx = at(c.ebx, 0x70);
                c.ebx = arithmetic(c, c.ebx, 0x64, false);
                push(c, g, c.ebx)?;
                c.ebx = byte(c.ebx, c.edx as u8);
                root(c, g, 0xbd5f5)?;
                0xbd5f5
            }
            0xbd5f5 => return leave(c, g),
            _ => unreachable!("node matrices entered at {pc:#x}"),
        };
    }
}

/// The skeleton walk from node 1, one node at a time through the inline
/// leaves. A set bit in the stack mask keeps the node's previous matrix.
/// Flags inside the loop are never observed: the epilogue's `add esp`
/// replaces them. `crate::sim_anim::skeleton` runs the same walk as one
/// kernel call and falls back to this loop; its tests hold the two equal.
pub(crate) fn nodes(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.edx = at(ld(g, c.esp, 0x1c)?, MATRIX_BYTES);
    c.ebp = at(ld(g, c.esp, 0x18)?, ORIENTATION_BYTES);
    c.ebx = NODE_BYTES;
    loop {
        c.ecx = ld(g, c.esp, 0x14)?;
        c.edi = ld(g, c.ecx, 0x4c)?;
        c.ecx = c.esi & 0x1f;
        c.eax = 1 << c.ecx;
        c.ecx = (c.esi as i32 >> 5) as u32;
        c.edi = at(c.edi, c.ebx);
        let kept = ld(g, c.esp, c.ecx.wrapping_mul(4).wrapping_add(0x70))? & c.eax;
        if kept == 0 {
            push(c, g, c.ebp)?;
            push(c, g, c.edx)?;
            pose(c, g, 0xbd55e)?;
            c.ecx = word(g, at(c.edi, 4))?;
            c.eax = ld(g, c.esp, 0x1c)?;
            c.ecx = c.ecx.wrapping_mul(MATRIX_BYTES);
            push(c, g, c.edx)?;
            c.ecx = at(c.ecx, c.eax);
            compose(c, g, 0xbd571)?;
        }
        c.eax = ld(g, c.esp, 0x14)?;
        c.ecx = ld(g, c.eax, 0x48)?;
        c.esi = at(c.esi, 1);
        c.ebx = at(c.ebx, NODE_BYTES);
        c.ebp = at(c.ebp, ORIENTATION_BYTES);
        c.edx = at(c.edx, MATRIX_BYTES);
        if (c.esi as i32) >= (c.ecx as i32) {
            return Ok(());
        }
    }
}
