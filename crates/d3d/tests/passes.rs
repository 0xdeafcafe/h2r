//! Render-pass coverage: scissor areas that are not one rectangle (several
//! inclusive rectangles, exclusive ones) and textures over the device's own
//! frame buffers (Halo's `0x14600` textures over the back, display and
//! depth surfaces). Pure region checks, the recording backend, and pixels
//! read back from synthetic frames on a real GPU (skipped without an
//! adapter).

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::device::{
    BACK_BUFFER_DATA, COPY_SWAP_FLAG, DEPTH_BUFFER_DATA, DISPLAY_BUFFER_DATA, Rect,
    THIRD_BUFFER_DATA, scissor_region,
};
use d3d::layout::device_field as f;
use d3d::{
    Device, Event, GfxBackend, GuestLayout, GuestMemory, PixelRect, Recorded, RecordingBackend,
    SparseMemory, UploadPolicy,
};
use gfx::GfxContext;
use gfx::surface::HostPixelOrder;
use gfx::vertex::fvf;

const L: GuestLayout = GuestLayout::HALO2;
const RECTS: u32 = MISC + 0x7100;
const VIEWPORT: u32 = MISC + 0x7180;
const FRAME_TEX: u32 = MISC + 0x7200;

const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
/// Xbox blend factors are the NV2A (GL) values: `D3DBLEND_ONE` is 1.
const D3DBLEND_ONE: u32 = 1;

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

fn last_clip(d: &Device<RecordingBackend>) -> (Option<PixelRect>, Vec<PixelRect>) {
    let draw = d.backend.draws().last().copied().unwrap().clone();
    (draw.clip, draw.clip_rects)
}

#[test]
fn several_and_exclusive_scissors_reach_the_backend_as_pieces() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    rects(&mut m, &[[0, 0, 40, 40], [20, 20, 60, 60]]);
    d.set_scissors(&mut m, 2, 0, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (clip, pieces) = last_clip(&d);
    let c = clip.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (0, 0, 60, 60));
    assert_eq!(pieces.len(), 3);
    assert_eq!(d.stats().scissor_split_draws, 1);

    // Exclusive: the frame around the rectangle.
    rects(&mut m, &[[16, 16, 48, 48]]);
    d.set_scissors(&mut m, 1, 1, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (clip, pieces) = last_clip(&d);
    let c = clip.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (0, 0, W, H));
    assert_eq!(pieces.len(), 4);
    assert!(
        pieces
            .iter()
            .all(|p| !(p.x < 48 && 16 < p.x + p.width && p.y < 48 && 16 < p.y + p.height))
    );

    // An exclusive rectangle over everything: nothing reaches the backend.
    let draws = d.backend.draws().len();
    rects(&mut m, &[[0, 0, 1000, 1000]]);
    d.set_scissors(&mut m, 1, 1, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.draws().len(), draws);

    // SetViewport resets the scissors, exclusive mode included.
    for (i, w) in [0, 0, W, H, 0.0f32.to_bits(), 1.0f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(VIEWPORT + 4 * i as u32, *w).unwrap();
    }
    d.set_viewport(&mut m, VIEWPORT).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (clip, pieces) = last_clip(&d);
    let c = clip.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (0, 0, W, H));
    assert!(pieces.is_empty());
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, Event::DrawSkipped { .. }))
    );
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

