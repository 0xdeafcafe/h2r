//! Which of the game's layer entries the native world replaces, and the
//! skip: clear their layer bit before the layer draw, restore it after
//! (docs/graphics/native-world.md, "Skip only the replaced work").
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
//!   object handle: [`crate::layout::objects::record_owner`] finds that).
//! - Which BSP geometry a structure item draws (the build `0x0004cbf0`,
//!   `0x0004cec1`..`0x0004cf21`): the record's `+0` bit 12 clear, a cluster
//!   (`+0xc` bits 9..17, the structure BSP's clusters at `+0xa0`, `0xb0`
//!   each); set, an instanced-geometry placement (`+0xc` bits 18..28, the
//!   placements at `+0x144`, `0x58` each); then the argument's low byte
//!   is the index into that section's parts (`[section+4] + 0x48·part`).
//! - The geometry draw `0x0004d0b0` switches the stencil around an item in
//!   layers 1 and 3: before it (`0x0004d640`), with either flag byte
//!   `0x00485a75`, `0x00485a76` set, mode 0 (stencil off); else, when the
//!   render record's `+0` has bit 14, mode 2 (`ALWAYS`, reference `0xff`,
//!   `REPLACE` on pass); after it (`0x0004d720`), on either condition,
//!   mode 1 (the same with reference 0). Modes by `0x00015370`'s table at
//!   `0x00015658`. Bit 14 is the item's visibility flag `0x200` moved up
//!   (`0x001332f0`). An item with neither leaves the stencil as the layer
//!   found it. [`stencil_plan`] says which do.

use crate::{GuestRead, GuestWrite};
use frame_scene::Owner;

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
pub const MAX_ITEMS: u32 = 1 << 16;
/// The geometry draw (an item's `+4`) that switches the stencil.
pub const GEOMETRY_DRAW: u32 = 0x0004_d0b0;
/// The render record's `+0` bit that marks an item's pixels in the
/// stencil (mode 2).
pub const STENCIL_MARK: u32 = 1 << 14;
/// The word holding the two flag bytes `0x00485a75` and `0x00485a76` (its
/// bytes 1 and 2): either set, layers 1 and 3 draw their items with the
/// stencil off.
pub const STENCIL_OFF_FLAGS: u32 = 0x0048_5a74;

/// Items whose layer bit was cleared, to set back after the layer draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub layer: u32,
    /// Item addresses.
    pub items: Vec<u32>,
}

/// A preview of a guest mask walk. Reads see earlier edits, but the guest sees
/// nothing until every GPU copy has been reserved and the original words still match.
pub struct MaskPreview<'a> {
    guest: &'a dyn GuestRead,
    reads: std::cell::RefCell<std::collections::BTreeMap<u32, u32>>,
    edits: std::collections::BTreeMap<u32, u32>,
    failed: std::cell::Cell<bool>,
}

pub struct MaskPlan {
    reads: std::collections::BTreeMap<u32, u32>,
    edits: Vec<(u32, u32)>,
}

impl<'a> MaskPreview<'a> {
    pub fn new(guest: &'a dyn GuestRead) -> Option<Self> {
        let list = guest.read_u32(CURRENT_LIST)?;
        let first = guest.read_u32(list.checked_add(0xc)?)?;
        let count = guest.read_u32(list.checked_add(4)?)?;
        let items = guest.read_u32(list.checked_add(0x14)?)?;
        if list == 0 || first > count || count - first > MAX_ITEMS { return None; }
        items.checked_add(count.checked_mul(ITEM_SIZE)?)?;
        let reads = [(CURRENT_LIST, list), (list + 0xc, first), (list + 4, count), (list + 0x14, items)].into();
        Some(Self { guest, reads: std::cell::RefCell::new(reads), edits: Default::default(), failed: false.into() })
    }

    pub fn finish(self) -> Option<MaskPlan> {
        let reads = self.reads.into_inner();
        let edits = self.edits.into_iter().filter(|(k, v)| reads.get(k) != Some(v)).collect();
        (!self.failed.get()).then_some(MaskPlan { reads, edits })
    }
}

impl GuestRead for MaskPreview<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        if let Some(v) = self.edits.get(&addr) { return Some(*v); }
        if let Some(v) = self.reads.borrow().get(&addr) { return Some(*v); }
        let Some(v) = self.guest.read_u32(addr) else { self.failed.set(true); return None };
        self.reads.borrow_mut().insert(addr, v);
        Some(v)
    }
}

impl GuestWrite for MaskPreview<'_> {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
        self.read_u32(addr)?;
        self.edits.insert(addr, value);
        Some(())
    }
}

