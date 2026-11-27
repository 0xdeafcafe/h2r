//! The pure half of the layer skip (docs/graphics/framescene.md): the
//! list items a layer draw holds, the world layers' view slots, and the
//! read of the items a layer leaves to the guest. It reads guest memory
//! through [`crate::camera::GuestRead`] and needs no GPU, so the feature
//! crate's hooks build it without the `gpu` feature; `layer_pass`
//! re-exports it.

/// One structure item of a layer draw's list as the skip sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkipItem {
    /// Its index in the list ([`frame_scene::LayerItem::slot`]).
    pub slot: u32,
    pub owner: crate::world::Owner,
    /// The low byte of its render argument: the index into its section's
    /// parts the build `0x0004cbf0` draws.
    pub part: u8,
    /// The item's shader datum (`+8`).
    pub shader: u32,
}

/// One item of a layer draw's list as the skip sees it: a structure
/// item, or an object's by its slot and shader datum (what the scene's
/// [`frame_scene::LayerItem`] for it holds: the plan decides it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListItem {
    World(SkipItem),
    Object { slot: u32, shader: u32 },
}

impl ListItem {
    pub fn slot(self) -> u32 {
        match self {
            ListItem::World(i) => i.slot,
            ListItem::Object { slot, .. } => slot,
        }
    }
}

/// The world layers cut to the scene (native-only design
/// section 2): 1, 3, 5, 8 and 20 in slot 1, 7 in slot 3 after the object
/// shadows, 2 in slot 6. A part of one of them the scene cannot draw keeps
/// its guest draw, counted by layer, kind and cause (`scene_kept_*`,
/// [`held_items`]), until the scene draws it.
pub const WORLD_LAYERS: [u8; 7] = [1, 2, 3, 5, 7, 8, 20];

/// The view slot a world layer draws in; `None` for any other layer.
pub fn world_slot(layer: u8) -> Option<frame_scene::ViewSlot> {
    use frame_scene::ViewSlot;
    match layer {
        1 | 3 | 5 | 8 | 20 => Some(ViewSlot::OpaqueBase),
        7 => Some(ViewSlot::ShadowsAndLayer7),
        2 => Some(ViewSlot::Layer2),
        _ => None,
    }
}

/// One geometry item of the current list that holds a layer's bit, as
/// [`held_items`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    /// Its index in the list.
    pub slot: u32,
    /// The item, or `None` when its render record does not read.
    pub item: Option<ListItem>,
}

impl Held {
    /// The counter name of its kind: `world`, `object` or `unreadable`.
    pub fn kind(self) -> &'static str {
        match self.item {
            Some(ListItem::World(_)) => "world",
            Some(ListItem::Object { .. }) => "object",
            None => "unreadable",
        }
    }
}

/// The geometry items of the current list that still hold `layer`'s bit,
/// in the list's order: after the native pass cleared the bit on those it
/// draws, the ones the guest still draws. `objects` false leaves the
/// object items out (layer 3's are the sorted walk's). Items that are not
/// a geometry build, or draw no record, are not listed. Reads only.
/// `None` when the layer is not current or the list does not read.
pub fn held_items(mem: &dyn crate::camera::GuestRead, layer: u32, objects: bool) -> Option<Vec<Held>> {
    use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, MAX_ITEMS, RENDER_RECORD_SIZE, RENDER_RECORDS};
    use crate::world::Owner;
    if layer >= 32 || mem.read_u32(CURRENT_LAYER)? != layer {
        return None;
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return None;
    }
    let count = mem.read_u32(list.checked_add(4)?)?;
    let first = mem.read_u32(list.checked_add(0xc)?)?;
    let items = mem.read_u32(list.checked_add(0x14)?)?;
    if first > count || count - first > MAX_ITEMS {
        return None;
    }
    items.checked_add(count.checked_mul(ITEM_SIZE)?)?;
    let bit = 1u32 << layer;
    let mut out = Vec::new();
    for i in first..count {
        let item = items + ITEM_SIZE * i;
        let mask = mem.read_u32(item + 0x10)?;
        if mask & bit == 0 || mem.read_u32(item)? != GEOMETRY_BUILD {
            continue;
        }
        let arg = mem.read_u32(item + 0xc)?;
        let record = (arg >> 8) as u16 as i16;
        if record < 0 {
            continue;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        let found = (|| {
            let shader = mem.read_u32(item + 8)?;
            Some(if mem.read_u32(base + 4)? != u32::MAX {
                ListItem::Object { slot: i, shader }
            } else {
                let packed = mem.read_u32(base + 0xc)?;
                let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
                    Owner::Cluster((packed >> 9) & 0x1ff)
                } else {
                    Owner::Instance((packed >> 18) & 0x7ff)
                };
                ListItem::World(SkipItem { slot: i, owner, part: arg as u8, shader })
            })
        })();
        if !objects && matches!(found, Some(ListItem::Object { .. })) {
            continue;
        }
        out.push(Held { slot: i, item: found });
    }
    Some(out)
}
