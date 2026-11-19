//! Planar fog, layer 16 (docs/graphics/native-renderer.md, "Stage 4b"):
//! the world's materials drawn again with their own layer-16 passes
//! (`crate::fog_gpu`, with the `gpu` feature), blended over the finished
//! scene by the fog the device holds. This module is the skip: what the
//! layer's items are, the layer draw's own gate, and the walk that
//! clears their bit ([`clear_fog`]).
//!
//! Research points (Code, the lifted ports `engine::render_loop::scene`
//! and `layer`):
//!
//! - `render_scene_passes` calls `0x0002_5d50` between its state call and
//!   the layer-16 begin (`0x0002c0d6`), then the layer draw (returns
//!   `0x0002c0e9`); the call's own content is not read, but whatever it
//!   pushes is in the device's state before the layer begins, so a pass
//!   asked at the layer draw reads it after the drain.
//! - The layer draw draws layer 16 only when `[FOG_MODE] == 3` or either
//!   fog byte (`0x00485a75`, `0x00485a76`) is set (`0x00044582`); a
//!   native pass applies the same gate ([`layer_draws`]).
//! - The layer's items are the scene list's with bit 16 set: the same
//!   structure and object items layer 3 drew. Only the structure items
//!   are cleared here (clusters first: a placement's fog needs the rigid
//!   program and its rows, as its layer 3 does); objects' fog is stage
//!   2's.

use crate::camera::{GuestRead, GuestWrite};
use crate::skip::{
    self, Cleared, CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, MAX_ITEMS,
    RENDER_RECORDS, RENDER_RECORD_SIZE,
};
use crate::world::Owner;

/// The layer this module draws.
pub const FOG_LAYER: usize = 16;
/// The layer draw's own gate (`0x00044582`): the fog mode (`0x0048574c`)
/// and the two fog bytes (`0x00485a75`, `0x00485a76`, the second the
/// player-chain's; the word at `0x00485a74`'s bytes 1 and 2, as
/// [`crate::skip::STENCIL_OFF_FLAGS`] reads it). Layer 16 draws when the
/// mode is 3 or either byte is set; the same test gates a native pass.
pub const FOG_MODE: u32 = 0x0048_574c;
pub const FOG_BYTES: u32 = 0x0048_5a74;
/// The late passes' layer-16 call: the layer draw's return address
/// (`[esp]` at its entry). Other callers of the layer draw with layer 16
/// current are not the scene's fog and stay on the D3D path.
pub const SCENE_FOG_RETURN: u32 = 0x0002_c0e9;

/// One structure item of the fog layer: what it draws and its shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FogItem {
    pub owner: Owner,
    pub part: u8,
    /// The item's shader datum's low word, the tag index.
    pub shader: u16,
}

/// Whether the layer draw would draw layer 16 at all: its own gate
/// (`0x00044582`), as the native pass must apply it. `None` when the
/// gate's words do not read.
pub fn layer_draws(mem: &dyn GuestRead) -> Option<bool> {
    if mem.read_u32(FOG_MODE)? == 3 {
        return Some(true);
    }
    Some(mem.read_u32(FOG_BYTES)? & 0x00ff_ff00 != 0)
}

/// Walks the current list's structure items of `layer` (the world's
/// walk, [`crate::skip`]'s, keeping the accepted items in order) and
/// clears the layer's bit on those `native` accepts: the fog pass draws
/// in the game's item order, whatever it blends. Object items are never
/// cleared (their fog is stage 2's). Returns the items accepted (in
/// order) and what was cleared, for the restore at the next layer draw.
/// `None` (nothing written) when it is another layer or the list does
/// not read; an unreadable item stops the walk with what was cleared.
pub fn clear_fog(
    mem: &mut dyn GuestWrite,
    layer: u32,
    native: &dyn Fn(FogItem) -> bool,
) -> Option<(Vec<FogItem>, Cleared)> {
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
    let mut cleared = Cleared { layer, items: Vec::new() };
    let mut drawn = Vec::new();
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        let Some(mask) = mem.read_u32(item + 0x10) else { break };
        if mask & bit == 0 {
            continue;
        }
        let Some(build) = mem.read_u32(item) else { break };
        if build != GEOMETRY_BUILD {
            continue;
        }
        let Some(arg) = mem.read_u32(item + 0xc) else { break };
        let Some(shader) = mem.read_u32(item + 8) else { break };
        let record = (arg >> 8) as u16 as i16;
        if record < 0 {
            continue;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        let Some(model) = mem.read_u32(base + 4) else { break };
        if model != u32::MAX {
            continue; // an object's item: its fog is stage 2's
        }
        let Some(packed) = mem.read_u32(base + 0xc) else { break };
        let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
            Owner::Cluster((packed >> 9) & 0x1ff)
        } else {
            Owner::Instance((packed >> 18) & 0x7ff)
        };
        let fog_item = FogItem { owner, part: arg as u8, shader: shader as u16 };
        if !native(fog_item) {
            continue;
        }
        if mem.write_u32(item + 0x10, mask & !bit).is_none() {
            break;
        }
        cleared.items.push(item);
        drawn.push(fog_item);
    }
    Some((drawn, cleared))
}

