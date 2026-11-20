//! The main BSP arm of the scene material walk at 0x44c04. Plan a whole
//! record before appending any items. A transparent producer, material
//! override or unrecognised shader sends the record back to its guest loop.

use engine::memory::GuestMemory;
use native_render::skip;

const SCENE_LIST: u32 = 0x004c_1a48;
const SCREEN_SCALE: u32 = 0x0046_70e4;
const ALPHA_SCALE: u32 = 0x0045_dc10;
const DOUBLE_LOD: u32 = 0x004b_a019;
const LOD_SCALES: u32 = 0x0048_5b28;
const MATERIAL_VIS: u32 = 0x004e_d280;
const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
const PART_PLANES: u32 = 0x0051_f400;
const PLANE_HASH: u32 = 0x0001_e8c0;
const PLANE_EQUAL: u32 = 0x0001_e8f0;
pub(super) const FIRST_PART_RETURN: u32 = 0x0004_4c15;

fn material_visible(m: &dyn GuestMemory, material: u32, placement: u32) -> Option<bool> {
    let bit = u32::from(m.u8(material.checked_add(0x1c)?).ok()?);
    if bit == 255 { return Some(true); }
    let bsp = u32::from(m.u16(ACTIVE_BSP_INDEX).ok()?);
    if bsp > i16::MAX as u32 { return None; }
    let masks = m.u32(MATERIAL_VIS).ok()?;
    let offset = if placement == 0x7ff { bsp * 32 + 1 } else { bsp * 1024 + placement + 0x201 };
    let word = m.u32(masks.checked_add(offset + bit/32*4)?).ok()?;
    Some(word & (1 << (bit%32)) != 0)
}

fn part_visible(m: &dyn GuestMemory, part: u32, mask: u32, masked: bool) -> Option<bool> {
    if !masked { return Some(true); }
    let first = u32::from(m.u16(part.checked_add(10)?).ok()?);
    let count = u32::from(m.u16(part.checked_add(12)?).ok()?);
    if first + count > i16::MAX as u32 || mask == 0 { return None; }
    for i in first..first+count {
        if m.u32(mask.checked_add(i/32*4)?).ok()? & (1 << (i%32)) != 0 { return Some(true); }
    }
    Some(false)
}

fn lod(m: &dyn GuestMemory, shader: u32, pixels: f32) -> Option<(u8, u32, u32)> {
    let table = m.u32(engine::tags::TAG_ENTRIES_GLOBAL).ok()?;
    let entry = table.checked_add((shader & 0xffff)*16)?;
    if shader as u16 > i16::MAX as u16 || m.u32(entry).ok()? != u32::from_be_bytes(*b"shad") { return None; }
    let tag = m.u32(entry.checked_add(8)?).ok()?;
    let classification = u32::from(m.u16(tag.checked_add(0x3c)?).ok()?);
    if classification > 5 { return None; }
    let instance = m.u32(tag.checked_add(0x24)?).ok()?;
    let template_id = m.u32(instance).ok()?;
    let template = m.u32(table.checked_add((template_id & 0xffff)*16 + 8)?).ok()?;
    let header = m.u32(template.checked_add(0x5c)?).ok()?;
    let count = m.u32(header).ok()?;
    if header == 0 || !(1..=256).contains(&count) { return None; }
    let levels = m.u32(header.checked_add(4)?).ok()?;
    let mut level = 0;
    if classification != 5 {
        let mut pixels = pixels;
        if m.u8(DOUBLE_LOD).ok()? != 0 { pixels *= m.f32(engine::light_constants::constant::TWO).ok()?; }
        pixels *= m.f32(LOD_SCALES + classification*4).ok()?;
        if !pixels.is_finite() { return None; }
        while level + 1 < count {
            let threshold = m.f32(levels.checked_add(level*10+6)?).ok()?;
            if !threshold.is_finite() { return None; }
            if pixels >= threshold { break; }
            level += 1;
        }
    }
    Some((level as u8, m.u32(levels.checked_add(level*10+2)?).ok()?, m.u32(levels.checked_add(2)?).ok()?))
}

