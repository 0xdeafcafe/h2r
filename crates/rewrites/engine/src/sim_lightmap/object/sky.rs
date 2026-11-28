//! The sky's light `0x000d1a30`, with the blend `0x00131c20` (its plain
//! arm), the normalise `0x00030bf0` and the pack `0x00131ed0`.
use super::{Collide, NONE, Run, push_args, ret};
use crate::sim_native::Memory;
use runtime::GuestFault;
use x87::abi::{arith_op, unary_op};

/// The sky the structure uses: a byte that one is, and its index into the
/// scenario's skies (`[0x004e0350] + 8` the count, `+0xc` 8-byte entries
/// whose `+4` is the tag).
const HAS_SKY: u32 = 0x004b_9ee9;
const SKY_INDEX: u32 = 0x004b_9eec;
const SCENARIO: u32 = 0x004e_0350;
/// Pointers to the start direction and colour (`0x000d1aa8`, `0x000d1abc`).
const DIRECTION: u32 = 0x0046_87a4;
const COLOUR: u32 = 0x0046_8718;
/// `[0x0045dbcc]`: each axis of the direction with no lights.
const UNLIT: u32 = 0x0045_dbcc;
/// `[0x00445420]`: the occlusion ray's length.
const REACH: u32 = 0x0044_5420;
/// `[0x0045dc0c]`: added to a light's number in its blend weight.
const BIAS: u32 = 0x0045_dc0c;
const ONE: u32 = 0x0045_dbc0;
const LEAST: u32 = 0x0045_dbdc;
const BYTE_SCALE: u32 = 0x0045_dc38;
/// What the output's `+0xc` takes.
const SKY_SECOND: u32 = 0xff55_5555;
/// The occlusion ray's flags.
const FLAGS: u32 = 0x1000_0001;

/// `0x000d1a30(object, centre, out)` called at `entry`: 2 with no sky (the
/// byte clear, the index -1 or out of range, no tag), else 0 and the
/// output: `+0` the direction, the sum of the lights' (sky `+0x78` of
/// them, 0x34 bytes at `+0x7c`, each with `+0x2c` above 0 counted: `+0xc`
/// yaw, `+0x10` pitch) normalised; `+0xc` `0xff555555`; `+0x10` the
/// colour packed, the first light's (`[+0x30] + 4`) blended towards each
/// next by its weight `[+0x30] + 0x10`, or the sky's `+0x34` when it has
/// no lights or a ray along the direction from the centre does not reach
/// the sky (no hit, or the record's `+0x5a` not -1).
pub fn sky<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    [object, centre, out]: [u32; 3],
) -> Result<u32, GuestFault> {
    let record = entry.wrapping_sub(0x5c);
    r.set_u16(record.wrapping_add(0x24), 0xffff)?;
    if r.u8(HAS_SKY)? == 0 {
        return Ok(2);
    }
    let index = r.u32(SKY_INDEX)?;
    if index == NONE {
        return Ok(2);
    }
    let scenario = r.u32(SCENARIO)?;
    let mut tag = NONE;
    let i = index as u16 as i16 as i32;
    if i >= 0 && i < r.u32(scenario.wrapping_add(8))? as i32 {
        tag = r.u32(r.u32(scenario.wrapping_add(0xc))?.wrapping_add(i as u32 * 8 + 4))?;
    }
    if tag == NONE {
        return Ok(2);
    }
    let sky = r.tag(tag)?;
    if sky == 0 {
        return Ok(2);
    }
    let mut direction = r.vec3(r.u32(DIRECTION)?)?;
    let mut colour = r.vec3(r.u32(COLOUR)?)?;
    let count = r.u32(sky.wrapping_add(0x78))?;
    if count as i32 > 0 {
        let lights = r.u32(sky.wrapping_add(0x7c))?;
        let mut weight = 0;
        for n in 0..count as i32 {
            let light = lights.wrapping_add(n as u32 * 0x34);
            if (r.u32(light.wrapping_add(0x2c))? as i32) <= 0 {
                continue;
            }
            let yaw = r.u32(light.wrapping_add(0xc))?;
            let pitch = r.u32(light.wrapping_add(0x10))?;
            direction = add_light(r, direction, yaw, pitch)?;
            let power = r.u32(light.wrapping_add(0x30))?;
            if n == 0 {
                colour = r.vec3(power.wrapping_add(4))?;
                weight = r.u32(power.wrapping_add(0x10))?;
                continue;
            }
            let w = r.u32(power.wrapping_add(0x10))?;
            if !r.above(w, 0)? {
                continue;
            }
            let one = r.u32(ONE)?;
            let share = r.div(weight, w)?;
            let k = super::fp(r.sse.int(n))?;
            let k = r.add(k, r.u32(BIAS)?)?;
            let share = r.div(share, k)?;
            let t = r.sub(one, share)?;
            let t = super::fp(r.sse.clamp(t, 0, one))?;
            let towards = r.vec3(power.wrapping_add(4))?;
            colour = blend(r, colour, towards, t)?;
            weight = r.add(weight, w)?;
        }
        direction = normalise(r, direction)?;
        let reach = r.u32(REACH)?;
        let mut delta = [0; 3];
        for (d, v) in delta.iter_mut().zip(direction) {
            *d = r.mul(v, reach)?;
        }
        let delta_at = entry.wrapping_sub(0x74);
        r.set_vec3(delta_at, delta)?;
        let query = entry.wrapping_sub(0xb4);
        push_args(r, query, ret::SKY_QUERY, &[FLAGS, centre, delta_at, object, NONE, record])?;
        let open = r.j.query(&mut *r.g, query)? && r.u16(record.wrapping_add(0x5a))? == 0xffff;
        if !open {
            colour = r.vec3(sky.wrapping_add(0x34))?;
        }
    } else {
        let v = r.u32(UNLIT)?;
        colour = r.vec3(sky.wrapping_add(0x34))?;
        direction = normalise(r, [v; 3])?;
    }
    r.set_u32(out, direction[0])?;
    r.set_u32(out.wrapping_add(4), direction[1])?;
    r.set_u32(out.wrapping_add(0xc), SKY_SECOND)?;
    r.set_u32(out.wrapping_add(8), direction[2])?;
    let one = r.u32(ONE)?;
    for c in &mut colour {
        *c = super::fp(r.sse.clamp(*c, 0, one))?;
    }
    let packed = pack(r, colour)?;
    r.set_u32(out.wrapping_add(0x10), packed)?;
    Ok(0)
}

