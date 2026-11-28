//! Scene objects drawn through resident render models and the game's pose
//! and material inputs. The walk reads the sorted layer-3 entries before
//! the guest prepares their draws. A complete span of objects returns
//! directly from the geometry handler after GPU reservation. It uses the
//! render context's snapshot of guest memory.
//!
//! Objects draw from the scene, in [`SLOT`]. An object the walk cannot
//! take (an unreadable part, an interleaved span, a refused reservation)
//! keeps its guest handler's draw ([`keep`]): it is counted
//! `native_objects_walk_fallback` with its cause's `walk_*` column, and
//! `scene_kept_3_object_<cause>` in `render.scene`. Nothing is dropped.
//! Their setup (`OBJECT_RENDER_SETUP`) is Rust's where it can build the
//! call and the guest's where it cannot (docs/graphics/native-objects.md,
//! "Object setup").

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use frame_scene::{FrameScene, View, ViewKind, ViewSlot};
use native_render::camera::GuestRead;
use native_render::object_walk::{self, Entry, Run, SortedObjects};
use native_render::objects::ItemKey;
use native_render::world_layers::{Site, layer_site};
use timeline::native::{Metric, ObjectWalk, ReservationCause, Stage, Timer};

use crate::native_world::Mem;
use hooks::{BuildError, Builder, Feature, Manifest};

pub mod before;

pub const ID: &str = "native-objects";

/// The view slot the layer-3 objects draw in: slot 1, with the opaque base.
pub const SLOT: ViewSlot = ViewSlot::OpaqueBase;

pub fn on(_: &hooks::Switches) -> bool {
    true
}

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
    /// layer-3 item's render record names ([`ItemKey`]). An item
    /// is offered to the host only when its shader is the part's and its
    /// record's implementation one the part has; the rest keep their D3D
    /// draws.
    pub parts: HashMap<(u16, u16, u8), SetPart>,
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

use crate::native_scene_lists::{LodDetail, ViewLodScale};

/// What draws the objects: boot's glue over the GPU models and the device.
pub trait Host: Send + Sync {
    fn retire_map(&self) {}

    /// Start the tag-index load on map activation or a snapshot's first view.
    fn preload(&self, _mem: &dyn GuestMemory) {}

    /// Draw objects reached only by the presentation lens into the display attachments.
    fn draw_display_objects(&self, _mem: &mut dyn GuestMemory, _view: &View, _lod: LodDetail) {}

    /// Reserve and commit all of a sorted run (`run`, the walk's entries
    /// from slot `first`). `view` is the frame step's open view with the
    /// scene it is in ([`crate::scene_step`]): the camera, and each
    /// object's frame and each part's rows, as extraction read them at the
    /// walk's start; `None` when the view has no camera. `consume` updates
    /// the guest loop only after all resources and both target copies are
    /// ready.
    fn draw_walk(
        &self,
        _mem: &mut dyn GuestMemory,
        _view: Option<(&FrameScene, &View)>,
        _slot: ViewSlot,
        _first: usize,
        _run: &[Entry],
        _display_lod: Option<LodDetail>,
        _consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        Stage::Objects.reservation_cause(ReservationCause::Declined);
        Err(ObjectWalk::Reservation)
    }

    /// [`Host::draw_walk`] before the layer draw, from the Rust-built
    /// list ([`before`]): the layer begin's `ZENABLE` applied and the
    /// shader cache the cleared one the layer setup leaves. `consume`
    /// clears the run's items' bit 3.
    #[allow(clippy::too_many_arguments)]
    fn draw_before(
        &self,
        _mem: &mut dyn GuestMemory,
        _view: Option<(&FrameScene, &View)>,
        _slot: ViewSlot,
        _first: usize,
        _run: &[Entry],
        _display_lod: Option<LodDetail>,
        _consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        Stage::Objects.reservation_cause(ReservationCause::Declined);
        Err(ObjectWalk::Reservation)
    }

