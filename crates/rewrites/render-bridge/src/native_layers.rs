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
//! refuses still draws. The sorted handlers' layers 18, 19 and 21 count
//! what they keep the same way ([`kept_counted`]). A layer 2, 17, 18, 19
//! or 21 draw from a caller this stage does not take ([`UNTAKEN_CALLER`])
//! keeps every item, counted under the cause `caller`. The first person's
//! layers count what they keep the same way under the site `fp`; the
//! sorted handlers' keep what the host refuses to D3D, counted under
//! `layer_plan_*`.
//!
//! The plan's outcomes are counted by cause for `render.scene`
//! ([`counters`]).
//!
//! The layer draw itself is implemented here for the opaque block's layers
//! 1, 3, 5, 7, 8 and 20 of the scene, the alternate view and the first
//! person, and for the light passes' and shadow layers' 10 to 14
//! ([`layer_return`], [`return_site`]): after every observer has run, when
//! no item of the list still holds the layer's bit, the call makes the
//! guest body's writes (`frame_step::layer`) and returns without running
//! it; otherwise the guest body runs, counted
//! `layer_return_<site><layer>_guest_<cause>`, and every item it draws that
//! no stage counted before the call or owns inside it ([`in_body`]) is
//! counted then, under the cause a stage left for its kind ([`leave`]) or
//! `unnamed`: each item the body draws is one `scene_kept_*` count.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
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
/// overlay, late, camouflage and hologram layers
/// (docs/graphics/retire-d3d.md, "By caller").
pub const UNTAKEN_CALLER: [u32; 5] = [2, 17, 18, 19, 21];

/// The cause an untaken caller's items are counted under.
pub const CALLER_CAUSE: &str = "caller";

/// Whether `layer` of `caller` is cut to the scene: its kept guest draws are counted `scene_kept_*`.
pub fn scene_only(layer: u32, caller: LayerCaller) -> bool {
    matches!(caller, LayerCaller::Scene | LayerCaller::AlternateView) && SCENE_ONLY.contains(&layer)
}

/// Whether the items `layer` of `caller` keeps are counted
/// `scene_kept_<site><layer>_<kind>_<cause>`: the layers cut to the scene
/// ([`scene_only`]) and the sorted handlers' 18, 19 and 21, which only
/// those handlers take.
pub fn kept_counted(layer: u32, caller: LayerCaller) -> bool {
    scene_only(layer, caller) || (matches!(caller, LayerCaller::Sorted(_)) && SORTED_LAYERS.contains(&layer))
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
    switches.feature(ID, true)
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

    /// The cache's open shader's restore list (`crate::open_shader`),
    /// written through the device as the guest's `0x00018900` writes it,
    /// in order; `Err` names why not, before anything is written.
    fn restore(&self, _mem: &mut dyn GuestMemory, _writes: &[crate::open_shader::Write]) -> Result<(), &'static str> {
        Err("no_device")
    }
}

/// The layers whose items the plan takes but the order rule holds back
/// (`order`: after a kept item they do not commute with) draw per item
/// over the guest's sorted entries, between the items it keeps
/// (`crate::late_sorted`; docs/graphics/retire-d3d-r5.md, "Layer 2 from
/// the plan"): R5's blended overlay and late layers, at the scene's and
/// the alternate view's calls.
pub const RUN_LAYERS: [u32; 7] = SCENE_ONLY;

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
/// with its cause as [`count_kept`] names it, counted once when the layer
/// is done (the layer draw's own turn leaves them to it). `false` when
/// nothing is held.
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
    with_tally(&ro, layer, |t| t.counted.extend(held.iter().map(|h| h.slot)));
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
/// The layer draws of the opaque block that returned, and those that ran the
/// guest body by cause (`layer_return_<site><layer>_returned`,
/// `_guest_<cause>`).
static RETURNS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
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
    let mut counted = Vec::with_capacity(held.len());
    for h in held {
        let cause = match h.item {
            None => "record",
            Some(_) if !left.items.contains_key(&h.slot) && extracted.is_some_and(|e| !e.contains(&h.slot)) => "remainder",
            Some(_) => left.cause(h.slot),
        };
        count_kept_as(site, layer, h.kind(), cause, 1);
        counted.push(h.slot);
    }
    with_tally(mem, layer, |t| t.counted.extend(counted));
}

/// The kind of an item a layer call's guest body draws
/// ([`crate::frame_step::layer::held`]): `Structure`, `Object`, `Other`
/// or `Unread`.
pub use crate::frame_step::layer::Keep;

