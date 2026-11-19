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
use native_render::objects::{self, ClearedObjects, ObjectFrame, SCENE_LAYER3_RETURN, WalkStats};
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

/// Where a layer-3 call of the observer ended (diagnostics: the first of
/// each prints a line with its detail, and every 1,800 layer-3 calls a
/// line counts them all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Layer 3, but `[esp]` is not the base layers' call `0x0002bed5`.
    WrongCaller,
    NoHost,
    /// The render camera record does not read as a camera.
    NoCamera,
    /// The host has no set (no loaded map name).
    NoMap,
    /// The current list does not read.
    NoList,
    /// The walk met no item whose handle names a live object.
    NoObjectItems,
    /// Live object items, none accepted (models not loaded yet, or
    /// refused, or their shaders not native).
    NoneAccepted,
    /// Accepted items, but no object read with its model's node count.
    NoneRead,
    /// The host could not ask the device for the pass.
    DrawRefused,
    Drawn,
}

impl Stop {
    pub const ALL: [Stop; 10] = [
        Stop::WrongCaller,
        Stop::NoHost,
        Stop::NoCamera,
        Stop::NoMap,
        Stop::NoList,
        Stop::NoObjectItems,
        Stop::NoneAccepted,
        Stop::NoneRead,
        Stop::DrawRefused,
        Stop::Drawn,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Stop::WrongCaller => "wrong caller",
            Stop::NoHost => "no host",
            Stop::NoCamera => "no camera",
            Stop::NoMap => "no map",
            Stop::NoList => "no list",
            Stop::NoObjectItems => "no object items",
            Stop::NoneAccepted => "none accepted",
            Stop::NoneRead => "none read",
            Stop::DrawRefused => "draw refused",
            Stop::Drawn => "drawn",
        }
    }
}

static STOPS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
/// Layer-3 calls of the observer, any caller.
static LAYER3: AtomicU64 = AtomicU64::new(0);
/// Definitions handed to `Host::want`.
static WANTED: AtomicU64 = AtomicU64::new(0);
/// The walks' counts, summed.
static WALKS: Mutex<Option<WalkStats>> = Mutex::new(None);
/// Distinct wrong callers printed so far (at most four).
static CALLERS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// How often each [`Stop`] happened, in [`Stop::ALL`] order.
pub fn stops() -> [u64; 10] {
    std::array::from_fn(|i| STOPS[i].load(Ordering::Relaxed))
}

/// The count of one [`Stop`].
pub fn stop_count(s: Stop) -> u64 {
    STOPS[s as usize].load(Ordering::Relaxed)
}

/// Layer-3 calls seen and definitions wanted.
pub fn diagnostics() -> (u64, u64) {
    (LAYER3.load(Ordering::Relaxed), WANTED.load(Ordering::Relaxed))
}

/// Counts `s`; its first time prints `detail`.
fn stop(s: Stop, detail: impl FnOnce() -> String) {
    if STOPS[s as usize].fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!("native-objects: first layer-3 call ending in \"{}\": {}", s.name(), detail());
    }
}

fn add_walk(w: &WalkStats) {
    let mut sum = WALKS.lock().unwrap_or_else(|e| e.into_inner());
    let t = sum.get_or_insert_with(WalkStats::default);
    t.walked += w.walked;
    t.in_layer += w.in_layer;
    t.geometry += w.geometry;
    t.object_handles += w.object_handles;
    t.live += w.live;
    t.accepted += w.accepted;
    t.write_failed += w.write_failed;
    for &(b, n) in &w.other_builds {
        match t.other_builds.iter().position(|(x, _)| *x == b) {
            Some(i) => t.other_builds[i].1 += n,
            None if t.other_builds.len() < 4 => t.other_builds.push((b, n)),
            None => {}
        }
    }
}

fn walk_text(w: &WalkStats) -> String {
    let builds: Vec<String> = w.other_builds.iter().map(|(b, n)| format!("{b:#010x} x{n}")).collect();
    format!(
        "items {} in layer {} geometry {} with handles {} live {} accepted {} write failed {}, other builds [{}]",
        w.walked,
        w.in_layer,
        w.geometry,
        w.object_handles,
        w.live,
        w.accepted,
        w.write_failed,
        builds.join(", ")
    )
}

/// The summary line every 1,800 layer-3 calls.
pub fn summary() -> String {
    let counts: Vec<String> = Stop::ALL
        .iter()
        .map(|&s| format!("{} {}", s.name(), stop_count(s)))
        .collect();
    let walk = WALKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or_else(|| "no walks".to_owned(), walk_text);
    let (l3, wanted) = diagnostics();
    format!("native-objects: {l3} layer-3 calls: {}; {wanted} definitions wanted; walks: {walk}", counts.join(", "))
}

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
    if m.read_u32(CURRENT_LAYER) != Some(LIGHTMAP_LAYER) {
        return;
    }
    if LAYER3.fetch_add(1, Ordering::Relaxed) % 1800 == 1 {
        eprintln!("{}", summary());
    }
    if ret != SCENE_LAYER3_RETURN {
        stop(Stop::WrongCaller, || format!("[esp] {ret:#010x}, the base layers' call returns to {SCENE_LAYER3_RETURN:#010x}"));
        let mut seen = CALLERS.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.contains(&ret) && seen.len() < 4 {
            seen.push(ret);
            eprintln!("native-objects: layer 3 called with [esp] {ret:#010x} (not the base layers'; left to D3D)");
        }
        return;
    }
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        stop(Stop::NoHost, || "no host installed".to_owned());
        return;
    };
    let Some(camera) = RenderCamera::read(&m, RENDER_CAMERA) else {
        stop(Stop::NoCamera, || format!("the render camera at {RENDER_CAMERA:#010x} does not read as a camera"));
        return;
    };
    let Some(set) = host.native(&*m.0) else {
        stop(Stop::NoMap, || "the host has no native set (no loaded map name)".to_owned());
        return;
    };
    let accept = |_handle: u32, definition: u32, shader: u32| {
        if set.accepts(definition, shader) {
            return true;
        }
        if !set.definitions.contains_key(&(definition as u16)) {
            WANTED.fetch_add(1, Ordering::Relaxed);
            host.want(definition);
        }
        false
    };
    let Some(mut cleared) = objects::clear_objects(&mut m, LIGHTMAP_LAYER, &accept) else {
        stop(Stop::NoList, || "the current list does not read".to_owned());
        return;
    };
    add_walk(&cleared.walk);
    if cleared.items.is_empty() {
        let w = walk_text(&cleared.walk);
        if cleared.walk.live == 0 {
            stop(Stop::NoObjectItems, || w);
        } else {
            let (d, sh) = (set.definitions.len(), set.shaders.len());
            stop(Stop::NoneAccepted, || format!("{w}; the set has {d} definitions, {sh} shaders"));
        }
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
    let accepted = cleared.handles.len();
    objects::keep_only(&mut m, &mut cleared, &|h: u32| drawn.contains(&h));
    if frames.is_empty() {
        stop(Stop::NoneRead, || {
            format!("{accepted} accepted objects, none read with its model's node count (node block at body +0x114/+0x116)")
        });
        return;
    }
    let count = frames.len() as u64;
    if !host.draw(&camera, frames) {
        stop(Stop::DrawRefused, || "the device cannot take an external pass".to_owned());
        objects::restore(&mut m, &cleared);
        return;
    }
    stop(Stop::Drawn, || format!("{count} objects, {} items", cleared.items.len()));
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
