//! Device host for native first-person arms and weapon. Every item is
//! planned at the layer-3 call against the device's entry states before its
//! guest draw is surrendered. The two model calls remain separate external
//! passes in guest order; their light and transparent neighbours stay on D3D.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use render_bridge::native_first_person::{Host, ItemMasks};
use render_bridge::native_objects::NativeSet;
use native_render::object_program_gpu::PlannedDraw;
use native_render::object_state::EntryStates;
use native_render::object_streams::StreamFormats;
use native_render::objects::{ClearedObjects, ItemDraw, ObjectFrame};
use native_render::program::ViewInputs;

use crate::native_first_person_load::{self, MapState, Models};
use crate::native_world_glue::{ReadMemory, palettes_hold, stage_palettes};

/// Hook memory as the stream-format reader expects it.
struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The object encoder uses the whole target as its viewport. A split or
/// otherwise partial view therefore keeps its original D3D draw.
fn full_target_viewport<B: d3d::backend::Backend>(
    device: &d3d::Device<B>,
    mem: &dyn GuestMemory,
) -> bool {
    let target = device.bound_target()[0];
    let Ok(header) = d3d::resource::SurfaceHeader::read(&ReadMemory(mem), target) else {
        return false;
    };
    let Ok(desc) = gfx::texture::TextureDesc::from_header(header.pixels.format, header.pixels.size)
    else {
        return false;
    };
    let viewport = device.viewport();
    viewport.x == 0
        && viewport.y == 0
        && viewport.width == desc.width
        && viewport.height == desc.height
        && viewport.min_z == 0.0
        && viewport.max_z == 1.0
}

/// State every item's plan inherits, resolved before `Host::draw` returns.
#[derive(Clone, Debug)]
struct LayerEntry {
    states: EntryStates,
    w_buffered: bool,
    w_clip: ([f32; 2], bool),
    view: [[f32; 4]; d3d::VIEW_CONSTANTS],
}

/// The installed host: current map generation and this call's entry state.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
    gpu: (wgpu::Device, wgpu::Queue),
    generation: Mutex<u64>,
    entry: Mutex<Option<LayerEntry>>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn native(&self, mem: &mut dyn GuestMemory) -> Option<Arc<NativeSet>> {
        *self.entry.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if !self
            .device
            .lock()
            .ok()?
            .begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem))
            .ok()?
        {
            return None;
        }
        let mut map_name = [0; 32];
        let name = crate::health::read_map_name(mem, &mut map_name).ok()?;
        if name.is_empty() {
            return None;
        }
        let map_generation = render_bridge::native_map_work::generation();
        let current = |state: &Option<MapState>| {
            state.as_ref().is_some_and(|s| s.name == name.as_ref() && s.map_generation == map_generation)
        };
        if !current(&*self.state.lock().unwrap_or_else(|e| e.into_inner())) {
            let _dispatch = timeline::setup::dispatch();
            let (palettes, formats) = {
                let device = self.device.lock().ok()?;
                (stage_palettes(&device, mem), StreamFormats::read(&Ro(mem)))
            };
            let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
            *generation += 1;
            let next = *generation;
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current(&*state) {
                let models = Arc::new(Models::default());
                let set = Arc::new(NativeSet {
                    map: name.to_string(),
                    generation: next,
                    ..NativeSet::default()
                });
                let worker = native_first_person_load::spawn_worker(
                    Arc::clone(&set),
                    Arc::clone(&models),
                    Arc::downgrade(&self.state),
                    palettes,
                    formats,
                    self.gpu.clone(),
                    map_generation,
                );
                *state = Some(MapState {
                    name: name.into_owned(),
                    generation: next,
                    map_generation,
                    models,
                    set,
                    next: None,
                    tried: Default::default(),
                    worker,
                });
            }
        }
        let (set, models) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let state = state.as_mut()?;
            state.activate(timeline::frames());
            (Arc::clone(&state.set), Arc::clone(&state.models))
        };
        let used = models
            .palettes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if used.iter().any(Option::is_some) {
            let device = self.device.lock().ok()?;
            let now = stage_palettes(&device, mem);
            if !palettes_hold(&used, &|stage| now[stage].clone()) {
                return None;
            }
        }

        let entry = {
            let device = self.device.lock().ok()?;
            if !full_target_viewport(&device, mem) {
                return None;
            }
            let read = ReadMemory(mem);
            let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
            let clip = device.depth_clip(&read, native_render::camera::D24_SCALE);
            LayerEntry {
                states: EntryStates::read(|state| {
                    table
                        .canonical_to_guest(state.index())
                        .and_then(|guest| device.render_state(&read, guest))
                }),
                w_buffered: clip.w_buffered == Some(true),
                w_clip: (clip.range, clip.clamp),
                view: std::array::from_fn(|register| device.constant(register)),
            }
        };
        if !entry.w_buffered {
            return None;
        }
        *self.entry.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry);
        Some(set)
    }

    fn want(&self, model: u32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = state.as_mut() else {
            return;
        };
        if state.tried.insert(model as u16)
            && let Some(worker) = &state.worker
        {
            let _ = worker.send(model);
        }
    }

    fn draw(
        &self,
        mem: &mut dyn GuestMemory,
        generation: u64,
        objects: Vec<ObjectFrame>,
        items: Vec<ItemDraw>,
        models: HashMap<u16, u32>,
        masks: ItemMasks,
    ) -> Option<ClearedObjects> {
        if crate::h1_cinematics::reflection_active(mem) { return None; }
        let gpu = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(
                &state
                    .as_ref()
                    .filter(|state| state.generation == generation)?
                    .models,
            )
        };
        let entry = self
            .entry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        let mut device = self.device.lock().ok()?;
        let plan = plan_view(&gpu, &objects, &items, &models, &entry);
        let ranges = plan.draws.len();
        if ranges == 0 {
            return None;
        }
        let (gpu_device, color, depth) = device.external_pipeline_target(&ReadMemory(mem))?;
        if !gpu.lit.try_lock().ok()?.as_mut()?.prewarm_draws(&gpu_device, color, depth, &plan.draws) {
            return None;
        }
        let mut drawn = vec![false; items.len()];
        for &index in &plan.drawn {
            drawn[index] = true;
        }
        let pass = Box::new(FirstPersonPass {
            models: Arc::clone(&gpu),
            draws: plan.draws,
            view: Some(entry.view),
            w_clip: entry.w_clip,
        });
        let view = mem
            .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
            .unwrap_or(u32::MAX);
        let promise = d3d::ExternalPlan {
            stage: timeline::native::Stage::FirstPerson,
            view,
            ranges,
            slots: Vec::new(),
        };
        let ready = device
            .prepare_external_bound(
                &mut crate::native_lights_glue::DeviceMemory(mem),
                promise,
                pass,
                Vec::new(),
            )
            .ok()??;
        // Pin the load through mask application and the commit. A replaced
        // generation cannot win after the copies have been reserved.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state
            .as_ref()
            .is_some_and(|state| state.generation == generation
                && state.map_generation == render_bridge::native_map_work::generation()
                && Arc::ptr_eq(&state.models, &gpu))
        {
            return None;
        }
        let cleared = masks.apply(mem, &drawn)?;
        ready.commit();
        Some(cleared)
    }
}

