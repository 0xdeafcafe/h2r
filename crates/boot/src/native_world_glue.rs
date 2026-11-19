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
//!   D3D draws (the skip only clears items of the shaders the world draws,
//!   and a shader only when all its materials are native). The load says
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
use feature::native_world::{Host, ShaderSet};
use map::{Map, ResourceFile, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::load::{self, LoadedWorld};
use native_render::look::{self, Stage};
use native_render::program::{self, MaterialProgram, ProgramSet, ViewInputs};
use native_render::transparent_gpu::{TransparentProgram, TransparentSet};
use native_render::world::{LightmapKey, Owner, World};

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
    shaders: Arc<ShaderSet>,
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
pub(crate) type State = Arc<Mutex<(Option<(String, u32)>, Load)>>;

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
    fn ask(&self, mem: &dyn GuestMemory, bsp: u32, camera: &RenderCamera) -> Option<Arc<ShaderSet>> {
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() || bsp == 0 {
            return None;
        }
        let key = (name, bsp);
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
                        let out = load_ready(&key.0, key.1, palettes);
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
                                    "native-world: {} BSP at {:#010x}: {} draws in {} batches, {} materials native, in {:.1} s",
                                    key.0,
                                    key.1,
                                    draws,
                                    batches,
                                    r.shaders.len(),
                                    t0.elapsed().as_secs_f32()
                                );
                                Load::Ready(Arc::new(r))
                            }
                            Err(e) => {
                                eprintln!("native-world: {} BSP at {:#010x}: not loaded ({e}); the D3D path draws it", key.0, key.1);
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
        // The palettes the load expanded its palettized stage bitmaps
        // through must still be the ones bound: otherwise the D3D path
        // draws the world (its samples would differ).
        if !palettes_hold(&ready.palettes, &|s| d.stage_palette(&ReadMemory(mem), s)) {
            if !PALETTE_CHANGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-world: a stage palette changed since the load expanded its bitmaps through it; the D3D path draws the world");
            }
            return None;
        }
        d.request_external_pass(Box::new(WorldPass {
            ready: Arc::clone(&ready),
            camera: *camera,
            view: None,
        }));
        Some(Arc::clone(&ready.shaders))
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
        eprintln!("native-world: {} materials draw with their own programs", set.len());
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
}

impl d3d::ExternalPass for WorldPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut gpu = upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return };
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
            gpu.prepare_programs(
                t.queue,
                &ViewInputs {
                    constants,
                    guest_size: t.guest_size,
                    size: t.size,
                    z_scale: t.z_scale,
                },
            );
        }
        gpu.prepare(
            t.queue,
            &self.camera,
            DepthRule {
                full_scale: t.z_scale,
                floor: true,
            },
        );
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

/// Said once: a pass without view constants, batches left out.
static NO_VIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static UNBOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PALETTE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The device palette at each texture stage (`A8R8G8B8` entries), or
/// `None`: what a palettized stage bitmap (P8, P8 bump) is sampled through
/// on the D3D path. A world keeps those of the stages it expanded
/// bitmaps through.
pub type StagePalettes = [Option<Vec<u32>>; 4];

/// The palettes bound at stages 0..3 now.
fn stage_palettes<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> StagePalettes {
    std::array::from_fn(|s| d.stage_palette(&ReadMemory(mem), s))
}

/// Whether each palette a world expanded through is still the one bound
/// at its stage (`bound`).
fn palettes_hold(used: &StagePalettes, bound: &dyn Fn(usize) -> Option<Vec<u32>>) -> bool {
    used.iter().enumerate().all(|(s, p)| p.is_none() || bound(s) == *p)
}

