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
//!   material resolved with the light's inputs but its colours,
//!   `light_look` and `compile_light`, cached by those inputs (`LookCache`;
//!   the colours go into each light's uniforms, `colour_uniforms`, so an
//!   animated colour resolves nothing). A light that meets a
//!   surface it cannot light, or whose pass is refused, keeps its D3D
//!   passes.
//! - [`Host::accepts`]: [`drawable`] and a plan over the loaded world.
//!   Each answer is counted ([`Tally`]): a reason ([`Keep`]) is said the
//!   first time it keeps a light on D3D, and every [`SUMMARY_EVERY`]
//!   lights a line gives the lights planned and the kept ones by reason.
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

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// What `accepts` said, by reason ([`Tally`]).
    tally: Mutex<Tally>,
}

/// Why a light keeps its D3D passes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Keep {
    /// The world (or its lights' part) is not loaded.
    NoWorld,
    /// The hook did not compute the light's constants.
    NoConstants,
    /// A shadowed light: the shadow maps are not drawn natively.
    Shadowed,
    /// Its sphere meets a world surface the native pass cannot light.
    MeetsD3dSurface,
    /// A part it meets has no material (its shader does not read).
    NoMaterial,
    /// A pass it needs is refused (`light_look`, `compile_light`, the
    /// material), with why.
    Refused(String),
    /// A texture a pass samples did not load: which.
    Unloaded(&'static str),
    /// Split: a native shader also draws a surface left to D3D.
    SharedShader,
}

impl std::fmt::Display for Keep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Keep::NoWorld => f.write_str("the world's lights are not loaded"),
            Keep::NoConstants => f.write_str("no constants from the hook"),
            Keep::Shadowed => f.write_str("shadowed"),
            Keep::MeetsD3dSurface => f.write_str("meets a surface left to D3D"),
            Keep::NoMaterial => f.write_str("meets a part without a material"),
            Keep::Refused(why) => write!(f, "pass refused: {why}"),
            Keep::Unloaded(what) => write!(f, "samples an unloaded {what}"),
            Keep::SharedShader => f.write_str("split: a native shader also draws a surface left to D3D"),
        }
    }
}

/// Lights `accepts` was asked about between summaries.
pub const SUMMARY_EVERY: u64 = 1800;

/// The host's answers so far: lights asked, lights planned, and the lights
/// kept on D3D by reason.
#[derive(Debug, Default)]
pub struct Tally {
    pub asked: u64,
    pub planned: u64,
    /// Lights split (the host's `split`): their native surfaces
    /// drawn natively, the rest by their D3D passes.
    pub split: u64,
    pub kept: BTreeMap<Keep, u64>,
}

impl Tally {
    /// Counts one answer; returns what to say on stderr: a reason the first
    /// time it keeps a light, and every [`SUMMARY_EVERY`] lights the
    /// counts.
    pub fn count(&mut self, answer: Result<(), Keep>) -> Vec<String> {
        self.asked += 1;
        let mut say = match answer {
            Ok(()) => {
                self.planned += 1;
                Vec::new()
            }
            Err(k) => self.count_kept(k),
        };
        if self.asked % SUMMARY_EVERY == 0 {
            say.push(self.summary());
        }
        say
    }

    /// Counts a reason a light keeps (some of) its D3D passes; the line to
    /// say the first time.
    pub fn count_kept(&mut self, k: Keep) -> Vec<String> {
        let n = self.kept.entry(k.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { vec![format!("native-lights: a light keeps its D3D passes: {k}")] } else { Vec::new() }
    }

    /// One line: lights asked, planned, and kept by reason, most first.
    pub fn summary(&self) -> String {
        let mut kept: Vec<_> = self.kept.iter().collect();
        kept.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let kept: Vec<String> = kept.iter().map(|(k, n)| format!("{n} {k}")).collect();
        format!(
            "native-lights: host planned {} of {} lights, split {}; kept on D3D: {}",
            self.planned,
            self.asked,
            self.split,
            if kept.is_empty() { "none".to_string() } else { kept.join(", ") }
        )
    }
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
        let answer = match light.constants {
            None => Err(Keep::NoConstants),
            Some(c) if c.shadowed() => Err(Keep::Shadowed),
            Some(_) => match self.ready() {
                None => Err(Keep::NoWorld),
                Some((_, lights)) => lights.plan(light).map(|_| ()),
            },
        };
        let accepted = answer.is_ok();
        let say = self.tally.lock().unwrap_or_else(|e| e.into_inner()).count(answer);
        for line in say {
            eprintln!("{line}");
        }
        accepted
    }

