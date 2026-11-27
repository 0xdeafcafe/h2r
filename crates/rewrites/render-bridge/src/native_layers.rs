//! `frame-scene.layers`: the scene's world layers 1, 5, 7, 8, 20, 2 and 17, the
//! alternate view's layers 1, 5 and 2, the first person's layers 1, 7, 8, 20 and 2, and the sorted transparent list's
//! camouflage layers 18 and 19 and hologram layer 21, drawn from the scene's plan instead of the guest's
//! draws under the layer call (docs/graphics/framescene.md, slice 4;
//! docs/graphics/native-water-fog.md for 18, 19 and 21). On with the native renderer;
//! `frame-scene.layers=off` leaves every layer to the D3D path.
//!
//! One observer before the layer draw `LAYER_DRAW` (`0x00044550`), as
//! `native_fog`'s:
//!
//! - first it sets back the bits it cleared at the last layer it took,
//!   each only if the item still holds the mask it left;
//! - then, when the current layer is one of [`LAYERS`] and the caller is
//!   the scene's, the alternate view's layers 1, 5 or 2, one of [`FIRST_PERSON_LAYERS`] and the caller the
//!   first person's `0x0003fdf0` (`native_render::world_layers::layer_site`),
//!   or one of [`SORTED_LAYERS`] and the caller its sorted handler
//!   (`native_render::world_layers::sorted_site`),
//!   it asks the [`Host`] (boot's glue: the plan, the GPU layer sets, the
//!   object models and the D3D device) for the native pass; the host
//!   clears the layer's bit on the items it draws, from the list's first,
//!   and the guest draws the rest.
//!
//! The scene's and the alternate view's layers 1, 2, 5, 7, 8, 17 and 20
//! ([`SCENE_ONLY`]) are cut to the scene: an item the host does not draw
//! keeps its guest draw, and is counted by layer, kind and the host's
//! cause, `scene_kept_<site><layer>_<kind>_<cause>` ([`count_kept`]), until
//! the scene draws it. No item is dropped: an object or a wall the pass
//! refuses still draws. A layer 2 or 17 draw from a caller this stage does
//! not take ([`UNTAKEN_CALLER`]) keeps every item, counted under the cause
//! `caller`. The first person's and the sorted handlers' layers
//! keep what the host refuses to D3D, counted under `layer_plan_*`.
//!
//! The plan's outcomes are counted by cause for `render.scene`
//! ([`counters`]).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use frame_scene::{FrameScene, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};
use native_render::camera::{GuestRead, GuestWrite};
use native_render::skip::{self, Cleared};
use native_render::world_layers::{Site, SortedHandler, layer_site, sorted_site};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "frame-scene.layers";

/// The scene's layers taken: 1, 5, 7, 8 and 20 around layer 3, the
/// overlay layer 2 after the dynamic lights (`layer_pass::OVERLAY_LAYER`)
/// and the late layer 17 (`layer_pass::BLOOM_LAYER`).
pub const LAYERS: [u32; 7] = [1, 5, 7, 8, 20, 2, 17];

/// The layers that draw only from the scene when `caller` is the scene's or
/// the alternate view's (`layer_pass::WORLD_LAYERS` less layer 3, which
/// `native_world` owns). The alternate view has no layer 17.
pub const SCENE_ONLY: [u32; 7] = [1, 2, 5, 7, 8, 17, 20];

/// The layers whose draws from a caller [`caller`] does not name keep
/// every item, each counted `scene_kept_<layer>_<kind>_caller`: R5's
/// overlay and late layers (docs/graphics/retire-d3d.md, "By caller").
pub const UNTAKEN_CALLER: [u32; 2] = [2, 17];

/// The cause an untaken caller's items are counted under.
pub const CALLER_CAUSE: &str = "caller";

/// Whether `layer` of `caller` is cut to the scene: its kept guest draws are counted `scene_kept_*`.
pub fn scene_only(layer: u32, caller: LayerCaller) -> bool {
    matches!(caller, LayerCaller::Scene | LayerCaller::AlternateView) && SCENE_ONLY.contains(&layer)
}

/// The first person's layers taken: `0x0003fdf0`'s calls besides layer
/// 3, which `native_first_person` owns. It calls layers 1, 3, 8, 20 and
/// 7, then layer 2 when `0x00044370` says so (`0x000400a6..0x00040284`);
/// it has no layer 5.
pub const FIRST_PERSON_LAYERS: [u32; 5] = [1, 7, 8, 20, 2];

