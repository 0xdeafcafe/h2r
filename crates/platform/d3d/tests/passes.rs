//! Render-pass coverage: scissor areas that are not one rectangle (several
//! inclusive rectangles, exclusive ones) and textures over the device's own
//! frame buffers (Halo's `0x14600` textures over the back, display and
//! depth surfaces). Pure region checks, the recording backend, and pixels
//! read back from synthetic frames on a real GPU (skipped without an
//! adapter).

use crate::common::*;
use d3d::abi::Regs;
use d3d::device::{
    BACK_BUFFER_DATA, COPY_SWAP_FLAG, DISPLAY_BUFFER_DATA, Rect,
    THIRD_BUFFER_DATA, scissor_region,
};
use d3d::layout::device_field as f;
use d3d::{
    Device, Event, GfxBackend, GuestLayout, GuestMemory, PixelRect, Recorded, RecordingBackend,
    SparseMemory, UploadPolicy,
};
use gfx::GfxContext;
use gfx::surface::HostPixelOrder;

const L: GuestLayout = GuestLayout::HALO2;
const RECTS: u32 = MISC + 0x7100;
const FRAME_TEX: u32 = MISC + 0x7200;

fn r(x1: i32, y1: i32, x2: i32, y2: i32) -> Rect {
    Rect { x1, y1, x2, y2 }
}

fn tuples(p: &[PixelRect]) -> Vec<(u32, u32, u32, u32)> {
    p.iter().map(|p| (p.x, p.y, p.width, p.height)).collect()
}

/// Pixels in the pieces, checking they do not overlap.
fn covered(p: &[PixelRect], w: u32, h: u32) -> Vec<bool> {
    let mut hit = vec![false; (w * h) as usize];
    for p in p {
        for y in p.y..p.y + p.height {
            for x in p.x..p.x + p.width {
                let i = (y * w + x) as usize;
                assert!(!hit[i], "pixel {x},{y} in two pieces of {p:?}");
                hit[i] = true;
            }
        }
    }
    hit
}

#[test]
fn scissor_region_unions_inclusive_rectangles_without_overlap() {
    let base = (0, 0, 64, 64);
    // No rectangles: the whole base.
    assert_eq!(tuples(&scissor_region(base, &[], false)), [(0, 0, 64, 64)]);
    assert_eq!(tuples(&scissor_region(base, &[], true)), [(0, 0, 64, 64)]);
    // One: the intersection.
    assert_eq!(
        tuples(&scissor_region(base, &[r(-5, 8, 16, 100)], false)),
        [(0, 8, 16, 56)]
    );
    // Two overlapping: three disjoint pieces covering exactly the union.
    let rects = [r(0, 0, 40, 40), r(20, 20, 60, 60)];
    let p = scissor_region(base, &rects, false);
    assert_eq!(
        tuples(&p),
        [(0, 0, 40, 20), (0, 20, 60, 20), (20, 40, 40, 20)]
    );
    let hit = covered(&p, 64, 64);
    for y in 0..64 {
        for x in 0..64 {
            let want = rects
                .iter()
                .any(|r| r.x1 <= x && x < r.x2 && r.y1 <= y && y < r.y2);
            assert_eq!(hit[(y * 64 + x) as usize], want, "{x},{y}");
        }
    }
    // Stacked rectangles of the same width merge into one piece.
    assert_eq!(
        tuples(&scissor_region(
            base,
            &[r(8, 0, 24, 10), r(8, 10, 24, 30)],
            false
        )),
        [(8, 0, 16, 30)]
    );
    // Rectangles outside the base keep nothing; empty ones are ignored.
    assert!(scissor_region(base, &[r(64, 0, 80, 10)], false).is_empty());
    assert!(scissor_region(base, &[r(10, 10, 10, 20)], false).is_empty());
    assert!(scissor_region((5, 5, 5, 9), &[], false).is_empty());
}