/// Installs the host beside the other native render stages.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let Some(gpu) = device.lock().ok().and_then(|d| d.backend.upload_device()) else { return };
    render_bridge::native_first_person::set_host(Arc::new(Glue {
        gpu,
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
        generation: Mutex::new(0),
        entry: Mutex::new(None),
    }));
}

#[derive(Default)]
struct ViewPlan {
    /// Places in the input items whose guest draws may be suppressed.
    drawn: Vec<usize>,
    /// Exact planned draws, in first-person list order.
    draws: Vec<(u32, PlannedDraw)>,
}

/// Plans every offered item. Only a fully resolved plan is marked drawn;
/// missing models, programs, constants, entry fields or W mode are refusals
/// here, before `Host::draw` returns, never silent encode-time omissions.
fn plan_view(
    models: &Models,
    objects: &[ObjectFrame],
    items: &[ItemDraw],
    record_models: &HashMap<u16, u32>,
    entry: &LayerEntry,
) -> ViewPlan {
    let mut out = ViewPlan::default();
    let Ok(lit) = models.lit.try_lock() else { return out };
    for (index, item) in items.iter().enumerate() {
        let frame = &objects[item.object];
        let model = record_models[&item.key.record];
        let planned = (|| -> Result<PlannedDraw, String> {
            if !entry.w_buffered {
                return Err("the device's draws are not W-buffered".into());
            }
            let lit = lit.as_ref().ok_or("no first-person model is uploaded")?;
            let draw = lit.plan(model, frame, &item.key, &item.rows, &entry.states)?;
            if draw.texaccum {
                return Err(
                    "it samples the texture accumulation target, which this pass does not bind"
                        .into(),
                );
            }
            Ok(draw)
        })();
        match planned {
            Ok(draw) => {
                out.drawn.push(index);
                out.draws.push((model, draw));
            }
            Err(why) => count_refusal(&why),
        }
    }
    out
}

static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REASONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut reasons = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let reasons = reasons.get_or_insert_with(HashSet::new);
    if reasons.len() < 64 && reasons.insert(why.to_owned()) {
        eprintln!(
            "native-first-person: an item keeps its D3D draw: {why} (each reason logged once)"
        );
    }
}

/// One first-person call's preplanned draws.
struct FirstPersonPass {
    models: Arc<Models>,
    draws: Vec<(u32, PlannedDraw)>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: ([f32; 2], bool),
}

impl FirstPersonPass {
    /// The call's packets for the D3D render pass open on these attachments.
    fn packets(
        &self,
        target: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let constants = self.view.ok_or("first-person view constants unavailable")?;
        let mut guard = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = guard.as_mut().ok_or("planned first-person geometry unavailable")?;
        if !self
            .draws
            .iter()
            .all(|(model, draw)| lit.has(*model) && !draw.indices.is_empty())
        {
            return Err("planned first-person geometry unavailable");
        }
        let view = ViewInputs {
            constants,
            guest_size: request.surface_size,
            size: target.size,
            z_scale: request.z_scale,
            w_clip: Some(self.w_clip),
        };
        lit.draws(
            &target.device,
            &target.queue,
            &target.uploads,
            target.color_format,
            target.depth_format,
            target.size,
            &view,
            &self.draws,
            None,
        )
    }
}

impl d3d::ExternalPass for FirstPersonPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.range, clip.clamp);
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self
                .draws
                .iter()
                .any(|(_, draw)| !draw.state.color_writes.is_empty()),
            depth_stencil: self.draws.iter().any(|(_, draw)| draw.state.depth_write),
        }
    }

    /// The draws join the D3D render pass already open on the call's
    /// attachments: no pass or command buffer of their own.
    fn record_draws(
        &mut self,
        target: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        Some(self.packets(target, request))
    }
}
