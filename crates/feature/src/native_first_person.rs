//! The first-person arms and weapon drawn by the native GPU renderer.
//! Off by default; `H2_FEATURES=impl.render.native_first_person=on` turns
//! it on (exactly that key: `impl.render=on` does not).
//!
//! One context-only observer before `LAYER_DRAW` acts only on the layer-3
//! call in `0x0003fdf0` (return `0x00040136`). Both invocations are
//! independent: accepted items are planned in list order, exactly the planned
//! ones keep their bits cleared until the next layer draw, and `0x000404b0`
//! and the following transparent call stay untouched.
//!
//! Each record's `+4` mode, which can differ from its owner's third-person
//! definition, keys the loaded model. The owner supplies lighting; the item's
//! `ItemKey` and record rows select its exact part, implementation and pose.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::GuestRead;
use native_render::objects::{
    self, ClearedObjects, FIRST_PERSON_LAYER3_RETURN, ItemDraw, ItemKey, ObjectFrame, RECORD_MODEL,
    RecordOwner,
};
use native_render::skip::{
    CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, LIGHTMAP_LAYER, RENDER_RECORD_SIZE,
    RENDER_RECORDS,
};

use crate::native_objects::{NativeSet, item_refusal};
use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-first-person";
pub const SWITCH: &str = "impl.render.native_first_person";

/// What draws first-person objects: boot's glue over the object programs.
pub trait Host: Send + Sync {
    /// The models and materials resolved for this map and load.
    fn native(&self, mem: &dyn GuestMemory) -> Option<Arc<NativeSet>>;
    /// Load a record's mode met in the first-person list (cheap to repeat).
    fn want(&self, model: u32);
    /// Plans every item before returning, in list order. `models` maps each
    /// record to its mode; each object's frame supplies its owner's lighting.
    /// The returned booleans correspond to `items` and commit only fully
    /// planned draws. Shadow items are planned for comparison, never drawn.
    fn draw(
        &self,
        generation: u64,
        objects: Vec<ObjectFrame>,
        items: Vec<ItemDraw>,
        models: HashMap<u16, u32>,
    ) -> Vec<bool>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CLEARED: Mutex<Option<(ClearedObjects, Vec<u32>)>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static OBJECTS: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// First-person layer-3 calls, native draws, objects and suppressed items,
/// and calls for which the host drew none of the offered items.
pub fn counts() -> (u64, u64, u64, u64, u64) {
    (
        CALLS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        OBJECTS.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
        REFUSED.load(Ordering::Relaxed),
    )
}

/// The mode a render record draws.
fn record_model(m: &dyn GuestRead, record: u16) -> Option<u32> {
    m.read_u32(RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record) + RECORD_MODEL)
}

/// Reads the whole first-person list before writing any mask and maps every
/// geometry record to its mode. Any uncertain in-layer item refuses the
/// entire call; an unsupported part simply keeps its own D3D draw.
fn admission(m: &dyn GuestRead, set: &NativeSet, host: &dyn Host) -> Option<HashMap<u16, u32>> {
    const MAX_ITEMS: u32 = 1 << 16;
    const FIRST_PERSON_ITEMS: u32 = 0x004c_8740;
    const FIRST_PERSON_RECORD_COUNT: u32 = 0x0050_943c;
    let list = m.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return None;
    }
    let count = m.read_u32(list + 4)?;
    let first = m.read_u32(list + 0xc)?;
    let items = m.read_u32(list + 0x14)?;
    if items != FIRST_PERSON_ITEMS || first > count || count - first > MAX_ITEMS {
        return None;
    }
    let records = m.read_u32(FIRST_PERSON_RECORD_COUNT)?;
    let bit = 1u32 << LIGHTMAP_LAYER;
    let mut models = HashMap::new();
    for i in first..count {
        let item = items + ITEM_SIZE * i;
        let mask = m.read_u32(item + 0x10)?;
        if mask & bit == 0 {
            continue;
        }
        if m.read_u32(item)? != GEOMETRY_BUILD {
            return None;
        }
        let key = objects::item_key(m, item)?;
        if u32::from(key.record) >= records {
            return None;
        }
        let model = record_model(m, key.record)?;
        match objects::item_owner(m, item)? {
            RecordOwner::World => continue,
            RecordOwner::Unresolved => return None,
            RecordOwner::Object(_) => {}
        }
        models.insert(key.record, model);
        if !set.accepts(model, &key) && !set.definitions.contains_key(&(model as u16)) {
            host.want(model);
        }
    }
    Some(models)
}

