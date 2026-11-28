//! The weather plates' host (docs/graphics/retire-d3d-r2.md, "R2's decals
//! and weather"): the plate draw `0x00037a60`'s resolved quad strip, taken
//! while the call runs (`render_bridge::native_weather`). The guest's
//! setup still runs; the strip's vertices, its scroll constants
//! (`c[21..24]`, hardware order) and the definition words the placement
//! writes are Rust's, and the strip is recorded natively as an indexed
//! pass. Every plate is Rust's; a strip Rust cannot draw is a named
//! refusal (`weather_plate_refused_<reason>`).

use std::sync::Arc;

use d3d::backend::{IndexData, StreamBytes, StreamData};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ResolvedDraw};
use gfx::vertex::{PrimitiveType, StreamLayout, VertexDataType};
use native_render::weather_quad::{Quad, SCROLL_CONSTANT};
use native_render::world_layers_gpu::IndexedPass;
use render_bridge::native_weather::{self as host, Draw};

/// The strip's corners as two triangles (`PrimitiveType::to_host`'s
/// order for one quad of a strip: `b, b+1, b+3, b+2`).
const STRIP_TRIANGLES: [u32; 6] = [0, 1, 3, 0, 3, 2];

/// Registers the admission, on with the native renderer.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| render_bridge::native_decals::on(&s)) {
        return;
    }
    let Ok(mut d) = device.lock() else { return };
    d.add_draw_admission(Arc::new(Glue::default()));
}

#[derive(Default)]
struct Glue {
    cache: crate::native_map::MapIndexedCache,
}

fn components(t: VertexDataType) -> Option<usize> {
    Some(match t {
        VertexDataType::Float1 => 1,
        VertexDataType::Float2 => 2,
        VertexDataType::Float3 => 3,
        VertexDataType::Float4 => 4,
        _ => return None,
    })
}

/// Each register's offset in the inline vertex: the position (register 0,
/// four floats, `0x003fb2d0`) and registers 1 to 4 (two floats at least,
/// `0x003fb280`). Other registers are left as the guest's.
fn slots(layout: &StreamLayout) -> Result<[usize; 5], &'static str> {
    let mut at = [None; 5];
    for a in &layout.attributes {
        let r = usize::from(a.register.0);
        if r >= at.len() {
            continue;
        }
        let need = if r == 0 { 4 } else { 2 };
        if components(a.data_type).is_none_or(|n| n < need) {
            return Err("vertex_declaration");
        }
        at[r] = Some(a.offset as usize);
    }
    let mut out = [0; 5];
    for (o, a) in out.iter_mut().zip(at) {
        *o = a.ok_or("vertex_declaration")?;
    }
    Ok(out)
}

/// Vertex `v`'s Rust values per register: how many floats and the floats.
fn expected(q: &Quad, v: usize) -> [(usize, [f32; 4]); 5] {
    let x = &q.vertices[v];
    let [px, py] = x.position;
    let two = |[a, b]: [f32; 2]| (2, [a, b, 0.0, 0.0]);
    [
        (4, [px, py, q.z, q.w]),
        two(x.position),
        two(x.layers[0]),
        two(x.layers[1]),
        two(x.layers[2]),
    ]
}

/// Each float of the strip's 4 vertices: `(vertex, register, byte offset,
/// Rust's value)`.
fn floats(
    layout: &StreamLayout,
    q: &Quad,
) -> Result<Vec<(usize, usize, usize, f32)>, &'static str> {
    let at = slots(layout)?;
    let stride = layout.stride as usize;
    let mut out = Vec::new();
    for v in 0..4 {
        for (r, (n, values)) in expected(q, v).into_iter().enumerate() {
            for (c, value) in values.into_iter().take(n).enumerate() {
                out.push((v, r, v * stride + at[r] + 4 * c, value));
            }
        }
    }
    Ok(out)
}

/// The guest's strip bytes with Rust's floats over them.
fn rust_bytes(layout: &StreamLayout, bytes: &[u8], q: &Quad) -> Result<Vec<u8>, &'static str> {
    let mut out = bytes.to_vec();
    for (_, _, o, value) in floats(layout, q)? {
        out.get_mut(o..o + 4)
            .ok_or("vertex_bytes")?
            .copy_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}

/// The plate's resolved strip: one stream of 4 inline vertices.
fn strip(d: &ResolvedDraw) -> Result<(&StreamLayout, &StreamBytes), &'static str> {
    if d.primitive != PrimitiveType::QuadStrip || d.vertex_count != 4 || d.indices.is_some() {
        return Err("geometry");
    }
    if d.layout.streams.len() != 1 || d.streams.len() != 1 {
        return Err("vertex_streams");
    }
    let (layout, stream) = (&d.layout.streams[0], &d.streams[0]);
    if stream.stride != layout.stride || stream.data.len() != 4 * layout.stride as usize {
        return Err("vertex_bytes");
    }
    Ok((layout, stream))
}

