//! Vertex repacking: turning guest vertex streams into buffers wgpu can read.
//!
//! wgpu's vertex formats cover only some Xbox `D3DVSDT_*` encodings, and
//! WebGPU insists on 4-byte-aligned strides and component-aligned offsets.
//! A [`RepackPlan`] decides, per stream, whether the guest buffer can be used
//! as is ("identity") or must be rewritten to a compact host layout, and
//! [`StreamPlan::repack`] performs the rewrite.
//!
//! Host-side convention: every attribute is consumed by shaders as float
//! (`vec2/3/4<f32>`). Missing components take the usual vertex-fetch defaults
//! `(0, 0, 0, 1)`, which matches the Xbox "expanded to (x, 0, 0, 1)" rules.
//!
//! | Guest type | Host format | Conversion |
//! |---|---|---|
//! | `FLOAT1..4`, `D3DCOLOR`, `NORMSHORT2/4`, `PBYTE2/4` | same width wgpu format | none |
//! | `SHORT1..4` | `Float32x1..4` | integer value as float |
//! | `NORMSHORT1/3` | `Float32x1/3` | `max(v / 32767, -1)` |
//! | `PBYTE1/3` | `Float32x1/3` | `v / 255` |
//! | `NORMPACKED3` | `Float32x3` | signed-normalised 11:11:10 (x in the low bits, verified) |
//! | `FLOAT2H` | `Float32x4` | `(x, y, 0, w)` |
//!
//! ## GPU fetch
//!
//! With [`FetchMode::Gpu`] (the default; `H2_VERTEX_FETCH=cpu` turns it
//! off) nothing is converted on the CPU: every type maps to a wgpu format
//! that reads the guest bytes as they are, and the vertex shader finishes
//! the job ([`AttributeFetch`], mirrored by `shaders::vsh::InputFetch`):
//!
//! | Guest type | Host format | Shader |
//! |---|---|---|
//! | `FLOAT1..4`, `D3DCOLOR`, `NORMSHORT2/4`, `PBYTE2/4` | as above | as is |
//! | `NORMSHORT1`, `PBYTE1` | `Snorm16`, `Unorm8` | as is |
//! | `NORMSHORT3`, `PBYTE3` | `Snorm16x4`, `Unorm8x4` | `(x, y, z, 1)` |
//! | `SHORT1/2/4` | `Sint16`, `Sint16x2`, `Sint16x4` | to float |
//! | `SHORT3` | `Sint16x4` | to float, `(x, y, z, 1)` |
//! | `NORMPACKED3` | `Uint32` | 11:11:10 decode |
//! | `FLOAT2H` | `Float32x3` | `(x, y, 0, w)` |
//!
//! The four-wide formats for three-component types read one component
//! past the attribute, so a stream is bound raw only when that stays
//! inside the vertex and the stride and offsets meet WebGPU's alignment
//! rules (stride a multiple of 4 up to 2048, offsets aligned to the
//! component size). Any other stream (a 6-byte `NORMSHORT3` stream, say)
//! is *realigned*: each attribute's guest bytes are copied, unconverted,
//! to a 4-aligned slot with room for the widened read, and the shader
//! decodes it the same way. The plan is a pure function of layout, strides
//! and mode, so the shader translator and the pipeline key agree.
//!

use std::borrow::Cow;

use crate::vertex::{OwnedVertexBufferLayout, VertexDataType, VertexLayout, VertexRegister};

/// Errors from repacking.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RepackError {
    /// The guest buffer is too small for the vertex count.
    #[error("vertex buffer too small: need {need} bytes, have {have}")]
    TooShort {
        /// Bytes required.
        need: usize,
        /// Bytes supplied.
        have: usize,
    },
}

/// Where vertex formats are converted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FetchMode {
    /// Repack every stream that needs it to floats on the CPU (the old
    /// path; `H2_VERTEX_FETCH=cpu`).
    Cpu,
    /// Bind guest bytes as they are and decode in the vertex shader.
    #[default]
    Gpu,
}

