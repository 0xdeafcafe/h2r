//! Directional overlay, blend weights and the final pair of animation layers.
use super::*;
use guest_layout::points::OBJECTS;

const ACTIVE_WEIGHT: u32 = 0x0044_54e4;
const INACTIVE_WEIGHT: u32 = 0x0045_dcd0;
const NEAR_ONE: u32 = 0x0045_dc78;

const SET_WEIGHT: u32 = 0x1d9240;
const GET_WEIGHT: u32 = 0x1d9370;
const SELECT: u32 = 0x1c6470;
const EVALUATE: u32 = 0x1c6f60;

type Checked<T> = std::result::Result<T, GuestFault>;

fn cmp_eq(c: &mut CpuAbi, a: u32, b: u32, width: u32) -> bool {
    sub_flags(c, a, b, width);
    c.eflags & 0x40 != 0
}

fn nz_byte(c: &mut CpuAbi, v: u8) -> bool {
    test_byte(c, v);
    v != 0
}

fn scalar_op(c: &mut CpuAbi, dst: usize, kind: Op, rhs: u32) {
    let mut reg = xmm(c, dst);
    reg[0] = op(c, kind, reg[0], rhs);
    put_xmm(c, dst, reg);
}

fn int_to_float(c: &mut CpuAbi, dst: usize, value: u32) {
    let before = u128::from_le_bytes(c.xmm[dst]);
    c.xmm[dst] = simd::cvtsi2ss(&mut c.mxcsr, before, value)
        .expect("masked SSE exceptions")
        .to_le_bytes();
}

/// The basis is three consecutive vectors at frame +74. Its third x
/// product is formed before the second dot product is stored.
fn project_direction(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    let s = c.esp;
    for (reg, offset) in [(2, 0x194), (1, 0x190), (0, 0x18c)] {
        put_xmm(c, reg, scalar(g.u32(at(c.esi, offset))?));
    }
    let [x, y, z] = [0, 1, 2].map(|i| xmm(c, i)[0]);
    let fz = g.u32(at(s, 0x7c))?;
    let fy = g.u32(at(s, 0x78))?;
    let pz = op(c, Mul, fz, z);
    let py = op(c, Mul, fy, y);
    let zy = op(c, Add, pz, py);
    let px = op(c, Mul, x, g.u32(at(s, 0x74))?);
    let forward = op(c, Add, zy, px);
    let sy = g.u32(at(s, 0x84))?;
    g.set_u32(at(s, 0x3c), forward)?;

    let pz = op(c, Mul, g.u32(at(s, 0x88))?, z);
    let py = op(c, Mul, sy, y);
    let zy = op(c, Add, pz, py);
    let px = op(c, Mul, x, g.u32(at(s, 0x80))?);
    put_xmm(c, 4, scalar(px));
    scalar_op(c, 0, Mul, g.u32(at(s, 0x8c))?);
    let side = op(c, Add, zy, px);
    g.set_u32(at(s, 0x40), side)?;

    let pz = op(c, Mul, g.u32(at(s, 0x94))?, z);
    let py = op(c, Mul, g.u32(at(s, 0x90))?, y);
    put_xmm(c, 2, scalar(py));
    let zy = op(c, Add, pz, py);
    let dot = op(c, Add, zy, xmm(c, 0)[0]);
    put_xmm(c, 3, scalar(dot));
    c.ecx = at(s, 0x3c);
    c.eax = at(s, 0x2c);
    g.set_u32(at(s, 0x44), dot)?;
    invoke(c, g, 0x11df30, 0x112b15)
}