/// The sorted transparent list's handlers' layers taken: the
/// camouflage handler `0x00041cc0`'s 18 and 19 (19 drawn into render
/// target 3, which the begin binds before the layer draw) and the
/// hologram handler `0x000423c0`'s 21. Their items are object render
/// records only. Regular layer replays and ranges this plan leaves behind
/// reach the sorted indexed admission.
pub const SORTED_LAYERS: [u32; 3] = [18, 19, 21];

/// Which caller's layer draw is about to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerCaller {
    /// The scene's (`0x0002bcd0`): structure and object items.
    Scene,
    /// The alternate branch of the scene: structure and object items.
    AlternateView,
    /// The first person's (`0x0003fdf0`): its arms' and weapon's items.
    FirstPerson,
    /// A sorted transparent entry's handler (`0x00041cc0`, `0x000423c0`):
    /// the scene's object items it adds for its records.
    Sorted(SortedHandler),
}

/// The caller of `layer` this stage takes, from the layer draw's return
/// address; `None` for every other.
pub fn caller(layer: u32, ret: u32) -> Option<LayerCaller> {
    if let Some(handler) = sorted_site(layer, ret).filter(|_| SORTED_LAYERS.contains(&layer)) {
        return Some(LayerCaller::Sorted(handler));
    }
    match layer_site(layer, ret) {
        Site::Scene if LAYERS.contains(&layer) => Some(LayerCaller::Scene),
        Site::AlternateView if matches!(layer, 1 | 2 | 5) => Some(LayerCaller::AlternateView),
        Site::FirstPerson if FIRST_PERSON_LAYERS.contains(&layer) => Some(LayerCaller::FirstPerson),
        _ => None,
    }
}

/// Whether the stage is placed: with the native renderer on, unless
/// `frame-scene.layers=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches) && switches.feature(ID, true)
}

/// What draws the layers: boot's glue over the plan, the GPU world and
/// the device.
pub trait Host: Send + Sync {
    /// Before `layer` of a scene view or of the first person (`caller`;
    /// `mem` the guest; `scene` one view holding the layer's one pass,
    /// extracted here, [`extract`]): plans the layer's items, clears the
    /// bit of those it draws over a preview, reserves the native
    /// replacement, applies the masks and commits. Returns what was
    /// cleared for the restore at the next layer draw, or `None` to leave
    /// the layer to the D3D path. `left` takes why each item it does not
    /// draw keeps its guest draw ([`Left`]).
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, left: &mut Left) -> Option<Cleared>;

    /// One run of [`RUN_LAYERS`]' sorted entries (`crate::late_sorted`),
    /// `scene` extracted for it: the items from `at.from` the plan draws,
    /// up to the first it keeps, reserved and committed, or why not (in
    /// `left`).
    fn ask_run(
        &self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene,
        at: &crate::late_sorted::RunAt<'_>, left: &mut Left,
    ) -> crate::late_sorted::Run;
}

/// The layers whose items the plan takes but the order rule holds back
/// (`order`: after a kept item they do not commute with) draw per item
/// over the guest's sorted entries, between the items it keeps
/// (`crate::late_sorted`; docs/graphics/retire-d3d-r5.md, "Layer 2 from
/// the plan"): R5's blended overlay and late layers, at the scene's and
/// the alternate view's calls.
pub const RUN_LAYERS: [u32; 2] = [2, 17];

/// A run layer held for its sorted entries, and its caller.
static SORTED: Mutex<Option<crate::late_sorted::Deferred>> = Mutex::new(None);
static SORTED_CALLER: Mutex<Option<LayerCaller>> = Mutex::new(None);

/// The host's ask for a run of the held layer's sorted entries.
fn ask_run(mem: &mut dyn GuestMemory, at: &crate::late_sorted::RunAt<'_>, left: &mut Left) -> crate::late_sorted::Run {
    use crate::late_sorted::Run;
    let caller = *SORTED_CALLER.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(caller), Some(layer)) = (caller, Ro(&*mem).read_u32(skip::CURRENT_LAYER)) else {
        left.all("unread");
        return Run::Refused;
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        left.all("host");
        return Run::Refused;
    };
    let Some(scene) = extract(&*mem, layer as u8, caller) else {
        left.all("unread");
        return Run::Refused;
    };
    host.ask_run(mem, layer, caller, &scene, at, left)
}

