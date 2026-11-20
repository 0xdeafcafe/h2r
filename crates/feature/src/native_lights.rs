//! Dynamic light geometry. The gathered walk can build and sort entries
//! before the original geometry handlers. The resolved-draw path below
//! admits individual packets when those handlers execute. On with the
//! native renderer (`impl.render.native`, on by default).
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
use native_render::lights::{Light, LightConstants};
use native_render::skip;

use crate::native_map_work::Stage;
use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-lights";

/// Whether the native lights are on.
pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_render_on(switches)
}

/// The light's list, which `RENDER_LIGHT_PASSES` makes current for its
/// layers (`0x000312c4`; a list as `native_render::skip` reads it: its
/// count at `+4`, the light's first item at `+0xc`, the items at `+0x14`).
pub const LIGHT_LIST: u32 = 0x004c_1a60;

/// `RENDER_LIGHT_PASSES`' draws of the layers a light can take natively,
/// by the layer draw's return address: 14 (`0x000313dd`), 12
/// (`0x0003144a`) and 13 (`0x000314ca`). Layers 10 and 11 (shadowed
/// lights only) and the first-person lights' draws on the same list
/// (`0x000404b0`) are not taken.
pub const SPLIT_LAYERS: [(u32, u32); 3] = [(14, 0x0003_13e2), (12, 0x0003_144f), (13, 0x0003_14cf)];

pub struct Dispatch<'a> {
    pub light: Light,
    pub serial: u64,
    pub shader_gel: Option<(u32, u32)>,
    pub view_target: [u32; 8],
    pub sorted: &'a SortedLayer,
}

pub trait Host: Send + Sync {
    /// Plan a sorted prefix. The handlers still emit their D3D packets so
    /// each native draw can fall back in the same queue slot.
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>) -> bool;
    fn clear_admission(&self);
    /// Before the guest constants or gel quad for this invocation.
    fn checkpoint_gel(&self, serial: u64);
    fn target(&self) -> Option<[u32; 8]>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Candidate {
    light: Light,
    serial: u64,
    shader_gel: Option<(u32, u32)>,
    layer: u32,
    inside_layer: bool,
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

fn finish_candidate() {
    if CANDIDATE.take().is_some() {
        refused("no eligible sorted geometry dispatch");
    }
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
/// `ligh` datum, ECX the view context, ESP (the return address; the light
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
        let or = mem.u32(LIGHT_LIST + 8).map_err(|_| "unreadable light layer mask")?;
        light_list::single_layer(or, c.flags, c.definition_flags).map(|layer| (layer, shader_gel))
    };
    match prepare() {
        Ok((layer, shader_gel)) => CANDIDATE.set(Some(Candidate { light, serial, layer, shader_gel, inside_layer: false })),
        Err(why) => { refused(why); }
    }
}

fn enter_layer(mem: &dyn GuestMemory, ret: u32) {
    CANDIDATE.with_borrow_mut(|candidate| {
        if let Some(c) = candidate {
            c.inside_layer = SPLIT_LAYERS.contains(&(c.layer, ret))
                && mem.u32(skip::CURRENT_LAYER).ok() == Some(c.layer)
                && mem.u32(skip::CURRENT_LIST).ok() == Some(LIGHT_LIST);
        }
    });
}

fn geometry(mem: &mut dyn GuestMemory, esp: u32, esi: u32, port_caller: impl FnOnce(u32) -> bool) -> Option<()> {
    let c = CANDIDATE.with_borrow_mut(|slot| slot.take_if(|c| c.inside_layer))?;
    let caller = mem.u32(esp).ok();
    let caller = match caller {
        Some(pc) if pc == light_list::HANDLER_RETURN || port_caller(pc) => pc,
        _ => { refused("outside the sorted light geometry call"); return None; }
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { refused("no light host"); return None; };
    let Some(view_target) = VIEW_TARGET.get() else { refused("light view target unavailable"); return None; };
    let accepted = match light_list::sorted_at(&Ro(mem), LIGHT_LIST, c.layer, esp, esi, caller) {
        Ok(sorted) => host.submit(mem, Dispatch { light: c.light, serial: c.serial, shader_gel: c.shader_gel, view_target,
            sorted: &sorted }),
        Err(why) => refused(why),
    };
    accepted.then_some(())
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
            b.snapshot(crate::SnapshotPolicy::Stateless);
            return Ok(());
        }
        if b.switches().feature(crate::render_timing::ID, false) {
            b.snapshot(crate::SnapshotPolicy::Stateless);
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
                    CANDIDATE.take();
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
                let inside = CANDIDATE.with_borrow(|c| c.as_ref().is_some_and(|c| c.inside_layer));
                if inside { finish_candidate(); }
                if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    host.clear_admission();
                }
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::section(Vec::new, |_, _| {
            forget_runs();
            Ok(())
        }));
        Ok(())
    }
}
