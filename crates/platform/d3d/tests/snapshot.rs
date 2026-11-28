//! Machine snapshots, phase 4 (docs/runtime/snapshots.md): the `d3d`
//! section over the recording backend. A device draws some frames and is
//! saved, then draws more; a second device, created the same way but
//! having drawn something else, loads the section over the saved guest
//! memory and draws the same frames. The recorded clears and presents, the
//! presented surfaces and guest memory match.

use crate::common::*;
use d3d::backend::Recorded;
use d3d::device::SnapshotError;
use d3d::{Device, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use snapshot::{BuildKey, Header, Snapshot, SnapshotWriter};

/// A transform only the device holds (`SetTransform` reads it once).
const MATRIX: u32 = MISC + 0x3000;
/// One `D3DRECT` for `SetScissors`.
const SCISSOR: u32 = MISC + 0x3100;

fn key() -> BuildKey {
    BuildKey {
        stamp: "d3d-test".into(),
        switches: String::new(),
    }
}

fn writer() -> SnapshotWriter {
    SnapshotWriter::new(Header {
        build: key(),
        name: "d3d".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    })
}

/// A clear, a quad from a vertex buffer, an inline quad, and `Swap`.
fn frame(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, n: u32) {
    set_states(d, m);
    d.clear(m, 0, 0, 0xF1, 0xFF00_0000 | n, 1.0, 0).unwrap();
    quad_vb(m, 0.0, 0.0, 32.0, 64.0, 0xFF00_0000 | (n << 8));
    d.note_write(0x8000_0000 | VB_DATA, 4 * 20);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    inline_quad(m, 32.0, 32.0, 64.0, 64.0, [n as u8, 255, 0, 255]);
    d.swap(m, 0).unwrap();
}

/// Device state that lives only on the host: a transform and a scissor.
fn host_state(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, scale: f32) {
    for i in 0..16u32 {
        let f = if i % 5 == 0 { scale } else { 0.0 };
        m.write_u32(MATRIX + 4 * i, f.to_bits()).unwrap();
    }
    d.set_transform(m, 2, MATRIX).unwrap();
    for (i, v) in [0u32, 0, 32 + scale as u32, 64].into_iter().enumerate() {
        m.write_u32(SCISSOR + 4 * i as u32, v).unwrap();
    }
    d.set_scissors(m, 1, 0, SCISSOR).unwrap();
}

/// Clears and presents from `calls`.
fn stream(calls: &[Recorded]) -> Vec<Recorded> {
    calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Clear { .. } | Recorded::ClearRects { .. } | Recorded::Present { .. } => {
                Some(c.clone())
            }
            _ => None,
        })
        .collect()
}

fn read(m: &SparseMemory, addr: u32, len: usize) -> Vec<u8> {
    let mut v = vec![0; len];
    m.read(addr, &mut v).unwrap();
    v
}

#[test]
fn a_loaded_device_draws_what_the_saved_one_does() {
    let (mut a, mut ma) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    host_state(&mut a, &mut ma, 2.0);
    // Three flips: the ring is not in its created order.
    for n in 1..=3 {
        frame(&mut a, &mut ma, n);
    }
    a.end_visibility_test(&mut ma, 5).unwrap();
    let mut w = writer();
    a.save_snapshot(&mut ma, &mut w).unwrap();
    let ram = ma.clone();
    let file = w.finish().unwrap();
    let mark = a.backend.calls.len();
    for n in 4..=6 {
        frame(&mut a, &mut ma, n);
    }
    let answer = a.get_visibility_test_result(&mut ma, 5).unwrap();

    // Created the same way, then two flips and other host state.
    let (mut b, mut other) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    host_state(&mut b, &mut other, 3.0);
    for n in 10..=11 {
        frame(&mut b, &mut other, n);
    }
    let mut mb = ram;
    b.load_snapshot(&Snapshot::open(&file, &key()).unwrap())
        .unwrap();
    assert_eq!(b.transform(2), a.transform(2));
    let mark_b = b.backend.calls.len();
    for n in 4..=6 {
        frame(&mut b, &mut mb, n);
    }
    assert_eq!(b.get_visibility_test_result(&mut mb, 5).unwrap(), answer);

    let (sa, sb) = (
        stream(&a.backend.calls[mark..]),
        stream(&b.backend.calls[mark_b..]),
    );
    let presents = sa
        .iter()
        .filter(|c| matches!(c, Recorded::Present { .. }))
        .count();
    assert_eq!(presents, 3);
    assert_eq!(sa, sb);
    assert_eq!(a.front_buffer(), b.front_buffer());
    assert_eq!(a.back_buffer(), b.back_buffer());
    let dev = d3d::GuestLayout::HALO2.device;
    assert_eq!(read(&ma, dev, 0x3000), read(&mb, dev, 0x3000));
    assert_eq!(
        read(&ma, PB.addr, PB.len as usize),
        read(&mb, PB.addr, PB.len as usize)
    );
}

