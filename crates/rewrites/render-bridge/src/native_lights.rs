//! Dynamic light geometry. The gathered walk can build and sort entries
//! before the original geometry handlers. The resolved-draw path below
//! admits individual packets when those handlers execute, for each of a
//! light's layers 12 and 13 its passes draw. At the passes' entry the
//! light and its steps (`scene_extract::layout::light_passes`) go into
//! the frame step's open view. The light frame body ([`frame`]) runs
//! `RENDER_DYNAMIC_LIGHTS` itself, [`items`] a light's item lists at
//! `SCENE_ITEMS`, and [`passes`] a light's passes at
//! `RENDER_LIGHT_PASSES`. Under the body the scene's light is made from its
//! hand-off and the item body's lists ([`handoff`]); the guest loop's
//! lights are read at their constants call. The first-person lights
//! (`0x000404b0`) and each slot's record replay run as bodies too
//! ([`first_person`]); a first-person light goes into the first person's
//! view at its first layer draw, its groups are of their own site
//! (`GroupSite::FirstPerson`), and its layers 12 and 13 whose groups the
//! scene holds whole are drawn from it alone ([`FIRST_PERSON_LAYERS`]).
//! On with the native renderer (`impl.render.native`, on by default).
mod additional;
pub mod chosen;
mod first_person;
pub mod slot_visibility;
mod frame;
mod handoff;
pub mod host;
mod items;
mod object_shadows;
pub mod object_shadow_scene;
mod passes;
pub mod scene_frame;
mod typed;
mod typed_groups;
pub mod walk;

pub use handoff::Source;
pub use typed_groups::GroupRefusal;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use engine::render::light;
use native_render::camera::GuestRead;
use native_render::light_list::{self, SortedCall};
use native_render::light_passes;
use native_render::lights::{Light, LightConstants};
use native_render::skip;

use crate::native_map_work::Stage;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-lights";

/// Whether the native lights are on.
pub fn on(_: &hooks::Switches) -> bool {
    true
}

/// The light's list, which `RENDER_LIGHT_PASSES` makes current for its
/// layers (`0x000312c4`; a list as `native_render::skip` reads it: its
/// count at `+4`, the light's first item at `+0xc`, the items at `+0x14`).
pub const LIGHT_LIST: u32 = 0x004c_1a60;

/// `RENDER_LIGHT_PASSES`' draws of the light's own list, by the layer
/// draw's return address: 14 (`0x000313dd`), 12 (`0x0003144a`) and 13
/// (`0x000314ca`). The gathered walk takes any of them; the resolved-draw
/// path takes 12 and 13 ([`RESOLVED_LAYERS`]). Layers 10 and 11 are the
/// shadow stage's (`native_render::shadows::site`); the first-person
/// lights' draws on the same list are [`FIRST_PERSON_LAYERS`].
pub const SPLIT_LAYERS: [(u32, u32); 3] = [(14, 0x0003_13e2), (12, 0x0003_144f), (13, 0x0003_14cf)];

/// The first-person lights' (`0x000404b0`) draws of the light's list, by
/// the layer draw's return address, in their order: 11 (`0x00040642`),
/// 14 (`0x000406ab`), 12 (`0x00040718`) and 13 (`0x0004079c`). Their 12
/// and 13 are drawn from the scene alone when the scene holds the group
/// whole (`walk`); every other draw, and a group the host refuses, keeps
/// its handlers. The resolved-draw path takes none of them. Under the
/// first-person body ([`first_person`]) the draw's caller is read through
/// `passes::caller`.
pub const FIRST_PERSON_LAYERS: [(u32, u32); 4] = [
    (11, 0x0004_0642),
    (14, 0x0004_06ab),
    (12, 0x0004_0718),
    (13, 0x0004_079c),
];

/// Where the light layers draw in the view's pass graph: every light's
/// layers 10, 11, 14, 12 and 13 in slot 4 ([`SCENE_PASSES`]), the
/// first-person lights' in slot 10. The scene's groups give both orders
/// whatever the view's render scale ([`native_render::light_scene::passes`]).
pub const LIGHT_SLOT: frame_scene::ViewSlot = frame_scene::ViewSlot::LightAccumulation;
pub const FIRST_PERSON_SLOT: frame_scene::ViewSlot = frame_scene::ViewSlot::FirstPerson;
pub use native_render::light_scene::{PER_LIGHT_LAYERS as SCENE_PASSES, passes as scene_passes};

/// One light layer of the open view, owned, in the slot it draws in.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenePass {
    pub slot: frame_scene::ViewSlot,
    pub light: frame_scene::Light,
    pub group: SceneGroup,
}

/// What the host is asked to draw: `view`'s light layers in the order the
/// game draws them ([`native_render::light_scene::order`]): slot 4's, then
/// slot 10's.
pub struct SceneDraw<'a> {
    pub passes: &'a [ScenePass],
    pub view_target: [u32; 8],
    /// The serial of the first pass; pass `n` takes `serial + n`.
    pub serial: u64,
}

/// `view`'s light layers, owned, in draw order.
pub fn gather_scene_passes(scene: &frame_scene::FrameScene, view: &frame_scene::View) -> Vec<ScenePass> {
    native_render::light_scene::order(scene, view)
        .into_iter()
        .map(|(slot, light, group)| {
            let entries = scene.entries_of(group).to_vec();
            let rows = entries.iter().map(|e| scene.pose_rows(e).to_vec()).collect();
            ScenePass {
                slot,
                light: scene.lights[light as usize].clone(),
                group: SceneGroup { group: group.clone(), entries, rows },
            }
        })
        .collect()
}

/// Whether the scene's own light path runs this frame: the host draws
/// the scene's light layers, so the guest's light frame is cut
/// ([`crate::cut`], row `dynamic_lights`). Without it the frame body
/// gathers and the guest's passes draw.
/// Whether a renderer (a host) is set. Without one nothing draws.
pub fn has_host() -> bool {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub fn scene_path_ready() -> bool {
    HOST.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|h| h.draws_scene())
}

/// What the host holds for an entry's state ([`Host::held_state`]);
/// nothing without a host.
pub fn held_state(mem: &dyn GuestMemory) -> frame_scene::EntryState {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    host.map_or_else(frame_scene::EntryState::default, |h| h.held_state(mem))
}

static SCENE_DRAWN: AtomicU64 = AtomicU64::new(0);

/// Light layers the host drew from the scene's groups (slots 4 and 10).
pub fn scene_drawn() -> u64 {
    SCENE_DRAWN.load(Ordering::Relaxed)
}

/// Draws the open view's light layers from the scene, once its light pass
/// is over: the native producer the cut `dynamic_lights` leaves the frame
/// to. Returns the layers drawn.
pub fn draw_scene(mem: &mut dyn GuestMemory, passes: &[ScenePass]) -> usize {
    if passes.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    let Some(view_target) = VIEW_TARGET.get() else {
        refused("light view target unavailable");
        return 0;
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return 0;
    };
    host.flush_light_run(mem);
    let serial = SERIAL.fetch_add(passes.len() as u64, Ordering::Relaxed);
    let drawn = host.draw_scene(mem, SceneDraw { passes, view_target, serial });
    SCENE_DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    drawn
}

