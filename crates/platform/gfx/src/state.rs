//! A typed model of Xbox Direct3D 8 render state and texture-stage state.
//!
//! The guest stores state as raw 32-bit words. Their *values* are the NV2A's
//! (OpenGL-flavoured) constants, not the PC D3D8 ones: `D3DBLEND_SRCALPHA` is
//! `0x302`, `D3DCMP_LESSEQUAL` is `0x203`, `D3DCULL_CCW` is `0x901`, and so
//! on. Each enum here records the Xbox value and converts both ways with
//! `from_xbox` / `xbox_value`. References: xemu `nv2a_regs.h`
//! (`NV097_SET_BLEND_FUNC_*`, `NV097_SET_STENCIL_OP_V_*`,
//! `NV097_SET_FRONT_POLYGON_MODE_V_*`, `NV097_SET_CULL_FACE_V_*`, commit
//! `4bdad199102b22ed0cfa998e9af6b35d76238b3e`) and Cxbx-Reloaded
//! `XbD3D8Types.h` (`X_D3DRS_*`, `X_D3DTSS_*`, `X_D3DTOP_*`, commit
//! `585c49a50af1255ab155099e06f24505f9c5a800`).
//!
//! Render-state *indices* are SDK-build dependent. [`RenderStateType`] uses
//! Cxbx's canonical numbering; [`crate::xdk::RenderStateTable`] maps a guest
//! slot of a given build (Halo 2: 5849, the default) to it, and
//! [`RenderState::set_guest`] does the lookup and the write in one step.

/// Declares a fieldless enum whose variants carry Xbox raw values.
macro_rules! xbox_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $val:expr ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant ),+
        }
        impl $name {
            /// Decodes the raw Xbox value; `None` if it is not a member.
            pub fn from_xbox(v: u32) -> Option<Self> {
                $( if v == $val { return Some(Self::$variant); } )+
                None
            }
            /// The raw value the guest uses for this variant.
            pub fn xbox_value(self) -> u32 {
                match self { $( Self::$variant => $val ),+ }
            }
        }
    };
}

xbox_enum! {
    /// Comparison function (`D3DCMPFUNC`): depth, alpha and stencil tests.
    pub enum CompareFunc {
        /// Never passes.
        Never = 0x200,
        /// `src < dst`.
        Less = 0x201,
        /// `src == dst`.
        Equal = 0x202,
        /// `src <= dst`.
        LessEqual = 0x203,
        /// `src > dst`.
        Greater = 0x204,
        /// `src != dst`.
        NotEqual = 0x205,
        /// `src >= dst`.
        GreaterEqual = 0x206,
        /// Always passes.
        Always = 0x207,
    }
}

impl CompareFunc {
    /// The Xbox values `0x200..=0x207` are confirmed by Cxbx's
    /// `EmuXB2PC_D3DCMPFUNC` (`XbConvert.h`, commit `585c49a5`). This variant
    /// additionally accepts the PC D3D8 numbering `1..=8` defensively; it is
    /// never produced by Xbox D3D libraries.
    pub fn from_xbox_or_pc(v: u32) -> Option<Self> {
        Self::from_xbox(v).or_else(|| match v {
            1..=8 => Self::from_xbox(0x200 + v - 1),
            _ => None,
        })
    }
}

xbox_enum! {
    /// Blend factor (`D3DBLEND`).
    pub enum BlendFactor {
        /// `(0, 0, 0, 0)`.
        Zero = 0x0000,
        /// `(1, 1, 1, 1)`.
        One = 0x0001,
        /// `(Rs, Gs, Bs, As)`.
        SrcColor = 0x0300,
        /// `1 - SrcColor`.
        InvSrcColor = 0x0301,
        /// `(As, As, As, As)`.
        SrcAlpha = 0x0302,
        /// `1 - SrcAlpha`.
        InvSrcAlpha = 0x0303,
        /// Destination alpha.
        DestAlpha = 0x0304,
        /// `1 - DestAlpha`.
        InvDestAlpha = 0x0305,
        /// Destination colour.
        DestColor = 0x0306,
        /// `1 - DestColor`.
        InvDestColor = 0x0307,
        /// `(f, f, f, 1)` with `f = min(As, 1 - Ad)`.
        SrcAlphaSat = 0x0308,
        /// The constant blend colour (`D3DRS_BLENDCOLOR`).
        ConstantColor = 0x8001,
        /// `1 - ConstantColor`.
        InvConstantColor = 0x8002,
        /// Alpha of the constant blend colour.
        ConstantAlpha = 0x8003,
        /// `1 - ConstantAlpha`.
        InvConstantAlpha = 0x8004,
    }
}

xbox_enum! {
    /// Blend equation (`D3DBLENDOP`).
    pub enum BlendOp {
        /// `src + dst`.
        Add = 0x8006,
        /// `src - dst`.
        Subtract = 0x800A,
        /// `dst - src`.
        ReverseSubtract = 0x800B,
        /// `min(src, dst)`.
        Min = 0x8007,
        /// `max(src, dst)`.
        Max = 0x8008,
        /// Xbox extension: `src + dst - 0.5`-style signed add. Approximated as [`Add`](Self::Add).
        AddSigned = 0xF006,
        /// Xbox extension: signed reverse subtract. Approximated as [`ReverseSubtract`](Self::ReverseSubtract).
        ReverseSubtractSigned = 0xF005,
    }
}

