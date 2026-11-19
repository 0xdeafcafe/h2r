//! Translation of the typed D3D8 state model ([`crate::state`]) into wgpu
//! pipeline and sampler descriptors.
//!
//! Everything here is a pure function: no device is needed, so the mapping is
//! unit-tested directly. Where D3D8 semantics cannot be expressed in a wgpu
//! pipeline the function documents the approximation.
//!
//! Handled by the pipeline: blend, depth/stencil, cull/front-face, fill mode,
//! colour write mask, depth bias. Handled at draw time (not part of the
//! pipeline): stencil reference, blend constant. Handled in shaders (future
//! work, see `docs/graphics/graphics.md`): alpha test, fog, shade mode, texture-stage
//! combiners.

use crate::state::*;

/// Optional wgpu features that change what translation may emit.
///
/// Build from the device's enabled features with [`Capabilities::from_features`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// `POLYGON_MODE_LINE`: wireframe fill.
    pub polygon_mode_line: bool,
    /// `POLYGON_MODE_POINT`: point fill.
    pub polygon_mode_point: bool,
    /// `ADDRESS_MODE_CLAMP_TO_BORDER`.
    pub clamp_to_border: bool,
    /// `TEXTURE_COMPRESSION_BC`.
    pub texture_compression_bc: bool,
    /// `DEPTH32FLOAT_STENCIL8`: lets depth/stencil surfaces be read back.
    pub depth32_stencil8: bool,
    /// `DEPTH_CLIP_CONTROL`: `PrimitiveState::unclipped_depth`, for draws
    /// that clamp depth ([`primitive_state`]).
    pub depth_clip_control: bool,
}

impl Capabilities {
    /// Everything available.
    pub const ALL: Self = Self {
        polygon_mode_line: true,
        polygon_mode_point: true,
        clamp_to_border: true,
        texture_compression_bc: true,
        depth32_stencil8: true,
        depth_clip_control: true,
    };
    /// Nothing optional available (the lowest common denominator).
    pub const NONE: Self = Self {
        polygon_mode_line: false,
        polygon_mode_point: false,
        clamp_to_border: false,
        texture_compression_bc: false,
        depth32_stencil8: false,
        depth_clip_control: false,
    };

    /// The optional wgpu features the renderer asks for, intersected with
    /// what the adapter offers.
    pub fn wanted_features() -> wgpu::Features {
        wgpu::Features::POLYGON_MODE_LINE
            | wgpu::Features::POLYGON_MODE_POINT
            | wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER
            | wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::DEPTH32FLOAT_STENCIL8
            | wgpu::Features::DEPTH_CLIP_CONTROL
    }

    /// Reads capabilities from enabled device features.
    pub fn from_features(f: wgpu::Features) -> Self {
        Self {
            polygon_mode_line: f.contains(wgpu::Features::POLYGON_MODE_LINE),
            polygon_mode_point: f.contains(wgpu::Features::POLYGON_MODE_POINT),
            clamp_to_border: f.contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER),
            texture_compression_bc: f.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            depth32_stencil8: f.contains(wgpu::Features::DEPTH32FLOAT_STENCIL8),
            depth_clip_control: f.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
        }
    }
}

/// Says once, on stderr, that a draw clamps depth but the device has no
/// `DEPTH_CLIP_CONTROL`, so it is clipped instead ([`primitive_state`]).
/// Returns whether this call said it.
pub fn depth_clamp_unsupported() -> bool {
    static SAID: std::sync::Once = std::sync::Once::new();
    let mut said = false;
    SAID.call_once(|| {
        eprintln!(
            "gfx: the device has no DEPTH_CLIP_CONTROL: draws that clamp depth \
             (D3DRS_DEPTHCLIPCONTROL) are clipped at the near and far planes instead"
        );
        said = true;
    });
    said
}

/// [`CompareFunc`] -> wgpu.
pub fn compare_function(f: CompareFunc) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as W;
    match f {
        CompareFunc::Never => W::Never,
        CompareFunc::Less => W::Less,
        CompareFunc::Equal => W::Equal,
        CompareFunc::LessEqual => W::LessEqual,
        CompareFunc::Greater => W::Greater,
        CompareFunc::NotEqual => W::NotEqual,
        CompareFunc::GreaterEqual => W::GreaterEqual,
        CompareFunc::Always => W::Always,
    }
}

