//! E1: world draws made in window pixels (a pass-through program, `mov
//! oPos, v0`) drawn into the overscanned world image so they land where the
//! world around them does (docs/game/high-framerate.md, "Screen-space draws
//! in the world").
//!
//! The world image shrinks the world by the overscan `s` about the screen's
//! centre (each program's viewport scale times `s`). A pass-through draw has
//! no viewport transform to scale: its vertices are already window pixels.
//! Halo 2 draws two kinds among its world draws, every gameplay frame:
//!
//! - **Sprites**: small quads the game placed on the screen by projecting a
//!   point of the world with frame N's camera (about 150 a frame on
//!   03a_oldmombasa, a few pixels each, depth-tested, blended). Their
//!   positions are scaled by `s` about the centre, as the world's are
//!   ([`Fit::Shrink`]): after the warp they are exactly where the guest drew
//!   them at the frame's own rotation, and turn with the world in between.
//! - **Full-target layers**: one quad over the whole target, blended over the
//!   world so far, its textures (a 640x480 target drawn that frame, noise
//!   sheets) looked up with coordinates its vertices carry. Shrunk, it would
//!   leave the band the overscan adds (what a turn reveals) without the
//!   layer. Instead the quad keeps covering the whole image and every other
//!   input is moved to the value it has at the point the shrunk image shows
//!   there ([`Fit::Extend`]): inside the guest's view the same texels as the
//!   guest's, in the band the layer's own coordinates carried on (clamped or
//!   wrapped as its samplers say). That needs a program whose outputs are its
//!   inputs moved (no constant, no arithmetic: then each output is affine in
//!   the inputs) and inputs that are affine over the quad's pixels, both
//!   checked; otherwise it is shrunk.
//!
//! Either needs the position's own vertex data to rewrite (a float
//! attribute at register 0), and a program in which the position reaches
//! `oPos` unchanged and nothing else: a program that also computes a
//! texture coordinate from it would look up a different texel once it
//! moved.

use super::reproject::Screen;
use crate::backend::{ResolvedDraw, StreamBytes};
use gfx::vertex::{VertexAttribute, VertexDataType};
use shaders::vsh::{IluOp, MacOp, OutDest, OutReg, Program, ProgramKind, SrcReg};

/// A pass-through program E1 can move ([`pass_through`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassThrough {
    /// Every output is an input moved: outputs are affine in the inputs.
    pub pure: bool,
}

/// How a pass-through world draw goes into the world image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Positions scaled by the overscan about the screen's centre.
    Shrink,
    /// Positions kept (the whole target), every other input moved outward.
    Extend,
}

/// Whether `microcode` passes `v0` to `oPos` unchanged (a `mov` of `v0`,
/// unswizzled, not negated, `x` and `y` at least) and reads `v0` nowhere
/// else; `None` otherwise.
pub fn pass_through(microcode: &[u32]) -> Option<PassThrough> {
    let p = Program::from_words(ProgramKind::Normal, microcode).ok()?;
    let mut pos = [false; 4];
    let mut pure = true;
    for inst in &p.instructions {
        let writes_pos = inst
            .out
            .is_some_and(|o| o.dest == OutDest::Output(OutReg::Pos));
        let reads_v0 = inst.sources().any(|s| s.reg == SrcReg::Input(0));
        if writes_pos || reads_v0 {
            // Only `mov oPos, v0` may touch either.
            let o = inst.out?;
            if !(writes_pos
                && inst.mac == MacOp::Mov
                && inst.ilu == IluOp::Nop
                && o.unit == shaders::vsh::Unit::Mac
                && !inst.mac_writes_temp()
                && inst.a.reg == SrcReg::Input(0)
                && !inst.a.neg
                && (0..4).all(|j| !o.mask.has(j) || inst.a.swizzle[j] as usize == j))
            {
                return None;
            }
            for (j, p) in pos.iter_mut().enumerate() {
                *p |= o.mask.has(j);
            }
        }
        let moves = |op_mac: bool| {
            if op_mac {
                matches!(inst.mac, MacOp::Nop | MacOp::Mov)
            } else {
                matches!(inst.ilu, IluOp::Nop | IluOp::Mov)
            }
        };
        pure &= moves(true)
            && moves(false)
            && !inst.mac_writes_temp()
            && !inst.ilu_writes_temp()
            && inst.sources().all(|s| matches!(s.reg, SrcReg::Input(_)))
            && inst
                .out
                .is_none_or(|o| matches!(o.dest, OutDest::Output(_)));
    }
    (pos[0] && pos[1]).then_some(PassThrough { pure })
}

/// One float attribute of `d`: where it is.
struct Attr {
    stream: usize,
    stride: usize,
    a: VertexAttribute,
}