#[test]
fn textures_over_the_frame_buffers_sample_a_back_buffer_copy_or_skip() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    // The headers the device creates name their buffers.
    assert_eq!(data_of(&m, f::BACK_BUFFER), BACK_BUFFER_DATA);
    assert_eq!(data_of(&m, f::DISPLAY_SURFACE), DISPLAY_BUFFER_DATA);
    assert_eq!(data_of(&m, f::DEPTH_STENCIL), DEPTH_BUFFER_DATA);
    quad(&mut d, &mut m, 0xFFFF_FFFF);

    // LIN_A8R8G8B8 over the back buffer, as 0x509354 is.
    frame_texture(&mut m, BACK_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let snap = d
        .backend
        .calls
        .iter()
        .find_map(|c| match c {
            Recorded::Snapshot { surface, id, .. } => Some((*surface, *id)),
            _ => None,
        })
        .expect("snapshot");
    assert_eq!(Some(snap.0), d.back_buffer());
    match d.backend.calls.last() {
        Some(Recorded::Draw { textures, draw, .. }) => {
            assert_eq!(textures[0], Some(snap.1));
            let info = draw.texture_info[0].unwrap();
            assert_eq!((info.width, info.height), (W, H));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(d.backend.uploads(), 0, "guest memory is not read");
    assert_eq!(d.stats().frame_snapshots, 1);
    assert!(d.take_events().is_empty());

    // Not the whole image, the wrong format, or inside but not at the
    // start: skipped with a reason, no draw.
    let draws = d.backend.draws().len();
    for (data, format, w, h) in [
        (BACK_BUFFER_DATA, 0x12, 32, 32),
        (BACK_BUFFER_DATA, 0x06, W, H),
        (BACK_BUFFER_DATA + 0x100, 0x12, W, H),
    ] {
        frame_texture(&mut m, data, format, w, h);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        let ev = d.take_events();
        assert!(
            ev.iter().any(
                |e| matches!(e, Event::DrawSkipped { reason, .. } if reason.contains("back buffer"))
            ),
            "{ev:?}"
        );
    }
    // Colour over the depth buffer (0x509368, 0x50936C): skipped.
    for format in [0x12, 0x3F] {
        frame_texture(&mut m, DEPTH_BUFFER_DATA, format, W, H);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        let ev = d.take_events();
        assert!(
            ev.iter().any(|e| matches!(e, Event::DrawSkipped { reason, .. } if reason.contains("depth buffer"))),
            "{ev:?}"
        );
    }
    assert_eq!(d.backend.draws().len(), draws);
    assert_eq!(d.backend.uploads(), 0);
}

fn snapshots(d: &Device<RecordingBackend>) -> Vec<u32> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Snapshot { surface, .. } => Some(*surface),
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
    assert_eq!(
        header_words(&m, f::DISPLAY_SURFACE),
        (BACK_BUFFER_DATA, 0x1111)
    );
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
    assert_eq!(header_words(&m, f::BACK_BUFFER), (BACK_BUFFER_DATA, 0x1111));
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
    // FINISH alone, or no flag bit 1/2: nothing shown, no count
    // (0x3f9d35 -> 0x3f98f0 without the increment).
    for flags in [4, 8] {
        assert_eq!(d.swap(&mut m, flags).unwrap(), 0, "flags {flags}");
    }
    assert!(presents(&d).is_empty());
    // BYPASSCOPY without COPY: a stop, not a silent guess.
    let e = d.swap(&mut m, 2).unwrap_err();
    assert!(e.to_string().contains("BYPASSCOPY"), "{e}");
    assert!(presents(&d).is_empty());
    // COPY alone counts and shows back0.
    let back = d.back_buffer().unwrap();
    assert_eq!(d.swap(&mut m, 1).unwrap(), 1);
    assert_eq!(presents(&d), [back]);
    assert_eq!(d.back_buffer(), Some(back));
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
    assert_eq!(header_words(&m, f::DISPLAY_SURFACE), (THIRD_BUFFER_DATA, 0));
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

fn gpu() -> Option<(Device<GfxBackend>, SparseMemory, wgpu::ErrorScopeGuard)> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (d, m) = create(GfxBackend::new(ctx), UploadPolicy::Always);
    Some((d, m, scope))
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

fn no_skips(d: &mut Device<GfxBackend>) {
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
}

fn finish(scope: wgpu::ErrorScopeGuard) {
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

#[test]
fn gpu_inclusive_scissors_blend_every_pixel_once() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    quad(&mut d, &mut m, 0xFF40_4040);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    // Additive blending: a pixel drawn twice would read 0x80.
    d.set_render_state(&mut m, RS_ALPHABLENDENABLE, 1).unwrap();
    d.set_render_state(&mut m, RS_SRCBLEND, D3DBLEND_ONE)
        .unwrap();
    d.set_render_state(&mut m, RS_DESTBLEND, D3DBLEND_ONE)
        .unwrap();
    rects(&mut m, &[[0, 0, 40, 40], [20, 20, 60, 60]]);
    d.set_scissors(&mut m, 2, 0, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    let grey = [0x40, 0x40, 0x40, 0xFF];
    let black = [0, 0, 0, 0xFF];
    assert_eq!(px(10, 10), grey);
    assert_eq!(px(30, 30), grey, "overlap drawn once");
    assert_eq!(px(50, 50), grey);
    assert_eq!(px(39, 19), grey);
    assert_eq!(px(50, 10), black);
    assert_eq!(px(10, 50), black);
    assert_eq!(px(62, 62), black);
    finish(scope);
}

#[test]
fn gpu_exclusive_scissor_leaves_the_inside_alone() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    quad(&mut d, &mut m, 0xFFFF_0000);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    rects(&mut m, &[[16, 16, 48, 48]]);
    d.set_scissors(&mut m, 1, 1, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    let (red, blue) = ([0xFF, 0, 0, 0xFF], [0, 0, 0xFF, 0xFF]);
    for (x, y) in [(0, 0), (8, 32), (47, 15), (48, 48), (63, 63)] {
        assert_eq!(px(x, y), red, "{x},{y} outside");
    }
    for (x, y) in [(16, 16), (32, 32), (47, 47)] {
        assert_eq!(px(x, y), blue, "{x},{y} inside");
    }
    finish(scope);
}

/// `XYZRHW | DIFFUSE | TEX1` over the whole target, white, with texel
/// coordinates mirrored left to right.
fn mirrored_textured_quad(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for v in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for v in [W as f32 * (1.0 - x), H as f32 * y] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
}

#[test]
fn gpu_texture_over_the_back_buffer_samples_what_was_drawn() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    // Left half red, right half blue, drawn on the host only.
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    m.write(RECTS, &[32u32, 0, 64, 64].map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(&mut m, 1, RECTS, 0xF0, 0xFF00_00FF, 1.0, 0)
        .unwrap();
    // Halo's texture over the back buffer, sampled while the back buffer
    // is the target, mirrored: the copy is what the draw reads.
    let back = data_of(&m, f::BACK_BUFFER);
    frame_texture(&mut m, back, 0x12, W, H);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    mirrored_textured_quad(&mut d, &mut m);
    // Stage 0 default: COLOROP MODULATE(texture, diffuse white).
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(d.stats().frame_snapshots, 1);
    let px = back_buffer(&mut d);
    let (red, blue) = ([0xFF, 0, 0, 0xFF], [0, 0, 0xFF, 0xFF]);
    assert_eq!(px(4, 32), blue);
    assert_eq!(px(28, 10), blue);
    assert_eq!(px(36, 50), red);
    assert_eq!(px(60, 32), red);
    finish(scope);
}

#[test]
fn gpu_render_to_texture_then_sample_it() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    // The 16x4 LIN_A8R8G8B8 test texture, its level bound as the target.
    texture(&mut m, 0x55);
    let level = MISC + 0x7300;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, level).unwrap();
    let back = m.read_u32(L.device + f::RENDER_TARGET).unwrap();
    d.set_render_target(&mut m, level, 0).unwrap();
    // Green everywhere, then an alpha-only clear of the right half to 0x40.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    m.write(RECTS, &[8u32, 0, 16, 4].map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(&mut m, 1, RECTS, 0x80, 0x4000_0000, 1.0, 0)
        .unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    // Sample the texture across the back buffer (texel coordinates).
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for v in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for v in [16.0 * x, 4.0 * y] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(d.stats().rtt_writebacks, 1);
    let px = back_buffer(&mut d);
    assert_eq!(px(8, 32), [0, 0xFF, 0, 0xFF]);
    assert_eq!(px(56, 32), [0, 0xFF, 0, 0x40]);
    finish(scope);
}

fn small_viewport<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    for (i, w) in [0, 0, 32, 32, 0.0f32.to_bits(), 1.0f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(VIEWPORT + 4 * i as u32, *w).unwrap();
    }
    d.set_viewport(m, VIEWPORT).unwrap();
}