/// Holds `layer` of `caller` for its sorted entries when the plan left
/// items to the order rule: every item the guest's layer draw holds, each
/// with its cause as [`count_kept`] names it. `false` when nothing is held.
fn hold_for_runs(mem: &dyn GuestMemory, layer: u32, caller: LayerCaller, left: &mut Left, extracted: Option<&HashSet<u32>>) -> bool {
    if !RUN_LAYERS.contains(&layer) || !scene_only(layer, caller) || !crate::late_sorted::wants(left) {
        return false;
    }
    let ro = Ro(mem);
    let (Some(list), Some(held)) = (ro.read_u32(skip::CURRENT_LIST), native_render::layer_items::held_items(&ro, layer, true)) else {
        return false;
    };
    for h in &held {
        if h.item.is_some() && !left.items.contains_key(&h.slot) && extracted.is_some_and(|e| !e.contains(&h.slot)) {
            left.item(h.slot, "remainder");
        }
    }
    let held = held.into_iter().map(|h| (h.slot, h.kind(), h.item.is_some())).collect();
    let hold = crate::late_sorted::Hold { layer, list, site: site(caller), late: false, held };
    crate::late_sorted::hold(&SORTED, hold, left);
    *SORTED_CALLER.lock().unwrap_or_else(|e| e.into_inner()) = Some(caller);
    true
}

/// Why the host left items of a layer to the guest: by list slot, and for
/// the whole layer when it stopped before naming items (the world not
/// loaded, the device refusing the pass). A slot it names keeps that
/// cause; the rest take the layer's, or `unnamed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Left {
    pub layer: Option<&'static str>,
    pub items: std::collections::HashMap<u32, &'static str>,
}

