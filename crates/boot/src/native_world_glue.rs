//! The native world's host (docs/graphics/native-renderer.md, stage 1):
//! loads the active BSP's world from the loaded map's file when the BSP
//! changes, on a worker, and asks the D3D device for the native pass at
//! each view's layer 3 (`feature::native_world`). Inert unless
//! `impl.render.native_world` is on (nothing asks).
//!
//! - **Load once.** On a new (map, BSP) the worker opens the map through
//!   the VFS ([`set_locator`]), finds the scenario BSP whose tag data sits
//!   at the active BSP's address, builds the world (`native_render::load`),
//!   decodes each lightmap page, and translates each material's layer-3
//!   pass. The GPU side is made on the render thread by the first pass.
//!   Until it is ready, or when it fails, the D3D path draws the world.
//! - **Per frame.** The camera and one external pass; the device records it
//!   after the next drain, in order with the draws.
//! - **Material programs** (`impl.render.native_world_programs`, on with
//!   the world). A material is native only when its layer-3 pass
//!   translates (`native_render::look::layer3_pass`,
//!   `native_render::program::compile_world`) and every part of it can be
//!   drawn with it (`program::world_fit`, its stage bitmaps decode); it
//!   then draws with its own vertex program, pixel shader, stage textures,
//!   constants and render states, and the device's view constants each
//!   frame. Every other material is left out of the world and keeps its
//!   D3D draws (the skip clears only the items of the parts the world
//!   draws, by owner and part index, whatever shares their shader). The load says
//!   the coverage and each reason once, with counts. A palettized stage
//!   bitmap (the bump map's P8) is expanded through the device palette
//!   bound at its stage when the load started, as the D3D path samples
//!   it; while that palette is no longer bound the D3D path draws the
//!   world.
//! - **Stage 1** (`impl.render.native_world_programs=off`, for the A/B
//!   only): every material whose base map decodes, drawn as base map
//!   times lightmap.
//! - **Lights** (`impl.render.native_lights`): the lights' part of the
//!   world, loaded with it (`native_lights_glue::load`).

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use feature::native_world::{Host, PartSet};
use map::{Map, ResourceFile, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, GpuWorld, Image, Owners, PassStencil, PassViewport};
use native_render::load::{self, LoadedWorld};
use native_render::look::{self, Stage};
use native_render::program::{self, MaterialProgram, ProgramSet, ViewInputs};
use native_render::transparent_gpu::{TransparentProgram, TransparentSet};
use native_render::skip::StencilPlan;
use native_render::world::{LightmapKey, Owner, World, WorldDraw};

use crate::telemetry::{MapFile, MapLocator};

static LOCATOR: Mutex<Option<MapLocator>> = Mutex::new(None);

/// How the glue finds a map's file by its short name (boot: the VFS).
pub fn set_locator(locate: MapLocator) {
    *LOCATOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(locate);
}

pub(crate) fn locate(name: &str) -> Option<MapFile> {
    LOCATOR.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?(name)
}

/// What the worker hands the render thread: the world and its images.
pub struct CpuWorld {
    pub world: World,
    pub base_maps: HashMap<u16, Image>,
    pub lightmaps: HashMap<LightmapKey, Image>,
    /// Materials with their own programs, and their stages' images.
    pub programs: HashMap<u16, MaterialProgram>,
    /// Material programs on: the world draws only with them, never with
    /// stage 1's program.
    pub programs_on: bool,
    pub stage_images: StageImages,
    /// Transparent materials (`impl.render.native_transparents`): each
    /// one's layer-15 passes, compiled.
    pub transparents: HashMap<u16, Vec<TransparentProgram>>,
}

/// One loaded world: CPU data until the first pass uploads it.
pub struct Ready {
    cpu: Mutex<Option<CpuWorld>>,
    gpu: Mutex<Option<GpuWorld>>,
    /// The parts the native pass draws, with their shaders: what the skip
    /// clears.
    parts: Arc<PartSet>,
    /// The transparent parts the native pass draws, with their shader's
    /// tag index (`native_transparents_glue`).
    pub(crate) transparent_parts: Arc<HashMap<(Owner, u8), u16>>,
    /// What the native lights draw over this world
    /// (`native_lights_glue`; `None` with them off).
    pub(crate) lights: Option<Arc<crate::native_lights_glue::LightsWorld>>,
    /// The device palettes the load expanded palettized stage bitmaps
    /// through, at the stages it used them (the rest `None`): each ask
    /// checks they are still bound.
    palettes: StagePalettes,
}

/// The current (map, BSP) and its world.
/// The current (map, BSP address, BSP index) and its world. The index
/// matters: a level's BSPs load at one address (03a's intro cinematic and
/// its first play space share `0x800e6010`), so the address alone loaded
/// the play space's world for the cinematic's BSP.
pub(crate) type State = Arc<Mutex<(Option<(String, u32, u16)>, Load)>>;

pub(crate) enum Load {
    Idle,
    Loading,
    Ready(Arc<Ready>),
    Failed,
}

/// The host: the device and the current world.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn ask(&self, mem: &dyn GuestMemory, bsp: u32, camera: &RenderCamera, plan: &StencilPlan) -> Option<Arc<PartSet>> {
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() || bsp == 0 {
            return None;
        }
        let index = active_bsp_index(mem)?;
        let key = (name, bsp, index);
        let ready = 'ready: {
            {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if st.0.as_ref() == Some(&key) {
                    match &st.1 {
                        Load::Ready(r) => break 'ready Arc::clone(r),
                        Load::Idle | Load::Loading | Load::Failed => return None,
                    }
                }
                *st = (Some(key.clone()), Load::Loading);
            }
            // The palettes bound now, for the stage bitmaps that are
            // palettized (P8, P8 bump): the D3D path samples them through
            // the device's palette at their stage, which the load expands
            // them through. Read with no other lock held.
            let palettes = self.device.lock().ok().map(|d| stage_palettes(&d, mem)).unwrap_or_default();
            {
                let state = Arc::clone(&self.state);
                std::thread::Builder::new()
                    .name("native-world load".into())
                    .spawn(move || {
                        let t0 = std::time::Instant::now();
                        let out = load_ready(&key.0, key.1, key.2, palettes);
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        if st.0.as_ref() != Some(&key) {
                            return;
                        }
                        st.1 = match out {
                            Ok(r) => {
                                // One guard, dropped before the print: two
                                // `lock()`s in the macro's arguments both live
                                // to the statement's end, and the second
                                // waited on the first forever (with `state`
                                // held, so the render thread's next ask hung).
                                let (draws, batches) = {
                                    let cpu = r.cpu.lock().unwrap_or_else(|e| e.into_inner());
                                    cpu.as_ref().map_or((0, 0), |c| (c.world.draws.len(), c.world.batches.len()))
                                };
                                eprintln!(
                                    "native-world: {} BSP {} at {:#010x}: {} draws in {} batches, {} parts native, in {:.1} s",
                                    key.0,
                                    key.2,
                                    key.1,
                                    draws,
                                    batches,
                                    r.parts.len(),
                                    t0.elapsed().as_secs_f32()
                                );
                                Load::Ready(Arc::new(r))
                            }
                            Err(e) => {
                                eprintln!("native-world: {} BSP {} at {:#010x}: not loaded ({e}); the D3D path draws it", key.0, key.2, key.1);
                                Load::Failed
                            }
                        };
                    })
                    .ok()?;
                return None;
            }
        };
        let mut d = self.device.lock().ok()?;
        if !d.can_external_pass() {
            return None;
        }
        // The programs write w as depth and clip w (`xps_w_depth`): a view
        // whose draws store z instead (`SET_CONTROL0` without
        // z-perspective) is the D3D path's.
        if d.hw().control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE == 0) {
            let n = NOT_W_BUFFERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 600 == 0 {
                eprintln!("native-world: {} views left to the D3D path: not W-buffered (SET_CONTROL0 without z-perspective)", n + 1);
            }
            return None;
        }
        // The palettes the load expanded its palettized stage bitmaps
        // through must still be the ones bound: otherwise the D3D path
        // draws the world (its samples would differ).
        if !palettes_hold(&ready.palettes, &|s| d.stage_palette(&ReadMemory(mem), s)) {
            if !PALETTE_CHANGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-world: a stage palette changed since the load expanded its bitmaps through it; the D3D path draws the world");
            }
            return None;
        }
        // The stencil the layer's draws take now, and how its items switch
        // it (`skip::stencil_plan`).
        let Some(stencil) = d.stencil_state(&ReadMemory(mem)) else {
            if !STENCIL_UNREAD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-world: the device's stencil state does not read; the D3D path draws the world");
            }
            return None;
        };
        let Some(stencil) = pass_stencil(&stencil, plan) else {
            let n = STENCIL_REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 600 == 0 {
                eprintln!(
                    "native-world: {} views left to the D3D path: an item switches the stencil and layer 3 starts in another state ({stencil:?})",
                    n + 1
                );
            }
            return None;
        };
        // The cull layer 3 starts in: what the passes that do not set
        // `CULLMODE` inherit (`native_render::look::WorldPass::inherits_cull`).
        let cull = d.cull_state(&ReadMemory(mem)).map(inherited_cull);
        d.request_external_pass(Box::new(WorldPass {
            ready: Arc::clone(&ready),
            camera: *camera,
            view: None,
            w_clip: None,
            clip: None,
            traced: false,
            stencil,
            cull,
        }));
        let owners = Diagnostics::get().owners;
        if owners == Owners::All {
            return Some(Arc::clone(&ready.parts));
        }
        Some(Arc::new(ready.parts.iter().filter(|((o, _), _)| owners.keeps(*o)).map(|(&k, &v)| (k, v)).collect()))
    }
}

/// Installs the host on the device (with the occlusion capture), and the
/// transparents' host over the same world.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let state: State = Arc::new(Mutex::new((None, Load::Idle)));
    feature::native_world::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::clone(&state),
    }));
    crate::native_transparents_glue::install(device, Arc::clone(&state));
    crate::native_lights_glue::install(device, state);
    // The post loads its own screen effects (`native_post_glue::EffectsSlot`).
    crate::native_post_glue::install(device);
}

