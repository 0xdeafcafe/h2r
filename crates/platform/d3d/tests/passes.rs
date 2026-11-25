//! Render-pass coverage: scissor areas that are not one rectangle (several
//! inclusive rectangles, exclusive ones) and textures over the device's own
//! frame buffers (Halo's `0x14600` textures over the back, display and
//! depth surfaces). Pure region checks, the recording backend, and pixels
//! read back from synthetic frames on a real GPU (skipped without an
//! adapter).

use crate::common::*;
use d3d::abi::Regs;
use d3d::device::{
    BACK_BUFFER_DATA, COPY_SWAP_FLAG, DEPTH_BUFFER_DATA, DISPLAY_BUFFER_DATA, FRAME_COPY_SLOTS, Rect,
    THIRD_BUFFER_DATA, scissor_region,
};
use d3d::layout::device_field as f;
use d3d::{
    Device, Event, GfxBackend, GuestLayout, GuestMemory, PixelRect, Recorded, RecordingBackend,
    SparseMemory, UploadPolicy,
};
use d3d::resource::SurfaceHeader;
use gfx::{D3DFormat, GfxContext};
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
    assert_eq!(d.backend.draws().len(), draws);
    assert_eq!(d.backend.uploads(), 0);
    // Colour over the depth buffer (0x509368, 0x50936C): drawn, from the
    // depth buffer's bits (tests/depth_textures.rs has the details).
    for format in [0x12, 0x3F] {
        frame_texture(&mut m, DEPTH_BUFFER_DATA, format, W, H);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        let ev = d.take_events();
        assert!(ev.is_empty(), "{ev:?}");
    }
    assert_eq!(d.backend.draws().len(), draws + 2);
    assert_eq!(d.stats().depth_alias_uploads, 2);
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

/// Frame-buffer copies that made a texture.
fn copy_textures_made(d: &Device<RecordingBackend>) -> usize {
    d.backend
        .calls
        .iter()
        .filter(|c| matches!(c, Recorded::Snapshot { .. }))
        .count()
}

/// The surface the copy the last draw bound at stage 0 was last filled
/// from.
fn last_draw_copy_source(d: &Device<RecordingBackend>) -> Option<u32> {
    let id = last_stage0(d)?;
    d.backend.calls.iter().rev().find_map(|c| match c {
        Recorded::Snapshot { surface, id: i, .. } | Recorded::SnapshotInto { surface, id: i, .. }
            if *i == id =>
        {
            Some(*surface)
        }
        _ => None,
    })
}

/// For each draw, whether the copy it binds at stage 0 was made or
/// refilled after the draw before it: the draw reads the frame as of a
/// copy of its own, not one a later write made stale.
fn fresh_copies(d: &Device<RecordingBackend>) -> Vec<bool> {
    let mut fresh = Vec::new();
    let mut filled = Vec::new();
    for c in &d.backend.calls {
        match c {
            Recorded::Snapshot { id, .. } | Recorded::SnapshotInto { id, .. } => filled.push(*id),
            Recorded::Draw { textures, .. } => {
                fresh.push(textures[0].is_some_and(|t| filled.contains(&t)));
                filled.clear();
            }
            _ => {}
        }
    }
    fresh
}

fn header_words(m: &SparseMemory, field: u32) -> (u32, u32) {
    let header = m.read_u32(L.device + field).unwrap();
    (
        m.read_u32(header + 4).unwrap(),
        m.read_u32(header + 8).unwrap(),
    )
}

