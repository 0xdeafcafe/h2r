//! The frame graph (docs/graphics/native-renderer.md, "How a frame
//! flows"): walks a [`Frame`]'s ops in order on [`Targets`]. A bind sets
//! the attachments, a clear and a copy are their own commands, and each
//! [`Stage`] records on the bound attachments with its view's inputs. The
//! graph adds its own stages after each game view's last op: the GPU
//! particles ([`particles`]), the Forge monitors' bodies ([`bodies`]) and
//! the marker skin ([`markers`]).
//!
//! A display frame between ticks ([`display_rate`]) is the newer frame
//! drawn again with each view's camera blended from the frame before.

pub mod display_rate;
mod bodies;
mod markers;
mod particles;
pub mod targets;

use std::collections::HashMap;

use frame_scene::{D24_SCALE, FrameScene, RenderCamera, SceneId, ViewKind, summary_hash};
use frame_types::{Slot, SlotAt, TargetRef};
use gfx::Clear;
use gfx::external::{ExternalAttachments, ExternalDraws, TargetCopy};

use crate::lighting::Lighting;
use crate::program::ViewInputs;
use crate::stage::{Frame, FrameOp, Stage, StageWork};
pub use targets::{GUEST, Target, Targets};

/// What a draw of a frame submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameReport {
    pub id: SceneId,
    pub lighting: Lighting,
    /// Stages recorded, the graph's own included.
    pub stages: u32,
    pub draws: u64,
    pub render_passes: u64,
    pub submits: u64,
}

/// Pixels of a private draw, with the scene they came from.
pub struct CapturedFrame {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
    pub id: SceneId,
    pub scene_hash: u64,
    pub report: FrameReport,
}

/// Each game view's camera for one draw: the scene's own, or blended from
/// the scene before for a display frame.
pub struct Cameras {
    views: Vec<(u8, RenderCamera)>,
}

impl Cameras {
    /// `b`'s cameras, blended from `a` by `alpha` when given.
    pub fn of(b: &FrameScene, from: Option<(&FrameScene, f32)>) -> Self {
        let blended = from.map(|(a, alpha)| frame_scene::blend::blend(a, b, alpha));
        let views = b
            .views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.key.kind == ViewKind::Game)
            .map(|(i, v)| {
                let camera = blended.as_ref().and_then(|bl| bl.cameras.get(i).copied()).unwrap_or(v.camera);
                (v.key.view, camera)
            })
            .collect();
        Self { views }
    }

    pub fn get(&self, view: u8) -> Option<RenderCamera> {
        self.views.iter().find(|(v, _)| *v == view).map(|(_, c)| *c)
    }

    /// Every game view's camera as drawn, in the scene's order.
    pub fn drawn(&self) -> Vec<RenderCamera> {
        self.views.iter().map(|(_, c)| *c).collect()
    }
}

/// A game view's viewport in the game's pixels, `(x, y, width, height)`,
/// for a usable camera.
pub(crate) fn guest_rect(camera: &RenderCamera) -> Option<(u32, u32, u32, u32)> {
    if !camera.is_usable() {
        return None;
    }
    let [top, left, bottom, right] = camera.viewport.map(|x| u32::try_from(x).unwrap_or(0));
    let width = right.checked_sub(left).filter(|&w| w > 0)?;
    let height = bottom.checked_sub(top).filter(|&h| h > 0)?;
    Some((left, top, width, height))
}

/// `rect` in the game's pixels as a viewport on a target of `size`.
pub(crate) fn texel_viewport(rect: (u32, u32, u32, u32), size: (u32, u32)) -> [f32; 4] {
    let (kx, ky) = (size.0 as f32 / GUEST.0 as f32, size.1 as f32 / GUEST.1 as f32);
    let (x, y, w, h) = rect;
    [x as f32 * kx, y as f32 * ky, w as f32 * kx, h as f32 * ky]
}

/// For each game view with stages, the index of its last stage whose slot
/// passes `keep`: where a graph stage of that view goes.
pub(crate) fn last_stages(ops: &[FrameOp], keep: impl Fn(Slot) -> bool) -> Vec<(usize, u8)> {
    let mut out: Vec<(usize, u8)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let FrameOp::Stage { slot, .. } = op else { continue };
        if !keep(slot.slot) {
            continue;
        }
        match out.iter_mut().find(|(_, v)| *v == slot.view) {
            Some(last) => last.0 = i,
            None => out.push((i, slot.view)),
        }
    }
    out
}

