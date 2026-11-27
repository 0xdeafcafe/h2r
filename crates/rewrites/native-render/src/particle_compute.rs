//! GPU particle systems: emission, update, compaction and the draw, all
//! over GPU-resident buffers (docs/graphics/native-particles.md, "GPU
//! particle systems"). The records are [`crate::particle_system`]'s.
//!
//! A frame is [`GpuParticles::emit`] (births staged for the next update),
//! [`GpuParticles::update`] (three compute dispatches, the first and
//! last indirect) and [`ParticleDraw::draw`] (one `draw_indirect`). The
//! CPU never reads a count or writes a vertex: the update's last
//! workgroup writes the draw's instance count and the next update's
//! dispatch size. Survivors keep their order, births follow them, so the
//! draw order is stable from frame to frame.
//!
//! Particles are visual only. Nothing here touches the sim arena or its
//! hash; a system's state lives in its own buffers and dies with them.

use wgpu::util::DeviceExt;

use crate::particle_system::{
    Counts, Definition, MAX_CAPACITY, Particle, Spawn, Sprite, Step, WORKGROUP, args,
};

const COMMON: &str = include_str!("particle_common.wgsl");

fn module(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{body}").into()),
    })
}

/// The three update kernels, shared by every system on a device.
pub struct ParticleCompute {
    update: wgpu::ComputePipeline,
    offsets: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
}

impl ParticleCompute {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = module(device, "particle compute", include_str!("particle_compute.wgsl"));
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            update: pipeline("update"),
            offsets: pipeline("offsets"),
            scatter: pipeline("scatter"),
        }
    }
}

/// Why a system was not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Zero, or above [`MAX_CAPACITY`].
    Capacity,
    /// `frames * sequences` is more than the sprites given.
    Sprites,
}

/// One particle system's buffers. Two particle buffers swap each update:
/// the update reads one, the scatter writes the other, the draw reads it.
pub struct GpuParticles {
    capacity: u32,
    particles: [wgpu::Buffer; 2],
    state: wgpu::Buffer,
    args: wgpu::Buffer,
    spawns: wgpu::Buffer,
    definition: wgpu::Buffer,
    step: wgpu::Buffer,
    /// `[from]`: update and scatter reading `particles[from]`.
    update: [wgpu::BindGroup; 2],
    offsets: wgpu::BindGroup,
    scatter: [wgpu::BindGroup; 2],
    /// The buffer holding the current particles.
    current: usize,
    pending: Vec<Spawn>,
}

