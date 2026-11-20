//! Primitive types, fixed-function vertex formats (FVF) and Xbox vertex
//! shader declarations (`D3DVSD_*` token streams).
//!
//! Constants come from Cxbx-Reloaded `XbD3D8Types.h` (`X_D3DPT_*`,
//! `X_D3DFVF_*`, `X_D3DVSDE_*`, `X_D3DVSDT_*`, `X_D3DVSD_*`) at commit
//! `585c49a50af1255ab155099e06f24505f9c5a800`; the data-type nibbles match
//! xemu's `NV097_SET_VERTEX_DATA_ARRAY_FORMAT_TYPE_*` (`nv2a_regs.h`,
//! `4bdad199102b22ed0cfa998e9af6b35d76238b3e`): high nibble = component
//! count, low nibble = encoding.

use std::fmt;

// ---------------------------------------------------------------------------
// Primitive types
// ---------------------------------------------------------------------------

/// Xbox `D3DPRIMITIVETYPE`.
///
/// The Xbox adds line loops, quad lists/strips and polygons to the PC set.
/// wgpu only has points, lines, line strips, triangle lists and triangle
/// strips, so the rest are rewritten by [`PrimitiveType::to_host`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    /// `D3DPT_POINTLIST` (1).
    PointList,
    /// `D3DPT_LINELIST` (2).
    LineList,
    /// `D3DPT_LINELOOP` (3), Xbox only.
    LineLoop,
    /// `D3DPT_LINESTRIP` (4).
    LineStrip,
    /// `D3DPT_TRIANGLELIST` (5).
    TriangleList,
    /// `D3DPT_TRIANGLESTRIP` (6).
    TriangleStrip,
    /// `D3DPT_TRIANGLEFAN` (7).
    TriangleFan,
    /// `D3DPT_QUADLIST` (8), Xbox only. Each 4 vertices form a quad.
    QuadList,
    /// `D3DPT_QUADSTRIP` (9), Xbox only. Vertices `(2i, 2i+1, 2i+3, 2i+2)` form quad `i`.
    QuadStrip,
    /// `D3DPT_POLYGON` (10), Xbox only. A convex polygon, drawn as a fan.
    Polygon,
}

#[cfg(feature = "gpu")]
/// How a guest primitive is submitted to wgpu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPrimitive {
    /// Draw the vertices as-is with this topology.
    Native(wgpu::PrimitiveTopology),
    /// Draw with this topology using the supplied `u32` indices (relative to
    /// the first vertex).
    Indexed(wgpu::PrimitiveTopology, Vec<u32>),
}

impl PrimitiveType {
    /// Decodes the raw Xbox value (`0` and values above 10 are invalid).
    pub fn from_xbox(v: u32) -> Option<Self> {
        Some(match v {
            1 => Self::PointList,
            2 => Self::LineList,
            3 => Self::LineLoop,
            4 => Self::LineStrip,
            5 => Self::TriangleList,
            6 => Self::TriangleStrip,
            7 => Self::TriangleFan,
            8 => Self::QuadList,
            9 => Self::QuadStrip,
            10 => Self::Polygon,
            _ => return None,
        })
    }

    /// The raw Xbox value.
    pub fn xbox_value(self) -> u32 {
        match self {
            Self::PointList => 1,
            Self::LineList => 2,
            Self::LineLoop => 3,
            Self::LineStrip => 4,
            Self::TriangleList => 5,
            Self::TriangleStrip => 6,
            Self::TriangleFan => 7,
            Self::QuadList => 8,
            Self::QuadStrip => 9,
            Self::Polygon => 10,
        }
    }

    #[cfg(feature = "gpu")]
    /// The topology of the *translated* draw (what the pipeline is built for).
    pub fn host_topology(self) -> wgpu::PrimitiveTopology {
        use wgpu::PrimitiveTopology as T;
        match self {
            Self::PointList => T::PointList,
            Self::LineList => T::LineList,
            Self::LineStrip | Self::LineLoop => T::LineStrip,
            Self::TriangleStrip => T::TriangleStrip,
            Self::TriangleList
            | Self::TriangleFan
            | Self::QuadList
            | Self::QuadStrip
            | Self::Polygon => T::TriangleList,
        }
    }