/// [`BlendFactor`] -> wgpu.
///
/// wgpu's `Constant` factor is one colour for both colour and alpha
/// channels, so `ConstantAlpha` is approximated by `Constant`; see
/// [`blend_constant`], which then loads the constant with the alpha value in
/// every channel.
pub fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as W;
    match f {
        BlendFactor::Zero => W::Zero,
        BlendFactor::One => W::One,
        BlendFactor::SrcColor => W::Src,
        BlendFactor::InvSrcColor => W::OneMinusSrc,
        BlendFactor::SrcAlpha => W::SrcAlpha,
        BlendFactor::InvSrcAlpha => W::OneMinusSrcAlpha,
        BlendFactor::DestAlpha => W::DstAlpha,
        BlendFactor::InvDestAlpha => W::OneMinusDstAlpha,
        BlendFactor::DestColor => W::Dst,
        BlendFactor::InvDestColor => W::OneMinusDst,
        BlendFactor::SrcAlphaSat => W::SrcAlphaSaturated,
        BlendFactor::ConstantColor | BlendFactor::ConstantAlpha => W::Constant,
        BlendFactor::InvConstantColor | BlendFactor::InvConstantAlpha => W::OneMinusConstant,
    }
}

/// [`BlendOp`] -> wgpu. The Xbox "signed" extensions have no equivalent and
/// fall back to their unsigned forms.
pub fn blend_operation(op: BlendOp) -> wgpu::BlendOperation {
    use wgpu::BlendOperation as W;
    match op {
        BlendOp::Add | BlendOp::AddSigned => W::Add,
        BlendOp::Subtract => W::Subtract,
        BlendOp::ReverseSubtract | BlendOp::ReverseSubtractSigned => W::ReverseSubtract,
        BlendOp::Min => W::Min,
        BlendOp::Max => W::Max,
    }
}

/// Translates the blend render states. `None` when blending is disabled.
///
/// wgpu requires `One`/`One` factors with `Min`/`Max`; D3D ignores the
/// factors for those ops, so they are forced here.
pub fn blend_state(rs: &RenderState) -> Option<wgpu::BlendState> {
    if !rs.blend_enable {
        return None;
    }
    let operation = blend_operation(rs.blend_op);
    let (src_factor, dst_factor) = match operation {
        wgpu::BlendOperation::Min | wgpu::BlendOperation::Max => (wgpu::BlendFactor::One, wgpu::BlendFactor::One),
        _ => (blend_factor(rs.src_blend), blend_factor(rs.dst_blend)),
    };
    let component = wgpu::BlendComponent { src_factor, dst_factor, operation };
    Some(wgpu::BlendState { color: component, alpha: component })
}

/// The blend constant to set on the render pass for this state.
///
/// If any factor uses the constant *alpha* (and none the constant colour),
/// the alpha is splatted into all channels, which makes wgpu's single
/// `Constant` behave like D3D's `CONSTANTALPHA`.
pub fn blend_constant(rs: &RenderState) -> wgpu::Color {
    let c = rs.blend_color;
    let (a, r, g, b) = (
        (c >> 24 & 0xFF) as f64 / 255.0,
        (c >> 16 & 0xFF) as f64 / 255.0,
        (c >> 8 & 0xFF) as f64 / 255.0,
        (c & 0xFF) as f64 / 255.0,
    );
    let uses = |f: BlendFactor| rs.src_blend == f || rs.dst_blend == f;
    let uses_alpha = uses(BlendFactor::ConstantAlpha) || uses(BlendFactor::InvConstantAlpha);
    let uses_color = uses(BlendFactor::ConstantColor) || uses(BlendFactor::InvConstantColor);
    if uses_alpha && !uses_color {
        wgpu::Color { r: a, g: a, b: a, a }
    } else {
        wgpu::Color { r, g, b, a }
    }
}

/// [`ColorWriteMask`] -> wgpu.
pub fn color_writes(m: ColorWriteMask) -> wgpu::ColorWrites {
    let mut w = wgpu::ColorWrites::empty();
    w.set(wgpu::ColorWrites::RED, m.red);
    w.set(wgpu::ColorWrites::GREEN, m.green);
    w.set(wgpu::ColorWrites::BLUE, m.blue);
    w.set(wgpu::ColorWrites::ALPHA, m.alpha);
    w
}

