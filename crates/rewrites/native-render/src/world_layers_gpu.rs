//! Records opaque indexed geometry from an ordered, resolved draw. The
//! guest has already selected the live shader, streams and attachments.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalDraws, ExternalPass, ExternalPassRequest, ExternalTarget,
    ExternalWrites, ResolvedDraw,
};
use d3d::nv2a::{NativeDrawKey, NativeDrawMaterial, NativeDrawShader};
use gfx::compile::{Pending, PendingPipeline};
use gfx::pipeline::{PipelineKey, ShaderId, TargetFormats};
use gfx::program::{ProgramBindings, StageDims};
use gfx::repack::{FetchMode, RepackPlan, StreamPlan};
use gfx::texture::TextureKind;
use gfx::translate::Capabilities;
use gfx::renderer::VertexBuffers;
use gfx::{BoundGroup, RecordedDraw};
use gfx::vertex::{PrimitiveType, VertexLayout};
use shaders::{psh::PixelUniforms, vsh::VertexUniforms};

use crate::pipelines::{self, Pipelines};
use crate::program::scaled_scissor;
use crate::world_layers::{Geometry, Owner, Texaccum};

const CACHE_BUDGET: u64 = 64 * 1024 * 1024;
const SECTION_BYTES: u64 = 128 * 1024 * 1024;
const SECTION_WINDOWS: u64 = 4096;
const SECTION_FRAMES: u64 = 32;
const LAYOUT_PLANS: usize = 256;
const DRAW_SHADERS: usize = 1024;

static BUFFER_BYTES: mem_watch::Counter = mem_watch::Counter::new("native.world_buffer_cache_bytes");
static SECTIONS: mem_watch::Counter = mem_watch::Counter::new("native.world_section_windows");
static SECTION_PAYLOAD: mem_watch::Counter = mem_watch::Counter::new("native.world_section_payload_bytes");

pub struct Cache {
    pool_refusals: Mutex<HashMap<&'static str, u64>>,
    device: Mutex<Option<(wgpu::Device, DeviceCache)>>,
    fetch: FetchMode,
}

impl Default for Cache {
    fn default() -> Self {
        Self { pool_refusals: Mutex::default(), device: Mutex::default(), fetch: FetchMode::Gpu }
    }
}

struct DeviceCache {
    bindings: ProgramBindings,
    shaders: HashMap<NativeDrawKey, Shader>,
    layouts: LayoutPlans,
    buffers: HashMap<BufferKey, (wgpu::Buffer, u64)>,
    buffer_bytes: mem_watch::Gauge,
    section_count: mem_watch::Gauge,
    section_bytes: mem_watch::Gauge,
    trimmed_at: u64,
    frame: u64,
    scope: Option<(u64, u32)>,
    sections: HashMap<SectionKey, Vec<ResidentSection>>,
    /// The scratch copies of the bound colour a pool's lens samples, one per
    /// target copy (reference, display).
    lens_copies: [Option<wgpu::TextureView>; 2],
}

#[derive(Default)]
struct LayoutPlans {
    layouts: HashMap<VertexLayout, Vec<(Vec<u32>, Arc<RepackPlan>)>>,
    count: usize,
}

impl LayoutPlans {
    // A DeviceCache belongs to one fetch mode.
    fn get(&mut self, layout: &VertexLayout, streams: &[d3d::StreamBytes], fetch: FetchMode) -> Arc<RepackPlan> {
        if let Some((_, plan)) = self.layouts.get(layout).and_then(|plans| {
            plans.iter().find(|(strides, _)| strides.iter().copied().eq(streams.iter().map(|s| s.stride)))
        }) {
            return Arc::clone(plan);
        }
        if self.count == LAYOUT_PLANS {
            self.layouts.clear();
            self.count = 0;
        }
        let strides: Vec<_> = streams.iter().map(|s| s.stride).collect();
        let plan = Arc::new(RepackPlan::with_mode(layout, &strides, fetch));
        self.layouts.entry(layout.clone()).or_default().push((strides, Arc::clone(&plan)));
        self.count += 1;
        plan
    }
}

struct TranslatedShader {
    linked: NativeDrawShader,
    module: wgpu::ShaderModule,
}

enum Shader {
    Translating { id: ShaderId, work: Arc<Pending<Option<TranslatedShader>>> },
    Ready(Program),
}

#[derive(Clone)]
struct Program {
    id: ShaderId,
    pipelines: Pipelines,
    dims: StageDims,
    discards: bool,
}

