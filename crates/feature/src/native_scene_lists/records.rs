//! The object arm of the shared record writer in the section-emission
//! loop. Resources and node palettes have already been chosen there.

use super::objects::{ACTIVE_BSP, LIGHT_VIS, MAIN_VIS, VIEW_KIND};
use super::*;
use native_render::skip::{RENDER_RECORD_SIZE, RENDER_RECORDS};

const COUNT: u32 = 0x004c_0b78;
const MAIN_COUNT: u32 = 0x004c_0b7c;
const CURRENT_VIS: u32 = 0x004c_0b80;
const PIXEL_SIZES: u32 = 0x004c_0b84;
const ATTACHMENTS: u32 = 0x004c_0ed6;
const ATTACHMENT_DATA: u32 = 0x004c_1228;
const ATTACHMENT_COUNT: u32 = 0x004c_1528;
const GROUP_DATA: u32 = 0x004c_152c;
const GROUP_COUNT: u32 = 0x004c_19ac;
const LIGHTMAP: u32 = 0x004e_0344;
const FULL_RECT: u32 = 0x0050_9410;
const EMPTY_RECT: u32 = 0x0050_9411;
const RECT_MIN: u32 = 0x0044_09d4;
const RECT_MAX: u32 = 0x0044_aed4;
const PIXEL_QUANTIZE: u32 = 0x0045_dc20;
const SHADE_QUANTIZE: u32 = 0x0045_dd5c;
const RECORD_LIMIT: u32 = 0x352;

pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::ObjectRecord, append(m, cpu))
}

fn append(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    fp_supported(cpu)?;
    let count = m.i32(COUNT)?;
    let count = u32::try_from(count).map_err(|_| Refusal::Layout)?;
    let mut out = Output {
        pop: 0x54,
        ..Output::default()
    };
    if count >= RECORD_LIMIT || cpu.eax == NONE {
        out.eax = Some(NONE);
        return Ok(out);
    }

    // Stack arguments are already pushed at this instruction site.
    let args = |offset| stack(m, cpu, offset);
    let model = args(4)?;
    if model == NONE || args(0)? & (1 << 12) != 0 {
        return Err(Refusal::Unsupported);
    }
    let model_data = tag(m, ptr(m, TAGS)?, model)?;
    let section_index = cpu.eax & 0x1ff;
    if section_index >= m.u32(at(model_data, 0x24)?)? {
        return Err(Refusal::Layout);
    }
    let section = at(ptr(m, at(model_data, 0x28)?)?, section_index * 0x5c)?;
    // The failed-geometry exit leaves a partly initialized scratch row.
    // Keep the guest writer for this path.
    let _geometry = ptr(m, at(section, 0x34)?)?;
    let lighting = args(0x10)?;
    let placement = cpu.ecx & 0x7ff;
    let light_class = light_class(m, placement, lighting)?;
    let vis = ptr(m, CURRENT_VIS)?;
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }

    let address = at(RENDER_RECORDS, count * RENDER_RECORD_SIZE)?;
    let mut row = [0u8; 0x20];
    m.read(address, &mut row)?;
    let mut flags = (args(0)? & 0x1f_ffff) | ((cpu.edx & 0xff) << 21) | (light_class << 29);
    let group = args(0x28)? & 0x1f;
    let packed = (args(0xc)? & 0xf)
        | (group << 4)
        | (section_index << 9)
        | (placement << 18)
        | (u32::from(m.u16(at(section, 0x18)?)?) << 29);
    let attachment = args(0x20)?;
    if group != 0x1f {
        group_data(m, &mut out, cpu, group, attachment)?;
    }
    let attachment_index = if attachment != NONE {
        let end = m.u32(ATTACHMENT_COUNT)?;
        if end > 0xc0 {
            return Err(Refusal::Layout);
        }
        if end < 0xc0 {
            out.word(at(ATTACHMENT_DATA, end * 4)?, attachment);
            out.word(ATTACHMENT_COUNT, end + 1);
            end as u8
        } else {
            u8::MAX
        }
    } else {
        u8::MAX
    };
    out.byte(at(ATTACHMENTS, count)?, attachment_index);
    let size = f32::from_bits(args(0x24)?) * m.f32(PIXEL_QUANTIZE)?;
    out.byte(at(PIXEL_SIZES, count)?, quantize(size, f32::floor)?);
    if lighting != 0 && attachment != NONE {
        let mirror = args(0x38)? as u8 != 0;
        let at_byte = if mirror { 0x76 } else { 0x77 };
        let value =
            f32::from_bits(args(if mirror { 0x3c } else { 0x40 })?) * m.f32(SHADE_QUANTIZE)?;
        out.byte(at(lighting, at_byte)?, quantize(value, f32::trunc)?);
    }

    let eax_high = if flags & 0x40 != 0 {
        let words = u32::from(m.u16(at(section, 0x28)?)?).div_ceil(32);
        let mut mask = 0;
        if words != 0 {
            let used_addr = at(vis, 0xa5c)?;
            let used = u32::from(m.u16(used_addr)?);
            if used > 512 {
                return Err(Refusal::Layout);
            }
            mask = at(vis, 0x25c)?;
            if used + words < 512 {
                mask = at(mask, used * 4)?;
                out.half(used_addr, (used + words) as u16);
            }
        }
        put(&mut row, 0x14, mask);
        if mask == 0 {
            flags = (flags & !0x40) | 1;
        }
        mask
    } else {
        flags
    };
    put(&mut row, 0, flags);
    put(&mut row, 4, model);
    put(&mut row, 8, args(8)?);
    put(&mut row, 0xc, packed);
    put(&mut row, 0x10, lighting);
    row[0x18..0x1a].copy_from_slice(&(args(0x14)? as u16).to_le_bytes());
    row[0x1a] = args(0x18)? as u8;
    row[0x1b] = (row[0x1b] & 0xf0) | (args(0x48)? as u8 & 0xf);
    put(&mut row, 0x1c, args(0x1c)?);
    out.array(address, row.to_vec());
    out.word(COUNT, count + 1);
    if kind == 0 {
        out.word(MAIN_COUNT, m.u32(MAIN_COUNT)?.wrapping_add(1));
    }
    out.eax = Some((eax_high & 0xffff_0000) | count);
    Ok(out)
}

