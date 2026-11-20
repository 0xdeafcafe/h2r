//! Guest memory to a view (docs/graphics/framescene.md, section 4.3):
//! slice 1 extracts the camera, the visible clusters and the world state
//! at a view's base layer 3, and the objects at the object walk, from the
//! same reads the stages made there before. What extraction cannot read
//! is a remainder: the guest still draws it, and it is not in the scene.

use frame_scene::{BuildError, Camera, InstanceInput, InstanceKey, Owner, PartFlags, SceneBuilder, WorldState};

use crate::GuestRead;
use crate::layout::aux_depth;
use crate::layout::camera::{self, RENDER_CAMERA};
use crate::layout::object_nodes::texaccum_bound;
use crate::layout::object_walk::Entry;
use crate::layout::objects::read_object;
use crate::layout::skip::{CURRENT_LIST, ITEM_SIZE, MAX_ITEMS, is_structure_item, structure_part};

/// The view's camera: the render camera record, as the world and object
/// stages read it at layer 3.
pub fn camera(mem: &dyn GuestRead) -> Option<Camera> {
    camera::read(mem, RENDER_CAMERA)
}

/// The world state the typed stages read for the view.
pub fn world(mem: &dyn GuestRead) -> WorldState {
    WorldState { texaccum: texaccum_bound(mem) }
}

/// The clusters the current list's `layer` items draw, each once, in the
/// list's order: the clusters the guest found visible for the view.
/// Instanced-geometry placements are not clusters and are left out.
/// `None` when the list does not read.
pub fn layer_clusters(mem: &dyn GuestRead, layer: u32) -> Option<Vec<u16>> {
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 || layer >= 32 {
        return None;
    }
    let count = mem.read_u32(list.checked_add(4)?)?;
    let first = mem.read_u32(list.checked_add(0xc)?)?;
    let items = mem.read_u32(list.checked_add(0x14)?)?;
    let bit = 1u32 << layer;
    let mut out: Vec<u16> = Vec::new();
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items.checked_add(ITEM_SIZE.checked_mul(i)?)?;
        if mem.read_u32(item.checked_add(0x10)?)? & bit == 0 {
            continue;
        }
        if is_structure_item(mem, item) == Some(true)
            && let Some((Owner::Cluster(c), _)) = structure_part(mem, item)
            && let Ok(c) = u16::try_from(c)
            && !out.contains(&c)
        {
            out.push(c);
        }
    }
    Some(out)
}

/// What [`objects`] put in the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Objects {
    pub instances: u32,
    pub parts: u32,
    /// Object items left to the guest: no part the walk can draw, an
    /// object that does not read, or rows that do not read.
    pub remainder: u32,
}