impl FetchMode {
    /// `H2_VERTEX_FETCH`: `cpu` selects [`FetchMode::Cpu`], anything else
    /// (or unset) [`FetchMode::Gpu`].
    pub fn from_env() -> Self {
        Self::parse(std::env::var("H2_VERTEX_FETCH").ok().as_deref())
    }

    fn parse(v: Option<&str>) -> Self {
        match v {
            Some(v) if v.eq_ignore_ascii_case("cpu") => Self::Cpu,
            _ => Self::Gpu,
        }
    }
}

/// How the vertex shader turns a host attribute into its `vec4f` register
/// (`shaders::vsh::InputFetch` has the same variants).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AttributeFetch {
    /// A float or normalised format, used as is.
    #[default]
    Float,
    /// Four float components holding three: `(x, y, z, 1)`.
    Xyz1,
    /// Signed integers, converted to float.
    Int,
    /// Four signed integers holding three: `(x, y, z, 1)`.
    IntXyz1,
    /// `NORMPACKED3` as a `u32`, decoded.
    NormPacked3,
    /// `FLOAT2H` as three floats: `(x, y, 0, w)`.
    Float2H,
}

impl AttributeFetch {
    /// Host format and shader fetch for `t` read raw.
    fn raw(t: VertexDataType) -> (wgpu::VertexFormat, AttributeFetch) {
        use VertexDataType::*;
        use wgpu::VertexFormat as F;
        match t {
            Short1 => (F::Sint16, Self::Int),
            Short2 => (F::Sint16x2, Self::Int),
            Short3 => (F::Sint16x4, Self::IntXyz1),
            Short4 => (F::Sint16x4, Self::Int),
            NormShort1 => (F::Snorm16, Self::Float),
            NormShort3 => (F::Snorm16x4, Self::Xyz1),
            PByte1 => (F::Unorm8, Self::Float),
            PByte3 => (F::Unorm8x4, Self::Xyz1),
            NormPacked3 => (F::Uint32, Self::NormPacked3),
            Float2H => (F::Float32x3, Self::Float2H),
            other => (other.wgpu_format().expect("direct format"), Self::Float),
        }
    }
}

/// How one attribute is rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conversion {
    /// Copy the bytes unchanged.
    Copy,
    /// Signed 16-bit integers to float, `n` components.
    ShortToFloat(u8),
    /// Signed-normalised 16-bit to float, `n` components.
    NormShortToFloat(u8),
    /// Unsigned-normalised bytes to float, `n` components.
    PByteToFloat(u8),
    /// 11:11:10 packed signed-normalised to `Float32x3`.
    NormPacked3,
    /// `(x, y, w)` to `(x, y, 0, w)`.
    Float2H,
}

impl Conversion {
    fn for_type(t: VertexDataType) -> (Conversion, wgpu::VertexFormat, u32) {
        use VertexDataType::*;
        use wgpu::VertexFormat as F;
        let float_n = |n: u8| match n {
            1 => F::Float32,
            2 => F::Float32x2,
            3 => F::Float32x3,
            _ => F::Float32x4,
        };
        match t {
            Short1 => (Conversion::ShortToFloat(1), float_n(1), 4),
            Short2 => (Conversion::ShortToFloat(2), float_n(2), 8),
            Short3 => (Conversion::ShortToFloat(3), float_n(3), 12),
            Short4 => (Conversion::ShortToFloat(4), float_n(4), 16),
            NormShort1 => (Conversion::NormShortToFloat(1), float_n(1), 4),
            NormShort3 => (Conversion::NormShortToFloat(3), float_n(3), 12),
            PByte1 => (Conversion::PByteToFloat(1), float_n(1), 4),
            PByte3 => (Conversion::PByteToFloat(3), float_n(3), 12),
            NormPacked3 => (Conversion::NormPacked3, float_n(3), 12),
            Float2H => (Conversion::Float2H, float_n(4), 16),
            other => {
                let f = other.wgpu_format().expect("direct format");
                (Conversion::Copy, f, other.byte_size())
            }
        }
    }
}