    /// What a run's plan reads of the device and the engine, by field
    /// ([`before::Field`]): as the device holds it now, or `predict`ed for
    /// the layer's first geometry handler from the device before the call.
    /// `None` without a device.
    fn entry_state(&self, _mem: &dyn GuestMemory, _predict: bool) -> Option<Vec<before::Field>> {
        None
    }
}

struct WalkLayer {
    kind: ViewKind,
    list: u32,
    base: u32,
    counter: u32,
    initialized: bool,
    entries: Vec<Entry>,
    runs: VecDeque<Run>,
    display_lod: Option<LodDetail>,
}

thread_local! {
    static WALK_LAYER: RefCell<Option<WalkLayer>> = const { RefCell::new(None) };
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn begin_walk(mem: &mut dyn GuestMemory, ret: u32, lod: Option<ViewLodScale>) {
    WALK_LAYER.with(|state| *state.borrow_mut() = None);
    before::restore_pending(mem);
    let kind = match layer_site(3, ret) {
        Site::Scene => ViewKind::Game,
        Site::AlternateView => ViewKind::Alternate,
        _ => return,
    };
    use crate::native_layers::{Keep, in_body, leave};
    let lightmap = native_render::skip::LIGHTMAP_LAYER;
    let Some(host) = host() else {
        leave(&Mem(mem), lightmap, Keep::Object, "walk_host");
        return;
    };
    // Use the same map generation check as activation on snapshot loads.
    host.preload(mem);
    let display = lod.map_or(LodDetail::Retail, |lod| lod.display(mem));
    if kind == ViewKind::Game
        && let Some(view) = crate::scene_step::open_view(mem)
        && view.display_visibility.as_ref().is_some_and(|v| !v.objects.is_empty())
    {
        host.draw_display_objects(mem, &view, display);
    }
    let Some(list) = Mem(mem).read_u32(native_render::skip::CURRENT_LIST) else {
        leave(&Mem(mem), lightmap, Keep::Object, "walk_list");
        return;
    };
    // The walk draws or counts each object item at its handler, or the
    // draw before the call did.
    in_body(&Mem(mem), lightmap, Keep::Object);
    let display_lod = lod.filter(|lod| lod.read(mem) != display).map(|_| display);
    if before::at_layer(mem, &*host, kind, list, display_lod) {
        return;
    }
    WALK_LAYER.with(|state| {
        *state.borrow_mut() = Some(WalkLayer {
            kind,
            list,
            base: 0,
            counter: 0,
            initialized: false,
            entries: Vec::new(),
            runs: VecDeque::new(),
            display_lod: lod.filter(|lod| lod.read(mem) != display).map(|_| display),
        })
    });
}

fn initialize_walk(layer: &mut WalkLayer, mem: &mut dyn GuestMemory, esp: u32, esi: u32) {
    let _planning = Timer::start(Stage::Objects, Metric::PrepareNs);
    layer.initialized = true;
    let site = kept_site(layer.kind);
    let Some(SortedObjects { base, current, counter, entries, causes }) = object_walk::read(&Mem(mem), layer.list, esp, esi) else {
        before::end_layer();
        // The sorted entries do not read: every object keeps its handler's
        // draw, one count for the list as `native_layers::count_kept`'s.
        ObjectWalk::Unreadable.count(1);
        crate::native_layers::count_kept_as(site, 3, "unreadable", "list", 1);
        return;
    };
    if let Some(host) = host() {
        before::compare(&*mem, &*host, &entries, current);
    }
    ObjectWalk::PartAuxState.count(causes.aux_state);
    ObjectWalk::PartSubparts.count(causes.subparts);
    ObjectWalk::PartShaderFlag.count(causes.shader_flag);
    ObjectWalk::PartArgument.count(causes.argument);
    ObjectWalk::PartProgram.count(causes.program);
    ObjectWalk::PartLayout.count(causes.layout);
    if !crate::scene_step::objects(&*mem, layer.kind, &entries) {
        // The scene holds none of them: each keeps its handler's draw.
        let n = scene_refused(&entries[current.min(entries.len())..]);
        ObjectWalk::Fallback.count(n);
        crate::native_layers::count_kept_as(site, 3, "object", SCENE_REFUSED, n as u64);
        return;
    }
    let decision = object_walk::decide(&entries, current);
    // Neither span is a run: each of its objects keeps its guest handler's
    // draw as the handler comes up (`walk_geometry`, [`HandlerAt::Guest`]).
    keep(site, ObjectWalk::Unsupported, decision.unsupported);
    keep(site, ObjectWalk::Order, decision.interleaved);
    layer.base = base;
    layer.counter = counter;
    layer.entries = entries;
    layer.runs = decision.runs.into();
}

fn walk_geometry(c: &mut hooks::Call) -> Result<hooks::Flow, runtime::GuestFault> {
    use hooks::Flow;
    if WALK_LAYER.with(|state| state.borrow().is_none()) {
        return Ok(Flow::Continue);
    }
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let esp = c.cpu.esp;
    let esi = c.cpu.esi;
    let Ok(caller) = mem.u32(esp) else { return Ok(Flow::Continue) };
    if caller != native_render::light_list::HANDLER_RETURN {
        return Ok(Flow::Continue);
    }
    // Move the plan out while interacting with the device. No RefCell
    // borrow survives an external boundary.
    let Some(mut layer) = WALK_LAYER.with(|state| state.borrow_mut().take()) else {
        return Ok(Flow::Continue);
    };
    if !layer.initialized {
        initialize_walk(&mut layer, &mut *mem, esp, esi);
    }
    let mut consumed = false;
    let stride = native_render::skip::ITEM_SIZE;
    let counter = layer.counter;
    // The slot the handler is at, when it is one of the walk's entries.
    let slot = esi.checked_sub(layer.base).filter(|d| d % stride == 0).map(|d| (d / stride) as usize);
    if handler_at(layer.entries.len(), layer.runs.front(), slot) == HandlerAt::Run {
        let run = layer.runs.pop_front().expect("matched the front");
        let skipped = (run.slots.len() - 1) as u32;
        let remaining = (layer.entries.len() - run.slots.start) as u32;
        let last = esi + skipped * stride;
        let mut consume = |mem: &mut dyn GuestMemory| {
            mem.u32(counter) == Ok(remaining)
                && mem.u32(esp) == Ok(caller)
                && mem.set_u32(counter, remaining - skipped).is_ok()
        };
        let first = run.slots.start;
        let result = host().ok_or_else(|| {
            // No host installed: nothing took the run.
            Stage::Objects.reservation_cause(ReservationCause::Declined);
            ObjectWalk::Reservation
        }).and_then(|host| {
            crate::scene_step::with_open(|view| {
                host.draw_walk(&mut *mem, view, SLOT, first, &layer.entries[run.slots], layer.display_lod, &mut consume)
            })
        });
        match result {
            Ok(()) => {
                c.cpu.esi = last;
                ObjectWalk::Native.count(run.objects);
                consumed = true;
            }
            // The host took nothing: the run's handlers draw it, from this
            // one on (each later slot is [`HandlerAt::Guest`]).
            Err(reason) => keep(kept_site(layer.kind), reason, run.objects),
        }
    }
    WALK_LAYER.with(|state| *state.borrow_mut() = Some(layer));
    if consumed {
        drop(mem);
        c.ret(engine::render_loop::layer::HANDLER_CLEANUP).map(Flow::Return)
    } else {
        Ok(Flow::Continue)
    }
}

/// The cause of the objects a walk keeps when the scene step refuses
/// their extraction (`scene_kept_<site>3_object_scene`).
pub const SCENE_REFUSED: &str = "scene";

/// The objects of `entries` (each once, by handle): what a walk the scene
/// refused leaves to the guest handlers.
pub fn scene_refused(entries: &[Entry]) -> usize {
    let mut seen = std::collections::HashSet::new();
    entries.iter().filter(|e| matches!(e, Entry::Object { handle, .. } if seen.insert(*handle))).count()
}

/// What the walk does at a geometry handler call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlerAt {
    /// The first slot of the next admitted run: the host is asked to draw
    /// the run whole.
    Run,
    /// Any other call: an object of a span the walk did not admit or of a
    /// run the host refused, an entry that is not an object, or a call
    /// outside the walk's entries. Its guest handler draws.
    Guest,
}

