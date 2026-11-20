//! A model of the engine's MOPP interpreters (docs/formats/mopp.md).
//!
//! The XBE carries three interpreters with one opcode table between them
//! (E: the byte tables at `0x00300860`, `0x003016a8`, `0x0030284c` map
//! every opcode to the same handler class). `0x00300290` is the integer
//! sphere query; this module follows it to the bit ([`query_sphere`]) and
//! generalises its per-axis interval tests to boxes ([`query_aabb`]).

use crate::code::{Code, FULL_BITS, ROOT_SHIFT};

/// A world-space axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// What the query sees, in full-precision integer units.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// Per-axis inclusive interval.
    Box { lo: [i64; 3], hi: [i64; 3] },
    /// The engine's sphere: centre `round(p * scale) - 1`, radius
    /// `round(r * scale)` (E `0x003008d0..0x00300955`).
    Sphere { c: [i64; 3], r: i64 },
}

/// One frame of the interpreter: the state the engine keeps in a 0x28-byte
/// struct (E `0x00300290`: `+0x10..+0x18` offsets, `+0x1c` key offset,
/// `+0x20` shift, `+0x24` property 0).
#[derive(Clone, Copy, Debug)]
struct Frame {
    offset: [i64; 3],
    key_offset: u32,
    shift: u32,
    property: u32,
}

/// An opcode the engine rejects (`> 0x6b`, or a hole in the table), or a
/// read past the end of the bytecode. The engine has no bounds checks.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    BadOpcode {
        at: usize,
        op: u8,
    },
    OutOfCode {
        at: usize,
    },
    /// Rescale below shift 0 (the engine would shift by a negative count).
    Shift {
        at: usize,
    },
    /// `0x61..0x63`, `0x65..0x67`, `0x69..0x6b` write property slots 1..3,
    /// which lie past the engine's 0x28-byte state (onto the return
    /// address): never valid in this build.
    Property {
        at: usize,
        op: u8,
    },
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

impl Shape {
    /// Frame-space interval on one axis.
    fn axis(&self, f: &Frame, a: usize) -> (i64, i64) {
        match *self {
            Shape::Box { lo, hi } => (
                (lo[a] >> f.shift) - f.offset[a],
                (hi[a] >> f.shift) - f.offset[a],
            ),
            Shape::Sphere { c, r } => {
                let cc = (c[a] >> f.shift) - f.offset[a];
                let rr = (r >> f.shift) + 1;
                (cc - rr, cc + rr)
            }
        }
    }
    /// Frame-space interval of a diagonal projection, opcode `0x13..=0x1c`
    /// (E `0x003002bf..0x003003bc`). Returns the interval and the operand
    /// multiplier.
    fn diagonal(&self, f: &Frame, op: u8) -> (i64, i64, i64) {
        // (signs of x, y, z; constant; multiplier)
        let (s, k, m): ([i64; 3], i64, i64) = match op {
            0x13 => ([0, 1, 1], 0, 2),
            0x14 => ([0, 1, -1], 0xff, 2),
            0x15 => ([1, 0, 1], 0, 2),
            0x16 => ([1, 0, -1], 0xff, 2),
            0x17 => ([1, 1, 0], 0, 2),
            0x18 => ([1, -1, 0], 0xff, 2),
            0x19 => ([1, 1, 1], 0, 3),
            0x1a => ([1, 1, -1], 0xff, 3),
            0x1b => ([1, -1, 1], 0xff, 3),
            _ => ([1, -1, -1], 0x1fe, 3),
        };
        match *self {
            Shape::Box { .. } => {
                // A frame coordinate is a floor: the finer value lies in
                // [l, h + 1). Projections are floors of sums of finer
                // values (V: retail programs put surfaces past a diagonal
                // plane by one unit that per-axis floors would not; the
                // sphere VM's `+ 1` on the diagonal radius is the engine's
                // allowance for it).
                let (mut lo, mut hi) = (k, k);
                for (a, &sign) in s.iter().enumerate() {
                    let (l, h) = self.axis(f, a);
                    match sign {
                        1 => {
                            lo += l;
                            hi += h + 1;
                        }
                        -1 => {
                            lo -= h + 1;
                            hi -= l;
                        }
                        _ => {}
                    }
                }
                (lo, hi - 1, m)
            }
            Shape::Sphere { c, r } => {
                let mut v = k;
                for a in 0..3 {
                    v += s[a] * ((c[a] >> f.shift) - f.offset[a]);
                }
                let rr = (r >> f.shift) + 1;
                let rd = if m == 2 { rr + (rr >> 1) + 1 } else { rr * 4 };
                (v - rd, v + rd, m)
            }
        }
    }
    /// Full-precision interval on one axis (opcodes `0x29..=0x2b`).
    fn full(&self, a: usize) -> (i64, i64) {
        match *self {
            Shape::Box { lo, hi } => (lo[a], hi[a]),
            Shape::Sphere { c, r } => (c[a] - r, c[a] + r),
        }
    }
}