pub(super) fn append(m: &mut dyn GuestMemory, sp: u32, index: u32) -> Option<u32> {
    if index >= 0x352 || m.u32(sp).ok()? != FIRST_PART_RETURN ||
        m.u32(sp.checked_add(4)?).ok()? != 0 || m.u32(sp.checked_add(0x60)?).ok()? != 0 ||
        m.u32(sp.checked_add(0x68)?).ok()? != 0x1f ||
        m.u32(sp.checked_add(0x1c)?).ok()? != u32::MAX { return None; }
    let record = skip::RENDER_RECORDS + index*skip::RENDER_RECORD_SIZE;
    if m.u32(record+4).ok()? != u32::MAX { return None; }
    let planes = m.u32(PART_PLANES).ok()?;
    if planes == 0 { return None; }
    // In opaque entries the computed clipping plane and centre are private
    // scratch. Only bypass the two known pure callbacks of the plane lookup.
    if m.u32(planes.checked_add(0x2c)?).ok()? != PLANE_HASH ||
        m.u32(planes.checked_add(0x30)?).ok()? != PLANE_EQUAL { return None; }
    let flags = m.u32(record).ok()?;
    let mask = m.u32(record+20).ok()?;
    let placement = m.u32(record+12).ok()? >> 18 & 0x7ff;
    let geometry = m.u32(sp.checked_add(0x28)?).ok()?;
    let count = m.u32(geometry).ok()?;
    if geometry == 0 || !(1..=256).contains(&count) { return None; }
    let parts = m.u32(geometry.checked_add(4)?).ok()?;
    let bsp = m.u32(super::ACTIVE_BSP).ok()?;
    let materials = m.u32(bsp.checked_add(0xa8)?).ok()?;
    let material_count = m.u32(bsp.checked_add(0xa4)?).ok()?;
    let pixels = m.u8(sp.checked_add(0x24)?).ok()?;
    let scale = m.f32(SCREEN_SCALE).ok()?;
    let alpha_scale = m.f32(ALPHA_SCALE).ok()?;
    let first = m.u32(SCENE_LIST+4).ok()?;
    let capacity = m.u32(SCENE_LIST).ok()?;
    if first > capacity || capacity > skip::MAX_ITEMS { return None; }
    let dst = m.u32(SCENE_LIST+20).ok()?.checked_add(first * skip::ITEM_SIZE)?;
    let mut all_layers = m.u32(SCENE_LIST+8).ok()?;
    let mut outputs = Vec::new();
    for n in 0..count {
        let part = parts.checked_add(n*0x48)?;
        let material_no = u32::from(m.u16(part.checked_add(4)?).ok()?);
        if material_no >= material_count || material_no > i16::MAX as u32 { return None; }
        let mat = materials.checked_add(material_no*32)?;
        if !material_visible(m, mat, placement)? || !part_visible(m, part, mask, flags & 64 != 0)? { continue; }
        let shader = m.u32(mat.checked_add(12)?).ok()?;
        let kind = m.u16(part).ok()?;
        if shader == u32::MAX || matches!(kind, 0 | 5) { continue; }
        let size = m.f32(part.checked_add(0x2c)?).ok()?;
        let size = if size > 0.0 { size } else { m.f32(engine::light_constants::constant::ONE).ok()? };
        let (level, layers, base_layers) = lod(m, shader, ((f32::from(pixels)*size)*scale)*alpha_scale)?;
        // The standard scene caller passes the same shader as its override.
        if layers & 0x8080 != 0 && layers & 0xa == 0 && base_layers & (1 << 21) != 0 { continue; }
        // The transparent producer allocates a second entry and publishes its
        // plane and position. Leave the complete section to that producer.
        if layers & (1<<15) != 0 { return None; }
        if layers == 0 || first + (outputs.len() / 24) as u32 == capacity { continue; }
        let mut item = [0; 24];
        let item_at = dst.checked_add(outputs.len() as u32)?;
        m.read(item_at, &mut item).ok()?;
        item[0..4].copy_from_slice(&skip::GEOMETRY_BUILD.to_le_bytes());
        item[4..8].copy_from_slice(&skip::GEOMETRY_DRAW.to_le_bytes());
        item[8..12].copy_from_slice(&shader.to_le_bytes());
        let argument = u32::from(pixels)<<24 | index<<8 | n;
        item[12..16].copy_from_slice(&argument.to_le_bytes());
        item[16..20].copy_from_slice(&layers.to_le_bytes());
        item[20] = 0;
        item[21] = level;
        all_layers |= layers;
        outputs.extend_from_slice(&item);
    }
    if !outputs.is_empty() {
        let mut old_header = [0; 8];
        m.read(SCENE_LIST+4, &mut old_header).ok()?;
        let mut header = [0; 8];
        header[..4].copy_from_slice(&(first+(outputs.len()/24) as u32).to_le_bytes());
        header[4..].copy_from_slice(&all_layers.to_le_bytes());
        m.write(SCENE_LIST+4, &header).ok()?;
        if m.write(dst, &outputs).is_err() {
            m.write(SCENE_LIST+4, &old_header).expect("restore scene list on refusal");
            return None;
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    const SP: u32 = 0x520000;
    const BSP: u32 = SP+0x1000;
    const ITEMS: u32 = SP+0x2000;
    const GEOM: u32 = BSP+0x400;
    const PARTS: u32 = GEOM+0x100;
    const MAT: u32 = BSP+0x800;
    const HEADER: u32 = SP+0x3000;
    const TABLE: u32 = HEADER+0x400;

    fn fixture() -> FlatMemory {
        let mut m = FlatMemory::new(0x400000, 0x140000);
        for (at, v) in [
            (SP, FIRST_PART_RETURN), (SP+0x68, 0x1f), (SP+0x1c, u32::MAX),
            (SP+0x28, GEOM), (skip::RENDER_RECORDS+4, u32::MAX),
            (skip::RENDER_RECORDS+12, 0x7ff << 18), (GEOM, 2), (GEOM+4, PARTS),
            (super::super::ACTIVE_BSP, BSP), (BSP+0xa4, 2), (BSP+0xa8, MAT),
            (MAT+12, 1), (MAT+32+12, 1), (SCENE_LIST, 8), (SCENE_LIST+20, ITEMS),
            (PART_PLANES, BSP+0xb00), (BSP+0xb2c, PLANE_HASH), (BSP+0xb30, PLANE_EQUAL),
            (engine::tags::TAG_ENTRIES_GLOBAL, TABLE), (TABLE+16, u32::from_be_bytes(*b"shad")),
            (TABLE+24, HEADER+0x100), (HEADER+0x124, HEADER+0x180), (HEADER+0x180, 2),
            (TABLE+40, HEADER+0x200), (HEADER+0x25c, HEADER), (HEADER, 1),
            (HEADER+4, HEADER+0x40), (HEADER+0x42, (1<<3)|(1<<16)),
        ] { m.set_u32(at, v).unwrap(); }
        m.set_u8(MAT+28, 255).unwrap();
        m.set_u8(MAT+32+28, 255).unwrap();
        m.set_u16(PARTS, 2).unwrap();
        m.set_u16(PARTS+0x48, 2).unwrap();
        m.set_u16(PARTS+0x48+4, 1).unwrap();
        m
    }

    #[test]
    fn all_opaque_parts_publish_in_order_including_the_other_layers_bits() {
        let mut m = fixture();
        m.set_u8(SP+0x24, 19).unwrap();
        m.set_u16(ITEMS+22, 0xbeef).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 2);
        assert_eq!(m.u32(SCENE_LIST+8).unwrap(), (1<<3)|(1<<16));
        assert_eq!(m.u32(ITEMS+12).unwrap(), 19<<24);
        assert_eq!(m.u32(ITEMS+24+12).unwrap(), (19<<24)|1);
        assert_eq!(m.u16(ITEMS+22).unwrap(), 0xbeef);
    }

    #[test]
    fn unknown_last_material_or_transparent_pass_leaves_the_whole_section_for_the_guest() {
        let mut m = fixture();
        m.set_u32(MAT+32+12, 7).unwrap();
        let before = m.bytes.clone();
        assert_eq!(append(&mut m, SP, 0), None);
        assert_eq!(m.bytes, before);
        m.set_u32(MAT+32+12, 1).unwrap();
        m.set_u32(HEADER+0x42, 1<<15).unwrap();
        assert_eq!(append(&mut m, SP, 0), None);
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 0);
    }

    #[test]
    fn scene_items_use_the_selected_lod_and_leave_a_full_list_alone() {
        let mut m = fixture();
        m.set_u32(HEADER, 2).unwrap();
        m.set_f32(HEADER+0x46, 10.0).unwrap();
        m.set_u32(HEADER+0x4c, 1<<7).unwrap();
        m.set_u32(SCENE_LIST, 1).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 1);
        assert_eq!(m.u32(SCENE_LIST+8).unwrap(), 1<<7);
        assert_eq!(m.u8(ITEMS+21).unwrap(), 1);
        let before = m.bytes.clone();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.bytes, before);
    }

    #[test]
    fn hidden_subparts_do_not_need_a_native_material_or_an_item() {
        let mut m = fixture();
        m.set_u32(skip::RENDER_RECORDS, 64).unwrap();
        m.set_u32(skip::RENDER_RECORDS+20, BSP+0x40).unwrap();
        m.set_u32(BSP+0x40, 1).unwrap();
        m.set_u16(PARTS+12, 1).unwrap();
        m.set_u16(PARTS+0x48+10, 1).unwrap();
        m.set_u16(PARTS+0x48+12, 1).unwrap();
        m.set_u32(MAT+32+12, 7).unwrap();
        assert_eq!(append(&mut m, SP, 0), Some(2));
        assert_eq!(m.u32(SCENE_LIST+4).unwrap(), 1);
    }
}
