//! BSP and model arms of the opaque geometry builder (0x4cbf0). Its output is one
//! private sort slot. The emitter separately checks whether the visible
//! subparts produce a single complete index range.

use guest_layout::memory::GuestMemory;
use native_render::{object_nodes, skip};

const FOG_ACTIVE: u32 = 0x004b_a004;

#[derive(Clone, Copy)]
pub struct Entry {
    pub bytes: [u8; 24],
    pub accepted: bool,
    pub model: bool,
}

fn ptr(m: &dyn GuestMemory, at: u32) -> Option<u32> {
    m.u32(at).ok().filter(|&v| v != 0)
}

fn tag(m: &dyn GuestMemory, table: u32, datum: u32) -> Option<u32> {
    ptr(m, table.checked_add((datum & 0xffff) * 16 + 8)?)
}

/// `old` preserves the slot words the guest leaves unwritten. Layer 5
/// even compares one such word, so zero-initializing it would change order.
pub fn build(m: &dyn GuestMemory, shader: u32, lod: u8, layer: u32, selector: u8,
    argument: u32, mut old: [u8; 24], models: bool) -> Option<Entry> {
    if !matches!(layer, 1 | 3 | 5 | 6 | 7 | 8 | 15 | 16) { return None; }
    let index = (argument >> 8) as u16;
    if index >= 0x352 { return None; }
    let record = skip::RENDER_RECORDS + u32::from(index) * skip::RENDER_RECORD_SIZE;
    let model = m.u32(record + 4).ok()?;
    if !models && model != u32::MAX { return None; }
    let flags = m.u32(record).ok()?;
    let packed = m.u32(record + 12).ok()?;
    let table = ptr(m, guest_layout::tags::TAG_ENTRIES_GLOBAL)?;
    let section_id = (packed >> 9) & 511;
    let (info, geometry) = if model != u32::MAX {
        if m.u32(table.checked_add((model & 0xffff) * 16)?).ok()? != u32::from_be_bytes(*b"mode") {
            return None;
        }
        let mode = tag(m, table, model)?;
        if section_id >= m.u32(mode.checked_add(0x24)?).ok()? { return None; }
        let section = ptr(m, mode.checked_add(0x28)?)?.checked_add(section_id * 0x5c)?;
        (section.checked_add(4)?, ptr(m, section.checked_add(0x34)?)?)
    } else {
        let bsp = ptr(m, super::ACTIVE_BSP)?;
        let section = if flags & (1 << 12) == 0 {
            ptr(m, bsp.checked_add(0xa0)?)?.checked_add(section_id * 0xb0)?
        } else {
            let n = (packed >> 18) & 2047;
            let placements = ptr(m, bsp.checked_add(0x144)?)?;
            let def = m.u16(placements.checked_add(n * 0x58 + 0x34)?).ok()?;
            if def > i16::MAX as u16 { return None; }
            ptr(m, bsp.checked_add(0x13c)?)?.checked_add(u32::from(def) * 0xc8)?
        };
        (section, ptr(m, section.checked_add(0x50)?)?)
    };
    let part = u32::from(argument as u8);
    if part >= m.u32(geometry).ok()? { return None; }
    // Particle materials dispatch through a virtual function. Only shad's
    // direct template accessor is side-effect free.
    if shader as u16 > i16::MAX as u16 ||
        m.u32(table.checked_add((shader & 0xffff)*16)?).ok()? != u32::from_be_bytes(*b"shad") {
        return None;
    }
    let material = tag(m, table, shader)?;
    let instance = ptr(m, material.checked_add(0x24)?)?;
    let template = tag(m, table, m.u32(instance).ok()?)?;
    let header = ptr(m, template.checked_add(0x5c)?)?;
    let lod_entry = ptr(m, header.checked_add(4)?)?.checked_add(u32::from(lod)*10)?;
    let layer_index = u32::from(m.u16(lod_entry).ok()? & 511) + layer;
    let layers = ptr(m, header.checked_add(12)?)?;
    let span = m.u16(layers.checked_add(layer_index*2)?).ok()?;
    let imp = match layer {
        3 => object_nodes::record_implementation(flags) as u8,
        16 if flags & (1 << 7) == 0 => u8::MAX,
        16 => {
            use engine::render_loop::layer::{FOG_A, FOG_B};
            let active = m.u8(FOG_ACTIVE).ok()? != 0 && m.u8(FOG_A).ok()? == 0 && m.u8(FOG_B).ok()? == 0;
            match (flags & (1 << 8) != 0, active) {
                (true, true) => u8::MAX,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 0,
            }
        }
        _ if span >> 9 != 0 => 0,
        _ => u8::MAX,
    };
    let mut accepted = false;
    old[4] = imp;
    if imp != u8::MAX {
        let pass_index = u32::from(span & 511) + u32::from(selector);
        let passes = ptr(m, header.checked_add(20)?)?;
        let pass = tag(m, table, m.u32(passes.checked_add(pass_index * 10 + 4)?).ok()?)?;
        let definitions = ptr(m, pass.checked_add(0x20)?)?;
        let implementation = ptr(m, definitions.checked_add(4)?)?.checked_add(u32::from(imp)*0x132)?;
        let vertex_tag = m.u32(implementation.checked_add(0x100)?).ok()?;
        let class = match m.u16(info.checked_add(0x14)?).ok()? {
            class @ 0..=2 => class,
            3 if model != u32::MAX && vertex_tag != u32::MAX => {
                let address = ptr(m, geometry.checked_add(4)?)?.checked_add(part * 0x48)?;
                let second = matches!(m.u16(address).ok()?, 4 | 5);
                let influences = m.u8(info.checked_add(0x10 + u32::from(second))?).ok()? as i8;
                let count = m.u32(tag(m, table, vertex_tag)?.checked_add(4)?).ok()?;
                let candidate = (i32::from(influences) + 2) as u32;
                if candidate < count { candidate as u16 } else { 0 }
            }
            _ => return None,
        };
        old[5] = if packed >> 29 <= 1 { 1 } else { 2 };
        if packed >> 29 != 0 {
            let sort_id = ((model & 0xffff) << 11) | section_id;
            old[12..16].copy_from_slice(&u32::to_le_bytes(sort_id));
        }
        old[8..10].copy_from_slice(&class.to_le_bytes());
        old[16..20].copy_from_slice(&implementation.to_le_bytes());
        old[20..24].copy_from_slice(&m.u32(record+0x10).ok()?.to_le_bytes());
        accepted = if layer == 5 {
            flags & (1 << 13) != 0
        } else if vertex_tag == u32::MAX {
            false
        } else {
            let vertex = tag(m, table, vertex_tag)?;
            let count = m.u32(vertex.checked_add(4)?).ok()?;
            u32::from(class) < count && {
                let classes = ptr(m, vertex.checked_add(8)?)?;
                m.u32(classes.checked_add(u32::from(class)*0x1c + 12)?).ok()? >> 4 != 0
            }
        };
    }
    Some(Entry { bytes: old, accepted, model: model != u32::MAX })
}

