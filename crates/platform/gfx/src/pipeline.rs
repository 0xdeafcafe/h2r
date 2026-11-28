//! Pipeline keys and the pipeline cache.
//!
//! A [`PipelineKey`] is the *translated* state: wgpu's own hashable
//! descriptor types plus the vertex layouts and a shader id. Two different
//! guest render-state configurations that translate identically (say, two
//! blend factor spellings, or a stencil reference change) share a pipeline.
//! The cache is generic over the stored value so the keying logic is
//! unit-testable without a GPU.

use hashbrown::{HashMap, hash_map::EntryRef};

use crate::state::RenderState;
use crate::translate::{self, Capabilities};
use crate::repack::RepackPlan;
use crate::vertex::{OwnedVertexBufferLayout, PrimitiveType, VertexLayout};

/// Identifies a shader program known to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ShaderId(pub u32);

/// Render target formats a pipeline is compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetFormats {
    /// Colour attachment format.
    pub color: wgpu::TextureFormat,
    /// Depth/stencil attachment format, if any.
    pub depth: Option<wgpu::TextureFormat>,
    /// MSAA sample count.
    pub samples: u32,
}

/// The meaning of a serialized [`PipelineKey`]. Bump it when a key that
/// still parses would build a different pipeline than it did (a field's
/// interpretation, a translation the key is made from): persisted keys of
/// another version are not loaded (`pipeline_store`).
pub const KEY_VERSION: u32 = 1;

/// Everything that distinguishes one wgpu render pipeline from another.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PipelineKey {
    /// Shader program.
    pub shader: ShaderId,
    /// Vertex buffer layouts as the *host* sees them (after repacking), one
    /// per stream.
    pub vertex_buffers: Vec<OwnedVertexBufferLayout>,
    /// Topology, culling and fill.
    pub primitive: wgpu::PrimitiveState,
    /// Depth/stencil (absent without a depth attachment).
    pub depth_stencil: Option<wgpu::DepthStencilState>,
    /// Colour target: format, blend, write mask.
    pub color_target: wgpu::ColorTargetState,
    /// MSAA sample count.
    pub samples: u32,
}

impl PipelineKey {
    /// Translates guest state into a key.
    ///
    /// `strides` overrides the declared per-stream vertex strides (guest
    /// `SetStreamSource` strides); pass `&[]` to use the declared ones.
    pub fn new(
        shader: ShaderId,
        layout: &VertexLayout,
        strides: &[u32],
        primitive: PrimitiveType,
        rs: &RenderState,
        targets: TargetFormats,
        caps: Capabilities,
    ) -> Self {
        Self::from_plan(shader, &RepackPlan::new(layout, strides), primitive, rs, targets, caps)
    }

    /// [`new`](Self::new) with the vertex buffers of `plan` (which may use
    /// [`FetchMode::Gpu`](crate::repack::FetchMode::Gpu)).
    pub fn from_plan(
        shader: ShaderId,
        plan: &RepackPlan,
        primitive: PrimitiveType,
        rs: &RenderState,
        targets: TargetFormats,
        caps: Capabilities,
    ) -> Self {
        Self {
            shader,
            vertex_buffers: plan.buffer_layouts(),
            primitive: translate::primitive_state(rs, primitive.host_topology(), caps),
            depth_stencil: translate::depth_stencil_state(rs, targets.depth, primitive.host_topology()),
            color_target: wgpu::ColorTargetState {
                format: targets.color,
                blend: translate::blend_state(rs),
                write_mask: translate::color_writes(rs.color_write),
            },
            samples: targets.samples,
        }
    }
}

static PIPELINES: mem_watch::Counter = mem_watch::Counter::new("gfx.pipeline_cache_entries");

/// Room for the busy draw pipelines and variants without holding every map's.
const DRAW_PIPELINES: usize = 1536;
/// Counting uses the same keys but has its own smaller working set.
pub(crate) const COUNT_PIPELINES: usize = 128;

#[derive(Debug)]
struct Cached<V> {
    value: V,
    /// A speculative insert stays cold until a draw asks for it.
    used: u64,
}

/// A bounded map from [`PipelineKey`] to a pipeline, with hit/miss counters.
#[derive(Debug)]
pub struct PipelineCache<V> {
    map: HashMap<PipelineKey, Cached<V>>,
    limit: usize,
    tick: u64,
    entries: mem_watch::Gauge,
    hits: u64,
    misses: u64,
}

impl<V> Default for PipelineCache<V> {
    fn default() -> Self {
        Self::with_limit(DRAW_PIPELINES)
    }
}

impl<V> PipelineCache<V> {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_limit(limit: usize) -> Self {
        assert!(limit > 0);
        Self {
            map: HashMap::new(),
            limit,
            tick: 0,
            entries: PIPELINES.track(0),
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        self.map.len() == self.limit
    }

