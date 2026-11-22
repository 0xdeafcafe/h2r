//! Render-thread cost of a draw-heavy frame with compressed vertex formats,
//! CPU repacking against GPU fetch: synthetic, retail-free.
//!
//! Each frame is `H2_BENCH_DRAWS` (default 1000) indexed vertex-program
//! draws through `GfxBackend` (what the render thread runs), over 300 to
//! 1,500 vertices each, cycling four shapes of compressed geometry
//! (`shapes`): a 4-aligned stream GPU mode binds raw, one stream per
//! attribute with a 6-byte `NORMSHORT3` position stream and a packed
//! stream with unaligned offsets (both realigned in GPU mode), and a
//! float-and-colour stream that binds raw in both modes.
//!
//! Timed per frame: the draws plus the flush that submits them (CPU time
//! on the calling thread); the GPU is then waited for outside the timing.
//! Triangles are a few pixels, so the GPU is never the limit.
//!
//! `cargo bench -p d3d --bench vertex_fetch` prints ms per frame for each
//! mode. Env: `H2_BENCH_FRAMES` (default 60), `H2_BENCH_DRAWS`,
//! `H2_BENCH_MODES` (`cpu` or `gpu` for one mode only, e.g. to profile).

use std::time::{Duration, Instant};

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, GfxBackend, ResolvedDraw};
use gfx::repack::FetchMode;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat, GfxContext};
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, MacOp, OutReg};

const W: u32 = 640;
const H: u32 = 480;