/// The world's GPU side, uploaded from its CPU side by the first pass that
/// needs it (on the render thread). `None` inside when it was taken and
/// is gone.
pub(crate) fn upload<'a>(ready: &'a Ready, t: &d3d::ExternalTarget<'_>) -> std::sync::MutexGuard<'a, Option<GpuWorld>> {
    let (device, queue) = (t.device, t.queue);
    let mut gpu = ready.gpu.lock().unwrap_or_else(|e| e.into_inner());
    if gpu.is_some() {
        return gpu;
    }
    let Some(cpu) = ready.cpu.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return gpu;
    };
    let CpuWorld {
        world,
        mut base_maps,
        lightmaps,
        programs,
        programs_on,
        stage_images,
        transparents,
    } = cpu;
    // With programs on, stage 1's groups are never drawn: no copy of the
    // lightmap pages for them (the programs upload their own).
    let mut g = GpuWorld::new(device, queue, world, &mut |m| base_maps.remove(&m), &mut |k| {
        if programs_on { None } else { lightmaps.get(&k).cloned() }
    });
    let mut stage = |s: Stage| match s {
        Stage::Bitmap { bitmap, image } => stage_images.get(&(bitmap, image)).cloned(),
        Stage::Empty | Stage::Lightmap => None,
    };
    if programs_on {
        // Set even when empty: with programs on, a batch without one is
        // never drawn with stage 1's program.
        let set = ProgramSet::new(device, queue, g.world(), programs, &mut stage, &mut |k| lightmaps.get(&k).cloned());
        eprintln!(
            "native-world: {} materials draw with their own programs, {} of them their placements with their rigid programs",
            set.len(),
            set.rigid_len()
        );
        g.set_programs(set);
    }
    if !transparents.is_empty() {
        let set = TransparentSet::new(device, queue, g.world(), transparents, &mut stage, &mut |k| lightmaps.get(&k).cloned());
        eprintln!("native-transparents: {} transparent materials uploaded", set.len());
        g.set_transparents(set);
    }
    *gpu = Some(g);
    gpu
}

