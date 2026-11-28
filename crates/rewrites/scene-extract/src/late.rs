//! Guest memory to a view's late passes (docs/graphics/framescene.md,
//! slice 5): what the fog's and the water's layer draws hold in the
//! current list, and what `render_screen_effects` and its callees are
//! asked to draw. The fog words come from the device, not from here: the
//! glue reads them through `d3d::Device::render_state` at the layer's
//! draw and hands them to the builder (`frame_scene::LateInput`).

use frame_scene::{PostEffect, PostParams};

use crate::GuestRead;
use crate::layout::post::PostFrame;
use crate::layout::skip::{CURRENT_LAYER, CURRENT_LIST, ITEM_SIZE, MAX_ITEMS};

/// Returns after the layer draws in the scene's late passes. The water
/// draw at 0x0002c11b is inside the water setup and teardown.
pub const SCENE_FOG_RETURN: u32 = 0x0002_c0e9;
pub const SCENE_WATER_RETURN: u32 = 0x0002_c120;

/// The first-person renderer normally begins layer 2 here. A fog range at
/// this site must independently identify layer 16 in the live handler.
pub const FIRST_PERSON_OVERLAY_RETURN: u32 = 0x0004_0289;

/// A layer number alone also matches draws from nested view handlers.
pub fn scene_layer_call(layer: u32, ret: u32) -> bool {
    matches!((layer, ret), (16, SCENE_FOG_RETURN) | (6, SCENE_WATER_RETURN))
}

/// Copy the 16-bit index list as quads. Bound extraction to the device's
/// vertex limit before allocating, and reject address wrap.
pub fn water_composite(mem: &dyn GuestRead, indices: u32, count: u32) -> Option<frame_scene::late::WaterComposite> {
    if indices == 0 || !(4..=1 << 20).contains(&count) || !count.is_multiple_of(4) {
        return None;
    }
    indices.checked_add(2 * count - 1)?;
    let mut quads = Vec::with_capacity(count as usize / 4);
    for i in 0..count / 4 {
        let at = indices + i * 8;
        let a = mem.read_u32(at)?;
        let b = mem.read_u32(at + 4)?;
        quads.push([a as u16, (a >> 16) as u16, b as u16, (b >> 16) as u16]);
    }
    Some(frame_scene::late::WaterComposite { quads })
}

/// Where the grid call `0x0003ca90` leaves the ocean's rows (`c[50..53]`,
/// the twelve words `0x000151c0` uploads).
pub const OCEAN_ROWS: u32 = 0x004c_1b38;

/// The water end's ocean of `definition` (its shader `shader`, the
/// implementation's flags `flags`, `clear` the end's second argument):
/// the grid's rows and the definition's parameter block upload. `None`
/// when a word does not read or the upload's arithmetic stops.
pub fn ocean(mem: &dyn GuestRead, definition: u32, shader: u32, flags: u32, clear: bool) -> Option<frame_scene::late::Ocean> {
    let mut rows = [[0u32; 4]; 3];
    for (n, w) in rows.iter_mut().flatten().enumerate() {
        *w = mem.read_u32(OCEAN_ROWS + 4 * n as u32)?;
    }
    Some(frame_scene::late::Ocean {
        definition,
        shader,
        rows,
        structure: crate::layout::structure_parameters::of(mem, definition)?,
        clear_alpha: clear,
        copies: flags >> 20 & 1 != 0,
        reflects: flags >> 19 & 1 != 0,
    })
}

