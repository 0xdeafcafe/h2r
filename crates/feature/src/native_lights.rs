//! `native-lights`: the dynamic lights drawn by the native renderer, in the
//! game's own order (docs/graphics/native-renderer.md, "Stage 4: lights
//! and shadows"). Off by default; `H2_FEATURES=impl.render.native_lights=on`
//! turns it on (exactly that key).
//!
//! `RENDER_DYNAMIC_LIGHTS` (`0x00031c80`) gathers each visible light and
//! calls `RENDER_LIGHT_PASSES` (`0x00031170`) once for it, which draws the
//! light's layers 10 to 14. Two hooks, both context-only and with nothing
//! after a body, so the pipelined render runs them on its own thread
//! (docs/graphics/pipelined-render.md, "Render-safe hooks"):
//!
//! - a `before` on the light passes: when the [`Host`] draws the light
//!   (its record copied with [`read_light`]), add it to the open [`Run`]
//!   and answer `ret 0x10` in place of the passes, so none of the light's
//!   layers is built or drawn, and do the passes' tail as they would:
//!   the host's device side first ([`Host::tail`]: stencil off, no
//!   scissors, the depth-clip range and viewport, through the device in
//!   render order), then the guest words the tail zeroes
//!   (`lights::TAIL_ZEROES`), so a skipped light leaves what a drawn one
//!   does. Without an open run, ask the host for one:
//!   it asks the device for an external pass that takes the run's lights
//!   when the device issues it, at its next entry, which is the next D3D
//!   light's first draw (or what follows the lights). A sealed run is
//!   replaced by a new one, so each run lands between the D3D lights it
//!   sits between;
//! - an observer before the dynamic lights: forgets the open run, so a run
//!   never spans two views (or a pass the device dropped), and records the
//!   device's bound target ([`Host::target`]). The tail's last call sets
//!   the view's target back, which a skipped light does not repeat, so a
//!   light is skipped only while the bound target is still that one.
//!
//! The light is read, and the tail's words written, in the memory the
//! hook's context sees, which on the render thread is the render's
//! snapshot of the frame; an attached light's frame was refreshed just
//! before the call.
//!
//! With `render-timing` on nothing is placed: its timed light passes need
//! the body's end, which a skipped light never reaches.
//!
//! **Split lights** ([`SPLIT_SWITCH`], off by default). A light the host
//! does not draw whole (it meets a world surface left to D3D, or a live
//! object) may still have native world surfaces: [`Host::split`] names
//! their shaders, the light joins the run for them and is not skipped, and
//! two more context-only observers keep its D3D passes off those surfaces:
//! before the layer draw, for the light passes' own draws of layers 14, 12
//! and 13 on the light's list ([`SPLIT_LAYERS`], by return address), the
//! layer's bit is cleared on those shaders' BSP items
//! (`native_render::skip::clear_structure`; objects' items keep it); before
//! the layer's end (`LAYER_END`, which follows each of those draws) it is
//! set back. So each surface gets the light once: its native surfaces in
//! the run's pass, the rest from its D3D passes. The two sets of pixels do
//! not overlap (the lit passes test depth equal against what layer 3 laid
//! down), so the run's pass coming before the light's D3D draws changes
//! nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::handle::DatumHandle;
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light;
use native_render::camera::{GuestRead, GuestWrite, RENDER_CAMERA, RenderCamera};
use native_render::lights::{LIGHT_PASSES_CLEANUP, Light, LightConstants, Run, TAIL_CLIP, TAIL_ZEROES};
use native_render::skip;

use crate::native_world::{Mem, Pending};
use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-lights";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_lights";

/// Whether the native lights are on (off by default).
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// Split lights ([`split_layer`]): a light the host cannot draw whole
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

