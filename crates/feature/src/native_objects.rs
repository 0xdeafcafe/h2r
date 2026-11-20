//! `native-objects`: objects' render models drawn by the native renderer
//! instead of their layer-3 entries (docs/graphics/native-renderer.md,
//! "Stage 2: objects and skinning"). On by default (since 2026-11-20);
//! `H2_FEATURES=impl.render.native_objects=off` turns it off for the
//! comparison against the D3D path (exactly that key: `impl.render=off`
//! does not, since this one changes what draws the objects).
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
//!   ([`native_render::objects::clear_objects`]: by item, the part and
//!   implementation its record picked, `objects::ItemKey`), copy each
//!   cleared object's node matrices, sphere and lighting and each item's
//!   node rows out of guest memory, and hand them, in the list's order,
//!   with the view's camera to the host, which plans every item's draw
//!   before the hook returns and says which it draws ([`Host::draw`]).
//!   Exactly those keep their bits cleared: an item whose object, rows or
//!   texture accumulation target do not read, or whose draw the host could
//!   not resolve, gets its bit back before the layer draws.
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
use native_render::objects::{self, ClearedObjects, FIRST_PERSON_LAYER3_RETURN, ItemDraw, ItemKey, ObjectFrame, SCENE_LAYER3_RETURN, WalkStats};
use native_render::skip::{CURRENT_LAYER, LIGHTMAP_LAYER, MaskPlan, MaskPreview};
use timeline::native::{Metric, Stage, Timer};

use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-objects";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_objects";

/// What the host draws natively right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeSet {
    /// The map the set is for (the per-map counts start again at another).
    pub map: String,
    /// The load the set belongs to: the host's models for one map, BSP
    /// and load (`Host::draw` refuses a view planned against another).
    pub generation: u64,
    /// Object definitions by tag index (the low word of the body's `+0`
    /// datum), each with its model's node count.
    pub definitions: HashMap<u16, usize>,
    /// The parts the host draws, by (definition, section, part): what a
    /// layer-3 item's render record names ([`objects::ItemKey`]). An item
    /// is offered to the host only when its shader is the part's and its
    /// record's implementation one the part has; the rest keep their D3D
    /// draws.
    pub parts: HashMap<(u16, u16, u8), SetPart>,
    /// Definitions whose objects the host is handed as drawn ones while
    /// their items keep their D3D draws: the host compares its uniforms
    /// with the D3D path's draws of them (`H2_NATIVE_OBJECTS_COMPARE` in
    /// the boot glue) and does not draw them.
    pub shadowed: HashSet<u16>,
}

/// One native part: its material's shader (tag index), the
/// implementations it has (bit `n`), and those of them whose program
/// samples the texture accumulation target (drawn only while the game has
/// it bound, [`native_render::object_nodes::texaccum_bound`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetPart {
    pub shader: u16,
    pub implementations: u8,
    pub texaccum: u8,
}

impl NativeSet {
    /// The part an item of `definition` with `key` draws, when the host
    /// draws it with the key's shader and implementation.
    pub fn part(&self, definition: u32, key: &ItemKey) -> Option<SetPart> {
        if !self.definitions.contains_key(&(definition as u16)) {
            return None;
        }
        let p = *self.parts.get(&(definition as u16, key.section, key.part))?;
        (p.shader == key.shader as u16 && key.implementation < 8 && p.implementations & (1 << key.implementation) != 0).then_some(p)
    }

    pub fn accepts(&self, definition: u32, key: &ItemKey) -> bool {
        self.part(definition, key).is_some()
    }

    pub fn node_count(&self, definition: u32) -> Option<usize> {
        self.definitions.get(&(definition as u16)).copied()
    }
}

/// Why an accepted item is given back before the host sees it, or `None`:
/// its object did not read with its model's node count, its record's node
/// rows do not read, or its part samples the texture accumulation target
/// and the game has none bound. `rows`: its record's.
pub fn item_refusal(
    m: &dyn GuestRead,
    set: &NativeSet,
    definition: u32,
    key: &ItemKey,
    frame_ok: bool,
    rows: &Result<Vec<native_render::object_nodes::Row>, String>,
) -> Option<String> {
    if !frame_ok {
        return Some("its object does not read with its model's node count".into());
    }
    let Some(part) = set.part(definition, key) else {
        return Some("not a native part".into());
    };
    if let Err(e) = rows {
        return Some(e.clone());
    }
    if part.texaccum & (1 << key.implementation) != 0 && !native_render::object_nodes::texaccum_bound(m) {
        return Some("it samples the texture accumulation target and none is bound".into());
    }
    None
}

