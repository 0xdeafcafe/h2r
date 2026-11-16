//! Vertex fetch on the GPU against the CPU repacker: every `D3DVSDT_*`
//! type the NV2A has, drawn through `GfxBackend` and `Nv2aShaders` with
//! `FetchMode::Gpu` (guest bytes bound raw, decoded by the vertex shader)
//! and `FetchMode::Cpu` (converted to floats on the render thread), must
//! give the same pixels. Each type is drawn twice: in a vertex laid out so
//! the stream binds raw, and in a tight, unaligned one that GPU mode
//! realigns (guest bytes copied unconverted into 4-aligned slots).
//!
//! The vertex program maps the attribute into colour range
//! (`oD0 = v3 * c0 + c1`) and the combiner writes `v0`, so the target holds
//! the interpolated attribute. A constant-attribute draw is also checked
//! against values decoded by hand. Synthetic data only. Skips when no GPU
//! adapter is available.

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, GfxBackend, ResolvedDraw};
use gfx::renderer::Clear;
use gfx::repack::{FetchMode, RepackPlan};
use gfx::surface::{HostPixelOrder, SurfaceId};
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{D3DFormat, GfxContext};
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, MacOp, OutReg};

const SIZE: u32 = 16;

const ALL: [VertexDataType; 19] = {
    use VertexDataType::*;
    [
        Float1,
        Float2,
        Float3,
        Float4,
        D3dColor,
        Short1,
        Short2,
        Short3,
        Short4,
        NormShort1,
        NormShort2,
        NormShort3,
        NormShort4,
        NormPacked3,
        PByte1,
        PByte2,
        PByte3,
        PByte4,
        Float2H,
    ]
};

/// xorshift64*.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Scale taking the type's values into about `[-0.5, 0.5]` (the bias adds
/// 0.5).
fn scale(t: VertexDataType) -> f32 {
    use VertexDataType::*;
    match t {
        Float1 | Float2 | Float3 | Float4 | Float2H => 0.25,
        Short1 | Short2 | Short3 | Short4 => 0.5 / 32768.0,
        _ => 0.5,
    }
}

/// One attribute's guest bytes: random, with the extremes mixed in.
fn attribute_bytes(t: VertexDataType, rng: &mut Rng) -> Vec<u8> {
    use VertexDataType::*;
    let n = t.byte_size() as usize;
    match t {
        Float1 | Float2 | Float3 | Float4 | Float2H => (0..n / 4)
            .flat_map(|_| {
                let u = (rng.next() >> 40) as f32 / (1u64 << 24) as f32;
                (u * 4.0 - 2.0).to_le_bytes()
            })
            .collect(),
        Short1 | Short2 | Short3 | Short4 | NormShort1 | NormShort2 | NormShort3 | NormShort4 => (0
            ..n / 2)
            .flat_map(|_| {
                let v = match rng.next() % 6 {
                    0 => i16::MIN,
                    1 => i16::MAX,
                    2 => (rng.next() % 512) as i16 - 256,
                    _ => rng.next() as i16,
                };
                v.to_le_bytes()
            })
            .collect(),
        _ => (0..n).map(|_| rng.next() as u8).collect(),
    }
}

/// `oPos = v0`, `oD0 = v3 * c0 + c1` (one constant per instruction).
fn program_words() -> Vec<u32> {
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mul, Some(Operand::v(3)), Some(Operand::c(0)), None).to_temp(0, 0xF),
        Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(1)))
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    let p = vsh::Program::parse(&code).unwrap();
    p.instructions.iter().flat_map(|i| i.words).collect()
}

/// `r0 = v0`.
fn copy_v0() -> PixelShaderDef {
    let mut d = PixelShaderDef {
        combiner_count: 1 | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        ..PixelShaderDef::default()
    };
    d.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    d.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    d.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0);
    d.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    d
}

fn psdef_words(def: &PixelShaderDef) -> [u32; 60] {
    let b = def.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

/// How the attribute sits in the vertex.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// Position at 0, the attribute at 16, 16 bytes of room: binds raw.
    Roomy,
    /// Attribute first, position right after it, no padding: the stride is
    /// unaligned (or a widened read would leave the vertex) for most types.
    Tight,
}

struct Gpu {
    b: GfxBackend,
    color: SurfaceId,
}

fn gpu() -> Option<Gpu> {
    if std::env::var_os("SHADERS_NO_GPU").is_some() {
        return None;
    }
    let ctx = match GfxContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    let mut b = GfxBackend::new(ctx);
    let color = b
        .create_surface(D3DFormat::A8R8G8B8, SIZE, SIZE, false)
        .unwrap();
    Some(Gpu { b, color })
}

