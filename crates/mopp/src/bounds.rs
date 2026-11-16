//! The box an existing program guarantees for each key it emits.
//!
//! A program may skip a subtree only when the query misses that subtree's
//! bounds, so every primitive lies inside every bound on the path to its
//! terminal. Intersecting the axis-aligned bounds on that path (bound
//! checks, both split forms, the 24-bit checks; diagonal splits are left
//! out, which only makes the box larger) gives a box the primitive is
//! inside. Used to carry keys we have no geometry for (instanced geometry
//! and the other non-structure key types) from a retail program into a
//! rebuilt one.

use crate::code::{Code, FULL_BITS, ROOT_SHIFT};
use crate::vm::{Aabb, Fault};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct State {
    pc: usize,
    shift: u32,
    offset: [i64; 3],
    key_offset: u32,
    lo: [i64; 3],
    hi: [i64; 3],
}

/// Every key the program can emit, with the world box outside which no
/// query reaches its terminal (the union over terminals, if a key has
/// several). It holds the primitive, and every query that returns the key.
pub fn key_bounds(code: &Code) -> Result<BTreeMap<u32, Aabb>, Fault> {
    let c = &code.bytecode;
    let top = (1i64 << FULL_BITS) - 1;
    let mut boxes: BTreeMap<u32, ([i64; 3], [i64; 3])> = BTreeMap::new();
    let mut stack = vec![State {
        pc: 0,
        shift: ROOT_SHIFT,
        offset: [0; 3],
        key_offset: 0,
        lo: [0; 3],
        hi: [top; 3],
    }];
    let byte = |at: usize| {
        c.get(at)
            .map(|&b| i64::from(b))
            .ok_or(Fault::OutOfCode { at })
    };
    let be = |at: usize, n: usize| -> Result<i64, Fault> {
        (0..n).try_fold(0i64, |v, i| Ok((v << 8) | byte(at + i)?))
    };
    // Full-precision range of frame coordinate `v` on an axis.
    let full_lo = |s: &State, a: usize, v: i64| (v + s.offset[a]) << s.shift;
    let full_hi = |s: &State, a: usize, v: i64| ((v + s.offset[a] + 1) << s.shift) - 1;
    let mut steps = 0usize;
    while let Some(mut s) = stack.pop() {
        loop {
            steps += 1;
            if steps > 64 * c.len() + 1024 {
                return Err(Fault::OutOfCode { at: s.pc });
            }
            let op = byte(s.pc)? as u8;
            match op {
                0x00 => break,
                0x01..=0x04 => {
                    let k = u32::from(op);
                    if k > s.shift {
                        return Err(Fault::Shift { at: s.pc });
                    }
                    for a in 0..3 {
                        s.offset[a] = (s.offset[a] + byte(s.pc + 1 + a)?) << k;
                    }
                    s.shift -= k;
                    s.pc += 4;
                }
                0x05 => s.pc += 2 + byte(s.pc + 1)? as usize,
                0x06 => s.pc += 3 + be(s.pc + 1, 2)? as usize,
                0x07 => s.pc += 4 + be(s.pc + 1, 3)? as usize,
                0x09 => {
                    s.key_offset = s.key_offset.wrapping_add(byte(s.pc + 1)? as u32);
                    s.pc += 2;
                }
                0x0a => {
                    s.key_offset = s.key_offset.wrapping_add(be(s.pc + 1, 2)? as u32);
                    s.pc += 3;
                }
                0x0b => {
                    s.key_offset = be(s.pc + 1, 4)? as u32;
                    s.pc += 5;
                }
                0x10..=0x12 | 0x20..=0x25 => {
                    let (a, low_max, high_min, first, second) = match op {
                        0x10..=0x12 => {
                            let j = byte(s.pc + 3)? as usize;
                            let a = usize::from(op - 0x10);
                            (a, byte(s.pc + 1)?, byte(s.pc + 2)?, s.pc + 4, s.pc + 4 + j)
                        }
                        0x20..=0x22 => {
                            let v = byte(s.pc + 1)?;
                            let j = byte(s.pc + 2)? as usize;
                            (usize::from(op - 0x20), v + 1, v, s.pc + 3, s.pc + 3 + j)
                        }
                        _ => {
                            let base = s.pc + 7;
                            let j1 = be(s.pc + 3, 2)? as usize;
                            let j2 = be(s.pc + 5, 2)? as usize;
                            (
                                usize::from(op - 0x23),
                                byte(s.pc + 1)?,
                                byte(s.pc + 2)?,
                                base + j1,
                                base + j2,
                            )
                        }
                    };
                    let mut low = s;
                    low.pc = first;
                    // The engine also enters the low child when the query
                    // ends below high_min, so a query there may lie in a
                    // gap above low_max (E 0x003003cd); the box covers that
                    // too, so it holds whatever the program can return.
                    low.hi[a] = low.hi[a].min(full_hi(&s, a, low_max.max(high_min - 1)));
                    stack.push(low);
                    s.lo[a] = s.lo[a].max(full_lo(&s, a, high_min));
                    s.pc = second;
                }
                0x13..=0x1c => {
                    let j = byte(s.pc + 3)? as usize;
                    let mut low = s;
                    low.pc = s.pc + 4;
                    stack.push(low);
                    s.pc += 4 + j;
                }
                0x26..=0x28 => {
                    let a = usize::from(op - 0x26);
                    let (l, h) = (byte(s.pc + 1)?, byte(s.pc + 2)?);
                    s.lo[a] = s.lo[a].max(full_lo(&s, a, l));
                    s.hi[a] = s.hi[a].min(full_hi(&s, a, h));
                    s.pc += 3;
                }
                0x29..=0x2b => {
                    let a = usize::from(op - 0x29);
                    s.lo[a] = s.lo[a].max(be(s.pc + 1, 3)?);
                    s.hi[a] = s.hi[a].min(be(s.pc + 4, 3)?);
                    s.pc += 7;
                }
                0x30..=0x53 => {
                    let k = match op {
                        0x30..=0x4f => i64::from(op - 0x30),
                        0x50 => byte(s.pc + 1)?,
                        0x51 => be(s.pc + 1, 2)?,
                        0x52 => be(s.pc + 1, 3)?,
                        _ => be(s.pc + 1, 4)?,
                    };
                    let key = s.key_offset.wrapping_add(k as u32);
                    let e = boxes.entry(key).or_insert((s.lo, s.hi));
                    for a in 0..3 {
                        e.0[a] = e.0[a].min(s.lo[a]);
                        e.1[a] = e.1[a].max(s.hi[a]);
                    }
                    break;
                }
                0x60 => s.pc += 2,
                0x64 => s.pc += 3,
                0x68 => s.pc += 5,
                0x61..=0x63 | 0x65..=0x67 | 0x69..=0x6b => {
                    return Err(Fault::Property { at: s.pc, op });
                }
                _ => return Err(Fault::BadOpcode { at: s.pc, op }),
            }
        }
    }
    let world = |a: usize, f: i64| f as f64 / f64::from(code.scale) + f64::from(code.offset[a]);
    Ok(boxes
        .into_iter()
        .map(|(k, (lo, hi))| {
            // One unit of slack each side absorbs the f32 round trip.
            let min = [0, 1, 2].map(|a| world(a, lo[a] - 1) as f32);
            let max = [0, 1, 2].map(|a| world(a, hi[a] + 2) as f32);
            (k, Aabb { min, max })
        })
        .collect())
}