#[test]
fn a_frame_copy_in_a_fitted_ui_scope_keeps_the_whole_frame() {
    // At a wide output a dialog blurs the menu under it by copying the
    // 640x480 frame between targets inside the menu's widget fit. The copy
    // must cover the frame; a widget keeps the fit.
    let (mut d, mut m) = create_with(RecordingBackend::new(), UploadPolicy::Always, |p| {
        (p.width, p.height) = (640, 480);
    });
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 112.0, 64.0, 208.0, 160.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    let fit = d3d::backend::UiFit::about((2560, 1080), [320., 240.]);
    assert!(fit.is_some());
    d.set_ui_fit(&mut m, fit).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.draws().last().unwrap().ui_fit, fit);

    let back = data_of(&m, f::BACK_BUFFER);
    frame_texture(&mut m, back, 0x12, 640, 480);
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    quad_vb(&mut m, 0.0, 0.0, 640.0, 480.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let copy = (*d.backend.draws().last().unwrap()).clone();
    assert_eq!(copy.texture_info[0].map(|i| (i.width, i.height)), Some((640, 480)));
    assert_eq!(copy.ui_fit, None, "a frame copy covers the frame");

    // Unbound again: a widget keeps the fit.
    d.set_texture(&mut m, 0, 0).unwrap();
    quad_vb(&mut m, 112.0, 64.0, 208.0, 160.0, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.draws().last().unwrap().ui_fit, fit);
}

/// Full-screen fills, tints, fades and colour passes push a quad whose
/// positions reach both edges of the frame along the axis the UI fit
/// shrinks. At any display shape the fit places widgets, and such a quad
/// spans the display — the screen effects' hand-pushed strips have no
/// submission site `render_aspect` could read, so the device reads the
/// draw's own vertices (docs/graphics/high-resolution.md).
#[test]
fn full_frame_quads_span_the_display_in_a_fitted_ui_scope() {
    let (mut d, mut m) = create_with(RecordingBackend::new(), UploadPolicy::Always, |p| {
        (p.width, p.height) = (640, 480);
    });
    set_states(&mut d, &mut m);
    for output in [
        (1920, 1080), // 16:9
        (2560, 1080), // 64:27
        (3440, 1440), // 21:9
        (5120, 2880), // a 5K raster
        (5120, 2648), // the same window's drawable
        (1200, 1600), // taller than 4:3: the fit shrinks y
    ] {
        let fit = d3d::backend::UiFit::about(output, [320., 240.]);
        assert!(fit.is_some(), "a fit at {output:?}");
        d.set_ui_fit(&mut m, fit).unwrap();
        let shrinks_x = fit.unwrap().transform[0] < 1.0;

        // A widget inside the view keeps the fit.
        quad_vb(&mut m, 112.0, 64.0, 208.0, 160.0, 0xFFFF_FFFF);
        bind_quad(&mut d, &mut m);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        assert_eq!(d.backend.draws().last().unwrap().ui_fit, fit, "a widget at {output:?}");

        // A quad over the whole window is a full-frame pass.
        quad_vb(&mut m, 0.0, 0.0, 640.0, 480.0, 0xFFFF_FFFF);
        bind_quad(&mut d, &mut m);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        assert_eq!(
            d.backend.draws().last().unwrap().ui_fit,
            None,
            "the whole frame spans the display at {output:?}"
        );

        // A cutscene's letterbox band: full width, part height. It spans
        // the display along the fitted axis only.
        quad_vb(&mut m, 0.0, 60.0, 640.0, 420.0, 0xFFFF_FFFF);
        bind_quad(&mut d, &mut m);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        assert_eq!(
            d.backend.draws().last().unwrap().ui_fit,
            if shrinks_x { None } else { fit },
            "the cinema window at {output:?}"
        );
    }
}

const SCREEN_VAF: u32 = MISC + 0x7800;
const SCREEN_PROGRAM: u32 = MISC + 0x7A00;
const SCREEN_CONSTANTS: u32 = MISC + 0x7C00;

