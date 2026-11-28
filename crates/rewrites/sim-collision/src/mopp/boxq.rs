//! The integer box query (entry F, `0x00300150` and `0x002ff8b0` below
//! it) over a decoded [`Tree`], one box at a time or as a packet of up to
//! eight.
//!
//! The setup converts each AABB coordinate to the program's
//! full-precision integer space with the original's x87 sequence
//! ([`Corners::convert`]). The walk then visits the nodes in the
//! original's order, low child first, and appends one key per terminal
//! reached, duplicates included. Two conditions keep the decoded
//! thresholds exact, and both are checked per query: `lo <= hi` on every
//! axis (the one-plane split looks only at `H` when `H <= S`), and every
//! corner inside [`super::CORNER_LIMIT`].
use super::decode::NONE;
use super::{CORNER_LIMIT, MAX_FRAMES, Node, Tree};
use crate::Bail;
use ::mopp::boxvm::diagonal_plane;
use sim_core::det::{Chain, checked, checked_guest};

/// The full-precision corners the setup stores in the box VM: `lo` at
/// `+0x20..+0x28`, `hi` at `+0x10..+0x18`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corners {
    pub lo: [i32; 3],
    pub hi: [i32; 3],
}

impl Corners {
    /// `0x00300150`'s conversion of the AABB `min`, `max` (binary32 bit
    /// patterns) against the code's `origin` and `scale`, per coordinate:
    /// `fld p; fsub origin; fmul scale` in the working precision of the
    /// x87 `control` word, `fstp` to binary32, `fld`, `fistp`, then a
    /// 32-bit decrement. Bails with [`Bail::Float`] off the checked path:
    /// an operand that is not zero or normal, a non-normal intermediate,
    /// extended precision, a rounding mode other than nearest, or a value
    /// `fistp` cannot represent.
    pub fn convert(
        min: [u32; 3],
        max: [u32; 3],
        origin: [u32; 3],
        scale: u32,
        control: u16,
    ) -> Result<Self, Bail> {
        Self::convert_inexact(min, max, origin, scale, control).map(|(c, _)| c)
    }

    /// [`Corners::convert`], and whether a conversion was inexact: the
    /// x87 status word's PE after the sequence (the `fsub`, `fmul`, the
    /// `fstp m32` or the `fistp`).
    pub fn convert_inexact(
        min: [u32; 3],
        max: [u32; 3],
        origin: [u32; 3],
        scale: u32,
        control: u16,
    ) -> Result<(Self, bool), Bail> {
        checked(|r| {
            let load = |bits: u32| Chain::load(r, control, f32::from_bits(bits));
            let full = |p: u32, a: usize| -> Option<i32> {
                let product = (load(p)? - load(origin[a])?) * load(scale)?;
                let stored = product.store();
                Some(Chain::load(r, control, stored)?.integer()?.wrapping_sub(1))
            };
            // The original's order: x min, x max, y min, y max, z min, z max.
            let mut c = Corners {
                lo: [0; 3],
                hi: [0; 3],
            };
            for a in 0..3 {
                c.lo[a] = full(min[a], a)?;
                c.hi[a] = full(max[a], a)?;
            }
            Some((c, r.x87_inexact()))
        })
        .ok_or(Bail::Float)
    }

    /// [`Corners::convert`] under the guest's x87 `control` (it must pass
    /// `sim_core::det::x87_env`) for any operand and any precision or
    /// rounding control: the corners and the x87 exception flags (`0x3f`)
    /// the sequence raises. A NaN or out-of-range value converts to the
    /// indefinite `0x8000_0000` with IE, as `fistp` does. Only a host
    /// without the checked region's controls declines ([`Bail::Env`]).
    pub fn convert_guest(
        min: [u32; 3],
        max: [u32; 3],
        origin: [u32; 3],
        scale: u32,
        control: u16,
    ) -> Result<(Self, u16), Bail> {
        // No SSE operation runs: any masked MXCSR will do.
        checked_guest(0x1f80, control, |r| {
            let load = |bits: u32| Chain::load(r, control, f32::from_bits(bits));
            let full = |p: u32, a: usize| -> Option<i32> {
                let product = (load(p)? - load(origin[a])?) * load(scale)?;
                let stored = product.store();
                Some(Chain::load(r, control, stored)?.integer()?.wrapping_sub(1))
            };
            let mut c = Corners {
                lo: [0; 3],
                hi: [0; 3],
            };
            for a in 0..3 {
                c.lo[a] = full(min[a], a)?;
                c.hi[a] = full(max[a], a)?;
            }
            Some((c, r.x87_flags()))
        })
        .ok_or(Bail::Env)
    }

