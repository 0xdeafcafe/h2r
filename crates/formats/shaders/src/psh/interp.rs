//! Reference CPU evaluation of the pixel pipeline, mirroring the generated
//! WGSL statement by statement.

use super::*;

/// RGBA.
pub type Vec4 = [f32; 4];

/// Interpolated inputs for one fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[allow(missing_docs)]
pub struct Inputs {
    pub d0: Vec4,
    pub d1: Vec4,
    pub b0: Vec4,
    pub b1: Vec4,
    pub t: [Vec4; 4],
    pub fog: f32,
    pub front: bool,
}

/// The values of `XpsUniforms`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct Uniforms {
    pub c0: [Vec4; 9],
    pub c1: [Vec4; 9],
    pub fog_color: Vec4,
    pub fog: Vec4,
    pub bump_mat: [Vec4; 4],
    pub bump_lum: [Vec4; 4],
    pub tex_scale: [Vec4; 4],
    pub eye: Vec4,
    pub alpha_ref: Vec4,
    pub border: [Vec4; 4],
    pub lod_bias: Vec4,
    pub stipple: [u32; 32],
    /// W-buffering: depth full scale, polygon offset units, slope factor.
    pub depth: Vec4,
    /// W-buffering: the clamp range of the stored w.
    pub depth_range: Vec4,
}

impl Default for Uniforms {
    fn default() -> Self {
        Self {
            c0: [[0.0; 4]; 9],
            c1: [[0.0; 4]; 9],
            fog_color: [0.0; 4],
            // D3D's vertex fog: the factor is the coordinate.
            fog: fog::Fog::FROM_VERTEX.uniform(),
            bump_mat: [[0.0; 4]; 4],
            bump_lum: [[1.0, 0.0, 0.0, 0.0]; 4],
            tex_scale: [[1.0; 4]; 4],
            eye: [0.0; 4],
            alpha_ref: [0.0; 4],
            border: [[0.0; 4]; 4],
            lod_bias: [0.0; 4],
            stipple: [0; 32],
            depth: [16_777_215.0, 0.0, 0.0, 1.0],
            depth_range: [0.0, 16_777_215.0, 0.0, 0.0],
        }
    }
}

impl Uniforms {
    /// The uniform buffer bytes, laid out as the WGSL struct (std140-like:
    /// every member here is a vec4 or an array of vec4).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0; Self::SIZE];
        self.write_bytes(&mut out);
        out
    }

    /// Size of the WGSL uniform block in bytes.
    pub const SIZE: usize = 49 * 16;

    /// Writes the WGSL layout into an existing buffer of [`Self::SIZE`] bytes.
    /// Stipple rows are copied as integer bits, without float conversion.
    pub fn write_bytes(&self, out: &mut [u8]) {
        assert_eq!(out.len(), Self::SIZE);
        let mut out = out;
        for vectors in [
            self.c0.as_slice(),
            self.c1.as_slice(),
            std::slice::from_ref(&self.fog_color),
            std::slice::from_ref(&self.fog),
            &self.bump_mat,
            &self.bump_lum,
            &self.tex_scale,
            std::slice::from_ref(&self.eye),
            std::slice::from_ref(&self.alpha_ref),
            &self.border,
            std::slice::from_ref(&self.lod_bias),
        ] {
            out = crate::common::uniform_vectors(out, vectors);
        }
        let (stipple, out) = out.split_at_mut(std::mem::size_of_val(&self.stipple));
        for (dst, row) in stipple.as_chunks_mut::<4>().0.iter_mut().zip(self.stipple) {
            *dst = row.to_le_bytes();
        }
        crate::common::uniform_vectors(out, &[self.depth, self.depth_range]);
    }
}

/// Texture lookups. `coord` is already scaled/projected as in the WGSL; for
/// 2D lookups only `x, y` are meaningful.
pub trait Sampler {
    /// Sample the texture bound to `stage`: the filtered value of the
    /// texels as the host stores them (`COLORSIGN` channels biased, see
    /// [`PixelOptions::color_sign`]).
    fn sample(&self, stage: usize, dim: TextureDim, coord: [f32; 3]) -> Vec4;
    /// Texel `(x, y)` of mip level 0 of the 2D texture bound to `stage`,
    /// unfiltered (shadow maps; `textureLoad`). The default panics.
    fn load(&self, stage: usize, x: u32, y: u32) -> Vec4 {
        panic!("stage {stage}: texel ({x}, {y}) loaded from a sampler without texels")
    }
    /// Width and height of mip level 0 of the texture bound to `stage`
    /// (`textureDimensions`). The default panics.
    fn size(&self, stage: usize) -> [u32; 2] {
        panic!("stage {stage}: size asked of a sampler without texels")
    }
}