/// Selects a vertex program for immediate draws the way Halo does
/// (SelectVertexShaderDirect, every attribute format NONE): `placed` puts
/// `v0` on the screen through constant rows (`oPos = v0 * c[96] + c[97]`,
/// unit coordinates scaled to the 640x480 window); otherwise `oPos = v0`,
/// the window coordinates as given.
fn screen_program(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, placed: bool) {
    use shaders::vsh::encode::{Inst, Operand, program};
    use shaders::vsh::{MacOp, OutReg};
    let position = if placed {
        vec![
            Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(96)), None)
                .to_temp(0, 0xF),
            Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(97)))
                .to_out(false, OutReg::Pos, 0xF),
        ]
    } else {
        vec![Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF)]
    };
    let mut insts = position;
    insts.push(Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_out(false, OutReg::D0, 0xF));
    m.write(SCREEN_PROGRAM, &program(&insts)).unwrap();
    d.load_vertex_shader_program(m, SCREEN_PROGRAM, 0).unwrap();
    for (i, v) in [640.0f32, 480.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0].iter().enumerate() {
        m.write_u32(SCREEN_CONSTANTS + 4 * i as u32, v.to_bits()).unwrap();
    }
    d.set_vertex_shader_constant(m, 96, SCREEN_CONSTANTS, 2, true).unwrap();
    for reg in 0..16u32 {
        m.write_u32(SCREEN_VAF + 16 * reg + 8, 0x02).unwrap(); // D3DVSDT_NONE
    }
    d.select_vertex_shader_direct(m, SCREEN_VAF, 0).unwrap();
}

/// The title screen's backdrop passes are immediate-mode quads drawn by
/// vertex programs: inline Begin/End words in the push buffer and
/// `D3DDevice_End` strips, their positions passed through or placed from
/// constant rows. Where they land in the window, not their raw `v0`,
/// decides the fit: a quad over the whole window spans the display at any
/// shape, and a widget keeps the fit (docs/graphics/high-resolution.md).
#[test]
fn immediate_program_quads_over_the_frame_span_the_display_in_a_fitted_ui_scope() {
    let (mut d, mut m) = create_with(RecordingBackend::new(), UploadPolicy::Always, |p| {
        (p.width, p.height) = (640, 480);
    });
    set_states(&mut d, &mut m);
    for output in [(1920, 1080), (2496, 1662)] {
        let fit = d3d::backend::UiFit::about(output, [320., 240.]);
        assert!(fit.is_some_and(|f| f.transform[0] < 1.0), "a pillarbox fit at {output:?}");
        d.set_ui_fit(&mut m, fit).unwrap();
        let last = |d: &Device<RecordingBackend>| {
            let draws = d.backend.draws();
            let draw = draws.last().unwrap();
            (draw.entry, draw.ui_fit)
        };

        // Window coordinates passed through (`no constant rows`).
        screen_program(&mut d, &mut m, false);
        inline_quad(&mut m, 0.0, 0.0, 640.0, 480.0, [255; 4]);
        d.kick_off(&mut m).unwrap();
        assert_eq!(
            last(&d),
            ("inline Begin/End", None),
            "a pass-through inline quad over the frame at {output:?}"
        );
        inline_quad(&mut m, 112.0, 64.0, 208.0, 160.0, [255; 4]);
        d.kick_off(&mut m).unwrap();
        assert_eq!(
            last(&d),
            ("inline Begin/End", fit),
            "a pass-through inline widget at {output:?}"
        );

        // Unit coordinates the program places through c[96..=97].
        screen_program(&mut d, &mut m, true);
        inline_quad(&mut m, 0.0, 0.0, 1.0, 1.0, [255; 4]);
        d.kick_off(&mut m).unwrap();
        assert_eq!(
            last(&d),
            ("inline Begin/End", None),
            "a constant-placed inline quad over the frame at {output:?}"
        );
        inline_quad(&mut m, 112.0 / 640.0, 64.0 / 480.0, 208.0 / 640.0, 160.0 / 480.0, [255; 4]);
        d.kick_off(&mut m).unwrap();
        assert_eq!(
            last(&d),
            ("inline Begin/End", fit),
            "a constant-placed inline widget at {output:?}"
        );

        // The same through D3DDevice_Begin/SetVertexData/End.
        for (corners, want, what) in [
            ([0.0, 0.0, 1.0, 1.0], None, "over the frame"),
            ([0.175, 0.125, 0.325, 0.375], fit, "a widget"),
        ] {
            let [x0, y0, x1, y1] = corners;
            d.begin(&mut m, QUADLIST).unwrap();
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                d.set_vertex_data(&mut m, 3, [1.0; 4]).unwrap();
                d.set_vertex_data(&mut m, -1, [x, y, 0.5, 1.0]).unwrap();
            }
            d.end(&mut m).unwrap();
            assert_eq!(
                last(&d),
                ("D3DDevice_End", want),
                "a constant-placed D3DDevice_End quad, {what}, at {output:?}"
            );
        }
    }
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

