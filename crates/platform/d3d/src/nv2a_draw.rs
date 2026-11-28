//! Shared shader translation inputs for ordinary draws and native indexed
//! passes that take over an already resolved draw.

use super::*;
use gfx::translate::Capabilities;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NativeDrawKey(ProgramKey);

/// GPU-independent shader and sampler selection for a resolved material.
pub struct NativeDrawMaterial {
    pub key: NativeDrawKey,
    def: PixelShaderDef,
    signs: [u8; 4],
    pub samplers: [SamplerKey; 4],
    pub writes_depth: bool,
}

/// GPU-independent shader inputs for one copy of a resolved draw.
pub struct NativeDrawProgram {
    pub key: NativeDrawKey,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub samplers: [SamplerKey; 4],
    pub writes_depth: bool,
}

impl NativeDrawProgram {
    pub fn resolve(draw: &ResolvedDraw, present: [bool; 4], caps: Capabilities,
        fetch: FetchMode, scale: impl Into<crate::scaled::RenderScale>) -> Result<Self, String> {
        let material = NativeDrawMaterial::resolve(draw, present, caps, fetch)?;
        let (vertex, pixel) = material.uniforms(draw, scale);
        Ok(Self { key: material.key, vertex, pixel, samplers: material.samplers, writes_depth: material.writes_depth })
    }
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

static TABLE: std::sync::LazyLock<RenderStateTable> = std::sync::LazyLock::new(RenderStateTable::default);

impl NativeDrawMaterial {
    pub fn resolve(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        fetch: FetchMode,
    ) -> Result<Self, String> {
        Self::resolve_inputs(draw, present, caps, || input_fetch(draw, fetch))
    }

    /// Resolve the shader using the layout plan that records its vertex buffers.
    pub fn resolve_with_plan(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        plan: &RepackPlan,
    ) -> Result<Self, String> {
        Self::resolve_inputs(draw, present, caps, || plan_input_fetch(plan))
    }

    fn resolve_inputs(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        input_fetch: impl FnOnce() -> [InputFetch; 16],
    ) -> Result<Self, String> {
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
        } = program_key_with_fetch(&TABLE, caps, draw, present, input_fetch)?;
        if !matches!(key.vertex, VertexSource::Microcode(_)) {
            return Err("requires a vertex program".into());
        }
        if key.pixel_options.shadow.iter().any(Option::is_some) || draw.glyph_cache != 0 {
            return Err("requires a backend-specific texture path".into());
        }
        Ok(Self {
            writes_depth: key.pixel_options.w_buffer,
            key: NativeDrawKey(key),
            def,
            signs,
            samplers: samplers(draw, border, caps),
        })
    }

    pub fn uniforms(&self, draw: &ResolvedDraw, scale: impl Into<crate::scaled::RenderScale>) -> (VertexUniforms, PixelUniforms) {
        uniforms(&TABLE, draw, &self.key.0, &self.def, self.signs, scale)
    }
}

