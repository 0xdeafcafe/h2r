//! GPU particle systems in the frame graph (docs/graphics/native-particles.md,
//! "The frame graph's particle stage"). The game publishes a [`Set`]
//! ([`crate::particle_set::publish`]); each game view of the frame gets
//! one stage in slot 9 (the transparents and effects), after the view's
//! last stage before the sorted transparents `0x0001e370`: the last of its
//! slots up to `AfterLights`, which spans slots 5 to 9. The
//! stage mirrors each system's births (`GpuParticles::mirror`) and draws
//! it with `ParticleDraw::w_buffered` on the scene's colour and depth,
//! with the view's camera, inside its viewport, sampling the system's
//! bitmap tag image.

use std::sync::{Arc, Mutex};

use frame_scene::RenderCamera;
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalEncoded};
use wgpu::util::DeviceExt;

use crate::particle_compute::{Blend, DrawBinding, GpuParticles, ParticleCompute, ParticleDraw};
use crate::particle_set::{self, Set, SpriteBitmap};
use crate::particle_system::MAX_CAPACITY;
use crate::program::ViewInputs;
use crate::stage::{FrameOp, Stage, StageWork};

use super::{Cameras, guest_rect, last_stages, nothing, texel_viewport};


/// One device's kernels, pipelines and sprite textures, kept across
/// frames. A bitmap is held here while the set names it, so a pointer
/// match is never a different bitmap.
struct Held {
    device: wgpu::Device,
    compute: Arc<ParticleCompute>,
    sampler: wgpu::Sampler,
    draws: Vec<(wgpu::TextureFormat, wgpu::TextureFormat, Blend, Arc<ParticleDraw>)>,
    sprites: Vec<(Arc<SpriteBitmap>, wgpu::TextureView)>,
}

static DEVICES: Mutex<Vec<Arc<Mutex<Held>>>> = Mutex::new(Vec::new());

fn held_for(device: &wgpu::Device) -> Arc<Mutex<Held>> {
    let mut all = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    for slot in all.iter() {
        if slot.lock().unwrap_or_else(|e| e.into_inner()).device == *device {
            return Arc::clone(slot);
        }
    }
    let made = Arc::new(Mutex::new(Held {
        device: device.clone(),
        compute: Arc::new(ParticleCompute::new(device)),
        sampler: device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("particle sprites"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        }),
        draws: Vec::new(),
        sprites: Vec::new(),
    }));
    all.push(Arc::clone(&made));
    made
}

impl Held {
    fn draw(&mut self, colour: wgpu::TextureFormat, depth: wgpu::TextureFormat, blend: Blend) -> Arc<ParticleDraw> {
        if let Some((.., d)) = self.draws.iter().find(|(c, z, b, _)| *c == colour && *z == depth && *b == blend) {
            return Arc::clone(d);
        }
        let made = Arc::new(ParticleDraw::w_buffered(&self.device, colour, depth, blend));
        self.draws.push((colour, depth, blend, Arc::clone(&made)));
        made
    }

    /// The bitmap's texture, uploaded the first time.
    fn sprite(&mut self, queue: &wgpu::Queue, bitmap: &Arc<SpriteBitmap>) -> wgpu::TextureView {
        if let Some((_, view)) = self.sprites.iter().find(|(b, _)| Arc::ptr_eq(b, bitmap)) {
            return view.clone();
        }
        let view = crate::gpu::texture(&self.device, queue, &bitmap.image, "particle sprite");
        self.sprites.push((Arc::clone(bitmap), view.clone()));
        view
    }
}

/// The slots before a view's sorted transparents: its particle stage
/// follows the last stage in them.
fn before_transparents(slot: Slot) -> bool {
    matches!(slot, Slot::Opaque | Slot::AfterOpaque | Slot::AfterLights)
}

/// The particle stage of each game view, by the index of the op it
/// follows, from the set the game last published. Empty when there is none.
pub(crate) fn plan(ops: &[FrameOp], cameras: &Cameras) -> Vec<(usize, SlotAt, Box<dyn Stage>)> {
    let Some(set) = particle_set::published() else {
        return Vec::new();
    };
    stages(&set, ops, cameras)
        .into_iter()
        .map(|(i, view, stage)| (i, SlotAt { slot: Slot::AfterLights, view }, Box::new(stage) as Box<dyn Stage>))
        .collect()
}

