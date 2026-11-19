//! Native light draws over sorted guest geometry. Each target copy prepares
//! immutable draw resources. Only the confirmed, suppressed prefix commits.
mod compare;

use crate::native_gels_glue::{Binding, Checkpoint, Outputs};
use compare::{Begin, End, ProbeState};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use engine::memory::{GuestMemory, MemFault};
use feature::native_lights::{Dispatch, Host};
use map::{Map, TagId};
use native_render::gpu::Image;
use native_render::light_gpu::{LayeredImage, LightDraw, LightSet, TextureDim, light_gpu_key};
use native_render::light_look::{
    DIFFUSE_LAYER, LightInputs, LightProgram, LightRefusal, LightStage, LookCache, SPECULAR_LAYER, compile_light, engine_inputs, light_look,
};
use native_render::light_list::{SortedItem, SortedLayer};
use native_render::lights::{Light, LightConstants};
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
    gels: Arc<Outputs>,
    checkpoint: Mutex<Option<(u64, Checkpoint)>>,
    /// What preparation returned, by reason ([`Tally`]).
    tally: Mutex<Tally>,
    order: Mutex<RunOrder>,
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
    pub shader_gels: u64,
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

    fn gel_binding(&self, mem: &dyn GuestMemory, serial: u64, identity: Option<(u32, u32)>,
        c: &LightConstants) -> Result<Option<Binding>, Keep>
    {
        let Some((definition, shader)) = identity else {
            return if c.animated_gel { Err(Keep::Unsupported("missing gel invocation")) } else { Ok(None) };
        };
        use native_render::gels;
        if !uses_gel_target(mem, definition, shader, c.flags, c.definition_flags) {
            return Err(Keep::Unsupported("engine texture 10 does not select this gel invocation"));
        }
        let (_, before) = self.checkpoint.lock().unwrap_or_else(|e| e.into_inner())
            .take_if(|(id, _)| *id == serial).ok_or(Keep::Unsupported("missing gel entry checkpoint"))?;
        let target = gels::Target::read(&ReadMemory(mem)).ok_or(Keep::Unsupported("unavailable gel target 13"))?;
        self.gels.binding_after(before, definition, shader, target.surface)
            .filter(|b| b.guest_size == target.size)
            .map(Some).ok_or(Keep::Unsupported("no matching target 13 binding for this light"))
    }

    fn prepare_light(&self, d: &d3d::Device<B>, mem: &dyn GuestMemory, light: &Light, sorted: &SortedLayer, split: bool, binding: Option<Binding>)
        -> Result<(PreparedLight, usize), Keep>
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
        let parts = select_parts(&world.clusters, &sorted.entries, split)?;
        let consumed = parts.len();
        let draws = world.resolve_parts(&constants, parts, binding.is_some())?;
        if draws.is_empty() {
            return Err(Keep::Unsupported("no supported light draws in the active list"));
        }
        let memory = ReadMemory(mem);
        let target = d.external_ready(&memory, false).map_err(|why| Keep::Unsupported(why.describe()))?;
        if !d.stencil_state(&memory).is_some_and(|s| !s.enable) {
            return Err(Keep::Unsupported("enabled or unknown stencil state at light geometry"));
        }
        let clip = d.light_scissor(&memory).map_err(Keep::Unsupported)?;
        let scissor = intersect_scissor(clip, target.rect);
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
        Ok((PreparedLight { ready, constants, handle: light.handle, draws, cull, scissor, binding }, consumed))
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {

    fn probe(device: &mut d3d::Device<B>, mem: &mut dyn GuestMemory, plan: &PreparedLight,
        state: Option<&mut ProbeState>) -> bool {
        let Some(state) = state else { return false };
        let Some(last) = plan.draws.last() else { return false };
        if !matches!(*state, ProbeState::Waiting) {
            return false;
        }
        if !device.external_pass_now(&mut DeviceMemory(mem), Box::new(Begin::new(plan.clone()))).unwrap_or(false) {
            eprintln!("native-lights compare: device refused the reference marker, keeping this light on D3D");
            *state = ProbeState::Done;
            return true;
        }
        *state = ProbeState::Reference { handle: plan.handle, last_layer: last.layer };
        eprintln!("native-lights compare: keeping light {:#010x} on D3D for {} planned passes", plan.handle, plan.draws.len());
        true
    }

    fn reserve(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>, esi: &mut u32) -> Result<bool, Keep> {
        let Dispatch { light, serial, shader_gel, view_target, sorted, split: split_enabled } = dispatch;
        self.end_probe(None, None);
        let mut probe = self.compare.as_ref().map(|p| p.lock().unwrap_or_else(|e| e.into_inner()));
        let mut d = self.device.lock().map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d.begin_external_boundary(&mut DeviceMemory(mem)).map_err(|_| Keep::Unsupported("could not drain light state"))? {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if d.bound_target() != view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let binding = self.gel_binding(mem, serial, shader_gel, &constants(&light)?)?;
        let (plan, count) = self.prepare_light(&d, mem, &light, sorted, split_enabled, binding)?;
        if Self::probe(&mut d, mem, &plan, probe.as_deref_mut()) {
            let rectangle: [Option<f32>; 4] = std::array::from_fn(|i|
                mem.u32(engine::light_constants::global::RECTANGLE + 4 * i as u32).ok().map(f32::from_bits));
            eprintln!("native-lights compare: projected [left,right,top,bottom] {rectangle:?}, guest scissor [x,y,w,h] {:?}, planned {count}/{} sorted dispatches",
                plan.scissor, sorted.entries.len());
            for (i, e) in sorted.entries.iter().enumerate() {
                eprintln!("  dispatch[{i}] slot {:#x} item {:#x} part {:?} impl {} {}",
                    e.slot, e.handler_item, e.geometry.map(|g| g.part), e.implementation,
                    if i < count { "native" } else { "D3D" });
            }
            return Err(Keep::Unsupported("D3D comparison sample"));
        }
        let c = &plan.constants;
        let layers = mem.u32(feature::native_lights::LIGHT_LIST + 8).ok()
            .and_then(|mask| native_render::light_list::single_layer(mask, c.flags, c.definition_flags).ok());
        if layers != plan.draws.first().map(|p| p.layer as u32) {
            return Err(Keep::Unsupported("active light layers changed during setup"));
        }
        if mem.u32(engine::light_passes::global::PASS_FLAGS).ok() != Some(c.flags) ||
            c.frame.iter().enumerate().any(|(i, word)|
                mem.u32(engine::light_constants::global::FRAME + 4 * i as u32).ok() != Some(*word)) {
            return Err(Keep::Unsupported("light globals differ from the prepared frame or flags"));
        }
        let split = count != sorted.entries.len();
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let gel = plan.constants.gel;
        let targets = d.stats().target_changes;
        let isolated = split || plan.binding.is_some();
        let key = order.key(serial, gel, targets, isolated);
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let mut request = d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights, view, ranges: count,
            slots: Vec::new(),
        };
        let gel_slot = plan.binding.map(|binding| binding.append(&mut request));
        let reason = Arc::new(Mutex::new(None));
        let pass = RunPass { light: plan, view: None, clip: None, key, gel_slot,
            bundles: [None, None], refusal: Arc::clone(&reason) };
        let reservation = d.prepare_external_pass(&mut DeviceMemory(mem), request, Box::new(pass))
            .map_err(|_| Keep::Unsupported("could not drain or confirm the light"))?
            .ok_or_else(|| reason.lock().unwrap_or_else(|e| e.into_inner()).take()
                .unwrap_or(Keep::Unsupported("target binding, validation or range count refused the light")))?;
        // On a write fault the reservation drops all copies. After the
        // cursor moves the only remaining operation is infallible commit.
        if !sorted.consume(count, esi, |addr, value| mem.set_u32(addr, value).is_ok()) {
            timeline::native::Stage::Lights.count(timeline::native::Metric::RefusedMasks, count as u64);
            return Err(Keep::Unsupported("cannot consume sorted light entries"));
        }
        reservation.commit();
        timeline::native::Stage::Lights.count(timeline::native::Metric::Suppressed, count as u64);
        order.previous = (!isolated).then_some((serial, gel, targets, key));
        Ok(split)
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>, esi: &mut u32) -> bool {
        let shader_gel = dispatch.shader_gel.is_some();
        let result = self.reserve(mem, dispatch, esi);
        let ok = result.is_ok();
        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        if shader_gel && ok { t.shader_gels += 1; }
        if matches!(&result, Ok(true)) { t.split += 1; }
        for msg in t.count(result.map(|_| ())) { eprintln!("{msg}"); }
        if t.asked == 1 || (ok && t.planned.is_power_of_two()) || (ok && shader_gel && t.shader_gels == 1) || t.asked % SUMMARY_EVERY == 0 {
            let [reference, display] = d3d::GfxBackend::native_bundle_passes();
            eprintln!("native-lights: {} admitted, {} split, {} shader gels, {} reference passes, {} scaled passes, {} worker round trips",
                t.planned, t.split, t.shader_gels, reference, display,
                d3d::threaded::LIGHT_WORKER_WAITS.load(Ordering::Relaxed));
        }
        ok
    }

    fn end_layer(&self, mem: &mut dyn GuestMemory) {
        if mem.u32(native_render::skip::CURRENT_LIST).ok() == Some(feature::native_lights::LIGHT_LIST)
            && let Ok(layer) = mem.u32(native_render::skip::CURRENT_LAYER)
        {
            self.end_probe(Some(layer as usize), Some(mem));
        }
    }

    fn checkpoint_gel(&self, serial: u64) {
        *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner()) = Some((serial, self.gels.checkpoint()));
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }
}

