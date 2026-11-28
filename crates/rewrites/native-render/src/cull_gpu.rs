//! The world's cull on the GPU (docs/graphics/native-world.md, "Cull").
//!
//! Each draw's world bounds are uploaded once ([`GpuCull::new`], again
//! after an edit, [`GpuCull::write_bounds`]); a frame uploads only the
//! view ([`CullView`]). Compute passes write `instance_count` into two
//! indirect buffers of one slot per draw ([`indirect_slots`]):
//!
//! | Phase | Writes | Draws |
//! |---|---|---|
//! | [`GpuCull::early`], at `prepare` | `early`: the frustum's set, or with occlusion its part the last test kept (`history`) | the world pass's first run |
//! | pyramid ([`GpuCull::late`]) | the depth target after the early run, reduced by max into a mip chain | |
//! | late ([`GpuCull::late`]) | `late`: the frustum's set the pyramid does not hide, less what `early` drew; `history` for the next frame | the second run |
//!
//! A draw first seen this frame is in `late`, so it is drawn the frame it
//! comes into view. The pyramid test is conservative (`cull.wgsl`,
//! `occluded`): it culls a box only when every pixel it can cover already
//! holds depth nearer than the nearest depth its fragments can write, so
//! the depth test would reject all of it. A view it cannot serve draws the
//! frustum's whole set and counts why ([`OcclusionSkips`]).

use crate::camera::Mat4;
use crate::cull::{CullStats, Frustum};
use crate::gpu::{DepthRule, PassViewport};
use crate::world::World;
use bytemuck::{Pod, Zeroable};

/// `wgpu`'s indexed indirect layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct IndirectSlot {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

/// Size of a slot in bytes.
pub const SLOT_SIZE: u64 = std::mem::size_of::<IndirectSlot>() as u64;

/// One slot per draw, in draw order, none drawn: the compute passes write
/// only `instance_count`.
pub fn indirect_slots(world: &World) -> Vec<IndirectSlot> {
    world
        .draws
        .iter()
        .map(|d| IndirectSlot {
            index_count: d.index_count,
            instance_count: 0,
            first_index: d.first_index,
            base_vertex: 0,
            first_instance: 0,
        })
        .collect()
}

/// A draw's bounds as `cull.wgsl` reads them; `min[3]` is 1 for a draw
/// with triangles and bounds, 0 for one never drawn.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuBounds {
    min: [f32; 4],
    max: [f32; 4],
}

fn gpu_bounds(world: &World) -> Vec<GpuBounds> {
    let mut b: Vec<_> = world
        .draws
        .iter()
        .map(|d| {
            let on = d.index_count != 0 && !d.bounds.is_empty();
            let (min, max) = if on { (d.bounds.min, d.bounds.max) } else { ([0.0; 3], [0.0; 3]) };
            GpuBounds {
                min: [min[0], min[1], min[2], f32::from(u8::from(on))],
                max: [max[0], max[1], max[2], 0.0],
            }
        })
        .collect();
    if b.is_empty() {
        b.push(GpuBounds::default());
    }
    b
}

/// The view `cull.wgsl` tests against (`View`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CullView {
    pub planes: [[f32; 4]; 6],
    pub view_proj: Mat4,
    /// [`DepthModel`]: full scale, floor flag, clamp range.
    pub depth: [f32; 4],
    /// [`DepthModel::w_row`].
    pub w_row: [f32; 4],
    /// [`ScreenMap::clip`].
    pub occlusion: Mat4,
    /// [`ScreenMap::screen`].
    pub screen: [f32; 4],
    /// [`ScreenMap::clamp`].
    pub clamp_px: [f32; 4],
    /// `x`: [`ScreenMap::margin`].
    pub margin: [f32; 4],
    /// `x`: the draw count.
    pub counts: [u32; 4],
}

/// How a view's draws store depth, for the pyramid test: the W-buffer
/// value of a world position `p` is `w_row · (p, 1)`, clamped to `range`,
/// floored to the format's step where `floor`, over `full_scale`. The
/// value is affine in `p`, so a box's least is at a corner, and the stored
/// depth is monotonic in it, so that corner bounds every fragment's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthModel {
    pub w_row: [f32; 4],
    pub full_scale: f32,
    pub floor: bool,
    pub range: [f32; 2],
}

impl DepthModel {
    /// The fixed stage (`world.wgsl`'s `w_depth`): the view depth `w` of
    /// the view-projection's last row, times `full_scale / far`.
    pub fn fixed(view_proj: &Mat4, rule: DepthRule, far: f32) -> Self {
        let k = rule.full_scale / far;
        Self {
            w_row: [view_proj[0][3] * k, view_proj[1][3] * k, view_proj[2][3] * k, view_proj[3][3] * k],
            full_scale: rule.full_scale,
            floor: rule.floor,
            range: [0.0, f32::MAX],
        }
    }

    /// A game view's material programs (`shaders::psh::wgsl`'s
    /// `xps_w_depth` with `program::frame_pixel`'s uniforms): the
    /// interpolated `w` of the device's `c[3]` row (the view-projection's
    /// w row, already in W-buffer units, `dph` with the world position),
    /// with no polygon offset, clamped to the pass's W clip range
    /// (`ViewInputs::w_clip`, the whole `[0, z_scale]` without one), the
    /// integer format's floor, over `z_scale`. A clipped fragment is not
    /// drawn and a clamped one stores the range's edge: both at least the
    /// clamped value.
    pub fn program(view: &crate::program::ViewInputs) -> Self {
        Self {
            w_row: view.constants[3],
            full_scale: view.z_scale,
            floor: true,
            range: view.w_clip.map_or([0.0, view.z_scale], |(range, _)| range),
        }
    }