impl MaskPlan {
    /// Reopen a detached mask walk to restore declined items in the overlay.
    /// Applying the resulting plan still validates the original read set.
    pub fn edit(self, guest: &dyn GuestRead) -> MaskPreview<'_> {
        MaskPreview {
            guest, reads: self.reads.into(), edits: self.edits.into_iter().collect(), failed: false.into(),
        }
    }

    /// Revalidate the list identity, item words and masks, then apply the edits.
    /// If a write faults, put the original words back before D3D proceeds.
    pub fn apply(&self, guest: &mut dyn GuestWrite) -> bool {
        if !self.reads.iter().all(|(&a, &v)| guest.read_u32(a) == Some(v)) { return false; }
        for (i, &(a, value)) in self.edits.iter().enumerate() {
            if guest.write_u32(a, value).is_none() {
                for &(addr, _) in self.edits[..=i].iter().rev() {
                    let old = self.reads[&addr];
                    assert!(guest.read_u32(addr) == Some(old) || guest.write_u32(addr, old).is_some(),
                        "native mask rollback failed at {addr:#x}");
                }
                return false;
            }
        }
        true
    }
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

/// A placement the current list draws in `layer`, with the render record
/// that draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementRecord {
    /// The placement's index in the BSP's instanced geometry.
    pub placement: u32,
    /// The record's address ([`RENDER_RECORDS`] plus its index times
    /// [`RENDER_RECORD_SIZE`]).
    pub record: u32,
    /// The record's fade byte (`+0x1a`): `0xff` draws solid, any other the
    /// pattern the guest's `SetStipple` takes for it.
    pub fade: u8,
    /// The placement's own record in the BSP (`+0x1c`), which the fade
    /// window reads; `0` when the record holds none.
    pub at: u32,
}

/// The placements `layer`'s current list draws, one per placement in the
/// order the list first names it, with their records' fade bytes. Every
/// item of a placement shares its record. `None` when the layer is not
/// current or the list does not read.
pub fn placement_records(mem: &dyn GuestRead, layer: u32) -> Option<Vec<PlacementRecord>> {
    if layer >= 32 || mem.read_u32(CURRENT_LAYER)? != layer {
        return None;
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return Some(Vec::new());
    }
    let count = mem.read_u32(list + 4)?;
    let first = mem.read_u32(list + 0xc)?;
    let items = mem.read_u32(list + 0x14)?;
    let mut out: Vec<PlacementRecord> = Vec::new();
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        if mem.read_u32(item + 0x10)? & (1 << layer) == 0 || !is_structure_item(mem, item)? {
            continue;
        }
        let Some((Owner::Instance(placement), _)) = structure_part(mem, item) else { continue };
        if out.iter().any(|p| p.placement == placement) {
            continue;
        }
        let record = RENDER_RECORDS + RENDER_RECORD_SIZE * ((mem.read_u32(item + 0xc)? >> 8) as u16 as i16 as u32);
        out.push(PlacementRecord {
            placement,
            record,
            fade: (mem.read_u32(record + 0x18)? >> 16) as u8,
            at: mem.read_u32(record + 0x1c)?,
        });
    }
    Some(out)
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

/// How `layer`'s draw switches the stencil around its items (the module
/// notes), for a native pass standing in for some of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StencilPlan {
    /// Either flag byte set: every item draws with the stencil off (mode
    /// 0) and leaves mode 1.
    pub off: bool,
    /// The BSP sections (clusters, placements) whose items' records have
    /// [`STENCIL_MARK`]: they draw in mode 2 (reference `0xff`) and leave
    /// mode 1. Ascending.
    pub marked: Vec<Owner>,
    /// Whether any item switches the stencil (`off`, or any marked item,
    /// an object's too): the items after one draw in mode 1 whatever the
    /// layer started with.
    pub switches: bool,
}