#[test]
fn scissor_region_exclusive_keeps_what_the_rectangles_leave() {
    let base = (0, 0, 64, 64);
    let p = scissor_region(base, &[r(16, 16, 48, 48)], true);
    assert_eq!(
        tuples(&p),
        [
            (0, 0, 64, 16),
            (0, 16, 16, 32),
            (48, 16, 16, 32),
            (0, 48, 64, 16)
        ]
    );
    let hit = covered(&p, 64, 64);
    assert!(!hit[(32 * 64 + 32) as usize]);
    assert_eq!(hit.iter().filter(|h| **h).count(), 64 * 64 - 32 * 32);
    // A rectangle over the whole base leaves nothing.
    assert!(scissor_region(base, &[r(-1, -1, 100, 100)], true).is_empty());
    // Exclusive within a narrower base (the viewport).
    assert_eq!(
        tuples(&scissor_region((0, 0, 32, 32), &[r(0, 0, 32, 16)], true)),
        [(0, 16, 32, 16)]
    );
}

fn rects(m: &mut SparseMemory, list: &[[u32; 4]]) {
    for (i, w) in list.iter().flatten().enumerate() {
        m.write_u32(RECTS + 4 * i as u32, *w).unwrap();
    }
}

fn quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    set_states(d, m);
    quad_vb(m, 0.0, 0.0, W as f32, H as f32, color);
    bind_quad(d, m);
}

/// A texture header over `data` as Halo's `0x14600` builds them:
/// `Common` 0x40001, linear 2D, one level, `format` at `w x h`.
fn frame_texture(m: &mut SparseMemory, data: u32, format: u32, w: u32, h: u32) {
    frame_texture_at(m, FRAME_TEX, data, format, w, h);
}

fn frame_texture_at(m: &mut SparseMemory, at: u32, data: u32, format: u32, w: u32, h: u32) {
    let pitch = w * 4;
    m.write_u32(at, 0x0004_0001).unwrap();
    m.write_u32(at + 4, data).unwrap();
    m.write_u32(at + 8, 0).unwrap();
    m.write_u32(at + 12, 1 | (2 << 4) | (format << 8) | (1 << 16))
        .unwrap();
    m.write_u32(
        at + 16,
        (w - 1) | ((h - 1) << 12) | ((pitch / 64 - 1) << 24),
    )
    .unwrap();
}

/// Halo's pair of frame textures (`0x509354`, `0x509358`): built once by
/// `0x14600` over the back and display surfaces' `Data` as they are then.
const PARITY_TEX: [u32; 2] = [FRAME_TEX + 0x40, FRAME_TEX + 0x60];

fn parity_textures(m: &mut SparseMemory) {
    for (at, field) in PARITY_TEX
        .into_iter()
        .zip([f::BACK_BUFFER, f::DISPLAY_SURFACE])
    {
        let data = data_of(m, field);
        frame_texture_at(m, at, data, 0x12, W, H);
    }
}

/// The texture switch `0x14f80` for frame `frame`: the current parity
/// (`0x14fe9`) or the previous one (`0x14fc1`, `0x150a7`).
fn parity(frame: u32, previous: bool) -> u32 {
    PARITY_TEX[(frame.wrapping_sub(previous as u32) & 1) as usize]
}

fn data_of(m: &SparseMemory, header_field: u32) -> u32 {
    let header = m.read_u32(L.device + header_field).unwrap();
    m.read_u32(header + 4).unwrap()
}

/// The surfaces copied for frame-buffer samples, in order: a copy that
/// made a texture or refilled one.
fn snapshots(d: &Device<RecordingBackend>) -> Vec<u32> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Snapshot { surface, .. } | Recorded::SnapshotInto { surface, .. } => {
                Some(*surface)
            }
            _ => None,
        })
        .collect()
}

fn header_words(m: &SparseMemory, field: u32) -> (u32, u32) {
    let header = m.read_u32(L.device + field).unwrap();
    (
        m.read_u32(header + 4).unwrap(),
        m.read_u32(header + 8).unwrap(),
    )
}