fn uses_gel_target(mem: &dyn GuestMemory, definition: u32, shader: u32, flags: u32, definition_flags: u32) -> bool {
    use native_render::gels;
    shader != u32::MAX && definition != 0
        && flags & engine::light_passes::flag::CONTEXT_ZERO == 0
        && definition_flags & engine::light_passes::DEFINITION_TEXTURE_18 == 0
        && mem.u8(gels::BUILDING).ok() == Some(0)
        && mem.u32(gels::DEFINITION).ok() == Some(definition)
        && definition.checked_add(engine::light_passes::DEFINITION_GEL_SHADER)
            .and_then(|a| mem.u32(a).ok()) == Some(shader)
}

fn constants(light: &Light) -> Result<LightConstants, Keep> {
    let c = light.constants.ok_or(Keep::NoConstants)?;
    if c.shadowed() {
        return Err(Keep::Shadowed);
    }
    Ok(c)
}

type ClusterParts = HashMap<(Owner, u8), (TagId, WorldDraw)>;

/// Only a prefix can precede the untouched D3D suffix. A later native
/// match must never move ahead of a fallback draw (even the same shader).
fn select_parts(clusters: &ClusterParts, items: &[SortedItem], split: bool)
    -> Result<Vec<(usize, usize, WorldDraw)>, Keep> {
    let mut selected = Vec::new();
    for entry in items {
        let part = entry.geometry.ok_or("gathered model or unsupported geometry handler").and_then(|item| {
            match clusters.get(&item.part) {
                Some((shader, d)) if shader.0 == item.shader =>
                    Ok((item.layers.trailing_zeros() as usize, usize::from(entry.implementation), *d)),
                Some(_) => Err("gathered shader datum differs from the cluster material"),
                None => Err("gathered part has no native cluster draw"),
            }
        });
        match part {
            Ok(p) => selected.push(p),
            Err(_) if split && !selected.is_empty() => break,
            Err(why) => return Err(Keep::Unsupported(why)),
        }
    }
    Ok(selected)
}