/// `0x000d1b1c`..`0x000d1b5d`: the light's direction from its yaw and
/// pitch on the x87, added to `d`. x is added on the stack; y and z are
/// stored as f32 first.
fn add_light<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, d: [u32; 3], yaw: u32, pitch: u32) -> Result<[u32; 3], GuestFault> {
    use super::fp;
    let mut x = r.x87();
    fp(x.fld(pitch))?;
    fp(x.fcos())?;
    fp(x.fld(yaw))?;
    fp(x.fcos())?;
    fp(x.arith_st(arith_op::MUL, 1))?;
    fp(x.fld(yaw))?;
    fp(x.fsin())?;
    fp(x.arith_st(arith_op::MUL, 2))?;
    let dy = fp(x.fstp_m32())?;
    fp(x.fld(pitch))?;
    fp(x.fsin())?;
    let dz = fp(x.fstp_m32())?;
    fp(x.fadd_m(d[0]))?;
    let sx = fp(x.fstp_m32())?;
    fp(x.fstp_st(0))?;
    fp(x.fld(dy))?;
    fp(x.fadd_m(d[1]))?;
    let sy = fp(x.fstp_m32())?;
    fp(x.fld(dz))?;
    fp(x.fadd_m(d[2]))?;
    let sz = fp(x.fstp_m32())?;
    Ok([sx, sy, sz])
}

/// `0x00131c20` with EAX 0: `a * (1 - t) + b * t`, each axis.
pub(super) fn blend<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, a: [u32; 3], b: [u32; 3], t: u32) -> Result<[u32; 3], GuestFault> {
    let rest = r.sub(r.u32(ONE)?, t)?;
    let mut out = [0; 3];
    for i in 0..3 {
        let x = r.mul(a[i], rest)?;
        let y = r.mul(b[i], t)?;
        out[i] = r.add(x, y)?;
    }
    Ok(out)
}

/// `0x00030bf0(eax = v)`: the length on the x87, stored as f32; a vector
/// whose unrounded length is under `[0x0045dbdc]` stays.
pub(super) fn normalise<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, v: [u32; 3]) -> Result<[u32; 3], GuestFault> {
    use super::fp;
    use arith_op::{ADD, MUL};
    let mut x = r.x87();
    fp(x.fld(v[2]))?;
    fp(x.fld(v[1]))?;
    fp(x.fld(v[0]))?;
    fp(x.fld_st(0))?;
    fp(x.arith_st(MUL, 1))?;
    fp(x.fld_st(2))?;
    fp(x.arith_st(MUL, 3))?;
    fp(x.arith_reg(ADD, 1, 0, true))?;
    fp(x.fld_st(3))?;
    fp(x.arith_st(MUL, 4))?;
    fp(x.arith_reg(ADD, 1, 0, true))?;
    fp(x.unary(unary_op::SQRT))?;
    fp(x.fstp_st(3))?;
    fp(x.fstp_st(0))?;
    fp(x.fstp_st(0))?;
    let length = fp(x.fst_m32())?;
    fp(x.unary(unary_op::ABS))?;
    fp(x.fld(r.u32(LEAST)?))?;
    let flags = fp(x.fcomip(1))?;
    fp(x.fstp_st(0))?;
    if flags & 0x41 == 0 {
        return Ok(v);
    }
    let inverse = r.div(r.u32(ONE)?, length)?;
    Ok([r.mul(v[0], inverse)?, r.mul(inverse, v[1])?, r.mul(inverse, v[2])?])
}

/// `0x00131ed0`: each channel times `[0x0045dc38]` on the x87, `fistp`'d
/// and masked to a byte, red at bit 16.
pub(super) fn pack<M: Memory, J: Collide<M>>(r: &mut Run<'_, M, J>, [red, green, blue]: [u32; 3]) -> Result<u32, GuestFault> {
    use super::fp;
    let mul = arith_op::MUL;
    let mut x = r.x87();
    fp(x.fld(red))?;
    fp(x.fld(green))?;
    fp(x.fld(blue))?;
    fp(x.fld(r.u32(BYTE_SCALE)?))?;
    fp(x.arith_reg(mul, 3, 0, false))?;
    fp(x.arith_reg(mul, 2, 0, false))?;
    fp(x.arith_reg(mul, 1, 0, true))?;
    let b = fp(x.fistp_m32())? as u32 & 0xff;
    let g = fp(x.fistp_m32())? as u32 & 0xff;
    let rr = fp(x.fistp_m32())? as u32 & 0xff;
    Ok(rr << 16 | g << 8 | b)
}