/// What the stages of the layer call under way have accounted for, for
/// [`layer_return`]: the slots [`count_kept`] counted, the kinds a stage
/// draws or counts inside the guest body ([`in_body`]), and each kind's
/// cause from a stage that leaves its items without naming them by slot
/// ([`leave`]). Taken at every layer draw's own turn.
#[derive(Debug, Default)]
struct Tally {
    layer: u32,
    list: u32,
    counted: HashSet<u32>,
    in_body: Vec<Keep>,
    causes: Vec<(Keep, &'static str)>,
    own: bool,
}

thread_local! {
    /// The layer draw's observers and its own turn run on the thread that
    /// makes the call.
    static TALLY: std::cell::RefCell<Option<Tally>> = const { std::cell::RefCell::new(None) };
}

/// `f` on the current call's tally: a new one when the last was another
/// layer's or list's.
fn with_tally(mem: &dyn GuestRead, layer: u32, f: impl FnOnce(&mut Tally)) {
    let list = mem.read_u32(skip::CURRENT_LIST).unwrap_or(0);
    TALLY.with(|t| {
        let mut t = t.borrow_mut();
        if t.as_ref().is_none_or(|t| (t.layer, t.list) != (layer, list)) {
            *t = Some(Tally { layer, list, ..Tally::default() });
        }
        f(t.as_mut().expect("set above"));
    });
}

/// A stage draws or counts `kind`'s items of `layer`'s current call
/// inside the guest body (layer 3's sorted walk, the resolved world's
/// opaque handlers): [`layer_return`] leaves them to it.
pub fn in_body(mem: &dyn GuestRead, layer: u32, kind: Keep) {
    with_tally(mem, layer, |t| t.in_body.push(kind));
}

/// A stage counts every item of `layer`'s current call it leaves to the
/// guest itself (the fog's and the water's `count_late_kept`, or their
/// sorted runs when done): [`layer_return`] counts none of them again.
pub fn counts_own(mem: &dyn GuestRead, layer: u32) {
    with_tally(mem, layer, |t| t.own = true);
}

/// A stage leaves `kind`'s items of `layer`'s current call to the guest
/// under `cause` without naming them by slot: [`layer_return`] counts
/// those no stage counted under it. The first cause named holds.
pub fn leave(mem: &dyn GuestRead, layer: u32, kind: Keep, cause: &'static str) {
    with_tally(mem, layer, |t| {
        if !t.causes.iter().any(|&(k, _)| k == kind) {
            t.causes.push((kind, cause));
        }
    });
}

/// The counter name of a held item's kind.
fn kind_name(kind: Keep) -> &'static str {
    match kind {
        Keep::Structure => "world",
        Keep::Object => "object",
        Keep::Unread => "unreadable",
        _ => "other",
    }
}

/// The cause the items of `kind` no stage counted are kept under, at
/// `site`'s `layer`: a stage's ([`leave`]), else `standing_down` when the
/// map's stages stood down for the whole call (`call`), else the first person's held
/// cut for its layer 3 (`native_first_person::held`: its guest body
/// draws them), else `unnamed`.
fn unnamed_cause(tally: Option<&Tally>, site: &str, layer: u32, kind: Keep, call: Keep) -> &'static str {
    if let Some(&(_, cause)) = tally.and_then(|t| t.causes.iter().find(|&&(k, _)| k == kind)) {
        return cause;
    }
    if call == Keep::StandingDown {
        return "standing_down";
    }
    if site == "fp" && layer == 3 {
        return "first_person_held";
    }
    "unnamed"
}

/// Counts every item a layer call's guest body draws that no stage
/// counted: each `scene_kept_<site><layer>_<kind>_<cause>` once
/// ([`unnamed_cause`]). An item whose render record index is negative
/// builds no entry and is not counted; nor is a kind a stage owns inside
/// the body ([`in_body`]).
fn count_unnamed(mem: &dyn GuestMemory, site: &str, layer: u32, tally: Option<&Tally>, call: Keep) {
    use crate::frame_step::layer as fl;
    if tally.is_some_and(|t| t.own) {
        return;
    }
    let Ok(held) = fl::held(mem, layer) else {
        if tally.is_none_or(|t| t.counted.is_empty()) {
            count_kept_as(site, layer, "unreadable", "list", 1);
        }
        return;
    };
    let items = mem.u32(fl::CURRENT_LIST).and_then(|l| mem.u32(l.wrapping_add(0x14))).unwrap_or(0);
    for (slot, kind) in held {
        if tally.is_some_and(|t| t.counted.contains(&slot) || t.in_body.contains(&kind)) {
            continue;
        }
        let item = items.wrapping_add(slot.wrapping_mul(0x18));
        if matches!(kind, Keep::Structure | Keep::Object)
            && mem.i32(item + 0xc).is_ok_and(|arg| ((arg >> 8) as i16) < 0)
        {
            continue;
        }
        count_kept_as(site, layer, kind_name(kind), unnamed_cause(tally, site, layer, kind, call), 1);
    }
}