xbox_enum! {
    /// Stencil operation (`D3DSTENCILOP`).
    pub enum StencilOp {
        /// Leave the stored value.
        Keep = 0x1E00,
        /// Set to zero.
        Zero = 0x0000,
        /// Set to the reference value.
        Replace = 0x1E01,
        /// Increment, clamping at the maximum.
        IncrSat = 0x1E02,
        /// Decrement, clamping at zero.
        DecrSat = 0x1E03,
        /// Bitwise invert.
        Invert = 0x150A,
        /// Increment with wraparound.
        Incr = 0x8507,
        /// Decrement with wraparound.
        Decr = 0x8508,
    }
}

xbox_enum! {
    /// `D3DRS_CULLMODE`: which winding is culled.
    pub enum CullMode {
        /// No culling.
        None = 0,
        /// Cull triangles whose winding appears clockwise on screen.
        Cw = 0x900,
        /// Cull triangles whose winding appears counter-clockwise on screen.
        Ccw = 0x901,
    }
}

xbox_enum! {
    /// `D3DRS_FRONTFACE` (Xbox extension): which winding is the "front" face
    /// for two-sided lighting and stencil. Does not affect culling.
    pub enum FrontFace {
        /// Clockwise is front (the Xbox default).
        Cw = 0x900,
        /// Counter-clockwise is front.
        Ccw = 0x901,
    }
}

xbox_enum! {
    /// `D3DRS_FILLMODE` / `D3DRS_BACKFILLMODE`.
    pub enum FillMode {
        /// Vertices only.
        Point = 0x1B00,
        /// Edges only.
        Wireframe = 0x1B01,
        /// Filled triangles.
        Solid = 0x1B02,
    }
}

xbox_enum! {
    /// `D3DRS_SHADEMODE`.
    pub enum ShadeMode {
        /// Provoking-vertex colour across the primitive.
        Flat = 0x1D00,
        /// Interpolated colour.
        Gouraud = 0x1D01,
    }
}

xbox_enum! {
    /// `D3DRS_ZENABLE`.
    pub enum DepthBufferMode {
        /// Depth test and write disabled.
        Disabled = 0,
        /// Z-buffering.
        Z = 1,
        /// W-buffering (`D3DZB_USEW`). Approximated with Z here; the guest-program translator
        /// has translated vertex programs store w (docs/graphics/shaders.md).
        W = 2,
    }
}

/// `D3DRS_COLORWRITEENABLE`: per-channel framebuffer write enables.
///
/// Xbox bit positions are the NV2A's: blue = bit 0, green = bit 8,
/// red = bit 16, alpha = bit 24.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorWriteMask {
    /// Write red.
    pub red: bool,
    /// Write green.
    pub green: bool,
    /// Write blue.
    pub blue: bool,
    /// Write alpha.
    pub alpha: bool,
}

impl ColorWriteMask {
    /// All channels enabled.
    pub const ALL: Self = Self { red: true, green: true, blue: true, alpha: true };

    /// Decodes the raw Xbox bit mask.
    pub fn from_xbox(v: u32) -> Self {
        Self {
            blue: v & (1 << 0) != 0,
            green: v & (1 << 8) != 0,
            red: v & (1 << 16) != 0,
            alpha: v & (1 << 24) != 0,
        }
    }

    /// Encodes to the raw Xbox bit mask.
    pub fn xbox_value(self) -> u32 {
        (self.blue as u32) | ((self.green as u32) << 8) | ((self.red as u32) << 16) | ((self.alpha as u32) << 24)
    }
}

/// Stencil test configuration (`D3DRS_STENCIL*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StencilState {
    /// `D3DRS_STENCILENABLE`.
    pub enable: bool,
    /// `D3DRS_STENCILFUNC`.
    pub func: CompareFunc,
    /// `D3DRS_STENCILREF`.
    pub reference: u8,
    /// `D3DRS_STENCILMASK`: mask applied to ref and buffer before comparing.
    pub read_mask: u8,
    /// `D3DRS_STENCILWRITEMASK`.
    pub write_mask: u8,
    /// `D3DRS_STENCILFAIL`: stencil test failed.
    pub fail: StencilOp,
    /// `D3DRS_STENCILZFAIL`: stencil passed, depth failed.
    pub depth_fail: StencilOp,
    /// `D3DRS_STENCILPASS`: both passed.
    pub pass: StencilOp,
}

impl Default for StencilState {
    fn default() -> Self {
        Self {
            enable: false,
            func: CompareFunc::Always,
            reference: 0,
            read_mask: 0xFF,
            write_mask: 0xFF,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Keep,
        }
    }
}

