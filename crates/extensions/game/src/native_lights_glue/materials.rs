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
    InputKey, LightInputs, LightProgram, LightRefusal, LightStage, compile_light,
    compile_light_model, engine_inputs, light_look,
};
use tags::material::{EngineInputs, ShaderLibrary};

use super::{Compiled, Keep, LitMaterial, models::ModelLights};

const CAPACITY: usize = 512;
const LOADING: Keep = Keep::MaterialPending;

type Images = HashMap<(usize, TagId, u32), Image>;
type Programs = HashMap<TagId, Vec<u8>>;

#[derive(Clone)]
pub(super) struct Resolved {
    pub(super) compiled: Arc<Compiled>,
    pub(super) buffer: Option<usize>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Variant {
    shader: TagId,
    inputs: InputKey,
    layer: usize,
    implementation: usize,
    // A structure part uses WorldVertex. Model programs depend on the actual
    // shader-selected stream in the section and its classification.
    model: Option<(u32, u16, u32)>,
}

type ResultSlot = OnceLock<(u64, Result<Resolved, Keep>)>;

struct Job {
    key: Variant,
    inputs: LightInputs,
    instance: Box<EngineInputs>,
    model: Option<Arc<ModelLights>>,
    slot: Weak<ResultSlot>,
}

#[derive(Default)]
struct Variants {
    slots: HashMap<Variant, Arc<ResultSlot>>,
    order: VecDeque<Variant>,
}

impl Variants {
    fn request(
        &mut self,
        sender: &mpsc::SyncSender<Job>,
        key: Variant,
        inputs: LightInputs,
        instance: &EngineInputs,
        model: Option<&Arc<ModelLights>>,
    ) -> Result<Resolved, Keep> {
        if let Some(slot) = self.slots.get(&key) {
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
        self.order.push_back(key);
        self.slots.insert(key, slot);
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

const QUEUE_DEPTH: usize = 64;

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
}

impl Resolver {
    pub(super) fn new(lib: ShaderLibrary, code: Programs) -> Self {
        Self {
            source: Mutex::new(Some((lib, code))),
            queue: OnceLock::new(),
        }
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
        inputs: LightInputs,
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
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader: material.shader,
            inputs: inputs.key(),
            layer,
            implementation,
            model: None,
        };
        self.request(key, &material.instance, inputs, None)
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
    ) -> Result<Resolved, Keep> {
        let key = Variant {
            shader,
            inputs: inputs.key(),
            layer,
            implementation,
            model: Some((
                entry.model,
                entry.stream.section,
                u32::from(entry.stream.vertex_class),
            )),
        };
        self.request(key, instance, inputs, Some(lights))
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
    uploaded: HashMap<(bool, u64), (Weak<LightProgram>, Weak<Mutex<LightGpu>>)>,
}

impl Worker {
    fn resolve(
        &mut self,
        key: Variant,
        instance: &EngineInputs,
        inputs: &LightInputs,
        model: Option<&ModelLights>,
    ) -> Result<Resolved, Keep> {
        let material = self
            .lib
            .material(key.shader, Some(engine_inputs(instance, inputs)))
            .map_err(|error| Keep::Refused(format!("material: {error}")))?;
        let look =
            light_look(&material, key.layer, key.implementation).map_err(
                |reason| match reason {
                    LightRefusal::NoLayer => {
                        Keep::Unsupported("built light entry has no native material pass")
                    }
                    reason => Keep::Refused(format!("{reason:?}")),
                },
            )?;
        let (program, buffer) = match (key.model, model) {
            (Some((model, section, class)), Some(lights)) => {
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
                (
                    compile_light_model(&look, code, buffer.provided(), buffer.fetch()),
                    Some(index),
                )
            }
            (None, None) => {
                let code = self
                    .code
                    .get(&look.vertex_shader)
                    .ok_or(Keep::Unsupported("no world-space vertex program"))?;
                (compile_light(&look, code), None)
            }
            _ => return Err(Keep::Unsupported("light geometry unavailable")),
        };
        let program = Arc::new(program.map_err(Keep::Refused)?);
        // A model load may have used different stage palettes from the BSP.
        // Keep their expanded images in distinct GPU entries.
        let id = (model.is_some(), light_gpu_key(&program));
        let gpu = self.uploaded.get(&id).and_then(|(original, gpu)| {
            same_gpu_pass(original.upgrade()?.as_ref(), &program)
                .then(|| gpu.upgrade())
                .flatten()
        });
        let gpu = match gpu {
            Some(gpu) => gpu,
            None => {
                let images = model.map_or(self.images.as_ref(), |lights| &lights.images);
                let gpu = LightGpu::new(
                    &self.device,
                    &self.queue,
                    Arc::clone(&program),
                    &self.layout,
                    &mut |stage, source| {
                        let LightStage::Bitmap { bitmap, image } = source else {
                            return None;
                        };
                        images.get(&(stage, bitmap, image)).cloned()
                    },
                );
                // Texture writes can precede a shader validation failure.
                self.queue.submit([]);
                let gpu = Arc::new(Mutex::new(
                    gpu.ok_or(Keep::Unsupported("light shader or texture upload failed"))?,
                ));
                if self.uploaded.len() >= CAPACITY {
                    self.uploaded.retain(|_, (_, gpu)| gpu.strong_count() != 0);
                }
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
    fn a_cold_material_keeps_the_guest_draw_and_a_full_queue_retries() {
        let (send, recv) = mpsc::sync_channel(1);
        let key = |layer| Variant {
            shader: TagId(9),
            inputs: InputKey([0; 5]),
            layer,
            implementation: 0,
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
        let request =
            |v: &mut Variants, layer| v.request(&send, key(layer), inputs, &instance, None).err();
        assert_eq!(request(&mut variants, 12), Some(LOADING));
        assert_eq!(
            request(&mut variants, 12),
            Some(LOADING),
            "same work is already queued"
        );
        assert_eq!(request(&mut variants, 13), Some(LOADING));
        assert!(
            !variants.slots.contains_key(&key(13)),
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
