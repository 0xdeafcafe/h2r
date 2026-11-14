//! Pipeline keys and the pipeline cache.
//!
//! A [`PipelineKey`] is the *translated* state: wgpu's own hashable
//! descriptor types plus the vertex layouts and a shader id. Two different
//! guest render-state configurations that translate identically (say, two
//! blend factor spellings, or a stencil reference change) share a pipeline.
//! The cache is generic over the stored value so the keying logic is
//! unit-testable without a GPU.

use std::collections::HashMap;

use crate::state::RenderState;
use crate::translate::{self, Capabilities};
use crate::repack::RepackPlan;
use crate::vertex::{OwnedVertexBufferLayout, PrimitiveType, VertexLayout};

/// Identifies a shader program known to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// Everything that distinguishes one wgpu render pipeline from another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
        let vertex_buffers = RepackPlan::new(layout, strides).buffer_layouts();
        Self {
            shader,
            vertex_buffers,
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

/// A map from [`PipelineKey`] to a built pipeline, with hit/miss counters.
#[derive(Debug)]
pub struct PipelineCache<V> {
    map: HashMap<PipelineKey, V>,
    hits: u64,
    misses: u64,
}

impl<V> Default for PipelineCache<V> {
    fn default() -> Self {
        Self { map: HashMap::new(), hits: 0, misses: 0 }
    }
}

impl<V> PipelineCache<V> {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached value for `key`, building it with `make` on a miss.
    pub fn get_or_insert_with(&mut self, key: &PipelineKey, make: impl FnOnce(&PipelineKey) -> V) -> &V {
        if self.map.contains_key(key) {
            self.hits += 1;
        } else {
            self.misses += 1;
            let v = make(key);
            self.map.insert(key.clone(), v);
        }
        &self.map[key]
    }

    /// Looks up without building or counting.
    pub fn get(&self, key: &PipelineKey) -> Option<&V> {
        self.map.get(key)
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
        self.map.clear();
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
pub struct ShaderProgram {
    /// The compiled module.
    pub module: wgpu::ShaderModule,
    /// Bind group layouts the program expects.
    pub layout: wgpu::PipelineLayout,
    /// Vertex entry point.
    pub vs_entry: String,
    /// Fragment entry point.
    pub fs_entry: String,
}

/// Builds a wgpu render pipeline from a key and the program it names.
pub fn create_render_pipeline(
    device: &wgpu::Device,
    key: &PipelineKey,
    program: &ShaderProgram,
    label: &str,
) -> wgpu::RenderPipeline {
    let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> =
        key.vertex_buffers.iter().map(|b| Some(b.as_wgpu())).collect();
    let targets = [Some(key.color_target.clone())];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        assert_eq!((cache.len(), cache.hits(), cache.misses()), (2, 3, 2));
        assert_eq!(*cache.get_or_insert_with(&a, |_| unreachable!()), 1);
        cache.clear();
        assert!(cache.is_empty());
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