#[derive(Default)]
struct RunOrder {
    previous: Option<(u64, Option<u32>, u64, u64)>,
}
impl RunOrder {
    fn key(&self, serial: u64, gel: Option<u32>, targets: u64, split: bool) -> u64 {
        match self.previous {
            Some((last, previous_gel, previous_targets, key)) if targets == previous_targets && !split && last.checked_add(1) == Some(serial) && previous_gel == gel => key,
            _ => serial,
        }
    }
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
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State, gels: Arc<Outputs>) {
    feature::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state, gels, checkpoint: Mutex::default(),
        tally: Mutex::new(Tally::default()),
        order: Mutex::default(),
        compare: (std::env::var("H2_NATIVE_LIGHTS_COMPARE").as_deref() == Ok("1")).then(Mutex::default),
    }));
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
    fn resolve_parts(&self, c: &native_render::lights::LightConstants, parts: Vec<(usize, usize, WorldDraw)>, produced_gel: bool)
        -> Result<Vec<Planned>, Keep>
    {
        // Resolved without the colours, which each light's uniforms take
        // (`colour_uniforms`): a light whose colour animates keeps its key.
        let inputs = LightInputs::from_constants(c).without_colours();
        let key = inputs.key();
        let mut out: Vec<Planned> = Vec::new();
        let mut r = self.resolver.lock().unwrap_or_else(|e| e.into_inner());
        let Resolver { lib, cache } = &mut *r;
        for (layer, imp, draw) in parts {
            let m = draw.material;
            let end = draw.first_index.checked_add(draw.index_count).ok_or(Keep::Unsupported("invalid cluster index range"))?;
            let range = draw.first_index..end;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            let made = cache.get_or_make(lit.shader, layer, imp, key, || self.resolve(lib, lit, &inputs, layer, imp));
            match made {
                Lit::Nothing => return Err(Keep::Unsupported("built light entry has no native material pass")),
                Lit::Refused(why) => return Err(Keep::Refused(why)),
                Lit::Draw(compiled) => {
                    if compiled.program.inputs & !draw.inputs != 0 {
                        return Err(Keep::Unsupported("cluster lacks a vertex stream read by its light program"));
                    }
                    self.binds(&compiled.program, c.gel, produced_gel)?;
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
        Ok(out)
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
    fn binds(&self, program: &LightProgram, gel: Option<u32>, produced_gel: bool) -> Result<(), Keep> {
        for (stage, (s, d)) in program.stages.iter().zip(&program.dims).enumerate() {
            if d.is_none() {
                continue;
            }
            let (loaded, what) = match *s {
                LightStage::Empty => (false, "empty stage"),
                LightStage::Bitmap { bitmap, image } => (self.images.contains_key(&(stage, bitmap, image)), "stage bitmap"),
                LightStage::Normalization => (self.normalization.is_some(), "normalization cube"),
                LightStage::Attenuation => (self.attenuation.is_some(), "attenuation volume"),
                LightStage::Gel => (produced_gel || gel.is_some_and(|g| self.gels.contains_key(&g)), "gel"),
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
    for d in &world.draws {
        match lit_of.get(&d.material).copied().flatten() {
            Some(true) if native(d.material) && matches!(d.owner, Owner::Cluster(_)) => {
                clusters.insert((d.owner, d.part), (shaders[usize::from(d.material)], *d));
            }
            Some(false) => {}
            Some(true) => {
                why[0] += 1;
            }
            None => {
                why[1] += 1;
            }
        }
    }
    for t in &world.transparents {
        match lit_of.get(&t.material).copied().flatten() {
            Some(false) => {}
            Some(true) => {
                why[2] += 1;
            }
            None => {
                why[3] += 1;
            }
        }
    }
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
    scissor: [u32; 4],
    binding: Option<Binding>,
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
            gel_view: None,
            gel_scale: self.binding.map(Binding::tex_scale),
            ranges: p.ranges.clone(),
        }
    }
}

/// Intersect in guest pixels, after the guest projected and rounded its
/// light rectangle. Use half-open edges, including offset viewports.
fn intersect_scissor([x,y,w,h]: [u32;4], (vx,vy,vw,vh): (u32,u32,u32,u32)) -> [u32;4] {
    let (left, top) = (x.max(vx), y.max(vy));
    [left, top, x.saturating_add(w).min(vx.saturating_add(vw)).saturating_sub(left),
        y.saturating_add(h).min(vy.saturating_add(vh)).saturating_sub(top)]
}

fn scale_scissor(rect: [u32;4], guest: (u32,u32), target: (u32,u32)) -> Option<[u32;4]> {
    let scale = target.0.checked_div(guest.0)?;
    if scale == 0 || guest.0.checked_mul(scale)? != target.0 || guest.1 == 0 ||
        guest.1.checked_mul(scale)? != target.1 { return None; }
    let [x,y,w,h] = rect;
    if w == 0 || h == 0 { return Some([0;4]); }
    if x.checked_add(w)? > guest.0 || y.checked_add(h)? > guest.1 { return None; }
    Some([x * scale, y * scale, w * scale, h * scale])
}

struct RunPass {
    light: PreparedLight,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    key: u64,
    gel_slot: Option<usize>,
    bundles: [Option<(native_render::light_gpu::LightBundles, [u32; 4])>; 2],
    refusal: Arc<Mutex<Option<Keep>>>,
}

impl d3d::ExternalPass for RunPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self.light.draws.iter().any(|d| !d.compiled.program.state.color_writes.is_empty()),
            depth_stencil: self.light.draws.iter().any(|d| d.compiled.program.state.depth_write),
        }
    }

    fn record_bundles(&mut self, target: &mut d3d::ExternalTarget<'_>) -> Option<d3d::backend::ExternalBundles> {
        let (recorded, scissor) = self.bundles[usize::from(target.copy == d3d::TargetCopy::Display)].take()?;
        recorded.upload(&mut target.encoder_uploads());
        Some(d3d::backend::ExternalBundles { run: self.key, scissor, groups: recorded.groups, ranges: recorded.ranges })
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        let result = self.prepare_gpu(t);
        if let Err(why) = result {
            *self.refusal.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Keep::Refused(format!("{:?} copy: {why}", t.copy)));
        }
        result
    }
}

