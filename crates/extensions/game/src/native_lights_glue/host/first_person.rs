//! E3's chosen first-person lights the guest did not record
//! (`render_bridge::native_lights::chosen::first_person`,
//! docs/graphics/e3-lighting.md, "The flashlight and the first-person
//! pass"). Retail's pass (`0x000404b0`) draws only its recorded slots'
//! groups; a light with no slot has none. Each such light draws its layers
//! 12 and 13 over the first-person view's object parts, posed by their
//! node rows, from its constants and each part's material: the part's
//! model section from the map's model parts ([`models`]), its pass
//! resolved as a gathered object entry's ([`LightsWorld::resolve_model`]),
//! the implementation the engine picks for the light's flags and the
//! material's integer input 4. Each light is one scene stage of E3 alone
//! ([`Glue::e3_stage`]), with the whole view as its clip.
use super::*;
use frame_scene::{InstanceKey, LayerOwner, LightBinding, LightPose, LightStream};
use native_render::light_models::LightPart;
use render_bridge::native_lights::chosen::first_person::{FirstPersonDraw, FirstPersonPart};

/// The passes a light draws over `parts`, as `(layer, implementation,
/// part)` in the order retail's lists draw them: layer 12 over every part
/// in order, then layer 13. A part whose shader `int_input` does not know
/// (its material is not among the models') is left out; the
/// implementation is the engine's for the light's flags and the
/// material's integer input 4, and a layer with none draws nothing.
pub(super) fn wanted(
    parts: &[FirstPersonPart],
    int_input: impl Fn(u32) -> Option<u16>,
    c: &LightConstants,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for layer in [DIFFUSE_LAYER, SPECULAR_LAYER] {
        for (k, part) in parts.iter().enumerate() {
            let Some(int_input) = int_input(part.shader) else {
                continue;
            };
            if let Some(imp) = engine::light_passes::implementation(
                layer as u32,
                c.flags,
                c.definition_flags as u8,
                int_input,
            ) {
                out.push((layer, imp as usize, k));
            }
        }
    }
    out
}

/// `part` as a gathered object entry of its held part `held`, so it
/// resolves and draws as one ([`LightsWorld::resolve_model`]): its model,
/// section and part, the held part's raw range and topology, the
/// part's classification, its shader and the implementation chosen
/// for the light. The owner and slot are not read there.
pub(super) fn entry(
    part: &FirstPersonPart,
    held: &LightPart,
    vertex_class: u16,
    implementation: u8,
) -> LightEntry {
    LightEntry {
        slot: 0,
        record: 0,
        owner: LayerOwner::Object(InstanceKey(0)),
        model: part.model,
        stream: LightStream {
            section: part.section,
            part: part.part,
            vertex_class,
            first_index: held.first_index,
            index_count: held.index_count,
            topology: held.topology,
        },
        pose: LightPose::Rows(0..part.rows.len() as u32),
        binding: LightBinding {
            shader: part.shader,
            selector: 0,
            variant: 0,
            implementation,
        },
        state: Default::default(),
        subparts: None,
    }
}

/// `part`'s entry for `implementation`, when the map's model parts hold
/// its part with one classification.
fn model_entry(
    lights: &models::ModelLights,
    part: &FirstPersonPart,
    implementation: u8,
) -> Option<LightEntry> {
    let section = lights
        .models
        .models
        .get(&part.model)?
        .sections
        .get(&part.section)?;
    let held = section.parts.get(&part.part)?;
    Some(entry(part, held, held.vertex_class?, implementation))
}

impl Glue {
    /// Draws `draw`'s lights over its parts, each a scene stage of E3
    /// alone at the view's target, and returns how many it queued. A
    /// light the passes cannot draw (the world or the models not loaded,
    /// a variant still building, no part with a light pass) is counted
    /// and drawn by nobody this frame.
    pub(in super::super) fn first_person_lights(
        &self,
        mem: &mut dyn GuestMemory,
        draw: FirstPersonDraw<'_>,
    ) -> usize {
        let held = self.ready(&*mem).and_then(|ready| {
            let models = self.model_lights(&*mem, &ready)?;
            Ok((ready, models))
        });
        let (ready, models) = match held {
            Ok(held) => held,
            Err(why) => {
                for _ in draw.lights {
                    self.count_host(Err(why.clone()));
                }
                return 0;
            }
        };
        let mut drawn = 0;
        for light in draw.lights {
            let answer = self.first_person_light(mem, &ready, &models, draw.parts, light);
            drawn += usize::from(answer.is_ok());
            self.count_host(answer);
        }
        drawn
    }