    /// How many primitives `vertex_count` vertices make (Xbox rules:
    /// partial primitives are dropped).
    pub fn primitive_count(self, vertex_count: u32) -> u32 {
        let n = vertex_count;
        match self {
            Self::PointList => n,
            Self::LineList => n / 2,
            Self::LineLoop => if n >= 2 { n } else { 0 },
            Self::LineStrip => n.saturating_sub(1),
            Self::TriangleList => n / 3,
            Self::TriangleStrip | Self::TriangleFan | Self::Polygon => n.saturating_sub(2),
            Self::QuadList => n / 4,
            Self::QuadStrip => (n / 2).saturating_sub(1),
        }
    }

    #[cfg(feature = "gpu")]
    /// Plans the wgpu submission for `vertex_count` vertices.
    ///
    /// Natively supported types need no indices; the others return an index
    /// list rewriting them to a triangle list / line strip.
    pub fn to_host(self, vertex_count: u32) -> HostPrimitive {
        let topo = self.host_topology();
        let n = vertex_count;
        let idx: Vec<u32> = match self {
            Self::PointList | Self::LineList | Self::LineStrip | Self::TriangleList | Self::TriangleStrip => {
                return HostPrimitive::Native(topo);
            }
            Self::LineLoop => {
                if n < 2 {
                    Vec::new()
                } else {
                    (0..n).chain(std::iter::once(0)).collect()
                }
            }
            Self::TriangleFan | Self::Polygon => (1..n.saturating_sub(1)).flat_map(|i| [0, i, i + 1]).collect(),
            Self::QuadList => (0..n / 4)
                .flat_map(|q| {
                    let b = q * 4;
                    [b, b + 1, b + 2, b, b + 2, b + 3]
                })
                .collect(),
            Self::QuadStrip => (0..self.primitive_count(n))
                .flat_map(|q| {
                    let b = q * 2;
                    // Quad corners in order: b, b+1, b+3, b+2.
                    [b, b + 1, b + 3, b, b + 3, b + 2]
                })
                .collect(),
        };
        HostPrimitive::Indexed(topo, idx)
    }
}

// ---------------------------------------------------------------------------
// Vertex data types and registers
// ---------------------------------------------------------------------------

/// Vertex input register (`D3DVSDE_*`): the Xbox vertex shader's `v0..v12`.
/// The register number is used directly as the wgpu shader location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VertexRegister(pub u8);

impl VertexRegister {
    /// `D3DVSDE_POSITION`.
    pub const POSITION: Self = Self(0);
    /// `D3DVSDE_BLENDWEIGHT`.
    pub const BLEND_WEIGHT: Self = Self(1);
    /// `D3DVSDE_NORMAL`.
    pub const NORMAL: Self = Self(2);
    /// `D3DVSDE_DIFFUSE`.
    pub const DIFFUSE: Self = Self(3);
    /// `D3DVSDE_SPECULAR`.
    pub const SPECULAR: Self = Self(4);
    /// `D3DVSDE_FOG` (Xbox).
    pub const FOG: Self = Self(5);
    /// `D3DVSDE_POINTSIZE`.
    pub const POINT_SIZE: Self = Self(6);
    /// `D3DVSDE_BACKDIFFUSE` (Xbox).
    pub const BACK_DIFFUSE: Self = Self(7);
    /// `D3DVSDE_BACKSPECULAR` (Xbox).
    pub const BACK_SPECULAR: Self = Self(8);
    /// `D3DVSDE_TEXCOORD0`.
    pub const TEXCOORD0: Self = Self(9);

    /// `D3DVSDE_TEXCOORDn` for `n` in `0..4`.
    pub const fn texcoord(n: u8) -> Self {
        Self(9 + n)
    }
}

