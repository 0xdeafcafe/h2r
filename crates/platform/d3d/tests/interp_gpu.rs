//! The interpolated replay (Design D) and rotation-only reprojection (E1)
//! on the GPU, through `ScaledBackend` at render scales 2 and 3, as the
//! window runs them: every frame the present hook sees, made or real, must
//! be the size of the guest's own frame at that scale, and a frame made
//! with the camera standing still must show the guest's picture (D: the
//! same pixels; E1: the overscanned world image zoomed back, the same
//! within filtering). Synthetic draws through a real vertex program (a
//! camera matrix at c0 and the viewport registers at c-38/c-37, as the
//! XDK's programs end). Skips without a GPU adapter. (The render scale is
//! a whole number: there is no fractional scale to test.)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
use d3d::interp::camera::build;
use d3d::interp::math::Mat4;
use d3d::interp::model::{Layout, write};
use d3d::interp::{InterpBackend, Mode};
use d3d::scaled::ScaledBackend;
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, GfxBackend, ResolvedDraw};
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat, GfxContext};
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, IluOp, MacOp, OutReg};

const W: u32 = 640;
const H: u32 = 480;
const F: u64 = 1_000_000_000 / 30;
/// c0 in hardware numbering.
const CAM: usize = 96;

fn context() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

fn share(ctx: &GfxContext) -> GfxContext {
    GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

/// `oPos = c0..c3 * v0`, then the XDK's epilogue: `mul oPos.xyz, r12,
/// c-38 : rcc r1.w, r12.w` and `mad oPos.xyz, r12, r1.w, c-37`;
/// `oD0 = v3`.
fn program_words() -> Vec<u32> {
    let mut insts = Vec::new();
    for (i, mask) in [8u8, 4, 2, 1].into_iter().enumerate() {
        insts.push(
            Inst::mac(
                MacOp::Dp4,
                Some(Operand::v(0)),
                Some(Operand::c(CAM as u8 + i as u8)),
                None,
            )
            .to_out(false, OutReg::Pos, mask),
        );
    }
    insts.push(
        Inst::mac(MacOp::Mul, Some(Operand::r(12)), Some(Operand::c(58)), None)
            .to_out(false, OutReg::Pos, 0xE)
            .with_ilu(IluOp::Rcc, Operand::r(12).swz("w"))
            .ilu_to_temp(1, 1),
    );
    insts.push(
        Inst::mac(
            MacOp::Mad,
            Some(Operand::r(12)),
            Some(Operand::r(1).swz("w")),
            Some(Operand::c(59)),
        )
        .to_out(false, OutReg::Pos, 0xE),
    );
    insts.push(
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    );
    let code = program(&insts);
    let p = vsh::Program::parse(&code).unwrap();
    p.instructions.iter().flat_map(|i| i.words).collect()
}

fn copy_v0() -> [u32; 60] {
    let mut d = PixelShaderDef {
        combiner_count: 1 | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        ..PixelShaderDef::default()
    };
    d.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    d.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    d.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0);
    d.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    let b = d.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

/// A world-space quad (two triangles) at height `z`, `x, y` in `±half`,
/// its corners coloured `colors` (ARGB).
fn quad(cam: &Mat4, z: f32, half: f32, colors: [u32; 4]) -> ResolvedDraw {
    let corners = [(-half, -half), (half, -half), (-half, half), (half, half)];
    let mut data = Vec::new();
    for i in [0usize, 1, 2, 2, 1, 3] {
        let (x, y) = corners[i];
        for f in [x, y, z, 1.0] {
            data.extend_from_slice(&f.to_le_bytes());
        }
        data.extend_from_slice(&colors[i].to_le_bytes());
    }
    let mut words = StateWords::defaults(false);
    words.render[147] = 0; // CULLMODE = NONE
    let w = copy_v0();
    words.render[..57].copy_from_slice(&w[..57]);
    words.render[136] = w[54];
    let mut c = [[0.0f32; 4]; CONSTANTS];
    write(&mut c, CAM, Layout::Rows, 4, cam);
    let (scale, offset) = (
        [W as f32 / 2.0, -(H as f32) / 2.0],
        [W as f32 / 2.0, H as f32 / 2.0],
    );
    c[58] = [scale[0], scale[1], 0.0, 0.0];
    c[59] = [offset[0], offset[1], 0.5, 0.0];
    let hw = d3d::hw::HwState {
        viewport_scale: c[58],
        viewport_offset: c[59],
        ..Default::default()
    };
    ResolvedDraw {
        entry: "interp_gpu",
        primitive: PrimitiveType::TriangleList,
        vertex_count: 6,
        streams: vec![StreamBytes {
            data: data.into(),
            stride: 20,
        }],
        layout: VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes: vec![
                    VertexAttribute {
                        register: VertexRegister(0),
                        data_type: VertexDataType::Float4,
                        offset: 0,
                    },
                    VertexAttribute {
                        register: VertexRegister(3),
                        data_type: VertexDataType::D3dColor,
                        offset: 16,
                    },
                ],
                stride: 20,
            }],
            pretransformed: false,
        },
        indices: None,
        state: words.translate(&RenderStateTable::default()),
        extras: words.stage_extras(),
        target: TargetInfo {
            width: W,
            height: H,
            format: D3DFormat::A8R8G8B8,
            depth_format: None,
        },
        texture_info: [None; 4],
        shader: ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode: program_words().into(),
            },
            provided_inputs: 1 | 1 << 3,
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: Arc::new(c),
            final_combiner_latch: w[8] | w[9] != 0,
            affine_projection: false,
            pixel_shader: Some(Arc::new(w)),
            combiner_state: Arc::new(words.render[..57].try_into().unwrap()),
        },
        raw: Arc::new(words.clone()),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: W,
            height: H,
            min_z: 0.0,
            max_z: 1.0,
        },
        textures: [None; 4],
        glyph_cache: 0,
        clip: None,
        clip_rects: Vec::new(),
        hw,
    }
}

