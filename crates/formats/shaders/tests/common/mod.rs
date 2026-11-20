//! Shared test helpers: naga validation through every backend we care about,
//! a tiny deterministic RNG, and a GPU compute harness that runs generated
//! shader functions on real hardware.
#![allow(dead_code)]

use wgpu::util::DeviceExt;

/// Parse + validate with naga, then also lower to MSL (the macOS target)
/// to catch backend-only failures. Panics with the source on error.
pub fn check(source: &str) -> naga::Module {
    let (module, info) = match shaders::validate(source) {
        Ok(v) => v,
        Err(e) => panic!("naga rejected generated WGSL:\n{e}\n---\n{source}"),
    };
    let options = naga::back::msl::Options {
        lang_version: (2, 1),
        ..Default::default()
    };
    let pipeline = naga::back::msl::PipelineOptions::default();
    if let Err(e) = naga::back::msl::write_string(&module, &info, &options, &pipeline) {
        panic!("MSL backend failed: {e:?}\n---\n{source}");
    }
    module
}

/// xorshift64*.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn f(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * u
    }
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// A texture for [`Gpu::run_with_textures`]: bound at (1, `binding`), with
/// a nearest-filtering, repeating sampler at (1, `binding + 1`).
pub struct GpuTexture {
    pub binding: u32,
    pub view: wgpu::TextureView,
    pub dim: wgpu::TextureViewDimension,
}

/// A GPU, or `None` (tests then skip with a message).
pub fn gpu() -> Option<Gpu> {
    if std::env::var_os("SHADERS_NO_GPU").is_some() {
        return None;
    }
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()?;
        Some(Gpu { device, queue })
    })
}

impl Gpu {
    /// Runs `entry` of `source` over `invocations` with uniform buffer
    /// `uniform` at (group 0, `uniform_binding`), read-only storage `input`
    /// at (1, 8) and read-write storage of `output_floats` at (1, 9).
    // Mirrors the dispatch's bindings one to one; a struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        source: &str,
        entry: &str,
        uniform_binding: u32,
        uniform: &[u8],
        input: &[f32],
        output_floats: usize,
        invocations: u32,
    ) -> Vec<f32> {
        self.run_with_textures(
            source,
            entry,
            uniform_binding,
            uniform,
            input,
            output_floats,
            invocations,
            &[],
        )
    }

    /// An RGBA8 texture of `size` texels (`layers` = 6 for a cube, `size.2`
    /// slices for 3D) from `texels`, laid out layer by layer, row by row.
    pub fn texture(
        &self,
        dim: wgpu::TextureViewDimension,
        size: [u32; 3],
        texels: &[[u8; 4]],
    ) -> wgpu::TextureView {
        use wgpu::util::TextureDataOrder;
        let dimension = if dim == wgpu::TextureViewDimension::D3 {
            wgpu::TextureDimension::D3
        } else {
            wgpu::TextureDimension::D2
        };
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            &bytes,
        );
        tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(dim),
            ..Default::default()
        })
    }

    /// [`Gpu::run`] with textures bound in group 1 as well.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_textures(
        &self,
        source: &str,
        entry: &str,
        uniform_binding: u32,
        uniform: &[u8],
        input: &[f32],
        output_floats: usize,
        invocations: u32,
        textures: &[GpuTexture],
    ) -> Vec<f32> {
        let d = &self.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("harness"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let entry_ty = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl0 = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[entry_ty(uniform_binding, wgpu::BufferBindingType::Uniform)],
        });
        let mut entries1 = vec![
            entry_ty(8, wgpu::BufferBindingType::Storage { read_only: true }),
            entry_ty(9, wgpu::BufferBindingType::Storage { read_only: false }),
        ];
        for t in textures {
            entries1.push(wgpu::BindGroupLayoutEntry {
                binding: t.binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: t.dim,
                    multisampled: false,
                },
                count: None,
            });
            entries1.push(wgpu::BindGroupLayoutEntry {
                binding: t.binding + 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let bgl1 = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries1,
        });
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl0), Some(&bgl1)],
            immediate_size: 0,
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });
        let ubuf = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let in_bytes: Vec<u8> = input.iter().flat_map(|f| f.to_le_bytes()).collect();
        let ibuf = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &in_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = (output_floats * 4) as u64;
        let obuf = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let rbuf = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let bg0 = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl0,
            entries: &[wgpu::BindGroupEntry {
                binding: uniform_binding,
                resource: ubuf.as_entire_binding(),
            }],
        });
        let mut bind1 = vec![
            wgpu::BindGroupEntry {
                binding: 8,
                resource: ibuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: obuf.as_entire_binding(),
            },
        ];
        for t in textures {
            bind1.push(wgpu::BindGroupEntry {
                binding: t.binding,
                resource: wgpu::BindingResource::TextureView(&t.view),
            });
            bind1.push(wgpu::BindGroupEntry {
                binding: t.binding + 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        let bg1 = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl1,
            entries: &bind1,
        });
        let mut enc = d.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg0, &[]);
            pass.set_bind_group(1, &bg1, &[]);
            pass.dispatch_workgroups(invocations, 1, 1);
        }
        enc.copy_buffer_to_buffer(&obuf, 0, &rbuf, 0, size);
        self.queue.submit([enc.finish()]);
        let slice = rbuf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        d.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = slice.get_mapped_range().expect("mapped range");
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    }
}