/// The layout for `t` placed as `p`.
fn layout(t: VertexDataType, p: Placement) -> VertexLayout {
    let size = t.byte_size();
    let (pos, attr, stride) = match p {
        Placement::Roomy => (0, 16, 32),
        Placement::Tight => (size, 0, size + 16),
    };
    VertexLayout {
        streams: vec![StreamLayout {
            stream: 0,
            attributes: vec![
                VertexAttribute {
                    register: VertexRegister(0),
                    data_type: VertexDataType::Float4,
                    offset: pos,
                },
                VertexAttribute {
                    register: VertexRegister(3),
                    data_type: t,
                    offset: attr,
                },
            ],
            stride,
        }],
        pretransformed: false,
    }
}

impl Gpu {
    /// Draws one triangle covering the target with the three attribute
    /// values `attrs` and returns the RGBA bytes of every pixel.
    fn draw(
        &mut self,
        mode: FetchMode,
        t: VertexDataType,
        p: Placement,
        attrs: &[Vec<u8>; 3],
    ) -> Vec<[u8; 4]> {
        let b = &mut self.b;
        b.set_vertex_fetch(mode);
        b.clear(
            self.color,
            None,
            Clear {
                color: Some([0.0; 4]),
                depth: None,
                stencil: None,
            },
        )
        .unwrap();
        let layout = layout(t, p);
        let s = &layout.streams[0];
        let (pos_off, attr_off) = (
            s.attributes[0].offset as usize,
            s.attributes[1].offset as usize,
        );
        let extent = SIZE as f32 * 3.0;
        let corners = [(0.0f32, 0.0f32), (extent, 0.0), (0.0, extent)];
        let mut data = Vec::new();
        for (v, (x, y)) in corners.iter().enumerate() {
            let mut vertex = vec![0u8; s.stride as usize];
            let pos: Vec<u8> = [*x, *y, 0.0, 1.0]
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();
            vertex[pos_off..pos_off + 16].copy_from_slice(&pos);
            vertex[attr_off..attr_off + attrs[v].len()].copy_from_slice(&attrs[v]);
            data.extend(vertex);
        }

        let mut words = StateWords::defaults(false);
        words.render[147] = 0; // CULLMODE = NONE
        let w = psdef_words(&copy_v0());
        words.render[..57].copy_from_slice(&w[..57]);
        words.render[136] = w[54];
        let mut constants = Box::new([[0.0f32; 4]; CONSTANTS]);
        constants[0] = [scale(t); 4];
        constants[1] = [0.5; 4];
        let draw = ResolvedDraw {
            entry: "vertex_fetch",
            primitive: PrimitiveType::TriangleList,
            vertex_count: 3,
            streams: vec![StreamBytes {
                data: data.into(),
                stride: s.stride,
            }],
            layout: layout.clone(),
            indices: None,
            state: words.translate(&RenderStateTable::default()),
            extras: words.stage_extras(),
            target: TargetInfo {
                width: SIZE,
                height: SIZE,
                format: D3DFormat::A8R8G8B8,
                depth_format: None,
            },
            texture_info: [None; 4],
            shader: ShaderInputs {
                vertex: VertexProcessing::Program {
                    start: 0,
                    microcode: program_words(),
                },
                provided_inputs: 1 | 1 << 3,
                vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
                constants,
                final_combiner_latch: w[8] | w[9] != 0,
                affine_projection: false,
                pixel_shader: Some(Box::new(w)),
                combiner_state: Box::new(words.render[..57].try_into().unwrap()),
            },
            raw: std::sync::Arc::new(words.clone()),
            viewport: Viewport {
                x: 0,
                y: 0,
                width: SIZE,
                height: SIZE,
                min_z: 0.0,
                max_z: 1.0,
            },
            textures: [None; 4],
            glyph_cache: 0,
            clip: None,
            clip_rects: Vec::new(),
            hw: d3d::hw::HwState::default(),
        };
        let scope = b.ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = b.draw(self.color, None, &draw, [None; 4]);
        b.flush().unwrap();
        let err = pollster::block_on(scope.pop());
        assert_eq!(r, Ok(DrawOutcome::Drawn), "{t:?} {p:?} {mode:?}");
        assert!(err.is_none(), "{t:?} {p:?} {mode:?}: {err:?}");
        let (px, order) = b.read_color(self.color).unwrap();
        px.chunks(4)
            .map(|c| match order {
                HostPixelOrder::Bgra8 => [c[2], c[1], c[0], c[3]],
                HostPixelOrder::Rgba8 => [c[0], c[1], c[2], c[3]],
            })
            .collect()
    }
}