/// Draws one cut light's layers (`passes`, all of one light) now, at its
/// passes `0x00031170` or its first-person slot, on `view_target`: the
/// place the guest's own passes would draw them. `false` when the host
/// refuses or is not there, or the target is unknown: nothing is drawn,
/// and the caller runs the guest's passes, so a light the host cannot
/// take is never dropped. A light with no drawn layer draws nothing.
pub(crate) fn draw_cut_light(
    mem: &mut dyn GuestMemory,
    passes: &[ScenePass],
    view_target: Option<[u32; 8]>,
) -> bool {
    if passes.is_empty() {
        return true;
    }
    if Stage::Lights.standing_down() {
        return false;
    }
    let (Some(view_target), Some(host)) = (view_target, HOST.lock().unwrap_or_else(|e| e.into_inner()).clone())
    else {
        return false;
    };
    let serial = SERIAL.fetch_add(passes.len() as u64, Ordering::Relaxed);
    // A light of slot 4 the host can draw tiled joins its run, drawn when
    // the run ends ([`flush_light_run`]); any other draws after the run.
    let draw = SceneDraw { passes, view_target, serial };
    if passes[0].slot == LIGHT_SLOT && host.hold_scene_light(mem, &draw) {
        SCENE_DRAWN.fetch_add(passes.len() as u64, Ordering::Relaxed);
        return true;
    }
    host.flush_light_run(mem);
    if !host.draw_scene_light(mem, draw) {
        return false;
    }
    SCENE_DRAWN.fetch_add(passes.len() as u64, Ordering::Relaxed);
    true
}

/// Draws the host's held run of lights ([`Host::hold_scene_light`]) now:
/// before anything else draws on the view's target (a light the guest or
/// the host draws per light, the first-person lights) and at the end of
/// the view's light frame. Returns the lights drawn.
pub fn flush_light_run(mem: &mut dyn GuestMemory) -> usize {
    match HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(host) => host.flush_light_run(mem),
        None => 0,
    }
}

/// The game view's target, read at the view's start.
pub(crate) fn view_target() -> Option<[u32; 8]> {
    VIEW_TARGET.get()
}

/// The target the device has bound now.
pub(crate) fn bound_target() -> Option<[u32; 8]> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()?.target()
}

/// First-person slots the scene path drew.
pub fn first_person_scene_slots() -> u64 {
    first_person::scene_slots()
}

/// Draws the first-person lights' layers the scene path took
/// (`first_person::run_scene`) at the loop's exit, on the target then
/// bound. Returns the layers drawn.
pub fn draw_first_person_scene(mem: &mut dyn GuestMemory, passes: &[ScenePass]) -> usize {
    if passes.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return 0;
    };
    let Some(view_target) = host.target() else {
        refused("first-person light view target unavailable");
        return 0;
    };
    host.flush_light_run(mem);
    let serial = SERIAL.fetch_add(passes.len() as u64, Ordering::Relaxed);
    let drawn = host.draw_scene(mem, SceneDraw { passes, view_target, serial });
    SCENE_DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    drawn
}

/// Whether a layer draw returning to `ret` is the first-person lights'
/// draw of `layer`.
pub fn first_person_site(layer: u32, ret: u32) -> bool {
    FIRST_PERSON_LAYERS.contains(&(layer, ret))
}

/// The layers the resolved-draw path admits: diffuse and specular.
pub const RESOLVED_LAYERS: u32 = 1 << 12 | 1 << 13;

pub struct Dispatch {
    pub light: Light,
    pub serial: u64,
    /// The layer being drawn, 12 or 13.
    pub layer: u32,
    pub shader_gel: Option<(u32, u32)>,
    pub view_target: [u32; 8],
    pub sorted: SortedCall,
}

/// A gathered group as the scene holds it (`frame_scene::LightGroup`),
/// copied out of the frame step: its entries and each entry's node rows
/// (empty but for an object's).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneGroup {
    pub group: frame_scene::LightGroup,
    pub entries: Vec<frame_scene::LightEntry>,
    pub rows: Vec<Vec<frame_scene::Row>>,
}

impl SceneGroup {
    /// The node rows of `entry`, one of [`Self::entries`].
    pub fn rows_of(&self, entry: &frame_scene::LightEntry) -> &[frame_scene::Row] {
        self.entries
            .iter()
            .position(|e| std::ptr::eq(e, entry))
            .and_then(|n| self.rows.get(n))
            .map_or(&[], Vec::as_slice)
    }
}

/// A layer the light's steps draw, whose group the scene holds whole: the
/// host may draw it from the scene alone, and then no handler runs
/// (framescene.md, slice 4).
pub struct Alone<'a> {
    pub light: Light,
    pub serial: u64,
    /// 12 or 13.
    pub layer: u32,
    pub shader_gel: Option<(u32, u32)>,
    pub view_target: [u32; 8],
    pub group: &'a SceneGroup,
}

/// A shadow layer whose whole gathered group is in the scene.
pub struct ShadowAlone<'a> {
    pub view: u32,
    pub group: &'a SceneGroup,
    /// The running dynamic light's static gel, when it has one.
    pub light: Option<LightConstants>,
}

pub trait Host: Send + Sync {
    fn retire_map(&self) {}

