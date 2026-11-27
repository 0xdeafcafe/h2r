//! The flare query's count pass (docs/graphics/d3d-visibility.md, "The
//! query in Rust"): each query quad's samples that pass the guest's
//! `LESSEQUAL` test against the bound depth, counted exactly in one
//! compute pass, read back once the frame is submitted.
//!
//! A quad is an axis-aligned rectangle of guest pixels at one depth, so
//! the count needs no raster: a guest pixel passes when the quad's stored
//! depth ([`reference`]) is at most the depth texel at its centre (the
//! render scale's texel, as `gfx::occlusion` sums). wgpu's occlusion
//! queries are boolean on Metal, so they cannot give the count.

use scene_extract::flares::Quad;

/// How the pass is drawn: W-buffered (`ZEnable(2)`, what `0x0002dd30`
/// sets) with the device's W clip, or Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthTest {
    pub w_buffered: bool,
    pub range: [f32; 2],
    pub clamp: bool,
}

/// The stored depth a quad's fragments test (`[0, 1]`), as
/// `DepthModel::program` stores a W-buffered fragment: its `w` held to the
/// clip range and floored, over the full scale. `None`: the W clip culls
/// the quad, and it counts nothing.
pub fn reference(q: &Quad, test: DepthTest, z_scale: f32) -> Option<f32> {
    if z_scale <= 0.0 || z_scale.is_nan() {
        return None;
    }
    let unit = |v: f32| (v / z_scale).max(0.0).min(1.0);
    if !test.w_buffered {
        return Some(unit(q.z));
    }
    let [lo, hi] = test.range;
    if !test.clamp && (q.w < lo || q.w > hi) {
        return None;
    }
    Some(unit(q.w.max(lo).min(hi).floor()))
}

/// Workgroups along each side of a quad: half sizes are at most 128, so a
/// quad is at most 257 pixels wide.
pub const TILES: u32 = 17;

#[cfg(feature = "gpu")]
pub use gpu::{FlareQueryGpu, QueryReadback};

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::{Arc, Mutex};

    use super::{DepthTest, Quad, TILES, reference};

    const WGSL: &str = r#"
struct Quad { rect: vec4<i32>, depth: f32, p0: u32, p1: u32, p2: u32 };
struct Map { guest: vec2<u32>, host: vec2<u32>, clip: vec4<i32> };
@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var<storage, read> quads: array<Quad>;
@group(0) @binding(2) var<storage, read_write> counts: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> map: Map;
var<workgroup> partial: atomic<u32>;