/// [`StencilPlan`] of `layer`'s current list: only layers 1 and 3 switch;
/// an item counts when it is in the layer and drawn by the geometry draw
/// (built by it, or drawn by it). `None` when something it reads does
/// not.
pub fn stencil_plan(mem: &dyn GuestRead, layer: u32) -> Option<StencilPlan> {
    let mut plan = StencilPlan::default();
    if layer != 1 && layer != 3 {
        return Some(plan);
    }
    if mem.read_u32(STENCIL_OFF_FLAGS)? & 0x00ff_ff00 != 0 {
        plan.off = true;
        plan.switches = true;
        return Some(plan);
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return Some(plan);
    }
    let count = mem.read_u32(list + 4)?;
    let first = mem.read_u32(list + 0xc)?;
    let items = mem.read_u32(list + 0x14)?;
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        if mem.read_u32(item + 0x10)? & (1 << layer) == 0 {
            continue;
        }
        if mem.read_u32(item)? != GEOMETRY_BUILD && mem.read_u32(item + 4)? != GEOMETRY_DRAW {
            continue;
        }
        let record = (mem.read_u32(item + 0xc)? as i32 >> 8) as u16 as i16;
        if record < 0 {
            continue;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        if mem.read_u32(base)? & STENCIL_MARK == 0 {
            continue;
        }
        plan.switches = true;
        if mem.read_u32(base + 4)? == u32::MAX {
            plan.marked.push(structure_part(mem, item)?.0);
        }
    }
    plan.marked.sort_unstable();
    plan.marked.dedup();
    Some(plan)
}