/// Store angle deltas and the four signed extents of the blend.
fn set_extents(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    let flag = g.u8(c.ebp)? | (1 << 6);
    g.set_u8(c.ebp, flag)?;
    test_byte(c, flag);
    c.eax = g.u16(at(c.edx, 8))? as i16 as u32;
    for offset in [0x2c, 0x30] {
        let p = at(c.esp, offset);
        put_xmm(c, 0, scalar(g.u32(p)?));
        scalar_op(c, 0, Sub, g.u32(at(p, 8))?);
        g.set_u32(p, xmm(c, 0)[0])?;
    }
    put_xmm(c, 0, scalar(0));
    int_to_float(c, 1, c.eax);
    scalar_op(c, 1, Mul, g.u32(c.edx)?);
    put_xmm(c, 2, xmm(c, 0));
    scalar_op(c, 2, Sub, xmm(c, 1)[0]);
    g.set_u32(at(c.ebp, 0x14), xmm(c, 2)[0])?;

    c.ecx = g.u16(at(c.edx, 0xa))? as i16 as u32;
    int_to_float(c, 1, c.ecx);
    scalar_op(c, 1, Mul, g.u32(at(c.edx, 4))?);
    g.set_u32(at(c.ebp, 0x18), xmm(c, 1)[0])?;

    c.eax = g.u16(at(c.edx, 0x14))? as i16 as u32;
    int_to_float(c, 1, c.eax);
    scalar_op(c, 1, Mul, g.u32(at(c.edx, 0xc))?);
    c.eax = g.u32(at(c.esp, 0xe8))?;
    scalar_op(c, 0, Sub, xmm(c, 1)[0]);
    g.set_u32(at(c.ebp, 0x1c), xmm(c, 0)[0])?;

    c.ecx = g.u16(at(c.edx, 0x16))? as i16 as u32;
    int_to_float(c, 0, c.ecx);
    scalar_op(c, 0, Mul, g.u32(at(c.edx, 0x10))?);
    push(c, g, 5)?;
    g.set_u32(at(c.ebp, 0x20), xmm(c, 0)[0])?;
    invoke(c, g, 0x111590, 0x112ba0)
}

/// Both comparisons pop both x87 temporaries; the second one is reached
/// only when the first angle is strictly inside the tolerance.
fn angle_small(c: &mut CpuAbi, g: &impl Memory, offset: u32, last: u32) -> Checked<bool> {
    let mut x = ray::load_fpu(c);
    fp::ld_f32(&mut x, g.u32(at(c.esp, offset))?);
    magnitude_is_small(c, g, &mut x, last)
}

fn apply_direction(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    c.edx = g.u32(at(c.esp, 0xf4))?;
    c.eax = g.u32(at(c.esp, 0xf0))?;
    c.ecx = g.u32(at(c.esp, 0xec))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(c.esp, 0x24))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.esp, 0x38))?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(c.esp, 0x38))?;
    for v in [c.edx, c.eax, c.ecx] {
        push(c, g, v)?;
    }
    c.esi = at(c.esp, 0x60);
    invoke(c, g, 0x1c7100, 0x112c2b)
}

/// The same four argument values take three different register paths
/// into the evaluator. Those registers are visible to the callee.
fn evaluate(c: &mut CpuAbi, g: &mut impl Memory, site: u32) -> Segment {
    let last = g.u32(at(c.esp, 0xf4))?;
    let first = g.u32(at(c.esp, 0xf0))?;
    let count = g.u32(at(c.esp, 0xec))?;
    let (operand, resume) = match site {
        0x112cd8 => {
            (c.edx, c.eax, c.ecx) = (last, first, count);
            (0x112da5, 0x112dae)
        }
        0x112d2a => {
            (c.eax, c.ecx, c.edx) = (last, first, count);
            (0x112d47, 0x112d54)
        }
        0x112d84 => {
            (c.ecx, c.edx, c.eax) = (last, first, count);
            (0x112da5, 0x112dae)
        }
        _ => unreachable!(),
    };
    for v in [last, first, count] {
        push(c, g, v)?;
    }
    c.ebx = at(c.esp, 0x54);
    let weight = g.u32(operand)?;
    push(c, g, weight)?;
    invoke(c, g, EVALUATE, resume)
}

fn select_args(c: &mut CpuAbi, g: &mut impl Memory, handle: u32) -> Checked<()> {
    for arg in [c.ebx, c.ebx, c.ebx, c.ebx, handle] {
        push(c, g, arg)?;
    }
    Ok(())
}

