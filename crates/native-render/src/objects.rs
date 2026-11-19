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
//! - A render record (`0x004ba138 + 0x20 · index`, filled by `0x0002cbf0`)
//!   does not hold the object handle. Its `+4` is the render model (mode)
//!   tag datum, `-1` for the structure BSP: `0x0003d4f0` stores it at the
//!   per-object render state's `+0x5c + 4i`, and `0x00133f70` hands it to
//!   `0x0002cbf0` as its second stack argument. Its `+0x10` is the object's
//!   lighting, a pointer to `+0x44` of the object's record in the
//!   render-state cache (`0x0004b5d0`: `lea ebx, [record + 0x44]`, stored at
//!   the state's `+0x48 + 4i`; the fifth stack argument of the call at
//!   `0x001341d0`), or 0 when the object has none.
//! - The render-state cache: a datum array at `[0x00509434]` (data `+0x44`,
//!   high water `+0x38`, as `0x0016b840` allocates it) of `0x100`-byte
//!   records; a record's `+4` is its owner's object handle, and the owner's
//!   body `+0xcc` holds the record's handle (`0x0003ddd0` checks
//!   `[data + 0x100 · (body[+0xcc] & 0xffff) + 4] == handle`). So an object
//!   geometry record names its object through `+0x10`, `-0x44`, `+4`, and
//!   the owner's `+0xcc` confirms it ([`record_owner`]).
//! - The object lookup `0x000badc0`: slot `handle & 0xffff` below `[H+0x38]`,
//!   header `[H+0x24] · slot + [H+0x44]` (`0x0003ddd0` indexes the same
//!   array with a fixed `0xc`), salt non-zero and equal to `handle >> 16`.
//! - The base layers' layer-3 call returns to `0x0002bed5`
//!   (`render_scene_passes`, the call at `0x0002bed0`). The other caller of
//!   layer 3 is the first-person draw `0x0003fdf0`: its call at `0x00040131`
//!   returns to `0x00040136`, after the transparents, for its own record list
//!   (`0x004c8740`, `[0x0050943c]` records) under its own camera. It stays on
//!   D3D ([`FIRST_PERSON_LAYER3_RETURN`]).

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
/// Where the first-person draw's layer-3 call (`0x00040131`, in
/// `0x0003fdf0`) returns. Left to D3D: its view is the first-person one.
pub const FIRST_PERSON_LAYER3_RETURN: u32 = 0x0004_0136;
/// A render record's render model tag datum (`-1`: the structure BSP).
pub const RECORD_MODEL: u32 = 4;
/// A render record's lighting: `+0x44` of a render-state cache record, or 0.
pub const RECORD_LIGHTING: u32 = 0x10;
/// The render-state cache's datum array pointer.
pub const RENDER_STATE_CACHE: u32 = 0x0050_9434;
pub const CACHE_RECORD_SIZE: u32 = 0x100;
/// A cache record's owner object handle.
pub const CACHE_OWNER: u32 = 4;
/// Where in a cache record a render record's lighting pointer points.
pub const CACHE_LIGHTING: u32 = 0x44;
/// An object body's render-state cache handle (`-1`: none).
pub const BODY_RENDER_STATE: u32 = 0xcc;
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
    /// What the walk met, for the feature's diagnostics.
    pub walk: WalkStats,
}

/// Counts from one [`clear_objects`] walk: where items fell out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Items walked (`first .. count` of the list).
    pub walked: u32,
    /// With the layer's bit set.
    pub in_layer: u32,
    /// Of those, built by `0x0004cbf0` (geometry).
    pub geometry: u32,
    /// Of those, whose render record is an object's (model datum not `-1`).
    pub object_records: u32,
    /// Of those, whose lighting pointer names a live owner
    /// ([`record_owner`]) with a definition.
    pub live: u32,
    /// Of those, that `native` accepted.
    pub accepted: u32,
    /// Accepted items whose mask could not be written (the walk stops).
    pub write_failed: u32,
    /// Build functions of in-layer items that are not geometry, up to four
    /// distinct, with their counts.
    pub other_builds: Vec<(u32, u32)>,
}