    /// Whether the W-buffer row is the w row of `view_proj` times one
    /// positive scale (to `1e-3` of the row's size): the programs' depth
    /// and the camera's lens describe one view.
    pub fn agrees_with(&self, view_proj: &Mat4) -> bool {
        let w = [view_proj[0][3], view_proj[1][3], view_proj[2][3], view_proj[3][3]];
        let norm = |r: [f32; 4]| r.iter().map(|x| x * x).sum::<f32>().sqrt();
        let (a, b) = (norm(self.w_row), norm(w));
        if a == 0.0 || b == 0.0 || !a.is_finite() {
            return false;
        }
        let k = a / b;
        let dot: f32 = self.w_row.iter().zip(&w).map(|(x, y)| x * y).sum();
        dot > 0.0 && self.w_row.iter().zip(&w).all(|(x, y)| (x - y * k).abs() <= 1e-3 * a)
    }

    /// The least depth a fragment of `b` stores (the reference for
    /// `cull.wgsl`'s `w_depth` of the box's nearest corner).
    pub fn nearest(&self, b: &crate::world::Aabb) -> f32 {
        let r = self.w_row;
        let c = |k: usize| if r[k] >= 0.0 { b.min[k] } else { b.max[k] };
        let w = r[0] * c(0) + r[1] * c(1) + r[2] * c(2) + r[3];
        let z = w.clamp(self.range[0], self.range[1]);
        let q = if self.floor { z.floor() } else { z };
        (q / self.full_scale).clamp(0.0, 1.0)
    }

    fn uniform(&self) -> ([f32; 4], [f32; 4]) {
        ([self.full_scale, f32::from(u8::from(self.floor)), self.range[0], self.range[1]], self.w_row)
    }
}

/// Where a view's draws land in its target, for the pyramid test: the
/// clip position is `clip` times the world position, and its pixel
/// `clip.xy / clip.w * screen.xy + screen.zw`, inside `clamp` (first x,
/// y, last x, y), widened by `margin` pixels each side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMap {
    pub clip: Mat4,
    pub screen: [f32; 4],
    pub clamp: [f32; 4],
    pub margin: f32,
}

impl ScreenMap {
    /// A view-projection into a viewport of the target: the fixed stage's
    /// camera and pass viewport, or a game view's lens for its copy
    /// (`display_camera` at the copy's projection factor, as the fixed
    /// stage draws the game) into the request's rectangle in texels.
    pub fn viewport(view_proj: &Mat4, v: PassViewport) -> Self {
        Self {
            clip: *view_proj,
            screen: [v.width * 0.5, -v.height * 0.5, v.x + v.width * 0.5, v.y + v.height * 0.5],
            clamp: [v.x.floor(), v.y.floor(), (v.x + v.width).ceil() - 1.0, (v.y + v.height).ceil() - 1.0],
            margin: 1.0,
        }
    }

    /// `self` widened by `margin` pixels each side.
    pub fn with_margin(self, margin: f32) -> Self {
        Self { margin, ..self }
    }
}

/// Whether a view's depth target can seed the pyramid, or the counter of
/// why not ([`OcclusionSkips`]): a game view's guest depth surface is
/// sampleable (`gfx::surface`'s depth usage), but a multisampled one is
/// not.
pub fn depth_servable(t: &wgpu::Texture) -> bool {
    sampleable(t)
}

/// Views whose late cull ran without the pyramid, by why: the frustum's
/// whole set drew. Counted per view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OcclusionSkips {
    /// The world draws with material programs: their depth is the guest's
    /// W-buffer range, which the pyramid test does not model.
    pub programs: u64,
    /// The depth target cannot be read in a compute pass (no
    /// `TEXTURE_BINDING`, multisampled, more than one level or layer, or
    /// not a depth format).
    pub depth_unsampled: u64,
    /// The pass drew with another viewport than the one `prepare` uploaded.
    pub viewport_moved: u64,
    /// A game view's `c[3]` row is not the camera's w row times a positive
    /// scale ([`DepthModel::agrees_with`]): the programs' depth is not the
    /// lens the screen map is.
    pub depth_model: u64,
    /// A later layer's view drew no game view's pyramid this frame: none
    /// was built from this target for the same view, screen and depth
    /// model ([`GpuCull::served_pyramid`]), so the frustum alone culls it.
    pub layer_view: u64,
}

/// What [`GpuCull::early`] dispatched, for [`GpuCull::late`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Planned {
    /// The early set is the frustum's: no late run.
    Frustum,
    /// The early set is the history's: the late run follows, with the
    /// pyramid for this viewport.
    Occlusion(PassViewport),
}

/// The depth pyramid of one depth target.
struct Pyramid {
    depth: wgpu::Texture,
    build: PyramidBuild,
    late: wgpu::BindGroup,
    /// Every level, for another cull's group ([`GpuCull::served_pyramid`]).
    whole: wgpu::TextureView,
    /// Two words: the token of the game view whose build last ran on the
    /// GPU (`PyramidBuild::stamp`), which a later layer's cull compares
    /// with the one it expects (`cull.wgsl`'s `pyramid_token`).
    token: wgpu::Buffer,
    /// The game view whose early draws it was last built from: the
    /// caller's token, the screen and the depth model it was tested with.
    served: Option<(u64, ScreenMap, DepthModel)>,
}

/// The passes that build one pyramid, for recording later.
#[derive(Clone)]
struct PyramidBuild {
    seed_pipeline: wgpu::ComputePipeline,
    reduce_pipeline: wgpu::ComputePipeline,
    seed: wgpu::BindGroup,
    reduce: Vec<wgpu::BindGroup>,
    sizes: Vec<(u32, u32)>,
    /// The game view's token, staged, and the pyramid's token buffer:
    /// copied after the build, so the token names a build that ran.
    stamp: Option<((wgpu::Buffer, u64), wgpu::Buffer)>,
}

