//! Which of the game's layer entries the native world replaces, and the
//! skip: clear their layer bit before the layer draw, restore it after
//! (docs/graphics/native-renderer.md, "Skip only the replaced work").
//!
//! Research points (Code):
//!
//! - The layer draw `0x00044550` reads the current layer at `0x00467130`
//!   and the current list at `0x00467138`.
//! - A list: `+0` capacity, `+4` count, `+8` the OR of the item masks,
//!   `+0xc` the first item, `+0x14` the items, `0x18` bytes each (appended
//!   by `0x0003b9d0`).
//! - An item: `+0` build, `+4` draw, `+8` shader, `+0xc` argument (render
//!   record index `<< 8`, read back with `sar 8` and `movsx` of the low
//!   word at `0x0004cc02`), `+0x10` layer mask.
//! - Structure and model geometry build with `0x0004cbf0`; its render
//!   record (`0x004ba138 + 0x20 · index`) has `+4` an object's render
//!   model tag datum, `-1` for BSP clusters and instanced geometry (not the
//!   object handle: [`crate::objects::record_owner`] finds that).
//! - Which BSP geometry a structure item draws (the build `0x0004cbf0`,
//!   `0x0004cec1`..`0x0004cf21`): the record's `+0` bit 12 clear, a cluster
//!   (`+0xc` bits 9..17, the structure BSP's clusters at `+0xa0`, `0xb0`
//!   each); set, an instanced-geometry placement (`+0xc` bits 18..28, the
//!   placements at `+0x144`, `0x58` each); then the argument's low byte
//!   is the index into that section's parts (`[section+4] + 0x48·part`).

use crate::camera::{GuestRead, GuestWrite};
use crate::world::Owner;

pub const LAYER_DRAW: u32 = 0x0004_4550;
pub const CURRENT_LAYER: u32 = 0x0046_7130;
pub const CURRENT_LIST: u32 = 0x0046_7138;
pub const RENDER_RECORDS: u32 = 0x004b_a138;
pub const RENDER_RECORD_SIZE: u32 = 0x20;
pub const GEOMETRY_BUILD: u32 = 0x0004_cbf0;
pub const ITEM_SIZE: u32 = 0x18;
/// The lightmap layer the native world replaces in stage 1.
pub const LIGHTMAP_LAYER: u32 = 3;
/// Items walked at most (the list's capacity is far below).
const MAX_ITEMS: u32 = 1 << 16;

/// Items whose layer bit was cleared, to set back after the layer draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub layer: u32,
    /// Item addresses.
    pub items: Vec<u32>,
}

/// Whether the item at `item` is BSP geometry (cluster or instance).
pub fn is_structure_item(mem: &dyn GuestRead, item: u32) -> Option<bool> {
    if mem.read_u32(item)? != GEOMETRY_BUILD {
        return Some(false);
    }
    let arg = mem.read_u32(item + 0xc)? as i32;
    let record = (arg >> 8) as u16 as i16;
    if record < 0 {
        return Some(false);
    }
    let model = mem.read_u32(RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32 + 4)?;
    Some(model == u32::MAX)
}

/// The BSP geometry a structure item ([`is_structure_item`]) draws: its
/// cluster or placement and the part index in that section, as the build
/// reads them.
pub fn structure_part(mem: &dyn GuestRead, item: u32) -> Option<(Owner, u8)> {
    let arg = mem.read_u32(item + 0xc)?;
    let record = (arg >> 8) as u16 as i16;
    if record < 0 {
        return None;
    }
    let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
    let packed = mem.read_u32(base + 0xc)?;
    let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
        Owner::Cluster((packed >> 9) & 0x1ff)
    } else {
        Owner::Instance((packed >> 18) & 0x7ff)
    };
    Some((owner, arg as u8))
}

/// Clears `layer`'s bit on every BSP item of the current list whose part
/// (owner and part index, [`structure_part`]) and shader (the item's
/// `+8`) `native` accepts, when the current layer is `layer`: the items of
/// parts the native pass does not draw keep their D3D draws, whatever
/// their shader. `None` and the rest as [`clear_structure`].
pub fn clear_parts(mem: &mut dyn GuestWrite, layer: u32, native: &dyn Fn(Owner, u8, u32) -> bool) -> Option<Cleared> {
    clear_items(mem, layer, &|mem, item, shader| {
        Some(structure_part(mem, item).is_some_and(|(owner, part)| native(owner, part, shader)))
    })
}

