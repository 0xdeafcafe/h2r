//! Billboard vertex program for H1 particles with an independently animated mask.
//! The particle callback selects declaration 7 or 8 from prt3 +4.
use shaders::vsh::{
    IluOp, MacOp, OutReg,
    encode::{self, Inst, Operand},
};
use tags::defs::{self, Val};

use super::{children, def, encode as encode_tag};
use crate::{Error, Result, shad::set};
use map::build::Block;

use Operand as O;

fn mov(a: O) -> Inst {
    Inst::mac(MacOp::Mov, Some(a), None, None)
}
fn add(a: O, b: O) -> Inst {
    Inst::mac(MacOp::Add, Some(a), None, Some(b))
}
fn mad(a: O, b: O, c: O) -> Inst {
    Inst::mac(MacOp::Mad, Some(a), Some(b), Some(c))
}
fn binary(op: MacOp, a: O, b: O) -> Inst {
    Inst::mac(op, Some(a), Some(b), None)
}

const XYZ: u8 = 14;

fn program(projected: bool, direction_aligned: bool) -> Vec<u8> {
    use MacOp::{Dph, Max, Min, Mul};
    let (v, r, c) = (O::v, O::r, O::c);
    let one = c(4).swz("w");
    let zero = c(15).swz("y");
    let mut code = Vec::new();
    for row in 0..3 {
        code.push(binary(Dph, v(4), c(174 + row)).to_temp(0, 8 >> row));
    }
    // Vertex 6 is the sprite's registration rectangle; v5 supplies size
    // and angle. H2 supplies interpolated sine/cosine pairs at c[a0+96].
    code.push(mov(v(0)).to_temp(10, 12));
    code.push(mad(r(10), v(6).swz("zw"), v(6).swz("xy")).to_temp(10, 12));
    code.push(binary(Mul, r(10), v(5).swz("x")).to_temp(10, 12));
    code.push(binary(Mul, v(5).swz("y"), c(148).swz("w")).to_temp(9, 1));
    code.push(Inst::ilu(IluOp::Exp, r(9).swz("w")).ilu_to_temp(1, 4));
    code.push(mad(c(148).swz("y"), r(1).swz("y"), c(148).swz("x")).to_temp(9, 1));
    code.push(Inst::mac(MacOp::Arl, Some(r(9).swz("w")), None, None));
    code.push(Inst::ilu(IluOp::Exp, r(9).swz("w")).ilu_to_temp(1, 4));
    let table = O::c_rel(96);
    code.push(mad(table.swz("zw"), r(1).swz("y"), table.swz("xy")).to_temp(9, 12));
    // right = corner.x*cos - corner.y*sin; up = corner.x*sin + corner.y*cos.
    code.push(binary(Mul, r(10).swz("x"), r(9).swz("yx")).to_temp(11, 12));
    code.push(mad(r(10).swz("y"), r(9).swz("x").negated(), r(11).swz("x")).to_temp(11, 8));
    code.push(mad(r(10).swz("y"), r(9).swz("y"), r(11).swz("y")).to_temp(11, 4));
    if direction_aligned {
        // H1 0x59291: the first axis follows particle velocity. The
        // other is perpendicular to it and the direction from the eye.
        code.push(binary(MacOp::Dp3, v(1), v(1)).to_temp(6, 1));
        code.push(Inst::ilu(IluOp::Rsq, r(6).swz("w")).ilu_to_temp(1, 1));
        code.push(binary(Mul, v(1), r(1).swz("w")).to_temp(6, XYZ));
        code.push(add(r(0), c(7).negated()).to_temp(10, XYZ));
        code.push(binary(Mul, r(10).swz("yzx"), r(6).swz("zxy")).to_temp(7, XYZ));
        code.push(mad(r(6).swz("yzx").negated(), r(10).swz("zxy"), r(7)).to_temp(7, XYZ));
        code.push(binary(MacOp::Dp3, r(7), r(7)).to_temp(7, 1));
        code.push(Inst::ilu(IluOp::Rsq, r(7).swz("w")).ilu_to_temp(1, 1));
        code.push(binary(Mul, r(7), r(1).swz("w")).to_temp(7, XYZ));
        code.push(mad(r(11).swz("x"), r(6), r(0)).to_temp(0, XYZ));
        code.push(mad(r(11).swz("y"), r(7), r(0)).to_temp(0, XYZ));
    } else {
        code.push(mad(r(11).swz("x"), c(5), r(0)).to_temp(0, XYZ));
        code.push(mad(r(11).swz("y"), c(4), r(0)).to_temp(0, XYZ));
    }
    for (axis, row) in [13, 14, 12].into_iter().enumerate() {
        code.push(binary(Dph, r(0), c(row)).to_temp(8, 8 >> axis));
    }
    code.push(add(one, r(8).negated()).to_temp(8, XYZ));
    code.push(binary(Max, r(8), zero).to_temp(8, XYZ));
    code.push(binary(Mul, r(8), r(8)).to_temp(8, XYZ));
    code.push(binary(Min, r(8), one).to_temp(8, XYZ));
    code.push(add(r(8).swz("x"), r(8).swz("y")).to_temp(8, 8));
    code.push(binary(Min, r(8).swz("x"), one).to_temp(8, 8));
    code.push(add(one, r(8).negated()).to_temp(8, XYZ));
    code.push(binary(Mul, r(8), r(8)).to_temp(8, XYZ));
    code.push(add(r(8).swz("y"), r(8).swz("x").negated()).to_temp(8, 4));
    code.push(mad(c(15).swz("x"), r(8).swz("y"), r(8).swz("x")).to_temp(8, 1));
    code.push(binary(Mul, r(8), c(15)).to_temp(8, 3));
    code.push(add(one, r(8).negated()).to_temp(8, 3));
    code.push(binary(Mul, r(8).swz("z"), r(8).swz("w")).to_temp(8, 1));

    for colour in [OutReg::D1, OutReg::B1] {
        code.push(mov(v(8).swz("zyx")).to_out(false, colour, XYZ));
        code.push(binary(Mul, v(8).swz("w"), r(8).swz("w")).to_out(false, colour, 1));
    }
    for row in 0..4 {
        code.push(binary(Dph, r(0), c(row)).to_out(false, OutReg::Pos, 8 >> row));
    }
    code.push(mov(v(0)).to_temp(11, 12));
    code.push(mad(r(11), v(3).swz("zw"), v(3).swz("xy")).to_temp(11, 12));
    code.push(mad(r(11), c(18).swz("xy"), c(18).swz("zw")).to_out(false, OutReg::T(0), 12));
    code.push(mov(one).to_out(false, OutReg::T(0), 1));
    // H1 0x56dec selects mesh UVs or clip-space XY/W. Add the offset
    // before interpolation; projective sampling divides by clip W.
    if projected {
        code.push(binary(Mul, r(12), c(19).swz("xy")).to_temp(11, 12));
        code.push(mad(r(12).swz("w"), c(19).swz("zw"), r(11)).to_out(false, OutReg::T(1), 12));
        code.push(mov(r(12).swz("w")).to_out(false, OutReg::T(1), 1));
    } else {
        code.push(mad(r(11), c(19).swz("xy"), c(19).swz("zw")).to_out(false, OutReg::T(1), 12));
        code.push(mov(one).to_out(false, OutReg::T(1), 1));
    }
    code.push(Inst::ilu(IluOp::Rcc, r(12).swz("w")).ilu_to_temp(1, 1));
    code.push(binary(Mul, r(12), c(10)).to_out(false, OutReg::Pos, XYZ));
    code.push(mad(r(12), r(1).swz("w"), c(11)).to_out(false, OutReg::Pos, XYZ));
    encode::program(&code)
}