/// The subset of D3D8 render state this crate models.
///
/// Defaults are the documented D3D8 defaults, with the Xbox specifics
/// (depth testing on, `FrontFace = CW`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderState {
    /// `D3DRS_ZENABLE`.
    pub depth_mode: DepthBufferMode,
    /// `D3DRS_ZWRITEENABLE`.
    pub depth_write: bool,
    /// `D3DRS_ZFUNC`.
    pub depth_func: CompareFunc,
    /// `D3DRS_ALPHABLENDENABLE`.
    pub blend_enable: bool,
    /// `D3DRS_SRCBLEND`.
    pub src_blend: BlendFactor,
    /// `D3DRS_DESTBLEND`.
    pub dst_blend: BlendFactor,
    /// `D3DRS_BLENDOP`.
    pub blend_op: BlendOp,
    /// `D3DRS_BLENDCOLOR`, `0xAARRGGBB`.
    pub blend_color: u32,
    /// `D3DRS_COLORWRITEENABLE`.
    pub color_write: ColorWriteMask,
    /// `D3DRS_ALPHATESTENABLE`. Alpha test is a shader concern in wgpu.
    pub alpha_test_enable: bool,
    /// `D3DRS_ALPHAFUNC`.
    pub alpha_func: CompareFunc,
    /// `D3DRS_ALPHAREF` (0..=255).
    pub alpha_ref: u8,
    /// `D3DRS_CULLMODE`.
    pub cull_mode: CullMode,
    /// `D3DRS_FRONTFACE`.
    pub front_face: FrontFace,
    /// `D3DRS_FILLMODE`.
    pub fill_mode: FillMode,
    /// `D3DRS_BACKFILLMODE`.
    pub back_fill_mode: FillMode,
    /// `D3DRS_SHADEMODE`. Shader concern.
    pub shade_mode: ShadeMode,
    /// `D3DRS_STENCIL*`.
    pub stencil: StencilState,
    /// `D3DRS_POLYGONOFFSETZSLOPESCALE`.
    pub depth_bias_slope_scale: f32,
    /// `D3DRS_POLYGONOFFSETZOFFSET`, in depth-buffer steps (not normalised).
    pub depth_bias: f32,
    /// `D3DRS_POINTOFFSETENABLE`: bias applies to point-filled polygons.
    pub point_offset_enable: bool,
    /// `D3DRS_WIREFRAMEOFFSETENABLE`: bias applies to wireframe polygons.
    pub wireframe_offset_enable: bool,
    /// `D3DRS_SOLIDOFFSETENABLE`: bias applies to filled polygons.
    pub solid_offset_enable: bool,
    /// `D3DRS_FOGENABLE`. Shader concern.
    pub fog_enable: bool,
    /// `D3DRS_FOGCOLOR`, `0xAARRGGBB`.
    pub fog_color: u32,
    /// `D3DRS_TEXTUREFACTOR`, `0xAARRGGBB`.
    pub texture_factor: u32,
    /// `D3DRS_DEPTHCLIPCONTROL` is `D3DDCC_DEPTHCLAMP` ([`clamps_depth`]):
    /// depth outside the clip range is clamped to it, not culled.
    pub depth_clamp: bool,
}

/// Whether a `D3DRS_DEPTHCLIPCONTROL` value clamps out-of-range depth
/// instead of culling it: bits 7:4 = 1 (`D3DDCC_DEPTHCLAMP`), as D3D and
/// Halo's inlined copies push it to `NV097_SET_ZMIN_MAX_CONTROL` (`0x1D78`;
/// xemu's `ZCLAMP_EN`).
pub fn clamps_depth(depth_clip_control: u32) -> bool {
    (depth_clip_control >> 4) & 0xF == 1
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            depth_mode: DepthBufferMode::Z,
            depth_write: true,
            depth_func: CompareFunc::LessEqual,
            blend_enable: false,
            src_blend: BlendFactor::One,
            dst_blend: BlendFactor::Zero,
            blend_op: BlendOp::Add,
            blend_color: 0,
            color_write: ColorWriteMask::ALL,
            alpha_test_enable: false,
            alpha_func: CompareFunc::Always,
            alpha_ref: 0,
            cull_mode: CullMode::Ccw,
            front_face: FrontFace::Cw,
            fill_mode: FillMode::Solid,
            back_fill_mode: FillMode::Solid,
            shade_mode: ShadeMode::Gouraud,
            stencil: StencilState::default(),
            depth_bias_slope_scale: 0.0,
            depth_bias: 0.0,
            point_offset_enable: false,
            wireframe_offset_enable: false,
            solid_offset_enable: false,
            fog_enable: false,
            fog_color: 0,
            texture_factor: 0xFFFF_FFFF,
            depth_clamp: false,
        }
    }
}

/// Render-state slot indices (`X_D3DRS_*`), Cxbx canonical numbering, restricted to
/// the states [`RenderState`] models. Anything else is
/// [`RenderStateType::Other`]: it is accepted and ignored by
/// [`RenderState::set_raw`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)] // names are the XDK names, documented on RenderState's fields
pub enum RenderStateType {
    ZFunc,
    AlphaFunc,
    AlphaBlendEnable,
    AlphaTestEnable,
    AlphaRef,
    SrcBlend,
    DestBlend,
    ZWriteEnable,
    ShadeMode,
    ColorWriteEnable,
    StencilZFail,
    StencilPass,
    StencilFunc,
    StencilRef,
    StencilMask,
    StencilWriteMask,
    BlendOp,
    BlendColor,
    PointOffsetEnable,
    WireframeOffsetEnable,
    SolidOffsetEnable,
    PolygonOffsetZSlopeScale,
    PolygonOffsetZOffset,
    FogEnable,
    FogColor,
    FillMode,
    BackFillMode,
    ZEnable,
    StencilEnable,
    StencilFail,
    FrontFace,
    CullMode,
    TextureFactor,
    DepthClipControl,
    /// A slot this model does not track (pixel-shader registers, lighting, ...).
    Other(u32),
}