// One workgroup per 16x16 tile of a quad (z the quad).
@compute @workgroup_size(16, 16)
fn count(@builtin(workgroup_id) tile: vec3<u32>, @builtin(local_invocation_id) at: vec3<u32>,
         @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicStore(&partial, 0u);
    }
    workgroupBarrier();
    let q = quads[tile.z];
    let lo = max(q.rect.xy, map.clip.xy);
    let hi = min(q.rect.zw, map.clip.zw);
    let g = q.rect.xy + vec2<i32>(tile.xy * 16u + at.xy);
    if (all(g >= lo) && all(g < hi)) {
        let p = min((vec2<u32>(g) * map.host + map.guest / 2u) / map.guest, map.host - 1u);
        if (q.depth <= textureLoad(depth, vec2<i32>(p), 0)) {
            atomicAdd(&partial, 1u);
        }
    }
    workgroupBarrier();
    if (li == 0u) {
        let n = atomicLoad(&partial);
        if (n != 0u) {
            atomicAdd(&counts[tile.z], n);
        }
    }
}
"#;

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct GpuQuad {
        rect: [i32; 4],
        depth: f32,
        pad: [u32; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct GpuMap {
        guest: [u32; 2],
        host: [u32; 2],
        clip: [i32; 4],
    }

    /// A culled quad's depth: above any stored depth.
    const CULLED: f32 = 2.0;

    /// The pass's pipeline on one device.
    pub struct FlareQueryGpu {
        layout: wgpu::BindGroupLayout,
        pipeline: wgpu::ComputePipeline,
    }

    impl FlareQueryGpu {
        pub fn new(device: &wgpu::Device) -> Self {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native flare query"),
                source: wgpu::ShaderSource::Wgsl(WGSL.into()),
            });
            let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("native flare query"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    storage(1, true),
                    storage(2, false),
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<GpuMap>() as u64),
                        },
                        count: None,
                    },
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native flare query"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("native flare query"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("count"),
                compilation_options: Default::default(),
                cache: None,
            });
            Self { layout, pipeline }
        }

        /// Records the counts of `quads` against `depth` (a single-sampled
        /// depth texture of `host` texels holding a target of `guest`
        /// pixels) into `encoder`, and their readback once it is
        /// submitted. `Err`: the depth cannot be read in a compute pass.
        #[allow(clippy::too_many_arguments)]
        pub fn record(
            &self,
            device: &wgpu::Device,
            encoder: &mut wgpu::CommandEncoder,
            depth: &wgpu::Texture,
            guest: (u32, u32),
            test: DepthTest,
            z_scale: f32,
            quads: &[Quad],
        ) -> Result<QueryReadback, &'static str> {
            use wgpu::TextureFormat as F;
            if !depth.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
                || depth.sample_count() != 1
                || !matches!(
                    depth.format(),
                    F::Depth16Unorm | F::Depth24Plus | F::Depth24PlusStencil8 | F::Depth32Float | F::Depth32FloatStencil8
                )
            {
                return Err("flare query depth is not readable");
            }
            if quads.is_empty() || guest.0 == 0 || guest.1 == 0 {
                return Err("flare query has nothing to count");
            }
            let n = quads.len() as u64;
            let gpu: Vec<GpuQuad> = quads
                .iter()
                .map(|q| GpuQuad {
                    rect: [q.x0, q.y0, q.x1, q.y1],
                    depth: reference(q, test, z_scale).unwrap_or(CULLED),
                    pad: [0; 3],
                })
                .collect();
            let map = GpuMap {
                guest: [guest.0, guest.1],
                host: [depth.width(), depth.height()],
                clip: [0, 0, guest.0 as i32, guest.1 as i32],
            };
            use wgpu::util::DeviceExt;
            let init = |label, contents: &[u8], usage| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            let quad_buffer = init("native flare query quads", bytemuck::cast_slice(&gpu), wgpu::BufferUsages::STORAGE);
            let map_buffer = init("native flare query map", bytemuck::bytes_of(&map), wgpu::BufferUsages::UNIFORM);
            let counts = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native flare query counts"),
                size: 4 * n,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native flare query readback"),
                size: 4 * n,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::DepthOnly,
                ..Default::default()
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native flare query"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: quad_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: counts.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: map_buffer.as_entire_binding() },
                ],
            });
            encoder.clear_buffer(&counts, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("native flare query"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(TILES, TILES, n as u32);
            }
            encoder.copy_buffer_to_buffer(&counts, 0, &readback, 0, 4 * n);
            let state = Arc::new(Mutex::new(State::Submitted));
            let mapped = Arc::clone(&state);
            encoder.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |r| {
                *mapped.lock().unwrap_or_else(|e| e.into_inner()) = if r.is_ok() { State::Mapped } else { State::Dropped };
            });
            Ok(QueryReadback { device: device.clone(), buffer: readback, count: quads.len(), state })
        }
    }

    enum State {
        /// Recorded; the map starts when the encoder is submitted.
        Submitted,
        Mapped,
        /// The GPU let the mapping go: nothing was counted.
        Dropped,
        Read(Arc<[u32]>),
    }

    /// The counts of one pass, in quad order.
    pub struct QueryReadback {
        device: wgpu::Device,
        buffer: wgpu::Buffer,
        count: usize,
        state: Arc<Mutex<State>>,
    }

    impl QueryReadback {
        /// The counts once the GPU has them, without waiting; `None` while
        /// pending. A dropped mapping counts 0 (logged once), as
        /// `gfx::occlusion`'s does.
        pub fn try_read(&self) -> Option<Arc<[u32]>> {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*s, State::Submitted) {
                drop(s);
                let _ = self.device.poll(wgpu::PollType::Poll);
                s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            }
            let v: Arc<[u32]> = match &*s {
                State::Submitted => return None,
                State::Read(v) => return Some(Arc::clone(v)),
                State::Dropped => {
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    ONCE.call_once(|| eprintln!("native flare query: a readback was dropped; it counts 0 (logged once)"));
                    vec![0; self.count].into()
                }
                State::Mapped => {
                    let v: Arc<[u32]> = {
                        let data = self.buffer.slice(..).get_mapped_range().ok()?;
                        bytemuck::cast_slice::<u8, u32>(&data).into()
                    };
                    self.buffer.unmap();
                    v
                }
            };
            *s = State::Read(Arc::clone(&v));
            Some(v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(z: f32, w: f32) -> Quad {
        Quad { x0: 0, y0: 0, x1: 4, y1: 4, z, w, area: 16 }
    }

    #[test]
    fn a_w_buffered_quad_tests_its_floored_w_over_the_full_scale() {
        let w = DepthTest { w_buffered: true, range: [0.0, 1000.0], clamp: true };
        assert_eq!(reference(&quad(5.0, 500.7), w, 1000.0), Some(0.5));
        assert_eq!(reference(&quad(5.0, 2000.0), w, 1000.0), Some(1.0), "clamped to the range");
        let culled = DepthTest { clamp: false, ..w };
        assert_eq!(reference(&quad(5.0, 2000.0), culled, 1000.0), None, "the W clip culls it");
        let z = DepthTest { w_buffered: false, ..w };
        assert_eq!(reference(&quad(250.0, 9.0), z, 1000.0), Some(0.25));
    }
}
