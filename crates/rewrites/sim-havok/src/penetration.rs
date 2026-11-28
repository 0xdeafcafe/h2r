//! The triangle case's penetration call (docs/engine/havok-convex.md,
//! "The penetration call"): the profiled wrapper `0x0030a6b0` and its
//! solver `0x00309b50` with its expanding polytope, each one guest
//! function on a [`Journal`], every float operation one guest
//! instruction: packed and scalar SSE under the MXCSR, x87 under the
//! control word.
//!
//! - [`penetration`] `0x0030a6b0`: the solver's state built on the
//!   frame, then the solver.
//! - [`solve`] `0x00309b50`: the simplex grown to a tetrahedron around
//!   the origin of `A - B`, with its exits; a tetrahedron that is not
//!   flat goes on to the expanding polytope (`0x00309e2e`), whose faces,
//!   vertices and free list are the solver's frame objects and live in
//!   Rust here, and whose failed rounds retry the cases.
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
        // A retry reruns the cases from the jump table (`0x00309b80`);
        // the retry count caps them at twenty.
        loop {
            if let Some(eax) = self.cases(j, out, l)? {
                return Ok(eax);
            }
            if let Some(eax) = self.polytope(j, out, l)? {
                return Ok(eax);
            }
        }
    }

    /// The jump table's cases from `0x00309b80`: `Some` an exit's eax,
    /// `None` the expanding polytope.
    fn cases(&mut self, j: &mut Journal, out: u32, l: u32) -> Result<Option<u32>, Fallback> {
        let (slack, tiny, zero) = (load(word(j, SLACK)?), load(word(j, TINY)?), load(word(j, ZERO)?));
        let mut edi: i32 = -1;
        let mut case = match self.count(j)? {
            0 => Case::Point0,
            1 => Case::Point,
            2 => Case::Segment,
            3 => Case::Triangle,
            4 => Case::Tetrahedron,
            // `ja 0x00309e2e`: the polytope.
            _ => return Ok(None),
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
                        return Ok(None);
                    }
                    if edi >= 3 {
                        return Ok(Some(2));
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
                            return self.plane(j, out, l).map(Some);
                        }
                        self.append(j, t)?;
                        Case::Tetrahedron
                    } else if edi >= 2 {
                        return self.line(j, out).map(Some);
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
                        return Ok(Some(2));
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
                        return Ok(Some(2));
                    } else {
                        self.dec(j)?;
                        Case::Point0
                    }
                }
                // 0x00309d52.
                Case::Point0 => {
                    if self.first(j, out, l.wrapping_sub(0xc))? {
                        return Ok(Some(0));
                    }
                    edi = 0;
                    Case::Point
                }
            };
        }
    }
}

/// The polytope's constants: the convergence floor of each vertex's
/// height over the nearest face (`0.001` in every lane), the step a
/// face's height adds to the visibility margin (`1e-5`), the jitter's
/// integer scale (`2^-31`), the walk's edge test (`-1e-4`) and its
/// neighbour ratio (`0.9`).
pub const CONVERGED: u32 = 0x0046_1f20;
pub const MARGIN_STEP: u32 = 0x0041_36f0;
pub const JITTER_SCALE: u32 = 0x0041_36d0;
pub const EDGE_SIDE: u32 = 0x0045_ddfc;
pub const NEIGHBOUR: u32 = 0x0045_dc84;
const MAX_BITS: u32 = 0x7f7f_ffff;
/// The visibility margin each expansion starts from, `1e-5`.
const MARGIN_BITS: u32 = 0x3727_c5ac;
/// The jitter's range, `-1e-4` to `1e-4`, pushed as immediates.
const JITTER_LO: u32 = 0xb8d1_b717;
const JITTER_HI: u32 = 0x38d1_b717;
/// The inline capacities of the polytope's arrays: faces (and the free
/// list), vertices (and the two point arrays).
const MAX_FACES: usize = 100;
const MAX_VERTICES: usize = 55;
/// The solver's frame offsets the polytope uses: the polytope object
/// (its faces at `+0x1c0`, `0x50` bytes each), the support triple, the
/// weights and the jitter.
const POLYTOPE: u32 = 0x820;
const FACES: u32 = 0x1c0;
const FACE_BYTES: u32 = 0x50;
const SUPPORT: u32 = 0x80;
const WEIGHTS: u32 = 0x34;
const JITTER: u32 = 0x40;