/// One frame: a large gradient floor and a smaller quad over it, seen
/// from above.
fn frame<B: Backend>(b: &mut B, color: B::Surface) {
    let cam = build::view_projection(1.2, [0.5, -0.3, 0.0], 0.0, 0.0);
    b.clear(
        color,
        None,
        Clear {
            color: Some([0.1, 0.2, 0.3, 1.0]),
            depth: None,
            stencil: None,
        },
    )
    .unwrap();
    let floor = quad(
        &cam,
        -10.0,
        40.0,
        [0xFF20_4080, 0xFFE0_6020, 0xFF40_C060, 0xFFF0_F0A0],
    );
    let panel = quad(
        &cam,
        -9.0,
        2.5,
        [0xFF80_1010, 0xFF10_8010, 0xFF10_1080, 0xFFC0_C0C0],
    );
    for d in [floor, panel] {
        assert_eq!(
            b.draw(color, None, &d, [None; 4]),
            Ok(d3d::DrawOutcome::Drawn)
        );
    }
    b.present(color, 0).unwrap();
}

/// What the window was handed: size and RGBA pixels per present.
type Shown = Arc<Mutex<Vec<((u32, u32), Vec<u8>)>>>;

fn scaled(ctx: &GfxContext, scale: u32, shown: &Shown) -> ScaledBackend<GfxBackend, GfxBackend> {
    let sink = Arc::clone(shown);
    let hook = move |ctx: &GfxContext,
                     store: &gfx::surface::SurfaceStore,
                     id,
                     _flags,
                     _gamma: Option<&[u8; 768]>| {
        let (mut px, order) = store.read_color(ctx, id).unwrap();
        let d = *store.get(id).unwrap().desc();
        if matches!(order, gfx::surface::HostPixelOrder::Bgra8) {
            for p in px.chunks_mut(4) {
                p.swap(0, 2);
            }
        }
        sink.lock().unwrap().push(((d.width, d.height), px));
    };
    ScaledBackend::new(
        GfxBackend::new(share(ctx)),
        GfxBackend::new(share(ctx)).on_present(hook),
        scale,
    )
    .unwrap()
}