/// [`StencilOp`] -> wgpu.
pub fn stencil_operation(op: StencilOp) -> wgpu::StencilOperation {
    use wgpu::StencilOperation as W;
    match op {
        StencilOp::Keep => W::Keep,
        StencilOp::Zero => W::Zero,
        StencilOp::Replace => W::Replace,
        StencilOp::IncrSat => W::IncrementClamp,
        StencilOp::DecrSat => W::DecrementClamp,
        StencilOp::Invert => W::Invert,
        StencilOp::Incr => W::IncrementWrap,
        StencilOp::Decr => W::DecrementWrap,
    }
}

/// The bias enable that applies to the polygon fill mode in effect.
fn offset_enabled(rs: &RenderState) -> bool {
    match rs.fill_mode {
        FillMode::Point => rs.point_offset_enable,
        FillMode::Wireframe => rs.wireframe_offset_enable,
        FillMode::Solid => rs.solid_offset_enable,
    }
}

/// Translates depth/stencil state for a pass whose depth attachment has
/// format `depth_format`. `None` if there is no depth attachment.
///
/// Notes:
/// * `W` buffering is treated as Z buffering.
/// * Depth bias: the Xbox offset is already in depth-buffer steps (xemu hands
///   it unchanged to `glPolygonOffset`), the same unit as wgpu's constant
///   bias, so it is passed through unscaled. It applies only to triangle
///   topologies (wgpu rejects bias on lines and points) and only when the
///   enable for the active fill mode (`SOLID/WIREFRAME/POINTOFFSETENABLE`)
///   is set.
/// * The stencil reference is a draw-time value (`set_stencil_reference`).
pub fn depth_stencil_state(
    rs: &RenderState,
    depth_format: Option<wgpu::TextureFormat>,
    topology: wgpu::PrimitiveTopology,
) -> Option<wgpu::DepthStencilState> {
    let format = depth_format?;
    let depth_on = rs.depth_mode != DepthBufferMode::Disabled;
    let stencil = if rs.stencil.enable && format.has_stencil_aspect() {
        let s = &rs.stencil;
        let face = wgpu::StencilFaceState {
            compare: compare_function(s.func),
            fail_op: stencil_operation(s.fail),
            depth_fail_op: stencil_operation(s.depth_fail),
            pass_op: stencil_operation(s.pass),
        };
        wgpu::StencilState {
            front: face,
            back: face,
            read_mask: s.read_mask as u32,
            write_mask: s.write_mask as u32,
        }
    } else {
        wgpu::StencilState::default()
    };
    let triangles = matches!(topology, wgpu::PrimitiveTopology::TriangleList | wgpu::PrimitiveTopology::TriangleStrip);
    let bias = if triangles && offset_enabled(rs) {
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        wgpu::DepthBiasState {
            constant: finite(rs.depth_bias).round().clamp(-1e9, 1e9) as i32,
            slope_scale: finite(rs.depth_bias_slope_scale),
            clamp: 0.0,
        }
    } else {
        wgpu::DepthBiasState::default()
    };
    Some(wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(depth_on && rs.depth_write),
        depth_compare: Some(if depth_on { compare_function(rs.depth_func) } else { wgpu::CompareFunction::Always }),
        stencil,
        bias,
    })
}

