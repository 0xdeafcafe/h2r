//! The native lights' host (docs/graphics/native-renderer.md, "Stage 4:
//! lights and shadows"): the world's lit passes for unshadowed lights.
//! Inert unless `impl.render.native_lights` is on: the hooks are not
//! placed (so `feature::native_lights::light_constants` never runs) and
//! nothing is loaded for lights. It needs the native world on, which
//! loads the BSP.
//!
//! - **At load** ([`load`], on the world's worker, before the world drops
//!   the materials left to D3D): each world material's shader and
//!   instance inputs, the vertex programs and stage images of every
//!   implementation of its layer-12 and layer-13 passes, the light
//!   textures (`matg` rasterizer data), the gels of the map's `ligh`
//!   tags, the opaque parts with their index ranges, and the bounds of
//!   world surfaces the native pass cannot light (materials left to D3D,
//!   transparents with lit layers).
//! - **Per light** ([`LightsWorld::plan`]): the parts whose bounds meet
//!   the light's sphere, by material; for each the implementation the
//!   engine would take (`engine::light_passes::implementation`), the
//!   material resolved with the light's inputs, `light_look` and
//!   `compile_light`, cached by inputs (`LookCache`). A light that meets a
//!   surface it cannot light, or whose pass is refused, keeps its D3D
//!   passes.
//! - [`Host::accepts`]: [`drawable`] and a plan over the loaded world.
//!   [`Host::open`]: one external pass per run, which takes the run's
//!   lights at its issue and draws them with `GpuWorld::encode_lights`.
//! - [`Host::tail`]: `0x00031170`'s tail on the device, in render order,
//!   through `d3d::Device::light_passes_tail` over the hook's guest memory
//!   (lock order as the D3D entries': the guest's memory, which the hook
//!   holds, then the device).
//! - [`Host::target`]: the device's bound target
//!   (`d3d::Device::bound_target`), which the feature records before the
//!   view's lights and compares at each light.
//!
//! Objects are not lit yet (stage 4's objects item): a native light's
//! passes over objects are skipped with the rest.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use feature::native_lights::Host;
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::Image;
use native_render::light_gpu::{LayeredImage, LightDraw, LightSet, TextureDim, light_gpu_key};
use native_render::light_look::{
    DIFFUSE_LAYER, LightInputs, LightProgram, LightRefusal, LightStage, LookCache, SPECULAR_LAYER, compile_light, engine_inputs, light_look,
};
use native_render::lights::{Light, Run};
use native_render::look;
use native_render::program::ViewInputs;
use native_render::world::{Aabb, World};
use tags::material::{EngineInputs, Material, ShaderLibrary, TextureSource};

use crate::native_world_glue::{Load, Ready, State, decode_image, upload};

/// The hook's guest memory as the device's.
pub struct DeviceMemory<'a>(pub &'a mut dyn GuestMemory);

fn guest_error(f: MemFault) -> d3d::GuestError {
    d3d::GuestError {
        addr: f.addr,
        len: f.len,
    }
}

