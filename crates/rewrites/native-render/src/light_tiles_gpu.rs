//! Light tiles on the GPU (docs/graphics/native-light-tiles.md): which
//! lights the tiled path takes ([`admit_light`]), the binning pass
//! ([`LightTiles::bin`]) and the run's layers 12 and 13 as instanced
//! draws of the lights' own translated passes ([`LightTiles::record`],
//! [`tiled_wgsl`]).
//!
//! A tiled draw is one of the run's geometry ranges with an instance per
//! light that draws it. Instance `i` runs the vertex program with light
//! `i`'s uniforms and hands `i` to the pixel program as a flat varying;
//! the pixel program reads light `i`'s uniforms and, where its tile does
//! not list light `i`, writes zero, which the admitted additive blends add
//! as nothing.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram};
use gfx::vertex::OwnedVertexBufferLayout;
use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;
use wgpu::util::DeviceExt;

use crate::light_gpu::{LightDraw, LightSet, same_gpu_pass};
use crate::light_look::{LightProgram, LightStage};
use crate::light_tiles::{MAX_RUN, TILE, TileRefusal, TileVolume, tiles};
use crate::program::{Cull, INPUTS, ViewInputs, pipeline_state};
use crate::world::WorldVertex;

/// One of a light's layer 12 or 13 passes, as admission reads it.
pub struct LitPass<'a> {
    pub program: &'a LightProgram,
    /// Its gel is the shader-driven target 13 (`Op::Gel`), drawn per light.
    pub gel_target: bool,
}

/// Whether `c` adds its source term to the destination: an add, the
/// destination's factor one, a source factor that reads no destination.
fn adds(c: &wgpu::BlendComponent) -> bool {
    use wgpu::BlendFactor as F;
    c.operation == wgpu::BlendOperation::Add
        && c.dst_factor == F::One
        && matches!(
            c.src_factor,
            F::Zero
                | F::One
                | F::Src
                | F::OneMinusSrc
                | F::SrcAlpha
                | F::OneMinusSrcAlpha
                | F::Constant
                | F::OneMinusConstant
        )
}

/// How a lit pass's colour blend reads the destination's alpha: the
/// alpha a shadowed light's layer 11 marked (native-shadows.md). Tiled,
/// the pass reads its light's mask layer instead ([`LightTiles::record_masked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaRead {
    None,
    /// Source factor `DstAlpha`.
    Dst,
    /// Source factor `OneMinusDstAlpha`.
    OneMinusDst,
}

/// [`AlphaRead`] of `p`: an add with destination factor one whose colour
/// source factor reads the destination's alpha.
pub fn alpha_read(p: &LightProgram) -> AlphaRead {
    use wgpu::BlendFactor as F;
    let Some(b) = p.state.blend else {
        return AlphaRead::None;
    };
    if !p.state.color_writes.intersects(wgpu::ColorWrites::COLOR)
        || b.color.operation != wgpu::BlendOperation::Add
        || b.color.dst_factor != F::One
    {
        return AlphaRead::None;
    }
    match b.color.src_factor {
        F::DstAlpha => AlphaRead::Dst,
        F::OneMinusDstAlpha => AlphaRead::OneMinusDst,
        _ => AlphaRead::None,
    }
}

/// Whether one pass of a light can draw tiled. A pass whose colour blend
/// reads the destination's alpha ([`alpha_read`]) is taken when it does
/// not write alpha: the run's masks hold the alpha it reads.
pub fn admit_pass(pass: &LitPass<'_>) -> Result<(), TileRefusal> {
    let p = pass.program;
    if p.stages
        .iter()
        .zip(p.dims)
        .any(|(s, d)| d.is_some() && matches!(s, LightStage::DepthShadow | LightStage::ColourMask))
    {
        return Err(TileRefusal::Shadow);
    }
    if pass.gel_target {
        return Err(TileRefusal::GelTarget);
    }
    let s = &p.state;
    let writes = s.color_writes;
    let blend_ok = match s.blend {
        _ if writes.is_empty() => true,
        None => false,
        Some(b) => {
            let reads = alpha_read(p) != AlphaRead::None;
            (!writes.intersects(wgpu::ColorWrites::COLOR) || adds(&b.color) || reads)
                && (!writes.contains(wgpu::ColorWrites::ALPHA) || (adds(&b.alpha) && !reads))
        }
    };
    if !blend_ok {
        return Err(TileRefusal::Blend);
    }
    use wgpu::CompareFunction as C;
    if s.depth_write || !matches!(s.depth_compare, C::Less | C::LessEqual | C::Equal) {
        return Err(TileRefusal::Depth);
    }
    tiled_wgsl(&p.wgsl).map(|_| ())
}