/// What a render record draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordOwner {
    /// The structure BSP (model datum `-1`).
    World,
    /// An object's geometry, by the owner's handle.
    Object(u32),
    /// An object's geometry whose owner does not resolve: no lighting
    /// pointer, one outside the render-state cache, or an owner that is not
    /// live or whose cache handle names another record. Left to D3D.
    Unresolved,
}

/// What render record `record` draws; `None` when it cannot be read.
pub fn record_owner(mem: &dyn GuestRead, record: u16) -> Option<RecordOwner> {
    let at = RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record);
    if mem.read_u32(at + RECORD_MODEL)? == u32::MAX {
        return Some(RecordOwner::World);
    }
    let lighting = mem.read_u32(at + RECORD_LIGHTING)?;
    Some(lighting_owner(mem, lighting).map_or(RecordOwner::Unresolved, RecordOwner::Object))
}

/// The live object whose render-state cache record holds `lighting` (a
/// pointer to the record's `+0x44`), confirmed by the owner's body `+0xcc`
/// naming that record.
pub fn lighting_owner(mem: &dyn GuestRead, lighting: u32) -> Option<u32> {
    let record = lighting.checked_sub(CACHE_LIGHTING)?;
    let cache = mem.read_u32(RENDER_STATE_CACHE)?;
    if cache == 0 {
        return None;
    }
    let offset = record.checked_sub(mem.read_u32(cache + ARRAY_DATA)?)?;
    if offset % CACHE_RECORD_SIZE != 0 {
        return None;
    }
    let index = offset / CACHE_RECORD_SIZE;
    if index >= mem.read_u32(cache + ARRAY_HIGH_WATER)? {
        return None;
    }
    let owner = mem.read_u32(record + CACHE_OWNER)?;
    let state = mem.read_u32(body(mem, owner)? + BODY_RENDER_STATE)?;
    (state != u32::MAX && state & 0xffff == index).then_some(owner)
}

/// The render record index of a geometry item (build `0x0004cbf0`, the
/// argument's `+0xc` high bits), `None` for anything else.
fn item_record(mem: &dyn GuestRead, item: u32) -> Option<u16> {
    if mem.read_u32(item)? != GEOMETRY_BUILD {
        return None;
    }
    let record = ((mem.read_u32(item + 0xc)? as i32) >> 8) as u16 as i16;
    (record >= 0).then_some(record as u16)
}

/// What a geometry item's render record draws, `None` for anything else.
pub fn item_owner(mem: &dyn GuestRead, item: u32) -> Option<RecordOwner> {
    record_owner(mem, item_record(mem, item)?)
}