impl d3d::GuestMemory for DeviceMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(guest_error)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        self.0.write(addr, data).map_err(guest_error)
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The loaded world, when it loaded its lights' part.
    fn ready(&self) -> Option<(Arc<Ready>, Arc<LightsWorld>)> {
        match &self.state.lock().unwrap_or_else(|e| e.into_inner()).1 {
            Load::Ready(r) => Some((Arc::clone(r), Arc::clone(r.lights.as_ref()?))),
            Load::Idle | Load::Loading | Load::Failed => None,
        }
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, light: &Light) -> bool {
        drawable(light) && self.ready().is_some_and(|(_, lights)| lights.plan(light).is_some())
    }

    fn open(&self, _camera: &RenderCamera, run: Arc<Run>) -> bool {
        let Some((ready, lights)) = self.ready() else { return false };
        let Ok(mut d) = self.device.lock() else { return false };
        if !d.can_external_pass() {
            return false;
        }
        d.request_external_pass(Box::new(RunPass {
            ready,
            lights,
            run,
            view: None,
            plans: Vec::new(),
        }));
        true
    }

    fn tail(&self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.light_passes_tail(&mut DeviceMemory(mem), clip).unwrap_or(false)
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

/// Whether the host may draw `light` natively: its constants computed at
/// the hook and no shadow (the shadow maps are not drawn natively, so a
/// shadowed light keeps all its D3D passes).
pub fn drawable(light: &Light) -> bool {
    light.constants.is_some_and(|c| !c.shadowed())
}

/// The `matg` tag block of rasterizer data (`+0x10c`, count then
/// address); `0x00022590` keeps its first element at `[0x00485a80]`.
pub const RASTERIZER_DATA: u32 = 0x10c;
/// The element's bitmap datums the light passes sample: engine texture 1
/// (`+0xc`, the normalization cube) and 9 (`+4`, the attenuation volume).
pub const NORMALIZATION_DATUM: u32 = 0xc;
pub const ATTENUATION_DATUM: u32 = 4;

/// The light textures' bitmaps named by the `matg` at `matg` (its payload
/// address), read through `read(address, len)`: `(normalization,
/// attenuation)`, each `None` when null or unreadable.
pub fn light_bitmaps(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(read(a, 4)?.try_into().ok()?)) };
    let element = || -> Option<u32> {
        let count = word(matg.wrapping_add(RASTERIZER_DATA))?;
        (count > 0).then_some(())?;
        word(matg.wrapping_add(RASTERIZER_DATA + 4))
    };
    let Some(e) = element() else { return (None, None) };
    let datum = |o: u32| word(e.wrapping_add(o)).filter(|&d| d != u32::MAX).map(map::TagId);
    (datum(NORMALIZATION_DATUM), datum(ATTENUATION_DATUM))
}

/// A decoded bitmap's first image as a cube (six faces of level 0) or a
/// volume (level 0's slices); `None` when it is not that shape.
pub fn layered(levels: Vec<assets::bitmap::Level>, dim: TextureDim) -> Option<LayeredImage> {
    let mut top: Vec<_> = levels.into_iter().filter(|l| l.level == 0).collect();
    match dim {
        TextureDim::Cube => LayeredImage::cube(top.into_iter().map(|l| (l.face, l.width, l.height, l.rgba)).collect()),
        TextureDim::D3 => {
            top.retain(|l| l.face == 0);
            let [l] = <[_; 1]>::try_from(top).ok()?;
            LayeredImage::volume(l.width, l.height, l.depth, l.rgba)
        }
        TextureDim::D2 => None,
    }
}

/// [`light_bitmaps`] of the map's `matg`, decoded: `(normalization cube,
/// attenuation volume)`. Logs what does not load (the light passes that
/// sample it stay on the D3D path).
pub(crate) fn light_textures<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    meta: &tags::shader::Metadata,
    files: &mut assets::ResourceFiles,
) -> (Option<LayeredImage>, Option<LayeredImage>) {
    let matg = map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address));
    let Some(matg) = matg else {
        eprintln!("native-lights: no matg; the light textures do not load");
        return (None, None);
    };
    let (n, a) = light_bitmaps(&|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec), matg);
    let mut decode = |id: Option<map::TagId>, dim, what| {
        let image = id.and_then(|id| {
            let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else {
                return None;
            };
            let d = assets::decode_image(b.images.first()?, files).ok()?;
            layered(d.levels, dim)
        });
        if image.is_none() {
            eprintln!("native-lights: the {what} ({id:?}) does not load");
        }
        image
    };
    (decode(n, TextureDim::Cube, "normalization cube"), decode(a, TextureDim::D3, "attenuation volume"))
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

/// An opaque world part a light may draw over: its material and where
/// its triangles are in the world's index buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LitPart {
    pub material: u16,
    pub bounds: Aabb,
    pub first_index: u32,
    pub index_count: u32,
}

/// The parts the sphere (`centre`, `radius`) meets, as index ranges by
/// material; `None` when it meets a surface in `elsewhere` (one the native
/// pass cannot light, so the light keeps its D3D passes).
pub fn parts_meeting(parts: &[LitPart], elsewhere: &[Aabb], centre: [f32; 3], radius: f32) -> Option<BTreeMap<u16, Vec<Range<u32>>>> {
    if elsewhere.iter().any(|b| b.meets_sphere(centre, radius)) {
        return None;
    }
    let mut out: BTreeMap<u16, Vec<Range<u32>>> = BTreeMap::new();
    for p in parts.iter().filter(|p| p.index_count > 0 && p.bounds.meets_sphere(centre, radius)) {
        out.entry(p.material).or_default().push(p.first_index..p.first_index + p.index_count);
    }
    Some(out)
}

