//! The leaves of the convex agent's closest-feature query
//! (docs/engine/havok-convex.md, "The query path"), each one guest
//! function on a [`Journal`], every float operation one guest
//! instruction: packed and scalar SSE under the MXCSR, x87 under the
//! control word.
//!
//! - [`relative`] `0x002da4d0`: B's transform in A's frame.
//! - [`rotate`] `0x002da6e0`: a vector by a transform's rotation.
//! - [`triangle`] `0x00303360`: a point against a triangle's three edge
//!   planes, with the triangle's normal.
//! - [`segments`] `0x00308590`: the closest points of two segments.
//! - [`contact`] `0x003032c0`: the contact from the query's final state.
//! - [`tetrahedron`] `0x00303540` with `0x00303110`: which face of a
//!   tetrahedron the point lies beyond.
//! - [`edge`] `0x003037a0`: a segment against a triangle, which of the
//!   two simplices loses a vertex.
//! - [`query`] `0x003048a0`: the query itself, on the leaves above, with
//!   the shapes' virtuals and the triangle case through [`Calls`].
//! - [`triangle_case`] `0x00303dd0` with `0x00303b80`: the stalled
//!   query's penetration step, its call `0x0030a6b0` through
//!   [`Penetration`], then the query's cases on what it leaves.
//!
//! On a [`Fallback`] the journal is discarded and nothing has been
//! written.
use crate::convex::Fallback;
use crate::fp::{Arith, Exact};
use crate::jacobian::{lane, put_vector, splat, vector, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::load;
use x87::{Cmp, F80, Mode};

/// `0.0`, `1.0`, the segments' parallel threshold (`2^-20`) and the
/// sign mask `xorps` flips a normal with.
pub const ZERO: u32 = 0x0045_dbd8;
pub const ONE: u32 = 0x0045_dbc0;
pub const PARALLEL: u32 = 0x0045_dd4c;
pub const SIGNS: u32 = 0x0041_24a0;
/// The `andps` mask that clears the sign bits, and [`tetrahedron`]'s two
/// thresholds (`1e-6`, `1.1`).
pub const ABS: u32 = 0x0045_5940;
pub const TINY: u32 = 0x0045_df1c;
pub const MARGIN: u32 = 0x0045_dc80;
/// `1.0` in every lane: [`edge`]'s `1 - t`.
pub const ONES: u32 = 0x0041_2420;

/// The query state's words [`triangle`] writes: the normal and the three
/// edge tests; and the two support points it compares the normal with.
pub mod state {
    pub const SUPPORT_A: u32 = 0x20;
    pub const SUPPORT_B: u32 = 0xa0;
    pub const NORMAL: u32 = 0x120;
    pub const SIDES: u32 = 0x160;
    /// The two simplex sizes, and the point [`super::contact`] takes when
    /// neither is one.
    pub const COUNT_A: u32 = 0x00;
    pub const COUNT_B: u32 = 0x04;
    pub const MIDPOINT: u32 = 0x130;
    /// The word [`super::edge`] sets when the segment drops to its
    /// second vertex.
    pub const FEATURE: u32 = 0x10;
    /// [`super::query`]'s other words: B's count when the loop last left
    /// a case, the byte that says a stalled query ends without the
    /// triangle case, A's parallel array, and B's vertices in B's own
    /// frame (`SUPPORT_B` holds them in A's).
    pub const LAST_B: u32 = 0x08;
    pub const SETTLE: u32 = 0x0c;
    pub const PARALLEL_A: u32 = 0x60;
    pub const LOCAL_B: u32 = 0xe0;
}

/// The feature cache [`query`] seeds from and writes back: the vertex IDs
/// (16 bits each, A's then B's), the two simplex sizes and the largest
/// each may grow to, signed bytes.
pub mod cache {
    pub const IDS: u32 = 0x00;
    pub const COUNT_A: u32 = 0x08;
    pub const COUNT_B: u32 = 0x09;
    pub const MAX_A: u32 = 0x0a;
    pub const MAX_B: u32 = 0x0b;
}

/// [`contact`]'s fields in its output.
pub mod contact_field {
    pub const NORMAL: u32 = 0x00;
    pub const POINT: u32 = 0x10;
    pub const DISTANCE: u32 = 0x20;
    pub const WEIGHT: u32 = 0x24;
}

/// `shufps d, s, imm`: two lanes of `d`, then two of `s`.
fn shuf2(d: u128, s: u128, imm: u8) -> u128 {
    let pick = |v: u128, sel: u8| u128::from(lane(v, usize::from(sel & 3)));
    pick(d, imm) | pick(d, imm >> 2) << 32 | pick(s, imm >> 4) << 64 | pick(s, imm >> 6) << 96
}

/// `shufps v, v, imm`.
pub(crate) fn shuf1(v: u128, imm: u8) -> u128 {
    shuf2(v, v, imm)
}

/// `movmskps`: the four sign bits.
pub(crate) fn signs(v: u128) -> u32 {
    (0..4).fold(0, |m, i| m | (lane(v, i) >> 31) << i)
}

pub(crate) fn vec(j: &Journal, at: u32) -> Result<u128, Fallback> {
    vector(j, at).ok_or(Fallback::Memory)
}

/// `movaps` store: 16-byte aligned, or the guest would fault.
pub(crate) fn put(j: &mut Journal, at: u32, v: u128) -> Result<(), Fallback> {
    if at & 15 != 0 {
        return Err(Fallback::Memory);
    }
    put_vector(j, at, v).ok_or(Fallback::Memory)
}

pub(crate) fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

pub(crate) fn units(fpu: Fpu) -> Result<(Sse, Fx), Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    Ok((Sse { e: Exact::new(fpu.mxcsr) }, Fx { mode: Mode::from_cw(fpu.control) }))
}

pub(crate) fn done(sse: &Sse) -> Result<(), Fallback> {
    if sse.e.trapped { Err(Fallback::Float) } else { Ok(()) }
}

/// The transposed rotation of the transform at `m`: three rows, each
/// with its `w` lane the row's `z` (`0x002da4d0`'s shuffles).
pub(crate) fn rows(j: &Journal, m: u32) -> Result<[u128; 3], Fallback> {
    let c2 = vec(j, m.wrapping_add(0x20))?;
    let c0 = vec(j, m)?;
    let c1 = vec(j, m.wrapping_add(0x10))?;
    let lo = shuf2(c0, c1, 0x44);
    let hi = shuf2(c0, c1, 0xee);
    let (c2lo, c2hi) = (shuf1(c2, 0x44), shuf1(c2, 0xee));
    Ok([shuf2(lo, c2lo, 0x88), shuf2(lo, c2lo, 0xdd), shuf2(hi, c2hi, 0x88)])
}

/// `(r0 x + r1 y) + r2 z`, each product row times the splat.
pub(crate) fn combine(sse: &mut Sse, r: &[u128; 3], v: u128) -> u128 {
    let z = sse.mul(r[2], splat(lane(v, 2)));
    let y = sse.mul(r[1], splat(lane(v, 1)));
    let x = sse.mul(r[0], splat(lane(v, 0)));
    let xy = sse.add(x, y);
    sse.add(xy, z)
}

/// `0x002da4d0`, thiscall on `out`, `ret 8`: each of B's four columns
/// (the translation included) by A's transposed rotation, then A's
/// translation, rotated the same way, taken from the fourth. Memory is
/// read and written in the guest's order, so aliasing behaves as it does.
pub fn relative(j: &mut Journal, out: u32, a: u32, b: u32, fpu: Fpu) -> Result<(), Fallback> {
    let (mut sse, _) = units(fpu)?;
    let r = rows(j, a)?;
    for i in 0..4u32 {
        let v = vec(j, b.wrapping_add(16 * i))?;
        let s = combine(&mut sse, &r, v);
        put(j, out.wrapping_add(16 * i), s)?;
    }
    let r = rows(j, a)?;
    let t = vec(j, a.wrapping_add(0x30))?;
    let y = sse.mul(r[1], splat(lane(t, 1)));
    let x = sse.mul(r[0], splat(lane(t, 0)));
    let z = sse.mul(r[2], splat(lane(t, 2)));
    let xy = sse.add(x, y);
    let moved = sse.add(xy, z);
    let fourth = vec(j, out.wrapping_add(0x30))?;
    let t = sse.sub(fourth, moved);
    put(j, out.wrapping_add(0x30), t)?;
    done(&sse)
}

/// `0x002da6e0`, thiscall on `out`, `ret 8`: `c0 x + c1 y + c2 z` over
/// the transform's three rotation columns, no translation.
pub fn rotate(j: &mut Journal, out: u32, m: u32, v: u32, fpu: Fpu) -> Result<(), Fallback> {
    let (mut sse, _) = units(fpu)?;
    let v = vec(j, v)?;
    let z = sse.mul(vec(j, m.wrapping_add(0x20))?, splat(lane(v, 2)));
    let y = sse.mul(vec(j, m.wrapping_add(0x10))?, splat(lane(v, 1)));
    let x = sse.mul(vec(j, m)?, splat(lane(v, 0)));
    let xy = sse.add(x, y);
    let s = sse.add(xy, z);
    put(j, out, s)?;
    done(&sse)
}

/// `u.yzx * v.zxy - u.zxy * v.yzx`, `w` included: the query's cross product.
pub(crate) fn cross(sse: &mut Sse, u: u128, v: u128) -> u128 {
    let left = sse.mul(shuf1(u, 0xc9), shuf1(v, 0xd2));
    let right = sse.mul(shuf1(u, 0xd2), shuf1(v, 0xc9));
    sse.sub(left, right)
}

/// [`triangle`]'s answer: the edge mask (eax) and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    pub mask: u32,
    pub edx: u32,
}

/// `0x00303360`, thiscall on the query state, `(point, triangle, orient)`,
/// `ret 0xc`. The triangle is three vertices at `+0x00`, `+0x10`, `+0x20`
/// with a parallel array at `+0x40`. The normal is `(c - b) x (a - c)`;
/// each edge's test is the normal dotted with `(p - v) x edge`, and the
/// mask holds their sign bits. With `orient` set the normal is stored,
/// and when all three signs are set and the normal points away from the
/// state's support difference, it is negated and the first two vertices
/// (both arrays) and the first two tests swap.
pub fn triangle(j: &mut Journal, st: u32, p: u32, tri: u32, orient: u32, fpu: Fpu) -> Result<Sides, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    let a = vec(j, tri)?;
    let b = vec(j, tri.wrapping_add(0x10))?;
    let c = vec(j, tri.wrapping_add(0x20))?;
    let ac = sse.sub(a, c);
    let cb = sse.sub(c, b);
    let ba = sse.sub(b, a);
    let n = cross(&mut sse, cb, ac);
    let p = vec(j, p)?;
    let pb = sse.sub(p, b);
    let e1 = cross(&mut sse, pb, cb);
    let pc = sse.sub(p, c);
    let e2 = cross(&mut sse, pc, ac);
    let pa = sse.sub(p, a);
    let e3 = cross(&mut sse, pa, ba);
    let (e12lo, e3lo) = (shuf2(e1, e2, 0x44), shuf1(e3, 0x44));
    let zrow = shuf2(shuf2(e1, e2, 0xee), shuf1(e3, 0xee), 0x88);
    let zs = sse.mul(zrow, splat(lane(n, 2)));
    let ys = sse.mul(shuf2(e12lo, e3lo, 0xdd), splat(lane(n, 1)));
    let xs = sse.mul(shuf2(e12lo, e3lo, 0x88), splat(lane(n, 0)));
    let xy = sse.add(xs, ys);
    let d = sse.add(xy, zs);
    let mask = signs(d) & 7;
    put(j, at(state::SIDES), d)?;
    if orient == 0 {
        done(&sse)?;
        return Ok(Sides { mask, edx: tri });
    }
    put(j, at(state::NORMAL), n)?;
    if mask != 7 {
        done(&sse)?;
        return Ok(Sides { mask, edx: tri });
    }
    let q = sse.sub(vec(j, at(state::SUPPORT_B))?, vec(j, at(state::SUPPORT_A))?);
    let along = sse.mul(n, q);
    let s = sse.hsum(along);
    done(&sse)?;
    if s >> 31 == 0 {
        return Ok(Sides { mask, edx: tri });
    }
    put(j, at(state::NORMAL), n ^ vec(j, SIGNS)?)?;
    for off in [0x00, 0x40] {
        let first = vec(j, tri.wrapping_add(off))?;
        let second = vec(j, tri.wrapping_add(off + 0x10))?;
        put(j, tri.wrapping_add(off), second)?;
        put(j, tri.wrapping_add(off + 0x10), first)?;
    }
    // 0x0030350d: the first test goes through the x87 (`fld`, `fstp`).
    let first = word(j, at(state::SIDES))?;
    let second = word(j, at(state::SIDES + 4))?;
    j.put(at(state::SIDES), second).ok_or(Fallback::Memory)?;
    j.put(at(state::SIDES + 4), x.st(load(first))).ok_or(Fallback::Memory)?;
    Ok(Sides { mask, edx: second })
}