/// Whether `t` placed as `p` binds raw in GPU mode.
fn binds_raw(t: VertexDataType, p: Placement) -> bool {
    let l = layout(t, p);
    let stride = l.streams[0].stride;
    RepackPlan::with_mode(&l, &[stride], FetchMode::Gpu).streams[0].identity
}

#[test]
fn gpu_fetch_matches_cpu_repack_for_every_type() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut realigned = Vec::new();
    for t in ALL {
        assert!(binds_raw(t, Placement::Roomy), "{t:?} should bind raw");
        for p in [Placement::Roomy, Placement::Tight] {
            if !binds_raw(t, p) {
                realigned.push((t, p));
            }
            for _ in 0..4 {
                let attrs = [0, 1, 2].map(|_| attribute_bytes(t, &mut rng));
                let cpu = g.draw(FetchMode::Cpu, t, p, &attrs);
                let gpu = g.draw(FetchMode::Gpu, t, p, &attrs);
                // Interpolation over the triangle: same inputs, so the same
                // pixels up to a rounding step.
                for (i, (c, n)) in cpu.iter().zip(&gpu).enumerate() {
                    assert!(
                        c.iter()
                            .zip(n)
                            .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1),
                        "{t:?} {p:?} pixel {i}: cpu {c:?} gpu {n:?} (attrs {attrs:?})"
                    );
                }
                // Not vacuous: something was drawn.
                assert!(
                    gpu.iter().any(|px| px[3] != 0),
                    "{t:?} {p:?}: nothing drawn"
                );
            }
        }
    }
    // The tight placement exercises realigning for the odd-sized types.
    assert!(
        realigned.iter().any(|(t, _)| *t == VertexDataType::PByte3),
        "{realigned:?}"
    );
}

/// Hand-decoded constant attributes, in GPU mode: `(type, bytes, value)`.
#[test]
fn gpu_fetch_decodes_known_values() {
    let Some(mut g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use VertexDataType::*;
    let shorts = |v: &[i16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let floats = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    // NORMPACKED3: x = +1 (0x3FF), y = -1 (0x401), z = 0x0FF (255/511).
    let packed: u32 = 0x3FF | (0x401 << 11) | (0x0FF << 22);
    let cases: Vec<(VertexDataType, Vec<u8>, [f32; 4])> = vec![
        (Short1, shorts(&[-16384]), [-16384.0, 0.0, 0.0, 1.0]),
        (
            Short2,
            shorts(&[16384, -8192]),
            [16384.0, -8192.0, 0.0, 1.0],
        ),
        (
            Short3,
            shorts(&[1000, -32768, 32767]),
            [1000.0, -32768.0, 32767.0, 1.0],
        ),
        (
            Short4,
            shorts(&[1, 2, 3, -32768]),
            [1.0, 2.0, 3.0, -32768.0],
        ),
        (NormShort1, shorts(&[-32768]), [-1.0, 0.0, 0.0, 1.0]),
        (
            NormShort3,
            shorts(&[16384, -16384, 32767]),
            [16384.0 / 32767.0, -16384.0 / 32767.0, 1.0, 1.0],
        ),
        (PByte1, vec![51], [0.2, 0.0, 0.0, 1.0]),
        (PByte3, vec![255, 0, 102], [1.0, 0.0, 0.4, 1.0]),
        (
            NormPacked3,
            packed.to_le_bytes().to_vec(),
            [1.0, -1.0, 255.0 / 511.0, 1.0],
        ),
        (Float2H, floats(&[1.5, -0.5, 2.0]), [1.5, -0.5, 0.0, 2.0]),
        (D3dColor, vec![0x33, 0x66, 0x99, 0xCC], [0.6, 0.4, 0.2, 0.8]),
    ];
    for (t, bytes, want) in cases {
        let attrs = [bytes.clone(), bytes.clone(), bytes];
        let px = g.draw(FetchMode::Gpu, t, Placement::Roomy, &attrs);
        let want = want.map(|v| ((v * scale(t) + 0.5).clamp(0.0, 1.0) * 255.0).round() as i32);
        let got = px[(4 * SIZE + 4) as usize];
        assert!(
            got.iter()
                .zip(want)
                .all(|(g, w)| (*g as i32 - w).abs() <= 1),
            "{t:?}: got {got:?}, want {want:?}"
        );
    }
}
