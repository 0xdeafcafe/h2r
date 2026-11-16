//! NV2A pixel-pipeline conformance: small synthetic combiner programs,
//! fixed-function stages and pipeline states, each drawn through
//! `GfxBackend` and the default `Nv2aShaders` provider into an
//! `A8R8G8B8` target and checked on known pixel values.
//!
//! The expected values are worked by hand from the semantics in
//! `docs/shaders.md` (NVIDIA's `NV_register_combiners` model, the D3D
//! encodings, and what Halo 2's own D3D pushes, cited per test), never by
//! running the translator. Every draw covers the pixel read back with
//! constant vertex attributes, so interpolation plays no part. Everything
//! here is synthetic. Skips when no GPU adapter is available.

use d3d::backend::{StreamBytes, TargetInfo, TextureInfo, Viewport};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, GfxBackend, ResolvedDraw};
use gfx::renderer::Clear;
use gfx::surface::{HostPixelOrder, SurfaceId};
use gfx::texture::{DecodeOptions, TextureDesc, TextureKind, decode_texture};
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
/// The pixel every check reads (inside the triangle, away from its edges).
const AT: (u32, u32) = (4, 4);

// Guest render-state slots (canonical numbering, identical on 5849).
const RS_ALPHABLENDENABLE: u32 = 59;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RS_BLENDCOLOR: u32 = 75;
const RS_FOGENABLE: u32 = 92;
const RS_FOGTABLEMODE: u32 = 93;
const RS_FOGSTART: u32 = 94;
const RS_FOGEND: u32 = 95;
const RS_FOGDENSITY: u32 = 96;
const RS_RANGEFOGENABLE: u32 = 97;
const RS_SPECULARENABLE: u32 = 103;
const RS_FOGCOLOR: u32 = 138;
const RS_TWOSIDEDLIGHTING: u32 = 141;
const RS_FRONTFACE: u32 = 146;
const RS_CULLMODE: u32 = 147;
const RS_TEXTUREFACTOR: u32 = 148;

// `D3DTSS_*` (Xbox numbering).
const TSS_COLOROP: usize = 12;
const TSS_COLORARG1: usize = 14;
const TSS_COLORARG2: usize = 15;
const TSS_ALPHAOP: usize = 16;
const TSS_ALPHAARG1: usize = 18;
const TSS_COLORSIGN: usize = 10;
const TSS_MAGFILTER: usize = 3;
const TSS_MINFILTER: usize = 4;
const TEXF_LINEAR: u32 = 2;
const TSS_ADDRESSU: usize = 0;
const TSS_BORDERCOLOR: usize = 29;
const TADDRESS_BORDER: u32 = 4;
// Xbox `D3DTOP_*` and `D3DTA_*`.
const TOP_SELECTARG1: u32 = 2;
const TOP_MODULATE: u32 = 4;
const TA_DIFFUSE: u32 = 0;
const TA_TFACTOR: u32 = 3;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// One draw: a combiner program (or the fixed-function stages), vertex
/// attributes, render and stage-0 states, and an optional texture.
#[derive(Clone)]
struct Case {
    /// `None`: fixed-function texture stages (pre-transformed vertices).
    def: Option<PixelShaderDef>,
    d0: [f32; 4],
    d1: [f32; 4],
    b0: [f32; 4],
    t0: [f32; 4],
    fog: f32,
    render: Vec<(u32, u32)>,
    stage0: Vec<(usize, u32)>,
    texture: Option<Tex>,
    /// Target contents before the draw.
    clear: [f32; 4],
    /// Diffuse as a `D3DCOLOR` attribute (fixed-function only).
    d3dcolor_diffuse: Option<u32>,
    /// Wind the triangle counter-clockwise on screen.
    ccw: bool,
    /// Combiner-state slots written after the definition was bound (the
    /// render-state mirror of pushed combiner methods).
    pushed: Vec<(usize, u32)>,
    stipple: [u32; 32],
    varying_t0: bool,
    small_triangle: bool,
}

#[derive(Clone)]
struct Tex {
    format: D3DFormat,
    width: u32,
    height: u32,
    levels: u32,
    bytes: Vec<u8>,
    palette: Option<Vec<u32>>,
}

impl Tex {
    /// A swizzled texture of one 32-bit value repeated (`A8R8G8B8` and
    /// friends): every texel the same, so layout cannot matter.
    fn solid32(format: D3DFormat, v: u32) -> Self {
        Self {
            format,
            width: 2,
            height: 2,
            levels: 1,
            bytes: v.to_le_bytes().repeat(4),
            palette: None,
        }
    }
    fn solid16(format: D3DFormat, v: u16) -> Self {
        Self {
            format,
            width: 2,
            height: 2,
            levels: 1,
            bytes: v.to_le_bytes().repeat(4),
            palette: None,
        }
    }
    fn solid8(format: D3DFormat, v: u8) -> Self {
        Self {
            format,
            width: 2,
            height: 2,
            levels: 1,
            bytes: vec![v; 4],
            palette: None,
        }
    }
}

impl Default for Case {
    fn default() -> Self {
        Self {
            def: None,
            d0: [0.0; 4],
            d1: [0.0; 4],
            b0: [0.0; 4],
            t0: [0.5, 0.5, 0.0, 1.0],
            fog: 0.0,
            render: Vec::new(),
            stage0: Vec::new(),
            texture: None,
            clear: [0.0; 4],
            d3dcolor_diffuse: None,
            ccw: false,
            pushed: Vec::new(),
            stipple: [0; 32],
            varying_t0: false,
            small_triangle: false,
        }
    }
}

struct Gpu {
    b: GfxBackend,
    color: SurfaceId,
    size: u32,
}

fn gpu() -> Option<Gpu> {
    gpu_with_size(SIZE)
}

fn gpu_with_size(size: u32) -> Option<Gpu> {
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
        .create_surface(D3DFormat::A8R8G8B8, size, size, false)
        .unwrap();
    Some(Gpu { b, color, size })
}

/// Pass-through vertex program: `oPos = v0` (window coordinates),
/// `oD0 = v3`, `oD1 = v4`, `oB0 = v6`, `oT0 = v9`, `oFog.x = v5.x`.
fn passthrough() -> Vec<u32> {
    let mov = |v: u8, o: OutReg, mask: u8| {
        Inst::mac(MacOp::Mov, Some(Operand::v(v)), None, None).to_out(false, o, mask)
    };
    let code = program(&[
        mov(0, OutReg::Pos, 0xF),
        mov(3, OutReg::D0, 0xF),
        mov(4, OutReg::D1, 0xF),
        mov(6, OutReg::B0, 0xF),
        mov(9, OutReg::T(0), 0xF),
        mov(5, OutReg::Fog, 0x8).last(),
    ]);
    let p = vsh::Program::parse(&code).unwrap();
    p.instructions.iter().flat_map(|i| i.words).collect()
}

