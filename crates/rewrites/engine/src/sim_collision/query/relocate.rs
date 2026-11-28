//! J checks the hit position against the recorded structure and optional
//! model leaves. It tries a normal offset first and then backs up along
//! the segment until both locations are valid or its fraction reaches zero.
use super::*;

const NUDGE: u32 = 0x0045_de54;
const PARALLEL_STEP: u32 = 0x0045_de50;

#[allow(clippy::too_many_arguments)]
pub(super) fn nudge(
    env: &mut Env,
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
    let model_leaf = record.word(0x4c);
    if last == NONE && (model.is_none() || model_leaf == NONE) {
        return Ok(());
    }
    let primary_addr = m.u32(layout::STRUCTURE_BSP)?;
    let primary = bsp(g, world, primary_addr)?;
    let leaf = |env: &mut Env, b: &Bsp, pos| -> Result<u32, Bail> {
        Ok(b.point_result_with(&mut env.fp, 0, pos)?.leaf)
    };
    let primary_matches = last == NONE || leaf(&mut *env, &primary, pos)? == last;
    if primary_matches && (model.is_none() || model_leaf == NONE) {
        return Ok(());
    }
    let model = model.map(|addr| bsp(g, world, addr)).transpose()?;
    if primary_matches
        && model
            .as_ref()
            .map(|b| leaf(&mut *env, b, pos))
            .transpose()?
            == Some(model_leaf)
    {
        return Ok(());
    }
    let m = Read(&*g, (0, 0));
    let displacement = m.u32(NUDGE)?;
    let plane = record.plane();
    let normal = [plane[0], plane[1], plane[2]];
    for axis in 0..3 {
        let offset = env.fp.mul(normal[axis], displacement);
        pos[axis] = env.fp.add(offset, pos[axis]);
    }
    env.exact()?;
    let locate = |env: &mut Env, pos| -> Result<Location, Bail> {
        let leaf = if index == -1 { NONE } else { leaf(&mut *env, &primary, pos)? };
        location(&m, structure, index, leaf)
    };
    let valid = |env: &mut Env, loc: Location, pos| -> Result<bool, Bail> {
        Ok(loc.leaf != NONE
            && model
                .as_ref()
                .map(|b| leaf(&mut *env, b, pos))
                .transpose()?
                .is_none_or(|leaf| leaf != NONE))
    };
    record.set_position(pos);
    let loc = locate(&mut *env, pos)?;
    record.put_location(0x1c, loc);
    if valid(&mut *env, loc, pos)? {
        return Ok(());
    }
    let d = clusters::dot(&mut env.fp, normal, ray.delta);
    // `ucomiss d, 0; lahf; test ah, 44h; jnp`: only an ordered zero takes
    // the parallel step. Otherwise `fld d; fabs; fdivr m32; fstp m32`
    // divides on the x87 under the caller's control word.
    let step = if env.fp.ucomiss(d, 0) == Some(Ordering::Equal) {
        m.u32(PARALLEL_STEP)?
    } else {
        let x = &env.x87;
        let q = x.div(x.load32(displacement), x.abs(x.load32(d)));
        x.store32(q)
    };
    // Limit work on malformed inputs or a displacement too small to move t.
    for _ in 0..4096 {
        let next = env.fp.sub(record.word(4), step);
        // `comiss next, 0; ja`: a NaN or non-positive time is zero.
        let t = if env.fp.comiss(next, 0) == Some(Ordering::Greater) {
            next
        } else {
            0
        };
        env.exact()?;
        if t == record.word(4) && t != 0 {
            return Err(Bail::Depth);
        }
        let pos = ray.position(env, t)?;
        let loc = locate(&mut *env, pos)?;
        record.put(4, t);
        record.set_position(pos);
        record.put_location(0x1c, loc);
        if t == 0 || valid(&mut *env, loc, pos)? {
            return Ok(());
        }
    }
    Err(Bail::Depth)
}