#[test]
fn a_device_created_differently_refuses_the_section_unchanged() {
    let (mut a, mut ma) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    frame(&mut a, &mut ma, 1);
    let mut w = writer();
    a.save_snapshot(&mut ma, &mut w).unwrap();
    let file = w.finish().unwrap();

    let (mut b, mut mb) = create_copy_swap(RecordingBackend::new(), UploadPolicy::Tracked);
    host_state(&mut b, &mut mb, 3.0);
    let before = b.transform(2).copied();
    let err = b
        .load_snapshot(&Snapshot::open(&file, &key()).unwrap())
        .unwrap_err();
    assert!(matches!(err, SnapshotError::Mismatch(_)), "{err}");
    assert_eq!(b.transform(2).copied(), before);
}

#[test]
fn a_save_inside_begin_end_is_refused() {
    let (mut a, mut ma) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    frame(&mut a, &mut ma, 1);
    // Begin with no End, written inline the way game code does.
    let dev = d3d::GuestLayout::HALO2.device;
    let put = ma.read_u32(dev).unwrap();
    ma.write_u32(put, (1 << 18) | d3d::pushbuf::method::BEGIN_END)
        .unwrap();
    ma.write_u32(put + 4, QUADLIST).unwrap();
    ma.write_u32(dev, put + 8).unwrap();
    let err = a.save_snapshot(&mut ma, &mut writer()).unwrap_err();
    assert!(matches!(err, SnapshotError::Busy(_)), "{err}");
}

#[test]
fn film_keyframe_creates_a_matching_device_before_guest_execution() {
    use d3d::device::DeviceConfig;
    let (mut recorded, mut ram) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    frame(&mut recorded, &mut ram, 1);
    let mut writer = writer();
    recorded.save_film_snapshot(&mut ram, &mut writer).unwrap();
    let file = writer.finish().unwrap();
    let snap = Snapshot::open(&file, &key()).unwrap();
    let mut replay = Device::new(
        DeviceConfig { upload: UploadPolicy::Tracked, ..DeviceConfig::halo2(PB, SCRATCH) },
        RecordingBackend::new(),
    );
    assert!(!replay.snapshot_blockers().is_empty());
    assert!(replay.film_load_blockers().is_empty());
    assert!(replay.load_snapshot(&snap).is_err());
    replay.load_film_snapshot(&snap, &ram).unwrap();
    let mut replay_ram = ram.clone();
    let record_at = recorded.backend.calls.len();
    let replay_at = replay.backend.calls.len();
    frame(&mut recorded, &mut ram, 2);
    frame(&mut replay, &mut replay_ram, 2);
    assert_eq!(stream(&recorded.backend.calls[record_at..]), stream(&replay.backend.calls[replay_at..]));
    let device = d3d::GuestLayout::HALO2.device;
    assert_eq!(read(&ram, device, 0x3000), read(&replay_ram, device, 0x3000));
}

