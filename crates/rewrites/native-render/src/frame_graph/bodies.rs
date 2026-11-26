//! The Forge monitors' bodies in the frame graph (docs/game/
//! forge-monitor-appearance.md, ML4). The game publishes a [`Set`]
//! ([`crate::bodies::publish`]); each game view of the frame gets one
//! stage after the last op of its own slots, with the markers' place
//! (after the transparents and the post passes, before the overlay). The
//! stage skins each body's model at its pose and draws it on the scene's
//! colour and depth with the view's view-projection, against the guest's
//! W-buffer (`model_gpu`, the D24 rule), inside the view's viewport.
//!
//! A frame with no published set, no usable camera for a view, or a depth
//! that is not a 24-bit W-buffer plans no stage and replays as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces,
    SceneStage,
};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{D24_SCALE, FrameScene, RenderCamera, TargetRef, ViewKind};
use gfx::D3DFormat;
use gfx::external::ExternalEncoded;

use super::markers::last_ops;
use crate::bodies::{self, BodyModel, Set};
use crate::gpu::{DepthRule, PassViewport};
use crate::instances::ObjectInput;
use crate::model::NodeMatrix;
use crate::model_gpu::GpuModels;

/// One device's uploaded models, kept across frames. The published
/// `BodyModel`s are held here as long as the device is, so a pointer match
/// is never a different model.
struct Uploaded {
    device: wgpu::Device,
    gpu: GpuModels,
    models: Vec<(Arc<BodyModel>, u32)>,
}

static DEVICES: Mutex<Vec<Arc<Mutex<Uploaded>>>> = Mutex::new(Vec::new());

fn uploaded_for(device: &wgpu::Device, queue: &wgpu::Queue) -> Arc<Mutex<Uploaded>> {
    let mut all = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    for slot in all.iter() {
        if slot.lock().unwrap_or_else(|e| e.into_inner()).device == *device {
            return Arc::clone(slot);
        }
    }
    let made = Arc::new(Mutex::new(Uploaded {
        device: device.clone(),
        gpu: GpuModels::new(device, queue),
        models: Vec::new(),
    }));
    all.push(Arc::clone(&made));
    made
}

impl Uploaded {
    /// The GPU index of `model`, uploading it the first time.
    fn index_of(&mut self, queue: &wgpu::Queue, model: &Arc<BodyModel>) -> u32 {
        if let Some((_, index)) = self.models.iter().find(|(m, _)| Arc::ptr_eq(m, model)) {
            return *index;
        }
        let bases = &model.bases;
        let index = self.gpu.add_model(&self.device, queue, model.model.clone(), &mut |material| {
            bases.iter().find(|(m, _)| *m == material).map(|(_, image)| image.clone())
        });
        self.models.push((Arc::clone(model), index));
        index
    }
}

/// The body stage to apply after the op at each index of `frame`, from the
/// set the game last published. Empty when there is none.
pub(crate) fn plan(scene: &FrameScene, frame: &ReplayFrame) -> HashMap<usize, ReplayOp> {
    let mut after = HashMap::new();
    let Some(set) = bodies::published() else {
        return after;
    };
    let (Some(colour), Some(depth)) = (frame.colour, frame.depth) else {
        return after;
    };
    if depth.format != D3DFormat::D24S8 as u32 && depth.format != D3DFormat::LinD24S8 as u32 {
        return after;
    }
    for (i, view) in last_ops(frame) {
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
        let stage = BodyStage {
            set: Arc::clone(&set),
            camera,
        };
        after.insert(
            i,
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
                    surface_size: (colour.width, colour.height),
                    display_projection_factor: 1.0,
                    z_scale: D24_SCALE,
                },
                plan: ExternalPlan {
                    stage: timeline::native::Stage::Objects,
                    view: u32::from(view),
                    ranges: set.bodies.len(),
                    slots: vec![],
                },
                pass: Arc::new(Mutex::new(Box::new(stage) as Box<dyn ExternalPass>)),
            }),
        );
    }
    after
}

/// One view's bodies.
struct BodyStage {
    set: Arc<Set>,
    camera: RenderCamera,
}

impl ExternalPass for BodyStage {
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

/// One body ready to draw: its GPU model, its nodes at the pose.
struct Placed {
    model: u32,
    nodes: Vec<NodeMatrix>,
    sphere: ([f32; 3], f32),
    highlight: [u8; 4],
}

impl BodyStage {
    fn encode_on(
        &self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        let (gx, gy, gw, gh) = request.rect;
        let (sw, sh) = request.surface_size;
        if sw == 0 || sh == 0 {
            return Err("body stage has an empty surface");
        }
        let (kx, ky) = (
            target.size.0 as f32 / sw as f32,
            target.size.1 as f32 / sh as f32,
        );
        let viewport = PassViewport {
            x: gx as f32 * kx,
            y: gy as f32 * ky,
            width: gw as f32 * kx,
            height: gh as f32 * ky,
        };
        let slot = uploaded_for(&target.device, &target.queue);
        let mut placed = Vec::new();
        {
            let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
            for body in &self.set.bodies {
                let Some(model) = self.set.models.get(body.model) else {
                    continue;
                };
                let Some(nodes) = bodies::nodes(&model.model, body) else {
                    continue;
                };
                placed.push(Placed {
                    model: held.index_of(&target.queue, model),
                    nodes,
                    sphere: body.sphere,
                    highlight: body.highlight,
                });
            }
        }
        if placed.is_empty() {
            return Err("body stage has no body it can place");
        }
        let camera = self.camera;
        let (device, queue) = (target.device.clone(), target.queue.clone());
        let arena = target.uploads.clone();
        let (colour, depth) = (target.color.clone(), target.depth.clone());
        let (colour_format, depth_format) = (target.color_format, target.depth_format);
        let ranges = placed.len();
        Ok(ExternalEncoded {
            ranges,
            draws: ranges,
            passes: 1,
            commands: Box::new(move |encoder| {
                let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
                let objects: Vec<ObjectInput<'_>> = placed
                    .iter()
                    .map(|p| ObjectInput {
                        model: p.model,
                        nodes: &p.nodes,
                        sphere: p.sphere,
                        highlight: p.highlight,
                    })
                    .collect();
                {
                    let mut uploads = gfx::ring::EncoderUploads {
                        arena: &arena,
                        device: &device,
                        queue: &queue,
                        encoder: &mut *encoder,
                    };
                    held.gpu
                        .prepare(&device, &mut uploads, &camera, DepthRule::D24, &objects);
                }
                held.gpu.encode(
                    &device,
                    encoder,
                    &colour,
                    colour_format,
                    &depth,
                    depth_format,
                    viewport,
                );
            }),
        })
    }
}