/// Review P2: with scissors set, the window clip replaces the viewport
/// (`0x3f8928` sets the surface clip to the whole target).
#[test]
fn window_scissors_are_not_clipped_by_the_viewport() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    small_viewport(&mut d, &mut m);
    // No scissors: the 32x32 viewport.
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let c = last_clip(&d).0.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (0, 0, 32, 32));
    // Two inclusive rectangles outside the viewport still draw.
    rects(&mut m, &[[40, 0, 48, 8], [0, 40, 8, 48]]);
    d.set_scissors(&mut m, 2, 0, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (_, pieces) = last_clip(&d);
    assert_eq!(tuples(&pieces), [(40, 0, 8, 8), (0, 40, 8, 8)]);
    // One inclusive rectangle is the surface clip (0x3f886c).
    rects(&mut m, &[[40, 40, 60, 60]]);
    d.set_scissors(&mut m, 1, 0, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (c, pieces) = last_clip(&d);
    let c = c.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (40, 40, 20, 20));
    assert!(pieces.is_empty());
    // Exclusive: everything on the target outside the rectangle.
    rects(&mut m, &[[8, 8, 16, 16]]);
    d.set_scissors(&mut m, 1, 1, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let (c, pieces) = last_clip(&d);
    let c = c.unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (0, 0, W, H));
    let hit = covered(&pieces, W, H);
    assert!(hit[40] && hit[(63 * W + 63) as usize] && !hit[(10 * W + 10) as usize]);
}