    fn split(&self, _mem: &dyn GuestMemory, light: &Light) -> Option<Arc<HashSet<u16>>> {
        let answer = match self.ready() {
            None => Err(Keep::NoWorld),
            Some((_, lights)) => lights.plan_split(light),
        };
        let mut tally = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        match answer {
            Ok((_, shaders)) => {
                tally.split += 1;
                Some(Arc::new(shaders))
            }
            Err(Keep::SharedShader) => {
                for line in tally.count_kept(Keep::SharedShader) {
                    eprintln!("{line}");
                }
                None
            }
            Err(_) => None,
        }
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

/// The `matg` tag block of rasterizer data: its count at `+0x108`, the
/// elements' address at `+0x10c`. `0x00022590` reads the globals tag
/// (`[0x004e034c]`), tests the count (`[eax+0x108]`) and keeps the address
/// (`[eax+0x10c]`, the first element) at `[0x00485a80]`, zero when the
/// block is empty.
pub const RASTERIZER_DATA: u32 = 0x108;
/// The element's bitmap datums the light passes sample: engine texture 1
/// (`+0xc`, the normalization cube) and 9 (`+4`, the attenuation volume),
/// the datum words of the tag references at `+8` and `+0` (`0x00019581`
/// reads `[[0x00485a80]+0xc]`, `0x00019673` `[[0x00485a80]+4]`).
pub const NORMALIZATION_DATUM: u32 = 0xc;
pub const ATTENUATION_DATUM: u32 = 4;

/// The light textures' bitmaps named by the `matg` at `matg` (its payload
/// address), read through `read(address, len)`: `(normalization,
/// attenuation)`, each `None` when null or unreadable.
pub fn light_bitmaps(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    (rasterizer_bitmap(read, matg, NORMALIZATION_DATUM), rasterizer_bitmap(read, matg, ATTENUATION_DATUM))
}

/// The bitmap datum at `offset` of the `matg`'s first rasterizer data
/// element (`[0x00485a80] + offset`), `None` when null or unreadable.
pub fn rasterizer_bitmap(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32, offset: u32) -> Option<map::TagId> {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(read(a, 4)?.try_into().ok()?)) };
    let count = word(matg.wrapping_add(RASTERIZER_DATA))?;
    (count > 0).then_some(())?;
    let e = word(matg.wrapping_add(RASTERIZER_DATA + 4))?;
    word(e.wrapping_add(offset)).filter(|&d| d != u32::MAX).map(map::TagId)
}

/// [`light_bitmaps`] over the map's ordinary metadata (`matg` the tag's
/// virtual address, `Tag::location`, which the element's address shares).
pub fn matg_light_bitmaps(meta: &tags::shader::Metadata, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    light_bitmaps(&|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec), matg)
}

/// A decoded bitmap's first image as a cube (six faces of level 0) or a
/// volume (level 0's slices), with the mip chain below it; `None` when it
/// is not that shape. `assets::decode_image` gives a cube's levels
/// face-major, each face's levels one slice deep, and a volume's as face 0
/// only, each level with all its slices. The texels are the decode's, as
/// the D3D path uploads them for a stage without `D3DTSS_COLORSIGN` (the
/// light passes' engine stages; one with signed channels is refused,
/// `native_render::program`).
pub fn layered(levels: Vec<assets::bitmap::Level>, dim: TextureDim) -> Option<LayeredImage> {
    let deepest = levels.iter().map(|l| l.level).max()?;
    let (top, rest): (Vec<_>, Vec<_>) = levels.into_iter().partition(|l| l.level == 0);
    let mut below: Vec<Vec<assets::bitmap::Level>> = (0..deepest).map(|_| Vec::new()).collect();
    for l in rest {
        below[l.level as usize - 1].push(l);
    }
    let image = match dim {
        TextureDim::Cube => {
            if top.iter().any(|l| l.depth != 1) {
                return None;
            }
            LayeredImage::cube(top.into_iter().map(|l| (l.face, l.width, l.height, l.rgba)).collect())?
        }
        TextureDim::D3 => {
            // One level 0 of face 0: a cube's six faces are not a volume.
            let [l] = <[_; 1]>::try_from(top).ok()?;
            (l.face == 0).then_some(())?;
            LayeredImage::volume(l.width, l.height, l.depth, l.rgba)?
        }
        TextureDim::D2 => return None,
    };
    // Each level's layers in order (a cube's faces 0..6, a volume's one
    // face); a level that is not whole ends the chain (`with_mips`).
    let mips = below
        .into_iter()
        .map(|mut faces| {
            faces.sort_by_key(|l| l.face);
            let whole = faces.iter().map(|l| l.face).eq(0..faces.len() as u32);
            if whole { faces.into_iter().flat_map(|l| l.rgba).collect() } else { Vec::new() }
        })
        .collect();
    Some(image.with_mips(mips))
}

