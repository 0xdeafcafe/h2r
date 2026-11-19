//! Shadow layer admission. The descriptor at 0x0043fa1c chooses list 2 for
//! generation; the caller supplies list 1 for application. Their prefixes,
//! masks and item identities are independent.

use crate::camera::GuestRead;
use crate::skip::{self, GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE};
use crate::world::Owner;

pub const CASTERS: u32 = 0x004c_1a78;
pub const RECEIVERS: u32 = 0x004c_1a60;
pub const CURRENT_TARGET: u32 = 0x0048_58b4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Light,
    AdditionalCaster,
    Object,
}

/// The layer draw's return address. 0x00030ed0 runs another generate/apply
/// pair on a temporary list suffix before the light continues to layer 14.
pub fn site(layer: u32, ret: u32) -> Option<Site> {
    match (layer, ret) {
        (10, 0x0003_12b2) | (11, 0x0003_134d) | (14, 0x0003_13e2) => Some(Site::Light),
        (10, 0x0003_1074) | (11, 0x0003_10f1) => Some(Site::AdditionalCaster),
        (10, 0x0003_295c) | (11, 0x0003_29d9) => Some(Site::Object),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Bsp(Owner),
    Model { datum: u32, record: u16 },
    Other { build: u32, draw: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub index: u32,
    pub shader: u32,
    pub argument: u32,
    pub variant: u8,
    pub geometry: Geometry,
    pub marked: bool,
}

/// Candidates in list order. A candidate still has to pass the build
/// callback, sort, and the entry loop's section-residency check.
pub fn items(mem: &dyn GuestRead, list: u32, layer: u32) -> Option<Vec<Item>> {
    let bit = 1u32.checked_shl(layer)?;
    let word = |offset| mem.read_u32(list.checked_add(offset)?);
    let count = word(4)?;
    let first = word(12)?;
    if first > count || count > word(0)? || count - first > 2560 {
        return None;
    }
    if word(8)? & bit == 0 || count == first {
        return Some(Vec::new());
    }
    let base = word(20)?;
    if base == 0 {
        return None;
    }
    base.checked_add(count.checked_mul(ITEM_SIZE)?.checked_sub(1)?)?;
    let mut out = Vec::new();
    for index in first..count {
        let at = base + index * ITEM_SIZE;
        if mem.read_u32(at + 16)? & bit == 0 {
            continue;
        }
        let argument = mem.read_u32(at + 12)?;
        let build = mem.read_u32(at)?;
        let draw = mem.read_u32(at + 4)?;
        let (geometry, marked) = if (build, draw) == (GEOMETRY_BUILD, GEOMETRY_DRAW) {
            let record = (argument >> 8) as u16;
            if (record as i16) < 0 {
                return None;
            }
            let addr = skip::RENDER_RECORDS + u32::from(record) * skip::RENDER_RECORD_SIZE;
            let marked = mem.read_u32(addr)? & (1 << 10) != 0;
            let datum = mem.read_u32(addr + 4)?;
            let owner = if datum == u32::MAX {
                Geometry::Bsp(skip::structure_part(mem, at)?.0)
            } else {
                Geometry::Model { datum, record }
            };
            (owner, marked)
        } else {
            (Geometry::Other { build, draw }, false)
        };
        out.push(Item {
            index,
            shader: mem.read_u32(at + 8)?,
            argument,
            variant: (mem.read_u32(at + 20)? >> 8) as u8,
            geometry,
            marked,
        });
    }
    Some(out)
}

/// 0x00044408 and 0x00044478 select the attachment and its clear.
pub fn colour_mask(flags: u32) -> bool {
    flags & (1 << 4 | 1 << 5) == 1 << 4
}

/// Geometry chooser 0x00030cd0, after the template has a layer-10 pass.
pub fn generation(flags: u32, object: bool, record_bit_10: bool) -> Option<usize> {
    match (record_bit_10, object, colour_mask(flags)) {
        (true, _, _) => None,
        (false, false, _) => Some(2),
        (false, true, true) => Some(0),
        (false, true, false) => Some(1),
    }
}

/// 0x00030d10 for the geometry builder's arguments (int input 4, true,
/// record bit 10). None means this material builds no receiver entry.
pub fn application(
    flags: u32,
    definition_bit_5: bool,
    int4: bool,
    record_bit_10: bool,
) -> Option<usize> {
    if flags & 1 != 0 {
        return None;
    }
    if definition_bit_5 {
        return Some(2);
    }
    if flags & 4 == 0 {
        return (flags & 2 != 0 && int4).then_some(2);
    }
    if record_bit_10 {
        Some(2)
    } else if flags & (1 << 6) != 0 {
        Some(6)
    } else if flags & (1 << 4) != 0 {
        Some(if colour_mask(flags) { 4 } else { 3 })
    } else {
        Some(((flags >> 3) & 1) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    #[test]
    fn geometry_selectors_distinguish_colour_masks_and_depth_maps() {
        assert_eq!(generation(0x14, true, false), Some(0));
        assert_eq!(generation(0x34, true, false), Some(1));
        assert_eq!(generation(0x14, false, false), Some(2));
        assert_eq!(generation(0x24, true, true), None);
        for (flags, want) in [(4, 0), (12, 1), (0x14, 4), (0x34, 3), (0x44, 6)] {
            assert_eq!(application(flags, false, false, false), Some(want));
            assert_eq!(application(flags, false, false, true), Some(2));
            assert_eq!(application(flags | 1, true, true, true), None);
        }
        assert_eq!(application(0, true, false, false), Some(2));
        assert_eq!(application(2, false, true, false), Some(2));
        assert_eq!(application(2, false, false, false), None);
    }

    #[test]
    fn incomplete_suffix_never_returns_partial_casters() {
        let mut mem = Mem::default();
        let list = 0x1000;
        let base = 0x2000;
        for (offset, value) in [(0, 4), (4, 4), (8, 1 << 10), (12, 1), (20, base)] {
            mem.w(list + offset, value);
        }
        for index in [1, 3] {
            let at = base + index * ITEM_SIZE;
            for (offset, value) in [(0, 1), (4, 2), (8, 3), (12, 5), (16, 1 << 10), (20, 2 << 8)] {
                mem.w(at + offset, value);
            }
        }
        mem.w(base + 2 * ITEM_SIZE + 16, 1 << 11);
        let got = items(&mem, list, 10).unwrap();
        assert_eq!(
            got.iter().map(|i| (i.index, i.variant)).collect::<Vec<_>>(),
            [(1, 2), (3, 2)]
        );
        mem.0.remove(&(base + 3 * ITEM_SIZE + 8));
        assert_eq!(items(&mem, list, 10), None);
        mem.w(list + 20, u32::MAX - 8);
        assert_eq!(items(&mem, list, 10), None);
    }
}