fn put(row: &mut [u8], off: usize, value: u32) {
    row[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

fn quantize(value: f32, integer: impl FnOnce(f32) -> f32) -> Result<u8> {
    if !value.is_finite() || value >= i32::MAX as f32 || value < i32::MIN as f32 {
        return Err(Refusal::Unsupported);
    }
    Ok(integer(value).clamp(0.0, 255.0) as u8)
}

fn light_class(m: &dyn GuestMemory, placement: u32, lighting: u32) -> Result<u32> {
    let lightmap = m.u32(LIGHTMAP)?;
    if lightmap != 0 && m.i32(at(lightmap, 0x80)?)? > 0 {
        let bsp = m.u32(ACTIVE_BSP)?;
        if bsp != 0 {
            let group = ptr(m, at(lightmap, 0x84)?)?;
            if m.u32(at(group, 0x1c)?)? != NONE && m.u32(at(group, 4)?)? == m.u32(at(bsp, 8)?)? {
                return if placement == 0x7ff {
                    Ok(2)
                } else {
                    Err(Refusal::Unsupported)
                };
            }
        }
    }
    if lighting == 0 {
        return Ok(3);
    }
    let owner = m.u32(lighting)?;
    if owner == NONE {
        return Ok(3);
    }
    let body = Pool::read(m, ptr(m, OBJECTS)?, 12)?.body(m, owner)?;
    Ok(if m.u8(at(body, 7)?)? & 1 != 0 { 2 } else { 3 })
}

fn group_data(
    m: &dyn GuestMemory,
    out: &mut Output,
    cpu: &CpuAbi,
    group: u32,
    attachment: u32,
) -> Result<()> {
    let address = at(GROUP_DATA, group * 0x24)?;
    let args = |offset| stack(m, cpu, offset);
    out.word(GROUP_COUNT, m.u32(GROUP_COUNT)?.max(group + 1));
    let value = args(0x2c)?;
    out.word(at(address, 0x20)?, value);
    if attachment == NONE {
        return Ok(());
    }
    let limits = if args(0x30)? as u8 != 0 || m.u8(FULL_RECT)? != 0 {
        [m.u32(RECT_MIN)?, m.u32(RECT_MAX)?]
    } else if args(0x34)? as u8 != 0 || m.u8(EMPTY_RECT)? != 0 {
        [m.u32(RECT_MAX)?, m.u32(RECT_MIN)?]
    } else {
        // Its alternative projects a sphere to a scissor rectangle.
        return Err(Refusal::Unsupported);
    };
    let centre = args(0x4c)?;
    for axis in 0..3 {
        out.word(at(address, 4 + axis * 4)?, m.u32(at(centre, axis * 4)?)?);
    }
    out.word(address, args(0x44)?);
    for i in 0..4 {
        out.word(at(address, 0x10 + i * 4)?, limits[i as usize % 2]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_record_quantization_uses_guest_bounds() {
        assert_eq!(quantize(34.8, f32::floor), Ok(34));
        assert_eq!(quantize(-1.0, f32::trunc), Ok(0));
        assert_eq!(quantize(256.0, f32::floor), Ok(255));
        assert_eq!(quantize(f32::NAN, f32::floor), Err(Refusal::Unsupported));
    }
}