/// Xbox vertex attribute data type (`D3DVSDT_*`).
///
/// The raw value is `components << 4 | encoding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexDataType {
    /// `FLOAT1` (0x12): `(x, 0, 0, 1)`.
    Float1,
    /// `FLOAT2` (0x22).
    Float2,
    /// `FLOAT3` (0x32).
    Float3,
    /// `FLOAT4` (0x42).
    Float4,
    /// `D3DCOLOR` (0x40): 4 unsigned bytes in `B, G, R, A` order, normalised.
    D3dColor,
    /// `SHORT1` (0x15).
    Short1,
    /// `SHORT2` (0x25).
    Short2,
    /// `SHORT3` (0x35).
    Short3,
    /// `SHORT4` (0x45).
    Short4,
    /// `NORMSHORT1` (0x11).
    NormShort1,
    /// `NORMSHORT2` (0x21).
    NormShort2,
    /// `NORMSHORT3` (0x31).
    NormShort3,
    /// `NORMSHORT4` (0x41).
    NormShort4,
    /// `NORMPACKED3` (0x16): 11:11:10 signed normalised in 32 bits.
    NormPacked3,
    /// `PBYTE1` (0x14).
    PByte1,
    /// `PBYTE2` (0x24).
    PByte2,
    /// `PBYTE3` (0x34).
    PByte3,
    /// `PBYTE4` (0x44): 4 unsigned bytes in `R, G, B, A` order, normalised.
    PByte4,
    /// `FLOAT2H` (0x72): `(x, y, 0, w)`: three floats.
    Float2H,
}

impl VertexDataType {
    /// Decodes a raw `D3DVSDT_*` value (`NONE` = 0x02 and unknowns give `None`).
    pub fn from_xbox(v: u32) -> Option<Self> {
        Some(match v {
            0x12 => Self::Float1,
            0x22 => Self::Float2,
            0x32 => Self::Float3,
            0x42 => Self::Float4,
            0x40 => Self::D3dColor,
            0x15 => Self::Short1,
            0x25 => Self::Short2,
            0x35 => Self::Short3,
            0x45 => Self::Short4,
            0x11 => Self::NormShort1,
            0x21 => Self::NormShort2,
            0x31 => Self::NormShort3,
            0x41 => Self::NormShort4,
            0x16 => Self::NormPacked3,
            0x14 => Self::PByte1,
            0x24 => Self::PByte2,
            0x34 => Self::PByte3,
            0x44 => Self::PByte4,
            0x72 => Self::Float2H,
            _ => return None,
        })
    }

    /// Size of one element in guest memory, in bytes.
    pub fn byte_size(self) -> u32 {
        match self {
            Self::Float1 => 4,
            Self::Float2 => 8,
            Self::Float3 | Self::Float2H => 12,
            Self::Float4 => 16,
            Self::D3dColor | Self::NormPacked3 | Self::PByte4 => 4,
            Self::Short1 | Self::NormShort1 => 2,
            Self::Short2 | Self::NormShort2 => 4,
            Self::Short3 | Self::NormShort3 => 6,
            Self::Short4 | Self::NormShort4 => 8,
            Self::PByte1 => 1,
            Self::PByte2 => 2,
            Self::PByte3 => 3,
        }
    }

    #[cfg(feature = "gpu")]
    /// The wgpu vertex format that reads this type directly, if one exists.
    ///
    /// `SHORTn` has no entry even for 2 and 4 components: wgpu would present
    /// it to the shader as integers, while Xbox shaders expect floats. Types
    /// returning `None` are rewritten by [`crate::repack`].
    pub fn wgpu_format(self) -> Option<wgpu::VertexFormat> {
        use wgpu::VertexFormat as V;
        Some(match self {
            Self::Float1 => V::Float32,
            Self::Float2 => V::Float32x2,
            Self::Float3 => V::Float32x3,
            Self::Float4 => V::Float32x4,
            Self::D3dColor => V::Unorm8x4Bgra,
            Self::NormShort2 => V::Snorm16x2,
            Self::NormShort4 => V::Snorm16x4,
            Self::PByte2 => V::Unorm8x2,
            Self::PByte4 => V::Unorm8x4,
            Self::Short1
            | Self::Short2
            | Self::Short3
            | Self::Short4
            | Self::NormShort1
            | Self::NormShort3
            | Self::NormPacked3
            | Self::PByte1
            | Self::PByte3
            | Self::Float2H => return None,
        })
    }
}