/// What draws the lights: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// Whether the host draws `light` natively now (its definition and
    /// what it lights are loaded and supported).
    fn accepts(&self, mem: &dyn GuestMemory, light: &Light) -> bool;
    /// Asks the device for an external pass over `run` with `camera`; the
    /// pass seals the run when the device issues it. `false`: nothing
    /// asked (the light goes to the D3D path).
    fn open(&self, camera: &RenderCamera, run: Arc<Run>) -> bool;
    /// The device side of the passes' tail, at this point of the render
    /// (`d3d::Device::light_passes_tail`, with `clip` the game's clip-range
    /// words). `false`: not done, the light goes to the D3D path.
    fn tail(&self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool;
    /// The device's bound target now (`d3d::Device::bound_target`); `None`
    /// when it cannot say (nothing is skipped then).
    fn target(&self) -> Option<[u32; 8]>;
    /// For a light it does not draw whole ([`Host::accepts`] no, or its
    /// sphere meets a live object): the world shaders (tag indices) whose
    /// surfaces it draws natively for `light`, while the light's D3D passes
    /// draw everything else. `None` (the default) or empty: all to D3D.
    fn split(&self, _mem: &dyn GuestMemory, _light: &Light) -> Option<Arc<HashSet<u16>>> {
        None
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
/// The run native lights join until the device issues its pass.
static OPEN: Mutex<Option<Arc<Run>>> = Mutex::new(None);
static LIGHTS: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);
/// The bound target before this view's lights.
static VIEW_TARGET: Mutex<Option<[u32; 8]>> = Mutex::new(None);
/// Lights left to D3D because the bound target was not the view's.
static TARGET_MOVED: AtomicU64 = AtomicU64::new(0);
/// Lights the host would draw left to D3D because they meet an object.
static OBJECTS_MET: AtomicU64 = AtomicU64::new(0);
/// Whether lights may be split ([`set_split`]).
static SPLIT_ENABLED: AtomicBool = AtomicBool::new(false);
/// The light whose passes are running, when split: the shaders whose
/// world items lose its layers. Set by the light's `before`, so the next
/// light's clears it.
static SPLIT: Mutex<Option<Arc<HashSet<u16>>>> = Mutex::new(None);
/// The items a split light's layer draw is running without, until the
/// layer's end sets them back.
static SPLIT_CLEARED: Mutex<Option<Pending>> = Mutex::new(None);
/// Lights split, layer draws split, world items left out of them.
static SPLITS: AtomicU64 = AtomicU64::new(0);
static SPLIT_DRAWS: AtomicU64 = AtomicU64::new(0);
static SPLIT_ITEMS: AtomicU64 = AtomicU64::new(0);

/// Lets lights split (the feature's build, with [`SPLIT_SWITCH`] on).
pub fn set_split(on: bool) {
    SPLIT_ENABLED.store(on, Ordering::Relaxed);
}

/// Lights split, their layer draws that left world items to the native
/// pass, and those items.
pub fn split_counts() -> (u64, u64, u64) {
    (
        SPLITS.load(Ordering::Relaxed),
        SPLIT_DRAWS.load(Ordering::Relaxed),
        SPLIT_ITEMS.load(Ordering::Relaxed),
    )
}

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Lights met, lights drawn natively, runs opened.
pub fn counts() -> (u64, u64, u64) {
    (
        LIGHTS.load(Ordering::Relaxed),
        SKIPPED.load(Ordering::Relaxed),
        RUNS.load(Ordering::Relaxed),
    )
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Lights left to D3D because the bound target had moved from the view's.
pub fn target_refusals() -> u64 {
    TARGET_MOVED.load(Ordering::Relaxed)
}

/// Lights the host would draw left to D3D because their sphere meets a
/// live object's (`objects::object_meets_sphere`): their passes light
/// the object, which the native lights do not yet.
pub fn object_refusals() -> u64 {
    OBJECTS_MET.load(Ordering::Relaxed)
}

/// The observer before the dynamic lights: no run carries over, and the
/// bound target now is the view's.
pub fn new_view() {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).take();
    SPLIT.lock().unwrap_or_else(|e| e.into_inner()).take();
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // The host's target takes the device lock: asked before VIEW_TARGET is
    // taken, so no guard is held across it.
    let target = host.and_then(|h| h.target());
    *VIEW_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = target;
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
/// asked: they only set the scissor rectangle, which the native pass does
/// not use. S: `0x00040e90` (called with EAX 0 before `0x0004a1e0`) is
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
    let gel = lp::gel(
        f.flags,
        definition_flags,
        mem.u32(definition.wrapping_add(lp::DEFINITION_GEL_SHADER)).ok()?,
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
    })
}

