//! The prepared path's callback ordering: a queued callback leaves the
//! whole prepared section unissued.

use crate::common::*;
use d3d::{GuestMemory, RecordingBackend, UploadPolicy};
use gfx::vertex::PrimitiveType::TriangleStrip;

fn indices(values: [u16; 4]) -> Vec<u8> {
    values.into_iter().flat_map(u16::to_le_bytes).collect()
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