/// Is the actual emitter going to submit this entire part as one range?
/// The masked list variant can coalesce triangle-list slices. Verify the
/// actual subparts tile the original part in order in addition to their bits.
pub fn full_range(m: &dyn GuestMemory, geometry: u32, at: u32, bitmap: u32, strip: bool) -> Option<bool> {
    if bitmap == 0 || at == 0 || geometry == 0 { return None; }
    let count = m.u32(geometry.checked_add(8)?).ok()?;
    let first = u32::from(m.u16(at.checked_add(10)?).ok()?);
    let n = u32::from(m.u16(at.checked_add(12)?).ok()?);
    if n == 0 || first+n > count || first+n > i16::MAX as u32 { return Some(false); }
    // The strip path submits separately for every subpart.
    if strip && n != 1 { return Some(false); }
    let parts = ptr(m, geometry.checked_add(12)?)?;
    let start = u32::from(m.u16(at.checked_add(6)?).ok()?);
    let end = start + u32::from(m.u16(at.checked_add(8)?).ok()?);
    let mut cursor = start;
    for index in first..first+n {
        let mask = m.u32(bitmap.checked_add(index/32*4)?).ok()?;
        let sub = parts.checked_add(index*8)?;
        if mask & (1 << (index%32)) == 0 || u32::from(m.u16(sub).ok()?) != cursor { return Some(false); }
        let length = u32::from(m.u16(sub.checked_add(2)?).ok()?);
        if length == 0 { return Some(false); }
        cursor += length;
    }
    Some(cursor == end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn full_range_requires_live_mask_and_ordered_contiguous_indices() {
        let mut m = FlatMemory::new(0x1000, 0x1000);
        let (geometry, part, subs, bitmap) = (0x1000, 0x1100, 0x1200, 0x1500);
        m.set_u32(geometry+8, 35).unwrap();
        m.set_u32(geometry+12, subs).unwrap();
        for (at, value) in [(part+6, 9), (part+8, 6), (part+10, 31), (part+12, 2),
            (subs+31*8, 9), (subs+31*8+2, 3), (subs+32*8, 12), (subs+32*8+2, 3)] {
            m.set_u16(at, value).unwrap();
        }
        m.set_u32(bitmap, 1<<31).unwrap();
        assert_eq!(full_range(&m, geometry, part, bitmap, false), Some(false));
        m.set_u32(bitmap+4, 1).unwrap();
        assert_eq!(full_range(&m, geometry, part, bitmap, false), Some(true));
        assert_eq!(full_range(&m, geometry, part, bitmap, true), Some(false));
        m.set_u16(subs+32*8, 11).unwrap();
        assert_eq!(full_range(&m, geometry, part, bitmap, false), Some(false));
        m.set_u16(part+12, 1).unwrap();
        m.set_u16(part+8, 3).unwrap();
        assert_eq!(full_range(&m, geometry, part, bitmap, true), Some(true));
    }
}