/// One attribute within a [`StreamPlan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlannedAttribute {
    /// Source register (= shader location).
    pub register: VertexRegister,
    /// Guest data type.
    pub data_type: VertexDataType,
    /// Offset in the guest vertex.
    pub guest_offset: u32,
    /// Offset in the host vertex.
    pub host_offset: u32,
    /// Format the shader sees.
    pub host_format: wgpu::VertexFormat,
    /// Rewrite rule.
    pub conversion: Conversion,
    /// How the shader reads it.
    pub fetch: AttributeFetch,
}

/// Plan for one vertex stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamPlan {
    /// Stream index.
    pub stream: u32,
    /// Guest bytes between vertices.
    pub guest_stride: u32,
    /// Host bytes between vertices (equals `guest_stride` when identity).
    pub host_stride: u32,
    /// Attributes read from this stream.
    pub attributes: Vec<PlannedAttribute>,
    /// The guest buffer can be bound unchanged.
    pub identity: bool,
}

/// wgpu's default `max_vertex_buffer_array_stride`.
const MAX_VERTEX_STRIDE: u32 = 2048;

fn format_alignment(f: wgpu::VertexFormat) -> u32 {
    f.size().min(4) as u32
}

impl StreamPlan {
    fn new(
        stream: u32,
        attrs: &[crate::vertex::VertexAttribute],
        guest_stride: u32,
        mode: FetchMode,
    ) -> Self {
        let stride_ok =
            guest_stride.is_multiple_of(4) && (4..=MAX_VERTEX_STRIDE).contains(&guest_stride);
        // wgpu needs 4-aligned strides of at most 2048 bytes, offsets
        // aligned to the component size, and every attribute must fit
        // inside the stride.
        let fits = |p: &PlannedAttribute| {
            p.guest_offset
                .is_multiple_of(format_alignment(p.host_format))
                && p.guest_offset as u64 + p.host_format.size() <= guest_stride as u64
        };
        if mode == FetchMode::Gpu {
            let mut raw: Vec<PlannedAttribute> = attrs
                .iter()
                .map(|a| {
                    let (host_format, fetch) = AttributeFetch::raw(a.data_type);
                    PlannedAttribute {
                        register: a.register,
                        data_type: a.data_type,
                        guest_offset: a.offset,
                        host_offset: a.offset,
                        host_format,
                        conversion: Conversion::Copy,
                        fetch,
                    }
                })
                .collect();
            if stride_ok && raw.iter().all(fits) {
                return Self {
                    stream,
                    guest_stride,
                    host_stride: guest_stride,
                    attributes: raw,
                    identity: true,
                };
            }
            // Realigned: the same raw formats, each attribute's guest bytes
            // copied to a 4-aligned slot with room for its widened read
            // (the rest of the slot stays zero).
            let mut off = 0u32;
            for p in &mut raw {
                p.host_offset = off;
                off = (off + p.host_format.size() as u32).next_multiple_of(4);
            }
            return Self {
                stream,
                guest_stride,
                host_stride: off.max(4),
                attributes: raw,
                identity: false,
            };
        }
        let mut planned: Vec<PlannedAttribute> = attrs
            .iter()
            .map(|a| {
                let (conversion, host_format, _) = Conversion::for_type(a.data_type);
                PlannedAttribute {
                    register: a.register,
                    data_type: a.data_type,
                    guest_offset: a.offset,
                    host_offset: a.offset,
                    host_format,
                    conversion,
                    fetch: AttributeFetch::Float,
                }
            })
            .collect();
        let identity = stride_ok
            && planned
                .iter()
                .all(|p| p.conversion == Conversion::Copy && fits(p));
        let mut host_stride = guest_stride;
        if !identity {
            let mut off = 0u32;
            for p in &mut planned {
                p.host_offset = off;
                off += p.host_format.size() as u32;
                off = off.next_multiple_of(4);
            }
            host_stride = off.max(4);
        }
        Self {
            stream,
            guest_stride,
            host_stride,
            attributes: planned,
            identity,
        }
    }

    /// Bytes the host buffer for `vertex_count` vertices binds: whole
    /// vertices, so a raw attribute read wider than its guest type (see
    /// "GPU fetch") stays inside it. Data shorter than this is padded.
    pub fn host_bytes(&self, vertex_count: u32) -> usize {
        self.host_stride as usize * vertex_count as usize
    }

