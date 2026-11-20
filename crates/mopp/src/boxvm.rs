//! The engine's box query: `0x00300150` (setup) and `0x002ff8b0` (the
//! interpreter), bit for bit.
//!
//! This is the query that keeps a body's contacts with the structure: the
//! BV-tree agent's collide (E `0x002ea530` -> `0x002e9d90`) takes the other
//! body's AABB (E `0x002e9140`: its shape's box at the tolerance, grown by
//! its linear motion), calls the shape's slot `+0x30` (E `0x002df640`, or
//! the structure shape's override `0x001eea50`, which calls it), and keeps
//! one child agent per key returned. A key missing from one tick's answer
//! drops that surface's contact points (E `0x001d06c0`), which is how a
//! biped's ground contact list (physics component `+0x88`, filled by the
//! contact callback `0x00277b80` through `0x001d0400`) loses a surface.
//!
//! It differs from the sphere query ([`crate::query_sphere`]) in its
//! tests. The box is held per frame as `L = (lo >> s) - offset` and
//! `H = (hi >> s) + 1 - offset`, from full-precision `lo = round(min) - 1`
//! and `hi = round(max) - 1` (E `0x00300167..0x0030021a`), and:
//!
//! - two-plane splits (`0x10..0x1c`, `0x23..0x25`) visit the low child
//!   only when `L < low_max` and the high child only when `H > high_min`,
//!   so a box in the gap between them visits neither;
//! - one-plane splits (`0x20..0x22`) visit the low child when `L <= split`
//!   and the high child when `H > split`;
//! - bound checks (`0x26..0x28`) end the path when `H < min` or
//!   `L >= max`.
//!
//! So `low_max` and a bound check's `max` are exclusive here: a box whose
//! low corner lies in the same frame cell as a primitive's top is cut off
//! from it.

use crate::code::{Code, ROOT_SHIFT};
use crate::vm::{Aabb, Fault};

/// x87 `fistp` with the default rounding (to nearest, ties to even) and
/// the invalid operation masked: out of range gives the integer
/// indefinite.
fn fistp(v: f32) -> i32 {
    let r = v.round_ties_even();
    if r.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&r) {
        i32::MIN
    } else {
        r as i32
    }
}

/// The query struct `0x00300150` fills: full-precision corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Query {
    /// `+0x20..+0x28`: `round((min - offset) * scale) - 1`.
    lo: [i32; 3],
    /// `+0x10..+0x18`: `round((max - offset) * scale) - 1`.
    hi: [i32; 3],
}

impl Query {
    fn new(code: &Code, aabb: &Aabb) -> Self {
        // The product is stored to an f32 before fistp (E 0x0030016f), as
        // in the sphere setup; f64 holds the difference and product
        // exactly, so the one f32 rounding matches.
        let full = |a: usize, p: f32| {
            let v = ((f64::from(p) - f64::from(code.offset[a])) * f64::from(code.scale)) as f32;
            fistp(v).wrapping_sub(1)
        };
        Query {
            lo: [0, 1, 2].map(|a| full(a, aabb.min[a])),
            hi: [0, 1, 2].map(|a| full(a, aabb.max[a])),
        }
    }
}

/// The interpreter's 0x38-byte state (E `0x002ffc2d..0x002ffcbd`).
#[derive(Clone, Copy, Debug)]
struct State {
    /// `+0x00..+0x08`: `(hi >> s) + 1 - offset`.
    h: [i64; 3],
    /// `+0x10..+0x18`: `(lo >> s) - offset`.
    l: [i64; 3],
    /// `+0x20..+0x28`.
    offset: [i64; 3],
    /// `+0x2c`.
    key_offset: u32,
    /// `+0x30`: bits taken by rescales; the frame shift is `16 - bits`.
    bits: u32,
    /// `+0x34`.
    property: u32,
}

impl State {
    fn frame(q: &Query, offset: [i64; 3], bits: u32, key_offset: u32, property: u32) -> Self {
        let s = ROOT_SHIFT - bits;
        State {
            h: [0, 1, 2].map(|a| (i64::from(q.hi[a]) >> s) + 1 - offset[a]),
            l: [0, 1, 2].map(|a| (i64::from(q.lo[a]) >> s) - offset[a]),
            offset,
            key_offset,
            bits,
            property,
        }
    }
}