    /// Evict a batch on a miss at the cap. A hit costs one borrowed lookup
    /// except when the cache is exactly full.
    fn make_room(&mut self, key: &PipelineKey) {
        if !self.is_full() || self.map.contains_key(key) {
            return;
        }
        let mut use_order: Vec<u64> = self.map.values().map(|entry| entry.used).collect();
        let evict = (self.limit / 4).max(1);
        let (_, oldest, _) = use_order.select_nth_unstable(evict - 1);
        self.map.retain(|_, entry| entry.used > *oldest);
        self.entries.set(self.map.len() as u64);
    }

    /// Returns the cached value for `key`, building it with `make` on a miss.
    pub fn get_or_insert_with(&mut self, key: &PipelineKey, make: impl FnOnce(&PipelineKey) -> V) -> &V {
        self.make_room(key);
        self.tick += 1;
        match self.map.entry_ref(key) {
            EntryRef::Occupied(entry) => {
                self.hits += 1;
                let entry = entry.into_mut();
                entry.used = self.tick;
                &entry.value
            }
            EntryRef::Vacant(entry) => {
                let _scope = mem_watch::scope("render.pipeline");
                self.misses += 1;
                let entry = entry.insert(Cached { value: make(key), used: self.tick });
                self.entries.set(self.entries.get() + 1);
                &entry.value
            }
        }
    }

    /// Adds `value` for `key` (replacing any) without counting it as a draw.
    pub fn insert(&mut self, key: PipelineKey, value: V) {
        let _scope = mem_watch::scope("render.pipeline");
        self.make_room(&key);
        self.map.insert(key, Cached { value, used: 0 });
        self.entries.set(self.map.len() as u64);
    }

    /// Every key and value.
    pub fn iter(&self) -> impl Iterator<Item = (&PipelineKey, &V)> {
        self.map.iter().map(|(key, entry)| (key, &entry.value))
    }

    /// Looks up without building or counting.
    pub fn get(&self, key: &PipelineKey) -> Option<&V> {
        self.map.get(key).map(|entry| &entry.value)
    }