    /// One light's passes over the first-person parts: each part's pass
    /// resolved with the light's constants, posed by its rows. A part
    /// whose section is not held or whose material has no pass in the
    /// layer is left out; a variant still building keeps the light for
    /// this frame.
    fn first_person_light(
        &self,
        mem: &mut dyn GuestMemory,
        ready: &Arc<Ready>,
        lights: &Arc<models::ModelLights>,
        parts: &[FirstPersonPart],
        light: &ChosenLight,
    ) -> Result<(), Keep> {
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let c = &light.constants;
        let int_input = |shader: u32| {
            lights
                .instances
                .get(&shader)
                .map(|(_, instance)| instance.ints[4] as u16)
        };
        let mut planned: Vec<Planned> = Vec::new();
        let mut pending = false;
        for (layer, imp, k) in wanted(parts, int_input, c) {
            let part = &parts[k];
            let Some(entry) = u8::try_from(imp)
                .ok()
                .and_then(|imp| model_entry(lights, part, imp))
            else {
                continue;
            };
            match world.resolve_model(c, lights, layer, &entry, &part.rows, None, false) {
                Ok(next) => planned.extend(next),
                Err(Keep::MaterialPending) => pending = true,
                Err(_) => {}
            }
        }
        if pending {
            return Err(Keep::MaterialPending);
        }
        if planned.is_empty() {
            return Err(Keep::Unsupported(
                "a first-person light meets no part with a native light pass",
            ));
        }
        let prepared = self.prepare_planned(&*mem, Arc::clone(ready), *c, planned, None, Some(WHOLE))?;
        self.e3_stage(&*mem, ready, prepared, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::light_passes::flag;
    use frame_scene::Topology;
    use native_render::light_models::DeviceBuffer;
    use native_render::light_models::{LightModel, LightModels, LightSection};

    fn constants(flags: u32) -> LightConstants {
        LightConstants {
            frame: [0; 36],
            flags,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: false,
        }
    }

    fn part(part: u8, shader: u32) -> FirstPersonPart {
        FirstPersonPart {
            model: 0x00f0_0001,
            section: 3,
            part,
            shader,
            rows: vec![[0.0; 4]; 6],
        }
    }

    /// Layer 12 over every part with a known material, in order, then
    /// layer 13 for a specular sphere over the material that asks.
    #[test]
    fn a_first_person_light_draws_layer_12_over_its_parts_then_13() {
        let parts = [part(0, 10), part(1, 11), part(2, 99)];
        let int_input = |shader: u32| match shader {
            10 => Some(1),
            11 => Some(0),
            _ => None,
        };
        let c = constants(flag::CONTEXT_ZERO | flag::SPECULAR);
        assert_eq!(
            wanted(&parts, int_input, &c),
            [(12, 1, 0), (12, 1, 1), (13, 0, 0)],
            "the unknown material is left out; layer 13 only where input 4 is set"
        );
        let diffuse = wanted(&parts, int_input, &constants(flag::CONTEXT_ZERO));
        assert!(
            diffuse.iter().all(|(l, ..)| *l == 12),
            "no colour B, no layer 13"
        );
    }

    /// A first-person part made an entry finds the held part it was made
    /// from, as a gathered object entry finds its own.
    #[test]
    fn a_first_person_entry_finds_its_held_part() {
        let held = LightPart {
            vertex_class: Some(1),
            first_index: 30,
            index_count: 12,
            topology: Topology::Strip,
            indices: 0..30,
            as_list: 30..42,
            strip_map: None,
            list_map: Default::default(),
        };
        let section = LightSection {
            decompression: [[0.0; 4]; 3],
            parts: HashMap::from([(2, held.clone())]),
            indices: DeviceBuffer::new(Vec::new(), wgpu::BufferUsages::INDEX, "test"),
            buffers: Vec::new(),
        };
        let mut model = LightModel::default();
        model.sections.insert(3, section);
        let mut models = LightModels::default();
        models.models.insert(0x00f0_0001, model);
        let e = entry(&part(2, 10), &held, 1, 4);
        assert_eq!(
            (
                e.binding.shader,
                e.binding.implementation,
                e.stream.vertex_class
            ),
            (10, 4, 1)
        );
        assert_eq!(e.pose, LightPose::Rows(0..6));
        let (_, found) = models.part(&e).expect("the held part");
        assert_eq!(found, std::slice::from_ref(&held.indices));
        let other = entry(&part(1, 10), &held, 1, 4);
        assert!(models.part(&other).is_none(), "a part the section lacks");
    }
}
