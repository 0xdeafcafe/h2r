//! The first-person arms and weapon drawn by the native GPU renderer.
//! Off by default; `H2_FEATURES=impl.render.native_first_person=on` turns
//! it on (exactly that key: `impl.render=on` does not).
//!
//! One context-only observer before `LAYER_DRAW` acts only on the layer-3
//! call in `0x0003fdf0` (its return address is `0x00040136`). The two calls
//! are independent: each accepted draw is queued in guest order, its bits
//! are restored at the next layer draw, and `0x000404b0` and the transparent
//! call after the second draw are left untouched. The host receives no
//! camera copy: the device supplies the first-person view constants and
//! W-clip at the external-pass boundary, exactly as its D3D draw sees them.
//!
//! Admission is whole-object and keyed by each record's mode datum, which
//! can differ from its owner's third-person definition. If any live layer-3
//! item of an object is not accepted, all of that object's accepted items
//! are restored. An incomplete or non-geometry list leaves the call on D3D.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::GuestRead;
use native_render::objects::{
    self, ClearedObjects, FIRST_PERSON_LAYER3_RETURN, ObjectFrame, RECORD_MODEL, RecordOwner,
};
use native_render::skip::{
    CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, LIGHTMAP_LAYER, RENDER_RECORD_SIZE,
    RENDER_RECORDS,
};

use crate::native_objects::NativeSet;
use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-first-person";
pub const SWITCH: &str = "impl.render.native_first_person";

/// What draws the first-person objects: boot's glue over the object programs.
pub trait Host: Send + Sync {
    /// The models and materials resolved for this map, or `None` to leave
    /// the whole first-person view to D3D.
    fn native(&self, mem: &dyn GuestMemory) -> Option<Arc<NativeSet>>;
    /// Load a record's mode met in the first-person list (cheap to repeat).
    fn want(&self, model: u32);
    /// Queue this first-person layer-3 draw. `false` restores every item.
    /// Compared modes are included but keep their D3D draws.
    fn draw(&self, objects: Vec<ObjectFrame>) -> bool;
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
/// and calls whose queue request was refused.
pub fn counts() -> (u64, u64, u64, u64, u64) {
    (
        CALLS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        OBJECTS.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
        REFUSED.load(Ordering::Relaxed),
    )
}

/// The record-model of each accepted object. A first-person record can use
/// a different mode from its owner definition's third-person model.
struct Admission {
    models: HashMap<u32, u32>,
    accepted: HashSet<u32>,
}

/// Reads the whole live list before writing any mask. Any uncertain item
/// leaves the entire call on D3D; an unaccepted material or mixed record
/// models veto just their owner object.
fn admit(m: &dyn GuestRead, set: &NativeSet, host: &dyn Host) -> Option<Admission> {
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
    let mut accepted = HashSet::new();
    let mut rejected = HashSet::new();
    for i in first..count {
        let item = items + ITEM_SIZE * i;
        let mask = m.read_u32(item + 0x10)?;
        if mask & bit == 0 {
            continue;
        }
        if m.read_u32(item)? != GEOMETRY_BUILD {
            return None;
        }
        let record = ((m.read_u32(item + 0xc)? as i32) >> 8) as u16 as i16;
        if record < 0 || record as u32 >= records {
            return None;
        }
        let at = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        let model = m.read_u32(at + RECORD_MODEL)?;
        let handle = match objects::item_owner(m, item)? {
            RecordOwner::World => continue,
            RecordOwner::Unresolved => return None,
            RecordOwner::Object(handle) => handle,
        };
        let shader = m.read_u32(item + 8)?;
        if !set.accepts(model, shader) {
            if !set.definitions.contains_key(&(model as u16)) {
                host.want(model);
            }
            accepted.remove(&handle);
            rejected.insert(handle);
            continue;
        }
        if let Some(&other) = models.get(&handle)
            && other != model
        {
            static MIXED: Mutex<Option<HashSet<(u32, u32, u32)>>> = Mutex::new(None);
            let mut logged = MIXED.lock().unwrap_or_else(|e| e.into_inner());
            if logged
                .get_or_insert_with(HashSet::new)
                .insert((handle, other, model))
            {
                eprintln!(
                    "native-first-person: object {handle:#010x} mixes record modes {other:#010x} and {model:#010x}; it stays on D3D (logged once per pair)"
                );
            }
            accepted.remove(&handle);
            rejected.insert(handle);
            continue;
        }
        models.insert(handle, model);
        if !rejected.contains(&handle) {
            accepted.insert(handle);
        }
    }
    Some(Admission { models, accepted })
}

/// The object readiness checks except the third-person model's node count:
/// first-person rows and geometry come from the record's mode, not the
/// owner's body model.
fn ready(
    m: &dyn GuestRead,
    set: &NativeSet,
    cleared: &ClearedObjects,
    frame: &mut ObjectFrame,
) -> bool {
    if set.texaccum.contains(&(frame.definition as u16))
        && !native_render::object_nodes::texaccum_bound(m)
    {
        return false;
    }
    let Some(implementations) = set.lit(frame.definition) else {
        return true;
    };
    frame
        .lighting
        .is_some_and(|lighting| lighting.root.is_some())
        && objects::attach_node_rows(m, frame, cleared).is_ok()
        && frame.implementations.iter().all(|&(_, implementation)| {
            implementation < 8 && implementations & (1 << implementation) != 0
        })
        && set
            .missing_section(
                frame.definition,
                frame.implementations.iter().map(|&(section, _)| section),
            )
            .is_none()
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
            "native-first-person: {drawn} native draws of {calls} first-person layer-3 calls, {objects} objects, {items} items skipped, {refused} queue refusals"
        );
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(set) = host.native(&*m.0) else {
        return;
    };
    let Some(admission) = admit(&m, &set, &*host) else {
        return;
    };
    let accept = |handle: u32, _definition: u32, _shader: u32| admission.accepted.contains(&handle);
    let Some(mut cleared) = objects::clear_objects(&mut m, LIGHTMAP_LAYER, &accept) else {
        return;
    };
    if cleared.walk.write_failed != 0 {
        objects::restore(&mut m, &cleared);
        return;
    }
    let mut frames = Vec::with_capacity(cleared.handles.len());
    for &handle in &cleared.handles {
        let Some(&model) = admission.models.get(&handle) else {
            continue;
        };
        if let Some(mut frame) = objects::read_object(&m, handle) {
            frame.definition = model;
            if ready(&m, &set, &cleared, &mut frame) {
                frames.push(frame);
            }
        }
    }
    let drawn: HashSet<u32> = frames
        .iter()
        .filter(|frame| !set.shadowed.contains(&(frame.definition as u16)))
        .map(|frame| frame.handle)
        .collect();
    objects::keep_only(&mut m, &mut cleared, &|handle| drawn.contains(&handle));
    if frames.is_empty() {
        return;
    }
    let count = frames.len() as u64;
    if !host.draw(frames) {
        REFUSED.fetch_add(1, Ordering::Relaxed);
        objects::restore(&mut m, &cleared);
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    OBJECTS.fetch_add(count, Ordering::Relaxed);
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
