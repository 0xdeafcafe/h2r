//! Dynamic light geometry. The gathered walk can build and sort entries
//! before the original geometry handlers. The resolved-draw path below
//! admits individual packets when those handlers execute, for each of a
//! light's layers 12 and 13 its passes draw. At the passes' entry the
//! light and its steps (`scene_extract::layout::light_passes`) go into
//! the frame step's open view. On with the native renderer
//! (`impl.render.native`, on by default).
pub mod walk;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::handle::DatumHandle;
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light;
use native_render::camera::GuestRead;
use native_render::light_list::{self, SortedLayer};
use native_render::light_passes;
use native_render::lights::{Light, LightConstants};
use native_render::skip;

use crate::native_map_work::Stage;
use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-lights";

/// Whether the native lights are on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
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
/// lights' draws on the same list (`0x000404b0`) are not taken.
pub const SPLIT_LAYERS: [(u32, u32); 3] = [(14, 0x0003_13e2), (12, 0x0003_144f), (13, 0x0003_14cf)];

/// The layers the resolved-draw path admits: diffuse and specular.
pub const RESOLVED_LAYERS: u32 = 1 << 12 | 1 << 13;

pub struct Dispatch<'a> {
    pub light: Light,
    pub serial: u64,
    /// The layer being drawn, 12 or 13.
    pub layer: u32,
    pub shader_gel: Option<(u32, u32)>,
    pub view_target: [u32; 8],
    pub sorted: &'a SortedLayer,
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
        self.entries.iter().position(|e| std::ptr::eq(e, entry)).and_then(|n| self.rows.get(n)).map_or(&[], Vec::as_slice)
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

pub trait Host: Send + Sync {
    /// Plan a sorted prefix of one layer. The handlers still emit their
    /// D3D packets so each native draw can fall back in the same queue
    /// slot. A light asks once per layer it draws.
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>) -> bool;
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
}
thread_local! {
    static CANDIDATE: RefCell<Option<Candidate>> = const { RefCell::new(None) };
    static VIEW_TARGET: Cell<Option<[u32; 8]>> = const { Cell::new(None) };
}

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
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