impl PyramidBuild {
    fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world depth pyramid"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.seed_pipeline);
        pass.set_bind_group(0, &self.seed, &[]);
        pass.dispatch_workgroups(self.sizes[0].0.div_ceil(TILE), self.sizes[0].1.div_ceil(TILE), 1);
        pass.set_pipeline(&self.reduce_pipeline);
        for (group, &(w, h)) in self.reduce.iter().zip(&self.sizes[1..]) {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(w.div_ceil(TILE), h.div_ceil(TILE), 1);
        }
        drop(pass);
        if let Some(((src, at), dst)) = &self.stamp {
            encoder.copy_buffer_to_buffer(src, *at, dst, 0, 8);
        }
    }
}

pub(crate) struct GpuCull {
    count: u32,
    /// The first run's slots.
    pub(crate) early: wgpu::Buffer,
    /// The second run's slots.
    pub(crate) late: wgpu::Buffer,
    bounds: wgpu::Buffer,
    view: wgpu::Buffer,
    stats: wgpu::Buffer,
    /// 1 per draw the view starts from (`cull.wgsl`'s `start`): all ones
    /// for the whole-world pass, the guest's selection for a game view
    /// ([`GpuCull::guest_dispatch`]).
    start: wgpu::Buffer,
    group: wgpu::BindGroup,
    early_frustum: wgpu::ComputePipeline,
    early_history: wgpu::ComputePipeline,
    late_occlusion: wgpu::ComputePipeline,
    late_frustum: wgpu::ComputePipeline,
    layer_occlusion: wgpu::ComputePipeline,
    pyramid_layout: wgpu::BindGroupLayout,
    seed_layout: wgpu::BindGroupLayout,
    reduce_layout: wgpu::BindGroupLayout,
    seed: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    /// The pyramids of the depth targets seen last (a game view's
    /// reference and display copies, split screen), newest last.
    pyramids: Vec<Pyramid>,
    planned: Option<Planned>,
    skips: OcclusionSkips,
}

const SHADER: &str = include_str!("cull.wgsl");
const PYRAMID_SHADER: &str = include_str!("cull_pyramid.wgsl");
const GROUP: u32 = 64;
const TILE: u32 = 8;

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_texture(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::R32Float,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device, module: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], entry: &str,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native world cull"),
        bind_group_layouts: &layouts.iter().map(|l| Some(*l)).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&layout),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// Whether the pyramid can read `t` as it is: one level and layer of a
/// single-sampled depth format the pass can bind.
fn sampleable(t: &wgpu::Texture) -> bool {
    use wgpu::TextureFormat as F;
    t.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
        && t.sample_count() == 1
        && t.mip_level_count() == 1
        && t.depth_or_array_layers() == 1
        && t.dimension() == wgpu::TextureDimension::D2
        && matches!(t.format(), F::Depth16Unorm | F::Depth24Plus | F::Depth24PlusStencil8 | F::Depth32Float | F::Depth32FloatStencil8)
}

