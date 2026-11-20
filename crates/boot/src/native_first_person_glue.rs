//! Device host for native first-person arms and weapon. Every item is
//! planned at the layer-3 call against the device's entry states before its
//! guest draw is surrendered. The two model calls remain separate external
//! passes in guest order; their light and transparent neighbours stay on D3D.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::memory::GuestMemory;
use feature::native_first_person::{Host, ItemMasks};
use feature::native_objects::NativeSet;
use native_render::object_program_gpu::{LitObjects, PlannedDraw};
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
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() {
            return None;
        }
        let current = |state: &Option<MapState>| {
            state.as_ref().map(|s| s.name.as_str()) == Some(name.as_str())
        };
        if !current(&*self.state.lock().unwrap_or_else(|e| e.into_inner())) {
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
                let worker = native_first_person_load::spawn_worker(
                    name.clone(),
                    next,
                    Arc::clone(&models),
                    Arc::clone(&self.state),
                    palettes,
                    formats,
                    compare().map(|c| c.definition),
                );
                *state = Some(MapState {
                    name: name.clone(),
                    generation: next,
                    models,
                    set: Arc::new(NativeSet {
                        map: name.clone(),
                        generation: next,
                        ..NativeSet::default()
                    }),
                    tried: Default::default(),
                    worker,
                });
            }
        }
        let (set, models) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let state = state.as_ref()?;
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
        let pending = !gpu
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        if ranges == 0 && plan.compare.is_none() && !pending {
            return None;
        }
        let mut drawn = vec![false; items.len()];
        for &index in &plan.drawn {
            drawn[index] = true;
        }
        let pass = Box::new(FirstPersonPass {
            models: Arc::clone(&gpu),
            draws: plan.draws,
            compare: plan.compare,
            view: Some(entry.view),
            w_clip: entry.w_clip,
        });
        if ranges == 0 {
            // Upload new models or arm a comparison; every guest item stays live.
            device.request_external_pass(pass);
            return None;
        }
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
            .is_some_and(|state| state.generation == generation && Arc::ptr_eq(&state.models, &gpu))
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
    feature::native_first_person::set_host(Arc::new(Glue {
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
    /// The shadow item selected for comparison: model, draw and label.
    compare: Option<(u32, PlannedDraw, String)>,
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
    let lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
    let compared = compare();
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
            Ok(draw) if item.shadow => {
                if out.compare.is_none()
                    && compared.is_some_and(|compare| {
                        compare.definition == model as u16 && compare.part == draw.part
                    })
                {
                    let label = format!(
                        "first-person object {:#010x} mode {model:#010x} part {} record {}",
                        frame.handle, draw.part, item.key.record
                    );
                    out.compare = Some((model, draw, label));
                }
            }
            Ok(draw) => {
                out.drawn.push(index);
                out.draws.push((model, draw));
            }
            Err(why) => {
                count_refusal(&why);
                if item.shadow && compared.is_some() {
                    eprintln!(
                        "native-first-person compare: mode {model:#010x} record {}: {why}",
                        item.key.record
                    );
                }
            }
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
    compare: Option<(u32, PlannedDraw, String)>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: ([f32; 2], bool),
}

impl FirstPersonPass {
    fn upload<'a>(
        &'a self,
        target: &d3d::ExternalTarget<'_>,
    ) -> MutexGuard<'a, Option<LitObjects>> {
        let mut guard = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = guard.get_or_insert_with(|| LitObjects::new(target.device));
        for cpu in std::mem::take(
            &mut *self
                .models
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        ) {
            if let Some(normalization) = &cpu.normalization {
                lit.set_normalization(target.device, target.queue, normalization);
            }
            lit.add_model(target.device, target.queue, cpu.definition, cpu.lit);
        }
        guard
    }

    fn view_inputs(
        &self,
        constants: [[f32; 4]; d3d::VIEW_CONSTANTS],
        target: &d3d::ExternalTarget<'_>,
    ) -> ViewInputs {
        ViewInputs {
            constants,
            guest_size: target.guest_size,
            size: target.size,
            z_scale: target.z_scale,
            w_clip: Some(self.w_clip),
        }
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

    fn prepare_copy(&mut self, target: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() {
            return Err("first-person view constants unavailable");
        }
        drop(self.upload(target));
        let lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        if lit.as_ref().is_some_and(|lit| {
            self.draws
                .iter()
                .all(|(model, draw)| lit.has(*model) && !draw.indices.is_empty())
        }) {
            Ok(())
        } else {
            Err("planned first-person geometry unavailable")
        }
    }

    fn encode(&mut self, target: d3d::ExternalTarget<'_>) {
        // Comparison-only views can upload models for the following view.
        drop(self.upload(&target));
        self.record_ranges(target);
    }

    fn record_ranges(&mut self, target: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else {
            return 0;
        };
        let mut guard = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = guard.as_mut() else {
            return 0;
        };
        let view = self.view_inputs(constants, &target);
        if let Some((model, draw, label)) = &self.compare {
            arm_compare(
                lit,
                *model,
                draw,
                label,
                &view,
                compare().map_or(0, |compare| compare.after),
            );
        }
        if self.draws.is_empty() {
            return 0;
        }
        lit.encode(
            target.device,
            target.queue,
            target.uploads,
            &mut *target.encoder,
            target.color,
            target.color_format,
            target.depth,
            target.depth_format,
            target.size,
            &view,
            &self.draws,
            None,
        )
    }
}

/// `H2_NATIVE_FIRST_PERSON_COMPARE=mode:part[:after]`: keep that record
/// mode on D3D and arm its planned draw on the selected external-target
/// encode (scaled output encodes a pass more than once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Compare {
    definition: u16,
    part: usize,
    after: u32,
}