#[test]
fn back_and_display_surfaces_are_distinct_and_swap_rotates_their_data() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    let (back, display) = (
        m.read_u32(L.device + f::BACK_BUFFER).unwrap(),
        m.read_u32(L.device + f::DISPLAY_SURFACE).unwrap(),
    );
    assert_ne!(back, display, "two headers");
    assert_eq!(m.read_u32(L.device + f::FRAME_BUFFER_COUNT).unwrap(), 2);
    assert_eq!(m.read_u32(L.device + f::FLAGS).unwrap() & COPY_SWAP_FLAG, 0);
    assert_ne!(d.back_buffer(), d.front_buffer());
    // Lock words travel with Data (0x3f9a70).
    m.write_u32(back + 8, 0x1111).unwrap();
    m.write_u32(display + 8, 0x2222).unwrap();
    let (b0, f0) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
    // Swap(0) is COPY | FINISH, and on this device it flips (0x3f9d75).
    d.swap(&mut m, 0).unwrap();
    assert_eq!(
        header_words(&m, f::BACK_BUFFER),
        (DISPLAY_BUFFER_DATA, 0x2222)
    );
    // Then 0x3f9cc0 stamps the display header's Lock with the GPU time and
    // records a fence for this swap's parity.
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE), (BACK_BUFFER_DATA, 2));
    assert_eq!(m.read_u32(L.device + f::SWAP_FENCES + 4).unwrap(), 2);
    assert_eq!(m.read_u32(L.device + f::GPU_TIME).unwrap(), 4);
    assert_eq!((d.back_buffer(), d.front_buffer()), (Some(f0), Some(b0)));
    // The headers themselves stay where they are.
    assert_eq!(m.read_u32(L.device + f::BACK_BUFFER).unwrap(), back);
    assert_eq!(m.read_u32(L.device + f::DISPLAY_SURFACE).unwrap(), display);
    assert_eq!(
        d.backend.calls.iter().rev().find_map(|c| match c {
            Recorded::Present { surface, .. } => Some(*surface),
            _ => None,
        }),
        Some(b0)
    );
    d.swap(&mut m, 0).unwrap();
    assert_eq!(header_words(&m, f::BACK_BUFFER), (BACK_BUFFER_DATA, 2));
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE), (DISPLAY_BUFFER_DATA, 6));
    assert_eq!(m.read_u32(L.device + f::SWAP_FENCES).unwrap(), 6);
    assert_eq!(m.read_u32(L.device + f::SWAP_FENCES + 4).unwrap(), 2);
    assert_eq!(m.read_u32(L.device + f::GPU_TIME).unwrap(), 8);
    assert_eq!((d.back_buffer(), d.front_buffer()), (Some(b0), Some(f0)));
    // Drawing into the back buffer never copies into the front: it holds
    // the image presented until the next flip.
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert!(
        !d.backend
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::CopySurface { .. }))
    );
}

#[test]
fn parity_textures_read_the_frame_drawn_and_the_frame_presented() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    parity_textures(&mut m);
    for frame in 0..4u32 {
        for previous in [false, true] {
            d.set_texture(&mut m, 0, parity(frame, previous)).unwrap();
            d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
            let want = if previous {
                d.front_buffer()
            } else {
                d.back_buffer()
            };
            assert_eq!(
                snapshots(&d).last().copied(),
                want,
                "frame {frame}, previous {previous}"
            );
        }
        d.swap(&mut m, 0).unwrap();
    }
    assert!(d.take_events().is_empty());
    assert_eq!(d.stats().frame_snapshots, 8);
    assert_eq!(d.stats().previous_frame_snapshots, 4);
    assert_eq!(d.backend.uploads(), 0, "guest memory is not read");
}

#[test]
fn copy_swap_device_keeps_its_back_buffer_and_shows_a_copy() {
    let (mut d, mut m) = create_copy_swap(RecordingBackend::new(), UploadPolicy::Always);
    assert_ne!(m.read_u32(L.device + f::FLAGS).unwrap() & COPY_SWAP_FLAG, 0);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    let (back, front) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
    d.swap(&mut m, 0).unwrap();
    assert_eq!(header_words(&m, f::BACK_BUFFER).0, BACK_BUFFER_DATA);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, DISPLAY_BUFFER_DATA);
    assert_eq!(d.back_buffer(), Some(back));
    // The display surface's image is still in back0 until a write.
    frame_texture(&mut m, DISPLAY_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    // The draw read back0 before writing it; the write then copies it to
    // the front, which later reads use.
    assert_eq!(snapshots(&d), [back]);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(snapshots(&d), [back, front]);
    assert!(d.take_events().is_empty());
}

