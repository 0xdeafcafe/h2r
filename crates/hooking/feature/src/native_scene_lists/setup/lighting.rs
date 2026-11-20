//! Object lighting cache lookup and the render setup's lighting copies.

use super::staged::Staged;
use super::*;
use engine::datum::{DatumArray, layout};

const LIGHTING_CACHE: u32 = 0x0050_9434;
const DEFAULT_LIGHTING: u32 = 0x004d_e2f4;
const ACTIVE_BSP: u32 = 0x004e_0348;

pub(super) fn cache(m: &mut Staged<'_>, handle: u32, body: u32, frame: u32) -> Result<u32> {
    let header = ptr(m, LIGHTING_CACHE)?;
    let pool = Pool::read(m, header, 0x100)?;
    let datum = m.u32(at(body, 0xcc)?)?;
    if datum != NONE && (datum & 0xffff) < pool.end {
        let index = datum & 0xffff;
        let entry = at(pool.data, index * pool.stride)?;
        if m.u32(at(entry, 4)?)? == handle {
            return Ok(entry);
        }
    }
    let array = DatumArray::at(header);
    let scan = array.header_field(m, layout::FREE_SCAN_START)?;
    let max = array.header_field(m, layout::MAXIMUM_COUNT)?;
    if !(0..=0x10000).contains(&max) || !(0..=max).contains(&scan) || pool.end > max as u32 {
        return Err(Refusal::Lighting);
    }
    ptr(m, at(header, layout::BITMAP)?)?;
    let (datum, entry) = if let Some(datum) = array.new_datum(m)? {
        (
            datum.raw(),
            at(pool.data, u32::from(datum.index()) * pool.stride)?,
        )
    } else {
        reuse_entry(m, array, pool, frame)?
    };

    m.set_u32(at(entry, 4)?, handle)?;
    m.set_u32(at(entry, 8)?, NONE)?;
    m.set_u32(at(entry, 0xc)?, NONE)?;
    m.set_u16(at(entry, 2)?, 0)?;
    initialize_light(m, at(entry, 0x44)?, handle)?;
    // The initializer stores the callback address used by the original
    // lighting payload.
    m.set_u32(at(entry, 0x48)?, 0x000b_e8b0)?;
    m.set_u32(at(entry, 0xbc)?, NONE)?;
    let bytes = m.u16(at(body, 0x118)?)? as i16;
    let pos = m.u16(at(body, 0x11a)?)? as i16;
    if !(0..=160).contains(&bytes) || bytes >= 10 && pos < 0 {
        return Err(Refusal::Lighting);
    }
    let count = bytes as usize / 10;
    if count != 0 {
        let permutations = m.bytes(at(body, pos as u32)?, count)?;
        for view in 0..4 {
            m.write(at(entry, 0xc0 + view * 0x10)?, &permutations)?;
        }
    }
    m.set_u32(at(body, 0xcc)?, datum)?;
    colors(m, body, entry)?;
    Ok(entry)
}

fn reuse_entry(
    m: &dyn GuestMemory,
    array: DatumArray,
    pool: Pool,
    frame: u32,
) -> Result<(u32, u32)> {
    let mut at_index = 0;
    let mut oldest = m.f32(0x0044_09d4)?;
    let wrapped_age = m.f32(0x0045_dccc)?;
    let mut found = None;
    while let Some(index) = array.next_index(m, at_index)? {
        if index < 0 || index as u32 >= pool.end {
            return Err(Refusal::Lighting);
        }
        let entry = at(pool.data, index as u32 * pool.stride)?;
        let age = frame.wrapping_sub(m.u32(at(entry, 0xc)?)?) as i32 as f32;
        let age = if age < 0.0 { wrapped_age } else { age };
        if age > oldest {
            oldest = age;
            found = Some(((u32::from(m.u16(entry)?) << 16) | index as u32, entry));
        }
        at_index = index + 1;
    }
    found.ok_or(Refusal::Lighting)
}

fn initialize_light(m: &mut Staged<'_>, entry: u32, handle: u32) -> Result<()> {
    let angle = |addr| -> Result<f64> {
        let mut bytes = [0; 8];
        m.read(addr, &mut bytes)?;
        Ok(f64::from_le_bytes(bytes))
    };
    let elevation = angle(0x0045_df90)?;
    let azimuth = angle(0x0045_dbe0)?;
    if !elevation.is_finite() || !azimuth.is_finite() {
        return Err(Refusal::Lighting);
    }
    let direction = [
        (azimuth.cos() * elevation.cos()) as f32,
        (azimuth.sin() * elevation.cos()) as f32,
        elevation.sin() as f32,
    ];
    let ambient = m.u32(0x0045_df88)?;
    let diffuse = m.u32(0x0045_df84)?;
    let strength = m.u32(0x0045_dd30)?;
    m.set_u32(entry, handle)?;
    m.set_u32(at(entry, 4)?, 0)?;
    m.set_u32(at(entry, 8)?, NONE)?;
    for axis in 0..3 {
        let offset = axis as u32 * 4;
        let d = direction[axis];
        m.set_u32(at(entry, 0x10 + offset)?, ambient)?;
        m.set_u32(at(entry, 0x30 + offset)?, diffuse)?;
        m.set_u32(at(entry, 0x48 + offset)?, ambient)?;
        m.set_u32(at(entry, 0x1c + offset)?, d.to_bits())?;
        m.set_u32(at(entry, 0x3c + offset)?, d.to_bits())?;
        m.set_u32(at(entry, 0x54 + offset)?, (0.0 - d).to_bits())?;
    }
    m.set_u32(at(entry, 0x28)?, strength)?;
    m.set_u32(at(entry, 0x2c)?, strength)?;
    m.set_u16(at(entry, 0x60)?, u16::MAX)?;
    m.set_u8(at(entry, 0x74)?, 0)?;
    m.set_u16(at(entry, 0x76)?, 0)?;
    Ok(())
}

