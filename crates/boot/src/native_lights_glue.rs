//! Native diffuse and specular passes for unshadowed BSP cluster lights.
//! The world worker loads the light programs, stage images and geometry.
//! At RENDER_LIGHT_PASSES, prepare_light validates the entire active
//! gathered list and resolves every pass before the guest can be skipped.
//! Each run owns its prepared lights and the world their indices refer to.
//! The original handles a refused light. Tally reports its reason once.
//! The comparison keeps one eligible light on D3D and brackets its draws.

mod compare;

use compare::{Begin, End, ProbeState};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::memory::{GuestMemory, MemFault};
use feature::native_lights::Host;
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::Image;
use native_render::light_gpu::{LayeredImage, LightDraw, LightSet, TextureDim, light_gpu_key};
use native_render::light_look::{
    DIFFUSE_LAYER, LightInputs, LightProgram, LightRefusal, LightStage, LookCache, SPECULAR_LAYER, compile_light, engine_inputs, light_look,
};
use native_render::light_list::Item;
use native_render::lights::{Light, LightConstants, Run};
use native_render::look;
use native_render::program::ViewInputs;
use native_render::world::{Owner, World, WorldDraw};
use native_render::program::InheritedCull;
use tags::material::{EngineInputs, Material, ShaderLibrary, TextureSource};

use crate::native_world_glue::{Load, Ready, State, ReadMemory, StagePalettes, inherited_cull, light_image, upload};

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
    /// What preparation returned, by reason ([`Tally`]).
    tally: Mutex<Tally>,
    compare: Option<Mutex<ProbeState>>,
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
    /// A part it meets has no material (its shader does not read).
    NoMaterial,
    /// A pass it needs is refused (`light_look`, `compile_light`, the
    /// material), with why.
    Refused(String),
    /// A texture a pass samples did not load: which.
    Unloaded(&'static str),
    /// Split: a native shader also draws a surface left to D3D.
    SharedShader,
    Unsupported(&'static str),
}

impl std::fmt::Display for Keep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Keep::NoWorld => f.write_str("the world's lights are not loaded"),
            Keep::NoConstants => f.write_str("no constants from the hook"),
            Keep::Shadowed => f.write_str("shadowed"),
            Keep::NoMaterial => f.write_str("meets a part without a material"),
            Keep::Refused(why) => write!(f, "pass refused: {why}"),
            Keep::Unloaded(what) => write!(f, "samples an unloaded {what}"),
            Keep::Unsupported(why) => f.write_str(why),
            Keep::SharedShader => f.write_str("split: a native shader also draws a surface left to D3D"),
        }
    }
}