/// Pixels whose channels all lie within `tol` of the other image's, as a
/// fraction, and the largest difference.
fn agreement(a: &[u8], b: &[u8], tol: u8) -> (f64, u8) {
    let (mut ok, mut worst) = (0usize, 0u8);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..3).map(|i| p[i].abs_diff(q[i])).max().unwrap();
        worst = worst.max(d);
        ok += usize::from(d <= tol);
    }
    (ok as f64 / (a.len() / 4) as f64, worst)
}

#[test]
fn fixed_capture_records_four_presents_even_if_readback_misses_wall_clock_slots() {
    let Some(ctx) = context() else {
        return;
    };
    let shown: Shown = Arc::default();
    let clock = Arc::new(AtomicU64::new(0));
    let now = Arc::clone(&clock);
    let mut presents = 0;
    let mut b = InterpBackend::new(scaled(&ctx, 2, &shown), || 120)
        .with_clock(move || now.load(Ordering::Relaxed))
        .with_capture_slots(move || {
            presents += 1;
            if presents == 3 { 4 } else { 0 }
        });
    let color = b.create_surface(D3DFormat::A8R8G8B8, W, H, false).unwrap();
    b.present(color, 0).unwrap();
    clock.store(F, Ordering::Relaxed);
    frame(&mut b, color);
    clock.store(F + 500_000_000, Ordering::Relaxed);
    frame(&mut b, color);
    assert_eq!(b.idle_at(F + 600_000_000), None);
    let stats = *b.stats().lock().unwrap();
    assert_eq!((stats.blended_frames, stats.replays, stats.dropped), (1, 3, 0));
    assert_eq!(shown.lock().unwrap().len(), 6);
}

#[test]
fn made_frames_match_the_guests_at_render_scale() {
    let Some(ctx) = context() else {
        return;
    };
    for scale in [2u32, 3] {
        // The guest's frame through the scaled backend alone.
        let shown: Shown = Arc::default();
        let mut plain = scaled(&ctx, scale, &shown);
        let color = plain
            .create_surface(D3DFormat::A8R8G8B8, W, H, false)
            .unwrap();
        frame(&mut plain, color);
        let real = shown.lock().unwrap().pop().unwrap();
        assert_eq!(real.0, (W * scale, H * scale), "the view's frame");

        for mode in [Mode::Replay, Mode::Reproject { ahead: false }] {
            let shown: Shown = Arc::default();
            let clock = Arc::new(AtomicU64::new(0));
            let c = Arc::clone(&clock);
            let mut b = InterpBackend::new(scaled(&ctx, scale, &shown), || 60)
                .with_clock(move || c.load(Ordering::Relaxed))
                .with_mode(mode);
            let color = b.create_surface(D3DFormat::A8R8G8B8, W, H, false).unwrap();
            // The swap chain is learnt from the first present.
            b.present(color, 0).unwrap();
            for i in 1..=2 {
                clock.store(i * F, Ordering::Relaxed);
                frame(&mut b, color);
            }
            clock.store(2 * F + F / 2, Ordering::Relaxed);
            b.idle_at(2 * F + F / 2);
            let st = *b.stats().lock().unwrap();
            assert_eq!(
                (st.blended_frames, st.replays, st.size_mismatch, st.unsure),
                (1, 1, 0, 0),
                "{scale}x {mode:?}: {st}"
            );
            let shown = shown.lock().unwrap();
            // The empty first present, the guest's first frame as drawn,
            // the made frame at its present, the one in between.
            assert_eq!(shown.len(), 4, "{scale}x {mode:?}");
            for (i, (size, px)) in shown[1..].iter().enumerate() {
                assert_eq!(*size, real.0, "{scale}x {mode:?}: frame {i}'s size");
                let (share, worst) = agreement(px, &real.1, 12);
                eprintln!(
                    "{scale}x {mode:?} frame {i}: {:.4} of pixels within 12, worst {worst}",
                    share
                );
                match (i, mode) {
                    // Drawn by the guest, or replayed with the same matrices.
                    (0, _) | (_, Mode::Replay) => assert_eq!(worst, 0, "{scale}x {mode:?}"),
                    // The world image drawn 1/1.1 smaller and zoomed back:
                    // the same picture up to filtering along edges.
                    _ => assert!(share > 0.98, "{scale}x {mode:?} frame {i}: {share}"),
                }
            }
        }
    }
}

