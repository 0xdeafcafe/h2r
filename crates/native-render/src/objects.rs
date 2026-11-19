//! Objects in guest memory, read once per view by the stage-2 hook, and
//! the skip of their layer-3 items (docs/graphics/native-renderer.md,
//! "Stage 2: objects and skinning").
//!
//! Research points (Code, `h2 disasm`):
//!
//! - The object array header `H = [0x004e0300]`; a handle's header is
//!   `[H+0x44] + 0xc · (handle & 0xffff)`, live when its slot is below the
//!   high-water mark `[H+0x38]` and its `u16` salt equals `handle >> 16`;
//!   the body is the header's `+8`.
//! - Body `+0` the definition tag datum, `+0x30` / `+0x3c` the world
//!   bounding sphere.
//! - Body `+0x114` (`u16` size in bytes) and `+0x116` (`i16` offset from
//!   the body): the node-matrix block `object_new` `0x000b7b40` reserves at
//!   `0x000b800b` through `0x000bc380`, `0x34` bytes a node
//!   ([`crate::model::NodeMatrix`]); `0x000bdf90` reads the root node
//!   through `[body+0x116]`.
//! - The base layers' layer-3 call returns to `0x0002bed5`
//!   (`render_scene_passes`, the call at `0x0002bed0`).

use crate::camera::{GuestRead, GuestWrite};
use crate::model::{NODE_MATRIX_SIZE, NodeMatrix};
use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, LIGHTMAP_LAYER, RENDER_RECORD_SIZE, RENDER_RECORDS};

pub const OBJECTS: u32 = 0x004e_0300;
const ARRAY_HIGH_WATER: u32 = 0x38;
const ARRAY_DATA: u32 = 0x44;
const HEADER_SIZE: u32 = 0xc;
const HEADER_BODY: u32 = 8;
pub const BODY_DEFINITION: u32 = 0;
pub const BODY_SPHERE: u32 = 0x30;
pub const BODY_SPHERE_RADIUS: u32 = 0x3c;
pub const BODY_NODES_SIZE: u32 = 0x114;
pub const BODY_NODES_OFFSET: u32 = 0x116;
/// Where the base layers' layer-3 call (`0x0002bed0`) returns.
pub const SCENE_LAYER3_RETURN: u32 = 0x0002_bed5;
/// The layers whose object items the native pass replaces (bit `n`:
/// layer `n`). Layer 3 only in the first cut.
pub const REPLACED_LAYERS: u32 = 1 << LIGHTMAP_LAYER;
/// Nodes read at most per object (the game's models stay far below).
pub const MAX_NODES: usize = 256;
const MAX_ITEMS: u32 = 1 << 16;

/// The body of the live object `handle`, if any.
pub fn body(mem: &dyn GuestRead, handle: u32) -> Option<u32> {
    if handle == u32::MAX {
        return None;
    }
    let array = mem.read_u32(OBJECTS)?;
    if array == 0 {
        return None;
    }
    let slot = handle & 0xffff;
    if slot >= mem.read_u32(array + ARRAY_HIGH_WATER)? {
        return None;
    }
    let header = mem.read_u32(array + ARRAY_DATA)?.checked_add(HEADER_SIZE * slot)?;
    let salt = read_u16(mem, header)?;
    if u32::from(salt) != handle >> 16 || salt == 0 {
        return None;
    }
    let body = mem.read_u32(header + HEADER_BODY)?;
    (body != 0).then_some(body)
}

/// Most object slots [`object_meets_sphere`] walks; past it the answer is
/// yes.
pub const MAX_SLOTS: u32 = 1 << 13;