type BufferRange = (wgpu::Buffer, u64, u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BufferKey {
    RawVertex(u64, u32, u32),
    RepackedVertex(u64, u32, StreamPlan),
    Indices(u64),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SectionOwner {
    Cluster(u32),
    Placement(u32),
    Model(u32, u32),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SectionKey {
    Sky { model: u32, section: u32, part: u32, shader: u32, pass: u32 },
    World {
        owner: SectionOwner,
        part: u8,
        shader: u32,
        variant: u8,
        implementation: u8,
        layer: u32,
    },
}

impl SectionKey {
    fn new(geometry: Geometry, layer: u32) -> Self {
        let owner = match geometry.owner {
            Owner::Cluster(i) => SectionOwner::Cluster(i),
            Owner::Placement(i) => SectionOwner::Placement(i),
            Owner::Object { model, section, .. } => SectionOwner::Model(model, section),
        };
        Self::World { owner, part: geometry.part, shader: geometry.shader, variant: geometry.variant,
            implementation: geometry.implementation, layer }
    }
}

struct ResidentSection {
    last_used: u64,
    source: Arc<ResolvedDraw>,
    fetch: FetchMode,
    repack: Arc<RepackPlan>,
    streams: VertexBuffers,
    indices: BufferRange,
    material: Option<ResidentMaterial>,
    charged_bytes: u64,
}

fn retain_windows(windows: &mut Vec<ResidentSection>, oldest: u64, count: &mut u64, bytes: &mut u64) {
    windows.retain(|section| {
        if section.last_used >= oldest {
            return true;
        }
        *count -= 1;
        *bytes -= section.charged_bytes;
        false
    });
}

struct ResidentMaterial {
    source: Arc<ResolvedDraw>,
    present: [bool; 4],
    program: Arc<NativeDrawMaterial>,
    shader: Program,
    // Pin the material/lightmap bind group for each live target copy.
    groups: Vec<([Option<wgpu::TextureView>; 4], Option<wgpu::BindGroup>)>,
    // Reference and display may require different attachment formats.
    pipelines: HashMap<TargetFormats, (PipelineKey, Arc<PendingPipeline>)>,
}

impl ResidentSection {
    fn matches(&self, draw: &ResolvedDraw, fetch: FetchMode) -> bool {
        let old = &self.source;
        let same_snapshot = |a: Option<u64>, b| a.is_some() && a == b;
        if std::ptr::eq(self.source.as_ref(), draw) { return self.fetch == fetch; }
        self.fetch == fetch && old.vertex_count == draw.vertex_count && old.layout == draw.layout
            && old.streams.len() == draw.streams.len()
            && old.streams.iter().zip(&draw.streams).all(|(a, b)| a.stride == b.stride
                && (same_snapshot(a.data.cache_id(), b.data.cache_id()) || a.data == b.data))
            && old.indices.as_ref().zip(draw.indices.as_ref()).is_some_and(|(a, b)|
                same_snapshot(a.cache_id(), b.cache_id()) || a == b)
    }
}

impl ResidentMaterial {
    // All shader and pipeline selection inputs. Vertex registers and stage
    // views are supplied live, including relocated lightmaps and texaccum.
    fn matches(&self, draw: &ResolvedDraw, present: [bool; 4]) -> bool {
        let old = &self.source;
        let (a, b) = (&old.shader, &draw.shader);
        self.present == present && old.primitive == draw.primitive && old.raw == draw.raw
            && old.state == draw.state && old.extras == draw.extras
            && d3d::nv2a::w_buffered(old) == d3d::nv2a::w_buffered(draw)
            && old.hw.stipple_control == draw.hw.stipple_control
            && old.texture_info == draw.texture_info && old.target == draw.target
            && old.glyph_cache == draw.glyph_cache && a.vertex == b.vertex
            && a.provided_inputs == b.provided_inputs && a.pixel_shader == b.pixel_shader
            && a.combiner_state == b.combiner_state && a.final_combiner_latch == b.final_combiner_latch
            && a.affine_projection == b.affine_projection
    }
}

impl BufferKey {
    fn vertex(id: u64, plan: &StreamPlan, count: u32) -> Self {
        if plan.identity {
            Self::RawVertex(id, plan.guest_stride, count)
        } else {
            Self::RepackedVertex(id, count, plan.clone())
        }
    }
}

impl DeviceCache {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            bindings: ProgramBindings::new(device),
            shaders: HashMap::new(),
            layouts: LayoutPlans::default(),
            buffers: HashMap::new(),
            buffer_bytes: BUFFER_BYTES.track(0),
            section_count: SECTIONS.track(0),
            section_bytes: SECTION_PAYLOAD.track(0),
            trimmed_at: 0,
            frame: 0, scope: None, sections: HashMap::new(), lens_copies: [None, None],
        }
    }

    fn resolve_shader(
        &mut self, device: &wgpu::Device, draw: &ResolvedDraw, present: [bool; 4],
        plan: &RepackPlan, caps: Capabilities,
    ) -> Result<(Arc<NativeDrawMaterial>, Program), &'static str> {
        let program = NativeDrawMaterial::resolve_with_plan(draw, present, caps, plan)
            .map_err(|_| "unresolved indexed material")?;
        let program = Arc::new(program);
        if self.shaders.len() == DRAW_SHADERS && !self.shaders.contains_key(&program.key) {
            self.shaders.clear();
        }
        let new_id = ShaderId(self.shaders.len().try_into().map_err(|_| "shader id overflow")?);
        let shader = self.shaders.entry(program.key.clone()).or_insert_with_key(|key| {
            let key = key.clone();
            let device = device.clone();
            let work = pipelines::background(move || {
                let linked = key.translate().ok()?;
                let module = gfx::program::validated_resource(&device, || {
                    device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("opaque indexed program"),
                        source: wgpu::ShaderSource::Wgsl(linked.source.as_str().into()),
                    })
                })?;
                Some(TranslatedShader { linked, module })
            });
            Shader::Translating { id: new_id, work }
        });
        if let Shader::Translating { id, work } = shader {
            let translated = work.try_get().ok_or("shader compiling")?;
            let TranslatedShader { linked, module } = translated.as_ref().ok_or("shader translation failed")?;
            // These layouts must also own the texture and uniform bindings at draw time.
            let bindings = self.bindings.program(device, module.clone(), linked.dims);
            *shader = Shader::Ready(Program {
                id: *id,
                pipelines: Pipelines::new(device, &linked.source, bindings, "native opaque layer"),
                dims: linked.dims,
                discards: linked.discards,
            });
        }
        let Shader::Ready(shader) = shader else { return Err("shader unavailable") };
        Ok((program, shader.clone()))
    }

    fn upload_section(
        &mut self, target: &ExternalAttachments, draw: &Arc<ResolvedDraw>,
        fetch: FetchMode, repack: Arc<RepackPlan>, charged_bytes: Option<u64>,
    ) -> Option<ResidentSection> {
        let resident = charged_bytes.is_some();
        let indices = draw.indices.as_ref()?;
        if resident && indices.iter().any(|&index| index >= draw.vertex_count) { return None; }
        let mut streams = VertexBuffers::with_capacity(draw.streams.len());
        for (plan, stream) in repack.streams.iter().zip(&draw.streams) {
            if plan.identity {
                plan.check(&stream.data, draw.vertex_count).ok()?;
            }
            let key = stream.data.cache_id().map(|id| BufferKey::vertex(id, plan, draw.vertex_count));
            streams.push(self.buffer(target, key, wgpu::BufferUsages::VERTEX, resident, || {
                plan.repack(&stream.data, draw.vertex_count).ok()
            })?);
        }
        let indices = self.buffer(target, indices.cache_id().map(BufferKey::Indices),
            wgpu::BufferUsages::INDEX, resident, || Some(Cow::Borrowed(bytemuck::cast_slice(&indices[..]))))?;
        Some(ResidentSection {
            last_used: self.frame, source: Arc::clone(draw), fetch, repack, streams, indices,
            material: None, charged_bytes: charged_bytes.unwrap_or(0),
        })
    }

    fn trim_sections(&mut self) {
        let frame = self.frame;
        if frame.saturating_sub(self.trimmed_at) < SECTION_FRAMES {
            return;
        }
        self.trimmed_at = frame;
        let (mut count, mut bytes) = (self.section_count.get(), self.section_bytes.get());
        self.sections.retain(|_, windows| {
            retain_windows(windows, frame.saturating_sub(SECTION_FRAMES), &mut count, &mut bytes);
            !windows.is_empty()
        });
        self.section_count.set(count);
        self.section_bytes.set(bytes);
    }

    fn buffer<'a>(
        &mut self,
        target: &ExternalAttachments,
        key: Option<BufferKey>,
        usage: wgpu::BufferUsages,
        resident: bool,
        bytes: impl FnOnce() -> Option<Cow<'a, [u8]>>,
    ) -> Option<BufferRange> {
        if let Some((buffer, used)) = key.as_ref().and_then(|k| self.buffers.get_mut(k)) {
            *used = self.frame;
            return Some((buffer.clone(), 0, buffer.size()));
        }
        let bytes = bytes()?;
        let len = bytes.len() as u64;
        let device = &target.device;
        if len == 0 || len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) > device.limits().max_buffer_size {
            return None;
        }
        let key = key.filter(|_| len < CACHE_BUDGET);
        if key.is_none() && !resident {
            if len.next_power_of_two() > device.limits().max_buffer_size {
                return None;
            }
            let (buffer, offset) = target.uploads.stage(device, &target.queue, &bytes);
            return Some((buffer, offset, len));
        }
        let buffer = gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
            label: Some("opaque indexed geometry"),
            contents: &bytes,
            usage,
        });
        if let Some(key) = key {
            let mut bytes = self.buffer_bytes.get() + buffer.size();
            if bytes > CACHE_BUDGET {
                // Section windows are budgeted separately. Trim the shared
                // snapshot index, oldest sightings first.
                let mut age: Vec<_> = self.buffers.iter().map(|(k, (_, used))| (*used, k.clone())).collect();
                age.sort_unstable_by_key(|(used, _)| *used);
                for (_, old) in age {
                    if bytes <= CACHE_BUDGET * 3 / 4 { break; }
                    if let Some((retired, _)) = self.buffers.remove(&old) { bytes -= retired.size(); }
                }
            }
            self.buffer_bytes.set(bytes);
            self.buffers.insert(key, (buffer.clone(), self.frame));
        }
        Some((buffer, 0, len))
    }
}