/// Translates cull mode, front face and fill mode.
///
/// D3D's `CULL_CW` removes triangles that appear clockwise on screen. Xbox
/// D3D programs the NV2A the same way (`SetRenderState_CullMode`,
/// `0x3f53d0`): `FRONT_FACE` is `D3DRS_FRONTFACE` and `CULL_FACE` is
/// `FRONT` when `CULLMODE == FRONTFACE`, else `BACK`. Both cull the
/// `CULLMODE` winding whatever `FRONTFACE` says, but the front face also
/// decides `@builtin(front_facing)` (two-sided lighting reads `oB0`/`oB1`
/// on back faces), so it is carried over too. Window y points down on the
/// NV2A and in wgpu's framebuffer, so windings match.
/// Only the front fill mode is honoured (wgpu has one polygon mode).
///
/// A draw that clamps depth ([`RenderState::depth_clamp`]) has its depth
/// unclipped when the device has `DEPTH_CLIP_CONTROL`: the NV2A clamps
/// depth outside the clip range rather than culling it. Without the
/// feature it is clipped as before ([`depth_clamp_unsupported`] says so
/// once). The other planes still clip: the WebGPU spec, 23.2.4 "Primitive
/// Clipping", bounds the clip volume by `-w <= x <= w`, `-w <= y <= w` and
/// `0 <= z <= w`, and with `unclippedDepth` only drops the last ("the clip
/// volume is not bounded in the z dimension"), so w < 0 stays clipped (the
/// x and y bounds need w >= 0) and clipping makes no vertex at w = 0 with
/// x or y away from 0. On Metal, wgpu-hal 30.0.2 (`metal/device.rs`, render
/// pipeline creation) sets `MTLDepthClipMode::Clamp` for it, which Apple
/// documents as "Clamp fragments outside the near or far planes" (only the
/// near and far planes).
pub fn primitive_state(
    rs: &RenderState,
    topology: wgpu::PrimitiveTopology,
    caps: Capabilities,
) -> wgpu::PrimitiveState {
    let front_face = match rs.front_face {
        FrontFace::Cw => wgpu::FrontFace::Cw,
        FrontFace::Ccw => wgpu::FrontFace::Ccw,
    };
    let cull_mode = match (rs.cull_mode, rs.front_face) {
        (CullMode::None, _) => None,
        (CullMode::Cw, FrontFace::Cw) | (CullMode::Ccw, FrontFace::Ccw) => Some(wgpu::Face::Front),
        _ => Some(wgpu::Face::Back),
    };
    let polygon_mode = match rs.fill_mode {
        FillMode::Solid => wgpu::PolygonMode::Fill,
        FillMode::Wireframe if caps.polygon_mode_line => wgpu::PolygonMode::Line,
        FillMode::Point if caps.polygon_mode_point => wgpu::PolygonMode::Point,
        _ => wgpu::PolygonMode::Fill,
    };
    wgpu::PrimitiveState {
        topology,
        strip_index_format: None,
        front_face,
        cull_mode,
        unclipped_depth: rs.depth_clamp && caps.depth_clip_control,
        polygon_mode,
        conservative: false,
    }
}

// ---------------------------------------------------------------------------
// Samplers
// ---------------------------------------------------------------------------

/// Hashable description of a sampler, derived from [`TextureStageState`].
/// Used as the sampler cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SamplerKey {
    /// U addressing.
    pub address_u: wgpu::AddressMode,
    /// V addressing.
    pub address_v: wgpu::AddressMode,
    /// W addressing.
    pub address_w: wgpu::AddressMode,
    /// Magnification filter.
    pub mag: wgpu::FilterMode,
    /// Minification filter.
    pub min: wgpu::FilterMode,
    /// Mip filter.
    pub mipmap: wgpu::MipmapFilterMode,
    /// `f32::to_bits` of the minimum LOD.
    pub lod_min: u32,
    /// `f32::to_bits` of the maximum LOD.
    pub lod_max: u32,
    /// Max anisotropy (1 = off).
    pub anisotropy: u16,
    /// Border colour (only with `ClampToBorder`).
    pub border: Option<wgpu::SamplerBorderColor>,
}

fn address_mode(a: TextureAddress, caps: Capabilities) -> wgpu::AddressMode {
    match a {
        TextureAddress::Wrap => wgpu::AddressMode::Repeat,
        TextureAddress::Mirror => wgpu::AddressMode::MirrorRepeat,
        // The NV2A's CLAMP differs from CLAMP_TO_EDGE only for border
        // handling with bilinear filtering; edge clamp is the safe choice.
        TextureAddress::Clamp | TextureAddress::ClampToEdge => wgpu::AddressMode::ClampToEdge,
        TextureAddress::Border if caps.clamp_to_border => wgpu::AddressMode::ClampToBorder,
        TextureAddress::Border => wgpu::AddressMode::ClampToEdge,
    }
}

fn border_color(argb: u32) -> wgpu::SamplerBorderColor {
    let a = argb >> 24;
    let (r, g, b) = ((argb >> 16) & 0xFF, (argb >> 8) & 0xFF, argb & 0xFF);
    if a < 0x80 {
        wgpu::SamplerBorderColor::TransparentBlack
    } else if r >= 0x80 && g >= 0x80 && b >= 0x80 {
        wgpu::SamplerBorderColor::OpaqueWhite
    } else {
        wgpu::SamplerBorderColor::OpaqueBlack
    }
}

