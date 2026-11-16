//! The compiler: a binary split tree over the surfaces' quantized bounds.
//!
//! It emits only opcodes whose semantics are read from the engine (E) and
//! that retail programs use (V): rescale `0x01..0x04`, the jump `0x07`,
//! key offsets `0x09`/`0x0a`/`0x0b`, the two-plane splits `0x10..0x12` and
//! `0x23..0x25`, the bound checks `0x26..0x28`, and the terminals
//! `0x30..0x53`. Every node's region is narrowed by bound checks to the
//! union of its surfaces' boxes, so a box query returns the surfaces whose
//! quantized boxes it overlaps.
//!
//! Invariant: every primitive in a node lies inside the node's frame,
//! `0..=255` on each axis, so every operand fits a byte.

use crate::code::{Code, FULL_BITS, RETAIL_INFO, ROOT_SHIFT};
use crate::geom::Surface;

/// Full-precision units a primitive's box is grown by on each side, so the
/// engine's rounding (sphere centres `round(p) - 1`, E `0x003008fd`) can
/// never miss it.
const PAD: i64 = 2;
/// Headroom left at the top of the 24-bit space.
const TOP_MARGIN: f64 = 4096.0;
/// An inner node checks an axis when that trims at least 1/this of the
/// region it inherits.
const INNER_CHECK_SHARE: i64 = 8;
/// The same for a single surface.
const LEAF_CHECK_SHARE: i64 = 32;
/// Only subtrees of at least this many surfaces rescale. These three knobs
/// were tuned on the retail BSPs: the output is then about as large as
/// retail's and slightly tighter (docs/formats/mopp.md).
const ZOOM_MIN_SURFACES: usize = 8;

#[derive(Clone, Copy)]
struct Prim {
    key: u32,
    lo: [i64; 3],
    hi: [i64; 3],
    /// Twice the centre, full precision.
    c2: [i64; 3],
}

#[derive(Clone, Copy)]
struct Frame {
    shift: u32,
    offset: [i64; 3],
    key_offset: u32,
}

type Region = [(i64, i64); 3];

fn frame_box(p: &Prim, f: &Frame) -> ([i64; 3], [i64; 3]) {
    let mut lo = [0; 3];
    let mut hi = [0; 3];
    for a in 0..3 {
        lo[a] = (p.lo[a] >> f.shift) - f.offset[a];
        hi[a] = (p.hi[a] >> f.shift) - f.offset[a];
    }
    (lo, hi)
}

fn union(prims: &[Prim], f: &Frame) -> Region {
    let mut u = [(i64::MAX, i64::MIN); 3];
    for p in prims {
        let (lo, hi) = frame_box(p, f);
        for a in 0..3 {
            u[a].0 = u[a].0.min(lo[a]);
            u[a].1 = u[a].1.max(hi[a]);
        }
    }
    u
}

fn key_len(rel: u32) -> usize {
    match rel {
        0..=0x1f => 1,
        0x20..=0xff => 2,
        0x100..=0xffff => 3,
        0x1_0000..=0xff_ffff => 4,
        _ => 5,
    }
}

fn terminal(out: &mut Vec<u8>, rel: u32) {
    match key_len(rel) {
        1 => out.push(0x30 + rel as u8),
        2 => out.extend([0x50, rel as u8]),
        3 => out.extend([0x51, (rel >> 8) as u8, rel as u8]),
        4 => out.extend([0x52, (rel >> 16) as u8, (rel >> 8) as u8, rel as u8]),
        _ => {
            out.push(0x53);
            out.extend(rel.to_be_bytes());
        }
    }
}

