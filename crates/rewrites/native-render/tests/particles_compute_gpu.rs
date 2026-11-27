//! GPU particle systems on a headless device (skips without an adapter;
//! docs/graphics/native-particles.md, "GPU particle systems"). An invented
//! system runs on the GPU and on the CPU reference evaluator
//! ([`reference`], test-only) for forty frames, more than a workgroup of
//! particles dying across blocks; every frame's buffer equals the
//! reference's in order within float tolerance. A dead particle is
//! compacted out, births past the capacity are dropped, and the draw's
//! instance count is the one the GPU wrote: a stale record beyond it is
//! not drawn.
#![cfg(feature = "gpu")]

#[path = "particles_compute_gpu/reference.rs"]
mod reference;

use native_render::particle_compute::{Blend, GpuParticles, ParticleCompute, ParticleDraw};
use native_render::particle_system::{
    ColourProperty, Counts, Definition, GRADIENT, Input, NO_INPUT, Particle, Property, Spawn, Sprite, View, args, flag,
    modifier, pack,
};

const DT: f32 = 1.0 / 60.0;

fn read(ctx: &gfx::GfxContext, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    ctx.queue.submit([enc.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    staging.slice(..).get_mapped_range().unwrap().to_vec()
}

fn counts(ctx: &gfx::GfxContext, s: &GpuParticles) -> Counts {
    *bytemuck::from_bytes(&read(ctx, s.counts(), size_of::<Counts>() as u64))
}

fn particles(ctx: &gfx::GfxContext, s: &GpuParticles, n: u32) -> Vec<Particle> {
    let bytes = read(ctx, s.particles(), u64::from(n.max(1)) * size_of::<Particle>() as u64);
    bytemuck::cast_slice(&bytes)[..n as usize].to_vec()
}

fn draw_args(ctx: &gfx::GfxContext, s: &GpuParticles) -> [u32; 4] {
    let bytes = read(ctx, s.indirect(), args::BYTES);
    let words: &[u32] = bytemuck::cast_slice(&bytes);
    let at = (args::DRAW / 4) as usize;
    [words[at], words[at + 1], words[at + 2], words[at + 3]]
}

fn step(ctx: &gfx::GfxContext, compute: &ParticleCompute, s: &mut GpuParticles) {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    s.update(compute, &ctx.queue, &mut enc, DT);
    ctx.queue.submit([enc.finish()]);
}

/// A small deterministic generator for the invented data.
struct Lcg(u64);
impl Lcg {
    fn u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
    fn unit(&mut self) -> f32 {
        (self.u32() >> 8) as f32 / (1u32 << 23) as f32
    }
}

fn curve(f: impl Fn(f32) -> f32) -> [f32; 16] {
    std::array::from_fn(|i| f(i as f32 / 15.0))
}

/// Every evaluation path: random sequence, frame blend, flips, the
/// particle's tint, spin, mapped and modified properties, a ranged colour.
fn invented() -> (Definition, Vec<Sprite>) {
    let sprites: Vec<Sprite> = (0..12)
        .map(|i| {
            let (u0, v0) = ((i % 4) as f32 * 0.25, (i / 4) as f32 * 0.25);
            let (w, h) = (0.25 - 0.01 * (i % 3) as f32, 0.2 + 0.015 * (i % 5) as f32);
            Sprite { window: [u0, u0 + w, v0, v0 + h], registration: [0.4 + 0.02 * i as f32, 0.55], _pad: [0.0; 2] }
        })
        .collect();
    let mut rotation = Property::curve(Input::Random0, curve(|x| x));
    rotation.range = Input::Random1 as u32;
    rotation.curve_hi = pack(curve(|x| 1.0 - 0.5 * x));
    rotation.map = 1;
    rotation.lo = -0.5;
    rotation.hi = 0.5;
    let mut alpha = Property::curve(Input::Life, curve(|x| 1.0 - x * x));
    alpha.modifier_input = Input::Random2 as u32;
    alpha.modifier = modifier::MULTIPLY;
    let mut size = Property::curve(Input::Life, curve(|x| 0.2 + 0.8 * x));
    size.modifier_input = Input::Random3 as u32;
    size.modifier = modifier::ADD;
    let frame = Property::curve(Input::Life, curve(|x| x));
    let colour = ColourProperty {
        input: Input::Life as u32,
        range: Input::Random0 as u32,
        modifier_input: NO_INPUT,
        modifier: modifier::NONE,
        curve_lo: pack(curve(|x| x)),
        curve_hi: pack(curve(|x| 1.0 - x)),
        gradient: std::array::from_fn(|i| {
            let t = i as f32 / (GRADIENT - 1) as f32;
            [1.0 - t, 0.3 + 0.5 * t, t * t, 0.0]
        }),
    };
    let definition = Definition {
        flags: flag::FLIP_U | flag::FLIP_V | flag::RANDOM_SEQUENCE | flag::PARTICLE_TINT | flag::SPIN,
        sequence_first: 0,
        sequence_pick: 3,
        frames: 4,
        sequences: 3,
        angle_scale: std::f32::consts::TAU,
        spin: 0.25,
        size_scale: 1.5,
        tint: [0.9, 1.0, 0.8, 0.6],
        gravity: [0.0, 0.0, -4.9],
        drag: 0.3,
        fixed_window: [0.0; 4],
        fixed_corners: [0.0; 4],
        rotation,
        alpha,
        size,
        frame,
        colour,
    };
    (definition, sprites)
}

/// Lifetimes half a step off a multiple of it, so death never hangs on
/// the last bit of the accumulated age.
fn spawns(rng: &mut Lcg, first: u32, n: u32) -> Vec<Spawn> {
    (first..first + n)
        .map(|id| Spawn {
            position: [rng.unit() * 8.0 - 4.0, rng.unit() * 8.0 - 4.0, rng.unit() * 2.0],
            lifetime: (3 + rng.u32() % 36) as f32 * DT + 0.5 * DT,
            velocity: [rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0, rng.unit() * 3.0],
            id,
            random: [rng.unit(), rng.unit(), rng.unit(), rng.unit()],
            words: rng.u32(),
            tint: rng.u32() & 0x00ff_ffff,
            _pad: [0; 2],
        })
        .collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3 + 1e-4 * a.abs().max(b.abs())
}

fn close4(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(&b).all(|(&x, &y)| close(x, y))
}

fn colour_close(a: u32, b: u32) -> bool {
    a.to_le_bytes().iter().zip(b.to_le_bytes()).all(|(&x, y)| x.abs_diff(y) <= 1)
}

#[test]
fn the_compute_update_equals_the_cpu_reference_over_forty_frames() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (definition, sprites) = invented();
    let compute = ParticleCompute::new(&ctx.device);
    let mut gpu = GpuParticles::new(&ctx.device, &compute, &definition, &sprites, 1024).unwrap();
    let mut cpu = reference::System { definition, sprites, capacity: 1024, particles: Vec::new() };
    let mut rng = Lcg(0x5eed);
    let (mut next_id, mut edges, mut compared, mut most) = (0, 0, 0, 0);
    let mut died = false;
    for frame in 0..40 {
        let n = if frame == 0 { 300 } else { 17 };
        let born = spawns(&mut rng, next_id, n);
        next_id += n;
        gpu.emit(&born);
        step(&ctx, &compute, &mut gpu);
        let before = cpu.particles.len();
        let near = cpu.step(&born, DT);
        died |= cpu.particles.len() < before + born.len();
        most = most.max(cpu.particles.len());

        let c = counts(&ctx, &gpu);
        assert_eq!(c.count as usize, cpu.particles.len(), "frame {frame}: live count");
        assert_eq!(draw_args(&ctx, &gpu), [6, c.count, 0, 0], "frame {frame}: the draw's arguments");
        let got = particles(&ctx, &gpu, c.count);
        for (i, (g, (want, edge))) in got.iter().zip(cpu.particles.iter().zip(&near)).enumerate() {
            let at = format!("frame {frame}, slot {i}, id {}", want.id);
            assert_eq!(g.id, want.id, "{at}: order");
            assert!(close(g.age, want.age), "{at}: age {} vs {}", g.age, want.age);
            for k in 0..3 {
                assert!(close(g.position[k], want.position[k]), "{at}: position {:?} vs {:?}", g.position, want.position);
                assert!(close(g.velocity[k], want.velocity[k]), "{at}: velocity {:?} vs {:?}", g.velocity, want.velocity);
            }
            assert!(colour_close(g.colour, want.colour), "{at}: colour {:08x} vs {:08x}", g.colour, want.colour);
            assert!(close(g.size, want.size), "{at}: size {} vs {}", g.size, want.size);
            assert!(close(g.rotation, want.rotation), "{at}: rotation {} vs {}", g.rotation, want.rotation);
            if *edge {
                edges += 1;
                continue;
            }
            assert!(close4(g.window, want.window), "{at}: window {:?} vs {:?}", g.window, want.window);
            assert!(close4(g.next_window, want.next_window), "{at}: next window {:?} vs {:?}", g.next_window, want.next_window);
            assert!(close4(g.corners, want.corners), "{at}: corners {:?} vs {:?}", g.corners, want.corners);
            assert!(close(g.blend, want.blend), "{at}: blend {} vs {}", g.blend, want.blend);
            compared += 1;
        }
    }
    assert!(died, "particles died along the way");
    assert!(most > 256, "more than one workgroup of particles: {most}");
    assert!(edges * 100 < compared, "frame edges skipped: {edges} of {compared}");
}

/// No bitmap, no curves: the definition's window and white, unit size.
fn plain() -> Definition {
    let (mut d, _) = invented();
    d.flags = flag::FIXED_WINDOW;
    d.fixed_window = [0.0, 1.0, 0.0, 1.0];
    d.fixed_corners = [-1.0, -1.0, 1.0, 1.0];
    d.frames = 0;
    d.sequences = 0;
    d.tint = [1.0; 4];
    d.gravity = [0.0; 3];
    d.drag = 0.0;
    d.rotation = Property::constant(0.0);
    d.alpha = Property::constant(1.0);
    d.size = Property::constant(1.0);
    d.size_scale = 0.25;
    d.colour.input = Input::Zero as u32;
    d.colour.range = Input::Zero as u32;
    d.colour.gradient = [[1.0, 0.0, 0.0, 0.0]; GRADIENT];
    d
}

fn at(position: [f32; 3], lifetime: f32, id: u32) -> Spawn {
    Spawn { position, lifetime, id, ..Default::default() }
}

#[test]
fn a_dead_particle_is_compacted_out_and_births_past_capacity_drop() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let compute = ParticleCompute::new(&ctx.device);
    let mut s = GpuParticles::new(&ctx.device, &compute, &plain(), &[], 8).unwrap();
    s.emit(&[at([0.0; 3], 0.5 * DT, 7), at([0.0; 3], 10.0, 8), at([0.0; 3], 10.0, 9)]);
    step(&ctx, &compute, &mut s);
    let ids = |s: &GpuParticles| particles(&ctx, s, counts(&ctx, s).count).iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(ids(&s), [7, 8, 9], "born in order, not aged on their first frame");
    step(&ctx, &compute, &mut s);
    assert_eq!(ids(&s), [8, 9], "7 outlived its half step and is gone");
    assert_eq!(counts(&ctx, &s), Counts { count: 2, old: 3, alive: 2, spawned: 0 });

    let burst: Vec<_> = (0..10).map(|i| at([0.0; 3], 10.0, 100 + i)).collect();
    s.emit(&burst);
    step(&ctx, &compute, &mut s);
    assert_eq!(ids(&s), [8, 9, 100, 101, 102, 103, 104, 105], "the survivors keep their places; six of ten fit");
    assert_eq!(draw_args(&ctx, &s), [6, 8, 0, 0]);
}