fn section_charge(count: u64, used: u64, payload: u64) -> Option<u64> {
    (count < SECTION_WINDOWS && payload <= SECTION_BYTES.saturating_sub(used)).then_some(payload)
}

/// Section node matrices in hardware register order, scoped to one sky draw,
/// and the material's constants evaluated from the plan
/// (`crate::sky_material`).
#[derive(Clone)]
struct SkyRows {
    nodes: Arc<[frame_scene::Row]>,
    decompression: [frame_scene::Row; 3],
    material: Arc<crate::sky_material::SkyConstants>,
}

impl SkyRows {
    /// The draw holds the view's rows and the evaluated material: each of
    /// its constant sets the rows, and the bound constants every register
    /// the material writes (`constants` where one differs).
    fn holds(&self, draw: &ResolvedDraw) -> Result<(), &'static str> {
        self.check(&draw.shader.constants[..])?;
        let combiner = &draw.shader.combiner_state[..];
        if self.material.difference(&draw.shader.constants[..], combiner).is_some() {
            return Err("constants");
        }
        if let Some((constants, _)) = &draw.retail_constants {
            self.check(&constants[..])?;
            if self.material.difference(&constants[..], combiner).is_some() {
                return Err("constants");
            }
        }
        Ok(())
    }

    fn check(&self, constants: &[frame_scene::Row]) -> Result<(), &'static str> {
        let start = crate::program::MODEL_ROWS;
        let compression = crate::program::POSITION_DECOMPRESSION;
        let same = |at: usize, rows: &[frame_scene::Row]| {
            constants.get(at..at + rows.len()).is_some_and(|live|
                live.iter().zip(rows).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)))
        };
        if self.nodes.is_empty() || self.nodes.len() % 3 != 0
            || self.nodes.len() > compression - start || !same(start, &self.nodes)
        {
            return Err("transform");
        }
        if !same(compression, &self.decompression) {
            return Err("decompression");
        }
        Ok(())
    }

    /// The view's rows and the evaluated material over the draw's
    /// uniforms: the pass draws the plan's constants.
    fn apply(&self, vertex: &mut VertexUniforms, pixel: &mut PixelUniforms) {
        let start = crate::program::MODEL_ROWS;
        vertex.constants[start..start + self.nodes.len()].copy_from_slice(&self.nodes);
        let compression = crate::program::POSITION_DECOMPRESSION;
        vertex.constants[compression..compression + 3].copy_from_slice(&self.decompression);
        self.material.apply(&mut vertex.constants[..], &mut pixel.c0, &mut pixel.c1);
    }
}

/// A texture backed by an independently pinned colour surface. Both
/// identities must name the same GPU texture on every target copy.
#[derive(Clone, Copy)]
struct SurfaceTexture {
    header: u32,
    size: (u32, u32),
}

/// One indexed range recorded at its original attachments.
pub struct IndexedPass {
    sorted_pool: bool,
    /// The stage of a sorted pool that samples the frame behind it (its
    /// refraction lens): the recording binds a copy of the bound colour
    /// there ([`IndexedPass::record_pool`]).
    lens: Option<usize>,
    /// Whether a recording of this pass was made ([`Drop`] counts a pool that never was).
    recorded: std::sync::atomic::AtomicBool,
    sky: Option<SkyRows>,
    draw: Arc<ResolvedDraw>,
    cache: Arc<Cache>,
    frame: u64,
    // Eligibility is fixed at guest extraction even if the render FIFO
    // records the candidate after a later swap.
    extracted_frame: u64,
    fetch: FetchMode,
    pinned: Option<SurfaceTexture>,
    sorted_layer: bool,
    section: Option<((u64, u32), SectionKey)>,
    /// A sky strip replayed with `0x0003ede0` passed over
    /// ([`IndexedPass::sky_replay`]): no guest textures are bound, so the
    /// stages are the resident material's, pinned when the strip was
    /// last admitted.
    replay: bool,
}

impl IndexedPass {
    pub fn new(
        draw: &ResolvedDraw,
        cache: Arc<Cache>,
        frame: u64,
        texaccum: Option<Texaccum>,
    ) -> Option<Self> {
        if draw.indices.as_ref()?.iter().any(|&i| i >= draw.vertex_count) { return None; }
        Self::candidate(draw, cache, frame, texaccum, None, false)
    }

    /// A standalone sorted-layer range can be recorded on the scene's
    /// attachments with the exact texture views captured for this packet.
    pub fn for_sorted_layer(draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64) -> Option<Self> {
        if draw.indices.as_ref()?.iter().any(|&i| i >= draw.vertex_count) {
            return None;
        }
        Self::candidate(draw, cache, frame, None, None, true)
    }