/// Run the program and collect the keys it emits. `on_key` sees each key
/// once per terminal reached (duplicates possible, as in the engine).
fn run(code: &[u8], q: &Shape, out: &mut Vec<u32>) -> Result<(), Fault> {
    let root = Frame {
        offset: [0; 3],
        key_offset: 0,
        shift: ROOT_SHIFT,
        property: 0,
    };
    walk(code, 0, root, q, out)
}

fn walk(
    code: &[u8],
    mut pc: usize,
    mut f: Frame,
    q: &Shape,
    out: &mut Vec<u32>,
) -> Result<(), Fault> {
    loop {
        let op = byte(code, pc)? as u8;
        match op {
            0x00 => return Ok(()),
            0x01..=0x04 => {
                let k = u32::from(op);
                if k > f.shift {
                    return Err(Fault::Shift { at: pc });
                }
                for a in 0..3 {
                    f.offset[a] = (f.offset[a] + byte(code, pc + 1 + a)?) << k;
                }
                f.shift -= k;
                pc += 4;
            }
            0x05 => pc += 2 + byte(code, pc + 1)? as usize,
            0x06 => pc += 3 + be(code, pc + 1, 2)? as usize,
            0x07 => pc += 4 + be(code, pc + 1, 3)? as usize,
            0x09 => {
                f.key_offset = f.key_offset.wrapping_add(byte(code, pc + 1)? as u32);
                pc += 2;
            }
            0x0a => {
                f.key_offset = f.key_offset.wrapping_add(be(code, pc + 1, 2)? as u32);
                pc += 3;
            }
            0x0b => {
                f.key_offset = be(code, pc + 1, 4)? as u32;
                pc += 5;
            }
            0x10..=0x1c => {
                let (lo, hi, m) = if op <= 0x12 {
                    let (l, h) = q.axis(&f, usize::from(op - 0x10));
                    (l, h, 1)
                } else {
                    q.diagonal(&f, op)
                };
                let low_max = byte(code, pc + 1)? * m;
                let high_min = byte(code, pc + 2)? * m;
                let jump = byte(code, pc + 3)? as usize;
                let first = pc + 4;
                let second = first + jump;
                // E 0x003003cd: first only if hi < high_min; second only
                // if lo > low_max; else both.
                if hi < high_min {
                    pc = first;
                } else if lo > low_max {
                    pc = second;
                } else {
                    walk(code, first, f, q, out)?;
                    pc = second;
                }
            }
            0x20..=0x22 => {
                let (lo, hi) = q.axis(&f, usize::from(op - 0x20));
                let split = byte(code, pc + 1)?;
                let jump = byte(code, pc + 2)? as usize;
                let first = pc + 3;
                let second = first + jump;
                // E 0x00300401: first only if hi < split; second only if
                // lo > split + 1; else both.
                if hi < split {
                    pc = first;
                } else if lo > split + 1 {
                    pc = second;
                } else {
                    walk(code, first, f, q, out)?;
                    pc = second;
                }
            }
            0x23..=0x25 => {
                let (lo, hi) = q.axis(&f, usize::from(op - 0x23));
                let low_max = byte(code, pc + 1)?;
                let high_min = byte(code, pc + 2)?;
                let j1 = be(code, pc + 3, 2)? as usize;
                let j2 = be(code, pc + 5, 2)? as usize;
                let base = pc + 7;
                if hi < high_min {
                    pc = base + j1;
                } else if lo > low_max {
                    pc = base + j2;
                } else {
                    walk(code, base + j1, f, q, out)?;
                    pc = base + j2;
                }
            }
            0x26..=0x28 => {
                let (lo, hi) = q.axis(&f, usize::from(op - 0x26));
                if hi < byte(code, pc + 1)? || lo > byte(code, pc + 2)? {
                    return Ok(());
                }
                pc += 3;
            }
            0x29..=0x2b => {
                let (lo, hi) = q.full(usize::from(op - 0x29));
                if hi < be(code, pc + 1, 3)? || lo > be(code, pc + 4, 3)? {
                    return Ok(());
                }
                pc += 7;
            }
            0x30..=0x53 => {
                let key = crate::code::terminal_key(op, |n| be(code, pc + 1, n).map(|v| v as u32))?;
                out.push(f.key_offset.wrapping_add(key));
                return Ok(());
            }
            0x60 => {
                f.property = byte(code, pc + 1)? as u32;
                pc += 2;
            }
            0x64 => {
                f.property = be(code, pc + 1, 2)? as u32;
                pc += 3;
            }
            0x68 => {
                f.property = be(code, pc + 1, 4)? as u32;
                pc += 5;
            }
            0x61..=0x63 | 0x65..=0x67 | 0x69..=0x6b => return Err(Fault::Property { at: pc, op }),
            _ => return Err(Fault::BadOpcode { at: pc, op }),
        }
    }
}

