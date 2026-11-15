//! Reference CPU interpreter for vertex microcode.
//!
//! This is the semantic definition the WGSL translation is tested against:
//! every op here has a twin helper in the generated prelude (see
//! `wgsl.rs`), written to the same formula.

use super::*;

/// Bias added before flooring in `ARL`. Host attribute conversion can land a
/// hair under an integer (17/255*255 = 16.99998); xemu uses the same bias.
pub const ARL_BIAS: f32 = 0.001;
/// Smallest magnitude `RCC` returns (2^-64).
pub const RCC_MIN: f32 = f32::from_bits(0x1F80_0000);
/// Largest magnitude `RCC` returns (2^64).
pub const RCC_MAX: f32 = f32::from_bits(0x5F80_0000);

/// A 4-vector.
pub type Vec4 = [f32; 4];

/// Program outputs, indexed by [`OutReg::slot`]: pos, d0, d1, fog, pts, b0,
/// b1, t0..t3. Unwritten registers keep `(0, 0, 0, 1)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outputs {
    /// Raw register values (before the viewport fixup).
    pub regs: [Vec4; 11],
}

impl Outputs {
    /// One output register.
    pub fn get(&self, r: OutReg) -> Vec4 {
        self.regs[r.slot()]
    }
}

/// Multiply where zero times anything (including inf/NaN) is zero, as the
/// NV2A does.
pub fn mul(a: f32, b: f32) -> f32 {
    if a == 0.0 || b == 0.0 { 0.0 } else { a * b }
}

fn mul4(a: Vec4, b: Vec4) -> Vec4 {
    [0, 1, 2, 3].map(|i| mul(a[i], b[i]))
}

/// Clamp away from zero and infinity, keeping the sign (used by `RCC` and
/// for `oPos.w`). `+0` counts as positive.
pub fn clamp_away(t: f32) -> f32 {
    let r = t.abs().clamp(RCC_MIN, RCC_MAX);
    if t.is_sign_negative() { -r } else { r }
}

/// `RCC`: `1/x` clamped to `±[2^-64, 2^64]`, sign kept (`+0` gives `+2^64`,
/// `-0` gives `-2^64`). The saturation is decided on `|x|` before dividing,
/// so no infinity is ever produced (see the WGSL twin for why that matters).
pub fn rcc(x: f32) -> f32 {
    let t = x.abs();
    let r = if t < RCC_MIN {
        RCC_MAX
    } else if t > RCC_MAX {
        RCC_MIN
    } else {
        1.0 / t
    };
    if x.is_sign_negative() { -r } else { r }
}

/// `EXP` (NV_vertex_program): `(2^floor(x), x - floor(x), 2^x, 1)`, with
/// `y = 0` for `±inf` where the subtraction would give NaN.
pub fn exp(x: f32) -> Vec4 {
    if x.is_infinite() {
        let e = if x > 0.0 { f32::INFINITY } else { 0.0 };
        return [e, 0.0, e, 1.0];
    }
    let f = x.floor();
    [f.exp2(), x - f, x.exp2(), 1.0]
}

/// `LOG` (NV_vertex_program): `(exponent, mantissa in [1, 2), log2|x|, 1)`
/// of `|x|`, taken from the IEEE fields rather than `floor(log2)`, which
/// rounds up just below powers of two. Zero (and denormals, which the NV2A
/// flushes) give `(-inf, 1, -inf, 1)`; infinity gives `(inf, 1, inf, 1)`.
pub fn log(x: f32) -> Vec4 {
    let bits = x.to_bits() & 0x7FFF_FFFF;
    let e = bits >> 23;
    if e == 0 {
        return [f32::NEG_INFINITY, 1.0, f32::NEG_INFINITY, 1.0];
    }
    if bits == 0x7F80_0000 {
        return [f32::INFINITY, 1.0, f32::INFINITY, 1.0];
    }
    if e == 0xFF {
        return [f32::NAN, f32::NAN, f32::NAN, 1.0];
    }
    let mantissa = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);
    [
        (e as i32 - 127) as f32,
        mantissa,
        f32::from_bits(bits).log2(),
        1.0,
    ]
}

/// `ARL`: `a0.x = floor(x + 0.001)`. The result is clamped to +-1024 (NaN
/// gives 0) so that `a0 + index` cannot overflow; any value past that range
/// already saturates the `0..=191` clamp on relative reads, so the clamp
/// changes nothing observable.
pub fn arl(x: f32) -> i32 {
    (x + ARL_BIAS).floor().clamp(-1024.0, 1024.0) as i32
}

/// MAC unit result. `ARL` returns A unchanged (the caller takes `x`).
pub fn mac(op: MacOp, a: Vec4, b: Vec4, c: Vec4) -> Vec4 {
    match op {
        MacOp::Nop => [0.0; 4],
        MacOp::Mov | MacOp::Arl => a,
        MacOp::Mul => mul4(a, b),
        MacOp::Add => [0, 1, 2, 3].map(|i| a[i] + c[i]),
        MacOp::Mad => {
            let p = mul4(a, b);
            [0, 1, 2, 3].map(|i| p[i] + c[i])
        }
        MacOp::Dp3 => {
            let p = mul4(a, b);
            [p[0] + p[1] + p[2]; 4]
        }
        MacOp::Dph => {
            let p = mul4(a, b);
            [p[0] + p[1] + p[2] + b[3]; 4]
        }
        MacOp::Dp4 => {
            let p = mul4(a, b);
            [p[0] + p[1] + p[2] + p[3]; 4]
        }
        MacOp::Dst => [1.0, mul(a[1], b[1]), a[2], b[3]],
        MacOp::Min => [0, 1, 2, 3].map(|i| a[i].min(b[i])),
        MacOp::Max => [0, 1, 2, 3].map(|i| a[i].max(b[i])),
        MacOp::Slt => [0, 1, 2, 3].map(|i| if a[i] < b[i] { 1.0 } else { 0.0 }),
        MacOp::Sge => [0, 1, 2, 3].map(|i| if a[i] >= b[i] { 1.0 } else { 0.0 }),
    }
}