    /// The sorted handler resolved the ripple frame, bump matrices, reflection,
    /// refraction sample and fog. The pool draws over the scene the attachments
    /// hold: it tests (and never writes) their depth, and its lens samples a
    /// copy of the bound colour made in each recording, not the guest's sample
    /// of an earlier frame state. Each input that does not hold is a named
    /// refusal ([`pool_refusal_counter`] names its counter) and the guest's
    /// D3D draw stands.
    pub fn for_pool(draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64) -> Result<Self, &'static str> {
        use gfx::state::{CompareFunc, DepthBufferMode};
        let render = &draw.state.render;
        if draw.target.depth_format.is_none() {
            return Err("no_depth");
        }
        if render.depth_mode == DepthBufferMode::Disabled
            || matches!(render.depth_func, CompareFunc::Always | CompareFunc::Never)
        {
            return Err("depth_not_tested");
        }
        if draw.writes_depth_stencil() {
            return Err("writes_depth");
        }
        let Some(lens) = pool_lens_stage(draw) else {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-water: sorted pool has no stage sampling the frame: {}", pool_lens_report(draw));
            }
            return Err("no_lens");
        };
        static LENS_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LENS_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("native-water: sorted pool lens at t{lens}: {}", pool_lens_report(draw));
        }
        let mut pass = Self::new(draw, cache, frame, None).ok_or("geometry")?;
        pass.sorted_pool = true;
        pass.lens = Some(lens);
        Ok(pass)
    }

    /// Layer 15, one material pass of one section part. The live pass
    /// supplies its selected program, evaluated constants and stage images.
    /// The node palette is the sky model's for the current view. With
    /// `resident` (`SkySection::resident`: the strip's draw over the load's
    /// geometry), the `SectionKey::Sky` entry is uploaded from the load's
    /// snapshots, one per part for the BSP, not from the draw's streams.
    pub fn for_sky(
        draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64, generation: u64,
        item: scene_extract::layout::sky::SkyItem, constants: crate::sky_material::SkyConstants,
        resident: Option<ResolvedDraw>,
    ) -> Result<Self, &'static str> {
        // Each refusal is the strip's `scene_kept_sky_<cause>`
        // (`render_bridge::native_sky`). The cull mode is the draw's own:
        // the pipeline takes it from the resolved state and the strip's
        // winding from its index order, as the guest's draw does.
        if draw.primitive != PrimitiveType::TriangleStrip {
            return Err("topology");
        }
        if draw.indices.as_ref().is_none_or(|indices| indices.iter().any(|&i| i >= draw.vertex_count)) {
            return Err("index_range");
        }
        // The stencil is the draw's own as well: the pipeline key and the
        // packet's reference come from the resolved state, so a strip
        // drawn with the scene's mark (mode 2, `0x0002c0bc`) writes it and
        // a strip whose state leaves the test off writes none, as on D3D.
        // A target with no stencil (no depth, or a `D16` one) draws with
        // the test off, as the D3D path's pipeline takes it
        // (`gfx::translate::depth_stencil_state`).
        if !matches!(&draw.shader.vertex, d3d::shader::VertexProcessing::Program { .. }) {
            return Err("program");
        }
        if draw.sampled_texture_stages().iter().enumerate().any(|(n, &reads)|
            reads && (draw.textures[n].is_none() || draw.texture_info[n].is_none()))
        {
            return Err("texture");
        }
        let sky = SkyRows { nodes: item.rows.into(), decompression: item.decompression, material: Arc::new(constants) };
        sky.holds(draw)?;
        let key = SectionKey::Sky {
            model: item.model, section: item.section, part: item.part,
            shader: item.shader, pass: item.pass,
        };
        let source = resident.as_ref().unwrap_or(draw);
        let mut candidate = Self::candidate(source, cache, frame, None, Some(((generation, 0), key)), false)
            .ok_or("candidate")?;
        candidate.sky = Some(sky);
        Ok(candidate)
    }

    /// This admitted sky strip, kept for the cut to replay
    /// ([`Self::sky_replay`]); `None` for any other pass.
    pub fn sky_template(&self) -> Option<SkyTemplate> {
        self.sky.as_ref()?;
        Some(SkyTemplate { draw: Arc::clone(&self.draw), cache: Arc::clone(&self.cache), section: self.section? })
    }

    /// The strip of `template` drawn again with `0x0003ede0` passed over
    /// (docs/graphics/retire-d3d-r5.md, "The sky's cut"): the made rows,
    /// the section's decompression and the evaluated material over the
    /// strip's draw, its geometry, program, pipeline and stage views the
    /// resident `SectionKey::Sky` entry's. The view block comes from the
    /// device at the issue ([`SkyCut`]).
    pub fn sky_replay(
        template: &SkyTemplate, frame: u64, rows: Vec<frame_scene::Row>, decompression: [frame_scene::Row; 3],
        constants: crate::sky_material::SkyConstants,
    ) -> Result<Self, &'static str> {
        let room = crate::program::POSITION_DECOMPRESSION - crate::program::MODEL_ROWS;
        if rows.is_empty() || rows.len() % 3 != 0 || rows.len() > room {
            return Err("transform");
        }
        let mut pass = Self::candidate(&template.draw, Arc::clone(&template.cache), frame, None, Some(template.section), false)
            .ok_or("candidate")?;
        pass.sky = Some(SkyRows { nodes: rows.into(), decompression, material: Arc::new(constants) });
        pass.replay = true;
        Ok(pass)
    }

    /// Sorted slots, render records, object handles and swap counters are
    /// transient. Validate the index range once with the matching geometry.
    pub fn for_section(
        draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64, texaccum: Option<Texaccum>,
        scope: (u64, u32), geometry: Geometry, layer: u32,
    ) -> Option<Self> {
        Self::candidate(draw, cache, frame, texaccum, Some((scope, SectionKey::new(geometry, layer))), false)
    }

    fn candidate(
        draw: &ResolvedDraw, cache: Arc<Cache>, frame: u64, texaccum: Option<Texaccum>,
        section: Option<((u64, u32), SectionKey)>,
        sorted_layer: bool,
    ) -> Option<Self> {
        let indices = draw.indices.as_ref()?;
        if indices.len() < 3
            || !matches!(
                draw.primitive,
                PrimitiveType::TriangleList | PrimitiveType::TriangleStrip
            )
            || (!sorted_layer && draw.target.depth_format.is_none())
            || draw.streams.len() != draw.layout.streams.len()
            || draw.hw.zpass_count_enable
        {
            return None;
        }
        Some(Self {
            sorted_pool: false,
            lens: None,
            recorded: Default::default(),
            sky: None,
            draw: Arc::new(draw.clone()),
            fetch: cache.fetch,
            cache,
            frame,
            extracted_frame: timeline::frames(),
            section,
            sorted_layer,
            pinned: texaccum.map(|t| SurfaceTexture { header: t.texture, size: t.size }),
            replay: false,
        })
    }

    /// Require the first external slot's surface to back the sampled
    /// stage. Used by generated lights whose target 13 is per invocation.
    pub fn pin_sampled_surface(&mut self, header: u32, size: (u32, u32)) {
        self.pinned = Some(SurfaceTexture { header, size });
    }

    fn prepare(
        &self,
        target: &ExternalAttachments,
        guest_size: (u32, u32),
    ) -> Result<ExternalDraws, &'static str> {
        self.prepare_with(target, guest_size, false).map(|(draws, _)| draws)
    }

    /// [`Self::prepare`], and with `lens` the pool's lens stage bound to the
    /// scratch copy of the bound colour, returned for the recording to fill
    /// before its draws.
    fn prepare_with(
        &self,
        target: &ExternalAttachments,
        guest_size: (u32, u32),
        lens: bool,
    ) -> Result<(ExternalDraws, Option<LensCopy>), &'static str> {
        let _scope = mem_watch::scope("native.world");
        // Both target copies retain the same geometry and material snapshot.
        let draw = &self.draw;
        let (width, height) = guest_size;
        let depth = draw.target.depth_format.map(|_| target.depth_format);
        if (target.targets.0 != target.targets.1) != depth.is_some() {
            return Err("shared color and depth");
        }
        let scale = d3d::scaled::RenderScale::between(guest_size, target.size).ok_or("zero raster extent")?;
        if (width, height) != (draw.target.width, draw.target.height)
            || depth.is_some_and(|_| target.size != (target.depth.texture().width(), target.depth.texture().height()))
        {
            return Err("incompatible target");
        }
        let caps = Capabilities::from_features(target.device.features());
        let mut active = self.cache.device.lock().map_err(|_| "device cache poisoned")?;
        if active.as_ref().is_none_or(|(device, _)| device != &target.device) {
            *active = Some((target.device.clone(), DeviceCache::new(&target.device)));
        }
        let (_, cache) = active.as_mut().ok_or("device cache unavailable")?;
        if cache.frame < self.frame {
            cache.frame = self.frame;
            cache.bindings.end_frame();
            cache.trim_sections();
        }

        let section_key = self.section.map(|(scope, key)| {
            if cache.scope != Some(scope) {
                cache.sections.clear();
                cache.buffers.clear();
                cache.buffer_bytes.set(0);
                cache.section_count.set(0);
                cache.section_bytes.set(0);
                cache.scope = Some(scope);
            }
            key
        });
        let mut stages = target.stages.clone();
        let mut lens_copy = None;
        if let Some(stage) = self.lens.filter(|_| lens) {
            let copy = pool_lens_copy(&target.device, &mut cache.lens_copies, target)?;
            stages[stage] = Some(copy.view.clone());
            lens_copy = Some(copy);
        }
        let present = stages.each_ref().map(Option::is_some);
        // Visibility can split one section part into several indexed windows.
        // Keep each live window: alternating ranges must not replace each other.
        let previous = section_key.and_then(|key| {
            let windows = cache.sections.get(&key)?;
            let i = windows.iter().position(|s| s.matches(draw, self.fetch))?;
            Some((key, i))
        });
        // A replay binds the stages the strip's resident material pinned.
        let (stages, present) = if self.replay {
            let pinned = previous.and_then(|(key, i)| cache.sections.get(&key)?[i].material.as_ref())
                .ok_or("replay material missing")?;
            (pinned.groups.last().ok_or("replay textures missing")?.0.clone(), pinned.present)
        } else {
            (stages, present)
        };
        let material = previous.and_then(|(key, i)| cache.sections.get(&key)?[i].material.as_ref())
            .filter(|m| m.matches(draw, present));
        let material_hit = material.is_some();
        let formats = TargetFormats { color: target.color_format, depth, samples: 1 };
        let cached_pipeline = material.and_then(|m| m.pipelines.get(&formats)).cloned();
        let repack = match previous {
            Some((key, i)) => Arc::clone(&cache.sections[&key][i].repack),
            None => cache.layouts.get(&draw.layout, &draw.streams, self.fetch),
        };
        let (program, shader) = if let Some(material) = material {
            (Arc::clone(&material.program), material.shader.clone())
        } else {
            cache.resolve_shader(&target.device, draw, present, &repack, caps)?
        };
        let dims = shader.dims;
        let mut consumed_surface = false;
        for (n, dim) in dims.iter().enumerate() {
            let Some(dim) = dim else { continue };
            let view = stages[n].as_ref().ok_or("sample view missing")?;
            let have = match draw.texture_info[n].ok_or("sample header missing")?.kind {
                TextureKind::D2 => wgpu::TextureViewDimension::D2,
                TextureKind::Volume => wgpu::TextureViewDimension::D3,
                TextureKind::Cube => wgpu::TextureViewDimension::Cube,
            };
            if *dim != have { return Err("sample dimension disagrees with shader"); }
            if view.texture() == target.color.texture()
                || (depth.is_some() && view.texture() == target.depth.texture())
            {
                return Err("sample aliases attachment");
            }
            if !gfx::program::filterable(view.texture().format(), target.device.features()) {
                return Err("sample is unfilterable");
            }
            if let Some(pinned) = self.pinned.filter(|p| draw.textures[n] == Some(p.header)) {
                consumed_surface = true;
                let info = draw.texture_info[n].ok_or("sample header missing")?;
                let sampled = target.sampled.first().and_then(Option::as_ref).ok_or("texaccum sample missing")?;
                let source = target.slots.first().and_then(Option::as_ref).ok_or("texaccum slot missing")?;
                if (info.width, info.height) != pinned.size
                    || info.format.layout() != gfx::format::Layout::Linear
                    || source.guest_size != pinned.size
                    || source.size != sampled.size
                    || source.sample.texture() != sampled.view.texture()
                    || source.sample.texture() != view.texture()
                {
                    return Err("texaccum binding mismatch");
                }
            }
        }
        // A bound but unsampled header cannot prove a surface dependency.
        if self.pinned.is_some() && !consumed_surface {
            return Err("unused texaccum binding");
        }
        // An unresolved native variant leaves this exact indexed range on D3D.
        // Check it before allocating streams, bind groups or submission uploads.
        let mut state = draw.state.render;
        if program.writes_depth {
            // The same offset is already in the translated W-depth uniforms.
            state.solid_offset_enable = false;
            state.wireframe_offset_enable = false;
            state.point_offset_enable = false;
        }
        let pipeline_hit = cached_pipeline.is_some();
        let (key, pipeline) = if let Some(cached) = cached_pipeline {
            cached
        } else {
            let mut key = PipelineKey::from_plan(
                shader.id,
                &repack,
                draw.primitive,
                &state,
                formats,
                caps,
            );
            if draw.primitive == PrimitiveType::TriangleStrip {
                key.primitive.strip_index_format = Some(wgpu::IndexFormat::Uint32);
            }
            let pipeline = shader.pipelines.request(&target.device, &key);
            (key, pipeline)
        };
        pipeline.visible_at_frame(self.extracted_frame)
            .ok_or("pipeline unavailable at extraction")?
            .as_ref().ok_or("pipeline compilation failed")?;
        let views = std::array::from_fn(|n| dims[n].and(stages[n].clone()));
        let cached_textures = previous.and_then(|(key, i)| cache.sections.get(&key)?[i].material.as_ref())
            .filter(|_| material_hit)
            .and_then(|m| m.groups.iter().find(|(stages, _)| stages == &views))
            .map(|(_, textures)| textures.clone());
        let bindings_hit = cached_textures.is_some();
        let textures = cached_textures.unwrap_or_else(|| cache.bindings.textures(
            &target.device,
            &target.queue,
            dims,
            views.each_ref().map(Option::as_ref),
            program.samplers,
            [[false; 4]; 4],
        ));
        if textures.is_none() && dims.iter().any(Option::is_some) {
            return Err("texture group unavailable");
        }

        let indices = draw.indices.as_ref().ok_or("indices missing")?;
        let index_count = indices.len().try_into().map_err(|_| "index count overflow")?;
        let frame = cache.frame;
        let charged_bytes = section_key.filter(|_| previous.is_none()).and_then(|key| {
            if let std::collections::hash_map::Entry::Occupied(mut entry) = cache.sections.entry(key) {
                // A streaming part should not keep each historical version
                // until the sweep for sections that disappeared entirely.
                let (mut count, mut bytes) = (cache.section_count.get(), cache.section_bytes.get());
                retain_windows(entry.get_mut(), frame.saturating_sub(1), &mut count, &mut bytes);
                if entry.get().is_empty() {
                    entry.remove();
                }
                cache.section_count.set(count);
                cache.section_bytes.set(bytes);
            }
            // Charge both the pinned snapshot and its uploaded buffers. Shared
            // streams can be counted twice, which keeps the budget conservative.
            let payload = indices.len() as u64 * 8 + repack.streams.iter().zip(&draw.streams)
                .map(|(plan, stream)| stream.data.len() as u64
                    + (plan.host_bytes(draw.vertex_count) as u64).next_multiple_of(4))
                .sum::<u64>();
            section_charge(cache.section_count.get(), cache.section_bytes.get(), payload)
        });
        let resident = previous.is_some() || charged_bytes.is_some();
        let mut fresh;
        let section = if let Some((key, i)) = previous {
            let section = &mut cache.sections.get_mut(&key).ok_or("resident section missing")?[i];
            section.last_used = frame;
            // Promote the current snapshot IDs after an identical first-use copy.
            section.source = Arc::clone(draw);
            if material_hit {
                section.material.as_mut().ok_or("resident material missing")?.source = Arc::clone(draw);
            }
            section
        } else {
            fresh = cache.upload_section(target, draw, self.fetch, repack, charged_bytes).ok_or("geometry upload failed")?;
            if let Some((key, bytes)) = section_key.zip(charged_bytes) {
                let windows = cache.sections.entry(key).or_default();
                windows.push(fresh);
                cache.section_count.set(cache.section_count.get() + 1);
                cache.section_bytes.set(cache.section_bytes.get() + bytes);
                windows.last_mut().ok_or("resident section missing")?
            } else { &mut fresh }
        };
        if resident {
            if !material_hit {
                section.material = Some(ResidentMaterial {
                    source: Arc::clone(draw), present, program: Arc::clone(&program),
                    shader: shader.clone(), pipelines: HashMap::new(), groups: Vec::new(),
                });
            }
            if !pipeline_hit {
                section.material.as_mut().ok_or("resident material missing")?.pipelines.insert(formats, (key.clone(), pipeline.clone()));
            }
        }
        if resident && !bindings_hit {
            let groups = &mut section.material.as_mut().ok_or("resident material missing")?.groups;
            if groups.len() == 2 { groups.remove(0); }
            groups.push((views, textures.clone()));
        }

        // Only the per-draw uniforms and scissors depend on the presentation
        // lens and UI fit. Keep their copy out of the resident cache.
        let copy = draw.for_copy(target.copy);
        let draw = copy.as_ref();
        let clip = if draw.clip_rects.is_empty() {
            std::slice::from_ref(draw.clip.as_ref().ok_or("clip unavailable")?)
        } else {
            &draw.clip_rects
        };
        let mut scissors = clip.iter().map(|r| scaled_scissor(&draw.display_rect(*r), scale, target.size));
        let first_scissor = scissors.next().flatten().ok_or("invalid clip rectangle")?;
        if scissors.clone().any(|scissor| scissor.is_none()) {
            return Err("invalid clip rectangle");
        }
        let streams = section.streams.clone();
        let indices = section.indices.clone();
        let (mut vertex, mut pixel) = program.uniforms(draw, scale);
        if let Some(rows) = &self.sky { rows.apply(&mut vertex, &mut pixel); }
        let sizes = [VertexUniforms::SIZE, PixelUniforms::SIZE];
        let fill = |_: usize, v: &mut [u8], p: &mut [u8]| {
            vertex.write_bytes(v);
            pixel.write_bytes(p);
        };
        let layout = cache.bindings.uniform_layout();
        let params = if resident {
            gfx::churn::native_cache(pipeline_hit && bindings_hit);
            target.uploads.resident_uniform_run(&target.device, &target.queue, layout, sizes, 1, fill)
        } else {
            target.uploads.uniform_run(&target.device, &target.queue, layout, sizes, 1, fill)
        }.ok_or("uniform upload failed")?;
        let offsets = params.offsets(0);
        let groups = std::iter::once(BoundGroup { index: 0, group: params.group, offsets: offsets.to_vec() })
            .chain(textures.map(|group| BoundGroup::new(1, group)));
        let first = RecordedDraw::prepared_indexed(
            &key, pipeline, streams, indices, index_count, groups, &state, target.size, first_scissor,
        );
        let mut packets = Vec::with_capacity(clip.len());
        packets.push(first);
        for scissor in scissors {
            packets.push(packets[0].clone().with_scissor(scissor.ok_or("invalid clip rectangle")?, target.size));
        }
        Ok((ExternalDraws { ranges: 1, packets, discards: shader.discards, writes: Vec::new() }, lens_copy))
    }
}