fn psdef_words(def: &PixelShaderDef) -> [u32; 60] {
    let b = def.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

impl Gpu {
    /// Draws `c` and returns the RGBA bytes at [`AT`].
    fn run(&mut self, c: &Case) -> [u8; 4] {
        self.run_at(c, AT)
    }

    fn run_at(&mut self, c: &Case, at: (u32, u32)) -> [u8; 4] {
        let size = self.size;
        let b = &mut self.b;
        b.clear(
            self.color,
            None,
            Clear {
                color: Some(c.clear),
                depth: None,
                stencil: None,
            },
        )
        .unwrap();

        let mut words = StateWords::defaults(false);
        words.render[147] = 0; // CULLMODE = NONE unless a case sets it
        for &(slot, v) in &c.render {
            words.render[slot as usize] = v;
        }
        for &(i, v) in &c.stage0 {
            words.stages[0][i] = v;
        }

        let extent = size as f32 * 3.0;
        let corners: [(f32, f32); 3] = if c.small_triangle {
            [(2.0, 2.0), (14.0, 2.0), (2.0, 14.0)]
        } else if c.ccw {
            [(0.0, 0.0), (0.0, extent), (extent, 0.0)]
        } else {
            [(0.0, 0.0), (extent, 0.0), (0.0, extent)]
        };
        let fixed = c.def.is_none();
        // Registers and types in stream order.
        let mut attrs: Vec<(u8, VertexDataType)> = vec![(0, VertexDataType::Float4)];
        match (fixed, c.d3dcolor_diffuse) {
            (true, Some(_)) => attrs.push((3, VertexDataType::D3dColor)),
            _ => attrs.push((3, VertexDataType::Float4)),
        }
        attrs.extend([
            (4, VertexDataType::Float4),
            (5, VertexDataType::Float4),
            (6, VertexDataType::Float4),
            (9, VertexDataType::Float4),
        ]);
        let mut offset = 0;
        let mut attributes = Vec::new();
        for &(r, t) in &attrs {
            attributes.push(VertexAttribute {
                register: VertexRegister(r),
                data_type: t,
                offset,
            });
            offset += if t == VertexDataType::D3dColor { 4 } else { 16 };
        }
        let stride = offset;
        let mut data = Vec::new();
        for (x, y) in corners {
            for &(r, t) in &attrs {
                if t == VertexDataType::D3dColor {
                    data.extend(c.d3dcolor_diffuse.unwrap().to_le_bytes());
                    continue;
                }
                let v = match r {
                    0 => [x, y, 0.0, 1.0],
                    3 => c.d0,
                    4 => c.d1,
                    5 => [c.fog, 0.0, 0.0, 1.0],
                    6 => c.b0,
                    _ if c.varying_t0 => [x / 48.0, y / 48.0, 0.0, 1.0],
                    _ => c.t0,
                };
                data.extend(v.iter().flat_map(|f| f.to_le_bytes()));
            }
        }
        let layout = VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes,
                stride,
            }],
            pretransformed: fixed,
        };

        // Texture.
        let (host, info) = match &c.texture {
            Some(t) => {
                let desc =
                    TextureDesc::new(t.format, TextureKind::D2, t.width, t.height, 1, t.levels);
                let decoded = decode_texture(
                    &desc,
                    &t.bytes,
                    t.palette.as_deref(),
                    // As the device does: COLORSIGN channels stored biased.
                    DecodeOptions {
                        bc_supported: b.bc_supported(),
                        ..Default::default()
                    }
                    .with_color_sign(words.stages[0][TSS_COLORSIGN]),
                )
                .unwrap();
                let host = b.upload_texture(&decoded).unwrap();
                let info = TextureInfo {
                    header: 0x1000,
                    palette: 0,
                    format: t.format,
                    kind: TextureKind::D2,
                    width: t.width,
                    height: t.height,
                    depth: 1,
                    levels: t.levels,
                };
                (Some(host), Some(info))
            }
            None => (None, None),
        };

        let (vertex, pixel_shader, combiner_state) = match &c.def {
            Some(def) => {
                let w = psdef_words(def);
                words.render[..57].copy_from_slice(&w[..57]);
                words.render[136] = w[54];
                for &(slot, v) in &c.pushed {
                    words.render[slot] = v;
                }
                (
                    VertexProcessing::Program {
                        start: 0,
                        microcode: passthrough(),
                    },
                    Some(Box::new(w)),
                    Box::new(words.render[..57].try_into().unwrap()),
                )
            }
            None => (
                VertexProcessing::FixedFunction {
                    fvf: 0x004 | 0x040 | 0x080 | 0x100,
                },
                None,
                Box::new(words.render[..57].try_into().unwrap()),
            ),
        };
        let provided = attrs.iter().fold(0u16, |m, (r, _)| m | 1 << r);
        let draw = ResolvedDraw {
            entry: "conformance",
            primitive: PrimitiveType::TriangleList,
            vertex_count: 3,
            layout,
            streams: vec![StreamBytes {
                data: data.into(),
                stride,
            }],
            indices: None,
            state: words.translate(&RenderStateTable::default()),
            extras: words.stage_extras(),
            target: TargetInfo {
                width: size,
                height: size,
                format: D3DFormat::A8R8G8B8,
                depth_format: None,
            },
            texture_info: [info, None, None, None],
            shader: ShaderInputs {
                vertex,
                provided_inputs: provided,
                vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
                constants: Box::new([[0.0; 4]; CONSTANTS]),
                // SetPixelShader's latch, from the definition it bound.
                final_combiner_latch: pixel_shader.as_ref().is_some_and(|w| w[8] | w[9] != 0),
                affine_projection: false,
                pixel_shader,
                combiner_state,
            },
            raw: Box::new(words.clone()),
            viewport: Viewport {
                x: 0,
                y: 0,
                width: size,
                height: size,
                min_z: 0.0,
                max_z: 1.0,
            },
            textures: [info.map(|i| i.header), None, None, None],
            glyph_cache: 0,
            clip: None,
            clip_rects: Vec::new(),
            hw: d3d::hw::HwState {
                stipple_pattern: c.stipple,
                ..Default::default()
            },
        };
        assert!(draw.state.bad.is_empty(), "{:?}", draw.state.bad);
        let scope = b.ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = b.draw(self.color, None, &draw, [host.as_ref(), None, None, None]);
        // The draw is encoded and submitted with the pending pass.
        b.flush().unwrap();
        let err = pollster::block_on(scope.pop());
        assert_eq!(r, Ok(DrawOutcome::Drawn));
        assert!(err.is_none(), "{err:?}");

        let (px, order) = b.read_color(self.color).unwrap();
        let i = 4 * (at.1 * size + at.0) as usize;
        let p = [px[i], px[i + 1], px[i + 2], px[i + 3]];
        match order {
            HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
            HostPixelOrder::Rgba8 => p,
        }
    }
}

