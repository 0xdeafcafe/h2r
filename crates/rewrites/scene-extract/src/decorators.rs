//! The decorators of a layer draw into the open view
//! (docs/graphics/native-decorators.md): each item of the current list in
//! the layer whose build is the model decorators' becomes a
//! [`frame_scene::DecoratorDraw`] with its instances, read by
//! [`crate::layout::decorators::read`]. What does not read is the view's
//! decorator remainder: the guest still draws it. An item whose build
//! declines (`crate::layout::decorators::implementation`) is neither: the
//! guest draws nothing for it.

use frame_scene::{BuildError, SceneBuilder};

use crate::layout::decorators::{Constants, DECORATOR_BUILD, read};
use crate::layout::skip::{CURRENT_LAYER, CURRENT_LIST, Cleared, ITEM_SIZE, MAX_ITEMS};
use crate::{GuestRead, GuestWrite};

/// What [`extract`] put in the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decorators {
    pub draws: u32,
    pub instances: u32,
    /// Draws left to the guest: memory that does not read, geometry the
    /// cache does not hold yet.
    pub remainder: u32,
}

/// The current list's model decorator items in `layer`, in the list's
/// order, into the open view. `Ok(None)`, nothing written, when the list
/// or the draw's constants do not read.
pub fn extract(mem: &dyn GuestRead, layer: u8, builder: &mut SceneBuilder) -> Result<Option<Decorators>, BuildError> {
    if layer >= 32 {
        return Ok(None);
    }
    let Some(list) = mem.read_u32(CURRENT_LIST).filter(|&l| l != 0) else {
        return Ok(None);
    };
    let (Some(count), Some(first), Some(items)) =
        (mem.read_u32(list.wrapping_add(4)), mem.read_u32(list.wrapping_add(0xc)), mem.read_u32(list.wrapping_add(0x14)))
    else {
        return Ok(None);
    };
    let Some(k) = Constants::read(mem) else {
        return Ok(None);
    };
    let bit = 1u32 << layer;
    let mut out = Decorators::default();
    for slot in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let Some(item) = ITEM_SIZE.checked_mul(slot).and_then(|o| items.checked_add(o)) else {
            break;
        };
        if mem.read_u32(item) != Some(DECORATOR_BUILD) {
            continue;
        }
        match mem.read_u32(item.wrapping_add(0x10)) {
            Some(mask) if mask & bit == 0 => continue,
            Some(_) => {}
            None => {
                out.remainder += 1;
                continue;
            }
        }
        match read(mem, &k, item, slot, layer) {
            Ok(Some((draw, instances))) => {
                out.draws += 1;
                out.instances += instances.len() as u32;
                builder.push_decorator(draw, &instances)?;
            }
            Ok(None) => {}
            Err(_) => out.remainder += 1,
        }
    }
    builder.add_decorator_remainder(out.remainder)?;
    Ok(Some(out))
}