impl GpuParticles {
    pub fn new(
        device: &wgpu::Device,
        compute: &ParticleCompute,
        definition: &Definition,
        sprites: &[Sprite],
        capacity: u32,
    ) -> Result<Self, Refused> {
        if capacity == 0 || capacity > MAX_CAPACITY {
            return Err(Refused::Capacity);
        }
        if (definition.frames as usize) * (definition.sequences as usize) > sprites.len() {
            return Err(Refused::Sprites);
        }
        let storage = wgpu::BufferUsages::STORAGE;
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let bytes = u64::from(capacity) * size_of::<Particle>() as u64;
        let particles = [0, 1].map(|_| buffer("particles", bytes, storage | wgpu::BufferUsages::COPY_SRC));
        let scan = buffer("particle scan", u64::from(capacity) * 4, storage);
        let blocks = buffer("particle blocks", u64::from(WORKGROUP) * 4, storage);
        let state = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle counts"),
            contents: bytemuck::bytes_of(&Counts::default()),
            usage: storage | wgpu::BufferUsages::COPY_SRC,
        });
        // Nothing to update or draw until the first births.
        let mut initial = [0u32; 12];
        initial[1..3].copy_from_slice(&[1, 1]);
        initial[5..7].copy_from_slice(&[1, 1]);
        initial[8] = 6;
        let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle indirect"),
            contents: bytemuck::cast_slice(&initial),
            usage: storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_SRC,
        });
        let spawns = buffer(
            "particle spawns",
            u64::from(capacity) * size_of::<Spawn>() as u64,
            storage | wgpu::BufferUsages::COPY_DST,
        );
        let definition_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle definition"),
            contents: bytemuck::bytes_of(definition),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // A binding is never empty.
        let placeholder = [Sprite { window: [0.0, 1.0, 0.0, 1.0], registration: [0.5; 2], _pad: [0.0; 2] }];
        let sprites = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle sprites"),
            contents: bytemuck::cast_slice(if sprites.is_empty() { &placeholder } else { sprites }),
            usage: storage,
        });
        let step = buffer(
            "particle step",
            size_of::<Step>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        let group = |pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|&(binding, b)| wgpu::BindGroupEntry { binding, resource: b.as_entire_binding() })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particle update"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            })
        };
        let update = [0, 1].map(|from: usize| {
            group(
                &compute.update,
                &[
                    (0, &particles[from]),
                    (2, &scan),
                    (3, &blocks),
                    (4, &state),
                    (7, &definition_buffer),
                    (8, &sprites),
                    (9, &step),
                ],
            )
        });
        let offsets = group(&compute.offsets, &[(3, &blocks), (4, &state), (5, &args), (9, &step)]);
        let scatter = [0, 1].map(|from: usize| {
            group(
                &compute.scatter,
                &[
                    (0, &particles[from]),
                    (1, &particles[1 - from]),
                    (2, &scan),
                    (3, &blocks),
                    (4, &state),
                    (6, &spawns),
                    (7, &definition_buffer),
                    (8, &sprites),
                ],
            )
        });
        Ok(Self {
            capacity,
            particles,
            state,
            args,
            spawns,
            definition: definition_buffer,
            step,
            update,
            offsets,
            scatter,
            current: 0,
            pending: Vec::new(),
        })
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Stage births for the next [`Self::update`]. Births beyond the
    /// capacity, or beyond the room the survivors leave, are dropped there.
    pub fn emit(&mut self, spawns: &[Spawn]) {
        let room = self.capacity as usize - self.pending.len().min(self.capacity as usize);
        self.pending.extend_from_slice(&spawns[..spawns.len().min(room)]);
    }

    /// Replace the definition from the next update on.
    pub fn set_definition(&self, queue: &wgpu::Queue, definition: &Definition) {
        queue.write_buffer(&self.definition, 0, bytemuck::bytes_of(definition));
    }

    /// One step of `dt` seconds: age, kill, integrate and evaluate, compact,
    /// then the staged births. The step and births go through the queue,
    /// so one update per submission: a second would overwrite them.
    pub fn update(&mut self, compute: &ParticleCompute, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, dt: f32) {
        self.record(compute, queue, encoder, dt, 0);
    }

    /// The staged births become the whole system: no particle survives,
    /// and each birth is evaluated at its own [`Spawn::age`], not aged. A
    /// mirror of particles another simulation owns (the guest's), drawn
    /// as they are this frame (docs/graphics/retire-d3d-r2.md, "R2's
    /// particle mirror"). One per submission, as [`Self::update`].
    pub fn mirror(&mut self, compute: &ParticleCompute, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        self.record(compute, queue, encoder, 0.0, 1);
    }

    fn record(&mut self, compute: &ParticleCompute, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, dt: f32, mirror: u32) {
        let step = Step { dt, spawns: self.pending.len() as u32, capacity: self.capacity, mirror };
        queue.write_buffer(&self.step, 0, bytemuck::bytes_of(&step));
        if !self.pending.is_empty() {
            queue.write_buffer(&self.spawns, 0, bytemuck::cast_slice(&self.pending));
            self.pending.clear();
        }
        let from = self.current;
        // One pass: each dispatch is its own usage scope, so the scatter's
        // indirect read follows the offsets kernel's write.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("particle update"), timestamp_writes: None });
            pass.set_pipeline(&compute.update);
            pass.set_bind_group(0, &self.update[from], &[]);
            pass.dispatch_workgroups_indirect(&self.args, args::UPDATE);
            pass.set_pipeline(&compute.offsets);
            pass.set_bind_group(0, &self.offsets, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&compute.scatter);
            pass.set_bind_group(0, &self.scatter[from], &[]);
            pass.dispatch_workgroups_indirect(&self.args, args::SCATTER);
        }
        self.current = 1 - from;
    }

    /// The current particles, `capacity` records of which the first
    /// [`Counts::count`] are live.
    pub fn particles(&self) -> &wgpu::Buffer {
        &self.particles[self.current]
    }

    /// The counters ([`Counts`]).
    pub fn counts(&self) -> &wgpu::Buffer {
        &self.state
    }

    /// The indirect arguments; the draw's are at [`args::DRAW`].
    pub fn indirect(&self) -> &wgpu::Buffer {
        &self.args
    }

    fn current(&self) -> usize {
        self.current
    }
}

/// How a system's particles blend into the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Alpha,
    Additive,
}

/// The draw pipeline for one target format and blend.
pub struct ParticleDraw {
    pipeline: wgpu::RenderPipeline,
}

/// A system's draw bindings: its two particle buffers, the view and the
/// sprite texture.
pub struct DrawBinding {
    groups: [wgpu::BindGroup; 2],
}

impl ParticleDraw {
    /// `depth`: the pass's depth format, tested (less-equal), never written.
    pub fn new(device: &wgpu::Device, color: wgpu::TextureFormat, depth: Option<wgpu::TextureFormat>, blend: Blend) -> Self {
        Self::with_fragment(device, color, depth, blend, "fs")
    }

    /// The draw into a guest W-buffer of `depth`'s format: the depth each
    /// fragment tests (less-equal, never written) is its W-buffer value
    /// from the view's `w_row` and `depth` ([`crate::particle_system::View`]),
    /// not the clip's z (docs/graphics/retire-d3d-r2.md, "R2's particle
    /// mirror").
    pub fn w_buffered(device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, blend: Blend) -> Self {
        Self::with_fragment(device, color, Some(depth), blend, "fs_w")
    }

    fn with_fragment(
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: Option<wgpu::TextureFormat>,
        blend: Blend,
        fragment: &str,
    ) -> Self {
        let module = module(device, "particle draw", include_str!("particle_draw.wgsl"));
        let blend = match blend {
            Blend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
            Blend::Additive => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particle draw"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: depth.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline }
    }

    /// Bind `system` for drawing with `view` (a [`crate::particle_system::View`]
    /// uniform) and its sprite texture.
    pub fn bind(
        &self,
        device: &wgpu::Device,
        system: &GpuParticles,
        view: &wgpu::Buffer,
        texture: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> DrawBinding {
        let layout = self.pipeline.get_bind_group_layout(0);
        let groups = [0, 1].map(|i: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particle draw"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: system.particles[i].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: view.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(texture) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        });
        DrawBinding { groups }
    }

    /// The draw entry: the system's current particles into the open pass,
    /// one `draw_indirect` whose instance count the last update wrote.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, system: &GpuParticles, binding: &DrawBinding) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &binding.groups[system.current()], &[]);
        pass.draw_indirect(system.indirect(), args::DRAW);
    }
}