/// Whether a live object's world bounding sphere (body `+0x30`, `+0x3c`)
/// meets the sphere (`centre`, `radius`). A light whose sphere meets one
/// keeps its D3D passes: they light the object, which the native lights
/// do not yet. Every slot below the high-water mark with a non-zero salt
/// and a body counts as live. Conservative: a slot or sphere that cannot
/// be read, a non-finite one, or too many slots, answers yes; no object
/// array (`[0x004e0300]` zero) answers no.
pub fn object_meets_sphere(mem: &dyn GuestRead, centre: [f32; 3], radius: f32) -> bool {
    let Some(array) = mem.read_u32(OBJECTS) else { return true };
    if array == 0 {
        return false;
    }
    let (Some(high), Some(data)) = (mem.read_u32(array.wrapping_add(ARRAY_HIGH_WATER)), mem.read_u32(array.wrapping_add(ARRAY_DATA))) else {
        return true;
    };
    if high > MAX_SLOTS {
        return true;
    }
    (0..high).any(|slot| {
        let header = data.wrapping_add(HEADER_SIZE * slot);
        let Some(salt) = read_u16(mem, header) else { return true };
        if salt == 0 {
            return false;
        }
        let Some(body) = mem.read_u32(header.wrapping_add(HEADER_BODY)) else { return true };
        if body == 0 {
            return false;
        }
        let f = |o: u32| mem.read_f32(body.wrapping_add(o));
        let (Some(x), Some(y), Some(z), Some(r)) = (f(BODY_SPHERE), f(BODY_SPHERE + 4), f(BODY_SPHERE + 8), f(BODY_SPHERE_RADIUS)) else {
            return true;
        };
        spheres_meet(([x, y, z], r), (centre, radius))
    })
}

/// Whether two spheres share a point; anything not finite meets.
pub fn spheres_meet(a: ([f32; 3], f32), b: ([f32; 3], f32)) -> bool {
    // Checked before any `max`, which would turn a NaN radius into 0.
    let finite = a.0.iter().chain(&b.0).chain([&a.1, &b.1]).all(|x| x.is_finite());
    if !finite {
        return true;
    }
    let d2: f32 = (0..3).map(|k| (a.0[k] - b.0[k]) * (a.0[k] - b.0[k])).sum();
    let reach = a.1.max(0.0) + b.1.max(0.0);
    !(d2.is_finite() && reach.is_finite()) || d2 <= reach * reach
}

fn read_u16(mem: &dyn GuestRead, addr: u32) -> Option<u16> {
    let w = mem.read_u32(addr & !3)?;
    Some(if addr & 2 == 0 { w as u16 } else { (w >> 16) as u16 })
}

/// One object as the native pass draws it this view.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectFrame {
    pub handle: u32,
    /// The definition tag datum.
    pub definition: u32,
    /// World-space node matrices, in model node order.
    pub nodes: Vec<NodeMatrix>,
    /// World bounding sphere: centre, radius.
    pub sphere: ([f32; 3], f32),
}

/// The definition datum of the live object `handle`.
pub fn definition(mem: &dyn GuestRead, handle: u32) -> Option<u32> {
    mem.read_u32(body(mem, handle)? + BODY_DEFINITION)
}

/// Reads object `handle`: its definition, its node matrices (`None` when
/// the block is empty, misaligned, too long or not finite) and its sphere.
pub fn read_object(mem: &dyn GuestRead, handle: u32) -> Option<ObjectFrame> {
    let body = body(mem, handle)?;
    let definition = mem.read_u32(body + BODY_DEFINITION)?;
    let size = usize::from(read_u16(mem, body + BODY_NODES_SIZE)?);
    let offset = read_u16(mem, body + BODY_NODES_OFFSET)? as i16;
    if size == 0 || size % NODE_MATRIX_SIZE != 0 || size / NODE_MATRIX_SIZE > MAX_NODES || offset < 0 {
        return None;
    }
    let base = body.checked_add(offset as u32)?;
    let mut nodes = Vec::with_capacity(size / NODE_MATRIX_SIZE);
    for n in 0..size / NODE_MATRIX_SIZE {
        let at = base + (n * NODE_MATRIX_SIZE) as u32;
        let mut w = [0f32; 13];
        for (k, x) in w.iter_mut().enumerate() {
            *x = mem.read_f32(at + 4 * k as u32)?;
        }
        let m = NodeMatrix::from_words(&w);
        if !m.is_finite() {
            return None;
        }
        nodes.push(m);
    }
    let f = |o: u32| mem.read_f32(body + o);
    let sphere = ([f(BODY_SPHERE)?, f(BODY_SPHERE + 4)?, f(BODY_SPHERE + 8)?], f(BODY_SPHERE_RADIUS)?);
    Some(ObjectFrame {
        handle,
        definition,
        nodes,
        sphere,
    })
}

/// What [`clear_objects`] cleared: the items (to restore), the object
/// each belongs to, and those objects' handles, each once, in list order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClearedObjects {
    pub layer: u32,
    pub items: Vec<u32>,
    /// The handle of `items[i]`'s object.
    pub owners: Vec<u32>,
    pub handles: Vec<u32>,
}

