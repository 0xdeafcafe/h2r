//! HUD tag combiners translated from the widget draw's bound definition.

use super::{PixelUniforms, overlay_pipeline, validated_resource};

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct HudPipelineKey {
    pub def: shaders::psh::PixelShaderDef,
    pub glyph: u8,
    pub alpha_kill: u8,
    pub alpha_func: Option<u32>,
    pub color_write: u32,
    pub blend: frame_scene::OverlayBlend,
    pub format: wgpu::TextureFormat,
}

pub(super) struct HudPipeline {
    pub(super) pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroupLayout,
    stages: wgpu::BindGroupLayout,
    sampled: [bool; 4],
}

// The overlay supplies screen positions and the widget's sprite coordinates.
const HUD_VERTEX: &str = r#"
@vertex fn vs_main(@location(0) position: vec2f, @location(1) uv: vec2f,
                   @location(2) color: vec4f, @location(3) uv1: vec2f,
                   @location(4) uv2: vec2f, @location(5) uv3: vec2f) -> XboxVaryings {
    var out: XboxVaryings;
    out.position = vec4f(position, 0.0, 1.0);
    out.d0 = color;
    out.t0 = vec4f(uv, 0.0, 1.0);
    out.t1 = vec4f(uv1, 0.0, 1.0);
    out.t2 = vec4f(uv2, 0.0, 1.0);
    out.t3 = vec4f(uv3, 0.0, 1.0);
    out.fog = 1.0;
    return out;
}
"#;

fn hud_stage(def: &shaders::psh::PixelShaderDef) -> Option<[bool; 4]> {
    use shaders::psh::{Input, Reg, TexMode};
    let program = def.decode();
    // The producer carries the retail vertex program's position, UV and diffuse
    // outputs. Specular and fog are absent from OverlayVertex.
    let unavailable = |input: Input| matches!(input.reg, Reg::V1 | Reg::V1R0Sum | Reg::Fog);
    if program.stages.iter().any(|s| {
        s.rgb
            .inputs
            .into_iter()
            .chain(s.alpha.inputs)
            .any(unavailable)
    }) || program.final_combiner.is_some_and(|f| {
        [f.a, f.b, f.c, f.d, f.e, f.f, f.g]
            .into_iter()
            .any(unavailable)
    }) {
        return None;
    }
    let mut sampled = [false; 4];
    for (index, mode) in program.texture_modes.into_iter().enumerate() {
        match mode {
            TexMode::None => {}
            TexMode::Project2d => sampled[index] = true,
            _ => return None,
        }
    }
    Some(sampled)
}

/// Whether the vertex data and texture stages can supply this program.
/// Preparing the GPU pass also checks the pixel translation and device.
pub fn hud_shader_supported(words: &[u32; 60]) -> bool {
    let def = shaders::psh::PixelShaderDef::from_words(words);
    hud_stage(&def).is_some()
}

/// The colour a glyph shader's captured state gives a fully covered glyph
/// texel under white diffuse: the flat colour of a text run drawn in that
/// shader's place. The combiners run on the CPU (`shaders::psh::interp`)
/// over the definition's own constants, which the draw snapshot has
/// filled. `None` when the overlay does not support the shader or it
/// draws nothing for a covered texel.
pub fn glyph_colour(words: &[u32; 60]) -> Option<[f32; 4]> {
    use shaders::psh::interp::{Inputs, Sampler, Uniforms, Vec4, run};
    use shaders::psh::{PixelOptions, TextureDim};

    // The glyph pages store coverage in alpha over white.
    struct Covered;
    impl Sampler for Covered {
        fn sample(&self, _: usize, _: TextureDim, _: [f32; 3]) -> Vec4 {
            [1.0; 4]
        }
    }
    let def = shaders::psh::PixelShaderDef::from_words(words);
    hud_stage(&def)?;
    let (c0, c1) = def.embedded_constants();
    let uniforms = Uniforms { c0, c1, ..Default::default() };
    let inputs = Inputs { d0: [1.0; 4], ..Default::default() };
    let out = run(&def, &PixelOptions::default(), &inputs, &uniforms, &Covered)?;
    (out[3] > 0.0).then_some(out)
}