impl Left {
    /// The cause of slot `slot`: its own, else the layer's, else `unnamed`.
    pub fn cause(&self, slot: u32) -> &'static str {
        self.items.get(&slot).copied().or(self.layer).unwrap_or("unnamed")
    }

    /// Names `slot`'s cause unless one is named already.
    pub fn item(&mut self, slot: u32, why: &'static str) {
        self.items.entry(slot).or_insert(why);
    }

    /// Names the whole layer's cause unless one is named already.
    pub fn all(&mut self, why: &'static str) {
        self.layer.get_or_insert(why);
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static PLACED: AtomicBool = AtomicBool::new(false);
/// What the last taken layer cleared, with each item as it was left
/// ([`ItemState`]).
static CLEARED: Mutex<Option<(Cleared, Vec<ItemState>)>> = Mutex::new(None);

/// An item as the skip left it: its mask (`+0x10`), shader (`+8`) and
/// record argument (`+0xc`). A sorted handler's layer can be the last of
/// its list (the next layer draw follows the next entry's rebuild of the
/// same items), so the bit goes back only on an item that is still the
/// one cleared.
type ItemState = [u32; 3];

fn left_of(m: &dyn GuestRead, item: u32) -> ItemState {
    [0x10, 8, 0xc].map(|at| m.read_u32(item + at).unwrap_or(0))
}
static ASKED: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
/// The plan's outcomes by cause (`taken`, `refused_<cause>`), per layer.
static OUTCOMES: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Whether the hook is placed this session: the world's load compiles
/// the layers' materials only then.
pub fn placed() -> bool {
    PLACED.load(Ordering::Relaxed)
}

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Items of a cut world layer that kept their guest draw, by
/// `<site><layer>_<kind>_<cause>`.
static KEPT: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Counts `n` items of `layer` (`site` `""` for the scene, `alt` for the
/// alternate view) of `kind` (`world`, `object`, `unreadable`) that keep
/// their guest draw under `cause`: `scene_kept_<site><layer>_<kind>_<cause>`
/// in [`counters`].
pub fn count_kept_as(site: &str, layer: u32, kind: &str, cause: &str, n: u64) {
    if n == 0 {
        return;
    }
    *KEPT.lock().unwrap_or_else(|e| e.into_inner()).entry(format!("{site}{layer}_{kind}_{cause}")).or_default() += n;
}

/// The items of the current list still holding `layer`'s bit after the
/// host's clear, each counted by kind and its cause in `left`
/// ([`count_kept_as`]): `record` for an item whose render record does not
/// read, `remainder` for one outside `extracted` (the slots the layer's
/// scene holds) that the host did not name. `objects` false leaves the
/// objects out (layer 3's are the sorted walk's). Nothing is written: they
/// keep their guest draw. A list that does not read is one `unreadable`
/// count under `list`.
pub fn count_kept(mem: &dyn GuestRead, layer: u32, site: &str, objects: bool, left: &Left, extracted: Option<&HashSet<u32>>) {
    let Some(held) = native_render::layer_items::held_items(mem, layer, objects) else {
        count_kept_as(site, layer, "unreadable", "list", 1);
        return;
    };
    for h in held {
        let cause = match h.item {
            None => "record",
            Some(_) if !left.items.contains_key(&h.slot) && extracted.is_some_and(|e| !e.contains(&h.slot)) => "remainder",
            Some(_) => left.cause(h.slot),
        };
        count_kept_as(site, layer, h.kind(), cause, 1);
    }
}

/// The counter site of `caller`: `""` for the scene, `alt` for the
/// alternate view.
fn site(caller: LayerCaller) -> &'static str {
    match caller {
        LayerCaller::AlternateView => "alt",
        _ => "",
    }
}

/// Counts `n` items of `layer` under `outcome` (`taken`, or a refusal's
/// cause): the host reports each ask's plan.
pub fn count(layer: u32, outcome: &str, n: u64) {
    if n == 0 {
        return;
    }
    let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *o.entry(format!("layer_plan_{layer}_{outcome}")).or_default() += n;
}

/// [`count`] for `caller`'s layer: the first person's outcomes under
/// `layer_plan_fp<layer>_<outcome>`, a sorted handler's under
/// `layer_plan_sorted<layer>_<outcome>`.
pub fn count_for(caller: LayerCaller, layer: u32, outcome: &str, n: u64) {
    let prefix = match caller {
        LayerCaller::Scene => return count(layer, outcome, n),
        LayerCaller::AlternateView => "alt",
        LayerCaller::FirstPerson => "fp",
        LayerCaller::Sorted(_) => "sorted",
    };
    if n == 0 {
        return;
    }
    let mut o = OUTCOMES.lock().unwrap_or_else(|e| e.into_inner());
    *o.entry(format!("layer_plan_{prefix}{layer}_{outcome}")).or_default() += n;
}

/// The counters `render.scene` reports: layer calls asked, layer calls
/// drawn natively, items skipped, and the plan's outcomes by layer and
/// cause.
pub fn counters() -> Vec<(String, u64)> {
    let mut out = vec![
        ("layer_plan_asked".to_owned(), ASKED.load(Ordering::Relaxed)),
        ("layer_plan_drawn".to_owned(), DRAWN.load(Ordering::Relaxed)),
        ("layer_plan_items".to_owned(), ITEMS.load(Ordering::Relaxed)),
    ];
    out.extend(OUTCOMES.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (k.clone(), v)));
    out.extend(KEPT.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (format!("scene_kept_{k}"), v)));
    out
}

/// The layer draw about to run as a scene of its own: one view with the
/// render camera and the layer's pass, its items and material inputs
/// (`scene_extract::extract::layer`, each object item with its owner's
/// lighting). Independent of the frame step's scene, whose observer at
/// the same point may run before or after this one.
pub fn extract(mem: &dyn GuestMemory, layer: u8, caller: LayerCaller) -> Option<Arc<FrameScene>> {
    let ro = Ro(mem);
    let camera = scene_extract::extract::camera(&ro)?;
    let mut b = SceneBuilder::default();
    b.begin(SceneId::default());
    let kind = match caller {
        LayerCaller::Scene | LayerCaller::Sorted(_) => ViewKind::Game,
        LayerCaller::AlternateView => ViewKind::Alternate,
        LayerCaller::FirstPerson => ViewKind::FirstPerson,
    };
    b.open_view(ViewKey { kind, player: 0, view: ro.read_u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(0) as u8 }, camera).ok()?;
    scene_extract::extract::layer(&ro, layer, true, &mut b).ok()??;
    b.seal(Stamp::default())
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

struct Mem<'a>(&'a mut dyn GuestMemory);

impl GuestRead for Mem<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

impl GuestWrite for Mem<'_> {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
        self.0.set_u32(addr, value).ok()
    }
}

