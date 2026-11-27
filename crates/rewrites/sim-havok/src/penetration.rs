//! The triangle case's penetration call (docs/engine/havok-convex.md,
//! "The penetration call"): the profiled wrapper `0x0030a6b0` and its
//! solver `0x00309b50` up to the expanding polytope, each one guest
//! function on a [`Journal`], every float operation one guest
//! instruction: packed and scalar SSE under the MXCSR, x87 under the
//! control word.
//!
//! - [`penetration`] `0x0030a6b0`: the solver's state built on the
//!   frame, then the solver.
//! - [`solve`] `0x00309b50`: the simplex grown to a tetrahedron around
//!   the origin of `A - B`, with its exits; a tetrahedron that is not
//!   flat goes on to the polytope, which declines
//!   ([`Fallback::Closest`]).
//! - [`barycentric`] `0x00308260`: a point's weights on a triangle.
//!
//! The shapes' support virtuals go through [`Calls`]. [`Port`] answers
//! [`gjk::Penetration`] with these. On a [`Fallback`] the journal is
//! discarded and nothing has been written.
use crate::convex::Fallback;
use crate::fp::Arith;
use crate::gjk::{self, cmp, combine, cross, done, put, rows, set, units, vec, word, ABS, ONE, ONES, SAME, SIGNS, TINY, ZERO};
use crate::jacobian::{constant::SLACK, lane, splat, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::load;
use x87::{Cmp, F80};

/// The monitor stream's head and end: the wrapper writes a timed entry
/// when the head is below the end.
pub const MONITOR_HEAD: u32 = 0x0048_0154;
pub const MONITOR_END: u32 = 0x0048_0158;
/// `0x004136c4`: the term the line exit adds to its weight's denominator.
pub const SPREAD: u32 = 0x0041_36c4;
const HALF_BITS: u32 = 0x3f00_0000;
const ONE_BITS: u32 = 0x3f80_0000;

/// The solver's state, which the wrapper builds on its frame: B's
/// transform in A's frame, the shapes, the simplex, a retry count and the
/// differences.
pub mod state {
    pub const TRANSFORM: u32 = 0x00;
    pub const TRANSLATION: u32 = 0x30;
    pub const SHAPE_A: u32 = 0x40;
    pub const SHAPE_B: u32 = 0x44;
    pub const SIMPLEX: u32 = 0x48;
    pub const RETRIES: u32 = 0x4c;
    pub const DIFFERENCES: u32 = 0x50;
}

/// The simplex: A's four vertices, B's four in B's frame, the count.
pub mod simplex {
    pub const LOCAL_B: u32 = 0x40;
    pub const COUNT: u32 = 0x80;
}

/// A support triple ([`support`]'s output): A's point, B's in B's frame,
/// B's in A's frame, and (from [`tri_support`]) its height.
mod triple {
    pub const A: u32 = 0x00;
    pub const LOCAL_B: u32 = 0x10;
    pub const B: u32 = 0x20;
    pub const HEIGHT: u32 = 0x30;
}

/// The shapes' calls the solver makes out.
pub trait Calls {
    /// The shape's `vt[+0x30]`, thiscall `(direction, out)`, `ret 8`: the
    /// support point along the direction. `esp` is the stack pointer at
    /// the call's entry.
    fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, esp: u32) -> Result<(), Fallback>;
}

/// [`gjk::Penetration`] on [`penetration`], under `fpu`.
pub struct Port<'c, C> {
    pub fpu: Fpu,
    pub calls: &'c mut C,
}

impl<C: Calls> gjk::Penetration for Port<'_, C> {
    fn penetration(&mut self, j: &mut Journal, args: [u32; 6], esp: u32) -> Result<u32, Fallback> {
        penetration(j, args, esp, self.fpu, &mut *self.calls)
    }
}

