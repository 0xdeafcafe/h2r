//! Device host for the native first-person arms and weapon. Each accepted
//! `0x0003fdf0` layer-3 call becomes one external pass at that exact point;
//! the device's current constants are the first-person view, its depth clip
//! is preserved, and the loaded depth/stencil attachment keeps the scene's
//! stencil (disabled by the first-person flag) intact.

use std::sync::{Arc, Mutex, MutexGuard};

use engine::memory::GuestMemory;
use feature::native_first_person::Host;
use feature::native_objects::NativeSet;
use native_render::object_program_gpu::{LitObjects, frame_parts};
use native_render::object_streams::StreamFormats;
use native_render::program::ViewInputs;

use crate::native_first_person_load::{self, MapState, Models};
use crate::native_world_glue::{ReadMemory, inherited_cull, palettes_hold, stage_palettes};

/// Hook memory as the stream-format reader expects it.
struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The installed host: current map generation and inherited cull.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
    generation: Mutex<u64>,
    cull: Mutex<Option<native_render::program::Cull>>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn native(&self, mem: &dyn GuestMemory) -> Option<Arc<NativeSet>> {
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() {
            return None;
        }
        let (clip, cull) = {
            let device = self.device.lock().ok()?;
            let clip = device.depth_clip(&ReadMemory(mem), 1.0);
            let cull = device.cull_state(&ReadMemory(mem)).map(inherited_cull);
            (clip, cull)
        };
        if clip.w_buffered != Some(true) || cull.is_none() {
            return None;
        }
        *self.cull.lock().unwrap_or_else(|e| e.into_inner()) = cull;

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
        Some(set)
    }

    fn want(&self, definition: u32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = state.as_mut() else { return };
        if state.tried.insert(definition as u16)
            && let Some(worker) = &state.worker
        {
            let _ = worker.send(definition);
        }
    }

    fn draw(&self, objects: Vec<native_render::objects::ObjectFrame>) -> bool {
        let Some(models) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|state| Arc::clone(&state.models))
        else {
            return false;
        };
        let cull = *self.cull.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(mut device) = self.device.lock() else {
            return false;
        };
        if !device.can_external_pass() {
            return false;
        }
        device.request_external_pass(Box::new(FirstPersonPass {
            models,
            objects,
            view: None,
            w_clip: None,
            cull,
            valid: true,
        }));
        true
    }
}

/// Installs the host beside the other native render stages.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_first_person::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
        generation: Mutex::new(0),
        cull: Mutex::new(None),
    }));
}

struct FirstPersonPass {
    models: Arc<Models>,
    objects: Vec<native_render::objects::ObjectFrame>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    cull: Option<native_render::program::Cull>,
    valid: bool,
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
            w_clip: self.w_clip,
        }
    }
}

impl d3d::ExternalPass for FirstPersonPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.valid &= clip.w_buffered == Some(true);
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn encode(&mut self, target: d3d::ExternalTarget<'_>) {
        if !self.valid {
            eprintln!("native-first-person: issue refused a non-W-buffered or unknown depth mode");
            return;
        }
        let Some(constants) = self.view else { return };
        let mut guard = self.upload(&target);
        let Some(lit) = guard.as_mut() else { return };
        lit.set_inherited_cull(self.cull);
        let objects: Vec<(u32, &native_render::objects::ObjectFrame)> = self
            .objects
            .iter()
            .filter(|object| lit.has(object.definition))
            .map(|object| (object.definition, object))
            .collect();
        let compared = compare();
        let (shadowed, native): (Vec<_>, Vec<_>) = objects.into_iter().partition(|(_, object)| {
            compared.is_some_and(|compare| compare.definition == object.definition as u16)
        });
        let view = self.view_inputs(constants, &target);
        if let (Some(compare), Some(&(_, object))) = (compared, shadowed.first()) {
            arm_compare(compare, lit, object, &view);
        }
        if native.is_empty() {
            return;
        }
        let expected = expected_parts(&self.models, &native);
        let drawn = lit.encode(
            target.device,
            target.queue,
            &mut *target.encoder,
            target.color,
            target.color_format,
            target.depth,
            target.depth_format,
            target.size,
            &view,
            &native,
            None,
        );
        if drawn != expected {
            eprintln!(
                "native-first-person: encoded {drawn} of {expected} prepared parts; mixed D3D/native output needs investigation"
            );
        }
    }
}

/// Number of parts the prepared object frames select in the CPU metadata.
fn expected_parts(
    models: &Models,
    objects: &[(u32, &native_render::objects::ObjectFrame)],
) -> usize {
    let parts = models.parts.lock().unwrap_or_else(|e| e.into_inner());
    objects
        .iter()
        .map(|(definition, frame)| {
            parts
                .get(definition)
                .map(|parts| frame_parts(frame, parts).len())
                .unwrap_or(0)
        })
        .sum()
}

/// `H2_NATIVE_FIRST_PERSON_COMPARE=definition:part[:after]`: keep that
/// definition on D3D and arm its native program/constants against the real
/// first-person draw on the selected external pass.
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
                "native-first-person: comparing {compare:?} with D3D (its objects stay on D3D)"
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
    compare: Compare,
    lit: &LitObjects,
    object: &native_render::objects::ObjectFrame,
    view: &ViewInputs,
) {
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) != compare.after {
        return;
    }
    let label = format!(
        "first-person object {:#010x} definition {:#010x} part {}",
        object.handle, object.definition, compare.part
    );
    match lit.part_uniforms(object.definition, object, compare.part, view, None) {
        Ok(uniforms) => {
            eprintln!(
                "native-first-person compare {label}: implementation {}, {} program words, rows c[50..] {:?}, depth {:?}, primitive {:?}, textures {:?}",
                uniforms.implementation,
                uniforms.words.len(),
                &uniforms.vertex.constants[50..53],
                uniforms.depth,
                uniforms.primitive,
                uniforms.textures
            );
            d3d::uniform_compare::arm(d3d::uniform_compare::Expected {
                label,
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
    fn comparison_parses_a_definition_part_and_pass() {
        assert_eq!(
            parse_compare("0xe92207ad:2:30"),
            Some(Compare {
                definition: 0x07ad,
                part: 2,
                after: 30
            })
        );
        assert_eq!(
            parse_compare("e92207ad"),
            Some(Compare {
                definition: 0x07ad,
                part: 0,
                after: 0
            })
        );
        assert_eq!(parse_compare("07ad:0:1:extra"), None);
        assert_eq!(parse_compare("not hex"), None);
    }
}