/// Clears `layer`'s bit on every BSP item of the current list whose
/// shader (the item's `+8`) `native` accepts, when the current layer is
/// `layer`: the per-material fallback keeps the rest on the D3D path.
/// `None` (nothing written) when it is another layer or the list cannot
/// be read; an unreadable item stops the walk with what was cleared so
/// far.
pub fn clear_structure(mem: &mut dyn GuestWrite, layer: u32, native: &dyn Fn(u32) -> bool) -> Option<Cleared> {
    clear_items(mem, layer, &|_, _, shader| Some(native(shader)))
}

/// The walk of [`clear_structure`] and [`clear_parts`]: `accept` sees a
/// structure item and its shader (`None` stops the walk).
fn clear_items(
    mem: &mut dyn GuestWrite,
    layer: u32,
    accept: &dyn Fn(&dyn GuestRead, u32, u32) -> Option<bool>,
) -> Option<Cleared> {
    if mem.read_u32(CURRENT_LAYER)? != layer || layer >= 32 {
        return None;
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return None;
    }
    let count = mem.read_u32(list + 4)?;
    let first = mem.read_u32(list + 0xc)?;
    let items = mem.read_u32(list + 0x14)?;
    let bit = 1u32 << layer;
    let mut out = Cleared {
        layer,
        items: Vec::new(),
    };
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        let Some(mask) = mem.read_u32(item + 0x10) else { break };
        if mask & bit == 0 {
            continue;
        }
        match is_structure_item(mem, item) {
            Some(true) => match mem.read_u32(item + 8).and_then(|shader| accept(&*mem, item, shader)) {
                Some(true) => {}
                Some(false) => continue,
                None => break,
            },
            Some(false) => continue,
            None => break,
        }
        if mem.write_u32(item + 0x10, mask & !bit).is_none() {
            break;
        }
        out.items.push(item);
    }
    Some(out)
}