impl RenderStateType {
    /// Maps a *canonical* index to a typed state. For a guest slot use
    /// [`from_guest_index`](Self::from_guest_index).
    pub fn from_index(i: u32) -> Self {
        use RenderStateType::*;
        match i {
            57 => ZFunc,
            58 => AlphaFunc,
            59 => AlphaBlendEnable,
            60 => AlphaTestEnable,
            61 => AlphaRef,
            62 => SrcBlend,
            63 => DestBlend,
            64 => ZWriteEnable,
            66 => ShadeMode,
            67 => ColorWriteEnable,
            68 => StencilZFail,
            69 => StencilPass,
            70 => StencilFunc,
            71 => StencilRef,
            72 => StencilMask,
            73 => StencilWriteMask,
            74 => BlendOp,
            75 => BlendColor,
            79 => PointOffsetEnable,
            80 => WireframeOffsetEnable,
            81 => SolidOffsetEnable,
            77 => PolygonOffsetZSlopeScale,
            78 => PolygonOffsetZOffset,
            92 => FogEnable,
            138 => FogColor,
            139 => FillMode,
            140 => BackFillMode,
            143 => ZEnable,
            144 => StencilEnable,
            145 => StencilFail,
            146 => FrontFace,
            147 => CullMode,
            148 => TextureFactor,
            82 => DepthClipControl,
            other => Other(other),
        }
    }

    /// Maps a guest slot of the build described by `table` to a typed state;
    /// `None` if the slot does not exist in that build.
    pub fn from_guest_index(table: &crate::xdk::RenderStateTable, guest: u32) -> Option<Self> {
        table.guest_to_canonical(guest).map(Self::from_index)
    }

    /// The canonical index.
    pub fn index(self) -> u32 {
        use RenderStateType::*;
        match self {
            ZFunc => 57,
            AlphaFunc => 58,
            AlphaBlendEnable => 59,
            AlphaTestEnable => 60,
            AlphaRef => 61,
            SrcBlend => 62,
            DestBlend => 63,
            ZWriteEnable => 64,
            ShadeMode => 66,
            ColorWriteEnable => 67,
            StencilZFail => 68,
            StencilPass => 69,
            StencilFunc => 70,
            StencilRef => 71,
            StencilMask => 72,
            StencilWriteMask => 73,
            BlendOp => 74,
            BlendColor => 75,
            PointOffsetEnable => 79,
            WireframeOffsetEnable => 80,
            SolidOffsetEnable => 81,
            PolygonOffsetZSlopeScale => 77,
            PolygonOffsetZOffset => 78,
            FogEnable => 92,
            FogColor => 138,
            FillMode => 139,
            BackFillMode => 140,
            ZEnable => 143,
            StencilEnable => 144,
            StencilFail => 145,
            FrontFace => 146,
            CullMode => 147,
            TextureFactor => 148,
            DepthClipControl => 82,
            Other(i) => i,
        }
    }
}

/// A raw render-state value that is not a member of the state's enumeration.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid value 0x{value:x} for render state {state:?}")]
pub struct InvalidStateValue {
    /// The state being set.
    pub state: RenderStateType,
    /// The offending raw value.
    pub value: u32,
}

impl RenderState {
    /// Applies a raw write to guest render-state slot `guest` of the build
    /// described by `table` (the HLE entry point).
    pub fn set_guest(&mut self, table: &crate::xdk::RenderStateTable, guest: u32, value: u32) -> Result<(), InvalidStateValue> {
        match RenderStateType::from_guest_index(table, guest) {
            Some(state) => self.set_raw(state, value),
            None => Ok(()), // slot beyond this build's array: nothing to do
        }
    }

