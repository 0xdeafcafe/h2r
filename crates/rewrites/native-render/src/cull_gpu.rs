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
    /// The pass's viewport in target pixels.
    pub viewport: [f32; 4],
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
    seed: wgpu::BindGroup,
    reduce: Vec<wgpu::BindGroup>,
    sizes: Vec<(u32, u32)>,
    late: wgpu::BindGroup,
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
    pyramid_layout: wgpu::BindGroupLayout,
    seed_layout: wgpu::BindGroupLayout,
    reduce_layout: wgpu::BindGroupLayout,
    seed: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    pyramid: Option<Pyramid>,
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
            entries: &[texture_entry(0, wgpu::TextureSampleType::Float { filterable: false })],
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
            pyramid: None,
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
        let view = CullView {
            planes: frustum.planes,
            view_proj: *view_proj,
            depth,
            w_row,
            viewport: [viewport.x, viewport.y, viewport.width, viewport.height],
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
            pass.set_bind_group(1, &self.pyramid.as_ref().expect("built above").late, &[]);
        } else {
            pass.set_pipeline(&self.late_frustum);
        }
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
        true
    }

    fn build_pyramid(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::Texture) {
        if self.pyramid.as_ref().is_none_or(|p| p.depth != *depth) {
            self.pyramid = Some(self.make_pyramid(device, depth));
        }
        let p = self.pyramid.as_ref().expect("made above");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("native world depth pyramid"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.seed);
        pass.set_bind_group(0, &p.seed, &[]);
        pass.dispatch_workgroups(p.sizes[0].0.div_ceil(TILE), p.sizes[0].1.div_ceil(TILE), 1);
        pass.set_pipeline(&self.reduce);
        for (group, &(w, h)) in p.reduce.iter().zip(&p.sizes[1..]) {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(w.div_ceil(TILE), h.div_ceil(TILE), 1);
        }
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
        let late = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world cull pyramid"),
            layout: &self.pyramid_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&whole) }],
        });
        Pyramid { depth: depth.clone(), seed, reduce, sizes, late }
    }

    /// A game view's frustum cull of the guest's selected parts, for a
    /// stage that encodes its own commands (`gfx::external::ExternalEncoded`):
    /// the view and the start set staged now in `uploads`, copied and
    /// dispatched in draw order when [`GuestDispatch::record`] runs, so
    /// each view's slots are written just before its draws read them.
    /// `selected` holds the selection's draw indices.
    pub(crate) fn guest_dispatch(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, uploads: &gfx::ring::SubmissionUploads, view_proj: &Mat4,
        frustum: &Frustum, selected: impl IntoIterator<Item = usize>,
    ) -> GuestDispatch {
        let view = CullView {
            planes: frustum.planes,
            view_proj: *view_proj,
            depth: [0.0; 4],
            w_row: [0.0; 4],
            viewport: [0.0; 4],
            counts: [self.count, 0, 0, 0],
        };
        let mut start = vec![0u32; self.count.max(1) as usize];
        for i in selected {
            if let Some(s) = start.get_mut(i) {
                *s = 1;
            }
        }
        GuestDispatch {
            view: (uploads.stage(device, queue, bytemuck::bytes_of(&view)), self.view.clone()),
            start: (uploads.stage(device, queue, bytemuck::cast_slice(&start)), self.start.clone()),
            start_len: start.len() as u64 * 4,
            stats: self.stats.clone(),
            pipeline: self.early_frustum.clone(),
            group: self.group.clone(),
            count: self.count,
        }
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

/// One game view's cull, recorded in draw order ([`GpuCull::guest_dispatch`]).
#[derive(Clone)]
pub struct GuestDispatch {
    view: ((wgpu::Buffer, u64), wgpu::Buffer),
    start: ((wgpu::Buffer, u64), wgpu::Buffer),
    start_len: u64,
    stats: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
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
        pass.dispatch_workgroups(self.count.div_ceil(GROUP), 1, 1);
    }
}

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
