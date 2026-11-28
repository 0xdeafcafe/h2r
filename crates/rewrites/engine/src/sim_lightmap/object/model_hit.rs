//! The lighting ray's hit on an object whose render model has lighting
//! blocks, `0x0014a8e0`, with the helpers under it: the model's parts
//! `0x001efb40`, the collision BSP of a hit `0x001efd80` and the vertex
//! fetch `0x0023ac30`. The geometry cache's residency call `0x0012de70`
//! (flags 3) is `View::section`, as in `0x0014ac60`.
use super::ray::{inside, untransform};
use super::{Collide, NONE, Run};
use crate::sim_native::Memory;
use runtime::GuestFault;

/// `[0x0045dc0c]`, `[0x0045dbc0]`, `[0x0045ddd0]`, `[0x0045dbbc]`: a
/// compressed coordinate's scale and offsets into its bounds.
const UNPACK: [u32; 4] = [0x0045_dc0c, 0x0045_dbc0, 0x0045_ddd0, 0x0045_dbbc];
/// `[0x00468788]`: the position of a vertex block of no known kind.
const DEFAULT_POSITION: u32 = 0x0046_8788;

/// `0x0014a8e0(record, render, hit)` called at `entry` (its return
/// address there): the J record's hit on the object `+0x40`, whose render
/// model `render` has lighting blocks. The hit's `+8`..`+0x28` but `+0x1c`
/// are -1 first. The section is the one of the record's region (`+0x44`)
/// in the object's permutation (`+0x14`, -1 as 0xff ends it); the point
/// (`+8`) is taken through the node's (`+0x46`) placement and the render
/// model's inverse; the lighting block of that section names the node's
/// triangle runs, and the run `+0x4c` is searched for the first triangle
/// on the record's surface (`+0x54`) of the collision BSP `+0x48` that
/// holds the point. Resolved: `+8` the object, `+0xc` the render model,
/// `+0x18` -1, `+0x1c` the region, `+0x20` the part holding `+0x24` (read
/// as the -1 it was set to, so 0), `+0x24`, `+0x28` the triangle's words,
/// `+0x2c` the last corner's node, `+0x30`, `+0x34` the weights. AL:
/// resolved.
pub(super) fn model_hit<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    record: u32,
    render: u32,
    hit: u32,
) -> Result<bool, GuestFault> {
    // The frame below the four saved registers: `+0x10` the collision
    // BSP, `+0x14` the section, `+0x18` the lighting entry, `+0x1c` the
    // triangle, `+0x20` the lighting block then the runtime section,
    // `+0x24` the triangle's indices, `+0x28` the compression bounds,
    // `+0x2c` the run, `+0x30` the triangle's entry, `+0x34` the corner's
    // node, `+0x38` the point, `+0x44` the parts, then the corners at
    // `+0x44`, `+0x50`, `+0x5c`. The result is the hit argument's low
    // byte.
    let f = entry.wrapping_sub(0x68);
    let at = |o: u32| f.wrapping_add(o);
    let result_at = entry.wrapping_add(0xc);
    for o in [0x18, 8, 0x14, 0x20, 0x24, 0x28, 0xc, 0x10] {
        r.set_u32(hit.wrapping_add(o), NONE)?;
    }
    let mode = r.tag(render)?;
    let body = r.body(r.u32(record.wrapping_add(0x40))?)?;
    let permutations = r.i16(body.wrapping_add(0x11a))?.wrapping_add(body);
    let region = r.i16(record.wrapping_add(0x44))?;
    let permutation = r.u8(region.wrapping_add(permutations))?;
    r.set_u8(result_at, 0)?;
    let index = if permutation == 0xff {
        0xff
    } else {
        let permutation = permutation as i8 as i32 as u32;
        let regions = r.u32(mode.wrapping_add(0x20))?;
        let list = r.u32((region << 4).wrapping_add(regions).wrapping_add(0xc))?;
        r.i16(list.wrapping_add(permutation << 4).wrapping_add(0xc))?
    };
    r.set_u32(hit.wrapping_add(0x14), index)?;
    if index == 0xff {
        return Ok(false);
    }
    let section = index.wrapping_mul(0x5c).wrapping_add(r.u32(mode.wrapping_add(0x28))?);
    let compression = r.u32(mode.wrapping_add(0x18))?;
    r.set_u32(at(0x28), compression)?;
    r.set_u32(at(0x14), section)?;
    // `0x0012de70(section + 0x38, 3)`, then `[section + 0x34]` read as the
    // runtime section.
    let Some(resident) = r.view().section(section.wrapping_add(0x38))? else {
        return Ok(false);
    };
    let object = r.u32(record.wrapping_add(0x40))?;
    let body = r.body(object)?;
    let point = r.vec3(record.wrapping_add(8))?;
    r.set_vec3(at(0x38), point)?;
    // `[body+0x114]` bytes of 0x34-byte node placements: the count by the
    // guest's reciprocal multiply.
    let bytes = r.i16(body.wrapping_add(0x114))?;
    let nodes = ((u64::from(bytes) * 0x4ec4_ec4f) >> 32) as u32 >> 4;
    if nodes as i32 > 0 {
        let node = r.i16(record.wrapping_add(0x46))?;
        let placed = r.i16(body.wrapping_add(0x116))?.wrapping_add(body);
        let m = node.wrapping_mul(0x34).wrapping_add(placed);
        let at_record = r.vec3(record.wrapping_add(8))?;
        let p = untransform(r, m, at_record)?;
        r.set_vec3(at(0x38), p)?;
        let node = r.i16(record.wrapping_add(0x46))?;
        let inverse = node.wrapping_mul(0x60).wrapping_add(r.u32(mode.wrapping_add(0x4c))?).wrapping_add(0x28);
        let q = untransform(r, inverse, p)?;
        r.set_vec3(at(0x38), q)?;
    }
    if !parts(r, object, at(0x44))? {
        return Ok(false);
    }
    let key = r.u32(record.wrapping_add(0x48))?;
    let bsp = collision_bsp(r, at(0x44), key)?;
    let index = r.u32(hit.wrapping_add(0x14))?;
    let lighting = r.u32(mode.wrapping_add(0x80))?.wrapping_add(index.wrapping_mul(8));
    r.set_u32(at(0x10), bsp)?;
    r.set_u32(at(0x20), lighting)?;
    let count = r.u32(resident.wrapping_add(0x64))?;
    if count as i32 <= 0 {
        return Ok(false);
    }
    // The section's node bytes: the lighting entry is the one at the
    // record's node.
    let node_bytes = r.u32(resident.wrapping_add(0x68))?;
    let node = r.i16(record.wrapping_add(0x46))?;
    let mut n = 0u32;
    while u32::from(r.u8(n.wrapping_add(node_bytes))?) != node {
        n += 1;
        if n as i32 >= count as i32 {
            return Ok(false);
        }
    }
    let lit = r.u32(lighting.wrapping_add(4))?.wrapping_add(n << 4);
    r.set_u32(at(0x18), lit)?;
    if r.u32(lit)? != r.u32(bsp.wrapping_add(0x10))? {
        return Ok(false);
    }
    let run = r.u32(lit.wrapping_add(4))?.wrapping_add(r.u32(record.wrapping_add(0x4c))?.wrapping_mul(8));
    let mut triangle = r.u32(run.wrapping_add(4))?;
    r.set_u32(at(0x2c), run)?;
    let end = r.i16(run.wrapping_add(2))?.wrapping_add(triangle);
    r.set_u32(at(0x1c), triangle)?;
    if triangle as i32 >= end as i32 {
        return Ok(false);
    }
    loop {
        'next: {
            if (triangle as i32) < 0 || triangle as i32 >= r.u32(lit.wrapping_add(8))? as i32 {
                break 'next;
            }
            let entry8 = r.u32(lit.wrapping_add(0xc))?.wrapping_add(triangle.wrapping_mul(8));
            let reference = r.u32(entry8.wrapping_add(4))?;
            r.set_u32(at(0x30), entry8)?;
            if reference == NONE {
                return Ok(false);
            }
            if reference as i32 >= r.u32(bsp)? as i32 {
                break 'next;
            }
            let surface = r.u32(record.wrapping_add(0x54))? & 0x7fff;
            let surfaces = r.u32(bsp.wrapping_add(4))?;
            if u32::from(r.u16(surfaces.wrapping_add(reference.wrapping_mul(8)))?) != surface {
                break 'next;
            }
            let first = u32::from(r.u16(entry8)?);
            let indices = r.u32(resident.wrapping_add(0x24))?.wrapping_add(first * 2);
            r.set_u32(at(0x20), resident)?;
            r.set_u32(at(0x24), indices)?;
            for corner in 0..3u32 {
                let index = u32::from(r.u16(indices.wrapping_add(2 * corner))?);
                let compression = r.u32(at(0x28))?;
                vertex(r, resident, compression, at(0x44 + 12 * corner), at(0x34), index)?;
            }
            let q = r.vec3(at(0x38))?;
            let v = [r.vec3(at(0x44))?, r.vec3(at(0x50))?, r.vec3(at(0x5c))?];
            let Some((towards_v1, towards_v2)) = inside(r, q, v)? else {
                break 'next;
            };
            r.set_u32(hit.wrapping_add(0x30), towards_v1)?;
            r.set_u32(hit.wrapping_add(0x34), towards_v2)?;
            return resolved(r, f, entry, record, render, hit);
        }
        let run = r.u32(at(0x2c))?;
        let end = r.i16(run.wrapping_add(2))?.wrapping_add(r.u32(run.wrapping_add(4))?);
        triangle = triangle.wrapping_add(1);
        r.set_u32(at(0x1c), triangle)?;
        if triangle as i32 >= end as i32 {
            return Ok(false);
        }
    }
}