/// The counter site of `caller`: `""` for the scene, `alt` for the
/// alternate view.
fn site(caller: LayerCaller) -> &'static str {
    match caller {
        LayerCaller::AlternateView => "alt",
        LayerCaller::FirstPerson => "fp",
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
    out.extend(RETURNS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (k.clone(), v)));
    out.extend(KEPT.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (format!("scene_kept_{k}"), v)));
    out.extend(crate::open_shader::counters());
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
        // No host draws: the layer's return counts every item under `host`.
        for kind in [Keep::Structure, Keep::Object] {
            leave(&Ro(&*mem), layer, kind, "host");
        }
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
    if kept_counted(layer, caller) || caller == LayerCaller::FirstPerson {
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

/// The site prefix of a layer draw that can return: `""` the opaque
/// block's and R5's layers 2, 6, 16 and 17 of the scene and the sorted
/// handlers' 18, 19 and 21, `alt` the alternate view's, `fp` the first person's; `light`, `extra`, `shadow` and `fplight` the light passes'
/// and shadow layers' of a light, its additional casters, an object
/// shadow and the first-person lights ([`light_site`]); `None` for every
/// other call of the layer draw.
pub fn return_site(layer: u32, ret: u32) -> Option<&'static str> {
    use crate::frame_step::layer::{FOG_WATER_LAYERS, LATE_LAYERS, LAYERS, LIGHT_LAYERS};
    if LIGHT_LAYERS.contains(&layer) {
        return light_site(layer, ret);
    }
    if FOG_WATER_LAYERS.contains(&layer) {
        return scene_extract::late::scene_layer_call(layer, ret).then_some("");
    }
    if LATE_LAYERS.contains(&layer) && caller(layer, ret).is_some_and(|c| matches!(c, LayerCaller::Sorted(_))) {
        return Some("");
    }
    if !LAYERS.contains(&layer) && !LATE_LAYERS.contains(&layer) {
        return None;
    }
    match layer_site(layer, ret) {
        Site::Scene => Some(""),
        Site::AlternateView => Some("alt"),
        Site::FirstPerson => Some("fp"),
        _ => None,
    }
}

/// The site of a light layer's draw returning to `ret` (the guest's site,
/// `light_gather::passes::caller`).
pub fn light_site(layer: u32, ret: u32) -> Option<&'static str> {
    use native_render::shadows::{Site as Shadow, site};
    match site(layer, ret) {
        Some(Shadow::Light) => return Some("light"),
        Some(Shadow::AdditionalCaster) => return Some("extra"),
        Some(Shadow::Object) => return Some("shadow"),
        None => {}
    }
    let at = |layers: &[(u32, u32)]| layers.contains(&(layer, ret));
    if at(&crate::native_lights::SPLIT_LAYERS) {
        Some("light")
    } else if at(&crate::native_lights::FIRST_PERSON_LAYERS) {
        Some("fplight")
    } else {
        None
    }
}

fn count_return(site: &str, layer: u32, outcome: &str) {
    let mut r = RETURNS.lock().unwrap_or_else(|e| e.into_inner());
    *r.entry(format!("layer_return_{site}{layer}_{outcome}")).or_default() += 1;
}

/// The layer draw's own turn, after its observers: true when the call is
/// one of the opaque block's and every item of it was native, its writes
/// made (`frame_step::layer::step`), so the caller returns without the
/// guest body. `ret` is the call's return address.
///
/// Every call takes the call's tally ([`Tally`]); a call of the opaque
/// block that runs the guest body for its items first counts each item no
/// stage counted ([`count_unnamed`]), so every item the body draws is a
/// `scene_kept_*` count by cause.
pub fn layer_return(mem: &mut dyn GuestMemory, ret: u32, standing_down: bool) -> bool {
    layer_turn(mem, ret, standing_down) == Turn::Return
}

/// What a layer draw's own turn does ([`layer_turn`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// The body's writes are made: the call returns.
    Return,
    /// The guest body runs.
    Guest,
    /// Every item is native and the cache holds an open shader: its
    /// restore list is written (`crate::open_shader`), then the body's
    /// writes ([`finish_restore`]) and the call returns. `site` and
    /// `layer` name the call's counter.
    Restore { restore: crate::open_shader::Restore, site: &'static str, layer: u32 },
}

/// [`layer_return`], with the open shader's restore: a call that
/// [`frame_step::layer::keep`](crate::frame_step::layer::keep) keeps only
/// for the cache's open shader, no item holding the layer's bit, is
/// [`Turn::Restore`], counted when it returns. A restore list that does
/// not read runs the guest body (`guest_open_shader`).
pub fn layer_turn(mem: &mut dyn GuestMemory, ret: u32, standing_down: bool) -> Turn {
    let tally = TALLY.with(|t| t.borrow_mut().take());
    let Ok(layer) = mem.u32(skip::CURRENT_LAYER) else { return Turn::Guest };
    let Some(site) = return_site(layer, ret) else { return Turn::Guest };
    let tally = tally.filter(|t| t.layer == layer);
    let step = if standing_down {
        Err(Keep::StandingDown)
    } else {
        crate::frame_step::layer::step(mem, layer)
    };
    match step {
        Ok(()) => {
            count_return(site, layer, "returned");
            Turn::Return
        }
        Err(Keep::OpenShader)
            if crate::frame_step::layer::held(&*mem, layer).is_ok_and(|held| held.is_empty()) =>
        {
            match crate::open_shader::read(&*mem) {
                Ok(Some(restore)) => Turn::Restore { restore, site, layer },
                _ => {
                    count_return(site, layer, "guest_open_shader");
                    Turn::Guest
                }
            }
        }
        Err(keep) => {
            count_unnamed(&*mem, site, layer, tally.as_ref(), keep);
            count_return(site, layer, &format!("guest_{}", keep.name()));
            Turn::Guest
        }
    }
}