    /// Plan a sorted prefix of one layer. The handlers still emit their
    /// D3D packets so each native draw can fall back in the same queue
    /// slot. A light asks once per layer it draws.
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch) -> bool;
    fn clear_admission(&self);
    /// Before the guest constants or gel quad for this invocation.
    fn checkpoint_gel(&self, serial: u64);
    fn target(&self) -> Option<[u32; 8]>;
    /// Draw `alone`'s layer from the scene at this point of the device's
    /// queue, before its handlers would run; `false` leaves the layer to
    /// them, nothing issued.
    fn draw_alone(&self, _mem: &mut dyn GuestMemory, _alone: Alone<'_>) -> bool {
        false
    }
    /// Draw the host's own lights' layers 12 and 13 ([`host`]) in the view
    /// whose light pass is over: each [`host::HostLight`] a sphere of its
    /// constants, the geometry it touches the host's to choose. Returns
    /// the lights drawn; a light it keeps draws nothing.
    fn draw_host(&self, _mem: &mut dyn GuestMemory, _draw: host::HostDraw<'_>) -> usize {
        0
    }
    /// Whether the host draws the scene's light layers ([`Self::draw_scene`])
    /// and the frame fills the scene's groups itself, so the guest's light
    /// frame can be cut whole.
    fn draws_scene(&self) -> bool {
        false
    }
    /// The entry state ([`frame_scene::EntryState`]) the host can draw a
    /// layer 12 or 13 entry with now: `aux_depth` while the device holds
    /// what the auxiliary state toggle leaves off and the toggle's values
    /// read, `fade` while the stipple is off and at rest. An entry with
    /// state the host does not hold keeps the guest's passes
    /// ([`held_state`]).
    fn held_state(&self, _mem: &dyn GuestMemory) -> frame_scene::EntryState {
        frame_scene::EntryState::default()
    }
    /// Draw `draw`'s light layers from the scene, slot 4's and then slot
    /// 10's, in the view whose light pass is over. Returns the layers
    /// drawn; one it keeps draws nothing.
    fn draw_scene(&self, _mem: &mut dyn GuestMemory, _draw: SceneDraw<'_>) -> usize {
        0
    }
    /// Take one cut light of slot 4 into the run of lights drawn tiled
    /// (docs/graphics/native-light-tiles.md): `true` holds it, and
    /// [`Self::flush_light_run`] draws it with the run's others or, when
    /// the run cannot draw tiled, on its per-light pass. `false` holds
    /// nothing; the caller flushes the run and draws the light per light.
    fn hold_scene_light(&self, _mem: &mut dyn GuestMemory, _draw: &SceneDraw<'_>) -> bool {
        false
    }
    /// Draws the held run, if any; the lights drawn.
    fn flush_light_run(&self, _mem: &mut dyn GuestMemory) -> usize {
        0
    }
    /// Draw one cut light's layers now, where its passes or its
    /// first-person slot stand ([`draw_cut_light`]), as one sequence.
    /// `false` issues nothing, and the guest's passes draw the light.
    fn draw_scene_light(&self, _mem: &mut dyn GuestMemory, _draw: SceneDraw<'_>) -> bool {
        false
    }
    /// Draw the view's object shadows from the scene in slot 3, before
    /// layer 7. Returns the shadows drawn; one it keeps draws nothing.
    fn draw_object_shadows(
        &self,
        _mem: &mut dyn GuestMemory,
        _draw: object_shadow_scene::ObjectShadowDraw<'_>,
    ) -> usize {
        0
    }
    /// Draw E3's chosen lights beyond the guest's ([`chosen`]) in the view
    /// whose light pass is over: each [`chosen::ChosenLight`]'s layers 12
    /// and 13 over the geometry its sphere meets, queued as a scene stage
    /// of E3 alone (`native_render::Stage::lighting_only`). Returns the
    /// lights queued; a light it keeps draws nothing.
    fn draw_chosen(&self, _mem: &mut dyn GuestMemory, _draw: chosen::ChosenDraw<'_>) -> usize {
        0
    }
    /// Draw E3's chosen first-person lights that retail's first-person
    /// pass did not record ([`chosen::first_person`]): each light's layers
    /// 12 and 13 over the first-person view's object parts, posed by
    /// their node rows, queued as a scene stage of E3 alone. Returns the
    /// lights queued; a light it keeps draws nothing.
    fn draw_first_person(
        &self,
        _mem: &mut dyn GuestMemory,
        _draw: chosen::first_person::FirstPersonDraw<'_>,
    ) -> usize {
        0
    }
    /// Draw a caster group from the scene, or return the refusal category
    /// the shadow stage counts. No draw is issued on refusal.
    fn draw_shadow_alone(
        &self,
        _mem: &mut dyn GuestMemory,
        _alone: ShadowAlone<'_>,
    ) -> Result<(), &'static str> {
        Err("device")
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// The light whose passes run: the layers of [`RESOLVED_LAYERS`] its
/// steps draw and that have not run yet, and the one running.
struct Candidate {
    light: Light,
    serial: u64,
    shader_gel: Option<(u32, u32)>,
    layers: u32,
    inside: Option<u32>,
    /// A first-person light's (`0x000404b0`): its layers are drawn only
    /// alone, and the object shadows do not read its constants.
    first_person: bool,
}
thread_local! {
    static CANDIDATE: RefCell<Option<Candidate>> = const { RefCell::new(None) };
    static VIEW_TARGET: Cell<Option<[u32; 8]>> = const { Cell::new(None) };
    /// The first-person light whose layers draw: its entry's datum
    /// address in `light_passes::FIRST_PERSON_LIGHTS` and the order (in
    /// [`FIRST_PERSON_LAYERS`]) of its last layer draw.
    static FIRST_PERSON: Cell<Option<(u32, usize)>> = const { Cell::new(None) };
}

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub(crate) fn retire_map() {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(host) = host {
        host.retire_map();
    }
}

/// Forward a scene caster group to the light material and geometry owner.
pub fn draw_shadow_alone(
    mem: &mut dyn GuestMemory,
    view: u32,
    group: &SceneGroup,
) -> Result<(), &'static str> {
    if Stage::Lights.standing_down() {
        return Err("device");
    }
    let light = CANDIDATE.with_borrow(|candidate| {
        candidate
            .as_ref()
            .filter(|c| !c.first_person)
            .and_then(|c| c.light.constants)
    });
    let host = HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("device")?;
    host.draw_shadow_alone(mem, ShadowAlone { view, group, light })
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// A light's passes are over: a layer it was to draw that never ran is
/// refused.
fn finish_candidate() {
    if CANDIDATE.take().is_some_and(|c| c.layers != 0) {
        refused("no eligible sorted geometry dispatch");
    }
}

/// The running layer is over, or another stage owns it: it leaves the
/// candidate, whose other layer stays.
fn leave_layer() -> Option<u32> {
    CANDIDATE.with_borrow_mut(|slot| {
        let c = slot.as_mut()?;
        let layer = c.inside.take()?;
        c.layers &= !(1 << layer);
        Some(layer)
    })
}

/// The dynamic light at an indexed emission from its diffuse or specular
/// layer. The sorted reservation may already have consumed the layer bit.
pub struct IndexedLight {
    pub targets: [u32; 2],
    pub serial: u64,
    pub shader_gel: Option<(u32, u32)>,
}

pub fn indexed_light(layer: u32, caller: u32, list: u32) -> Option<IndexedLight> {
    if !matches!(layer, 12 | 13) || list != LIGHT_LIST || !SPLIT_LAYERS.contains(&(layer, caller)) {
        return None;
    }
    CANDIDATE.with_borrow(|candidate| {
        let current = candidate.as_ref().filter(|c| !c.first_person)?;
        let [color, depth, ..] = VIEW_TARGET.get()?;
        Some(IndexedLight {
            targets: [color, depth],
            serial: current.serial,
            shader_gel: current.shader_gel,
        })
    })
}

pub fn new_view() {
    handoff::new_view();
    finish_candidate();
    FIRST_PERSON.set(None);
    SERIAL.fetch_add(1, Ordering::Relaxed);
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let target = host.and_then(|h| {
        h.clear_admission();
        h.target()
    });
    VIEW_TARGET.set(target);
}

/// Forgets the pending light candidate and the view's target, without
/// refusing or setting anything back: a snapshot load, whose guest RAM is
/// not the memory they were read from. Takes no device lock (unlike
/// [`new_view`], which asks the host for the next view's target).
pub fn forget_runs() {
    slot_visibility::forget();
    first_person::forget();
    handoff::forget();
    CANDIDATE.take();
    VIEW_TARGET.set(None);
    FIRST_PERSON.set(None);
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        host.clear_admission();
    }
}

/// The light `handle` as the passes would draw it, with the caller's
/// shadow decision and weight. `None` when the record cannot be read, is
/// gone, or fails the record's checks (`light::LightRecord::check`).
pub fn read_light(mem: &dyn GuestMemory, handle: u32, shadow: bool, weight: f32) -> Option<Light> {
    let r = light::read(mem, DatumHandle(handle)).ok()??.ok()?;
    r.check().ok()?;
    Some(Light {
        handle,
        definition: r.definition.0,
        position: r.position,
        forward: r.forward,
        up: r.up,
        colour_a: r.colour_a,
        colour_b: r.colour_b,
        intensity: r.intensity,
        bounds: (r.bounds_centre, r.bounds_radius),
        shadow,
        weight,
        constants: None,
    })
}

fn refused(why: &'static str) -> bool {
    timeline::native::Stage::Lights.count(timeline::native::Metric::RefusedOwnership, 1);
    static SEEN: std::sync::LazyLock<Mutex<HashSet<&'static str>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));
    if SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(why) {
        eprintln!("native-lights: a light keeps its D3D passes: {why}");
    }
    false
}

