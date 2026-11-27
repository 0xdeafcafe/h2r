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
//! The scene's and the alternate view's layers 1, 2, 5, 7, 8 and 20
//! ([`SCENE_ONLY`]) draw only from the scene: what the host does not draw
//! loses the layer's bit too, so the guest builds no entry for it, and is
//! counted `scene_dropped_world_<cause>` ([`count_dropped`]). The first
//! person's and the sorted handlers' layers still keep what the host
//! refuses to D3D until their own cuts.
//!
//! The plan's outcomes are counted by cause for `render.scene`
//! ([`counters`]).

use std::collections::BTreeMap;
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
/// `native_world` owns).
pub const SCENE_ONLY: [u32; 6] = [1, 2, 5, 7, 8, 20];

/// Whether `layer` of `caller` draws only from the scene.
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
    /// the layer to the D3D path.
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene) -> Option<Cleared>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static PLACED: AtomicBool = AtomicBool::new(false);
/// What the last taken layer cleared, with each item as it was left
/// ([`Left`]).
static CLEARED: Mutex<Option<(Cleared, Vec<Left>)>> = Mutex::new(None);

/// An item as the skip left it: its mask (`+0x10`), shader (`+8`) and
/// record argument (`+0xc`). A sorted handler's layer can be the last of
/// its list (the next layer draw follows the next entry's rebuild of the
/// same items), so the bit goes back only on an item that is still the
/// one cleared.
type Left = [u32; 3];

fn left_of(m: &dyn GuestRead, item: u32) -> Left {
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

/// World items the scene did not draw, by cause: `refused` (no scene draw
/// for the part), `unreadable` (its render record or list did not read).
static DROPPED: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// Counts `n` world items dropped under `cause`: `scene_dropped_world_<cause>`
/// in [`counters`], and the world stage's `scene_dropped` total.
pub fn count_dropped(cause: &'static str, n: u64) {
    if n == 0 {
        return;
    }
    *DROPPED.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += n;
    timeline::native::Stage::World.count(timeline::native::Metric::SceneDropped, n);
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
    out.extend(DROPPED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (format!("scene_dropped_world_{k}"), v)));
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
    let Some(layer) = Mem(mem).read_u32(skip::CURRENT_LAYER) else { return };
    let Some(caller) = caller(layer, ret) else { return };
    if layer == 7 && caller == LayerCaller::Scene {
        // The object shadows draw in slot 3 ahead of layer 7.
        let shadows = crate::scene_step::open_object_shadow_passes();
        crate::native_lights::object_shadow_scene::draw(mem, &shadows);
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    ASKED.fetch_add(1, Ordering::Relaxed);
    let drawn = match extract(&*mem, layer as u8, caller) {
        Some(scene) => host.ask(mem, layer, caller, &scene),
        None => {
            count_for(caller, layer, "refused_unread", 1);
            None
        }
    };
    if let Some(cleared) = drawn.filter(|c| !c.items.is_empty()) {
        DRAWN.fetch_add(1, Ordering::Relaxed);
        ITEMS.fetch_add(cleared.items.len() as u64, Ordering::Relaxed);
        remember(mem, cleared);
    }
    drop_rest(mem, layer, caller);
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

/// A scene-only layer's items the host did not draw lose the layer's bit:
/// the guest builds no entry for them, so none reaches the device. Counted
/// `scene_dropped_world_refused`, or `_unreadable` for a part whose record
/// or list does not read.
fn drop_rest(mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller) {
    if !scene_only(layer, caller) {
        return;
    }
    let skip = native_render::layer_pass::clear_scene_layer(&mut Mem(&mut *mem), layer, true, &|_| false);
    match skip {
        Some(skip) => {
            count_dropped("refused", skip.refused as u64);
            count_dropped("unreadable", skip.unreadable as u64);
            if !skip.cleared.items.is_empty() {
                remember(mem, skip.cleared);
            }
        }
        None => count_dropped("unreadable", 1),
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

    #[test]
    fn a_cleared_bit_goes_back_only_on_the_same_item() {
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