#[test]
fn gpu_window_scissors_draw_outside_a_small_viewport() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    quad(&mut d, &mut m, 0xFFFF_0000);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    small_viewport(&mut d, &mut m);
    rects(&mut m, &[[40, 0, 48, 8], [0, 40, 8, 48]]);
    d.set_scissors(&mut m, 2, 0, RECTS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    let (red, blue) = ([0xFF, 0, 0, 0xFF], [0, 0, 0xFF, 0xFF]);
    assert_eq!(px(44, 4), red);
    assert_eq!(px(4, 44), red);
    assert_eq!(px(16, 16), blue, "inside the viewport, outside the windows");
    finish(scope);
}

/// Review P2: an unsupported frame-buffer texture on a stage the draw does
/// not read leaves the draw alone.
#[test]
fn gpu_unused_depth_texture_stage_does_not_cancel_the_draw() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    quad(&mut d, &mut m, 0xFFFF_0000);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    // Stage 1 is disabled by default, so stage 3 is never read.
    frame_texture(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 3, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(back_buffer(&mut d)(32, 32), [0xFF, 0, 0, 0xFF]);
    // Read on stage 0 it is still skipped.
    d.set_texture(&mut m, 3, 0).unwrap();
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(
        d.take_events()
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. }))
    );
    finish(scope);
}

/// Review P2: an X8 texture over the back buffer reads alpha as one.
#[test]
fn gpu_x8_texture_over_the_back_buffer_reads_opaque_alpha() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0x4011_2233, 1.0, 0).unwrap();
    let back = data_of(&m, f::BACK_BUFFER);
    for (format, alpha) in [(0x1E, 0xFF), (0x12, 0x40)] {
        // LIN_X8R8G8B8 (0x1E): opaque; LIN_A8R8G8B8: the stored alpha.
        frame_texture(&mut m, back, format, W, H);
        d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
        mirrored_textured_quad(&mut d, &mut m);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        no_skips(&mut d);
        assert_eq!(
            back_buffer(&mut d)(20, 20),
            [0x11, 0x22, 0x33, alpha],
            "format {format:#x}"
        );
        // Reset for the next round.
        d.clear(&mut m, 0, 0, 0xF0, 0x4011_2233, 1.0, 0).unwrap();
    }
    finish(scope);
}

/// Review P2 (re-check): a pixel-shader stage whose mode fetches no texel
/// (CLIPPLANE, PASSTHRU, dot-only) does not read the bound image, so an
/// unsupported depth alias on it leaves the draw alone.
#[test]
fn gpu_clipplane_stage_over_a_depth_alias_does_not_cancel_the_draw() {
    use shaders::psh::PixelShaderDef;
    use shaders::psh::bits::*;
    const PSDEF: u32 = MISC + 0x7400;
    const PS_OBJECT: u32 = MISC + 0x7500;
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    quad(&mut d, &mut m, 0xFFFF_0000);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    // r0 = v0 (diffuse); stage 3 texkill on its coordinates, which the
    // pre-transformed quad leaves at 0 (nonnegative: nothing is killed).
    let mut def = PixelShaderDef {
        combiner_count: 1,
        texture_modes: texture_modes([0, 0, 0, TEX_CLIPPLANE]),
        ..PixelShaderDef::default()
    };
    // r0 = v0 * 1.
    def.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    frame_texture(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H);
    d.set_texture(&mut m, 3, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(back_buffer(&mut d)(32, 32), [0xFF, 0, 0, 0xFF]);
    // The same alias on a PROJECT2D stage is read: skipped.
    def.texture_modes = texture_modes([0, 0, 0, TEX_PROJECT2D]);
    m.write(PSDEF, &def.to_bytes()).unwrap();
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(
        d.take_events()
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. }))
    );
    finish(scope);
}

