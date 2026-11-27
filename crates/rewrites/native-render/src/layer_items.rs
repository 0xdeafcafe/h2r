//! The pure half of the layer skip (docs/graphics/framescene.md): the
//! list items a layer draw holds, the world layers' view slots, and the
//! clear of a scene-only layer's bits. It reads and writes guest memory
//! through [`crate::camera::GuestWrite`] and needs no GPU, so the feature
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

/// The world layers that draw only from the scene (native-only design
/// section 2): 1, 3, 5, 8 and 20 in slot 1, 7 in slot 3 after the object
/// shadows, 2 in slot 6. A part of one of them the scene cannot draw is a
/// named drop, never a guest draw.
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

/// What [`clear_scene_layer`] cleared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneLayerSkip {
    /// The items the scene draws, in the list's order.
    pub drawn: Vec<ListItem>,
    /// Every item whose bit was cleared, drawn or dropped, for the restore.
    pub cleared: crate::skip::Cleared,
    /// Items the scene did not draw: their guest draw is gone too.
    pub refused: usize,
    /// Items whose render record did not read: dropped, never drawn.
    pub unreadable: usize,
}

/// `layer_pass::clear_layer_items` for a layer that draws only from the scene: no
/// kept item keeps a guest draw, so there is no order rule. Each geometry
/// item of the list holding `layer`'s bit loses it: `take` says the scene
/// draws it ([`SceneLayerSkip::drawn`]); the rest are counted `refused` (the
/// scene has no draw for them) or `unreadable` (the item's render record
/// does not read). `objects` false leaves the object items alone (layer
/// 3's objects are consumed by the sorted walk). Items that are not a
/// geometry build, or draw no record, draw nothing and keep their bit.
/// `None` when the layer is not current or the list does not read; nothing
/// is written then.
pub fn clear_scene_layer(
    mem: &mut dyn crate::camera::GuestWrite,
    layer: u32,
    objects: bool,
    take: &dyn Fn(ListItem) -> bool,
) -> Option<SceneLayerSkip> {
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
    let mut out = SceneLayerSkip {
        drawn: Vec::new(),
        cleared: crate::skip::Cleared { layer, items: Vec::new() },
        refused: 0,
        unreadable: 0,
    };
    let mut writes = Vec::new();
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
        match found {
            Some(ListItem::Object { .. }) if !objects => continue,
            Some(found) if take(found) => out.drawn.push(found),
            Some(_) => out.refused += 1,
            None => out.unreadable += 1,
        }
        writes.push((item, mask));
    }
    for (item, mask) in writes {
        mem.write_u32(item + 0x10, mask & !bit)?;
        out.cleared.items.push(item);
    }
    Some(out)
}