/// Declaration indices 0..6 are mesh formats. The two particle orientations select 7 and 8.
pub fn build(projected: bool) -> Result<Block> {
    let d = def(b"vrtx");
    let row = d
        .child("programs")
        .ok_or(Error::Invalid("vertex programs definition"))?;
    let mut root = defs::default_record(d);
    set(
        &mut root,
        "unknown_0",
        Val::Bytes(1u32.to_le_bytes().to_vec()),
    )?;
    let mut rows = vec![defs::default_record(row); 7];
    for aligned in [false, true] {
        let mut entry = defs::default_record(row);
        children(
            &mut entry,
            row,
            "block_4",
            [0u16, 1, 2, 3, 4, 5, 6, 7, 9, 14]
                .into_iter()
                .map(|stream| vec![("unknown_0", Val::Bytes(stream.to_le_bytes().to_vec()))]),
        )?;
        set(
            &mut entry,
            "compiled_shader",
            Val::Data(program(projected, aligned)),
        )?;
        rows.push(entry);
    }
    set(&mut root, "programs", Val::Block(rows))?;
    encode_tag(d, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::vsh::{CONSTANT_COUNT, INPUT_COUNT, Program, interp};

    #[test]
    fn independent_mask_coordinates_use_the_sprite_or_perspective_divide() {
        let mut v = [[0.; 4]; INPUT_COUNT];
        v[0] = [1., 0., 0., 1.];
        v[3] = [0.25, 0.125, 0.25, 0.5];
        v[4] = [0.75, -0.5, 0., 1.];
        v[8] = [0.25, 0.5, 1., 0.75];
        let mut c = [[0.; 4]; CONSTANT_COUNT];
        for i in 0..3 {
            c[i][i] = 1.;
            c[174 + i][i] = 1.;
        }
        c[3][3] = 2.;
        c[4][3] = 1.;
        c[10] = [1.; 4];
        c[18] = [1., 1., 0., 0.];
        c[19] = [1., 1., 0.25, 0.5];
        for projected in [false, true] {
            let p = Program::parse(&program(projected, false)).unwrap();
            let out = interp::run(&p, &v, &mut c);
            assert_eq!(&out.get(OutReg::T(0))[..2], &[0.5, 0.125]);
            let t = out.get(OutReg::T(1));
            assert_eq!(
                [t[0] / t[3], t[1] / t[3]],
                if projected {
                    [0.625, 0.25]
                } else {
                    [0.75, 0.625]
                }
            );
            assert_eq!(out.get(OutReg::D1), [1., 0.5, 0.25, 0.75]);
        }
        // With the same centre and registration rectangle, a nonzero
        // corner on a directional shard follows the supplied velocity.
        v[1] = [0., 1., 0., 0.];
        v[5][0] = 1.;
        v[6] = [0., 0., 1., 1.];
        c[96] = [0., 1., 0., 0.];
        let program = Program::parse(&program(false, true)).unwrap();
        let out = interp::run(&program, &v, &mut c);
        assert_eq!(&out.get(OutReg::Pos)[..2], &[0.375, 0.25]);
    }
}