fn floats(t: VertexDataType) -> Option<usize> {
    match t {
        VertexDataType::Float1 => Some(1),
        VertexDataType::Float2 => Some(2),
        VertexDataType::Float3 => Some(3),
        VertexDataType::Float4 => Some(4),
        _ => None,
    }
}

fn attrs(d: &ResolvedDraw) -> Vec<Attr> {
    let mut v = Vec::new();
    for (i, (layout, stream)) in d.layout.streams.iter().zip(&d.streams).enumerate() {
        let stride = if stream.stride > 0 {
            stream.stride
        } else {
            layout.stride
        } as usize;
        for a in &layout.attributes {
            v.push(Attr {
                stream: i,
                stride,
                a: *a,
            });
        }
    }
    v
}

fn bytes_of<'a>(d: &'a ResolvedDraw, at: &Attr, vertex: usize) -> Option<&'a [u8]> {
    let o = vertex * at.stride + at.a.offset as usize;
    d.streams[at.stream]
        .data
        .get(o..o + at.a.data_type.byte_size() as usize)
}

fn read_floats(b: &[u8], n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| f32::from_le_bytes(b[4 * i..4 * i + 4].try_into().expect("4 bytes")) as f64)
        .collect()
}

/// `values[k]` (one per vertex) as `a x + b y + c` over the vertices'
/// window positions, if it is one (within a thousandth of its size).
fn affine(xy: &[[f64; 2]], values: &[f64]) -> Option<[f64; 3]> {
    // Three vertices spanning the most area.
    let n = xy.len();
    let mut best = (0.0, [0, 0, 0]);
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let area = ((xy[j][0] - xy[i][0]) * (xy[k][1] - xy[i][1])
                    - (xy[k][0] - xy[i][0]) * (xy[j][1] - xy[i][1]))
                    .abs();
                if area > best.0 {
                    best = (area, [i, j, k]);
                }
            }
        }
    }
    let [i, j, k] = best.1;
    let det = (xy[j][0] - xy[i][0]) * (xy[k][1] - xy[i][1])
        - (xy[k][0] - xy[i][0]) * (xy[j][1] - xy[i][1]);
    if det.abs() < 1e-6 {
        return None;
    }
    let (dv1, dv2) = (values[j] - values[i], values[k] - values[i]);
    let a = (dv1 * (xy[k][1] - xy[i][1]) - dv2 * (xy[j][1] - xy[i][1])) / det;
    let b = (dv2 * (xy[j][0] - xy[i][0]) - dv1 * (xy[k][0] - xy[i][0])) / det;
    let c = values[i] - a * xy[i][0] - b * xy[i][1];
    let size = values.iter().fold(1.0f64, |m, v| m.max(v.abs()));
    xy.iter()
        .zip(values)
        .all(|(p, v)| (a * p[0] + b * p[1] + c - v).abs() <= 1e-3 * size)
        .then_some([a, b, c])
}

