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
//! Admission is whole-object: if any live layer-3 item of an object is not
//! accepted, all of that object's accepted items are restored. This avoids
//! the object encoder drawing a native part beside its own D3D part.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::objects::{self, ClearedObjects, FIRST_PERSON_LAYER3_RETURN, ObjectFrame};
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};

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
    /// Load a definition met in the first-person list (cheap to repeat).
    fn want(&self, definition: u32);
    /// Queue this first-person layer-3 draw. `false` restores every item.
    /// Compared definitions are included but keep their D3D draws.
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

/// The observer before a layer draw. Only `0x0003fdf0`'s layer 3 is owned.
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let mut m = Mem(mem);
    if ret != FIRST_PERSON_LAYER3_RETURN || m.read_u32(CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    CALLS.fetch_add(1, Ordering::Relaxed);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(set) = host.native(&*m.0) else {
        return;
    };

    // A rejected item vetoes its whole object. The object encoder selects
    // parts by section, so drawing just the accepted subset is not exact.
    let rejected = RefCell::new(HashSet::new());
    let accept = |handle: u32, definition: u32, shader: u32| {
        if set.accepts(definition, shader) {
            return true;
        }
        rejected.borrow_mut().insert(handle);
        if !set.definitions.contains_key(&(definition as u16)) {
            host.want(definition);
        }
        false
    };
    let Some(mut cleared) = objects::clear_objects(&mut m, LIGHTMAP_LAYER, &accept) else {
        return;
    };
    let rejected = rejected.into_inner();
    let mut frames = Vec::with_capacity(cleared.handles.len());
    for &handle in &cleared.handles {
        if rejected.contains(&handle) {
            continue;
        }
        if let Some(mut frame) = objects::read_object(&m, handle)
            && crate::native_objects::ready(&m, &set, &cleared, &mut frame)
        {
            frames.push(frame);
        }
    }
    let drawn: HashSet<u32> = frames
        .iter()
        .filter(|f| !set.shadowed.contains(&(f.definition as u16)))
        .map(|f| f.handle)
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
    let (calls, drawn, objects, items, refused) = counts();
    if calls % 1800 == 1 {
        eprintln!(
            "native-first-person: {drawn} native draws of {calls} first-person layer-3 calls, {objects} objects, {items} items skipped, {refused} queue refusals"
        );
    }
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