/// The scratch copy of the bound colour a pool's lens stage samples: copy
/// `from` into `to` (`extent`) before the pool draws, since an attachment
/// cannot be sampled as it is drawn into.
struct LensCopy {
    from: wgpu::Texture,
    to: wgpu::Texture,
    view: wgpu::TextureView,
    extent: wgpu::Extent3d,
}

/// The counter of a pool the native pass declines for `reason`
/// ([`IndexedPass::for_pool`]'s, and the recording's own).
pub fn pool_refusal_counter(reason: &str) -> String {
    format!("layer_plan_15_pool_refused_{}", reason.replace([' ', '-'], "_"))
}

static POOL_RECORDINGS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn count_pool_recording(counter: String) {
    *POOL_RECORDINGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().entry(counter).or_default() += 1;
}

/// The recordings of sorted pools by outcome: `layer_plan_15_pool_recorded`
/// per recording made, `layer_plan_15_pool_refused_<reason>` per recording
/// refused, and `layer_plan_15_pool_unrecorded` per admitted pass dropped
/// without a recording (the guest's D3D draw then stands, as
/// `layer_plan_15_pool_on_d3d` counts for the admission's refusals).
pub fn pool_recording_counters() -> Vec<(String, u64)> {
    POOL_RECORDINGS.lock().unwrap_or_else(|e| e.into_inner()).iter().flatten().map(|(k, &v)| (k.clone(), v)).collect()
}