/// Whether a light can draw tiled: `steps` the layers its passes draw,
/// `passes` its layer 12 and 13 passes, `volume` its geometry's. A light
/// with layer 10 or 11 in its steps is [`TileRefusal::Shadow`], with 14
/// [`TileRefusal::Opaque`].
pub fn admit_light(
    steps: &[u8],
    passes: &[LitPass<'_>],
    volume: Option<TileVolume>,
) -> Result<TileVolume, TileRefusal> {
    if steps.iter().any(|&l| l == 10 || l == 11) {
        return Err(TileRefusal::Shadow);
    }
    if steps.contains(&14) {
        return Err(TileRefusal::Opaque);
    }
    for pass in passes {
        admit_pass(pass)?;
    }
    volume.ok_or(TileRefusal::Bounds)
}

/// Lights in a run whose layers 10 and 11 draw ahead of the run, each a
/// mask layer of its own after the run's first.
pub const MAX_SHADOWED: usize = 3;

/// Whether a shadowed light can draw tiled: its `steps` are layers 10 and
/// 11 and then only 12 and 13, and its layer 11 receivers write alpha
/// only (`receivers`, their colour writes). Layer 14, or a receiver that
/// multiplies the destination's colour (native-shadows.md, implementations
/// 3 and 4), keeps the light per light.
pub fn admit_shadowed_light(
    steps: &[u8],
    receivers: &[wgpu::ColorWrites],
) -> Result<(), TileRefusal> {
    if steps.contains(&14) {
        return Err(TileRefusal::Opaque);
    }
    let Some(rest) = steps.strip_prefix(&[10, 11]) else {
        return Err(TileRefusal::Shadow);
    };
    if rest.iter().any(|l| !matches!(l, 12 | 13))
        || receivers
            .iter()
            .any(|w| !(wgpu::ColorWrites::ALPHA).contains(*w))
    {
        return Err(TileRefusal::Shadow);
    }
    Ok(())
}

const XVS: &str = "@group(0) @binding(0) var<uniform> xvs: XvsUniforms;";
const XPS: &str = "@group(0) @binding(1) var<uniform> xps: XpsUniforms;";
const FOG: &str = "    @location(8) fog: f32,\n}";
const VS: &str = "fn vs_main(";
const FS: &str = "fn fs_main(v: XboxVaryings";

const TILE_WGSL: &str = r#"
struct XlTiles { tiles_x: u32, tile: u32, pad0: u32, pad1: u32 }
@group(2) @binding(0) var<uniform> xl_tiles: XlTiles;
@group(2) @binding(1) var<storage, read> xl_masks: array<u32>;
@group(2) @binding(2) var<storage, read> xl_bits: array<u32>;
@group(2) @binding(3) var<storage, read> xl_alpha_layers: array<u32>;
@group(2) @binding(4) var xl_alpha: texture_2d_array<f32>;

// The light's colour where its tile lists it, else zero: an additive
// blend adds a zero source as nothing.
fn xl_tile_colour(v: XboxVaryings, c: vec4f) -> vec4f {
    let t = vec2u(v.position.xy) / xl_tiles.tile;
    let lit = (xl_masks[t.y * xl_tiles.tiles_x + t.x] & (1u << xl_bits[v.xl_light])) != 0u;
    return select(vec4f(0.0), xl_alpha_factor(v, c), lit);
}
"#;

/// The blend's source factor applied in the program ([`AlphaRead`]): the
/// destination alpha the light's passes read is its mask layer's, and
/// the pipeline's factor becomes one. A unorm target clamps the source
/// before the blend multiplies it (`clamp`).
fn alpha_factor_wgsl(read: AlphaRead, clamp: bool) -> String {
    let rgb = if clamp { "saturate(c.rgb)" } else { "c.rgb" };
    let factor = match read {
        AlphaRead::None => {
            return "fn xl_alpha_factor(v: XboxVaryings, c: vec4f) -> vec4f { return c; }\n".into();
        }
        AlphaRead::Dst => "a",
        AlphaRead::OneMinusDst => "(1.0 - a)",
    };
    format!(
        "fn xl_alpha_factor(v: XboxVaryings, c: vec4f) -> vec4f {{\n    \
         let a = textureLoad(xl_alpha, vec2u(v.position.xy), xl_alpha_layers[v.xl_light], 0).a;\n    \
         return vec4f({rgb} * {factor}, c.a);\n}}\n"
    )
}

/// A translated light pass ([`LightProgram::wgsl`]) made instanced: its
/// vertex and pixel uniforms arrays indexed by the instance, the instance
/// a flat varying, the colour zero where the tile does not list the
/// instance's light. [`TileRefusal::Program`] when the module lacks the
/// shape `shaders::link` gives.
pub fn tiled_wgsl(wgsl: &str) -> Result<String, TileRefusal> {
    tiled_wgsl_reading(wgsl, AlphaRead::None, false)
}

/// [`tiled_wgsl`] for a pass whose blend reads the destination's alpha
/// as `read`, on a target that clamps (`clamp`, unorm) or not.
pub fn tiled_wgsl_reading(wgsl: &str, read: AlphaRead, clamp: bool) -> Result<String, TileRefusal> {
    let once = |s: &str, p: &str| s.matches(p).count() == 1;
    if !(once(wgsl, XVS) && once(wgsl, XPS) && once(wgsl, FOG) && once(wgsl, VS) && once(wgsl, FS))
    {
        return Err(TileRefusal::Program);
    }
    let s = wgsl
        .replace(
            XVS,
            "@group(0) @binding(0) var<storage, read> xvs_lights: array<XvsUniforms>;",
        )
        .replace(
            XPS,
            "@group(0) @binding(1) var<storage, read> xps_lights: array<XpsUniforms>;",
        )
        .replace("xvs.", "xvs_lights[xvs_light].")
        .replace("xps.", "xps_lights[xps_light].")
        .replace(
            FOG,
            "    @location(8) fog: f32,\n    @location(9) @interpolate(flat) xl_light: u32,\n}",
        )
        .replace(VS, "fn vs_main(@builtin(instance_index) xl_instance: u32, ");
    let vs = s.find(VS).ok_or(TileRefusal::Program)?;
    let open = vs + s[vs..].find("{\n").ok_or(TileRefusal::Program)? + 2;
    let ret = open
        + s[open..]
            .find("    return out;\n")
            .ok_or(TileRefusal::Program)?;
    let mut s = format!(
        "{}    xvs_light = xl_instance;\n{}    out.xl_light = xl_instance;\n{}",
        &s[..open],
        &s[open..ret],
        &s[ret..]
    );
    let fs = s.find(FS).ok_or(TileRefusal::Program)?;
    let open = fs + s[fs..].find("{\n").ok_or(TileRefusal::Program)? + 2;
    s.insert_str(open, "    xps_light = v.xl_light;\n");
    let (head, body) = s.split_at(open);
    let body = if body.matches("    return XpsOut(r.color, depth);\n").count() == 1 {
        body.replace(
            "    return XpsOut(r.color, depth);\n",
            "    return XpsOut(xl_tile_colour(v, r.color), depth);\n",
        )
    } else if body.matches("    return r.color;\n").count() == 1 {
        body.replace(
            "    return r.color;\n",
            "    return xl_tile_colour(v, r.color);\n",
        )
    } else {
        return Err(TileRefusal::Program);
    };
    let factor = alpha_factor_wgsl(read, clamp);
    Ok(format!(
        "var<private> xvs_light: u32;\nvar<private> xps_light: u32;\n{head}{body}{TILE_WGSL}{factor}"
    ))
}

const BIN_WGSL: &str = r#"
struct Info { size: vec2u, tiles_x: u32, count: u32 }
struct Volume { rect: vec4f, depth: vec4f }
@group(0) @binding(0) var scene_depth: texture_depth_2d;
@group(0) @binding(1) var<uniform> info: Info;
@group(0) @binding(2) var<storage, read> volumes: array<Volume>;
@group(0) @binding(3) var<storage, read_write> masks: array<u32>;

var<workgroup> far_bits: atomic<u32>;
var<workgroup> mask: atomic<u32>;

@compute @workgroup_size(16, 16)
fn bin(
    @builtin(workgroup_id) tile: vec3u,
    @builtin(local_invocation_id) local: vec3u,
    @builtin(local_invocation_index) li: u32,
) {
    if (li == 0u) {
        atomicStore(&far_bits, 0u);
        atomicStore(&mask, 0u);
    }
    workgroupBarrier();
    let p = tile.xy * 16u + local.xy;
    if (all(p < info.size)) {
        // Non-negative floats order as their bits.
        let d = max(textureLoad(scene_depth, p, 0), 0.0);
        atomicMax(&far_bits, bitcast<u32>(d));
    }
    workgroupBarrier();
    let far = bitcast<f32>(atomicLoad(&far_bits));
    if (li < info.count) {
        let v = volumes[li];
        let lo = vec2f(tile.xy * 16u);
        let hi = vec2f(min((tile.xy + 1u) * 16u, info.size));
        if (v.rect.x < hi.x && v.rect.z > lo.x && v.rect.y < hi.y && v.rect.w > lo.y
            && v.depth.x <= far) {
            atomicOr(&mask, 1u << li);
        }
    }
    workgroupBarrier();
    if (li == 0u) {
        masks[tile.y * info.tiles_x + tile.x] = atomicLoad(&mask);
    }
}
"#;

/// A run's tile masks: per tile, row by row, a bit per run light.
pub struct TileGrid {
    pub size: (u32, u32),
    pub tiles: (u32, u32),
    pub masks: wgpu::Buffer,
    bin: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
}

impl TileGrid {
    /// Records the binning dispatch that fills [`Self::masks`]: before the
    /// tiled draws, after the scene depth it reads is final.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("light tiles bin"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.bin);
        pass.set_bind_group(0, &self.group, &[]);
        pass.dispatch_workgroups(self.tiles.0, self.tiles.1, 1);
    }
}