/// Sets the bits [`clear_structure`] cleared back.
pub fn restore(mem: &mut dyn GuestWrite, cleared: &Cleared) {
    let bit = 1u32 << cleared.layer;
    for &item in &cleared.items {
        if let Some(mask) = mem.read_u32(item + 0x10) {
            let _ = mem.write_u32(item + 0x10, mask | bit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;

    /// A list of four items: a BSP cluster (layers 1, 3), an object
    /// (layer 3), a BSP instance without layer 3, and another kind of item
    /// in layer 3; render records 2 (BSP), 5 (object), 7 (BSP).
    fn scene() -> Mem {
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, 3);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST, 100);
        m.w(LIST + 4, 4);
        m.w(LIST + 8, 0b1010);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        let item = |m: &mut Mem, i: u32, build: u32, record: u32, mask: u32| {
            let a = ITEMS + ITEM_SIZE * i;
            m.w(a, build);
            m.w(a + 4, 0x0004_d0b0);
            m.w(a + 8, 0xe000_0010 + i);
            m.w(a + 0xc, record << 8 | 1);
            m.w(a + 0x10, mask);
        };
        item(&mut m, 0, GEOMETRY_BUILD, 2, 0b1010);
        item(&mut m, 1, GEOMETRY_BUILD, 5, 0b1000);
        item(&mut m, 2, GEOMETRY_BUILD, 7, 0b0010);
        item(&mut m, 3, 0x0005_0000, 2, 0b1000);
        m.w(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX);
        m.w(RENDER_RECORDS + 0x20 * 5 + 4, 0xe1a3_0004);
        m.w(RENDER_RECORDS + 0x20 * 7 + 4, u32::MAX);
        m
    }

    fn mask(m: &Mem, i: u32) -> u32 {
        m.read_u32(ITEMS + ITEM_SIZE * i + 0x10).unwrap()
    }

    #[test]
    fn only_bsp_items_in_the_layer_lose_its_bit_and_get_it_back() {
        let mut m = scene();
        let before: Vec<u32> = (0..4).map(|i| mask(&m, i)).collect();
        let c = clear_structure(&mut m, LIGHTMAP_LAYER, &|_| true).unwrap();
        assert_eq!(c.items, vec![ITEMS]);
        assert_eq!(mask(&m, 0), 0b0010, "BSP: layer 1 kept, layer 3 gone");
        assert_eq!(mask(&m, 1), 0b1000, "object kept");
        assert_eq!(mask(&m, 2), 0b0010, "not in layer 3");
        assert_eq!(mask(&m, 3), 0b1000, "another build function");
        assert_eq!(m.read_u32(LIST + 8), Some(0b1010), "the list's OR is not touched");
        restore(&mut m, &c);
        let after: Vec<u32> = (0..4).map(|i| mask(&m, i)).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn materials_left_to_the_d3d_path_keep_their_bit() {
        let mut m = scene();
        m.w(ITEMS + 8, 0xe000_0042);
        let c = clear_structure(&mut m, LIGHTMAP_LAYER, &|shader| shader != 0xe000_0042).unwrap();
        assert!(c.items.is_empty());
        assert_eq!(mask(&m, 0), 0b1010);
    }

    #[test]
    fn a_structure_item_names_its_cluster_or_placement_and_part() {
        let mut m = scene();
        // Record 2: a cluster (bit 12 clear), cluster 37 in bits 9..17,
        // no placement; record 7: placement 140 (bit 12 set).
        m.w(RENDER_RECORDS + 0x20 * 2, 0x0000_0040);
        m.w(RENDER_RECORDS + 0x20 * 2 + 0xc, 0x7ff << 18 | 37 << 9 | 0x15);
        m.w(RENDER_RECORDS + 0x20 * 7, 1 << 12);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0xc, 140 << 18 | 3 << 9);
        m.w(ITEMS + 0xc, 2 << 8 | 4);
        m.w(ITEMS + ITEM_SIZE * 2 + 0xc, 7 << 8);
        assert_eq!(structure_part(&m, ITEMS), Some((Owner::Cluster(37), 4)));
        assert_eq!(structure_part(&m, ITEMS + ITEM_SIZE * 2), Some((Owner::Instance(140), 0)));
        m.w(ITEMS + 0xc, 0x00ff_ff04);
        assert_eq!(structure_part(&m, ITEMS), None, "record -1");
    }

    #[test]
    fn the_skip_by_part_clears_only_the_parts_the_native_pass_draws() {
        // Two BSP items of one shader, cluster 37's parts 4 and 5: only
        // part 4 is native (part 5's material is left to D3D).
        let mut m = scene();
        m.w(RENDER_RECORDS + 0x20 * 2, 0);
        m.w(RENDER_RECORDS + 0x20 * 2 + 0xc, 0x7ff << 18 | 37 << 9);
        m.w(ITEMS + 0xc, 2 << 8 | 4);
        let second = ITEMS + ITEM_SIZE * 2;
        m.w(second, GEOMETRY_BUILD);
        m.w(second + 8, 0xe000_0010);
        m.w(second + 0xc, 2 << 8 | 5);
        m.w(second + 0x10, 0b1010);
        let native = |o: Owner, part: u8, shader: u32| o == Owner::Cluster(37) && part == 4 && shader == 0xe000_0010;
        let c = clear_parts(&mut m, LIGHTMAP_LAYER, &native).unwrap();
        assert_eq!(c.items, vec![ITEMS]);
        assert_eq!(mask(&m, 0), 0b0010);
        assert_eq!(mask(&m, 2), 0b1010, "the same shader, a part left to D3D");
        assert_eq!(mask(&m, 1), 0b1000, "objects are never cleared");
        restore(&mut m, &c);
        assert_eq!(mask(&m, 0), 0b1010);
        // Another shader on the native part (a stale table): kept.
        m.w(ITEMS + 8, 0xe000_0099);
        assert!(clear_parts(&mut m, LIGHTMAP_LAYER, &native).unwrap().items.is_empty());
    }

    #[test]
    fn another_layer_or_no_list_writes_nothing() {
        let mut m = scene();
        m.w(CURRENT_LAYER, 1);
        assert_eq!(clear_structure(&mut m, LIGHTMAP_LAYER, &|_| true), None);
        assert_eq!(mask(&m, 0), 0b1010);
        let mut m = scene();
        m.w(CURRENT_LIST, 0);
        assert_eq!(clear_structure(&mut m, LIGHTMAP_LAYER, &|_| true), None);
    }

    #[test]
    fn the_walk_starts_at_the_first_item_and_negative_records_are_not_bsp() {
        let mut m = scene();
        m.w(LIST + 0xc, 1);
        assert_eq!(clear_structure(&mut m, LIGHTMAP_LAYER, &|_| true).unwrap().items, Vec::<u32>::new());
        let mut m = scene();
        // Record index -1 in the argument's low word.
        m.w(ITEMS + 0xc, 0x00ff_ff01);
        assert_eq!(is_structure_item(&m, ITEMS), Some(false));
    }
}