/// The stage of `draw` that samples the frame behind a pool: the first
/// sampled 2D stage the device resolved as a frame sample
/// (`StageExtras::samples_frame`: the bound colour, a snapshot of it or a
/// frame-sized render level), whatever its texture mode. The texture mode
/// does not identify it: Delta Towers' `tower_pool` samples its signed
/// ripple bitmap projectively at t0 and reads the refraction target
/// (engine texture 20) at t2 as a bump environment map offset by that
/// ripple, so a copy of the scene bound at the first projective stage
/// replaces the ripple and every bump offset follows the scene's colour.
/// The texture's own size does not matter: its coordinates are normalised
/// by its header, the recording's copy is of the bound colour at the
/// recording's size (the display copy's, or the reference copy's), and the
/// guest's lens can be any resolve size.
pub fn pool_lens_stage(draw: &ResolvedDraw) -> Option<usize> {
    let sampled = draw.sampled_texture_stages();
    (0..4).find(|&n| {
        sampled[n]
            && draw.extras[n].samples_frame
            && draw.texture_info[n].is_some_and(|info| info.kind == TextureKind::D2)
    })
}

/// What each stage of `draw` is, for the one line the first lens (or the
/// first refused one) logs: its texture mode, whether the program samples
/// it, whether it samples the frame, and the bound image's kind and size,
/// against the guest target's size.
fn pool_lens_report(draw: &ResolvedDraw) -> String {
    use shaders::psh::PixelShaderDef;
    let modes = draw.shader.pixel_shader.as_ref().map_or(0, |w| PixelShaderDef::from_words(w).texture_modes);
    let sampled = draw.sampled_texture_stages();
    let stages: Vec<String> = (0..4).map(|n| format!(
        "t{n}: mode {:#x} sampled {} frame {} image {}", (modes >> (5 * n)) & 0x1f, sampled[n],
        draw.extras[n].samples_frame,
        draw.texture_info[n].map_or("none".to_owned(), |i| format!("{:?} {}x{}", i.kind, i.width, i.height)),
    )).collect();
    format!("[{}] against the guest target {}x{}", stages.join("; "), draw.target.width, draw.target.height)
}