/// The registers at the light passes' entry the constants need: EAX the
/// `ligh` datum, ECX the light volume, ESP (the return address; the light
/// datum, its frame, the shadow word and the weight above it), and the
/// floating-point modes the arithmetic runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub eax: u32,
    pub ecx: u32,
    pub esp: u32,
    pub mxcsr: u32,
    pub x87_control: u16,
}

impl Entry {
    fn fpu(&self) -> engine::light_constants::Fpu {
        engine::light_constants::Fpu {
            mxcsr: self.mxcsr,
            x87_control: self.x87_control,
        }
    }
}

/// The light's constants as `0x00031170` sets them for `source`: its call
/// to `0x0004a1e0` (`engine::light_constants`), then its flag code
/// (`engine::light_passes::pass_flags`). Reads guest memory, writes none.
/// `None` when a read faults or the arithmetic would raise an unmasked
/// exception. The per-layer candidate needs only the frame and flags.
/// Whole-pass admission also computes the sphere's six projected words. S: the item lists `0x00040e90` (the passes' first call)
/// leave what `0x0004a1e0` reads alone, but for the casters' layer 10,
/// which `source`'s shadow word carries.
pub fn source_constants(
    mem: &dyn GuestMemory,
    source: &Source,
    fpu: engine::light_constants::Fpu,
) -> Option<LightConstants> {
    use engine::light_constants as lc;
    use engine::light_passes as lp;
    let g = lc::light_globals(mem, &source.call, fpu, &mut |_| None).ok()?;
    let f = lp::pass_flags(mem, &g, source.call.shadow as u8, fpu.mxcsr).ok()?;
    constants_of(mem, &g, f.flags, source.call.definition)
}

/// The constants of the light whose `0x0004a1e0` call has run and whose
/// flags are set: the globals as the call left them
/// (`engine::light_constants::LightGlobals::stored`) and the flags at
/// `[0x004ba014]`. The first-person lights' (`0x000404b0`): their call's
/// light volume is a local of the loop's frame, and the loop sets the
/// flags itself (`light_passes::first_person_steps`), so they are read,
/// not computed. `None` when a read faults.
pub fn stored_light_constants(mem: &dyn GuestMemory) -> Option<LightConstants> {
    let g = engine::light_constants::LightGlobals::stored(mem).ok()?;
    let flags = mem.u32(engine::light_passes::global::PASS_FLAGS).ok()?;
    constants_of(mem, &g, flags, g.definition)
}

/// [`LightConstants`] of the globals `g` with the passes' `flags` and the
/// `ligh` definition at `definition`.
fn constants_of(
    mem: &dyn GuestMemory,
    g: &engine::light_constants::LightGlobals,
    flags: u32,
    definition: u32,
) -> Option<LightConstants> {
    use engine::light_passes as lp;
    let definition_flags = mem.u32(definition).ok()?;
    let gel_shader = mem
        .u32(definition.wrapping_add(lp::DEFINITION_GEL_SHADER))
        .ok()?;
    let gel = lp::gel(
        flags,
        definition_flags,
        gel_shader,
        mem.u32(definition.wrapping_add(lp::DEFINITION_GEL)).ok()?,
    );
    Some(LightConstants {
        frame: g.frame_constants(mem).ok()?,
        flags,
        colour_a: g.colour_a_input(),
        colour_b: g.colour_b_input(),
        mode: g.mode,
        definition_flags,
        definition_8a: mem.u16(definition.wrapping_add(0x8a)).ok()?,
        definition_80: mem.u16(definition.wrapping_add(0x80)).ok()?,
        falloff_0: g.falloff[0],
        gel,
        animated_gel: gel_shader != u32::MAX,
    })
}

/// A light's passes begin: the last light's candidate ends, the scene's
/// light closes until this one is in it, and the light takes its serial.
fn enter_light() -> u64 {
    finish_candidate();
    crate::scene_step::light_entry();
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        host.clear_admission();
    }
    SERIAL.fetch_add(1, Ordering::Relaxed)
}

/// The light whose passes run with the registers `entry`, read from
/// guest memory ([`Source::observed`]): the guest loop's lights at their
/// constants call, and a hand-off's whose lists the guest makes.
fn observe(mem: &dyn GuestMemory, entry: Entry, serial: u64) {
    let Some(source) = Source::observed(mem, &entry) else {
        refused("unreadable light arguments");
        return;
    };
    open_light(mem, &source, entry.fpu(), serial);
}

/// The light of `source` into the scene with its steps, and its layers
/// 12 and 13 that draw become the candidate's.
fn open_light(
    mem: &dyn GuestMemory,
    source: &Source,
    fpu: engine::light_constants::Fpu,
    serial: u64,
) {
    let call = &source.call;
    let Some(mut light) = read_light(
        mem,
        call.light,
        call.shadow & 0xff != 0,
        f32::from_bits(call.weight),
    ) else {
        refused("invalid or unreadable light record");
        return;
    };
    light.constants = source_constants(mem, source, fpu);
    let prepare = || {
        let c = light.constants.ok_or("unreadable light constants")?;
        let inputs = source
            .inputs(mem, c.flags, c.definition_flags)
            .ok_or("unreadable light layer mask")?;
        let steps = light_passes::steps(&inputs);
        crate::scene_step::light(mem, scene_light(&light, &inputs), steps.as_slice(), call);
        let shader_gel = if c.animated_gel {
            if light.definition != source.definition {
                return Err("light definition differs from the constants call");
            }
            let shader = mem
                .u32(
                    call.definition
                        .wrapping_add(engine::light_passes::DEFINITION_GEL_SHADER),
                )
                .map_err(|_| "unreadable gel shader")?;
            let host = HOST
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .ok_or("no light host")?;
            host.checkpoint_gel(serial);
            Some((call.definition, shader))
        } else {
            None
        };
        Ok((steps.drawn() & RESOLVED_LAYERS, shader_gel))
    };
    match prepare() {
        Ok((0, _)) => {}
        Ok((layers, shader_gel)) => CANDIDATE.set(Some(Candidate {
            light,
            serial,
            layers,
            shader_gel,
            inside: None,
            first_person: false,
        })),
        Err(why) => {
            refused(why);
        }
    }
}

/// The scene's record of `light` (its steps set by the builder).
fn scene_light(light: &Light, inputs: &light_passes::Inputs) -> frame_scene::SceneLight {
    frame_scene::SceneLight {
        handle: light.handle,
        definition: light.definition,
        flags: inputs.flags,
        definition_flags: inputs.definition_flags,
        additional_casters: inputs.additional_casters,
        guest_weight: Some(light.weight),
        ..Default::default()
    }
}

/// The first-person light entry whose layers the loop `0x000404b0` draws,
/// read from its frame at a layer draw (`esp` the layer draw's: the
/// return address, then the loop's frame, its light index at `+0x14` and
/// its entry pointer at `+0x18`): the address of the entry's datum
/// (`light_passes::FIRST_PERSON_LIGHT_DATUM`). `None` when they do not
/// read or do not agree with the table.
pub fn first_person_entry(mem: &dyn GuestMemory, esp: u32) -> Option<u32> {
    use light_passes::{
        FIRST_PERSON_LIGHT_COUNT, FIRST_PERSON_LIGHT_DATUM, FIRST_PERSON_LIGHT_SIZE,
        FIRST_PERSON_LIGHTS,
    };
    let index = mem.u32(esp.wrapping_add(4 + 0x14)).ok()?;
    let at = mem.u32(esp.wrapping_add(4 + 0x18)).ok()?;
    let count = mem.u32(FIRST_PERSON_LIGHT_COUNT).ok()? as i32;
    let expected = FIRST_PERSON_LIGHTS
        .checked_add(index.checked_mul(FIRST_PERSON_LIGHT_SIZE)?)?
        .checked_add(FIRST_PERSON_LIGHT_DATUM)?;
    ((index as i32) < count.min(8) && at == expected).then_some(at)
}