    /// Number of distinct pipelines held.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Lookups that found an existing pipeline.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Lookups that had to build a pipeline.
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Drops every pipeline (e.g. after the surface format changes).
    pub fn clear(&mut self) {
        self.map = HashMap::new();
        self.entries.set(0);
    }
}

/// A compiled shader program that pipelines can be built from.
///
/// This is the plug-in point for shader generators (register combiners,
/// vertex microcode): translate guest state to WGSL elsewhere, wrap the module
/// and its pipeline layout here, [`Renderer::register_shader`] it under a
/// [`ShaderId`], and put that id in [`PipelineKey::shader`]. The pipeline
/// layout and the bind groups a draw supplies must agree; the renderer does
/// not interpret them.
///
/// [`Renderer::register_shader`]: crate::renderer::Renderer::register_shader
#[derive(Clone)]
pub struct ShaderProgram {
    /// The compiled module.
    pub module: wgpu::ShaderModule,
    /// Bind group layouts the program expects.
    pub layout: wgpu::PipelineLayout,
    /// Vertex entry point.
    pub vs_entry: String,
    /// Fragment entry point.
    pub fs_entry: String,
    /// The fragment entry the pixel-count pass uses instead of its own
    /// (see [`crate::occlusion`]), for a program whose fragment stage
    /// writes the depth: it must write the same depth and 1 in red. `None`
    /// counts with the rasterized depth.
    pub count_entry: Option<String>,
}

/// Builds a wgpu render pipeline from a key and the program it names.
pub fn create_render_pipeline(
    device: &wgpu::Device,
    key: &PipelineKey,
    program: &ShaderProgram,
    label: &str,
) -> wgpu::RenderPipeline {
    let _scope = mem_watch::scope("render.pipeline");
    let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> =
        key.vertex_buffers.iter().map(|b| Some(b.as_wgpu())).collect();
    let targets = [Some(key.color_target.clone())];
    crate::churn::create_render_pipeline(device, &wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&program.layout),
        vertex: wgpu::VertexState {
            module: &program.module,
            entry_point: Some(&program.vs_entry),
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: key.primitive,
        depth_stencil: key.depth_stencil.clone(),
        multisample: wgpu::MultisampleState { count: key.samples, mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: &program.module,
            entry_point: Some(&program.fs_entry),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::*;
    use crate::vertex::fvf;

    fn key(rs: &RenderState) -> PipelineKey {
        let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        PipelineKey::new(
            ShaderId(0),
            &layout,
            &[],
            PrimitiveType::TriangleList,
            rs,
            TargetFormats { color: wgpu::TextureFormat::Bgra8Unorm, depth: Some(wgpu::TextureFormat::Depth24PlusStencil8), samples: 1 },
            Capabilities::ALL,
        )
    }

    #[test]
    fn equal_state_equal_key() {
        assert_eq!(key(&RenderState::default()), key(&RenderState::default()));
    }

    #[test]
    fn irrelevant_state_does_not_change_key() {
        let a = RenderState::default();
        let mut b = a;
        // Draw-time / shader-side values: not part of the pipeline.
        b.stencil.reference = 7;
        b.blend_color = 0x1234_5678;
        b.texture_factor = 0;
        b.alpha_ref = 99;
        b.fog_color = 5;
        assert_eq!(key(&a), key(&b));
        // Disabled-blend factors are invisible too.
        let mut c = a;
        c.src_blend = BlendFactor::DestColor;
        assert_eq!(key(&a), key(&c));
    }

    #[test]
    fn relevant_state_changes_key() {
        let a = RenderState::default();
        let mut b = a;
        b.blend_enable = true;
        assert_ne!(key(&a), key(&b));
        let mut c = a;
        c.cull_mode = CullMode::None;
        assert_ne!(key(&a), key(&c));
        let mut d = a;
        d.depth_func = CompareFunc::Always;
        assert_ne!(key(&a), key(&d));
        let mut e = a;
        e.color_write.alpha = false;
        assert_ne!(key(&a), key(&e));
    }

    #[test]
    fn depth_clamp_is_part_of_key_where_the_device_can() {
        let a = RenderState::default();
        let b = RenderState {
            depth_clamp: true,
            ..a
        };
        assert_ne!(key(&a), key(&b));
        assert!(key(&b).primitive.unclipped_depth);
        assert!(!key(&a).primitive.unclipped_depth);
        // Without DEPTH_CLIP_CONTROL both draws clip, and share a pipeline.
        let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        let t = TargetFormats {
            color: wgpu::TextureFormat::Bgra8Unorm,
            depth: None,
            samples: 1,
        };
        let caps = Capabilities {
            depth_clip_control: false,
            ..Capabilities::ALL
        };
        let mk = |rs: &RenderState| {
            PipelineKey::new(
                ShaderId(0),
                &layout,
                &[],
                PrimitiveType::TriangleList,
                rs,
                t,
                caps,
            )
        };
        assert_eq!(mk(&a), mk(&b));
        assert!(!mk(&b).primitive.unclipped_depth);
    }

    #[test]
    fn cache_hits_and_misses() {
        let mut cache: PipelineCache<u32> = PipelineCache::new();
        let a = key(&RenderState::default());
        let other = RenderState { blend_enable: true, ..Default::default() };
        let b = key(&other);
        let mut built = 0;
        for k in [&a, &a, &b, &a, &b] {
            cache.get_or_insert_with(k, |_| {
                built += 1;
                built
            });
        }
        assert_eq!(built, 2);
        assert_eq!(cache.entries.get(), 2);
        assert_eq!((cache.len(), cache.hits(), cache.misses()), (2, 3, 2));
        assert_eq!(*cache.get_or_insert_with(&a, |_| unreachable!()), 1);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.entries.get(), 0);
    }

    #[test]
    fn a_full_cache_evicts_speculation_then_least_recently_drawn() {
        use std::sync::Arc;

        let mut cache = PipelineCache::with_limit(4);
        let keys = [0, 1, 2, 3, 4, 5].map(|id| {
            let mut k = key(&RenderState::default());
            k.shader = ShaderId(id);
            k
        });
        let [a, b, c, speculative, d, e] = &keys;

        let prewarmed = Arc::new(0);
        let weak = Arc::downgrade(&prewarmed);
        cache.insert(speculative.clone(), prewarmed);
        for k in [a, b, c] {
            cache.get_or_insert_with(k, |k| Arc::new(k.shader.0));
        }
        assert!(cache.is_full());
        assert_eq!(**cache.get_or_insert_with(a, |_| unreachable!()), 0);

        cache.get_or_insert_with(d, |k| Arc::new(k.shader.0));
        assert!(cache.get(speculative).is_none());
        assert!(weak.upgrade().is_none(), "only the cache held this pipeline");
        let recorded = cache.get(b).unwrap().clone();
        cache.get_or_insert_with(e, |k| Arc::new(k.shader.0));
        assert!(cache.get(b).is_none(), "oldest draw was evicted");
        assert_eq!(*recorded, 1, "a recorded draw still owns its pipeline");
        assert!(cache.get(a).is_some(), "a cache hit refreshes the draw's age");
        assert_eq!(cache.len(), 4);
        assert_eq!(cache.entries.get(), 4);
    }

    #[test]
    fn vertex_layout_is_part_of_key() {
        let rs = RenderState::default();
        let l1 = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        let l2 = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE | 0x100).unwrap();
        let t = TargetFormats { color: wgpu::TextureFormat::Bgra8Unorm, depth: None, samples: 1 };
        let mk = |l: &VertexLayout| {
            PipelineKey::new(ShaderId(0), l, &[], PrimitiveType::TriangleList, &rs, t, Capabilities::ALL)
        };
        assert_ne!(mk(&l1), mk(&l2));
        // Topology comes from the guest primitive type.
        let strip = PipelineKey::new(ShaderId(0), &l1, &[], PrimitiveType::TriangleStrip, &rs, t, Capabilities::ALL);
        assert_ne!(strip, mk(&l1));
        // Quad lists are rewritten to triangle lists and share a pipeline.
        let quad = PipelineKey::new(ShaderId(0), &l1, &[], PrimitiveType::QuadList, &rs, t, Capabilities::ALL);
        assert_eq!(quad, mk(&l1));
    }
}
