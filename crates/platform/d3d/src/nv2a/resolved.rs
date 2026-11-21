//! Pure capture of the existing NV2A translator's state for an isolated
//! native command recording. Resident views are supplied by DrawAdmission;
//! no texture upload, queue write or shared uniform ring is used here.
use super::*;

pub struct ResolvedProgram {
    pub words: Vec<u32>,
    pub pixel: PixelShaderDef,
    pub pixel_options: PixelOptions,
    pub vertex_options: VertexOptions,
    pub vertex_uniforms: VertexUniforms,
    pub pixel_uniforms: PixelUniforms,
    pub samplers: [SamplerKey; 4],
}

pub fn resolve(
    draw: &ResolvedDraw,
    present: [bool; 4],
    caps: gfx::translate::Capabilities,
    scale: u32,
) -> Result<ResolvedProgram, String> {
    let table = RenderStateTable::default();
    let KeyParts {
        key,
        def,
        signs,
        border,
        ..
    } = program_key(&table, caps, draw, present, FetchMode::Cpu)?;
    let VertexSource::Microcode(words) = &key.vertex else {
        return Err("native effects require a programmable vertex shader".into());
    };
    let words = words.to_vec();
    let render = &draw.state.render;
    let (c0, c1) = def.embedded_constants();
    let mut vu = VertexUniforms {
        surface: [draw.target.width as f32, draw.target.height as f32],
        // Same full scale as gfx's built-in shader.
        z_max: gfx::renderer::d3dz_max(draw.target.depth_format),
        shift: gfx::renderer::window_shift(1, scale),
        ..VertexUniforms::default()
    };
    match key.vertex {
        VertexSource::Microcode(_) => {
            vu.constants = *draw.shader.constants;
            vu.defaults = draw.shader.vertex_defaults;
        }
        VertexSource::Pretransformed(_) => {
            vu.constants[FF_HALF_PIXEL] = [0.5, 0.5, 0.0, 0.0];
            vu.constants[FF_ZERO] = [0.0; 4];
            // Missing diffuse is white; missing specular black with no fog.
            vu.defaults[3] = [1.0; 4];
            vu.defaults[4] = [0.0, 0.0, 0.0, 1.0];
        }
    }
    let mut pu = PixelUniforms {
        c0,
        c1,
        fog_color: argb(render.fog_color),
        fog: fog_uniform(&table, draw),
        alpha_ref: [render.alpha_ref as f32, 0.0, 0.0, 0.0],
        eye: eye_uniform(&draw.hw),
        ..PixelUniforms::default()
    };
    pu.stipple = draw.hw.stipple_pattern;
    pu.stipple_scale = [scale.max(1) as f32; 2];
    let w_buffer = key.pixel_options.w_buffer;
    if w_clipped(&key.vertex, draw) {
        vu.w_clip = Some(w_clip_range(&draw.hw, vu.z_max));
        vu.w_clamp = depth_clamp(&table, draw);
    }
    if w_buffer {
        // The polygon offset joins the stored w in the fragment stage
        // (the pipeline's depth bias would offset the rasterized z,
        // which is not what is stored); see `PreparedShader`.
        let (units, slope) = if polygon_offset(render, draw.primitive) {
            let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
            (
                finite(render.depth_bias),
                finite(render.depth_bias_slope_scale),
            )
        } else {
            (0.0, 0.0)
        };
        let integer = !matches!(
            draw.target.depth_format,
            Some(
                gfx::D3DFormat::F16
                    | gfx::D3DFormat::LinF16
                    | gfx::D3DFormat::F24S8
                    | gfx::D3DFormat::LinF24S8
            )
        );
        pu.depth = [vu.z_max, units, slope, if integer { 1.0 } else { 0.0 }];
        let [lo, hi] = vu.w_clip.unwrap_or([0.0, vu.z_max]);
        pu.depth_range = [lo, hi];
    }
    pu.lod_bias = std::array::from_fn(|n| {
        let bias = draw.state.stages[n].mip_lod_bias;
        if bias.is_finite() { bias } else { 0.0 }
    });
    pu.border = std::array::from_fn(|n| border_value(draw.state.stages[n].border_color, signs[n]));
    for n in 0..4 {
        let e = &draw.extras[n];
        pu.bump_mat[n] = e.bump_env_mat;
        pu.bump_lum[n] = [e.bump_env_lscale, e.bump_env_loffset, 0.0, 0.0];
        if let Some(info) = draw.texture_info[n]
            && info.format.layout() == gfx::format::Layout::Linear
        {
            // Linear textures are addressed in texels.
            let (w, h) = (info.width.max(1) as f32, info.height.max(1) as f32);
            pu.tex_scale[n] = [1.0 / w, 1.0 / h, 1.0, 1.0];
        }
    }
    let samplers: [SamplerKey; 4] = std::array::from_fn(|n| {
        let mut k = SamplerKey::from_stage(&draw.state.stages[n], caps);
        if border[n] != 0 {
            // The shader supplies the border; the sampler clamps.
            for (axis, mode) in [&mut k.address_u, &mut k.address_v, &mut k.address_w]
                .into_iter()
                .enumerate()
            {
                if border[n] >> axis & 1 != 0 {
                    *mode = wgpu::AddressMode::ClampToEdge;
                }
            }
            k.border = None;
        }
        k
    });
    Ok(ResolvedProgram {
        words,
        pixel: def,
        pixel_options: key.pixel_options,
        vertex_options: key.vertex_options,
        vertex_uniforms: vu,
        pixel_uniforms: pu,
        samplers,
    })
}
