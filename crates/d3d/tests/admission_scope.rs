//! Boundary drains must never consume a guest interrupt callback.
mod common;
use common::*;
use d3d::pushbuf::method;
use d3d::{GuestMemory, RecordingBackend, UploadPolicy};

#[test]
fn refused_boundary_keeps_callback_and_following_commands_in_order() {
    let (mut device, mut mem) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let dev = mem.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let mut put = mem.read_u32(dev).unwrap();
    let header = |method: u32, count: u32| (count << 18) | method;
    // Invented callback address/context, never invoked by this test.
    for word in [
        header(0x1d8c, 2),
        0x123400,
        0x456700,
        header(method::NOP, 1),
        7,
        header(0x1480, 1),
        0x5a5a,
    ] {
        mem.write_u32(put, word).unwrap();
        put += 4;
    }
    mem.write_u32(dev, put).unwrap();
    assert!(!device.drain_admission_boundary(&mut mem).unwrap());
    assert!(
        !device.drain_admission_boundary(&mut mem).unwrap(),
        "retry must still refuse"
    );
    assert_ne!(
        device.stipple_pattern()[0],
        0x5a5a,
        "no commands after callback may run"
    );
    let cb = device.drain_until_callback(&mut mem).unwrap().unwrap();
    assert_eq!((cb.function, cb.context, cb.kind), (0x123400, 0x456700, 7));
    assert!(device.drain_admission_boundary(&mut mem).unwrap());
    assert_eq!(device.stipple_pattern()[0], 0x5a5a);
    assert_eq!(device.stats().push_callbacks, 1);
}

#[test]
fn a_refused_native_array_leaves_the_one_ordinary_draw_for_the_caller() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Refuse(AtomicUsize);
    impl d3d::DrawAdmission for Refuse {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            assert_eq!(snapshot.draw.vertex_count, 4);
            self.0.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(backend, UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let owner = Refuse(AtomicUsize::new(0));
    assert!(!device.try_native_array_draw(
        &mut mem, gfx::vertex::PrimitiveType::QuadList, 0, 4, &owner,
    ).unwrap());
    assert_eq!(owner.0.load(Ordering::Relaxed), 1);
    assert!(device.backend.draws().is_empty());

    device.draw_vertices(&mut mem, QUADLIST, 0, 4).unwrap();
    assert_eq!(device.backend.draws().len(), 1);
}
