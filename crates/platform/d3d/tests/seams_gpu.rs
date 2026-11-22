//! Seams between triangles at a render scale (docs/graphics/high-resolution.md,
//! "Subpixel snapping and cracks"): a mesh with T-junctions, and two draws
//! whose shared vertex differs by one ulp, through `ScaledBackend` with a
//! real vertex program (`mov oPos, v0`: positions are guest window
//! coordinates). Scaled, no clear-colour pixel may show inside the mesh. At
//! 1x the guest's 1/16-pixel truncation still applies: the image is the one
//! the mesh gives with its positions truncated on the CPU first. Synthetic
//! geometry; skips without a GPU adapter.

use std::sync::{Arc, Mutex};

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
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
use shaders::vsh::{self, MacOp, OutReg};

/// The guest's frame: 64x64.
const N: u32 = 64;
/// The mesh covers `[LO, HI]` in both axes.
const LO: f32 = 8.0;
const HI: f32 = 56.0;
const CLEAR: [u8; 3] = [0, 0, 255];
const WHITE: u32 = 0xFFFF_FFFF;

type V2 = [f32; 2];

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

/// `mov oPos, v0`, `mov oD0, v3`.
fn program_words() -> Vec<u32> {
    let insts = [
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ];
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

/// A white triangle list in window coordinates.
fn draw(tris: &[[V2; 3]]) -> ResolvedDraw {
    let mut data = Vec::new();
    for v in tris.iter().flatten() {
        for f in [v[0], v[1], 0.0, 1.0] {
            data.extend_from_slice(&f.to_le_bytes());
        }
        data.extend_from_slice(&WHITE.to_le_bytes());
    }
    let mut words = StateWords::defaults(false);
    words.render[147] = 0; // CULLMODE = NONE
    let w = copy_v0();
    words.render[..57].copy_from_slice(&w[..57]);
    words.render[136] = w[54];
    ResolvedDraw {
        entry: "seams_gpu",
        primitive: PrimitiveType::TriangleList,
        vertex_count: 3 * tris.len() as u32,
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
            width: N,
            height: N,
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
            constants: Arc::new([[0.0f32; 4]; CONSTANTS]),
            final_combiner_latch: w[8] | w[9] != 0,
            affine_projection: false,
            pixel_shader: Some(Arc::new(w)),
            combiner_state: Arc::new(words.render[..57].try_into().unwrap()),
        },
        raw: Arc::new(words.clone()),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: N,
            height: N,
            min_z: 0.0,
            max_z: 1.0,
        },
        textures: [None; 4],
        retail_constants: None,
        ui_fit: None,
        glyph_cache: 0,
        clip: None,
        clip_rects: Vec::new(),
        hw: d3d::hw::HwState::default(),
    }
}

/// The square `[LO, HI]²` cut by an edge from `(LO, 24)` with slope
/// `SLOPE`. Below it one triangle pair uses the edge whole; above it a
/// strip of 48 quads meets it at 47 T-junction vertices, `(LO + k, 24 + k
/// SLOPE)`, whose y truncates up (towards the strip) by up to 1/16 of a
/// pixel on the guest's grid, spread evenly over that range. One draw each
/// side, as a BSP's sections are drawn.
fn t_junctions() -> Vec<Vec<[V2; 3]>> {
    const SLOPE: f32 = 0.3711;
    let on = |k: u32| [LO + k as f32, 24.0 + k as f32 * SLOPE];
    let (a, b) = (on(0), on(48));
    let below = vec![[a, b, [HI, HI]], [a, [HI, HI], [LO, HI]]];
    let top = |k: u32| [LO + k as f32, LO];
    let mut above = Vec::new();
    for k in 0..48 {
        above.push([top(k), top(k + 1), on(k)]);
        above.push([on(k), top(k + 1), on(k + 1)]);
    }
    vec![below, above]
}