impl GpuCull {
    pub(crate) fn new(device: &wgpu::Device, world: &World) -> Self {
        let init = |label, contents: &[u8], usage| {
            gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let mut slots = indirect_slots(world);
        if slots.is_empty() {
            slots.push(IndirectSlot::default());
        }
        let indirect = wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let early = init("native world early slots", bytemuck::cast_slice(&slots), indirect);
        let late = init("native world late slots", bytemuck::cast_slice(&slots), indirect);
        let bounds = init(
            "native world cull bounds",
            bytemuck::cast_slice(&gpu_bounds(world)),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let history = init(
            "native world cull history",
            bytemuck::cast_slice(&vec![0u32; slots.len()]),
            wgpu::BufferUsages::STORAGE,
        );
        let start = init(
            "native world cull start",
            bytemuck::cast_slice(&vec![1u32; slots.len()]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let buffer = |label, size, usage| {
            gfx::churn::create_buffer(device, &wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let view = buffer(
            "native world cull view",
            size_of::<CullView>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let stats = buffer(
            "native world cull stats",
            16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world cull"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, false),
                storage(3, false),
                storage(4, false),
                storage(5, false),
                storage(6, true),
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world cull"),
            layout: &layout,
            entries: &[&view, &bounds, &early, &late, &history, &stats, &start]
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() })
                .collect::<Vec<_>>(),
        });
        let pyramid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world cull pyramid"),
            entries: &[texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }), storage(1, true)],
        });
        let seed_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world pyramid seed"),
            entries: &[texture_entry(0, wgpu::TextureSampleType::Depth), storage_texture(1)],
        });
        let reduce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world pyramid reduce"),
            entries: &[texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }), storage_texture(3)],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world cull"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pyramid_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world pyramid"),
            source: wgpu::ShaderSource::Wgsl(PYRAMID_SHADER.into()),
        });
        Self {
            count: world.draws.len() as u32,
            early_frustum: pipeline(device, &module, &[&layout], "early_frustum"),
            early_history: pipeline(device, &module, &[&layout], "early_history"),
            late_occlusion: pipeline(device, &module, &[&layout, &pyramid_layout], "late_occlusion"),
            late_frustum: pipeline(device, &module, &[&layout], "late_frustum"),
            layer_occlusion: pipeline(device, &module, &[&layout, &pyramid_layout], "layer_occlusion"),
            seed: pipeline(device, &pyramid_module, &[&seed_layout], "seed"),
            reduce: pipeline(device, &pyramid_module, &[&reduce_layout], "reduce"),
            early,
            late,
            bounds,
            view,
            stats,
            start,
            group,
            pyramid_layout,
            seed_layout,
            reduce_layout,
            pyramids: Vec::new(),
            planned: None,
            skips: OcclusionSkips::default(),
        }
    }

    /// The draws' bounds again, after an edit moved them.
    pub(crate) fn write_bounds(&self, queue: &wgpu::Queue, world: &World) {
        queue.write_buffer(&self.bounds, 0, bytemuck::cast_slice(&gpu_bounds(world)));
    }

    pub(crate) fn skips(&self) -> OcclusionSkips {
        self.skips
    }

    /// Uploads the view and writes the early slots. `occlusion`: the late
    /// run follows ([`GpuCull::late`]) and the early set is the history's;
    /// otherwise the early set is the frustum's whole set.
    pub(crate) fn early(
        &mut self, uploads: &mut gfx::ring::EncoderUploads<'_>, view_proj: &Mat4, frustum: &Frustum, depth: DepthRule,
        far: f32, viewport: PassViewport, occlusion: bool,
    ) {
        let (depth, w_row) = DepthModel::fixed(view_proj, depth, far).uniform();
        let map = ScreenMap::viewport(view_proj, viewport);
        let view = CullView {
            planes: frustum.planes,
            view_proj: *view_proj,
            depth,
            w_row,
            occlusion: map.clip,
            screen: map.screen,
            clamp_px: map.clamp,
            margin: [map.margin, 0.0, 0.0, 0.0],
            counts: [self.count, 0, 0, 0],
        };
        uploads.write(&self.view, 0, bytemuck::bytes_of(&view));
        uploads.encoder.clear_buffer(&self.stats, 0, None);
        if !occlusion {
            self.skips.programs += 1;
        }
        self.planned = Some(if occlusion { Planned::Occlusion(viewport) } else { Planned::Frustum });
        if self.count == 0 {
            return;
        }
        let mut pass = uploads.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world early cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(if occlusion { &self.early_history } else { &self.early_frustum });
        pass.set_bind_group(0, &self.group, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
    }

    /// After the early run: builds the pyramid of `depth` and writes the
    /// late slots. False when [`GpuCull::early`] planned no late run.
    pub(crate) fn late(
        &mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView,
        viewport: PassViewport,
    ) -> bool {
        let Some(Planned::Occlusion(planned)) = self.planned.take() else { return false };
        let texture = depth.texture();
        let served = if !sampleable(texture) {
            self.skips.depth_unsampled += 1;
            false
        } else if planned != viewport {
            self.skips.viewport_moved += 1;
            false
        } else {
            true
        };
        if self.count == 0 {
            return true;
        }
        if served {
            self.build_pyramid(device, encoder, texture);
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world late cull"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        if served {
            pass.set_pipeline(&self.late_occlusion);
            pass.set_bind_group(1, &self.pyramids.last().expect("built above").late, &[]);
        } else {
            pass.set_pipeline(&self.late_frustum);
        }
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
        true
    }

    fn build_pyramid(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::Texture) {
        self.pyramid_for(device, depth).build.record(encoder);
    }

    /// `depth`'s pyramid, made on first use, kept as the newest.
    fn pyramid_for(&mut self, device: &wgpu::Device, depth: &wgpu::Texture) -> &mut Pyramid {
        if let Some(at) = self.pyramids.iter().position(|p| p.depth == *depth) {
            let p = self.pyramids.remove(at);
            self.pyramids.push(p);
        } else {
            if self.pyramids.len() >= 4 {
                self.pyramids.remove(0);
            }
            let p = self.make_pyramid(device, depth);
            self.pyramids.push(p);
        }
        self.pyramids.last_mut().expect("pushed above")
    }

    fn make_pyramid(&self, device: &wgpu::Device, depth: &wgpu::Texture) -> Pyramid {
        let (width, height) = (depth.width(), depth.height());
        let levels = 32 - width.max(height).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native world depth pyramid"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let level = |k: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: k,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let views: Vec<_> = (0..levels).map(level).collect();
        let sizes = (0..levels).map(|k| ((width >> k).max(1), (height >> k).max(1))).collect();
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        let seed = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world pyramid seed"),
            layout: &self.seed_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[0]) },
            ],
        });
        let reduce = views
            .windows(2)
            .map(|pair| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native world pyramid reduce"),
                    layout: &self.reduce_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&pair[0]) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&pair[1]) },
                    ],
                })
            })
            .collect();
        let whole = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let token = gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
            label: Some("native world pyramid token"),
            contents: bytemuck::cast_slice(&[u32::MAX; 2]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let late = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world cull pyramid"),
            layout: &self.pyramid_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&whole) },
                wgpu::BindGroupEntry { binding: 1, resource: token.as_entire_binding() },
            ],
        });
        let build = PyramidBuild {
            seed_pipeline: self.seed.clone(), reduce_pipeline: self.reduce.clone(), seed, reduce, sizes, stamp: None,
        };
        Pyramid { depth: depth.clone(), build, late, whole, token, served: None }
    }

    /// A game view's cull of the guest's selected parts, for a stage that
    /// encodes its own commands (`gfx::external::ExternalEncoded`): the
    /// view and the start set staged now in `uploads`, copied and
    /// dispatched in draw order when [`GuestDispatch::record`] runs, so
    /// each view's slots are written just before its draws read them.
    /// `start` is per draw (`cull.wgsl`'s `start`: 0, 1, or [`ORDERED`]).
    /// With `occlusion` (the programs' depth and screen, the depth target)
    /// the early set is the history's and [`GuestDispatch::record_late`]
    /// tests the rest against the pyramid; a target the pyramid cannot
    /// read is counted and drawn by the frustum alone.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn guest_dispatch(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads, view_proj: &Mat4,
        frustum: &Frustum, start: &[u32], occlusion: Option<(DepthModel, ScreenMap, &wgpu::Texture, u64)>,
    ) -> GuestDispatch {
        let occlusion = occlusion.filter(|(model, map, depth, _)| {
            if !sampleable(depth) {
                self.skips.depth_unsampled += 1;
                false
            } else if !model.agrees_with(&map.clip) {
                self.skips.depth_model += 1;
                false
            } else {
                true
            }
        });
        let view = self.dispatch_view(view_proj, frustum, occlusion.as_ref().map(|(model, map, ..)| (model, map)));
        let late = occlusion.map(|(model, map, depth, token)| {
            let words = token_words(token);
            let staged = uploads.stage(device, queue, bytemuck::cast_slice(&words));
            let p = self.pyramid_for(device, depth);
            p.served = Some((token, map, model));
            let mut build = p.build.clone();
            build.stamp = Some((staged, p.token.clone()));
            (build, p.late.clone())
        });
        let pipeline = if late.is_some() { self.early_history.clone() } else { self.early_frustum.clone() };
        let mut dispatch = self.staged(device, queue, uploads, &view, start, pipeline, None);
        dispatch.late = late.map(|(build, pyramid)| (build, self.late_occlusion.clone(), pyramid));
        dispatch
    }

    /// The view a dispatch tests against, with the pyramid's depth and
    /// screen when it reads one.
    fn dispatch_view(&self, view_proj: &Mat4, frustum: &Frustum, occlusion: Option<(&DepthModel, &ScreenMap)>) -> CullView {
        cull_view(view_proj, frustum, occlusion, self.count)
    }

    /// `view` and `start` staged for [`GuestDispatch::record`].
    #[allow(clippy::too_many_arguments)]
    fn staged(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads, view: &CullView,
        start: &[u32], pipeline: wgpu::ComputePipeline, early_pyramid: Option<wgpu::BindGroup>,
    ) -> GuestDispatch {
        let mut words = vec![0u32; self.count.max(1) as usize];
        for (w, &s) in words.iter_mut().zip(start) {
            *w = s;
        }
        GuestDispatch {
            view: (uploads.stage(device, queue, bytemuck::bytes_of(view)), self.view.clone()),
            start: (uploads.stage(device, queue, bytemuck::cast_slice(&words)), self.start.clone()),
            start_len: words.len() as u64 * 4,
            stats: self.stats.clone(),
            pipeline,
            early_pyramid,
            late: None,
            group: self.group.clone(),
            count: self.count,
        }
    }

    /// The pyramid of `depth` and its token buffer when the game view
    /// `token` planned its build last, tested with this screen and depth
    /// model: what a later layer of that view may test against
    /// ([`GpuCull::layer_dispatch`]). Planned is not run: the layer's cull
    /// also compares the token the build stamped on the GPU.
    pub(crate) fn served_pyramid(
        &self, depth: &wgpu::Texture, token: u64, map: &ScreenMap, model: &DepthModel,
    ) -> Option<ServedPyramid> {
        let p = self.pyramids.iter().find(|p| p.depth == *depth)?;
        (p.served == Some((token, *map, *model))).then(|| ServedPyramid { token, view: p.whole.clone(), stamp: p.token.clone() })
    }

    /// A later layer's cull of its structure items in one run, staged as
    /// [`GpuCull::guest_dispatch`]'s is: the frustum's set of `start`
    /// (0, [`OCCLUDABLE`] or [`ORDERED`]), less, with `occlusion` (the
    /// layer's depth model and screen and a game view's pyramid,
    /// [`GpuCull::served_pyramid`]), the occludable items that pyramid
    /// hides. The early slots are the layer's; no late run follows.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layer_dispatch(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads, view_proj: &Mat4,
        frustum: &Frustum, start: &[u32], occlusion: Option<(DepthModel, ScreenMap, ServedPyramid)>,
    ) -> GuestDispatch {
        if occlusion.is_none() {
            self.skips.layer_view += 1;
        }
        let mut view = self.dispatch_view(view_proj, frustum, occlusion.as_ref().map(|(model, map, _)| (model, map)));
        let (pipeline, group) = match &occlusion {
            Some((_, _, pyramid)) => {
                let words = token_words(pyramid.token);
                view.counts[1] = words[0];
                view.counts[2] = words[1];
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native layer cull pyramid"),
                    layout: &self.pyramid_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pyramid.view) },
                        wgpu::BindGroupEntry { binding: 1, resource: pyramid.stamp.as_entire_binding() },
                    ],
                });
                (self.layer_occlusion.clone(), Some(group))
            }
            None => (self.early_frustum.clone(), None),
        };
        self.staged(device, queue, uploads, &view, start, pipeline, group)
    }

    /// The last view's counts, read back (blocking): for tests and tools.
    pub(crate) fn read_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> CullStats {
        let words: Vec<u32> = read_back(device, queue, &self.stats, 16);
        CullStats {
            visible: words[0],
            drawn: words[1] + words[2],
            occluded: words[3],
            total: self.count,
        }
    }

    /// Whether each draw was drawn in the last view (early or late), read
    /// back (blocking): for tests and tools.
    pub(crate) fn read_drawn(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<bool> {
        let size = u64::from(self.count.max(1)) * SLOT_SIZE;
        let early: Vec<IndirectSlot> = read_back(device, queue, &self.early, size);
        let late: Vec<IndirectSlot> = read_back(device, queue, &self.late, size);
        early
            .iter()
            .zip(&late)
            .take(self.count as usize)
            .map(|(e, l)| e.instance_count != 0 || l.instance_count != 0)
            .collect()
    }
}