/// A world material as the light passes resolve it.
#[derive(Clone, Debug)]
pub struct LitMaterial {
    pub shader: TagId,
    /// `EngineInputs::for_shader`: its integer input 4 picks layer 12's
    /// implementation 0 and layer 13's.
    pub instance: EngineInputs,
}

/// A light pass compiled for some inputs, with its GPU key.
#[derive(Debug)]
pub struct Compiled {
    pub program: LightProgram,
    pub key: u64,
}

/// What one (material, layer, implementation, inputs) gives.
#[derive(Clone, Debug)]
enum Lit {
    /// No entry: the template has no pass in the layer.
    Nothing,
    Draw(Arc<Compiled>),
    /// The light keeps its D3D passes (why, for the count).
    Refused(String),
}

/// One material's pass for one light: the program and the ranges.
#[derive(Clone, Debug)]
pub struct Planned {
    pub compiled: Arc<Compiled>,
    pub ranges: Vec<Range<u32>>,
}

/// What resolves light passes at run time: the map's shader library and
/// the passes resolved so far.
struct Resolver {
    lib: ShaderLibrary,
    cache: LookCache<Lit>,
    /// Refusals so far by reason (each said once on stderr).
    refused: HashMap<String, u64>,
}

/// Distinct (material, layer, implementation, inputs) kept resolved.
const CACHE_CAPACITY: usize = 512;

/// The lights' part of a loaded world ([`load`]).
pub struct LightsWorld {
    resolver: Mutex<Resolver>,
    materials: HashMap<u16, LitMaterial>,
    parts: Vec<LitPart>,
    /// Bounds of world surfaces the native pass cannot light.
    elsewhere: Vec<Aabb>,
    /// World-space vertex programs by `vrtx`.
    microcode: HashMap<TagId, Vec<u8>>,
    /// Stage images by (bitmap, image).
    images: HashMap<(TagId, u32), Image>,
    /// Gel bitmaps' image 0 by datum.
    gels: HashMap<u32, Image>,
    normalization: Option<LayeredImage>,
    attenuation: Option<LayeredImage>,
}

impl LightsWorld {
    /// What `light` draws over the world, pass by pass, or `None` when it
    /// must keep its D3D passes. Resolves (once per distinct inputs) under
    /// the resolver's lock, which nothing else is held across.
    pub fn plan(&self, light: &Light) -> Option<Vec<Planned>> {
        let c = light.constants.filter(|c| !c.shadowed())?;
        let (centre, radius) = light.bounds;
        let meeting = parts_meeting(&self.parts, &self.elsewhere, centre, radius)?;
        let inputs = LightInputs::from_constants(&c);
        let key = inputs.key();
        let mut out = Vec::new();
        let mut r = self.resolver.lock().unwrap_or_else(|e| e.into_inner());
        let Resolver { lib, cache, refused } = &mut *r;
        for (m, ranges) in meeting {
            let lit = self.materials.get(&m)?;
            let int_input = lit.instance.ints[4] as u16;
            for layer in [DIFFUSE_LAYER, SPECULAR_LAYER] {
                let Some(imp) = engine::light_passes::implementation(layer as u32, c.flags, c.definition_flags as u8, int_input) else {
                    continue;
                };
                let imp = imp as usize;
                let made = cache.get_or_make(lit.shader, layer, imp, key, || self.resolve(lib, lit, &inputs, layer, imp));
                match made {
                    Lit::Nothing => {}
                    Lit::Refused(why) => {
                        let n = refused.entry(why.clone()).or_insert(0);
                        *n += 1;
                        if *n == 1 {
                            eprintln!("native-lights: a light keeps its D3D passes: {why}");
                        }
                        return None;
                    }
                    Lit::Draw(compiled) => {
                        if !self.binds(&compiled.program, c.gel) {
                            return None;
                        }
                        out.push(Planned {
                            compiled,
                            ranges: ranges.clone(),
                        });
                    }
                }
            }
        }
        Some(out)
    }

