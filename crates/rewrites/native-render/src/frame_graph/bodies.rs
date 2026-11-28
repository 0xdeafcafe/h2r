//! The Forge monitors' bodies in the frame graph (docs/game/
//! forge-monitor-appearance.md, ML4). The game publishes a [`Set`]
//! ([`crate::bodies::publish`]); each game view of the frame gets one
//! stage after its last stage, with the markers' place
//! (after the transparents and the post passes, before the overlay). The
//! stage skins each body's model at its pose and draws it on the scene's
//! colour and depth with the view's view-projection, against the guest's
//! W-buffer (`model_gpu`, the D24 rule), inside the view's viewport.

use std::sync::{Arc, Mutex};

use frame_scene::RenderCamera;
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalEncoded};

use super::{Cameras, guest_rect, last_stages, nothing, texel_viewport};
use crate::bodies::{self, BodyModel, Set};
use crate::gpu::{DepthRule, PassViewport};
use crate::instances::ObjectInput;
use crate::model::NodeMatrix;
use crate::model_gpu::GpuModels;
use crate::program::ViewInputs;
use crate::stage::{FrameOp, Stage, StageWork};

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

/// The body stage of each game view, by the index of the op it follows,
/// from the set the game last published. Empty when there is none.
pub(crate) fn plan(ops: &[FrameOp], cameras: &Cameras) -> Vec<(usize, SlotAt, Box<dyn Stage>)> {
    let Some(set) = bodies::published() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, view) in last_stages(ops, Slot::in_game_view) {
        let Some(camera) = cameras.get(view) else { continue };
        let Some(rect) = guest_rect(&camera) else { continue };
        let stage = BodyStage { set: Arc::clone(&set), camera, rect };
        out.push((i, SlotAt { slot: Slot::AfterPost, view }, Box::new(stage) as Box<dyn Stage>));
    }
    out
}

/// One view's bodies.
struct BodyStage {
    set: Arc<Set>,
    camera: RenderCamera,
    rect: (u32, u32, u32, u32),
}

impl Stage for BodyStage {
    fn name(&self) -> &'static str {
        "bodies"
    }

    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        StageWork::Encoded(self.encode_on(t).unwrap_or_else(|_| nothing()))
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
    fn encode_on(&self, target: &ExternalAttachments) -> Result<ExternalEncoded, &'static str> {
        let [x, y, width, height] = texel_viewport(self.rect, target.size);
        let viewport = PassViewport { x, y, width, height };
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