/// The view a dispatch of `count` items tests against, with the
/// pyramid's depth and screen when it reads one.
fn cull_view(view_proj: &Mat4, frustum: &Frustum, occlusion: Option<(&DepthModel, &ScreenMap)>, count: u32) -> CullView {
    let (depth, w_row) = occlusion.map_or(([1.0, 0.0, 0.0, 0.0], [0.0; 4]), |(m, _)| m.uniform());
    let map = occlusion.map(|(_, map)| *map);
    CullView {
        planes: frustum.planes,
        view_proj: *view_proj,
        depth,
        w_row,
        occlusion: map.map_or([[0.0; 4]; 4], |m| m.clip),
        screen: map.map_or([0.0; 4], |m| m.screen),
        clamp_px: map.map_or([0.0; 4], |m| m.clamp),
        margin: [map.map_or(0.0, |m| m.margin), 0.0, 0.0, 0.0],
        counts: [count, 0, 0, 0],
    }
}

/// A game view's pyramid a later layer may test against
/// ([`GpuCull::served_pyramid`]).
pub(crate) struct ServedPyramid {
    token: u64,
    view: wgpu::TextureView,
    stamp: wgpu::Buffer,
}

/// A token as the pyramid's two words: low, high. Never both `u32::MAX`,
/// the words of a pyramid no game view built.
fn token_words(token: u64) -> [u32; 2] {
    let token = token.min(u64::MAX - 1);
    [token as u32, (token >> 32) as u32]
}

