//! The native world's host (docs/graphics/native-renderer.md, stage 1):
//! loads the active BSP's world from the loaded map's file when the BSP
//! changes, on a worker, and asks the D3D device for the native pass at
//! each view's layer 3 (`feature::native_world`). Inert unless
//! `impl.render.native_world` is on (nothing asks).
//!
//! - **Load once.** On a new (map, BSP) the worker opens the map through
//!   the VFS ([`set_locator`]), finds the scenario BSP whose tag data sits
//!   at the active BSP's address, builds the world (`native_render::load`),
//!   and decodes each material's base map and each lightmap page. The GPU
//!   side is made on the render thread by the first pass. Until it is
//!   ready, or when it fails, the D3D path draws the world.
//! - **Per frame.** The camera and one external pass; the device records it
//!   after the next drain, in order with the draws.
//! - **Fallback.** A material whose base map does not decode is left out
//!   of the world and keeps its D3D draws (the skip only clears items of
//!   the shaders the world draws).
//! - **Lights** (`impl.render.native_lights`): the lights' part of the
//!   world, loaded with it (`native_lights_glue::load`).
//! - **Material programs** (`impl.render.native_world_programs`, off by
//!   default). A native material whose layer-3 pass translates
//!   (`native_render::look`, `native_render::program`) draws with its own
//!   vertex program, pixel shader, stage textures and constants, and the
//!   device's view constants each frame; the others keep stage 1's base
//!   map times lightmap.

use std::collections::HashMap;
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
        let ready = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.0.as_ref() != Some(&key) {
                *st = (Some(key.clone()), Load::Loading);
                let state = Arc::clone(&self.state);
                std::thread::Builder::new()
                    .name("native-world load".into())
                    .spawn(move || {
                        let t0 = std::time::Instant::now();
                        let out = load_ready(&key.0, key.1);
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        if st.0.as_ref() != Some(&key) {
                            return;
                        }
                        st.1 = match out {
                            Ok(r) => {
                                eprintln!(
                                    "native-world: {} BSP at {:#010x}: {} draws in {} batches, {} materials native, in {:.1} s",
                                    key.0,
                                    key.1,
                                    r.cpu.lock().unwrap().as_ref().map_or(0, |c| c.world.draws.len()),
                                    r.cpu.lock().unwrap().as_ref().map_or(0, |c| c.world.batches.len()),
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
            match &st.1 {
                Load::Ready(r) => Arc::clone(r),
                Load::Idle | Load::Loading | Load::Failed => return None,
            }
        };
        let mut d = self.device.lock().ok()?;
        if !d.can_external_pass() {
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
        stage_images,
        transparents,
    } = cpu;
    let mut g = GpuWorld::new(device, queue, world, &mut |m| base_maps.remove(&m), &mut |k| lightmaps.get(&k).cloned());
    let mut stage = |s: Stage| match s {
        Stage::Bitmap { bitmap, image } => stage_images.get(&(bitmap, image)).cloned(),
        Stage::Empty | Stage::Lightmap => None,
    };
    if !programs.is_empty() {
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
        if let Some(constants) = self.view
            && gpu.program_count() > 0
        {
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
fn load_ready(name: &str, bsp: u32) -> Result<Ready, load::Error> {
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

    // Base maps: a material without one stays on the D3D path.
    let mut base_maps = HashMap::new();
    let mut native = ShaderSet::new();
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
            native.insert(shader.0 as u16);
        }
    }
    // Lightmap pages through their palettes.
    let mut lightmaps = HashMap::new();
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
            let Some(l) = d.levels.into_iter().find(|l| l.face == 0 && l.level == 0) else { continue };
            // P8 decodes as L8: the index in every channel.
            let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|p| p[0]).collect();
            let Some(page) = Image::level(l.width, l.height, load::expand_page(&indices, palette)) else { continue };
            lightmaps.insert(key, page);
        }
    }
    // Material programs: each native material's own layer-3 pass where it
    // translates, with its stages' images.
    let programs_on = feature::Switches::from_env().is_ok_and(|s| feature::native_world::programs_on(&s));
    let (programs, mut stage_images) = if programs_on {
        material_programs(&mut map, &mut files, &shaders, &base_maps)
    } else {
        Default::default()
    };
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
        crate::native_lights_glue::load(&mut map, &mut files, &shaders, &world, &|m| base_maps.contains_key(&m)).map(Arc::new)
    } else {
        None
    };
    // Draw only what a native material owns: the rest keeps its D3D draws.
    let world = keep_materials(world, &base_maps);
    Ok(Ready {
        cpu: Mutex::new(Some(CpuWorld {
            world,
            base_maps,
            lightmaps,
            programs,
            stage_images,
            transparents,
        })),
        gpu: Mutex::new(None),
        shaders: Arc::new(native),
        transparent_parts: Arc::new(transparent_parts),
        lights,
    })
}

/// The world without the draws of materials left to the D3D path.
fn keep_materials(mut world: World, native: &HashMap<u16, Image>) -> World {
    world.draws.retain(|d| native.contains_key(&d.material));
    world.batches.retain(|b| native.contains_key(&b.material));
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

/// The materials of `native` (by BSP material index; `shaders` the BSP's
/// shader per material) whose layer-3 pass translates, with the images
/// their stages sample. Says on stderr how many stay with stage 1 and why.
fn material_programs<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    native: &HashMap<u16, Image>,
) -> (HashMap<u16, MaterialProgram>, StageImages) {
    let mut programs = HashMap::new();
    let mut images = StageImages::new();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("native-world: material programs: the shader tags do not read ({e}); stage 1 draws every material");
            return (programs, images);
        }
    };
    let mut refused: HashMap<String, usize> = HashMap::new();
    let mut materials: Vec<u16> = native.keys().copied().collect();
    materials.sort_unstable();
    for m in materials {
        let Some(&shader) = shaders.get(usize::from(m)) else { continue };
        let compiled = lib
            .material(shader, None)
            .map_err(|e| format!("material: {e}"))
            .and_then(|mat| look::layer3_look(&mat).map_err(|r| format!("{r:?}")))
            .and_then(|pass| {
                let code = look::world_program(map, lib.metadata(), pass.vertex_shader).ok_or("no world-space vertex program")?;
                program::compile(&pass, &code)
            });
        match compiled {
            Ok(p) => {
                for (n, s) in p.stages.iter().enumerate() {
                    if let Stage::Bitmap { bitmap, image } = *s
                        && p.dims[n].is_some()
                        && !images.contains_key(&(bitmap, image))
                        && let Some(i) = decode_image(map, files, bitmap, image as usize)
                    {
                        images.insert((bitmap, image), i);
                    }
                }
                programs.insert(m, p);
            }
            Err(why) => *refused.entry(why).or_default() += 1,
        }
    }
    let mut why: Vec<_> = refused.into_iter().collect();
    why.sort();
    eprintln!("native-world: {} materials translate; with stage 1: {why:?}", programs.len());
    (programs, images)
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
        let native: HashMap<u16, Image> = [(2, Image::solid([1; 4])), (3, Image::solid([1; 4]))].into_iter().collect();
        let w = keep_materials(b.finish(), &native);
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(2, 0, 2), (3, 2, 1)]);
        assert_eq!(w.draws.iter().map(|d| (d.material, d.batch)).collect::<Vec<_>>(), vec![(2, 0), (2, 0), (3, 1)]);
    }
}