/// The light passes' `before`, on any guest memory: `esp` at the entry
/// (the return address; the light datum, position, shadow byte and weight
/// above it). `true` when the light joined a native run, the tail is done
/// and the passes must not run.
pub fn on_light(mem: &mut dyn GuestMemory, esp: u32) -> bool {
    on_light_at(mem, esp, None)
}

/// [`on_light`] with the entry's registers, so the light carries its
/// constants ([`light_constants`]).
pub fn on_light_at(mem: &mut dyn GuestMemory, esp: u32, entry: Option<Entry>) -> bool {
    LIGHTS.fetch_add(1, Ordering::Relaxed);
    // A new light's passes: the last light's split is over.
    SPLIT.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let (Ok(handle), Ok(shadow), Ok(weight)) = (
        mem.u32(esp.wrapping_add(4)),
        mem.u8(esp.wrapping_add(0xc)),
        mem.u32(esp.wrapping_add(0x10)),
    ) else {
        return false;
    };
    let Some(mut light) = read_light(mem, handle, shadow != 0, f32::from_bits(weight)) else {
        return false;
    };
    light.constants = entry.and_then(|e| light_constants(mem, &e));
    let whole = host.accepts(mem, &light);
    let (centre, radius) = light.bounds;
    if whole && native_render::objects::object_meets_sphere(&Ro(mem), centre, radius) {
        OBJECTS_MET.fetch_add(1, Ordering::Relaxed);
        return split(&*host, mem, light);
    }
    if !whole {
        return split(&*host, mem, light);
    }
    // The tail sets the view's target back; skipped, it must still be bound.
    let view = *VIEW_TARGET.lock().unwrap_or_else(|e| e.into_inner());
    if view.is_none() || host.target() != view {
        TARGET_MOVED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let (Ok(near), Ok(far)) = (mem.u32(TAIL_CLIP[0]), mem.u32(TAIL_CLIP[1])) else {
        return false;
    };
    // The device side first (a light it cannot finish stays on D3D, and
    // the tail done early is the state that light's own tail ends with).
    // Then the guest words; a light that goes on to D3D sets and zeroes
    // them again.
    if !host.tail(mem, [near, far]) || !zero_tail(mem) {
        return false;
    }
    if !join(&*host, &Ro(mem), light) {
        return false;
    }
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    true
}

/// The guest words the passes' tail zeroes; `false` when one cannot be
/// written (nothing is skipped then).
pub fn zero_tail(mem: &mut dyn GuestMemory) -> bool {
    TAIL_ZEROES
        .iter()
        .all(|&(addr, bytes)| mem.write(addr, &[0u8; 4][..bytes as usize]).is_ok())
}

/// A light the host does not draw whole. With splitting on and world
/// shaders the host draws for it ([`Host::split`]), the light joins the
/// run (its native surfaces, drawn at the run's pass, which the device
/// issues at its next entry: this light's own first D3D call) and its D3D
/// passes run without those shaders' world items in layers 14, 12 and 13
/// ([`split_layer`]), so each surface is lit once. Returns `false`: the
/// passes run, and do their own tail.
fn split(host: &dyn Host, mem: &mut dyn GuestMemory, light: Light) -> bool {
    if !SPLIT_ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(shaders) = host.split(mem, &light).filter(|s| !s.is_empty()) else {
        return false;
    };
    // The run's pass draws into the bound target: the view's, as for a
    // skipped light.
    let view = *VIEW_TARGET.lock().unwrap_or_else(|e| e.into_inner());
    if view.is_none() || host.target() != view {
        TARGET_MOVED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    if !join(host, &Ro(mem), light) {
        return false;
    }
    *SPLIT.lock().unwrap_or_else(|e| e.into_inner()) = Some(shaders);
    SPLITS.fetch_add(1, Ordering::Relaxed);
    false
}

/// The observer before the layer draw (`ret` its return address): sets
/// back what the last split layer left out, then, for a split light's
/// layer 14, 12 or 13 on [`LIGHT_LIST`] ([`SPLIT_LAYERS`]), clears the
/// layer's bit on the world items of the light's native shaders
/// (`skip::clear_structure`: BSP clusters and instances only, so objects
/// keep their D3D light) for [`layer_end`] to set back.
pub fn split_layer(mem: &mut dyn GuestMemory, ret: u32) {
    restore_split(mem);
    let Some(shaders) = SPLIT.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    let mut m = Mem(mem);
    let Some(layer) = m.read_u32(skip::CURRENT_LAYER) else {
        return;
    };
    if !SPLIT_LAYERS.contains(&(layer, ret)) || m.read_u32(skip::CURRENT_LIST) != Some(LIGHT_LIST) {
        return;
    }
    let Some(c) = skip::clear_structure(&mut m, layer, &|shader| shaders.contains(&(shader as u16))) else {
        return;
    };
    SPLIT_DRAWS.fetch_add(1, Ordering::Relaxed);
    SPLIT_ITEMS.fetch_add(c.items.len() as u64, Ordering::Relaxed);
    let left = c.items.iter().map(|&item| m.read_u32(item + 0x10).unwrap_or(0)).collect();
    *SPLIT_CLEARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { cleared: c, left });
}

/// The observer before the layer's end (`LAYER_END`, right after each
/// layer draw of the light passes): sets back what [`split_layer`] left
/// out, on the items that still hold the mask it left: right after the
/// draw, before anything else reads the list (where the list is emptied
/// between lights is not traced).
pub fn layer_end(mem: &mut dyn GuestMemory) {
    restore_split(mem);
}

fn restore_split(mem: &mut dyn GuestMemory) {
    let Some(p) = SPLIT_CLEARED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut m = Mem(mem);
    let bit = 1u32 << p.cleared.layer;
    for (&item, &left) in p.cleared.items.iter().zip(&p.left) {
        if m.read_u32(item + 0x10) == Some(left) {
            let _ = m.write_u32(item + 0x10, left | bit);
        }
    }
}

/// Adds the light to the open run, or opens one; `false` when no pass
/// could be asked for.
fn join(host: &dyn Host, m: &Ro<'_>, light: Light) -> bool {
    // OPEN is only held to read or store the run: never across the run's
    // own lock or the host's (open takes the device lock).
    let current = OPEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(run) = current
        && !run.is_sealed()
    {
        return run.push(light);
    }
    let Some(camera) = RenderCamera::read(m, RENDER_CAMERA) else {
        return false;
    };
    let run = Arc::new(Run::new());
    run.push(light);
    let opened = host.open(&camera, Arc::clone(&run));
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = opened.then_some(run);
    if !opened {
        return false;
    }
    let runs = RUNS.fetch_add(1, Ordering::Relaxed) + 1;
    if runs % 1800 == 1 {
        let (l, s, r) = counts();
        let o = object_refusals();
        eprintln!("native-lights: {s} of {l} dynamic lights drawn natively, in {r} runs; {o} left to D3D for meeting an object");
    }
    true
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
            .before(&points::RENDER_LIGHT_PASSES, "native_lights_skip", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let skip = {
                    let mut mem = rt.memory();
                    let entry = Entry {
                        eax: c.cpu.eax,
                        ecx: c.cpu.ecx,
                        esp: c.cpu.esp,
                        mxcsr: c.cpu.mxcsr,
                        x87_control: c.cpu.x87.control,
                    };
                    on_light_at(&mut *mem, c.cpu.esp, Some(entry))
                };
                if skip {
                    c.ret(LIGHT_PASSES_CLEANUP).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            })
            .context_only();
        let split = split_on(b.switches());
        set_split(split);
        if split {
            b.raw()
                .observe_before(&points::LAYER_DRAW, "native_lights_split", |o| {
                    let mut g = o.guest();
                    let ret = g.u32(o.cpu.esp).unwrap_or(0);
                    split_layer(&mut *g, ret);
                })
                .after_feature(crate::native_world::ID)
                .after_feature(crate::native_objects::ID)
                .context_only();
            b.raw()
                .observe_before(&points::LAYER_END, "native_lights_split_end", |o| {
                    let mut g = o.guest();
                    layer_end(&mut *g);
                })
                .context_only();
        }
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