fn node(prims: &mut [Prim], mut f: Frame, mut r: Region) -> Vec<u8> {
    let mut out = Vec::new();
    let mut u = union(prims, &f);

    // Zoom in while the surfaces fit a frame twice (or more) as fine. Small
    // subtrees stay in their parent's frame: smaller code, near as tight.
    while prims.len() >= ZOOM_MIN_SURFACES {
        let span = (0..3).map(|a| u[a].1 - u[a].0).max().unwrap_or(0);
        let k = (1..=f.shift.min(4)).rev().find(|&k| (span + 1) << k <= 256);
        let Some(k) = k else { break };
        let b = [u[0].0, u[1].0, u[2].0];
        out.extend([k as u8, b[0] as u8, b[1] as u8, b[2] as u8]);
        for a in 0..3 {
            f.offset[a] = (f.offset[a] + b[a]) << k;
            r[a] = (
                r[a].0.saturating_sub(b[a]).saturating_mul(1 << k),
                r[a].1
                    .saturating_sub(b[a])
                    .saturating_mul(1 << k)
                    .saturating_add((1 << k) - 1),
            );
        }
        f.shift -= k;
        u = union(prims, &f);
    }

    // Narrow the region to the surfaces' union.
    for a in 0..3 {
        let trim = (u[a].0 - r[a].0)
            .max(0)
            .saturating_add((r[a].1 - u[a].1).max(0));
        let width = r[a].1.saturating_sub(r[a].0);
        // Check only where it cuts off a real share of the region.
        let share = if prims.len() == 1 {
            LEAF_CHECK_SHARE
        } else {
            INNER_CHECK_SHARE
        };
        let worth = trim.saturating_mul(share) >= width;
        if trim > 0 && worth {
            debug_assert!(0 <= u[a].0 && u[a].1 <= 255);
            out.extend([0x26 + a as u8, u[a].0 as u8, u[a].1 as u8]);
            r[a] = u[a];
        }
    }

    if prims.len() == 1 {
        terminal(&mut out, prims[0].key.wrapping_sub(f.key_offset));
        return out;
    }

    // Move the key offset up when the terminals get shorter for it.
    let min_rel = prims
        .iter()
        .map(|p| p.key.wrapping_sub(f.key_offset))
        .min()
        .unwrap_or(0);
    if (1..=0xffff).contains(&min_rel) {
        let now: usize = prims
            .iter()
            .map(|p| key_len(p.key.wrapping_sub(f.key_offset)))
            .sum();
        let after: usize = prims
            .iter()
            .map(|p| key_len(p.key.wrapping_sub(f.key_offset).wrapping_sub(min_rel)))
            .sum();
        let cost = if min_rel <= 0xff { 2 } else { 3 };
        if after + cost < now {
            if min_rel <= 0xff {
                out.extend([0x09, min_rel as u8]);
            } else {
                out.extend([0x0a, (min_rel >> 8) as u8, min_rel as u8]);
            }
            f.key_offset = f.key_offset.wrapping_add(min_rel);
        }
    }

    // Split at the median centre on the axis where centres spread most.
    let axis = (0..3)
        .max_by_key(|&a| {
            let (mn, mx) = prims.iter().fold((i64::MAX, i64::MIN), |(mn, mx), p| {
                (mn.min(p.c2[a]), mx.max(p.c2[a]))
            });
            mx - mn
        })
        .unwrap_or(0);
    prims.sort_by_key(|p| (p.c2[axis], p.key));
    let mid = prims.len() / 2;
    let (low, high) = prims.split_at_mut(mid);
    let low_max = low
        .iter()
        .map(|p| frame_box(p, &f).1[axis])
        .max()
        .unwrap_or(0);
    let high_min = high
        .iter()
        .map(|p| frame_box(p, &f).0[axis])
        .min()
        .unwrap_or(0);
    // The engine visits the low child when hi < high_min or lo <= low_max,
    // so a query there has lo <= max(low_max, high_min - 1); it visits the
    // high child only when hi >= high_min (E 0x003003cd).
    let mut r_low = r;
    r_low[axis].1 = r_low[axis].1.min(low_max.max(high_min - 1));
    let mut r_high = r;
    r_high[axis].0 = r_high[axis].0.max(high_min);
    let low_code = node(low, f, r_low);
    let high_code = node(high, f, r_high);

    let (lm, hm) = (low_max as u8, high_min as u8);
    if low_code.len() <= 0xff {
        out.extend([0x10 + axis as u8, lm, hm, low_code.len() as u8]);
        out.extend(low_code);
    } else if low_code.len() <= 0xffff {
        let j2 = low_code.len() as u16;
        out.extend([0x23 + axis as u8, lm, hm, 0, 0]);
        out.extend(j2.to_be_bytes());
        out.extend(low_code);
    } else {
        // High child through a 24-bit jump: J2 = 0 lands on the jump, J1 = 4
        // on the low child just after it.
        let j = low_code.len() as u32;
        out.extend([0x23 + axis as u8, lm, hm, 0, 4, 0, 0]);
        out.extend([0x07, (j >> 16) as u8, (j >> 8) as u8, j as u8]);
        out.extend(low_code);
    }
    out.extend(high_code);
    out
}