    /// Applies a raw write of `value` to render state `state`.
    ///
    /// Untracked slots succeed without effect so callers can forward the
    /// guest's whole state stream.
    pub fn set_raw(&mut self, state: RenderStateType, value: u32) -> Result<(), InvalidStateValue> {
        use RenderStateType as R;
        let bad = || InvalidStateValue { state, value };
        let b = value != 0;
        let f = f32::from_bits(value);
        match state {
            R::ZFunc => self.depth_func = CompareFunc::from_xbox_or_pc(value).ok_or_else(bad)?,
            R::AlphaFunc => self.alpha_func = CompareFunc::from_xbox_or_pc(value).ok_or_else(bad)?,
            R::AlphaBlendEnable => self.blend_enable = b,
            R::AlphaTestEnable => self.alpha_test_enable = b,
            R::AlphaRef => self.alpha_ref = value as u8,
            R::SrcBlend => self.src_blend = BlendFactor::from_xbox(value).ok_or_else(bad)?,
            R::DestBlend => self.dst_blend = BlendFactor::from_xbox(value).ok_or_else(bad)?,
            R::ZWriteEnable => self.depth_write = b,
            R::ShadeMode => self.shade_mode = ShadeMode::from_xbox(value).ok_or_else(bad)?,
            R::ColorWriteEnable => self.color_write = ColorWriteMask::from_xbox(value),
            R::StencilZFail => self.stencil.depth_fail = StencilOp::from_xbox(value).ok_or_else(bad)?,
            R::StencilPass => self.stencil.pass = StencilOp::from_xbox(value).ok_or_else(bad)?,
            R::StencilFail => self.stencil.fail = StencilOp::from_xbox(value).ok_or_else(bad)?,
            R::StencilFunc => self.stencil.func = CompareFunc::from_xbox_or_pc(value).ok_or_else(bad)?,
            R::StencilRef => self.stencil.reference = value as u8,
            R::StencilMask => self.stencil.read_mask = value as u8,
            R::StencilWriteMask => self.stencil.write_mask = value as u8,
            R::StencilEnable => self.stencil.enable = b,
            R::BlendOp => self.blend_op = BlendOp::from_xbox(value).ok_or_else(bad)?,
            R::BlendColor => self.blend_color = value,
            R::PointOffsetEnable => self.point_offset_enable = b,
            R::WireframeOffsetEnable => self.wireframe_offset_enable = b,
            R::SolidOffsetEnable => self.solid_offset_enable = b,
            R::PolygonOffsetZSlopeScale => self.depth_bias_slope_scale = f,
            R::PolygonOffsetZOffset => self.depth_bias = f,
            R::FogEnable => self.fog_enable = b,
            R::FogColor => self.fog_color = value,
            R::FillMode => self.fill_mode = FillMode::from_xbox(value).ok_or_else(bad)?,
            R::BackFillMode => self.back_fill_mode = FillMode::from_xbox(value).ok_or_else(bad)?,
            R::ZEnable => self.depth_mode = DepthBufferMode::from_xbox(value).ok_or_else(bad)?,
            R::FrontFace => self.front_face = FrontFace::from_xbox(value).ok_or_else(bad)?,
            R::CullMode => self.cull_mode = CullMode::from_xbox(value).ok_or_else(bad)?,
            R::TextureFactor => self.texture_factor = value,
            R::DepthClipControl => self.depth_clamp = clamps_depth(value),
            R::Other(_) => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Texture stage state
// ---------------------------------------------------------------------------

xbox_enum! {
    /// `D3DTEXTUREADDRESS`.
    pub enum TextureAddress {
        /// Tile.
        Wrap = 1,
        /// Mirror at every integer boundary.
        Mirror = 2,
        /// Clamp to [0, 1].
        Clamp = 3,
        /// Use the border colour outside [0, 1].
        Border = 4,
        /// Clamp to the edge texel.
        ClampToEdge = 5,
    }
}

xbox_enum! {
    /// `D3DTEXTUREFILTERTYPE`.
    pub enum TextureFilter {
        /// No filtering (only valid for the mip filter: disables mipmapping).
        None = 0,
        /// Nearest.
        Point = 1,
        /// Bilinear.
        Linear = 2,
        /// Anisotropic.
        Anisotropic = 3,
        /// Xbox: 2x2 quincunx filter. Approximated as linear.
        Quincunx = 4,
        /// Xbox: gaussian cubic filter. Approximated as linear.
        GaussianCubic = 5,
    }
}

xbox_enum! {
    /// `D3DTEXTUREOP`: fixed-function texture-stage combiner operation.
    pub enum TextureOp {
        /// Stage and all later stages are disabled.
        Disable = 1,
        /// `arg1`.
        SelectArg1 = 2,
        /// `arg2`.
        SelectArg2 = 3,
        /// `arg1 * arg2`.
        Modulate = 4,
        /// `(arg1 * arg2) << 1`.
        Modulate2x = 5,
        /// `(arg1 * arg2) << 2`.
        Modulate4x = 6,
        /// `arg1 + arg2`.
        Add = 7,
        /// `arg1 + arg2 - 0.5`.
        AddSigned = 8,
        /// `(arg1 + arg2 - 0.5) << 1`.
        AddSigned2x = 9,
        /// `arg1 - arg2`.
        Subtract = 10,
        /// `arg1 + arg2 - arg1 * arg2`.
        AddSmooth = 11,
        /// Blend by diffuse alpha.
        BlendDiffuseAlpha = 12,
        /// Blend by current alpha.
        BlendCurrentAlpha = 13,
        /// Blend by texture alpha.
        BlendTextureAlpha = 14,
        /// Blend by `D3DRS_TEXTUREFACTOR` alpha.
        BlendFactorAlpha = 15,
        /// Premultiplied-alpha blend by texture alpha.
        BlendTextureAlphaPm = 16,
        /// Modulate with the next stage's texture (Xbox: unsupported).
        PreModulate = 17,
        /// `arg1.rgb + arg1.a * arg2.rgb`.
        ModulateAlphaAddColor = 18,
        /// `arg1.rgb * arg2.rgb + arg1.a`.
        ModulateColorAddAlpha = 19,
        /// `(1 - arg1.a) * arg2.rgb + arg1.rgb`.
        ModulateInvAlphaAddColor = 20,
        /// `(1 - arg1.rgb) * arg2.rgb + arg1.a`.
        ModulateInvColorAddAlpha = 21,
        /// `dot3(arg1, arg2)`.
        DotProduct3 = 22,
        /// `arg1 + arg2 * arg0`.
        MultiplyAdd = 23,
        /// `arg0 * arg1 + (1 - arg0) * arg2`.
        Lerp = 24,
        /// Bump-environment mapping.
        BumpEnvMap = 25,
        /// Bump-environment mapping with luminance.
        BumpEnvMapLuminance = 26,
    }
}

/// Source of a texture-stage argument (`D3DTA_*` low bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureArgSource {
    /// Interpolated vertex diffuse colour.
    Diffuse,
    /// Output of the previous stage.
    Current,
    /// This stage's texture sample.
    Texture,
    /// `D3DRS_TEXTUREFACTOR`.
    TFactor,
    /// Interpolated vertex specular colour.
    Specular,
    /// Temporary register.
    Temp,
}

/// A texture-stage argument with its `D3DTA_COMPLEMENT` / `D3DTA_ALPHAREPLICATE`
/// modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureArg {
    /// Where the value comes from.
    pub source: TextureArgSource,
    /// Use `1 - value`.
    pub complement: bool,
    /// Replicate the alpha channel into all channels.
    pub alpha_replicate: bool,
}

impl TextureArg {
    /// `D3DTA_COMPLEMENT`.
    pub const COMPLEMENT: u32 = 0x10;
    /// `D3DTA_ALPHAREPLICATE`.
    pub const ALPHA_REPLICATE: u32 = 0x20;

