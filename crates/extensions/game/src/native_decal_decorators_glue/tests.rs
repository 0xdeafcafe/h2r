use engine::memory::FlatMemory;
use render_bridge::native_decal_decorators::{Kind, Stream};

use super::*;

/// A call of `count` vertices from vertex 0 whose block's stride is
/// `stride`.
fn call(count: u32, stride: u8) -> Call {
    Call {
        key: Some(Key {
            args: [0xe3c0_0011, 0, 0, 15, 0],
            implementation: (0, 0x0059_0000),
            block: 0x0057_0e00,
            kind: Kind::Triangles,
            stream: Stream {
                kind: 7,
                stride,
                offset: 0,
                buffer: 0x0057_0f00,
                data: 0x8012_3000,
            },
            c53: [0; 4],
        }),
        empty: false,
        class_type: 2,
        stamp: (0x0057_1278, 0x77),
        start: 0,
        count,
    }
}

#[test]
fn a_quad_list_is_indexed_on_the_xbox_diagonal_and_a_triangle_list_in_order() {
    let mut draw = crate::native_hud_glue::tests::ui_draw();
    draw.entry = "D3DDevice_DrawVertices";
    draw.primitive = PrimitiveType::QuadList;
    draw.vertex_count = 8;
    let quads = indexed(&draw).unwrap();
    assert_eq!(quads.primitive, PrimitiveType::TriangleList);
    assert_eq!(
        &quads.indices.unwrap()[..],
        &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]
    );
    draw.primitive = PrimitiveType::TriangleList;
    draw.vertex_count = 7;
    assert_eq!(
        &indexed(&draw).unwrap().indices.unwrap()[..],
        &[0, 1, 2, 3, 4, 5],
        "a partial triangle is dropped, as on the Xbox"
    );
    draw.vertex_count = 2;
    assert_eq!(indexed(&draw).err(), Some("empty"));
    draw.primitive = PrimitiveType::TriangleStrip;
    assert_eq!(indexed(&draw).err(), Some("topology"));
    draw.primitive = PrimitiveType::TriangleList;
    draw.indices = Some(vec![0, 1, 2].into());
    assert_eq!(indexed(&draw).err(), Some("indexed"));
}

#[test]
fn a_replay_is_the_template_with_the_placements_vertices() {
    let template = crate::native_hud_glue::tests::ui_draw();
    let vertices: Vec<u8> = (0..48).collect();
    let draw = replay(&template, &call(6, 8), vertices.clone()).unwrap();
    assert_eq!(draw.vertex_count, 6);
    assert_eq!(&draw.streams[0].data[..], &vertices[..]);
    assert_eq!(draw.streams[0].stride, 8);
    // Only the vertices differ from the template.
    assert_eq!(draw_differences(&template, &draw), ["draw_vertices"]);
    assert!(draw_differences(&draw, &draw.clone()).is_empty());
    // A body that drew with other constants or another primitive.
    let mut other = draw.clone();
    let mut constants = *other.shader.constants;
    constants[149] = [1.0; 4];
    other.shader.constants = Arc::new(constants);
    other.primitive = PrimitiveType::QuadList;
    assert_eq!(
        draw_differences(&draw, &other),
        ["draw_primitive", "draw_shader"]
    );
    // A stride or a byte count other than the template's.
    assert_eq!(
        replay(&template, &call(6, 12), vec![0; 72]).err(),
        Some("stride")
    );
    assert_eq!(
        replay(&template, &call(6, 8), vec![0; 40]).err(),
        Some("stride")
    );
}

#[test]
fn the_words_hold_until_any_d3d_or_shader_cache_word_moves() {
    let layout = d3d::GuestLayout::HALO2;
    let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
    let before = Words::read_at(&layout, 10, &m).unwrap();
    assert_eq!(before, Words::read_at(&layout, 10, &m).unwrap());
    // Another draw, or a push, is another state even with every array the same.
    assert_ne!(before, Words::read_at(&layout, 11, &m).unwrap());
    m.set_u32(layout.device + d3d::layout::device_field::PUT, 0x40)
        .unwrap();
    let pushed = Words::read_at(&layout, 10, &m).unwrap();
    assert_ne!(before, pushed);
    assert!(
        before.differences(&pushed).is_empty(),
        "the put is a body's own"
    );
    // A render state, a constant and the shader cache's implementation.
    m.set_u32(layout.render_state + 4 * 7, 1).unwrap();
    m.set_u32(layout.vs_constants + 16 * 149, 1).unwrap();
    m.set_u32(
        engine::render_loop::layer::SETUP_CONTEXT + 0x24,
        0x0059_0000,
    )
    .unwrap();
    let after = Words::read_at(&layout, 10, &m).unwrap();
    assert_eq!(
        before.differences(&after),
        [
            "words_render_state",
            "words_constants",
            "words_shader_cache"
        ]
    );
}
