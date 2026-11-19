//! Mark the original light's draw interval in backend order. Both markers
//! encode without modifying an attachment. The D3D encoder can therefore
//! compare bad frame constants too: none of the light rows is a match key.

use super::{LightStage, PreparedLight};
use native_render::program::{ViewInputs, bias_signed, describe_image, pipeline_state, sampler_key};

#[derive(Default)]
pub(super) enum ProbeState {
    #[default]
    Waiting,
    Reference { handle: u32, last_layer: usize },
    Done,
}

pub(super) struct Begin {
    light: PreparedLight,
    constants: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
}

impl Begin {
    pub(super) fn new(light: PreparedLight) -> Self {
        Self { light, constants: None, clip: None }
    }
}

impl d3d::ExternalPass for Begin {
    fn confirm_issue(&self) -> bool {
        true
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.constants = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        // Compare the guest-resolution reference. ScaledBackend also sends
        // the same external pass to its optional high-resolution copy.
        if t.copy != d3d::TargetCopy::Reference {
            return;
        }
        let Some(constants) = self.constants else { return };
        let view = ViewInputs {
            constants,
            size: t.size,
            guest_size: t.guest_size,
            z_scale: t.z_scale,
            w_clip: self.clip.map(|clip| (clip.range, clip.clamp)),
        };
        let light = &self.light;
        let world = light.ready.lights.as_ref().expect("prepared light owns its textures");
        let bc = t.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let mut seen = std::collections::HashSet::new();
        let candidates = light.draws.iter().filter_map(|p| {
            if !seen.insert((p.material, p.layer, p.implementation)) {
                return None;
            }
            let d = light.draw(p);
            let program = d.program;
            let (vertex, pixel) = d.uniforms(&view);
            let (primitive, (write, compare)) = pipeline_state(Some(&program.state), d.cull);
            let textures = std::array::from_fn(|n| {
                program.dims[n]?;
                let bitmap = match program.stages[n] {
                    LightStage::Empty => return None,
                    LightStage::Bitmap { bitmap, image } => world.images.get(&(n, bitmap, image))?,
                    LightStage::Gel if light.binding.is_some() => {
                        let (w, h) = light.binding.as_ref()?.guest_size;
                        return Some(format!("linear gel target 13 {w}x{h}"));
                    }
                    LightStage::Gel => world.gels.get(&d.gel?)?,
                    stage => {
                        let image = if stage == LightStage::Normalization {
                            world.normalization.as_ref()?
                        } else {
                            world.attenuation.as_ref()?
                        };
                        return Some(format!("{stage:?} Rgba8Unorm {}x{}x{} levels {}", image.width, image.height, image.layers, 1 + image.mips.len()));
                    }
                };
                if program.signed[n].contains(&true) {
                    let mut image = bitmap.clone();
                    bias_signed(&mut image, program.signed[n]);
                    Some(describe_image(&image, bc))
                } else {
                    Some(describe_image(bitmap, bc))
                }
            });
            Some(d3d::uniform_compare::Expected {
                label: format!("light {:#010x} material {} layer {} implementation {}", light.handle, p.material, p.layer, p.implementation),
                words: program.words.clone(),
                vertex, pixel,
                options: native_render::light_look::vertex_options(),
                def: program.def.clone(),
                pixel_options: program.pixel_options,
                primitive,
                depth: (write, compare, Default::default()),
                textures,
                samplers: std::array::from_fn(|n| program.dims[n].and(program.samplers[n].as_ref()).and_then(sampler_key)),
                draws: 1,
            })
        }).collect();
        d3d::uniform_compare::begin_light(light.handle, candidates);
    }
}

pub(super) struct End(pub(super) u32);

impl d3d::ExternalPass for End {
    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        if t.copy == d3d::TargetCopy::Reference {
            d3d::uniform_compare::end_light(self.0);
        }
    }
}