/// Sets the bits [`clear_fog`] cleared back.
pub fn restore(mem: &mut dyn GuestWrite, cleared: &Cleared) {
    skip::restore(mem, cleared);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;
    const DRAW: u32 = 0x0004_d0b0;

    /// A layer-16 list of four items: a BSP cluster, an object, a BSP
    /// placement, and another build's; records 2 (cluster), 5 (object),
    /// 7 (placement).
    fn scene() -> Mem {
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, 16);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST, 100);
        m.w(LIST + 4, 4);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        let item = |m: &mut Mem, i: u32, build: u32, record: u32| {
            let a = ITEMS + ITEM_SIZE * i;
            m.w(a, build);
            m.w(a + 4, DRAW);
            m.w(a + 8, 0xe000_0010 + i);
            m.w(a + 0xc, record << 8 | (i + 1));
            m.w(a + 0x10, 1 << 16 | 1 << 3);
        };
        item(&mut m, 0, GEOMETRY_BUILD, 2);
        item(&mut m, 1, GEOMETRY_BUILD, 5);
        item(&mut m, 2, GEOMETRY_BUILD, 7);
        item(&mut m, 3, 0x0005_0000, 2);
        m.w(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX);
        m.w(RENDER_RECORDS + 0x20 * 2, 0);
        m.w(RENDER_RECORDS + 0x20 * 2 + 0xc, 37 << 9);
        m.w(RENDER_RECORDS + 0x20 * 5 + 4, 0xe1a3_0004);
        m.w(RENDER_RECORDS + 0x20 * 7 + 4, u32::MAX);
        m.w(RENDER_RECORDS + 0x20 * 7, 1 << 12);
        m.w(RENDER_RECORDS + 0x20 * 7 + 0xc, 140 << 18);
        m
    }

    fn mask(m: &Mem, i: u32) -> u32 {
        m.read_u32(ITEMS + ITEM_SIZE * i + 0x10).unwrap()
    }

    #[test]
    fn the_gate_is_the_layer_draws_own() {
        let mut m = scene();
        m.w(FOG_MODE, 0);
        m.w(FOG_BYTES, 0);
        assert_eq!(layer_draws(&m), Some(false));
        m.w(FOG_MODE, 3);
        assert_eq!(layer_draws(&m), Some(true));
        m.w(FOG_MODE, 1);
        assert_eq!(layer_draws(&m), Some(false), "mode 1 alone does not draw");
        for word in [0x0000_0100, 0x0001_0000] {
            m.w(FOG_BYTES, word);
            assert_eq!(layer_draws(&m), Some(true), "a fog byte ({word:#x}) draws");
        }
    }

    #[test]
    fn only_structure_items_lose_their_bit_in_order() {
        let mut m = scene();
        let (drawn, cleared) = clear_fog(&mut m, 16, &|_| true).unwrap();
        assert_eq!(
            drawn,
            vec![
                FogItem { owner: Owner::Cluster(37), part: 1, shader: 0x0010 },
                FogItem { owner: Owner::Instance(140), part: 3, shader: 0x0012 },
            ],
            "the object (1) and the other build (3) stay"
        );
        assert_eq!(cleared.items, vec![ITEMS, ITEMS + ITEM_SIZE * 2]);
        assert_eq!(mask(&m, 0), 1 << 3, "layer 3 kept, layer 16 gone");
        assert_eq!(mask(&m, 1), 1 << 16 | 1 << 3, "the object kept");
        restore(&mut m, &cleared);
        assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    }

    #[test]
    fn the_walk_clears_only_what_the_host_accepts() {
        let mut m = scene();
        let native = |i: FogItem| i == FogItem { owner: Owner::Cluster(37), part: 1, shader: 0x0010 };
        let (drawn, cleared) = clear_fog(&mut m, 16, &native).unwrap();
        assert_eq!(drawn.len(), 1);
        assert_eq!(cleared.items, vec![ITEMS]);
        assert_eq!(mask(&m, 2), 1 << 16 | 1 << 3, "a placement the host leaves");
        // A shader the host does not draw with: kept, even on the part.
        let other = |i: FogItem| i.shader == 0x0099;
        let mut m = scene();
        assert!(clear_fog(&mut m, 16, &other).unwrap().0.is_empty());
    }

    #[test]
    fn another_layer_or_no_list_writes_nothing() {
        let mut m = scene();
        m.w(CURRENT_LAYER, 3);
        assert!(clear_fog(&mut m, 16, &|_| true).is_none());
        m.w(CURRENT_LAYER, 16);
        m.w(CURRENT_LIST, 0);
        assert!(clear_fog(&mut m, 16, &|_| true).is_none());
        assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    }
}