pub fn new_view() {
    finish_candidate();
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
    CANDIDATE.take();
    VIEW_TARGET.set(None);
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

/// The light's constants as `0x00031170` would set them: the call it makes
/// to `0x0004a1e0` (`engine::light_constants`, with the definition from
/// the tag table and the shadow word kept under scene flag bit 10), then
/// its flag code (`engine::light_passes::pass_flags`). Reads guest memory,
/// writes none. `None` when a read faults or the arithmetic would raise an
/// unmasked exception. The sphere's screen bounds (`0x001650a0`) are not
/// asked: they set the scissor in the original body, before our geometry dispatch. S: `0x00040e90` (called with EAX 0 before `0x0004a1e0`) is
/// taken to leave what `0x0004a1e0` reads alone.
pub fn light_constants(mem: &dyn GuestMemory, e: &Entry) -> Option<LightConstants> {
    use engine::light_constants as lc;
    use engine::light_passes as lp;
    let arg = |offset: u32| mem.u32(e.esp.wrapping_add(offset)).ok();
    let table = mem.u32(engine::tags::TAG_ENTRIES_GLOBAL).ok()?;
    let definition = mem
        .u32(table.wrapping_add((e.eax & 0xffff) << 4).wrapping_add(8))
        .ok()?;
    let scene = mem.u32(lp::global::SCENE_FLAGS).ok()?;
    let shadow = lp::shadow_argument(arg(0xc)?, scene);
    let call = lc::Call {
        frame: arg(8)?,
        context: e.ecx,
        light: arg(4)?,
        definition,
        shadow,
        weight: arg(0x10)?,
    };
    let fpu = lc::Fpu {
        mxcsr: e.mxcsr,
        x87_control: e.x87_control,
    };
    let g = lc::light_globals(mem, &call, fpu, &mut |_| None).ok()?;
    let f = lp::pass_flags(mem, &g, shadow as u8, e.mxcsr).ok()?;
    let definition_flags = mem.u32(definition).ok()?;
    let gel_shader = mem.u32(definition.wrapping_add(lp::DEFINITION_GEL_SHADER)).ok()?;
    let gel = lp::gel(
        f.flags,
        definition_flags,
        gel_shader,
        mem.u32(definition.wrapping_add(lp::DEFINITION_GEL)).ok()?,
    );
    Some(LightConstants {
        frame: g.frame_constants(mem).ok()?,
        flags: f.flags,
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

fn begin_light(mem: &dyn GuestMemory, entry: Entry) {
    finish_candidate();
    crate::scene_step::light_entry();
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        host.clear_admission();
    }
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let esp = entry.esp;
    let (Ok(handle), Ok(shadow), Ok(weight)) = (mem.u32(esp.wrapping_add(4)),
        mem.u8(esp.wrapping_add(12)), mem.u32(esp.wrapping_add(16))) else {
        refused("unreadable light arguments");
        return;
    };
    let Some(mut light) = read_light(mem, handle, shadow != 0, f32::from_bits(weight)) else {
        refused("invalid or unreadable light record");
        return;
    };
    light.constants = light_constants(mem, &entry);
    let prepare = || {
        let c = light.constants.ok_or("unreadable light constants")?;
        let inputs = light_passes::read(&Ro(mem), c.flags, c.definition_flags).ok_or("unreadable light layer mask")?;
        let steps = light_passes::steps(&inputs);
        crate::scene_step::light(mem, scene_light(&light, &inputs), steps.as_slice());
        let shader_gel = if c.animated_gel {
            if light.definition != entry.eax { return Err("light definition differs from the constants call"); }
            let table = mem.u32(engine::tags::TAG_ENTRIES_GLOBAL).map_err(|_| "unreadable light tag table")?;
            let ptr = mem.u32(table.wrapping_add((entry.eax & 0xffff) << 4).wrapping_add(8))
                .map_err(|_| "unreadable gel definition")?;
            let shader = mem.u32(ptr.wrapping_add(engine::light_passes::DEFINITION_GEL_SHADER))
                .map_err(|_| "unreadable gel shader")?;
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("no light host")?;
            host.checkpoint_gel(serial);
            Some((ptr, shader))
        } else { None };
        Ok((steps.drawn() & RESOLVED_LAYERS, shader_gel))
    };
    match prepare() {
        Ok((0, _)) => {}
        Ok((layers, shader_gel)) => CANDIDATE.set(Some(Candidate { light, serial, layers, shader_gel, inside: None })),
        Err(why) => { refused(why); }
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

/// The layer of [`SPLIT_LAYERS`] a layer draw returning to `ret` runs,
/// when the candidate is to draw it and the current layer and list are
/// that layer and the light's.
fn enter_layer(mem: &dyn GuestMemory, ret: u32) {
    CANDIDATE.with_borrow_mut(|candidate| {
        if let Some(c) = candidate {
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

fn geometry(mem: &mut dyn GuestMemory, esp: u32, esi: u32, port_caller: impl FnOnce(u32) -> bool) -> Option<()> {
    let (light, serial, shader_gel) = CANDIDATE.with_borrow(|slot| {
        slot.as_ref().filter(|c| c.inside.is_some()).map(|c| (c.light, c.serial, c.shader_gel))
    })?;
    let layer = leave_layer()?;
    let caller = mem.u32(esp).ok();
    let caller = match caller {
        Some(pc) if pc == light_list::HANDLER_RETURN || port_caller(pc) => pc,
        _ => { refused("outside the sorted light geometry call"); return None; }
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { refused("no light host"); return None; };
    let Some(view_target) = VIEW_TARGET.get() else { refused("light view target unavailable"); return None; };
    let accepted = match light_list::sorted_at(&Ro(mem), LIGHT_LIST, layer, esp, esi, caller) {
        Ok(sorted) => host.submit(mem, Dispatch { light, serial, layer, shader_gel, view_target, sorted: &sorted }),
        Err(why) => refused(why),
    };
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
    let Some((light, serial, shader_gel)) = CANDIDATE.with_borrow(|slot| {
        slot.as_ref().filter(|c| c.layers & 1 << layer != 0).map(|c| (c.light, c.serial, c.shader_gel))
    }) else {
        return false;
    };
    let Some(view_target) = VIEW_TARGET.get() else { return refused("light view target unavailable") };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return refused("no light host") };
    if !host.draw_alone(mem, Alone { light, serial, layer, shader_gel, view_target, group }) {
        return false;
    }
    CANDIDATE.with_borrow_mut(|slot| {
        if let Some(c) = slot.as_mut() {
            c.layers &= !(1 << layer);
            c.inside = None;
        }
    });
    ALONE.fetch_add(1, Ordering::Relaxed);
    let objects = group.entries.iter().filter(|e| matches!(e.owner, frame_scene::LayerOwner::Object(_))).count();
    ALONE_OBJECTS.fetch_add(objects as u64, Ordering::Relaxed);
    true
}

static ALONE: AtomicU64 = AtomicU64::new(0);
static ALONE_OBJECTS: AtomicU64 = AtomicU64::new(0);

/// Light layers drawn from the scene alone, no handler run.
pub fn drawn_alone() -> u64 {
    ALONE.load(Ordering::Relaxed)
}

/// Object entries of those layers: model parts drawn from the scene.
pub fn objects_drawn_alone() -> u64 {
    ALONE_OBJECTS.load(Ordering::Relaxed)
}

pub struct NativeLights;

impl Feature for NativeLights {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Dynamic lights and their shadows drawn by the native GPU renderer")
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
            eprintln!("native-lights: not placed with render-timing on (its light passes' timer needs their end)");
            return Ok(());
        }
        b.raw()
            .observe_before(&points::RENDER_DYNAMIC_LIGHTS, "native_lights_view", |_| {
                if !Stage::Lights.standing_down() {
                    new_view();
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_LIGHT_PASSES, "native_lights_begin", |o| {
                if Stage::Lights.standing_down() {
                    return;
                }
                begin_light(&*o.guest(), Entry { eax: o.cpu.eax, ecx: o.cpu.ecx, esp: o.cpu.esp,
                    mxcsr: o.cpu.mxcsr, x87_control: o.cpu.x87.control });
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_lights_layer", |o| {
                if Stage::Lights.standing_down() {
                    return;
                }
                let mem = o.guest();
                enter_layer(&*mem, mem.u32(o.cpu.esp).unwrap_or(0));
            })
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_objects::ID)
            .context_only();
        b.raw()
            .before(&points::LIGHT_GEOMETRY_DRAW, "native_lights_geometry", |c| {
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
            })
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
    use super::*;
    use engine::FlatMemory;

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
        Candidate { light, serial: 1, shader_gel: None, layers, inside: None }
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
    fn a_light_drawing_both_sorted_layers_is_the_candidate_of_each_in_turn() {
        CANDIDATE.set(Some(candidate(RESOLVED_LAYERS)));
        // Layer 14 runs first; the resolved path does not take it.
        enter_layer(&drawing(14, LIGHT_LIST), TARGET_1_RETURN);
        assert_eq!(layers(), Some((RESOLVED_LAYERS, None)));
        enter_layer(&drawing(12, LIGHT_LIST), DIFFUSE_RETURN);
        assert_eq!(layers(), Some((RESOLVED_LAYERS, Some(12))));
        assert_eq!(leave_layer(), Some(12), "the first geometry call takes the layer");
        assert_eq!(leave_layer(), None, "and only the first");
        assert_eq!(layers(), Some((1 << 13, None)), "the specular layer is still to come");
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
        assert_eq!(layers(), Some((1 << 12, None)), "not a layer the light draws");
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
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch<'_>) -> bool { false }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> { None }
        fn draw_alone(&self, _: &mut dyn GuestMemory, alone: Alone<'_>) -> bool {
            self.0 && alone.group.entries.is_empty()
        }
    }

    #[test]
    fn a_whole_scene_group_is_drawn_alone_only_for_a_layer_the_light_still_draws() {
        let group = SceneGroup {
            group: frame_scene::LightGroup {
                light: 0, layer: 12, site: frame_scene::GroupSite::Light, projection: None, entries: 0..0, remainder: 0,
            },
            entries: Vec::new(),
            rows: Vec::new(),
        };
        let mut m = drawing(12, LIGHT_LIST);
        assert!(!draw_alone(&mut m, 12, &group), "no light running");
        CANDIDATE.set(Some(candidate(RESOLVED_LAYERS)));
        VIEW_TARGET.set(Some([1; 8]));
        set_host(Arc::new(Stub(false)));
        assert!(!draw_alone(&mut m, 12, &group), "the host keeps it on its handlers");
        assert_eq!(layers(), Some((RESOLVED_LAYERS, None)));
        set_host(Arc::new(Stub(true)));
        assert!(!draw_alone(&mut m, 14, &group), "not a layer drawn from the scene");
        enter_layer(&m, 0x0003_144f);
        assert!(draw_alone(&mut m, 12, &group));
        assert_eq!(layers(), Some((1 << 13, None)), "the layer leaves the candidate, its specular stays");
        assert!(!draw_alone(&mut m, 12, &group), "drawn once");
        CANDIDATE.take();
        VIEW_TARGET.set(None);
        *HOST.lock().unwrap() = None;
    }
}
