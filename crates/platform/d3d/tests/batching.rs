//! `GfxBackend` batches consecutive draws to one target into one render
//! pass and packs their per-draw data into upload rings. These tests check
//! the result matches drawing each one alone: a switch to another target
//! submits the pending pass before anything of the next draw is staged, and
//! many draws share one pass. Synthetic resources; skipped without a GPU.

mod common;

use common::*;
use d3d::layout::device_field as f;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;

const DEV: u32 = 0x0040_4FE0;
const RS_TEXTUREFACTOR: u32 = 148;

fn gpu() -> Option<(Device<GfxBackend>, SparseMemory)> {
    match GfxContext::headless() {
        Ok(ctx) => Some(create(GfxBackend::new(ctx), UploadPolicy::Tracked)),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

/// `XYZRHW | DIFFUSE` quads (strip order) at `VB_HEADER`, one per rectangle.
fn quads(m: &mut SparseMemory, rects: &[(f32, f32, f32, f32)]) {
    let mut bytes = Vec::new();
    for &(x0, y0, x1, y1) in rects {
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            bytes.extend(vertex(x, y, 0xFFFF_FFFF));
        }
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
}

/// Stage 0 outputs the texture factor: the draw goes through the shader
/// provider, with its colour in the pixel uniforms.
fn tfactor_stage(m: &mut SparseMemory) {
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 3).unwrap(); // ALPHAARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 13), 2).unwrap(); // ALPHAOP = SELECTARG1
}

/// The 16x4 `LIN_A8R8G8B8` level of the test texture, as the target.
fn texture_target(m: &mut SparseMemory) -> u32 {
    texture(m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(m, surf).unwrap();
    surf
}

#[test]
fn a_draw_after_a_target_switch_keeps_its_uniforms() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    tfactor_stage(&mut m);
    // Quad 0: the back buffer's top-left corner; quad 1: the left half of
    // the 16x4 target; quad 2: its right half.
    quads(
        &mut m,
        &[
            (0.0, 0.0, 8.0, 8.0),
            (0.0, 0.0, 8.0, 4.0),
            (8.0, 0.0, 16.0, 4.0),
        ],
    );
    bind_quad(&mut d, &mut m);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    // Visit the texture target once, so binding it again needs no backend
    // call (its host surface exists and matches guest memory).
    let surf = texture_target(&mut m);
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFFFF_0000)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();

    // The first draw on the new target stages its uniforms while the back
    // buffer's pass is still pending; the draws after it stage more, enough
    // to reach where the first one's would be if the switch's submit had
    // let the ring start over under it.
    d.set_render_target(&mut m, surf, 0).unwrap();
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
    for i in 0..32u32 {
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_0000 | i)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 8, 4).unwrap();
    }
    d.set_render_target(&mut m, back, 0).unwrap();
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
    );
    // Written back to guest memory when the guest waits on the texture
    // (A8R8G8B8: B, G, R, A).
    d.block_on_resource(&mut m, TEX_HEADER).unwrap();
    let texel = |m: &SparseMemory, x: u32, y: u32| {
        m.read_u32(0x8000_0000 | (TEX_DATA + 64 * y + 4 * x))
            .unwrap()
    };
    for (x, y) in [(0, 0), (7, 3), (3, 1)] {
        assert_eq!(texel(&m, x, y), 0xFF00_FF00, "left half at ({x}, {y})");
    }
    assert_eq!(texel(&m, 12, 2), 0xFF00_001F, "right half: the last draw");
    let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    // The back buffer reads back in memory order: B, G, R, A.
    assert_eq!(pixel(&px, 2, 2), [0, 0, 0xFF, 0xFF], "back buffer quad");
}

/// The clear and eight draws to one target: one submission, encoded as one
/// render pass per draw by default and as one render pass when merged; the
/// pixels are the same either way.
#[test]
fn draws_to_one_target_share_a_submission() {
    for merge in [false, true] {
        let Some((mut d, mut m)) = gpu() else {
            return;
        };
        d.backend.set_merge_passes(merge);
        set_states(&mut d, &mut m);
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        let rects: Vec<_> = (0..8)
            .map(|i| {
                let x = 8.0 * i as f32;
                (x, 0.0, x + 8.0, 8.0)
            })
            .collect();
        quads(&mut m, &rects);
        bind_quad(&mut d, &mut m);
        let before = d.backend.batch_stats();
        for i in 0..8 {
            d.draw_vertices(&mut m, TRIANGLESTRIP, 4 * i, 4).unwrap();
        }
        let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
        let after = d.backend.batch_stats();
        assert_eq!(after.draws - before.draws, 8);
        assert_eq!(after.passes - before.passes, 1, "one pending pass");
        assert_eq!(after.submits - before.submits, 1, "one submission");
        let render_passes = if merge { 1 } else { 8 };
        assert_eq!(
            after.render_passes - before.render_passes,
            render_passes,
            "merge {merge}"
        );
        for i in 0..8 {
            assert_eq!(pixel(&px, 8 * i + 4, 4), [0xFF; 4], "quad {i}");
        }
        assert_eq!(pixel(&px, 4, 12), [0, 0, 0, 0xFF], "below the quads");
    }
}

