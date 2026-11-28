//! GPU particle systems in the frame graph (docs/graphics/native-particles.md,
//! "The frame graph's particle stage"). The game publishes a [`Set`]
//! ([`crate::particle_set::publish`]); each game view of the frame gets
//! one stage in slot 9 ([`SLOT`], the transparents and effects), after the
//! view's last op before the sorted transparents `0x0001e370`: the last of
//! its carried slots up to `AfterLights`, which spans slots 5 to 9. The
//! stage mirrors each system's births (`GpuParticles::mirror`) and draws
//! it with `ParticleDraw::w_buffered` on the scene's colour and depth,
//! with the view's camera, inside its viewport, sampling the system's
//! bitmap tag image.
//!
//! A frame with no published set, no usable camera for a view, a view
//! with no op before its sorted transparents, or a depth that is not a
//! 24-bit W-buffer plans no stage and replays as before.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces,
    SceneStage,
};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{D24_SCALE, FrameScene, RenderCamera, Slot, TargetRef, ViewKind};
use gfx::D3DFormat;
use gfx::external::ExternalEncoded;
use wgpu::util::DeviceExt;

use crate::particle_compute::{Blend, DrawBinding, GpuParticles, ParticleCompute, ParticleDraw};
use crate::particle_set::{self, Set, SpriteBitmap};
use crate::particle_system::MAX_CAPACITY;


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

/// For each guest view with game-view ops, the index of its last op
/// before the sorted transparents (carried `Transparents`) and the view:
/// where its particle stage goes, in slot 9.
pub(super) fn effects_ops(frame: &ReplayFrame) -> Vec<(usize, u8)> {
    let mut seen = HashSet::new();
    let mut out: Vec<(usize, u8)> = frame
        .ops
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, (at, op))| {
            matches!(at.slot, Slot::Opaque | Slot::AfterOpaque | Slot::AfterLights)
                && !op.is_marker()
                && seen.insert(at.view)
        })
        .map(|(i, (at, _))| (i, at.view))
        .collect();
    out.reverse();
    out
}

/// The particle stage to apply after the op at each index of `frame`,
/// from the set the game last published. Empty when there is none.
pub(crate) fn plan(scene: &FrameScene, frame: &ReplayFrame) -> HashMap<usize, ReplayOp> {
    let Some(set) = particle_set::published() else {
        return HashMap::new();
    };
    stages(&set, scene, frame)
        .into_iter()
        .map(|(i, placed)| (i, placed.op()))
        .collect()
}

/// One view's stage before it is boxed: where it draws and what.
struct Placed {
    view: u8,
    rect: (u32, u32, u32, u32),
    surface: (u32, u32),
    stage: ParticleStage,
}

impl Placed {
    fn op(self) -> ReplayOp {
        let (left, top, width, height) = self.rect;
        let ranges = self.stage.set.systems.iter().filter(|s| !s.births.is_empty()).count();
        ReplayOp::Stage(SceneStage {
            surfaces: ExternalSurfaces {
                color: TargetRef::SceneColour,
                depth: TargetRef::SceneDepth,
                sampled: vec![],
                slots: vec![],
                slot_sizes: vec![],
            },
            request: ExternalPassRequest {
                rect: (left, top, width, height),
                surface_size: self.surface,
                display_projection_factor: 1.0,
                z_scale: D24_SCALE,
            },
            plan: ExternalPlan {
                stage: timeline::native::Stage::Effects,
                view: u32::from(self.view),
                ranges,
                slots: vec![],
            },
            pass: Arc::new(Mutex::new(Box::new(self.stage) as Box<dyn ExternalPass>)),
        })
    }
}

