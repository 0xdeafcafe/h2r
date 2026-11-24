//! J checks the hit position against the recorded structure and optional
//! model leaves. It tries a normal offset first and then backs up along
//! the segment until both locations are valid or its fraction reaches zero.
use super::*;

const NUDGE: u32 = 0x0045_de54;
const PARALLEL_STEP: u32 = 0x0045_de50;

pub(super) fn nudge(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    ray: Ray,
    structure: u32,
    index: i16,
    hit_bsp: Option<u32>,
    record: &mut Record,
) -> Result<(), Bail> {
    let m = Read(&*g, (0, 0));
    let model = match record.word(0) {
        1 | 2 => None,
        3 => {
            let key = record.word(0x3c);
            let placement = m
                .u32(structure.wrapping_add(0x144))?
                .wrapping_add(key.wrapping_mul(0x58));
            let def = i32::from(m.u16(placement.wrapping_add(0x34))? as i16) as u32;
            Some(
                m.u32(structure.wrapping_add(0x13c))?
                    .wrapping_add(def.wrapping_mul(0xc8))
                    .wrapping_add(0x70),
            )
        }
        4 => Some(hit_bsp.ok_or(Bail::Data)?),
        _ => return Err(Bail::Data),
    };
    let mut pos = std::array::from_fn(|i| record.word(8 + 4 * i));
    let last = record.word(0x1c);
    let primary_addr = m.u32(layout::STRUCTURE_BSP)?;
    let primary = bsp(g, world, primary_addr)?;
    let model = model.map(|addr| bsp(g, world, addr)).transpose()?;
    let model_leaf = record.word(0x4c);
    if (last == NONE || primary.point_to_leaf(0, pos)? == last)
        && (model_leaf == NONE
            || model
                .as_ref()
                .map(|b| b.point_to_leaf(0, pos))
                .transpose()?
                .is_none_or(|leaf| leaf == model_leaf))
    {
        return Ok(());
    }
    let m = Read(&*g, (0, 0));
    let displacement = m.u32(NUDGE)?;
    let plane = record.plane();
    let normal = [plane[0], plane[1], plane[2]];
    let mut fp = Fp::new();
    for axis in 0..3 {
        let offset = fp.mul(normal[axis], displacement);
        pos[axis] = fp.add(offset, pos[axis]);
    }
    if !fp.exact() {
        return Err(Bail::Float);
    }
    let locate = |pos| -> Result<Location, Bail> {
        let leaf = if index == -1 {
            NONE
        } else {
            primary.point_to_leaf(0, pos)?
        };
        location(&m, structure, index, leaf)
    };
    let valid = |loc: Location, pos| -> Result<bool, Bail> {
        Ok(loc.leaf != NONE
            && model
                .as_ref()
                .map(|b| b.point_to_leaf(0, pos))
                .transpose()?
                .is_none_or(|leaf| leaf != NONE))
    };
    record.set_position(pos);
    let loc = locate(pos)?;
    record.put_location(0x1c, loc);
    if valid(loc, pos)? {
        return Ok(());
    }
    let d = clusters::dot(&mut fp, normal, ray.delta);
    fp.check(d);
    if !fp.exact() {
        return Err(Bail::Float);
    }
    let step = if d & 0x7fff_ffff == 0 {
        m.u32(PARALLEL_STEP)?
    } else {
        // The x87 quotient of two ordinary binary32 values rounds to this
        // binary32 result under all three admitted precision controls.
        fp.div(displacement, d & 0x7fff_ffff)
    };
    // Limit work on malformed inputs or a displacement too small to move t.
    for _ in 0..4096 {
        let next = fp.sub(record.word(4), step);
        fp.check(next);
        if !fp.exact() {
            return Err(Bail::Float);
        }
        let t = if det::compare(next, 0).is_gt() {
            next
        } else {
            0
        };
        if t == record.word(4) && t != 0 {
            return Err(Bail::Depth);
        }
        let pos = ray.position(t)?;
        let loc = locate(pos)?;
        record.put(4, t);
        record.set_position(pos);
        record.put_location(0x1c, loc);
        if t == 0 || valid(loc, pos)? {
            return Ok(());
        }
    }
    Err(Bail::Depth)
}