/// One of the run's geometry ranges in layer 12 or 13 with the lights
/// that draw it: each light's bit in the [`TileGrid`] and its draw. The
/// draws share their pass, geometry, ranges and textures; a light that
/// draws the range twice is listed twice.
pub struct TiledDraw<'a> {
    pub layer: u8,
    /// The light set whose gels and engine textures the draw binds, when
    /// not [`LightTiles::record`]'s own (a light retains only its gel).
    pub set: Option<&'a LightSet>,
    pub lights: Vec<(u32, LightDraw<'a>)>,
}

struct Item {
    pipeline: Arc<wgpu::RenderPipeline>,
    textures: wgpu::BindGroup,
    geometry: Option<(wgpu::Buffer, wgpu::Buffer)>,
    ranges: Vec<Range<u32>>,
    instances: Range<u32>,
    blend_constant: wgpu::Color,
}

/// A run's tiled layers, recorded ([`LightTiles::record`]).
pub struct TiledPass {
    uniforms: wgpu::BindGroup,
    tiles: wgpu::BindGroup,
    items: Vec<Item>,
}

impl TiledPass {
    /// The run's draws into `pass`, open on the view's target with the
    /// scene's depth: layer 12's, then 13's, each draw once with an
    /// instance per light, in `scissor` (this copy's texels).
    pub fn encode(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        scissor: [u32; 4],
    ) {
        let [x, y, w, h] = scissor;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_stencil_reference(0);
        pass.set_bind_group(0, &self.uniforms, &[]);
        pass.set_bind_group(2, &self.tiles, &[]);
        for item in &self.items {
            let (v, i) = match &item.geometry {
                Some((v, i)) => (v, i),
                None => (vertices, indices),
            };
            pass.set_pipeline(&item.pipeline);
            pass.set_blend_constant(item.blend_constant);
            pass.set_bind_group(1, &item.textures, &[]);
            pass.set_vertex_buffer(0, v.slice(..));
            pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
            for r in &item.ranges {
                pass.draw_indexed(r.clone(), 0, item.instances.clone());
            }
        }
    }