/// [`segments`]' answer: the case flags (eax and ecx) and the squared
/// distance's bits (edx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closest {
    pub flags: u32,
    pub distance: u32,
}

pub(crate) fn cmp(a: F80, b: F80) -> Cmp {
    x87::compare(a, b, false).0
}

/// `0x00308590`, cdecl `(p1, d1, p2, d2, out)`: the closest points of the
/// segments `p1 + s d1` and `p2 + t d2`, `s` and `t` clamped to `[0, 1]`.
/// `out` gets the point on the first, the difference to the point on the
/// second and its squared length. The dot products are SSE; the solve is
/// x87, its branches as the guest's flag tests take them. The flags say
/// which ends clamped: 1 `s = 1`, 2 `s = 0` (first pass or second),
/// 4 `t = 1`, 8 `t = 0`.
pub fn segments(j: &mut Journal, p1: u32, d1: u32, p2: u32, d2: u32, out: u32, fpu: Fpu) -> Result<Closest, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let zero = load(word(j, ZERO)?);
    let one = load(word(j, ONE)?);
    let k = load(word(j, PARALLEL)?);
    let dir1 = vec(j, d1)?;
    let base1 = vec(j, p1)?;
    let w = sse.sub(vec(j, p2)?, base1);
    let dir2 = vec(j, d2)?;
    let m = sse.mul(dir1, dir2);
    let b = sse.hsum(m);
    let m = sse.mul(dir1, w);
    let d = sse.hsum(m);
    let m = sse.mul(dir2, w);
    let e = sse.hsum(m);
    let m = sse.mul(dir1, dir1);
    let a = sse.hsum(m);
    let m = sse.mul(dir2, dir2);
    let c = sse.hsum(m);
    done(&sse)?;
    let ca = x.st(x.mul(load(c), load(a)));
    let bb = x.st(x.mul(load(b), load(b)));
    let den = x.st(x.sub(load(ca), load(bb)).abs());
    // `fsubp`: c d - e b, kept in the register.
    let num_x = x.sub(x.mul(load(c), load(d)), x.mul(load(e), load(b)));
    let num = x.st(num_x);
    let numden = x.mul(num_x, load(den));
    let den2 = x.mul(load(den), load(den));
    let (mut flags, t) = match cmp(den2, numden) {
        // `test ah,0x41; jnp`: at or past the end.
        Cmp::Less | Cmp::Equal => (1, one),
        _ => match cmp(load(num), zero) {
            Cmp::Less | Cmp::Equal => (2, zero),
            _ => {
                let parallel = x.mul(x.add(load(bb), load(ca)), k);
                // `test ah,5; jp`: only below divides.
                if cmp(parallel, load(den)) == Cmp::Less { (0, x.div(load(num), load(den))) } else { (1, one) }
            }
        },
    };
    let s_x = x.sub(x.mul(load(b), t), load(e));
    let s = x.st(s_x);
    let (u, t) = match cmp(s_x, load(c)) {
        Cmp::Less | Cmp::Unordered => match cmp(load(s), zero) {
            // 0x0030879d: t stands.
            Cmp::Greater | Cmp::Unordered => (x.st(x.div(load(s), load(c))), t),
            _ => {
                flags = 8;
                (0, again(&x, b, 0, d, a, zero, one, &mut flags))
            }
        },
        _ => {
            flags = 4;
            (ONE_BITS, again(&x, b, ONE_BITS, d, a, zero, one, &mut flags))
        }
    };
    let t = x.st(t);
    let scaled = sse_mul_splat(&mut sse, t, dir1);
    let near1 = sse.add(base1, scaled);
    put(j, out, near1)?;
    let dir2 = vec(j, d2)?;
    let base2 = vec(j, p2)?;
    let scaled = sse_mul_splat(&mut sse, u, dir2);
    let near2 = sse.add(base2, scaled);
    let diff = sse.sub(near1, near2);
    put(j, out.wrapping_add(0x10), diff)?;
    let r = vec(j, out.wrapping_add(0x10))?;
    let m = sse.mul(r, r);
    let distance = sse.hsum(m);
    done(&sse)?;
    j.put(out.wrapping_add(0x20), distance).ok_or(Fallback::Memory)?;
    Ok(Closest { flags, distance })
}

const ONE_BITS: u32 = 0x3f80_0000;
const HALF_BITS: u32 = 0x3f00_0000;

/// `0x003032c0`, thiscall on the query state, `(cache, separation, out)`,
/// `ret 0xc`; the cache is not read. The separation is a direction with
/// the distance in `w`. Out: the negated direction, the distance, and
/// the contact point with a weight: A's support point when A's simplex is
/// one vertex (1.0), else B's moved back along the direction by the
/// distance when B's is (0.0), else the state's midpoint (0.5). Returns
/// edx at the return: the distance's bits on B's case, else the
/// separation pointer.
pub fn contact(j: &mut Journal, st: u32, sep: u32, out: u32, fpu: Fpu) -> Result<u32, Fallback> {
    let (mut sse, _) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    let field = |off: u32| out.wrapping_add(off);
    let distance = word(j, sep.wrapping_add(0xc))?;
    j.put(field(contact_field::DISTANCE), distance).ok_or(Fallback::Memory)?;
    let normal = vec(j, sep)? ^ vec(j, SIGNS)?;
    put(j, field(contact_field::NORMAL), normal)?;
    let (point, weight, edx) = if word(j, at(state::COUNT_A))? == 1 {
        (vec(j, at(state::SUPPORT_A))?, ONE_BITS, sep)
    } else if word(j, at(state::COUNT_B))? == 1 {
        let distance = word(j, sep.wrapping_add(0xc))?;
        let back = sse.mul(splat(distance), normal);
        (sse.add(vec(j, at(state::SUPPORT_B))?, back), 0, distance)
    } else {
        (vec(j, at(state::MIDPOINT))?, HALF_BITS, sep)
    };
    j.put(field(contact_field::WEIGHT), weight).ok_or(Fallback::Memory)?;
    put(j, field(contact_field::POINT), point)?;
    done(&sse)?;
    Ok(edx)
}

/// `splat(s) * v`, the operand order of `mulps xmm2, xmm1`.
fn sse_mul_splat(sse: &mut Sse, s: u32, v: u128) -> u128 {
    sse.mul(splat(s), v)
}

/// 0x0030874f: with `t` clamped to `u`, `s` again from `b u + d` over `a`.
#[allow(clippy::too_many_arguments)]
fn again(x: &Fx, b: u32, u: u32, d: u32, a: u32, zero: F80, one: F80, flags: &mut u32) -> F80 {
    let tt_x = x.add(x.mul(load(b), load(u)), load(d));
    let tt = x.st(tt_x);
    match cmp(tt_x, zero) {
        Cmp::Less | Cmp::Equal => {
            *flags |= 2;
            zero
        }
        _ => match cmp(load(tt), load(a)) {
            Cmp::Less | Cmp::Unordered => x.div(load(tt), load(a)),
            _ => {
                *flags |= 1;
                one
            }
        },
    }
}

const MAX_BITS: u32 = 0x7f7f_ffff;
const MINUS_ONE_BITS: u32 = 0xbf80_0000;

/// [`tetrahedron`]'s answer: eax (a face index, or `-1` as `u32`) and ecx.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub eax: u32,
    pub ecx: u32,
}

/// `0x00303540`: eax the four vertices (`+0x00` to `+0x30`, the last the
/// newest), ebx the point, one stack word `s`, plain `ret`. Each face
/// opposite vertex 0, 1, 2 has the normal `(e) x (f)` over two of its
/// edges; its test is `n . s (p - v3)`, signed-squared and divided by
/// `|n|^2` (`FLT_MAX` for a zero normal). Below `1e-6` everywhere: `-1`.
/// One face past `1.1` times the next: that face. Otherwise
/// `0x00303110` picks among the three by their plain tests. Reads only;
/// the frame and the test array it hands down are dead at the return.
pub fn tetrahedron(j: &Journal, verts: u32, p: u32, s: u32, fpu: Fpu) -> Result<Face, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let vert = |i: u32| verts.wrapping_add(16 * i);
    let zero = load(word(j, ZERO)?);
    let v1 = vec(j, vert(1))?;
    let v0 = vec(j, vert(0))?;
    let v3 = vec(j, vert(3))?;
    let v2 = vec(j, vert(2))?;
    let e10 = sse.sub(v1, v0);
    let e02 = sse.sub(v0, v2);
    let e30 = sse.sub(v3, v0);
    let e31 = sse.sub(v3, v1);
    let e32 = sse.sub(v3, v2);
    let e21 = sse.sub(v2, v1);
    let n = [cross(&mut sse, e21, e31), cross(&mut sse, e02, e32), cross(&mut sse, e10, e30)];
    let to_p = sse.sub(vec(j, p)?, vec(j, vert(3))?);
    let q = sse.mul(splat(s), to_p);
    let (lo, n2lo) = (shuf2(n[0], n[1], 0x44), shuf1(n[2], 0x44));
    let zrow = shuf2(shuf2(n[0], n[1], 0xee), shuf1(n[2], 0xee), 0x88);
    let zs = sse.mul(zrow, splat(lane(q, 2)));
    let ys = sse.mul(shuf2(lo, n2lo, 0xdd), splat(lane(q, 1)));
    let xs = sse.mul(shuf2(lo, n2lo, 0x88), splat(lane(q, 0)));
    let xy = sse.add(xs, ys);
    let d = sse.add(xy, zs);
    let magnitude = d & vec(j, ABS)?;
    let signed = sse.mul(d, magnitude);
    let mut r = [0u32; 3];
    for (i, n) in n.into_iter().enumerate() {
        let m = sse.mul(n, n);
        let sq = sse.hsum(m);
        // `fucompp; test ah,0x44; jnp`: only an equal zero gives FLT_MAX.
        r[i] = if x87::compare(load(sq), zero, true).0 == Cmp::Equal {
            MAX_BITS
        } else {
            x.st(x.div(load(lane(signed, i)), load(sq)))
        };
    }
    done(&sse)?;
    // 0x003036ec: the largest test into the third slot, the next in st0.
    let (mut index, m, low) = if cmp(load(r[0]), load(r[1])) == Cmp::Greater { (0, load(r[0]), x.st(load(r[1]))) } else { (1, load(r[1]), r[0]) };
    let (largest, second) = if cmp(m, load(r[2])) == Cmp::Greater {
        let next = if cmp(load(low), load(r[2])) == Cmp::Greater { load(low) } else { load(r[2]) };
        (x.st(m), next)
    } else {
        index = 2;
        (r[2], m)
    };
    if cmp(load(largest), load(word(j, TINY)?)) == Cmp::Less {
        return Ok(Face { eax: u32::MAX, ecx: index });
    }
    let reach = x.mul(load(word(j, MARGIN)?), second);
    if cmp(load(largest), reach) == Cmp::Greater {
        return Ok(Face { eax: index, ecx: index });
    }
    let mut w = [lane(d, 0), lane(d, 1), lane(d, 2)];
    let eax = drop_edge(j, verts, p, &mut w, fpu)?;
    Ok(Face { eax, ecx: vert(3) })
}