    /// Resolves `lit` with the light's inputs and compiles its pass.
    fn resolve(&self, lib: &mut ShaderLibrary, lit: &LitMaterial, inputs: &LightInputs, layer: usize, imp: usize) -> Lit {
        let material = match lib.material(lit.shader, Some(engine_inputs(&lit.instance, inputs))) {
            Ok(m) => m,
            Err(e) => return Lit::Refused(format!("material: {e}")),
        };
        let look = match light_look(&material, layer, imp) {
            Ok(l) => l,
            Err(LightRefusal::NoLayer) => return Lit::Nothing,
            Err(r) => return Lit::Refused(format!("{r:?}")),
        };
        let Some(code) = self.microcode.get(&look.vertex_shader) else {
            return Lit::Refused("no world-space vertex program".into());
        };
        match compile_light(&look, code) {
            Ok(program) => {
                let key = light_gpu_key(&program);
                Lit::Draw(Arc::new(Compiled { program, key }))
            }
            Err(e) => Lit::Refused(e),
        }
    }

    /// Whether every texture `program` samples is loaded, with `gel` the
    /// light's.
    fn binds(&self, program: &LightProgram, gel: Option<u32>) -> bool {
        program.stages.iter().zip(&program.dims).all(|(s, d)| {
            d.is_none()
                || match *s {
                    LightStage::Empty => false,
                    LightStage::Bitmap { bitmap, image } => self.images.contains_key(&(bitmap, image)),
                    LightStage::Normalization => self.normalization.is_some(),
                    LightStage::Attenuation => self.attenuation.is_some(),
                    LightStage::Gel => gel.is_some_and(|g| self.gels.contains_key(&g)),
                }
        })
    }
}

/// Whether `material` draws anything in layer 12 or 13 (first level of
/// detail).
fn lit_layers(material: &Material) -> bool {
    material
        .layers
        .iter()
        .any(|l| l.lod == 0 && (l.layer == DIFFUSE_LAYER || l.layer == SPECULAR_LAYER) && !l.passes.is_empty())
}

/// At load, before the world drops the materials left to D3D: the lights'
/// part of `world` (`shaders` the BSP's shader per material, `native` the
/// materials the native world draws). `None` when the shader tags do not
/// read. Says on stderr what loaded.
pub(crate) fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    native: &dyn Fn(u16) -> bool,
) -> Option<LightsWorld> {
    let mut lib = match ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("native-lights: the shader tags do not read ({e}); every light keeps its D3D passes");
            return None;
        }
    };
    let mut materials = HashMap::new();
    let mut microcode = HashMap::new();
    let mut images = HashMap::new();
    // Each material once, with default inputs: what any light's pass may
    // need that does not depend on the light (programs, bitmaps).
    let mut lit_of: HashMap<u16, Option<bool>> = HashMap::new();
    let mut ids: Vec<u16> = world.materials();
    ids.extend(world.transparent_materials());
    ids.sort_unstable();
    ids.dedup();
    for m in ids {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            lit_of.insert(m, None);
            continue;
        };
        let Ok(instance) = lib.shader(shader).map(|s| EngineInputs::for_shader(&s)) else {
            lit_of.insert(m, None);
            continue;
        };
        let Ok(material) = lib.material(shader, None) else {
            lit_of.insert(m, None);
            continue;
        };
        let lit = lit_layers(&material);
        lit_of.insert(m, Some(lit));
        materials.insert(m, LitMaterial { shader, instance });
        if !lit || !native(m) {
            continue;
        }
        for imp in material
            .layers
            .iter()
            .filter(|l| l.lod == 0 && (l.layer == DIFFUSE_LAYER || l.layer == SPECULAR_LAYER))
            .flat_map(|l| &l.passes)
            .flat_map(|p| &p.implementations)
        {
            if let Some(vs) = imp.vertex_shader
                && !microcode.contains_key(&vs)
                && let Some(code) = look::world_program(map, lib.metadata(), vs)
            {
                microcode.insert(vs, code);
            }
            for t in imp.textures.iter().flatten() {
                if let TextureSource::Bitmap { bitmap, image, .. } = t.source
                    && !images.contains_key(&(bitmap, image))
                    && let Some(i) = decode_image(map, files, bitmap, image as usize)
                {
                    images.insert((bitmap, image), i);
                }
            }
        }
    }
    // Opaque parts of native materials are drawn; any other surface that
    // has (or may have) lit layers keeps the lights meeting it on D3D.
    let mut parts = Vec::new();
    let mut elsewhere = Vec::new();
    for d in &world.draws {
        match lit_of.get(&d.material).copied().flatten() {
            Some(true) if native(d.material) => parts.push(LitPart {
                material: d.material,
                bounds: d.bounds,
                first_index: d.first_index,
                index_count: d.index_count,
            }),
            Some(false) => {}
            _ => elsewhere.push(d.bounds),
        }
    }
    for t in &world.transparents {
        if lit_of.get(&t.material).copied().flatten() != Some(false) {
            elsewhere.push(t.bounds);
        }
    }
    let (normalization, attenuation) = light_textures(map, lib.metadata(), files);
    let gels = gels(map, &lib, files);
    eprintln!(
        "native-lights: {} lit parts, {} surfaces left to D3D, {} vertex programs, {} stage images, {} gels",
        parts.len(),
        elsewhere.len(),
        microcode.len(),
        images.len(),
        gels.len()
    );
    Some(LightsWorld {
        resolver: Mutex::new(Resolver {
            lib,
            cache: LookCache::new(CACHE_CAPACITY),
            refused: HashMap::new(),
        }),
        materials,
        parts,
        elsewhere,
        microcode,
        images,
        gels,
        normalization,
        attenuation,
    })
}