fn compare() -> Option<Compare> {
    static COMPARE: std::sync::OnceLock<Option<Compare>> = std::sync::OnceLock::new();
    *COMPARE.get_or_init(|| {
        let compare = std::env::var("H2_NATIVE_FIRST_PERSON_COMPARE")
            .ok()
            .and_then(|value| parse_compare(&value));
        if let Some(compare) = compare {
            eprintln!(
                "native-first-person: comparing {compare:?} with D3D (its items stay on D3D)"
            );
        }
        compare
    })
}

fn parse_compare(value: &str) -> Option<Compare> {
    let mut fields = value.trim().split(':');
    let definition = fields.next()?.trim();
    let definition = definition
        .strip_prefix("0x")
        .or_else(|| definition.strip_prefix("0X"))
        .unwrap_or(definition);
    let definition = u32::from_str_radix(definition, 16).ok()? as u16;
    let part = fields
        .next()
        .map_or(Some(0), |part| part.trim().parse().ok())?;
    let after = fields
        .next()
        .map_or(Some(0), |after| after.trim().parse().ok())?;
    if fields.next().is_some() {
        return None;
    }
    Some(Compare {
        definition,
        part,
        after,
    })
}

fn arm_compare(
    lit: &LitObjects,
    model: u32,
    draw: &PlannedDraw,
    label: &str,
    view: &ViewInputs,
    after: u32,
) {
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) != after {
        return;
    }
    match lit.part_uniforms(model, draw, view, None) {
        Ok(uniforms) => {
            eprintln!(
                "native-first-person compare {label}: implementation {}, {} program words, depth {:?}, primitive {:?}, textures {:?}",
                uniforms.implementation,
                uniforms.words.len(),
                uniforms.depth,
                uniforms.primitive,
                uniforms.textures
            );
            d3d::uniform_compare::arm(d3d::uniform_compare::Expected {
                label: label.to_owned(),
                words: uniforms.words,
                vertex: uniforms.vertex,
                pixel: uniforms.pixel,
                options: uniforms.options,
                def: uniforms.def,
                pixel_options: uniforms.pixel_options,
                primitive: uniforms.primitive,
                depth: (uniforms.depth.0, uniforms.depth.1, Default::default()),
                textures: uniforms.textures,
                samplers: uniforms.samplers,
                color: None,
                draws: 4,
            });
        }
        Err(why) => eprintln!("native-first-person compare {label}: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_parses_a_mode_part_and_encode() {
        assert_eq!(
            parse_compare("0xe92207ad:2:30"),
            Some(Compare {
                definition: 0x07ad,
                part: 2,
                after: 30,
            })
        );
        assert_eq!(
            parse_compare("e92207ad"),
            Some(Compare {
                definition: 0x07ad,
                part: 0,
                after: 0,
            })
        );
        assert_eq!(parse_compare("07ad:0:1:extra"), None);
        assert_eq!(parse_compare("not hex"), None);
    }
}