/// `0x00303110`, cdecl `(point, tests)` with edi the vertices: for each
/// face with both neighbours' tests not below zero, the neighbour whose
/// edge the point projects past less is struck (`-1.0`); then the first
/// test above zero names the face, or `-1`.
fn drop_edge(j: &Journal, verts: u32, p: u32, w: &mut [u32; 3], fpu: Fpu) -> Result<u32, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let zero = load(word(j, ZERO)?);
    let vert = |i: usize| verts.wrapping_add(16 * i as u32);
    for i in 0..3 {
        // The tables at 0x004136b2 and 0x004136b0.
        let (c, e) = ((i + 1) % 3, (i + 2) % 3);
        if matches!(cmp(load(w[c]), zero), Cmp::Less | Cmp::Unordered) || matches!(cmp(load(w[e]), zero), Cmp::Less | Cmp::Unordered) {
            continue;
        }
        let v = vec(j, vert(i))?;
        let to_c = sse.sub(vec(j, vert(c))?, v);
        let to_q = sse.sub(vec(j, vert(3))?, v);
        let to_e = sse.sub(vec(j, vert(e))?, v);
        let to_p = sse.sub(vec(j, p)?, v);
        let m = sse.mul(to_p, to_c);
        let a = sse.hsum(m);
        let m = sse.mul(to_p, to_e);
        let b = sse.hsum(m);
        let m = sse.mul(to_q, to_c);
        let c_ = sse.hsum(m);
        let m = sse.mul(to_q, to_e);
        let d = sse.hsum(m);
        let cb = x.mul(load(c_), load(b));
        let da = x.mul(load(d), load(a));
        if cmp(cb, da) == Cmp::Greater {
            w[e] = MINUS_ONE_BITS;
        } else {
            w[c] = MINUS_ONE_BITS;
        }
    }
    done(&sse)?;
    Ok((0..3).find(|&k| cmp(load(w[k]), zero) == Cmp::Greater).map_or(u32::MAX, |k| k as u32))
}

/// [`edge`]'s answer: eax (`1` the segment meets the triangle inside it,
/// `0` the segment drops to one vertex, `2` the triangle to an edge), and
/// ecx and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// `dec dword ptr [at]`.
fn dec(j: &mut Journal, at: u32) -> Result<(), Fallback> {
    let n = word(j, at)?;
    j.put(at, n.wrapping_sub(1)).ok_or(Fallback::Memory)
}

/// `movaps` from one slot to another.
fn moved(j: &mut Journal, to: u32, from: u32) -> Result<(), Fallback> {
    let v = vec(j, from)?;
    put(j, to, v)
}

/// [`edge`]'s x87 tie-break between two edges: `(new[b] - old[b]) old[a]`
/// below `(new[a] - old[a]) old[b]`, `old` the sides at entry and `new`
/// the state's after the second vertex's test (`fcompp; test ah,5; jnp`).
fn skew(x: &Fx, j: &Journal, st: u32, old: u128, a: u32, b: u32) -> Result<bool, Fallback> {
    let new = |i: u32| word(j, st.wrapping_add(state::SIDES + 4 * i)).map(load);
    let old = |i: u32| load(lane(old, i as usize));
    let first = x.mul(x.sub(new(a)?, old(a)), old(b));
    let second = x.mul(x.sub(new(b)?, old(b)), old(a));
    Ok(cmp(second, first) == Cmp::Less)
}

/// Where the segment from `a` (test `da`) to `b` (test `db`) crosses the
/// plane: `t = da / (da - db)`, then `(1 - t) a + t b`.
fn crossing(j: &Journal, sse: &mut Sse, a: u128, b: u128, da: u32, db: u32) -> Result<u128, Fallback> {
    let span = sse.e.sub(da, db);
    let t = splat(sse.e.div(da, span));
    let rest = sse.sub(vec(j, ONES)?, t);
    let far = sse.mul(t, b);
    let near = sse.mul(rest, a);
    Ok(sse.add(near, far))
}

/// `n . (v - t0)`, the guest's horizontal sum.
fn height(sse: &mut Sse, n: u128, v: u128, t0: u128) -> u32 {
    let d = sse.sub(v, t0);
    let m = sse.mul(n, d);
    sse.hsum(m)
}

/// `0x003037a0`, thiscall on the query state, `(segment, triangle,
/// segment count, triangle count, fresh, feature)`, `ret 0x18`; `esp` is
/// the stack pointer at entry, for the frame the calls below read their
/// vectors from. The segment is two vertices at `+0x00`, `+0x10`, the
/// triangle three at `+0x00` to `+0x20`, each with a parallel array at
/// `+0x40`. Only `fresh`'s low byte is tested.
///
/// Fresh: the triangle's normal is `(t1 - t2) x (t0 - t2)`. A segment
/// whose ends lie either side of the plane meets it at a point, and a
/// point [`triangle`] finds inside answers `1`. Ends on one side: the
/// nearer goes first (both arrays swap) and is tested with `orient`;
/// inside, the segment count drops and the answer is `0`. Otherwise the
/// segment's closest approach ([`segments`]) to the edges `t2 t1` and
/// `t2 t0` chooses the nearer, `t2` (both arrays) overwrites the vertex
/// off it, the triangle count drops, and the answer is `2`.
///
/// Not fresh: the state's normal and sides stand. A crossing inside
/// answers `1`; the second vertex inside drops the segment to it (both
/// arrays) and stores `feature` at `+0x10`, answer `0`; else the second
/// vertex's edge mask, against the old sides where two edges compete,
/// names the vertex `t2` overwrites (or none), the triangle count drops,
/// and the answer is `2`.
#[allow(clippy::too_many_arguments)]
pub fn edge(j: &mut Journal, st: u32, esp: u32, seg: u32, tri: u32, seg_count: u32, tri_count: u32, fresh: u32, feature: u32, fpu: Fpu) -> Result<Edge, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    // `push ebp; and esp,-16; sub esp,0xa4`, then three pushes.
    let frame = |off: u32| (esp.wrapping_sub(4) & !15).wrapping_sub(0xb0).wrapping_add(off);
    let s_at = |i: u32| seg.wrapping_add(16 * i);
    let t_at = |i: u32| tri.wrapping_add(16 * i);
    if fresh & 0xff == 0 {
        let sides = vec(j, at(state::SIDES))?;
        let t0 = vec(j, t_at(0))?;
        let b1 = vec(j, s_at(1))?;
        let n = vec(j, at(state::NORMAL))?;
        let d1 = height(&mut sse, n, b1, t0);
        let b0 = vec(j, s_at(0))?;
        let d0 = height(&mut sse, n, b0, t0);
        let across = sse.e.mul(d0, d1) >> 31 != 0;
        let p = if across { Some(crossing(j, &mut sse, b0, b1, d0, d1)?) } else { None };
        done(&sse)?;
        if let Some(p) = p {
            put(j, frame(0x30), p)?;
            let s = triangle(j, st, frame(0x30), tri, 0, fpu)?;
            if s.mask == 7 {
                return Ok(Edge { eax: 1, ecx: st, edx: s.edx });
            }
        }
        let s = triangle(j, st, s_at(1), tri, 0, fpu)?;
        if s.mask == 7 {
            dec(j, seg_count)?;
            moved(j, s_at(0), s_at(1))?;
            moved(j, s_at(4), s_at(5))?;
            j.put(at(state::FEATURE), feature).ok_or(Fallback::Memory)?;
            return Ok(Edge { eax: 0, ecx: st, edx: feature });
        }
        // 0x00303a89: the vertex off the edge the second vertex lies past.
        let view: &Journal = j;
        let below = |a, b| skew(&x, view, st, sides, a, b);
        let off = match s.mask {
            3 => None,
            5 => Some(1),
            6 => Some(0),
            1 => (!below(1, 2)?).then_some(1),
            2 => below(2, 0)?.then_some(0),
            4 => Some(if below(0, 1)? { 1 } else { 0 }),
            _ => Some(0),
        };
        dec(j, tri_count)?;
        if let Some(k) = off {
            moved(j, t_at(k), t_at(2))?;
            moved(j, t_at(k + 4), t_at(6))?;
        }
        return Ok(Edge { eax: 2, ecx: st, edx: s.edx });
    }
    let t0 = vec(j, t_at(0))?;
    let t2 = vec(j, t_at(2))?;
    let t1 = vec(j, t_at(1))?;
    let u = sse.sub(t1, t2);
    let w = sse.sub(t0, t2);
    let n = cross(&mut sse, u, w);
    put(j, frame(0x30), w)?;
    put(j, frame(0x40), u)?;
    let s0 = vec(j, s_at(0))?;
    let d0 = height(&mut sse, n, s0, t0);
    let s1 = vec(j, s_at(1))?;
    let d1 = height(&mut sse, n, s1, t0);
    // `movmskps` both, `xor`, `test al,1`.
    let p = if (d0 ^ d1) >> 31 != 0 { Some(crossing(j, &mut sse, s0, s1, d0, d1)?) } else { None };
    done(&sse)?;
    if let Some(p) = p {
        put(j, frame(0x10), p)?;
        let s = triangle(j, st, frame(0x10), tri, 0, fpu)?;
        if s.mask == 7 {
            return Ok(Edge { eax: 1, ecx: st, edx: s.edx });
        }
    } else {
        // `fabs` both, `fcompp; test ah,5; jp`: only a nearer second end swaps.
        if cmp(load(d1).abs(), load(d0).abs()) == Cmp::Less {
            moved(j, s_at(0), s_at(1))?;
            put(j, s_at(1), s0)?;
            let a = vec(j, s_at(4))?;
            let b = vec(j, s_at(5))?;
            put(j, s_at(4), b)?;
            put(j, s_at(5), a)?;
        }
        let s = triangle(j, st, seg, tri, 1, fpu)?;
        if s.mask == 7 {
            dec(j, seg_count)?;
            return Ok(Edge { eax: 0, ecx: st, edx: s.edx });
        }
    }
    // 0x0030390f: the segment against the edges from t2 the frame holds.
    let along = sse.sub(vec(j, s_at(1))?, vec(j, s_at(0))?);
    done(&sse)?;
    put(j, frame(0x10), along)?;
    let first = segments(j, seg, frame(0x10), t_at(2), frame(0x40), frame(0x50), fpu)?;
    let second = segments(j, seg, frame(0x10), t_at(2), frame(0x30), frame(0x80), fpu)?;
    // `fcomp; test ah,0x41; jne`: only a farther first edge keeps t0.
    let k = u32::from(cmp(load(first.distance), load(second.distance)) == Cmp::Greater);
    moved(j, t_at(k), t_at(2))?;
    moved(j, t_at(k + 4), t_at(6))?;
    dec(j, tri_count)?;
    Ok(Edge { eax: 2, ecx: k << 4, edx: second.distance })
}