/// `0x0030a6b0`, cdecl `(shape A, shape B, transform, simplex,
/// differences, out)`; `esp` is the stack pointer at entry. With the
/// monitor stream off (else [`Fallback::Monitor`]), the transform's four
/// columns, the shapes, the simplex, a zero retry count and the
/// differences go to the frame as the solver's [`state`], and the
/// solver's eax is the call's.
pub fn penetration(j: &mut Journal, args: [u32; 6], esp: u32, fpu: Fpu, calls: &mut impl Calls) -> Result<u32, Fallback> {
    units(fpu)?;
    if word(j, MONITOR_HEAD)? < word(j, MONITOR_END)? {
        return Err(Fallback::Monitor);
    }
    let [a, b, transform, simplex, differences, out] = args;
    // `push ebp; and esp,-16; sub esp,0x6c; push esi`: the state at
    // `esp+0x10`.
    let w = esp.wrapping_sub(4) & !15;
    let s = w.wrapping_sub(0x60);
    for i in 0..4 {
        let column = vec(j, transform.wrapping_add(16 * i))?;
        put(j, s.wrapping_add(16 * i), column)?;
    }
    set(j, s.wrapping_add(state::SHAPE_A), a)?;
    set(j, s.wrapping_add(state::SHAPE_B), b)?;
    set(j, s.wrapping_add(state::SIMPLEX), simplex)?;
    set(j, s.wrapping_add(state::RETRIES), 0)?;
    set(j, s.wrapping_add(state::DIFFERENCES), differences)?;
    set(j, w.wrapping_sub(0x74), out)?;
    // Nothing below writes the monitor stream, so its exit check is off too.
    solve(j, s, out, w.wrapping_sub(0x78), fpu, calls)
}

/// The cases of [`solve`]'s jump table (`0x0030a698`), by simplex count.
#[derive(Clone, Copy)]
enum Case {
    Point0,
    Point,
    Segment,
    Triangle,
    Tetrahedron,
}

struct Solver<'c, C> {
    s: u32,
    fpu: Fpu,
    sse: Sse,
    x: Fx,
    calls: &'c mut C,
}

/// `fcomp; test ah,0x41; jp`: above, or unordered.
fn above(c: Cmp) -> bool {
    matches!(c, Cmp::Greater | Cmp::Unordered)
}