/// The graph's own stages, by the index of the op they follow, in the
/// order they draw there.
fn graph_stages(ops: &[FrameOp], cameras: &Cameras) -> HashMap<usize, Vec<(SlotAt, Box<dyn Stage>)>> {
    let mut after: HashMap<usize, Vec<(SlotAt, Box<dyn Stage>)>> = HashMap::new();
    let mut add = |stages: Vec<(usize, SlotAt, Box<dyn Stage>)>| {
        for (i, at, stage) in stages {
            after.entry(i).or_default().push((at, stage));
        }
    };
    add(particles::plan(ops, cameras));
    add(bodies::plan(ops, cameras));
    add(markers::plan(ops, cameras));
    after
}

/// The work of a graph stage with nothing to draw this frame.
pub(crate) fn nothing() -> gfx::external::ExternalEncoded {
    gfx::external::ExternalEncoded { ranges: 0, draws: 0, passes: 0, commands: Box::new(|_| {}) }
}

/// The device and arena one draw records with.
pub(crate) struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub uploads: &'a gfx::ring::UploadPool,
}

/// One draw of a frame: its encoder, upload arena and the persistent
/// ranges written ahead of it, submitted whole or split where two stages
/// write one range with different bytes.
struct Walk<'a, 'g> {
    gpu: &'a Gpu<'g>,
    targets: &'a mut Targets,
    lighting: Lighting,
    encoder: wgpu::CommandEncoder,
    arena: gfx::ring::SubmissionUploads,
    written: HashMap<(wgpu::Buffer, u64), Vec<u8>>,
    bound: (TargetRef, TargetRef),
    report: FrameReport,
}

impl<'a, 'g> Walk<'a, 'g> {
    fn new(gpu: &'a Gpu<'g>, targets: &'a mut Targets, lighting: Lighting, id: SceneId) -> Self {
        Self {
            gpu,
            targets,
            lighting,
            encoder: gpu.device.create_command_encoder(&Default::default()),
            arena: gpu.uploads.begin(),
            written: HashMap::new(),
            bound: (TargetRef::SceneColour, TargetRef::SceneDepth),
            report: FrameReport { id, lighting, stages: 0, draws: 0, render_passes: 0, submits: 0 },
        }
    }

    fn target(&mut self, r: TargetRef) -> Option<Target> {
        self.targets.get(self.gpu.device, r)
    }

    fn submit(&mut self) {
        let encoder = std::mem::replace(&mut self.encoder, self.gpu.device.create_command_encoder(&Default::default()));
        let uploads = self.arena.upload_commands(self.gpu.queue);
        self.gpu.queue.submit(uploads.into_iter().chain([encoder.finish()]));
        self.arena = self.gpu.uploads.begin();
        self.written.clear();
        self.report.submits += 1;
    }

    fn op(&mut self, op: &mut FrameOp, view: &dyn Fn(SlotAt, (u32, u32)) -> ViewInputs) {
        match op {
            FrameOp::Bind { colour, depth } => self.bound = (*colour, *depth),
            FrameOp::Clear { colour, depth, clear } => self.clear(*colour, *depth, *clear),
            FrameOp::Copy { from, to } => self.copy(*from, *to),
            FrameOp::Stage { slot, stage } => self.stage(*slot, &mut **stage, view),
        }
    }

