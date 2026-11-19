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
    let header = |method, count| (count << 18) | method;
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
