//! Light material variants depend on the live light inputs and the chosen
//! model stream. A bounded, per-BSP queue resolves new variants. The guest
//! only observes a completed result at the following frame boundary.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};

use frame_scene::LightEntry;
use map::TagId;
use native_render::gpu::Image;
use native_render::light_gpu::{LightGpu, light_gpu_key, same_gpu_pass};
use native_render::light_look::{
    InputKey, LightInputs, LightProgram, LightRefusal, LightStage, animated_light_look, compile_light,
    compile_light_model, engine_inputs,
};
use tags::material::{EngineInputs, ShaderLibrary};

use super::{Compiled, Keep, LitMaterial, models::{ModelLights, ModelSetId}};

const CAPACITY: usize = 512;
const QUEUE_DEPTH: usize = 64;
const LOADING: Keep = Keep::MaterialPending;

type Images = HashMap<(usize, TagId, u32), Image>;
type Programs = HashMap<(TagId, u32), Vec<u8>>;

/// What a live edit of an image reaches: the light passes the worker has
/// built (by whether they belong to a model part) and the images it
/// builds the next ones from.
#[derive(Default)]
struct Shared {
    built: Mutex<Vec<(bool, Weak<Mutex<LightGpu>>)>>,
    /// Edited stage images by (stage, bitmap, image), which a pass built
    /// later takes over the load's.
    edited: Mutex<Images>,
}

fn remember_gpu<T>(built: &mut Vec<(bool, Weak<T>)>, model: bool, gpu: &Arc<T>) {
    // A retired variant can leave a dead weak entry without an image edit
    // ever occurring. Keep all live edit targets, including in-flight draws.
    if built.len() >= CAPACITY + QUEUE_DEPTH {
        built.retain(|(_, gpu)| gpu.strong_count() != 0);
    }
    built.push((model, Arc::downgrade(gpu)));
}

#[derive(Clone)]
pub(super) struct Resolved {
    pub(super) compiled: Arc<Compiled>,
    pub(super) buffer: Option<usize>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Variant {
    shader: TagId,
    inputs: Option<InputKey>,
    layer: usize,
    implementation: usize,
    // A structure part uses WorldVertex. Model programs depend on the actual
    // shader-selected stream in the section and its classification.
    model: Option<(u32, u16)>,
    vertex_class: u32,
    selection: Option<(u8, u8)>,
}

/// World resources belong to this BSP resolver. Model resources can be
/// replaced independently, including while draws still hold the old images.
type ResourceKey<T> = (Option<ModelSetId>, T);

fn resource_key<T>(model: Option<&ModelLights>, value: T) -> ResourceKey<T> {
    (model.map(|resources| resources.id), value)
}

type ResultSlot = OnceLock<(u64, Result<Resolved, Keep>)>;

struct Job {
    key: Variant,
    inputs: Option<LightInputs>,
    instance: Box<EngineInputs>,
    model: Option<Arc<ModelLights>>,
    slot: Weak<ResultSlot>,
}

#[derive(Default)]
struct Variants {
    slots: HashMap<ResourceKey<Variant>, Arc<ResultSlot>>,
    order: VecDeque<ResourceKey<Variant>>,
}

impl Variants {
    fn request(
        &mut self,
        sender: &mpsc::SyncSender<Job>,
        key: Variant,
        inputs: Option<LightInputs>,
        instance: &EngineInputs,
        model: Option<&Arc<ModelLights>>,
    ) -> Result<Resolved, Keep> {
        let scoped = resource_key(model.map(Arc::as_ref), key);
        if let Some(slot) = self.slots.get(&scoped) {
            let (_, result) = slot
                .get()
                .filter(|(frame, _)| *frame < timeline::frames())
                .ok_or(LOADING)?;
            return result.clone();
        }
        // Only the worker evicts completed GPU resources. Bound the extra
        // entries by the number of outstanding requests if it falls behind.
        if self.slots.len() >= CAPACITY + QUEUE_DEPTH {
            return Err(LOADING);
        }
        let slot = Arc::new(ResultSlot::new());
        let job = Job {
            key,
            inputs,
            instance: Box::new(instance.clone()),
            model: model.cloned(),
            slot: Arc::downgrade(&slot),
        };
        sender.try_send(job).map_err(|_| LOADING)?;
        self.order.push_back(scoped);
        self.slots.insert(scoped, slot);
        Err(LOADING)
    }

