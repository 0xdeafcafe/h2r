//! The prepared path and the ordinary API share the same packet semantics.
//! A material setter or guest draw may reuse an index or bitmap header
//! after a section has already been prepared.

use crate::common::*;
use d3d::backend::{Recorded, ResolvedDraw};
use d3d::{GuestMemory, RecordingBackend, UploadPolicy};
use gfx::vertex::PrimitiveType::TriangleStrip;

fn indices(values: [u16; 4]) -> Vec<u8> {
    values.into_iter().flat_map(u16::to_le_bytes).collect()
}

fn interleave(native: bool, address: u32) -> Vec<(u32, [Option<u32>; 4], ResolvedDraw)> {
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_vertex_cache(Some(1024 * 1024));
    m.map(address & !0xfff, 4096);
    quad_vb(&mut m, 4.0, 8.0, 24.0, 32.0, 0xff55_6677);
    texture(&mut m, 18);
    bind_quad(&mut d, &mut m);
    set_states(&mut d, &mut m);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();

    for (ordinal, original) in [[0, 1, 2, 3], [1, 3, 2, 0], [3, 0, 2, 1]].into_iter().enumerate() {
        m.write(address, &indices(original)).unwrap();
        d.note_write(address, 8);
        let use_native = native && ordinal != 1;
        let prepared = use_native.then(|| {
            let ranges = vec![d.prepare_index_range(&m, address, 4).unwrap()];
            let vertices = d.prepare_world_vertices(&m, None, &ranges).unwrap();
            let targets = d.prepare_opaque_targets(&m, None).unwrap();
            let texture = d.prepare_stage_texture(&m, TEX_HEADER, 0, 0).unwrap();
            (ranges, vertices, targets, [Some(texture), None, None, None])
        });

        // Model an auxiliary material setup that streams a new image and
        // changes the index window *after* the last selection.
        if ordinal == 2 {
            m.write(address, &indices([2, 1, 0, 2])).unwrap();
            d.note_write(address, 8);
            let replacement = 0x8000_0000 | (TEX_DATA + 0x1000);
            m.map(replacement, 4096);
            m.write(replacement, &[45; 256]).unwrap();
            m.write_u32(TEX_HEADER + 4, TEX_DATA + 0x1000).unwrap();
        }
        d.set_render_state(&mut m, RS_ALPHABLENDENABLE, ordinal as u32 & 1).unwrap();
        if let Some((ranges, vertices, targets, textures)) = prepared {
            let mut counts = Vec::new();
            let emitted = d.emit_world_ranges(&mut m, TriangleStrip, ranges, vertices, None,
                &targets, &textures, 23, |count| counts.push(count)).unwrap();
            assert!(emitted);
            assert_eq!(counts, [4]);
        } else {
            d.draw_indexed_vertices(&mut m, TRIANGLESTRIP, 4, address).unwrap();
        }
    }
    d.backend.calls.into_iter().filter_map(|call| match call {
        Recorded::Draw { color, draw, textures } => Some((color, textures, *draw)),
        _ => None,
    }).collect()
}

#[test]
fn serial_and_prepared_packets_match_across_guest_draw_and_resource_reuse() {
    for address in [0x8050_0000, MISC + 0x8400] {
        let serial = interleave(false, address);
        let alternating = interleave(true, address);
        assert_eq!(serial.len(), 3);
        assert_eq!(alternating, serial);
        assert!(alternating[0].1[0].is_some());
        assert_ne!(alternating[0].1[0], alternating[2].1[0], "reused header selected its new host texture");
        assert_eq!(alternating[2].2.indices.as_deref(), Some([2, 1, 0, 2].as_slice()));
    }
}

#[test]
fn queued_callback_leaves_entire_prepared_section_unissued() {
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    let address = MISC + 0x8300;
    m.write(address, &indices([0, 1, 2, 3])).unwrap();
    let ranges = vec![d.prepare_index_range(&m, address, 4).unwrap(),
        d.prepare_index_range(&m, address, 4).unwrap()];
    let targets = d.prepare_opaque_targets(&m, None).unwrap();
    let vertices = d.prepare_world_vertices(&m, None, &ranges).unwrap();
    let dev = m.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let mut put = m.read_u32(dev).unwrap();
    for word in [(2 << 18) | 0x1d8c, 0xabc, 0x123, (1 << 18) | d3d::pushbuf::method::NOP, 7] {
        m.write_u32(put, word).unwrap();
        put += 4;
    }
    m.write_u32(dev, put).unwrap();
    assert!(!d.emit_world_ranges(&mut m, TriangleStrip, ranges, vertices, None, &targets,
        &[None; 4], 23, |_| panic!("callback must precede every draw")).unwrap());
    assert!(d.backend.draws().is_empty());
    let callback = d.drain_until_callback(&mut m).unwrap().unwrap();
    assert_eq!((callback.function, callback.context, callback.kind), (0xabc, 0x123, 7));
}