/// The owner's handle of an object geometry item whose owner resolves,
/// `None` for anything else.
pub fn item_object(mem: &dyn GuestRead, item: u32) -> Option<u32> {
    match item_owner(mem, item)? {
        RecordOwner::Object(h) => Some(h),
        _ => None,
    }
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
        out.walk.walked += 1;
        let Some(mask) = mem.read_u32(item + 0x10) else { break };
        if mask & bit == 0 {
            continue;
        }
        out.walk.in_layer += 1;
        match mem.read_u32(item) {
            Some(GEOMETRY_BUILD) => out.walk.geometry += 1,
            Some(build) => {
                let builds = &mut out.walk.other_builds;
                match builds.iter().position(|(b, _)| *b == build) {
                    Some(i) => builds[i].1 += 1,
                    None if builds.len() < 4 => builds.push((build, 1)),
                    None => {}
                }
                continue;
            }
            None => continue,
        }
        let handle = match item_owner(mem, item) {
            Some(RecordOwner::Object(h)) => {
                out.walk.object_records += 1;
                h
            }
            Some(RecordOwner::Unresolved) => {
                out.walk.object_records += 1;
                continue;
            }
            Some(RecordOwner::World) | None => continue,
        };
        let def = match seen.iter().find(|(h, _)| *h == handle) {
            Some(&(_, d)) => d,
            None => {
                let d = definition(mem, handle);
                seen.push((handle, d));
                d
            }
        };
        let (Some(def), Some(shader)) = (def, mem.read_u32(item + 8)) else { continue };
        out.walk.live += 1;
        if !native(handle, def, shader) {
            continue;
        }
        out.walk.accepted += 1;
        if mem.write_u32(item + 0x10, mask & !bit).is_none() {
            out.walk.write_failed += 1;
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
    kept.walk = std::mem::take(&mut cleared.walk);
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
    pub const CACHE: u32 = 0x0073_0000;
    pub const CACHE_DATA: u32 = 0x0074_0000;
    const LIST: u32 = 0x004c_1a48;
    const ITEMS: u32 = 0x0060_0000;

    /// The lighting pointer of render-state cache record `index`.
    pub fn lighting(index: u32) -> u32 {
        CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_LIGHTING
    }

    /// Makes render-state cache record `index` (salt `0x4000 + index`)
    /// `owner`'s, and points `body`'s cache handle at it.
    pub fn cache(m: &mut Mem, index: u32, owner: u32, body: u32) {
        m.w(RENDER_STATE_CACHE, CACHE);
        m.w(CACHE + 0x38, 64);
        m.w(CACHE + 0x44, CACHE_DATA);
        m.w(CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_OWNER, owner);
        m.w(body + BODY_RENDER_STATE, (0x4000 + index) << 16 | index);
    }

    /// Render record `record`: object geometry of model `0xe100_0000 |
    /// record` lit by `lighting`, or the BSP's for `None`.
    pub fn record(m: &mut Mem, record: u32, lighting: Option<u32>) {
        let at = RENDER_RECORDS + 0x20 * record;
        match lighting {
            Some(l) => {
                m.w(at + RECORD_MODEL, 0xe100_0000 | record);
                m.w(at + RECORD_LIGHTING, l);
            }
            None => {
                m.w(at + RECORD_MODEL, u32::MAX);
                m.w(at + RECORD_LIGHTING, 0);
            }
        }
    }

    /// Puts object `slot` (salt `0x8000 + slot`) with `nodes` node matrices
    /// (node `n` translated by `n` along x) and definition `def` in `m`, its
    /// render-state cache record `slot + 7` (so the two indices differ).
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
        let handle = salt << 16 | slot;
        cache(m, slot + 7, handle, body);
        handle
    }

    /// Item `i` of render record `record`, lit by `lighting` (`None`: the
    /// BSP's).
    fn item(m: &mut Mem, i: u32, record: u32, lighting: Option<u32>, shader: u32, mask: u32) {
        let a = ITEMS + ITEM_SIZE * i;
        m.w(a, GEOMETRY_BUILD);
        m.w(a + 8, shader);
        m.w(a + 0xc, record << 8 | 2);
        m.w(a + 0x10, mask);
        self::record(m, record, lighting);
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
        item(&mut m, 0, 1, None, 0x10, 0b1010);
        item(&mut m, 1, 2, Some(lighting(8)), 0x10, 0b1010);
        item(&mut m, 2, 3, Some(lighting(8)), 0x11, 0b1000);
        item(&mut m, 3, 4, Some(lighting(9)), 0x10, 0b1000);
        // Cache record 3 names a handle with no live object.
        m.w(CACHE_DATA + CACHE_RECORD_SIZE * 3 + CACHE_OWNER, 0x1234_0003);
        item(&mut m, 4, 5, Some(lighting(3)), 0x10, 0b1000);
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
    fn a_record_names_its_object_through_the_render_state_cache() {
        let (mut m, a, b) = scene();
        assert_eq!(record_owner(&m, 1), Some(RecordOwner::World), "model -1: the BSP");
        assert_eq!(record_owner(&m, 2), Some(RecordOwner::Object(a)));
        assert_eq!(record_owner(&m, 4), Some(RecordOwner::Object(b)));
        assert_eq!(item_object(&m, ITEMS + ITEM_SIZE), Some(a));
        assert_eq!(item_object(&m, ITEMS), None, "the BSP's item");
        // The model datum is not a handle: a record whose model datum
        // happens to equal a live handle still resolves by its lighting.
        m.w(RENDER_RECORDS + 0x20 * 4 + RECORD_MODEL, a);
        assert_eq!(record_owner(&m, 4), Some(RecordOwner::Object(b)));
        // The dead owner.
        assert_eq!(record_owner(&m, 5), Some(RecordOwner::Unresolved));
        assert_eq!(item_object(&m, ITEMS + 4 * ITEM_SIZE), None);
        // An unreadable record.
        assert_eq!(record_owner(&m, 40), None);
    }

    #[test]
    fn a_lighting_pointer_that_does_not_lead_back_to_its_owner_resolves_to_nothing() {
        let (mut m, a, _) = scene();
        let unresolved = |m: &Mem| record_owner(m, 2) == Some(RecordOwner::Unresolved);
        let set = |m: &mut Mem, l: u32| m.w(RENDER_RECORDS + 0x20 * 2 + RECORD_LIGHTING, l);
        assert_eq!(lighting_owner(&m, lighting(8)), Some(a));
        // No lighting.
        set(&mut m, 0);
        assert!(unresolved(&m));
        // Not at a record's +0x44.
        set(&mut m, lighting(8) + 4);
        assert!(unresolved(&m));
        // Below the array, and past its high-water mark.
        set(&mut m, CACHE_DATA - CACHE_RECORD_SIZE + CACHE_LIGHTING);
        assert!(unresolved(&m));
        set(&mut m, lighting(8));
        m.w(CACHE + 0x38, 8);
        assert!(unresolved(&m));
        m.w(CACHE + 0x38, 64);
        assert!(!unresolved(&m));
        // The owner's cache handle names another record (a stale cache).
        m.w(BODIES + 0x1000 + BODY_RENDER_STATE, 0x4009_0009);
        assert!(unresolved(&m));
        m.w(BODIES + 0x1000 + BODY_RENDER_STATE, u32::MAX);
        assert!(unresolved(&m));
        m.w(BODIES + 0x1000 + BODY_RENDER_STATE, 0x4008_0008);
        assert!(!unresolved(&m));
        // A stale owner handle (another salt).
        m.w(CACHE_DATA + CACHE_RECORD_SIZE * 8 + CACHE_OWNER, a ^ 0x0001_0000);
        assert!(unresolved(&m));
        m.w(CACHE_DATA + CACHE_RECORD_SIZE * 8 + CACHE_OWNER, a);
        assert!(!unresolved(&m));
        // No cache.
        m.w(RENDER_STATE_CACHE, 0);
        assert!(unresolved(&m));
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
        let w = &c.walk;
        assert_eq!(
            (w.walked, w.in_layer, w.geometry, w.object_records, w.live, w.accepted, w.write_failed),
            (5, 5, 5, 4, 3, 2, 0),
            "the BSP is no object; the dead owner has no body; B's shader is refused"
        );
        assert!(w.other_builds.is_empty());
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
    fn the_walk_counts_items_of_other_builds_by_build() {
        let (mut m, ..) = scene();
        m.w(ITEMS + ITEM_SIZE * 3, 0x0004_b5d0);
        m.w(ITEMS + ITEM_SIZE * 4, 0x0004_b5d0);
        m.w(ITEMS, 0x0005_0000);
        let c = clear_objects(&mut m, 3, &|_, _, _| false).unwrap();
        assert_eq!((c.walk.in_layer, c.walk.geometry, c.walk.accepted), (5, 2, 0));
        assert_eq!(c.walk.other_builds, vec![(0x0005_0000, 1), (0x0004_b5d0, 2)]);
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