#[test]
fn a_new_device_starts_its_frame_buffers_afresh() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    let old = [d.back_buffer().unwrap(), d.front_buffer().unwrap()];
    d.swap(&mut m, 0).unwrap();
    assert_eq!(header_words(&m, f::BACK_BUFFER).0, DISPLAY_BUFFER_DATA);
    d.release_device(&mut m, L.device).unwrap();
    for s in old {
        assert!(
            d.backend
                .calls
                .iter()
                .any(|c| matches!(c, Recorded::DestroySurface { id } if *id == s))
        );
    }
    recreate(&mut d, &mut m);
    // The headers name their first buffers again, and the display
    // surface is a new, never presented surface.
    assert_eq!(header_words(&m, f::BACK_BUFFER).0, BACK_BUFFER_DATA);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, DISPLAY_BUFFER_DATA);
    let front = d.front_buffer().unwrap();
    assert!(!old.contains(&front));
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    parity_textures(&mut m);
    d.set_texture(&mut m, 0, parity(0, true)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(snapshots(&d).last().copied(), Some(front));
}

fn presents(d: &Device<RecordingBackend>) -> Vec<u32> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Present { surface, .. } => Some(*surface),
            _ => None,
        })
        .collect()
}

fn copies(d: &Device<RecordingBackend>) -> Vec<(u32, u32)> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::CopySurface { src, dst } => Some((*src, *dst)),
            _ => None,
        })
        .collect()
}

/// The third frame-buffer header (`+0x1A1C`, two back buffers).
const THIRD_HEADER: u32 = f::DISPLAY_SURFACE + 4;

#[test]
fn copy_swap_flags_follow_the_original() {
    let (mut d, mut m) = create_copy_swap(RecordingBackend::new(), UploadPolicy::Always);
    let display = m.read_u32(L.device + f::DISPLAY_SURFACE).unwrap();
    m.write_u32(display + 8, 0x1111).unwrap();
    m.write_u32(L.device + f::SWAP_FENCES, 0x2222).unwrap();
    // FINISH alone, or no flag bit 1/2: nothing shown, no count
    // (0x3f9d35 -> 0x3f98f0 without the increment).
    for flags in [4, 8] {
        assert_eq!(d.swap(&mut m, flags).unwrap(), 0, "flags {flags}");
    }
    assert!(presents(&d).is_empty());
    // FINISH still stamps the display header and records a fence.
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).1, 0);
    assert_eq!(m.read_u32(L.device + f::SWAP_FENCES).unwrap(), 0);
    assert_eq!(m.read_u32(L.device + f::GPU_TIME).unwrap(), 2);
    // COPY alone counts and, with two frame buffers, scans out at once.
    let (back, front) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
    assert_eq!(d.swap(&mut m, 1).unwrap(), 1);
    assert_eq!(presents(&d), [back]);
    assert_eq!(d.back_buffer(), Some(back));
    // BYPASSCOPY counts and re-shows the display buffer, into which
    // nothing new is copied: the image the copy above put there.
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(copies(&d), [(back, front)]);
    assert_eq!(d.swap(&mut m, 2).unwrap(), 2);
    assert_eq!(presents(&d), [back, front]);
}