    fn retire(&mut self) -> Vec<Arc<ResultSlot>> {
        let mut retired = Vec::new();
        while self.slots.len() > CAPACITY {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.slots.remove(&key) {
                retired.push(old);
            }
        }
        retired
    }
}

struct Queue {
    // The worker retains the table until the last sender closes, so dropping
    // the map on the guest does not destroy the resident GPU resources.
    variants: Arc<Mutex<Variants>>,
    lifetime: Arc<()>,
    sender: mpsc::SyncSender<Job>,
}

pub(super) struct Resolver {
    source: Mutex<Option<(ShaderLibrary, Programs)>>,
    queue: OnceLock<Queue>,
    shared: Arc<Shared>,
}

impl Resolver {
    pub(super) fn new(lib: ShaderLibrary, code: Programs) -> Self {
        Self {
            source: Mutex::new(Some((lib, code))),
            queue: OnceLock::new(),
            shared: Arc::default(),
        }
    }

    /// Rewrites the stage textures of the built world passes that sample
    /// an image of `decoded` (stage images by (stage, bitmap, image), as
    /// the load decoded them, each through its stage's palette), and
    /// keeps them for the passes built later. A texture whose size, level
    /// count or format changed keeps its pixels, and its pass says so.
    /// Passes of model parts hold the model load's images: they are
    /// rebuilt by a reload of the models, not here.
    pub(super) fn refresh_images(&self, device: &wgpu::Device, queue: &wgpu::Queue, decoded: &Images) {
        self.shared
            .edited
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(decoded.iter().map(|(k, v)| (*k, v.clone())));
        let mut built = self.shared.built.lock().unwrap_or_else(|e| e.into_inner());
        built.retain(|(_, gpu)| gpu.strong_count() != 0);
        let mut images: Vec<(TagId, u32)> = decoded
            .keys()
            .map(|&(_, bitmap, image)| (bitmap, image))
            .collect();
        images.sort_by_key(|&(bitmap, image)| (bitmap.0, image));
        images.dedup();
        for (model, gpu) in built.iter() {
            let (false, Some(gpu)) = (*model, gpu.upgrade()) else {
                continue;
            };
            let mut gpu = gpu.lock().unwrap_or_else(|e| e.into_inner());
            for &(bitmap, image) in &images {
                let held = gpu.held(bitmap, image);
                if held.is_empty() {
                    continue;
                }
                let by_stage: HashMap<usize, Image> = decoded
                    .iter()
                    .filter(|((_, b, i), _)| (*b, *i) == (bitmap, image))
                    .map(|(&(stage, ..), pixels)| (stage, pixels.clone()))
                    .collect();
                if let Err(why) = gpu.edit_image(device, queue, bitmap, image, &by_stage) {
                    eprintln!(
                        "native-lights: bitmap {:#010x} image {image} keeps its pixels in a light pass until the map reloads ({why:?})",
                        bitmap.0
                    );
                }
            }
        }
        drop(built);
        // Flush the writes before the next admitted draw.
        queue.submit([]);
    }

    pub(super) fn start(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        images: Arc<Images>,
    ) {
        let Some((lib, code)) = self.source.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let mut worker = Worker {
            lib,
            code,
            images,
            device: device.clone(),
            queue: queue.clone(),
            layout: layout.clone(),
            uploaded: HashMap::new(),
            shared: Arc::clone(&self.shared),
        };
        let (sender, receiver) = mpsc::sync_channel::<Job>(QUEUE_DEPTH);
        let queue = Queue {
            variants: Arc::default(),
            lifetime: Arc::new(()),
            sender,
        };
        let active = Arc::downgrade(&queue.lifetime);
        let table = Arc::clone(&queue.variants);
        if let Err(error) = background::spawn("native light materials", move || {
            for job in receiver {
                if active.strong_count() == 0 {
                    break;
                }
                if let Some(slot) = job.slot.upgrade() {
                    let _timer = timeline::setup::Stage::Lights.worker();
                    let result =
                        worker.resolve(job.key, &job.instance, &job.inputs, job.model.as_deref());
                    let _ = slot.set((timeline::frames(), result));
                }
                let retired = table.lock().unwrap_or_else(|e| e.into_inner()).retire();
                drop(retired);
            }
        }) {
            eprintln!("native-lights: could not start material worker: {error}");
            return;
        }
        let _ = self.queue.set(queue);
    }

    fn request(
        &self,
        key: Variant,
        instance: &EngineInputs,
        inputs: Option<LightInputs>,
        model: Option<&Arc<ModelLights>>,
    ) -> Result<Resolved, Keep> {
        let queue = self.queue.get().ok_or(LOADING)?;
        let mut variants = queue.variants.try_lock().map_err(|_| LOADING)?;
        variants.request(&queue.sender, key, inputs, instance, model)
    }