/// The objects of a sorted layer-3 walk (`entries`, indexed by slot) into
/// the builder's open view: one instance per object that reads, in the
/// order its first item comes, with its parts in slot order and each
/// part's palette rows. The model is the first part's render model. At
/// the first part with [`PartFlags::AUX_DEPTH`] the view takes what the
/// auxiliary state toggle sets ([`aux_depth::read`]); when that does not
/// read, such parts are a remainder.
pub fn objects(mem: &dyn GuestRead, entries: &[Entry], builder: &mut SceneBuilder) -> Result<Objects, BuildError> {
    // Objects in first-item order, each with its items' slots.
    let mut order: Vec<(u32, Vec<usize>)> = Vec::new();
    for (slot, e) in entries.iter().enumerate() {
        if let Entry::Object { handle, .. } = *e {
            match order.iter_mut().find(|(h, _)| *h == handle) {
                Some((_, slots)) => slots.push(slot),
                None => order.push((handle, vec![slot])),
            }
        }
    }
    let mut out = Objects::default();
    // Whether the view's auxiliary depth is set, once asked.
    let mut aux = None;
    for (handle, slots) in order {
        let Some(frame) = read_object(mem, handle) else {
            out.remainder += slots.len() as u32;
            continue;
        };
        let model = slots
            .iter()
            .find_map(|&s| match entries[s] {
                Entry::Object { part: Some(p), .. } => Some(p.mode),
                _ => None,
            })
            .unwrap_or(u32::MAX);
        let instance = builder.push_instance(InstanceInput {
            key: InstanceKey(handle),
            definition: frame.definition,
            model,
            nodes: &frame.nodes,
            sphere: frame.sphere,
            lighting: frame.lighting,
            defaults: frame.defaults,
        })?;
        out.instances += 1;
        for slot in slots {
            let Entry::Object { part: Some(part), .. } = entries[slot] else {
                out.remainder += 1;
                continue;
            };
            let Some(rows) = part.rows(mem) else {
                out.remainder += 1;
                continue;
            };
            if part.flags.contains(PartFlags::AUX_DEPTH) {
                if aux.is_none() {
                    let read = aux_depth::read(mem);
                    if let Some(a) = read {
                        builder.set_aux_depth(a)?;
                    }
                    aux = Some(read.is_some());
                }
                if aux == Some(false) {
                    out.remainder += 1;
                    continue;
                }
            }
            builder.push_part_flagged(instance, slot as u32, part.key, 3, part.flags, part.subparts, &rows)?;
            out.parts += 1;
        }
    }
    builder.add_remainder(out.remainder)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use frame_scene::{ItemKey, NODE_MATRIX_SIZE, PartFlags, SceneId, Stamp, Subparts, ViewKey, ViewKind};

    use super::*;
    use crate::layout::object_nodes::{RECORD_MATRIX, RECORD_NODES};
    use crate::layout::object_walk::Part;
    use crate::layout::skip::{GEOMETRY_BUILD, RENDER_RECORD_SIZE, RENDER_RECORDS};
    use crate::testing::{Mem, camera_record, lighting, object, record};

    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;
    const MATRIX: u32 = 0x0068_0000;

    fn key(record: u16, part: u8) -> ItemKey {
        ItemKey { record, shader: 0xe000_0010, section: 0, part, implementation: 0 }
    }

    fn part(record: u16, slot: u8) -> Part {
        Part {
            key: key(record, slot),
            mode: 0xe100_0000 | u32::from(record),
            program: 0,
            vertex_shader: 0,
            geometry_class: 0,
            node_influences: 0,
            triangle_list: true,
            first_index: 0,
            index_count: 3,
            flags: PartFlags::NONE,
            subparts: None,
        }
    }

    /// Render record `record` lit by `lighting` whose draw uploads its
    /// single matrix, the 13 words of a node matrix (scale, then forward,
    /// left, up, position): `x, x+1, …`.
    fn object_record(m: &mut Mem, record: u32, lighting: Option<u32>, x: f32) {
        crate::testing::record(m, record, lighting);
        let at = RENDER_RECORDS + RENDER_RECORD_SIZE * record;
        let matrix = MATRIX + 0x40 * record;
        m.w(at + RECORD_MATRIX, matrix);
        m.w(at + RECORD_NODES, u32::MAX);
        for i in 0..NODE_MATRIX_SIZE as u32 / 4 {
            m.f(matrix + 4 * i, x + i as f32);
        }
    }

    fn view(b: &mut SceneBuilder, m: &Mem) {
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera(m).unwrap()).unwrap();
    }

    #[test]
    fn a_synthetic_view_extracts_its_camera_instances_parts_and_rows() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let a = object(&mut m, 1, 0xe000_0100, 3);
        let c = object(&mut m, 2, 0xe000_0200, 1);
        object_record(&mut m, 4, Some(lighting(8)), 10.0);
        object_record(&mut m, 5, Some(lighting(8)), 20.0);
        object_record(&mut m, 6, Some(lighting(9)), 30.0);
        // Slots: A, other, B, A (A's parts not next to each other).
        let entries = [
            Entry::Object { handle: a, part: Some(part(4, 0)) },
            Entry::Other,
            Entry::Object { handle: c, part: Some(part(6, 2)) },
            Entry::Object { handle: a, part: Some(part(5, 3)) },
            Entry::Excluded,
        ];
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = objects(&m, &entries, &mut b).unwrap();
        assert_eq!(got, Objects { instances: 2, parts: 3, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        assert_eq!(v.camera.position, [1.0, 2.0, 3.0]);
        let instances = s.instances_of(v);
        assert_eq!(instances.iter().map(|i| i.key).collect::<Vec<_>>(), [InstanceKey(a), InstanceKey(c)]);
        assert_eq!(instances[0].definition, 0xe000_0100);
        assert_eq!(instances[0].model, 0xe100_0004, "the first part's render model");
        assert_eq!(s.nodes_of(&instances[0]).len(), 3);
        assert_eq!(s.nodes_of(&instances[0])[2].position, [2.0, 0.0, 0.0]);
        assert!(instances[0].defaults.is_none(), "no defaults in this memory");
        let slots: Vec<u32> = s.parts_of(&instances[0]).iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 3]);
        let (owner, p) = s.part_at(v, 3).unwrap();
        assert_eq!(owner.key, InstanceKey(a));
        assert_eq!(p.key, key(5, 3));
        // The single matrix transposed into three rows, without its scale.
        let rows = s.rows_of(p);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][3], 20.0 + 10.0, "row 0 ends with the position's x");
        let (owner, p) = s.part_at(v, 2).unwrap();
        assert_eq!((owner.key, p.key.part), (InstanceKey(c), 2));
        assert!(s.part_at(v, 1).is_none() && s.part_at(v, 4).is_none());
    }

    #[test]
    fn a_marked_or_strip_part_is_in_the_scene_with_its_flags() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let a = object(&mut m, 1, 0xe000_0100, 1);
        object_record(&mut m, 4, Some(lighting(8)), 10.0);
        object_record(&mut m, 5, Some(lighting(8)), 20.0);
        let marked = Part { flags: PartFlags::STENCIL_MARK, ..part(4, 0) };
        let strip = Part { flags: PartFlags::STRIP, triangle_list: false, ..part(5, 1) };
        let entries = [
            Entry::Object { handle: a, part: Some(marked) },
            Entry::Object { handle: a, part: Some(strip) },
        ];
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = objects(&m, &entries, &mut b).unwrap();
        assert_eq!(got, Objects { instances: 1, parts: 2, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        let v = &s.views[0];
        assert_eq!(s.part_at(v, 0).unwrap().1.flags, PartFlags::STENCIL_MARK);
        assert_eq!(s.part_at(v, 1).unwrap().1.flags, PartFlags::STRIP);
        assert_eq!(v.remainder, 0, "neither is left to the guest");
    }

    #[test]
    fn an_auxiliary_depth_part_is_in_the_scene_and_its_view_has_what_the_toggle_sets() {
        use crate::layout::aux_depth::{AUX_SCALE, CLIP_FAR, CLIP_NEAR, ONE, SCALE_DENOMINATOR, SCALE_NUMERATOR, VIEW};
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let a = object(&mut m, 1, 0xe000_0100, 1);
        object_record(&mut m, 4, Some(lighting(8)), 10.0);
        object_record(&mut m, 5, Some(lighting(8)), 20.0);
        let aux = Part { flags: PartFlags::AUX_DEPTH, ..part(4, 0) };
        let plain = part(5, 1);
        let entries = [Entry::Object { handle: a, part: Some(aux) }, Entry::Object { handle: a, part: Some(plain) }];
        // What the toggle reads does not read: the part is left to the guest.
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        assert_eq!(objects(&m, &entries, &mut b).unwrap(), Objects { instances: 1, parts: 1, remainder: 1 });
        assert_eq!(b.seal(Stamp::default()).unwrap().views[0].aux_depth, None);
        for (at, x) in [(AUX_SCALE, 0.5), (ONE, 1.0), (CLIP_NEAR, 2.0), (CLIP_FAR, 900.0), (SCALE_NUMERATOR, 1.0), (SCALE_DENOMINATOR, 1.0)] {
            m.f(at, x);
        }
        for k in 0..12 {
            m.f(VIEW + 4 + 4 * k, 1.0);
        }
        for k in 0..16 {
            m.f(VIEW + 0x78 + 4 * k, 1.0);
        }
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        assert_eq!(objects(&m, &entries, &mut b).unwrap(), Objects { instances: 1, parts: 2, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        let v = &s.views[0];
        assert_eq!(s.part_at(v, 0).unwrap().1.flags, PartFlags::AUX_DEPTH);
        let got = v.aux_depth.unwrap();
        assert_eq!((got.scale, got.clip, got.rest_clip), (0.5, [1.0, 900.0], [2.0, 900.0]));
        assert_eq!(got.rows, got.rest_rows.map(|r| r.map(|x| x * 0.5)));
    }

    #[test]
    fn a_part_drawing_chosen_subparts_is_in_the_scene_with_its_mask() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let a = object(&mut m, 1, 0xe000_0100, 1);
        object_record(&mut m, 4, Some(lighting(8)), 10.0);
        let visible = Subparts { first: 2, count: 3, visible: 0b101 };
        let chosen = Part { flags: PartFlags::SUBPARTS, subparts: Some(visible), ..part(4, 0) };
        let entries = [Entry::Object { handle: a, part: Some(chosen) }];
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = objects(&m, &entries, &mut b).unwrap();
        assert_eq!(got, Objects { instances: 1, parts: 1, remainder: 0 });
        let s = b.seal(Stamp::default()).unwrap();
        let (_, p) = s.part_at(&s.views[0], 0).unwrap();
        assert_eq!((p.flags, p.subparts), (PartFlags::SUBPARTS, Some(visible)));
    }

    #[test]
    fn what_extraction_cannot_read_is_a_remainder_not_a_part() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let a = object(&mut m, 1, 0xe000_0100, 1);
        object_record(&mut m, 4, Some(lighting(8)), 10.0);
        // Record 5's rows do not read; object 0x9999 does not exist.
        record(&mut m, 5, Some(lighting(8)));
        let entries = [
            Entry::Object { handle: a, part: Some(part(4, 0)) },
            Entry::Object { handle: a, part: None },
            Entry::Object { handle: a, part: Some(part(5, 1)) },
            Entry::Object { handle: 0x9999_0009, part: Some(part(4, 2)) },
        ];
        let mut b = SceneBuilder::default();
        view(&mut b, &m);
        let got = objects(&m, &entries, &mut b).unwrap();
        assert_eq!(got, Objects { instances: 1, parts: 1, remainder: 3 });
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        assert_eq!(v.remainder, 3);
        assert!(s.part_at(v, 0).is_some());
        for slot in 1..4 {
            assert!(s.part_at(v, slot).is_none(), "slot {slot} is the guest's");
        }
    }

    #[test]
    fn objects_need_an_open_view() {
        let m = Mem::default();
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(objects(&m, &[], &mut b).unwrap_err(), BuildError::NoView);
    }

    #[test]
    fn the_clusters_are_the_layers_structure_items_once_each() {
        let mut m = Mem::default();
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 5);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        // Structure records: 1 and 2 cluster 7, 3 cluster 2, 4 a placement.
        let bsp = |m: &mut Mem, record: u32, key: u32, flags: u32| {
            crate::testing::record(m, record, None);
            let at = RENDER_RECORDS + RENDER_RECORD_SIZE * record;
            m.w(at, flags);
            m.w(at + 0xc, key);
        };
        bsp(&mut m, 1, 7 << 9, 0);
        bsp(&mut m, 2, 7 << 9, 0);
        bsp(&mut m, 3, 2 << 9, 0);
        bsp(&mut m, 4, 1 << 18, 1 << 12);
        for (i, (record, mask)) in [(1u32, 0b1000u32), (3, 0b0010), (2, 0b1000), (4, 0b1000), (3, 0b1000)].into_iter().enumerate() {
            let at = ITEMS + ITEM_SIZE * i as u32;
            m.w(at, GEOMETRY_BUILD);
            m.w(at + 0xc, record << 8);
            m.w(at + 0x10, mask);
        }
        assert_eq!(layer_clusters(&m, 3), Some(vec![7, 2]));
        assert_eq!(layer_clusters(&m, 1), Some(vec![2]));
        m.w(CURRENT_LIST, 0);
        assert_eq!(layer_clusters(&m, 3), None);
    }
}