    /// Bytes of guest data needed for `vertex_count` vertices (the last
    /// vertex only needs the bytes its attributes touch).
    pub fn required_guest_bytes(&self, vertex_count: u32) -> usize {
        if vertex_count == 0 {
            return 0;
        }
        let end = self
            .attributes
            .iter()
            .map(|a| a.guest_offset + a.data_type.byte_size())
            .max()
            .unwrap_or(0);
        self.guest_stride as usize * (vertex_count as usize - 1) + end as usize
    }

    /// The wgpu buffer layout for the host buffer.
    pub fn buffer_layout(&self) -> OwnedVertexBufferLayout {
        OwnedVertexBufferLayout {
            stride: self.host_stride as u64,
            attributes: self
                .attributes
                .iter()
                .map(|a| wgpu::VertexAttribute {
                    format: a.host_format,
                    offset: a.host_offset as u64,
                    shader_location: a.register.0 as u32,
                })
                .collect(),
        }
    }

    /// Produces the host vertex buffer for `vertex_count` vertices of
    /// `guest`. Borrows the input when the stream is identity.
    pub fn repack<'a>(
        &self,
        guest: &'a [u8],
        vertex_count: u32,
    ) -> Result<Cow<'a, [u8]>, RepackError> {
        self.check(guest, vertex_count)?;
        if self.identity {
            let len = (self.guest_stride as usize * vertex_count as usize).min(guest.len());
            return Ok(Cow::Borrowed(&guest[..len]));
        }
        let mut out = vec![0u8; self.host_bytes(vertex_count)];
        self.convert(guest, vertex_count, &mut out);
        Ok(Cow::Owned(out))
    }

    /// Errors unless `guest` holds `vertex_count` vertices.
    pub fn check(&self, guest: &[u8], vertex_count: u32) -> Result<(), RepackError> {
        let need = self.required_guest_bytes(vertex_count);
        if guest.len() < need {
            return Err(RepackError::TooShort {
                need,
                have: guest.len(),
            });
        }
        Ok(())
    }

    /// Writes the host buffer for `vertex_count` vertices of `guest` (already
    /// [`check`](Self::check)ed) into `out`, which holds
    /// [`host_bytes`](Self::host_bytes) bytes: the guest bytes (zero-padded)
    /// for an identity stream, the realigned or converted vertices
    /// otherwise (built in `scratch` first).
    pub fn write_into(
        &self,
        guest: &[u8],
        vertex_count: u32,
        mut out: wgpu::WriteOnly<'_, [u8]>,
        scratch: &mut Vec<u8>,
    ) {
        let len = self.host_bytes(vertex_count);
        debug_assert_eq!(out.len(), len);
        if self.identity {
            let n = len.min(guest.len());
            let (mut head, mut tail) = out.split_at(n);
            head.copy_from_slice(&guest[..n]);
            tail.fill(0);
        } else {
            scratch.clear();
            scratch.resize(len, 0);
            self.convert(guest, vertex_count, scratch);
            out.copy_from_slice(scratch);
        }
    }

    fn convert(&self, guest: &[u8], vertex_count: u32, out: &mut [u8]) {
        let (gs, hs) = (self.guest_stride as usize, self.host_stride as usize);
        // One attribute copied as is (a realigned stream, or a lone direct
        // attribute): a tight loop over fixed-size copies.
        if let [a] = self.attributes.as_slice()
            && a.conversion == Conversion::Copy
        {
            let n = a.data_type.byte_size() as usize;
            let (go, ho) = (a.guest_offset as usize, a.host_offset as usize);
            let count = vertex_count as usize;
            match n {
                1 => copy_lone::<1>(guest, out, gs, hs, go, ho, count),
                2 => copy_lone::<2>(guest, out, gs, hs, go, ho, count),
                3 => copy_lone::<3>(guest, out, gs, hs, go, ho, count),
                4 => copy_lone::<4>(guest, out, gs, hs, go, ho, count),
                6 => copy_lone::<6>(guest, out, gs, hs, go, ho, count),
                8 => copy_lone::<8>(guest, out, gs, hs, go, ho, count),
                12 => copy_lone::<12>(guest, out, gs, hs, go, ho, count),
                16 => copy_lone::<16>(guest, out, gs, hs, go, ho, count),
                _ => self.convert_each(guest, vertex_count, out),
            }
            return;
        }
        self.convert_each(guest, vertex_count, out);
    }

    fn convert_each(&self, guest: &[u8], vertex_count: u32, out: &mut [u8]) {
        for v in 0..vertex_count as usize {
            let src = &guest[v * self.guest_stride as usize..];
            let dst = &mut out[v * self.host_stride as usize..][..self.host_stride as usize];
            for a in &self.attributes {
                convert_attribute(
                    a,
                    &src[a.guest_offset as usize..],
                    &mut dst[a.host_offset as usize..],
                );
            }
        }
    }
}

