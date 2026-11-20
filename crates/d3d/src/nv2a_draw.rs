//! Shared shader translation inputs for ordinary draws and native indexed
//! passes that take over an already resolved draw.

use super::*;
use gfx::translate::Capabilities;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NativeDrawKey(ProgramKey);

/// GPU-independent shader inputs for one copy of a resolved draw.
pub struct NativeDrawProgram {
    pub key: NativeDrawKey,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub samplers: [SamplerKey; 4],
    pub writes_depth: bool,
}

/// The linked shader and the dimensions of the stages it really samples.
pub struct NativeDrawShader {
    pub source: String,
    pub dims: StageDims,
    pub discards: bool,
}

impl NativeDrawKey {
    pub fn translate(&self) -> Result<NativeDrawShader, String> {
        let linked = translate(&self.0)?;
        Ok(NativeDrawShader {
            source: linked.source,
            dims: linked.dims,
            discards: linked.discards,
        })
    }
}

impl NativeDrawProgram {
    pub fn resolve(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        fetch: FetchMode,
        scale: u32,
    ) -> Result<Self, String> {
        static TABLE: std::sync::LazyLock<RenderStateTable> =
            std::sync::LazyLock::new(RenderStateTable::default);
        let sampled = crate::device::sampled_stages(&draw.shader, &draw.raw, &draw.state);
        if (0..4).any(|n| sampled[n] && (!present[n] || draw.texture_info[n].is_none())) {
            return Err("a sampled stage has no resolved texture view".into());
        }
        let KeyParts {
            key,
            def,
            signs,
            border,
            ..
        } = program_key(&TABLE, caps, draw, present, fetch)?;
        if !matches!(key.vertex, VertexSource::Microcode(_)) {
            return Err("requires a vertex program".into());
        }
        if key.pixel_options.shadow.iter().any(Option::is_some) || draw.glyph_cache != 0 {
            return Err("requires a backend-specific texture path".into());
        }
        let (vertex, pixel) = uniforms(&TABLE, draw, &key, &def, signs, scale);
        Ok(Self {
            writes_depth: key.pixel_options.w_buffer,
            key: NativeDrawKey(key),
            vertex,
            pixel,
            samplers: samplers(draw, border, caps),
        })
    }
}

pub(super) fn uniforms(
    table: &RenderStateTable,
    draw: &ResolvedDraw,
    key: &ProgramKey,
    def: &PixelShaderDef,
    signs: [u8; 4],
    scale: u32,
) -> (VertexUniforms, PixelUniforms) {
    let (c0, c1) = def.embedded_constants();
    let render = &draw.state.render;
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
        fog: fog_uniform(table, draw),
        alpha_ref: [render.alpha_ref as f32, 0.0, 0.0, 0.0],
        eye: eye_uniform(&draw.hw),
        ..PixelUniforms::default()
    };
    pu.stipple = draw.hw.stipple_pattern;
    let w_buffer = key.pixel_options.w_buffer;
    if w_clipped(&key.vertex, draw) {
        vu.w_clip = Some(w_clip_range(&draw.hw, vu.z_max));
        vu.w_clamp = depth_clamp(table, draw);
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
        pu.depth_range = [lo, hi, 0.0, 0.0];
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
    (vu, pu)
}

pub(super) fn samplers(
    draw: &ResolvedDraw,
    border: [u8; 4],
    caps: Capabilities,
) -> [SamplerKey; 4] {
    std::array::from_fn(|n| {
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
    })
}