#[test]
fn the_draw_count_comes_from_the_gpu_buffer() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    // A row of 64 texels is the copy's 256-byte alignment.
    const SIZE: u32 = 64;
    let compute = ParticleCompute::new(&ctx.device);
    let mut s = GpuParticles::new(&ctx.device, &compute, &plain(), &[], 4).unwrap();
    // A stays; B, to its right, dies at the second step. After the third
    // the current buffer is the one the first scatter wrote, B still in
    // its second slot, one past the count.
    s.emit(&[at([0.0, 0.0, 0.5], 10.0, 1), at([0.75, 0.0, 0.5], 0.5 * DT, 2)]);
    for _ in 0..3 {
        step(&ctx, &compute, &mut s);
    }
    assert_eq!(counts(&ctx, &s).count, 1);
    assert_eq!(draw_args(&ctx, &s), [6, 1, 0, 0]);
    assert_eq!(particles(&ctx, &s, 2)[1].id, 2, "B's stale record is still in the buffer");

    let view = View {
        clip_from_world: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        right: [1.0, 0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0, 0.0],
    };
    use wgpu::util::DeviceExt;
    let view = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&view),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let texture = |size, usage| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        })
    };
    let white = texture(1, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
    ctx.queue.write_texture(
        white.as_image_copy(),
        &[255; 4],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let target = texture(SIZE, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let tv = target.create_view(&Default::default());
    let sampler = ctx.device.create_sampler(&Default::default());
    let draw = ParticleDraw::new(&ctx.device, wgpu::TextureFormat::Rgba8Unorm, None, Blend::Alpha);
    let binding = draw.bind(&ctx.device, &s, &view, &white.create_view(&Default::default()), &sampler);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tv,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        draw.draw(&mut pass, &s, &binding);
    }
    let out = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let px = out.slice(..).get_mapped_range().unwrap().to_vec();
    let pixel = |x: u32, y: u32| {
        let i = ((y * SIZE + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(pixel(SIZE / 2, SIZE / 2), [255, 0, 0, 255], "A drawn red at the centre");
    assert_eq!(pixel(1, 1), [0, 0, 0, 255], "clear outside A");
    assert_eq!(pixel(56, SIZE / 2), [0, 0, 0, 255], "B, past the GPU's count, not drawn");
}