/// The gels of the map's light definitions (`ligh +0x7c`), image 0 each.
fn gels<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, lib: &ShaderLibrary, files: &mut assets::ResourceFiles) -> HashMap<u32, Image> {
    let datums: Vec<u32> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"ligh")
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .filter_map(|a| {
            let b = lib.metadata().bytes(a.wrapping_add(engine::light_passes::DEFINITION_GEL), 4).ok()?;
            Some(u32::from_le_bytes(b.try_into().ok()?))
        })
        .filter(|&d| d != u32::MAX)
        .collect();
    let mut out = HashMap::new();
    for d in datums {
        if !out.contains_key(&d)
            && let Some(i) = decode_image(map, files, TagId(d), 0)
        {
            out.insert(d, i);
        }
    }
    out
}

/// One run's pass: the run's lights planned at the issue, drawn over the
/// world at the encode.
struct RunPass {
    ready: Arc<Ready>,
    lights: Arc<LightsWorld>,
    run: Arc<Run>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    plans: Vec<(Light, Vec<Planned>)>,
}

impl d3d::ExternalPass for RunPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
        let lights = Arc::clone(&self.lights);
        self.plans = self.run.seal().into_iter().filter_map(|l| Some((l, lights.plan(&l)?))).collect();
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        let Some(constants) = self.view else { return };
        if self.plans.is_empty() {
            return;
        }
        let lw = &*self.lights;
        let mut gpu = upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return };
        if gpu.lights().is_none() {
            gpu.set_lights(LightSet::new(t.device, t.queue, lw.normalization.as_ref(), lw.attenuation.as_ref()));
        }
        let draws: Vec<LightDraw<'_>> = self
            .plans
            .iter()
            .filter_map(|(light, planned)| Some((light.constants.as_ref()?, planned)))
            .flat_map(|(c, planned)| {
                planned.iter().map(move |p| LightDraw {
                    program: &p.compiled.program,
                    key: p.compiled.key,
                    frame: &c.frame,
                    gel: c.gel,
                    ranges: p.ranges.clone(),
                })
            })
            .collect();
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
        };
        gpu.encode_lights(
            t.device,
            t.queue,
            &mut *t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            t.size,
            &view,
            &draws,
            &mut |s| match s {
                LightStage::Bitmap { bitmap, image } => lw.images.get(&(bitmap, image)).cloned(),
                _ => None,
            },
            &mut |g| lw.gels.get(&g).cloned(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    fn level(face: u32, level: u32, depth: u32) -> assets::bitmap::Level {
        assets::bitmap::Level {
            face,
            level,
            width: 2,
            height: 2,
            depth,
            rgba: vec![face as u8; (16 * depth) as usize],
        }
    }

    #[test]
    fn the_light_bitmaps_come_from_the_rasterizer_data() {
        let mut m = std::collections::HashMap::new();
        let mut put = |a: u32, v: u32| {
            m.insert(a, v);
        };
        let matg = 0x8000_0000;
        put(matg + RASTERIZER_DATA, 1);
        put(matg + RASTERIZER_DATA + 4, 0x8000_1000);
        put(0x8000_1000 + NORMALIZATION_DATUM, 0xe001_0002);
        put(0x8000_1000 + ATTENUATION_DATUM, u32::MAX);
        let read = |a: u32, len: usize| (len == 4).then(|| m.get(&a).map(|v| v.to_le_bytes().to_vec())).flatten();
        assert_eq!(light_bitmaps(&read, matg), (Some(map::TagId(0xe001_0002)), None));
        assert_eq!(light_bitmaps(&read, 0x9000_0000), (None, None), "unreadable");
    }

    #[test]
    fn cube_and_volume_shapes_from_decoded_levels() {
        let faces: Vec<_> = (0..6).flat_map(|f| [level(f, 0, 1), level(f, 1, 1)]).collect();
        let c = layered(faces, TextureDim::Cube).expect("six faces at level 0");
        assert_eq!((c.layers, c.rgba.len()), (6, 96));
        assert!(layered(vec![level(0, 0, 4)], TextureDim::Cube).is_none());
        let v = layered(vec![level(0, 0, 4), level(0, 1, 2)], TextureDim::D3).expect("level 0's slices");
        assert_eq!((v.layers, v.rgba.len()), (4, 64));
        assert!(layered(vec![level(0, 1, 4)], TextureDim::D3).is_none(), "no level 0");
    }

    #[test]
    fn a_light_takes_the_parts_its_sphere_meets_unless_it_meets_a_d3d_surface() {
        let unit = |x: f32| Aabb { min: [x, 0.0, 0.0], max: [x + 1.0, 1.0, 1.0] };
        let part = |material, x, first_index| LitPart { material, bounds: unit(x), first_index, index_count: 6 };
        let parts = [part(2, 0.0, 0), part(1, 2.0, 6), part(2, 4.0, 12), part(1, 10.0, 18)];
        let got = parts_meeting(&parts, &[], [3.0, 0.5, 0.5], 1.5).expect("nothing elsewhere");
        let want: BTreeMap<u16, Vec<Range<u32>>> = [(1, vec![6..12]), (2, vec![12..18])].into_iter().collect();
        assert_eq!(got, want);
        let got = parts_meeting(&parts, &[], [3.0, 0.5, 0.5], 3.0).unwrap();
        assert_eq!(got[&2], vec![0..6, 12..18], "a material's parts in order");
        assert!(parts_meeting(&parts, &[unit(6.0)], [3.0, 0.5, 0.5], 3.0).is_none(), "meets a D3D surface");
        assert!(parts_meeting(&parts, &[unit(20.0)], [3.0, 0.5, 0.5], 3.0).is_some(), "a far one does not matter");
        assert!(parts_meeting(&parts, &[], [50.0, 0.0, 0.0], 1.0).unwrap().is_empty());
    }

    #[test]
    fn only_unshadowed_lights_with_constants_are_drawable() {
        use native_render::lights::LightConstants;
        let c = LightConstants {
            frame: [0; 36],
            flags: 0,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
        };
        let light = |constants| Light {
            handle: 1,
            definition: 2,
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [1.0; 3],
            colour_b: [0.0; 3],
            intensity: 1.0,
            bounds: ([0.0; 3], 1.0),
            shadow: false,
            weight: 1.0,
            constants,
        };
        assert!(drawable(&light(Some(c))));
        assert!(!drawable(&light(Some(LightConstants { flags: 4 | 0x20, ..c }))), "shadowed");
        assert!(!drawable(&light(None)), "no constants");
    }

    #[test]
    fn the_device_sees_the_hooks_memory_and_its_faults() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut dm = DeviceMemory(&mut m);
        d3d::GuestMemory::write_u32(&mut dm, 0x1010, 0xdead_beef).unwrap();
        assert_eq!(d3d::GuestMemory::read_u32(&dm, 0x1010).unwrap(), 0xdead_beef);
        let e = d3d::GuestMemory::read_u32(&dm, 0x10fe).unwrap_err();
        assert_eq!((e.addr, e.len), (0x10fe, 4));
        assert!(d3d::GuestMemory::write(&mut dm, 0x0ffc, &[0; 4]).is_err());
        assert_eq!(m.u32(0x1010).unwrap(), 0xdead_beef);
    }
}