/// Clears `layer`'s bit on the current list's items at `slots` (a
/// decorator plan's, in the list's order): the skip of what the native
/// stage draws. All or nothing: `None` when the current layer is not
/// `layer`, the list does not read, or a slot is not a model decorator
/// item with the bit set; then nothing is written, so the caller writes
/// through a preview (`skip::MaskPreview`) and applies it only on `Some`.
pub fn clear_slots(mem: &mut dyn GuestWrite, layer: u32, slots: &[u32]) -> Option<Cleared> {
    if layer >= 32 || mem.read_u32(CURRENT_LAYER)? != layer {
        return None;
    }
    let list = mem.read_u32(CURRENT_LIST).filter(|&l| l != 0)?;
    let count = mem.read_u32(list.checked_add(4)?)?;
    let first = mem.read_u32(list.checked_add(0xc)?)?;
    let items = mem.read_u32(list.checked_add(0x14)?)?;
    let bit = 1u32 << layer;
    let mut masks = Vec::with_capacity(slots.len());
    for &slot in slots {
        if slot < first || slot >= count || slot - first >= MAX_ITEMS {
            return None;
        }
        let item = items.checked_add(ITEM_SIZE.checked_mul(slot)?)?;
        let mask = mem.read_u32(item.checked_add(0x10)?)?;
        if mem.read_u32(item)? != DECORATOR_BUILD || mask & bit == 0 {
            return None;
        }
        masks.push((item, mask));
    }
    let mut out = Cleared { layer, items: Vec::with_capacity(masks.len()) };
    for (item, mask) in masks {
        mem.write_u32(item + 0x10, mask & !bit)?;
        out.items.push(item);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use frame_scene::{SceneId, Stamp, ViewKey, ViewKind};

    use super::*;
    use crate::extract::camera;
    use crate::layout::camera::RENDER_CAMERA;
    use crate::layout::decorators::tests::{constants, put_constants, put_implementation};
    use crate::layout::decorators::*;
    use crate::testing::{Mem, camera_record};

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;
    const BSP: u32 = 0x0061_0000;
    const BLOCK: u32 = 0x0061_1000;
    const PLACEMENTS: u32 = 0x0061_2000;
    const GEOMETRY: u32 = 0x0061_3000;
    const INSTANCE_BLOCK: u32 = 0x0061_4000;
    const INSTANCES: u32 = 0x0061_5000;
    const SCENARIO_DATA: u32 = 0x0062_0000;
    const PALETTE: u32 = 0x0062_1000;
    const TAGS: u32 = 0x0063_0000;
    const SET: u32 = 0x0064_0000;
    const CLASSES: u32 = 0x0064_1000;
    const PERMUTATIONS: u32 = 0x0064_2000;
    const MODELS: u32 = 0x0064_3000;
    const SET_DATUM: u32 = 0xe400_0005;

    /// One BSP decorator block with placement 2 (palette 1, geometry
    /// record 0, instances 0..4 of which 3 is past the block), a set
    /// with two models (model 1 has no indices), one class with two
    /// permutations (model 0 and model 1), and visible cluster 1 at
    /// (100.5, 200.5, 300.5).
    fn world(m: &mut Mem) {
        put_constants(m, &constants());
        m.w(ACTIVE_BSP, BSP);
        m.w(BSP + BSP_DECORATORS, BLOCK);
        m.w(BLOCK + BLOCK_PLACEMENTS, PLACEMENTS);
        m.w(BLOCK + BLOCK_GEOMETRY, GEOMETRY);
        let p = PLACEMENTS + 2 * PLACEMENT_SIZE;
        m.w(p, 1); // palette 1
        m.w(p + 4, 0); // geometry record 0 in the high half
        m.w(p + 8, 4 << 16); // first 0, count 4
        m.w(GEOMETRY + 0x1c, 1 << 16); // resident byte +0x1e
        m.w(GEOMETRY + GEOMETRY_INSTANCES, INSTANCE_BLOCK);
        m.w(INSTANCE_BLOCK, 3);
        m.w(INSTANCE_BLOCK + 4, INSTANCES);
        m.w(SCENARIO, SCENARIO_DATA);
        m.w(SCENARIO_DATA + SCENARIO_PALETTE, PALETTE);
        m.w(PALETTE + 8 + 4, SET_DATUM);
        m.w(TAG_TABLE, TAGS);
        m.w(TAGS + 0x10 * 5 + 8, SET);
        // The items' shader (tag 2) at detail level 3, layer 3, pass 0.
        put_implementation(m, 2, 3, 3, 0, 0x40);
        m.f(SET + SET_FADE, 0.0);
        m.f(SET + SET_FADE + 4, 1.0);
        m.w(SET + SET_CLASSES, CLASSES);
        m.w(SET + SET_MODEL_COUNT, 2);
        m.w(SET + SET_MODELS, MODELS);
        m.w(SET + SET_GEOMETRY + 0x1c, 1 << 16);
        m.w(MODELS + 4, 36 << 16 | 6); // model 0: first 6, 36 indices
        m.w(MODELS + 8 + 4, 9); // model 1: first 9, none
        m.w(CLASSES + CLASS_PERMUTATIONS, PERMUTATIONS);
        for (n, model) in [(0u32, 0u32), (1, 1)] {
            let perm = PERMUTATIONS + PERMUTATION_SIZE * n;
            m.w(perm + 8, model << 16);
            m.f(perm + 0xc, 1.0);
            m.f(perm + 0x10, 1.0);
            m.w(perm + 0x14, 0x00ff_ffff);
            m.w(perm + 0x18, 0x00ff_ffff);
            m.f(perm + 0x1c, 1.0);
            m.f(perm + 0x20, 1.0);
        }
        // Instances 0 and 2 draw model 0, instance 1 permutation 1 (model
        // 1, no indices).
        for (n, packed, x) in [(0u32, 0u32, 16u32), (1, 1 << 3, 32), (2, 0, 48)] {
            let at = INSTANCES + INSTANCE_SIZE * n;
            m.w(at, packed);
            m.w(at + 4, x);
            m.w(at + 8, 0x00ff_ffff);
            m.w(at + 0xc, 0x00ff_ffff);
            m.w(at + 0x10, 0);
            m.f(at + 0x14, 0.5);
        }
        for (c, v) in [100.5f32, 200.5, 300.5].into_iter().enumerate() {
            m.f(VISIBLE_CLUSTERS + 0x10 + 4 + 4 * c as u32, v);
        }
        for (lod, (a, b)) in [(0u32, (1.0f32, 2.0f32)), (1, (3.0, 4.0))] {
            m.f(LOD_FACTORS[0] + 4 * lod, a);
            m.f(LOD_FACTORS[1] + 4 * lod, b);
        }
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 3);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        // Slot 0 another build, slot 1 placement 2 at detail level 1 in
        // cluster slot 1, slot 2 placement 2 outside layer 3.
        for (slot, build, mask) in [(0u32, 0x0004_cbf0u32, 1u32 << 3), (1, DECORATOR_BUILD, 1 << 3), (2, DECORATOR_BUILD, 1 << 15)] {
            let at = ITEMS + ITEM_SIZE * slot;
            m.w(at, build);
            m.w(at + 4, DECORATOR_DRAW);
            m.w(at + 8, 0xe500_0002);
            m.w(at + 0xc, 1 << 30 | 2 << 8 | 1);
            m.w(at + 0x10, mask);
            m.w(at + 0x14, 0x0300);
        }
    }

    fn view(b: &mut SceneBuilder, m: &Mem) {
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera(m).unwrap()).unwrap();
    }

    #[test]
    fn a_layers_decorators_are_in_the_scene_with_their_instances_and_registers() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = extract(&m, 3, &mut b).unwrap().unwrap();
        assert_eq!(got, Decorators { draws: 1, instances: 2, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let draws = s.decorators_of(v);
        assert_eq!(draws.len(), 1);
        let d = &draws[0];
        assert_eq!((d.slot, d.layer, d.set, d.shader, d.variant, d.pass), (1, 3, SET_DATUM, 0xe500_0002, 3, 0));
        assert_eq!((d.placement(), d.lod()), (2, 1));
        assert_eq!(d.lod_constant, [3.0, 4.0, 0.0, 1.0]);
        let instances = s.decorator_instances_of(d);
        // Instance 1 names a model with no indices, 3 is past the block.
        assert_eq!(instances.iter().map(|i| i.index).collect::<Vec<_>>(), [0, 2]);
        assert!(instances.iter().all(|i| (i.model, i.first_index, i.index_count) == (0, 6, 36)));
        // x 16/16 = 1 and 48/16 = 3 from the cluster's origin less the bias.
        assert_eq!(instances[0].position(), [101.0, 200.0, 300.0]);
        assert_eq!(instances[1].position()[0], 103.0);
        assert_eq!(instances[0].fade(), 0.5);
        assert_eq!(s.decorator_counts(v).remainder, 0);
    }

    #[test]
    fn what_does_not_read_or_is_not_resident_is_a_remainder_and_layer_1_takes_nothing() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        assert_eq!(extract(&m, 1, &mut b).unwrap(), Some(Decorators::default()), "no decorator item in layer 1");
        m.w(GEOMETRY + 0x1c, 0);
        assert_eq!(extract(&m, 3, &mut b).unwrap(), Some(Decorators { draws: 0, instances: 0, remainder: 1 }));
        m.w(GEOMETRY + 0x1c, 1 << 16);
        m.0.remove(&(TAGS + 0x10 * 5 + 8));
        assert_eq!(extract(&m, 3, &mut b).unwrap(), Some(Decorators { draws: 0, instances: 0, remainder: 1 }));
        // A shader whose build declines draws nothing: neither a draw nor
        // a remainder.
        m.w(TAGS + 0x10 * 5 + 8, SET);
        put_implementation(&mut m, 2, 3, 3, 0, 0);
        assert_eq!(extract(&m, 3, &mut b).unwrap(), Some(Decorators::default()));
        let s = b.seal(Stamp::default()).unwrap();
        assert_eq!(s.decorator_counts(&s.views[0]).remainder, 2);
        m.w(CURRENT_LIST, 0);
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        assert_eq!(extract(&m, 3, &mut b).unwrap(), None, "no list, nothing written");
    }

    #[test]
    fn the_skip_clears_the_planned_slots_or_nothing() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        m.w(CURRENT_LAYER, 3);
        let mask = |m: &Mem, slot: u32| m.read_u32(ITEMS + ITEM_SIZE * slot + 0x10).unwrap();
        // Slot 0 is another build, slot 2 lacks the bit: nothing written.
        assert_eq!(clear_slots(&mut m, 3, &[0]), None);
        assert_eq!(clear_slots(&mut m, 3, &[1, 2]), None);
        assert_eq!(clear_slots(&mut m, 3, &[1, 3]), None, "past the list");
        assert_eq!(mask(&m, 1), 1 << 3);
        assert_eq!(clear_slots(&mut m, 5, &[1]), None, "another layer is current");
        let got = clear_slots(&mut m, 3, &[1]).unwrap();
        assert_eq!(got, Cleared { layer: 3, items: vec![ITEMS + ITEM_SIZE] });
        assert_eq!(mask(&m, 1), 0);
        assert_eq!(clear_slots(&mut m, 3, &[1]), None, "already cleared");
    }
}