#[test]
fn copy_swap_with_two_back_buffers_exchanges_the_display_and_third_headers() {
    let (mut d, mut m) = create_with(RecordingBackend::new(), UploadPolicy::Always, |p| {
        p.swap_effect = 3;
        p.back_buffer_count = 2;
    });
    assert_eq!(m.read_u32(L.device + f::FRAME_BUFFER_COUNT).unwrap(), 3);
    assert_eq!(header_words(&m, f::BACK_BUFFER).0, BACK_BUFFER_DATA);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, DISPLAY_BUFFER_DATA);
    assert_eq!(header_words(&m, THIRD_HEADER).0, THIRD_BUFFER_DATA);
    let display = m.read_u32(L.device + f::DISPLAY_SURFACE).unwrap();
    m.write_u32(display + 8, 0x2222).unwrap();
    let (back, front) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
    // The third buffer's surface: the one a texture over its Data reads.
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    frame_texture(&mut m, THIRD_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let third = *snapshots(&d).last().unwrap();
    assert!(third != back && third != front);
    d.set_texture(&mut m, 0, 0).unwrap();

    // 0x3f991c: display and third trade Data and Lock; back stays.
    assert_eq!(d.swap(&mut m, 0).unwrap(), 1);
    assert_eq!(header_words(&m, f::BACK_BUFFER).0, BACK_BUFFER_DATA);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, THIRD_BUFFER_DATA);
    assert_eq!(
        header_words(&m, THIRD_HEADER),
        (DISPLAY_BUFFER_DATA, 0x2222)
    );
    assert_eq!(
        (d.back_buffer(), d.front_buffer()),
        (Some(back), Some(third))
    );
    assert_eq!(presents(&d), [back]);
    // The first write copies the image shown into the new display buffer.
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert_eq!(copies(&d), [(back, third)]);

    // Swapped again with nothing drawn: the pending copy reaches the
    // old display buffer before it becomes the third.
    d.swap(&mut m, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    assert_eq!(copies(&d), [(back, third), (back, front)]);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, THIRD_BUFFER_DATA);
    assert_eq!(d.front_buffer(), Some(third));
}

#[test]
fn a_lost_shown_image_does_not_outlive_the_device() {
    let (mut d, mut m) = create_copy_swap(RecordingBackend::new(), UploadPolicy::Always);
    d.backend.no_copies = true;
    d.swap(&mut m, 0).unwrap();
    // The image shown is in back0 and cannot be copied out before this
    // write: it is lost.
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    assert!(d.persist_display(&mut m).is_err());
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    frame_texture(&mut m, DISPLAY_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(
        d.take_events().iter().any(
            |e| matches!(e, Event::DrawSkipped { reason, .. } if reason.contains("drawn over"))
        )
    );
    d.set_texture(&mut m, 0, 0).unwrap();
    d.release_device(&mut m, L.device).unwrap();
    d.backend.no_copies = false;
    recreate(&mut d, &mut m);
    // A new device: its front was never drawn over.
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    frame_texture(&mut m, DISPLAY_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(d.take_events().is_empty());
    assert_eq!(snapshots(&d).last().copied(), d.front_buffer());
    d.set_texture(&mut m, 0, 0).unwrap();
    assert_eq!(d.persist_display(&mut m).unwrap(), 0);
}

#[test]
fn three_buffer_copy_swap_shows_the_copy_only_on_finish() {
    let (mut d, mut m) = create_with(RecordingBackend::new(), UploadPolicy::Always, |p| {
        p.swap_effect = 3;
        p.back_buffer_count = 2;
    });
    let (back, front) = (d.back_buffer().unwrap(), d.front_buffer().unwrap());
    // Frame 1 in back0. COPY alone: counted, display and third headers
    // exchanged, copy owed, but scanout does not move (0x3f9981).
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    assert_eq!(d.swap(&mut m, 1).unwrap(), 1);
    assert!(presents(&d).is_empty(), "nothing shown before FINISH");
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, THIRD_BUFFER_DATA);
    let display = d.front_buffer().unwrap();
    assert_ne!(display, front);
    // Frame 2 starts in back0: its first write copies frame 1 into the
    // display buffer.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    assert_eq!(copies(&d), [(back, display)]);
    // FINISH alone: scanout moves to the display header's buffer, holding
    // frame 1, not back0's frame 2; the count does not change.
    let fences = m.read_u32(L.device + f::SWAP_FENCES + 4).unwrap();
    assert_eq!(d.swap(&mut m, 4).unwrap(), 1);
    assert_eq!(presents(&d), [display]);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE), (THIRD_BUFFER_DATA, 2));
    let slot = m.read_u32(L.device + f::SWAP_FENCES + 4).unwrap();
    assert_ne!(slot, fences);
    assert_eq!(slot, 2);
    assert_eq!(m.read_u32(L.device + f::GPU_TIME).unwrap(), 4);
    // COPY | FINISH together: exchanged back, copied and shown at once.
    assert_eq!(d.swap(&mut m, 5).unwrap(), 2);
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE).0, DISPLAY_BUFFER_DATA);
    assert_eq!(d.front_buffer(), Some(front));
    assert_eq!(presents(&d), [display, back], "back0 holds the owed copy");
    assert_eq!(d.back_buffer(), Some(back));
}