/// Lights considered between summaries.
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
    fn end_probe(&self, layer: Option<usize>, mem: Option<&mut dyn GuestMemory>) {
        let Some(compare) = &self.compare else { return };
        let mut state = compare.lock().unwrap_or_else(|e| e.into_inner());
        if let ProbeState::Reference { handle, last_layer } = *state
            && layer.is_none_or(|l| l == last_layer)
            && let Ok(mut device) = self.device.lock()
        {
            if let Some(mem) = mem {
                if !device.external_pass_now(&mut DeviceMemory(mem), Box::new(End(handle))).unwrap_or(false) {
                    device.request_external_pass(Box::new(End(handle)));
                }
            } else {
                device.request_external_pass(Box::new(End(handle)));
            }
            *state = ProbeState::Done;
        }
    }

    fn prepare_light(&self, mem: &dyn GuestMemory, light: &Light, split: bool)
        -> Result<(PreparedLight, HashSet<u16>), Keep>
    {
        let constants = constants(light)?;
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let (Some((name, bsp, index)), Load::Ready(r)) = (&state.0, &state.1) else { return Err(Keep::NoWorld) };
            if mem.u32(feature::native_world::ACTIVE_BSP).ok() != Some(*bsp)
                || crate::native_world_glue::active_bsp_index(mem) != Some(*index)
                || crate::health::loaded_map_name(mem).ok().as_ref() != Some(name)
            {
                return Err(Keep::NoWorld);
            }
            Arc::clone(r)
        };
        let world = ready.lights.as_ref().ok_or(Keep::NoWorld)?;
        let list = gathered(mem, constants.flags, constants.definition_flags, split)?;
        let (selected, kept) = select_parts(&world.clusters, &list, split)?;
        let (draws, shaders) = world.resolve_parts(&constants, selected)?;
        if split && (!kept.is_disjoint(&shaders) || !world.d3d_shaders.is_disjoint(&shaders)) {
            return Err(Keep::SharedShader);
        }
        if draws.is_empty() {
            return Err(Keep::Unsupported("no supported light draws in the active list"));
        }
        let d = self.device.lock().map_err(|_| Keep::Unsupported("device unavailable"))?;
        let memory = ReadMemory(mem);
        let target = d.external_ready(&memory, false).map_err(|why| Keep::Unsupported(why.describe()))?;
        if target.rect.2 == 0 || target.rect.3 == 0 {
            return Err(Keep::Unsupported("empty light viewport"));
        }
        if target.rect != (0, 0, target.surface_size.0, target.surface_size.1) {
            return Err(Keep::Unsupported("light viewport covers only part of the target"));
        }
        if d.depth_clip(&memory, target.z_scale).w_buffered != Some(true) {
            return Err(Keep::Unsupported("light view does not have a known W-buffer"));
        }
        let mut verified = [false; 4];
        for pass in &draws {
            let program = &pass.compiled.program;
            for (n, stage) in program.stages.iter().enumerate() {
                if let LightStage::Bitmap { bitmap, image } = stage
                    && program.dims[n].is_some() && !verified[n]
                    && world.palettized.contains(&(n, *bitmap, *image))
                {
                    if d.stage_palette(&memory, n) != world.palettes[n] {
                        return Err(Keep::Unsupported("stage palette changed since light texture upload"));
                    }
                    verified[n] = true;
                }
            }
        }
        let cull = d.cull_state(&memory).map(inherited_cull);
        if cull.is_none() && draws.iter().any(|p| p.compiled.program.inherits_cull) {
            return Err(Keep::Unsupported("unreadable inherited CULLMODE or FRONTFACE"));
        }
        let cull = InheritedCull { device: cull, clusters: true, placements: false };
        Ok((PreparedLight { ready, constants, handle: light.handle, draws, cull }, shaders))
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    type Plan = PreparedLight;

    fn probe(&self, mem: &mut dyn GuestMemory, plan: &PreparedLight) -> bool {
        let Some(compare) = &self.compare else { return false };
        let mut state = compare.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last) = plan.draws.last() else { return false };
        if !matches!(*state, ProbeState::Waiting) {
            return false;
        }
        let Ok(mut device) = self.device.lock() else { return false };
        if !device.external_pass_now(&mut DeviceMemory(mem), Box::new(Begin::new(plan.clone()))).unwrap_or(false) {
            eprintln!("native-lights compare: device refused the reference marker, keeping this light on D3D");
            *state = ProbeState::Done;
            return true;
        }
        *state = ProbeState::Reference { handle: plan.handle, last_layer: last.layer };
        eprintln!("native-lights compare: keeping light {:#010x} on D3D for {} planned passes", plan.handle, plan.draws.len());
        true
    }

    fn end_layer(&self, mem: &mut dyn GuestMemory) {
        if mem.u32(native_render::skip::CURRENT_LIST).ok() == Some(feature::native_lights::LIGHT_LIST)
            && let Ok(layer) = mem.u32(native_render::skip::CURRENT_LAYER)
        {
            self.end_probe(Some(layer as usize), Some(mem));
        }
    }

    fn checks_gathered_objects(&self) -> bool {
        true
    }

    fn prepare(&self, mem: &dyn GuestMemory, light: &Light) -> Option<PreparedLight> {
        self.end_probe(None, None);
        let answer = self.prepare_light(mem, light, false);
        let say = self.tally.lock().unwrap_or_else(|e| e.into_inner()).count(
            answer.as_ref().map(|_| ()).map_err(Clone::clone)
        );
        for line in say {
            eprintln!("{line}");
        }
        answer.ok().map(|(plan, _)| plan)
    }

    fn split(&self, mem: &dyn GuestMemory, light: &Light) -> Option<(PreparedLight, Arc<HashSet<u16>>)> {
        let answer = self.prepare_light(mem, light, true);
        let mut tally = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        match answer {
            Ok((plan, shaders)) if !plan.draws.is_empty() && !shaders.is_empty() => {
                tally.split += 1;
                Some((plan, Arc::new(shaders)))
            }
            Ok(_) => None,
            Err(why) => {
                for line in tally.count_kept(why) {
                    eprintln!("{line}");
                }
                None
            }
        }
    }

    fn open(&self, mem: &mut dyn GuestMemory, _camera: &RenderCamera, run: Arc<Run<PreparedLight>>) -> Result<(), &'static str> {
        let mut d = self.device.lock().map_err(|_| "device unavailable")?;
        // The caller publishes the run only after open returns. Seal this
        // one light before any other caller can add work to it.
        let [light] = <[PreparedLight; 1]>::try_from(run.seal()).map_err(|_| "expected one prepared light at submission")?;
        let encoded = Arc::new(AtomicBool::new(false));
        let pass = RunPass { light, view: None, clip: None, encoded: Arc::clone(&encoded) };
        let issued = d.external_pass_now(&mut DeviceMemory(mem), Box::new(pass))
            .map_err(|_| "could not drain or issue the light pass")?;
        if !issued {
            return Err("device declined the light pass at submission");
        }
        if !encoded.load(Ordering::Acquire) {
            return Err("light encoder could not bind all prepared draws");
        }
        Ok(())
    }

    fn tail(&self, mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool {
        let Ok(mut d) = self.device.lock() else { return false };
        d.light_passes_tail(&mut DeviceMemory(mem), clip).unwrap_or(false)
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

fn constants(light: &Light) -> Result<LightConstants, Keep> {
    let c = light.constants.ok_or(Keep::NoConstants)?;
    if c.shadowed() {
        return Err(Keep::Shadowed);
    }
    if c.animated_gel {
        return Err(Keep::Unsupported("animated gel renders into target 13"));
    }
    Ok(c)
}

type ClusterParts = HashMap<(Owner, u8), (TagId, WorldDraw)>;

fn select_parts(
    clusters: &ClusterParts,
    items: &[Item],
    split: bool,
) -> Result<(Vec<(usize, WorldDraw)>, HashSet<u16>), Keep> {
    let mut selected = Vec::new();
    let mut kept = HashSet::new();
    for item in items {
        let Some((_, draw)) = clusters.get(&item.part).filter(|(shader, _)| shader.0 == item.shader) else {
            if split {
                kept.insert(item.shader as u16);
                continue;
            }
            return Err(Keep::Unsupported("gathered part or shader has no native cluster draw"));
        };
        for layer in [DIFFUSE_LAYER, SPECULAR_LAYER] {
            if item.layers & (1 << layer) != 0 {
                selected.push((layer, *draw));
            }
        }
    }
    // Stable within each layer, including repeated items.
    selected.sort_by_key(|(layer, _)| *layer);
    Ok((selected, kept))
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
        compare: (std::env::var("H2_NATIVE_LIGHTS_COMPARE").as_deref() == Ok("1")).then(Mutex::default),
    }));
}