    /// The query's preconditions: an ordered box inside the corner limit.
    pub fn check(&self) -> Result<(), Bail> {
        let inside = |v: i32| (-CORNER_LIMIT..CORNER_LIMIT).contains(&v);
        let ok = (0..3).all(|a| self.lo[a] <= self.hi[a] && inside(self.lo[a]) && inside(self.hi[a]));
        if ok { Ok(()) } else { Err(Bail::Data) }
    }

    fn wide(&self) -> ([i64; 3], [i64; 3]) {
        (self.lo.map(i64::from), self.hi.map(i64::from))
    }
}

/// A box query's answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The keys the original appends, in its order.
    pub keys: Vec<u32>,
    /// The deepest nesting of the interpreter's self-calls: one per split
    /// whose two sides are both visited, counting an empty low side. The
    /// adapter sizes the dead stack window from it.
    pub frames: u32,
}

/// The shifted corner sums of a diagonal split: `lower` and `upper`.
fn diagonal(op: u8, shift: u8, lo: &[i64; 3], hi: &[i64; 3]) -> (i64, i64) {
    let (signs, _, _) = diagonal_plane(op);
    let (mut lower, mut upper) = (0, 0);
    for (a, &sign) in signs.iter().enumerate() {
        let (l, h) = (lo[a] >> shift, hi[a] >> shift);
        match sign {
            1 => {
                lower += l;
                upper += h;
            }
            -1 => {
                lower -= h;
                upper -= l;
            }
            _ => {}
        }
    }
    (lower, upper)
}

/// The boxes of `mask` for which `test` holds.
fn select(mask: u8, test: impl Fn(usize) -> bool) -> u8 {
    (0..8usize)
        .filter(|&i| mask & (1u8 << i) != 0 && test(i))
        .fold(0u8, |m, i| m | (1u8 << i))
}

impl Tree {
    /// The keys the original appends for the box `q`, with at most `room`
    /// of them: one more is [`Bail::Grow`] (the original would grow the
    /// array). A reached [`Node::Trap`] or a `guest_only` program is
    /// [`Bail::Data`], nesting past [`MAX_FRAMES`] is [`Bail::Depth`].
    pub fn query_box(&self, q: &Corners, room: usize) -> Result<Found, Bail> {
        if self.guest_only() {
            return Err(Bail::Data);
        }
        q.check()?;
        let (lo, hi) = q.wide();
        let nodes = self.nodes();
        let mut found = Found::default();
        // The high sides still to visit, with the nesting they resume at.
        let mut pending: Vec<(u32, u32)> = Vec::new();
        let mut next = Some((self.root(), 0u32));
        while let Some((mut n, mut depth)) = next {
            while n != NONE {
                let (visit_low, visit_high, low, high) = match nodes[n as usize] {
                    Node::Split {
                        axis,
                        below,
                        above,
                        low,
                        high,
                    } => {
                        let a = usize::from(axis);
                        (lo[a] < below, hi[a] >= above, low, high)
                    }
                    Node::Diagonal {
                        op,
                        shift,
                        below,
                        above,
                        low,
                        high,
                    } => {
                        let (lower, upper) = diagonal(op, shift, &lo, &hi);
                        (lower < below, upper > above, low, high)
                    }
                    Node::Bound {
                        axis,
                        above,
                        below,
                        next,
                    } => {
                        let a = usize::from(axis);
                        n = if hi[a] >= above && lo[a] < below {
                            next
                        } else {
                            NONE
                        };
                        continue;
                    }
                    Node::Leaf { key } => {
                        if found.keys.len() >= room {
                            return Err(Bail::Grow);
                        }
                        found.keys.push(key);
                        break;
                    }
                    Node::Trap { .. } => return Err(Bail::Data),
                };
                n = match (visit_low, visit_high) {
                    (true, true) => {
                        // The original calls itself for the low side and
                        // carries on with the high side in its own frame.
                        if high != NONE {
                            pending.push((high, depth));
                        }
                        depth += 1;
                        if depth > MAX_FRAMES {
                            return Err(Bail::Depth);
                        }
                        found.frames = found.frames.max(depth);
                        low
                    }
                    (true, false) => low,
                    (false, true) => high,
                    (false, false) => NONE,
                };
            }
            next = pending.pop();
        }
        Ok(found)
    }