#[test]
fn gpu_three_buffer_copy_then_finish_shows_the_copied_frame() {
    let Some((mut d, mut m, scope)) = gpu_with(|p| {
        p.swap_effect = 3;
        p.back_buffer_count = 2;
    }) else {
        return;
    };
    set_states(&mut d, &mut m);
    let (red, green) = ([0xFF, 0, 0, 0xFF], [0, 0xFF, 0, 0xFF]);
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 1).unwrap();
    let before = d.backend.last_presented();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    assert_eq!(
        d.backend.last_presented(),
        before,
        "COPY alone shows nothing"
    );
    d.swap(&mut m, 4).unwrap();
    let shown = d.backend.last_presented().unwrap();
    assert_eq!(rgba(&mut d, shown, 10, 10), red, "frame 1, copied");
    assert_eq!(back_buffer(&mut d)(10, 10), green, "frame 2 drawing on");
    finish(scope);
}

/// The display buffer as a render target while a COPY is owed: the copy is
/// made before the target is written, so FINISH shows what was drawn
/// into it afterwards.
#[test]
fn gpu_owed_copy_lands_before_the_display_buffer_is_drawn_into() {
    let Some((mut d, mut m, scope)) = gpu_with(|p| {
        p.swap_effect = 3;
        p.back_buffer_count = 2;
    }) else {
        return;
    };
    set_states(&mut d, &mut m);
    let (red, green) = ([0xFF, 0, 0, 0xFF], [0, 0xFF, 0, 0xFF]);
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 1).unwrap();
    let display = m.read_u32(L.device + f::DISPLAY_SURFACE).unwrap();
    d.set_render_target(&mut m, display, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.swap(&mut m, 4).unwrap();
    let shown = d.backend.last_presented().unwrap();
    assert_eq!(shown, d.front_buffer().unwrap());
    assert_eq!(rgba(&mut d, shown, 10, 10), green, "drawn over the copy");
    assert_eq!(back_buffer(&mut d)(10, 10), red, "back0 untouched");
    finish(scope);
}

/// CreateDevice again after a teardown, with the same parameters.
fn recreate<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    let out = call(
        d,
        m,
        0x3f5240,
        Regs {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..Regs::default()
        },
        &[PARAMS],
    )
    .unwrap();
    assert_eq!(out.eax, Some(0));
}

// GPU ---------------------------------------------------------------------

/// [`gpu`] with the present parameters changed by `edit`.
fn gpu_with(
    edit: impl FnOnce(&mut d3d::device::PresentParameters),
) -> Option<(Device<GfxBackend>, SparseMemory, wgpu::ErrorScopeGuard)> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (d, m) = create_with(GfxBackend::new(ctx), UploadPolicy::Always, edit);
    Some((d, m, scope))
}

/// Pixel `x, y` of `surface` as RGBA.
fn rgba(d: &mut Device<GfxBackend>, surface: gfx::surface::SurfaceId, x: u32, y: u32) -> [u8; 4] {
    let (px, order) = d.backend.read_color(surface).unwrap();
    let p = pixel(&px, x, y);
    match order {
        HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
        _ => p,
    }
}

/// The back buffer as RGBA.
fn back_buffer(d: &mut Device<GfxBackend>) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let (px, order) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    move |x, y| {
        let p = pixel(&px, x, y);
        match order {
            HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
            _ => p,
        }
    }
}

fn finish(scope: wgpu::ErrorScopeGuard) {
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}