impl RunPass {
    fn prepare_gpu(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        let copy = usize::from(t.copy == d3d::TargetCopy::Display);
        if self.bundles[copy].is_some() { return Err("ambiguous target copy"); }
        let scissor = scale_scissor(self.light.scissor, t.guest_size, t.size).ok_or("unsupported scissor scale")?;
        let constants = self.view.ok_or("missing view constants")?;
        let clip = self.clip.filter(|c| c.w_buffered == Some(true)).ok_or("unknown W-buffer at issue")?;
        let view = ViewInputs { constants, guest_size: t.guest_size, size: t.size, z_scale: t.z_scale, w_clip: Some((clip.range, clip.clamp)) };
        let ready = &self.light.ready;
        let lw = ready.lights.as_ref().ok_or("world lights unavailable")?;
        let mut gpu = upload(ready, t);
        let gpu = gpu.as_mut().ok_or("world upload failed")?;
        if gpu.lights().is_none() {
            gpu.set_lights(LightSet::new(t.device, t.queue, lw.normalization.as_ref(), lw.attenuation.as_ref()));
        }
        // Resolve the pinned slot for this physical copy before any guest cursor moves.
        let sample = match &self.light.binding {
            Some(binding) => Some(binding.sample(t, self.gel_slot.ok_or("missing gel slot")?).ok_or("target 13 has no matching sample view on this copy")?),
            None => None,
        };
        let draws: Vec<_> = self.light.draws.iter().map(|p| {
            let mut draw = self.light.draw(p);
            draw.gel_view = sample;
            draw
        }).collect();
        let recorded = gpu.prepare_lights(
            t.device, t.queue, t.color_format, t.depth_format, &view, &draws,
            &mut |stage, s| match s {
                LightStage::Bitmap { bitmap, image } => lw.images.get(&(stage, bitmap, image)).cloned(),
                _ => None,
            },
            &mut |g| lw.gels.get(&g).cloned(),
        ).ok_or("incomplete GPU resources or ranges")?;
        self.bundles[copy] = Some((recorded, scissor));
        Ok(())
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
        let entry = |slot, d: WorldDraw, implementation| SortedItem {
            slot, handler_item: slot + 80, implementation,
            geometry: Some(native_render::light_list::Item { part: (d.owner, d.part), shader: shader.0, layers: 1 << 12 }),
        };
        let mut items = [entry(1, b, 2), entry(2, b, 3), entry(3, a, 1), entry(4, b, 1)];
        assert_eq!(select_parts(&clusters, &items, false).unwrap(),
            [(DIFFUSE_LAYER, 2, b), (DIFFUSE_LAYER, 3, b), (DIFFUSE_LAYER, 1, a), (DIFFUSE_LAYER, 1, b)]);
        items[2].geometry.as_mut().unwrap().shader ^= 1 << 24;
        assert_eq!(select_parts(&clusters, &items, false).unwrap_err(),
            Keep::Unsupported("gathered shader datum differs from the cluster material"));
        assert_eq!(select_parts(&clusters, &items, true).unwrap(), [(DIFFUSE_LAYER, 2, b), (DIFFUSE_LAYER, 3, b)]);
        items[0].geometry = None;
        assert!(select_parts(&clusters, &items, true).is_err(), "cannot move native draws before a D3D prefix");
    }