impl<C: Calls> Solver<'_, C> {
    fn at(&self, off: u32) -> u32 {
        self.s.wrapping_add(off)
    }

    fn simplex(&self, j: &Journal) -> Result<u32, Fallback> {
        word(j, self.at(state::SIMPLEX))
    }

    fn difference(&self, j: &Journal, i: u32) -> Result<u128, Fallback> {
        vec(j, word(j, self.at(state::DIFFERENCES))?.wrapping_add(16 * i))
    }

    fn count(&self, j: &Journal) -> Result<u32, Fallback> {
        word(j, self.simplex(j)?.wrapping_add(simplex::COUNT))
    }

    fn dec(&self, j: &mut Journal) -> Result<(), Fallback> {
        let n = self.count(j)?.wrapping_sub(1);
        set(j, self.simplex(j)?.wrapping_add(simplex::COUNT), n)
    }

    /// `y + x`, then `z +` that, of `v * v`.
    fn square(&mut self, v: u128) -> u32 {
        let vv = self.sse.mul(v, v);
        self.sse.hsum(vv)
    }

    /// `0x00309300`, thiscall on the state, `(direction, out)`, `ret 8`:
    /// A's support along the direction to `out`; B's along it negated
    /// and rotated into B's frame to `out+0x10`; that one placed in A's
    /// frame, `c0 x + c1 y + (c2 z + c3)`, to `out+0x20`.
    fn support(&mut self, j: &mut Journal, dir: u32, out: u32, esp: u32) -> Result<(), Fallback> {
        // `push ebp; and esp,-16; sub esp,0x14`, three pushes.
        let r = esp.wrapping_sub(4) & !15;
        let a = word(j, self.at(state::SHAPE_A))?;
        self.calls.support(j, a, dir, out, r.wrapping_sub(0x2c))?;
        let d = vec(j, dir)? ^ vec(j, SIGNS)?;
        let m = rows(j, self.s)?;
        let local = combine(&mut self.sse, &m, d);
        let local_at = r.wrapping_sub(0x10);
        put(j, local_at, local)?;
        let b = word(j, self.at(state::SHAPE_B))?;
        let local_out = out.wrapping_add(triple::LOCAL_B);
        self.calls.support(j, b, local_at, local_out, r.wrapping_sub(0x2c))?;
        let v = vec(j, local_out)?;
        let c = |i: u32| self.at(state::TRANSFORM + 16 * i);
        let (c0, c1, c2, c3) = (vec(j, c(0))?, vec(j, c(1))?, vec(j, c(2))?, vec(j, c(3))?);
        let z = self.sse.mul(c2, splat(lane(v, 2)));
        let zt = self.sse.add(z, c3);
        let y = self.sse.mul(c1, splat(lane(v, 1)));
        let x = self.sse.mul(c0, splat(lane(v, 0)));
        let xy = self.sse.add(x, y);
        let placed = self.sse.add(xy, zt);
        put(j, out.wrapping_add(triple::B), placed)
    }

    /// `0x00309200`, thiscall on the state, `(triple)`, `ret 4`: the
    /// difference `A - B` to the differences and A's and B's own points
    /// to the simplex, at the count, then the count up one.
    fn append(&mut self, j: &mut Journal, t: u32) -> Result<(), Fallback> {
        let a = vec(j, t)?;
        let d = self.sse.sub(a, vec(j, t.wrapping_add(triple::B))?);
        let diffs = word(j, self.at(state::DIFFERENCES))?;
        let slot = diffs.wrapping_add(self.count(j)?.wrapping_mul(16));
        put(j, slot, d)?;
        let slot = self.simplex(j)?.wrapping_add(self.count(j)?.wrapping_mul(16));
        put(j, slot, vec(j, t)?)?;
        let slot = self.simplex(j)?.wrapping_add(self.count(j)?.wrapping_add(4).wrapping_mul(16));
        put(j, slot, vec(j, t.wrapping_add(triple::LOCAL_B))?)?;
        let n = self.count(j)?.wrapping_add(1);
        set(j, self.simplex(j)?.wrapping_add(simplex::COUNT), n)
    }

    /// `((p_A - p_B) - D0) . dir` of a triple.
    fn height(&mut self, j: &Journal, t: u32, d0: u128, dir: u128) -> Result<u32, Fallback> {
        let gap = self.sse.sub(vec(j, t)?, vec(j, t.wrapping_add(triple::B))?);
        let off = self.sse.sub(gap, d0);
        let along = self.sse.mul(off, dir);
        Ok(self.sse.hsum(along))
    }

    /// `|(p_A - p_B) - D0|^2` of a triple.
    fn spread(&mut self, j: &Journal, t: u32, d0: u128) -> Result<u32, Fallback> {
        let gap = self.sse.sub(vec(j, t)?, vec(j, t.wrapping_add(triple::B))?);
        let off = self.sse.sub(gap, d0);
        Ok(self.square(off))
    }

    /// `0x003093e0`, thiscall on the state, `(direction, triangle, out)`,
    /// `ret 0xc`: the supports along the direction and against it; `out`
    /// keeps the one whose height over the triangle's first vertex,
    /// along its own direction, is the larger (toward it on a tie,
    /// against it when unordered), and the height at `+0x30`.
    fn tri_support(&mut self, j: &mut Journal, dir: u32, tri: u32, out: u32, esp: u32) -> Result<(), Fallback> {
        // `push ebp; and esp,-16; sub esp,0x54`, three pushes.
        let base = (esp.wrapping_sub(4) & !15).wrapping_sub(0x60);
        let (against, toward) = (base.wrapping_add(0x20), base.wrapping_add(0x30));
        let neg = vec(j, dir)? ^ vec(j, SIGNS)?;
        put(j, against, neg)?;
        self.support(j, dir, toward, base.wrapping_sub(0xc))?;
        self.support(j, against, out, base.wrapping_sub(0xc))?;
        let d0 = vec(j, tri)?;
        let n = vec(j, dir)?;
        let up = self.height(j, toward, d0, n)?;
        let neg = vec(j, against)?;
        let down = self.height(j, out, d0, neg)?;
        // `fcomp; test ah,1; jne`: below, or unordered.
        if matches!(cmp(load(up), load(down)), Cmp::Less | Cmp::Unordered) {
            return set(j, out.wrapping_add(triple::HEIGHT), down);
        }
        let (a, local, b) = (vec(j, toward)?, vec(j, toward.wrapping_add(0x10))?, vec(j, toward.wrapping_add(0x20))?);
        put(j, out, a)?;
        put(j, out.wrapping_add(triple::LOCAL_B), local)?;
        put(j, out.wrapping_add(triple::B), b)?;
        set(j, out.wrapping_add(triple::HEIGHT), up)
    }

    /// `0x003094d0`, thiscall on the state, `(flag, out)`, `ret 8`, for an
    /// empty simplex: the support along the translation negated (`+y`
    /// when its square is not above [`SLACK`]) joins it. When the new
    /// difference's square is not above [`TINY`], `out` gets the
    /// direction, A's point, the difference's length and the weight
    /// `0.5`, and the flag is set (true).
    fn first(&mut self, j: &mut Journal, out: u32, esp: u32) -> Result<bool, Fallback> {
        // `push ebp; and esp,-16; sub esp,0x5c; push esi`.
        let base = (esp.wrapping_sub(4) & !15).wrapping_sub(0x60);
        let (dir_at, t) = (base.wrapping_add(0x10), base.wrapping_add(0x30));
        let mut dir = vec(j, self.at(state::TRANSLATION))? ^ vec(j, SIGNS)?;
        let s = self.square(dir);
        if !above(cmp(load(s), load(word(j, SLACK)?))) {
            dir = u128::from(ONE_BITS) << 32;
        }
        put(j, dir_at, dir)?;
        self.support(j, dir_at, t, base.wrapping_sub(0xc))?;
        self.append(j, t)?;
        let d0 = self.difference(j, 0)?;
        let q = self.square(d0);
        if above(cmp(load(q), load(word(j, TINY)?))) {
            return Ok(false);
        }
        let d0 = self.difference(j, 0)?;
        let q = self.square(d0);
        let length = self.sse.e.sqrt(q);
        set(j, out.wrapping_add(gjk::contact_field::DISTANCE), self.x.st(load(length)))?;
        put(j, out, vec(j, dir_at)?)?;
        put(j, out.wrapping_add(gjk::contact_field::POINT), vec(j, t)?)?;
        set(j, out.wrapping_add(gjk::contact_field::WEIGHT), HALF_BITS)?;
        Ok(true)
    }

    /// `0x00309610`, thiscall on the state, `ret`, for one vertex: the
    /// supports along the first difference and against it; the one
    /// farther from it (toward it on a tie, against it when unordered)
    /// joins.
    fn grow_point(&mut self, j: &mut Journal, esp: u32) -> Result<(), Fallback> {
        // `push ebp; and esp,-16; sub esp,0x78`, two pushes.
        let base = (esp.wrapping_sub(4) & !15).wrapping_sub(0x80);
        let (toward, dir_at, against) = (base.wrapping_add(0x20), base.wrapping_add(0x10), base.wrapping_add(0x50));
        let diffs = word(j, self.at(state::DIFFERENCES))?;
        self.support(j, diffs, toward, base.wrapping_sub(0xc))?;
        let d0 = vec(j, diffs)?;
        let near = self.spread(j, toward, d0)?;
        put(j, dir_at, d0 ^ vec(j, SIGNS)?)?;
        self.support(j, dir_at, against, base.wrapping_sub(0xc))?;
        let d0 = vec(j, diffs)?;
        let far = self.spread(j, against, d0)?;
        let pick = if matches!(cmp(load(near), load(far)), Cmp::Less | Cmp::Unordered) { against } else { toward };
        self.append(j, pick)
    }

    /// `0x00309700`, thiscall on the state, `ret`, for two vertices: a
    /// perpendicular of the edge `u = D0 - D1` (the smallest of `|u|`'s
    /// lanes zero, the other two swapped, the first negated), and `u`
    /// crossed with it, each unit (one Newton step on `rsqrtss`); of the
    /// two [`tri_support`]s, the higher (the second on a tie or
    /// unordered) joins.
    fn grow_segment(&mut self, j: &mut Journal, esp: u32) -> Result<(), Fallback> {
        // `push ebp; and esp,-16; sub esp,0xb8`, two pushes.
        let base = (esp.wrapping_sub(4) & !15).wrapping_sub(0xc0);
        let (side_at, cross_at) = (base.wrapping_add(0x30), base.wrapping_add(0x10));
        let (first, second) = (base.wrapping_add(0x40), base.wrapping_add(0x80));
        let u = self.sse.sub(self.difference(j, 0)?, self.difference(j, 1)?);
        // The perpendicular's `w` is the frame's word, never stored.
        let p = perpendicular(&self.x, u, word(j, side_at.wrapping_add(12))?);
        let c = cross(&mut self.sse, u, p);
        let side = self.sse.normalize(p);
        put(j, side_at, side)?;
        let across = self.sse.normalize(c);
        put(j, cross_at, across)?;
        let diffs = word(j, self.at(state::DIFFERENCES))?;
        self.tri_support(j, side_at, diffs, first, base.wrapping_sub(0x10))?;
        self.tri_support(j, cross_at, diffs, second, base.wrapping_sub(0x10))?;
        let (h1, h2) = (word(j, first.wrapping_add(triple::HEIGHT))?, word(j, second.wrapping_add(triple::HEIGHT))?);
        // `test ah,0x41; jne`: the first only when above.
        let pick = if cmp(load(h1), load(h2)) == Cmp::Greater { first } else { second };
        self.append(j, pick)
    }

    /// `0x0030a162`: a flat triangle after a grown segment. `out` gets
    /// the distance 0, the edge's unit perpendicular as the normal (its
    /// `w` as `out` held it), and the point `(1 - k) T0 + k T1` on A's
    /// first two vertices, `k = a / ((a + a) + SPREAD)` with `a` the
    /// first difference's taxicab length (x87), weight `0.5`; eax 0.
    fn line(&mut self, j: &mut Journal, out: u32) -> Result<u32, Fallback> {
        set(j, out.wrapping_add(gjk::contact_field::DISTANCE), 0)?;
        let u = self.sse.sub(self.difference(j, 0)?, self.difference(j, 1)?);
        let (hi, zero_at, keep_at) = order(u);
        set(j, out.wrapping_add(4 * zero_at as u32), 0)?;
        set(j, out.wrapping_add(4 * hi as u32), lane(u, keep_at))?;
        set(j, out.wrapping_add(4 * keep_at as u32), self.x.st(load(lane(u, hi)).neg()))?;
        let n = vec(j, out)?;
        let unit = self.sse.normalize(n);
        put(j, out, unit)?;
        let d0 = self.difference(j, 0)? & vec(j, ABS)?;
        let a = self.sse.hsum(d0);
        let den = self.x.add(self.x.add(load(a), load(a)), load(word(j, SPREAD)?));
        let k = self.x.st(self.x.div(load(a), den));
        let t = self.simplex(j)?;
        let t1 = vec(j, t.wrapping_add(0x10))?;
        let near = self.sse.mul(splat(k), t1);
        let rest = self.sse.sub(vec(j, ONES)?, splat(k));
        let far = self.sse.mul(rest, vec(j, t)?);
        let point = self.sse.add(far, near);
        put(j, out.wrapping_add(gjk::contact_field::POINT), point)?;
        set(j, out.wrapping_add(gjk::contact_field::WEIGHT), HALF_BITS)?;
        Ok(0)
    }

    /// `0x0030a2f3`: the triangle's support is below [`SLACK`] over it.
    /// `out` gets the distance 0, the unit normal, and the point
    /// `(w0 T0 + w1 T1) + w2 T2` on A's vertices by [`barycentric`]'s
    /// weights of [`SAME`] on the differences' triangle, weight `0.5`;
    /// eax 0.
    fn plane(&mut self, j: &mut Journal, out: u32, l: u32) -> Result<u32, Fallback> {
        let n_at = l.wrapping_add(0x60);
        let unit = self.sse.normalize(vec(j, n_at)?);
        set(j, out.wrapping_add(gjk::contact_field::DISTANCE), 0)?;
        put(j, out, unit)?;
        put(j, n_at, unit)?;
        let diffs = word(j, self.at(state::DIFFERENCES))?;
        let weights = l.wrapping_add(0x20);
        barycentric(j, SAME, diffs, diffs.wrapping_add(0x10), diffs.wrapping_add(0x20), weights, self.fpu)?;
        let w = |i: u32| word(j, weights.wrapping_add(4 * i));
        let (w0, w1, w2) = (w(0)?, w(1)?, w(2)?);
        let t = self.simplex(j)?;
        let z = self.sse.mul(splat(w2), vec(j, t.wrapping_add(0x20))?);
        let y = self.sse.mul(splat(w1), vec(j, t.wrapping_add(0x10))?);
        let x = self.sse.mul(splat(w0), vec(j, t)?);
        let xy = self.sse.add(x, y);
        let point = self.sse.add(xy, z);
        put(j, out.wrapping_add(gjk::contact_field::POINT), point)?;
        set(j, out.wrapping_add(gjk::contact_field::WEIGHT), HALF_BITS)?;
        Ok(0)
    }

    fn run(&mut self, j: &mut Journal, out: u32, esp: u32) -> Result<u32, Fallback> {
        // `push ebp; and esp,-16`, the handler frame (three pushes), ecx,
        // `_alloca(0x3004)`, three pushes.
        let l = (esp.wrapping_sub(4) & !15).wrapping_sub(0x3020);
        let (slack, tiny, zero) = (load(word(j, SLACK)?), load(word(j, TINY)?), load(word(j, ZERO)?));
        let mut edi: i32 = -1;
        let mut case = match self.count(j)? {
            0 => Case::Point0,
            1 => Case::Point,
            2 => Case::Segment,
            3 => Case::Triangle,
            4 => Case::Tetrahedron,
            // `ja 0x00309e2e`: the polytope.
            _ => return Err(Fallback::Closest),
        };
        loop {
            case = match case {
                // 0x00309b9c: `(D3 - D0) . ((D0 - D1) x (D1 - D2))`.
                Case::Tetrahedron => {
                    let (d0, d1, d2, d3) = (self.difference(j, 0)?, self.difference(j, 1)?, self.difference(j, 2)?, self.difference(j, 3)?);
                    let u = self.sse.sub(d0, d1);
                    let v = self.sse.sub(d1, d2);
                    let n = cross(&mut self.sse, u, v);
                    let e = self.sse.sub(d3, d0);
                    let m = self.sse.mul(e, n);
                    let volume = self.sse.hsum(m);
                    if above(cmp(load(volume).abs(), slack)) {
                        // 0x00309e2e: the expanding polytope.
                        return Err(Fallback::Closest);
                    }
                    if edi >= 3 {
                        return Ok(2);
                    }
                    self.dec(j)?;
                    Case::Triangle
                }
                // 0x00309c29: `(D0 - D1) x (D1 - D2)`, `fucompp` with 0.
                Case::Triangle => {
                    let (d0, d1, d2) = (self.difference(j, 0)?, self.difference(j, 1)?, self.difference(j, 2)?);
                    let u = self.sse.sub(d0, d1);
                    let v = self.sse.sub(d1, d2);
                    let n = cross(&mut self.sse, u, v);
                    put(j, l.wrapping_add(0x60), n)?;
                    let nn = self.square(n);
                    if cmp(load(nn), zero) != Cmp::Equal {
                        edi = 3;
                        let t = l.wrapping_add(0xb0);
                        let diffs = word(j, self.at(state::DIFFERENCES))?;
                        self.tri_support(j, l.wrapping_add(0x60), diffs, t, l.wrapping_sub(0x10))?;
                        let height = word(j, t.wrapping_add(triple::HEIGHT))?;
                        // `test ah,5; jnp`: below only.
                        if cmp(load(height), slack) == Cmp::Less {
                            return self.plane(j, out, l);
                        }
                        self.append(j, t)?;
                        Case::Tetrahedron
                    } else if edi >= 2 {
                        return self.line(j, out);
                    } else {
                        self.dec(j)?;
                        Case::Segment
                    }
                }
                // 0x00309cb4: `|D0 - D1|^2` against TINY.
                Case::Segment => {
                    let u = self.sse.sub(self.difference(j, 0)?, self.difference(j, 1)?);
                    let q = self.square(u);
                    if above(cmp(load(q), tiny)) {
                        edi = 2;
                        self.grow_segment(j, l.wrapping_sub(4))?;
                        Case::Triangle
                    } else if edi >= 2 {
                        return Ok(2);
                    } else {
                        self.dec(j)?;
                        Case::Point
                    }
                }
                // 0x00309d09: `|D0|^2` against TINY.
                Case::Point => {
                    let d0 = self.difference(j, 0)?;
                    let q = self.square(d0);
                    if above(cmp(load(q), tiny)) {
                        edi = 1;
                        self.grow_point(j, l.wrapping_sub(4))?;
                        Case::Segment
                    } else if edi >= 0 {
                        return Ok(2);
                    } else {
                        self.dec(j)?;
                        Case::Point0
                    }
                }
                // 0x00309d52.
                Case::Point0 => {
                    if self.first(j, out, l.wrapping_sub(0xc))? {
                        return Ok(0);
                    }
                    edi = 0;
                    Case::Point
                }
            };
        }
    }
}

