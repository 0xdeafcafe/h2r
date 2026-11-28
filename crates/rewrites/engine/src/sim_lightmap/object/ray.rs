//! The lighting ray `0x0014af40` and its structure hit `0x0014ac60`, with
//! the helpers under it: the inverse transform `0x00142700`, the part
//! search `0x0016e1b0`, the triangle test `0x0011e800` and its axis pick
//! `0x00120850`.
use super::{Collide, NONE, Run, push_args, ret};
use crate::sim_native::Memory;
use runtime::GuestFault;
use x87::abi::unary_op;

/// `[0x0045df0c]`: the restart's step along the ray.
const STEP: u32 = 0x0045_df0c;
const ONE: u32 = 0x0045_dbc0;
/// `[0x0045dbdc]`, `[0x0045dbd8]`, `[0x0045ddfc]`: the triangle test's
/// epsilons.
const EPSILON: u32 = 0x0045_dbdc;
const AXIS_FLOOR: u32 = 0x0045_dbd8;
const EDGE_FLOOR: u32 = 0x0045_ddfc;
/// Six-byte rows of two axis words, by the largest axis and its sign.
const AXES: u32 = 0x0044_0b94;
/// Entry J's flags: the ray's (by the caller's byte), and the structure's.
const FLAGS_BYTE: u32 = 0x4480_000d;
const FLAGS: u32 = 0x0480_0c0d;
const FLAGS_STRUCTURE: u32 = 0x0480_0005;

/// `0x0014af40` called at `entry` (its return address there), AL `byte`,
/// ECX `point`, EDI `start`; arguments the delta, the object J ignores,
/// a word it does not read and the 0x38-byte hit. The start is copied from
/// the point; with a lit structure the hit's first three words are -1 and
/// entry J is asked. An object hit asks J again for the structure alone
/// (an object of kind 6 whose definition `+0xc0` is not 2 does not); a
/// structure hit is resolved by `0x0014ac60`. A miss to resolve restarts
/// a step on (`[0x0045df0c]` times the delta) while the J record's `+0x58`
/// bit 0 is set, three times at most. AL: resolved.
pub fn ray<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    byte: u8,
    point: u32,
    start: u32,
    [delta, object, _, hit]: [u32; 4],
) -> Result<bool, GuestFault> {
    let mut flags = if byte != 0 { FLAGS_BYTE } else { FLAGS };
    let p = r.vec3(point)?;
    r.set_vec3(start, p)?;
    let flags_at = entry.wrapping_sub(0x64);
    let restarts_at = entry.wrapping_sub(0x60);
    let record = entry.wrapping_sub(0x5c);
    r.set_u32(flags_at, flags)?;
    r.set_u16(record.wrapping_add(0x24), 0xffff)?;
    // `0x0014af7a`: the result byte is the hit argument's low byte.
    let result_at = entry.wrapping_add(0x10);
    r.set_u8(result_at, 0)?;
    if !r.lit()? {
        return Ok(false);
    }
    r.set_vec3(hit, [NONE; 3])?;
    let mut restarts = 0u32;
    r.set_u32(restarts_at, restarts)?;
    let query = entry.wrapping_sub(0x8c);
    loop {
        push_args(r, query, ret::QUERY, &[flags, start, delta, object, NONE, record])?;
        if !r.j.query(&mut *r.g, query)? {
            return Ok(false);
        }
        let other = r.u32(record.wrapping_add(0x40))?;
        let mut structure = other == NONE;
        if !structure {
            let body = r.body(other)?;
            let definition = r.tag(r.u32(body)?)?;
            let passes = r.u8(body.wrapping_add(0xaa))? == 6 && r.u16(definition.wrapping_add(0xc0))? != 2;
            if !passes {
                if r.model_lighting(other, hit.wrapping_add(0xc), hit.wrapping_add(0x10))? {
                    let render = r.u32(hit.wrapping_add(0xc))?;
                    unimplemented!(
                        "lightmap: a hit on object {other:#010x}, whose render model {render:#010x} has lighting blocks (0x0014a8e0)"
                    );
                }
                let args = [FLAGS_STRUCTURE, start, delta, NONE, NONE, record];
                push_args(r, query, ret::STRUCTURE_QUERY, &args)?;
                flags = FLAGS_STRUCTURE;
                r.set_u32(flags_at, flags)?;
                structure = r.j.query(&mut *r.g, query)?;
            }
        }
        if structure {
            let at = entry.wrapping_sub(0x7c);
            push_args(r, at, ret::STRUCTURE_HIT, &[record, hit])?;
            let resolved = structure_hit(r, at, record, hit)?;
            r.set_u8(result_at, resolved as u8)?;
            if resolved {
                return Ok(true);
            }
        }
        if r.u8(record.wrapping_add(0x58))? & 1 == 0 {
            return Ok(false);
        }
        restarts += 1;
        r.set_u32(restarts_at, restarts)?;
        if restarts >= 3 {
            return Ok(false);
        }
        let step = r.u32(STEP)?;
        let d = r.vec3(delta)?;
        let mut s = r.vec3(start)?;
        for i in 0..3 {
            let along = r.mul(d[i], step)?;
            s[i] = r.add(along, s[i])?;
        }
        r.set_vec3(start, s)?;
    }
}