pub(super) fn refresh(
    m: &mut Staged<'_>,
    handle: u32,
    body: u32,
    definition: u32,
    entry: u32,
    frame: u32,
    pixels: f32,
) -> Result<()> {
    if m.u8(at(definition, 0x1c)?)? & 1 != 0 {
        colors(m, body, entry)?;
    }
    let source = if m.u8(at(body, 7)?)? & 1 != 0 {
        at(ptr(m, DEFAULT_LIGHTING)?, 0x1c)?
    } else {
        let bsp_light = m.u16(at(body, 0xd0)?)?;
        if bsp_light != u16::MAX {
            let block = ptr(m, ACTIVE_BSP)?;
            if u32::from(bsp_light) >= m.u32(at(block, 0x22c)?)? {
                return Err(Refusal::Lighting);
            }
            at(ptr(m, at(block, 0x230)?)?, u32::from(bsp_light) * 0x5c + 8)?
        } else {
            let root = lights::root_owner(m, handle)?;
            if root == handle {
                if !keeps_light_sample(m, entry, body, frame, pixels)? {
                    return Err(Refusal::Lighting);
                }
                return Ok(());
            }
            let root_body = Pool::read(m, ptr(m, OBJECTS)?, 12)?.body(m, root)?;
            let root_entry = cache(m, root, root_body, frame)?;
            if m.u32(at(entry, 4)?)? != handle {
                return Err(Refusal::Lighting);
            }
            let sample = m.bytes(at(root_entry, 0x1c)?, 0x14)?;
            m.write(at(entry, 0x30)?, &sample)?;
            m.write(at(entry, 0x1c)?, &sample)?;
            at(root_entry, 0x54)?
        }
    };
    let light = m.bytes(source, 0x54)?;
    m.write(at(entry, 0x54)?, &light)?;
    Ok(())
}

fn colors(m: &mut Staged<'_>, body: u32, entry: u32) -> Result<()> {
    let size = m.u16(at(body, 0x124)?)? as i16;
    let offset = m.u16(at(body, 0x126)?)? as i16;
    if size < 0 || (size >= 24 && offset < 0) {
        return Err(Refusal::Lighting);
    }
    let count = (size as u32) / 24;
    for index in 0..count.min(4) {
        let [r, g, b] = m.vec3(at(body, offset as u32 + (count + index) * 12)?)?;
        let scale = f64::from(m.f32(BYTE_SCALE)?);
        let channel = |value: f32| -> Result<u32> {
            let value = (f64::from(value) * scale).round_ties_even();
            if value.is_finite() && (i32::MIN as f64..=i32::MAX as f64).contains(&value) {
                Ok(value as i32 as u32 & 0xff)
            } else {
                Err(Refusal::Lighting)
            }
        };
        let rgb = channel(r)? << 16 | channel(g)? << 8 | channel(b)?;
        m.set_u32(at(entry, 0xa8 + index * 4)?, rgb)?;
    }
    m.set_u8(at(entry, 0xb8)?, count.min(4) as u8)?;
    Ok(())
}

fn keeps_light_sample(
    m: &dyn GuestMemory,
    entry: u32,
    body: u32,
    frame: u32,
    pixels: f32,
) -> Result<bool> {
    let last_sample = m.u32(at(entry, 8)?)?;
    if last_sample == frame {
        return Ok(true);
    }
    if m.u8(at(entry, 2)?)? != 0 {
        return Ok(m.u8(at(entry, 3)?)? != 0 && m.u8(at(body, 0xc1)?)? & 1 != 0);
    }
    let mut period = 60;
    for (threshold, frames) in [
        (0x0045_e17c, 3),
        (0x0045_e314, 6),
        (0x0044_5420, 9),
        (0x0045_dc94, 15),
        (0x0045_dc00, 45),
    ] {
        if pixels > m.f32(threshold)? {
            period = frames;
            break;
        }
    }
    let age = frame.wrapping_sub(last_sample) as i32;
    let age = if age < 0 { 1 } else { age };
    Ok(last_sample != NONE && age <= period)
}