    /// A plain, unmodified argument.
    pub const fn plain(source: TextureArgSource) -> Self {
        Self { source, complement: false, alpha_replicate: false }
    }

    /// Decodes a raw `D3DTA_*` word.
    pub fn from_xbox(v: u32) -> Option<Self> {
        let source = match v & 0xF {
            0 => TextureArgSource::Diffuse,
            1 => TextureArgSource::Current,
            2 => TextureArgSource::Texture,
            3 => TextureArgSource::TFactor,
            4 => TextureArgSource::Specular,
            5 => TextureArgSource::Temp,
            _ => return None,
        };
        if v & !(0xF | Self::COMPLEMENT | Self::ALPHA_REPLICATE) != 0 {
            return None;
        }
        Some(Self {
            source,
            complement: v & Self::COMPLEMENT != 0,
            alpha_replicate: v & Self::ALPHA_REPLICATE != 0,
        })
    }

    /// Encodes back to the raw word.
    pub fn xbox_value(self) -> u32 {
        let s = match self.source {
            TextureArgSource::Diffuse => 0,
            TextureArgSource::Current => 1,
            TextureArgSource::Texture => 2,
            TextureArgSource::TFactor => 3,
            TextureArgSource::Specular => 4,
            TextureArgSource::Temp => 5,
        };
        s | if self.complement { Self::COMPLEMENT } else { 0 }
            | if self.alpha_replicate { Self::ALPHA_REPLICATE } else { 0 }
    }
}

/// Texture-stage slot indices (`X_D3DTSS_*`), the subset modelled here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum TextureStageStateType {
    AddressU,
    AddressV,
    AddressW,
    MagFilter,
    MinFilter,
    MipFilter,
    MipMapLodBias,
    MaxMipLevel,
    MaxAnisotropy,
    ColorOp,
    ColorArg0,
    ColorArg1,
    ColorArg2,
    AlphaOp,
    AlphaArg0,
    AlphaArg1,
    AlphaArg2,
    ResultArg,
    TexCoordIndex,
    BorderColor,
    /// Untracked slot (colour-key, bump matrices, transform flags, ...).
    Other(u32),
}

impl TextureStageStateType {
    /// Maps an XDK `X_D3DTSS_*` index to a typed state.
    pub fn from_index(i: u32) -> Self {
        use TextureStageStateType::*;
        match i {
            0 => AddressU,
            1 => AddressV,
            2 => AddressW,
            3 => MagFilter,
            4 => MinFilter,
            5 => MipFilter,
            6 => MipMapLodBias,
            7 => MaxMipLevel,
            8 => MaxAnisotropy,
            12 => ColorOp,
            13 => ColorArg0,
            14 => ColorArg1,
            15 => ColorArg2,
            16 => AlphaOp,
            17 => AlphaArg0,
            18 => AlphaArg1,
            19 => AlphaArg2,
            20 => ResultArg,
            28 => TexCoordIndex,
            29 => BorderColor,
            o => Other(o),
        }
    }
}

/// Per-stage fixed-function and sampler state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureStageState {
    /// `D3DTSS_ADDRESSU`.
    pub address_u: TextureAddress,
    /// `D3DTSS_ADDRESSV`.
    pub address_v: TextureAddress,
    /// `D3DTSS_ADDRESSW`.
    pub address_w: TextureAddress,
    /// `D3DTSS_MAGFILTER`.
    pub mag_filter: TextureFilter,
    /// `D3DTSS_MINFILTER`.
    pub min_filter: TextureFilter,
    /// `D3DTSS_MIPFILTER`.
    pub mip_filter: TextureFilter,
    /// `D3DTSS_MIPMAPLODBIAS`.
    pub mip_lod_bias: f32,
    /// `D3DTSS_MAXMIPLEVEL`: highest-resolution level used (0 = base).
    pub max_mip_level: u32,
    /// `D3DTSS_MAXANISOTROPY`.
    pub max_anisotropy: u32,
    /// `D3DTSS_COLOROP`.
    pub color_op: TextureOp,
    /// `D3DTSS_COLORARG0`.
    pub color_arg0: TextureArg,
    /// `D3DTSS_COLORARG1`.
    pub color_arg1: TextureArg,
    /// `D3DTSS_COLORARG2`.
    pub color_arg2: TextureArg,
    /// `D3DTSS_ALPHAOP`.
    pub alpha_op: TextureOp,
    /// `D3DTSS_ALPHAARG0`.
    pub alpha_arg0: TextureArg,
    /// `D3DTSS_ALPHAARG1`.
    pub alpha_arg1: TextureArg,
    /// `D3DTSS_ALPHAARG2`.
    pub alpha_arg2: TextureArg,
    /// `D3DTSS_RESULTARG`.
    pub result_arg: TextureArg,
    /// `D3DTSS_TEXCOORDINDEX` (low 16 bits; high bits are generation flags).
    pub tex_coord_index: u32,
    /// `D3DTSS_BORDERCOLOR`, `0xAARRGGBB`.
    pub border_color: u32,
}