/// `FLT_EPSILON`, which scales [`query`]'s stall test, and the tolerance
/// the query starts from (about `1e-10`), doubled on every pass.
pub const STALL: u32 = 0x0041_359c;
const TOLERANCE_BITS: u32 = 0x2edb_e6ff;
/// `0x004136a8`: [`triangle`]'s edge mask to the vertex the point lies
/// beyond; below zero, `8 + e` names the vertex to drop (past 3, the
/// second).
const OFF: [i8; 8] = [-1, 0, 1, -6, 2, -7, -8, -1];
/// `0x004136b0` and `0x004136b2`: a triangle vertex's two neighbours.
const SECOND: [u32; 3] = [2, 0, 1];
const FIRST: [u32; 3] = [1, 2, 0];
/// More than this many vertices in a simplex declines.
const MAX_COUNT: u32 = 256;

/// [`query`]'s arguments after the state: the two shapes, B's transform
/// in A's frame, the feature cache and the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub a: u32,
    pub b: u32,
    pub transform: u32,
    pub cache: u32,
    pub out: u32,
}

/// [`query`]'s answer: eax (0 separated, 3 stalled, or the triangle
/// case's), and ecx and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Query {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// The calls [`query`] makes out: each shape's two virtuals, and the
/// triangle case. `esp` is the stack pointer at the call's entry (at the
/// return address). The query reads no register a call leaves.
pub trait Calls {
    /// The shape's `vt[+0x34]`, thiscall `(ids, count, out)`, `ret 0xc`:
    /// the vertices the cached IDs name.
    fn vertices(&mut self, j: &mut Journal, shape: u32, ids: u32, count: u32, out: u32, esp: u32) -> Result<(), Fallback>;
    /// The shape's `vt[+0x30]`, thiscall `(direction, out)`, `ret 8`: the
    /// support point along the direction.
    fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, esp: u32) -> Result<(), Fallback>;
    /// `0x00303dd0`, thiscall on the state, `(shape A, shape B,
    /// transform, cache)`, `ret 0x10` ([`triangle_case`]): its eax
    /// becomes the query's.
    fn triangle_case(&mut self, j: &mut Journal, st: u32, args: Args, esp: u32) -> Result<u32, Fallback>;
}

/// Which simplex is the larger in a `(1, n)` or `(n, 1)` case.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Big {
    A,
    B,
}

impl Big {
    /// The larger simplex's vertices, and the single vertex's.
    fn base(self) -> u32 {
        match self {
            Big::A => state::SUPPORT_A,
            Big::B => state::SUPPORT_B,
        }
    }
    fn point(self) -> u32 {
        match self {
            Big::A => state::SUPPORT_B,
            Big::B => state::SUPPORT_A,
        }
    }
    fn count(self) -> u32 {
        match self {
            Big::A => state::COUNT_A,
            Big::B => state::COUNT_B,
        }
    }
}

/// Where a case leaves the loop: back to the dispatch (`0x00304d2b`), or
/// out to the support step with `0x00304d40`'s 1 or `0x003051d8`'s 0.
enum Flow {
    Dispatch,
    Out(bool),
}

pub(crate) fn set(j: &mut Journal, at: u32, v: u32) -> Result<(), Fallback> {
    j.put(at, v).ok_or(Fallback::Memory)
}

fn put_byte(j: &mut Journal, at: u32, v: u8) -> Result<(), Fallback> {
    let w = word(j, at & !3)?;
    let s = 8 * (at & 3);
    set(j, at & !3, w & !(0xff << s) | u32::from(v) << s)
}

/// `mov [at], r16`.
fn put_half(j: &mut Journal, at: u32, v: u32) -> Result<(), Fallback> {
    put_byte(j, at, v as u8)?;
    put_byte(j, at.wrapping_add(1), (v >> 8) as u8)
}

/// `movsx r32, byte ptr [at]`.
fn signed_byte(j: &Journal, at: u32) -> Result<u32, Fallback> {
    Ok(j.byte(at).ok_or(Fallback::Memory)? as i8 as i32 as u32)
}

struct Run<'c, C> {
    st: u32,
    /// The frame's esp after the three register pushes.
    f: u32,
    args: Args,
    fpu: Fpu,
    sse: Sse,
    x: Fx,
    calls: &'c mut C,
    /// The frame offsets of the `(2, 2)` case's two edges, A's then B's.
    edges: (u32, u32),
}

/// The cases, which make no call out: [`query`]'s and [`triangle_case`]'s.
impl<C> Run<'_, C> {
    fn at(&self, off: u32) -> u32 {
        self.st.wrapping_add(off)
    }

    fn slot(&self, base: u32, i: u32) -> u32 {
        self.at(base).wrapping_add(i.wrapping_mul(16))
    }

    fn frame(&self, off: u32) -> u32 {
        self.f.wrapping_add(off)
    }

    /// `c0 x + c1 y + (c2 z + c3)` over the transform's columns: a vertex
    /// of B into A's frame (`0x00304920`, `0x0030537b`).
    fn place(&mut self, j: &Journal, v: u128) -> Result<u128, Fallback> {
        let t = self.args.transform;
        let c0 = vec(j, t)?;
        let c1 = vec(j, t.wrapping_add(0x10))?;
        let c2 = vec(j, t.wrapping_add(0x20))?;
        let c3 = vec(j, t.wrapping_add(0x30))?;
        let x = self.sse.mul(c0, splat(lane(v, 0)));
        let y = self.sse.mul(c1, splat(lane(v, 1)));
        let z = self.sse.mul(c2, splat(lane(v, 2)));
        let zt = self.sse.add(z, c3);
        let xy = self.sse.add(x, y);
        Ok(self.sse.add(xy, zt))
    }

    /// `0x003051d8`: B's count to `+0x08`, out with 0.
    fn settle(&self, j: &mut Journal) -> Result<Flow, Fallback> {
        let n = word(j, self.at(state::COUNT_B))?;
        set(j, self.at(state::LAST_B), n)?;
        Ok(Flow::Out(false))
    }

    /// `0x003051c3`: the normal from A's first vertex to B's.
    fn between(&mut self, j: &mut Journal) -> Result<Flow, Fallback> {
        let b0 = vec(j, self.at(state::SUPPORT_B))?;
        let a0 = vec(j, self.at(state::SUPPORT_A))?;
        let n = self.sse.sub(b0, a0);
        put(j, self.at(state::NORMAL), n)?;
        self.settle(j)
    }

    /// `(1, 2)` `0x00304fd6`, `(2, 1)` `0x003050e5`: the point against the
    /// segment. Its ends' projections on opposite sides: the normal is
    /// the segment's perpendicular toward the point. Otherwise the
    /// segment drops to its first end, or to its second (both arrays)
    /// when both projections are below zero.
    fn segment(&mut self, j: &mut Journal, big: Big) -> Result<Flow, Fallback> {
        let base = big.base();
        let v1 = vec(j, self.slot(base, 1))?;
        let v0 = vec(j, self.slot(base, 0))?;
        let p = vec(j, self.at(big.point()))?;
        let e = self.sse.sub(v1, v0);
        let u0 = self.sse.sub(v0, p);
        let u1 = self.sse.sub(v1, p);
        let m = self.sse.mul(u1, e);
        let d1 = self.sse.hsum(m);
        let m = self.sse.mul(u0, e);
        let d0 = self.sse.hsum(m);
        if d1 >> 31 != d0 >> 31 {
            let c = cross(&mut self.sse, u1, u0);
            let n = match big {
                Big::B => cross(&mut self.sse, c, e),
                Big::A => cross(&mut self.sse, e, c),
            };
            put(j, self.at(state::NORMAL), n)?;
            return self.settle(j);
        }
        if d1 >> 31 != 0 {
            let par = vec(j, self.slot(base + 0x40, 1))?;
            put(j, self.slot(base, 0), v1)?;
            put(j, self.slot(base + 0x40, 0), par)?;
        }
        dec(j, self.at(big.count()))?;
        self.between(j)
    }

    /// `(1, 3)` `0x00304ae6`, `(3, 1)` `0x00304c43`: the point against the
    /// triangle ([`triangle`], B's normal toward A). Inside: out with 0.
    /// Beyond one edge only: the vertex [`OFF`] names drops and the
    /// segment case runs. Beyond a vertex: the triangle drops to that
    /// vertex's edge the point projects onto, or to the vertex alone.
    /// Only B's parallel array follows.
    fn face(&mut self, j: &mut Journal, big: Big) -> Result<Flow, Fallback> {
        let base = big.base();
        let follow = big == Big::B;
        let orient = if follow { 1 } else { u32::MAX };
        let mask = triangle(j, self.st, self.at(big.point()), self.at(base), orient, self.fpu)?.mask;
        if mask == 7 {
            return self.settle(j);
        }
        let count = self.at(big.count());
        let e = OFF[mask as usize];
        if e < 0 {
            let k = if e + 8 > 3 { 1 } else { (e + 8) as u32 };
            let v2 = vec(j, self.slot(base, 2))?;
            dec(j, count)?;
            put(j, self.slot(base, k), v2)?;
            if follow {
                moved(j, self.slot(base + 0x40, k), self.slot(base + 0x40, 2))?;
            }
            return self.segment(j, big);
        }
        let i = e as u32;
        let (far, near) = (SECOND[i as usize], FIRST[i as usize]);
        let p = vec(j, self.at(big.point()))?;
        let vi = vec(j, self.slot(base, i))?;
        let vn = vec(j, self.slot(base, near))?;
        let vf = vec(j, self.slot(base, far))?;
        let to_far = self.sse.sub(vf, vi);
        let to_p = self.sse.sub(p, vi);
        let to_near = self.sse.sub(vn, vi);
        let m = self.sse.mul(to_p, to_far);
        let s = self.sse.hsum(m);
        let v2 = vec(j, self.slot(base, 2))?;
        set(j, count, 2)?;
        if s >> 31 == 0 {
            self.third(j, base, near, v2, follow)?;
            return Ok(Flow::Dispatch);
        }
        self.third(j, base, far, v2, follow)?;
        let m = self.sse.mul(to_p, to_near);
        if self.sse.hsum(m) >> 31 == 0 {
            return Ok(Flow::Dispatch);
        }
        let n = word(j, count)?;
        let k = if near == n { far } else { near };
        let n = n.wrapping_sub(1);
        set(j, count, n)?;
        moved(j, self.slot(base, k), self.slot(base, n))?;
        if follow {
            let n = word(j, count)?;
            moved(j, self.slot(base + 0x40, k), self.slot(base + 0x40, n))?;
        }
        Ok(Flow::Dispatch)
    }

    /// The third vertex `v` into slot `k`, and its parallel twin.
    fn third(&self, j: &mut Journal, base: u32, k: u32, v: u128, follow: bool) -> Result<(), Fallback> {
        put(j, self.slot(base, k), v)?;
        if follow {
            moved(j, self.slot(base + 0x40, k), self.slot(base + 0x40, 2))?;
        }
        Ok(())
    }

    /// `(1, 4)` `0x00304aa1`, `(4, 1)` `0x00304c12`: [`tetrahedron`]
    /// (`s` -1 for B's, 1 for A's). No face: out with 1. A face: its
    /// opposite vertex takes the fourth's place (B's both arrays) and the
    /// triangle case runs.
    fn tetra(&mut self, j: &mut Journal, big: Big) -> Result<Flow, Fallback> {
        let base = big.base();
        let s = if big == Big::B { MINUS_ONE_BITS } else { ONE_BITS };
        let face = tetrahedron(j, self.at(base), self.at(big.point()), s, self.fpu)?;
        if (face.eax as i32) < 0 {
            return Ok(Flow::Out(true));
        }
        let v3 = vec(j, self.slot(base, 3))?;
        dec(j, self.at(big.count()))?;
        put(j, self.slot(base, face.eax), v3)?;
        if big == Big::B {
            moved(j, self.slot(base + 0x40, face.eax), self.slot(base + 0x40, 3))?;
        }
        self.face(j, big)
    }

    /// `(2, 2)` `0x003049fe`: the two segments' closest points
    /// ([`segments`] into `+0x130`). Neither end clamped: the normal is
    /// the edges' cross product, turned toward B. Otherwise each clamped
    /// segment drops to the end it clamped at.
    fn pair(&mut self, j: &mut Journal) -> Result<Flow, Fallback> {
        let (da, db) = (self.frame(self.edges.0), self.frame(self.edges.1));
        let (a0, b0) = (self.at(state::SUPPORT_A), self.at(state::SUPPORT_B));
        let (a1, b1) = (self.slot(state::SUPPORT_A, 1), self.slot(state::SUPPORT_B, 1));
        let ea = self.sse.sub(vec(j, a1)?, vec(j, a0)?);
        put(j, da, ea)?;
        let eb = self.sse.sub(vec(j, b1)?, vec(j, b0)?);
        put(j, db, eb)?;
        let flags = segments(j, a0, da, b0, db, self.at(state::MIDPOINT), self.fpu)?.flags;
        if flags == 0 {
            let eb = vec(j, db)?;
            let ea = vec(j, da)?;
            let n = cross(&mut self.sse, ea, eb);
            let t = self.sse.sub(vec(j, a0)?, vec(j, b0)?);
            let m = self.sse.mul(n, t);
            let s = self.sse.hsum(m);
            put(j, self.at(state::NORMAL), n)?;
            if s >> 31 == 0 {
                put(j, self.at(state::NORMAL), n ^ vec(j, SIGNS)?)?;
            }
            return self.settle(j);
        }
        if flags & 1 != 0 {
            moved(j, a0, a1)?;
        }
        if flags & 3 != 0 {
            set(j, self.at(state::COUNT_A), 1)?;
        }
        if flags & 4 != 0 {
            moved(j, b0, b1)?;
            moved(j, self.at(state::LOCAL_B), self.slot(state::LOCAL_B, 1))?;
        }
        if flags & 12 != 0 {
            set(j, self.at(state::COUNT_B), 1)?;
        }
        Ok(Flow::Dispatch)
    }

    /// `(2, 3)` `0x003049ae` (A's segment, feature 1), `(3, 2)`
    /// `0x003049cc` (B's, feature -1): [`edge`], fresh when B's count was
    /// 2 when the loop last left a case. 1: out with 1; 2: the `(2, 2)`
    /// case; 0: out with 0.
    fn edge(&mut self, j: &mut Journal, big: Big) -> Result<Flow, Fallback> {
        let fresh = u32::from(word(j, self.at(state::LAST_B))? == 2);
        let (seg, tri, feature) = match big {
            Big::B => (Big::A, Big::B, 1),
            Big::A => (Big::B, Big::A, u32::MAX),
        };
        let (seg_at, tri_at) = (self.at(seg.base()), self.at(tri.base()));
        let (seg_count, tri_count) = (self.at(seg.count()), self.at(tri.count()));
        // Six arguments and the return address below the frame.
        let esp = self.f.wrapping_sub(28);
        match edge(j, self.st, esp, seg_at, tri_at, seg_count, tri_count, fresh, feature, self.fpu)?.eax {
            1 => Ok(Flow::Out(true)),
            2 => self.pair(j),
            _ => self.settle(j),
        }
    }

    /// The jump table at `0x00305650` by `(nA << 3 | nB) - 9`.
    fn case(&mut self, j: &mut Journal) -> Result<Flow, Fallback> {
        let na = word(j, self.at(state::COUNT_A))?;
        let nb = word(j, self.at(state::COUNT_B))?;
        match (na << 3 | nb).wrapping_sub(9) {
            0 => self.between(j),
            1 => self.segment(j, Big::B),
            2 => self.face(j, Big::B),
            3 => self.tetra(j, Big::B),
            8 => self.segment(j, Big::A),
            9 => self.pair(j),
            10 => self.edge(j, Big::B),
            16 => self.face(j, Big::A),
            17 => self.edge(j, Big::A),
            24 => self.tetra(j, Big::A),
            _ => Ok(Flow::Out(true)),
        }
    }

    /// The support tests: `|d| d`, `d` the new vertex less an old one
    /// along the direction, against the threshold. The first (`test
    /// ah,0x41; jne`) passes only above; the rest (`test ah,5; jnp`) fail
    /// only below.
    fn clear(&mut self, new: u128, old: u128, dir: u128, thr: u32, first: bool) -> bool {
        let g = self.sse.sub(new, old);
        let m = self.sse.mul(g, dir);
        let d = self.sse.hsum(m);
        let sq = self.x.mul(load(d).abs(), load(d));
        match cmp(sq, load(thr)) {
            Cmp::Greater => true,
            Cmp::Less => false,
            _ => !first,
        }
    }
}