/// The observer before a layer draw. Only `0x0003fdf0`'s layer 3 is owned.
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let mut m = Mem(mem);
    if ret != FIRST_PERSON_LAYER3_RETURN || m.read_u32(CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    let calls = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if calls % 1800 == 1 {
        let (_, drawn, objects, items, refused) = counts();
        eprintln!(
            "native-first-person: {drawn} native draws of {calls} first-person layer-3 calls, {objects} objects, {items} items skipped, {refused} refused calls"
        );
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(set) = host.native(&*m.0) else {
        return;
    };
    let Some(models) = admission(&m, &set, &*host) else {
        return;
    };
    let accept = |_handle: u32, _definition: u32, key: &ItemKey| {
        models
            .get(&key.record)
            .is_some_and(|&model| set.accepts(model, key))
    };
    let Some(mut cleared) = objects::clear_objects(&mut m, LIGHTMAP_LAYER, &accept) else {
        return;
    };
    if cleared.walk.write_failed != 0 {
        objects::restore(&mut m, &cleared);
        return;
    }
    if cleared.items.is_empty() {
        return;
    }

    // Each accepted owner is read once. Its body node count need not match
    // the first-person mode: the record supplies the actual geometry rows.
    let mut frames: Vec<ObjectFrame> = Vec::with_capacity(cleared.handles.len());
    let mut frame_of: HashMap<u32, Option<usize>> = HashMap::new();
    for &handle in &cleared.handles {
        let frame = objects::read_object(&m, handle).map(|frame| {
            frames.push(frame);
            frames.len() - 1
        });
        frame_of.insert(handle, frame);
    }
    let mut items = Vec::with_capacity(cleared.items.len());
    for (i, (&owner, key)) in cleared.owners.iter().zip(&cleared.keys).enumerate() {
        let object = frame_of.get(&owner).copied().flatten();
        let Some(&model) = models.get(&key.record) else {
            continue;
        };
        let rows = objects::record_rows(&m, key.record);
        if item_refusal(&m, &set, model, key, object.is_some(), &rows).is_some() {
            continue;
        }
        let (Some(object), Ok(rows)) = (object, rows) else {
            continue;
        };
        let shadow = set.shadowed.contains(&(model as u16));
        items.push(ItemDraw {
            index: i,
            object,
            key: *key,
            rows,
            shadow,
        });
    }
    if items.is_empty() {
        objects::restore(&mut m, &cleared);
        return;
    }
    let indices: Vec<usize> = items.iter().map(|item| item.index).collect();
    let drawn = host.draw(set.generation, frames, items, models);
    let keep: HashSet<usize> = indices
        .iter()
        .zip(&drawn)
        .filter(|(_, drawn)| **drawn)
        .map(|(&index, _)| index)
        .collect();
    objects::keep_items(&mut m, &mut cleared, &|index| keep.contains(&index));
    if cleared.items.is_empty() {
        REFUSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    OBJECTS.fetch_add(cleared.handles.len() as u64, Ordering::Relaxed);
    ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    let left = cleared
        .items
        .iter()
        .map(|&item| m.read_u32(item + 0x10).unwrap_or(0))
        .collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some((cleared, left));
}

/// Restores the previous first-person layer-3 skip, unless the list has
/// since been rebuilt (an item whose mask changed is left alone).
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some((cleared, left)) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << cleared.layer;
    for (&item, &left) in cleared.items.iter().zip(&left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

pub struct NativeFirstPerson;

impl Feature for NativeFirstPerson {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "First-person arms and weapon drawn by the native GPU renderer",
        )
        .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().feature(SWITCH, false) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_first_person_skip", |o| {
                let mut guest = o.guest();
                let ret = guest.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *guest, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_objects::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