/// `copies`' scratch for `target`'s copy, made again when the colour's
/// extent or format changes, and the copy to record into it.
fn pool_lens_copy(
    device: &wgpu::Device, copies: &mut [Option<wgpu::TextureView>; 2], target: &ExternalAttachments,
) -> Result<LensCopy, &'static str> {
    if !crate::water_gpu::WaterSet::can_copy(&target.color, target.size) {
        return Err("lens source cannot be copied");
    }
    if !gfx::program::filterable(target.color_format, device.features()) {
        return Err("lens source is unfilterable");
    }
    let texture = target.color.texture();
    let extent = texture.size();
    let format = texture.format();
    let scratch = &mut copies[match target.copy {
        d3d::TargetCopy::Reference => 0,
        d3d::TargetCopy::Display => 1,
    }];
    if scratch.as_ref().is_some_and(|v| v.texture().size() != extent || v.texture().format() != format) {
        *scratch = None;
    }
    let view = scratch.get_or_insert_with(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native pool lens copy"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }).create_view(&Default::default())
    });
    Ok(LensCopy { from: texture.clone(), to: view.texture().clone(), view: view.clone(), extent })
}

impl IndexedPass {
    /// A sorted pool's recording: the lens copy of the bound colour, then the
    /// pool's packets in a pass over the same attachments, loaded, so the
    /// pool tests the depth the world left and blends over the colour the
    /// copy was taken of.
    fn record_pool(
        &mut self, target: &ExternalAttachments, request: ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalEncoded, &'static str> {
        let (draws, lens) = self.prepare_with(target, request.surface_size, true)?;
        let lens = lens.ok_or("lens unavailable")?;
        if draws.packets.iter().any(RecordedDraw::counts_samples) {
            return Err("pool draw counts samples");
        }
        let (color, depth) = (target.color.clone(), target.depth.clone());
        let has_stencil = target.depth_format.has_stencil_aspect();
        let packets = draws.packets;
        let count = packets.len();
        let LensCopy { from, to, extent, .. } = lens;
        let size = target.size;
        let commands = move |encoder: &mut wgpu::CommandEncoder| {
            encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), extent);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native pool"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: has_stencil
                        .then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, size.0 as f32, size.1 as f32, 0.0, 1.0);
            // Checked when the packets were made: every pipeline built, no
            // counted draw.
            let _ = gfx::renderer::encode_in_pass(&mut pass, &packets);
        };
        Ok(d3d::backend::ExternalEncoded { ranges: draws.ranges, draws: count, passes: 1, commands: Box::new(commands) })
    }
}

impl Drop for IndexedPass {
    /// An admitted pool that was dropped without a recording was declined
    /// upstream of the pass (the stage's own counters and the `d3d:` line
    /// logged once name where): counted so it is never silent.
    fn drop(&mut self) {
        if self.sorted_pool && !self.recorded.load(std::sync::atomic::Ordering::Relaxed) {
            count_pool_recording("layer_plan_15_pool_unrecorded".to_owned());
        }
    }
}

/// An admitted sky strip as the cut replays it: its draw (over the load's
/// geometry), the strip cache and its `SectionKey::Sky` scope and key.
#[derive(Clone)]
pub struct SkyTemplate {
    draw: Arc<ResolvedDraw>,
    cache: Arc<Cache>,
    section: ((u64, u32), SectionKey),
}

impl SkyTemplate {
    /// The stencil the strip drew with, which its replay draws with: the
    /// cut's arm compares it between guest passes.
    pub fn stencil(&self) -> &gfx::state::StencilState {
        &self.draw.state.render.stencil
    }
}

/// The sky's strips as one external pass, issued in slot 8 with
/// `0x0003ede0` passed over: each a [`IndexedPass::sky_replay`], in the
/// guest's order, with the device's view block at the issue over each
/// strip's (`view_constants`).
pub struct SkyCut {
    strips: Vec<IndexedPass>,
}

impl SkyCut {
    /// `None` for no strips or one that is not a replay.
    pub fn new(strips: Vec<IndexedPass>) -> Option<Self> {
        (!strips.is_empty() && strips.iter().all(|s| s.replay)).then_some(Self { strips })
    }

    pub fn len(&self) -> usize {
        self.strips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strips.is_empty()
    }
}

impl ExternalPass for SkyCut {
    fn replays_scene(&self) -> bool {
        self.strips.iter().all(|s| s.replays_scene())
    }

    fn uses_validated_resources(&self) -> bool { true }

    fn encode(&mut self, _target: ExternalTarget<'_>) {}

    fn colour_only(&self) -> bool {
        self.strips.iter().all(|s| s.colour_only())
    }

    fn writes(&self) -> ExternalWrites {
        self.strips.iter().map(|s| s.writes()).fold(ExternalWrites::default(), |a, b| ExternalWrites {
            color: a.color || b.color,
            depth_stencil: a.depth_stencil || b.depth_stencil,
        })
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        for strip in &mut self.strips {
            let mut draw = (*strip.draw).clone();
            let mut all = *draw.shader.constants;
            all[..d3d::VIEW_CONSTANTS].copy_from_slice(constants);
            draw.shader.constants = Arc::new(all);
            strip.draw = Arc::new(draw);
        }
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let mut out: Option<ExternalDraws> = None;
        for strip in &mut self.strips {
            let draws = match strip.record_draws(target, request)? {
                Ok(draws) => draws,
                Err(why) => return Some(Err(why)),
            };
            match &mut out {
                None => out = Some(draws),
                Some(all) => {
                    all.ranges += draws.ranges;
                    all.packets.extend(draws.packets);
                    all.discards |= draws.discards;
                    all.writes.extend(draws.writes);
                }
            }
        }
        out.map(Ok)
    }
}

impl ExternalPass for IndexedPass {
    fn replays_scene(&self) -> bool {
        // A standalone range owns its entire draw. prepare() allocates its
        // vertices and uniforms from each recording's arena and binds every
        // sampled stage to the recording's own stage view, so a textured
        // range reads the textures the replay captured beside its packet.
        // A section's resident cache and a pinned texaccum surface are
        // outside this case. Sky material groups match the replay's actual
        // stage views and formats. The immutable per-view palette is owned
        // by this pass.
        self.sorted_pool || self.sorted_layer || self.sky.is_some() || (self.section.is_none() && self.pinned.is_none())
    }

    fn uses_validated_resources(&self) -> bool { true }