/// The streams pass-through world draw `d` (program `pt`) is drawn with
/// into the world image overscanned by `s` about `screen`'s centre, and
/// how; `None` when its positions cannot be rewritten.
pub fn world_image_streams(
    d: &ResolvedDraw,
    pt: PassThrough,
    s: f64,
    screen: &Screen,
) -> Option<(Vec<StreamBytes>, Fit)> {
    let all = attrs(d);
    let pos = all.iter().find(|a| a.a.register.0 == 0)?;
    let pn = floats(pos.a.data_type).filter(|&n| n >= 2)?;
    let n = d.vertex_count as usize;
    if n == 0 {
        return None;
    }
    let mut xy = Vec::with_capacity(n);
    let mut ws = Vec::with_capacity(n);
    for v in 0..n {
        let p = read_floats(bytes_of(d, pos, v)?, pn);
        if !p.iter().all(|x| x.is_finite()) {
            return None;
        }
        xy.push([p[0], p[1]]);
        ws.push(if pn == 4 { p[3] } else { 1.0 });
    }
    let c = screen.offset;
    let [w, h] = screen.size;
    let mut streams: Vec<Vec<u8>> = d.streams.iter().map(|s| s.data.to_vec()).collect();
    let mut put = |at: &Attr, v: usize, comp: usize, x: f64| {
        let o = v * at.stride + at.a.offset as usize + 4 * comp;
        streams[at.stream][o..o + 4].copy_from_slice(&(x as f32).to_le_bytes());
    };
    let (x0, y0, x1, y1) = xy.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |b, p| (b.0.min(p[0]), b.1.min(p[1]), b.2.max(p[0]), b.3.max(p[1])),
    );
    let covers = x0 <= 1.0 && y0 <= 1.0 && x1 >= w - 1.0 && y1 >= h - 1.0;
    let affine_w = ws
        .iter()
        .all(|x| (x - ws[0]).abs() <= 1e-6 * ws[0].abs().max(1.0));
    let extend = pt.pure && covers && affine_w && n >= 3 && {
        // Every other input affine over the pixels (float ones moved,
        // others the same on every vertex).
        let mut fits = Vec::new();
        let ok =
            all.iter()
                .filter(|a| a.a.register.0 != 0)
                .all(|at| match floats(at.a.data_type) {
                    Some(k) => {
                        let mut vals = Vec::with_capacity(n);
                        for v in 0..n {
                            let Some(b) = bytes_of(d, at, v) else {
                                return false;
                            };
                            vals.push(read_floats(b, k));
                        }
                        (0..k).all(|comp| {
                            let col: Vec<f64> = vals.iter().map(|x| x[comp]).collect();
                            match affine(&xy, &col) {
                                Some(f) => {
                                    fits.push((at, comp, f));
                                    true
                                }
                                None => false,
                            }
                        })
                    }
                    None => (0..n).all(|v| bytes_of(d, at, v) == bytes_of(d, at, 0)),
                });
        if ok {
            for (at, comp, [a, b, k]) in fits {
                for (v, p) in xy.iter().enumerate() {
                    // The guest's pixel the image shows at `p`.
                    let q = [c[0] + (p[0] - c[0]) / s, c[1] + (p[1] - c[1]) / s];
                    put(at, v, comp, a * q[0] + b * q[1] + k);
                }
            }
        }
        ok
    };
    if !extend {
        for (v, p) in xy.iter().enumerate() {
            put(pos, v, 0, c[0] + (p[0] - c[0]) * s);
            put(pos, v, 1, c[1] + (p[1] - c[1]) * s);
        }
    }
    let out = d
        .streams
        .iter()
        .zip(streams)
        .map(|(s, data)| StreamBytes {
            data: data.into(),
            stride: s.stride,
        })
        .collect();
    Some((out, if extend { Fit::Extend } else { Fit::Shrink }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::vsh::encode::{Inst, Operand as O};

    fn words(insts: &[Inst]) -> Vec<u32> {
        let n = insts.len();
        insts
            .iter()
            .enumerate()
            .flat_map(|(i, x)| {
                let x = if i + 1 == n {
                    x.clone().last()
                } else {
                    x.clone()
                };
                x.words()
            })
            .collect()
    }

    fn mov(src: O, out: OutReg) -> Inst {
        Inst::mac(MacOp::Mov, Some(src), None, None).to_out(false, out, 0xF)
    }

    #[test]
    fn pass_throughs_that_move_only_the_position() {
        // Halo 2's two: everything moved (pure), and one with texture
        // matrices (not pure, the position still only moved).
        let pure = words(&[mov(O::v(0), OutReg::Pos), mov(O::v(1), OutReg::T(0))]);
        assert_eq!(pass_through(&pure), Some(PassThrough { pure: true }));
        let matrices = words(&[
            mov(O::v(0), OutReg::Pos),
            Inst::mac(MacOp::Dp4, Some(O::v(1)), Some(O::c(18)), None).to_out(
                false,
                OutReg::T(0),
                8,
            ),
        ]);
        assert_eq!(pass_through(&matrices), Some(PassThrough { pure: false }));
        // The position also read for a texture coordinate: a moved
        // position would look up another texel.
        let lookup = words(&[mov(O::v(0), OutReg::Pos), mov(O::v(0), OutReg::T(0))]);
        assert_eq!(pass_through(&lookup), None);
        // A swizzled or negated position, or one through a constant.
        assert_eq!(
            pass_through(&words(&[mov(O::v(0).swz("yxzw"), OutReg::Pos)])),
            None
        );
        assert_eq!(
            pass_through(&words(&[mov(O::v(0).negated(), OutReg::Pos)])),
            None
        );
        assert_eq!(
            pass_through(&words(&[Inst::mac(
                MacOp::Dp4,
                Some(O::v(0)),
                Some(O::c(96)),
                None
            )
            .to_out(false, OutReg::Pos, 0xF)])),
            None
        );
    }

    #[test]
    fn affine_fits_exact_and_rejects_the_rest() {
        let xy = [[0.0, 0.0], [640.0, 0.0], [640.0, 480.0], [0.0, 480.0]];
        let f = affine(&xy, &[0.0, 1.0, 1.0, 0.0]).unwrap();
        assert!((f[0] - 1.0 / 640.0).abs() < 1e-12 && f[1].abs() < 1e-12 && f[2].abs() < 1e-12);
        assert_eq!(affine(&xy, &[0.0, 1.0, 0.0, 1.0]), None, "a saddle");
    }
}