/// A sampled stage must be bound and must not be the draw's own target.
fn stages(s: &DrawSnapshot<'_>) -> Result<(), &'static str> {
    for (n, reads) in s.sampled_stages.into_iter().enumerate() {
        let bound = s.bound_textures[n];
        if !reads {
            continue;
        }
        if bound == 0 {
            return Err("texture_unbound");
        }
        if bound == s.color_header || bound == s.depth_header {
            return Err("texture_target");
        }
        if s.draw.textures[n] != Some(bound) {
            return Err("texture_binding");
        }
    }
    Ok(())
}

/// The guest's resolved strip with Rust's vertices, scroll constants and
/// definition words, as an indexed triangle list.
fn rust_draw(d: &ResolvedDraw, rust: &Draw) -> Result<ResolvedDraw, &'static str> {
    let (layout, stream) = strip(d)?;
    let bytes = rust_bytes(layout, &stream.data, &rust.quad)?;
    let mut out = d.clone();
    out.streams[0] = StreamBytes {
        data: StreamData::Owned(bytes),
        stride: stream.stride,
    };
    let mut constants = *d.shader.constants;
    for (i, row) in rust.quad.constants.iter().enumerate() {
        constants[SCROLL_CONSTANT + i] = *row;
    }
    out.shader.constants = Arc::new(constants);
    let mut combiner = *d.shader.combiner_state;
    for (n, word) in combiner.iter_mut().enumerate() {
        if rust.written[n] {
            *word = rust.define[n];
        }
    }
    out.shader.combiner_state = Arc::new(combiner);
    if let Some(pixel) = &d.shader.pixel_shader {
        let mut pixel = **pixel;
        for (n, word) in pixel.iter_mut().enumerate() {
            if rust.written[n] {
                *word = rust.define[n];
            }
        }
        out.shader.pixel_shader = Some(Arc::new(pixel));
    }
    out.primitive = PrimitiveType::TriangleList;
    out.indices = Some(IndexData::Owned(STRIP_TRIANGLES.to_vec()));
    Ok(out)
}

impl DrawAdmission for Glue {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let rust = match host::take_draw()? {
            Ok(Some(rust)) => rust,
            Ok(None) => {
                // Rust's placement refused a plate the guest draws.
                host::refuse("placement");
                return None;
            }
            Err(why) => {
                host::refuse(why);
                return None;
            }
        };
        let d = snapshot.draw;
        let made = stages(snapshot)
            .and_then(|()| rust_draw(d, &rust))
            .and_then(|draw| {
                IndexedPass::new(
                    &draw,
                    self.cache.get(),
                    crate::d3d_diag::presents() + 1,
                    None,
                )
                .ok_or("pass")
            });
        let pass = match made {
            Ok(pass) => pass,
            Err(why) => {
                host::refuse(why);
                return None;
            }
        };
        host::admitted();
        Some((
            ExternalPlan {
                stage: timeline::native::Stage::Effects,
                view: 0,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(pass),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::effect::WeatherPlate;
    use gfx::vertex::{VertexAttribute, VertexRegister};
    use native_render::weather_place::Globals;
    use render_bridge::native_weather::{Inputs, plan};

    /// The inline layout the plate's calls make: position then registers
    /// 1 to 4, two floats each.
    fn layout() -> StreamLayout {
        let mut attributes = vec![VertexAttribute {
            register: VertexRegister::POSITION,
            data_type: VertexDataType::Float4,
            offset: 0,
        }];
        for r in 1..5u8 {
            attributes.push(VertexAttribute {
                register: VertexRegister(r),
                data_type: VertexDataType::Float2,
                offset: 16 + 8 * (u32::from(r) - 1),
            });
        }
        StreamLayout {
            stream: 0,
            attributes,
            stride: 48,
        }
    }

    fn draw() -> Draw {
        plan(&Inputs {
            plate: WeatherPlate {
                element: 0,
                player: 0,
                window: [0, 0, 640, 480],
                bitmaps: [1, 2, 3],
                layer_scale: [1.0; 3],
                scroll: [[0.25, 0.5], [1.0, 2.0], [3.0, 4.0]],
                extent: [1.0, 4.0],
                size: [2.0, 3.0, 4.0],
                tint: [1.0, 0.5, 0.25],
                fill: [0.1, 0.2, 0.3],
            },
            globals: Globals {
                scale: 1.0,
                limit: 0.25,
                target: 2,
            },
            near: 0.5,
            far: 100.0,
        })
        .unwrap()
    }

    #[test]
    fn rusts_strip_writes_its_floats_and_leaves_the_rest() {
        let rust = draw();
        let mut l = layout();
        // A carried register past 4 keeps the guest's bytes.
        l.attributes.push(VertexAttribute {
            register: VertexRegister(5),
            data_type: VertexDataType::Float2,
            offset: 48,
        });
        l.stride = 56;
        let bytes = rust_bytes(&l, &[0xab; 4 * 56], &rust.quad).unwrap();
        let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let v = &rust.quad.vertices[1];
        assert_eq!(
            [f(56), f(60), f(64), f(68)],
            [v.position[0], v.position[1], rust.quad.z, rust.quad.w]
        );
        assert_eq!([f(56 + 16), f(56 + 20)], v.position);
        assert_eq!([f(56 + 40), f(56 + 44)], v.layers[2]);
        assert_eq!(&bytes[56 + 48..56 + 56], &[0xab; 8]);
    }

    #[test]
    fn a_strip_without_the_plates_registers_is_refused_by_name() {
        let mut l = layout();
        l.attributes.retain(|a| a.register.0 != 3);
        assert_eq!(slots(&l), Err("vertex_declaration"));
        let mut l = layout();
        l.attributes[0].data_type = VertexDataType::Float3;
        assert_eq!(slots(&l), Err("vertex_declaration"));
        let mut l = layout();
        l.attributes[2].data_type = VertexDataType::D3dColor;
        assert_eq!(slots(&l), Err("vertex_declaration"));
        assert_eq!(slots(&layout()), Ok([0, 16, 24, 32, 40]));
    }
}