/// Each view's stage for `set`, by the index of the op it follows.
fn stages(set: &Arc<Set>, scene: &FrameScene, frame: &ReplayFrame) -> Vec<(usize, Placed)> {
    let mut out = Vec::new();
    if !set.draws() {
        return out;
    }
    let (Some(colour), Some(depth)) = (frame.colour, frame.depth) else {
        return out;
    };
    if depth.format != D3DFormat::D24S8 as u32 && depth.format != D3DFormat::LinD24S8 as u32 {
        return out;
    }
    for (i, view) in effects_ops(frame) {
        let Some(v) = scene
            .views
            .iter()
            .find(|v| v.key.kind == ViewKind::Game && v.key.view == view)
        else {
            continue;
        };
        let camera = v.camera;
        if !camera.is_usable() {
            continue;
        }
        let [top, left, bottom, right] = camera.viewport.map(|x| u32::try_from(x).unwrap_or(0));
        let (Some(width), Some(height)) = (
            right.checked_sub(left).filter(|&w| w > 0),
            bottom.checked_sub(top).filter(|&h| h > 0),
        ) else {
            continue;
        };
        out.push((
            i,
            Placed {
                view,
                rect: (left, top, width, height),
                surface: (colour.width, colour.height),
                stage: ParticleStage { set: Arc::clone(set), camera },
            },
        ));
    }
    out
}

/// One view's particle systems.
struct ParticleStage {
    set: Arc<Set>,
    camera: RenderCamera,
}

impl ExternalPass for ParticleStage {
    fn replays_scene(&self) -> bool {
        true
    }