/// Copies `N` bytes at guest offset `go` of each of `count` vertices to host
/// offset `ho`.
fn copy_lone<const N: usize>(
    guest: &[u8],
    out: &mut [u8],
    gs: usize,
    hs: usize,
    go: usize,
    ho: usize,
    count: usize,
) {
    for v in 0..count {
        let src: &[u8; N] = guest[v * gs + go..][..N].try_into().unwrap();
        out[v * hs + ho..][..N].copy_from_slice(src);
    }
}

/// `dst[..n] = src[..n]` for an attribute's size: fixed-size copies the
/// compiler inlines, not a `memcpy` call per attribute.
#[inline]
fn copy_small(src: &[u8], dst: &mut [u8], n: usize) {
    fn copy<const N: usize>(src: &[u8], dst: &mut [u8]) {
        let s: &[u8; N] = src[..N].try_into().unwrap();
        dst[..N].copy_from_slice(s);
    }
    match n {
        1 => copy::<1>(src, dst),
        2 => copy::<2>(src, dst),
        3 => copy::<3>(src, dst),
        4 => copy::<4>(src, dst),
        6 => copy::<6>(src, dst),
        8 => copy::<8>(src, dst),
        12 => copy::<12>(src, dst),
        16 => copy::<16>(src, dst),
        _ => dst[..n].copy_from_slice(&src[..n]),
    }
}

fn put_f32(dst: &mut [u8], i: usize, v: f32) {
    dst[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
}

fn convert_attribute(a: &PlannedAttribute, src: &[u8], dst: &mut [u8]) {
    let i16_at = |i: usize| i16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
    match a.conversion {
        Conversion::Copy => copy_small(src, dst, a.data_type.byte_size() as usize),
        Conversion::ShortToFloat(n) => {
            for i in 0..n as usize {
                put_f32(dst, i, i16_at(i) as f32);
            }
        }
        Conversion::NormShortToFloat(n) => {
            for i in 0..n as usize {
                put_f32(dst, i, (i16_at(i) as f32 / 32767.0).max(-1.0));
            }
        }
        Conversion::PByteToFloat(n) => {
            #[allow(clippy::needless_range_loop)]
            // index also addresses dst; keep the panic on short src
            for i in 0..n as usize {
                put_f32(dst, i, src[i] as f32 / 255.0);
            }
        }
        Conversion::NormPacked3 => {
            let raw = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
            let signed = |v: u32, bits: u32| -> f32 {
                let shift = 32 - bits;
                let s = ((v << shift) as i32) >> shift;
                (s as f32 / ((1i32 << (bits - 1)) - 1) as f32).max(-1.0)
            };
            put_f32(dst, 0, signed(raw & 0x7FF, 11));
            put_f32(dst, 1, signed((raw >> 11) & 0x7FF, 11));
            put_f32(dst, 2, signed(raw >> 22, 10));
        }
        Conversion::Float2H => {
            let f = |i: usize| {
                f32::from_le_bytes([src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]])
            };
            put_f32(dst, 0, f(0));
            put_f32(dst, 1, f(1));
            put_f32(dst, 2, 0.0);
            put_f32(dst, 3, f(2));
        }
    }
}

/// Repack plan for a whole vertex layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepackPlan {
    /// One plan per stream, in the layout's order.
    pub streams: Vec<StreamPlan>,
}