impl SamplerKey {
    /// Translates a texture stage's sampler state.
    ///
    /// * Mip filter `None` clamps sampling to the base level.
    /// * `MaxMipLevel` becomes the minimum LOD clamp.
    /// * Quincunx/gaussian filters are approximated by bilinear.
    /// * `MIPMAPLODBIAS` has no sampler equivalent in wgpu; it must be applied in the
    ///   shader and is not represented here.
    /// * The border colour is snapped to the nearest of wgpu's three presets.
    pub fn from_stage(s: &TextureStageState, caps: Capabilities) -> Self {
        let filter = |f: TextureFilter| match f {
            TextureFilter::None | TextureFilter::Point => wgpu::FilterMode::Nearest,
            _ => wgpu::FilterMode::Linear,
        };
        let aniso = s.min_filter == TextureFilter::Anisotropic || s.mag_filter == TextureFilter::Anisotropic;
        let mipmap = match s.mip_filter {
            TextureFilter::None | TextureFilter::Point => wgpu::MipmapFilterMode::Nearest,
            _ => wgpu::MipmapFilterMode::Linear,
        };
        let lod_min = s.max_mip_level as f32;
        let lod_max = if s.mip_filter == TextureFilter::None { lod_min } else { 32.0f32.max(lod_min) };
        let (mag, min, mipmap, anisotropy) = if aniso && s.max_anisotropy > 1 {
            // wgpu requires every filter to be linear when anisotropy > 1.
            (wgpu::FilterMode::Linear, wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear, s.max_anisotropy.min(16) as u16)
        } else {
            (filter(s.mag_filter), filter(s.min_filter), mipmap, 1)
        };
        let modes = [s.address_u, s.address_v, s.address_w].map(|a| address_mode(a, caps));
        let border = modes
            .contains(&wgpu::AddressMode::ClampToBorder)
            .then(|| border_color(s.border_color));
        Self {
            address_u: modes[0],
            address_v: modes[1],
            address_w: modes[2],
            mag,
            min,
            mipmap,
            lod_min: lod_min.to_bits(),
            lod_max: lod_max.to_bits(),
            anisotropy,
            border,
        }
    }

    /// The wgpu descriptor for this key.
    pub fn descriptor(&self) -> wgpu::SamplerDescriptor<'static> {
        wgpu::SamplerDescriptor {
            label: Some("xbox sampler"),
            address_mode_u: self.address_u,
            address_mode_v: self.address_v,
            address_mode_w: self.address_w,
            mag_filter: self.mag,
            min_filter: self.min,
            mipmap_filter: self.mipmap,
            lod_min_clamp: f32::from_bits(self.lod_min),
            lod_max_clamp: f32::from_bits(self.lod_max),
            compare: None,
            anisotropy_clamp: self.anisotropy,
            border_color: self.border,
        }
    }
}

#[cfg(test)]
mod tests {
    const TRI: wgpu::PrimitiveTopology = wgpu::PrimitiveTopology::TriangleList;
    use super::*;

    fn rs() -> RenderState {
        RenderState::default()
    }

    #[test]
    fn blend_disabled_is_none() {
        assert_eq!(blend_state(&rs()), None);
    }