    fn record_encoded(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        Some(self.encode_on(target, request))
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
    fn encode_on(
        &self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        let (gx, gy, gw, gh) = request.rect;
        let (sw, sh) = request.surface_size;
        if sw == 0 || sh == 0 {
            return Err("particle stage has an empty surface");
        }
        let (kx, ky) = (
            target.size.0 as f32 / sw as f32,
            target.size.1 as f32 / sh as f32,
        );
        let viewport = [gx as f32 * kx, gy as f32 * ky, gw as f32 * kx, gh as f32 * ky];
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
    use frame_scene::ViewSlot;
    /// The slot every system draws in: 9, with the transparents and effects.
    const SLOT: ViewSlot = ViewSlot::Transparents;
    use super::*;
    use crate::gpu::Image;
    use crate::particle_set::System;
    use crate::particle_system::Spawn;
    use frame_scene::{AuxTarget, SceneBuilder, SceneId, SlotAt, Stamp, ViewKey};
    use gfx::Clear;

    fn op(slot: Slot, view: u8) -> (SlotAt, ReplayOp) {
        (
            SlotAt { slot, view },
            ReplayOp::Clear {
                colour: TargetRef::SceneColour,
                depth: TargetRef::None,
                clear: Clear::NONE,
            },
        )
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

    /// A scene of game views 0 and 1.
    fn scene() -> FrameScene {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        for view in [0, 1] {
            b.open_view(ViewKey { kind: ViewKind::Game, player: view, view }, camera()).unwrap();
        }
        (*b.seal(Stamp::default()).unwrap()).clone()
    }

    /// Two views, each through its slots to the overlay, on a D24 W-buffer.
    fn frame() -> ReplayFrame {
        frame_on(D3DFormat::D24S8)
    }

    fn frame_on(depth: D3DFormat) -> ReplayFrame {
        let target = |format: D3DFormat, depth| AuxTarget { width: 640, height: 480, format: format as u32, depth };
        ReplayFrame {
            ops: vec![
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
            ],
            colour: Some(target(D3DFormat::A8R8G8B8, false)),
            depth: Some(target(depth, true)),
            ..Default::default()
        }
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
    fn a_published_system_draws_once_a_view_in_slot_9_with_its_sprite() {
        let sprite = bitmap(0xe5a0_0010);
        let set = Arc::new(Set { systems: vec![system(&sprite, 3)] });
        let (scene, frame) = (scene(), frame());
        let placed = stages(&set, &scene, &frame);
        let at: Vec<(usize, u8)> = placed.iter().map(|(i, p)| (*i, p.view)).collect();
        assert_eq!(at, [(2, 0), (7, 1)], "one stage a view, after its last op before the sorted list");
        for (i, p) in &placed {
            let (slot, next) = (frame.ops[*i].0.slot, frame.ops[i + 1].0);
            assert_eq!(ViewSlot::carried(slot).last(), Some(&SLOT), "the op it follows ends in slot 9");
            assert_eq!(ViewSlot::carried(next.slot).first(), Some(&SLOT), "the next starts in slot 9");
            assert_eq!(next.view, p.view);
            assert_eq!(SLOT.number(), Some(9));
            assert_eq!(p.rect, (0, 0, 640, 480));
            let systems = &p.stage.set.systems;
            assert_eq!(systems.len(), 1);
            assert!(Arc::ptr_eq(&systems[0].bitmap, &sprite), "the system's own bitmap, not a bound texture");
            assert_eq!(systems[0].bitmap.tag, 0xe5a0_0010);
        }
        let ops: HashMap<usize, ReplayOp> = placed.into_iter().map(|(i, p)| (i, p.op())).collect();
        for i in [2, 7] {
            let Some(ReplayOp::Stage(stage)) = ops.get(&i) else { panic!("no stage after op {i}") };
            assert_eq!(stage.plan.stage, timeline::native::Stage::Effects);
            assert_eq!(stage.plan.ranges, 1);
            assert_eq!((stage.surfaces.color, stage.surfaces.depth), (TargetRef::SceneColour, TargetRef::SceneDepth));
        }
    }

    #[test]
    fn the_stage_plans_each_blend_as_its_template_pass_sets_it() {
        use crate::particle_compute::Program;
        use wgpu::{BlendFactor as F, ColorWrites as W};
        let sprite = bitmap(2);
        let systems = Blend::ALL.into_iter().map(|blend| System { blend, ..system(&sprite, 1) }).collect();
        let placed = stages(&Arc::new(Set { systems }), &scene(), &frame());
        assert_eq!(placed.len(), 2);
        // (blend, SRCBLEND, DESTBLEND, COLORWRITEENABLE, program, alpha
        // test, the W-buffered entry), from each pass's implementation 0.
        let want = [
            (Blend::Alpha, F::SrcAlpha, F::OneMinusSrcAlpha, W::COLOR, Program::Plain, true, "fs_w_tested"),
            (Blend::Additive, F::SrcAlpha, F::One, W::COLOR, Program::Plain, false, "fs_w"),
            (Blend::MultiplyAdd, F::One, F::OneMinusSrcAlpha, W::COLOR, Program::ColourAlpha, true, "fs_w_colour_alpha_tested"),
            (Blend::Glass, F::One, F::OneMinusSrcAlpha, W::ALL, Program::OwnAlpha, false, "fs_w_own_alpha"),
        ];
        for (_, p) in &placed {
            let blends: Vec<Blend> = p.stage.set.systems.iter().map(|s| s.blend).collect();
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

    #[test]
    fn no_system_no_pass() {
        let (scene, frame) = (scene(), frame());
        assert!(stages(&Arc::new(Set::default()), &scene, &frame).is_empty(), "no system");
        let empty = Arc::new(Set { systems: vec![system(&bitmap(1), 0)] });
        assert!(stages(&empty, &scene, &frame).is_empty(), "a system with no birth");
        let set = Arc::new(Set { systems: vec![system(&bitmap(1), 1)] });
        assert!(stages(&set, &scene, &frame).len() == 2, "a birth: a stage a view");
        assert!(stages(&set, &scene, &frame_on(D3DFormat::D16)).is_empty(), "not a D24 W-buffer");
        let menu = ReplayFrame { ops: vec![op(Slot::Outside, 0), op(Slot::Overlay, 0)], ..frame };
        assert!(stages(&set, &scene, &menu).is_empty(), "no game view");
        assert!(effects_ops(&ReplayFrame::default()).is_empty());
    }
}