fn clamp_full(v: f64) -> i64 {
    let top = (1i64 << FULL_BITS) - 1;
    (v as i64).clamp(-(1 << 30), top + (1 << 30))
}

/// Keys of every primitive whose quantized bounds overlap `aabb`, by the
/// engine's opcode semantics applied to box intervals. Each axis of the box
/// becomes `[floor(min) - 1, ceil(max) + 1]` in full-precision units.
pub fn query_aabb(code: &Code, aabb: &Aabb) -> Result<Vec<u32>, Fault> {
    let mut lo = [0i64; 3];
    let mut hi = [0i64; 3];
    for a in 0..3 {
        lo[a] = clamp_full(code.to_full(a, aabb.min[a]).floor()) - 1;
        hi[a] = clamp_full(code.to_full(a, aabb.max[a]).ceil()) + 1;
    }
    let mut out = Vec::new();
    run(&code.bytecode, &Shape::Box { lo, hi }, &mut out)?;
    Ok(out)
}

/// The engine's sphere query (`0x003008d0` then `0x00300290`), bit for
/// bit: x87 `fistp` rounds to nearest even, centre minus one.
pub fn query_sphere(code: &Code, center: [f32; 3], radius: f32) -> Result<Vec<u32>, Fault> {
    let mut c = [0i64; 3];
    for (a, slot) in c.iter_mut().enumerate() {
        // The engine computes in x87 extended precision from f32 operands
        // and stores the product to an f32 before fistp (E 0x003008ed).
        // f64 holds the difference and product exactly, so the one f32
        // rounding matches.
        let v = ((f64::from(center[a]) - f64::from(code.offset[a])) * f64::from(code.scale)) as f32;
        *slot = i64::from(v.round_ties_even() as i32) - 1;
    }
    let r =
        i64::from(((f64::from(radius) * f64::from(code.scale)) as f32).round_ties_even() as i32);
    let mut out = Vec::new();
    run(&code.bytecode, &Shape::Sphere { c, r }, &mut out)?;
    Ok(out)
}