/// The hook's guest memory as the device reads it, read-only.
struct ReadMemory<'a>(&'a dyn GuestMemory);

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
fn load_ready(name: &str, bsp: u32, palettes: StagePalettes) -> Result<Ready, load::Error> {
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
    let index = bsps
        .iter()
        .position(|(id, _)| map.tag(*id).ok().and_then(|t| t.location).is_some_and(|l| l.address == bsp))
        .ok_or("no scenario BSP at the active BSP's address")?;
    let mut raw = Raw(files);
    let LoadedWorld {
        world,
        shaders,
        lightmap_bitmap,
        palettes,
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
            let (Some(image), Some(palette)) = (b.images.get(usize::from(key.page)), palettes.get(usize::from(key.palette))) else {
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
    let mut used_palettes = StagePalettes::default();
    let mut palettized = Vec::new();
    let (mut programs, mut stage_images, refused) = if programs_on {
        let out = material_programs(&mut map, &mut files, &shaders, &world, &lightmaps, &palettes);
        native.extend(out.programs.keys().copied());
        used_palettes = out.palettes;
        palettized = out.palettized;
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
    // The game's skip is per shader: a shader is skipped only when every
    // material of the world that uses it is native.
    let (native, shader_set, shared) = native_shaders(&world.materials(), &shaders, &native);
    programs.retain(|m, _| native.contains(m));
    base_maps.retain(|m, _| native.contains(m));
    if programs_on {
        log_coverage(&mut map, &world, &shaders, &native, refused, &shared);
        palettized.retain(|m| native.contains(m));
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
        crate::native_transparents_glue::load(&mut map, &mut files, &shaders, &world, &lightmaps, &mut stage_images)
    } else {
        Default::default()
    };
    // The lights' part, over the whole world: the parts of materials left
    // to D3D are where native lights cannot draw.
    let lights_on = feature::Switches::from_env().is_ok_and(|s| feature::native_lights::on(&s));
    let lights = if lights_on {
        crate::native_lights_glue::load(&mut map, &mut files, &shaders, &world, &|m| native.contains(&m)).map(Arc::new)
    } else {
        None
    };
    // Draw only what a native material owns: the rest keeps its D3D draws.
    let world = keep_materials(world, &native);
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
        shaders: Arc::new(shader_set),
        transparent_parts: Arc::new(transparent_parts),
        lights,
        palettes: used_palettes,
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
/// shader of each), those drawn natively and the shaders the skip clears:
/// a shader only when every one of the world's materials that uses it is
/// in `native`, since the game skips by shader. Also the materials left
/// out for sharing a shader with one that is not native.
fn native_shaders(materials: &[u16], shaders: &[TagId], native: &HashSet<u16>) -> (HashSet<u16>, ShaderSet, Vec<u16>) {
    let shader = |m: u16| shaders.get(usize::from(m)).map(|s| s.0 as u16);
    let refused: HashSet<u16> = materials
        .iter()
        .filter(|&&m| !native.contains(&m))
        .filter_map(|&m| shader(m))
        .collect();
    let mut kept = HashSet::new();
    let mut set = ShaderSet::new();
    let mut shared = Vec::new();
    for &m in materials.iter().filter(|&&m| native.contains(&m)) {
        let Some(s) = shader(m) else { continue };
        if refused.contains(&s) {
            shared.push(m);
        } else {
            kept.insert(m);
            set.insert(s);
        }
    }
    (kept, set, shared)
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
    mut refused: Refused,
    shared: &[u16],
) {
    if !shared.is_empty() {
        refused.insert("shares its shader with a material left to D3D".into(), shared.to_vec());
    }
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

/// The world without the draws of materials left to the D3D path.
fn keep_materials(mut world: World, native: &HashSet<u16>) -> World {
    world.draws.retain(|d| native.contains(&d.material));
    world.batches.retain(|b| native.contains(&b.material));
    let mut first = 0;
    for b in &mut world.batches {
        b.first_draw = first;
        first += b.draw_count;
    }
    for (bi, b) in world.batches.iter().enumerate() {
        for d in &mut world.draws[b.first_draw as usize..(b.first_draw + b.draw_count) as usize] {
            d.batch = bi as u32;
        }
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
    /// The palettes palettized stage bitmaps were expanded through, at
    /// their stages.
    palettes: StagePalettes,
    /// Materials with a palettized stage bitmap.
    palettized: Vec<u16>,
}

/// The world's materials whose layer-3 pass translates and whose every
/// part the native pass can draw with it (`program::compile_world`,
/// `program::world_fit`), with the images their stages sample; and the
/// others by the reason they stay on the D3D path. A palettized stage
/// bitmap is expanded through `palettes` at its stage ([`stage_image`]).
fn material_programs<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    palettes: &StagePalettes,
) -> Programs {
    let mut out = Programs::default();
    let Programs {
        programs,
        images,
        refused,
        palettes: used,
        palettized,
    } = &mut out;
    let materials = world.materials();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            refused.insert(format!("the shader tags do not read ({e})"), materials);
            return out;
        }
    };
    let mut failed: HashSet<(TagId, u32)> = HashSet::new();
    // The stage each palettized image was expanded at.
    let mut expanded_at: HashMap<(TagId, u32), usize> = HashMap::new();
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
                program::compile_world(&pass, &code)
            })
            .and_then(|p| {
                program::world_fit(world, m, &p, &|k: LightmapKey| lightmaps.contains_key(&k))?;
                for (n, s) in p.stages.iter().enumerate() {
                    let Stage::Bitmap { bitmap, image } = *s else { continue };
                    if p.dims[n].is_none() {
                        continue;
                    }
                    if let Some(&at) = expanded_at.get(&(bitmap, image)) {
                        // One expansion per image: another stage must hold
                        // the same palette.
                        if n == 3 || palettes[n] != palettes[at] {
                            return Err(format!("stage {n} bitmap is palettized and was expanded through another stage's palette"));
                        }
                        uses_palette = true;
                        continue;
                    }
                    if images.contains_key(&(bitmap, image)) {
                        continue;
                    }
                    if failed.contains(&(bitmap, image)) {
                        return Err(format!("stage {n} bitmap does not decode"));
                    }
                    match decode_stage(map, files, bitmap, image as usize, n, &palettes[n]) {
                        Ok((i, expanded)) => {
                            images.insert((bitmap, image), i);
                            if expanded {
                                expanded_at.insert((bitmap, image), n);
                                used[n].clone_from(&palettes[n]);
                                uses_palette = true;
                            }
                        }
                        Err(StageRefusal::Image(why)) => {
                            failed.insert((bitmap, image));
                            return Err(format!("stage {n} bitmap {why}"));
                        }
                        // Not the image's fault: it may do at another
                        // stage.
                        Err(StageRefusal::Palette(why)) => return Err(format!("stage {n} bitmap {why}")),
                    }
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

/// Why a stage cannot have a bitmap's image: the image itself, or (only at
/// this stage) its palette.
#[derive(Debug, PartialEq, Eq)]
enum StageRefusal {
    Image(&'static str),
    Palette(&'static str),
}

/// Image `index` of a stage's bitmap at stage `stage`, and whether it was
/// expanded through the stage's palette; or why the stage cannot have it:
/// it does not decode, it is a linear texture (addressed in texels on the
/// D3D path, `tex_scale`, where the programs here take normalised
/// coordinates), or it is palettized and the stage has no palette for it
/// ([`stage_image`]).
fn decode_stage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool), StageRefusal> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).map_err(|_| StageRefusal::Image("does not read"))?.data else {
        return Err(StageRefusal::Image("is not a bitmap"));
    };
    let image = b.images.get(index).ok_or(StageRefusal::Image("has no such image"))?;
    let d = assets::decode_image(image, files).map_err(|_| StageRefusal::Image("does not decode"))?;
    if d.desc.format.layout() == gfx::format::Layout::Linear {
        return Err(StageRefusal::Image("is a linear texture"));
    }
    stage_image(image.format, d.levels, stage, palette)
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
            return Err(StageRefusal::Palette("is palettized at stage 3 (the lightmap's palette is bound there)"));
        }
        let p = palette
            .as_ref()
            .ok_or(StageRefusal::Palette("is palettized and no palette is bound at its stage"))?;
        Some(
            p.as_slice()
                .try_into()
                .map_err(|_| StageRefusal::Palette("is palettized and its stage's palette has fewer than 256 entries"))?,
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
        let w = keep_materials(b.finish(), &native);
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(2, 0, 2), (3, 2, 1)]);
        assert_eq!(w.draws.iter().map(|d| (d.material, d.batch)).collect::<Vec<_>>(), vec![(2, 0), (2, 0), (3, 1)]);
    }

    #[test]
    fn a_shader_is_skipped_only_when_all_its_materials_are_native() {
        // Materials 0..4; 1 and 3 share shader 0xe000_0011. 1 translates,
        // 3 does not: the shader stays on D3D, so 1 does too (its items
        // carry the same shader and the skip goes by shader).
        let shaders = [0xe000_0010, 0xe000_0011, 0xe000_0012, 0xe000_0011, 0xe000_0014].map(TagId);
        let translated: HashSet<u16> = [0, 1, 2].into_iter().collect();
        let (native, set, shared) = native_shaders(&[0, 1, 2, 3, 4], &shaders, &translated);
        assert_eq!(native, [0, 2].into_iter().collect::<HashSet<u16>>());
        assert_eq!(set, [0x10, 0x12].into_iter().collect::<ShaderSet>(), "by tag index");
        assert_eq!(shared, vec![1]);
        // A material without a shader is never native.
        let (native, _, _) = native_shaders(&[0, 9], &shaders, &[0, 9].into_iter().collect());
        assert_eq!(native, [0].into_iter().collect::<HashSet<u16>>());
    }

    #[test]
    fn an_untranslated_material_keeps_its_d3d_draws_and_a_translated_one_is_skipped() {
        // The world as the loader leaves it with programs on: material 7
        // translated, material 8 refused. 8 is neither drawn natively nor
        // skipped, so the D3D path still draws it; 7 is drawn natively
        // and its shader is skipped.
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
        b.add_section(Owner::Cluster(0), &tri(7), &Placement::IDENTITY, None);
        b.add_section(Owner::Cluster(1), &tri(8), &Placement::IDENTITY, None);
        let world = b.finish();
        let mut shaders = vec![TagId(0xe000_0000); 9];
        shaders[7] = TagId(0xe000_0107);
        shaders[8] = TagId(0xe000_0108);
        let translated: HashSet<u16> = [7].into_iter().collect();
        let (native, set, _) = native_shaders(&world.materials(), &shaders, &translated);
        let w = keep_materials(world, &native);
        assert!(w.draws.iter().all(|d| d.material == 7), "8 is not drawn natively (no stage 1 look)");
        assert!(set.contains(&0x107) && !set.contains(&0x108), "only 7's shader is skipped");
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
        let palette = |e: StageRefusal| matches!(e, StageRefusal::Palette(_));
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
}
