//! Author the field vertex program against the seven H2 Xbox mesh declarations.
//! Its colour output is H1's two-sided facing term times fog at each vertex.
use shaders::vsh::{
    IluOp, MacOp, OutReg,
    encode::{self, Inst, Operand},
};
use tags::defs::{self, Val};

use super::super::{children, def, encode as encode_tag};
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

const COMPONENT: [&str; 4] = ["x", "y", "z", "w"];
const XYZ: u8 = 14;

fn program(format: usize) -> Vec<u8> {
    use MacOp::{Dp3, Dph, Max, Min, Mul};
    let (v, r, c) = (O::v, O::r, O::c);
    let one = c(4).swz("w");
    let zero = c(15).swz("y");
    let uv = match format {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 3,
    };
    let mut code = Vec::new();

    // Position in r0, normal in r2. The world declaration already supplies
    // these in world space. Model declarations decompress and pose them.
    if format == 0 {
        code.push(mov(v(0)).to_temp(0, XYZ));
        code.push(mov(v(uv + 1)).to_temp(2, XYZ));
    } else {
        code.push(binary(Mul, v(0), c(170)).to_temp(10, XYZ));
        code.push(add(r(10), c(171)).to_temp(10, XYZ));
        let nodes = format.saturating_sub(2).max(1);
        if format == 1 {
            for row in 0..3 {
                code.push(mov(c(50 + row)).to_temp(4 + row, 15));
            }
        } else {
            // v1 indexes node transforms, v2 holds the trailing weights.
            // The leading weight is one minus their sum.
            code.push(binary(Mul, v(1), c(7).swz("w")).to_temp(3, 15));
            if nodes > 1 {
                code.push(mov(one).to_temp(9, 8));
                for channel in &COMPONENT[1..nodes] {
                    code.push(add(r(9).swz("x"), v(2).swz(channel).negated()).to_temp(9, 8));
                }
            }
            for node in (0..nodes).rev() {
                code.push(Inst::mac(
                    MacOp::Arl,
                    Some(r(3).swz(COMPONENT[node])),
                    None,
                    None,
                ));
                let weight = if node == 0 {
                    r(9).swz("x")
                } else {
                    v(2).swz(COMPONENT[node])
                };
                for row in 0..3 {
                    let matrix = O::c_rel(50 + row);
                    let op = if nodes == 1 {
                        mov(matrix)
                    } else if node == nodes - 1 {
                        binary(Mul, weight, matrix)
                    } else {
                        mad(weight, matrix, r(4 + row))
                    };
                    code.push(op.to_temp(4 + row, 15));
                }
            }
        }
        for row in 0..3 {
            code.push(binary(Dph, r(10), r(4 + row)).to_temp(0, 8 >> row));
            code.push(binary(Dp3, v(uv + 1), r(4 + row)).to_temp(2, 8 >> row));
        }
    }

    // Four independent rows of UV pairs, after the native mesh UV decompression.
    code.push(mad(v(uv).swz("xy"), c(172).swz("xy"), c(172).swz("zw")).to_temp(9, 12));
    code.push(mov(zero).to_temp(9, 2));
    for stage in 0..4 {
        for axis in 0..2 {
            code.push(binary(Dph, r(9), c(18 + 2 * stage + axis)).to_out(
                false,
                OutReg::T(stage),
                8 >> axis,
            ));
        }
    }

    // Perspective clip W is view depth. Orthographic clip W is constant,
    // so use its depth row instead. Both faces use abs(N dot forward).
    code.push(binary(Dp3, c(3), c(3)).to_temp(3, 8));
    code.push(binary(MacOp::Slt, zero, r(3).swz("x")).to_temp(3, 8));
    code.push(mov(c(3)).to_temp(5, XYZ));
    code.push(add(r(5), c(2).negated()).to_temp(5, XYZ));
    code.push(mad(r(3).swz("x"), r(5), c(2)).to_temp(5, XYZ));
    code.push(binary(Dp3, r(2), r(5)).to_temp(3, 8));
    code.push(binary(Max, r(3).swz("x"), r(3).swz("x").negated()).to_temp(3, 8));
    code.push(binary(Dp3, r(2), r(2)).to_temp(3, 4));
    code.push(binary(Dp3, r(5), r(5)).to_temp(3, 2));
    code.push(binary(Mul, r(3).swz("y"), r(3).swz("z")).to_temp(3, 4));
    code.push(Inst::ilu(IluOp::Rsq, r(3).swz("y")).ilu_to_temp(3, 4));
    code.push(binary(Mul, r(3).swz("x"), r(3).swz("y")).to_temp(3, 8));
    code.push(binary(Min, r(3).swz("x"), one).to_temp(3, 8));

    // H2 transparent layer fog: its two regional terms and sky term. Apply
    // visibility before interpolation, as H1 does when writing oD1.rgb.
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
    for colour in [OutReg::D0, OutReg::B0] {
        code.push(binary(Mul, r(3).swz("x"), r(8).swz("w")).to_out(false, colour, 1));
    }

    for axis in 0..4 {
        code.push(binary(Dph, r(0), c(axis)).to_out(false, OutReg::Pos, 8 >> axis));
    }
    code.push(Inst::ilu(IluOp::Rcc, r(12).swz("w")).ilu_to_temp(1, 1));
    code.push(binary(Mul, r(12), c(10)).to_out(false, OutReg::Pos, XYZ));
    code.push(mad(r(12), r(1).swz("w"), c(11)).to_out(false, OutReg::Pos, XYZ));
    encode::program(&code)
}