/// Many draws to one target. Merged into one render pass, they stage more
/// than the program uniform ring holds (4 MiB at the 256-byte offset
/// alignment Metal and most adapters have): the ring moves to a new buffer
/// inside the pending pass, and the draws recorded before the move keep
/// their uniforms. One render pass per draw, the pending pass is submitted
/// about every 1024 draws (Metal loses the device near 2048 render passes
/// in one submission). The pixels are the same either way.
#[test]
fn many_draws_to_one_target_keep_their_uniforms() {
    for merge in [true, false] {
        let Some((mut d, mut m)) = gpu() else {
            return;
        };
        d.backend.set_merge_passes(merge);
        set_states(&mut d, &mut m);
        tfactor_stage(&mut m);
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        quads(
            &mut m,
            &[
                (0.0, 0.0, 8.0, 8.0),
                (8.0, 0.0, 16.0, 8.0),
                (16.0, 0.0, 24.0, 8.0),
            ],
        );
        bind_quad(&mut d, &mut m);
        let before = d.backend.batch_stats();
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFFFF_0000)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        // A new texture factor each draw: new pixel uniforms each time.
        const N: u32 = 17_000;
        for i in 0..N {
            d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_0000 | i)
                .unwrap();
            d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
        }
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 8, 4).unwrap();
        let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
        let after = d.backend.batch_stats();
        assert_eq!(after.draws - before.draws, N as u64 + 2);
        let passes = after.passes - before.passes;
        if merge {
            assert_eq!(passes, 1, "one pending pass");
        } else {
            assert!((16..=18).contains(&passes), "{passes} pending passes");
        }
        // B, G, R, A.
        assert_eq!(
            pixel(&px, 4, 4),
            [0, 0, 0xFF, 0xFF],
            "first draw, merge {merge}"
        );
        let last = N - 1;
        assert_eq!(
            pixel(&px, 12, 4),
            [last as u8, (last >> 8) as u8, 0, 0xFF],
            "last of the many, merge {merge}"
        );
        assert_eq!(
            pixel(&px, 20, 4),
            [0, 0xFF, 0, 0xFF],
            "after them, merge {merge}"
        );
    }
}

/// Under the exact merge rule (`set_merge_masks(false)`) a draw whose program can `discard`
/// starts a render pass of its own: on Apple GPUs such a draw's
/// implicit-LOD samples at triangle edges depend on what earlier draws in
/// the same render pass left in the tile (docs/graphics/graphics.md, "Merged
/// passes"). Three provider draws, the middle one alpha-tested: two render
/// passes merged, three per draw, and the same pixels.
#[test]
fn a_draw_that_can_discard_starts_its_own_render_pass() {
    let mut pixels = Vec::new();
    for merge in [None, Some(false)] {
        let Some((mut d, mut m)) = gpu() else {
            return;
        };
        if let Some(merge) = merge {
            d.backend.set_merge_passes(merge);
        }
        d.backend.set_merge_masks(false);
        set_states(&mut d, &mut m);
        tfactor_stage(&mut m);
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        quads(
            &mut m,
            &[
                (0.0, 0.0, 8.0, 8.0),
                (8.0, 0.0, 16.0, 8.0),
                (16.0, 0.0, 24.0, 8.0),
            ],
        );
        bind_quad(&mut d, &mut m);
        let l = d3d::GuestLayout::HALO2;
        let before = d.backend.batch_stats();
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFFFF_0000)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        m.write_u32(l.render_state_slot(60), 1).unwrap(); // ALPHATESTENABLE
        m.write_u32(l.render_state_slot(58), 0x204).unwrap(); // ALPHAFUNC = GREATER
        m.write_u32(l.render_state_slot(61), 0x80).unwrap(); // ALPHAREF
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
        m.write_u32(l.render_state_slot(60), 0).unwrap();
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_00FF)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 8, 4).unwrap();
        assert!(d.take_events().is_empty());
        let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
        let after = d.backend.batch_stats();
        assert_eq!(after.draws - before.draws, 3);
        assert_eq!(after.passes - before.passes, 1, "one pending pass");
        let render_passes = if merge.is_none() { 2 } else { 3 };
        assert_eq!(
            after.render_passes - before.render_passes,
            render_passes,
            "merge {merge:?}"
        );
        // B, G, R, A.
        assert_eq!(pixel(&px, 4, 4), [0, 0, 0xFF, 0xFF]);
        assert_eq!(pixel(&px, 12, 4), [0, 0xFF, 0, 0xFF]);
        assert_eq!(pixel(&px, 20, 4), [0xFF, 0, 0, 0xFF]);
        pixels.push(px);
    }
    assert!(pixels[0] == pixels[1], "merged and per-draw pixels differ");
}