/// `0x00309700`'s and `0x0030a162`'s lane order on `|u|` (x87 `fabs`,
/// `fcomp`): `(hi, zero, keep)`, `zero` the smallest lane (`y` before
/// `x` only when below it, `z` only when below both).
fn order(u: u128) -> (usize, usize, usize) {
    let mag = |i: usize| load(lane(u, i)).abs();
    let (mut hi, mut zero, mut keep) = (1usize, 0usize, 2usize);
    let mut least: F80 = mag(0);
    if cmp(mag(1), mag(0)) == Cmp::Less {
        (hi, zero) = (0, 1);
        least = mag(1);
    }
    if cmp(mag(2), least) == Cmp::Less {
        keep = zero;
        zero = 2;
    }
    (hi, zero, keep)
}

/// `0x00309700`'s perpendicular of `u`: lane `zero` 0, lane `hi` gets
/// `u`'s lane `keep`, lane `keep` gets `-u[hi]` through the x87; `w` as
/// given.
fn perpendicular(x: &Fx, u: u128, w: u32) -> u128 {
    let (hi, zero, keep) = order(u);
    let mut p = [0u32; 4];
    p[3] = w;
    p[zero] = 0;
    p[hi] = lane(u, keep);
    p[keep] = x.st(load(lane(u, hi)).neg());
    p.iter().enumerate().fold(0, |v, (i, &l)| v | u128::from(l) << (32 * i))
}