/// One game view's cull, recorded in draw order ([`GpuCull::guest_dispatch`]).
#[derive(Clone)]
pub struct GuestDispatch {
    view: ((wgpu::Buffer, u64), wgpu::Buffer),
    start: ((wgpu::Buffer, u64), wgpu::Buffer),
    start_len: u64,
    stats: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    /// The early run's pyramid group: a later layer's, which reads a game
    /// view's ([`GpuCull::layer_dispatch`]).
    early_pyramid: Option<wgpu::BindGroup>,
    /// The pyramid build, the late cull and the pyramid's group.
    late: Option<(PyramidBuild, wgpu::ComputePipeline, wgpu::BindGroup)>,
    group: wgpu::BindGroup,
    count: u32,
}

impl GuestDispatch {
    /// Copies the staged view and start set and writes the early slots.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let ((src, at), dst) = &self.view;
        encoder.copy_buffer_to_buffer(src, *at, dst, 0, size_of::<CullView>() as u64);
        let ((src, at), dst) = &self.start;
        encoder.copy_buffer_to_buffer(src, *at, dst, 0, self.start_len);
        encoder.clear_buffer(&self.stats, 0, None);
        if self.count == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world guest cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        if let Some(pyramid) = &self.early_pyramid {
            pass.set_bind_group(1, pyramid, &[]);
        }
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
    }

    /// Whether a late run follows the early one: the pyramid is read.
    pub fn has_late(&self) -> bool {
        self.late.is_some()
    }

    /// After the early run's draws: builds the pyramid of the depth target
    /// and writes the late slots. Nothing without occlusion.
    pub fn record_late(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some((build, pipeline, pyramid)) = &self.late else { return };
        if self.count == 0 {
            return;
        }
        build.record(encoder);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world guest late cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_bind_group(1, pyramid, &[]);
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
    }
}

/// [`GpuCull::guest_dispatch`]'s start value of a draw that keeps its
/// place in the order: the frustum alone culls it, in the early run.
pub const ORDERED: u32 = 2;
/// A draw the pyramid may cull.
pub const OCCLUDABLE: u32 = 1;

fn read_back<T: Pod>(device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Buffer, size: u64) -> Vec<T> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native world cull read back"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(src, 0, &buffer, 0, size);
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map the cull read back"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll the cull read back");
    bytemuck::pod_collect_to_vec(&buffer.slice(..).get_mapped_range().expect("mapped"))
}

/// One batch of a decorator plan as `cull_decorators.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct DecoratorBatch {
    first: u32,
    count: u32,
    index_count: u32,
    first_index: u32,
    occludable: u32,
    pad: [u32; 3],
}

/// The decorators' cull on the GPU (docs/graphics/native-decorators.md,
/// "Cull"): per batch of a view's plan, the instances whose placement
/// group's sphere the frustum keeps (and, with this view's pyramid, that
/// it does not hide) compacted in the guest's order into `dst`, and one
/// indirect slot per batch drawing them. Its buffers grow to the largest
/// plan; each dispatch copies its inputs in draw order.
pub struct DecoratorCull {
    view: wgpu::Buffer,
    batches: wgpu::Buffer,
    spheres: wgpu::Buffer,
    src: wgpu::Buffer,
    dst: wgpu::Buffer,
    slots: wgpu::Buffer,
    /// Instances and batches the buffers hold.
    capacity: (u64, u64),
    view_group: wgpu::BindGroup,
    buffers_group: wgpu::BindGroup,
    buffers_layout: wgpu::BindGroupLayout,
    pyramid_layout: wgpu::BindGroupLayout,
    frustum: wgpu::ComputePipeline,
    occlusion: wgpu::ComputePipeline,
    /// Views culled without this view's pyramid, and instances whose group
    /// sphere did not read (drawn, never culled).
    pub skips: DecoratorSkips,
}

/// What [`DecoratorCull`] could not test, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecoratorSkips {
    /// Views with no pyramid of their layer 3 on the target
    /// ([`GpuCull::served_pyramid`]): the frustum alone culls.
    pub no_pyramid: u64,
    /// Instances whose placement group's sphere did not read: drawn.
    pub unbounded: u64,
}

const DECORATOR_SHADER: &str = include_str!("cull_decorators.wgsl");