/// Original list identities and tentative mask changes. Applying a reservation's
/// per-item result restores every other candidate in the overlay before writing RAM.
pub struct ItemMasks {
    plan: MaskPlan,
    candidates: ClearedObjects,
    // Places in the complete candidate list for the items the host received.
    // Comparison items can never be suppressed.
    offered: Vec<Option<usize>>,
}

impl ItemMasks {
    pub fn apply(self, mem: &mut dyn GuestMemory, drawn: &[bool]) -> Option<ClearedObjects> {
        if drawn.len() != self.offered.len() { return None; }
        let mut keep = HashSet::new();
        for (original, &accepted) in self.offered.iter().zip(drawn) {
            if accepted { keep.insert((*original)?); }
        }
        if keep.is_empty() { return None; }
        let mut guest = Mem(mem);
        let mut overlay = self.plan.edit(&guest);
        let mut candidates = self.candidates;
        objects::keep_items(&mut overlay, &mut candidates, &|i| keep.contains(&i));
        let selected = overlay.finish()?;
        selected.apply(&mut guest).then_some(candidates)
    }
}

/// Layer planning may leave every guest item live for resolved-draw admission.
pub enum Drawn {
    Masked(ClearedObjects),
    Queued { objects: usize, items: usize },
}

/// What draws the objects: boot's glue over the GPU models and the device.
pub trait Host: Send + Sync {
    /// Drain work preceding the base layers' layer 3. Return the set drawn
    /// natively now, or `None` to leave every object to the D3D path.
    fn native(&self, mem: &mut dyn GuestMemory) -> Option<Arc<NativeSet>>;
    /// A definition met in layer 3 that the set does not hold: load its
    /// model if it has not been tried (cheap to call again).
    fn want(&self, definition: u32);
    /// Plans the view's pass over `items` (in the list's order, each with
    /// its object's frame in `objects`) against the set of `generation`,
    /// then either reserves every required GPU copy before applying `masks`
    /// or matches the planned items to their original D3D draws as they resolve.
    /// An item is drawn natively only when everything its
    /// draw needs resolved (its model's GPU side, its part and
    /// implementation, every per-object constant, every state field); the
    /// rest, and every item when no pass is asked for, get their bits back
    /// before the layer draws. A `shadow` item is never drawn.
    fn draw(
        &self, mem: &mut dyn GuestMemory, camera: &RenderCamera, generation: u64,
        objects: Vec<ObjectFrame>, items: Vec<ItemDraw>, masks: ItemMasks,
    ) -> Option<Drawn>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Bits cleared at the last base-layer layer 3, with each item's mask as
/// it was left, until the next layer draw sets them back (one render runs
/// at a time, so that is the same render's).
static CLEARED: Mutex<Option<(objects::ClearedObjects, Vec<u32>)>> = Mutex::new(None);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static OBJECTS: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);

/// Where a layer-3 call of the observer ended (diagnostics: the first of
/// each prints a line with its detail, and every 1,800 layer-3 calls a
/// line counts them all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Layer 3 from the first-person draw (`[esp]` `0x00040136`): its own
    /// view and camera, left to D3D until first person is a stage of its
    /// own.
    FirstPerson,
    /// Layer 3, but `[esp]` is neither the base layers' call `0x0002bed5`
    /// nor the first-person one.
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
    /// Accepted items, but none whose object, rows and texture
    /// accumulation target read ([`item_refusal`]).
    NoneRead,
    /// The host drew none of the items (it could not plan or ask for the
    /// pass).
    DrawRefused,
    Drawn,
}