impl TextureStageState {
    /// D3D8 defaults for stage `n`: stage 0 modulates texture by diffuse,
    /// later stages are disabled.
    pub fn default_for_stage(n: u32) -> Self {
        use TextureArgSource::*;
        Self {
            address_u: TextureAddress::Wrap,
            address_v: TextureAddress::Wrap,
            address_w: TextureAddress::Wrap,
            mag_filter: TextureFilter::Point,
            min_filter: TextureFilter::Point,
            mip_filter: TextureFilter::None,
            mip_lod_bias: 0.0,
            max_mip_level: 0,
            max_anisotropy: 1,
            color_op: if n == 0 { TextureOp::Modulate } else { TextureOp::Disable },
            color_arg0: TextureArg::plain(Current),
            color_arg1: TextureArg::plain(Texture),
            color_arg2: TextureArg::plain(Current),
            alpha_op: if n == 0 { TextureOp::SelectArg1 } else { TextureOp::Disable },
            alpha_arg0: TextureArg::plain(Current),
            alpha_arg1: TextureArg::plain(Texture),
            alpha_arg2: TextureArg::plain(Current),
            result_arg: TextureArg::plain(Current),
            tex_coord_index: n,
            border_color: 0,
        }
    }

    /// Applies a raw guest write.
    pub fn set_raw(&mut self, state: TextureStageStateType, value: u32) -> Result<(), (TextureStageStateType, u32)> {
        use TextureStageStateType as T;
        let bad = || (state, value);
        match state {
            T::AddressU => self.address_u = TextureAddress::from_xbox(value).ok_or_else(bad)?,
            T::AddressV => self.address_v = TextureAddress::from_xbox(value).ok_or_else(bad)?,
            T::AddressW => self.address_w = TextureAddress::from_xbox(value).ok_or_else(bad)?,
            T::MagFilter => self.mag_filter = TextureFilter::from_xbox(value).ok_or_else(bad)?,
            T::MinFilter => self.min_filter = TextureFilter::from_xbox(value).ok_or_else(bad)?,
            T::MipFilter => self.mip_filter = TextureFilter::from_xbox(value).ok_or_else(bad)?,
            T::MipMapLodBias => self.mip_lod_bias = f32::from_bits(value),
            T::MaxMipLevel => self.max_mip_level = value,
            T::MaxAnisotropy => self.max_anisotropy = value.max(1),
            T::ColorOp => self.color_op = TextureOp::from_xbox(value).ok_or_else(bad)?,
            T::AlphaOp => self.alpha_op = TextureOp::from_xbox(value).ok_or_else(bad)?,
            T::ColorArg0 => self.color_arg0 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::ColorArg1 => self.color_arg1 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::ColorArg2 => self.color_arg2 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::AlphaArg0 => self.alpha_arg0 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::AlphaArg1 => self.alpha_arg1 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::AlphaArg2 => self.alpha_arg2 = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::ResultArg => self.result_arg = TextureArg::from_xbox(value).ok_or_else(bad)?,
            T::TexCoordIndex => self.tex_coord_index = value,
            T::BorderColor => self.border_color = value,
            T::Other(_) => {}
        }
        Ok(())
    }
}

/// Which attachments a pass clears (D3D `Clear` flags, typed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clear {
    /// Clear colour (`D3DCLEAR_TARGET`).
    pub color: Option<[f32; 4]>,
    /// Clear depth (`D3DCLEAR_ZBUFFER`).
    pub depth: Option<f32>,
    /// Clear stencil (`D3DCLEAR_STENCIL`).
    pub stencil: Option<u8>,
}

impl Clear {
    /// Clear nothing.
    pub const NONE: Self = Self {
        color: None,
        depth: None,
        stencil: None,
    };