/// Before a layer draw returning to `ret`: when it is one of
/// [`FIRST_PERSON_LAYERS`] on the light's list, the first of a light's
/// (another entry, or a layer not after the last one drawn) begins it
/// ([`begin_first_person`]).
fn first_person_layer(mem: &dyn GuestMemory, esp: u32, ret: u32) {
    let Some(order) = FIRST_PERSON_LAYERS.iter().position(|&(_, at)| at == ret) else {
        return;
    };
    let layer = FIRST_PERSON_LAYERS[order].0;
    if mem.u32(skip::CURRENT_LAYER).ok() != Some(layer)
        || mem.u32(skip::CURRENT_LIST).ok() != Some(LIGHT_LIST)
    {
        return;
    }
    let Some(entry) = first_person_entry(mem, esp) else {
        FIRST_PERSON.set(None);
        refused("unreadable first-person light entry");
        return;
    };
    let new = first_person_begins(FIRST_PERSON.get(), entry, order);
    FIRST_PERSON.set(Some((entry, order)));
    if new {
        begin_first_person(mem, entry);
    }
}

/// Whether the layer draw of order `order` (in [`FIRST_PERSON_LAYERS`])
/// for `entry` begins a light after `last`: another entry, or the same
/// entry's layers over again (the next view's, or the loop's next pass).
fn first_person_begins(last: Option<(u32, usize)>, entry: u32, order: usize) -> bool {
    last.is_none_or(|(at, before)| at != entry || order <= before)
}

/// A first-person light's layers begin: its record (weight 1, no shadow,
/// as the loop's call), its constants as the call left them
/// ([`stored_light_constants`]) and its steps
/// (`light_passes::first_person_steps`) go into the first person's view,
/// and its layers 12 and 13 that draw become the candidate's.
fn begin_first_person(mem: &dyn GuestMemory, entry: u32) {
    finish_candidate();
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        host.clear_admission();
    }
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let Ok(handle) = mem.u32(entry) else {
        refused("unreadable first-person light datum");
        return;
    };
    let Some(mut light) = read_light(mem, handle, false, 1.0) else {
        refused("invalid or unreadable first-person light record");
        return;
    };
    light.constants = stored_light_constants(mem);
    let prepare = || {
        let c = light
            .constants
            .ok_or("unreadable first-person light constants")?;
        if c.animated_gel {
            return Err("a first-person light's shader gel draws through its handler");
        }
        let inputs = light_passes::read(&Ro(mem), c.flags, c.definition_flags)
            .ok_or("unreadable light layer mask")?;
        let steps = light_passes::first_person_steps(&inputs);
        crate::scene_step::first_person_light(mem, scene_light(&light, &inputs), steps.as_slice());
        Ok(steps.drawn() & RESOLVED_LAYERS)
    };
    match prepare() {
        Ok(0) => {}
        Ok(layers) => CANDIDATE.set(Some(Candidate {
            light,
            serial,
            layers,
            shader_gel: None,
            inside: None,
            first_person: true,
        })),
        Err(why) => {
            refused(why);
        }
    }
}

/// The layer of [`SPLIT_LAYERS`] a layer draw returning to `ret` runs,
/// when the candidate is to draw it and the current layer and list are
/// that layer and the light's.
fn enter_layer(mem: &dyn GuestMemory, ret: u32) {
    CANDIDATE.with_borrow_mut(|candidate| {
        if let Some(c) = candidate.as_mut().filter(|c| !c.first_person) {
            c.inside = SPLIT_LAYERS
                .iter()
                .find(|&&(layer, at)| at == ret && c.layers & 1 << layer != 0)
                .map(|&(layer, _)| layer)
                .filter(|&layer| {
                    mem.u32(skip::CURRENT_LAYER).ok() == Some(layer)
                        && mem.u32(skip::CURRENT_LIST).ok() == Some(LIGHT_LIST)
                });
        }
    });
}