    #[test]
    fn classic_alpha_blend() {
        let mut s = rs();
        s.blend_enable = true;
        s.src_blend = BlendFactor::SrcAlpha;
        s.dst_blend = BlendFactor::InvSrcAlpha;
        let b = blend_state(&s).unwrap();
        assert_eq!(b.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(b.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(b.color.operation, wgpu::BlendOperation::Add);
        assert_eq!(b.alpha, b.color);
    }

    #[test]
    fn additive_and_modulate_factors() {
        let mut s = rs();
        s.blend_enable = true;
        s.src_blend = BlendFactor::One;
        s.dst_blend = BlendFactor::One;
        let b = blend_state(&s).unwrap();
        assert_eq!(b.color.src_factor, wgpu::BlendFactor::One);
        s.src_blend = BlendFactor::DestColor;
        s.dst_blend = BlendFactor::Zero;
        let b = blend_state(&s).unwrap();
        assert_eq!(b.color.src_factor, wgpu::BlendFactor::Dst);
        assert_eq!(b.color.dst_factor, wgpu::BlendFactor::Zero);
        s.src_blend = BlendFactor::SrcAlphaSat;
        assert_eq!(blend_state(&s).unwrap().color.src_factor, wgpu::BlendFactor::SrcAlphaSaturated);
    }

    #[test]
    fn min_max_force_one_one() {
        let mut s = rs();
        s.blend_enable = true;
        s.blend_op = BlendOp::Max;
        s.src_blend = BlendFactor::SrcAlpha;
        s.dst_blend = BlendFactor::InvSrcAlpha;
        let b = blend_state(&s).unwrap();
        assert_eq!(b.color.operation, wgpu::BlendOperation::Max);
        assert_eq!((b.color.src_factor, b.color.dst_factor), (wgpu::BlendFactor::One, wgpu::BlendFactor::One));
    }

    #[test]
    fn blend_constant_alpha_is_splatted() {
        let mut s = rs();
        s.blend_color = 0x80_FF_00_00;
        s.src_blend = BlendFactor::ConstantAlpha;
        s.dst_blend = BlendFactor::InvConstantAlpha;
        let c = blend_constant(&s);
        assert!((c.r - 128.0 / 255.0).abs() < 1e-9 && c.r == c.g && c.g == c.b && c.b == c.a);
        s.src_blend = BlendFactor::ConstantColor;
        let c = blend_constant(&s);
        assert_eq!((c.r, c.g, c.b), (1.0, 0.0, 0.0));
    }

    #[test]
    fn color_mask_translation() {
        let w = color_writes(ColorWriteMask { red: true, green: false, blue: true, alpha: false });
        assert_eq!(w, wgpu::ColorWrites::RED | wgpu::ColorWrites::BLUE);
        assert_eq!(color_writes(ColorWriteMask::ALL), wgpu::ColorWrites::ALL);
    }

    #[test]
    fn depth_defaults() {
        let d = depth_stencil_state(&rs(), Some(wgpu::TextureFormat::Depth24PlusStencil8), TRI).unwrap();
        assert_eq!(d.depth_write_enabled, Some(true));
        assert_eq!(d.depth_compare, Some(wgpu::CompareFunction::LessEqual));
        assert_eq!(d.stencil, wgpu::StencilState::default());
        assert!(depth_stencil_state(&rs(), None, TRI).is_none());
    }

    #[test]
    fn depth_disabled_ignores_write_and_func() {
        let mut s = rs();
        s.depth_mode = DepthBufferMode::Disabled;
        s.depth_func = CompareFunc::Less;
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth16Unorm), TRI).unwrap();
        assert_eq!(d.depth_write_enabled, Some(false));
        assert_eq!(d.depth_compare, Some(wgpu::CompareFunction::Always));
    }

