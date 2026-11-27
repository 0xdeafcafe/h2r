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

/// Whether one pass of a light can draw tiled.
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
            (!writes.intersects(wgpu::ColorWrites::COLOR) || adds(&b.color))
                && (!writes.contains(wgpu::ColorWrites::ALPHA) || adds(&b.alpha))
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

// The light's colour where its tile lists it, else zero: an additive
// blend adds a zero source as nothing.
fn xl_tile_colour(v: XboxVaryings, c: vec4f) -> vec4f {
    let t = vec2u(v.position.xy) / xl_tiles.tile;
    let lit = (xl_masks[t.y * xl_tiles.tiles_x + t.x] & (1u << xl_bits[v.xl_light])) != 0u;
    return select(vec4f(0.0), c, lit);
}
"#;

/// A translated light pass ([`LightProgram::wgsl`]) made instanced: its
/// vertex and pixel uniforms arrays indexed by the instance, the instance
/// a flat varying, the colour zero where the tile does not list the
/// instance's light. [`TileRefusal::Program`] when the module lacks the
/// shape `shaders::link` gives.
pub fn tiled_wgsl(wgsl: &str) -> Result<String, TileRefusal> {
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
    Ok(format!(
        "var<private> xvs_light: u32;\nvar<private> xps_light: u32;\n{head}{body}{TILE_WGSL}"
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
}

/// One of the run's geometry ranges in layer 12 or 13 with the lights
/// that draw it: each light's bit in the [`TileGrid`] and its draw. The
/// draws share their pass, geometry, ranges and textures; a light that
/// draws the range twice is listed twice.
pub struct TiledDraw<'a> {
    pub layer: u8,
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
            ],
        });
        Self {
            bin,
            bin_layout,
            uniforms_layout,
            tiles_layout,
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
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("light tiles bin"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.bin);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(tx, ty, 1);
        }
        TileGrid {
            size,
            tiles: (tx, ty),
            masks,
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
        let wgsl = tiled_wgsl(&p.wgsl).map_err(|_| "light tiles program shape")?;
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
                blend: s.blend,
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
        let mut order: Vec<&TiledDraw<'_>> =
            draws.iter().filter(|d| !d.lights.is_empty()).collect();
        order.sort_by_key(|d| d.layer);
        let mut vertex_bytes = Vec::new();
        let mut pixel_bytes = Vec::new();
        let mut bits = Vec::new();
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
            let mut pass = first.gpu.try_lock().map_err(|_| "light pass busy")?;
            let (textures, _) = set
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
            ],
        });
        Ok(TiledPass {
            uniforms,
            tiles,
            items,
        })
    }
}

#[cfg(test)]
mod tests;