impl NativeDrawProgram {
    /// A UI producer may use pretransformed vertices and the display-only
    /// glyph filter. Use the same shader key and uniforms as an ordinary draw.
    pub fn resolve_ui(
        draw: &ResolvedDraw,
        present: [bool; 4],
        caps: Capabilities,
        fetch: FetchMode,
        scale: impl Into<crate::scaled::RenderScale>,
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
        let scale: crate::scaled::RenderScale = scale.into();
        if scale.magnified() {
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
    scale: impl Into<crate::scaled::RenderScale>,
) -> (VertexUniforms, PixelUniforms) {
    let (c0, c1) = def.embedded_constants();
    let render = &draw.state.render;
    let scale: crate::scaled::RenderScale = scale.into();
    let mut vu = VertexUniforms {
        surface: [draw.target.width as f32, draw.target.height as f32],
        // Same full scale as gfx's built-in shader.
        z_max: gfx::renderer::d3dz_max(draw.target.depth_format),
        shift: scale.shift(),
        ui_transform: draw.ui_fit.map_or([1.0, 1.0, 0.0, 0.0], |fit| fit.transform),
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
    pu.stipple = draw.hw.stipple_rows(scale);
    let guest = (draw.target.width, draw.target.height);
    pu.stipple_scale = PixelUniforms::stipple_grid(guest, scale.size(guest.0, guest.1));
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
        pu.depth_range = [lo, hi];
    }
    pu.lod_bias = std::array::from_fn(|n| {
        let bias = draw.state.stages[n].mip_lod_bias;
        if bias.is_finite() { bias } else { 0.0 }
    });
    pu.border = std::array::from_fn(|n| border_value(draw.state.stages[n].border_color, signs[n]));
    for n in 0..4 {
        let e = &draw.extras[n];
        pu.bump_mat[n] = draw.bump_matrix(n);
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
    std::array::from_fn(|n| sampler_key(&draw.state.stages[n], caps, border[n]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{PixelRect, TargetInfo, TextureInfo, Viewport};
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
        let fvf = fvf::XYZRHW | fvf::DIFFUSE | (1 << fvf::TEXCOUNT_SHIFT);
        let glyph = TextureInfo {
            header: 0x1000,
            palette: 0,
            format: D3DFormat::A8R8G8B8,
            kind: TextureKind::D2,
            width: 16,
            height: 16,
            depth: 1,
            levels: 1,
        };
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
                provided_inputs: (1 << 9) | 0b1001,
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
            textures: [Some(glyph.header), None, None, None],
            texture_info: [Some(glyph), None, None, None],
            retail_constants: None,
            ui_fit: None,
            glyph_cache: 1,
        }
    }

    #[test]
    fn depth_billboard_fetch_fits_only_its_window_coordinate_on_the_display_copy() {
        let mut draw = ui_draw();
        let fit = crate::backend::UiFit::projected_sprite(0.75, 80.).with_screen_input(1);
        draw.ui_fit = Some(fit);
        let resolve = |d: &ResolvedDraw| {
            NativeDrawProgram::resolve_ui(d, [true, false, false, false],
                Capabilities::ALL, FetchMode::Cpu, 4u32).unwrap()
        };
        let display = resolve(&draw);
        let retail = resolve(&draw.for_reference());
        let mut expected = [InputFetch::Float; 16];
        expected[1] = InputFetch::WindowFloat;
        assert_eq!(display.key.0.vertex_options.input_fetch, expected);
        assert_eq!(display.vertex.ui_transform, fit.transform);
        assert_eq!(retail.key.0.vertex_options.input_fetch, [InputFetch::Float; 16]);
        assert_eq!(retail.vertex.ui_transform, [1., 1., 0., 0.]);
    }

    #[test]
    fn frame_distortion_follows_the_display_lens_on_both_horizontal_coefficients() {
        let mut draw = ui_draw();
        let matrix = [8., 5., -4., 3.];
        draw.extras[0].samples_frame = true;
        draw.extras[0].bump_env_mat = matrix;
        draw.extras[1].bump_env_mat = matrix;
        draw.retail_constants = Some((draw.shader.constants.clone(), 0.75));
        assert_eq!(draw.bump_matrix(0), [6., 5., -3., 3.]);
        assert_eq!(draw.bump_matrix(1), matrix);
        assert_eq!(draw.for_reference().bump_matrix(0), matrix);
    }

    #[test]
    fn hud_vertex_output_uses_glyph_colour_projected_uv_and_window_position() {
        let mut draw = ui_draw();
        let mut stream = Vec::new();
        for x in [1.0f32, 2., 2., 1.] {
            for value in [x, 3., 0., 1.] {
                stream.extend_from_slice(&value.to_le_bytes());
            }
            stream.extend_from_slice(&[0, 51, 255, 102]);
            for value in [4.0f32, 12.] {
                stream.extend_from_slice(&value.to_le_bytes());
            }
        }
        draw.streams.push(crate::backend::StreamBytes { data: stream.into(), stride: 28 });
        draw.texture_info[0].as_mut().unwrap().format = D3DFormat::LinA8R8G8B8;
        draw.state.stages[1].tex_coord_index = 0;
        draw.texture_info[1] = draw.texture_info[0];
        draw.texture_info[1].as_mut().unwrap().format = D3DFormat::A8R8G8B8;
        let sampled = [true, true, false, false];
        let corners = crate::nv2a::overlay_vertices(&draw, sampled).unwrap();
        assert_eq!(corners[0], ([1.5, 3.5], [[0.25, 0.75], [4., 12.], [0.; 2], [0.; 2]], [1., 0.2, 0., 0.4]));
        assert_eq!(corners[1].0, [2.5, 3.5]);
        let mut varied = draw.streams[0].data.to_vec();
        varied[28 + 12..28 + 16].copy_from_slice(&0.5f32.to_le_bytes());
        draw.streams[0].data = varied.into();
        assert!(crate::nv2a::overlay_vertices(&draw, sampled).is_none());
    }

    #[test]
    fn a_display_stipple_reaches_the_display_copy_only() {
        let mut draw = ui_draw();
        draw.hw.display_stipple = Some([u32::MAX; 32]);
        let caps = Capabilities::ALL;
        let present = [true, false, false, false];
        let reference = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 1u32).unwrap();
        let display = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 2u32).unwrap();
        assert_eq!(reference.pixel.stipple, [0xa5a5a5a5; 32], "the reference copy keeps the guest's pattern");
        assert_eq!(display.pixel.stipple, [u32::MAX; 32], "the display copy draws the override");
        draw.hw.display_stipple = None;
        let display = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 2u32).unwrap();
        assert_eq!(display.pixel.stipple, [0xa5a5a5a5; 32]);
    }

    #[test]
    fn ui_program_carries_alpha_fog_stipple_flat_depth_and_stencil() {
        let mut draw = ui_draw();
        let caps = Capabilities::ALL;
        let present = [true, false, false, false];
        assert!(matches!(
            NativeDrawProgram::resolve_ui(&draw, [false; 4], caps, FetchMode::Cpu, 1u32),
            Err(UiProgramError::Texture)
        ));
        let reference = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 1u32).unwrap();
        let display = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 2u32).unwrap();
        assert_eq!(reference.pixel.alpha_ref[0], 91.0);
        assert_eq!(reference.pixel.fog_color, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            crate::nv2a::draw_fog(&draw),
            (reference.pixel.fog, reference.pixel.fog_color),
            "a native pass in the draw's place reads the fog register its D3D draw does"
        );
        assert_eq!(reference.pixel.stipple, [0xa5a5a5a5; 32]);
        let guest = (draw.target.width, draw.target.height);
        assert_eq!(reference.pixel.stipple_scale, PixelUniforms::stipple_grid(guest, guest));
        assert_eq!(display.pixel.stipple_scale, PixelUniforms::stipple_grid(guest, (2 * guest.0, 2 * guest.1)));
        let host = (guest.0 * 3 / 2, guest.1 * 3 / 4);
        let raster = crate::scaled::RenderScale::between(guest, host).unwrap();
        let uneven = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, raster).unwrap();
        assert_eq!(uneven.pixel.stipple_scale, PixelUniforms::stipple_grid(guest, host));
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
        let wire = NativeDrawProgram::resolve_ui(&draw, present, caps, FetchMode::Cpu, 1u32).unwrap();
        assert!(!wire.key.0.pixel_options.stipple);
    }
}
