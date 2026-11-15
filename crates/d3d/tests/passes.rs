//! Render-pass coverage: scissor areas that are not one rectangle (several
//! inclusive rectangles, exclusive ones) and textures over the device's own
//! frame buffers (Halo's `0x14600` textures over the back, display and
//! depth surfaces). Pure region checks, the recording backend, and pixels
//! read back from synthetic frames on a real GPU (skipped without an
//! adapter).

mod common;

use common::*;
use d3d::device::{BACK_BUFFER_DATA, DEPTH_BUFFER_DATA, Rect, scissor_region};
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
    let pitch = w * 4;
    m.write_u32(FRAME_TEX, 0x0004_0001).unwrap();
    m.write_u32(FRAME_TEX + 4, data).unwrap();
    m.write_u32(FRAME_TEX + 8, 0).unwrap();
    m.write_u32(FRAME_TEX + 12, 1 | (2 << 4) | (format << 8) | (1 << 16))
        .unwrap();
    m.write_u32(
        FRAME_TEX + 16,
        (w - 1) | ((h - 1) << 12) | ((pitch / 64 - 1) << 24),
    )
    .unwrap();
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
    assert_eq!(data_of(&m, f::DISPLAY_SURFACE), BACK_BUFFER_DATA);
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
            Recorded::Snapshot { surface, id } => Some((*surface, *id)),
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
fn back_buffer(d: &Device<GfxBackend>) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let (px, order) = d
        .backend
        .store
        .read_color(&d.backend.ctx, d.back_buffer().unwrap())
        .unwrap();
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
    let px = back_buffer(&d);
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
    let px = back_buffer(&d);
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
    let px = back_buffer(&d);
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
    let px = back_buffer(&d);
    assert_eq!(px(8, 32), [0, 0xFF, 0, 0xFF]);
    assert_eq!(px(56, 32), [0, 0xFF, 0, 0x40]);
    finish(scope);
}