/// A free-standing colour target the size of the frame: draws into it
/// sample the back buffer without writing it.
const OFFSCREEN: u32 = MISC + 0x7300;

fn offscreen(m: &mut SparseMemory) {
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, W, H, W * 4, 0x0040_0000)
        .write(m, OFFSCREEN)
        .unwrap();
}

/// The texture stage 0 of the last draw bound.
fn last_stage0(d: &Device<RecordingBackend>) -> Option<u32> {
    d.backend
        .calls
        .iter()
        .rev()
        .find_map(|c| match c {
            Recorded::Draw { textures, .. } => Some(textures[0]),
            _ => None,
        })
        .flatten()
}

/// `n` draws with the bindings as they are: the distinct copies they bound
/// at stage 0, in order.
fn sampling_draws(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, n: usize) -> Vec<u32> {
    let mut seen = Vec::new();
    for _ in 0..n {
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
        let t = last_stage0(d).expect("the draw samples the copy");
        if seen.last() != Some(&t) {
            seen.push(t);
        }
    }
    seen
}

#[test]
fn frame_sampling_draws_with_no_write_between_share_one_copy() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    offscreen(&mut m);
    let back = m.read_u32(L.device + f::BACK_BUFFER).unwrap();
    { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x12, W, H); }
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.set_render_target(&mut m, OFFSCREEN, 0).unwrap();
    // Five draws read the back buffer and write only the other target.
    let ids = sampling_draws(&mut d, &mut m, 5);
    assert_eq!(snapshots(&d), [d.back_buffer().unwrap()]);
    assert_eq!(ids.len(), 1, "every draw binds the one copy");
    let snap = d.backend.calls.iter().find_map(|c| match c {
        Recorded::Snapshot { id, .. } => Some(*id),
        _ => None,
    });
    assert_eq!(ids.first().copied(), snap);
    assert_eq!(d.stats().frame_snapshots, 1);
    assert_eq!(d.stats().frame_snapshot_reuses, 4);
    assert_eq!(d.frame_snapshots_held(), 1);
    // An X8 view of the same buffer forces alpha: a copy of its own.
    { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x1E, W, H); }
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.stats().frame_snapshots, 2);
    assert_eq!(d.frame_snapshots_held(), 2);
    assert!(d.take_events().is_empty());
    // A present ends what the copies hold and keeps their textures: the
    // next frame's sample refills one.
    d.set_render_target(&mut m, back, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    assert_eq!(d.frame_snapshots_held(), 2);
    { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x12, W, H); }
    d.set_render_target(&mut m, OFFSCREEN, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.stats().frame_snapshots, 3);
    assert_eq!(d.stats().frame_copy_textures, 2);
    assert_eq!(fresh_copies(&d).last(), Some(&true));
    assert!(matches!(
        d.backend.calls.iter().rev().find(|c| matches!(c, Recorded::Snapshot { .. } | Recorded::SnapshotInto { .. })),
        Some(Recorded::SnapshotInto { .. })
    ));
    assert!(d.take_events().is_empty());
}

#[test]
fn a_write_between_frame_samples_makes_a_new_copy() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    offscreen(&mut m);
    let back = m.read_u32(L.device + f::BACK_BUFFER).unwrap();
    { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x12, W, H); }
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.set_render_target(&mut m, OFFSCREEN, 0).unwrap();
    let first = sampling_draws(&mut d, &mut m, 2);
    assert_eq!(first.len(), 1);

    // A clear of the back buffer: the next sample sees it.
    d.set_render_target(&mut m, back, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    d.set_render_target(&mut m, OFFSCREEN, 0).unwrap();
    let second = sampling_draws(&mut d, &mut m, 2);
    assert_eq!(second.len(), 1);
    // The clear emptied the copy: the next sample refills its texture.
    assert_eq!(second, first);

    // A draw into the back buffer that samples it reads the copy from
    // before it, and its own write makes the next copy fresh: each of
    // these draws reads what the one before drew.
    d.set_render_target(&mut m, back, 0).unwrap();
    let feedback = sampling_draws(&mut d, &mut m, 3);
    assert_eq!(feedback, first, "one texture, refilled");
    assert_eq!(
        fresh_copies(&d),
        [true, false, true, false, false, true, true],
        "a copy after each write, and none where nothing wrote the frame"
    );
    assert_eq!(d.stats().frame_snapshots, 4);
    assert_eq!(d.stats().frame_copy_textures, 1);
    assert!(d.take_events().is_empty());
    assert_eq!(
        snapshots(&d),
        [d.back_buffer().unwrap(); 4],
        "every copy is of the back buffer"
    );
}