/// One view's world pass.
struct WorldPass {
    ready: Arc<Ready>,
    camera: RenderCamera,
    /// The device's view constants at the pass (material programs).
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`): the range
    /// and clamp the D3D path draws the layer with. Without it the
    /// programs culled what lies past `z_max`, where a cinematic's range
    /// reaches further and clamps (03a's intro: distant buildings left as
    /// sky-coloured slivers).
    w_clip: Option<([f32; 2], bool)>,
    /// The whole clip as the device gave it, for [`trace`].
    clip: Option<d3d::DepthClip>,
    /// Whether this pass was traced (it encodes once per copy of the
    /// targets).
    traced: bool,
    /// The stencil the pass draws with ([`pass_stencil`]).
    stencil: (gfx::state::StencilState, Vec<Owner>),
    /// The device's cull at the layer-3 call ([`inherited_cull`]; `None`:
    /// it did not read).
    cull: Option<native_render::program::Cull>,
}

/// `CULLMODE` and `FRONTFACE` as the D3D path's pipeline takes them
/// (`gfx::translate::primitive_state`).
pub(crate) fn inherited_cull((cull, front): (gfx::state::CullMode, gfx::state::FrontFace)) -> native_render::program::Cull {
    let rs = gfx::state::RenderState {
        cull_mode: cull,
        front_face: front,
        ..Default::default()
    };
    let p = gfx::translate::primitive_state(
        &rs,
        wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
    );
    (p.cull_mode, p.front_face)
}

impl d3d::ExternalPass for WorldPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
        self.clip = Some(clip);
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut gpu = upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return };
        let (placements, clusters) = Diagnostics::get().inherit_cull;
        gpu.set_inherited_cull(native_render::program::InheritedCull {
            device: self.cull,
            placements,
            clusters,
        });
        if gpu.has_programs() {
            // The programs read the view from the device's constants: a
            // pass issued without them draws nothing rather than a world
            // with last frame's (or no) camera.
            let Some(constants) = self.view else {
                if !NO_VIEW.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("native-world: a pass came without the device's view constants; it draws nothing");
                }
                return;
            };
            let view = ViewInputs {
                constants,
                guest_size: t.guest_size,
                size: t.size,
                z_scale: t.z_scale,
                w_clip: self.w_clip,
            };
            gpu.prepare_programs(t.queue, &view);
            if let Some(dump) = Diagnostics::get().dump {
                arm_dump(&self.ready, gpu, &view, dump);
            }
        }
        let diag = Diagnostics::get();
        gpu.set_cull(!diag.no_cull);
        gpu.set_owners(diag.owners);
        let culled = gpu.prepare(
            t.queue,
            &self.camera,
            DepthRule {
                full_scale: t.z_scale,
                floor: true,
            },
        );
        if diag.trace
            && !std::mem::replace(&mut self.traced, true)
            && trace(&self.camera, self.view.as_ref(), self.clip, &t, culled)
            && let Some(at) = diag.probe
        {
            let [_, _, width, height] = t.viewport;
            let hits = probe(gpu.world(), &self.camera.view_projection(), (width, height), at);
            eprintln!("native-world probe at {at:?}: {} parts: {hits:?}", hits.len());
        }
        let (state, marked) = &self.stencil;
        let mut stencil = PassStencil::from_device(state, t.depth_format);
        if stencil != PassStencil::default() {
            stencil.marked = marked.clone();
        }
        gpu.set_stencil(stencil);
        let [x, y, width, height] = t.viewport;
        gpu.encode(
            t.device,
            t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            PassViewport { x, y, width, height },
            t.size,
        );
        let unbound = gpu.unbound_batches();
        if unbound > 0 && !UNBOUND.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("native-world: {unbound} batch draws left out: their material program could not be bound");
        }
    }
}

/// The pass's diagnostics, from the environment (read once):
///
/// - `H2_NATIVE_WORLD_TRACE=1`: a line per pass for the first
///   [`TRACE_FIRST`] passes, then every [`TRACE_EVERY`]th ([`trace`]),
///   or every `n`th with `H2_NATIVE_WORLD_TRACE_EVERY=n` (with
///   `H2_NATIVE_WORLD_PROBE`, to probe near a given frame);
/// - `H2_NATIVE_WORLD_NO_CULL=1`: every draw drawn, no CPU cull
///   (`GpuWorld::set_cull`): a part the cull leaves out is then drawn, one
///   the GPU clips or the depth test rejects is still missing.
#[derive(Clone, Copy, Debug, Default)]
struct Diagnostics {
    trace: bool,
    /// Passes between traced ones after the first [`TRACE_FIRST`].
    trace_every: u64,
    no_cull: bool,
    /// `H2_NATIVE_WORLD_PROBE=x,y` (guest pixels, with the trace on): the
    /// parts whose triangles cover that pixel, on each traced pass
    /// ([`probe`]).
    probe: Option<(f32, f32)>,
    /// `H2_NATIVE_WORLD_OWNERS=clusters|placements`: only those sections'
    /// parts drawn natively, the rest left to the D3D path (their items
    /// not skipped): which kind a difference belongs to.
    owners: Owners,
    /// `H2_NATIVE_WORLD_DUMP=placement[:material][@passes]`: the uniforms
    /// the native pass draws that placement's rigid draws of the material
    /// (each of its materials without one) with, against the D3D path's draws of it
    /// ([`arm_dump`]; with `H2_NATIVE_WORLD_OWNERS=clusters`, so that the
    /// D3D path draws the placement), armed after that many of the world's
    /// passes (0 without one: the first, while the game may still be
    /// streaming the bitmaps in at low detail).
    dump: Option<Dump>,
    /// `H2_NATIVE_WORLD_INHERIT_CULL=none|placements|all` (all without
    /// it): whose draws of a pass that does not set `CULLMODE` take the
    /// device's cull at the layer-3 call, as (placements, clusters)
    /// (`native_render::program::InheritedCull`); the others cull nothing.
    inherit_cull: (bool, bool),
}

/// [`Diagnostics::dump`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Dump {
    instance: u32,
    material: Option<u16>,
    after: u64,
}

impl Diagnostics {
    fn get() -> Self {
        static D: std::sync::OnceLock<Diagnostics> = std::sync::OnceLock::new();
        *D.get_or_init(|| {
            let on = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0");
            let d = Diagnostics {
                trace: on("H2_NATIVE_WORLD_TRACE"),
                trace_every: parse_trace_every(std::env::var("H2_NATIVE_WORLD_TRACE_EVERY").ok().as_deref()),
                no_cull: on("H2_NATIVE_WORLD_NO_CULL"),
                probe: std::env::var("H2_NATIVE_WORLD_PROBE").ok().and_then(|v| parse_probe(&v)),
                owners: std::env::var("H2_NATIVE_WORLD_OWNERS").ok().and_then(|v| Owners::parse(&v)).unwrap_or_default(),
                dump: std::env::var("H2_NATIVE_WORLD_DUMP").ok().and_then(|v| parse_dump(&v)),
                inherit_cull: parse_inherit_cull(std::env::var("H2_NATIVE_WORLD_INHERIT_CULL").ok().as_deref()),
            };
            if d.trace || d.no_cull || d.owners != Owners::All || d.dump.is_some() || d.inherit_cull != (true, true) || d.trace_every != TRACE_EVERY {
                eprintln!("native-world: diagnostics {d:?}");
            }
            d
        })
    }
}

const TRACE_FIRST: u64 = 8;
const TRACE_EVERY: u64 = 250;
static TRACED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One pass's view as the native pass draws it, on one line: the device's
/// W-buffer clip at the issue (range, clamp, whether draws are
/// W-buffered: `d3d::DepthClip`) against the depth full scale, the render
/// camera the CPU cull uses (position, forward, fov, near, far) and its
/// view-projection beside the device's first four view constants (the
/// programs' transform: a cull by another camera than the constants' shows
/// as rows that disagree), the viewport, and the cull's count.
fn trace(
    camera: &RenderCamera,
    view: Option<&[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    t: &d3d::ExternalTarget<'_>,
    culled: native_render::cull::CullStats,
) -> bool {
    let n = TRACED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n >= TRACE_FIRST && n % Diagnostics::get().trace_every != 0 {
        return false;
    }
    let vp = camera.view_projection();
    let constants = view.map(|c| [c[0], c[1], c[2], c[3]]);
    eprintln!(
        "native-world trace {n}: z_scale {} clip {:?} camera pos {:?} fwd {:?} fov {} near {} far {} vp {:?} c0..3 {:?} viewport {:?} guest {:?} size {:?} cull {}/{}",
        t.z_scale,
        clip,
        camera.position,
        camera.forward,
        camera.fov,
        camera.near,
        camera.far,
        vp,
        constants,
        t.viewport,
        t.guest_size,
        t.size,
        culled.visible,
        culled.total,
    );
    true
}

/// `"x,y"` as a probe pixel.
/// `placement[:material][@passes]` ([`Diagnostics::dump`]).
fn parse_dump(v: &str) -> Option<Dump> {
    let (v, after) = match v.split_once('@') {
        Some((v, n)) => (v, n.trim().parse::<u64>().ok()?),
        None => (v, 0),
    };
    let (instance, material) = match v.split_once(':') {
        Some((i, m)) => (i, Some(m.trim().parse::<u16>().ok()?)),
        None => (v, None),
    };
    Some(Dump { instance: instance.trim().parse::<u32>().ok()?, material, after })
}

/// The world [`arm_dump`] counts passes of (its `Ready`'s address), the
/// count, and whether it armed for it.
static DUMPED: Mutex<(usize, u64, bool)> = Mutex::new((0, 0, false));

/// Once per loaded world: prints the uniforms the native pass draws
/// placement `instance`'s rigid draws of `material` (of each of its
/// materials without one) with in this frame, and arms
/// `d3d::uniform_compare` with them, which prints, for the D3D path's next
/// draws with the same rows and program, every read constant, read input
/// and uniform that differs; with several materials, each D3D draw is
/// compared with the one whose pixel shader it is
/// (`d3d::uniform_compare::pick`: a placement's materials can share a
/// vertex program). A D3D draw no material pairs with exactly is one the
/// native pass may not draw. Draws of the placement natively are skipped
/// on the D3D path, so this wants the placements left to it
/// (`H2_NATIVE_WORLD_OWNERS=clusters`).
fn arm_dump(ready: &Arc<Ready>, gpu: &GpuWorld, view: &ViewInputs, Dump { instance, material, after }: Dump) {
    let key = Arc::as_ptr(ready) as usize;
    let mut dumped = DUMPED.lock().unwrap_or_else(|e| e.into_inner());
    if dumped.0 != key {
        *dumped = (key, 0, false);
    }
    dumped.1 += 1;
    if dumped.2 || dumped.1 <= after {
        return;
    }
    dumped.2 = true;
    let draws = gpu.rigid_uniforms(instance, view);
    let materials: Vec<u16> = draws.iter().map(|d| d.material).collect();
    let chosen: Vec<_> = draws.into_iter().filter(|d| material.is_none_or(|m| m == d.material)).collect();
    if chosen.is_empty() {
        eprintln!("native-world dump: placement {instance} has no rigid draws of {material:?} here (materials {materials:?})");
        return;
    }
    let candidates = chosen
        .into_iter()
        .map(|d| {
            let label = format!("placement {instance} material {}", d.material);
            let read: Vec<String> = d
                .vertex
                .constants
                .iter()
                .enumerate()
                .skip(d3d::VIEW_CONSTANTS)
                .filter(|(_, c)| **c != [0.0; 4])
                .map(|(n, c)| format!("c[{n}] {c:?}"))
                .collect();
            eprintln!(
                "native-world dump {label} (materials {materials:?}): {} program words; beyond the view's: {}; defaults {:?}; w_clip {:?} clamp {} shift {}; cull {:?} front {:?}",
                d.words.len(),
                read.join(", "),
                d.vertex.defaults,
                d.vertex.w_clip,
                d.vertex.w_clamp,
                d.vertex.shift,
                d.pipeline.0.cull_mode,
                d.pipeline.0.front_face
            );
            d3d::uniform_compare::Expected {
                label,
                words: d.words,
                vertex: d.vertex,
                pixel: d.pixel,
                samplers: d.samplers,
                options: d.options,
                def: d.def,
                pixel_options: d.pixel_options,
                textures: d.textures,
                primitive: d.pipeline.0,
                depth: (d.pipeline.1.0, d.pipeline.1.1, Default::default()),
                draws: 4,
            }
        })
        .collect();
    d3d::uniform_compare::arm_all(candidates);
}

/// [`Diagnostics::inherit_cull`]: all without a value or with one it does
/// not know.
fn parse_inherit_cull(v: Option<&str>) -> (bool, bool) {
    match v.map(str::trim) {
        Some("none") => (false, false),
        Some("placements") => (true, false),
        _ => (true, true),
    }
}

/// [`Diagnostics::trace_every`]: [`TRACE_EVERY`] without a positive count.
fn parse_trace_every(v: Option<&str>) -> u64 {
    v.and_then(|v| v.trim().parse::<u64>().ok()).filter(|&n| n > 0).unwrap_or(TRACE_EVERY)
}

fn parse_probe(v: &str) -> Option<(f32, f32)> {
    let (x, y) = v.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// One part under a probed pixel ([`probe`]).
#[derive(Clone, Debug, PartialEq)]
struct ProbeHit {
    owner: Owner,
    part: u8,
    material: u16,
    /// The view depth (clip `w`) of the nearest covering triangle there.
    depth: f32,
    /// Pixels from the pixel to that triangle's nearest edge: under about
    /// one, the pixel is on an edge (a rasterization tie can differ).
    edge: f32,
}

/// The world's parts whose triangles cover pixel `at` (pixel centres at
/// `+0.5`, y down, in a `size` viewport) through `view_proj` (the render
/// camera's, the CPU cull's: within a pixel of the programs'), nearest
/// first: which part a differing pixel belongs to (a cluster or a
/// placement), how deep, and whether it lies on an edge. A triangle with a
/// vertex behind the eye is skipped (S: not clipped here).
fn probe(world: &World, view_proj: &native_render::camera::Mat4, size: (f32, f32), at: (f32, f32)) -> Vec<ProbeHit> {
    let screen = |p: [f32; 3]| {
        let c = native_render::camera::transform(view_proj, [p[0], p[1], p[2], 1.0]);
        (c[3] > 0.0).then(|| ((c[0] / c[3] + 1.0) * 0.5 * size.0, (1.0 - c[1] / c[3]) * 0.5 * size.1, c[3]))
    };
    let (px, py) = (at.0 + 0.5, at.1 + 0.5);
    let mut hits: Vec<ProbeHit> = Vec::new();
    for d in &world.draws {
        let mut best: Option<(f32, f32)> = None;
        let range = d.first_index as usize..(d.first_index + d.index_count) as usize;
        for tri in world.indices[range].chunks_exact(3) {
            let v: Option<Vec<(f32, f32, f32)>> = tri.iter().map(|&i| screen(world.vertices[i as usize].position)).collect();
            let Some(v) = v else { continue };
            let Some((depth, edge)) = covers(&v, (px, py)) else { continue };
            if best.is_none_or(|(b, _)| depth < b) {
                best = Some((depth, edge));
            }
        }
        if let Some((depth, edge)) = best {
            hits.push(ProbeHit { owner: d.owner, part: d.part, material: d.material, depth, edge });
        }
    }
    hits.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    hits.truncate(8);
    hits
}

/// Whether screen triangle `v` (x, y, view depth) covers `p`, within one
/// pixel outside its edges: the view depth there (interpolated
/// perspective-correctly) and the signed distance in pixels to its
/// nearest edge (negative outside).
fn covers(v: &[(f32, f32, f32)], p: (f32, f32)) -> Option<(f32, f32)> {
    let [a, b, c] = [v[0], v[1], v[2]];
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if area.abs() < 1e-6 {
        return None;
    }
    let sign = area.signum();
    let edge = |s: (f32, f32, f32), e: (f32, f32, f32)| {
        let len = ((e.0 - s.0).powi(2) + (e.1 - s.1).powi(2)).sqrt();
        sign * ((e.0 - s.0) * (p.1 - s.1) - (e.1 - s.1) * (p.0 - s.0)) / len
    };
    let (ea, eb, ec) = (edge(b, c), edge(c, a), edge(a, b));
    let nearest = ea.min(eb).min(ec);
    if nearest < -1.0 {
        return None;
    }
    // Barycentrics (screen space), then perspective-correct depth.
    let wa = sign * ((b.0 - p.0) * (c.1 - p.1) - (b.1 - p.1) * (c.0 - p.0)) / area.abs();
    let wb = sign * ((c.0 - p.0) * (a.1 - p.1) - (c.1 - p.1) * (a.0 - p.0)) / area.abs();
    let wc = 1.0 - wa - wb;
    let inv = wa / a.2 + wb / b.2 + wc / c.2;
    (inv > 0.0).then(|| (1.0 / inv, nearest))
}

static NOT_W_BUFFERED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Said once: a pass without view constants, batches left out.
static NO_VIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static UNBOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PALETTE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STENCIL_UNREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STENCIL_REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The stencil the native pass draws layer 3 with, from the device's state
/// as the layer starts (`now`) and how its items switch it (`plan`,
/// `0x0004d640`/`0x0004d720`): the state and the sections drawn with mode
/// 2's reference; `None` when that cannot be exact.
///
/// The native pass draws its parts before the layer's items and clears
/// them, so their switches do not run. Without any switch, every item
/// draws with `now`. With one, an item leaves mode 1, so `now` must be
/// mode 1 for the items left to D3D to see what they would have: then the
/// flags' mode 0 is the stencil off for every item, and otherwise a marked
/// section draws with mode 1's state and reference `0xff` (mode 2), the
/// rest with mode 1. Each part writes what its own item would have where
/// it is drawn (`ALWAYS`; the depth test decides which part is last).
fn pass_stencil(now: &gfx::state::StencilState, plan: &StencilPlan) -> Option<(gfx::state::StencilState, Vec<Owner>)> {
    if !plan.switches {
        return Some((*now, Vec::new()));
    }
    if !PassStencil::is_structure_mode(now) {
        return None;
    }
    if plan.off {
        return Some((gfx::state::StencilState { enable: false, ..*now }, Vec::new()));
    }
    Some((*now, plan.marked.clone()))
}

/// The device palette at each texture stage (`A8R8G8B8` entries), or
/// `None`: what a palettized stage bitmap (P8, P8 bump) is sampled through
/// on the D3D path. A world keeps those of the stages it expanded
/// bitmaps through.
pub type StagePalettes = [Option<Vec<u32>>; 4];

/// The palettes bound at stages 0..3 now.
pub(crate) fn stage_palettes<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> StagePalettes {
    std::array::from_fn(|s| d.stage_palette(&ReadMemory(mem), s))
}

/// Whether each palette a world expanded through is still the one bound
/// at its stage (`bound`).
pub(crate) fn palettes_hold(used: &StagePalettes, bound: &dyn Fn(usize) -> Option<Vec<u32>>) -> bool {
    used.iter().enumerate().all(|(s, p)| p.is_none() || bound(s) == *p)
}

/// The hook's guest memory as the device reads it, read-only.
pub(crate) struct ReadMemory<'a>(pub(crate) &'a dyn GuestMemory);

impl d3d::GuestMemory for ReadMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(|f| d3d::GuestError { addr: f.addr, len: f.len })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        Err(d3d::GuestError {
            addr,
            len: data.len() as u32,
        })
    }
}

/// Raw resource reads over the map and its shared maps.
pub(crate) struct Raw(pub(crate) assets::ResourceFiles);

impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0
            .read(pointer.file, pointer.offset, size)
            .map_err(|_| tags::Error::MissingResource(pointer.file))
    }
}

pub(crate) fn region(f: &MapFile) -> assets::Region {
    assets::Region {
        path: f.path.clone(),
        base: f.offset,
        length: f.len,
    }
}

/// Loads the world of map `name`'s BSP whose tag data sits at `bsp`.
/// The active structure BSP's index in the scenario's BSP block: the
/// game globals' (`engine::points::GAME_GLOBALS`) `i16` at `+2`, as
/// `0x0011c1b0` and `0x00137d40` index the block with it
/// (`live_telemetry::layout::GLOBALS_ACTIVE_BSP`). `None` when negative
/// or unread.
pub(crate) fn active_bsp_index(mem: &dyn GuestMemory) -> Option<u16> {
    let globals = mem.u32(engine::points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    u16::try_from(mem.u16(globals + 2).ok()? as i16).ok()
}

/// Which of the scenario's BSPs (`addresses`: each one's load address, in
/// block order) is the active one: the game's index, when that BSP loads
/// at the active BSP's address. Not the first BSP at that address: a
/// level's BSPs share it.
fn active_scenario_bsp(addresses: &[Option<u32>], index: u16, bsp: u32) -> Option<usize> {
    let i = usize::from(index);
    (addresses.get(i).copied().flatten() == Some(bsp)).then_some(i)
}

fn load_ready(name: &str, bsp: u32, active: u16, palettes: StagePalettes) -> Result<Ready, load::Error> {
    let file = locate(name).ok_or("the map's file is not found")?;
    let mut map = Map::from_region(BufReader::new(std::fs::File::open(&file.path)?), file.offset, file.len)?;
    let mut files = assets::ResourceFiles::new();
    files.set(ResourceFile::Local, region(&file))?;
    for (which, short) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        if let Some(f) = locate(short) {
            files.set(which, region(&f))?;
        }
    }
    let bsps = load::scenario_bsps(&mut map)?;
    let addresses: Vec<Option<u32>> = bsps
        .iter()
        .map(|(id, _)| map.tag(*id).ok().and_then(|t| t.location).map(|l| l.address))
        .collect();
    let index = active_scenario_bsp(&addresses, active, bsp).ok_or("the active BSP index is not a scenario BSP at the active BSP's address")?;
    let mut raw = Raw(files);
    let LoadedWorld {
        world,
        shaders,
        lightmap_bitmap,
        palettes: lightmap_palettes,
        ..
    } = load::load_world(&mut map, &mut raw, index)?;
    let mut files = raw.0;

    let programs_on = feature::Switches::from_env().is_ok_and(|s| feature::native_world::programs_on(&s));
    // Lightmap pages through their palettes, with their mip chains.
    let mut lightmaps = HashMap::new();
    let mut linear_pages = 0;
    if let Some(id) = lightmap_bitmap
        && let Ok(tags::DecodedTag {
            data: tags::TagData::Bitmap(b),
            ..
        }) = tags::DecodedTag::read(&mut map, id)
    {
        for key in world.lightmap_keys() {
            let (Some(image), Some(palette)) = (b.images.get(usize::from(key.page)), lightmap_palettes.get(usize::from(key.palette))) else {
                continue;
            };
            if image.format != tags::PixelFormat::P8 {
                continue;
            }
            let Ok(d) = assets::decode_image(image, &mut files) else { continue };
            if programs_on && d.desc.format.layout() == gfx::format::Layout::Linear {
                // A linear texture is addressed in texels on the D3D path
                // (its `tex_scale`); the programs here take coordinates
                // as normalised. Its materials stay on the D3D path.
                linear_pages += 1;
                continue;
            }
            let Some(page) = expand_levels(d.levels, palette) else { continue };
            lightmaps.insert(key, page);
        }
    }
    if linear_pages > 0 {
        eprintln!("native-world: {linear_pages} lightmap pages are linear textures; their materials stay on the D3D path");
    }
    // What draws natively. With material programs (the default) a
    // material is native only when its own layer-3 pass translates and
    // every part of it can be drawn with it: the rest keep their D3D
    // draws, never stage 1's look. Without them (the A/B against stage 1:
    // `impl.render.native_world_programs=off`), every material whose base
    // map decodes, drawn as base map times lightmap.
    let mut base_maps = HashMap::new();
    let mut native = HashSet::new();
    let mut decoder = StageDecoder::new(palettes);
    let mut palettized = Vec::new();
    let mut rigid_refused = Refused::new();
    let (programs, mut stage_images, refused) = if programs_on {
        let out = material_programs(&mut map, &mut files, &shaders, &world, &lightmaps, &mut decoder);
        native.extend(out.programs.keys().copied());
        palettized = out.palettized;
        rigid_refused = out.rigid_refused;
        (out.programs, out.images, out.refused)
    } else {
        let mut decoded: HashMap<TagId, Option<Image>> = HashMap::new();
        for m in world.materials() {
            let Some(&shader) = shaders.get(usize::from(m)) else { continue };
            let Some(bitmap) = load::base_map(&mut map, shader) else { continue };
            let image = decoded
                .entry(bitmap)
                .or_insert_with(|| decode_first(&mut map, &mut files, bitmap))
                .clone();
            if let Some(image) = image {
                base_maps.insert(m, image);
                native.insert(m);
            }
        }
        Default::default()
    };
    // The skip is per part (an item names its cluster or placement and
    // part, `skip::clear_parts`), so every native material is drawn
    // natively whatever other materials share its shader. The lights'
    // split is per shader (`feature::native_lights`), so they take only
    // the materials whose shader is native whole.
    let whole = whole_shader_materials(&world.materials(), &shaders, &native);
    // A native material's instanced placements draw natively only with
    // its rigid program ([`drawn_natively`]); the lights, whose native
    // passes would meet the ones left to D3D, take only materials whose
    // every placement draws natively.
    let rigid: HashSet<u16> = programs.iter().filter(|(_, p)| p.rigid.is_some()).map(|(&m, _)| m).collect();
    let instanced: HashSet<u16> = world
        .draws
        .iter()
        .filter(|d| matches!(d.owner, Owner::Instance(_)) && !rigid.contains(&d.material))
        .map(|d| d.material)
        .collect();
    if programs_on {
        log_coverage(&mut map, &world, &shaders, &native, refused);
        let placed = world.draws.iter().filter(|d| native.contains(&d.material) && matches!(d.owner, Owner::Instance(_)));
        let (with, without) = placed.fold((0, 0), |(w, wo), d| if rigid.contains(&d.material) { (w + 1, wo) } else { (w, wo + 1) });
        eprintln!(
            "native-world: {with} instanced-geometry parts draw natively with their rigid programs (the placement's rows in c[50..52] per draw, as 0x0004d830 uploads them); D3D keeps {without} of native materials without one"
        );
        for (why, ms) in &rigid_refused {
            let parts = world.draws.iter().filter(|d| ms.contains(&d.material) && matches!(d.owner, Owner::Instance(_))).count();
            eprintln!("native-world: D3D keeps the placements of {} materials ({parts} parts): {why}", ms.len());
        }
        if !palettized.is_empty() {
            // The materials whose look depends on a device palette (the
            // bump maps' normals): the ones to compare first in an A/B.
            let parts = world.draws.iter().filter(|d| palettized.contains(&d.material)).count();
            eprintln!(
                "native-world: {} native materials ({parts} parts) sample a palettized stage bitmap (P8 bump or P8) through the device's stage palette: {:?}",
                palettized.len(),
                palettized
            );
        }
    }
    // Transparent materials: each one's layer-15 passes where they all
    // translate, and the parts the native pass can draw.
    let transparents_on = feature::Switches::from_env().is_ok_and(|s| feature::native_transparents::on(&s));
    let (transparents, transparent_parts) = if transparents_on {
        crate::native_transparents_glue::load(&mut map, &mut files, &shaders, &world, &lightmaps, &mut stage_images, &mut decoder)
    } else {
        Default::default()
    };
    // The lights' part, over the whole world: the parts of materials left
    // to D3D, or sharing a shader with one (the lights' split skips by
    // shader), are where native lights cannot draw.
    let lights_on = feature::Switches::from_env().is_ok_and(|s| feature::native_lights::on(&s));
    let lights = if lights_on {
        crate::native_lights_glue::load(&mut map, &mut files, &shaders, &world, &|m| whole.contains(&m) && !instanced.contains(&m))
            .map(Arc::new)
    } else {
        None
    };
    // The screen effects are not the world's: `native_post_glue` loads
    // them itself, with or without the native world.
    // Draw only a native material's clusters, and its placements with its
    // rigid program: the rest keeps its D3D draws.
    let world = keep_draws(world, &|d| native.contains(&d.material) && drawn_natively(d, &rigid));
    let parts = part_set(&world, &shaders);
    Ok(Ready {
        cpu: Mutex::new(Some(CpuWorld {
            world,
            base_maps,
            lightmaps,
            programs,
            programs_on,
            stage_images,
            transparents,
        })),
        gpu: Mutex::new(None),
        parts: Arc::new(parts),
        transparent_parts: Arc::new(transparent_parts),
        lights,
        palettes: decoder.used().clone(),
    })
}

/// A P8 page's decoded levels (the index in every channel) through
/// `palette`, largest first, as one image with its mip chain.
fn expand_levels(levels: Vec<assets::Level>, palette: &[u32; 256]) -> Option<Image> {
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    Image::from_levels(
        levels
            .into_iter()
            .map(|l| {
                // P8 decodes as L8: the index in every channel.
                let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|p| p[0]).collect();
                (l.width, l.height, load::expand_page(&indices, palette))
            })
            .collect(),
    )
}

/// Of the world's materials (`materials`, by BSP index; `shaders` the
/// shader of each), those in `native` whose shader no other material
/// outside `native` uses: what a skip by shader (the lights' split) may
/// clear.
fn whole_shader_materials(materials: &[u16], shaders: &[TagId], native: &HashSet<u16>) -> HashSet<u16> {
    let shader = |m: u16| shaders.get(usize::from(m)).map(|s| s.0 as u16);
    let refused: HashSet<u16> = materials
        .iter()
        .filter(|&&m| !native.contains(&m))
        .filter_map(|&m| shader(m))
        .collect();
    materials
        .iter()
        .copied()
        .filter(|m| native.contains(m) && shader(*m).is_some_and(|s| !refused.contains(&s)))
        .collect()
}

/// The skip's table: each part the world draws (by owner and part index),
/// with its material's shader by tag index. `world` holds only native
/// materials' draws ([`keep_draws`]).
fn part_set(world: &World, shaders: &[TagId]) -> PartSet {
    world
        .draws
        .iter()
        .filter_map(|d| Some(((d.owner, d.part), shaders.get(usize::from(d.material))?.0 as u16)))
        .collect()
}

/// Materials left to the D3D path, by reason.
type Refused = std::collections::BTreeMap<String, Vec<u16>>;

/// The coverage of one load on stderr: how many materials (and parts)
/// draw natively, then one line per reason the rest stay on the D3D path,
/// most parts first, with a material that has it.
fn log_coverage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    world: &World,
    shaders: &[TagId],
    native: &HashSet<u16>,
    refused: Refused,
) {
    let mut parts: HashMap<u16, usize> = HashMap::new();
    for d in &world.draws {
        *parts.entry(d.material).or_default() += 1;
    }
    let count = |ms: &[u16]| ms.iter().map(|m| parts.get(m).copied().unwrap_or(0)).sum::<usize>();
    let materials = world.materials();
    let drawn: Vec<u16> = native.iter().copied().collect();
    eprintln!(
        "native-world: {} of {} materials draw natively with their own programs ({} of {} parts); {} stay on the D3D path for {} reasons",
        native.len(),
        materials.len(),
        count(&drawn),
        world.draws.len(),
        materials.len().saturating_sub(native.len()),
        refused.len()
    );
    let mut lines: Vec<(usize, String, Vec<u16>)> = refused.into_iter().map(|(why, ms)| (count(&ms), why, ms)).collect();
    lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (n, why, ms) in lines {
        let example = ms
            .first()
            .and_then(|&m| shaders.get(usize::from(m)))
            .and_then(|&s| map.tag(s).ok().and_then(|t| t.name.clone()))
            .unwrap_or_default();
        eprintln!("native-world: D3D keeps {} materials ({n} parts): {why}; e.g. {example}", ms.len());
    }
}

/// Whether the native pass draws a part of a native material: a
/// cluster's, and a placement's when the material has a rigid program
/// (`rigid`). The passes after layer 3 (shadows 11, lights 12 and 13, fog
/// 16, overlays 7) test `ZFUNC` equal against layer 3's depth, so the
/// native pass must write the D3D path's bits: a cluster goes through the
/// world program with the same position bits on both paths, and the D3D
/// path draws a placement with the rigid classification of the pass's
/// vertex program, which places the definition's vertices on the GPU with
/// the placement's rows in `c[50..52]` (`0x0004d830`: forward, left and up
/// by column and the position, copied from the placement the render
/// record points at, its scale not applied). The native pass does the
/// same, with the same rows per placement (`program::rigid_vertex`).
fn drawn_natively(d: &WorldDraw, rigid: &HashSet<u16>) -> bool {
    match d.owner {
        Owner::Cluster(_) => true,
        Owner::Instance(_) => rigid.contains(&d.material),
    }
}

/// The world with only the draws `keep` takes, batched again (the draws
/// stay in their order, so a batch is a run of one material and page).
fn keep_draws(mut world: World, keep: &dyn Fn(&WorldDraw) -> bool) -> World {
    world.draws.retain(|d| keep(d));
    world.batches.clear();
    for (i, d) in world.draws.iter_mut().enumerate() {
        match world.batches.last_mut() {
            Some(b) if b.material == d.material && b.lightmap == d.lightmap => b.draw_count += 1,
            _ => world.batches.push(native_render::world::Batch {
                material: d.material,
                lightmap: d.lightmap,
                first_draw: i as u32,
                draw_count: 1,
            }),
        }
        d.batch = world.batches.len() as u32 - 1;
    }
    world
}

/// Stage images by (bitmap, image index).
pub type StageImages = HashMap<(TagId, u32), Image>;

/// What [`material_programs`] gives.
#[derive(Default)]
struct Programs {
    programs: HashMap<u16, MaterialProgram>,
    images: StageImages,
    refused: Refused,
    /// Native materials whose placements stay on the D3D path, by reason
    /// (their rigid program does not translate or fit).
    rigid_refused: Refused,
    /// Materials with a palettized stage bitmap.
    palettized: Vec<u16>,
}

/// The world's materials whose layer-3 pass translates and whose every
/// part the native pass can draw with it (`program::compile_world`,
/// `program::world_fit`), with the images their stages sample; and the
/// others by the reason they stay on the D3D path. Stage images go
/// through `decoder` (a palettized one expanded through its stage's
/// palette, [`StageDecoder`]).
fn material_programs<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    decoder: &mut StageDecoder,
) -> Programs {
    let mut out = Programs::default();
    let Programs {
        programs,
        images,
        refused,
        rigid_refused,
        palettized,
    } = &mut out;
    let placed: HashSet<u16> = world
        .draws
        .iter()
        .filter(|d| matches!(d.owner, Owner::Instance(_)))
        .map(|d| d.material)
        .collect();
    let materials = world.materials();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            refused.insert(format!("the shader tags do not read ({e})"), materials);
            return out;
        }
    };
    for m in materials {
        let mut uses_palette = false;
        let Some(&shader) = shaders.get(usize::from(m)) else {
            refused.entry("no shader".into()).or_default().push(m);
            continue;
        };
        let compiled = lib
            .material(shader, None)
            .map_err(|e| format!("material: {e}"))
            .and_then(|mat| look::layer3_pass(&mat).map_err(|r| format!("layer 3: {r:?}")))
            .and_then(|pass| {
                let code = look::world_program(map, lib.metadata(), pass.look.vertex_shader).ok_or("no world-space vertex program")?;
                let mut p = program::compile_world(&pass, &code)?;
                if placed.contains(&m) {
                    // Its placements: the pass with the rigid
                    // classification, or they stay on the D3D path.
                    let rigid = look::rigid_program(map, lib.metadata(), pass.look.vertex_shader)
                        .ok_or_else(|| "no rigid vertex program".to_string())
                        .and_then(|code| program::compile_rigid(&pass, &code))
                        .and_then(|r| program::rigid_fit(world, m, &r).map(|()| r));
                    match rigid {
                        Ok(r) => p.rigid = Some(r),
                        Err(why) => rigid_refused.entry(why).or_default().push(m),
                    }
                }
                Ok(p)
            })
            .and_then(|p| {
                program::world_fit(world, m, &p, &|k: LightmapKey| lightmaps.contains_key(&k))?;
                for (n, s) in p.stages.iter().enumerate() {
                    let Stage::Bitmap { bitmap, image } = *s else { continue };
                    if p.dims[n].is_none() {
                        continue;
                    }
                    // A linear texture is addressed in texels on the D3D
                    // path (`tex_scale`); the programs take normalised
                    // coordinates.
                    uses_palette |= decoder.decode(map, files, images, bitmap, image, n, true)?;
                }
                Ok(p)
            });
        match compiled {
            Ok(p) => {
                programs.insert(m, p);
                if uses_palette {
                    palettized.push(m);
                }
            }
            Err(why) => refused.entry(why).or_default().push(m),
        }
    }
    palettized.sort_unstable();
    out
}

/// Stage images as their stages sample them, for every native pass that
/// loads with the world (its layer 3, the transparents; the objects can
/// take it too): a palettized bitmap (P8, P8 bump) is expanded through the
/// device palette bound at its stage when the load started
/// ([`stage_image`]), once per image, and another stage may share that
/// expansion only when it holds the same palette. Records which palettes
/// it used ([`Ready`] checks they stay bound) and which images do not
/// decode.
#[derive(Default)]
pub(crate) struct StageDecoder {
    /// The palettes bound at each stage when the load started.
    palettes: StagePalettes,
    /// Those an expansion used.
    used: StagePalettes,
    /// The stage each palettized image was expanded at.
    expanded_at: HashMap<(TagId, u32), usize>,
    /// Images that do not decode at any stage.
    failed: HashSet<(TagId, u32)>,
}

impl StageDecoder {
    pub(crate) fn new(palettes: StagePalettes) -> Self {
        Self {
            palettes,
            ..Self::default()
        }
    }

    /// The palettes the expansions used, at their stages (the rest
    /// `None`).
    pub(crate) fn used(&self) -> &StagePalettes {
        &self.used
    }

    /// Image `image` of `bitmap` as stage `stage` samples it, put in
    /// `images` unless there already: `true` when it is palettized (and
    /// so went through the stage's palette), or why the stage cannot have
    /// it, as `stage <n> bitmap <why>`. `refuse_linear`: a linear texture
    /// is refused (it is not cached as failed: another pass may take it).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode<R: std::io::Read + std::io::Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut assets::ResourceFiles,
        images: &mut StageImages,
        bitmap: TagId,
        image: u32,
        stage: usize,
        refuse_linear: bool,
    ) -> Result<bool, String> {
        self.take(images, (bitmap, image), stage, &mut |palette| {
            decode_stage(map, files, bitmap, image as usize, stage, palette, refuse_linear)
        })
    }

    /// [`decode`](Self::decode) with the decode itself as `decode` (given
    /// the stage's palette).
    fn take(
        &mut self,
        images: &mut StageImages,
        key: (TagId, u32),
        stage: usize,
        decode: &mut dyn FnMut(&Option<Vec<u32>>) -> Result<(Image, bool), StageRefusal>,
    ) -> Result<bool, String> {
        let palette = self.palettes.get(stage).ok_or_else(|| format!("stage {stage} does not exist"))?;
        if let Some(&at) = self.expanded_at.get(&key) {
            // One expansion per image: this stage must hold the same
            // palette (and stage 3 holds the lightmap's).
            if stage == 3 || *palette != self.palettes[at] {
                return Err(format!("stage {stage} bitmap is palettized and was expanded through another stage's palette"));
            }
            return Ok(true);
        }
        if images.contains_key(&key) {
            return Ok(false);
        }
        if self.failed.contains(&key) {
            return Err(format!("stage {stage} bitmap does not decode"));
        }
        match decode(palette) {
            Ok((i, expanded)) => {
                images.insert(key, i);
                if expanded {
                    self.expanded_at.insert(key, stage);
                    self.used[stage].clone_from(palette);
                }
                Ok(expanded)
            }
            Err(StageRefusal::Image(why)) => {
                self.failed.insert(key);
                Err(format!("stage {stage} bitmap {why}"))
            }
            // Not the image's fault: it may do at another stage or pass.
            Err(StageRefusal::Here(why)) => Err(format!("stage {stage} bitmap {why}")),
        }
    }
}

/// Why a stage cannot have a bitmap's image: the image itself, or only
/// here (this stage's palette, or a linear texture this pass refuses).
#[derive(Debug, PartialEq, Eq)]
enum StageRefusal {
    Image(&'static str),
    Here(&'static str),
}

/// Image `index` of a stage's bitmap at stage `stage`, and whether it was
/// expanded through the stage's palette; or why the stage cannot have it:
/// it does not decode, it is a linear texture (addressed in texels on the
/// D3D path, `tex_scale`, where the programs here take normalised
/// coordinates; only with `refuse_linear`), or it is palettized and the
/// stage has no palette for it ([`stage_image`]).
fn decode_stage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
    stage: usize,
    palette: &Option<Vec<u32>>,
    refuse_linear: bool,
) -> Result<(Image, bool), StageRefusal> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).map_err(|_| StageRefusal::Image("does not read"))?.data else {
        return Err(StageRefusal::Image("is not a bitmap"));
    };
    let image = b.images.get(index).ok_or(StageRefusal::Image("has no such image"))?;
    let (d, blocks) = assets::decode_image_blocks(image, files).map_err(|_| StageRefusal::Image("does not decode"))?;
    if refuse_linear && d.desc.format.layout() == gfx::format::Layout::Linear {
        return Err(StageRefusal::Here("is a linear texture"));
    }
    let (image, palettized) = stage_image(image.format, d.levels, stage, palette)?;
    Ok((with_blocks(image, blocks), palettized))
}

/// `image` with its BC blocks, when the D3D path uploads the bitmap as BC
/// (`assets::decode_image_blocks`): the native pass then samples the same
/// GPU-decoded texels. The CPU decoder (`gfx::dxt`) truncates the
/// interpolated colours, the GPU rounds them, which left native lit
/// surfaces one level darker than the D3D path's on a few percent of
/// pixels.
fn with_blocks(image: Image, blocks: Option<assets::BlockLevels>) -> Image {
    let Some(b) = blocks else { return image };
    let mut levels: Vec<_> = b.levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    image.with_blocks(b.host.to_wgpu(), levels.into_iter().map(|l| l.rgba).collect())
}

/// A stage bitmap's decoded levels as the stage samples them, and whether
/// they went through its palette. A palettized format (P8, P8 bump)
/// decodes as its index in every channel, and the D3D path samples it
/// through the device palette bound at its stage: so it is expanded
/// through `palette`, and refused at stage 3 (where the structure draw
/// binds each lightmap's own palette, `0x00015c90`) or without a full
/// palette. For the world's layer 3, stages 0..2 hold the device's default
/// palette (bound on every stage at `0x00015b70`; only the lightmap binder
/// binds another, at stage 3).
fn stage_image(
    format: tags::PixelFormat,
    levels: Vec<assets::Level>,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool), StageRefusal> {
    let palette: Option<[u32; 256]> = if matches!(format, tags::PixelFormat::P8 | tags::PixelFormat::P8Bump) {
        if stage == 3 {
            return Err(StageRefusal::Here("is palettized at stage 3 (the lightmap's palette is bound there)"));
        }
        let p = palette
            .as_ref()
            .ok_or(StageRefusal::Here("is palettized and no palette is bound at its stage"))?;
        Some(
            p.as_slice()
                .try_into()
                .map_err(|_| StageRefusal::Here("is palettized and its stage's palette has fewer than 256 entries"))?,
        )
    } else {
        None
    };
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    let image = Image::from_levels(
        levels
            .into_iter()
            .map(|l| match &palette {
                // Decoded as L8: the index in every channel.
                Some(p) => {
                    let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|t| t[0]).collect();
                    (l.width, l.height, load::expand_page(&indices, p))
                }
                None => (l.width, l.height, l.rgba),
            })
            .collect(),
    )
    .ok_or(StageRefusal::Image("does not decode"))?;
    Ok((image, palette.is_some()))
}

/// The first image of a bitmap as RGBA, with its mip chain.
pub(crate) fn decode_first<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, files: &mut assets::ResourceFiles, id: TagId) -> Option<Image> {
    decode_image(map, files, id, 0)
}

/// Image `index` of a bitmap as RGBA, with its mip chain.
pub(crate) fn decode_image<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
) -> Option<Image> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else {
        return None;
    };
    let d = assets::decode_image(b.images.get(index)?, files).ok()?;
    let mut levels: Vec<_> = d.levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    // The whole decoded chain: the native pass samples with mipmaps, as
    // the D3D path does.
    Image::from_levels(levels.into_iter().map(|l| (l.width, l.height, l.rgba)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

    #[test]
    fn materials_left_to_the_d3d_path_leave_the_world() {
        let tri = |m| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            parts: vec![PartInput {
                material: m,
                transparent: false,
                triangles: vec![[0, 1, 2]],
            }],
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        for (i, m) in [1u16, 2, 2, 3].into_iter().enumerate() {
            b.add_section(Owner::Cluster(i as u32), &tri(m), &Placement::IDENTITY, None);
        }
        let native: HashSet<u16> = [2, 3].into_iter().collect();
        let w = keep_draws(b.finish(), &|d| native.contains(&d.material));
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(2, 0, 2), (3, 2, 1)]);
        assert_eq!(w.draws.iter().map(|d| (d.material, d.batch)).collect::<Vec<_>>(), vec![(2, 0), (2, 0), (3, 1)]);
    }

    #[test]
    fn a_skip_by_shader_takes_only_materials_whose_shader_is_native_whole() {
        // Materials 0..4; 1 and 3 share shader 0xe000_0011. 1 translates,
        // 3 does not: a skip by shader (the lights' split) would clear 3's
        // items too, so it takes 0 and 2 only.
        let shaders = [0xe000_0010, 0xe000_0011, 0xe000_0012, 0xe000_0011, 0xe000_0014].map(TagId);
        let translated: HashSet<u16> = [0, 1, 2].into_iter().collect();
        let whole = whole_shader_materials(&[0, 1, 2, 3, 4], &shaders, &translated);
        assert_eq!(whole, [0, 2].into_iter().collect::<HashSet<u16>>());
        // A material without a shader is never taken.
        let whole = whole_shader_materials(&[0, 9], &shaders, &[0, 9].into_iter().collect());
        assert_eq!(whole, [0].into_iter().collect::<HashSet<u16>>());
    }

    #[test]
    fn an_untranslated_material_keeps_its_d3d_draws_and_a_translated_one_is_skipped() {
        // The world as the loader leaves it with programs on: material 7
        // translated, materials 8 and 9 refused, 9 sharing 7's shader. 8
        // and 9 are neither drawn natively nor skipped, so the D3D path
        // still draws them; 7's cluster part is drawn natively and
        // skipped, the shader's other parts not, and 7's part on an
        // instanced placement stays on the D3D path without a rigid
        // program (the D3D path places it on the GPU), and is drawn
        // natively with one.
        let tri = |m| PartInput {
            material: m,
            transparent: false,
            triangles: vec![[0, 1, 2]],
        };
        let section = |parts| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            parts,
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &section(vec![tri(7), tri(9)]), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(3), &section(vec![tri(8), tri(7)]), &Placement::IDENTITY, None);
        let world = b.finish();
        let mut shaders = vec![TagId(0xe000_0000); 10];
        shaders[7] = TagId(0xe000_0107);
        shaders[8] = TagId(0xe000_0108);
        shaders[9] = TagId(0xe000_0107);
        let translated: HashSet<u16> = [7].into_iter().collect();
        let w = keep_draws(world.clone(), &|d| translated.contains(&d.material) && drawn_natively(d, &HashSet::new()));
        assert!(w.draws.iter().all(|d| d.material == 7), "8 and 9 are not drawn natively (no stage 1 look)");
        let parts = part_set(&w, &shaders);
        let want: PartSet = [((Owner::Cluster(0), 0), 0x107)].into_iter().collect();
        assert_eq!(parts, want, "7's cluster part by owner and index, with its shader; not 9's, which shares it, nor the placement's");
        // Batched again over what is left: one batch, one draw.
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(7, 0, 1)]);
        assert_eq!(w.draws[0].batch, 0);
        // With 7's rigid program its placement's part is drawn and skipped
        // too, in the same batch after the cluster's.
        let rigid: HashSet<u16> = [7].into_iter().collect();
        let w = keep_draws(world, &|d| translated.contains(&d.material) && drawn_natively(d, &rigid));
        let parts = part_set(&w, &shaders);
        let want: PartSet = [((Owner::Cluster(0), 0), 0x107), ((Owner::Instance(3), 1), 0x107)].into_iter().collect();
        assert_eq!(parts, want);
        let owners: Vec<_> = w.draws.iter().map(|d| (d.owner, d.batch)).collect();
        assert_eq!(owners, vec![(Owner::Cluster(0), 0), (Owner::Instance(3), 0)]);
    }

    /// 03a's intro cinematic: the level's BSPs load at one address, and the
    /// game's index says which is active. The first BSP at the address is
    /// not it (the address alone gave the play space's world, whose parts
    /// matched some of the cinematic BSP's items by cluster, part and
    /// shader: those items were skipped and nothing drew them).
    #[test]
    fn the_active_bsp_is_the_game_s_index_not_the_first_at_its_address() {
        let at = 0x800e_6010;
        let addresses = [Some(at), Some(at), Some(0x8010_0000), None];
        assert_eq!(active_scenario_bsp(&addresses, 1, at), Some(1));
        assert_eq!(active_scenario_bsp(&addresses, 0, at), Some(0));
        assert_eq!(active_scenario_bsp(&addresses, 2, at), None, "loads elsewhere");
        assert_eq!(active_scenario_bsp(&addresses, 3, at), None, "no location");
        assert_eq!(active_scenario_bsp(&addresses, 4, at), None, "past the block");
    }

    #[test]
    fn the_owners_switch_keeps_one_kind_of_section() {
        assert_eq!(Owners::parse("clusters"), Some(Owners::Clusters));
        assert_eq!(Owners::parse(" placements "), Some(Owners::Placements));
        assert_eq!(Owners::parse(""), Some(Owners::All));
        assert_eq!(Owners::parse("objects"), None);
        let (c, i) = (Owner::Cluster(31), Owner::Instance(282));
        assert!(Owners::All.keeps(c) && Owners::All.keeps(i));
        assert!(Owners::Clusters.keeps(c) && !Owners::Clusters.keeps(i));
        assert!(!Owners::Placements.keeps(c) && Owners::Placements.keeps(i));
    }

    #[test]
    fn the_probe_finds_the_covering_triangle_its_depth_and_edge() {
        let dump = |instance, material, after| Some(Dump { instance, material, after });
        assert_eq!(parse_dump("282"), dump(282, None, 0));
        assert_eq!(parse_dump(" 282 : 107 "), dump(282, Some(107), 0));
        assert_eq!(parse_dump("282:107@600"), dump(282, Some(107), 600));
        assert_eq!(parse_dump("282@30"), dump(282, None, 30));
        assert_eq!(parse_dump("282:"), None);
        assert_eq!(parse_dump("282@"), None);
        assert_eq!(parse_dump("girder"), None);
        assert_eq!(parse_inherit_cull(None), (true, true));
        assert_eq!(parse_inherit_cull(Some("none")), (false, false));
        assert_eq!(parse_inherit_cull(Some(" placements ")), (true, false));
        assert_eq!(parse_inherit_cull(Some("all")), (true, true));
        assert_eq!(parse_inherit_cull(Some("clusters")), (true, true));
        assert_eq!(parse_trace_every(None), TRACE_EVERY);
        assert_eq!(parse_trace_every(Some(" 10 ")), 10);
        assert_eq!(parse_trace_every(Some("0")), TRACE_EVERY);
        assert_eq!(parse_trace_every(Some("often")), TRACE_EVERY);
        // The D3D draws of placement 282 on 03a: `CULL_CW`, front face `CW`.
        use gfx::state::{CullMode, FrontFace};
        assert_eq!(inherited_cull((CullMode::Cw, FrontFace::Cw)), (Some(wgpu::Face::Front), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::Ccw, FrontFace::Cw)), (Some(wgpu::Face::Back), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::None, FrontFace::Ccw)), (None, wgpu::FrontFace::Ccw));
        assert_eq!(parse_probe("520, 80"), Some((520.0, 80.0)));
        assert_eq!(parse_probe("520"), None);
        assert_eq!(parse_probe("x,1"), None);
        // A screen triangle (0,0) (10,0) (0,10), view depths 2, 4, 4; both
        // windings.
        let t = [(0.0, 0.0, 2.0), (10.0, 0.0, 4.0), (0.0, 10.0, 4.0)];
        let (depth, edge) = covers(&t, (2.0, 2.0)).unwrap();
        assert!((edge - 2.0).abs() < 1e-4, "2 px from the x and y edges: {edge}");
        // Screen barycentrics (0.6, 0.2, 0.2): 1 / (0.6/2 + 0.2/4 + 0.2/4).
        assert!((depth - 2.5).abs() < 1e-4, "{depth}");
        let r = [t[0], t[2], t[1]];
        assert_eq!(covers(&r, (2.0, 2.0)).map(|(d, e)| ((d * 1e3).round(), (e * 1e3).round())), Some((2500.0, 2000.0)));
        let (_, edge) = covers(&t, (5.0, 5.0)).unwrap();
        assert!(edge.abs() < 1e-4, "on the hypotenuse");
        assert!(covers(&t, (-0.5, 3.0)).unwrap().1 < 0.0, "half a pixel outside: kept, negative");
        assert_eq!(covers(&t, (-2.0, 3.0)), None, "past a pixel outside");
        assert_eq!(covers(&[(0.0, 0.0, 1.0), (1.0, 1.0, 1.0), (2.0, 2.0, 1.0)], (1.0, 1.0)), None, "degenerate");
    }

    #[test]
    fn the_active_bsp_index_reads_the_game_globals() {
        let mut m = engine::memory::FlatMemory::new(0x0040_0000, 0x0021_0000);
        assert_eq!(active_bsp_index(&m), None, "no globals yet");
        let globals = 0x0050_0000;
        m.set_u32(engine::points::GAME_GLOBALS.base, globals).unwrap();
        m.set_u32(globals, 0x0001_0000).unwrap();
        assert_eq!(active_bsp_index(&m), Some(1));
        m.set_u32(globals, 0xffff_0000).unwrap();
        assert_eq!(active_bsp_index(&m), None, "-1: none active");
    }

    #[test]
    fn the_pass_stencil_follows_the_items_switches() {
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        let mode1 = StencilState {
            enable: true,
            func: CompareFunc::Always,
            reference: 0,
            read_mask: 0xff,
            write_mask: 0xff,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Replace,
        };
        let other = StencilState { enable: true, func: CompareFunc::Equal, reference: 3, ..mode1 };
        let none = StencilPlan::default();
        assert_eq!(pass_stencil(&other, &none), Some((other, vec![])), "no switch: the state as it is");
        let marked = StencilPlan { off: false, marked: vec![Owner::Cluster(4)], switches: true };
        assert_eq!(pass_stencil(&mode1, &marked), Some((mode1, vec![Owner::Cluster(4)])));
        assert_eq!(pass_stencil(&other, &marked), None, "a switch leaves mode 1, which the layer did not start in");
        let object = StencilPlan { switches: true, ..StencilPlan::default() };
        assert_eq!(pass_stencil(&mode1, &object), Some((mode1, vec![])), "an object's switch: mode 1 throughout");
        let off = StencilPlan { off: true, marked: vec![], switches: true };
        let (state, marked) = pass_stencil(&mode1, &off).unwrap();
        assert!(!state.enable && marked.is_empty(), "mode 0: the stencil off");
    }

    /// The device's stencil state as `0x00015370` sets it for `mode`
    /// (`engine::render::stencil::for_mode`); mode 0 only clears the
    /// enable, over `before`.
    fn device_mode(mode: u16, before: gfx::state::StencilState) -> gfx::state::StencilState {
        use engine::render::stencil::{Compare, Op, for_mode};
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        let op = |o| match o {
            Op::Keep => StencilOp::Keep,
            Op::Replace => StencilOp::Replace,
        };
        match for_mode(mode).expect("a mode the helper takes") {
            None => StencilState { enable: false, ..before },
            Some(s) => StencilState {
                enable: s.enable,
                func: match s.func {
                    Compare::Equal => CompareFunc::Equal,
                    Compare::NotEqual => CompareFunc::NotEqual,
                    Compare::Always => CompareFunc::Always,
                },
                reference: s.reference,
                read_mask: s.mask,
                write_mask: s.write_mask,
                fail: op(s.fail),
                depth_fail: op(s.depth_fail),
                pass: op(s.pass),
            },
        }
    }

    /// The stencil value a pixel keeps after the native pass draws
    /// `owner`'s part over `stored` (depth passing), as the pass's
    /// pipeline evaluates it: the device's state translated
    /// (`PassStencil::from_device`, a D24S8 depth) with the owner's
    /// reference (`PassStencil::reference_of`), `WorldPass::encode`'s.
    fn native_write(state: &gfx::state::StencilState, marked: &[Owner], owner: Owner, stored: u8) -> u8 {
        let mut s = PassStencil::from_device(state, wgpu::TextureFormat::Depth24PlusStencil8);
        if s == PassStencil::default() {
            return stored;
        }
        s.marked = marked.to_vec();
        let reference = s.reference_of(owner) as u8;
        let face = s.state.front;
        let (read, write) = (s.state.read_mask as u8, s.state.write_mask as u8);
        let passes = match face.compare {
            wgpu::CompareFunction::Always => true,
            wgpu::CompareFunction::Equal => reference & read == stored & read,
            wgpu::CompareFunction::NotEqual => reference & read != stored & read,
            other => panic!("not a compare the device's modes use: {other:?}"),
        };
        let op = if passes { face.pass_op } else { face.fail_op };
        match op {
            wgpu::StencilOperation::Keep => stored,
            wgpu::StencilOperation::Replace => (stored & !write) | (reference & write),
            other => panic!("not an op the device's modes use: {other:?}"),
        }
    }

    /// What the game's layer 3 leaves in the stencil where a section's
    /// item draws (`0x0004d640`, the draw, `0x0004d720`), against what the
    /// native pass writes for its part, over every stored value; and the
    /// device's state after the layer, which the native pass does not set
    /// (the D3D items left in the layer and the passes after it see the
    /// state the layer started in): the same as the game's after its last
    /// switching item, mode 1.
    #[test]
    fn the_native_stencil_writes_and_leaves_what_the_game_s_layer_3_does() {
        let mode1 = device_mode(1, gfx::state::StencilState::default());
        let marked = Owner::Cluster(4);
        let plain = [Owner::Cluster(1), Owner::Instance(2)];
        let cases = [
            // (the layer's plan, the game's mode for the marked section and
            // for the rest)
            (StencilPlan { off: false, marked: vec![marked], switches: true }, 2, 1),
            (StencilPlan { switches: true, ..StencilPlan::default() }, 1, 1),
            (StencilPlan { off: true, marked: vec![], switches: true }, 0, 0),
            (StencilPlan::default(), 1, 1),
        ];
        for (plan, marked_mode, plain_mode) in cases {
            let (state, sections) = pass_stencil(&mode1, &plan).expect("mode 1 at the layer's start: drawn");
            for stored in 0..=255u8 {
                for (owner, mode) in std::iter::once((marked, marked_mode)).chain(plain.iter().map(|&o| (o, plain_mode))) {
                    let game = device_mode(mode, mode1);
                    let expect = if game.enable {
                        let s = engine::render::stencil::for_mode(mode).unwrap().unwrap();
                        if s.passes(stored) { s.after_pass(stored) } else { stored }
                    } else {
                        stored
                    };
                    assert_eq!(
                        native_write(&state, &sections, owner, stored),
                        expect,
                        "{plan:?}: {owner:?} over {stored:#x} (game mode {mode})"
                    );
                }
            }
            // After the layer: the game's switching items end in mode 1
            // (0x0004d720), and without a switch the state is the start's.
            // The native pass sets no device state, so the device holds
            // `mode1`, the start: equal either way.
            let game_after = if plan.switches { device_mode(1, mode1) } else { mode1 };
            assert_eq!(game_after, mode1, "{plan:?}");
        }
        // Mode 2's write is 0xff, mode 1's 0: the reference alone differs.
        assert_eq!(device_mode(2, mode1), gfx::state::StencilState { reference: PassStencil::MARK as u8, ..mode1 });
    }

    /// A 2x2 level and its 1x1 mip as `assets` decodes a P8 image: the
    /// index in every channel.
    fn indexed(indices: [u8; 4]) -> Vec<assets::Level> {
        let level = |level, width, idx: &[u8]| assets::Level {
            face: 0,
            level,
            width,
            height: width,
            depth: 1,
            rgba: idx.iter().flat_map(|&i| [i, i, i, 0xff]).collect(),
        };
        vec![level(1, 1, &indices[..1]), level(0, 2, &indices)]
    }

    /// A palette of tangent-space normals packed as colours, `A8R8G8B8`.
    fn normals() -> Vec<u32> {
        (0..256u32).map(|i| 0xff00_0000 | ((i ^ 0x80) << 16) | ((0xff - i) << 8) | 0xfe).collect()
    }

    #[test]
    fn a_palettized_bump_map_samples_its_stage_palette_not_its_index() {
        // The 03a A/B: a P8 bump map sampled as its index (grey) gave the
        // bump term a wrong normal, so `r0` clamped to black on most walls
        // and over-brightened others. The stage samples the palette entry,
        // as the D3D path does through the palette bound at that stage.
        let (image, expanded) = stage_image(tags::PixelFormat::P8Bump, indexed([0, 1, 0x80, 0xff]), 2, &Some(normals())).unwrap();
        assert!(expanded);
        assert_eq!((image.width, image.height), (2, 2));
        let texel = |i: usize| <[u8; 4]>::try_from(&image.rgba[4 * i..4 * i + 4]).unwrap();
        assert_eq!(texel(0), [0x80, 0xff, 0xfe, 0xff]);
        assert_eq!(texel(1), [0x81, 0xfe, 0xfe, 0xff]);
        assert_eq!(texel(2), [0x00, 0x7f, 0xfe, 0xff]);
        assert_eq!(texel(3), [0x7f, 0x00, 0xfe, 0xff]);
        // The mip goes through the same palette.
        assert_eq!(image.mips, vec![vec![0x80, 0xff, 0xfe, 0xff]]);
        // Plain P8 the same.
        assert!(stage_image(tags::PixelFormat::P8, indexed([0; 4]), 0, &Some(normals())).unwrap().1);
    }

    #[test]
    fn a_palettized_stage_without_its_palette_stays_on_the_d3d_path() {
        let p8 = tags::PixelFormat::P8Bump;
        let palette = |e: StageRefusal| matches!(e, StageRefusal::Here(_));
        assert!(palette(stage_image(p8, indexed([0; 4]), 1, &None).unwrap_err()));
        // Stage 3 holds each lightmap's palette.
        assert!(palette(stage_image(p8, indexed([0; 4]), 3, &Some(normals())).unwrap_err()));
        // A palette of fewer entries than the indices can reach.
        assert!(palette(stage_image(p8, indexed([0; 4]), 0, &Some(normals()[..32].to_vec())).unwrap_err()));
        // A direct format keeps its colours and needs no palette.
        let (image, expanded) = stage_image(tags::PixelFormat::Argb8888, indexed([9, 8, 7, 6]), 3, &None).unwrap();
        assert!(!expanded);
        assert_eq!(&image.rgba[..4], &[9, 9, 9, 0xff]);
    }

    #[test]
    fn a_world_draws_only_while_the_palettes_it_expanded_through_are_bound() {
        let used: StagePalettes = [None, None, Some(normals()), None];
        assert!(palettes_hold(&used, &|s| (s == 2).then(normals)));
        // Other stages are not looked at.
        assert!(palettes_hold(&used, &|s| if s == 2 { Some(normals()) } else { Some(vec![1]) }));
        assert!(!palettes_hold(&used, &|_| None));
        let mut other = normals();
        other[5] = 0;
        assert!(!palettes_hold(&used, &|_| Some(other.clone())));
        assert!(palettes_hold(&StagePalettes::default(), &|_| None));
    }

    #[test]
    fn passes_share_a_palettized_image_only_through_the_same_stage_palette() {
        // Stages 1 and 2 hold the default palette, stage 0 none: as at
        // 03a's layer 3. The world's pass expands a P8 bump map at stage
        // 2; a transparent pass (sharing the decoder and its images)
        // takes it at stage 1, and is refused it at stage 0 and 3.
        let mut d = StageDecoder::new([None, Some(normals()), Some(normals()), Some(normals())]);
        let mut images = StageImages::new();
        let bump = (TagId(0xe000_0040), 0);
        let mut decodes = 0;
        let p8 = |stage: usize| {
            move |palette: &Option<Vec<u32>>| stage_image(tags::PixelFormat::P8Bump, indexed([1, 2, 3, 4]), stage, palette)
        };
        assert_eq!(d.take(&mut images, bump, 2, &mut |pal| { decodes += 1; p8(2)(pal) }), Ok(true));
        assert_eq!(images[&bump].rgba[..4], [0x81, 0xfe, 0xfe, 0xff], "palette entry 1, not grey 1");
        assert_eq!(d.used(), &[None, None, Some(normals()), None]);
        assert_eq!(d.take(&mut images, bump, 1, &mut |pal| { decodes += 1; p8(1)(pal) }), Ok(true));
        assert_eq!(decodes, 1, "expanded once");
        assert!(d.take(&mut images, bump, 0, &mut |pal| p8(0)(pal)).is_err(), "no palette at stage 0");
        assert!(d.take(&mut images, bump, 3, &mut |pal| p8(3)(pal)).is_err(), "the lightmap's stage");

        // A transparent's own palettized bitmap at a stage without a
        // palette is refused, and not cached as failed: it still expands
        // at a stage that has one.
        let other = (TagId(0xe000_0041), 0);
        let err = d.take(&mut images, other, 0, &mut |pal| p8(0)(pal)).unwrap_err();
        assert!(err.starts_with("stage 0 bitmap is palettized"), "{err}");
        assert!(!images.contains_key(&other));
        assert_eq!(d.take(&mut images, other, 1, &mut |pal| p8(1)(pal)), Ok(true));
        assert_eq!(d.used()[1], Some(normals()));

        // A direct format is no palette's business; an image that does
        // not decode fails at every stage after.
        let plain = (TagId(0xe000_0042), 0);
        let argb = |pal: &Option<Vec<u32>>| stage_image(tags::PixelFormat::Argb8888, indexed([5; 4]), 0, pal);
        assert_eq!(d.take(&mut images, plain, 0, &mut |pal| argb(pal)), Ok(false));
        let broken = (TagId(0xe000_0043), 0);
        assert!(d.take(&mut images, broken, 1, &mut |_| Err(StageRefusal::Image("does not decode"))).is_err());
        let err = d.take(&mut images, broken, 2, &mut |_| unreachable!("cached as failed")).unwrap_err();
        assert_eq!(err, "stage 2 bitmap does not decode");
    }
}