    /// Clear everything: `color`, depth 1.0, stencil 0.
    pub fn all(color: [f32; 4]) -> Self {
        Self {
            color: Some(color),
            depth: Some(1.0),
            stencil: Some(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbox_values_match_nv2a_constants() {
        assert_eq!(BlendFactor::SrcAlpha.xbox_value(), 0x302);
        assert_eq!(BlendFactor::InvSrcAlpha.xbox_value(), 0x303);
        assert_eq!(BlendFactor::SrcAlphaSat.xbox_value(), 0x308);
        assert_eq!(BlendFactor::ConstantColor.xbox_value(), 0x8001);
        assert_eq!(BlendOp::Add.xbox_value(), 0x8006);
        assert_eq!(BlendOp::ReverseSubtract.xbox_value(), 0x800B);
        assert_eq!(StencilOp::Keep.xbox_value(), 0x1E00);
        assert_eq!(StencilOp::Invert.xbox_value(), 0x150A);
        assert_eq!(StencilOp::Decr.xbox_value(), 0x8508);
        assert_eq!(CullMode::Cw.xbox_value(), 0x900);
        assert_eq!(CullMode::Ccw.xbox_value(), 0x901);
        assert_eq!(FillMode::Wireframe.xbox_value(), 0x1B01);
        assert_eq!(CompareFunc::LessEqual.xbox_value(), 0x203);
    }

    #[test]
    fn enums_roundtrip() {
        for v in 0..0x10000u32 {
            if let Some(f) = BlendFactor::from_xbox(v) {
                assert_eq!(f.xbox_value(), v);
            }
            if let Some(f) = StencilOp::from_xbox(v) {
                assert_eq!(f.xbox_value(), v);
            }
            if let Some(f) = CompareFunc::from_xbox(v) {
                assert_eq!(f.xbox_value(), v);
            }
        }
        assert_eq!(BlendFactor::from_xbox(0x309), None);
        assert_eq!(CullMode::from_xbox(2), None);
    }

    #[test]
    fn compare_accepts_pc_numbering() {
        assert_eq!(CompareFunc::from_xbox_or_pc(1), Some(CompareFunc::Never));
        assert_eq!(CompareFunc::from_xbox_or_pc(4), Some(CompareFunc::LessEqual));
        assert_eq!(CompareFunc::from_xbox_or_pc(8), Some(CompareFunc::Always));
        assert_eq!(CompareFunc::from_xbox_or_pc(0), None);
    }

    #[test]
    fn color_write_mask_bits() {
        let m = ColorWriteMask::from_xbox((1 << 16) | (1 << 24));
        assert_eq!(m, ColorWriteMask { red: true, green: false, blue: false, alpha: true });
        assert_eq!(m.xbox_value(), (1 << 16) | (1 << 24));
        assert_eq!(ColorWriteMask::from_xbox(0x0101_0101), ColorWriteMask::ALL);
    }

    #[test]
    fn set_raw_updates_typed_state() {
        let mut rs = RenderState::default();
        rs.set_raw(RenderStateType::from_index(59), 1).unwrap();
        rs.set_raw(RenderStateType::from_index(62), 0x302).unwrap();
        rs.set_raw(RenderStateType::from_index(63), 0x303).unwrap();
        rs.set_raw(RenderStateType::from_index(147), 0x900).unwrap();
        rs.set_raw(RenderStateType::from_index(77), 2.5f32.to_bits()).unwrap();
        rs.set_raw(RenderStateType::from_index(9999), 5).unwrap(); // ignored
        // Guest slots go through the build table (Halo 2: 5849).
        let t = crate::xdk::RenderStateTable::default();
        rs.set_guest(&t, 64, 0).unwrap(); // ZWRITEENABLE
        assert!(!rs.depth_write);
        rs.set_guest(&t, 100_000, 1).unwrap(); // beyond the array: ignored
        assert!(rs.blend_enable);
        assert_eq!(rs.src_blend, BlendFactor::SrcAlpha);
        assert_eq!(rs.dst_blend, BlendFactor::InvSrcAlpha);
        assert_eq!(rs.cull_mode, CullMode::Cw);
        assert_eq!(rs.depth_bias_slope_scale, 2.5);
    }

    #[test]
    fn set_raw_rejects_bad_values() {
        let mut rs = RenderState::default();
        let e = rs.set_raw(RenderStateType::SrcBlend, 0x1234).unwrap_err();
        assert_eq!(e, InvalidStateValue { state: RenderStateType::SrcBlend, value: 0x1234 });
        assert_eq!(rs, RenderState::default());
    }

    #[test]
    fn render_state_index_roundtrip() {
        for i in 0..200 {
            assert_eq!(RenderStateType::from_index(i).index(), i);
        }
    }

    #[test]
    fn texture_arg_modifiers() {
        let a = TextureArg::from_xbox(2 | TextureArg::COMPLEMENT | TextureArg::ALPHA_REPLICATE).unwrap();
        assert_eq!(a.source, TextureArgSource::Texture);
        assert!(a.complement && a.alpha_replicate);
        assert_eq!(a.xbox_value(), 0x32);
        assert_eq!(TextureArg::from_xbox(0xF), None);
        assert_eq!(TextureArg::from_xbox(0x100), None);
    }

    #[test]
    fn depth_clip_control_sets_depth_clamp() {
        assert_eq!(
            RenderStateType::from_index(82),
            RenderStateType::DepthClipControl
        );
        assert_eq!(RenderStateType::DepthClipControl.index(), 82);
        let mut s = RenderState::default();
        assert!(!s.depth_clamp);
        // D3DDCC_DEPTHCLAMP is bits 7:4 = 1; CULLPRIMITIVE (0) and
        // CLIPPLANES (bit 0) alone do not clamp.
        for (value, clamp) in [
            (0x10, true),
            (0x11, true),
            (0, false),
            (1, false),
            (0x20, false),
            (0x30, false),
        ] {
            s.set_raw(RenderStateType::DepthClipControl, value).unwrap();
            assert_eq!(s.depth_clamp, clamp, "{value:#x}");
        }
    }

    #[test]
    fn stage_defaults_and_raw_set() {
        let s0 = TextureStageState::default_for_stage(0);
        let s1 = TextureStageState::default_for_stage(1);
        assert_eq!(s0.color_op, TextureOp::Modulate);
        assert_eq!(s1.color_op, TextureOp::Disable);
        assert_eq!(s1.tex_coord_index, 1);
        let mut s = s0;
        s.set_raw(TextureStageStateType::from_index(0), 3).unwrap();
        s.set_raw(TextureStageStateType::from_index(12), 7).unwrap();
        assert_eq!(s.address_u, TextureAddress::Clamp);
        assert_eq!(s.color_op, TextureOp::Add);
        assert!(s.set_raw(TextureStageStateType::AddressV, 0).is_err());
    }
}