pub(super) fn run(c: &mut CpuAbi, g: &mut impl Memory, mut pc: u32) -> Segment {
    loop {
        let s = c.esp;
        pc = match pc {
            0x1129c5 | 0x1129cd => {
                if pc == 0x1129c5 {
                    put_xmm(c, 0, scalar(g.u32(INACTIVE_WEIGHT)?));
                }
                c.ebx = low(c.ebx, g.u8(at(s, 0x12))?);
                g.set_u32(at(s, 0x24), xmm(c, 0)[0])?;
                c.eax = g.u32(at(s, 0x24))?;
                push(c, g, c.eax)?;
                c.ecx = at(c.ebp, 0x90);
                return invoke(c, g, SET_WEIGHT, 0x1129e7);
            }
            0x1129e7 => {
                c.esi = g.u32(at(s, 0x2c))?;
                c.ecx = low(c.ecx, g.u8(at(c.esi, 0x212))?);
                c.eax = u32::MAX;
                let no_seat = cmp_eq(c, c.ecx, c.eax, 8);
                g.set_u8(at(s, 0x12), 0)?;
                if no_seat && cmp_eq(c, g.u32(at(c.esi, 0x13c))?, c.eax, 32) {
                    0x112c37
                } else {
                    c.edi = g.u32(at(c.ebp, 0x114))?;
                    c.ecx = c.edi >> 16;
                    let none = cmp_eq(c, c.ecx, c.eax, 16);
                    g.set_u32(at(s, 0x20), c.edi)?;
                    if none {
                        0x112c37
                    } else {
                        c.eax = g.u32(at(s, 0x18))?;
                        push(c, g, c.edi)?;
                        c.ecx = at(c.esp, 0x4c);
                        push(c, g, c.ecx)?;
                        c.ebx = 0;
                        logic_flags(c, 0);
                        return invoke(c, g, 0x1cb490, 0x112a32);
                    }
                }
            }
            0x112a32 => {
                c.ecx = g.u32(at(s, 0x48))?;
                c.eax = u32::MAX;
                if cmp_eq(c, c.ecx, c.eax, 32)
                    || cmp_eq(c, u32::from(g.u16(at(s, 0x4e))?), c.eax, 16)
                {
                    0x112c37
                } else {
                    c.eax = g.u32(at(s, 0x68))?;
                    push(c, g, c.edi)?;
                    return invoke(c, g, 0x1daf30, 0x112a56);
                }
            }
            0x112a56 => {
                c.edx = c.eax;
                logic_flags(c, c.edx);
                if c.edx == 0 {
                    0x112c37
                } else {
                    return project_direction(c, g);
                }
            }
            0x112b15 => return set_extents(c, g),
            0x112ba0 => {
                c.ebx = low(c.ebx, c.eax as u8);
                c.eax = low(c.eax, g.u8(at(c.ebp, 0x8d))?);
                let active = nz_byte(c, c.eax as u8);
                g.set_u8(at(s, 0x12), c.ebx as u8)?;
                if active {
                    c.eax = at(c.ebp, 0x8c);
                    return invoke(c, g, GET_WEIGHT, 0x112bbb);
                }
                0x112c2b
            }
            0x112bbb => {
                let weight = xmm(c, 0)[0];
                let above = compare(c, weight, g.u32(SMALL)?) == 0;
                g.set_u32(at(s, 0x20), weight)?;
                if above {
                    let near_one = g.u32(NEAR_ONE)?;
                    put_xmm(c, 1, scalar(near_one));
                    if compare(c, near_one, weight) == 0
                        || !angle_small(c, g, 0x2c, 0x112be5)?
                        || !angle_small(c, g, 0x30, 0x112bf7)?
                    {
                        return apply_direction(c, g);
                    }
                }
                0x112c2b
            }
            0x112c2b | 0x112c37 => {
                if pc == 0x112c2b {
                    let active = nz_byte(c, c.ebx as u8);
                    put_xmm(c, 0, scalar(g.u32(ACTIVE_WEIGHT)?));
                    if !active {
                        put_xmm(c, 0, scalar(g.u32(INACTIVE_WEIGHT)?));
                    }
                } else {
                    put_xmm(c, 0, scalar(g.u32(INACTIVE_WEIGHT)?));
                }
                c.ebx = low(c.ebx, g.u8(at(s, 0x12))?);
                g.set_u32(at(s, 0x2c), xmm(c, 0)[0])?;
                c.edx = g.u32(at(s, 0x2c))?;
                push(c, g, c.edx)?;
                c.ecx = at(c.ebp, 0x8c);
                return invoke(c, g, SET_WEIGHT, 0x112c59);
            }
            0x112c59 | 0x112c5e => {
                if pc == 0x112c59 {
                    c.ebx = u32::MAX;
                }
                c.eax = g.u32(OBJECTS.base)?;
                c.ecx = g.u32(at(c.eax, 0x44))?;
                c.edx = g.u32(at(s, 0x6c))?;
                c.eax = g.u32(at(at(c.ecx, c.edx), 8))?;
                let paired = !cmp_eq(c, u32::from(g.u8(at(c.eax, 0x212))?), c.ebx, 8)
                    && !cmp_eq(c, u32::from(g.u8(at(c.eax, 0x213))?), c.ebx, 8);
                let empty = cmp_eq(c, u32::from(g.u16(at(c.ebp, 0x10e))?), c.ebx, 16);
                if paired {
                    if empty {
                        0x112d57
                    } else {
                        c.edx = g.u32(at(s, 0x18))?;
                        c.ecx = g.u32(at(c.edx, 0x68))?;
                        let no_graph = cmp_eq(c, c.ecx, c.ebx, 32);
                        c.eax = g.u32(at(c.ebp, 0x10c))?;
                        if !no_graph {
                            select_args(c, g, c.eax)?;
                            c.edx = 1 << 10;
                            c.eax = at(c.esp, 0x5c);
                            return invoke(c, g, SELECT, 0x112d2a);
                        }
                        0x112d57
                    }
                } else if empty {
                    0x112dae
                } else {
                    c.eax = low(c.eax, g.u8(at(c.ebp, 0x81))?);
                    c.esi = 0;
                    if nz_byte(c, c.eax as u8) {
                        c.eax = at(c.ebp, 0x80);
                        return invoke(c, g, GET_WEIGHT, 0x112ca2);
                    }
                    0x112cb4
                }
            }
            0x112ca2 => {
                let threshold = g.u32(SMALL)?;
                put_xmm(c, 1, scalar(threshold));
                if compare(c, threshold, xmm(c, 0)[0]) == 0 {
                    c.esi = 1 << 10;
                }
                0x112cb4
            }
            0x112cb4 => {
                c.eax = g.u32(at(s, 0x18))?;
                let no_graph = cmp_eq(c, g.u32(at(c.eax, 0x68))?, c.ebx, 32);
                c.ebp = g.u32(at(c.ebp, 0x10c))?;
                if !no_graph {
                    select_args(c, g, c.ebp)?;
                    c.ecx = at(c.esp, 0x5c);
                    push(c, g, c.ecx)?;
                    c.ebx = c.esi;
                    return invoke(c, g, 0x1cb410, 0x112cd8);
                }
                0x112dae
            }
            0x112cd8 | 0x112d2a | 0x112d84 => {
                if nz_byte(c, c.eax as u8) {
                    return evaluate(c, g, pc);
                }
                if pc == 0x112d2a { 0x112d57 } else { 0x112dae }
            }
            0x112d54 | 0x112d57 => {
                if pc == 0x112d54 {
                    c.ebx = u32::MAX;
                    logic_flags(c, c.ebx);
                }
                if !cmp_eq(c, u32::from(g.u16(at(c.ebp, 0x112))?), c.ebx, 16) {
                    c.eax = g.u32(at(s, 0x18))?;
                    c.ecx = g.u32(at(c.eax, 0x68))?;
                    let no_graph = cmp_eq(c, c.ecx, c.ebx, 32);
                    c.ebp = g.u32(at(c.ebp, 0x110))?;
                    if !no_graph {
                        select_args(c, g, c.ebp)?;
                        c.edx = 1 << 11;
                        c.eax = at(c.esp, 0x5c);
                        return invoke(c, g, SELECT, 0x112d84);
                    }
                }
                0x112dae
            }
            0x112dae => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                add_flags(c, c.esp, 0xd4);
                c.esp = at(c.esp, 0xd4);
                return ret(g, c, 16).map(Step::Transfer);
            }
            _ => unreachable!("invalid pose tail continuation {pc:#x}"),
        };
    }
}