#[test]
fn normal_snapshots_keep_the_v1_payload_and_warm_restore_rules() {
    use d3d::device::{PresentParameters, SNAPSHOT_SECTION};

    let (mut device, mut ram) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    host_state(&mut device, &mut ram, 2.0);
    let mut normal = writer();
    let mut film = writer();
    device.save_snapshot(&mut ram, &mut normal).unwrap();
    device.save_film_snapshot(&mut ram, &mut film).unwrap();
    let normal = normal.finish().unwrap();
    let film = film.finish().unwrap();
    let normal = Snapshot::open(&normal, &key()).unwrap();
    let film = Snapshot::open(&film, &key()).unwrap();

    // The old reader requires version 1 and starts decoding the device
    // with the back-buffer header at byte zero.
    let v1 = normal.require(SNAPSHOT_SECTION, 1..=1).unwrap();
    let v2 = film.require(SNAPSHOT_SECTION, 2..=2).unwrap();
    assert_eq!(&v1[..4], &SCRATCH.addr.to_le_bytes());
    assert_eq!(&v2[PresentParameters::SIZE as usize..], v1.as_slice());

    // Refresh rate differs, but both devices have identical ring shape
    // and guest header layout. Only film playback requires an exact match.
    let (mut other, _) = create_with(RecordingBackend::new(), UploadPolicy::Tracked, |p| {
        p.refresh_rate += 1;
    });
    other.load_snapshot(&normal).unwrap();
    other.load_snapshot(&film).unwrap();
    assert!(matches!(other.load_film_snapshot(&film, &ram), Err(SnapshotError::Mismatch(_))));
}

#[test]
fn opaque_simple_matches_serial_pushed_state_at_a_guest_boundary() {
    let (mut a, mut ma) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let (mut b, mut mb) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let header = synthetic_encode()[RS_ZFUNC as usize];
    a.set_render_state_simple(&mut ma, header, 0x203).unwrap();
    b.set_render_state_simple(&mut mb, header, 0x203).unwrap();

    // Make the guest array already hold the next requested value while
    // the GPU still holds 0x203. The pushed register must still change.
    let slot = d3d::GuestLayout::HALO2.render_state_slot(RS_ZFUNC);
    ma.write_u32(slot, 0x207).unwrap();
    mb.write_u32(slot, 0x207).unwrap();
    a.set_render_state_simple(&mut ma, header, 0x207).unwrap();
    assert!(b.try_opaque_simple(&mb, header, 0x207));
    a.set_render_state_simple(&mut ma, header, 0x207).unwrap();
    assert!(b.try_opaque_simple(&mb, header, 0x207));
    assert_eq!(a.stats().simple_state_calls, b.stats().simple_state_calls);
    let (mut wa, mut wb) = (writer(), writer());
    a.save_snapshot(&mut ma, &mut wa).unwrap();
    b.save_snapshot(&mut mb, &mut wb).unwrap();
    assert_eq!(wa.finish().unwrap(), wb.finish().unwrap());

    // A queued command must be drained by the next ordinary D3D entry.
    let dev = mb.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let put = mb.read_u32(dev).unwrap();
    mb.write_u32(put, header).unwrap();
    mb.write_u32(put + 4, 0x201).unwrap();
    mb.write_u32(dev, put + 8).unwrap();
    assert!(!b.try_opaque_simple(&mb, header, 0x204));
    assert_eq!(a.stats().simple_state_calls, b.stats().simple_state_calls);
}

#[cfg(feature = "gpu")]
#[test]
fn warm_restore_releases_mark_samples_with_the_same_ring() {
    use std::cell::RefCell;
    use std::sync::Arc;

    use d3d::present_camera::Image;

    thread_local! {
        static SAMPLE: RefCell<Option<Image>> = const { RefCell::new(None) };
    }

    let backend = d3d::ThreadedBackend::new(RecordingBackend::new()).unwrap();
    let (mut device, mut ram) = create(backend, UploadPolicy::Tracked);
    let ring = [device.back_buffer().unwrap(), device.front_buffer().unwrap()];
    let mut saved = writer();
    device.save_snapshot(&mut ram, &mut saved).unwrap();
    let saved = saved.finish().unwrap();
    let snapshot = Snapshot::open(&saved, &key()).unwrap();

    let payload = Arc::new(19_u32);
    let previous = Arc::downgrade(&payload);
    let _ = SAMPLE.replace(Some(Image {
        views: [None; 4],
        payload,
    }));
    device.set_mark_image_source(|| SAMPLE.take());
    device.swap(&mut ram, 0).unwrap();
    device.backend.finish().unwrap();
    assert!(previous.upgrade().is_some());

    device.load_snapshot(&snapshot).unwrap();
    assert_eq!([device.back_buffer().unwrap(), device.front_buffer().unwrap()], ring);
    assert!(previous.upgrade().is_none());
}
