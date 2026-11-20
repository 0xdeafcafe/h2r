//! Persistent interleaved bindings built from an actual sorted indexed draw.
//! RepackPlan supplies the host format widths. Keep the guest format and
//! bytes, even for attributes whose normalized inputs can have two bit
//! representations of the same value.

use gfx::renderer::VertexStream;
use gfx::repack::{FetchMode, RepackPlan};
use gfx::vertex::{StreamLayout, VertexAttribute, VertexLayout};

pub struct PackedVertices {
    pub layout: VertexLayout,
    pub stride: u32,
    pub bytes: Vec<u8>,
}

/// Packs the draw's actual declarations into one GPU-ready raw stream.
/// Vertex and index numbering are unchanged. A malformed, ambiguous or
/// incomplete declaration leaves the caller's original binding in place.
pub fn pack(layout: &VertexLayout, streams: &[VertexStream<'_>], count: u32) -> Option<PackedVertices> {
    if layout.pretransformed || count == 0 || streams.len() != layout.streams.len() || streams.len() < 2 {
        return None;
    }
    let strides: Vec<_> = streams.iter().map(|s| s.stride).collect();
    let plan = RepackPlan::with_mode(layout, &strides, FetchMode::Gpu);
    let mut copies = Vec::new();
    let mut attributes = Vec::new();
    let mut mask = 0u16;
    let mut stride = 0u32;
    for (stream, (source, p)) in streams.iter().zip(&plan.streams).enumerate() {
        for a in &p.attributes {
            let register = 1u16.checked_shl(u32::from(a.register.0))?;
            let width = a.data_type.byte_size();
            if mask & register != 0 || a.guest_offset.checked_add(width)? > p.guest_stride {
                return None;
            }
            mask |= register;
            attributes.push(VertexAttribute { register: a.register, data_type: a.data_type, offset: stride });
            copies.push((stream, a.guest_offset as usize, stride as usize, width as usize));
            stride += (a.host_format.size() as u32).next_multiple_of(4);
        }
        p.check(source.data, count).ok()?;
    }
    if stride == 0 {
        return None;
    }
    let layout = VertexLayout {
        streams: vec![StreamLayout { stream: 0, attributes, stride }],
        pretransformed: false,
    };
    let packed = RepackPlan::with_mode(&layout, &[stride], FetchMode::Gpu);
    if !packed.streams[0].identity || packed.input_fetch() != plan.input_fetch() {
        return None;
    }
    let mut bytes = vec![0; (stride as usize).checked_mul(count as usize)?];
    for (v, dest) in bytes.chunks_exact_mut(stride as usize).enumerate() {
        for &(s, from, to, n) in &copies {
            let source = &streams[s];
            let start = from + v * plan.streams[s].guest_stride as usize;
            dest[to..to + n].copy_from_slice(&source.data[start..start + n]);
        }
    }
    Some(PackedVertices { layout, stride, bytes })
}

/// Runs the actual vertex program for the opt-in coverage probe. Raw
/// oPos x/y are in guest pixels. The probe measures triangle coverage
/// before clipping, depth testing and blending.
pub fn screen_vertices(
    words: &[u32],
    constants: &[[f32; 4]; shaders::vsh::CONSTANT_COUNT],
    defaults: [[f32; 4]; 16],
    layout: &VertexLayout,
    streams: &[VertexStream<'_>],
    count: u32,
) -> Option<Vec<Option<(f32, f32, f32)>>> {
    use shaders::vsh::{self, OutReg, Program, ProgramKind};

    if streams.len() != layout.streams.len() {
        return None;
    }
    let program = Program::from_words(ProgramKind::Normal, words).ok()?;
    let strides: Vec<_> = streams.iter().map(|s| s.stride).collect();
    let plan = RepackPlan::with_mode(layout, &strides, FetchMode::Gpu);
    for (p, s) in plan.streams.iter().zip(streams) {
        p.check(s.data, count).ok()?;
    }
    Some((0..count as usize).map(|v| {
        let mut inputs = defaults;
        for (p, s) in plan.streams.iter().zip(streams) {
            let row = v * p.guest_stride as usize;
            for a in &p.attributes {
                let offset = row + a.guest_offset as usize;
                let bytes = s.data.get(offset..offset + a.data_type.byte_size() as usize)?;
                *inputs.get_mut(usize::from(a.register.0))? = decode(a.data_type, bytes)?;
            }
        }
        let mut constants = *constants;
        let [x, y, _, w] = vsh::interp::run(&program, &inputs, &mut constants).get(OutReg::Pos);
        if w <= 0.0 || !x.is_finite() || !y.is_finite() || !w.is_finite() {
            return None;
        }
        Some(((16.0 * x).trunc() / 16.0, (16.0 * y).trunc() / 16.0, w))
    }).collect())
}

fn decode(kind: gfx::vertex::VertexDataType, bytes: &[u8]) -> Option<[f32; 4]> {
    use crate::object_streams::Attribute as A;
    use gfx::vertex::VertexDataType::*;
    let attribute = match kind {
        Float1 => A::Float(1), Float2 => A::Float(2), Float3 => A::Float(3), Float4 => A::Float(4),
        Short1 => A::Short(1), Short2 => A::Short(2), Short3 => A::Short(3), Short4 => A::Short(4),
        NormShort1 => A::NormShort(1), NormShort2 => A::NormShort(2), NormShort3 => A::NormShort(3), NormShort4 => A::NormShort(4),
        PByte1 => A::PByte(1), PByte2 => A::PByte(2), PByte3 => A::PByte(3), PByte4 => A::PByte(4),
        NormPacked3 => A::NormPacked3,
        D3dColor => A::Colour,
        Float2H => {
            let [x, y, w, _] = A::Float(3).decode(bytes);
            return Some([x, y, 0.0, w]);
        }
    };
    Some(attribute.decode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::repack::AttributeFetch;
    use gfx::vertex::{VertexDataType as Type, VertexRegister as V};

    fn declarations() -> VertexLayout {
        VertexLayout {
            streams: vec![
                StreamLayout {
                    stream: 0,
                    stride: 6,
                    attributes: vec![VertexAttribute { register: V(0), data_type: Type::NormShort3, offset: 0 }],
                },
                StreamLayout {
                    stream: 2,
                    stride: 4,
                    attributes: vec![VertexAttribute { register: V(5), data_type: Type::NormPacked3, offset: 0 }],
                },
            ],
            pretransformed: false,
        }
    }

    #[test]
    fn packing_keeps_vertex_order_and_the_raw_normalized_endpoints() {
        let positions = [0x00, 0x80, 2, 0, 3, 0, 0xfe, 0xff, 5, 0, 6, 0];
        let frames = [0x00, 0x04, 0, 0, 11, 12, 13, 14];
        let sources = [
            VertexStream { data: &positions, stride: 6 },
            VertexStream { data: &frames, stride: 4 },
        ];
        let p = pack(&declarations(), &sources, 2).unwrap();
        assert_eq!(p.stride, 12);
        assert_eq!(&p.bytes[..6], &positions[..6]);
        assert_eq!(&p.bytes[6..8], &[0; 2]);
        assert_eq!(&p.bytes[8..12], &frames[..4]);
        assert_eq!(&p.bytes[12..18], &positions[6..]);
        assert_eq!(&p.bytes[20..24], &frames[4..]);
        let planned = RepackPlan::with_mode(&p.layout, &[p.stride], FetchMode::Gpu);
        assert_eq!(planned.input_fetch()[0], AttributeFetch::Xyz1);
        assert_eq!(planned.input_fetch()[5], AttributeFetch::NormPacked3);
        assert!(planned.streams[0].identity);
    }

    #[test]
    fn ambiguous_or_short_declarations_keep_the_original_draw() {
        let bytes = [0; 12];
        let sources = [
            VertexStream { data: &bytes, stride: 6 },
            VertexStream { data: &bytes, stride: 4 },
        ];
        let mut layout = declarations();
        assert!(pack(&layout, &sources, 3).is_none());
        layout.streams[1].attributes[0].register = V(0);
        assert!(pack(&layout, &sources, 2).is_none());
        layout = declarations();
        layout.streams[0].attributes[0].offset = 2;
        assert!(pack(&layout, &sources, 1).is_none());
    }
}