    pub(super) fn world(
        &self,
        material: &LitMaterial,
        inputs: LightInputs,
        layer: usize,
        implementation: usize,
        vertex_class: u32,
        selection: Option<(u8, u8)>,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader: material.shader,
            inputs: Some(inputs.key()),
            layer,
            implementation,
            vertex_class,
            selection,
            model: None,
        };
        self.request(key, &material.instance, Some(inputs), None)
    }

    /// A structure caster pass, resolved with the instance's inputs only.
    pub(super) fn world_shadow(
        &self,
        material: &LitMaterial,
        implementation: usize,
        vertex_class: u32,
        selection: Option<(u8, u8)>,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader: material.shader,
            inputs: None,
            layer: native_render::shadow_look::CASTER_LAYER,
            implementation,
            vertex_class,
            selection,
            model: None,
        };
        self.request(key, &material.instance, None, None)
    }

    /// A structure receiver pass, resolved with the instance's inputs only.
    pub(super) fn world_receiver(
        &self,
        material: &LitMaterial,
        layer: usize,
        implementation: usize,
        vertex_class: u32,
        selection: Option<(u8, u8)>,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader: material.shader,
            inputs: None,
            layer,
            implementation,
            vertex_class,
            selection,
            model: None,
        };
        self.request(key, &material.instance, None, None)
    }

    /// A model receiver pass over the entry's resident section buffer.
    pub(super) fn model_receiver(
        &self,
        shader: TagId,
        instance: &EngineInputs,
        lights: &Arc<ModelLights>,
        entry: &LightEntry,
        layer: usize,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader,
            inputs: None,
            layer,
            implementation: usize::from(entry.binding.implementation),
            model: Some((entry.model, entry.stream.section)),
            vertex_class: u32::from(entry.stream.vertex_class),
            selection: Some((entry.binding.variant, entry.binding.selector)),
        };
        self.request(key, instance, None, Some(lights))
    }

    pub(super) fn model(
        &self,
        shader: TagId,
        instance: &EngineInputs,
        inputs: LightInputs,
        layer: usize,
        implementation: usize,
        lights: &Arc<ModelLights>,
        entry: &LightEntry,
        selection: Option<(u8, u8)>,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader,
            inputs: Some(inputs.key()),
            layer,
            implementation,
            model: Some((entry.model, entry.stream.section)),
            vertex_class: u32::from(entry.stream.vertex_class),
            selection,
        };
        self.request(key, instance, Some(inputs), Some(lights))
    }

    /// A model caster pass over the entry's resident section buffer.
    pub(super) fn model_shadow(
        &self,
        shader: TagId,
        instance: &EngineInputs,
        lights: &Arc<ModelLights>,
        entry: &LightEntry,
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader,
            inputs: None,
            layer: native_render::shadow_look::CASTER_LAYER,
            implementation: usize::from(entry.binding.implementation),
            model: Some((entry.model, entry.stream.section)),
            vertex_class: u32::from(entry.stream.vertex_class),
            selection: Some((entry.binding.variant, entry.binding.selector)),
        };
        self.request(key, instance, None, Some(lights))
    }
}

// Only the worker touches the shader library and constructs GPU resources.
// Equal shaders and static texture bindings share uploads across input
// variants. The corresponding program retains its own uniform values.
struct Worker {
    lib: ShaderLibrary,
    code: Programs,
    images: Arc<Images>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    uploaded: HashMap<ResourceKey<u64>, (Weak<LightProgram>, Weak<Mutex<LightGpu>>)>,
    shared: Arc<Shared>,
}