/// `0x00413700`: the starting tetrahedron's faces, each edge as (start
/// vertex, twin face, twin edge).
const TETRAHEDRON: [[(usize, usize, usize); 3]; 4] = [
    [(0, 2, 2), (2, 3, 0), (1, 1, 0)],
    [(0, 0, 2), (1, 3, 2), (3, 2, 0)],
    [(0, 1, 2), (3, 3, 1), (2, 0, 0)],
    [(1, 0, 1), (2, 2, 1), (3, 1, 1)],
];

/// A face's edge: the face and the edge's index in it. Edge `k` runs
/// from vertex `k` to vertex `k + 1` (mod 3).
type Edge = (usize, usize);

/// One `0x50`-byte face: the unit normal, the plane's distance from the
/// origin, the edges' start vertices and twins, and the flag byte at
/// `+0x44` (1 visible, 2 not, 3 walked, 4 inconsistent).
#[derive(Clone, Copy, Default)]
struct Face {
    normal: u128,
    distance: u32,
    vertex: [usize; 3],
    twin: [Edge; 3],
    flag: u8,
}

/// A `0x20`-byte vertex: the difference `A - B`, and the horizon edge
/// that starts at it (`+0x10`) while an expansion runs.
#[derive(Clone, Copy)]
struct Vertex {
    point: u128,
    horizon: Option<Edge>,
}

/// The polytope object the solver builds at `esp+0x820` (`0x00309a50`):
/// the visibility margin and the nearest visible height (`+0x00`,
/// `+0x04`), the free list of face slots (`+0x08`), the faces (`+0x1b0`)
/// and the vertices (`+0x2100`), all on the solver's frame and dead at
/// its return, so they live here.
struct Polytope {
    margin: u32,
    nearest: u32,
    free: Vec<usize>,
    faces: Vec<Face>,
    vertices: Vec<Vertex>,
}

