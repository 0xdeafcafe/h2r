//! The decorators of a layer draw into the open view
//! (docs/graphics/native-decorators.md): each item of the current list in
//! the layer whose build is the model decorators' becomes a
//! [`frame_scene::DecoratorDraw`] with its instances, read by
//! [`crate::layout::decorators::read`]. What does not read is the view's
//! decorator remainder: the guest still draws it. An item whose build
//! declines (`crate::layout::decorators::implementation`) is neither: the
//! guest draws nothing for it.

use frame_scene::{BuildError, SceneBuilder};

use crate::layout::decorators::{Constants, DECORATOR_BUILD, DECORATOR_DRAW, ModelItem, Refusal, model_items, read, read_placement};
use crate::visibility::frusta::WindowFrusta;
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

/// Result of comparing the placement producer with the live list on this view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementSource {
    Rust,
    Different,
    NoWindows,
    Unread,
    NotResident,
    VirtualShader,
}

impl PlacementSource {
    pub const ALL: [Self; 6] = [Self::Rust, Self::Different, Self::NoWindows, Self::Unread, Self::NotResident, Self::VirtualShader];

    pub const fn counter(self) -> &'static str {
        match self {
            Self::Rust => "scene_decorator_placements_equal",
            Self::Different => "scene_decorator_placements_differ",
            Self::NoWindows => "scene_decorator_placements_no_windows",
            Self::Unread => "scene_decorator_placements_unread",
            Self::NotResident => "scene_decorator_placements_nonresident",
            Self::VirtualShader => "scene_decorator_placements_virtual_shader",
        }
    }
}

impl From<Refusal> for PlacementSource {
    fn from(reason: Refusal) -> Self {
        match reason {
            Refusal::GridMismatch => Self::Different,
            Refusal::Unread => Self::Unread,
            Refusal::NotResident => Self::NotResident,
            Refusal::Virtual => Self::VirtualShader,
        }
    }
}

struct List {
    first: u32,
    count: u32,
    items: u32,
}

impl List {
    fn read(mem: &dyn GuestRead) -> Option<Self> {
        let list = mem.read_u32(CURRENT_LIST).filter(|&l| l != 0)?;
        let first = mem.read_u32(list.checked_add(0xc)?)?;
        let count = mem.read_u32(list.checked_add(4)?)?;
        let items = mem.read_u32(list.checked_add(0x14)?)?;
        if first > count || count - first > MAX_ITEMS {
            return None;
        }
        items.checked_add(count.checked_mul(ITEM_SIZE)?)?;
        Some(Self { first, count, items })
    }

    /// The slot is needed only for masking the guest draw. Keys and order
    /// come from the Rust producer; compare the whole layer before taking it.
    fn matched_slots(&self, mem: &dyn GuestRead, layer: u8, candidates: &[ModelItem]) -> Result<Vec<u32>, PlacementSource> {
        let mut slots = Vec::with_capacity(candidates.len());
        for slot in self.first..self.count {
            let item = self.items + ITEM_SIZE * slot;
            if mem.read_u32(item).ok_or(PlacementSource::Unread)? != DECORATOR_BUILD {
                continue;
            }
            let mask = mem.read_u32(item + 0x10).ok_or(PlacementSource::Unread)?;
            if mask & (1 << layer) == 0 {
                continue;
            }
            if !candidates.get(slots.len()).is_some_and(|key| key.matches(mem, item)) {
                return Err(PlacementSource::Different);
            }
            slots.push(slot);
        }
        if slots.len() != candidates.len() {
            return Err(PlacementSource::Different);
        }
        Ok(slots)
    }
}

type DrawResult = Result<Option<(frame_scene::DecoratorDraw, Vec<frame_scene::DecoratorInstance>)>, Refusal>;

fn publish(builder: &mut SceneBuilder, out: &mut Decorators, result: DrawResult) -> Result<(), BuildError> {
    match result {
        Ok(Some((draw, instances))) => {
            builder.push_decorator(draw, &instances)?;
            out.draws += 1;
            out.instances += instances.len() as u32;
        }
        Ok(None) => {}
        Err(_) => out.remainder += 1,
    }
    Ok(())
}

/// Extract using the guest list when the caller has no R4 windows.
pub fn extract(mem: &dyn GuestRead, layer: u8, builder: &mut SceneBuilder) -> Result<Option<Decorators>, BuildError> {
    Ok(extract_with_windows(mem, layer, None, builder)?.map(|(out, _)| out))
}