fn env(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

/// `oPos = v0`, `oD0 = v2`, `oD1 = v3`, `oT0 = v9`.
fn program_words() -> Vec<u32> {
    let mov = |v: u8, o: OutReg| {
        Inst::mac(MacOp::Mov, Some(Operand::v(v)), None, None).to_out(false, o, 0xF)
    };
    let code = program(&[
        mov(0, OutReg::Pos),
        mov(2, OutReg::D0),
        mov(3, OutReg::D1),
        mov(9, OutReg::T(0)).last(),
    ]);
    let p = vsh::Program::parse(&code).unwrap();
    p.instructions.iter().flat_map(|i| i.words).collect()
}

/// `r0 = v0 * v1`.
fn combiner() -> [u32; 60] {
    let mut d = PixelShaderDef {
        combiner_count: 1 | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        ..PixelShaderDef::default()
    };
    d.rgb_inputs[0] = inputs(REG_V0, REG_V1, 0, 0);
    d.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    d.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0);
    d.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    let b = d.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

/// One stream: `(register, type)` packed in vertex order, and the stride.
type Stream = (Vec<(u8, VertexDataType)>, u32);

/// The draw shapes a frame cycles through.
fn shapes() -> Vec<Vec<Stream>> {
    use VertexDataType::*;
    vec![
        // Environment-like: one 4-aligned stream (raw in GPU mode).
        vec![(
            vec![(0, Float3), (2, NormPacked3), (3, NormPacked3), (9, Short2)],
            24,
        )],
        // Object-like: one stream per attribute, compressed position with
        // a 6-byte stride (realigned in GPU mode).
        vec![
            (vec![(0, NormShort3)], 6),
            (vec![(9, NormShort2)], 4),
            (vec![(2, NormPacked3), (3, NormPacked3)], 8),
        ],
        // Packed, unaligned offsets (realigned in GPU mode).
        vec![(
            vec![(0, Float3), (2, NormShort3), (3, PByte3), (9, Float2)],
            29,
        )],
        // Floats and a colour: raw in both modes.
        vec![(vec![(0, Float3), (2, D3dColor), (9, Float2)], 24)],
    ]
}

fn layout(streams: &[Stream]) -> VertexLayout {
    VertexLayout {
        streams: streams
            .iter()
            .enumerate()
            .map(|(i, (attrs, stride))| {
                let mut offset = 0;
                StreamLayout {
                    stream: i as u32,
                    attributes: attrs
                        .iter()
                        .map(|&(r, t)| {
                            let a = VertexAttribute {
                                register: VertexRegister(r),
                                data_type: t,
                                offset,
                            };
                            offset += t.byte_size();
                            a
                        })
                        .collect(),
                    stride: *stride,
                }
            })
            .collect(),
        pretransformed: false,
    }
}

fn make_draw(rng: &mut Rng, shape: &[Stream]) -> ResolvedDraw {
    let layout = layout(shape);
    let count = 300 + rng.below(1200);
    let streams = layout
        .streams
        .iter()
        .map(|s| {
            let mut data = vec![0u8; (s.stride * count) as usize];
            for v in 0..count as usize {
                let vertex = &mut data[v * s.stride as usize..][..s.stride as usize];
                for b in vertex.iter_mut() {
                    *b = rng.next() as u8;
                }
                // Floats on screen (small triangles), not NaNs.
                for a in &s.attributes {
                    let n = match a.data_type {
                        VertexDataType::Float2 => 2,
                        VertexDataType::Float3 => 3,
                        _ => continue,
                    };
                    for c in 0..n {
                        let f = rng.below(H - 8) as f32;
                        let o = a.offset as usize + 4 * c;
                        vertex[o..o + 4].copy_from_slice(&f.to_le_bytes());
                    }
                }
            }
            StreamBytes {
                data: data.into(),
                stride: s.stride,
            }
        })
        .collect();
    let indices: Vec<u32> = (0..count / 3 * 3).collect();
    let mut words = StateWords::defaults(false);
    words.render[147] = 0;
    let w = combiner();
    words.render[..57].copy_from_slice(&w[..57]);
    words.render[136] = w[54];
    let provided = shape
        .iter()
        .flat_map(|s| &s.0)
        .fold(0u16, |m, (r, _)| m | 1 << r);
    ResolvedDraw {
        entry: "bench",
        primitive: PrimitiveType::TriangleList,
        vertex_count: count,
        streams,
        layout,
        indices: Some(indices.into()),
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
            provided_inputs: provided,
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: std::sync::Arc::new([[0.0; 4]; CONSTANTS]),
            final_combiner_latch: w[8] | w[9] != 0,
            affine_projection: false,
            pixel_shader: Some(std::sync::Arc::new(w)),
            combiner_state: std::sync::Arc::new(words.render[..57].try_into().unwrap()),
        },
        raw: std::sync::Arc::new(words.clone()),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: W,
            height: H,
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

fn main() {
    let ctx = match GfxContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("no GPU adapter ({e}); skipping");
            return;
        }
    };
    let frames = env("H2_BENCH_FRAMES", 60);
    let draws = env("H2_BENCH_DRAWS", 1000);
    let mut b = GfxBackend::new(ctx);
    let color = b.create_surface(D3DFormat::A8R8G8B8, W, H, false).unwrap();
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let shapes = shapes();
    let frame: Vec<ResolvedDraw> = (0..draws)
        .map(|i| make_draw(&mut rng, &shapes[i as usize % shapes.len()]))
        .collect();
    let bytes: usize = frame
        .iter()
        .flat_map(|d| &d.streams)
        .map(|s| s.data.len())
        .sum();
    println!(
        "{draws} draws/frame, {:.1} MB of vertices/frame, {frames} frames",
        bytes as f64 / 1e6
    );
    let modes: Vec<FetchMode> = match std::env::var("H2_BENCH_MODES").as_deref() {
        Ok("cpu") => vec![FetchMode::Cpu],
        Ok("gpu") => vec![FetchMode::Gpu],
        _ => vec![
            FetchMode::Cpu,
            FetchMode::Gpu,
            FetchMode::Cpu,
            FetchMode::Gpu,
        ],
    };
    for mode in modes {
        b.set_vertex_fetch(mode);
        let run = |b: &mut GfxBackend| -> Duration {
            b.clear(
                color,
                None,
                Clear {
                    color: Some([0.0; 4]),
                    depth: None,
                    stencil: None,
                },
            )
            .unwrap();
            let t = Instant::now();
            for d in &frame {
                assert_eq!(b.draw(color, None, d, [None; 4]), Ok(DrawOutcome::Drawn));
            }
            b.flush().unwrap();
            let e = t.elapsed();
            let _ = b.ctx.device.poll(wgpu::PollType::wait_indefinitely());
            e
        };
        // Warm-up: pipelines compile on workers; wait them out.
        for _ in 0..3 {
            run(&mut b);
        }
        let mut times: Vec<Duration> = (0..frames).map(|_| run(&mut b)).collect();
        times.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / times.len() as f64;
        println!(
            "{mode:?}: {:.2} ms/frame mean, {:.2} median, {:.2} p90",
            mean,
            ms(times[times.len() / 2]),
            ms(times[times.len() * 9 / 10]),
        );
    }
}