/// With merged passes, a draw after one that writes only some colour
/// channels starts a new render pass unless it writes the same channels:
/// on Apple GPUs a later draw's edge pixels otherwise depend on that draw
/// (docs/graphics/graphics.md, "Merged passes"). Three provider draws, the first
/// with alpha writes off: two render passes merged, three per draw, and the
/// same pixels.
#[test]
fn a_draw_after_a_partly_masked_draw_starts_a_new_render_pass() {
    let mut pixels = Vec::new();
    for merge in [None, Some(false)] {
        let Some((mut d, mut m)) = gpu() else {
            return;
        };
        if let Some(merge) = merge {
            d.backend.set_merge_passes(merge);
        }
        // The exact rule splits here; the loose default does not
        // (the_loose_merge_rule_does_not_split_after_a_masked_draw).
        d.backend.set_merge_masks(false);
        set_states(&mut d, &mut m);
        tfactor_stage(&mut m);
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        quads(
            &mut m,
            &[
                (0.0, 0.0, 8.0, 8.0),
                (8.0, 0.0, 16.0, 8.0),
                (16.0, 0.0, 24.0, 8.0),
            ],
        );
        bind_quad(&mut d, &mut m);
        let l = d3d::GuestLayout::HALO2;
        let before = d.backend.batch_stats();
        m.write_u32(l.render_state_slot(67), 0x0001_0101).unwrap(); // COLORWRITEENABLE = RGB
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0x00FF_0000)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        m.write_u32(l.render_state_slot(67), 0x0101_0101).unwrap(); // ALL
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
        d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_00FF)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 8, 4).unwrap();
        assert!(d.take_events().is_empty());
        let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
        let after = d.backend.batch_stats();
        assert_eq!(after.draws - before.draws, 3);
        assert_eq!(after.passes - before.passes, 1, "one pending pass");
        let render_passes = if merge.is_none() { 2 } else { 3 };
        assert_eq!(
            after.render_passes - before.render_passes,
            render_passes,
            "merge {merge:?}"
        );
        // B, G, R, A: the first quad keeps the cleared alpha.
        assert_eq!(pixel(&px, 4, 4), [0, 0, 0xFF, 0xFF]);
        assert_eq!(pixel(&px, 12, 4), [0, 0xFF, 0, 0xFF]);
        assert_eq!(pixel(&px, 20, 4), [0xFF, 0, 0, 0xFF]);
        pixels.push(px);
    }
    assert!(pixels[0] == pixels[1], "merged and per-draw pixels differ");
}

/// The loose rule (`set_merge_masks`, the default) keeps the draw after a
/// partly masked one in the same render pass.
#[test]
fn the_loose_merge_rule_does_not_split_after_a_masked_draw() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    tfactor_stage(&mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    quads(&mut m, &[(0.0, 0.0, 8.0, 8.0), (8.0, 0.0, 16.0, 8.0)]);
    bind_quad(&mut d, &mut m);
    let l = d3d::GuestLayout::HALO2;
    let before = d.backend.batch_stats();
    m.write_u32(l.render_state_slot(67), 0x0001_0101).unwrap(); // COLORWRITEENABLE = RGB
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0x00FF_0000)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    m.write_u32(l.render_state_slot(67), 0x0101_0101).unwrap(); // ALL
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
    let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    let after = d.backend.batch_stats();
    assert_eq!(after.render_passes - before.render_passes, 1);
    assert_eq!(pixel(&px, 4, 4), [0, 0, 0xFF, 0xFF]);
    assert_eq!(pixel(&px, 12, 4), [0, 0xFF, 0, 0xFF]);
}