/// A sampler for programs that never sample; panics if used.
pub struct NoTextures;
impl Sampler for NoTextures {
    fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> Vec4 {
        panic!("stage {stage} sampled with NoTextures")
    }
}

fn fog_factor(u: &Uniforms, f: f32) -> f32 {
    fog::factor_from_uniform(u.fog, f)
}

fn sign_d3d(x: f32) -> f32 {
    (x * 255.0 - 128.0) / 127.0
}
fn sign_gl(x: f32) -> f32 {
    let b = x * 255.0;
    if b >= 128.0 {
        (b - 255.5) / 127.5
    } else {
        (b + 0.5) / 127.5
    }
}
fn sign_nv(x: f32) -> f32 {
    let b = x * 255.0;
    if b >= 128.0 {
        (b - 256.0) / 127.0
    } else {
        b / 127.0
    }
}

/// `D3DTSS_COLORSIGN`: the masked channels of a sample from a texture that
/// stores signed bytes biased (`s + 128`), as `2s/255` (see
/// [`PixelOptions::color_sign`]).
fn signed(v: Vec4, mask: u8) -> Vec4 {
    let bit = [bits::SIGN_R, bits::SIGN_G, bits::SIGN_B, bits::SIGN_A];
    std::array::from_fn(|j| {
        if mask & bit[j] == 0 {
            return v[j];
        }
        (2.0 * v[j] - 256.0 / 255.0).max(-1.0)
    })
}

