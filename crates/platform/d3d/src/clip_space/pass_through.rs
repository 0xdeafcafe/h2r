//! Pass-through vertex programs (`mov oPos, v0`): draws whose vertices are
//! already window pixels. The carried draws' camera classification
//! (`crate::scene_sink::camera`) labels them screen-space, so a display
//! frame between ticks draws them as the tick did.

use shaders::vsh::{IluOp, MacOp, OutDest, OutReg, Program, ProgramKind, SrcReg};

/// A pass-through program ([`pass_through`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassThrough {
    /// Every output is an input moved: outputs are affine in the inputs.
    pub pure: bool,
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
}