/// `0x0014abcb`..`0x0014ac45`: the resolved hit's words.
fn resolved<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    f: u32,
    entry: u32,
    record: u32,
    render: u32,
    hit: u32,
) -> Result<bool, GuestFault> {
    let at = |o: u32| f.wrapping_add(o);
    r.set_u32(hit.wrapping_add(0x18), NONE)?;
    let object = r.u32(record.wrapping_add(0x40))?;
    let section = r.u32(at(0x20))?;
    r.set_u32(hit.wrapping_add(8), object)?;
    r.set_u32(hit.wrapping_add(0x20), 0)?;
    let count = r.u32(section)?;
    r.set_u8(entry.wrapping_add(0xc), 1)?;
    if count as i32 > 0 {
        // The part search inline, on `+0x24` as it stands.
        let mut p = r.u32(section.wrapping_add(4))?.wrapping_add(6);
        let offset = r.u32(hit.wrapping_add(0x24))? as i32;
        let mut n = 0u32;
        loop {
            let from = i32::from(r.u16(p)?);
            if offset >= from && offset < from + i32::from(r.u16(p.wrapping_add(2))?) {
                r.set_u32(hit.wrapping_add(0x20), n)?;
                break;
            }
            n += 1;
            p = p.wrapping_add(0x48);
            if n as i32 >= count as i32 {
                break;
            }
        }
    }
    let entry8 = r.u32(at(0x30))?;
    let first = u32::from(r.u16(entry8)?);
    r.set_u32(hit.wrapping_add(0x24), first)?;
    r.set_u32(hit.wrapping_add(0xc), render)?;
    let word = u32::from(r.u16(entry8.wrapping_add(2))?);
    r.set_u32(hit.wrapping_add(0x28), word)?;
    let region = r.i16(record.wrapping_add(0x44))?;
    r.set_u32(hit.wrapping_add(0x1c), region)?;
    let node = r.u32(at(0x34))?;
    r.set_u32(hit.wrapping_add(0x2c), node)?;
    Ok(true)
}

