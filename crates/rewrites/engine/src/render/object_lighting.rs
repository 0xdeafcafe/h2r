//! The object lighting cache (`[0x00509434]`): one `0x100`-byte record a
//! lit object, named by the object's body `+0xcc` and owned through the
//! record's `+4`. The ports here make every write the guest makes, under
//! the caller's float state:
//!
//! | Guest | Port | Does |
//! |---|---|---|
//! | `0x0003dc90` | [`sample`] | the default lighting, or the object's record's lighting `+0x54` once it is lit; a child takes its root's |
//! | `0x000baf80` | [`root`] | the root of an object's parent chain |
//! | `0x0003ddd0` | [`record`] | the object's record: its own, a new one, or the oldest one taken over |
//! | `0x0003dd10` | [`colours`] | the record's change colours `+0xa8..+0xb8` |
//! | `0x0002c3b0` | `default_light` | a new record's light at `+0x44` |
//! | `0x000bad50` | `change_colour` | one of the body's change colours |
//! | `0x00131ed0` | `pack` | a colour as a D3D word's low 24 bits |
//!
//! A new record is unlit (`+3` 0) until the object setup samples it
//! (`0x0003dfe0`), so its first [`sample`] gives 0.

use crate::datum::DatumArray;
use crate::handle::DatumHandle;
use crate::light_constants::{Error, Fpu, Sse, X87};
use crate::memory::GuestMemory;

/// The record array's header pointer.
pub const CACHE: u32 = 0x0050_9434;
const OBJECTS: u32 = 0x004e_0300;
const TAGS: u32 = 0x004e_3b44;
const DEFAULT_LIGHTING: u32 = 0x004d_e2f4;
const FRAME: u32 = 0x004b_a034;
/// The age a record must pass to be taken over, and the age a record
/// stamped in the future counts as.
const OLDEST_FLOOR: u32 = 0x0044_09d4;
const FUTURE_AGE: u32 = 0x0045_dccc;
const LIGHT_ELEVATION: u32 = 0x0045_df90;
const LIGHT_AZIMUTH: u32 = 0x0045_dbe0;
const AMBIENT: u32 = 0x0045_df88;
const DIFFUSE: u32 = 0x0045_df84;
const STRENGTH: u32 = 0x0045_dd30;
const BYTE_SCALE: u32 = 0x0045_dc38;
/// What `0x0003ddd0` stores at record `+0x48`: the lighting payload's
/// callback.
const CALLBACK: u32 = 0x000b_e8b0;
const NONE: u32 = u32::MAX;
const RECORD_SHIFT: u32 = 8;

/// What `0x0003dc90` returns: EAX the lighting (0 for an unlit record or
/// none), and EDX as it leaves it on the arms that give lighting (the
/// caster passes its high bytes on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub eax: u32,
    pub edx: u32,
}

fn body(m: &dyn GuestMemory, object: u32) -> Result<u32, Error> {
    let headers = m.u32(m.u32(OBJECTS)?.wrapping_add(0x44))?;
    Ok(m.u32(headers.wrapping_add((object & 0xffff) * 12 + 8))?)
}

fn record_at(m: &dyn GuestMemory, handle: u32) -> Result<u32, Error> {
    let data = m.u32(m.u32(CACHE)?.wrapping_add(0x44))?;
    Ok(data.wrapping_add((handle & 0xffff) << RECORD_SHIFT))
}

/// `0x0003dc90`: the lighting a root object draws with. An object whose
/// body `+7` bit 0 is set takes the default lighting `[0x004d_e2f4] +
/// 0x1c`; any other gets its record ([`record`]) and that record's
/// `+0x54` once it is lit, else 0.
pub fn sample(m: &mut dyn GuestMemory, object: u32, fpu: Fpu) -> Result<Sample, Error> {
    let headers = m.u32(m.u32(OBJECTS)?.wrapping_add(0x44))?;
    let b = body(m, object)?;
    let flags = m.u8(b.wrapping_add(7))?;
    if flags & 1 != 0 {
        return Ok(Sample {
            eax: m.u32(DEFAULT_LIGHTING)?.wrapping_add(0x1c),
            edx: (headers & !0xff) | u32::from(flags),
        });
    }
    if m.u32(b.wrapping_add(0x14))? != NONE {
        // 0x0003dcc8: a child asks for its parent's (0x000baf80) and
        // goes round again.
        return sample(m, root(m, object)?, fpu);
    }
    let handle = record(m, object, fpu)?;
    if handle == NONE {
        return Ok(Sample { eax: 0, edx: 0 });
    }
    let data = m.u32(m.u32(CACHE)?.wrapping_add(0x44))?;
    let entry = data.wrapping_add((handle & 0xffff) << RECORD_SHIFT);
    let eax = if m.u8(entry.wrapping_add(3))? != 0 {
        entry.wrapping_add(0x54)
    } else {
        0
    };
    Ok(Sample { eax, edx: data })
}