/// Produce the layer's model placements from the visible cells and the
/// reference portal windows. Compare shader, pass, detail level, mask,
/// callback, packed argument and order. An unmatched view retains the guest list.
pub fn extract_with_windows(
    mem: &dyn GuestRead, layer: u8, windows: Option<&WindowFrusta>, builder: &mut SceneBuilder,
) -> Result<Option<(Decorators, PlacementSource)>, BuildError> {
    if layer >= 32 {
        return Ok(None);
    }
    let (Some(list), Some(k)) = (List::read(mem), Constants::read(mem)) else {
        return Ok(None);
    };
    let selected = (|| {
        let windows = windows.ok_or(PlacementSource::NoWindows)?;
        let camera = builder.view().ok_or(PlacementSource::Unread)?.camera;
        let candidates = model_items(mem, &k, camera.position, windows, layer)?;
        let slots = list.matched_slots(mem, layer, &candidates)?;
        Ok(candidates.into_iter().zip(slots))
    })();
    let mut out = Decorators::default();
    let source = match selected {
        Ok(entries) => {
            for (key, slot) in entries {
                publish(builder, &mut out, read_placement(mem, &k, slot, layer, key.shader, key.argument, key.pass, key.variant, Some(key.origin)))?;
            }
            PlacementSource::Rust
        }
        Err(source) => {
            for slot in list.first..list.count {
                let item = list.items + ITEM_SIZE * slot;
                if mem.read_u32(item) != Some(DECORATOR_BUILD) {
                    continue;
                }
                match mem.read_u32(item + 0x10) {
                    Some(mask) if mask & (1 << layer) == 0 => continue,
                    Some(_) if mem.read_u32(item + 4) == Some(DECORATOR_DRAW) => {
                        publish(builder, &mut out, read(mem, &k, item, slot, layer))?;
                    }
                    Some(_) => out.remainder += 1,
                    None => out.remainder += 1,
                }
            }
            source
        }
    };
    builder.add_decorator_remainder(out.remainder)?;
    Ok(Some((out, source)))
}