    fn clear(&mut self, colour: TargetRef, depth: TargetRef, clear: Clear) {
        let (c, d) = (self.target(colour), self.target(depth));
        if c.is_none() && d.is_none() {
            return;
        }
        let colour_ops = wgpu::Operations {
            load: clear.color.map_or(wgpu::LoadOp::Load, |[r, g, b, a]| {
                wgpu::LoadOp::Clear(wgpu::Color { r: r.into(), g: g.into(), b: b.into(), a: a.into() })
            }),
            store: wgpu::StoreOp::Store,
        };
        let depth_ops = wgpu::Operations {
            load: clear.depth.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
            store: wgpu::StoreOp::Store,
        };
        let stencil_ops = wgpu::Operations {
            load: clear.stencil.map_or(wgpu::LoadOp::Load, |s| wgpu::LoadOp::Clear(u32::from(s))),
            store: wgpu::StoreOp::Store,
        };
        let _ = self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame clear"),
            color_attachments: &[c.as_ref().map(|c| wgpu::RenderPassColorAttachment {
                view: &c.view,
                depth_slice: None,
                resolve_target: None,
                ops: colour_ops,
            })],
            depth_stencil_attachment: d.as_ref().map(|d| wgpu::RenderPassDepthStencilAttachment {
                view: &d.view,
                depth_ops: Some(depth_ops),
                stencil_ops: d.format.has_stencil_aspect().then_some(stencil_ops),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.report.render_passes += 1;
    }

    fn copy(&mut self, from: TargetRef, to: TargetRef) {
        let (Some(from), Some(to)) = (self.target(from), self.target(to)) else {
            return;
        };
        let size = wgpu::Extent3d {
            width: from.size.0.min(to.size.0),
            height: from.size.1.min(to.size.1),
            depth_or_array_layers: 1,
        };
        self.encoder.copy_texture_to_texture(from.texture.as_image_copy(), to.texture.as_image_copy(), size);
    }

    fn stage(&mut self, slot: SlotAt, stage: &mut dyn Stage, view: &dyn Fn(SlotAt, (u32, u32)) -> ViewInputs) {
        if stage.lighting_only().is_some_and(|m| m != self.lighting) {
            return;
        }
        let (colour, depth) = self.bound;
        let Some(c) = self.target(colour) else {
            unimplemented!("frame graph: stage {} with no colour bound", stage.name())
        };
        // A colour-only bind (the overlay, the ocean's reflection) gives
        // its stages a depth of the colour's size: the scene's for the
        // scene's colour.
        let d = match depth {
            TargetRef::None => self.targets.unbound_depth(self.gpu.device, c.size),
            depth => self.target(depth).unwrap_or_else(|| unimplemented!("frame graph: stage {} with no depth", stage.name())),
        };
        let sampled = self.sampled(&*stage, (colour, depth));
        let attachments = ExternalAttachments {
            targets: (gfx::surface::SurfaceId(0), gfx::surface::SurfaceId(1)),
            uploads: self.arena.clone(),
            device: self.gpu.device.clone(),
            queue: self.gpu.queue.clone(),
            color: c.view.clone(),
            color_format: c.format,
            depth: d.view.clone(),
            depth_format: d.format,
            size: c.size,
            sampled,
            slots: Vec::new(),
            stages: [None, None, None, None],
            single_display: true,
            lighting: self.lighting,
            copy: TargetCopy::Display,
        };
        let inputs = view(slot, c.size);
        self.report.stages += 1;
        match stage.record(&attachments, &inputs) {
            StageWork::Draws(draws) => self.draws(&c, &d, draws, stage.name()),
            StageWork::Encoded(encoded) => {
                (encoded.commands)(&mut self.encoder);
                self.report.draws += encoded.draws as u64;
                self.report.render_passes += encoded.passes as u64;
            }
        }
    }

    /// The levels `stage` samples ([`Stage::samples`]), in its order.
    fn sampled(&mut self, stage: &dyn Stage, bound: (TargetRef, TargetRef)) -> Vec<Option<gfx::external::SampledLevel>> {
        stage
            .samples()
            .into_iter()
            .map(|r| {
                if r != TargetRef::None && (r == bound.0 || r == bound.1) {
                    unimplemented!("frame graph: stage {} samples {r:?}, its own attachment", stage.name());
                }
                self.target(r).map(|t| gfx::external::SampledLevel { view: t.view, size: t.size })
            })
            .collect()
    }

    fn draws(&mut self, c: &Target, d: &Target, draws: ExternalDraws, name: &str) {
        if draws.packets.is_empty() {
            return;
        }
        // A range already written this submission with other bytes: the
        // work before reads the old bytes, so it goes first.
        let clash = draws
            .writes
            .iter()
            .any(|(b, o, bytes)| self.written.get(&(b.clone(), *o)).is_some_and(|held| held != bytes));
        if clash {
            self.submit();
        }
        for (buffer, offset, bytes) in draws.writes {
            self.arena.write_front(self.gpu.device, self.gpu.queue, &buffer, offset, &bytes);
            self.written.insert((buffer, offset), bytes);
        }
        let mut pass = self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(name),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &c.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &d.view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: d.format.has_stencil_aspect().then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Err(e) = gfx::renderer::encode_in_pass(&mut pass, &draws.packets) {
            unimplemented!("frame graph: stage {name} packets: {e}");
        }
        drop(pass);
        self.report.draws += draws.packets.len() as u64;
        self.report.render_passes += 1;
    }
}

/// The inputs a stage in `slot` gets on a target of `size`: its game
/// view's camera, or none outside a game view.
fn view_inputs(cameras: &Cameras, slot: SlotAt, size: (u32, u32)) -> ViewInputs {
    let camera = slot.slot.in_game_view().then(|| cameras.get(slot.view)).flatten();
    match camera {
        Some(camera) => ViewInputs { guest_size: GUEST, ..ViewInputs::from_camera(&camera, size) },
        None => ViewInputs {
            constants: std::array::from_fn(|_| [0.0; 4]),
            guest_size: GUEST,
            size,
            z_scale: D24_SCALE,
            w_clip: None,
        },
    }
}

/// Draws `frame` on `targets` with `cameras` and submits it.
pub(crate) fn walk(
    gpu: &Gpu<'_>,
    targets: &mut Targets,
    frame: &mut Frame,
    cameras: &Cameras,
    lighting: Lighting,
) -> FrameReport {
    if lighting == Lighting::E3 {
        unimplemented!("frame graph: E3 sun tiles and cone shadows, which read the guest's object shadows");
    }
    let Frame { scene, ops, .. } = frame;
    let mut extra = graph_stages(ops, cameras);
    let view = |slot: SlotAt, size: (u32, u32)| view_inputs(cameras, slot, size);
    let mut walk = Walk::new(gpu, targets, lighting, scene.id);
    for (i, op) in ops.iter_mut().enumerate() {
        walk.op(op, &view);
        for (slot, mut stage) in extra.remove(&i).unwrap_or_default() {
            walk.stage(slot, &mut *stage, &view);
        }
    }
    walk.submit();
    walk.report
}

/// `f` drawn on `r`'s display targets, each view's camera blended from
/// `from`'s scene by its fraction when given ([`Renderer::draw`]).
pub fn draw(r: &mut crate::renderer::Renderer, f: &mut Frame, from: Option<(&FrameScene, f32)>) -> FrameReport {
    r.draw(f, from).0
}

/// `frame` drawn privately at `scale` on targets of its own and read back.
pub(crate) fn capture(
    gpu: &Gpu<'_>,
    ctx: &gfx::GfxContext,
    frame: &mut Frame,
    scale: crate::render_scale::RenderScale,
    lighting: Lighting,
) -> CapturedFrame {
    let mut targets = Targets::new(scale);
    let cameras = Cameras::of(&frame.scene, None);
    let report = walk(gpu, &mut targets, frame, &cameras, lighting);
    let colour = targets.get(gpu.device, TargetRef::SceneColour).expect("scene colour");
    let rgba = gfx::readback::read_texture(ctx, &colour.texture, wgpu::TextureAspect::All, 4, colour.size);
    CapturedFrame {
        size: [colour.size.0, colour.size.1],
        rgba,
        id: frame.scene.id,
        scene_hash: summary_hash(&frame.scene),
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named;
    impl Stage for Named {
        fn name(&self) -> &'static str {
            "named"
        }
        fn record(&mut self, _: &ExternalAttachments, _: &ViewInputs) -> StageWork {
            unimplemented!("test: record")
        }
    }

    fn stage(slot: Slot, view: u8) -> FrameOp {
        FrameOp::Stage { slot: SlotAt { slot, view }, stage: Box::new(Named) }
    }

    #[test]
    fn a_views_graph_stage_follows_its_last_game_view_stage() {
        let ops = vec![
            stage(Slot::Outside, 0),
            stage(Slot::Opaque, 0),
            FrameOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth },
            stage(Slot::Transparents, 0),
            stage(Slot::Opaque, 1),
            stage(Slot::AfterPost, 1),
            stage(Slot::Overlay, 1),
        ];
        assert_eq!(last_stages(&ops, Slot::in_game_view), [(3, 0), (5, 1)]);
        assert!(last_stages(&[], Slot::in_game_view).is_empty());
    }

    #[test]
    fn a_viewport_scales_from_the_game_frame_to_the_target() {
        assert_eq!(texel_viewport((0, 240, 640, 240), (1280, 960)), [0.0, 480.0, 1280.0, 480.0]);
    }
}
