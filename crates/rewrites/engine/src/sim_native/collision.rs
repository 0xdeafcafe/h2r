use super::*;

/// Walk packed 24-bit children until reaching an encoded leaf or the solid
/// sentinel. A point exactly on the plane takes the upper child. Unordered
/// comparisons take the lower child.
pub fn point_to_leaf(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    put_xmm(c, 0, scalar(0));
    let saved = c.esi;
    push(c, g, saved)?;
    c.esi = g.u32(at(c.ecx, 4))?;
    let saved = c.edi;
    push(c, g, saved)?;
    c.edi = g.u32(at(c.ecx, 12))?;
    for depth in 0.. {
        if depth == 256 {
            const NEXT_NODE: u32 = 0x0014_a290;
            logic_flags(c, 0);
            return Ok(TransferAbi::resume(NEXT_NODE));
        }
        let node = at(c.esi, (c.eax & 0x7f_ffff) * 8);
        let plane_id = g.u16(node)? as i16 as u32;
        let plane = at(c.edi, plane_id << 4);
        let position = c.edx;
        let mut product = |axis| {
            Ok::<_, GuestFault>(op(
                c,
                Mul,
                g.u32(at(plane, axis))?,
                g.u32(at(position, axis))?,
            ))
        };
        let z = product(8)?;
        let y = product(4)?;
        let x = product(0)?;
        let zy = op(c, Add, z, y);
        let dot = op(c, Add, zy, x);
        let distance = op(c, Sub, dot, g.u32(at(plane, 12))?);
        put_xmm(c, 1, scalar(distance));
        put_xmm(c, 2, scalar(x));
        let side = u32::from(compare(c, distance, 0) & 1 == 0);
        c.ecx = at(node, side * 2);
        let child = (g.u32(at(c.ecx, 1 + side))? as i32 >> 8) as u32;
        c.eax = child;
        if child & 0x80_0000 != 0 {
            if child != u32::MAX {
                c.eax &= 0x7f_ffff;
            }
            let result = c.eax;
            logic_flags(c, result);
            break;
        }
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    ret(g, c, 0)
}

/// Segment versus sphere. The early branches define AL alone, while the
/// final discriminant comparison defines the whole EAX.
pub fn geometry_test(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let (center, origin) = (c.ecx, c.eax);
    let mut difference = |offset| {
        Ok::<_, GuestFault>(op(
            c,
            Sub,
            g.u32(at(center, offset))?,
            g.u32(at(origin, offset))?,
        ))
    };
    let y = difference(4)?;
    let z = difference(8)?;
    let x = difference(0)?;
    let zz = op(c, Mul, z, z);
    let yy = op(c, Mul, y, y);
    let zy = op(c, Add, zz, yy);
    let xx = op(c, Mul, x, x);
    let length = op(c, Add, zy, xx);
    let radius = g.u32(at(c.esp, 4))?;
    let rr = op(c, Mul, radius, radius);
    c.esp = c.esp.wrapping_sub(12);
    let gap = op(c, Sub, length, rr);
    for (i, v) in [radius, rr, gap, x, y, z, 0].into_iter().enumerate() {
        put_xmm(c, i, scalar(v));
    }
    if compare(c, 0, gap) == 0 {
        c.eax = (c.eax & !255) | 1;
    } else {
        let direction = c.edx;
        c.eax = g.u32(direction)?;
        c.ecx = g.u32(at(direction, 4))?;
        c.edx = g.u32(at(direction, 8))?;
        let (dx, dy, dz) = (c.eax, c.ecx, c.edx);
        g.set_u32(at(c.esp, 8), dz)?;
        let pz = op(c, Mul, dz, z);
        g.set_u32(at(c.esp, 4), dy)?;
        let py = op(c, Mul, dy, y);
        let zy = op(c, Add, pz, py);
        g.set_u32(c.esp, dx)?;
        let px = op(c, Mul, dx, x);
        let dot = op(c, Add, zy, px);
        for (i, v) in [(0, dot), (1, dz), (4, dx), (5, dy), (7, px)] {
            put_xmm(c, i, scalar(v));
        }
        if compare(c, dot, 0) & 1 == 0 {
            c.eax &= !255;
        } else {
            let zz = op(c, Mul, dz, dz);
            let yy = op(c, Mul, dy, dy);
            let zy = op(c, Add, zz, yy);
            let xx = op(c, Mul, dx, dx);
            let speed = op(c, Add, zy, xx);
            let dot2 = op(c, Mul, dot, dot);
            let product = op(c, Mul, speed, gap);
            let disc = op(c, Sub, dot2, product);
            for (i, v) in [(1, speed), (3, disc), (4, product)] {
                put_xmm(c, i, scalar(v));
            }
            if compare(c, 0, disc) & 1 == 0 {
                c.eax &= !255;
            } else {
                let neg = op(c, Sub, 0, speed);
                let end = op(c, Sub, neg, dot);
                put_xmm(c, 2, scalar(end));
                if compare(c, 0, end) == 0 {
                    c.eax = (c.eax & !255) | 1;
                } else {
                    let end2 = op(c, Mul, end, end);
                    put_xmm(c, 0, scalar(end2));
                    c.eax = u32::from(compare(c, disc, end2) == 0);
                }
            }
        }
    }
    let frame = c.esp;
    add_flags(c, frame, 12);
    c.esp = at(frame, 12);
    ret(g, c, 4)
}

/// Initialize a collision result and translate object-query flags to the
/// six BSP filter bits. Build the structure segment context and dispatch its BSP walk.
pub fn query(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    const COLLISION_GLOBALS: u32 = 0x004e_0348;
    const BSP_INDEX: u32 = 0x0046_86c4;
    const LOCATIONS: u32 = 0x004e_d280;
    const STRUCTURE: u32 = 0x004e_0340;
    const SEGMENT_SCALE: u32 = 0x0044_aed4;
    const AFTER_FILTER: u32 = 0x0016_983f;
    const AFTER_STRUCTURE: u32 = 0x0016_9881;
    c.esp = c.esp.wrapping_sub(0x444);
    let globals = g.u32(COLLISION_GLOBALS)?;
    let saved = c.ebx;
    push(c, g, saved)?;
    c.ebx = g.u32(at(c.esp, 0x44c))?;
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = g.u32(at(c.esp, 0x464))?;
    for saved in [c.esi, c.edi] {
        push(c, g, saved)?;
    }
    let sp = c.esp;
    g.set_u8(at(sp, 19), 0)?;
    g.set_u32(at(sp, 40), globals)?;
    if c.ebx & (3 << 23) == 0 {
        c.ebx |= 3 << 23;
        g.set_u32(at(sp, 0x458), c.ebx)?;
    }
    let one = g.u32(crate::function_eval::constant::ONE)?;
    let bsp = g.u16(BSP_INDEX)?;
    c.edi = (c.edi & 0xffff_0000) | u32::from(bsp);
    c.esi = u32::MAX;
    g.set_u32(at(c.ebp, 4), one)?;
    g.set_u32(c.ebp, 0)?;
    for offset in [20, 28] {
        g.set_u32(at(c.ebp, offset), u32::MAX)?;
        g.set_u16(at(c.ebp, offset + 4), u16::MAX)?;
        g.set_u16(at(c.ebp, offset + 6), bsp)?;
    }
    for offset in [56, 60, 64] {
        g.set_u32(at(c.ebp, offset), u32::MAX)?;
    }
    push(c, g, AFTER_FILTER)?;
    let filter = (c.ebx >> 23) & 63;
    pop(c, g)?;
    let delta = g.u32(at(sp, 0x460))?;
    put_xmm(c, 0, scalar(g.u32(SEGMENT_SCALE)?));
    push(c, g, delta)?;
    let start = g.u32(at(c.esp, 0x460))?;
    push(c, g, start)?;
    let location = at(g.u32(LOCATIONS)?, at((bsp as i16 as u32) << 5, 1));
    push(c, g, location)?;
    c.edx = g.u32(STRUCTURE)?;
    push(c, g, 256)?;
    c.eax = at(c.esp, 0x48);
    c.ecx = filter;
    g.set_u32(at(c.esp, 0x34), filter)?;
    // Final SHL of the sign-extended BSP index; LEAs preserve its flags.
    let i = bsp as i16 as u32;
    flags(c, szp(i << 5) | ((i >> 27) & 1), STATUS & !(0x800 | 0x10));
    push(c, g, AFTER_STRUCTURE)?;
    structure_segment(c, g)
}

/// The structure query wraps the recursive BSP walk in a 40-byte context
/// on the guest stack. Keep the segment limit and the visited-leaf list
/// in that frame so the traversal and subsequent queries share them.
fn structure_segment(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(40);
    let sp = c.esp;
    let scale = xmm(c, 0)[0];
    put_xmm(c, 2, scalar(0));
    let negative = compare(c, 0, scale) == 0;
    g.set_u32(sp, c.ecx)?;
    c.ecx = (c.ecx & 0xffff_0000) | u32::from(g.u16(at(sp, 44))?);
    g.set_u32(at(sp, 4), c.edx)?;
    c.edx = g.u32(at(sp, 48))?;
    g.set_u16(at(sp, 8), c.ecx as u16)?;
    c.ecx = g.u32(at(sp, 52))?;
    g.set_u32(at(sp, 12), c.edx)?;
    c.edx = g.u32(at(sp, 56))?;
    g.set_u32(at(sp, 16), c.ecx)?;
    g.set_u32(at(sp, 20), c.edx)?;
    g.set_u32(at(sp, 24), c.eax)?;
    let limit = if negative { 0 } else { scale };
    put_xmm(c, 1, scalar(limit));
    c.ecx = 0;
    g.set_u32(c.eax, limit)?;
    g.set_u32(at(c.eax, 24), 0)?;
    c.eax = u32::MAX;
    let negative = compare(c, 0, scale) == 0;
    g.set_u32(at(sp, 28), c.eax)?;
    g.set_u8(at(sp, 32), 0)?;
    g.set_u32(at(sp, 36), c.eax)?;
    if negative {
        g.set_u32(at(sp, 44), 0)?;
    } else {
        let one = g.u32(crate::function_eval::constant::ONE)?;
        put_xmm(c, 1, scalar(one));
        let above = compare(c, scale, one) == 0;
        g.set_u32(at(sp, 44), one)?;
        if !above {
            g.set_u32(at(sp, 44), scale)?;
        }
    }
    c.eax = g.u32(at(sp, 44))?;
    for arg in [c.eax, c.ecx, c.ecx] {
        push(c, g, arg)?;
    }
    c.ecx = at(c.esp, 12);
    push(c, g, c.ecx)?;
    const AFTER_BSP: u32 = 0x001d_e6bd;
    call(c, g, guest_layout::points::COLLISION_BSP_SEGMENT.base, AFTER_BSP)
}

/// Finish a world-only miss when both the outer query and its BSP walk
/// are native. Other owners of the BSP wrapper keep their own return path.
pub fn complete_segment(c: &mut CpuAbi, g: &mut impl Memory, transfer: TransferAbi) -> Result {
    const AFTER_BSP: u32 = 0x001d_e6bd;
    const AFTER_STRUCTURE: u32 = 0x0016_9881;
    if transfer != TransferAbi::ret(AFTER_BSP) || g.u32(at(c.esp, 40))? != AFTER_STRUCTURE {
        return Ok(transfer);
    }
    let sp = at(c.esp, 60); // 40 local bytes, saved PC, 16 argument bytes
    let count = g.u32(at(sp, 0x50))? as i32;
    // If the output overlaps the caller frame, writes to either can
    // change the remaining query's control fields.
    let overlaps_frame = c.ebp.wrapping_sub(sp) < 0x470 || sp.wrapping_sub(c.ebp) < 0x60;
    if overlaps_frame
        || (c.eax as u8 != 0 && c.ebx & 1 != 0)
        || (count > 0 && c.ebx & 14 != 0)
        || g.u8(at(sp, 19))? != 0
    {
        return Ok(transfer);
    }
    add_flags(c, c.esp, 40);
    c.esp = at(c.esp, 40);
    ret(g, c, 16)?;
    put_xmm(c, 3, scalar(0));
    c.edx = count as u32;
    if count > 0 {
        let first = g.u32(at(sp, 0x54))?;
        g.set_u32(at(c.ebp, 20), first)?;
        g.set_u16(at(c.ebp, 24), cluster(g, first)?)?;
        let last = g.u32(at(sp, at(0x50, c.edx << 2)))?;
        g.set_u16(at(c.ebp, 26), c.edi as u16)?;
        g.set_u32(at(c.ebp, 28), last)?;
        g.set_u16(at(c.ebp, 32), cluster(g, last)?)?;
        g.set_u16(at(c.ebp, 34), c.edi as u16)?;
    }
    let fraction = g.u32(at(c.ebp, 4))?;
    c.eax = g.u32(at(sp, 0x460))?;
    c.ecx = g.u32(at(sp, 0x45c))?;
    c.ebx = at(c.ebp, 8);
    for offset in [0, 4, 8] {
        let d = g.u32(at(c.eax, offset))?;
        let scaled = if offset == 0 {
            op(c, Mul, d, fraction)
        } else {
            op(c, Mul, fraction, d)
        };
        let position = op(c, Add, scaled, g.u32(at(c.ecx, offset))?);
        put_xmm(c, if offset == 8 { 0 } else { 1 }, scalar(position));
        g.set_u32(at(c.ebx, offset), position)?;
    }
    c.eax = low(c.eax, g.u8(at(sp, 19))?);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    add_flags(c, c.esp, 0x444);
    c.esp = at(c.esp, 0x444);
    ret(g, c, 24)
}

fn cluster(g: &impl Memory, leaf: u32) -> std::result::Result<u16, GuestFault> {
    if leaf == u32::MAX {
        return Ok(u16::MAX);
    }
    const COLLISION_GLOBALS: u32 = 0x004e_0348;
    let globals = g.u32(COLLISION_GLOBALS)?;
    let clusters = g.u32(at(globals, 0x30))?;
    g.u16(at(clusters, leaf << 3))
}
