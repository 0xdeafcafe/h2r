//! Object-model segment queries (169510 and 1eff40). The object hierarchy,
//! damage permutations, and node matrices are live reads. Each selected
//! part runs the caller's BSP walk in node space.
mod parts;
mod transform;

use crate::{Bail, GuestRead, Hit, Outcome, sphere::Sphere};
pub use parts::PartRay;
use parts::{Model, PartHit};
use sim_core::det::Fp;
use std::cmp::Ordering;

const OBJECT_TABLE: u32 = 0x004e_0300;
const TAG_TABLE: u32 = 0x004e_3b44;
const NONE: u32 = u32::MAX;

pub struct ObjectQuery {
    pub object: u32,
    /// The caller checked the reference's flags, ignore handles and sphere.
    pub pretested: bool,
    pub flags: u32,
    pub bsp_flags: u32,
    pub start: [u32; 3],
    pub delta: [u32; 3],
    pub ignore: [u32; 2],
    /// A model replaces the existing result only with a strictly nearer hit.
    pub fraction: u32,
}

pub struct ObjectHit {
    pub object: u32,
    pub key: u32,
    pub bsp: u32,
    pub hit: Hit,
    pub material: u16,
    pub plane: [u32; 4],
}

impl ObjectHit {
    /// Scatter only the fields written on an object hit, leaving locations,
    /// position and padding to the whole-query caller.
    pub fn write_result<E>(
        &self,
        mut put: impl FnMut(u32, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        put(0, &4u32.to_le_bytes())?;
        put(4, &self.hit.fraction.to_le_bytes())?;
        put(0x24, &self.material.to_le_bytes())?;
        for (i, component) in self.plane.iter().enumerate() {
            put(0x28 + 4 * i as u32, &component.to_le_bytes())?;
        }
        put(0x38, &NONE.to_le_bytes())?;
        put(0x3c, &NONE.to_le_bytes())?;
        put(0x40, &self.object.to_le_bytes())?;
        put(0x44, &((self.key >> 24) as u16).to_le_bytes())?;
        put(0x46, &((self.key & 255) as u16).to_le_bytes())?;
        put(0x48, &self.key.to_le_bytes())?;
        put(0x4c, &self.hit.leaf.to_le_bytes())?;
        put(0x50, &self.hit.surface.to_le_bytes())?;
        put(0x54, &self.hit.plane.to_le_bytes())?;
        put(0x58, &[self.hit.flags, self.hit.breakable])?;
        put(0x5a, &self.hit.material.to_le_bytes())
    }
}

fn object(g: &impl GuestRead, id: u32) -> Result<(u32, u32), Bail> {
    let objects = g.u32(OBJECT_TABLE)?;
    let headers = g.u32(objects.wrapping_add(0x44))?;
    let header = headers.wrapping_add((id & 0xffff) * 12);
    Ok((header, g.u32(header.wrapping_add(8))?))
}

fn model(g: &impl GuestRead, body: u32) -> Result<Option<Model>, Bail> {
    let table = g.u32(TAG_TABLE)?;
    let tag = |id| g.u32(table.wrapping_add((id & 0xffff) * 16 + 8));
    let object_tag = tag(g.u32(body)?)?;
    let hlmt_id = g.u32(object_tag.wrapping_add(0x38))?;
    if hlmt_id == NONE {
        return Ok(None);
    }
    let hlmt = tag(hlmt_id)?;
    let coll_id = g.u32(hlmt.wrapping_add(12))?;
    if coll_id == NONE {
        return Ok(None);
    }
    Ok(Some(Model {
        hlmt,
        coll: tag(coll_id)?,
        states: body.wrapping_add(g.u16(body.wrapping_add(0x11a))? as i16 as u32),
        matrices: body.wrapping_add(g.u16(body.wrapping_add(0x116))? as i16 as u32),
    }))
}

fn accepts(
    fp: &mut Fp,
    g: &impl GuestRead,
    q: &ObjectQuery,
    id: u32,
    header: u32,
    body: u32,
) -> Result<bool, Bail> {
    if q.ignore.contains(&id) || g.u8(header.wrapping_add(2))? & 16 != 0 {
        return Ok(false);
    }
    let state = g.u32(body.wrapping_add(4))?;
    let kind = u32::from(g.u8(header.wrapping_add(3))?);
    let f = q.flags;
    if state & 1 != 0
        || f & 1u32.wrapping_shl(kind + 4) == 0
        || f & (1 << 19) != 0 && state & (1 << 20) != 0
        || f & (1 << 18) != 0 && state & (1 << 22) == 0
        || f & (1 << 30) != 0 && state & (1 << 31) == 0
    {
        return Ok(false);
    }
    if kind == 0
        && (f & (1 << 20) != 0 && g.u8(body.wrapping_add(0x10a))? & 4 != 0
            || f & (1 << 22) != 0 && g.u8(body.wrapping_add(0x348))? & 8 != 0)
        || kind == 11 && f & (1 << 21) != 0 && g.u32(body.wrapping_add(0x12c))? & 2 != 0
    {
        return Ok(false);
    }
    let sphere = Sphere {
        center: g.vec3(body.wrapping_add(0x40))?,
        radius: g.u32(body.wrapping_add(0x4c))?,
    };
    let offset = sphere.offset_with(fp, q.start)?;
    Ok(offset.contains_start() || offset.sweep_with(fp, q.delta)?.hit())
}

fn result(fp: &mut Fp, g: &impl GuestRead, object: u32, hlmt: u32, part: PartHit) -> Result<ObjectHit, Bail> {
    let materials = g.u32(hlmt.wrapping_add(0x5c))?;
    let index = part.hit.material as i16 as u32;
    let material = g.u16(
        materials
            .wrapping_add(index.wrapping_mul(20))
            .wrapping_add(16),
    )?;
    let p = part.hit.plane_addr;
    let [x, y, z] = g.vec3(p)?;
    let plane = part.matrix.plane_with(
        fp,
        [x, y, z, g.u32(p.wrapping_add(12))?],
        part.hit.plane & 0x8000 != 0,
    );
    if !fp.exact() {
        return Err(Bail::Float);
    }
    Ok(ObjectHit {
        object,
        key: part.key,
        bsp: part.bsp,
        hit: part.hit,
        material,
        plane,
    })
}

/// Visit each object's child list before its next sibling. The explicit
/// continuation stack carries the caller's pretest mode across children.
/// Bound malformed cyclic lists before returning anything to the caller.
pub fn trace_objects<G: GuestRead>(
    g: &mut G,
    q: &ObjectQuery,
    visit: impl FnMut(&mut G, PartRay) -> Result<Outcome, Bail>,
) -> Result<Option<ObjectHit>, Bail> {
    trace_objects_with(&mut Fp::new(), g, q, visit)
}

/// [`trace_objects`] in the caller's `fp`: under [`Fp::guest`] every
/// operand runs as the guest's SSE instructions do and `fp` gathers the
/// flags they raise. The part walks' own flags are the caller's.
pub fn trace_objects_with<G: GuestRead>(
    fp: &mut Fp,
    g: &mut G,
    q: &ObjectQuery,
    mut visit: impl FnMut(&mut G, PartRay) -> Result<Outcome, Bail>,
) -> Result<Option<ObjectHit>, Bail> {
    if q.object == NONE {
        return Err(Bail::Data);
    }
    let mut best = None;
    let mut fraction = q.fraction;
    let mut pending = Vec::new();
    let mut current = q.object;
    let mut pretested = q.pretested;
    let mut visited = 0;
    while current != NONE || !pending.is_empty() {
        if current == NONE {
            let (parent_body, parent_pretested): (u32, bool) = pending.pop().ok_or(Bail::Data)?;
            current = g.u32(parent_body.wrapping_add(12))?;
            pretested = parent_pretested;
            continue;
        }
        visited += 1;
        if visited > 65536 {
            return Err(Bail::Data);
        }
        let (header, body) = object(g, current)?;
        if pretested || accepts(fp, g, q, current, header, body)? {
            if let Some(m) = model(g, body)?
                && let Some(part) = m.ray(fp, g, q.start, q.delta, q.bsp_flags, &mut visit)?
            {
                // Ordered: a NaN fraction never takes the part.
                let closer = fp.comiss(fraction, part.hit.fraction) == Some(Ordering::Greater);
                if !fp.exact() {
                    return Err(Bail::Float);
                }
                if closer {
                    fraction = part.hit.fraction;
                    best = Some(result(fp, g, current, m.hlmt, part)?);
                    if q.flags & (1 << 28) != 0 {
                        break;
                    }
                }
            }
            if q.flags & (1 << 17) == 0 {
                let child = g.u32(body.wrapping_add(16))?;
                if child != NONE {
                    if pending.len() >= crate::bsp::MAX_DEPTH as usize {
                        return Err(Bail::Depth);
                    }
                    pending.push((body, pretested));
                    pretested = false;
                    current = child;
                    continue;
                }
            }
        }
        current = g.u32(body.wrapping_add(12))?;
    }
    Ok(best)
}