    #[test]
    fn light_clip_intersects_guest_viewport_before_scaling() {
        let clip = intersect_scissor([7, 4, 8, 10], (9, 2, 8, 9));
        assert_eq!(clip, [9, 4, 6, 7]);
        assert_eq!(scale_scissor(clip, (20, 15), (60, 45)), Some([27, 12, 18, 21]));
        assert_eq!(scale_scissor(clip, (20, 15), (60, 40)), None);
        let empty = intersect_scissor([17, 0, 2, 9], (9, 2, 8, 9));
        assert_eq!(scale_scissor(empty, (20, 15), (40, 30)), Some([0; 4]));
    }

    #[test]
    fn a_run_requires_adjacent_whole_lights_with_the_same_gel() {
        let order = RunOrder { previous: Some((5, Some(12), 9, 3)) };
        assert_eq!(order.key(6, Some(12), 9, false), 3);
        assert_eq!(order.key(6, Some(12), 11, false), 6); // away and back
        assert_eq!(order.key(7, Some(12), 9, false), 7); // refused light or view boundary
        assert_eq!(order.key(6, None, 9, false), 6);
        assert_eq!(order.key(6, Some(12), 9, true), 6);
        assert_eq!(RunOrder::default().key(7, None, 9, false), 7); // preceding split
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
    fn target_13_requires_the_current_light_and_engine_texture_10_selector() {
        use native_render::gels;
        let mut mem = FlatMemory::new(gels::DEFINITION, 0x1000);
        let definition = gels::DEFINITION + 0x400;
        let shader = 0xabcd_0005;
        mem.set_u32(gels::DEFINITION, definition).unwrap();
        mem.set_u32(definition + engine::light_passes::DEFINITION_GEL_SHADER, shader).unwrap();
        assert!(uses_gel_target(&mem, definition, shader, 0, 0));
        assert!(!uses_gel_target(&mem, definition, shader, engine::light_passes::flag::CONTEXT_ZERO, 0));
        assert!(!uses_gel_target(&mem, definition, shader, 0, engine::light_passes::DEFINITION_TEXTURE_18));
        assert!(!uses_gel_target(&mem, definition, shader + 1, 0, 0));
        assert!(!uses_gel_target(&mem, definition + 4, shader, 0, 0));
        mem.write(gels::BUILDING, &[1]).unwrap();
        assert!(!uses_gel_target(&mem, definition, shader, 0, 0));
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
        assert!(constants(&light(Some(LightConstants { animated_gel: true, ..c }))).unwrap().animated_gel);
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