fn dot_map(m: DotMap, c: Vec4) -> [f32; 3] {
    if let Some(v) = hilo_map(m, c) {
        return v;
    }
    let f: fn(f32) -> f32 = match m {
        DotMap::Minus1To1D3d => sign_d3d,
        DotMap::Minus1To1Gl => sign_gl,
        DotMap::Minus1To1 => sign_nv,
        _ => |x| x,
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn map_in(x: f32, m: InputMap) -> f32 {
    match m {
        InputMap::UnsignedIdentity => x.max(0.0),
        InputMap::UnsignedInvert => 1.0 - x.clamp(0.0, 1.0),
        InputMap::ExpandNormal => 2.0 * x.max(0.0) - 1.0,
        InputMap::ExpandNegate => 1.0 - 2.0 * x.max(0.0),
        InputMap::HalfBiasNormal => x.max(0.0) - 0.5,
        InputMap::HalfBiasNegate => 0.5 - x.max(0.0),
        InputMap::SignedIdentity => x,
        InputMap::SignedNegate => -x,
    }
}

fn map_out(x: f32, m: OutMap) -> f32 {
    match m {
        OutMap::Identity => x,
        OutMap::Bias => x - 0.5,
        OutMap::Shl1 => x * 2.0,
        OutMap::Shl1Bias => (x - 0.5) * 2.0,
        OutMap::Shl2 => x * 4.0,
        OutMap::Shl2Bias => (x - 0.5) * 4.0,
        OutMap::Shr1 => x * 0.5,
        OutMap::Shr1Bias => (x - 0.5) * 0.5,
    }
}

struct Regs {
    v0: Vec4,
    v1: Vec4,
    t: [Vec4; 4],
    r0: Vec4,
    r1: Vec4,
    fog: Vec4,
    fc_sum: Vec4,
    fc_prod: Vec4,
}

impl Regs {
    fn get(&self, r: Reg, stage: Option<usize>, c: &Combiners, u: &Uniforms) -> Vec4 {
        let idx = |unique: bool| match stage {
            None => 8,
            Some(s) if unique => s,
            Some(_) => 0,
        };
        match r {
            Reg::Zero | Reg::Invalid(_) => [0.0; 4],
            Reg::C0 => u.c0[idx(c.unique_c0)],
            Reg::C1 => u.c1[idx(c.unique_c1)],
            Reg::Fog => self.fog,
            Reg::V0 => self.v0,
            Reg::V1 => self.v1,
            Reg::T(n) => self.t[n as usize],
            Reg::R0 => self.r0,
            Reg::R1 => self.r1,
            Reg::V1R0Sum if stage.is_none() => self.fc_sum,
            Reg::EfProd if stage.is_none() => self.fc_prod,
            Reg::V1R0Sum | Reg::EfProd => [0.0; 4],
        }
    }
    fn get_mut(&mut self, r: Reg) -> Option<&mut Vec4> {
        match r {
            Reg::V0 => Some(&mut self.v0),
            Reg::V1 => Some(&mut self.v1),
            Reg::T(n) => Some(&mut self.t[n as usize]),
            Reg::R0 => Some(&mut self.r0),
            Reg::R1 => Some(&mut self.r1),
            _ => None,
        }
    }
    /// RGB (3 values) or alpha (1 value, in `[0]`) of a mapped input.
    fn input(
        &self,
        i: &Input,
        alpha_portion: bool,
        stage: Option<usize>,
        c: &Combiners,
        u: &Uniforms,
    ) -> [f32; 3] {
        let v = self.get(i.reg, stage, c, u);
        let ch = match (alpha_portion, i.alpha) {
            (false, false) => [v[0], v[1], v[2]],
            (false, true) => [v[3]; 3],
            (true, false) => [v[2]; 3],
            (true, true) => [v[3]; 3],
        };
        ch.map(|x| map_in(x, i.map))
    }
}

/// Evaluates one fragment. Returns `None` when it is killed.
pub fn run(
    def: &PixelShaderDef,
    options: &PixelOptions,
    inputs: &Inputs,
    u: &Uniforms,
    sampler: &dyn Sampler,
) -> Option<Vec4> {
    let c = def.decode();
    let clamp01 = |v: Vec4| v.map(|x| x.clamp(0.0, 1.0));
    let (v0, v1) = if options.two_sided_lighting && !inputs.front {
        (inputs.b0, inputs.b1)
    } else {
        (inputs.d0, inputs.d1)
    };
    let fog = [
        u.fog_color[0],
        u.fog_color[1],
        u.fog_color[2],
        fog_factor(u, inputs.fog),
    ];
    let mut r = Regs {
        v0: clamp01(v0),
        v1: clamp01(v1),
        t: [[0.0, 0.0, 0.0, 1.0]; 4],
        r0: [0.0; 4],
        r1: [0.0; 4],
        fog,
        fc_sum: [0.0; 4],
        fc_prod: [0.0; 4],
    };
    let mut kill = false;
    let mut dots = [0.0f32; 4];
    let tc = inputs.t;

    for i in 0..4 {
        let mode = c.texture_modes[i];
        let dim = match mode {
            TexMode::Project2d | TexMode::Project3d | TexMode::Cubemap => mode
                .texture_dim()
                .map(|d| options.texture_dims[i].unwrap_or(d)),
            _ => mode.texture_dim(),
        };
        // An input selector that does not name an earlier stage reads zero,
        // as in the translation.
        let source = |r: &Regs, i: usize| c.input_stage(i).map_or([0.0; 4], |s| r.t[s]);
        let src = if mode.reads_input_texture() {
            source(&r, i)
        } else {
            [0.0; 4]
        };
        let mapped = dot_map(c.dot_mapping[i], src);
        let t = tc[i];
        let scale = u.tex_scale[i];
        let sign = options.color_sign[i];
        let axes = options.border[i] & 7;
        let outside =
            |c: [f32; 3]| (0..3).any(|j| axes >> j & 1 != 0 && !(0.0..=1.0).contains(&c[j]));
        let s2 = |uv: [f32; 2]| {
            let c = [uv[0] * scale[0], uv[1] * scale[1], 0.5];
            if outside(c) {
                return u.border[i];
            }
            signed(sampler.sample(i, TextureDim::D2, [c[0], c[1], 0.0]), sign)
        };
        let s3 = |d: TextureDim, v: [f32; 3]| {
            if d == TextureDim::D3 && outside(v) {
                return u.border[i];
            }
            signed(sampler.sample(i, d, v), sign)
        };
        use TexMode::*;
        let valid = match mode {
            BumpEnvMap | BumpEnvMapLum | DependentAr | DependentGb => i >= 1,
            DotProduct => i == 1 || i == 2,
            DotSt | DotZw | Brdf => i >= 2,
            DotRflctDiff => i == 2,
            DotRflctSpec | DotStr3d | DotStrCube | DotRflctSpecConst => i == 3,
            _ => true,
        };
        if !valid {
            continue;
        }
        match mode {
            None => {}
            Project2d | Project3d if options.shadow[i].is_some() && dim == Some(TextureDim::D2) => {
                r.t[i] = super::shadow::sample(
                    i,
                    &options.shadow[i].unwrap(),
                    mode == Project3d,
                    options.shadow_filter(i),
                    &tc,
                    &u.tex_scale,
                    sampler,
                );
            }
            Project2d | Project3d | Cubemap => {
                r.t[i] = match dim.unwrap() {
                    TextureDim::D2 => s2([t[0] / t[3], t[1] / t[3]]),
                    TextureDim::D3 => s3(TextureDim::D3, [t[0] / t[3], t[1] / t[3], t[2] / t[3]]),
                    TextureDim::Cube => s3(TextureDim::Cube, [t[0], t[1], t[2]]),
                }
            }
            Passthru => r.t[i] = clamp01(t),
            ClipPlane => {
                r.t[i] = [0.0; 4];
                for (j, &x) in t.iter().enumerate() {
                    let ge = c.compare_mode[i] & (1 << j) != 0;
                    kill |= if ge { x >= 0.0 } else { x < 0.0 };
                }
            }
            BumpEnvMap | BumpEnvMapLum => {
                let m = u.bump_mat[i];
                let uv = [
                    t[0] + m[0] * src[0] + m[2] * src[1],
                    t[1] + m[1] * src[0] + m[3] * src[1],
                ];
                r.t[i] = s2(uv);
                if mode == BumpEnvMapLum {
                    let l = (u.bump_lum[i][0] * src[2] + u.bump_lum[i][1]).clamp(0.0, 1.0);
                    r.t[i] = r.t[i].map(|x| x * l);
                }
            }
            DotProduct | DotZw => {
                dots[i] = dot3([t[0], t[1], t[2]], mapped);
                r.t[i] = [0.0; 4];
            }
            DotSt => {
                dots[i] = dot3([t[0], t[1], t[2]], mapped);
                r.t[i] = s2([dots[i - 1], dots[i]]);
            }
            DotRflctDiff => {
                dots[i] = dot3([t[0], t[1], t[2]], mapped);
                let m3 = dot_map(c.dot_mapping[3], source(&r, 3));
                let n3 = dot3([tc[3][0], tc[3][1], tc[3][2]], m3);
                r.t[i] = s3(TextureDim::Cube, [dots[1], dots[2], n3]);
            }
            DotRflctSpec | DotRflctSpecConst => {
                dots[i] = dot3([t[0], t[1], t[2]], mapped);
                let n = [dots[1], dots[2], dots[3]];
                let e = if mode == DotRflctSpec {
                    [tc[1][3], tc[2][3], tc[3][3]]
                } else {
                    [u.eye[0], u.eye[1], u.eye[2]]
                };
                let k = 2.0 * dot3(n, e) / dot3(n, n);
                r.t[i] = s3(TextureDim::Cube, [0, 1, 2].map(|j| k * n[j] - e[j]));
            }
            DotStr3d | DotStrCube => {
                dots[i] = dot3([t[0], t[1], t[2]], mapped);
                let d = if mode == DotStr3d {
                    TextureDim::D3
                } else {
                    TextureDim::Cube
                };
                r.t[i] = s3(d, [dots[1], dots[2], dots[3]]);
            }
            DependentAr => r.t[i] = s2([src[3], src[0]]),
            DependentGb => r.t[i] = s2([src[1], src[2]]),
            Brdf | Reserved(_) => r.t[i] = [0.0; 4],
        }
        if options.alpha_kill[i] && dim.is_some() && r.t[i][3] == 0.0 {
            kill = true;
        }
    }

    r.r0 = [
        0.0,
        0.0,
        0.0,
        if c.texture_modes[0] == TexMode::None {
            1.0
        } else {
            r.t[0][3]
        },
    ];

    for (n, st) in c.stages.iter().enumerate() {
        let mux = if c.mux_msb {
            r.r0[3] >= 0.5
        } else {
            ((r.r0[3] * 255.0) as u32) & 1 == 1
        };
        // Compute both portions before any write.
        let mut pending: Vec<(Reg, Option<[f32; 3]>, Option<f32>)> = Vec::new();
        for (alpha, p) in [(false, &st.rgb), (true, &st.alpha)] {
            let [a, b, cc, d] = p.inputs.map(|i| r.input(&i, alpha, Some(n), &c, u));
            let o = &p.output;
            let prod = |x: [f32; 3], y: [f32; 3], dot: bool| {
                if dot && !alpha {
                    [dot3(x, y); 3]
                } else {
                    [x[0] * y[0], x[1] * y[1], x[2] * y[2]]
                }
            };
            let ab = prod(a, b, o.ab_dot);
            let cd = prod(cc, d, o.cd_dot);
            let sum = if o.mux {
                if mux { cd } else { ab }
            } else {
                [0, 1, 2].map(|j| ab[j] + cd[j])
            };
            for (which, reg, val) in [(0, o.ab, ab), (1, o.cd, cd), (2, o.sum, sum)] {
                let v = val.map(|x| map_out(x, o.map).clamp(-1.0, 1.0));
                if alpha {
                    pending.push((reg, Option::None, Some(v[0])));
                } else {
                    pending.push((reg, Some(v), Option::None));
                    let bta = match which {
                        0 => o.ab_blue_to_alpha,
                        1 => o.cd_blue_to_alpha,
                        _ => false,
                    };
                    if bta {
                        pending.push((reg, Option::None, Some(v[2])));
                    }
                }
            }
        }
        for (reg, rgb, a) in pending {
            if let Some(dst) = r.get_mut(reg) {
                if let Some(v) = rgb {
                    dst[0] = v[0];
                    dst[1] = v[1];
                    dst[2] = v[2];
                }
                if let Some(a) = a {
                    dst[3] = a;
                }
            }
        }
    }

    let color = if let Some(f) = &c.final_combiner {
        // Each operand goes through an unsigned mapping (clamp to [0, 1],
        // inverted when complemented) before the add; see docs/graphics/shaders.md.
        let un = |x: f32, invert: bool| {
            let x = x.clamp(0.0, 1.0);
            if invert { 1.0 - x } else { x }
        };
        let mut sum =
            [0, 1, 2].map(|j| un(r.v1[j], f.complement_v1) + un(r.r0[j], f.complement_r0));
        if f.clamp_sum {
            sum = sum.map(|x| x.clamp(0.0, 1.0));
        }
        let sum = [sum[0], sum[1], sum[2], 0.0];
        r.fc_sum = sum;
        let e = r.input(&f.e, false, Option::None, &c, u);
        let ff = r.input(&f.f, false, Option::None, &c, u);
        r.fc_prod = [e[0] * ff[0], e[1] * ff[1], e[2] * ff[2], 0.0];
        let a = r.input(&f.a, false, Option::None, &c, u);
        let b = r.input(&f.b, false, Option::None, &c, u);
        let cc = r.input(&f.c, false, Option::None, &c, u);
        let d = r.input(&f.d, false, Option::None, &c, u);
        let g = r.input(&f.g, true, Option::None, &c, u)[0];
        let rgb = [0, 1, 2].map(|j| (a[j] * b[j] + (1.0 - a[j]) * cc[j] + d[j]).clamp(0.0, 1.0));
        [rgb[0], rgb[1], rgb[2], g.clamp(0.0, 1.0)]
    } else {
        clamp01(r.r0)
    };

    if let Some(f) = options.alpha_test
        && !f.passes((color[3] * 255.0).round_ties_even(), u.alpha_ref[0])
    {
        kill = true;
    }
    (!kill).then_some(color)
}

/// The HILO dot mappings (`None` for the others): the texel as one 32-bit
/// word laid out as `A8R8G8B8` samples it, hi = a:r, lo = g:b.
fn hilo_map(m: DotMap, c: Vec4) -> Option<[f32; 3]> {
    let b = c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round_ties_even());
    let (hi, lo) = (b[3] * 256.0 + b[0], b[1] * 256.0 + b[2]);
    let twos = |h: f32| if h >= 32768.0 { h - 65536.0 } else { h };
    let hemisphere = |x: f32, y: f32| [x, y, (1.0 - (x * x + y * y)).max(0.0).sqrt()];
    Some(match m {
        DotMap::Hilo1 => [hi / 65535.0, lo / 65535.0, 1.0],
        DotMap::HiloHemisphereD3d => hemisphere((hi - 32768.0) / 32767.0, (lo - 32768.0) / 32767.0),
        DotMap::HiloHemisphereGl => hemisphere(
            (2.0 * twos(hi) + 1.0) / 65535.0,
            (2.0 * twos(lo) + 1.0) / 65535.0,
        ),
        DotMap::HiloHemisphere => hemisphere(twos(hi) / 32767.0, twos(lo) / 32767.0),
        _ => return None,
    })
}