#[test]
fn a_long_session_of_frame_sampling_draws_holds_a_bounded_number_of_copies() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    offscreen(&mut m);
    let back = m.read_u32(L.device + f::BACK_BUFFER).unwrap();
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    let mut held = 0;
    for frame in 0..1000 {
        // The back buffer's Data turns over at each flip.
        { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x12, W, H); }
        // 25 draws that only read the frame (a distortion target), then
        // 25 that read and write it (refraction drawn into the frame).
        d.set_render_target(&mut m, OFFSCREEN, 0).unwrap();
        for _ in 0..25 {
            d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
            held = held.max(d.frame_snapshots_held());
        }
        d.set_render_target(&mut m, back, 0).unwrap();
        for _ in 0..25 {
            d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
            held = held.max(d.frame_snapshots_held());
        }
        d.swap(&mut m, 0).unwrap();
        assert!(d.frame_snapshots_held() <= FRAME_COPY_SLOTS, "frame {frame}");
        assert!(d.take_events().is_empty(), "frame {frame}");
        d.backend.calls.clear();
    }
    assert_eq!(held, 1);
    let s = d.stats();
    // Per frame: one copy for the 25 reads, which the first write's read
    // shares; each later write's read needs its own.
    assert_eq!(s.frame_snapshots, 1000 * 25);
    assert_eq!(s.frame_snapshot_reuses, 1000 * 25);
    // Every one of them refilled the one texture the first made.
    assert_eq!(s.frame_copy_textures, 1);
}

/// Particles blended into the frame they sample (refused native, drawn
/// through D3D): every draw writes the frame, so every draw needs a copy
/// of its own. The copies refill the pool's textures, so the textures
/// made stay at the pool's size however many draws there are, and each
/// draw still reads the frame as the draw before left it.
#[test]
fn five_hundred_frame_writing_samples_hold_at_most_the_pool() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    let back = m.read_u32(L.device + f::BACK_BUFFER).unwrap();
    { let data = data_of(&m, f::BACK_BUFFER); frame_texture(&mut m, data, 0x12, W, H); }
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    let ids = sampling_draws(&mut d, &mut m, 500);
    let made = copy_textures_made(&d);
    assert!(made <= FRAME_COPY_SLOTS, "{made} copy textures for 500 draws");
    assert!(d.frame_snapshots_held() <= FRAME_COPY_SLOTS);
    let mut bound = ids.clone();
    bound.sort_unstable();
    bound.dedup();
    assert!(bound.len() <= FRAME_COPY_SLOTS, "{} textures bound", bound.len());
    assert_eq!(fresh_copies(&d), vec![true; 500], "each draw reads a copy taken after the one before wrote");
    assert_eq!(snapshots(&d), vec![d.back_buffer().unwrap(); 500]);
    let s = d.stats();
    assert_eq!(s.frame_snapshots, 500);
    assert_eq!(s.frame_copy_textures, made as u64);
    assert!(d.take_events().is_empty());
}