/// `0x00309b50`, thiscall on the [`state`] `s`, `(out)`, `ret 4`; `esp`
/// is the stack pointer at entry. The case for the simplex count (the
/// table at `0x0030a698`) runs, each falling to the one below when its
/// simplex is flat and growing the simplex by a support point when not:
///
/// | Count | Flat when | Grows by |
/// |---|---|---|
/// | 4 | `\|volume\|` not above [`SLACK`] | (not flat: the polytope, declined) |
/// | 3 | the normal's square is 0 | `0x003093e0` along the normal; below [`SLACK`] over the plane, the plane exit |
/// | 2 | `\|D0 - D1\|^2` not above [`TINY`] | `0x00309700` |
/// | 1 | `\|D0\|^2` not above [`TINY`] | `0x00309610` |
/// | 0 | | `0x003094d0`, which may end with 0 |
///
/// A flat simplex already grown past its size returns 2, but a flat
/// triangle after a grown segment takes the line exit. The exits store
/// `out`'s normal, point, distance and weight and return 0.
pub fn solve(j: &mut Journal, s: u32, out: u32, esp: u32, fpu: Fpu, calls: &mut impl Calls) -> Result<u32, Fallback> {
    let (sse, x) = units(fpu)?;
    let mut r = Solver { s, fpu, sse, x, calls };
    let eax = r.run(j, out, esp)?;
    done(&r.sse)?;
    Ok(eax)
}

