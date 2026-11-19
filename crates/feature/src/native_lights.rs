//! Replace geometry dispatches inside a dynamic light. The guest gathers,
//! builds, sorts and applies its per-light state before admission. Its
//! layer teardown and the light's tail run along both paths.
mod reference;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::handle::DatumHandle;
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light;
use native_render::camera::GuestRead;
use native_render::light_list::{self, SortedLayer};
use native_render::lights::{Light, LightConstants};
use native_render::skip;

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-lights";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_lights";

/// Whether the native lights are on (off by default).
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// A light the host cannot draw whole
/// (it meets a world surface left to D3D, or a live object) draws its
/// native world surfaces natively and keeps its D3D passes for the rest.
/// Off by default; looked up exactly; only with the native lights on.
pub const SPLIT_SWITCH: &str = "impl.render.native_lights_split";

/// Whether [`SPLIT_SWITCH`] is on (only with the native lights on).
pub fn split_on(switches: &crate::Switches) -> bool {
    on(switches) && switches.feature(SPLIT_SWITCH, false)
}

/// The light's list, which `RENDER_LIGHT_PASSES` makes current for its
/// layers (`0x000312c4`; a list as `native_render::skip` reads it: its
/// count at `+4`, the light's first item at `+0xc`, the items at `+0x14`).
pub const LIGHT_LIST: u32 = 0x004c_1a60;

/// `RENDER_LIGHT_PASSES`' draws of the layers a split light takes from
/// its native surfaces, by the layer draw's return address: 14
/// (`0x000313dd`), 12 (`0x0003144a`) and 13 (`0x000314ca`). Layers 10 and
/// 11 (shadowed lights only) and the first-person lights' draws on the
/// same list (`0x000404b0`) are not split.
pub const SPLIT_LAYERS: [(u32, u32); 3] = [(14, 0x0003_13e2), (12, 0x0003_144f), (13, 0x0003_14cf)];

pub struct Dispatch<'a> {
    pub light: Light,
    pub serial: u64,
    pub shader_gel: Option<(u32, u32)>,
    pub view_target: [u32; 8],
    pub sorted: &'a SortedLayer,
    pub split: bool,
}

pub trait Host: Send + Sync {
    /// Confirm all target copies. Consume only the confirmed prefix on
    /// this stack before publishing any draw to the worker.
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>, esi: &mut u32) -> bool;
    /// Before the guest constants or gel quad for this invocation.
    fn checkpoint_gel(&self, serial: u64);
    fn target(&self) -> Option<[u32; 8]>;
    fn end_layer(&self, mem: &mut dyn GuestMemory);
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static SERIAL: AtomicU64 = AtomicU64::new(0);
static SPLIT_ENABLED: AtomicBool = AtomicBool::new(false);


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
    if let Some(c) = CANDIDATE.take() {
        refused("no eligible sorted geometry dispatch");
        reference::refused(c.light);
    }
}

pub fn new_view() {
    finish_candidate();
    reference::next_light();
    SERIAL.fetch_add(1, Ordering::Relaxed);
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let target = host.and_then(|h| h.target());
    VIEW_TARGET.set(target);
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
    reference::next_light();
    timeline::native::Stage::Lights.count(timeline::native::Metric::Examined, 1);
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
        Err(why) => { refused(why); reference::refused(light); }
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

fn geometry(mem: &mut dyn GuestMemory, esp: u32, esi: &mut u32, port_caller: impl FnOnce(u32) -> bool) -> Option<u32> {
    let c = CANDIDATE.with_borrow_mut(|slot| slot.take_if(|c| c.inside_layer))?;
    let caller = mem.u32(esp).ok();
    let caller = match caller {
        Some(pc) if pc == light_list::HANDLER_RETURN || port_caller(pc) => pc,
        _ => { refused("outside the sorted light geometry call"); return None; }
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(host) = host else { refused("no light host"); return None; };
    let Some(view_target) = VIEW_TARGET.get() else { refused("light view target unavailable"); return None; };
    let accepted = match light_list::sorted_at(&Ro(mem), LIGHT_LIST, c.layer, esp, *esi, caller) {
        Ok(sorted) => host.submit(mem, Dispatch { light: c.light, serial: c.serial, shader_gel: c.shader_gel, view_target,
            sorted: &sorted, split: SPLIT_ENABLED.load(Ordering::Relaxed) }, esi),
        Err(why) => refused(why),
    };
    if !accepted { reference::refused(c.light); }
    accepted.then_some(caller)
}

pub struct NativeLights;

impl Feature for NativeLights {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Dynamic lights and their shadows drawn by the native GPU renderer")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        if b.switches().feature(crate::render_timing::ID, false) {
            eprintln!("native-lights: not placed with render-timing on (its light passes' timer needs their end)");
            return Ok(());
        }
        b.raw()
            .observe_before(&points::RENDER_DYNAMIC_LIGHTS, "native_lights_view", |_| new_view())
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_LIGHT_PASSES, "native_lights_begin", |o| {
                begin_light(&*o.guest(), Entry { eax: o.cpu.eax, ecx: o.cpu.ecx, esp: o.cpu.esp,
                    mxcsr: o.cpu.mxcsr, x87_control: o.cpu.x87.control });
            })
            .context_only();
        SPLIT_ENABLED.store(split_on(b.switches()), Ordering::Relaxed);
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_lights_layer", |o| {
                let mem = o.guest();
                enter_layer(&*mem, mem.u32(o.cpu.esp).unwrap_or(0));
            })
            .after_feature(crate::native_world::ID)
            .after_feature(crate::native_objects::ID)
            .context_only();
        b.raw()
            .before(&points::LIGHT_GEOMETRY_DRAW, "native_lights_geometry", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let Some(caller) = geometry(&mut *rt.memory(), c.cpu.esp, &mut c.cpu.esi, |pc| {
                    c.ctx.live_continuations().iter().any(|k| k.token == pc
                        && k.target == skip::GEOMETRY_DRAW && k.origin == crate::render_loop::SORTED_HANDLER)
                }) else {
                    return Ok(Flow::Continue);
                };
                // sorted() already checked the return address on this locked stack.
                c.cpu.esp = c.cpu.esp.wrapping_add(4 + engine::render_loop::layer::HANDLER_CLEANUP);
                Ok(Flow::Return(runtime::TransferAbi::ret(caller)))
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "native_lights_end", |_| {
                let inside = CANDIDATE.with_borrow(|c| c.as_ref().is_some_and(|c| c.inside_layer));
                if inside { finish_candidate(); }
            })
            .context_only();
        let compare = std::env::var("H2_NATIVE_LIGHTS_COMPARE").as_deref() == Ok("1");
        reference::enable(compare);
        if compare {
            b.raw()
                .observe_before(&points::LAYER_END, "native_lights_compare_end", |o| {
                    let mut guest = o.guest();
                    let ret = guest.u32(o.cpu.esp).unwrap_or(0);
                    if !(native_render::lights::RENDER_LIGHT_PASSES..0x0003_1511).contains(&ret) {
                        return;
                    }
                    reference::layer_end(&*guest);
                    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(h) = host {
                        h.end_layer(&mut *guest);
                    }
                })
                .context_only();
        }
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