/// The steps that call out: the support points, the stall, the query.
impl<C: Calls> Run<'_, C> {
    /// The new vertex at `base[n]` against `base[0]`, then for a simplex
    /// of two or three against `base[2]` and `base[1]`.
    fn fresh_vertex(&mut self, j: &Journal, base: u32, n: u32, dir: u128, thr: u32) -> Result<bool, Fallback> {
        let new = vec(j, self.slot(base, n))?;
        let first = vec(j, self.slot(base, 0))?;
        if !self.clear(new, first, dir, thr, true) {
            return Ok(false);
        }
        let rest: &[u32] = match n {
            2 => &[1],
            3 => &[2, 1],
            _ => &[],
        };
        for &k in rest {
            let old = vec(j, self.slot(base, k))?;
            if !self.clear(new, old, dir, thr, false) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `0x00304df4`, `0x003053d4`: A's support point along the normal, kept
    /// when it is new.
    fn grow_a(&mut self, j: &mut Journal, thr: u32) -> Result<bool, Fallback> {
        let n = word(j, self.at(state::COUNT_A))?;
        if n == signed_byte(j, self.args.cache.wrapping_add(cache::MAX_A))? || word(j, self.at(state::FEATURE))? == 1 {
            return Ok(false);
        }
        let (shape, normal, out, esp) = (self.args.a, self.at(state::NORMAL), self.slot(state::SUPPORT_A, n), self.f.wrapping_sub(12));
        self.calls.support(j, shape, normal, out, esp)?;
        let n = word(j, self.at(state::COUNT_A))?;
        let dir = vec(j, self.at(state::NORMAL))?;
        if !self.fresh_vertex(j, state::SUPPORT_A, n, dir, thr)? {
            return Ok(false);
        }
        set(j, self.at(state::COUNT_A), n.wrapping_add(1))?;
        Ok(true)
    }

    /// `0x003051e4`: B's support point along the negated normal in B's
    /// frame (`+0x70` of the frame), kept, and placed into A's frame,
    /// when it is new.
    fn grow_b(&mut self, j: &mut Journal, thr: u32) -> Result<bool, Fallback> {
        let n = word(j, self.at(state::COUNT_B))?;
        if n == signed_byte(j, self.args.cache.wrapping_add(cache::MAX_B))? || word(j, self.at(state::FEATURE))? == u32::MAX {
            return Ok(false);
        }
        let r = rows(j, self.args.transform)?;
        let normal = vec(j, self.at(state::NORMAL))?;
        let local = combine(&mut self.sse, &r, normal) ^ vec(j, SIGNS)?;
        let dir = self.frame(0x70);
        put(j, dir, local)?;
        let (shape, out, esp) = (self.args.b, self.slot(state::LOCAL_B, n), self.f.wrapping_sub(12));
        self.calls.support(j, shape, dir, out, esp)?;
        let n = word(j, self.at(state::COUNT_B))?;
        let d = vec(j, dir)?;
        if !self.fresh_vertex(j, state::LOCAL_B, n, d, thr)? {
            return Ok(false);
        }
        let v = vec(j, self.slot(state::LOCAL_B, n))?;
        let placed = self.place(j, v)?;
        put(j, self.slot(state::SUPPORT_B, n), placed)?;
        let n = word(j, self.at(state::COUNT_B))?;
        set(j, self.at(state::COUNT_B), n.wrapping_add(1))?;
        Ok(true)
    }

    /// `0x003054f1`: a query out with 1, or whose distance has stalled.
    /// With the `+0x0c` byte clear the triangle case answers; otherwise a
    /// pair of simplices over four vertices becomes `(3, 1)` or `(1, 3)`,
    /// the larger keeping three, and the answer is 3.
    fn stall(&mut self, j: &mut Journal) -> Result<u32, Fallback> {
        if j.byte(self.at(state::SETTLE)).ok_or(Fallback::Memory)? == 0 {
            let (st, args, esp) = (self.st, self.args, self.f.wrapping_sub(20));
            return self.calls.triangle_case(j, st, args, esp);
        }
        let nb = word(j, self.at(state::COUNT_B))?;
        let na = word(j, self.at(state::COUNT_A))?;
        if na.wrapping_add(nb) as i32 > 4 {
            let (a, b) = if na as i32 > nb as i32 { (3, 1) } else { (1, 3) };
            set(j, self.at(state::COUNT_A), a)?;
            set(j, self.at(state::COUNT_B), b)?;
        }
        Ok(3)
    }

    /// `0x00305547`: the unit normal (one Newton step on `rsqrtss`) with
    /// the distance along it in `w`, then the sizes and the vertex IDs
    /// (each vertex's `w`, low 16 bits) back to the cache.
    fn finish(&mut self, j: &mut Journal, eax: u32) -> Result<Query, Fallback> {
        let a0 = vec(j, self.at(state::SUPPORT_A))?;
        let b0 = vec(j, self.at(state::SUPPORT_B))?;
        let n = vec(j, self.at(state::NORMAL))?;
        let gap = self.sse.sub(b0, a0);
        let sq = self.sse.mul(n, n);
        let nn = self.sse.hsum(sq);
        let r = self.sse.e.rsqrt(nn);
        let sr = self.sse.e.mul(nn, r);
        let srr = self.sse.e.mul(sr, r);
        let three = self.sse.e.sub(3.0f32.to_bits(), srr);
        let half = self.sse.e.mul(HALF_BITS, r);
        let k = self.sse.e.mul(half, three);
        let along = self.sse.mul(gap, n);
        let unit = self.sse.mul(splat(k), n);
        put(j, self.args.out, unit)?;
        let s = self.sse.hsum(along);
        let distance = self.sse.e.mul(s, k);
        set(j, self.args.out.wrapping_add(0xc), distance)?;
        let c = self.args.cache;
        let na = word(j, self.at(state::COUNT_A))?;
        put_byte(j, c.wrapping_add(cache::COUNT_A), na as u8)?;
        let nb = word(j, self.at(state::COUNT_B))?;
        put_byte(j, c.wrapping_add(cache::COUNT_B), nb as u8)?;
        // Two do-while loops: one ID at least from each side.
        let mut i = 0u32;
        loop {
            let id = word(j, self.slot(state::SUPPORT_A, i).wrapping_add(0xc))?;
            put_half(j, c.wrapping_add(2 * i), id)?;
            i += 1;
            if i as i32 >= word(j, self.at(state::COUNT_A))? as i32 {
                break;
            }
            if i >= MAX_COUNT {
                return Err(Fallback::Memory);
            }
        }
        let mut k = 0u32;
        loop {
            let id = word(j, self.slot(state::LOCAL_B, k).wrapping_add(0xc))?;
            put_half(j, c.wrapping_add(2 * (i + k)), id)?;
            let ecx = word(j, self.at(state::COUNT_B))?;
            k += 1;
            if k as i32 >= ecx as i32 {
                return Ok(Query { eax, ecx, edx: k });
            }
            if k >= MAX_COUNT {
                return Err(Fallback::Memory);
            }
        }
    }

    fn run(&mut self, j: &mut Journal) -> Result<Query, Fallback> {
        let c = self.args.cache;
        let na = signed_byte(j, c.wrapping_add(cache::COUNT_A))?;
        set(j, self.at(state::COUNT_A), na)?;
        let nb = signed_byte(j, c.wrapping_add(cache::COUNT_B))?;
        set(j, self.at(state::COUNT_B), nb)?;
        // Three arguments and the return address below the frame.
        let esp = self.f.wrapping_sub(16);
        let (a, b) = (self.args.a, self.args.b);
        let (a_out, b_out) = (self.at(state::SUPPORT_A), self.at(state::LOCAL_B));
        let na = word(j, self.at(state::COUNT_A))?;
        self.calls.vertices(j, a, c, na, a_out, esp)?;
        let nb = word(j, self.at(state::COUNT_B))?;
        let na = word(j, self.at(state::COUNT_A))?;
        self.calls.vertices(j, b, c.wrapping_add(na.wrapping_mul(2)), nb, b_out, esp)?;
        let nb = word(j, self.at(state::COUNT_B))?;
        if nb.wrapping_sub(1) as i32 >= 0 {
            if nb > MAX_COUNT {
                return Err(Fallback::Memory);
            }
            for i in 0..nb {
                let v = vec(j, self.slot(state::LOCAL_B, i))?;
                let placed = self.place(j, v)?;
                put(j, self.slot(state::SUPPORT_B, i), placed)?;
            }
        }
        let scale = load(word(j, STALL)?);
        let (mut tolerance, mut pass) = (TOLERANCE_BITS, 0u32);
        // The tolerance doubles each pass, so the support tests fail
        // within a few hundred passes at most.
        let eax = loop {
            tolerance = self.x.st(self.x.add(load(tolerance), load(tolerance)));
            pass = pass.wrapping_add(1);
            set(j, self.at(state::FEATURE), 0)?;
            let out = loop {
                match self.case(j)? {
                    Flow::Dispatch => continue,
                    Flow::Out(out) => break out,
                }
            };
            // 0x00304d45: |n|^2 and the gap along n.
            let n = vec(j, self.at(state::NORMAL))?;
            let m = self.sse.mul(n, n);
            let nn = self.sse.hsum(m);
            let (b0, a0) = (vec(j, self.at(state::SUPPORT_B))?, vec(j, self.at(state::SUPPORT_A))?);
            let gap = self.sse.sub(b0, a0);
            let m = self.sse.mul(gap, n);
            let d = self.sse.hsum(m);
            // `fcompp; test ah,1; je`: on, only while the cube of the gap
            // is above the tolerance's share of |n|^2 (or unordered).
            let cube = self.x.mul(self.x.mul(load(d), load(d)), load(d));
            let floor = self.x.mul(self.x.mul(load(tolerance), scale), load(nn));
            if out || !matches!(cmp(floor, cube), Cmp::Less | Cmp::Unordered) {
                break self.stall(j)?;
            }
            let thr = self.x.st(self.x.mul(load(nn), load(tolerance)));
            // Odd passes try A first, even ones B: the order is the
            // short circuit's, which clippy reads as the same block.
            #[allow(clippy::if_same_then_else)]
            let grew = if pass & 1 != 0 {
                self.grow_a(j, thr)? || self.grow_b(j, thr)?
            } else {
                self.grow_b(j, thr)? || self.grow_a(j, thr)?
            };
            if !grew {
                break 0;
            }
        };
        self.finish(j, eax)
    }
}

/// `0x003048a0`, thiscall on the query state, `(shape A, shape B,
/// transform, cache, out)`, `ret 0x14`; `esp` is the stack pointer at
/// entry. The simplex sizes come from the cache, the vertices through
/// each shape's `vt[+0x34]` (A's at `+0x20`, B's at `+0xe0` and placed
/// into A's frame at `+0xa0`). Each pass doubles the tolerance and runs
/// the case for the two sizes until one leaves the loop; then, unless the
/// gap has stalled, a new support point of A (odd passes first) or B
/// joins its simplex when it is not one already there, and the next pass
/// runs. Out: the unit normal from A to B with the distance in `w`, the
/// sizes and vertex IDs back in the cache, eax 0 (3 or the triangle
/// case's on a stall), ecx B's size and edx the IDs B wrote.
pub fn query(j: &mut Journal, st: u32, esp: u32, args: Args, fpu: Fpu, calls: &mut impl Calls) -> Result<Query, Fallback> {
    let (sse, x) = units(fpu)?;
    // `push ebp; and esp,-16; sub esp,0x74`, then three pushes.
    let f = (esp.wrapping_sub(4) & !15).wrapping_sub(0x80);
    Run { st, f, args, fpu, sse, x, calls, edges: (0x60, 0x50) }.run(j)
}

/// `0x004801e0`: the vector [`triangle_case`] tests vertices against;
/// two whose difference is within it in `x`, `y` and `z` are one.
pub const SAME: u32 = 0x0048_01e0;

/// [`triangle_case`]'s frame: the simplex it hands [`Penetration`] (A's
/// four vertices, B's four in B's frame, the count), the differences
/// `A - B`, the direction the call leaves, and the saved eax.
pub mod tri_frame {
    pub const SIMPLEX: u32 = 0x20;
    pub const SIMPLEX_B: u32 = 0x60;
    pub const COUNT: u32 = 0xa0;
    pub const DIFFERENCES: u32 = 0xd0;
    pub const DIRECTION: u32 = 0x110;
    pub const EAX: u32 = 0x1c;
}

/// The call [`triangle_case`] makes out; [`crate::penetration::Port`]
/// answers it in Rust, the expanding polytope included.
pub trait Penetration {
    /// `0x0030a6b0`, cdecl `(shape A, shape B, transform, simplex,
    /// differences, direction)`: the simplex at entry is up to four
    /// vertex pairs; it leaves up to three and their count in the
    /// simplex, and the direction. `esp` is the stack pointer at the
    /// call's entry. Its eax becomes the triangle case's.
    fn penetration(&mut self, j: &mut Journal, args: [u32; 6], esp: u32) -> Result<u32, Fallback>;
}

/// `cmpltps`: each lane all ones where `a < b`, ordered.
pub(crate) fn below(sse: &mut Sse, a: u128, b: u128) -> u128 {
    simd::cmpps(&mut sse.e.mxcsr, a, b, 1).unwrap_or_else(|simd::Xm| {
        sse.e.trapped = true;
        0
    })
}

impl Run<'_, ()> {
    /// [`triangle_case`]'s pair `k` for [`Penetration`]: A's vertex `ai`,
    /// the difference to B's placed vertex `bi`, and B's own.
    fn pair_out(&mut self, j: &mut Journal, k: u32, ai: u32, bi: u32) -> Result<(), Fallback> {
        let av = vec(j, self.slot(state::SUPPORT_A, ai))?;
        let d = self.sse.sub(av, vec(j, self.slot(state::SUPPORT_B, bi))?);
        put(j, self.frame(tri_frame::DIFFERENCES + 16 * k), d)?;
        put(j, self.frame(tri_frame::SIMPLEX + 16 * k), av)?;
        let local = vec(j, self.slot(state::LOCAL_B, bi))?;
        put(j, self.frame(tri_frame::SIMPLEX_B + 16 * k), local)
    }

    /// Whether `u - v` is past [`SAME`] in any of `x`, `y`, `z`.
    fn apart(&mut self, j: &Journal, u: u128, v: u128) -> Result<bool, Fallback> {
        let d = self.sse.sub(u, v) & vec(j, ABS)?;
        let same = vec(j, SAME)?;
        Ok(signs(below(&mut self.sse, same, d)) & 7 != 0)
    }

    /// `0x00303b80`, thiscall on the state, plain `ret`, for simplices
    /// over more than four vertices. A vertex of one side inside the
    /// other's triangle ([`triangle`], not oriented) becomes that side's
    /// only vertex, A's tested first. Otherwise each pair of edges, one
    /// of each side (a triangle's three, a segment's one), is measured
    /// with [`segments`], and a triangle keeps the vertices of its edge
    /// in the nearest pair (the first on a tie): its third takes the
    /// last vertex. B's arrays both follow, A's parallel array does not.
    fn reduce(&mut self, j: &mut Journal) -> Result<(), Fallback> {
        // `push ebp; and esp,-16; sub esp,0x74`, three pushes, from the
        // return address below the caller's frame.
        let g = (self.f.wrapping_sub(8) & !15).wrapping_sub(0x80);
        let (a, b, lb) = (state::SUPPORT_A, state::SUPPORT_B, state::LOCAL_B);
        let (count_a, count_b) = (self.at(state::COUNT_A), self.at(state::COUNT_B));
        if word(j, count_b)? == 3 {
            let mut i = 0u32;
            while (i as i32) < word(j, count_a)? as i32 && i < MAX_COUNT {
                if triangle(j, self.st, self.slot(a, i), self.at(b), 0, self.fpu)?.mask == 7 {
                    moved(j, self.at(a), self.slot(a, i))?;
                    return set(j, count_a, 1);
                }
                i += 1;
            }
        }
        if word(j, count_a)? == 3 {
            let mut i = 0u32;
            while (i as i32) < word(j, count_b)? as i32 && i < MAX_COUNT {
                if triangle(j, self.st, self.slot(b, i), self.at(a), 0, self.fpu)?.mask == 7 {
                    moved(j, self.at(lb), self.slot(lb, i))?;
                    moved(j, self.at(b), self.slot(b, i))?;
                    return set(j, count_b, 1);
                }
                i += 1;
            }
        }
        let edges = |n: u32| if n == 3 { 3 } else { 1 };
        let (edges_a, edges_b) = (edges(word(j, count_a)?), edges(word(j, count_b)?));
        let (mut best, mut best_a, mut best_b) = (MAX_BITS, 0usize, 0usize);
        let (da, db, out) = (g.wrapping_add(0x40), g.wrapping_add(0x30), g.wrapping_add(0x50));
        for i in 0..edges_a {
            for k in 0..edges_b {
                let ai = vec(j, self.slot(a, i as u32))?;
                let ea = self.sse.sub(vec(j, self.slot(a, FIRST[i]))?, ai);
                put(j, da, ea)?;
                let bk = vec(j, self.slot(b, k as u32))?;
                let eb = self.sse.sub(vec(j, self.slot(b, FIRST[k]))?, bk);
                put(j, db, eb)?;
                let d = segments(j, self.slot(a, i as u32), da, self.slot(b, k as u32), db, out, self.fpu)?.distance;
                // `fcomp; test ah,5; jp`: only an ordered nearer pair.
                if cmp(load(d), load(best)) == Cmp::Less {
                    (best, best_a, best_b) = (d, i, k);
                }
            }
        }
        if edges_a == 3 {
            let n = word(j, count_a)?.wrapping_sub(1);
            set(j, count_a, n)?;
            moved(j, self.slot(a, SECOND[best_a]), self.slot(a, n))?;
        }
        if edges_b == 3 {
            let n = word(j, count_b)?.wrapping_sub(1);
            set(j, count_b, n)?;
            let c = SECOND[best_b];
            moved(j, self.slot(lb, c), self.slot(lb, n))?;
            moved(j, self.slot(b, c), self.slot(b, word(j, count_b)?))?;
        }
        done(&self.sse)
    }
}