/// The observer before the layer draw: the last bits back, then this
/// layer's skip. `ret` is the layer draw's return address.
pub fn before_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_pending(mem);
    crate::late_sorted::stale(&SORTED, &*mem);
    let Some(layer) = Mem(mem).read_u32(skip::CURRENT_LAYER) else { return };
    let Some(caller) = caller(layer, ret) else {
        if UNTAKEN_CALLER.contains(&layer) {
            let left = Left { layer: Some(CALLER_CAUSE), ..Left::default() };
            count_kept(&Ro(&*mem), layer, "", true, &left, None);
        }
        return;
    };
    if layer == 7 && caller == LayerCaller::Scene {
        // The object shadows draw in slot 3 ahead of layer 7.
        let shadows = crate::scene_step::open_object_shadow_passes();
        crate::native_lights::object_shadow_scene::draw(mem, &shadows);
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    ASKED.fetch_add(1, Ordering::Relaxed);
    let mut left = Left::default();
    let mut extracted = None;
    let drawn = match extract(&*mem, layer as u8, caller) {
        Some(scene) => {
            extracted = scene.layer_passes.first().map(|p| scene.items_of(p).iter().map(|i| i.slot).collect::<HashSet<u32>>());
            host.ask(mem, layer, caller, &scene, &mut left)
        }
        None => {
            count_for(caller, layer, "refused_unread", 1);
            left.all("unread");
            None
        }
    };
    if let Some(cleared) = drawn.filter(|c| !c.items.is_empty()) {
        DRAWN.fetch_add(1, Ordering::Relaxed);
        ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
        remember(mem, cleared);
    }
    if hold_for_runs(&*mem, layer, caller, &mut left, extracted.as_ref()) {
        // Counted when the layer is done (`crate::late_sorted`).
        return;
    }
    if scene_only(layer, caller) {
        count_kept(&Ro(&*mem), layer, site(caller), true, &left, extracted.as_ref());
    }
}

/// What the layer's call will restore at the next layer draw: `cleared`
/// with each item as it was left, added to what is already pending.
fn remember(mem: &dyn GuestMemory, cleared: Cleared) {
    let ro = Ro(mem);
    let mut pending = CLEARED.lock().unwrap_or_else(|e| e.into_inner());
    let (all, left) = pending.get_or_insert_with(|| (Cleared { layer: cleared.layer, items: Vec::new() }, Vec::new()));
    for item in cleared.items {
        if !all.items.contains(&item) {
            left.push(left_of(&ro, item));
            all.items.push(item);
        }
    }
}

/// Sets back the bits the last taken layer cleared, on the items that
/// still hold the mask, shader and argument it left.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let Some((cleared, left)) = CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << cleared.layer;
    for (&item, &was) in cleared.items.iter().zip(&left) {
        if left_of(&m, item) == was {
            let _ = m.write_u32(item + 0x10, was[0] | bit);
        }
    }
}

pub(crate) fn forget_pending() {
    CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take();
    crate::late_sorted::forget(&SORTED);
}

pub struct NativeLayers;