impl RepackPlan {
    /// Plans `layout`. `strides[i]` overrides the declared stride of the
    /// layout's i-th stream (the guest's `SetStreamSource` stride); missing
    /// entries and zeroes use the declared stride.
    pub fn new(layout: &VertexLayout, strides: &[u32]) -> Self {
        Self::with_mode(layout, strides, FetchMode::Cpu)
    }

    /// [`new`](Self::new) for `mode`.
    pub fn with_mode(layout: &VertexLayout, strides: &[u32], mode: FetchMode) -> Self {
        let streams = layout
            .streams
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let stride = strides
                    .get(i)
                    .copied()
                    .filter(|&v| v != 0)
                    .unwrap_or(s.stride);
                StreamPlan::new(s.stream, &s.attributes, stride, mode)
            })
            .collect();
        Self { streams }
    }

    /// wgpu layouts for pipeline creation, one per stream.
    pub fn buffer_layouts(&self) -> Vec<OwnedVertexBufferLayout> {
        self.streams.iter().map(StreamPlan::buffer_layout).collect()
    }

    /// The shader fetch of each vertex register (bit `n` of the result's
    /// index = `v<n>`); registers no stream supplies read
    /// [`AttributeFetch::Float`].
    pub fn input_fetch(&self) -> [AttributeFetch; 16] {
        let mut out = [AttributeFetch::Float; 16];
        for a in self.streams.iter().flat_map(|s| &s.attributes) {
            if let Some(slot) = out.get_mut(a.register.0 as usize) {
                *slot = a.fetch;
            }
        }
        out
    }

    /// Whether any stream needs rewriting.
    pub fn needs_repack(&self) -> bool {
        self.streams.iter().any(|s| !s.identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vertex::{StreamLayout, VertexAttribute, fvf};

    fn layout(attrs: &[(u8, VertexDataType, u32)], stride: u32) -> VertexLayout {
        VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes: attrs
                    .iter()
                    .map(|&(r, t, o)| VertexAttribute {
                        register: VertexRegister(r),
                        data_type: t,
                        offset: o,
                    })
                    .collect(),
                stride,
            }],
            pretransformed: false,
        }
    }

    fn floats(b: &[u8]) -> Vec<f32> {
        b.chunks(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn oversized_or_overrunning_strides_are_repacked() {
        let l = layout(&[(0, VertexDataType::Float4, 0)], 16);
        // Stride override smaller than the attribute: must not be identity.
        let plan = RepackPlan::new(&l, &[8]);
        assert!(plan.needs_repack());
        assert!(plan.buffer_layouts()[0].stride >= 16);
        // Stride beyond wgpu's 2048 limit.
        let plan = RepackPlan::new(&l, &[4096]);
        assert!(plan.needs_repack());
        assert!(plan.buffer_layouts()[0].stride <= 2048);
    }

    #[test]
    fn fvf_layouts_are_identity_and_borrowed() {
        let l = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE | 0x100).unwrap();
        let plan = RepackPlan::new(&l, &[]);
        assert!(!plan.needs_repack());
        let data = vec![7u8; 28 * 2];
        let out = plan.streams[0].repack(&data, 2).unwrap();
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.len(), 56);
        assert_eq!(plan.buffer_layouts()[0].stride, 28);
    }

    #[test]
    fn short_types_become_floats() {
        let l = layout(
            &[
                (0, VertexDataType::Short3, 0),
                (9, VertexDataType::Short2, 6),
            ],
            10,
        );
        let plan = RepackPlan::new(&l, &[]);
        let s = &plan.streams[0];
        assert!(!s.identity);
        assert_eq!(
            (
                s.host_stride,
                s.attributes[0].host_offset,
                s.attributes[1].host_offset
            ),
            (20, 0, 12)
        );
        let mut g = Vec::new();
        for v in [-3i16, 4, 5, 100, -200] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        let out = s.repack(&g, 1).unwrap();
        assert_eq!(floats(&out), vec![-3.0, 4.0, 5.0, 100.0, -200.0]);
        let bl = &plan.buffer_layouts()[0];
        assert_eq!(bl.attributes[0].format, wgpu::VertexFormat::Float32x3);
        assert_eq!(bl.attributes[1].format, wgpu::VertexFormat::Float32x2);
        assert_eq!(bl.attributes[1].shader_location, 9);
    }

    #[test]
    fn normshort_and_pbyte() {
        let l = layout(
            &[
                (0, VertexDataType::NormShort3, 0),
                (3, VertexDataType::PByte3, 6),
            ],
            9,
        );
        let s = &RepackPlan::new(&l, &[]).streams[0];
        let mut g = Vec::new();
        for v in [32767i16, -32768, 0] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        g.extend_from_slice(&[255, 0, 51]);
        let f = floats(&s.repack(&g, 1).unwrap());
        assert_eq!(f[..3], [1.0, -1.0, 0.0]);
        assert_eq!(f[3..], [1.0, 0.0, 0.2]);
    }

    #[test]
    fn normpacked3_extremes() {
        let l = layout(&[(2, VertexDataType::NormPacked3, 0)], 4);
        let s = &RepackPlan::new(&l, &[]).streams[0];
        // x = +1.0 (0x3FF), y = -1.0 (0x401 -> -1023), z = 0.
        let raw: u32 = 0x3FF | (0x401 << 11);
        let f = floats(&s.repack(&raw.to_le_bytes(), 1).unwrap());
        assert_eq!(f, vec![1.0, -1.0, 0.0]);
        // z = 0x1FF (+1.0 of 10 bits).
        let raw: u32 = 0x1FF << 22;
        assert_eq!(floats(&s.repack(&raw.to_le_bytes(), 1).unwrap())[2], 1.0);
    }

    #[test]
    fn float2h_inserts_zero() {
        let l = layout(&[(9, VertexDataType::Float2H, 0)], 12);
        let s = &RepackPlan::new(&l, &[]).streams[0];
        let mut g = Vec::new();
        for v in [1.5f32, 2.5, 4.0] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(floats(&s.repack(&g, 1).unwrap()), vec![1.5, 2.5, 0.0, 4.0]);
        assert_eq!(
            s.buffer_layout().attributes[0].format,
            wgpu::VertexFormat::Float32x4
        );
    }

    #[test]
    fn mixed_stream_keeps_direct_attributes_and_realigns() {
        // Position FLOAT3, then a 3-byte PBYTE3 colour, stride 15 (not 4-aligned).
        let l = layout(
            &[
                (0, VertexDataType::Float3, 0),
                (3, VertexDataType::PByte3, 12),
            ],
            15,
        );
        let s = &RepackPlan::new(&l, &[]).streams[0];
        assert!(!s.identity);
        assert_eq!(s.host_stride, 24);
        let mut g = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            g.extend_from_slice(&v.to_le_bytes());
        }
        g.extend_from_slice(&[255, 255, 0]);
        // two vertices
        let mut two = g.clone();
        two.extend_from_slice(&g);
        let f = floats(&s.repack(&two, 2).unwrap());
        assert_eq!(f.len(), 12);
        assert_eq!(&f[..6], &[1.0, 2.0, 3.0, 1.0, 1.0, 0.0]);
        assert_eq!(&f[6..9], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn stride_override_and_short_buffer() {
        let l = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        let plan = RepackPlan::new(&l, &[32]);
        assert!(plan.streams[0].identity);
        assert_eq!(plan.streams[0].host_stride, 32);
        // Only the last vertex's attribute bytes are required.
        assert_eq!(plan.streams[0].required_guest_bytes(3), 64 + 20);
        assert_eq!(
            plan.streams[0].repack(&[0; 80], 3),
            Err(RepackError::TooShort { need: 84, have: 80 })
        );
        assert!(plan.streams[0].repack(&[0; 84], 3).is_ok());
        assert_eq!(plan.streams[0].repack(&[], 0).unwrap().len(), 0);
    }

    #[test]
    fn multi_stream_plan() {
        let tokens = [
            1 << 29,
            (2 << 29) | (0x42 << 16), // stream 0: position FLOAT4
            (1 << 29) | 1,
            (2 << 29) | (0x35 << 16) | 9, // stream 1: texcoord0 SHORT3
            0xFFFF_FFFF,
        ];
        let l = VertexLayout::from_declaration(&tokens).unwrap();
        let plan = RepackPlan::new(&l, &[0, 6]);
        assert_eq!(plan.streams.len(), 2);
        assert!(plan.streams[0].identity);
        assert!(!plan.streams[1].identity);
        assert_eq!(plan.streams[1].guest_stride, 6);
        assert_eq!(plan.streams[1].host_stride, 12);
        assert!(plan.needs_repack());
        assert_eq!(plan.buffer_layouts().len(), 2);
    }

    #[test]
    fn fetch_mode_from_env_value() {
        assert_eq!(FetchMode::parse(None), FetchMode::Gpu);
        assert_eq!(FetchMode::parse(Some("CPU")), FetchMode::Cpu);
        assert_eq!(FetchMode::parse(Some("gpu")), FetchMode::Gpu);
    }

    #[test]
    fn gpu_mode_binds_aligned_streams_raw() {
        use VertexDataType::*;
        // Position, NORMPACKED3 normal, SHORT2 uv, NORMSHORT3 + pad.
        let l = layout(
            &[
                (0, Float3, 0),
                (2, NormPacked3, 12),
                (9, Short2, 16),
                (3, NormShort3, 20),
            ],
            28,
        );
        let plan = RepackPlan::with_mode(&l, &[], FetchMode::Gpu);
        let s = &plan.streams[0];
        assert!(s.identity && !plan.needs_repack());
        let bl = s.buffer_layout();
        let formats: Vec<_> = bl.attributes.iter().map(|a| a.format).collect();
        use wgpu::VertexFormat as F;
        assert_eq!(
            formats,
            [F::Float32x3, F::Uint32, F::Sint16x2, F::Snorm16x4]
        );
        let fetch = plan.input_fetch();
        assert_eq!(
            [fetch[0], fetch[2], fetch[9], fetch[3], fetch[1]],
            [
                AttributeFetch::Float,
                AttributeFetch::NormPacked3,
                AttributeFetch::Int,
                AttributeFetch::Xyz1,
                AttributeFetch::Float,
            ]
        );
        // The same layout in CPU mode converts.
        assert!(RepackPlan::with_mode(&l, &[], FetchMode::Cpu).needs_repack());
        // Bytes pass through untouched (whole vertices, padded).
        let data: Vec<u8> = (0..56).map(|i| i as u8).collect();
        let mut out = vec![0xAA; s.host_bytes(2)];
        s.write_into(
            &data[..50],
            2,
            wgpu::WriteOnly::from_mut(out.as_mut_slice()),
            &mut Vec::new(),
        );
        assert_eq!(&out[..50], &data[..50]);
        assert!(out[50..].iter().all(|&b| b == 0));
    }

    #[test]
    fn gpu_mode_realigns_unaligned_streams_without_converting() {
        use VertexDataType::*;
        // A 6-byte NORMSHORT3 stream: stride not a multiple of 4.
        let l = layout(&[(0, NormShort3, 0)], 6);
        let plan = RepackPlan::with_mode(&l, &[], FetchMode::Gpu);
        let s = &plan.streams[0];
        assert!(!s.identity);
        assert_eq!(s.host_stride, 8);
        assert_eq!(s.attributes[0].host_format, wgpu::VertexFormat::Snorm16x4);
        assert_eq!(s.attributes[0].fetch, AttributeFetch::Xyz1);
        let guest = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let out = s.repack(&guest, 2).unwrap();
        assert_eq!(
            &out[..],
            &[1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0]
        );
        // Several attributes at odd offsets: each to its own aligned slot.
        let l = layout(&[(0, PByte3, 0), (3, Short2, 3), (9, PByte1, 7)], 8);
        let s = &RepackPlan::with_mode(&l, &[], FetchMode::Gpu).streams[0];
        let offs: Vec<u32> = s.attributes.iter().map(|a| a.host_offset).collect();
        assert_eq!((offs, s.host_stride), (vec![0, 4, 8], 12));
        let out = s.repack(&[10, 11, 12, 20, 21, 22, 23, 30], 1).unwrap();
        assert_eq!(&out[..], &[10, 11, 12, 0, 20, 21, 22, 23, 30, 0, 0, 0]);
    }
}