/// `0x0014ac60(record, hit)` called at `entry`: the J record's structure
/// (`+0` 1, a cluster's surface run `[bsp+0x30] + 8 * [+0x4c]`) or
/// instance (`+0` 3, instance `+0x3c` and its definition's run at
/// `+0xbc`, the point taken into its space) hit, through the section's
/// triangles whose surface is the record's `+0x54`, the first that holds
/// the point: the hit's `+0` (the run's word), `+4` (the instance),
/// `+0x18`, `+0x20`, `+0x24` (the part and the offset in it), `+0x28` and
/// the weights at `+0x30`, `+0x34`. `+0x18` is cleared first. A section
/// that is not resident resolves nothing.
pub(super) fn structure_hit<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    record: u32,
    hit: u32,
) -> Result<bool, GuestFault> {
    // The frame below the four saved registers: `+0x12` the vertex flag
    // the guest never clears, `+0x13` the result, `+0x14` the section,
    // `+0x18` the run, `+0x1c` the instance, `+0x20` the triangle,
    // `+0x24` the surface, `+0x28` the structure, `+0x2c`..`+0x34` the
    // part search's outputs, `+0x38` the point, `+0x44`, `+0x50`, `+0x5c`
    // the corners.
    let f = entry.wrapping_sub(0x68);
    let at = |o: u32| f.wrapping_add(o);
    let point = r.vec3(record.wrapping_add(8))?;
    r.set_vec3(at(0x38), point)?;
    let bsp = r.u32(super::BSP)?;
    let surface = r.u32(record.wrapping_add(0x54))? & 0x7fff;
    let run = r.u32(record.wrapping_add(0x4c))?;
    let instance = r.u32(record.wrapping_add(0x3c))?;
    let kind = r.u32(record)?;
    r.set_u8(at(0x13), 0)?;
    r.set_u32(at(0x28), bsp)?;
    r.set_u32(at(0x14), 0)?;
    r.set_u32(at(0x1c), instance)?;
    r.set_u32(at(0x24), surface)?;
    let mut definition = 0;
    match kind {
        1 => {
            let runs = r.u32(bsp.wrapping_add(0x30))?.wrapping_add(run.wrapping_mul(8));
            let cluster = r.i16(runs)?.wrapping_mul(0xb0).wrapping_add(r.u32(bsp.wrapping_add(0xa0))?);
            r.set_u32(at(0x18), runs)?;
            let resident = r.view().section(cluster.wrapping_add(0x28))?;
            if let Some(section) = resident {
                r.set_u32(at(0x14), section)?;
            }
        }
        3 => {
            let placed = instance.wrapping_mul(0x58).wrapping_add(r.u32(bsp.wrapping_add(0x144))?);
            definition = r
                .i16(placed.wrapping_add(0x34))?
                .wrapping_mul(0xc8)
                .wrapping_add(r.u32(bsp.wrapping_add(0x13c))?);
            if r.u32(definition.wrapping_add(0xb8))? == 0 {
                return Ok(false);
            }
            let runs = r.u32(definition.wrapping_add(0xbc))?.wrapping_add(run.wrapping_mul(8));
            r.set_u32(at(0x18), runs)?;
            let local = untransform(r, placed, point)?;
            r.set_vec3(at(0x38), local)?;
            let resident = r.view().section(definition.wrapping_add(0x28))?;
            if let Some(section) = resident {
                r.set_u32(at(0x14), section)?;
            }
        }
        _ => {}
    }
    let runs = r.u32(at(0x18))?;
    let section = r.u32(at(0x14))?;
    r.set_u32(hit.wrapping_add(0x18), 0)?;
    if section == 0 {
        return Ok(false);
    }
    let first = r.u32(runs.wrapping_add(4))?;
    let mut triangle = first;
    if triangle as i32 >= r.i16(runs.wrapping_add(2))?.wrapping_add(first) as i32 {
        return Ok(false);
    }
    loop {
        r.set_u32(at(0x20), triangle)?;
        let list = if instance == NONE {
            r.u32(bsp.wrapping_add(0x50))?
        } else {
            r.u32(definition.wrapping_add(0xc4))?
        };
        let entry8 = list.wrapping_add(triangle.wrapping_mul(8));
        let reference = r.u32(entry8.wrapping_add(4))?;
        if reference != NONE {
            let surfaces = if instance == NONE {
                r.u32(r.u32(bsp.wrapping_add(0x18))?.wrapping_add(4))?
            } else {
                r.u32(definition.wrapping_add(0x74))?
            };
            let first = u32::from(r.u16(entry8)?);
            if r.i16(surfaces.wrapping_add(reference.wrapping_mul(8)))? == surface && corners(r, f, section, first)? {
                let q = r.vec3(at(0x38))?;
                let v = [r.vec3(at(0x44))?, r.vec3(at(0x50))?, r.vec3(at(0x5c))?];
                if let Some((towards_v1, towards_v2)) = inside(r, q, v)? {
                    r.set_u32(hit.wrapping_add(0x30), towards_v1)?;
                    r.set_u32(hit.wrapping_add(0x34), towards_v2)?;
                    let offset = r.u32(at(0x2c))?;
                    r.set_u32(hit.wrapping_add(0x24), offset)?;
                    let word = u32::from(r.u16(entry8.wrapping_add(2))?);
                    r.set_u32(hit.wrapping_add(0x28), word)?;
                    let part = r.u32(at(0x34))?;
                    r.set_u32(hit.wrapping_add(0x20), part)?;
                    let zero = r.u32(at(0x30))?;
                    r.set_u32(hit.wrapping_add(0x18), zero)?;
                    let head = r.i16(runs)?;
                    r.set_u32(hit, head)?;
                    let instance = r.u32(record.wrapping_add(0x3c))?;
                    r.set_u32(hit.wrapping_add(4), instance)?;
                    r.set_u8(at(0x13), 1)?;
                    return Ok(true);
                }
            }
        }
        let end = r.i16(runs.wrapping_add(2))?.wrapping_add(r.u32(runs.wrapping_add(4))?);
        triangle = triangle.wrapping_add(1);
        if triangle as i32 >= end as i32 {
            return Ok(false);
        }
    }
}

