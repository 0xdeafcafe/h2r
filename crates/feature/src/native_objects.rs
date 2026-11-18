//! `native-objects`: objects' render models drawn by the native renderer
//! instead of their layer-3 entries (docs/graphics/native-renderer.md,
//! "Stage 2: objects and skinning"). Off by default;
//! `H2_FEATURES=impl.render.native_objects=on` turns it on (exactly that
//! key: `impl.render=on` does not, since this one changes what draws the
//! objects).
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), an
//! opaque point, so nothing here replaces guest code:
//!
//! - first it sets back the bits it cleared at the last base-layer layer 3
//!   (the layers after 3, 5 and up, come next in the same view), each only
//!   if the item still holds the mask it left ([`restore_pending`]);
//! - then, when the current layer is 3 and the call is the base layers'
//!   (`[esp] == 0x0002bed5`): ask the [`Host`] which object definitions and
//!   shaders it draws, clear layer 3's bit on those objects' items
//!   ([`native_render::objects::clear_objects`]), copy each cleared
//!   object's node matrices and sphere out of guest memory, and hand the
//!   copies with the view's camera to the host for one external pass.
//!   Objects that cannot be read or whose node count disagrees with their
//!   model get their bits back at once (the D3D path draws them); when the
//!   host cannot draw, all do.
//!
//! Nothing runs after the body, and the observer is context-only, so the
//! pipelined render runs it on its own thread
//! (docs/graphics/pipelined-render.md, "Render-safe hooks"). There the
//! memory it reads is the render's snapshot of the frame: the node
//! matrices are copied out of the same frame the rest of the render draws,
//! whatever the next tick does to the bodies or the pool meanwhile, and
//! the pass carries the copies (nothing reads guest memory after the
//! hook). The host's pass request goes to the D3D device in render order.
//!
//! It sits inside `render-timing`'s observers and the world's
//! (`after_feature`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::{GuestRead, GuestWrite, RENDER_CAMERA, RenderCamera};
use native_render::objects::{self, ClearedObjects, ObjectFrame, SCENE_LAYER3_RETURN};
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER};

use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-objects";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_objects";

/// What the host draws natively right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeSet {
    /// Object definitions by tag index (the low word of the body's `+0`
    /// datum), each with its model's node count.
    pub definitions: HashMap<u16, usize>,
    /// Shaders by tag index (the low word of an item's `+8`): the
    /// materials the pass draws; the rest keep their D3D draws.
    pub shaders: HashSet<u16>,
}

impl NativeSet {
    pub fn accepts(&self, definition: u32, shader: u32) -> bool {
        self.definitions.contains_key(&(definition as u16)) && self.shaders.contains(&(shader as u16))
    }

    pub fn node_count(&self, definition: u32) -> Option<usize> {
        self.definitions.get(&(definition as u16)).copied()
    }
}

/// What draws the objects: boot's glue over the GPU models and the device.
pub trait Host: Send + Sync {
    /// Before the base layers' layer 3 (`mem` read only): the set drawn
    /// natively now, or `None` to leave every object to the D3D path.
    fn native(&self, mem: &dyn GuestMemory) -> Option<Arc<NativeSet>>;
    /// A definition met in layer 3 that the set does not hold: load its
    /// model if it has not been tried (cheap to call again).
    fn want(&self, definition: u32);
    /// Asks the device for the view's pass over `objects`. `false`: not
    /// drawn (the skip is undone).
    fn draw(&self, camera: &RenderCamera, objects: Vec<ObjectFrame>) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last base-layer layer 3, with each item's mask as
/// it was left, until the next layer draw sets them back (one render runs
/// at a time, so that is the same render's).
static CLEARED: Mutex<Option<(ClearedObjects, Vec<u32>)>> = Mutex::new(None);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static OBJECTS: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Base-layer layer-3 views seen, views drawn natively, objects drawn,
/// items skipped.
pub fn counts() -> (u64, u64, u64, u64) {
    (
        VIEWS.load(Ordering::Relaxed),
        DRAWN.load(Ordering::Relaxed),
        OBJECTS.load(Ordering::Relaxed),
        ITEMS.load(Ordering::Relaxed),
    )
}

/// The observer before the layer draw; `ret` is the layer draw's return
/// address (`[esp]` at entry).
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    let mut m = Mem(mem);
    if ret != SCENE_LAYER3_RETURN || m.read_u32(CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let Some(camera) = RenderCamera::read(&m, RENDER_CAMERA) else {
        return;
    };
    let Some(set) = host.native(&*m.0) else {
        return;
    };
    let accept = |_handle: u32, definition: u32, shader: u32| {
        if set.accepts(definition, shader) {
            return true;
        }
        if !set.definitions.contains_key(&(definition as u16)) {
            host.want(definition);
        }
        false
    };
    let Some(mut cleared) = objects::clear_objects(&mut m, LIGHTMAP_LAYER, &accept) else {
        return;
    };
    if cleared.items.is_empty() {
        return;
    }
    let mut frames = Vec::with_capacity(cleared.handles.len());
    for &h in &cleared.handles {
        if let Some(f) = objects::read_object(&m, h)
            && set.node_count(f.definition) == Some(f.nodes.len())
        {
            frames.push(f);
        }
    }
    let drawn: HashSet<u32> = frames.iter().map(|f| f.handle).collect();
    objects::keep_only(&mut m, &mut cleared, &|h: u32| drawn.contains(&h));
    if frames.is_empty() {
        return;
    }
    let count = frames.len() as u64;
    if !host.draw(&camera, frames) {
        objects::restore(&mut m, &cleared);
        return;
    }
    DRAWN.fetch_add(1, Ordering::Relaxed);
    OBJECTS.fetch_add(count, Ordering::Relaxed);
    ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
    if VIEWS.load(Ordering::Relaxed) % 1800 == 1 {
        let (v, d, o, i) = counts();
        eprintln!("native-objects: {d} of {v} layer-3 views drawn natively, {o} objects, {i} object items skipped");
    }
    let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some((cleared, left));
}

/// Sets back the bits the last base-layer layer 3 cleared, on the items
/// that still hold the mask it left: a list built again since is left
/// alone.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some((c, left)) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << c.layer;
    for (&item, &left) in c.items.iter().zip(&left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

pub struct NativeObjects;

impl Feature for NativeObjects {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Objects' render models drawn and skinned by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().feature(SWITCH, false) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_objects_skip", |o| {
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