/// `0x001efb40(esi = out; object)`: the object's model (definition
/// `+0x38`) with a collision model (`+0xc`): `out` the object, the model,
/// the collision model, the body's permutation table (`+0x11a`) and node
/// placements (`+0x116`), at `+0`, `+4`, `+8`, `+0xc`, `+0x10`. With
/// either -1 nothing is stored. AL: stored.
pub(super) fn parts<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, object: u32, out: u32) -> Result<bool, GuestFault> {
    let body = r.body(object)?;
    let definition = r.tag(r.u32(body)?)?;
    let model = r.u32(definition.wrapping_add(0x38))?;
    if model == NONE {
        return Ok(false);
    }
    let model = r.tag(model)?;
    let collision = r.u32(model.wrapping_add(0xc))?;
    if collision == NONE {
        return Ok(false);
    }
    let permutations = r.i16(body.wrapping_add(0x11a))?.wrapping_add(body);
    let collision = r.tag(collision)?;
    let nodes = r.i16(body.wrapping_add(0x116))?.wrapping_add(body);
    r.set_u32(out.wrapping_add(0xc), permutations)?;
    r.set_u32(out, object)?;
    r.set_u32(out.wrapping_add(4), model)?;
    r.set_u32(out.wrapping_add(8), collision)?;
    r.set_u32(out.wrapping_add(0x10), nodes)?;
    Ok(true)
}