/// `0x0014adc7`..`0x0014ae9f` for the triangle whose indices start at
/// `first` (u16s at the section's `+0x24`): its part ([`part`]), then its
/// three corners from the vertex block (`[section+0x3c]`: `+2` the count,
/// 12-byte positions at `[+8] + [+0xc]`) into the frame at `+0x44`,
/// `+0x50`, `+0x5c`, each that exists setting the frame's flag `+0x12`. A
/// first corner missing with the flag clear skips the triangle; a frame
/// slot not set keeps the bytes it had.
fn corners<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, f: u32, section: u32, first: u32) -> Result<bool, GuestFault> {
    let at = |o: u32| f.wrapping_add(o);
    let indices = r.u32(section.wrapping_add(0x24))?.wrapping_add(first * 2);
    part(r, section, first, [at(0x30), at(0x34), at(0x2c)])?;
    let vertices = r.u32(section.wrapping_add(0x3c))?;
    for (corner, slot) in [0x44u32, 0x50, 0x5c].into_iter().enumerate() {
        let index = u32::from(r.u16(indices.wrapping_add(2 * corner as u32))?);
        let base = r.u32(vertices.wrapping_add(0xc))?.wrapping_add(r.u32(vertices.wrapping_add(8))?);
        if base != 0 && index < u32::from(r.u16(vertices.wrapping_add(2))?) {
            let position = r.vec3(base.wrapping_add(index * 12))?;
            r.set_vec3(at(slot), position)?;
            r.set_u8(at(0x12), 1)?;
        } else if corner == 0 && r.u8(at(0x12))? == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `0x0016e1b0(eax = zero, ecx = index, ebx = section; part, offset)`:
/// `zero` cleared; the section's parts (`[+0]` of them, 0x48 bytes at
/// `[+4]`, each `+6` its first index and `+8` its count) searched for the
/// one holding `index`: `offset` is the index in it and `part` its number,
/// -1 when none does (a negative count gives part 0).
pub(super) fn part<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    section: u32,
    index: u32,
    [zero, part, offset]: [u32; 3],
) -> Result<(), GuestFault> {
    r.set_u32(zero, 0)?;
    let count = r.u32(section)?;
    let mut n = 0u32;
    if count as i32 > 0 {
        let mut p = r.u32(section.wrapping_add(4))?;
        loop {
            let from = u32::from(r.u16(p.wrapping_add(6))?);
            if from <= index && from + u32::from(r.u16(p.wrapping_add(8))?) > index {
                r.set_u32(offset, index - from)?;
                break;
            }
            n += 1;
            p = p.wrapping_add(0x48);
            if n as i32 >= count as i32 {
                break;
            }
        }
    }
    r.set_u32(part, if n == count { NONE } else { n })
}

/// `0x00142700(ecx = m, esi = p)`: `p` taken into the space of the
/// placement `m` (`+0` its scale, `+4` three axes, `+0x28` its origin):
/// less the origin, over the scale unless it is 1, dotted with each axis.
/// A scale of 0 gives the origin of the space.
pub(super) fn untransform<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, m: u32, p: [u32; 3]) -> Result<[u32; 3], GuestFault> {
    let scale = r.u32(m)?;
    if super::fp(r.sse.equal(scale, 0))? {
        return Ok([0; 3]);
    }
    let one = r.u32(ONE)?;
    let origin = r.vec3(m.wrapping_add(0x28))?;
    let mut x = [0; 3];
    for i in 0..3 {
        x[i] = r.sub(p[i], origin[i])?;
    }
    if !super::fp(r.sse.equal(scale, one))? {
        let inverse = r.div(one, scale)?;
        for v in &mut x {
            *v = r.mul(inverse, *v)?;
        }
    }
    let mut out = [0; 3];
    for (axis, o) in out.iter_mut().enumerate() {
        let a = m.wrapping_add(4 + 12 * axis as u32);
        let k = r.vec3(a)?;
        let zz = r.mul(k[2], x[2])?;
        let yy = r.mul(k[1], x[1])?;
        let sum = r.add(zz, yy)?;
        let xx = r.mul(k[0], x[0])?;
        *o = r.add(sum, xx)?;
    }
    Ok(out)
}