fn geometry(
    mem: &mut dyn GuestMemory,
    esp: u32,
    esi: u32,
    port_caller: impl FnOnce(u32) -> bool,
) -> Option<()> {
    let (light, serial, shader_gel) = CANDIDATE.with_borrow(|slot| {
        slot.as_ref()
            .filter(|c| c.inside.is_some())
            .map(|c| (c.light, c.serial, c.shader_gel))
    })?;
    let layer = leave_layer()?;
    let caller = mem.u32(esp).ok();
    let caller = match caller {
        Some(pc) if pc == light_list::HANDLER_RETURN || port_caller(pc) => pc,
        _ => {
            refused("outside the sorted light geometry call");
            return None;
        }
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else {
        refused("no light host");
        return None;
    };
    let Some(view_target) = VIEW_TARGET.get() else {
        refused("light view target unavailable");
        return None;
    };
    let accepted = host.submit(
        mem,
        Dispatch {
            light,
            serial,
            layer,
            shader_gel,
            view_target,
            sorted: SortedCall {
                list: LIGHT_LIST,
                layer,
                esp,
                esi,
                caller,
            },
        },
    );
    accepted.then_some(())
}

/// The gathered walk's build of `layer` (12 or 13) of the running light,
/// whose group the scene holds whole: the host draws it from the scene
/// alone. `true` when it did: the layer leaves the candidate, and the
/// walk hands the guest's entry loop no entries. `false`, with the
/// reason counted, leaves the layer to its handlers as before.
pub fn draw_alone(mem: &mut dyn GuestMemory, layer: u32, group: &SceneGroup) -> bool {
    if !matches!(layer, 12 | 13) || Stage::Lights.standing_down() {
        return false;
    }
    let Some((light, serial, shader_gel, first_person)) = CANDIDATE.with_borrow(|slot| {
        slot.as_ref()
            .filter(|c| c.layers & 1 << layer != 0)
            .map(|c| (c.light, c.serial, c.shader_gel, c.first_person))
    }) else {
        return false;
    };
    let Some(view_target) = VIEW_TARGET.get() else {
        return refused("light view target unavailable");
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return refused("no light host");
    };
    if !host.draw_alone(
        mem,
        Alone {
            light,
            serial,
            layer,
            shader_gel,
            view_target,
            group,
        },
    ) {
        return false;
    }
    CANDIDATE.with_borrow_mut(|slot| {
        if let Some(c) = slot.as_mut() {
            c.layers &= !(1 << layer);
            c.inside = None;
        }
    });
    ALONE.fetch_add(1, Ordering::Relaxed);
    if first_person {
        FIRST_PERSON_ALONE.fetch_add(1, Ordering::Relaxed);
    }
    let objects = group
        .entries
        .iter()
        .filter(|e| matches!(e.owner, frame_scene::LayerOwner::Object(_)))
        .count();
    ALONE_OBJECTS.fetch_add(objects as u64, Ordering::Relaxed);
    true
}

static ALONE: AtomicU64 = AtomicU64::new(0);
static ALONE_OBJECTS: AtomicU64 = AtomicU64::new(0);
static FIRST_PERSON_ALONE: AtomicU64 = AtomicU64::new(0);

/// Light layers drawn from the scene alone, no handler run.
pub fn drawn_alone() -> u64 {
    ALONE.load(Ordering::Relaxed)
}

/// Of those layers, the first-person lights'.
pub fn first_person_drawn_alone() -> u64 {
    FIRST_PERSON_ALONE.load(Ordering::Relaxed)
}

/// Object entries of those layers: model parts drawn from the scene.
pub fn objects_drawn_alone() -> u64 {
    ALONE_OBJECTS.load(Ordering::Relaxed)
}

pub struct NativeLights;

impl Feature for NativeLights {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Dynamic lights and their shadows drawn by the native GPU renderer",
        )
        .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Switched off, or not placed with render-timing: no hooks, nothing
        // held. Placed: the pending light (`CANDIDATE`) and the view's
        // target (`VIEW_TARGET`) belong to the render they were read in,
        // so a load drops them ([`forget_runs`]).
        if !on(b.switches()) {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            return Ok(());
        }
        if b.switches().feature(crate::render_timing::ID, false) {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            eprintln!(
                "native-lights: not placed with render-timing on (its light passes' timer needs their end)"
            );
            return Ok(());
        }
        frame::install(b);
        items::install(b);
        passes::install(b);
        additional::install(b);
        first_person::install(b);
        object_shadows::install(b);
        b.raw()
            .observe_before(&points::RENDER_DYNAMIC_LIGHTS, "native_lights_view", |o| {
                if !Stage::Lights.standing_down() {
                    let mem = o.guest();
                    crate::native_world::load_active_bsp(&*mem);
                    slot_visibility::begin(&*mem);
                    new_view();
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_LIGHT_PASSES, "native_lights_begin", |o| {
                if Stage::Lights.standing_down() {
                    return;
                }
                let entry = Entry {
                    eax: o.cpu.eax,
                    ecx: o.cpu.ecx,
                    esp: o.cpu.esp,
                    mxcsr: o.cpu.mxcsr,
                    x87_control: o.cpu.x87.control,
                };
                let mem = o.guest();
                let serial = enter_light();
                // The frame body's light waits for its item lists
                // (`handoff`); the guest loop's waits for its constants call.
                if !handoff::entered(&*mem, entry, serial) {
                    handoff::defer(entry, serial);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_lights_layer", |o| {
                if Stage::Lights.standing_down() {
                    return;
                }
                let mem = o.guest();
                // The passes body's layer draws name the guest's site.
                let ret = engine::render::light_gather::passes::caller(&*mem, o.cpu.esp)
                    .unwrap_or(0);
                enter_layer(&*mem, ret);
                first_person_layer(&*mem, o.cpu.esp, ret);
            })
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_objects::ID)
            .context_only();
        b.raw()
            .before(
                &points::LIGHT_GEOMETRY_DRAW,
                "native_lights_geometry",
                |c| {
                    if Stage::Lights.standing_down() {
                        return Ok(Flow::Continue);
                    }
                    if walk::waiting() {
                        // The gathered walk will verify the actual first emission.
                        // On refusal its entire original sorted suffix still runs.
                        // The light's other layer stays the candidate's.
                        leave_layer();
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let _ = geometry(&mut *rt.memory(), c.cpu.esp, c.cpu.esi, |_| false);
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "native_lights_end", |_| {
                if Stage::Lights.standing_down() {
                    return;
                }
                if leave_layer().is_some() {
                    refused("no eligible sorted geometry dispatch");
                }
                if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    host.clear_admission();
                }
            })
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_runs();
            Ok(())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// The tests that install a host hold this: the host is global.
    static HOST_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    use super::*;
    use guest_layout::FlatMemory;

    const DIFFUSE_RETURN: u32 = 0x0003_144f;
    const SPECULAR_RETURN: u32 = 0x0003_14cf;
    const TARGET_1_RETURN: u32 = 0x0003_13e2;

    fn candidate(layers: u32) -> Candidate {
        let light = Light {
            handle: 1,
            definition: 2,
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [1.0; 3],
            colour_b: [0.0; 3],
            intensity: 1.0,
            bounds: ([0.0; 3], 1.0),
            shadow: true,
            weight: 1.0,
            constants: None,
        };
        Candidate {
            light,
            serial: 1,
            shader_gel: None,
            layers,
            inside: None,
            first_person: false,
        }
    }

    fn drawing(layer: u32, list: u32) -> FlatMemory {
        let mut m = FlatMemory::new(0, 0x0050_0000);
        m.set_u32(skip::CURRENT_LAYER, layer).unwrap();
        m.set_u32(skip::CURRENT_LIST, list).unwrap();
        m
    }

    fn layers() -> Option<(u32, Option<u32>)> {
        CANDIDATE.with_borrow(|c| c.as_ref().map(|c| (c.layers, c.inside)))
    }

    #[test]
    fn indexed_light_is_scoped_to_the_dynamic_layers_after_reservation() {
        CANDIDATE.set(Some(candidate(RESOLVED_LAYERS)));
        VIEW_TARGET.set(Some([10, 20, 0, 0, 0, 0, 0, 0]));
        enter_layer(&drawing(12, LIGHT_LIST), DIFFUSE_RETURN);
        assert_eq!(leave_layer(), Some(12));
        let light = indexed_light(12, DIFFUSE_RETURN, LIGHT_LIST).unwrap();
        assert_eq!(light.targets, [10, 20]);
        assert_eq!(light.serial, 1);
        for (layer, caller, list) in [
            (13, DIFFUSE_RETURN, LIGHT_LIST),
            (14, TARGET_1_RETURN, LIGHT_LIST),
            (12, DIFFUSE_RETURN, 0),
            (12, FIRST_PERSON_LAYERS[2].1, LIGHT_LIST),
        ] {
            assert!(indexed_light(layer, caller, list).is_none());
        }
        CANDIDATE.set(Some(Candidate { first_person: true, ..candidate(RESOLVED_LAYERS) }));
        assert!(indexed_light(12, DIFFUSE_RETURN, LIGHT_LIST).is_none());
        CANDIDATE.take();
        VIEW_TARGET.take();
        assert!(indexed_light(12, DIFFUSE_RETURN, LIGHT_LIST).is_none());
    }

    #[test]
    fn a_light_drawing_both_sorted_layers_is_the_candidate_of_each_in_turn() {
        CANDIDATE.set(Some(candidate(RESOLVED_LAYERS)));
        // Layer 14 runs first; the resolved path does not take it.
        enter_layer(&drawing(14, LIGHT_LIST), TARGET_1_RETURN);
        assert_eq!(layers(), Some((RESOLVED_LAYERS, None)));
        enter_layer(&drawing(12, LIGHT_LIST), DIFFUSE_RETURN);
        assert_eq!(layers(), Some((RESOLVED_LAYERS, Some(12))));
        assert_eq!(
            leave_layer(),
            Some(12),
            "the first geometry call takes the layer"
        );
        assert_eq!(leave_layer(), None, "and only the first");
        assert_eq!(
            layers(),
            Some((1 << 13, None)),
            "the specular layer is still to come"
        );
        enter_layer(&drawing(13, LIGHT_LIST), SPECULAR_RETURN);
        assert_eq!(leave_layer(), Some(13));
        assert_eq!(layers(), Some((0, None)));
        finish_candidate();
        assert!(layers().is_none());
    }

    #[test]
    fn a_layer_is_entered_only_at_its_draw_on_the_lights_list() {
        CANDIDATE.set(Some(candidate(1 << 12)));
        enter_layer(&drawing(13, LIGHT_LIST), SPECULAR_RETURN);
        assert_eq!(
            layers(),
            Some((1 << 12, None)),
            "not a layer the light draws"
        );
        enter_layer(&drawing(12, LIGHT_LIST + 0x18), DIFFUSE_RETURN);
        assert_eq!(layers(), Some((1 << 12, None)), "another list");
        enter_layer(&drawing(13, LIGHT_LIST), DIFFUSE_RETURN);
        assert_eq!(layers(), Some((1 << 12, None)), "another current layer");
        enter_layer(&drawing(12, LIGHT_LIST), DIFFUSE_RETURN);
        assert_eq!(layers(), Some((1 << 12, Some(12))));
        // Another stage (the gathered walk) owns the layer: it leaves
        // the candidate without a draw.
        assert_eq!(leave_layer(), Some(12));
        assert_eq!(layers(), Some((0, None)));
        CANDIDATE.take();
    }

    struct Stub(bool);

    impl Host for Stub {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn draw_alone(&self, _: &mut dyn GuestMemory, alone: Alone<'_>) -> bool {
            self.0 && alone.group.entries.is_empty()
        }
    }

    #[test]
    fn a_whole_scene_group_is_drawn_alone_only_for_a_layer_the_light_still_draws() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let group = SceneGroup {
            group: frame_scene::LightGroup {
                light: 0,
                layer: 12,
                site: frame_scene::GroupSite::Light,
                projection: None,
                pass: None,
                entries: 0..0,
                remainder: 0,
            },
            entries: Vec::new(),
            rows: Vec::new(),
        };
        let mut m = drawing(12, LIGHT_LIST);
        assert!(!draw_alone(&mut m, 12, &group), "no light running");
        CANDIDATE.set(Some(candidate(RESOLVED_LAYERS)));
        VIEW_TARGET.set(Some([1; 8]));
        set_host(Arc::new(Stub(false)));
        assert!(
            !draw_alone(&mut m, 12, &group),
            "the host keeps it on its handlers"
        );
        assert_eq!(layers(), Some((RESOLVED_LAYERS, None)));
        set_host(Arc::new(Stub(true)));
        assert!(
            !draw_alone(&mut m, 14, &group),
            "not a layer drawn from the scene"
        );
        enter_layer(&m, 0x0003_144f);
        assert!(draw_alone(&mut m, 12, &group));
        assert_eq!(
            layers(),
            Some((1 << 13, None)),
            "the layer leaves the candidate, its specular stays"
        );
        assert!(!draw_alone(&mut m, 12, &group), "drawn once");
        CANDIDATE.take();
        VIEW_TARGET.set(None);
        *HOST.lock().unwrap() = None;
    }

    #[test]
    fn the_first_person_entry_is_read_from_the_loops_frame() {
        use light_passes::{
            FIRST_PERSON_LIGHT_COUNT, FIRST_PERSON_LIGHT_DATUM, FIRST_PERSON_LIGHT_SIZE,
            FIRST_PERSON_LIGHTS,
        };
        const ESP: u32 = 0x1000;
        let second = FIRST_PERSON_LIGHTS + FIRST_PERSON_LIGHT_SIZE + FIRST_PERSON_LIGHT_DATUM;
        let mut m = FlatMemory::new(0, 0x0060_0000);
        m.set_u32(FIRST_PERSON_LIGHT_COUNT, 2).unwrap();
        m.set_u32(ESP + 4 + 0x14, 1).unwrap();
        m.set_u32(ESP + 4 + 0x18, second).unwrap();
        assert_eq!(first_person_entry(&m, ESP), Some(second));
        m.set_u32(ESP + 4 + 0x18, second + 4).unwrap();
        assert_eq!(first_person_entry(&m, ESP), None, "not the index's entry");
        m.set_u32(ESP + 4 + 0x18, second).unwrap();
        m.set_u32(FIRST_PERSON_LIGHT_COUNT, 1).unwrap();
        assert_eq!(first_person_entry(&m, ESP), None, "past the count");
    }

    #[test]
    fn a_first_person_light_begins_at_its_first_layer_draw_only() {
        const A: u32 = 0x004c_6b00;
        const B: u32 = A + 0x408;
        assert!(
            first_person_begins(None, A, 2),
            "the first draw, whichever layer"
        );
        assert!(
            !first_person_begins(Some((A, 0)), A, 2),
            "its layer 12 after its 11"
        );
        assert!(!first_person_begins(Some((A, 2)), A, 3));
        assert!(first_person_begins(Some((A, 3)), B, 0), "the next entry");
        assert!(
            first_person_begins(Some((A, 2)), A, 2),
            "the same entry drawn over again"
        );
        assert!(first_person_site(12, 0x0004_0718) && first_person_site(13, 0x0004_079c));
        assert!(
            !first_person_site(12, 0x0004_079c),
            "13's return names only 13"
        );
        assert!(!SPLIT_LAYERS.iter().any(|&(l, at)| first_person_site(l, at)));
    }

    #[test]
    fn a_first_person_candidate_is_not_the_resolved_draw_paths() {
        CANDIDATE.set(Some(Candidate {
            first_person: true,
            ..candidate(RESOLVED_LAYERS)
        }));
        enter_layer(&drawing(12, LIGHT_LIST), 0x0003_144f);
        assert_eq!(
            layers(),
            Some((RESOLVED_LAYERS, None)),
            "a dynamic light's layer draw does not enter a first-person light's layer"
        );
        enter_layer(&drawing(12, LIGHT_LIST), 0x0004_0718);
        assert_eq!(layers(), Some((RESOLVED_LAYERS, None)), "nor its own");
        CANDIDATE.take();
    }

    /// Records the slots of the layers it is asked to draw.
    struct Records(Mutex<Vec<u8>>);

    impl Host for Records {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn draws_scene(&self) -> bool {
            true
        }
        fn draw_scene(&self, _: &mut dyn GuestMemory, draw: SceneDraw<'_>) -> usize {
            let mut slots = self.0.lock().unwrap();
            slots.extend(draw.passes.iter().filter_map(|p| p.slot.number()));
            draw.passes.len()
        }
    }

    #[test]
    fn a_scene_with_one_dynamic_light_records_the_slot_4_pass() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        use frame_scene::{GroupSite, SceneBuilder, SceneId, SceneLight, Stamp, ViewKey, ViewKind};
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey { kind: ViewKind::Game, player: 0, view: 0 },
            frame_scene::RenderCamera {
                position: [0.0; 3],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov: 1.0,
                projection_xy: None,
                viewport: [0, 0, 480, 640],
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        b.push_light(SceneLight { handle: 1, definition: 2, flags: 7, ..Default::default() }, &[]).unwrap();
        b.push_light_group(
            frame_scene::LightGroup {
                light: 0,
                layer: 12,
                site: GroupSite::Light,
                projection: None,
                pass: None,
                entries: 0..0,
                remainder: 0,
            },
            &[],
        )
        .unwrap();
        let scene = b.seal(Stamp::default()).unwrap();
        let passes = gather_scene_passes(&scene, &scene.views[0]);
        assert_eq!(passes.len(), 1);
        let host = Arc::new(Records(Mutex::new(Vec::new())));
        set_host(host.clone());
        VIEW_TARGET.set(Some([1; 8]));
        let mut m = FlatMemory::new(0, 0x1000);
        assert_eq!(draw_scene(&mut m, &passes), 1);
        assert_eq!(*host.0.lock().unwrap(), [4], "the light's layer 12 in slot 4");
    }

    /// Draws a cut light now when `accept`, counting what it is handed.
    struct Cut {
        accept: bool,
        handed: Mutex<usize>,
    }

    impl Host for Cut {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn draw_scene_light(&self, _: &mut dyn GuestMemory, draw: SceneDraw<'_>) -> bool {
            *self.handed.lock().unwrap() += draw.passes.len();
            self.accept
        }
    }

    /// Holds the lights its `tiled` says, and logs each call in order.
    struct Runs {
        tiled: fn(u32) -> bool,
        log: Mutex<Vec<String>>,
        held: Mutex<usize>,
    }

    impl Host for Runs {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn hold_scene_light(&self, _: &mut dyn GuestMemory, draw: &SceneDraw<'_>) -> bool {
            let handle = draw.passes[0].light.handle;
            if !(self.tiled)(handle) {
                return false;
            }
            self.log.lock().unwrap().push(format!("hold {handle}"));
            *self.held.lock().unwrap() += 1;
            true
        }
        fn flush_light_run(&self, _: &mut dyn GuestMemory) -> usize {
            let n = std::mem::take(&mut *self.held.lock().unwrap());
            if n > 0 {
                self.log.lock().unwrap().push(format!("run of {n}"));
            }
            n
        }
        fn draw_scene_light(&self, _: &mut dyn GuestMemory, draw: SceneDraw<'_>) -> bool {
            self.log.lock().unwrap().push(format!("draw {}", draw.passes[0].light.handle));
            true
        }
    }

    /// A held run draws before any light drawn on its own, and at the
    /// view's end; first-person lights are never held.
    #[test]
    fn a_held_run_draws_before_a_light_drawn_per_light_and_at_the_end() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let pass = |handle, slot| ScenePass {
            slot,
            light: frame_scene::Light { handle, ..Default::default() },
            group: SceneGroup {
                group: frame_scene::LightGroup {
                    light: 0,
                    layer: 12,
                    site: frame_scene::GroupSite::Light,
                    projection: None,
                    pass: None,
                    entries: 0..0,
                    remainder: 0,
                },
                entries: Vec::new(),
                rows: Vec::new(),
            },
        };
        let host = Arc::new(Runs { tiled: |h| h % 2 == 0, log: Mutex::new(Vec::new()), held: Mutex::new(0) });
        set_host(host.clone());
        let mut m = FlatMemory::new(0, 0x1000);
        for (handle, slot) in [(2, LIGHT_SLOT), (4, LIGHT_SLOT), (5, LIGHT_SLOT), (6, LIGHT_SLOT), (8, FIRST_PERSON_SLOT)] {
            assert!(draw_cut_light(&mut m, &[pass(handle, slot)], Some([1; 8])));
        }
        flush_light_run(&mut m);
        assert_eq!(
            *host.log.lock().unwrap(),
            ["hold 2", "hold 4", "run of 2", "draw 5", "hold 6", "run of 1", "draw 8"],
        );
        *HOST.lock().unwrap() = None;
    }

    /// A cut light draws at its passes or not at all: a host that refuses
    /// it, no host and no target all answer `false`, and the caller panics
    /// (`lights: host`); a light with no drawn layer needs no host.
    #[test]
    fn a_cut_light_the_host_refuses_is_not_drawn() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let pass = ScenePass {
            slot: LIGHT_SLOT,
            light: frame_scene::Light::default(),
            group: SceneGroup {
                group: frame_scene::LightGroup {
                    light: 0,
                    layer: 12,
                    site: frame_scene::GroupSite::Light,
                    projection: None,
                    pass: None,
                    entries: 0..0,
                    remainder: 0,
                },
                entries: Vec::new(),
                rows: Vec::new(),
            },
        };
        let mut m = FlatMemory::new(0, 0x1000);
        *HOST.lock().unwrap() = None;
        assert!(!draw_cut_light(&mut m, std::slice::from_ref(&pass), Some([1; 8])), "no host");
        assert!(draw_cut_light(&mut m, &[], None), "nothing to draw");
        for accept in [false, true] {
            let host = Arc::new(Cut { accept, handed: Mutex::new(0) });
            set_host(host.clone());
            assert!(!draw_cut_light(&mut m, std::slice::from_ref(&pass), None), "no target");
            assert_eq!(*host.handed.lock().unwrap(), 0);
            assert_eq!(draw_cut_light(&mut m, std::slice::from_ref(&pass), Some([1; 8])), accept);
            assert_eq!(*host.handed.lock().unwrap(), 1);
        }
        *HOST.lock().unwrap() = None;
    }

    /// The layers a host is handed, with their slots.
    struct Layers(Mutex<Vec<(u8, u8)>>);

    impl Host for Layers {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn draws_scene(&self) -> bool {
            true
        }
        fn draw_scene(&self, _: &mut dyn GuestMemory, draw: SceneDraw<'_>) -> usize {
            let mut seen = self.0.lock().unwrap();
            seen.extend(draw.passes.iter().filter_map(|p| Some((p.slot.number()?, p.group.group.layer))));
            draw.passes.len()
        }
    }

    /// A cut light's groups, put into the scene in any order, reach the
    /// host as one pass per layer in slot 4 in the passes' order 10, 11,
    /// 14, 12, 13.
    #[test]
    fn each_layer_of_a_cut_light_is_one_recorded_pass_in_order() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        use frame_scene::{GroupSite, SceneBuilder, SceneId, SceneLight, Stamp, ViewKey, ViewKind};
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey { kind: ViewKind::Game, player: 0, view: 0 },
            frame_scene::RenderCamera {
                position: [0.0; 3],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov: 1.0,
                projection_xy: None,
                viewport: [0, 0, 480, 640],
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        b.push_light(SceneLight { handle: 1, definition: 2, flags: 7, ..Default::default() }, &[]).unwrap();
        for layer in [12, 10, 13, 11, 14] {
            b.push_light_group(
                frame_scene::LightGroup {
                    light: 0,
                    layer,
                    site: GroupSite::Light,
                    projection: matches!(layer, 10 | 11).then_some([[0; 4]; frame_scene::PROJECTION_ROWS]),
                    pass: None,
                    entries: 0..0,
                    remainder: 0,
                },
                &[],
            )
            .unwrap();
        }
        let scene = b.seal(Stamp::default()).unwrap();
        let passes = gather_scene_passes(&scene, &scene.views[0]);
        let host = Arc::new(Layers(Mutex::new(Vec::new())));
        set_host(host.clone());
        VIEW_TARGET.set(Some([1; 8]));
        let mut m = FlatMemory::new(0, 0x1000);
        assert_eq!(draw_scene(&mut m, &passes), 5);
        assert_eq!(*host.0.lock().unwrap(), [(4, 10), (4, 11), (4, 14), (4, 12), (4, 13)]);
    }

    #[test]
    fn the_light_frame_is_cut_only_while_the_scene_path_runs() {
        let _host = HOST_TEST.lock().unwrap_or_else(|e| e.into_inner());
        // One implementation at the point: the frame body answers the cut.
        assert!(crate::cut::OWN_PRODUCER.contains(&"dynamic_lights"));
        assert_eq!(crate::cut::cut("dynamic_lights").map(|c| c.entry), Some(0x0003_1c80));
        set_host(Arc::new(Stub(false)));
        assert!(!scene_path_ready(), "a host that does not draw the scene leaves the frame to the body");
        set_host(Arc::new(Records(Mutex::new(Vec::new()))));
        assert!(scene_path_ready());
    }
}