/// The object handle of a geometry item (build `0x0004cbf0`, render
/// record `+4` not `-1`), `None` for anything else.
pub fn item_object(mem: &dyn GuestRead, item: u32) -> Option<u32> {
    if mem.read_u32(item)? != GEOMETRY_BUILD {
        return None;
    }
    let record = ((mem.read_u32(item + 0xc)? as i32) >> 8) as u16 as i16;
    if record < 0 {
        return None;
    }
    let handle = mem.read_u32(RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32 + 4)?;
    (handle != u32::MAX).then_some(handle)
}

/// Clears `layer`'s bit on every object item of the current list that
/// `native(handle, definition, shader)` accepts, when the current layer is `layer`
/// and `layer` is in [`REPLACED_LAYERS`]. `None` (nothing written) for
/// another layer or an unreadable list; an unreadable item ends the walk
/// with what was cleared so far.
pub fn clear_objects(
    mem: &mut dyn GuestWrite,
    layer: u32,
    native: &dyn Fn(u32, u32, u32) -> bool,
) -> Option<ClearedObjects> {
    if layer >= 32 || REPLACED_LAYERS & (1 << layer) == 0 || mem.read_u32(CURRENT_LAYER)? != layer {
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
    let mut out = ClearedObjects {
        layer,
        ..Default::default()
    };
    // Definitions looked up once per handle.
    let mut seen: Vec<(u32, Option<u32>)> = Vec::new();
    for i in first..count.min(first.saturating_add(MAX_ITEMS)) {
        let item = items + ITEM_SIZE * i;
        let Some(mask) = mem.read_u32(item + 0x10) else { break };
        if mask & bit == 0 {
            continue;
        }
        let Some(handle) = item_object(mem, item) else { continue };
        let def = match seen.iter().find(|(h, _)| *h == handle) {
            Some(&(_, d)) => d,
            None => {
                let d = definition(mem, handle);
                seen.push((handle, d));
                d
            }
        };
        let (Some(def), Some(shader)) = (def, mem.read_u32(item + 8)) else { continue };
        if !native(handle, def, shader) {
            continue;
        }
        if mem.write_u32(item + 0x10, mask & !bit).is_none() {
            break;
        }
        out.items.push(item);
        out.owners.push(handle);
        if !out.handles.contains(&handle) {
            out.handles.push(handle);
        }
    }
    Some(out)
}

/// Gives the objects `keep` refuses their bits back at once (they are not
/// drawn natively after all) and drops them from `cleared`.
pub fn keep_only(mem: &mut dyn GuestWrite, cleared: &mut ClearedObjects, keep: &dyn Fn(u32) -> bool) {
    let mut back = ClearedObjects {
        layer: cleared.layer,
        ..Default::default()
    };
    let mut kept = ClearedObjects {
        layer: cleared.layer,
        ..Default::default()
    };
    for (&item, &owner) in cleared.items.iter().zip(&cleared.owners) {
        let to = if keep(owner) { &mut kept } else { &mut back };
        to.items.push(item);
        to.owners.push(owner);
    }
    kept.handles = cleared.handles.iter().copied().filter(|&h| keep(h)).collect();
    restore(mem, &back);
    *cleared = kept;
}

/// Sets the bits [`clear_objects`] cleared back.
pub fn restore(mem: &mut dyn GuestWrite, cleared: &ClearedObjects) {
    crate::skip::restore(
        mem,
        &crate::skip::Cleared {
            layer: cleared.layer,
            items: cleared.items.clone(),
        },
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    pub const ARRAY: u32 = 0x0070_0000;
    pub const HEADERS: u32 = 0x0071_0000;
    pub const BODIES: u32 = 0x0072_0000;
    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;

    /// Puts object `slot` (salt `0x8000 + slot`) with `nodes` node matrices
    /// (node `n` translated by `n` along x) and definition `def` in `m`.
    pub fn object(m: &mut Mem, slot: u32, def: u32, nodes: u32) -> u32 {
        m.w(OBJECTS, ARRAY);
        m.w(ARRAY + 0x38, 64);
        m.w(ARRAY + 0x44, HEADERS);
        let salt = 0x8000 + slot;
        let header = HEADERS + 0xc * slot;
        let body = BODIES + 0x1000 * slot;
        m.w(header, salt | 0x0100_0000);
        m.w(header + 8, body);
        m.w(body, def);
        for (k, v) in [1.0f32, 2.0, 3.0, 0.5].into_iter().enumerate() {
            m.f(body + 0x30 + 4 * k as u32, v);
        }
        // Size, then offset 0x200.
        m.w(body + 0x114, (0x34 * nodes) | (0x200 << 16));
        for n in 0..nodes {
            let at = body + 0x200 + 0x34 * n;
            let w = NodeMatrix {
                position: [n as f32, 0.0, 0.0],
                ..NodeMatrix::IDENTITY
            };
            for (k, f) in bytemuck::cast_ref::<NodeMatrix, [f32; 13]>(&w).iter().enumerate() {
                m.f(at + 4 * k as u32, *f);
            }
        }
        salt << 16 | slot
    }

    fn item(m: &mut Mem, i: u32, record: u32, handle: u32, shader: u32, mask: u32) {
        let a = ITEMS + ITEM_SIZE * i;
        m.w(a, GEOMETRY_BUILD);
        m.w(a + 8, shader);
        m.w(a + 0xc, record << 8 | 2);
        m.w(a + 0x10, mask);
        m.w(RENDER_RECORDS + 0x20 * record + 4, handle);
    }

    /// Layer 3 current; items: a BSP part, two parts of object A (shaders
    /// 0x10 and 0x11), a part of object B, a part of a dead handle.
    fn scene() -> (Mem, u32, u32) {
        let mut m = Mem::default();
        let a = object(&mut m, 1, 0xe000_0100, 3);
        let b = object(&mut m, 2, 0xe000_0200, 1);
        m.w(CURRENT_LAYER, 3);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 5);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        item(&mut m, 0, 1, u32::MAX, 0x10, 0b1010);
        item(&mut m, 1, 2, a, 0x10, 0b1010);
        item(&mut m, 2, 3, a, 0x11, 0b1000);
        item(&mut m, 3, 4, b, 0x10, 0b1000);
        item(&mut m, 4, 5, 0x1234_0003, 0x10, 0b1000);
        (m, a, b)
    }

    fn mask(m: &Mem, i: u32) -> u32 {
        m.read_u32(ITEMS + ITEM_SIZE * i + 0x10).unwrap()
    }

    #[test]
    fn a_light_sphere_meets_live_objects_only() {
        let mut m = Mem::default();
        object(&mut m, 1, 0xe000_0100, 1);
        object(&mut m, 2, 0xe000_0200, 1);
        m.w(ARRAY + 0x38, 4);
        m.w(HEADERS, 0); // slot 0 free
        m.w(HEADERS + 0xc * 3, 0); // slot 3 free
        // Object 1 at (1, 2, 3) r 0.5; object 2 moved to (20, 0, 0) r 1.
        for (k, v) in [20.0f32, 0.0, 0.0, 1.0].into_iter().enumerate() {
            m.f(BODIES + 0x2000 + 0x30 + 4 * k as u32, v);
        }
        assert!(object_meets_sphere(&m, [1.0, 2.0, 5.0], 1.5), "touches object 1");
        assert!(!object_meets_sphere(&m, [1.0, 2.0, 5.0], 1.4));
        assert!(object_meets_sphere(&m, [17.0, 0.0, 0.0], 2.0), "touches object 2");
        assert!(!object_meets_sphere(&m, [10.0, 0.0, 0.0], 2.0));
        // A free slot's stale body is not an object.
        m.w(HEADERS + 8, BODIES + 0x2000);
        assert!(!object_meets_sphere(&m, [10.0, 0.0, 0.0], 2.0));
        // Past the high-water mark: not walked.
        m.w(ARRAY + 0x38, 2);
        assert!(!object_meets_sphere(&m, [17.0, 0.0, 0.0], 2.0));
        // Unreadable or not finite: yes.
        m.w(ARRAY + 0x38, 4);
        m.f(BODIES + 0x2000 + 0x3c, f32::NAN);
        assert!(object_meets_sphere(&m, [-50.0, 0.0, 0.0], 1.0));
        m.f(BODIES + 0x2000 + 0x3c, 1.0);
        assert!(!object_meets_sphere(&m, [-50.0, 0.0, 0.0], 1.0));
        m.w(ARRAY + 0x38, 5);
        assert!(object_meets_sphere(&m, [-50.0, 0.0, 0.0], 1.0), "slot 4 unreadable");
        m.w(ARRAY + 0x38, MAX_SLOTS + 1);
        assert!(object_meets_sphere(&m, [-50.0, 0.0, 0.0], 1.0), "too many slots");
        // No array: no objects.
        m.w(OBJECTS, 0);
        assert!(!object_meets_sphere(&m, [1.0, 2.0, 3.0], 1.0));
    }

    #[test]
    fn handles_find_their_bodies_and_stale_ones_do_not() {
        let (m, a, _) = scene();
        assert_eq!(body(&m, a), Some(BODIES + 0x1000));
        assert_eq!(body(&m, a ^ 0x0001_0000), None, "salt");
        assert_eq!(body(&m, 0x8040_0040), None, "past the high-water mark");
        assert_eq!(body(&m, u32::MAX), None);
    }

    #[test]
    fn an_object_reads_its_nodes_and_sphere() {
        let (m, a, _) = scene();
        let o = read_object(&m, a).unwrap();
        assert_eq!(o.definition, 0xe000_0100);
        assert_eq!(o.nodes.len(), 3);
        assert_eq!(o.nodes[2].position, [2.0, 0.0, 0.0]);
        assert_eq!(o.sphere, ([1.0, 2.0, 3.0], 0.5));
    }

    #[test]
    fn a_bad_node_block_reads_as_none() {
        let (mut m, a, _) = scene();
        m.w(BODIES + 0x1000 + 0x114, 0x35 | (0x200 << 16));
        assert_eq!(read_object(&m, a), None, "not a whole node");
        let (mut m, a, _) = scene();
        m.f(BODIES + 0x1000 + 0x200 + 4, f32::NAN);
        assert_eq!(read_object(&m, a), None, "NaN");
    }

    #[test]
    fn only_accepted_object_items_lose_the_bit_and_get_it_back() {
        let (mut m, a, b) = scene();
        let before: Vec<u32> = (0..5).map(|i| mask(&m, i)).collect();
        let c = clear_objects(&mut m, 3, &|_, def, shader| def == 0xe000_0100 || (def == 0xe000_0200 && shader != 0x10)).unwrap();
        assert_eq!(c.items, vec![ITEMS + ITEM_SIZE, ITEMS + 2 * ITEM_SIZE]);
        assert_eq!(c.owners, vec![a, a]);
        assert_eq!(c.handles, vec![a], "one handle per object; B's material stays on D3D");
        assert_eq!(mask(&m, 0), 0b1010, "BSP untouched");
        assert_eq!(mask(&m, 1), 0b0010, "layer 1 kept");
        assert_eq!(mask(&m, 2), 0);
        assert_eq!(mask(&m, 3), 0b1000);
        assert_eq!(mask(&m, 4), 0b1000, "dead handle");
        restore(&mut m, &c);
        assert_eq!((0..5).map(|i| mask(&m, i)).collect::<Vec<_>>(), before);
        let _ = b;
    }

    #[test]
    fn objects_that_are_not_drawn_after_all_get_their_bits_back_at_once() {
        let (mut m, a, b) = scene();
        let mut c = clear_objects(&mut m, 3, &|_, _, _| true).unwrap();
        assert_eq!(c.handles, vec![a, b]);
        keep_only(&mut m, &mut c, &|h| h == b);
        assert_eq!((c.items.clone(), c.owners.clone(), c.handles.clone()), (vec![ITEMS + 3 * ITEM_SIZE], vec![b], vec![b]));
        assert_eq!((mask(&m, 1), mask(&m, 2), mask(&m, 3)), (0b1010, 0b1000, 0), "A's back, B's still clear");
        restore(&mut m, &c);
        assert_eq!(mask(&m, 3), 0b1000);
    }

    #[test]
    fn other_layers_write_nothing() {
        let (mut m, ..) = scene();
        m.w(CURRENT_LAYER, 1);
        assert_eq!(clear_objects(&mut m, 1, &|_, _, _| true), None, "not a replaced layer");
        assert_eq!(clear_objects(&mut m, 3, &|_, _, _| true), None, "not the current layer");
        assert_eq!(mask(&m, 1), 0b1010);
    }
}