/// Three provider draws, the middle one alpha-tested (its program can
/// `discard`), the first writing `first_mask` (`COLORWRITEENABLE`) and the
/// others all channels, or `first_mask` too with `same_mask`, with merged
/// passes under the exact rule (`merge` true) or one render pass per draw
/// (`merge` false). Returns the back buffer and the render passes the
/// draws took.
fn discard_after(merge: bool, first_mask: u32, same_mask: bool) -> Option<(Vec<u8>, u64)> {
    let (mut d, mut m) = gpu()?;
    d.backend.set_merge_passes(merge);
    d.backend.set_merge_masks(false);
    set_states(&mut d, &mut m);
    tfactor_stage(&mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    quads(
        &mut m,
        &[
            (0.0, 0.0, 8.0, 8.0),
            (8.0, 0.0, 16.0, 8.0),
            (16.0, 0.0, 24.0, 8.0),
        ],
    );
    bind_quad(&mut d, &mut m);
    let l = d3d::GuestLayout::HALO2;
    let before = d.backend.batch_stats();
    m.write_u32(l.render_state_slot(67), first_mask).unwrap(); // COLORWRITEENABLE
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFFFF_0000)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    if !same_mask {
        m.write_u32(l.render_state_slot(67), 0x0101_0101).unwrap(); // ALL
    }
    m.write_u32(l.render_state_slot(60), 1).unwrap(); // ALPHATESTENABLE
    m.write_u32(l.render_state_slot(58), 0x204).unwrap(); // ALPHAFUNC = GREATER
    m.write_u32(l.render_state_slot(61), 0x80).unwrap(); // ALPHAREF
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_FF00)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
    m.write_u32(l.render_state_slot(60), 0).unwrap();
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF00_00FF)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 8, 4).unwrap();
    assert!(d.take_events().is_empty());
    let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    let after = d.backend.batch_stats();
    assert_eq!(after.draws - before.draws, 3);
    assert_eq!(after.passes - before.passes, 1, "one pending pass");
    Some((px, after.render_passes - before.render_passes))
}

/// The exact merge rule gives a draw that can `discard` a render pass of
/// its own, after fully written draws and after a draw writing only some
/// colour channels; the draw after it joins it unless the masks split.
/// Merged and per-draw pixels match.
#[test]
fn the_exact_merge_rule_splits_at_a_discarding_draw() {
    let all = 0x0101_0101;
    let rgb = 0x0001_0101;
    let Some((reference, passes)) = discard_after(false, all, false) else {
        return;
    };
    assert_eq!(passes, 3, "one render pass per draw");
    let (px, passes) = discard_after(true, all, false).unwrap();
    assert_eq!(passes, 2, "exact, after fully written draws");
    assert!(px == reference, "exact and per-draw pixels differ");
    // B, G, R, A.
    assert_eq!(pixel(&px, 4, 4), [0, 0, 0xFF, 0xFF]);
    assert_eq!(pixel(&px, 12, 4), [0, 0xFF, 0, 0xFF]);
    assert_eq!(pixel(&px, 20, 4), [0xFF, 0, 0, 0xFF]);
    // After an RGB-only draw with the same mask: the discarding draw starts
    // a render pass and the draw after it (RGB, no discard) joins it.
    let (reference, passes) = discard_after(false, rgb, true).unwrap();
    assert_eq!(passes, 3);
    let (px, passes) = discard_after(true, rgb, true).unwrap();
    assert_eq!(passes, 2, "exact, after a masked draw");
    assert!(px == reference, "exact and per-draw pixels differ");
    assert_eq!(pixel(&px, 12, 4), [0, 0xFF, 0, 0xFF]);
}

/// `XYZRHW | DIFFUSE | TEX1` quad over the 16x4 texture at `TEX_HEADER`,
/// bound to stage 0 (default stage: texture times white diffuse).
fn textured_quad(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [16.0 * x, 16.0 * y, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for f in [16.0 * x, 4.0 * y] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(
        m,
        gfx::vertex::fvf::XYZRHW | gfx::vertex::fvf::DIFFUSE | 0x100,
    )
    .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.set_texture(m, 0, TEX_HEADER).unwrap();
}

/// The texture's colour at (8, 8) of a fresh frame (B, G, R, A).
fn textured_pixel(d: &mut Device<GfxBackend>, m: &mut SparseMemory) -> [u8; 4] {
    d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    let (px, _) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    pixel(&px, 8, 8)
}

/// A texture group dropped after frames unused is rebuilt when the texture
/// is sampled again, and rewriting the texture's guest bytes replaces its
/// view: the draws see the old texels, then the new ones.
#[test]
fn texture_groups_follow_eviction_and_replacement() {
    let Some((mut d, mut m)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    texture(&mut m, 0x40);
    textured_quad(&mut d, &mut m);
    assert_eq!(textured_pixel(&mut d, &mut m), [0x40; 4]);
    // More frames than a group may go unused.
    for _ in 0..6 {
        d.swap(&mut m, 0).unwrap();
    }
    assert_eq!(textured_pixel(&mut d, &mut m), [0x40; 4], "after eviction");
    m.write(0x8000_0000 | TEX_DATA, &[0x90; 256]).unwrap();
    d.note_write(0x8000_0000 | TEX_DATA, 256);
    assert_eq!(textured_pixel(&mut d, &mut m), [0x90; 4], "replaced");
    d.swap(&mut m, 0).unwrap();
    assert_eq!(textured_pixel(&mut d, &mut m), [0x90; 4]);
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
    );
}