/// The current list's items with `layer`'s bit set, when the current
/// layer is `layer`: what the guest's layer draw would draw. `None` when
/// another layer is current or the list does not read.
pub fn layer_items(mem: &dyn GuestRead, layer: u8) -> Option<u32> {
    if layer >= 32 || mem.read_u32(CURRENT_LAYER)? != u32::from(layer) {
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
    let bit = 1u32 << layer;
    let mut n = 0;
    for i in first..count {
        let item = items.checked_add(ITEM_SIZE.checked_mul(i)?)?;
        if mem.read_u32(item.checked_add(0x10)?)? & bit != 0 {
            n += 1;
        }
    }
    Some(n)
}

/// One call of `render_screen_effects` with its stack bytes `late` and
/// `colour`: what it was asked to draw. Its callees' calls are added as
/// they come ([`post_effect`], the colour pass's arguments). `None` when a
/// word it reads cannot be read.
pub fn post(mem: &dyn GuestRead, late: bool, colour: bool) -> Option<PostParams> {
    let f = PostFrame::read(mem, late, colour)?;
    Some(PostParams {
        colour: f.colour,
        late: f.late,
        view_kind: f.view_kind,
        screen_effects: f.screen_effects,
        screen_effects_gated: f.screen_effects_gated,
        colour_pass: None,
        effects: 0..0,
    })
}

/// One call of the screen effect `0x0002d140`: its datum (EAX), the
/// address of its four values and its grid's columns and rows (its stack
/// words). The values are read by value; the address is not kept.
pub fn post_effect(mem: &dyn GuestRead, datum: u32, values: u32, columns: u32, rows: u32) -> PostEffect {
    let read = || {
        let mut v = [0u32; 4];
        for (i, w) in v.iter_mut().enumerate() {
            *w = mem.read_u32(values.checked_add(4 * i as u32)?)?;
        }
        Some(v)
    };
    PostEffect { datum, values: if values == 0 { None } else { read() }, columns, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::post::{SCENARIO, SCENARIO_SCREEN_EFFECTS, VIEW_KIND};
    use crate::testing::Mem;

    #[test]
    fn only_the_matching_layer_belongs_to_each_late_call() {
        for (layer, other, ret) in [(6, 16, SCENE_WATER_RETURN), (16, 6, SCENE_FOG_RETURN)] {
            assert!(scene_layer_call(layer, ret));
            assert!(!scene_layer_call(layer, ret + 1));
            assert!(!scene_layer_call(other, ret));
        }
    }

    #[test]
    fn composite_geometry_copies_unaligned_quads_and_refuses_incomplete_memory() {
        let mut mem = Mem::default();
        mem.w(0x2012, (14 << 16) | 11);
        assert!(water_composite(&mem, 0x2012, 4).is_none());
        mem.w(0x2016, (7 << 16) | 21);
        assert_eq!(water_composite(&mem, 0x2012, 4).unwrap().quads, [[11, 14, 21, 7]]);
        assert!(water_composite(&mem, u32::MAX - 5, 4).is_none());
    }

    #[test]
    fn the_ocean_extract_reads_rows_structure_and_flags() {
        let mut mem = crate::layout::structure_parameters::tests::guest();
        let block = crate::layout::structure_parameters::tests::BLOCK;
        for n in 0..12 {
            mem.w(OCEAN_ROWS + 4 * n, 0x100 + n);
        }
        let o = ocean(&mem, block, 0xe000_0007, 1 << 20, true).expect("every word reads");
        assert_eq!(o.rows, [[0x100, 0x101, 0x102, 0x103], [0x104, 0x105, 0x106, 0x107], [0x108, 0x109, 0x10a, 0x10b]]);
        assert_eq!(o.structure, crate::layout::structure_parameters::of(&mem, block).unwrap());
        assert_eq!(o.structure.constants[4], [3.0f32, 4.0, 5.0, 1.0].map(f32::to_bits));
        assert_eq!((o.definition, o.shader, o.clear_alpha, o.copies, o.reflects), (block, 0xe000_0007, true, true, false));
        let o = ocean(&mem, block, 0, 1 << 19, false).unwrap();
        assert_eq!((o.clear_alpha, o.copies, o.reflects), (false, false, true));
    }

    #[test]
    fn an_unread_definition_is_no_ocean() {
        let mut mem = crate::layout::structure_parameters::tests::guest();
        let block = crate::layout::structure_parameters::tests::BLOCK;
        assert!(ocean(&mem, block, 0, 0, false).is_none(), "the rows do not read");
        for n in 0..12 {
            mem.w(OCEAN_ROWS + 4 * n, 0);
        }
        assert!(ocean(&mem, block, 0, 0, false).is_some());
        assert!(ocean(&mem, 0x0070_0000, 0, 0, false).is_none(), "the block does not read");
    }

    const LIST: u32 = 0x0010_0000;
    const ITEMS: u32 = 0x0020_0000;

    /// A list of four items: two with bit 16, one with bit 6, one with
    /// both.
    fn list(layer: u32) -> Mem {
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, layer);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 4);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        for (i, mask) in [1u32 << 16, 1 << 16, 1 << 6, 1 << 16 | 1 << 6].into_iter().enumerate() {
            m.w(ITEMS + ITEM_SIZE * i as u32 + 0x10, mask);
        }
        m
    }

    #[test]
    fn a_late_layers_items_are_those_with_its_bit_in_the_current_list() {
        assert_eq!(layer_items(&list(16), 16), Some(3));
        assert_eq!(layer_items(&list(6), 6), Some(2));
        assert_eq!(layer_items(&list(6), 16), None, "another layer is current");
        let mut m = list(16);
        m.0.remove(&(ITEMS + ITEM_SIZE * 2 + 0x10));
        assert_eq!(layer_items(&m, 16), None, "an item that does not read");
        m.w(CURRENT_LIST, 0);
        assert_eq!(layer_items(&m, 16), None, "no list");
    }

    #[test]
    fn a_post_call_reads_its_gates_and_a_screen_effect_its_values() {
        let mut m = Mem::default();
        m.w(VIEW_KIND, 1);
        m.w(SCENARIO, 0x0070_0000);
        m.w(0x0070_0000 + SCENARIO_SCREEN_EFFECTS, 2);
        m.w(0x0048_5604, 0);
        m.w(0x0048_50c8, 0);
        let p = post(&m, true, true).unwrap();
        assert_eq!((p.colour, p.late, p.view_kind, p.screen_effects, p.screen_effects_gated), (true, true, 1, 2, true));
        assert_eq!((p.colour_pass, p.effects), (None, 0..0));
        m.0.remove(&VIEW_KIND);
        assert_eq!(post(&m, true, true), None);
        for (i, v) in [5u32, 6, 7, 8].into_iter().enumerate() {
            m.w(0x0080_0000 + 4 * i as u32, v);
        }
        let e = post_effect(&m, 0xe000_0001, 0x0080_0000, 2, 1);
        assert_eq!(e, PostEffect { datum: 0xe000_0001, values: Some([5, 6, 7, 8]), columns: 2, rows: 1 });
        assert_eq!(post_effect(&m, 1, 0x0090_0000, 2, 1).values, None, "values that do not read");
        assert_eq!(post_effect(&m, 1, 0, 2, 1).values, None, "no values");
    }
}