/// One vertex attribute within a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexAttribute {
    /// Source register / shader location.
    pub register: VertexRegister,
    /// Data type in the guest buffer.
    pub data_type: VertexDataType,
    /// Byte offset from the start of the vertex.
    pub offset: u32,
}

/// All attributes read from one vertex stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct StreamLayout {
    /// Stream index.
    pub stream: u32,
    /// Attributes, in declaration order.
    pub attributes: Vec<VertexAttribute>,
    /// Vertex size implied by the declaration. The guest's actual stride is
    /// set separately (`SetStreamSource`) and takes precedence at draw time.
    pub stride: u32,
}

/// A full vertex format: the pipeline's vertex-input description.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VertexLayout {
    /// Streams in ascending index order.
    pub streams: Vec<StreamLayout>,
    /// Whether the position is pre-transformed screen space (`XYZRHW`):
    /// a shader-selection input, not a vertex-input one.
    pub pretransformed: bool,
}

/// Errors from decoding FVF codes or declarations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VertexError {
    /// An FVF flag combination that D3D8 does not allow.
    #[error("invalid FVF 0x{0:x}: {1}")]
    InvalidFvf(u32, &'static str),
    /// A declaration token could not be decoded.
    #[error("invalid vertex declaration token 0x{token:08x} at index {index}: {reason}")]
    InvalidToken {
        /// Token position.
        index: usize,
        /// Raw token.
        token: u32,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// Declaration ended without `D3DVSD_END`.
    #[error("vertex declaration is missing D3DVSD_END")]
    MissingEnd,
}

/// `D3DFVF_*` flags (Xbox values equal PC values for the supported subset).
pub mod fvf {
    /// Position `x, y, z`.
    pub const XYZ: u32 = 0x002;
    /// Pre-transformed position `x, y, z, rhw`.
    pub const XYZRHW: u32 = 0x004;
    /// Mask of the position field.
    pub const POSITION_MASK: u32 = 0x00E;
    /// Vertex normal.
    pub const NORMAL: u32 = 0x010;
    /// Point size.
    pub const PSIZE: u32 = 0x020;
    /// Diffuse `D3DCOLOR`.
    pub const DIFFUSE: u32 = 0x040;
    /// Specular `D3DCOLOR`.
    pub const SPECULAR: u32 = 0x080;
    /// Shift of the texture-coordinate-set count.
    pub const TEXCOUNT_SHIFT: u32 = 8;
    /// Mask of the texture-coordinate-set count.
    pub const TEXCOUNT_MASK: u32 = 0xF00;
}

impl VertexLayout {
    /// Builds the single-stream layout implied by a fixed-function vertex
    /// format code. Elements appear in the canonical D3D order: position,
    /// blend weights, normal, point size, diffuse, specular, texcoords.
    pub fn from_fvf(code: u32) -> Result<Self, VertexError> {
        let pos = code & fvf::POSITION_MASK;
        let mut attrs = Vec::new();
        let mut offset = 0u32;
        let mut push = |reg: VertexRegister, ty: VertexDataType, offset: &mut u32| {
            attrs.push(VertexAttribute { register: reg, data_type: ty, offset: *offset });
            *offset += ty.byte_size();
        };

        let mut pretransformed = false;
        match pos {
            0 => return Err(VertexError::InvalidFvf(code, "no position")),
            fvf::XYZ => push(VertexRegister::POSITION, VertexDataType::Float3, &mut offset),
            fvf::XYZRHW => {
                pretransformed = true;
                push(VertexRegister::POSITION, VertexDataType::Float4, &mut offset);
            }
            p @ 0x006..=0x00C if p % 2 == 0 => {
                // XYZB1..XYZB4: position plus 1..4 blend weights.
                push(VertexRegister::POSITION, VertexDataType::Float3, &mut offset);
                let weights = (p - 0x004) / 2;
                let ty = match weights {
                    1 => VertexDataType::Float1,
                    2 => VertexDataType::Float2,
                    3 => VertexDataType::Float3,
                    _ => VertexDataType::Float4,
                };
                push(VertexRegister::BLEND_WEIGHT, ty, &mut offset);
            }
            _ => return Err(VertexError::InvalidFvf(code, "unsupported position type")),
        }
        if code & fvf::NORMAL != 0 {
            if pretransformed {
                return Err(VertexError::InvalidFvf(code, "XYZRHW cannot have a normal"));
            }
            push(VertexRegister::NORMAL, VertexDataType::Float3, &mut offset);
        }
        if code & fvf::PSIZE != 0 {
            push(VertexRegister::POINT_SIZE, VertexDataType::Float1, &mut offset);
        }
        if code & fvf::DIFFUSE != 0 {
            push(VertexRegister::DIFFUSE, VertexDataType::D3dColor, &mut offset);
        }
        if code & fvf::SPECULAR != 0 {
            push(VertexRegister::SPECULAR, VertexDataType::D3dColor, &mut offset);
        }
        let tex_count = (code & fvf::TEXCOUNT_MASK) >> fvf::TEXCOUNT_SHIFT;
        if tex_count > 4 {
            return Err(VertexError::InvalidFvf(code, "the NV2A has only 4 texcoord registers"));
        }
        for i in 0..tex_count {
            let ty = match (code >> (16 + 2 * i)) & 3 {
                0 => VertexDataType::Float2,
                1 => VertexDataType::Float3,
                2 => VertexDataType::Float4,
                _ => VertexDataType::Float1,
            };
            push(VertexRegister::texcoord(i as u8), ty, &mut offset);
        }
        Ok(Self {
            streams: vec![StreamLayout { stream: 0, attributes: attrs, stride: offset }],
            pretransformed,
        })
    }

    /// Parses an Xbox vertex shader declaration (`D3DVSD_*` tokens).
    ///
    /// Supported tokens: `NOP`, `STREAM`, `REG` (stream data), `SKIP`, `END`.
    /// Tessellator, constant-memory and extension tokens are rejected.
    pub fn from_declaration(tokens: &[u32]) -> Result<Self, VertexError> {
        let mut streams: Vec<StreamLayout> = Vec::new();
        let mut cur: Option<usize> = None;
        let mut ended = false;
        for (index, &token) in tokens.iter().enumerate() {
            if token == 0xFFFF_FFFF {
                ended = true;
                break;
            }
            let err = |reason| VertexError::InvalidToken { index, token, reason };
            match token >> 29 {
                0 => {} // NOP
                1 => {
                    if token & 0x1000_0000 != 0 {
                        return Err(err("tessellator stream selection is unsupported"));
                    }
                    let n = token & 0xF;
                    let pos = match streams.iter().position(|s| s.stream == n) {
                        Some(p) => p,
                        None => {
                            streams.push(StreamLayout { stream: n, ..Default::default() });
                            streams.len() - 1
                        }
                    };
                    cur = Some(pos);
                }
                2 => {
                    let s = &mut streams[cur.ok_or_else(|| err("stream data before any D3DVSD_STREAM"))?];
                    if token & 0x0800_0000 != 0 {
                        // SKIPBYTES (0x18000000 | n << 16): count bytes.
                        s.stride += (token >> 16) & 0xF;
                    } else if token & 0x1000_0000 != 0 {
                        // SKIP: count DWORDs.
                        s.stride += ((token >> 16) & 0xF) * 4;
                    } else {
                        let reg = token & 0x1F;
                        if reg > 12 {
                            return Err(err("vertex register out of range"));
                        }
                        let raw = (token >> 16) & 0xFF;
                        if raw == 0x02 {
                            // D3DVSDT_NONE: the register is declared empty.
                            continue;
                        }
                        let ty = VertexDataType::from_xbox(raw).ok_or_else(|| err("unknown vertex data type"))?;
                        s.attributes.push(VertexAttribute {
                            register: VertexRegister(reg as u8),
                            data_type: ty,
                            offset: s.stride,
                        });
                        s.stride += ty.byte_size();
                    }
                }
                3 => return Err(err("tessellator tokens are unsupported")),
                4 => return Err(err("constant-memory tokens are not part of the input layout")),
                _ => return Err(err("unsupported token type")),
            }
        }
        if !ended {
            return Err(VertexError::MissingEnd);
        }
        streams.sort_by_key(|s| s.stream);
        let pretransformed = false;
        Ok(Self { streams, pretransformed })
    }
}

#[cfg(feature = "gpu")]
/// A `wgpu::VertexBufferLayout` that owns its attribute list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct OwnedVertexBufferLayout {
    /// Bytes between vertices.
    pub stride: u64,
    /// Attributes read from this buffer.
    pub attributes: Vec<wgpu::VertexAttribute>,
}