pub fn build() -> Result<Block> {
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
    let mut rows = Vec::new();
    for format in 0..7 {
        let mut streams = vec![0u16];
        if format >= 2 {
            streams.push(1);
        }
        if format >= 4 {
            streams.push(2);
        }
        streams.extend(3..=6);
        let mut entry = defs::default_record(row);
        children(
            &mut entry,
            row,
            "block_4",
            streams
                .into_iter()
                .map(|stream| vec![("unknown_0", Val::Bytes(stream.to_le_bytes().to_vec()))]),
        )?;
        set(&mut entry, "compiled_shader", Val::Data(program(format)))?;
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
    fn seven_mesh_declarations_preserve_four_uv_pairs_and_two_sided_fade() {
        for format in 0..7 {
            let p = Program::parse(&program(format)).unwrap();
            let uv = match format {
                0 | 1 => 1,
                2 | 3 => 2,
                _ => 3,
            };
            let mut inputs = [[0.0; 4]; INPUT_COUNT];
            let mut pose = [[0.0; 4]; CONSTANT_COUNT];
            for i in 0..4 {
                pose[i][i] = 1.0;
            }
            for i in 0..3 {
                pose[50 + i][i] = 1.0;
            }
            pose[4][3] = 1.0;
            pose[7][3] = 3.0;
            pose[10] = [1.0; 4];
            pose[170] = [1.0; 4];
            pose[172] = [2.0, 0.5, 0.5, 0.25];
            // All fog planes at one, both native fog amounts half opaque.
            for row in 12..=14 {
                pose[row][3] = 1.0;
            }
            pose[15] = [0.0, 0.0, 0.5, 0.5];
            for s in 0..4 {
                pose[18 + s * 2] = [1.0, 0.0, 0.0, s as f32 * 0.25];
                pose[19 + s * 2] = [0.0, 1.0, 0.0, s as f32 * -0.5];
            }
            inputs[0] = [0.25, -0.5, 0.5, 1.0];
            if format >= 4 {
                inputs[2] = [0.0, 0.1, 0.2, 0.3];
            }
            inputs[uv] = [0.25, 0.5, 0.0, 1.0];
            for (n, angle) in [
                ([0.0, 0.0, 1.0, 0.0], 1.0),
                ([0.0, 0.0, -1.0, 0.0], 1.0),
                ([0.8, 0.0, -0.6, 0.0], 0.6),
                ([1.0, 0.0, 0.0, 0.0], 0.0),
            ] {
                inputs[uv + 1] = n;
                let out = interp::run(&p, &inputs, &mut pose);
                assert!(
                    (out.get(OutReg::D0)[3] - angle * 0.25).abs() < 1e-5,
                    "format {format} normal {n:?}"
                );
                for stage in 0..4 {
                    let t = out.get(OutReg::T(stage));
                    assert_eq!(
                        &t[..2],
                        &[1.0 + stage as f32 * 0.25, 0.5 - stage as f32 * 0.5]
                    );
                }
                for i in 0..4 {
                    assert!((out.get(OutReg::Pos)[i] - inputs[0][i]).abs() < 1e-5);
                }
            }
        }
    }
}