impl Feature for NativeLayers {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "World layers 1, 5, 7, 8, 20, 2 and 17, the alternate view's 1, 5 and 2, the first person's 1, 7, 8, 20 and 2 and the camouflage handler's 18 and 19 and the hologram's 21 drawn from the scene's plan")
            .doc("docs/graphics/framescene.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        PLACED.store(true, Ordering::Relaxed);
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_layers_skip", |o| {
                if crate::native_map_work::Stage::World.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                before_layer(&mut *g, ret);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        // Layers 2 and 17 per item over the sorted entries.
        b.raw()
            .observe_before(&points::scene_effects::LAYER_BUILT, "native_layers_sorted", |o| {
                if crate::native_map_work::Stage::World.standing_down() {
                    return;
                }
                let mut g = o.guest();
                crate::late_sorted::at_built(&SORTED, &mut *g, o.cpu.esp, o.cpu.eax, &mut ask_run);
            })
            .after_feature(crate::native_world::ID)
            .context_only();
        b.raw()
            .observe_before(&points::scene_effects::LAYER_ENTRY_DONE, "native_layers_entry", |o| {
                let mut g = o.guest();
                crate::late_sorted::after_entry(&SORTED, &mut *g, o.cpu.esi, &mut ask_run);
            })
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_is_on_with_the_native_renderer_unless_turned_off() {
        let on = |spec: &str| super::on(&hooks::Switches::parse(spec).unwrap());
        assert!(on(""), "on by default");
        assert!(!on("frame-scene.layers=off"));
        assert!(!on("impl.render.native=off"), "the D3D path is the off arm");
    }

    #[test]
    fn the_plans_outcomes_count_by_layer_and_cause() {
        count(5, "taken", 3);
        count(5, "refused_object", 2);
        count(7, "taken", 0);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_5_taken"] >= 3);
        assert!(c["layer_plan_5_refused_object"] >= 2);
        assert!(!c.contains_key("layer_plan_7_taken"), "nothing counted, no row");
        count_for(LayerCaller::FirstPerson, 8, "taken", 2);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_fp8_taken"] >= 2, "the first person's apart");
    }

    #[test]
    fn the_stage_takes_the_scenes_layers_and_the_first_persons_besides_layer_3() {
        assert_eq!(caller(5, 0x0002_bf39), Some(LayerCaller::Scene));
        assert_eq!(caller(20, 0x0002_c010), Some(LayerCaller::Scene));
        assert_eq!(caller(1, 0x0004_00ab), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(8, 0x0004_01a7), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(7, 0x0004_0271), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(20, 0x0004_020b), Some(LayerCaller::FirstPerson));
        assert_eq!(caller(3, 0x0004_0136), None, "native_first_person's");
        assert_eq!(caller(3, 0x0002_bed5), None, "native_objects' and the world's");
        assert_eq!(caller(1, 0x0002_bd5c), Some(LayerCaller::AlternateView));
        assert_eq!(caller(5, 0x0002_bd8c), Some(LayerCaller::AlternateView));
        assert_eq!(caller(5, 0x0002_bd5c), None);
        assert_eq!(caller(3, 0x0002_bd74), None, "owned by the base-layer stages");
    }

    #[test]
    fn the_stage_takes_the_scenes_late_layer_17() {
        assert_eq!(caller(17, 0x0002_c30b), Some(LayerCaller::Scene));
        assert_eq!(caller(17, 0x0002_c010), None, "another layer's return");
        assert_eq!(caller(16, 0x0002_c0e9), None, "the fog's late layer is native_fog's");
    }

    #[test]
    fn the_stage_takes_the_scenes_and_the_first_persons_overlay_layer_2() {
        assert_eq!(caller(2, 0x0002_c060), Some(LayerCaller::Scene), "after the dynamic lights");
        assert_eq!(caller(2, 0x0004_0289), Some(LayerCaller::FirstPerson), "after the first person's layer 7");
        assert_eq!(caller(2, 0x0002_bda8), Some(LayerCaller::AlternateView), "an alternate view's");
        assert_eq!(caller(2, 0x0002_c30b), None, "layer 17's return");
        count_for(LayerCaller::AlternateView, 2, "refused_state", 3);
        count(2, "taken", 2);
        count_for(LayerCaller::FirstPerson, 2, "refused_state", 1);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_alt2_refused_state"] >= 3);
        assert!(c["layer_plan_2_taken"] >= 2);
        assert!(c["layer_plan_fp2_refused_state"] >= 1, "the first person's apart");
    }

    #[test]
    fn the_stage_takes_the_sorted_handlers_layers_18_19_and_21() {
        assert_eq!(caller(18, 0x0004_2015), Some(LayerCaller::Sorted(SortedHandler::Camouflage)));
        assert_eq!(caller(16, 0x0004_1ffd), None, "the handler's replay of the scene's layers stays on D3D");
        assert_eq!(caller(18, 0x0002_c0e9), None);
        assert_eq!(caller(19, 0x0004_205f), Some(LayerCaller::Sorted(SortedHandler::Camouflage)), "the modulate layer");
        assert_eq!(caller(21, 0x0004_2721), Some(LayerCaller::Sorted(SortedHandler::Hologram)));
        assert_eq!(caller(21, 0x0004_205f), None, "another layer's return");
        count_for(LayerCaller::Sorted(SortedHandler::Camouflage), 18, "taken", 4);
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["layer_plan_sorted18_taken"] >= 4, "a sorted handler's apart");
    }

    /// The tests that touch the process-wide host and pending restore run
    /// one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    const LIST: u32 = 0x004c_1a48;
    const LIST_ITEMS: u32 = 0x004d_0000;

    /// Guest memory with `layer` current and its list's items: per item its
    /// render record and whether the record is an object's. Record
    /// `0x7000` is past the memory: it does not read. The render camera
    /// reads as no camera.
    fn list(layer: u32, items: &[(u32, bool)]) -> engine::memory::FlatMemory {
        use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};
        let mut m = engine::memory::FlatMemory::new(0x0040_0000, 0x0010_0000);
        m.set_u32(CURRENT_LAYER, layer).unwrap();
        m.set_u32(CURRENT_LIST, LIST).unwrap();
        m.set_u32(LIST + 4, items.len() as u32).unwrap();
        m.set_u32(LIST + 0xc, 0).unwrap();
        m.set_u32(LIST + 0x14, LIST_ITEMS).unwrap();
        for (i, &(record, object)) in items.iter().enumerate() {
            let at = LIST_ITEMS + 0x18 * i as u32;
            m.set_u32(at, GEOMETRY_BUILD).unwrap();
            m.set_u32(at + 8, 0xe000_0010 + i as u32).unwrap();
            m.set_u32(at + 0xc, record << 8 | i as u32).unwrap();
            m.set_u32(at + 0x10, 1 << layer).unwrap();
            let _ = m.set_u32(RENDER_RECORDS + 0x20 * record + 4, if object { 0xe100_0001 } else { u32::MAX });
        }
        m
    }

    fn mask(m: &engine::memory::FlatMemory, slot: u32) -> u32 {
        m.u32(LIST_ITEMS + 0x18 * slot + 0x10).unwrap()
    }

    fn kept(name: &str) -> u64 {
        counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    /// A host that names the whole layer `device` and draws nothing.
    struct Refuses;

    impl Host for Refuses {
        fn ask(&self, _: &mut dyn GuestMemory, _: u32, _: LayerCaller, _: &FrameScene, left: &mut Left) -> Option<Cleared> {
            left.all("device");
            None
        }

        fn ask_run(
            &self, _: &mut dyn GuestMemory, _: u32, _: LayerCaller, _: &FrameScene,
            _: &crate::late_sorted::RunAt<'_>, left: &mut Left,
        ) -> crate::late_sorted::Run {
            left.all("device");
            crate::late_sorted::Run::Refused
        }
    }

    /// A layer-2 item the plan leaves to the order rule holds the layer
    /// for its sorted entries: nothing is counted kept until it is done,
    /// then every item the runs did not draw, by its cause.
    #[test]
    fn an_order_item_of_layer_2_holds_the_layer_for_its_sorted_entries() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let m = list(2, &[(1, false), (2, false)]);
        let mut left = Left::default();
        left.item(0, "state");
        left.item(1, crate::late_sorted::ORDER);
        let extracted: HashSet<u32> = [0, 1].into_iter().collect();
        assert!(!hold_for_runs(&m, 5, LayerCaller::Scene, &mut left.clone(), Some(&extracted)), "not a run layer");
        assert!(!hold_for_runs(&m, 2, LayerCaller::FirstPerson, &mut left.clone(), Some(&extracted)), "not cut to the scene");
        let (state, unnamed) = (kept("scene_kept_2_world_state"), kept("scene_kept_2_world_unnamed"));
        assert!(hold_for_runs(&m, 2, LayerCaller::Scene, &mut left, Some(&extracted)));
        assert_eq!(kept("scene_kept_2_world_state"), state, "counted when done");
        crate::late_sorted::stale(&SORTED, &m);
        assert_eq!(kept("scene_kept_2_world_state"), state + 1);
        assert_eq!(kept("scene_kept_2_world_unnamed"), unnamed + 1, "no run decided the order item");
    }

    /// A scene layer the host does not draw (here its view does not
    /// extract: no camera) keeps every item's guest draw, the wall's and
    /// the object's, each counted by layer, kind and cause. Nothing goes
    /// undrawn.
    #[test]
    fn a_scene_layer_the_host_does_not_draw_keeps_its_guest_draws() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_host(Arc::new(Refuses));
        let (world, object) = (kept("scene_kept_5_world_unread"), kept("scene_kept_5_object_unread"));
        let mut m = list(5, &[(1, false), (2, true)]);
        before_layer(&mut m, 0x0002_bf39);
        assert_eq!((mask(&m, 0), mask(&m, 1)), (1 << 5, 1 << 5), "both keep their guest draw");
        assert_eq!(kept("scene_kept_5_world_unread"), world + 1);
        assert_eq!(kept("scene_kept_5_object_unread"), object + 1);
        // The next layer draw has nothing to set back.
        m.set_u32(native_render::skip::CURRENT_LAYER, 6).unwrap();
        before_layer(&mut m, 0);
        assert_eq!((mask(&m, 0), mask(&m, 1)), (1 << 5, 1 << 5));
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Each kept item takes the cause the host named for its slot, else
    /// `remainder` when the layer's scene does not hold it, else the
    /// layer's cause; an unreadable record is `record`.
    #[test]
    fn a_kept_item_is_counted_by_its_own_cause_first() {
        let m = list(8, &[(1, false), (3, false), (2, true), (0x7000, false)]);
        let mut left = Left::default();
        left.item(0, "set");
        left.all("device");
        left.all("later");
        assert_eq!(left.layer, Some("device"), "the first cause stays");
        let extracted: HashSet<u32> = [0, 2, 3].into_iter().collect();
        let names = ["scene_kept_alt8_world_set", "scene_kept_alt8_world_remainder", "scene_kept_alt8_object_device", "scene_kept_alt8_unreadable_record"];
        let before: Vec<u64> = names.iter().map(|n| kept(n)).collect();
        count_kept(&Ro(&m), 8, "alt", true, &left, Some(&extracted));
        for (n, b) in names.iter().zip(before) {
            assert_eq!(kept(n), b + 1, "{n}");
        }
        assert_eq!(mask(&m, 0), 1 << 8, "counting writes nothing");
    }

    /// Layer 17's items the host does not draw keep their guest draw and
    /// are counted as layer 2's are; a layer 2 or 17 draw from a caller
    /// the stage does not take keeps every item under `caller`.
    #[test]
    fn layer_17_and_an_untaken_callers_layer_2_count_what_they_keep() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_host(Arc::new(Refuses));
        assert!(scene_only(17, LayerCaller::Scene));
        let before = (kept("scene_kept_17_world_unread"), kept("scene_kept_17_object_unread"));
        let mut m = list(17, &[(1, false), (2, true)]);
        before_layer(&mut m, 0x0002_c30b);
        assert_eq!((mask(&m, 0), mask(&m, 1)), (1 << 17, 1 << 17), "both keep their guest draw");
        assert_eq!((kept("scene_kept_17_world_unread"), kept("scene_kept_17_object_unread")), (before.0 + 1, before.1 + 1));
        let before = (kept("scene_kept_2_world_caller"), kept("scene_kept_2_object_caller"));
        let mut m = list(2, &[(1, false), (2, true)]);
        before_layer(&mut m, 0x0002_c30b);
        assert_eq!((mask(&m, 0), mask(&m, 1)), (1 << 2, 1 << 2));
        assert_eq!((kept("scene_kept_2_world_caller"), kept("scene_kept_2_object_caller")), (before.0 + 1, before.1 + 1));
        let other = kept("scene_kept_5_world_caller");
        let mut m = list(5, &[(1, false)]);
        before_layer(&mut m, 0x0002_c30b);
        assert_eq!(kept("scene_kept_5_world_caller"), other, "only R5's layers count an untaken caller");
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[test]
    fn a_cleared_bit_goes_back_only_on_the_same_item() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        use engine::memory::FlatMemory;
        const BASE: u32 = 0x0060_0000;
        let mut mem = FlatMemory::new(BASE, 0x100);
        let (a, b) = (BASE, BASE + 0x18);
        for (item, shader) in [(a, 0xe000_0020u32), (b, 0xe000_0021)] {
            mem.set_u32(item + 8, shader).unwrap();
            mem.set_u32(item + 0xc, 0x0000_0500).unwrap();
            mem.set_u32(item + 0x10, 1 << 15).unwrap();
        }
        let cleared = Cleared { layer: 18, items: vec![a, b] };
        let left = vec![left_of(&Mem(&mut mem), a), left_of(&Mem(&mut mem), b)];
        *CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some((cleared, left));
        // The list was built again: `b` is another record's item now.
        mem.set_u32(b + 0xc, 0x0000_0600).unwrap();
        restore_pending(&mut mem);
        assert_eq!(mem.u32(a + 0x10).unwrap(), 1 << 15 | 1 << 18, "the same item's bit back");
        assert_eq!(mem.u32(b + 0x10).unwrap(), 1 << 15, "another item left alone");
    }
}