/// Each view's stage for `set`, by the index of the op it follows.
fn stages(set: &Arc<Set>, ops: &[FrameOp], cameras: &Cameras) -> Vec<(usize, u8, ParticleStage)> {
    if !set.draws() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, view) in last_stages(ops, before_transparents) {
        let Some(camera) = cameras.get(view) else { continue };
        let Some(rect) = guest_rect(&camera) else { continue };
        out.push((i, view, ParticleStage { set: Arc::clone(set), camera, rect }));
    }
    out
}

/// One view's particle systems.
struct ParticleStage {
    set: Arc<Set>,
    camera: RenderCamera,
    rect: (u32, u32, u32, u32),
}

impl Stage for ParticleStage {
    fn name(&self) -> &'static str {
        "particles"
    }

    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        StageWork::Encoded(self.encode_on(t).unwrap_or_else(|_| nothing()))
    }
}

/// One system ready to draw: its buffers with the births staged, its
/// pipeline and bindings.
struct Drawn {
    particles: GpuParticles,
    draw: Arc<ParticleDraw>,
    binding: DrawBinding,
}

impl ParticleStage {
    fn encode_on(&self, target: &ExternalAttachments) -> Result<ExternalEncoded, &'static str> {
        let viewport = texel_viewport(self.rect, target.size);
        let device = &target.device;
        let view = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle view"),
            contents: particle_set::view(&self.camera).as_bytes(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let slot = held_for(device);
        let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
        held.sprites
            .retain(|(b, _)| self.set.systems.iter().any(|s| Arc::ptr_eq(&s.bitmap, b)));
        let compute = Arc::clone(&held.compute);
        let mut drawn = Vec::new();
        // A system per view and recording: its births go through the
        // queue, so two never share buffers in one submission.
        for system in &self.set.systems {
            if system.births.is_empty() {
                continue;
            }
            let capacity = system.births.len().min(MAX_CAPACITY as usize) as u32;
            let Ok(mut particles) = GpuParticles::new(device, &compute, &system.definition, &system.sprites, capacity)
            else {
                continue;
            };
            particles.emit(&system.births);
            let draw = held.draw(target.color_format, target.depth_format, system.blend);
            let texture = held.sprite(&target.queue, &system.bitmap);
            let binding = draw.bind(device, &particles, &view, &texture, &held.sampler);
            drawn.push(Drawn { particles, draw, binding });
        }
        drop(held);
        if drawn.is_empty() {
            return Err("particle stage has no system it can draw");
        }
        let ranges = drawn.len();
        let drawn = Mutex::new(drawn);
        let queue = target.queue.clone();
        let (colour, depth) = (target.color.clone(), target.depth.clone());
        let stencil = target.depth_format.has_stencil_aspect();
        Ok(ExternalEncoded {
            ranges,
            draws: ranges,
            passes: 1,
            commands: Box::new(move |encoder| {
                let mut drawn = drawn.lock().unwrap_or_else(|e| e.into_inner());
                for d in drawn.iter_mut() {
                    d.particles.mirror(&compute, &queue, encoder);
                }
                fn kept<V>() -> wgpu::Operations<V> {
                    wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }
                }
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("particles"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &colour,
                        depth_slice: None,
                        resolve_target: None,
                        ops: kept(),
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(kept()),
                        stencil_ops: stencil.then(kept::<u32>),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let [x, y, w, h] = viewport;
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                for d in drawn.iter() {
                    d.draw.draw(&mut pass, &d.particles, &d.binding);
                }
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::Image;
    use crate::particle_set::System;
    use crate::particle_system::Spawn;
    use frame_scene::{SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};

    struct Named;
    impl Stage for Named {
        fn name(&self) -> &'static str {
            "named"
        }
        fn record(&mut self, _: &ExternalAttachments, _: &ViewInputs) -> StageWork {
            unimplemented!("test: record")
        }
    }

    fn op(slot: Slot, view: u8) -> FrameOp {
        FrameOp::Stage { slot: SlotAt { slot, view }, stage: Box::new(Named) }
    }

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 70f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        }
    }

    /// The cameras of game views 0 and 1.
    fn cameras() -> Cameras {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        for view in [0, 1] {
            b.open_view(ViewKey { kind: ViewKind::Game, player: view, view }, camera()).unwrap();
        }
        Cameras::of(&b.seal(Stamp::default()).unwrap(), None)
    }

    /// Two views, each through its slots to the overlay.
    fn ops() -> Vec<FrameOp> {
        vec![
            op(Slot::Outside, 0),
            op(Slot::Opaque, 0),
            op(Slot::AfterLights, 0),
            op(Slot::Transparents, 0),
            op(Slot::AfterPost, 0),
            op(Slot::Opaque, 1),
            op(Slot::AfterOpaque, 1),
            op(Slot::AfterLights, 1),
            op(Slot::Transparents, 1),
            op(Slot::Overlay, 1),
        ]
    }

    fn system(bitmap: &Arc<SpriteBitmap>, births: usize) -> System {
        System {
            bitmap: Arc::clone(bitmap),
            definition: bytemuck::Zeroable::zeroed(),
            sprites: vec![],
            blend: Blend::Alpha,
            births: vec![Spawn { position: [8.0, 0.0, 0.0], lifetime: 1.0, ..Default::default() }; births],
        }
    }

    fn bitmap(tag: u32) -> Arc<SpriteBitmap> {
        Arc::new(SpriteBitmap { tag, image: Image::solid([255, 128, 0, 255]) })
    }

    #[test]
    fn a_published_system_draws_once_a_view_before_its_sorted_transparents() {
        let sprite = bitmap(0xe5a0_0010);
        let set = Arc::new(Set { systems: vec![system(&sprite, 3)] });
        let placed = stages(&set, &ops(), &cameras());
        let at: Vec<(usize, u8)> = placed.iter().map(|(i, v, _)| (*i, *v)).collect();
        assert_eq!(at, [(2, 0), (7, 1)]);
        for (_, _, p) in &placed {
            assert_eq!(p.rect, (0, 0, 640, 480));
            assert!(Arc::ptr_eq(&p.set.systems[0].bitmap, &sprite), "the system's own bitmap");
        }
    }

    #[test]
    fn no_system_no_stage() {
        let (ops, cameras) = (ops(), cameras());
        assert!(stages(&Arc::new(Set::default()), &ops, &cameras).is_empty(), "no system");
        let empty = Arc::new(Set { systems: vec![system(&bitmap(1), 0)] });
        assert!(stages(&empty, &ops, &cameras).is_empty(), "a system with no birth");
        let set = Arc::new(Set { systems: vec![system(&bitmap(1), 1)] });
        let menu = vec![op(Slot::Outside, 0), op(Slot::Overlay, 0)];
        assert!(stages(&set, &menu, &cameras).is_empty(), "no game view");
    }

    #[test]
    fn the_stage_plans_each_blend_as_its_template_pass_sets_it() {
        use crate::particle_compute::Program;
        use wgpu::{BlendFactor as F, ColorWrites as W};
        let sprite = bitmap(2);
        let systems = Blend::ALL.into_iter().map(|blend| System { blend, ..system(&sprite, 1) }).collect();
        let placed = stages(&Arc::new(Set { systems }), &ops(), &cameras());
        assert_eq!(placed.len(), 2);
        // (blend, SRCBLEND, DESTBLEND, COLORWRITEENABLE, program, alpha
        // test, the W-buffered entry), from each pass's implementation 0.
        let want = [
            (Blend::Alpha, F::SrcAlpha, F::OneMinusSrcAlpha, W::COLOR, Program::Plain, true, "fs_w_tested"),
            (Blend::Additive, F::SrcAlpha, F::One, W::COLOR, Program::Plain, false, "fs_w"),
            (Blend::MultiplyAdd, F::One, F::OneMinusSrcAlpha, W::COLOR, Program::ColourAlpha, true, "fs_w_colour_alpha_tested"),
            (Blend::Glass, F::One, F::OneMinusSrcAlpha, W::ALL, Program::OwnAlpha, false, "fs_w_own_alpha"),
        ];
        for (_, _, p) in &placed {
            let blends: Vec<Blend> = p.set.systems.iter().map(|s| s.blend).collect();
            assert_eq!(blends, Blend::ALL, "each system keeps its blend");
        }
        for (blend, src, dst, mask, program, test, entry) in want {
            let plan = blend.plan();
            for c in [plan.blend.color, plan.blend.alpha] {
                assert_eq!((c.src_factor, c.dst_factor, c.operation), (src, dst, wgpu::BlendOperation::Add), "{blend:?}");
            }
            assert_eq!((plan.write_mask, plan.program, plan.alpha_test), (mask, program, test), "{blend:?}");
            assert_eq!(plan.fragments[1], entry, "{blend:?}: the stage's draw is W-buffered");
            assert_eq!(plan.fragments[0], entry.replacen("fs_w", "fs", 1), "{blend:?}");
        }
    }
}