impl Stop {
    pub const ALL: [Stop; 11] = [
        Stop::FirstPerson,
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
            Stop::FirstPerson => "first person",
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

static STOPS: [AtomicU64; 11] = [const { AtomicU64::new(0) }; 11];
/// Layer-3 calls of the observer, any caller.
static LAYER3: AtomicU64 = AtomicU64::new(0);
/// Definitions handed to `Host::want`.
static WANTED: AtomicU64 = AtomicU64::new(0);
/// The walks' counts, summed.
static WALKS: Mutex<Option<WalkStats>> = Mutex::new(None);

/// Per map: live object items by the layer-3 implementation their records
/// pick, the views counted, and whether they were logged.
static IMPLEMENTATION_COUNTS: Mutex<Option<(String, [u64; native_render::object_nodes::IMPLEMENTATIONS], u64, bool)>> = Mutex::new(None);

/// Views counted before a map's implementation counts are logged, once.
const IMPLEMENTATION_VIEWS: u64 = 600;

/// Adds one walk's implementation counts for `map`, starting again at a
/// new map; logs them once per map, after [`IMPLEMENTATION_VIEWS`] views.
fn count_implementations(map: &str, w: &WalkStats) {
    let mut c = IMPLEMENTATION_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    if c.as_ref().is_none_or(|c| c.0 != map) {
        *c = Some((map.to_owned(), [0; native_render::object_nodes::IMPLEMENTATIONS], 0, false));
    }
    let Some((_, counts, views, logged)) = c.as_mut() else { return };
    for (t, n) in counts.iter_mut().zip(w.implementations) {
        *t += u64::from(n);
    }
    *views += 1;
    if !*logged && *views >= IMPLEMENTATION_VIEWS {
        *logged = true;
        let each: Vec<String> = counts.iter().enumerate().map(|(i, n)| format!("{i}: {n}")).collect();
        eprintln!("native-objects: {map}: layer-3 object items by the implementation their record picks (+0 bits 29..31), over {views} views: {}", each.join(", "));
    }
}
/// Distinct wrong callers printed so far (at most four).
static CALLERS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// How often each [`Stop`] happened, in [`Stop::ALL`] order.
pub fn stops() -> [u64; 11] {
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
    t.object_records += w.object_records;
    t.live += w.live;
    t.accepted += w.accepted;
    t.write_failed += w.write_failed;
    for (a, n) in t.implementations.iter_mut().zip(w.implementations) {
        *a += n;
    }
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
        "items {} in layer {} geometry {} object records {} live {} accepted {} write failed {}, other builds [{}]",
        w.walked,
        w.in_layer,
        w.geometry,
        w.object_records,
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

/// Base-layer layer-3 views seen, views planned, objects planned and
/// guest items skipped by masks.
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
    if ret == FIRST_PERSON_LAYER3_RETURN {
        stop(Stop::FirstPerson, || {
            format!("[esp] {ret:#010x}, the first-person draw's layer 3 (its own view and camera): left to D3D")
        });
        return;
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
    let Some(set) = host.native(m.0) else {
        stop(Stop::NoMap, || "the host has no native set at this boundary".to_owned());
        return;
    };
    let planning = Timer::start(Stage::Objects, Metric::PrepareNs);
    let Some(camera) = RenderCamera::read(&m, RENDER_CAMERA) else {
        stop(Stop::NoCamera, || format!("the render camera at {RENDER_CAMERA:#010x} does not read as a camera"));
        return;
    };
    let accept = |_handle: u32, definition: u32, key: &ItemKey| {
        if set.accepts(definition, key) {
            return true;
        }
        if !set.definitions.contains_key(&(definition as u16)) {
            WANTED.fetch_add(1, Ordering::Relaxed);
            host.want(definition);
        }
        false
    };
    let Some(mut preview) = MaskPreview::new(&m) else {
        stop(Stop::NoList, || "the current list does not read".to_owned());
        return;
    };
    let Some(cleared) = objects::clear_objects(&mut preview, LIGHTMAP_LAYER, &accept) else {
        stop(Stop::NoList, || "the current list does not read".to_owned());
        return;
    };
    let unowned = u64::from(cleared.walk.refused_ownership());
    Stage::Objects.count(Metric::Examined, unowned);
    Stage::Objects.count(Metric::RefusedOwnership, unowned);
    add_walk(&cleared.walk);
    count_implementations(&set.map, &cleared.walk);
    if cleared.items.is_empty() {
        let w = walk_text(&cleared.walk);
        if cleared.walk.live == 0 {
            stop(Stop::NoObjectItems, || w);
        } else {
            let (d, p) = (set.definitions.len(), set.parts.len());
            stop(Stop::NoneAccepted, || format!("{w}; the set has {d} definitions, {p} parts"));
        }
        return;
    }
    // Each accepted object read once, with its model's node count.
    let mut frames: Vec<ObjectFrame> = Vec::with_capacity(cleared.handles.len());
    let mut frame_of: HashMap<u32, Option<usize>> = HashMap::new();
    for &h in &cleared.handles {
        let f = objects::read_object(&m, h).filter(|f| set.node_count(f.definition) == Some(f.nodes.len()));
        frame_of.insert(h, f.map(|f| {
            frames.push(f);
            frames.len() - 1
        }));
    }
    // Each accepted item, in the list's order, with its record's rows;
    // the ones that cannot be drawn go back now.
    let mut items = Vec::with_capacity(cleared.items.len());
    for (i, (&owner, key)) in cleared.owners.iter().zip(&cleared.keys).enumerate() {
        let object = frame_of.get(&owner).copied().flatten();
        let definition = object.map_or(0, |o| frames[o].definition);
        let rows = objects::record_rows(&m, key.record);
        if item_refusal(&m, &set, definition, key, object.is_some(), &rows).is_some() {
            continue;
        }
        let (Some(object), Ok(rows)) = (object, rows) else { continue };
        let shadow = set.shadowed.contains(&(definition as u16));
        items.push(ItemDraw { index: i, object, key: *key, rows, shadow });
    }
    let unresolved = (cleared.items.len() - items.len()) as u64;
    Stage::Objects.count(Metric::Examined, unresolved);
    Stage::Objects.count(Metric::RefusedInputs, unresolved);
    if items.is_empty() {
        let accepted = cleared.items.len();
        stop(Stop::NoneRead, || format!("{accepted} accepted items, none whose object, rows and texture accumulation target read"));
        return;
    }
    let count = items.len();
    let Some(plan) = preview.finish() else {
        Stage::Objects.count(Metric::Examined, count as u64);
        Stage::Objects.count(Metric::RefusedMasks, count as u64);
        return;
    };
    let masks = ItemMasks {
        plan, candidates: cleared, offered: items.iter().map(|d| (!d.shadow).then_some(d.index)).collect(),
    };
    drop(planning);
    let Some(drawn) = host.draw(m.0, &camera, set.generation, frames, items, masks) else {
        stop(Stop::DrawRefused, || format!("the host drew none of {count} items"));
        return;
    };
    let (objects, items, skipped) = match drawn {
        Drawn::Masked(cleared) => {
            let counts = (cleared.handles.len(), cleared.items.len(), cleared.items.len());
            let left = cleared.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
            *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some((cleared, left));
            counts
        }
        Drawn::Queued { objects, items } => (objects, items, 0),
    };
    stop(Stop::Drawn, || format!("{objects} objects, {items} of {count} items planned"));
    DRAWN.fetch_add(1, Ordering::Relaxed);
    OBJECTS.fetch_add(objects as u64, Ordering::Relaxed);
    ITEMS.fetch_add(skipped as u64, Ordering::Relaxed);
    if VIEWS.load(Ordering::Relaxed) % 1800 == 1 {
        let (v, d, o, i) = counts();
        eprintln!("native-objects: {d} of {v} layer-3 views planned, {o} objects, {i} object items skipped");
    }
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

/// Drops the pending restore without setting anything back: a snapshot
/// load, whose guest RAM is not the memory those bits were cleared in.
/// The loaded item lists are the loaded render's, built again each frame.
pub fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

pub struct NativeObjects;

impl Feature for NativeObjects {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Objects' render models drawn and skinned by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Switched off: no hooks, nothing held. On: the host's models are
        // keyed by the map name it reads from guest RAM at every ask, so
        // a load re-keys them; the counters and logged-once sets only
        // count. The one thing held for the guest is `CLEARED`, the bits
        // to set back at the next layer draw, which a load drops
        // ([`forget_pending`]).
        if !b.switches().feature(SWITCH, true) {
            b.snapshot(crate::SnapshotPolicy::Stateless);
            return Ok(());
        }
        b.snapshot(crate::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_pending();
            Ok(())
        }));
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_objects_skip", |o| {
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        Ok(())
    }
}