    /// Up to eight boxes through the tree in one walk (section 6.2). Each
    /// node is visited once for the boxes that reach it, and each box's
    /// keys come out in the serial order, so every answer equals
    /// [`Self::query_box`] with unlimited room, including the per-box depth limit.
    pub fn query_packet(&self, boxes: &[Corners]) -> Vec<Result<Vec<u32>, Bail>> {
        assert!(boxes.len() <= 8, "a packet holds at most eight boxes");
        let mut out: Vec<Result<Vec<u32>, Bail>> = boxes
            .iter()
            .map(|b| {
                if self.guest_only() {
                    Err(Bail::Data)
                } else {
                    b.check().map(|()| Vec::new())
                }
            })
            .collect();
        let corners: Vec<([i64; 3], [i64; 3])> = boxes.iter().map(Corners::wide).collect();
        let mut live: u8 = 0;
        for (i, r) in out.iter().enumerate() {
            if r.is_ok() {
                live |= 1 << i;
            }
        }
        let nodes = self.nodes();
        let mut pending = Vec::new();
        let mut next = Some((self.root(), live, [0u16; 8]));
        while let Some((mut n, mut mask, mut depth)) = next {
            mask &= live;
            while n != NONE && mask != 0 {
                let (low_mask, high_mask, low, high) = match nodes[n as usize] {
                    Node::Split {
                        axis,
                        below,
                        above,
                        low,
                        high,
                    } => {
                        let a = usize::from(axis);
                        (
                            select(mask, |i| corners[i].0[a] < below),
                            select(mask, |i| corners[i].1[a] >= above),
                            low,
                            high,
                        )
                    }
                    Node::Diagonal {
                        op,
                        shift,
                        below,
                        above,
                        low,
                        high,
                    } => {
                        let sums: Vec<(i64, i64)> = corners
                            .iter()
                            .map(|(lo, hi)| diagonal(op, shift, lo, hi))
                            .collect();
                        (
                            select(mask, |i| sums[i].0 < below),
                            select(mask, |i| sums[i].1 > above),
                            low,
                            high,
                        )
                    }
                    Node::Bound {
                        axis,
                        above,
                        below,
                        next,
                    } => {
                        let a = usize::from(axis);
                        mask = select(mask, |i| corners[i].1[a] >= above && corners[i].0[a] < below);
                        n = next;
                        continue;
                    }
                    Node::Leaf { key } => {
                        for (i, r) in out.iter_mut().enumerate() {
                            if mask & (1 << i) != 0
                                && let Ok(keys) = r
                            {
                                keys.push(key);
                            }
                        }
                        break;
                    }
                    Node::Trap { .. } => {
                        for (i, r) in out.iter_mut().enumerate() {
                            if mask & (1 << i) != 0 {
                                *r = Err(Bail::Data);
                            }
                        }
                        live &= !mask;
                        break;
                    }
                };
                if high != NONE && high_mask != 0 {
                    pending.push((high, high_mask, depth));
                }
                // The interpreter recursively calls the low side only for boxes
                // that take both children, even if the low side is empty.
                for (i, d) in depth.iter_mut().enumerate() {
                    if low_mask & high_mask & (1 << i) != 0 {
                        *d += 1;
                        if u32::from(*d) > MAX_FRAMES {
                            live &= !(1 << i);
                            out[i] = Err(Bail::Depth);
                        }
                    }
                }
                n = low;
                mask = low_mask & live;
            }
            next = pending.pop();
        }
        out
    }
}