/// The polytope's steps (`0x00309e2e` to `0x0030a150`, `0x0030a440` to
/// `0x0030a694`, and their calls).
impl<C: Calls> Solver<'_, C> {
    /// `0x0030a9d0`: the face's unit normal `(V0 - V1) x (V1 - V2)` and
    /// its distance `n . V0`.
    fn face_plane(&mut self, p: &mut Polytope, f: usize) {
        let [a, b, c] = p.faces[f].vertex.map(|v| p.vertices[v].point);
        let e0 = self.sse.sub(a, b);
        let e1 = self.sse.sub(b, c);
        let n = cross(&mut self.sse, e0, e1);
        let unit = self.sse.normalize(n);
        let m = self.sse.mul(unit, a);
        p.faces[f].distance = self.sse.hsum(m);
        p.faces[f].normal = unit;
    }

    /// `0x00309a50` with `0x0030ad20`: the tetrahedron on the four
    /// differences, its vertices 0 and 1 swapped when vertex 0 lies above
    /// face 3, each face's plane.
    fn tetrahedron(&mut self, j: &Journal, d: [u128; 4]) -> Result<Polytope, Fallback> {
        let mut p = Polytope {
            margin: 0,
            nearest: 0,
            free: Vec::new(),
            faces: Vec::with_capacity(MAX_FACES),
            vertices: d.iter().map(|&point| Vertex { point, horizon: None }).collect(),
        };
        for edges in TETRAHEDRON {
            p.faces.push(Face {
                vertex: edges.map(|e| e.0),
                twin: edges.map(|e| (e.1, e.2)),
                ..Face::default()
            });
        }
        self.face_plane(&mut p, 3);
        let gap = self.sse.sub(d[0], d[1]);
        let m = self.sse.mul(gap, p.faces[3].normal);
        let s = self.sse.hsum(m);
        // `test ah,0x41; jne`: only above swaps.
        if cmp(load(s), load(word(j, ZERO)?)) == Cmp::Greater {
            for face in &mut p.faces {
                for v in &mut face.vertex {
                    *v = match *v {
                        0 => 1,
                        1 => 0,
                        other => other,
                    };
                }
            }
        }
        for f in 0..4 {
            self.face_plane(&mut p, f);
        }
        Ok(p)
    }

    /// `0x00309120`: the face with the least distance, the first on a tie
    /// (`fcomp; test ah,5; jp`: only an ordered lesser one replaces it).
    fn nearest_face(p: &Polytope) -> usize {
        let mut best = 0;
        for f in 1..p.faces.len() {
            if cmp(load(p.faces[f].distance), load(p.faces[best].distance)) == Cmp::Less {
                best = f;
            }
        }
        best
    }

    /// `0x0030a770`, thiscall on the face, `(point, margin, nearest)`:
    /// the point's height over the face. Above the margin: 1, the nearest
    /// lowered to it. Otherwise 2, the margin raised to the height plus
    /// [`MARGIN_STEP`] when that is above it. Then 4 when the margin is
    /// not below the nearest.
    fn visible(&mut self, j: &Journal, p: &mut Polytope, f: usize, point: u128) -> Result<u8, Fallback> {
        let v0 = p.vertices[p.faces[f].vertex[0]].point;
        let off = self.sse.sub(point, v0);
        let m = self.sse.mul(p.faces[f].normal, off);
        let h = self.sse.hsum(m);
        let r = if cmp(load(h), load(p.margin)) == Cmp::Greater {
            // `test ah,5; jp`: only below lowers it.
            if cmp(load(h), load(p.nearest)) == Cmp::Less {
                p.nearest = h;
            }
            1
        } else {
            let raised = self.x.add(load(h), load(word(j, MARGIN_STEP)?));
            if cmp(raised, load(p.margin)) == Cmp::Greater {
                p.margin = self.x.st(raised);
            }
            2
        };
        // `test ah,1; je`: 4 unless below or unordered.
        Ok(if matches!(cmp(load(p.margin), load(p.nearest)), Cmp::Less | Cmp::Unordered) { r } else { 4 })
    }

    /// `0x0030a880`, thiscall on the polytope, `(count, last, face,
    /// point)`, `ret 0x10`, for a visible face: each edge's twin face not
    /// yet tested is tested, and a visible one recursed into; a twin not
    /// visible makes the edge's twin a horizon edge (counted, the last
    /// kept, and stored at its start vertex). A 4, or a 2 from below,
    /// stops the walk with 2. The face is freed (distance `FLT_MAX`, its
    /// slot to the free list) either way.
    fn horizon(&mut self, j: &Journal, p: &mut Polytope, f: usize, point: u128, count: &mut i32, last: &mut Option<Edge>) -> Result<u32, Fallback> {
        let mut ebx = 0;
        for k in 0..3 {
            let twin = p.faces[f].twin[k];
            let g = twin.0;
            if p.faces[g].flag == 0 {
                let r = self.visible(j, p, g, point)?;
                p.faces[g].flag = r;
                if r == 1 {
                    ebx = self.horizon(j, p, g, point, count, last)?;
                    if ebx == 2 {
                        break;
                    }
                } else if r == 4 {
                    ebx = 2;
                    break;
                }
            }
            if p.faces[g].flag == 2 {
                let start = p.faces[g].vertex[twin.1];
                p.vertices[start].horizon = Some(twin);
                *count += 1;
                *last = Some(twin);
            }
        }
        p.faces[f].distance = MAX_BITS;
        if p.free.len() >= MAX_FACES {
            return Err(Fallback::Memory);
        }
        p.free.push(f);
        Ok(ebx)
    }

    /// `0x0030abe0`, thiscall on the polytope, `(count, last, face,
    /// point)`, `ret 0x10`: the margin to `1e-5`, the nearest to
    /// `FLT_MAX`, then the nearest face's test; visible, [`Self::horizon`]
    /// from it, else 2.
    fn expand_from(&mut self, j: &Journal, p: &mut Polytope, f: usize, point: u128, count: &mut i32, last: &mut Option<Edge>) -> Result<u32, Fallback> {
        (*count, *last) = (0, None);
        (p.margin, p.nearest) = (MARGIN_BITS, MAX_BITS);
        let r = self.visible(j, p, f, point)?;
        p.faces[f].flag = r;
        if r != 1 {
            return Ok(2);
        }
        self.horizon(j, p, f, point, count, last)
    }

    /// `0x0030a810`: every face's flag cleared, every vertex's horizon edge.
    fn clear(p: &mut Polytope) {
        for face in &mut p.faces {
            face.flag = 0;
        }
        for v in &mut p.vertices {
            v.horizon = None;
        }
    }

    /// `0x0030ae70`, thiscall on the polytope, `(count, last, point)`,
    /// `ret 0xc`: the point joins as a vertex; `count` face slots come
    /// from the free list's end (`0x0030a950`), then new ones; from the
    /// last horizon edge round the horizon (each edge's end vertex names
    /// the next), each slot becomes the face over the edge reversed and
    /// the new vertex, its twins the edge and its neighbours in the ring,
    /// with its plane (`0x0030ac50`). Flags and horizon edges are cleared.
    /// 0 when the ring closes on the edge it started from, else 2.
    fn add_vertex(&mut self, p: &mut Polytope, count: i32, last: Option<Edge>, point: u128) -> Result<u32, Fallback> {
        let v = p.vertices.len();
        p.vertices.push(Vertex { point, horizon: None });
        let mut ring = Vec::new();
        let mut left = count;
        while left > 0 {
            let Some(f) = p.free.pop() else { break };
            ring.push(f);
            left -= 1;
        }
        while left > 0 {
            p.faces.push(Face::default());
            ring.push(p.faces.len() - 1);
            left -= 1;
        }
        // The slot list is a 100-entry array on 0x0030ae70's frame.
        if ring.len() > MAX_FACES || p.faces.len() > MAX_FACES {
            return Err(Fallback::Memory);
        }
        let (mut edge, mut edi) = (last, 0);
        for i in 0..ring.len() {
            let Some(h) = edge else {
                edi = 2;
                break;
            };
            let (f, next) = (ring[i], ring[(i + 1) % ring.len()]);
            let (from, to) = (p.faces[h.0].vertex[h.1], p.faces[h.0].vertex[(h.1 + 1) % 3]);
            p.faces[f].vertex = [to, from, v];
            p.faces[h.0].twin[h.1] = (f, 0);
            p.faces[f].twin[0] = h;
            p.faces[next].twin[1] = (f, 2);
            p.faces[f].twin[2] = (next, 1);
            self.face_plane(p, f);
            edge = p.vertices[to].horizon.take();
        }
        Self::clear(p);
        Ok(if edge == last { edi } else { 2 })
    }

    /// `0x0030aac0`, thiscall on the polytope, `(face)`, `ret 4`: from the
    /// face, across the first edge (not the one come over) whose
    /// `(e x n) . V` is below [`EDGE_SIDE`], when the starting face's
    /// distance is not above 0 or its [`NEIGHBOUR`] share is not above
    /// the twin's; a face walked before ends the walk after the step.
    fn walk(&mut self, j: &Journal, p: &mut Polytope, start: usize) -> Result<usize, Fallback> {
        Self::clear(p);
        let (side, zero, share) = (load(word(j, EDGE_SIDE)?), load(word(j, ZERO)?), load(word(j, NEIGHBOUR)?));
        let (mut f, mut skip, mut stop) = (start, None, false);
        loop {
            if stop {
                return Ok(f);
            }
            p.faces[f].flag = 3;
            let mut moved = false;
            for k in 0..3 {
                if skip == Some((f, k)) {
                    continue;
                }
                let v = p.vertices[p.faces[f].vertex[k]].point;
                let w = p.vertices[p.faces[f].vertex[(k + 1) % 3]].point;
                let e = self.sse.sub(w, v);
                let c = cross(&mut self.sse, e, p.faces[f].normal);
                let m = self.sse.mul(c, v);
                let s = self.sse.hsum(m);
                // `test ah,5; jp`: only below goes on.
                if cmp(load(s), side) != Cmp::Less {
                    continue;
                }
                let twin = p.faces[f].twin[k];
                // `0x0030ab6a` reads the starting face's distance, not this one's.
                let d = load(p.faces[start].distance);
                let go = cmp(d, zero) != Cmp::Greater || cmp(self.x.mul(d, share), load(p.faces[twin.0].distance)) != Cmp::Greater;
                if go {
                    stop = p.faces[twin.0].flag == 3;
                    skip = Some(twin);
                    f = twin.0;
                    moved = true;
                    break;
                }
            }
            if !moved {
                return Ok(f);
            }
        }
    }

    /// `0x00309270`, cdecl `(seed, lo, hi, out)`: three lanes of a
    /// linear congruential sequence (`(0x3039 - s * 0x3e39b193) &
    /// 0x7fffffff`), each `(n 2^-31 + lo)(hi - lo)` on the x87; `w` is
    /// not written.
    fn jitter(&mut self, j: &mut Journal, seed: u32, out: u32) -> Result<(), Fallback> {
        let scale = load(word(j, JITTER_SCALE)?);
        let range = self.x.sub(load(JITTER_HI), load(JITTER_LO));
        let mut s = seed.wrapping_mul(0x3e39_b193);
        for lane_at in 0..3 {
            let n = 0x3039u32.wrapping_sub(s) & 0x7fff_ffff;
            s = n.wrapping_mul(0x3e39_b193);
            let f = self.x.mul(F80::from_i64(i64::from(n)), scale);
            let f = self.x.add(f, load(JITTER_LO));
            let f = self.x.mul(f, range);
            set(j, out.wrapping_add(4 * lane_at), self.x.st(f))?;
        }
        Ok(())
    }

    /// `0x00309e2e`: the expanding polytope on the four differences. Each
    /// round takes the nearest face's support along its normal; when the
    /// point is below [`CONVERGED`] over any of the face's vertices along
    /// it, the face answers. Otherwise the faces the point sees go, the
    /// horizon's new faces come, and the round repeats; a vertex or face
    /// array that would fill answers with 1. A walk or ring that fails
    /// retries the cases ([`Self::retry`]). `None` is a retry.
    fn polytope(&mut self, j: &mut Journal, out: u32, l: u32) -> Result<Option<u32>, Fallback> {
        let d = [self.difference(j, 0)?, self.difference(j, 1)?, self.difference(j, 2)?, self.difference(j, 3)?];
        let mut p = self.tetrahedron(j, d)?;
        let t = self.simplex(j)?;
        let view: &Journal = j;
        let mut a = (0..4u32).map(|i| vec(view, t.wrapping_add(16 * i))).collect::<Result<Vec<_>, _>>()?;
        let mut b = (0..4u32).map(|i| vec(view, t.wrapping_add(simplex::LOCAL_B + 16 * i))).collect::<Result<Vec<_>, _>>()?;
        let converged = vec(j, CONVERGED)?;
        let triple_at = l.wrapping_add(SUPPORT);
        loop {
            let f = Self::nearest_face(&p);
            // The support reads the direction from the face's own slot.
            let face_at = l.wrapping_add(POLYTOPE + FACES).wrapping_add(FACE_BYTES * f as u32);
            put(j, face_at, p.faces[f].normal)?;
            self.support(j, face_at, triple_at, l.wrapping_sub(0xc))?;
            let point = self.sse.sub(vec(j, triple_at)?, vec(j, triple_at.wrapping_add(triple::B))?);
            // 0x002da730: `(point - Vk) . n` for the three vertices, `w` the third's.
            let face = p.faces[f];
            let rows = face.vertex.map(|v| self.sse.sub(point, p.vertices[v].point));
            let column = |c: usize| {
                let k = [lane(rows[0], c), lane(rows[1], c), lane(rows[2], c), lane(rows[2], c)];
                k.iter().enumerate().fold(0u128, |v, (i, &w)| v | u128::from(w) << (32 * i))
            };
            let n = face.normal;
            let ys = self.sse.mul(column(1), splat(lane(n, 1)));
            let xs = self.sse.mul(column(0), splat(lane(n, 0)));
            let zs = self.sse.mul(column(2), splat(lane(n, 2)));
            let xy = self.sse.add(xs, ys);
            let heights = self.sse.add(xy, zs);
            if gjk::signs(gjk::below(&mut self.sse, heights, converged)) & 7 != 0 {
                return self.answer(j, &mut p, &a, &b, f, out, l, 0).map(Some);
            }
            let (mut count, mut last) = (0, None);
            if self.expand_from(j, &mut p, f, point, &mut count, &mut last)? != 0 {
                match self.retry(j, l)? {
                    Some(eax) => return self.answer(j, &mut p, &a, &b, f, out, l, eax).map(Some),
                    None => return Ok(None),
                }
            }
            // `setl`: room for a vertex, and for the new faces.
            let room = (MAX_FACES - p.faces.len() + p.free.len()) as i32;
            if p.vertices.len() >= MAX_VERTICES || room < count {
                return self.answer(j, &mut p, &a, &b, f, out, l, 1).map(Some);
            }
            let ring = self.add_vertex(&mut p, count, last, point)?;
            a.push(vec(j, triple_at)?);
            b.push(vec(j, triple_at.wrapping_add(triple::LOCAL_B))?);
            if ring != 0 {
                match self.retry(j, l)? {
                    Some(eax) => return self.answer(j, &mut p, &a, &b, f, out, l, eax).map(Some),
                    None => return Ok(None),
                }
            }
        }
    }

    /// `0x0030a07d`: the retry count up one. The first retry keeps one
    /// vertex; the next eighteen empty the simplex and move the
    /// translation by [`Self::jitter`] (seeded by the count); the
    /// twentieth answers with 2 (`Some`).
    fn retry(&mut self, j: &mut Journal, l: u32) -> Result<Option<u32>, Fallback> {
        let r = word(j, self.at(state::RETRIES))?.wrapping_add(1);
        set(j, self.at(state::RETRIES), r)?;
        let count_at = self.simplex(j)?.wrapping_add(simplex::COUNT);
        if r == 1 {
            set(j, count_at, 1)?;
            return Ok(None);
        }
        if r as i32 >= 0x14 {
            return Ok(Some(2));
        }
        set(j, count_at, 0)?;
        let at = l.wrapping_add(JITTER);
        self.jitter(j, r, at)?;
        let moved = self.sse.add(vec(j, at)?, vec(j, self.at(state::TRANSLATION))?);
        put(j, at, moved)?;
        put(j, self.at(state::TRANSLATION), moved)?;
        Ok(None)
    }

    /// `0x0030a440`: the face's vertices to the first three differences
    /// and [`barycentric`]'s weights of [`SAME`] on them; while a weight
    /// is below 0 the face moves by [`Self::walk`], ten times at most (the
    /// tenth face keeps the ninth's differences and weights). The face's
    /// A and B points become the simplex of three; `out` gets the
    /// normal negated, the point `(w0 A0 + w1 A1) + w2 A2`, the distance
    /// negated; its weight stands. Returns `eax`.
    #[allow(clippy::too_many_arguments)]
    fn answer(&mut self, j: &mut Journal, p: &mut Polytope, a: &[u128], b: &[u128], start: usize, out: u32, l: u32, eax: u32) -> Result<u32, Fallback> {
        let zero = load(word(j, ZERO)?);
        let diffs = word(j, self.at(state::DIFFERENCES))?;
        let weights = l.wrapping_add(WEIGHTS);
        let mut f = start;
        let mut walks = 0;
        loop {
            for k in 0..3u32 {
                let point = p.vertices[p.faces[f].vertex[k as usize]].point;
                put(j, diffs.wrapping_add(16 * k), point)?;
            }
            barycentric(j, SAME, diffs, diffs.wrapping_add(0x10), diffs.wrapping_add(0x20), weights, self.fpu)?;
            let mut inside = true;
            for k in 0..3 {
                // `test ah,5; jnp`: below only.
                if cmp(load(word(j, weights.wrapping_add(4 * k))?), zero) == Cmp::Less {
                    inside = false;
                    break;
                }
            }
            if inside {
                break;
            }
            f = self.walk(j, p, f)?;
            walks += 1;
            if walks >= 10 {
                break;
            }
        }
        let t = self.simplex(j)?;
        for k in 0..3u32 {
            let v = p.faces[f].vertex[k as usize];
            if v >= a.len() || v >= b.len() {
                return Err(Fallback::Memory);
            }
            put(j, t.wrapping_add(16 * k), a[v])?;
            put(j, t.wrapping_add(simplex::LOCAL_B + 16 * k), b[v])?;
        }
        set(j, t.wrapping_add(simplex::COUNT), 3)?;
        let w = |k: u32| word(j, weights.wrapping_add(4 * k));
        let (w0, w1, w2) = (w(0)?, w(1)?, w(2)?);
        let y = self.sse.mul(splat(w1), vec(j, t.wrapping_add(0x10))?);
        let z = self.sse.mul(splat(w2), vec(j, t.wrapping_add(0x20))?);
        let x = self.sse.mul(splat(w0), vec(j, t)?);
        let xy = self.sse.add(x, y);
        let point = self.sse.add(xy, z);
        put(j, out.wrapping_add(gjk::contact_field::POINT), point)?;
        let distance = self.x.st(load(p.faces[f].distance).neg());
        set(j, out.wrapping_add(gjk::contact_field::DISTANCE), distance)?;
        put(j, out, p.faces[f].normal ^ vec(j, SIGNS)?)?;
        Ok(eax)
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
/// | 4 | `\|volume\|` not above [`SLACK`] | (not flat: the expanding polytope) |
/// | 3 | the normal's square is 0 | `0x003093e0` along the normal; below [`SLACK`] over the plane, the plane exit |
/// | 2 | `\|D0 - D1\|^2` not above [`TINY`] | `0x00309700` |
/// | 1 | `\|D0\|^2` not above [`TINY`] | `0x00309610` |
/// | 0 | | `0x003094d0`, which may end with 0 |
///
/// A flat simplex already grown past its size returns 2, but a flat
/// triangle after a grown segment takes the line exit. The exits store
/// `out`'s normal, point, distance and weight and return 0. The
/// polytope answers 0, 1 when its arrays would fill, or 2 after twenty
/// retries, with `out`'s normal, point and distance.
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

    /// Shape 1 is the cube `[-1, 1]^3` (ties to `+1`), shape 2 the origin.
    struct Cube;

    impl Calls for Cube {
        fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, _: u32) -> Result<(), Fallback> {
            let d = vec(j, dir)?;
            for i in 0..4 {
                let v = if shape == 2 || i == 3 {
                    0.0f32
                } else if f32::from_bits(lane(d, i)) < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                set(j, out + 4 * i as u32, v.to_bits())?;
            }
            Ok(())
        }
    }

    /// The origin deep inside the cube: the tetrahedron of four of its
    /// corners grows to the cube, and the nearest face answers one unit
    /// out along an axis.
    #[test]
    fn a_point_inside_a_cube_is_one_unit_deep() {
        let mut h = heap();
        put4(&mut h, SIGNS, [-0.0; 4]);
        put4(&mut h, ABS, [f32::from_bits(0x7fff_ffff); 4]);
        put4(&mut h, ONES, [1.0; 4]);
        put4(&mut h, CONVERGED, [0.001; 4]);
        for (at, v) in [(SLACK, 0.001f32), (TINY, 1e-6), (MARGIN_STEP, 1e-5), (EDGE_SIDE, -1e-4), (NEIGHBOUR, 0.9), (JITTER_SCALE, f32::from_bits(0x3000_0000))] {
            h.0.insert(at, v.to_bits());
        }
        let (s, simplex_at, diffs, out, esp) = (0x1000, 0x2000, 0x3000, 0x4000, 0x10_0000);
        for (i, column) in [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0; 4]].into_iter().enumerate() {
            put4(&mut h, s + 16 * i as u32, column);
        }
        h.0.insert(s + state::SHAPE_A, 1);
        h.0.insert(s + state::SHAPE_B, 2);
        h.0.insert(s + state::SIMPLEX, simplex_at);
        h.0.insert(s + state::DIFFERENCES, diffs);
        for (i, corner) in [[1.0, 1.0, 1.0, 0.0], [1.0, -1.0, -1.0, 0.0], [-1.0, 1.0, -1.0, 0.0], [-1.0, -1.0, 1.0, 0.0]].into_iter().enumerate() {
            put4(&mut h, simplex_at + 16 * i as u32, corner);
            put4(&mut h, diffs + 16 * i as u32, corner);
        }
        h.0.insert(simplex_at + simplex::COUNT, 4);
        let mut j = Journal::new(&h);
        assert_eq!(solve(&mut j, s, out, esp, FPU, &mut Cube), Ok(0));
        let normal: [f32; 4] = std::array::from_fn(|i| f32::from_bits(j.get(out + 4 * i as u32).unwrap()));
        let distance = f32::from_bits(j.get(out + gjk::contact_field::DISTANCE).unwrap());
        assert!((distance + 1.0).abs() < 1e-3, "{distance}");
        let axis = normal[..3].iter().filter(|c| (c.abs() - 1.0).abs() < 1e-3).count();
        let rest = normal[..3].iter().filter(|c| c.abs() < 1e-3).count();
        assert_eq!((axis, rest), (1, 2), "{normal:?}");
        assert_eq!(j.get(simplex_at + simplex::COUNT), Some(3));
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
