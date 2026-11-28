//! The host's own lights (docs/game/forge-monitor-appearance.md, ML5):
//! sphere lights with no guest record, whose constants
//! (`render_bridge::native_lights::host`) the frame step made once the
//! view's light pass was over. Each draws its layers 12 and 13 over the
//! world's structure parts its sphere meets, in the view's target, as one
//! external pass a light, with the same resident geometry, material
//! variants and light textures a guest light's scene group uses
//! ([`Glue::scene_light_draws`]). No shadow map is made, no gel is
//! sampled, and no object (model) part is lit: a host light lights the
//! structure and its placements only.
//!
//! E3's chosen lights beyond the guest's
//! (`render_bridge::native_lights::chosen`, docs/graphics/e3-lighting.md)
//! draw the same way from the constants extraction made for them, each
//! pushed as a stage of E3 alone (`native_render::stage::Stage::lighting_only`)
//! with its materials, gel and geometry retained, so the frame graph
//! records it in an E3 frame and skips it in any other. A chosen light
//! the renderer shadows (`native_render::cone_shadows::view_shadows`)
//! comes with its shadowed constants, so its parts take the shadowed
//! implementations, and the cone's shadow term goes into the destination
//! alpha just before its pass, as retail's layer 11 would. The chosen first-person lights retail's pass did not
//! record draw over the first-person view's object parts
//! ([`first_person`]).
mod first_person;

use super::*;
use render_bridge::native_lights::chosen::{ChosenDraw, ChosenLight};
use render_bridge::native_lights::host::{HostDraw, HostLight};

/// A light the passes draw as a sphere over the structure: a host light
/// or a chosen one.
#[derive(Clone, Copy)]
pub(super) struct Sphere<'a> {
    pub origin: [f32; 3],
    pub radius: f32,
    pub constants: &'a LightConstants,
    /// The light whose cone shadow its passes read (a chosen light the
    /// renderer shadows), by handle.
    pub cone: Option<u32>,
}

impl<'a> From<&'a HostLight> for Sphere<'a> {
    fn from(l: &'a HostLight) -> Self {
        Self {
            origin: l.origin,
            radius: l.radius,
            constants: &l.constants,
            cone: None,
        }
    }
}

impl<'a> From<&'a ChosenLight> for Sphere<'a> {
    fn from(l: &'a ChosenLight) -> Self {
        Self {
            origin: l.origin,
            radius: l.radius,
            constants: &l.constants,
            cone: l.shadowed.then_some(l.handle),
        }
    }
}

/// How a sphere light's pass is issued.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Issue {
    /// At once, in every mode: a host light.
    Now,
    /// As a retained stage of E3 alone: a chosen light.
    E3Stage,
}

/// The whole target: [`intersect_scissor`] crops it to the view's.
const WHOLE: [u32; 4] = [0, 0, u32::MAX / 2, u32::MAX / 2];

/// The parts a host light draws, as `(layer, implementation, part)` in
/// the order the guest's lists draw them: layer 12's parts by their place
/// in the world's indices, then layer 13's. A part is in when its bounds
/// meet the light's sphere and its material is one the passes hold; the
/// implementation is the engine's choice for the light's flags and the
/// material's integer input 4 (`engine::light_passes::implementation`),
/// and a layer with none draws nothing.
pub(super) fn parts<'a>(
    draws: impl Iterator<Item = &'a WorldDraw>,
    materials: &HashMap<u16, LitMaterial>,
    light: Sphere<'_>,
) -> Vec<(usize, usize, WorldDraw)> {
    let c = light.constants;
    let mut met: Vec<&WorldDraw> = draws
        .filter(|d| d.bounds.meets_sphere(light.origin, light.radius))
        .collect();
    met.sort_by_key(|d| (d.first_index, d.index_count));
    let mut out = Vec::new();
    for layer in [DIFFUSE_LAYER, SPECULAR_LAYER] {
        for d in &met {
            let Some(m) = materials.get(&d.material) else {
                continue;
            };
            let int_input = m.instance.ints[4] as u16;
            if let Some(imp) = engine::light_passes::implementation(
                layer as u32,
                c.flags,
                c.definition_flags as u8,
                int_input,
            ) {
                out.push((layer, imp as usize, **d));
            }
        }
    }
    out
}