/// `0x001efd80(parts, key)`: the collision model's (`[parts+8]`) BSP the
/// key names (region `key >> 24`, 12 bytes at `+0x20`; permutation byte
/// 2, 20 bytes at `+8`; BSP byte 1, 0x44 bytes at `+8`), past its first
/// word.
pub(super) fn collision_bsp<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, parts: u32, key: u32) -> Result<u32, GuestFault> {
    let collision = r.u32(parts.wrapping_add(8))?;
    let regions = r.u32(collision.wrapping_add(0x20))?;
    let permutations = r.u32(regions.wrapping_add((key >> 24) * 12).wrapping_add(8))?;
    let bsps = r.u32(permutations.wrapping_add((key >> 16 & 0xff) * 20).wrapping_add(8))?;
    Ok(bsps.wrapping_add((key >> 8 & 0xff) * 0x44).wrapping_add(4))
}

/// `0x0023ac30(edx = section, eax = compression, ecx = out, edi = node;
/// index)`: the position of vertex `index` from the runtime section's
/// vertex block (`[+0x3c]`: its kind byte `+0`, the bytes at `[+0xc] +
/// [+8]`) into `out`, and its node into `node` (unless 0):
///
/// | Kind | Vertex | Position | Node |
/// |---|---|---|---|
/// | 1 | 12 bytes | three floats | 0 |
/// | 2 | 6 bytes | three i16s in the bounds | 0 |
/// | 3 | 16 bytes | three floats | byte `+0xc` |
/// | 4 | 8 bytes | three i16s in the bounds | byte `+6` |
/// | other | | `[0x00468788]` | 0 |
pub(super) fn vertex<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    section: u32,
    compression: u32,
    out: u32,
    node: u32,
    index: u32,
) -> Result<(), GuestFault> {
    let block = r.u32(section.wrapping_add(0x3c))?;
    let kind = r.u8(block)?;
    // Every kind but the default reads the bytes' base.
    let base = if matches!(kind, 1..=4) {
        let offset = r.u32(block.wrapping_add(0xc))?;
        offset.wrapping_add(r.u32(block.wrapping_add(8))?)
    } else {
        0
    };
    let (position, from) = match kind {
        1 => (r.vec3(base.wrapping_add(index.wrapping_mul(12)))?, None),
        2 => (unpack(r, compression, base.wrapping_add(index.wrapping_mul(6)))?, None),
        3 => {
            let v = base.wrapping_add(index << 4);
            (r.vec3(v)?, Some(v.wrapping_add(0xc)))
        }
        4 => {
            let v = base.wrapping_add(index.wrapping_mul(8));
            (unpack(r, compression, v)?, Some(v.wrapping_add(6)))
        }
        _ => {
            let default = r.u32(DEFAULT_POSITION)?;
            (r.vec3(default)?, None)
        }
    };
    r.set_vec3(out, position)?;
    if node != 0 {
        let n = match from {
            Some(at) => u32::from(r.u8(at)?),
            None => 0,
        };
        r.set_u32(node, n)?;
    }
    Ok(())
}

/// `0x0023ac72` and `0x0023ad80`: three i16s at `v`, each
/// `(((i * k0 + k1) * k2 + k3) * (max - min)) + min` with the bounds
/// pairs at `compression`.
fn unpack<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, compression: u32, v: u32) -> Result<[u32; 3], GuestFault> {
    let b: [u32; 6] = std::array::from_fn(|i| compression.wrapping_add(4 * i as u32));
    let range_x = r.sub(r.u32(b[1])?, r.u32(b[0])?)?;
    let k = [r.u32(UNPACK[0])?, r.u32(UNPACK[1])?, r.u32(UNPACK[2])?, r.u32(UNPACK[3])?];
    let range_y = r.sub(r.u32(b[3])?, r.u32(b[2])?)?;
    let range_z = r.sub(r.u32(b[5])?, r.u32(b[4])?)?;
    let mut out = [0; 3];
    for (i, (o, range)) in out.iter_mut().zip([range_x, range_y, range_z]).enumerate() {
        let c = r.i16(v.wrapping_add(2 * i as u32))? as i32;
        let mut x = super::fp(r.sse.int(c))?;
        x = r.mul(x, k[0])?;
        x = r.add(x, k[1])?;
        x = r.mul(x, k[2])?;
        x = r.add(x, k[3])?;
        x = r.mul(x, range)?;
        *o = r.add(x, r.u32(b[2 * i])?)?;
    }
    Ok(out)
}