/// What a decoded light texture is, for the load's log line.
fn shape(levels: &[assets::bitmap::Level]) -> String {
    let faces = levels.iter().map(|l| l.face).max().map_or(0, |f| f + 1);
    let top: Vec<_> = levels.iter().filter(|l| l.level == 0).map(|l| (l.width, l.height, l.depth)).collect();
    format!("{faces} faces, level 0 {top:?}")
}

/// [`light_bitmaps`] of the map's `matg`, decoded: `(normalization cube,
/// attenuation volume)`. Says on stderr what loaded and, for what does not,
/// why (the light passes that sample it stay on the D3D path).
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
    let (n, a) = matg_light_bitmaps(meta, matg);
    let mut decode = |id: Option<map::TagId>, dim, what| {
        let image = (|| -> Result<LayeredImage, String> {
            let id = id.ok_or_else(|| format!("no datum in matg {matg:#x} +{RASTERIZER_DATA:#x}'s element"))?;
            let tag = tags::DecodedTag::read(map, id).map_err(|e| format!("{id:?} does not read: {e}"))?;
            let tags::TagData::Bitmap(b) = tag.data else {
                return Err(format!("{id:?} is not a bitmap"));
            };
            let image = b.images.first().ok_or_else(|| format!("{id:?} has no image"))?;
            let d = assets::decode_image(image, files).map_err(|e| format!("{id:?} does not decode: {e}"))?;
            let shape = shape(&d.levels);
            layered(d.levels, dim).ok_or_else(|| format!("{id:?} is not a {dim:?} ({shape})"))
        })();
        match image {
            Ok(i) => {
                eprintln!("native-lights: the {what} loads ({}x{}, {} layers, {} levels)", i.width, i.height, i.layers, 1 + i.mips.len());
                Some(i)
            }
            Err(why) => {
                eprintln!("native-lights: the {what} does not load: {why}");
                None
            }
        }
    };
    (decode(n, TextureDim::Cube, "normalization cube"), decode(a, TextureDim::D3, "attenuation volume"))
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
        tally: Mutex::new(Tally::default()),
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
    /// Those surfaces' shaders (tag indices).
    d3d_shaders: HashSet<u16>,
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
    /// What `light` draws over the world, pass by pass, or why it must keep
    /// its D3D passes. Resolves (once per distinct inputs) under the
    /// resolver's lock, which nothing else is held across.
    pub fn plan(&self, light: &Light) -> Result<Vec<Planned>, Keep> {
        self.plan_with(light, true).map(|(planned, _)| planned)
    }

    /// [`Self::plan`] for a split light: the native parts its sphere meets
    /// whatever else it meets, with the world shaders (tag indices) of
    /// those parts' materials, which its D3D passes then leave out
    /// (`feature::native_lights::split_layer`). Refused when one of those
    /// shaders also draws a surface left to D3D, since leaving its items
    /// out would leave that surface unlit.
    pub fn plan_split(&self, light: &Light) -> Result<(Vec<Planned>, HashSet<u16>), Keep> {
        let (planned, shaders) = self.plan_with(light, false)?;
        if !shaders.is_disjoint(&self.d3d_shaders) {
            return Err(Keep::SharedShader);
        }
        Ok((planned, shaders))
    }

    fn plan_with(&self, light: &Light, whole: bool) -> Result<(Vec<Planned>, HashSet<u16>), Keep> {
        let c = light.constants.ok_or(Keep::NoConstants)?;
        if c.shadowed() {
            return Err(Keep::Shadowed);
        }
        let (centre, radius) = light.bounds;
        let elsewhere: &[Aabb] = if whole { &self.elsewhere } else { &[] };
        let meeting = parts_meeting(&self.parts, elsewhere, centre, radius).ok_or(Keep::MeetsD3dSurface)?;
        let mut shaders = HashSet::new();
        // Resolved without the colours, which each light's uniforms take
        // (`colour_uniforms`): a light whose colour animates keeps its key.
        let inputs = LightInputs::from_constants(&c).without_colours();
        let key = inputs.key();
        let mut out = Vec::new();
        let mut r = self.resolver.lock().unwrap_or_else(|e| e.into_inner());
        let Resolver { lib, cache } = &mut *r;
        for (m, ranges) in meeting {
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            shaders.insert(lit.shader.0 as u16);
            let int_input = lit.instance.ints[4] as u16;
            for layer in [DIFFUSE_LAYER, SPECULAR_LAYER] {
                let Some(imp) = engine::light_passes::implementation(layer as u32, c.flags, c.definition_flags as u8, int_input) else {
                    continue;
                };
                let imp = imp as usize;
                let made = cache.get_or_make(lit.shader, layer, imp, key, || self.resolve(lib, lit, &inputs, layer, imp));
                match made {
                    Lit::Nothing => {}
                    Lit::Refused(why) => return Err(Keep::Refused(why)),
                    Lit::Draw(compiled) => {
                        self.binds(&compiled.program, c.gel)?;
                        out.push(Planned {
                            compiled,
                            ranges: ranges.clone(),
                        });
                    }
                }
            }
        }
        Ok((out, shaders))
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
    /// light's; the first that is not, when one is not.
    fn binds(&self, program: &LightProgram, gel: Option<u32>) -> Result<(), Keep> {
        for (s, d) in program.stages.iter().zip(&program.dims) {
            if d.is_none() {
                continue;
            }
            let (loaded, what) = match *s {
                LightStage::Empty => (false, "empty stage"),
                LightStage::Bitmap { bitmap, image } => (self.images.contains_key(&(bitmap, image)), "stage bitmap"),
                LightStage::Normalization => (self.normalization.is_some(), "normalization cube"),
                LightStage::Attenuation => (self.attenuation.is_some(), "attenuation volume"),
                LightStage::Gel => (gel.is_some_and(|g| self.gels.contains_key(&g)), "gel"),
            };
            if !loaded {
                return Err(Keep::Unloaded(what));
            }
        }
        Ok(())
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
    // Why each surface is left to D3D: opaque and drawn by D3D (the world
    // left its material there), opaque with a shader that does not read,
    // transparent with lit layers, transparent with a shader that does not
    // read.
    let mut why = [0usize; 4];
    let mut d3d_materials = Vec::new();
    for d in &world.draws {
        match lit_of.get(&d.material).copied().flatten() {
            Some(true) if native(d.material) => parts.push(LitPart {
                material: d.material,
                bounds: d.bounds,
                first_index: d.first_index,
                index_count: d.index_count,
            }),
            Some(false) => {}
            Some(true) => {
                why[0] += 1;
                elsewhere.push(d.bounds);
                d3d_materials.push(d.material);
            }
            None => {
                why[1] += 1;
                elsewhere.push(d.bounds);
                d3d_materials.push(d.material);
            }
        }
    }
    for t in &world.transparents {
        match lit_of.get(&t.material).copied().flatten() {
            Some(false) => {}
            Some(true) => {
                why[2] += 1;
                elsewhere.push(t.bounds);
                d3d_materials.push(t.material);
            }
            None => {
                why[3] += 1;
                elsewhere.push(t.bounds);
                d3d_materials.push(t.material);
            }
        }
    }
    let d3d_shaders = d3d_materials
        .iter()
        .filter_map(|&m| shaders.get(usize::from(m)))
        .map(|s| s.0 as u16)
        .collect();
    let (normalization, attenuation) = light_textures(map, lib.metadata(), files);
    let gels = gels(map, &lib, files);
    eprintln!(
        "native-lights: {} lit parts, {} surfaces left to D3D ({} opaque D3D draws, {} opaque unread shaders, {} transparent lit, {} transparent unread; a light meeting one keeps its D3D passes), {} vertex programs, {} stage images, {} gels",
        parts.len(),
        elsewhere.len(),
        why[0],
        why[1],
        why[2],
        why[3],
        microcode.len(),
        images.len(),
        gels.len()
    );
    Some(LightsWorld {
        resolver: Mutex::new(Resolver {
            lib,
            cache: LookCache::new(CACHE_CAPACITY),
        }),
        materials,
        parts,
        elsewhere,
        d3d_shaders,
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
        // The native parts each light meets: a light drawn whole meets
        // nothing else, and a split light's other surfaces are its D3D
        // passes' (`LightsWorld::plan_split`, checked when it joined).
        self.plans = self.run.seal().into_iter().filter_map(|l| Some((l, lights.plan_with(&l, false).ok()?.0))).collect();
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
                    colours: {
                        let l = LightInputs::from_constants(c);
                        [l.colour_a, l.colour_b]
                    },
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

    /// A synthetic metadata region at virtual `base`, zeroes but `words`.
    fn metadata(base: u32, len: usize, words: &[(u32, u32)]) -> tags::shader::Metadata {
        let mut b = vec![0u8; len];
        for &(a, v) in words {
            let o = (a - base) as usize;
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        tags::shader::Metadata::new(base, b)
    }

    #[test]
    fn the_light_bitmaps_come_from_matgs_rasterizer_block_through_the_metadata() {
        // `0x00022590`: count at matg +0x108, the first element's address
        // at +0x10c; `0x00019530` reads datums at the element's +0xc
        // (texture 1) and +4 (texture 9), each a tag reference's datum.
        assert_eq!((RASTERIZER_DATA, NORMALIZATION_DATUM, ATTENUATION_DATUM), (0x108, 0xc, 4));
        let base = 0x8000_0000;
        let matg = base + 0x200;
        let element = base + 0x800;
        let bitm = map::FourCC(*b"bitm").as_u32();
        let mut words = vec![
            (matg + 0x108, 1),
            (matg + 0x10c, element),
            // The next field: reading the block one word late took this
            // for the elements' address (and the address for the count).
            (matg + 0x110, 0x40),
            (element, bitm),
            (element + 4, 0xe000_0009),
            (element + 8, bitm),
            (element + 0xc, 0xe000_0001),
        ];
        let meta = metadata(base, 0x1000, &words);
        assert_eq!(matg_light_bitmaps(&meta, matg), (Some(TagId(0xe000_0001)), Some(TagId(0xe000_0009))));
        // An object's lightmap stage: the element's +0x64 (0x00018d1f).
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), None, "a null datum");
        words.push((element + 0x60, bitm));
        words.push((element + 0x64, 0xe000_0064));
        let meta = metadata(base, 0x1000, &words);
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), Some(TagId(0xe000_0064)));
        // The tag's file offset is not a metadata address.
        assert_eq!(matg_light_bitmaps(&meta, 0x200), (None, None));
        // A null reference is no bitmap.
        words[6].1 = u32::MAX;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, Some(TagId(0xe000_0009))));
        // An empty block names nothing, whatever its address says.
        words[0].1 = 0;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, None));
        // An element outside the metadata does not read.
        words[0].1 = 1;
        words[1].1 = base + 0x2000;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, None));
    }

    #[test]
    fn the_light_textures_take_the_shapes_the_bitmap_decode_gives() {
        use gfx::D3DFormat;
        use gfx::texture::{TextureDesc, TextureKind};
        // A swizzled X8R8G8B8 cube (an Xrgb8888 cube bitmap's descriptor),
        // 4x4 with three levels, each face's level 0 filled with its number.
        let desc = TextureDesc::new(D3DFormat::X8R8G8B8, TextureKind::Cube, 4, 4, 1, 3);
        let layout = desc.layout().unwrap();
        let mut data = vec![0u8; layout.total_size];
        for f in 0..6 {
            let at = f * layout.face_stride;
            data[at..at + layout.levels[0].size].fill(f as u8 + 1);
        }
        let levels = assets::bitmap::decode_bytes(&desc, &data).unwrap();
        assert_eq!(levels.len(), 18, "six faces of three levels, face-major");
        assert!(levels.iter().all(|l| l.depth == 1));
        let cube_levels = levels.clone();
        let c = layered(levels, TextureDim::Cube).expect("a cube");
        assert_eq!((c.dim, c.width, c.height, c.layers, c.rgba.len()), (TextureDim::Cube, 4, 4, 6, 6 * 64));
        for f in 0..6 {
            assert_eq!(c.rgba[f * 64], f as u8 + 1, "face {f} in its layer");
        }
        assert!(layered(cube_levels, TextureDim::D3).is_none(), "a cube is not a volume");

        // A swizzled AL8 volume (an Ay8 volume bitmap's), 4x4x4, three levels.
        let desc = TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3);
        let data = vec![0x80u8; desc.layout().unwrap().total_size];
        let levels = assets::bitmap::decode_bytes(&desc, &data).unwrap();
        let shape: Vec<_> = levels.iter().map(|l| (l.face, l.level, l.width, l.depth)).collect();
        assert_eq!(shape, vec![(0, 0, 4, 4), (0, 1, 2, 2), (0, 2, 1, 1)], "face 0 only, each level all its slices");
        let volume_levels = levels.clone();
        let v = layered(levels, TextureDim::D3).expect("a volume");
        assert_eq!((v.dim, v.width, v.height, v.layers, v.rgba.len()), (TextureDim::D3, 4, 4, 4, 4 * 64));
        assert!(v.rgba.iter().all(|&b| b == 0x80), "AL8 decodes to (v, v, v, v)");
        assert!(layered(volume_levels, TextureDim::Cube).is_none(), "a volume is not a cube");
    }

    /// A decoded texel as the sampler returns it (R, G, B, A), whichever
    /// host format the decode chose.
    fn sampled(host: gfx::HostFormat, px: &[u8]) -> [u8; 4] {
        match host {
            gfx::HostFormat::Bgra8Unorm => [px[2], px[1], px[0], px[3]],
            gfx::HostFormat::Rgba8Unorm => [px[0], px[1], px[2], px[3]],
            h => panic!("{h:?}"),
        }
    }

    #[test]
    fn the_native_light_textures_hold_the_texels_the_d3d_path_uploads() {
        use gfx::D3DFormat;
        use gfx::texture::{TextureDesc, TextureKind};
        // The engine stages sample without D3DTSS_COLORSIGN (the combiner
        // expands with _bx2), so the D3D path decodes with no sign bits.
        let cases = [
            (TextureDesc::new(D3DFormat::X8R8G8B8, TextureKind::Cube, 4, 4, 1, 3), TextureDim::Cube),
            (TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3), TextureDim::D3),
        ];
        for (desc, dim) in cases {
            let len = desc.layout().unwrap().total_size;
            // Synthetic texels, all different nearby.
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let d3d = gfx::decode_texture(&desc, &data, None, gfx::DecodeOptions::default().with_color_sign(0)).unwrap();
            let native = layered(assets::bitmap::decode_bytes(&desc, &data).unwrap(), dim).expect("the shape");
            assert_eq!(1 + native.mips.len() as u32, desc.levels, "{dim:?}: the whole chain");
            for l in &d3d.levels {
                let bytes = if l.level == 0 { &native.rgba } else { &native.mips[l.level as usize - 1] };
                assert_eq!(native.level_size(l.level), (l.width, l.height, if dim == TextureDim::Cube { 6 } else { l.depth }));
                let layer = (l.width * l.height * l.depth * 4) as usize;
                let at = l.face as usize * layer;
                for (i, px) in l.data.chunks_exact(4).enumerate() {
                    let ours = &bytes[at + 4 * i..at + 4 * i + 4];
                    assert_eq!(sampled(d3d.host, px), [ours[0], ours[1], ours[2], ours[3]], "{dim:?} face {} level {} texel {i}", l.face, l.level);
                }
            }
        }
    }

    #[test]
    fn the_tally_says_each_reason_once_and_sums_up() {
        let mut t = Tally::default();
        assert_eq!(
            t.count(Err(Keep::Unloaded("normalization cube"))),
            vec!["native-lights: a light keeps its D3D passes: samples an unloaded normalization cube".to_string()]
        );
        assert!(t.count(Err(Keep::Unloaded("normalization cube"))).is_empty(), "once per reason");
        assert_eq!(
            t.count(Err(Keep::MeetsD3dSurface)),
            vec!["native-lights: a light keeps its D3D passes: meets a surface left to D3D".to_string()]
        );
        assert!(t.count(Ok(())).is_empty());
        assert_eq!(
            t.summary(),
            "native-lights: host planned 1 of 4 lights, split 0; kept on D3D: 2 samples an unloaded normalization cube, 1 meets a surface left to D3D"
        );
        for _ in 5..SUMMARY_EVERY {
            assert!(t.count(Ok(())).is_empty());
        }
        let said = t.count(Ok(()));
        assert_eq!(t.asked, SUMMARY_EVERY);
        assert_eq!(said, vec![t.summary()], "the counts every SUMMARY_EVERY lights");
        assert_eq!(Tally::default().summary(), "native-lights: host planned 0 of 0 lights, split 0; kept on D3D: none");
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