/// `0x000baf80`: the root of `object`'s parent chain (body `+0x14`, -1 for
/// none). A chain longer than the object array is a cycle the guest would
/// loop on forever.
pub fn root(m: &dyn GuestMemory, object: u32) -> Result<u32, Error> {
    let mut current = object;
    for _ in 0..=0xffff {
        let parent = m.u32(body(m, current)?.wrapping_add(0x14))?;
        if parent == NONE {
            return Ok(current);
        }
        current = parent;
    }
    unimplemented!("residency: lighting parent chain of {object:#010x} loops")
}

/// `0x0003ddd0`: the object's record handle, or `NONE` when the array is
/// full and no record is older than `[0x004409d4]` frames. The object's
/// own record (body `+0xcc`, whose `+4` is the object) is given as it is;
/// otherwise a new record (`datum_new`) or the oldest is made the
/// object's: its owner, stamps, default light, permutations and colours.
pub fn record(m: &mut dyn GuestMemory, object: u32, fpu: Fpu) -> Result<u32, Error> {
    let b = body(m, object)?;
    let pool = m.u32(CACHE)?;
    let current = m.u32(b.wrapping_add(0xcc))?;
    if current != NONE && m.u32(record_at(m, current)?.wrapping_add(4))? == object {
        return Ok(current);
    }
    let array = DatumArray::at(pool);
    let handle = match array.new_datum(m)? {
        Some(h) => h.raw(),
        None => match oldest(m, array, fpu)? {
            NONE => return Ok(NONE),
            h => h,
        },
    };
    let entry = record_at(m, handle)?;
    m.set_u32(entry + 4, object)?;
    m.set_u32(entry + 8, NONE)?;
    m.set_u32(entry + 0xc, NONE)?;
    m.set_u8(entry + 2, 0)?;
    m.set_u8(entry + 3, 0)?;
    default_light(m, entry + 0x44, fpu)?;
    m.set_u32(entry + 0x44, object)?;
    m.set_u32(entry + 0x48, CALLBACK)?;
    m.set_u32(entry + 0xbc, NONE)?;
    // 0x0003df4b: the permutation bytes, a tenth of body +0x118 from
    // body + body[+0x11a], copied to each of the four views.
    let size = i32::from(m.u16(b.wrapping_add(0x118))? as i16) / 10;
    let from = b.wrapping_add(i32::from(m.u16(b.wrapping_add(0x11a))? as i16) as u32);
    if size < 0 {
        unimplemented!("residency: lighting record permutations of {size} bytes");
    }
    let bytes = m.bytes(from, size as usize)?;
    for view in 0..4 {
        m.write(entry + 0xc0 + view * 0x10, &bytes)?;
    }
    m.set_u32(b.wrapping_add(0xcc), handle)?;
    colours(m, object, true, fpu)?;
    Ok(handle)
}

/// `0x0003de3e..0x0003dedb`: the live record whose stamp `+0xc` is the
/// most frames old, past the floor; a stamp ahead of the frame counts as
/// `[0x0045dccc]`.
fn oldest(m: &dyn GuestMemory, array: DatumArray, fpu: Fpu) -> Result<u32, Error> {
    let mut sse = Sse { mxcsr: fpu.mxcsr };
    let mut best = m.u32(OLDEST_FLOOR)?;
    let future = m.u32(FUTURE_AGE)?;
    let mut chosen = NONE;
    let handle = |m: &dyn GuestMemory, index: Option<i32>| -> Result<u32, Error> {
        Ok(match index {
            Some(i) => DatumHandle::new(array.salt_at(m, i)?, i as u32).raw(),
            None => NONE,
        })
    };
    let mut current = handle(m, array.next_index(m, 0)?)?;
    while current != NONE {
        let index = current & 0xffff;
        let stamp = m.u32(record_at(m, current)?.wrapping_add(0xc))?;
        let mut age = sse.int(m.u32(FRAME)?.wrapping_sub(stamp) as i32)?;
        if sse.above(0, age)? {
            age = future;
        }
        if sse.above(age, best)? {
            best = age;
            chosen = current;
        }
        current = handle(m, array.next_index(m, index as i32 + 1)?)?;
    }
    Ok(chosen)
}