    #[test]
    fn stencil_translation() {
        let mut s = rs();
        s.stencil = StencilState {
            enable: true,
            func: CompareFunc::Equal,
            reference: 1,
            read_mask: 0x0F,
            write_mask: 0xF0,
            fail: StencilOp::Zero,
            depth_fail: StencilOp::IncrSat,
            pass: StencilOp::Decr,
        };
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth24PlusStencil8), TRI).unwrap();
        assert_eq!(d.stencil.front.compare, wgpu::CompareFunction::Equal);
        assert_eq!(d.stencil.front.fail_op, wgpu::StencilOperation::Zero);
        assert_eq!(d.stencil.front.depth_fail_op, wgpu::StencilOperation::IncrementClamp);
        assert_eq!(d.stencil.front.pass_op, wgpu::StencilOperation::DecrementWrap);
        assert_eq!(d.stencil.back, d.stencil.front);
        assert_eq!((d.stencil.read_mask, d.stencil.write_mask), (0x0F, 0xF0));
        // Depth-only formats get no stencil state.
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth16Unorm), TRI).unwrap();
        assert_eq!(d.stencil, wgpu::StencilState::default());
    }

    #[test]
    fn depth_bias_is_in_depth_steps_and_gated() {
        let mut s = rs();
        s.depth_bias = -1.0;
        s.depth_bias_slope_scale = 2.0;
        // Disabled by default.
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth24PlusStencil8), TRI).unwrap();
        assert_eq!(d.bias, wgpu::DepthBiasState::default());
        s.solid_offset_enable = true;
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth24PlusStencil8), TRI).unwrap();
        assert_eq!(d.bias.constant, -1);
        assert_eq!(d.bias.slope_scale, 2.0);
        // Lines and points never carry bias (wgpu validation).
        for t in [wgpu::PrimitiveTopology::LineList, wgpu::PrimitiveTopology::LineStrip, wgpu::PrimitiveTopology::PointList] {
            let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth24PlusStencil8), t).unwrap();
            assert_eq!(d.bias, wgpu::DepthBiasState::default());
        }
        // Wireframe polygons use their own enable.
        s.fill_mode = FillMode::Wireframe;
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth16Unorm), TRI).unwrap();
        assert_eq!(d.bias, wgpu::DepthBiasState::default());
        s.wireframe_offset_enable = true;
        let d = depth_stencil_state(&s, Some(wgpu::TextureFormat::Depth16Unorm), TRI).unwrap();
        assert_eq!(d.bias.constant, -1);
    }

    /// The winding a primitive state removes.
    fn culled_winding(p: &wgpu::PrimitiveState) -> Option<wgpu::FrontFace> {
        let other = match p.front_face {
            wgpu::FrontFace::Cw => wgpu::FrontFace::Ccw,
            wgpu::FrontFace::Ccw => wgpu::FrontFace::Cw,
        };
        match p.cull_mode? {
            wgpu::Face::Front => Some(p.front_face),
            wgpu::Face::Back => Some(other),
        }
    }

    #[test]
    fn cull_and_front_face() {
        let topo = wgpu::PrimitiveTopology::TriangleList;
        let mut s = rs();
        s.cull_mode = CullMode::None;
        assert_eq!(primitive_state(&s, topo, Capabilities::ALL).cull_mode, None);
        // CULLMODE alone picks the culled winding; FRONTFACE only says which
        // winding is the front (for front_facing), as on the NV2A.
        for front in [FrontFace::Cw, FrontFace::Ccw] {
            s.front_face = front;
            let want_front = match front {
                FrontFace::Cw => wgpu::FrontFace::Cw,
                FrontFace::Ccw => wgpu::FrontFace::Ccw,
            };
            s.cull_mode = CullMode::None;
            assert_eq!(primitive_state(&s, topo, Capabilities::ALL).front_face, want_front);
            s.cull_mode = CullMode::Cw;
            let p = primitive_state(&s, topo, Capabilities::ALL);
            assert_eq!(p.front_face, want_front);
            assert_eq!(culled_winding(&p), Some(wgpu::FrontFace::Cw));
            s.cull_mode = CullMode::Ccw;
            let p = primitive_state(&s, topo, Capabilities::ALL);
            assert_eq!(culled_winding(&p), Some(wgpu::FrontFace::Ccw));
        }
        // CULL_FACE is FRONT exactly when CULLMODE == FRONTFACE.
        s.front_face = FrontFace::Cw;
        s.cull_mode = CullMode::Cw;
        assert_eq!(primitive_state(&s, topo, Capabilities::ALL).cull_mode, Some(wgpu::Face::Front));
        s.cull_mode = CullMode::Ccw;
        assert_eq!(primitive_state(&s, topo, Capabilities::ALL).cull_mode, Some(wgpu::Face::Back));
    }

    #[test]
    fn depth_clamp_unclips_depth_where_the_device_can() {
        let topo = wgpu::PrimitiveTopology::TriangleList;
        let mut s = rs();
        assert!(!primitive_state(&s, topo, Capabilities::ALL).unclipped_depth);
        s.depth_clamp = true;
        assert!(primitive_state(&s, topo, Capabilities::ALL).unclipped_depth);
        // Without DEPTH_CLIP_CONTROL the draw is clipped, as before.
        assert!(!primitive_state(&s, topo, Capabilities::NONE).unclipped_depth);
        let caps = Capabilities {
            depth_clip_control: false,
            ..Capabilities::ALL
        };
        assert!(!primitive_state(&s, topo, caps).unclipped_depth);
        // The rest of the primitive state is the clamp's business only.
        let mut c = s;
        c.depth_clamp = false;
        let a = primitive_state(&s, topo, Capabilities::ALL);
        let b = primitive_state(&c, topo, Capabilities::ALL);
        assert_eq!(
            wgpu::PrimitiveState {
                unclipped_depth: false,
                ..a
            },
            b
        );
    }

    #[test]
    fn depth_clip_control_is_asked_for_and_read_back() {
        assert!(Capabilities::wanted_features().contains(wgpu::Features::DEPTH_CLIP_CONTROL));
        assert!(Capabilities::from_features(wgpu::Features::DEPTH_CLIP_CONTROL).depth_clip_control);
        assert!(!Capabilities::from_features(wgpu::Features::empty()).depth_clip_control);
    }

    #[test]
    fn missing_depth_clip_control_is_said_once() {
        let said = (0..3).filter(|_| depth_clamp_unsupported()).count();
        assert!(said <= 1);
        assert!(!depth_clamp_unsupported());
    }

    #[test]
    fn fill_mode_respects_capabilities() {
        let topo = wgpu::PrimitiveTopology::TriangleList;
        let mut s = rs();
        s.fill_mode = FillMode::Wireframe;
        assert_eq!(primitive_state(&s, topo, Capabilities::ALL).polygon_mode, wgpu::PolygonMode::Line);
        assert_eq!(primitive_state(&s, topo, Capabilities::NONE).polygon_mode, wgpu::PolygonMode::Fill);
        s.fill_mode = FillMode::Point;
        assert_eq!(primitive_state(&s, topo, Capabilities::ALL).polygon_mode, wgpu::PolygonMode::Point);
    }

    #[test]
    fn sampler_defaults_point_no_mips() {
        let st = TextureStageState::default_for_stage(0);
        let k = SamplerKey::from_stage(&st, Capabilities::ALL);
        assert_eq!(k.mag, wgpu::FilterMode::Nearest);
        assert_eq!(k.address_u, wgpu::AddressMode::Repeat);
        let d = k.descriptor();
        assert_eq!(d.lod_max_clamp, 0.0);
        assert_eq!(d.anisotropy_clamp, 1);
        assert_eq!(d.border_color, None);
    }

    #[test]
    fn sampler_trilinear_clamp() {
        let mut st = TextureStageState::default_for_stage(0);
        st.mag_filter = TextureFilter::Linear;
        st.min_filter = TextureFilter::Linear;
        st.mip_filter = TextureFilter::Linear;
        st.address_u = TextureAddress::Clamp;
        st.address_v = TextureAddress::Mirror;
        st.max_mip_level = 2;
        let d = SamplerKey::from_stage(&st, Capabilities::ALL).descriptor();
        assert_eq!(d.mipmap_filter, wgpu::MipmapFilterMode::Linear);
        assert_eq!(d.address_mode_u, wgpu::AddressMode::ClampToEdge);
        assert_eq!(d.address_mode_v, wgpu::AddressMode::MirrorRepeat);
        assert_eq!(d.lod_min_clamp, 2.0);
        assert_eq!(d.lod_max_clamp, 32.0);
    }

    #[test]
    fn sampler_anisotropy_forces_linear() {
        let mut st = TextureStageState::default_for_stage(0);
        st.min_filter = TextureFilter::Anisotropic;
        st.max_anisotropy = 8;
        let k = SamplerKey::from_stage(&st, Capabilities::ALL);
        assert_eq!(k.anisotropy, 8);
        assert_eq!(k.mag, wgpu::FilterMode::Linear);
        assert_eq!(k.mipmap, wgpu::MipmapFilterMode::Linear);
    }

    #[test]
    fn sampler_border() {
        let mut st = TextureStageState::default_for_stage(0);
        st.address_u = TextureAddress::Border;
        st.border_color = 0xFFFF_FFFF;
        let k = SamplerKey::from_stage(&st, Capabilities::ALL);
        assert_eq!(k.address_u, wgpu::AddressMode::ClampToBorder);
        assert_eq!(k.border, Some(wgpu::SamplerBorderColor::OpaqueWhite));
        st.border_color = 0xFF00_0000;
        assert_eq!(SamplerKey::from_stage(&st, Capabilities::ALL).border, Some(wgpu::SamplerBorderColor::OpaqueBlack));
        st.border_color = 0;
        assert_eq!(SamplerKey::from_stage(&st, Capabilities::ALL).border, Some(wgpu::SamplerBorderColor::TransparentBlack));
        // Without the feature it degrades to edge clamp and no border.
        let k = SamplerKey::from_stage(&st, Capabilities::NONE);
        assert_eq!(k.address_u, wgpu::AddressMode::ClampToEdge);
        assert_eq!(k.border, None);
    }

    #[test]
    fn identical_stage_state_gives_equal_keys() {
        let a = TextureStageState::default_for_stage(0);
        let mut b = TextureStageState::default_for_stage(1);
        b.tex_coord_index = 0; // not a sampler property
        assert_eq!(SamplerKey::from_stage(&a, Capabilities::ALL), SamplerKey::from_stage(&b, Capabilities::ALL));
    }

    #[test]
    fn sampler_lod_range_is_never_inverted() {
        let mut st = TextureStageState::default_for_stage(0);
        st.mip_filter = TextureFilter::Linear;
        st.max_mip_level = 40;
        let d = SamplerKey::from_stage(&st, Capabilities::ALL).descriptor();
        assert!(d.lod_min_clamp <= d.lod_max_clamp);
    }
}