/// Clears `layer`'s bit on the current list's items at `slots` (a
/// decorator plan's, in the list's order): the skip of what the native
/// stage draws. All or nothing: `None` when the current layer is not
/// `layer`, the list does not read, or a slot is not a model decorator
/// item with the bit set; then nothing is written, so the caller writes
/// through a preview (`skip::MaskPreview`) and applies it only on `Some`.
/// Each skipped item's build stamps (`layout::decorators::build_stamps`)
/// are written with its mask, so the geometry cache keeps the geometry
/// the native stage draws as the guest's build would; an item whose
/// stamps do not read, or whose geometry is no longer resident, refuses
/// the whole skip.
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
        if mem.read_u32(item)? != DECORATOR_BUILD || mem.read_u32(item + 4)? != DECORATOR_DRAW || mask & bit == 0 {
            return None;
        }
        masks.push((item, mask));
    }
    let mut stamps = Vec::with_capacity(2 * masks.len());
    for &(item, _) in &masks {
        stamps.extend(crate::layout::decorators::build_stamps(&*mem, item).ok()?);
    }
    let mut out = Cleared { layer, items: Vec::with_capacity(masks.len()) };
    for (item, mask) in masks {
        mem.write_u32(item + 0x10, mask & !bit)?;
        out.items.push(item);
    }
    for (at, stamp) in stamps {
        mem.write_u32(at, stamp)?;
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

    /// The first grid cell is empty. The second holds one model group in
    /// BSP cluster 7. The sphere meets its portal window and range limit.
    fn placement_inputs(m: &mut Mem) -> WindowFrusta {
        use crate::visibility::frusta::Frustum;
        let cells = 0x0068_0000;
        let shader_refs = 0x0068_1000;
        m.w(0x004b_a024, 1 << 8);
        m.w(0x0050_9438, 2);
        m.w(VISIBLE_CLUSTERS, 1);
        m.w(VISIBLE_CLUSTERS + 0x10, 2);
        for (axis, centre) in [101.0, 201.0, 300.0].into_iter().enumerate() {
            m.f(BLOCK + 4 * axis as u32, centre);
        }
        for (axis, pos) in [100.5, 200.5, 299.5].into_iter().enumerate() {
            m.f(VISIBLE_CLUSTERS + 4 + 4 * axis as u32, pos);
        }
        m.w(BLOCK + 0xc, 2);
        m.w(BLOCK + 0x20, 3);
        m.w(BLOCK + 0x24, cells);
        m.w(cells, 2 << 16 | 1);
        for off in [4, 8, 12] { m.w(cells + off, u32::MAX); }
        m.w(cells + 0x18 + 0x10, 0);
        m.w(cells + 2 * 0x18 + 0x10, 1 << 16);
        m.w(cells + 2 * 0x18 + 0x14, 2);
        let frustum = 0x0068_2000;
        m.w(engine::render_loop::occlusion::guest::MAIN_VIS, frustum);
        for (i, v) in [-500.0, 500.0, -500.0, 500.0, -500.0, 500.0].into_iter().enumerate() {
            m.f(frustum + 0xf4 + i as u32 * 4, v);
        }
        for i in 0..24 { m.f(frustum + 0x10c + i * 4, 0.0); }
        m.f(0x0052_5930, 500.0);
        m.f(0x0052_5924, 0.5);
        m.f(0x0045_dbdc, 0.0001);
        m.f(0x0045_ddfc, -0.0001);
        m.f(0x0043_f9d0, 2.0);
        m.w(BLOCK + BLOCK_PLACEMENTS - 4, 3);
        m.w(BLOCK + BLOCK_GEOMETRY - 4, 1);
        m.w(SCENARIO_DATA + SCENARIO_PALETTE - 4, 2);
        m.w(SET + SET_CLASSES - 4, 1);
        m.w(CLASSES + 4, 0);
        m.w(CLASSES + CLASS_PERMUTATIONS - 4, 2);
        m.w(PERMUTATIONS + 4, 0);
        m.w(PERMUTATIONS + 8, 1 << 8);
        m.w(SET, 1);
        m.w(SET + 4, shader_refs);
        m.w(shader_refs + 4, 0xe500_0002);
        m.w(PLACEMENTS + 2 * PLACEMENT_SIZE + 4, 7);
        m.w(PLACEMENTS + 2 * PLACEMENT_SIZE + 0x14, 0);
        m.f(0x0045_e4d0, 0.5);
        m.f(0x0052_5980 + 4, 500.0);
        m.f(0x0004_1925, 8.0);
        m.f(0x0048_5b28, 1.0);
        m.w(0x004b_a018, 0);
        let pp = 0x0066_0300;
        let lods = 0x0066_0400;
        m.w(pp, 4);
        for lod in 0..3 {
            m.f(lods + LOD_SIZE * lod + 6, 10.0);
        }
        m.w(lods + LOD_SIZE * 3 + 2, 1 << 3);
        WindowFrusta(vec![(7, Frustum { bounds: [-500.0, 500.0, -500.0, 500.0, -500.0, 500.0], planes: [[0.0; 4]; 6] })])
    }

    #[test]
    fn model_placement_uses_the_cell_group_window_and_shader_level() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        let windows = placement_inputs(&mut m);
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = extract_with_windows(&m, 3, Some(&windows), &mut b).unwrap().unwrap();
        assert_eq!(got, (Decorators { draws: 1, instances: 2, remainder: 0 }, PlacementSource::Rust));
        let scene = b.seal(Stamp::default()).unwrap();
        let draw = &scene.decorators_of(&scene.views[0])[0];
        assert_eq!((draw.slot, draw.argument, draw.variant), (1, 1 << 30 | 2 << 8 | 1, 3));
        assert_eq!(scene.decorator_instances_of(draw)[1].position(), [103.0, 200.0, 300.0]);

        for windows in [WindowFrusta::default(), windows.clone()] {
            // A sphere beyond the LOD limit or without a reachable window
            // is culled even though the guest fixture still lists it.
            m.f(0x0052_5980 + 4, 1.0);
            let mut b = SceneBuilder::default();
            view(&mut b, &m);
            let (out, source) = extract_with_windows(&m, 3, Some(&windows), &mut b).unwrap().unwrap();
            assert_eq!((source, out.draws), (PlacementSource::Different, 1));
        }
    }

    #[test]
    fn the_cell_walk_preserves_order_and_origin_bits_and_rejects_a_touching_clip_plane() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        let windows = placement_inputs(&mut m);
        let source = |m: &Mem| {
            let mut b = SceneBuilder::default();
            view(&mut b, m);
            extract_with_windows(m, 3, Some(&windows), &mut b).unwrap().unwrap().1
        };
        m.w(VISIBLE_CLUSTERS, 2);
        assert_eq!(source(&m), PlacementSource::Different, "the first slot addresses another leaf");
        m.w(VISIBLE_CLUSTERS, 1);
        m.w(VISIBLE_CLUSTERS + 4, 100.5f32.to_bits() + 1);
        assert_eq!(source(&m), PlacementSource::Different, "one bit of the first origin differs");
        // X distance to the plane is exactly the leaf extent times the
        // plane scale. Both leaves are rejected; there are no model items.
        let plane = 0x0068_2000 + 0x10c;
        m.f(plane, 1.0);
        m.f(plane + 12, 99.5);
        m.w(0x0050_9438, 0);
        m.w(LIST + 4, 1);
        assert_eq!(source(&m), PlacementSource::Rust);
    }

    #[test]
    fn a_duplicate_guest_slot_or_different_callback_rejects_the_whole_placement_list() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        let windows = placement_inputs(&mut m);
        for (mask, draw, draws) in [(1 << 3, DECORATOR_DRAW, 2), (1 << 15, DECORATOR_DRAW + 4, 0)] {
            m.w(ITEMS + 2 * ITEM_SIZE + 0x10, mask);
            m.w(ITEMS + ITEM_SIZE + 4, draw);
            let mut b = SceneBuilder::default();
            view(&mut b, &m);
            let (out, source) = extract_with_windows(&m, 3, Some(&windows), &mut b).unwrap().unwrap();
            assert_eq!((source, out.draws), (PlacementSource::Different, draws));
        }
    }

    const CACHE: u32 = 0x0065_0000;
    const CACHE_DATA_AT: u32 = 0x0065_1000;
    const CACHE_TABLE: u32 = 0x0066_0000;

    /// The geometry cache: the frame's stamp 77; the placement's geometry
    /// record in entry 3, the set's geometry in entry 5, both stamped 1.
    fn cache(m: &mut Mem) {
        m.w(GEOMETRY_CACHE, CACHE);
        m.w(CACHE + CACHE_STAMP, 77);
        m.w(CACHE + CACHE_DATA, CACHE_DATA_AT);
        m.w(CACHE_DATA_AT + CACHE_ENTRIES, CACHE_TABLE);
        m.w(GEOMETRY + INFO_ENTRY, 3);
        m.w(SET + SET_GEOMETRY + INFO_ENTRY, 5);
        for entry in [3, 5] {
            m.w(CACHE_TABLE + CACHE_ENTRY_SIZE * entry + CACHE_ENTRY_STAMP, 1);
        }
    }

    #[test]
    fn the_skip_clears_the_planned_slots_or_nothing() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        cache(&mut m);
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

    /// A skipped item's build stamps the cache entries of its placement's
    /// geometry record and its set's geometry, as `0x0012de70` does for
    /// resident blocks; geometry no longer resident refuses the skip.
    #[test]
    fn the_skip_stamps_the_geometry_the_build_would_have_asked_for() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        world(&mut m);
        cache(&mut m);
        m.w(CURRENT_LAYER, 3);
        let stamp = |m: &Mem, entry: u32| m.read_u32(CACHE_TABLE + CACHE_ENTRY_SIZE * entry + CACHE_ENTRY_STAMP).unwrap();
        assert_eq!(
            build_stamps(&m, ITEMS + ITEM_SIZE),
            Ok([(CACHE_TABLE + 3 * CACHE_ENTRY_SIZE + CACHE_ENTRY_STAMP, 77), (CACHE_TABLE + 5 * CACHE_ENTRY_SIZE + CACHE_ENTRY_STAMP, 77)])
        );
        m.w(SET + SET_GEOMETRY + 0x1c, 0);
        assert_eq!(clear_slots(&mut m, 3, &[1]), None, "the set's geometry is not resident");
        assert_eq!((stamp(&m, 3), stamp(&m, 5)), (1, 1));
        m.w(SET + SET_GEOMETRY + 0x1c, 1 << 16);
        assert!(clear_slots(&mut m, 3, &[1]).is_some());
        assert_eq!((stamp(&m, 3), stamp(&m, 5)), (77, 77));
    }
}