/// ILU unit result. Scalar ops use `c[0]` (the caller has applied the
/// swizzle, so that is the first selected component).
pub fn ilu(op: IluOp, c: Vec4) -> Vec4 {
    let x = c[0];
    match op {
        IluOp::Nop => [0.0; 4],
        IluOp::Mov => c,
        IluOp::Rcp => [1.0 / x; 4],
        IluOp::Rcc => [rcc(x); 4],
        IluOp::Rsq => [1.0 / x.abs().sqrt(); 4],
        IluOp::Exp => exp(x),
        IluOp::Log => log(x),
        IluOp::Lit => {
            let sx = c[0].max(0.0);
            let sy = c[1].max(0.0);
            let lim = 128.0 - 1.0 / 256.0;
            let p = c[3].clamp(-lim, lim);
            let z = if sx > 0.0 {
                (p * sy.log2()).exp2()
            } else {
                0.0
            };
            [1.0, sx, z, 1.0]
        }
    }
}

fn write(dst: &mut Vec4, val: Vec4, mask: Mask) {
    for i in 0..4 {
        if mask.has(i) {
            dst[i] = val[i];
        }
    }
}

/// Runs `program` on one vertex. `constants` is written by read/write
/// programs. Returns the raw output registers.
pub fn run(
    program: &Program,
    inputs: &[Vec4; INPUT_COUNT],
    constants: &mut [Vec4; CONSTANT_COUNT],
) -> Outputs {
    let mut r = [[0.0f32; 4]; TEMP_COUNT];
    let mut out = Outputs {
        regs: [[0.0, 0.0, 0.0, 1.0]; 11],
    };
    let mut a0: i32 = 0;
    for inst in &program.instructions {
        let read = |s: &Source, r: &[Vec4; TEMP_COUNT], out: &Outputs| -> Vec4 {
            let v = match s.reg {
                SrcReg::Temp(12) => out.regs[0],
                SrcReg::Temp(n) => r.get(n as usize).copied().unwrap_or([0.0; 4]),
                SrcReg::Input(n) => inputs[n as usize & 15],
                SrcReg::Const { index, relative } => {
                    let i = if relative {
                        (a0 + index as i32).clamp(0, CONSTANT_COUNT as i32 - 1)
                    } else {
                        index as i32
                    };
                    constants.get(i as usize).copied().unwrap_or([0.0; 4])
                }
                SrcReg::Unused => [0.0; 4],
            };
            let sw = s.swizzle.map(|c| v[c as usize & 3]);
            if s.neg { sw.map(|x| -x) } else { sw }
        };
        // Only the operands the units use are read: the others may name
        // registers that do not exist (decode only validates used ones).
        let (ra, rb, rc) = inst.mac_reads();
        let rc = rc || inst.ilu != IluOp::Nop;
        let zero = [0.0; 4];
        let a = if ra { read(&inst.a, &r, &out) } else { zero };
        let b = if rb { read(&inst.b, &r, &out) } else { zero };
        let mut c = if rc { read(&inst.c, &r, &out) } else { zero };
        let m = mac(inst.mac, a, b, c);
        if inst.ilu.is_scalar() {
            c = [c[0]; 4];
        }
        let l = ilu(inst.ilu, c);

        if inst.mac == MacOp::Arl {
            a0 = arl(m[0]);
        }
        let temp_write =
            |reg: u8, val: Vec4, mask: Mask, r: &mut [Vec4; TEMP_COUNT], out: &mut Outputs| {
                if reg == 12 {
                    write(&mut out.regs[0], val, mask);
                } else if let Some(dst) = r.get_mut(reg as usize) {
                    write(dst, val, mask);
                }
            };
        if inst.mac_writes_temp() {
            temp_write(inst.mac_temp, m, inst.mac_mask, &mut r, &mut out);
        }
        if inst.ilu_writes_temp() {
            temp_write(inst.ilu_temp, l, inst.ilu_mask, &mut r, &mut out);
        }
        if let Some(o) = inst.out {
            let val = if o.unit == Unit::Mac { m } else { l };
            match o.dest {
                OutDest::Output(OutReg::Fog) => out.regs[OutReg::Fog.slot()][0] = val[0],
                OutDest::Output(reg) => write(&mut out.regs[reg.slot()], val, o.mask),
                OutDest::Const(i) => {
                    if (i as usize) < CONSTANT_COUNT {
                        write(&mut constants[i as usize], val, o.mask)
                    }
                }
            }
        }
    }
    out
}

/// The viewport fixup the generated vertex stage applies to `oPos`, giving
/// wgpu clip coordinates. See `docs/shaders.md`.
pub fn fixup(pos: Vec4, surface: [f32; 2], z_max: f32) -> Vec4 {
    let x = (pos[0] * 16.0).trunc() / 16.0;
    let y = (pos[1] * 16.0).trunc() / 16.0;
    let w = clamp_away(pos[3]);
    let nx = 2.0 * x / surface[0] - 1.0;
    let ny = 1.0 - 2.0 * y / surface[1];
    // The depth clip range is inclusive: z == z_max stays at 1, and anything
    // past it stays above 1 (at least the float after 1) so it clips.
    let nz = if pos[2] > z_max {
        (pos[2] / z_max).max(f32::from_bits(0x3f80_0001))
    } else {
        (pos[2] / z_max).min(1.0)
    };
    [nx * w, ny * w, nz * w, w]
}