/// `0x0002c3b0`: the default light at `light` (record `+0x44`), its
/// direction from the elevation and azimuth doubles by the x87.
fn default_light(m: &mut dyn GuestMemory, light: u32, fpu: Fpu) -> Result<(), Error> {
    let mut sse = Sse { mxcsr: fpu.mxcsr };
    let double =
        |at| -> Result<u64, Error> { Ok(u64::from(m.u32(at)?) | u64::from(m.u32(at + 4)?) << 32) };
    let elevation = double(LIGHT_ELEVATION)?;
    let azimuth = double(LIGHT_AZIMUTH)?;
    let ambient = m.u32(AMBIENT)?;
    let diffuse = m.u32(DIFFUSE)?;
    let strength = m.u32(STRENGTH)?;
    let mut x = X87::new(fpu.x87_control);
    x.fld_m64(elevation)?;
    x.fcos()?;
    m.set_u8(light + 0x74, 0)?;
    m.set_u32(light + 4, 0)?;
    m.set_u8(light + 0x77, 0)?;
    m.set_u8(light + 0x76, 0)?;
    m.set_u32(light, NONE)?;
    m.set_u32(light + 8, NONE)?;
    for at in [0x10, 0x48, 0x14, 0x4c, 0x18, 0x50] {
        m.set_u32(light + at, ambient)?;
    }
    for at in [0x30, 0x34, 0x38] {
        m.set_u32(light + at, diffuse)?;
    }
    m.set_u32(light + 0x28, strength)?;
    m.set_u32(light + 0x2c, strength)?;
    let mul = x87::abi::arith_op::MUL;
    x.fld_m64(azimuth)?;
    x.fcos()?;
    x.arith_reg(mul, 0, 1, false)?;
    let dx = x.fstp_m32()?;
    m.set_u32(light + 0x3c, dx)?;
    x.fld_m64(azimuth)?;
    x.fsin()?;
    x.arith_reg(mul, 0, 1, false)?;
    let dy = x.fstp_m32()?;
    m.set_u32(light + 0x40, dy)?;
    x.fstp_st(0)?;
    x.fld_m64(elevation)?;
    x.fsin()?;
    let dz = x.fstp_m32()?;
    m.set_u32(light + 0x44, dz)?;
    let direction = [dx, dy, dz];
    for (i, d) in direction.into_iter().enumerate() {
        m.set_u32(light + 0x54 + 4 * i as u32, sse.sub(0, d)?)?;
    }
    for (i, d) in direction.into_iter().enumerate() {
        m.set_u32(light + 0x1c + 4 * i as u32, d)?;
    }
    m.set_u16(light + 0x60, 0xffff)?;
    Ok(())
}

/// `0x0003dd10`: the object's record's change colours, packed, and their
/// count at `+0xb8`. Unless `force`, only for a definition that asks for
/// them (tag `+0x1c` bit 0). An object without a record has none.
pub fn colours(m: &mut dyn GuestMemory, object: u32, force: bool, fpu: Fpu) -> Result<(), Error> {
    let b = body(m, object)?;
    let handle = m.u32(b.wrapping_add(0xcc))?;
    if handle == NONE {
        return Ok(());
    }
    if !force {
        let datum = m.u32(b)?;
        let tag = m.u32(TAGS)?.wrapping_add((datum & 0xffff) << 4);
        let definition = m.u32(tag.wrapping_add(8))?;
        if m.u8(definition.wrapping_add(0x1c))? & 1 == 0 {
            return Ok(());
        }
    }
    let entry = record_at(m, handle)?;
    m.set_u8(entry + 0xb8, 0)?;
    for i in 0..4 {
        if let Some(colour) = change_colour(m, b, i)? {
            m.set_u32(entry + 0xa8 + 4 * i, pack(m, colour, fpu)?)?;
            let count = m.u8(entry + 0xb8)?;
            m.set_u8(entry + 0xb8, count.wrapping_add(1))?;
        }
    }
    Ok(())
}

/// `0x000bad50`: change colour `index` of the body, the second half of
/// the 12-byte colours at `body + body[+0x126]`, `body[+0x124]` bytes.
fn change_colour(m: &dyn GuestMemory, body: u32, index: u32) -> Result<Option<[u32; 3]>, Error> {
    let size = i32::from(m.u16(body.wrapping_add(0x124))? as i16) as u32;
    let twelfths = ((u64::from(size) * 0xaaaa_aaab) >> 32) as u32 >> 3;
    let half = twelfths as i32 >> 1;
    if index as i32 >= half {
        return Ok(None);
    }
    let base = body.wrapping_add(i32::from(m.u16(body.wrapping_add(0x126))? as i16) as u32);
    let at = base.wrapping_add((half as u32 + index) * 12);
    Ok(Some([m.u32(at)?, m.u32(at + 4)?, m.u32(at + 8)?]))
}

/// `0x00131ed0`: each channel times `[0x0045dc38]`, rounded by the x87
/// and masked to a byte (no clamp), red at bit 16.
fn pack(m: &dyn GuestMemory, [r, g, b]: [u32; 3], fpu: Fpu) -> Result<u32, Error> {
    let mul = x87::abi::arith_op::MUL;
    let mut x = X87::new(fpu.x87_control);
    x.fld(r)?;
    x.fld(g)?;
    x.fld(b)?;
    x.fld(m.u32(BYTE_SCALE)?)?;
    x.arith_reg(mul, 3, 0, false)?;
    x.arith_reg(mul, 2, 0, false)?;
    x.arith_reg(mul, 1, 0, true)?;
    let blue = x.fistp_m32()? as u32 & 0xff;
    let green = x.fistp_m32()? as u32 & 0xff;
    let red = x.fistp_m32()? as u32 & 0xff;
    Ok(red << 16 | green << 8 | blue)
}

#[cfg(test)]
mod tests;