/// `0x00303dd0`, thiscall on the query state, `(shape A, shape B,
/// transform, cache)`, `ret 0x10`; the cache is not read and `args.out`
/// is unused. `esp` is the stack pointer at entry.
///
/// The simplices go to [`Penetration`] as up to four vertex pairs: B's
/// vertices against A's first when B has three or more, else A's against
/// B's first when A has three or more, else the first pair alone; a
/// fourth pair from the other side's second vertex fills a three. What
/// it leaves comes back as A's and B's first three vertices. One pair
/// left: the normal is `(1, 0, 0, 0)`, both sizes 1. Otherwise both are
/// three less the vertices within [`SAME`] of another (A's third against
/// its second and first, then its first against its second, the last
/// vertex taking the first's place; B's the same in B's frame, the last
/// test only with two left), B's are placed into A's frame, more than
/// four in all are cut down (`0x00303b80`), and [`query`]'s cases run
/// until one leaves the loop. The normal is the direction negated.
/// Returns the call's eax.
pub fn triangle_case(j: &mut Journal, st: u32, esp: u32, args: Args, fpu: Fpu, calls: &mut impl Penetration) -> Result<u32, Fallback> {
    let (sse, x) = units(fpu)?;
    // `push ebp; and esp,-16; sub esp,0x134`, then three pushes.
    let f = (esp.wrapping_sub(4) & !15).wrapping_sub(0x140);
    let mut r = Run { st, f, args, fpu, sse, x, calls: &mut (), edges: (0xb0, 0xc0) };
    let (a, b, lb) = (state::SUPPORT_A, state::SUPPORT_B, state::LOCAL_B);
    let (count_a, count_b) = (r.at(state::COUNT_A), r.at(state::COUNT_B));
    let na = word(j, count_a)? as i32;
    let nb = word(j, count_b)? as i32;
    let fr = |off: u32| f.wrapping_add(off);
    r.pair_out(j, 0, 0, 0)?;
    let count = if nb >= 3 {
        r.pair_out(j, 1, 0, 1)?;
        r.pair_out(j, 2, 0, 2)?;
        if nb > 3 {
            r.pair_out(j, 3, 0, 3)?;
            4
        } else if na > 1 {
            r.pair_out(j, 3, 1, 0)?;
            4
        } else {
            3
        }
    } else if na >= 3 {
        r.pair_out(j, 1, 1, 0)?;
        r.pair_out(j, 2, 2, 0)?;
        if na > 3 {
            r.pair_out(j, 3, 3, 0)?;
            4
        } else if nb > 1 {
            r.pair_out(j, 3, 0, 1)?;
            4
        } else {
            3
        }
    } else {
        1
    };
    set(j, fr(tri_frame::COUNT), count)?;
    done(&r.sse)?;
    let out = [args.a, args.b, args.transform, fr(tri_frame::SIMPLEX), fr(tri_frame::DIFFERENCES), fr(tri_frame::DIRECTION)];
    let eax = calls.penetration(j, out, f.wrapping_sub(0x1c))?;
    set(j, fr(tri_frame::EAX), eax)?;
    for i in 0..3 {
        moved(j, r.slot(a, i), fr(tri_frame::SIMPLEX + 16 * i))?;
        moved(j, r.slot(lb, i), fr(tri_frame::SIMPLEX_B + 16 * i))?;
    }
    if word(j, fr(tri_frame::COUNT))? == 1 {
        for (i, v) in [ONE_BITS, 0, 0, 0].into_iter().enumerate() {
            set(j, r.at(state::NORMAL + 4 * i as u32), v)?;
        }
        set(j, count_b, 1)?;
        set(j, count_a, 1)?;
        return Ok(eax);
    }
    set(j, count_b, 3)?;
    set(j, count_a, 3)?;
    // 0x00304013: A's duplicates.
    let (a1, a2) = (vec(j, r.slot(a, 1))?, vec(j, r.slot(a, 2))?);
    if !r.apart(j, a1, a2)? || !r.apart(j, vec(j, r.at(a))?, a2)? {
        set(j, count_a, 2)?;
    }
    if !r.apart(j, vec(j, r.at(a))?, a1)? {
        let n = word(j, count_a)?.wrapping_sub(1);
        set(j, count_a, n)?;
        moved(j, r.at(a), r.slot(a, n))?;
    }
    // 0x003040ac: B's, in B's frame.
    let (l1, l2) = (vec(j, r.slot(lb, 1))?, vec(j, r.slot(lb, 2))?);
    if !r.apart(j, l1, l2)? || !r.apart(j, vec(j, r.at(lb))?, l2)? {
        dec(j, count_b)?;
    }
    if word(j, count_b)? as i32 >= 2 && !r.apart(j, vec(j, r.at(lb))?, l1)? {
        let n = word(j, count_b)?.wrapping_sub(1);
        set(j, count_b, n)?;
        moved(j, r.at(lb), r.slot(lb, n))?;
    }
    let nb = word(j, count_b)?;
    for i in 0..nb.min(MAX_COUNT) {
        let v = vec(j, r.slot(lb, i))?;
        let placed = r.place(j, v)?;
        put(j, r.slot(b, i), placed)?;
    }
    if word(j, count_a)?.wrapping_add(word(j, count_b)?) as i32 > 4 {
        r.reduce(j)?;
    }
    set(j, r.at(state::FEATURE), 0)?;
    while let Flow::Dispatch = r.case(j)? {}
    let normal = vec(j, fr(tri_frame::DIRECTION))? ^ vec(j, SIGNS)?;
    put(j, r.at(state::NORMAL), normal)?;
    done(&r.sse)?;
    Ok(eax)
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

    fn get4(j: &Journal, at: u32) -> [f32; 4] {
        std::array::from_fn(|i| f32::from_bits(j.get(at + 4 * i as u32).unwrap()))
    }

    fn constants(h: &mut Heap) {
        h.0.insert(ONE, ONE_BITS);
        h.0.insert(PARALLEL, 0x3580_0000);
        for i in 0..4 {
            h.0.insert(SIGNS + 4 * i, 0x8000_0000);
        }
    }

    #[test]
    fn shuffles_pick_two_lanes_from_each_side() {
        let d = 0x4444_4444_3333_3333_2222_2222_1111_1111u128;
        let s = 0x8888_8888_7777_7777_6666_6666_5555_5555u128;
        assert_eq!(shuf2(d, s, 0x88), 0x7777_7777_5555_5555_3333_3333_1111_1111);
        assert_eq!(shuf2(d, s, 0xdd), 0x8888_8888_6666_6666_4444_4444_2222_2222);
        assert_eq!(shuf1(d, 0xc9), 0x4444_4444_1111_1111_3333_3333_2222_2222);
        assert_eq!(signs(0x8000_0000_0000_0000_8000_0000_0000_0000), 0b1010);
    }

    #[test]
    fn a_transform_in_its_own_frame_is_the_identity() {
        let mut h = Heap::default();
        let m = 0x1000;
        put4(&mut h, m, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, m + 0x10, [-1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, m + 0x20, [0.0, 0.0, 1.0, 0.0]);
        put4(&mut h, m + 0x30, [3.0, 4.0, 5.0, 1.0]);
        let mut j = Journal::new(&h);
        relative(&mut j, 0x2000, m, m, FPU).unwrap();
        assert_eq!(get4(&j, 0x2000)[..3], [1.0, 0.0, 0.0]);
        assert_eq!(get4(&j, 0x2010)[..3], [0.0, 1.0, 0.0]);
        assert_eq!(get4(&j, 0x2030)[..3], [0.0, 0.0, 0.0]);
        put4(&mut h, 0x3000, [1.0, 2.0, 3.0, 0.0]);
        let mut j = Journal::new(&h);
        rotate(&mut j, 0x2000, m, 0x3000, FPU).unwrap();
        assert_eq!(get4(&j, 0x2000)[..3], [-2.0, 1.0, 3.0]);
        assert_eq!(rotate(&mut Journal::new(&h), 0x2008, m, 0x3000, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn crossing_segments_meet_inside_and_parallel_ones_clamp() {
        let mut h = Heap::default();
        constants(&mut h);
        // x from -1 to 1 at z 0; y from -1 to 1 at z 2: closest at the middles.
        put4(&mut h, 0x100, [-1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x110, [2.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x120, [0.0, -1.0, 2.0, 0.0]);
        put4(&mut h, 0x130, [0.0, 2.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        let r = segments(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(r, Closest { flags: 0, distance: 4.0f32.to_bits() });
        assert_eq!(get4(&j, 0x200)[..3], [0.0, 0.0, 0.0]);
        assert_eq!(get4(&j, 0x210)[..3], [0.0, 0.0, -2.0]);
        // The second parallel to the first and past its end.
        put4(&mut h, 0x120, [5.0, 1.0, 0.0, 0.0]);
        put4(&mut h, 0x130, [1.0, 0.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        let r = segments(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(r.flags & 1, 1, "s clamps to the first segment's end");
        assert_eq!(get4(&j, 0x200)[..3], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_point_beyond_one_face_names_it() {
        let mut h = Heap::default();
        constants(&mut h);
        h.0.insert(TINY, 1e-6f32.to_bits());
        h.0.insert(MARGIN, 1.1f32.to_bits());
        for i in 0..4 {
            h.0.insert(ABS + 4 * i, 0x7fff_ffff);
        }
        let (verts, p) = (0x1000, 0x2000);
        put4(&mut h, verts, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x20, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x30, [0.0, 0.0, 1.0, 0.0]);
        // At v3 every test is zero: no face.
        put4(&mut h, p, [0.0, 0.0, 1.0, 0.0]);
        let one = 1.0f32.to_bits();
        assert_eq!(tetrahedron(&Journal::new(&h), verts, p, one, FPU).map(|f| f.eax), Ok(u32::MAX));
        // Far along -x, one face's test dominates.
        put4(&mut h, p, [-5.0, 0.1, 0.1, 0.0]);
        let face = tetrahedron(&Journal::new(&h), verts, p, one, FPU).unwrap();
        assert!(face.eax < 3, "{face:?}");
        assert_eq!(face.eax, face.ecx);
        assert_eq!(tetrahedron(&Journal::new(&h), verts + 8, p, one, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn a_segment_through_a_triangle_meets_it_and_one_beside_it_drops_a_vertex() {
        let mut h = Heap::default();
        constants(&mut h);
        for i in 0..4 {
            h.0.insert(ONES + 4 * i, ONE_BITS);
        }
        let (st, seg, tri, counts, esp) = (0x1000, 0x2000, 0x3000, 0x4000, 0x8000);
        put4(&mut h, tri, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x20, [0.0, 1.0, 0.0, 0.0]);
        h.0.insert(counts, 2);
        h.0.insert(counts + 4, 3);
        let run = |h: &Heap, fresh: u32| {
            let mut j = Journal::new(h);
            let e = edge(&mut j, st, esp, seg, tri, counts, counts + 4, fresh, 9, FPU);
            (e, j.get(counts).unwrap(), j.get(counts + 4).unwrap())
        };
        // Through the middle: it meets the plane inside.
        put4(&mut h, seg, [0.25, 0.25, 1.0, 0.0]);
        put4(&mut h, seg + 0x10, [0.25, 0.25, -1.0, 0.0]);
        assert_eq!(run(&h, 1), (Ok(Edge { eax: 1, ecx: st, edx: tri }), 2, 3));
        // Above the triangle, nearer end first: the segment drops to it.
        put4(&mut h, seg + 0x10, [0.25, 0.25, 2.0, 0.0]);
        assert_eq!(run(&h, 1).0.map(|e| e.eax), Ok(0));
        assert_eq!(run(&h, 1).1, 1);
        // Off to the side: the triangle loses a vertex to t2.
        put4(&mut h, seg, [3.0, 3.0, 1.0, 0.0]);
        put4(&mut h, seg + 0x10, [4.0, 4.0, 1.0, 0.0]);
        let (e, a, b) = run(&h, 1);
        let e = e.unwrap();
        assert_eq!((e.eax, e.ecx & !0x10, a, b), (2, 0, 2, 2), "{e:?}");
        // Only the low byte says fresh; a zero normal never crosses.
        assert_eq!(run(&h, 0x100).0.map(|e| e.eax), Ok(2));
        assert_eq!(run(&h, 0x100).0.map(|e| e.ecx), Ok(st));
        let mut j = Journal::new(&h);
        assert_eq!(edge(&mut j, st, esp, seg + 8, tri, counts, counts + 4, 1, 9, FPU), Err(Fallback::Memory));
    }

    /// Capsules through the ported virtuals; the triangle case answers
    /// a fixed eax.
    struct Capsules {
        triangle: u32,
    }

    impl Calls for Capsules {
        fn vertices(&mut self, j: &mut Journal, shape: u32, ids: u32, count: u32, out: u32, _: u32) -> Result<(), Fallback> {
            crate::shape_support::capsule_vertices(j, shape, ids, count, out)
        }

        fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, _: u32) -> Result<(), Fallback> {
            let mut x = x87::abi::X87Abi::default();
            x87::abi::init(&mut x);
            x.control = FPU.control;
            let mut m = FPU.mxcsr;
            crate::shape_support::capsule(j, shape, dir, out, &mut m, &mut x).map(|_| ())
        }

        fn triangle_case(&mut self, _: &mut Journal, _: u32, _: Args, _: u32) -> Result<u32, Fallback> {
            Ok(self.triangle)
        }
    }

    #[test]
    fn two_capsules_apart_give_the_gap_between_them() {
        let mut h = Heap::default();
        constants(&mut h);
        h.0.insert(STALL, 0x3400_0000);
        let (st, a, b, t, c, out, esp) = (0x1000, 0x2000, 0x2100, 0x2200, 0x2300, 0x2400, 0x8000);
        for shape in [a, b] {
            put4(&mut h, shape + 0x10, [0.0, 0.0, -1.0, 0.25]);
            put4(&mut h, shape + 0x20, [0.0, 0.0, 1.0, 0.25]);
        }
        put4(&mut h, t, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, t + 0x10, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, t + 0x20, [0.0, 0.0, 1.0, 0.0]);
        put4(&mut h, t + 0x30, [5.0, 0.0, 0.0, 1.0]);
        // One vertex each (end A, ID 0), each may grow to two.
        h.0.insert(c + 8, 0x0202_0101);
        h.0.insert(st + state::SETTLE, 1);
        let args = Args { a, b, transform: t, cache: c, out };
        let mut j = Journal::new(&h);
        let q = query(&mut j, st, esp, args, FPU, &mut Capsules { triangle: 7 }).unwrap();
        // Parallel capsules: the other ends are no nearer, so neither grows.
        assert_eq!(q, Query { eax: 0, ecx: 1, edx: 1 });
        let n = get4(&j, out);
        assert!((n[0] - 1.0).abs() < 1e-3 && n[1] == 0.0 && n[2] == 0.0, "{n:?}");
        assert!((n[3] - 5.0).abs() < 1e-2, "the distance in w: {n:?}");
        assert_eq!((j.get(c), j.get(c + 8)), (Some(0), Some(0x0202_0101)));
        assert_eq!(query(&mut Journal::new(&h), st + 8, esp, args, FPU, &mut Capsules { triangle: 7 }), Err(Fallback::Memory));
    }

    #[test]
    fn a_query_with_no_case_stalls() {
        let mut h = Heap::default();
        constants(&mut h);
        let (st, c, out) = (0x1000, 0x2300, 0x2400);
        let args = Args { a: 0x2000, b: 0x2100, transform: 0x2200, cache: c, out };
        // No vertices: no case, out with 1; the +0x0c byte says 3.
        h.0.insert(st + state::SETTLE, 1);
        let q = query(&mut Journal::new(&h), st, 0x8000, args, FPU, &mut Capsules { triangle: 7 }).unwrap();
        assert_eq!(q, Query { eax: 3, ecx: 0, edx: 1 });
        // Clear, the triangle case answers.
        h.0.insert(st + state::SETTLE, 0);
        let q = query(&mut Journal::new(&h), st, 0x8000, args, FPU, &mut Capsules { triangle: 7 }).unwrap();
        assert_eq!(q.eax, 7);
    }

    /// A penetration call that leaves a fixed simplex and direction, and
    /// keeps what it was handed.
    struct Leaves {
        count: u32,
        a: [[f32; 4]; 3],
        b: [[f32; 4]; 3],
        direction: [f32; 4],
        seen: Option<([u32; 6], u32, u32)>,
    }

    impl Penetration for Leaves {
        fn penetration(&mut self, j: &mut Journal, args: [u32; 6], esp: u32) -> Result<u32, Fallback> {
            let simplex = args[3];
            self.seen = Some((args, esp, j.get(simplex + 0x80).unwrap()));
            let mut put4 = |at: u32, v: [f32; 4]| {
                for (i, x) in v.into_iter().enumerate() {
                    j.put(at + 4 * i as u32, x.to_bits()).unwrap();
                }
            };
            for i in 0..3u32 {
                put4(simplex + 16 * i, self.a[i as usize]);
                put4(simplex + 0x40 + 16 * i, self.b[i as usize]);
            }
            put4(args[5], self.direction);
            j.put(simplex + 0x80, self.count).unwrap();
            Ok(0x55)
        }
    }

    const TRI_ARGS: Args = Args { a: 0x2000, b: 0x2100, transform: 0x2200, cache: 0x2300, out: 0 };

    /// The state for a triangle case: the sizes, the identity transform,
    /// and the constants the duplicate tests read.
    fn triangle_heap(na: u32, nb: u32) -> Heap {
        let mut h = Heap::default();
        constants(&mut h);
        for i in 0..4 {
            h.0.insert(ABS + 4 * i, 0x7fff_ffff);
            h.0.insert(SAME + 4 * i, 1e-4f32.to_bits());
        }
        let t = TRI_ARGS.transform;
        put4(&mut h, t, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, t + 0x10, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, t + 0x20, [0.0, 0.0, 1.0, 0.0]);
        put4(&mut h, t + 0x30, [0.0; 4]);
        h.0.insert(0x1000 + state::COUNT_A, na);
        h.0.insert(0x1000 + state::COUNT_B, nb);
        h
    }

    #[test]
    fn a_triangle_case_left_one_pair_faces_x() {
        let h = triangle_heap(1, 1);
        let (st, esp) = (0x1000, 0x8000);
        let mut calls = Leaves { count: 1, a: [[2.0, 0.0, 0.0, 0.0]; 3], b: [[3.0, 0.0, 0.0, 0.0]; 3], direction: [0.0, 0.0, 1.0, 0.0], seen: None };
        let mut j = Journal::new(&h);
        assert_eq!(triangle_case(&mut j, st, esp, TRI_ARGS, FPU, &mut calls), Ok(0x55));
        // `and esp,-16` from 0x7ffc, then 0x140 of frame.
        let f = 0x7eb0;
        let (args, call_esp, count) = calls.seen.unwrap();
        assert_eq!(args, [0x2000, 0x2100, 0x2200, f + 0x20, f + 0xd0, f + 0x110]);
        assert_eq!((call_esp, count), (f - 0x1c, 1));
        assert_eq!(get4(&j, st + state::NORMAL), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!((j.get(st + state::COUNT_A), j.get(st + state::COUNT_B)), (Some(1), Some(1)));
        assert_eq!(get4(&j, st + state::SUPPORT_A), [2.0, 0.0, 0.0, 0.0]);
        assert_eq!(get4(&j, st + state::LOCAL_B), [3.0, 0.0, 0.0, 0.0]);
        assert_eq!(triangle_case(&mut Journal::new(&h), st + 8, esp, TRI_ARGS, FPU, &mut calls), Err(Fallback::Memory));
    }

    #[test]
    fn a_triangle_case_drops_repeated_vertices_and_turns_the_direction() {
        // B's triangle at entry: three pairs.
        let h = triangle_heap(1, 3);
        let (st, esp) = (0x1000, 0x8000);
        let tri = [[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        let mut calls = Leaves { count: 3, a: [[0.25, 0.25, 1.0, 0.0]; 3], b: tri, direction: [0.0, 0.0, 1.0, 0.5], seen: None };
        let mut j = Journal::new(&h);
        assert_eq!(triangle_case(&mut j, st, esp, TRI_ARGS, FPU, &mut calls), Ok(0x55));
        assert_eq!(calls.seen.unwrap().2, 3);
        // A's three are one.
        assert_eq!(j.get(st + state::COUNT_A), Some(1));
        assert_eq!(get4(&j, st + state::NORMAL), [-0.0, -0.0, -1.0, -0.5]);
        assert_eq!(j.get(st + state::FEATURE), Some(0));
    }

    #[test]
    fn two_triangles_are_cut_to_four_vertices() {
        let h = triangle_heap(3, 3);
        let (st, esp) = (0x1000, 0x8000);
        let a = [[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        let b = [[5.0, 5.0, 1.0, 0.0], [6.0, 5.0, 2.0, 0.0], [5.0, 6.0, 3.0, 0.0]];
        let mut calls = Leaves { count: 3, a, b, direction: [1.0, 0.0, 0.0, 0.0], seen: None };
        let mut j = Journal::new(&h);
        assert_eq!(triangle_case(&mut j, st, esp, TRI_ARGS, FPU, &mut calls), Ok(0x55));
        let (na, nb) = (j.get(st + state::COUNT_A).unwrap(), j.get(st + state::COUNT_B).unwrap());
        assert!(na + nb <= 4 && na >= 1 && nb >= 1, "({na}, {nb})");
        assert_eq!(get4(&j, st + state::NORMAL), [-1.0, -0.0, -0.0, -0.0]);
    }

    #[test]
    fn a_point_inside_a_triangle_sets_every_side() {
        let mut h = Heap::default();
        constants(&mut h);
        let (st, tri, p) = (0x1000, 0x2000, 0x3000);
        put4(&mut h, tri, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x20, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, p, [0.25, 0.25, 1.0, 0.0]);
        let inside = triangle(&mut Journal::new(&h), st, p, tri, 0, FPU).unwrap().mask;
        put4(&mut h, p, [2.0, 2.0, 1.0, 0.0]);
        let outside = triangle(&mut Journal::new(&h), st, p, tri, 0, FPU).unwrap().mask;
        assert!(inside == 0 || inside == 7, "{inside:#b}");
        assert_ne!(inside, outside);
        assert_eq!(triangle(&mut Journal::new(&h), st + 8, p, tri, 0, FPU), Err(Fallback::Memory));
    }
}
