//! Machine snapshots, phase 4 (docs/runtime/snapshots.md): the `d3d`
//! section over the recording backend. A device draws some frames and is
//! saved, then draws more; a second device, created the same way but
//! having drawn something else, loads the section over the saved guest
//! memory and draws the same frames. The recorded draw streams, the
//! presented surfaces and guest memory match.

mod common;

use common::*;
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

/// Clears, draws and presents from `calls`, with texture handles left out
/// (a device that dropped its caches uploads again under new handles).
fn stream(calls: &[Recorded]) -> Vec<Recorded> {
    calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { color, draw, .. } => Some(Recorded::Draw {
                color: *color,
                draw: draw.clone(),
                textures: [None; 4],
            }),
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
    let draws = sa
        .iter()
        .filter(|c| matches!(c, Recorded::Draw { .. }))
        .count();
    assert_eq!(draws, 6);
    assert_eq!(sa, sb);
    // The scissor saved with the device clips the draws after the load.
    for c in &sb {
        if let Recorded::Draw { draw, .. } = c {
            assert_eq!(draw.clip.map(|r| r.width), Some(34), "{:?}", draw.clip);
        }
    }
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