/// Bytes within 2 of `want` (unorm8 rounding of a GPU result).
#[track_caller]
fn expect(got: [u8; 4], want: [f32; 4], what: &str) {
    let want = want.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as i32);
    assert!(
        got.iter()
            .zip(want)
            .all(|(g, w)| (*g as i32 - w).abs() <= 2),
        "{what}: got {got:?}, want {want:?}"
    );
}

/// One general combiner stage writing `r0` (rgb from AB or the sum).
fn stage(d: &mut PixelShaderDef, n: usize, rgb: u32, rgb_out: u32, alpha: u32, alpha_out: u32) {
    d.rgb_inputs[n] = rgb;
    d.rgb_outputs[n] = rgb_out;
    d.alpha_inputs[n] = alpha;
    d.alpha_outputs[n] = alpha_out;
}

/// `r0 = v0` (rgb and alpha) as stage 0, `count` stages in all.
fn copy_v0(count: u32) -> PixelShaderDef {
    let mut d = PixelShaderDef {
        combiner_count: count | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        ..PixelShaderDef::default()
    };
    stage(
        &mut d,
        0,
        inputs(REG_V0, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
        inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
    );
    d
}

/// `r0 = t0` (rgb and alpha) with stage 0 in PROJECT2D.
fn copy_t0() -> PixelShaderDef {
    let mut d = PixelShaderDef {
        combiner_count: 1 | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        texture_modes: texture_modes([TEX_PROJECT2D, 0, 0, 0]),
        ..PixelShaderDef::default()
    };
    stage(
        &mut d,
        0,
        inputs(REG_T0, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
        inputs(REG_T0 | CHAN_ALPHA, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
    );
    d
}

/// Combiner constants are `D3DCOLOR`s (`NV097_SET_COMBINER_FACTOR` is
/// ARGB): `0xFF0000FF` is blue. `UNIQUE_C0/C1` select per-stage constants,
/// otherwise stage 0's is used throughout; the final combiner reads its
/// own pair (`PSFinalCombinerConstant0/1`).
#[test]
fn combiner_constants_unpack_argb_per_stage_and_global() {
    let Some(mut g) = gpu() else { return };
    // Stage 0: r0 = c0. Stage 1: r0 = r0 + c1.
    let mut d = PixelShaderDef {
        combiner_count: 2 | COUNT_MUX_MSB | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1,
        ..PixelShaderDef::default()
    };
    stage(
        &mut d,
        0,
        inputs(REG_C0, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
        inputs(REG_C0 | CHAN_ALPHA, ONE, 0, 0),
        outputs(REG_R0, 0, 0, 0),
    );
    stage(
        &mut d,
        1,
        inputs(REG_R0, ONE, REG_C1, ONE),
        outputs(0, 0, REG_R0, 0),
        0,
        0,
    );
    d.constant0 = [0xFF00_00FF, 0xFF00_FF00, 0, 0, 0, 0, 0, 0];
    d.constant1 = [0xFF00_4000, 0xFF80_0000, 0, 0, 0, 0, 0, 0];
    let mut c = Case {
        def: Some(d.clone()),
        ..Case::default()
    };
    expect(
        g.run(&c),
        [128.0 / 255.0, 0.0, 1.0, 1.0],
        "per-stage c0[0], c1[1]",
    );

    let mut global = d.clone();
    global.combiner_count = 2 | COUNT_MUX_MSB | COUNT_UNIQUE_C0;
    c.def = Some(global);
    expect(g.run(&c), [0.0, 64.0 / 255.0, 1.0, 1.0], "c1 from stage 0");

    // The final combiner's c0 is its own: D = c0 -> PSFinalCombinerConstant0.
    let mut fin = d;
    fin.final_inputs_abcd = inputs(0, 0, 0, REG_C0);
    fin.final_inputs_efg = final_efg(0, 0, REG_C1 | CHAN_ALPHA, 0);
    fin.final_constant0 = 0x0000_FF00;
    fin.final_constant1 = 0x8000_0000;
    c.def = Some(fin);
    expect(g.run(&c), [0.0, 1.0, 0.0, 128.0 / 255.0], "final constants");
}

/// The eight input mappings (`NV_register_combiners`), on a positive and a
/// negative register value. Read as `r0 = 0.5 * map(x) + 0.5`.
#[test]
fn input_mappings() {
    let Some(mut g) = gpu() else { return };
    let half = 128.0 / 255.0;
    for map in [
        MAP_UNSIGNED_IDENTITY,
        MAP_UNSIGNED_INVERT,
        MAP_EXPAND_NORMAL,
        MAP_EXPAND_NEGATE,
        MAP_HALFBIAS_NORMAL,
        MAP_HALFBIAS_NEGATE,
        MAP_SIGNED_IDENTITY,
        MAP_SIGNED_NEGATE,
    ] {
        // Stage 0: r1 = x (0.75 from v0, or -0x80/255 as -c1). Stage 1:
        // r0 = map(r1) * c0 + 1 * c0, c0 = 0x80/255.
        for x in [0.75f32, -half] {
            let mut d = PixelShaderDef {
                combiner_count: 2 | COUNT_MUX_MSB,
                ..PixelShaderDef::default()
            };
            let src = if x > 0.0 {
                inputs(REG_V0, ONE, 0, 0)
            } else {
                inputs(REG_C1 | MAP_SIGNED_NEGATE, ONE, 0, 0)
            };
            stage(&mut d, 0, src, outputs(REG_R1, 0, 0, 0), 0, 0);
            stage(
                &mut d,
                1,
                inputs(REG_R1 | map, REG_C0, ONE, REG_C0),
                outputs(0, 0, REG_R0, 0),
                inputs(ONE, ONE, 0, 0),
                outputs(REG_R0, 0, 0, 0),
            );
            d.constant0[0] = 0xFF80_8080;
            d.constant1[0] = 0x0080_8080;
            let c = Case {
                def: Some(d),
                d0: [0.75; 4],
                ..Case::default()
            };
            let v = half * spec_map(map, x) + half;
            expect(
                g.run(&c),
                [v, v, v, 1.0],
                &format!("mapping {map:#x} of {x}"),
            );
        }
    }
}

/// The input mappings as `NV_register_combiners` tabulates them.
fn spec_map(map: u8, x: f32) -> f32 {
    match map {
        MAP_UNSIGNED_IDENTITY => x.max(0.0),
        MAP_UNSIGNED_INVERT => 1.0 - x.clamp(0.0, 1.0),
        MAP_EXPAND_NORMAL => 2.0 * x.max(0.0) - 1.0,
        MAP_EXPAND_NEGATE => 1.0 - 2.0 * x.max(0.0),
        MAP_HALFBIAS_NORMAL => x.max(0.0) - 0.5,
        MAP_HALFBIAS_NEGATE => 0.5 - x.max(0.0),
        MAP_SIGNED_IDENTITY => x,
        _ => -x,
    }
}

/// Output scale/bias, the [-1, 1] register clamp, mux on `r0.a` (MSB),
/// AB/CD dot products, and AB blue-to-alpha.
#[test]
fn output_mapping_mux_dot_and_blue_to_alpha() {
    let Some(mut g) = gpu() else { return };
    for (flags, x, want) in [
        (OUT_BIAS, 0.75f32, 0.25f32),
        (OUT_SHIFTLEFT_1, 0.3, 0.6),
        (OUT_SHIFTLEFT_1_BIAS, 0.7, 0.4),
        (OUT_SHIFTLEFT_2, 0.2, 0.8),
        (OUT_SHIFTRIGHT_1, 0.7, 0.35),
        (OUT_SHIFTLEFT_2, 0.5, 1.0), // 2.0 clamps
    ] {
        let mut d = copy_v0(1);
        d.rgb_outputs[0] = outputs(REG_R0, 0, 0, flags);
        let c = Case {
            def: Some(d),
            d0: [x, x, x, 1.0],
            ..Case::default()
        };
        expect(
            g.run(&c),
            [want, want, want, 1.0],
            &format!("output map {flags:#x}"),
        );
    }

    // Mux: stage 1 writes red (AB) or blue (CD) by r0.a >= 0.5.
    for (a, want) in [(0.25f32, RED), (0.75, BLUE)] {
        let mut d = copy_v0(2);
        stage(
            &mut d,
            1,
            inputs(REG_C0, ONE, REG_C1, ONE),
            outputs(0, 0, REG_R0, OUT_AB_CD_MUX),
            inputs(ONE, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        );
        d.constant0[1] = 0xFFFF_0000;
        d.constant1[1] = 0xFF00_00FF;
        let c = Case {
            def: Some(d),
            d0: [0.0, 0.0, 0.0, a],
            ..Case::default()
        };
        expect(g.run(&c), want, &format!("mux on r0.a = {a}"));
    }

    // Dot products: expand(v0) . expand(c0) and CD = expand(v0) . expand(c1).
    // v0 = (1, 0.5, 0.5) -> (1, 0, 0); c0 -> (0.6, 0.8, 0) roughly; c1 = v0.
    let mut d = copy_v0(2);
    // Stage 0's alpha portion writes r0.a = 0 so blue-to-alpha shows.
    d.alpha_inputs[0] = 0;
    stage(
        &mut d,
        1,
        inputs(
            REG_V0 | MAP_EXPAND_NORMAL,
            REG_C0 | MAP_EXPAND_NORMAL,
            REG_V0 | MAP_EXPAND_NORMAL,
            REG_C1 | MAP_EXPAND_NORMAL,
        ),
        outputs(
            REG_R0,
            REG_R1,
            0,
            OUT_AB_DOT_PRODUCT | OUT_CD_DOT_PRODUCT | OUT_AB_BLUE_TO_ALPHA,
        ),
        0,
        0,
    );
    d.constant0[1] = 0xFFCC_E680; // (0.8, 0.902, 0.502) -> (0.6, 0.804, 0.004)
    d.constant1[1] = 0xFFFF_8080;
    let c = Case {
        def: Some(d),
        d0: [1.0, 0.5, 0.5, 1.0],
        ..Case::default()
    };
    let e = |b: f32| 2.0 * b / 255.0 - 1.0;
    let ev0 = [1.0, 0.0, 0.0];
    let dot = ev0[0] * e(204.0) + ev0[1] * e(230.0) + ev0[2] * e(128.0);
    expect(
        g.run(&c),
        [dot, dot, dot, dot],
        "AB dot replicated, blue to alpha",
    );
}

/// Whether a final combiner runs is latched from the bound definition
/// (`SetPixelShader`, device `+0x788`). Pushing zero to both final-combiner
/// methods afterwards leaves an explicit all-zero final combiner (RGB 0,
/// alpha 0), not D3D's default one; a definition that never had one still
/// gets the default (`r0`), with or without the pushes.
#[test]
fn explicit_final_combiner_survives_zero_pushes() {
    let Some(mut g) = gpu() else { return };
    let mut d = copy_v0(1);
    // Final: rgb = D = r0, alpha = G = r0.a.
    d.final_inputs_abcd = inputs(0, 0, 0, REG_R0);
    d.final_inputs_efg = final_efg(0, 0, REG_R0 | CHAN_ALPHA, 0);
    let mut c = Case {
        def: Some(d.clone()),
        d0: [1.0, 0.0, 0.0, 1.0],
        clear: [0.0, 0.0, 1.0, 1.0],
        ..Case::default()
    };
    expect(g.run(&c), RED, "explicit final");
    c.pushed = vec![(8, 0), (9, 0)];
    expect(g.run(&c), [0.0; 4], "explicit final, zero pushed");
    // No final combiner in the definition: the default one, r0.
    d.final_inputs_abcd = 0;
    d.final_inputs_efg = 0;
    c.def = Some(d);
    expect(g.run(&c), RED, "default final, zero pushed");
    c.pushed.clear();
    expect(g.run(&c), RED, "default final");
}

/// The final combiner: `A*B + (1-A)*C + D`, the `v1 + r0` input with its
/// complement and clamp flags, the `E*F` input, and `G` as alpha.
#[test]
fn final_combiner_lerp_sum_product_and_g() {
    let Some(mut g) = gpu() else { return };
    // Lerp: A = v0.a (0.25), B = r0 (red), C = c1 (blue), D = 0.
    let mut d = copy_v0(1);
    d.final_inputs_abcd = inputs(REG_V0 | CHAN_ALPHA, REG_R0, REG_C1, 0);
    d.final_inputs_efg = final_efg(0, 0, REG_C0 | CHAN_ALPHA, 0);
    d.final_constant0 = 0x8000_0000;
    d.final_constant1 = 0xFF00_00FF;
    let c = Case {
        def: Some(d),
        d0: [1.0, 0.0, 0.0, 0.25],
        ..Case::default()
    };
    expect(
        g.run(&c),
        [0.25, 0.0, 0.75, 128.0 / 255.0],
        "lerp, G = c0.a",
    );

    // v1 + r0: r0 = 0.75 red, v1 = 0.5 red.
    for (flags, want) in [
        (FINAL_CLAMP_SUM, 1.0f32),
        (0, 1.0), // 1.25, then the output clamp
        (FINAL_COMPLEMENT_R0, 0.75),
        (FINAL_COMPLEMENT_V1, 1.0),
        (FINAL_COMPLEMENT_R0 | FINAL_COMPLEMENT_V1, 0.75),
    ] {
        let mut d = copy_v0(1);
        d.final_inputs_abcd = inputs(0, 0, 0, REG_V1R0_SUM);
        d.final_inputs_efg = final_efg(0, 0, ONE, flags);
        let c = Case {
            def: Some(d),
            d0: [0.75, 0.0, 0.0, 1.0],
            d1: [0.5, 0.0, 0.0, 0.0],
            ..Case::default()
        };
        // Green and blue: 0 + 0, inverted where complemented.
        let complemented = flags & (FINAL_COMPLEMENT_R0 | FINAL_COMPLEMENT_V1) != 0;
        let gb = if complemented { 1.0 } else { 0.0 };
        expect(
            g.run(&c),
            [want, gb, gb, 1.0],
            &format!("v1+r0 flags {flags:#x}"),
        );
    }

    // E * F: E = v0.a (0.25), F = c0 (green); D = EF.
    let mut d = copy_v0(1);
    d.final_inputs_abcd = inputs(0, 0, 0, REG_EF_PROD);
    d.final_inputs_efg = final_efg(
        REG_V0 | CHAN_ALPHA,
        REG_C0,
        REG_ZERO | MAP_UNSIGNED_INVERT,
        0,
    );
    d.final_constant0 = 0xFF00_FF00;
    let c = Case {
        def: Some(d),
        d0: [1.0, 0.0, 0.0, 0.25],
        ..Case::default()
    };
    expect(g.run(&c), [0.0, 0.25, 0.0, 1.0], "E*F");

    // G with its channel bit clear reads blue (as the alpha portion).
    let mut d = copy_v0(1);
    d.final_inputs_abcd = inputs(0, 0, 0, REG_R0);
    d.final_inputs_efg = final_efg(0, 0, REG_V0, 0);
    let c = Case {
        def: Some(d),
        d0: [0.0, 0.0, 0.4, 0.9],
        ..Case::default()
    };
    expect(g.run(&c), [0.0, 0.0, 0.4, 0.4], "G = v0.b");
}

/// Fog with a definition that has no final combiner: D3D programs
/// `fog.a * r0 + (1 - fog.a) * fog.rgb` from FOGENABLE (`0x00400146`).
/// `D3DRS_FOGCOLOR` is a `D3DCOLOR` (`0xFF0000FF` is blue). The factor is
/// `oFog.x` with no table mode, or the linear/exp/exp2 functions of it
/// with the parameters D3D derives (`0x00400000`).
#[test]
fn fog_modes_and_color_order() {
    let Some(mut g) = gpu() else { return };
    let base = Case {
        def: Some(copy_v0(1)),
        d0: RED,
        render: vec![(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0xFF00_00FF)],
        ..Case::default()
    };
    let with = |extra: &[(u32, u32)], fog: f32| {
        let mut c = base.clone();
        c.render.extend_from_slice(extra);
        c.fog = fog;
        c
    };
    let mix = |k: f32| [k, 0.0, 1.0 - k, 1.0];
    expect(g.run(&with(&[], 0.25)), mix(0.25), "factor from the vertex");
    expect(g.run(&with(&[], 1.5)), mix(1.0), "vertex factor clamps");
    let linear = [
        (RS_FOGTABLEMODE, 3),
        (RS_FOGSTART, f(0.0)),
        (RS_FOGEND, f(100.0)),
    ];
    expect(g.run(&with(&linear, 75.0)), mix(0.25), "linear");
    let step = [
        (RS_FOGTABLEMODE, 3),
        (RS_FOGSTART, f(50.0)),
        (RS_FOGEND, f(50.0)),
    ];
    expect(
        g.run(&with(&step, 40.0)),
        mix(1.0),
        "linear, start == end, near",
    );
    expect(
        g.run(&with(&step, 60.0)),
        mix(0.0),
        "linear, start == end, far",
    );
    let exp = [(RS_FOGTABLEMODE, 1), (RS_FOGDENSITY, f(0.01))];
    expect(g.run(&with(&exp, 100.0)), mix((-1.0f32).exp()), "exp");
    let exp2 = [(RS_FOGTABLEMODE, 2), (RS_FOGDENSITY, f(0.01))];
    expect(g.run(&with(&exp2, 50.0)), mix((-0.25f32).exp()), "exp2");
    let mut off = with(&[], 0.25);
    off.render[0] = (RS_FOGENABLE, 0);
    expect(g.run(&off), RED, "FOGENABLE off");
    // Specular add (v1 + r0) in the same default final combiner.
    let mut spec = off.clone();
    spec.render.push((RS_SPECULARENABLE, 1));
    spec.d1 = [0.0, 0.5, 0.0, 0.0];
    expect(g.run(&spec), [1.0, 0.5, 0.0, 1.0], "specular");
    let mut both = with(&[(RS_SPECULARENABLE, 1)], 0.5);
    both.d1 = [0.0, 0.5, 0.0, 0.0];
    expect(g.run(&both), [0.5, 0.25, 0.5, 1.0], "specular and fog");
}

/// The rest of D3D's fog commit (`0x00400000`) as the hardware applies it
/// to a vertex program's `oFog.x`: a table mode D3D does not name (here 7)
/// falls through to EXP2; `RANGEFOGENABLE` only changes the generation
/// mode, which a vertex program ignores; the factor clamps to [0, 1] on
/// both sides of the linear range; the fog colour's alpha never shows; and
/// a definition that reads the fog register in its own final combiner
/// gets the same factor.
#[test]
fn fog_table_fallthrough_range_fog_and_clamps() {
    let Some(mut g) = gpu() else { return };
    let base = Case {
        def: Some(copy_v0(1)),
        d0: RED,
        render: vec![(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0x4000_00FF)],
        ..Case::default()
    };
    let with = |extra: &[(u32, u32)], fog: f32| {
        let mut c = base.clone();
        c.render.extend_from_slice(extra);
        c.fog = fog;
        c
    };
    let mix = |k: f32| [k, 0.0, 1.0 - k, 1.0];
    let other = [(RS_FOGTABLEMODE, 7), (RS_FOGDENSITY, f(0.01))];
    expect(
        g.run(&with(&other, 50.0)),
        mix((-0.25f32).exp()),
        "unnamed table mode is exp2",
    );
    let linear = [
        (RS_FOGTABLEMODE, 3),
        (RS_FOGSTART, f(20.0)),
        (RS_FOGEND, f(60.0)),
    ];
    for range in [0, 1] {
        let mut l = linear.to_vec();
        l.push((RS_RANGEFOGENABLE, range));
        expect(g.run(&with(&l, 30.0)), mix(0.75), "linear, range fog");
        expect(g.run(&with(&l, 0.0)), mix(1.0), "nearer than start");
        expect(g.run(&with(&l, 100.0)), mix(0.0), "beyond end");
        expect(g.run(&with(&l, -50.0)), mix(1.0), "behind the eye");
    }
    // Vertex fog clamps below 0 too.
    expect(g.run(&with(&[], -0.5)), mix(0.0), "negative vertex factor");
    // A definition's own final combiner: rgb = fog.a * v0 + (1 - fog.a) *
    // fog.rgb, alpha = fog.a (the factor, not FOGCOLOR's 0x40).
    let mut own = copy_v0(1);
    own.final_inputs_abcd = inputs(REG_FOG | CHAN_ALPHA, REG_R0, REG_FOG, REG_ZERO);
    own.final_inputs_efg = final_efg(0, 0, REG_FOG | CHAN_ALPHA, 0);
    let mut c = with(&linear, 50.0);
    c.def = Some(own);
    expect(g.run(&c), [0.25, 0.0, 0.75, 0.25], "own final combiner");
}

/// Texture formats sample in NV2A channel order: `A8R8G8B8` words are ARGB
/// (bytes B, G, R, A), 16-bit formats put red in the top bits, DXT colours
/// are 5:6:5 with red on top, palettes hold `D3DCOLOR`s, and `X` formats
/// read alpha 1.
#[test]
fn texture_formats_decode_in_nv2a_channel_order() {
    let Some(mut g) = gpu() else { return };
    let b = |x: u32| x as f32 / 255.0;
    let dxt1 = |c: u16| {
        let mut v = Vec::new();
        v.extend(c.to_le_bytes());
        v.extend(c.to_le_bytes());
        v.extend([0u8; 4]); // every texel colour 0
        Tex {
            format: D3DFormat::Dxt1,
            width: 4,
            height: 4,
            levels: 1,
            bytes: v,
            palette: None,
        }
    };
    let cases: Vec<(Tex, [f32; 4], &str)> = vec![
        (
            Tex::solid32(D3DFormat::A8R8G8B8, 0x80FF_4020),
            [1.0, b(0x40), b(0x20), b(0x80)],
            "A8R8G8B8",
        ),
        (
            Tex::solid32(D3DFormat::X8R8G8B8, 0x0020_40FF),
            [b(0x20), b(0x40), 1.0, 1.0],
            "X8R8G8B8",
        ),
        (
            Tex::solid32(D3DFormat::A8B8G8R8, 0x80FF_4020),
            [b(0x20), b(0x40), 1.0, b(0x80)],
            "A8B8G8R8",
        ),
        (Tex::solid16(D3DFormat::R5G6B5, 0x001F), BLUE, "R5G6B5 blue"),
        (Tex::solid16(D3DFormat::R5G6B5, 0xF800), RED, "R5G6B5 red"),
        (
            Tex::solid16(D3DFormat::A1R5G5B5, 0x7C00),
            [1.0, 0.0, 0.0, 0.0],
            "A1R5G5B5",
        ),
        (Tex::solid16(D3DFormat::A4R4G4B4, 0xF00F), BLUE, "A4R4G4B4"),
        (
            Tex::solid8(D3DFormat::L8, 0x80),
            [b(0x80), b(0x80), b(0x80), 1.0],
            "L8",
        ),
        (
            Tex::solid16(D3DFormat::A8L8, 0x4080),
            [b(0x80), b(0x80), b(0x80), b(0x40)],
            "A8L8",
        ),
        (dxt1(0x001F), BLUE, "DXT1 blue"),
        (dxt1(0xF800), RED, "DXT1 red"),
        (
            Tex {
                palette: Some(vec![0xFF00_00FF, 0xFFFF_0000]),
                ..Tex::solid8(D3DFormat::P8, 0)
            },
            BLUE,
            "P8 entry 0",
        ),
    ];
    for (tex, want, what) in cases {
        let c = Case {
            def: Some(copy_t0()),
            texture: Some(tex),
            ..Case::default()
        };
        expect(g.run(&c), want, what);
    }
}

/// `D3DTSS_COLORSIGN` makes the texture unit read channels as signed bytes
/// `s`, delivered as `2s/255` (Xbox captures: 127 reads 254/255) and
/// clamped at -1: with R, G and B signed, 0x7F reads 254/255, 0x80 reads -1
/// and 0xC0 reads -128/255. Read through `r0 = 0.5 * t0 + 0.5` (signed
/// identity).
#[test]
fn color_sign_on_texture_stage() {
    let Some(mut g) = gpu() else { return };
    let mut d = copy_t0();
    d.rgb_inputs[0] = inputs(REG_T0 | MAP_SIGNED_IDENTITY, REG_C0, ONE, REG_C0);
    d.rgb_outputs[0] = outputs(0, 0, REG_R0, 0);
    d.constant0[0] = 0xFF80_8080;
    let half = 128.0 / 255.0;
    let tex = Tex::solid32(D3DFormat::A8R8G8B8, 0xFF7F_80C0);
    let mut c = Case {
        def: Some(d),
        texture: Some(tex),
        stage0: vec![(TSS_COLORSIGN, 0xE000_0000)],
        ..Case::default()
    };
    let s = |x: f32| half * x + half;
    expect(
        g.run(&c),
        [s(254.0 / 255.0), s(-1.0), s(-128.0 / 255.0), 1.0],
        "signed RGB",
    );
    c.stage0.clear();
    let u = |x: u32| s(x as f32 / 255.0);
    expect(g.run(&c), [u(0x7F), u(0x80), u(0xC0), 1.0], "unsigned");
}

/// The signed transfer, exactly, at points read from the Xbox captures of
/// nxdk_pgraph_tests (`Texture_signed_component_tests`, signed red ramp:
/// bytes 64, 100, 127 give framebuffer 128, 200, 254). Point sampled and
/// copied straight to the target, so no tolerance.
#[test]
fn color_sign_matches_hardware_ramp() {
    let Some(mut g) = gpu() else { return };
    let c = Case {
        def: Some(copy_t0()),
        texture: Some(Tex::solid32(D3DFormat::A8R8G8B8, 0xFF7F_6440)),
        stage0: vec![(TSS_COLORSIGN, 0xE000_0000)],
        ..Case::default()
    };
    assert_eq!(g.run(&c), [254, 200, 128, 255]);
}

/// Signed texels are filtered as signed values. A bilinear sample half-way
/// between 0x7F (+127) and 0x80 (-128) is about 0, so `0.5 * t0 + 0.5`
/// is mid grey. Converting after filtering the unsigned bytes would see
/// 0x7F.8 and read -1 (black) or +1 (white).
#[test]
fn color_sign_filters_signed_values() {
    let Some(mut g) = gpu() else { return };
    let mut d = copy_t0();
    d.rgb_inputs[0] = inputs(REG_T0 | MAP_SIGNED_IDENTITY, REG_C0, ONE, REG_C0);
    d.rgb_outputs[0] = outputs(0, 0, REG_R0, 0);
    d.constant0[0] = 0xFF80_8080;
    // 2x2 swizzled (Morton order): left column 0x7F, right column 0x80.
    let tex = Tex {
        format: D3DFormat::A8R8G8B8,
        width: 2,
        height: 2,
        levels: 1,
        bytes: [0xFF7F_7F7Fu32, 0xFF80_8080, 0xFF7F_7F7F, 0xFF80_8080]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect(),
        palette: None,
    };
    let c = Case {
        def: Some(d),
        texture: Some(tex),
        stage0: vec![
            (TSS_COLORSIGN, 0xE000_0000),
            (TSS_MAGFILTER, TEXF_LINEAR),
            (TSS_MINFILTER, TEXF_LINEAR),
        ],
        ..Case::default()
    };
    // (127 - 128) / 2 = -0.5 -> 2 * -0.5 / 255.
    let half = 128.0 / 255.0;
    let v = half * (-1.0 / 255.0) + half;
    expect(g.run(&c), [v, v, v, 1.0], "filtered signed seam");
}

/// `r0.rgb = 0.5 * signed(t0.rgb) + 0.5`, `r0.a = t0.a`.
fn rebias_t0() -> PixelShaderDef {
    let mut d = copy_t0();
    d.rgb_inputs[0] = inputs(REG_T0 | MAP_SIGNED_IDENTITY, REG_C0, ONE, REG_C0);
    d.rgb_outputs[0] = outputs(0, 0, REG_R0, 0);
    d.constant0[0] = 0xFF80_8080;
    d
}

/// A signed stage's border is the border colour read through the sign unit,
/// not a host preset applied to biased texels: an opaque-black border with
/// signed RGB reads 0 (mid grey through the rebias), not -1 (black).
/// Inside the texture the texels still read signed.
#[test]
fn color_sign_border_reads_the_border_colour() {
    let Some(mut g) = gpu() else { return };
    let half = 128.0 / 255.0;
    let mut c = Case {
        def: Some(rebias_t0()),
        texture: Some(Tex::solid32(D3DFormat::A8R8G8B8, 0xFF40_4040)),
        stage0: vec![
            (TSS_COLORSIGN, 0xE000_0000),
            (TSS_ADDRESSU, TADDRESS_BORDER),
            (TSS_BORDERCOLOR, 0xFF00_0000),
        ],
        t0: [-1.0, 0.5, 0.0, 1.0],
        ..Case::default()
    };
    expect(
        g.run(&c),
        [half, half, half, 1.0],
        "black border, signed RGB",
    );
    // 0x40 in the border's red reads 128/255 like a texel would.
    c.stage0[2].1 = 0xFF40_0000;
    let s = |x: f32| half * x + half;
    expect(
        g.run(&c),
        [s(half), half, half, 1.0],
        "red border byte 0x40",
    );
    c.t0 = [0.5, 0.5, 0.0, 1.0];
    expect(
        g.run(&c),
        [s(half), s(half), s(half), 1.0],
        "inside the texture",
    );
}

/// An unbound signed stage samples the opaque-black stand-in as black:
/// mid grey through the rebias, as without `COLORSIGN`.
#[test]
fn color_sign_unbound_stage_reads_black() {
    let Some(mut g) = gpu() else { return };
    let half = 128.0 / 255.0;
    let mut c = Case {
        def: Some(rebias_t0()),
        stage0: vec![(TSS_COLORSIGN, 0xE000_0000)],
        ..Case::default()
    };
    expect(g.run(&c), [half, half, half, 1.0], "signed stand-in");
    c.stage0.clear();
    expect(g.run(&c), [half, half, half, 1.0], "unsigned stand-in");
}

/// `D3DRS_BLENDCOLOR` is a `D3DCOLOR` (`NV097_SET_BLEND_COLOR` is ARGB);
/// Halo 2 blends with `CONSTANTCOLOR` (0x8001) and `BLENDCOLOR =
/// 0x00FFFFFF` to keep RGB and drop alpha.
#[test]
fn blend_color_constant_factors() {
    let Some(mut g) = gpu() else { return };
    let white = Case {
        def: Some(copy_v0(1)),
        d0: [1.0; 4],
        clear: [0.0, 0.0, 1.0, 1.0],
        ..Case::default()
    };
    let blend = |src: u32, dst: u32, color: u32| {
        let mut c = white.clone();
        c.render.extend([
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, src),
            (RS_DESTBLEND, dst),
            (RS_BLENDCOLOR, color),
        ]);
        c
    };
    // src * (1, 0, 0, 0.5) + dst * 0.
    expect(
        g.run(&blend(0x8001, 0, 0x80FF_0000)),
        [1.0, 0.0, 0.0, 128.0 / 255.0],
        "CONSTANTCOLOR",
    );
    // src * 0.5 (constant alpha) + dst * 0.
    expect(
        g.run(&blend(0x8003, 0, 0x80FF_0000)),
        [0.5, 0.5, 0.5, 0.5],
        "CONSTANTALPHA",
    );
    // Halo's: src * (1, 1, 1, 0) + dst * 1 on blue: (1, 1, 1, 1).
    expect(
        g.run(&blend(0x8001, 1, 0x00FF_FFFF)),
        [1.0, 1.0, 1.0, 1.0],
        "0x00FFFFFF additive",
    );
    // INVCONSTANTCOLOR on the destination: dst * (1 - (0, 0, 1, 0)).
    expect(
        g.run(&blend(0, 0x8002, 0x0000_00FF)),
        [0.0, 0.0, 0.0, 1.0],
        "INVCONSTANTCOLOR",
    );
}

/// Fixed-function stages: `TFACTOR` is a `D3DCOLOR` (`0xFF0000FF` blue),
/// `D3DCOLOR` vertex diffuse is bytes B, G, R, A.
#[test]
fn fixed_function_tfactor_and_vertex_color_order() {
    let Some(mut g) = gpu() else { return };
    let c = Case {
        render: vec![(RS_TEXTUREFACTOR, 0xFF00_00FF)],
        stage0: vec![
            (TSS_COLOROP, TOP_SELECTARG1),
            (TSS_COLORARG1, TA_TFACTOR),
            (TSS_ALPHAOP, TOP_SELECTARG1),
            (TSS_ALPHAARG1, TA_TFACTOR),
        ],
        ..Case::default()
    };
    expect(g.run(&c), BLUE, "TFACTOR");
    let c = Case {
        render: vec![(RS_TEXTUREFACTOR, 0xFF80_8080)],
        stage0: vec![
            (TSS_COLOROP, TOP_MODULATE),
            (TSS_COLORARG1, TA_DIFFUSE),
            (TSS_COLORARG2, TA_TFACTOR),
            (TSS_ALPHAOP, TOP_SELECTARG1),
            (TSS_ALPHAARG1, TA_DIFFUSE),
        ],
        d3dcolor_diffuse: Some(0xFF20_40FF),
        ..Case::default()
    };
    let h = 128.0 / 255.0;
    expect(
        g.run(&c),
        [h * 0x20 as f32 / 255.0, h * 0x40 as f32 / 255.0, h, 1.0],
        "D3DCOLOR diffuse * TFACTOR",
    );
}

/// `D3DRS_FRONTFACE` names the front winding for two-sided lighting (back
/// faces read `oB0`); `CULLMODE` alone decides culling
/// (`SetRenderState_CullMode`, `0x3f53d0`).
#[test]
fn front_face_drives_two_sided_lighting_and_culling() {
    let Some(mut g) = gpu() else { return };
    let base = Case {
        def: Some(copy_v0(1)),
        d0: RED,
        b0: BLUE,
        clear: [0.0, 1.0, 0.0, 1.0],
        render: vec![(RS_TWOSIDEDLIGHTING, 1)],
        ..Case::default()
    };
    let green = [0.0, 1.0, 0.0, 1.0];
    // The default triangle is clockwise on screen.
    for (front, ccw, want) in [
        (0x900, false, RED),
        (0x900, true, BLUE),
        (0x901, false, BLUE),
        (0x901, true, RED),
    ] {
        let mut c = base.clone();
        c.render.push((RS_FRONTFACE, front));
        c.ccw = ccw;
        expect(g.run(&c), want, &format!("FRONTFACE {front:#x}, ccw {ccw}"));
    }
    for (cull, front, ccw, want) in [
        (0x900, 0x900, false, green), // CW culled
        (0x900, 0x901, false, green), // whatever FRONTFACE says
        (0x900, 0x901, true, RED),    // CCW kept, and it is the front
        (0x901, 0x900, true, green),
        (0x901, 0x900, false, RED),
    ] {
        let mut c = base.clone();
        c.render
            .extend([(RS_FRONTFACE, front), (RS_CULLMODE, cull)]);
        c.ccw = ccw;
        expect(
            g.run(&c),
            want,
            &format!("CULLMODE {cull:#x}, FRONTFACE {front:#x}, ccw {ccw}"),
        );
    }
}

#[test]
fn polygon_stipple_orientation_and_wraparound() {
    let Some(mut g) = gpu_with_size(64) else {
        return;
    };
    let mut c = Case {
        d0: RED,
        clear: BLUE,
        render: vec![(83, 1)],
        ..Case::default()
    };
    // Independent asymmetric pattern: row 27 bit 3 corresponds to (4,4).
    // It repeats at both 32-pixel boundaries; neighbours remain clear.
    c.stipple[27] = 0x0100_8008;
    c.stipple[26] = 0x0000_0004;
    for at in [(4, 4), (8, 4), (31, 4), (36, 4), (4, 36), (36, 36), (5, 5)] {
        expect(g.run_at(&c, at), RED, &format!("covered {at:?}"));
    }
    for at in [(5, 4), (4, 5), (35, 4), (4, 35)] {
        expect(g.run_at(&c, at), BLUE, &format!("uncovered {at:?}"));
    }
    c.render.clear();
    expect(g.run_at(&c, (5, 4)), RED, "stipple disabled");
}

#[test]
fn polygon_stipple_does_not_mask_wireframe_or_point_fill() {
    let Some(mut g) = gpu() else { return };
    for (mode, supported) in [
        (0x1b01, g.b.ctx.caps.polygon_mode_line),
        (0x1b00, g.b.ctx.caps.polygon_mode_point),
    ] {
        if !supported {
            continue;
        }
        let mut c = Case {
            d0: RED,
            clear: BLUE,
            small_triangle: true,
            render: vec![(139, mode)],
            ..Case::default()
        };
        g.run(&c);
        let (before, order) = g.b.read_color(g.color).unwrap();
        let blue = match order {
            HostPixelOrder::Bgra8 => [255, 0, 0, 255],
            HostPixelOrder::Rgba8 => [0, 0, 255, 255],
        };
        assert!(
            before.as_chunks::<4>().0.iter().any(|p| *p != blue),
            "mode {mode:#x} draws pixels"
        );
        c.render.push((83, 1));
        g.run(&c);
        let (after, _) = g.b.read_color(g.color).unwrap();
        assert_eq!(
            after, before,
            "zero stipple mask must not change mode {mode:#x}"
        );
    }
}

#[test]
fn unsigned_custom_border_keeps_all_channels() {
    let Some(mut g) = gpu() else { return };
    let c = Case {
        def: Some(copy_t0()),
        texture: Some(Tex::solid32(D3DFormat::A8R8G8B8, 0xFFFFFFFF)),
        stage0: vec![
            (TSS_ADDRESSU, TADDRESS_BORDER),
            (TSS_BORDERCOLOR, 0x7F11_5DCD),
        ],
        t0: [-1.0, 0.5, 0.0, 1.0],
        ..Case::default()
    };
    expect(
        g.run(&c),
        [17.0 / 255.0, 93.0 / 255.0, 205.0 / 255.0, 127.0 / 255.0],
        "custom border",
    );
}

#[test]
fn texture_lod_bias_changes_the_selected_mip() {
    let Some(mut g) = gpu() else { return };
    let mut bytes = 0xffff_0000u32.to_le_bytes().repeat(16);
    bytes.extend(0xff00_ff00u32.to_le_bytes().repeat(4));
    bytes.extend(0xff00_00ffu32.to_le_bytes());
    let mut c = Case {
        def: Some(copy_t0()),
        texture: Some(Tex {
            format: D3DFormat::A8R8G8B8,
            width: 4,
            height: 4,
            levels: 3,
            bytes,
            palette: None,
        }),
        stage0: vec![(5, 1), (6, 0.0f32.to_bits())],
        varying_t0: true,
        ..Case::default()
    };
    expect(g.run(&c), RED, "unbiased mip");
    c.stage0[1].1 = 6.0f32.to_bits();
    expect(g.run(&c), BLUE, "positive bias selects smaller mip");
}
