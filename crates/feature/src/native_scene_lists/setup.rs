//! Prepare the object section builder's inputs from the current camera
//! and object. The original tail still owns section residency, render
//! cache updates, node palettes, children and visibility/retry writes.

use super::lights::{MIN_DEPTH, PIXEL_SCALE, PROJECTION_DEPTH, SIZE_FACTOR};
use super::objects::{LIGHT_VIS, List, MAIN_VIS, VIEW_KIND, VIS_FLAGS};
use super::*;

const CAMERA: u32 = 0x004b_9da0;
const CAMERA_DISTANCE_SCALE: u32 = CAMERA + 0x2c;
const OBJECT_SIZE_SCALE: u32 = 0x004b_a028;
const FORCE_FULL_LOD: u32 = 0x004b_a019;
const FADE_SPAN_SCALE: u32 = 0x0045_dda8;
const FADE_MIN: u32 = 0x0045_df60;
const DEFAULT_MODEL_DISTANCE: u32 = 0x0044_aed4;
const ONE: u32 = 0x0045_dbc0;
const BYTE_SCALE: u32 = 0x0045_dc38;
const VIEW_CATEGORY: u32 = 0x004b_9ef0;
const VIEW_SKY_DATUM: u32 = 0x004b_9f8c;
const SKY_FADE_FRACTION: u32 = 0x004b_9ffc;
const SKY_FLAT_AMBIENT: u32 = 0x004b_a004;
const DISTANCE_EPSILON: u32 = 0x0045_dbd8;

/// The two LOD fades as seen at the section builder's caller. Keep the
/// original comparison order around the clamp and near/far transitions.
fn fades(
    distance: f32,
    radius_span: f32,
    range_end: f32,
    minimum: f32,
    near: f32,
    near_start: f32,
    one: f32,
) -> (f32, f32) {
    let detail = if distance >= range_end + radius_span {
        0.0
    } else if distance > range_end {
        let factor = one - (distance - range_end) / radius_span;
        if factor < 0.0 {
            0.0
        } else if factor > one {
            one
        } else if minimum > factor || minimum == factor {
            0.0
        } else {
            factor
        }
    } else {
        one
    };
    let visible = if near <= 0.0 || distance <= near_start {
        one
    } else if distance >= near {
        0.0
    } else {
        one - (distance - near_start) / (near - near_start)
    };
    (visible, detail)
}

fn lod_bytes(
    m: &dyn GuestMemory,
    table: u32,
    handle: u32,
    body: u32,
    distance: f32,
    radius: f32,
) -> Result<(u8, u8)> {
    if m.u8(FORCE_FULL_LOD)? != 0 || m.u32(at(body, 4)?)? & (1 << 19) != 0 {
        return Ok((255, 255));
    }
    let object = tag(m, table, m.u32(body)?)?;
    let model = m.u32(at(object, 0x38)?)?;
    if model == NONE {
        return Ok((0, 0));
    }
    let root = lights::root_owner(m, handle)?;
    let model = if root != handle {
        let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
        let root_body = objects.body(m, root)?;
        let root_object = tag(m, table, m.u32(root_body)?)?;
        let root_model = m.u32(at(root_object, 0x38)?)?;
        if root_model == NONE {
            model
        } else {
            root_model
        }
    } else {
        model
    };
    let distances = at(tag(m, table, model)?, 0x28)?;
    let range = (4i32 - i32::from(m.u16(at(distances, 0x24)?)?)).clamp(0, 4);
    let range_end = if range == 0 {
        m.f32(DEFAULT_MODEL_DISTANCE)?
    } else {
        m.f32(at(distances, 0xc + range as u32 * 4)?)?
    };
    let near = m.f32(distances)?;
    let near_start = m.f32(at(distances, 4)?)?;
    let span = radius * m.f32(FADE_SPAN_SCALE)?;
    let min = m.f32(FADE_MIN)?;
    let one = m.f32(ONE)?;
    let scale = m.f32(BYTE_SCALE)?;
    if ![range_end, near, near_start, span, min, one, scale]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }
    let (visible, detail) = fades(distance, span, range_end, min, near, near_start, one);
    let byte = |v: f32| -> Result<u8> {
        // The CRT helper truncates the x87 product to an integer.
        let scaled = f64::from(v) * f64::from(scale);
        if !(0.0..=255.0).contains(&scaled) {
            return Err(Refusal::Unsupported);
        }
        Ok(scaled as u8)
    };
    Ok((byte(visible)?, byte(detail)?))
}

fn section_flags(m: &dyn GuestMemory, vis: u32, tags: u32, flags: u16) -> Result<u32> {
    let flags = u32::from(flags);
    let mut section = ((((flags >> 2) & 0x2c00) | (flags & 0x4000)) >> 9) | ((flags & 0x200) << 5);
    if m.u8(at(vis, VIS_FLAGS)?)? & 0x80 != 0 {
        section |= 1;
    }
    if m.u32(VIEW_CATEGORY)? == 3 {
        let sky = m.u32(VIEW_SKY_DATUM)?;
        if sky != NONE
            && flags & 0x400 == 0
            && m.f32(SKY_FADE_FRACTION)? == m.f32(DISTANCE_EPSILON)?
            && m.u8(tag(m, tags, sky)?)? & 2 == 0
        {
            section |= 0x100;
        }
        if m.u8(SKY_FLAT_AMBIENT)? == 0 || flags & 0x400 != 0 {
            section |= 0x80;
        }
    }
    Ok(section)
}

pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::ObjectSetup, setup_inputs(m, cpu))
}

fn setup_inputs(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    fp_supported(cpu)?;
    if cpu.x87.control != 0x027f || cpu.x87.tag != 0xffff {
        return Err(Refusal::Unsupported);
    }
    let vis = cpu.esi;
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }
    let list_addr = ptr(m, at(vis, 0x14)?)?;
    let list = List::read(m, list_addr)?;
    let index = u32::from(stack(m, cpu, 4)? as u16);
    if index as usize >= list.count {
        return Err(Refusal::Layout);
    }
    let handle = m.u32(at(list.column(m, 0xc)?, index * 4)?)?;
    let body = Pool::read(m, ptr(m, OBJECTS)?, 12)?.body(m, handle)?;
    let [px, py, pz] = m.vec3(at(body, 0x30)?)?;
    let radius = m.f32(at(body, 0x3c)?)?;
    let [x, y, z, w] = PROJECTION_DEPTH;
    let [x, y, z, w] = [m.f32(x)?, m.f32(y)?, m.f32(z)?, m.f32(w)?];
    let depth = (((z * pz + y * py) + x * px) + w).abs();
    let min_depth = m.f32(MIN_DEPTH)?;
    let depth = if depth > min_depth { depth } else { min_depth };
    let pixel_size = (((m.f32(PIXEL_SCALE)? / depth) * radius) * m.f32(SIZE_FACTOR)?)
        * m.f32(OBJECT_SIZE_SCALE)?;
    let [cx, cy, cz] = m.vec3(CAMERA)?;
    let (dx, dy, dz) = (
        f64::from(px) - f64::from(cx),
        f64::from(py) - f64::from(cy),
        f64::from(pz) - f64::from(cz),
    );
    let distance =
        (((dz * dz + dy * dy) + dx * dx).sqrt() * f64::from(m.f32(CAMERA_DISTANCE_SCALE)?)) as f32;
    if ![px, py, pz, radius, depth, pixel_size, distance]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }

    let tags = ptr(m, TAGS)?;
    let (visible, detail) = lod_bytes(m, tags, handle, body, distance, radius)?;
    let row_flags = m.u16(at(list.column(m, 8)?, index * 2)?)?;
    let flags = section_flags(m, vis, tags, row_flags)?;
    let mask = if row_flags & 0x800 == 0 {
        m.u16(at(list.column(m, 0x14)?, index * 2)?)?
    } else {
        u16::MAX
    };
    let mode = if mask == u16::MAX { 1 } else { 0x40 };
    let retry = kind == 2 || stack(m, cpu, 8)? as u8 != 0;

    // Same frame as after saving EBX, EBP and EDI. The untouched bytes of
    // the section selector's scratch storage retain their caller values.
    let esp = cpu.esp.checked_sub(0x1f8).ok_or(Refusal::Layout)?;
    let mut out = Output::default();
    out.word(esp, cpu.edi);
    out.word(at(esp, 4)?, cpu.ebp);
    out.word(at(esp, 8)?, cpu.ebx);
    out.byte(at(esp, 0xf)?, u8::from(retry));
    out.byte(at(esp, 0x13)?, detail);
    out.word(at(esp, 0x14)?, handle);

    let start = at(esp, 0x1a8)?;
    let mut input = m.bytes(start, 0x50)?;
    let store = |bytes: &mut [u8], slot: usize, word: u32| {
        bytes[slot - 0x1a8..slot - 0x1a8 + 4].copy_from_slice(&word.to_le_bytes());
    };
    for (slot, value) in [
        (0x1a8, px.to_bits()),
        (0x1ac, py.to_bits()),
        (0x1b0, pz.to_bits()),
        (0x1b4, radius.to_bits()),
        (0x1b8, 0),
        (0x1c0, flags),
        (0x1c4, mode),
        (0x1c8, pixel_size.to_bits()),
        (0x1cc, distance.to_bits()),
        (0x1d4, stack(m, cpu, 16)?),
        (0x1dc, stack(m, cpu, 12)?),
        (0x1e4, NONE),
        (0x1ec, 0),
        (0x1f0, 0),
        (0x1f4, 0),
    ] {
        store(&mut input, slot, value);
    }
    input[0x1bc - 0x1a8] = ((row_flags >> 9) & 1) as u8;
    input[0x1d0 - 0x1a8] = visible;
    input[0x1d2 - 0x1a8..0x1d4 - 0x1a8].copy_from_slice(&mask.to_le_bytes());
    input[0x1e0 - 0x1a8] = 0;
    input[0x1e8 - 0x1a8] = 0;
    out.array(start, input);
    out.frame = Some(SetupFrame {
        esp,
        index,
        list: list_addr,
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_setup_lod_uses_both_fade_ranges() {
        let pick = |distance| fades(distance, 10.0, 50.0, 0.001, 20.0, 10.0, 1.0);
        assert_eq!(pick(15.0), (0.5, 1.0));
        assert_eq!(pick(25.0), (0.0, 1.0));
        assert_eq!(pick(55.0), (0.0, 0.5));
        assert_eq!(pick(60.0), (0.0, 0.0));
    }
}