    /// Instanced draws recorded.
    pub fn draws(&self) -> usize {
        self.items.iter().map(|i| i.ranges.len()).sum()
    }
}

type TiledFor = (
    u64,
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    Option<Cull>,
    Option<u64>,
);

/// The binning pass and the tiled programs' pipelines.
pub struct LightTiles {
    bin: wgpu::ComputePipeline,
    bin_layout: wgpu::BindGroupLayout,
    uniforms_layout: wgpu::BindGroupLayout,
    tiles_layout: wgpu::BindGroupLayout,
    /// Bound as the masks of a run that reads none.
    no_masks: wgpu::TextureView,
    pipelines: HashMap<TiledFor, Arc<wgpu::RenderPipeline>>,
}

fn storage(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// The view a run's masks bind as: every layer, as an array.
pub fn mask_view() -> wgpu::TextureViewDescriptor<'static> {
    wgpu::TextureViewDescriptor {
        label: Some("light tiles masks"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    }
}

impl LightTiles {
    pub fn new(device: &wgpu::Device) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let bin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light tiles bin"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform(1, compute),
                storage(2, compute, true),
                storage(3, compute, false),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("light tiles bin"),
            source: wgpu::ShaderSource::Wgsl(BIN_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("light tiles bin"),
            bind_group_layouts: &[Some(&bin_layout)],
            immediate_size: 0,
        });
        let bin = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("light tiles bin"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("bin"),
            compilation_options: Default::default(),
            cache: None,
        });
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light tiles uniforms"),
            entries: &[storage(0, both, true), storage(1, both, true)],
        });
        let frag = wgpu::ShaderStages::FRAGMENT;
        let tiles_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light tiles masks"),
            entries: &[
                uniform(0, frag),
                storage(1, frag, true),
                storage(2, frag, true),
                storage(3, frag, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let no_masks = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("light tiles no masks"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&mask_view());
        Self {
            bin,
            bin_layout,
            uniforms_layout,
            tiles_layout,
            no_masks,
            pipelines: HashMap::new(),
        }
    }

    /// Records the binning pass of a run whose lights' volumes are
    /// `volumes` (at most [`MAX_RUN`], bit `i` for volume `i`) over the
    /// scene's `depth` (`size` texels) into `encoder`.
    pub fn bin(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        volumes: &[TileVolume],
    ) -> TileGrid {
        let grid = self.prepare_bin(device, depth, size, volumes);
        grid.encode(encoder);
        grid
    }

    /// [`Self::bin`]'s buffers and bindings, made now; the dispatch is
    /// [`TileGrid::encode`], for commands recorded later.
    pub fn prepare_bin(
        &self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        volumes: &[TileVolume],
    ) -> TileGrid {
        let count = volumes.len().min(MAX_RUN) as u32;
        let (tx, ty) = tiles(size);
        let info: Vec<u8> = [size.0, size.1, tx, count]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let mut packed: Vec<f32> = volumes
            .iter()
            .take(MAX_RUN)
            .flat_map(|v| {
                [
                    v.rect[0], v.rect[1], v.rect[2], v.rect[3], v.near, 0.0, 0.0, 0.0,
                ]
            })
            .collect();
        if packed.is_empty() {
            packed.resize(8, 0.0);
        }
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let info = init("light tiles info", &info, wgpu::BufferUsages::UNIFORM);
        let volumes = init(
            "light tiles volumes",
            bytemuck::cast_slice(&packed),
            wgpu::BufferUsages::STORAGE,
        );
        let masks = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light tiles masks"),
            size: u64::from(tx * ty).max(1) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light tiles bin"),
            layout: &self.bin_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: info.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: volumes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: masks.as_entire_binding(),
                },
            ],
        });
        TileGrid {
            size,
            tiles: (tx, ty),
            masks,
            bin: self.bin.clone(),
            group,
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        draw: &LightDraw<'_>,
        textures_layout: &wgpu::BindGroupLayout,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> Result<Arc<wgpu::RenderPipeline>, &'static str> {
        let p = draw.program;
        let model = draw.model.as_ref().map(|m| m.layout);
        let key = (
            crate::light_gpu::light_gpu_key(p),
            color,
            depth,
            draw.cull,
            model.map(|m| m.id),
        );
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Ok(Arc::clone(pipeline));
        }
        let read = alpha_read(p);
        let clamp = matches!(
            color,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgb10a2Unorm
        );
        let wgsl =
            tiled_wgsl_reading(&p.wgsl, read, clamp).map_err(|_| "light tiles program shape")?;
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native light tiles pass"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            })
        })
        .ok_or("light tiles program invalid")?;
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native light tiles pass"),
            bind_group_layouts: &[
                Some(&self.uniforms_layout),
                Some(textures_layout),
                Some(&self.tiles_layout),
            ],
            immediate_size: 0,
        });
        let s = p.state;
        let (stride, attributes) = match model {
            Some(m) => (m.stride, m.attributes.clone()),
            None => (
                std::mem::size_of::<WorldVertex>() as u64,
                INPUTS
                    .map(|(location, offset, format)| wgpu::VertexAttribute {
                        format,
                        offset,
                        shader_location: location,
                    })
                    .to_vec(),
            ),
        };
        // The per-light pass's pipeline state (`LightGpu::pipeline`).
        let key_state = PipelineKey {
            shader: ShaderId(u32::MAX - 6),
            vertex_buffers: vec![OwnedVertexBufferLayout {
                stride,
                attributes,
                step_mode: wgpu::VertexStepMode::Vertex,
            }],
            primitive: pipeline_state(Some(&s), draw.cull).0,
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth,
                depth_write_enabled: Some(s.depth_write),
                depth_compare: Some(s.depth_compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            color_target: wgpu::ColorTargetState {
                format: color,
                // A read of the destination's alpha is the program's
                // (`xl_alpha_factor`): the blend adds the source as is.
                blend: s.blend.map(|mut b| {
                    if read != AlphaRead::None {
                        b.color.src_factor = wgpu::BlendFactor::One;
                    }
                    b
                }),
                write_mask: s.color_writes,
            },
            samples: 1,
        };
        let program = ShaderProgram {
            module,
            layout,
            vs_entry: "vs_main".into(),
            fs_entry: "fs_main".into(),
            count_entry: None,
        };
        let pipeline = gfx::program::validated_resource(device, || {
            gfx::pipeline::create_render_pipeline(
                device,
                &key_state,
                &program,
                "native light tiles pass",
            )
        })
        .ok_or("light tiles pipeline invalid")?;
        let pipeline = Arc::new(pipeline);
        self.pipelines.insert(key, Arc::clone(&pipeline));
        Ok(pipeline)
    }

    /// The run's tiled layers over `grid`: each of `draws` one instanced
    /// draw per range, layer 12's before 13's, each light's uniforms as its
    /// per-light pass computes them ([`LightDraw::uniforms`]). Refuses the
    /// whole run when a draw's lights differ in what they share, or a
    /// resource is missing; the run's lights then draw per light.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        set: &LightSet,
        grid: &TileGrid,
        view: &ViewInputs,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        draws: &[TiledDraw<'_>],
    ) -> Result<TiledPass, &'static str> {
        self.record_masked(device, set, grid, view, color, depth, draws, None)
    }

    /// [`Self::record`] with the run's masks: `masks` the array
    /// ([`mask_view`]) and, by light bit, the layer holding the
    /// destination alpha that light's passes read. A pass that reads the
    /// destination's alpha ([`alpha_read`]) with no masks refuses the run.
    #[allow(clippy::too_many_arguments)]
    pub fn record_masked(
        &mut self,
        device: &wgpu::Device,
        set: &LightSet,
        grid: &TileGrid,
        view: &ViewInputs,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        draws: &[TiledDraw<'_>],
        masks: Option<(&wgpu::TextureView, &[u32])>,
    ) -> Result<TiledPass, &'static str> {
        let mut order: Vec<&TiledDraw<'_>> =
            draws.iter().filter(|d| !d.lights.is_empty()).collect();
        order.sort_by_key(|d| d.layer);
        let mut vertex_bytes = Vec::new();
        let mut pixel_bytes = Vec::new();
        let mut bits = Vec::new();
        let mut layers: Vec<u32> = Vec::new();
        let mut items = Vec::new();
        for d in order {
            let (_, first) = &d.lights[0];
            for (bit, l) in &d.lights {
                let same = std::ptr::eq(l.gpu, first.gpu)
                    && same_gpu_pass(l.program, first.program)
                    && l.ranges == first.ranges
                    && l.cull == first.cull
                    && l.gel == first.gel
                    && l.gel_view == first.gel_view
                    && l.shadow_map.is_none()
                    && l.model.as_ref().map(|m| (&m.vertices, &m.indices))
                        == first.model.as_ref().map(|m| (&m.vertices, &m.indices));
                if !same || *bit as usize >= MAX_RUN {
                    return Err("tiled draw lights differ");
                }
            }
            if masks.is_none() && alpha_read(first.program) != AlphaRead::None {
                return Err("a tiled pass reads the destination's alpha with no masks");
            }
            let mut pass = first.gpu.try_lock().map_err(|_| "light pass busy")?;
            let (textures, _) = d
                .set
                .unwrap_or(set)
                .texture_group(device, first, &mut pass)
                .ok_or("incomplete GPU resources")?;
            let pipeline = self.pipeline(device, first, pass.textures_layout(), color, depth)?;
            drop(pass);
            let base = bits.len() as u32;
            for (bit, l) in &d.lights {
                let (v, p) = l.uniforms(view);
                let at = vertex_bytes.len();
                vertex_bytes.resize(at + VertexUniforms::SIZE, 0);
                v.write_bytes(&mut vertex_bytes[at..]);
                let at = pixel_bytes.len();
                pixel_bytes.resize(at + PixelUniforms::SIZE, 0);
                p.write_bytes(&mut pixel_bytes[at..]);
                bits.push(*bit);
                layers.push(
                    masks
                        .and_then(|(_, l)| l.get(*bit as usize).copied())
                        .unwrap_or(0),
                );
            }
            items.push(Item {
                pipeline,
                textures,
                geometry: first
                    .model
                    .as_ref()
                    .map(|m| (m.vertices.clone(), m.indices.clone())),
                ranges: first
                    .ranges
                    .iter()
                    .filter(|r| !r.is_empty())
                    .cloned()
                    .collect(),
                instances: base..bits.len() as u32,
                blend_constant: first.program.state.blend_constant,
            });
        }
        if items.is_empty() {
            return Err("no tiled draws");
        }
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let vertex = init("light tiles vertex uniforms", &vertex_bytes, storage);
        let pixel = init("light tiles pixel uniforms", &pixel_bytes, storage);
        let bits: Vec<u8> = bits.iter().flat_map(|b| b.to_le_bytes()).collect();
        let bits = init("light tiles bits", &bits, storage);
        let layers: Vec<u8> = layers.iter().flat_map(|l| l.to_le_bytes()).collect();
        let layers = init("light tiles mask layers", &layers, storage);
        let alpha = masks.map_or(&self.no_masks, |(view, _)| view);
        let info: Vec<u8> = [grid.tiles.0, TILE, 0, 0]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let info = init("light tiles grid", &info, wgpu::BufferUsages::UNIFORM);
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light tiles uniforms"),
            layout: &self.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vertex.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel.as_entire_binding(),
                },
            ],
        });
        let tiles = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light tiles masks"),
            layout: &self.tiles_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: info.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: grid.masks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: bits.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: layers.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(alpha),
                },
            ],
        });
        Ok(TiledPass {
            uniforms,
            tiles,
            items,
        })
    }
}