/// The open shader's restore, as the body runs it: the drawn layer and
/// the draw byte the body sets before `0x00016b10`, then the host's
/// replay ([`crate::open_shader::replay`]). `Err` names why it was not
/// made: `unwritable`, `host` (none installed), the host's cause.
pub fn begin_restore(
    mem: &mut dyn GuestMemory, restore: &crate::open_shader::Restore, layer: u32,
) -> Result<(), &'static str> {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    mem.set_u32(crate::frame_step::layer::DRAWN_LAYER, layer).map_err(|_| "unwritable")?;
    mem.set_u8(crate::frame_step::layer::DRAW_FLAG, 1).map_err(|_| "unwritable")?;
    let host = host.ok_or("host")?;
    crate::open_shader::replay(mem, restore, |mem, writes| host.restore(mem, writes))
}

/// After the restore: the body's remaining writes
/// (`frame_step::layer::writes`), counted `returned_open_shader`.
pub fn finish_restore(mem: &mut dyn GuestMemory, site: &str, layer: u32) -> Result<(), MemFault> {
    crate::frame_step::layer::writes(mem)?;
    count_return(site, layer, "returned_open_shader");
    Ok(())
}

/// The original body at `addr`.
fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(runtime::TransferAbi::fault(event))
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
        b.raw()
            .implement(&points::LAYER_DRAW, "frame_step_layer_return", |cpu, ctx, addr| {
                if addr == points::LAYER_DRAW.base {
                    let standing_down = crate::native_map_work::Stage::World.standing_down();
                    let rt = Arc::clone(ctx.runtime());
                    let mut g = rt.memory();
                    // The light passes' body names the guest's site.
                    let ret = engine::render::light_gather::passes::caller(&*g, cpu.esp).unwrap_or(0);
                    match layer_turn(&mut *g, ret, standing_down) {
                        Turn::Return => return g.ret(cpu, 0),
                        Turn::Guest => {}
                        Turn::Restore { restore, site, layer } => {
                            if let Err(cause) = begin_restore(&mut *g, &restore, layer) {
                                unimplemented!("open shader: {cause}");
                            }
                            if finish_restore(&mut *g, site, layer).is_err() {
                                unimplemented!("open shader: unwritable");
                            }
                            return g.ret(cpu, 0);
                        }
                    }
                }
                original(cpu, ctx, addr)
            })
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
    fn list(layer: u32, items: &[(u32, bool)]) -> guest_layout::memory::FlatMemory {
        use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};
        let mut m = guest_layout::memory::FlatMemory::new(0x0040_0000, 0x0010_0000);
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

    fn mask(m: &guest_layout::memory::FlatMemory, slot: u32) -> u32 {
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
        assert!(!hold_for_runs(&m, 3, LayerCaller::Scene, &mut left.clone(), Some(&extracted)), "not a run layer");
        assert!(!hold_for_runs(&m, 2, LayerCaller::FirstPerson, &mut left.clone(), Some(&extracted)), "not cut to the scene");
        let (state, unnamed) = (kept("scene_kept_2_world_state"), kept("scene_kept_2_world_unnamed"));
        assert!(hold_for_runs(&m, 2, LayerCaller::Scene, &mut left, Some(&extracted)));
        assert_eq!(kept("scene_kept_2_world_state"), state, "counted when done");
        crate::late_sorted::stale(&SORTED, &m);
        assert_eq!(kept("scene_kept_2_world_state"), state + 1);
        assert_eq!(kept("scene_kept_2_world_unnamed"), unnamed + 1, "no run decided the order item");
    }

    /// An opaque layer's order item holds the layer the same way, and the
    /// layer draw's own turn leaves its items to the runs: each is counted
    /// once, when the layer is done.
    #[test]
    fn an_order_item_of_an_opaque_layer_is_counted_once_when_its_runs_are_done() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        TALLY.with(|t| t.borrow_mut().take());
        let m = list(5, &[(1, false), (2, false)]);
        let mut left = Left::default();
        left.item(0, "state");
        left.item(1, crate::late_sorted::ORDER);
        let extracted: HashSet<u32> = [0, 1].into_iter().collect();
        let (state, unnamed) = (kept("scene_kept_5_world_state"), kept("scene_kept_5_world_unnamed"));
        assert!(hold_for_runs(&m, 5, LayerCaller::Scene, &mut left, Some(&extracted)));
        let tally = TALLY.with(|t| t.borrow_mut().take());
        count_unnamed(&m, "", 5, tally.as_ref(), Keep::Structure);
        assert_eq!((kept("scene_kept_5_world_state"), kept("scene_kept_5_world_unnamed")), (state, unnamed), "the turn counts none");
        crate::late_sorted::stale(&SORTED, &m);
        assert_eq!(kept("scene_kept_5_world_state"), state + 1);
        assert_eq!(kept("scene_kept_5_world_unnamed"), unnamed + 1);
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
        let mut m = list(21, &[(2, true)]);
        let hologram = kept("scene_kept_21_object_caller");
        before_layer(&mut m, 0x0002_c30b);
        assert_eq!(kept("scene_kept_21_object_caller"), hologram + 1, "a layer 21 draw no handler made");
        let mut m = list(5, &[(1, false)]);
        before_layer(&mut m, 0x0002_c30b);
        assert_eq!(kept("scene_kept_5_world_caller"), other, "only R5's layers count an untaken caller");
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The camouflage handler's layers 18 and 19 and the hologram's 21
    /// keep what the host does not draw, each item counted
    /// `scene_kept_<layer>_<kind>_<cause>`; nothing goes undrawn.
    #[test]
    fn a_camouflage_or_hologram_item_the_host_does_not_draw_counts_scene_kept() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_host(Arc::new(Refuses));
        for (layer, ret) in [(18, 0x0004_2015), (19, 0x0004_205f), (21, 0x0004_2721)] {
            assert!(kept_counted(layer, caller(layer, ret).unwrap()), "{layer}");
            let name = format!("scene_kept_{layer}_object_unread");
            let before = kept(&name);
            let mut m = list(layer, &[(2, true)]);
            before_layer(&mut m, ret);
            assert_eq!(mask(&m, 0), 1 << layer, "{layer}: keeps its guest draw");
            assert_eq!(kept(&name), before + 1, "{name}");
            restore_pending(&mut m);
        }
        assert!(!kept_counted(1, LayerCaller::FirstPerson), "the first person's are layer_plan_fp*");
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[test]
    fn the_light_passes_layer_draws_return_or_run_the_guest_counted_by_site_and_cause() {
        use crate::frame_step::layer as fl;
        use guest_layout::memory::FlatMemory;
        let get = |k: &str| counters().into_iter().find(|(n, _)| n == k).map_or(0, |(_, v)| v);
        let mut mem = FlatMemory::new(0x0045_0000, 0x7_0000);
        let (list, items) = (0x004a_0000, 0x004a_0100);
        mem.set_u32(fl::CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 4, 1).unwrap();
        mem.set_u32(list + 0x14, items).unwrap();
        mem.set_u32(items, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(items + 0xc, 3 << 8).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, 0xe000_0003).unwrap();
        // A light's layer 12 (`0x0003144f`) with no item holding the bit.
        mem.set_u32(fl::CURRENT_LAYER, 12).unwrap();
        let returned = get("layer_return_light12_returned");
        assert!(layer_return(&mut mem, 0x0003_144f, false));
        assert_eq!(get("layer_return_light12_returned"), returned + 1);
        assert_eq!(mem.u32(fl::DRAWN_LAYER).unwrap(), u32::MAX, "the body's writes made");
        // The first-person lights' layer 14 (`0x000406ab`), an object's item kept.
        mem.set_u32(fl::CURRENT_LAYER, 14).unwrap();
        mem.set_u32(items + 0x10, 1 << 14).unwrap();
        let kept_object = get("layer_return_fplight14_guest_object");
        assert!(!layer_return(&mut mem, 0x0004_06ab, false));
        assert_eq!(get("layer_return_fplight14_guest_object"), kept_object + 1);
        // An object shadow's layer 11 (`0x000329d9`), a structure item kept.
        mem.set_u32(fl::CURRENT_LAYER, 11).unwrap();
        mem.set_u32(items + 0x10, 1 << 11).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, u32::MAX).unwrap();
        let structure = get("layer_return_shadow11_guest_structure");
        assert!(!layer_return(&mut mem, 0x0003_29d9, false));
        assert_eq!(get("layer_return_shadow11_guest_structure"), structure + 1);
        // The additional casters' layer 10 (`0x00031074`) returns once nothing holds its bit.
        mem.set_u32(fl::CURRENT_LAYER, 10).unwrap();
        mem.set_u32(items + 0x10, 0).unwrap();
        assert!(layer_return(&mut mem, 0x0003_1074, false));
        assert!(get("layer_return_extra10_returned") > 0);
        // A light layer's draw from another caller is not the frame step's.
        let returns = || counters().into_iter().filter(|(k, _)| k.starts_with("layer_return_")).map(|(_, v)| v).sum::<u64>();
        let before = returns();
        mem.set_u32(fl::CURRENT_LAYER, 12).unwrap();
        assert!(!layer_return(&mut mem, 0x0004_1ffd, false));
        assert_eq!(returns(), before);
    }

    /// A layer draw whose items are all native but whose cache holds an
    /// open shader restores its list through the host (none here: it
    /// names `host`), then the body's writes; with an item still holding
    /// the bit the body runs.
    #[test]
    fn an_open_shader_with_every_item_native_is_restored_and_returns() {
        use crate::frame_step::layer as fl;
        use crate::open_shader::IMPLEMENTATION_RESTORE;
        use guest_layout::memory::FlatMemory;
        let get = |k: &str| counters().into_iter().find(|(n, _)| n == k).map_or(0, |(_, v)| v);
        let mut mem = FlatMemory::new(0x0040_0000, 0x10_0000);
        let (list, items, implementation) = (0x004a_0000, 0x004a_0100, 0x004a_1000);
        mem.set_u32(fl::CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 4, 1).unwrap();
        mem.set_u32(list + 0x14, items).unwrap();
        mem.set_u32(items, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(fl::SETUP + fl::SETUP_OPEN, implementation).unwrap();
        // No entries: the restore reads, and writes nothing.
        mem.set_u16(implementation + IMPLEMENTATION_RESTORE, 0).unwrap();
        mem.set_u32(fl::SETUP + crate::open_shader::SETUP_DESCRIPTOR, 0x004a_2000).unwrap();
        mem.set_u32(0x004a_2000 + crate::open_shader::DESCRIPTOR_STATES, 0x004a_3000).unwrap();
        mem.set_u32(fl::CURRENT_LAYER, 1).unwrap();
        let Turn::Restore { restore, site, layer } = layer_turn(&mut mem, 0x0002_bd5c, false) else {
            panic!("the open shader alone holds the body");
        };
        assert_eq!((restore.implementation, site, layer), (implementation, "alt", 1));
        assert_eq!(begin_restore(&mut mem, &restore, layer), Err("host"), "no host installed");
        assert_eq!(mem.u32(fl::DRAWN_LAYER).unwrap(), 1, "the body's drawn layer during the restore");
        let returned = get("layer_return_alt1_returned_open_shader");
        finish_restore(&mut mem, site, layer).unwrap();
        assert_eq!(get("layer_return_alt1_returned_open_shader"), returned + 1);
        assert_eq!((mem.u32(fl::DRAWN_LAYER).unwrap(), mem.u32(fl::SETUP + fl::SETUP_OPEN).unwrap()), (u32::MAX, 0), "the body's writes made");
        // An item holding the bit: the body runs, counted under its kind.
        mem.set_u32(fl::SETUP + fl::SETUP_OPEN, implementation).unwrap();
        mem.set_u32(items + 0x10, 1 << 1).unwrap();
        mem.set_u32(items + 0xc, 3 << 8).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, 0xe000_0003).unwrap();
        let open = get("layer_return_alt1_guest_open_shader");
        assert_eq!(layer_turn(&mut mem, 0x0002_bd5c, false), Turn::Guest);
        assert_eq!(get("layer_return_alt1_guest_open_shader"), open + 1, "the cache's shader is still its first cause");
    }

    #[test]
    fn the_opaque_blocks_layer_draws_return_or_run_the_guest_counted_by_cause() {
        use crate::frame_step::layer as fl;
        use guest_layout::memory::FlatMemory;
        let get = |k: &str| counters().into_iter().find(|(n, _)| n == k).map_or(0, |(_, v)| v);
        let mut mem = FlatMemory::new(0x0045_0000, 0x7_0000);
        let (list, items) = (0x004a_0000, 0x004a_0100);
        mem.set_u32(fl::CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 4, 2).unwrap();
        mem.set_u32(list + 0x14, items).unwrap();
        mem.set_u32(items, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(items + 0x18, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(items + 0x18 + 0xc, 3 << 8).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, 0xe000_0003).unwrap();
        // The alternate view's layer 1 (`0x0002bd5c`): nothing holds the bit.
        mem.set_u32(fl::CURRENT_LAYER, 1).unwrap();
        let returned = get("layer_return_alt1_returned");
        assert!(layer_return(&mut mem, 0x0002_bd5c, false));
        assert_eq!(get("layer_return_alt1_returned"), returned + 1);
        assert_eq!(mem.u32(fl::DRAWN_LAYER).unwrap(), u32::MAX, "the body's writes made");
        // The first person's layer 20 (`0x0004020b`): an object's item kept.
        mem.set_u32(fl::CURRENT_LAYER, 20).unwrap();
        mem.set_u32(items + 0x18 + 0x10, 1 << 20).unwrap();
        let object = get("layer_return_fp20_guest_object");
        assert!(!layer_return(&mut mem, 0x0004_020b, false));
        assert_eq!(get("layer_return_fp20_guest_object"), object + 1);
        // An open shader whose restore list does not read, and the map's
        // stages standing down.
        mem.set_u32(items + 0x18 + 0x10, 0).unwrap();
        mem.set_u32(fl::SETUP + fl::SETUP_OPEN, 1).unwrap();
        let open = get("layer_return_fp20_guest_open_shader");
        assert!(!layer_return(&mut mem, 0x0004_020b, false));
        assert_eq!(get("layer_return_fp20_guest_open_shader"), open + 1);
        let down = get("layer_return_fp20_guest_standing_down");
        assert!(!layer_return(&mut mem, 0x0004_020b, true));
        assert_eq!(get("layer_return_fp20_guest_standing_down"), down + 1);
        // Another caller's layer draw is not the frame step's, and not counted.
        mem.set_u32(fl::SETUP + fl::SETUP_OPEN, 0).unwrap();
        mem.set_u32(fl::CURRENT_LAYER, 3).unwrap();
        let returns = || counters().into_iter().filter(|(k, _)| k.starts_with("layer_return_")).map(|(_, v)| v).sum::<u64>();
        let before = returns();
        assert!(!layer_return(&mut mem, 0x0004_1ffd, false), "a light pass's");
        mem.set_u32(fl::CURRENT_LAYER, 2).unwrap();
        assert!(!layer_return(&mut mem, 0x0002_c30b, false), "layer 2 from layer 17's site");
        mem.set_u32(fl::CURRENT_LAYER, 18).unwrap();
        assert!(!layer_return(&mut mem, 0x0002_c060, false), "layer 18 from no sorted handler");
        assert_eq!(returns(), before);
    }

    /// R5's layers return as the opaque block's do: the scene's 2 and 17,
    /// the alternate view's and first person's 2, and the sorted
    /// handlers' 18, 19 and 21, once no item holds the bit; an item kept
    /// runs the body, counted by its kind.
    #[test]
    fn r5s_layer_draws_return_when_every_item_is_native() {
        use crate::frame_step::layer as fl;
        use guest_layout::memory::FlatMemory;
        let get = |k: &str| counters().into_iter().find(|(n, _)| n == k).map_or(0, |(_, v)| v);
        let mut mem = FlatMemory::new(0x0045_0000, 0x7_0000);
        let (list, items) = (0x004a_0000, 0x004a_0100);
        mem.set_u32(fl::CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 4, 1).unwrap();
        mem.set_u32(list + 0x14, items).unwrap();
        mem.set_u32(items, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(items + 0xc, 3 << 8).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, 0xe000_0003).unwrap();
        for (layer, ret, site) in [
            (2, 0x0002_c060, ""),
            (2, 0x0002_bda8, "alt"),
            (2, 0x0004_0289, "fp"),
            (17, 0x0002_c30b, ""),
            (18, 0x0004_2015, ""),
            (19, 0x0004_205f, ""),
            (21, 0x0004_2721, ""),
        ] {
            assert_eq!(return_site(layer, ret), Some(site), "{layer} at {ret:#x}");
            mem.set_u32(fl::CURRENT_LAYER, layer).unwrap();
            mem.set_u32(items + 0x10, 0).unwrap();
            mem.set_u32(fl::DRAWN_LAYER, layer).unwrap();
            let returned = get(&format!("layer_return_{site}{layer}_returned"));
            assert!(layer_return(&mut mem, ret, false), "{layer} at {ret:#x}");
            assert_eq!(get(&format!("layer_return_{site}{layer}_returned")), returned + 1);
            assert_eq!(mem.u32(fl::DRAWN_LAYER).unwrap(), u32::MAX, "the body's writes made");
            mem.set_u32(items + 0x10, 1 << layer).unwrap();
            let object = get(&format!("layer_return_{site}{layer}_guest_object"));
            assert!(!layer_return(&mut mem, ret, false), "{layer}: an item kept runs the body");
            assert_eq!(get(&format!("layer_return_{site}{layer}_guest_object")), object + 1);
        }
        assert_eq!(return_site(17, 0x0002_bda8), None, "the alternate view has no layer 17");
        assert_eq!(return_site(16, scene_extract::late::FIRST_PERSON_OVERLAY_RETURN), None, "the first person's fog is the range hook's");
        assert_eq!(return_site(21, 0x0004_2015), None, "21 is the hologram's");
    }

    /// The scene's fog and water return once nothing holds the bit, and
    /// fog with its gate off returns with its items held; the body they
    /// run counts nothing again, as the fog and water count their own.
    #[test]
    fn the_scenes_fog_and_water_return_and_their_kept_items_are_counted_once() {
        use crate::frame_step::layer as fl;
        use guest_layout::memory::FlatMemory;
        use scene_extract::late::{SCENE_FOG_RETURN, SCENE_WATER_RETURN};
        let get = |k: &str| counters().into_iter().find(|(n, _)| n == k).map_or(0, |(_, v)| v);
        let mut mem = FlatMemory::new(0x0045_0000, 0x7_0000);
        let (list, items) = (0x004a_0000, 0x004a_0100);
        mem.set_u32(fl::CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 4, 1).unwrap();
        mem.set_u32(list + 0x14, items).unwrap();
        mem.set_u32(items, fl::GEOMETRY_BUILD).unwrap();
        mem.set_u32(items + 0xc, 3 << 8).unwrap();
        mem.set_u32(fl::RENDER_RECORDS + 3 * 0x20 + 4, u32::MAX).unwrap();
        for (layer, ret) in [(6, SCENE_WATER_RETURN), (16, SCENE_FOG_RETURN)] {
            assert_eq!(return_site(layer, ret), Some(""));
            mem.set_u32(fl::CURRENT_LAYER, layer).unwrap();
            mem.set_u32(fl::FOG_MODE, 3).unwrap();
            mem.set_u32(items + 0x10, 0).unwrap();
            let returned = get(&format!("layer_return_{layer}_returned"));
            assert!(layer_return(&mut mem, ret, false), "{layer}");
            assert_eq!(get(&format!("layer_return_{layer}_returned")), returned + 1);
            // An item kept, counted by the stage before the call.
            mem.set_u32(items + 0x10, 1 << layer).unwrap();
            let unnamed = get(&format!("scene_kept_{layer}_world_unnamed"));
            counts_own(&Ro(&mem), layer);
            assert!(!layer_return(&mut mem, ret, false), "{layer}: the body draws it");
            assert_eq!(get(&format!("scene_kept_{layer}_world_unnamed")), unnamed, "{layer}: counted once, by its stage");
            assert!(get(&format!("layer_return_{layer}_guest_structure")) > 0);
        }
        // Fog's gate off: the body builds nothing, so the held item does not hold the call.
        mem.set_u32(fl::FOG_MODE, 0).unwrap();
        mem.set_u32(fl::FOG_BYTES, 0).unwrap();
        assert!(layer_return(&mut mem, SCENE_FOG_RETURN, false), "gated fog returns");
        mem.set_u32(fl::CURRENT_LAYER, 6).unwrap();
        mem.set_u32(items + 0x10, 1 << 6).unwrap();
        assert!(!layer_return(&mut mem, SCENE_WATER_RETURN, false), "the fog gate is layer 16's alone");
    }

    /// Every item a layer call's guest body draws is one `scene_kept_*`
    /// count: one a stage counted before the call is not counted again,
    /// the rest take a stage's cause for their kind, else `unnamed`. An
    /// item with no render record builds no entry, and a kind a stage
    /// draws inside the body (layer 3's walk) is the stage's to count.
    #[test]
    fn every_item_the_guest_body_draws_is_counted_once_under_its_stages_cause() {
        let mut m = list(3, &[(1, false), (2, true), (4, false)]);
        // A decorator's item (another build) and an item with no record.
        m.set_u32(LIST + 4, 5).unwrap();
        let (other, unrecorded) = (LIST_ITEMS + 0x18 * 3, LIST_ITEMS + 0x18 * 4);
        m.set_u32(other, 0x0004_e000).unwrap();
        m.set_u32(other + 0x10, 1 << 3).unwrap();
        m.set_u32(unrecorded, native_render::skip::GEOMETRY_BUILD).unwrap();
        m.set_u32(unrecorded + 0xc, 0xffff_ff00).unwrap();
        m.set_u32(unrecorded + 0x10, 1 << 3).unwrap();
        let names = ["scene_kept_3_world_device", "scene_kept_3_world_unnamed", "scene_kept_3_object_unnamed", "scene_kept_3_other_decorator_state"];
        let before: Vec<u64> = names.iter().map(|n| kept(n)).collect();
        let delta = |b: &[u64]| names.iter().zip(b).map(|(n, b)| kept(n) - b).collect::<Vec<_>>();
        // The world's stage counts its walls; the decorators name theirs.
        let left = Left { layer: Some("device"), ..Left::default() };
        count_kept(&Ro(&m), 3, "", false, &left, None);
        leave(&Ro(&m), 3, Keep::Other, "decorator_state");
        assert!(!layer_return(&mut m, 0x0002_bed5, false), "the scene's layer 3 runs the body");
        assert_eq!(delta(&before), [2, 0, 1, 1]);
        // The next call: the walk owns the objects, no stage counted the walls.
        let before: Vec<u64> = names.iter().map(|n| kept(n)).collect();
        in_body(&Ro(&m), 3, Keep::Object);
        assert!(!layer_return(&mut m, 0x0002_bed5, false));
        assert_eq!(delta(&before), [0, 2, 0, 0], "the tally is the call's own");
    }

    /// The first person's layers count what they keep under `fp`, before
    /// the call by the host's cause and not again at its return; its layer
    /// 3, which no stage takes while its cut is held, under
    /// `first_person_held`.
    #[test]
    fn the_first_persons_kept_items_count_under_its_site() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_host(Arc::new(Refuses));
        let names = ["scene_kept_fp7_world_unread", "scene_kept_fp7_object_unread", "scene_kept_fp3_object_first_person_held"];
        let before: Vec<u64> = names.iter().map(|n| kept(n)).collect();
        let mut m = list(7, &[(1, false), (2, true)]);
        before_layer(&mut m, 0x0004_0271);
        assert!(!layer_return(&mut m, 0x0004_0271, false));
        let mut m = list(3, &[(2, true)]);
        assert!(!layer_return(&mut m, 0x0004_0136, false));
        let after: Vec<u64> = names.iter().map(|n| kept(n)).collect();
        assert_eq!(after.iter().zip(&before).map(|(a, b)| a - b).collect::<Vec<_>>(), [1, 1, 1]);
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// With no host the layer's items are counted `host` at the return.
    #[test]
    fn a_layer_with_no_host_counts_its_items_under_host() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        *HOST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let (world, object) = (kept("scene_kept_8_world_host"), kept("scene_kept_8_object_host"));
        let mut m = list(8, &[(1, false), (2, true)]);
        before_layer(&mut m, 0x0002_bfaa);
        assert!(!layer_return(&mut m, 0x0002_bfaa, false));
        assert_eq!((kept("scene_kept_8_world_host"), kept("scene_kept_8_object_host")), (world + 1, object + 1));
    }

    #[test]
    fn a_cleared_bit_goes_back_only_on_the_same_item() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        use guest_layout::memory::FlatMemory;
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
