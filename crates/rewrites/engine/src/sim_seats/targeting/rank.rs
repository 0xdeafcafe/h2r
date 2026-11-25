//! Ordering of the 0x60-byte target candidate records passed to qsort.
use super::*;

struct Ranking {
    weights: [u32; 2],
    math: Sse,
}

impl Ranking {
    // A missing candidate flag applies the preference penalty once, in order.
    fn penalize(&mut self, g: &impl Memory, pair: [u32; 2], preferences: u32, penalty: u32) -> Result<(), GuestFault> {
        for (preference, flag) in [(0x20, 1), (0x10, 2)] {
            if preferences & preference != 0 {
                for (score, addr) in self.weights.iter_mut().zip(pair) {
                    if g.u8(at(addr, 0x58))? & flag == 0 {
                        *score = self.math.sub(*score, penalty).map_err(fp_error)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn greater(&mut self, c: &mut CpuAbi, a: u32, b: u32) -> Result<bool, GuestFault> {
        let bits = self.math.comiss(a, b).map_err(fp_error)?;
        c.eflags = (c.eflags & !ARITH) | bits;
        c.eflags_defined |= ARITH;
        Ok(bits & 0x41 == 0)
    }
}

fn compare_records(c: &mut CpuAbi, g: &mut impl Memory, _: u32) -> Transfer {
    c.edx = g.u32(at(c.esp, 4))?;
    c.ecx = g.u32(at(c.esp, 8))?;
    let pair = [c.edx, c.ecx];
    let mut rank = Ranking {
        weights: [g.u32(at(pair[0], 0x4c))?, g.u32(at(pair[1], 0x4c))?],
        math: Sse { mxcsr: c.mxcsr },
    };
    push(c, g, c.ebx)?;
    push(c, g, c.esi)?;
    let params = g.u32(QUERY_PARAMS)?;
    if params != 0 {
        let preferences = g.u32(at(params, 0x14))?;
        let penalty = g.u32(0x45dbbc)?;
        scalar(c, 2, penalty);
        rank.penalize(g, pair, preferences, penalty)?;
    }
    let [lhs, rhs] = rank.weights;
    scalar(c, 1, lhs);
    scalar(c, 0, rhs);

    c.eax = 1;
    let mut left_first = rank.greater(c, lhs, rhs)?;
    let mut decided = left_first || rank.greater(c, rhs, lhs)?;
    // Larger values of the first two fields win; smaller values win in the
    // last two. Unordered comparisons fall through to the next field.
    for (offset, descending) in [(0x50, true), (0x54, true), (0x44, false), (0x48, false)] {
        if decided { break; }
        let [a, b] = if descending { pair } else { [pair[1], pair[0]] };
        let (a, b) = (g.u32(at(a, offset))?, g.u32(at(b, offset))?);
        scalar(c, 0, a);
        left_first = rank.greater(c, a, b)?;
        if left_first { break; }
        scalar(c, 0, b);
        decided = rank.greater(c, b, a)?;
    }
    if !left_first && !decided {
        let (a, b) = (g.u8(at(pair[0], 0x28))?, g.u8(at(pair[1], 0x28))?);
        left_first = a == 0 && b != 0;
        if a != 0 && b == 0 {
            logic(c, 0, 0x80);
        } else if !left_first {
            c.eax = g.u32(pair[0])? & 0xffff;
            c.ecx = g.u32(pair[1])? & 0xffff;
            c.eax = arithmetic(c, c.eax, c.ecx, true);
        }
    }
    if left_first {
        c.eax = u32::MAX;
        logic(c, c.eax, 1 << 31);
    }
    c.mxcsr = rank.math.mxcsr;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0)
}

pub fn compare(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<2, _>(c, g, pc, COMPARE..0x1a67e5, &[], |c, p, pc| compare_records(c, p, pc))
}
