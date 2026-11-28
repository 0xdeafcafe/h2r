use super::*;
use guest_layout::points;
use x87::abi as fp;

const AFTER_SUBTREE: u32 = 0x001d_e7dd;
const TEST_SURFACE: u32 = 0x001d_ea00;
const AFTER_SURFACE: u32 = 0x001d_e921;
const STORE_FRACTION: u32 = 0x001d_e7bf;

fn fraction(
    c: &mut CpuAbi,
    g: &impl Memory,
    dist: u32,
    delta: u32,
    addr: u32,
) -> std::result::Result<u32, GuestFault> {
    // The denominator is consumed as a single. Division stays extended
    // until after FCHS, including under directed rounding modes.
    let mut x = ray::load_fpu(c);
    fp::ld_f32(&mut x, dist);
    fp::arith(
        &mut x,
        fp::arith_op::DIV,
        0,
        fp::src_kind::F32,
        u64::from(delta),
    );
    fp::unary(&mut x, fp::unary_op::CHS);
    let (_, store) = fp::st_f32(&x, 0);
    fp::store_commit(&mut x, store.status, true);
    ray::finish_fpu(c, g, &x, STORE_FRACTION, addr)?;
    Ok(store.value)
}

/// Walk a segment through the packed BSP. At a crossing, recurse with
/// the rounded split fraction and the original post-recursion return PC.
pub fn segment(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.eax = g.u32(at(c.esp, 8))?;
    c.esp = c.esp.wrapping_sub(8);
    for saved in [c.ebx, c.ebp] {
        push(c, g, saved)?;
    }
    c.ebp = g.u32(at(c.esp, 20))?;
    let saved = c.esi;
    push(c, g, saved)?;
    let sp = c.esp;
    for depth in 0.. {
        if c.eax & 0x80_0000 != 0 {
            break;
        }
        if depth == 256 {
            const NEXT_NODE: u32 = 0x001d_e6f0;
            logic_flags(c, 0);
            return Ok(TransferAbi::resume(NEXT_NODE));
        }
        let bsp = g.u32(at(c.ebp, 4))?;
        let nodes = g.u32(at(bsp, 4))?;
        c.esi = at(nodes, (c.eax & 0x7f_ffff) * 8);
        let plane_index = g.u16(c.esi)? as i16 as u32;
        c.edx = g.u32(at(bsp, 12))?;
        let plane = at(c.edx, plane_index << 4);
        let p = g.u32(at(c.ebp, 16))?;
        let d = g.u32(at(c.ebp, 20))?;
        let normal = [g.u32(plane)?, g.u32(at(plane, 4))?, g.u32(at(plane, 8))?];
        let y = op(c, Mul, g.u32(at(p, 4))?, normal[1]);
        let z = op(c, Mul, g.u32(at(p, 8))?, normal[2]);
        let zy = op(c, Add, z, y);
        let x = op(c, Mul, normal[0], g.u32(p)?);
        let dot = op(c, Add, zy, x);
        let dist = op(c, Sub, dot, g.u32(at(plane, 12))?);
        let y = op(c, Mul, g.u32(at(d, 4))?, normal[1]);
        let z = op(c, Mul, g.u32(at(d, 8))?, normal[2]);
        let zy = op(c, Add, z, y);
        let x = op(c, Mul, g.u32(d)?, normal[0]);
        let delta = op(c, Add, zy, x);
        let end = op(c, Mul, delta, g.u32(at(sp, 36))?);
        let start = op(c, Mul, delta, g.u32(at(sp, 32))?);
        let start = op(c, Add, start, dist);
        g.set_u32(at(sp, 24), dist)?;
        let end = op(c, Add, end, dist);
        g.set_u32(at(sp, 16), delta)?;
        for (i, v) in [0, delta, end, start].into_iter().enumerate() {
            put_xmm(c, i, scalar(v));
        }
        let lo = u32::from(compare(c, start, 0) & 1 == 0);
        let hi = u32::from(compare(c, end, 0) & 1 == 0);
        if lo != hi {
            let increasing = compare(c, delta, 0) == 0;
            c.ebx = low(c.ebx, u8::from(increasing));
            let split = fraction(c, g, dist, delta, at(sp, 28))?;
            g.set_u32(at(sp, 28), split)?;
            let t0 = g.u32(at(sp, 32))?;
            let side = u32::from(!increasing);
            c.eax = side;
            push(c, g, split)?;
            push(c, g, t0)?;
            c.ecx = at(c.esi, 2 * side);
            // SAR by 8 defines CF from the eighth bit shifted out.
            let raw = g.u32(at(c.ecx, 1 + side))?;
            c.edx = (raw as i32 >> 8) as u32;
            flags(c, szp(c.edx) | ((raw >> 7) & 1), STATUS & !(0x800 | 0x10));
            for arg in [c.edx, c.ebp] {
                push(c, g, arg)?;
            }
            return call(c, g, points::COLLISION_BSP_SEGMENT.base, AFTER_SUBTREE);
        }
        c.ecx = at(c.esi, 2 * lo);
        c.eax = (g.u32(at(c.ecx, 1 + lo))? as i32 >> 8) as u32;
    }
    c.esi = u32::MAX;
    c.ebx = low(c.ebx, 3);
    g.set_u32(at(sp, 36), c.esi)?;
    g.set_u8(at(sp, 28), 3)?;
    g.set_u8(at(sp, 12), 0)?;
    if c.eax != u32::MAX {
        let bsp = g.u32(at(c.ebp, 4))?;
        let leaves = g.u32(at(bsp, 20))?;
        c.eax &= 0x7f_ffff;
        let leaf_kind = 1 + (g.u8(at(leaves, c.eax * 4))? & 1);
        g.set_u32(at(sp, 36), c.eax)?;
        c.esi = c.eax;
        c.eax = low(c.eax, leaf_kind);
        c.ebx = low(c.ebx, leaf_kind);
        g.set_u8(at(sp, 28), leaf_kind)?;
    }
    c.ecx = g.u32(c.ebp)?;
    c.edx = c.ecx & 1;
    let saved = c.edi;
    push(c, g, saved)?;
    g.set_u32(at(c.esp, 20), c.ecx)?;
    let bits = c.ecx;
    let previous = g.u8(at(c.ebp, 32))?;
    let kind = c.ebx as u8;
    let surface_leaf = if bits & 1 != 0 && matches!(previous, 1 | 2) && kind == 3 {
        c.edx = g.u32(at(c.ebp, 28))?;
        Some(c.edx)
    } else if bits & 2 != 0 && previous == 3 && matches!(kind, 1 | 2) {
        Some(c.esi)
    } else if bits & 4 == 0 && previous == 2 && kind == 2 {
        let leaf = if bits & 1 != 0 {
            c.eax = g.u32(at(c.ebp, 28))?;
            c.eax
        } else {
            c.esi
        };
        g.set_u32(at(c.esp, 28), leaf)?;
        g.set_u8(at(c.esp, 16), 1)?;
        Some(leaf)
    } else {
        None
    };
    if let Some(leaf) = surface_leaf {
        if !(bits & 4 == 0 && previous == 2 && kind == 2) {
            g.set_u32(at(c.esp, 28), leaf)?;
        }
        c.ecx = g.u32(at(c.esp, 28))?;
        if c.ecx != u32::MAX {
            let filter = g.u32(at(c.esp, 16))?;
            let t = g.u32(at(c.esp, 36))?;
            c.ebx = g.u32(at(c.ebp, 4))?;
            c.esi = g.u32(at(c.ebp, 20))?;
            c.edi = g.u32(at(c.ebp, 16))?;
            push(c, g, filter)?;
            let plane = g.u32(at(c.ebp, 36))?;
            push(c, g, t)?;
            c.eax = g.u32(at(c.ebp, 12))?;
            push(c, g, plane)?;
            push(c, g, c.eax)?;
            logic_flags(c, 0);
            c.edx = u32::from(g.u16(at(c.ebp, 8))?);
            push(c, g, c.edx)?;
            return call(c, g, TEST_SURFACE, AFTER_SURFACE);
        }
    }
    // The flags gate reloads AL from the preceding leaf on this path.
    if bits & 1 != 0 && surface_leaf.is_none() {
        c.eax = low(c.eax, previous);
    }
    if c.esi != u32::MAX {
        c.eax = g.u32(at(c.ebp, 24))?;
        c.ecx = g.u32(at(c.eax, 24))?;
        if (c.ecx as i32) < 256 {
            g.set_u32(at(c.eax, at(28, c.ecx << 2)), c.esi)?;
            c.eax = g.u32(at(c.ebp, 24))?;
            g.set_u32(at(c.eax, 24), g.u32(at(c.eax, 24))?.wrapping_add(1))?;
        } else {
            g.set_u32(at(c.eax, 0x418), c.esi)?;
        }
    }
    c.edi = pop(c, g)?;
    g.set_u32(at(c.ebp, 28), c.esi)?;
    c.esi = pop(c, g)?;
    g.set_u8(at(c.ebp, 32), kind)?;
    c.ebp = pop(c, g)?;
    c.eax &= !255;
    c.ebx = pop(c, g)?;
    add_flags(c, c.esp, 8);
    c.esp = at(c.esp, 8);
    ret(g, c, 16)
}