fn gathered(mem: &dyn GuestMemory, flags: u32, definition: u32, split: bool) -> Result<Vec<native_render::light_list::Item>, Keep> {
    struct Read<'a>(&'a dyn GuestMemory);
    impl native_render::camera::GuestRead for Read<'_> {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.u32(addr).ok()
        }
    }
    native_render::light_list::read(&Read(mem), feature::native_lights::LIGHT_LIST, flags, definition, split)
        .map_err(Keep::Unsupported)
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
    material: u16,
    layer: usize,
    implementation: usize,
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
    clusters: ClusterParts,
    /// Those surfaces' shaders (tag indices).
    d3d_shaders: HashSet<u16>,
    /// World-space vertex programs by `vrtx`.
    microcode: HashMap<TagId, Vec<u8>>,
    /// Stage images by (stage, bitmap, image) since palettes are per stage.
    images: HashMap<(usize, TagId, u32), Image>,
    palettized: HashSet<(usize, TagId, u32)>,
    palettes: StagePalettes,
    /// Gel bitmaps' image 0 by datum.
    gels: HashMap<u32, Image>,
    normalization: Option<LayeredImage>,
    attenuation: Option<LayeredImage>,
}

impl LightsWorld {
    fn resolve_parts(&self, c: &native_render::lights::LightConstants, parts: Vec<(usize, WorldDraw)>)
        -> Result<(Vec<Planned>, HashSet<u16>), Keep>
    {
        let mut shaders = HashSet::new();
        // Resolved without the colours, which each light's uniforms take
        // (`colour_uniforms`): a light whose colour animates keeps its key.
        let inputs = LightInputs::from_constants(c).without_colours();
        let key = inputs.key();
        let mut out: Vec<Planned> = Vec::new();
        let mut r = self.resolver.lock().unwrap_or_else(|e| e.into_inner());
        let Resolver { lib, cache } = &mut *r;
        for (layer, draw) in parts {
            let m = draw.material;
            let range = draw.first_index..draw.first_index + draw.index_count;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            shaders.insert(lit.shader.0 as u16);
            let int_input = lit.instance.ints[4] as u16;
            let Some(imp) = engine::light_passes::implementation(layer as u32, c.flags, c.definition_flags as u8, int_input) else {
                continue;
            };
            let imp = imp as usize;
            let made = cache.get_or_make(lit.shader, layer, imp, key, || self.resolve(lib, lit, &inputs, layer, imp));
            match made {
                Lit::Nothing => {}
                Lit::Refused(why) => return Err(Keep::Refused(why)),
                Lit::Draw(compiled) => {
                    if compiled.program.inputs & !draw.inputs != 0 {
                        return Err(Keep::Unsupported("cluster lacks a vertex stream read by its light program"));
                    }
                    self.binds(&compiled.program, c.gel)?;
                    if let Some(previous) = out.last_mut()
                        && previous.material == m && previous.layer == layer && Arc::ptr_eq(&previous.compiled, &compiled)
                    {
                        previous.ranges.push(range);
                        continue;
                    }
                    out.push(Planned {
                        compiled,
                        material: m, layer, implementation: imp, ranges: vec![range],
                    });
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
        for (stage, (s, d)) in program.stages.iter().zip(&program.dims).enumerate() {
            if d.is_none() {
                continue;
            }
            let (loaded, what) = match *s {
                LightStage::Empty => (false, "empty stage"),
                LightStage::Bitmap { bitmap, image } => (self.images.contains_key(&(stage, bitmap, image)), "stage bitmap"),
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
    palettes: &StagePalettes,
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
    let mut palettized = HashSet::new();
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
            for (stage, t) in imp.textures.iter().enumerate().take(4) {
                let Some(TextureSource::Bitmap { bitmap, image, .. }) = t.map(|t| t.source) else { continue };
                let key = (stage, bitmap, image);
                if !images.contains_key(&key)
                    && let Some((pixels, expanded)) = light_image(map, files, bitmap, image as usize, stage, &palettes[stage])
                {
                    images.insert(key, pixels);
                    if expanded {
                        palettized.insert(key);
                    }
                }
            }
        }
    }
    // Opaque parts of native materials are drawn; any other surface that
    // has (or may have) lit layers keeps the lights meeting it on D3D.
    let mut clusters = HashMap::new();
    // Why each surface is left to D3D: opaque and drawn by D3D (the world
    // left its material there), opaque with a shader that does not read,
    // transparent with lit layers, transparent with a shader that does not
    // read.
    let mut why = [0usize; 4];
    let mut d3d_materials = Vec::new();
    for d in &world.draws {
        match lit_of.get(&d.material).copied().flatten() {
            Some(true) if native(d.material) && matches!(d.owner, Owner::Cluster(_)) => {
                clusters.insert((d.owner, d.part), (shaders[usize::from(d.material)], *d));
            }
            Some(false) => {}
            Some(true) => {
                why[0] += 1;
                d3d_materials.push(d.material);
            }
            None => {
                why[1] += 1;
                d3d_materials.push(d.material);
            }
        }
    }
    for t in &world.transparents {
        match lit_of.get(&t.material).copied().flatten() {
            Some(false) => {}
            Some(true) => {
                why[2] += 1;
                d3d_materials.push(t.material);
            }
            None => {
                why[3] += 1;
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
        "native-lights: {} lit parts, {} surfaces left to D3D ({} opaque D3D draws, {} opaque unread shaders, {} transparent lit, {} transparent unread; a whole light containing one stays on D3D), {} vertex programs, {} stage images, {} gels",
        clusters.len(),
        why.iter().sum::<usize>(),
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
        clusters,
        d3d_shaders,
        microcode,
        images,
        palettized,
        palettes: palettes.clone(),
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
            && let Some((i, _)) = light_image(map, files, TagId(d), 0, 0, &None)
        {
            out.insert(d, i);
        }
    }
    out
}

/// Owned at admission, including the world whose index buffer the ranges
/// address. Issuing or replaying a run never reads the list or resolves a
/// material a second time.
#[derive(Clone)]
struct PreparedLight {
    ready: Arc<Ready>,
    constants: native_render::lights::LightConstants,
    handle: u32,
    draws: Vec<Planned>,
    cull: InheritedCull,
}

impl PreparedLight {
    fn draw<'a>(&'a self, p: &'a Planned) -> LightDraw<'a> {
        let c = &self.constants;
        let inputs = LightInputs::from_constants(c);
        LightDraw {
            program: &p.compiled.program,
            key: p.compiled.key,
            cull: self.cull.of(p.compiled.program.inherits_cull, true),
            colours: [inputs.colour_a, inputs.colour_b],
            frame: &c.frame,
            gel: c.gel,
            ranges: p.ranges.clone(),
        }
    }
}

struct RunPass {
    light: PreparedLight,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<([f32; 2], bool)>,
    encoded: Arc<AtomicBool>,
}

impl d3d::ExternalPass for RunPass {
    fn confirm_issue(&self) -> bool {
        true
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some((clip.range, clip.clamp));
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let Some(constants) = self.view else { return };
        let view = ViewInputs { constants, guest_size: t.guest_size, size: t.size, z_scale: t.z_scale, w_clip: self.clip };
        let ready = &self.light.ready;
        let Some(lw) = &ready.lights else { return };
        let mut gpu = upload(ready, &t);
        let Some(gpu) = gpu.as_mut() else { return };
        if gpu.lights().is_none() {
            gpu.set_lights(LightSet::new(t.device, t.queue, lw.normalization.as_ref(), lw.attenuation.as_ref()));
        }
        let draws: Vec<_> = self.light.draws.iter().map(|p| self.light.draw(p)).collect();
        let drawn = gpu.encode_lights(
            t.device, t.queue, t.encoder, t.color, t.color_format, t.depth, t.depth_format, t.size, &view, &draws,
            &mut |stage, s| match s {
                LightStage::Bitmap { bitmap, image } => lw.images.get(&(stage, bitmap, image)).cloned(),
                _ => None,
            },
            &mut |g| lw.gels.get(&g).cloned(),
        );
        // LightSet checks all resources before opening its render pass.
        if drawn != 0 {
            debug_assert_eq!(drawn, draws.iter().map(|d| d.ranges.len()).sum::<usize>());
            self.encoded.store(true, Ordering::Release);
        }
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
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), Some(TagId(0)), "an unwritten word reads as datum 0, as the game reads it");
        // A null reference: datum -1.
        words.push((element + 0x60, bitm));
        words.push((element + 0x64, u32::MAX));
        let meta = metadata(base, 0x1000, &words);
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), None, "a null datum");
        words.pop();
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
    fn gathered_parts_match_owner_part_and_full_shader_in_guest_layer_order() {
        let shader = TagId(0x1234_0017);
        let part = |owner, first_index| WorldDraw {
            owner: Owner::Cluster(owner), part: 1, material: 3, lightmap: None,
            first_index, index_count: 3, bounds: native_render::world::Aabb::EMPTY,
            batch: 0, inputs: 1,
        };
        let (a, b) = (part(7, 30), part(8, 6));
        let clusters = ClusterParts::from([((a.owner, 1), (shader, a)), ((b.owner, 1), (shader, b))]);
        let item = |d: WorldDraw, layers| Item { part: (d.owner, d.part), shader: shader.0, layers };
        let mut items = [item(b, 3 << 12), item(a, 1 << 12)];
        assert_eq!(select_parts(&clusters, &items, false).unwrap().0,
            vec![(DIFFUSE_LAYER, b), (DIFFUSE_LAYER, a), (SPECULAR_LAYER, b)]);
        items[1].shader ^= 1 << 24;
        assert!(select_parts(&clusters, &items, false).is_err());
        assert_eq!(select_parts(&clusters, &items, true).unwrap().1, HashSet::from([shader.0 as u16]));
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
            t.count(Err(Keep::Unsupported("gathered model"))),
            vec!["native-lights: a light keeps its D3D passes: gathered model".to_string()]
        );
        assert!(t.count(Ok(())).is_empty());
        assert_eq!(
            t.summary(),
            "native-lights: host planned 1 of 4 lights, split 0; kept on D3D: 2 samples an unloaded normalization cube, 1 gathered model"
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
            animated_gel: false,
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
        assert!(constants(&light(Some(c))).is_ok());
        assert!(matches!(constants(&light(Some(LightConstants { flags: 4 | 0x20, ..c }))), Err(Keep::Shadowed)));
        assert!(matches!(constants(&light(Some(LightConstants { animated_gel: true, ..c }))), Err(Keep::Unsupported(_))));
        assert!(matches!(constants(&light(None)), Err(Keep::NoConstants)));
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