fn byte(code: &[u8], at: usize) -> Result<i64, Fault> {
    code.get(at)
        .map(|&b| i64::from(b))
        .ok_or(Fault::OutOfCode { at })
}
fn be(code: &[u8], at: usize, n: usize) -> Result<i64, Fault> {
    let mut v = 0i64;
    for i in 0..n {
        v = (v << 8) | byte(code, at + i)?;
    }
    Ok(v)
}

/// Axis signs, threshold multiplier and bias of a diagonal split.
pub fn diagonal_plane(op: u8) -> ([i64; 3], i64, i64) {
    match op {
        0x13 => ([0, 1, 1], 2, 0),
        0x14 => ([0, 1, -1], 2, 0xff),
        0x15 => ([1, 0, 1], 2, 0),
        0x16 => ([1, 0, -1], 2, 0xff),
        0x17 => ([1, 1, 0], 2, 0),
        0x18 => ([1, -1, 0], 2, 0xff),
        0x19 => ([1, 1, 1], 3, 0),
        0x1a => ([1, 1, -1], 3, 0xff),
        0x1b => ([1, -1, 1], 3, 0xff),
        _ => ([1, -1, -1], 3, 0x1fe),
    }
}

/// Lower and upper projections of the box for a diagonal split, and the
/// thresholds' multiplier and constant (E `0x002ff8de..0x002ffa4f`).
pub fn diagonal(lo: [i64; 3], hi: [i64; 3], op: u8) -> (i64, i64, i64, i64) {
    let (s, m, k) = diagonal_plane(op);
    let (mut lower, mut upper) = (0, 0);
    for (a, &sign) in s.iter().enumerate() {
        match sign {
            1 => {
                lower += lo[a];
                upper += hi[a];
            }
            -1 => {
                lower -= hi[a];
                upper -= lo[a];
            }
            _ => {}
        }
    }
    (lower, upper, m, k)
}

/// Keys the engine's box query returns for `aabb`, in the engine's order,
/// one per terminal reached (duplicates possible).
pub fn query_box(code: &Code, aabb: &Aabb) -> Result<Vec<u32>, Fault> {
    let q = Query::new(code, aabb);
    let root = State::frame(&q, [0; 3], 0, 0, 0);
    let mut out = Vec::new();
    walk(&code.bytecode, 0, root, &q, &mut out)?;
    Ok(out)
}

/// A two-plane split: low child when `lower < low_max`, high child when
/// `upper > high_min` (E `0x002ffa52`, `0x002ffa85`, `0x002ffafe`).
pub enum Visit {
    None,
    Low,
    High,
    Both,
}
pub fn two_plane(lower: i64, upper: i64, low_max: i64, high_min: i64) -> Visit {
    match (lower < low_max, upper > high_min) {
        (false, false) => Visit::None,
        (true, false) => Visit::Low,
        (false, true) => Visit::High,
        (true, true) => Visit::Both,
    }
}