impl DecoratorCull {
    pub fn new(device: &wgpu::Device) -> Self {
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native decorator cull view"),
            entries: &[uniform],
        });
        let pyramid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native decorator cull pyramid"),
            entries: &[texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }), storage(1, true)],
        });
        let buffers_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native decorator cull buffers"),
            entries: &[storage(0, true), storage(1, true), storage(2, true), storage(3, false), storage(4, false)],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native decorator cull"),
            source: wgpu::ShaderSource::Wgsl(format!("{SHADER}\n{DECORATOR_SHADER}").into()),
        });
        let layout = |groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native decorator cull"),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let compute = |layout: &wgpu::PipelineLayout, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let frustum = compute(&layout(&[Some(&view_layout), None, Some(&buffers_layout)]), "decorators_frustum");
        let occlusion =
            compute(&layout(&[Some(&view_layout), Some(&pyramid_layout), Some(&buffers_layout)]), "decorators_occlusion");
        let view = gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
            label: Some("native decorator cull view"),
            size: size_of::<CullView>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native decorator cull view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: view.as_entire_binding() }],
        });
        let (batches, spheres, src, dst, slots, buffers_group) = Self::buffers(device, &buffers_layout, (64, 16));
        Self {
            view,
            batches,
            spheres,
            src,
            dst,
            slots,
            capacity: (64, 16),
            view_group,
            buffers_group,
            buffers_layout,
            pyramid_layout,
            frustum,
            occlusion,
            skips: DecoratorSkips::default(),
        }
    }

    #[allow(clippy::type_complexity)]
    fn buffers(
        device: &wgpu::Device, layout: &wgpu::BindGroupLayout, (instances, batches): (u64, u64),
    ) -> (wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup) {
        use wgpu::BufferUsages as U;
        let buffer = |label, size: u64, usage| {
            gfx::churn::create_buffer(device, &wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let stride = crate::decorators::INSTANCE_STRIDE;
        let batch = buffer("native decorator cull batches", batches * size_of::<DecoratorBatch>() as u64, U::STORAGE | U::COPY_DST);
        let spheres = buffer("native decorator cull spheres", instances * 16, U::STORAGE | U::COPY_DST);
        let src = buffer("native decorator cull instances", instances * stride, U::STORAGE | U::COPY_DST);
        let dst = buffer("native decorator culled instances", instances * stride, U::STORAGE | U::VERTEX);
        let slots = buffer("native decorator cull slots", batches * SLOT_SIZE, U::STORAGE | U::INDIRECT);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native decorator cull buffers"),
            layout,
            entries: &[&batch, &spheres, &src, &dst, &slots]
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() })
                .collect::<Vec<_>>(),
        });
        (batch, spheres, src, dst, slots, group)
    }

    /// `plan`'s cull, staged now and recorded in draw order
    /// ([`DecoratorDispatch::record`]). `occludable` is per batch
    /// (`gpu::depth_hides` of its pipeline); `occlusion` the view's depth
    /// model and screen and its layer 3's pyramid
    /// ([`GpuCull::served_pyramid`]), without which the frustum alone
    /// culls.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dispatch(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads,
        plan: &crate::decorators::Plan, occludable: &[bool], view_proj: &Mat4, frustum: &Frustum,
        occlusion: Option<(DepthModel, ScreenMap, ServedPyramid)>,
    ) -> DecoratorDispatch {
        let (instances, batches) = (plan.instances.len() as u64, plan.batches.len() as u64);
        if instances > self.capacity.0 || batches > self.capacity.1 {
            let capacity = (instances.max(self.capacity.0).next_power_of_two(), batches.max(self.capacity.1).next_power_of_two());
            (self.batches, self.spheres, self.src, self.dst, self.slots, self.buffers_group) =
                Self::buffers(device, &self.buffers_layout, capacity);
            self.capacity = capacity;
        }
        if occlusion.is_none() {
            self.skips.no_pyramid += 1;
        }
        self.skips.unbounded += plan.spheres.iter().filter(|s| !s[3].is_finite()).count() as u64;
        let mut view = cull_view(view_proj, frustum, occlusion.as_ref().map(|(m, map, _)| (m, map)), batches as u32);
        let pyramid = occlusion.map(|(_, _, pyramid)| {
            let words = token_words(pyramid.token);
            view.counts[1] = words[0];
            view.counts[2] = words[1];
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native decorator cull pyramid"),
                layout: &self.pyramid_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pyramid.view) },
                    wgpu::BindGroupEntry { binding: 1, resource: pyramid.stamp.as_entire_binding() },
                ],
            })
        });
        let rows: Vec<DecoratorBatch> = plan
            .batches
            .iter()
            .zip(occludable.iter().copied().chain(std::iter::repeat(false)))
            .map(|(b, occludable)| DecoratorBatch {
                first: b.instances.start,
                count: b.instances.end - b.instances.start,
                index_count: b.index_count,
                first_index: b.first_index,
                occludable: u32::from(occludable),
                pad: [0; 3],
            })
            .collect();
        let mut copies = Vec::new();
        let mut stage = |bytes: &[u8], dst: &wgpu::Buffer| {
            if !bytes.is_empty() {
                copies.push((uploads.stage(device, queue, bytes), dst.clone(), bytes.len() as u64));
            }
        };
        stage(bytemuck::bytes_of(&view), &self.view);
        stage(bytemuck::cast_slice(&rows), &self.batches);
        stage(bytemuck::cast_slice(&plan.spheres), &self.spheres);
        stage(plan.instance_bytes(), &self.src);
        DecoratorDispatch {
            copies,
            pipeline: if pyramid.is_some() { self.occlusion.clone() } else { self.frustum.clone() },
            view_group: self.view_group.clone(),
            pyramid,
            buffers_group: self.buffers_group.clone(),
            batches: batches as u32,
            instances: self.dst.clone(),
            slots: self.slots.clone(),
        }
    }
}

