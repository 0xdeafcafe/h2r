//! Static validation of a program by the engine's rules: every reachable
//! path ends in `0x00` or a terminal, uses no opcode the engine asserts on
//! or that overruns its state, stays inside the bytecode, and keeps the
//! shift non-negative.

use crate::vm::Fault;
use std::collections::HashSet;

/// What a full walk of every branch found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Instructions executed per opcode over the walk (each reachable
    /// `(pc, key offset, shift)` once).
    pub histogram: Vec<u32>,
    /// Every key a terminal can emit.
    pub keys: Vec<u32>,
    /// Bytes of bytecode never reached.
    pub unreached: usize,
    pub max_depth: usize,
    /// One past the last byte any instruction reads.
    pub end: usize,
}

/// Walk every branch of `code`.
pub fn validate(code: &[u8]) -> Result<Report, Fault> {
    let mut r = Report {
        histogram: vec![0; 256],
        ..Report::default()
    };
    let mut seen = HashSet::new();
    let mut reached = vec![false; code.len()];
    let mut stack = vec![(0usize, 0u32, crate::code::ROOT_SHIFT, 0usize)];
    while let Some((mut pc, mut ko, mut shift, depth)) = stack.pop() {
        r.max_depth = r.max_depth.max(depth);
        loop {
            if !seen.insert((pc, ko, shift)) {
                break;
            }
            let op = *code.get(pc).ok_or(Fault::OutOfCode { at: pc })?;
            let len = match op {
                0x00 => 1,
                0x01..=0x04 => 4,
                0x05 | 0x09 | 0x60 => 2,
                0x06 | 0x0a | 0x20..=0x22 | 0x26..=0x28 | 0x64 => 3,
                0x07 | 0x10..=0x1c => 4,
                0x0b | 0x68 => 5,
                0x23..=0x25 | 0x29..=0x2b => 7,
                0x30..=0x4f => 1,
                0x50 => 2,
                0x51 => 3,
                0x52 => 4,
                0x53 => 5,
                0x61..=0x63 | 0x65..=0x67 | 0x69..=0x6b => {
                    return Err(Fault::Property { at: pc, op });
                }
                _ => return Err(Fault::BadOpcode { at: pc, op }),
            };
            if pc + len > code.len() {
                return Err(Fault::OutOfCode { at: pc });
            }
            reached[pc..pc + len].iter_mut().for_each(|b| *b = true);
            r.histogram[usize::from(op)] += 1;
            let b = |i: usize| u32::from(code[pc + i]);
            let be = |i: usize, n: usize| (0..n).fold(0u32, |v, k| (v << 8) | b(i + k));
            let next = pc + len;
            match op {
                0x00 => break,
                0x01..=0x04 => {
                    let k = u32::from(op);
                    if k > shift {
                        return Err(Fault::Shift { at: pc });
                    }
                    shift -= k;
                    pc = next;
                }
                0x05 => pc = next + b(1) as usize,
                0x06 => pc = next + be(1, 2) as usize,
                0x07 => pc = next + be(1, 3) as usize,
                0x09 => {
                    ko = ko.wrapping_add(b(1));
                    pc = next;
                }
                0x0a => {
                    ko = ko.wrapping_add(be(1, 2));
                    pc = next;
                }
                0x0b => {
                    ko = be(1, 4);
                    pc = next;
                }
                0x10..=0x1c => {
                    stack.push((next, ko, shift, depth + 1));
                    pc = next + b(3) as usize;
                }
                0x20..=0x22 => {
                    stack.push((next, ko, shift, depth + 1));
                    pc = next + b(2) as usize;
                }
                0x23..=0x25 => {
                    stack.push((next + be(3, 2) as usize, ko, shift, depth + 1));
                    pc = next + be(5, 2) as usize;
                }
                0x26..=0x2b | 0x60 | 0x64 | 0x68 => pc = next,
                0x30..=0x53 => {
                    let key = match op {
                        0x30..=0x4f => u32::from(op - 0x30),
                        0x50 => b(1),
                        0x51 => be(1, 2),
                        0x52 => be(1, 3),
                        _ => be(1, 4),
                    };
                    r.keys.push(ko.wrapping_add(key));
                    break;
                }
                _ => unreachable!(),
            }
        }
    }
    r.keys.sort_unstable();
    r.keys.dedup();
    r.end = reached.iter().rposition(|b| *b).map_or(0, |i| i + 1);
    r.unreached = reached.iter().filter(|b| !**b).count();
    Ok(r)
}