/// Compile `surfaces` into a MOPP code object.
pub fn build(surfaces: &[Surface]) -> Code {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for s in surfaces {
        for v in &s.vertices {
            for a in 0..3 {
                min[a] = min[a].min(v[a]);
                max[a] = max[a].max(v[a]);
            }
        }
    }
    if !min.iter().chain(max.iter()).all(|v| v.is_finite()) {
        return Code {
            offset: [0.0; 3],
            scale: 1.0,
            info: RETAIL_INFO,
            bytecode: vec![0x00],
        };
    }
    let extent = (0..3)
        .map(|a| f64::from(max[a]) - f64::from(min[a]))
        .fold(0.0, f64::max);
    let pad = 0.01 + extent * 1e-4;
    let offset = min.map(|m| (f64::from(m) - pad) as f32);
    let full_top = f64::from(1u32 << FULL_BITS);
    let scale = ((full_top - TOP_MARGIN) / (extent + 2.0 * pad)) as f32;
    let mut code = Code {
        offset,
        scale,
        info: RETAIL_INFO,
        bytecode: Vec::new(),
    };
    let top = (1i64 << FULL_BITS) - 1;
    let mut prims: Vec<Prim> = surfaces
        .iter()
        .filter(|s| !s.vertices.is_empty())
        .map(|s| {
            let b = s.bounds();
            let mut lo = [0; 3];
            let mut hi = [0; 3];
            let mut c2 = [0; 3];
            for a in 0..3 {
                lo[a] = ((code.to_full(a, b.min[a]).floor() as i64) - PAD).clamp(0, top);
                hi[a] = ((code.to_full(a, b.max[a]).ceil() as i64) + PAD).clamp(0, top);
                c2[a] = lo[a] + hi[a];
            }
            Prim {
                key: s.key,
                lo,
                hi,
                c2,
            }
        })
        .collect();
    let base = prims.iter().map(|p| p.key).min().unwrap_or(0);
    let f = Frame {
        shift: ROOT_SHIFT,
        offset: [0; 3],
        key_offset: base,
    };
    let mut bytecode = Vec::new();
    if base != 0 {
        bytecode.push(0x0b);
        bytecode.extend(base.to_be_bytes());
    }
    if prims.is_empty() {
        bytecode.push(0x00);
    } else {
        let unbounded = [(i64::MIN / 4, i64::MAX / 4); 3];
        bytecode.extend(node(&mut prims, f, unbounded));
    }
    code.bytecode = bytecode;
    code
}

/// The `sbsp+0x1e0` block for a structure's collision BSP: the code object
/// (header and bytecode, padded to 16 bytes) over every collision surface,
/// keyed as structure surfaces (`0x20000000 | index`). The block's element
/// size is one byte; its count is the returned length.
pub fn build_structure(bsp: &tags::collision::Bsp) -> Vec<u8> {
    build(&crate::geom::structure_surfaces(bsp)).to_block()
}