/// The handler call at `slot` (`None` outside the list) of a walk of
/// `entries` entries whose next admitted run is `next`.
pub fn handler_at(entries: usize, next: Option<&Run>, slot: Option<usize>) -> HandlerAt {
    match (slot, next) {
        (Some(slot), Some(run)) if slot < entries && run.slots.start == slot => HandlerAt::Run,
        _ => HandlerAt::Guest,
    }
}

/// `objects` the walk does not draw, under `reason`: they keep their
/// guest handlers' draws. Counted `native_objects_walk_fallback` and the
/// cause's `walk_*` column, and `scene_kept_<site>3_object_<cause>` in
/// `render.scene` (`site` from [`kept_site`]).
pub fn keep(site: &str, reason: ObjectWalk, objects: usize) {
    if objects == 0 {
        return;
    }
    reason.fallback(objects);
    crate::native_layers::count_kept_as(site, 3, "object", reason.name(), objects as u64);
}

/// The `scene_kept_*` site of a walk's view: `""` for the scene, `alt`
/// for the alternate view.
pub fn kept_site(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::Alternate => "alt",
        _ => "",
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Drops the walk's layer: a snapshot load, whose guest RAM is not the
/// memory it was read from. The loaded item lists are the loaded
/// render's, built again each frame.
pub fn forget_pending() {
    WALK_LAYER.with(|state| *state.borrow_mut() = None);
    before::forget();
}

pub(crate) fn retire_map() {
    if let Some(host) = host() { host.retire_map(); }
}

pub struct NativeObjects;

impl NativeObjects {
    pub fn with_lod_scale(lod: Option<ViewLodScale>) -> impl Feature {
        SceneObjects(lod)
    }
}

impl Feature for NativeObjects {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Objects' render models drawn and skinned by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        SceneObjects(None).build(b)
    }
}