    fn encode(&mut self, _target: ExternalTarget<'_>) {}

    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn with_draw(&self, draw: Arc<ResolvedDraw>) -> Option<Box<dyn ExternalPass>> {
        // A display frame between ticks changes the constants only
        // (`d3d::scene_sink::Retarget`): the geometry and state the
        // candidate checks accepted stay.
        if draw.streams.len() != self.draw.streams.len() || self.sky.as_ref().is_some_and(|rows| rows.holds(&draw).is_err()) {
            return None;
        }
        Some(Box::new(Self {
            sorted_pool: self.sorted_pool,
            lens: self.lens,
            recorded: Default::default(),
            sky: self.sky.clone(),
            draw,
            cache: Arc::clone(&self.cache),
            frame: self.frame,
            extracted_frame: self.extracted_frame,
            fetch: self.fetch,
            pinned: self.pinned,
            sorted_layer: self.sorted_layer,
            section: self.section,
            replay: self.replay,
        }))
    }

    fn sampled(&self) -> Vec<u32> {
        self.pinned.iter().map(|target| target.header).collect()
    }

    fn colour_only(&self) -> bool {
        self.draw.target.depth_format.is_none()
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: self.draw.state.render.color_write.xbox_value() != 0,
            depth_stencil: self.draw.writes_depth_stencil(),
        }
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        // A pool needs its lens copy first: it records as commands.
        if self.sorted_pool {
            count_pool_recording("layer_plan_15_pool_record_draws_declined".to_owned());
            return None;
        }
        Some(self.prepare(target, request.surface_size))
    }

    fn record_encoded(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        if !self.sorted_pool { return None; }
        count_pool_recording("layer_plan_15_pool_record_encoded_called".to_owned());
        let result = self.record_pool(target, request);
        count_pool_recording(match &result {
            Ok(_) => {
                self.recorded.store(true, std::sync::atomic::Ordering::Relaxed);
                "layer_plan_15_pool_recorded".to_owned()
            }
            Err(why) => pool_refusal_counter(why),
        });
        if let Err(why) = &result {
            let mut refusals = self.cache.pool_refusals.lock().unwrap_or_else(|e| e.into_inner());
            let count = refusals.entry(*why).or_default();
            *count += 1;
            if count.is_power_of_two() {
                eprintln!("native-water: sorted pool recording refusals for this BSP: {refusals:?}");
            }
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::ScissorRect;

    #[test]
    fn indexed_layout_plans_reuse_exact_strides_and_bound_changing_draws() {
        let layout = VertexLayout { streams: vec![Default::default()], pretransformed: false };
        let mut cache = LayoutPlans::default();
        let mut stream = [d3d::StreamBytes { stride: 4, data: vec![0; 4].into() }];
        let reused = cache.get(&layout, &stream, FetchMode::Gpu);
        assert!(Arc::ptr_eq(&reused, &cache.get(&layout, &stream, FetchMode::Gpu)));
        stream[0].stride = 8;
        let different = cache.get(&layout, &stream, FetchMode::Gpu);
        assert!(!Arc::ptr_eq(&reused, &different));
        assert_eq!(different.streams[0].guest_stride, 8);
        for n in 3..=(LAYOUT_PLANS as u32 * 3) {
            stream[0].stride = n * 4;
            cache.get(&layout, &stream, FetchMode::Gpu);
            assert!(cache.count <= LAYOUT_PLANS);
        }
        assert_eq!(cache.layouts.values().map(Vec::len).sum::<usize>(), cache.count);
    }

    #[test]
    fn resident_windows_limit_both_geometry_bytes_and_snapshot_count() {
        assert_eq!(section_charge(SECTION_WINDOWS - 1, SECTION_BYTES - 8, 8), Some(8));
        assert_eq!(section_charge(0, SECTION_BYTES - 8, 9), None);
        assert_eq!(section_charge(SECTION_WINDOWS, 0, 8), None);
        assert_eq!(section_charge(0, 0, SECTION_BYTES + 1), None);
    }

    #[test]
    fn sky_palettes_and_section_compression_preserve_other_material_constants() {
        let mut v = VertexUniforms::default();
        v.constants.fill([7.0; 4]);
        let mut p = PixelUniforms::default();
        let material = Arc::new(crate::sky_material::SkyConstants {
            vertex: vec![tags::material::Register { index: 30, value: [4.0; 4], written: 0b0001, origin: tags::material::Origin::Instance }],
            pixel: vec![(0, 2, 0xff00_0000)],
        });
        let first = SkyRows { nodes: vec![[3.5; 4]; 6].into(), decompression: [[2.0; 4]; 3], material: Arc::clone(&material) };
        let second = SkyRows { nodes: vec![[-1.25; 4]; 6].into(), decompression: [[8.5; 4]; 3], material };
        first.apply(&mut v, &mut p);
        assert_eq!(first.check(&v.constants), Ok(()));
        assert_eq!(v.constants[30], [4.0, 7.0, 7.0, 7.0], "the plan's register, written components only");
        assert_eq!(p.c0[2], [0.0, 0.0, 0.0, 1.0], "the plan's combiner constant");
        assert_eq!(second.check(&v.constants), Err("transform"));
        second.apply(&mut v, &mut p);
        assert_eq!(second.check(&v.constants), Ok(()));
        let start = crate::program::MODEL_ROWS;
        assert_eq!(v.constants[start - 1], [7.0; 4]);
        assert_eq!(v.constants[start + 6], [7.0; 4]);
        v.constants[crate::program::TEXCOORD_DECOMPRESSION] = [0.0; 4];
        assert_eq!(second.check(&v.constants), Err("decompression"));
    }

    #[test]
    fn converted_streams_require_the_same_snapshot_count_and_repack_plan() {
        use gfx::vertex::{
            StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
        };
        let plan = |ty| {
            let layout = VertexLayout {
                streams: vec![StreamLayout {
                    stream: 0,
                    stride: 4,
                    attributes: vec![VertexAttribute {
                        register: VertexRegister(0),
                        data_type: ty,
                        offset: 0,
                    }],
                }],
                pretransformed: false,
            };
            RepackPlan::with_mode(&layout, &[4], FetchMode::Cpu)
                .streams
                .remove(0)
        };
        let short = plan(VertexDataType::Short2);
        let packed = plan(VertexDataType::NormPacked3);
        assert!(!short.identity && !packed.identity);
        let key = BufferKey::vertex(7, &short, 3);
        assert_eq!(key, BufferKey::vertex(7, &short.clone(), 3));
        assert_ne!(key, BufferKey::vertex(7, &packed, 3));
        assert_ne!(key, BufferKey::vertex(8, &short, 3));
        assert_ne!(key, BufferKey::vertex(7, &short, 4));
    }

    #[test]
    fn scaled_scissors_keep_the_same_guest_rectangle() {
        let clip = d3d::backend::PixelRect {
            x: 7,
            y: 5,
            width: 11,
            height: 9,
        };
        assert_eq!(
            scaled_scissor(&clip, 2.into(), (32, 32)),
            Some(ScissorRect {
                x: 14,
                y: 10,
                width: 18,
                height: 18
            })
        );
        assert_eq!(scaled_scissor(&clip, 2.into(), (14, 32)), None);
    }
}