impl HudPipeline {
    pub(super) fn new(
        device: &wgpu::Device,
        key: &HudPipelineKey,
        blend: Option<wgpu::BlendState>,
    ) -> Option<Self> {
        let sampled = hud_stage(&key.def)?;
        let alpha_test = match key.alpha_func {
            Some(func) => Some(gfx::guest_program::alpha_func(gfx::state::CompareFunc::from_xbox(
                func,
            )?)),
            None => None,
        };
        let pixel = shaders::psh::translate(
            &key.def,
            &shaders::psh::PixelOptions {
                glyph: key.glyph,
                alpha_test,
                alpha_kill: std::array::from_fn(|i| key.alpha_kill & (1 << i) != 0),
                ..Default::default()
            },
        );
        if !pixel.warnings.is_empty() {
            return None;
        }
        let uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HUD pixel constants"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            }],
        });
        let entries: Vec<_> = (0..4u32)
            .filter(|&n| sampled[n as usize])
            .flat_map(|n| {
                [
                    wgpu::BindGroupLayoutEntry {
                        binding: 2 * n,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        count: None,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2 * n + 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        count: None,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    },
                ]
            })
            .collect();
        let stages = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HUD widget stage"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HUD program"),
            bind_group_layouts: &[Some(&uniforms), Some(&stages)],
            immediate_size: 0,
        });
        let module = validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("HUD tag program"),
                source: wgpu::ShaderSource::Wgsl((pixel.module() + HUD_VERTEX).into()),
            })
        })?;
        let pipeline = validated_resource(device, || {
            overlay_pipeline(
                device,
                &module,
                &layout,
                "fs_main",
                key.color_write,
                key.format,
                blend,
            )
        })?;
        Some(Self {
            pipeline,
            uniforms,
            stages,
            sampled,
        })
    }

    pub(super) fn bind(
        &self,
        device: &wgpu::Device,
        textures: &dyn super::OverlayTextures,
        batch: &crate::overlay::OverlayBatch,
        fallback_sampler: &wgpu::Sampler,
        pixel: &PixelUniforms,
    ) -> Option<(wgpu::BindGroup, wgpu::BindGroup)> {
        use wgpu::util::DeviceExt;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HUD constants"),
            contents: &pixel.to_bytes(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let constants = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HUD constants"),
            layout: &self.uniforms,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: buffer.as_entire_binding(),
            }],
        });
        let mut entries = Vec::new();
        for (n, sampled) in self.sampled.into_iter().enumerate() {
            if !sampled {
                continue;
            }
            let texture = batch.shader_stages[n].unwrap_or(batch.material.texture);
            let view = textures.stage(n, texture)?;
            if view.texture().dimension() != wgpu::TextureDimension::D2
                || !gfx::program::filterable(view.texture().format(), device.features())
            {
                return None;
            }
            let sampler = textures
                .stage_sampler(n, texture)
                .unwrap_or(fallback_sampler);
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let stages = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HUD sprite"),
            layout: &self.stages,
            entries: &entries,
        });
        Some((constants, stages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{NoGlyphs, OverlayPlan};
    use crate::ui_gpu::{OverlayGpu, OverlayTextures};
    use frame_scene::{
        ClipRect, Overlay, OverlayBlend, OverlayMaterial, OverlayTexture, OverlayVertex, Program,
        Sampler,
    };
    use shaders::psh::bits::*;

    #[test]
    fn a_meter_compares_its_fill_to_the_blue_ramp() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let size = wgpu::Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        };
        let texture = |usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("synthetic HUD ramp"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            })
        };
        let ramp = texture(wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        ctx.queue.write_texture(
            ramp.as_image_copy(),
            &[0, 0, 33, 255, 0, 0, 191, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            size,
        );
        let ink = texture(wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        ctx.queue.write_texture(
            ink.as_image_copy(),
            &[29, 0, 0, 255, 217, 0, 0, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            size,
        );
        struct Textures([wgpu::TextureView; 2]);
        impl OverlayTextures for Textures {
            fn view(&self, id: OverlayTexture) -> Option<&wgpu::TextureView> {
                match id {
                    OverlayTexture::Bound { header: 82 } => Some(&self.0[0]),
                    OverlayTexture::Bound { header: 91 } => Some(&self.0[1]),
                    _ => None,
                }
            }
        }
        let textures = Textures([ramp, ink].map(|t| t.create_view(&Default::default())));
        let target =
            texture(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&Default::default());
        let mut gpu = OverlayGpu::new(&ctx.device, &ctx.queue);
        let material = OverlayMaterial {
            program: Program::Shader { tag: 11 },
            blend: OverlayBlend::Off,
            texture: OverlayTexture::Bitmap { tag: 13, image: 0 },
            sampler: Sampler {
                point: true,
                wrap: false,
            },
        };
        let mut overlay = Overlay::new([2, 1]);
        let corners = [[0., 0.], [2., 0.], [2., 1.], [0., 1.]].map(|[x, y]| OverlayVertex {
            position: [x, y],
            uv: [x / 2., y],
            color: [1.; 4],
        });
        overlay
            .push_quad(material, ClipRect::full([2, 1]), corners)
            .unwrap();
        overlay.items[0].shader_stages = [
            Some(OverlayTexture::Bound { header: 82 }),
            Some(OverlayTexture::Bound { header: 91 }),
            None,
            None,
        ];
        overlay.items[0].stage_uv = Some(Box::new(
            corners.map(|v| [v.uv, [1.0 - v.uv[0], v.uv[1]], [0.; 2], [0.; 2]]),
        ));
        // r0.a = (fill + (1 - ramp.blue)) / 2. Its MSB selects black
        // below the ramp threshold and the mirrored ink texture at or above the threshold.
        let mut words = [0; 60];
        words[0] = inputs(REG_C0 | CHAN_ALPHA, ONE, REG_T0 | MAP_UNSIGNED_INVERT, ONE);
        words[26] = outputs(0, 0, REG_R0, OUT_SHIFTRIGHT_1);
        words[35] = inputs(0, 0, REG_T1, ONE);
        words[46] = outputs(0, 0, REG_R0, OUT_AB_CD_MUX);
        words[53] = 2 | COUNT_MUX_MSB;
        words[54] = texture_modes([TEX_PROJECT2D, TEX_PROJECT2D, 0, 0]);
        words[8] = inputs(0, 0, 0, REG_R0);
        for (fill, expected) in [(0, [0, 0]), (127, [217, 0]), (255, [217, 29])] {
            words[10] = fill << 24;
            overlay.items[0].hud_input[0] = fill as f32 / 255.;
            overlay.items[0].shader_words = Some(words.into());
            let plan = OverlayPlan::new(&overlay, [2, 1], &NoGlyphs);
            let (encoded, stats) = gpu.prepare(&ctx.device, &view, &plan, &textures).unwrap();
            assert_eq!(stats.batches, 1);
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            (encoded.commands)(&mut encoder);
            ctx.queue.submit([encoder.finish()]);
            let pixels =
                gfx::readback::read_texture(&ctx, &target, wgpu::TextureAspect::All, 4, (2, 1));
            assert_eq!([pixels[0], pixels[4]], expected);
        }
        assert_eq!(gpu.hud.len(), 1, "fill changes only the constants");
    }

    /// A glyph shader's colour is what its combiners give a covered texel:
    /// here the final combiner copies the final constant, so the colour is
    /// the constant; an unsupported or invisible shader gives none.
    #[test]
    fn a_glyph_shaders_colour_is_its_combiners_result_for_a_covered_texel() {
        let mut words = [0u32; 60];
        words[8] = inputs(0, 0, 0, REG_C0);
        words[9] = final_efg(0, 0, REG_C0 | CHAN_ALPHA, 0);
        words[43] = 0x83_37_42_a8;
        let colour = glyph_colour(&words).unwrap();
        let want = [0x37u8, 0x42, 0xa8, 0x83].map(|c| f32::from(c) / 255.0);
        for (got, want) in colour.iter().zip(want) {
            assert!((got - want).abs() < 1e-5, "{colour:?} != {want:?}");
        }
        // The same shader with a transparent constant draws nothing.
        words[43] = 0x00_37_42_a8;
        assert_eq!(glyph_colour(&words), None);
        // A program reading the specular register is not one the overlay draws.
        words[8] = inputs(0, 0, 0, REG_V1);
        words[43] = 0x83_37_42_a8;
        assert_eq!(glyph_colour(&words), None);
    }
}