/// Relative/absolute closeness for comparing GPU and CPU floats.
pub fn close(a: f32, b: f32, tol: f32) -> bool {
    if a == b || (a.is_nan() && b.is_nan()) {
        return true;
    }
    // A non-finite value only matches itself (handled above): the relative
    // tolerance below would scale to infinity and accept anything.
    if !a.is_finite() || !b.is_finite() {
        return false;
    }
    (a - b).abs() <= tol * 1f32.max(a.abs()).max(b.abs())
}

/// One ILU boundary case: input `x` and the results NV_vertex_program
/// defines for `RCC`, `EXP` and `LOG` of it. The values come from the spec
/// text and IEEE field arithmetic, not from the interpreter.
pub struct IluCase {
    pub x: f32,
    pub rcc: f32,
    pub exp: [f32; 4],
    pub log: [f32; 4],
}

/// The boundary table shared by the CPU (`tests/vsh.rs`) and GPU
/// (`tests/gpu.rs`) tests.
pub fn ilu_boundary_cases() -> Vec<IluCase> {
    let inf = f32::INFINITY;
    let two64 = f32::from_bits(0x5F80_0000);
    let two_m64 = f32::from_bits(0x1F80_0000);
    let two_m70 = f32::from_bits(0x1C80_0000);
    let two_66 = f32::from_bits(0x6080_0000);
    let below = f32::from_bits(0x037F_FFFF);
    vec![
        // RCC(+0) = +2^64, RCC(-0) = -2^64; LOG(0) = (-inf, 1, -inf, 1).
        IluCase {
            x: 0.0,
            rcc: two64,
            exp: [1.0, 0.0, 1.0, 1.0],
            log: [-inf, 1.0, -inf, 1.0],
        },
        IluCase {
            x: -0.0,
            rcc: -two64,
            exp: [1.0, 0.0, 1.0, 1.0],
            log: [-inf, 1.0, -inf, 1.0],
        },
        // Saturation both ways: 1/2^-70 is past 2^64, 1/2^66 below 2^-64.
        IluCase {
            x: two_m70,
            rcc: two64,
            exp: [1.0, two_m70, 1.0, 1.0],
            log: [-70.0, 1.0, -70.0, 1.0],
        },
        IluCase {
            x: -two_66,
            rcc: -two_m64,
            exp: [0.0, 0.0, 0.0, 1.0],
            log: [66.0, 1.0, 66.0, 1.0],
        },
        // Exact values.
        IluCase {
            x: 4.0,
            rcc: 0.25,
            exp: [16.0, 0.0, 16.0, 1.0],
            log: [2.0, 1.0, 2.0, 1.0],
        },
        IluCase {
            x: 2.5,
            rcc: 0.4,
            exp: [4.0, 0.5, 5.656_854, 1.0],
            log: [1.0, 1.25, 1.321_928, 1.0],
        },
        IluCase {
            x: -1.5,
            rcc: -1.0 / 1.5,
            exp: [0.25, 0.5, 0.353_553_38, 1.0],
            log: [0.0, 1.5, 0.584_962_5, 1.0],
        },
        // Just below a power of two: bits 0x037fffff are (2 - 2^-23) * 2^-121,
        // so the exponent is -121 and the mantissa 1.99999988, although
        // log2 rounds to -120.
        IluCase {
            x: below,
            rcc: two64,
            exp: [1.0, below, 1.0, 1.0],
            log: [-121.0, f32::from_bits(0x3FFF_FFFF), -120.0, 1.0],
        },
        // Infinities: LOG(+-inf) = (inf, 1, inf, 1); EXP(+inf) = (inf, 0,
        // inf, 1) and EXP(-inf) = (0, 0, 0, 1); RCC saturates to 2^-64.
        IluCase {
            x: inf,
            rcc: two_m64,
            exp: [inf, 0.0, inf, 1.0],
            log: [inf, 1.0, inf, 1.0],
        },
        IluCase {
            x: -inf,
            rcc: -two_m64,
            exp: [0.0, 0.0, 0.0, 1.0],
            log: [inf, 1.0, inf, 1.0],
        },
    ]
}

/// The program the boundary tests run: `rcc oT0, v0.x`, `exp oT1, v0.x`,
/// `log oT2, v0.x`.
pub fn ilu_boundary_program() -> shaders::vsh::Program {
    use shaders::vsh::encode::{Inst, Operand, program};
    use shaders::vsh::{IluOp, OutReg};
    let x = Operand::v(0).swz("x");
    shaders::vsh::Program::parse(&program(&[
        Inst::ilu(IluOp::Rcc, x).to_out(true, OutReg::T(0), 15),
        Inst::ilu(IluOp::Exp, x).to_out(true, OutReg::T(1), 15),
        Inst::ilu(IluOp::Log, x).to_out(true, OutReg::T(2), 15),
    ]))
    .unwrap()
}
