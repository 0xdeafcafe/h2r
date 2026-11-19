//! The gathered geometry at the entry to one unshadowed light's passes.
//! The list may retain a prefix from the preceding light. Both the list's
//! OR word and the item's own mask gate a layer. Read the complete active
//! interval before returning any part to the host.

use crate::camera::GuestRead;
use crate::skip::{self, GEOMETRY_DRAW, ITEM_SIZE};
use crate::world::Owner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub part: (Owner, u8),
    pub shader: u32,
    pub layers: u32,
}

/// Gated by the light's flags (engine::light_passes) and the list's OR.
/// Layer 14 is conservatively refused until its setup is accounted for.
pub fn read(
    mem: &dyn GuestRead,
    list: u32,
    flags: u32,
    definition_flags: u32,
    split: bool,
) -> Result<Vec<Item>, &'static str> {
    const DIFFUSE: u32 = 1 << 12;
    const SPECULAR: u32 = 1 << 13;
    let unread = "unreadable gathered light list";
    let word = |offset| mem.read_u32(list.checked_add(offset)?);
    let layers = word(8).ok_or(unread)?;
    if flags & 4 != 0 {
        return Err("shadowed light");
    }
    if layers & (1 << 11) != 0 && (flags & 2 != 0 || definition_flags & (1 << 5) != 0) {
        return Err("light needs layer 11");
    }
    if layers & (1 << 14) != 0 {
        return Err("light needs layer 14");
    }
    let layers = layers & (DIFFUSE | if flags & 3 == 3 { SPECULAR } else { 0 });
    if layers == 0 {
        return Ok(Vec::new());
    }
    let capacity = word(0).ok_or(unread)?;
    let count = word(4).ok_or(unread)?;
    let first = word(0xc).ok_or(unread)?;
    let items = word(0x14).ok_or(unread)?;
    if first > count || count > capacity || count - first > 1 << 16 {
        return Err("invalid gathered light list bounds");
    }
    let last_byte = || count.checked_mul(ITEM_SIZE)?.checked_sub(1)?.checked_add(items);
    if count > first && (items == 0 || last_byte().is_none()) {
        return Err("invalid gathered light item address");
    }

    let mut out = Vec::new();
    for index in first..count {
        let item = items + index * ITEM_SIZE;
        let mask = mem.read_u32(item + 0x10).ok_or(unread)? & layers;
        if mask == 0 {
            continue;
        }
        if !skip::is_structure_item(mem, item).ok_or(unread)? {
            // The split path only masks BSP items built by GEOMETRY_BUILD.
            if split {
                continue;
            }
            return Err("gathered model or unsupported geometry handler");
        }
        if mem.read_u32(item + 4).ok_or(unread)? != GEOMETRY_DRAW {
            return Err("unsupported structure draw");
        }
        if (mem.read_u32(item + 0x14).ok_or(unread)? >> 8) as u8 != 0 {
            return Err("light item requires another material variant");
        }
        let part = skip::structure_part(mem, item).ok_or(unread)?;
        out.push(Item {
            part,
            shader: mem.read_u32(item + 8).ok_or(unread)?,
            layers: mask,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip::GEOMETRY_BUILD;
    use crate::camera::tests::Mem;

    const LIST: u32 = 0x1000;
    const ITEMS: u32 = 0x2000;
    const BOTH: u32 = 3 << 12;

    fn list() -> Mem {
        let mut m = Mem::default();
        for (offset, value) in [(0, 7), (4, 4), (8, BOTH), (0xc, 1), (0x14, ITEMS)] {
            m.w(LIST + offset, value);
        }
        // Entry 0 is an unreadable prefix. Entry 2 is a different layer.
        m.w(ITEMS + 2 * ITEM_SIZE + 0x10, 1 << 3);
        for (index, layers) in [(1, BOTH), (3, 1 << 13)] {
            let item = ITEMS + index * ITEM_SIZE;
            for (offset, value) in [(0, GEOMETRY_BUILD), (4, GEOMETRY_DRAW), (8, 135), (0xc, index << 8 | 2),
                (0x10, layers), (0x14, 0)] {
                m.w(item + offset, value);
            }
            let rec = skip::RENDER_RECORDS + index * skip::RENDER_RECORD_SIZE;
            m.w(rec, if index == 1 { 0 } else { 1 << 12 });
            m.w(rec + 4, u32::MAX);
            m.w(rec + 0xc, 6 << 18 | 3 << 9);
        }
        m
    }

    #[test]
    fn gathered_parts_follow_the_active_interval_and_both_layer_gates() {
        let mut m = list();
        let cluster = Item { part: (Owner::Cluster(3), 2), shader: 135, layers: BOTH };
        let placement = Item { part: (Owner::Instance(6), 2), layers: 1 << 13, ..cluster };
        assert_eq!(read(&m, LIST, 3, 0, false), Ok(vec![cluster, placement]));
        let diffuse = vec![Item { layers: 1 << 12, ..cluster }];
        assert_eq!(read(&m, LIST, 2, 0, false).unwrap(), diffuse);
        m.w(LIST + 8, 1 << 12);
        assert_eq!(read(&m, LIST, 3, 0, false).unwrap(), diffuse);
    }

    #[test]
    fn a_late_bad_item_refuses_the_whole_light() {
        let last = ITEMS + 3 * ITEM_SIZE;
        for (address, value) in [
            (last, 8), (last + 4, 8), (last + 0x14, 1 << 8),
            (skip::RENDER_RECORDS + 3 * skip::RENDER_RECORD_SIZE + 4, 42),
            (LIST + 0xc, 5), (LIST + 4, 8), (LIST + 0x14, u32::MAX - 15),
        ] {
            let mut m = list();
            m.w(address, value);
            assert!(read(&m, LIST, 3, 0, false).is_err(), "address {address:#x}");
        }
        let mut m = list();
        m.w(skip::RENDER_RECORDS + 3 * skip::RENDER_RECORD_SIZE + 4, 42);
        assert_eq!(read(&m, LIST, 3, 0, true).unwrap().len(), 1, "split keeps model entries on D3D");
        let mut m = list();
        m.0.remove(&(last + 8));
        assert!(read(&m, LIST, 3, 0, false).is_err());
    }

    #[test]
    fn unsupported_layers_only_count_when_the_light_runs_them() {
        let mut m = list();
        m.w(LIST + 8, (1 << 11) | BOTH);
        assert!(read(&m, LIST, 0, 0, false).is_ok());
        assert!(read(&m, LIST, 2, 0, false).is_err());
        assert!(read(&m, LIST, 0, 1 << 5, false).is_err());
        m.w(LIST + 8, (1 << 14) | BOTH);
        assert!(read(&m, LIST, 0, 0, false).is_err());
        m.w(LIST + 8, BOTH);
        assert!(read(&m, LIST, 4, 0, false).is_err());
    }
}