/// `0x0011e800(esi = q, eax = v0, edx = v1, ecx = v2)`: whether `q` lies on
/// the triangle (near its plane, inside its edges, projected on the plane
/// of the normal's two smaller axes), and its two weights.
pub(super) fn inside<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    q: [u32; 3],
    [v0, v1, v2]: [[u32; 3]; 3],
) -> Result<Option<(u32, u32)>, GuestFault> {
    let mut a = [0; 3];
    let mut e1 = [0; 3];
    let mut e2 = [0; 3];
    for i in 0..3 {
        a[i] = r.sub(q[i], v0[i])?;
        e1[i] = r.sub(v1[i], v0[i])?;
        e2[i] = r.sub(v2[i], v0[i])?;
    }
    let zy = r.mul(e2[2], e1[1])?;
    let yx = r.mul(e1[1], e2[0])?;
    let zx = r.mul(e2[2], e1[0])?;
    let yz = r.mul(e2[1], e1[2])?;
    let xz = r.mul(e1[2], e2[0])?;
    let ny = r.sub(xz, zx)?;
    let xy = r.mul(e2[1], e1[0])?;
    let nz = r.sub(xy, yx)?;
    let dy = r.mul(ny, a[1])?;
    let dz = r.mul(nz, a[2])?;
    let d = r.add(dy, dz)?;
    let nx = r.sub(zy, yz)?;
    let dx = r.mul(nx, a[0])?;
    let d = r.add(d, dx)?;
    let dd = r.mul(d, d)?;
    let zz = r.mul(nz, nz)?;
    let yy = r.mul(ny, ny)?;
    let length = r.add(zz, yy)?;
    let xx = r.mul(nx, nx)?;
    let length = r.add(length, xx)?;
    let epsilon = r.u32(EPSILON)?;
    let length = r.mul(length, epsilon)?;
    if !r.above(length, dd)? {
        return Ok(None);
    }
    let n = [nx, ny, nz];
    let axis = largest(r, n)? as usize;
    let positive = r.above(n[axis], r.u32(AXIS_FLOOR)?)?;
    let row = AXES.wrapping_add(6 * (u32::from(positive) + 2 * axis as u32));
    let u = r.i16(row)? as usize;
    let v = r.i16(row.wrapping_add(2))? as usize;
    let floor = r.u32(EDGE_FLOOR)?;
    let p = r.mul(a[v], e1[u])?;
    let p2 = r.mul(e1[v], a[u])?;
    let s = r.sub(p, p2)?;
    if !r.above(s, floor)? {
        return Ok(None);
    }
    let p = r.mul(e2[v], a[u])?;
    let p2 = r.mul(a[v], e2[u])?;
    let t = r.sub(p, p2)?;
    if !r.above(t, floor)? {
        return Ok(None);
    }
    let p = r.mul(e2[v], e1[u])?;
    let p2 = r.mul(e1[v], e2[u])?;
    let w = r.sub(p, p2)?;
    if !r.above(floor, w)? && !r.above(w, epsilon)? {
        return Ok(None);
    }
    let edge = r.add(w, epsilon)?;
    let sum = r.add(t, s)?;
    if r.below(edge, sum)? {
        return Ok(None);
    }
    let inverse = r.div(r.u32(ONE)?, w)?;
    Ok(Some((r.mul(inverse, t)?, r.mul(inverse, s)?)))
}

/// `0x00120850(eax = v)`: the axis of the largest magnitude (`fabs` on the
/// x87, stored as f32), z before y before x on ties.
pub(super) fn largest<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, v: [u32; 3]) -> Result<u16, GuestFault> {
    let mut x = r.x87();
    let mut m = [0; 3];
    for (o, c) in m.iter_mut().zip(v) {
        super::fp(x.fld(c))?;
        super::fp(x.unary(unary_op::ABS))?;
        *o = super::fp(x.fstp_m32())?;
    }
    super::fp(x.fld(m[1]))?;
    super::fp(x.fld(m[2]))?;
    let flags = super::fp(x.fcomip(1))?;
    super::fp(x.fstp_st(0))?;
    if flags & 1 == 0 && !r.below(m[2], m[0])? {
        return Ok(2);
    }
    Ok(if r.below(m[1], m[0])? { 0 } else { 1 })
}