#[cfg(feature = "gpu")]
impl OwnedVertexBufferLayout {
    /// Borrows as a wgpu layout (per-vertex stepping).
    pub fn as_wgpu(&self) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: self.stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &self.attributes,
        }
    }
}

impl fmt::Display for PrimitiveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_values_roundtrip() {
        for v in 1..=10 {
            assert_eq!(PrimitiveType::from_xbox(v).unwrap().xbox_value(), v);
        }
        assert_eq!(PrimitiveType::from_xbox(0), None);
        assert_eq!(PrimitiveType::from_xbox(11), None);
    }

    #[test]
    fn primitive_counts() {
        assert_eq!(PrimitiveType::TriangleList.primitive_count(10), 3);
        assert_eq!(PrimitiveType::TriangleStrip.primitive_count(5), 3);
        assert_eq!(PrimitiveType::TriangleStrip.primitive_count(1), 0);
        assert_eq!(PrimitiveType::QuadList.primitive_count(9), 2);
        assert_eq!(PrimitiveType::QuadStrip.primitive_count(6), 2);
        assert_eq!(PrimitiveType::LineLoop.primitive_count(3), 3);
        assert_eq!(PrimitiveType::LineStrip.primitive_count(0), 0);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn native_and_rewritten_primitives() {
        use wgpu::PrimitiveTopology as T;
        assert_eq!(PrimitiveType::TriangleStrip.to_host(5), HostPrimitive::Native(T::TriangleStrip));
        assert_eq!(
            PrimitiveType::TriangleFan.to_host(5),
            HostPrimitive::Indexed(T::TriangleList, vec![0, 1, 2, 0, 2, 3, 0, 3, 4])
        );
        assert_eq!(
            PrimitiveType::QuadList.to_host(8),
            HostPrimitive::Indexed(T::TriangleList, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7])
        );
        assert_eq!(
            PrimitiveType::QuadStrip.to_host(6),
            HostPrimitive::Indexed(T::TriangleList, vec![0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4])
        );
        assert_eq!(
            PrimitiveType::LineLoop.to_host(3),
            HostPrimitive::Indexed(T::LineStrip, vec![0, 1, 2, 0])
        );
        assert_eq!(PrimitiveType::Polygon.host_topology(), T::TriangleList);
    }

    #[test]
    fn fvf_xyzrhw_diffuse() {
        let l = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        assert!(l.pretransformed);
        let s = &l.streams[0];
        assert_eq!(s.stride, 20);
        assert_eq!(s.attributes[0].data_type, VertexDataType::Float4);
        assert_eq!(s.attributes[1].register, VertexRegister::DIFFUSE);
        assert_eq!(s.attributes[1].offset, 16);
    }

    #[test]
    fn fvf_full_vertex() {
        // XYZ | NORMAL | DIFFUSE | TEX2, second set 3D (size bits = 1).
        let code = fvf::XYZ | fvf::NORMAL | fvf::DIFFUSE | 0x200 | (1 << 18);
        let l = VertexLayout::from_fvf(code).unwrap();
        let s = &l.streams[0];
        let offs: Vec<_> = s.attributes.iter().map(|a| (a.register.0, a.offset)).collect();
        assert_eq!(offs, vec![(0, 0), (2, 12), (3, 24), (9, 28), (10, 36)]);
        assert_eq!(s.stride, 48);
    }

    #[test]
    fn fvf_blend_weights() {
        let l = VertexLayout::from_fvf(0x00A | fvf::NORMAL).unwrap(); // XYZB3
        let s = &l.streams[0];
        assert_eq!(s.attributes[1].register, VertexRegister::BLEND_WEIGHT);
        assert_eq!(s.attributes[1].data_type, VertexDataType::Float3);
        assert_eq!(s.stride, 12 + 12 + 12);
    }

    #[test]
    fn fvf_errors() {
        assert!(VertexLayout::from_fvf(fvf::DIFFUSE).is_err());
        assert!(VertexLayout::from_fvf(fvf::XYZRHW | fvf::NORMAL).is_err());
        assert!(VertexLayout::from_fvf(fvf::XYZ | 0x500).is_err());
    }

    #[test]
    fn declaration_parse() {
        let stream0 = 1 << 29; // D3DVSD_STREAM(0)
        let reg = |r: u32, t: u32| (2 << 29) | (t << 16) | r;
        let tokens = [
            stream0,
            reg(0, 0x32),             // position: FLOAT3
            reg(3, 0x40),             // diffuse: D3DCOLOR
            (2 << 29) | (1 << 28) | (1 << 16), // SKIP 1 dword
            reg(9, 0x22),             // texcoord0: FLOAT2
            0xFFFF_FFFF,
        ];
        let l = VertexLayout::from_declaration(&tokens).unwrap();
        let s = &l.streams[0];
        let got: Vec<_> = s.attributes.iter().map(|a| (a.register.0, a.offset)).collect();
        assert_eq!(got, vec![(0, 0), (3, 12), (9, 20)]);
        assert_eq!(s.stride, 28);
    }

    #[test]
    fn declaration_skip_bytes_and_none() {
        // STREAM 0, SKIPBYTES 2, FLOAT3 at v0, NONE at v1, SKIP 1 dword, END.
        let toks = [0x2000_0000, 0x5802_0000, 0x4000_0000 | (0x32 << 16), 0x4000_0001 | (0x02 << 16), 0x5001_0000, 0xFFFF_FFFF];
        let l = VertexLayout::from_declaration(&toks).unwrap();
        let s = &l.streams[0];
        assert_eq!(s.attributes.len(), 1);
        assert_eq!(s.attributes[0].offset, 2);
        assert_eq!(s.stride, 2 + 12 + 4);
    }

    #[test]
    fn declaration_errors() {
        assert_eq!(VertexLayout::from_declaration(&[0]), Err(VertexError::MissingEnd));
        let bad_type = [(1 << 29), (2 << 29) | (0x99 << 16), 0xFFFF_FFFF];
        assert!(matches!(
            VertexLayout::from_declaration(&bad_type),
            Err(VertexError::InvalidToken { index: 1, .. })
        ));
        let no_stream = [(2 << 29) | (0x32 << 16), 0xFFFF_FFFF];
        assert!(VertexLayout::from_declaration(&no_stream).is_err());
        assert!(VertexLayout::from_declaration(&[3 << 29, 0xFFFF_FFFF]).is_err());
    }

    #[test]
    fn data_type_sizes_and_formats() {
        assert_eq!(VertexDataType::from_xbox(0x45), Some(VertexDataType::Short4));
        assert_eq!(VertexDataType::Short3.byte_size(), 6);
        assert_eq!(VertexDataType::PByte3.byte_size(), 3);
        #[cfg(feature = "gpu")]
        assert_eq!(VertexDataType::NormShort4.wgpu_format(), Some(wgpu::VertexFormat::Snorm16x4));
        #[cfg(feature = "gpu")]
        assert_eq!(VertexDataType::Short4.wgpu_format(), None);
        #[cfg(feature = "gpu")]
        assert_eq!(VertexDataType::D3dColor.wgpu_format(), Some(wgpu::VertexFormat::Unorm8x4Bgra));
        assert_eq!(VertexDataType::from_xbox(0x02), None);
    }
}