/// Whole-target clip to `rect` (`[x1, y1, x2, y2]`), or none.
fn scissor(d: &mut Device<GfxBackend>, m: &mut SparseMemory, rect: Option<[u32; 4]>) {
    match rect {
        Some(r) => {
            rects(m, &[r]);
            d.set_scissors(m, 1, 0, RECTS).unwrap();
        }
        None => d.set_scissors(m, 0, 0, RECTS).unwrap(),
    }
}

#[test]
fn gpu_previous_frame_reads_the_image_presented_and_current_reads_this_frame() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    parity_textures(&mut m);
    let (red, green, blue) = ([0xFF, 0, 0, 0xFF], [0, 0xFF, 0, 0xFF], [0, 0, 0xFF, 0xFF]);
    // Frame 0: red.
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    // Frame 1: green, the right half blue.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    m.write(RECTS, &[32u32, 0, 64, 64].map(u32::to_le_bytes).concat())
        .unwrap();
    d.clear(&mut m, 1, RECTS, 0xF0, 0xFF00_00FF, 1.0, 0)
        .unwrap();
    assert_eq!(back_buffer(&d)(10, 10), green);
    // The previous parity over the left half: frame 0's red, not this
    // frame's green.
    mirrored_textured_quad(&mut d, &mut m);
    d.set_texture(&mut m, 0, parity(1, true)).unwrap();
    scissor(&mut d, &mut m, Some([0, 0, 32, 64]));
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&d);
    assert_eq!(px(10, 10), red, "previous frame");
    assert_eq!(px(50, 10), blue);
    // The current parity, mirrored over everything: what this frame has
    // drawn so far, the previous-frame draw included (same-frame feedback).
    d.set_texture(&mut m, 0, parity(1, false)).unwrap();
    scissor(&mut d, &mut m, None);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&d);
    assert_eq!(px(10, 10), blue, "current frame, mirrored");
    assert_eq!(px(50, 10), red);
    // The front still holds frame 0: drawing frame 1 did not reach it.
    let (front, order) = d
        .backend
        .store
        .read_color(&d.backend.ctx, d.front_buffer().unwrap())
        .unwrap();
    let p = pixel(&front, 10, 10);
    let p = match order {
        HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
        _ => p,
    };
    assert_eq!(p, red);
    d.swap(&mut m, 0).unwrap();
    // Frame 2: the parities have turned over. Previous is frame 1's final
    // image (left blue, right red), mirrored.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_texture(&mut m, 0, parity(2, true)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&d);
    assert_eq!(px(10, 10), red);
    assert_eq!(px(50, 10), blue);
    // And current is this frame again.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_texture(&mut m, 0, parity(2, false)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(back_buffer(&d)(10, 10), green);
    assert_eq!(d.stats().previous_frame_snapshots, 2);
    finish(scope);
}

#[test]
fn gpu_previous_frame_is_empty_after_the_device_is_recreated() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    d.release_device(&mut m, L.device).unwrap();
    recreate(&mut d, &mut m);
    set_states(&mut d, &mut m);
    // Halo rebuilds its textures from the new headers (0x12b9e).
    parity_textures(&mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_FFFF, 1.0, 0).unwrap();
    mirrored_textured_quad(&mut d, &mut m);
    d.set_texture(&mut m, 0, parity(0, true)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    // Nothing was presented on this device: the old red does not leak in.
    assert_eq!(back_buffer(&d)(10, 10), [0, 0, 0, 0]);
    finish(scope);
}