const DIFF_WGSL: &str = r#"
@group(0) @binding(0) var a: texture_2d<f32>;
@group(0) @binding(1) var b: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> worst: atomic<u32>;

@compute @workgroup_size(8, 8)
fn diff(@builtin(global_invocation_id) g: vec3u) {
    let size = textureDimensions(a);
    if (any(g.xy >= size)) {
        return;
    }
    let d = abs(textureLoad(a, g.xy, 0) - textureLoad(b, g.xy, 0));
    let m = max(max(d.x, d.y), max(d.z, d.w));
    // A NaN differs by everything.
    atomicMax(&worst, bitcast<u32>(select(1e30, m, m == m)));
}

// The colour channels alone: a masked run's tiled copy keeps the alpha
// the run began with, its per-light target the receivers' marks.
@compute @workgroup_size(8, 8)
fn diff_rgb(@builtin(global_invocation_id) g: vec3u) {
    let size = textureDimensions(a);
    if (any(g.xy >= size)) {
        return;
    }
    let d = abs(textureLoad(a, g.xy, 0) - textureLoad(b, g.xy, 0));
    let m = max(d.x, max(d.y, d.z));
    atomicMax(&worst, bitcast<u32>(select(1e30, m, m == m)));
}
"#;

/// The compare arm's GPU side: the largest per-channel difference
/// between two equal-sized colour targets.
pub struct Compare {
    pipeline: wgpu::ComputePipeline,
    rgb: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// One comparison, made now and recorded later ([`Diff::encode`]); its
/// result is [`Diff::readback`], four bytes to map after submission.
pub struct Diff {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    size: (u32, u32),
    worst: wgpu::Buffer,
    readback: wgpu::Buffer,
}

impl Compare {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light tiles compare"),
            entries: &[
                texture(0),
                texture(1),
                storage(2, wgpu::ShaderStages::COMPUTE, false),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("light tiles compare"),
            source: wgpu::ShaderSource::Wgsl(DIFF_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("light tiles compare"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let entry = |name| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("light tiles compare"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let (pipeline, rgb) = (entry("diff"), entry("diff_rgb"));
        Self {
            pipeline,
            rgb,
            layout,
        }
    }

    pub fn prepare(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Diff {
        self.prepare_channels(device, a, b, size, true)
    }

    /// [`Self::prepare`] over the colour channels, and alpha when `alpha`.
    pub fn prepare_channels(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
        size: (u32, u32),
        alpha: bool,
    ) -> Diff {
        let buffer = |usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("light tiles compare"),
                size: 4,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as B;
        let worst = buffer(B::STORAGE | B::COPY_SRC | B::COPY_DST);
        let readback = buffer(B::MAP_READ | B::COPY_DST);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light tiles compare"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: worst.as_entire_binding(),
                },
            ],
        });
        let pipeline = if alpha { &self.pipeline } else { &self.rgb };
        Diff {
            pipeline: pipeline.clone(),
            group,
            size,
            worst,
            readback,
        }
    }
}

impl Diff {
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.worst, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("light tiles compare"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(self.size.0.div_ceil(8), self.size.1.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&self.worst, 0, &self.readback, 0, 4);
    }

    /// The four bytes [`Self::encode`] leaves: the difference's bits.
    pub fn readback(&self) -> &wgpu::Buffer {
        &self.readback
    }
}

/// The largest difference [`Diff::readback`] holds.
pub fn difference(bytes: &[u8]) -> f32 {
    bytes.get(..4).map_or(f32::INFINITY, |b| {
        f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    })
}

#[cfg(test)]
mod tests;