impl Glue {
    /// Counts one sphere light's answer with the others' ([`Tally`]).
    fn count_host(&self, answer: Result<(), Keep>) {
        let mut tally = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        for msg in tally.count(answer) {
            eprintln!("{msg}");
        }
    }

    /// Draws `draw`'s lights, each pushed as a stage, and returns how
    /// many it drew. A light the passes cannot
    /// draw (the world not loaded, a variant still building, no part met)
    /// is counted and drawn by nobody this frame.
    pub(super) fn host_lights(&self, mem: &mut dyn GuestMemory, draw: HostDraw<'_>) -> usize {
        let lights: Vec<Sphere<'_>> = draw.lights.iter().map(Sphere::from).collect();
        self.sphere_lights(mem, &lights, Issue::Now)
    }

    /// Draws `draw`'s chosen lights, each a stage of E3 alone, and returns
    /// how many it pushed.
    pub(super) fn chosen_lights(&self, mem: &mut dyn GuestMemory, draw: ChosenDraw<'_>) -> usize {
        let lights: Vec<Sphere<'_>> = draw.lights.iter().map(Sphere::from).collect();
        self.sphere_lights(mem, &lights, Issue::E3Stage)
    }

    fn sphere_lights(
        &self,
        mem: &mut dyn GuestMemory,
        lights: &[Sphere<'_>],
        issue: Issue,
    ) -> usize {
        let ready = match self.ready(&*mem) {
            Ok(r) => r,
            Err(why) => {
                self.count_host(Err(why));
                return 0;
            }
        };
        let mut drawn = 0;
        for &light in lights {
            let answer = self.host_light(mem, &ready, light, issue);
            drawn += usize::from(answer.is_ok());
            self.count_host(answer);
        }
        drawn
    }

    /// One sphere light's passes: its parts resolved with its constants,
    /// the whole view as the clip, and the draw pushed as a stage
    /// ([`Issue`]). A part whose material has no pass in the layer or whose
    /// program the light cannot read is left out; a variant still building keeps the light for this frame.
    fn host_light(
        &self,
        mem: &mut dyn GuestMemory,
        ready: &Arc<Ready>,
        light: Sphere<'_>,
        issue: Issue,
    ) -> Result<(), Keep> {
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let c = light.constants;
        let wanted = parts(
            world.clusters.values().map(|(_, draw)| draw),
            &world.materials,
            light,
        );
        let mut planned: Vec<Planned> = Vec::new();
        let mut pending = false;
        for part in wanted {
            match world.resolve_parts(c, [part], false) {
                Ok(done) => {
                    for next in done {
                        match planned.last_mut() {
                            Some(last)
                                if last.placement.is_none() && next.placement.is_none()
                                    && last.material == next.material
                                    && last.layer == next.layer
                                    && Arc::ptr_eq(&last.compiled, &next.compiled) =>
                            {
                                last.ranges.extend(next.ranges);
                            }
                            _ => planned.push(next),
                        }
                    }
                }
                Err(Keep::MaterialPending) => pending = true,
                Err(_) => {}
            }
        }
        if pending {
            return Err(Keep::MaterialPending);
        }
        if planned.is_empty() {
            return Err(Keep::Unsupported(
                "a sphere light meets no part with a native light pass",
            ));
        }
        let prepared = self.prepare_planned(&*mem, Arc::clone(ready), *c, planned, None, Some(WHOLE))?;
        if issue == Issue::E3Stage {
            return self.e3_stage(&*mem, ready, prepared, light.cone);
        }
        push(&*mem, frame_types::Slot::Opaque, Box::new(RunPass::new(prepared)));
        Ok(())
    }

    /// Pushes `prepared` as a stage of E3 alone, its materials, gel and
    /// geometry retained; the frame graph records it in an E3 frame.
    /// `cone`: the light the renderer shadows, whose cone shadow the pass
    /// reads.
    fn e3_stage(
        &self,
        mem: &dyn GuestMemory,
        ready: &Arc<Ready>,
        prepared: PreparedLight,
        cone: Option<u32>,
    ) -> Result<(), Keep> {
        if let Some(cone) = cone {
            unimplemented!("lights: cone shadow {cone:#x} before the chosen light's pass");
        }
        let pass = RunPass {
            retained: Some(RetainedLight::of(ready, &prepared)?),
            light: prepared,
            lighting_only: Some(native_render::lighting::Lighting::E3),
            gel: None,
        };
        push(mem, frame_types::Slot::Opaque, Box::new(pass));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::world::Aabb;

    fn draw(first: u32, material: u16, at: f32) -> WorldDraw {
        WorldDraw {
            owner: Owner::Cluster(0),
            part: 0,
            material,
            lightmap: None,
            first_index: first,
            index_count: 3,
            bounds: Aabb {
                min: [at, 0.0, 0.0],
                max: [at + 1.0, 1.0, 1.0],
            },
            batch: 0,
            inputs: 1,
        }
    }

    fn material(int_input: i32) -> LitMaterial {
        let mut instance = EngineInputs::default();
        instance.ints[4] = int_input;
        LitMaterial {
            shader: TagId(1),
            instance,
        }
    }

    fn light(flags: u32) -> HostLight {
        HostLight {
            seat: 1,
            origin: [0.0, 0.0, 0.0],
            radius: 2.0,
            constants: LightConstants {
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
            },
        }
    }

    #[test]
    fn a_sphere_light_draws_layer_12_over_the_parts_it_meets_in_world_order() {
        let draws = [draw(30, 0, 1.0), draw(10, 0, 0.5), draw(20, 0, 50.0)];
        let materials = HashMap::from([(0, material(0))]);
        // Context zero (bit 0): the sphere implementation, one.
        let got = parts(draws.iter(), &materials, Sphere::from(&light(1)));
        assert_eq!(
            got.iter()
                .map(|(l, i, d)| (*l, *i, d.first_index))
                .collect::<Vec<_>>(),
            [(12, 1, 10), (12, 1, 30)],
            "the far part is left out; the others by their place in the indices"
        );
    }

    #[test]
    fn layer_13_draws_only_for_a_specular_light_over_a_material_that_asks() {
        let draws = [draw(10, 0, 0.0), draw(20, 1, 0.0)];
        let materials = HashMap::from([(0, material(1)), (1, material(0))]);
        // Bits 0 and 1: context zero with a colour B above the threshold.
        let got = parts(draws.iter(), &materials, Sphere::from(&light(3)));
        assert_eq!(
            got.iter()
                .map(|(l, i, d)| (*l, *i, d.first_index))
                .collect::<Vec<_>>(),
            [(12, 1, 10), (12, 1, 20), (13, 0, 10)],
            "layer 13 only for the material whose integer input 4 is set"
        );
        let plain = parts(draws.iter(), &materials, Sphere::from(&light(1)));
        assert!(
            plain.iter().all(|(l, ..)| *l == 12),
            "no colour B, no layer 13"
        );
    }

    #[test]
    fn a_part_with_a_material_the_passes_do_not_hold_is_left_out() {
        let draws = [draw(10, 7, 0.0), draw(20, u16::MAX, 0.0)];
        let materials = HashMap::from([(0, material(0))]);
        assert!(parts(draws.iter(), &materials, Sphere::from(&light(1))).is_empty());
    }
}