struct SceneObjects(Option<ViewLodScale>);

impl Feature for SceneObjects {
    fn manifest(&self) -> Manifest {
        NativeObjects.manifest()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Switched off: no hooks, nothing held. On: the host's models are
        // keyed by the map name it reads from guest RAM at every ask, so
        // a load re-keys them. The walk's layer is dropped at a load
        // ([`forget_pending`]).
        if !on(b.switches()) {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            return Ok(());
        }
        b.snapshot(hooks::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_pending();
            Ok(())
        }));
        b.game()
            .on_start("native_objects_preload", |o| {
                if let Some(host) = host() {
                    host.preload(&*o.guest());
                }
            })
            .after_feature(crate::native_map_work::ID);
        let lod = self.0;
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_objects_skip", move |o| {
                if crate::native_map_work::Stage::Objects.standing_down() {
                    return;
                }
                let mut g = o.guest();
                let ret = g.u32(o.cpu.esp).unwrap_or(0);
                begin_walk(&mut *g, ret, lod);
            })
            .after_feature(crate::render_timing::ID)
            .after_feature(crate::scene_step::ID)
            .after_feature(crate::native_world::ID)
            .context_only();
        b.raw()
            .before(&points::LIGHT_GEOMETRY_DRAW, "native_object_walk", walk_geometry)
            .after_feature(crate::native_lights::ID)
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "native_object_walk_end", |_| {
                WALK_LAYER.with(|state| *state.borrow_mut() = None);
                before::end_layer();
            })
            .context_only();
        Ok(())
    }
}