impl Worker {
    fn resolve(
        &mut self,
        key: Variant,
        instance: &EngineInputs,
        inputs: &Option<LightInputs>,
        model: Option<&ModelLights>,
    ) -> Result<Resolved, Keep> {
        let resolved_inputs = inputs.as_ref().map_or_else(
            || instance.clone(),
            |inputs| engine_inputs(instance, inputs),
        );
        let material = self
            .lib
            .material(key.shader, Some(resolved_inputs))
            .map_err(|error| Keep::Refused(format!("material: {error}").into()))?;
        // A lit pass the instance animates compiles from its baseline and
        // samples its constants at each draw's clock.
        let mut animation = None;
        let look = if key.layer == native_render::shadow_look::CASTER_LAYER {
            native_render::shadow_look::shadow_look(&material, key.implementation, key.selection)
        } else if matches!(
            key.layer,
            native_render::shadow_look::RECEIVER_LAYER | native_render::shadow_look::OPACITY_LAYER
        ) {
            native_render::shadow_look::receiver_look(&material, key.layer, key.implementation, key.selection)
        } else {
            let properties = native_render::layer_pass::instance_properties(&self.lib, key.shader);
            animated_light_look(&material, key.layer, key.implementation, key.selection, Some(&properties))
                .map(|(look, sampled)| {
                    animation = sampled;
                    look
                })
        }
        .map_err(|reason| match reason {
            LightRefusal::NoLayer => {
                Keep::Unsupported("built light entry has no native material pass")
            }
            reason => Keep::Refused(format!("{reason:?}").into()),
        })?;
        let (program, buffer) = match (key.model, model) {
            (Some((model, section)), Some(lights)) => {
                let class = key.vertex_class;
                let section = lights
                    .models
                    .models
                    .get(&model)
                    .and_then(|m| m.sections.get(&section))
                    .ok_or(Keep::Unsupported("model light section unavailable"))?;
                let (index, buffer) = section
                    .buffers
                    .iter()
                    .enumerate()
                    .find(|(_, b)| {
                        b.vertex_shader == look.vertex_shader && b.classification == class
                    })
                    .ok_or(Keep::Unsupported(
                        "no model light buffer for the entry's program and classification",
                    ))?;
                let code = lights
                    .microcode
                    .get(&(look.vertex_shader, class))
                    .ok_or(Keep::Unsupported("no model vertex program"))?;
                let program = if key.layer == native_render::shadow_look::CASTER_LAYER {
                    native_render::shadow_look::compile_shadow_model(
                        &look,
                        code,
                        buffer.provided(),
                        buffer.fetch(),
                    )
                } else if matches!(
                    key.layer,
                    native_render::shadow_look::RECEIVER_LAYER
                        | native_render::shadow_look::OPACITY_LAYER
                ) {
                    native_render::shadow_look::compile_receiver_model(
                        &look,
                        code,
                        buffer.provided(),
                        buffer.fetch(),
                    )
                } else {
                    compile_light_model(&look, code, buffer.provided(), buffer.fetch())
                };
                (program, Some(index))
            }
            (None, None) => {
                let code = self
                    .code
                    .get(&(look.vertex_shader, key.vertex_class))
                    .ok_or(Keep::Unsupported("no structure light program for the selected classification"))?;
                let program = if key.layer == native_render::shadow_look::CASTER_LAYER {
                    native_render::shadow_look::compile_shadow(&look, code)
                } else if matches!(
                    key.layer,
                    native_render::shadow_look::RECEIVER_LAYER
                        | native_render::shadow_look::OPACITY_LAYER
                ) {
                    native_render::shadow_look::compile_receiver(&look, code)
                } else {
                    compile_light(&look, code)
                };
                (program, None)
            }
            _ => return Err(Keep::Unsupported("light geometry unavailable")),
        };
        let mut program = program.map_err(|why| Keep::Refused(why.into()))?;
        program.animation = animation;
        let program = Arc::new(program);
        let id = resource_key(model, light_gpu_key(&program));
        let gpu = self.uploaded.get(&id).and_then(|(original, gpu)| {
            same_gpu_pass(original.upgrade()?.as_ref(), &program)
                .then(|| gpu.upgrade())
                .flatten()
        });
        let gpu = match gpu {
            Some(gpu) => gpu,
            None => {
                let images = model.map_or(self.images.as_ref(), |lights| &lights.images);
                let edited = self.shared.edited.lock().unwrap_or_else(|e| e.into_inner());
                let gpu = LightGpu::new(
                    &self.device,
                    &self.queue,
                    Arc::clone(&program),
                    &self.layout,
                    &mut |stage, source| {
                        let LightStage::Bitmap { bitmap, image } = source else {
                            return None;
                        };
                        let key = (stage, bitmap, image);
                        model
                            .is_none()
                            .then(|| edited.get(&key).cloned())
                            .flatten()
                            .or_else(|| images.get(&key).cloned())
                    },
                );
                drop(edited);
                // Texture writes can precede a shader validation failure.
                self.queue.submit([]);
                let gpu = Arc::new(Mutex::new(
                    gpu.ok_or(Keep::Unsupported("light shader or texture upload failed"))?,
                ));
                if self.uploaded.len() >= CAPACITY {
                    self.uploaded.retain(|_, (_, gpu)| gpu.strong_count() != 0);
                }
                remember_gpu(
                    &mut self.shared.built.lock().unwrap_or_else(|e| e.into_inner()),
                    model.is_some(),
                    &gpu,
                );
                self.uploaded
                    .insert(id, (Arc::downgrade(&program), Arc::downgrade(&gpu)));
                gpu
            }
        };
        Ok(Resolved {
            compiled: Arc::new(Compiled { program, gpu }),
            buffer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_light_variants_do_not_accumulate_live_edit_targets() {
        let held = Arc::new(0);
        let mut built = Vec::new();
        remember_gpu(&mut built, false, &held);
        for i in 1..=4 * (CAPACITY + QUEUE_DEPTH) {
            let gpu = Arc::new(i);
            remember_gpu(&mut built, i % 2 != 0, &gpu);
            assert!(built.len() <= CAPACITY + QUEUE_DEPTH);
        }
        assert_eq!(
            built.iter().filter(|(_, gpu)| gpu.strong_count() != 0).count(),
            1
        );
        assert_eq!(*built[0].1.upgrade().unwrap(), 0);
    }

    #[test]
    fn model_reload_requests_fresh_variants_and_uploads_for_the_same_light_pass() {
        let old = Arc::new(ModelLights::default());
        let reloaded = Arc::new(ModelLights::default());
        let mut variants = Variants::default();
        let (send, recv) = mpsc::sync_channel(2);
        let instance = EngineInputs::default();
        for layer in [10, 11, 12, 13, 14] {
            let key = Variant {
                shader: TagId(27),
                inputs: None,
                layer,
                implementation: 2,
                model: Some((5, 3)),
                vertex_class: 4,
                selection: None,
            };
            // Two draws against the old buffers share one request. The same
            // shader + section after reload must enqueue another.
            for resources in [&old, &old, &reloaded] {
                assert_eq!(variants.request(&send, key, None, &instance, Some(resources)).err(), Some(LOADING));
            }
            let jobs: Vec<_> = recv.try_iter().collect();
            assert_eq!(jobs.len(), 2);
            assert!(Arc::ptr_eq(jobs[0].model.as_ref().unwrap(), &old));
            assert!(Arc::ptr_eq(jobs[1].model.as_ref().unwrap(), &reloaded));
        }

        let same_program_hash = 16_u64;
        let uploaded = HashMap::from([
            (resource_key(None, same_program_hash), "world palette"),
            (resource_key(Some(&old), same_program_hash), "original palette"),
        ]);
        assert_eq!(uploaded[&resource_key(Some(&old), same_program_hash)], "original palette");
        assert!(!uploaded.contains_key(&resource_key(Some(&reloaded), same_program_hash)));
    }

    #[test]
    fn a_cold_material_keeps_the_guest_draw_and_a_full_queue_retries() {
        let (send, recv) = mpsc::sync_channel(1);
        let key = |layer| Variant {
            shader: TagId(9),
            inputs: Some(InputKey([0; 5])),
            layer,
            implementation: 0,
            vertex_class: 0,
            selection: None,
            model: None,
        };
        let instance = EngineInputs::default();
        let inputs = LightInputs {
            colour_a: [0.; 3],
            colour_b: [0.; 3],
            mode: 0,
            definition_8a: 0,
            definition_80: 0,
            definition_flags: 0,
            falloff_0: 0.,
        };
        let mut variants = Variants::default();
        let request = |v: &mut Variants, layer| {
            v.request(&send, key(layer), Some(inputs), &instance, None)
                .err()
        };
        assert_eq!(request(&mut variants, 12), Some(LOADING));
        assert_eq!(
            request(&mut variants, 12),
            Some(LOADING),
            "same work is already queued"
        );
        assert_eq!(request(&mut variants, 13), Some(LOADING));
        assert!(
            !variants.slots.contains_key(&resource_key(None, key(13))),
            "a full queue does not retain an unbuilt slot"
        );

        let slot = recv.try_recv().unwrap().slot.upgrade().unwrap();
        assert!(
            slot.get().is_none(),
            "the caller left preparation to the worker"
        );
        assert!(
            slot.set((timeline::frames(), Err(Keep::NoMaterial)))
                .is_ok()
        );
        timeline::frame();
        assert_eq!(request(&mut variants, 12), Some(Keep::NoMaterial));
        assert_eq!(request(&mut variants, 13), Some(LOADING));
        assert_eq!(
            recv.try_recv().unwrap().key.layer,
            13,
            "the retry reached the worker"
        );
    }
}
