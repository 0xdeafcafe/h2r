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

/// A UI program that still needs the ordinary draw path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiProgramError {
    Shader,
    Texture,
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

    /// A UI producer may use pretransformed vertices and the display-only
    /// glyph filter. Use the same shader key and uniforms as an ordinary draw.
    pub fn resolve_ui(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        fetch: FetchMode,
        scale: u32,
    ) -> Result<Self, UiProgramError> {
        static TABLE: std::sync::LazyLock<RenderStateTable> =
            std::sync::LazyLock::new(RenderStateTable::default);
        if hot_render::global().active() {
            return Err(UiProgramError::Shader);
        }
        let sampled = crate::device::sampled_stages(&draw.shader, &draw.raw, &draw.state);
        if (0..4).any(|n| sampled[n] && (!present[n] || draw.texture_info[n].is_none())) {
            return Err(UiProgramError::Texture);
        }
        // The ordinary backend uses a different vertex and fragment program
        // for its built-in diffuse fast path.
        if matches!(draw.shader.vertex, VertexProcessing::FixedFunction { .. })
            && gfx::Renderer::builtin_supports(&draw.layout)
            && builtin_equivalent(draw, present.into_iter().any(|bound| bound))
        {
            return Err(UiProgramError::Shader);
        }
        let KeyParts { mut key, def, signs, border, .. } =
            program_key(&TABLE, caps, draw, present, fetch)
                .map_err(|_| UiProgramError::Shader)?;
        if key.pixel_options.shadow.iter().any(Option::is_some) {
            return Err(UiProgramError::Texture);
        }
        if scale > 1 {
            key.pixel_options.glyph = draw.glyph_cache & 0xF;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{PixelRect, TargetInfo, Viewport};
    use crate::shader::ShaderInputs;
    use crate::state::StateWords;
    use gfx::format::D3DFormat;
    use gfx::state::{DepthBufferMode, FillMode, StencilOp};
    use gfx::vertex::{PrimitiveType, VertexLayout, fvf};
    use std::sync::Arc;

    fn ui_draw() -> ResolvedDraw {
        let raw = StateWords::defaults(false);
        let mut state = raw.translate(&RenderStateTable::default());
        let render = &mut state.render;
        render.depth_mode = DepthBufferMode::Z;
        render.depth_func = CompareFunc::LessEqual;
        render.depth_write = true;
        render.stencil.enable = true;
        render.stencil.pass = StencilOp::Replace;
        render.stencil.reference = 3;
        render.alpha_test_enable = true;
        render.alpha_func = CompareFunc::Greater;
        render.alpha_ref = 91;
        render.shade_mode = ShadeMode::Flat;
        render.fog_enable = true;
        render.fog_color = 0xff_00_00_ff;
        let fvf = fvf::XYZRHW | fvf::DIFFUSE;
        ResolvedDraw {
            entry: "synthetic UI fan",
            primitive: PrimitiveType::TriangleFan,
            vertex_count: 4,
            layout: VertexLayout::from_fvf(fvf).unwrap(),
            streams: Vec::new(),
            indices: None,
            state,
            extras: raw.stage_extras(),
            raw: Arc::new(raw),
            target: TargetInfo {
                width: 32, height: 32, format: D3DFormat::A8R8G8B8,
                depth_format: Some(D3DFormat::D24S8),
            },
            shader: ShaderInputs {
                vertex: VertexProcessing::FixedFunction { fvf },
                provided_inputs: 0b1001,
                vertex_defaults: [[0.0; 4]; 16],
                constants: Arc::new([[0.0; 4]; 192]),
                pixel_shader: None,
                combiner_state: Arc::new([0; 57]),
                final_combiner_latch: false,
                affine_projection: false,
            },
            viewport: Viewport {
                x: 0, y: 0, width: 32, height: 32, min_z: 0.0, max_z: 1.0,
            },
            clip: Some(PixelRect { x: 0, y: 0, width: 32, height: 32 }),
            clip_rects: Vec::new(),
            hw: crate::hw::HwState {
                stipple_control: Some(1),
                stipple_pattern: [0xa5a5a5a5; 32],
                ..Default::default()
            },
            textures: [None; 4],
            texture_info: [None; 4],
            glyph_cache: 1,
        }
    }

    #[test]
    fn ui_program_carries_alpha_fog_stipple_flat_depth_and_stencil() {
        let mut draw = ui_draw();
        let caps = Capabilities::ALL;
        let reference = NativeDrawProgram::resolve_ui(&draw, [false; 4], caps, FetchMode::Cpu, 1).unwrap();
        let display = NativeDrawProgram::resolve_ui(&draw, [false; 4], caps, FetchMode::Cpu, 2).unwrap();
        assert_eq!(reference.pixel.alpha_ref[0], 91.0);
        assert_eq!(reference.pixel.fog_color, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(reference.pixel.stipple, [0xa5a5a5a5; 32]);
        assert_eq!(reference.key.0.pixel_options.alpha_test, Some(alpha_func(CompareFunc::Greater)));
        assert!(reference.key.0.pixel_options.stipple);
        assert!(reference.key.0.pixel_options.flat_shading);
        assert!(reference.key.0.vertex_options.flat_shading);
        assert_eq!(reference.key.0.pixel_options.glyph, 0);
        assert_eq!(display.key.0.pixel_options.glyph, 1);
        assert!(reference.key.translate().unwrap().discards);

        draw.state.render.fill_mode = FillMode::Wireframe;
        let state = &draw.state.render;
        let topology = draw.primitive.host_topology();
        assert_eq!(gfx::translate::primitive_state(state, topology, caps).polygon_mode, wgpu::PolygonMode::Line);
        let depth = gfx::translate::depth_stencil_state(
            state, Some(wgpu::TextureFormat::Depth24PlusStencil8), topology,
        ).unwrap();
        assert_eq!(depth.depth_compare, Some(wgpu::CompareFunction::LessEqual));
        assert_eq!(depth.depth_write_enabled, Some(true));
        assert_eq!(depth.stencil.front.pass_op, wgpu::StencilOperation::Replace);
        assert!(draw.writes_depth_stencil());
        let wire = NativeDrawProgram::resolve_ui(&draw, [false; 4], caps, FetchMode::Cpu, 1).unwrap();
        assert!(!wire.key.0.pixel_options.stipple);
    }
}