/// The current and the previous frame both sampled, between writes, for
/// many frames: two contents in use at once, still within the pool, and
/// each draw's copy is of the buffer it names.
#[test]
fn current_and_previous_frame_samples_share_the_pool_across_presents() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    parity_textures(&mut m);
    for frame in 0..200u32 {
        for _ in 0..5 {
            for previous in [false, true] {
                d.set_texture(&mut m, 0, parity(frame, previous)).unwrap();
                d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
                let want = if previous { d.front_buffer() } else { d.back_buffer() };
                assert_eq!(last_draw_copy_source(&d), want, "frame {frame}, previous {previous}");
            }
        }
        assert!(d.frame_snapshots_held() <= FRAME_COPY_SLOTS, "frame {frame}");
        d.swap(&mut m, 0).unwrap();
        assert!(d.take_events().is_empty(), "frame {frame}");
    }
    assert!(copy_textures_made(&d) <= FRAME_COPY_SLOTS);
    let s = d.stats();
    // Per frame: each current sample follows a write (5 copies); the
    // previous frame is copied once and shared by the other 4.
    assert_eq!(s.frame_snapshots, 200 * 6);
    assert_eq!(s.frame_snapshot_reuses, 200 * 4);
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

fn gpu() -> Option<(Device<GfxBackend>, SparseMemory, wgpu::ErrorScopeGuard)> {
    gpu_with(|_| {})
}

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

/// The level drawn into, then sampled: eagerly through a write-back and an
/// upload, lazily from the surface itself; the same pixels.
#[test]
fn gpu_render_to_texture_then_sample_it() {
    for lazy in [false, true] {
        let Some((mut d, mut m, scope)) = gpu() else {
            return;
        };
        d.set_rtt_direct(lazy);
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
        assert_eq!(d.stats().rtt_writebacks, u64::from(!lazy));
        assert_eq!(d.stats().rtt_direct_samples, u64::from(lazy));
        let px = back_buffer(&mut d);
        assert_eq!(px(8, 32), [0, 0xFF, 0, 0xFF], "lazy {lazy}");
        assert_eq!(px(56, 32), [0, 0xFF, 0, 0x40], "lazy {lazy}");
        finish(scope);
    }
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

/// Review P2: a frame-buffer texture on a stage the draw does not read
/// leaves the draw alone (and is not read back).
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
    // Read on stage 0 it is sampled from a depth readback.
    d.set_texture(&mut m, 3, 0).unwrap();
    d.set_texture(&mut m, 0, FRAME_TEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(d.stats().depth_reads, 1);
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
/// (CLIPPLANE, PASSTHRU, dot-only) does not read the bound image, so a
/// depth alias on it is neither read back nor bound.
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
    assert_eq!(
        d.stats().depth_reads,
        0,
        "a stage that is not read is not read back"
    );
    // The same alias on a PROJECT2D stage is read, from a depth readback.
    def.texture_modes = texture_modes([0, 0, 0, TEX_PROJECT2D]);
    m.write(PSDEF, &def.to_bytes()).unwrap();
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(d.stats().depth_reads, 1);
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
    assert_eq!(back_buffer(&mut d)(10, 10), green);
    // The previous parity over the left half: frame 0's red, not this
    // frame's green.
    mirrored_textured_quad(&mut d, &mut m);
    d.set_texture(&mut m, 0, parity(1, true)).unwrap();
    scissor(&mut d, &mut m, Some([0, 0, 32, 64]));
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    assert_eq!(px(10, 10), red, "previous frame");
    assert_eq!(px(50, 10), blue);
    // The current parity, mirrored over everything: what this frame has
    // drawn so far, the previous-frame draw included (same-frame feedback).
    d.set_texture(&mut m, 0, parity(1, false)).unwrap();
    scissor(&mut d, &mut m, None);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    assert_eq!(px(10, 10), blue, "current frame, mirrored");
    assert_eq!(px(50, 10), red);
    // The front still holds frame 0: drawing frame 1 did not reach it.
    let front_id = d.front_buffer().unwrap();
    let (front, order) = d.backend.read_color(front_id).unwrap();
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
    let px = back_buffer(&mut d);
    assert_eq!(px(10, 10), red);
    assert_eq!(px(50, 10), blue);
    // And current is this frame again.
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_FF00, 1.0, 0).unwrap();
    d.set_texture(&mut m, 0, parity(2, false)).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(back_buffer(&mut d)(10, 10), green);
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
    assert_eq!(back_buffer(&mut d)(10, 10), [0, 0, 0, 0]);
    finish(scope);
}
