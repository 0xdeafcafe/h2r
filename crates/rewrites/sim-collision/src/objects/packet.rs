//! Four reference spheres from one list. Each lane uses that list's own
//! sum-of-squares order. Check each operand before executing packed FP so
//! replay of a wider-range case enters the scalar checked region intact.
use super::VisitQuery;
use sim_core::lanes::{self, Op, Vec4};

#[derive(Clone, Copy, Default)]
pub(super) struct Candidate {
    pub handle: u32,
    pub bounds: [u32; 4],
}

pub(super) fn overlaps(q: &VisitQuery, list: usize, refs: &[Candidate; 4]) -> Option<[bool; 4]> {
    let op = |op, a: Vec4, b: Vec4| lanes::ordinary(a, b).then(|| lanes::binary32(op, a, b));
    let component = |axis| refs.map(|r| r.bounds[axis]);
    let square = |axis: usize| {
        let d = op(Op::Sub, component(axis), [q.centre[axis]; 4])?;
        op(Op::Mul, d, d)
    };
    let (xx, yy, zz) = (square(0)?, square(1)?, square(2)?);
    let (first, last) = if list == 0 { (yy, xx) } else { (xx, yy) };
    let apart = op(Op::Add, op(Op::Add, zz, first)?, last)?;
    let reach = op(Op::Add, component(3), [q.radius; 4])?;
    let rr = op(Op::Mul, reach, reach)?;
    lanes::ordinary(rr, apart)
        .then(|| std::array::from_fn(|i| f32::from_bits(rr[i]) >= f32::from_bits(apart[i])))
}