fn walk(
    code: &[u8],
    mut pc: usize,
    mut st: State,
    q: &Query,
    out: &mut Vec<u32>,
) -> Result<(), Fault> {
    loop {
        let op = byte(code, pc)? as u8;
        match op {
            0x00 => return Ok(()),
            0x01..=0x04 => {
                let k = u32::from(op);
                let bits = st.bits + k;
                if bits > ROOT_SHIFT {
                    return Err(Fault::Shift { at: pc });
                }
                let mut offset = [0i64; 3];
                for (a, o) in offset.iter_mut().enumerate() {
                    *o = (byte(code, pc + 1 + a)? + st.offset[a]) << k;
                }
                st = State::frame(q, offset, bits, st.key_offset, st.property);
                pc += 4;
            }
            0x05 => pc += 2 + byte(code, pc + 1)? as usize,
            0x06 => pc += 3 + be(code, pc + 1, 2)? as usize,
            0x07 => pc += 4 + be(code, pc + 1, 3)? as usize,
            0x09 => {
                st.key_offset = st.key_offset.wrapping_add(byte(code, pc + 1)? as u32);
                pc += 2;
            }
            0x0a => {
                st.key_offset = st.key_offset.wrapping_add(be(code, pc + 1, 2)? as u32);
                pc += 3;
            }
            0x0b => {
                st.key_offset = be(code, pc + 1, 4)? as u32;
                pc += 5;
            }
            0x10..=0x1c => {
                let (lower, upper, m, k) = if op <= 0x12 {
                    let a = usize::from(op - 0x10);
                    (st.l[a], st.h[a], 1, 0)
                } else {
                    diagonal(st.l, st.h, op)
                };
                let low_max = byte(code, pc + 1)? * m - k;
                let high_min = byte(code, pc + 2)? * m - k;
                let first = pc + 4;
                let second = first + byte(code, pc + 3)? as usize;
                match two_plane(lower, upper, low_max, high_min) {
                    Visit::None => return Ok(()),
                    Visit::Low => pc = first,
                    Visit::High => pc = second,
                    Visit::Both => {
                        walk(code, first, st, q, out)?;
                        pc = second;
                    }
                }
            }
            0x20..=0x22 => {
                let a = usize::from(op - 0x20);
                let split = byte(code, pc + 1)?;
                let first = pc + 3;
                let second = first + byte(code, pc + 2)? as usize;
                // E 0x002ffac7: low only when H <= split; high only when
                // L > split.
                if st.h[a] <= split {
                    pc = first;
                } else if st.l[a] > split {
                    pc = second;
                } else {
                    walk(code, first, st, q, out)?;
                    pc = second;
                }
            }
            0x23..=0x25 => {
                let a = usize::from(op - 0x23);
                let low_max = byte(code, pc + 1)?;
                let high_min = byte(code, pc + 2)?;
                let base = pc + 7;
                let first = base + be(code, pc + 3, 2)? as usize;
                let second = base + be(code, pc + 5, 2)? as usize;
                match two_plane(st.l[a], st.h[a], low_max, high_min) {
                    Visit::None => return Ok(()),
                    Visit::Low => pc = first,
                    Visit::High => pc = second,
                    Visit::Both => {
                        walk(code, first, st, q, out)?;
                        pc = second;
                    }
                }
            }
            0x26..=0x28 => {
                // E 0x002ffb73: out when H < min or L >= max.
                let a = usize::from(op - 0x26);
                if st.h[a] < byte(code, pc + 1)? || st.l[a] >= byte(code, pc + 2)? {
                    return Ok(());
                }
                pc += 3;
            }
            0x29..=0x2b => {
                // E 0x002ffb9d: full-precision corners, out when
                // hi < min or lo > max.
                let a = usize::from(op - 0x29);
                if i64::from(q.hi[a]) < be(code, pc + 1, 3)?
                    || i64::from(q.lo[a]) > be(code, pc + 4, 3)?
                {
                    return Ok(());
                }
                pc += 7;
            }
            0x30..=0x53 => {
                let key = crate::code::terminal_key(op, |n| be(code, pc + 1, n).map(|v| v as u32))?;
                out.push(st.key_offset.wrapping_add(key));
                return Ok(());
            }
            0x60 => {
                st.property = byte(code, pc + 1)? as u32;
                pc += 2;
            }
            0x64 => {
                st.property = be(code, pc + 1, 2)? as u32;
                pc += 3;
            }
            0x68 => {
                st.property = be(code, pc + 1, 4)? as u32;
                pc += 5;
            }
            // Slots 1..3 lie past the 0x38-byte state, on the return
            // address (E 0x002ffd5a).
            0x61..=0x63 | 0x65..=0x67 | 0x69..=0x6b => return Err(Fault::Property { at: pc, op }),
            _ => return Err(Fault::BadOpcode { at: pc, op }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program over the unit-scaled space: world coordinate `p` is full
    /// coordinate `p` (offset 0, scale 1), so the root frame cell of `p` is
    /// `p >> 16`. `cell(c)` is the world coordinate of a root cell's low
    /// edge.
    fn prog(bytecode: &[u8]) -> Code {
        Code {
            offset: [0.0; 3],
            scale: 1.0,
            info: crate::code::RETAIL_INFO,
            bytecode: bytecode.to_vec(),
        }
    }
    fn cell(c: f32) -> f32 {
        c * 65536.0
    }
    /// A box from root cell `lo` to root cell `hi` on x (both inclusive,
    /// well inside the cells), cell 0..1 on y and z.
    fn on_x(lo: f32, hi: f32) -> Aabb {
        Aabb {
            min: [cell(lo) + 1000.0, 1000.0, 1000.0],
            max: [cell(hi) + 60000.0, 60000.0, 60000.0],
        }
    }
    fn q(code: &Code, b: Aabb) -> Vec<u32> {
        let mut k = query_box(code, &b).unwrap();
        k.sort_unstable();
        k
    }

    #[test]
    fn setup_rounds_both_corners_down_one() {
        let c = prog(&[0x00]);
        let b = Aabb {
            min: [10.4, 10.5, 11.5],
            max: [20.5, 21.5, -0.4],
        };
        let qq = Query::new(&c, &b);
        // Ties to even, then minus one.
        assert_eq!(qq.lo, [9, 9, 11]);
        assert_eq!(qq.hi, [19, 21, -1]);
        // Root frame: H = (hi >> 16) + 1, L = lo >> 16.
        let st = State::frame(&qq, [0; 3], 0, 0, 0);
        assert_eq!(st.h, [1, 1, 0]);
        assert_eq!(st.l, [0, 0, 0]);
        assert_eq!(fistp(3.0e9), i32::MIN);
    }

    #[test]
    fn two_plane_low_max_is_exclusive() {
        // Low child (key 1) for x cells below 10, high child (key 2) from
        // cell 20: 0x10 low_max 10, high_min 20, low child one byte long.
        let p = prog(&[0x10, 10, 20, 1, 0x31, 0x32]);
        assert_eq!(q(&p, on_x(9.0, 9.0)), vec![1]);
        // L == low_max: the low child is not visited.
        assert_eq!(q(&p, on_x(10.0, 12.0)), Vec::<u32>::new());
        // In the gap: neither.
        assert_eq!(q(&p, on_x(11.0, 19.0)), Vec::<u32>::new());
        // H > high_min once the box reaches cell 20.
        assert_eq!(q(&p, on_x(15.0, 20.0)), vec![2]);
        assert_eq!(q(&p, on_x(5.0, 25.0)), vec![1, 2]);
        // The same through the 16-bit form.
        let p = prog(&[0x23, 10, 20, 0, 0, 0, 1, 0x31, 0x32]);
        assert_eq!(q(&p, on_x(9.0, 9.0)), vec![1]);
        assert_eq!(q(&p, on_x(10.0, 12.0)), Vec::<u32>::new());
        assert_eq!(q(&p, on_x(15.0, 20.0)), vec![2]);
        assert_eq!(q(&p, on_x(5.0, 25.0)), vec![1, 2]);
    }

    #[test]
    fn bound_check_max_is_exclusive() {
        let p = prog(&[0x26, 10, 20, 0x31]);
        // H < min: out; H == min (box reaches cell 9): in.
        assert_eq!(q(&p, on_x(5.0, 8.0)), Vec::<u32>::new());
        assert_eq!(q(&p, on_x(5.0, 9.0)), vec![1]);
        // L >= max: out, though the box starts in the bound's top cell.
        assert_eq!(q(&p, on_x(19.0, 25.0)), vec![1]);
        assert_eq!(q(&p, on_x(20.0, 25.0)), Vec::<u32>::new());
    }

    #[test]
    fn one_plane_split() {
        let p = prog(&[0x20, 10, 1, 0x31, 0x32]);
        assert_eq!(q(&p, on_x(5.0, 9.0)), vec![1]);
        // H > split from cell 10: both while L <= split.
        assert_eq!(q(&p, on_x(5.0, 10.0)), vec![1, 2]);
        assert_eq!(q(&p, on_x(10.0, 12.0)), vec![1, 2]);
        assert_eq!(q(&p, on_x(11.0, 12.0)), vec![2]);
    }

    #[test]
    fn rescale_recomputes_from_full_precision() {
        // Zoom by 4 at root cell (1, 0, 0): frame cells are 4096 units, the
        // frame starts at full 65536. Bound x cells 2..5 (exclusive).
        let p = prog(&[0x04, 1, 0, 0, 0x26, 2, 5, 0x31]);
        let at = |lo: f32, hi: f32| Aabb {
            min: [65536.0 + lo * 4096.0 + 10.0, 10.0, 10.0],
            max: [65536.0 + hi * 4096.0 + 4000.0, 4000.0, 4000.0],
        };
        assert_eq!(q(&p, at(4.0, 6.0)), vec![1]);
        assert_eq!(q(&p, at(5.0, 6.0)), Vec::<u32>::new());
        assert_eq!(q(&p, at(0.0, 0.0)), Vec::<u32>::new());
        assert_eq!(q(&p, at(0.0, 1.0)), vec![1]);
        // Past shift 0 faults.
        let p = prog(&[
            0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x01, 0, 0, 0,
        ]);
        assert_eq!(query_box(&p, &on_x(0.0, 0.0)), Err(Fault::Shift { at: 16 }));
    }

    #[test]
    fn diagonal_split() {
        // x + y (0x17, multiplier 2): low child when Lx + Ly < 2 * 10, high
        // when Hx + Hy > 2 * 12.
        let p = prog(&[0x17, 10, 12, 1, 0x31, 0x32]);
        let b = |x: f32, y: f32| Aabb {
            min: [cell(x) + 1000.0, cell(y) + 1000.0, 1000.0],
            max: [cell(x) + 60000.0, cell(y) + 60000.0, 60000.0],
        };
        // Lx + Ly = 19, Hx + Hy = 21.
        assert_eq!(q(&p, b(9.0, 10.0)), vec![1]);
        // 20 and 22: neither.
        assert_eq!(q(&p, b(10.0, 10.0)), Vec::<u32>::new());
        // 23 and 25: high.
        assert_eq!(q(&p, b(11.0, 12.0)), vec![2]);
        // x - y + 0xff (0x18): Lx - Hy = -2, Hx - Ly = 0 against 2 * b - 0xff.
        let p = prog(&[0x18, 128, 128, 1, 0x31, 0x32]);
        assert_eq!(q(&p, b(9.0, 10.0)), vec![1]);
        assert_eq!(q(&p, b(12.0, 10.0)), vec![2]);
    }

    #[test]
    fn full_precision_check_and_keys() {
        // 0x29: x in [100, 200] full units, then key offset and a 2-byte
        // key; 0x0b sets the offset outright.
        let p = prog(&[0x0b, 0, 0, 1, 0, 0x29, 0, 0, 100, 0, 0, 200, 0x51, 1, 2]);
        let b = |lo: f32, hi: f32| Aabb {
            min: [lo, 0.0, 0.0],
            max: [hi, 0.0, 0.0],
        };
        // hi = round(max) - 1.
        assert_eq!(q(&p, b(0.0, 100.0)), Vec::<u32>::new());
        assert_eq!(q(&p, b(0.0, 101.0)), vec![0x0102 + 0x100]);
        // lo = round(min) - 1 > 200 only from 202.
        assert_eq!(q(&p, b(201.0, 300.0)), vec![0x0202]);
        assert_eq!(q(&p, b(202.0, 300.0)), Vec::<u32>::new());
    }

    #[test]
    fn faults() {
        let b = on_x(0.0, 0.0);
        assert_eq!(
            query_box(&prog(&[0x08]), &b),
            Err(Fault::BadOpcode { at: 0, op: 0x08 })
        );
        assert_eq!(
            query_box(&prog(&[0x61, 1]), &b),
            Err(Fault::Property { at: 0, op: 0x61 })
        );
        assert_eq!(
            query_box(&prog(&[0x26, 0]), &b),
            Err(Fault::OutOfCode { at: 2 })
        );
        assert_eq!(query_box(&prog(&[0x60, 7, 0x30]), &b), Ok(vec![0]));
    }
}