/// The square split along `(LO, 20)`..`(HI, 44)` into two draws that
/// each hold one end of the shared edge one ulp below the other's copy (a
/// vertex two programs compute a few ulps apart). On the guest grid each
/// such end truncates a whole 1/16 up: the upper draw's left end leaves a
/// sliver that narrows to the right.
fn ulp_seam() -> Vec<Vec<[V2; 3]>> {
    let (a, b) = ([LO, 20.0], [HI, 44.0]);
    let b_low = [HI, f32::from_bits(44.0f32.to_bits() - 1)];
    let a_up = [LO, f32::from_bits(20.0f32.to_bits() - 1)];
    let above = vec![[[LO, LO], [HI, LO], a_up], [a_up, [HI, LO], b]];
    let below = vec![[a, b_low, [HI, HI]], [a, [HI, HI], [LO, HI]]];
    vec![below, above]
}

fn truncated(draws: &[Vec<[V2; 3]>]) -> Vec<Vec<[V2; 3]>> {
    draws
        .iter()
        .map(|d| {
            d.iter()
                .map(|t| t.map(|v| v.map(|c| (c * 16.0).trunc() / 16.0)))
                .collect()
        })
        .collect()
}

type Shown = Arc<Mutex<Vec<((u32, u32), Vec<u8>)>>>;

/// The frame the window is handed: the draws over a blue clear, through
/// `ScaledBackend` at `scale`, as RGBA.
fn render(ctx: &GfxContext, scale: u32, draws: &[Vec<[V2; 3]>]) -> (u32, Vec<u8>) {
    let shown: Shown = Arc::default();
    let sink = Arc::clone(&shown);
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
    let mut b = ScaledBackend::new(
        GfxBackend::new(share(ctx)),
        GfxBackend::new(share(ctx)).on_present(hook),
        scale,
    )
    .unwrap();
    let color = b.create_surface(D3DFormat::A8R8G8B8, N, N, false).unwrap();
    b.clear(
        color,
        None,
        Clear {
            color: Some([0.0, 0.0, 1.0, 1.0]),
            depth: None,
            stencil: None,
        },
    )
    .unwrap();
    for tris in draws {
        assert_eq!(
            b.draw(color, None, &draw(tris), [None; 4]),
            Ok(d3d::DrawOutcome::Drawn)
        );
    }
    b.present(color, 0).unwrap();
    let ((w, h), px) = shown.lock().unwrap().pop().unwrap();
    assert_eq!((w, h), (N * scale, N * scale), "{scale}x frame");
    (w, px)
}

/// Clear-colour pixels inside the mesh, one guest pixel in from its edge.
fn holes(width: u32, px: &[u8], scale: u32) -> usize {
    let (lo, hi) = ((LO as u32 + 1) * scale, (HI as u32 - 1) * scale);
    let mut n = 0;
    for y in lo..hi {
        for x in lo..hi {
            let i = 4 * (y * width + x) as usize;
            n += usize::from(px[i..i + 3] == CLEAR);
        }
    }
    n
}

#[test]
fn seams_stay_closed_at_render_scale() {
    let Some(ctx) = context() else {
        return;
    };
    let mut open = Vec::new();
    for (name, mesh) in [("T-junctions", t_junctions()), ("ulp seam", ulp_seam())] {
        for scale in [2u32, 3, 4, 6] {
            let (w, px) = render(&ctx, scale, &mesh);
            let n = holes(w, &px, scale);
            eprintln!("{name} at {scale}x: {n} clear pixels inside the mesh");
            if n != 0 {
                open.push(format!("{name} at {scale}x: {n}"));
            }
        }
    }
    assert!(open.is_empty(), "clear pixels inside the mesh: {open:?}");
}

#[test]
fn at_1x_positions_still_truncate_to_a_sixteenth() {
    let Some(ctx) = context() else {
        return;
    };
    for (name, mesh) in [("T-junctions", t_junctions()), ("ulp seam", ulp_seam())] {
        let (_, px) = render(&ctx, 1, &mesh);
        let (_, pre) = render(&ctx, 1, &truncated(&mesh));
        assert!(px == pre, "{name}: 1x differs from the CPU-truncated mesh");
        eprintln!(
            "{name} at 1x: {} clear pixels inside the mesh",
            holes(N, &px, 1)
        );
    }
}