/// E1 on one plain `GfxBackend`, as a headless run (`H2_BOOT_GFX`) makes
/// it: no window and no display clock, the in-between frames paced on the
/// backend's own clock and handed to the replay hook
/// (`GfxBackend::on_present_replay`), never to the present hook, so the
/// guest's swap count is the guest's. The in-between frame is built (the
/// world image warped, the camera still: the guest's picture within
/// filtering) and a gap is counted under its reason.
#[test]
fn reprojection_without_a_display_hands_in_between_frames_to_the_replay_hook() {
    let Some(ctx) = context() else {
        return;
    };
    fn read(
        ctx: &GfxContext,
        store: &gfx::surface::SurfaceStore,
        id: gfx::surface::SurfaceId,
    ) -> Vec<u8> {
        let (mut px, order) = store.read_color(ctx, id).unwrap();
        if matches!(order, gfx::surface::HostPixelOrder::Bgra8) {
            for p in px.chunks_mut(4) {
                p.swap(0, 2);
            }
        }
        px
    }
    let guest: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let between: Arc<Mutex<Vec<(u32, Vec<u8>)>>> = Arc::default();
    let (g, bt) = (Arc::clone(&guest), Arc::clone(&between));
    let backend = GfxBackend::new(share(&ctx))
        .on_present(move |ctx, store, id, _flags, _gamma| {
            g.lock().unwrap().push(read(ctx, store, id))
        })
        .on_present_replay(move |ctx, store, id, flags, _gamma| {
            bt.lock().unwrap().push((flags, read(ctx, store, id)))
        });
    let clock = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&clock);
    let mut b = InterpBackend::new(backend, || 60)
        .with_clock(move || c.load(Ordering::Relaxed))
        .with_mode(Mode::Reproject { ahead: false });
    let color = b.create_surface(D3DFormat::A8R8G8B8, W, H, false).unwrap();
    b.present(color, 0).unwrap();
    for i in 1..=2 {
        clock.store(i * F, Ordering::Relaxed);
        frame(&mut b, color);
    }
    // The guest's first frame, shown as drawn.
    let real = guest.lock().unwrap()[1].clone();
    // Half a guest frame on: one in-between frame is due.
    clock.store(2 * F + F / 2, Ordering::Relaxed);
    b.idle_at(2 * F + F / 2);
    let st = *b.stats().lock().unwrap();
    assert_eq!((st.blended_frames, st.replays), (1, 1), "{st}");
    assert_eq!(
        st.warps, 2,
        "the present's and the in-between frame's: {st}"
    );
    assert_eq!(st.size_mismatch + st.unsure, 0, "{st}");
    {
        let guest = guest.lock().unwrap();
        let between = between.lock().unwrap();
        assert_eq!(guest.len() as u64, st.frames, "only the guest's presents");
        assert_eq!(between.len(), 1, "the in-between frame, to the replay hook");
        assert_eq!(between[0].0, 0, "no swap flags");
        let (share, worst) = agreement(&between[0].1, &real, 12);
        eprintln!("headless E1 in between: {share:.4} of pixels within 12, worst {worst}");
        assert!(share > 0.98, "{share}");
    }
    // A hitch (more than 45 ms): the next frame is held, counted as a gap,
    // and nothing is made after it until a pair forms again.
    let gaps = st.unpaired_gap;
    clock.store(2 * F + 100_000_000, Ordering::Relaxed);
    frame(&mut b, color);
    assert_eq!(b.idle_at(2 * F + 110_000_000), None);
    let st = *b.stats().lock().unwrap();
    assert_eq!(st.unpaired_gap, gaps + 1, "{st}");
    assert_eq!(st.replays, 1, "{st}");
    assert_eq!(between.lock().unwrap().len(), 1);
    assert_eq!(guest.lock().unwrap().len() as u64, st.frames);
}