/// Whether `layer`'s current list holds a BSP item in the layer
/// ([`is_structure_item`]): the layer draws the world. Not every layer-3
/// draw does: the first-person render (`0x0003fdf0`) draws layers 1, 3,
/// 8, 20 and 7 of the scene list built again with the player's
/// first-person models only (layer 3 at `0x000400ce`..`0x00040131`), with
/// its own projection in the view constants and, under the flag byte
/// `0x00485a76` its caller sets, over the scene's depth (no clear,
/// `0x0004004f`..`0x00040066`). A native pass asked there draws the whole
/// world again over the finished scene. `None` when the list does not
/// read.
pub fn has_structure_items(mem: &dyn GuestRead, layer: u32) -> Option<bool> {
    if layer >= 32 {
        return Some(false);
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return Some(false);
    }
    let count = mem.read_u32(list + 4)?;
    let first = mem.read_u32(list + 0xc)?;
    let items = mem.read_u32(list + 0x14)?;
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        if mem.read_u32(item + 0x10)? & (1 << layer) == 0 {
            continue;
        }
        if is_structure_item(mem, item)? {
            return Some(true);
        }
    }
    Some(false)
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
    use crate::testing::Mem;

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

    #[test]
    fn mask_preview_rechecks_identity_and_rolls_back_a_write_fault() {
        struct Fault(Mem, u32);
        impl GuestRead for Fault {
            fn read_u32(&self, a: u32) -> Option<u32> { self.0.read_u32(a) }
        }
        impl GuestWrite for Fault {
            fn write_u32(&mut self, a: u32, v: u32) -> Option<()> {
                if a == self.1 { None } else { self.0.write_u32(a, v) }
            }
        }
        let mut guest = Fault(scene(), ITEMS + ITEM_SIZE + 0x10);
        let mut preview = MaskPreview::new(&guest).unwrap();
        let a = ITEMS + 0x10;
        let b = a + ITEM_SIZE;
        let original = [guest.read_u32(a), guest.read_u32(b)];
        preview.read_u32(CURRENT_LIST).unwrap();
        preview.write_u32(a, 2).unwrap();
        preview.write_u32(b, 0).unwrap();
        let plan = preview.finish().unwrap();
        assert_eq!([guest.read_u32(a), guest.read_u32(b)], original);
        assert!(!plan.apply(&mut guest));
        assert_eq!([guest.read_u32(a), guest.read_u32(b)], original);
        guest.1 = 0;
        guest.0.w(CURRENT_LIST, LIST + 4);
        assert!(!plan.apply(&mut guest));
        guest.0.w(CURRENT_LIST, LIST);
        assert!(plan.apply(&mut guest));
        assert_eq!([guest.read_u32(a), guest.read_u32(b)], [Some(2), Some(0)]);
    }

    #[test]
    fn a_marked_record_or_a_flag_switches_the_stencil_in_layers_1_and_3() {
        let mut m = scene();
        m.w(STENCIL_OFF_FLAGS, 0x1200_0011);
        for record in [2, 5, 7] {
            m.w(RENDER_RECORDS + 0x20 * record, 0x1008 | 0x8000);
            m.w(RENDER_RECORDS + 0x20 * record + 0xc, 4 << 18 | 3 << 9);
        }
        let none = Some(StencilPlan::default());
        assert_eq!(stencil_plan(&m, 3), none, "no mark, no flag (the bytes around them do not count)");
        assert_eq!(stencil_plan(&m, 5), none, "another layer");
        // Record 7's item is in layer 1 only: its mark counts there. Its
        // `+0` bit 12 is set: placement 4.
        m.w(RENDER_RECORDS + 0x20 * 7, STENCIL_MARK | 1 << 12);
        assert_eq!(stencil_plan(&m, 3), none);
        let plan = stencil_plan(&m, 1).unwrap();
        assert_eq!((plan.off, plan.switches, plan.marked), (false, true, vec![Owner::Instance(4)]));
        // Record 2 (cluster 3), drawn by two items in layer 3.
        m.w(RENDER_RECORDS + 0x20 * 2, STENCIL_MARK);
        let plan = stencil_plan(&m, 3).unwrap();
        assert_eq!((plan.switches, plan.marked), (true, vec![Owner::Cluster(3)]), "once");
        m.w(RENDER_RECORDS + 0x20 * 2, 0);
        // An object's item (record 5) in layer 3 switches it too, but is
        // not a BSP section.
        m.w(RENDER_RECORDS + 0x20 * 5, STENCIL_MARK | 1);
        let plan = stencil_plan(&m, 3).unwrap();
        assert_eq!((plan.switches, plan.marked), (true, vec![]));
        m.w(RENDER_RECORDS + 0x20 * 5, 0);
        assert_eq!(stencil_plan(&m, 3), none);
        for flag in [0x0000_0100, 0x0001_0000] {
            m.w(STENCIL_OFF_FLAGS, flag);
            let plan = stencil_plan(&m, 3).unwrap();
            assert!(plan.off && plan.switches, "flag {flag:#x}");
        }
        // Unreadable: the caller cannot say.
        let mut m = scene();
        m.w(STENCIL_OFF_FLAGS, 0);
        assert_eq!(stencil_plan(&m, 3), None, "the records' first words are not there");
    }

    #[test]
    fn a_list_of_models_only_does_not_draw_the_world() {
        let mut m = scene();
        assert_eq!(has_structure_items(&m, 3), Some(true), "item 0, a cluster in layers 1 and 3");
        assert_eq!(has_structure_items(&m, 1), Some(true));
        assert_eq!(has_structure_items(&m, 5), Some(false), "no item in layer 5");
        // The first-person render's list: models only (item 0's record an
        // object's too). The object item (1) and the other build (3) are
        // in layer 3; item 2 (BSP) is in layer 1 only.
        m.w(RENDER_RECORDS + 0x20 * 2 + 4, 0xe1a3_0009);
        assert_eq!(has_structure_items(&m, 3), Some(false));
        assert_eq!(has_structure_items(&m, 1), Some(true), "item 2, record 7");
        // A list starting past its BSP items, and no list.
        let mut m = scene();
        m.w(LIST + 0xc, 1);
        assert_eq!(has_structure_items(&m, 3), Some(false));
        m.w(CURRENT_LIST, 0);
        assert_eq!(has_structure_items(&m, 3), Some(false));
        // An unreadable record: the caller cannot say.
        let mut m = scene();
        m.w(LIST + 0x14, 0x7000_0000);
        assert_eq!(has_structure_items(&m, 3), None);
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
    fn a_placement_names_its_records_fade_once_for_all_its_items() {
        // Record 7: placement 140, fade byte 0x88 (+0x1a), its placement
        // record at +0x1c; two items of it, and a cluster item that names
        // no placement.
        let mut m = scene();
        m.w(RENDER_RECORDS + 0x20 * 7, 1 << 12);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0xc, 140 << 18);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0x18, 0x0088_0000);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0x1c, 0x1234_0000);
        m.w(RENDER_RECORDS + 0x20 * 2, 0);
        m.w(RENDER_RECORDS + 0x20 * 2 + 0xc, 0x7ff << 18 | 37 << 9);
        m.w(ITEMS + 0xc, 2 << 8);
        let second = ITEMS + ITEM_SIZE * 2;
        m.w(second + 0xc, 7 << 8);
        m.w(second + 0x10, 0b1000);
        let third = ITEMS + ITEM_SIZE * 3;
        m.w(third, GEOMETRY_BUILD);
        m.w(third + 0xc, 7 << 8 | 1);
        m.w(third + 0x10, 0b1000);
        let got = placement_records(&m, LIGHTMAP_LAYER).unwrap();
        assert_eq!(got, vec![PlacementRecord {
            placement: 140,
            record: RENDER_RECORDS + 0x20 * 7,
            fade: 0x88,
            at: 0x1234_0000,
        }]);
        // A solid placement reads 0xff; another layer reads nothing.
        m.w(RENDER_RECORDS + 0x20 * 7 + 0x18, 0x00ff_0000);
        assert_eq!(placement_records(&m, LIGHTMAP_LAYER).unwrap()[0].fade, 0xff);
        assert_eq!(placement_records(&m, 1), None);
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