/// `0x00308260`, cdecl `(p, a, b, c, out)`: the weights of `p` on the
/// triangle `abc` to `out` (three floats, `a`'s first). With
/// `e1 = a - b`, `e2 = c - b` and the determinant
/// `|e2|^2 |e1|^2 - (e2 . e1)^2` (x87) above 0, the plane solve; else
/// the longest edge's projection, `e3 = a - c` included, its far vertex
/// 0 (all three 0 when the longest is `e3` and its square is not above 0).
pub fn barycentric(j: &mut Journal, p: u32, a: u32, b: u32, c: u32, out: u32, fpu: Fpu) -> Result<(), Fallback> {
    let (mut sse, x) = units(fpu)?;
    let zero = load(word(j, ZERO)?);
    let one = load(word(j, ONE)?);
    let bv = vec(j, b)?;
    let av = vec(j, a)?;
    let cv = vec(j, c)?;
    let e1 = sse.sub(av, bv);
    let m = sse.mul(e1, e1);
    let l1 = sse.hsum(m);
    let e2 = sse.sub(cv, bv);
    let m = sse.mul(e2, e2);
    let l2 = sse.hsum(m);
    let ll = x.mul(load(l2), load(l1));
    let m = sse.mul(e2, e1);
    let dot = sse.hsum(m);
    let dd = x.mul(load(dot), load(dot));
    let det = x.sub(ll, dd);
    let det32 = x.st(det);
    let at = |k: u32| out.wrapping_add(4 * k);
    if cmp(det, zero) == Cmp::Greater {
        let inv = x.div(one, load(det32));
        let w = sse.sub(bv, vec(j, p)?);
        let m = sse.mul(w, e1);
        let d1 = sse.hsum(m);
        let m = sse.mul(w, e2);
        let d2 = sse.hsum(m);
        done(&sse)?;
        let s = x.sub(x.mul(load(d2), load(dot)), x.mul(load(d1), load(l2)));
        let t = x.sub(x.mul(load(dot), load(d1)), x.mul(load(d2), load(l1)));
        let t32 = x.st(t);
        set(j, at(0), x.st(x.mul(s, inv)))?;
        let rest = x.sub(x.sub(load(det32), s), load(t32));
        set(j, at(1), x.st(x.mul(rest, inv)))?;
        return set(j, at(2), x.st(x.mul(load(t32), inv)));
    }
    let e3 = sse.sub(av, cv);
    let m = sse.mul(e3, e3);
    let l3 = sse.hsum(m);
    // `test ah,1; jne`: below, or unordered.
    let below = |u: u32, v: u32| matches!(cmp(load(u), load(v)), Cmp::Less | Cmp::Unordered);
    // The edge, its square, the vertex it is measured from, which weight
    // is zero, and which takes `r` and which `1 - r`.
    let (edge, square, from, none, near, far) = if !below(l1, l2) {
        if below(l3, l1) {
            (e1, l1, bv, 2, 0, 1)
        } else {
            set(j, at(1), 0)?;
            if cmp(load(l3), zero) != Cmp::Greater {
                set(j, at(0), 0)?;
                done(&sse)?;
                return set(j, at(2), 0);
            }
            (e3, l3, cv, 1, 0, 2)
        }
    } else if below(l3, l2) {
        (e2, l2, bv, 0, 2, 1)
    } else {
        (e3, l3, cv, 1, 0, 2)
    };
    let off = sse.sub(vec(j, p)?, from);
    let m = sse.mul(off, edge);
    let d = sse.hsum(m);
    done(&sse)?;
    let r = x.div(load(d), load(square));
    set(j, at(none), 0)?;
    set(j, at(near), x.st(r))?;
    set(j, at(far), x.st(x.sub(one, r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied().or(Some(0))
        }
    }

    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn put4(h: &mut Heap, at: u32, v: [f32; 4]) {
        for (i, x) in v.into_iter().enumerate() {
            h.0.insert(at + 4 * i as u32, x.to_bits());
        }
    }

    fn weights(j: &Journal, at: u32) -> [f32; 3] {
        std::array::from_fn(|i| f32::from_bits(j.get(at + 4 * i as u32).unwrap()))
    }

    fn heap() -> Heap {
        let mut h = Heap::default();
        h.0.insert(ZERO, 0);
        h.0.insert(ONE, 1f32.to_bits());
        h
    }

    #[test]
    fn a_point_inside_a_triangle_weighs_its_vertices() {
        let mut h = heap();
        put4(&mut h, 0x100, [0.25, 0.25, 0.0, 0.0]);
        put4(&mut h, 0x110, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x120, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x130, [0.0, 1.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        barycentric(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(weights(&j, 0x200), [0.25, 0.5, 0.25]);
    }

    #[test]
    fn a_flat_triangle_projects_on_its_longest_edge() {
        let mut h = heap();
        put4(&mut h, 0x100, [0.5, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x110, [2.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x120, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x130, [1.0, 0.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        barycentric(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        // e1 = a - b is the longest: a weighs 0.25, b the rest, c none.
        assert_eq!(weights(&j, 0x200), [0.25, 0.75, 0.0]);
    }

    #[test]
    fn a_point_triangle_weighs_nothing() {
        let mut h = heap();
        for at in [0x110, 0x120, 0x130] {
            put4(&mut h, at, [1.0, 1.0, 1.0, 0.0]);
        }
        h.0.insert(0x200, 1f32.to_bits());
        let mut j = Journal::new(&h);
        barycentric(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(weights(&j, 0x200), [0.0; 3]);
    }

    #[test]
    fn the_lane_order_zeroes_the_smallest() {
        let x = Fx { mode: x87::Mode::from_cw(FPU.control) };
        let v = |a: [f32; 4]| a.iter().enumerate().fold(0u128, |v, (i, f)| v | u128::from(f.to_bits()) << (32 * i));
        assert_eq!(order(v([3.0, 2.0, 1.0, 0.0])), (0, 2, 1));
        assert_eq!(order(v([1.0, 2.0, 3.0, 0.0])), (1, 0, 2));
        assert_eq!(order(v([2.0, 1.0, 3.0, 0.0])), (0, 1, 2));
        let p = perpendicular(&x, v([1.0, 2.0, 3.0, 0.0]), 0);
        // x is the smallest: (0, z, -y).
        assert_eq!(p, v([0.0, 3.0, -2.0, 0.0]));
    }
}