/// One view's decorator cull, recorded in draw order
/// ([`DecoratorCull::dispatch`]): the instances to draw are `instances`
/// from each batch's first, as many as its slot in `slots` says.
#[derive(Clone)]
pub struct DecoratorDispatch {
    copies: Vec<((wgpu::Buffer, u64), wgpu::Buffer, u64)>,
    pipeline: wgpu::ComputePipeline,
    view_group: wgpu::BindGroup,
    pyramid: Option<wgpu::BindGroup>,
    buffers_group: wgpu::BindGroup,
    batches: u32,
    /// The compacted `v11..v15`, at each batch's instance range.
    pub instances: wgpu::Buffer,
    /// One indexed indirect slot per batch.
    pub slots: wgpu::Buffer,
}

impl DecoratorDispatch {
    /// Copies the staged inputs and writes the compacted instances and the
    /// batches' slots.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        for ((src, at), dst, len) in &self.copies {
            encoder.copy_buffer_to_buffer(src, *at, dst, 0, *len);
        }
        if self.batches == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native decorator cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.view_group, &[]);
        if let Some(pyramid) = &self.pyramid {
            pass.set_bind_group(1, pyramid, &[]);
        }
        pass.set_bind_group(2, &self.buffers_group, &[]);
        pass.dispatch_workgroups(self.batches.div_ceil(GROUP), 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::RenderCamera;
    use crate::world::{LightmapKey, Owner, PartInput, Placement, SectionInput, WorldBuilder};

    /// A later layer's cull reads a game view's pyramid only when that
    /// view's build ran on the GPU for the token it expects: a box behind
    /// the depth is hidden then, an `ORDERED` one is not, and a token whose
    /// build was planned but never recorded culls by the frustum alone.
    #[test]
    fn a_later_layer_reads_the_pyramid_its_game_view_built() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        const SIZE: u32 = 32;
        let quad = |x: f32| SectionInput {
            positions: vec![[x, 1.0, -1.0], [x, -1.0, -1.0], [x, 1.0, 1.0], [x, -1.0, 1.0]],
            uvs: vec![[0.5; 2]; 4],
            lightmap_uvs: vec![[0.5; 2]; 4],
            normals: vec![[-1.0, 0.0, 0.0]; 4],
            parts: vec![PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
            ..Default::default()
        };
        let key = LightmapKey { page: 0, palette: Some(0) };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &quad(20.0), &Placement::IDENTITY, Some(key));
        b.add_section(Owner::Cluster(1), &quad(20.0), &Placement::IDENTITY, Some(key));
        let world = b.finish();
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 60f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, SIZE as i16, SIZE as i16],
            near: 0.0625,
            far: 1024.0,
        };
        let vp = camera.view_projection();
        let frustum = Frustum::sides(&vp);
        let map = ScreenMap::viewport(&vp, PassViewport { x: 0.0, y: 0.0, width: SIZE as f32, height: SIZE as f32 });
        // The W-buffer value is the view depth, over 100: the boxes store 0.2.
        let k = 1.0 / vp[0][3];
        let model = DepthModel {
            w_row: [vp[0][3] * k, vp[1][3] * k, vp[2][3] * k, vp[3][3] * k],
            full_scale: 100.0,
            floor: false,
            range: [0.0, 100.0],
        };
        assert!(model.agrees_with(&vp));
        let depth = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let mut guest = GpuCull::new(&ctx.device, &world);
        let mut layer = GpuCull::new(&ctx.device, &world);
        let run = |record: &dyn Fn(&mut wgpu::CommandEncoder), arena: gfx::ring::SubmissionUploads| {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            record(&mut enc);
            let front = arena.upload_commands(&ctx.queue);
            ctx.queue.submit(front.into_iter().chain([enc.finish()]));
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        };
        // The game view: its depth nearer than the boxes everywhere, the
        // pyramid built from it for token 7.
        let arena = gfx::ring::UploadPool::default().begin();
        let view = guest.guest_dispatch(&ctx.device, &ctx.queue, &arena, &vp, &frustum, &[OCCLUDABLE; 2],
            Some((model, map, &depth, 7)));
        assert!(view.has_late());
        let dv = depth.create_view(&Default::default());
        run(&|enc| {
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv, stencil_ops: None,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.1), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            }));
            view.record(enc);
            view.record_late(enc);
        }, arena);
        let start = [OCCLUDABLE, ORDERED];
        let layer_run = |layer: &mut GpuCull, served: Option<ServedPyramid>| {
            let arena = gfx::ring::UploadPool::default().begin();
            let d = layer.layer_dispatch(&ctx.device, &ctx.queue, &arena, &vp, &frustum, &start,
                served.map(|s| (model, map, s)));
            run(&|enc| d.record(enc), arena);
            (layer.read_drawn(&ctx.device, &ctx.queue), layer.read_stats(&ctx.device, &ctx.queue).occluded)
        };
        let served = guest.served_pyramid(&depth, 7, &map, &model);
        assert!(served.is_some(), "token 7 planned the build");
        assert_eq!(layer_run(&mut layer, served), (vec![false, true], 1), "hidden, the ordered box kept");
        assert!(guest.served_pyramid(&depth, 8, &map, &model).is_none(), "another view's token");
        assert!(guest.served_pyramid(&depth, 7, &map.with_margin(5.0), &model).is_none(), "another screen");
        assert_eq!(layer_run(&mut layer, None), (vec![true, true], 0), "no pyramid: the frustum's set");
        assert_eq!(layer.skips().layer_view, 1);
        // Planned for token 9 and never recorded: the GPU still holds 7.
        let arena = gfx::ring::UploadPool::default().begin();
        let _ = guest.guest_dispatch(&ctx.device, &ctx.queue, &arena, &vp, &frustum, &[OCCLUDABLE; 2],
            Some((model, map, &depth, 9)));
        drop(arena);
        let served = guest.served_pyramid(&depth, 9, &map, &model);
        assert!(served.is_some());
        assert_eq!(layer_run(&mut layer, served), (vec![true, true], 0), "the build that ran was 7's");
    }
}
